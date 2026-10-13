// Written for Clustine from the pinned jar's own classes
// (`net.minecraft.world.level.levelgen.material`: `MaterialSystem`,
// `MaterialRuleContext`, the conditions and the rules; `Biome`'s temperature;
// `ChunkStatusTasks.collectPossibleBiomes`).

//! The game's "build surface" step: what the ground of a filled chunk becomes.
//!
//! After the fill a chunk is the dimension's default block, air and fluids
//! ([`crate::fill`]). This step goes through its columns and makes of the ground what
//! the dimension's material rule says: grass and dirt, sand, snow, the terracotta of
//! the badlands, bedrock at the bottom, deepslate below zero, the Nether's and the
//! End's ground, **and the ore veins**, which in 26.3 are a material rule and not part
//! of the fill. Before a column's rule the pillars of the eroded badlands are raised,
//! and after it the icebergs of the frozen oceans.
//!
//! ```
//! use clustine_terrain::biome::{BiomeSource, Dimension};
//! use clustine_terrain::fill::Terrain;
//! use clustine_terrain::surface::Biomes;
//!
//! let seed = 13579;
//! let overworld = Terrain::new(seed, Dimension::Overworld);
//! let biomes = Biomes::from_source(&BiomeSource::new(seed, Dimension::Overworld), 0, 0);
//! let chunk = overworld.surface(0, 0, &biomes);
//! // Bedrock under the world, and the sea untouched above its floor.
//! assert_eq!(chunk.get(0, -64, 0), clustine_data::blocks::BEDROCK);
//! assert_eq!(chunk.get(0, 62, 0), clustine_data::blocks::WATER);
//! ```
//!
//! **What the step is in the pinned jar** (`MaterialSystem.buildSurface`). The columns
//! of the chunk are gone through x first, then z. For each:
//!
//! 1. The biome at the column's highest block ([`crate::biome::zoom`]); in the eroded
//!    badlands a pillar of the default block may be raised on the column.
//! 2. The column's numbers: how deep the surface is (a noise and a random number of
//!    the column), and the slope, which is the difference of the heights of the
//!    neighbouring columns **as they are at that moment**, with what earlier columns'
//!    rules, pillars and icebergs made of them, and held within the chunk.
//! 3. Down the column from its highest block: the rule is asked for every block that
//!    is neither air nor a fluid (not only for the default block), with how many such
//!    blocks lie above it and below it without a gap, and where the fluid above it
//!    begins. What the rule gives replaces the block; a fluid it gives is marked for
//!    post-processing, as the fill's fluids are.
//! 4. In the two frozen oceans an iceberg may be put on and under the sea.
//!
//! The rule itself is emitted Rust (`clustine_worldgen_data::material`); this module
//! is the context that answers its conditions. The game compiles a rule into objects
//! that remember an answer for a column or a block. An answer depends on the block
//! alone, so here a condition is computed when it is asked and what is costly is
//! remembered: the column's noises, the block's biome.
//!
//! **What it reads beside the blocks.** The biomes of the chunk and of one ring of
//! cells around it ([`Biomes`]): the biome at a block is that of one of the eight
//! cells around the block, so the blocks at a chunk's edge look into the neighbours.
//! The router's `chunk_surface_level` as a volume of the chunk's columns, one block
//! high: rules for the surface apply only from eight blocks below it. And for the ore
//! veins the density functions the rule names, two of them as a volume of the chunk
//! up to its highest section that is not air (`MaterialRuleContext.getDensitiesInChunk`)
//! and one at single blocks.
//!
//! **A condition that asks for a biome is skipped where the biome cannot be.** The
//! game does the same with the biomes of the nine chunks around
//! (`collectPossibleBiomes`); here it is the biomes of [`Biomes`], which are fewer and
//! still all that a block of the chunk can have. Without it every block of stone
//! would ask for its biome, because the overworld's rule for the underground begins
//! by asking whether the biome is the sulfur caves.
//!
//! **What it was compared with**, in October 2026: `tests/java/GameSurface.java` has
//! the game's own classes do the fill and then this step, without a server, and
//! `tests/game_surface.rs` holds every block and every mark to it, and the numbers on
//! the way: all 380,108,800 blocks of 5,600 chunks of the three dimensions on three
//! seeds are equal. The head of that test has the rest. Of SteelMC's hashes of chunks
//! at the game's `terrain` status, which are of chunks after the carvers too, 210 of
//! the overworld's 2,500 and 393 of the Nether's are this step's
//! (`tests/steelmc_terrain.rs`).
//!
//! **The carvers' half that is here.** A carver only names blocks ([`crate::carver`]);
//! what becomes of them is decided here ([`System::apply_carving_mask`]), because
//! dirt left under carved grass is given the top of the ground by this module's
//! rule ([`Terrain::top_material`]). Both were compared with the game's in
//! `tests/game_carvers.rs`: the carved chunks block by block, and the top material
//! also by itself at 7,342,080 blocks, on slopes, under the sea and below the
//! surface in every biome.
//!
//! [`Terrain::top_material`]: crate::fill::Terrain::top_material

use std::marker::PhantomData;

use clustine_data::{BLOCKS, BlockState, DIMENSION_TYPES, TAGS, blocks, synced_registry};
use clustine_noise::math::map;
use clustine_noise::{
    LegacyRandom, NameHash, NormalNoise, PositionalRandom, Random, RandomSplitter, SimplexNoise,
};
use clustine_world::{Biome, InChunk};
use clustine_worldgen_data::{
    Anchor, BiomeClimate, Context, Give, Interpolated, MaterialRule, NoiseParameters, NoiseRouter,
    NoiseSettings, RouterEntry, Vein, biomes, density, noises,
};

use crate::aquifer::Aquifer;
use crate::biome::zoom::Zoom;
use crate::biome::{BiomeSource, Dimension};
use crate::carver::Mask;
use crate::fill::Filled;
use crate::noise_chunk::{self, Scratch, Volume};

/// The game's `DimensionType.WAY_BELOW_MIN_Y`.
const WAY_BELOW_MIN_Y: i32 = -32_512;

/// How far below the preliminary surface the rules for the surface still apply
/// (`HOW_FAR_BELOW_PRELIMINARY_SURFACE_LEVEL_TO_BUILD_SURFACE`).
const BELOW_PRELIMINARY_SURFACE: i32 = 8;

/// How many bands of terracotta there are before they repeat.
const BANDS: usize = 192;

/// How many cells of biomes across the surface of one chunk reads: the chunk's four
/// and one on each side.
const CELLS_ACROSS: usize = 6;

/// The biomes that the surface of one chunk reads: the chunk's own cells and one ring
/// of cells around them, at every height of the dimension.
///
/// In the game these are the biomes stored in the chunk and in its eight neighbours,
/// which were all made before a surface is. Whoever keeps chunks' biomes gives them
/// ([`Self::from_columns`]); otherwise they are made here ([`Self::from_source`]),
/// which costs two and a quarter times the biomes of a chunk.
///
/// **A biome that is an exact tie between two entries is not always the game's**
/// (see [`crate::biome`]); with the game's own biomes given here, the surface is the
/// game's.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Biomes {
    /// The lowest cell of each axis.
    min_x: i32,
    min_y: i32,
    min_z: i32,
    cells_high: usize,
    /// In the order of x, then z, then y.
    cells: Vec<Biome>,
    /// A bit for each biome's number: whether a cell has it.
    possible: Vec<u64>,
    /// The one biome every cell has, where there is but one.
    only: Option<Biome>,
}

impl Biomes {
    /// The biomes around the chunk at `chunk_x`, `chunk_z`, made by the dimension's
    /// source.
    #[must_use]
    pub fn from_source(source: &BiomeSource, chunk_x: i32, chunk_z: i32) -> Self {
        Self::from_columns(source.dimension(), chunk_x, chunk_z, |cell_x, cell_z| {
            source.column(cell_x, cell_z)
        })
    }

    /// The biomes around the chunk at `chunk_x`, `chunk_z`, from whoever has them:
    /// `column` gives the column of cells at a cell's x and z, every cell of the
    /// dimension's height from the lowest up (as [`BiomeSource::column`] does). It is
    /// asked for the 36 columns from one cell before the chunk to one after it.
    ///
    /// # Panics
    ///
    /// If a column has not as many cells as the dimension is high.
    #[must_use]
    pub fn from_columns(
        dimension: Dimension,
        chunk_x: i32,
        chunk_z: i32,
        mut column: impl FnMut(i32, i32) -> Vec<Biome>,
    ) -> Self {
        let (lowest, sections) = dimension.sections();
        let cells_high = sections * 4;
        let (min_x, min_z) = (chunk_x * 4 - 1, chunk_z * 4 - 1);
        let mut cells = Vec::with_capacity(CELLS_ACROSS * CELLS_ACROSS * cells_high);
        let mut possible = Vec::new();
        for x in 0..CELLS_ACROSS as i32 {
            for z in 0..CELLS_ACROSS as i32 {
                let column = column(min_x + x, min_z + z);
                assert_eq!(
                    column.len(),
                    cells_high,
                    "a column of biomes has a cell for every four blocks of the dimension"
                );
                for biome in &column {
                    let word = usize::from(biome.0) / 64;
                    if possible.len() <= word {
                        possible.resize(word + 1, 0);
                    }
                    possible[word] |= 1 << (biome.0 % 64);
                }
                cells.extend(column);
            }
        }
        let only = cells
            .first()
            .copied()
            .filter(|first| cells.iter().all(|cell| cell == first));
        Self {
            min_x,
            min_y: lowest * 4,
            min_z,
            cells_high,
            cells,
            possible,
            only,
        }
    }

    /// The biome of the cell `x`, `y`, `z`, the height held within the dimension's
    /// as a chunk of the game holds it.
    ///
    /// # Panics
    ///
    /// If the cell is not of the chunk or of the ring around it.
    #[must_use]
    pub fn cell(&self, x: i32, y: i32, z: i32) -> Biome {
        let (across_x, across_z) = ((x - self.min_x) as usize, (z - self.min_z) as usize);
        assert!(
            across_x < CELLS_ACROSS && across_z < CELLS_ACROSS,
            "the cell {x} {z} is not among the biomes that were given"
        );
        let up = (y - self.min_y).clamp(0, self.cells_high as i32 - 1) as usize;
        self.cells[(across_x * CELLS_ACROSS + across_z) * self.cells_high + up]
    }

    /// Makes the cell `x`, `y`, `z` another biome, for whoever knows a cell better
    /// than the columns that were given: a test that was told what the game
    /// answered at a tie. `false`, and nothing changed, where the cell is not of
    /// the chunk or the ring around it or not within the dimension's heights.
    pub fn set_cell(&mut self, x: i32, y: i32, z: i32, biome: Biome) -> bool {
        let (across_x, across_z, up) = (x - self.min_x, z - self.min_z, y - self.min_y);
        let across = 0..CELLS_ACROSS as i32;
        if !across.contains(&across_x)
            || !across.contains(&across_z)
            || !(0..self.cells_high as i32).contains(&up)
        {
            return false;
        }
        let index =
            (across_x as usize * CELLS_ACROSS + across_z as usize) * self.cells_high + up as usize;
        self.cells[index] = biome;
        let word = usize::from(biome.0) / 64;
        if self.possible.len() <= word {
            self.possible.resize(word + 1, 0);
        }
        // A biome that no cell has any more stays possible, which costs a rule a
        // question and changes no answer.
        self.possible[word] |= 1 << (biome.0 % 64);
        self.only = self.only.filter(|only| *only == biome);
        true
    }

    /// The biome at a block of the chunk, as the game's `BiomeManager.getBiome` gives
    /// it.
    #[must_use]
    pub fn at_block(&self, zoom: Zoom, x: i32, y: i32, z: i32) -> Biome {
        // Where the eight cells around the block have one biome, it does not matter
        // which of them is nearest; the distances are what costs.
        if let Some(only) = self.only {
            return only;
        }
        let (low_x, low_y, low_z) = ((x - 2) >> 2, (y - 2) >> 2, (z - 2) >> 2);
        let first = self.cell(low_x, low_y, low_z);
        let one = (0..8).all(|corner| {
            self.cell(
                low_x + (corner >> 2 & 1),
                low_y + (corner >> 1 & 1),
                low_z + (corner & 1),
            ) == first
        });
        if one {
            return first;
        }
        let (cell_x, cell_y, cell_z) = zoom.cell(x, y, z);
        self.cell(cell_x, cell_y, cell_z)
    }

    /// Whether a block of the chunk can have one of these biomes.
    fn may_have(&self, biomes: &[u16]) -> bool {
        biomes.iter().any(|biome| {
            self.possible
                .get(usize::from(*biome) / 64)
                .is_some_and(|word| word >> (biome % 64) & 1 == 1)
        })
    }
}

/// The three noises a biome's temperature is varied by. They are the same in every
/// world: the game seeds them with fixed numbers.
struct Temperature {
    /// Makes it colder with the height above the sea.
    height: SimplexNoise,
    /// The patches of a frozen ocean that are not frozen.
    frozen: [SimplexNoise; 3],
    info: SimplexNoise,
}

impl Temperature {
    fn new() -> Self {
        let mut frozen = LegacyRandom::from_seed(3456);
        Self {
            height: SimplexNoise::new_without_offset(&mut LegacyRandom::from_seed(1234)),
            frozen: [
                SimplexNoise::new_without_offset(&mut frozen),
                SimplexNoise::new_without_offset(&mut frozen),
                SimplexNoise::new_without_offset(&mut frozen),
            ],
            info: SimplexNoise::new_without_offset(&mut LegacyRandom::from_seed(2345)),
        }
    }

    /// The frozen oceans' noise: three octaves, the widest counting most.
    fn frozen(&self, x: f64, z: f64) -> f32 {
        const LAYERS: [(f64, f32); 3] =
            [(1.0, 0.142_857_15), (0.5, 0.285_714_3), (0.25, 0.571_428_6)];
        let mut sum = 0.0_f32;
        for (noise, (frequency, amplitude)) in self.frozen.iter().zip(LAYERS) {
            sum += amplitude * noise.get_2d(x * frequency, z * frequency);
        }
        sum
    }

    /// The game's `Biome.getTemperature` at a block.
    fn at(&self, climate: &BiomeClimate, x: i32, y: i32, z: i32, sea_level: i32) -> f32 {
        let mut base = climate.temperature;
        if climate.frozen {
            let (block_x, block_z) = (f64::from(x), f64::from(z));
            let frozen = f64::from(self.frozen(block_x * 0.05, block_z * 0.05) * 7.0);
            let info = f64::from(self.info.get_2d(block_x * 0.2, block_z * 0.2));
            if frozen + info < 0.3
                && f64::from(self.info.get_2d(block_x * 0.09, block_z * 0.09)) < 0.8
            {
                base = 0.2;
            }
        }
        let snow_line = sea_level + 17;
        if y > snow_line {
            let noise = self
                .height
                .get_2d(f64::from(x as f32 / 8.0), f64::from(z as f32 / 8.0))
                * 8.0;
            base - (noise + y as f32 - snow_line as f32) * 0.05 / 40.0
        } else {
            base
        }
    }
}

/// What the surface of one dimension of one world is made with: the game's
/// `MaterialSystem` and what its rule's conditions read.
pub struct System<R: NoiseRouter> {
    settings: &'static NoiseSettings,
    default_block: BlockState,
    /// The lowest and the highest block of a chunk of the dimension, which in the
    /// Nether and the End is higher than the terrain.
    chunk_min_y: i32,
    chunk_max_y: i32,
    /// The factory of the dimension's random numbers.
    random: RandomSplitter,
    bands: [BlockState; BANDS],
    surface: NormalNoise,
    surface_secondary: NormalNoise,
    clay_bands_offset: NormalNoise,
    badlands_pillar: NormalNoise,
    badlands_pillar_roof: NormalNoise,
    badlands_surface: NormalNoise,
    iceberg_pillar: NormalNoise,
    iceberg_pillar_roof: NormalNoise,
    iceberg_surface: NormalNoise,
    /// The noises and the factories of random numbers of the rule's conditions, by
    /// the numbers the rule gives.
    rule_noises: Vec<NormalNoise>,
    rule_randoms: Vec<RandomSplitter>,
    /// The factory of the ore veins' random numbers.
    ore: RandomSplitter,
    zoom: Zoom,
    eroded_badlands: Biome,
    frozen_ocean: Biome,
    deep_frozen_ocean: Biome,
    temperature: Temperature,
    /// The numbers of the interpolated functions that the router's
    /// `chunk_surface_level` reaches, and of those the rule's density functions reach.
    in_surface_level: Vec<usize>,
    in_rule: Vec<usize>,
    /// For each block by its number: whether it is `#minecraft:uncarvable`.
    uncarvable: Vec<bool>,
    router: PhantomData<fn() -> R>,
}

/// For each block by its number: whether the carvers leave it
/// (`#minecraft:uncarvable`, which is bedrock alone in the game's data).
fn uncarvable() -> Vec<bool> {
    let tag = TAGS
        .iter()
        .find(|tags| tags.registry == "minecraft:block")
        .and_then(|tags| {
            tags.tags
                .iter()
                .find(|tag| tag.name == "minecraft:uncarvable")
        })
        .expect("the game's data has the tag of what carvers leave");
    let mut uncarvable = vec![false; BLOCKS.len()];
    for block in tag.entries {
        uncarvable[*block as usize] = true;
    }
    uncarvable
}

/// The bands of terracotta of a world (`MaterialSystem.generateBands`).
fn generate_bands(random: &mut impl Random) -> [BlockState; BANDS] {
    let mut bands = [blocks::TERRACOTTA; BANDS];
    let mut at = 0;
    while at < BANDS {
        at += random.next_i32_bounded(5) as usize + 1;
        if at < BANDS {
            bands[at] = blocks::ORANGE_TERRACOTTA;
        }
        at += 1;
    }
    make_bands(random, &mut bands, 1, blocks::YELLOW_TERRACOTTA);
    make_bands(random, &mut bands, 2, blocks::BROWN_TERRACOTTA);
    make_bands(random, &mut bands, 1, blocks::RED_TERRACOTTA);
    let white = random.next_i32_between(9, 15);
    let mut at = 0;
    let mut made = 0;
    while made < white && at < BANDS {
        bands[at] = blocks::WHITE_TERRACOTTA;
        if at > 1 && random.next_bool() {
            bands[at - 1] = blocks::LIGHT_GRAY_TERRACOTTA;
        }
        if at + 1 < BANDS && random.next_bool() {
            bands[at + 1] = blocks::LIGHT_GRAY_TERRACOTTA;
        }
        made += 1;
        at += random.next_i32_bounded(16) as usize + 4;
    }
    bands
}

fn make_bands(
    random: &mut impl Random,
    bands: &mut [BlockState; BANDS],
    base_width: i32,
    state: BlockState,
) {
    let count = random.next_i32_between(6, 15);
    for _ in 0..count {
        let width = (base_width + random.next_i32_bounded(3)) as usize;
        let start = random.next_i32_bounded(BANDS as i32) as usize;
        for band in bands.iter_mut().skip(start).take(width) {
            *band = state;
        }
    }
}

/// The game's `Mth.floor` of a float.
#[inline]
fn floor(value: f32) -> i32 {
    let whole = value as i32;
    if value < whole as f32 {
        whole - 1
    } else {
        whole
    }
}

/// What a block is to the loop down a column.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Air,
    Fluid,
    /// Neither: what the game's `MaterialSystem.isStone` says yes to.
    Ground,
}

impl<R: NoiseRouter> System<R> {
    /// The surface system of the world with this seed, for the dimension whose router
    /// is `R`.
    #[must_use]
    pub fn new(seed: i64, dimension: Dimension, router: &R) -> Self {
        let settings = router.settings();
        let random = density::splitter(seed, settings.legacy_random_source);
        let noise = |parameters: &NoiseParameters| density::noise(&random, parameters);
        let kind = DIMENSION_TYPES
            .iter()
            .find(|kind| kind.name == dimension.type_name())
            .expect("the three dimension types are in the game's registry");
        let registry = synced_registry("minecraft:worldgen/biome")
            .expect("the registry of biomes is among the synchronised ones");
        let biome = |name: &str| {
            let id = registry
                .id_of(name)
                .expect("the biomes the surface asks for are in the registry");
            Biome(id as u16)
        };
        let named = |name: &str| random.with_hash_of(&NameHash::new(name));
        let interpolated = router.interpolated();
        let numbers = |wanted: fn(&Interpolated) -> bool| -> Vec<usize> {
            (0..interpolated.len())
                .filter(|number| wanted(&interpolated[*number]))
                .collect()
        };
        Self {
            settings,
            default_block: BlockState(settings.default_block as u16),
            chunk_min_y: kind.min_y,
            chunk_max_y: kind.min_y + kind.height as i32 - 1,
            bands: generate_bands(&mut named("minecraft:clay_bands")),
            surface: noise(&noises::SURFACE),
            surface_secondary: noise(&noises::SURFACE_SECONDARY),
            clay_bands_offset: noise(&noises::CLAY_BANDS_OFFSET),
            badlands_pillar: noise(&noises::BADLANDS_PILLAR),
            badlands_pillar_roof: noise(&noises::BADLANDS_PILLAR_ROOF),
            badlands_surface: noise(&noises::BADLANDS_SURFACE),
            iceberg_pillar: noise(&noises::ICEBERG_PILLAR),
            iceberg_pillar_roof: noise(&noises::ICEBERG_PILLAR_ROOF),
            iceberg_surface: noise(&noises::ICEBERG_SURFACE),
            rule_noises: R::Material::NOISES
                .iter()
                .map(|parameters| noise(parameters))
                .collect(),
            rule_randoms: R::Material::RANDOMS
                .iter()
                .map(|name| named(name).next_positional())
                .collect(),
            ore: named("minecraft:ore").next_positional(),
            zoom: Zoom::new(seed),
            eroded_badlands: biome("minecraft:eroded_badlands"),
            frozen_ocean: biome("minecraft:frozen_ocean"),
            deep_frozen_ocean: biome("minecraft:deep_frozen_ocean"),
            temperature: Temperature::new(),
            in_surface_level: numbers(|function| function.in_chunk_surface_level),
            in_rule: numbers(|function| function.in_material_rule),
            uncarvable: uncarvable(),
            random,
            router: PhantomData,
        }
    }

    /// Which cell a block takes its biome from.
    #[must_use]
    pub fn zoom(&self) -> Zoom {
        self.zoom
    }

    /// The bands of terracotta of the world, from the lowest; they repeat.
    #[must_use]
    pub fn bands(&self) -> &[BlockState] {
        &self.bands
    }

    /// How many blocks deep the surface is at a column (`getSurfaceDepth`).
    #[must_use]
    pub fn surface_depth(&self, x: i32, z: i32) -> i32 {
        let noise = f64::from(self.surface.get(f64::from(x), 0.0, f64::from(z)));
        (noise * 2.75 + 3.0 + self.random.at(x, 0, z).next_f64() * 0.25) as i32
    }

    /// The second noise of a column's depth (`getSurfaceSecondary`).
    #[must_use]
    pub fn surface_secondary(&self, x: i32, z: i32) -> f64 {
        f64::from(self.surface_secondary.get(f64::from(x), 0.0, f64::from(z)))
    }

    /// The band of terracotta at a block (`getBand`).
    #[must_use]
    pub fn band(&self, x: i32, y: i32, z: i32) -> BlockState {
        let noise = self.clay_bands_offset.get(f64::from(x), 0.0, f64::from(z)) * 4.0;
        // Java's `Math.round` of a float: the floor of the value and a half, which a
        // double holds exactly.
        let offset = (f64::from(noise) + 0.5).floor() as i32;
        self.bands[(y + offset + BANDS as i32).rem_euclid(BANDS as i32) as usize]
    }

    /// Whether it is cold enough to snow at a block of this biome
    /// (`Biome.coldEnoughToSnow`).
    #[must_use]
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    pub fn cold_enough_to_snow(&self, biome: Biome, x: i32, y: i32, z: i32) -> bool {
        // Not "below": a temperature that is no number is cold.
        !(self.temperature_at(biome, x, y, z) >= 0.15)
    }

    fn temperature_at(&self, biome: Biome, x: i32, y: i32, z: i32) -> f32 {
        let climate = &biomes::CLIMATE[usize::from(biome.0)];
        self.temperature
            .at(climate, x, y, z, self.settings.sea_level)
    }

    /// The router's `chunk_surface_level` at every column of a box of columns, as
    /// the game samples it for the surface: a volume one block high at the height
    /// zero. In the order of x, then z counting most.
    #[must_use]
    pub fn surface_level(&self, router: &R, columns: &Volume, scratch: &mut Scratch) -> Vec<f32> {
        let flat = Volume {
            min_y: 0,
            size_y: 1,
            ..*columns
        };
        let mut memory = R::Memory::default();
        noise_chunk::interpolated(router, &mut memory, &self.in_surface_level, &flat, scratch);
        let mut levels = Vec::with_capacity(flat.len());
        for z in 0..flat.size_z {
            for x in 0..flat.size_x {
                for number in &self.in_surface_level {
                    memory.give(*number, scratch.interpolated(*number)[flat.index(x, 0, z)]);
                }
                levels.push(router.sample(
                    &mut memory,
                    RouterEntry::ChunkSurfaceLevel,
                    flat.min_x + x as i32,
                    0,
                    flat.min_z + z as i32,
                ));
            }
        }
        levels
    }

    /// Each density function the rule names, at every block of `volume`, in the
    /// volume's order: its name, its values as the game samples the volume at once,
    /// and its values at the blocks alone. For a test to compare with the game's.
    #[must_use]
    pub fn densities(
        &self,
        router: &R,
        volume: &Volume,
        scratch: &mut Scratch,
    ) -> Vec<(&'static str, Vec<f32>, Vec<f32>)> {
        let mut veins = Veins::new(self, router, *volume, scratch);
        let mut functions = Vec::new();
        for (function, name) in R::Material::FUNCTIONS.iter().enumerate() {
            let mut sampled = [Vec::with_capacity(volume.len()), Vec::new()];
            for (values, in_volume) in sampled.iter_mut().zip([true, false]) {
                for z in 0..volume.size_z as i32 {
                    for x in 0..volume.size_x as i32 {
                        for y in 0..volume.size_y as i32 {
                            values.push(veins.density(
                                router,
                                function,
                                in_volume,
                                volume.min_x + x,
                                volume.min_y + y,
                                volume.min_z + z,
                            ));
                        }
                    }
                }
            }
            let [in_volume, alone] = sampled;
            functions.push((*name, in_volume, alone));
        }
        functions
    }

    /// The step itself: makes of the filled chunk `filled`, which is the chunk at
    /// the chunk position `(x, z)`, the chunk after the game's `buildSurface`.
    /// `biomes` are those around the chunk. `watch` names a
    /// block to say more about, for looking into a difference.
    pub fn build(
        &self,
        router: &R,
        filled: &mut Filled,
        (chunk_x, chunk_z): (i32, i32),
        biomes: &Biomes,
        scratch: &mut Scratch,
        watch: Option<[i32; 3]>,
    ) -> Option<Watched> {
        let heights = heights(filled);
        // The volume the game samples the ore veins' functions in: the chunk, from
        // the bottom of the terrain to the top of its highest section that is not
        // air, as it is before any column is gone through.
        let highest = heights.iter().copied().max().unwrap_or(filled.min_y) - 1;
        let top = if highest < filled.min_y {
            filled.min_y - 1
        } else {
            (highest >> 4 << 4) + 15
        };
        let expected = Volume {
            min_x: chunk_x << 4,
            min_y: filled.min_y,
            min_z: chunk_z << 4,
            size_x: 16,
            size_y: (top - filled.min_y + 1).max(1) as usize,
            size_z: 16,
        };
        let mut builder = Builder::new(self, router, filled, heights, expected, biomes, scratch);
        let mut watched = None;
        for x in 0..16 {
            for z in 0..16 {
                builder.column(x, z, watch, &mut watched);
            }
        }
        filled.marks.sort_unstable();
        watched
    }

    /// The game's `MaterialSystem.topMaterial`: what the rule makes of the one block
    /// at `x`, `y`, `z` of the chunk `filled`, taken as the highest block of ground
    /// in its column, with a fluid right above it or none. The carvers ask it for
    /// what to put under what they carved.
    ///
    /// Compared with the game's through the carvers ([`Self::apply_carving_mask`]).
    #[allow(clippy::too_many_arguments)]
    pub fn top_material(
        &self,
        router: &R,
        filled: &mut Filled,
        biomes: &Biomes,
        scratch: &mut Scratch,
        x: i32,
        y: i32,
        z: i32,
        under_fluid: bool,
    ) -> Option<BlockState> {
        let heights = heights(filled);
        let mut builder = Builder::new(self, router, filled, heights, ONE_BLOCK, biomes, scratch);
        builder.top_material(x, y, z, under_fluid)
    }

    /// [`Self::top_material`] at several blocks of one chunk, each its x, y and z and
    /// whether a fluid is right above it: the answers in that order. The chunk's
    /// heights are read once for all.
    pub fn top_materials(
        &self,
        router: &R,
        filled: &mut Filled,
        biomes: &Biomes,
        scratch: &mut Scratch,
        asked: &[(i32, i32, i32, bool)],
    ) -> Vec<Option<BlockState>> {
        let heights = heights(filled);
        let mut builder = Builder::new(self, router, filled, heights, ONE_BLOCK, biomes, scratch);
        asked
            .iter()
            .map(|(x, y, z, under_fluid)| builder.top_material(*x, *y, *z, *under_fluid))
            .collect()
    }

    /// The second half of the game's carving (`applyCarvingMask`): makes of each
    /// block of the chunk `filled` that `mask` names what the chunk's aquifer says is
    /// there when nothing is in the way, which is air, water or lava, or leaves it
    /// where the aquifer says ground, and leaves what is `#minecraft:uncarvable`.
    /// Where grass or mycelium was carved away and dirt is left below what was
    /// carved, the dirt becomes what the material rule makes of the top of the
    /// ground there. A fluid the aquifer wants looked at again is marked, and a
    /// fluid the rule gives.
    ///
    /// The blocks are gone through in the game's order ([`Mask::columns`], each
    /// from its top down), because the rule reads the heights of the columns
    /// beside a block as they are at that moment. `aquifer` is the aquifer of the
    /// chunk's volume; `biomes` are those around the chunk. Gives the number of
    /// blocks the rule was asked for.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_carving_mask(
        &self,
        router: &R,
        aquifer: &mut Aquifer<'_, R>,
        filled: &mut Filled,
        (chunk_x, chunk_z): (i32, i32),
        mask: &Mask,
        biomes: &Biomes,
        scratch: &mut Scratch,
    ) -> usize {
        if mask.is_empty() {
            return 0;
        }
        let heights = heights(filled);
        let mut builder = Builder::new(self, router, filled, heights, ONE_BLOCK, biomes, scratch);
        let (grass, mycelium, dirt) = (
            blocks::GRASS_BLOCK.block_id(),
            blocks::MYCELIUM.block_id(),
            blocks::DIRT.block_id(),
        );
        let mut asked = 0;
        for (x, z, lowest, highest) in mask.columns() {
            let (block_x, block_z) = ((chunk_x << 4) + x as i32, (chunk_z << 4) + z as i32);
            // Whether grass was met in this run of the column, carved or not.
            let mut grass_above = false;
            for y in (lowest..=highest).rev() {
                let state = builder.block(x, y, z);
                let block = state.block_id();
                if block.is_some_and(|block| self.uncarvable[block]) {
                    continue;
                }
                grass_above |= block == grass || block == mycelium;
                let Some(carved) = aquifer.compute_substance(block_x, y, block_z, 0.0) else {
                    continue;
                };
                builder.put(x, y, z, carved);
                if aquifer.should_schedule_fluid_update() && carved.has_fluid() {
                    builder.filled.marks.push(InChunk::new(x, y, z));
                }
                if grass_above && builder.block(x, y - 1, z).block_id() == dirt {
                    asked += 1;
                    let top = builder.top_material(block_x, y - 1, block_z, carved.has_fluid());
                    if let Some(top) = top {
                        // A fluid is marked, as `set` does.
                        builder.set(x, y - 1, z, top);
                    }
                }
            }
        }
        filled.marks.sort_unstable();
        asked
    }
}

/// The volume of a rule that is asked for one block; where it is, is set when it is
/// asked.
const ONE_BLOCK: Volume = Volume {
    min_x: 0,
    min_y: 0,
    min_z: 0,
    size_x: 1,
    size_y: 1,
    size_z: 1,
};

/// For each column of a chunk, in the order of `z << 4 | x`: one more than the height
/// of its highest block that is not air, or the bottom of the terrain. The game's
/// heightmap `WORLD_SURFACE_WG`.
fn heights(filled: &Filled) -> [i32; 256] {
    let mut heights = [filled.min_y; 256];
    for (column, height) in heights.iter_mut().enumerate() {
        for up in (0..filled.height).rev() {
            if !filled.blocks[up << 8 | column].is_air() {
                *height = filled.min_y + up as i32 + 1;
                break;
            }
        }
    }
    heights
}

/// What the rule was asked and answered at a block that was watched.
#[derive(Clone, PartialEq, Debug)]
pub struct Watched {
    /// The block before the rule and what the rule gave.
    pub before: BlockState,
    pub after: Option<BlockState>,
    pub biome: Biome,
    pub surface_depth: i32,
    pub surface_secondary: f64,
    /// The preliminary surface plus the depth minus eight.
    pub min_surface_level: i32,
    pub stone_depth_above: i32,
    pub stone_depth_below: i32,
    /// Where the fluid above the block begins, or the least number for none.
    pub water_height: i32,
    pub slope: (i32, i32),
    /// Each condition the rule asked, in order, with its answer.
    pub asked: Vec<String>,
}

/// The rule's density functions over the volume the game samples them in.
struct Veins<R: NoiseRouter> {
    volume: Volume,
    /// Each interpolated function of the rule's at every block of the volume, by the
    /// function's number.
    values: Vec<(usize, Vec<f32>)>,
    /// What the router remembers while it is asked as part of the volume, and while
    /// it is asked at single blocks. The two do not give the same bits, and what one
    /// remembered is not the other's.
    in_volume: R::Memory,
    alone: R::Memory,
    /// The block of the volume whose values the memory was given last.
    given: Option<usize>,
}

impl<R: NoiseRouter> Veins<R> {
    fn new(system: &System<R>, router: &R, volume: Volume, scratch: &mut Scratch) -> Self {
        let mut memory = R::Memory::default();
        noise_chunk::interpolated(router, &mut memory, &system.in_rule, &volume, scratch);
        memory.in_volume(true);
        Self {
            volume,
            values: system
                .in_rule
                .iter()
                .map(|number| (*number, scratch.interpolated(*number).to_vec()))
                .collect(),
            in_volume: memory,
            alone: R::Memory::default(),
            given: None,
        }
    }

    /// A density function of the rule at a block: as part of the volume
    /// (`in_volume`), or at the block alone. A block outside the volume is asked
    /// alone either way, as the game asks it.
    fn density(
        &mut self,
        router: &R,
        function: usize,
        in_volume: bool,
        x: i32,
        y: i32,
        z: i32,
    ) -> f32 {
        match self.volume.index_of_block(x, y, z) {
            Some(index) if in_volume => {
                if self.given != Some(index) {
                    for (number, values) in &self.values {
                        self.in_volume.give(*number, values[index]);
                    }
                    self.given = Some(index);
                }
                router.material_density(&mut self.in_volume, function, x, y, z)
            }
            _ => router.material_density(&mut self.alone, function, x, y, z),
        }
    }
}

/// The surface of one chunk while it is made: the game's `MaterialRuleContext` with
/// the chunk it works on.
struct Builder<'a, R: NoiseRouter> {
    system: &'a System<R>,
    router: &'a R,
    filled: &'a mut Filled,
    /// The live heightmap: see [`heights`].
    heights: [i32; 256],
    expected: Volume,
    biomes: &'a Biomes,
    scratch: &'a mut Scratch,
    /// Made when they are first asked for.
    surface_level: Option<Vec<f32>>,
    veins: Option<Veins<R>>,
    water: Option<usize>,
    default: Option<usize>,

    // The column.
    x: i32,
    z: i32,
    slope_x: i32,
    slope_z: i32,
    surface_depth: i32,
    surface_secondary: Option<f64>,
    min_surface_level: Option<i32>,
    flat_noises: Vec<Option<f64>>,

    // The block.
    y: i32,
    water_height: i32,
    stone_depth_above: i32,
    stone_depth_below: i32,
    biome: Option<Biome>,
    noises: Vec<Option<f64>>,
}

impl<'a, R: NoiseRouter> Builder<'a, R> {
    fn new(
        system: &'a System<R>,
        router: &'a R,
        filled: &'a mut Filled,
        heights: [i32; 256],
        expected: Volume,
        biomes: &'a Biomes,
        scratch: &'a mut Scratch,
    ) -> Self {
        Self {
            system,
            router,
            filled,
            heights,
            expected,
            biomes,
            scratch,
            surface_level: None,
            veins: None,
            water: blocks::WATER.block_id(),
            default: system.default_block.block_id(),
            x: 0,
            z: 0,
            slope_x: 0,
            slope_z: 0,
            surface_depth: 0,
            surface_secondary: None,
            min_surface_level: None,
            flat_noises: vec![None; system.rule_noises.len()],
            y: 0,
            water_height: i32::MIN,
            stone_depth_above: 0,
            stone_depth_below: 0,
            biome: None,
            noises: vec![None; system.rule_noises.len()],
        }
    }

    #[inline]
    fn block(&self, x: usize, y: i32, z: usize) -> BlockState {
        self.filled.get(x, y, z)
    }

    /// What a block is to the loop. Nearly every block of a filled chunk is air or
    /// the default block, which are known without a look into the table of states.
    #[inline]
    fn kind(&self, state: BlockState) -> Kind {
        if state == blocks::AIR {
            Kind::Air
        } else if state == self.system.default_block {
            Kind::Ground
        } else if state.is_air() {
            Kind::Air
        } else if state.has_fluid() {
            Kind::Fluid
        } else {
            Kind::Ground
        }
    }

    /// The game's `topMaterial` on the chunk as it is now: a context of its own for
    /// the one block, which remembers nothing of an earlier one, and the slope from
    /// the live heights.
    fn top_material(&mut self, x: i32, y: i32, z: i32, under_fluid: bool) -> Option<BlockState> {
        self.expected = Volume {
            min_x: x,
            min_y: y,
            min_z: z,
            ..ONE_BLOCK
        };
        self.surface_level = None;
        self.veins = None;
        let (slope_x, slope_z) = self.slope((x & 15) as usize, (z & 15) as usize);
        self.update_xz(x, z, slope_x, slope_z);
        self.update_y(1, 1, if under_fluid { y + 1 } else { i32::MIN }, y);
        R::Material::apply(self).map(BlockState)
    }

    /// Sets a block as the game's column does: nothing outside the chunk's heights,
    /// a fluid is marked, and the heightmap follows.
    fn set(&mut self, x: usize, y: i32, z: usize, state: BlockState) {
        if y < self.system.chunk_min_y || y > self.system.chunk_max_y {
            return;
        }
        let above = y - self.filled.min_y;
        // A chunk of the Nether or the End is higher than its terrain. Nothing of
        // the game's data puts a block there: no pillar and no iceberg is in those
        // dimensions, and a rule is asked only where a block is.
        debug_assert!(above >= 0 && (above as usize) < self.filled.height);
        if above < 0 || above as usize >= self.filled.height {
            return;
        }
        if state.has_fluid() {
            self.filled.marks.push(InChunk::new(x, y, z));
        }
        self.put(x, y, z, state);
    }

    /// Sets a block of the terrain and has the heightmap follow; nothing is marked.
    fn put(&mut self, x: usize, y: i32, z: usize, state: BlockState) {
        let above = y - self.filled.min_y;
        self.filled.blocks[(above as usize) << 8 | z << 4 | x] = state;
        let height = &mut self.heights[z << 4 | x];
        if state.is_air() {
            if *height == y + 1 {
                let mut top = y - 1;
                while top >= self.filled.min_y
                    && self.filled.blocks[((top - self.filled.min_y) as usize) << 8 | z << 4 | x]
                        .is_air()
                {
                    top -= 1;
                }
                *height = top + 1;
            }
        } else if *height < y + 1 {
            *height = y + 1;
        }
    }

    /// The slope at a column: the differences of the neighbouring columns' heights
    /// along x and along z, the neighbours held within the chunk.
    fn slope(&self, x: usize, z: usize) -> (i32, i32) {
        let height = |x: usize, z: usize| self.heights[z << 4 | x];
        (
            height((x + 1).min(15), z) - height(x.saturating_sub(1), z),
            height(x, (z + 1).min(15)) - height(x, z.saturating_sub(1)),
        )
    }

    fn update_xz(&mut self, x: i32, z: i32, slope_x: i32, slope_z: i32) {
        self.x = x;
        self.z = z;
        self.slope_x = slope_x;
        self.slope_z = slope_z;
        self.surface_depth = self.system.surface_depth(x, z);
        self.surface_secondary = None;
        self.min_surface_level = None;
        self.flat_noises.fill(None);
    }

    fn update_y(&mut self, above: i32, below: i32, water_height: i32, y: i32) {
        self.y = y;
        self.stone_depth_above = above;
        self.stone_depth_below = below;
        self.water_height = water_height;
        self.biome = None;
        self.noises.fill(None);
    }

    fn secondary(&mut self) -> f64 {
        match self.surface_secondary {
            Some(secondary) => secondary,
            None => {
                let secondary = self.system.surface_secondary(self.x, self.z);
                self.surface_secondary = Some(secondary);
                secondary
            }
        }
    }

    fn biome_here(&mut self) -> Biome {
        match self.biome {
            Some(biome) => biome,
            None => {
                let biome = self
                    .biomes
                    .at_block(self.system.zoom, self.x, self.y, self.z);
                self.biome = Some(biome);
                biome
            }
        }
    }

    /// The game's `getMinSurfaceLevel`: from here up the rules for the surface apply.
    fn min_surface_level(&mut self) -> i32 {
        if let Some(level) = self.min_surface_level {
            return level;
        }
        let columns = Volume {
            min_y: 0,
            size_y: 1,
            ..self.expected
        };
        let preliminary = match columns.index_of_block(self.x, 0, self.z) {
            Some(index) => {
                if self.surface_level.is_none() {
                    self.surface_level = Some(self.system.surface_level(
                        self.router,
                        &columns,
                        self.scratch,
                    ));
                }
                match &self.surface_level {
                    Some(levels) => levels[index],
                    None => 0.0,
                }
            }
            // A column outside the volume is asked alone.
            None => self.router.sample(
                &mut R::Memory::default(),
                RouterEntry::ChunkSurfaceLevel,
                self.x,
                0,
                self.z,
            ),
        };
        let level = floor(preliminary) + self.surface_depth - BELOW_PRELIMINARY_SURFACE;
        self.min_surface_level = Some(level);
        level
    }

    /// A density function of the rule at the block: as part of the volume the game
    /// samples at once (`in_volume`), or at the block alone.
    fn density(&mut self, function: usize, in_volume: bool) -> f32 {
        let system = self.system;
        let router = self.router;
        let expected = self.expected;
        let scratch = &mut *self.scratch;
        self.veins
            .get_or_insert_with(|| Veins::new(system, router, expected, scratch))
            .density(router, function, in_volume, self.x, self.y, self.z)
    }

    /// The pillars of the eroded badlands (`erodedBadlandsExtension`): where two
    /// noises say so, the column is raised with the default block to a height that
    /// a third gives.
    fn eroded_badlands_extension(&mut self, x: usize, z: usize, height: i32) {
        let system = self.system;
        let (block_x, block_z) = (f64::from(self.expected_x(x)), f64::from(self.expected_z(z)));
        let surface = f64::from(system.badlands_surface.get(block_x, 0.0, block_z)) * 8.25;
        let pillar = f64::from(
            system
                .badlands_pillar
                .get(block_x * 0.2, 0.0, block_z * 0.2)
                * 15.0,
        );
        let amount = surface.abs().min(pillar);
        if amount <= 0.0 {
            return;
        }
        let roof = (f64::from(system.badlands_pillar_roof.get(
            block_x * 0.75,
            0.0,
            block_z * 0.75,
        )) * 1.5)
            .abs();
        let top = 64.0 + (amount * amount * 2.5).min((roof * 50.0).ceil() + 24.0);
        let top = clustine_noise::math::floor(top);
        if height > top {
            return;
        }
        let min_y = system.chunk_min_y;
        // Nothing is raised over water: the first block that is not air decides.
        let mut y = top;
        while y >= min_y {
            let state = self.block(x, y, z);
            if state.block_id() == self.default {
                break;
            }
            if state.block_id() == self.water {
                return;
            }
            y -= 1;
        }
        let mut y = top;
        while y >= min_y && self.block(x, y, z).is_air() {
            self.set(x, y, z, system.default_block);
            y -= 1;
        }
    }

    fn expected_x(&self, x: usize) -> i32 {
        self.chunk_min_x() + x as i32
    }

    fn expected_z(&self, z: usize) -> i32 {
        self.chunk_min_z() + z as i32
    }

    /// The chunk's lowest block along x and z. (The volume of [`System::top_material`]
    /// is one block; it never raises a pillar or an iceberg.)
    fn chunk_min_x(&self) -> i32 {
        self.expected.min_x >> 4 << 4
    }

    fn chunk_min_z(&self) -> i32 {
        self.expected.min_z >> 4 << 4
    }

    /// The icebergs of the frozen oceans (`frozenOceanExtension`): packed ice from
    /// some way under the sea to some way over it, with snow on top where it is
    /// high, all but a block in a hundred.
    fn frozen_ocean_extension(
        &mut self,
        min_surface_level: i32,
        biome: Biome,
        x: usize,
        z: usize,
        height: i32,
    ) {
        let system = self.system;
        let sea_level = system.settings.sea_level;
        let (in_x, in_z) = (self.expected_x(x), self.expected_z(z));
        let (block_x, block_z) = (f64::from(in_x), f64::from(in_z));
        let surface = f64::from(system.iceberg_surface.get(block_x, 0.0, block_z)) * 8.25;
        let pillar = f64::from(
            system
                .iceberg_pillar
                .get(block_x * 1.28, 0.0, block_z * 1.28)
                * 15.0,
        );
        let amount = surface.abs().min(pillar);
        if amount <= 1.8 {
            return;
        }
        let roof = (f64::from(
            system
                .iceberg_pillar_roof
                .get(block_x * 1.17, 0.0, block_z * 1.17),
        ) * 1.5)
            .abs();
        let mut over = (amount * amount * 1.2).min((roof * 40.0).ceil() + 14.0);
        if system.temperature_at(biome, in_x, sea_level, in_z) > 0.1 {
            over -= 2.0;
        }
        if over <= 2.0 {
            return;
        }
        let under = (f64::from(sea_level) - over - 7.0) as i32;
        let top = (over + f64::from(sea_level)) as i32;
        let mut random = system.random.at(in_x, 0, in_z);
        let snow_blocks = 2 + random.next_i32_bounded(4);
        let snow_above = sea_level + 18 + random.next_i32_bounded(10);
        let mut snow = 0;
        let mut y = height.max(top + 1);
        while y >= min_surface_level {
            let state = self.block(x, y, z);
            let frozen = (state.is_air() && y < top && random.next_f64() > 0.01)
                || (state.block_id() == self.water
                    && y > under
                    && y < sea_level
                    && random.next_f64() > 0.15);
            if frozen {
                if snow <= snow_blocks && y > snow_above {
                    self.set(x, y, z, blocks::SNOW_BLOCK);
                    snow += 1;
                } else {
                    self.set(x, y, z, blocks::PACKED_ICE);
                }
            }
            y -= 1;
        }
    }

    /// One column of the chunk: the game's loop body.
    fn column(
        &mut self,
        x: usize,
        z: usize,
        watch: Option<[i32; 3]>,
        watched: &mut Option<Watched>,
    ) {
        let system = self.system;
        let (block_x, block_z) = (self.expected_x(x), self.expected_z(z));
        let height = self.heights[z << 4 | x];
        let biome = self.biomes.at_block(system.zoom, block_x, height, block_z);
        if biome == system.eroded_badlands {
            self.eroded_badlands_extension(x, z, height);
        }
        let from = self.heights[z << 4 | x];
        let (slope_x, slope_z) = self.slope(x, z);
        self.update_xz(block_x, block_z, slope_x, slope_z);

        let min_y = system.chunk_min_y;
        let mut above = 0;
        let mut water_height = i32::MIN;
        // The height of the lowest block of the run of ground the loop is in.
        let mut run_from = i32::MAX;
        for y in (min_y..=from).rev() {
            let state = self.block(x, y, z);
            match self.kind(state) {
                Kind::Air => {
                    above = 0;
                    water_height = i32::MIN;
                    continue;
                }
                Kind::Fluid => {
                    if water_height == i32::MIN {
                        water_height = y + 1;
                    }
                    continue;
                }
                Kind::Ground => {}
            }
            if run_from >= y {
                run_from = WAY_BELOW_MIN_Y;
                let mut below = y - 1;
                while below >= min_y - 1 {
                    if self.kind(self.block(x, below, z)) != Kind::Ground {
                        run_from = below + 1;
                        break;
                    }
                    below -= 1;
                }
            }
            above += 1;
            self.update_y(above, y - run_from + 1, water_height, y);
            if y > system.chunk_max_y {
                continue;
            }
            let after = if watch == Some([block_x, y, block_z]) {
                let mut asked = Vec::new();
                let after = R::Material::apply(&mut Listening {
                    builder: self,
                    asked: &mut asked,
                });
                *watched = Some(Watched {
                    before: state,
                    after: after.map(BlockState),
                    biome: self.biome_here(),
                    surface_depth: self.surface_depth,
                    surface_secondary: self.secondary(),
                    min_surface_level: self.min_surface_level(),
                    stone_depth_above: self.stone_depth_above,
                    stone_depth_below: self.stone_depth_below,
                    water_height: self.water_height,
                    slope: (self.slope_x, self.slope_z),
                    asked,
                });
                after
            } else {
                R::Material::apply(self)
            };
            if let Some(after) = after {
                self.set(x, y, z, BlockState(after));
            }
        }

        if biome == system.frozen_ocean || biome == system.deep_frozen_ocean {
            let level = self.min_surface_level();
            self.frozen_ocean_extension(level, biome, x, z, height);
        }
    }
}

impl<R: NoiseRouter> Context for Builder<'_, R> {
    fn stone_depth(
        &mut self,
        offset: i32,
        add_surface_depth: bool,
        secondary_depth_range: i32,
        ceiling: bool,
    ) -> bool {
        let depth = if ceiling {
            self.stone_depth_below
        } else {
            self.stone_depth_above
        };
        let surface = if add_surface_depth {
            self.surface_depth
        } else {
            0
        };
        let secondary = if secondary_depth_range == 0 {
            0
        } else {
            map(
                self.secondary(),
                -1.0,
                1.0,
                0.0,
                f64::from(secondary_depth_range),
            ) as i32
        };
        depth <= 1 + offset + surface + secondary
    }

    fn biome(&mut self, biomes: &[u16]) -> bool {
        self.biomes.may_have(biomes) && biomes.contains(&self.biome_here().0)
    }

    fn y_above(
        &mut self,
        anchor: Anchor,
        surface_depth_multiplier: i32,
        add_stone_depth: bool,
    ) -> bool {
        let settings = self.system.settings;
        let stone = if add_stone_depth {
            self.stone_depth_above
        } else {
            0
        };
        self.y + stone
            >= anchor.resolve(settings.min_y, settings.height)
                + self.surface_depth * surface_depth_multiplier
    }

    fn water(&mut self, offset: i32, surface_depth_multiplier: i32, add_stone_depth: bool) -> bool {
        if self.water_height == i32::MIN {
            return true;
        }
        let stone = if add_stone_depth {
            self.stone_depth_above
        } else {
            0
        };
        self.y + stone >= self.water_height + offset + self.surface_depth * surface_depth_multiplier
    }

    fn hole(&mut self) -> bool {
        self.surface_depth <= 0
    }

    fn steep(&mut self) -> bool {
        self.slope_x <= -4 || self.slope_z >= 4
    }

    fn temperature(&mut self) -> bool {
        let biome = self.biome_here();
        self.system
            .cold_enough_to_snow(biome, self.x, self.y, self.z)
    }

    fn above_preliminary_surface(&mut self) -> bool {
        self.y >= self.min_surface_level()
    }

    fn noise(&mut self, number: usize, min: f64, max: f64) -> bool {
        let value = match self.flat_noises[number] {
            Some(value) => value,
            None => {
                let value = f64::from(self.system.rule_noises[number].get(
                    f64::from(self.x),
                    0.0,
                    f64::from(self.z),
                ));
                self.flat_noises[number] = Some(value);
                value
            }
        };
        value >= min && value <= max
    }

    fn noise_3d(&mut self, number: usize, min: f64, max: f64) -> bool {
        let value = match self.noises[number] {
            Some(value) => value,
            None => {
                let value = f64::from(self.system.rule_noises[number].get(
                    f64::from(self.x),
                    f64::from(self.y),
                    f64::from(self.z),
                ));
                self.noises[number] = Some(value);
                value
            }
        };
        value >= min && value <= max
    }

    fn vertical_gradient(
        &mut self,
        random: usize,
        true_at_and_below: Anchor,
        false_at_and_above: Anchor,
    ) -> bool {
        let settings = self.system.settings;
        let always = true_at_and_below.resolve(settings.min_y, settings.height);
        let never = false_at_and_above.resolve(settings.min_y, settings.height);
        if self.y <= always {
            return true;
        }
        if self.y >= never {
            return false;
        }
        let chance = map(
            f64::from(self.y),
            f64::from(always),
            f64::from(never),
            1.0,
            0.0,
        );
        let mut random = self.system.rule_randoms[random].at(self.x, self.y, self.z);
        f64::from(random.next_f32()) < chance
    }

    fn band(&mut self) -> u16 {
        self.system.band(self.x, self.y, self.z).0
    }

    fn ore_vein(&mut self, vein: &Vein) -> Option<u16> {
        let density = self.density(vein.density, true);
        if density <= 0.0 {
            return None;
        }
        let mut random = self.system.ore.at(self.x, self.y, self.z);
        if random.next_f32() > density {
            return None;
        }
        let richness = self.density(vein.richness, true);
        if random.next_f32() < richness && self.density(vein.filler_gap, false) < 0.0 {
            return Some(if random.next_f32() < vein.raw_ore_chance {
                vein.raw_ore
            } else {
                vein.ore
            });
        }
        Some(vein.filler)
    }
}

/// A context that writes down what it is asked, for [`Watched`].
struct Listening<'b, 'a, R: NoiseRouter> {
    builder: &'b mut Builder<'a, R>,
    asked: &'b mut Vec<String>,
}

impl<R: NoiseRouter> Listening<'_, '_, R> {
    fn heard<T: std::fmt::Debug>(&mut self, question: String, answer: T) -> T {
        self.asked.push(format!("{question}: {answer:?}"));
        answer
    }
}

impl<R: NoiseRouter> Context for Listening<'_, '_, R> {
    fn stone_depth(
        &mut self,
        offset: i32,
        add_surface_depth: bool,
        secondary_depth_range: i32,
        ceiling: bool,
    ) -> bool {
        let answer =
            self.builder
                .stone_depth(offset, add_surface_depth, secondary_depth_range, ceiling);
        self.heard(
            format!(
                "stone_depth({offset}, {add_surface_depth}, {secondary_depth_range}, {ceiling})"
            ),
            answer,
        )
    }

    fn biome(&mut self, biomes: &[u16]) -> bool {
        let answer = self.builder.biome(biomes);
        self.heard(format!("biome({biomes:?})"), answer)
    }

    fn y_above(
        &mut self,
        anchor: Anchor,
        surface_depth_multiplier: i32,
        add_stone_depth: bool,
    ) -> bool {
        let answer = self
            .builder
            .y_above(anchor, surface_depth_multiplier, add_stone_depth);
        self.heard(
            format!("y_above({anchor:?}, {surface_depth_multiplier}, {add_stone_depth})"),
            answer,
        )
    }

    fn water(&mut self, offset: i32, surface_depth_multiplier: i32, add_stone_depth: bool) -> bool {
        let answer = self
            .builder
            .water(offset, surface_depth_multiplier, add_stone_depth);
        self.heard(
            format!("water({offset}, {surface_depth_multiplier}, {add_stone_depth})"),
            answer,
        )
    }

    fn hole(&mut self) -> bool {
        let answer = self.builder.hole();
        self.heard("hole".to_owned(), answer)
    }

    fn steep(&mut self) -> bool {
        let answer = self.builder.steep();
        self.heard("steep".to_owned(), answer)
    }

    fn temperature(&mut self) -> bool {
        let answer = self.builder.temperature();
        self.heard("temperature".to_owned(), answer)
    }

    fn above_preliminary_surface(&mut self) -> bool {
        let answer = self.builder.above_preliminary_surface();
        self.heard("above_preliminary_surface".to_owned(), answer)
    }

    fn noise(&mut self, number: usize, min: f64, max: f64) -> bool {
        let answer = self.builder.noise(number, min, max);
        let name = R::Material::NOISES[number].name;
        self.heard(format!("noise({name}, {min}, {max})"), answer)
    }

    fn noise_3d(&mut self, number: usize, min: f64, max: f64) -> bool {
        let answer = self.builder.noise_3d(number, min, max);
        let name = R::Material::NOISES[number].name;
        self.heard(format!("noise_3d({name}, {min}, {max})"), answer)
    }

    fn vertical_gradient(
        &mut self,
        random: usize,
        true_at_and_below: Anchor,
        false_at_and_above: Anchor,
    ) -> bool {
        let answer = self
            .builder
            .vertical_gradient(random, true_at_and_below, false_at_and_above);
        let name = R::Material::RANDOMS[random];
        self.heard(
            format!("vertical_gradient({name}, {true_at_and_below:?}, {false_at_and_above:?})"),
            answer,
        )
    }

    fn band(&mut self) -> u16 {
        let answer = self.builder.band();
        self.heard("band".to_owned(), answer)
    }

    fn ore_vein(&mut self, vein: &Vein) -> Option<u16> {
        let answer = self.builder.ore_vein(vein);
        let density = self.builder.density(vein.density, true);
        let name = R::Material::FUNCTIONS[vein.density];
        self.heard(format!("ore_vein({name} = {density})"), answer)
    }
}

#[cfg(test)]
mod tests {
    use clustine_noise::Xoroshiro;

    use super::*;

    #[test]
    fn the_bands_are_terracotta_of_seven_kinds_and_the_same_for_a_seed() {
        let bands = generate_bands(&mut Xoroshiro::from_seed(5));
        assert_eq!(bands, generate_bands(&mut Xoroshiro::from_seed(5)));
        assert_ne!(bands, generate_bands(&mut Xoroshiro::from_seed(6)));
        let kinds = [
            blocks::TERRACOTTA,
            blocks::ORANGE_TERRACOTTA,
            blocks::YELLOW_TERRACOTTA,
            blocks::BROWN_TERRACOTTA,
            blocks::RED_TERRACOTTA,
            blocks::WHITE_TERRACOTTA,
            blocks::LIGHT_GRAY_TERRACOTTA,
        ];
        assert!(bands.iter().all(|band| kinds.contains(band)));
        assert!(kinds.iter().all(|kind| bands.contains(kind)));
    }

    #[test]
    fn the_floor_of_a_float_goes_down_below_zero() {
        assert_eq!(floor(2.75), 2);
        assert_eq!(floor(-2.25), -3);
        assert_eq!(floor(-3.0), -3);
        assert_eq!(floor(0.0), 0);
    }

    #[test]
    fn a_frozen_ocean_is_warmer_in_patches_and_a_height_is_colder() {
        let temperature = Temperature::new();
        let frozen = BiomeClimate {
            name: "frozen",
            temperature: 0.0,
            frozen: true,
        };
        let mut warm = 0;
        for x in 0..200 {
            let at = temperature.at(&frozen, x * 16, 63, 0, 63);
            assert!(at == 0.0 || at == 0.2);
            warm += usize::from(at == 0.2);
        }
        assert!(warm > 0 && warm < 200);

        let plain = BiomeClimate {
            name: "plain",
            temperature: 0.8,
            frozen: false,
        };
        assert_eq!(temperature.at(&plain, 5, 80, 5, 63), 0.8);
        assert!(temperature.at(&plain, 5, 200, 5, 63) < 0.8);
    }

    #[test]
    fn biomes_hold_a_height_within_the_dimension_and_say_what_may_be() {
        let biomes = Biomes::from_columns(Dimension::Nether, 2, -3, |x, z| {
            let mut column = vec![Biome(7); 64];
            column[63] = Biome((x + z).rem_euclid(3) as u16 + 100);
            column
        });
        assert_eq!(biomes.cell(7, 0, -13), Biome(7));
        assert_eq!(biomes.cell(7, -5, -13), Biome(7));
        assert_eq!(biomes.cell(8, 63, -12), Biome(102));
        assert_eq!(biomes.cell(8, 900, -12), Biome(102));
        assert!(biomes.may_have(&[3, 7]));
        assert!(biomes.may_have(&[101]));
        assert!(!biomes.may_have(&[3, 99, 103, 500]));
    }
}
