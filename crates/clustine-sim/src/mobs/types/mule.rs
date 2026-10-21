//! Mules (`Mule`, an `AbstractChestedHorse`; `docs/groundwork/n7-brief.md`, sections 5.9
//! to 5.14): a horse's and a donkey's foal, which breeds with nothing; an egg used on
//! one makes a mule foal, of the type's stats. A chest goes on a tamed one, as on a
//! donkey.

use clustine_data::{entity_types, sounds};
use clustine_world::{BlockPos, ItemStack};

use super::donkey::randomize_chested;
use super::equine::{self, Behaviour, Equine};
use super::horse::common;
use super::{Breeding, Dimensions, Interaction, Mate, MobType, Spawning, Used, animal};
use crate::mobs::Mob;
use crate::mobs::ai::goals::Items;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::MobTick;
use crate::region::Random;
use crate::shown::DataShown;

pub(crate) const EQUINE: Equine = Equine {
    kind: entity_types::MULE,
    max_temper: 100,
    rears: true,
    grazes: true,
    chested: true,
    grows: true,
    baby_scale: 0.5,
    food: Items::Tag("minecraft:horse_food"),
    behaviour: Behaviour::Horse,
    angry: |_| Some(sounds::ENTITY_MULE_ANGRY),
    eat: |_| Some(sounds::ENTITY_MULE_EAT),
    chest_sound: sounds::ENTITY_MULE_CHEST,
};

/// The mule.
pub(crate) struct Mule;

impl MobType for Mule {
    fn register_goals(&self, setup: &mut Setup) {
        equine::register_goals(setup, &EQUINE);
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        common::walk_target_value(ai, pos)
    }

    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::custom_server_ai_step(tick);
    }

    fn before_ai_step(&self, tick: &mut MobTick<'_>) {
        equine::before_ai_step(tick);
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        common::after_ai_step(tick, &EQUINE);
    }

    fn immobile(&self, tick: &MobTick<'_>) -> bool {
        equine::immobile(tick, &EQUINE)
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        equine::mob_interact(used, &EQUINE, true)
    }

    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let kind = entity_types::MULE as u16;
        randomize_chested(spawn.mob, kind, spawn.level);
        equine::clamp_health(spawn.mob, kind);
    }

    /// `Mule.getBreedOffspring`: a mule, of nothing inherited.
    fn breed_offspring(&self, _: &mut Breeding<'_>) -> Option<Mob> {
        Some(equine::foal())
    }

    /// `AbstractHorse.canMate`: never.
    fn can_mate(&self, _: Mate<'_>, _: Mate<'_>) -> bool {
        false
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn hurt_taken(&self, mob: &mut Mob, draws: &mut Random, moment: u32) {
        equine::hurt_taken(mob, entity_types::MULE as u16, draws, moment);
    }

    fn drop_equipment(&self, mob: &mut Mob) -> Vec<ItemStack> {
        equine::drop_equipment(mob)
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        equine::dimensions(mob, &EQUINE)
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_MULE_AMBIENT)
    }

    fn ambient_interval(&self) -> u32 {
        400
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_MULE_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_MULE_DEATH)
    }

    fn sound_volume(&self) -> f32 {
        0.8
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        equine::shown_chested(mob)
    }
}
