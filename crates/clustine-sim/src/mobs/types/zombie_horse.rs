//! Zombie horses (`ZombieHorse`; `docs/groundwork/n7-brief.md`, sections 5.11 and 5.17):
//! of a jump and a speed by formulas of their own, floating and tempted by a red
//! mushroom, its food; any use makes it persistent; a foal never grows up; it breeds
//! with nothing.
//!
//! Named differences: the zombie riding a natural one, its spear and the mob control it
//! gives come with N9's jockeys; the sun's burning and the armour that keeps it off are
//! N9's; the use that makes it persistent is only one its own `mobInteract` takes up
//! (a lead or a name tag on it does not).

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
    kind: entity_types::ZOMBIE_HORSE,
    max_temper: 100,
    rears: true,
    grazes: true,
    chested: false,
    grows: false,
    baby_scale: 0.7,
    food: Items::Tag("minecraft:zombie_horse_food"),
    behaviour: Behaviour::Zombie,
    angry: |_| Some(sounds::ENTITY_ZOMBIE_HORSE_ANGRY),
    eat: |_| Some(sounds::ENTITY_ZOMBIE_HORSE_EAT),
    chest_sound: sounds::ENTITY_DONKEY_CHEST,
};

/// `generateZombieHorseJumpStrength(r::nextDouble)`: `0.5 + a/15 + b/15 + c/15`.
pub(crate) fn generate_jump_strength(r: &mut Random) -> f64 {
    let mut jump = 0.5;
    for _ in 0..3 {
        jump += r.next_double() * 0.066_666_666_666_666_67;
    }
    jump
}

/// `generateZombieHorseSpeed(r::nextDouble)`: `(9 + a + b + c) / 42.16`.
pub(crate) fn generate_speed(r: &mut Random) -> f64 {
    let mut speed = 9.0;
    for _ in 0..3 {
        speed += r.next_double() * 1.0;
    }
    speed / 42.159_999_847_412_11
}

/// The zombie horse.
pub(crate) struct ZombieHorse;

impl MobType for ZombieHorse {
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

    /// `ZombieHorse.interact`'s persistence, then its `mobInteract`: the horse's without
    /// the golden dandelion's test.
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        used.mob.body.flags.persistent = true;
        equine::mob_interact(used, &EQUINE, false)
    }

    /// `ZombieHorse.randomizeAttributes`: its jump, then its speed.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let kind = entity_types::ZOMBIE_HORSE as u16;
        let jump = generate_jump_strength(spawn.level);
        equine::set_base(spawn.mob, kind, ids::JUMP_STRENGTH, jump);
        let speed = generate_speed(spawn.level);
        equine::set_base(spawn.mob, kind, ids::MOVEMENT_SPEED, speed);
    }

    /// `ZombieHorse.getBreedOffspring`: a zombie foal, of nothing inherited.
    fn breed_offspring(&self, _: &mut Breeding<'_>) -> Option<Mob> {
        Some(equine::foal())
    }

    /// `canFallInLove()` false: it breeds with nothing.
    fn can_mate(&self, _: Mate<'_>, _: Mate<'_>) -> bool {
        false
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn hurt_taken(&self, mob: &mut Mob, draws: &mut Random, moment: u32) {
        equine::hurt_taken(mob, entity_types::ZOMBIE_HORSE as u16, draws, moment);
    }

    fn drop_equipment(&self, mob: &mut Mob) -> Vec<ItemStack> {
        equine::drop_equipment(mob)
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        equine::dimensions(mob, &EQUINE)
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_ZOMBIE_HORSE_AMBIENT)
    }

    fn ambient_interval(&self) -> u32 {
        400
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_ZOMBIE_HORSE_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_ZOMBIE_HORSE_DEATH)
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
