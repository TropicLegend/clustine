// Adapted from SteelMC (steel-worldgen/src/structure/utils.rs at commit
// 885c4b3e60ed79862c37311780774f76806cb714), which ports the game's own classes; the
// data it reads and the context it asks are Clustine's. See NOTICE in this crate.

use crate::geometry::IVec3;
use crate::geometry::{BoundingBox, Direction};
use clustine_noise::random::LegacyRandom;
use clustine_noise::random::Random;

const VANILLA_HORIZONTAL_DIRECTIONS: [Direction; 4] = [
    Direction::North,
    Direction::East,
    Direction::South,
    Direction::West,
];

pub(crate) fn random_horizontal_direction(rng: &mut LegacyRandom) -> Direction {
    VANILLA_HORIZONTAL_DIRECTIONS[rng.next_i32_bounded(4) as usize]
}

/// Vanilla's `StructurePiece.makeBoundingBox`: north/south keep width/depth,
/// east/west swap them.
pub(crate) const fn make_oriented_piece_bounding_box(
    chunk_min_x: i32,
    y: i32,
    chunk_min_z: i32,
    orientation: Direction,
    width: i32,
    height: i32,
    depth: i32,
) -> BoundingBox {
    let z_axis = matches!(orientation, Direction::North | Direction::South);
    let (box_width, box_depth) = if z_axis {
        (width, depth)
    } else {
        (depth, width)
    };
    BoundingBox::new(
        IVec3::new(chunk_min_x, y, chunk_min_z),
        IVec3::new(
            chunk_min_x + box_width - 1,
            y + height - 1,
            chunk_min_z + box_depth - 1,
        ),
    )
}
