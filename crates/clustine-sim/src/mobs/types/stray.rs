//! The stray (`Stray`; `n7k-brief.md` 11.2): a skeleton ([`super::skeletons`]) of its own
//! sounds, which powder snow does not freeze (`#freeze_immune_entity_types`). Named: its
//! arrows' slowness and its spawn rule are N9's.

use clustine_data::sounds;
use clustine_world::BlockPos;

use super::skeletons;
use super::{MobType, Spawning};
use crate::mobs::Mob;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::DamageSource;

/// The stray.
pub(crate) struct Stray;

impl MobType for Stray {
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
        Some(sounds::ENTITY_STRAY_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_STRAY_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_STRAY_DEATH)
    }

    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_STRAY_STEP)
    }
}
