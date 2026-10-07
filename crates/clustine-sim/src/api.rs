//! What goes into a region's tick and what comes out of it.
//!
//! These types cross the boundary between the worker and the edge, so they have to stay
//! serialisable and free of anything specific to the Minecraft protocol.

use clustine_data::BlockState;
use clustine_world::{BlockPos, Chunk, ChunkPos, EntityId, PlayerId, Vec3};
use serde::{Deserialize, Serialize};

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

/// The number of slots in a player's hotbar.
pub const HOTBAR_SLOTS: usize = 9;

/// A number of items of one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemStack {
    /// Id in the item registry.
    pub item: i32,
    pub count: i32,
}

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

/// A player entering the region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerJoin {
    pub player: PlayerId,
    pub name: String,
}

/// A player as one region hands them to another: everything a region knows about them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerTransfer {
    /// The player keeps their entity, so that nobody watching sees them vanish.
    pub entity_id: EntityId,
    pub name: String,
    pub pose: Pose,
    pub hotbar: [Option<ItemStack>; HOTBAR_SLOTS],
    pub selected_slot: u8,
    /// The number of the last input the region applied; see [`TickInputs::inputs`].
    pub last_input: u64,
}

/// A player entering or leaving the region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlayerChange {
    /// A player enters the world.
    Join(PlayerJoin),
    /// A player's connection has ended.
    Leave(PlayerId),
    /// A player comes in from another region, as that region let them go with
    /// [`PlayerEvent::Departed`].
    Arrive(PlayerId, PlayerTransfer),
    /// An entity that another region let go will not arrive anywhere, because its player
    /// left in the meantime. It is reported as removed to those watching `chunk`, where
    /// it was seen last.
    Discard { entity: EntityId, chunk: ChunkPos },
}

/// What kind of thing an entity is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EntityKind {
    Player { player: PlayerId, name: String },
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
    /// which for a block item places it against that face. `sequence` is as for `Dig`.
    UseItemOn {
        position: BlockPos,
        face: Face,
        sequence: i32,
    },
    /// The player selected another hotbar slot, from 0 to 8.
    SelectSlot { slot: u8 },
    /// A creative-mode player put a stack into a hotbar slot, or emptied it.
    SetHotbarSlot { slot: u8, stack: Option<ItemStack> },
}

/// Everything that happened since the previous tick.
///
/// A tick applies all of `player_changes` and then all of `inputs`. What players did
/// and what became of them arrives as one sequence, though, and its order is lost when
/// it is sorted into the two. [`TickInputs::change`] and [`TickInputs::input`] sort it
/// so that nothing a player did before a change is applied after it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TickInputs {
    /// Players entering and leaving, in the order it happened. The order matters: a
    /// player can leave and come back, or join and leave, within one tick.
    pub player_changes: Vec<PlayerChange>,
    /// What players did, in the order it arrived. The inputs of a player are numbered in
    /// ascending order. An input whose number is not above that of the player's last
    /// applied input is ignored, so that inputs can be sent again to the region a player
    /// has moved to without any being applied twice.
    ///
    /// Only what a player did after the last of their changes in `player_changes`
    /// belongs here; see [`TickInputs::change`].
    pub inputs: Vec<(PlayerId, u64, PlayerInput)>,
    /// Chunks someone started to need. A chunk stays loaded while it has tickets.
    pub tickets_added: Vec<ChunkPos>,
    /// Chunks someone stopped needing; one entry releases one ticket.
    pub tickets_removed: Vec<ChunkPos>,
    /// Chunks that storage delivered in answer to earlier [`TickOutput::chunk_requests`].
    pub chunks_loaded: Vec<(ChunkPos, Chunk)>,
}

impl TickInputs {
    /// Adds what became of a player, which came after everything added so far.
    ///
    /// What that player did before, as far as it waits here for the coming tick, is
    /// dropped. It was meant for a player who was not in the region or no longer is:
    ///
    /// - Before a join or an arrival the player was not there, so the region would have
    ///   ignored it. Applied after the arrival instead, it would be taken for what the
    ///   player did since, and an input sent to the region while the player was away
    ///   would be applied ahead of earlier ones that are sent again with the arrival,
    ///   which then count as applied already and are lost.
    /// - Before leaving, it was the last a player did. If they are back within the
    ///   tick, it must not be the first thing their new self does, and its number must
    ///   not make the region ignore what they really do.
    pub fn change(&mut self, change: PlayerChange) {
        let player = match &change {
            PlayerChange::Join(join) => Some(join.player),
            PlayerChange::Leave(player) | PlayerChange::Arrive(player, _) => Some(*player),
            PlayerChange::Discard { .. } => None,
        };
        if let Some(player) = player {
            self.inputs.retain(|(actor, ..)| *actor != player);
        }
        self.player_changes.push(change);
    }

    /// Adds something a player did, which came after everything added so far.
    pub fn input(&mut self, player: PlayerId, number: u64, input: PlayerInput) {
        self.inputs.push((player, number, input));
    }
}

/// Something that concerns a single player.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlayerEvent {
    /// The player has entered the world.
    Spawned {
        entity_id: EntityId,
        position: Vec3,
        hotbar: [Option<ItemStack>; HOTBAR_SLOTS],
        selected_slot: u8,
    },
    /// Everything the player did with a sequence number up to `sequence` has been
    /// handled, whether it took effect or not. The client then stops showing its own
    /// guess of the outcome and shows what the region reported.
    Acknowledged { sequence: i32 },
    /// The player has stepped out of the region's part of the world and is no longer in
    /// the region. Whoever routes the player passes this on to the region they are in
    /// now as [`PlayerChange::Arrive`], together with every input numbered above
    /// [`PlayerTransfer::last_input`].
    Departed(PlayerTransfer),
    /// The player could not enter the world, because the region has no entity id left
    /// for them.
    Refused,
}

/// Something that happened in the region and concerns everyone who can see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RegionEvent {
    /// An entity has come into existence.
    EntitySpawned(EntityState),
    /// An entity has ceased to exist. `chunk` is where it was last.
    EntityRemoved { entity: EntityId, chunk: ChunkPos },
    /// A block has changed.
    BlockChanged {
        position: BlockPos,
        state: BlockState,
    },
    /// An entity has a new pose. At most one per entity and tick.
    EntityMoved {
        entity: EntityId,
        pose: Pose,
        /// The chunk the entity was in before, which differs from the chunk of the new
        /// position when it crossed a chunk border.
        previous_chunk: ChunkPos,
    },
}

impl RegionEvent {
    /// The chunks from which the event can be observed.
    pub fn chunks(&self) -> [ChunkPos; 2] {
        match self {
            Self::EntitySpawned(state) => [state.chunk(); 2],
            Self::EntityRemoved { chunk, .. } => [*chunk; 2],
            Self::BlockChanged { position, .. } => [position.chunk(); 2],
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

/// What a tick resulted in.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TickOutput {
    /// The number of this tick; the first tick of a region is 1.
    pub tick: u64,
    pub player_events: Vec<(PlayerId, PlayerEvent)>,
    pub events: Vec<RegionEvent>,
    /// Chunks that have to be fetched from storage and passed in through
    /// [`TickInputs::chunks_loaded`].
    pub chunk_requests: Vec<ChunkPos>,
}
