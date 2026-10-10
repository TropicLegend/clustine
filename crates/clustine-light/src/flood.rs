//! Light as a flood from its sources over the nine chunks.
//!
//! The rules are adapted from SteelMC's `steel-core/src/chunk/light/mod.rs`
//! (`get_light_opacity`, `get_light_block_into`), `propagation.rs` (`target_level`,
//! `perform_light_increase`), `sky_sources.rs` (`find_lowest_source_y`,
//! `is_edge_occluded`) and `sky_propagation/algorithms.rs`
//! (`try_propagate_skylight_inner`), branch 26.3 at commit
//! 885c4b3e60ed79862c37311780774f76806cb714, AGPL-3.0. Only the rules: there is no
//! queue of changes, no cache of sections and no decrease here, because a chunk is lit
//! once and from nothing. The grid, the order of the flood and the cut by distance are
//! this crate's.
//!
//! A level only falls along its way, so cells are worked off from level 15 down, and a
//! cell's level is final when its turn comes.

use clustine_data::{BlockState, Face, blocks};

use crate::faces;
use crate::{Blocks, ChunkLight, LIGHT_ARRAY_LENGTH, SECTION_EDGE, SectionLight};

/// Cells along a horizontal edge of the grid: three chunks and a wall on either side,
/// so that no step leaves the grid.
const WIDTH: usize = 3 * SECTION_EDGE + 2;
/// Cells of one horizontal layer.
const LAYER: usize = WIDTH * WIDTH;
/// The grid's coordinate of the centre chunk's first block, along x and along z.
const CENTRE: usize = SECTION_EDGE + 1;

/// The low bits of a cell's kind: what the block takes from light that enters, 1 to 15.
const TAKES: u8 = 0x0f;
/// The block dampens by nothing, so the sky may come straight down through it.
const CLEAR: u8 = 0x20;
/// The block has faces that stop light by their shape.
const SHAPED: u8 = 0x40;

/// A cell nothing enters: a chunk that is not there, and the rim of the grid.
const WALL: u8 = 15;

/// The six steps, in the order of [`Face::ALL`].
const STEPS: [isize; 6] = [
    -(LAYER as isize),
    LAYER as isize,
    -(WIDTH as isize),
    WIDTH as isize,
    -1,
    1,
];

/// What light needs of a block state.
#[derive(Clone, Copy)]
struct Cell {
    kind: u8,
    gives: u8,
    air: bool,
}

impl Cell {
    fn of(state: BlockState) -> Cell {
        let props = state.props();
        let dampening = props.light_dampening().min(15);
        let mut kind = dampening.max(1);
        if dampening == 0 {
            kind |= CLEAR;
        }
        if faces::has_shape(state) {
            kind |= SHAPED;
        }
        Cell {
            kind,
            gives: props.light_emission().min(15),
            air: props.is_air(),
        }
    }
}

/// The blocks of the nine chunks as light sees them, with one section of air below and
/// one above the world, inside a rim of walls.
struct Grid {
    /// Sections of the world, without the two beyond it.
    sections: usize,
    kind: Vec<u8>,
    state: Vec<u16>,
    /// The cells that give light, with their level, in the order they were read.
    emitters: Vec<(u32, u8)>,
    /// For each of the nine chunks, north-west first: whether it is there.
    present: [bool; 9],
    /// For each chunk and each of its sections: whether it holds a block that is not air.
    occupied: Vec<bool>,
    /// For each column of the grid: how many steps sideways lie between it and the
    /// centre chunk. Light of a level no higher than that never arrives there.
    distance: Vec<u8>,
}

/// The cell of a block: `x` and `z` in the grid, `y` counted from the bottom of the
/// section below the world.
fn cell(x: usize, y: usize, z: usize) -> usize {
    ((y + 1) * WIDTH + z) * WIDTH + x
}

impl Grid {
    fn read<B: Blocks + ?Sized>(blocks: &B) -> Grid {
        let height = blocks.height() as usize;
        assert!(
            height > 0 && height % SECTION_EDGE == 0,
            "a world's height is a multiple of 16 and not 0"
        );
        let sections = height / SECTION_EDGE;
        let cells = ((sections + 2) * SECTION_EDGE + 2) * LAYER;
        let mut grid = Grid {
            sections,
            kind: vec![WALL; cells],
            state: vec![blocks::AIR.0; cells],
            emitters: Vec::new(),
            present: [false; 9],
            occupied: vec![false; 9 * sections],
            distance: (0..LAYER)
                .map(|column| beyond_centre(column % WIDTH) + beyond_centre(column / WIDTH))
                .collect(),
        };
        let min_y = blocks.min_y();
        for (slot, (chunk_x, chunk_z)) in chunk_offsets().enumerate() {
            if (chunk_x, chunk_z) != (0, 0) && !blocks.has_chunk(chunk_x, chunk_z) {
                continue;
            }
            grid.present[slot] = true;
            let first_x = slot % 3 * SECTION_EDGE + 1;
            let first_z = slot / 3 * SECTION_EDGE + 1;
            let air = Cell::of(blocks::AIR);
            grid.fill(first_x, 0, first_z, blocks::AIR, air);
            grid.fill(first_x, sections + 1, first_z, blocks::AIR, air);
            for section in 0..sections {
                if let Some(state) = blocks.uniform_section(chunk_x, chunk_z, section) {
                    let of = Cell::of(state);
                    grid.fill(first_x, section + 1, first_z, state, of);
                    grid.occupied[slot * sections + section] = !of.air;
                    continue;
                }
                let mut last = blocks::AIR;
                let mut of = air;
                let mut occupied = false;
                for y in 0..SECTION_EDGE {
                    let world_y = min_y + (section * SECTION_EDGE + y) as i32;
                    for z in 0..SECTION_EDGE {
                        let world_z = chunk_z * SECTION_EDGE as i32 + z as i32;
                        let row = cell(first_x, (section + 1) * SECTION_EDGE + y, first_z + z);
                        for x in 0..SECTION_EDGE {
                            let world_x = chunk_x * SECTION_EDGE as i32 + x as i32;
                            let state = blocks.state(world_x, world_y, world_z);
                            if state != last {
                                last = state;
                                of = Cell::of(state);
                            }
                            grid.kind[row + x] = of.kind;
                            grid.state[row + x] = state.0;
                            occupied |= !of.air;
                            if of.gives > 0 {
                                grid.emitters.push(((row + x) as u32, of.gives));
                            }
                        }
                    }
                }
                grid.occupied[slot * sections + section] = occupied;
            }
        }
        grid
    }

    /// Makes a whole section one state. `section` counts from the one below the world.
    fn fill(
        &mut self,
        first_x: usize,
        section: usize,
        first_z: usize,
        state: BlockState,
        of: Cell,
    ) {
        for y in 0..SECTION_EDGE {
            for z in 0..SECTION_EDGE {
                let row = cell(first_x, section * SECTION_EDGE + y, first_z + z);
                self.kind[row..row + SECTION_EDGE].fill(of.kind);
                self.state[row..row + SECTION_EDGE].fill(state.0);
                if of.gives > 0 {
                    for at in row..row + SECTION_EDGE {
                        self.emitters.push((at as u32, of.gives));
                    }
                }
            }
        }
    }

    /// Blocks from the bottom of the section below the world to the top of the one
    /// above it.
    fn height(&self) -> usize {
        (self.sections + 2) * SECTION_EDGE
    }

    /// Spreads the levels of the cells in `waiting` until nothing rises any more.
    ///
    /// `waiting[level]` holds cells that had that level when they were put there. A
    /// cell that has risen since is passed over: it waits again further up.
    fn spread(&self, levels: &mut [u8], waiting: &mut [Vec<u32>; 16]) {
        for level in (2..=15u8).rev() {
            let cells = std::mem::take(&mut waiting[usize::from(level)]);
            for from in cells {
                let from = from as usize;
                if levels[from] != level {
                    continue;
                }
                for face in Face::ALL {
                    let to = from.wrapping_add_signed(STEPS[face as usize]);
                    let had = levels[to];
                    if had >= level - 1 {
                        continue;
                    }
                    let kind = self.kind[to];
                    let takes = kind & TAKES;
                    if takes >= level || level - takes <= had {
                        continue;
                    }
                    let arrives = level - takes;
                    if arrives <= self.distance[to % LAYER] {
                        continue;
                    }
                    if (kind | self.kind[from]) & SHAPED != 0
                        && faces::stop_light(
                            BlockState(self.state[from]),
                            BlockState(self.state[to]),
                            face,
                        )
                    {
                        continue;
                    }
                    levels[to] = arrives;
                    waiting[usize::from(arrives)].push(to as u32);
                }
            }
        }
    }

    fn block_light(&self) -> Vec<u8> {
        let mut levels = vec![0; self.kind.len()];
        let mut waiting: [Vec<u32>; 16] = Default::default();
        for (at, gives) in &self.emitters {
            let column = *at as usize % LAYER;
            if *gives > self.distance[column] {
                levels[*at as usize] = *gives;
                waiting[usize::from(*gives)].push(*at);
            }
        }
        self.spread(&mut levels, &mut waiting);
        levels
    }

    fn sky_light(&self) -> Vec<u8> {
        let height = self.height();
        // Above the highest section of a chunk that holds anything the sky is open,
        // whatever the column.
        let mut open_from = [0; 9];
        for (slot, open) in open_from.iter_mut().enumerate() {
            let sections = &self.occupied[slot * self.sections..(slot + 1) * self.sections];
            if let Some(highest) = sections.iter().rposition(|occupied| *occupied) {
                *open = (highest + 2) * SECTION_EDGE;
            }
        }
        let open_everywhere = open_from.iter().copied().max().unwrap_or(0);

        // For each column of a chunk that is there, the lowest block that the sky
        // reaches straight down.
        const NO_SKY: u16 = u16::MAX;
        let mut lowest = vec![NO_SKY; LAYER];
        let mut levels = vec![0; self.kind.len()];
        levels[cell(0, open_everywhere, 0)..cell(0, height, 0)].fill(15);
        for (slot, open) in open_from.into_iter().enumerate() {
            if !self.present[slot] {
                continue;
            }
            for z in 0..SECTION_EDGE {
                for x in 0..SECTION_EDGE {
                    let grid_x = slot % 3 * SECTION_EDGE + 1 + x;
                    let grid_z = slot / 3 * SECTION_EDGE + 1 + z;
                    let mut y = open;
                    let mut above = blocks::AIR.0;
                    let mut above_shaped = false;
                    while y > 0 {
                        let below = cell(grid_x, y - 1, grid_z);
                        let kind = self.kind[below];
                        if kind & CLEAR == 0 {
                            break;
                        }
                        let shaped = kind & SHAPED != 0;
                        if (shaped || above_shaped)
                            && faces::stop_light(
                                BlockState(above),
                                BlockState(self.state[below]),
                                Face::Down,
                            )
                        {
                            break;
                        }
                        y -= 1;
                        above = self.state[below];
                        above_shaped = shaped;
                    }
                    lowest[grid_z * WIDTH + grid_x] = y as u16;
                    for lit in y..open_everywhere {
                        levels[cell(grid_x, lit, grid_z)] = 15;
                    }
                }
            }
        }

        // The sky's light spreads from where a column of it has something beside or
        // below it that the sky does not reach.
        let mut waiting: [Vec<u32>; 16] = Default::default();
        for column in 0..LAYER {
            let from = lowest[column];
            if from == NO_SKY || self.distance[column] >= 15 {
                continue;
            }
            let from = usize::from(from);
            let beside = [column - 1, column + 1, column - WIDTH, column + WIDTH]
                .into_iter()
                .map(|other| lowest[other])
                .filter(|other| *other != NO_SKY)
                .max()
                .map_or(0, usize::from);
            let to = if from > 0 {
                beside.max(from + 1)
            } else {
                beside
            };
            for y in from..to {
                waiting[15].push(cell(column % WIDTH, y, column / WIDTH) as u32);
            }
        }
        self.spread(&mut levels, &mut waiting);
        levels
    }

    /// Whether a light section of the centre chunk, counted from the one below the
    /// world, has light of its own: it or one of the 26 around it holds a block.
    fn has_light(&self, light_section: usize) -> bool {
        // The light section is the world's section `light_section - 1`.
        let first = light_section.saturating_sub(2);
        let last = light_section.min(self.sections - 1);
        (0..9)
            .any(|slot| (first..=last).any(|section| self.occupied[slot * self.sections + section]))
    }

    /// The cases of the centre chunk's light sections for the levels of the grid.
    fn sections_of(&self, levels: &[u8]) -> Vec<SectionLight> {
        (0..self.sections + 2)
            .map(|section| {
                if !self.has_light(section) {
                    return SectionLight::Absent;
                }
                let mut array = Box::new([0; LIGHT_ARRAY_LENGTH]);
                for y in 0..SECTION_EDGE {
                    for z in 0..SECTION_EDGE {
                        let row = cell(CENTRE, section * SECTION_EDGE + y, CENTRE + z);
                        for pair in 0..SECTION_EDGE / 2 {
                            array[(y << 8 | z << 4) / 2 + pair] =
                                levels[row + 2 * pair] | levels[row + 2 * pair + 1] << 4;
                        }
                    }
                }
                SectionLight::from_array(array)
            })
            .collect()
    }
}

/// How far a grid coordinate lies outside the centre chunk.
fn beyond_centre(at: usize) -> u8 {
    let last = CENTRE + SECTION_EDGE - 1;
    (CENTRE.saturating_sub(at) + at.saturating_sub(last)) as u8
}

/// The offsets of the nine chunks, north-west first, row by row.
fn chunk_offsets() -> impl Iterator<Item = (i32, i32)> {
    (-1..=1).flat_map(|chunk_z| (-1..=1).map(move |chunk_x| (chunk_x, chunk_z)))
}

pub(crate) fn light<B: Blocks + ?Sized>(blocks: &B) -> ChunkLight {
    let grid = Grid::read(blocks);
    let sky = if blocks.has_sky() {
        grid.sections_of(&grid.sky_light())
    } else {
        vec![SectionLight::Absent; grid.sections + 2]
    };
    let block = grid.sections_of(&grid.block_light());
    ChunkLight { sky, block }
}
