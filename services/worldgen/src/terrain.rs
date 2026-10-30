//! A preview of the game's land: the bare terrain of a seed, chunk by chunk.
//!
//! **This is a preview, and the generation service of
//! `docs/adr/0021-generation-is-a-function.md` replaces it.** It exists so that the
//! land `clustine-terrain` makes can be walked with a real client before that
//! service is built. What it is not:
//!
//! - It makes a chunk in one call on the thread of whoever asks, which is the world
//!   store's one chunk thread: there is no pool, no order of runs, and the store does
//!   not keep what it handed out, so a chunk nobody changed is made again every time
//!   it is loaded.
//! - It stops after the game's carvers. The ground is bent where structures are to
//!   stand, as the game bends it before it places them, but no block of a structure
//!   or a feature is there: no trees, no plants, no ores but the veins, no lakes,
//!   springs, snow on the ground or ice on water, no villages.
//! - The marks of the fill (fluids that are to flow a step when the chunk is first
//!   ticked) are carried, and nothing acts on them yet: water stands where the game
//!   would let it run.
//!
//! What it does as the service will: a chunk is a function of the seed and the
//! position, and its light is made beside its eight neighbours' blocks, each of which
//! is this generator's as well, which the light says (`Light::beside`,
//! `docs/adr/0023-light-at-the-edge.md`, section 7). A chunk's light therefore needs
//! nine chunks' blocks. The generator keeps the unlit chunks it made last, so that
//! requests for neighbouring chunks share them; what it keeps changes what a chunk
//! costs and never what it is.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use clustine_data::{DIMENSION_TYPES, DimensionType};
use clustine_light::Around;
use clustine_structures::{Start, Structures};
use clustine_terrain::biome::zoom::obfuscate_seed;
use clustine_terrain::biome::{BiomeSource, Dimension};
use clustine_terrain::fill::Terrain;
use clustine_terrain::surface::Biomes;
use clustine_world::generation::GeneratorSettings;
use clustine_world::{BIOME_CELLS, Biome, BlockPos, Chunk, ChunkGenerator, ChunkPos, SECTION_SIZE};

/// The version of the terrain generator: what a chunk says it is exactly as made by
/// (`docs/adr/0022-what-a-chunk-carries.md`, section 1), and what its light says its
/// neighbours were made by. It is raised when the generator would make another chunk
/// from the same seed, and the settings name it, so that a world of an earlier version
/// is refused and not continued with land that does not fit.
///
/// 2: the ground is bent where structures stand.
pub const TERRAIN_VERSION: u16 = 2;

/// How many unlit chunks the generator keeps. A join at the view distance 8 asks for
/// 329 chunks, whose light reads the ring around them as well: within 23 by 23
/// chunks, which this holds with room for whoever walks meanwhile.
const KEPT: usize = 1024;

/// How many chunks' structure starts the generator keeps. A chunk's ground asks for
/// the starts of the 17 by 17 chunks around it, which for a join at the view
/// distance 8 is within 39 by 39 chunks.
const STARTS_KEPT: usize = 4096;

/// A cell of four blocks along each axis, as biomes are kept.
const CELL: usize = 4;

/// The unlit chunks made last, each with when it was last asked for.
#[derive(Default)]
struct Kept {
    /// A number that grows with every question, which is what "last" goes by.
    asked: u64,
    /// By position. The order of the map decides nothing: which chunk is let go
    /// follows from `asked`, and a chunk that was let go is made again the same.
    chunks: BTreeMap<ChunkPos, (u64, Arc<Chunk>)>,
}

/// The structures that start in the chunks asked for last, kept as [`Kept`] keeps
/// chunks. Most chunks have none, and a start is the same whoever asks.
#[derive(Default)]
struct KeptStarts {
    asked: u64,
    starts: BTreeMap<ChunkPos, (u64, Arc<[Start]>)>,
}

/// Biome cells by cell (x, y, z), four blocks across.
pub type BiomeAnswers = BTreeMap<(i32, i32, i32), Biome>;

/// Generates the overworld of a seed as the game's fill, surface and carvers leave
/// it, with the game's biomes, lit. See the head of this module for what that leaves
/// out and for what replaces it.
pub struct TerrainGenerator {
    seed: i64,
    dimension: &'static DimensionType,
    /// Where structures start, and the ground and biomes they and the chunks are
    /// made of.
    structures: Structures,
    /// Whether structures start at all, and so bend the ground. Off only for
    /// comparisons with the official server run with `generate-structures=false`.
    with_structures: bool,
    /// Biome cells answered otherwise than the source answers them, by cell: only for
    /// comparisons with the official server, which answers an exact tie of two biomes
    /// by what its thread looked up last; the judge gives the game's answers.
    biome_answers: Option<Arc<BiomeAnswers>>,
    /// Structures that never start, by their place in the data: only for comparisons
    /// with the official server given a data pack that leaves them no biome. Whole
    /// sets only, since within a set the game chooses among those that can start.
    structures_off: Vec<usize>,
    /// Change what a chunk costs, never what it is.
    kept: Mutex<Kept>,
    starts: Mutex<KeptStarts>,
    /// What the light is worked out in, kept to save its allocations. Whoever finds
    /// it taken works in one of their own.
    scratch: Mutex<clustine_light::Scratch>,
}

impl std::fmt::Debug for TerrainGenerator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerrainGenerator")
            .field("seed", &self.seed)
            .finish_non_exhaustive()
    }
}

impl TerrainGenerator {
    /// The generator of the overworld of the world with this seed. Making one seeds
    /// the dimension's noises, which takes a moment; make it once.
    pub fn new(seed: i64) -> Self {
        let dimension = DIMENSION_TYPES
            .iter()
            .find(|dimension| dimension.name == Dimension::Overworld.type_name())
            .expect("the overworld is a vanilla dimension type");
        Self {
            seed,
            dimension,
            structures: Structures::new(seed, Dimension::Overworld),
            with_structures: true,
            biome_answers: None,
            structures_off: Vec::new(),
            kept: Mutex::new(Kept::default()),
            starts: Mutex::new(KeptStarts::default()),
            scratch: Mutex::new(clustine_light::Scratch::new()),
        }
    }

    /// The generator of the same overworld where no structure starts, so that the
    /// ground is bent for none: what the official server makes with
    /// `generate-structures=false`, against which features are judged until phase S
    /// places structures' blocks.
    pub fn without_structures(seed: i64) -> Self {
        Self {
            with_structures: false,
            ..Self::new(seed)
        }
    }

    /// This generator with the biome cells of `answers` in place of the source's,
    /// for the surface and the chunks' biomes: for a comparison with the official
    /// server, which may answer an exact tie of two biomes with the other one
    /// (the terrain plan, T3). Cells are biome cells, four blocks across.
    #[must_use]
    pub fn with_biome_answers(mut self, answers: BiomeAnswers) -> Self {
        self.biome_answers = Some(Arc::new(answers));
        self
    }

    /// This generator where the structures `off`, by their place in the data, never
    /// start: for the judge of structures, which turns off in the game, with a data
    /// pack, the sets whose structures are not built yet.
    #[must_use]
    pub fn with_structures_off(mut self, off: Vec<usize>) -> Self {
        self.structures_off = off;
        self
    }

    /// Whether structures start in what this makes.
    pub fn with_structures(&self) -> bool {
        self.with_structures
    }

    /// The dimension type of what this makes.
    pub fn dimension(&self) -> &'static DimensionType {
        self.dimension
    }

    /// The seed of the world.
    pub fn seed(&self) -> i64 {
        self.seed
    }

    /// What a client is sent in place of the seed, and blurs the borders of biomes
    /// with: the game's `BiomeManager.obfuscateSeed`.
    pub fn hashed_seed(seed: i64) -> i64 {
        obfuscate_seed(seed)
    }

    /// The height of the overworld's sea, which a client is told when it logs in. Like
    /// [`Self::hashed_seed`] it needs no generator, so that whoever only tells clients
    /// of the world makes none.
    pub fn sea_level() -> i32 {
        clustine_terrain::biome::noise_settings(Dimension::Overworld).sea_level
    }

    fn terrain(&self) -> &Terrain {
        self.structures.terrain()
    }

    fn biomes(&self) -> &BiomeSource {
        self.structures.biomes()
    }

    /// The structures that start in the chunk, as `Structures::starts` makes them,
    /// from what is kept where it is there; none where structures are off.
    pub fn starts(&self, position: ChunkPos) -> Arc<[Start]> {
        if !self.with_structures {
            return Arc::from([]);
        }
        {
            let mut kept = self.starts.lock().unwrap_or_else(PoisonError::into_inner);
            kept.asked += 1;
            let asked = kept.asked;
            if let Some((last, starts)) = kept.starts.get_mut(&position) {
                *last = asked;
                return Arc::clone(starts);
            }
        }
        let mut starts = self.structures.starts(position.x, position.z);
        starts.retain(|start| !self.structures_off.contains(&start.structure));
        let starts: Arc<[Start]> = starts.into();
        let mut kept = self.starts.lock().unwrap_or_else(PoisonError::into_inner);
        let asked = kept.asked;
        kept.starts.insert(position, (asked, Arc::clone(&starts)));
        while kept.starts.len() > STARTS_KEPT {
            let longest = kept.starts.iter().min_by_key(|(_, (last, _))| *last);
            let Some(longest) = longest.map(|(position, _)| *position) else {
                break;
            };
            kept.starts.remove(&longest);
        }
        starts
    }

    /// The chunk's blocks, biomes and marks as the game's fill, surface and carvers
    /// leave them, with the ground bent where structures stand, without light and
    /// not yet marked as generated. This is what the light of the chunk and of the
    /// eight around it is made from.
    pub fn unlit(&self, position: ChunkPos) -> Chunk {
        let mut around = Biomes::from_source(self.biomes(), position.x, position.z);
        if let Some(answers) = &self.biome_answers {
            // The cells around the chunk reach a little beyond it on each side; a cell
            // they do not reach is refused by `set_cell`.
            let (low, high) = ((position.x << 2) - 2, (position.x << 2) + 5);
            for (&(x, y, z), biome) in
                answers.range((low, i32::MIN, i32::MIN)..=(high, i32::MAX, i32::MAX))
            {
                around.set_cell(x, y, z, *biome);
            }
        }
        let starts_of = |x, z| self.starts(ChunkPos::new(x, z));
        let beard = Structures::beardifier(position.x, position.z, starts_of);
        let carved = self
            .terrain()
            .carved_bent(position.x, position.z, &beard, &around, self.biomes())
            .chunk;
        let marks = carved.marks.clone();
        let mut sections = carved.sections(self.dimension, Biome(0));
        // The chunk's own cells are among those the surface was given, so they are
        // not asked of the source a second time.
        let (cell_x, cell_z) = (position.x << 2, position.z << 2);
        let lowest_cell = self.dimension.min_y >> 2;
        let cells_a_section = SECTION_SIZE / CELL;
        for (index, section) in sections.iter_mut().enumerate() {
            let mut cells = Box::new([Biome(0); BIOME_CELLS]);
            for (cell, biome) in cells.iter_mut().enumerate() {
                let (x, z, y) = (cell & 3, cell >> 2 & 3, cell >> 4);
                *biome = around.cell(
                    cell_x + x as i32,
                    lowest_cell + (index * cells_a_section + y) as i32,
                    cell_z + z as i32,
                );
            }
            section.set_biomes(cells);
        }
        let mut chunk = Chunk::from_sections(self.dimension.min_y, sections);
        chunk.set_marks(marks);
        chunk
    }

    /// [`Self::unlit`], from what is kept where it is there.
    fn shared(&self, position: ChunkPos) -> Arc<Chunk> {
        {
            let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
            kept.asked += 1;
            let asked = kept.asked;
            if let Some((last, chunk)) = kept.chunks.get_mut(&position) {
                *last = asked;
                return Arc::clone(chunk);
            }
        }
        // Made without the lock held: two who ask for the same chunk at once both
        // make it, and it is the same chunk.
        let chunk = Arc::new(self.unlit(position));
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        let asked = kept.asked;
        kept.chunks.insert(position, (asked, Arc::clone(&chunk)));
        while kept.chunks.len() > KEPT {
            let longest = kept.chunks.iter().min_by_key(|(_, (last, _))| *last);
            let Some(longest) = longest.map(|(position, _)| *position) else {
                break;
            };
            kept.chunks.remove(&longest);
        }
        chunk
    }

    /// How many unlit chunks are kept at the moment.
    pub fn kept(&self) -> usize {
        let kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        kept.chunks.len()
    }

    /// Lets go of every unlit chunk that is kept, as if nothing had been asked yet.
    pub fn forget(&self) {
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        kept.chunks.clear();
    }
}

impl clustine_features::structures::StartsOf for TerrainGenerator {
    fn starts(&self, source: ChunkPos) -> Arc<[Start]> {
        TerrainGenerator::starts(self, source)
    }
}

impl ChunkGenerator for TerrainGenerator {
    /// The chunk the game's search of the climate ends in (`clustine_terrain::spawn`).
    fn origin(&self) -> ChunkPos {
        let [x, z] = clustine_terrain::spawn::search_origin(self.seed);
        ChunkPos::new(x >> 4, z >> 4)
    }

    /// As every generator's, from the chunks before they are lit: the light changes
    /// nothing the spawn is chosen by.
    fn spawn(&self) -> BlockPos {
        clustine_world::spawn::place_to_stand(self.origin(), |position| self.shared(position))
    }

    fn generate(&self, position: ChunkPos) -> Chunk {
        let centre = self.shared(position);
        // North-west first, row by row from the north, without the centre: the order
        // `Around` takes them in.
        let beside: Vec<Arc<Chunk>> = (-1..=1)
            .flat_map(|z| (-1..=1).map(move |x| (x, z)))
            .filter(|offset| *offset != (0, 0))
            .map(|(x, z)| self.shared(ChunkPos::new(position.x + x, position.z + z)))
            .collect();
        let neighbours: [Option<&Chunk>; 8] = std::array::from_fn(|index| Some(&*beside[index]));
        let around = Around {
            centre: &centre,
            neighbours,
            // The overworld has a sky; a dimension type does not say yet whether it
            // has one.
            sky: true,
        };
        let mut light = match self.scratch.try_lock() {
            Ok(mut scratch) => {
                clustine_light::light_with(&around, clustine_light::Unlisted::NONE, &mut scratch).0
            }
            Err(_) => clustine_light::light(&around),
        };
        // The eight beside it are this generator's as well, which an edge goes by
        // when it asks whether the light still holds beside the chunks it has.
        light.beside = TERRAIN_VERSION;
        let mut chunk = (*centre).clone();
        chunk.set_light(light);
        // Said last: nothing changes the chunk after this.
        chunk.mark_generated(TERRAIN_VERSION, false);
        chunk
    }

    fn settings(&self) -> GeneratorSettings {
        // The version is a line of its own in a world's `meta`: a newer one opens the
        // world (ADR-0021, section 6).
        let identity = format!("terrain;preview;{};seed {}", self.dimension.name, self.seed);
        GeneratorSettings {
            identity,
            version: TERRAIN_VERSION,
        }
    }
}
