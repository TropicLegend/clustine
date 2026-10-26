//! The parched (`Parched`; `n7k-brief.md` 11.2): a skeleton ([`super::skeletons`]) of 16
//! health and its own sounds, which does not burn in daylight (not of
//! `#burn_in_daylight`) and freezes in powder snow as any mob. Named: its arrows'
//! weakness, its immunity to weakness and its ride on a camel husk (a husk's natural
//! spawn) are N9's.

use clustine_data::sounds;
use clustine_world::BlockPos;

use super::skeletons;
use super::{MobType, Spawning};
use crate::mobs::Mob;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::DamageSource;

/// The parched.
pub(crate) struct Parched;

impl MobType for Parched {
    fn register_goals(&self, setup: &mut Setup) {
        skeletons::register_goals(setup);
    }

    fn after_finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        skeletons::finalize_spawn(spawn);
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        skeletons::walk_target_value(ai, pos)
    }

    fn avoids_sun(&self, mob: &Mob) -> bool {
        skeletons::avoids_sun(mob)
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_PARCHED_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_PARCHED_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_PARCHED_DEATH)
    }

    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_PARCHED_STEP)
    }
}
