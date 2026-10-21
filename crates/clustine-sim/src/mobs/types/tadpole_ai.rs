//! The tadpole's brain (`TadpoleAi`; `docs/groundwork/n7-brief.md`, section 7.5.8),
//! beside its fish's goals: in its core it panics when hurt, looks, swims where its
//! brain says and lets its temptation cool; idle it looks at players, follows a player
//! holding slime balls and swims about. Its goals (a panic of their own, fleeing
//! players, swimming about) run first in each step, and both move it (the brief's 7.0.2
//! point 4).
//!
//! What reads across a line (`docs/groundwork/n7j-plan.md`, section 3): its sensors see
//! the region's land, copies and faces (item 1); a tempting player or a look target only
//! a copy shows is followed as the shared behaviours do, and one beyond the copies is
//! kept and not followed that tick (item 2, the plan's section 8 item 8).

use crate::mobs::brain::behaviours::Speed;
use crate::mobs::brain::behaviours::cooldowns::count_down;
use crate::mobs::brain::behaviours::looking::{LookAt, look_sometimes};
use crate::mobs::brain::behaviours::panic::panic;
use crate::mobs::brain::behaviours::social::FollowTemptation;
use crate::mobs::brain::behaviours::walking::{
    StrollTo, look_sink, sink, stroll_to, walk_to_look_target,
};
use crate::mobs::brain::sensors::{
    FROG_TEMPTATIONS, HURT_BY, NEAREST_LIVING_ENTITIES, NEAREST_PLAYERS,
};
use crate::mobs::brain::{
    Activity, ActivityData, Brain, Gate, MemoryKind, Order, Provider, Running, Status,
};
use crate::mobs::tick::MobTick;

use super::axolotl_ai::{in_water, stand_still};

/// `BRAIN_PROVIDER`: its four sensors, and `TadpoleAi.getActivities`.
pub(crate) static PROVIDER: Provider = Provider {
    memories: &[],
    sensors: &[
        &NEAREST_LIVING_ENTITIES,
        &NEAREST_PLAYERS,
        &HURT_BY,
        &FROG_TEMPTATIONS,
    ],
    activities,
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// The activities, in `getActivities`' order: core and idle.
fn activities() -> Vec<ActivityData> {
    vec![core(), idle()]
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
        ],
    )
}

/// `initIdleActivity`, with its own priorities.
fn idle() -> ActivityData {
    let wander = Gate::new(
        vec![(MemoryKind::WalkTarget, Status::Absent)],
        Vec::new(),
        Order::Ordered,
        Running::TryAll,
        vec![
            (stroll_to(0.5, StrollTo::Swim), 2),
            (walk_to_look_target(0.5, 3), 3),
            (stand_still(in_water), 5),
        ],
    );
    ActivityData {
        activity: Activity::Idle,
        behaviours: vec![
            (0, look_sometimes(LookAt::Player, 6.0, (30, 60))),
            (1, FollowTemptation::new(Speed::Fixed(1.25)).boxed()),
            (2, Box::new(wander)),
        ],
        conditions: Vec::new(),
        erase: Vec::new(),
    }
}

/// `TadpoleAi.updateActivity(tadpole)`: idle, the one activity beside its core.
pub(crate) fn update_activity(brain: &mut Brain) {
    brain.set_active_to_first_valid(&[Activity::Idle]);
}

/// `customServerAiStep`'s brain: its tick and its activity.
pub(crate) fn step(tick: &mut MobTick<'_>) {
    crate::mobs::brain::run(tick, |brain, _| update_activity(brain));
}
