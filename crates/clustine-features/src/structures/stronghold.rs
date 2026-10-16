// Adapted from SteelMC's `steel-core/src/worldgen/structure/piece_placer/stronghold.rs`
// (see NOTICE in this crate), which follows the game's `StrongholdPieces`.

//! The pieces of strongholds (`StrongholdPieces`): straight corridors, turns, the
//! prison, the room crossings (a pillar with torches, a fountain, or a room with an
//! upper floor and a chest), stairs straight and winding, the five-way crossing, the
//! corridor with a chest, the library of one storey or two, the portal room and the
//! filler corridors.
//!
//! Most walls are of stone bricks chosen by `SmoothStoneSelector`: one draw for each
//! block of a box's faces, and none for the cave air inside. The door of a piece's
//! entrance is the kind its layout drew.
//!
//! What a piece keeps between runs: whether the corridor's chest or the portal room's
//! spawner stands (`hasPlacedChest`, `hasPlacedSpawner`), bit [`ONE_PLACED`] of the
//! piece's flags. Each is placed only by the chunk that holds its position, so the bit
//! changes nothing the game draws; it is kept as the game keeps it.
//!
//! The portal room draws for each of its twelve frame blocks whether it holds an eye,
//! and fills the portal where all twelve do.

use clustine_data::{BlockState, blocks};
use clustine_noise::{Random, WorldgenRandom};
use clustine_structures::geometry::Direction;
use clustine_structures::piece::StructurePiece;
use clustine_structures::stronghold::{StrongholdPieceData, StrongholdSmallDoorType};
use clustine_world::generation::PieceState;

use super::Placing;
use super::code_piece::{CodePiece, Corner};

/// The piece's flag that its one chest or spawner stands.
pub(crate) const ONE_PLACED: u16 = 1;

const CORRIDOR_LOOT: &str = "minecraft:chests/stronghold_corridor";
const CROSSING_LOOT: &str = "minecraft:chests/stronghold_crossing";
const LIBRARY_LOOT: &str = "minecraft:chests/stronghold_library";
const SILVERFISH: &str = "minecraft:silverfish";

/// Places a stronghold piece in the centre chunk; returns its state as it is to be
/// kept.
pub(crate) fn place(
    placing: &mut Placing<'_, '_, '_>,
    piece: &StructurePiece,
    data: &StrongholdPieceData,
    state: PieceState,
) -> PieceState {
    let mut at = CodePiece::new(placing, piece.bounding_box, piece.orientation);
    let done = state.flags & ONE_PLACED != 0;
    let placed_now = match *data {
        StrongholdPieceData::Straight {
            entry_door,
            left_child,
            right_child,
        } => {
            straight(&mut at, entry_door, left_child, right_child);
            false
        }
        StrongholdPieceData::PrisonHall { entry_door } => {
            prison_hall(&mut at, entry_door);
            false
        }
        StrongholdPieceData::LeftTurn { entry_door } => {
            turn(&mut at, entry_door, piece.orientation, true);
            false
        }
        StrongholdPieceData::RightTurn { entry_door } => {
            turn(&mut at, entry_door, piece.orientation, false);
            false
        }
        StrongholdPieceData::RoomCrossing {
            entry_door,
            crossing_type,
        } => {
            room_crossing(&mut at, entry_door, crossing_type);
            false
        }
        StrongholdPieceData::StraightStairsDown { entry_door } => {
            straight_stairs_down(&mut at, entry_door);
            false
        }
        StrongholdPieceData::StairsDown { entry_door, .. } => {
            stairs_down(&mut at, entry_door);
            false
        }
        StrongholdPieceData::FiveCrossing {
            entry_door,
            left_low,
            left_high,
            right_low,
            right_high,
        } => {
            five_crossing(
                &mut at,
                entry_door,
                [left_low, left_high, right_low, right_high],
            );
            false
        }
        StrongholdPieceData::ChestCorridor {
            entry_door,
            has_placed_chest,
        } => chest_corridor(&mut at, entry_door, has_placed_chest || done),
        StrongholdPieceData::Library {
            entry_door,
            is_tall,
        } => {
            library(&mut at, entry_door, is_tall);
            false
        }
        StrongholdPieceData::PortalRoom { has_placed_spawner } => {
            portal_room(&mut at, has_placed_spawner || done)
        }
        StrongholdPieceData::FillerCorridor { steps } => {
            filler_corridor(&mut at, steps);
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

/// `SmoothStoneSelector`: the faces of stone bricks, a fifth cracked, three tenths
/// mossy and a twentieth infested; cave air inside, without a draw.
fn smooth_stone(random: &mut WorldgenRandom, on_edge: bool) -> BlockState {
    if !on_edge {
        return blocks::CAVE_AIR;
    }
    let selection = random.next_f32();
    if selection < 0.2 {
        blocks::CRACKED_STONE_BRICKS
    } else if selection < 0.5 {
        blocks::MOSSY_STONE_BRICKS
    } else if selection < 0.55 {
        blocks::INFESTED_STONE_BRICKS
    } else {
        blocks::STONE_BRICKS
    }
}

/// `generateBox` with the stone selector.
fn stone_box(at: &mut CodePiece<'_, '_, '_, '_>, from: Corner, to: Corner, skip_air: bool) {
    at.generate_box_selected(from, to, skip_air, smooth_stone);
}

fn fill(at: &mut CodePiece<'_, '_, '_, '_>, from: Corner, to: Corner, state: BlockState) {
    at.generate_box(from, to, state, state, false);
}

fn air(at: &mut CodePiece<'_, '_, '_, '_>, from: Corner, to: Corner) {
    fill(at, from, to, blocks::CAVE_AIR);
}

fn with(state: BlockState, name: &str, value: &str) -> BlockState {
    state
        .with_property(name, value)
        .unwrap_or_else(|| panic!("{name} = {value} is a state of the block"))
}

fn facing(state: BlockState, direction: Direction) -> BlockState {
    with(state, "facing", direction.as_str())
}

/// Bars or a fence joined on the sides named.
fn joined(state: BlockState, sides: &[&str]) -> BlockState {
    sides
        .iter()
        .fold(state, |state, side| with(state, side, "true"))
}

fn upper(door: BlockState) -> BlockState {
    with(door, "half", "upper")
}

/// `StrongholdPiece.generateSmallDoor`.
fn small_door(
    at: &mut CodePiece<'_, '_, '_, '_>,
    door: StrongholdSmallDoorType,
    x: i32,
    y: i32,
    z: i32,
) {
    let frame = |at: &mut CodePiece<'_, '_, '_, '_>| {
        let bricks = blocks::STONE_BRICKS;
        at.place_block(bricks, x, y, z);
        at.place_block(bricks, x, y + 1, z);
        at.place_block(bricks, x, y + 2, z);
        at.place_block(bricks, x + 1, y + 2, z);
        at.place_block(bricks, x + 2, y + 2, z);
        at.place_block(bricks, x + 2, y + 1, z);
        at.place_block(bricks, x + 2, y, z);
    };
    match door {
        StrongholdSmallDoorType::Opening => air(at, [x, y, z], [x + 2, y + 2, z]),
        StrongholdSmallDoorType::WoodDoor => {
            frame(at);
            at.place_block(blocks::OAK_DOOR, x + 1, y, z);
            at.place_block(upper(blocks::OAK_DOOR), x + 1, y + 1, z);
        }
        StrongholdSmallDoorType::Grates => {
            at.place_block(blocks::CAVE_AIR, x + 1, y, z);
            at.place_block(blocks::CAVE_AIR, x + 1, y + 1, z);
            let west = joined(blocks::IRON_BARS, &["west"]);
            at.place_block(west, x, y, z);
            at.place_block(west, x, y + 1, z);
            let across = joined(blocks::IRON_BARS, &["east", "west"]);
            at.place_block(across, x, y + 2, z);
            at.place_block(across, x + 1, y + 2, z);
            at.place_block(across, x + 2, y + 2, z);
            let east = joined(blocks::IRON_BARS, &["east"]);
            at.place_block(east, x + 2, y + 1, z);
            at.place_block(east, x + 2, y, z);
        }
        StrongholdSmallDoorType::IronDoor => {
            frame(at);
            at.place_block(blocks::IRON_DOOR, x + 1, y, z);
            at.place_block(upper(blocks::IRON_DOOR), x + 1, y + 1, z);
            let button = with(blocks::STONE_BUTTON, "face", "wall");
            at.place_block(facing(button, Direction::North), x + 2, y + 1, z + 1);
            at.place_block(facing(button, Direction::South), x + 2, y + 1, z - 1);
        }
    }
}

/// `StraightCorridor`.
fn straight(
    at: &mut CodePiece<'_, '_, '_, '_>,
    door: StrongholdSmallDoorType,
    left: bool,
    right: bool,
) {
    stone_box(at, [0, 0, 0], [4, 4, 6], true);
    small_door(at, door, 1, 1, 0);
    small_door(at, StrongholdSmallDoorType::Opening, 1, 1, 6);
    let east = facing(blocks::WALL_TORCH, Direction::East);
    let west = facing(blocks::WALL_TORCH, Direction::West);
    at.maybe_generate_block(0.1, 1, 2, 1, east);
    at.maybe_generate_block(0.1, 3, 2, 1, west);
    at.maybe_generate_block(0.1, 1, 2, 5, east);
    at.maybe_generate_block(0.1, 3, 2, 5, west);
    if left {
        air(at, [0, 1, 2], [0, 3, 4]);
    }
    if right {
        air(at, [4, 1, 2], [4, 3, 4]);
    }
}

/// `PrisonHall`.
fn prison_hall(at: &mut CodePiece<'_, '_, '_, '_>, door: StrongholdSmallDoorType) {
    stone_box(at, [0, 0, 0], [8, 4, 10], true);
    small_door(at, door, 1, 1, 0);
    air(at, [1, 1, 10], [3, 3, 10]);
    stone_box(at, [4, 1, 1], [4, 3, 1], false);
    stone_box(at, [4, 1, 3], [4, 3, 3], false);
    stone_box(at, [4, 1, 7], [4, 3, 7], false);
    stone_box(at, [4, 1, 9], [4, 3, 9], false);
    let along = joined(blocks::IRON_BARS, &["north", "south"]);
    let along_east = joined(along, &["east"]);
    let across = joined(blocks::IRON_BARS, &["west", "east"]);
    for y in 1..=3 {
        at.place_block(along, 4, y, 4);
        at.place_block(along_east, 4, y, 5);
        at.place_block(along, 4, y, 6);
        at.place_block(across, 5, y, 5);
        at.place_block(across, 6, y, 5);
        at.place_block(across, 7, y, 5);
    }
    at.place_block(along, 4, 3, 2);
    at.place_block(along, 4, 3, 8);
    let bottom = facing(blocks::IRON_DOOR, Direction::West);
    let top = upper(bottom);
    at.place_block(bottom, 4, 1, 2);
    at.place_block(top, 4, 2, 2);
    at.place_block(bottom, 4, 1, 8);
    at.place_block(top, 4, 2, 8);
}

/// `LeftTurn` and `RightTurn`: the opening on the left or the right, which the game
/// takes from the orientation.
fn turn(
    at: &mut CodePiece<'_, '_, '_, '_>,
    door: StrongholdSmallDoorType,
    orientation: Option<Direction>,
    left: bool,
) {
    stone_box(at, [0, 0, 0], [4, 4, 4], true);
    small_door(at, door, 1, 1, 0);
    let north_or_east = matches!(orientation, Some(Direction::North | Direction::East));
    if north_or_east == left {
        air(at, [0, 1, 1], [0, 3, 3]);
    } else {
        air(at, [4, 1, 1], [4, 3, 3]);
    }
}

/// `RoomCrossing`: a pillar with four torches, a fountain, or a room with an upper
/// floor, a ladder and a chest.
fn room_crossing(at: &mut CodePiece<'_, '_, '_, '_>, door: StrongholdSmallDoorType, kind: i32) {
    stone_box(at, [0, 0, 0], [10, 6, 10], true);
    small_door(at, door, 4, 1, 0);
    air(at, [4, 1, 10], [6, 3, 10]);
    air(at, [0, 1, 4], [0, 3, 6]);
    air(at, [10, 1, 4], [10, 3, 6]);
    let bricks = blocks::STONE_BRICKS;
    match kind {
        0 => {
            at.place_block(bricks, 5, 1, 5);
            at.place_block(bricks, 5, 2, 5);
            at.place_block(bricks, 5, 3, 5);
            at.place_block(facing(blocks::WALL_TORCH, Direction::West), 4, 3, 5);
            at.place_block(facing(blocks::WALL_TORCH, Direction::East), 6, 3, 5);
            at.place_block(facing(blocks::WALL_TORCH, Direction::South), 5, 3, 4);
            at.place_block(facing(blocks::WALL_TORCH, Direction::North), 5, 3, 6);
            let slab = blocks::SMOOTH_STONE_SLAB;
            for (x, z) in [
                (4, 4),
                (4, 5),
                (4, 6),
                (6, 4),
                (6, 5),
                (6, 6),
                (5, 4),
                (5, 6),
            ] {
                at.place_block(slab, x, 1, z);
            }
        }
        1 => {
            for i in 0..5 {
                at.place_block(bricks, 3, 1, 3 + i);
                at.place_block(bricks, 7, 1, 3 + i);
                at.place_block(bricks, 3 + i, 1, 3);
                at.place_block(bricks, 3 + i, 1, 7);
            }
            at.place_block(bricks, 5, 1, 5);
            at.place_block(bricks, 5, 2, 5);
            at.place_block(bricks, 5, 3, 5);
            at.place_block(blocks::WATER, 5, 4, 5);
        }
        2 => {
            let cobble = blocks::COBBLESTONE;
            for z in 1..=9 {
                at.place_block(cobble, 1, 3, z);
                at.place_block(cobble, 9, 3, z);
            }
            for x in 1..=9 {
                at.place_block(cobble, x, 3, 1);
                at.place_block(cobble, x, 3, 9);
            }
            for (x, y, z) in [
                (5, 1, 4),
                (5, 1, 6),
                (5, 3, 4),
                (5, 3, 6),
                (4, 1, 5),
                (6, 1, 5),
                (4, 3, 5),
                (6, 3, 5),
            ] {
                at.place_block(cobble, x, y, z);
            }
            for y in 1..=3 {
                at.place_block(cobble, 4, y, 4);
                at.place_block(cobble, 6, y, 4);
                at.place_block(cobble, 4, y, 6);
                at.place_block(cobble, 6, y, 6);
            }
            at.place_block(blocks::WALL_TORCH, 5, 3, 5);
            let planks = blocks::OAK_PLANKS;
            for z in 2..=8 {
                at.place_block(planks, 2, 3, z);
                at.place_block(planks, 3, 3, z);
                if z <= 3 || z >= 7 {
                    at.place_block(planks, 4, 3, z);
                    at.place_block(planks, 5, 3, z);
                    at.place_block(planks, 6, 3, z);
                }
                at.place_block(planks, 7, 3, z);
                at.place_block(planks, 8, 3, z);
            }
            let ladder = facing(blocks::LADDER, Direction::West);
            at.place_block(ladder, 9, 1, 3);
            at.place_block(ladder, 9, 2, 3);
            at.place_block(ladder, 9, 3, 3);
            at.create_chest(3, 4, 8, CROSSING_LOOT);
        }
        _ => {}
    }
}

/// `StraightStairsDown`.
fn straight_stairs_down(at: &mut CodePiece<'_, '_, '_, '_>, door: StrongholdSmallDoorType) {
    stone_box(at, [0, 0, 0], [4, 10, 7], true);
    small_door(at, door, 1, 7, 0);
    small_door(at, StrongholdSmallDoorType::Opening, 1, 1, 7);
    let stairs = facing(blocks::COBBLESTONE_STAIRS, Direction::South);
    for i in 0..6 {
        at.place_block(stairs, 1, 6 - i, 1 + i);
        at.place_block(stairs, 2, 6 - i, 1 + i);
        at.place_block(stairs, 3, 6 - i, 1 + i);
        if i < 5 {
            at.place_block(blocks::STONE_BRICKS, 1, 5 - i, 1 + i);
            at.place_block(blocks::STONE_BRICKS, 2, 5 - i, 1 + i);
            at.place_block(blocks::STONE_BRICKS, 3, 5 - i, 1 + i);
        }
    }
}

/// `StairsDown`, also the stronghold's first piece: a winding stair of bricks and
/// slabs.
fn stairs_down(at: &mut CodePiece<'_, '_, '_, '_>, door: StrongholdSmallDoorType) {
    stone_box(at, [0, 0, 0], [4, 10, 4], true);
    small_door(at, door, 1, 7, 0);
    small_door(at, StrongholdSmallDoorType::Opening, 1, 1, 4);
    let (bricks, slab) = (blocks::STONE_BRICKS, blocks::SMOOTH_STONE_SLAB);
    for (state, x, y, z) in [
        (bricks, 2, 6, 1),
        (bricks, 1, 5, 1),
        (slab, 1, 6, 1),
        (bricks, 1, 5, 2),
        (bricks, 1, 4, 3),
        (slab, 1, 5, 3),
        (bricks, 2, 4, 3),
        (bricks, 3, 3, 3),
        (slab, 3, 4, 3),
        (bricks, 3, 3, 2),
        (bricks, 3, 2, 1),
        (slab, 3, 3, 1),
        (bricks, 2, 2, 1),
        (bricks, 1, 1, 1),
        (slab, 1, 2, 1),
        (bricks, 1, 1, 2),
        (slab, 1, 1, 3),
    ] {
        at.place_block(state, x, y, z);
    }
}

/// `FiveCrossing`; `exits` are its left low, left high, right low and right high
/// openings.
fn five_crossing(
    at: &mut CodePiece<'_, '_, '_, '_>,
    door: StrongholdSmallDoorType,
    exits: [bool; 4],
) {
    let [left_low, left_high, right_low, right_high] = exits;
    stone_box(at, [0, 0, 0], [9, 8, 10], true);
    small_door(at, door, 4, 3, 0);
    if left_low {
        air(at, [0, 3, 1], [0, 5, 3]);
    }
    if right_low {
        air(at, [9, 3, 1], [9, 5, 3]);
    }
    if left_high {
        air(at, [0, 5, 7], [0, 7, 9]);
    }
    if right_high {
        air(at, [9, 5, 7], [9, 7, 9]);
    }
    air(at, [5, 1, 10], [7, 3, 10]);
    stone_box(at, [1, 2, 1], [8, 2, 6], false);
    stone_box(at, [4, 1, 5], [4, 4, 9], false);
    stone_box(at, [8, 1, 5], [8, 4, 9], false);
    stone_box(at, [1, 4, 7], [3, 4, 9], false);
    stone_box(at, [1, 3, 5], [3, 3, 6], false);
    let slab = blocks::SMOOTH_STONE_SLAB;
    fill(at, [1, 3, 4], [3, 3, 4], slab);
    fill(at, [1, 4, 6], [3, 4, 6], slab);
    stone_box(at, [5, 1, 7], [7, 1, 8], false);
    fill(at, [5, 1, 9], [7, 1, 9], slab);
    fill(at, [5, 2, 7], [7, 2, 7], slab);
    fill(at, [4, 5, 7], [4, 5, 9], slab);
    fill(at, [8, 5, 7], [8, 5, 9], slab);
    fill(at, [5, 5, 7], [7, 5, 9], with(slab, "type", "double"));
    at.place_block(facing(blocks::WALL_TORCH, Direction::South), 6, 5, 6);
}

/// `ChestCorridor`. Returns whether its chest was placed by this run.
fn chest_corridor(
    at: &mut CodePiece<'_, '_, '_, '_>,
    door: StrongholdSmallDoorType,
    placed: bool,
) -> bool {
    stone_box(at, [0, 0, 0], [4, 4, 6], true);
    small_door(at, door, 1, 1, 0);
    small_door(at, StrongholdSmallDoorType::Opening, 1, 1, 6);
    fill(at, [3, 1, 2], [3, 1, 4], blocks::STONE_BRICKS);
    let slab = blocks::STONE_BRICK_SLAB;
    at.place_block(slab, 3, 1, 1);
    at.place_block(slab, 3, 1, 5);
    at.place_block(slab, 3, 2, 2);
    at.place_block(slab, 3, 2, 4);
    for z in 2..=4 {
        at.place_block(slab, 2, 1, z);
    }
    let chest = at.world_pos(3, 2, 3);
    if !placed && at.inside(chest) {
        at.create_chest(3, 2, 3, CORRIDOR_LOOT);
        return true;
    }
    false
}

/// `Library`: shelves along the walls and in three rows, cobwebs, torches, and in the
/// tall one a gallery of planks with fences, a ladder, a chandelier and a second
/// chest.
fn library(at: &mut CodePiece<'_, '_, '_, '_>, door: StrongholdSmallDoorType, tall: bool) {
    let height = if tall { 11 } else { 6 };
    stone_box(at, [0, 0, 0], [13, height - 1, 14], true);
    small_door(at, door, 4, 1, 0);
    at.generate_maybe_box(
        0.07,
        [2, 1, 1],
        [11, 4, 13],
        blocks::COBWEB,
        blocks::COBWEB,
        false,
        false,
    );
    let (planks, shelf) = (blocks::OAK_PLANKS, blocks::BOOKSHELF);
    for z in 1..=13 {
        if (z - 1) % 4 == 0 {
            fill(at, [1, 1, z], [1, 4, z], planks);
            fill(at, [12, 1, z], [12, 4, z], planks);
            at.place_block(facing(blocks::WALL_TORCH, Direction::East), 2, 3, z);
            at.place_block(facing(blocks::WALL_TORCH, Direction::West), 11, 3, z);
            if tall {
                fill(at, [1, 6, z], [1, 9, z], planks);
                fill(at, [12, 6, z], [12, 9, z], planks);
            }
        } else {
            fill(at, [1, 1, z], [1, 4, z], shelf);
            fill(at, [12, 1, z], [12, 4, z], shelf);
            if tall {
                fill(at, [1, 6, z], [1, 9, z], shelf);
                fill(at, [12, 6, z], [12, 9, z], shelf);
            }
        }
    }
    for z in (3..12).step_by(2) {
        fill(at, [3, 1, z], [4, 3, z], shelf);
        fill(at, [6, 1, z], [7, 3, z], shelf);
        fill(at, [9, 1, z], [10, 3, z], shelf);
    }
    if tall {
        fill(at, [1, 5, 1], [3, 5, 13], planks);
        fill(at, [10, 5, 1], [12, 5, 13], planks);
        fill(at, [4, 5, 1], [9, 5, 2], planks);
        fill(at, [4, 5, 12], [9, 5, 13], planks);
        at.place_block(planks, 9, 5, 11);
        at.place_block(planks, 8, 5, 11);
        at.place_block(planks, 9, 5, 10);
        let fence = blocks::OAK_FENCE;
        let across = joined(fence, &["west", "east"]);
        let along = joined(fence, &["north", "south"]);
        fill(at, [3, 6, 3], [3, 6, 11], along);
        fill(at, [10, 6, 3], [10, 6, 9], along);
        fill(at, [4, 6, 2], [9, 6, 2], across);
        fill(at, [4, 6, 12], [7, 6, 12], across);
        at.place_block(joined(fence, &["north", "east"]), 3, 6, 2);
        at.place_block(joined(fence, &["south", "east"]), 3, 6, 12);
        at.place_block(joined(fence, &["north", "west"]), 10, 6, 2);
        for i in 0..=2 {
            at.place_block(joined(fence, &["south", "west"]), 8 + i, 6, 12 - i);
            if i != 2 {
                at.place_block(joined(fence, &["north", "east"]), 8 + i, 6, 11 - i);
            }
        }
        let ladder = facing(blocks::LADDER, Direction::South);
        for y in 1..=7 {
            at.place_block(ladder, 10, y, 13);
        }
        let east = joined(fence, &["east"]);
        let west = joined(fence, &["west"]);
        at.place_block(east, 6, 9, 7);
        at.place_block(west, 7, 9, 7);
        at.place_block(east, 6, 8, 7);
        at.place_block(west, 7, 8, 7);
        let all = joined(along, &["west", "east"]);
        at.place_block(all, 6, 7, 7);
        at.place_block(all, 7, 7, 7);
        at.place_block(east, 5, 7, 7);
        at.place_block(west, 8, 7, 7);
        at.place_block(joined(east, &["north"]), 6, 7, 6);
        at.place_block(joined(east, &["south"]), 6, 7, 8);
        at.place_block(joined(west, &["north"]), 7, 7, 6);
        at.place_block(joined(west, &["south"]), 7, 7, 8);
        for (x, z) in [(5, 7), (8, 7), (6, 6), (6, 8), (7, 6), (7, 8)] {
            at.place_block(blocks::TORCH, x, 8, z);
        }
    }
    at.create_chest(3, 3, 5, LIBRARY_LOOT);
    if tall {
        at.place_block(blocks::CAVE_AIR, 12, 9, 1);
        at.create_chest(12, 8, 1, LIBRARY_LOOT);
    }
}

/// `PortalRoom`. Returns whether its spawner was placed by this run.
fn portal_room(at: &mut CodePiece<'_, '_, '_, '_>, placed: bool) -> bool {
    stone_box(at, [0, 0, 0], [10, 7, 15], false);
    small_door(at, StrongholdSmallDoorType::Grates, 4, 1, 0);
    stone_box(at, [1, 6, 1], [1, 6, 14], false);
    stone_box(at, [9, 6, 1], [9, 6, 14], false);
    stone_box(at, [2, 6, 1], [8, 6, 2], false);
    stone_box(at, [2, 6, 14], [8, 6, 14], false);
    stone_box(at, [1, 1, 1], [2, 1, 4], false);
    stone_box(at, [8, 1, 1], [9, 1, 4], false);
    fill(at, [1, 1, 1], [1, 1, 3], blocks::LAVA);
    fill(at, [9, 1, 1], [9, 1, 3], blocks::LAVA);
    stone_box(at, [3, 1, 8], [7, 1, 12], false);
    fill(at, [4, 1, 9], [6, 1, 11], blocks::LAVA);
    let along = joined(blocks::IRON_BARS, &["north", "south"]);
    let across = joined(blocks::IRON_BARS, &["west", "east"]);
    for z in (3..14).step_by(2) {
        fill(at, [0, 3, z], [0, 4, z], along);
        fill(at, [10, 3, z], [10, 4, z], along);
    }
    for x in (2..9).step_by(2) {
        fill(at, [x, 3, 15], [x, 4, 15], across);
    }
    let stairs = facing(blocks::STONE_BRICK_STAIRS, Direction::North);
    stone_box(at, [4, 1, 5], [6, 1, 7], false);
    stone_box(at, [4, 2, 6], [6, 2, 7], false);
    stone_box(at, [4, 3, 7], [6, 3, 7], false);
    for x in 4..=6 {
        at.place_block(stairs, x, 1, 4);
        at.place_block(stairs, x, 2, 5);
        at.place_block(stairs, x, 3, 6);
    }

    let mut eyes = [false; 12];
    for eye in &mut eyes {
        *eye = at.random().next_f32() > 0.9;
    }
    let frames = [
        (Direction::North, 4, 8),
        (Direction::North, 5, 8),
        (Direction::North, 6, 8),
        (Direction::South, 4, 12),
        (Direction::South, 5, 12),
        (Direction::South, 6, 12),
        (Direction::East, 3, 9),
        (Direction::East, 3, 10),
        (Direction::East, 3, 11),
        (Direction::West, 7, 9),
        (Direction::West, 7, 10),
        (Direction::West, 7, 11),
    ];
    for ((direction, x, z), eye) in frames.into_iter().zip(eyes) {
        let frame = facing(blocks::END_PORTAL_FRAME, direction);
        at.place_block(
            with(frame, "eye", if eye { "true" } else { "false" }),
            x,
            3,
            z,
        );
    }
    if eyes.iter().all(|eye| *eye) {
        for z in 9..=11 {
            for x in 4..=6 {
                at.place_block(blocks::END_PORTAL, x, 3, z);
            }
        }
    }

    let spawner = at.world_pos(5, 3, 6);
    if !placed && at.inside(spawner) {
        at.place_spawner(spawner, SILVERFISH);
        return true;
    }
    false
}

/// `FillerCorridor`: a plain tunnel of bricks, `steps` long, drawing nothing.
fn filler_corridor(at: &mut CodePiece<'_, '_, '_, '_>, steps: i32) {
    let bricks = blocks::STONE_BRICKS;
    for i in 0..steps {
        for x in 0..=4 {
            at.place_block(bricks, x, 0, i);
        }
        for y in 1..=3 {
            at.place_block(bricks, 0, y, i);
            at.place_block(blocks::CAVE_AIR, 1, y, i);
            at.place_block(blocks::CAVE_AIR, 2, y, i);
            at.place_block(blocks::CAVE_AIR, 3, y, i);
            at.place_block(bricks, 4, y, i);
        }
        for x in 0..=4 {
            at.place_block(bricks, x, 4, i);
        }
    }
}

#[cfg(test)]
mod tests {
    use clustine_noise::WorldgenRandom;
    use clustine_structures::geometry::{BoundingBox, IVec3};
    use clustine_structures::piece::{ProceduralPieceData, StructurePiecePayload};
    use clustine_world::{BlockPos, Ticked};

    use super::*;
    use crate::blocks::same_block;
    use crate::structures::code_piece::tests::{drew_floats, piece, placing};
    use crate::testing::World;

    const SEED: u64 = 11;

    fn stronghold_piece(
        min: (i32, i32, i32),
        max: (i32, i32, i32),
        facing: Direction,
        data: StrongholdPieceData,
    ) -> StructurePiece {
        piece(
            BoundingBox::new(
                IVec3::new(min.0, min.1, min.2),
                IVec3::new(max.0, max.1, max.2),
            ),
            Some(facing),
            StructurePiecePayload::Procedural(ProceduralPieceData::Stronghold(data)),
        )
    }

    fn run(
        world: &mut World,
        piece: &StructurePiece,
        state: PieceState,
    ) -> (PieceState, WorldgenRandom) {
        let StructurePiecePayload::Procedural(ProceduralPieceData::Stronghold(data)) =
            &piece.payload
        else {
            unreachable!("the tests make stronghold pieces");
        };
        placing(world, SEED, |placing| place(placing, piece, data, state))
    }

    fn placed(world: &World, block: BlockState) -> usize {
        world
            .writes
            .iter()
            .filter(|(_, state)| same_block(*state, block))
            .count()
    }

    #[test]
    fn a_straight_corridor_draws_once_for_each_block_of_its_walls_then_four_torches() {
        let mut world = World::flat(blocks::STONE, 40);
        let corridor = stronghold_piece(
            (2, 20, 2),
            (6, 24, 8),
            Direction::South,
            StrongholdPieceData::Straight {
                entry_door: StrongholdSmallDoorType::Opening,
                left_child: true,
                right_child: false,
            },
        );
        let (kept, mut random) = run(&mut world, &corridor, PieceState::default());
        // 5 × 5 × 7 blocks, of which the 3 × 3 × 5 inside are cave air, drawn for none.
        assert!(drew_floats(&mut random, SEED, 5 * 5 * 7 - 3 * 3 * 5 + 4));
        assert_eq!(kept, PieceState::default());
        assert_eq!(world.get(BlockPos::new(4, 21, 5)), blocks::CAVE_AIR);
        // The opening to the left, at x 0 of the piece, and the wall to the right.
        assert_eq!(world.get(BlockPos::new(2, 22, 5)), blocks::CAVE_AIR);
        assert_ne!(world.get(BlockPos::new(6, 22, 5)), blocks::CAVE_AIR);
        // The doorways at both ends.
        assert_eq!(world.get(BlockPos::new(4, 22, 2)), blocks::CAVE_AIR);
        assert_eq!(world.get(BlockPos::new(4, 22, 8)), blocks::CAVE_AIR);
    }

    #[test]
    fn walls_already_open_to_the_air_are_left_and_not_drawn_for() {
        let mut world = World::flat(blocks::STONE, 22);
        let corridor = stronghold_piece(
            (2, 20, 2),
            (6, 24, 8),
            Direction::South,
            StrongholdPieceData::Straight {
                entry_door: StrongholdSmallDoorType::Opening,
                left_child: false,
                right_child: false,
            },
        );
        let (_, mut random) = run(&mut world, &corridor, PieceState::default());
        // Only the three lowest layers stand in rock: their faces are drawn for.
        let walls = 5 * 3 * 7 - 3 * 2 * 5;
        assert!(drew_floats(&mut random, SEED, walls + 4));
        assert_eq!(world.get(BlockPos::new(2, 23, 5)), blocks::AIR);
    }

    #[test]
    fn a_filler_corridor_draws_nothing() {
        let mut world = World::flat(blocks::STONE, 40);
        let filler = stronghold_piece(
            (2, 20, 2),
            (6, 24, 4),
            Direction::North,
            StrongholdPieceData::FillerCorridor { steps: 3 },
        );
        let (_, mut random) = run(&mut world, &filler, PieceState::default());
        assert!(drew_floats(&mut random, SEED, 0));
        assert_eq!(world.get(BlockPos::new(4, 22, 3)), blocks::CAVE_AIR);
        assert_eq!(world.get(BlockPos::new(2, 22, 3)), blocks::STONE_BRICKS);
        assert_eq!(world.get(BlockPos::new(4, 24, 3)), blocks::STONE_BRICKS);
    }

    #[test]
    fn the_portal_room_draws_an_eye_for_each_frame_and_places_its_spawner_once() {
        let mut world = World::flat(blocks::STONE, 40);
        let room = stronghold_piece(
            (0, 20, 0),
            (10, 27, 15),
            Direction::North,
            StrongholdPieceData::PortalRoom {
                has_placed_spawner: false,
            },
        );
        let (kept, mut random) = run(&mut world, &room, PieceState::default());
        // The walls' draws: every face block of each box with the selector, the
        // walls whole, as none of these skips air.
        let walls = (11 * 8 * 16 - 9 * 6 * 14) + 14 + 14 + 14 + 7 + 8 + 8 + 25 + 9 + 6 + 3;
        assert!(drew_floats(&mut random, SEED, walls + 12));

        let mut eyes = WorldgenRandom::from_seed(SEED);
        for _ in 0..walls {
            eyes.next_f32();
        }
        let first_eye = eyes.next_f32() > 0.9;
        let frame = blocks::END_PORTAL_FRAME
            .with_property("facing", "north")
            .and_then(|frame| frame.with_property("eye", if first_eye { "true" } else { "false" }))
            .unwrap();
        assert_eq!(world.get(BlockPos::new(4, 23, 7)), frame);

        assert_eq!(world.get(BlockPos::new(5, 23, 9)), blocks::SPAWNER);
        assert_eq!(kept.flags, ONE_PLACED);
        let lava = world
            .ticks
            .iter()
            .filter(|(_, tick)| matches!(tick.of, Ticked::Fluid(_)))
            .count();
        assert_eq!(lava, 3 + 3 + 9);

        // Placed again in another chunk's run, it knows its spawner stands.
        let mut again = World::flat(blocks::STONE, 40);
        let (kept_again, _) = run(&mut again, &room, kept);
        assert_eq!(placed(&again, blocks::SPAWNER), 0);
        assert_eq!(kept_again, kept);
    }

    #[test]
    fn a_chest_corridor_places_its_chest_once_and_draws_its_seed() {
        let mut world = World::flat(blocks::STONE, 40);
        let corridor = stronghold_piece(
            (2, 20, 2),
            (6, 24, 8),
            Direction::South,
            StrongholdPieceData::ChestCorridor {
                entry_door: StrongholdSmallDoorType::WoodDoor,
                has_placed_chest: false,
            },
        );
        let (kept, mut random) = run(&mut world, &corridor, PieceState::default());
        let mut expected = WorldgenRandom::from_seed(SEED);
        for _ in 0..5 * 5 * 7 - 3 * 3 * 5 {
            expected.next_f32();
        }
        expected.next_i64();
        assert_eq!(random.next_i64(), expected.next_i64());
        assert!(same_block(
            world.get(BlockPos::new(5, 22, 5)),
            blocks::CHEST
        ));
        assert_eq!(kept.flags, ONE_PLACED);
        // The wooden door, lower and upper half.
        assert!(same_block(
            world.get(BlockPos::new(4, 21, 2)),
            blocks::OAK_DOOR
        ));
        assert_eq!(
            world.get(BlockPos::new(4, 22, 2)).property("half"),
            Some("upper")
        );

        let mut again = World::flat(blocks::STONE, 40);
        run(&mut again, &corridor, kept);
        assert_eq!(placed(&again, blocks::CHEST), 0);
    }

    #[test]
    fn the_fountain_schedules_its_water() {
        let mut world = World::flat(blocks::STONE, 40);
        let room = stronghold_piece(
            (2, 20, 2),
            (12, 26, 12),
            Direction::South,
            StrongholdPieceData::RoomCrossing {
                entry_door: StrongholdSmallDoorType::Grates,
                crossing_type: 1,
            },
        );
        run(&mut world, &room, PieceState::default());
        assert_eq!(world.get(BlockPos::new(7, 24, 7)), blocks::WATER);
        assert!(world.ticks.iter().any(|(at, tick)| {
            *at == BlockPos::new(7, 24, 7) && matches!(tick.of, Ticked::Fluid(_))
        }));
        // The grates of the entrance are marked to join up when the chunk is done.
        assert!(placed(&world, blocks::IRON_BARS) > 0);
        assert!(world.marks.contains(&BlockPos::new(6, 21, 2)));
    }

    #[test]
    fn a_prison_turned_east_turns_its_doors() {
        let mut world = World::flat(blocks::STONE, 40);
        let prison = stronghold_piece(
            (2, 20, 2),
            (12, 24, 10),
            Direction::East,
            StrongholdPieceData::PrisonHall {
                entry_door: StrongholdSmallDoorType::IronDoor,
            },
        );
        run(&mut world, &prison, PieceState::default());
        let door = world.get(BlockPos::new(4, 21, 6));
        assert!(same_block(door, blocks::IRON_DOOR));
        assert_eq!(door.property("facing"), Some("north"));
        assert_eq!(door.property("half"), Some("lower"));
    }

    #[test]
    fn a_left_turn_opens_to_its_left_by_its_orientation() {
        for (facing, open_x) in [(Direction::North, 2), (Direction::South, 6)] {
            let mut world = World::flat(blocks::STONE, 40);
            let turn = stronghold_piece(
                (2, 20, 2),
                (6, 24, 6),
                facing,
                StrongholdPieceData::LeftTurn {
                    entry_door: StrongholdSmallDoorType::Opening,
                },
            );
            run(&mut world, &turn, PieceState::default());
            assert_eq!(world.get(BlockPos::new(open_x, 22, 4)), blocks::CAVE_AIR);
            assert_ne!(
                world.get(BlockPos::new(8 - open_x, 22, 4)),
                blocks::CAVE_AIR
            );
        }
    }

    #[test]
    fn a_tall_library_has_its_gallery_and_two_chests() {
        let mut world = World::flat(blocks::STONE, 40);
        let library = stronghold_piece(
            (0, 20, 0),
            (13, 30, 14),
            Direction::North,
            StrongholdPieceData::Library {
                entry_door: StrongholdSmallDoorType::Opening,
                is_tall: true,
            },
        );
        run(&mut world, &library, PieceState::default());
        assert_eq!(world.get(BlockPos::new(1, 21, 12)), blocks::BOOKSHELF);
        assert_eq!(world.get(BlockPos::new(1, 26, 12)), blocks::BOOKSHELF);
        assert_eq!(world.get(BlockPos::new(2, 25, 12)), blocks::OAK_PLANKS);
        assert_eq!(placed(&world, blocks::CHEST), 2);
        assert!(placed(&world, blocks::LADDER) == 7);
    }
}
