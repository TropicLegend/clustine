//! What the spawning of a chunk reads (`docs/groundwork/n7l-plan.md`, sections 2.1 and
//! 3.2): the lit chunk and the nine finished chunks around it, as a
//! [`SpawnView`].

use clustine_data::{BlockState, blocks};
use clustine_sim::generation::SpawnView;
use clustine_terrain::biome::zoom::Zoom;
use clustine_world::generation::FinalHeightmap;
use clustine_world::{Biome, BlockPos, Chunk, ChunkPos, SECTION_SIZE};

/// The lit centre and the nine finished chunks, in the order of `lattice::around`
/// (rows of z, x within each; the centre fifth), as the spawn reads them.
pub struct ChunkSpawnView<'a> {
    pub position: ChunkPos,
    pub lit: &'a Chunk,
    pub nine: [&'a Chunk; 9],
    pub zoom: Zoom,
    pub sea_level: i32,
}

impl ChunkSpawnView<'_> {
    /// The chunk of the nine that holds the column of `at`, if any does.
    fn chunk_of(&self, at: BlockPos) -> Option<&Chunk> {
        let chunk = at.chunk();
        let (dx, dz) = (chunk.x - self.position.x, chunk.z - self.position.z);
        if dx.abs() > 1 || dz.abs() > 1 {
            return None;
        }
        if (dx, dz) == (0, 0) {
            return Some(self.lit);
        }
        Some(self.nine[((dz + 1) * 3 + dx + 1) as usize])
    }

    /// The column of `at` within its chunk.
    fn column(at: BlockPos) -> (usize, usize) {
        (at.x.rem_euclid(16) as usize, at.z.rem_euclid(16) as usize)
    }

    /// The noise biome of the cell of `at` (`LevelReader.getNoiseBiome`): its height
    /// held within the chunk's cells, as a chunk of the game holds it. The plains
    /// beyond the nine, which neither the zoom nor the spawn reaches.
    fn cell_biome(&self, at: BlockPos) -> Biome {
        let Some(chunk) = self.chunk_of(at) else {
            return Biome(0);
        };
        let top = chunk.min_y() + chunk.height() as i32 - 1;
        let above_bottom = (at.y.clamp(chunk.min_y(), top) - chunk.min_y()) as usize;
        let (x, z) = Self::column(at);
        chunk.sections()[above_bottom / SECTION_SIZE].biome_at(x, above_bottom % SECTION_SIZE, z)
    }
}

impl SpawnView for ChunkSpawnView<'_> {
    fn position(&self) -> ChunkPos {
        self.position
    }

    fn max_y(&self) -> i32 {
        self.lit.min_y() + self.lit.height() as i32 - 1
    }

    fn sea_level(&self) -> i32 {
        self.sea_level
    }

    fn block(&self, at: BlockPos) -> BlockState {
        let Some(chunk) = self.chunk_of(at) else {
            return blocks::AIR;
        };
        let (x, z) = Self::column(at);
        chunk.get(x, at.y, z).unwrap_or(blocks::AIR)
    }

    fn height(&self, map: FinalHeightmap, x: i32, z: i32) -> i32 {
        let Some(chunk) = self.chunk_of(BlockPos::new(x, 0, z)) else {
            return self.lit.min_y();
        };
        let heights = chunk.heights();
        let (x, z) = Self::column(BlockPos::new(x, 0, z));
        let column = z << 4 | x;
        let above_bottom = match map {
            FinalHeightmap::WorldSurface => heights.world_surface[column],
            FinalHeightmap::OceanFloor => heights.ocean_floor[column],
            FinalHeightmap::MotionBlocking => heights.motion_blocking[column],
            FinalHeightmap::MotionBlockingNoLeaves => heights.motion_blocking_no_leaves[column],
        };
        chunk.min_y() + i32::from(above_bottom)
    }

    /// The lit centre's light; beside it, or where the centre has none, the open sky.
    fn sky_light(&self, at: BlockPos) -> u8 {
        if at.chunk() != self.position {
            return 15;
        }
        let (x, z) = Self::column(at);
        self.lit
            .light()
            .map_or(15, |light| light.sky_at(self.lit.min_y(), x, at.y, z))
    }

    fn block_light(&self, at: BlockPos) -> u8 {
        if at.chunk() != self.position {
            return 0;
        }
        let (x, z) = Self::column(at);
        self.lit
            .light()
            .map_or(0, |light| light.block_at(self.lit.min_y(), x, at.y, z))
    }

    fn zoomed_biome(&self, at: BlockPos) -> Biome {
        let (x, y, z) = self.zoom.cell(at.x, at.y, at.z);
        self.cell_biome(BlockPos::new(x << 2, y << 2, z << 2))
    }

    fn noise_biome(&self, at: BlockPos) -> Biome {
        self.cell_biome(at)
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::DIMENSION_TYPES;

    use super::*;

    fn overworld() -> &'static clustine_data::DimensionType {
        DIMENSION_TYPES
            .iter()
            .find(|dimension| dimension.name == "minecraft:overworld")
            .expect("the overworld")
    }

    /// The view finds blocks and biomes in the nine by position, light in the lit
    /// centre, and the corner's top at the dimension's highest block.
    #[test]
    fn the_view_reads_the_nine_by_position() {
        let mut chunks: Vec<Chunk> = (0..9)
            .map(|index| Chunk::empty(overworld(), Biome(index)))
            .collect();
        // The chunk west of the centre (dx −1, dz 0) is the fourth.
        chunks[3].set(15, 70, 2, blocks::STONE);
        let nine: [&Chunk; 9] = std::array::from_fn(|index| &chunks[index]);
        let view = ChunkSpawnView {
            position: ChunkPos::new(5, -2),
            lit: &chunks[4],
            nine,
            zoom: Zoom::new(1),
            sea_level: 63,
        };
        assert_eq!(view.max_y(), 319);
        assert_eq!(view.block(BlockPos::new(79, 70, -30)), blocks::STONE);
        assert_eq!(view.block(BlockPos::new(80, 70, -30)), blocks::AIR);
        assert_eq!(view.noise_biome(BlockPos::new(79, 400, -30)), Biome(3));
        assert_eq!(view.noise_biome(BlockPos::new(100, 70, -16)), Biome(8));
        assert_eq!(view.height(FinalHeightmap::MotionBlocking, 85, -30), -64);
        // No light: the open sky.
        assert_eq!(view.sky_light(BlockPos::new(85, 70, -30)), 15);
        assert_eq!(view.block_light(BlockPos::new(85, 70, -30)), 0);
    }

    /// The End's noise settings disable the making of mobs and the overworld's do not;
    /// the flat world makes its chunks without any entity.
    #[test]
    fn the_end_and_the_flat_world_make_no_animals() {
        use clustine_terrain::biome::Dimension;
        use clustine_world::ChunkGenerator;
        assert!(crate::features::makes_mobs(Dimension::Overworld));
        assert!(!crate::features::makes_mobs(Dimension::End));
        let flat = crate::FlatGenerator::classic();
        for x in -3..3 {
            for z in -3..3 {
                assert!(flat.generate(ChunkPos::new(x, z)).entities().is_empty());
            }
        }
    }

    /// The overworld's stages make the same animals whenever they make a chunk again,
    /// and only in it: nine chunks of grass in the plains, the seed's W and R.
    #[test]
    fn the_same_chunk_made_twice_makes_the_same_animals() {
        use crate::stages::Stages;
        let stages =
            crate::FeatureStages::new(std::sync::Arc::new(crate::TerrainGenerator::new(4)));
        let plains = clustine_data::synced_registry("minecraft:worldgen/biome")
            .and_then(|biomes| biomes.id_of("minecraft:plains"))
            .expect("the plains");
        let mut chunk = Chunk::empty(overworld(), Biome(plains as u16));
        for z in 0..16 {
            for x in 0..16 {
                chunk.set(x, 64, z, blocks::GRASS_BLOCK);
            }
        }
        let nine = [&chunk; 9];
        let mut made = 0;
        for x in 0..40 {
            let position = ChunkPos::new(x, -5);
            let first = stages.spawn(position, &chunk, nine);
            assert_eq!(first, stages.spawn(position, &chunk, nine));
            assert!(first.iter().all(|entity| entity.chunk() == position));
            made += first.len();
        }
        // About one chunk in ten makes a group.
        assert!(made > 0);
    }
}
