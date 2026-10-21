//! Walking and looking where a memory says (the brief's 7.3.2 to 7.3.6): `RandomStroll`'s
//! one-shots, `SetWalkTargetFromLookTarget`, `MoveToTargetSink`, the one behaviour that
//! makes paths, and `LookAtTargetSink`, the one that turns the head.

use std::collections::{BTreeMap, BTreeSet};

use clustine_world::{BlockPos, Entry};
use serde::{Deserialize, Serialize};

use super::super::sensors::find;
use super::super::{Behaviour, Cx, HeldShot, HeldTimed, MemoryKind, Shot, Status, Timed, Value};
use super::{
    Counter, Speed, Walk, block_of, bottom_centre, entry, look_target, manhattan, position_of,
    read_entry, visible_by, walk, walk_target, walk_to_point,
};
use crate::mobs::ai::goals::view_vector;
use crate::mobs::ai::navigation::{self, Made};
use crate::mobs::ai::{controls, random_pos};
use crate::physics::{self, Vec3, World};

/// `(double)(float)(π / 2)`, the angle the random positions towards a direction take.
pub(crate) const HALF_PI: f64 = 1.570_796_370_506_286_6;

/// Where a stroll goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum StrollTo {
    /// `stroll(speed, h, v)`: `LandRandomPos.getPos(mob, h, v)`; with `in_water` false
    /// (`stroll(speed, false)`), never while the mob is in water.
    Land {
        horizontal: i32,
        vertical: i32,
        in_water: bool,
    },
    /// `fly(speed)`: `AirAndWaterRandomPos.getPos(mob, 10, 7, -2, view, π/2)`.
    Fly,
    /// `swim(speed)`: `getTargetSwimPos`, while the mob is in water.
    Swim,
}

/// `RandomStroll` (7.3.2): with no walk target, one to a random place (none where none is
/// found); it fires either way while its condition holds.
pub(crate) struct RandomStroll {
    pub(crate) speed: f32,
    pub(crate) to: StrollTo,
}

/// `RandomStroll.stroll(speed)`: on land within 10 and 7, in water too.
pub(crate) fn stroll(speed: f32) -> Box<dyn Behaviour> {
    stroll_to(
        speed,
        StrollTo::Land {
            horizontal: 10,
            vertical: 7,
            in_water: true,
        },
    )
}

/// A `RandomStroll` of any form, boxed.
pub(crate) fn stroll_to(speed: f32, to: StrollTo) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(RandomStroll { speed, to }))
}

/// `SWIM_XY_DISTANCE_TIERS`.
const SWIM_TIERS: [(i32, i32); 6] = [(1, 1), (3, 3), (5, 5), (6, 5), (7, 7), (10, 7)];

/// `RandomStroll.getTargetSwimPos(mob)`: a first swimmable place near, then out along
/// the same way as far as the water lasts.
pub(crate) fn target_swim_pos(cx: &mut Cx<'_, '_, '_>) -> Option<Vec3> {
    let mut best: Option<Vec3> = None;
    let mut candidate: Option<Vec3> = None;
    for (xz, y) in SWIM_TIERS {
        candidate = match best {
            None => random_pos::swimmable_pos(cx.ai, xz, y),
            Some(best) => {
                let position = cx.ai.position();
                let way = physics::normalize(physics::sub(best, position));
                Some([
                    position[0] + way[0] * f64::from(xz),
                    position[1] + way[1] * f64::from(y),
                    position[2] + way[2] * f64::from(xz),
                ])
            }
        };
        let restricted = random_pos::mob_restricted(cx.ai, f64::from(xz));
        let stops = match candidate {
            None => true,
            Some(at) => {
                let pos = physics::containing(at);
                cx.ai.tick.world.state(pos).props().fluid().is_none()
                    || (restricted && !cx.ai.within_home(pos))
            }
        };
        if stops {
            return best;
        }
        best = candidate;
    }
    candidate
}

impl Shot for RandomStroll {
    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::WalkTarget]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx.memories.check(MemoryKind::WalkTarget, Status::Absent) {
            return false;
        }
        let place = match self.to {
            StrollTo::Land {
                horizontal,
                vertical,
                in_water,
            } => {
                if !in_water && cx.ai.in_water() {
                    return false;
                }
                random_pos::land_pos(cx.ai, horizontal, vertical)
            }
            StrollTo::Fly => {
                let view = view_vector(cx.ai.tick.pitch, cx.ai.tick.yaw);
                random_pos::air_and_water_pos(cx.ai, 10, 7, -2, view[0], view[2], HALF_PI)
            }
            StrollTo::Swim => {
                if !cx.ai.in_water() {
                    return false;
                }
                target_swim_pos(cx)
            }
        };
        match place {
            Some(at) => cx
                .memories
                .set(MemoryKind::WalkTarget, walk_to_point(at, self.speed, 0)),
            None => cx.memories.erase(MemoryKind::WalkTarget),
        }
        true
    }
}

/// `SetWalkTargetFromLookTarget` (7.3.4): with no walk target and a look target, a walk
/// to what is looked at, while `predicate` holds. No draws.
pub(crate) struct SetWalkTargetFromLookTarget {
    pub(crate) predicate: fn(&Cx<'_, '_, '_>) -> bool,
    pub(crate) speed: Speed,
    pub(crate) close_enough: i32,
}

fn any(_: &Cx<'_, '_, '_>) -> bool {
    true
}

/// `SetWalkTargetFromLookTarget.create(speed, closeEnough)`.
pub(crate) fn walk_to_look_target(speed: f32, close_enough: i32) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(SetWalkTargetFromLookTarget {
        predicate: any,
        speed: Speed::Fixed(speed),
        close_enough,
    }))
}

impl Shot for SetWalkTargetFromLookTarget {
    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::WalkTarget, MemoryKind::LookTarget]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx.memories.check(MemoryKind::WalkTarget, Status::Absent) {
            return false;
        }
        let Some(tracker) = look_target(cx) else {
            return false;
        };
        if !(self.predicate)(cx) {
            return false;
        }
        let speed = self.speed.of(cx);
        cx.memories.set(
            MemoryKind::WalkTarget,
            walk(tracker, speed, self.close_enough),
        );
        true
    }
}

/// What `MoveToTargetSink` keeps: its run, its retry cooldown (lowered only by the start
/// attempts it makes, a [`Counter`]), whether it holds a path (the navigation's, which
/// the body keeps), the block it last made a path to and the speed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SinkState {
    pub(crate) running: bool,
    pub(crate) end: i64,
    /// `remainingCooldown`.
    pub(crate) cooldown: Counter,
    /// `path != null`.
    pub(crate) path: bool,
    /// `lastTargetPos`.
    pub(crate) last_target: Option<BlockPos>,
    /// `speedModifier`.
    pub(crate) speed: f32,
}

/// `MoveToTargetSink(min, max)` (7.3.5): a walk target becomes a path the navigation
/// follows; reached or unreachable, it is erased; a mob stuck on its way waits 0 to 39
/// start attempts before it tries again; `cant_reach_walk_target_since` holds the game
/// time of the first failure to reach.
///
/// A walk target that names an entity the view has not ("unknown") is kept and nothing
/// is done with it this tick: no path is made to it, it is not erased, and a run on its
/// way goes on along the path it has (the plan, section 8 item 8).
pub(crate) struct MoveToTargetSink {
    pub(crate) min: i64,
    pub(crate) max: i64,
    /// What a type asks before the sink's own start conditions (the armadillo's: not
    /// while scared); where it refuses, the cooldown waits too.
    pub(crate) may_start: fn(&mut Cx<'_, '_, '_>) -> bool,
    pub(crate) state: SinkState,
    /// The path `tryComputePath` made, for `start` in the same tick.
    made: Option<Made>,
}

/// The sink's entry: `cant_reach_walk_target_since` registered, no path, a walk target.
const SINK_ENTRY: [(MemoryKind, Status); 3] = [
    (MemoryKind::CantReachWalkTargetSince, Status::Registered),
    (MemoryKind::Path, Status::Absent),
    (MemoryKind::WalkTarget, Status::Present),
];

fn always(_: &mut Cx<'_, '_, '_>) -> bool {
    true
}

impl MoveToTargetSink {
    /// `MoveToTargetSink(min, max)`.
    pub(crate) fn new(min: i64, max: i64) -> Self {
        Self::refusing(min, max, always)
    }

    /// A sink of a type's that refuses to start where `may_start` is false.
    pub(crate) fn refusing(min: i64, max: i64, may_start: fn(&mut Cx<'_, '_, '_>) -> bool) -> Self {
        Self {
            min,
            max,
            may_start,
            state: SinkState::default(),
            made: None,
        }
    }

    /// `reachedTarget(mob, walkTarget)`: within its distance, by Manhattan's; never for
    /// a target the view has not.
    fn reached(cx: &Cx<'_, '_, '_>, walk: &Walk) -> bool {
        block_of(cx, &walk.to)
            .is_some_and(|pos| manhattan(pos, cx.ai.block_position()) <= walk.close_enough)
    }

    /// `tryComputePath(mob, walkTarget, time)`.
    fn try_compute_path(&mut self, cx: &mut Cx<'_, '_, '_>, walk: &Walk, pos: BlockPos) -> bool {
        let mut made = navigation::create_path(cx.ai, pos, 0);
        self.state.speed = walk.speed;
        // Reached, or a path that reaches: it can reach.
        let reaches = manhattan(pos, cx.ai.block_position()) <= walk.close_enough
            || made
                .as_ref()
                .and_then(|made| made.path(cx.ai))
                .is_some_and(|path| path.reached);
        if reaches {
            cx.memories.erase(MemoryKind::CantReachWalkTargetSince);
        } else if cx
            .memories
            .check(MemoryKind::CantReachWalkTargetSince, Status::Absent)
        {
            cx.memories
                .set(MemoryKind::CantReachWalkTargetSince, Value::Long(cx.time));
        }
        if made.is_none()
            && let Some(towards) =
                random_pos::default_pos_towards(cx.ai, 10, 7, bottom_centre(pos), HALF_PI)
        {
            made = navigation::create_path_at(cx.ai, towards, 0);
        }
        self.state.path = made.is_some();
        let found = made.is_some();
        self.made = made;
        found
    }

    fn start(&mut self, cx: &mut Cx<'_, '_, '_>) {
        cx.memories.set(MemoryKind::Path, Value::Path);
        let made = self.made.take();
        navigation::move_to_path(cx.ai, made, f64::from(self.state.speed));
    }

    fn can_still_use(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !self.state.path || self.state.last_target.is_none() {
            return false;
        }
        let Some(walk) = walk_target(cx) else {
            return false;
        };
        let spectator = match walk.to {
            super::super::Tracker::Entity { key, .. } => {
                find(cx.ai, key).is_some_and(|looked| looked.spectator)
            }
            _ => false,
        };
        !navigation::is_done(cx.ai) && !Self::reached(cx, &walk) && !spectator
    }

    fn tick(&mut self, cx: &mut Cx<'_, '_, '_>) {
        let followed = cx.ai.tick.mob.body.path.followed.is_some();
        if self.state.path != followed {
            self.state.path = followed;
            if followed {
                cx.memories.set(MemoryKind::Path, Value::Path);
            } else {
                cx.memories.erase(MemoryKind::Path);
            }
        }
        let Some(last) = self.state.last_target else {
            return;
        };
        if !followed {
            return;
        }
        let Some(walk) = walk_target(cx) else {
            return;
        };
        let Some(pos) = block_of(cx, &walk.to) else {
            return;
        };
        if super::super::sensors::block_distance_sqr(pos, last) > 4.0
            && self.try_compute_path(cx, &walk, pos)
        {
            self.state.last_target = Some(pos);
            self.start(cx);
        }
    }

    fn stop(&mut self, cx: &mut Cx<'_, '_, '_>) {
        if let Some(walk) = walk_target(cx)
            && !Self::reached(cx, &walk)
            && cx.ai.tick.mob.body.progress.stuck
        {
            let wait = cx.ai.tick.draws.next_int(40) as i32;
            self.state.cooldown = Counter::at_rest(wait);
        }
        navigation::stop(cx.ai);
        cx.memories.erase(MemoryKind::WalkTarget);
        cx.memories.erase(MemoryKind::Path);
        self.state.path = false;
    }
}

impl Behaviour for MoveToTargetSink {
    fn required(&self, into: &mut BTreeSet<MemoryKind>) {
        into.extend(SINK_ENTRY.iter().map(|(kind, _)| *kind));
    }

    fn running(&self) -> bool {
        self.state.running
    }

    fn try_start(&mut self, cx: &mut Cx<'_, '_, '_>) -> bool {
        let now = cx.ai.tick.lived;
        let met = SINK_ENTRY
            .iter()
            .all(|(kind, status)| cx.memories.check(*kind, *status));
        if !met || !(self.may_start)(cx) {
            self.state.cooldown.pause(now);
            return false;
        }
        // `checkExtraStartConditions`: each attempt lowers the cooldown, until it is
        // spent.
        if self.state.cooldown.at(now) > 0 {
            self.state.cooldown.lower(now);
            return false;
        }
        self.state.cooldown = Counter::default();
        let Some(walk) = walk_target(cx) else {
            return false;
        };
        let Some(pos) = block_of(cx, &walk.to) else {
            // Unknown: kept, and nothing done with it this tick.
            return false;
        };
        let reached = manhattan(pos, cx.ai.block_position()) <= walk.close_enough;
        if reached || !self.try_compute_path(cx, &walk, pos) {
            cx.memories.erase(MemoryKind::WalkTarget);
            if reached {
                cx.memories.erase(MemoryKind::CantReachWalkTargetSince);
            }
            self.made = None;
            return false;
        }
        self.state.last_target = Some(pos);
        // `Behavior.tryStart`: running, its end drawn, then `start`.
        self.state.running = true;
        let bound = u32::try_from(self.max + 1 - self.min).unwrap_or(1).max(1);
        let drawn = i64::from(cx.ai.tick.draws.next_int(bound));
        self.state.end = cx.time + self.min + drawn;
        self.start(cx);
        true
    }

    fn tick_or_stop(&mut self, cx: &mut Cx<'_, '_, '_>) {
        if cx.time <= self.state.end && self.can_still_use(cx) {
            self.tick(cx);
        } else {
            self.do_stop(cx);
        }
    }

    fn do_stop(&mut self, cx: &mut Cx<'_, '_, '_>) {
        self.state.running = false;
        self.state.end = 0;
        self.stop(cx);
    }

    fn load(&mut self, first: u16, parts: &BTreeMap<u16, Entry>) {
        self.state = read_entry(first, 1, parts);
        self.made = None;
    }

    fn save(&self, first: u16, into: &mut Vec<Entry>) {
        if self.state != SinkState::default() {
            into.push(entry(first, 1, &self.state));
        }
    }
}

/// `new MoveToTargetSink()`: 150 to 250 ticks.
pub(crate) fn sink() -> Box<dyn Behaviour> {
    Box::new(MoveToTargetSink::new(150, 250))
}

/// `LookAtTargetSink(min, max)` (7.3.6): the head turned to the look target while it is
/// seen, then the target forgotten. A target the view has not is taken as out of sight,
/// the game's answer for one beyond its sight (the plan, section 8 item 8).
pub(crate) struct LookAtTargetSink {
    pub(crate) min: i64,
    pub(crate) max: i64,
}

/// `LookAtTargetSink(min, max)`, boxed.
pub(crate) fn look_sink(min: i64, max: i64) -> Box<dyn Behaviour> {
    Box::new(HeldTimed::new(LookAtTargetSink { min, max }))
}

impl Timed for LookAtTargetSink {
    type State = ();

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[(MemoryKind::LookTarget, Status::Present)]
    }

    fn durations(&self) -> (i64, i64) {
        (self.min, self.max)
    }

    fn can_still_use(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) -> bool {
        look_target(cx).is_some_and(|tracker| visible_by(cx, &tracker))
    }

    fn tick(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        if let Some(at) = look_target(cx).and_then(|tracker| position_of(cx, &tracker)) {
            controls::set_look_at_default(cx.ai, at);
        }
    }

    fn stop(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        cx.memories.erase(MemoryKind::LookTarget);
    }
}
