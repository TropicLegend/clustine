// Adapted from SteelMC's `steel-core/src/worldgen/structure/piece_placer/fortress.rs`
// (see NOTICE in this crate), and checked block for block against the pinned jar's
// `NetherFortressPieces` and its piece classes.

//! The pieces of the Nether fortress (`NetherFortressPieces`): bridges straight and
//! crossing, the bridges' dead ends, the rooms on the bridges (a crossing, stairs and
//! the blaze's throne), and the castle's: its entrance, its small corridors straight,
//! crossing and turning, its stairs, its balcony and its room of nether wart.
//!
//! Everything is of nether bricks and their fences and stairs, placed with the piece's
//! orientation; under a piece's floor, columns of bricks reach down through air and
//! liquid (`fillColumnDown`).
//!
//! Only the turning corridors' chests draw from the run's random source (their loot
//! seeds); the throne's spawner of blazes draws nothing. A bridge's dead end draws its
//! ragged shape from a random source of its own, seeded with the number its layout drew
//! (`selfSeed`), so every chunk it reaches draws the same.
//!
//! What a piece keeps between runs: whether the turning corridor's chest or the
//! throne's spawner stands (`isNeedingChest` cleared, `hasPlacedSpawner`), bit
//! [`ONE_PLACED`] of the piece's flags. Each is placed only by the chunk that holds its
//! position, so the bit changes nothing the game draws; it is kept as the game keeps it.

use clustine_data::{BlockState, blocks};
use clustine_noise::{LegacyRandom, Random};
use clustine_structures::fortress::FortressPieceData;
use clustine_structures::geometry::Direction;
use clustine_structures::piece::StructurePiece;
use clustine_world::generation::PieceState;

use super::Placing;
use super::code_piece::{CodePiece, Corner};
use super::scattered::replaceable_by_structures;

/// The piece's flag that its one chest or spawner stands.
pub(crate) const ONE_PLACED: u16 = 1;

const NETHER_BRIDGE_LOOT: &str = "minecraft:chests/nether_bridge";
const BLAZE: &str = "minecraft:blaze";

const BRICKS: BlockState = blocks::NETHER_BRICKS;

/// Places a fortress piece in the centre chunk; returns its state as it is to be kept.
pub(crate) fn place(
    placing: &mut Placing<'_, '_, '_>,
    piece: &StructurePiece,
    data: &FortressPieceData,
    state: PieceState,
) -> PieceState {
    let mut at = CodePiece::new(placing, piece.bounding_box, piece.orientation);
    let done = state.flags & ONE_PLACED != 0;
    let placed_now = match *data {
        FortressPieceData::BridgeCrossing => {
            bridge_crossing(&mut at);
            false
        }
        FortressPieceData::BridgeEndFiller { self_seed } => {
            bridge_end_filler(&mut at, self_seed);
            false
        }
        FortressPieceData::BridgeStraight => {
            bridge_straight(&mut at);
            false
        }
        FortressPieceData::CastleCorridorStairs => {
            castle_corridor_stairs(&mut at);
            false
        }
        FortressPieceData::CastleCorridorTBalcony => {
            castle_corridor_t_balcony(&mut at);
            false
        }
        FortressPieceData::CastleEntrance => {
            castle_entrance(&mut at);
            false
        }
        FortressPieceData::CastleSmallCorridorCrossing => {
            castle_small_corridor_crossing(&mut at);
            false
        }
        FortressPieceData::CastleSmallCorridor => {
            castle_small_corridor(&mut at);
            false
        }
        FortressPieceData::CastleSmallCorridorLeftTurn { is_needing_chest } => {
            castle_small_corridor_turn(&mut at, true, is_needing_chest && !done)
        }
        FortressPieceData::CastleSmallCorridorRightTurn { is_needing_chest } => {
            castle_small_corridor_turn(&mut at, false, is_needing_chest && !done)
        }
        FortressPieceData::CastleStalkRoom => {
            castle_stalk_room(&mut at);
            false
        }
        FortressPieceData::MonsterThrone { has_placed_spawner } => {
            monster_throne(&mut at, has_placed_spawner || done)
        }
        FortressPieceData::RoomCrossing => {
            room_crossing(&mut at);
            false
        }
        FortressPieceData::StairsRoom => {
            stairs_room(&mut at);
            false
        }
    };
    if placed_now {
        PieceState {
            flags: state.flags | ONE_PLACED,
            ..state
        }
    } else {
        state
    }
}

fn with(state: BlockState, name: &str, value: &str) -> BlockState {
    state
        .with_property(name, value)
        .unwrap_or_else(|| panic!("{name} = {value} is a state of the block"))
}

/// A fence of nether bricks joined on the sides named, as the game sets them on the
/// default state.
fn fence(sides: &[&str]) -> BlockState {
    sides
        .iter()
        .fold(blocks::NETHER_BRICK_FENCE, |state, side| {
            with(state, side, "true")
        })
}

/// A fence joined to the north and the south.
fn fence_ns() -> BlockState {
    fence(&["north", "south"])
}

/// A fence joined to the west and the east.
fn fence_we() -> BlockState {
    fence(&["west", "east"])
}

fn stairs(facing: Direction) -> BlockState {
    with(blocks::NETHER_BRICK_STAIRS, "facing", facing.as_str())
}

/// `generateBox` of one block throughout, keeping nothing: every box of the fortress
/// is so.
fn fill(at: &mut CodePiece<'_, '_, '_, '_>, from: Corner, to: Corner, state: BlockState) {
    at.generate_box(from, to, state, state, false);
}

/// `fillColumnDown`: `state` from the position down, through what structures may
/// replace, to two blocks above the bottom of the world. The game sets these blocks
/// straight, neither turned nor scheduled.
fn fill_column_down(at: &mut CodePiece<'_, '_, '_, '_>, state: BlockState, x: i32, y: i32, z: i32) {
    let mut pos = at.world_pos(x, y, z);
    if !at.inside(pos) {
        return;
    }
    let level = at.level();
    while replaceable_by_structures(level.block(pos)) && pos.y > level.min_y() + 1 {
        level.set_block(pos, state);
        pos = pos.offset(0, -1, 0);
    }
}

/// Columns of bricks under every block of a floor from (0, 0) to (`max_x`, `max_z`),
/// x outermost, as the game's loops run.
fn columns_under(at: &mut CodePiece<'_, '_, '_, '_>, max_x: i32, max_z: i32) {
    for x in 0..=max_x {
        for z in 0..=max_z {
            fill_column_down(at, BRICKS, x, -1, z);
        }
    }
}

/// `BridgeCrossing`: two bridges crossing, with their piers.
fn bridge_crossing(at: &mut CodePiece<'_, '_, '_, '_>) {
    fill(at, [7, 3, 0], [11, 4, 18], BRICKS);
    fill(at, [0, 3, 7], [18, 4, 11], BRICKS);
    fill(at, [8, 5, 0], [10, 7, 18], blocks::AIR);
    fill(at, [0, 5, 8], [18, 7, 10], blocks::AIR);
    fill(at, [7, 5, 0], [7, 5, 7], BRICKS);
    fill(at, [7, 5, 11], [7, 5, 18], BRICKS);
    fill(at, [11, 5, 0], [11, 5, 7], BRICKS);
    fill(at, [11, 5, 11], [11, 5, 18], BRICKS);
    fill(at, [0, 5, 7], [7, 5, 7], BRICKS);
    fill(at, [11, 5, 7], [18, 5, 7], BRICKS);
    fill(at, [0, 5, 11], [7, 5, 11], BRICKS);
    fill(at, [11, 5, 11], [18, 5, 11], BRICKS);
    fill(at, [7, 2, 0], [11, 2, 5], BRICKS);
    fill(at, [7, 2, 13], [11, 2, 18], BRICKS);
    fill(at, [7, 0, 0], [11, 1, 3], BRICKS);
    fill(at, [7, 0, 15], [11, 1, 18], BRICKS);
    for x in 7..=11 {
        for z in 0..=2 {
            fill_column_down(at, BRICKS, x, -1, z);
            fill_column_down(at, BRICKS, x, -1, 18 - z);
        }
    }
    fill(at, [0, 2, 7], [5, 2, 11], BRICKS);
    fill(at, [13, 2, 7], [18, 2, 11], BRICKS);
    fill(at, [0, 0, 7], [3, 1, 11], BRICKS);
    fill(at, [15, 0, 7], [18, 1, 11], BRICKS);
    for x in 0..=2 {
        for z in 7..=11 {
            fill_column_down(at, BRICKS, x, -1, z);
            fill_column_down(at, BRICKS, 18 - x, -1, z);
        }
    }
}

/// `BridgeEndFiller`: a bridge broken off, its ragged end drawn from its own random
/// source (`RandomSource.createThreadLocalInstance(selfSeed)`), the same in every
/// chunk.
fn bridge_end_filler(at: &mut CodePiece<'_, '_, '_, '_>, self_seed: i32) {
    let mut random = LegacyRandom::from_seed(i64::from(self_seed) as u64);
    for x in 0..=4 {
        for y in 3..=4 {
            let z = random.next_i32_bounded(8);
            fill(at, [x, y, 0], [x, y, z], BRICKS);
        }
    }
    let z = random.next_i32_bounded(8);
    fill(at, [0, 5, 0], [0, 5, z], BRICKS);
    let z = random.next_i32_bounded(8);
    fill(at, [4, 5, 0], [4, 5, z], BRICKS);
    for x in 0..=4 {
        let z = random.next_i32_bounded(5);
        fill(at, [x, 2, 0], [x, 2, z], BRICKS);
    }
    for x in 0..=4 {
        for y in 0..=1 {
            let z = random.next_i32_bounded(3);
            fill(at, [x, y, 0], [x, y, z], BRICKS);
        }
    }
}

/// `BridgeStraight`: a length of bridge with its piers at both ends.
fn bridge_straight(at: &mut CodePiece<'_, '_, '_, '_>) {
    fill(at, [0, 3, 0], [4, 4, 18], BRICKS);
    fill(at, [1, 5, 0], [3, 7, 18], blocks::AIR);
    fill(at, [0, 5, 0], [0, 5, 18], BRICKS);
    fill(at, [4, 5, 0], [4, 5, 18], BRICKS);
    fill(at, [0, 2, 0], [4, 2, 5], BRICKS);
    fill(at, [0, 2, 13], [4, 2, 18], BRICKS);
    fill(at, [0, 0, 0], [4, 1, 3], BRICKS);
    fill(at, [0, 0, 15], [4, 1, 18], BRICKS);
    for x in 0..=4 {
        for z in 0..=2 {
            fill_column_down(at, BRICKS, x, -1, z);
            fill_column_down(at, BRICKS, x, -1, 18 - z);
        }
    }
    let east = fence(&["north", "east", "south"]);
    let west = fence(&["north", "south", "west"]);
    fill(at, [0, 1, 1], [0, 4, 1], east);
    fill(at, [0, 3, 4], [0, 4, 4], east);
    fill(at, [0, 3, 14], [0, 4, 14], east);
    fill(at, [0, 1, 17], [0, 4, 17], east);
    fill(at, [4, 1, 1], [4, 4, 1], west);
    fill(at, [4, 3, 4], [4, 4, 4], west);
    fill(at, [4, 3, 14], [4, 4, 14], west);
    fill(at, [4, 1, 17], [4, 4, 17], west);
}

/// `CastleCorridorStairsPiece`: a corridor stepping down by one block in each of its
/// ten slices.
fn castle_corridor_stairs(at: &mut CodePiece<'_, '_, '_, '_>) {
    let south = stairs(Direction::South);
    let ns = fence_ns();
    for step in 0..=9 {
        let floor = (7 - step).max(1);
        let roof = (floor + 5).max(14 - step).min(13);
        fill(at, [0, 0, step], [4, floor, step], BRICKS);
        fill(at, [1, floor + 1, step], [3, roof - 1, step], blocks::AIR);
        if step <= 6 {
            at.place_block(south, 1, floor + 1, step);
            at.place_block(south, 2, floor + 1, step);
            at.place_block(south, 3, floor + 1, step);
        }
        fill(at, [0, roof, step], [4, roof, step], BRICKS);
        fill(at, [0, floor + 1, step], [0, roof - 1, step], BRICKS);
        fill(at, [4, floor + 1, step], [4, roof - 1, step], BRICKS);
        if step & 1 == 0 {
            fill(at, [0, floor + 2, step], [0, floor + 3, step], ns);
            fill(at, [4, floor + 2, step], [4, floor + 3, step], ns);
        }
        for x in 0..=4 {
            fill_column_down(at, BRICKS, x, -1, step);
        }
    }
}

/// `CastleCorridorTBalconyPiece`: a corridor's junction with a balcony out over the
/// lava.
fn castle_corridor_t_balcony(at: &mut CodePiece<'_, '_, '_, '_>) {
    let ns = fence_ns();
    let we = fence_we();
    fill(at, [0, 0, 0], [8, 1, 8], BRICKS);
    fill(at, [0, 2, 0], [8, 5, 8], blocks::AIR);
    fill(at, [0, 6, 0], [8, 6, 5], BRICKS);
    fill(at, [0, 2, 0], [2, 5, 0], BRICKS);
    fill(at, [6, 2, 0], [8, 5, 0], BRICKS);
    fill(at, [1, 3, 0], [1, 4, 0], we);
    fill(at, [7, 3, 0], [7, 4, 0], we);
    fill(at, [0, 2, 4], [8, 2, 8], BRICKS);
    fill(at, [1, 1, 4], [2, 2, 4], blocks::AIR);
    fill(at, [6, 1, 4], [7, 2, 4], blocks::AIR);
    fill(at, [1, 3, 8], [7, 3, 8], we);
    at.place_block(fence(&["east", "south"]), 0, 3, 8);
    at.place_block(fence(&["west", "south"]), 8, 3, 8);
    fill(at, [0, 3, 6], [0, 3, 7], ns);
    fill(at, [8, 3, 6], [8, 3, 7], ns);
    fill(at, [0, 3, 4], [0, 5, 5], BRICKS);
    fill(at, [8, 3, 4], [8, 5, 5], BRICKS);
    fill(at, [1, 3, 5], [2, 5, 5], BRICKS);
    fill(at, [6, 3, 5], [7, 5, 5], BRICKS);
    fill(at, [1, 4, 5], [1, 5, 5], we);
    fill(at, [7, 4, 5], [7, 5, 5], we);
    for z in 0..=5 {
        for x in 0..=8 {
            fill_column_down(at, BRICKS, x, -1, z);
        }
    }
}

/// The floor and the walls the castle's entrance and its room of nether wart share.
fn castle_hall(at: &mut CodePiece<'_, '_, '_, '_>) {
    fill(at, [0, 3, 0], [12, 4, 12], BRICKS);
    fill(at, [0, 5, 0], [12, 13, 12], blocks::AIR);
    fill(at, [0, 5, 0], [1, 12, 12], BRICKS);
    fill(at, [11, 5, 0], [12, 12, 12], BRICKS);
    fill(at, [2, 5, 11], [4, 12, 12], BRICKS);
    fill(at, [8, 5, 11], [10, 12, 12], BRICKS);
    fill(at, [5, 9, 11], [7, 12, 12], BRICKS);
    fill(at, [2, 5, 0], [4, 12, 1], BRICKS);
    fill(at, [8, 5, 0], [10, 12, 1], BRICKS);
    fill(at, [5, 9, 0], [7, 12, 1], BRICKS);
    fill(at, [2, 11, 2], [10, 12, 10], BRICKS);
}

/// The battlements along the top of the castle's halls and the fences of their
/// windows.
fn castle_battlements(at: &mut CodePiece<'_, '_, '_, '_>) {
    let we = fence_we();
    let ns = fence_ns();
    for i in (1..=11).step_by(2) {
        fill(at, [i, 10, 0], [i, 11, 0], we);
        fill(at, [i, 10, 12], [i, 11, 12], we);
        fill(at, [0, 10, i], [0, 11, i], ns);
        fill(at, [12, 10, i], [12, 11, i], ns);
        at.place_block(BRICKS, i, 13, 0);
        at.place_block(BRICKS, i, 13, 12);
        at.place_block(BRICKS, 0, 13, i);
        at.place_block(BRICKS, 12, 13, i);
        if i != 11 {
            at.place_block(we, i + 1, 13, 0);
            at.place_block(we, i + 1, 13, 12);
            at.place_block(ns, 0, 13, i + 1);
            at.place_block(ns, 12, 13, i + 1);
        }
    }
    at.place_block(fence(&["north", "east"]), 0, 13, 0);
    at.place_block(fence(&["south", "east"]), 0, 13, 12);
    at.place_block(fence(&["south", "west"]), 12, 13, 12);
    at.place_block(fence(&["north", "west"]), 12, 13, 0);
    let west = fence(&["north", "south", "west"]);
    let east = fence(&["north", "south", "east"]);
    for z in (3..=9).step_by(2) {
        fill(at, [1, 7, z], [1, 8, z], west);
        fill(at, [11, 7, z], [11, 8, z], east);
    }
}

/// The cross of bricks the castle's halls stand on, with its piers.
fn castle_foundation(at: &mut CodePiece<'_, '_, '_, '_>) {
    fill(at, [4, 2, 0], [8, 2, 12], BRICKS);
    fill(at, [0, 2, 4], [12, 2, 8], BRICKS);
    fill(at, [4, 0, 0], [8, 1, 3], BRICKS);
    fill(at, [4, 0, 9], [8, 1, 12], BRICKS);
    fill(at, [0, 0, 4], [3, 1, 8], BRICKS);
    fill(at, [9, 0, 4], [12, 1, 8], BRICKS);
    for x in 4..=8 {
        for z in 0..=2 {
            fill_column_down(at, BRICKS, x, -1, z);
            fill_column_down(at, BRICKS, x, -1, 12 - z);
        }
    }
    for x in 0..=2 {
        for z in 4..=8 {
            fill_column_down(at, BRICKS, x, -1, z);
            fill_column_down(at, BRICKS, 12 - x, -1, z);
        }
    }
}

/// `CastleEntrance`: the castle's hall, with a well of lava in its middle.
fn castle_entrance(at: &mut CodePiece<'_, '_, '_, '_>) {
    castle_hall(at);
    fill(at, [5, 8, 0], [7, 8, 0], blocks::NETHER_BRICK_FENCE);
    castle_battlements(at);
    castle_foundation(at);
    fill(at, [5, 5, 5], [7, 5, 7], BRICKS);
    fill(at, [6, 1, 6], [6, 4, 6], blocks::AIR);
    at.place_block(BRICKS, 6, 0, 6);
    // The game then schedules the lava's tick once more where it lies in the chunk,
    // which `placeBlock` has just scheduled. A chunk being generated keeps one tick of
    // a fluid at a place: `ProtoChunkTicks.schedule` adds a tick to its list only when
    // its set, hashed by `SavedTick.UNIQUE_TICK_HASH`, takes it. The second changes
    // nothing there, and the view a run writes into keeps every tick it is given, so it
    // is not made here.
    at.place_block(blocks::LAVA, 6, 5, 6);
}

/// `CastleSmallCorridorCrossingPiece`: four ways meeting.
fn castle_small_corridor_crossing(at: &mut CodePiece<'_, '_, '_, '_>) {
    fill(at, [0, 0, 0], [4, 1, 4], BRICKS);
    fill(at, [0, 2, 0], [4, 5, 4], blocks::AIR);
    fill(at, [0, 2, 0], [0, 5, 0], BRICKS);
    fill(at, [4, 2, 0], [4, 5, 0], BRICKS);
    fill(at, [0, 2, 4], [0, 5, 4], BRICKS);
    fill(at, [4, 2, 4], [4, 5, 4], BRICKS);
    fill(at, [0, 6, 0], [4, 6, 4], BRICKS);
    columns_under(at, 4, 4);
}

/// `CastleSmallCorridorPiece`: a corridor with windows on both sides.
fn castle_small_corridor(at: &mut CodePiece<'_, '_, '_, '_>) {
    let ns = fence_ns();
    fill(at, [0, 0, 0], [4, 1, 4], BRICKS);
    fill(at, [0, 2, 0], [4, 5, 4], blocks::AIR);
    fill(at, [0, 2, 0], [0, 5, 4], BRICKS);
    fill(at, [4, 2, 0], [4, 5, 4], BRICKS);
    fill(at, [0, 3, 1], [0, 4, 1], ns);
    fill(at, [0, 3, 3], [0, 4, 3], ns);
    fill(at, [4, 3, 1], [4, 4, 1], ns);
    fill(at, [4, 3, 3], [4, 4, 3], ns);
    fill(at, [0, 6, 0], [4, 6, 4], BRICKS);
    columns_under(at, 4, 4);
}

/// `CastleSmallCorridorLeftTurnPiece` and `...RightTurnPiece`: a corridor turning,
/// which may hold a chest. Returns whether the chest was placed now.
fn castle_small_corridor_turn(
    at: &mut CodePiece<'_, '_, '_, '_>,
    left: bool,
    needing_chest: bool,
) -> bool {
    let ns = fence_ns();
    let we = fence_we();
    fill(at, [0, 0, 0], [4, 1, 4], BRICKS);
    fill(at, [0, 2, 0], [4, 5, 4], blocks::AIR);
    // The walls, and where the chest stands, by the way the corridor turns.
    let chest_x = if left {
        fill(at, [4, 2, 0], [4, 5, 4], BRICKS);
        fill(at, [4, 3, 1], [4, 4, 1], ns);
        fill(at, [4, 3, 3], [4, 4, 3], ns);
        fill(at, [0, 2, 0], [0, 5, 0], BRICKS);
        fill(at, [0, 2, 4], [3, 5, 4], BRICKS);
        3
    } else {
        fill(at, [0, 2, 0], [0, 5, 4], BRICKS);
        fill(at, [0, 3, 1], [0, 4, 1], ns);
        fill(at, [0, 3, 3], [0, 4, 3], ns);
        fill(at, [4, 2, 0], [4, 5, 0], BRICKS);
        fill(at, [1, 2, 4], [4, 5, 4], BRICKS);
        1
    };
    fill(at, [1, 3, 4], [1, 4, 4], we);
    fill(at, [3, 3, 4], [3, 4, 4], we);
    let mut placed = false;
    if needing_chest && at.inside(at.world_pos(chest_x, 2, 3)) {
        placed = true;
        at.create_chest(chest_x, 2, 3, NETHER_BRIDGE_LOOT);
    }
    fill(at, [0, 6, 0], [4, 6, 4], BRICKS);
    columns_under(at, 4, 4);
    placed
}

/// `CastleStalkRoom`: the castle's hall with stairs up its middle and beds of nether
/// wart on soul sand either side.
fn castle_stalk_room(at: &mut CodePiece<'_, '_, '_, '_>) {
    castle_hall(at);
    castle_battlements(at);
    let north = stairs(Direction::North);
    for i in 0..=6 {
        let z = i + 4;
        for x in 5..=7 {
            at.place_block(north, x, 5 + i, z);
        }
        if (5..=8).contains(&z) {
            fill(at, [5, 5, z], [7, i + 4, z], BRICKS);
        } else if (9..=10).contains(&z) {
            fill(at, [5, 8, z], [7, i + 4, z], BRICKS);
        }
        if i >= 1 {
            fill(at, [5, 6 + i, z], [7, 9 + i, z], blocks::AIR);
        }
    }
    for x in 5..=7 {
        at.place_block(north, x, 12, 11);
    }
    fill(at, [5, 6, 7], [5, 7, 7], fence(&["north", "south", "east"]));
    fill(at, [7, 6, 7], [7, 7, 7], fence(&["north", "south", "west"]));
    fill(at, [5, 13, 12], [7, 13, 12], blocks::AIR);
    fill(at, [2, 5, 2], [3, 5, 3], BRICKS);
    fill(at, [2, 5, 9], [3, 5, 10], BRICKS);
    fill(at, [2, 5, 4], [2, 5, 8], BRICKS);
    fill(at, [9, 5, 2], [10, 5, 3], BRICKS);
    fill(at, [9, 5, 9], [10, 5, 10], BRICKS);
    fill(at, [10, 5, 4], [10, 5, 8], BRICKS);
    let east = stairs(Direction::East);
    let west = stairs(Direction::West);
    at.place_block(west, 4, 5, 2);
    at.place_block(west, 4, 5, 3);
    at.place_block(west, 4, 5, 9);
    at.place_block(west, 4, 5, 10);
    at.place_block(east, 8, 5, 2);
    at.place_block(east, 8, 5, 3);
    at.place_block(east, 8, 5, 9);
    at.place_block(east, 8, 5, 10);
    fill(at, [3, 4, 4], [4, 4, 8], blocks::SOUL_SAND);
    fill(at, [8, 4, 4], [9, 4, 8], blocks::SOUL_SAND);
    fill(at, [3, 5, 4], [4, 5, 8], blocks::NETHER_WART);
    fill(at, [8, 5, 4], [9, 5, 8], blocks::NETHER_WART);
    castle_foundation(at);
}

/// `MonsterThrone`: a raised platform with the blazes' spawner. Returns whether the
/// spawner was placed now.
fn monster_throne(at: &mut CodePiece<'_, '_, '_, '_>, placed_spawner: bool) -> bool {
    let we = fence_we();
    let ns = fence_ns();
    fill(at, [0, 2, 0], [6, 7, 7], blocks::AIR);
    fill(at, [1, 0, 0], [5, 1, 7], BRICKS);
    fill(at, [1, 2, 1], [5, 2, 7], BRICKS);
    fill(at, [1, 3, 2], [5, 3, 7], BRICKS);
    fill(at, [1, 4, 3], [5, 4, 7], BRICKS);
    fill(at, [1, 2, 0], [1, 4, 2], BRICKS);
    fill(at, [5, 2, 0], [5, 4, 2], BRICKS);
    fill(at, [1, 5, 2], [1, 5, 3], BRICKS);
    fill(at, [5, 5, 2], [5, 5, 3], BRICKS);
    fill(at, [0, 5, 3], [0, 5, 8], BRICKS);
    fill(at, [6, 5, 3], [6, 5, 8], BRICKS);
    fill(at, [1, 5, 8], [5, 5, 8], BRICKS);
    at.place_block(fence(&["west"]), 1, 6, 3);
    at.place_block(fence(&["east"]), 5, 6, 3);
    at.place_block(fence(&["east", "north"]), 0, 6, 3);
    at.place_block(fence(&["west", "north"]), 6, 6, 3);
    fill(at, [0, 6, 4], [0, 6, 7], ns);
    fill(at, [6, 6, 4], [6, 6, 7], ns);
    at.place_block(fence(&["east", "south"]), 0, 6, 8);
    at.place_block(fence(&["west", "south"]), 6, 6, 8);
    fill(at, [1, 6, 8], [5, 6, 8], we);
    at.place_block(fence(&["east"]), 1, 7, 8);
    fill(at, [2, 7, 8], [4, 7, 8], we);
    at.place_block(fence(&["west"]), 5, 7, 8);
    at.place_block(fence(&["east"]), 2, 8, 8);
    at.place_block(we, 3, 8, 8);
    at.place_block(fence(&["west"]), 4, 8, 8);
    let mut placed = false;
    if !placed_spawner {
        let pos = at.world_pos(3, 5, 5);
        if at.inside(pos) {
            placed = true;
            at.place_spawner(pos, BLAZE);
        }
    }
    columns_under(at, 6, 6);
    placed
}

/// `RoomCrossing`: a room on the bridges where three ways meet.
fn room_crossing(at: &mut CodePiece<'_, '_, '_, '_>) {
    let we = fence_we();
    let ns = fence_ns();
    fill(at, [0, 0, 0], [6, 1, 6], BRICKS);
    fill(at, [0, 2, 0], [6, 7, 6], blocks::AIR);
    fill(at, [0, 2, 0], [1, 6, 0], BRICKS);
    fill(at, [0, 2, 6], [1, 6, 6], BRICKS);
    fill(at, [5, 2, 0], [6, 6, 0], BRICKS);
    fill(at, [5, 2, 6], [6, 6, 6], BRICKS);
    fill(at, [0, 2, 0], [0, 6, 1], BRICKS);
    fill(at, [0, 2, 5], [0, 6, 6], BRICKS);
    fill(at, [6, 2, 0], [6, 6, 1], BRICKS);
    fill(at, [6, 2, 5], [6, 6, 6], BRICKS);
    fill(at, [2, 6, 0], [4, 6, 0], BRICKS);
    fill(at, [2, 5, 0], [4, 5, 0], we);
    fill(at, [2, 6, 6], [4, 6, 6], BRICKS);
    fill(at, [2, 5, 6], [4, 5, 6], we);
    fill(at, [0, 6, 2], [0, 6, 4], BRICKS);
    fill(at, [0, 5, 2], [0, 5, 4], ns);
    fill(at, [6, 6, 2], [6, 6, 4], BRICKS);
    fill(at, [6, 5, 2], [6, 5, 4], ns);
    columns_under(at, 6, 6);
}

/// `StairsRoom`: a room on the bridges with steps up to a way out on its right.
fn stairs_room(at: &mut CodePiece<'_, '_, '_, '_>) {
    let we = fence_we();
    let ns = fence_ns();
    fill(at, [0, 0, 0], [6, 1, 6], BRICKS);
    fill(at, [0, 2, 0], [6, 10, 6], blocks::AIR);
    fill(at, [0, 2, 0], [1, 8, 0], BRICKS);
    fill(at, [5, 2, 0], [6, 8, 0], BRICKS);
    fill(at, [0, 2, 1], [0, 8, 6], BRICKS);
    fill(at, [6, 2, 1], [6, 8, 6], BRICKS);
    fill(at, [1, 2, 6], [5, 8, 6], BRICKS);
    fill(at, [0, 3, 2], [0, 5, 4], ns);
    fill(at, [6, 3, 2], [6, 5, 2], ns);
    fill(at, [6, 3, 4], [6, 5, 4], ns);
    at.place_block(BRICKS, 5, 2, 5);
    fill(at, [4, 2, 5], [4, 3, 5], BRICKS);
    fill(at, [3, 2, 5], [3, 4, 5], BRICKS);
    fill(at, [2, 2, 5], [2, 5, 5], BRICKS);
    fill(at, [1, 2, 5], [1, 6, 5], BRICKS);
    fill(at, [1, 7, 1], [5, 7, 4], BRICKS);
    fill(at, [6, 8, 2], [6, 8, 4], blocks::AIR);
    fill(at, [2, 6, 0], [4, 8, 0], BRICKS);
    fill(at, [2, 5, 0], [4, 5, 0], we);
    columns_under(at, 6, 6);
}

#[cfg(test)]
mod tests {
    use clustine_noise::WorldgenRandom;
    use clustine_structures::geometry::{BoundingBox, IVec3};
    use clustine_structures::piece::{ProceduralPieceData, StructurePiecePayload};
    use clustine_world::{BlockPos, ChunkPos, Ticked};

    use super::*;
    use crate::blocks::same_block;
    use crate::structures::code_piece::tests::{chunk_box, piece};
    use crate::testing::{World, settings};

    const SEED: u64 = 7;
    /// The top of the toy world's netherrack; the pieces stand five above it, so that
    /// their columns reach four blocks down.
    const GROUND: i32 = 15;
    const FLOOR: i32 = GROUND + 5;

    /// A box wide enough for every piece whole, within the chunks the toy world lets a
    /// run write.
    fn whole() -> BoundingBox {
        BoundingBox::new(IVec3::new(-16, -63, -16), IVec3::new(31, 319, 31))
    }

    /// The piece of `data` facing `facing`, its corner at (`x`, [`FLOOR`], `z`), of the
    /// size (width, height, depth) its layout gives it (`orientBox`).
    fn fortress_piece(
        x: i32,
        z: i32,
        size: (i32, i32, i32),
        facing: Direction,
        data: FortressPieceData,
    ) -> StructurePiece {
        let (width, height, depth) = size;
        let (dx, dz) = match facing {
            Direction::East | Direction::West => (depth, width),
            _ => (width, depth),
        };
        piece(
            BoundingBox::new(
                IVec3::new(x, FLOOR, z),
                IVec3::new(x + dx - 1, FLOOR + height - 1, z + dz - 1),
            ),
            Some(facing),
            StructurePiecePayload::Procedural(ProceduralPieceData::NetherFortress(data)),
        )
    }

    /// Places `piece` with the chunk's box `clip` and a random source seeded with
    /// `seed`.
    fn run_with(
        world: &mut World,
        piece: &StructurePiece,
        state: PieceState,
        clip: BoundingBox,
        seed: u64,
    ) -> (PieceState, WorldgenRandom) {
        let StructurePiecePayload::Procedural(ProceduralPieceData::NetherFortress(data)) =
            &piece.payload
        else {
            unreachable!("the tests make fortress pieces");
        };
        let mut random = WorldgenRandom::from_seed(seed);
        let kept = world.with_level(|level| {
            let mut placing = Placing {
                level,
                random: &mut random,
                settings: settings(),
                chunk_box: clip,
                centre: ChunkPos { x: 0, z: 0 },
                pivot: IVec3::new(0, 0, 0),
            };
            place(&mut placing, piece, data, state)
        });
        (kept, random)
    }

    fn run(world: &mut World, piece: &StructurePiece, state: PieceState) -> PieceState {
        run_with(world, piece, state, whole(), SEED).0
    }

    /// Whether `random` has drawn exactly `longs` numbers of 64 bits since it was
    /// seeded with [`SEED`].
    fn drew_longs(random: &mut WorldgenRandom, longs: usize) -> bool {
        let mut expected = WorldgenRandom::from_seed(SEED);
        for _ in 0..longs {
            expected.next_i64();
        }
        expected.next_i64() == random.next_i64()
    }

    fn netherrack() -> World {
        World::flat(blocks::NETHERRACK, GROUND)
    }

    /// The places a run set a block at, each once.
    fn places_set(world: &World) -> usize {
        let mut places: Vec<BlockPos> = world.writes.iter().map(|(at, _)| *at).collect();
        places.sort_by_key(|at| (at.x, at.y, at.z));
        places.dedup();
        places.len()
    }

    fn turn(left: bool, is_needing_chest: bool) -> FortressPieceData {
        if left {
            FortressPieceData::CastleSmallCorridorLeftTurn { is_needing_chest }
        } else {
            FortressPieceData::CastleSmallCorridorRightTurn { is_needing_chest }
        }
    }

    #[test]
    fn each_piece_sets_as_many_places_as_the_game_and_draws_only_for_its_chest() {
        // The counts are of the places the game's own `postProcess` of each class sets
        // in a world of air over ground four blocks below the piece's floor, read by
        // running its bytecode; the turning corridors place their chest.
        let cases = [
            (FortressPieceData::BridgeCrossing, (19, 10, 19), 1225, 0),
            (
                FortressPieceData::BridgeEndFiller { self_seed: 12345 },
                (5, 10, 8),
                86,
                0,
            ),
            (FortressPieceData::BridgeStraight, (5, 10, 19), 659, 0),
            (FortressPieceData::CastleCorridorStairs, (5, 14, 10), 725, 0),
            (FortressPieceData::CastleCorridorTBalcony, (9, 7, 9), 756, 0),
            (FortressPieceData::CastleEntrance, (13, 14, 13), 2366, 0),
            (
                FortressPieceData::CastleSmallCorridorCrossing,
                (5, 7, 5),
                275,
                0,
            ),
            (turn(true, true), (5, 7, 5), 275, 1),
            (FortressPieceData::CastleSmallCorridor, (5, 7, 5), 275, 0),
            (turn(false, true), (5, 7, 5), 275, 1),
            (FortressPieceData::CastleStalkRoom, (13, 14, 13), 2373, 0),
            (
                FortressPieceData::MonsterThrone {
                    has_placed_spawner: false,
                },
                (7, 8, 9),
                634,
                0,
            ),
            (FortressPieceData::RoomCrossing, (7, 9, 7), 588, 0),
            (FortressPieceData::StairsRoom, (7, 11, 7), 735, 0),
        ];
        for (data, size, count, longs) in cases {
            let mut world = netherrack();
            let piece = fortress_piece(0, 0, size, Direction::North, data);
            let (_, mut random) =
                run_with(&mut world, &piece, PieceState::default(), whole(), SEED);
            assert_eq!(places_set(&world), count, "{data:?}");
            assert!(drew_longs(&mut random, longs), "{data:?}");
        }
    }

    #[test]
    fn a_straight_bridge_turned_east_turns_its_fences_and_marks_them() {
        let mut world = netherrack();
        let bridge = fortress_piece(
            0,
            0,
            (5, 10, 19),
            Direction::East,
            FortressPieceData::BridgeStraight,
        );
        run(&mut world, &bridge, PieceState::default());
        // The fence at (0, 1, 1), joined north, east and south, turned a quarter
        // clockwise: joined east, south and west.
        let at = BlockPos::new(1, FLOOR + 1, 0);
        let fence = world.get(at);
        assert!(same_block(fence, blocks::NETHER_BRICK_FENCE));
        let joined: Vec<_> = ["north", "east", "south", "west"]
            .into_iter()
            .filter(|side| fence.property(side) == Some("true"))
            .collect();
        assert_eq!(joined, ["east", "south", "west"]);
        assert!(world.marks.contains(&at));
        // The deck and the air above it.
        assert_eq!(
            world.get(BlockPos::new(9, FLOOR + 3, 2)),
            blocks::NETHER_BRICKS
        );
        assert_eq!(world.get(BlockPos::new(9, FLOOR + 6, 2)), blocks::AIR);
    }

    #[test]
    fn the_stalk_room_facing_south_mirrors_its_stairs_over_its_wart() {
        let mut world = netherrack();
        let room = fortress_piece(
            1,
            1,
            (13, 14, 13),
            Direction::South,
            FortressPieceData::CastleStalkRoom,
        );
        run(&mut world, &room, PieceState::default());
        // Facing south, the piece's frame is the world's mirrored north to south: the
        // middle stairs face north in the piece and south in the world.
        let stairs = world.get(BlockPos::new(6, FLOOR + 5, 5));
        assert!(same_block(stairs, blocks::NETHER_BRICK_STAIRS));
        assert_eq!(stairs.property("facing"), Some("south"));
        assert_eq!(world.get(BlockPos::new(4, FLOOR + 4, 5)), blocks::SOUL_SAND);
        assert_eq!(
            world.get(BlockPos::new(4, FLOOR + 5, 5)),
            blocks::NETHER_WART
        );
        assert_eq!(
            world.get(BlockPos::new(10, FLOOR + 5, 9)),
            blocks::NETHER_WART
        );
        // The flank stairs face west in the piece, which the mirror keeps.
        let west = world.get(BlockPos::new(5, FLOOR + 5, 3));
        assert_eq!(west.property("facing"), Some("west"));
    }

    #[test]
    fn the_castle_entrance_holds_lava_over_a_shaft_and_schedules_it_once() {
        let mut world = netherrack();
        let entrance = fortress_piece(
            0,
            0,
            (13, 14, 13),
            Direction::North,
            FortressPieceData::CastleEntrance,
        );
        run(&mut world, &entrance, PieceState::default());
        let lava = BlockPos::new(6, FLOOR + 5, 6);
        assert_eq!(world.get(lava), blocks::LAVA);
        for y in 1..=4 {
            assert_eq!(world.get(BlockPos::new(6, FLOOR + y, 6)), blocks::AIR);
        }
        assert_eq!(world.get(BlockPos::new(6, FLOOR, 6)), blocks::NETHER_BRICKS);
        assert_eq!(world.ticks.len(), 1);
        assert_eq!(world.ticks[0].0, lava);
        assert!(matches!(world.ticks[0].1.of, Ticked::Fluid(_)));
    }

    #[test]
    fn the_entrance_s_lava_gets_one_tick_in_its_chunk_and_none_from_another() {
        // `ProtoChunkTicks.schedule` keeps one tick of a fluid at a place, so the
        // game's second scheduling of the lava leaves one; where the lava lies in
        // another chunk, neither `placeBlock` nor the second scheduling makes a tick.
        let entrance = |x| {
            fortress_piece(
                x,
                0,
                (13, 14, 13),
                Direction::North,
                FortressPieceData::CastleEntrance,
            )
        };
        let mut world = netherrack();
        run_with(
            &mut world,
            &entrance(0),
            PieceState::default(),
            chunk_box(),
            SEED,
        );
        let lava = BlockPos::new(6, FLOOR + 5, 6);
        let fluid_ticks: Vec<_> = world
            .ticks
            .iter()
            .filter(|(_, tick)| matches!(tick.of, Ticked::Fluid(_)))
            .map(|(at, _)| *at)
            .collect();
        assert_eq!(fluid_ticks, [lava]);

        // Its corner at x = 10, the lava stands at x = 16, in the chunk to the east.
        let mut world = netherrack();
        run_with(
            &mut world,
            &entrance(10),
            PieceState::default(),
            chunk_box(),
            SEED,
        );
        assert!(!world.writes.is_empty());
        assert!(world.ticks.is_empty());
    }

    #[test]
    fn the_throne_places_its_blaze_spawner_once() {
        let mut world = netherrack();
        let throne = fortress_piece(
            0,
            0,
            (7, 8, 9),
            Direction::North,
            FortressPieceData::MonsterThrone {
                has_placed_spawner: false,
            },
        );
        let (kept, mut random) =
            run_with(&mut world, &throne, PieceState::default(), whole(), SEED);
        // Facing north, the piece's z runs from the box's far side.
        let spawner = BlockPos::new(3, FLOOR + 5, 8 - 5);
        assert_eq!(world.get(spawner), blocks::SPAWNER);
        assert_eq!(kept.flags, ONE_PLACED);
        assert!(drew_longs(&mut random, 0));

        let mut again = netherrack();
        assert_eq!(run(&mut again, &throne, kept), kept);
        assert_eq!(again.get(spawner), blocks::AIR);
    }

    #[test]
    fn a_turning_corridor_s_chest_stands_by_its_solid_wall_and_is_placed_once() {
        for (left, x) in [(true, 3), (false, 1)] {
            let mut world = netherrack();
            let corridor = fortress_piece(2, 2, (5, 7, 5), Direction::North, turn(left, true));
            let (kept, mut random) =
                run_with(&mut world, &corridor, PieceState::default(), whole(), SEED);
            let at = BlockPos::new(2 + x, FLOOR + 2, 2 + 4 - 3);
            assert!(same_block(world.get(at), blocks::CHEST), "left {left}");
            assert_eq!(kept.flags, ONE_PLACED);
            assert!(drew_longs(&mut random, 1));

            let mut again = netherrack();
            let (_, mut random) = run_with(&mut again, &corridor, kept, whole(), SEED);
            assert_eq!(again.get(at), blocks::AIR);
            assert!(drew_longs(&mut random, 0));
        }
    }

    #[test]
    fn a_corridor_without_a_chest_or_whose_chest_is_in_another_chunk_draws_nothing() {
        let plain = fortress_piece(2, 2, (5, 7, 5), Direction::North, turn(true, false));
        let mut world = netherrack();
        let (kept, mut random) = run_with(&mut world, &plain, PieceState::default(), whole(), SEED);
        assert_eq!(kept.flags, 0);
        assert!(drew_longs(&mut random, 0));

        // The chest's place, (16, y, 3), lies in the chunk to the east.
        let across = fortress_piece(13, 2, (5, 7, 5), Direction::North, turn(true, true));
        let mut world = netherrack();
        let (kept, mut random) = run_with(
            &mut world,
            &across,
            PieceState::default(),
            chunk_box(),
            SEED,
        );
        assert_eq!(kept.flags, 0);
        assert!(drew_longs(&mut random, 0));
        assert!(!world.writes.is_empty());
        assert!(world.writes.iter().all(|(at, _)| at.x <= 15));
    }

    #[test]
    fn a_bridge_s_end_is_drawn_from_its_own_seed_whatever_the_run_s() {
        let end = fortress_piece(
            0,
            0,
            (5, 10, 8),
            Direction::North,
            FortressPieceData::BridgeEndFiller { self_seed: 12345 },
        );
        let mut first = netherrack();
        let (_, mut random) = run_with(&mut first, &end, PieceState::default(), whole(), SEED);
        assert!(drew_longs(&mut random, 0));
        let mut second = netherrack();
        run_with(&mut second, &end, PieceState::default(), whole(), 99);
        assert_eq!(first.writes, second.writes);
        // With 12345 the game's draws run the deck's upper row at x = 1 the whole
        // length and at x = 2 one block only, the rail at x = 4 the whole length and
        // the one at x = 0 three blocks.
        let world_z = |z: i32| 7 - z;
        let bricks = |y: i32, x: i32, z: i32| {
            first.get(BlockPos::new(x, FLOOR + y, world_z(z))) == blocks::NETHER_BRICKS
        };
        assert!(bricks(4, 1, 7));
        assert!(bricks(4, 2, 0));
        assert!(!bricks(4, 2, 1));
        assert!(bricks(5, 4, 7));
        assert!(bricks(5, 0, 2));
        assert!(!bricks(5, 0, 3));
    }

    #[test]
    fn corridor_stairs_step_down_from_a_floor_of_seven_to_one() {
        let mut world = netherrack();
        let stairs = fortress_piece(
            0,
            0,
            (5, 14, 10),
            Direction::North,
            FortressPieceData::CastleCorridorStairs,
        );
        run(&mut world, &stairs, PieceState::default());
        let at = |y: i32, z: i32| world.get(BlockPos::new(2, FLOOR + y, 9 - z));
        // The first slice: floor at seven, a step on it, the roof at thirteen.
        let step = at(8, 0);
        assert!(same_block(step, blocks::NETHER_BRICK_STAIRS));
        assert_eq!(step.property("facing"), Some("south"));
        assert_eq!(at(13, 0), blocks::NETHER_BRICKS);
        // The last: floor at one, roof at six, air between.
        assert_eq!(at(1, 9), blocks::NETHER_BRICKS);
        for y in 2..=5 {
            assert_eq!(at(y, 9), blocks::AIR);
        }
        assert_eq!(at(6, 9), blocks::NETHER_BRICKS);
    }

    #[test]
    fn columns_reach_down_through_air_and_lava_to_the_ground() {
        let mut world = netherrack();
        world.set(BlockPos::new(0, FLOOR - 2, 0), blocks::LAVA);
        let room = fortress_piece(
            0,
            0,
            (7, 9, 7),
            Direction::North,
            FortressPieceData::RoomCrossing,
        );
        run(&mut world, &room, PieceState::default());
        for y in GROUND + 1..FLOOR {
            assert_eq!(world.get(BlockPos::new(0, y, 0)), blocks::NETHER_BRICKS);
        }
        assert!(
            world
                .writes
                .iter()
                .all(|(at, _)| at.y > GROUND && (0..=6).contains(&at.x))
        );
    }
}
