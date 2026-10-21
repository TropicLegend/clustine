//! The zombie nautilus (`ZombieNautilus`; `docs/groundwork/n7-brief.md`, section 7.12,
//! with 9.7.5 and 10.7.5): a nautilus of the undead that is never a baby and never
//! breeds, breathes as the undead do, never dries out, and is of the temperate variant,
//! or the warm (coral) one in a warm ocean. It is tamed, fed, saddled and armoured as a
//! nautilus is ([`super::nautilus`]), and its brain, without the nautilus's panic, waits
//! for the brain's machinery at the same seam.
//!
//! Named differences beyond the nautilus's: it does not burn in daylight (N9); no
//! drowned rides it, so nothing keeps it from a lead (N9's jockeys); its bucket's
//! variant component is not kept (stacks carry no components).

use clustine_data::sounds;

use super::animal::{self, variants};
use super::nautilus::{self, Family, Voices, state_of};
use super::tamable::{Pet, Tame};
use super::{Breeding, Interaction, MobType, Spawning, Used};
use crate::living::{Sinks, Travel};
use crate::mobs::Mob;
use crate::mobs::ai::Setup;
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::MobTick;
use crate::region::Random;
use crate::shown::{DataShown, Holder};

/// The synced index of `ZombieNautilus.DATA_VARIANT_ID`.
pub(crate) const VARIANT_INDEX: u8 = 21;

/// The biome tag of the warm variant (`#spawns_coral_variant_zombie_nautilus`).
const WARM_BIOMES: &str = "minecraft:spawns_coral_variant_zombie_nautilus";

/// Its sounds, the same at every age.
const VOICES: Voices = Voices {
    ambient: [
        sounds::ENTITY_ZOMBIE_NAUTILUS_AMBIENT,
        sounds::ENTITY_ZOMBIE_NAUTILUS_AMBIENT_LAND,
    ],
    hurt: [
        sounds::ENTITY_ZOMBIE_NAUTILUS_HURT,
        sounds::ENTITY_ZOMBIE_NAUTILUS_HURT_LAND,
    ],
    death: [
        sounds::ENTITY_ZOMBIE_NAUTILUS_DEATH,
        sounds::ENTITY_ZOMBIE_NAUTILUS_DEATH_LAND,
    ],
    dash: [
        sounds::ENTITY_ZOMBIE_NAUTILUS_DASH,
        sounds::ENTITY_ZOMBIE_NAUTILUS_DASH_LAND,
    ],
    dash_ready: [
        sounds::ENTITY_ZOMBIE_NAUTILUS_DASH_READY,
        sounds::ENTITY_ZOMBIE_NAUTILUS_DASH_READY_LAND,
    ],
    eat: sounds::ENTITY_ZOMBIE_NAUTILUS_EAT,
};

/// The zombie nautilus.
pub(crate) struct ZombieNautilus;

impl Family for ZombieNautilus {
    fn voices(&self, _: &Mob) -> &'static Voices {
        &VOICES
    }

    fn charge_speed(&self) -> f32 {
        0.5
    }
}

impl Pet for ZombieNautilus {
    fn tame(&self, mob: &Mob) -> Tame {
        state_of(mob).1.tame
    }

    fn set_tame(&self, mob: &mut Mob, tame: Tame) {
        nautilus::update(mob, |own| own.tame = tame);
    }
}

/// What a variant's entry is, for its spawning (`zombie_nautilus_variant/*.json`): the
/// warm one at priority 1 in its biomes, the temperate one at 0 everywhere.
#[derive(Debug, Clone, Copy)]
struct Entry {
    name: &'static str,
    priority: i32,
    in_tag: Option<&'static str>,
}

/// `VariantUtils.selectVariantToSpawn` of the zombie nautilus's variant (the brief's
/// 7.12.6): of the variants whose condition holds, those of the highest priority, one
/// drawn of the level's random even where one is left (`Util.getRandomSafe`).
pub(crate) fn select_variant(biome: Option<&str>, level: &mut Random) -> u8 {
    let entries = [
        Entry {
            name: "minecraft:warm",
            priority: 1,
            in_tag: Some(WARM_BIOMES),
        },
        Entry {
            name: "minecraft:temperate",
            priority: 0,
            in_tag: None,
        },
    ];
    let holding: Vec<Entry> = entries
        .into_iter()
        .filter(|entry| {
            entry
                .in_tag
                .is_none_or(|tag| animal::biome_in_tag(biome, tag))
        })
        .collect();
    let best = holding
        .iter()
        .map(|entry| entry.priority)
        .max()
        .unwrap_or(0);
    let kept: Vec<&str> = holding
        .iter()
        .filter(|entry| entry.priority == best)
        .map(|entry| entry.name)
        .collect();
    let chosen = if kept.is_empty() {
        "minecraft:temperate"
    } else {
        kept[level.next_int(kept.len() as u32) as usize]
    };
    variants::id(nautilus::VARIANTS, chosen)
}

impl MobType for ZombieNautilus {
    fn register_goals(&self, setup: &mut Setup) {
        nautilus::register(setup);
    }

    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        nautilus::custom_server_ai_step(tick);
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
    }

    /// Its eyes' medium only: it has no air of its own to keep.
    fn after_base_tick(&self, tick: &mut MobTick<'_>, air: i32) {
        nautilus::after_base_tick(tick, air, false);
    }

    fn after_tick(&self, tick: &mut MobTick<'_>) {
        nautilus::after_tick(tick, self);
    }

    fn travel(&self, tick: &MobTick<'_>) -> Travel {
        Travel::Swims {
            speed: tick.mob.body.inputs.speed,
            sinks: Sinks::Never,
        }
    }

    fn pushed_by_fluid(&self) -> bool {
        false
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        nautilus::mob_interact(used, self)
    }

    /// `ZombieNautilus.finalizeSpawn`: its variant first, then the nautilus's.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let variant = select_variant(spawn.biome, spawn.level);
        nautilus::update(spawn.mob, |own| own.variant = variant);
        nautilus::init_memories(spawn);
    }

    /// `getBreedOffspring`: none.
    fn breed_offspring(&self, _: &mut Breeding<'_>) -> Option<Mob> {
        None
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn plays_step_sounds(&self) -> bool {
        false
    }

    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        Some(nautilus::medium(VOICES.ambient, &state_of(mob).1))
    }

    fn ambient_interval(&self) -> u32 {
        120
    }

    fn hurt_sound(&self, mob: &Mob, _: &DamageSource) -> Option<i32> {
        Some(nautilus::medium(VOICES.hurt, &state_of(mob).1))
    }

    fn death_sound(&self, mob: &Mob) -> Option<i32> {
        Some(nautilus::medium(VOICES.death, &state_of(mob).1))
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn requires_custom_persistence(&self, mob: &Mob) -> bool {
        state_of(mob).1.tame.is_tame()
    }

    fn pet(&self) -> Option<&'static dyn Pet> {
        Some(&ZombieNautilus)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let (_, own) = state_of(mob);
        let fresh = nautilus::NautilusState::default();
        let mut values = nautilus::shown(mob);
        values.push((
            VARIANT_INDEX,
            DataShown::Holder(Holder::ZombieNautilusVariant, i32::from(own.variant)),
            own.variant == fresh.variant,
        ));
        values
    }
}
