//! The silverfish (`Silverfish` and its two goals; `docs/groundwork/n9-plan.md`, section
//! 8.4; n9 Silverfish), with the infested blocks it hides in (`region/infested.rs`).
//!
//! It floats, climbs out of powder snow, fights what hurts it (alerting other
//! silverfish) and players it sees, and wanders; with no target and its path done, one ask
//! in five of its goal ticks (`mob_griefing` being on) it picks a side
//! (`Direction.getRandom`), and where the block there is a host (stone, cobblestone, the
//! stone bricks, deepslate) it merges into it: the block made infested, its spawn's smoke
//! shown, itself gone. Hurt by something, or by what always wakes them
//! (`#always_triggers_silverfish`), it wakes its friends ten of its goal ticks later: the
//! infested blocks within 10 across and 5 up and down, nearest first by the game's order,
//! are broken (each making its silverfish), one by one until a `nextBoolean` stops it.
//!
//! Its body always faces where it looks (`tick` sets the body to its yaw), and its yaw
//! follows its body's turns (`setYBodyRot`, **inferred** as the body control's setter). It
//! likes to walk where a host block is below (10). It makes no step sounds
//! (`MovementEmission.EVENTS`).
//!
//! Named: its friends' waking is asked at the hurt's head, before the hurt's own refusals
//! (an invulnerable silverfish's would not be); an infested block in another region's land
//! is broken to air there with no silverfish (`Region::mob_breaks_block`).

use clustine_data::{BlockState, blocks, sounds};
use clustine_world::{BlockPos, Entry};
use serde::{Deserialize, Serialize};

use super::{MobType, monster_ai};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    ClimbOnTopOfPowderSnowGoal, FloatGoal, MeleeAttackGoal, RandomStrollGoal, Strolling,
};
use crate::mobs::ai::maths::reduced_tick_delay;
use crate::mobs::ai::navigation;
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::targets::{self, NearestTarget, Target};
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::{DamageSource, Struck};
use crate::mobs::body::{Event, Removed};
use crate::mobs::part::mind_ids;
use crate::mobs::tick::{Local, MobTick};
use crate::region::Random;

/// Whether the type is whole, so that `built()` gives it and an egg makes it.
pub(crate) const BUILT: bool = true;

/// Each host block and its infested block (`InfestedBlock.BLOCK_BY_HOST_BLOCK`, as
/// `Blocks` registers them).
const HOSTS: [(BlockState, BlockState); 7] = [
    (blocks::STONE, blocks::INFESTED_STONE),
    (blocks::COBBLESTONE, blocks::INFESTED_COBBLESTONE),
    (blocks::STONE_BRICKS, blocks::INFESTED_STONE_BRICKS),
    (
        blocks::MOSSY_STONE_BRICKS,
        blocks::INFESTED_MOSSY_STONE_BRICKS,
    ),
    (
        blocks::CRACKED_STONE_BRICKS,
        blocks::INFESTED_CRACKED_STONE_BRICKS,
    ),
    (
        blocks::CHISELED_STONE_BRICKS,
        blocks::INFESTED_CHISELED_STONE_BRICKS,
    ),
    (blocks::DEEPSLATE, blocks::INFESTED_DEEPSLATE),
];

/// Whether two states are of one block.
fn same_block(a: BlockState, b: BlockState) -> bool {
    a.block_id().is_some() && a.block_id() == b.block_id()
}

/// `getNewStateWithProperties`: the default state of `block` with what of its
/// properties `from` has too (a deepslate's axis).
fn with_properties_of(block: BlockState, from: BlockState) -> BlockState {
    match from.property("axis") {
        Some(axis) => block.with_property("axis", axis).unwrap_or(block),
        None => block,
    }
}

/// `InfestedBlock.isCompatibleHostBlock(state)`: a block a silverfish can hide in.
pub(crate) fn is_host(state: BlockState) -> bool {
    HOSTS.iter().any(|(host, _)| same_block(*host, state))
}

/// Whether `state` is an infested block (`instanceof InfestedBlock`).
pub(crate) fn is_infested(state: BlockState) -> bool {
    HOSTS
        .iter()
        .any(|(_, infested)| same_block(*infested, state))
}

/// `infestedStateByHost(state)`: the infested block of the host `state`, its properties
/// kept; the state itself for no host.
pub(crate) fn infested_by_host(state: BlockState) -> BlockState {
    HOSTS
        .iter()
        .find(|(host, _)| same_block(*host, state))
        .map_or(state, |(_, infested)| with_properties_of(*infested, state))
}

/// `Silverfish.registerGoals`' targets: who hurt it, alerting silverfish; players it
/// sees.
pub(crate) const TARGETS: &[(i32, Target)] = &[
    (
        1,
        Target::HurtBy {
            ignore: &[],
            alert: true,
        },
    ),
    (2, Target::Nearest(NearestTarget::players(true))),
];

/// The place among its goals of its waking of friends, whose part its hurt writes.
const WAKE_SLOT: u16 = 2;

/// `Silverfish.registerGoals`.
fn register_goals(setup: &mut Setup) {
    setup.add_goal(1, FloatGoal);
    setup.add_goal(1, ClimbOnTopOfPowderSnowGoal);
    setup.add_goal(3, SilverfishWakeUpFriendsGoal);
    setup.add_goal(
        4,
        MeleeAttackGoal {
            speed: 1.0,
            following_even_if_not_seen: false,
        },
    );
    setup.add_goal(5, SilverfishMergeWithStoneGoal);
    targets::register(setup, TARGETS);
}

/// `SilverfishWakeUpFriendsGoal`'s state: `lookForFriends`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Waking {
    pub(crate) look_for_friends: i32,
}

/// `SilverfishWakeUpFriendsGoal`: once its hurt has asked (`notifyHurt`, ten goal ticks
/// on), the infested blocks about it broken.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SilverfishWakeUpFriendsGoal;

/// The offsets the waking walks: 0, 1, −1, 2, −2, … to `reach` each way.
fn outwards(reach: i32) -> impl Iterator<Item = i32> {
    let mut next = Some(0);
    std::iter::from_fn(move || {
        let at = next?;
        let after = i32::from(at <= 0) - at;
        next = (after <= reach && after >= -reach).then_some(after);
        Some(at)
    })
}

impl Goal for SilverfishWakeUpFriendsGoal {
    type State = Waking;

    fn flags(&self) -> u8 {
        0
    }

    fn can_use(&self, state: &mut Waking, _: &mut Ai<'_, '_>) -> bool {
        state.look_for_friends > 0
    }

    fn tick(&self, state: &mut Waking, ai: &mut Ai<'_, '_>) {
        state.look_for_friends -= 1;
        if state.look_for_friends > 0 {
            return;
        }
        let at = ai.block_position();
        for y in outwards(5) {
            for x in outwards(10) {
                for z in outwards(10) {
                    let pos = BlockPos::new(at.x + x, at.y + y, at.z + z);
                    if !is_infested(ai.tick.world.state(pos)) {
                        continue;
                    }
                    // `mob_griefing` is on: `destroyBlock(pos, true, silverfish)`.
                    ai.tick.out.local.push(Local::BreakBlock { pos });
                    if ai.tick.draws.next_bool() {
                        return;
                    }
                }
            }
        }
    }
}

/// `notifyHurt()` of the waking goal kept in `mob`'s mind: ten of its goal ticks
/// (`adjustedTickDelay(20)`) where it was not counting already.
fn notify_hurt(mob: &mut Mob) {
    let id = mind_ids::FIRST_GOAL + WAKE_SLOT;
    let slots = &mut mob.mind.slots;
    let at = slots.binary_search_by_key(&id, |entry| entry.id);
    let mut waking: Waking = at
        .ok()
        .map(|at| &slots[at])
        .filter(|entry| entry.version == SilverfishWakeUpFriendsGoal::VERSION)
        .and_then(|entry| postcard::from_bytes(&entry.bytes).ok())
        .unwrap_or_default();
    if waking.look_for_friends != 0 {
        return;
    }
    waking.look_for_friends = SilverfishWakeUpFriendsGoal.adjusted_tick_delay(20);
    let entry = Entry {
        id,
        version: SilverfishWakeUpFriendsGoal::VERSION,
        bytes: postcard::to_allocvec(&waking).expect("a goal's state is encoded into memory"),
    };
    match at {
        Ok(at) => slots[at] = entry,
        Err(at) => slots.insert(at, entry),
    }
}

/// `SilverfishMergeWithStoneGoal`'s state: the stroll's, the side chosen
/// (`Direction`'s ordinal) and whether it merges.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Merging {
    pub(crate) stroll: Strolling,
    pub(crate) selected_direction: u8,
    pub(crate) do_merge: bool,
}

/// `SilverfishMergeWithStoneGoal`: a `RandomStrollGoal(1.0, 10)` that, with no target
/// and its path done, one ask in five merges into a host block beside it instead.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SilverfishMergeWithStoneGoal;

impl SilverfishMergeWithStoneGoal {
    /// The stroll it is.
    fn stroll() -> RandomStrollGoal {
        RandomStrollGoal {
            interval: 10,
            ..RandomStrollGoal::new(1.0)
        }
    }

    /// The block beside it towards `direction` (`Direction`'s ordinal: down, up, north,
    /// south, west, east), from `BlockPos.containing(x, y + 0.5, z)`.
    fn beside(ai: &Ai<'_, '_>, direction: u8) -> BlockPos {
        let [x, y, z] = ai.position();
        let at = crate::physics::containing([x, y + 0.5, z]);
        let [dx, dy, dz] = match direction {
            0 => [0, -1, 0],
            1 => [0, 1, 0],
            2 => [0, 0, -1],
            3 => [0, 0, 1],
            4 => [-1, 0, 0],
            _ => [1, 0, 0],
        };
        BlockPos::new(at.x + dx, at.y + dy, at.z + dz)
    }
}

impl Goal for SilverfishMergeWithStoneGoal {
    type State = Merging;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, state: &mut Merging, ai: &mut Ai<'_, '_>) -> bool {
        if ai.tick.mob.body.combat.target.is_some() || !navigation::is_done(ai) {
            return false;
        }
        if ai.tick.draws.next_int(reduced_tick_delay(10) as u32) == 0 {
            // `Direction.getRandom(random)`.
            state.selected_direction = ai.tick.draws.next_int(6) as u8;
            let pos = Self::beside(ai, state.selected_direction);
            if is_host(ai.tick.world.state(pos)) {
                state.do_merge = true;
                return true;
            }
        }
        state.do_merge = false;
        Self::stroll().can_use(&mut state.stroll, ai)
    }

    fn can_continue(&self, state: &mut Merging, ai: &mut Ai<'_, '_>) -> bool {
        !state.do_merge && Self::stroll().can_continue(&mut state.stroll, ai)
    }

    /// Merged: the host block made infested (`setBlockAndUpdate`), its spawn's smoke
    /// (`spawnAnim`), and itself gone (`discard`).
    fn start(&self, state: &mut Merging, ai: &mut Ai<'_, '_>) {
        if !state.do_merge {
            Self::stroll().start(&mut state.stroll, ai);
            return;
        }
        let pos = Self::beside(ai, state.selected_direction);
        let host = ai.tick.world.state(pos);
        if is_host(host) {
            ai.tick.out.local.push(Local::SetBlock {
                pos,
                state: infested_by_host(host),
                flags: 3,
            });
            ai.tick.out.events.push(Event::Entity(20));
            ai.tick.out.removed = Some(Removed::Discarded);
        }
    }

    fn stop(&self, state: &mut Merging, ai: &mut Ai<'_, '_>) {
        Self::stroll().stop(&mut state.stroll, ai);
    }
}

/// The silverfish.
pub(crate) struct Silverfish;

impl MobType for Silverfish {
    fn register_goals(&self, setup: &mut Setup) {
        register_goals(setup);
    }

    /// `Silverfish.tick`'s head: its body to its yaw.
    fn before_tick(&self, tick: &mut MobTick<'_>) {
        tick.mob.body.head.body_yaw = tick.yaw;
    }

    /// `setYBodyRot(rot)`'s `setYRot(rot)`: its yaw to where its body turned.
    fn after_tick(&self, tick: &mut MobTick<'_>) {
        tick.yaw = tick.mob.body.head.body_yaw;
    }

    /// `getWalkTargetValue`: 10 above a host block, else a monster's.
    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        let below = ai.tick.world.state(BlockPos::new(pos.x, pos.y - 1, pos.z));
        if is_host(below) {
            10.0
        } else {
            monster_ai::walk_target_value(ai, pos)
        }
    }

    /// `Silverfish.hurtServer`'s head: hurt by something, or by what always wakes them,
    /// it asks its friends' waking.
    fn before_hurt_server(&self, struck: &mut Struck<'_>, source: &DamageSource, _: f32) {
        if source.cause.is_some() || source.kind.is("minecraft:always_triggers_silverfish") {
            notify_hurt(struck.mob);
        }
    }

    fn plays_step_sounds(&self) -> bool {
        false
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SILVERFISH_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_SILVERFISH_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SILVERFISH_DEATH)
    }

    fn experience_reward(&self, _: u16, mob: &Mob, draws: &mut Random) -> i32 {
        super::illagers::experience(5, mob, draws)
    }
}
