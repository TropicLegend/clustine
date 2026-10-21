//! Bees (`Bee` and its nested classes; `docs/groundwork/n7-brief.md`, section 6.4): a
//! flying animal of flowers that hovers once it has first moved, finds flowers within
//! five blocks and pollinates them, grows the crops it flies over with its nectar, finds
//! a beehive or bee nest within 20 blocks and goes home with nectar, at night (and in
//! rain, which Clustine does not have), or after three minutes without. It is neutral:
//! what hurts it, it stings once, and dies of it within a minute.
//!
//! Its hive and its flower are positions it keeps; one beyond the blocks the region
//! reads is "unknown" and kept (the plan's 7.5.1), where the game reads and loads it.
//! Entering and leaving a hive is the hive's, `crate::region::hives`.
//!
//! Its counters (the time since it left its hive without nectar, the cooldowns of
//! looking for a hive and a flower, the time out of its hive it was told to stay) are
//! kept as the moments they count from or end at (`docs/groundwork/n7-plan.md`, section
//! 11.2), so that a bee hovering at rest commits nothing for them.
//!
//! Named differences: the weather is clear, so rain neither keeps bees home nor stops
//! their pollinating; a sting poisons nothing and counts no stinger on what it struck
//! (no mob effects, no stinger count), and is taken as having struck (the hurt is the
//! target's, after the bee's tick); a hurt bee's alert of the bees near it is not built;
//! the flower effect of a wither rose or an open eyeblossom fed to it is not applied
//! (they are only taken); a bee floats up out of water as other mobs do, not at its
//! quarter rate (`jumpInLiquid`); the visited nodes' multiplier its trips set
//! (`setMaxVisitedNodesMultiplier`) is not kept, so its searches see as many nodes as
//! its range gives; hives are found by a scan of the blocks the region reads, not by
//! points of interest, ties of distance in the order of chunk, section, height, z and
//! x; its nectar's drips are drawn (one draw a tick, and the level's two per drip) and
//! not sent.

use clustine_blocks::data::{flag, in_tag, is_block, is_class, number, set, set_flag};
use clustine_data::{BlockClass, BlockState, PathType, blocks, items, sounds};
use clustine_world::{BlockPos, ChunkPos};
use serde::{Deserialize, Serialize};

use super::animal::{self, Own};
use super::neutral::{self, Anger};
use super::tamable::Idle;
use super::{Breeding, Dimensions, Interaction, MobType, Used, cow};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    Attacking, FloatGoal, HurtByTargetGoal, Items, MeleeAttackGoal, NearestAttackableTargetGoal,
    Targeting, TemptGoal, manhattan_ordered, view_vector,
};
use crate::mobs::ai::navigation::{self, Kind, Settings};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::{self, Class, Looked};
use crate::mobs::ai::{Ai, MoveKind, Operation, Setup, controls, random_pos};
use crate::mobs::body::hurt::{DamageSource, DamageType};
use crate::mobs::body::{self, Event};
use crate::mobs::tick::{Local, MobTick};
use crate::mobs::view::MobWorld;
use crate::physics::{self, Vec3};
use crate::region::Random;
use crate::shown::DataShown;

/// `#bee_food`: the flowers.
const FOOD: Items = Items::Tag("minecraft:bee_food");

/// A baby bee's size: the type's (0.55 by 0.5, eyes at 0.3) at `AgeableMob`'s half.
const BABY: Dimensions = Dimensions {
    width: 0.55 * 0.5,
    height: 0.5 * 0.5,
    eye_height: 0.3 * 0.5,
};

/// The synced data's indices: `Entity`'s no gravity, the bee's flags and its anger's end.
const NO_GRAVITY_INDEX: u8 = 5;
const FLAGS_INDEX: u8 = 18;
const ANGER_INDEX: u8 = 19;

/// The bits of `DATA_FLAGS_ID`.
pub(crate) mod flag_bits {
    pub(crate) const ROLL: u8 = 2;
    pub(crate) const HAS_STUNG: u8 = 4;
    pub(crate) const HAS_NECTAR: u8 = 8;
}

/// `TOO_FAR_DISTANCE`: a hive or flower further than this is forgotten.
const TOO_FAR: i32 = 48;

/// `TICKS_WITHOUT_NECTAR_BEFORE_GOING_HOME`.
const TIRED_OF_LOOKING: i64 = 3600;

/// `COOLDOWN_BEFORE_LOCATING_NEW_HIVE` and `_FLOWER`.
const LOCATE_COOLDOWN: u32 = 200;

/// `MAX_CROPS_GROWABLE`.
const MAX_CROPS: i32 = 10;

/// What a bee keeps beyond an animal: its flags, its anger, its counters, its hive and
/// flower, the hives it gave up on (`BeeGoToHiveGoal`'s blacklist, which its locating
/// reads too), whether it pollinates (`BeePollinateGoal.pollinating`, which its hive and
/// its hurt read), whether gravity has left it, and the cooldowns its validating goals
/// drew as it was made.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct BeeState {
    pub(crate) flags: u8,
    pub(crate) anger: Anger,
    pub(crate) time_since_sting: i32,
    /// The moment `ticksWithoutNectarSinceExitingHive` counts from.
    pub(crate) nectarless_from: u32,
    /// The moments `stayOutOfHiveCountdown`, `remainingCooldownBeforeLocatingNewHive`
    /// and `...NewFlower` come to nothing.
    pub(crate) stay_out_until: u32,
    pub(crate) crops_grown: i32,
    pub(crate) hive_cooldown_until: u32,
    pub(crate) flower_cooldown_until: u32,
    pub(crate) flower: Option<BlockPos>,
    pub(crate) hive: Option<BlockPos>,
    pub(crate) under_water_ticks: i32,
    pub(crate) no_gravity: bool,
    pub(crate) pollinating: bool,
    pub(crate) blacklist: Vec<BlockPos>,
    pub(crate) validate_hive_cooldown: i32,
    pub(crate) validate_flower_cooldown: i32,
}

impl Own for BeeState {}

impl BeeState {
    pub(crate) fn has(&self, bit: u8) -> bool {
        self.flags & bit != 0
    }

    pub(crate) fn set(&mut self, bit: u8, on: bool) {
        if on {
            self.flags |= bit;
        } else {
            self.flags &= !bit;
        }
    }

    /// `ticksWithoutNectarSinceExitingHive` as its goals read it in the tick that began
    /// at `now`: one a tick from its last reset, and none with nectar (gaining nectar
    /// resets it, and it does not count on while the bee has some).
    pub(crate) fn ticks_without_nectar(&self, now: u32) -> i64 {
        if self.has(flag_bits::HAS_NECTAR) {
            0
        } else {
            (i64::from(now) - i64::from(self.nectarless_from)).max(0)
        }
    }

    /// `resetTicksWithoutNectarSinceExitingHive()` at the moment `now`.
    pub(crate) fn reset_ticks_without_nectar(&mut self, now: u32) {
        self.nectarless_from = now;
    }

    /// What is left at `now` of a countdown that ends at `until`.
    fn left(until: u32, now: u32) -> i64 {
        (i64::from(until) - i64::from(now)).max(0)
    }

    pub(crate) fn stay_out_of_hive(&self, now: u32) -> i64 {
        Self::left(self.stay_out_until, now)
    }

    pub(crate) fn hive_cooldown(&self, now: u32) -> i64 {
        Self::left(self.hive_cooldown_until, now)
    }

    pub(crate) fn flower_cooldown(&self, now: u32) -> i64 {
        Self::left(self.flower_cooldown_until, now)
    }

    /// `remainingCooldownBeforeLocatingNewFlower = ticks` at the moment `now`.
    pub(crate) fn set_flower_cooldown(&mut self, now: u32, ticks: i32) {
        self.flower_cooldown_until = now.saturating_add(ticks.max(0).unsigned_abs());
    }

    /// `setHasNectar(nectar)` at the moment `now`: with nectar it has just come from a
    /// flower.
    pub(crate) fn set_has_nectar(&mut self, nectar: bool, now: u32) {
        if nectar {
            self.reset_ticks_without_nectar(now);
        }
        self.set(flag_bits::HAS_NECTAR, nectar);
    }

    /// `dropOffNectar()`: what a hive takes of it.
    pub(crate) fn drop_off_nectar(&mut self) {
        self.set(flag_bits::HAS_NECTAR, false);
        self.crops_grown = 0;
    }

    /// `dropHive()` at the moment `now`.
    fn drop_hive(&mut self, now: u32) {
        self.hive = None;
        self.hive_cooldown_until = now + LOCATE_COOLDOWN;
    }

    /// `dropFlower()` at the moment `now`: its cooldown drawn again.
    fn drop_flower(&mut self, draws: &mut Random, now: u32) {
        self.flower = None;
        let ticks = draws.next_int_between(20, 60);
        self.set_flower_cooldown(now, ticks);
    }

    /// `blacklistTarget(pos)`: the three it gave up on last.
    fn blacklist(&mut self, pos: BlockPos) {
        self.blacklist.push(pos);
        while self.blacklist.len() > 3 {
            self.blacklist.remove(0);
        }
    }
}

/// The bee.
pub(crate) struct Bee;

pub(crate) fn state_of(mob: &Mob) -> BeeState {
    animal::read::<BeeState>(&mob.state).1
}

pub(crate) fn put_state(mob: &mut Mob, own: &BeeState) {
    let animal = animal::animal_of(&mob.state);
    mob.state = animal::write(&animal, own);
}

fn update(mob: &mut Mob, change: impl FnOnce(&mut BeeState)) {
    let mut own = state_of(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        put_state(mob, &own);
    }
}

/// The world's game time as anger and the validating goals read it.
fn game_time(world: &dyn MobWorld) -> i64 {
    world.game_time()
}

/// `Vec3i.closerThan(other, d)`: the low corners' squared distance below `d²`.
fn closer_than(a: BlockPos, b: BlockPos, distance: i32) -> bool {
    let (dx, dy, dz) = (
        f64::from(a.x - b.x),
        f64::from(a.y - b.y),
        f64::from(a.z - b.z),
    );
    dx * dx + dy * dy + dz * dz < f64::from(distance * distance)
}

/// `Vec3i.distSqr(other)`.
fn dist_sqr(a: BlockPos, b: BlockPos) -> f64 {
    let (dx, dy, dz) = (
        f64::from(a.x - b.x),
        f64::from(a.y - b.y),
        f64::from(a.z - b.z),
    );
    dx * dx + dy * dy + dz * dz
}

/// `Vec3.atCenterOf(pos)`.
fn centre_of(pos: BlockPos) -> Vec3 {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) + 0.5,
    ]
}

/// `Vec3.atBottomCenterOf(pos)`.
fn bottom_centre_of(pos: BlockPos) -> Vec3 {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y),
        f64::from(pos.z) + 0.5,
    ]
}

/// Whether `state` is a beehive or a bee nest (`#beehives`).
pub(crate) fn is_hive(state: BlockState) -> bool {
    is_class(state, BlockClass::BeehiveBlock)
}

/// What the region knows of the hive at `pos`: whether it is one, and if so how many
/// bees it holds; `None` where the region does not read it (unknown: kept).
fn hive_at(world: &dyn MobWorld, pos: BlockPos) -> Option<Option<usize>> {
    if !world.is_loaded(pos) {
        return None;
    }
    if !is_hive(world.state(pos)) {
        return Some(None);
    }
    let count = world
        .block_entity_data(pos)
        .map_or(0, crate::region::hives::occupant_count);
    Some(Some(count))
}

/// `isHiveValid()`: a hive at its hive's place, not too far; one the region does not
/// read is kept as valid.
fn hive_valid(own: &BeeState, world: &dyn MobWorld, at: BlockPos) -> bool {
    let Some(hive) = own.hive else {
        return false;
    };
    if !closer_than(hive, at, TOO_FAR) {
        return false;
    }
    !matches!(hive_at(world, hive), Some(None))
}

/// `isHiveNearFire()`: fire (`FireBlock`, not soul fire) in the three by three by three
/// around its hive.
fn hive_near_fire(own: &BeeState, world: &dyn MobWorld, at: BlockPos) -> bool {
    let Some(hive) = own.hive else {
        return false;
    };
    if !hive_valid(own, world, at) || !world.is_loaded(hive) {
        return false;
    }
    (-1..=1).any(|dx| {
        (-1..=1).any(|dy| {
            (-1..=1).any(|dz| {
                let pos = BlockPos::new(hive.x + dx, hive.y + dy, hive.z + dz);
                is_class(world.state(pos), BlockClass::FireBlock)
            })
        })
    })
}

/// `EnvironmentAttributes.BEES_STAY_IN_HIVE` in the overworld: by the `day` timeline,
/// from tick 12542 to 23460 of the day (and in rain, which Clustine has not).
pub(crate) fn bees_stay_in_hive(day: u64) -> bool {
    (12_542..23_460).contains(&(day % crate::time::DAY))
}

/// `wantsToEnterHive()`: not told to stay out, pollinating, stung or set on a target;
/// with nectar, tired of looking, or at night; and no fire at its hive.
fn wants_to_enter_hive(ai: &Ai<'_, '_>) -> bool {
    let own = state_of(&ai.tick.mob);
    let now = ai.tick.lived;
    if own.stay_out_of_hive(now) > 0
        || own.pollinating
        || own.has(flag_bits::HAS_STUNG)
        || sensing::target(ai).is_some()
    {
        return false;
    }
    let wants = own.has(flag_bits::HAS_NECTAR)
        || own.ticks_without_nectar(now) > TIRED_OF_LOOKING
        || bees_stay_in_hive(ai.tick.world.day());
    wants && !hive_near_fire(&own, ai.tick.world, ai.block_position())
}

/// `isAngry()` of the bee in `ai`.
fn angry(ai: &Ai<'_, '_>) -> bool {
    state_of(&ai.tick.mob)
        .anger
        .is_angry(game_time(ai.tick.world))
}

/// `Bee.attractsBees(state)`: of `#bee_attractive`, not under water, and of a sunflower
/// only its top.
pub(crate) fn attracts_bees(state: BlockState) -> bool {
    if !in_tag(state, "minecraft:bee_attractive") {
        return false;
    }
    if state.property("waterlogged").is_some() && flag(state, "waterlogged") {
        return false;
    }
    if is_block(state, blocks::SUNFLOWER) {
        return state.property("half") == Some("upper");
    }
    true
}

/// `pathfindRandomlyTowards(pos)`: a place in the air towards `pos`, nearer as it nears,
/// up or down where it is far above or below, gone to at 1.
fn pathfind_randomly_towards(ai: &mut Ai<'_, '_>, pos: BlockPos) {
    let target = bottom_centre_of(pos);
    let at = ai.block_position();
    let dy = target[1] as i32 - at.y;
    let y_offset = if dy > 2 {
        4
    } else if dy < -2 {
        -4
    } else {
        0
    };
    let distance = (at.x - pos.x).abs() + (at.y - pos.y).abs() + (at.z - pos.z).abs();
    let (h, v) = if distance < 15 {
        (distance / 2, distance / 2)
    } else {
        (6, 8)
    };
    let Some(wanted) =
        random_pos::air_pos_towards(ai, h, v, y_offset, target, 0.314_159_274_101_257_3)
    else {
        return;
    };
    navigation::move_to(ai, wanted, 1.0);
}

/// `BaseBeeGoal`: a bee's goal that an angry bee does not take up or keep (the bee's own
/// test made first, with its draws).
trait BeeGoal {
    type State: Serialize + serde::de::DeserializeOwned + Default + PartialEq;
    fn flags(&self) -> u8 {
        0
    }
    fn every_tick(&self) -> bool {
        false
    }
    fn can_bee_use(&self, state: &mut Self::State, ai: &mut Ai<'_, '_>) -> bool;
    fn can_bee_continue(&self, state: &mut Self::State, ai: &mut Ai<'_, '_>) -> bool;
    fn start(&self, state: &mut Self::State, ai: &mut Ai<'_, '_>) {
        let _ = (state, ai);
    }
    fn stop(&self, state: &mut Self::State, ai: &mut Ai<'_, '_>) {
        let _ = (state, ai);
    }
    fn tick(&self, state: &mut Self::State, ai: &mut Ai<'_, '_>) {
        let _ = (state, ai);
    }
}

/// A [`BeeGoal`] as the selector holds it.
#[derive(Debug, Clone, Copy)]
struct Base<G>(G);

impl<G: BeeGoal + 'static> Goal for Base<G> {
    type State = G::State;

    fn flags(&self) -> u8 {
        self.0.flags()
    }

    fn every_tick(&self) -> bool {
        self.0.every_tick()
    }

    fn can_use(&self, state: &mut G::State, ai: &mut Ai<'_, '_>) -> bool {
        self.0.can_bee_use(state, ai) && !angry(ai)
    }

    fn can_continue(&self, state: &mut G::State, ai: &mut Ai<'_, '_>) -> bool {
        self.0.can_bee_continue(state, ai) && !angry(ai)
    }

    fn start(&self, state: &mut G::State, ai: &mut Ai<'_, '_>) {
        self.0.start(state, ai);
    }

    fn stop(&self, state: &mut G::State, ai: &mut Ai<'_, '_>) {
        self.0.stop(state, ai);
    }

    fn tick(&self, state: &mut G::State, ai: &mut Ai<'_, '_>) {
        self.0.tick(state, ai);
    }
}

/// `doHurtTarget` of a bee: its sting, of its attack damage as a whole number, which the
/// region lands after the bee's tick with its sound where it took; then it has stung and
/// is angry no more (taken as having struck: named).
fn sting(ai: &mut Ai<'_, '_>, target: &Looked) {
    let damage = ai.attribute(clustine_data::attributes::ATTACK_DAMAGE) as i32 as f32;
    ai.tick.out.local.push(Local::Sting {
        key: target.key,
        kind: ai.tick.kind,
        from: ai.position(),
        amount: damage,
    });
    let mut own = state_of(&ai.tick.mob);
    own.set(flag_bits::HAS_STUNG, true);
    neutral::stop_being_angry(&mut ai.tick.mob, &mut own.anger);
    put_state(&mut ai.tick.mob, &own);
}

/// `BeeAttackGoal(bee, 1.4, true)`: an angry bee that has not stung chases and stings.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BeeAttackGoal;

impl BeeAttackGoal {
    const MELEE: MeleeAttackGoal = MeleeAttackGoal {
        speed: 1.4,
        following_even_if_not_seen: true,
    };

    fn may(ai: &Ai<'_, '_>) -> bool {
        angry(ai) && !state_of(&ai.tick.mob).has(flag_bits::HAS_STUNG)
    }
}

impl Goal for BeeAttackGoal {
    type State = Attacking;

    fn flags(&self) -> u8 {
        Self::MELEE.flags()
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) -> bool {
        Self::MELEE.can_use(state, ai) && Self::may(ai)
    }

    fn can_continue(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) -> bool {
        Self::MELEE.can_continue(state, ai) && Self::may(ai)
    }

    fn start(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) {
        Self::MELEE.start(state, ai);
    }

    fn stop(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) {
        Self::MELEE.stop(state, ai);
    }

    fn tick(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) {
        let Some(target) = Self::MELEE.chase(state, ai) else {
            return;
        };
        if MeleeAttackGoal::can_perform_attack(state, ai, &target) {
            Self::MELEE.reset_attack_cooldown(state);
            sting(ai, &target);
        }
    }
}

/// `BeeEnterHiveGoal`: a bee that wants to go in, at its hive, goes in, if the hive has
/// room; at a full one it forgets it.
#[derive(Debug, Clone, Copy)]
struct EnterHive;

impl BeeGoal for EnterHive {
    type State = ();

    fn can_bee_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        let own = state_of(&ai.tick.mob);
        let Some(hive) = own.hive else {
            return false;
        };
        if !wants_to_enter_hive(ai) {
            return false;
        }
        if physics::distance_sqr(centre_of(hive), ai.position()) >= 4.0 {
            return false;
        }
        if !hive_valid(&own, ai.tick.world, ai.block_position()) {
            return false;
        }
        match hive_at(ai.tick.world, hive) {
            Some(Some(count)) if count >= crate::region::hives::MAX_OCCUPANTS => {
                update(&mut ai.tick.mob, |own| own.hive = None);
                false
            }
            Some(Some(_)) => true,
            _ => false,
        }
    }

    fn can_bee_continue(&self, _: &mut (), _: &mut Ai<'_, '_>) -> bool {
        false
    }

    /// `hive.addOccupant(bee)`: the hive's to carry out.
    fn start(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        if let Some(hive) = state_of(&ai.tick.mob).hive {
            ai.tick.out.local.push(Local::EnterHive { hive });
        }
    }
}

/// `ValidateHiveGoal`'s and `ValidateFlowerGoal`'s last validation, in game time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Validated {
    pub(crate) last: i64,
}

impl Default for Validated {
    fn default() -> Self {
        Self { last: -1 }
    }
}

/// `ValidateHiveGoal`: every 20 to 40 ticks, a hive the region reads that is gone is
/// forgotten.
#[derive(Debug, Clone, Copy)]
struct ValidateHive;

impl BeeGoal for ValidateHive {
    type State = Validated;

    fn can_bee_use(&self, state: &mut Validated, ai: &mut Ai<'_, '_>) -> bool {
        let cooldown = i64::from(state_of(&ai.tick.mob).validate_hive_cooldown);
        game_time(ai.tick.world) > state.last + cooldown
    }

    fn can_bee_continue(&self, _: &mut Validated, _: &mut Ai<'_, '_>) -> bool {
        false
    }

    fn start(&self, state: &mut Validated, ai: &mut Ai<'_, '_>) {
        let mut own = state_of(&ai.tick.mob);
        if let Some(hive) = own.hive
            && ai.tick.world.is_loaded(hive)
            && !hive_valid(&own, ai.tick.world, ai.block_position())
        {
            own.drop_hive(ai.tick.lived);
            put_state(&mut ai.tick.mob, &own);
        }
        state.last = game_time(ai.tick.world);
    }
}

/// `ValidateFlowerGoal`: likewise, a flower that is no longer one is forgotten.
#[derive(Debug, Clone, Copy)]
struct ValidateFlower;

impl BeeGoal for ValidateFlower {
    type State = Validated;

    fn can_bee_use(&self, state: &mut Validated, ai: &mut Ai<'_, '_>) -> bool {
        let cooldown = i64::from(state_of(&ai.tick.mob).validate_flower_cooldown);
        game_time(ai.tick.world) > state.last + cooldown
    }

    fn can_bee_continue(&self, _: &mut Validated, _: &mut Ai<'_, '_>) -> bool {
        false
    }

    fn start(&self, state: &mut Validated, ai: &mut Ai<'_, '_>) {
        let mut own = state_of(&ai.tick.mob);
        if let Some(flower) = own.flower
            && ai.tick.world.is_loaded(flower)
            && !attracts_bees(ai.tick.world.state(flower))
        {
            own.drop_flower(ai.tick.draws, ai.tick.lived);
            put_state(&mut ai.tick.mob, &own);
        }
        state.last = game_time(ai.tick.world);
    }
}

/// `BeePollinateGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Pollinating {
    pub(crate) successful_ticks: i32,
    pub(crate) last_sound_tick: i32,
    pub(crate) hover: Option<Vec3>,
    pub(crate) ticks: i32,
    /// `unreachableFlowerCache`: flowers it could not reach, each until a game time.
    pub(crate) unreachable: Vec<(BlockPos, i64)>,
}

/// `BeePollinateGoal`: a bee without nectar goes to the nearest flower it can reach
/// within five blocks and hovers over it; after 400 ticks within a block of it, it may
/// leave with nectar.
#[derive(Debug, Clone, Copy)]
struct Pollinate;

impl Pollinate {
    /// `findNearbyFlower()`: the first attractive flower by Manhattan distance that a
    /// path reaches, those it could not reach within 600 ticks skipped; what it met of
    /// those before the one found is all it remembers of them.
    fn find_flower(state: &mut Pollinating, ai: &mut Ai<'_, '_>) -> Option<BlockPos> {
        let now = game_time(ai.tick.world);
        let mut next = Vec::new();
        let mut found = None;
        for pos in manhattan_ordered(ai.block_position(), 5, 5, 5) {
            if !attracts_bees(ai.tick.world.state(pos)) {
                continue;
            }
            let until = state
                .unreachable
                .iter()
                .find(|(at, _)| *at == pos)
                .map_or(i64::MIN, |(_, until)| *until);
            if now < until {
                next.push((pos, until));
                continue;
            }
            let reached = navigation::create_path(ai, pos, 1)
                .is_some_and(|made| made.path(ai).is_some_and(|path| path.reached));
            if reached {
                found = Some(pos);
                break;
            }
            next.push((pos, now + 600));
        }
        state.unreachable = next;
        found
    }

    /// `setWantedPos()`: its move control to the hover point at 0.35.
    fn set_wanted_pos(state: &Pollinating, ai: &mut Ai<'_, '_>) {
        if let Some(hover) = state.hover {
            controls::set_wanted_position(ai, hover, 0.349_999_994_039_535_5);
        }
    }

    /// `getOffset()`.
    fn offset(ai: &mut Ai<'_, '_>) -> f64 {
        f64::from((ai.tick.draws.next_float() * 2.0 - 1.0) * 0.333_333_34)
    }
}

impl BeeGoal for Pollinate {
    type State = Pollinating;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_bee_use(&self, state: &mut Pollinating, ai: &mut Ai<'_, '_>) -> bool {
        let own = state_of(&ai.tick.mob);
        let now = ai.tick.lived;
        if own.flower_cooldown(now) > 0 || own.has(flag_bits::HAS_NECTAR) {
            return false;
        }
        // No rain falls.
        if let Some(flower) = Self::find_flower(state, ai) {
            update(&mut ai.tick.mob, |own| own.flower = Some(flower));
            navigation::move_to(ai, centre_of(flower), 1.2);
            return true;
        }
        let cooldown = ai.tick.draws.next_int_between(20, 60);
        update(&mut ai.tick.mob, |own| {
            own.set_flower_cooldown(now, cooldown)
        });
        false
    }

    fn can_bee_continue(&self, state: &mut Pollinating, ai: &mut Ai<'_, '_>) -> bool {
        let own = state_of(&ai.tick.mob);
        if !own.pollinating || own.flower.is_none() {
            return false;
        }
        if state.successful_ticks > 400 {
            return ai.tick.draws.next_float() < 0.2;
        }
        true
    }

    fn start(&self, state: &mut Pollinating, ai: &mut Ai<'_, '_>) {
        state.successful_ticks = 0;
        state.ticks = 0;
        state.last_sound_tick = 0;
        let now = ai.tick.lived;
        update(&mut ai.tick.mob, |own| {
            own.pollinating = true;
            own.reset_ticks_without_nectar(now);
        });
    }

    fn stop(&self, state: &mut Pollinating, ai: &mut Ai<'_, '_>) {
        let long_enough = state.successful_ticks > 400;
        let now = ai.tick.lived;
        update(&mut ai.tick.mob, |own| {
            if long_enough {
                own.set_has_nectar(true, now);
            }
            own.pollinating = false;
            own.flower_cooldown_until = now + LOCATE_COOLDOWN;
        });
        navigation::stop(ai);
    }

    fn tick(&self, state: &mut Pollinating, ai: &mut Ai<'_, '_>) {
        let Some(flower) = state_of(&ai.tick.mob).flower else {
            return;
        };
        state.ticks += 1;
        if state.ticks > 600 {
            let now = ai.tick.lived;
            let mut own = state_of(&ai.tick.mob);
            own.drop_flower(ai.tick.draws, now);
            own.pollinating = false;
            own.flower_cooldown_until = now + LOCATE_COOLDOWN;
            put_state(&mut ai.tick.mob, &own);
            return;
        }
        let bottom = bottom_centre_of(flower);
        let above = [bottom[0], bottom[1] + 0.600_000_023_841_857_9, bottom[2]];
        let position = ai.position();
        if physics::distance_sqr(above, position).sqrt() > 1.0 {
            state.hover = Some(above);
            Self::set_wanted_pos(state, ai);
            return;
        }
        let hover = *state.hover.get_or_insert(above);
        let arrived = physics::distance_sqr(position, hover).sqrt() <= 0.1;
        let mut moves = true;
        if arrived {
            if ai.tick.draws.next_int(25) == 0 {
                let dx = Self::offset(ai);
                let dz = Self::offset(ai);
                state.hover = Some([above[0] + dx, above[1], above[2] + dz]);
                navigation::stop(ai);
            } else {
                moves = false;
            }
            controls::set_look_at_default(ai, above);
        }
        if moves {
            Self::set_wanted_pos(state, ai);
        }
        state.successful_ticks += 1;
        if ai.tick.draws.next_float() < 0.05 && state.successful_ticks > state.last_sound_tick + 60
        {
            state.last_sound_tick = state.successful_ticks;
            ai.tick.out.events.push(Event::Sound {
                sound: sounds::ENTITY_BEE_POLLINATE,
                volume: 1.0,
                pitch: 1.0,
            });
        }
    }
}

/// `BeeLocateHiveGoal`: a bee that wants to go in and has no hive looks for the nearest
/// with room within 20 blocks, every ten seconds, passing those it gave up on unless
/// they are all it finds.
#[derive(Debug, Clone, Copy)]
struct LocateHive;

/// `findNearbyHivesWithSpace()`: the hives within 20 blocks (by the low corners) with
/// room, nearest first (ties in the order of chunk, section, height, z and x).
pub(crate) fn hives_with_space(world: &dyn MobWorld, at: BlockPos) -> Vec<BlockPos> {
    let centre = ChunkPos::containing(f64::from(at.x), f64::from(at.z));
    let mut found = Vec::new();
    for cz in centre.z - 2..=centre.z + 2 {
        for cx in centre.x - 2..=centre.x + 2 {
            let (low_x, low_z) = (cx * 16, cz * 16);
            let xs = (low_x.max(at.x - 20))..=((low_x + 15).min(at.x + 20));
            let zs = (low_z.max(at.z - 20))..=((low_z + 15).min(at.z + 20));
            for y in at.y - 20..=at.y + 20 {
                for z in zs.clone() {
                    for x in xs.clone() {
                        let pos = BlockPos::new(x, y, z);
                        if dist_sqr(pos, at) > 400.0 || !world.is_loaded(pos) {
                            continue;
                        }
                        if !is_hive(world.state(pos)) {
                            continue;
                        }
                        let room = world
                            .block_entity_data(pos)
                            .map_or(0, crate::region::hives::occupant_count)
                            < crate::region::hives::MAX_OCCUPANTS;
                        if room {
                            found.push(pos);
                        }
                    }
                }
            }
        }
    }
    found.sort_by(|a, b| dist_sqr(*a, at).total_cmp(&dist_sqr(*b, at)));
    found
}

impl BeeGoal for LocateHive {
    type State = ();

    fn can_bee_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        let own = state_of(&ai.tick.mob);
        own.hive_cooldown(ai.tick.lived) == 0 && own.hive.is_none() && wants_to_enter_hive(ai)
    }

    fn can_bee_continue(&self, _: &mut (), _: &mut Ai<'_, '_>) -> bool {
        false
    }

    fn start(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        let mut own = state_of(&ai.tick.mob);
        own.hive_cooldown_until = ai.tick.lived + LOCATE_COOLDOWN;
        let found = hives_with_space(ai.tick.world, ai.block_position());
        if let Some(first) = found.first().copied() {
            match found.iter().find(|pos| !own.blacklist.contains(pos)) {
                Some(pos) => own.hive = Some(*pos),
                None => {
                    own.blacklist.clear();
                    own.hive = Some(first);
                }
            }
        }
        put_state(&mut ai.tick.mob, &own);
    }
}

/// `BeeGoToHiveGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct GoingHome {
    pub(crate) travelling_ticks: i32,
    pub(crate) last_path: Option<Vec<[i32; 3]>>,
    pub(crate) ticks_stuck: i32,
}

/// `BeeGoToHiveGoal`: a bee that wants to go in flies to its hive: in the air towards it
/// from afar, by a path within 16 blocks; one it cannot reach, or after two minutes,
/// it gives up on.
#[derive(Debug, Clone, Copy)]
struct GoToHive;

impl GoToHive {
    /// `hasReachedTarget(pos)`: within two blocks, or at the end of a path that reaches
    /// it.
    fn reached(ai: &Ai<'_, '_>, hive: BlockPos) -> bool {
        if closer_than(hive, ai.block_position(), 2) {
            return true;
        }
        let path = ai.tick.mob.body.path.followed.as_ref();
        path.is_some_and(|path| {
            path.target == hive
                && path.reached
                && ai.tick.mob.body.progress.next as usize >= path.nodes.len()
        })
    }

    /// `pathfindDirectlyTowards(pos)`: a path to it, to within one block from three
    /// away, else two; whether it reaches.
    fn pathfind_directly(ai: &mut Ai<'_, '_>, hive: BlockPos) -> bool {
        let reach = if closer_than(hive, ai.block_position(), 3) {
            1
        } else {
            2
        };
        let at = [f64::from(hive.x), f64::from(hive.y), f64::from(hive.z)];
        navigation::move_to_reach(ai, at, reach, 1.0);
        ai.tick
            .mob
            .body
            .path
            .followed
            .as_ref()
            .is_some_and(|path| path.reached)
    }

    fn nodes(ai: &Ai<'_, '_>) -> Option<Vec<[i32; 3]>> {
        ai.tick.mob.body.path.followed.as_ref().map(|path| {
            path.nodes
                .iter()
                .map(|node| [node.x, node.y, node.z])
                .collect()
        })
    }
}

impl BeeGoal for GoToHive {
    type State = GoingHome;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_bee_use(&self, _: &mut GoingHome, ai: &mut Ai<'_, '_>) -> bool {
        let Some(hive) = state_of(&ai.tick.mob).hive else {
            return false;
        };
        closer_than(hive, ai.block_position(), TOO_FAR)
            && ai.home().is_none()
            && wants_to_enter_hive(ai)
            && !Self::reached(ai, hive)
            && is_hive(ai.tick.world.state(hive))
    }

    fn can_bee_continue(&self, state: &mut GoingHome, ai: &mut Ai<'_, '_>) -> bool {
        self.can_bee_use(state, ai)
    }

    fn start(&self, state: &mut GoingHome, _: &mut Ai<'_, '_>) {
        state.travelling_ticks = 0;
        state.ticks_stuck = 0;
    }

    fn stop(&self, state: &mut GoingHome, ai: &mut Ai<'_, '_>) {
        state.travelling_ticks = 0;
        state.ticks_stuck = 0;
        navigation::stop(ai);
    }

    fn tick(&self, state: &mut GoingHome, ai: &mut Ai<'_, '_>) {
        let Some(hive) = state_of(&ai.tick.mob).hive else {
            return;
        };
        state.travelling_ticks += 1;
        if state.travelling_ticks > crate::mobs::ai::maths::reduced_tick_delay(2400) {
            let now = ai.tick.lived;
            update(&mut ai.tick.mob, |own| {
                own.blacklist(hive);
                own.drop_hive(now);
            });
            return;
        }
        if navigation::in_progress(ai) {
            return;
        }
        if closer_than(hive, ai.block_position(), 16) {
            let now = ai.tick.lived;
            if !Self::pathfind_directly(ai, hive) {
                update(&mut ai.tick.mob, |own| {
                    own.blacklist(hive);
                    own.drop_hive(now);
                });
            } else {
                let nodes = Self::nodes(ai);
                if state.last_path.is_some() && state.last_path == nodes {
                    state.ticks_stuck += 1;
                    if state.ticks_stuck > 60 {
                        update(&mut ai.tick.mob, |own| own.drop_hive(now));
                        state.ticks_stuck = 0;
                    }
                } else {
                    state.last_path = nodes;
                }
            }
            return;
        }
        if !closer_than(hive, ai.block_position(), TOO_FAR) {
            let now = ai.tick.lived;
            update(&mut ai.tick.mob, |own| own.drop_hive(now));
            return;
        }
        pathfind_randomly_towards(ai, hive);
    }
}

/// `BeeGoToKnownFlowerGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct GoingToFlower {
    pub(crate) travelling_ticks: i32,
}

/// `BeeGoToKnownFlowerGoal`: a bee half a minute out without nectar flies back to the
/// flower it knows.
#[derive(Debug, Clone, Copy)]
struct GoToKnownFlower;

impl BeeGoal for GoToKnownFlower {
    type State = GoingToFlower;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_bee_use(&self, _: &mut GoingToFlower, ai: &mut Ai<'_, '_>) -> bool {
        let own = state_of(&ai.tick.mob);
        let Some(flower) = own.flower else {
            return false;
        };
        ai.home().is_none()
            && own.ticks_without_nectar(ai.tick.lived) > 600
            && !closer_than(flower, ai.block_position(), 2)
    }

    fn can_bee_continue(&self, state: &mut GoingToFlower, ai: &mut Ai<'_, '_>) -> bool {
        self.can_bee_use(state, ai)
    }

    fn start(&self, state: &mut GoingToFlower, _: &mut Ai<'_, '_>) {
        state.travelling_ticks = 0;
    }

    fn stop(&self, state: &mut GoingToFlower, ai: &mut Ai<'_, '_>) {
        state.travelling_ticks = 0;
        navigation::stop(ai);
    }

    fn tick(&self, state: &mut GoingToFlower, ai: &mut Ai<'_, '_>) {
        let Some(flower) = state_of(&ai.tick.mob).flower else {
            return;
        };
        state.travelling_ticks += 1;
        if state.travelling_ticks > crate::mobs::ai::maths::reduced_tick_delay(2400) {
            let mut own = state_of(&ai.tick.mob);
            own.drop_flower(ai.tick.draws, ai.tick.lived);
            put_state(&mut ai.tick.mob, &own);
            return;
        }
        if navigation::in_progress(ai) {
            return;
        }
        if !closer_than(flower, ai.block_position(), TOO_FAR) {
            let mut own = state_of(&ai.tick.mob);
            own.drop_flower(ai.tick.draws, ai.tick.lived);
            put_state(&mut ai.tick.mob, &own);
            return;
        }
        pathfind_randomly_towards(ai, flower);
    }
}

/// What a bee's nectar makes of the crop `state` (`BeeGrowCropGoal`'s), if anything.
pub(crate) fn grown(state: BlockState) -> Option<BlockState> {
    if !in_tag(state, "minecraft:bee_growables") {
        return None;
    }
    let age = number(state, "age");
    if is_class(state, BlockClass::TorchflowerCropBlock) {
        // **Inferred**: its last age is the torchflower itself (its class is not in the
        // dump).
        let age = age?;
        return Some(if age + 1 >= 2 {
            blocks::TORCHFLOWER
        } else {
            set(state, "age", &(age + 1).to_string())
        });
    }
    let crop = is_class(state, BlockClass::CropBlock)
        || is_class(state, BlockClass::BeetrootBlock)
        || is_class(state, BlockClass::CarrotBlock)
        || is_class(state, BlockClass::PotatoBlock);
    if crop {
        let max = if is_class(state, BlockClass::BeetrootBlock) {
            3
        } else {
            7
        };
        let age = age?;
        return (age < max).then(|| set(state, "age", &(age + 1).to_string()));
    }
    if is_class(state, BlockClass::StemBlock) {
        let age = age?;
        return (age < 7).then(|| set(state, "age", &(age + 1).to_string()));
    }
    if is_block(state, blocks::SWEET_BERRY_BUSH) {
        let age = age?;
        return (age < 3).then(|| set(state, "age", &(age + 1).to_string()));
    }
    // Cave vines: bone meal gives them berries where they have none (**Inferred**).
    let vines = is_class(state, BlockClass::CaveVinesBlock)
        || is_class(state, BlockClass::CaveVinesPlantBlock);
    if vines && !flag(state, "berries") {
        return Some(set_flag(state, "berries", true));
    }
    None
}

/// `BeeGrowCropGoal`: a bee with nectar and a hive grows the crops one and two blocks
/// below it now and then, ten at most a trip.
#[derive(Debug, Clone, Copy)]
struct GrowCrop;

impl BeeGoal for GrowCrop {
    type State = ();

    fn can_bee_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        let own = state_of(&ai.tick.mob);
        if own.crops_grown >= MAX_CROPS {
            return false;
        }
        if ai.tick.draws.next_float() < 0.3 {
            return false;
        }
        own.has(flag_bits::HAS_NECTAR) && hive_valid(&own, ai.tick.world, ai.block_position())
    }

    fn can_bee_continue(&self, state: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        self.can_bee_use(state, ai)
    }

    fn tick(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        if ai.tick.draws.next_int(15) != 0 {
            return;
        }
        let at = ai.block_position();
        for below in 1..=2 {
            let pos = BlockPos::new(at.x, at.y - below, at.z);
            let Some(state) = grown(ai.tick.world.state(pos)) else {
                continue;
            };
            ai.tick.out.local.push(Local::LevelEvent {
                kind: 2011,
                pos,
                data: 15,
            });
            ai.tick.out.local.push(Local::SetBlock {
                pos,
                state,
                flags: 3,
            });
            update(&mut ai.tick.mob, |own| own.crops_grown += 1);
        }
    }
}

/// `BeeWanderGoal`: an idle bee, one time in ten, flies to a place in the air ahead of
/// it, or towards its hive when it is 24 blocks or more from it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BeeWanderGoal;

impl BeeWanderGoal {
    fn find_pos(ai: &mut Ai<'_, '_>) -> Option<Vec3> {
        let own = state_of(&ai.tick.mob);
        let at = ai.block_position();
        let towards_hive = own
            .hive
            .filter(|hive| hive_valid(&own, ai.tick.world, at) && !closer_than(*hive, at, 24));
        let direction = match towards_hive {
            Some(hive) => physics::normalize(physics::sub(centre_of(hive), ai.position())),
            None => view_vector(ai.tick.pitch, ai.tick.yaw),
        };
        random_pos::hover_pos(ai, 8, 7, direction[0], direction[2], 1.570_796_4, 3, 1).or_else(
            || {
                random_pos::air_and_water_pos(
                    ai,
                    8,
                    4,
                    -2,
                    direction[0],
                    direction[2],
                    1.570_796_370_506_286_6,
                )
            },
        )
    }
}

impl Goal for BeeWanderGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        navigation::is_done(ai) && ai.tick.draws.next_int(10) == 0
    }

    fn can_continue(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        navigation::in_progress(ai)
    }

    fn start(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        if let Some(wanted) = Self::find_pos(ai) {
            let made = navigation::create_path(ai, physics::containing(wanted), 1);
            navigation::move_to_path(ai, made, 1.0);
        }
    }
}

/// `BeeHurtByOtherGoal`: who hurt it, while it is angry (its alert of other bees is
/// named).
#[derive(Debug, Clone, Copy)]
pub(crate) struct BeeHurtByOtherGoal;

impl BeeHurtByOtherGoal {
    const HURT_BY: HurtByTargetGoal = HurtByTargetGoal { ignore: &[] };
}

impl Goal for BeeHurtByOtherGoal {
    type State = Targeting;

    fn flags(&self) -> u8 {
        Self::HURT_BY.flags()
    }

    fn can_use(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        Self::HURT_BY.can_use(state, ai)
    }

    fn can_continue(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        angry(ai) && Self::HURT_BY.can_continue(state, ai)
    }

    fn start(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        Self::HURT_BY.start(state, ai);
    }

    fn stop(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        Self::HURT_BY.stop(state, ai);
    }
}

/// `BeeBecomeAngryTargetGoal`: an angry bee that has not stung takes as target the
/// player it is angry at; no player can be one, so it only draws.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BeeBecomeAngryTargetGoal;

impl BeeBecomeAngryTargetGoal {
    fn nearest() -> NearestAttackableTargetGoal {
        NearestAttackableTargetGoal::new(Class::Player, true)
    }

    fn may(ai: &Ai<'_, '_>) -> bool {
        angry(ai) && !state_of(&ai.tick.mob).has(flag_bits::HAS_STUNG)
    }
}

impl Goal for BeeBecomeAngryTargetGoal {
    type State = Targeting;

    fn flags(&self) -> u8 {
        flags::TARGET
    }

    fn can_use(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        Self::may(ai) && Self::nearest().can_use(state, ai)
    }

    fn can_continue(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        if !Self::may(ai) || ai.tick.mob.body.combat.target.is_none() {
            state.target_mob = None;
            return false;
        }
        Self::nearest().can_continue(state, ai)
    }

    fn start(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        Self::nearest().start(state, ai);
    }

    fn stop(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        Self::nearest().stop(state, ai);
    }
}

/// `TimeUtil.rangeOfSeconds(20, 39).sample(random)`: anger of 400 to 780 ticks, one draw.
fn anger_time(draws: &mut Random) -> i64 {
    400 + i64::from(draws.next_int(381))
}

impl MobType for Bee {
    fn register_goals(&self, setup: &mut Setup) {
        cow::animal_maluses(setup);
        setup.set_malus(PathType::Fire, -1.0);
        setup.set_malus(PathType::Water, -1.0);
        setup.set_malus(PathType::WaterBorder, 16.0);
        setup.set_malus(PathType::Cocoa, -1.0);
        setup.set_malus(PathType::Fence, -1.0);
        // `Bee$1`: a flying navigation that opens no doors, floats not (until its float
        // goal), plans 48 blocks, and stands on anything.
        let mut settings = Settings::of(Kind::Flying);
        settings.flags.can_open_doors = false;
        settings.set_can_float(false);
        settings.set_required_path_length(48.0);
        setup.config.navigation = settings;
        setup.config.move_kind = MoveKind::Flying {
            max_turn: 20,
            hovers: true,
        };
        setup.add_goal(0, BeeAttackGoal);
        setup.add_goal(1, Base(EnterHive));
        setup.add_goal(2, animal::BreedGoal { speed: 1.0 });
        setup.add_goal(3, TemptGoal::new(1.25, FOOD, false));
        setup.add_goal(3, Base(ValidateHive));
        setup.add_goal(3, Base(ValidateFlower));
        setup.add_goal(4, Base(Pollinate));
        setup.add_goal(5, animal::FollowParentGoal { speed: 1.25 });
        setup.add_goal(5, Base(LocateHive));
        setup.add_goal(5, Base(GoToHive));
        setup.add_goal(6, Base(GoToKnownFlower));
        setup.add_goal(7, Base(GrowCrop));
        setup.add_goal(8, BeeWanderGoal);
        setup.add_goal(9, FloatGoal);
        setup.add_target(1, BeeHurtByOtherGoal);
        setup.add_target(2, BeeBecomeAngryTargetGoal);
        // `ResetUniversalAngerTargetGoal(true)`: only with `universal_anger`, which is off.
        setup.add_target(3, Idle { flags: 0 });
    }

    /// `Bee$1.isStableDestination(pos)`: anything but air below.
    fn stable_destination(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> Option<bool> {
        let below = BlockPos::new(pos.x, pos.y - 1, pos.z);
        Some(!ai.tick.world.state(below).props().is_air())
    }

    /// `getWalkTargetValue`: ten for air, nothing for the rest.
    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        if ai.tick.world.state(pos).props().is_air() {
            10.0
        } else {
            0.0
        }
    }

    /// The constructor: its validating goals' cooldowns (20 to 40), then its flower's
    /// (20 to 60), each of its own stream.
    fn construct(&self, mob: &mut Mob, own: &mut Random) {
        let hive = own.next_int_between(20, 40);
        let flower = own.next_int_between(20, 40);
        let cooldown = own.next_int_between(20, 60);
        update(mob, |state| {
            state.validate_hive_cooldown = hive;
            state.validate_flower_cooldown = flower;
            state.set_flower_cooldown(0, cooldown);
        });
    }

    /// `Bee$1.tick`: a pollinating bee's navigation does nothing.
    fn navigation_ticks(&self, mob: &Mob) -> bool {
        !state_of(mob).pollinating
    }

    /// `Bee.customServerAiStep` (it does not call `Animal`'s): drowning after a second
    /// under water, death by its sting's draw, time without nectar, and its anger.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        let mut own = state_of(&tick.mob);
        let stung = own.has(flag_bits::HAS_STUNG);
        own.under_water_ticks = if tick.living.fluids.in_water() {
            own.under_water_ticks + 1
        } else {
            0
        };
        put_state(&mut tick.mob, &own);
        if own.under_water_ticks > 20 {
            body::hurt_server(tick, crate::living::Damage::Drown, 1.0);
        }
        let mut own = state_of(&tick.mob);
        if stung {
            own.time_since_sting += 1;
            if own.time_since_sting % 5 == 0 {
                let bound = (1200 - own.time_since_sting).clamp(1, 1200) as u32;
                if tick.draws.next_int(bound) == 0 {
                    put_state(&mut tick.mob, &own);
                    let health = tick.mob.body.vitals.health;
                    body::hurt_by(
                        tick,
                        &DamageSource::of(DamageType("minecraft:generic")),
                        health,
                    );
                    own = state_of(&tick.mob);
                }
            }
        }
        let now = tick.world.game_time();
        neutral::update_persistent_anger(
            &mut tick.mob,
            &mut own.anger,
            tick.world,
            now,
            false,
            tick.draws,
            anger_time,
        );
        put_state(&mut tick.mob, &own);
    }

    /// `FlyingMoveControl(bee, 20, true)`: from its first move on, gravity leaves it.
    fn tick_move_control(&self, ai: &mut Ai<'_, '_>) {
        if ai.tick.mob.body.controls.movement.operation == Operation::MoveTo {
            update(&mut ai.tick.mob, |own| own.no_gravity = true);
            // `travel` comes after the controls: this tick's fall is the first it is spared.
            ai.tick.living.attributes.gravity = 0.0;
        }
        controls::tick_move(ai);
    }

    /// `BeeLookControl`: an angry bee turns nothing; a pollinating one keeps its pitch.
    fn tick_look_control(&self, ai: &mut Ai<'_, '_>) {
        if angry(ai) {
            return;
        }
        let pollinating = state_of(&ai.tick.mob).pollinating;
        controls::tick_plain_look_resetting(ai, !pollinating);
    }

    /// `Animal.aiStep`, then the bee's: its cooldowns down, its roll as it closes on what
    /// it is angry at, a hive gone forgotten every second; then `Bee.tick`'s nectar
    /// drips, drawn and not sent.
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
        let mut own = state_of(&tick.mob);
        // Its countdowns count themselves down: each is the moment it ends.
        let now = game_time(tick.world);
        let position = tick.living.body.position;
        let close = tick
            .mob
            .body
            .combat
            .target
            .and_then(|key| super::tamable::find(tick.world, key))
            .filter(sensing::valid_target)
            .is_some_and(|target| target.distance_sqr(position) < 4.0);
        let rolling = own.anger.is_angry(now) && !own.has(flag_bits::HAS_STUNG) && close;
        own.set(flag_bits::ROLL, rolling);
        let at = physics::containing(position);
        let tick_count = tick.tick_count.saturating_add(1);
        if tick_count.is_multiple_of(20) && !hive_valid(&own, tick.world, at) {
            own.hive = None;
        }
        if own.has(flag_bits::HAS_NECTAR)
            && own.crops_grown < MAX_CROPS
            && tick.draws.next_float() < 0.05
        {
            let drips = tick.draws.next_int(2) + 1;
            for _ in 0..drips * 2 {
                tick.draws.next_double();
            }
        }
        put_state(&mut tick.mob, &own);
    }

    /// `Bee.hurtServer`: a hurt it is not spared stops its pollinating.
    fn before_hurt(&self, mob: &mut Mob) {
        update(mob, |own| own.pollinating = false);
    }

    /// `Bee.mobInteract`: a flower with an effect for bees is taken and gives it
    /// (named: no effect is applied); any other is an animal's food.
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        let effect = used.held.is_some_and(|held| {
            held.count > 0 && matches!(held.item, items::WITHER_ROSE | items::OPEN_EYEBLOSSOM)
        });
        if effect {
            return Interaction::Success;
        }
        animal::mob_interact(used, FOOD, None)
    }

    fn breed_offspring(&self, _: &mut Breeding<'_>) -> Option<Mob> {
        Some(Mob::fresh())
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        animal::dimensions(mob, BABY)
    }

    fn no_gravity(&self, mob: &Mob) -> bool {
        state_of(mob).no_gravity
    }

    fn omnidirectional_air(&self) -> bool {
        true
    }

    /// No ambient sound: the buzz is the client's own.
    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        None
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_BEE_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_BEE_DEATH)
    }

    fn sound_volume(&self) -> f32 {
        0.4
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let own = state_of(mob);
        let mut values = animal::shown(mob);
        values.push((
            NO_GRAVITY_INDEX,
            DataShown::Boolean(own.no_gravity),
            !own.no_gravity,
        ));
        values.push((FLAGS_INDEX, DataShown::Byte(own.flags), own.flags == 0));
        values.push((
            ANGER_INDEX,
            DataShown::Long(own.anger.end),
            own.anger.end == -1,
        ));
        values
    }
}
