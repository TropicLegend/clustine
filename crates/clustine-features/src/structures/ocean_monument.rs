// Adapted from SteelMC's `steel-core/src/worldgen/structure/piece_placer/ocean_monument/`
// (`mod.rs`, `helpers.rs` and `shell.rs`; see NOTICE in this crate), which follows the
// game's `OceanMonumentPieces`.

//! The ocean monument (`OceanMonumentPieces.MonumentBuilding`): the building's shell of
//! prismarine with its wings, entrance, roof and walls, standing in water up to the sea
//! (or y 64, whichever is higher) and on pillars down to the ground; then its rooms
//! (`rooms`), each placed where its box reaches the centre chunk.
//!
//! Water is placed as the game's `generateWaterBox` places it: below the sea where no
//! ice or water stands, and air above the sea where no ice or water stands; every block
//! of water placed schedules its tick. The three elder guardians are not made, but what
//! making them draws from the region's random source is drawn.

mod rooms;

use clustine_data::{BlockState, blocks};
use clustine_structures::geometry::Direction;
use clustine_structures::ocean_monument::{
    OceanMonumentChildPieceKind, OceanMonumentPieceData, OceanMonumentRoomData,
};
use clustine_structures::piece::StructurePiece;

use super::Placing;
use super::scattered::{Piece, finalize_mob_spawn, inside};
use crate::blocks::same_block;

const BASE_GRAY: BlockState = blocks::PRISMARINE;
const BASE_LIGHT: BlockState = blocks::PRISMARINE_BRICKS;
const BASE_BLACK: BlockState = blocks::DARK_PRISMARINE;
const DOT_DECO: BlockState = BASE_LIGHT;
const LAMP: BlockState = blocks::SEA_LANTERN;
const FILL: BlockState = blocks::WATER;

/// `MonumentBuilding.postProcess`: the shell, then each room whose box reaches the
/// centre chunk, in the order the layout made them.
pub(super) fn place(
    placing: &mut Placing<'_, '_, '_>,
    piece: &StructurePiece,
    data: &OceanMonumentPieceData,
) {
    let building = Piece::new(piece.bounding_box, piece.orientation);
    shell(placing, &building);
    for child in &data.child_pieces {
        if !child.bounding_box.intersects(placing.chunk_box) {
            continue;
        }
        let at = Piece::new(child.bounding_box, piece.orientation);
        let p = &mut *placing;
        match &child.kind {
            OceanMonumentChildPieceKind::EntryRoom { room } => rooms::entry(p, &at, *room),
            OceanMonumentChildPieceKind::CoreRoom => rooms::core(p, &at),
            OceanMonumentChildPieceKind::DoubleXRoom { west, east } => {
                rooms::double_x(p, &at, *west, *east);
            }
            OceanMonumentChildPieceKind::DoubleXYRoom {
                west,
                east,
                west_up,
                east_up,
            } => rooms::double_xy(p, &at, [*west, *east, *west_up, *east_up]),
            OceanMonumentChildPieceKind::DoubleYRoom { room, above } => {
                rooms::double_y(p, &at, *room, *above);
            }
            OceanMonumentChildPieceKind::DoubleYZRoom {
                south,
                north,
                south_up,
                north_up,
            } => rooms::double_yz(p, &at, [*south, *north, *south_up, *north_up]),
            OceanMonumentChildPieceKind::DoubleZRoom { south, north } => {
                rooms::double_z(p, &at, *south, *north);
            }
            OceanMonumentChildPieceKind::SimpleRoom { room, main_design } => {
                rooms::simple(p, &at, *room, *main_design);
            }
            OceanMonumentChildPieceKind::SimpleTopRoom { room } => {
                rooms::simple_top(p, &at, *room);
            }
            OceanMonumentChildPieceKind::WingRoom { main_design } => {
                rooms::wing(p, &at, *main_design);
            }
            OceanMonumentChildPieceKind::Penthouse => rooms::penthouse(p, &at),
        }
    }
}

/// Whether a room has an opening on a side (`hasOpening`, by `get3DDataValue`).
fn open(room: OceanMonumentRoomData, direction: Direction) -> bool {
    room.has_opening[direction.get_3d_data_value() as usize]
}

/// `generateWaterBox`: water below the sea, and air at and above it where no water
/// stands already, leaving ice and water as they are.
fn water_box(p: &mut Placing<'_, '_, '_>, at: &Piece, from: [i32; 3], to: [i32; 3]) {
    let sea_level = p.settings.generation().sea_level;
    let ([x0, y0, z0], [x1, y1, z1]) = (from, to);
    for y in y0..=y1 {
        for x in x0..=x1 {
            for z in z0..=z1 {
                let state = at.get_block(p, x, y, z);
                let kept = [blocks::ICE, blocks::PACKED_ICE, blocks::BLUE_ICE, FILL]
                    .iter()
                    .any(|keep| same_block(state, *keep));
                if kept {
                    continue;
                }
                if at.world_pos(x, y, z).y >= sea_level && state != FILL {
                    at.place_block(p, blocks::AIR, x, y, z);
                } else {
                    at.place_block(p, FILL, x, y, z);
                }
            }
        }
    }
}

/// `generateBoxOnFillOnly`: `state` only where still water stands.
fn box_on_fill_only(
    p: &mut Placing<'_, '_, '_>,
    at: &Piece,
    from: [i32; 3],
    to: [i32; 3],
    state: BlockState,
) {
    let ([x0, y0, z0], [x1, y1, z1]) = (from, to);
    for y in y0..=y1 {
        for x in x0..=x1 {
            for z in z0..=z1 {
                if at.get_block(p, x, y, z) == FILL {
                    at.place_block(p, state, x, y, z);
                }
            }
        }
    }
}

/// `generateDefaultFloor`: a floor of eight by eight, with a hole in its middle where
/// the room opens downwards.
fn default_floor(p: &mut Placing<'_, '_, '_>, at: &Piece, x: i32, z: i32, down: bool) {
    if down {
        at.fill(p, [x, 0, z], [x + 2, 0, z + 7], BASE_GRAY);
        at.fill(p, [x + 5, 0, z], [x + 7, 0, z + 7], BASE_GRAY);
        at.fill(p, [x + 3, 0, z], [x + 4, 0, z + 2], BASE_GRAY);
        at.fill(p, [x + 3, 0, z + 5], [x + 4, 0, z + 7], BASE_GRAY);
        at.fill(p, [x + 3, 0, z + 2], [x + 4, 0, z + 2], BASE_LIGHT);
        at.fill(p, [x + 3, 0, z + 5], [x + 4, 0, z + 5], BASE_LIGHT);
        at.fill(p, [x + 2, 0, z + 3], [x + 2, 0, z + 4], BASE_LIGHT);
        at.fill(p, [x + 5, 0, z + 3], [x + 5, 0, z + 4], BASE_LIGHT);
    } else {
        at.fill(p, [x, 0, z], [x + 7, 0, z + 7], BASE_GRAY);
    }
}

/// `spawnElder`: an elder guardian where the position is in the centre chunk, of
/// which only the draws of `finalizeSpawn` are kept.
fn spawn_elder(p: &mut Placing<'_, '_, '_>, at: &Piece, x: i32, y: i32, z: i32) {
    if inside(p.chunk_box, at.world_pos(x, y, z)) {
        finalize_mob_spawn(p);
    }
}

/// The building's own blocks (`MonumentBuilding.postProcess` before its rooms).
fn shell(p: &mut Placing<'_, '_, '_>, at: &Piece) {
    let sea_level = p.settings.generation().sea_level;
    let water_height = sea_level.max(64) - at.bounding_box.min_y();
    water_box(p, at, [0, 0, 0], [58, water_height, 58]);
    wing(p, at, false, 0);
    wing(p, at, true, 33);
    entrance_archs(p, at);
    entrance_wall(p, at);
    roof_piece(p, at);
    lower_wall(p, at);
    middle_wall(p, at);
    upper_wall(p, at);

    // The pillars under the building's edge.
    for i in 0..7 {
        let mut j = 0;
        while j < 7 {
            if j == 0 && i == 3 {
                j = 6;
            }
            let (x, z) = (i * 9, j * 9);
            for dx in 0..4 {
                for dz in 0..4 {
                    at.place_block(p, BASE_LIGHT, x + dx, 0, z + dz);
                    at.fill_column_down(p, BASE_LIGHT, x + dx, -1, z + dz);
                }
            }
            if i != 0 && i != 6 {
                j += 6;
            } else {
                j += 1;
            }
        }
    }

    // The water around the building, stepping up as it goes out.
    for i in 0..5 {
        water_box(p, at, [-1 - i, i * 2, -1 - i], [-1 - i, 23, 58 + i]);
        water_box(p, at, [58 + i, i * 2, -1 - i], [58 + i, 23, 58 + i]);
        water_box(p, at, [-i, i * 2, -1 - i], [57 + i, 23, -1 - i]);
        water_box(p, at, [-i, i * 2, 58 + i], [57 + i, 23, 58 + i]);
    }
}

/// `generateWing`.
fn wing(p: &mut Placing<'_, '_, '_>, at: &Piece, flipped: bool, x: i32) {
    if !at.chunk_intersects(p.chunk_box, x, 0, x + 23, 20) {
        return;
    }
    at.fill(p, [x, 0, 0], [x + 24, 0, 20], BASE_GRAY);
    water_box(p, at, [x, 1, 0], [x + 24, 10, 20]);
    for i in 0..4 {
        at.fill(p, [x + i, i + 1, i], [x + i, i + 1, 20], BASE_LIGHT);
        at.fill(
            p,
            [x + i + 7, i + 5, i + 7],
            [x + i + 7, i + 5, 20],
            BASE_LIGHT,
        );
        at.fill(
            p,
            [x + 17 - i, i + 5, i + 7],
            [x + 17 - i, i + 5, 20],
            BASE_LIGHT,
        );
        at.fill(
            p,
            [x + 24 - i, i + 1, i],
            [x + 24 - i, i + 1, 20],
            BASE_LIGHT,
        );
        at.fill(p, [x + i + 1, i + 1, i], [x + 23 - i, i + 1, i], BASE_LIGHT);
        at.fill(
            p,
            [x + i + 8, i + 5, i + 7],
            [x + 16 - i, i + 5, i + 7],
            BASE_LIGHT,
        );
    }
    at.fill(p, [x + 4, 4, 4], [x + 6, 4, 20], BASE_GRAY);
    at.fill(p, [x + 7, 4, 4], [x + 17, 4, 6], BASE_GRAY);
    at.fill(p, [x + 18, 4, 4], [x + 20, 4, 20], BASE_GRAY);
    at.fill(p, [x + 11, 8, 11], [x + 13, 8, 20], BASE_GRAY);
    at.place_block(p, DOT_DECO, x + 12, 9, 12);
    at.place_block(p, DOT_DECO, x + 12, 9, 15);
    at.place_block(p, DOT_DECO, x + 12, 9, 18);
    let left = x + if flipped { 19 } else { 5 };
    let right = x + if flipped { 5 } else { 19 };
    for z in (5..=20).rev().step_by(3) {
        at.place_block(p, DOT_DECO, left, 5, z);
    }
    for z in (7..=19).rev().step_by(3) {
        at.place_block(p, DOT_DECO, right, 5, z);
    }
    for i in 0..4 {
        let deco = if flipped {
            x + 24 - (17 - i * 3)
        } else {
            x + 17 - i * 3
        };
        at.place_block(p, DOT_DECO, deco, 5, 5);
    }
    at.place_block(p, DOT_DECO, right, 5, 5);
    at.fill(p, [x + 11, 1, 12], [x + 13, 7, 12], BASE_GRAY);
    at.fill(p, [x + 12, 1, 11], [x + 12, 7, 13], BASE_GRAY);
}

/// `generateEntranceArchs`.
fn entrance_archs(p: &mut Placing<'_, '_, '_>, at: &Piece) {
    if !at.chunk_intersects(p.chunk_box, 22, 5, 35, 17) {
        return;
    }
    water_box(p, at, [25, 0, 0], [32, 8, 20]);
    for i in 0..4 {
        let z = 5 + i * 4;
        at.fill(p, [24, 2, z], [24, 4, z], BASE_LIGHT);
        at.fill(p, [22, 4, z], [23, 4, z], BASE_LIGHT);
        at.place_block(p, BASE_LIGHT, 25, 5, z);
        at.place_block(p, BASE_LIGHT, 26, 6, z);
        at.place_block(p, LAMP, 26, 5, z);
        at.fill(p, [33, 2, z], [33, 4, z], BASE_LIGHT);
        at.fill(p, [34, 4, z], [35, 4, z], BASE_LIGHT);
        at.place_block(p, BASE_LIGHT, 32, 5, z);
        at.place_block(p, BASE_LIGHT, 31, 6, z);
        at.place_block(p, LAMP, 31, 5, z);
        at.fill(p, [27, 6, z], [30, 6, z], BASE_GRAY);
    }
}

/// `generateEntranceWall`.
fn entrance_wall(p: &mut Placing<'_, '_, '_>, at: &Piece) {
    if !at.chunk_intersects(p.chunk_box, 15, 20, 42, 21) {
        return;
    }
    at.fill(p, [15, 0, 21], [42, 0, 21], BASE_GRAY);
    water_box(p, at, [26, 1, 21], [31, 3, 21]);
    at.fill(p, [21, 12, 21], [36, 12, 21], BASE_GRAY);
    at.fill(p, [17, 11, 21], [40, 11, 21], BASE_GRAY);
    at.fill(p, [16, 10, 21], [41, 10, 21], BASE_GRAY);
    at.fill(p, [15, 7, 21], [42, 9, 21], BASE_GRAY);
    at.fill(p, [16, 6, 21], [41, 6, 21], BASE_GRAY);
    at.fill(p, [17, 5, 21], [40, 5, 21], BASE_GRAY);
    at.fill(p, [21, 4, 21], [36, 4, 21], BASE_GRAY);
    at.fill(p, [22, 3, 21], [26, 3, 21], BASE_GRAY);
    at.fill(p, [31, 3, 21], [35, 3, 21], BASE_GRAY);
    at.fill(p, [23, 2, 21], [25, 2, 21], BASE_GRAY);
    at.fill(p, [32, 2, 21], [34, 2, 21], BASE_GRAY);
    at.fill(p, [28, 4, 20], [29, 4, 21], BASE_LIGHT);
    at.place_block(p, BASE_LIGHT, 27, 3, 21);
    at.place_block(p, BASE_LIGHT, 30, 3, 21);
    at.place_block(p, BASE_LIGHT, 26, 2, 21);
    at.place_block(p, BASE_LIGHT, 31, 2, 21);
    at.place_block(p, BASE_LIGHT, 25, 1, 21);
    at.place_block(p, BASE_LIGHT, 32, 1, 21);
    for i in 0..7 {
        at.place_block(p, BASE_BLACK, 28 - i, 6 + i, 21);
        at.place_block(p, BASE_BLACK, 29 + i, 6 + i, 21);
    }
    for i in 0..4 {
        at.place_block(p, BASE_BLACK, 28 - i, 9 + i, 21);
        at.place_block(p, BASE_BLACK, 29 + i, 9 + i, 21);
    }
    at.place_block(p, BASE_BLACK, 28, 12, 21);
    at.place_block(p, BASE_BLACK, 29, 12, 21);
    for i in 0..3 {
        at.place_block(p, BASE_BLACK, 22 - i * 2, 8, 21);
        at.place_block(p, BASE_BLACK, 22 - i * 2, 9, 21);
        at.place_block(p, BASE_BLACK, 35 + i * 2, 8, 21);
        at.place_block(p, BASE_BLACK, 35 + i * 2, 9, 21);
    }
    water_box(p, at, [15, 13, 21], [42, 15, 21]);
    water_box(p, at, [15, 1, 21], [15, 6, 21]);
    water_box(p, at, [16, 1, 21], [16, 5, 21]);
    water_box(p, at, [17, 1, 21], [20, 4, 21]);
    water_box(p, at, [21, 1, 21], [21, 3, 21]);
    water_box(p, at, [22, 1, 21], [22, 2, 21]);
    water_box(p, at, [23, 1, 21], [24, 1, 21]);
    water_box(p, at, [42, 1, 21], [42, 6, 21]);
    water_box(p, at, [41, 1, 21], [41, 5, 21]);
    water_box(p, at, [37, 1, 21], [40, 4, 21]);
    water_box(p, at, [36, 1, 21], [36, 3, 21]);
    water_box(p, at, [33, 1, 21], [34, 1, 21]);
    water_box(p, at, [35, 1, 21], [35, 2, 21]);
}

/// `generateRoofPiece`.
fn roof_piece(p: &mut Placing<'_, '_, '_>, at: &Piece) {
    if !at.chunk_intersects(p.chunk_box, 21, 21, 36, 36) {
        return;
    }
    at.fill(p, [21, 0, 22], [36, 0, 36], BASE_GRAY);
    water_box(p, at, [21, 1, 22], [36, 23, 36]);
    for i in 0..4 {
        at.fill(
            p,
            [21 + i, 13 + i, 21 + i],
            [36 - i, 13 + i, 21 + i],
            BASE_LIGHT,
        );
        at.fill(
            p,
            [21 + i, 13 + i, 36 - i],
            [36 - i, 13 + i, 36 - i],
            BASE_LIGHT,
        );
        at.fill(
            p,
            [21 + i, 13 + i, 22 + i],
            [21 + i, 13 + i, 35 - i],
            BASE_LIGHT,
        );
        at.fill(
            p,
            [36 - i, 13 + i, 22 + i],
            [36 - i, 13 + i, 35 - i],
            BASE_LIGHT,
        );
    }
    at.fill(p, [25, 16, 25], [32, 16, 32], BASE_GRAY);
    at.fill(p, [25, 17, 25], [25, 19, 25], BASE_LIGHT);
    at.fill(p, [32, 17, 25], [32, 19, 25], BASE_LIGHT);
    at.fill(p, [25, 17, 32], [25, 19, 32], BASE_LIGHT);
    at.fill(p, [32, 17, 32], [32, 19, 32], BASE_LIGHT);
    at.place_block(p, BASE_LIGHT, 26, 20, 26);
    at.place_block(p, BASE_LIGHT, 27, 21, 27);
    at.place_block(p, LAMP, 27, 20, 27);
    at.place_block(p, BASE_LIGHT, 26, 20, 31);
    at.place_block(p, BASE_LIGHT, 27, 21, 30);
    at.place_block(p, LAMP, 27, 20, 30);
    at.place_block(p, BASE_LIGHT, 31, 20, 31);
    at.place_block(p, BASE_LIGHT, 30, 21, 30);
    at.place_block(p, LAMP, 30, 20, 30);
    at.place_block(p, BASE_LIGHT, 31, 20, 26);
    at.place_block(p, BASE_LIGHT, 30, 21, 27);
    at.place_block(p, LAMP, 30, 20, 27);
    at.fill(p, [28, 21, 27], [29, 21, 27], BASE_GRAY);
    at.fill(p, [27, 21, 28], [27, 21, 29], BASE_GRAY);
    at.fill(p, [28, 21, 30], [29, 21, 30], BASE_GRAY);
    at.fill(p, [30, 21, 28], [30, 21, 29], BASE_GRAY);
}

/// `generateLowerWall`.
fn lower_wall(p: &mut Placing<'_, '_, '_>, at: &Piece) {
    if at.chunk_intersects(p.chunk_box, 0, 21, 6, 58) {
        at.fill(p, [0, 0, 21], [6, 0, 57], BASE_GRAY);
        water_box(p, at, [0, 1, 21], [6, 7, 57]);
        at.fill(p, [4, 4, 21], [6, 4, 53], BASE_GRAY);
        for i in 0..4 {
            at.fill(p, [i, i + 1, 21], [i, i + 1, 57 - i], BASE_LIGHT);
        }
        for z in (23..53).step_by(3) {
            at.place_block(p, DOT_DECO, 5, 5, z);
        }
        at.place_block(p, DOT_DECO, 5, 5, 52);
        // The game lays the steps of this side twice.
        for i in 0..4 {
            at.fill(p, [i, i + 1, 21], [i, i + 1, 57 - i], BASE_LIGHT);
        }
        at.fill(p, [4, 1, 52], [6, 3, 52], BASE_GRAY);
        at.fill(p, [5, 1, 51], [5, 3, 53], BASE_GRAY);
    }
    if at.chunk_intersects(p.chunk_box, 51, 21, 58, 58) {
        at.fill(p, [51, 0, 21], [57, 0, 57], BASE_GRAY);
        water_box(p, at, [51, 1, 21], [57, 7, 57]);
        at.fill(p, [51, 4, 21], [53, 4, 53], BASE_GRAY);
        for i in 0..4 {
            at.fill(p, [57 - i, i + 1, 21], [57 - i, i + 1, 57 - i], BASE_LIGHT);
        }
        for z in (23..53).step_by(3) {
            at.place_block(p, DOT_DECO, 52, 5, z);
        }
        at.place_block(p, DOT_DECO, 52, 5, 52);
        at.fill(p, [51, 1, 52], [53, 3, 52], BASE_GRAY);
        at.fill(p, [52, 1, 51], [52, 3, 53], BASE_GRAY);
    }
    if at.chunk_intersects(p.chunk_box, 0, 51, 57, 57) {
        at.fill(p, [7, 0, 51], [50, 0, 57], BASE_GRAY);
        water_box(p, at, [7, 1, 51], [50, 10, 57]);
        for i in 0..4 {
            at.fill(
                p,
                [i + 1, i + 1, 57 - i],
                [56 - i, i + 1, 57 - i],
                BASE_LIGHT,
            );
        }
    }
}

/// `generateMiddleWall`.
fn middle_wall(p: &mut Placing<'_, '_, '_>, at: &Piece) {
    if at.chunk_intersects(p.chunk_box, 7, 21, 13, 50) {
        at.fill(p, [7, 0, 21], [13, 0, 50], BASE_GRAY);
        water_box(p, at, [7, 1, 21], [13, 10, 50]);
        at.fill(p, [11, 8, 21], [13, 8, 53], BASE_GRAY);
        for i in 0..4 {
            at.fill(p, [i + 7, i + 5, 21], [i + 7, i + 5, 54], BASE_LIGHT);
        }
        for z in (21..=45).step_by(3) {
            at.place_block(p, DOT_DECO, 12, 9, z);
        }
    }
    if at.chunk_intersects(p.chunk_box, 44, 21, 50, 54) {
        at.fill(p, [44, 0, 21], [50, 0, 50], BASE_GRAY);
        water_box(p, at, [44, 1, 21], [50, 10, 50]);
        at.fill(p, [44, 8, 21], [46, 8, 53], BASE_GRAY);
        for i in 0..4 {
            at.fill(p, [50 - i, i + 5, 21], [50 - i, i + 5, 54], BASE_LIGHT);
        }
        for z in (21..=45).step_by(3) {
            at.place_block(p, DOT_DECO, 45, 9, z);
        }
    }
    if at.chunk_intersects(p.chunk_box, 8, 44, 49, 54) {
        at.fill(p, [14, 0, 44], [43, 0, 50], BASE_GRAY);
        water_box(p, at, [14, 1, 44], [43, 10, 50]);
        for x in (12..=45).step_by(3) {
            at.place_block(p, DOT_DECO, x, 9, 45);
            at.place_block(p, DOT_DECO, x, 9, 52);
            if matches!(x, 12 | 18 | 24 | 33 | 39 | 45) {
                at.place_block(p, DOT_DECO, x, 9, 47);
                at.place_block(p, DOT_DECO, x, 9, 50);
                at.place_block(p, DOT_DECO, x, 10, 45);
                at.place_block(p, DOT_DECO, x, 10, 46);
                at.place_block(p, DOT_DECO, x, 10, 51);
                at.place_block(p, DOT_DECO, x, 10, 52);
                at.place_block(p, DOT_DECO, x, 11, 47);
                at.place_block(p, DOT_DECO, x, 11, 50);
                at.place_block(p, DOT_DECO, x, 12, 48);
                at.place_block(p, DOT_DECO, x, 12, 49);
            }
        }
        for i in 0..3 {
            at.fill(p, [8 + i, 5 + i, 54], [49 - i, 5 + i, 54], BASE_GRAY);
        }
        at.fill(p, [11, 8, 54], [46, 8, 54], BASE_LIGHT);
        at.fill(p, [14, 8, 44], [43, 8, 53], BASE_GRAY);
    }
}

/// `generateUpperWall`.
fn upper_wall(p: &mut Placing<'_, '_, '_>, at: &Piece) {
    if at.chunk_intersects(p.chunk_box, 14, 21, 20, 43) {
        at.fill(p, [14, 0, 21], [20, 0, 43], BASE_GRAY);
        water_box(p, at, [14, 1, 22], [20, 14, 43]);
        at.fill(p, [18, 12, 22], [20, 12, 39], BASE_GRAY);
        at.fill(p, [18, 12, 21], [20, 12, 21], BASE_LIGHT);
        for i in 0..4 {
            at.fill(p, [i + 14, i + 9, 21], [i + 14, i + 9, 43 - i], BASE_LIGHT);
        }
        for z in (23..=39).step_by(3) {
            at.place_block(p, DOT_DECO, 19, 13, z);
        }
    }
    if at.chunk_intersects(p.chunk_box, 37, 21, 43, 43) {
        at.fill(p, [37, 0, 21], [43, 0, 43], BASE_GRAY);
        water_box(p, at, [37, 1, 22], [43, 14, 43]);
        at.fill(p, [37, 12, 22], [39, 12, 39], BASE_GRAY);
        at.fill(p, [37, 12, 21], [39, 12, 21], BASE_LIGHT);
        for i in 0..4 {
            at.fill(p, [43 - i, i + 9, 21], [43 - i, i + 9, 43 - i], BASE_LIGHT);
        }
        for z in (23..=39).step_by(3) {
            at.place_block(p, DOT_DECO, 38, 13, z);
        }
    }
    if at.chunk_intersects(p.chunk_box, 15, 37, 42, 43) {
        at.fill(p, [21, 0, 37], [36, 0, 43], BASE_GRAY);
        water_box(p, at, [21, 1, 37], [36, 14, 43]);
        at.fill(p, [21, 12, 37], [36, 12, 39], BASE_GRAY);
        for i in 0..4 {
            at.fill(
                p,
                [15 + i, i + 9, 43 - i],
                [42 - i, i + 9, 43 - i],
                BASE_LIGHT,
            );
        }
        for x in (21..=36).step_by(3) {
            at.place_block(p, DOT_DECO, x, 13, 38);
        }
    }
}

#[cfg(test)]
mod tests {
    use clustine_noise::Random;
    use clustine_structures::geometry::{BoundingBox, IVec3};
    use clustine_structures::ocean_monument::OceanMonumentChildPiece;
    use clustine_structures::piece::{ProceduralPieceData, StructurePiecePayload};
    use clustine_world::BlockPos;

    use super::*;
    use crate::structures::scattered::region_random;
    use crate::structures::scattered::testing::{Run, chunk, fresh_region_random, piece};
    use crate::testing::World;

    fn child(
        from: (i32, i32, i32),
        to: (i32, i32, i32),
        kind: OceanMonumentChildPieceKind,
    ) -> OceanMonumentChildPiece {
        OceanMonumentChildPiece {
            bounding_box: BoundingBox::new(
                IVec3::new(from.0, from.1, from.2),
                IVec3::new(to.0, to.1, to.2),
            ),
            kind,
        }
    }

    /// A building facing north over the centre chunk, with a core room, a wing of the
    /// first design and the penthouse, all reaching into the centre chunk.
    fn monument() -> (StructurePiece, OceanMonumentPieceData) {
        let data = OceanMonumentPieceData {
            child_pieces: vec![
                child(
                    (0, 40, 0),
                    (15, 47, 15),
                    OceanMonumentChildPieceKind::CoreRoom,
                ),
                child(
                    (0, 40, 0),
                    (22, 47, 20),
                    OceanMonumentChildPieceKind::WingRoom { main_design: 0 },
                ),
                child(
                    (0, 52, 0),
                    (13, 56, 13),
                    OceanMonumentChildPieceKind::Penthouse,
                ),
            ],
        };
        let building = piece(
            BoundingBox::new(IVec3::new(-20, 39, -20), IVec3::new(37, 61, 37)),
            Direction::North,
            StructurePiecePayload::Procedural(ProceduralPieceData::OceanMonument(data.clone())),
        );
        (building, data)
    }

    #[test]
    fn the_core_room_holds_eight_blocks_of_gold() {
        let (building, mut data) = monument();
        // The core alone, which the wing of this made-up layout would overlap.
        data.child_pieces.truncate(1);
        let mut run = Run::new(World::flat(blocks::SAND, 30), 4);
        run.place(chunk(0, 0), |p| place(p, &building, &data));
        let core = Piece::new(data.child_pieces[0].bounding_box, Some(Direction::North));
        for (x, y, z) in [(7, 4, 7), (8, 4, 7), (7, 5, 8), (8, 5, 8)] {
            assert_eq!(run.get(core.world_pos(x, y, z)), blocks::GOLD_BLOCK);
        }
        assert_eq!(run.get(core.world_pos(6, 3, 6)), LAMP);
    }

    #[test]
    fn the_building_stands_in_water_below_the_sea_with_its_ticks_and_air_above() {
        let (building, data) = monument();
        let mut run = Run::new(World::flat(blocks::SAND, 30), 4);
        run.place(chunk(0, 0), |p| place(p, &building, &data));
        let sea_level = crate::testing::settings().generation().sea_level;
        let world = &run.world.world;
        assert!(!world.ticks.is_empty());
        assert!(
            world
                .writes
                .iter()
                .filter(|(_, state)| *state == FILL)
                .all(|(at, _)| at.y < sea_level)
        );
        // Above the sea, up to y 64, the building's water box leaves air.
        assert_eq!(run.get(BlockPos::new(3, 64, 3)), blocks::AIR);
        assert!(
            world
                .writes
                .iter()
                .any(|(at, state)| at.y == sea_level && *state == blocks::AIR)
        );
    }

    #[test]
    fn the_elder_guardians_of_the_wing_and_the_penthouse_draw_from_the_region() {
        let (building, data) = monument();
        let mut run = Run::new(World::flat(blocks::SAND, 30), 4);
        let after = run.place(chunk(0, 0), |p| {
            place(p, &building, &data);
            region_random(p).next_i64()
        });
        let mut expected = fresh_region_random();
        for _ in 0..2 {
            expected.next_f64();
            expected.next_f64();
            expected.next_f32();
        }
        assert_eq!(after, expected.next_i64());
    }

    #[test]
    fn a_room_whose_box_misses_the_centre_chunk_is_not_placed() {
        let (building, data) = monument();
        let mut run = Run::new(World::flat(blocks::SAND, 30), 4);
        let after = run.place(chunk(-1, -1), |p| {
            place(p, &building, &data);
            region_random(p).next_i64()
        });
        assert_eq!(after, fresh_region_random().next_i64());
        assert!(
            !run.world
                .world
                .writes
                .iter()
                .any(|(_, state)| *state == blocks::GOLD_BLOCK)
        );
    }
}
