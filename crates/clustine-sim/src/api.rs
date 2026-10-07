//! What goes into a region's tick and what comes out of it.
//!
//! These types cross the boundary between the worker and the edge, so they have to stay
//! serialisable and free of anything specific to the Minecraft protocol.

use clustine_world::{Chunk, ChunkPos, EntityId, PlayerId, Vec3};
use serde::{Deserialize, Serialize};

/// A player entering the region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerJoin {
    pub player: PlayerId,
    pub name: String,
}

/// Everything that happened since the previous tick.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TickInputs {
    pub joins: Vec<PlayerJoin>,
    pub leaves: Vec<PlayerId>,
    /// Chunks someone started to need. A chunk stays loaded while it has tickets.
    pub tickets_added: Vec<ChunkPos>,
    /// Chunks someone stopped needing; one entry releases one ticket.
    pub tickets_removed: Vec<ChunkPos>,
    /// Chunks that storage delivered in answer to earlier [`TickOutput::chunk_requests`].
    pub chunks_loaded: Vec<(ChunkPos, Chunk)>,
}

/// Something that concerns a single player.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlayerEvent {
    /// The player has entered the world.
    Spawned { entity_id: EntityId, position: Vec3 },
}

/// What a tick resulted in.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TickOutput {
    /// The number of this tick; the first tick of a region is 1.
    pub tick: u64,
    pub player_events: Vec<(PlayerId, PlayerEvent)>,
    /// Chunks that have to be fetched from storage and passed in through
    /// [`TickInputs::chunks_loaded`].
    pub chunk_requests: Vec<ChunkPos>,
}
