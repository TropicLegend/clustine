// Adapted from SteelMC, steel-worldgen/src/biomes/biome_source.rs at 885c4b3
// (AGPL-3.0-or-later, Copyright (C) 2026 Alve Jeansson and contributors; see NOTICE).
// Changed for Clustine in October 2026: a biome is a function of the position alone,
// with no memory of an earlier search; a chunk is filled column by column; the End asks
// the emitted router for its islands.

//! Which biome is where: for a seed and a biome cell, the biome the game gives.
//!
//! A biome cell is a cube of four blocks; positions here count cells (the game's
//! "quart" positions), so the cell `x`, `y`, `z` holds the blocks from `4x`, `4y`,
//! `4z`. The 64 cells of a chunk's section are what the game stores and sends for it,
//! straight from its biome source. (The finer look of a border that a client draws,
//! and that the game's features ask for at a block, is made from these cells
//! afterwards, by offsets hashed from the seed; nothing of it is stored, and it is
//! not here.)
//!
//! - **The overworld and the Nether** look the biome up by the climate of the cell
//!   ([`climate`]) in a list of climates ([`search`]).
//! - **The End** has the biome `the_end` within 64 chunks of the origin and, beyond,
//!   one of four by how much island the router's `erosion` says there is at the
//!   chunk's middle.
//!
//! **Where this is not the game's.** The game breaks an exact tie between two
//! entries of a list by what the same thread found the time before, so that its
//! biome at such a cell depends on the order chunks were made in. Here a tie goes to
//! the first entry in the order of the game's tree, whatever was asked before
//! (`docs/groundwork/terrain-plan.md`, the rule of step T3; ADR-0021, section 1).
//! [`BiomeSource::found`] says which other biomes were tied.
//!
//! **What it was compared with**, in October 2026 and for seed 13579:
//!
//! - *Chunks the official server made*, the overworld only (`tests/official.rs`): of
//!   155,136 cells of 101 finished chunks, 155,132 are equal and 4 are exact ties; of
//!   631,296 cells of 411 chunks the server had left unfinished, 737 are exact ties.
//!   **The sample has no chunk of the Nether or the End**, so those two were not
//!   compared with chunks of the official server.
//! - *The game's own classes*, asked without a server by `tests/java/GameBiomes.java`
//!   (`tests/game.rs`), all three dimensions: 4,724,736 cells of the overworld with
//!   1,552 exact ties, 2,822,144 of the Nether with 63, 5,280,768 of the End with
//!   none, and no other difference. The game's searches replayed in the game's order
//!   with the game's memory gave the game's biome at every cell.
//! - *SteelMC's hashes* of 2,500 chunks a dimension (`tests/steelmc_hashes.rs`). They
//!   are of another build of the game than the pinned jar, which that test's head
//!   says more about; where they part, the jar is followed.
//!
//! None of the three runs in the ordinary checks but a few of SteelMC's hashes: the
//! first two need files that are not in the repository.

pub mod climate;
pub mod search;

use clustine_data::biome_parameters::BiomeParameterList;
use clustine_data::{DIMENSION_TYPES, synced_registry};
use clustine_world::{BIOME_CELLS, Biome};
use clustine_worldgen_data::{EndRouter, NetherRouter, NoiseRouter, OverworldRouter, RouterEntry};

use climate::{Climate, DEPTH, quantise};
use search::{Nearest, ParameterTree};

/// The three dimensions the game makes terrain for.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Dimension {
    Overworld,
    Nether,
    End,
}

impl Dimension {
    pub const ALL: [Dimension; 3] = [Dimension::Overworld, Dimension::Nether, Dimension::End];

    /// The name of the dimension's type in the game's registry.
    pub const fn type_name(self) -> &'static str {
        match self {
            Dimension::Overworld => "minecraft:overworld",
            Dimension::Nether => "minecraft:the_nether",
            Dimension::End => "minecraft:the_end",
        }
    }

    /// The section coordinate of the lowest section of a chunk, and how many
    /// sections a chunk has. Biomes are made for all of them, also where the
    /// dimension's terrain ends lower, as in the upper half of the Nether.
    pub fn sections(self) -> (i32, usize) {
        let kind = DIMENSION_TYPES
            .iter()
            .find(|kind| kind.name == self.type_name())
            .expect("the three dimension types are in the game's registry");
        (kind.min_y >> 4, (kind.height / 16) as usize)
    }
}

/// The biomes of one section: its 64 cells in the order of `y << 4 | z << 2 | x`,
/// which is what `clustine_world::Section::set_biomes` takes.
pub type SectionBiomes = Box<[Biome; BIOME_CELLS]>;

/// A source that looks biomes up by climate.
struct ByClimate<R: NoiseRouter> {
    router: R,
    tree: &'static ParameterTree,
}

impl<R: NoiseRouter> ByClimate<R> {
    fn climate(&self, x: i32, y: i32, z: i32) -> Climate {
        Climate::sample(&self.router, &mut R::Memory::default(), x, y, z)
    }

    fn search(&self, climate: &Climate, found: &mut Option<Nearest>) -> Biome {
        let nearest = match found {
            Some(before) => self.tree.nearest_guessing(climate, before.entry),
            None => self.tree.nearest(climate),
        };
        *found = Some(nearest);
        nearest.biome
    }

    /// The nearest entry for every cell of the chunk, section by section.
    fn chunk(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        lowest: i32,
        sections: usize,
    ) -> Vec<SectionBiomes> {
        let mut filled = vec![Box::new([Biome(0); BIOME_CELLS]); sections];
        let mut memory = R::Memory::default();
        let lowest_cell = lowest * 4;
        // The entry found for the cell before, which is likely to be near the next
        // cell's climate too. It only shortens the search (see `nearest_guessing`).
        let mut found: Option<Nearest> = None;
        for z in 0..4 {
            for x in 0..4 {
                let (cell_x, cell_z) = (chunk_x * 4 + x as i32, chunk_z * 4 + z as i32);
                // Of the six, depth alone changes with the height (a test of
                // `climate` holds the routers to that), so the others are asked for
                // once a column. Where the climate is what it was a cell below, so
                // is its biome: the search has no memory.
                let mut climate =
                    Climate::sample(&self.router, &mut memory, cell_x, lowest_cell, cell_z);
                let mut biome = self.search(&climate, &mut found);
                for up in 0..sections * 4 {
                    if up != 0 {
                        let block_y = (lowest_cell + up as i32) << 2;
                        let depth = quantise(self.router.sample(
                            &mut memory,
                            RouterEntry::Depth,
                            cell_x << 2,
                            block_y,
                            cell_z << 2,
                        ));
                        if depth != climate.0[DEPTH] {
                            climate.0[DEPTH] = depth;
                            biome = self.search(&climate, &mut found);
                        }
                    }
                    filled[up / 4][(up % 4) << 4 | z << 2 | x] = biome;
                }
            }
        }
        filled
    }
}

/// The End's source, with the five biomes it chooses between.
struct ByIslands {
    router: EndRouter,
    the_end: Biome,
    highlands: Biome,
    midlands: Biome,
    small_islands: Biome,
    barrens: Biome,
}

impl ByIslands {
    fn new(seed: i64) -> ByIslands {
        let biomes = synced_registry("minecraft:worldgen/biome")
            .expect("the registry of biomes is among the synchronised ones");
        let biome = |name: &str| {
            let id = biomes
                .id_of(name)
                .expect("the End's five biomes are in the registry");
            Biome(id as u16)
        };
        ByIslands {
            router: EndRouter::new(seed),
            the_end: biome("minecraft:the_end"),
            highlands: biome("minecraft:end_highlands"),
            midlands: biome("minecraft:end_midlands"),
            small_islands: biome("minecraft:small_end_islands"),
            barrens: biome("minecraft:end_barrens"),
        }
    }

    /// The biome of every cell of a chunk: the game decides by the chunk a cell is
    /// in, and by nothing finer.
    fn of_chunk(&self, chunk_x: i32, chunk_z: i32) -> Biome {
        let (x, z) = (i64::from(chunk_x), i64::from(chunk_z));
        if x * x + z * z <= 4096 {
            return self.the_end;
        }
        // The game asks at the block in the middle of the chunk, at the height of
        // the cell; the islands do not depend on the height.
        let islands = self.router.sample(
            &mut Default::default(),
            RouterEntry::Erosion,
            (chunk_x * 2 + 1) * 8,
            0,
            (chunk_z * 2 + 1) * 8,
        );
        if islands > 0.25 {
            self.highlands
        } else if islands >= -0.0625 {
            self.midlands
        } else if islands < -0.21875 {
            self.small_islands
        } else {
            self.barrens
        }
    }
}

/// A router with its noises is some thousand bytes, and the three differ much in
/// size, so each is kept behind a pointer.
enum Kind {
    Overworld(Box<ByClimate<OverworldRouter>>),
    Nether(Box<ByClimate<NetherRouter>>),
    End(Box<ByIslands>),
}

/// The biomes of one dimension of a world: a function of the seed and the position.
pub struct BiomeSource {
    dimension: Dimension,
    kind: Kind,
}

/// What was found at a cell, with what else might have been.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Found {
    pub biome: Biome,
    /// The climate the biome was looked up by; for the End, the router's climate,
    /// of which the game reads the islands alone.
    pub climate: Climate,
    /// The entry of the dimension's list that was found, for a dimension that has a
    /// list.
    pub nearest: Option<Nearest>,
    /// The other biomes that have an entry exactly as near: any of them is what the
    /// game may have at this cell. Sorted, and empty almost everywhere.
    pub tied: Vec<Biome>,
}

impl BiomeSource {
    /// The source of a dimension of the world with this seed. Making one seeds the
    /// dimension's noises; keep it for as long as biomes of the world are asked for.
    pub fn new(seed: i64, dimension: Dimension) -> BiomeSource {
        let kind = match dimension {
            Dimension::Overworld => Kind::Overworld(Box::new(ByClimate {
                router: OverworldRouter::new(seed),
                tree: ParameterTree::of(BiomeParameterList::Overworld),
            })),
            Dimension::Nether => Kind::Nether(Box::new(ByClimate {
                router: NetherRouter::new(seed),
                tree: ParameterTree::of(BiomeParameterList::Nether),
            })),
            Dimension::End => Kind::End(Box::new(ByIslands::new(seed))),
        };
        BiomeSource { dimension, kind }
    }

    pub fn dimension(&self) -> Dimension {
        self.dimension
    }

    /// The list of climates the dimension's biomes are looked up in; the End has
    /// none.
    pub fn parameters(&self) -> Option<&'static ParameterTree> {
        match &self.kind {
            Kind::Overworld(source) => Some(source.tree),
            Kind::Nether(source) => Some(source.tree),
            Kind::End(_) => None,
        }
    }

    /// The climate at the biome cell `x`, `y`, `z`.
    pub fn climate(&self, x: i32, y: i32, z: i32) -> Climate {
        match &self.kind {
            Kind::Overworld(source) => source.climate(x, y, z),
            Kind::Nether(source) => source.climate(x, y, z),
            Kind::End(source) => Climate::sample(&source.router, &mut Default::default(), x, y, z),
        }
    }

    /// The biome of the biome cell `x`, `y`, `z`.
    pub fn biome(&self, x: i32, y: i32, z: i32) -> Biome {
        match &self.kind {
            Kind::Overworld(source) => source.tree.nearest(&source.climate(x, y, z)).biome,
            Kind::Nether(source) => source.tree.nearest(&source.climate(x, y, z)).biome,
            // A cell is four blocks and a chunk sixteen.
            Kind::End(source) => source.of_chunk(x >> 2, z >> 2),
        }
    }

    /// The biome of a cell with the climate it was found by and the biomes it was
    /// tied with. Slower than [`Self::biome`]; for tests and for looking into a
    /// difference.
    pub fn found(&self, x: i32, y: i32, z: i32) -> Found {
        let climate = self.climate(x, y, z);
        let Some(tree) = self.parameters() else {
            return Found {
                biome: self.biome(x, y, z),
                climate,
                nearest: None,
                tied: Vec::new(),
            };
        };
        let nearest = tree.nearest(&climate);
        let mut tied: Vec<Biome> = tree
            .tied(&climate)
            .into_iter()
            .map(|found| found.biome)
            .filter(|biome| *biome != nearest.biome)
            .collect();
        tied.sort_unstable();
        tied.dedup();
        Found {
            biome: nearest.biome,
            climate,
            nearest: Some(nearest),
            tied,
        }
    }

    /// The biomes of the chunk at the chunk position `chunk_x`, `chunk_z`: for each
    /// of its sections from the lowest up, the 64 cells. Every cell is what
    /// [`Self::biome`] gives for it.
    pub fn chunk(&self, chunk_x: i32, chunk_z: i32) -> Vec<SectionBiomes> {
        let (lowest, sections) = self.dimension.sections();
        match &self.kind {
            Kind::Overworld(source) => source.chunk(chunk_x, chunk_z, lowest, sections),
            Kind::Nether(source) => source.chunk(chunk_x, chunk_z, lowest, sections),
            Kind::End(source) => {
                vec![Box::new([source.of_chunk(chunk_x, chunk_z); BIOME_CELLS]); sections]
            }
        }
    }
}

/// The biomes of one chunk of the world with this seed, as [`BiomeSource::chunk`]
/// gives them. This seeds the dimension's noises every time it is called, which costs
/// more than the chunk; whoever makes more than one chunk keeps a [`BiomeSource`].
pub fn biomes_of(
    seed: i64,
    dimension: Dimension,
    chunk_x: i32,
    chunk_z: i32,
) -> Vec<SectionBiomes> {
    BiomeSource::new(seed, dimension).chunk(chunk_x, chunk_z)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(name: &str) -> Biome {
        let biomes = synced_registry("minecraft:worldgen/biome").unwrap();
        Biome(biomes.id_of(name).unwrap() as u16)
    }

    #[test]
    fn a_chunk_has_the_sections_of_its_dimensions_type() {
        assert_eq!(Dimension::Overworld.sections(), (-4, 24));
        assert_eq!(Dimension::Nether.sections(), (0, 16));
        assert_eq!(Dimension::End.sections(), (0, 16));
    }

    #[test]
    fn every_cell_of_a_filled_chunk_is_the_biome_asked_for_alone() {
        for dimension in Dimension::ALL {
            let source = BiomeSource::new(13579, dimension);
            let (lowest, count) = dimension.sections();
            for (chunk_x, chunk_z) in [(0, 0), (-3, 7), (-292_300, 122_122), (300, -300)] {
                let sections = source.chunk(chunk_x, chunk_z);
                assert_eq!(sections.len(), count);
                for (index, cells) in sections.iter().enumerate() {
                    for cell in 0..BIOME_CELLS {
                        let (x, y, z) = (cell & 3, cell >> 4, cell >> 2 & 3);
                        let alone = source.biome(
                            chunk_x * 4 + x as i32,
                            (lowest + index as i32) * 4 + y as i32,
                            chunk_z * 4 + z as i32,
                        );
                        assert_eq!(
                            cells[cell], alone,
                            "{dimension:?} {chunk_x} {chunk_z} {cell}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_biome_does_not_depend_on_what_was_asked_before() {
        let source = BiomeSource::new(13579, Dimension::Overworld);
        let cells = [(0, 16, 0), (901, -3, -77), (-40, 20, 12), (5, 5, 5)];
        let forwards: Vec<Biome> = cells
            .iter()
            .map(|(x, y, z)| source.biome(*x, *y, *z))
            .collect();
        let mut backwards: Vec<Biome> = cells
            .iter()
            .rev()
            .map(|(x, y, z)| source.biome(*x, *y, *z))
            .collect();
        backwards.reverse();
        assert_eq!(forwards, backwards);
        assert_eq!(
            biomes_of(13579, Dimension::Overworld, 2, -5),
            source.chunk(2, -5)
        );
    }

    #[test]
    fn the_end_is_the_end_within_sixty_four_chunks_of_the_origin() {
        let source = BiomeSource::new(13579, Dimension::End);
        let the_end = id("minecraft:the_end");
        for (chunk_x, chunk_z) in [(0, 0), (64, 0), (0, -64), (-45, 45), (38, -51)] {
            assert_eq!(source.biome(chunk_x * 4, 9, chunk_z * 4 + 3), the_end);
        }
        // One chunk further out the islands decide, and there are none so near.
        let outer = [
            id("minecraft:end_highlands"),
            id("minecraft:end_midlands"),
            id("minecraft:small_end_islands"),
            id("minecraft:end_barrens"),
        ];
        for (chunk_x, chunk_z) in [(65, 0), (0, -65), (46, 46)] {
            let found = source.found(chunk_x * 4, 9, chunk_z * 4);
            assert!(outer.contains(&found.biome));
            assert_eq!(found.nearest, None);
            assert!(found.tied.is_empty());
        }
    }

    #[test]
    fn the_biomes_tied_with_the_one_found_are_the_others_at_its_distance() {
        let source = BiomeSource::new(13579, Dimension::Nether);
        let tree = source.parameters().unwrap();
        let found = source.found(10, 8, -20);
        let nearest = found.nearest.unwrap();
        assert_eq!(found.biome, source.biome(10, 8, -20));
        assert_eq!(found.climate, source.climate(10, 8, -20));
        assert_eq!(
            nearest.distance,
            tree.distance_of(nearest.entry, &found.climate)
        );
        for biome in &found.tied {
            assert_ne!(*biome, found.biome);
            assert_eq!(
                tree.distance_to(*biome, &found.climate),
                Some(nearest.distance)
            );
        }
    }
}
