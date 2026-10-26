//! Ramming (the brief's 7.3.18 and 7.3.19): `PrepareRamNearestTarget`, which walks to a
//! start 4 to 7 blocks from the target in one of the four directions and lowers the
//! head, and `RamTarget`, the charge.

use clustine_blocks::data::in_tag;
use clustine_data::attributes;
use clustine_world::{BlockPos, EntityId};
use serde::{Deserialize, Serialize};

use super::super::sensors::{block_distance_sqr, find, find_closest_visible, living_in};
use super::super::{Behaviour, Cx, HeldTimed, MemoryKind, Status, Timed, Tracker, Value};
use super::{
    bottom_centre, manhattan, position_of, sample, tracking, walk, walk_target, walk_to_point,
};
use crate::mobs::Mob;
use crate::mobs::ai::sensing::{Conditions, Looked};
use crate::mobs::ai::{maths, navigation, pathfinder};
use crate::mobs::body::Event;
use crate::mobs::body::hurt::voice_pitch;
use crate::mobs::tick::Local;
use crate::physics::{self, Vec3, World};

/// The entity events of the head lowered and raised again.
pub(crate) const LOWER_HEAD: u8 = 58;
pub(crate) const RAISE_HEAD: u8 = 59;

/// The point `ram_target` holds (a `Vec3`, kept as a point tracker).
pub(crate) fn ram_target(cx: &Cx<'_, '_, '_>) -> Option<Vec3> {
    match cx.memories.get(MemoryKind::RamTarget)? {
        Value::Tracker(Tracker::Point(point)) => Some(point),
        _ => None,
    }
}

/// `PrepareRamNearestTarget.RamCandidate`: where to start from, where the target stood,
/// and the target.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct RamCandidate {
    pub(crate) start: BlockPos,
    pub(crate) target_pos: BlockPos,
    pub(crate) target: EntityId,
}

/// What a preparation keeps: when it reached its start, and its candidate.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Preparing {
    pub(crate) reached_at: Option<i64>,
    pub(crate) candidate: Option<RamCandidate>,
}

/// `PrepareRamNearestTarget(cooldownOnFail, minDistance, maxDistance, walkSpeed,
/// ramTargeting, prepareTime, sound)` (7.3.18), 160 ticks: the closest seen thing the ram
/// may take, a start found for it, a walk there, then the head lowered for
/// `prepare_time` ticks, after which `ram_target` is set at the edge of the target's
/// block. A target the view no longer has is not taken as dead: nothing is done with it
/// this tick (the plan, section 8 item 8).
pub(crate) struct PrepareRamNearestTarget {
    pub(crate) cooldown_on_fail: fn(&Mob) -> i32,
    pub(crate) min_distance: i32,
    pub(crate) max_distance: i32,
    pub(crate) walk_speed: f32,
    /// The ram's `TargetingConditions`' selector (a combat condition).
    pub(crate) targets: fn(&Looked) -> bool,
    pub(crate) prepare_time: i64,
    pub(crate) sound: fn(&Mob) -> i32,
}

impl PrepareRamNearestTarget {
    pub(crate) fn boxed(self) -> Box<dyn Behaviour> {
        Box::new(HeldTimed::new(self))
    }

    /// `isWalkableBlock(mob, pos)`: somewhere to stand, of a cell that costs nothing.
    fn walkable(cx: &Cx<'_, '_, '_>, pos: BlockPos) -> bool {
        let world = cx.ai.tick.world;
        navigation::is_stable_destination(cx.ai, pos)
            && cx
                .ai
                .malus(pathfinder::static_path_type(world, world.min_y(), pos))
                == 0.0
    }

    /// `calculateRammingStartPosition(mob, target)`: in each horizontal direction, as far
    /// as `max_distance` walkable blocks go; those at least `min_distance` away, nearest
    /// the mob first, the first a path reaches.
    pub(crate) fn start_position(
        &self,
        cx: &mut Cx<'_, '_, '_>,
        target: BlockPos,
    ) -> Option<BlockPos> {
        if !Self::walkable(cx, target) {
            return None;
        }
        let mut starts = Vec::new();
        // `Direction.Plane.HORIZONTAL`: north, east, south, west.
        for (dx, dz) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
            let mut at = target;
            for _ in 0..self.max_distance {
                at = BlockPos::new(at.x + dx, at.y, at.z + dz);
                if !Self::walkable(cx, at) {
                    at = BlockPos::new(at.x - dx, at.y, at.z - dz);
                    break;
                }
            }
            if manhattan(at, target) >= self.min_distance {
                starts.push(at);
            }
        }
        let own = cx.ai.block_position();
        starts.sort_by(|a, b| block_distance_sqr(own, *a).total_cmp(&block_distance_sqr(own, *b)));
        starts.into_iter().find(|&start| {
            navigation::create_path(cx.ai, start, 0)
                .as_ref()
                .and_then(|made| made.path(cx.ai))
                .is_some_and(|path| path.reached)
        })
    }

    /// `chooseRamPosition(mob, target)`.
    fn choose(&self, state: &mut Preparing, cx: &mut Cx<'_, '_, '_>, target: &Looked) {
        state.reached_at = None;
        let target_pos = target.block_position();
        state.candidate = self
            .start_position(cx, target_pos)
            .map(|start| RamCandidate {
                start,
                target_pos,
                target: target.key,
            });
    }
}

impl Timed for PrepareRamNearestTarget {
    type State = Preparing;

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[
            (MemoryKind::LookTarget, Status::Registered),
            (MemoryKind::RamCooldownTicks, Status::Absent),
            (MemoryKind::VisibleMobs, Status::Present),
            (MemoryKind::RamTarget, Status::Absent),
        ]
    }

    fn durations(&self) -> (i64, i64) {
        (160, 160)
    }

    fn start(&self, state: &mut Preparing, cx: &mut Cx<'_, '_, '_>) {
        let targets = self.targets;
        let found = find_closest_visible(cx, &mut |cx, looked| {
            Conditions::combat().test(cx.ai, looked, &|target| targets(target))
        });
        if let Some(target) = found {
            self.choose(state, cx, &target);
        }
    }

    fn can_still_use(&self, state: &mut Preparing, cx: &mut Cx<'_, '_, '_>) -> bool {
        // Unknown is not dead: a target the view has not is still taken as alive.
        state.candidate.is_some_and(|candidate| {
            find(cx.ai, candidate.target).is_none_or(|target| target.alive)
        })
    }

    fn tick(&self, state: &mut Preparing, cx: &mut Cx<'_, '_, '_>) {
        let Some(candidate) = state.candidate else {
            return;
        };
        cx.memories.set(
            MemoryKind::WalkTarget,
            walk(Tracker::Block(candidate.start), self.walk_speed, 0),
        );
        cx.memories.set(
            MemoryKind::LookTarget,
            Value::Tracker(tracking(candidate.target, true)),
        );
        let Some(target) = find(cx.ai, candidate.target) else {
            return;
        };
        if target.block_position() != candidate.target_pos {
            cx.ai.tick.out.events.push(Event::Entity(RAISE_HEAD));
            navigation::stop(cx.ai);
            self.choose(state, cx, &target);
        } else if cx.ai.block_position() == candidate.start {
            cx.ai.tick.out.events.push(Event::Entity(LOWER_HEAD));
            let reached_at = *state.reached_at.get_or_insert(cx.time);
            if cx.time - reached_at >= self.prepare_time {
                let edge = edge_of_block(cx.ai.block_position(), candidate.target_pos);
                cx.memories
                    .set(MemoryKind::RamTarget, Value::Tracker(Tracker::Point(edge)));
                let sound = (self.sound)(&cx.ai.tick.mob);
                let baby = cx.ai.tick.mob_type.is_baby(&cx.ai.tick.mob);
                let pitch =
                    voice_pitch(cx.ai.tick.draws, baby) * cx.ai.tick.mob_type.voice_pitch_factor();
                cx.ai.tick.out.events.push(Event::Sound {
                    sound,
                    volume: 1.0,
                    pitch,
                });
                state.candidate = None;
            }
        }
    }

    fn stop(&self, _: &mut Preparing, cx: &mut Cx<'_, '_, '_>) {
        if !cx.memories.check(MemoryKind::RamTarget, Status::Present) {
            cx.ai.tick.out.events.push(Event::Entity(RAISE_HEAD));
            let cooldown = (self.cooldown_on_fail)(&cx.ai.tick.mob);
            cx.memories
                .set(MemoryKind::RamCooldownTicks, Value::Int(cooldown));
        }
    }
}

/// `getEdgeOfBlock(start, target)`: the bottom centre of the target's block, moved half
/// a block towards the start on each axis it differs on.
pub(crate) fn edge_of_block(start: BlockPos, target: BlockPos) -> Vec3 {
    let sign = |d: i32| f64::from(d.signum());
    let base = bottom_centre(target);
    [
        base[0] + 0.5 * sign(start.x - target.x),
        base[1],
        base[2] + 0.5 * sign(start.z - target.z),
    ]
}

/// What a ram's type gives it (`RamTarget`'s functions of the goat, and `dropHorn`).
pub(crate) trait Rammer: Sync {
    /// `getTimeBetweenRams(goat)`: the cooldown's bounds.
    fn time_between_rams(&self, mob: &Mob) -> (i32, i32);
    /// `getKnockbackForce(goat)`.
    fn knockback(&self, mob: &Mob) -> f64;
    fn impact_sound(&self, mob: &Mob) -> i32;
    fn horn_break_sound(&self, mob: &Mob) -> i32;
    /// `dropHorn()`: whether a horn came off.
    fn drop_horn(&self, cx: &mut Cx<'_, '_, '_>) -> bool;
}

/// `RamTarget(timeBetweenRams, ramTargeting, speed, knockback, impactSound,
/// hornBreakSound)` (7.3.19), 200 ticks: a charge at `ram_target`; the first thing the
/// ram may take that the mob's box meets is hurt (`noAggroMobAttack`) and knocked back
/// along the charge; a horn-snapping block ahead knocks a horn off; either, or the walk
/// ending, ends the ram, raises the head and draws its cooldown.
///
/// **Named**: the hurt is the region's `Local::Hurt` (a mob attack), where the game's
/// source angers nothing; no mob has effects, so neither speed nor slowness changes the
/// push; no mob blocks with a shield.
pub(crate) struct RamTarget {
    pub(crate) speed: f32,
    pub(crate) targets: fn(&Looked) -> bool,
    pub(crate) rammer: &'static dyn Rammer,
}

/// What a ram keeps: `ramDirection`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Ramming {
    pub(crate) direction: Vec3,
}

impl RamTarget {
    pub(crate) fn boxed(self) -> Box<dyn Behaviour> {
        Box::new(HeldTimed::new(self))
    }

    /// `finishRam(level, goat)`.
    fn finish(&self, cx: &mut Cx<'_, '_, '_>) {
        cx.ai.tick.out.events.push(Event::Entity(RAISE_HEAD));
        let bounds = self.rammer.time_between_rams(&cx.ai.tick.mob);
        let cooldown = sample(cx.ai.tick.draws, bounds);
        cx.memories
            .set(MemoryKind::RamCooldownTicks, Value::Int(cooldown));
        cx.memories.erase(MemoryKind::RamTarget);
    }

    fn sound(cx: &mut Cx<'_, '_, '_>, sound: i32) {
        cx.ai.tick.out.events.push(Event::Sound {
            sound,
            volume: 1.0,
            pitch: 1.0,
        });
    }

    /// `hasRammedHornBreakingBlock(level, goat)`: the block a step ahead along its
    /// motion, or the one above it, snaps horns.
    fn rammed_horn_breaking_block(cx: &Cx<'_, '_, '_>) -> bool {
        let motion = cx.ai.tick.living.body.motion;
        let ahead = physics::normalize([motion[0], 0.0, motion[2]]);
        let pos = physics::containing(physics::add(cx.ai.position(), ahead));
        let world = cx.ai.tick.world;
        let snaps = |pos: BlockPos| in_tag(world.state(pos), "minecraft:snaps_goat_horn");
        snaps(pos) || snaps(BlockPos::new(pos.x, pos.y + 1, pos.z))
    }
}

impl Timed for RamTarget {
    type State = Ramming;

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[
            (MemoryKind::RamCooldownTicks, Status::Absent),
            (MemoryKind::RamTarget, Status::Present),
        ]
    }

    fn durations(&self) -> (i64, i64) {
        (200, 200)
    }

    fn can_still_use(&self, _: &mut Ramming, cx: &mut Cx<'_, '_, '_>) -> bool {
        cx.memories.check(MemoryKind::RamTarget, Status::Present)
    }

    fn start(&self, state: &mut Ramming, cx: &mut Cx<'_, '_, '_>) {
        let Some(target) = ram_target(cx) else {
            return;
        };
        let block = cx.ai.block_position();
        state.direction = physics::normalize([
            f64::from(block.x) - target[0],
            0.0,
            f64::from(block.z) - target[2],
        ]);
        cx.memories
            .set(MemoryKind::WalkTarget, walk_to_point(target, self.speed, 0));
    }

    fn tick(&self, state: &mut Ramming, cx: &mut Cx<'_, '_, '_>) {
        let bounds = cx.ai.tick.living.body.bounds();
        let targets = self.targets;
        let hit = living_in(cx.ai, bounds)
            .into_iter()
            .find(|looked| Conditions::combat().test(cx.ai, looked, &|target| targets(target)));
        if let Some(hit) = hit {
            let amount = cx.ai.attribute(attributes::ATTACK_DAMAGE) as f32;
            let speed = cx.ai.tick.mob.body.inputs.speed;
            let force = maths::clamp(speed * 1.65, 0.2, 3.0) + 0.25 * 0.0;
            let strength = f64::from(force) * self.rammer.knockback(&cx.ai.tick.mob);
            let from = cx.ai.position();
            let kind = cx.ai.tick.kind;
            cx.ai.tick.out.local.push(Local::Hurt {
                key: hit.key,
                kind,
                from,
                amount,
                sound: None,
                knockback: Some([strength, state.direction[0], state.direction[2]]),
                fling: None,
            });
            self.finish(cx);
            Self::sound(cx, self.rammer.impact_sound(&cx.ai.tick.mob));
        } else if Self::rammed_horn_breaking_block(cx) {
            Self::sound(cx, self.rammer.impact_sound(&cx.ai.tick.mob));
            if self.rammer.drop_horn(cx) {
                Self::sound(cx, self.rammer.horn_break_sound(&cx.ai.tick.mob));
            }
            self.finish(cx);
        } else {
            let walk_to = walk_target(cx).and_then(|walk| position_of(cx, &walk.to));
            let done = match (walk_to, ram_target(cx)) {
                (Some(at), Some(target)) => physics::distance_sqr(at, target) < 0.25 * 0.25,
                _ => true,
            };
            if done {
                self.finish(cx);
            }
        }
    }
}
