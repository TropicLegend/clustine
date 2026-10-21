//! The nautilus (`AbstractNautilus`, `Nautilus`; `docs/groundwork/n7-brief.md`, section
//! 7.12, with 9.7.5 and 10.7.5), and what the zombie nautilus shares with it, which
//! [`super::zombie_nautilus`] uses: a tamed animal of the water that dries out on land,
//! is tamed by a pufferfish or its bucket one time in three, eats fish to heal, to grow
//! and to fall in love once tamed, wears a saddle and nautilus armour once tamed and
//! grown, is kept near the place it was last left, and dashes under a rider.
//!
//! Its mind is a brain in the game (`NautilusAi`), whose machinery is not built yet:
//! until its provider is wired at [`brain_seam`], a nautilus has no mind and swims,
//! sinks or lies by its body alone. What the brain will read is kept or drawn where the
//! game draws it: the attack cooldown `finalizeSpawn` draws ([`NautilusState::
//! attack_target_cooldown`]), and whom a hurt angers ([`anger_target`]).
//!
//! Riding is the lead's (`docs/groundwork/n7j-plan.md`, section 4.2, part R): what it
//! needs is here as plain functions ([`steers`], [`ridden_input`], [`ridden_rotation`],
//! [`ridden_yaw`], [`ridden_speed`], [`on_player_jump`], [`execute_riders_jump`],
//! [`handle_start_jump`]), and nobody is taken on yet ([`ride`]).
//!
//! Named differences: a tamed nautilus clicked takes nobody on, and a sneaking click on
//! one opens no menu (its mount menu is the riding's); Breath of the Nautilus is given
//! to nobody, and poison is no different (no mob effects until N9); `isUnderWater` is
//! the one its last `baseTick` found, so a sound in the tick it goes under or comes up
//! is the medium's of the tick before; no mob plays a swimming sound yet; a use that
//! does nothing does not make it persistent (the game's `interact` does, whatever the
//! use).

use clustine_blocks::data::item_in_tag;
use clustine_data::{EntityTypeInfo, PathType, entity_types, items, sounds};
use clustine_world::{BlockPos, EntityId, ItemStack};
use serde::{Deserialize, Serialize};

use super::animal::{self, Own, variants};
use super::tamable::{self, Pet, Tame};
use super::{Breeding, Did, Dimensions, Interaction, MobType, Spawning, Used, water};
use crate::living::{Damage, Sinks, Travel, slots};
use crate::mobs::Mob;
use crate::mobs::ai::goals::Items;
use crate::mobs::ai::maths::{table_cos, table_sin, wrap_degrees};
use crate::mobs::ai::navigation::{Kind, Settings};
use crate::mobs::ai::{LookKind, MoveKind, Setup};
use crate::mobs::body::hurt::{DamageSource, voice_pitch};
use crate::mobs::body::{Event, Home};
use crate::mobs::tick::MobTick;
use crate::physics::Vec3;
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::region::Random;
use crate::shown::DataShown;

/// The synced index of `AbstractNautilus.DASH`.
pub(crate) const DASH_INDEX: u8 = 20;

/// The registry of the zombie nautilus's variants, whose default every nautilus's state
/// keeps.
pub(crate) const VARIANTS: &str = "minecraft:zombie_nautilus_variant";

/// `NAUTILUS_TOTAL_AIR_SUPPLY`.
const TOTAL_AIR: i32 = 300;

/// The ticks a dash keeps a nautilus from the next (`executeRidersJump`'s 40), and the
/// cooldown below which it no longer shows as dashing.
#[allow(dead_code, reason = "what the riding (part R) calls, which it wires")]
pub(crate) const DASH_COOLDOWN: i32 = 40;
const DASHING_UNTIL: i32 = 35;

/// `#nautilus_food`, `#nautilus_taming_items` and `#nautilus_bucket_food`.
const FOOD: &str = "minecraft:nautilus_food";
const TAMING_ITEMS: &str = "minecraft:nautilus_taming_items";
const BUCKET_FOOD: &str = "minecraft:nautilus_bucket_food";

/// What `AbstractNautilus` keeps beyond a tamed animal: its dash and the moment its
/// cooldown runs out, whether its eyes were under water at its last `baseTick`, the
/// attack cooldown its `finalizeSpawn` drew for its brain, and a zombie nautilus's
/// variant (the default for a nautilus, which has none).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct NautilusState {
    pub(crate) tame: Tame,
    /// `DASH`.
    pub(crate) dashing: bool,
    /// The moment `dashCooldown` comes to nothing: the cooldown at the tick of `lived`
    /// `L` is `dash_ready_at - L` before that tick's count, never below nothing.
    pub(crate) dash_ready_at: u32,
    /// `isUnderWater()`: `wasEyeInWater` as its last `baseTick` found it, which its
    /// sounds read.
    pub(crate) under_water: bool,
    /// `ATTACK_TARGET_COOLDOWN` as `NautilusAi.initMemories` drew it at its spawn, held
    /// here for the brain to take as its memory when its provider is wired; `None` for a
    /// nautilus made otherwise (bred), whose brain the game gives none.
    pub(crate) attack_target_cooldown: Option<i32>,
    /// A zombie nautilus's variant, by its id in [`VARIANTS`].
    pub(crate) variant: u8,
}

impl Default for NautilusState {
    /// `defineSynchedData`'s: not dashing, wild, and the temperate variant
    /// (`getDefaultOrAny`).
    fn default() -> Self {
        Self {
            tame: Tame::default(),
            dashing: false,
            dash_ready_at: 0,
            under_water: false,
            attack_target_cooldown: None,
            variant: variants::id(VARIANTS, "minecraft:temperate"),
        }
    }
}

impl Own for NautilusState {}

/// The state of a nautilus or a zombie nautilus.
pub(crate) fn state_of(mob: &Mob) -> (animal::Animal, NautilusState) {
    animal::read::<NautilusState>(&mob.state)
}

/// Changes the nautilus's own state.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut NautilusState)) {
    let (shared, mut own) = state_of(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        mob.state = animal::write(&shared, &own);
    }
}

/// `dashCooldown` at the tick of `lived`, before that tick's count.
pub(crate) fn dash_cooldown(state: &NautilusState, lived: u32) -> i32 {
    (i64::from(state.dash_ready_at) - i64::from(lived)).clamp(0, i64::from(i32::MAX)) as i32
}

/// The sounds of a type of the family, each its pair under water and on land where it
/// has both.
pub(crate) struct Voices {
    pub(crate) ambient: [i32; 2],
    pub(crate) hurt: [i32; 2],
    pub(crate) death: [i32; 2],
    pub(crate) dash: [i32; 2],
    pub(crate) dash_ready: [i32; 2],
    pub(crate) eat: i32,
}

/// The nautilus's.
const ADULT: Voices = Voices {
    ambient: [
        sounds::ENTITY_NAUTILUS_AMBIENT,
        sounds::ENTITY_NAUTILUS_AMBIENT_LAND,
    ],
    hurt: [
        sounds::ENTITY_NAUTILUS_HURT,
        sounds::ENTITY_NAUTILUS_HURT_LAND,
    ],
    death: [
        sounds::ENTITY_NAUTILUS_DEATH,
        sounds::ENTITY_NAUTILUS_DEATH_LAND,
    ],
    dash: [
        sounds::ENTITY_NAUTILUS_DASH,
        sounds::ENTITY_NAUTILUS_DASH_LAND,
    ],
    dash_ready: [
        sounds::ENTITY_NAUTILUS_DASH_READY,
        sounds::ENTITY_NAUTILUS_DASH_READY_LAND,
    ],
    eat: sounds::ENTITY_NAUTILUS_EAT,
};

/// A baby nautilus's: its own voice, an adult's dash.
const BABY: Voices = Voices {
    ambient: [
        sounds::ENTITY_BABY_NAUTILUS_AMBIENT,
        sounds::ENTITY_BABY_NAUTILUS_AMBIENT_LAND,
    ],
    hurt: [
        sounds::ENTITY_BABY_NAUTILUS_HURT,
        sounds::ENTITY_BABY_NAUTILUS_HURT_LAND,
    ],
    death: [
        sounds::ENTITY_BABY_NAUTILUS_DEATH,
        sounds::ENTITY_BABY_NAUTILUS_DEATH_LAND,
    ],
    dash: ADULT.dash,
    dash_ready: ADULT.dash_ready,
    eat: sounds::ENTITY_BABY_NAUTILUS_EAT,
};

/// The sound of `pair` in the medium the nautilus's eyes were in.
pub(crate) fn medium(pair: [i32; 2], state: &NautilusState) -> i32 {
    if state.under_water { pair[0] } else { pair[1] }
}

/// What the two types of the family do differently.
pub(crate) trait Family: Sync {
    /// Its sounds, as its age has them.
    fn voices(&self, mob: &Mob) -> &'static Voices;
    /// `ChargeAttack`'s speed (the brain's): 0.6, a zombie's 0.5.
    #[allow(dead_code, reason = "what the brain (part A) reads, which it wires")]
    fn charge_speed(&self) -> f32;
}

/// The nautilus.
pub(crate) struct Nautilus;

impl Family for Nautilus {
    fn voices(&self, mob: &Mob) -> &'static Voices {
        if animal::is_baby(mob) { &BABY } else { &ADULT }
    }

    fn charge_speed(&self) -> f32 {
        0.6
    }
}

impl Pet for Nautilus {
    fn tame(&self, mob: &Mob) -> Tame {
        state_of(mob).1.tame
    }

    fn set_tame(&self, mob: &mut Mob, tame: Tame) {
        update(mob, |own| own.tame = tame);
    }
}

/// `Mob.isSaddled()`.
pub(crate) fn saddled(mob: &Mob) -> bool {
    mob.body.equipment.slots[usize::from(slots::SADDLE)].is_some_and(|stack| stack.count > 0)
}

/// `AbstractNautilus.isFood(stack)`: a tamed one's or a baby's food is `#nautilus_food`,
/// a wild adult's what tames it.
pub(crate) fn food(mob: &Mob) -> Items {
    let (shared, own) = state_of(mob);
    if own.tame.is_tame() || shared.is_baby() {
        Items::Tag(FOOD)
    } else {
        Items::Tag(TAMING_ITEMS)
    }
}

/// `UniformInt(2400, 3600).sample(random)`: the cooldown before a wild nautilus picks a
/// fight again (`NautilusAi.initMemories`, and `findNearestValidAttackTarget`).
pub(crate) fn attack_target_cooldown(level: &mut Random) -> i32 {
    level.next_int_between(2400, 3600)
}

/// The seam of the brain (`Nautilus.customServerAiStep`'s first two steps: `brain.tick`
/// and `NautilusAi.updateActivity`, `setActiveActivityToFirstValid([FIGHT, IDLE])`):
/// nothing yet. The provider (`NautilusAi`, its memories `[ANGRY_AT,
/// ATTACK_TARGET_COOLDOWN]` and sensors `[NEAREST_LIVING_ENTITIES, NEAREST_ADULT,
/// NEAREST_PLAYERS, HURT_BY, NAUTILUS_TEMPTATIONS]`) is wired here with part A.
pub(crate) fn brain_seam(tick: &mut MobTick<'_>) {
    let _ = tick;
}

/// Whom a hurt that took angers it (`hurtServer`, `NautilusAi.setAngerTarget`): the
/// living thing that caused it, which the brain remembers as `ANGRY_AT` for 400 ticks
/// once its memories exist (the seam: no memory is kept yet).
#[allow(dead_code, reason = "what the brain (part A) reads, which it wires")]
pub(crate) fn anger_target(source: &DamageSource) -> Option<EntityId> {
    source
        .cause
        .as_ref()
        .filter(|cause| cause.living)
        .map(|cause| cause.key)
}

/// `getNautilusRestrictionRadius()`: 32 for a grown one without a saddle, else 16.
pub(crate) fn restriction_radius(mob: &Mob) -> i32 {
    if !animal::is_baby(mob) && !saddled(mob) {
        32
    } else {
        16
    }
}

/// `Vec3i.closerThan(other, distance)`: the corners' squared distance below the
/// distance's square.
fn closer_than(a: BlockPos, b: BlockPos, distance: f64) -> bool {
    let [dx, dy, dz] = [
        f64::from(a.x - b.x),
        f64::from(a.y - b.y),
        f64::from(a.z - b.z),
    ];
    dx * dx + dy * dy + dz * dz < distance * distance
}

/// `checkRestriction()`: a tamed one, neither leashed nor ridden, keeps its home where
/// it is near and of its radius, and is given one at its block otherwise.
pub(crate) fn check_restriction(tick: &mut MobTick<'_>) {
    if tick.leashed || tick.carries || !state_of(&tick.mob).1.tame.is_tame() {
        return;
    }
    let radius = restriction_radius(&tick.mob);
    let here = tick.living.body.block_position();
    let home = tick.mob.body.home.clone();
    if let Some(centre) = home.centre
        && closer_than(centre, here, f64::from(radius + 8))
        && home.radius == radius
    {
        return;
    }
    tick.mob.body.home = Home {
        centre: Some(here),
        radius,
    };
}

/// `AbstractNautilus.customServerAiStep` after the brain: its home, then the animal's
/// end of love.
pub(crate) fn custom_server_ai_step(tick: &mut MobTick<'_>) {
    brain_seam(tick);
    check_restriction(tick);
    animal::custom_server_ai_step(tick);
}

/// Its `baseTick` after `Mob`'s: whether its eyes are under water now (every type of
/// the family), and for a nautilus (`breathes_air`), its own air: out of water one less
/// a tick from what it had as the tick began, and at -20 a drying-out hit of 2 with its
/// air back to nothing; in water all 300, unless it has no AI.
pub(crate) fn after_base_tick(tick: &mut MobTick<'_>, air: i32, breathes_air: bool) {
    let under = water::eye_in_water(tick);
    update(&mut tick.mob, |own| own.under_water = under);
    if !breathes_air || tick.mob.body.flags.no_ai {
        return;
    }
    if water::alive(&tick.mob) && !tick.living.body.in_water {
        tick.living.air = air - 1;
        if tick.living.air <= -20 {
            tick.living.air = 0;
            tick.living.hurt(Damage::DryOut, 2.0);
        }
    } else {
        tick.living.air = TOTAL_AIR;
    }
}

/// `AbstractNautilus.tick()` after the living tick: a dash shows no more once its
/// cooldown is below 35, the cooldown counts down (with its type's ready sound at
/// nothing), and in water it draws for bubbles the server never sends.
pub(crate) fn after_tick(tick: &mut MobTick<'_>, family: &dyn Family) {
    let lived = tick.lived;
    let (_, own) = state_of(&tick.mob);
    let cooldown = dash_cooldown(&own, lived);
    if own.dashing && cooldown < DASHING_UNTIL {
        update(&mut tick.mob, |own| own.dashing = false);
    }
    if cooldown == 1 {
        let sound = medium(family.voices(&tick.mob).dash_ready, &own);
        make_sound(tick, sound);
    }
    if tick.living.body.in_water {
        spawn_bubbles(tick);
    }
}

/// `Mob.makeSound(sound)` in its tick: at its volume and voice, two draws.
fn make_sound(tick: &mut MobTick<'_>, sound: i32) {
    let pitch = voice_pitch(tick.draws, animal::is_baby(&tick.mob));
    tick.out.events.push(Event::Sound {
        sound,
        volume: 1.0,
        pitch,
    });
}

/// `spawnBubbles()`: one draw, and four more when it is below the chance its speed
/// gives; the particle is the client's.
fn spawn_bubbles(tick: &mut MobTick<'_>) {
    let [x, y, z] = tick.living.body.motion;
    let speed = (x * x + y * y + z * z).sqrt();
    let chance = (speed * 2.0).clamp(0.150_000_005_960_464_48, 1.0);
    if f64::from(tick.draws.next_float()) < chance {
        tick.draws.next_double();
        for _ in 0..3 {
            tick.draws.next_float();
        }
    }
}

/// `Mob.makeSound` of a use: at its volume and voice, two draws of its own random.
fn sound_of_use(used: &mut Used<'_>, sound: i32) -> Did {
    let pitch = voice_pitch(used.draws, animal::is_baby(used.mob));
    Did::Sound {
        sound,
        source: SOUND_SOURCE_NEUTRAL,
        at: used.position,
        volume: 1.0,
        pitch,
        but_the_player: false,
    }
}

/// `AbstractNautilus.usePlayerItem`: a bucket of fish eaten leaves a water bucket
/// (`ItemUtils.createFilledResult`: a creative player keeps the bucket and is given a
/// water bucket where they have none); anything else is kept by a creative player.
fn use_player_item(used: &mut Used<'_>) {
    let bucket = used
        .held
        .is_some_and(|stack| stack.count > 0 && item_in_tag(stack.item, BUCKET_FOOD));
    if bucket {
        used.did.push(Did::Give {
            stack: ItemStack {
                item: items::WATER_BUCKET,
                count: 1,
            },
            no_duplicates: true,
        });
    }
}

/// `tryToTame(player)`: one time in three, of its own draw, tamed by the player with its
/// path dropped and hearts shown; else smoke; then its eating sound.
fn try_to_tame(used: &mut Used<'_>, family: &dyn Family) {
    if used.draws.next_int(3) == 0 {
        let player = used.player_uuid;
        update(used.mob, |own| tamable::tame(&mut own.tame, player));
        used.mob.body.path.followed = None;
        used.did.push(Did::Event(7));
    } else {
        used.did.push(Did::Event(6));
    }
    let eat = family.voices(used.mob).eat;
    let sound = sound_of_use(used, eat);
    used.did.push(sound);
}

/// `canUseSlot(SADDLE or BODY)`: alive, grown and tamed.
pub(crate) fn can_use_slot(mob: &Mob) -> bool {
    water::alive(mob) && !animal::is_baby(mob) && state_of(mob).1.tame.is_tame()
}

/// `ItemStack.interactLivingEntity` of a saddle or nautilus armour
/// (`Equippable.equipOnTarget`; that nautilus armour is put on by its use is
/// **Inferred**: no other part of the nautilus's use puts it on): one of it in its slot
/// of a nautilus that may wear it there and wears nothing there, dropped whatever kills
/// it, with its sound (the nautilus's own saddling under water and above it, the armour's
/// own), a seed of its own random, and `GameEvent.EQUIP`.
fn equip(used: &mut Used<'_>, stack: ItemStack) -> Interaction {
    let Some(equippable) = clustine_data::ItemComponents::of(stack.item).equippable else {
        return Interaction::Pass;
    };
    let slot = equippable.slot;
    if (slot != slots::SADDLE && slot != slots::BODY)
        || !super::equine::equippable_by(stack.item, slot, used.kind)
        || !can_use_slot(used.mob)
        || used.mob.body.equipment.slots[usize::from(slot)].is_some()
    {
        return Interaction::Pass;
    }
    used.mob.body.equipment.slots[usize::from(slot)] = Some(ItemStack {
        item: stack.item,
        count: 1,
    });
    super::equine::guarantee_drop(used.mob, slot);
    let sound = if slot == slots::SADDLE {
        if state_of(used.mob).1.under_water {
            sounds::ITEM_NAUTILUS_SADDLE_UNDERWATER_EQUIP
        } else {
            sounds::ITEM_NAUTILUS_SADDLE_EQUIP
        }
    } else {
        equippable.sound
    };
    let _seed = used.draws.next_u64();
    used.did.push(Did::Sound {
        sound,
        source: SOUND_SOURCE_NEUTRAL,
        at: used.position,
        volume: 1.0,
        pitch: 1.0,
        but_the_player: false,
    });
    used.did.push(Did::GameEvent("minecraft:equip"));
    Interaction::Success
}

/// `TamableAnimal.mobInteract`, which is `Animal`'s: the nautilus's food puts a tamed
/// adult free of its cooldown in love or grows a baby, with its eating sound, a bucket
/// of fish leaving a water bucket; anything else, the golden dandelion's.
fn animal_interact(used: &mut Used<'_>, family: &dyn Family) -> Interaction {
    let food = food(used.mob);
    let eat = family.voices(used.mob).eat;
    let before = used.did.len();
    let result = animal::mob_interact(used, food, Some(eat));
    if result != Interaction::Pass && food.test(used.held) {
        let mut done = std::mem::take(&mut used.did);
        let added = done.split_off(before);
        used.did = done;
        use_player_item(used);
        used.did.extend(added);
    }
    result
}

/// The seam of riding (`doPlayerRide`: `player.startRiding(this)`, and the home cleared
/// where that left it without a rider): nobody is taken on until the riding (part R)
/// wires it; the click is taken all the same, as the game's is.
pub(crate) fn ride(used: &mut Used<'_>) {
    let _ = used;
}

/// `AbstractNautilus.mobInteract` (the brief's 7.12.4).
pub(crate) fn mob_interact(used: &mut Used<'_>, family: &dyn Family) -> Interaction {
    // `interact`'s `setPersistenceRequired()`, before the use.
    used.mob.body.flags.persistent = true;
    if animal::is_baby(used.mob) {
        return animal_interact(used, family);
    }
    let tamed = state_of(used.mob).1.tame.is_tame();
    if tamed && used.sneaking {
        // `openCustomInventoryScreen`: the mount menu is the riding's (named).
        return Interaction::Success;
    }
    let held = used.held.filter(|stack| stack.count > 0);
    let is_food = food(used.mob).test(held);
    if let Some(stack) = held {
        if !tamed && is_food {
            use_player_item(used);
            try_to_tame(used, family);
            return Interaction::SuccessServer;
        }
        if is_food && tamable::hurt(used.mob, used.kind) {
            use_player_item(used);
            let eat = family.voices(used.mob).eat;
            let sound = sound_of_use(used, eat);
            tamable::feed(used, 2.0, 1.0, Some(sound));
            return Interaction::Success;
        }
        let result = equip(used, stack);
        if result != Interaction::Pass {
            return result;
        }
    }
    if tamed && !used.sneaking && !is_food {
        ride(used);
        return Interaction::Success;
    }
    animal_interact(used, family)
}

/// `getControllingPassenger()`'s test of the nautilus: a player on it steers it where it
/// is saddled (that the first passenger is a player is the riding's to judge).
#[allow(dead_code, reason = "what the riding (part R) calls, which it wires")]
pub(crate) fn steers(mob: &Mob) -> bool {
    saddled(mob)
}

/// `getRiddenInput(player, …)`: the rider's strafe, and where they move on, the way they
/// look: up and along by their pitch, at half going back.
#[allow(dead_code, reason = "what the riding (part R) calls, which it wires")]
pub(crate) fn ridden_input(xxa: f32, zza: f32, rider_pitch: f32) -> Vec3 {
    let (mut up, mut along) = (0.0_f32, 0.0_f32);
    if zza != 0.0 {
        let radians = rider_pitch * 0.017_453_292;
        let mut c = table_cos(radians);
        let mut s = -table_sin(radians);
        if zza < 0.0 {
            c *= -0.5;
            s *= -0.5;
        }
        up = s;
        along = c;
    }
    [f64::from(xxa), f64::from(up), f64::from(along)]
}

/// `getRiddenRotation(rider)`: half the rider's pitch, and their yaw.
#[allow(dead_code, reason = "what the riding (part R) calls, which it wires")]
pub(crate) fn ridden_rotation(rider_yaw: f32, rider_pitch: f32) -> (f32, f32) {
    (rider_pitch * 0.5, rider_yaw)
}

/// `tickRidden`'s turn: the yaw half way to the rider's (its head, body and old yaw set
/// to it too).
#[allow(dead_code, reason = "what the riding (part R) calls, which it wires")]
pub(crate) fn ridden_yaw(own_yaw: f32, rider_yaw: f32) -> f32 {
    own_yaw + wrap_degrees(rider_yaw - own_yaw) * 0.5
}

/// `getRiddenSpeed(player)`: 0.0325 of its speed in water, 0.02 on land, in floats.
#[allow(dead_code, reason = "what the riding (part R) calls, which it wires")]
pub(crate) fn ridden_speed(movement_speed: f64, in_water: bool) -> f32 {
    let factor = if in_water { 0.0325_f32 } else { 0.02_f32 };
    factor * movement_speed as f32
}

/// `onPlayerJump(power)`: where it is saddled and its dash is ready at `lived`, the
/// pending jump's scale (`PlayerRideableJumping`: whole from 90, else `0.4 + 0.4 ×
/// power / 90`).
#[allow(dead_code, reason = "what the riding (part R) calls, which it wires")]
pub(crate) fn on_player_jump(mob: &Mob, lived: u32, power: i32) -> Option<f32> {
    let (_, own) = state_of(mob);
    if !saddled(mob) || dash_cooldown(&own, lived) > 0 {
        return None;
    }
    Some(if power >= 90 {
        1.0
    } else {
        0.4 + 0.4 * power as f32 / 90.0
    })
}

/// `executeRidersJump(scale, player)` at the moment `moment`: the motion it adds along
/// the rider's look (`1.2` of the scale in water, `0.5` on land, times its speed and the
/// block's speed factor), its cooldown of 40, and its dash shown.
#[allow(dead_code, reason = "what the riding (part R) calls, which it wires")]
pub(crate) fn execute_riders_jump(
    mob: &mut Mob,
    moment: u32,
    scale: f32,
    look: Vec3,
    in_water: bool,
    movement_speed: f64,
    block_speed_factor: f32,
) -> Vec3 {
    let factor = if in_water { 1.2_f32 } else { 0.5_f32 };
    let length = f64::from(factor * scale * movement_speed as f32 * block_speed_factor);
    update(mob, |own| {
        own.dash_ready_at = moment.saturating_add(DASH_COOLDOWN as u32);
        own.dashing = true;
    });
    look.map(|axis| axis * length)
}

/// `handleStartJump(power)` (the rider's jump): its type's dash sound in the medium it
/// is in, and it shows as dashing, with the cooldown of 40 where none ran
/// (`onSyncedDataUpdated(DASH)`). The sound, for the riding to play with
/// `GameEvent.ENTITY_ACTION`.
#[allow(dead_code, reason = "what the riding (part R) calls, which it wires")]
pub(crate) fn handle_start_jump(mob: &mut Mob, moment: u32, family: &dyn Family) -> i32 {
    let (_, own) = state_of(mob);
    let sound = medium(family.voices(mob).dash, &own);
    update(mob, |own| {
        own.dashing = true;
        if dash_cooldown(own, moment) == 0 {
            own.dash_ready_at = moment.saturating_add(DASH_COOLDOWN as u32);
        }
    });
    sound
}

/// The size of a baby nautilus: the type's halved (`BABY_DIMENSIONS`).
fn baby_size() -> Dimensions {
    let info = EntityTypeInfo::of(entity_types::NAUTILUS);
    water::scaled(info.width, info.height, info.eye_height, 0.5)
}

/// What every type of the family sets of its AI in its constructor: the water's
/// navigation, the smooth swimming controls, no cost for water.
pub(crate) fn register(setup: &mut Setup) {
    setup.config.navigation = Settings::of(Kind::WaterBound { breach: false });
    setup.config.move_kind = MoveKind::SmoothSwimming {
        max_turn_x: 85,
        max_turn_y: 10,
        in_water: 0.011,
        outside_water: 0.0,
        gravity: true,
    };
    setup.config.look_kind = LookKind::SmoothSwimming {
        max_y_rot_from_center: 10,
    };
    setup.set_malus(PathType::Water, 0.0);
}

/// The synced values of the family: `AgeableMob`'s, `TamableAnimal`'s and the dash.
pub(crate) fn shown(mob: &Mob) -> Vec<(u8, DataShown, bool)> {
    let (_, own) = state_of(mob);
    let mut values = animal::shown(mob);
    values.extend(own.tame.shown());
    values.push((DASH_INDEX, DataShown::Boolean(own.dashing), !own.dashing));
    values
}

/// `getBreedOffspring`: a new nautilus, tamed with this parent's owner where it is tamed.
fn offspring(breeding: &Breeding<'_>) -> Mob {
    let (_, this) = state_of(breeding.this);
    let mut own = NautilusState::default();
    if this.tame.is_tame() {
        own.tame.owner = this.tame.owner;
        own.tame.set_tame_flag(true);
    }
    let mut child = Mob::fresh();
    child.state = animal::write(&animal::Animal::default(), &own);
    child
}

/// `NautilusAi.initMemories`: the attack cooldown drawn of the level's random, kept for
/// the brain.
pub(crate) fn init_memories(spawn: &mut Spawning<'_>) {
    let cooldown = attack_target_cooldown(spawn.level);
    update(spawn.mob, |own| own.attack_target_cooldown = Some(cooldown));
}

impl MobType for Nautilus {
    fn register_goals(&self, setup: &mut Setup) {
        register(setup);
    }

    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        custom_server_ai_step(tick);
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
    }

    fn after_base_tick(&self, tick: &mut MobTick<'_>, air: i32) {
        after_base_tick(tick, air, true);
    }

    fn after_tick(&self, tick: &mut MobTick<'_>) {
        after_tick(tick, self);
    }

    /// `travelInWater`: at its speed, its motion times 0.9, no sinking.
    fn travel(&self, tick: &MobTick<'_>) -> Travel {
        Travel::Swims {
            speed: tick.mob.body.inputs.speed,
            sinks: Sinks::Never,
        }
    }

    fn pushed_by_fluid(&self) -> bool {
        false
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        mob_interact(used, self)
    }

    /// `AbstractNautilus.finalizeSpawn`: the brain's attack cooldown, then
    /// `TamableAnimal`'s (no baby of a first spawn) before `Mob`'s.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        init_memories(spawn);
    }

    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        Some(offspring(breeding))
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        animal::dimensions(mob, baby_size())
    }

    fn plays_step_sounds(&self) -> bool {
        false
    }

    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        Some(medium(self.voices(mob).ambient, &state_of(mob).1))
    }

    fn ambient_interval(&self) -> u32 {
        120
    }

    fn hurt_sound(&self, mob: &Mob, _: &DamageSource) -> Option<i32> {
        Some(medium(self.voices(mob).hurt, &state_of(mob).1))
    }

    fn death_sound(&self, mob: &Mob) -> Option<i32> {
        Some(medium(self.voices(mob).death, &state_of(mob).1))
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn requires_custom_persistence(&self, mob: &Mob) -> bool {
        state_of(mob).1.tame.is_tame()
    }

    fn pet(&self) -> Option<&'static dyn Pet> {
        Some(&Nautilus)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        shown(mob)
    }
}
