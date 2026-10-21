//! Skeleton horses (`SkeletonHorse`; `docs/groundwork/n7-brief.md`, section 5.17): of
//! random jump alone, with no behaviour goals (no panic, no temptation, no floating); a
//! wild one passes a player's use; a foal never grows up; it breeds with nothing.
//!
//! Named differences: the trap (`SkeletonTrap`, its goal and its riders) comes from a
//! thunderstorm's lightning, which waits for weather and monsters (N9), so no skeleton
//! horse is a trap; its water sounds, ambient and swimming, are not chosen in water (its
//! sounds are read without its fluids).

use clustine_data::{attributes as ids, entity_types, sounds};
use clustine_world::{BlockPos, ItemStack};

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
    kind: entity_types::SKELETON_HORSE,
    max_temper: 100,
    rears: true,
    grazes: true,
    chested: false,
    grows: false,
    baby_scale: 0.7,
    food: Items::Tag("minecraft:horse_food"),
    behaviour: Behaviour::None,
    angry: |_| None,
    eat: |_| None,
    chest_sound: sounds::ENTITY_DONKEY_CHEST,
};

/// The skeleton horse.
pub(crate) struct SkeletonHorse;

impl MobType for SkeletonHorse {
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

    /// `SkeletonHorse.mobInteract`: a wild one passes; a tamed one is `AbstractHorse`'s.
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        if !equine::is_tamed(used.mob) {
            return Interaction::Pass;
        }
        equine::abstract_mob_interact(used, &EQUINE)
    }

    /// `SkeletonHorse.randomizeAttributes`: its jump alone.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let kind = entity_types::SKELETON_HORSE as u16;
        let jump = equine::generate_jump_strength(spawn.level);
        equine::set_base(spawn.mob, kind, ids::JUMP_STRENGTH, jump);
    }

    /// `SkeletonHorse.getBreedOffspring`: a skeleton foal, of nothing inherited.
    fn breed_offspring(&self, _: &mut Breeding<'_>) -> Option<Mob> {
        Some(equine::foal())
    }

    fn can_mate(&self, _: Mate<'_>, _: Mate<'_>) -> bool {
        false
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn hurt_taken(&self, mob: &mut Mob, draws: &mut Random, moment: u32) {
        equine::hurt_taken(mob, entity_types::SKELETON_HORSE as u16, draws, moment);
    }

    fn drop_equipment(&self, mob: &mut Mob) -> Vec<ItemStack> {
        equine::drop_equipment(mob)
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        equine::dimensions(mob, &EQUINE)
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SKELETON_HORSE_AMBIENT)
    }

    fn ambient_interval(&self) -> u32 {
        400
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_SKELETON_HORSE_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SKELETON_HORSE_DEATH)
    }

    fn sound_volume(&self) -> f32 {
        0.8
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        equine::shown(mob)
    }
}
