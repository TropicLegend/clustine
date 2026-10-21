//! Donkeys (`Donkey`, an `AbstractChestedHorse`; `docs/groundwork/n7-brief.md`, sections
//! 5.9 to 5.14): slower and lower-jumping than a horse, of random health alone; a chest
//! goes on a tamed one, and its inventory then has three rows of five. With a horse a
//! donkey makes a mule.

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
    kind: entity_types::DONKEY,
    max_temper: 100,
    rears: true,
    grazes: true,
    chested: true,
    grows: true,
    baby_scale: 0.5,
    food: Items::Tag("minecraft:horse_food"),
    behaviour: Behaviour::Horse,
    angry: |_| Some(sounds::ENTITY_DONKEY_ANGRY),
    eat: |_| Some(sounds::ENTITY_DONKEY_EAT),
    chest_sound: sounds::ENTITY_DONKEY_CHEST,
};

/// `AbstractChestedHorse.randomizeAttributes(r)`: its health alone, of two draws.
pub(crate) fn randomize_chested(mob: &mut Mob, kind: u16, r: &mut Random) {
    let health = equine::generate_max_health(r);
    equine::set_base(mob, kind, ids::MAX_HEALTH, health);
}

/// The donkey.
pub(crate) struct Donkey;

impl MobType for Donkey {
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
        let kind = entity_types::DONKEY as u16;
        randomize_chested(spawn.mob, kind, spawn.level);
        equine::clamp_health(spawn.mob, kind);
    }

    /// `Donkey.getBreedOffspring`: with a horse a mule, else a donkey, its stats about
    /// its parents'.
    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        let this_kind = entity_types::DONKEY as u16;
        let child_kind = if i32::from(breeding.partner_kind) == entity_types::HORSE {
            entity_types::MULE as u16
        } else {
            this_kind
        };
        breeding.child_kind = Some(child_kind);
        let mut child = equine::foal();
        equine::set_offspring_attributes(
            (breeding.this, this_kind),
            (breeding.partner, breeding.partner_kind),
            (&mut child, child_kind),
            breeding.draws,
        );
        Some(child)
    }

    fn can_mate(&self, this: Mate<'_>, partner: Mate<'_>) -> bool {
        equine::horse_or_donkey_mates(this, partner)
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn hurt_taken(&self, mob: &mut Mob, draws: &mut Random, moment: u32) {
        equine::hurt_taken(mob, entity_types::DONKEY as u16, draws, moment);
    }

    fn drop_equipment(&self, mob: &mut Mob) -> Vec<ItemStack> {
        equine::drop_equipment(mob)
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        equine::dimensions(mob, &EQUINE)
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_DONKEY_AMBIENT)
    }

    fn ambient_interval(&self) -> u32 {
        400
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_DONKEY_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_DONKEY_DEATH)
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
