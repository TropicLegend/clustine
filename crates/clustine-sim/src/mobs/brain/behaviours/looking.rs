//! Choosing what to look at (the brief's 7.3.7, 7.3.8 and 7.3.22):
//! `SetEntityLookTargetSometimes` with its ticker, `SetEntityLookTarget` and
//! `RandomLookAround`.

use serde::{Deserialize, Serialize};

use super::super::sensors::find_closest_visible;
use super::super::{Behaviour, Cx, HeldShot, HeldTimed, MemoryKind, Shot, Status, Timed, Value};
use super::{Counter, HeldKept, KeptShot, sample, tracking};
use crate::mobs::ai::maths;
use crate::mobs::ai::sensing::Looked;
use crate::physics::Vec3;

/// Which things a look takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LookAt {
    /// Any living thing.
    Any,
    /// The things of one entity type (`EntityType.PLAYER` is [`LookAt::Player`]).
    Kind(u16),
    /// Players.
    Player,
}

impl LookAt {
    fn takes(self, looked: &Looked) -> bool {
        match self {
            LookAt::Any => true,
            LookAt::Kind(kind) => looked.kind == Some(kind),
            LookAt::Player => looked.is_player(),
        }
    }
}

/// `SetEntityLookTargetSometimes.create([type,] maxDist, interval)` (7.3.7): with no look
/// target, the closest seen thing within `max_distance` is looked at, once its ticker
/// says; the ticker counts only on ticks a candidate is found, and is kept as a
/// [`Counter`] (`ticksUntilNextStart`).
pub(crate) struct SetEntityLookTargetSometimes {
    pub(crate) look_at: LookAt,
    pub(crate) max_distance: f32,
    /// The interval's bounds (`UniformInt`, its minimum at least 2: the `Ticker`'s
    /// constructor throws below).
    pub(crate) interval: (i32, i32),
}

/// [`SetEntityLookTargetSometimes`], boxed.
pub(crate) fn look_sometimes(
    look_at: LookAt,
    max_distance: f32,
    interval: (i32, i32),
) -> Box<dyn Behaviour> {
    assert!(interval.0 > 1, "the ticker's interval starts at 2 or more");
    Box::new(HeldKept::new(SetEntityLookTargetSometimes {
        look_at,
        max_distance,
        interval,
    }))
}

/// The ticker's state: `ticksUntilNextStart`, as a [`Counter`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Ticker {
    pub(crate) ticks: Counter,
}

/// The closest seen thing `look_at` takes, within `max_distance`, and not `except`'s
/// passenger where `passengers` is false.
fn closest(
    cx: &mut Cx<'_, '_, '_>,
    look_at: LookAt,
    max_distance: f32,
    passengers: bool,
) -> Option<Looked> {
    // `maxDist * maxDist` in float, widened at the comparison.
    let most = f64::from(max_distance * max_distance);
    let own = cx.ai.tick.key;
    let from = cx.ai.position();
    find_closest_visible(cx, &mut |_, looked| {
        look_at.takes(looked)
            && looked.distance_sqr(from) <= most
            && (passengers || looked.vehicle != Some(own))
    })
}

impl KeptShot for SetEntityLookTargetSometimes {
    type State = Ticker;

    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::LookTarget, MemoryKind::VisibleMobs]
    }

    fn trigger(&self, state: &mut Ticker, cx: &mut Cx<'_, '_, '_>) -> bool {
        let now = cx.ai.tick.lived;
        if !cx.memories.check(MemoryKind::LookTarget, Status::Absent)
            || !cx.memories.check(MemoryKind::VisibleMobs, Status::Present)
        {
            state.ticks.pause(now);
            return false;
        }
        let Some(target) = closest(cx, self.look_at, self.max_distance, true) else {
            state.ticks.pause(now);
            return false;
        };
        // `Ticker.tickDownAndCheck(level.getRandom())`.
        let left = state.ticks.at(now);
        if left == 0 {
            let next = sample(cx.ai.tick.draws, self.interval) - 1;
            state.ticks.set(next, now, true);
            return false;
        }
        if left > 1 {
            state.ticks.lower(now);
            return false;
        }
        // Down to nothing: it fires, and the look target it sets keeps the ticker from
        // counting until it is gone, so it rests at nothing.
        state.ticks = Counter::default();
        cx.memories.set(
            MemoryKind::LookTarget,
            Value::Tracker(tracking(target.key, true)),
        );
        true
    }

    fn left(&self, state: &mut Ticker, now: u32, _: i64) {
        // As of the tick after the last it was asked at.
        state.ticks.pause(now + 1);
    }
}

/// `SetEntityLookTarget.create([category | type,] maxDist)` (7.3.8): with no look target,
/// the closest seen thing within `max_distance` that does not ride the mob. No draws.
pub(crate) struct SetEntityLookTarget {
    pub(crate) look_at: LookAt,
    pub(crate) max_distance: f32,
}

/// [`SetEntityLookTarget`], boxed.
pub(crate) fn look_at(look_at: LookAt, max_distance: f32) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(SetEntityLookTarget {
        look_at,
        max_distance,
    }))
}

impl Shot for SetEntityLookTarget {
    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::LookTarget, MemoryKind::VisibleMobs]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx.memories.check(MemoryKind::LookTarget, Status::Absent)
            || !cx.memories.check(MemoryKind::VisibleMobs, Status::Present)
        {
            return false;
        }
        let Some(target) = closest(cx, self.look_at, self.max_distance, false) else {
            return false;
        };
        cx.memories.set(
            MemoryKind::LookTarget,
            Value::Tracker(tracking(target.key, true)),
        );
        true
    }
}

/// `RandomLookAround(interval, maxYaw, minPitch, maxPitch)` (7.3.22): with no look target
/// and no gaze cooldown, a look at a point a block from the eyes, drawn about the yaw,
/// and a cooldown drawn. It starts and stops in one tick.
pub(crate) struct RandomLookAround {
    pub(crate) interval: (i32, i32),
    pub(crate) max_yaw: f32,
    pub(crate) min_pitch: f32,
    pub(crate) max_pitch: f32,
}

/// [`RandomLookAround`], boxed.
pub(crate) fn look_around(
    interval: (i32, i32),
    max_yaw: f32,
    min_pitch: f32,
    max_pitch: f32,
) -> Box<dyn Behaviour> {
    assert!(
        min_pitch <= max_pitch,
        "the least pitch is not above the most"
    );
    Box::new(HeldTimed::new(RandomLookAround {
        interval,
        max_yaw,
        min_pitch,
        max_pitch,
    }))
}

/// `Vec3.directionFromRotation(pitch, yaw)`, of the game's tables.
pub(crate) fn direction_from_rotation(pitch: f32, yaw: f32) -> Vec3 {
    let turned = -yaw * maths::RADIANS - std::f32::consts::PI;
    let c = maths::table_cos(turned);
    let s = maths::table_sin(turned);
    let cp = -maths::table_cos(-pitch * maths::RADIANS);
    let sp = maths::table_sin(-pitch * maths::RADIANS);
    [f64::from(s * cp), f64::from(sp), f64::from(c * cp)]
}

impl Timed for RandomLookAround {
    type State = ();

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[
            (MemoryKind::LookTarget, Status::Absent),
            (MemoryKind::GazeCooldownTicks, Status::Absent),
        ]
    }

    fn start(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        let range = self.max_pitch - self.min_pitch;
        let draws = &mut *cx.ai.tick.draws;
        let pitch = maths::clamp(draws.next_float() * range + self.min_pitch, -90.0, 90.0);
        let yaw = maths::wrap_degrees(
            cx.ai.tick.yaw + 2.0 * draws.next_float() * self.max_yaw - self.max_yaw,
        );
        let way = direction_from_rotation(pitch, yaw);
        let position = cx.ai.position();
        let eyes = [position[0], cx.ai.eye_y(), position[2]];
        let at = [eyes[0] + way[0], eyes[1] + way[1], eyes[2] + way[2]];
        cx.memories.set(
            MemoryKind::LookTarget,
            Value::Tracker(super::super::Tracker::Point(at)),
        );
        let cooldown = sample(cx.ai.tick.draws, self.interval);
        cx.memories
            .set(MemoryKind::GazeCooldownTicks, Value::Int(cooldown));
    }
}
