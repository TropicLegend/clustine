//! What goes into a region's tick and what comes out of it.
//!
//! These types cross the boundary between the worker and the edge, so they have to stay
//! serialisable and free of anything specific to the Minecraft protocol.

use clustine_data::{BlockState, Swing};
use clustine_world::{
    Arrival, BlockPos, Change, Chunk, ChunkPos, EdgeId, Effect, EffectId, Entity, EntityChange,
    EntityData, EntityId, FiledEffect, PlayerId, RegionId, Vec3,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::placement::Click;
use crate::state::StateDelta;

/// Where an entity is and how it is oriented.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pose {
    /// The position of the feet.
    pub position: Vec3,
    /// Horizontal rotation in degrees; 0 looks towards positive z.
    pub yaw: f32,
    /// Vertical rotation in degrees; negative looks up.
    pub pitch: f32,
    pub on_ground: bool,
}

impl Pose {
    /// Standing at `position`, looking straight ahead towards positive z.
    pub const fn at(position: Vec3) -> Self {
        Self {
            position,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
        }
    }
}

pub use clustine_world::{
    ENDER_SLOTS, EnderChest, HOTBAR_SLOTS, INVENTORY_SLOTS, Inventory, ItemStack, OFF_HAND,
};

/// A side of a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Face {
    /// Towards negative y.
    Bottom,
    /// Towards positive y.
    Top,
    /// Towards negative z.
    North,
    /// Towards positive z.
    South,
    /// Towards negative x.
    West,
    /// Towards positive x.
    East,
}

impl Face {
    /// The block that touches `position` on this side.
    pub fn neighbour(self, position: BlockPos) -> BlockPos {
        match self {
            Self::Bottom => position.offset(0, -1, 0),
            Self::Top => position.offset(0, 1, 0),
            Self::North => position.offset(0, 0, -1),
            Self::South => position.offset(0, 0, 1),
            Self::West => position.offset(-1, 0, 0),
            Self::East => position.offset(1, 0, 0),
        }
    }
}

/// A player's experience (`Player.experienceLevel`, `experienceProgress`,
/// `totalExperience`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Experience {
    pub level: i32,
    /// Towards the next level, from 0 to 1.
    pub progress: f32,
    pub total: i32,
}

/// Where a player is and what they hold: what the world store keeps of a player from
/// one stay to the next. See `docs/adr/0020-one-stay-per-player.md`, section 2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub pose: Pose,
    pub flying: bool,
    pub inventory: Inventory,
    pub selected_slot: u8,
    /// What their ender chest holds (`docs/groundwork/n6-plan.md`, decision 5).
    pub ender: EnderChest,
    pub experience: Experience,
    /// Where they wake after a death, if a bed set it (`docs/groundwork/n7d-plan.md`).
    pub respawn: Option<Respawn>,
    /// Their effects, each level with the ticks it has left, in the order of the effects
    /// and of the levels they hide (`docs/groundwork/n9-plan.md`, section 5.4): an
    /// effect does not run down while they are away.
    pub effects: Vec<clustine_world::components::EffectInstance>,
    /// The ticks since they last slept (`TIME_SINCE_REST`), which the phantoms read.
    pub time_since_rest: u32,
}

/// A player asleep in a bed: its head, and the game time of their region they lay down
/// at (`docs/groundwork/n7d-plan.md`, sections 1 and 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sleep {
    pub head: BlockPos,
    pub since: u64,
}

/// A player's respawn point as a bed sets it (`ServerPlayer.RespawnConfig`, never forced):
/// the bed's head, and the look they had.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Respawn {
    pub pos: BlockPos,
    pub yaw: f32,
    pub pitch: f32,
}

/// What a region says to the world store about a stay, in the commit of a tick. See
/// `docs/adr/0020-one-stay-per-player.md`, section 3, and [`TickOutput::stays`] for
/// which tick makes which.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[expect(
    clippy::large_enum_variant,
    reason = "a tick has a few notes at most, and a `Has` carries a whole inventory"
)]
pub enum StayNote {
    /// The home region has given `player` the stay `entity` and holds it as entering,
    /// or still holds it so and names it again.
    Entering { player: PlayerId, entity: EntityId },
    /// The stay is in this region after the tick, or was let go in it (`let_go`), with
    /// `hops` hand-overs behind it, this place and this face.
    Has {
        player: PlayerId,
        entity: EntityId,
        hops: u32,
        place: Place,
        face: PlayerFace,
        let_go: bool,
        /// The vehicle the player left the world on, with all its passengers, root
        /// first, taken out of the world with them for their next stay
        /// (`docs/groundwork/n7h-logout-on-a-mount.md`, section 3).
        unit: Option<Unit>,
    },
    /// The home region took the unit whose root is `root` out of the player's record, to
    /// put it back under their stay that is entering (the same design, section 5). No
    /// stay of the notes' map: it follows them in a commit, and the store takes it before
    /// them (section 13 item 3).
    Took { player: PlayerId, root: EntityId },
    /// The stay is no more: its player left, their edge went with them on the way to
    /// another region, or an edge said that a stay let go will arrive nowhere. The
    /// world store judges whether it ends the player's time in the world
    /// (`docs/groundwork/n7-plan.md`, sections 8.3.2, 9.2.4 and 10.4). A player's `Gone`
    /// is the last of their notes in a commit, behind the `Has` of a ride they left.
    Gone {
        player: PlayerId,
        entity: EntityId,
        hops: u32,
    },
}

impl StayNote {
    /// The `Has` of a stay held at `place`, its player standing and riding nothing.
    pub fn held(player: PlayerId, entity: EntityId, hops: u32, place: Place) -> Self {
        Self::Has {
            player,
            entity,
            hops,
            face: PlayerFace::standing(&place),
            place,
            let_go: false,
            unit: None,
        }
    }

    /// The player the note is of.
    pub fn player(&self) -> PlayerId {
        match self {
            Self::Entering { player, .. }
            | Self::Has { player, .. }
            | Self::Took { player, .. }
            | Self::Gone { player, .. } => *player,
        }
    }
}

/// A vehicle and everything that rides it, root first, as a player takes it out of the
/// world and back (`docs/groundwork/n7h-logout-on-a-mount.md`). Two are equal if they
/// serialise alike, entities having no equality of their own.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Unit(pub Vec<Entity>);

impl Unit {
    /// Its root, the vehicle the others ride.
    pub fn root(&self) -> &Entity {
        self.0.first().expect("a unit has its root")
    }
}

impl PartialEq for Unit {
    fn eq(&self, other: &Self) -> bool {
        postcard::to_allocvec(self).ok() == postcard::to_allocvec(other).ok()
    }
}

/// What a mob sees of a player another region holds, or that this one let go a moment
/// ago: the store keeps the latest of every player and streams it to the regions that
/// copy or hold its chunk (`docs/groundwork/n7-plan.md`, sections 7.3 and 11.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerFace {
    /// Their feet.
    pub position: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub eye_height: f32,
    pub main_hand: Option<ItemStack>,
    pub off_hand: Option<ItemStack>,
    /// Feet, legs, chest and head.
    pub worn: [Option<ItemStack>; 4],
    pub sneaking: bool,
    /// The game mode's id: 1, creative, for everyone until survival.
    pub game_mode: u8,
    /// The game time they lay down at, while they sleep (their region's clock), from
    /// which their sleep counter is reckoned.
    pub sleeping: Option<u64>,
    /// What they ride.
    pub vehicle: Option<EntityId>,
    pub alive: bool,
    /// The mob they last hurt and when, in game time.
    pub last_hurt_mob: Option<(EntityId, u64)>,
    /// The mob that last hurt them and when, in game time.
    pub last_hurt_by: Option<(EntityId, u64)>,
    /// `hasMovedHorizontallyRecently`.
    pub moved_horizontally: bool,
    /// `isSprinting` (an armadillo's scare reads it).
    pub sprinting: bool,
}

impl PlayerFace {
    /// What mobs see of a player standing so and holding this (the view's `SeenPlayer`).
    pub fn of(
        pose: &Pose,
        sneaking: bool,
        flying: bool,
        inventory: &Inventory,
        selected_slot: u8,
        vehicle: Option<EntityId>,
    ) -> Self {
        let slot = |index: usize| inventory.0[index].clone();
        Self {
            position: [pose.position.x, pose.position.y, pose.position.z],
            yaw: pose.yaw,
            pitch: pose.pitch,
            eye_height: if sneaking && !flying {
                CROUCHING_EYE_HEIGHT
            } else {
                EYE_HEIGHT
            },
            main_hand: slot(usize::from(selected_slot)),
            off_hand: slot(OFF_HAND),
            worn: std::array::from_fn(|index| slot(clustine_world::ARMOUR_START + index)),
            sneaking,
            // Everyone is in creative mode until survival.
            game_mode: 1,
            sleeping: None,
            vehicle,
            alive: true,
            last_hurt_mob: None,
            last_hurt_by: None,
            moved_horizontally: false,
            sprinting: false,
        }
    }

    /// The face of a player at their place, standing and riding nothing.
    pub fn standing(place: &Place) -> Self {
        Self::of(
            &place.pose,
            false,
            place.flying,
            &place.inventory,
            place.selected_slot,
            None,
        )
    }
}

/// How far above their feet a player's eyes are, standing and crouching
/// (`Player.STANDING_DIMENSIONS`, `CROUCHING_DIMENSIONS`).
const EYE_HEIGHT: f32 = 1.62;
const CROUCHING_EYE_HEIGHT: f32 = 1.27;

/// The place of a face in the order the store took it: the sequence of the commit that
/// wrote it in the log, and the index of its note in the commit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FaceSeq(pub u64, pub u32);

/// A face or its removal, as the world store streams them, with the stay they are of.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[expect(
    clippy::large_enum_variant,
    reason = "nearly every change is a face, whose six stacks grew with their components; \
              boxing it would allocate once for each"
)]
pub enum FaceChange {
    Face {
        player: PlayerId,
        entity: EntityId,
        hops: u32,
        seq: FaceSeq,
        face: PlayerFace,
    },
    /// The face of the stay at `seq` and every earlier face are gone: the player moved to
    /// a chunk the region neither holds nor copies, or left the world.
    Removed {
        player: PlayerId,
        entity: EntityId,
        hops: u32,
        seq: FaceSeq,
    },
    /// The faces that follow are those of `chunks` (every chunk where `None`) as the
    /// store has them now: a face the region keeps there at or below `seq` is out of
    /// date, as the player may have left it while the region was not told.
    Base {
        chunks: Option<Vec<ChunkPos>>,
        seq: FaceSeq,
    },
}

/// A player of the world as the store publishes them once a second, for the rules that
/// count every player (sleep, the 32 and 128 blocks, owners beyond the copies).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldPlayer {
    pub player: PlayerId,
    pub entity: EntityId,
    pub position: [f64; 3],
    pub game_mode: u8,
    /// Their sleep counter (`sleepCounter`, 0 to 100) while they sleep, as the store
    /// reckons it from their region's last commit: low by up to a second.
    pub sleeping: Option<u32>,
}

/// The world store's answer to a [`StayNote::Entering`]: the stay `entity` of `player`
/// may enter. `place` is where the player was last, if anywhere, and `holder` the
/// region that held the chunk of that place when the store took the note, if one did,
/// which is advice and no more. See `docs/adr/0020-one-stay-per-player.md`, section 4.
///
/// A region does not know its own id, so whoever makes this for it says `None` for a
/// place the region itself was holding: a `holder` is always another region to the
/// tick, which lets the stay go there without placing it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entered {
    pub player: PlayerId,
    pub entity: EntityId,
    pub place: Option<Place>,
    pub holder: Option<RegionId>,
    /// The vehicle the player left the world on, to be put back on
    /// (`docs/groundwork/n7h-logout-on-a-mount.md`, section 5).
    pub unit: Option<Unit>,
}

/// A player entering the region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerJoin {
    pub player: PlayerId,
    pub name: String,
    /// Which of the edge's connections the join is of: the edge numbers them from 1
    /// with every start. The stay the join begins carries it until its first input is
    /// applied, and every answer to the join names it, so that an edge which has not
    /// been told the stay's entity can tell the stay of this connection from one of an
    /// earlier connection of the same player. See
    /// `docs/adr/0020-one-stay-per-player.md`, sections 4.1 and 4.3.
    pub attempt: u64,
}

/// A player as one region hands them to another: everything a region knows about them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerTransfer {
    /// The player keeps their entity, so that nobody watching sees them vanish.
    pub entity_id: EntityId,
    pub name: String,
    pub pose: Pose,
    pub inventory: Inventory,
    pub selected_slot: u8,
    /// What their ender chest holds, which goes where they go.
    pub ender: EnderChest,
    /// The number of the last input the region applied; see [`TickInputs::inputs`].
    pub last_input: u64,
    /// How often the stay has been handed on from one region to another, this time
    /// counted: a region that lets a player go, and a split for those who go with it,
    /// each add one. Of two copies of a stay the one with more is the later. See
    /// `docs/adr/0020-one-stay-per-player.md`, section 6.
    pub hops: u32,
    /// Whether the player flies; see [`PlayerInput::SetFlying`].
    pub flying: bool,
    /// [`PlayerJoin::attempt`] of the join that began the stay, for as long as no
    /// input of the stay has been applied by any region.
    pub attempt: Option<u64>,
    /// What the player rides, handed on with them when their unit crossed a line: the
    /// unit's new chunk, which is their holding chunk, and that they are on their way
    /// to it (`docs/groundwork/n5-plan.md`, section 4 and 7).
    pub riding: Option<crate::state::PlayerRiding>,
    /// The keys they hold, which their client says only when they change.
    pub keys: Keys,
    /// Moves of their vehicle kept for them; see [`crate::PlayerState::kept`].
    pub kept: Vec<VehicleMove>,
    pub experience: Experience,
    /// The ticks since their attack strength was last reset (`attackStrengthTicker`),
    /// which the next region counts on from (`docs/groundwork/n7-plan.md`, 7.7.2); none
    /// where it never was.
    pub attack_ticks: Option<u32>,
    pub sleeping: Option<Sleep>,
    pub respawn: Option<Respawn>,
    /// The use under way, which goes on in the next region (the client does not see a
    /// hand-over).
    pub using: Option<Using>,
    /// Each cooldown's group, the ticks it has run and the ticks it lasts, which the
    /// next region counts on from.
    pub cooldowns: Vec<(String, u32, u32)>,
    /// Their effects with the ticks each level has left, and the ticks since they last
    /// slept, as the place has them.
    pub effects: Vec<clustine_world::components::EffectInstance>,
    pub time_since_rest: u32,
}

/// The number of spawning categories counted: `MobCategory` but `MISC`, in its order.
pub const SPAWN_CATEGORIES: usize = 7;

/// A chunk's or a region's mobs by spawning category.
pub type SpawnCounts = [u32; SPAWN_CATEGORIES];

/// A tick's counts of mobs (`docs/groundwork/n9-plan.md`, section 9.2): each chunk's,
/// whole, and whether it is every loaded chunk's (a region's first tick after it is made
/// or restored), which replaces what the store kept of the region.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CountsReport {
    pub full: bool,
    pub chunks: Vec<(ChunkPos, SpawnCounts)>,
}

/// What the store published of the other regions' mobs (section 9.2): their totals by
/// category, and each world player's counts over the other regions' chunks within 128
/// blocks of them, reckoned at the positions it published with them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedCounts {
    pub totals: SpawnCounts,
    pub players: Vec<(PlayerId, SpawnCounts)>,
}

/// A hand (`InteractionHand`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Hand {
    Main,
    Off,
}

/// A use of an item under way (`LivingEntity.useItem` and `useItemRemaining`,
/// `docs/groundwork/n9-plan.md`, section 5.5): the hand it is made with, the item it
/// was begun with (the use stops when the hand holds another: `isSameItem`), and the
/// ticks it has left. The stack is read from the hand, not copied: a crossbow's charge
/// is written into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Using {
    pub hand: Hand,
    pub item: i32,
    pub remaining: i32,
}

/// An item group's cooldown (`ItemCooldowns`), by the group's name, from the tick it
/// began in to the tick it ends in, on the region's ticks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cooldown {
    pub group: String,
    pub start: u64,
    pub end: u64,
}

/// A player entering or leaving the region. Each but `Discard` names the edge it came
/// from, which is not on the wire: the runner knows which edge a link belongs to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[expect(
    clippy::large_enum_variant,
    reason = "a tick has a few changes of players at most, and an arrival carries a whole player"
)]
pub enum PlayerChange {
    /// A player enters the world through the edge, and is that edge's from then on.
    ///
    /// A join begins a new stay whatever the region has: a stay is a player's time in
    /// the world from one join to the leave that ends it, and has one entity for all of
    /// it. If the region has the player already, under this edge or another, they have
    /// connected anew: the entity they had is reported removed and they enter the world
    /// afresh. A join through an edge the region does not know is ignored. See
    /// `docs/adr/0014-merging-and-splitting.md`, section 2.1.
    Join(EdgeId, PlayerJoin),
    /// A player's connection through the edge has ended. It removes the player only if
    /// they are that edge's: what another edge says is about an earlier connection,
    /// which must not end the current one.
    ///
    /// The entity names the stay that has ended: the player is removed only if they
    /// have that entity, as a leave that comes late must not end a later stay.
    ///
    /// The last is the attempt of the connection that ended, which an edge names where
    /// it names no entity: the edge has not been told the entity of a player who is
    /// still entering the world. Such a leave ends only a stay of that edge that
    /// carries that attempt, one the region has or one it holds as entering. A stay
    /// carries its attempt until its first input is applied, and an edge sends no
    /// input of a stay whose entity it does not know, so the leave and its stay meet.
    /// A leave that names neither names no stay and ends none. See
    /// `docs/adr/0020-one-stay-per-player.md`, section 4.4.
    Leave(EdgeId, PlayerId, Option<EntityId>, Option<u64>),
    /// A player comes in from another region, as that region let them go with
    /// [`Durable::Departed`], and is the edge's from then on.
    ///
    /// Of two stays of a player the one with the higher entity id is the later, so an
    /// arrival takes the place of a player the region has with a lower entity id: the
    /// entity that was there is reported removed where it stood. A player the region
    /// has with a higher entity id stays as they are, and the entity that was on its
    /// way is reported removed; so is the entity of an arrival through an edge the
    /// region does not know, as nobody could be told about the player.
    ///
    /// Of two copies of one stay the one that was handed on more often is the later
    /// ([`PlayerTransfer::hops`]). An arrival of the stay the region has, as often
    /// handed on or less, changes nothing; one that was handed on more often takes the
    /// place of the copy that is there, and nothing is reported removed, as the entity
    /// lives on. See `docs/adr/0020-one-stay-per-player.md`, section 6.
    ///
    /// An arrival is also passed over if the region holds a later stay of the player
    /// as entering, and whoever loses to a later stay has their edge told with
    /// [`Durable::Ended`]: the edge of the stay that was there, or the edge an arrival
    /// that is passed over came through.
    ///
    /// A player who arrives in a chunk the region believes another to hold is not taken
    /// in: the arrival goes on to that region with a [`Durable::NotMine`]. Not so in
    /// the areas the region is pinned to, where it drops the belief instead. Whatever
    /// else the region knows of the chunk, it takes the player in, and claims the chunk
    /// if it knows nothing of it.
    Arrive(EdgeId, PlayerId, PlayerTransfer),
    /// An entity that another region let go will not arrive anywhere, because its player
    /// left in the meantime. It is reported as removed to those watching `chunk`, where
    /// it was seen last. The region notes the stay `Gone`, with the hand-overs of its
    /// transfer, unless it holds that stay or a later one of the player.
    Discard {
        player: PlayerId,
        entity: EntityId,
        hops: u32,
        chunk: ChunkPos,
    },
}

/// What kind of thing an entity is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EntityKind {
    Player {
        player: PlayerId,
        name: String,
    },
    /// An entity other than a player (`docs/adr/0028-entities.md`): its type, an index
    /// into `clustine_data::ENTITY_TYPES`, its UUID, its motion, its changes of holder
    /// and what it is beside that.
    Other {
        kind: u16,
        uuid: Uuid,
        motion: [f64; 3],
        hops: u32,
        /// Its ticks lived as shown, which an edge that meets it after it was made
        /// reckons a cloud's radius and wait by (`docs/groundwork/n9-plan.md`, section
        /// 10).
        lived: u32,
        data: EntityData,
        /// What it rides, who rides it and who holds its leash, so that a client shown
        /// it from a snapshot is shown those too.
        riding: clustine_world::Riding,
    },
}

/// Everything needed to show an entity to someone who has not seen it before.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityState {
    pub entity: EntityId,
    pub kind: EntityKind,
    pub pose: Pose,
}

impl EntityState {
    /// The chunk the entity is in.
    pub fn chunk(&self) -> ChunkPos {
        ChunkPos::containing(self.pose.position.x, self.pose.position.z)
    }
}

/// Something a player did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlayerInput {
    /// The player moved, turned, or left or touched the ground. Parts that did not
    /// change are absent.
    Move {
        position: Option<Vec3>,
        /// Yaw and pitch in degrees.
        rotation: Option<(f32, f32)>,
        on_ground: bool,
    },
    /// The player broke a block. `sequence` numbers the changes the player's client has
    /// already shown on its own; see [`PlayerEvent::Acknowledged`].
    Dig { position: BlockPos, sequence: i32 },
    /// The player used the item in their hand on `face` of the block at `position`,
    /// which uses the block (a door) or places the item's block against that face.
    /// `cursor` is where on the block the click landed, each from 0 to 1; `sneaking`
    /// whether the player held the sneak key, which places a block in hand against a
    /// door rather than opening it. `sequence` is as for `Dig`. See
    /// `docs/groundwork/w6-plan.md`, section 3.
    UseItemOn {
        position: BlockPos,
        face: Face,
        cursor: [f32; 3],
        sneaking: bool,
        sequence: i32,
    },
    /// The player selected another hotbar slot, from 0 to 8.
    SelectSlot { slot: u8 },
    /// A creative-mode player put a stack into a slot of their inventory, numbered as
    /// [`Inventory`] numbers them, or emptied it.
    SetInventorySlot { slot: u8, stack: Option<ItemStack> },
    /// The player used the item in their hand without a block it uses on: a bucket or
    /// a lily pad, which find what they reach by a ray along `rotation` (yaw, pitch),
    /// the player's rotation from then on. `sneaking` and `sequence` are as for
    /// `UseItemOn`. See `docs/groundwork/w6-plan.md`, section 5.
    UseItem {
        rotation: (f32, f32),
        sneaking: bool,
        sequence: i32,
    },
    /// The player began or stopped flying. The region keeps it, hands it on with the
    /// player and tells the world store with their place; nobody else is shown it. See
    /// `docs/adr/0020-one-stay-per-player.md`, section 13.
    SetFlying { flying: bool },
    /// The player swapped what they hold in the main hand with what they hold in the
    /// off hand (the game's `SWAP_ITEM_WITH_OFFHAND`, the key F). Added at the end, so
    /// that what is stored of inputs reads as it did.
    SwapHands,
    /// A use (`UseItemOn` or `UseItem`) made with the off hand, which a client sends when
    /// the same use with the main hand did nothing. Any other input so wrapped is passed
    /// over.
    WithOffHand(Box<PlayerInput>),
    /// The player threw what they hold in the main hand (the game's `DROP_ITEM`, Q, and
    /// `DROP_ALL_ITEMS`, ctrl+Q): one of it, or the whole stack if `all`. `crouching` is
    /// whether the sneak key is down, which lowers the eyes the item leaves from. The
    /// client takes it out of the slot itself; a throw the region refuses puts the slot
    /// back with a [`PlayerEvent::InventorySlot`] (`docs/groundwork/n2-plan.md`, section
    /// 11). Added at the end, so that what is stored of inputs reads as it did.
    Drop { all: bool, crouching: bool },
    /// A creative-mode player threw `stack` out of the inventory screen (the game's
    /// `SetCreativeModeSlot` with a negative slot), which takes nothing from the
    /// inventory. The edge passes on only what the game's throttler of such throws lets
    /// through.
    Throw { stack: ItemStack },
    /// The keys the player holds, as `player_input` says each time they change (the
    /// shift dismount and a minecart's rider read them; `docs/groundwork/n5-plan.md`,
    /// section 7, item 8). Sprinting comes with them, from `player_command` too.
    Keys(Keys),
    /// The player used the entity `entity` (`interact`): `location` is where on it the
    /// click landed, relative to its position, and `sneaking` the packet's own flag, which
    /// the game sets on the player before anything else. Made with the off hand it comes
    /// wrapped in [`PlayerInput::WithOffHand`].
    Interact {
        entity: EntityId,
        location: [f64; 3],
        sneaking: bool,
    },
    /// The player hit the entity `entity` (`attack`).
    Attack { entity: EntityId },
    /// Where the player's client put the vehicle it drives (`move_vehicle`).
    MoveVehicle(VehicleMove),
    /// Which of a driven boat's paddles move (`paddle_boat`).
    PaddleBoat { left: bool, right: bool },
    /// The player picked the item of the entity `entity` (`pick_item_from_entity`, a
    /// middle click): answered by the player's region from its record or its copy of
    /// the entity, which selects the item in the hotbar or, for a creative player, puts
    /// it there (`docs/groundwork/n5-plan.md`, section 8 item 7). Added at the end, so
    /// that what is stored of inputs reads as it did.
    PickItemFromEntity { entity: EntityId },
    /// A click in the menu the player's client shows, of the serial `serial` (the
    /// number of the input that opened it, [`PlayerEvent::MenuOpened`]): `slot` is the
    /// menu's slot, -999 outside the window and -1 its margin, `button` and `kind` as
    /// the game's `clicked` takes them. The client's hashes and state id stay at the
    /// edge. A click of another serial than the open menu's is refused and answered
    /// with the menu whole, or with none (`docs/groundwork/n6-plan.md`, sections 7 and
    /// 8). Added at the end, as every input of N6 is.
    ContainerClick {
        serial: u64,
        slot: i16,
        button: i8,
        kind: ClickKind,
    },
    /// The player's client closed the menu of the serial `serial`.
    CloseContainer { serial: u64 },
    /// The player asked for the inventory of the vehicle they ride (`player_command`'s
    /// `OPEN_INVENTORY`, a chest boat's or a chest cart's key).
    OpenVehicleInventory,
    /// The player asked to leave their bed (`player_command`'s `STOP_SLEEPING`).
    LeaveBed,
    /// The player began the jump of what they ride with a charge of `power`, 0 to 100
    /// (`player_command`'s `START_RIDING_JUMP`: a horse's jump bar).
    RidingJump { power: i32 },
    /// The edge's link to the region was resumed, and player events may have been lost
    /// on it: the region answers with the player's menu whole, or with
    /// [`PlayerEvent::NoMenu`] (section 7 item 7). An input, so that the region stays a
    /// function of its inputs.
    MenuResync,
    /// The player swung their arm at nothing they hit (`punch`): at air, or at a block
    /// they begin to break. Everyone else near sees the arm swing.
    Punch,
    /// The player let go of the use key (`RELEASE_USE_ITEM`): a bow is loosed, a
    /// trident thrown, a meal broken off (`docs/groundwork/n9-plan.md`, section 5.5).
    ReleaseUse,
}

/// The kinds of click in a menu (the game's `ContainerInput`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClickKind {
    Pickup,
    QuickMove,
    Swap,
    Clone,
    Throw,
    QuickCraft,
    PickupAll,
}

/// The kinds of menu of N6, each a kind of window on the client (`minecraft:menu`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MenuKind {
    /// Three rows of nine (`generic_9x3`): a chest, a barrel, an ender chest, a chest
    /// boat or a chest cart.
    Rows3,
    /// Six rows of nine (`generic_9x6`): a double chest.
    Rows6,
    /// A shulker box's three rows (`shulker_box`).
    ShulkerBox,
    /// Three by three (`generic_3x3`): a dispenser or a dropper.
    Grid3x3,
    /// A hopper's five (`hopper`), a hopper cart's too.
    Hopper,
    /// A horse's inventory (`HorseInventoryMenu`, opened by `mount_screen_open`): its
    /// saddle, its body's armour or carpet, then three rows of `columns` of its chest
    /// (none without one). `entity` is the horse, whose id the client is told. Added at
    /// the end, so that a menu kept by an older build reads as it did.
    Mount { columns: u8, entity: EntityId },
}

impl MenuKind {
    /// The slots of the container the menu shows, before the player's inventory's 36.
    pub fn container_slots(self) -> usize {
        match self {
            Self::Rows3 | Self::ShulkerBox => 27,
            Self::Rows6 => 54,
            Self::Grid3x3 => 9,
            Self::Hopper => 5,
            Self::Mount { columns, .. } => 2 + usize::from(columns) * 3,
        }
    }

    /// All the menu's slots: the container's, then the main inventory's 27 and the
    /// hotbar's 9, as every menu of N6 lays them out.
    pub fn slots(self) -> usize {
        self.container_slots() + 36
    }
}

/// The keys a player holds (`player_input`'s flags, and `player_command`'s sprinting).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keys {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub sneak: bool,
    pub sprint: bool,
}

/// A vehicle's place as its driver's client sends it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VehicleMove {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
}

/// What is left to do of something a player did to blocks that the region the player is
/// in does not have. It goes from region to region, each doing the part that concerns
/// its own blocks, until it has been dealt with.
///
/// A region's players can reach a few blocks beyond where the region ends. What they do
/// there is for the region that has those blocks to decide, as it alone knows them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteAction {
    /// Who did it.
    pub player: PlayerId,
    /// The entity of the stay they did it in. A client numbers its actions afresh with
    /// every connection, so the number alone does not say which stay an answer is for.
    /// See `docs/adr/0020-one-stay-per-player.md`, section 11.
    pub entity: EntityId,
    /// The number the player's client gave the action. Nobody acknowledges it to the
    /// player before the action has been dealt with; see [`Durable::RemoteDone`].
    pub sequence: i32,
    pub step: RemoteStep,
}

/// The next step of a [`RemoteAction`]. The player's region has found the player to be
/// within reach and, for placing, to hold the block.
///
/// The first three are what regions passed on before clicks carried what vanilla
/// decides by; a region still takes them, from a state stored by an older build, as
/// it did then. New variants are added at the end, so that what is stored reads as
/// it did (`docs/groundwork/w6-plan.md`, section 4.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RemoteStep {
    /// Break the block at `position`.
    Break { position: BlockPos },
    /// Place `block` at `target`, provided there is a block at `against` to place it
    /// against. `placer` is where the player's feet are, who must not be built into.
    PlaceAgainst {
        against: BlockPos,
        target: BlockPos,
        block: BlockState,
        placer: Vec3,
    },
    /// Place `block` at `target`, provided the spot is free. The block it is placed
    /// against has been found to be there.
    Place {
        target: BlockPos,
        block: BlockState,
        placer: Vec3,
    },
    /// A click on the block at `click.position`, for the region that holds it to use
    /// the block, or to find where a block is placed and place it or pass it on.
    UseOn { click: Click },
    /// Place the item of `click` at `target`, which is not the clicked block. The
    /// clicked block was `clicked` when the region that holds it read it: the region
    /// that holds `target` places against that unless it has the clicked block itself.
    PlaceAt {
        click: Click,
        target: BlockPos,
        clicked: BlockState,
    },
    /// A click as `UseOn`'s, made with the off hand, whose item `click` holds: as the game's
    /// `ServerPlayerGameMode.useItemOn` for the off hand, the clicked block is not used
    /// without its item (`useWithoutItem` is for the main hand only); in Clustine the
    /// block is not used at all, and the item is placed, the uses of a block with an item
    /// in the off hand (a flower into a pot) being a difference to mend when they matter.
    /// Added at the end, so that what is stored of remote actions reads as it did.
    OffHandOn { click: Click },
}

impl RemoteStep {
    /// The block this step is about: the region that has it is the one to take the step.
    pub fn concerns(&self) -> BlockPos {
        match self {
            Self::Break { position } => *position,
            Self::PlaceAgainst { against, .. } => *against,
            Self::Place { target, .. } => *target,
            Self::UseOn { click } | Self::OffHandOn { click } => click.position,
            Self::PlaceAt { target, .. } => *target,
        }
    }
}

/// Something a region tells an edge that must reach it even if the region's owner dies
/// right after: an entry of the region's outbox for that edge. It stays in the outbox, and
/// is sent again on every new link, until the edge has confirmed it. See
/// `docs/adr/0008-durable-regions-and-resuming.md`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
///
/// `Departed`, `Refused`, a `Remote` made of a player's own action and a `NotMine` for
/// an arrival go to the outbox of the edge the player belongs to or arrived through;
/// `RemoteDone`, and a `Remote` or a `NotMine` that answers a remote action, go to the
/// outbox of the edge the action came from. `Ended` goes to the outbox of the edge of
/// the stay it ends, or, for an arrival that is passed over, of the edge the arrival
/// came through.
pub enum Durable {
    /// The player has stepped into a chunk the region believes `to` to hold, and is no
    /// longer in the region. Whoever routes the player passes this on to `to` as
    /// [`PlayerChange::Arrive`], together with every input numbered above
    /// [`PlayerTransfer::last_input`]. Nothing says that the entity is gone: it lives on
    /// in the region it walked into. Passed on to an edge as [`PlayerEvent::Departed`].
    Departed {
        player: PlayerId,
        transfer: PlayerTransfer,
        /// The region the world store said holds the chunk the player stands in.
        to: RegionId,
    },
    /// The player could not enter the world, because the region has no entity id left
    /// for them. `attempt` is that of the join it answers. Passed on to an edge as
    /// [`PlayerEvent::Refused`].
    Refused { player: PlayerId, attempt: u64 },
    /// What is left of an action concerns a chunk this region does not hold: the one
    /// with the block [`RemoteStep::concerns`] names. The player's own action that is
    /// passed on is not among the acknowledged ones of the tick.
    Remote {
        action: RemoteAction,
        /// The region this one believes to hold that chunk. `None` if it has no answer
        /// of the world store about the chunk: the action is then for the region that
        /// serves the edge the chunk, which the edge knows at first hand. See
        /// `docs/adr/0012-the-tick-on-chunks.md`, section 2.3.
        to: Option<RegionId>,
    },
    /// A remote action has been dealt with, whether or not it changed anything. The
    /// player can now be told so, as with [`PlayerEvent::Acknowledged`]; what it changed
    /// has been reported among the tick's events. `entity` is that of the action it
    /// answers.
    RemoteDone {
        player: PlayerId,
        entity: EntityId,
        sequence: i32,
    },
    /// An arrival or a remote action reached this region for a chunk it believes
    /// `holder` to hold, and goes there. For an arrival the player is on their way as
    /// after a `Departed`. A region that does not know who holds the chunk never says
    /// this: it takes an arrival in, and passes an action on as a `Remote` without a
    /// region. Nor does a region say it of a chunk of its own pinned areas, where it
    /// drops the belief and does the same. See `docs/adr/0012-the-tick-on-chunks.md`,
    /// sections 2.2 and 2.4, and `docs/adr/0014-merging-and-splitting.md`, section 2.2.
    NotMine { what: Misdirected, holder: RegionId },
    /// This region has absorbed `region`, and what follows in the outbox, as far as
    /// `numbers` reaches, is what that region had in its outbox for the edge, under
    /// new numbers. It names no players: the presence answers that follow every
    /// welcome say whom the region has. See
    /// `docs/adr/0014-merging-and-splitting.md`, section 2.3.
    Absorbed {
        region: RegionId,
        /// The `since` a welcome of the absorbed region would have said to the edge:
        /// its word for the numbering the two shared. 0 if the absorbed region did not
        /// know the edge, or knew it with a lower start than this region does: it has
        /// then forgotten whatever the edge kept for it.
        since: u64,
        /// The number of the last message of the edge that the absorbed region
        /// applied; 0 likewise.
        applied: u64,
        /// The numbers that the entries behind this one had in the absorbed region's
        /// outbox, in ascending order; none likewise. The edge passes over those it
        /// had seen there already.
        numbers: Vec<u64>,
    },
    /// A part of this region has become the region `region`, and the stays named are
    /// in it from now on: those of the edge's players who went, each with their
    /// entity and with the attempt the stay carried at the split
    /// ([`PlayerTransfer::attempt`]), in ascending order. Which chunks went is said on
    /// the link, anew on every link. See `docs/adr/0014-merging-and-splitting.md`,
    /// section 2.4.
    SplitOff {
        region: RegionId,
        players: Vec<(PlayerId, EntityId, Option<u64>)>,
    },
    /// The stay `entity` of `player`, which was this edge's, has been ended by the
    /// server: a later stay of the player took its place. `attempt` is that of the
    /// stay's join, as long as the stay had it.
    ///
    /// Made by every removal of a stay that its own edge did not ask for: a join or
    /// an arrival over a stay that is there, an arrival that is passed over for a
    /// later stay, an entering stay that is placed or let go over a present one, and
    /// the store's word that a stay is dead. Not by a leave, nor for an edge that is
    /// gone or reset. See `docs/adr/0020-one-stay-per-player.md`, section 6.
    Ended {
        player: PlayerId,
        entity: EntityId,
        attempt: Option<u64>,
    },
}

/// What a [`Durable::NotMine`] sends on its way again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[expect(
    clippy::large_enum_variant,
    reason = "a few a tick at most, and an arrival carries a whole player"
)]
pub enum Misdirected {
    /// A player who was let go to this region, as in [`PlayerChange::Arrive`].
    Arrival {
        player: PlayerId,
        transfer: PlayerTransfer,
    },
    /// A remote action that was passed on to this region.
    Remote(RemoteAction),
}

/// Something the runner tells the region about an edge. See
/// `docs/adr/0008-durable-regions-and-resuming.md`, section 2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EdgeEvent {
    /// The edge is there with this start. If the region knows the edge with a lower
    /// start, the edge is reset: its players are removed and reported so, the entity of
    /// every [`Durable::Departed`] in its outbox, and of every [`Durable::NotMine`] for
    /// an arrival, is reported removed too, its outbox is dropped, and `applied` and
    /// `sent` start from 0 again. An edge the region does not know is noted with nothing
    /// applied or sent. The same start changes nothing, and so does a lower one, which
    /// the runner never passes on.
    Started { edge: EdgeId, start: u64 },
    /// The edge has the outbox entries up to `number`, which are dropped.
    Confirmed { edge: EdgeId, number: u64 },
    /// The edge has been away too long. Its players and the entities of the players on
    /// their way in its outbox are removed as for a reset, and the region forgets the
    /// edge.
    Gone { edge: EdgeId },
}

/// Everything that happened since the previous tick.
///
/// A tick applies all of `edges`, then `applied`, then `dead`, then all of
/// `player_changes`, then `entered`, then all of `remote_actions`, and then all of
/// `inputs`. What players did and what became of them arrives as one sequence, though,
/// and its order is lost when it is sorted into the two. [`TickInputs::change`] and
/// [`TickInputs::input`] sort it so that nothing a player did before they came to the
/// region is applied after it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TickInputs {
    /// What became of edges, in the order it happened. Applied before anything else.
    pub edges: Vec<EdgeEvent>,
    /// For edges whose messages are among these inputs: the number of the last of them.
    /// The region notes it as that edge's [`EdgeState::applied`]; an edge it does not
    /// know is passed over.
    ///
    /// [`EdgeState::applied`]: crate::EdgeState::applied
    pub applied: Vec<(EdgeId, u64)>,
    /// Players entering and leaving, in the order it happened. The order matters: a
    /// player can leave and come back, or join and leave, within one tick.
    pub player_changes: Vec<PlayerChange>,
    /// What players did, in the order it arrived. The inputs of a player are numbered in
    /// ascending order. An input whose number is not above that of the player's last
    /// applied input is ignored, so that inputs can be sent again to the region a player
    /// has moved to without any being applied twice.
    ///
    /// Each names the edge it came through. Only the edge a player belongs to acts for
    /// them: an input through another edge is ignored.
    ///
    /// Each names, between the player and the number, the entity of the stay it is of:
    /// an edge numbers a player's inputs from 1 with every connection, so the number
    /// alone does not say which of two stays an input belongs to. An input for a stay
    /// the region does not have is ignored. See
    /// `docs/adr/0014-merging-and-splitting.md`, section 2.1.
    ///
    /// Only what a player did after the last join or arrival of theirs in
    /// `player_changes` belongs here; see [`TickInputs::change`].
    pub inputs: Vec<(EdgeId, PlayerId, EntityId, u64, PlayerInput)>,
    /// What players of other regions did to blocks of this one, in the order it arrived,
    /// each with the edge that passed it on. It is applied after `player_changes` and
    /// before `inputs`, and answered one for one with a [`Durable::RemoteDone`], a
    /// [`Durable::Remote`] or a [`Durable::NotMine`] for that edge. An action through an
    /// edge the region does not know is ignored, as there is nobody to answer.
    pub remote_actions: Vec<(EdgeId, RemoteAction)>,
    /// Subscriptions of links to chunks that began, each with its kind. The region
    /// counts them on every chunk, whatever it knows of it. A chunk the region holds is
    /// loaded while it has tickets.
    pub tickets_added: Vec<(ChunkPos, Ticket)>,
    /// Subscriptions that ended; one entry releases one ticket of its kind. Additions
    /// are counted first, so that a ticket released and taken again within one tick
    /// keeps the chunk loaded.
    pub tickets_removed: Vec<(ChunkPos, Ticket)>,
    /// Chunks that storage delivered in answer to earlier [`TickOutput::chunk_requests`].
    pub chunks_loaded: Vec<(ChunkPos, Chunk)>,
    /// The light some of `chunks_loaded` had when the runner kept them warm at a merge or
    /// a split, their blocks unchanged since ([`crate::Region::hand_over_light`]): the
    /// region adopts it rather than lighting them whole (ADR-0029, section 7).
    pub chunks_kept: Vec<(ChunkPos, clustine_light::engine::Kept)>,
    /// Chunks the world store has granted the region in answer to earlier
    /// [`TickOutput::claims`]: the region holds them from this tick on. See
    /// `docs/adr/0012-the-tick-on-chunks.md`, section 1.3.
    pub granted: Vec<ChunkPos>,
    /// Chunks the region claimed that another region holds, with that region. The
    /// region believes it for as long as it wants the chunk. Ignored for a chunk the
    /// region holds: it does not unlearn that.
    pub foreign: Vec<(ChunkPos, RegionId)>,
    /// Chunks of which the region is to forget that it believes the region named to
    /// hold them, and to claim again if it wants them. A belief that has changed since
    /// is left alone: it is newer than the doubt.
    pub unbelieve: Vec<(ChunkPos, RegionId)>,
    /// Border copies of chunks of other regions' land, or of land nobody holds, as
    /// their bases: read like loaded chunks, never written, ticked or saved
    /// (`docs/adr/0025-effects-between-regions.md`, section 3). The runner hands a base
    /// in only once its chunk is used by nobody or by a region coupled to this one
    /// (section 8 of the same record). Applied with the chunks, before the region
    /// works out which chunks tick.
    pub copy_bases: Vec<(ChunkPos, Chunk)>,
    /// The changes of later commits in copied chunks, blocks and block entities' data,
    /// in the order the store sent them, applied after the bases.
    pub copy_updates: Vec<(ChunkPos, Vec<Change>)>,
    /// The entity changes of later commits in copied chunks, in the order the store
    /// sent them, applied after the bases (`docs/adr/0028-entities.md`, section 3). They
    /// touch other parts of a chunk than block changes, so their order among those does
    /// not matter.
    pub copy_entity_updates: Vec<(ChunkPos, Vec<EntityChange>)>,
    /// Copies the region no longer has: gone from its tickets' reach, withdrawn
    /// because their chunk came to be used by a region not coupled to this one, or
    /// loaded as the region's own.
    pub copies_withdrawn: Vec<ChunkPos>,
    /// Effects for chunks the region has loaded, ready by the rule of the same record's
    /// section 5, in the order of their numbers. Run where seam entries run, after the
    /// inputs and the reckoning of which chunks tick.
    pub effects: Vec<FiledEffect>,
    /// Entities sent into chunks the region holds, ready and of loaded chunks, in the
    /// order of their numbers (`docs/groundwork/n3-plan.md`): taken after the effects and
    /// before the entities tick.
    pub arrivals: Vec<Arrival>,
    /// Chunks the region holds or is about to hold, not loaded, for which an arrival
    /// waits: loaded for it with an arrival ticket (the plan, sections 5 and 6).
    pub arrivals_waiting: Vec<ChunkPos>,
    /// The world store's answers to the region's [`StayNote::Entering`] notes, in the
    /// order they came. An answer is taken only for the very stay the region still
    /// holds as entering for the player, and passed over otherwise. Whatever the case,
    /// a stay of the player that is in the region goes first, with a
    /// [`Durable::Ended`] to its edge. Then the stay enters by its place:
    ///
    /// - none, or one whose feet are below [`RegionConfig::lowest_y`]: the player is
    ///   placed at the spawn point, looking ahead and not flying, with the starting
    ///   inventory if there is no place and with what the place has them hold otherwise;
    /// - a place and no `holder`: the player is placed there as the place has them;
    /// - a place and a `holder`: the stay is let go to that region in this tick
    ///   without being placed, with a [`Durable::Departed`] whose transfer has one
    ///   hand-over and the attempt of the join.
    ///
    /// See `docs/adr/0020-one-stay-per-player.md`, section 4, step 7.
    ///
    /// [`RegionConfig::lowest_y`]: crate::RegionConfig::lowest_y
    pub entered: Vec<Entered>,
    /// Of each player named, a stay below `(entity, hops)` is dead: one with a lower
    /// entity id, or that stay with fewer hand-overs. Such a stay that the region has
    /// is removed, and so is one with a lower entity id that it holds as entering;
    /// its entity is reported removed if it had been shown, and its edge is told with
    /// [`Durable::Ended`]. Nothing of an entry is kept after the tick. See section 5
    /// of the same record.
    pub dead: Vec<(PlayerId, EntityId, u32)>,
    /// The world store asks the region for chunks of its pinned land, each lend under its
    /// number, in the order asked. Answered in [`TickOutput::lend_answers`] at the end of
    /// the tick (`docs/adr/0027-lending-at-a-line.md`, section 3, step 2).
    pub lends_asked: Vec<LendAsked>,
    /// Chunks the region asked to be lent that are not: asked again later where the
    /// flag says so, and not before what the region knows of them changes otherwise
    /// (the same record, section 2).
    pub not_lent: Vec<(Vec<ChunkPos>, bool)>,
    /// Lends called off, by their numbers: the region that asked keeps its ask, and the
    /// lender takes the chunks up again as its own.
    pub lends_called_off: Vec<(u64, Vec<ChunkPos>)>,
    /// Chunks of the region's pinned land that the region named holds by a lend, or by
    /// what a merge or a split made of one: believed until they are pinned back.
    pub lent: Vec<(Vec<ChunkPos>, RegionId)>,
    /// Chunks of the region's pinned land that whoever held them by a grant has given
    /// back.
    pub pinned_back: Vec<ChunkPos>,
    /// The world's clock as the store last published it, if it did since the last tick:
    /// the region takes the later of it and its own ([`WorldClock::later`]).
    pub clock: Option<WorldClock>,
    /// The faces and their removals the store streamed since the last tick, in order
    /// (`docs/groundwork/n7-plan.md`, section 7.3).
    pub faces: Vec<FaceChange>,
    /// The world's players as the store last published them, if it did since the last
    /// tick.
    pub world_players: Option<Vec<WorldPlayer>>,
    /// The other regions' counts the store published with the players, where it has
    /// every region's (section 9.2).
    pub counts: Option<PublishedCounts>,
}

/// The world's clock (`docs/groundwork/n7-plan.md`, sections 7.4, 8.4 and 9.3): the game
/// time, which every tick raises by one, and the ticks of the overworld's clock (the
/// game's `world_clock` `minecraft:overworld`, whose rate is 1), which the day and night
/// follow and a night slept through moves on. Every region keeps it, each the later of
/// its own and what the world store last published of every region's, so that it only
/// ever moves forward and the regions' stay within a second of one another.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WorldClock {
    pub game_time: u64,
    pub day: u64,
}

impl WorldClock {
    /// The clock a tick later: a tick moves both on by one.
    pub fn next(self) -> Self {
        Self {
            game_time: self.game_time + 1,
            day: self.day + 1,
        }
    }

    /// The later of each of the two, apart.
    pub fn later(self, other: Self) -> Self {
        Self {
            game_time: self.game_time.max(other.game_time),
            day: self.day.max(other.day),
        }
    }
}

/// The world store's ask of a lend of chunks of the region's pinned land to the region
/// `to` (`docs/adr/0027-lending-at-a-line.md`, section 3, step 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LendAsked {
    pub lend: u64,
    pub chunks: Vec<ChunkPos>,
    pub to: RegionId,
}

/// A lender's answer to a [`LendAsked`] for some of its chunks (the same record,
/// section 3, step 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LendAnswer {
    /// Not lent: asked again later if `again`, and not otherwise while what stands
    /// there stands.
    Refused {
        lend: u64,
        chunks: Vec<ChunkPos>,
        again: bool,
    },
    /// Let go of for the lend: the region has stopped ticking them, and drops them at
    /// the start of its next tick, so that they are to be saved as they are after this
    /// one, with their ticks, before the answer goes to the store.
    Lending { lend: u64, chunks: Vec<ChunkPos> },
}

/// What a link's subscription to a chunk is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ticket {
    /// For a viewer whose player is this region's. It makes the region claim the chunk
    /// if it knows nothing of it, and go on knowing who holds it otherwise.
    Viewer,
    /// For a viewer whose player is another region's. It makes the region claim the
    /// chunk only in an area the region is pinned to, where a claim takes nothing from
    /// anyone. A chunk the region holds it keeps from being given back, as any ticket
    /// does.
    Guest,
    /// For a chunk beside a viewer's chunk that the region holds, which the chunk needs
    /// loaded to tick (`docs/adr/0024-block-behaviour-in-a-region.md`, section 2). It
    /// wants and claims the chunk as a viewer's ticket does.
    ViewerRing,
    /// The same beside a guest's chunk only: it wants and claims the chunk as a guest's
    /// ticket does.
    GuestRing,
}

impl TickInputs {
    /// Adds what became of a player, which came after everything added so far.
    ///
    /// For a join, what that player did before, through any edge and as far as it
    /// waits here for the coming tick, is dropped; for an arrival, what waits that
    /// names the entity that arrives. The stay was not there then, so the region would
    /// have ignored it. Applied after the arrival instead, it would be taken for what
    /// the player did since, and an input sent to the region while the player was away
    /// would be applied ahead of earlier ones that are sent again with the arrival,
    /// which then count as applied already and are lost.
    ///
    /// An arrival leaves what waits of another stay of the player: with several edges
    /// an earlier stay can arrive where a later one is, and must not take away what
    /// the later one did in that tick. See `docs/adr/0020-one-stay-per-player.md`,
    /// section 12.
    ///
    /// A leave drops nothing. Whether it applies only the tick can tell: one for a stay
    /// the region does not have changes nothing, and must not take away what the stay
    /// that is there did. Nor is anything lost by keeping it all. A tick applies
    /// changes before inputs, so what a player did before a leave that takes effect
    /// finds nobody; and if they are back within the tick, the join or the arrival
    /// that brings them drops it.
    pub fn change(&mut self, change: PlayerChange) {
        match &change {
            PlayerChange::Join(_, PlayerJoin { player, .. }) => {
                self.inputs.retain(|(_, actor, ..)| actor != player);
            }
            PlayerChange::Arrive(_, player, transfer) => {
                let arriving = transfer.entity_id;
                self.inputs
                    .retain(|(_, actor, entity, ..)| actor != player || *entity != arriving);
            }
            PlayerChange::Leave(..) | PlayerChange::Discard { .. } => {}
        }
        self.player_changes.push(change);
    }

    /// Adds something a player did while they had `entity`, as passed on by `edge`,
    /// which came after everything added so far.
    pub fn input(
        &mut self,
        edge: EdgeId,
        player: PlayerId,
        entity: EntityId,
        number: u64,
        input: PlayerInput,
    ) {
        self.inputs.push((edge, player, entity, number, input));
    }
}

/// Something that concerns a single player.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[expect(
    clippy::large_enum_variant,
    reason = "a few a tick at most, and a departure carries a whole player"
)]
pub enum PlayerEvent {
    /// The player has entered the world. `attempt` is that of the join it answers
    /// ([`PlayerJoin::attempt`]).
    Spawned {
        attempt: u64,
        entity_id: EntityId,
        pose: Pose,
        flying: bool,
        inventory: Inventory,
        selected_slot: u8,
        /// What they ride, seated or waiting for it: a player put back on the unit they
        /// left the world on (`docs/groundwork/n7h-logout-on-a-mount.md`, section 6).
        riding: Option<EntityId>,
    },
    /// Everything the player did with a sequence number up to `sequence` has been
    /// handled, whether it took effect or not. The client then stops showing its own
    /// guess of the outcome and shows what the region reported.
    Acknowledged { sequence: i32 },
    /// The player has been let go to another region. A region does not emit this
    /// itself: it makes a [`Durable::Departed`], which a worker passes on as this.
    Departed(PlayerTransfer),
    /// The player could not enter the world. A region does not emit this itself: it
    /// makes a [`Durable::Refused`], which a worker passes on as this.
    Refused,
    /// The region changed a slot of the player's inventory of its own accord, as the
    /// game's server does where the client does not foresee it (the swap of the hands),
    /// and the client is to be told what it holds now. `slot` is numbered as
    /// [`Inventory`] numbers them.
    InventorySlot { slot: u8, stack: Option<ItemStack> },
    /// The vehicle the player drives was put back where the region has it: the move
    /// their client sent was refused (the clientbound `move_vehicle`).
    VehicleCorrected {
        position: Vec3,
        yaw: f32,
        pitch: f32,
    },
    /// The player got on (`Some`) or off (`None`) a vehicle, which the game does as a
    /// teleport: their client is sent where they are now and who rides the vehicle.
    Rides {
        vehicle: Option<EntityId>,
        pose: Pose,
    },
    /// The player was put out for holding a vehicle in the air too long (the game's
    /// kick after 160 ticks).
    Kicked { reason: String },
    /// A sentence of the game by its key, with numbers for its arguments, in their chat
    /// or, with `overlay`, on their action bar (`sendSystemMessage`, `sendOverlayMessage`).
    Message {
        key: String,
        args: Vec<i32>,
        overlay: bool,
    },
    /// The player was put somewhere by the server (`connection.teleport`): lying down in a
    /// bed, getting up from one, or sent back while asleep.
    Teleported { pose: Pose },
    /// The region selected the hotbar slot `slot`, from 0 to 8, of its own accord (a
    /// pick of an entity's item), which the client is told (`set_held_slot`).
    HeldSlot { slot: u8 },
    /// Their experience changed, or they entered the world with some (`set_experience`).
    Experience(Experience),
    /// A menu, of the serial `serial`, whole: opened (the edge sends `open_screen`,
    /// closing a menu of another serial first), or reported again whole (after a
    /// restore, when its container's chunks load, or answering a click of another
    /// serial or a [`PlayerInput::MenuResync`]), which the edge sends as
    /// `container_set_content`. `applied` is the number of the last input of the player
    /// applied when it was reported; `title` the translation key of its name; `slots`
    /// every slot of the menu, [`MenuKind::slots`] of them; `carried` the stack on the
    /// cursor (`docs/groundwork/n6-plan.md`, sections 7 to 9).
    MenuOpened {
        serial: u64,
        applied: u64,
        kind: MenuKind,
        title: String,
        slots: Vec<Option<ItemStack>>,
        carried: Option<ItemStack>,
    },
    /// The menu of the serial `serial` changed, or a click was answered: the slots that
    /// changed against what the region last reported, by the menu's numbers, and the
    /// carried stack if it changed. A click that changed nothing, was refused or was
    /// passed over is answered with none, so that the edge compares what its client
    /// guessed (section 7 item 1).
    MenuChanged {
        serial: u64,
        applied: u64,
        slots: Vec<(u16, Option<ItemStack>)>,
        carried: Option<Option<ItemStack>>,
    },
    /// The menu of the serial `serial` is closed: by the client, by another opening, by
    /// `stillValid`, or at a hand-over. The slots of the inventory the carried stack went
    /// into come as [`PlayerEvent::InventorySlot`]s after it.
    MenuClosed { serial: u64, applied: u64 },
    /// The answer to a [`PlayerInput::MenuResync`] of a player with no menu open: their
    /// inventory whole, as a resume may have lost what a close sent.
    NoMenu { applied: u64, inventory: Inventory },
    /// An item's cooldown began, or ended at 0 (`ClientboundCooldownPacket`): the group
    /// by name and its ticks.
    Cooldown { group: String, ticks: i32 },
    /// One of their effects was added or changed (`ClientboundUpdateMobEffectPacket`):
    /// the level in force, with the ticks it has left, and whether the client blends it
    /// in (an effect new to them, `onEffectAdded`).
    EffectAdded {
        instance: clustine_world::components::EffectInstance,
        blend: bool,
    },
    /// One of their effects ended or was taken off (`ClientboundRemoveMobEffectPacket`),
    /// by name.
    EffectRemoved { effect: String },
    /// Attributes of their own entity that their effects changed
    /// (`ClientboundUpdateAttributesPacket`): their client moves by them.
    Attributes(Vec<AttributeShown>),
}

/// Something that happened in the region and concerns everyone who can see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RegionEvent {
    /// An entity has come into existence.
    EntitySpawned(EntityState),
    /// An entity has gone from one of the region's chunks into another, as it is now:
    /// for an edge that watches the chunk it came into and not the one it left, or that
    /// was shown the two chunks at different ticks, between which it moved, and so is
    /// shown it in neither (the game's `ChunkMap` tracks an entity coming into a
    /// player's view as one appearing). An edge that knows the entity passes it over,
    /// its move being in the entity's own report.
    EntityEntered(EntityState),
    /// An entity has ceased to exist. `chunk` is where it was last.
    EntityRemoved { entity: EntityId, chunk: ChunkPos },
    /// A block has changed. `seen` says whether it was set with the game's
    /// `UPDATE_CLIENTS`: one set without it (a block a piston moves, which clients
    /// move themselves from the piston's block event) is not sent, but a block waiting
    /// to be sent is sent as it is then, as `ChunkHolder.broadcastChanges` reads it.
    BlockChanged {
        position: BlockPos,
        state: BlockState,
        seen: bool,
    },
    /// The block entity at the position has new data, the bytes of one NBT compound as
    /// [`clustine_world::BlockEntity`] keeps them; empty where the game makes a fresh
    /// one for a block of the same kind (`docs/adr/0026-redstone.md`, section 6). It
    /// comes in its place among the tick's [`Self::BlockChanged`]. `seen` says whether
    /// the edges are to be told: a container's slots and openers are durable as any
    /// data is, but the game never sends them to clients, so neither are the edges
    /// (`docs/groundwork/n6-plan.md`, section 6 item 12).
    BlockEntityChanged {
        position: BlockPos,
        data: Vec<u8>,
        seen: bool,
    },
    /// A block broke, which clients show with its sound and bits (the game's level
    /// event 2001). The block itself changes by a [`Self::BlockChanged`] beside it.
    /// `breaker` is the player who broke it, whose client shows it already.
    BlockDestroyed {
        position: BlockPos,
        state: BlockState,
        breaker: Option<PlayerId>,
    },
    /// A block event ran (`docs/adr/0026-redstone.md`, section 4): the game's
    /// `ClientboundBlockEventPacket`, of the block whose id is `block`, with its two
    /// numbers, which clients act on (a piston's move, a note block's note).
    BlockEvent {
        position: BlockPos,
        block: u16,
        kind: i32,
        data: i32,
    },
    /// A sound the server plays at a place (`Level.playSound`), the game's sound event
    /// of id `sound` from its `source` (4 for blocks): heard by the players within 16
    /// blocks, or 16 times `volume` if that is more, but `except`, whose own client
    /// plays it (`docs/adr/0026-redstone.md`, section 8).
    Sound {
        sound: i32,
        source: i32,
        position: Vec3,
        volume: f32,
        pitch: f32,
        except: Option<PlayerId>,
    },
    /// The game's level event `kind` at a block (`Level.levelEvent`), with its `data`:
    /// what clients show of something there, a torch burning out (1502) say. Seen by
    /// the players within 64 blocks; the breaking of a block and lava's hiss have
    /// variants of their own.
    LevelEvent {
        kind: i32,
        position: BlockPos,
        data: i32,
    },
    /// The phase of block events begins here (`docs/adr/0026-redstone.md`, section
    /// 8): the game sends the tick's block changes before its block events, and those
    /// made from here on, in the block events, the entities and the block entities,
    /// with the next tick's. Said in a tick in which something comes after it. It has
    /// no place, and whoever filters events by where they are passes it to all.
    BlockEventsBegin,
    /// Lava met water, or took the place of a block, which clients show with a hiss and
    /// smoke (the game's level event 1501).
    Fizzed { position: BlockPos },
    /// An entity has a new pose. At most one per entity and tick.
    EntityMoved {
        entity: EntityId,
        pose: Pose,
        /// The chunk the entity was in before, which differs from the chunk of the new
        /// position when it crossed a chunk border.
        previous_chunk: ChunkPos,
    },
    /// The entity, a player in `chunk`, swung an arm, as the game's `LivingEntity.swing`
    /// shows it to everyone but the player, whose client swings it already: when they
    /// punch, throw an item, place a block or use one. `off_hand` is the arm, `swing`
    /// how it moves (a spear stabs), and `own` whether the player's own client is shown
    /// it too, as for a use whose result is the game's `SUCCESS_SERVER`, which the client
    /// did not foresee.
    ArmSwung {
        entity: EntityId,
        chunk: ChunkPos,
        off_hand: bool,
        swing: Swing,
        own: bool,
    },
    /// An entity other than a player as its tick left it, reported in a tick that
    /// changed what an edge shows of it or set one of its flags for the edge
    /// (`docs/groundwork/n4-plan.md`, section 7; the brief's section 9).
    EntityTracked(Tracked),
    /// An entity other than a player is gone: it died, was picked up, despawned or
    /// merged into another. Passed over by an edge that shows it with a higher `hops`
    /// (`docs/adr/0028-entities.md`, section 6).
    EntityGone {
        entity: EntityId,
        chunk: ChunkPos,
        hops: u32,
    },
    /// The player entity `collector` picked up `count` of the item `item`, which is in
    /// `chunk` (`TakeItemEntity`); the item's removal, if it went whole, follows.
    ItemTaken {
        item: EntityId,
        collector: EntityId,
        count: i32,
        chunk: ChunkPos,
    },
    /// A falling block landed at `position` as `state` (`AddTransientBlock`, sent after
    /// the block's change).
    TransientBlock { position: BlockPos, state: u16 },
    /// An entity other than a player was sent across a line, from `chunk`, with the
    /// `hops` it had and where it was: an edge goes on showing it there until the region
    /// it went to reports it with more `hops` (`docs/adr/0028-entities.md`, section 6).
    HandedOn {
        entity: EntityId,
        chunk: ChunkPos,
        hops: u32,
        position: [f64; 3],
        motion: [f64; 3],
    },
    /// Who rides the vehicle `vehicle` now, in order, by key: the vehicle's word, which
    /// the edge takes for its other passengers and checks for players against what each
    /// player's own region says (`docs/groundwork/n5-plan.md`, section 8, item 4).
    Passengers {
        vehicle: EntityId,
        chunk: ChunkPos,
        hops: u32,
        passengers: Vec<EntityId>,
    },
    /// Who holds the leash of `entity` now (`set_entity_link`), if anyone.
    Leashed {
        entity: EntityId,
        chunk: ChunkPos,
        holder: Option<EntityId>,
    },
    /// The vehicle `vehicle` was hit and wobbles, or a boat's paddles moved: its data
    /// changed in a way a client shows (`VehicleEntity`'s hurt data, `AbstractBoat`'s
    /// paddles and bubble time). The edge sends what changed.
    DataChanged {
        entity: EntityId,
        chunk: ChunkPos,
        hops: u32,
        data: EntityData,
    },
    /// Something happened to `entity` that clients show their own way (the game's
    /// `broadcastEntityEvent`): a bubble column's push to a driven boat, whose driver's
    /// client moves it, an armour stand's wobble.
    EntityEvent {
        entity: EntityId,
        chunk: ChunkPos,
        event: u8,
    },
    /// A sound at an entity (`sound_entity`): the sound's id, its source category, its
    /// volume, its pitch and the seed the level's sound seeds drew.
    EntitySound {
        entity: EntityId,
        chunk: ChunkPos,
        sound: i32,
        category: u8,
        volume: f32,
        pitch: f32,
        seed: i64,
    },
    /// An explosion (`ServerLevel.explode`), which every edge sends to each of its
    /// players within 64 blocks of `centre`, whatever region they are in, with the push
    /// `knockback` names for them, if any (`docs/groundwork/n6-brief.md`, finding 2.9 and
    /// section 9.1). `blocks` is how many positions the rays reached, air among them;
    /// `large` whether it shows the large particle (a radius of 2 or more that breaks
    /// blocks); `sound` whether it plays its sound (its source is not silent). The
    /// blocks it broke come as changes of their own.
    Explosion {
        centre: [f64; 3],
        radius: f32,
        blocks: i32,
        knockback: Vec<(EntityId, [f64; 3])>,
        large: bool,
        sound: bool,
    },
    /// The particle of a vibration a sculk sensor took up (`VibrationParticleOption`,
    /// sent by `ServerLevel.sendParticles` as `level_particles` with count 1 and no
    /// spread or speed), which every edge shows to each of its players whose block's
    /// centre is within 32 blocks of `from`, whatever region they are in
    /// (`docs/groundwork/n6-brief.md`, sections 8.6 and 9.6): it starts at `from`, the
    /// event's position or, for one reloaded with its sensor's chunk, the point it had
    /// reached; it goes to the sensor's block `to` (the `minecraft:block` position
    /// source) and arrives in `ticks` ticks.
    Vibration {
        from: [f64; 3],
        to: BlockPos,
        ticks: i32,
    },
    /// A living entity in `chunk` took a hurt that counts (`broadcastDamageEvent`,
    /// `docs/groundwork/n7-brief.md`, section 9.1): of the damage type of id
    /// `damage_type` in the registry the server sends (Clustine's sorted order),
    /// caused by `cause` and dealt by `direct` (keys, which the edge translates),
    /// with the position the source names, if any. Clients show its hurt from it.
    DamageEvent {
        entity: EntityId,
        chunk: ChunkPos,
        damage_type: i32,
        cause: Option<EntityId>,
        direct: Option<EntityId>,
        position: Option<[f64; 3]>,
    },
    /// The attributes a client is sent of a mob in `chunk` (`UpdateAttributes`):
    /// every synced instance it has made, ascending by the attribute's id, where they
    /// changed or it made its first.
    AttributesChanged {
        entity: EntityId,
        chunk: ChunkPos,
        attributes: Vec<AttributeShown>,
    },
    /// What a mob in `chunk` wears and holds changed (`SetEquipment`): every slot,
    /// by `EquipmentSlot` ordinal, empty ones as `None`.
    EquipmentChanged {
        entity: EntityId,
        chunk: ChunkPos,
        slots: Vec<(u8, Option<ItemStack>)>,
    },
    /// The player entity `entity` in `chunk` lay down in the bed whose head is `head`, or
    /// got up (`None`): their pose and sleeping position, which those who see them and
    /// they themselves are sent (`docs/groundwork/n7d-brief.md`, section 2.3).
    Sleeping {
        entity: EntityId,
        chunk: ChunkPos,
        head: Option<BlockPos>,
    },
    /// The player entity `attacker` in `chunk` struck `target` in a way its client
    /// shows (`ClientboundAnimatePacket` of the target: 1 a critical hit, 2 a magic
    /// one, 0 a waking), sent to the attacker and the players who see the attacker.
    Animated {
        target: EntityId,
        attacker: EntityId,
        chunk: ChunkPos,
        action: u8,
    },
    /// Values a player's entity shows the others changed (`docs/groundwork/n9-plan.md`,
    /// section 10): the living flags of a use (index 8), and from N9d the effect
    /// particles; the edge writes them as entity data to everyone but the player, whose
    /// own client foresees them.
    PlayerData {
        entity: EntityId,
        chunk: ChunkPos,
        values: Vec<(u8, crate::shown::DataShown)>,
    },
}

/// One attribute of an entity as clients are sent it: its id in the game's registry,
/// its base value, and its modifiers (id, amount, operation's ordinal).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttributeShown {
    pub attribute: u16,
    pub base: f64,
    pub modifiers: Vec<(String, f64, u8)>,
}

/// What an edge is told of an entity other than a player in a tick that changed it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tracked {
    pub entity: EntityId,
    pub chunk: ChunkPos,
    pub hops: u32,
    pub position: [f64; 3],
    pub motion: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    /// [`Tracked::HORIZONTAL_COLLISION`] and the rest.
    pub flags: u8,
    /// Its data, where the tick changed it (an item's stack after a merge).
    pub data: Option<EntityData>,
}

impl Tracked {
    /// The tick's move collided along x or z (`horizontalCollision`).
    pub const HORIZONTAL_COLLISION: u8 = 1;
    /// The tick's move collided along y (`verticalCollision`).
    pub const VERTICAL_COLLISION: u8 = 2;
    /// Its position is to be sent whole (`getRequiresPrecisePosition`).
    pub const PRECISE: u8 = 4;
    /// It is to be sent this tick whatever the interval says (`needsSync`).
    pub const NEEDS_SYNC: u8 = 8;
    /// This tick is to be one of its update ticks (`syncPosition`).
    pub const SYNC_POSITION: u8 = 16;
    /// Its motion is to be sent to everyone, itself included (`syncVelocity`).
    pub const SYNC_VELOCITY: u8 = 32;
    /// It is on fire (the shared flags' bit 0, its data index 0).
    pub const ON_FIRE: u8 = 64;
}

impl RegionEvent {
    /// The chunks from which the event can be observed.
    pub fn chunks(&self) -> [ChunkPos; 2] {
        match self {
            Self::EntitySpawned(state) | Self::EntityEntered(state) => [state.chunk(); 2],
            Self::EntityRemoved { chunk, .. }
            | Self::ArmSwung { chunk, .. }
            | Self::EntityGone { chunk, .. }
            | Self::ItemTaken { chunk, .. } => [*chunk; 2],
            Self::EntityTracked(tracked) => [tracked.chunk; 2],
            Self::TransientBlock { position, .. } => [position.chunk(); 2],
            Self::HandedOn { chunk, .. }
            | Self::Passengers { chunk, .. }
            | Self::Leashed { chunk, .. }
            | Self::DataChanged { chunk, .. }
            | Self::EntityEvent { chunk, .. }
            | Self::EntitySound { chunk, .. }
            | Self::DamageEvent { chunk, .. }
            | Self::AttributesChanged { chunk, .. }
            | Self::EquipmentChanged { chunk, .. }
            | Self::Animated { chunk, .. }
            | Self::PlayerData { chunk, .. }
            | Self::Sleeping { chunk, .. } => [*chunk; 2],
            Self::BlockChanged { position, .. }
            | Self::BlockEntityChanged { position, .. }
            | Self::BlockEvent { position, .. }
            | Self::LevelEvent { position, .. }
            | Self::BlockDestroyed { position, .. }
            | Self::Fizzed { position } => [position.chunk(); 2],
            Self::Sound { position, .. } => [ChunkPos::containing(position.x, position.z); 2],
            // The chunk of its centre: a player within 64 blocks of it has the chunk in
            // view unless their view reaches less than four chunks.
            Self::Explosion { centre, .. } => [ChunkPos::containing(centre[0], centre[2]); 2],
            // The chunk where it starts: a player within 32 blocks of it has the chunk in
            // view unless their view reaches less than two chunks.
            Self::Vibration { from, .. } => [ChunkPos::containing(from[0], from[2]); 2],
            // No place: see the variant.
            Self::BlockEventsBegin => [ChunkPos::new(0, 0); 2],
            Self::EntityMoved {
                pose,
                previous_chunk,
                ..
            } => [
                ChunkPos::containing(pose.position.x, pose.position.z),
                *previous_chunk,
            ],
        }
    }
}

/// The game's `SoundSource.BLOCKS`, which a block's sounds are of.
pub const SOUND_SOURCE_BLOCKS: i32 = 4;

/// The game's `SoundSource.PLAYERS`, which what a player plays as themself is of.
pub const SOUND_SOURCE_PLAYERS: i32 = 7;

/// The game's `SoundSource.RECORDS`, which a note block's notes are of.
pub const SOUND_SOURCE_RECORDS: i32 = 2;

/// What a tick resulted in.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TickOutput {
    /// The number of this tick; the first tick of a region is 1.
    pub tick: u64,
    /// The world's clock after the tick, which the commit carries beside the region's
    /// state, so that the store keeps the world's (`docs/groundwork/n7-plan.md`, section
    /// 8.4).
    pub clock: WorldClock,
    /// Whether the clock moved in the tick other than by its one tick (a publication
    /// taken, a night slept through), which clients are then told at once.
    pub clock_jumped: bool,
    /// [`PlayerEvent::Spawned`], in the order the players joined, or, where the world
    /// store says where a player enters, in the order of its answers
    /// ([`TickInputs::entered`]); then [`PlayerEvent::InventorySlot`], in the order of
    /// the inputs that made them; and then [`PlayerEvent::Acknowledged`], in the order
    /// of the players. What else concerns a single player is among `durable`.
    pub player_events: Vec<(PlayerId, PlayerEvent)>,
    pub events: Vec<RegionEvent>,
    /// Chunks that have to be fetched from storage and passed in through
    /// [`TickInputs::chunks_loaded`].
    pub chunk_requests: Vec<ChunkPos>,
    /// The outbox entries made in this tick, each with the edge whose outbox it is in
    /// and its number there. An edge's entries are numbered on from its
    /// [`EdgeState::sent`].
    ///
    /// They are in this order: the stays that [`TickInputs::dead`] ended; refusals,
    /// the stays that a join or an arrival ended and the arrivals that are another
    /// region's, in the order of [`TickInputs::player_changes`]; the stays that
    /// [`TickInputs::entered`] ended, in its order; the answers to
    /// [`TickInputs::remote_actions`], one for one and in their order; what players did
    /// to blocks of chunks the region does not hold, in the order of
    /// [`TickInputs::inputs`]; and those who were let go: the stays let go without
    /// being placed, in the order of [`TickInputs::entered`], and then the players, in
    /// their order.
    ///
    /// So every [`Durable::Ended`] of a tick has a lower number than every
    /// [`Durable::Departed`] of that tick in the same outbox, and a stay that is let
    /// go without being placed is numbered with the tick's other departures and not
    /// where it is decided. An edge remembers only the highest number it has seen, and
    /// the runner publishes a tick's departures last. See
    /// `docs/adr/0020-one-stay-per-player.md`, section 6.
    ///
    /// [`EdgeState::sent`]: crate::EdgeState::sent
    pub durable: Vec<(EdgeId, u64, Durable)>,
    /// Chunks the region asks the world store to grant it, in ascending order: those it
    /// wants and knows nothing of. Each is answered once, through
    /// [`TickInputs::granted`] or [`TickInputs::foreign`], and is not claimed again
    /// before.
    pub claims: Vec<ChunkPos>,
    /// Chunks the region gives back to the world store, in ascending order: nothing has
    /// used them for [`RegionConfig::return_after`] ticks. None of them is loaded, and
    /// every change the region made to them is in a save it asked for before.
    ///
    /// [`RegionConfig::return_after`]: crate::RegionConfig::return_after
    ///
    /// A lent chunk is given back once no circuit's part is near it, loaded: the region
    /// drops it at the start of its next tick, so that it is to be saved as it is after
    /// this one before the return goes to the store
    /// (`docs/adr/0027-lending-at-a-line.md`, section 4).
    pub returns: Vec<ChunkPos>,
    /// Chunks across a pinned line the region asks to be lent, ascending: for a circuit's
    /// part placed in its own pinned land within reach of them, or asked again
    /// (`docs/adr/0027-lending-at-a-line.md`, section 2).
    pub lends: Vec<ChunkPos>,
    /// The region's answers to [`TickInputs::lends_asked`], in the order asked.
    pub lend_answers: Vec<LendAnswer>,
    /// Chunks whose scheduled ticks changed in this tick, in ascending order: a tick
    /// scheduled or run there, or the ticks of a chunk that stopped ticking written
    /// into it. They are to be saved as chunks whose blocks changed are
    /// (`docs/adr/0024-block-behaviour-in-a-region.md`, section 4).
    pub ticks_changed: Vec<ChunkPos>,
    /// Chunks the region ticks with scheduled ticks pending at the end of this tick, in
    /// ascending order. Their saved forms change with every tick, since what is kept is
    /// the delay left, without anything being committed; so where a stop is near they
    /// are to be saved as unsaved chunks are
    /// (`docs/adr/0024-block-behaviour-in-a-region.md`, section 4).
    pub ticks_pending: Vec<ChunkPos>,
    /// Chunks post-processed in this tick, whose marks were taken, in ascending order
    /// (`docs/adr/0024-block-behaviour-in-a-region.md`, section 6). They are to be
    /// committed and saved as chunks whose blocks changed are.
    pub marks_taken: Vec<ChunkPos>,
    /// Chunks whose seam entries changed in this tick, in ascending order
    /// (`docs/adr/0024-block-behaviour-in-a-region.md`, section 5). They are to be
    /// committed and saved as chunks whose blocks changed are.
    pub seams_changed: Vec<ChunkPos>,
    /// Chunks whose kept block events changed in this tick, in ascending order
    /// (`docs/adr/0026-redstone.md`, section 4). They are to be committed and saved as
    /// chunks whose blocks changed are.
    pub events_changed: Vec<ChunkPos>,
    /// What the tick did in chunks the region does not have loaded and does not hold
    /// or ask for, in the order it did them
    /// (`docs/adr/0025-effects-between-regions.md`, section 4). They are to be
    /// committed with the tick's commit.
    pub effects: Vec<Effect>,
    /// The effects the tick took, ran or kept as seam entries, ascending (the same
    /// record, section 5). A tick that takes any is to be committed whatever else it
    /// changed.
    pub taken: Vec<EffectId>,
    /// Everything that changed in the region's state in this tick.
    pub delta: StateDelta,
    /// What the region says to the world store of stays in this tick, in the order of
    /// the players, a [`StayNote::Entering`] before a [`StayNote::Has`] of the same
    /// player:
    ///
    /// - `Entering` for the stay a join of this tick began, if the region still holds
    ///   it as entering at the end of the tick;
    /// - `Has` for every player who is in the region after the tick and of whom the
    ///   tick changed anything, which is the players of [`StateDelta::players`]: one
    ///   who was placed, one who arrived, one who did something;
    /// - `Has` for every player who was let go, and for every stay let go without
    ///   being placed, with the hand-overs and the place of the transfer;
    /// - in the first tick of a region, which is the first after
    ///   [`Region::restore`], after a merge or after a split: a note for every stay
    ///   the region has, changed or not.
    ///
    /// None for a player who was removed. A tick with notes is to be committed
    /// whatever else it changed. See `docs/adr/0020-one-stay-per-player.md`,
    /// section 3.
    ///
    /// [`Region::restore`]: crate::Region::restore
    pub stays: Vec<StayNote>,
    /// What the tick did to entities other than players, by chunk, in the order it did
    /// it: to be committed with the tick (`docs/adr/0028-entities.md`, section 3). Only
    /// for chunks the region has loaded.
    pub entity_changes: Vec<(ChunkPos, EntityChange)>,
    /// The counts of the region's mobs by spawning category, of the chunks whose counts
    /// changed in the tick, or of every loaded chunk (`docs/groundwork/n9-plan.md`,
    /// section 9.2): carried with the commit, not written to the log.
    pub counts: Option<CountsReport>,
    /// The chunks whose entities changed in the tick without a commit: counters, and a
    /// resting item's motion (`docs/groundwork/n4-plan.md`, sections 2 and 7). The
    /// worker saves their entities at its checkpoints and its stop, so that what the
    /// store has of them is never older than the last checkpoint.
    pub entities_ageing: Vec<ChunkPos>,
    /// Mobs whose record this build cannot read (a part of a later version, or bytes that
    /// are not a part's): each is left as it is and does not tick, and the worker logs it
    /// (`docs/groundwork/n7-plan.md`, section 9.6, item 2).
    pub unreadable_mobs: Vec<EntityId>,
    /// The entities the tick sent across a line, each to the chunk it is in, whole with
    /// the `hops` it had: their chunks' changes have them `Gone`.
    pub arrivals: Vec<(ChunkPos, Entity)>,
    /// The arrivals the tick took, ascending.
    pub arrivals_taken: Vec<EffectId>,
}
