// Adapted from SteelMC's `steel-core/src/worldgen/structure/piece_placer/mod.rs` and
// `scattered_feature.rs`, which follow the game's `StructurePiece`.

//! What the game's pieces written in code share (`StructurePiece`): a piece's own
//! coordinates turned into the world's by its orientation, and the ways it places
//! blocks: one at a time, boxes of them (whole, chosen by a selector, or each by a
//! chance), the upper half of a sphere, a chest of loot and a spawner.
//!
//! Everything is clipped to the centre chunk's writable box. A block placed through
//! [`CodePiece::place_block`] is mirrored and turned as the piece's orientation says,
//! is marked for post-processing where the game's `SHAPE_CHECK_BLOCKS` hold it, and
//! schedules the tick of a fluid it holds; the game places it with the flags 2, which
//! lack `UPDATE_KNOWN_SHAPE`.

use clustine_data::{BlockState, Face, SupportType, blocks};
use clustine_noise::{Random, WorldgenRandom};
use clustine_protocol::nbt::Nbt;
use clustine_structures::geometry::{BoundingBox, Direction, Rotation};
use clustine_structures::piece::StructureMirror;
use clustine_structures::state_transform::transform_state;
use clustine_world::{BlockEntity, BlockPos};

use super::Placing;
use crate::blocks::same_block;
use crate::context::{Heightmap, Level};
use crate::features::ground::monster_room::{reorient, spawner_data};
use crate::features::ground::shared::{block_entity, compound};

/// The blocks whose shape the game settles once the chunk is finished
/// (`StructurePiece.SHAPE_CHECK_BLOCKS`).
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

/// One corner of a box in a piece's own coordinates.
pub(crate) type Corner = [i32; 3];

/// A piece written in code, as it places its blocks into one chunk.
pub(crate) struct CodePiece<'a, 'p, 'l, 'v> {
    pub placing: &'a mut Placing<'p, 'l, 'v>,
    pub bounds: BoundingBox,
    orientation: Option<Direction>,
    mirror: StructureMirror,
    rotation: Rotation,
    /// Blocks that [`Self::place_block`] leaves where they stand (`canBeReplaced`,
    /// which only mineshafts narrow).
    kept: &'static [BlockState],
}

impl<'a, 'p, 'l, 'v> CodePiece<'a, 'p, 'l, 'v> {
    /// A piece with the box and orientation its layout gave it. The mirror and the
    /// turn follow from the orientation as `StructurePiece.setOrientation` sets them.
    pub fn new(
        placing: &'a mut Placing<'p, 'l, 'v>,
        bounds: BoundingBox,
        orientation: Option<Direction>,
    ) -> Self {
        let (mirror, rotation) = match orientation {
            Some(Direction::South) => (StructureMirror::LeftRight, Rotation::None),
            Some(Direction::West) => (StructureMirror::LeftRight, Rotation::Clockwise90),
            Some(Direction::East) => (StructureMirror::None, Rotation::Clockwise90),
            _ => (StructureMirror::None, Rotation::None),
        };
        Self {
            placing,
            bounds,
            orientation,
            mirror,
            rotation,
            kept: &[],
        }
    }

    /// The same piece, leaving the blocks `kept` where it meets them.
    pub fn keeping(self, kept: &'static [BlockState]) -> Self {
        Self { kept, ..self }
    }

    pub fn chunk_box(&self) -> BoundingBox {
        self.placing.chunk_box
    }

    pub fn random(&mut self) -> &mut WorldgenRandom {
        self.placing.random
    }

    pub fn level(&mut self) -> &mut Level<'v> {
        self.placing.level
    }

    /// Whether the chunk's writable box holds `pos` (`chunkBB.isInside`).
    pub fn inside(&self, pos: BlockPos) -> bool {
        self.placing.chunk_box.contains_xyz(pos.x, pos.y, pos.z)
    }

    /// `getWorldPos`: a piece without an orientation is in the world's coordinates.
    pub fn world_pos(&self, x: i32, y: i32, z: i32) -> BlockPos {
        let b = self.bounds;
        let Some(orientation) = self.orientation else {
            return BlockPos::new(x, y, z);
        };
        let (wx, wz) = match orientation {
            Direction::North => (b.min_x() + x, b.max_z() - z),
            Direction::South => (b.min_x() + x, b.min_z() + z),
            Direction::West => (b.max_x() - z, b.min_z() + x),
            Direction::East => (b.min_x() + z, b.min_z() + x),
            Direction::Up | Direction::Down => (x, z),
        };
        BlockPos::new(wx, y + b.min_y(), wz)
    }

    /// `getBlock`: air outside the chunk's box.
    pub fn block(&mut self, x: i32, y: i32, z: i32) -> BlockState {
        let pos = self.world_pos(x, y, z);
        if self.inside(pos) {
            self.placing.level.block(pos)
        } else {
            blocks::AIR
        }
    }

    /// `placeBlock`.
    pub fn place_block(&mut self, state: BlockState, x: i32, y: i32, z: i32) {
        let pos = self.world_pos(x, y, z);
        if !self.inside(pos) {
            return;
        }
        if !self.kept.is_empty() {
            let there = self.block(x, y, z);
            if self.kept.iter().any(|kept| same_block(there, *kept)) {
                return;
            }
        }
        let state = transform_state(state, self.mirror, self.rotation);
        let level = &mut *self.placing.level;
        level.set_block(pos, state);
        if let Some(fluid) = level.block(pos).props().fluid() {
            level.schedule_fluid_tick(pos, fluid.fluid, 0);
        }
        if SHAPE_CHECK_BLOCKS
            .iter()
            .any(|block| same_block(state, *block))
        {
            level.mark_for_post_processing(pos);
        }
    }

    /// `generateBox` of an edge and a filling: the filling inside, the edge on the
    /// box's faces.
    pub fn generate_box(
        &mut self,
        from: Corner,
        to: Corner,
        edge: BlockState,
        fill: BlockState,
        skip_air: bool,
    ) {
        self.generate_box_selected(
            from,
            to,
            skip_air,
            |_, on_edge| {
                if on_edge { edge } else { fill }
            },
        );
    }

    /// `generateBox` with a `BlockSelector`, which is asked for every block placed,
    /// whether it is on the box's faces, and may draw.
    pub fn generate_box_selected(
        &mut self,
        from: Corner,
        to: Corner,
        skip_air: bool,
        mut select: impl FnMut(&mut WorldgenRandom, bool) -> BlockState,
    ) {
        let ([x0, y0, z0], [x1, y1, z1]) = (from, to);
        for y in y0..=y1 {
            for x in x0..=x1 {
                for z in z0..=z1 {
                    if skip_air && self.block(x, y, z).props().is_air() {
                        continue;
                    }
                    let on_edge = y == y0 || y == y1 || x == x0 || x == x1 || z == z0 || z == z1;
                    let state = select(self.placing.random, on_edge);
                    self.place_block(state, x, y, z);
                }
            }
        }
    }

    /// `generateMaybeBox`: each block by a chance, drawn first, of `probability`.
    #[expect(
        clippy::too_many_arguments,
        reason = "the game's StructurePiece.generateMaybeBox takes as many"
    )]
    pub fn generate_maybe_box(
        &mut self,
        probability: f32,
        from: Corner,
        to: Corner,
        edge: BlockState,
        fill: BlockState,
        skip_air: bool,
        inside_only: bool,
    ) {
        let ([x0, y0, z0], [x1, y1, z1]) = (from, to);
        for y in y0..=y1 {
            for x in x0..=x1 {
                for z in z0..=z1 {
                    if self.placing.random.next_f32() > probability
                        || (skip_air && self.block(x, y, z).props().is_air())
                        || (inside_only && !self.is_interior(x, y, z))
                    {
                        continue;
                    }
                    let on_edge = y == y0 || y == y1 || x == x0 || x == x1 || z == z0 || z == z1;
                    self.place_block(if on_edge { edge } else { fill }, x, y, z);
                }
            }
        }
    }

    /// `generateUpperHalfSphere`, in the game's single precision.
    pub fn generate_upper_half_sphere(
        &mut self,
        from: Corner,
        to: Corner,
        fill: BlockState,
        skip_air: bool,
    ) {
        let ([x0, y0, z0], [x1, y1, z1]) = (from, to);
        let diag_x = (x1 - x0 + 1) as f32;
        let diag_y = (y1 - y0 + 1) as f32;
        let diag_z = (z1 - z0 + 1) as f32;
        let cx = x0 as f32 + diag_x / 2.0;
        let cz = z0 as f32 + diag_z / 2.0;
        for y in y0..=y1 {
            let dy = (y - y0) as f32 / diag_y;
            for x in x0..=x1 {
                let dx = (x as f32 - cx) / (diag_x * 0.5);
                for z in z0..=z1 {
                    let dz = (z as f32 - cz) / (diag_z * 0.5);
                    if skip_air && self.block(x, y, z).props().is_air() {
                        continue;
                    }
                    if dx * dx + dy * dy + dz * dz <= 1.05 {
                        self.place_block(fill, x, y, z);
                    }
                }
            }
        }
    }

    /// `maybeGenerateBlock`: the chance is drawn whether or not the block lands in the
    /// chunk.
    pub fn maybe_generate_block(
        &mut self,
        probability: f32,
        x: i32,
        y: i32,
        z: i32,
        state: BlockState,
    ) {
        if self.placing.random.next_f32() < probability {
            self.place_block(state, x, y, z);
        }
    }

    /// `isInterior`: the block above is in the chunk and under the ocean floor that
    /// terrain left.
    pub fn is_interior(&mut self, x: i32, y: i32, z: i32) -> bool {
        let pos = self.world_pos(x, y + 1, z);
        self.inside(pos)
            && pos.y
                < self
                    .placing
                    .level
                    .height_at(Heightmap::OceanFloorWg, pos.x, pos.z)
    }

    /// `createChest`: a chest turned away from the wall beside it, with `table` and a
    /// seed drawn for it, unless one stands there already.
    pub fn create_chest(&mut self, x: i32, y: i32, z: i32, table: &str) -> bool {
        let pos = self.world_pos(x, y, z);
        let level = &mut *self.placing.level;
        if !self.placing.chunk_box.contains_xyz(pos.x, pos.y, pos.z)
            || same_block(level.block(pos), blocks::CHEST)
        {
            return false;
        }
        let state = reorient(level, pos, blocks::CHEST);
        level.set_block(pos, state);
        // `ChestBlockEntity.setLootTable(table, random.nextLong())`: the region makes
        // the chest's block entity as it is asked for, so the seed is always drawn.
        let seed = self.placing.random.next_i64();
        level.set_block_entity(pos, Some(loot_chest(table, seed)));
        true
    }

    /// A spawner of `mob` set straight into the world (`level.setBlock` and
    /// `SpawnerBlockEntity.setEntityId`), which draws nothing: a fresh spawner has
    /// no potentials to draw among.
    pub fn place_spawner(&mut self, pos: BlockPos, mob: &str) {
        let level = &mut *self.placing.level;
        level.set_block(pos, blocks::SPAWNER);
        level.set_block_entity(pos, Some(spawner_data(mob)));
    }
}

/// What the game saves for a chest whose loot is still to be rolled
/// (`RandomizableContainer.trySaveLootTable`): the table, the seed unless it is
/// nought, and no components.
pub(crate) fn loot_chest(table: &str, seed: i64) -> BlockEntity {
    let mut members = vec![("LootTable", Nbt::String(table.to_string()))];
    if seed != 0 {
        members.push(("LootTableSeed", Nbt::Long(seed)));
    }
    members.push(("components", compound(Vec::new())));
    block_entity("minecraft:chest", members)
}

/// `isFaceSturdy` of `face` for a full block's support.
pub(crate) fn sturdy(state: BlockState, face: Face) -> bool {
    state.props().is_face_sturdy(face, SupportType::Full)
}

#[cfg(test)]
pub(crate) mod tests {
    use clustine_noise::{Random, WorldgenRandom};
    use clustine_structures::geometry::{BoundingBox, Direction, IVec3, Identifier};
    use clustine_structures::piece::{StructurePiece, StructurePiecePayload};
    use clustine_world::ChunkPos;

    use super::*;
    use crate::testing::{World, settings};

    /// The writable box of the toy world's centre chunk, (0, 0).
    pub fn chunk_box() -> BoundingBox {
        BoundingBox::new(IVec3::new(0, -63, 0), IVec3::new(15, 319, 15))
    }

    /// Runs `f` with the toy world as a piece is placed into its centre chunk, with a
    /// random source seeded with `seed`; returns what `f` returns and the random
    /// source as `f` left it.
    pub fn placing<R>(
        world: &mut World,
        seed: u64,
        f: impl FnOnce(&mut Placing<'_, '_, '_>) -> R,
    ) -> (R, WorldgenRandom) {
        let mut random = WorldgenRandom::from_seed(seed);
        let result = world.with_level(|level| {
            let mut placing = Placing {
                level,
                random: &mut random,
                settings: settings(),
                chunk_box: chunk_box(),
                centre: ChunkPos { x: 0, z: 0 },
                pivot: IVec3::new(0, 0, 0),
            };
            f(&mut placing)
        });
        (result, random)
    }

    /// A piece of the game's code with a box, an orientation and a payload.
    pub fn piece(
        bounds: BoundingBox,
        orientation: Option<Direction>,
        payload: StructurePiecePayload,
    ) -> StructurePiece {
        StructurePiece {
            payload,
            ..StructurePiece::non_jigsaw(Identifier::of("minecraft:test"), bounds, 0, orientation)
        }
    }

    /// Whether `random` has drawn exactly `floats` single-precision numbers since it
    /// was seeded with `seed`.
    pub fn drew_floats(random: &mut WorldgenRandom, seed: u64, floats: usize) -> bool {
        let mut expected = WorldgenRandom::from_seed(seed);
        for _ in 0..floats {
            expected.next_f32();
        }
        expected.next_i64() == random.next_i64()
    }

    #[test]
    fn a_piece_facing_each_way_maps_its_corner_as_the_game_does() {
        let mut world = World::flat(blocks::STONE, 40);
        let bounds = BoundingBox::new(IVec3::new(2, 10, 3), IVec3::new(6, 14, 9));
        let corners: Vec<_> = [
            Direction::North,
            Direction::South,
            Direction::West,
            Direction::East,
        ]
        .into_iter()
        .map(|facing| {
            placing(&mut world, 1, |placing| {
                CodePiece::new(placing, bounds, Some(facing)).world_pos(1, 2, 3)
            })
            .0
        })
        .collect();
        assert_eq!(
            corners,
            [
                BlockPos::new(3, 12, 6),
                BlockPos::new(3, 12, 6),
                BlockPos::new(3, 12, 4),
                BlockPos::new(5, 12, 4),
            ]
        );
        let unoriented = placing(&mut world, 1, |placing| {
            CodePiece::new(placing, bounds, None).world_pos(1, 2, 3)
        });
        assert_eq!(unoriented.0, BlockPos::new(1, 2, 3));
    }

    #[test]
    fn blocks_outside_the_chunk_are_not_placed_and_read_as_air() {
        let mut world = World::flat(blocks::STONE, 40);
        let bounds = BoundingBox::new(IVec3::new(14, 10, 0), IVec3::new(20, 12, 4));
        placing(&mut world, 1, |placing| {
            let mut at = CodePiece::new(placing, bounds, Some(Direction::South));
            assert_eq!(at.block(5, 0, 0), blocks::AIR);
            assert_eq!(at.block(1, 0, 0), blocks::STONE);
            at.generate_box([0, 0, 0], [6, 2, 4], blocks::GLASS, blocks::GLASS, false);
        });
        assert!(world.writes.iter().all(|(pos, _)| pos.x <= 15));
        assert_eq!(world.writes.len(), 2 * 3 * 5);
    }

    #[test]
    fn a_turned_piece_turns_its_blocks_and_marks_those_whose_shape_waits() {
        let mut world = World::flat(blocks::STONE, 40);
        let bounds = BoundingBox::new(IVec3::new(2, 10, 2), IVec3::new(6, 14, 6));
        let torch = blocks::WALL_TORCH.with_property("facing", "north").unwrap();
        placing(&mut world, 1, |placing| {
            let mut at = CodePiece::new(placing, bounds, Some(Direction::East));
            at.place_block(torch, 1, 1, 1);
            at.place_block(blocks::WATER, 2, 1, 1);
        });
        let east = blocks::WALL_TORCH.with_property("facing", "east").unwrap();
        assert_eq!(world.get(BlockPos::new(3, 11, 3)), east);
        assert_eq!(world.marks, [BlockPos::new(3, 11, 3)]);
        assert_eq!(world.ticks.len(), 1);
        assert_eq!(world.ticks[0].0, BlockPos::new(3, 11, 4));
    }
}
