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

    /// The middle of the bottom of the block at `pos`, where the game stands a player
    /// it puts on a block (`Vec3.atBottomCenterOf`).
    pub fn bottom_centre_of(pos: BlockPos) -> Self {
        Self::new(
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        )
    }
}

/// An entity's key: unique for the life of a world and never given twice
/// (`docs/adr/0028-entities.md`, section 1). Clients know an entity by a wire id of
/// their edge's instead, and the game's own `getId()` is [`EntityId::game_id`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EntityId(pub u64);

impl EntityId {
    /// What the game's `Entity.getId()` would be, which its rules read: an item at rest
    /// moves on the ticks where `(tickCount + getId()) % 4 == 0`, and a mob weighs its
    /// goals afresh by `tickCount + getId()`. The key's low half, which counts within
    /// its block.
    pub const fn game_id(self) -> i32 {
        self.0 as u32 as i32
    }

    /// Whether generation made it, with its chunk (`docs/groundwork/n7l-plan.md`,
    /// section 2.3): only those keys have bit 63, which no region's block reaches.
    pub const fn is_generated(self) -> bool {
        self.0 & GENERATED_BIT != 0
    }
}

/// Bit 63, set in every key generation makes and in no key of a region's block.
const GENERATED_BIT: u64 = 1 << 63;

/// The 63 bits a generated key's parts are packed into and mixed within.
const LOW_63: u64 = GENERATED_BIT - 1;

/// What a generated entity's key names (`docs/groundwork/n7l-plan.md`, sections 2.3 and
/// 9 item 1): the dimension (one of four), the chunk it lands in, whether a structure's
/// run made it or the chunk's spawn, and its place among those of its class that chunk
/// has, counting from 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generated {
    pub dimension: u8,
    pub chunk: ChunkPos,
    pub structure: bool,
    pub index: u16,
}

impl Generated {
    /// The farthest chunk from the origin along x or z within the game's world border
    /// (29,999,984 blocks): no entity is made beyond it.
    pub const BORDER: i32 = 1_874_999;

    /// Its key: the parts packed into 63 bits (the dimension 2, x and z 22 each, signed,
    /// the class 1, the index 16), put through a bijection of 63 bits so that the low
    /// half, the game's `getId()`, is scattered, and bit 63 set. `None` for a chunk
    /// beyond the border or a dimension beyond the fourth.
    pub const fn key(self) -> Option<EntityId> {
        if self.dimension > 3
            || self.chunk.x < -Self::BORDER
            || self.chunk.x > Self::BORDER
            || self.chunk.z < -Self::BORDER
            || self.chunk.z > Self::BORDER
        {
            return None;
        }
        let offset = 1 << 21;
        let packed = (self.dimension as u64) << 61
            | ((self.chunk.x + offset) as u64) << 39
            | ((self.chunk.z + offset) as u64) << 17
            | (self.structure as u64) << 16
            | self.index as u64;
        Some(EntityId(GENERATED_BIT | mix(packed)))
    }

    /// What the generated key `key` names, `None` for a key generation did not make.
    pub const fn of(key: EntityId) -> Option<Self> {
        if !key.is_generated() {
            return None;
        }
        let packed = unmix(key.0 & LOW_63);
        Some(Self {
            dimension: (packed >> 61) as u8,
            chunk: ChunkPos::new(coordinate(packed, 39), coordinate(packed, 17)),
            structure: (packed >> 16) & 1 == 1,
            index: packed as u16,
        })
    }
}

/// The signed coordinate of 22 bits at `shift` of a packed key.
const fn coordinate(packed: u64, shift: u32) -> i32 {
    ((packed >> shift) & ((1 << 22) - 1)) as i32 - (1 << 21)
}

/// The odd multipliers of [`mix`]: any odd number is a bijection of the 63 bits under
/// multiplication modulo 2^63.
const MIX_A: u64 = 0x2545_f491_4f6c_dd1d;
const MIX_B: u64 = 0x1bd1_1bda_a9fc_1a23;

/// A bijection of 63 bits: xor-shifts and multiplications by odd numbers modulo 2^63,
/// each of which has an inverse ([`unmix`]).
const fn mix(value: u64) -> u64 {
    let mut v = value & LOW_63;
    v ^= v >> 31;
    v = v.wrapping_mul(MIX_A) & LOW_63;
    v ^= v >> 29;
    v = v.wrapping_mul(MIX_B) & LOW_63;
    v ^= v >> 32;
    v
}

/// The inverse of [`mix`].
const fn unmix(value: u64) -> u64 {
    let mut v = value & LOW_63;
    v = unshift(v, 32);
    v = v.wrapping_mul(inverse(MIX_B)) & LOW_63;
    v = unshift(v, 29);
    v = v.wrapping_mul(inverse(MIX_A)) & LOW_63;
    unshift(v, 31)
}

/// The inverse of `v ^= v >> shift` within 63 bits.
const fn unshift(value: u64, shift: u32) -> u64 {
    let mut v = value;
    let mut done = shift;
    while done < 63 {
        v = value ^ (v >> shift);
        done += shift;
    }
    v & LOW_63
}

/// The inverse of the odd `odd` modulo 2^64 (and so 2^63), by Newton's iteration: each
/// step doubles the bits that are right.
const fn inverse(odd: u64) -> u64 {
    let mut inverse = odd;
    let mut step = 0;
    while step < 6 {
        inverse = inverse.wrapping_mul(2u64.wrapping_sub(odd.wrapping_mul(inverse)));
        step += 1;
    }
    inverse
}

/// Identifies a player: the UUID of their profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PlayerId(pub Uuid);

/// Identifies a region. The world store gives a region its id, which is never used
/// again (`docs/adr/0010-regions-that-follow-players.md`); regions that it pins side
/// by side are numbered from west to east.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RegionId(pub u32);

impl std::fmt::Display for RegionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

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

/// A block of entity keys: `first` up to, but not including, `end`.
///
/// Keys must be unique within a world, so everything that hands them out gets a block of
/// its own. The store gives a region a fresh block at every hello, so that keys a lost
/// tick gave out are never given again (`docs/adr/0028-entities.md`, section 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityIds {
    pub first: EntityId,
    pub end: EntityId,
}

impl EntityIds {
    /// The number of keys in a block made by [`EntityIds::block`].
    pub const BLOCK_SIZE: u64 = 1 << 32;

    /// The number of blocks [`EntityIds::block`] can make: 2^31, so that no key of a
    /// region's block has bit 63, which generated keys have
    /// (`docs/groundwork/n7l-plan.md`, section 2.3).
    pub const BLOCK_COUNT: u32 = 1 << 31;

    /// The block with the given index, or `None` if `index` is not below
    /// [`EntityIds::BLOCK_COUNT`]. A key's high half is its block's index, so blocks with
    /// different indices share no key, and no block contains key 0, which stands for
    /// none.
    pub const fn block(index: u32) -> Option<Self> {
        if index >= Self::BLOCK_COUNT {
            return None;
        }
        let start = index as u64 * Self::BLOCK_SIZE;
        Some(Self {
            first: EntityId(if start == 0 { 1 } else { start }),
            end: EntityId(start + Self::BLOCK_SIZE),
        })
    }

    /// The index of the block this is, as [`EntityIds::block`] takes it.
    pub const fn index(self) -> u32 {
        (self.first.0 / Self::BLOCK_SIZE) as u32
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

    fn generated(x: i32, z: i32, structure: bool, index: u16) -> Generated {
        Generated {
            dimension: 0,
            chunk: ChunkPos::new(x, z),
            structure,
            index,
        }
    }

    /// The mixer is a bijection of 63 bits: its inverse gives back every value tried,
    /// the extremes among them.
    #[test]
    fn the_mixer_is_undone_by_its_inverse() {
        let mut value = 0x0123_4567_89ab_cdefu64;
        for _ in 0..100_000 {
            value = value
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let v = value & LOW_63;
            assert_eq!(unmix(mix(v)), v);
        }
        for v in [0, 1, LOW_63, LOW_63 - 1, 1 << 62] {
            assert_eq!(unmix(mix(v)), v);
        }
    }

    /// Every generated key has bit 63 and reads back as what it names; no block of a
    /// region's keys reaches bit 63.
    #[test]
    fn a_generated_key_has_bit_63_and_names_its_chunk_class_and_index() {
        let border = Generated::BORDER;
        for x in [-border, -1, 0, 1, 7, border] {
            for z in [-border, -3, 0, 5, border] {
                for structure in [false, true] {
                    for index in [0, 1, 65_535] {
                        for dimension in 0..4 {
                            let made = Generated {
                                dimension,
                                ..generated(x, z, structure, index)
                            };
                            let key = made.key().expect("within the border");
                            assert!(key.is_generated());
                            assert_eq!(Generated::of(key), Some(made));
                        }
                    }
                }
            }
        }
        let last = EntityIds::block(EntityIds::BLOCK_COUNT - 1).expect("the last block");
        assert!(!EntityId(last.end.0 - 1).is_generated());
        assert_eq!(EntityIds::block(EntityIds::BLOCK_COUNT), None);
        assert_eq!(Generated::of(last.first), None);
    }

    /// Beyond the world border, or of a fifth dimension, no key is made.
    #[test]
    fn no_key_is_made_beyond_the_border() {
        let border = Generated::BORDER;
        assert_eq!(generated(border + 1, 0, false, 0).key(), None);
        assert_eq!(generated(0, -border - 1, false, 0).key(), None);
        let fifth = Generated {
            dimension: 4,
            ..generated(0, 0, false, 0)
        };
        assert_eq!(fifth.key(), None);
    }

    /// Keys are distinct over many chunks, both classes and many indices, and a chunk's
    /// first 64 have 64 different game ids.
    #[test]
    fn generated_keys_are_distinct_and_their_game_ids_spread() {
        let mut seen = std::collections::BTreeSet::new();
        for x in -12..12 {
            for z in -12..12 {
                for structure in [false, true] {
                    for index in 0..64 {
                        let key = generated(x * 977, z * 131, structure, index)
                            .key()
                            .expect("within the border");
                        assert!(seen.insert(key), "{key:?} made twice");
                    }
                }
            }
        }
        let ids: std::collections::BTreeSet<i32> = (0..64)
            .map(|index| {
                generated(3, -8, false, index)
                    .key()
                    .expect("within the border")
                    .game_id()
            })
            .collect();
        assert_eq!(ids.len(), 64);
    }

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
