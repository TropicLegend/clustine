//! Horses (`Horse`; `docs/groundwork/n7-brief.md`, sections 5.9 to 5.13): of seven coats
//! and five markings, a herd sharing its coat; random health, speed and jump at spawn;
//! a foal of two horses takes each parent's coat and markings or a new one, and its
//! stats about its parents'; with a donkey a horse makes a mule. It wears a saddle and
//! horse armour.

use clustine_data::{attributes as ids, entity_types, sounds};
use clustine_world::BlockPos;

use super::equine::{self, Behaviour, Equine};
use super::{Breeding, Dimensions, Interaction, Mate, MobType, Spawning, Used, animal};
use crate::mobs::Mob;
use crate::mobs::ai::goals::Items;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::MobTick;
use crate::region::Random;
use crate::shown::DataShown;
use clustine_world::ItemStack;

/// `Horse.DATA_ID_TYPE_VARIANT`.
const VARIANT_INDEX: u8 = 19;

/// How many coats (`Variant`: white, creamy, chestnut, brown, black, gray, dark brown) and
/// markings (`Markings`: none, white, white field, white dots, black dots) there are.
pub(crate) const COATS: u32 = 7;
pub(crate) const MARKINGS: u32 = 5;

pub(crate) const EQUINE: Equine = Equine {
    kind: entity_types::HORSE,
    max_temper: 100,
    rears: true,
    grazes: true,
    chested: false,
    grows: true,
    baby_scale: 0.7,
    food: Items::Tag("minecraft:horse_food"),
    behaviour: Behaviour::Horse,
    angry: |baby| {
        Some(if baby {
            sounds::ENTITY_BABY_HORSE_ANGRY
        } else {
            sounds::ENTITY_HORSE_ANGRY
        })
    },
    eat: |baby| {
        Some(if baby {
            sounds::ENTITY_BABY_HORSE_EAT
        } else {
            sounds::ENTITY_HORSE_EAT
        })
    },
    chest_sound: sounds::ENTITY_DONKEY_CHEST,
};

/// The type variant of a coat and markings (`setVariantAndMarkings`).
pub(crate) fn type_variant(coat: i32, markings: i32) -> i32 {
    (coat & 255) | ((markings << 8) & 0xFF00)
}

/// A coat and markings of a type variant (both `byId`, wrapping).
pub(crate) fn coat_and_markings(variant: i32) -> (i32, i32) {
    (
        (variant & 255).rem_euclid(COATS as i32),
        ((variant & 0xFF00) >> 8).rem_euclid(MARKINGS as i32),
    )
}

/// `Horse.randomizeAttributes(r)`: health, then speed, then jump.
pub(crate) fn randomize(mob: &mut Mob, r: &mut Random) {
    let kind = entity_types::HORSE as u16;
    let health = equine::generate_max_health(r);
    equine::set_base(mob, kind, ids::MAX_HEALTH, health);
    let speed = equine::generate_speed(r);
    equine::set_base(mob, kind, ids::MOVEMENT_SPEED, speed);
    let jump = equine::generate_jump_strength(r);
    equine::set_base(mob, kind, ids::JUMP_STRENGTH, jump);
}

/// The horse.
pub(crate) struct Horse;

/// What every horse of the family's module shares of `MobType` but its goals, its use,
/// its spawn, its breeding and what it shows: each module calls these with its kind.
pub(crate) mod common {
    use super::*;

    pub(crate) fn after_ai_step(tick: &mut MobTick<'_>, equine: &Equine) {
        animal::after_ai_step_growing(tick, equine.grows);
        equine::ai_step(tick, equine);
        equine::tick_end(tick);
    }

    pub(crate) fn walk_target_value(ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        animal::walk_target_value(ai, pos, clustine_data::blocks::GRASS_BLOCK)
    }
}

impl MobType for Horse {
    /// `AbstractHorse.getControllingPassenger`: a player first on a saddled one steers it,
    /// whatever they hold.
    fn steered_by(&self, mob: &Mob, _: [Option<clustine_world::ItemStack>; 2]) -> bool {
        equine::riding::steered_by_rider(mob)
    }

    fn tick_ridden(&self, tick: &mut MobTick<'_>, yaw: f32, pitch: f32) {
        equine::riding::tick_ridden(tick, yaw, pitch);
    }

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

    /// `Horse.finalizeSpawn`: a coat (a herd's, or one drawn), markings drawn, then
    /// `AbstractHorse`'s random stats, all of the level's random.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let coat = spawn.level.next_int(COATS) as i32;
        let markings = spawn.level.next_int(MARKINGS) as i32;
        equine::update(spawn.mob, |horse| {
            horse.variant = type_variant(coat, markings);
        });
        randomize(spawn.mob, spawn.level);
        equine::clamp_health(spawn.mob, entity_types::HORSE as u16);
    }

    /// `Horse.getBreedOffspring`: with a donkey a mule, its stats alone inherited; with a
    /// horse a foal taking a coat (four in nine this parent's, four the partner's, one a
    /// new one) and markings (two in five, two, one), then its stats.
    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        let this_kind = entity_types::HORSE as u16;
        let partner_kind = breeding.partner_kind;
        if i32::from(partner_kind) == entity_types::DONKEY {
            let mule = entity_types::MULE as u16;
            breeding.child_kind = Some(mule);
            let mut child = equine::foal();
            equine::set_offspring_attributes(
                (breeding.this, this_kind),
                (breeding.partner, partner_kind),
                (&mut child, mule),
                breeding.draws,
            );
            return Some(child);
        }
        let this = equine::horse_of(breeding.this);
        let partner = equine::horse_of(breeding.partner);
        let (this_coat, this_markings) = coat_and_markings(this.variant);
        let (partner_coat, partner_markings) = coat_and_markings(partner.variant);
        let pick = breeding.draws.next_int(9);
        let coat = if pick < 4 {
            this_coat
        } else if pick < 8 {
            partner_coat
        } else {
            breeding.draws.next_int(COATS) as i32
        };
        let pick = breeding.draws.next_int(5);
        let markings = if pick < 2 {
            this_markings
        } else if pick < 4 {
            partner_markings
        } else {
            breeding.draws.next_int(MARKINGS) as i32
        };
        let mut child = equine::foal();
        equine::update(&mut child, |horse| {
            horse.variant = type_variant(coat, markings);
        });
        equine::set_offspring_attributes(
            (breeding.this, this_kind),
            (breeding.partner, partner_kind),
            (&mut child, this_kind),
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
        equine::hurt_taken(mob, entity_types::HORSE as u16, draws, moment);
    }

    fn drop_equipment(&self, mob: &mut Mob) -> Vec<ItemStack> {
        equine::drop_equipment(mob)
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        equine::dimensions(mob, &EQUINE)
    }

    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        Some(if animal::is_baby(mob) {
            sounds::ENTITY_BABY_HORSE_AMBIENT
        } else {
            sounds::ENTITY_HORSE_AMBIENT
        })
    }

    fn ambient_interval(&self) -> u32 {
        400
    }

    fn hurt_sound(&self, mob: &Mob, _: &DamageSource) -> Option<i32> {
        Some(if animal::is_baby(mob) {
            sounds::ENTITY_BABY_HORSE_HURT
        } else {
            sounds::ENTITY_HORSE_HURT
        })
    }

    fn death_sound(&self, mob: &Mob) -> Option<i32> {
        Some(if animal::is_baby(mob) {
            sounds::ENTITY_BABY_HORSE_DEATH
        } else {
            sounds::ENTITY_HORSE_DEATH
        })
    }

    fn sound_volume(&self) -> f32 {
        0.8
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let variant = equine::horse_of(mob).variant;
        let mut values = equine::shown(mob);
        values.push((VARIANT_INDEX, DataShown::Int(variant), variant == 0));
        values
    }
}
