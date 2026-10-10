//! The light of a chunk, as the game's rules give it for the blocks of the chunk and of
//! the eight chunks around it (ADR-0022, section 6).
//!
//! [`light`] is a function: no clock, no thread, no hash map, nothing kept between two
//! calls. The same blocks give the same light on every machine.
//!
//! The rules, which are the game's as SteelMC's 26.3 branch has them (see `NOTICE`):
//!
//! - A block takes from light that enters it its dampening, and at least 1. A block
//!   that dampens by 15 is never entered.
//! - Light does not pass from one block to the next where the two faces that meet
//!   there stop it: one of them is whole, or both together cover the face. Only blocks
//!   that occlude by their shape have such faces (slabs, stairs and the like).
//! - **Block light** starts at every block that gives light, with the level it gives,
//!   whatever the block itself would take.
//! - **Sky light** is 15 in a column from above the world down to the first block that
//!   dampens at all or whose upper face, with the lower face of the block above, stops
//!   light. From there it spreads as block light does, downwards too.
//! - A chunk that is not there is solid: it gives no light and takes none. Above and
//!   below the world is air.
//!
//! A level falls by at least one a step, so light that reaches the centre chunk started
//! at most 14 blocks from it: the nine chunks are all it depends on.
//!
//! Which case a light section comes out as is the official server's rule for its
//! packet: a section has light at all if it, or one of the 26 sections around it in the
//! nine chunks, holds a block that is not air; otherwise it is [`SectionLight::Absent`].
//! The official server's files cannot tell a dark section from an absent one, so that
//! half of the rule is from the game's code as remembered and from the flat world's
//! packet, not from the comparison below.
//!
//! **Where this is not the game.** The game spreads light only into sections that have
//! light of their own, and reads a section without any from the one above it. Here
//! light spreads through all of the nine chunks alike, and a section without light of
//! its own is left out of the result afterwards. The two could differ only where light
//! reaches a listed section by a way through a section that neither holds a block nor
//! has a neighbour that does. Block light has no such way, since it starts at a block
//! and goes 14 steps at most. Sky light has one far below something that floats, where
//! the game has rules of its own for sections without light; no chunk of the sample
//! has such a place, so the comparison does not say whether the two agree there.
//!
//! `tests/official.rs` compares the result with the light the official server stored
//! for chunks it made, section by section.

mod faces;
mod flood;

use clustine_data::BlockState;

/// The version of the rules in this crate. Raised whenever any chunk's light would come
/// out otherwise. Light says which version made it.
pub const VERSION: u16 = 1;

/// Bytes in the light array of one section: four bits a block.
pub const LIGHT_ARRAY_LENGTH: usize = 2048;

/// Blocks along the edge of a section.
const SECTION_EDGE: usize = 16;

/// The blocks light is made from: the chunk whose light is wanted and the eight around
/// it, and what of the dimension light needs.
///
/// Chunks are named by their offset from the centre chunk, each of -1, 0 and 1; blocks
/// by `x` and `z` from the centre chunk's north-west corner, so that the nine chunks
/// span -16 to 31, and by their `y` in the world.
pub trait Blocks {
    /// The `y` of the lowest block of the world.
    fn min_y(&self) -> i32;

    /// The height of the world in blocks: a multiple of 16, and not 0.
    fn height(&self) -> u32;

    /// Whether the dimension has a sky. Without one there is no sky light at all.
    fn has_sky(&self) -> bool;

    /// Whether the chunk at this offset from the centre is there. One that is not gives
    /// no light and takes none. The centre chunk is always there.
    fn has_chunk(&self, chunk_x: i32, chunk_z: i32) -> bool;

    /// The block at a position within the world's height, in a chunk that is there.
    fn state(&self, x: i32, y: i32, z: i32) -> BlockState;

    /// The one state of a whole section (counted from the lowest) of a chunk that is
    /// there, if the section is known to hold one state only. It saves 4,096 calls of
    /// [`Self::state`]; answering `None` is always right.
    fn uniform_section(&self, chunk_x: i32, chunk_z: i32, section: usize) -> Option<BlockState> {
        let _ = (chunk_x, chunk_z, section);
        None
    }
}

/// The light of one section, sky or block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionLight {
    /// The section has no light of its own: the packet lists it in neither mask.
    Absent,
    /// Level 0 everywhere.
    Dark,
    /// Level 15 everywhere.
    Full,
    /// Levels per block, four bits each, indexed by `y << 8 | z << 4 | x`; the block
    /// with the even index is in the low half of its byte. Never all 0 and never all
    /// 15: those are [`Self::Dark`] and [`Self::Full`], so that equal light is equal.
    Levels(Box<[u8; LIGHT_ARRAY_LENGTH]>),
}

impl SectionLight {
    /// The case for an array of levels: [`Self::Dark`] if all are 0, [`Self::Full`] if
    /// all are 15, the array otherwise.
    pub fn from_array(levels: Box<[u8; LIGHT_ARRAY_LENGTH]>) -> SectionLight {
        if levels.iter().all(|byte| *byte == 0) {
            SectionLight::Dark
        } else if levels.iter().all(|byte| *byte == 0xff) {
            SectionLight::Full
        } else {
            SectionLight::Levels(levels)
        }
    }

    /// The level at a position within the section, each coordinate below 16. `None` for
    /// a section that has no light of its own.
    pub fn level(&self, x: usize, y: usize, z: usize) -> Option<u8> {
        assert!(
            x < SECTION_EDGE && y < SECTION_EDGE && z < SECTION_EDGE,
            "a position within a section"
        );
        match self {
            SectionLight::Absent => None,
            SectionLight::Dark => Some(0),
            SectionLight::Full => Some(15),
            SectionLight::Levels(levels) => {
                let index = y << 8 | z << 4 | x;
                Some(levels[index / 2] >> (index % 2 * 4) & 15)
            }
        }
    }

    /// The 2,048 bytes the client is sent for this section, if it is sent any.
    pub fn to_array(&self) -> Option<Box<[u8; LIGHT_ARRAY_LENGTH]>> {
        match self {
            SectionLight::Absent => None,
            SectionLight::Dark => Some(Box::new([0; LIGHT_ARRAY_LENGTH])),
            SectionLight::Full => Some(Box::new([0xff; LIGHT_ARRAY_LENGTH])),
            SectionLight::Levels(levels) => Some(levels.clone()),
        }
    }
}

/// The light of a chunk: for sky and for block light one entry for the section below
/// the world, one for each section and one for the section above, bottom first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkLight {
    /// All [`SectionLight::Absent`] in a dimension without a sky.
    pub sky: Vec<SectionLight>,
    pub block: Vec<SectionLight>,
}

/// The light of the centre chunk of `blocks`, as the game's rules give it.
///
/// Panics if the height is 0 or not a multiple of 16; a dimension has neither.
pub fn light<B: Blocks + ?Sized>(blocks: &B) -> ChunkLight {
    flood::light(blocks)
}
