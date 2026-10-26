//! The zombie (`Zombie`; `docs/groundwork/n7k-brief.md` 3.4.1): what its family shares
//! ([`super::zombie_family`]), its sounds, and its drowning into a drowned.

use clustine_data::{entity_types, sounds};
use clustine_world::BlockPos;

use super::zombie_family as family;
use super::{Breeding, Dimensions, MobType, Spawning};
use crate::mobs::Mob;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::MobTick;
use crate::region::Random;
use crate::shown::DataShown;

/// The zombie.
pub(crate) struct Zombie;

const KIND: u16 = entity_types::ZOMBIE as u16;

/// `Zombie.getConversionSound()`: the level event of a zombie turned drowned.
pub(crate) const CONVERTED: i32 = 1040;

impl MobType for Zombie {
    fn register_goals(&self, setup: &mut Setup) {
        family::register_goals(setup);
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        family::walk_target_value(ai, pos)
    }

    fn after_finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        family::finalize_spawn(KIND, spawn, family::zombie_equipment);
    }

    fn set_baby(&self, mob: &mut Mob, baby: bool) {
        family::set_baby(KIND, mob, baby);
    }

    /// The drowning's `after`: `handleAttributes(CONVERSION)` on the new mob, at the
    /// special multiplier of the world's easy, 0.
    fn finalize_conversion(&self, into: u16, new: &mut Mob, own: &mut Random) {
        family::handle_attributes(into, new, own, 0.0, super::SpawnReason::Conversion);
    }

    fn after_tick(&self, tick: &mut MobTick<'_>) {
        family::tick_drowning(tick, entity_types::DROWNED, CONVERTED);
    }

    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        family::from_template(KIND, mob, tag);
    }

    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        family::offspring_from_egg(breeding)
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        family::dimensions(mob, 0.775)
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        family::is_baby(mob)
    }

    fn experience_reward(&self, _: u16, mob: &Mob, draws: &mut Random) -> i32 {
        family::experience_reward(mob, draws)
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_ZOMBIE_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_ZOMBIE_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_ZOMBIE_DEATH)
    }

    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_ZOMBIE_STEP)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        family::shown(mob)
    }
}
