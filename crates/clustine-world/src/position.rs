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

/// Identifies an edge across its restarts. It follows from the edge's name, which stays
/// the same when the edge starts again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EdgeId(pub u64);

impl EdgeId {
    /// The id of the edge called `name`. It is a 64-bit FNV-1a hash of the name, which,
    /// unlike the hashers of the standard library, is the same in every build and on
    /// every machine.
    pub const fn from_name(name: &str) -> Self {
        const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;
        let bytes = name.as_bytes();
        let mut hash = OFFSET_BASIS;
        let mut index = 0;
        while index < bytes.len() {
            hash ^= bytes[index] as u64;
            hash = hash.wrapping_mul(PRIME);
            index += 1;
        }
        Self(hash)
    }
}

/// A block of entity ids: `first` up to, but not including, `end`.
///
/// Ids must be unique within a world, so everything that hands them out gets a block of
/// its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityIds {
    pub first: EntityId,
    pub end: EntityId,
}

impl EntityIds {
    /// The number of ids in a block made by [`EntityIds::block`].
    pub const BLOCK_SIZE: i32 = 1 << 20;

    /// The number of blocks [`EntityIds::block`] can make.
    pub const BLOCK_COUNT: u32 = (i32::MAX / Self::BLOCK_SIZE) as u32;

    /// The block with the given index, or `None` if `index` is not below
    /// [`EntityIds::BLOCK_COUNT`]. Blocks with different indices share no id, and no
    /// block contains id 0, which clients reject.
    pub const fn block(index: u32) -> Option<Self> {
        if index >= Self::BLOCK_COUNT {
            return None;
        }
        let start = index as i32 * Self::BLOCK_SIZE;
        Some(Self {
            first: EntityId(if start == 0 { 1 } else { start }),
            end: EntityId(start + Self::BLOCK_SIZE),
        })
    }

    pub const fn contains(self, id: EntityId) -> bool {
        self.first.0 <= id.0 && id.0 < self.end.0
    }
}

/// A part of the world that reaches from one chunk x coordinate to another and has no
/// limit along z.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkArea {
    /// The lowest chunk x coordinate in the area, or `None` if it has no western end.
    pub min_x: Option<i32>,
    /// The chunk x coordinate just beyond the area, or `None` if it has no eastern end.
    pub max_x: Option<i32>,
}

impl ChunkArea {
    /// The whole world.
    pub const EVERYWHERE: Self = Self {
        min_x: None,
        max_x: None,
    };

    pub fn contains(self, chunk: ChunkPos) -> bool {
        self.min_x.is_none_or(|min| min <= chunk.x) && self.max_x.is_none_or(|max| chunk.x < max)
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

    #[test]
    fn entity_id_blocks_do_not_overlap_and_skip_zero() {
        let first = EntityIds::block(0).unwrap();
        let second = EntityIds::block(1).unwrap();
        assert_eq!(first.first, EntityId(1));
        assert_eq!(first.end, second.first);
        assert!(first.contains(EntityId(1)));
        assert!(!first.contains(EntityId(0)));
        assert!(!first.contains(second.first));
        assert!(second.contains(second.first));

        let last = EntityIds::block(EntityIds::BLOCK_COUNT - 1).unwrap();
        assert!(last.end.0 > last.first.0);
        assert_eq!(EntityIds::block(EntityIds::BLOCK_COUNT), None);
    }

    #[test]
    fn an_edge_id_is_the_fnv_1a_hash_of_the_name() {
        // The published test vectors of 64-bit FNV-1a.
        assert_eq!(EdgeId::from_name(""), EdgeId(0xcbf2_9ce4_8422_2325));
        assert_eq!(EdgeId::from_name("a"), EdgeId(0xaf63_dc4c_8601_ec8c));
        assert_eq!(EdgeId::from_name("foobar"), EdgeId(0x8594_4171_f739_67e8));
        assert_ne!(EdgeId::from_name("edge-0"), EdgeId::from_name("edge-1"));
    }

    #[test]
    fn areas_include_their_western_end_only() {
        let area = ChunkArea {
            min_x: Some(-2),
            max_x: Some(3),
        };
        assert!(!area.contains(ChunkPos::new(-3, 0)));
        assert!(area.contains(ChunkPos::new(-2, 1000)));
        assert!(area.contains(ChunkPos::new(2, -1000)));
        assert!(!area.contains(ChunkPos::new(3, 0)));

        let west = ChunkArea {
            min_x: None,
            max_x: Some(0),
        };
        assert!(west.contains(ChunkPos::new(i32::MIN, 0)));
        assert!(!west.contains(ChunkPos::new(0, 0)));
        assert!(ChunkArea::EVERYWHERE.contains(ChunkPos::new(i32::MAX, i32::MIN)));
    }

    #[test]
    fn points_map_to_the_chunk_they_are_in() {
        assert_eq!(ChunkPos::containing(0.5, 15.9), ChunkPos::new(0, 0));
        assert_eq!(ChunkPos::containing(16.0, -0.1), ChunkPos::new(1, -1));
        assert_eq!(ChunkPos::containing(-16.0, -16.1), ChunkPos::new(-1, -2));
    }
}
