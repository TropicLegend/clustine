//! A toy world for the unit tests: one flat ground under air, the blocks a test sets
//! on it, the three by three chunks around the origin's chunk writable, and one biome
//! everywhere. Its heights are worked out from its blocks as they are now.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use clustine_data::{BlockState, blocks};
use clustine_noise::WorldgenRandom;
use clustine_terrain::biome::Dimension;
use clustine_world::generation::{
    FinalHeightmap, Neighbourhood, PieceKey, PieceState, Placing, StartRef, WorldgenHeightmap,
};
use clustine_world::{Biome, BlockEntity, BlockPos, ChunkPos, ScheduledTick};

use crate::context::{Context, Level};
use crate::settings::FeatureSettings;

const MIN_Y: i32 = -64;
const HEIGHT: i32 = 384;

/// The overworld's features, read once for every test.
pub fn settings() -> &'static FeatureSettings {
    static SETTINGS: OnceLock<FeatureSettings> = OnceLock::new();
    SETTINGS.get_or_init(|| {
        FeatureSettings::new(13579, Dimension::Overworld).expect("the data is read whole")
    })
}

pub struct World {
    ground: BlockState,
    ground_top: i32,
    blocks: BTreeMap<BlockPos, BlockState>,
    /// The highest y of a block set in each column, where the heights start looking.
    tops: BTreeMap<(i32, i32), i32>,
    pub biome: Biome,
    pub ticks: Vec<(BlockPos, ScheduledTick)>,
    pub marks: Vec<BlockPos>,
    /// Every block a write changed, in order.
    pub writes: Vec<(BlockPos, BlockState)>,
}

impl World {
    /// `ground` up to `top` included, air above.
    pub fn flat(ground: BlockState, top: i32) -> Self {
        Self {
            ground,
            ground_top: top,
            blocks: BTreeMap::new(),
            tops: BTreeMap::new(),
            biome: Biome(0),
            ticks: Vec::new(),
            marks: Vec::new(),
            writes: Vec::new(),
        }
    }

    pub fn set(&mut self, at: BlockPos, state: BlockState) {
        self.insert(at, state);
    }

    fn insert(&mut self, at: BlockPos, state: BlockState) {
        self.blocks.insert(at, state);
        let top = self.tops.entry((at.x, at.z)).or_insert(at.y);
        *top = (*top).max(at.y);
    }

    pub fn get(&self, at: BlockPos) -> BlockState {
        if !(MIN_Y..MIN_Y + HEIGHT).contains(&at.y) {
            return blocks::VOID_AIR;
        }
        if let Some(state) = self.blocks.get(&at) {
            return *state;
        }
        if at.y <= self.ground_top {
            self.ground
        } else {
            blocks::AIR
        }
    }

    /// Runs `f` with this world as a feature sees it.
    pub fn with_level<R>(&mut self, f: impl FnOnce(&mut Level<'_>) -> R) -> R {
        let zoom = settings().zoom();
        let mut level = Level::new(self, zoom, MIN_Y, HEIGHT);
        f(&mut level)
    }

    /// Runs `f` with a context whose random source is seeded with `seed`, and no
    /// placed feature on top.
    pub fn with_context<R>(&mut self, seed: u64, f: impl FnOnce(&mut Context<'_, '_>) -> R) -> R {
        let mut random = WorldgenRandom::from_seed(seed);
        self.with_random(&mut random, None, f)
    }

    /// Runs `f` with a context of the random source given and the placed feature
    /// `top`.
    pub fn with_random<R>(
        &mut self,
        random: &mut WorldgenRandom,
        top: Option<usize>,
        f: impl FnOnce(&mut Context<'_, '_>) -> R,
    ) -> R {
        let settings = settings();
        let mut level = Level::new(self, settings.zoom(), MIN_Y, HEIGHT);
        let mut context = Context {
            level: &mut level,
            random,
            settings,
            top,
        };
        f(&mut context)
    }

    fn column_height(&self, x: i32, z: i32, counts: impl Fn(BlockState) -> bool) -> i32 {
        let set = self.tops.get(&(x, z)).copied().unwrap_or(MIN_Y);
        let top = set.max(self.ground_top).min(MIN_Y + HEIGHT - 1);
        for y in (MIN_Y..=top).rev() {
            if counts(self.get(BlockPos::new(x, y, z))) {
                return y + 1;
            }
        }
        MIN_Y
    }
}

fn blocks_motion(state: BlockState) -> bool {
    let props = state.props();
    props.in_motion_heightmap() || props.fluid().is_some()
}

impl Neighbourhood for World {
    fn centre(&self) -> ChunkPos {
        ChunkPos { x: 0, z: 0 }
    }

    fn block(&mut self, at: BlockPos) -> BlockState {
        self.get(at)
    }

    fn set_block(&mut self, at: BlockPos, state: BlockState) -> bool {
        let chunk = at.chunk();
        if chunk.x.abs() > 1 || chunk.z.abs() > 1 || !(MIN_Y..MIN_Y + HEIGHT).contains(&at.y) {
            return false;
        }
        self.insert(at, state);
        self.writes.push((at, state));
        true
    }

    fn block_entity(&mut self, _: BlockPos) -> Option<&BlockEntity> {
        None
    }

    fn set_block_entity(&mut self, _: BlockPos, _: Option<BlockEntity>) -> bool {
        false
    }

    fn height(&mut self, map: FinalHeightmap, x: i32, z: i32) -> i32 {
        match map {
            FinalHeightmap::WorldSurface => {
                self.column_height(x, z, |state| !state.props().is_air())
            }
            FinalHeightmap::OceanFloor => {
                self.column_height(x, z, |state| state.props().in_motion_heightmap())
            }
            FinalHeightmap::MotionBlocking => self.column_height(x, z, blocks_motion),
            FinalHeightmap::MotionBlockingNoLeaves => self.column_height(x, z, |state| {
                state.props().in_motion_heightmap_no_leaves() || state.props().fluid().is_some()
            }),
        }
    }

    fn worldgen_height(&mut self, map: WorldgenHeightmap, x: i32, z: i32) -> i32 {
        match map {
            WorldgenHeightmap::WorldSurface => self.height(FinalHeightmap::WorldSurface, x, z),
            WorldgenHeightmap::OceanFloor => self.height(FinalHeightmap::OceanFloor, x, z),
        }
    }

    fn biome(&mut self, _: BlockPos) -> Biome {
        self.biome
    }

    fn schedule(&mut self, at: BlockPos, tick: ScheduledTick) {
        self.ticks.push((at, tick));
    }

    fn mark(&mut self, at: BlockPos) {
        self.marks.push(at);
    }

    fn starts(&self) -> &[StartRef] {
        &[]
    }

    fn piece(&mut self, _: PieceKey) -> PieceState {
        PieceState::default()
    }

    fn set_piece(&mut self, _: PieceKey, _: PieceState) {}

    fn attempt(&mut self, _: Placing, place: &mut dyn FnMut(&mut dyn Neighbourhood)) -> bool {
        place(self);
        true
    }
}
