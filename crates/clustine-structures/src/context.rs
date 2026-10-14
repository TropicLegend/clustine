// The trait follows SteelMC's `StructureGenerationContext`
// (steel-worldgen/src/structure/generation.rs at commit
// 885c4b3e60ed79862c37311780774f76806cb714), so that the structure code adapted from
// SteelMC asks the same questions; what answers them here is `clustine-terrain`, and is
// Clustine's own. See NOTICE in this crate.

//! What a structure asks of the world while it looks for a place to start: the
//! chunk it starts in, the heights of the dimension, the ground the game's generator
//! would give at a column before there is a chunk (`getBaseHeight`, `getBaseColumn`),
//! and the biome at a block.

use std::cell::RefCell;
use std::collections::BTreeMap;

use clustine_data::BlockState;
use clustine_terrain::biome::BiomeSource;
use clustine_terrain::fill::{Heightmap, Terrain};

use crate::compat::{BiomeRef, TemplateLookup};

/// What a block of a column of terrain is to a structure.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColumnBlock {
    Air,
    Fluid,
    Solid,
}

/// What a structure asks while it makes its start.
pub trait StructureGenerationContext {
    fn seed(&self) -> i64;
    fn chunk_x(&self) -> i32;
    fn chunk_z(&self) -> i32;
    fn chunk_min_x(&self) -> i32 {
        self.chunk_x() * 16
    }
    fn chunk_min_z(&self) -> i32 {
        self.chunk_z() * 16
    }
    /// The game's `ChunkPos.getMiddleBlockX`.
    fn center_block_x(&self) -> i32 {
        self.chunk_min_x() + 8
    }
    fn center_block_z(&self) -> i32 {
        self.chunk_min_z() + 8
    }
    fn sea_level(&self) -> i32;
    /// The lowest block the generator fills (`ChunkGenerator.getMinY`): of the
    /// terrain, which in the Nether and the End is lower than the dimension is high.
    fn min_y(&self) -> i32;
    /// How many blocks the generator fills (`getGenDepth`).
    fn height(&self) -> i32;
    /// One above the highest block the generator fills.
    fn max_y(&self) -> i32 {
        self.min_y() + self.height()
    }
    /// The lowest block of the dimension (the game's `LevelHeightAccessor.getMinY`).
    fn level_min_y(&self) -> i32;
    /// How many blocks high the dimension is.
    fn level_height(&self) -> i32;
    /// One above the highest block of the dimension.
    fn level_max_y(&self) -> i32 {
        self.level_min_y() + self.level_height()
    }
    /// The table of structure templates.
    fn templates(&self) -> &'static TemplateLookup {
        &TemplateLookup
    }
    /// The game's `getBaseHeight` (which is `getFirstFreeHeight`): one above the
    /// highest block that counts, for the world's surface or the ocean's floor.
    fn base_height(&mut self, x: i32, z: i32, ocean_floor: bool) -> i32;
    /// The same; SteelMC tells two ways of computing it apart, the game has one.
    fn base_height_full(&mut self, x: i32, z: i32, ocean_floor: bool) -> i32 {
        self.base_height(x, z, ocean_floor)
    }
    /// The biome at a block, as the game's biome source gives it for the block's
    /// cell: no blur.
    fn biome_at(&mut self, block_x: i32, block_y: i32, block_z: i32) -> BiomeRef;
    /// What the game's `getBaseColumn` has at a height; air beyond the terrain.
    fn column_state(&mut self, x: i32, y: i32, z: i32) -> ColumnBlock;
    /// The block itself of the game's `getBaseColumn` at a height.
    fn column_block(&mut self, x: i32, y: i32, z: i32) -> BlockState;
    /// The highest solid block of a column below `start_y` and not below
    /// `min_solid_y` that has air above it.
    fn solid_block_below_air(
        &mut self,
        x: i32,
        z: i32,
        start_y: i32,
        min_solid_y: i32,
    ) -> Option<i32> {
        if start_y <= min_solid_y {
            return None;
        }
        let mut above = self.column_state(x, start_y, z);
        for y in (min_solid_y..start_y).rev() {
            let current = self.column_state(x, y, z);
            if above == ColumnBlock::Air && current == ColumnBlock::Solid {
                return Some(y);
            }
            above = current;
        }
        None
    }
    /// The game's `getFirstOccupiedHeight` at the middle of the chunk for the world's
    /// surface.
    fn surface_y(&mut self) -> i32 {
        let (x, z) = (self.center_block_x(), self.center_block_z());
        self.base_height(x, z, false) - 1
    }
    /// [`Self::base_height`] through a shared reference.
    fn terrain_surface_height(&self, x: i32, z: i32, ocean_floor: bool) -> i32;
    /// Whether the column's block at a height counts for the heightmap.
    fn terrain_is_opaque(&self, x: i32, y: i32, z: i32, ocean_floor: bool) -> bool;
    /// Whether it is cold enough to snow at a block of a biome
    /// (`Biome.coldEnoughToSnow`).
    fn cold_enough_to_snow(&self, biome: BiomeRef, x: i32, y: i32, z: i32) -> bool;
}

/// The context over the terrain and the biomes of a dimension, for one chunk. It
/// remembers the columns it was asked for.
pub struct TerrainContext<'a> {
    seed: i64,
    chunk_x: i32,
    chunk_z: i32,
    terrain: &'a Terrain,
    biomes: &'a BiomeSource,
    dimension_min_y: i32,
    dimension_height: i32,
    columns: RefCell<BTreeMap<(i32, i32), Vec<BlockState>>>,
    heights: RefCell<BTreeMap<(i32, i32, bool), i32>>,
}

impl<'a> TerrainContext<'a> {
    #[must_use]
    pub fn new(
        seed: i64,
        chunk_x: i32,
        chunk_z: i32,
        terrain: &'a Terrain,
        biomes: &'a BiomeSource,
    ) -> Self {
        let (lowest_section, sections) = terrain.dimension().sections();
        Self {
            seed,
            chunk_x,
            chunk_z,
            terrain,
            biomes,
            dimension_min_y: lowest_section * 16,
            dimension_height: sections as i32 * 16,
            columns: RefCell::default(),
            heights: RefCell::default(),
        }
    }

    fn height_of(&self, x: i32, z: i32, ocean_floor: bool) -> i32 {
        if let Some(height) = self.heights.borrow().get(&(x, z, ocean_floor)) {
            return *height;
        }
        let heightmap = if ocean_floor {
            Heightmap::OceanFloorWg
        } else {
            Heightmap::WorldSurfaceWg
        };
        let height = self.terrain.base_height(x, z, heightmap);
        self.heights
            .borrow_mut()
            .insert((x, z, ocean_floor), height);
        height
    }

    fn block_of(&self, x: i32, y: i32, z: i32) -> BlockState {
        let settings = self.terrain.settings();
        if y < settings.min_y || y >= settings.min_y + settings.height {
            return clustine_data::blocks::AIR;
        }
        let mut columns = self.columns.borrow_mut();
        let column = columns
            .entry((x, z))
            .or_insert_with(|| self.terrain.base_column(x, z));
        column[(y - settings.min_y) as usize]
    }
}

impl StructureGenerationContext for TerrainContext<'_> {
    fn seed(&self) -> i64 {
        self.seed
    }

    fn chunk_x(&self) -> i32 {
        self.chunk_x
    }

    fn chunk_z(&self) -> i32 {
        self.chunk_z
    }

    fn sea_level(&self) -> i32 {
        self.terrain.settings().sea_level
    }

    fn min_y(&self) -> i32 {
        self.terrain.settings().min_y
    }

    fn height(&self) -> i32 {
        self.terrain.settings().height
    }

    fn level_min_y(&self) -> i32 {
        self.dimension_min_y
    }

    fn level_height(&self) -> i32 {
        self.dimension_height
    }

    fn base_height(&mut self, x: i32, z: i32, ocean_floor: bool) -> i32 {
        self.height_of(x, z, ocean_floor)
    }

    fn biome_at(&mut self, block_x: i32, block_y: i32, block_z: i32) -> BiomeRef {
        BiomeRef {
            key: self.biomes.biome(block_x >> 2, block_y >> 2, block_z >> 2),
        }
    }

    fn column_state(&mut self, x: i32, y: i32, z: i32) -> ColumnBlock {
        let state = self.block_of(x, y, z);
        if state.is_air() {
            ColumnBlock::Air
        } else if state.props().fluid().is_some() {
            ColumnBlock::Fluid
        } else {
            ColumnBlock::Solid
        }
    }

    fn column_block(&mut self, x: i32, y: i32, z: i32) -> BlockState {
        self.block_of(x, y, z)
    }

    fn terrain_surface_height(&self, x: i32, z: i32, ocean_floor: bool) -> i32 {
        self.height_of(x, z, ocean_floor)
    }

    fn terrain_is_opaque(&self, x: i32, y: i32, z: i32, ocean_floor: bool) -> bool {
        let state = self.block_of(x, y, z);
        if ocean_floor {
            state.props().in_motion_heightmap()
        } else {
            !state.is_air()
        }
    }

    fn cold_enough_to_snow(&self, biome: BiomeRef, x: i32, y: i32, z: i32) -> bool {
        self.terrain.cold_enough_to_snow(biome.key, x, y, z)
    }
}
