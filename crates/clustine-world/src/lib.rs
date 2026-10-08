//! In-memory world model: chunks, 16^3 sections, palettes, block entities.
//!
//! Nothing here knows about the network protocol or about storage; those convert from
//! and to these types.

mod chunk;
mod light;
mod position;
mod section;

pub use chunk::{Chunk, ChunkGenerator};
pub use light::{LIGHT_ARRAY_LENGTH, SectionLight, sky_light};
pub use position::{
    BlockPos, ChunkArea, ChunkPos, EdgeId, EntityId, EntityIds, PlayerId, RegionId, Vec3,
};
pub use section::{Biome, Section};

/// Blocks along each edge of a section.
pub const SECTION_SIZE: usize = 16;
/// Block positions in a section.
pub const BLOCKS_PER_SECTION: usize = SECTION_SIZE * SECTION_SIZE * SECTION_SIZE;
/// Columns in a chunk.
pub const COLUMNS_PER_CHUNK: usize = SECTION_SIZE * SECTION_SIZE;
