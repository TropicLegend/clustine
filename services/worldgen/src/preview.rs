//! The preview of [`crate::terrain`] as the stages of the scheduler, so that its land is
//! made on the scheduler's pool, with its cache and its look-ahead, rather than on the
//! world store's one thread for chunks.
//!
//! The preview has no features and no structures of its own beyond the ground it bends
//! for them, so it has no starts to hand on and its runs place nothing. What the
//! scheduler makes of it is the chunk [`TerrainGenerator`] makes: the same blocks,
//! biomes and marks, lit beside its eight neighbours, marked with the same version.

use std::sync::Arc;

use clustine_data::DimensionType;
use clustine_world::ChunkPos;
use clustine_world::generation::{GeneratorSettings, Neighbourhood};

use crate::TerrainGenerator;
use crate::stages::{Stages, Starts, StartsNear, Terrain, WorldgenHeights};
use clustine_world::ChunkGenerator;

/// The preview of a seed's land, as stages.
#[derive(Debug, Clone)]
pub struct PreviewStages {
    terrain: Arc<TerrainGenerator>,
}

impl PreviewStages {
    pub fn new(terrain: Arc<TerrainGenerator>) -> Self {
        Self { terrain }
    }
}

impl Stages for PreviewStages {
    fn settings(&self) -> GeneratorSettings {
        // The same identity and version as the preview's own: a world it made is
        // continued by these stages, chunk for chunk the same.
        self.terrain.settings()
    }

    fn dimension(&self) -> &'static DimensionType {
        self.terrain.dimension()
    }

    fn starts(&self, _source: ChunkPos) -> Starts {
        // The preview bends the ground for its starts itself and places none of them.
        Starts::default()
    }

    fn terrain(&self, position: ChunkPos, _starts: &dyn StartsNear) -> Terrain {
        let chunk = self.terrain.unlit(position);
        // Before features the final heightmaps are terrain's own, so the two that
        // features read frozen are the first two of them.
        let heights = chunk.heights();
        let worldgen_heights = Box::new(WorldgenHeights {
            world_surface: heights.world_surface,
            ocean_floor: heights.ocean_floor,
        });
        Terrain {
            chunk,
            worldgen_heights,
            references: Vec::new(),
        }
    }

    fn decorate(&self, _view: &mut dyn Neighbourhood, _centre: ChunkPos) {}
}
