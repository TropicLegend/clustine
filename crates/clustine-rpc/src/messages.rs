//! The messages services exchange.

use clustine_data::BlockState;
use clustine_sim::api::{EntityState, PlayerEvent, PlayerInput, PlayerJoin, RegionEvent};
use clustine_world::{BlockPos, Chunk, ChunkPos, PlayerId};
use serde::{Deserialize, Serialize};

/// What an edge tells the worker that owns a region. See
/// `docs/adr/0005-edge-worker-interface.md`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EdgeToWorker {
    /// A player has logged in and wants to enter the world.
    PlayerJoin(PlayerJoin),
    /// A player's connection has ended.
    PlayerLeave { player: PlayerId },
    /// Something a player did.
    Input {
        player: PlayerId,
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
