//! World generation service: stateless chunk generation.
//!
//! Two generators: the flat world, which a server has unless it is given a seed, and
//! [`TerrainGenerator`], a preview of the game's bare land for a seed (see
//! [`terrain`]).

pub mod features;
pub mod preview;
pub mod scheduler;
pub mod stages;
pub mod terrain;

use clustine_data::{BlockState, DIMENSION_TYPES, DimensionType, blocks, synced_registry};
use clustine_light::Around;
use clustine_world::generation::GeneratorSettings;
use clustine_world::{Biome, BlockPos, Chunk, ChunkGenerator, ChunkPos, SECTION_SIZE};

pub use clustine_world::spawn;

pub use crate::features::{FEATURES_VERSION, FeatureStages};
pub use crate::preview::PreviewStages;
pub use crate::terrain::{TERRAIN_VERSION, TerrainGenerator};

/// Generates a flat world: the same layers of blocks in every chunk.
#[derive(Debug, Clone)]
pub struct FlatGenerator {
    dimension: &'static DimensionType,
    biome: Biome,
    /// Blocks from the bottom of the world upwards, one per layer.
    layers: Vec<BlockState>,
    /// The chunk of every position, with its light: made once, since every chunk has
    /// the same blocks and stands beside eight like it.
    made: Chunk,
    /// Where a new world's spawn is, if not where the game's rule puts it.
    spawn: Option<BlockPos>,
}

impl FlatGenerator {
    /// The default superflat world of vanilla ("Classic Flat"): one layer of bedrock,
    /// two of dirt and one of grass blocks at the bottom of the overworld, in plains.
    pub fn classic() -> Self {
        let dimension = DIMENSION_TYPES
            .iter()
            .find(|dimension| dimension.name == "minecraft:overworld")
            .expect("the overworld is a vanilla dimension type");
        let plains = synced_registry("minecraft:worldgen/biome")
            .and_then(|biomes| biomes.id_of("minecraft:plains"))
            .expect("plains is a vanilla biome");
        let biome = Biome(plains as u16);
        let layers = vec![
            blocks::BEDROCK,
            blocks::DIRT,
            blocks::DIRT,
            blocks::GRASS_BLOCK,
        ];
        Self {
            dimension,
            biome,
            made: made(dimension, biome, &layers),
            layers,
            spawn: None,
        }
    }

    /// The same world with a new world's spawn at `spawn`, the block a player's feet
    /// are in, where the game's rule would put it on the first column of the chunk at
    /// the origin. For tests and trials that need players to enter somewhere else, such
    /// as beside a boundary between regions. A world keeps the spawn it was made with.
    pub fn with_spawn(mut self, spawn: BlockPos) -> Self {
        self.spawn = Some(spawn);
        self
    }

    /// The y coordinate of the lowest air block, where a player stands.
    pub fn surface_y(&self) -> i32 {
        self.dimension.min_y + self.layers.len() as i32
    }
}

/// The version of the flat generator: what a chunk says it is exactly as made by
/// (`docs/adr/0022-what-a-chunk-carries.md`, section 1). It is raised when the
/// generator would make another chunk from the same settings.
pub const FLAT_VERSION: u16 = 1;

/// The chunk of the given layers, lit as it is beside eight like it, and marked as
/// generated.
fn made(dimension: &DimensionType, biome: Biome, layers: &[BlockState]) -> Chunk {
    let mut chunk = Chunk::empty(dimension, biome);
    for (layer, state) in layers.iter().enumerate() {
        let y = dimension.min_y + layer as i32;
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                chunk.set(x, y, z, *state);
            }
        }
    }
    let mut light = clustine_light::light(&Around {
        centre: &chunk,
        neighbours: [Some(&chunk); 8],
        // The flat world is the overworld, which has a sky; a dimension type does not
        // say yet whether it has one.
        sky: true,
    });
    // The eight beside it are this generator's as well, which an edge goes by when
    // it asks whether the light still holds beside the chunks it has.
    light.beside = FLAT_VERSION;
    chunk.set_light(light);
    // Exactly as this generator makes it, until something changes it. Said last: the
    // blocks were put there by changes, each of which takes it back.
    chunk.mark_generated(FLAT_VERSION, false);
    chunk
}

impl ChunkGenerator for FlatGenerator {
    fn generate(&self, _position: ChunkPos) -> Chunk {
        self.made.clone()
    }

    fn spawn(&self) -> BlockPos {
        self.spawn
            .unwrap_or_else(|| clustine_world::spawn::place_to_stand(self.origin(), |_| &self.made))
    }

    fn settings(&self) -> GeneratorSettings {
        let layers: Vec<_> = self
            .layers
            .iter()
            .map(|state| state.block().map_or("unknown", |block| block.name))
            .collect();
        let identity = format!(
            "flat;{};from {};biome {}",
            layers.join(","),
            self.dimension.min_y,
            self.biome.0
        );
        GeneratorSettings {
            identity,
            version: FLAT_VERSION,
        }
    }
}

#[cfg(test)]
mod tests {
    use clustine_world::SectionLight;

    use super::*;

    #[test]
    fn the_flat_worlds_spawn_is_on_its_grass_at_the_origin_unless_it_is_set() {
        // The game's rule: the first column of the chunk at the origin, on the grass,
        // where players entered the flat world before a world kept its spawn.
        let flat = FlatGenerator::classic();
        assert_eq!(flat.spawn(), BlockPos::new(0, flat.surface_y(), 0));
        assert_eq!(flat.spawn(), BlockPos::new(0, -60, 0));
        let beside = BlockPos::new(13, -60, 8);
        assert_eq!(FlatGenerator::classic().with_spawn(beside).spawn(), beside);
    }

    #[test]
    fn classic_flat_has_four_layers_at_the_bottom() {
        let generator = FlatGenerator::classic();
        let chunk = generator.generate(ChunkPos::new(3, -7));

        for (x, z) in [(0, 0), (15, 15), (7, 9)] {
            assert_eq!(chunk.get(x, -64, z), Some(blocks::BEDROCK));
            assert_eq!(chunk.get(x, -63, z), Some(blocks::DIRT));
            assert_eq!(chunk.get(x, -62, z), Some(blocks::DIRT));
            assert_eq!(chunk.get(x, -61, z), Some(blocks::GRASS_BLOCK));
            assert_eq!(chunk.get(x, -60, z), Some(blocks::AIR));
        }
        assert_eq!(generator.surface_y(), -60);
        assert_eq!(chunk.sections()[0].non_air_count(), 4 * 256);
        assert!(
            chunk.sections()[1..]
                .iter()
                .all(|section| section.non_air_count() == 0)
        );
        assert_eq!(chunk.surface_heights(), [4; 256]);
    }

    #[test]
    fn settings_describe_the_layers() {
        let settings = FlatGenerator::classic().settings();
        assert_eq!(settings.version, FLAT_VERSION);
        let settings = settings.identity;
        assert!(
            settings.starts_with(
                "flat;minecraft:bedrock,minecraft:dirt,minecraft:dirt,minecraft:grass_block;from -64;"
            ),
            "{settings}"
        );
    }

    #[test]
    fn a_flat_chunk_carries_the_light_of_its_blocks_beside_eight_like_it() {
        let generator = FlatGenerator::classic();
        let chunk = generator.generate(ChunkPos::new(3, -7));
        let light = chunk.light().expect("a flat chunk has light");
        assert_eq!(light.made_by, clustine_light::VERSION);
        assert_eq!(light.sky.len(), chunk.sections().len() + 2);
        assert_eq!(light.block.len(), chunk.sections().len() + 2);

        // Made again from the chunk's blocks alone, without its light or its mark.
        let bare = Chunk::from_sections(chunk.min_y(), chunk.sections().to_vec());
        assert_eq!(bare.light(), None);
        let mut again = clustine_light::light(&Around {
            centre: &bare,
            neighbours: [Some(&bare); 8],
            sky: true,
        });
        // The rules do not say whose chunks they lit; the generator does.
        assert_eq!(again.beside, 0);
        again.beside = FLAT_VERSION;
        assert_eq!(*light, again);
    }

    /// Test 6 of ADR-0022: the edge's packet is made from `sky_light` until it is made
    /// from what the chunk carries, and the flat world must not change at that step.
    #[test]
    fn a_flat_chunk_has_exactly_the_sky_light_that_is_reckoned_by_columns_today() {
        let chunk = FlatGenerator::classic().generate(ChunkPos::new(0, 0));
        let light = chunk.light().expect("a flat chunk has light");
        assert_eq!(light.sky, clustine_world::sky_light(&chunk));

        // Written out, so that the two cannot be wrong together unseen: dark below the
        // world, the ground's section dark in its four layers and 15 above them, the
        // next section full, nothing further up.
        assert_eq!(light.sky[0], SectionLight::Dark);
        let SectionLight::Levels(ground) = &light.sky[1] else {
            panic!("levels in the ground's section, not {:?}", light.sky[1]);
        };
        assert!(ground[..4 * 128].iter().all(|byte| *byte == 0));
        assert!(ground[4 * 128..].iter().all(|byte| *byte == 0xff));
        assert_eq!(light.sky[2], SectionLight::Full);
        assert!(light.sky[3..].iter().all(|s| *s == SectionLight::Absent));

        // Nothing gives light: block light is dark wherever the sky's is listed, which
        // is what the edge's packet says today.
        assert_eq!(light.block[..3], [const { SectionLight::Dark }; 3]);
        assert!(light.block[3..].iter().all(|s| *s == SectionLight::Absent));
    }

    #[test]
    fn a_flat_chunk_is_as_generated_and_lit_until_a_change() {
        let generator = FlatGenerator::classic();
        let chunk = generator.generate(ChunkPos::new(1, 1));
        assert_eq!(chunk.generated(), FLAT_VERSION);
        assert!(!chunk.with_failure());
        assert!(chunk.light().is_some());

        // Grass to dirt is a change light cannot see: no longer as generated, still lit.
        let mut dug = chunk.clone();
        dug.set(5, -61, 5, blocks::DIRT);
        assert_eq!(dug.generated(), 0);
        assert_eq!(dug.light(), chunk.light());

        // A hole is one it can.
        let mut holed = chunk.clone();
        holed.set(5, -61, 5, blocks::AIR);
        assert_eq!(holed.generated(), 0);
        assert_eq!(holed.light(), None);

        // The copy that was changed took nothing from the generator's own.
        assert_eq!(generator.generate(ChunkPos::new(1, 1)), chunk);
    }

    #[test]
    fn every_chunk_is_the_same() {
        let generator = FlatGenerator::classic();
        assert_eq!(
            generator.generate(ChunkPos::new(0, 0)),
            generator.generate(ChunkPos::new(-100, 2500))
        );
    }
}
