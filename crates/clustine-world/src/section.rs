//! A 16×16×16 cube of blocks.

use clustine_data::BlockState;
use serde::{Deserialize, Serialize};

use crate::{BLOCKS_PER_SECTION, SECTION_SIZE};

/// A biome, as its id in the biome registry Clustine sends to clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Biome(pub u16);

/// Biome cells in a section: a biome is said for every cube of four blocks.
pub const BIOME_CELLS: usize = 64;

/// The narrowest and the widest index of a palette. They are the client's, so that a
/// section's blocks can be copied into its packet as they are.
const NARROWEST: u8 = 4;
const WIDEST: u8 = 8;
/// The width of a state's id where a section has more states than a palette holds:
/// what 26.3's 35,723 states need.
const DIRECT_BITS: u8 = 16;

/// The blocks of a section, in one of the three forms of the client's container
/// (`docs/adr/0022-what-a-chunk-carries.md`, section 2). Most sections of a world are
/// all air or all stone, and a section of one block state does not allocate; nearly
/// all others hold at most sixteen states, and take 2 KB where a state for each
/// position took 8.
#[derive(Debug, Clone)]
enum Blocks {
    Uniform(BlockState),
    /// A palette, in the order the states were first put there, and for each position
    /// an index into it of `bits` bits. The indices are packed into 64-bit words,
    /// lowest bits first, none across two words, in the order `y << 8 | z << 4 | x`.
    /// A palette may hold states that no position has any more.
    Indexed {
        bits: u8,
        palette: Vec<BlockState>,
        words: Box<[u64]>,
    },
    /// The id of the state of each position, packed like indices.
    Direct(Box<[u64]>),
}

/// How many values of `bits` bits a word holds.
const fn per_word(bits: u8) -> usize {
    64 / bits as usize
}

/// How many words hold a value of `bits` bits for each position of a section.
const fn words_for(bits: u8) -> usize {
    BLOCKS_PER_SECTION.div_ceil(per_word(bits))
}

fn read(words: &[u64], bits: u8, index: usize) -> u16 {
    let per_word = per_word(bits);
    let shift = (index % per_word) * usize::from(bits);
    ((words[index / per_word] >> shift) & ((1 << bits) - 1)) as u16
}

fn write(words: &mut [u64], bits: u8, index: usize, value: u16) {
    let per_word = per_word(bits);
    let shift = (index % per_word) * usize::from(bits);
    let word = &mut words[index / per_word];
    *word = *word & !(((1 << bits) - 1) << shift) | u64::from(value) << shift;
}

/// Words that hold `values` at `bits` bits each.
fn packed(bits: u8, values: impl Iterator<Item = u16>) -> Box<[u64]> {
    let mut words = vec![0; words_for(bits)].into_boxed_slice();
    for (index, value) in values.enumerate() {
        write(&mut words, bits, index, value);
    }
    words
}

/// The width of an index into a palette of `states` states, or `None` if a palette
/// does not hold that many.
fn width_for(states: usize) -> Option<u8> {
    (NARROWEST..=WIDEST).find(|bits| states <= 1 << bits)
}

impl Blocks {
    fn get(&self, index: usize) -> BlockState {
        match self {
            Self::Uniform(state) => *state,
            Self::Indexed {
                bits,
                palette,
                words,
            } => palette[usize::from(read(words, *bits, index))],
            Self::Direct(words) => BlockState(read(words, DIRECT_BITS, index)),
        }
    }

    /// The blocks in the narrowest form that holds `palette`, whose states are in the
    /// order they are first met in `states`.
    fn of(palette: Vec<BlockState>, states: impl Iterator<Item = BlockState>) -> Self {
        let Some(bits) = width_for(palette.len()) else {
            return Self::Direct(packed(DIRECT_BITS, states.map(|state| state.0)));
        };
        // Most positions hold what the one before held.
        let mut last = (palette[0], 0);
        let indices = states.map(|state| {
            if state != last.0 {
                let found = palette.iter().position(|known| *known == state);
                last = (state, found.expect("the palette has every state") as u16);
            }
            last.1
        });
        let words = packed(bits, indices);
        Self::Indexed {
            bits,
            palette,
            words,
        }
    }
}

/// The biomes of a section: one for all of it, or one for each cell of four blocks,
/// indexed by `y << 4 | z << 2 | x`, which is the client's order.
#[derive(Debug, Clone)]
enum Biomes {
    One(Biome),
    Cells(Box<[Biome; BIOME_CELLS]>),
}

/// A 16×16×16 cube of blocks, with the biome of each of its 64 cells.
///
/// Two sections are equal if they hold the same blocks and biomes, however they came
/// to: a section that has held many states and holds two again equals a fresh one.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "SectionRepr", into = "SectionRepr")]
pub struct Section {
    blocks: Blocks,
    /// Number of blocks that are not air, as the game counts them for a client: air,
    /// cave air and void air are all air (`BlockState::is_air`).
    non_air: u16,
    /// Number of blocks whose state has a fluid in it.
    fluids: u16,
    biomes: Biomes,
}

impl PartialEq for Section {
    fn eq(&self, other: &Self) -> bool {
        self.non_air == other.non_air
            && self.fluids == other.fluids
            && self.biomes().eq(other.biomes())
            && match (&self.blocks, &other.blocks) {
                (Blocks::Uniform(a), Blocks::Uniform(b)) => a == b,
                // The forms and the palettes say how a section came to its blocks.
                _ => self.states().eq(other.states()),
            }
    }
}

impl Eq for Section {}

impl Section {
    /// A section of the given blocks, in the order of `y << 8 | z << 4 | x`.
    pub fn from_states(states: Box<[BlockState; BLOCKS_PER_SECTION]>, biome: Biome) -> Self {
        let mut palette = vec![states[0]];
        let mut last = states[0];
        for state in states.iter() {
            if *state != last && !palette.contains(state) {
                palette.push(*state);
            }
            last = *state;
        }
        if palette.len() == 1 {
            return Self::filled(states[0], biome);
        }
        let blocks = Blocks::of(palette, states.iter().copied());
        Self::counted(blocks, Biomes::One(biome))
    }

    /// A section consisting of `state` only.
    pub fn filled(state: BlockState, biome: Biome) -> Self {
        Self::counted(Blocks::Uniform(state), Biomes::One(biome))
    }

    /// A section of `blocks`, with its counts.
    fn counted(blocks: Blocks, biomes: Biomes) -> Self {
        let mut section = Self {
            blocks,
            non_air: 0,
            fluids: 0,
            biomes,
        };
        let (non_air, fluids) = match &section.blocks {
            Blocks::Uniform(state) => {
                let all = BLOCKS_PER_SECTION as u16;
                (
                    if state.is_air() { 0 } else { all },
                    if state.has_fluid() { all } else { 0 },
                )
            }
            _ => section.states().fold((0, 0), |(non_air, fluids), state| {
                (
                    non_air + u16::from(!state.is_air()),
                    fluids + u16::from(state.has_fluid()),
                )
            }),
        };
        section.non_air = non_air;
        section.fluids = fluids;
        section
    }

    /// The biome at the block `x`, `y`, `z`, each in `0..16`.
    pub fn biome_at(&self, x: usize, y: usize, z: usize) -> Biome {
        let index = index(x, y, z);
        match &self.biomes {
            Biomes::One(biome) => *biome,
            Biomes::Cells(cells) => {
                let (x, y, z) = (index & 15, index >> 8, index >> 4 & 15);
                cells[(y >> 2) << 4 | (z >> 2) << 2 | x >> 2]
            }
        }
    }

    /// The biome of each cell of four blocks, in the order of `y << 4 | z << 2 | x`.
    pub fn biomes(&self) -> impl Iterator<Item = Biome> + '_ {
        (0..BIOME_CELLS).map(move |cell| match &self.biomes {
            Biomes::One(biome) => *biome,
            Biomes::Cells(cells) => cells[cell],
        })
    }

    /// The biome all of the section has, if it has one.
    pub fn uniform_biome(&self) -> Option<Biome> {
        match &self.biomes {
            Biomes::One(biome) => Some(*biome),
            Biomes::Cells(_) => None,
        }
    }

    /// Gives the section the biomes `cells`, in the order of `y << 4 | z << 2 | x`.
    pub fn set_biomes(&mut self, cells: Box<[Biome; BIOME_CELLS]>) {
        let first = cells[0];
        self.biomes = if cells.iter().all(|biome| *biome == first) {
            Biomes::One(first)
        } else {
            Biomes::Cells(cells)
        };
    }

    /// Number of blocks that are not air. Cave air and void air are air here, as they
    /// are to the game where it counts a section's blocks and looks for a surface.
    pub fn non_air_count(&self) -> u16 {
        self.non_air
    }

    /// Number of blocks whose state has a fluid: water and lava, and blocks that are
    /// waterlogged.
    pub fn fluid_count(&self) -> u16 {
        self.fluids
    }

    /// The block state every position holds, if they all hold the same one.
    ///
    /// A section that became uniform through individual changes may still report `None`.
    pub fn uniform_state(&self) -> Option<BlockState> {
        match &self.blocks {
            Blocks::Uniform(state) => Some(*state),
            _ => None,
        }
    }

    /// Whether a position may hold a state of which `wanted` holds: false only if
    /// none does. It asks the palette, which can still have a state that no position
    /// has any more, so true is "look", not "there is one".
    pub fn may_hold(&self, mut wanted: impl FnMut(BlockState) -> bool) -> bool {
        match &self.blocks {
            Blocks::Uniform(state) => wanted(*state),
            Blocks::Indexed { palette, .. } => palette.iter().any(|state| wanted(*state)),
            Blocks::Direct(_) => true,
        }
    }

    /// The block at `x`, `y`, `z`, each in `0..16`.
    pub fn get(&self, x: usize, y: usize, z: usize) -> BlockState {
        // Checked for uniform sections too, so that a wrong coordinate fails everywhere.
        self.blocks.get(index(x, y, z))
    }

    /// Replaces the block at `x`, `y`, `z`, each in `0..16`, and returns the old one.
    ///
    /// A state the section has not held is added to its palette; a palette that is
    /// full moves the section to the next width, and beyond 256 states to an id for
    /// each position. A section never moves back by itself.
    pub fn set(&mut self, x: usize, y: usize, z: usize, state: BlockState) -> BlockState {
        let index = index(x, y, z);
        let old = self.blocks.get(index);
        if old == state {
            return old;
        }
        if let Blocks::Uniform(all) = self.blocks {
            self.blocks = Blocks::Indexed {
                bits: NARROWEST,
                palette: vec![all],
                words: vec![0; words_for(NARROWEST)].into_boxed_slice(),
            };
        }
        let full = match &self.blocks {
            Blocks::Indexed { bits, palette, .. } => {
                (!palette.contains(&state) && palette.len() == 1 << bits).then_some(*bits)
            }
            _ => None,
        };
        if let Some(bits) = full {
            // The palette is full: the next width with the palette as it is, or,
            // beyond the widest, an id for each position.
            let wider = if bits == WIDEST {
                Blocks::Direct(packed(DIRECT_BITS, self.states().map(|state| state.0)))
            } else {
                let Blocks::Indexed { palette, words, .. } = &self.blocks else {
                    unreachable!("the form was just looked at");
                };
                let indices = (0..BLOCKS_PER_SECTION).map(|at| read(words, bits, at));
                Blocks::Indexed {
                    bits: bits + 1,
                    words: packed(bits + 1, indices),
                    palette: palette.clone(),
                }
            };
            self.blocks = wider;
        }
        match &mut self.blocks {
            Blocks::Uniform(_) => unreachable!("a uniform section was given a palette above"),
            Blocks::Indexed {
                bits,
                palette,
                words,
            } => {
                let found = palette.iter().position(|known| *known == state);
                let at = found.unwrap_or_else(|| {
                    palette.push(state);
                    palette.len() - 1
                });
                write(words, *bits, index, at as u16);
            }
            Blocks::Direct(words) => write(words, DIRECT_BITS, index, state.0),
        }
        self.non_air -= u16::from(!old.is_air());
        self.non_air += u16::from(!state.is_air());
        self.fluids -= u16::from(old.has_fluid());
        self.fluids += u16::from(state.has_fluid());
        old
    }

    /// All blocks in the order of `y << 8 | z << 4 | x`.
    pub fn states(&self) -> impl Iterator<Item = BlockState> + '_ {
        (0..BLOCKS_PER_SECTION).map(move |index| self.blocks.get(index))
    }
}

/// How a section is serialised: without the counts, which the reader makes, and with
/// the words as bytes, lowest byte first, since postcard would write each word as an
/// integer of varying length.
#[derive(Serialize, Deserialize)]
struct SectionRepr {
    blocks: BlocksRepr,
    biomes: BiomesRepr,
}

#[derive(Serialize, Deserialize)]
enum BlocksRepr {
    Uniform(BlockState),
    Indexed {
        bits: u8,
        palette: Vec<BlockState>,
        words: Vec<u8>,
    },
    Direct(Vec<u8>),
}

#[derive(Serialize, Deserialize)]
enum BiomesRepr {
    One(Biome),
    Cells(Vec<Biome>),
}

fn bytes_of(words: &[u64]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}

/// The words for values of `bits` bits that `bytes` are, if they are as many as a
/// section has.
fn words_of(bytes: &[u8], bits: u8) -> Result<Box<[u64]>, String> {
    let expected = words_for(bits) * 8;
    if bytes.len() != expected {
        return Err(format!(
            "a section's blocks at {bits} bits are {expected} bytes, not {}",
            bytes.len()
        ));
    }
    let words = bytes
        .chunks_exact(8)
        .map(|word| u64::from_le_bytes(word.try_into().expect("chunks of eight bytes")));
    Ok(words.collect())
}

impl From<Section> for SectionRepr {
    fn from(section: Section) -> Self {
        Self {
            blocks: match section.blocks {
                Blocks::Uniform(state) => BlocksRepr::Uniform(state),
                Blocks::Indexed {
                    bits,
                    palette,
                    words,
                } => BlocksRepr::Indexed {
                    bits,
                    palette,
                    words: bytes_of(&words),
                },
                Blocks::Direct(words) => BlocksRepr::Direct(bytes_of(&words)),
            },
            biomes: match section.biomes {
                Biomes::One(biome) => BiomesRepr::One(biome),
                Biomes::Cells(cells) => BiomesRepr::Cells(cells.to_vec()),
            },
        }
    }
}

impl TryFrom<SectionRepr> for Section {
    type Error = String;

    /// Checks what it is given: whoever reads a section from a message can then index
    /// by what it holds.
    fn try_from(repr: SectionRepr) -> Result<Self, Self::Error> {
        let blocks = match repr.blocks {
            BlocksRepr::Uniform(state) => Blocks::Uniform(state),
            BlocksRepr::Indexed {
                bits,
                palette,
                words,
            } => {
                if !(NARROWEST..=WIDEST).contains(&bits) {
                    return Err(format!("a section's palette is indexed by {bits} bits"));
                }
                if palette.is_empty() || palette.len() > 1 << bits {
                    return Err(format!(
                        "a section's palette of {bits} bits has {} states",
                        palette.len()
                    ));
                }
                let words = words_of(&words, bits)?;
                let beyond = (0..BLOCKS_PER_SECTION)
                    .find(|index| usize::from(read(&words, bits, *index)) >= palette.len());
                if let Some(index) = beyond {
                    return Err(format!(
                        "position {index} of a section names a state beyond its palette"
                    ));
                }
                Blocks::Indexed {
                    bits,
                    palette,
                    words,
                }
            }
            BlocksRepr::Direct(words) => Blocks::Direct(words_of(&words, DIRECT_BITS)?),
        };
        let biomes = match repr.biomes {
            BiomesRepr::One(biome) => Biomes::One(biome),
            BiomesRepr::Cells(cells) => {
                let cells: Box<[Biome; BIOME_CELLS]> = cells
                    .into_boxed_slice()
                    .try_into()
                    .map_err(|cells: Box<[Biome]>| {
                        format!("a section has {BIOME_CELLS} biomes, not {}", cells.len())
                    })?;
                Biomes::Cells(cells)
            }
        };
        Ok(Self::counted(blocks, biomes))
    }
}

fn index(x: usize, y: usize, z: usize) -> usize {
    assert!(
        x < SECTION_SIZE && y < SECTION_SIZE && z < SECTION_SIZE,
        "({x}, {y}, {z}) is outside a section"
    );
    y << 8 | z << 4 | x
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;

    use super::*;

    const PLAINS: Biome = Biome(0);

    /// A sequence of numbers that looks random and is the same every time.
    fn numbers(mut seed: u64) -> impl Iterator<Item = usize> {
        std::iter::repeat_with(move || {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) as usize
        })
    }

    /// Fails unless `section` holds exactly `plain`, with the counts a recount gives.
    fn holds(section: &Section, plain: &[BlockState; BLOCKS_PER_SECTION]) {
        assert!(section.states().eq(plain.iter().copied()));
        let non_air = plain.iter().filter(|state| !state.is_air()).count();
        let fluids = plain.iter().filter(|state| state.has_fluid()).count();
        assert_eq!(usize::from(section.non_air_count()), non_air);
        assert_eq!(usize::from(section.fluid_count()), fluids);
    }

    /// Whatever is set, in whatever order, a section holds what a plain array holds,
    /// through every width of its palette and beyond the widest; and so does the
    /// section its serialised form is read back as.
    #[test]
    fn a_section_holds_what_a_plain_array_holds_through_every_form() {
        for (seed, kinds) in [
            (1, 3),
            (2, 16),
            (3, 17),
            (4, 40),
            (5, 256),
            (6, 257),
            (7, 3000),
        ] {
            let mut numbers = numbers(seed);
            let mut section = Section::filled(blocks::AIR, PLAINS);
            let mut plain = Box::new([blocks::AIR; BLOCKS_PER_SECTION]);
            for step in 0..6000 {
                let at = numbers.next().unwrap() % BLOCKS_PER_SECTION;
                // Among them air, water and lava, which the counts turn on.
                let state = BlockState((numbers.next().unwrap() % kinds * 7) as u16);
                let (x, y, z) = (at & 15, at >> 8, at >> 4 & 15);
                assert_eq!(section.set(x, y, z, state), plain[at]);
                plain[at] = state;
                assert_eq!(section.get(x, y, z), state);
                if step % 997 == 0 {
                    holds(&section, &plain);
                }
            }
            holds(&section, &plain);
            assert_eq!(section, Section::from_states(plain.clone(), PLAINS));
            let read = Section::try_from(SectionRepr::from(section.clone())).unwrap();
            holds(&read, &plain);
            assert_eq!(read, section);
        }
    }

    /// The seventeenth state moves a section to five bits and the 257th to an id for
    /// each position, and the bytes of each form are what the record counts.
    #[test]
    fn a_full_palette_moves_a_section_to_the_next_width() {
        let bytes = |section: &Section| match SectionRepr::from(section.clone()).blocks {
            BlocksRepr::Uniform(_) => (0, 0),
            BlocksRepr::Indexed { bits, words, .. } => (bits, words.len()),
            BlocksRepr::Direct(words) => (DIRECT_BITS, words.len()),
        };
        let mut section = Section::filled(BlockState(0), PLAINS);
        assert_eq!(bytes(&section), (0, 0));
        let mut widths = Vec::new();
        for state in 1..=256u16 {
            section.set(
                usize::from(state & 15),
                usize::from(state >> 4 & 15),
                0,
                BlockState(state),
            );
            if widths.last() != Some(&bytes(&section)) {
                widths.push(bytes(&section));
                assert_eq!(section.get(1, 0, 0), BlockState(1));
            }
        }
        let expected = [
            (4, 2048),
            (5, 2736),
            (6, 3280),
            (7, 3648),
            (8, 4096),
            (16, 8192),
        ];
        assert_eq!(widths, expected);
        // Sixteen states are still four bits, and 256 still eight.
        let mut sixteen = Section::filled(BlockState(0), PLAINS);
        for state in 1..16u16 {
            sixteen.set(usize::from(state), 0, 0, BlockState(state));
        }
        assert_eq!(bytes(&sixteen), (4, 2048));
    }

    /// A section that has held many states and holds two again equals a fresh one of
    /// those two, though its form is another.
    #[test]
    fn sections_are_equal_by_what_they_hold_and_not_by_how_they_came_to_it() {
        let mut worn = Section::filled(blocks::AIR, PLAINS);
        for state in 1..300u16 {
            worn.set(
                usize::from(state & 15),
                usize::from(state >> 4 & 15),
                1,
                BlockState(state),
            );
        }
        for state in 1..300u16 {
            worn.set(
                usize::from(state & 15),
                usize::from(state >> 4 & 15),
                1,
                blocks::AIR,
            );
        }
        worn.set(0, 0, 0, blocks::STONE);
        let mut fresh = Section::filled(blocks::AIR, PLAINS);
        fresh.set(0, 0, 0, blocks::STONE);
        assert_eq!(worn, fresh);
        assert_eq!(worn.uniform_state(), None);
        fresh.set(0, 0, 0, blocks::DIRT);
        assert_ne!(worn, fresh);
    }

    #[test]
    fn a_section_has_a_biome_for_each_cell_of_four_blocks() {
        let mut section = Section::filled(blocks::STONE, PLAINS);
        assert_eq!(section.uniform_biome(), Some(PLAINS));
        let cells: Box<[Biome; BIOME_CELLS]> =
            Box::new(std::array::from_fn(|cell| Biome(cell as u16)));
        section.set_biomes(cells);
        assert_eq!(section.uniform_biome(), None);
        // The cell of the block (5, 9, 14): x 1, y 2, z 3.
        assert_eq!(section.biome_at(5, 9, 14), Biome(2 << 4 | 3 << 2 | 1));
        assert!(section.biomes().eq((0..64).map(Biome)));
        assert_ne!(section, Section::filled(blocks::STONE, PLAINS));
        let read = Section::try_from(SectionRepr::from(section.clone())).unwrap();
        assert_eq!(read, section);
        // Sixty-four cells of one biome are one biome.
        section.set_biomes(Box::new([PLAINS; BIOME_CELLS]));
        assert_eq!(section, Section::filled(blocks::STONE, PLAINS));
        assert_eq!(section.uniform_biome(), Some(PLAINS));
    }

    /// What is read is checked, so that nobody indexes by what a message claimed.
    #[test]
    fn a_section_that_cannot_be_is_an_error_and_no_panic() {
        let mut section = Section::filled(blocks::AIR, PLAINS);
        section.set(1, 2, 3, blocks::STONE);
        let repr = || SectionRepr::from(section.clone());
        let broken = |change: &dyn Fn(&mut SectionRepr)| {
            let mut repr = repr();
            change(&mut repr);
            Section::try_from(repr).is_err()
        };
        assert!(Section::try_from(repr()).is_ok());
        type Change<'a> = &'a dyn Fn(&mut u8, &mut Vec<BlockState>, &mut Vec<u8>);
        let blocks = |change: Change| {
            broken(&|repr| {
                let BlocksRepr::Indexed {
                    bits,
                    palette,
                    words,
                } = &mut repr.blocks
                else {
                    unreachable!("two states are a palette");
                };
                change(bits, palette, words);
            })
        };
        // An index beyond the palette, too few words, a width that is none, a
        // palette too long for its width, and none at all.
        assert!(blocks(&|_, _, words| words[0] = 0x0f));
        assert!(blocks(&|_, _, words| words.truncate(2040)));
        assert!(blocks(&|bits, _, _| *bits = 3));
        assert!(blocks(&|bits, _, _| *bits = 9));
        assert!(blocks(&|_, palette, _| palette.resize(17, blocks::DIRT)));
        assert!(blocks(&|_, palette, _| palette.clear()));
        assert!(broken(
            &|repr| repr.blocks = BlocksRepr::Direct(vec![0; 8191])
        ));
        assert!(broken(
            &|repr| repr.biomes = BiomesRepr::Cells(vec![PLAINS; 63])
        ));
        assert!(!broken(
            &|repr| repr.blocks = BlocksRepr::Direct(vec![0; 8192])
        ));
    }

    #[test]
    fn filled_sections_count_their_blocks() {
        let air = Section::filled(blocks::AIR, PLAINS);
        assert_eq!(air.non_air_count(), 0);
        assert_eq!(air.uniform_state(), Some(blocks::AIR));

        let stone = Section::filled(blocks::STONE, PLAINS);
        assert_eq!(stone.non_air_count(), 4096);
        assert_eq!(stone.get(15, 15, 15), blocks::STONE);
    }

    /// The game counts cave air and void air as air where it counts a section's
    /// blocks, and so does a section, however the air came into it.
    #[test]
    fn cave_air_and_void_air_count_as_air() {
        for air in [blocks::CAVE_AIR, blocks::VOID_AIR] {
            assert_eq!(Section::filled(air, PLAINS).non_air_count(), 0);
            let mut section = Section::filled(blocks::STONE, PLAINS);
            section.set(1, 2, 3, air);
            assert_eq!(section.non_air_count(), 4095);
            section.set(1, 2, 3, blocks::AIR);
            assert_eq!(section.non_air_count(), 4095);
            section.set(1, 2, 3, blocks::DIRT);
            assert_eq!(section.non_air_count(), 4096);
            let again = Section::from_states(
                Box::new(std::array::from_fn(|index| {
                    if index == 0 { air } else { blocks::STONE }
                })),
                PLAINS,
            );
            assert_eq!(again.non_air_count(), 4095);
            // Still another block than plain air to whoever asks what is there.
            assert_ne!(
                Section::filled(air, PLAINS),
                Section::filled(blocks::AIR, PLAINS)
            );
        }
    }

    #[test]
    fn setting_blocks_keeps_the_count() {
        let mut section = Section::filled(blocks::AIR, PLAINS);
        assert_eq!(section.set(1, 2, 3, blocks::STONE), blocks::AIR);
        assert_eq!(section.set(4, 5, 6, blocks::DIRT), blocks::AIR);
        assert_eq!(section.non_air_count(), 2);
        assert_eq!(section.get(1, 2, 3), blocks::STONE);
        assert_eq!(section.get(3, 2, 1), blocks::AIR);
        assert_eq!(section.uniform_state(), None);

        // Replacing one solid block by another does not change the count.
        assert_eq!(section.set(1, 2, 3, blocks::DIRT), blocks::STONE);
        assert_eq!(section.non_air_count(), 2);
        assert_eq!(section.set(1, 2, 3, blocks::AIR), blocks::DIRT);
        assert_eq!(section.non_air_count(), 1);
    }

    #[test]
    fn setting_the_same_state_does_not_allocate() {
        let mut section = Section::filled(blocks::STONE, PLAINS);
        section.set(0, 0, 0, blocks::STONE);
        assert_eq!(section.uniform_state(), Some(blocks::STONE));
    }

    #[test]
    fn equality_is_about_content_not_about_history() {
        // Changing a block and changing it back leaves the section stored block by block.
        let mut edited = Section::filled(blocks::STONE, PLAINS);
        edited.set(1, 2, 3, blocks::DIRT);
        edited.set(1, 2, 3, blocks::STONE);
        assert_eq!(edited.uniform_state(), None);
        assert_eq!(edited, Section::filled(blocks::STONE, PLAINS));
        assert_ne!(edited, Section::filled(blocks::DIRT, PLAINS));
        assert_ne!(edited, Section::filled(blocks::STONE, Biome(1)));

        edited.set(0, 0, 0, blocks::AIR);
        assert_ne!(edited, Section::filled(blocks::STONE, PLAINS));
    }

    #[test]
    fn sections_can_be_built_from_their_blocks() {
        let mut expected = Section::filled(blocks::AIR, PLAINS);
        expected.set(1, 0, 0, blocks::STONE);
        expected.set(0, 1, 0, blocks::DIRT);
        let states: Vec<_> = expected.states().collect();
        let built = Section::from_states(states.try_into().unwrap(), PLAINS);
        assert_eq!(built, expected);
        assert_eq!(built.non_air_count(), 2);

        // All equal blocks collapse into the compact form.
        let uniform = Section::from_states(Box::new([blocks::STONE; 4096]), PLAINS);
        assert_eq!(uniform.uniform_state(), Some(blocks::STONE));
        assert_eq!(uniform.non_air_count(), 4096);
    }

    #[test]
    fn states_are_ordered_by_y_then_z_then_x() {
        let mut section = Section::filled(blocks::AIR, PLAINS);
        section.set(1, 0, 0, blocks::STONE);
        section.set(0, 0, 1, blocks::DIRT);
        section.set(0, 1, 0, blocks::BEDROCK);
        let states: Vec<_> = section.states().collect();
        assert_eq!(states.len(), 4096);
        assert_eq!(states[1], blocks::STONE);
        assert_eq!(states[16], blocks::DIRT);
        assert_eq!(states[256], blocks::BEDROCK);
    }

    #[test]
    #[should_panic(expected = "outside a section")]
    fn out_of_range_coordinates_panic() {
        Section::filled(blocks::AIR, PLAINS).get(16, 0, 0);
    }
}
