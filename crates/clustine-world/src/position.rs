//! Positions of blocks and chunks.

/// The position of a block in the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// The chunk column this block is in.
    pub const fn chunk(self) -> ChunkPos {
        ChunkPos {
            x: self.x >> 4,
            z: self.z >> 4,
        }
    }

    /// The x and z coordinates within the chunk column, each in `0..16`.
    pub const fn in_chunk(self) -> (usize, usize) {
        ((self.x & 15) as usize, (self.z & 15) as usize)
    }
}

/// The position of a chunk column: block coordinates divided by 16, rounded down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

impl ChunkPos {
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_coordinates_round_down() {
        let block = BlockPos::new(-1, 70, -17);
        assert_eq!(block.chunk(), ChunkPos::new(-1, -2));
        assert_eq!(block.in_chunk(), (15, 15));

        let block = BlockPos::new(16, 0, 31);
        assert_eq!(block.chunk(), ChunkPos::new(1, 1));
        assert_eq!(block.in_chunk(), (0, 15));
    }
}
