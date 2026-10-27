//! The endermite (`Endermite`; `docs/groundwork/n9-plan.md`, sections 4.4 and 8.4; n9
//! Endermite): made by one ender pearl in twenty where monsters spawn
//! (`region/teleports.rs`), and by its egg.
//!
//! It floats, climbs out of powder snow, fights what hurts it (alerting other endermites)
//! and players it sees, wanders on land and looks about. Its body always faces where it
//! looks, and its yaw follows its body's turns, as the silverfish's (**inferred**: the
//! body control's setter). It makes no step sounds (`MovementEmission.EVENTS`). Unless it
//! is kept, it is gone after two minutes (`life` at 2400).
//!
//! Its life is its ticks lived (`lived`), each of which its `aiStep` counts, so nothing of
//! it is written as it counts (named: a kept endermite's ticks count too, and none is kept
//! in Clustine, which has no name tags).

use clustine_data::sounds;

use super::MobType;
use crate::mobs::Mob;
use crate::mobs::ai::Setup;
use crate::mobs::ai::goals::{
    ClimbOnTopOfPowderSnowGoal, FloatGoal, LookAtPlayerGoal, MeleeAttackGoal, RandomLookAroundGoal,
    RandomStrollGoal,
};
use crate::mobs::ai::sensing::Class;
use crate::mobs::ai::targets::{self, NearestTarget, Target};
use crate::mobs::body::Removed;
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::MobTick;
use crate::region::Random;

/// Whether the type is whole, so that `built()` gives it and an egg makes it.
pub(crate) const BUILT: bool = true;

/// `MAX_LIFE`.
pub(crate) const MAX_LIFE: u32 = 2400;

/// `Endermite.registerGoals`' targets: who hurt it, alerting endermites; players it sees.
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

/// The endermite.
pub(crate) struct Endermite;

impl MobType for Endermite {
    fn register_goals(&self, setup: &mut Setup) {
        setup.add_goal(1, FloatGoal);
        setup.add_goal(1, ClimbOnTopOfPowderSnowGoal);
        setup.add_goal(
            2,
            MeleeAttackGoal {
                speed: 1.0,
                following_even_if_not_seen: false,
            },
        );
        setup.add_goal(3, RandomStrollGoal::avoiding_water(1.0));
        setup.add_goal(7, LookAtPlayerGoal::new(Class::Player, 8.0));
        setup.add_goal(8, RandomLookAroundGoal);
        targets::register(setup, TARGETS);
    }

    /// `Endermite.tick`'s head: its body to its yaw.
    fn before_tick(&self, tick: &mut MobTick<'_>) {
        tick.mob.body.head.body_yaw = tick.yaw;
    }

    /// `Endermite.aiStep` after `Monster`'s: its life counted, and at two minutes, unless
    /// it is kept, gone (`discard`).
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        if !tick.mob.body.flags.persistent && tick.lived.saturating_add(1) >= MAX_LIFE {
            tick.out.removed = Some(Removed::Discarded);
        }
    }

    /// `setYBodyRot(rot)`'s `setYRot(rot)`: its yaw to where its body turned.
    fn after_tick(&self, tick: &mut MobTick<'_>) {
        tick.yaw = tick.mob.body.head.body_yaw;
    }

    fn plays_step_sounds(&self) -> bool {
        false
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_ENDERMITE_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_ENDERMITE_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_ENDERMITE_DEATH)
    }

    /// `xpReward` 3, with what its equipment adds.
    fn experience_reward(&self, _: u16, mob: &Mob, draws: &mut Random) -> i32 {
        super::illagers::experience(3, mob, draws)
    }
}
