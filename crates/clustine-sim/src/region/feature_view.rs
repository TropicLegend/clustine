//! A region's view for a feature it places in its tick: a sapling's tree, bone meal's
//! grass, a huge mushroom (`docs/groundwork/n8-plan.md`, section 7.2; ADR-0029, section 9).
//!
//! The features crate places through `clustine_world::generation::Neighbourhood`, as it
//! does in generation. Here the reads come from the region's loaded chunks and copies, and
//! from the view's own writes, so that a feature reads what it wrote as the game's does;
//! a write into the region's land goes straight through the region's set with the flags
//! the feature uses, its updates made in the order the feature makes them; a write into a
//! copy is kept and read back, and becomes one `SetIf` effect for its cell when the feature
//! is done, expecting what the view first read there; a place the region cannot read
//! refuses the write, as the region's set drops one there.

use std::collections::BTreeMap;

use clustine_data::{BlockState, blocks};
use clustine_world::generation::{
    FinalHeightmap, Neighbourhood, PieceKey, PieceState, Placing, StartRef, WorldgenHeightmap,
};
use clustine_world::{
    Biome, BlockEntity, BlockPos, ChunkPos, Effect, Effected, InChunk, ScheduledTick,
};

use super::Region;
use super::updates::UPDATE_LIMIT;
use crate::api::TickOutput;

/// A region's view for one feature placed about `centre`.
pub(crate) struct FeatureView<'r> {
    region: &'r mut Region,
    output: &'r mut TickOutput,
    centre: ChunkPos,
    /// The flags each write into the region's land is set with: 19 for a tree's logs,
    /// leaves and decorations (`TreeFeature`), 3 for what `Feature.setBlock` sets.
    flags: u32,
    /// The writes into copies, by cell: what the view first read there, and what the
    /// feature last wrote.
    across: BTreeMap<BlockPos, (BlockState, BlockState)>,
}

impl<'r> FeatureView<'r> {
    pub(crate) fn new(
        region: &'r mut Region,
        output: &'r mut TickOutput,
        centre: ChunkPos,
        flags: u32,
    ) -> Self {
        Self {
            region,
            output,
            centre,
            flags,
            across: BTreeMap::new(),
        }
    }

    /// The feature is done: each cell it wrote in a copy becomes one effect, in the order
    /// of the cells, for the holder to set where it still holds what the view read there.
    pub(crate) fn finish(self) {
        for (at, (expected, state)) in self.across {
            if expected != state && self.region.sends_to(at.chunk()) {
                self.region.made.push(Effect {
                    at,
                    what: Effected::SetIf {
                        expected,
                        state,
                        flags: self.flags,
                    },
                });
            }
        }
    }

    /// The column's height in `chunk`'s map `map`, read where the region has the chunk.
    fn column_height(&self, map: FinalHeightmap, x: i32, z: i32) -> Option<i32> {
        let chunk = self
            .region
            .readable_chunk(ChunkPos::containing(f64::from(x), f64::from(z)))?;
        let column = ((z & 15) << 4 | (x & 15)) as usize;
        let heights = chunk.heights();
        let above = match map {
            FinalHeightmap::WorldSurface => heights.world_surface[column],
            FinalHeightmap::OceanFloor => heights.ocean_floor[column],
            FinalHeightmap::MotionBlocking => heights.motion_blocking[column],
            FinalHeightmap::MotionBlockingNoLeaves => heights.motion_blocking_no_leaves[column],
        };
        Some(chunk.min_y() + i32::from(above))
    }
}

impl Neighbourhood for FeatureView<'_> {
    fn centre(&self) -> ChunkPos {
        self.centre
    }

    fn block(&mut self, at: BlockPos) -> BlockState {
        if let Some((_, now)) = self.across.get(&at) {
            return *now;
        }
        // A place the region cannot read is air, as the game's region of generation
        // reads beyond its edge; a tree is never near one, its sapling's 3×3 being
        // present.
        self.region.block(at).unwrap_or(blocks::AIR)
    }

    fn set_block(&mut self, at: BlockPos, state: BlockState) -> bool {
        let Some(old) = self.region.block(at) else {
            return false;
        };
        if self.region.chunks.contains_key(&at.chunk()) {
            self.region
                .set_block_updated(at, state, self.flags, UPDATE_LIMIT, self.output);
            return true;
        }
        let entry = self.across.entry(at).or_insert((old, old));
        entry.1 = state;
        true
    }

    fn block_entity(&mut self, at: BlockPos) -> Option<&BlockEntity> {
        let chunk = self.region.chunks.get(&at.chunk())?;
        let (x, z) = at.in_chunk();
        chunk.block_entities().get(&InChunk::new(x, at.y, z))
    }

    /// A bee nest's bees go into the region's own land only: a `SetIf` carries a state
    /// and no block entity, so a nest placed across a line is empty (named, the plan's
    /// 7.4).
    fn set_block_entity(&mut self, at: BlockPos, entity: Option<BlockEntity>) -> bool {
        let Some(chunk) = self.region.chunks.get_mut(&at.chunk()) else {
            return false;
        };
        let (x, z) = at.in_chunk();
        match entity {
            Some(entity) => chunk
                .set_block_entity_data(InChunk::new(x, at.y, z), entity.data)
                .is_ok(),
            None => false,
        }
    }

    fn height(&mut self, map: FinalHeightmap, x: i32, z: i32) -> i32 {
        self.column_height(map, x, z)
            .unwrap_or(self.region.config.lowest_y)
    }

    /// Terrain's heights are not kept past generation; the final ones stand for them,
    /// which no tree, mushroom or bone meal feature reads.
    fn worldgen_height(&mut self, map: WorldgenHeightmap, x: i32, z: i32) -> i32 {
        let map = match map {
            WorldgenHeightmap::WorldSurface => FinalHeightmap::WorldSurface,
            WorldgenHeightmap::OceanFloor => FinalHeightmap::OceanFloor,
        };
        self.height(map, x, z)
    }

    fn biome(&mut self, at: BlockPos) -> Biome {
        let Some(chunk) = self.region.readable_chunk(at.chunk()) else {
            return Biome(0);
        };
        let index = usize::try_from((at.y - chunk.min_y()).div_euclid(16)).unwrap_or(0);
        let Some(section) = chunk.sections().get(index) else {
            return Biome(0);
        };
        let (x, z) = at.in_chunk();
        section.biome_at(x, at.y.rem_euclid(16) as usize, z)
    }

    fn schedule(&mut self, at: BlockPos, tick: ScheduledTick) {
        self.region
            .schedule_tick(at, tick.of, tick.delay, tick.priority);
    }

    /// A `ServerLevel` keeps no marks for post-processing (**Inferred**): passed over.
    fn mark(&mut self, _at: BlockPos) {}

    fn starts(&self) -> &[StartRef] {
        &[]
    }

    fn piece(&mut self, _piece: PieceKey) -> PieceState {
        PieceState::default()
    }

    fn set_piece(&mut self, _piece: PieceKey, _state: PieceState) {}

    /// No undo: a feature that panics panics the tick, as any rule does.
    fn attempt(&mut self, _what: Placing, place: &mut dyn FnMut(&mut dyn Neighbourhood)) -> bool {
        place(self);
        true
    }
}

impl Region {
    /// Places the configured feature `name` at `at` through the region's view, its
    /// writes set with `flags`, drawing from a random of the game's kind seeded with
    /// `seed` (the plan's 7.3: the stream's next long, made where the game would start
    /// drawing for it). Whether the feature placed; false where the world has no features
    /// (a flat world, a test that grows nothing) or none of that name.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "saplings and bone meal call it (N8f, part D4)")
    )]
    pub(crate) fn place_feature(
        &mut self,
        name: &str,
        at: BlockPos,
        flags: u32,
        seed: u64,
        output: &mut TickOutput,
    ) -> bool {
        let Some(settings) = self.config.features.0.clone() else {
            return false;
        };
        let Some(feature) = settings.configured_named(name) else {
            return false;
        };
        let mut random = clustine_noise::WorldgenRandom::from_seed(seed);
        let mut view = FeatureView::new(self, output, at.chunk(), flags);
        let placed = {
            let mut level = clustine_features::context::Level::new(
                &mut view,
                settings.zoom(),
                settings.min_y(),
                settings.height(),
            );
            feature.place(
                &mut clustine_features::context::Context {
                    level: &mut level,
                    random: &mut random,
                    settings: &settings,
                    top: None,
                },
                at,
            )
        };
        view.finish();
        placed
    }
}
