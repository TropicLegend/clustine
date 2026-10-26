//! The light of a chunk, as the game's rules give it for the blocks of the chunk and of
//! the eight chunks around it (ADR-0022, section 6).
//!
//! [`light`] is a function: no clock, no thread, no hash map, nothing kept between two
//! calls. The same blocks give the same light on every machine. [`light_with`] is the
//! same function for someone who lights many chunks: it is lent a [`Scratch`] to work
//! in, of which it reads nothing an earlier call wrote, and it can say more
//! ([`Unlisted`]). [`reach`] says which neighbours' light a block change can alter.
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

pub mod engine;
mod faces;
mod flood;

use clustine_world::{Chunk, Light, SECTION_SIZE, SectionLight};

pub use faces::light_dampening_into;
pub use flood::Scratch;

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
    light_with(around, Unlisted::NONE, &mut Scratch::new()).0
}

/// As [`light`], working in `scratch`, and for each light section named in `also` that
/// comes out [`SectionLight::Absent`], the case its levels have.
///
/// The light is exactly [`light`]'s, whatever `also` names and whatever `scratch` was
/// used for before. A section named in `also` that the light lists is not said twice,
/// and one that the chunk does not have is passed over.
///
/// Panics as [`light`] does. The scratch may be used again after that.
pub fn light_with(
    around: &Around<'_>,
    also: Unlisted,
    scratch: &mut Scratch,
) -> (Light, UnlistedLevels) {
    flood::light(around, also, scratch)
}

/// Which of the eight neighbours (north-west first, row by row, without the centre: the
/// order of [`Around::neighbours`]) a change of a block in column `x`, `z` of a chunk
/// can alter the light of, by distance alone.
///
/// **The distance is 14 steps sideways.** A level starts at 15 at most and falls by at
/// least one a step, so light from a column arrives 14 columns away with 1 at most and
/// 15 away with nothing; and a block that only takes light, or stops it, alters nothing
/// further away than the light it took would have gone. The steps are counted as the
/// flood counts them when it cuts by distance, along x and along z together, from the
/// column to the nearest column of the neighbour: the neighbour to the west of column
/// `x` is `x + 1` steps away, the one to the east `16 - x`, one in the same row or line
/// of chunks 0, and a diagonal neighbour the sum of its two. This function asks the
/// flood's own table of distances whether the neighbour's flood would start from the
/// brightest source in that column, which it does up to 14 steps and not from 15.
///
/// So each neighbour beside the chunk is reached from 14 of the 16 columns across the
/// chunk towards it, and each diagonal neighbour from the 91 columns of 256 nearest its
/// corner: 4.9 neighbours a column at the mean.
///
/// **What distance does not cover**: a change that puts the first block that is not air
/// into a section, or takes the last out, can alter which light sections are
/// [`SectionLight::Absent`] in all eight neighbours, wherever in the chunk it is. Whoever
/// asks this has to look at that apart.
///
/// Panics if `x` or `z` is not below 16.
pub fn reach(x: usize, z: usize) -> [bool; 8] {
    flood::reach(x, z)
}

/// The light sections that [`light_with`] is to say the levels of even where it leaves
/// them out of the light: one bit a light section for the sky and one for blocks, the
/// lowest bit for the section below the chunk, as [`Light::sky`] and [`Light::block`]
/// count them.
///
/// **What an edge does with it.** A client that was once sent a section of light cannot
/// be told to forget it: a light update can only name sections, with levels or as dark.
/// So an edge names here the sections it has listed for a chunk before
/// ([`Unlisted::listed_in`] the light it holds), and puts what comes back into the new
/// light ([`UnlistedLevels::put_into`]) before it compares the two and sends what
/// differs: a section that was listed once stays listed, with the levels it has now
/// (ADR-0023, section 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Unlisted {
    pub sky: u64,
    pub block: u64,
}

impl Unlisted {
    /// Nothing more than the light itself.
    pub const NONE: Unlisted = Unlisted { sky: 0, block: 0 };

    /// How many light sections can be named: the section below a chunk and the 63
    /// above that. The game's own dimensions have 26 at most.
    pub const SECTIONS: usize = u64::BITS as usize;

    /// The sections that `light` lists: every one that is not
    /// [`SectionLight::Absent`], for the sky and for blocks apart. Sections from
    /// [`Unlisted::SECTIONS`] up cannot be named and are left out.
    pub fn listed_in(light: &Light) -> Unlisted {
        let listed = |sections: &[SectionLight]| {
            sections
                .iter()
                .take(Unlisted::SECTIONS)
                .enumerate()
                .filter(|(_, section)| **section != SectionLight::Absent)
                .fold(0, |mask, (index, _)| mask | 1 << index)
        };
        Unlisted {
            sky: listed(&light.sky),
            block: listed(&light.block),
        }
    }
}

/// What [`light_with`] says beyond the light: for each section that was named in its
/// [`Unlisted`] and that the light leaves out, the number of the section, as
/// [`Light::sky`] and [`Light::block`] count them, and the case of its levels, which is
/// never [`SectionLight::Absent`]. In ascending order of the sections.
///
/// These are the levels the flood has for the section all the same: the sky's light
/// under whatever is above it, any height above, and the blocks' light, which is dark
/// where no block is near.
///
/// In a dimension without a sky there is no sky light to say, and `sky` is empty
/// whatever was asked.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UnlistedLevels {
    pub sky: Vec<(usize, SectionLight)>,
    pub block: Vec<(usize, SectionLight)>,
}

impl UnlistedLevels {
    /// Whether nothing that was asked about came out [`SectionLight::Absent`].
    pub fn is_empty(&self) -> bool {
        self.sky.is_empty() && self.block.is_empty()
    }

    /// Lists these sections in `light`, which is the light they were given with: an
    /// edge's light for a chunk, in which what was listed once stays listed.
    pub fn put_into(self, light: &mut Light) {
        for (index, section) in self.sky {
            light.sky[index] = section;
        }
        for (index, section) in self.block {
            light.block[index] = section;
        }
    }
}
