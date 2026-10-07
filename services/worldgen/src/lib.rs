//! World generation service: stateless chunk generation.

use clustine_data::{BlockState, DIMENSION_TYPES, DimensionType, blocks, synced_registry};
use clustine_world::{Biome, Chunk, ChunkGenerator, ChunkPos, SECTION_SIZE};

/// Generates a flat world: the same layers of blocks in every chunk.
#[derive(Debug, Clone)]
pub struct FlatGenerator {
    dimension: &'static DimensionType,
    biome: Biome,
    /// Blocks from the bottom of the world upwards, one per layer.
    layers: Vec<BlockState>,
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
        Self {
            dimension,
            biome: Biome(plains as u16),
            layers: vec![
                blocks::BEDROCK,
                blocks::DIRT,
                blocks::DIRT,
                blocks::GRASS_BLOCK,
            ],
        }
    }

    /// The y coordinate of the lowest air block, where a player stands.
    pub fn surface_y(&self) -> i32 {
        self.dimension.min_y + self.layers.len() as i32
    }
}

impl ChunkGenerator for FlatGenerator {
    fn generate(&self, _position: ChunkPos) -> Chunk {
        let mut chunk = Chunk::empty(self.dimension, self.biome);
        for (layer, state) in self.layers.iter().enumerate() {
            let y = self.dimension.min_y + layer as i32;
            for z in 0..SECTION_SIZE {
                for x in 0..SECTION_SIZE {
                    chunk.set(x, y, z, *state);
                }
            }
        }
        chunk
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn every_chunk_is_the_same() {
        let generator = FlatGenerator::classic();
        assert_eq!(
            generator.generate(ChunkPos::new(0, 0)),
            generator.generate(ChunkPos::new(-100, 2500))
        );
    }
}
