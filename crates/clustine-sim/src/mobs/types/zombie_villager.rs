//! The zombie villager (`ZombieVillager`; `docs/groundwork/n7k-brief.md` 3.4.3): a zombie
//! with a villager's data, whose profession its construction draws, whose type the biome
//! at its feet settles when it is made unless a template's data settled it, and which
//! does not drown into anything.
//!
//! Its cure (part T2; `docs/groundwork/n10-brief.md`, section 6): a golden apple on one
//! that is weak starts it, its countdown runs each tick (faster beside iron bars and
//! beds), and at its end it becomes the villager it was, with its data, gossips, offers
//! and experience (`region/trading.rs`'s `cure_finished`, in the conversion).
//!
//! Named differences: a template's `Offers`, `Gossips` and `ConversionPlayer` are not
//! read; a villager never becomes a zombie villager (only on normal and hard, and
//! Clustine is easy), so its offers and gossips are those a template gave, none.

use clustine_data::villagers::{PROFESSION_NAMES, TYPE_NAMES, professions, types};
use clustine_data::{entity_types, items, sounds};
use clustine_world::BlockPos;

use super::zombie_family::{self as family, VillagerState};
use super::{Breeding, Did, Dimensions, Interaction, MobType, Spawning, Used, template};
use crate::mobs::Mob;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::effects::{self, Effect};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::{Local, MobTick};
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
        offers: None,
        gossips: Default::default(),
        conversion_starter: None,
    })
}

/// The entity event of a cure begun (the client's cure sound).
const CURE_STARTED: u8 = 16;

/// The level event of a cure's end (`ZOMBIE_VILLAGER_CONVERTED`).
const CURED_EVENT: i32 = 1027;

/// The most blocks of `#speeds_up_zombie_villager_curing` a countdown's look counts.
const MOST_CURING_BLOCKS: i32 = 14;

/// `startConverting(starter, time)` (n7 ZombieVillager 365–387) at the moment `now`: the
/// player who began it kept, its countdown set, its weakness taken off and strength
/// given for as long (at `min(difficulty − 1, 0)`, nought on easy).
fn start_converting(mob: &mut Mob, starter: Option<u128>, time: i32, now: u32) {
    update_villager(mob, |villager| {
        villager.conversion_starter = starter;
        villager.conversion = time;
    });
    effects::remove(mob, KIND, Effect::WEAKNESS);
    effects::add(mob, KIND, Effect::STRENGTH, 0, time, now);
}

/// `getConversionProgress()` (n7 ZombieVillager 461–554): one, and where its own
/// `nextFloat() < 0.01` a look at the cube of eight blocks a side below its truncated
/// position for up to 14 blocks of `#speeds_up_zombie_villager_curing`, each adding one
/// on its own `nextFloat() < 0.3`.
fn conversion_progress(tick: &mut MobTick<'_>) -> i32 {
    let mut progress = 1;
    if tick.draws.next_float() >= 0.01 {
        return progress;
    }
    let [x, y, z] = tick.living.body.position.map(|axis| axis as i32);
    let mut found = 0;
    let mut i = x - 4;
    while i < x + 4 && found < MOST_CURING_BLOCKS {
        let mut j = y - 4;
        while j < y + 4 && found < MOST_CURING_BLOCKS {
            let mut k = z - 4;
            while k < z + 4 && found < MOST_CURING_BLOCKS {
                let speeds = tick
                    .world
                    .block(BlockPos::new(i, j, k))
                    .is_some_and(|state| {
                        clustine_blocks::data::in_tag(
                            state,
                            "minecraft:speeds_up_zombie_villager_curing",
                        )
                    });
                if speeds {
                    if tick.draws.next_float() < 0.3 {
                        progress += 1;
                    }
                    found += 1;
                }
                k += 1;
            }
            j += 1;
        }
        i += 1;
    }
    progress
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
    /// `Zombie.doHurtTarget`'s fire.
    fn hit_extras(&self, ai: &Ai<'_, '_>) -> Vec<clustine_world::HitExtra> {
        family::hit_extras(ai)
    }

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

    /// `ZombieVillager.mobInteract` (277–364): a golden apple on one that is weak starts
    /// its cure, `3600 + nextInt(2401)` ticks of its own random, the player who used it
    /// kept, heard by the client as entity event 16 (the apple is kept: every player is
    /// creative); on one that is not it is taken for nothing (`CONSUME`). Anything else is
    /// the zombie's: nothing.
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        if !used
            .held
            .as_ref()
            .is_some_and(|stack| stack.count > 0 && stack.item == items::GOLDEN_APPLE)
        {
            return Interaction::Pass;
        }
        if !used.mob.body.effects.has(Effect::WEAKNESS) {
            return Interaction::Consume;
        }
        let time = 3600 + used.draws.next_int(2401) as i32;
        start_converting(used.mob, used.player_uuid, time, used.moment);
        used.did.push(Did::Event(CURE_STARTED));
        Interaction::SuccessServer
    }

    /// `ZombieVillager.tick`'s before `Zombie.tick` (244–276): a living one that is
    /// curing counts down by its progress, and at nothing or below becomes a villager
    /// (`finishConversion`: `convertTo(VILLAGER, single(this, false, false))`, its
    /// equipment not kept, heard as level event 1027).
    fn before_tick(&self, tick: &mut MobTick<'_>) {
        let own = villager(&tick.mob);
        if tick.mob.body.vitals.dying() || own.conversion == -1 {
            return;
        }
        let progress = conversion_progress(tick);
        let left = own.conversion.wrapping_sub(progress);
        update_villager(&mut tick.mob, |villager| villager.conversion = left);
        if left <= 0 {
            tick.out.local.push(Local::Convert {
                into: entity_types::VILLAGER as u16,
                keep_equipment: false,
                sound: Some(CURED_EVENT),
            });
        }
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
