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
//! - A dimension without a sky has no sky light: every entry of [`Light::sky`] is
//!   [`SectionLight::Absent`], so that a packet made from it lists no sky light at all.
//!   That is the game's code as remembered (it has no engine for the sky there); no
//!   packet and no file of the official server's nether was compared with it.
//!
//! A level falls by at least one a step, so light that reaches the centre chunk started
//! at most 14 blocks from it: the nine chunks are all it depends on.
//!
//! Which case a light section comes out as is the official server's rule for its
//! packet: a section has light at all if it, or one of the 26 sections around it in the
//! nine chunks, holds a block that is not air; otherwise it is [`SectionLight::Absent`].
//! A section that has light is in the one form equal light has ([`SectionLight::of`]):
//! dark, full, or its levels. The official server's files cannot tell a dark section
//! from an absent one, so that half of the rule is from the game's code as remembered
//! and from the flat world's packet, not from the comparison below.
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

#[cfg(doc)]
use clustine_world::SectionLight;
use clustine_world::{Chunk, Light, SECTION_SIZE};

/// The version of the rules in this crate. Raised whenever any chunk's light would come
/// out otherwise. Light says which version made it ([`Light::made_by`]).
pub const VERSION: u16 = 1;

/// Blocks along the edge of a section.
const SECTION_EDGE: usize = SECTION_SIZE;

/// The chunk whose light is wanted and the chunks around it.
///
/// North is towards negative `z` and west towards negative `x`, as in the game: the
/// chunk to the north-west of the chunk at `(x, z)` is the one at `(x - 1, z - 1)`.
#[derive(Debug, Clone, Copy)]
pub struct Around<'a> {
    pub centre: &'a Chunk,
    /// North-west, north, north-east, west, east, south-west, south, south-east: row by
    /// row from the north, each row from the west. One that is not there gives no light
    /// and takes none.
    pub neighbours: [Option<&'a Chunk>; 8],
    /// Whether the dimension has a sky. Without one there is no sky light at all.
    pub sky: bool,
}

/// The light of the centre chunk, as the game's rules give it for these blocks: made by
/// [`VERSION`], with an entry for the section below the chunk, one for each section and
/// one for the section above, for the sky and for blocks, each in its one form.
///
/// Nothing but the nine chunks is read, and of them only their blocks.
///
/// Panics if a neighbour does not span the heights the centre spans; the chunks of one
/// dimension all do.
pub fn light(around: &Around<'_>) -> Light {
    flood::light(around)
}
