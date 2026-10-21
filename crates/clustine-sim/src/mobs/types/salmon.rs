//! The salmon (`Salmon` and `Salmon$Variant`; `docs/groundwork/n7-brief.md`, section
//! 6.6.6): a schooling fish of schools of five, small, medium or large, its size drawn
//! as it is made and its box scaled by it.

use clustine_data::{EntityTypeInfo, entity_types, sounds};

use super::fish::{self, Species};
use super::{Dimensions, Interaction, MobType, Spawning, Used, water};
use crate::living::Travel;
use crate::mobs::Mob;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::MobTick;
use crate::region::Random;
use crate::shown::DataShown;

/// The sizes' ids (`Salmon.Variant`).
pub(crate) const SMALL: i32 = 0;
pub(crate) const MEDIUM: i32 = 1;
pub(crate) const LARGE: i32 = 2;

/// The salmon.
pub(crate) struct Salmon;

fn kind() -> u16 {
    entity_types::SALMON as u16
}

/// `Salmon.Variant.BY_ID`: an id out of range is the nearest size.
pub(crate) fn size(variant: i32) -> i32 {
    variant.clamp(SMALL, LARGE)
}

/// `boundingBoxScale` of a size.
pub(crate) fn scale(variant: i32) -> f32 {
    match size(variant) {
        SMALL => 0.5,
        MEDIUM => 1.0,
        _ => 1.5,
    }
}

/// `finalizeSpawn`'s size, before the parent's: a weighted list of small 30, medium 50
/// and large 15, one `nextInt(95)` of its own random.
pub(crate) fn random_size(own: &mut Random) -> i32 {
    let drawn = own.next_int(95);
    if drawn < 30 {
        SMALL
    } else if drawn < 80 {
        MEDIUM
    } else {
        LARGE
    }
}

impl MobType for Salmon {
    fn register_goals(&self, setup: &mut Setup) {
        fish::register_goals(setup, Species::Salmon);
    }

    fn construct(&self, mob: &mut Mob, own: &mut Random) {
        fish::construct(mob, Species::Salmon, own);
    }

    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let variant = random_size(spawn.own);
        fish::update(kind(), spawn.mob, |fish| fish.variant = variant);
    }

    fn after_base_tick(&self, tick: &mut MobTick<'_>, air: i32) {
        water::handle_air_supply(tick, air);
    }

    fn before_ai_step(&self, tick: &mut MobTick<'_>) {
        fish::flop(tick, sounds::ENTITY_SALMON_FLOP);
    }

    fn after_tick(&self, tick: &mut MobTick<'_>) {
        fish::census(tick);
    }

    fn travel(&self, tick: &MobTick<'_>) -> Travel {
        fish::travel(tick)
    }

    fn tick_move_control(&self, ai: &mut Ai<'_, '_>) {
        fish::tick_move_control(ai);
    }

    fn pushed_by_fluid(&self) -> bool {
        false
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        fish::mob_interact(used)
    }

    fn remove_when_far_away(&self, kind: u16, mob: &Mob, _: u32, _: f64) -> bool {
        fish::remove_when_far_away(kind, mob)
    }

    fn can_be_leashed(&self, _: u16, _: &Mob) -> bool {
        false
    }

    /// The type's size scaled by its own.
    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        let info = EntityTypeInfo::of(entity_types::SALMON);
        let factor = scale(fish::read(kind(), &mob.state).variant);
        Some(water::scaled(
            info.width,
            info.height,
            info.eye_height,
            factor,
        ))
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SALMON_AMBIENT)
    }

    fn ambient_interval(&self) -> u32 {
        120
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_SALMON_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SALMON_DEATH)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        fish::shown(kind(), mob, true)
    }
}
