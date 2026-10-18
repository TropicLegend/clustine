//! Entities other than players as their chunks keep them, the changes a tick makes to
//! them, and a player's inventory (`docs/adr/0028-entities.md`, sections 2 and 3;
//! `docs/groundwork/n2-plan.md`).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{BlockPos, ChunkPos, EdgeId, EntityId};

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
    /// How far it has fallen since it last stood on something (the game's
    /// `fallDistance`, a `double`).
    pub fall_distance: f64,
    /// The ticks it has left to burn, below 0 the ticks it is safe from fire for (the
    /// game's `remainingFireTicks`, 0 for an entity that has never burned).
    pub fire_ticks: i32,
    /// What it rides, who rides it and who holds its leash (`docs/groundwork/n5-plan.md`).
    pub riding: Riding,
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
            && self.fall_distance.to_bits() == other.fall_distance.to_bits()
            && self.fire_ticks == other.fire_ticks
            && self.riding == other.riding
            && self.data == other.data
    }
}

impl Eq for Entity {}

/// A `float` of the game's that is told apart by its bits, so that a record read back is
/// equal to the one written.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Float(pub f32);

impl PartialEq for Float {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for Float {}

/// A `double` of the game's, told apart by its bits as [`Float`] is.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Double(pub f64);

impl PartialEq for Double {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for Double {}

/// What an entity rides, who rides it and who holds its leash. A unit, a root vehicle
/// and all its passengers, lives in the root's chunk, each member a record of that
/// chunk's entities, whatever a passenger's own position (ADR-0028, section 5).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Riding {
    /// The vehicle it rides.
    pub vehicle: Option<EntityId>,
    /// Who rides it, in the game's order, which is the seat: the first is the one who
    /// steers, if a player.
    pub passengers: Vec<Seat>,
    /// Ticks before it may ride again (the game's `boardingCooldown`).
    pub cooldown: u16,
    /// Who holds its leash; boxed, as few entities are leashed and every record would be
    /// the larger for it.
    pub leash: Option<Box<Leash>>,
}

impl Riding {
    /// Whether it rides nothing, carries nobody and is held by nobody.
    pub fn is_free(&self) -> bool {
        self.vehicle.is_none() && self.passengers.is_empty() && self.leash.is_none()
    }
}

/// A place in a vehicle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seat {
    /// The entity sitting there: a member of the unit, or a player, whose entity is the
    /// player's region's.
    pub key: EntityId,
    pub player: bool,
    /// For a player handed over to this vehicle's region and not come yet: the seat is
    /// theirs for as long as it says (`docs/groundwork/n5-plan.md`, sections 7 and 8).
    pub pending: Option<Pending>,
}

/// A seat kept for a player on their way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    /// Ticks it is kept for still, as of the record's tick.
    pub left: u16,
    /// The edge the player belongs to, whose going drops the seat.
    pub edge: EdgeId,
}

/// Who holds a leash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LeashHolder {
    /// An entity, a player among them.
    Entity(EntityId),
    /// A fence, through the knot at it, which is found or made again when the leashed
    /// entity is taken in and its holder is not there (the game's delayed leash).
    Fence(BlockPos),
}

/// A leash and the swing it has (the game's `LeashData`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Leash {
    pub holder: LeashHolder,
    /// The game's `angularMomentum`.
    pub angular_momentum: Double,
}

/// What `VehicleEntity` keeps of the hits it took: the game sends it and does not save
/// it; Clustine keeps it so that a cart hit just before a line is not whole after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hurt {
    /// Ticks the wobble lasts still (`DATA_ID_HURT`).
    pub time: i32,
    /// The side it wobbles to, 1 or -1 (`DATA_ID_HURTDIR`).
    pub direction: i32,
    /// The damage taken, which falls by one a tick (`DATA_ID_DAMAGE`).
    pub damage: Float,
}

impl Default for Hurt {
    fn default() -> Self {
        Self {
            time: 0,
            direction: 1,
            damage: Float(0.0),
        }
    }
}

/// Where a block-attached entity is attached (the game's `BlockAttachedEntity.pos` and,
/// for those that hang, `direction`, the 3D data value of a direction: 0 down, 1 up, 2
/// north, 3 south, 4 west, 5 east).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attached {
    pub block: BlockPos,
    pub direction: u8,
}

/// The stacks of a container entity, by slot, ascending, each once.
pub type Contents = Vec<(u8, ItemStack)>;

/// What an entity is beside what every entity is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntityData {
    Item {
        stack: ItemStack,
        /// Ticks before it may be picked up (the game's pick-up delay, 40 after a throw).
        pick_up_delay: u16,
        /// Who threw it, if a player did.
        thrower: Option<Uuid>,
        /// What it can still take before it is destroyed (the game's 5).
        health: i16,
    },
    /// A block falling (`FallingBlockEntity`).
    FallingBlock {
        /// The block's state id.
        state: u16,
        /// The ticks it has fallen (the game's `time`).
        time: i32,
        /// Whether it drops as an item where it cannot land.
        drop_item: bool,
        /// Whether its drop is called off (a block that broke where it landed).
        cancel_drop: bool,
    },
    /// A minecart of any kind, the kind its entity type (`AbstractMinecart` and
    /// `OldMinecartBehavior`).
    Minecart {
        hurt: Hurt,
        /// A block shown in it other than its kind's (`DisplayState`).
        display: Option<u16>,
        /// Where the block is shown, if not the kind's (`DisplayOffset`).
        display_offset: Option<i32>,
        /// `flipped`.
        flipped: bool,
        /// A chest's or a hopper's stacks.
        contents: Contents,
        /// A furnace cart's fuel, in ticks.
        fuel: i32,
        /// A furnace cart's push, x and z.
        push: [Double; 2],
    },
    /// A boat or a raft of any wood, with a chest or not, the kind its entity type
    /// (`AbstractBoat`).
    Boat {
        hurt: Hurt,
        /// Whether the left and right paddles move.
        paddles: [bool; 2],
        /// `DATA_ID_BUBBLE_TIME`.
        bubble_time: i32,
        /// `outOfControlTicks`.
        out_of_control: Float,
        /// `deltaRotation`.
        turning: Float,
        /// `status` and `oldStatus`, as their ordinals.
        status: [u8; 2],
        /// A chest boat's stacks.
        contents: Contents,
    },
    /// An armour stand (`ArmorStand`, a reduced `LivingEntity`).
    ArmorStand {
        /// `DATA_CLIENT_FLAGS`: 1 small, 4 arms, 8 no base plate, 16 marker; and
        /// invisibility, bit 32 here.
        flags: u8,
        /// `DisabledSlots`.
        disabled_slots: i32,
        /// The head's, the body's, the left and right arms' and legs' rotations; boxed,
        /// as every entity's record would be the larger for them.
        pose: Box<[[Float; 3]; 6]>,
        /// What it wears and holds, by `EquipmentSlot` ordinal.
        equipment: Contents,
        health: Float,
        /// The age at its last hit, for the game's two hits within five ticks.
        last_hit: Option<u32>,
        /// `DATA_AIR_SUPPLY_ID`: the ticks of air it has left under water, 300 when
        /// full, down to -20.
        air: i16,
        /// `DATA_TICKS_FROZEN`: the ticks it has stood in powder snow, which a client
        /// shows as a shiver once it reaches 140.
        frozen: u16,
    },
    /// An item frame or a glow item frame.
    ItemFrame {
        attached: Attached,
        item: Option<ItemStack>,
        /// From 0 to 7.
        rotation: u8,
        drop_chance: Float,
        fixed: bool,
        invisible: bool,
    },
    /// A painting, its variant an index into the painting variants' registry.
    Painting { attached: Attached, variant: u16 },
    /// A cushion, of a dye's colour.
    Cushion { attached: Attached, colour: u8 },
    /// The knot of a leash tied to a fence.
    LeashKnot { attached: Attached },
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
    FallDistance(f64),
    FireTicks(i32),
    Riding(Riding),
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
            (Self::FallDistance(a), Self::FallDistance(b)) => a.to_bits() == b.to_bits(),
            (Self::FireTicks(a), Self::FireTicks(b)) => a == b,
            (Self::Riding(a), Self::Riding(b)) => a == b,
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
            Self::FallDistance(distance) => entity.fall_distance = *distance,
            Self::FireTicks(ticks) => entity.fire_ticks = *ticks,
            Self::Riding(riding) => entity.riding = riding.clone(),
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

/// The slots of a player's ender chest (`PlayerEnderChestContainer`).
pub const ENDER_SLOTS: usize = 27;

/// A player's ender chest: [`ENDER_SLOTS`] slots, which every ender chest opens for them
/// (`docs/groundwork/n6-plan.md`, decision 5). Boxed and serialised as [`Inventory`] is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnderChest(pub Box<[Option<ItemStack>; ENDER_SLOTS]>);

impl Default for EnderChest {
    fn default() -> Self {
        Self(Box::new([None; ENDER_SLOTS]))
    }
}

impl EnderChest {
    /// The slot `slot`, if the chest has one so numbered.
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
}

impl Serialize for EnderChest {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.as_slice().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for EnderChest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let slots = Vec::<Option<ItemStack>>::deserialize(deserializer)?;
        let slots: Box<[Option<ItemStack>; ENDER_SLOTS]> =
            slots.try_into().map_err(|slots: Vec<_>| {
                serde::de::Error::invalid_length(slots.len(), &"the slots of an ender chest")
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
                health: 5,
            },
            fall_distance: 0.0,
            fire_ticks: -1,
            riding: Default::default(),
        };
        EntityField::Position([3.25, -60.0, -0.5]).set(&mut entity);
        EntityField::Age(12).set(&mut entity);
        assert_eq!(entity.position, [3.25, -60.0, -0.5]);
        assert_eq!(entity.age, 12);
        assert_eq!(entity.chunk(), ChunkPos::new(0, -1));
    }
}
