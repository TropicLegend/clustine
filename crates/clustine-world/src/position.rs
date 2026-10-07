//! Positions of blocks and chunks.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The position of a block in the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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

    /// The position `dx`, `dy` and `dz` blocks away. Saturates at the limits of `i32`.
    pub const fn offset(self, dx: i32, dy: i32, dz: i32) -> Self {
        Self {
            x: self.x.saturating_add(dx),
            y: self.y.saturating_add(dy),
            z: self.z.saturating_add(dz),
        }
    }

    /// The x and z coordinates within the chunk column, each in `0..16`.
    pub const fn in_chunk(self) -> (usize, usize) {
        ((self.x & 15) as usize, (self.z & 15) as usize)
    }
}

/// The position of a chunk column: block coordinates divided by 16, rounded down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

impl ChunkPos {
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    /// The chunk column that contains the point with these x and z coordinates.
    pub fn containing(x: f64, z: f64) -> Self {
        Self {
            x: (x.floor() as i32) >> 4,
            z: (z.floor() as i32) >> 4,
        }
    }
}

/// A point in the world, such as where an entity stands.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }
}

/// Identifies an entity to clients. Ids are unique within a world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EntityId(pub i32);

/// Identifies a player: the UUID of their profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PlayerId(pub Uuid);

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

    #[test]
    fn points_map_to_the_chunk_they_are_in() {
        assert_eq!(ChunkPos::containing(0.5, 15.9), ChunkPos::new(0, 0));
        assert_eq!(ChunkPos::containing(16.0, -0.1), ChunkPos::new(1, -1));
        assert_eq!(ChunkPos::containing(-16.0, -16.1), ChunkPos::new(-1, -2));
    }
}
