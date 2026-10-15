//! The game's overworld with its features, as the stages of the scheduler
//! (`docs/groundwork/f-plan.md`, section 3): the land of [`TerrainGenerator`] for each
//! chunk by itself, then the runs of `clustine-features` in the order of ADR-0021.
//!
//! Until phase S the structures start and bend the ground, and no block of theirs is
//! placed: the runs have no starts to hand on. Until the plan's switch (step F6) the
//! served world stays the preview, and these stages are reached by a hidden setting.

use std::sync::Arc;

use clustine_data::DimensionType;
use clustine_features::FeatureSettings;
use clustine_terrain::biome::Dimension;
use clustine_world::ChunkGenerator;
use clustine_world::ChunkPos;
use clustine_world::generation::{GeneratorSettings, Neighbourhood};

use crate::TerrainGenerator;
use crate::stages::{Stages, Starts, StartsNear, Terrain, WorldgenHeights};

/// Raised whenever a change makes any chunk of these stages come out otherwise
/// (ADR-0021, section 8). Which kinds of features are built is in the identity.
pub const FEATURES_VERSION: u16 = 1;

/// A seed's overworld with its features, as stages.
#[derive(Debug, Clone)]
pub struct FeatureStages {
    terrain: Arc<TerrainGenerator>,
    features: Arc<FeatureSettings>,
}

impl FeatureStages {
    /// The stages of `terrain`'s world. Reading the features' data takes a moment;
    /// make them once.
    pub fn new(terrain: Arc<TerrainGenerator>) -> Self {
        let features = FeatureSettings::new(terrain.seed(), Dimension::Overworld)
            .expect("the features of the pinned jar's data are read whole by every build");
        Self {
            terrain,
            features: Arc::new(features),
        }
    }
}

impl Stages for FeatureStages {
    fn settings(&self) -> GeneratorSettings {
        // The batches of kinds built so far are in the identity, so that a world
        // made while fewer were built is refused rather than continued with more,
        // which would leave a seam between its chunks (the plan, section 3).
        let structures = if self.terrain.with_structures() {
            ""
        } else {
            ";no structures"
        };
        GeneratorSettings {
            identity: format!(
                "terrain;features {};{};seed {}{structures}",
                clustine_features::GENERATION,
                self.terrain.dimension().name,
                self.terrain.seed()
            ),
            version: FEATURES_VERSION,
        }
    }

    fn dimension(&self) -> &'static DimensionType {
        self.terrain.dimension()
    }

    fn origin(&self) -> ChunkPos {
        ChunkGenerator::origin(&*self.terrain)
    }

    fn starts(&self, _source: ChunkPos) -> Starts {
        // Pieces are placed in phase S; the ground is bent for them by the terrain.
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

    fn decorate(&self, view: &mut dyn Neighbourhood, centre: ChunkPos) {
        clustine_features::decorate(&self.features, view, centre);
    }
}
