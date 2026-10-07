//! The messages services exchange.

use clustine_data::BlockState;
use clustine_region::{Layout, RegionId, RoutingTable};
use clustine_sim::api::{
    EntityState, PlayerEvent, PlayerInput, PlayerJoin, PlayerTransfer, RegionEvent,
};
use clustine_world::{BlockPos, Chunk, ChunkPos, EntityId, EntityIds, PlayerId, Vec3};
use serde::{Deserialize, Serialize};

/// What an edge tells the worker that owns a region. See
/// `docs/adr/0005-edge-worker-interface.md`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EdgeToWorker {
    /// A player has logged in and wants to enter the world.
    PlayerJoin(PlayerJoin),
    /// A player's connection has ended.
    PlayerLeave { player: PlayerId },
    /// A player has walked in from another region, which let them go with
    /// [`PlayerEvent::Departed`].
    PlayerArrive {
        player: PlayerId,
        transfer: PlayerTransfer,
    },
    /// The entity of a player who left while being handed over will not arrive. The
    /// region reports it as removed to those watching `chunk`, where it was seen last.
    Discard { entity: EntityId, chunk: ChunkPos },
    /// Something a player did. An edge numbers the inputs of a player in ascending
    /// order; see `TickInputs::inputs`.
    Input {
        player: PlayerId,
        number: u64,
        input: PlayerInput,
    },
    /// The edge wants a snapshot of these chunks, followed by every later change to
    /// them. A subscription also keeps the chunk loaded.
    Subscribe { chunks: Vec<ChunkPos> },
    /// The edge no longer needs these chunks.
    Unsubscribe { chunks: Vec<ChunkPos> },
}

/// What a worker tells an edge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WorkerToEdge {
    /// The state of a subscribed chunk after `tick`, with the entities that are in it.
    /// Sent once per subscription; later changes follow as [`WorkerToEdge::TickDelta`].
    ChunkSnapshot {
        position: ChunkPos,
        tick: u64,
        chunk: Chunk,
        entities: Vec<EntityState>,
    },
    /// What happened during `tick` in the chunks the edge is subscribed to.
    TickDelta { tick: u64, events: Vec<RegionEvent> },
    /// Something that concerns a single player on this edge.
    ToPlayer {
        player: PlayerId,
        event: PlayerEvent,
    },
}

/// What a worker asks of the world store. Requests are handled in the order they are made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StoreRequest {
    /// Load the chunk, generating it if it has never been stored. Answered with
    /// [`StoreReply::Loaded`].
    Load { position: ChunkPos },
    /// Store the chunk as it is after `tick`. Not answered.
    Save {
        position: ChunkPos,
        tick: u64,
        chunk: Chunk,
    },
    /// Record the block changes of `tick`, so that they are not lost if the process dies
    /// before the chunks they are in have been saved. Not answered.
    Log {
        tick: u64,
        changes: Vec<(BlockPos, BlockState)>,
    },
    /// Every change logged so far is contained in a chunk saved before this request, so
    /// the log can be emptied. Not answered.
    Checkpoint,
    /// Answer with [`StoreReply::Flushed`] once everything requested before is done.
    Flush,
}

/// What the world store answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StoreReply {
    Loaded { position: ChunkPos, chunk: Chunk },
    Flushed,
}

/// What a service says first on a connection to a worker or to the world store: which
/// region the connection is about and who the service takes its owner to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionHello {
    pub region: RegionId,
    /// The epoch of the region's owner; see [`Assignment::epoch`].
    pub epoch: u64,
    /// [`Layout::fingerprint`] of the layout the region is part of.
    pub layout: u64,
}

/// The answer to a [`RegionHello`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegionWelcome {
    /// The connection now carries the messages of the region.
    Accepted,
    /// The connection is closed, for the reason given.
    Refused { reason: String },
}

/// A region a worker has been given to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assignment {
    pub region: RegionId,
    /// Counts the owners the region has had. An owner with a lower epoch than another
    /// has been replaced.
    pub epoch: u64,
    /// The entity ids the region may hand out under this assignment.
    pub entity_ids: EntityIds,
}

/// What a worker or an edge tells the coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToCoordinator {
    /// A worker offers to run regions. Answered with [`FromCoordinator::Assigned`] or
    /// [`FromCoordinator::Refused`].
    RegisterWorker {
        /// Identifies the worker across restarts and reconnections.
        name: String,
        /// Host and port at which edges reach the worker.
        address: String,
        /// What the worker is running already, which is the case when it registers
        /// again after losing its connection, with the fingerprint of the layout those
        /// regions belong to.
        holding: Vec<Assignment>,
        layout: Option<u64>,
    },
    /// The worker is still there. A worker that is silent for too long loses its regions.
    Heartbeat,
    /// An edge wants the routing table, now and whenever it changes. Answered with
    /// [`FromCoordinator::Routing`].
    WatchRouting,
}

/// What the coordinator tells a worker or an edge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FromCoordinator {
    /// To a worker: how the world is divided and which regions the worker is to run.
    /// Sent again whenever that changes.
    Assigned {
        layout: Layout,
        /// Where players enter the world.
        spawn: Vec3,
        assignments: Vec<Assignment>,
    },
    /// To a worker: it cannot take part, for the reason given.
    Refused { reason: String },
    /// To an edge: the current routing table.
    Routing(RoutingTable),
}
