//! Which class of the game's ladder each mob's type is (`Animal`, `WaterAnimal`,
//! `AbstractGolem` …; the brief's sections 1.1 and 1.12), for the defaults its class
//! gives every type of it: whether it despawns far from players, the experience of its
//! death, whether a lead holds it. A type's own module (N7e on) overrides what its class
//! does not decide (a chicken jockey, a tamed cat).

use clustine_data::{EntityTypeInfo, MobCategory, entity_types as types};

/// A mob's class, as far as its defaults tell classes apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Family {
    /// `Animal` (and `TamableAnimal`, `AbstractHorse` …).
    Animal,
    /// `WaterAnimal` (the fish and the tadpole) and `AgeableWaterCreature` (squids and
    /// dolphins).
    WaterAnimal,
    /// `AbstractGolem`, `AbstractVillager` and the allay: kept wherever they are.
    Kept,
    /// The bat's `AmbientCreature`, which no lead holds.
    Ambient,
    /// `Monster` (and `PatrollingMonster`, `Raider`, the guardians), an `Enemy`: hostile
    /// sounds, no lead, despawned far from players (`n7k-plan.md`, section 2.1).
    Monster,
    /// Any other mob (`Mob`'s defaults: monsters, raiders, the rest).
    Other,
}

/// The class of the mob type `kind`.
pub(crate) fn of(kind: u16) -> Family {
    let kind = i32::from(kind);
    match kind {
        types::ARMADILLO
        | types::AXOLOTL
        | types::BEE
        | types::CAMEL
        | types::CAMEL_HUSK
        | types::CAT
        | types::CHICKEN
        | types::COW
        | types::DONKEY
        | types::FOX
        | types::FROG
        | types::GOAT
        | types::HAPPY_GHAST
        | types::HOGLIN
        | types::HORSE
        | types::LLAMA
        | types::MOOSHROOM
        | types::MULE
        | types::NAUTILUS
        | types::OCELOT
        | types::PANDA
        | types::PARROT
        | types::PIG
        | types::POLAR_BEAR
        | types::RABBIT
        | types::SHEEP
        | types::SKELETON_HORSE
        | types::SNIFFER
        | types::STRIDER
        | types::TRADER_LLAMA
        | types::TURTLE
        | types::WOLF
        | types::ZOMBIE_HORSE
        | types::ZOMBIE_NAUTILUS => Family::Animal,
        types::COD
        | types::SALMON
        | types::PUFFERFISH
        | types::TROPICAL_FISH
        | types::TADPOLE
        | types::SQUID
        | types::GLOW_SQUID
        | types::DOLPHIN => Family::WaterAnimal,
        types::IRON_GOLEM
        | types::SNOW_GOLEM
        | types::COPPER_GOLEM
        | types::VILLAGER
        | types::WANDERING_TRADER
        | types::ALLAY => Family::Kept,
        types::BAT => Family::Ambient,
        types::ZOMBIE
        | types::HUSK
        | types::DROWNED
        | types::ZOMBIE_VILLAGER
        | types::SKELETON
        | types::STRAY
        | types::BOGGED
        | types::PARCHED
        | types::CREEPER
        | types::SPIDER
        | types::CAVE_SPIDER
        | types::WITCH
        | types::PILLAGER
        | types::VINDICATOR
        | types::EVOKER
        | types::GUARDIAN
        | types::ELDER_GUARDIAN => Family::Monster,
        _ => Family::Other,
    }
}

/// `Monster.getSoundSource()`'s `HOSTILE` for a monster's own sounds; `NEUTRAL`, `Mob`'s,
/// for every other mob's.
pub(crate) fn sound_source(kind: u16) -> i32 {
    match of(kind) {
        Family::Monster => SOUND_SOURCE_HOSTILE,
        _ => crate::physics::item::SOUND_SOURCE_NEUTRAL,
    }
}

/// `SoundSource.HOSTILE` by its ordinal.
const SOUND_SOURCE_HOSTILE: i32 = 5;

/// Whether a mob of the type `kind` is an `Enemy`: the monsters, and the other types of the
/// game's `monster` category that implement it (slimes, ghasts and the rest, when they
/// come). The undead horses and the zombie nautilus are animals, and no enemy.
pub(crate) fn is_enemy(kind: u16) -> bool {
    match of(kind) {
        Family::Monster => true,
        Family::Other => EntityTypeInfo::of(i32::from(kind)).category == MobCategory::Monster,
        Family::Animal | Family::WaterAnimal | Family::Kept | Family::Ambient => false,
    }
}

/// `removeWhenFarAway(distance)` by the class of `kind` (the brief's table in 1.12): an
/// animal is kept, but the camel husk, the zombie horse and the nautili (`true` in their
/// own classes); an untamed cat or untrusting ocelot after 2400 ticks of its
/// `tickCount` (none is tamed before N7h); an axolotl and a fish not from a bucket and
/// unnamed (none is, without components: the plan's question 4); golems, villagers, the
/// wandering trader and the allay never; every other mob, the bat among them, always.
pub(crate) fn remove_when_far_away(kind: u16, tick_count: u32) -> bool {
    let id = i32::from(kind);
    match of(kind) {
        Family::Animal => match id {
            types::CAMEL_HUSK | types::ZOMBIE_HORSE | types::NAUTILUS | types::ZOMBIE_NAUTILUS => {
                true
            }
            types::CAT | types::OCELOT => tick_count > 2400,
            types::AXOLOTL => true,
            _ => false,
        },
        Family::Kept => false,
        // A monster from an egg despawns by the game's rule; a raider's patrolling and a
        // zombie villager's conversion are their types' own (N7k).
        Family::WaterAnimal | Family::Ambient | Family::Monster | Family::Other => true,
    }
}

/// `canBeLeashed()` by the class of `kind`: every mob that is not an `Enemy` (a
/// monster's type, but the undead horses and the zombie nautilus, which the game makes
/// animals), but the bat. **Inferred** for the monsters: `Enemy` is an interface the
/// dump's classes implement one by one, read here as the category.
pub(crate) fn can_be_leashed(kind: u16) -> bool {
    match of(kind) {
        Family::Ambient | Family::Monster => false,
        // `AbstractVillager.canBeLeashed` is false.
        Family::Kept => !matches!(i32::from(kind), types::VILLAGER | types::WANDERING_TRADER),
        Family::Animal | Family::WaterAnimal => true,
        Family::Other => EntityTypeInfo::of(i32::from(kind)).category != MobCategory::Monster,
    }
}

/// `MobCategory.getDespawnDistance()` of the type `kind`: 64 for the water's ambient,
/// 128 for the rest.
pub(crate) fn despawn_distance(kind: u16) -> i32 {
    match EntityTypeInfo::of(i32::from(kind)).category {
        MobCategory::WaterAmbient => 64,
        _ => 128,
    }
}
