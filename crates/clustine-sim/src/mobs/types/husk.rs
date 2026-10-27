//! The husk (`Husk`; `docs/groundwork/n7k-brief.md` 3.4.2): a zombie of its own sounds
//! that does not burn in daylight (it is not in `#burn_in_daylight`), draws its loot's
//! chance twice at its making, and drowns into a zombie.
//!
//! Named differences: its hunger on a hit is N9's (an attack); a natural spawn's camel
//! husk and parched are N9's (an egg's husk draws nothing for them).

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

/// The husk.
pub(crate) struct Husk;

const KIND: u16 = entity_types::HUSK as u16;

/// `Husk.getConversionSound()`: the level event of a husk turned zombie.
pub(crate) const CONVERTED: i32 = 1041;

/// `Husk.finalizeSpawn` after `Zombie`'s: unless it is a conversion, its loot's chance
/// drawn again; the group made a `HuskGroupData`, whose camel husk only a natural spawn
/// tries (N9's), drawing nothing for an egg's or a structure's.
fn finalize_spawn(spawn: &mut Spawning<'_>) {
    family::finalize_spawn(KIND, spawn, family::zombie_equipment);
    if spawn.reason != super::SpawnReason::Conversion {
        let special = spawn.difficulty.1;
        spawn.mob.body.flags.picks_up_loot = spawn.level.next_float() < 0.55 * special;
    }
}

impl MobType for Husk {
    /// `Zombie.doHurtTarget`'s fire, then an empty-handed husk's hunger for
    /// `140 · (int) eff` ticks, given even for none (finding 47).
    fn hit_extras(&self, ai: &Ai<'_, '_>) -> Vec<clustine_world::HitExtra> {
        let mut extras = family::hit_extras(ai);
        if ai.tick.mob.body.equipment.slots[usize::from(crate::living::slots::MAIN_HAND)].is_none()
        {
            extras.push(clustine_world::HitExtra::HuskHunger);
        }
        extras
    }

    fn register_goals(&self, setup: &mut Setup) {
        family::register_goals(setup);
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        family::walk_target_value(ai, pos)
    }

    fn after_finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        finalize_spawn(spawn);
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
        family::tick_drowning(tick, entity_types::ZOMBIE, CONVERTED);
    }

    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        family::from_template(KIND, mob, tag);
    }

    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        family::offspring_from_egg(breeding)
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        family::dimensions(mob, 0.825)
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        family::is_baby(mob)
    }

    fn experience_reward(&self, _: u16, mob: &Mob, draws: &mut Random) -> i32 {
        family::experience_reward(mob, draws)
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_HUSK_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_HUSK_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_HUSK_DEATH)
    }

    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_HUSK_STEP)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        family::shown(mob)
    }
}
