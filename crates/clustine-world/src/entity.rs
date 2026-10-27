//! Entities other than players as their chunks keep them, the changes a tick makes to
//! them, and a player's inventory (`docs/adr/0028-entities.md`, sections 2 and 3;
//! `docs/groundwork/n2-plan.md`).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::projectiles::{Arrow, Cloud, Fangs, Firework, Spit};
use crate::{BlockPos, ChunkPos, Components, EdgeId, EntityId};

/// A number of items of one kind, with the components it carries beyond its item's
/// defaults (`docs/groundwork/n9-plan.md`, section 3.1).
///
/// Not `Copy`, since its components may own lists: a stack that is read is borrowed and
/// one that is given away is moved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemStack {
    /// Id in the item registry.
    pub item: i32,
    pub count: i32,
    pub components: Components,
}

impl ItemStack {
    /// `count` of `item`, with no components of its own.
    pub const fn new(item: i32, count: i32) -> Self {
        ItemStack {
            item,
            count,
            components: Components::NONE,
        }
    }

    /// The stack with `count` in place of its count, its components kept.
    pub fn with_count(&self, count: i32) -> ItemStack {
        ItemStack {
            count,
            ..self.clone()
        }
    }

    /// Whether the two are the same item with equal components, so that they merge and
    /// stack in one slot (`ItemStack.isSameItemSameComponents`): counts are not compared.
    pub fn same_kind(&self, other: &ItemStack) -> bool {
        self.item == other.item && self.components == other.components
    }
}

/// An entity other than a player, as its chunk keeps it.
///
/// Its counters (`lived` and the data's delays) are as of the tick of the record that
/// holds it: a commit, a copy's update or a file. Regions count their ticks each from
/// their own start, so a region that takes an entity in keeps its counters in its own
/// ticks and writes them as of each record again; the game freezes them while an entity
/// does not tick, and so do these (`docs/groundwork/n2-plan.md`, section 11).
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
    /// The ticks it has ticked: never lowered, but by a restore after a crash, which
    /// lives again the ticks since the entity's last record. What it draws in its own
    /// tick is keyed by it, and so are the game's counters that count its ticks
    /// (`docs/groundwork/n7-plan.md`, section 11.3).
    pub lived: u32,
    /// `lived` at its last fresh load, the game's reload of its chunk: the game's
    /// `tickCount` is `lived - fresh_at`, which the game starts again at 0 at every load.
    pub fresh_at: u32,
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

    /// The game's `tickCount`: the ticks it has ticked since its chunk was last loaded
    /// fresh.
    pub fn tick_count(&self) -> u32 {
        self.lived.saturating_sub(self.fresh_at)
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
            && self.lived == other.lived
            && self.fresh_at == other.fresh_at
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

    /// Who is shown riding it: every seat but one granted across a line and not yet
    /// taken, whose player their region has not yet decided to send
    /// (`docs/groundwork/n7j-mount-across-a-line.md`, section 8 item 2).
    pub fn shown_passengers(&self) -> Vec<EntityId> {
        self.passengers
            .iter()
            .filter(|seat| !seat.pending.is_some_and(|pending| pending.granted))
            .map(|seat| seat.key)
            .collect()
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
    /// Granted to a player of another region who clicked the vehicle across a line, and
    /// not taken yet: not shown to anyone, and its first seating a mount
    /// (`docs/groundwork/n7j-mount-across-a-line.md`, section 8).
    pub granted: bool,
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
        /// What `lived` is ahead of its age, the game's `ItemEntity.age`, which a merge
        /// with a younger item lowers: the age is `lived - age_offset`.
        age_offset: u32,
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
        /// A TNT cart's fuse, in ticks, -1 while it is not primed (`TNTFuse`).
        fuse: i32,
        /// Whether a hopper cart pulls, which an activator rail turns off (`Enabled`).
        enabled: bool,
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
    /// Primed TNT (`PrimedTnt`, `docs/groundwork/n6-brief.md`, section 1.7).
    PrimedTnt {
        /// The ticks before it explodes (`fuse`, saved as a short).
        fuse: i16,
        /// The block it shows, a state id (`block_state`).
        block: u16,
        /// Its explosion's power, 4 unless summoned otherwise (`explosion_power`).
        power: Float,
        /// Who primed it (`owner`), if anyone did.
        owner: Option<Uuid>,
    },
    /// A living mob of any type (`Mob` and what derives from it): its body, its mind and
    /// its type's own state, each in the encoding of the code that keeps it, so that a
    /// type's code adds what it keeps without a change to this record
    /// (`docs/groundwork/n7-plan.md`, section 11.3).
    Mob {
        /// The body's parts, ascending by id, each once: its vitals, attributes,
        /// equipment, controls, path and the rest that every mob has.
        body: Vec<Entry>,
        /// The mind's parts, ascending by id, each once: the goal and target selectors'
        /// state, each goal's, and a brain's. Copies of a chunk leave them out, as
        /// nothing reads another mob's mind across a line.
        mind: Vec<Entry>,
        /// What its type keeps beside (a sheep's colour, a cow's variant, love).
        state: TypeState,
    },
    /// An orb of experience (`ExperienceOrb`, `docs/groundwork/n7-brief.md`, section
    /// 1.7): what one is worth, how many of that worth it stands for, what it can still
    /// take, and the player it follows.
    ExperienceOrb {
        /// `DATA_VALUE`: the points each of it gives.
        value: i32,
        /// `count`: the orbs of the same worth merged into it.
        count: i32,
        /// What it can still take before it is destroyed (the game's 5).
        health: i16,
        /// What `lived` is ahead of its age, the game's `age` (gone at 6000), which a
        /// merge with a younger orb lowers: the age is `lived - age_offset`.
        age_offset: u32,
        /// `followingPlayer`: the player's entity it moves to, kept while they are within
        /// 8 blocks.
        following: Option<EntityId>,
    },
    /// A thrown item, the kind its entity type (`ThrowableItemProjectile`; the snowball,
    /// `docs/groundwork/n7k-plan.md`, section 10): what it shows, who threw it, and what
    /// `Projectile` keeps of its flight.
    Thrown {
        /// `DATA_ITEM_STACK`, one of the item thrown.
        stack: ItemStack,
        /// `owner`: the entity that threw it, a player's or a mob's, by key.
        owner: Option<EntityId>,
        /// `leftOwner`: it has been clear of its owner's unit once, and may hit it now.
        left_owner: bool,
        /// `hasBeenShot`: its first tick has posted `projectile_shoot`.
        has_been_shot: bool,
    },
    /// An arrow, a spectral arrow or a trident (`docs/groundwork/n9-plan.md`, 4.2, 4.3).
    Arrow(Box<Arrow>),
    /// The llama's spit (4.4).
    Spit(Spit),
    /// A firework rocket (4.7).
    Firework(Box<Firework>),
    /// An area effect cloud (4.5).
    Cloud(Box<Cloud>),
    /// The evoker's fangs (4.8).
    Fangs(Fangs),
}

/// A part of a mob's body or mind: an id that the code of that part gives it and keeps
/// across builds, the version of that code's encoding, and the bytes. A build that does
/// not know an id drops the part, and reads every earlier version of those it knows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: u16,
    pub version: u8,
    pub bytes: Vec<u8>,
}

/// What a mob's type keeps beside its body and mind, in its own encoding's `version`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeState {
    pub version: u8,
    pub bytes: Vec<u8>,
}

/// Puts `entry` into `entries`, ascending by id, in place of one of the same id.
fn put_entry(entries: &mut Vec<Entry>, entry: &Entry) {
    match entries.binary_search_by_key(&entry.id, |held| held.id) {
        Ok(at) => entries[at] = entry.clone(),
        Err(at) => entries.insert(at, entry.clone()),
    }
}

/// Takes the entry of `id` out of `entries`, if it is there.
fn drop_entry(entries: &mut Vec<Entry>, id: u16) {
    if let Ok(at) = entries.binary_search_by_key(&id, |held| held.id) {
        entries.remove(at);
    }
}

/// What a tick did to an entity of a chunk. Applied in the order a tick made them, after
/// the tick's block changes, and idempotent from any state at or after the last
/// checkpoint, as block changes are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[expect(
    clippy::large_enum_variant,
    reason = "a whole entity is the change of every entity made or taken in, and boxing it \
              would allocate once for each"
)]
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
    Rotation {
        yaw: f32,
        pitch: f32,
    },
    OnGround(bool),
    Lived(u32),
    FreshAt(u32),
    /// Its changes of holder, raised at a take-in without its record whole
    /// (`docs/groundwork/n7-plan.md`, section 10, item 1).
    Hops(u32),
    Data(EntityData),
    /// A part of a mob's body, set; on an entity that is no mob, nothing.
    BodyEntry(Entry),
    /// The part of a mob's body of this id, gone.
    BodyGone(u16),
    /// A part of a mob's mind, set.
    MindEntry(Entry),
    /// The part of a mob's mind of this id, gone.
    MindGone(u16),
    /// A mob's type's own state.
    TypeState(TypeState),
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
            (Self::Lived(a), Self::Lived(b))
            | (Self::FreshAt(a), Self::FreshAt(b))
            | (Self::Hops(a), Self::Hops(b)) => a == b,
            (Self::Data(a), Self::Data(b)) => a == b,
            (Self::BodyEntry(a), Self::BodyEntry(b)) | (Self::MindEntry(a), Self::MindEntry(b)) => {
                a == b
            }
            (Self::BodyGone(a), Self::BodyGone(b)) | (Self::MindGone(a), Self::MindGone(b)) => {
                a == b
            }
            (Self::TypeState(a), Self::TypeState(b)) => a == b,
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
            Self::Lived(lived) => entity.lived = *lived,
            Self::FreshAt(fresh_at) => entity.fresh_at = *fresh_at,
            Self::Hops(hops) => entity.hops = *hops,
            Self::Data(data) => entity.data = data.clone(),
            Self::BodyEntry(entry) => {
                if let EntityData::Mob { body, .. } = &mut entity.data {
                    put_entry(body, entry);
                }
            }
            Self::BodyGone(id) => {
                if let EntityData::Mob { body, .. } = &mut entity.data {
                    drop_entry(body, *id);
                }
            }
            Self::MindEntry(entry) => {
                if let EntityData::Mob { mind, .. } = &mut entity.data {
                    put_entry(mind, entry);
                }
            }
            Self::MindGone(id) => {
                if let EntityData::Mob { mind, .. } = &mut entity.data {
                    drop_entry(mind, *id);
                }
            }
            Self::TypeState(type_state) => {
                if let EntityData::Mob { state, .. } = &mut entity.data {
                    *state = type_state.clone();
                }
            }
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
        Self(Box::new([const { None }; INVENTORY_SLOTS]))
    }
}

impl Inventory {
    /// The inventory with `hotbar` in its first nine slots and nothing else.
    pub fn with_hotbar(hotbar: [Option<ItemStack>; HOTBAR_SLOTS]) -> Self {
        let mut inventory = Self::default();
        for (slot, stack) in inventory.0.iter_mut().zip(hotbar) {
            *slot = stack;
        }
        inventory
    }

    /// The hotbar's nine slots.
    pub fn hotbar(&self) -> [Option<ItemStack>; HOTBAR_SLOTS] {
        std::array::from_fn(|slot| self.0[slot].clone())
    }

    /// The slot `slot`, if the inventory has one so numbered.
    pub fn get(&self, slot: usize) -> Option<Option<ItemStack>> {
        self.0.get(slot).cloned()
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
        Self(Box::new([const { None }; ENDER_SLOTS]))
    }
}

impl EnderChest {
    /// The slot `slot`, if the chest has one so numbered.
    pub fn get(&self, slot: usize) -> Option<Option<ItemStack>> {
        self.0.get(slot).cloned()
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
    fn stacks_are_of_one_kind_where_item_and_components_are_the_same_whatever_their_counts() {
        let plain = ItemStack::new(3, 1);
        assert!(plain.same_kind(&ItemStack::new(3, 64)));
        assert!(!plain.same_kind(&ItemStack::new(4, 1)));
        let mut worn = plain.with_count(5);
        worn.components.set(crate::Component::Damage(1));
        assert!(!plain.same_kind(&worn));
        assert!(worn.same_kind(&worn.with_count(1)));
        assert_eq!(worn.with_count(1).components, worn.components);
    }

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
        inventory.set(40, Some(ItemStack::new(3, 1)));
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
            lived: 0,
            fresh_at: 0,
            data: EntityData::Item {
                stack: ItemStack::new(1, 1),
                pick_up_delay: 40,
                thrower: None,
                health: 5,
                age_offset: 0,
            },
            fall_distance: 0.0,
            fire_ticks: -1,
            riding: Default::default(),
        };
        EntityField::Position([3.25, -60.0, -0.5]).set(&mut entity);
        EntityField::Lived(12).set(&mut entity);
        EntityField::FreshAt(10).set(&mut entity);
        EntityField::Hops(3).set(&mut entity);
        assert_eq!(entity.position, [3.25, -60.0, -0.5]);
        assert_eq!((entity.lived, entity.tick_count(), entity.hops), (12, 2, 3));
        assert_eq!(entity.chunk(), ChunkPos::new(0, -1));
    }

    #[test]
    fn a_mob_s_parts_are_set_in_order_of_their_ids_and_dropped_by_id_and_nothing_else_has_them() {
        let entry = |id: u16, byte: u8| Entry {
            id,
            version: 1,
            bytes: vec![byte],
        };
        let mut mob = Entity {
            key: EntityId(9),
            kind: 30,
            uuid: Uuid::from_u128(1),
            position: [0.5, -60.0, 0.5],
            motion: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
            hops: 0,
            lived: 0,
            fresh_at: 0,
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
            data: EntityData::Mob {
                body: Vec::new(),
                mind: Vec::new(),
                state: TypeState::default(),
            },
        };
        for field in [
            EntityField::BodyEntry(entry(5, 1)),
            EntityField::BodyEntry(entry(2, 2)),
            EntityField::BodyEntry(entry(5, 3)),
            EntityField::MindEntry(entry(7, 4)),
            EntityField::MindEntry(entry(1, 5)),
            EntityField::MindGone(7),
            EntityField::BodyGone(4),
            EntityField::TypeState(TypeState {
                version: 2,
                bytes: vec![6],
            }),
        ] {
            field.set(&mut mob);
        }
        assert_eq!(
            mob.data,
            EntityData::Mob {
                body: vec![entry(2, 2), entry(5, 3)],
                mind: vec![entry(1, 5)],
                state: TypeState {
                    version: 2,
                    bytes: vec![6],
                },
            }
        );
        // An item takes none of it.
        let mut item = Entity {
            data: EntityData::Item {
                stack: ItemStack::new(1, 1),
                pick_up_delay: 0,
                thrower: None,
                health: 5,
                age_offset: 0,
            },
            ..mob
        };
        let before = item.clone();
        EntityField::BodyEntry(entry(1, 1)).set(&mut item);
        EntityField::MindGone(1).set(&mut item);
        assert_eq!(item, before);
    }
}
