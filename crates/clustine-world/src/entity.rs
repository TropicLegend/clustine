//! Entities other than players as their chunks keep them, the changes a tick makes to
//! them, and a player's inventory (`docs/adr/0028-entities.md`, sections 2 and 3;
//! `docs/groundwork/n2-plan.md`).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ChunkPos, EntityId};

/// A number of items of one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemStack {
    /// Id in the item registry.
    pub item: i32,
    pub count: i32,
}

/// An entity other than a player, as its chunk keeps it.
///
/// Its counters (`age` and the data's delays) are as of the tick of the record that
/// holds it: a commit, a copy's update or a file. Regions count their ticks each from
/// their own start, so a region that takes an entity in keeps its counters in its own
/// ticks and writes them as of each record again; the game freezes them while an entity
/// does not tick, and so do these (the plan, section 11).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub key: EntityId,
    /// Its type, an index into `clustine_data::ENTITY_TYPES`.
    pub kind: u16,
    pub uuid: Uuid,
    pub position: [f64; 3],
    pub motion: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    /// Changes of holder behind it (`docs/adr/0028-entities.md`, section 6).
    pub hops: u32,
    /// The ticks it has ticked.
    pub age: u32,
    pub data: EntityData,
}

impl Entity {
    /// The chunk its position is in, by the game's rule (the floor of x and z over 16).
    pub fn chunk(&self) -> ChunkPos {
        ChunkPos::containing(self.position[0], self.position[2])
    }
}

/// Floats are told apart by their bits, so that an entity read back is equal to the one
/// written, and equality is an equivalence.
impl PartialEq for Entity {
    fn eq(&self, other: &Self) -> bool {
        let bits = |values: [f64; 3]| values.map(f64::to_bits);
        self.key == other.key
            && self.kind == other.kind
            && self.uuid == other.uuid
            && bits(self.position) == bits(other.position)
            && bits(self.motion) == bits(other.motion)
            && self.yaw.to_bits() == other.yaw.to_bits()
            && self.pitch.to_bits() == other.pitch.to_bits()
            && self.on_ground == other.on_ground
            && self.hops == other.hops
            && self.age == other.age
            && self.data == other.data
    }
}

impl Eq for Entity {}

/// What an entity is beside what every entity is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntityData {
    Item {
        stack: ItemStack,
        /// Ticks before it may be picked up (the game's pick-up delay, 40 after a throw).
        pick_up_delay: u16,
        /// Who threw it, if a player did.
        thrower: Option<Uuid>,
    },
}

/// What a tick did to an entity of a chunk. Applied in the order a tick made them, after
/// the tick's block changes, and idempotent from any state at or after the last
/// checkpoint, as block changes are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntityChange {
    /// The entity as it is after the tick, new to the chunk or taken in again.
    Whole(Entity),
    /// It left the chunk.
    Gone(EntityId),
    /// Fields the tick changed, each to a definite value.
    Set(EntityId, Vec<EntityField>),
}

impl EntityChange {
    /// The entity it is of.
    pub fn key(&self) -> EntityId {
        match self {
            Self::Whole(entity) => entity.key,
            Self::Gone(key) | Self::Set(key, _) => *key,
        }
    }
}

/// A field of an entity that a tick set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EntityField {
    Position([f64; 3]),
    Motion([f64; 3]),
    Rotation { yaw: f32, pitch: f32 },
    OnGround(bool),
    Age(u32),
    Data(EntityData),
}

impl PartialEq for EntityField {
    fn eq(&self, other: &Self) -> bool {
        let bits = |values: &[f64; 3]| values.map(f64::to_bits);
        match (self, other) {
            (Self::Position(a), Self::Position(b)) | (Self::Motion(a), Self::Motion(b)) => {
                bits(a) == bits(b)
            }
            (
                Self::Rotation { yaw, pitch },
                Self::Rotation {
                    yaw: other_yaw,
                    pitch: other_pitch,
                },
            ) => yaw.to_bits() == other_yaw.to_bits() && pitch.to_bits() == other_pitch.to_bits(),
            (Self::OnGround(a), Self::OnGround(b)) => a == b,
            (Self::Age(a), Self::Age(b)) => a == b,
            (Self::Data(a), Self::Data(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for EntityField {}

impl EntityField {
    /// Sets this field of `entity`.
    pub fn set(&self, entity: &mut Entity) {
        match self {
            Self::Position(position) => entity.position = *position,
            Self::Motion(motion) => entity.motion = *motion,
            Self::Rotation { yaw, pitch } => (entity.yaw, entity.pitch) = (*yaw, *pitch),
            Self::OnGround(on_ground) => entity.on_ground = *on_ground,
            Self::Age(age) => entity.age = *age,
            Self::Data(data) => entity.data = data.clone(),
        }
    }
}

/// The most entities a chunk holds. The simulation adds none beyond it; nothing
/// ordinary makes that many in one chunk.
pub const MAX_CHUNK_ENTITIES: usize = 4096;

/// The slots of a player's inventory, as the game's `Inventory` numbers them.
pub const INVENTORY_SLOTS: usize = 41;
/// The hotbar: slots 0 to 8.
pub const HOTBAR_SLOTS: usize = 9;
/// The first of the main inventory's 27 slots.
pub const MAIN_START: usize = 9;
/// Armour, from the feet up: 36 feet, 37 legs, 38 chest, 39 head.
pub const ARMOUR_START: usize = 36;
/// The off hand.
pub const OFF_HAND: usize = 40;

/// A player's inventory: [`INVENTORY_SLOTS`] slots, numbered as the game's `Inventory`.
/// A type of its own because serde derives arrays only up to 32. The slots are boxed:
/// inline they would make every message that carries a player (a transfer, a place, a
/// presence) some 500 bytes, and every enum with such a variant as large.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inventory(pub Box<[Option<ItemStack>; INVENTORY_SLOTS]>);

impl Default for Inventory {
    fn default() -> Self {
        Self(Box::new([None; INVENTORY_SLOTS]))
    }
}

impl Inventory {
    /// The inventory with `hotbar` in its first nine slots and nothing else.
    pub fn with_hotbar(hotbar: [Option<ItemStack>; HOTBAR_SLOTS]) -> Self {
        let mut inventory = Self::default();
        inventory.0[..HOTBAR_SLOTS].copy_from_slice(&hotbar);
        inventory
    }

    /// The hotbar's nine slots.
    pub fn hotbar(&self) -> [Option<ItemStack>; HOTBAR_SLOTS] {
        let mut hotbar = [None; HOTBAR_SLOTS];
        hotbar.copy_from_slice(&self.0[..HOTBAR_SLOTS]);
        hotbar
    }

    /// The slot `slot`, if the inventory has one so numbered.
    pub fn get(&self, slot: usize) -> Option<Option<ItemStack>> {
        self.0.get(slot).copied()
    }

    /// Puts `stack` into `slot`; false if there is no such slot.
    pub fn set(&mut self, slot: usize, stack: Option<ItemStack>) -> bool {
        match self.0.get_mut(slot) {
            Some(place) => {
                *place = stack;
                true
            }
            None => false,
        }
    }

    /// The inventory slot shown in the client's container slot `container` of the
    /// player's own inventory screen (`InventoryMenu`), if it is one: 5 to 8 the armour,
    /// head first, 9 to 35 the main inventory, 36 to 44 the hotbar, 45 the off hand. The
    /// crafting slots 0 to 4 are none.
    pub fn from_container(container: i16) -> Option<usize> {
        let container = usize::try_from(container).ok()?;
        match container {
            5..=8 => Some(ARMOUR_START + 3 - (container - 5)),
            9..=35 => Some(container),
            36..=44 => Some(container - 36),
            45 => Some(OFF_HAND),
            _ => None,
        }
    }

    /// The container slot of the player's own inventory screen that shows `slot`.
    pub fn to_container(slot: usize) -> Option<i16> {
        let container = match slot {
            0..=8 => slot + 36,
            9..=35 => slot,
            36..=39 => 5 + (39 - slot),
            40 => 45,
            _ => return None,
        };
        i16::try_from(container).ok()
    }
}

impl Serialize for Inventory {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.as_slice().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Inventory {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let slots = Vec::<Option<ItemStack>>::deserialize(deserializer)?;
        let slots: Box<[Option<ItemStack>; INVENTORY_SLOTS]> =
            slots.try_into().map_err(|slots: Vec<_>| {
                serde::de::Error::invalid_length(slots.len(), &"the slots of an inventory")
            })?;
        Ok(Self(slots))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_container_slot_of_the_inventory_screen_shows_the_inventory_slot_the_game_puts_there() {
        // `InventoryMenu`: armour from the head down at 5 to 8, the main inventory at 9
        // to 35, the hotbar at 36 to 44 and the off hand at 45.
        assert_eq!(Inventory::from_container(5), Some(39));
        assert_eq!(Inventory::from_container(8), Some(36));
        assert_eq!(Inventory::from_container(9), Some(9));
        assert_eq!(Inventory::from_container(35), Some(35));
        assert_eq!(Inventory::from_container(36), Some(0));
        assert_eq!(Inventory::from_container(44), Some(8));
        assert_eq!(Inventory::from_container(45), Some(40));
        for crafting in [-1, 0, 1, 4, 46] {
            assert_eq!(Inventory::from_container(crafting), None);
        }
        for slot in 0..INVENTORY_SLOTS {
            let container = Inventory::to_container(slot).expect("every slot is shown");
            assert_eq!(Inventory::from_container(container), Some(slot));
        }
        assert_eq!(Inventory::to_container(INVENTORY_SLOTS), None);
    }

    #[test]
    fn an_inventory_travels_with_all_its_slots_and_refuses_another_count() {
        let mut inventory = Inventory::default();
        inventory.set(40, Some(ItemStack { item: 3, count: 1 }));
        let bytes = postcard::to_allocvec(&inventory).unwrap();
        assert_eq!(
            postcard::from_bytes::<Inventory>(&bytes).unwrap(),
            inventory
        );
        let short = postcard::to_allocvec(&vec![None::<ItemStack>; 9]).unwrap();
        assert!(postcard::from_bytes::<Inventory>(&short).is_err());
    }

    #[test]
    fn a_field_set_on_an_entity_is_what_it_then_has() {
        let mut entity = Entity {
            key: EntityId(7),
            kind: 72,
            uuid: Uuid::from_u128(9),
            position: [0.5, -59.0, 0.5],
            motion: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            hops: 0,
            age: 0,
            data: EntityData::Item {
                stack: ItemStack { item: 1, count: 1 },
                pick_up_delay: 40,
                thrower: None,
            },
        };
        EntityField::Position([3.25, -60.0, -0.5]).set(&mut entity);
        EntityField::Age(12).set(&mut entity);
        assert_eq!(entity.position, [3.25, -60.0, -0.5]);
        assert_eq!(entity.age, 12);
        assert_eq!(entity.chunk(), ChunkPos::new(0, -1));
    }
}
