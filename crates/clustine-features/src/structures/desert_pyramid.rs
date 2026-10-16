// Adapted from SteelMC's `steel-core/src/worldgen/structure/piece_placer/desert_pyramid.rs`
// (see NOTICE in this crate), which follows the game's `DesertPyramidPiece` and
// `DesertPyramidStructure.afterPlace`.

//! The desert pyramid (`DesertPyramidPiece`): the pyramid of sandstone with its towers,
//! the hall's floor of terracotta, the trap of TNT under a pressure plate, the four
//! chests around it, and the cellar under the north-east corner with its collapsed
//! roof; then the start's `afterPlace`, which turns some of the cellar's sand into
//! suspicious sand.
//!
//! The piece settles on the lowest ground under it, less a random 0 to 2, the first
//! time a chunk places it; the offset is drawn every time, as the game draws it before
//! it asks whether the height is set.
//!
//! The game's piece notes, as it places the cellar, every position that may become
//! suspicious sand and one position of the collapsed roof, and `afterPlace` reads the
//! notes. The notes are on the piece object, which every chunk shares; they are not
//! saved with it, and each placing adds the whole list again whichever chunk it is in
//! (`placeSand` writes no block). `afterPlace` collects them into a sorted set, and the
//! roof's position is drawn from the world's seed at a corner of the roof. So what
//! `afterPlace` sees is a function of the piece's settled box alone, and is worked out
//! here from that box rather than carried from the placing: every chunk that refers to
//! the start has the piece placed in it first, since the start's box is the piece's.

use clustine_data::{BlockState, blocks};
use clustine_noise::Random;
use clustine_structures::Start;
use clustine_structures::bounding_box_of;
use clustine_structures::geometry::Direction;
use clustine_structures::piece::{ProceduralPieceData, StructurePiece, StructurePiecePayload};
use clustine_world::generation::{PieceKey, PieceState};
use clustine_world::{BlockPos, ChunkPos};

use super::Placing;
use super::scattered::{
    Piece, inside, loot_data, pack, region_random, seed_random_at, settle_on_lowest, settled_box,
    with,
};
use crate::features::ground::monster_room::keeps_entity_of;

const LOOT: &str = "minecraft:chests/desert_pyramid";
const ARCHAEOLOGY: &str = "minecraft:archaeology/desert_pyramid";

/// The corner of the collapsed roof, in the piece's frame, and its far corner's x and
/// z: the roof's position is drawn from the world's seed at the corner.
const ROOF_FROM: [i32; 3] = [14, 0, 11];
const ROOF_TO: [i32; 2] = [18, 15];

fn stairs(facing: &str) -> BlockState {
    with(blocks::SANDSTONE_STAIRS, "facing", facing)
}

/// `DesertPyramidPiece.postProcess`. Returns the piece's state: where it settled, and
/// which chests it has placed (bit `get2DDataValue` of the chest's direction).
pub(super) fn place(
    placing: &mut Placing<'_, '_, '_>,
    piece: &StructurePiece,
    state: PieceState,
) -> PieceState {
    let offset = -placing.random.next_i32_bounded(3);
    let Some(mut state) = settle_on_lowest(placing, piece.bounding_box, state, offset) else {
        return state;
    };
    let at = Piece::new(settled_box(piece.bounding_box, state), piece.orientation);
    let p = placing;

    let sandstone = blocks::SANDSTONE;
    let air = blocks::AIR;
    let cut = blocks::CUT_SANDSTONE;
    let chiseled = blocks::CHISELED_SANDSTONE;
    let orange = blocks::ORANGE_TERRACOTTA;
    let blue = blocks::BLUE_TERRACOTTA;

    at.fill(p, [0, -4, 0], [20, 0, 20], sandstone);
    for i in 1..=9 {
        at.fill(p, [i, i, i], [20 - i, i, 20 - i], sandstone);
        at.fill(p, [i + 1, i, i + 1], [19 - i, i, 19 - i], air);
    }
    for x in 0..21 {
        for z in 0..21 {
            at.fill_column_down(p, sandstone, x, -5, z);
        }
    }

    let (north, south, east, west) = (
        stairs("north"),
        stairs("south"),
        stairs("east"),
        stairs("west"),
    );
    at.generate_box(p, [0, 0, 0], [4, 9, 4], sandstone, air, false);
    at.fill(p, [1, 10, 1], [3, 10, 3], sandstone);
    at.place_block(p, north, 2, 10, 0);
    at.place_block(p, south, 2, 10, 4);
    at.place_block(p, east, 0, 10, 2);
    at.place_block(p, west, 4, 10, 2);
    at.generate_box(p, [16, 0, 0], [20, 9, 4], sandstone, air, false);
    at.fill(p, [17, 10, 1], [19, 10, 3], sandstone);
    at.place_block(p, north, 18, 10, 0);
    at.place_block(p, south, 18, 10, 4);
    at.place_block(p, east, 16, 10, 2);
    at.place_block(p, west, 20, 10, 2);
    at.generate_box(p, [8, 0, 0], [12, 4, 4], sandstone, air, false);
    at.fill(p, [9, 1, 0], [11, 3, 4], air);
    for (x, y) in [(9, 1), (9, 2), (9, 3), (10, 3), (11, 3), (11, 2), (11, 1)] {
        at.place_block(p, cut, x, y, 1);
    }
    at.generate_box(p, [4, 1, 1], [8, 3, 3], sandstone, air, false);
    at.fill(p, [4, 1, 2], [8, 2, 2], air);
    at.generate_box(p, [12, 1, 1], [16, 3, 3], sandstone, air, false);
    at.fill(p, [12, 1, 2], [16, 2, 2], air);
    at.fill(p, [5, 4, 5], [15, 4, 15], sandstone);
    at.fill(p, [9, 4, 9], [11, 4, 11], air);
    at.fill(p, [8, 1, 8], [8, 3, 8], cut);
    at.fill(p, [12, 1, 8], [12, 3, 8], cut);
    at.fill(p, [8, 1, 12], [8, 3, 12], cut);
    at.fill(p, [12, 1, 12], [12, 3, 12], cut);
    at.fill(p, [1, 1, 5], [4, 4, 11], sandstone);
    at.fill(p, [16, 1, 5], [19, 4, 11], sandstone);
    at.fill(p, [6, 7, 9], [6, 7, 11], sandstone);
    at.fill(p, [14, 7, 9], [14, 7, 11], sandstone);
    at.fill(p, [5, 5, 9], [5, 7, 11], cut);
    at.fill(p, [15, 5, 9], [15, 7, 11], cut);
    at.place_block(p, air, 5, 5, 10);
    at.place_block(p, air, 5, 6, 10);
    at.place_block(p, air, 6, 6, 10);
    at.place_block(p, air, 15, 5, 10);
    at.place_block(p, air, 15, 6, 10);
    at.place_block(p, air, 14, 6, 10);
    at.fill(p, [2, 4, 4], [2, 6, 4], air);
    at.fill(p, [18, 4, 4], [18, 6, 4], air);
    at.place_block(p, north, 2, 4, 5);
    at.place_block(p, north, 2, 3, 4);
    at.place_block(p, north, 18, 4, 5);
    at.place_block(p, north, 18, 3, 4);
    at.fill(p, [1, 1, 3], [2, 2, 3], sandstone);
    at.fill(p, [18, 1, 3], [19, 2, 3], sandstone);
    at.place_block(p, sandstone, 1, 1, 2);
    at.place_block(p, sandstone, 19, 1, 2);
    at.place_block(p, blocks::SANDSTONE_SLAB, 1, 2, 2);
    at.place_block(p, blocks::SANDSTONE_SLAB, 19, 2, 2);
    at.place_block(p, west, 2, 1, 2);
    at.place_block(p, east, 18, 1, 2);
    at.fill(p, [4, 3, 5], [4, 3, 17], sandstone);
    at.fill(p, [16, 3, 5], [16, 3, 17], sandstone);
    at.fill(p, [3, 1, 5], [4, 2, 16], air);
    at.fill(p, [15, 1, 5], [16, 2, 16], air);
    for z in (5..=17).step_by(2) {
        at.place_block(p, cut, 4, 1, z);
        at.place_block(p, chiseled, 4, 2, z);
        at.place_block(p, cut, 16, 1, z);
        at.place_block(p, chiseled, 16, 2, z);
    }

    for (x, z) in [
        (10, 7),
        (10, 8),
        (9, 9),
        (11, 9),
        (8, 10),
        (12, 10),
        (7, 10),
        (13, 10),
        (9, 11),
        (11, 11),
        (10, 12),
        (10, 13),
    ] {
        at.place_block(p, orange, x, 0, z);
    }
    at.place_block(p, blue, 10, 0, 10);

    // The fronts of the two towers, each as a column of three blocks across.
    let tower = [
        [cut, orange, cut],
        [cut, orange, cut],
        [orange, chiseled, orange],
        [cut, orange, cut],
        [orange, chiseled, orange],
        [orange, orange, orange],
        [cut, cut, cut],
    ];
    for x in [0, 20] {
        for (row, y) in tower.iter().zip(2..) {
            for (state, z) in row.iter().zip(1..) {
                at.place_block(p, *state, x, y, z);
            }
        }
    }
    for x in [2, 18] {
        for (row, y) in tower.iter().zip(2..) {
            for (state, dx) in row.iter().zip(-1..) {
                at.place_block(p, *state, x + dx, y, 0);
            }
        }
    }

    at.fill(p, [8, 4, 0], [12, 6, 0], cut);
    at.place_block(p, air, 8, 6, 0);
    at.place_block(p, air, 12, 6, 0);
    at.place_block(p, orange, 9, 5, 0);
    at.place_block(p, chiseled, 10, 5, 0);
    at.place_block(p, orange, 11, 5, 0);
    at.fill(p, [8, -14, 8], [12, -11, 12], cut);
    at.fill(p, [8, -10, 8], [12, -10, 12], chiseled);
    at.fill(p, [8, -9, 8], [12, -9, 12], cut);
    at.fill(p, [8, -8, 8], [12, -1, 12], sandstone);
    at.fill(p, [9, -11, 9], [11, -1, 11], air);
    at.place_block(p, blocks::STONE_PRESSURE_PLATE, 10, -11, 10);
    at.generate_box(p, [9, -13, 9], [11, -13, 11], blocks::TNT, air, false);
    at.place_block(p, air, 8, -11, 10);
    at.place_block(p, air, 8, -10, 10);
    at.place_block(p, chiseled, 7, -10, 10);
    at.place_block(p, cut, 7, -11, 10);
    at.place_block(p, air, 12, -11, 10);
    at.place_block(p, air, 12, -10, 10);
    at.place_block(p, chiseled, 13, -10, 10);
    at.place_block(p, cut, 13, -11, 10);
    at.place_block(p, air, 10, -11, 8);
    at.place_block(p, air, 10, -10, 8);
    at.place_block(p, chiseled, 10, -10, 7);
    at.place_block(p, cut, 10, -11, 7);
    at.place_block(p, air, 10, -11, 12);
    at.place_block(p, air, 10, -10, 12);
    at.place_block(p, chiseled, 10, -10, 13);
    at.place_block(p, cut, 10, -11, 13);

    for direction in Direction::HORIZONTAL {
        let bit = 1 << direction.get_2d_data_value();
        if state.flags & bit == 0 {
            let (dx, dz) = direction.offset_xz();
            if at.create_chest(p, 10 + dx * 2, -11, 10 + dz * 2, LOOT) {
                state.flags |= bit;
            }
        }
    }

    add_cellar(p, &at);
    state
}

/// `addCellar`: the stairs down and the room, whose sand the region's random source
/// chooses.
fn add_cellar(p: &mut Placing<'_, '_, '_>, at: &Piece) {
    let (sand, sandstone) = (blocks::SAND, blocks::SANDSTONE);
    let (cut, chiseled) = (blocks::CUT_SANDSTONE, blocks::CHISELED_SANDSTONE);
    let (orange, blue) = (blocks::ORANGE_TERRACOTTA, blocks::BLUE_TERRACOTTA);

    let west = stairs("west");
    at.place_block(p, west, 13, -1, 17);
    at.place_block(p, west, 14, -2, 17);
    at.place_block(p, west, 15, -3, 17);
    let variant = region_random(p).next_bool();
    for x in 12..=16 {
        at.place_block(p, sand, x, 0, 17);
    }
    at.place_block(p, sand, 14, -1, 17);
    at.place_block(p, if variant { sand } else { sandstone }, 15, -1, 17);
    at.place_block(p, if variant { sandstone } else { sand }, 16, -1, 17);
    at.place_block(p, sand, 15, -2, 17);
    at.place_block(p, sandstone, 16, -2, 17);
    at.place_block(p, sand, 16, -3, 17);

    for (y, state) in [(-3, cut), (-2, chiseled), (-1, cut)] {
        at.generate_box(p, [13, y, 10], [13, y, 15], state, state, true);
        at.generate_box(p, [19, y, 10], [19, y, 15], state, state, true);
        at.generate_box(p, [13, y, 10], [19, y, 11], state, state, true);
        at.generate_box(p, [13, y, 16], [19, y, 16], state, state, true);
    }
    // The sand of the room itself is left to `afterPlace`.
    let [x0, y0, z0] = ROOF_FROM;
    let [x1, z1] = ROOF_TO;
    for x in x0..=x1 {
        for z in z0..=z1 {
            let state = if region_random(p).next_f32() < 0.33 {
                sandstone
            } else {
                sand
            };
            at.place_block(p, state, x, y0, z);
        }
    }
    at.place_block(p, blue, 16, -4, 13);
    for (x, z) in [
        (17, 12),
        (17, 14),
        (15, 12),
        (15, 14),
        (18, 13),
        (14, 13),
        (16, 15),
        (16, 11),
        (19, 13),
    ] {
        at.place_block(p, orange, x, -4, z);
    }
    at.place_block(p, cut, 20, -3, 13);
    at.place_block(p, chiseled, 20, -2, 13);
    at.place_block(p, orange, 13, -4, 13);
    at.place_block(p, cut, 12, -3, 13);
    at.place_block(p, chiseled, 12, -2, 13);
    at.place_block(p, orange, 16, -4, 16);
    at.place_block(p, orange, 16, -4, 10);
    at.place_block(p, cut, 16, -3, 9);
    at.place_block(p, chiseled, 16, -2, 9);
}

/// The positions the game's piece notes as possible suspicious sand
/// (`potentialSuspiciousSandWorldPositions`), in the order it notes them.
fn noted_sand(at: &Piece) -> Vec<BlockPos> {
    let mut noted = Vec::new();
    // `placeSandBox(14, -3, 11, 18, -1, 15)`.
    for y in -3..=-1 {
        for x in 14..=18 {
            for z in 11..=15 {
                noted.push(at.world_pos(x, y, z));
            }
        }
    }
    for (x, y, z) in [
        (19, -3, 13),
        (19, -2, 13),
        (13, -3, 13),
        (13, -2, 13),
        (16, -3, 16),
        (16, -2, 16),
        (16, -3, 10),
        (16, -2, 10),
    ] {
        noted.push(at.world_pos(x, y, z));
    }
    noted
}

/// `randomCollapsedRoofPos`: a column of the roof drawn from the world's seed at the
/// roof's corner.
fn collapsed_roof(placing: &Placing<'_, '_, '_>, at: &Piece) -> BlockPos {
    let [x0, y0, z0] = ROOF_FROM;
    let [x1, z1] = ROOF_TO;
    let mut random = seed_random_at(placing, at.world_pos(x0, y0, z0));
    let x = random.next_i32_between(x0, x1);
    let z = random.next_i32_between(z0, z1);
    at.world_pos(x, y0, z)
}

/// `DesertPyramidStructure.afterPlace`, with the state of the start's one piece.
pub(super) fn after_place(placing: &mut Placing<'_, '_, '_>, start: &Start) {
    let states: Vec<PieceState> = (0..start.pieces.len())
        .map(|piece| {
            let piece = u16::try_from(piece).expect("a start has fewer than 65,536 pieces");
            piece_state(placing, start, piece)
        })
        .collect();
    after_place_settled(placing, start, &states);
}

/// `afterPlace` once the pieces' states are known: of each pyramid piece (the game
/// asks `instanceof DesertPyramidPiece`), the roof's position becomes suspicious sand;
/// then the noted positions, shuffled by a source of the world's seed at the middle of
/// the start's pieces, become suspicious sand for the first five to seven and sand for
/// the rest, each only in the centre chunk.
pub(super) fn after_place_settled(
    placing: &mut Placing<'_, '_, '_>,
    start: &Start,
    states: &[PieceState],
) {
    let mut noted = Vec::new();
    let mut boxes = Vec::new();
    for (piece, state) in start.pieces.iter().zip(states) {
        let mut moved = piece.clone();
        moved.bounding_box = settled_box(piece.bounding_box, *state);
        if let StructurePiecePayload::Procedural(ProceduralPieceData::DesertPyramid(_)) =
            piece.payload
        {
            let at = Piece::new(moved.bounding_box, piece.orientation);
            noted.extend(noted_sand(&at));
            let roof = collapsed_roof(placing, &at);
            place_suspicious_sand(placing, roof);
        }
        boxes.push(moved);
    }
    // A `SortedArraySet` by `Vec3i.compareTo`: y, then z, then x.
    noted.sort_by_key(|pos| (pos.y, pos.z, pos.x));
    noted.dedup();

    let Some(bounds) = bounding_box_of(&boxes, 0) else {
        return;
    };
    let centre = bounds.center();
    let mut random = seed_random_at(placing, BlockPos::new(centre.x, centre.y, centre.z));
    // `Util.shuffle`.
    for i in (1..noted.len()).rev() {
        let bound = i32::try_from(i + 1).expect("the noted sand is a few dozen positions");
        let j = random.next_i32_bounded(bound) as usize;
        noted.swap(i, j);
    }
    let mut suspicious = noted
        .len()
        .min(random.next_i32_between_exclusive(5, 8) as usize);
    for pos in noted {
        if suspicious > 0 {
            suspicious -= 1;
            place_suspicious_sand(placing, pos);
        } else if inside(placing.chunk_box, pos) {
            placing.level.set_block(pos, blocks::SAND);
        }
    }
}

/// `placeSuspiciousSand`: its loot is the pyramid's, seeded by its position.
fn place_suspicious_sand(placing: &mut Placing<'_, '_, '_>, pos: BlockPos) {
    if !inside(placing.chunk_box, pos) {
        return;
    }
    placing.level.set_block(pos, blocks::SUSPICIOUS_SAND);
    if keeps_entity_of(placing.level, pos, blocks::SUSPICIOUS_SAND) {
        let data = loot_data("minecraft:brushable_block", ARCHAEOLOGY, pack(pos));
        placing.level.set_block_entity(pos, Some(data));
    }
}

/// The state of a piece of `start` as the run keeps it: the start is found among the
/// centre's references by its chunk and its structure, of which a chunk starts one at
/// most.
fn piece_state(placing: &mut Placing<'_, '_, '_>, start: &Start, piece: u16) -> PieceState {
    let Some(starts_of) = placing.settings.structures() else {
        return PieceState::default();
    };
    let source = ChunkPos::new(start.chunk.0, start.chunk.1);
    let view = placing.level.view();
    let reference = view.starts().iter().copied().find(|reference| {
        reference.source == source
            && starts_of.starts(source)[usize::from(reference.index)].structure == start.structure
    });
    match reference {
        Some(start) => view.piece(PieceKey { start, piece }),
        None => PieceState::default(),
    }
}

#[cfg(test)]
mod tests {
    use clustine_protocol::codec::Reader;
    use clustine_protocol::nbt::Nbt;
    use clustine_structures::desert_pyramid::DesertPyramidPieceData;
    use clustine_structures::geometry::{BoundingBox, IVec3};
    use clustine_worldgen_data::structures::STRUCTURES;

    use std::collections::BTreeMap;
    use std::sync::Arc;

    use clustine_noise::WorldgenRandom;
    use clustine_terrain::biome::Dimension;
    use clustine_world::generation::{
        FinalHeightmap, Neighbourhood, Placing as Attempt, StartRef, WorldgenHeightmap,
    };
    use clustine_world::{Biome, BlockEntity, ScheduledTick};

    use super::*;
    use crate::FeatureSettings;
    use crate::context::Level;
    use crate::features::ground::shared::testing::WithEntities;
    use crate::structures::scattered::testing::{Run, area, chunk, fresh_region_random, piece};
    use crate::testing::World;

    fn pyramid(orientation: Direction) -> StructurePiece {
        piece(
            BoundingBox::new(IVec3::new(0, 64, 0), IVec3::new(20, 78, 20)),
            orientation,
            StructurePiecePayload::Procedural(ProceduralPieceData::DesertPyramid(
                DesertPyramidPieceData::new(),
            )),
        )
    }

    fn start(piece: StructurePiece) -> Start {
        let structure = STRUCTURES
            .iter()
            .position(|structure| structure.name == "minecraft:desert_pyramid")
            .expect("the data has the desert pyramid");
        Start {
            structure,
            chunk: (0, 0),
            bounding_box: piece.bounding_box,
            pieces: vec![piece],
        }
    }

    fn nbt(entity: &clustine_world::BlockEntity) -> Nbt {
        Reader::new(&entity.data)
            .nbt()
            .expect("the data reads")
            .expect("the data is a compound")
    }

    #[test]
    fn the_pyramid_settles_on_its_lowest_ground_less_its_draw_and_keeps_its_chests() {
        let mut run = Run::new(World::flat(blocks::SAND, 70), 3);
        let offset = -run.random.clone().next_i32_bounded(3);
        let piece = pyramid(Direction::North);
        let state = run.place(area(), |p| place(p, &piece, PieceState::default()));
        assert!(state.settled);
        assert_eq!(state.shift_y, 71 + offset - 64);
        // The four chests, one for each direction's bit.
        assert_eq!(state.flags, 0b1111);
        let at = Piece::new(settled_box(piece.bounding_box, state), piece.orientation);
        for (dx, dz) in [(2, 0), (-2, 0), (0, 2), (0, -2)] {
            let pos = at.world_pos(10 + dx, -11, 10 + dz);
            assert_eq!(run.get(pos).property("type"), Some("single"));
            let data = nbt(run.world.entities.get(&pos).expect("a chest keeps one"));
            assert_eq!(data.get("LootTable"), Some(&Nbt::String(LOOT.to_string())));
        }
        // The trap under the plate: a box one block high is all skin, so all TNT.
        assert_eq!(
            run.get(at.world_pos(10, -11, 10)),
            blocks::STONE_PRESSURE_PLATE
        );
        assert_eq!(run.get(at.world_pos(10, -13, 10)), blocks::TNT);
        assert_eq!(run.get(at.world_pos(9, -13, 9)), blocks::TNT);
        assert_eq!(run.get(at.world_pos(10, 0, 10)), blocks::BLUE_TERRACOTTA);
    }

    #[test]
    fn a_settled_pyramid_places_no_chest_again() {
        let mut run = Run::new(World::flat(blocks::SAND, 70), 3);
        let piece = pyramid(Direction::East);
        let state = run.place(area(), |p| place(p, &piece, PieceState::default()));
        let mut copy = run.random.clone();
        let again = run.place(area(), |p| place(p, &piece, state));
        assert_eq!(again, state);
        // The second placing draws its offset and no seed.
        copy.next_i32_bounded(3);
        assert_eq!(run.random.next_i64(), copy.next_i64());
    }

    #[test]
    fn the_cellar_draws_its_stairs_and_its_roof_from_the_region() {
        let mut run = Run::new(World::flat(blocks::SAND, 70), 3);
        let piece = pyramid(Direction::South);
        let after = run.place(area(), |p| {
            place(p, &piece, PieceState::default());
            region_random(p).next_i64()
        });
        let mut expected = fresh_region_random();
        expected.next_bool();
        for _ in 0..25 {
            expected.next_f32();
        }
        assert_eq!(after, expected.next_i64());
    }

    #[test]
    fn the_noted_sand_is_eighty_three_positions_of_the_cellar() {
        let piece = pyramid(Direction::North);
        let at = Piece::new(piece.bounding_box, piece.orientation);
        let mut noted = noted_sand(&at);
        noted.sort();
        noted.dedup();
        assert_eq!(noted.len(), 5 * 3 * 5 + 8);
    }

    #[test]
    fn after_place_turns_five_to_seven_of_the_noted_sand_suspicious_and_the_rest_sand() {
        let mut run = Run::new(World::flat(blocks::SAND, 70), 3);
        let piece = pyramid(Direction::West);
        let start = start(piece.clone());
        let state = run.place(area(), |p| place(p, &piece, PieceState::default()));
        run.place(area(), |p| after_place_settled(p, &start, &[state]));
        let at = Piece::new(settled_box(piece.bounding_box, state), piece.orientation);
        let suspicious = noted_sand(&at)
            .into_iter()
            .filter(|pos| run.get(*pos) == blocks::SUSPICIOUS_SAND)
            .count();
        assert!((5..=7).contains(&suspicious), "{suspicious} suspicious");
        for pos in noted_sand(&at) {
            let state = run.get(pos);
            assert!(state == blocks::SAND || state == blocks::SUSPICIOUS_SAND);
            if state == blocks::SUSPICIOUS_SAND {
                let data = nbt(run.world.entities.get(&pos).expect("it keeps one"));
                assert_eq!(
                    data.get("LootTable"),
                    Some(&Nbt::String(ARCHAEOLOGY.to_string()))
                );
                assert_eq!(data.get("LootTableSeed"), Some(&Nbt::Long(pack(pos))));
            }
        }
        // The collapsed roof's one block.
        let roof = run.place(area(), |p| collapsed_roof(p, &at));
        assert_eq!(run.get(roof), blocks::SUSPICIOUS_SAND);
        assert_eq!(roof.y, at.world_pos(0, 0, 0).y);
    }

    #[test]
    fn after_place_writes_only_in_the_centre_chunk() {
        let mut run = Run::new(World::flat(blocks::SAND, 70), 3);
        let piece = pyramid(Direction::North);
        let start = start(piece.clone());
        let state = run.place(area(), |p| place(p, &piece, PieceState::default()));
        let before = run.world.world.writes.len();
        run.place(chunk(-1, -1), |p| after_place_settled(p, &start, &[state]));
        assert_eq!(run.world.world.writes.len(), before);
    }

    /// The toy world, with the references and piece states of a run.
    struct Referring {
        world: WithEntities,
        references: Vec<StartRef>,
        states: BTreeMap<PieceKey, PieceState>,
    }

    impl Neighbourhood for Referring {
        fn centre(&self) -> ChunkPos {
            self.world.centre()
        }
        fn block(&mut self, at: BlockPos) -> BlockState {
            self.world.block(at)
        }
        fn set_block(&mut self, at: BlockPos, state: BlockState) -> bool {
            self.world.set_block(at, state)
        }
        fn block_entity(&mut self, at: BlockPos) -> Option<&BlockEntity> {
            self.world.block_entity(at)
        }
        fn set_block_entity(&mut self, at: BlockPos, entity: Option<BlockEntity>) -> bool {
            self.world.set_block_entity(at, entity)
        }
        fn height(&mut self, map: FinalHeightmap, x: i32, z: i32) -> i32 {
            self.world.height(map, x, z)
        }
        fn worldgen_height(&mut self, map: WorldgenHeightmap, x: i32, z: i32) -> i32 {
            self.world.worldgen_height(map, x, z)
        }
        fn biome(&mut self, at: BlockPos) -> Biome {
            self.world.biome(at)
        }
        fn schedule(&mut self, at: BlockPos, tick: ScheduledTick) {
            self.world.schedule(at, tick);
        }
        fn mark(&mut self, at: BlockPos) {
            self.world.mark(at);
        }
        fn starts(&self) -> &[StartRef] {
            &self.references
        }
        fn piece(&mut self, piece: PieceKey) -> PieceState {
            self.states.get(&piece).copied().unwrap_or_default()
        }
        fn set_piece(&mut self, piece: PieceKey, state: PieceState) {
            self.states.insert(piece, state);
        }
        fn attempt(&mut self, _: Attempt, place: &mut dyn FnMut(&mut dyn Neighbourhood)) -> bool {
            place(self);
            true
        }
    }

    struct Starts(Arc<[Start]>);

    impl crate::structures::StartsOf for Starts {
        fn starts(&self, _: ChunkPos) -> Arc<[Start]> {
            self.0.clone()
        }
    }

    #[test]
    fn after_place_finds_the_pyramid_s_settled_box_by_its_reference() {
        let piece = pyramid(Direction::North);
        let pyramid = start(piece.clone());
        // Another start of the same chunk comes first, so that the reference's index
        // matters.
        let mut other = pyramid.clone();
        other.structure = STRUCTURES
            .iter()
            .position(|structure| structure.name == "minecraft:swamp_hut")
            .expect("the data has the swamp hut");
        let starts: Arc<[Start]> = Arc::from(vec![other, pyramid.clone()]);
        let settings = FeatureSettings::new(13579, Dimension::Overworld)
            .expect("the data is read whole")
            .with_structures(Arc::new(Starts(starts)));
        let source = ChunkPos::new(0, 0);
        let reference = StartRef { source, index: 1 };
        let state = PieceState {
            shift_y: 7,
            settled: true,
            flags: 0b1111,
        };
        let mut view = Referring {
            world: WithEntities::new(World::flat(blocks::SAND, 70)),
            references: vec![StartRef { source, index: 0 }, reference],
            states: BTreeMap::from([(
                PieceKey {
                    start: reference,
                    piece: 0,
                },
                state,
            )]),
        };
        let mut random = WorldgenRandom::from_seed(1);
        let mut level = Level::new(&mut view, settings.zoom(), -64, 384);
        let mut placing = Placing {
            level: &mut level,
            random: &mut random,
            settings: &settings,
            chunk_box: area(),
            centre: source,
            pivot: IVec3::ZERO,
        };
        after_place(&mut placing, &pyramid);
        let at = Piece::new(settled_box(piece.bounding_box, state), piece.orientation);
        let suspicious = noted_sand(&at)
            .into_iter()
            .filter(|pos| view.world.world.get(*pos) == blocks::SUSPICIOUS_SAND)
            .count();
        assert!((5..=7).contains(&suspicious), "{suspicious} suspicious");
    }
}
