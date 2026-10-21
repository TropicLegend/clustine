//! Camel husks (`CamelHusk`; `docs/groundwork/n7-brief.md`, section 7.7.7): a camel of
//! the monsters' category that despawns as a monster does (the class ladder's
//! `removeWhenFarAway`), never breeds nor falls in love nor is a calf, eats rabbit feet,
//! is kept by any player's use, and sounds as its own. Its body, brain, attributes,
//! sizes and riding are the camel's ([`super::camel`]), of this [`Breed`].
//!
//! Named differences: no husk or parched rides it (N9's, which a natural husk's spawn
//! makes); so no mob controls it (`isMobControlled`), it may always be leashed and it
//! panics as a camel does; `chargeSpeedModifier` (4) is read by N9's spears alone.

use clustine_data::{entity_types, sounds};

use super::camel::{Breed, Camel, Sounds};
use crate::mobs::ai::goals::Items;

/// `chargeSpeedModifier()`, which N9's mounted husks' spears read.
#[allow(dead_code, reason = "what N9's spears read")]
pub(crate) const CHARGE_SPEED_MODIFIER: f32 = 4.0;

pub(crate) static BREED: Breed = Breed {
    kind: entity_types::CAMEL_HUSK,
    food: Items::Tag("minecraft:camel_husk_food"),
    breeds: false,
    kept_by_a_use: true,
    step_volume: 0.4,
    sounds: Sounds {
        ambient: sounds::ENTITY_CAMEL_HUSK_AMBIENT,
        hurt: sounds::ENTITY_CAMEL_HUSK_HURT,
        death: sounds::ENTITY_CAMEL_HUSK_DEATH,
        dash: sounds::ENTITY_CAMEL_HUSK_DASH,
        dash_ready: sounds::ENTITY_CAMEL_HUSK_DASH_READY,
        eat: sounds::ENTITY_CAMEL_HUSK_EAT,
        stand: sounds::ENTITY_CAMEL_HUSK_STAND,
        sit: sounds::ENTITY_CAMEL_HUSK_SIT,
        saddle: sounds::ENTITY_CAMEL_HUSK_SADDLE,
        step: sounds::ENTITY_CAMEL_HUSK_STEP,
        step_sand: sounds::ENTITY_CAMEL_HUSK_STEP_SAND,
    },
};

/// The camel husk.
pub(crate) static CAMEL_HUSK: Camel = Camel { breed: &BREED };
