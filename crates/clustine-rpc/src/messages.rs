//! The messages services exchange.

use clustine_sim::api::{PlayerEvent, PlayerInput, PlayerJoin, RegionEvent};
use clustine_world::{Chunk, ChunkPos, PlayerId};
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
    /// The state of a subscribed chunk after `tick`. Sent once per subscription.
    ChunkSnapshot {
        position: ChunkPos,
        tick: u64,
        chunk: Chunk,
    },
    /// What happened during `tick` in the chunks the edge is subscribed to.
    TickDelta { tick: u64, events: Vec<RegionEvent> },
    /// Something that concerns a single player on this edge.
    ToPlayer {
        player: PlayerId,
        event: PlayerEvent,
    },
}

/// What a worker asks of the world store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StoreRequest {
    /// Load the chunk, generating it if it has never been stored.
    Load { position: ChunkPos },
}

/// What the world store answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StoreReply {
    Loaded { position: ChunkPos, chunk: Chunk },
}
