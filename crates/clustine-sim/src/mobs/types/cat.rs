//! Cats (`Cat`, `CatTemptGoal`, `CatAvoidEntityGoal`, `CatRelaxOnOwnerGoal`,
//! `CatLieOnBlockGoal`, `CatSitOnBlockGoal`, `CatVariants`, `CatSoundVariants`;
//! `docs/groundwork/n7-brief.md`, sections 5.0, 5.5 and 5.6): a stray cat runs from
//! players and creeps after one holding fish, which tames it one time in three; a tamed
//! cat sits and stands at its owner's click, follows them and teleports to them, lies on
//! beds and sits on furnaces, chests and beds, lies on its sleeping owner's bed, wears a
//! collar dyed by its owner, heals on fish, purrs, and breeds with another tamed cat.
//! Wild ones hunt rabbits and baby turtles. Its variant is one of ten, or black at the
//! full moon; its voice one of two.
//!
//! The avoiding of players is the game's goal removed and added again as tameness
//! changes (`reassessTameGoals`), so last among its goals while it is a stray: here it is
//! always last and does nothing while tamed, and taming stops it at once, as its removal
//! does.
//!
//! Named differences: the morning gift is left out (it needs the day's timeline of its
//! chance and the gift's loot table, which is not compiled; its draw of the chance is
//! made); a chest's openers are not read, so a cat sits on an open chest too; no
//! structure is known at a block, so an egg in a witch hut makes no black cat; its
//! sprint is not shown, only its crouch; a cat lying on a sleeping player is not told to
//! clients (they work it out themselves).

use clustine_blocks::data::{facing, in_tag, is_block, relative};
use clustine_data::{blocks, entity_types};
use clustine_world::{BlockPos, EntityId};
use serde::{Deserialize, Serialize};

use super::animal::{self, Own, variants};
use super::ocelot::{
    AvoidPlayersGoal, BABY, OcelotAttackGoal, crouches, feline_stroll, goal_state, pose_shown,
    remove_goal,
};
use super::sheep::{dye_colour, mixed_colour};
use super::tamable::{
    self, FollowOwnerGoal, NonTameRandomTargetGoal, Pet, SitWhenOrderedToGoal, TamablePanicGoal,
    Tame,
};
use super::{Breeding, Did, Dimensions, Interaction, Mate, MobType, Spawning, Used, cow, template};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    FloatGoal, Items, LeapAtTargetGoal, LookAtPlayerGoal, MoveToBlockGoal, MovingToBlock,
    TemptGoal, Tempted,
};
use crate::mobs::ai::maths;
use crate::mobs::ai::selector::Goal;
use crate::mobs::ai::sensing::Class;
use crate::mobs::ai::{Ai, Setup, navigation};
use crate::mobs::body::Event;
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::MobTick;
use crate::physics::aabb::Aabb;
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::region::Random;
use crate::shown::{DataShown, Holder};

/// The registries of a cat's variants.
pub(crate) const VARIANTS: &str = "minecraft:cat_variant";
const SOUND_VARIANTS: &str = "minecraft:cat_sound_variant";

/// `#cat_food`.
const FOOD: Items = Items::Tag("minecraft:cat_food");

/// The synced data's indices of a cat's own values.
const VARIANT_INDEX: u8 = 20;
const LYING_INDEX: u8 = 21;
const RELAXED_INDEX: u8 = 22;
const COLLAR_INDEX: u8 = 23;
const SOUND_VARIANT_INDEX: u8 = 24;

/// A collar's colour before it is dyed: red.
const RED: u8 = 14;

/// The places among its goals of its tempt goal and its avoiding of players.
const TEMPT_SLOT: u16 = 4;
const AVOID_SLOT: u16 = 13;

/// What a cat keeps beyond an animal: its tameness, its variant and voice by their ids,
/// its lying down and its relaxing, its collar's colour, and its crouch (its pose, not
/// saved by the game).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct CatState {
    pub(crate) tame: Tame,
    pub(crate) variant: u8,
    pub(crate) sound: u8,
    pub(crate) lying: bool,
    pub(crate) relaxed: bool,
    pub(crate) collar: u8,
    pub(crate) crouching: bool,
}

impl Default for CatState {
    /// `defineSynchedData`'s: black, the classic voice, a red collar.
    fn default() -> Self {
        Self {
            tame: Tame::default(),
            variant: variants::id(VARIANTS, "minecraft:black"),
            sound: variants::id(SOUND_VARIANTS, "minecraft:classic"),
            lying: false,
            relaxed: false,
            collar: RED,
            crouching: false,
        }
    }
}

impl Own for CatState {}

/// The cat.
pub(crate) struct Cat;

pub(crate) fn state_of(mob: &Mob) -> (animal::Animal, CatState) {
    animal::read::<CatState>(&mob.state)
}

fn put(mob: &mut Mob, shared: &animal::Animal, own: &CatState) {
    mob.state = animal::write(shared, own);
}

/// Changes the cat's own state of `mob`.
fn update(mob: &mut Mob, change: impl FnOnce(&mut CatState)) {
    let (shared, mut own) = state_of(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        put(mob, &shared, &own);
    }
}

impl Pet for Cat {
    fn tame(&self, mob: &Mob) -> Tame {
        state_of(mob).1.tame
    }

    fn set_tame(&self, mob: &mut Mob, tame: Tame) {
        update(mob, |own| own.tame = tame);
    }
}

/// [`select_variant_at`] at the day time `day`, outside any structure, as an egg's
/// spawn asks it.
#[cfg(test)]
pub(crate) fn select_variant(day: u64, level: &mut dyn clustine_world::generation::Draws) -> u8 {
    select_variant_at(crate::mobs::making::moon_brightness(day), None, level)
}

/// `VariantUtils.selectVariantToSpawn` of `cat_variant` (the brief's 5.5, 8.7) under a
/// moon of the brightness `moon` in the structure `structure`: all black alone of
/// priority 1 in a structure of `#cats_spawn_as_black` (the swamp hut, the tag's one
/// entry); else of the ten of priority 0, and all black besides under a moon of 0.9 or
/// more (the full moon), one drawn.
pub(crate) fn select_variant_at(
    moon: f32,
    structure: Option<&str>,
    level: &mut dyn clustine_world::generation::Draws,
) -> u8 {
    let entries = clustine_data::synced_registry(VARIANTS).map_or(&[][..], |known| known.entries);
    let black_hut = structure == Some("minecraft:swamp_hut");
    let kept: Vec<&str> = entries
        .iter()
        .copied()
        .filter(|name| {
            if black_hut {
                *name == "minecraft:all_black"
            } else {
                *name != "minecraft:all_black" || moon >= 0.9
            }
        })
        .collect();
    let chosen = if kept.is_empty() {
        "minecraft:black"
    } else {
        kept[level.next_int(kept.len() as u32) as usize]
    };
    variants::id(VARIANTS, chosen)
}

/// The sounds of a cat's voice.
#[derive(Debug, Clone, Copy)]
enum Voice {
    Ambient,
    BegForFood,
    Death,
    Eat,
    Hurt,
    Purr,
    Purreow,
    StrayAmbient,
}

/// The sound of `voice` in the cat's sound variant, a kitten's own where it is one.
fn voice(mob: &Mob, voice: Voice) -> Option<i32> {
    let (shared, own) = state_of(mob);
    let what = match voice {
        Voice::Ambient => "ambient",
        Voice::BegForFood => "beg_for_food",
        Voice::Death => "death",
        Voice::Eat => "eat",
        Voice::Hurt => "hurt",
        Voice::Purr => "purr",
        Voice::Purreow => "purreow",
        Voice::StrayAmbient => "stray_ambient",
    };
    let name = if shared.is_baby() {
        format!("minecraft:entity.baby_cat.{what}")
    } else {
        let variant = clustine_data::synced_registry(SOUND_VARIANTS)
            .and_then(|known| known.entries.get(usize::from(own.sound)))
            .and_then(|name| name.strip_prefix("minecraft:"))
            .unwrap_or("classic");
        if variant == "classic" {
            format!("minecraft:entity.cat.{what}")
        } else {
            format!("minecraft:entity.cat_{variant}.{what}")
        }
    };
    tamable::sound(&name)
}

/// `playEatingSound()`: its eating sound at the cat, of no draw.
fn eating_sound(used: &Used<'_>) -> Option<Did> {
    voice(used.mob, Voice::Eat).map(|sound| Did::Sound {
        sound,
        source: SOUND_SOURCE_NEUTRAL,
        at: used.position,
        volume: 1.0,
        pitch: 1.0,
        but_the_player: false,
    })
}

/// `tryToTame(player)`: one time in three, tamed by the player (its avoiding of players
/// removed) and ordered to sit, with hearts; else smoke.
fn try_to_tame(used: &mut Used<'_>) {
    if used.draws.next_int(3) != 0 {
        used.did.push(Did::Event(6));
        return;
    }
    let player = used.player_uuid;
    update(used.mob, |own| {
        tamable::tame(&mut own.tame, player);
        own.tame.ordered_to_sit = true;
    });
    remove_goal(used.mob, AVOID_SLOT);
    used.did.push(Did::Event(7));
}

/// `Cat.mobInteract` (the brief's 5.6).
fn mob_interact(used: &mut Used<'_>) -> Interaction {
    let (_, own) = state_of(used.mob);
    let held = used.held.as_ref().filter(|stack| stack.count > 0);
    if own.tame.is_tame() {
        if own.tame.is_owned_by(used.player_uuid) {
            let collar_dye = held.as_ref().is_some_and(|stack| {
                clustine_blocks::data::item_in_tag(stack.item, "minecraft:cat_collar_dyes")
            });
            if collar_dye {
                let dye = held.and_then(|stack| dye_colour(stack.item));
                if let Some(dye) = dye.filter(|dye| *dye != own.collar) {
                    update(used.mob, |own| own.collar = dye);
                    used.mob.body.flags.persistent = true;
                    return Interaction::Success;
                }
            } else if FOOD.test(held) && tamable::hurt(used.mob, used.kind) {
                let eat = eating_sound(used);
                tamable::feed(used, 1.0, 1.0, eat);
                return Interaction::Success;
            }
            let result = animal_use(used);
            if result == Interaction::Pass {
                update(used.mob, |own| {
                    own.tame.ordered_to_sit = !own.tame.ordered_to_sit
                });
                return Interaction::Success;
            }
            return result;
        }
    } else if FOOD.test(held) {
        try_to_tame(used);
        used.mob.body.flags.persistent = true;
        used.did.extend(eating_sound(used));
        return Interaction::Success;
    }
    let result = animal_use(used);
    if result != Interaction::Pass {
        used.mob.body.flags.persistent = true;
    }
    result
}

/// `Animal.mobInteract` of a cat, whose eating sound is its own and draws nothing.
fn animal_use(used: &mut Used<'_>) -> Interaction {
    let fed = FOOD.test(used.held.as_ref());
    let result = animal::mob_interact(used, FOOD, None);
    if fed && result != Interaction::Pass {
        used.did.extend(eating_sound(used));
    }
    result
}

/// `CatTemptGoal`'s state: the tempt's, and the player it lets come near without
/// running.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct CatTempted {
    pub(crate) tempted: Tempted,
    pub(crate) selected: Option<EntityId>,
}

/// `CatTemptGoal(cat, 0.6, food, true)`: a stray's tempt, which now and then picks the
/// player it follows to stop being scared of, and now and then forgets them.
#[derive(Clone, Copy)]
struct CatTemptGoal;

const TEMPT: TemptGoal = TemptGoal {
    speed: 0.6,
    items: FOOD,
    can_scare: true,
    stop_distance: 2.5,
};

fn is_tame(mob: &Mob) -> bool {
    state_of(mob).1.tame.is_tame()
}

impl Goal for CatTemptGoal {
    type State = CatTempted;

    fn flags(&self) -> u8 {
        TEMPT.flags()
    }

    fn can_use(&self, state: &mut CatTempted, ai: &mut Ai<'_, '_>) -> bool {
        TEMPT.can_use(&mut state.tempted, ai) && !is_tame(&ai.tick.mob)
    }

    fn can_continue(&self, state: &mut CatTempted, ai: &mut Ai<'_, '_>) -> bool {
        let unafraid = state.selected.is_some() && state.selected == state.tempted.player;
        let tempt = TemptGoal {
            can_scare: !unafraid,
            ..TEMPT
        };
        tempt.can_continue(&mut state.tempted, ai) && !is_tame(&ai.tick.mob)
    }

    fn start(&self, state: &mut CatTempted, ai: &mut Ai<'_, '_>) {
        TEMPT.start(&mut state.tempted, ai);
    }

    fn stop(&self, state: &mut CatTempted, ai: &mut Ai<'_, '_>) {
        TEMPT.stop(&mut state.tempted, ai);
    }

    fn tick(&self, state: &mut CatTempted, ai: &mut Ai<'_, '_>) {
        TEMPT.tick(&mut state.tempted, ai);
        let pick = self.adjusted_tick_delay(600) as u32;
        let forget = self.adjusted_tick_delay(500) as u32;
        if state.selected.is_none() && ai.tick.draws.next_int(pick) == 0 {
            state.selected = state.tempted.player;
        } else if ai.tick.draws.next_int(forget) == 0 {
            state.selected = None;
        }
    }
}

/// `CatRelaxOnOwnerGoal`'s state: its owner, the bed's spot beside their head, the
/// ticks it has been on the bed, and when its owner lay down.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Relaxing {
    pub(crate) owner: Option<u128>,
    pub(crate) goal: Option<BlockPos>,
    pub(crate) on_bed_ticks: i32,
    pub(crate) slept_since: Option<u64>,
}

/// `CatRelaxOnOwnerGoal(cat)`: a tamed cat lies on its owner's bed while they sleep.
#[derive(Clone, Copy)]
struct CatRelaxOnOwnerGoal;

impl CatRelaxOnOwnerGoal {
    /// `spaceIsOccupied()`: another cat lies or relaxes within two blocks of the spot.
    fn occupied(ai: &Ai<'_, '_>, goal: BlockPos) -> bool {
        let bounds = Aabb::of_block(goal).inflate([2.0, 2.0, 2.0]);
        let me = ai.tick.key;
        ai.tick.world.entities_in(&bounds).iter().any(|seen| {
            if seen.entity.key == me || i32::from(seen.entity.kind) != entity_types::CAT {
                return false;
            }
            let clustine_world::EntityData::Mob { state, .. } = &seen.entity.data else {
                return false;
            };
            let (_, other) = animal::read::<CatState>(state);
            other.lying || other.relaxed
        })
    }

    /// The owner as the goal holds them.
    fn owner(state: &Relaxing, ai: &Ai<'_, '_>) -> Option<crate::mobs::view::SeenPlayer> {
        state
            .owner
            .and_then(|uuid| ai.tick.world.player_anywhere(uuid))
    }
}

/// `Mob.lookAt(entity, maxYaw, maxPitch)`: its body turned towards the eyes at `eyes`,
/// by at most so much.
fn look_at(ai: &mut Ai<'_, '_>, eyes: [f64; 3], max_yaw: f32, max_pitch: f32) {
    let [x, _, z] = ai.position();
    let dx = eyes[0] - x;
    let dz = eyes[2] - z;
    let dy = eyes[1] - ai.eye_y();
    let horizontal = (dx * dx + dz * dz).sqrt();
    let yaw = maths::yaw_towards(dx, dz);
    let pitch = maths::pitch_towards(dy, horizontal);
    // `Mob.rotlerp`: turned by the wrapped difference, at most `max`, unwrapped.
    let turn = |from: f32, to: f32, max: f32| {
        let by = maths::wrap_degrees(to - from).clamp(-max, max);
        from + by
    };
    ai.tick.pitch = turn(ai.tick.pitch, pitch, max_pitch);
    ai.tick.yaw = turn(ai.tick.yaw, yaw, max_yaw);
}

impl Goal for CatRelaxOnOwnerGoal {
    type State = Relaxing;

    fn flags(&self) -> u8 {
        0
    }

    fn can_use(&self, state: &mut Relaxing, ai: &mut Ai<'_, '_>) -> bool {
        let tame = Cat.tame(&ai.tick.mob);
        if !tame.is_tame() || tame.ordered_to_sit {
            return false;
        }
        let Some(owner) = tamable::owner(ai.tick.world, &tame) else {
            return false;
        };
        state.owner = tame.owner;
        let Some(since) = owner.sleeping else {
            return false;
        };
        state.slept_since = Some(since);
        if tamable::distance_sqr(ai.position(), owner.position) > 100.0 {
            return false;
        }
        let at = crate::physics::containing(owner.position);
        let bed = ai.tick.world.state(at);
        if !in_tag(bed, "minecraft:cats_can_lie_on") {
            return false;
        }
        // Beside the bed's head, the way it faces back (`FACING.getOpposite()`).
        let goal = facing(bed).map_or(at, |face| relative(at, opposite(face)));
        state.goal = Some(goal);
        !Self::occupied(ai, goal)
    }

    fn can_continue(&self, state: &mut Relaxing, ai: &mut Ai<'_, '_>) -> bool {
        let tame = Cat.tame(&ai.tick.mob);
        if !tame.is_tame() || tame.ordered_to_sit {
            return false;
        }
        let sleeping = Self::owner(state, ai).is_some_and(|owner| owner.sleeping.is_some());
        let Some(goal) = state.goal.filter(|_| sleeping) else {
            return false;
        };
        !Self::occupied(ai, goal)
    }

    fn start(&self, state: &mut Relaxing, ai: &mut Ai<'_, '_>) {
        if let Some(goal) = state.goal {
            tamable::update(&Cat, &mut ai.tick.mob, |tame| {
                tame.set_in_sitting_pose(false)
            });
            let at = [f64::from(goal.x), f64::from(goal.y), f64::from(goal.z)];
            navigation::move_to(ai, at, 1.100_000_023_841_858);
        }
    }

    /// `stop()`: it gets up; where its owner slept a hundred ticks, the level's draw of
    /// the gift's chance (the gift itself is left out, see the module's comment).
    fn stop(&self, state: &mut Relaxing, ai: &mut Ai<'_, '_>) {
        update(&mut ai.tick.mob, |own| own.lying = false);
        let now = ai.tick.world.game_time();
        let slept = state
            .slept_since
            .is_some_and(|since| now - since as i64 >= 100);
        if slept {
            let _chance = ai.tick.draws.next_float();
        }
        state.on_bed_ticks = 0;
        update(&mut ai.tick.mob, |own| own.relaxed = false);
        navigation::stop(ai);
    }

    fn tick(&self, state: &mut Relaxing, ai: &mut Ai<'_, '_>) {
        let (Some(owner), Some(goal)) = (Self::owner(state, ai), state.goal) else {
            return;
        };
        if let Some(since) = owner.sleeping {
            state.slept_since = Some(since);
        }
        tamable::update(&Cat, &mut ai.tick.mob, |tame| {
            tame.set_in_sitting_pose(false)
        });
        let at = [f64::from(goal.x), f64::from(goal.y), f64::from(goal.z)];
        navigation::move_to(ai, at, 1.100_000_023_841_858);
        if tamable::distance_sqr(ai.position(), owner.position) < 2.5 {
            state.on_bed_ticks += 1;
            if state.on_bed_ticks > self.adjusted_tick_delay(16) {
                update(&mut ai.tick.mob, |own| {
                    own.lying = true;
                    own.relaxed = false;
                });
            } else {
                let eyes = [
                    owner.position[0],
                    owner.position[1] + owner.eye_height,
                    owner.position[2],
                ];
                look_at(ai, eyes, 45.0, 45.0);
                update(&mut ai.tick.mob, |own| own.relaxed = true);
            }
        } else {
            update(&mut ai.tick.mob, |own| own.lying = false);
        }
    }
}

/// The face opposite `face`.
fn opposite(face: clustine_data::Face) -> clustine_data::Face {
    use clustine_data::Face;
    match face {
        Face::Down => Face::Up,
        Face::Up => Face::Down,
        Face::North => Face::South,
        Face::South => Face::North,
        Face::West => Face::East,
        Face::East => Face::West,
    }
}

/// `CatLieOnBlockGoal.isValidTarget`: a bed with air above.
fn lie_valid(ai: &Ai<'_, '_>, pos: BlockPos) -> bool {
    let world = ai.tick.world;
    world.state(BlockPos::new(pos.x, pos.y + 1, pos.z)).is_air()
        && in_tag(world.state(pos), "minecraft:cats_can_lie_on")
}

/// `CatSitOnBlockGoal.isValidTarget`: air above one of `#cats_can_sit_on`, a chest not
/// open (its openers are not read: see the module's comment), a lit furnace, a bed but
/// at its head.
fn sit_valid(ai: &Ai<'_, '_>, pos: BlockPos) -> bool {
    let world = ai.tick.world;
    if !world.state(BlockPos::new(pos.x, pos.y + 1, pos.z)).is_air() {
        return false;
    }
    let state = world.state(pos);
    if !in_tag(state, "minecraft:cats_can_sit_on") {
        return false;
    }
    if is_block(state, blocks::CHEST) {
        return true;
    }
    if is_block(state, blocks::FURNACE) {
        return state.property("lit") == Some("true");
    }
    if in_tag(state, "minecraft:beds") {
        return state.property("part") != Some("head");
    }
    true
}

const LIE_ON: MoveToBlockGoal = MoveToBlockGoal {
    speed: 1.1,
    search_range: 8,
    vertical_search_range: 6,
    vertical_search_start: -2,
    valid: lie_valid,
    next_start: Some(lie_next_start),
};

fn lie_next_start(_: &mut Ai<'_, '_>) -> i32 {
    40
}

const SIT_ON: MoveToBlockGoal = MoveToBlockGoal {
    speed: 0.8,
    search_range: 8,
    vertical_search_range: 1,
    vertical_search_start: 0,
    valid: sit_valid,
    next_start: None,
};

/// `CatLieOnBlockGoal(cat, 1.1, 8)`: a tamed cat goes to lie on a bed near.
#[derive(Clone, Copy)]
struct CatLieOnBlockGoal;

impl Goal for CatLieOnBlockGoal {
    type State = MovingToBlock;

    fn flags(&self) -> u8 {
        LIE_ON.flags()
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) -> bool {
        let (_, own) = state_of(&ai.tick.mob);
        own.tame.is_tame() && !own.tame.ordered_to_sit && !own.lying && LIE_ON.can_use(state, ai)
    }

    fn can_continue(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) -> bool {
        LIE_ON.can_continue(state, ai)
    }

    fn start(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) {
        LIE_ON.start(state, ai);
        tamable::update(&Cat, &mut ai.tick.mob, |tame| {
            tame.set_in_sitting_pose(false)
        });
    }

    fn stop(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) {
        LIE_ON.stop(state, ai);
        update(&mut ai.tick.mob, |own| own.lying = false);
    }

    fn tick(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) {
        LIE_ON.tick(state, ai);
        tamable::update(&Cat, &mut ai.tick.mob, |tame| {
            tame.set_in_sitting_pose(false)
        });
        let reached = state.reached;
        update(&mut ai.tick.mob, |own| {
            if !reached {
                own.lying = false;
            } else if !own.lying {
                own.lying = true;
            }
        });
    }
}

/// `CatSitOnBlockGoal(cat, 0.8)`: a tamed cat goes to sit on a lit furnace, a chest or a
/// bed near.
#[derive(Clone, Copy)]
struct CatSitOnBlockGoal;

impl Goal for CatSitOnBlockGoal {
    type State = MovingToBlock;

    fn flags(&self) -> u8 {
        SIT_ON.flags()
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) -> bool {
        let tame = Cat.tame(&ai.tick.mob);
        tame.is_tame() && !tame.ordered_to_sit && SIT_ON.can_use(state, ai)
    }

    fn can_continue(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) -> bool {
        SIT_ON.can_continue(state, ai)
    }

    fn start(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) {
        SIT_ON.start(state, ai);
        tamable::update(&Cat, &mut ai.tick.mob, |tame| {
            tame.set_in_sitting_pose(false)
        });
    }

    fn stop(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) {
        SIT_ON.stop(state, ai);
        tamable::update(&Cat, &mut ai.tick.mob, |tame| {
            tame.set_in_sitting_pose(false)
        });
    }

    fn tick(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) {
        SIT_ON.tick(state, ai);
        let reached = state.reached;
        tamable::update(&Cat, &mut ai.tick.mob, |tame| {
            tame.set_in_sitting_pose(reached)
        });
    }
}

/// Whether a cat runs from players: while it is a stray.
fn stray(mob: &Mob) -> bool {
    !is_tame(mob)
}

/// `Cat.tick` after the living body's: a stray tempted begs every five seconds, and a
/// cat lying or relaxing purrs every quarter second.
fn cat_tick(tick: &mut MobTick<'_>) {
    let tick_count = tick.tick_count.saturating_add(1);
    let (_, own) = state_of(&tick.mob);
    let tempted = goal_state::<CatTempted>(&tick.mob, TEMPT_SLOT)
        .tempted
        .running;
    if tempted
        && !own.tame.is_tame()
        && tick_count.is_multiple_of(100)
        && let Some(sound) = voice(&tick.mob, Voice::BegForFood)
    {
        tick.out.events.push(Event::Sound {
            sound,
            volume: 1.0,
            pitch: 1.0,
        });
    }
    if (own.lying || own.relaxed) && tick_count.is_multiple_of(5) {
        let volume = 0.6 + 0.4 * (tick.draws.next_float() - tick.draws.next_float());
        if let Some(sound) = voice(&tick.mob, Voice::Purr) {
            tick.out.events.push(Event::Sound {
                sound,
                volume,
                pitch: 1.0,
            });
        }
    }
}

impl MobType for Cat {
    fn register_goals(&self, setup: &mut Setup) {
        cow::animal_maluses(setup);
        setup.add_goal(1, FloatGoal);
        setup.add_goal(
            1,
            TamablePanicGoal {
                pet: &Cat,
                speed: 1.5,
                environmental: false,
            },
        );
        setup.add_goal(2, SitWhenOrderedToGoal { pet: &Cat });
        setup.add_goal(3, CatRelaxOnOwnerGoal);
        setup.add_goal(4, CatTemptGoal);
        setup.add_goal(5, CatLieOnBlockGoal);
        setup.add_goal(
            6,
            FollowOwnerGoal {
                pet: &Cat,
                speed: 1.0,
                start: 10.0,
                stop: 5.0,
            },
        );
        setup.add_goal(7, CatSitOnBlockGoal);
        setup.add_goal(8, LeapAtTargetGoal { yd: 0.3 });
        setup.add_goal(9, OcelotAttackGoal);
        setup.add_goal(10, animal::BreedGoal { speed: 0.8 });
        setup.add_goal(11, feline_stroll());
        setup.add_goal(12, LookAtPlayerGoal::new(Class::Player, 10.0));
        setup.add_goal(4, AvoidPlayersGoal { active: stray });
        setup.add_target(
            1,
            NonTameRandomTargetGoal {
                pet: &Cat,
                class: Class::Kinds(&[entity_types::RABBIT as u16]),
                selector: None,
            },
        );
        setup.add_target(
            1,
            NonTameRandomTargetGoal {
                pet: &Cat,
                class: Class::Kinds(&[entity_types::TURTLE as u16]),
                selector: Some(tamable::baby_turtle_on_land),
            },
        );
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        animal::walk_target_value(ai, pos, blocks::GRASS_BLOCK)
    }

    /// `Cat.customServerAiStep`, which replaces `Animal`'s: its crouch.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        let crouching = crouches(tick);
        update(&mut tick.mob, |own| own.crouching = crouching);
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
        if !tick.mob.body.vitals.dying() {
            cat_tick(tick);
        }
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        mob_interact(used)
    }

    /// `Cat.finalizeSpawn` calls its base's first: `AgeableMob`'s babies of a group,
    /// then `Mob`'s.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        animal::finalize_ageable_default(spawn);
    }

    /// `Cat.readAdditionalSaveData`: its `variant` by name (where a tag has none, the
    /// old `CatType` by the number the game's data fixer maps), which its
    /// `finalizeSpawn` draws again, and its `CollarColor` by its legacy id (red where it
    /// names none), which it keeps.
    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        // `CatTypeFix`'s order of the old numbers.
        const OLD: [&str; 11] = [
            "minecraft:tabby",
            "minecraft:black",
            "minecraft:red",
            "minecraft:siamese",
            "minecraft:british_shorthair",
            "minecraft:calico",
            "minecraft:persian",
            "minecraft:ragdoll",
            "minecraft:white",
            "minecraft:jellie",
            "minecraft:all_black",
        ];
        let known = |name: &str| {
            clustine_data::synced_registry(VARIANTS)
                .and_then(|registry| registry.id_of(name))
                .map(|_| variants::id(VARIANTS, name))
        };
        let variant = match template::text(tag, "variant") {
            Some(name) => known(name),
            None => template::int(tag, "CatType")
                .and_then(|old| usize::try_from(old).ok())
                .and_then(|old| OLD.get(old))
                .and_then(|name| known(name)),
        };
        let collar = template::int(tag, "CollarColor")
            .and_then(|id| u8::try_from(id).ok())
            .filter(|&id| id < 16)
            .unwrap_or(RED);
        update(mob, |own| {
            if let Some(variant) = variant {
                own.variant = variant;
            }
            own.collar = collar;
        });
    }

    /// `Cat.finalizeSpawn` after its base's draws: its variant, then its voice.
    fn after_finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let variant = select_variant_at(spawn.moon_brightness, spawn.structure, spawn.level);
        let sound = spawn.level.next_int(variants::count(SOUND_VARIANTS)) as u8;
        update(spawn.mob, |own| {
            own.variant = variant;
            own.sound = sound;
        });
    }

    /// `Cat.getBreedOffspring`: a parent's variant by this one's coin, and where this one
    /// is tamed, its owner, tamed, and the parents' collars mixed. Its voice is the
    /// classic one (the brief's finding 5.0.12).
    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        let (_, this) = state_of(breeding.this);
        let (_, partner) = state_of(breeding.partner);
        let mut own = CatState {
            variant: if breeding.draws.next_bool() {
                this.variant
            } else {
                partner.variant
            },
            ..CatState::default()
        };
        if this.tame.is_tame() {
            own.tame.owner = this.tame.owner;
            own.tame.set_tame_flag(true);
            own.collar = mixed_colour(this.collar, partner.collar, breeding.draws);
        }
        let mut child = Mob::fresh();
        child.state = animal::write(&animal::Animal::default(), &own);
        Some(child)
    }

    /// `Cat.canMate(partner)`: of one type, both tamed (their love is the breeding's).
    fn can_mate(&self, this: Mate<'_>, partner: Mate<'_>) -> bool {
        let (_, mine) = state_of(this.mob);
        let (_, other) = state_of(partner.mob);
        this.entity.kind == partner.entity.kind && mine.tame.is_tame() && other.tame.is_tame()
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        animal::dimensions(mob, BABY)
    }

    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        voice(mob, Voice::StrayAmbient)
    }

    /// `Cat.getAmbientSound()`: a tamed cat purrs in love, else purreows one time in
    /// four, else mews; a stray's is its own.
    fn ambient_sound_drawn(
        &self,
        mob: &Mob,
        draws: &mut Random,
        _: i64,
        lived: u32,
        _: Option<(bool, bool)>,
    ) -> Option<i32> {
        let (shared, own) = state_of(mob);
        if !own.tame.is_tame() {
            return voice(mob, Voice::StrayAmbient);
        }
        if shared.in_love(lived) {
            return voice(mob, Voice::Purr);
        }
        if draws.next_int(4) == 0 {
            return voice(mob, Voice::Purreow);
        }
        voice(mob, Voice::Ambient)
    }

    fn ambient_interval(&self) -> u32 {
        120
    }

    fn hurt_sound(&self, mob: &Mob, _: &DamageSource) -> Option<i32> {
        voice(mob, Voice::Hurt)
    }

    fn death_sound(&self, mob: &Mob) -> Option<i32> {
        voice(mob, Voice::Death)
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    /// `removeWhenFarAway`: a stray, after 2400 ticks.
    fn remove_when_far_away(&self, _: u16, mob: &Mob, tick_count: u32, _: f64) -> bool {
        !is_tame(mob) && tick_count > 2400
    }

    fn pet(&self) -> Option<&'static dyn Pet> {
        Some(&Cat)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let (_, own) = state_of(mob);
        let fresh = CatState::default();
        let mut values = animal::shown(mob);
        values.push(pose_shown(own.crouching));
        values.extend(own.tame.shown());
        values.extend([
            (
                VARIANT_INDEX,
                DataShown::Holder(Holder::CatVariant, i32::from(own.variant)),
                own.variant == fresh.variant,
            ),
            (LYING_INDEX, DataShown::Boolean(own.lying), !own.lying),
            (RELAXED_INDEX, DataShown::Boolean(own.relaxed), !own.relaxed),
            (
                COLLAR_INDEX,
                DataShown::Int(i32::from(own.collar)),
                own.collar == RED,
            ),
            (
                SOUND_VARIANT_INDEX,
                DataShown::Holder(Holder::CatSoundVariant, i32::from(own.sound)),
                own.sound == fresh.sound,
            ),
        ]);
        values
    }
}
