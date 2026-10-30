//! In-memory world model: chunks, 16^3 sections, palettes, block entities.
//!
//! Nothing here knows about the network protocol or about storage; those convert from
//! and to these types.

mod bytes;
mod change;
mod chunk;
pub mod components;
mod difficulty;
mod dimension;
pub mod dragon;
mod effects;
mod entity;
pub mod generation;
mod light;
pub mod poi;
mod position;
pub mod projectiles;
pub mod raid;
mod section;
pub mod spawn;
pub mod trading;
mod travel;

pub use change::Change;
pub use chunk::{
    BlockEntity, Chunk, ChunkGenerator, DataRefused, Heights, InChunk, KeptBox, KeptEvent,
    KeptKind, KeptStructure, MAX_BLOCK_ENTITY_DATA, MAX_CHUNK_ENTITY_DATA, OverrideBox,
    ScheduledTick, Seam, SeamKind, Ticked, light_tells_apart,
};
pub use components::{Component, Components};
pub use difficulty::Difficulty;
pub use dimension::Dimension;
pub use effects::{
    Arrival, Blast, Causer, Due, Effect, EffectId, EffectKey, Effected, EntityUse, FiledEffect,
    GivenEffects, HitExtra, HurtBy, RaidNews, RaiderUse, Thrower, Told, ToldWhat, VillagerUse,
};
pub use entity::{
    ARMOUR_START, Attached, Contents, Double, ENDER_SLOTS, EnderChest, Entity, EntityChange,
    EntityData, EntityField, Entry, Float, HOTBAR_SLOTS, Hurt, INVENTORY_SLOTS, Inventory,
    ItemStack, Leash, LeashHolder, MAIN_START, MAX_CHUNK_ENTITIES, OFF_HAND, Pending, Riding, Seat,
    TypeState,
};
pub use light::{LIGHT_ARRAY_LENGTH, Light, SectionLight, sky_light};
pub use position::{
    BlockPos, ChunkArea, ChunkPos, EdgeId, EntityId, EntityIds, Generated, PlayerId, RegionId, Vec3,
};
pub use projectiles::{
    Arrow, ArrowKind, Cloud, Fangs, Fireball, Firework, InGround, Pickup, ProjectileOwner, Spit,
};
pub use section::{BIOME_CELLS, Biome, Section};
pub use travel::{
    LevelChunk, PortalAxis, PortalClock, PortalKind, PortalState, Travel, Traveller, Way,
};

/// Blocks along each edge of a section.
pub const SECTION_SIZE: usize = 16;
/// Block positions in a section.
pub const BLOCKS_PER_SECTION: usize = SECTION_SIZE * SECTION_SIZE * SECTION_SIZE;
/// Columns in a chunk.
pub const COLUMNS_PER_CHUNK: usize = SECTION_SIZE * SECTION_SIZE;
