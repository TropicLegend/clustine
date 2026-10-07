//! A chunk column: a stack of sections.

use clustine_data::{BlockState, DimensionType, blocks};

use crate::{Biome, COLUMNS_PER_CHUNK, ChunkPos, SECTION_SIZE, Section};

/// A 16×16 column of the world from the bottom of its dimension to the top.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// The y coordinate of the lowest block.
    min_y: i32,
    /// From bottom to top.
    sections: Vec<Section>,
}

impl Chunk {
    /// An empty chunk column for a dimension of the given type.
    pub fn empty(dimension: &DimensionType, biome: Biome) -> Self {
        let section_count = dimension.height as usize / SECTION_SIZE;
        Self {
            min_y: dimension.min_y,
            sections: vec![Section::filled(blocks::AIR, biome); section_count],
        }
    }

    /// The y coordinate of the lowest block.
    pub fn min_y(&self) -> i32 {
        self.min_y
    }

    /// The number of blocks from the bottom of the column to its top.
    pub fn height(&self) -> usize {
        self.sections.len() * SECTION_SIZE
    }

    /// The sections from bottom to top.
    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    /// The block at `x` and `z` (each in `0..16`) and world height `y`, or `None` if `y`
    /// is below or above the dimension.
    pub fn get(&self, x: usize, y: i32, z: usize) -> Option<BlockState> {
        let (section, y) = self.locate(y)?;
        Some(self.sections[section].get(x, y, z))
    }

    /// Replaces the block at `x` and `z` (each in `0..16`) and world height `y`. Returns
    /// the old block, or `None` without changing anything if `y` is outside the dimension.
    pub fn set(&mut self, x: usize, y: i32, z: usize, state: BlockState) -> Option<BlockState> {
        let (section, y) = self.locate(y)?;
        Some(self.sections[section].set(x, y, z, state))
    }

    /// For each column, indexed by `z << 4 | x`, the height above the highest block that
    /// is not air, counted from the bottom of the dimension; 0 for an empty column.
    pub fn surface_heights(&self) -> [u16; COLUMNS_PER_CHUNK] {
        let mut heights = [0; COLUMNS_PER_CHUNK];
        let mut unresolved = COLUMNS_PER_CHUNK;
        for (index, section) in self.sections.iter().enumerate().rev() {
            if section.non_air_count() == 0 {
                continue;
            }
            for y in (0..SECTION_SIZE).rev() {
                for z in 0..SECTION_SIZE {
                    for x in 0..SECTION_SIZE {
                        let height = &mut heights[z << 4 | x];
                        if *height == 0 && section.get(x, y, z) != blocks::AIR {
                            *height = (index * SECTION_SIZE + y + 1) as u16;
                            unresolved -= 1;
                        }
                    }
                }
                if unresolved == 0 {
                    return heights;
                }
            }
        }
        heights
    }

    /// The section index and the y coordinate within that section for world height `y`.
    fn locate(&self, y: i32) -> Option<(usize, usize)> {
        let above_bottom = usize::try_from(y.checked_sub(self.min_y)?).ok()?;
        (above_bottom < self.height())
            .then_some((above_bottom / SECTION_SIZE, above_bottom % SECTION_SIZE))
    }
}

/// Produces the initial contents of chunks that have never been stored.
///
/// A generator must be deterministic: the same position always yields the same chunk,
/// because unmodified chunks are regenerated instead of being persisted.
pub trait ChunkGenerator: Send + Sync {
    fn generate(&self, position: ChunkPos) -> Chunk;
}

#[cfg(test)]
mod tests {
    use clustine_data::DIMENSION_TYPES;

    use super::*;

    fn overworld() -> &'static DimensionType {
        DIMENSION_TYPES
            .iter()
            .find(|dimension| dimension.name == "minecraft:overworld")
            .unwrap()
    }

    fn empty() -> Chunk {
        Chunk::empty(overworld(), Biome(0))
    }

    #[test]
    fn overworld_chunk_has_24_sections_from_minus_64() {
        let chunk = empty();
        assert_eq!(chunk.sections().len(), 24);
        assert_eq!(chunk.min_y(), -64);
        assert_eq!(chunk.height(), 384);
    }

    #[test]
    fn blocks_are_addressed_by_world_height() {
        let mut chunk = empty();
        assert_eq!(chunk.set(3, -64, 4, blocks::BEDROCK), Some(blocks::AIR));
        assert_eq!(chunk.set(3, 319, 4, blocks::STONE), Some(blocks::AIR));
        assert_eq!(chunk.get(3, -64, 4), Some(blocks::BEDROCK));
        assert_eq!(chunk.get(3, 319, 4), Some(blocks::STONE));
        assert_eq!(chunk.get(3, 0, 4), Some(blocks::AIR));
        assert_eq!(chunk.sections()[0].non_air_count(), 1);
        assert_eq!(chunk.sections()[23].non_air_count(), 1);
    }

    #[test]
    fn heights_outside_the_dimension_are_rejected() {
        let mut chunk = empty();
        assert_eq!(chunk.get(0, -65, 0), None);
        assert_eq!(chunk.get(0, 320, 0), None);
        assert_eq!(chunk.get(0, i32::MIN, 0), None);
        assert_eq!(chunk.set(0, 320, 0, blocks::STONE), None);
        assert_eq!(chunk, empty());
    }

    #[test]
    fn surface_heights_point_above_the_highest_block() {
        let mut chunk = empty();
        assert_eq!(chunk.surface_heights(), [0; 256]);

        chunk.set(0, -64, 0, blocks::BEDROCK);
        chunk.set(5, 100, 2, blocks::STONE);
        chunk.set(5, 20, 2, blocks::STONE);
        chunk.set(15, 319, 15, blocks::STONE);
        let heights = chunk.surface_heights();
        assert_eq!(heights[0], 1);
        assert_eq!(heights[2 << 4 | 5], 165);
        assert_eq!(heights[255], 384);
        assert_eq!(heights.iter().filter(|height| **height == 0).count(), 253);
    }
}
