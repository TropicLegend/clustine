//! A 16×16×16 cube of blocks.

use clustine_data::{BlockState, blocks};
use serde::{Deserialize, Serialize};

use crate::{BLOCKS_PER_SECTION, SECTION_SIZE};

/// A biome, as its id in the biome registry Clustine sends to clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Biome(pub u16);

/// The blocks of a section. Most sections of a world are all air or all stone, so a
/// section of one block state does not allocate.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Blocks {
    Uniform(BlockState),
    /// Indexed by `y << 8 | z << 4 | x`.
    Mixed(Box<[BlockState; BLOCKS_PER_SECTION]>),
}

/// A 16×16×16 cube of blocks with one biome.
///
/// Biomes can vary within a section in vanilla; that is not modelled yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SectionRepr", into = "SectionRepr")]
pub struct Section {
    blocks: Blocks,
    /// Number of blocks that are not air.
    non_air: u16,
    biome: Biome,
}

impl Section {
    /// A section consisting of `state` only.
    pub fn filled(state: BlockState, biome: Biome) -> Self {
        Self {
            blocks: Blocks::Uniform(state),
            non_air: if state == blocks::AIR {
                0
            } else {
                BLOCKS_PER_SECTION as u16
            },
            biome,
        }
    }

    pub fn biome(&self) -> Biome {
        self.biome
    }

    /// Number of blocks that are not air.
    pub fn non_air_count(&self) -> u16 {
        self.non_air
    }

    /// The block state every position holds, if they all hold the same one.
    ///
    /// A section that became uniform through individual changes may still report `None`.
    pub fn uniform_state(&self) -> Option<BlockState> {
        match &self.blocks {
            Blocks::Uniform(state) => Some(*state),
            Blocks::Mixed(_) => None,
        }
    }

    /// The block at `x`, `y`, `z`, each in `0..16`.
    pub fn get(&self, x: usize, y: usize, z: usize) -> BlockState {
        // Checked for uniform sections too, so that a wrong coordinate fails everywhere.
        let index = index(x, y, z);
        match &self.blocks {
            Blocks::Uniform(state) => *state,
            Blocks::Mixed(states) => states[index],
        }
    }

    /// Replaces the block at `x`, `y`, `z`, each in `0..16`, and returns the old one.
    pub fn set(&mut self, x: usize, y: usize, z: usize, state: BlockState) -> BlockState {
        let index = index(x, y, z);
        let states = match &mut self.blocks {
            Blocks::Uniform(old) if *old == state => return state,
            Blocks::Uniform(old) => {
                self.blocks = Blocks::Mixed(Box::new([*old; BLOCKS_PER_SECTION]));
                let Blocks::Mixed(states) = &mut self.blocks else {
                    unreachable!("just assigned");
                };
                states
            }
            Blocks::Mixed(states) => states,
        };
        let old = std::mem::replace(&mut states[index], state);
        self.non_air -= u16::from(old != blocks::AIR);
        self.non_air += u16::from(state != blocks::AIR);
        old
    }

    /// All blocks in the order of `y << 8 | z << 4 | x`.
    pub fn states(&self) -> impl Iterator<Item = BlockState> + '_ {
        (0..BLOCKS_PER_SECTION).map(move |index| match &self.blocks {
            Blocks::Uniform(state) => *state,
            Blocks::Mixed(states) => states[index],
        })
    }
}

/// How a section is serialised: without the derived block count, and with the blocks of
/// a mixed section as a sequence, since serde has no support for arrays this long.
#[derive(Serialize, Deserialize)]
struct SectionRepr {
    biome: Biome,
    blocks: BlocksRepr,
}

#[derive(Serialize, Deserialize)]
enum BlocksRepr {
    Uniform(BlockState),
    Mixed(Vec<BlockState>),
}

impl From<Section> for SectionRepr {
    fn from(section: Section) -> Self {
        Self {
            biome: section.biome,
            blocks: match section.blocks {
                Blocks::Uniform(state) => BlocksRepr::Uniform(state),
                Blocks::Mixed(states) => BlocksRepr::Mixed(states.to_vec()),
            },
        }
    }
}

impl TryFrom<SectionRepr> for Section {
    type Error = String;

    fn try_from(repr: SectionRepr) -> Result<Self, Self::Error> {
        match repr.blocks {
            BlocksRepr::Uniform(state) => Ok(Self::filled(state, repr.biome)),
            BlocksRepr::Mixed(states) => {
                let non_air = states.iter().filter(|state| **state != blocks::AIR).count();
                let states: Box<[BlockState; BLOCKS_PER_SECTION]> = states
                    .into_boxed_slice()
                    .try_into()
                    .map_err(|states: Box<[BlockState]>| {
                        format!(
                            "a section has {BLOCKS_PER_SECTION} blocks, not {}",
                            states.len()
                        )
                    })?;
                Ok(Self {
                    blocks: Blocks::Mixed(states),
                    non_air: non_air as u16,
                    biome: repr.biome,
                })
            }
        }
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
    use super::*;

    const PLAINS: Biome = Biome(0);

    #[test]
    fn filled_sections_count_their_blocks() {
        let air = Section::filled(blocks::AIR, PLAINS);
        assert_eq!(air.non_air_count(), 0);
        assert_eq!(air.uniform_state(), Some(blocks::AIR));

        let stone = Section::filled(blocks::STONE, PLAINS);
        assert_eq!(stone.non_air_count(), 4096);
        assert_eq!(stone.get(15, 15, 15), blocks::STONE);
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
