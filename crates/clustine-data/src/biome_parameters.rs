//! Which climate is which biome: the game's two parameter lists, of the overworld and
//! of the Nether.
//!
//! They are in the game's code and in none of its data files, so `cargo datagen` asks
//! the game for them (ADR-0019, section 5) and packs them into
//! `generated/biome_parameters.bin`: one section for each list, the rows in the list's
//! own order, which decides between two entries that fit a climate equally well.
//!
//! ADR-0019 puts this table into the crate of world-generation data, which does not
//! exist yet; it is kept here until that crate is made and moves there unchanged.

use crate::packed::{self, Table};

/// The layout of the table this module reads.
pub const LAYOUT: u16 = 1;

/// The bytes of a row: twelve bounds and the offset as `i16`, and the biome as `u8`.
pub const ROW_BYTES: usize = 27;

static TABLE: Table<2> = Table::parse(
    include_bytes!("generated/biome_parameters.bin"),
    packed::KIND_BIOME_PARAMETERS,
    LAYOUT,
    [ROW_BYTES, ROW_BYTES],
);

/// One of the game's two parameter lists; the number is its section in the table.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum BiomeParameterList {
    Overworld = 0,
    Nether = 1,
}

impl BiomeParameterList {
    /// The number of entries of the list.
    pub fn len(self) -> usize {
        TABLE.rows(self as usize)
    }

    /// Whether the list has no entries; neither of the game's is empty.
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// Entry `index` of the list. Panics if there is none.
    pub fn get(self, index: usize) -> BiomeParameters {
        let row = TABLE.row(self as usize, index);
        let mut entry = BiomeParameters {
            bounds: [[0; 2]; 6],
            offset: packed::row_i16(row, 24),
            biome: row[26],
        };
        for (parameter, bounds) in entry.bounds.iter_mut().enumerate() {
            *bounds = [
                packed::row_i16(row, 4 * parameter),
                packed::row_i16(row, 4 * parameter + 2),
            ];
        }
        entry
    }

    /// The entries in the list's own order.
    pub fn iter(self) -> impl Iterator<Item = BiomeParameters> {
        (0..self.len()).map(move |index| self.get(index))
    }
}

/// One entry of a parameter list: the climate a biome is found in.
///
/// The numbers are the integers the game computes with, ten thousand to one unit of a
/// noise's value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BiomeParameters {
    /// The lower and the upper bound of temperature, humidity, continentalness,
    /// erosion, depth and weirdness, in that order.
    pub bounds: [[i16; 2]; 6],
    pub offset: i16,
    /// The biome's id in the registry `minecraft:worldgen/biome` of
    /// [`crate::SYNCED_REGISTRIES`], which is sorted by name.
    pub biome: u8,
}
