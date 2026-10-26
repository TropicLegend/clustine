//! Light kept up to date as blocks change and chunks come and go, for a region that
//! reads it in its tick (`docs/groundwork/n8-plan.md`, section 3, and ADR-0029).
//!
//! **The invariant** (the plan's 3.1): after [`Lighting::run`], every present chunk holds
//! exactly what [`crate::light`] gives for it and its eight neighbours as they are present,
//! an absent neighbour counting as a wall, with the levels of every section kept, listed
//! or not. Light is a function of the blocks, so how a run gets there is invisible.
//!
//! **This implementation** (the plan's 3.9, "E1") gets there the plain way: a run lights
//! whole, with [`crate::light_with`], every present chunk a noted change can alter and
//! every chunk beside one that came, was replaced or went. It is exact by construction and
//! slow; the incremental engine replaces its bodies behind the same interface, and this
//! code stays as the oracle its tests are judged by.
//!
//! No clock, no thread, no hash map: what a run does is a function of what it was told
//! and of the chunks it reads.

use std::collections::{BTreeMap, BTreeSet};

use clustine_world::{BlockPos, Chunk, ChunkPos, Light};

use crate::{Around, Scratch, Unlisted};

/// What the engine reads of the world: a present chunk, or `None` for a wall.
pub trait Chunks {
    fn chunk(&self, at: ChunkPos) -> Option<&Chunk>;
}

/// The two levels at a block, as stored: the sky's undarkened, and the blocks'.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightAt {
    pub sky: u8,
    pub block: u8,
}

/// What a run did, for the figures (the plan's section 8). Counted, never timed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Work {
    pub chunks_lit_whole: u32,
    pub adopted: u32,
    pub cells: u64,
}

/// A chunk's levels as a run left them, handed out at a merge or a split and adopted
/// where the chunk comes back with the same blocks (the plan's 4.5). `walls` names the
/// eight neighbours (in [`crate::Around::neighbours`]' order, a bit each) that were
/// absent at that run; `taken` names the hand-over (the region and its tick), so that two
/// chunks kept from it know their levels agree.
#[derive(Clone, PartialEq, Eq)]
pub struct Kept {
    /// The levels of every section, the sky's and the blocks', as [`Light`] keeps them.
    pub light: Light,
    pub walls: u8,
    pub taken: (u64, u64),
}

impl std::fmt::Debug for Kept {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Kept")
            .field("sections", &self.light.sky.len())
            .field("walls", &self.walls)
            .field("taken", &self.taken)
            .finish()
    }
}

/// The light of a region's present chunks, and what waits for the next run.
pub struct Lighting {
    sky: bool,
    /// The chunks present, each with its levels as of the last run (every section
    /// listed) and its lowest block's height, or `None` where it has not been lit since
    /// it came.
    present: BTreeMap<ChunkPos, Option<(Light, i32)>>,
    /// The present chunks the next run lights whole.
    pending: BTreeSet<ChunkPos>,
    scratch: Scratch,
}

impl std::fmt::Debug for Lighting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lighting")
            .field("sky", &self.sky)
            .field("present", &self.present.len())
            .field("pending", &self.pending.len())
            .finish()
    }
}

/// The eight chunks around `at`, in [`Around::neighbours`]' order.
fn around(at: ChunkPos) -> [ChunkPos; 8] {
    [
        ChunkPos::new(at.x - 1, at.z - 1),
        ChunkPos::new(at.x, at.z - 1),
        ChunkPos::new(at.x + 1, at.z - 1),
        ChunkPos::new(at.x - 1, at.z),
        ChunkPos::new(at.x + 1, at.z),
        ChunkPos::new(at.x - 1, at.z + 1),
        ChunkPos::new(at.x, at.z + 1),
        ChunkPos::new(at.x + 1, at.z + 1),
    ]
}

/// Every light section, for the sky and for blocks: what a run asks the levels of, so
/// that a section left out of a chunk's light still has its levels kept.
const EVERY_SECTION: Unlisted = Unlisted {
    sky: u64::MAX,
    block: u64::MAX,
};

impl Lighting {
    /// No chunk present, in a dimension with a sky or without one.
    pub fn new(sky: bool) -> Self {
        Self {
            sky,
            present: BTreeMap::new(),
            pending: BTreeSet::new(),
            scratch: Scratch::new(),
        }
    }

    /// Notes that the chunk at `at` and those about it are to be lit whole.
    fn about(&mut self, at: ChunkPos) {
        for chunk in std::iter::once(at).chain(around(at)) {
            if self.present.contains_key(&chunk) {
                self.pending.insert(chunk);
            }
        }
    }

    /// A chunk became present. Where it is present already, its blocks were replaced
    /// whole (a copy's new base, a loaded chunk become a copy or the reverse): the run
    /// takes it as a departure and then an arrival, since the old blocks may have lit
    /// its neighbours.
    pub fn arrived(&mut self, at: ChunkPos) {
        self.present.insert(at, None);
        self.about(at);
    }

    /// A chunk became present with levels kept from a run elsewhere, its blocks as they
    /// were then. This implementation lights it whole all the same, which is exact.
    pub fn adopt(&mut self, at: ChunkPos, kept: Kept) {
        let _ = kept;
        self.arrived(at);
    }

    /// A chunk stopped being present.
    pub fn left(&mut self, at: ChunkPos) {
        if self.present.remove(&at).is_some() {
            self.pending.remove(&at);
            self.about(at);
        }
    }

    /// A block changed in a way [`clustine_world::light_tells_apart`] sees. Passed over
    /// where its chunk is not present.
    pub fn changed(&mut self, at: BlockPos) {
        let chunk = at.chunk();
        if !self.present.contains_key(&chunk) {
            return;
        }
        // Every neighbour, not only those `reach` names: a change that fills or empties
        // a section can change which of their sections are listed (`reach`'s note).
        self.about(chunk);
    }

    /// Brings every present chunk to the invariant, reading `chunks` as they are. A
    /// present chunk that `chunks` does not give is left unlit until it does.
    pub fn run(&mut self, chunks: &dyn Chunks) -> Work {
        let mut work = Work::default();
        let pending = std::mem::take(&mut self.pending);
        for at in pending {
            let Some(centre) = chunks.chunk(at) else {
                continue;
            };
            let neighbours = around(at).map(|near| {
                self.present
                    .contains_key(&near)
                    .then(|| chunks.chunk(near))
                    .flatten()
            });
            let lit = crate::light_with(
                &Around {
                    centre,
                    neighbours,
                    sky: self.sky,
                },
                EVERY_SECTION,
                &mut self.scratch,
            );
            let (mut light, unlisted) = lit;
            unlisted.put_into(&mut light);
            work.chunks_lit_whole += 1;
            work.cells += (centre.sections().len() * clustine_world::SECTION_SIZE.pow(3)) as u64;
            self.present.insert(at, Some((light, centre.min_y())));
        }
        work
    }

    /// The levels at a block as of the last run; `None` where no present chunk is, or
    /// the chunk has not been lit since it came. Above and below the sections it keeps:
    /// sky 15, blocks 0.
    pub fn at(&self, at: BlockPos) -> Option<LightAt> {
        let (light, min_y) = self.present.get(&at.chunk())?.as_ref()?;
        let (x, z) = ((at.x & 15) as usize, (at.z & 15) as usize);
        Some(LightAt {
            sky: if self.sky {
                light.sky_at(*min_y, x, at.y, z)
            } else {
                0
            },
            block: light.block_at(*min_y, x, at.y, z),
        })
    }

    /// The chunk's light in the crate's form, sections listed by its rule, for the tests
    /// and for whoever later wants it whole.
    pub fn light(&self, at: ChunkPos, chunks: &dyn Chunks) -> Option<Light> {
        let centre = chunks.chunk(at)?;
        self.present.get(&at)?;
        let neighbours = around(at).map(|near| {
            self.present
                .contains_key(&near)
                .then(|| chunks.chunk(near))
                .flatten()
        });
        Some(crate::light(&Around {
            centre,
            neighbours,
            sky: self.sky,
        }))
    }

    /// The chunk's levels to hand out with it, after a run, and which of its neighbours
    /// were absent at that run.
    pub fn kept(&self, at: ChunkPos, taken: (u64, u64)) -> Option<Kept> {
        let (light, _) = self.present.get(&at)?.as_ref()?;
        let walls = around(at)
            .iter()
            .enumerate()
            .filter(|(_, near)| !self.present.contains_key(near))
            .fold(0u8, |walls, (bit, _)| walls | 1 << bit);
        Some(Kept {
            light: light.clone(),
            walls,
            taken,
        })
    }

    /// Whether the chunk is present.
    pub fn has(&self, at: ChunkPos) -> bool {
        self.present.contains_key(&at)
    }

    /// Whether the chunk is present and lit as of the last run, nothing noted since
    /// that would change it.
    pub fn is_lit(&self, at: ChunkPos) -> bool {
        matches!(self.present.get(&at), Some(Some(_))) && !self.pending.contains(&at)
    }

    /// Whether anything waits for a run.
    pub fn is_settled(&self) -> bool {
        self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clustine_world::{Biome, Chunk, Section};

    /// Chunks held in a map, for the tests.
    struct Held(BTreeMap<ChunkPos, Chunk>);

    impl Chunks for Held {
        fn chunk(&self, at: ChunkPos) -> Option<&Chunk> {
            self.0.get(&at)
        }
    }

    fn stone_floor() -> Chunk {
        let sections = (0..24)
            .map(|_| Section::filled(clustine_data::blocks::AIR, Biome(0)))
            .collect();
        let mut chunk = Chunk::from_sections(-64, sections);
        for x in 0..16 {
            for z in 0..16 {
                chunk.set(x, -61, z, clustine_data::blocks::STONE);
            }
        }
        chunk
    }

    fn lit_alone(held: &Held, at: ChunkPos, sky: bool) -> Light {
        let neighbours = around(at).map(|near| held.0.get(&near));
        crate::light(&Around {
            centre: &held.0[&at],
            neighbours,
            sky,
        })
    }

    #[test]
    fn a_run_lights_every_present_chunk_as_the_function_does_with_its_present_neighbours() {
        let mut held = Held(BTreeMap::new());
        let mut lighting = Lighting::new(true);
        for x in -1..=1 {
            for z in -1..=1 {
                let at = ChunkPos::new(x, z);
                held.0.insert(at, stone_floor());
                lighting.arrived(at);
            }
        }
        let work = lighting.run(&held);
        assert_eq!(work.chunks_lit_whole, 9);
        let centre = ChunkPos::new(0, 0);
        assert_eq!(
            lighting.light(centre, &held),
            Some(lit_alone(&held, centre, true))
        );
        // Under the floor, the sky is dark; above it, full.
        assert_eq!(
            lighting.at(BlockPos::new(3, -62, 3)),
            Some(LightAt { sky: 0, block: 0 })
        );
        assert_eq!(
            lighting.at(BlockPos::new(3, -60, 3)),
            Some(LightAt { sky: 15, block: 0 })
        );
    }

    #[test]
    fn a_change_relights_its_chunk_and_those_about_it_and_a_departure_its_neighbours() {
        let mut held = Held(BTreeMap::new());
        let mut lighting = Lighting::new(true);
        for x in -1..=1 {
            for z in -1..=1 {
                let at = ChunkPos::new(x, z);
                held.0.insert(at, stone_floor());
                lighting.arrived(at);
            }
        }
        lighting.run(&held);
        // A glowstone on the floor of the centre.
        held.0.get_mut(&ChunkPos::new(0, 0)).unwrap().set(
            15,
            -60,
            8,
            clustine_data::blocks::GLOWSTONE,
        );
        lighting.changed(BlockPos::new(15, -60, 8));
        assert!(!lighting.is_lit(ChunkPos::new(1, 0)));
        let work = lighting.run(&held);
        assert_eq!(work.chunks_lit_whole, 9);
        assert_eq!(
            lighting.at(BlockPos::new(16, -60, 8)).map(|at| at.block),
            Some(14)
        );
        // The east chunk goes: the centre is lit as beside a wall there.
        held.0.remove(&ChunkPos::new(1, 0));
        lighting.left(ChunkPos::new(1, 0));
        lighting.run(&held);
        assert_eq!(lighting.at(BlockPos::new(16, -60, 8)), None);
        let centre = ChunkPos::new(0, 0);
        let alone = lit_alone(&held, centre, true);
        assert_eq!(lighting.light(centre, &held), Some(alone));
    }

    #[test]
    fn nothing_is_lit_before_a_run_and_a_chunk_not_present_is_passed_over() {
        let mut lighting = Lighting::new(true);
        lighting.changed(BlockPos::new(0, 0, 0));
        assert!(lighting.is_settled());
        lighting.arrived(ChunkPos::new(0, 0));
        assert_eq!(lighting.at(BlockPos::new(0, 0, 0)), None);
        assert!(!lighting.is_lit(ChunkPos::new(0, 0)));
    }
}
