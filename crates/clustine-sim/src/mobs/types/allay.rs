//! The allay (`Allay`, `AllayAi`, `InventoryCarrier`; `docs/groundwork/n7-brief.md`,
//! section 7.10): a flying helper, not an animal, that holds in its hand the kind of item
//! a player gave it, gathers items of that kind into a slot of its own and brings them to
//! that player or to a note block it heard. It dances by a playing jukebox, and dancing
//! is duplicated by an amethyst shard; it heals a point every half a second, takes no
//! fall and never despawns, and the player it likes cannot hurt it.
//!
//! Everything here but its brain's activities is built (`docs/groundwork/n7j-plan.md`,
//! section 4.2): its flight (`travelFlying`, the flying move control that hovers), its
//! state, its synced data, the gift of an item and its taking back, the duplication with
//! its cooldown, the healing, the dance's end, the pick-up of what it wants, its drops
//! and sounds. Until the brain comes it has no goals: it hovers where it is, and picks up
//! only what comes within a block of it. [`brain_seams`] are what its behaviours and its
//! two listeners will call.
//!
//! What its brain keeps it keeps here meanwhile: the player it likes (`LIKED_PLAYER`),
//! which the gift sets and the taking back clears and which its hurts read, goes into
//! the brain's memories when they come.
//!
//! Named differences: the jukebox and the note block are heard through entity listeners
//! that move with it (`DynamicGameEventListener`), which `region/vibrations.rs` does not
//! have yet, so it never starts dancing by itself; its arm's swing as a player takes its
//! item is not shown; a copy made by duplication faces south (the game keeps the yaw its
//! constructor drew); its steps on the ground sound as the block's (its own are silent);
//! what it throws keeps no thrower.

use clustine_blocks::data::{is_class, item_in_tag};
use clustine_data::{BlockClass, entity_types, sounds};
use clustine_world::{BlockPos, EntityData, EntityId, ItemStack, TypeState};
use serde::{Deserialize, Serialize};

use super::{Did, Interaction, MobType, Used};
use crate::living::{Travel, slots};
use crate::mobs::Mob;
use crate::mobs::ai::navigation::{Kind, Settings};
use crate::mobs::ai::{Ai, MoveKind, Operation, Setup, controls};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::{Local, MobTick};
use crate::physics::aabb::Aabb;
use crate::physics::item::{SOUND_SOURCE_NEUTRAL, max_stack_size};
use crate::region::DEFAULT_PICK_UP_DELAY;
use crate::shown::DataShown;

/// The version of an allay's type state.
const VERSION: u8 = 1;

/// The synced data's indices: dancing and whether it may duplicate (`PathfinderMob`
/// defines none after `Mob`'s 15), and every entity's `NoGravity`.
pub(crate) const DANCING_INDEX: u8 = 16;
pub(crate) const CAN_DUPLICATE_INDEX: u8 = 17;
const NO_GRAVITY_INDEX: u8 = 5;

/// `DUPLICATION_COOLDOWN_TICKS`.
pub(crate) const DUPLICATION_COOLDOWN: u32 = 6000;

/// How far a playing jukebox keeps it dancing (`shouldStopDancing`'s 10, and
/// `JUKEBOX_PLAY`'s notification radius).
const JUKEBOX_REACH: f64 = 10.0;

/// `0.3f` widened: how far below its eyes it throws from (`BehaviorUtils.throwItem`).
const THROW_DROP: f64 = 0.300_000_011_920_928_96;

/// What an allay keeps: whether it dances and to which jukebox (neither saved by the
/// game), when its duplication's cooldown ends (`duplicationCooldown`, kept as the moment
/// it comes to nothing, and none once it has: `DATA_CAN_DUPLICATE`), the player it likes,
/// the one slot of its inventory, and whether its flight has set `NoGravity`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct AllayState {
    pub(crate) dancing: bool,
    pub(crate) jukebox: Option<BlockPos>,
    pub(crate) cooldown_until: Option<u32>,
    pub(crate) liked_player: Option<u128>,
    pub(crate) inventory: Option<ItemStack>,
    pub(crate) no_gravity: bool,
}

impl AllayState {
    /// `canDuplicate()`: its cooldown has come to nothing.
    pub(crate) fn can_duplicate(&self) -> bool {
        self.cooldown_until.is_none()
    }

    /// `duplicationCooldown` at the moment `now`.
    pub(crate) fn cooldown(&self, now: u32) -> i32 {
        self.cooldown_until
            .map_or(0, |until| (i64::from(until) - i64::from(now)).max(0) as i32)
    }
}

/// An allay's state from its type state; a record of nothing, or one this build cannot
/// read, as a fresh allay's.
pub(crate) fn read(mob: &Mob) -> AllayState {
    if mob.state.bytes.is_empty() || mob.state.version != VERSION {
        return AllayState::default();
    }
    postcard::from_bytes(&mob.state.bytes).unwrap_or_default()
}

fn put(mob: &mut Mob, own: &AllayState) {
    mob.state = if *own == AllayState::default() {
        TypeState::default()
    } else {
        TypeState {
            version: VERSION,
            bytes: postcard::to_allocvec(own).expect("a type state is encoded into memory"),
        }
    };
}

/// Changes an allay's state.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut AllayState)) {
    let mut own = read(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        put(mob, &own);
    }
}

/// `resetDuplicationCooldown()` at the moment `now`: 6000 ticks until it may again.
pub(crate) fn reset_duplication_cooldown(own: &mut AllayState, now: u32) {
    own.cooldown_until = Some(now.saturating_add(DUPLICATION_COOLDOWN));
}

/// `allayConsidersItemEqual(a, b)`: the same item (and the same potion's contents, which
/// no stack carries yet).
fn considers_equal(a: ItemStack, b: ItemStack) -> bool {
    a.item == b.item
}

/// `wantsToPickUp(level, stack)`: it holds something of the same kind and its slot can
/// take the stack (`mobGriefing` is always on).
pub(crate) fn wants_to_pick_up(mob: &Mob, stack: ItemStack) -> bool {
    let Some(hand) = mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] else {
        return false;
    };
    considers_equal(hand, stack) && can_add_item(read(mob).inventory, stack)
}

/// `SimpleContainer.canAddItem(stack)` of its one slot: empty, or of the same item and
/// not full.
fn can_add_item(slot: Option<ItemStack>, stack: ItemStack) -> bool {
    match slot.filter(|held| held.count > 0) {
        None => true,
        Some(held) => held.item == stack.item && held.count < max_stack_size(held),
    }
}

/// `SimpleContainer.addItem(stack)` of its one slot: how many of the stack go in.
fn add_item(slot: &mut Option<ItemStack>, stack: ItemStack) -> i32 {
    match slot.filter(|held| held.count > 0) {
        None => {
            *slot = Some(stack);
            stack.count
        }
        Some(held) if held.item == stack.item => {
            let moved = (max_stack_size(held) - held.count).clamp(0, stack.count);
            *slot = Some(ItemStack {
                count: held.count + moved,
                ..held
            });
            moved
        }
        Some(_) => 0,
    }
}

/// The seams of its brain and its listeners (`AllayAi`, `Allay$JukeboxListener`,
/// `Allay$VibrationUser`), which part A's machinery and the entity listeners of
/// `region/vibrations.rs` will call.
#[allow(
    dead_code,
    reason = "what the allay's brain and listeners call once they are wired"
)]
pub(crate) mod brain_seams {
    use super::*;

    /// `setDancing(dancing)`: a panicking allay does not start.
    pub(crate) fn set_dancing(mob: &mut Mob, dancing: bool, panicking: bool) {
        if dancing && panicking {
            return;
        }
        update(mob, |own| own.dancing = dancing);
    }

    /// `setJukeboxPlaying(pos, playing)`: a jukebox that starts sets it dancing, where it
    /// does not already; one that stops (or any, where it knows none) ends it.
    pub(crate) fn set_jukebox_playing(mob: &mut Mob, pos: BlockPos, playing: bool) {
        let own = read(mob);
        if playing {
            if !own.dancing {
                update(mob, |own| own.jukebox = Some(pos));
                set_dancing(mob, true, false);
            }
        } else if own.jukebox == Some(pos) || own.jukebox.is_none() {
            update(mob, |own| {
                own.jukebox = None;
                own.dancing = false;
            });
        }
    }

    /// `hasItemToThrow`: its slot holds something.
    pub(crate) fn has_item_to_throw(mob: &Mob) -> bool {
        read(mob).inventory.is_some_and(|stack| stack.count > 0)
    }

    /// `AllayAi.throwItem`'s first step: one item out of its slot to throw.
    pub(crate) fn take_one_to_throw(mob: &mut Mob) -> Option<ItemStack> {
        let mut taken = None;
        update(mob, |own| {
            if let Some(stack) = own.inventory.filter(|stack| stack.count > 0) {
                taken = Some(ItemStack { count: 1, ..stack });
                own.inventory = (stack.count > 1).then_some(ItemStack {
                    count: stack.count - 1,
                    ..stack
                });
            }
        });
        taken
    }

    /// `THROW_SOUND_PITCHES`, one of which a throw's sound is at.
    pub(crate) const THROW_SOUND_PITCHES: [f32; 16] = [
        0.5625, 0.625, 0.75, 0.9375, 1.0, 1.0, 1.125, 1.25, 1.5, 1.875, 2.0, 2.25, 2.5, 3.0, 3.75,
        4.0,
    ];
}

/// `shouldStopDancing()`: it knows no jukebox, is ten or more from its middle, or the
/// block there is no jukebox any more.
fn should_stop_dancing(tick: &MobTick<'_>, own: &AllayState) -> bool {
    let Some(jukebox) = own.jukebox else {
        return true;
    };
    !super::turtle::closer_to_centre_than(jukebox, tick.living.body.position, JUKEBOX_REACH)
        || !is_class(tick.world.state(jukebox), BlockClass::JukeboxBlock)
}

/// `Mob.aiStep`'s pick-up (`canPickUpLoot`: its hand holds something; the brain's pick-up
/// cooldown is not kept yet): each item within a block of its box that it wants, as much
/// of it as its slot takes (`InventoryCarrier.pickUpItem`), shown as taken.
fn pick_up(tick: &mut MobTick<'_>) {
    if tick.mob.body.vitals.dying()
        || tick.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)].is_none()
    {
        return;
    }
    let body = &tick.living.body;
    let bounds = Aabb::of_entity(body.position, body.width, body.height).inflate([1.0, 1.0, 1.0]);
    let found: Vec<(EntityId, ItemStack)> = tick
        .world
        .entities_in(&bounds)
        .iter()
        .filter(|seen| !seen.copied)
        .filter_map(|seen| match seen.entity.data {
            EntityData::Item {
                stack,
                pick_up_delay: 0,
                ..
            } if stack.count > 0 => Some((seen.entity.key, stack)),
            _ => None,
        })
        .collect();
    for (key, stack) in found {
        if !wants_to_pick_up(&tick.mob, stack) {
            continue;
        }
        let mut moved = 0;
        update(&mut tick.mob, |own| {
            moved = add_item(&mut own.inventory, stack)
        });
        if moved > 0 {
            tick.out.local.push(Local::TakeSome {
                item: key,
                count: moved,
            });
        }
    }
}

/// `Allay.mobInteract` (the brief's 7.10.5): dancing, an amethyst shard duplicates it;
/// an empty hand of its own takes what the player holds, one of it, and likes them; a
/// player with an empty main hand takes its item back, and what it gathered falls at
/// its eyes. The player keeps what they gave (every player is creative), and the sounds
/// are not sent to them, whose client plays its own.
fn mob_interact(used: &mut Used<'_>) -> Interaction {
    let hand = usize::from(slots::MAIN_HAND);
    let held = used.held.filter(|stack| stack.count > 0);
    let own = read(used.mob);
    let holding = used.mob.body.equipment.slots[hand].filter(|stack| stack.count > 0);
    let sound = |sound: i32| Did::Sound {
        sound,
        source: SOUND_SOURCE_NEUTRAL,
        at: used.position,
        volume: 2.0,
        pitch: 1.0,
        but_the_player: true,
    };
    if own.dancing
        && held.is_some_and(|stack| item_in_tag(stack.item, "minecraft:duplicates_allays"))
        && own.can_duplicate()
    {
        // `duplicateAllay()`: a copy where it is, kept, both waiting 6000 ticks.
        let mut copy = Mob::fresh();
        copy.body.flags.persistent = true;
        update(&mut copy, |made| reset_duplication_cooldown(made, 0));
        let now = used.moment;
        update(used.mob, |own| reset_duplication_cooldown(own, now));
        used.did.push(Did::Spawn {
            kind: used.kind,
            mob: Box::new(copy),
        });
        used.did.push(Did::Event(18));
        used.did.push(sound(sounds::BLOCK_AMETHYST_BLOCK_CHIME));
        return Interaction::Success;
    }
    if holding.is_none()
        && let Some(given) = held
    {
        used.mob.body.equipment.slots[hand] = Some(ItemStack { count: 1, ..given });
        used.did.push(sound(sounds::ENTITY_ALLAY_ITEM_GIVEN));
        let player = used.player_uuid;
        update(used.mob, |own| own.liked_player = player);
        return Interaction::Success;
    }
    if let Some(taken) = holding
        && held.is_none()
    {
        used.mob.body.equipment.slots[hand] = None;
        used.did.push(sound(sounds::ENTITY_ALLAY_ITEM_TAKEN));
        let gathered = own.inventory.filter(|stack| stack.count > 0);
        update(used.mob, |own| {
            own.inventory = None;
            own.liked_player = None;
        });
        if let Some(stack) = gathered {
            // `BehaviorUtils.throwItem(this, stack, position())`: aimed at its own feet,
            // so with no motion of its own.
            let info = clustine_data::EntityTypeInfo::of(entity_types::ALLAY);
            let [x, y, z] = used.position;
            used.did.push(Did::Throw {
                stack,
                at: [x, y + f64::from(info.eye_height) - THROW_DROP, z],
                pick_up_delay: DEFAULT_PICK_UP_DELAY,
                motion: [0.0; 3],
            });
        }
        used.did.push(Did::Give {
            stack: taken,
            no_duplicates: false,
        });
        return Interaction::Success;
    }
    Interaction::Pass
}

/// The allay.
pub(crate) struct Allay;

impl MobType for Allay {
    /// Its constructor's controls and `createNavigation`: a flying navigation that opens
    /// no doors, floats and plans 48 blocks, and a flying move control that turns 20 and
    /// hovers. No goals: its mind is its brain's.
    fn register_goals(&self, setup: &mut Setup) {
        setup.config.navigation = Settings::of(Kind::Flying);
        setup.config.navigation.flags.can_open_doors = false;
        setup.config.navigation.set_can_float(true);
        setup.config.navigation.set_required_path_length(48.0);
        setup.config.move_kind = MoveKind::Flying {
            max_turn: 20,
            hovers: true,
        };
    }

    /// `FlyingMoveControl.tick()` and its `NoGravity`, which a hovering one sets as it
    /// first steers and never clears.
    fn tick_move_control(&self, ai: &mut Ai<'_, '_>) {
        let steering = ai.tick.mob.body.controls.movement.operation == Operation::MoveTo;
        controls::tick_move(ai);
        if steering {
            update(&mut ai.tick.mob, |own| own.no_gravity = true);
        }
    }

    /// `Allay.customServerAiStep`: the brain's tick and its activity's update, which
    /// part A's machinery brings.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        let _ = tick;
    }

    /// `Allay.travel`: `travelFlying` at its speed in the air, 0.02 in water and lava.
    fn travel(&self, tick: &MobTick<'_>) -> Travel {
        Travel::Flying {
            water: 0.02,
            lava: 0.02,
            air: tick.mob.body.inputs.speed,
        }
    }

    /// `Allay.aiStep` after the living one: the pick-up (`Mob.aiStep`'s), then every
    /// tenth tick a point of health; every twentieth, the end of a dance it should stop;
    /// and its duplication's cooldown counted down.
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        pick_up(tick);
        let tick_count = tick.tick_count.wrapping_add(1);
        if !tick.mob.body.vitals.dying() && tick_count.is_multiple_of(10) {
            super::tamable::heal(&mut tick.mob, tick.kind, 1.0);
        }
        let own = read(&tick.mob);
        if own.dancing && should_stop_dancing(tick, &own) && tick_count.is_multiple_of(20) {
            update(&mut tick.mob, |own| {
                own.dancing = false;
                own.jukebox = None;
            });
        }
        // `updateDuplicationCooldown()`: down by one, `canDuplicate` again at nothing.
        let next = tick.lived.saturating_add(1);
        update(&mut tick.mob, |own| {
            if own.cooldown_until.is_some() && own.cooldown(next) == 0 {
                own.cooldown_until = None;
            }
        });
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        mob_interact(used)
    }

    /// `Allay.hurtServer`: the player it likes does it no harm.
    fn refuses_hurt(&self, mob: &Mob, source: &DamageSource) -> bool {
        let liked = read(mob).liked_player;
        liked.is_some()
            && source
                .cause
                .as_ref()
                .and_then(|cause| cause.player)
                .is_some_and(|player| Some(player) == liked)
    }

    /// `Allay.dropEquipment`: what it gathered, then what it holds.
    fn drop_equipment(&self, mob: &mut Mob) -> Vec<ItemStack> {
        let mut drops = Vec::new();
        let own = read(mob);
        if let Some(stack) = own.inventory.filter(|stack| stack.count > 0) {
            drops.push(stack);
        }
        update(mob, |own| own.inventory = None);
        if let Some(stack) = mob.body.equipment.slots[usize::from(slots::MAIN_HAND)].take() {
            drops.push(stack);
        }
        drops
    }

    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        Some(
            if mob.body.equipment.slots[usize::from(slots::MAIN_HAND)].is_some() {
                sounds::ENTITY_ALLAY_AMBIENT_WITH_ITEM
            } else {
                sounds::ENTITY_ALLAY_AMBIENT_WITHOUT_ITEM
            },
        )
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_ALLAY_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_ALLAY_DEATH)
    }

    fn sound_volume(&self) -> f32 {
        0.4
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let own = read(mob);
        vec![
            (
                NO_GRAVITY_INDEX,
                DataShown::Boolean(own.no_gravity),
                !own.no_gravity,
            ),
            (DANCING_INDEX, DataShown::Boolean(own.dancing), !own.dancing),
            (
                CAN_DUPLICATE_INDEX,
                DataShown::Boolean(own.can_duplicate()),
                own.can_duplicate(),
            ),
        ]
    }
}
