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
use clustine_world::{Chunk, LIGHT_ARRAY_LENGTH, Light, SectionLight};

use crate::faces;
use crate::{Around, SECTION_EDGE, Unlisted, UnlistedLevels, VERSION};

/// Cells along a horizontal edge of the grid: three chunks and a wall on either side,
/// so that no step leaves the grid.
const WIDTH: usize = 3 * SECTION_EDGE + 2;
/// Cells of one horizontal layer.
const LAYER: usize = WIDTH * WIDTH;
/// The grid's coordinate of the centre chunk's first block, along x and along z.
const CENTRE: usize = SECTION_EDGE + 1;

/// The low bits of a cell's kind: what the block takes from light that enters, 1 to 15.
pub(crate) const TAKES: u8 = 0x0f;
/// The block dampens by nothing, so the sky may come straight down through it.
pub(crate) const CLEAR: u8 = 0x20;
/// The block has faces that stop light by their shape.
pub(crate) const SHAPED: u8 = 0x40;

/// A cell nothing enters: a chunk that is not there, and the rim of the grid.
const WALL: u8 = 15;

/// The highest level there is: the sky's, and what the brightest block gives.
const BRIGHTEST: u8 = 15;

/// For each column of the grid: how many steps sideways lie between it and the centre
/// chunk. Light of a level no higher than that never arrives there, since a level falls
/// by at least one a step; so nothing that is [`BRIGHTEST`] steps away or more has any
/// part in the centre chunk's light.
static DISTANCE: [u8; LAYER] = {
    let mut distance = [0; LAYER];
    let mut column = 0;
    while column < LAYER {
        distance[column] = beyond_centre(column % WIDTH) + beyond_centre(column / WIDTH);
        column += 1;
    }
    distance
};

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
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Cell {
    pub(crate) kind: u8,
    pub(crate) gives: u8,
}

impl Cell {
    pub(crate) fn of(state: BlockState) -> Cell {
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
        }
    }
}

/// The blocks of the nine chunks as light sees them, with one section of air below and
/// one above the world, inside a rim of walls.
///
/// It is part of a [`Scratch`], and [`Grid::read`] writes every field anew.
#[derive(Default)]
struct Grid {
    /// Sections of the world, without the two beyond it.
    sections: usize,
    kind: Vec<u8>,
    /// The state of each cell of a chunk that is there. The cells of the rim and of a
    /// chunk that is not there hold whatever they held: a wall is never entered, never
    /// spread from and never under the sky, so nobody asks what it is.
    state: Vec<u16>,
    /// The cells that give light, with their level, in the order they were read.
    emitters: Vec<(u32, u8)>,
    /// For each of the nine chunks, north-west first: whether it is there.
    present: [bool; 9],
    /// For each chunk and each of its sections: whether it holds a block that is not air.
    occupied: Vec<bool>,
}

/// What a thread keeps between two calls of [`light_with`](crate::light_with), so that
/// a call allocates nothing large: the grid of the nine chunks as light sees them, the
/// levels of its cells and the lists of cells the flood has yet to spread from. For a
/// world of 24 sections that is 4.2 MB once it has been used.
///
/// A call assumes nothing of what an earlier call left in it: everything it reads it
/// has written first. So a scratch may go from a chunk to any other, of any height,
/// and may be used again after a call that panicked half way. A new one holds nothing
/// and costs nothing until its first call.
#[derive(Default)]
pub struct Scratch {
    grid: Grid,
    /// One level for each cell: the sky's while that is made, then the blocks'.
    levels: Vec<u8>,
    /// See [`Grid::spread`].
    waiting: [Vec<u32>; 16],
}

impl Scratch {
    /// A scratch that holds nothing yet.
    pub fn new() -> Scratch {
        Scratch::default()
    }
}

/// The cell of a block: `x` and `z` in the grid, `y` counted from the bottom of the
/// section below the world.
fn cell(x: usize, y: usize, z: usize) -> usize {
    ((y + 1) * WIDTH + z) * WIDTH + x
}

impl Grid {
    /// Makes the grid that of these nine chunks, whatever it held.
    fn read(&mut self, around: &Around<'_>) {
        let grid = self;
        let centre = around.centre;
        let sections = centre.sections().len();
        assert!(sections > 0, "a chunk has a section");
        let cells = ((sections + 2) * SECTION_EDGE + 2) * LAYER;
        grid.sections = sections;
        // Every kind is written here, and again below with its state if a chunk is
        // there: what an earlier call left, whole or half way, is gone or is behind a
        // wall. The states are not all written twice: that was a millisecond of the
        // sixteen a call takes in a build without optimisation.
        refill(&mut grid.kind, cells, WALL);
        if grid.state.len() != cells {
            grid.state = vec![blocks::AIR.0; cells];
        }
        refill(&mut grid.occupied, 9 * sections, false);
        grid.emitters.clear();
        grid.present = [false; 9];
        let air = Cell::of(blocks::AIR);
        for (slot, chunk) in nine(around).into_iter().enumerate() {
            let Some(chunk) = chunk else {
                continue;
            };
            assert!(
                chunk.min_y() == centre.min_y() && chunk.sections().len() == sections,
                "the chunks of a dimension span the same heights"
            );
            grid.present[slot] = true;
            let first_x = slot % 3 * SECTION_EDGE + 1;
            let first_z = slot / 3 * SECTION_EDGE + 1;
            grid.fill(first_x, 0, first_z, blocks::AIR, air);
            grid.fill(first_x, sections + 1, first_z, blocks::AIR, air);
            for (index, section) in chunk.sections().iter().enumerate() {
                // The section's own count is by the table's `is_air`, which is what
                // "holds a block" means in the rule for which sections have light.
                grid.occupied[slot * sections + index] = section.non_air_count() > 0;
                if let Some(state) = section.uniform_state() {
                    grid.fill(first_x, index + 1, first_z, state, Cell::of(state));
                    continue;
                }
                // Stone with its ores is one thing to light. Where every state the
                // section may hold is, and has no faces of its own, any of them stands
                // for all: only a block with such faces is ever asked which state it is.
                let any = section.get(0, 0, 0);
                let of = Cell::of(any);
                if of.kind & SHAPED == 0 && !section.may_hold(|state| Cell::of(state) != of) {
                    grid.fill(first_x, index + 1, first_z, any, of);
                    continue;
                }
                // Most positions hold what the one before held, so what light needs of
                // a state is looked up once for each run of it.
                let mut last = blocks::AIR;
                let mut of = air;
                let first = cell(first_x, (index + 1) * SECTION_EDGE, first_z);
                for (position, state) in section.states().enumerate() {
                    if state != last {
                        last = state;
                        of = Cell::of(state);
                    }
                    // A section's order is `y << 8 | z << 4 | x`.
                    let at = first
                        + (position >> 8) * LAYER
                        + (position >> 4 & 15) * WIDTH
                        + (position & 15);
                    grid.kind[at] = of.kind;
                    grid.state[at] = state.0;
                    if of.gives > 0 {
                        grid.emitters.push((at as u32, of.gives));
                    }
                }
            }
        }
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
        for level in (2..=BRIGHTEST).rev() {
            // A cell is only ever put into a list further down, so this one is
            // complete. It is handed back empty, to keep what it has allocated.
            let mut cells = std::mem::take(&mut waiting[usize::from(level)]);
            for from in &cells {
                let from = *from as usize;
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
                    if arrives <= DISTANCE[to % LAYER] {
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
            cells.clear();
            waiting[usize::from(level)] = cells;
        }
    }

    /// Makes `levels` the block light of every cell that matters to the centre chunk.
    fn block_light(&self, levels: &mut Vec<u8>, waiting: &mut [Vec<u32>; 16]) {
        self.begin(levels, waiting);
        for (at, gives) in &self.emitters {
            let column = *at as usize % LAYER;
            if *gives > DISTANCE[column] {
                levels[*at as usize] = *gives;
                waiting[usize::from(*gives)].push(*at);
            }
        }
        self.spread(levels, waiting);
    }

    /// Darkness everywhere and nothing waiting, whatever the two held: the lists of
    /// the lowest levels are never worked off, and a call that panicked left the others
    /// as they were.
    fn begin(&self, levels: &mut Vec<u8>, waiting: &mut [Vec<u32>; 16]) {
        refill(levels, self.kind.len(), 0);
        for cells in waiting.iter_mut() {
            cells.clear();
        }
    }

    /// Makes `levels` the sky light of every cell that matters to the centre chunk.
    fn sky_light(&self, levels: &mut Vec<u8>, waiting: &mut [Vec<u32>; 16]) {
        self.begin(levels, waiting);
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
        let mut lowest = [NO_SKY; LAYER];
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
        for column in 0..LAYER {
            let from = lowest[column];
            if from == NO_SKY || DISTANCE[column] >= BRIGHTEST {
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
        self.spread(levels, waiting);
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
    ///
    /// A section without light of its own is left out, as the official server leaves
    /// it out. If `also` names it, the case of the levels it has all the same is put
    /// into `unlisted`, with its number.
    fn sections_of(
        &self,
        levels: &[u8],
        also: u64,
        unlisted: &mut Vec<(usize, SectionLight)>,
    ) -> Vec<SectionLight> {
        (0..self.sections + 2)
            .map(|section| {
                let listed = self.has_light(section);
                let asked = section < Unlisted::SECTIONS && also >> section & 1 == 1;
                if !listed && !asked {
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
                let light = SectionLight::of(array);
                if listed {
                    return light;
                }
                unlisted.push((section, light));
                SectionLight::Absent
            })
            .collect()
    }
}

/// Makes `cells` hold `length` times `value` and nothing else, in the memory it has if
/// it is of that length already: a thread lights chunks of one height.
///
/// Otherwise the memory is asked for anew, which costs a first call nothing where the
/// value is zero. `Vec::resize` would do for both, and is an element at a time in a
/// build without optimisation: it doubled the time of a call there.
fn refill<T: Copy>(cells: &mut Vec<T>, length: usize, value: T) {
    if cells.len() == length {
        cells.fill(value);
    } else {
        *cells = vec![value; length];
    }
}

/// How far a grid coordinate lies outside the centre chunk.
const fn beyond_centre(at: usize) -> u8 {
    let last = CENTRE + SECTION_EDGE - 1;
    (CENTRE.saturating_sub(at) + at.saturating_sub(last)) as u8
}

/// The offsets in chunks, along x and along z, of the eight neighbours in the order of
/// [`Around::neighbours`].
const NEIGHBOURS: [(isize, isize); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

pub(crate) fn reach(x: usize, z: usize) -> [bool; 8] {
    assert!(
        x < SECTION_EDGE && z < SECTION_EDGE,
        "a column of a chunk is within it"
    );
    NEIGHBOURS.map(|(chunk_x, chunk_z)| {
        // The column as the neighbour's own flood has it, with the neighbour in the
        // middle of the grid: this chunk is one chunk the other way from there.
        let grid_x = (CENTRE + x).wrapping_add_signed(-chunk_x * SECTION_EDGE as isize);
        let grid_z = (CENTRE + z).wrapping_add_signed(-chunk_z * SECTION_EDGE as isize);
        // What that flood does with the brightest source there is: it starts from it,
        // and passes over the column otherwise.
        BRIGHTEST > DISTANCE[grid_z * WIDTH + grid_x]
    })
}

/// The nine chunks by their place in the grid: north-west first, row by row, the centre
/// in the middle. North is the lower `z`, as the grid's rows are.
fn nine<'a>(around: &Around<'a>) -> [Option<&'a Chunk>; 9] {
    let [
        north_west,
        north,
        north_east,
        west,
        east,
        south_west,
        south,
        south_east,
    ] = around.neighbours;
    [
        north_west,
        north,
        north_east,
        west,
        Some(around.centre),
        east,
        south_west,
        south,
        south_east,
    ]
}

pub(crate) fn light(
    around: &Around<'_>,
    also: Unlisted,
    scratch: &mut Scratch,
) -> (Light, UnlistedLevels) {
    let Scratch {
        grid,
        levels,
        waiting,
    } = scratch;
    grid.read(around);
    let mut unlisted = UnlistedLevels::default();
    let sky = if around.sky {
        grid.sky_light(levels, waiting);
        grid.sections_of(levels, also.sky, &mut unlisted.sky)
    } else {
        vec![SectionLight::Absent; grid.sections + 2]
    };
    grid.block_light(levels, waiting);
    let block = grid.sections_of(levels, also.block, &mut unlisted.block);
    let light = Light {
        made_by: VERSION,
        // Whoever lights a chunk it has generated says which generator's chunks
        // these were; the rules do not know.
        beside: 0,
        sky,
        block,
    };
    (light, unlisted)
}

#[cfg(test)]
mod tests {
    use clustine_world::{Biome, Section};

    use super::*;

    fn chunk(height: usize, seed: u32) -> Chunk {
        let sections = vec![Section::filled(blocks::AIR, Biome(0)); height / SECTION_EDGE];
        let mut chunk = Chunk::from_sections(0, sections);
        let mut number = seed;
        for y in 0..height {
            for z in 0..SECTION_EDGE {
                for x in 0..SECTION_EDGE {
                    number = number.wrapping_mul(1664525).wrapping_add(1013904223);
                    let state = match number >> 24 {
                        0..120 => blocks::AIR,
                        120..230 => blocks::STONE,
                        230..250 => blocks::WATER,
                        250..254 => blocks::TORCH,
                        _ => blocks::GLOWSTONE,
                    };
                    chunk.set(x, y as i32, z, state);
                }
            }
        }
        chunk
    }

    /// The chunk with itself to the north and to the south-east and nothing elsewhere.
    fn alone(centre: &Chunk) -> Around<'_> {
        let mut neighbours = [None; 8];
        neighbours[1] = Some(centre);
        neighbours[7] = Some(centre);
        Around {
            centre,
            neighbours,
            sky: true,
        }
    }

    fn cells_of(height: usize) -> usize {
        (height + 2 * SECTION_EDGE + 2) * LAYER
    }

    #[test]
    fn a_scratch_full_of_anything_gives_the_light_a_new_one_gives() {
        let all = Unlisted { sky: !0, block: !0 };
        for height in [16, 48, 96] {
            let centre = chunk(height, 5);
            let expected = light(&alone(&centre), all, &mut Scratch::new());
            // Every field as wrong as it can be: lengths that fit no chunk and that
            // fit this one, cells that claim to be clear and shaped and hold a state
            // that does not exist, so that asking what it is panics; levels that are
            // full, and every list of waiting cells holding cells that exist.
            for cells in [7, cells_of(height), 3 * cells_of(height)] {
                let mut scratch = Scratch {
                    grid: Grid {
                        sections: 1000,
                        kind: vec![CLEAR | SHAPED | 1; cells],
                        state: vec![u16::MAX; cells],
                        emitters: (0..cells as u32).map(|at| (at, 15)).collect(),
                        present: [true; 9],
                        occupied: vec![true; cells],
                    },
                    levels: vec![15; cells],
                    waiting: std::array::from_fn(|_| (0..cells as u32).collect()),
                };
                let made = light(&alone(&centre), all, &mut scratch);
                assert!(made == expected, "height {height}, {cells} cells");
            }
        }
    }

    #[test]
    fn a_scratch_keeps_its_memory_from_one_call_to_the_next() {
        let centre = chunk(48, 9);
        let mut scratch = Scratch::new();
        light(&alone(&centre), Unlisted::NONE, &mut scratch);
        let held = |scratch: &Scratch| {
            (
                scratch.grid.kind.as_ptr(),
                scratch.grid.state.as_ptr(),
                scratch.levels.as_ptr(),
                scratch.waiting.each_ref().map(|cells| cells.capacity()),
            )
        };
        let before = held(&scratch);
        light(&alone(&centre), Unlisted::NONE, &mut scratch);
        assert!(before == held(&scratch));
    }

    #[test]
    fn the_distance_of_a_column_is_its_steps_to_the_centre_chunk() {
        let column = |x: usize, z: usize| usize::from(DISTANCE[z * WIDTH + x]);
        assert_eq!(column(CENTRE, CENTRE), 0);
        assert_eq!(column(CENTRE + 15, CENTRE + 15), 0);
        assert_eq!(column(CENTRE - 1, CENTRE), 1);
        assert_eq!(column(CENTRE + 16, CENTRE + 16), 2);
        assert_eq!(column(1, CENTRE + 3), 16);
        assert_eq!(column(0, 0), 34);
    }
}
