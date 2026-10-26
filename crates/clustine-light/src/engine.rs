//! Light kept up to date as blocks change and chunks come and go, for a region that
//! reads it in its tick (`docs/groundwork/n8-plan.md`, section 3, and ADR-0029).
//!
//! **The invariant** (the plan's 3.1): after [`Lighting::run`], every present chunk holds
//! exactly what [`crate::light`] gives for it and its eight neighbours as they are present,
//! an absent neighbour counting as a wall, with the levels of every section kept, listed
//! or not. Light is a function of the blocks, so how a run gets there is invisible.
//!
//! **How a run gets there** (the plan's 3.4). [`Lighting`] keeps the levels of every
//! present chunk and changes them by the game's own propagation: a queue of decreases
//! worked off first and then a queue of increases, both first in, first out, with the
//! game's entries (a level, the directions to go, whether it is a block's own light).
//! The order is the plan's: what went (departures, and the old blocks of chunks replaced
//! whole) and the noted changes of chunks already lit, in one batch; then the chunks
//! adopted with kept levels, in one batch, their faces mended; then the chunks that
//! arrived, one at a time in ascending order, each to its fixed point before the next.
//! The values are the one fixed point whatever the order, since a level is the greatest
//! that any way from a source gives and the queues find it from any start that is no
//! brighter than it and has every place where it may be brighter queued (`pass.rs`).
//!
//! [`Oracle`] is the first implementation (the plan's 3.9, "E1"), which lights chunks
//! whole: it is what the tests judge this one by, and the region's check of itself.
//!
//! No clock, no thread, no hash map: what a run does is a function of what it was told
//! and of the chunks it reads.

mod levels;
mod oracle;
mod pass;

use std::collections::BTreeMap;

use clustine_world::{BlockPos, Chunk, ChunkPos, Light, SectionLight};

use crate::{Around, VERSION};
use levels::{Levels, Lit};
use pass::{BLOCK, Pass, Queues, SKY, Shape};

pub use oracle::Oracle;

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
///
/// `cells` counts the entries the queues worked off and the cells of faces looked at;
/// `peak` is the most entries the two queues held at once in the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Work {
    pub chunks_lit_whole: u32,
    pub adopted: u32,
    pub cells: u64,
    pub peak: u64,
}

/// A chunk's levels as a run left them, handed out at a merge or a split and adopted
/// where the chunk comes back with the same blocks (the plan's 4.5). `walls` names the
/// eight neighbours (in [`crate::Around::neighbours`]' order, a bit each) that were
/// absent at that run; `taken` names the hand-over (the region and its tick), so that two
/// chunks kept from it know their levels agree.
///
/// A section of `light` that is [`SectionLight::Absent`] is adopted as full sky and no
/// block light: the levels of a section above everything, which is what the region's
/// test for carried light makes sure of (the plan's 3.5, clause 3).
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

/// What a present chunk waits for.
enum Pending {
    Nothing,
    /// To be lit whole from its blocks.
    Arrival,
    /// To take these levels and have its faces mended.
    Adoption(Box<Kept>),
}

/// A present chunk.
struct Slot {
    at: ChunkPos,
    /// Its levels, where it has been lit (or adopted) and is a part of the fixed point.
    /// A chunk without them is a wall to every other.
    lit: Option<Lit>,
    pending: Pending,
    /// Blocks noted as changed since the last run, `(y, z, x)`, `x` and `z` in the chunk.
    checks: Vec<(i32, u8, u8)>,
    /// Whether a run wrote its levels, so that they are put into their one form after.
    touched: bool,
}

/// The light of a region's present chunks, and what waits for the next run.
pub struct Lighting {
    sky: bool,
    /// The present chunks. A slot that went is `None` until another takes its place.
    slots: Vec<Option<Slot>>,
    free: Vec<usize>,
    index: BTreeMap<ChunkPos, usize>,
    /// The levels of chunks that went, or whose blocks were replaced whole, since the
    /// last run: what they lit of their neighbours is taken away at the next.
    departures: Vec<(ChunkPos, Lit)>,
    /// The lowest block and the number of sections of the dimension, from the first
    /// chunk read.
    shape: Option<Shape>,
    queues: Queues,
}

impl std::fmt::Debug for Lighting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lighting")
            .field("sky", &self.sky)
            .field("present", &self.index.len())
            .field("departures", &self.departures.len())
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

impl Lighting {
    /// No chunk present, in a dimension with a sky or without one.
    pub fn new(sky: bool) -> Self {
        Self {
            sky,
            slots: Vec::new(),
            free: Vec::new(),
            index: BTreeMap::new(),
            departures: Vec::new(),
            shape: None,
            queues: Queues::default(),
        }
    }

    fn slot(&self, at: ChunkPos) -> Option<&Slot> {
        self.slots[*self.index.get(&at)?].as_ref()
    }

    fn slot_mut(&mut self, at: ChunkPos) -> Option<&mut Slot> {
        self.slots[*self.index.get(&at)?].as_mut()
    }

    /// Notes that the chunk at `at` waits for `pending`, its old levels, if it had any,
    /// going as a departure.
    fn renew(&mut self, at: ChunkPos, pending: Pending) {
        if let Some(slot) = self.slot_mut(at) {
            let old = slot.lit.take();
            slot.pending = pending;
            slot.checks.clear();
            if let Some(old) = old {
                self.departures.push((at, old));
            }
            return;
        }
        let slot = Slot {
            at,
            lit: None,
            pending,
            checks: Vec::new(),
            touched: false,
        };
        let place = match self.free.pop() {
            Some(place) => {
                self.slots[place] = Some(slot);
                place
            }
            None => {
                self.slots.push(Some(slot));
                self.slots.len() - 1
            }
        };
        self.index.insert(at, place);
    }

    /// A chunk became present. Where it is present already, its blocks were replaced
    /// whole (a copy's new base, a loaded chunk become a copy or the reverse): the run
    /// takes it as a departure and then an arrival, since the old blocks may have lit
    /// its neighbours.
    pub fn arrived(&mut self, at: ChunkPos) {
        self.renew(at, Pending::Arrival);
    }

    /// A chunk became present with levels kept from a run elsewhere, its blocks as they
    /// were then. Its faces are mended against its neighbours as they are at the run:
    /// towards a neighbour adopted from the same hand-over in the same run they stand;
    /// towards any other they are taken as a departure (a wall, decreased) and then,
    /// where the neighbour is present, as an arrival (pulled in both ways).
    ///
    /// Where it is present already, its old levels go as a departure, as for
    /// [`Lighting::arrived`]. Changes noted in it before are forgotten: `kept` is of its
    /// blocks as they are.
    pub fn adopt(&mut self, at: ChunkPos, kept: Kept) {
        self.renew(at, Pending::Adoption(Box::new(kept)));
    }

    /// A chunk stopped being present.
    pub fn left(&mut self, at: ChunkPos) {
        let Some(place) = self.index.remove(&at) else {
            return;
        };
        let slot = self.slots[place]
            .take()
            .expect("an indexed slot holds a chunk");
        self.free.push(place);
        if let Some(lit) = slot.lit {
            self.departures.push((at, lit));
        }
    }

    /// A block changed in a way [`clustine_world::light_tells_apart`] sees. Passed over
    /// where its chunk is not present, or waits to be lit whole.
    pub fn changed(&mut self, at: BlockPos) {
        let Some(slot) = self.slot_mut(at.chunk()) else {
            return;
        };
        if matches!(slot.pending, Pending::Arrival) {
            return;
        }
        slot.checks
            .push((at.y, (at.z & 15) as u8, (at.x & 15) as u8));
    }

    /// The shape of the dimension, taken from `chunk` the first time; every chunk of a
    /// dimension spans the same heights.
    fn shape_of(&mut self, chunk: &Chunk) -> Shape {
        let shape = Shape {
            min_y: chunk.min_y(),
            sections: chunk.sections().len(),
        };
        assert!(shape.sections > 0, "a chunk has a section");
        let known = *self.shape.get_or_insert(shape);
        assert!(
            known == shape,
            "the chunks of a dimension span the same heights"
        );
        known
    }

    fn layers(&self) -> &'static [usize] {
        if self.sky { &[SKY, BLOCK] } else { &[BLOCK] }
    }

    /// Brings every present chunk to the invariant, reading `chunks` as they are. A
    /// present chunk that `chunks` does not give is left unlit, a wall to the others,
    /// until a run where it does.
    pub fn run(&mut self, chunks: &dyn Chunks) -> Work {
        let mut work = Work::default();
        self.queues.peak = 0;

        // What went, and the changes of chunks lit before.
        let departures = std::mem::take(&mut self.departures);
        let mut checked = Vec::new();
        for (place, slot) in self.slots.iter_mut().enumerate() {
            let Some(slot) = slot else { continue };
            if slot.lit.is_some() && !slot.checks.is_empty() {
                slot.checks.sort_unstable();
                slot.checks.dedup();
                checked.push((slot.at, place));
            }
        }
        checked.sort_unstable();
        if let Some(shape) = self
            .shape
            .filter(|_| !departures.is_empty() || !checked.is_empty())
        {
            for &layer in self.layers() {
                let mut pass = self.pass(chunks, shape, layer);
                for (at, lit) in &departures {
                    pass.depart(*at, lit);
                }
                for (_, place) in &checked {
                    pass.check_all(*place);
                }
                pass.propagate();
                work.cells += pass.cells;
            }
        }
        for (_, place) in checked {
            if let Some(slot) = self.slots[place].as_mut() {
                slot.checks.clear();
            }
        }

        // The adoptions, all placed before any is mended, so that the light of one that
        // came through another of the same hand-over is taken away through both.
        let mut adopted = Vec::new();
        let waiting: Vec<(ChunkPos, usize)> = self
            .index
            .iter()
            .filter(|(_, place)| {
                matches!(
                    self.slots[**place].as_ref().map(|slot| &slot.pending),
                    Some(Pending::Adoption(_))
                )
            })
            .map(|(at, place)| (*at, *place))
            .collect();
        for (at, place) in waiting {
            let Some(chunk) = chunks.chunk(at) else {
                continue;
            };
            let shape = self.shape_of(chunk);
            let slot = self.slots[place]
                .as_mut()
                .expect("an indexed slot holds a chunk");
            let Pending::Adoption(kept) = std::mem::replace(&mut slot.pending, Pending::Nothing)
            else {
                unreachable!("the slot was chosen for its adoption");
            };
            let fits = |sections: &[SectionLight]| sections.len() == shape.sections + 2;
            if !fits(&kept.light.block) || (self.sky && !fits(&kept.light.sky)) {
                // Levels of another height are no levels of this chunk: it is lit whole.
                slot.pending = Pending::Arrival;
                continue;
            }
            let Kept {
                light,
                walls,
                taken,
            } = *kept;
            slot.lit = Some(Lit::adopted(light, self.sky, shape));
            slot.touched = true;
            slot.checks.sort_unstable();
            slot.checks.dedup();
            adopted.push((at, place, walls, taken));
            work.adopted += 1;
        }
        if !adopted.is_empty() {
            let shape = self.shape.expect("a chunk was adopted");
            for &layer in self.layers() {
                let mut pass = self.pass(chunks, shape, layer);
                for &(at, place, walls, taken) in &adopted {
                    pass.mend(at, place, walls, taken, &adopted);
                }
                for &(_, place, _, _) in &adopted {
                    pass.check_all(place);
                }
                pass.propagate();
                work.cells += pass.cells;
            }
            for &(_, place, _, _) in &adopted {
                if let Some(slot) = self.slots[place].as_mut() {
                    slot.checks.clear();
                }
            }
        }

        // The arrivals, one at a time in ascending order.
        let arriving: Vec<(ChunkPos, usize)> = self
            .index
            .iter()
            .filter(|(_, place)| {
                matches!(
                    self.slots[**place].as_ref().map(|slot| &slot.pending),
                    Some(Pending::Arrival)
                )
            })
            .map(|(at, place)| (*at, *place))
            .collect();
        for (at, place) in arriving {
            let Some(chunk) = chunks.chunk(at) else {
                continue;
            };
            let shape = self.shape_of(chunk);
            let lit = Lit::arriving(chunk, self.sky, shape);
            let slot = self.slots[place]
                .as_mut()
                .expect("an indexed slot holds a chunk");
            slot.lit = Some(lit);
            slot.pending = Pending::Nothing;
            slot.checks.clear();
            slot.touched = true;
            work.chunks_lit_whole += 1;
            for &layer in self.layers() {
                let mut pass = self.pass(chunks, shape, layer);
                pass.arrive(at, place, chunk);
                pass.propagate();
                work.cells += pass.cells;
            }
        }

        // Every section written is put into its one form, so that what is kept and
        // handed out is equal where the light is.
        for slot in self.slots.iter_mut().flatten() {
            if std::mem::take(&mut slot.touched)
                && let Some(lit) = slot.lit.as_mut()
            {
                lit.compact();
            }
        }
        work.peak = self.queues.peak as u64;
        work
    }

    fn pass<'s, 'c>(
        &'s mut self,
        chunks: &'c dyn Chunks,
        shape: Shape,
        layer: usize,
    ) -> Pass<'s, 'c> {
        Pass::new(
            &mut self.slots,
            &self.index,
            chunks,
            &mut self.queues,
            shape,
            layer,
        )
    }

    /// The levels at a block as of the last run; `None` where no present chunk is, or
    /// the chunk has not been lit since it came. Above the sections it keeps: sky 15,
    /// blocks 0; below the light section under them, blocks 0.
    pub fn at(&self, at: BlockPos) -> Option<LightAt> {
        let lit = self.slot(at.chunk())?.lit.as_ref()?;
        let shape = self.shape.expect("a chunk was lit");
        let (x, z) = ((at.x & 15) as usize, (at.z & 15) as usize);
        // As `Light::sky_at` reads: below the kept sections, the lowest of them.
        let above_bottom = at.y - shape.min_y;
        let section = (above_bottom.div_euclid(16) + 1).max(0) as usize;
        if section >= shape.sections + 2 {
            return Some(LightAt {
                sky: if self.sky { 15 } else { 0 },
                block: 0,
            });
        }
        let index = (above_bottom.rem_euclid(16) as usize) << 8 | z << 4 | x;
        Some(LightAt {
            sky: if self.sky {
                lit.layers[SKY][section].get(index)
            } else {
                0
            },
            // Below the light section under the lowest kept one the game keeps no block
            // light, and reads 0 (found by the tests from the specification).
            block: if above_bottom < -16 {
                0
            } else {
                lit.layers[BLOCK][section].get(index)
            },
        })
    }

    /// The chunk's light in the crate's form, sections listed by its rule, for the tests
    /// and for whoever later wants it whole. The neighbours that count are those lit, as
    /// `chunks` gives them; a present chunk not lit yet is lit whole here, as the oracle
    /// does.
    pub fn light(&self, at: ChunkPos, chunks: &dyn Chunks) -> Option<Light> {
        let centre = chunks.chunk(at)?;
        let slot = self.slot(at)?;
        let Some(lit) = slot.lit.as_ref() else {
            let neighbours = around(at).map(|near| {
                self.index
                    .contains_key(&near)
                    .then(|| chunks.chunk(near))
                    .flatten()
            });
            return Some(crate::light(&Around {
                centre,
                neighbours,
                sky: self.sky,
            }));
        };
        let sections = centre.sections().len();
        // Which world sections of the nine chunks hold a block: a light section is
        // listed where it or one of the 26 around it does (`flood.rs`'s `has_light`).
        let mut occupied = vec![false; sections];
        let lit_neighbours = around(at).into_iter().filter_map(|near| {
            self.slot(near)
                .filter(|slot| slot.lit.is_some())
                .and_then(|_| chunks.chunk(near))
        });
        for chunk in std::iter::once(centre).chain(lit_neighbours) {
            for (index, section) in chunk.sections().iter().enumerate() {
                occupied[index] |= section.non_air_count() > 0;
            }
        }
        let listed = |light_section: usize| {
            let first = light_section.saturating_sub(2);
            let last = light_section.min(sections - 1);
            (first..=last).any(|section| occupied[section])
        };
        let layer = |levels: &[Levels]| -> Vec<SectionLight> {
            levels
                .iter()
                .enumerate()
                .map(|(index, levels)| {
                    if listed(index) {
                        levels.section()
                    } else {
                        SectionLight::Absent
                    }
                })
                .collect()
        };
        Some(Light {
            made_by: VERSION,
            beside: 0,
            sky: if self.sky {
                layer(&lit.layers[SKY])
            } else {
                vec![SectionLight::Absent; sections + 2]
            },
            block: layer(&lit.layers[BLOCK]),
        })
    }

    /// The chunk's levels to hand out with it, after a run, and which of its neighbours
    /// were walls at that run (absent, or present and not lit). A neighbour that went or
    /// was replaced since the run was no wall at it, and its light is in these levels:
    /// so a region may take each chunk's levels and let it go, one after the other.
    pub fn kept(&self, at: ChunkPos, taken: (u64, u64)) -> Option<Kept> {
        let lit = self.slot(at)?.lit.as_ref()?;
        let walls = around(at)
            .iter()
            .enumerate()
            .filter(|(_, near)| {
                self.slot(**near).is_none_or(|slot| slot.lit.is_none())
                    && !self.departures.iter().any(|(gone, _)| gone == *near)
            })
            .fold(0u8, |walls, (bit, _)| walls | 1 << bit);
        let sections = lit.layers[BLOCK].len();
        let layer = |levels: &[Levels]| levels.iter().map(Levels::section).collect();
        Some(Kept {
            light: Light {
                made_by: VERSION,
                beside: 0,
                sky: if self.sky {
                    layer(&lit.layers[SKY])
                } else {
                    vec![SectionLight::Absent; sections]
                },
                block: layer(&lit.layers[BLOCK]),
            },
            walls,
            taken,
        })
    }

    /// Whether the chunk is present.
    pub fn has(&self, at: ChunkPos) -> bool {
        self.index.contains_key(&at)
    }

    /// Whether the chunk is present and lit as of the last run, nothing noted since
    /// that would change it: nothing waits in it or in a chunk about it, and nothing
    /// about it went.
    pub fn is_lit(&self, at: ChunkPos) -> bool {
        if !self.slot(at).is_some_and(|slot| slot.lit.is_some()) {
            return false;
        }
        let near = |other: ChunkPos| (other.x - at.x).abs() <= 1 && (other.z - at.z).abs() <= 1;
        let quiet = std::iter::once(at).chain(around(at)).all(|other| {
            self.slot(other).is_none_or(|slot| {
                matches!(slot.pending, Pending::Nothing) && slot.checks.is_empty()
            })
        });
        quiet && !self.departures.iter().any(|(gone, _)| near(*gone))
    }

    /// Whether anything waits for a run.
    pub fn is_settled(&self) -> bool {
        self.departures.is_empty()
            && self
                .slots
                .iter()
                .flatten()
                .all(|slot| matches!(slot.pending, Pending::Nothing) && slot.checks.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clustine_world::{Biome, Section};

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
    fn a_change_lights_its_neighbour_and_a_departure_takes_the_light_away() {
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
        assert_eq!(work.chunks_lit_whole, 0);
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
