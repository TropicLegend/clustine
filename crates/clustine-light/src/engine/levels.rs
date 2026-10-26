//! What the engine keeps of a present chunk (the plan's 3.3): the levels of every light
//! section, the sky's and the blocks', and for each column the lowest block the sky
//! reaches at 15; and what it needs of a block, read once for every state.

use std::sync::OnceLock;

use clustine_data::{BLOCK_STATE_COUNT, BlockState, Face, blocks};
use clustine_world::{Chunk, LIGHT_ARRAY_LENGTH, Light, SectionLight};

use super::pass::Shape;
use crate::faces;
use crate::flood::{CLEAR, Cell, SHAPED};

/// The levels of one light section, 4 bits a block in the order `y << 8 | z << 4 | x`.
/// A section of one level holds no array, so that the sky above everything and the dark
/// under the ground cost a byte.
#[derive(Clone)]
pub(super) enum Levels {
    Uniform(u8),
    Nibbles(Box<[u8; LIGHT_ARRAY_LENGTH]>),
}

impl Levels {
    #[inline]
    pub(super) fn get(&self, index: usize) -> u8 {
        match self {
            Levels::Uniform(level) => *level,
            Levels::Nibbles(levels) => levels[index >> 1] >> ((index & 1) << 2) & 15,
        }
    }

    #[inline]
    pub(super) fn set(&mut self, index: usize, level: u8) {
        if let Levels::Uniform(all) = *self {
            if all == level {
                return;
            }
            *self = Levels::Nibbles(Box::new([all | all << 4; LIGHT_ARRAY_LENGTH]));
        }
        let Levels::Nibbles(levels) = self else {
            unreachable!("a uniform section was given an array above");
        };
        let shift = (index & 1) << 2;
        let byte = &mut levels[index >> 1];
        *byte = *byte & !(15 << shift) | level << shift;
    }

    /// The levels of a section as a [`Light`] keeps them; a section left out is taken as
    /// `absent` everywhere.
    fn of(section: SectionLight, absent: u8) -> Levels {
        match section {
            SectionLight::Absent => Levels::Uniform(absent),
            SectionLight::Dark => Levels::Uniform(0),
            SectionLight::Full => Levels::Uniform(15),
            SectionLight::Levels(levels) => Levels::Nibbles(levels),
        }
    }

    /// The section in the one form equal light has.
    pub(super) fn section(&self) -> SectionLight {
        match self {
            Levels::Uniform(0) => SectionLight::Dark,
            Levels::Uniform(15) => SectionLight::Full,
            Levels::Uniform(level) => {
                SectionLight::of(Box::new([*level | *level << 4; LIGHT_ARRAY_LENGTH]))
            }
            Levels::Nibbles(levels) => SectionLight::of(levels.clone()),
        }
    }

    /// Drops the array of a section that is dark or full all through.
    fn compact(&mut self) {
        if let Levels::Nibbles(levels) = self {
            let first = levels[0];
            if (first == 0 || first == 0xff) && levels.iter().all(|byte| *byte == first) {
                *self = Levels::Uniform(first & 15);
            }
        }
    }
}

/// The levels of a chunk that is part of the fixed point.
pub(super) struct Lit {
    /// For the sky ([`super::pass::SKY`]) and for blocks ([`super::pass::BLOCK`]), an entry for the section below
    /// the chunk, one for each section and one for the section above. The sky's is
    /// empty in a dimension without one.
    pub(super) layers: [Vec<Levels>; 2],
    /// For each column (`z << 4 | x`): the lowest cell the sky reaches straight down,
    /// counted from the bottom of the section below the chunk. Every cell from there up
    /// is 15 and is a source; every cell below is at most 14.
    pub(super) lowest: Box<[u16; 256]>,
}

impl Lit {
    /// The levels of a chunk about to be lit whole: the sky's columns at 15 and nothing
    /// else, which the arrival then spreads.
    pub(super) fn arriving(chunk: &Chunk, sky: bool, shape: Shape) -> Lit {
        let count = shape.sections + 2;
        let mut lowest = Box::new([0u16; 256]);
        let mut sky_levels = Vec::new();
        if sky {
            let open = open_from(chunk);
            for (column, lowest) in lowest.iter_mut().enumerate() {
                *lowest = lowest_in(chunk, column & 15, column >> 4, open);
            }
            let least = usize::from(*lowest.iter().min().expect("a chunk has columns"));
            let most = usize::from(*lowest.iter().max().expect("a chunk has columns"));
            for section in 0..count {
                let bottom = section * 16;
                if bottom >= most {
                    sky_levels.push(Levels::Uniform(15));
                } else if bottom + 16 <= least {
                    sky_levels.push(Levels::Uniform(0));
                } else {
                    let mut levels = Levels::Uniform(0);
                    for (column, lowest) in lowest.iter().enumerate() {
                        for y in usize::from(*lowest).max(bottom)..bottom + 16 {
                            levels.set((y - bottom) << 8 | column, 15);
                        }
                    }
                    sky_levels.push(levels);
                }
            }
        }
        Lit {
            layers: [sky_levels, vec![Levels::Uniform(0); count]],
            lowest,
        }
    }

    /// The levels kept by a run elsewhere. The sky's columns are read off the levels:
    /// only a source is 15.
    pub(super) fn adopted(light: Light, sky: bool, shape: Shape) -> Lit {
        let sky_levels: Vec<Levels> = if sky {
            light
                .sky
                .into_iter()
                .map(|section| Levels::of(section, 15))
                .collect()
        } else {
            Vec::new()
        };
        let block = light
            .block
            .into_iter()
            .map(|section| Levels::of(section, 0))
            .collect();
        let mut lowest = Box::new([0u16; 256]);
        if sky {
            for (column, lowest) in lowest.iter_mut().enumerate() {
                *lowest = lowest_kept(&sky_levels, column, shape);
            }
        }
        Lit {
            layers: [sky_levels, block],
            lowest,
        }
    }

    pub(super) fn compact(&mut self) {
        for levels in self.layers.iter_mut().flatten() {
            levels.compact();
        }
    }
}

/// The lowest cell of a column from which up every level is 15.
fn lowest_kept(levels: &[Levels], column: usize, shape: Shape) -> u16 {
    for section in (0..shape.sections + 2).rev() {
        match &levels[section] {
            Levels::Uniform(15) => continue,
            Levels::Uniform(_) => return (section as u16 + 1) * 16,
            Levels::Nibbles(_) => {
                for y in (0..16).rev() {
                    if levels[section].get(y << 8 | column) != 15 {
                        return (section * 16 + y + 1) as u16;
                    }
                }
            }
        }
    }
    0
}

/// What light needs of every block state, read once.
pub(super) fn cell(state: BlockState) -> Cell {
    static CELLS: OnceLock<Box<[Cell]>> = OnceLock::new();
    let cells = CELLS.get_or_init(|| {
        (0..BLOCK_STATE_COUNT)
            .map(|id| Cell::of(BlockState(id as u16)))
            .collect()
    });
    match cells.get(usize::from(state.0)) {
        Some(cell) => *cell,
        None => Cell::of(state),
    }
}

/// The state at a cell of `chunk`, `y` counted from the bottom of the section below it:
/// air in that section and in the one above.
#[inline]
pub(super) fn state_at(chunk: &Chunk, x: usize, y: usize, z: usize) -> BlockState {
    let section = y >> 4;
    match section
        .checked_sub(1)
        .and_then(|index| chunk.sections().get(index))
    {
        Some(section) => section.get(x, y & 15, z),
        None => blocks::AIR,
    }
}

/// The cell above the highest section of `chunk` that holds a block: the sky is open
/// from there up in every column (`flood.rs`'s `sky_light`).
pub(super) fn open_from(chunk: &Chunk) -> usize {
    chunk
        .sections()
        .iter()
        .rposition(|section| section.non_air_count() > 0)
        .map_or(0, |highest| (highest + 2) * 16)
}

/// The lowest cell of a column that the sky reaches straight down from `open`: down
/// through blocks that dampen nothing, as long as the two faces that meet on the way
/// do not stop light (`flood.rs`'s `sky_light`, the same rule).
pub(super) fn lowest_in(chunk: &Chunk, x: usize, z: usize, open: usize) -> u16 {
    let mut y = open;
    let mut above = blocks::AIR;
    let mut above_shaped = false;
    while y > 0 {
        // A whole section of one clear state without faces is passed at once.
        if y & 15 == 0 && !above_shaped {
            let index = (y >> 4) - 1;
            let uniform = match index.checked_sub(1) {
                Some(index) => chunk.sections()[index].uniform_state(),
                None => Some(blocks::AIR),
            };
            if let Some(state) = uniform {
                let of = cell(state);
                if of.kind & CLEAR != 0 && of.kind & SHAPED == 0 {
                    y -= 16;
                    above = state;
                    continue;
                }
            }
        }
        let below = state_at(chunk, x, y - 1, z);
        let kind = cell(below).kind;
        if kind & CLEAR == 0 {
            break;
        }
        let shaped = kind & SHAPED != 0;
        if (shaped || above_shaped) && faces::stop_light(above, below, Face::Down) {
            break;
        }
        y -= 1;
        above = below;
        above_shaped = shaped;
    }
    y as u16
}

/// The levels of a layer, for the tests of this module.
#[cfg(test)]
pub(super) fn level_of(lit: &Lit, layer: usize, y: usize, column: usize) -> u8 {
    lit.layers[layer][y >> 4].get((y & 15) << 8 | column)
}

#[cfg(test)]
mod tests {
    use clustine_world::{Biome, Section};

    use super::super::pass::{BLOCK, SKY};
    use super::*;

    #[test]
    fn a_column_open_to_the_bottom_is_a_source_all_the_way_down() {
        let chunk = Chunk::from_sections(0, vec![Section::filled(blocks::AIR, Biome(0)); 2]);
        let shape = Shape {
            min_y: 0,
            sections: 2,
        };
        let lit = Lit::arriving(&chunk, true, shape);
        assert!(lit.lowest.iter().all(|lowest| *lowest == 0));
        assert!(
            lit.layers[SKY]
                .iter()
                .all(|levels| matches!(levels, Levels::Uniform(15)))
        );
        let kept = lowest_kept(&lit.layers[SKY], 7, shape);
        assert_eq!(kept, 0);
    }

    #[test]
    fn the_sky_stops_above_a_block_that_dampens_and_on_a_slab() {
        let mut chunk = Chunk::from_sections(0, vec![Section::filled(blocks::AIR, Biome(0)); 2]);
        chunk.set(0, 5, 0, blocks::STONE);
        chunk.set(
            1,
            9,
            0,
            BlockState::parse("minecraft:oak_slab[type=bottom]").unwrap(),
        );
        chunk.set(2, 3, 0, blocks::GLASS);
        let shape = Shape {
            min_y: 0,
            sections: 2,
        };
        let lit = Lit::arriving(&chunk, true, shape);
        // Cells count from the bottom of the section below the chunk.
        assert_eq!(lit.lowest[0], 16 + 6);
        assert_eq!(lit.lowest[1], 16 + 9);
        assert_eq!(lit.lowest[2], 0);
        assert_eq!(level_of(&lit, SKY, 16 + 6, 0), 15);
        assert_eq!(level_of(&lit, SKY, 16 + 5, 0), 0);
        for column in 0..3 {
            assert_eq!(
                lowest_kept(&lit.layers[SKY], column, shape),
                lit.lowest[column]
            );
        }
        assert!(
            lit.layers[BLOCK]
                .iter()
                .all(|l| matches!(l, Levels::Uniform(0)))
        );
    }
}
