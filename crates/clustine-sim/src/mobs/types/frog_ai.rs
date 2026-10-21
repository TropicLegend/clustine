//! The frog's brain (`FrogAi`, `Croak`, `ShootTongue`, `TryLaySpawnOnFluidNearLand`,
//! `LongJumpToPreferredBlock` and `FrogAttackablesSensor`; `docs/groundwork/n7-brief.md`,
//! sections 7.5.4 to 7.5.7): in its core it panics, looks, walks and lets its temptation
//! and its long jump cool; idle on land it looks at players, breeds, follows slime balls,
//! goes to land, strolls and croaks; in water it swims and goes to land; pregnant it
//! goes to land beside water and lays frogspawn; it long-jumps up to four blocks across
//! and two up, liking lily pads and big dripleaf; and it would eat a small slime or
//! magma cube with its tongue.
//!
//! What reads across a line (`docs/groundwork/n7j-plan.md`, section 3): its sensors see
//! the region's land, copies and faces (item 1); a partner, a tempting player or a look
//! target only a copy shows is followed as the shared behaviours do (item 2); breeding
//! across a line is the region's protocol, which makes the frog the region breeds in
//! pregnant (item 4); frogspawn laid a block over a line is the region's `SetIf` block
//! effect (item 7); the long jump's candidates and the searches for land read blocks
//! beyond the copies as unloaded (item 8), and a jump that lands over a line carries the
//! frog over with its brain (item 10). "Unknown" is never dead (the plan's section 8 item
//! 8): a tongue's prey the view has not is kept and waited on, as the shared attackers
//! keep theirs.
//!
//! Named differences:
//!
//! - **The tongue** (`ShootTongue`) has no prey until slimes and magma cubes are built
//!   (N7k, N9), and so never starts. Built here as far as the contract reaches: its prey's
//!   pull towards the frog (`setDeltaMovement`) and its removal as soon as it is dead are
//!   not made (no local write sets another mob's motion or removes it), its hit is
//!   [`Local::Hurt`], its tongue target is kept in the frog's state and not shown (the
//!   contract has no synced value of an optional entity), and a slime's size is not read
//!   (`Frog.canEat` takes every one of `#frog_food`). Its counters are kept as values
//!   written each tick it runs, for at most 100 ticks.
//! - `unreachable_tongue_targets` keeps the targets' entity keys where the game keeps
//!   their UUIDs: the contract's values have no list of UUIDs.
//! - Its pregnancy (`IS_PREGNANT`) comes from a breeding by the frog's type state, which
//!   is all the breeding hook gives back, and is taken into its brain at its next brain
//!   tick, a tick after the game's.
//! - A long jump's discarded friction (`setDiscardFriction`), which the living body has
//!   no switch for, is the frog's own: its travel in the air takes friction and air drag
//!   modified to nothing while it is set ([`super::frog::Frog`]'s `before_travel`).
//! - Its navigation is the amphibious one: the frog's own node evaluator (its start in
//!   water, lily pads and big dripleaf open, no corner cut by a water border) is not in
//!   the contract's navigation.

use clustine_blocks::data::{entity_in_tag, fluid_in_tag, in_tag};
use clustine_data::{BlockState, PathType, blocks, entity_types, sounds};
use clustine_world::{BlockPos, EntityId};
use serde::{Deserialize, Serialize};

use super::axolotl_ai::{breeding, in_water, on_ground, stand_still};
use super::frog::{self, Pose};
use crate::mobs::Mob;
use crate::mobs::ai::navigation;
use crate::mobs::ai::pathfinder;
use crate::mobs::ai::sensing::Looked;
use crate::mobs::body::Event;
use crate::mobs::brain::behaviours::attacking::{StartAttacking, stop_attacking};
use crate::mobs::brain::behaviours::cooldowns::count_down;
use crate::mobs::brain::behaviours::jumping::{
    Leaper, LongJumpMidJump, LongJumpToRandomPos, Preferred, default_landing,
};
use crate::mobs::brain::behaviours::liquids::{find_land, find_land_near_liquid};
use crate::mobs::brain::behaviours::looking::{LookAt, look_sometimes};
use crate::mobs::brain::behaviours::panic::panic;
use crate::mobs::brain::behaviours::social::{AnimalMakeLove, FollowTemptation};
use crate::mobs::brain::behaviours::walking::{
    StrollTo, look_sink, sink, stroll, stroll_to, walk_to_look_target,
};
use crate::mobs::brain::behaviours::{Speed, centre, entity_of, tracking, walk_to_point};
use crate::mobs::brain::sensors::{
    self, FROG_TEMPTATIONS, HURT_BY, IS_IN_WATER, NEAREST_LIVING_ENTITIES, closer_than, find,
    find_closest_visible,
};
use crate::mobs::brain::{
    Activity, ActivityData, Behaviour, Brain, Cx, Gate, HeldShot, HeldTimed, MemoryKind, Order,
    Provider, Running, Sensor, Shot, Status, Timed, Value,
};
use crate::mobs::tick::{Local, MobTick};
use crate::physics;

/// `BRAIN_PROVIDER`: its five sensors, and `FrogAi.getActivities`.
pub(crate) static PROVIDER: Provider = Provider {
    memories: &[],
    sensors: &[
        &NEAREST_LIVING_ENTITIES,
        &HURT_BY,
        &FROG_ATTACKABLES,
        &FROG_TEMPTATIONS,
        &IS_IN_WATER,
    ],
    activities,
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// `TIME_BETWEEN_LONG_JUMPS`: `UniformInt(100, 140)`.
pub(crate) const TIME_BETWEEN_LONG_JUMPS: (i32, i32) = (100, 140);

/// `#frog_prefer_jump_to`: lily pads and big dripleaf.
pub(crate) const PREFER_JUMP_TO: &str = "minecraft:frog_prefer_jump_to";

/// `Frog.canEat(entity)`: of `#frog_food` (slimes and magma cubes). **Named**: a cube's
/// size, which only one of 1 may be, is not read until slimes are built.
pub(crate) fn can_eat(looked: &Looked) -> bool {
    looked
        .kind
        .is_some_and(|kind| entity_in_tag(i32::from(kind), "minecraft:frog_food"))
}

/// The keys of `unreachable_tongue_targets`, none where it is not there.
fn unreachable(cx: &Cx<'_, '_, '_>) -> Vec<EntityId> {
    match cx.memories.get(MemoryKind::UnreachableTongueTargets) {
        Some(Value::Entities(keys)) => keys,
        _ => Vec::new(),
    }
}

/// `FrogAttackablesSensor` (`FROG_ATTACKABLES`): of the seen living things, the closest
/// it may attack and eat that its tongue has not found out of reach lately, within 10
/// blocks, as `nearest_attackable`.
pub(crate) struct FrogAttackablesSensor;

pub(crate) static FROG_ATTACKABLES: FrogAttackablesSensor = FrogAttackablesSensor;

impl Sensor for FrogAttackablesSensor {
    fn requires(&self) -> &'static [MemoryKind] {
        &[
            MemoryKind::NearestAttackable,
            MemoryKind::VisibleMobs,
            MemoryKind::UnreachableTongueTargets,
        ]
    }

    fn scan(&self, cx: &mut Cx<'_, '_, '_>) {
        if sensors::visible_mobs(cx.memories).is_none() {
            cx.memories.erase(MemoryKind::NearestAttackable);
            return;
        }
        let out_of_reach = unreachable(cx);
        let found = find_closest_visible(cx, &mut |cx, looked| {
            sensors::attackable(cx, looked)
                && can_eat(looked)
                && !out_of_reach.contains(&looked.key)
                && closer_than(cx.ai, looked, 10.0)
        });
        match found {
            Some(found) => cx
                .memories
                .set(MemoryKind::NearestAttackable, Value::Entity(found.key)),
            None => cx.memories.erase(MemoryKind::NearestAttackable),
        }
    }
}

/// The frog's pose, as its state keeps it.
fn pose(cx: &Cx<'_, '_, '_>) -> Pose {
    frog::read(&cx.ai.tick.mob).pose
}

fn set_pose(cx: &mut Cx<'_, '_, '_>, pose: Pose) {
    frog::update(&mut cx.ai.tick.mob, |frog| frog.pose = pose);
}

/// `Croak`'s `croakCounter`, which belongs to the behaviour and outlives its runs: its
/// value, and while it runs, the `lived` from which each tick has counted it up by one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Croaking {
    pub(crate) value: i32,
    pub(crate) since: Option<u32>,
}

impl Croaking {
    /// The counter at `now`, before that tick's count.
    pub(crate) fn at(self, now: u32) -> i32 {
        match self.since {
            None => self.value,
            Some(since) => {
                let counted = (i64::from(now) - i64::from(since)).max(0);
                (i64::from(self.value) + counted).min(i64::from(i32::MAX)) as i32
            }
        }
    }
}

/// `CROAK_TICKS`.
const CROAK_TICKS: i32 = 60;

/// `Croak`: with no walk target, a standing frog croaks for 60 ticks (`CROAKING`);
/// started in a liquid it takes no pose and keeps its counter, so that a frog that has
/// croaked before stops at once.
pub(crate) struct Croak;

impl Timed for Croak {
    type State = Croaking;

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[(MemoryKind::WalkTarget, Status::Absent)]
    }

    fn durations(&self) -> (i64, i64) {
        (100, 100)
    }

    fn extra_start(&self, _: &mut Croaking, cx: &mut Cx<'_, '_, '_>) -> bool {
        pose(cx) == Pose::Standing
    }

    fn can_still_use(&self, counter: &mut Croaking, cx: &mut Cx<'_, '_, '_>) -> bool {
        counter.at(cx.ai.tick.lived) < CROAK_TICKS
    }

    fn start(&self, counter: &mut Croaking, cx: &mut Cx<'_, '_, '_>) {
        let now = cx.ai.tick.lived;
        if cx.ai.in_liquid() {
            // Its counter goes on from where it was.
            counter.since = Some(now);
            return;
        }
        set_pose(cx, Pose::Croaking);
        *counter = Croaking {
            value: 0,
            since: Some(now),
        };
    }

    fn stop(&self, counter: &mut Croaking, cx: &mut Cx<'_, '_, '_>) {
        *counter = Croaking {
            value: counter.at(cx.ai.tick.lived),
            since: None,
        };
        set_pose(cx, Pose::Standing);
    }
}

/// The four horizontal directions in `Direction.Plane.HORIZONTAL`'s order: north, east,
/// south, west.
const HORIZONTAL: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];

/// Whether the face its block's collision shape shows upwards is empty
/// (`getCollisionShape(level, pos).getFaceShape(UP).isEmpty()`): no box crosses the plane
/// just under its top.
fn top_face_empty(state: BlockState) -> bool {
    const PLANE: f64 = 1.0 - 1.0e-7;
    !state
        .props()
        .collision_shape()
        .boxes()
        .any(|bounds| bounds[1] < PLANE && bounds[4] > PLANE)
}

/// `TryLaySpawnOnFluidNearLand.create(FROGSPAWN)`: a pregnant frog on its way somewhere,
/// in water or on the ground, lays frogspawn on the first water of the four blocks
/// beside the one under it that has air over it, and is no longer pregnant. It fires
/// whether or not it laid.
pub(crate) struct TryLaySpawnOnFluidNearLand;

impl Shot for TryLaySpawnOnFluidNearLand {
    fn required(&self) -> &[MemoryKind] {
        &[
            MemoryKind::AttackTarget,
            MemoryKind::WalkTarget,
            MemoryKind::IsPregnant,
        ]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx.memories.check(MemoryKind::AttackTarget, Status::Absent)
            || !cx.memories.check(MemoryKind::WalkTarget, Status::Present)
            || !cx.memories.check(MemoryKind::IsPregnant, Status::Present)
        {
            return false;
        }
        if !cx.ai.in_water() && !cx.ai.on_ground() {
            return false;
        }
        let at = cx.ai.block_position();
        let below = BlockPos::new(at.x, at.y - 1, at.z);
        let world = cx.ai.tick.world;
        for (dx, dz) in HORIZONTAL {
            let side = BlockPos::new(below.x + dx, below.y, below.z + dz);
            let state = world.state(side);
            if !top_face_empty(state) {
                continue;
            }
            if !fluid_in_tag(state, "minecraft:supports_frogspawn")
                && !in_tag(state, "minecraft:supports_frogspawn")
            {
                continue;
            }
            let above = BlockPos::new(side.x, side.y + 1, side.z);
            if world.state(above).is_air() {
                let position = cx.ai.position();
                let local = &mut cx.ai.tick.out.local;
                // `setBlockAndUpdate`: the region's own block, or a `SetIf` over a line.
                local.push(Local::SetBlock {
                    pos: above,
                    state: blocks::FROGSPAWN,
                    flags: 3,
                });
                local.push(Local::GameEventAt {
                    name: "minecraft:block_place",
                    at: centre(above),
                    affected: Some(blocks::FROGSPAWN),
                });
                local.push(Local::Sound {
                    sound: sounds::ENTITY_FROG_LAY_SPAWN,
                    source: crate::api::SOUND_SOURCE_BLOCKS,
                    at: position,
                    volume: 1.0,
                    pitch: 1.0,
                });
                cx.memories.erase(MemoryKind::IsPregnant);
                return true;
            }
        }
        true
    }
}

/// `ShootTongue`'s phases (`ShootTongue$State`), by their ordinals.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Phase {
    MoveToTarget,
    CatchAnimation,
    EatAnimation,
    #[default]
    Done,
}

/// What `ShootTongue` keeps: `eatAnimationTimer`, `calculatePathCounter` and its phase.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Tongue {
    pub(crate) eat_timer: i32,
    pub(crate) path_counter: i32,
    pub(crate) phase: Phase,
}

/// `ShootTongue(FROG_TONGUE, FROG_EAT)`: with an attack target it can reach within 1.75
/// blocks of its path's end, it walks to it, and within 1.75 shoots its tongue, catches
/// it for seven ticks, eats it (the hit) and finishes four ticks later; a target its path
/// does not reach is forgotten and kept out of reach for 100 ticks (see the module's
/// comment for what it leaves out).
pub(crate) struct ShootTongue;

/// `getDistToTarget() < 1.75f`, the tongue's reach.
const TONGUE_REACH: f32 = 1.75;

/// The attack target as the view has it.
fn attack_target(cx: &Cx<'_, '_, '_>) -> Option<Looked> {
    entity_of(cx, MemoryKind::AttackTarget).and_then(|key| find(cx.ai, key))
}

/// `addUnreachableTargetToMemory(frog, target)`: the target added to the five kept, the
/// oldest dropped, the whole list set again for 100 ticks.
fn add_unreachable(cx: &mut Cx<'_, '_, '_>, key: EntityId) {
    let mut list = unreachable(cx);
    let new = !list.contains(&key);
    if list.len() == 5 && new {
        list.remove(0);
    }
    if new {
        list.push(key);
    }
    cx.memories.set_with_expiry(
        MemoryKind::UnreachableTongueTargets,
        Value::Entities(list),
        100,
    );
}

/// `Entity.distanceTo(entity)`, in floats.
fn float_distance(a: physics::Vec3, b: physics::Vec3) -> f32 {
    let dx = (a[0] - b[0]) as f32;
    let dy = (a[1] - b[1]) as f32;
    let dz = (a[2] - b[2]) as f32;
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// The tongue's target as the frog's state keeps it (`setTongueTarget`).
fn set_tongue_target(cx: &mut Cx<'_, '_, '_>, target: Option<EntityId>) {
    frog::update(&mut cx.ai.tick.mob, |frog| frog.tongue_target = target);
}

impl ShootTongue {
    /// `eatEntity(level, frog)`: the eating sound, and the tongue's target, where it is
    /// there and alive, hit (`doHurtTarget`).
    fn eat(cx: &mut Cx<'_, '_, '_>) {
        cx.ai.tick.out.events.push(Event::Sound {
            sound: sounds::ENTITY_FROG_EAT,
            volume: 2.0,
            pitch: 1.0,
        });
        let Some(target) = frog::read(&cx.ai.tick.mob)
            .tongue_target
            .and_then(|key| find(cx.ai, key))
            .filter(|looked| looked.alive)
        else {
            return;
        };
        let amount = cx.ai.attribute(clustine_data::attributes::ATTACK_DAMAGE) as f32;
        cx.ai.tick.out.local.push(Local::Hurt {
            key: target.key,
            kind: cx.ai.tick.kind,
            from: cx.ai.position(),
            amount,
            sound: None,
            knockback: None,
        });
    }
}

impl Timed for ShootTongue {
    type State = Tongue;

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[
            (MemoryKind::WalkTarget, Status::Absent),
            (MemoryKind::LookTarget, Status::Registered),
            (MemoryKind::AttackTarget, Status::Present),
            (MemoryKind::IsPanicking, Status::Absent),
        ]
    }

    fn durations(&self) -> (i64, i64) {
        (100, 100)
    }

    fn extra_start(&self, _: &mut Tongue, cx: &mut Cx<'_, '_, '_>) -> bool {
        // A target the view has not is unknown: kept, and not tried this tick.
        let Some(target) = attack_target(cx) else {
            return false;
        };
        let reaches = navigation::create_path(cx.ai, target.block_position(), 0)
            .as_ref()
            .and_then(|made| made.path(cx.ai))
            .is_some_and(|path| path.dist_to_target < TONGUE_REACH);
        if !reaches {
            cx.memories.erase(MemoryKind::AttackTarget);
            add_unreachable(cx, target.key);
        }
        reaches && pose(cx) != Pose::Croaking && can_eat(&target)
    }

    fn can_still_use(&self, tongue: &mut Tongue, cx: &mut Cx<'_, '_, '_>) -> bool {
        cx.memories.check(MemoryKind::AttackTarget, Status::Present)
            && tongue.phase != Phase::Done
            && cx.memories.check(MemoryKind::IsPanicking, Status::Absent)
    }

    fn start(&self, tongue: &mut Tongue, cx: &mut Cx<'_, '_, '_>) {
        let Some(target) = attack_target(cx) else {
            return;
        };
        // `BehaviorUtils.lookAtEntity`.
        cx.memories.set(
            MemoryKind::LookTarget,
            Value::Tracker(tracking(target.key, true)),
        );
        set_tongue_target(cx, Some(target.key));
        cx.memories.set(
            MemoryKind::WalkTarget,
            walk_to_point(target.position, 2.0, 0),
        );
        *tongue = Tongue {
            eat_timer: tongue.eat_timer,
            path_counter: 10,
            phase: Phase::MoveToTarget,
        };
    }

    fn tick(&self, tongue: &mut Tongue, cx: &mut Cx<'_, '_, '_>) {
        let Some(target) = attack_target(cx) else {
            return;
        };
        set_tongue_target(cx, Some(target.key));
        match tongue.phase {
            Phase::MoveToTarget => {
                if float_distance(target.position, cx.ai.position()) < TONGUE_REACH {
                    cx.ai.tick.out.events.push(Event::Sound {
                        sound: sounds::ENTITY_FROG_TONGUE,
                        volume: 2.0,
                        pitch: 1.0,
                    });
                    set_pose(cx, Pose::UsingTongue);
                    // The prey's pull towards the frog is not made (named).
                    tongue.eat_timer = 0;
                    tongue.phase = Phase::CatchAnimation;
                } else if tongue.path_counter <= 0 {
                    cx.memories.set(
                        MemoryKind::WalkTarget,
                        walk_to_point(target.position, 2.0, 0),
                    );
                    tongue.path_counter = 10;
                } else {
                    tongue.path_counter -= 1;
                }
            }
            Phase::CatchAnimation => {
                let timer = tongue.eat_timer;
                tongue.eat_timer += 1;
                if timer >= 6 {
                    tongue.phase = Phase::EatAnimation;
                    Self::eat(cx);
                }
            }
            Phase::EatAnimation => {
                if tongue.eat_timer >= 10 {
                    tongue.phase = Phase::Done;
                } else {
                    tongue.eat_timer += 1;
                }
            }
            Phase::Done => {}
        }
    }

    fn stop(&self, tongue: &mut Tongue, cx: &mut Cx<'_, '_, '_>) {
        cx.memories.erase(MemoryKind::AttackTarget);
        set_tongue_target(cx, None);
        set_pose(cx, Pose::Standing);
        // At rest it keeps nothing the next start does not set again but its timer,
        // which only a catch reads after setting it.
        *tongue = Tongue {
            eat_timer: 0,
            path_counter: 0,
            phase: Phase::Done,
        };
    }
}

/// The frog as its long jump reads it: its sound, its landing (`isAcceptableLandingSpot`),
/// its friction and its pose.
pub(crate) struct FrogLeaper;

pub(crate) static FROG_LEAPER: FrogLeaper = FrogLeaper;

/// `FrogAi.isAcceptableLandingSpot(frog, pos)`: no fluid at the place, under it or over
/// it; a lily pad or big dripleaf at it or under it; a trapdoor at it, or air over one
/// (which the frog's path finding shuns, the bytecode's); else the default landing.
pub(crate) fn acceptable_landing(cx: &Cx<'_, '_, '_>, pos: BlockPos) -> bool {
    let world = cx.ai.tick.world;
    let below = BlockPos::new(pos.x, pos.y - 1, pos.z);
    let above = BlockPos::new(pos.x, pos.y + 1, pos.z);
    let fluid = |at: BlockPos| world.state(at).props().fluid().is_some();
    if fluid(pos) || fluid(below) || fluid(above) {
        return false;
    }
    if in_tag(world.state(pos), PREFER_JUMP_TO) || in_tag(world.state(below), PREFER_JUMP_TO) {
        return true;
    }
    let at = pathfinder::static_path_type(world, world.min_y(), pos);
    let under = pathfinder::static_path_type(world, world.min_y(), below);
    if at == PathType::Trapdoor || (world.state(pos).is_air() && under == PathType::Trapdoor) {
        return true;
    }
    default_landing(cx, pos)
}

impl Leaper for FrogLeaper {
    fn jump_sound(&self, _: &Mob) -> i32 {
        sounds::ENTITY_FROG_LONG_JUMP
    }

    fn landing(&self, cx: &Cx<'_, '_, '_>, pos: BlockPos) -> bool {
        acceptable_landing(cx, pos)
    }

    fn set_discard_friction(&self, cx: &mut Cx<'_, '_, '_>, discard: bool) {
        frog::update(&mut cx.ai.tick.mob, |frog| frog.discards_friction = discard);
    }

    fn set_long_jumping(&self, cx: &mut Cx<'_, '_, '_>, jumping: bool) {
        set_pose(
            cx,
            if jumping {
                Pose::LongJumping
            } else {
                Pose::Standing
            },
        );
    }
}

/// `canAttack(frog)`: not while breeding.
fn may_attack(cx: &mut Cx<'_, '_, '_>) -> bool {
    !breeding(cx)
}

/// The finder of its `StartAttacking`: `nearest_attackable`.
fn nearest_attackable(cx: &mut Cx<'_, '_, '_>) -> Option<EntityId> {
    entity_of(cx, MemoryKind::NearestAttackable)
}

fn start_attacking() -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(StartAttacking {
        condition: may_attack,
        finder: nearest_attackable,
    }))
}

fn look_at_players() -> Box<dyn Behaviour> {
    look_sometimes(LookAt::Player, 6.0, (30, 60))
}

fn tempted() -> Box<dyn Behaviour> {
    FollowTemptation::new(Speed::Fixed(1.25)).boxed()
}

fn croak() -> Box<dyn Behaviour> {
    Box::new(HeldTimed::new(Croak))
}

/// The activities, in `getActivities`' order: core, idle, swim, lay spawn, tongue and
/// long jump.
fn activities() -> Vec<ActivityData> {
    vec![core(), idle(), swim(), lay_spawn(), tongue(), long_jump()]
}

/// `initCoreActivity`.
fn core() -> ActivityData {
    ActivityData::numbered(
        Activity::Core,
        0,
        vec![
            panic(2.0),
            look_sink(45, 90),
            sink(),
            count_down(MemoryKind::TemptationCooldownTicks),
            count_down(MemoryKind::LongJumpCoolingDown),
        ],
    )
}

/// `initIdleActivity`: on land, not mid-jump.
fn idle() -> ActivityData {
    let wander = Gate::new(
        vec![(MemoryKind::WalkTarget, Status::Absent)],
        Vec::new(),
        Order::Shuffled,
        Running::RunOne,
        vec![
            (stroll(1.0), 1),
            (walk_to_look_target(1.0, 3), 1),
            (croak(), 3),
            (stand_still(on_ground), 2),
        ],
    );
    ActivityData {
        activity: Activity::Idle,
        behaviours: vec![
            (0, look_at_players()),
            (0, AnimalMakeLove::new(entity_types::FROG as u16).boxed()),
            (1, tempted()),
            (2, start_attacking()),
            (3, find_land(6, 1.0)),
            (4, Box::new(wander)),
        ],
        conditions: vec![
            (MemoryKind::LongJumpMidJump, Status::Absent),
            (MemoryKind::IsInWater, Status::Absent),
        ],
        erase: Vec::new(),
    }
}

/// `initSwimActivity`: in water, not mid-jump.
fn swim() -> ActivityData {
    let wander = Gate::new(
        vec![(MemoryKind::WalkTarget, Status::Absent)],
        Vec::new(),
        Order::Ordered,
        Running::TryAll,
        vec![
            (stroll_to(0.75, StrollTo::Swim), 1),
            (
                stroll_to(
                    1.0,
                    StrollTo::Land {
                        horizontal: 10,
                        vertical: 7,
                        in_water: true,
                    },
                ),
                1,
            ),
            (walk_to_look_target(1.0, 3), 1),
            (stand_still(in_water), 5),
        ],
    );
    ActivityData {
        activity: Activity::Swim,
        behaviours: vec![
            (0, look_at_players()),
            (1, tempted()),
            (2, start_attacking()),
            (3, find_land(8, 1.5)),
            (5, Box::new(wander)),
        ],
        conditions: vec![
            (MemoryKind::LongJumpMidJump, Status::Absent),
            (MemoryKind::IsInWater, Status::Present),
        ],
        erase: Vec::new(),
    }
}

/// `initLaySpawnActivity`: pregnant, not mid-jump.
fn lay_spawn() -> ActivityData {
    let wander = Gate::run_one(vec![
        (stroll(1.0), 2),
        (walk_to_look_target(1.0, 3), 1),
        (croak(), 2),
        (stand_still(on_ground), 1),
    ]);
    ActivityData {
        activity: Activity::LaySpawn,
        behaviours: vec![
            (0, look_at_players()),
            (1, start_attacking()),
            (
                2,
                find_land_near_liquid(8, 1.0, "minecraft:frog_tries_to_find_land_near"),
            ),
            (3, Box::new(HeldShot::new(TryLaySpawnOnFluidNearLand))),
            (4, Box::new(wander)),
        ],
        conditions: vec![
            (MemoryKind::LongJumpMidJump, Status::Absent),
            (MemoryKind::IsPregnant, Status::Present),
        ],
        erase: Vec::new(),
    }
}

/// `initTongueActivity`: while it has an attack target, which it forgets as it stops.
fn tongue() -> ActivityData {
    ActivityData::while_present(
        Activity::Tongue,
        0,
        vec![stop_attacking(), Box::new(HeldTimed::new(ShootTongue))],
        MemoryKind::AttackTarget,
    )
}

/// `initJumpActivity`: on land, untempted, not breeding and its jump cooled.
fn long_jump() -> ActivityData {
    ActivityData {
        activity: Activity::LongJump,
        behaviours: vec![
            (
                0,
                LongJumpMidJump {
                    interval: TIME_BETWEEN_LONG_JUMPS,
                    landing_sound: sounds::ENTITY_FROG_STEP,
                    leaper: &FROG_LEAPER,
                }
                .boxed(),
            ),
            (
                1,
                LongJumpToRandomPos {
                    interval: TIME_BETWEEN_LONG_JUMPS,
                    max_height: 2,
                    max_width: 4,
                    max_velocity_multiplier: 3.571_428_8,
                    leaper: &FROG_LEAPER,
                    preferred: Some(Preferred {
                        tag: PREFER_JUMP_TO,
                        chance: 0.5,
                    }),
                }
                .boxed(),
            ),
        ],
        conditions: vec![
            (MemoryKind::TemptingPlayer, Status::Absent),
            (MemoryKind::BreedTarget, Status::Absent),
            (MemoryKind::LongJumpCoolingDown, Status::Absent),
            (MemoryKind::IsInWater, Status::Absent),
        ],
        erase: Vec::new(),
    }
}

/// `FrogAi.updateActivity(frog)`: the first of the tongue, laying, jumping, swimming and
/// idling whose requirements are met.
pub(crate) fn update_activity(brain: &mut Brain) {
    brain.set_active_to_first_valid(&[
        Activity::Tongue,
        Activity::LaySpawn,
        Activity::LongJump,
        Activity::Swim,
        Activity::Idle,
    ]);
}

/// `customServerAiStep`'s brain: a pregnancy a breeding left in its state taken into its
/// brain (see the module's comment), then its tick and its activities.
pub(crate) fn step(tick: &mut MobTick<'_>) {
    if frog::read(&tick.mob).pregnant {
        frog::update(&mut tick.mob, |frog| frog.pregnant = false);
        let mut memories = crate::mobs::brain::memories_of(&PROVIDER, &tick.mob, tick.lived);
        memories.set(MemoryKind::IsPregnant, Value::Unit);
        crate::mobs::brain::write_memories(&PROVIDER, &mut tick.mob, memories);
    }
    crate::mobs::brain::run(tick, |brain, _| update_activity(brain));
}

/// `brain.setMemory(LONG_JUMP_COOLDOWN_TICKS, ticks)` of a frog being made: lowered from
/// its first brain tick.
pub(crate) fn set_long_jump_cooldown(mob: &mut Mob, ticks: i32) {
    let mut memories = crate::mobs::brain::memories_of(&PROVIDER, mob, 0);
    memories.set(MemoryKind::LongJumpCoolingDown, Value::Int(ticks));
    crate::mobs::brain::write_memories(&PROVIDER, mob, memories);
}
