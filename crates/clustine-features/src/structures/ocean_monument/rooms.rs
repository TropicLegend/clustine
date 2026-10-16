// Adapted from SteelMC's `steel-core/src/worldgen/structure/piece_placer/ocean_monument/`
// (`room_core.rs`, `room_double.rs`, `room_simple.rs` and `room_special.rs`; see
// NOTICE in this crate), which follow the game's `OceanMonumentPieces`.

//! The monument's rooms, each in its own frame of the building's orientation: the
//! entry, the core with its gold, the rooms of one, two and four cells with their
//! designs, the top rooms full of wet sponge, the two wings with their elder
//! guardians, and the penthouse with the third.
//!
//! Only the simple room (its centre pillar) and the simple top room (its sponges)
//! draw from the run's random source.

use clustine_data::blocks;
use clustine_noise::Random;
use clustine_structures::geometry::Direction;
use clustine_structures::ocean_monument::OceanMonumentRoomData as Room;

use super::{
    BASE_BLACK, BASE_GRAY, BASE_LIGHT, LAMP, box_on_fill_only, default_floor, open, spawn_elder,
    water_box,
};
use crate::structures::Placing;
use crate::structures::scattered::Piece;

/// Whether a room is above the lowest floor of the grid, so that it lays its own floor.
fn above_ground(room: Room) -> bool {
    room.index / 25 > 0
}

/// `OceanMonumentEntryRoom`.
pub(super) fn entry(p: &mut Placing<'_, '_, '_>, at: &Piece, room: Room) {
    at.fill(p, [0, 3, 0], [2, 3, 7], BASE_LIGHT);
    at.fill(p, [5, 3, 0], [7, 3, 7], BASE_LIGHT);
    at.fill(p, [0, 2, 0], [1, 2, 7], BASE_LIGHT);
    at.fill(p, [6, 2, 0], [7, 2, 7], BASE_LIGHT);
    at.fill(p, [0, 1, 0], [0, 1, 7], BASE_LIGHT);
    at.fill(p, [7, 1, 0], [7, 1, 7], BASE_LIGHT);
    at.fill(p, [0, 1, 7], [7, 3, 7], BASE_LIGHT);
    at.fill(p, [1, 1, 0], [2, 3, 0], BASE_LIGHT);
    at.fill(p, [5, 1, 0], [6, 3, 0], BASE_LIGHT);
    if open(room, Direction::North) {
        water_box(p, at, [3, 1, 7], [4, 2, 7]);
    }
    if open(room, Direction::West) {
        water_box(p, at, [0, 1, 3], [1, 2, 4]);
    }
    if open(room, Direction::East) {
        water_box(p, at, [6, 1, 3], [7, 2, 4]);
    }
}

/// `OceanMonumentCoreRoom`: the treasure room, eight blocks of gold in a shell of dark
/// prismarine.
pub(super) fn core(p: &mut Placing<'_, '_, '_>, at: &Piece) {
    box_on_fill_only(p, at, [1, 8, 0], [14, 8, 14], BASE_GRAY);
    at.fill(p, [0, 7, 0], [0, 7, 15], BASE_LIGHT);
    at.fill(p, [15, 7, 0], [15, 7, 15], BASE_LIGHT);
    at.fill(p, [1, 7, 0], [15, 7, 0], BASE_LIGHT);
    at.fill(p, [1, 7, 15], [14, 7, 15], BASE_LIGHT);
    for y in 1..=6 {
        let block = if y == 2 || y == 6 {
            BASE_GRAY
        } else {
            BASE_LIGHT
        };
        for x in [0, 15] {
            at.fill(p, [x, y, 0], [x, y, 1], block);
            at.fill(p, [x, y, 6], [x, y, 9], block);
            at.fill(p, [x, y, 14], [x, y, 15], block);
        }
        at.fill(p, [1, y, 0], [1, y, 0], block);
        at.fill(p, [6, y, 0], [9, y, 0], block);
        at.fill(p, [14, y, 0], [14, y, 0], block);
        at.fill(p, [1, y, 15], [14, y, 15], block);
    }
    at.fill(p, [6, 3, 6], [9, 6, 9], BASE_BLACK);
    at.fill(p, [7, 4, 7], [8, 5, 8], blocks::GOLD_BLOCK);
    for y in [3, 6] {
        for x in [6, 9] {
            at.place_block(p, LAMP, x, y, 6);
            at.place_block(p, LAMP, x, y, 9);
        }
    }
    for (from, to) in [
        ([5, 1, 6], [5, 2, 6]),
        ([5, 1, 9], [5, 2, 9]),
        ([10, 1, 6], [10, 2, 6]),
        ([10, 1, 9], [10, 2, 9]),
        ([6, 1, 5], [6, 2, 5]),
        ([9, 1, 5], [9, 2, 5]),
        ([6, 1, 10], [6, 2, 10]),
        ([9, 1, 10], [9, 2, 10]),
        ([5, 2, 5], [5, 6, 5]),
        ([5, 2, 10], [5, 6, 10]),
        ([10, 2, 5], [10, 6, 5]),
        ([10, 2, 10], [10, 6, 10]),
        ([5, 7, 1], [5, 7, 6]),
        ([10, 7, 1], [10, 7, 6]),
        ([5, 7, 9], [5, 7, 14]),
        ([10, 7, 9], [10, 7, 14]),
        ([1, 7, 5], [6, 7, 5]),
        ([1, 7, 10], [6, 7, 10]),
        ([9, 7, 5], [14, 7, 5]),
        ([9, 7, 10], [14, 7, 10]),
        ([2, 1, 2], [2, 1, 3]),
        ([3, 1, 2], [3, 1, 2]),
        ([13, 1, 2], [13, 1, 3]),
        ([12, 1, 2], [12, 1, 2]),
        ([2, 1, 12], [2, 1, 13]),
        ([3, 1, 13], [3, 1, 13]),
        ([13, 1, 12], [13, 1, 13]),
        ([12, 1, 13], [12, 1, 13]),
    ] {
        at.fill(p, from, to, BASE_LIGHT);
    }
}

/// The water of a room's openings on its sides, for a cell whose frame starts at
/// `(x, y, z)` and is eight blocks across: south, north, west and east, where
/// `sides` says.
fn side_openings(
    p: &mut Placing<'_, '_, '_>,
    at: &Piece,
    room: Room,
    [x, y, z]: [i32; 3],
    sides: &[Direction],
) {
    for side in sides {
        if !open(room, *side) {
            continue;
        }
        let (from, to) = match side {
            Direction::South => ([x + 3, y, z], [x + 4, y + 1, z]),
            Direction::North => ([x + 3, y, z + 7], [x + 4, y + 1, z + 7]),
            Direction::West => ([x, y, z + 3], [x, y + 1, z + 4]),
            _ => ([x + 7, y, z + 3], [x + 7, y + 1, z + 4]),
        };
        water_box(p, at, from, to);
    }
}

/// `OceanMonumentDoubleXRoom`: two cells along x.
pub(super) fn double_x(p: &mut Placing<'_, '_, '_>, at: &Piece, west: Room, east: Room) {
    if above_ground(west) {
        default_floor(p, at, 8, 0, open(east, Direction::Down));
        default_floor(p, at, 0, 0, open(west, Direction::Down));
    }
    if !west.has_up_connection {
        box_on_fill_only(p, at, [1, 4, 1], [7, 4, 6], BASE_GRAY);
    }
    if !east.has_up_connection {
        box_on_fill_only(p, at, [8, 4, 1], [14, 4, 6], BASE_GRAY);
    }
    for (y, block) in [(3, BASE_LIGHT), (2, BASE_GRAY), (1, BASE_LIGHT)] {
        at.fill(p, [0, y, 0], [0, y, 7], block);
        at.fill(p, [15, y, 0], [15, y, 7], block);
        at.fill(p, [1, y, 0], [15, y, 0], block);
        at.fill(p, [1, y, 7], [14, y, 7], block);
    }
    at.fill(p, [5, 1, 0], [10, 1, 4], BASE_LIGHT);
    at.fill(p, [6, 2, 0], [9, 2, 3], BASE_GRAY);
    at.fill(p, [5, 3, 0], [10, 3, 4], BASE_LIGHT);
    at.place_block(p, LAMP, 6, 2, 3);
    at.place_block(p, LAMP, 9, 2, 3);
    let (s, n, w, e) = (
        Direction::South,
        Direction::North,
        Direction::West,
        Direction::East,
    );
    side_openings(p, at, west, [0, 1, 0], &[s, n, w]);
    side_openings(p, at, east, [8, 1, 0], &[s, n, e]);
}

/// `OceanMonumentDoubleXYRoom`: two cells along x, two high: `[west, east, west_up,
/// east_up]`.
pub(super) fn double_xy(p: &mut Placing<'_, '_, '_>, at: &Piece, cells: [Room; 4]) {
    let [west, east, west_up, east_up] = cells;
    if above_ground(west) {
        default_floor(p, at, 8, 0, open(east, Direction::Down));
        default_floor(p, at, 0, 0, open(west, Direction::Down));
    }
    if !west_up.has_up_connection {
        box_on_fill_only(p, at, [1, 8, 1], [7, 8, 6], BASE_GRAY);
    }
    if !east_up.has_up_connection {
        box_on_fill_only(p, at, [8, 8, 1], [14, 8, 6], BASE_GRAY);
    }
    for y in 1..=7 {
        let block = if y == 2 || y == 6 {
            BASE_GRAY
        } else {
            BASE_LIGHT
        };
        at.fill(p, [0, y, 0], [0, y, 7], block);
        at.fill(p, [15, y, 0], [15, y, 7], block);
        at.fill(p, [1, y, 0], [15, y, 0], block);
        at.fill(p, [1, y, 7], [14, y, 7], block);
    }
    for (from, to) in [
        ([2, 1, 3], [2, 7, 4]),
        ([3, 1, 2], [4, 7, 2]),
        ([3, 1, 5], [4, 7, 5]),
        ([13, 1, 3], [13, 7, 4]),
        ([11, 1, 2], [12, 7, 2]),
        ([11, 1, 5], [12, 7, 5]),
        ([5, 1, 3], [5, 3, 4]),
        ([10, 1, 3], [10, 3, 4]),
        ([5, 7, 2], [10, 7, 5]),
        ([5, 5, 2], [5, 7, 2]),
        ([10, 5, 2], [10, 7, 2]),
        ([5, 5, 5], [5, 7, 5]),
        ([10, 5, 5], [10, 7, 5]),
    ] {
        at.fill(p, from, to, BASE_LIGHT);
    }
    at.place_block(p, BASE_LIGHT, 6, 6, 2);
    at.place_block(p, BASE_LIGHT, 9, 6, 2);
    at.place_block(p, BASE_LIGHT, 6, 6, 5);
    at.place_block(p, BASE_LIGHT, 9, 6, 5);
    at.fill(p, [5, 4, 3], [6, 4, 4], BASE_LIGHT);
    at.fill(p, [9, 4, 3], [10, 4, 4], BASE_LIGHT);
    at.place_block(p, LAMP, 5, 4, 2);
    at.place_block(p, LAMP, 5, 4, 5);
    at.place_block(p, LAMP, 10, 4, 2);
    at.place_block(p, LAMP, 10, 4, 5);
    let (s, n, w, e) = (
        Direction::South,
        Direction::North,
        Direction::West,
        Direction::East,
    );
    side_openings(p, at, west, [0, 1, 0], &[s, n, w]);
    side_openings(p, at, east, [8, 1, 0], &[s, n, e]);
    side_openings(p, at, west_up, [0, 5, 0], &[s, n, w]);
    side_openings(p, at, east_up, [8, 5, 0], &[s, n, e]);
}

/// `OceanMonumentDoubleYRoom`: one cell, two high.
pub(super) fn double_y(p: &mut Placing<'_, '_, '_>, at: &Piece, room: Room, above: Room) {
    if above_ground(room) {
        default_floor(p, at, 0, 0, open(room, Direction::Down));
    }
    if !above.has_up_connection {
        box_on_fill_only(p, at, [1, 8, 1], [6, 8, 6], BASE_GRAY);
    }
    for (from, to) in [
        ([0, 4, 0], [0, 4, 7]),
        ([7, 4, 0], [7, 4, 7]),
        ([1, 4, 0], [6, 4, 0]),
        ([1, 4, 7], [6, 4, 7]),
        ([2, 4, 1], [2, 4, 2]),
        ([1, 4, 2], [1, 4, 2]),
        ([5, 4, 1], [5, 4, 2]),
        ([6, 4, 2], [6, 4, 2]),
        ([2, 4, 5], [2, 4, 6]),
        ([1, 4, 5], [1, 4, 5]),
        ([5, 4, 5], [5, 4, 6]),
        ([6, 4, 5], [6, 4, 5]),
    ] {
        at.fill(p, from, to, BASE_LIGHT);
    }
    for (cell, y) in [(room, 1), (above, 5)] {
        // Each side either opens between two columns under a lintel or is a wall
        // with a band of prismarine.
        if open(cell, Direction::South) {
            at.fill(p, [2, y, 0], [2, y + 2, 0], BASE_LIGHT);
            at.fill(p, [5, y, 0], [5, y + 2, 0], BASE_LIGHT);
            at.fill(p, [3, y + 2, 0], [4, y + 2, 0], BASE_LIGHT);
        } else {
            at.fill(p, [0, y, 0], [7, y + 2, 0], BASE_LIGHT);
            at.fill(p, [0, y + 1, 0], [7, y + 1, 0], BASE_GRAY);
        }
        if open(cell, Direction::North) {
            at.fill(p, [2, y, 7], [2, y + 2, 7], BASE_LIGHT);
            at.fill(p, [5, y, 7], [5, y + 2, 7], BASE_LIGHT);
            at.fill(p, [3, y + 2, 7], [4, y + 2, 7], BASE_LIGHT);
        } else {
            at.fill(p, [0, y, 7], [7, y + 2, 7], BASE_LIGHT);
            at.fill(p, [0, y + 1, 7], [7, y + 1, 7], BASE_GRAY);
        }
        if open(cell, Direction::West) {
            at.fill(p, [0, y, 2], [0, y + 2, 2], BASE_LIGHT);
            at.fill(p, [0, y, 5], [0, y + 2, 5], BASE_LIGHT);
            at.fill(p, [0, y + 2, 3], [0, y + 2, 4], BASE_LIGHT);
        } else {
            at.fill(p, [0, y, 0], [0, y + 2, 7], BASE_LIGHT);
            at.fill(p, [0, y + 1, 0], [0, y + 1, 7], BASE_GRAY);
        }
        if open(cell, Direction::East) {
            at.fill(p, [7, y, 2], [7, y + 2, 2], BASE_LIGHT);
            at.fill(p, [7, y, 5], [7, y + 2, 5], BASE_LIGHT);
            at.fill(p, [7, y + 2, 3], [7, y + 2, 4], BASE_LIGHT);
        } else {
            at.fill(p, [7, y, 0], [7, y + 2, 7], BASE_LIGHT);
            at.fill(p, [7, y + 1, 0], [7, y + 1, 7], BASE_GRAY);
        }
    }
}

/// `OceanMonumentDoubleYZRoom`: two cells along z, two high: `[south, north,
/// south_up, north_up]`.
pub(super) fn double_yz(p: &mut Placing<'_, '_, '_>, at: &Piece, cells: [Room; 4]) {
    let [south, north, south_up, north_up] = cells;
    if above_ground(south) {
        default_floor(p, at, 0, 8, open(north, Direction::Down));
        default_floor(p, at, 0, 0, open(south, Direction::Down));
    }
    if !south_up.has_up_connection {
        box_on_fill_only(p, at, [1, 8, 1], [6, 8, 7], BASE_GRAY);
    }
    if !north_up.has_up_connection {
        box_on_fill_only(p, at, [1, 8, 8], [6, 8, 14], BASE_GRAY);
    }
    for y in 1..=7 {
        let block = if y == 2 || y == 6 {
            BASE_GRAY
        } else {
            BASE_LIGHT
        };
        at.fill(p, [0, y, 0], [0, y, 15], block);
        at.fill(p, [7, y, 0], [7, y, 15], block);
        at.fill(p, [1, y, 0], [6, y, 0], block);
        at.fill(p, [1, y, 15], [6, y, 15], block);
    }
    for y in 1..=7 {
        let block = if y == 2 || y == 6 { LAMP } else { BASE_BLACK };
        at.fill(p, [3, y, 7], [4, y, 8], block);
    }
    let (s, n, w, e) = (
        Direction::South,
        Direction::North,
        Direction::West,
        Direction::East,
    );
    side_openings(p, at, south, [0, 1, 0], &[s, e, w]);
    side_openings(p, at, north, [0, 1, 8], &[n, w, e]);
    side_openings(p, at, south_up, [0, 5, 0], &[s]);
    if open(south_up, e) {
        water_box(p, at, [7, 5, 3], [7, 6, 4]);
        at.fill(p, [5, 4, 2], [6, 4, 5], BASE_LIGHT);
        at.fill(p, [6, 1, 2], [6, 3, 2], BASE_LIGHT);
        at.fill(p, [6, 1, 5], [6, 3, 5], BASE_LIGHT);
    }
    if open(south_up, w) {
        water_box(p, at, [0, 5, 3], [0, 6, 4]);
        at.fill(p, [1, 4, 2], [2, 4, 5], BASE_LIGHT);
        at.fill(p, [1, 1, 2], [1, 3, 2], BASE_LIGHT);
        at.fill(p, [1, 1, 5], [1, 3, 5], BASE_LIGHT);
    }
    side_openings(p, at, north_up, [0, 5, 8], &[n]);
    if open(north_up, w) {
        water_box(p, at, [0, 5, 11], [0, 6, 12]);
        at.fill(p, [1, 4, 10], [2, 4, 13], BASE_LIGHT);
        at.fill(p, [1, 1, 10], [1, 3, 10], BASE_LIGHT);
        at.fill(p, [1, 1, 13], [1, 3, 13], BASE_LIGHT);
    }
    if open(north_up, e) {
        water_box(p, at, [7, 5, 11], [7, 6, 12]);
        at.fill(p, [5, 4, 10], [6, 4, 13], BASE_LIGHT);
        at.fill(p, [6, 1, 10], [6, 3, 10], BASE_LIGHT);
        at.fill(p, [6, 1, 13], [6, 3, 13], BASE_LIGHT);
    }
}

/// `OceanMonumentDoubleZRoom`: two cells along z.
pub(super) fn double_z(p: &mut Placing<'_, '_, '_>, at: &Piece, south: Room, north: Room) {
    if above_ground(south) {
        default_floor(p, at, 0, 8, open(north, Direction::Down));
        default_floor(p, at, 0, 0, open(south, Direction::Down));
    }
    if !south.has_up_connection {
        box_on_fill_only(p, at, [1, 4, 1], [6, 4, 7], BASE_GRAY);
    }
    if !north.has_up_connection {
        box_on_fill_only(p, at, [1, 4, 8], [6, 4, 14], BASE_GRAY);
    }
    for (y, block) in [(3, BASE_LIGHT), (2, BASE_GRAY), (1, BASE_LIGHT)] {
        at.fill(p, [0, y, 0], [0, y, 15], block);
        at.fill(p, [7, y, 0], [7, y, 15], block);
        at.fill(p, [1, y, 0], [7, y, 0], block);
        at.fill(p, [1, y, 15], [6, y, 15], block);
    }
    for (from, to) in [
        ([1, 1, 1], [1, 1, 2]),
        ([6, 1, 1], [6, 1, 2]),
        ([1, 3, 1], [1, 3, 2]),
        ([6, 3, 1], [6, 3, 2]),
        ([1, 1, 13], [1, 1, 14]),
        ([6, 1, 13], [6, 1, 14]),
        ([1, 3, 13], [1, 3, 14]),
        ([6, 3, 13], [6, 3, 14]),
        ([2, 1, 6], [2, 3, 6]),
        ([5, 1, 6], [5, 3, 6]),
        ([2, 1, 9], [2, 3, 9]),
        ([5, 1, 9], [5, 3, 9]),
        ([3, 2, 6], [4, 2, 6]),
        ([3, 2, 9], [4, 2, 9]),
        ([2, 2, 7], [2, 2, 8]),
        ([5, 2, 7], [5, 2, 8]),
    ] {
        at.fill(p, from, to, BASE_LIGHT);
    }
    at.place_block(p, LAMP, 2, 2, 5);
    at.place_block(p, LAMP, 5, 2, 5);
    at.place_block(p, LAMP, 2, 2, 10);
    at.place_block(p, LAMP, 5, 2, 10);
    at.place_block(p, BASE_LIGHT, 2, 3, 5);
    at.place_block(p, BASE_LIGHT, 5, 3, 5);
    at.place_block(p, BASE_LIGHT, 2, 3, 10);
    at.place_block(p, BASE_LIGHT, 5, 3, 10);
    let (s, n, w, e) = (
        Direction::South,
        Direction::North,
        Direction::West,
        Direction::East,
    );
    side_openings(p, at, south, [0, 1, 0], &[s, e, w]);
    side_openings(p, at, north, [0, 1, 8], &[n, w, e]);
}

/// `OceanMonumentSimpleRoom`: one cell of one of three designs, and a pillar in its
/// middle where the room is closed above and below, has more than one opening and a
/// draw says so.
pub(super) fn simple(p: &mut Placing<'_, '_, '_>, at: &Piece, room: Room, design: i32) {
    if above_ground(room) {
        default_floor(p, at, 0, 0, open(room, Direction::Down));
    }
    if !room.has_up_connection {
        box_on_fill_only(p, at, [1, 4, 1], [6, 4, 6], BASE_GRAY);
    }
    let pillar = design != 0
        && p.random.next_bool()
        && !open(room, Direction::Down)
        && !open(room, Direction::Up)
        && room.count_openings() > 1;
    match design {
        0 => simple_design_0(p, at, room),
        1 => simple_design_1(p, at, room),
        2 => simple_design_2(p, at, room),
        _ => {}
    }
    if pillar {
        at.fill(p, [3, 1, 3], [4, 1, 4], BASE_LIGHT);
        at.fill(p, [3, 2, 3], [4, 2, 4], BASE_GRAY);
        at.fill(p, [3, 3, 3], [4, 3, 4], BASE_LIGHT);
    }
}

/// The first design: four low pillars in the corners with a lamp in each.
fn simple_design_0(p: &mut Placing<'_, '_, '_>, at: &Piece, room: Room) {
    for (x, z) in [(0, 0), (5, 0), (0, 5), (5, 5)] {
        at.fill(p, [x, 1, z], [x + 2, 1, z + 2], BASE_LIGHT);
        at.fill(p, [x, 3, z], [x + 2, 3, z + 2], BASE_LIGHT);
    }
    at.fill(p, [0, 2, 0], [0, 2, 2], BASE_GRAY);
    at.fill(p, [1, 2, 0], [2, 2, 0], BASE_GRAY);
    at.place_block(p, LAMP, 1, 2, 1);
    at.fill(p, [7, 2, 0], [7, 2, 2], BASE_GRAY);
    at.fill(p, [5, 2, 0], [6, 2, 0], BASE_GRAY);
    at.place_block(p, LAMP, 6, 2, 1);
    at.fill(p, [0, 2, 5], [0, 2, 7], BASE_GRAY);
    at.fill(p, [1, 2, 7], [2, 2, 7], BASE_GRAY);
    at.place_block(p, LAMP, 1, 2, 6);
    at.fill(p, [7, 2, 5], [7, 2, 7], BASE_GRAY);
    at.fill(p, [5, 2, 7], [6, 2, 7], BASE_GRAY);
    at.place_block(p, LAMP, 6, 2, 6);
    if open(room, Direction::South) {
        at.fill(p, [3, 3, 0], [4, 3, 0], BASE_LIGHT);
    } else {
        at.fill(p, [3, 3, 0], [4, 3, 1], BASE_LIGHT);
        at.fill(p, [3, 2, 0], [4, 2, 0], BASE_GRAY);
        at.fill(p, [3, 1, 0], [4, 1, 1], BASE_LIGHT);
    }
    if open(room, Direction::North) {
        at.fill(p, [3, 3, 7], [4, 3, 7], BASE_LIGHT);
    } else {
        at.fill(p, [3, 3, 6], [4, 3, 7], BASE_LIGHT);
        at.fill(p, [3, 2, 7], [4, 2, 7], BASE_GRAY);
        at.fill(p, [3, 1, 6], [4, 1, 7], BASE_LIGHT);
    }
    if open(room, Direction::West) {
        at.fill(p, [0, 3, 3], [0, 3, 4], BASE_LIGHT);
    } else {
        at.fill(p, [0, 3, 3], [1, 3, 4], BASE_LIGHT);
        at.fill(p, [0, 2, 3], [0, 2, 4], BASE_GRAY);
        at.fill(p, [0, 1, 3], [1, 1, 4], BASE_LIGHT);
    }
    if open(room, Direction::East) {
        at.fill(p, [7, 3, 3], [7, 3, 4], BASE_LIGHT);
    } else {
        at.fill(p, [6, 3, 3], [7, 3, 4], BASE_LIGHT);
        at.fill(p, [7, 2, 3], [7, 2, 4], BASE_GRAY);
        at.fill(p, [6, 1, 3], [7, 1, 4], BASE_LIGHT);
    }
}

/// The second design: four columns with lamps around the middle and walls on the
/// closed sides.
fn simple_design_1(p: &mut Placing<'_, '_, '_>, at: &Piece, room: Room) {
    for (x, z) in [(2, 2), (2, 5), (5, 5), (5, 2)] {
        at.fill(p, [x, 1, z], [x, 3, z], BASE_LIGHT);
        at.place_block(p, LAMP, x, 2, z);
    }
    for (from, to) in [
        ([0, 1, 0], [1, 3, 0]),
        ([0, 1, 1], [0, 3, 1]),
        ([0, 1, 7], [1, 3, 7]),
        ([0, 1, 6], [0, 3, 6]),
        ([6, 1, 7], [7, 3, 7]),
        ([7, 1, 6], [7, 3, 6]),
        ([6, 1, 0], [7, 3, 0]),
        ([7, 1, 1], [7, 3, 1]),
    ] {
        at.fill(p, from, to, BASE_LIGHT);
    }
    for (x, z) in [
        (1, 0),
        (0, 1),
        (1, 7),
        (0, 6),
        (6, 7),
        (7, 6),
        (6, 0),
        (7, 1),
    ] {
        at.place_block(p, BASE_GRAY, x, 2, z);
    }
    if !open(room, Direction::South) {
        at.fill(p, [1, 3, 0], [6, 3, 0], BASE_LIGHT);
        at.fill(p, [1, 2, 0], [6, 2, 0], BASE_GRAY);
        at.fill(p, [1, 1, 0], [6, 1, 0], BASE_LIGHT);
    }
    if !open(room, Direction::North) {
        at.fill(p, [1, 3, 7], [6, 3, 7], BASE_LIGHT);
        at.fill(p, [1, 2, 7], [6, 2, 7], BASE_GRAY);
        at.fill(p, [1, 1, 7], [6, 1, 7], BASE_LIGHT);
    }
    if !open(room, Direction::West) {
        at.fill(p, [0, 3, 1], [0, 3, 6], BASE_LIGHT);
        at.fill(p, [0, 2, 1], [0, 2, 6], BASE_GRAY);
        at.fill(p, [0, 1, 1], [0, 1, 6], BASE_LIGHT);
    }
    if !open(room, Direction::East) {
        at.fill(p, [7, 3, 1], [7, 3, 6], BASE_LIGHT);
        at.fill(p, [7, 2, 1], [7, 2, 6], BASE_GRAY);
        at.fill(p, [7, 1, 1], [7, 1, 6], BASE_LIGHT);
    }
}

/// The third design: walls banded with dark prismarine, open where the room opens.
fn simple_design_2(p: &mut Placing<'_, '_, '_>, at: &Piece, room: Room) {
    for (y, block) in [(1, BASE_LIGHT), (2, BASE_BLACK), (3, BASE_LIGHT)] {
        at.fill(p, [0, y, 0], [0, y, 7], block);
        at.fill(p, [7, y, 0], [7, y, 7], block);
        at.fill(p, [1, y, 0], [6, y, 0], block);
        at.fill(p, [1, y, 7], [6, y, 7], block);
    }
    at.fill(p, [0, 1, 3], [0, 2, 4], BASE_BLACK);
    at.fill(p, [7, 1, 3], [7, 2, 4], BASE_BLACK);
    at.fill(p, [3, 1, 0], [4, 2, 0], BASE_BLACK);
    at.fill(p, [3, 1, 7], [4, 2, 7], BASE_BLACK);
    let (s, n, w, e) = (
        Direction::South,
        Direction::North,
        Direction::West,
        Direction::East,
    );
    side_openings(p, at, room, [0, 1, 0], &[s, n, w, e]);
}

/// `OceanMonumentSimpleTopRoom`: the third design over a floor of wet sponge, two or
/// three high in the columns the draws choose.
pub(super) fn simple_top(p: &mut Placing<'_, '_, '_>, at: &Piece, room: Room) {
    if above_ground(room) {
        default_floor(p, at, 0, 0, open(room, Direction::Down));
    }
    if !room.has_up_connection {
        box_on_fill_only(p, at, [1, 4, 1], [6, 4, 6], BASE_GRAY);
    }
    for x in 1..=6 {
        for z in 1..=6 {
            if p.random.next_i32_bounded(3) != 0 {
                let y = 2 + i32::from(p.random.next_i32_bounded(4) != 0);
                at.fill(p, [x, y, z], [x, 3, z], blocks::WET_SPONGE);
            }
        }
    }
    simple_design_2(p, at, room);
}

/// `OceanMonumentWingRoom`: one of two designs, each with its elder guardian.
pub(super) fn wing(p: &mut Placing<'_, '_, '_>, at: &Piece, design: i32) {
    if design == 0 {
        for i in 0..4 {
            at.fill(p, [10 - i, 3 - i, 20 - i], [12 + i, 3 - i, 20], BASE_LIGHT);
        }
        at.fill(p, [7, 0, 6], [15, 0, 16], BASE_LIGHT);
        at.fill(p, [6, 0, 6], [6, 3, 20], BASE_LIGHT);
        at.fill(p, [16, 0, 6], [16, 3, 20], BASE_LIGHT);
        at.fill(p, [7, 1, 7], [7, 1, 20], BASE_LIGHT);
        at.fill(p, [15, 1, 7], [15, 1, 20], BASE_LIGHT);
        at.fill(p, [7, 1, 6], [9, 3, 6], BASE_LIGHT);
        at.fill(p, [13, 1, 6], [15, 3, 6], BASE_LIGHT);
        at.fill(p, [8, 1, 7], [9, 1, 7], BASE_LIGHT);
        at.fill(p, [13, 1, 7], [14, 1, 7], BASE_LIGHT);
        at.fill(p, [9, 0, 5], [13, 0, 5], BASE_LIGHT);
        at.fill(p, [10, 0, 7], [12, 0, 7], BASE_BLACK);
        at.fill(p, [8, 0, 10], [8, 0, 12], BASE_BLACK);
        at.fill(p, [14, 0, 10], [14, 0, 12], BASE_BLACK);
        for z in (7..=18).rev().step_by(3) {
            at.place_block(p, LAMP, 6, 3, z);
            at.place_block(p, LAMP, 16, 3, z);
        }
        at.place_block(p, LAMP, 10, 0, 10);
        at.place_block(p, LAMP, 12, 0, 10);
        at.place_block(p, LAMP, 10, 0, 12);
        at.place_block(p, LAMP, 12, 0, 12);
        at.place_block(p, LAMP, 8, 3, 6);
        at.place_block(p, LAMP, 14, 3, 6);
        for (x, z) in [(4, 4), (18, 4), (4, 18), (18, 18)] {
            at.place_block(p, BASE_LIGHT, x, 2, z);
            at.place_block(p, LAMP, x, 1, z);
            at.place_block(p, BASE_LIGHT, x, 0, z);
        }
        at.place_block(p, BASE_LIGHT, 9, 7, 20);
        at.place_block(p, BASE_LIGHT, 13, 7, 20);
        at.fill(p, [6, 0, 21], [7, 4, 21], BASE_LIGHT);
        at.fill(p, [15, 0, 21], [16, 4, 21], BASE_LIGHT);
        spawn_elder(p, at, 11, 2, 16);
    } else if design == 1 {
        at.fill(p, [9, 3, 18], [13, 3, 20], BASE_LIGHT);
        at.fill(p, [9, 0, 18], [9, 2, 18], BASE_LIGHT);
        at.fill(p, [13, 0, 18], [13, 2, 18], BASE_LIGHT);
        for x in [9, 13] {
            at.place_block(p, BASE_LIGHT, x, 6, 20);
            at.place_block(p, LAMP, x, 5, 20);
            at.place_block(p, BASE_LIGHT, x, 4, 20);
        }
        at.fill(p, [7, 3, 7], [15, 3, 14], BASE_LIGHT);
        for x in [10, 12] {
            at.fill(p, [x, 0, 10], [x, 6, 10], BASE_LIGHT);
            at.fill(p, [x, 0, 12], [x, 6, 12], BASE_LIGHT);
            at.place_block(p, LAMP, x, 0, 10);
            at.place_block(p, LAMP, x, 0, 12);
            at.place_block(p, LAMP, x, 4, 10);
            at.place_block(p, LAMP, x, 4, 12);
        }
        for x in [8, 14] {
            at.fill(p, [x, 0, 7], [x, 2, 7], BASE_LIGHT);
            at.fill(p, [x, 0, 14], [x, 2, 14], BASE_LIGHT);
        }
        at.fill(p, [8, 3, 8], [8, 3, 13], BASE_BLACK);
        at.fill(p, [14, 3, 8], [14, 3, 13], BASE_BLACK);
        spawn_elder(p, at, 11, 5, 13);
    }
}

/// `OceanMonumentPenthouse`: the room on the roof, with the third elder guardian.
pub(super) fn penthouse(p: &mut Placing<'_, '_, '_>, at: &Piece) {
    at.fill(p, [2, -1, 2], [11, -1, 11], BASE_LIGHT);
    at.fill(p, [0, -1, 0], [1, -1, 11], BASE_GRAY);
    at.fill(p, [12, -1, 0], [13, -1, 11], BASE_GRAY);
    at.fill(p, [2, -1, 0], [11, -1, 1], BASE_GRAY);
    at.fill(p, [2, -1, 12], [11, -1, 13], BASE_GRAY);
    at.fill(p, [0, 0, 0], [0, 0, 13], BASE_LIGHT);
    at.fill(p, [13, 0, 0], [13, 0, 13], BASE_LIGHT);
    at.fill(p, [1, 0, 0], [12, 0, 0], BASE_LIGHT);
    at.fill(p, [1, 0, 13], [12, 0, 13], BASE_LIGHT);
    for i in (2..=11).step_by(3) {
        at.place_block(p, LAMP, 0, 0, i);
        at.place_block(p, LAMP, 13, 0, i);
        at.place_block(p, LAMP, i, 0, 0);
    }
    at.fill(p, [2, 0, 3], [4, 0, 9], BASE_LIGHT);
    at.fill(p, [9, 0, 3], [11, 0, 9], BASE_LIGHT);
    at.fill(p, [4, 0, 9], [9, 0, 11], BASE_LIGHT);
    at.place_block(p, BASE_LIGHT, 5, 0, 8);
    at.place_block(p, BASE_LIGHT, 8, 0, 8);
    at.place_block(p, BASE_LIGHT, 10, 0, 10);
    at.place_block(p, BASE_LIGHT, 3, 0, 10);
    at.fill(p, [3, 0, 3], [3, 0, 7], BASE_BLACK);
    at.fill(p, [10, 0, 3], [10, 0, 7], BASE_BLACK);
    at.fill(p, [6, 0, 10], [7, 0, 10], BASE_BLACK);
    for x in [3, 10] {
        for z in (2..=8).step_by(3) {
            at.fill(p, [x, 0, z], [x, 2, z], BASE_LIGHT);
        }
    }
    at.fill(p, [5, 0, 10], [5, 2, 10], BASE_LIGHT);
    at.fill(p, [8, 0, 10], [8, 2, 10], BASE_LIGHT);
    at.fill(p, [6, -1, 7], [7, -1, 8], BASE_BLACK);
    water_box(p, at, [6, -1, 3], [7, -1, 4]);
    spawn_elder(p, at, 6, 1, 6);
}
