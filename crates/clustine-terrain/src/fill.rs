// Written for Clustine from the pinned jar's own classes
// (`NoiseBasedChunkGenerator.doFill`, `iterateNoiseColumn`, `getBaseHeight`,
// `NoiseChunk`). SteelMC's steel-worldgen/src/noise/noise_chunk.rs at 885c4b3
// (AGPL-3.0-or-later, Copyright (C) 2026 Alve Jeansson and contributors; see NOTICE)
// was read beside them.

//! The game's "fill from noise" step: for a seed, a dimension and a chunk, the ground,
//! the air and the fluids, before any surface, cave carver, feature or structure.
//!
//! ```
//! use clustine_terrain::biome::Dimension;
//! use clustine_terrain::fill::{Heightmap, Terrain};
//!
//! let overworld = Terrain::new(13579, Dimension::Overworld);
//! let filled = overworld.fill(0, 0);
//! // The chunk at the origin of that world is sea over stone.
//! assert_eq!(filled.get(0, 62, 0), clustine_data::blocks::WATER);
//! assert_eq!(filled.get(0, -64, 0), clustine_data::blocks::STONE);
//! // The same column asked alone, without filling a chunk.
//! assert_eq!(overworld.base_height(0, 0, Heightmap::WorldSurfaceWg), 63);
//! assert_eq!(overworld.base_height(0, 0, Heightmap::OceanFloorWg), 55);
//! ```
//!
//! **What the step is in the pinned jar** (`NoiseBasedChunkGenerator.doFill`): the
//! router's final density of every block of the chunk as one volume
//! ([`crate::noise_chunk`]); then, column by column from the top, the aquifer's answer
//! for the block ([`crate::aquifer`]): the dimension's default block where it says
//! ground, nothing where it says air, else the fluid. A fluid the aquifer wants looked
//! at again is marked for post-processing ([`Filled::marks`]). The game also updates
//! the two heightmaps of world generation as it sets blocks; `clustine_world::Chunk`
//! makes its heightmaps from the blocks, so none are given here.
//!
//! **Ore veins are not part of this step in 26.3.** They are a material rule
//! (`OreVeinRule`) and come with the surface (step T5 of
//! `docs/groundwork/terrain-plan.md`). No structure bends the ground here either: the
//! beardifier adds nothing until structures have starts (T7b), and old chunks are not
//! blended in.
//!
//! **What it was compared with**, in October 2026: `tests/java/GameFill.java` has the
//! game's own classes do this step, without a server, and `tests/game_fill.rs` holds
//! every block, every mark, the bits of the density and base heights to it. The head
//! of that test has the numbers.

use clustine_data::{BlockState, DimensionType, blocks};
use clustine_noise::RandomSplitter;
use clustine_world::{BLOCKS_PER_SECTION, Biome, Chunk, InChunk, SECTION_SIZE, Section};
use clustine_worldgen_data::{
    EndRouter, NetherRouter, NoiseRouter, NoiseSettings, OverworldRouter, density,
};

use crate::aquifer::{self, Aquifer};
use crate::biome::Dimension;
use crate::noise_chunk::{self, Scratch, Volume};

/// The two heightmaps the game keeps while it generates, by what counts as ground.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Heightmap {
    /// The highest block of the tag `#minecraft:blocks_motion_in_heightmap`: the
    /// ground under a sea.
    OceanFloorWg,
    /// The highest block that is not air: the top of a sea.
    WorldSurfaceWg,
}

impl Heightmap {
    fn counts(self, state: BlockState) -> bool {
        match self {
            Heightmap::OceanFloorWg => state.props().in_motion_heightmap(),
            Heightmap::WorldSurfaceWg => !state.is_air(),
        }
    }
}

/// A chunk after the fill.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Filled {
    /// The lowest block of the dimension's terrain and how many blocks high it is.
    /// The Nether's and the End's terrain is lower than their dimension.
    pub min_y: i32,
    pub height: usize,
    /// The blocks, in the order of `(y - min_y) << 8 | z << 4 | x`: section after
    /// section from the bottom, each in the order `clustine_world::Section` takes.
    pub blocks: Vec<BlockState>,
    /// The fluids the game marks for post-processing at this step: when the chunk is
    /// first ticked, each is to flow a step. Ascending.
    pub marks: Vec<InChunk>,
}

impl Filled {
    /// The block at `x` and `z` (each in `0..16`) and the height `y`; air outside the
    /// terrain's heights.
    #[must_use]
    pub fn get(&self, x: usize, y: i32, z: usize) -> BlockState {
        let above = y - self.min_y;
        if above < 0 || above as usize >= self.height {
            return blocks::AIR;
        }
        self.blocks[(above as usize) << 8 | z << 4 | x]
    }

    /// The sections of a chunk of the dimension `kind`, from its bottom to its top,
    /// with one biome throughout: the terrain's sections, and air where the dimension
    /// is higher or lower than its terrain.
    #[must_use]
    pub fn sections(&self, kind: &DimensionType, biome: Biome) -> Vec<Section> {
        (0..kind.height as usize / SECTION_SIZE)
            .map(|section| {
                let bottom = kind.min_y + (section * SECTION_SIZE) as i32;
                let from = bottom - self.min_y;
                if from < 0 || from as usize + SECTION_SIZE > self.height {
                    return Section::filled(blocks::AIR, biome);
                }
                let start = (from as usize) << 8;
                let states: Box<[BlockState; BLOCKS_PER_SECTION]> = self.blocks
                    [start..start + BLOCKS_PER_SECTION]
                    .to_vec()
                    .into_boxed_slice()
                    .try_into()
                    .expect("a section's worth of blocks was taken");
                Section::from_states(states, biome)
            })
            .collect()
    }

    /// The chunk of the dimension `kind` with these blocks and marks, one biome
    /// throughout, and the heightmaps `Chunk` makes of the blocks.
    #[must_use]
    pub fn into_chunk(self, kind: &DimensionType, biome: Biome) -> Chunk {
        let mut chunk = Chunk::from_sections(kind.min_y, self.sections(kind, biome));
        chunk.set_marks(self.marks);
        chunk
    }
}

/// The noise chunk of one volume: the router, what it remembers, the volume's final
/// density and its aquifer. The fill reads the last two; the surface rules and the
/// ore veins of the next step are computed over the same.
pub struct NoiseChunk<'a, R: NoiseRouter> {
    pub router: &'a R,
    pub volume: Volume,
    /// The final density of every block, in the volume's order ([`Volume::index`]).
    pub density: Vec<f32>,
    pub aquifer: Aquifer<'a, R>,
}

impl<'a, R: NoiseRouter> NoiseChunk<'a, R> {
    /// The noise chunk of `volume`. `positions` is [`aquifer::positions`] of the
    /// dimension's factory of random numbers.
    #[must_use]
    pub fn new(
        router: &'a R,
        positions: &RandomSplitter,
        volume: Volume,
        scratch: &mut Scratch,
    ) -> Self {
        // The game makes the aquifer first, with the noise chunk; neither reads what
        // the other computed.
        let aquifer = Aquifer::new(router, positions, &volume);
        let mut density = Vec::new();
        noise_chunk::final_density(
            router,
            &mut R::Memory::default(),
            &volume,
            scratch,
            &mut density,
        );
        Self {
            router,
            volume,
            density,
            aquifer,
        }
    }
}

/// The shape of one dimension's ground for one seed.
struct Shape<R: NoiseRouter> {
    router: R,
    /// The factory of the aquifers' positions.
    positions: RandomSplitter,
}

impl<R: NoiseRouter> Shape<R> {
    fn new(seed: i64) -> Self {
        let router = R::new(seed);
        let dimension = density::splitter(seed, router.settings().legacy_random_source);
        Self {
            positions: aquifer::positions(&dimension),
            router,
        }
    }

    fn settings(&self) -> &'static NoiseSettings {
        self.router.settings()
    }

    fn default_block(&self) -> BlockState {
        BlockState(self.settings().default_block as u16)
    }

    fn noise_chunk(&self, volume: Volume, scratch: &mut Scratch) -> NoiseChunk<'_, R> {
        NoiseChunk::new(&self.router, &self.positions, volume, scratch)
    }

    fn fill(&self, chunk_x: i32, chunk_z: i32, scratch: &mut Scratch) -> (Filled, Vec<f32>) {
        let settings = self.settings();
        let volume = Volume::of_chunk(chunk_x, chunk_z, settings.min_y, settings.height);
        let mut noise_chunk = self.noise_chunk(volume, scratch);
        let default_block = self.default_block();

        let mut blocks = vec![blocks::AIR; volume.len()];
        let mut marks = Vec::new();
        // The game's order: z, then x, then down the column.
        for z in 0..volume.size_z {
            let block_z = volume.min_z + z as i32;
            for x in 0..volume.size_x {
                let block_x = volume.min_x + x as i32;
                for y in (0..volume.size_y).rev() {
                    let block_y = volume.min_y + y as i32;
                    let density = noise_chunk.density[volume.index(x, y, z)];
                    let state = noise_chunk
                        .aquifer
                        .compute_substance(block_x, block_y, block_z, f64::from(density))
                        .unwrap_or(default_block);
                    if state == blocks::AIR {
                        continue;
                    }
                    blocks[y << 8 | z << 4 | x] = state;
                    if noise_chunk.aquifer.should_schedule_fluid_update() && state.has_fluid() {
                        marks.push(InChunk::new(x, block_y, z));
                    }
                }
            }
        }
        marks.sort_unstable();
        let filled = Filled {
            min_y: volume.min_y,
            height: volume.size_y,
            blocks,
            marks,
        };
        (filled, noise_chunk.density)
    }

    /// The column at a block's x and z as the game makes it alone
    /// (`iterateNoiseColumn`), from the bottom of the terrain.
    fn base_column(&self, x: i32, z: i32) -> Vec<BlockState> {
        let settings = self.settings();
        let volume = Volume::of_column(x, z, settings.min_y, settings.height);
        let mut noise_chunk = self.noise_chunk(volume, &mut Scratch::default());
        let default_block = self.default_block();
        let mut column = vec![blocks::AIR; volume.size_y];
        for y in (0..volume.size_y).rev() {
            column[y] = noise_chunk
                .aquifer
                .compute_substance(
                    x,
                    volume.min_y + y as i32,
                    z,
                    f64::from(noise_chunk.density[y]),
                )
                .unwrap_or(default_block);
        }
        column
    }

    fn base_height(&self, x: i32, z: i32, heightmap: Heightmap) -> i32 {
        let min_y = self.settings().min_y;
        self.base_column(x, z)
            .iter()
            .rposition(|state| heightmap.counts(*state))
            .map_or(min_y, |y| min_y + y as i32 + 1)
    }

    fn final_density(&self, volume: &Volume, scratch: &mut Scratch) -> Vec<f32> {
        let mut density = Vec::new();
        noise_chunk::final_density(
            &self.router,
            &mut R::Memory::default(),
            volume,
            scratch,
            &mut density,
        );
        density
    }
}

/// A router with its noises is some thousand bytes, and the three differ much in
/// size, so each is kept behind a pointer.
enum Kind {
    Overworld(Box<Shape<OverworldRouter>>),
    Nether(Box<Shape<NetherRouter>>),
    End(Box<Shape<EndRouter>>),
}

/// Calls a method of the dimension's shape, whichever router it has.
macro_rules! shape {
    ($terrain:expr, $shape:ident => $call:expr) => {
        match &$terrain.kind {
            Kind::Overworld($shape) => $call,
            Kind::Nether($shape) => $call,
            Kind::End($shape) => $call,
        }
    };
}

/// The ground of one dimension of a world, before surface, caves and everything
/// later: a function of the seed and the position. Making one seeds the dimension's
/// noises; keep it for as long as terrain of the world is asked for.
pub struct Terrain {
    dimension: Dimension,
    kind: Kind,
}

impl Terrain {
    #[must_use]
    pub fn new(seed: i64, dimension: Dimension) -> Self {
        let kind = match dimension {
            Dimension::Overworld => Kind::Overworld(Box::new(Shape::new(seed))),
            Dimension::Nether => Kind::Nether(Box::new(Shape::new(seed))),
            Dimension::End => Kind::End(Box::new(Shape::new(seed))),
        };
        Self { dimension, kind }
    }

    #[must_use]
    pub fn dimension(&self) -> Dimension {
        self.dimension
    }

    /// The dimension's noise settings: the heights of its terrain, its sea level, its
    /// default block and fluid.
    #[must_use]
    pub fn settings(&self) -> &'static NoiseSettings {
        shape!(self, shape => shape.settings())
    }

    /// The blocks of the chunk after the game's "fill from noise" step, and the
    /// positions it marks.
    #[must_use]
    pub fn fill(&self, chunk_x: i32, chunk_z: i32) -> Filled {
        self.fill_with_density(chunk_x, chunk_z, &mut Scratch::default())
            .0
    }

    /// [`Self::fill`], and with the blocks the final density they were made from: of
    /// every block of the chunk's terrain, in the order of [`Volume::index`] of
    /// [`Self::volume`]. `scratch` is kept between calls to save allocations and
    /// holds the corners afterwards.
    #[must_use]
    pub fn fill_with_density(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        scratch: &mut Scratch,
    ) -> (Filled, Vec<f32>) {
        shape!(self, shape => shape.fill(chunk_x, chunk_z, scratch))
    }

    /// The blocks of a chunk's terrain as a volume: what the game fills at once.
    #[must_use]
    pub fn volume(&self, chunk_x: i32, chunk_z: i32) -> Volume {
        let settings = self.settings();
        Volume::of_chunk(chunk_x, chunk_z, settings.min_y, settings.height)
    }

    /// The game's `getBaseHeight`: one more than the height of the highest block of
    /// the column that counts for the heightmap, or the bottom of the terrain where
    /// none does. No chunk is filled for it: the game makes the column alone, with an
    /// aquifer of its own, **and the answer is not always the column's in its filled
    /// chunk** (see [`crate::aquifer`]). Structures ask it for where the ground is
    /// before there is a chunk.
    #[must_use]
    pub fn base_height(&self, x: i32, z: i32, heightmap: Heightmap) -> i32 {
        shape!(self, shape => shape.base_height(x, z, heightmap))
    }

    /// The game's `getBaseColumn`: the blocks of the column at a block's x and z, from
    /// the bottom of the terrain, made alone as for [`Self::base_height`].
    #[must_use]
    pub fn base_column(&self, x: i32, z: i32) -> Vec<BlockState> {
        shape!(self, shape => shape.base_column(x, z))
    }

    /// The router's final density at every block of a volume, in the volume's order,
    /// as the game's volumes have it; `scratch` is kept between calls to save
    /// allocations and holds the corners afterwards.
    #[must_use]
    pub fn final_density(&self, volume: &Volume, scratch: &mut Scratch) -> Vec<f32> {
        shape!(self, shape => shape.final_density(volume, scratch))
    }

    /// The numbers of the interpolated functions the final density reaches, each with
    /// its cell sizes: across, and up.
    #[must_use]
    pub fn interpolated(&self) -> Vec<(usize, i32, i32)> {
        let all = shape!(self, shape => shape.router.interpolated());
        all.iter()
            .enumerate()
            .filter(|(_, function)| function.in_final_density)
            .map(|(number, function)| (number, function.cell_size_xz, function.cell_size_y))
            .collect()
    }

    /// The height above which the aquifer of the chunk answers by the sea level
    /// alone; `None` in a dimension without aquifers.
    #[must_use]
    pub fn aquifer_ceiling(&self, chunk_x: i32, chunk_z: i32) -> Option<i32> {
        shape!(self, shape => {
            let settings = shape.settings();
            let volume = Volume::of_chunk(chunk_x, chunk_z, settings.min_y, settings.height);
            Aquifer::new(&shape.router, &shape.positions, &volume).skip_sampling_above_y()
        })
    }
}
