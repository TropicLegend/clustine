// Adapted from SteelMC's `steel-core/src/worldgen/structure/piece_placer/scattered_feature.rs`
// and `piece_placer/mod.rs` (see NOTICE in this crate), which follow the game's
// `StructurePiece` and `ScatteredFeaturePiece`.

//! What the game's `StructurePiece` gives the pieces that code draws block by block
//! (the ocean monument's and the three scattered features'): positions in the piece's
//! own frame turned into the world's by its orientation, blocks placed only inside the
//! centre chunk, boxes, columns, chests and dispensers with their loot; and what
//! `ScatteredFeaturePiece` adds, the height a piece settles at when it is first placed.
//!
//! A block is placed as the game's `placeBlock` places it: mirrored and turned with the
//! piece, set with the flags 2 (so a block that asks for it is marked, as the region's
//! `setBlock` does), its fluid scheduled, and marked for post-processing where it is
//! one of `SHAPE_CHECK_BLOCKS`.

use clustine_data::{BlockState, blocks};
use clustine_noise::{LegacyRandom, PositionalRandom, Random, RandomSource};
use clustine_protocol::nbt::Nbt;
use clustine_structures::geometry::{BoundingBox, Direction, IVec3, Rotation};
use clustine_structures::piece::StructureMirror;
use clustine_structures::state_transform::transform_state;
use clustine_world::generation::PieceState;
use clustine_world::{BlockEntity, BlockPos};

use super::Placing;
use crate::blocks::same_block;
use crate::context::Heightmap;
use crate::features::ground::monster_room::{keeps_entity_of, reorient};
use crate::features::ground::shared::{block_entity, compound};

/// `StructurePiece.SHAPE_CHECK_BLOCKS`: what the game marks for post-processing when a
/// piece places it, so that it takes its shape from its neighbours when the chunk is
/// finished.
const SHAPE_CHECK_BLOCKS: [BlockState; 12] = [
    blocks::NETHER_BRICK_FENCE,
    blocks::TORCH,
    blocks::WALL_TORCH,
    blocks::OAK_FENCE,
    blocks::SPRUCE_FENCE,
    blocks::DARK_OAK_FENCE,
    blocks::PALE_OAK_FENCE,
    blocks::ACACIA_FENCE,
    blocks::BIRCH_FENCE,
    blocks::JUNGLE_FENCE,
    blocks::LADDER,
    blocks::IRON_BARS,
];

/// A piece as its blocks are placed: its box as it stands now, and its orientation.
#[derive(Debug, Clone, Copy)]
pub(super) struct Piece {
    pub(super) bounding_box: BoundingBox,
    orientation: Option<Direction>,
    mirror: StructureMirror,
    rotation: Rotation,
}

impl Piece {
    /// The piece of `bounding_box` facing `orientation`, mirrored and turned as the
    /// game's `StructurePiece.setOrientation` makes it.
    pub(super) fn new(bounding_box: BoundingBox, orientation: Option<Direction>) -> Self {
        let (mirror, rotation) = match orientation {
            Some(Direction::South) => (StructureMirror::LeftRight, Rotation::None),
            Some(Direction::West) => (StructureMirror::LeftRight, Rotation::Clockwise90),
            Some(Direction::East) => (StructureMirror::None, Rotation::Clockwise90),
            _ => (StructureMirror::None, Rotation::None),
        };
        Self {
            bounding_box,
            orientation,
            mirror,
            rotation,
        }
    }

    /// `getWorldPos`: a position in the piece's frame, in the world.
    pub(super) fn world_pos(&self, x: i32, y: i32, z: i32) -> BlockPos {
        let bounds = self.bounding_box;
        let Some(orientation) = self.orientation else {
            return BlockPos::new(x, y, z);
        };
        let (world_x, world_z) = match orientation {
            Direction::North => (bounds.min_x() + x, bounds.max_z() - z),
            Direction::South => (bounds.min_x() + x, bounds.min_z() + z),
            Direction::West => (bounds.max_x() - z, bounds.min_z() + x),
            Direction::East => (bounds.min_x() + z, bounds.min_z() + x),
            Direction::Up | Direction::Down => (x, z),
        };
        BlockPos::new(world_x, y + bounds.min_y(), world_z)
    }

    /// `placeBlock`.
    pub(super) fn place_block(
        &self,
        placing: &mut Placing<'_, '_, '_>,
        state: BlockState,
        x: i32,
        y: i32,
        z: i32,
    ) {
        let pos = self.world_pos(x, y, z);
        if !inside(placing.chunk_box, pos) {
            return;
        }
        let state = transform_state(state, self.mirror, self.rotation);
        let level = &mut *placing.level;
        level.set_block(pos, state);
        if let Some(fluid) = level.block(pos).props().fluid() {
            level.schedule_fluid_tick(pos, fluid.fluid, 0);
        }
        if SHAPE_CHECK_BLOCKS
            .iter()
            .any(|checked| same_block(state, *checked))
        {
            level.mark_for_post_processing(pos);
        }
    }

    /// `getBlock`: what stands at a position of the piece, and air outside the centre
    /// chunk.
    pub(super) fn get_block(
        &self,
        placing: &mut Placing<'_, '_, '_>,
        x: i32,
        y: i32,
        z: i32,
    ) -> BlockState {
        let pos = self.world_pos(x, y, z);
        if inside(placing.chunk_box, pos) {
            placing.level.block(pos)
        } else {
            blocks::AIR
        }
    }

    /// `generateBox`: `edge` on the box's skin and `fill` within, from `from` to `to`
    /// both included; where `skip_air`, what is air (or outside the centre chunk) is
    /// left.
    pub(super) fn generate_box(
        &self,
        placing: &mut Placing<'_, '_, '_>,
        from: [i32; 3],
        to: [i32; 3],
        edge: BlockState,
        fill: BlockState,
        skip_air: bool,
    ) {
        let ([x0, y0, z0], [x1, y1, z1]) = (from, to);
        for y in y0..=y1 {
            for x in x0..=x1 {
                for z in z0..=z1 {
                    if skip_air && self.get_block(placing, x, y, z).props().is_air() {
                        continue;
                    }
                    let inner = y != y0 && y != y1 && x != x0 && x != x1 && z != z0 && z != z1;
                    let state = if inner { fill } else { edge };
                    self.place_block(placing, state, x, y, z);
                }
            }
        }
    }

    /// `generateBox` of one block throughout, not skipping air: what most of the
    /// game's calls are.
    pub(super) fn fill(
        &self,
        placing: &mut Placing<'_, '_, '_>,
        from: [i32; 3],
        to: [i32; 3],
        state: BlockState,
    ) {
        self.generate_box(placing, from, to, state, state, false);
    }

    /// `generateBox` with a `BlockSelector`: `select` chooses each block, given the
    /// run's random source and whether the block is on the skin. It is asked for every
    /// position, inside the centre chunk or not, as the game asks it.
    pub(super) fn generate_box_selected(
        &self,
        placing: &mut Placing<'_, '_, '_>,
        from: [i32; 3],
        to: [i32; 3],
        skip_air: bool,
        mut select: impl FnMut(&mut clustine_noise::WorldgenRandom, bool) -> BlockState,
    ) {
        let ([x0, y0, z0], [x1, y1, z1]) = (from, to);
        for y in y0..=y1 {
            for x in x0..=x1 {
                for z in z0..=z1 {
                    if skip_air && self.get_block(placing, x, y, z).props().is_air() {
                        continue;
                    }
                    let edge = y == y0 || y == y1 || x == x0 || x == x1 || z == z0 || z == z1;
                    let state = select(placing.random, edge);
                    self.place_block(placing, state, x, y, z);
                }
            }
        }
    }

    /// `generateAirBox`.
    pub(super) fn generate_air_box(
        &self,
        placing: &mut Placing<'_, '_, '_>,
        from: [i32; 3],
        to: [i32; 3],
    ) {
        let ([x0, y0, z0], [x1, y1, z1]) = (from, to);
        for y in y0..=y1 {
            for x in x0..=x1 {
                for z in z0..=z1 {
                    self.place_block(placing, blocks::AIR, x, y, z);
                }
            }
        }
    }

    /// `fillColumnDown`: `state` from the position down, through what structures may
    /// replace, to two blocks above the bottom of the world. The game sets these
    /// blocks straight, neither turned nor scheduled.
    pub(super) fn fill_column_down(
        &self,
        placing: &mut Placing<'_, '_, '_>,
        state: BlockState,
        x: i32,
        y: i32,
        z: i32,
    ) {
        let mut pos = self.world_pos(x, y, z);
        if !inside(placing.chunk_box, pos) {
            return;
        }
        let level = &mut *placing.level;
        while replaceable_by_structures(level.block(pos)) && pos.y > level.min_y() + 1 {
            level.set_block(pos, state);
            pos = pos.offset(0, -1, 0);
        }
    }

    /// `chunkIntersects`: whether the columns of a rectangle of the piece reach into
    /// the centre chunk.
    pub(super) fn chunk_intersects(
        &self,
        chunk_box: BoundingBox,
        x0: i32,
        z0: i32,
        x1: i32,
        z1: i32,
    ) -> bool {
        let a = self.world_pos(x0, 0, z0);
        let b = self.world_pos(x1, 0, z1);
        chunk_box.intersects_xz(a.x.min(b.x), a.z.min(b.z), a.x.max(b.x), a.z.max(b.z))
    }

    /// `createChest`: a chest at a position of the piece in the centre chunk where no
    /// chest stands yet, turned from the walls beside it (not with the piece), with
    /// its loot table and a seed drawn from the run's random source. Returns whether
    /// it was placed.
    pub(super) fn create_chest(
        &self,
        placing: &mut Placing<'_, '_, '_>,
        x: i32,
        y: i32,
        z: i32,
        table: &str,
    ) -> bool {
        let pos = self.world_pos(x, y, z);
        if !inside(placing.chunk_box, pos) || same_block(placing.level.block(pos), blocks::CHEST) {
            return false;
        }
        let state = reorient(placing.level, pos, blocks::CHEST);
        placing.level.set_block(pos, state);
        set_loot(placing, pos, blocks::CHEST, "minecraft:chest", table);
        true
    }

    /// `createDispenser`: as a chest, but placed as any block of the piece is, facing
    /// `facing` in the piece's frame.
    pub(super) fn create_dispenser(
        &self,
        placing: &mut Placing<'_, '_, '_>,
        x: i32,
        y: i32,
        z: i32,
        facing: Direction,
        table: &str,
    ) -> bool {
        let pos = self.world_pos(x, y, z);
        if !inside(placing.chunk_box, pos)
            || same_block(placing.level.block(pos), blocks::DISPENSER)
        {
            return false;
        }
        self.place_block(
            placing,
            with(blocks::DISPENSER, "facing", facing.as_str()),
            x,
            y,
            z,
        );
        set_loot(
            placing,
            pos,
            blocks::DISPENSER,
            "minecraft:dispenser",
            table,
        );
        true
    }
}

/// `RandomizableContainer.setBlockEntityLootTable`: the seed is drawn only where the
/// block keeps a container to take it.
fn set_loot(
    placing: &mut Placing<'_, '_, '_>,
    pos: BlockPos,
    block: BlockState,
    kind: &str,
    table: &str,
) {
    if keeps_entity_of(placing.level, pos, block) {
        let seed = placing.random.next_i64();
        placing
            .level
            .set_block_entity(pos, Some(loot_data(kind, table, seed)));
    }
}

/// What the game saves for a block entity whose loot is still to be rolled (a
/// container's `trySaveLootTable`, which the brushable blocks share): the table, the
/// seed unless it is nought, and no components.
pub(super) fn loot_data(kind: &str, table: &str, seed: i64) -> BlockEntity {
    let mut members = vec![("LootTable", Nbt::String(table.to_string()))];
    if seed != 0 {
        members.push(("LootTableSeed", Nbt::Long(seed)));
    }
    members.push(("components", compound(Vec::new())));
    block_entity(kind, members)
}

/// Whether the centre chunk's box holds a position (`BoundingBox.isInside`).
pub(super) fn inside(chunk_box: BoundingBox, pos: BlockPos) -> bool {
    chunk_box.contains(IVec3::new(pos.x, pos.y, pos.z))
}

/// `isReplaceableByStructures`: air, a liquid's own block, glow lichen and seagrass.
fn replaceable_by_structures(state: BlockState) -> bool {
    state.props().is_air()
        || state.props().is_liquid()
        || same_block(state, blocks::GLOW_LICHEN)
        || same_block(state, blocks::SEAGRASS)
        || same_block(state, blocks::TALL_SEAGRASS)
}

/// A state with one property set, which the pieces only ask of blocks that have it.
pub(super) fn with(state: BlockState, name: &str, value: &str) -> BlockState {
    state
        .with_property(name, value)
        .unwrap_or_else(|| panic!("the block has {name} {value}"))
}

/// The box of a piece as it stands after the height it settled at.
pub(super) fn settled_box(made: BoundingBox, state: PieceState) -> BoundingBox {
    made.translate(IVec3::new(0, state.shift_y, 0))
}

/// `ScatteredFeaturePiece.updateAverageGroundHeight`: on the first placing whose
/// centre chunk the piece reaches, the piece moves to the mean height of the ground
/// (`MOTION_BLOCKING_NO_LEAVES`) over the columns it has in that chunk, plus
/// `offset`. None where it has none there, and the piece is then not placed.
///
/// The game takes the piece as settled once its height is not negative, so a piece
/// whose ground lies below y 0 moves again in the next chunk; the state says so.
pub(super) fn settle_on_average(
    placing: &mut Placing<'_, '_, '_>,
    made: BoundingBox,
    state: PieceState,
    offset: i32,
) -> Option<PieceState> {
    if state.settled {
        return Some(state);
    }
    let bounds = settled_box(made, state);
    let (mut total, mut count) = (0, 0);
    for z in bounds.min_z()..=bounds.max_z() {
        for x in bounds.min_x()..=bounds.max_x() {
            if placing.chunk_box.contains_xyz(x, 64, z) {
                total += placing
                    .level
                    .height_at(Heightmap::MotionBlockingNoLeaves, x, z);
                count += 1;
            }
        }
    }
    if count == 0 {
        return None;
    }
    Some(settle(state, bounds, total / count, offset))
}

/// `ScatteredFeaturePiece.updateHeightPositionToLowestGroundHeight`: as
/// [`settle_on_average`], to the lowest ground over all the piece's columns,
/// whichever chunk they are in.
pub(super) fn settle_on_lowest(
    placing: &mut Placing<'_, '_, '_>,
    made: BoundingBox,
    state: PieceState,
    offset: i32,
) -> Option<PieceState> {
    if state.settled {
        return Some(state);
    }
    let bounds = settled_box(made, state);
    let mut lowest = placing.level.top_y();
    let mut found = false;
    for z in bounds.min_z()..=bounds.max_z() {
        for x in bounds.min_x()..=bounds.max_x() {
            let height = placing
                .level
                .height_at(Heightmap::MotionBlockingNoLeaves, x, z);
            lowest = lowest.min(height);
            found = true;
        }
    }
    found.then(|| settle(state, bounds, lowest, offset))
}

/// The state of a piece moved from `bounds` to stand at `height` plus `offset`; the
/// game's `heightPosition` is the height, and counts as set once it is not negative.
fn settle(state: PieceState, bounds: BoundingBox, height: i32, offset: i32) -> PieceState {
    PieceState {
        shift_y: state.shift_y + height - bounds.min_y() + offset,
        settled: height >= 0,
        flags: state.flags,
    }
}

/// The region's random source (`WorldGenRegion.getRandom`), which mobs draw from as
/// they are made.
pub(super) fn region_random<'a>(placing: &'a mut Placing<'_, '_, '_>) -> &'a mut RandomSource {
    let settings = placing.settings;
    placing
        .level
        .region_random(settings.seed(), settings.legacy_random_source())
}

/// What `Mob.finalizeSpawn` draws from the region's random source for a mob a piece
/// makes: the follow range's random bonus (`triangle`, two doubles) and whether it is
/// left-handed: drawn for a mob of a type the maker does not make yet, as the draws
/// are the region's, which other pieces and features draw from after.
pub(super) fn finalize_mob_spawn(placing: &mut Placing<'_, '_, '_>) {
    let random = region_random(placing);
    random.triangle(0.0, 0.114_850_000_000_000_01);
    random.next_f32();
}

/// The world's seed's own random source at a position, as the game makes one with
/// `RandomSource.create(level.getSeed()).forkPositional().at(pos)`.
pub(super) fn seed_random_at(placing: &Placing<'_, '_, '_>, pos: BlockPos) -> RandomSource {
    LegacyRandom::from_seed(placing.settings.seed() as u64)
        .next_positional()
        .at(pos.x, pos.y, pos.z)
}

/// `BlockPos.asLong`.
pub(super) fn pack(pos: BlockPos) -> i64 {
    let x = i64::from(pos.x) & 0x3FF_FFFF;
    let y = i64::from(pos.y) & 0xFFF;
    let z = i64::from(pos.z) & 0x3FF_FFFF;
    x << 38 | z << 12 | y
}

#[cfg(test)]
pub(super) mod testing {
    //! A piece placed into the crate's toy world, with block entities kept.

    use clustine_data::BlockState;
    use clustine_noise::{RandomSource, WorldgenRandom};
    use clustine_structures::geometry::{BoundingBox, Direction, IVec3, Identifier};
    use clustine_structures::piece::{StructurePiece, StructurePiecePayload};
    use clustine_world::generation::{KeyOnly, Maker, Makings};
    use clustine_world::{BlockPos, ChunkPos};

    use crate::context::Level;
    use crate::features::ground::shared::testing::WithEntities;
    use crate::structures::Placing;
    use crate::testing::{World, settings};

    /// The writable box of the chunk `(x, z)`.
    pub fn chunk(x: i32, z: i32) -> BoundingBox {
        BoundingBox::new(
            IVec3::new(x * 16, -63, z * 16),
            IVec3::new(x * 16 + 15, 319, z * 16 + 15),
        )
    }

    /// The whole of the toy world's writable three by three, as one box.
    pub fn area() -> BoundingBox {
        BoundingBox::new(IVec3::new(-16, -63, -16), IVec3::new(31, 319, 31))
    }

    /// A piece of `payload` in `bounding_box`, facing `orientation`.
    pub fn piece(
        bounding_box: BoundingBox,
        orientation: Direction,
        payload: StructurePiecePayload,
    ) -> StructurePiece {
        let mut piece = StructurePiece::non_jigsaw(
            Identifier::new_static("minecraft", "test"),
            bounding_box,
            0,
            Some(orientation),
        );
        piece.payload = payload;
        piece
    }

    /// The toy world and the run's random source.
    pub struct Run {
        pub world: WithEntities,
        pub random: WorldgenRandom,
        /// The makings of the run's entities, carried from placement to placement.
        pub makings: Makings,
    }

    impl Run {
        pub fn new(world: World, seed: u64) -> Self {
            Self {
                world: WithEntities::new(world),
                random: WorldgenRandom::from_seed(seed),
                makings: Makings::new(0, ChunkPos::new(0, 0)),
            }
        }

        /// Runs `f` with this world as a placing whose centre chunk is `chunk_box`.
        pub fn place<R>(
            &mut self,
            chunk_box: BoundingBox,
            f: impl FnOnce(&mut Placing<'_, '_, '_>) -> R,
        ) -> R {
            self.place_making(&KeyOnly, chunk_box, f)
        }

        /// [`Self::place`], the structures' entities made by `maker`, each with the
        /// next making of the run of chunk (0, 0).
        pub fn place_making<R>(
            &mut self,
            maker: &dyn Maker,
            chunk_box: BoundingBox,
            f: impl FnOnce(&mut Placing<'_, '_, '_>) -> R,
        ) -> R {
            let settings = settings();
            let mut level = Level::new(&mut self.world, settings.zoom(), -64, 384)
                .with_maker(maker, self.makings);
            let mut placing = Placing {
                level: &mut level,
                random: &mut self.random,
                settings,
                chunk_box,
                centre: ChunkPos::new(0, 0),
                pivot: IVec3::ZERO,
            };
            let result = f(&mut placing);
            self.makings = level.makings();
            result
        }

        pub fn get(&self, at: BlockPos) -> BlockState {
            self.world.world.get(at)
        }
    }

    /// The region's random source of a fresh run, as the toy world's centre has it.
    pub fn fresh_region_random() -> RandomSource {
        let settings = settings();
        let mut world = World::flat(clustine_data::blocks::STONE, 0);
        let mut level = Level::new(&mut world, settings.zoom(), -64, 384);
        level
            .region_random(settings.seed(), settings.legacy_random_source())
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{Run, area, chunk, fresh_region_random};
    use super::*;
    use crate::testing::World;

    fn frame() -> BoundingBox {
        BoundingBox::new(IVec3::new(10, 64, 30), IVec3::new(20, 70, 44))
    }

    #[test]
    fn a_piece_s_frame_turns_with_its_orientation_as_the_game_s_get_world_pos() {
        let bounds = frame();
        let at = |orientation| Piece::new(bounds, Some(orientation)).world_pos(1, 2, 3);
        assert_eq!(at(Direction::North), BlockPos::new(11, 66, 41));
        assert_eq!(at(Direction::South), BlockPos::new(11, 66, 33));
        assert_eq!(at(Direction::West), BlockPos::new(17, 66, 31));
        assert_eq!(at(Direction::East), BlockPos::new(13, 66, 31));
        assert_eq!(
            Piece::new(bounds, None).world_pos(1, 2, 3),
            BlockPos::new(1, 2, 3)
        );
    }

    #[test]
    fn a_piece_facing_west_mirrors_and_turns_its_blocks() {
        let stairs = with(blocks::OAK_STAIRS, "facing", "north");
        let mut run = Run::new(World::flat(blocks::STONE, 0), 1);
        let bounds = BoundingBox::new(IVec3::new(0, 10, 0), IVec3::new(4, 12, 4));
        run.place(area(), |p| {
            Piece::new(bounds, Some(Direction::West)).place_block(p, stairs, 0, 0, 0);
        });
        // Mirrored left to right, north becomes south; turned clockwise, west.
        let placed = run.get(BlockPos::new(4, 10, 0));
        assert_eq!(placed.property("facing"), Some("west"));
    }

    #[test]
    fn blocks_are_placed_only_in_the_centre_chunk() {
        let mut run = Run::new(World::flat(blocks::STONE, 0), 1);
        let bounds = BoundingBox::new(IVec3::new(10, 10, 4), IVec3::new(20, 12, 8));
        run.place(chunk(0, 0), |p| {
            let at = Piece::new(bounds, Some(Direction::South));
            at.fill(p, [0, 0, 0], [10, 2, 4], blocks::GOLD_BLOCK);
        });
        assert!(!run.world.world.writes.is_empty());
        assert!(run.world.world.writes.iter().all(|(at, _)| at.x <= 15));
        assert_eq!(run.get(BlockPos::new(15, 11, 6)), blocks::GOLD_BLOCK);
        assert_eq!(run.get(BlockPos::new(16, 11, 6)), blocks::AIR);
    }

    #[test]
    fn water_a_piece_places_is_scheduled_and_a_fence_is_marked() {
        let mut run = Run::new(World::flat(blocks::STONE, 0), 1);
        let bounds = BoundingBox::new(IVec3::new(0, 10, 0), IVec3::new(4, 12, 4));
        run.place(area(), |p| {
            let at = Piece::new(bounds, Some(Direction::North));
            at.place_block(p, blocks::WATER, 1, 0, 1);
            at.place_block(p, blocks::OAK_FENCE, 2, 0, 1);
            at.place_block(p, blocks::GOLD_BLOCK, 3, 0, 1);
        });
        let water = BlockPos::new(1, 10, 3);
        assert_eq!(run.world.world.ticks.len(), 1);
        assert_eq!(run.world.world.ticks[0].0, water);
        assert_eq!(run.world.world.marks, [BlockPos::new(2, 10, 3)]);
    }

    #[test]
    fn a_column_is_filled_down_through_water_to_the_ground() {
        let mut world = World::flat(blocks::STONE, 50);
        for y in 51..=55 {
            world.set(BlockPos::new(3, y, 3), blocks::WATER);
        }
        let mut run = Run::new(world, 1);
        let bounds = BoundingBox::new(IVec3::new(3, 60, 3), IVec3::new(5, 62, 5));
        run.place(area(), |p| {
            let at = Piece::new(bounds, Some(Direction::South));
            at.fill_column_down(p, blocks::OAK_LOG, 0, -1, 0);
        });
        for y in 51..=59 {
            assert_eq!(run.get(BlockPos::new(3, y, 3)), blocks::OAK_LOG, "at {y}");
        }
        assert_eq!(run.get(BlockPos::new(3, 50, 3)), blocks::STONE);
    }

    /// A box at y 64 across chunks 0 and 1 along x.
    fn across() -> BoundingBox {
        BoundingBox::new(IVec3::new(10, 64, 2), IVec3::new(21, 70, 5))
    }

    #[test]
    fn a_piece_settles_on_the_mean_ground_of_its_columns_in_the_first_chunk_only() {
        let mut world = World::flat(blocks::STONE, 70);
        for x in 16..=21 {
            for z in 2..=5 {
                for y in 71..=90 {
                    world.set(BlockPos::new(x, y, z), blocks::STONE);
                }
            }
        }
        let mut run = Run::new(world, 1);
        let first = run.place(chunk(0, 0), |p| {
            settle_on_average(p, across(), PieceState::default(), 0)
        });
        let first = first.expect("the piece has columns in the chunk");
        assert_eq!(
            first,
            PieceState {
                shift_y: 71 - 64,
                settled: true,
                flags: 0,
            }
        );
        // The next chunk's run keeps the box where the first put it.
        let second = run.place(chunk(1, 0), |p| settle_on_average(p, across(), first, 0));
        assert_eq!(second, Some(first));
        assert_eq!(settled_box(across(), first).min_y(), 71);
    }

    #[test]
    fn a_piece_settles_on_the_lowest_ground_under_all_its_columns_with_its_offset() {
        let mut world = World::flat(blocks::STONE, 70);
        world.set(BlockPos::new(21, 70, 5), blocks::AIR);
        let mut run = Run::new(world, 1);
        let state = run.place(chunk(0, 0), |p| {
            settle_on_lowest(p, across(), PieceState::default(), -2)
        });
        let state = state.expect("the piece has columns");
        assert_eq!(state.shift_y, 70 - 2 - 64);
        assert!(state.settled);
    }

    #[test]
    fn a_piece_on_ground_below_nought_settles_again_in_the_next_chunk() {
        let mut run = Run::new(World::flat(blocks::STONE, -10), 1);
        let state = run
            .place(chunk(0, 0), |p| {
                settle_on_average(p, across(), PieceState::default(), 0)
            })
            .expect("the piece has columns");
        assert!(!state.settled);
        assert_eq!(settled_box(across(), state).min_y(), -9);
        // Moved again from where it stands, to the same height.
        let again = run
            .place(chunk(1, 0), |p| settle_on_average(p, across(), state, 0))
            .expect("the piece has columns");
        assert_eq!(again, state);
    }

    #[test]
    fn a_piece_with_no_column_in_the_chunk_does_not_settle() {
        let mut run = Run::new(World::flat(blocks::STONE, 70), 1);
        let state = run.place(chunk(-1, 0), |p| {
            settle_on_average(p, across(), PieceState::default(), 0)
        });
        assert_eq!(state, None);
    }

    #[test]
    fn a_chest_draws_its_seed_from_the_run_and_is_not_placed_twice() {
        let mut run = Run::new(World::flat(blocks::STONE, 0), 7);
        let mut expected = run.random.clone();
        let bounds = BoundingBox::new(IVec3::new(0, 10, 0), IVec3::new(4, 12, 4));
        let at = Piece::new(bounds, Some(Direction::North));
        let (first, second) = run.place(area(), |p| {
            (
                at.create_chest(p, 1, 0, 1, "minecraft:chests/test"),
                at.create_chest(p, 1, 0, 1, "minecraft:chests/test"),
            )
        });
        assert!(first && !second);
        let pos = at.world_pos(1, 0, 1);
        let entity = run.world.entities.get(&pos).expect("the chest keeps one");
        assert_eq!(
            *entity,
            loot_data(
                "minecraft:chest",
                "minecraft:chests/test",
                expected.next_i64()
            )
        );
        assert_eq!(run.random.next_i64(), expected.next_i64());
    }

    #[test]
    fn a_mob_s_finalize_spawn_draws_three_numbers_from_the_region() {
        let mut run = Run::new(World::flat(blocks::STONE, 0), 1);
        let after = run.place(area(), |p| {
            finalize_mob_spawn(p);
            region_random(p).next_i64()
        });
        let mut expected = fresh_region_random();
        expected.next_f64();
        expected.next_f64();
        expected.next_f32();
        assert_eq!(after, expected.next_i64());
    }

    #[test]
    fn a_position_packs_as_the_game_s_block_pos_as_long() {
        assert_eq!(pack(BlockPos::new(0, 0, 0)), 0);
        assert_eq!(pack(BlockPos::new(1, 2, 3)), (1 << 38) | (3 << 12) | 2);
        assert_eq!(pack(BlockPos::new(-1, -1, -1)), -1);
    }
}
