// Adapted from SteelMC's `steel-core/src/worldgen/structure/piece_placer/jungle_temple.rs`
// (see NOTICE in this crate), which follows the game's `JungleTemplePiece`.

//! The jungle temple (`JungleTemplePiece`): walls of cobblestone, mossy where the
//! piece's `MossStoneSelector` draws it, the stairs down, the tripwire trap with its two
//! dispensers of arrows, the lever puzzle with its sticky pistons, vines, and two
//! chests.
//!
//! The piece settles on the mean height of the ground under its columns in the first
//! chunk that places it.

use clustine_data::{BlockState, blocks};
use clustine_noise::Random;
use clustine_structures::geometry::Direction;
use clustine_structures::piece::StructurePiece;
use clustine_world::generation::PieceState;

use super::Placing;
use super::scattered::{Piece, settle_on_average, settled_box, with};

const LOOT: &str = "minecraft:chests/jungle_temple";
const DISPENSER_LOOT: &str = "minecraft:chests/jungle_temple_dispenser";

/// What the piece keeps of its chests and traps, as bits of `PieceState::flags`.
const MAIN_CHEST: u16 = 1;
const HIDDEN_CHEST: u16 = 1 << 1;
const TRAP_1: u16 = 1 << 2;
const TRAP_2: u16 = 1 << 3;

/// `generateBox` with the `MossStoneSelector`: cobblestone for four draws in ten,
/// mossy cobblestone otherwise, not skipping air.
fn moss_box(p: &mut Placing<'_, '_, '_>, at: &Piece, from: [i32; 3], to: [i32; 3]) {
    at.generate_box_selected(p, from, to, false, |random, _| {
        if random.next_f32() < 0.4 {
            blocks::COBBLESTONE
        } else {
            blocks::MOSSY_COBBLESTONE
        }
    });
}

fn stairs(facing: &str) -> BlockState {
    with(blocks::COBBLESTONE_STAIRS, "facing", facing)
}

fn tripwire_hook(facing: &str) -> BlockState {
    with(
        with(blocks::TRIPWIRE_HOOK, "facing", facing),
        "attached",
        "true",
    )
}

fn tripwire(sides: [&str; 2]) -> BlockState {
    let mut state = with(blocks::TRIPWIRE, "attached", "true");
    for side in sides {
        state = with(state, side, "true");
    }
    state
}

/// Redstone wire joined to the sides given, `side` or `up`.
fn wire(sides: &[(&str, &str)]) -> BlockState {
    sides
        .iter()
        .fold(blocks::REDSTONE_WIRE, |state, (side, how)| {
            with(state, side, how)
        })
}

fn vine(side: &str) -> BlockState {
    with(blocks::VINE, side, "true")
}

/// `JungleTemplePiece.postProcess`. Returns the piece's state: where it settled, and
/// which chests and dispensers it has placed.
#[expect(
    clippy::too_many_lines,
    reason = "the game's postProcess is one long list of blocks"
)]
pub(super) fn place(
    placing: &mut Placing<'_, '_, '_>,
    piece: &StructurePiece,
    state: PieceState,
) -> PieceState {
    let Some(mut state) = settle_on_average(placing, piece.bounding_box, state, 0) else {
        return state;
    };
    let at = Piece::new(settled_box(piece.bounding_box, state), piece.orientation);
    let p = placing;
    let air = blocks::AIR;
    let mossy = blocks::MOSSY_COBBLESTONE;

    moss_box(p, &at, [0, -4, 0], [11, 0, 14]);
    moss_box(p, &at, [2, 1, 2], [9, 2, 2]);
    moss_box(p, &at, [2, 1, 12], [9, 2, 12]);
    moss_box(p, &at, [2, 1, 3], [2, 2, 11]);
    moss_box(p, &at, [9, 1, 3], [9, 2, 11]);
    moss_box(p, &at, [1, 3, 1], [10, 6, 1]);
    moss_box(p, &at, [1, 3, 13], [10, 6, 13]);
    moss_box(p, &at, [1, 3, 2], [1, 6, 12]);
    moss_box(p, &at, [10, 3, 2], [10, 6, 12]);
    moss_box(p, &at, [2, 3, 2], [9, 3, 12]);
    moss_box(p, &at, [2, 6, 2], [9, 6, 12]);
    moss_box(p, &at, [3, 7, 3], [8, 7, 11]);
    moss_box(p, &at, [4, 8, 4], [7, 8, 10]);
    at.generate_air_box(p, [3, 1, 3], [8, 2, 11]);
    at.generate_air_box(p, [4, 3, 6], [7, 3, 9]);
    at.generate_air_box(p, [2, 4, 2], [9, 5, 12]);
    at.generate_air_box(p, [4, 6, 5], [7, 6, 9]);
    at.generate_air_box(p, [5, 7, 6], [6, 7, 8]);
    at.generate_air_box(p, [5, 1, 2], [6, 2, 2]);
    at.generate_air_box(p, [5, 2, 12], [6, 2, 12]);
    at.generate_air_box(p, [5, 5, 1], [6, 5, 1]);
    at.generate_air_box(p, [5, 5, 13], [6, 5, 13]);
    at.place_block(p, air, 1, 5, 5);
    at.place_block(p, air, 10, 5, 5);
    at.place_block(p, air, 1, 5, 9);
    at.place_block(p, air, 10, 5, 9);

    for z in [0, 14] {
        for x in [2, 4, 7, 9] {
            moss_box(p, &at, [x, 4, z], [x, 5, z]);
        }
    }
    moss_box(p, &at, [5, 6, 0], [6, 6, 0]);
    for x in [0, 11] {
        for z in (2..=12).step_by(2) {
            moss_box(p, &at, [x, 4, z], [x, 5, z]);
        }
        moss_box(p, &at, [x, 6, 5], [x, 6, 5]);
        moss_box(p, &at, [x, 6, 9], [x, 6, 9]);
    }
    moss_box(p, &at, [2, 7, 2], [2, 9, 2]);
    moss_box(p, &at, [9, 7, 2], [9, 9, 2]);
    moss_box(p, &at, [2, 7, 12], [2, 9, 12]);
    moss_box(p, &at, [9, 7, 12], [9, 9, 12]);
    moss_box(p, &at, [4, 9, 4], [4, 9, 4]);
    moss_box(p, &at, [7, 9, 4], [7, 9, 4]);
    moss_box(p, &at, [4, 9, 10], [4, 9, 10]);
    moss_box(p, &at, [7, 9, 10], [7, 9, 10]);
    moss_box(p, &at, [5, 9, 7], [6, 9, 7]);

    let (east, west) = (stairs("east"), stairs("west"));
    let (south, north) = (stairs("south"), stairs("north"));
    at.place_block(p, north, 5, 9, 6);
    at.place_block(p, north, 6, 9, 6);
    at.place_block(p, south, 5, 9, 8);
    at.place_block(p, south, 6, 9, 8);
    for x in 4..=7 {
        at.place_block(p, north, x, 0, 0);
    }
    at.place_block(p, north, 4, 1, 8);
    at.place_block(p, north, 4, 2, 9);
    at.place_block(p, north, 4, 3, 10);
    at.place_block(p, north, 7, 1, 8);
    at.place_block(p, north, 7, 2, 9);
    at.place_block(p, north, 7, 3, 10);
    moss_box(p, &at, [4, 1, 9], [4, 1, 9]);
    moss_box(p, &at, [7, 1, 9], [7, 1, 9]);
    moss_box(p, &at, [4, 1, 10], [7, 2, 10]);
    moss_box(p, &at, [5, 4, 5], [6, 4, 5]);
    at.place_block(p, east, 4, 4, 5);
    at.place_block(p, west, 7, 4, 5);
    for i in 0..4 {
        at.place_block(p, south, 5, -i, 6 + i);
        at.place_block(p, south, 6, -i, 6 + i);
        at.generate_air_box(p, [5, -i, 7 + i], [6, -i, 9 + i]);
    }
    at.generate_air_box(p, [1, -3, 12], [10, -1, 13]);
    at.generate_air_box(p, [1, -3, 1], [3, -1, 13]);
    at.generate_air_box(p, [1, -3, 1], [9, -1, 5]);
    for z in (1..=13).step_by(2) {
        moss_box(p, &at, [1, -3, z], [1, -2, z]);
    }
    for z in (2..=12).step_by(2) {
        moss_box(p, &at, [1, -1, z], [3, -1, z]);
    }
    moss_box(p, &at, [2, -2, 1], [5, -2, 1]);
    moss_box(p, &at, [7, -2, 1], [9, -2, 1]);
    moss_box(p, &at, [6, -3, 1], [6, -3, 1]);
    moss_box(p, &at, [6, -1, 1], [6, -1, 1]);

    // The tripwire across the corridor to the west, and the dispenser it fires.
    at.place_block(p, tripwire_hook("east"), 1, -3, 8);
    at.place_block(p, tripwire_hook("west"), 4, -3, 8);
    at.place_block(p, tripwire(["east", "west"]), 2, -3, 8);
    at.place_block(p, tripwire(["east", "west"]), 3, -3, 8);
    let north_south = wire(&[("north", "side"), ("south", "side")]);
    for z in (2..=7).rev() {
        at.place_block(p, north_south, 5, -3, z);
    }
    at.place_block(p, wire(&[("north", "side"), ("west", "side")]), 5, -3, 1);
    at.place_block(p, wire(&[("east", "side"), ("west", "side")]), 4, -3, 1);
    at.place_block(p, mossy, 3, -3, 1);
    if state.flags & TRAP_1 == 0
        && at.create_dispenser(p, 3, -2, 1, Direction::North, DISPENSER_LOOT)
    {
        state.flags |= TRAP_1;
    }
    at.place_block(p, vine("south"), 3, -2, 2);

    // The tripwire across the corridor to the north, and its dispenser.
    at.place_block(p, tripwire_hook("north"), 7, -3, 1);
    at.place_block(p, tripwire_hook("south"), 7, -3, 5);
    for z in 2..=4 {
        at.place_block(p, tripwire(["north", "south"]), 7, -3, z);
    }
    at.place_block(p, wire(&[("east", "side"), ("west", "side")]), 8, -3, 6);
    at.place_block(p, wire(&[("west", "side"), ("south", "side")]), 9, -3, 6);
    at.place_block(p, wire(&[("north", "side"), ("south", "up")]), 9, -3, 5);
    at.place_block(p, mossy, 9, -3, 4);
    at.place_block(p, north_south, 9, -2, 4);
    if state.flags & TRAP_2 == 0
        && at.create_dispenser(p, 9, -2, 3, Direction::West, DISPENSER_LOOT)
    {
        state.flags |= TRAP_2;
    }
    at.place_block(p, vine("east"), 8, -1, 3);
    at.place_block(p, vine("east"), 8, -2, 3);
    if state.flags & MAIN_CHEST == 0 && at.create_chest(p, 8, -3, 3, LOOT) {
        state.flags |= MAIN_CHEST;
    }
    for (x, y, z) in [
        (9, -3, 2),
        (8, -3, 1),
        (4, -3, 5),
        (5, -2, 5),
        (5, -1, 5),
        (6, -3, 5),
        (7, -2, 5),
        (7, -1, 5),
        (8, -3, 5),
    ] {
        at.place_block(p, mossy, x, y, z);
    }
    moss_box(p, &at, [9, -1, 1], [9, -1, 5]);

    // The lever puzzle and the hidden chest behind it.
    at.generate_air_box(p, [8, -3, 8], [10, -1, 10]);
    for x in 8..=10 {
        at.place_block(p, blocks::CHISELED_STONE_BRICKS, x, -2, 11);
    }
    let lever = with(with(blocks::LEVER, "facing", "north"), "face", "wall");
    for x in 8..=10 {
        at.place_block(p, lever, x, -2, 12);
    }
    moss_box(p, &at, [8, -3, 8], [8, -3, 10]);
    moss_box(p, &at, [10, -3, 8], [10, -3, 10]);
    at.place_block(p, mossy, 10, -2, 9);
    at.place_block(p, north_south, 8, -2, 9);
    at.place_block(p, north_south, 8, -2, 10);
    let all_sides = wire(&[
        ("north", "side"),
        ("south", "side"),
        ("east", "side"),
        ("west", "side"),
    ]);
    at.place_block(p, all_sides, 10, -1, 9);
    at.place_block(p, with(blocks::STICKY_PISTON, "facing", "up"), 9, -2, 8);
    at.place_block(p, with(blocks::STICKY_PISTON, "facing", "west"), 10, -2, 8);
    at.place_block(p, with(blocks::STICKY_PISTON, "facing", "west"), 10, -1, 8);
    at.place_block(p, with(blocks::REPEATER, "facing", "north"), 10, -2, 10);
    if state.flags & HIDDEN_CHEST == 0 && at.create_chest(p, 9, -3, 10, LOOT) {
        state.flags |= HIDDEN_CHEST;
    }
    state
}

#[cfg(test)]
mod tests {
    use clustine_protocol::codec::Reader;
    use clustine_protocol::nbt::Nbt;
    use clustine_structures::geometry::{BoundingBox, IVec3};
    use clustine_structures::jungle_temple::JungleTemplePieceData;
    use clustine_structures::piece::{ProceduralPieceData, StructurePiecePayload};

    use super::*;
    use crate::structures::scattered::testing::{Run, area, chunk, piece};
    use crate::testing::World;

    fn temple(orientation: Direction) -> StructurePiece {
        piece(
            BoundingBox::new(IVec3::new(0, 64, 0), IVec3::new(11, 73, 14)),
            orientation,
            StructurePiecePayload::Procedural(ProceduralPieceData::JungleTemple(
                JungleTemplePieceData::new(),
            )),
        )
    }

    fn loot_table(run: &Run, pos: clustine_world::BlockPos) -> Nbt {
        let entity = run.world.entities.get(&pos).expect("it keeps one");
        let data = Reader::new(&entity.data)
            .nbt()
            .expect("the data reads")
            .expect("the data is a compound");
        data.get("LootTable").expect("it has a table").clone()
    }

    #[test]
    fn the_temple_settles_on_its_mean_ground_and_places_its_chests_and_traps() {
        let mut run = Run::new(World::flat(blocks::GRASS_BLOCK, 80), 5);
        let piece = temple(Direction::North);
        let state = run.place(area(), |p| place(p, &piece, PieceState::default()));
        assert_eq!(state.shift_y, 81 - 64);
        assert_eq!(state.flags, MAIN_CHEST | HIDDEN_CHEST | TRAP_1 | TRAP_2);
        let at = Piece::new(settled_box(piece.bounding_box, state), piece.orientation);
        for (x, y, z) in [(8, -3, 3), (9, -3, 10)] {
            let pos = at.world_pos(x, y, z);
            assert!(crate::blocks::same_block(run.get(pos), blocks::CHEST));
            assert_eq!(loot_table(&run, pos), Nbt::String(LOOT.to_string()));
        }
        let trap = at.world_pos(3, -2, 1);
        assert_eq!(run.get(trap).property("facing"), Some("north"));
        assert_eq!(
            loot_table(&run, trap),
            Nbt::String(DISPENSER_LOOT.to_string())
        );
        let lever = run.get(at.world_pos(9, -2, 12));
        assert_eq!(lever.property("face"), Some("wall"));
    }

    #[test]
    fn the_moss_is_drawn_for_every_block_whichever_chunk_is_placed() {
        // Settled already, so that a chunk away from the temple places it too.
        let state = PieceState {
            shift_y: 0,
            settled: true,
            flags: 0,
        };
        let piece = temple(Direction::East);
        let mut away = Run::new(World::flat(blocks::GRASS_BLOCK, 60), 9);
        away.place(chunk(-1, -1), |p| place(p, &piece, state));
        let mut whole = Run::new(World::flat(blocks::GRASS_BLOCK, 60), 9);
        let placed = whole.place(area(), |p| place(p, &piece, state));
        assert_eq!(placed.flags, 0b1111);
        assert!(away.world.world.writes.is_empty());
        // The whole temple draws the same moss and a seed for each of its four
        // containers: one step of the source each.
        for _ in 0..4 {
            away.random.next_i64();
        }
        assert_eq!(away.random.next_i64(), whole.random.next_i64());
    }
}
