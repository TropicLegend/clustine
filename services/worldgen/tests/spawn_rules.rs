//! The rules by which a new world's spawn is found, written from section 2 of
//! `docs/groundwork/t8-plan.md` ("A place to stand") and not from the code: the square
//! spiral of 121 chunks from the origin chunk, the columns of a chunk x outer and z
//! inner, the column rule of `PlayerSpawnFinder.getLevelRespawnPos`, and the origin
//! chunk's block 8, 64, 8 where no chunk has a column to stand on.

use std::collections::BTreeMap;

use clustine_data::{BlockState, blocks};
use clustine_world::{Biome, BlockPos, Chunk, ChunkPos, Section};
use clustine_worldgen::spawn::{
    FALLBACK_HEIGHT, SPIRAL_CHUNKS, feet, in_chunk, place_to_stand, spiral,
};

/// The overworld's bottom and its number of sections.
const MIN_Y: i32 = -64;
const SECTIONS: usize = 24;

/// Where the sea's surface is in the chunks made here, and the sea floor below it.
const SEA_TOP: i32 = 62;
const SEA_FLOOR: i32 = 40;

fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap_or_else(|| panic!("no block state {text}"))
}

fn air() -> Chunk {
    Chunk::from_sections(
        MIN_Y,
        vec![Section::filled(blocks::AIR, Biome(0)); SECTIONS],
    )
}

/// Fills the column from the bottom of the world up to `top` inclusive.
fn pile(chunk: &mut Chunk, x: usize, z: usize, top: i32, with: BlockState) {
    for y in MIN_Y..=top {
        chunk.set(x, y, z, with);
    }
}

/// A column of stone up to the sea floor with water above it up to the sea's top:
/// the top is water, so no column of it can be stood on.
fn sea_column(chunk: &mut Chunk, x: usize, z: usize) {
    pile(chunk, x, z, SEA_FLOOR, blocks::STONE);
    for y in SEA_FLOOR + 1..=SEA_TOP {
        chunk.set(x, y, z, blocks::WATER);
    }
}

fn sea() -> Chunk {
    let mut chunk = air();
    for x in 0..16 {
        for z in 0..16 {
            sea_column(&mut chunk, x, z);
        }
    }
    chunk
}

/// Makes the column dry ground of stone up to `top`.
fn dry_column(chunk: &mut Chunk, x: usize, z: usize, top: i32) {
    for y in MIN_Y..=SEA_TOP {
        chunk.set(x, y, z, blocks::AIR);
    }
    pile(chunk, x, z, top, blocks::STONE);
}

// --- The spiral ------------------------------------------------------------------------

/// The spiral as section 2 gives it: begin at (0, 0) heading (0, -1); after each chunk,
/// turn when `x == z`, or `x < 0 && x == -z`, or `x > 0 && x == 1 - z`, the turn being
/// `(dx, dz) = (-dz, dx)`; then step.
fn spiral_model() -> Vec<(i32, i32)> {
    let (mut x, mut z) = (0i32, 0i32);
    let (mut dx, mut dz) = (0i32, -1i32);
    let mut offsets = Vec::new();
    for _ in 0..121 {
        offsets.push((x, z));
        if x == z || (x < 0 && x == -z) || (x > 0 && x == 1 - z) {
            (dx, dz) = (-dz, dx);
        }
        x += dx;
        z += dz;
    }
    offsets
}

#[test]
fn the_spiral_takes_the_121_chunks_in_the_order_of_the_game() {
    let model = spiral_model();
    // The beginning the plan spells out, so that the model is checked as well.
    assert_eq!(
        model[..10],
        [
            (0, 0),
            (1, 0),
            (1, 1),
            (0, 1),
            (-1, 1),
            (-1, 0),
            (-1, -1),
            (0, -1),
            (1, -1),
            (2, -1)
        ]
    );
    assert_eq!(SPIRAL_CHUNKS, 121);
    let offsets: Vec<(i32, i32)> = spiral().collect();
    assert_eq!(offsets, model);
}

#[test]
fn the_spiral_covers_the_square_of_eleven_chunks_once_each() {
    let mut offsets: Vec<(i32, i32)> = spiral().collect();
    offsets.sort_unstable();
    let mut square: Vec<(i32, i32)> = (-5..=5)
        .flat_map(|x| (-5..=5).map(move |z| (x, z)))
        .collect();
    square.sort_unstable();
    assert_eq!(offsets, square);
}

// --- The column rule -------------------------------------------------------------------

#[test]
fn dry_ground_gives_the_block_above_it() {
    let mut chunk = air();
    pile(&mut chunk, 3, 7, 70, blocks::STONE);
    assert_eq!(feet(&chunk, 3, 7), Some(71));
}

#[test]
fn a_column_whose_top_is_water_is_refused() {
    let mut chunk = air();
    sea_column(&mut chunk, 0, 0);
    assert_eq!(feet(&chunk, 0, 0), None);
}

#[test]
fn a_column_whose_top_is_lava_is_refused() {
    let mut chunk = air();
    pile(&mut chunk, 0, 0, 60, blocks::STONE);
    chunk.set(0, 61, 0, blocks::LAVA);
    assert_eq!(feet(&chunk, 0, 0), None);
}

#[test]
fn a_column_with_a_fluid_on_the_way_down_before_the_ground_is_refused() {
    // Stone, water above it, and a bottom slab on the water. The slab blocks motion,
    // so the top is no fluid and the first test lets the column through; the scan
    // from above the slab passes the slab, whose top is not full, and meets the water
    // before any ground.
    let mut chunk = air();
    pile(&mut chunk, 5, 5, 60, blocks::STONE);
    chunk.set(5, 61, 5, blocks::WATER);
    chunk.set(5, 62, 5, state("minecraft:oak_slab[type=bottom]"));
    assert_eq!(feet(&chunk, 5, 5), None);
}

#[test]
fn a_fluid_below_the_ground_does_not_matter() {
    // The scan stops at the first full top, which comes before the water.
    let mut chunk = air();
    pile(&mut chunk, 5, 5, 50, blocks::STONE);
    chunk.set(5, 51, 5, blocks::WATER);
    chunk.set(5, 52, 5, blocks::STONE);
    assert_eq!(feet(&chunk, 5, 5), Some(53));
}

#[test]
fn a_waterlogged_top_slab_is_refused_though_its_top_is_full() {
    // The fluid is asked about before the face, so a full top holding water refuses
    // the column.
    let mut chunk = air();
    pile(&mut chunk, 1, 2, 60, blocks::STONE);
    chunk.set(
        1,
        61,
        2,
        state("minecraft:oak_slab[type=top,waterlogged=true]"),
    );
    assert_eq!(feet(&chunk, 1, 2), None);
}

#[test]
fn waterlogged_leaves_are_refused() {
    let mut chunk = air();
    pile(&mut chunk, 1, 2, 60, blocks::STONE);
    chunk.set(1, 61, 2, state("minecraft:oak_leaves[waterlogged=true]"));
    assert_eq!(feet(&chunk, 1, 2), None);
}

#[test]
fn a_top_slab_gives_a_place_on_top_of_it() {
    let mut chunk = air();
    pile(&mut chunk, 1, 2, 60, blocks::STONE);
    chunk.set(1, 61, 2, state("minecraft:oak_slab[type=top]"));
    assert_eq!(feet(&chunk, 1, 2), Some(62));
}

#[test]
fn ice_over_water_gives_a_place_on_top_of_it() {
    let mut chunk = air();
    sea_column(&mut chunk, 4, 9);
    chunk.set(4, SEA_TOP, 9, blocks::ICE);
    assert_eq!(feet(&chunk, 4, 9), Some(SEA_TOP + 1));
}

#[test]
fn leaves_over_water_give_a_place_on_top_of_them() {
    // Leaves block motion, so the ocean floor's map counts them and the top is not
    // taken for a fluid.
    let mut chunk = air();
    sea_column(&mut chunk, 4, 9);
    chunk.set(4, SEA_TOP + 1, 9, blocks::OAK_LEAVES);
    assert_eq!(feet(&chunk, 4, 9), Some(SEA_TOP + 2));
}

#[test]
fn leaves_over_air_give_a_place_on_the_leaves() {
    // A canopy: the scan begins above the highest block that blocks motion.
    let mut chunk = air();
    pile(&mut chunk, 8, 8, 64, blocks::GRASS_BLOCK);
    for y in 65..70 {
        chunk.set(8, y, 8, state("minecraft:oak_log"));
    }
    chunk.set(8, 70, 8, blocks::OAK_LEAVES);
    assert_eq!(feet(&chunk, 8, 8), Some(71));
}

#[test]
fn a_bottom_slab_is_passed_and_the_feet_are_in_it() {
    // The slab's top is not full; the scan goes on down to the stone under it and the
    // spawn is the block above the stone, which is the slab's.
    let mut chunk = air();
    pile(&mut chunk, 6, 0, 60, blocks::STONE);
    chunk.set(6, 61, 0, state("minecraft:oak_slab[type=bottom]"));
    assert_eq!(feet(&chunk, 6, 0), Some(61));
}

#[test]
fn a_bottom_slab_over_air_is_passed_down_to_the_ground() {
    // Under the slab is air; the scan, finding no fluid, goes on to the stone.
    let mut chunk = air();
    pile(&mut chunk, 6, 0, 55, blocks::STONE);
    chunk.set(6, 61, 0, state("minecraft:oak_slab[type=bottom]"));
    assert_eq!(feet(&chunk, 6, 0), Some(56));
}

#[test]
fn a_plant_on_the_ground_holds_the_feet() {
    // Grass neither blocks motion nor holds a fluid: the column's height is the
    // ground's, and the scan begins in the grass.
    let mut chunk = air();
    pile(&mut chunk, 2, 14, 66, blocks::GRASS_BLOCK);
    chunk.set(2, 67, 14, blocks::SHORT_GRASS);
    assert_eq!(feet(&chunk, 2, 14), Some(67));
}

#[test]
fn an_empty_column_is_refused() {
    let chunk = air();
    assert_eq!(feet(&chunk, 0, 0), None);
    assert_eq!(feet(&chunk, 15, 15), None);
}

#[test]
fn ground_at_the_bottom_of_the_world_is_stood_on() {
    let mut chunk = air();
    chunk.set(9, MIN_Y, 9, blocks::STONE);
    assert_eq!(feet(&chunk, 9, 9), Some(MIN_Y + 1));
}

// --- Within a chunk ---------------------------------------------------------------------

#[test]
fn within_a_chunk_the_first_column_x_outer_and_z_inner_wins() {
    let mut chunk = sea();
    dry_column(&mut chunk, 3, 0, 70);
    dry_column(&mut chunk, 0, 5, 65);
    let position = ChunkPos::new(-3, 7);
    assert_eq!(
        in_chunk(&chunk, position),
        Some(BlockPos::new(-48, 66, 112 + 5))
    );
}

#[test]
fn within_a_chunk_a_later_z_of_the_same_x_loses_to_an_earlier_one() {
    let mut chunk = sea();
    dry_column(&mut chunk, 7, 15, 70);
    dry_column(&mut chunk, 7, 2, 80);
    dry_column(&mut chunk, 8, 0, 60);
    assert_eq!(
        in_chunk(&chunk, ChunkPos::new(0, 0)),
        Some(BlockPos::new(7, 81, 2))
    );
}

#[test]
fn a_chunk_of_sea_has_no_column_to_stand_on() {
    assert_eq!(in_chunk(&sea(), ChunkPos::new(2, 2)), None);
}

// --- Across chunks ----------------------------------------------------------------------

/// The spawn over a world of sea in which only `dry` chunks have one dry column each,
/// with the positions asked for in turn.
fn spawn_over(
    origin: ChunkPos,
    dry: &BTreeMap<ChunkPos, (usize, usize, i32)>,
) -> (BlockPos, Vec<ChunkPos>) {
    let sea = sea();
    let chunks: BTreeMap<ChunkPos, Chunk> = dry
        .iter()
        .map(|(position, &(x, z, top))| {
            let mut chunk = sea.clone();
            dry_column(&mut chunk, x, z, top);
            (*position, chunk)
        })
        .collect();
    let mut asked = Vec::new();
    let spawn = place_to_stand(origin, |position| {
        asked.push(position);
        chunks.get(&position).unwrap_or(&sea)
    });
    (spawn, asked)
}

#[test]
fn across_chunks_the_first_chunk_of_the_spiral_with_a_dry_column_wins() {
    let origin = ChunkPos::new(-20, 33);
    let model = spiral_model();
    let at = |index: usize| ChunkPos::new(origin.x + model[index].0, origin.z + model[index].1);
    // The 30th of the spiral has a column in its north-west corner, the 6th one far
    // into the chunk: the 6th still wins, as chunks come before columns.
    let dry = BTreeMap::from([(at(30), (0, 0, 90)), (at(6), (12, 13, 70))]);
    let (spawn, asked) = spawn_over(origin, &dry);
    let winner = at(6);
    assert_eq!(
        spawn,
        BlockPos::new(winner.x * 16 + 12, 71, winner.z * 16 + 13)
    );
    // The chunks are asked for in the spiral's order, and none after the one that has
    // a column.
    let expected: Vec<ChunkPos> = (0..=6).map(at).collect();
    assert_eq!(asked, expected);
}

#[test]
fn the_origin_chunk_wins_where_it_has_a_dry_column() {
    let origin = ChunkPos::new(5, -9);
    let dry = BTreeMap::from([(origin, (15, 15, 63)), (ChunkPos::new(6, -9), (0, 0, 63))]);
    let (spawn, asked) = spawn_over(origin, &dry);
    assert_eq!(spawn, BlockPos::new(5 * 16 + 15, 64, -9 * 16 + 15));
    assert_eq!(asked, vec![origin]);
}

#[test]
fn the_last_chunk_of_the_spiral_is_still_searched() {
    let origin = ChunkPos::new(0, 0);
    let last = spiral_model()[120];
    let position = ChunkPos::new(last.0, last.1);
    let dry = BTreeMap::from([(position, (2, 3, 100))]);
    let (spawn, asked) = spawn_over(origin, &dry);
    assert_eq!(
        spawn,
        BlockPos::new(position.x * 16 + 2, 101, position.z * 16 + 3)
    );
    assert_eq!(asked.len(), 121);
}

#[test]
fn a_dry_chunk_just_outside_the_spiral_is_not_found() {
    // Six chunks out is beyond the square of eleven, so the fallback is taken.
    let origin = ChunkPos::new(10, 10);
    let dry = BTreeMap::from([(ChunkPos::new(16, 10), (0, 0, 70))]);
    let (spawn, _) = spawn_over(origin, &dry);
    assert_eq!(
        spawn,
        BlockPos::new(10 * 16 + 8, FALLBACK_HEIGHT, 10 * 16 + 8)
    );
}

// --- The fallback -----------------------------------------------------------------------

#[test]
fn a_world_without_a_dry_column_in_121_chunks_gives_the_origin_chunks_block_8_64_8() {
    assert_eq!(FALLBACK_HEIGHT, 64);
    let origin = ChunkPos::new(-3, -11);
    let (spawn, asked) = spawn_over(origin, &BTreeMap::new());
    assert_eq!(spawn, BlockPos::new(-3 * 16 + 8, 64, -11 * 16 + 8));
    // Every chunk of the spiral was tried, each once, in order.
    let expected: Vec<ChunkPos> = spiral_model()
        .into_iter()
        .map(|(x, z)| ChunkPos::new(origin.x + x, origin.z + z))
        .collect();
    assert_eq!(asked, expected);
}

#[test]
fn a_world_of_air_gives_the_fallback_too() {
    let empty = air();
    let origin = ChunkPos::new(7, 0);
    let spawn = place_to_stand(origin, |_| &empty);
    assert_eq!(spawn, BlockPos::new(7 * 16 + 8, 64, 8));
}
