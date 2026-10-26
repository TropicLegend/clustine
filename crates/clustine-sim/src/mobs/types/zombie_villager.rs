//! The zombie villager (`ZombieVillager`; `docs/groundwork/n7k-brief.md` 3.4.3): a zombie
//! with a villager's data, whose profession its construction draws, whose type the biome
//! at its feet settles when it is made unless a template's data settled it, and which
//! does not drown into anything.
//!
//! Named differences: its cure. A golden apple cures only a zombie villager that is
//! weak, and nothing in Clustine gives weakness yet, so the apple does what it does on
//! one that is not (nothing, the game's `CONSUME`); a template's conversion under way
//! counts nothing down, and no zombie villager becomes a villager. Its offers and
//! gossips are not kept (none is made with any before trades, N10).

use clustine_data::villagers::{PROFESSION_NAMES, TYPE_NAMES, professions, types};
use clustine_data::{entity_types, items, sounds};
use clustine_world::BlockPos;

use super::zombie_family::{self as family, VillagerState};
use super::{Breeding, Dimensions, Interaction, MobType, Spawning, Used, template};
use crate::mobs::Mob;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::region::Random;
use crate::shown::DataShown;

/// The zombie villager.
pub(crate) struct ZombieVillager;

const KIND: u16 = entity_types::ZOMBIE_VILLAGER as u16;

/// The synched indices of `DATA_CONVERTING_ID`, `DATA_VILLAGER_DATA` and
/// `DATA_VILLAGER_DATA_FINALIZED`.
const CONVERTING_INDEX: u8 = 19;
const VILLAGER_DATA_INDEX: u8 = 20;
const FINALISED_INDEX: u8 = 21;

/// `VillagerProfession`'s registry's size, which `getRandom` draws over.
const PROFESSIONS: u32 = 15;

/// The villager's state of a zombie villager: the one its construction made, or, for a
/// record without one, `Villager.createDefaultVillagerData()`'s (plains, none, level 1).
pub(crate) fn villager(mob: &Mob) -> VillagerState {
    family::read(mob).villager.unwrap_or(VillagerState {
        kind: types::PLAINS,
        profession: professions::NONE,
        level: 1,
        finalised: false,
        drawn_profession: professions::NONE,
        conversion: -1,
        xp: 0,
    })
}

/// Changes the villager's state of a zombie villager.
fn update_villager(mob: &mut Mob, change: impl FnOnce(&mut VillagerState)) {
    let mut own = villager(mob);
    change(&mut own);
    family::update(mob, |state| state.villager = Some(own));
}

/// The id of the entry `name` among `names`, if it is one.
fn id_of(names: &[&str], name: &str) -> Option<i32> {
    names
        .iter()
        .position(|known| *known == name)
        .and_then(|at| i32::try_from(at).ok())
}

impl MobType for ZombieVillager {
    /// `initializeZombieVillagerData(random)`: the default data with a profession drawn
    /// from the registry's (`getRandom`, one `nextInt(15)`), the synched value's default.
    fn define_synched_data(&self, mob: &mut Mob, own: &mut Random) {
        let profession = own.next_int(PROFESSIONS) as i32;
        update_villager(mob, |villager| {
            villager.profession = profession;
            villager.drawn_profession = profession;
        });
    }

    fn register_goals(&self, setup: &mut Setup) {
        family::register_goals(setup);
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        family::walk_target_value(ai, pos)
    }

    /// `finalizeVillagerType(level, blockPosition())` before `Zombie`'s: the type of the
    /// biome at its block, unless it is finalised already; finalised then.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let biome = spawn.biome;
        update_villager(spawn.mob, |villager| {
            if !villager.finalised {
                villager.kind =
                    biome.map_or(types::PLAINS, clustine_data::villagers::type_by_biome_name);
                villager.finalised = true;
            }
        });
    }

    fn after_finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        family::finalize_spawn(KIND, spawn, family::zombie_equipment);
    }

    fn set_baby(&self, mob: &mut Mob, baby: bool) {
        family::set_baby(KIND, mob, baby);
    }

    /// What `ZombieVillager.readAdditionalSaveData` reads of a template's tag after the
    /// zombie's: its `VillagerData`, which marks it finalised where it is there, as
    /// `VillagerDataFinalized` does; a `ConversionTime` other than -1 (the cure's count,
    /// named: it counts nothing down); its `Xp`.
    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        family::from_template(KIND, mob, tag);
        let data = template::compound(tag, "VillagerData");
        let finalised = template::boolean(tag, "VillagerDataFinalized").unwrap_or(false);
        let conversion = template::int(tag, "ConversionTime").unwrap_or(-1);
        let xp = template::int(tag, "Xp").unwrap_or(0);
        update_villager(mob, |villager| {
            if finalised || data.is_some() {
                villager.finalised = true;
                if let Some(data) = &data {
                    // `VillagerData.CODEC`'s fields, each at its default where it is
                    // missing (**Inferred**: plains, none, level 1).
                    villager.kind = template::text(data, "type")
                        .and_then(|name| id_of(&TYPE_NAMES, name))
                        .unwrap_or(types::PLAINS);
                    villager.profession = template::text(data, "profession")
                        .and_then(|name| id_of(&PROFESSION_NAMES, name))
                        .unwrap_or(professions::NONE);
                    villager.level = template::int(data, "level").unwrap_or(1);
                }
            }
            villager.conversion = conversion;
            villager.xp = xp;
        });
    }

    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        family::offspring_from_egg(breeding)
    }

    /// `ZombieVillager.mobInteract`: a golden apple on one that is weak would start its
    /// cure (named, as nothing is weak); on one that is not it is taken for nothing
    /// (`CONSUME`). Anything else is the zombie's: nothing.
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        if !used
            .held
            .is_some_and(|stack| stack.item == items::GOLDEN_APPLE)
        {
            return Interaction::Pass;
        }
        Interaction::Consume
    }

    /// `removeWhenFarAway`: only one that is not curing and has no experience.
    fn remove_when_far_away(&self, _: u16, mob: &Mob, _: u32, _: f64) -> bool {
        let villager = villager(mob);
        villager.conversion == -1 && villager.xp == 0
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        family::dimensions(mob, 0.67)
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        family::is_baby(mob)
    }

    /// `getVoicePitch()`: a baby's about 2.
    fn baby_voice_pitch(&self) -> f32 {
        2.0
    }

    fn experience_reward(&self, _: u16, mob: &Mob, draws: &mut Random) -> i32 {
        family::experience_reward(mob, draws)
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_ZOMBIE_VILLAGER_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_ZOMBIE_VILLAGER_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_ZOMBIE_VILLAGER_DEATH)
    }

    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_ZOMBIE_VILLAGER_STEP)
    }

    /// The zombie's values, then its curing (19), its villager's data (20; at its default
    /// while it is plains, of the profession its construction drew, at level 1, which a
    /// client being paired is not sent) and whether that is finalised (21).
    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let villager = villager(mob);
        let converting = villager.conversion != -1;
        let mut values = family::shown(mob);
        values.push((
            CONVERTING_INDEX,
            DataShown::Boolean(converting),
            !converting,
        ));
        values.push((
            VILLAGER_DATA_INDEX,
            DataShown::VillagerData {
                kind: villager.kind,
                profession: villager.profession,
                level: villager.level,
            },
            villager.kind == types::PLAINS
                && villager.profession == villager.drawn_profession
                && villager.level == 1,
        ));
        values.push((
            FINALISED_INDEX,
            DataShown::Boolean(villager.finalised),
            !villager.finalised,
        ));
        values
    }
}
