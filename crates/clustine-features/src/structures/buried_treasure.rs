// Adapted from SteelMC's `steel-core/src/worldgen/structure/piece_placer/buried_treasure.rs`
// (see NOTICE in this crate), and checked against the pinned jar's
// `BuriedTreasurePieces$BuriedTreasurePiece` and `StructurePiece.createChest`.

//! Buried treasure (`BuriedTreasurePieces.BuriedTreasurePiece.postProcess`): down its
//! column from the ocean floor to the first block that stands on sandstone, stone,
//! andesite, granite or diorite; the open sides around that block are filled (with
//! the stone below where they would hang over a hole, with the block itself or sand
//! otherwise), and a chest with the treasure's loot takes its place. The piece's box
//! shrinks to that block, and is kept so.

use clustine_data::{BlockState, Face, blocks};
use clustine_structures::geometry::IVec3;
use clustine_structures::piece::StructurePiece;
use clustine_world::BlockPos;
use clustine_world::generation::PieceState;

use super::Placing;
use super::template_piece::{CHESTS, set_loot_table_if};
use crate::blocks::{relative, same_block};
use crate::context::Heightmap;
use crate::features::ground::monster_room::reorient;

/// `Direction.values()`: the order the sides are filled in.
const ALL: [Face; 6] = [
    Face::Down,
    Face::Up,
    Face::North,
    Face::South,
    Face::West,
    Face::East,
];

/// Places the treasure. `state` says how far the box has moved; the state returned is
/// to be kept.
pub(super) fn place(
    placing: &mut Placing<'_, '_, '_>,
    piece: &StructurePiece,
    mut state: PieceState,
) -> PieceState {
    let (x, z) = (piece.bounding_box.min_x(), piece.bounding_box.min_z());
    let mut y = placing.level.height_at(Heightmap::OceanFloorWg, x, z);
    while y > placing.level.min_y() {
        let at = BlockPos::new(x, y, z);
        let here = placing.level.block(at);
        let below = placing.level.block(BlockPos::new(x, y - 1, z));
        if !is_base(below) {
            y -= 1;
            continue;
        }
        let soft = if here.props().is_air() || is_liquid(here) {
            blocks::SAND
        } else {
            here
        };
        for face in ALL {
            let side = relative(at, face);
            let there = placing.level.block(side);
            if !(there.props().is_air() || is_liquid(there)) {
                continue;
            }
            let under = placing
                .level
                .block(BlockPos::new(side.x, side.y - 1, side.z));
            let hangs = under.props().is_air() || is_liquid(under);
            let fill = if hangs && face != Face::Up {
                below
            } else {
                soft
            };
            placing.level.set_block(side, fill);
        }
        // `this.boundingBox = new BoundingBox(pos)`.
        state.shift_y = y - piece.bounding_box.min_y();
        create_chest(placing, at);
        return state;
    }
    state
}

/// What the treasure may lie on.
fn is_base(state: BlockState) -> bool {
    [
        blocks::SANDSTONE,
        blocks::STONE,
        blocks::ANDESITE,
        blocks::GRANITE,
        blocks::DIORITE,
    ]
    .iter()
    .any(|base| same_block(state, *base))
}

/// `isLiquid`: water or lava, flowing or not.
fn is_liquid(state: BlockState) -> bool {
    same_block(state, blocks::WATER) || same_block(state, blocks::LAVA)
}

/// `StructurePiece.createChest` with no state given: inside the centre's box and where
/// no chest stands, a chest turned by its walls, and its loot where it keeps a block
/// entity.
fn create_chest(placing: &mut Placing<'_, '_, '_>, at: BlockPos) {
    let inside = placing.chunk_box.contains(IVec3::new(at.x, at.y, at.z));
    if !inside || same_block(placing.level.block(at), blocks::CHEST) {
        return;
    }
    let chest = reorient(placing.level, at, blocks::CHEST);
    placing.level.set_block(at, chest);
    set_loot_table_if(
        placing.level,
        &mut *placing.random,
        at,
        "minecraft:chests/buried_treasure",
        &CHESTS,
    );
}
