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
//! (`OreVeinRule`) and come with the surface ([`crate::surface`],
//! [`Terrain::surface`]). No structure bends the ground here either: the
//! beardifier adds nothing until structures have starts (T7b), and old chunks are not
//! blended in.
//!
//! **What it was compared with**, in October 2026: `tests/java/GameFill.java` has the
//! game's own classes do this step, without a server, and `tests/game_fill.rs` holds
//! every block, every mark, the bits of the density and base heights to it. The head
//! of that test has the numbers.

use clustine_data::{BlockState, DIMENSION_TYPES, DimensionType, blocks};
use clustine_noise::RandomSplitter;
use clustine_world::{BLOCKS_PER_SECTION, Biome, Chunk, InChunk, SECTION_SIZE, Section};
use clustine_worldgen_data::{
    EndRouter, NetherRouter, NoiseRouter, NoiseSettings, OverworldRouter, density,
};

use crate::aquifer::{self, Aquifer};
use crate::biome::zoom::Zoom;
use crate::biome::{BiomeSource, Dimension};
use crate::carver::{self, Heights, Mask, Origin};
use crate::noise_chunk::{self, Scratch, Volume};
use crate::surface::{self, Biomes, Watched};

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
    /// first ticked, each is to flow a step. Ascending. A position stands as often
    /// as the game marked it: the carvers mark a fluid of the fill again where they
    /// reach it (`clustine_world::Chunk` keeps each once).
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

/// A chunk after the fill, the surface and the carvers, which is what the
/// game's `buildTerrain` makes where no structure bends the ground.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Carved {
    pub chunk: Filled,
    /// Which blocks the carvers reached, whatever became of them. The game keeps
    /// none after the step; it is here for looking into what was carved.
    pub mask: Mask,
}

/// The noise chunk of one volume: the router, what it remembers, the volume's final
/// density and its aquifer. The fill reads the last two. (The surface step reads
/// neither: what it asks of the router it asks anew.)
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
    /// What the surface is made with.
    surface: surface::System<R>,
}

impl<R: NoiseRouter> Shape<R> {
    fn new(seed: i64, dimension: Dimension) -> Self {
        let router = R::new(seed);
        let random = density::splitter(seed, router.settings().legacy_random_source);
        Self {
            positions: aquifer::positions(&random),
            surface: surface::System::new(seed, dimension, &router),
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

    /// The second half of the carving, with an aquifer made for it: the game uses
    /// the one of the fill, which answers the same, because what an aquifer
    /// remembers changes no answer.
    fn apply_carving_mask(
        &self,
        filled: &mut Filled,
        chunk: (i32, i32),
        mask: &Mask,
        biomes: &Biomes,
        scratch: &mut Scratch,
    ) -> usize {
        let settings = self.settings();
        let volume = Volume::of_chunk(chunk.0, chunk.1, settings.min_y, settings.height);
        let mut aquifer = Aquifer::new(&self.router, &self.positions, &volume);
        self.surface.apply_carving_mask(
            &self.router,
            &mut aquifer,
            filled,
            chunk,
            mask,
            biomes,
            scratch,
        )
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
    seed: i64,
    dimension: Dimension,
    kind: Kind,
}

impl Terrain {
    #[must_use]
    pub fn new(seed: i64, dimension: Dimension) -> Self {
        let kind = match dimension {
            Dimension::Overworld => Kind::Overworld(Box::new(Shape::new(seed, dimension))),
            Dimension::Nether => Kind::Nether(Box::new(Shape::new(seed, dimension))),
            Dimension::End => Kind::End(Box::new(Shape::new(seed, dimension))),
        };
        Self {
            seed,
            dimension,
            kind,
        }
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

    /// The chunk after the game's "build surface" step: [`Self::fill`], and then the
    /// ground made what the dimension's material rule says, with the ore veins, the
    /// pillars of the eroded badlands and the icebergs ([`crate::surface`]). The
    /// marks are the fill's and those of the fluids the rule put. `biomes` are the
    /// biomes around the chunk.
    #[must_use]
    pub fn surface(&self, chunk_x: i32, chunk_z: i32, biomes: &Biomes) -> Filled {
        let mut scratch = Scratch::default();
        let mut filled = self.fill_with_density(chunk_x, chunk_z, &mut scratch).0;
        self.build_surface(&mut filled, chunk_x, chunk_z, biomes, &mut scratch);
        filled
    }

    /// The game's "build surface" step alone, on a chunk that [`Self::fill`] made.
    /// Nothing of the fill but its blocks and marks is read: what the step asks of
    /// the noises it asks anew. `scratch` is kept between calls to save allocations.
    pub fn build_surface(
        &self,
        filled: &mut Filled,
        chunk_x: i32,
        chunk_z: i32,
        biomes: &Biomes,
        scratch: &mut Scratch,
    ) {
        shape!(self, shape => {
            shape.surface.build(&shape.router, filled, (chunk_x, chunk_z), biomes, scratch, None);
        });
    }

    /// [`Self::build_surface`], and what the rule was asked and answered at the block
    /// `watch` (its x, y and z): for looking into a difference. `None` where the rule
    /// was not asked there, which is where the block was air or a fluid.
    pub fn build_surface_watching(
        &self,
        filled: &mut Filled,
        chunk_x: i32,
        chunk_z: i32,
        biomes: &Biomes,
        watch: [i32; 3],
    ) -> Option<Watched> {
        let mut scratch = Scratch::default();
        shape!(self, shape => shape.surface.build(
            &shape.router,
            filled,
            (chunk_x, chunk_z),
            biomes,
            &mut scratch,
            Some(watch),
        ))
    }

    /// The heights the carvers read (the game's `WorldGenerationContext`): the
    /// terrain's, held within the dimension's.
    #[must_use]
    pub fn carver_heights(&self) -> Heights {
        let settings = self.settings();
        let kind = DIMENSION_TYPES
            .iter()
            .find(|kind| kind.name == self.dimension.type_name())
            .expect("the three dimension types are in the game's registry");
        Heights {
            min_y: settings.min_y.max(kind.min_y),
            depth: settings.height.min(kind.height as i32),
        }
    }

    /// The first half of the game's carving: which blocks of the chunk the caves
    /// and ravines of the chunks up to eight away reach ([`crate::carver`]). A
    /// function of the seed and the chunk's position: `biomes` is asked only for
    /// what a source chunk's carvers are.
    #[must_use]
    pub fn carving_mask(&self, chunk_x: i32, chunk_z: i32, biomes: &BiomeSource) -> Mask {
        self.carving_mask_with(chunk_x, chunk_z, |x, z| biomes.biome(x * 4, 0, z * 4))
    }

    /// [`Self::carving_mask`] with the biome of each source chunk from whoever has
    /// it: `biome_of_source` gives, for a chunk, the biome of the cell at its lowest
    /// corner and the height 0, as the dimension's source of biomes gives it.
    #[must_use]
    pub fn carving_mask_with(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        biome_of_source: impl FnMut(i32, i32) -> Biome,
    ) -> Mask {
        carver::mask(
            self.seed,
            self.carver_heights(),
            (chunk_x, chunk_z),
            biome_of_source,
        )
    }

    /// Which carvers of which source chunks reached a block of the chunk (`x` and
    /// `z` in `0..16`): for looking into a difference.
    #[must_use]
    pub fn carved_by(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        biome_of_source: impl FnMut(i32, i32) -> Biome,
        block: (usize, i32, usize),
    ) -> Vec<Origin> {
        carver::origins(
            self.seed,
            self.carver_heights(),
            (chunk_x, chunk_z),
            biome_of_source,
            block,
        )
    }

    /// Every carver that starts in a chunk up to eight away, with the number of the
    /// chunk's blocks it reaches ([`carver::started`]).
    #[must_use]
    pub fn carvers_started(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        biome_of_source: impl FnMut(i32, i32) -> Biome,
    ) -> Vec<(Origin, usize)> {
        carver::started(
            self.seed,
            self.carver_heights(),
            (chunk_x, chunk_z),
            biome_of_source,
        )
    }

    /// The second half of the game's carving (`applyCarvingMask`), on a chunk that
    /// [`Self::surface`] made: each block `mask` names becomes what the chunk's
    /// aquifer says is there when nothing is in the way (air, water or lava) or
    /// stays where that is ground, bedrock stays, and dirt left under carved grass
    /// becomes the top of the ground. `biomes` are those around the chunk, as for
    /// the surface. Gives how often the material rule was asked. `scratch` is kept
    /// between calls to save allocations.
    ///
    /// The game asks the rule for the biome at a block through its source of
    /// biomes, not through the chunks' stored cells as the surface does. The two
    /// are the same cells but for the lowest two blocks of a dimension, where a
    /// chunk holds the height within its own; no dirt lies under grass there.
    pub fn apply_carving_mask(
        &self,
        filled: &mut Filled,
        chunk_x: i32,
        chunk_z: i32,
        mask: &Mask,
        biomes: &Biomes,
        scratch: &mut Scratch,
    ) -> usize {
        shape!(self, shape => {
            shape.apply_carving_mask(filled, (chunk_x, chunk_z), mask, biomes, scratch)
        })
    }

    /// The chunk after the game's fill, surface and carvers: its `buildTerrain`
    /// where no structure bends the ground. [`Self::surface`], then
    /// [`Self::carving_mask`] applied ([`Self::apply_carving_mask`]). `biomes` are
    /// the biomes around the chunk and `source` the dimension's source of biomes.
    #[must_use]
    pub fn carved(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        biomes: &Biomes,
        source: &BiomeSource,
    ) -> Carved {
        let mut scratch = Scratch::default();
        let mut chunk = self.fill_with_density(chunk_x, chunk_z, &mut scratch).0;
        self.build_surface(&mut chunk, chunk_x, chunk_z, biomes, &mut scratch);
        let mask = self.carving_mask(chunk_x, chunk_z, source);
        self.apply_carving_mask(&mut chunk, chunk_x, chunk_z, &mask, biomes, &mut scratch);
        Carved { chunk, mask }
    }

    /// The game's `MaterialSystem.topMaterial`: what the material rule makes of the
    /// block at `x`, `y`, `z` of the chunk `filled` if it is taken as the highest
    /// block of ground of its column, with a fluid right above it (`under_fluid`) or
    /// none. The carvers ask it for what to put under what they carved, and it was
    /// compared with the game's through them ([`Self::apply_carving_mask`]).
    #[must_use]
    pub fn top_material(
        &self,
        filled: &mut Filled,
        biomes: &Biomes,
        x: i32,
        y: i32,
        z: i32,
        under_fluid: bool,
    ) -> Option<BlockState> {
        let mut scratch = Scratch::default();
        shape!(self, shape => shape.surface.top_material(
            &shape.router,
            filled,
            biomes,
            &mut scratch,
            x,
            y,
            z,
            under_fluid,
        ))
    }

    /// [`Self::top_material`] at several blocks of one chunk, each its x, y and z and
    /// whether a fluid is right above it: the answers in that order.
    #[must_use]
    pub fn top_materials(
        &self,
        filled: &mut Filled,
        biomes: &Biomes,
        asked: &[(i32, i32, i32, bool)],
    ) -> Vec<Option<BlockState>> {
        let mut scratch = Scratch::default();
        shape!(self, shape => {
            shape.surface.top_materials(&shape.router, filled, biomes, &mut scratch, asked)
        })
    }

    /// Which cell of biomes a block of this world takes its biome from.
    #[must_use]
    pub fn zoom(&self) -> Zoom {
        shape!(self, shape => shape.surface.zoom())
    }

    /// The bands of terracotta of the world's badlands, from the lowest; they repeat.
    #[must_use]
    pub fn bands(&self) -> Vec<BlockState> {
        shape!(self, shape => shape.surface.bands().to_vec())
    }

    /// The band of terracotta at a block.
    #[must_use]
    pub fn band(&self, x: i32, y: i32, z: i32) -> BlockState {
        shape!(self, shape => shape.surface.band(x, y, z))
    }

    /// How many blocks deep the surface is at a column, and the second noise of its
    /// depth.
    #[must_use]
    pub fn surface_depth(&self, x: i32, z: i32) -> (i32, f64) {
        shape!(self, shape => (
            shape.surface.surface_depth(x, z),
            shape.surface.surface_secondary(x, z),
        ))
    }

    /// The router's `chunk_surface_level` at the 256 columns of a chunk, in the order
    /// of `z << 4 | x`, as the surface rules sample it: a volume one block high.
    #[must_use]
    pub fn surface_level(&self, chunk_x: i32, chunk_z: i32) -> Vec<f32> {
        let columns = self.volume(chunk_x, chunk_z);
        shape!(self, shape => {
            shape.surface.surface_level(&shape.router, &columns, &mut Scratch::default())
        })
    }

    /// Each density function the dimension's material rule names (the ore veins'),
    /// at every block of `volume`, in the volume's order: its name, its values as the
    /// game samples the volume at once, and its values at the blocks alone.
    #[must_use]
    pub fn material_densities(&self, volume: &Volume) -> Vec<(&'static str, Vec<f32>, Vec<f32>)> {
        shape!(self, shape => {
            shape.surface.densities(&shape.router, volume, &mut Scratch::default())
        })
    }

    /// Whether it is cold enough to snow at a block of a biome.
    #[must_use]
    pub fn cold_enough_to_snow(
        &self,
        biome: clustine_world::Biome,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        shape!(self, shape => shape.surface.cold_enough_to_snow(biome, x, y, z))
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
