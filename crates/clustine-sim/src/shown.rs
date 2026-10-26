//! What clients are shown of a mob's synced data (`SynchedEntityData`, the brief's 9.1),
//! read from its record, for the edge, which owns the protocol: each value with its
//! index and whether it is the type's default (a client being paired is sent only the
//! others). The body's values are every mob's; a type adds its own through
//! [`crate::mobs::types::MobType::shown`].
//!
//! Named difference: the pose `DYING` a dying mob has is not shown; the death's entity
//! event shows its fall.

use clustine_world::EntityData;

use crate::mobs::{Mob, types};

/// A synced value as the edge writes it, by the serializer the game's index has.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DataShown {
    Byte(u8),
    Int(i32),
    Float(f32),
    Boolean(bool),
    /// A long (`EntityDataSerializers.LONG`): a wolf's end of anger, in game time.
    Long(i64),
    /// An entity by its UUID, or none (`OPTIONAL_LIVING_ENTITY_REFERENCE`): a pet's
    /// owner.
    OptionalUuid(Option<u128>),
    /// A pose by its ordinal (`EntityDataSerializers.POSE`): a cat crouching.
    Pose(i32),
    /// An entry of a registry the server sends, by its id there: a farm animal's
    /// variant or sound variant, each of its own serializer.
    Holder(Holder, i32),
    /// A value of an enumeration of the game's own serializer, by its ordinal
    /// (`ByteBufCodecs.idMapper` of the enumeration, a VarInt): a copper golem's state
    /// and its weathering.
    Serialised(Serialiser, i32),
    /// A villager's or a zombie villager's data (`EntityDataSerializers.VILLAGER_DATA`):
    /// its type and profession by their registry ids
    /// ([`clustine_data::villagers`]) and its level, 1 to 5.
    VillagerData {
        kind: i32,
        profession: i32,
        level: i32,
    },
}

/// The serializer of a [`DataShown::Serialised`] value: the enumerations the game syncs
/// by their ordinals with serializers of their own (`EntityDataSerializers`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Serialiser {
    ArmadilloState,
    SnifferState,
    CopperGolemState,
    WeatheringCopperState,
}

/// The registry an entry of [`DataShown::Holder`] is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Holder {
    CowVariant,
    CowSoundVariant,
    PigVariant,
    PigSoundVariant,
    ChickenVariant,
    ChickenSoundVariant,
    WolfVariant,
    WolfSoundVariant,
    CatVariant,
    CatSoundVariant,
    FrogVariant,
    ZombieNautilusVariant,
}

/// The indices of `LivingEntity`'s and `Mob`'s values (the brief's 9.1).
pub mod index {
    pub const AIR_SUPPLY: u8 = 1;
    pub const TICKS_FROZEN: u8 = 7;
    pub const HEALTH: u8 = 9;
    pub const MOB_FLAGS: u8 = 15;
    /// `AgeableMob.DATA_BABY_ID`, an ageable type's.
    pub const BABY: u8 = 16;
}

/// `Mob.DATA_MOB_FLAGS_ID`'s bits.
const NO_AI: u8 = 1;
const LEFT_HANDED: u8 = 2;
const AGGRESSIVE: u8 = 4;

/// The synced values of the mob of type `kind` whose record is `data`, ascending by
/// index: none for a record that is not a mob's or cannot be read.
pub fn mob_values(kind: u16, data: &EntityData) -> Vec<(u8, DataShown, bool)> {
    let Ok(mob) = Mob::decode(data) else {
        return Vec::new();
    };
    let vitals = &mob.body.vitals;
    let flags = &mob.body.flags;
    let mob_flags = if flags.no_ai { NO_AI } else { 0 }
        | if flags.left_handed { LEFT_HANDED } else { 0 }
        | if mob.body.combat.aggressive {
            AGGRESSIVE
        } else {
            0
        };
    let mut values = vec![
        (
            index::AIR_SUPPLY,
            DataShown::Int(vitals.air),
            // Defined as the type's most (a dolphin's 4800).
            vitals.air == types::of(kind).max_air(),
        ),
        (
            index::TICKS_FROZEN,
            DataShown::Int(vitals.frozen),
            vitals.frozen == 0,
        ),
        (
            index::HEALTH,
            DataShown::Float(vitals.health),
            vitals.health.to_bits() == 1.0_f32.to_bits(),
        ),
        (index::MOB_FLAGS, DataShown::Byte(mob_flags), mob_flags == 0),
    ];
    values.extend(types::of(kind).shown(&mob));
    values.sort_by_key(|(index, _, _)| *index);
    values
}

/// `Entity.FLAG_INVISIBLE`, the shared flags' bit 5.
const INVISIBLE: u8 = 0x20;

/// The shared flags (`DATA_SHARED_FLAGS_ID`) a mob's record sets beyond its burning,
/// which its tick's report carries: invisible while it has invisibility. Read from the
/// effects' part alone, as an edge reads it at every change; none for a record that is
/// no mob's or whose effects cannot be read.
pub fn mob_shared_flags(data: &EntityData) -> u8 {
    let EntityData::Mob { body, .. } = data else {
        return 0;
    };
    match crate::mobs::part::decode::<crate::mobs::body::effects::Effects>(body) {
        Ok(effects) if effects.invisible() => INVISIBLE,
        _ => 0,
    }
}

/// The yaw of a mob's head (`getYHeadRot`) as its record has it, which a client is sent
/// apart from its body's (`ClientboundRotateHeadPacket`, and in its spawn); none for a
/// record that is no mob's. Read from the head's part alone, whose first value it is in
/// every version, as an edge reads it every tick.
pub fn head_yaw(data: &EntityData) -> Option<f32> {
    let EntityData::Mob { body, .. } = data else {
        return None;
    };
    let head = crate::mobs::part::body_ids::HEAD;
    let Ok(at) = body.binary_search_by_key(&head, |entry| entry.id) else {
        // A mob whose head is as a fresh one's: at 0.
        return Some(0.0);
    };
    postcard::take_from_bytes::<f32>(&body[at].bytes)
        .ok()
        .map(|(yaw, _)| yaw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mob_shows_its_health_air_and_flags_and_only_those_not_the_defaults_are_paired() {
        let mut mob = Mob::fresh();
        mob.body.vitals.health = 10.0;
        mob.body.vitals.air = 300;
        mob.body.flags.left_handed = true;
        mob.body.combat.aggressive = true;
        let data = mob.encode();
        // A type that adds nothing of its own (a hoglin, until its part).
        let cow = clustine_data::entity_types::HOGLIN as u16;
        let values = mob_values(cow, &data);
        assert_eq!(
            values,
            [
                (index::AIR_SUPPLY, DataShown::Int(300), true),
                (index::TICKS_FROZEN, DataShown::Int(0), true),
                (index::HEALTH, DataShown::Float(10.0), false),
                (
                    index::MOB_FLAGS,
                    DataShown::Byte(LEFT_HANDED | AGGRESSIVE),
                    false
                ),
            ]
        );
        // Not a mob's record: nothing.
        let knot = EntityData::LeashKnot {
            attached: clustine_world::Attached {
                block: clustine_world::BlockPos::new(0, 0, 0),
                direction: 1,
            },
        };
        assert!(mob_values(cow, &knot).is_empty());
    }
}
