//! In-memory world model: chunks, 16^3 sections, palettes, block entities.
//!
//! Nothing here knows about the network protocol or about storage; those convert from
//! and to these types.

mod bytes;
mod change;
mod chunk;
mod effects;
mod entity;
pub mod generation;
mod light;
mod position;
mod section;
pub mod spawn;

pub use change::Change;
pub use chunk::{
    BlockEntity, Chunk, ChunkGenerator, DataRefused, Heights, InChunk, KeptEvent,
    MAX_BLOCK_ENTITY_DATA, MAX_CHUNK_ENTITY_DATA, ScheduledTick, Seam, SeamKind, Ticked,
    light_tells_apart,
};
pub use effects::{
    Arrival, Blast, Due, Effect, EffectId, EffectKey, Effected, EntityUse, FiledEffect,
};
pub use entity::{
    ARMOUR_START, Attached, Contents, Double, ENDER_SLOTS, EnderChest, Entity, EntityChange,
    EntityData, EntityField, Float, HOTBAR_SLOTS, Hurt, INVENTORY_SLOTS, Inventory, ItemStack,
    Leash, LeashHolder, MAIN_START, MAX_CHUNK_ENTITIES, OFF_HAND, Pending, Riding, Seat,
};
pub use light::{LIGHT_ARRAY_LENGTH, Light, SectionLight, sky_light};
pub use position::{
    BlockPos, ChunkArea, ChunkPos, EdgeId, EntityId, EntityIds, PlayerId, RegionId, Vec3,
};
pub use section::{BIOME_CELLS, Biome, Section};

/// Blocks along each edge of a section.
pub const SECTION_SIZE: usize = 16;
/// Block positions in a section.
pub const BLOCKS_PER_SECTION: usize = SECTION_SIZE * SECTION_SIZE * SECTION_SIZE;
/// Columns in a chunk.
pub const COLUMNS_PER_CHUNK: usize = SECTION_SIZE * SECTION_SIZE;
