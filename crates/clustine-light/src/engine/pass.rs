//! The propagation: the game's queues of decreases and increases, worked off over the
//! present chunks of one layer.
//!
//! Ported from the game's 26.3 classes as dumped in `n8-javap/` (the plan's 3.4):
//! `LightEngine` (`runLightUpdates`, `propagateIncreases`, `propagateDecreases`),
//! `LightEngine$QueueEntry` (the entries: a level in the low four bits, a bit for each of
//! the six directions, a bit for light a block gives), `BlockLightEngine` (`checkNode`,
//! `propagateIncrease`, `propagateDecrease`) and `SkyLightEngine` (`checkNode`,
//! `updateSourcesInColumn`, `removeSourcesBelow`, `addSourcesAbove`, its entries for a
//! source added and removed). What is not ported, and why, is the plan's 3.4: the
//! section storages' gating, their maps and the queued data of loading chunks. Light
//! spreads through every section of a present chunk alike, as `flood.rs` spreads it.
//!
//! **Why the result is the fixed point.** A level is the greatest a way from a source
//! gives (a source's own level, less what each block entered takes). A decrease sets to
//! 0 every cell whose level may have come by a way that is gone, which is any cell no
//! brighter than one less than the cell it is decreased from: it takes away more than it
//! must, never less. Where a decrease meets a cell brighter than that, the cell has its
//! light from elsewhere, and it is queued to spread back. After the decreases every cell
//! is at most its true level, and every place where a cell may be brighter than its
//! neighbours allow is queued; the increases then raise each cell to the greatest level
//! a neighbour gives it, which is the fixed point.
//!
//! **Where this differs from the game**, both for the oracle's sake and neither seen in
//! a value: a check of a block whose own light is no less than the level stored there
//! takes the stored light away and lets it come back, where the game only pulls light
//! in. The game's way would keep light that passed the block before its faces changed,
//! where the block both gives light and stops it by its shape. And the chunks of a
//! window are held at hand, not cached two at a time as the game caches them.

use std::collections::{BTreeMap, VecDeque};

use clustine_data::{BlockState, Face};
use clustine_world::{Chunk, ChunkPos};

use super::levels::{Levels, Lit, cell, lowest_in, open_from, state_at};
use super::{Chunks, Slot};
use crate::faces;
use crate::flood::{SHAPED, TAKES};

/// The layer of the sky's light.
pub(super) const SKY: usize = 0;
/// The layer of the blocks' light.
pub(super) const BLOCK: usize = 1;

/// The lowest block of the dimension and its number of sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Shape {
    pub(super) min_y: i32,
    pub(super) sections: usize,
}

/// An entry's level, in its low four bits.
const LEVEL: u16 = 15;
/// The bit of the first direction; the six follow in the order of [`Face::ALL`].
const FIRST_DIRECTION: u16 = 1 << 4;
/// Every direction.
const ALL: u16 = 0b11_1111 << 4;
/// The entry is the light the block gives (the game's `FLAG_INCREASE_FROM_EMISSION`).
const EMISSION: u16 = 1 << 11;

const UP: usize = Face::Up as usize;

fn direction(face: usize) -> u16 {
    FIRST_DIRECTION << face
}

fn all_but(face: usize) -> u16 {
    ALL & !direction(face)
}

/// The face opposite: the faces come in pairs, down and up first.
fn opposite(face: usize) -> usize {
    face ^ 1
}

/// The four sides of a chunk: the direction towards the neighbour there, and the
/// neighbour's offset in chunks.
const SIDES: [(usize, i32, i32); 4] = [
    (Face::North as usize, 0, -1),
    (Face::South as usize, 0, 1),
    (Face::West as usize, -1, 0),
    (Face::East as usize, 1, 0),
];

/// The bit of a side's neighbour in a [`super::Kept`]'s `walls`, by the order of
/// [`crate::Around::neighbours`].
fn wall_bit(side: usize) -> u8 {
    match side {
        2 => 1 << 1,
        3 => 1 << 6,
        4 => 1 << 3,
        _ => 1 << 4,
    }
}

/// A cell and what to do there: the game's packed position and entry.
#[derive(Clone, Copy)]
struct Node {
    x: i32,
    z: i32,
    /// Counted from the bottom of the section below the chunks.
    y: u16,
    entry: u16,
}

/// The two queues, kept from run to run for their memory.
#[derive(Default)]
pub(super) struct Queues {
    decrease: VecDeque<Node>,
    increase: VecDeque<Node>,
    /// The most entries both held at once since the run began.
    pub(super) peak: usize,
}

/// A chunk of the window: not looked up yet, a wall, or lit, with its slot and blocks.
#[derive(Clone, Copy)]
enum Near<'c> {
    Unknown,
    Wall,
    Lit(usize, &'c Chunk),
}

/// One layer's work over the present chunks.
pub(super) struct Pass<'s, 'c> {
    slots: &'s mut [Option<Slot>],
    index: &'s BTreeMap<ChunkPos, usize>,
    chunks: &'c dyn Chunks,
    queues: &'s mut Queues,
    shape: Shape,
    layer: usize,
    /// Cells from the bottom of the section below the chunks to the top of the one above.
    height: u16,
    /// The chunk in the middle of the window, and the nine about it, north-west first.
    centre: ChunkPos,
    window: [Near<'c>; 9],
    /// Entries worked off and cells looked at.
    pub(super) cells: u64,
}

impl<'s, 'c> Pass<'s, 'c> {
    pub(super) fn new(
        slots: &'s mut [Option<Slot>],
        index: &'s BTreeMap<ChunkPos, usize>,
        chunks: &'c dyn Chunks,
        queues: &'s mut Queues,
        shape: Shape,
        layer: usize,
    ) -> Self {
        Pass {
            slots,
            index,
            chunks,
            queues,
            shape,
            layer,
            height: ((shape.sections + 2) * 16) as u16,
            centre: ChunkPos::new(i32::MIN, i32::MIN),
            window: [Near::Unknown; 9],
            cells: 0,
        }
    }

    /// The chunk at `x`, `z` (in chunks), from the window. One beyond it moves the
    /// window there: a step from a cell of the window's middle never leaves it.
    #[inline]
    fn near(&mut self, x: i32, z: i32) -> Option<(usize, &'c Chunk)> {
        let (mut dx, mut dz) = (x.wrapping_sub(self.centre.x), z.wrapping_sub(self.centre.z));
        if !(-1..=1).contains(&dx) || !(-1..=1).contains(&dz) {
            self.centre = ChunkPos::new(x, z);
            self.window = [Near::Unknown; 9];
            (dx, dz) = (0, 0);
        }
        let slot = ((dz + 1) * 3 + dx + 1) as usize;
        if let Near::Unknown = self.window[slot] {
            let at = ChunkPos::new(x, z);
            self.window[slot] = match self.index.get(&at) {
                Some(place)
                    if self.slots[*place]
                        .as_ref()
                        .is_some_and(|slot| slot.lit.is_some()) =>
                {
                    match self.chunks.chunk(at) {
                        Some(chunk) => Near::Lit(*place, chunk),
                        None => Near::Wall,
                    }
                }
                _ => Near::Wall,
            };
        }
        match self.window[slot] {
            Near::Lit(place, chunk) => Some((place, chunk)),
            _ => None,
        }
    }

    fn lit(&self, place: usize) -> &Lit {
        self.slots[place]
            .as_ref()
            .and_then(|slot| slot.lit.as_ref())
            .expect("the window holds lit chunks")
    }

    fn levels(&self, place: usize, y: u16) -> &Levels {
        &self.lit(place).layers[self.layer][usize::from(y >> 4)]
    }

    #[inline]
    fn get(&self, place: usize, x: i32, y: u16, z: i32) -> u8 {
        self.levels(place, y).get(index(x, y, z))
    }

    #[inline]
    fn set(&mut self, place: usize, x: i32, y: u16, z: i32, level: u8) {
        let layer = self.layer;
        let slot = self.slots[place]
            .as_mut()
            .expect("the window holds lit chunks");
        slot.touched = true;
        let lit = slot.lit.as_mut().expect("the window holds lit chunks");
        lit.layers[layer][usize::from(y >> 4)].set(index(x, y, z), level);
    }

    fn decrease(&mut self, x: i32, y: u16, z: i32, entry: u16) {
        self.queues.decrease.push_back(Node { x, z, y, entry });
    }

    fn increase(&mut self, x: i32, y: u16, z: i32, entry: u16) {
        self.queues.increase.push_back(Node { x, z, y, entry });
    }

    /// The cell one step from `node` towards `face`, unless it is beyond the sections
    /// kept, where nothing is.
    #[inline]
    fn step(&self, node: Node, face: usize) -> Option<(i32, u16, i32)> {
        let Node { x, y, z, .. } = node;
        match face {
            0 => y.checked_sub(1).map(|y| (x, y, z)),
            1 => (y + 1 < self.height).then_some((x, y + 1, z)),
            2 => Some((x, y, z - 1)),
            3 => Some((x, y, z + 1)),
            4 => Some((x - 1, y, z)),
            _ => Some((x + 1, y, z)),
        }
    }

    /// What a block gives in this layer: the sky's light starts from its columns, not
    /// from blocks.
    fn gives(&self, state: BlockState) -> u8 {
        if self.layer == BLOCK {
            cell(state).gives
        } else {
            0
        }
    }

    /// Works off the decreases and then the increases, to the fixed point.
    pub(super) fn propagate(&mut self) {
        self.note_peak();
        while let Some(node) = self.queues.decrease.pop_front() {
            self.cells += 1;
            self.propagate_decrease(node);
            self.note_peak();
        }
        while let Some(node) = self.queues.increase.pop_front() {
            self.cells += 1;
            self.propagate_increase(node);
            self.note_peak();
        }
    }

    fn note_peak(&mut self) {
        let held = self.queues.decrease.len() + self.queues.increase.len();
        self.queues.peak = self.queues.peak.max(held);
    }

    /// The game's `propagateDecrease`: each neighbour no brighter than one less than
    /// this cell was may have had its light from here, and is set to 0 and decreased in
    /// turn; a brighter one has it from elsewhere and is queued to spread it back.
    fn propagate_decrease(&mut self, node: Node) {
        let from = node.entry & LEVEL;
        for face in 0..6 {
            if node.entry & direction(face) == 0 {
                continue;
            }
            let Some((x, y, z)) = self.step(node, face) else {
                continue;
            };
            let Some((place, chunk)) = self.near(x >> 4, z >> 4) else {
                continue;
            };
            let level = self.get(place, x, y, z);
            if level == 0 {
                continue;
            }
            if u16::from(level) < from {
                let state = state_at(chunk, (x & 15) as usize, usize::from(y), (z & 15) as usize);
                let gives = self.gives(state);
                self.set(place, x, y, z, 0);
                if gives < level {
                    self.decrease(x, y, z, u16::from(level) | all_but(opposite(face)));
                }
                if gives > 0 {
                    self.increase(x, y, z, u16::from(gives) | ALL | EMISSION);
                }
            } else {
                self.increase(x, y, z, u16::from(level) | direction(opposite(face)));
            }
        }
    }

    /// The game's `propagateIncreases` and `propagateIncrease`: an entry whose level the
    /// cell no longer has is stale and passed over; otherwise each neighbour that would
    /// be brighter for it is raised and spreads in turn.
    fn propagate_increase(&mut self, node: Node) {
        let Some((place, chunk)) = self.near(node.x >> 4, node.z >> 4) else {
            return;
        };
        let from = (node.entry & LEVEL) as u8;
        let mut level = self.get(place, node.x, node.y, node.z);
        if node.entry & EMISSION != 0 && level < from {
            self.set(place, node.x, node.y, node.z, from);
            level = from;
        }
        if level != from {
            return;
        }
        let mut from_state = None;
        for face in 0..6 {
            if node.entry & direction(face) == 0 {
                continue;
            }
            let Some((x, y, z)) = self.step(node, face) else {
                continue;
            };
            let Some((to_place, to_chunk)) = self.near(x >> 4, z >> 4) else {
                continue;
            };
            let had = self.get(to_place, x, y, z);
            if from <= had + 1 {
                continue;
            }
            let to_state = state_at(
                to_chunk,
                (x & 15) as usize,
                usize::from(y),
                (z & 15) as usize,
            );
            let to_kind = cell(to_state).kind;
            let takes = to_kind & TAKES;
            if takes >= from || from - takes <= had {
                continue;
            }
            let arrives = from - takes;
            let from_state = *from_state.get_or_insert_with(|| {
                state_at(
                    chunk,
                    (node.x & 15) as usize,
                    usize::from(node.y),
                    (node.z & 15) as usize,
                )
            });
            if (to_kind | cell(from_state).kind) & SHAPED != 0
                && faces::stop_light(from_state, to_state, Face::ALL[face])
            {
                continue;
            }
            self.set(to_place, x, y, z, arrives);
            if arrives > 1 {
                self.increase(x, y, z, u16::from(arrives) | all_but(opposite(face)));
            }
        }
    }

    /// The game's `checkNode` for every block noted as changed in the chunk.
    pub(super) fn check_all(&mut self, place: usize) {
        let slot = self.slots[place]
            .as_ref()
            .expect("a checked chunk is present");
        let at = slot.at;
        let checks = slot.checks.clone();
        let Some((_, chunk)) = self.near(at.x, at.z) else {
            return;
        };
        let bottom = self.shape.min_y - 16;
        for (y, z, x) in checks {
            let y = y - bottom;
            if y < 16 || y >= i32::from(self.height) - 16 {
                continue;
            }
            let (x, z) = (at.x * 16 + i32::from(x), at.z * 16 + i32::from(z));
            self.check(place, chunk, x, y as u16, z);
        }
    }

    fn check(&mut self, place: usize, chunk: &'c Chunk, x: i32, y: u16, z: i32) {
        self.cells += 1;
        if self.layer == SKY {
            let column = ((z & 15) << 4 | (x & 15)) as usize;
            let lowest = lowest_in(
                chunk,
                (x & 15) as usize,
                (z & 15) as usize,
                open_from(chunk),
            );
            self.move_sources(place, x, z, column, lowest);
            if y >= lowest {
                // A source stays one: what it lit is taken away and given again, as
                // its faces may be other than they were.
                self.decrease(x, y, z, 15 | all_but(UP));
                self.increase(x, y, z, 15 | all_but(UP));
                return;
            }
        }
        let old = self.get(place, x, y, z);
        if old > 0 {
            self.set(place, x, y, z, 0);
            self.decrease(x, y, z, u16::from(old) | ALL);
        } else {
            // The game's `PULL_LIGHT_IN_ENTRY`: every lit neighbour spreads back.
            self.decrease(x, y, z, 1 | ALL);
        }
        let state = state_at(chunk, (x & 15) as usize, usize::from(y), (z & 15) as usize);
        let gives = self.gives(state);
        if gives > 0 {
            self.increase(x, y, z, u16::from(gives) | ALL | EMISSION);
        }
    }

    /// The game's `updateSourcesInColumn`: where the lowest cell the sky reaches has
    /// risen, the cells it left are sources no more and are decreased
    /// (`removeSourcesBelow`); where it has fallen, the cells it reaches now are sources
    /// and spread (`addSourcesAbove`).
    fn move_sources(&mut self, place: usize, x: i32, z: i32, column: usize, lowest: u16) {
        let slot = self.slots[place]
            .as_mut()
            .expect("a checked chunk is present");
        let lit = slot.lit.as_mut().expect("a checked chunk is lit");
        let old = std::mem::replace(&mut lit.lowest[column], lowest);
        if lowest > old {
            for y in old..lowest {
                self.set(place, x, y, z, 0);
                let entry = if y + 1 == lowest { ALL } else { all_but(UP) };
                self.decrease(x, y, z, 15 | entry);
            }
        } else {
            for y in lowest..old {
                self.set(place, x, y, z, 15);
                self.increase(x, y, z, 15 | all_but(UP));
            }
        }
    }

    /// Takes away what a chunk that went lit of its neighbours: each cell of its faces
    /// becomes a wall, decreased towards the neighbour as the game decreases a block
    /// that comes to stop light. A neighbour's cell no darker than the face's own has its
    /// light from elsewhere and is passed over, as the decrease would pass it over.
    pub(super) fn depart(&mut self, at: ChunkPos, lit: &Lit) {
        for (side, dx, dz) in SIDES {
            let Some((place, _)) = self.near(at.x + dx, at.z + dz) else {
                continue;
            };
            for y in 0..self.height {
                let gone = &lit.layers[self.layer][usize::from(y >> 4)];
                if let Levels::Uniform(0) = gone {
                    continue;
                }
                for along in 0..16 {
                    let (x, z) = face_cell(at, side, along);
                    let level = gone.get(index(x, y, z));
                    let (nx, nz) = beyond(x, z, side);
                    self.cells += 1;
                    let there = self.get(place, nx, y, nz);
                    if level == 0 || there == 0 || there >= level {
                        continue;
                    }
                    self.decrease(x, y, z, u16::from(level) | direction(side));
                }
            }
        }
    }

    /// Mends the faces of a chunk just adopted (the plan's 3.2 and section 14): towards
    /// a neighbour adopted from the same hand-over in this run they stand; towards a
    /// neighbour that was there when the levels were kept, whatever came through is
    /// taken away (a decrease from the brightest level there is, which takes away more
    /// than came and never less); and towards a neighbour present now, light is pulled
    /// across both ways.
    pub(super) fn mend(
        &mut self,
        at: ChunkPos,
        place: usize,
        walls: u8,
        taken: (u64, u64),
        adopted: &[(ChunkPos, usize, u8, (u64, u64))],
    ) {
        for (side, dx, dz) in SIDES {
            let neighbour = ChunkPos::new(at.x + dx, at.z + dz);
            let was_there = walls & wall_bit(side) == 0;
            let standing = was_there
                && adopted.iter().any(|(other, _, other_walls, other_taken)| {
                    *other == neighbour
                        && *other_taken == taken
                        && other_walls & wall_bit(opposite(side)) == 0
                });
            if standing {
                continue;
            }
            let now = self.near(neighbour.x, neighbour.z).map(|(there, _)| there);
            if !was_there && now.is_none() {
                continue;
            }
            for y in 0..self.height {
                let section = usize::from(y >> 4);
                let own = self.lit(place).layers[self.layer][section].clone_uniform();
                let theirs =
                    now.map(|there| self.lit(there).layers[self.layer][section].clone_uniform());
                // Nothing to take away and nothing to pull, a section at a time.
                match (own, theirs) {
                    (Some(0), None | Some(Some(0 | 1))) => continue,
                    (Some(15), None | Some(Some(14 | 15))) => continue,
                    _ => {}
                }
                for along in 0..16 {
                    let (x, z) = face_cell(at, side, along);
                    let (nx, nz) = beyond(x, z, side);
                    self.cells += 1;
                    let level = self.get(place, x, y, z);
                    // A cell decreased from across the face does not decrease back, as
                    // the face is a wall to it; so what the neighbour gives it now is
                    // pulled in whatever it holds before the decrease.
                    let decreased = was_there && level > 0 && level < 15;
                    if decreased {
                        self.decrease(nx, y, nz, 15 | direction(opposite(side)));
                    }
                    let Some(there) = now else { continue };
                    let other = self.get(there, nx, y, nz);
                    if level > 1 && level > other + 1 {
                        self.increase(x, y, z, u16::from(level) | direction(side));
                    }
                    if other > 1 && (decreased || other > level + 1) {
                        self.increase(nx, y, nz, u16::from(other) | direction(opposite(side)));
                    }
                }
            }
        }
    }

    /// Lights a chunk just placed with its sky's columns: the columns spread where they
    /// have something beside or below them that the sky does not reach, the blocks
    /// that give light spread, and the lit neighbours' faces spread into it.
    pub(super) fn arrive(&mut self, at: ChunkPos, place: usize, chunk: &'c Chunk) {
        if self.layer == SKY {
            self.seed_columns(at, place);
        } else {
            self.seed_emitters(at, place, chunk);
        }
        for (side, dx, dz) in SIDES {
            let Some((there, _)) = self.near(at.x + dx, at.z + dz) else {
                continue;
            };
            for y in 0..self.height {
                let section = usize::from(y >> 4);
                let theirs = self.lit(there).layers[self.layer][section].clone_uniform();
                let own = self.lit(place).layers[self.layer][section].clone_uniform();
                match (theirs, own) {
                    (Some(0 | 1), _) => continue,
                    (Some(theirs), Some(own)) if theirs <= own + 1 => continue,
                    _ => {}
                }
                for along in 0..16 {
                    let (x, z) = face_cell(at, side, along);
                    let (nx, nz) = beyond(x, z, side);
                    self.cells += 1;
                    let other = self.get(there, nx, y, nz);
                    if other > 1 && other > self.get(place, x, y, z) + 1 {
                        self.increase(nx, y, nz, u16::from(other) | direction(opposite(side)));
                    }
                }
            }
        }
    }

    /// Queues the sources of each column that have a cell beside or below them that is
    /// not a source (`flood.rs`'s `sky_light`, and the game's `addSourcesAbove`).
    fn seed_columns(&mut self, at: ChunkPos, place: usize) {
        let lowest = *self.lit(place).lowest;
        let mut beside_lowest = [[0u16; 16]; 4];
        for (number, (_, dx, dz)) in SIDES.into_iter().enumerate() {
            if let Some((there, _)) = self.near(at.x + dx, at.z + dz) {
                let theirs = &self.lit(there).lowest;
                for (along, beside) in beside_lowest[number].iter_mut().enumerate() {
                    // The neighbour's column across the face from column `along`.
                    let column = match number {
                        0 => 15 << 4 | along,
                        1 => along,
                        2 => along << 4 | 15,
                        _ => along << 4,
                    };
                    *beside = theirs[column];
                }
            }
        }
        for z in 0..16usize {
            for x in 0..16usize {
                let from = lowest[z << 4 | x];
                let beside = [
                    if z > 0 {
                        lowest[(z - 1) << 4 | x]
                    } else {
                        beside_lowest[0][x]
                    },
                    if z < 15 {
                        lowest[(z + 1) << 4 | x]
                    } else {
                        beside_lowest[1][x]
                    },
                    if x > 0 {
                        lowest[z << 4 | (x - 1)]
                    } else {
                        beside_lowest[2][z]
                    },
                    if x < 15 {
                        lowest[z << 4 | (x + 1)]
                    } else {
                        beside_lowest[3][z]
                    },
                ]
                .into_iter()
                .max()
                .unwrap_or(0);
                let to = if from > 0 {
                    beside.max(from + 1)
                } else {
                    beside
                };
                let (gx, gz) = (at.x * 16 + x as i32, at.z * 16 + z as i32);
                for y in from..to {
                    self.increase(gx, y, gz, 15 | all_but(UP));
                }
            }
        }
    }

    /// Sets and queues every block of the chunk that gives light.
    fn seed_emitters(&mut self, at: ChunkPos, place: usize, chunk: &'c Chunk) {
        for (index, section) in chunk.sections().iter().enumerate() {
            if !section.may_hold(|state| cell(state).gives > 0) {
                continue;
            }
            let bottom = ((index + 1) * 16) as u16;
            let mut last = None;
            let mut gives = 0;
            for (position, state) in section.states().enumerate() {
                if last != Some(state) {
                    last = Some(state);
                    gives = cell(state).gives;
                }
                if gives == 0 {
                    continue;
                }
                let x = at.x * 16 + (position & 15) as i32;
                let z = at.z * 16 + (position >> 4 & 15) as i32;
                let y = bottom + (position >> 8) as u16;
                self.set(place, x, y, z, gives);
                self.increase(x, y, z, u16::from(gives) | ALL | EMISSION);
            }
        }
    }
}

impl Levels {
    /// The one level of a uniform section.
    fn clone_uniform(&self) -> Option<u8> {
        match self {
            Levels::Uniform(level) => Some(*level),
            Levels::Nibbles(_) => None,
        }
    }
}

/// A cell's place in its section's levels.
#[inline]
fn index(x: i32, y: u16, z: i32) -> usize {
    usize::from(y & 15) << 8 | ((z & 15) as usize) << 4 | (x & 15) as usize
}

/// The cell at `along` on the side `side` of the chunk at `at`, in blocks.
fn face_cell(at: ChunkPos, side: usize, along: i32) -> (i32, i32) {
    let (x0, z0) = (at.x * 16, at.z * 16);
    match side {
        2 => (x0 + along, z0),
        3 => (x0 + along, z0 + 15),
        4 => (x0, z0 + along),
        _ => (x0 + 15, z0 + along),
    }
}

/// The cell one step beyond a face cell, in the neighbour.
fn beyond(x: i32, z: i32, side: usize) -> (i32, i32) {
    match side {
        2 => (x, z - 1),
        3 => (x, z + 1),
        4 => (x - 1, z),
        _ => (x + 1, z),
    }
}
