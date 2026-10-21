//! What the four fish share (`AbstractFish`, `AbstractSchoolingFish`,
//! `FollowFlockLeaderGoal`; `docs/groundwork/n7-brief.md`, sections 6.6.1 to 6.6.4): the
//! fish's move control and its own swim, its flop on land and its air, its goals, its
//! bucket, and the schools of the cod, the salmon and the tropical fish. Each fish's own
//! module (`cod`, `salmon`, `pufferfish`, `tropical_fish`) builds on this.
//!
//! A fish keeps no `Animal` part: its type state is its own encoding ([`FishState`]),
//! under a version of its own that no animal's reading takes for an animal's.
//!
//! What the game does not save of a fish (its school, a pufferfish's counters) is kept
//! here as every tick's state is (the plan's 11.2), and a reload of the region is no
//! reload for it. Counters that count every tick are stamps against `lived`.
//!
//! Named differences: a school keeps to one region (a fish that only a copy shows is
//! neither followed nor counted as a follower, as the game's would be after a reload,
//! the brief's 11.6.3); the census of a leader counts the copies' fish too; the swim's
//! sound and the splash are not made (the living body plays no swimming sounds yet).

use clustine_data::{PathType, entity_types};
use clustine_world::{EntityData, EntityId, Entry, TypeState};
use serde::{Deserialize, Serialize};

use super::{Interaction, Used, water};
use crate::living::{Sinks, Travel};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{AvoidEntityGoal, PanicGoal, RandomStrollGoal, Strolling};
use crate::mobs::ai::maths::{move_rotlerp, yaw_towards};
use crate::mobs::ai::navigation::{self, Kind, Settings};
use crate::mobs::ai::selector::Goal;
use crate::mobs::ai::sensing::{self, Class};
use crate::mobs::ai::{Ai, Operation, Setup};
use crate::mobs::part::mind_ids;
use crate::mobs::tick::{Local, MobTick};
use crate::physics::aabb::Aabb;
use crate::region::Random;
use crate::shown::DataShown;

/// The version of a fish's type state: high, so that it is never an animal's.
const VERSION: u8 = 129;

/// The synced data's indices: `FROM_BUCKET` (16) and each fish's own (17).
const FROM_BUCKET_INDEX: u8 = 16;
pub(crate) const OWN_INDEX: u8 = 17;

/// `AbstractFish.getMaxSchoolSize()`, and the salmon's.
const MAX_SCHOOL: i32 = 8;
const SALMON_MAX_SCHOOL: i32 = 5;

/// The slot of `FollowFlockLeaderGoal` among a schooling fish's goals: after the panic,
/// the avoidance and the swim.
const FLOCK_SLOT: u16 = 3;

/// The four fish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Species {
    Cod,
    Salmon,
    Pufferfish,
    TropicalFish,
}

impl Species {
    /// The fish of the entity type `kind`.
    pub(crate) fn of(kind: u16) -> Option<Self> {
        Some(match i32::from(kind) {
            entity_types::COD => Self::Cod,
            entity_types::SALMON => Self::Salmon,
            entity_types::PUFFERFISH => Self::Pufferfish,
            entity_types::TROPICAL_FISH => Self::TropicalFish,
            _ => return None,
        })
    }

    /// Whether it is an `AbstractSchoolingFish`.
    pub(crate) fn schools(self) -> bool {
        self != Self::Pufferfish
    }

    /// `getMaxSchoolSize()`.
    pub(crate) fn max_school(self) -> i32 {
        if self == Self::Salmon {
            SALMON_MAX_SCHOOL
        } else {
            MAX_SCHOOL
        }
    }

    /// Its own synced value as `defineSynchedData` has it: a salmon's medium size, a
    /// pufferfish's flat state, a tropical fish's packed kob, white on white.
    fn fresh_variant(self) -> i32 {
        match self {
            Self::Salmon => 1,
            _ => 0,
        }
    }
}

/// What a fish keeps: whether it came from a bucket (`FROM_BUCKET`), its school
/// (`leader`, `schoolSize`), its own value (a salmon's size, a tropical fish's packed
/// variant), and a pufferfish's puff with the stamps of its counters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct FishState {
    pub(crate) from_bucket: bool,
    pub(crate) leader: Option<EntityId>,
    pub(crate) school_size: i32,
    pub(crate) variant: i32,
    pub(crate) puff: i32,
    /// The tick whose pufferfish's step reads `inflateCounter` as 1, while it counts.
    pub(crate) inflate_from: Option<u32>,
    /// The tick whose step reads `deflateTimer` as 0 and counts it on, while it counts.
    pub(crate) deflate_from: Option<u32>,
}

impl FishState {
    /// A fresh fish of `species`.
    pub(crate) fn fresh(species: Species) -> Self {
        Self {
            from_bucket: false,
            leader: None,
            school_size: 1,
            variant: species.fresh_variant(),
            puff: 0,
            inflate_from: None,
            deflate_from: None,
        }
    }

    /// `hasFollowers()`.
    pub(crate) fn has_followers(&self) -> bool {
        self.school_size > 1
    }

    /// `canBeFollowed()`.
    fn can_be_followed(&self, species: Species) -> bool {
        self.has_followers() && self.school_size < species.max_school()
    }
}

/// A fish's state from its type state: a record of nothing, or one this build cannot
/// read, as a fresh fish's.
pub(crate) fn read(kind: u16, state: &TypeState) -> FishState {
    let species = Species::of(kind).unwrap_or(Species::Cod);
    if state.version != VERSION || state.bytes.is_empty() {
        return FishState::fresh(species);
    }
    postcard::from_bytes(&state.bytes).unwrap_or_else(|_| FishState::fresh(species))
}

/// The type state of `fish`: nothing for a fresh one's.
pub(crate) fn write(kind: u16, fish: &FishState) -> TypeState {
    let species = Species::of(kind).unwrap_or(Species::Cod);
    if *fish == FishState::fresh(species) {
        return TypeState::default();
    }
    TypeState {
        version: VERSION,
        bytes: postcard::to_allocvec(fish).expect("a type state is encoded into memory"),
    }
}

/// Changes the fish state of `mob`, of the type `kind`.
pub(crate) fn update(kind: u16, mob: &mut Mob, change: impl FnOnce(&mut FishState)) {
    let mut fish = read(kind, &mob.state);
    let before = fish.clone();
    change(&mut fish);
    if fish != before {
        mob.state = write(kind, &fish);
    }
}

/// `setFromBucket(true)`, of a fish poured from a bucket.
pub(crate) fn set_from_bucket(kind: u16, mob: &mut Mob) {
    update(kind, mob, |fish| fish.from_bucket = true);
}

/// `AbstractFish.registerGoals` (and `AbstractSchoolingFish`'s, a schooling fish's
/// flock after), its navigation and its constructor's malus.
pub(crate) fn register_goals(setup: &mut Setup, species: Species) {
    setup.config.navigation = Settings::of(Kind::WaterBound { breach: false });
    // `WaterAnimal`'s constructor: water costs it nothing.
    setup.set_malus(PathType::Water, 0.0);
    setup.add_goal(0, PanicGoal { speed: 1.25 });
    setup.add_goal(
        2,
        AvoidEntityGoal {
            class: Class::Player,
            max_distance: 8.0,
            walk_speed: 1.6,
            sprint_speed: 1.4,
        },
    );
    setup.add_goal(4, FishSwimGoal);
    if species.schools() {
        setup.add_goal(5, FollowFlockLeaderGoal { species });
    }
}

/// The schooling fish's constructor's draw: `FollowFlockLeaderGoal`'s first
/// `nextStartTick`, in its slot, as `registerGoals` makes it in `Mob`'s constructor.
pub(crate) fn construct(mob: &mut Mob, species: Species, own: &mut Random) {
    if !species.schools() {
        return;
    }
    let flocking = Flocking {
        time_to_recalc: 0,
        next_start_tick: next_start_tick(own),
    };
    let id = mind_ids::FIRST_GOAL + FLOCK_SLOT;
    let entry = Entry {
        id,
        version: <FollowFlockLeaderGoal as Goal>::VERSION,
        bytes: postcard::to_allocvec(&flocking).expect("a goal's state is encoded into memory"),
    };
    let slots = &mut mob.mind.slots;
    match slots.binary_search_by_key(&id, |entry| entry.id) {
        Ok(at) => slots[at] = entry,
        Err(at) => slots.insert(at, entry),
    }
}

/// `FollowFlockLeaderGoal.nextStartTick(mob)`: `reducedTickDelay(200 + nextInt(200) %
/// 20)`, one draw.
fn next_start_tick(draws: &mut Random) -> i32 {
    crate::mobs::ai::maths::reduced_tick_delay(200 + draws.next_int(200) as i32 % 20)
}

/// `AbstractFish.aiStep`'s flop, before the living one: on the ground out of water, a
/// hop of 0.4 with a little sideways (two draws), off the ground, its motion sent at
/// once, and its flop's sound (two draws more).
pub(crate) fn flop(tick: &mut MobTick<'_>, sound: i32) {
    let body = &tick.living.body;
    if body.in_water || !body.on_ground || !body.vertical_collision {
        return;
    }
    let x = (tick.draws.next_float() * 2.0 - 1.0) * 0.05;
    let z = (tick.draws.next_float() * 2.0 - 1.0) * 0.05;
    let body = &mut tick.living.body;
    body.motion[0] += f64::from(x);
    body.motion[1] += 0.400_000_005_960_464_5;
    body.motion[2] += f64::from(z);
    body.on_ground = false;
    tick.out.needs_sync = true;
    water::make_sound(tick, sound, 1.0, false);
}

/// `AbstractFish.travelInWater`: `0.01f` along its input, and a sink of 0.005 while it
/// has no target.
pub(crate) fn travel(tick: &MobTick<'_>) -> Travel {
    let sinks = if tick.mob.body.combat.target.is_none() {
        Sinks::Always
    } else {
        Sinks::Never
    };
    Travel::Swims { speed: 0.01, sinks }
}

/// `Mob.setSpeed(speed)`, which sets `zza` too.
fn set_speed(ai: &mut Ai<'_, '_>, speed: f32) {
    let inputs = &mut ai.tick.mob.body.inputs;
    inputs.speed = speed;
    inputs.zza = speed;
}

/// `AbstractFish.FishMoveControl.tick()`: buoyed while its eyes are in water; towards
/// what it wants at a speed eased an eighth of the way each tick, the height pushed
/// into its motion straight, the yaw turned up to 90 a tick. Its operation is left as it
/// was.
pub(crate) fn tick_move_control(ai: &mut Ai<'_, '_>) {
    if water::eye_in_water(ai.tick) {
        ai.tick.living.body.motion[1] += 0.005;
    }
    let movement = ai.tick.mob.body.controls.movement.clone();
    if movement.operation != Operation::MoveTo || navigation::is_done(ai) {
        set_speed(ai, 0.0);
        return;
    }
    let target = (movement.speed * ai.attribute(clustine_data::attributes::MOVEMENT_SPEED)) as f32;
    let speed = ai.tick.mob.body.inputs.speed;
    set_speed(ai, speed + 0.125 * (target - speed));
    let [x, y, z] = ai.position();
    let dx = movement.wanted[0] - x;
    let dy = movement.wanted[1] - y;
    let dz = movement.wanted[2] - z;
    if dy != 0.0 {
        let distance = (dx * dx + dy * dy + dz * dz).sqrt();
        let speed = f64::from(ai.tick.mob.body.inputs.speed);
        ai.tick.living.body.motion[1] += speed * (dy / distance) * 0.1;
    }
    if dx != 0.0 || dz != 0.0 {
        let yaw = yaw_towards(dx, dz);
        ai.tick.yaw = move_rotlerp(ai.tick.yaw, yaw, 90.0);
        ai.tick.mob.body.head.body_yaw = ai.tick.yaw;
    }
}

/// `AbstractFish.mobInteract`: a water bucket takes it; anything else passes.
pub(crate) fn mob_interact(used: &mut Used<'_>) -> Interaction {
    water::bucket_pickup(used).unwrap_or(Interaction::Pass)
}

/// `AbstractFish.removeWhenFarAway`: unless it came from a bucket (no fish has a name).
pub(crate) fn remove_when_far_away(kind: u16, mob: &Mob) -> bool {
    !read(kind, &mob.state).from_bucket
}

/// The synced values of a fish: `FROM_BUCKET` (16) and its own value (17), where it has
/// one.
pub(crate) fn shown(kind: u16, mob: &Mob, own: bool) -> Vec<(u8, DataShown, bool)> {
    let species = Species::of(kind).unwrap_or(Species::Cod);
    let fish = read(kind, &mob.state);
    let mut values = vec![(
        FROM_BUCKET_INDEX,
        DataShown::Boolean(fish.from_bucket),
        !fish.from_bucket,
    )];
    if own {
        let value = if species == Species::Pufferfish {
            fish.puff
        } else {
            fish.variant
        };
        let fresh = if species == Species::Pufferfish {
            0
        } else {
            species.fresh_variant()
        };
        values.push((OWN_INDEX, DataShown::Int(value), value == fresh));
    }
    values
}

/// A fish of the view as a school reads it: its key, whether a copy shows it, its fish
/// state.
#[derive(Debug, Clone)]
struct Schoolmate {
    key: EntityId,
    copied: bool,
    fish: FishState,
}

/// The fish of `kind` the view has in `bounds`, in its order, the mob itself as its
/// tick has it.
fn schoolmates(ai: &Ai<'_, '_>, bounds: &Aabb) -> Vec<Schoolmate> {
    let kind = ai.tick.kind;
    ai.tick
        .world
        .entities_in(bounds)
        .iter()
        .filter(|seen| seen.entity.kind == kind)
        .filter_map(|seen| {
            if seen.entity.key == ai.tick.key {
                return Some(Schoolmate {
                    key: ai.tick.key,
                    copied: false,
                    fish: read(kind, &ai.tick.mob.state),
                });
            }
            let EntityData::Mob { state, .. } = &seen.entity.data else {
                return None;
            };
            Some(Schoolmate {
                key: seen.entity.key,
                copied: seen.copied,
                fish: read(kind, state),
            })
        })
        .collect()
}

/// Whether the fish whose state is `fish` follows a leader that lives
/// (`isFollower()`): one of the region's own, its record or, for the mob itself,
/// its tick.
fn follows(ai: &Ai<'_, '_>, fish: &FishState) -> bool {
    let Some(leader) = fish.leader else {
        return false;
    };
    if leader == ai.tick.key {
        return water::alive(&ai.tick.mob);
    }
    ai.tick
        .world
        .entity(leader)
        .is_some_and(|seen| !seen.copied && sensing::Looked::of_seen(&seen).alive)
}

/// The mob's leader as the view has it, if it follows one.
fn leader(ai: &Ai<'_, '_>) -> Option<sensing::Looked> {
    let fish = read(ai.tick.kind, &ai.tick.mob.state);
    if !follows(ai, &fish) {
        return None;
    }
    sensing::find(ai, fish.leader?)
}

/// `canRandomSwim()`: a schooling fish swims about only while it follows nobody.
fn can_random_swim(ai: &Ai<'_, '_>) -> bool {
    let schools = Species::of(ai.tick.kind).is_some_and(Species::schools);
    !schools || !follows(ai, &read(ai.tick.kind, &ai.tick.mob.state))
}

/// `AbstractFish.FishSwimGoal`: `RandomSwimmingGoal(fish, 1.0, 40)` while the fish may
/// swim about.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FishSwimGoal;

impl FishSwimGoal {
    fn inner() -> RandomStrollGoal {
        RandomStrollGoal::swimming(1.0, 40)
    }
}

impl Goal for FishSwimGoal {
    type State = Strolling;

    fn flags(&self) -> u8 {
        Self::inner().flags()
    }

    fn can_use(&self, state: &mut Strolling, ai: &mut Ai<'_, '_>) -> bool {
        can_random_swim(ai) && Self::inner().can_use(state, ai)
    }

    fn can_continue(&self, state: &mut Strolling, ai: &mut Ai<'_, '_>) -> bool {
        Self::inner().can_continue(state, ai)
    }

    fn start(&self, state: &mut Strolling, ai: &mut Ai<'_, '_>) {
        Self::inner().start(state, ai);
    }

    fn stop(&self, state: &mut Strolling, ai: &mut Ai<'_, '_>) {
        Self::inner().stop(state, ai);
    }
}

/// `FollowFlockLeaderGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Flocking {
    pub(crate) time_to_recalc: i32,
    pub(crate) next_start_tick: i32,
}

/// `FollowFlockLeaderGoal(fish)`: a fish alone looks every five seconds or so for a
/// school to join, or gathers one of its own; a follower keeps within 11 blocks of its
/// leader. No flags.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FollowFlockLeaderGoal {
    pub(crate) species: Species,
}

impl FollowFlockLeaderGoal {
    /// `stopFollowing()`: one fewer in the leader's school, and no leader.
    fn stop_following(ai: &mut Ai<'_, '_>) {
        let kind = ai.tick.kind;
        let fish = read(kind, &ai.tick.mob.state);
        let Some(leader) = fish.leader else {
            return;
        };
        if leader == ai.tick.key {
            update(kind, &mut ai.tick.mob, |fish| {
                fish.school_size -= 1;
                fish.leader = None;
            });
        } else {
            ai.tick.out.local.push(Local::LeaveSchool { leader });
            update(kind, &mut ai.tick.mob, |fish| fish.leader = None);
        }
    }

    /// The census and the joining of `canUse`: of the fish of its kind within 8 blocks
    /// that may be followed or follow nobody (itself among them), the first that may be
    /// followed leads, else the mob itself; the leader takes as many of those that
    /// follow nobody as its school has room for, counting itself among them where it is,
    /// and leaving itself out.
    fn gather(&self, ai: &mut Ai<'_, '_>) {
        let bounds = Aabb::of_entity(ai.position(), ai.width(), ai.height()).inflate([8.0; 3]);
        let species = self.species;
        let candidates: Vec<Schoolmate> = schoolmates(ai, &bounds)
            .into_iter()
            .filter(|mate| !mate.copied)
            .filter(|mate| mate.fish.can_be_followed(species) || !follows(ai, &mate.fish))
            .collect();
        let me = ai.tick.key;
        let leader = candidates
            .iter()
            .find(|mate| mate.fish.can_be_followed(species))
            .map_or(me, |mate| mate.key);
        let leader_size = candidates
            .iter()
            .find(|mate| mate.key == leader)
            .map_or(1, |mate| mate.fish.school_size);
        let room = usize::try_from(species.max_school() - leader_size).unwrap_or(0);
        let joining: Vec<EntityId> = candidates
            .iter()
            .filter(|mate| !follows(ai, &mate.fish))
            .take(room)
            .map(|mate| mate.key)
            .filter(|key| *key != leader)
            .collect();
        if joining.is_empty() {
            return;
        }
        let count = joining.len() as i32;
        let kind = ai.tick.kind;
        if joining.contains(&me) {
            update(kind, &mut ai.tick.mob, |fish| fish.leader = Some(leader));
        }
        if leader == me {
            update(kind, &mut ai.tick.mob, |fish| fish.school_size += count);
        }
        let others: Vec<EntityId> = joining.into_iter().filter(|key| *key != me).collect();
        let leader_gains = if leader == me { 0 } else { count };
        if !others.is_empty() || leader_gains != 0 {
            ai.tick.out.local.push(Local::JoinSchool {
                leader,
                followers: others,
                leader_gains,
            });
        }
    }
}

impl Goal for FollowFlockLeaderGoal {
    type State = Flocking;

    fn flags(&self) -> u8 {
        0
    }

    fn can_use(&self, state: &mut Flocking, ai: &mut Ai<'_, '_>) -> bool {
        let fish = read(ai.tick.kind, &ai.tick.mob.state);
        if fish.has_followers() {
            return false;
        }
        if follows(ai, &fish) {
            return true;
        }
        if state.next_start_tick > 0 {
            state.next_start_tick -= 1;
            return false;
        }
        state.next_start_tick = next_start_tick(ai.tick.draws);
        self.gather(ai);
        follows(ai, &read(ai.tick.kind, &ai.tick.mob.state))
    }

    fn can_continue(&self, _: &mut Flocking, ai: &mut Ai<'_, '_>) -> bool {
        // `isFollower() && inRangeOfLeader()`: within 11 blocks.
        leader(ai).is_some_and(|leader| leader.distance_sqr(ai.position()) <= 121.0)
    }

    fn start(&self, state: &mut Flocking, _: &mut Ai<'_, '_>) {
        state.time_to_recalc = 0;
    }

    fn stop(&self, _: &mut Flocking, ai: &mut Ai<'_, '_>) {
        Self::stop_following(ai);
    }

    fn tick(&self, state: &mut Flocking, ai: &mut Ai<'_, '_>) {
        state.time_to_recalc -= 1;
        if state.time_to_recalc > 0 {
            return;
        }
        state.time_to_recalc = self.adjusted_tick_delay(10);
        // `pathToLeader()`.
        if let Some(leader) = leader(ai) {
            navigation::move_to_entity(ai, leader.block_position(), 1.0);
        }
    }
}

/// `AbstractSchoolingFish.tick()` after the fish's whole tick: a leader with followers
/// counts its school once in about 200 ticks (a draw of the level's on its behalf every
/// tick), and finding none of its kind about but itself, leads nobody. Its followers keep
/// their leader, and the count is wrong after, as the game's is.
pub(crate) fn census(tick: &mut MobTick<'_>) {
    let kind = tick.kind;
    if !read(kind, &tick.mob.state).has_followers() {
        return;
    }
    if tick.draws.next_int(200) != 1 {
        return;
    }
    let body = &tick.living.body;
    let bounds = Aabb::of_entity(body.position, body.width, body.height).inflate([8.0; 3]);
    let found = tick
        .world
        .entities_in(&bounds)
        .iter()
        .filter(|seen| seen.entity.kind == kind)
        .count();
    if found <= 1 {
        update(kind, &mut tick.mob, |fish| fish.school_size = 1);
    }
}

/// What the region does to a fish of its own that a school's gathering names: it
/// follows `leader` (`startFollowing`).
pub(crate) fn start_following(kind: u16, mob: &mut Mob, leader: EntityId) {
    update(kind, mob, |fish| fish.leader = Some(leader));
}

/// What the region does to the leader of a school that others joined or left: its
/// school `by` larger.
pub(crate) fn school_grew(kind: u16, mob: &mut Mob, by: i32) {
    update(kind, mob, |fish| fish.school_size += by);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_fish_keeps_no_state_and_a_school_s_size_starts_at_one() {
        let kind = entity_types::COD as u16;
        let fresh = FishState::fresh(Species::Cod);
        assert_eq!(fresh.school_size, 1);
        assert_eq!(write(kind, &fresh), TypeState::default());
        let mut grown = fresh.clone();
        grown.school_size = 3;
        let state = write(kind, &grown);
        assert_eq!(read(kind, &state), grown);
        // A salmon's fresh size is medium.
        assert_eq!(FishState::fresh(Species::Salmon).variant, 1);
    }

    #[test]
    fn the_flock_goal_waits_one_hundred_to_one_hundred_and_ten_of_its_ticks_to_look() {
        for seed in 0..200 {
            let mut draws = Random::of_entity(seed, EntityId(1), 0);
            let ticks = next_start_tick(&mut draws);
            assert!((100..=110).contains(&ticks), "{ticks}");
        }
    }
}
