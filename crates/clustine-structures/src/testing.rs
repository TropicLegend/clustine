//! A toy world for the unit tests, after `clustine-features/src/testing.rs`: one flat
//! ground under air, the blocks a test sets on it, the three by three chunks around
//! the origin's chunk writable, block entities kept as a chunk keeps them (one where
//! the block keeps one, its data kept while the type stays), and heights worked out
//! from the blocks as they are now.

use std::collections::BTreeMap;

use clustine_data::{BlockState, blocks};
use clustine_world::generation::{
    FinalHeightmap, Neighbourhood, PieceKey, PieceState, Placing, StartRef, WorldgenHeightmap,
};
use clustine_world::{Biome, BlockEntity, BlockPos, ChunkPos, ScheduledTick};

pub const MIN_Y: i32 = -64;
pub const HEIGHT: i32 = 384;

pub struct World {
    ground: BlockState,
    ground_top: i32,
    blocks: BTreeMap<BlockPos, BlockState>,
    /// The highest y of a block set in each column, where the heights start looking.
    tops: BTreeMap<(i32, i32), i32>,
    pub entities: BTreeMap<BlockPos, BlockEntity>,
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
            entities: BTreeMap::new(),
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
        match state.block_info().and_then(|info| info.block_entity_type) {
            None => {
                self.entities.remove(&at);
            }
            Some(kind) => {
                if self.entities.get(&at).map(|entity| entity.kind) != Some(kind) {
                    self.entities.insert(
                        at,
                        BlockEntity {
                            kind,
                            data: Vec::new(),
                        },
                    );
                }
            }
        }
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

    fn block_entity(&mut self, at: BlockPos) -> Option<&BlockEntity> {
        self.entities.get(&at)
    }

    fn set_block_entity(&mut self, at: BlockPos, entity: Option<BlockEntity>) -> bool {
        let Some(there) = self.entities.get_mut(&at) else {
            return entity.is_none();
        };
        match entity {
            Some(entity) if entity.kind != there.kind => false,
            Some(entity) => {
                there.data = entity.data;
                true
            }
            None => {
                there.data = Vec::new();
                true
            }
        }
    }

    fn height(&mut self, map: FinalHeightmap, x: i32, z: i32) -> i32 {
        match map {
            FinalHeightmap::WorldSurface => {
                self.column_height(x, z, |state| !state.props().is_air())
            }
            FinalHeightmap::OceanFloor => {
                self.column_height(x, z, |state| state.props().in_motion_heightmap())
            }
            FinalHeightmap::MotionBlocking => self.column_height(x, z, |state| {
                state.props().in_motion_heightmap() || state.props().fluid().is_some()
            }),
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
        Biome(0)
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
