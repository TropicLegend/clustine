//! A dimension of the game's world with its features, as the stages of the scheduler
//! (`docs/groundwork/f-plan.md`, section 3): the land of [`TerrainGenerator`] for each
//! chunk by itself, then the runs of `clustine-features` in the order of ADR-0021.
//!
//! The structures start and bend the ground, and the runs are handed their starts and
//! references; their blocks are placed only where the features' settings say so,
//! which until phase S's switch only the judge does (`docs/groundwork/s-plan.md`).

use std::sync::Arc;

use clustine_data::DimensionType;
use clustine_features::FeatureSettings;
use clustine_structures::template_blocks::TemplateLibrary;
use clustine_terrain::biome::Dimension;
use clustine_world::BlockPos;
use clustine_world::ChunkGenerator;
use clustine_world::ChunkPos;
use clustine_world::generation::{
    BlockBox, FromNoise, GeneratorSettings, Neighbourhood, PieceState, StartRef, dimension_number,
};
use clustine_world::{Chunk, Entity};

use crate::TerrainGenerator;
use crate::spawning::ChunkSpawnView;
use crate::stages::{PieceMade, Stages, Start, Starts, StartsNear, Terrain, WorldgenHeights};

/// Raised whenever a change makes any chunk of these stages come out otherwise
/// (ADR-0021, section 8). Which kinds of features are built is in the identity.
///
/// 2 (2026-10-16): fossils and the template feature give the blocks at their edges the
/// features' shapes (gravel beside them schedules its tick), and their shape fix-ups
/// mark nothing, as the game's.
///
/// 3 (2026-10-21): a chunk is made with its animals (N7l), and the identity names the
/// generation of what is made with chunks, `clustine_sim::generation::ENTITIES_GENERATION`.
///
/// 4 (2026-10-26): the structures make their entities, and the mansion handles only
/// the markers in the centre chunk, as the game's `filterBlocks` does.
pub const FEATURES_VERSION: u16 = 4;

/// A dimension of a seed's world with its features, as stages.
#[derive(Debug, Clone)]
pub struct FeatureStages {
    terrain: Arc<TerrainGenerator>,
    features: Arc<FeatureSettings>,
    /// What makes the records of the entities runs and the spawn make, from the
    /// simulation's own code (`docs/groundwork/n7l-plan.md`, section 2.1).
    generation: clustine_sim::generation::Generation,
}

impl FeatureStages {
    /// The stages of `terrain`'s world, without the blocks of structure templates:
    /// a feature built of one places nothing. Reading the features' data takes a
    /// moment; make them once.
    pub fn new(terrain: Arc<TerrainGenerator>) -> Self {
        Self::of(terrain, None)
    }

    /// The stages of `terrain`'s world with the blocks of the structure templates,
    /// read from the operator's jar ([`crate::templates::read_jar`]).
    pub fn with_templates(terrain: Arc<TerrainGenerator>, templates: Arc<TemplateLibrary>) -> Self {
        Self::of(terrain, Some(templates))
    }

    /// The stages of `terrain`'s world with features read and changed by the caller:
    /// for the judge, which turns some off on both sides and places the structures'
    /// blocks of `terrain`'s starts ([`FeatureSettings::with_structures`]).
    pub fn with_features(terrain: Arc<TerrainGenerator>, features: FeatureSettings) -> Self {
        let generation = generation_of(&terrain);
        Self {
            terrain,
            features: Arc::new(features),
            generation,
        }
    }

    fn of(terrain: Arc<TerrainGenerator>, templates: Option<Arc<TemplateLibrary>>) -> Self {
        let mut features = FeatureSettings::new(terrain.seed(), terrain.level())
            .expect("the features of the pinned jar's data are read whole by every build");
        if let Some(templates) = templates {
            features = features.with_templates(templates);
        }
        // The structures' blocks are placed wherever the terrain has their starts
        // (phase S's switch, S3: `docs/groundwork/s-plan.md`, section 4); the identity
        // names their generation, so that a world made before is refused.
        if terrain.with_structures() {
            features = features.with_structures(terrain.clone());
        }
        let generation = generation_of(&terrain);
        Self {
            terrain,
            features: Arc::new(features),
            generation,
        }
    }
}

/// The maker of `terrain`'s world: its entities' own streams keyed by its seed, so that
/// a chunk made twice makes the same records.
fn generation_of(terrain: &TerrainGenerator) -> clustine_sim::generation::Generation {
    clustine_sim::generation::Generation {
        world: terrain.seed().cast_unsigned(),
    }
}

impl Stages for FeatureStages {
    fn maker(&self) -> &dyn clustine_world::generation::Maker {
        &self.generation
    }

    fn settings(&self) -> GeneratorSettings {
        // The batches of kinds built so far are in the identity, so that a world
        // made while fewer were built is refused rather than continued with more,
        // which would leave a seam between its chunks (the plan, section 3).
        let structures = if self.terrain.with_structures() {
            ""
        } else {
            ";no structures"
        };
        // Without the templates' blocks fossils and wells are not placed, and a world
        // made so is not to be continued with them.
        let templates = if self.features.templates().is_some() {
            ""
        } else {
            ";no templates"
        };
        // Whose blocks are placed is a generation of its own while phase S is built.
        let placed = if self.features.structures().is_some() {
            format!(";structures {}", clustine_features::STRUCTURES_GENERATION)
        } else {
            String::new()
        };
        GeneratorSettings {
            identity: format!(
                "terrain;features {}{placed};entities {};{};seed {}{structures}{templates}",
                clustine_features::GENERATION,
                clustine_sim::generation::ENTITIES_GENERATION,
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

    fn starts(&self, source: ChunkPos) -> Starts {
        let starts = self.terrain.starts(source);
        Starts {
            starts: starts
                .iter()
                .map(|start| Start {
                    structure: start.data().name,
                    pieces: start
                        .pieces
                        .iter()
                        .map(|piece| {
                            let (min, max) = (
                                piece.bounding_box.min_corner(),
                                piece.bounding_box.max_corner(),
                            );
                            PieceMade {
                                bounds: BlockBox {
                                    min: BlockPos::new(min.x, min.y, min.z),
                                    max: BlockPos::new(max.x, max.y, max.z),
                                },
                                state: PieceState::default(),
                            }
                        })
                        .collect(),
                })
                .collect(),
        }
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
        // The starts within eight whose box reaches into the chunk, as the game finds
        // them (`createReferences`); each by its place among its source's starts. The
        // terrain's own starts are those the scheduler hands, made by the same function.
        let references =
            clustine_structures::Structures::references(position.x, position.z, |x, z| {
                self.terrain.starts(ChunkPos::new(x, z))
            })
            .into_iter()
            .map(|(structure, (x, z))| {
                let source = ChunkPos::new(x, z);
                let index = self
                    .terrain
                    .starts(source)
                    .iter()
                    .position(|start| start.structure == structure)
                    .expect("a reference names a start of its source");
                StartRef {
                    source,
                    index: u16::try_from(index).expect("a chunk starts few structures"),
                }
            })
            .collect();
        Terrain {
            chunk,
            worldgen_heights,
            references,
        }
    }

    /// The runs make the structures' entities by the simulation's own code
    /// (`docs/groundwork/n7l-plan.md`, section 5.4).
    fn decorate(&self, view: &mut dyn Neighbourhood, centre: ChunkPos) {
        clustine_features::decorate_with(
            &self.features,
            view,
            centre,
            &self.generation,
            dimension_number(self.dimension().name),
        );
    }

    /// The game's `spawnOriginalMobs` (`docs/groundwork/n7l-plan.md`, section 3.2): W
    /// seeded for the chunk's decoration, R the region random the chunk's run began
    /// with, made anew.
    fn spawn(&self, position: ChunkPos, lit: &Chunk, nine: [&Chunk; 9]) -> Vec<Entity> {
        if !makes_mobs(self.terrain.level()) {
            return Vec::new();
        }
        let view = ChunkSpawnView {
            position,
            lit,
            nine,
            zoom: self.features.zoom(),
            sea_level: TerrainGenerator::sea_level_of(self.terrain.level()),
        };
        let seed = self.terrain.seed();
        let mut spawn = clustine_sim::generation::SpawnRandom::new(seed, position);
        let mut region = clustine_features::context::region_random(
            seed,
            self.features.legacy_random_source(),
            position,
        );
        clustine_sim::generation::spawn_chunk(
            &view,
            self.generation.world,
            dimension_number(self.dimension().name),
            &mut spawn,
            &mut FromNoise(&mut region),
        )
    }
}

/// Whether a dimension makes mobs with its chunks: not where its noise settings say
/// `disable_mob_generation`, as the End's do (`NoiseBasedChunkGenerator.spawnOriginalMobs`).
/// The game rule `spawn_mobs`, which turns it off too, is always on here.
pub fn makes_mobs(dimension: Dimension) -> bool {
    !clustine_terrain::biome::noise_settings(dimension).disable_mob_generation
}
