//! What clients are shown of a mob's synced data (`SynchedEntityData`, the brief's 9.1),
//! read from its record, for the edge, which owns the protocol: each value with its
//! index and whether it is the type's default (a client being paired is sent only the
//! others). The body's values are every mob's; a type adds its own through
//! [`crate::mobs::types::MobType::shown`].
//!
//! Named difference: the pose `DYING` a dying mob has is not shown; the death's entity
//! event shows its fall.

use clustine_world::{EntityData, EntityId, ItemStack};
use serde::{Deserialize, Serialize};

use crate::mobs::{Mob, types};

/// A synced value as the edge writes it, by the serializer the game's index has.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// An entity by its key, or none (`OPTIONAL_UNSIGNED_INT` of an entity's id, or
    /// `INT` of one): the guardian's beam's target, a projectile's owner. The edge
    /// writes the key's wire id (`docs/groundwork/n9-plan.md`, section 10).
    Entity(Option<EntityId>),
    /// A block state by id, or none (`OPTIONAL_BLOCK_STATE`): what an enderman carries.
    OptionalBlockState(Option<u16>),
    /// A list of particles (`PARTICLES`): a living entity's effect particles.
    Particles(Vec<ShownParticle>),
    /// A stack (`ITEM_STACK`): a rocket's.
    Stack(Option<ItemStack>),
    /// A number or none (`OPTIONAL_UNSIGNED_INT`).
    OptionalUnsignedInt(Option<u32>),
}

/// A particle of a [`DataShown::Particles`] list: `entity_effect` with a colour, or a
/// particle without options by its type's id (`docs/groundwork/n9-plan.md`, 5.2).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ShownParticle {
    EntityEffect { colour: i32 },
    Simple(i32),
}

/// The serializer of a [`DataShown::Serialised`] value: the enumerations the game syncs
/// by their ordinals with serializers of their own (`EntityDataSerializers`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Serialiser {
    ArmadilloState,
    SnifferState,
    CopperGolemState,
    WeatheringCopperState,
}

/// The registry an entry of [`DataShown::Holder`] is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    /// `LivingEntity.DATA_LIVING_ENTITY_FLAGS`: an item in use (1), with the off hand (2).
    pub const LIVING_FLAGS: u8 = 8;
    pub const HEALTH: u8 = 9;
    pub const MOB_FLAGS: u8 = 15;
    /// `AgeableMob.DATA_BABY_ID`, an ageable type's.
    pub const BABY: u8 = 16;
}

/// The witch, whose living flags are never shown.
const WITCH: u16 = clustine_data::entity_types::WITCH as u16;

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
    // The living flags of its use of an item, but not a witch's: its drinking is its
    // own, and the official server never sends them for it.
    if kind != WITCH {
        let living = mob.body.using.flags;
        values.push((index::LIVING_FLAGS, DataShown::Byte(living), living == 0));
    }
    values.extend(types::of(kind).shown(&mob));
    values.sort_by_key(|(index, _, _)| *index);
    values
}

/// `Entity.FLAG_INVISIBLE`, the shared flags' bit 5.
const INVISIBLE: u8 = 0x20;

/// `Entity.FLAG_SWIMMING`, the shared flags' bit 4.
const SWIMMING: u8 = 0x10;

/// The shared flags (`DATA_SHARED_FLAGS_ID`) a mob's record sets beyond its burning,
/// which its tick's report carries: invisible while it has invisibility, swimming while
/// it swims (a drowned). Read from the effects' and the swimming's parts alone, as an
/// edge reads them at every change; none for a record that is no mob's, and nothing of
/// a part that cannot be read.
pub fn mob_shared_flags(data: &EntityData) -> u8 {
    let EntityData::Mob { body, .. } = data else {
        return 0;
    };
    let invisible = match crate::mobs::part::decode::<crate::mobs::body::effects::Effects>(body) {
        Ok(effects) if effects.invisible() => INVISIBLE,
        _ => 0,
    };
    let swimming = match crate::mobs::part::decode::<crate::mobs::body::Swimming>(body) {
        Ok(swimming) if swimming.swimming => SWIMMING,
        _ => 0,
    };
    invisible | swimming
}

/// What a mob wears and holds as its record has it, by slot (the game's
/// `EquipmentSlot` ordinals), the empty slots left out: what a client newly shown it is
/// sent (`SetEquipment`). Empty for a record that is no mob's, or whose equipment cannot be
/// read.
pub fn mob_equipment(data: &EntityData) -> Vec<(u8, clustine_world::ItemStack)> {
    let EntityData::Mob { body, .. } = data else {
        return Vec::new();
    };
    crate::mobs::part::decode::<crate::mobs::body::Equipment>(body)
        .map(|equipment| {
            (0u8..)
                .zip(equipment.slots)
                .filter_map(|(slot, stack)| stack.map(|stack| (slot, stack)))
                .collect()
        })
        .unwrap_or_default()
}

/// The attributes a client newly shown a mob of the type `kind` is sent with it, as its
/// record has them (`getSyncableAttributes`, `sendPairingData`): a baby zombie's speed
/// with its modifier, which no change will ever report. Empty for a record that is no
/// mob's.
pub fn mob_attributes(kind: u16, data: &EntityData) -> Vec<crate::api::AttributeShown> {
    let EntityData::Mob { body, .. } = data else {
        return Vec::new();
    };
    crate::mobs::part::decode::<crate::mobs::body::Attributes>(body)
        .map(|attributes| {
            attributes
                .syncable(kind, false)
                .into_iter()
                .map(|instance| crate::api::AttributeShown {
                    attribute: instance.attribute,
                    base: instance.base,
                    modifiers: instance
                        .modifiers
                        .into_iter()
                        .map(|modifier| (modifier.id, modifier.amount, modifier.operation))
                        .collect(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A mob's record wearing and holding `stacks` by slot and nothing else, as an edge's
/// tests make one to show (the edge has no other way to write a mob's parts).
pub fn mob_data_wearing(stacks: &[(u8, clustine_world::ItemStack)]) -> EntityData {
    let mut mob = crate::mobs::Mob::fresh();
    for (slot, stack) in stacks {
        if let Some(worn) = mob.body.equipment.slots.get_mut(usize::from(*slot)) {
            *worn = Some(stack.clone());
        }
    }
    mob.encode()
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
                (index::LIVING_FLAGS, DataShown::Byte(0), true),
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
