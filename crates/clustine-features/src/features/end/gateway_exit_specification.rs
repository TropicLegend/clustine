//! Tests of the search for an End gateway's exit, written from the specification by
//! someone who did not write the search: `docs/groundwork/p-plan.md`, section 19.8 and
//! its review (items 2 and 5), the brief's section 2.6, and the game's bytecode
//! (`d-javap` TheEndGatewayBlockEntity.txt, lines 340-662, and EndIslandFeature.txt).
//!
//! The game's `findOrCreateValidTeleportPos` is written out again here, step by step
//! from the bytecode, over a world of chunks the test chooses ([`game_exit`]); the
//! search, fed the same chunks one at a time, must come to the same exit and island.
//! The island's blocks are those of the game's `EndIslandFeature.place` with
//! `RandomSource.create(pos.asLong())`, a `LegacyRandomSource`, written out here with
//! Java's `Random` ([`JavaRandom`]).

use std::collections::{BTreeMap, BTreeSet};

use clustine_data::{BlockState, blocks};
use clustine_world::{Biome, BlockPos, Chunk, ChunkPos, Dimension};

use super::gateway_exit::{
    GatewayExit, GatewaySearch, Step, find_tallest_block, find_valid_spawn_in_chunk, island_writes,
};

/// The End's lowest block and its height (`DimensionTypes`: min_y 0, height 256).
fn bounds() -> (i32, u32) {
    let rules = Dimension::End.rules();
    (rules.min_y, rules.height)
}

/// The End's top block, `getMaxY()`.
fn max_y() -> i32 {
    let (min_y, height) = bounds();
    min_y + height as i32 - 1
}

fn empty() -> Chunk {
    Chunk::empty(Dimension::End.rules(), Biome(0))
}

/// Java's `java.util.Random`, which `LegacyRandomSource` is for `nextInt`: a 48-bit
/// linear congruential generator, its seed scrambled with the multiplier.
struct JavaRandom(i64);

impl JavaRandom {
    const MULTIPLIER: i64 = 0x5_DEEC_E66D;
    const MASK: i64 = (1 << 48) - 1;

    fn new(seed: i64) -> Self {
        Self((seed ^ Self::MULTIPLIER) & Self::MASK)
    }

    fn next(&mut self, bits: u32) -> i32 {
        self.0 = (self.0.wrapping_mul(Self::MULTIPLIER).wrapping_add(0xB)) & Self::MASK;
        (self.0 >> (48 - bits)) as i32
    }

    fn next_int(&mut self, bound: i32) -> i32 {
        if bound & bound.wrapping_neg() == bound {
            return ((i64::from(bound) * i64::from(self.next(31))) >> 31) as i32;
        }
        loop {
            let bits = self.next(31);
            let value = bits % bound;
            if bits.wrapping_sub(value).wrapping_add(bound - 1) >= 0 {
                return value;
            }
        }
    }
}

/// `BlockPos.asLong`: 26 bits of x at 38, 26 of z at 12, 12 of y at 0.
fn as_long(pos: BlockPos) -> i64 {
    (i64::from(pos.x) & ((1 << 26) - 1)) << 38
        | (i64::from(pos.z) & ((1 << 26) - 1)) << 12
        | (i64::from(pos.y) & ((1 << 12) - 1))
}

/// `EndIslandFeature.place` at `origin` with `RandomSource.create(origin.asLong())`
/// (offsets 0-138): the radius a float from `nextInt(3) + 4`, each layer down from the
/// origin's while it is over 0.5, x outer and z inner from `floor(-r)` to `ceil(r)`,
/// end stone where `x² + z² <= (r + 1)²` in floats, the radius less `nextInt(2) + 0.5`
/// after each layer. The positions in the order they are set.
fn game_island(origin: BlockPos) -> Vec<BlockPos> {
    let mut random = JavaRandom::new(as_long(origin));
    let mut radius = random.next_int(3) as f32 + 4.0;
    let mut y = 0;
    let mut set = Vec::new();
    while radius > 0.5 {
        let low = (-radius).floor() as i32;
        let high = radius.ceil() as i32;
        for x in low..=high {
            for z in low..=high {
                if ((x * x + z * z) as f32) <= (radius + 1.0) * (radius + 1.0) {
                    set.push(origin.offset(x, y, z));
                }
            }
        }
        radius -= random.next_int(2) as f32 + 0.5;
        y -= 1;
    }
    set
}

/// A world of chunks: those it was given, every other one empty, as the void of the
/// outer End is where nothing was generated.
#[derive(Default, Clone)]
struct World(BTreeMap<ChunkPos, Chunk>);

impl World {
    fn set(&mut self, pos: BlockPos, state: BlockState) {
        let chunk = self.0.entry(pos.chunk()).or_insert_with(empty);
        let (x, z) = pos.in_chunk();
        chunk.set(x, pos.y, z, state);
    }

    fn chunk(&self, at: ChunkPos) -> Chunk {
        self.0.get(&at).cloned().unwrap_or_else(empty)
    }

    fn get(&self, pos: BlockPos) -> BlockState {
        let (x, z) = pos.in_chunk();
        self.0
            .get(&pos.chunk())
            .and_then(|chunk| chunk.get(x, pos.y, z))
            .unwrap_or(blocks::AIR)
    }

    /// A floor of `state` over the whole of each chunk in `chunks`, at `y`.
    fn floor(&mut self, chunks: impl IntoIterator<Item = ChunkPos>, y: i32, state: BlockState) {
        for at in chunks {
            for x in 0..16 {
                for z in 0..16 {
                    self.set(BlockPos::new(at.x * 16 + x, y, at.z * 16 + z), state);
                }
            }
        }
    }
}

/// The game's reading of the level as `findOrCreateValidTeleportPos` goes: the world,
/// with the island's blocks over it once it is placed; and the chunks in the order it
/// first reads them.
struct Game<'a> {
    world: &'a World,
    laid: BTreeMap<BlockPos, BlockState>,
    read: Vec<ChunkPos>,
}

impl Game<'_> {
    fn note(&mut self, at: ChunkPos) {
        if !self.read.contains(&at) {
            self.read.push(at);
        }
    }

    fn block(&mut self, pos: BlockPos) -> BlockState {
        self.note(pos.chunk());
        if let Some(state) = self.laid.get(&pos) {
            return *state;
        }
        self.world.get(pos)
    }

    /// `getChunk(Level, Vec3)`: `Mth.floor(x / 16.0)`, `Mth.floor(z / 16.0)`.
    fn chunk_at(point: (f64, f64)) -> ChunkPos {
        ChunkPos::new(
            (point.0 / 16.0).floor() as i32,
            (point.1 / 16.0).floor() as i32,
        )
    }

    /// The highest section index with a block that is not air, by reading every block
    /// (`getHighestFilledSectionIndex`).
    fn highest_filled(chunk: &Chunk) -> Option<i32> {
        let (min_y, height) = bounds();
        let sections = height as i32 / 16;
        (0..sections).rev().find(|section| {
            (0..16).any(|dy| {
                let y = min_y + section * 16 + dy;
                (0..16)
                    .any(|x| (0..16).any(|z| !chunk.get(x, y, z).unwrap_or(blocks::AIR).is_air()))
            })
        })
    }

    /// `isChunkEmpty` (lines 461-472).
    fn empty_at(&mut self, point: (f64, f64)) -> bool {
        let at = Self::chunk_at(point);
        self.note(at);
        Self::highest_filled(&self.world.chunk(at)).is_none()
    }
}

/// `findValidSpawnInChunk` (lines 569-662), read from the bytecode: from (minX, 30,
/// minZ) to (maxX, `getHighestSectionPosition() + 15`, maxZ) by `betweenClosed`, which
/// takes the least and greatest of each axis and counts x fastest, then y, then z; end
/// stone whose two blocks above have no full collision; the first whose
/// `distToCenterSqr(0, 0, 0)` is strictly less than the best's.
fn game_spawn(at: ChunkPos, chunk: &Chunk) -> Option<BlockPos> {
    let (min_y, _) = bounds();
    let highest = Game::highest_filled(chunk).map_or(min_y, |section| min_y + section * 16);
    let top = highest + 16 - 1;
    let (low, high) = (30.min(top), 30.max(top));
    let get = |x: i32, y: i32, z: i32| chunk.get(x as usize, y, z as usize).unwrap_or(blocks::AIR);
    let mut best: Option<(BlockPos, f64)> = None;
    for z in 0..16 {
        for y in low..=high {
            for x in 0..16 {
                if get(x, y, z) != blocks::END_STONE
                    || get(x, y + 1, z).props().collision_is_full_block()
                    || get(x, y + 2, z).props().collision_is_full_block()
                {
                    continue;
                }
                let pos = BlockPos::new(at.x * 16 + x, y, at.z * 16 + z);
                let (cx, cy, cz) = (
                    f64::from(pos.x) + 0.5,
                    f64::from(pos.y) + 0.5,
                    f64::from(pos.z) + 0.5,
                );
                let distance = cx * cx + cy * cy + cz * cz;
                if best.is_none() || distance < best.map_or(f64::MAX, |(_, d)| d) {
                    best = Some((pos, distance));
                }
            }
        }
    }
    best.map(|(pos, _)| pos)
}

/// `findTallestBlock` (lines 474-551), read from the bytecode.
fn game_tallest(
    block: &mut dyn FnMut(BlockPos) -> BlockState,
    pos: BlockPos,
    radius: i32,
    allow_bedrock: bool,
) -> BlockPos {
    let (min_y, _) = bounds();
    let mut best: Option<BlockPos> = None;
    for dx in -radius..=radius {
        for dz in -radius..=radius {
            if dx == 0 && dz == 0 && !allow_bedrock {
                continue;
            }
            let mut y = max_y();
            while y > best.map_or(min_y, |best| best.y) {
                let at = BlockPos::new(pos.x + dx, y, pos.z + dz);
                let state = block(at);
                if state.props().collision_is_full_block()
                    && (allow_bedrock || state != blocks::BEDROCK)
                {
                    best = Some(at);
                    break;
                }
                y -= 1;
            }
        }
    }
    best.unwrap_or(pos)
}

/// `findOrCreateValidTeleportPos(level, from).above(10)` over `world`, read from the
/// bytecode (lines 340-459): with the island's place where one was made, and the chunks
/// in the order the game first reads them.
fn game_exit(from: BlockPos, world: &World) -> (GatewayExit, Vec<ChunkPos>) {
    let mut game = Game {
        world,
        laid: BTreeMap::new(),
        read: Vec::new(),
    };
    // `new Vec3(x, 0, z).normalize()`.
    let (x, z) = (f64::from(from.x), f64::from(from.z));
    let length = (x * x + 0.0 * 0.0 + z * z).sqrt();
    let direction = if length < f64::from(1.0e-5_f32) {
        (0.0, 0.0)
    } else {
        (x / length, z / length)
    };
    let mut point = (direction.0 * 1024.0, direction.1 * 1024.0);
    let mut steps = 16;
    loop {
        if game.empty_at(point) {
            break;
        }
        let left = steps;
        steps -= 1;
        if left <= 0 {
            break;
        }
        point = (point.0 + direction.0 * -16.0, point.1 + direction.1 * -16.0);
    }
    let mut steps = 16;
    loop {
        if !game.empty_at(point) {
            break;
        }
        let left = steps;
        steps -= 1;
        if left <= 0 {
            break;
        }
        point = (point.0 + direction.0 * 16.0, point.1 + direction.1 * 16.0);
    }
    let at = Game::chunk_at(point);
    let chunk = world.chunk(at);
    let (origin, island) = match game_spawn(at, &chunk) {
        Some(spawn) => (spawn, None),
        None => {
            // `BlockPos.containing(x + 0.5, 75.0, z + 0.5)`.
            let origin = BlockPos::new(
                (point.0 + 0.5).floor() as i32,
                75,
                (point.1 + 0.5).floor() as i32,
            );
            for pos in game_island(origin) {
                game.laid.insert(pos, blocks::END_STONE);
            }
            (origin, Some(origin))
        }
    };
    let tallest = game_tallest(&mut |pos| game.block(pos), origin, 16, true);
    let exit = GatewayExit {
        exit: tallest.offset(0, 10, 0),
        island,
    };
    (exit, game.read)
}

/// The search run to its end for the gateway at `from`, fed from `world`: what it
/// found, and the chunks it asked for in order.
fn searched(from: BlockPos, world: &World) -> (GatewayExit, Vec<ChunkPos>) {
    let (min_y, height) = bounds();
    let mut search = GatewaySearch::new(from, min_y, height);
    let mut asked = Vec::new();
    for _ in 0..1_000 {
        match search.step() {
            Step::Need(at) => {
                assert!(!asked.contains(&at), "{at:?} asked for twice: {asked:?}");
                asked.push(at);
                search.feed(at, &world.chunk(at));
            }
            Step::Done(exit) => return (exit, asked),
        }
    }
    panic!("the search did not end: {asked:?}");
}

/// End stone a layer thick at y = 40 over chunk x `from..=to`, z -2 to 2: land along
/// the x axis.
fn land_along_x(from: i32, to: i32) -> World {
    let mut world = World::default();
    let chunks = (from..=to).flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)));
    world.floor(chunks, 40, blocks::END_STONE);
    world
}

// The island.

/// EndIslandFeature.txt with `RandomSource.create(pos.asLong())` (lambda at 743-757):
/// the blocks laid and set for an island are the game's, in its order, about origins
/// on every side of the middle.
#[test]
fn the_island_s_blocks_are_end_island_feature_s_with_the_random_of_its_place() {
    for origin in [
        BlockPos::new(1280, 75, 0),
        BlockPos::new(-905, 75, -905),
        BlockPos::new(3, 75, -1_100),
        BlockPos::new(-1_000_000, 75, 777),
        BlockPos::new(0, 75, 0),
    ] {
        let made: Vec<(BlockPos, BlockState)> = island_writes(origin);
        let expected: Vec<(BlockPos, BlockState)> = game_island(origin)
            .into_iter()
            .map(|pos| (pos, blocks::END_STONE))
            .collect();
        assert_eq!(made, expected, "the island about {origin:?}");
        // Its top layer is the origin's, and it goes down from there.
        assert!(made.iter().all(|(pos, _)| pos.y <= origin.y));
        assert!(made.iter().any(|(pos, _)| *pos == origin));
    }
}

// `findExitPortalXZPosTentative`.

/// Lines 394-459: in a void, the point is 1024 out and steps on 16 at a time, at most
/// 16 times, to 1280; no end stone there, so the island is wanted at
/// `containing(x + 0.5, 75, z + 0.5)`, and the exit is ten above the tallest block over
/// it.
#[test]
fn in_a_void_the_point_steps_on_sixteen_times_and_an_island_is_wanted_at_1280() {
    let world = World::default();
    let from = BlockPos::new(96, 75, 0);
    let (found, asked) = searched(from, &world);
    let (expected, read) = game_exit(from, &world);
    assert_eq!(found, expected);
    assert_eq!(found.island, Some(BlockPos::new(1280, 75, 0)));
    assert_eq!(found.exit.y, 85, "the island's top is at 75");
    // Chunks 64 to 80 along the line, then the rest of the 3 x 3 about the island.
    let line: Vec<ChunkPos> = (64..=80).map(|x| ChunkPos::new(x, 0)).collect();
    assert_eq!(asked[..line.len()], line[..]);
    let about: BTreeSet<ChunkPos> = (79..=81)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .filter(|chunk| !line.contains(chunk))
        .collect();
    let rest: BTreeSet<ChunkPos> = asked[line.len()..].iter().copied().collect();
    assert_eq!(rest, about, "{asked:?}");
    assert_eq!(
        asked.iter().copied().collect::<BTreeSet<_>>(),
        read.iter().copied().collect::<BTreeSet<_>>(),
        "the chunks the game reads"
    );
}

/// Lines 39-83: over land, the point steps back 16 at a time while the chunk there is
/// not empty; at the first empty one it steps on again while empty, to the land's edge,
/// and stands on its end stone there, nearest the origin.
#[test]
fn over_land_the_point_steps_back_to_the_void_then_on_to_the_land_s_edge() {
    let world = land_along_x(60, 70);
    let from = BlockPos::new(96, 75, 0);
    let (found, _) = searched(from, &world);
    assert_eq!(found, game_exit(from, &world).0);
    // Back from 1024 (chunk 64) to 944 (chunk 59, empty), on to 960 (chunk 60): its
    // end stone nearest the origin is (960, 40, 0); the tallest block within 16 of it,
    // bedrock allowed, the first found at y = 40 in the order dx then dz, is
    // (960, 40, -16), as chunk 59 is empty.
    assert_eq!(found.island, None);
    assert_eq!(found.exit, BlockPos::new(960, 50, -16));
}

/// Offsets 48-53: at most sixteen steps back; then, on land still, no step on.
#[test]
fn over_wide_land_the_point_steps_back_no_more_than_sixteen_times() {
    let world = land_along_x(40, 70);
    let from = BlockPos::new(96, 75, 0);
    let (found, _) = searched(from, &world);
    assert_eq!(found, game_exit(from, &world).0);
    // 1024 - 16 x 16 = 768, chunk 48, land; its spawn (768, 40, 0), and the first
    // tallest in the order dx then dz (752, 40, -16).
    assert_eq!(found.exit, BlockPos::new(752, 50, -16));
}

/// Offsets 99-104: at most sixteen steps on: void out to 1280 and land beyond it is
/// never reached.
#[test]
fn over_wide_void_the_point_steps_on_no_more_than_sixteen_times() {
    let world = land_along_x(81, 90);
    let from = BlockPos::new(96, 75, 0);
    let (found, _) = searched(from, &world);
    assert_eq!(found, game_exit(from, &world).0);
    assert_eq!(found.island, Some(BlockPos::new(1280, 75, 0)));
}

/// `normalize` and `Mth.floor(x / 16.0)` westwards and northwards, on a diagonal: the
/// island's place rounds halves up (`containing(x + 0.5, …)`).
#[test]
fn on_a_diagonal_to_the_north_west_the_points_and_the_island_are_the_game_s() {
    let world = World::default();
    for from in [
        BlockPos::new(-70, 75, -70),
        BlockPos::new(-3, 75, 100),
        BlockPos::new(57, 80, -91),
    ] {
        let (found, asked) = searched(from, &world);
        let (expected, read) = game_exit(from, &world);
        assert_eq!(found, expected, "from {from:?}");
        let mut asked = asked;
        let mut read = read;
        asked.sort();
        read.sort();
        assert_eq!(asked, read, "the chunks read from {from:?}");
    }
    let (found, _) = searched(BlockPos::new(-70, 75, -70), &world);
    // 1280 / sqrt(2) = 905.097, so -905.097 + 0.5 floors to -905.
    assert_eq!(found.island, Some(BlockPos::new(-905, 75, -905)));
}

/// `normalize` of a vector shorter than 1.0E-5: no direction, so every point is the
/// middle, in chunk (0, 0).
#[test]
fn a_gateway_over_the_middle_looks_in_the_middle_s_chunk() {
    let mut world = World::default();
    world.set(BlockPos::new(5, 60, 6), blocks::END_STONE);
    let from = BlockPos::new(0, 75, 0);
    let (found, asked) = searched(from, &world);
    assert_eq!(found, game_exit(from, &world).0);
    assert_eq!(asked[0], ChunkPos::new(0, 0));
    assert_eq!(found.exit, BlockPos::new(5, 70, 6));
    assert_eq!(found.island, None);
}

/// The game against the search over worlds of scattered land, from gateways on every
/// side: the same exit and island each time.
#[test]
fn over_scattered_land_the_search_finds_what_the_game_finds() {
    let mut random = JavaRandom::new(198);
    for case in 0..6 {
        let mut world = World::default();
        // Lumps of end stone in some chunks of a ring 700 to 1350 out, at heights
        // from 10 to 90, with now and then a block over them.
        for _ in 0..400 {
            let angle = f64::from(random.next_int(360)).to_radians();
            let out = f64::from(700 + random.next_int(650));
            let (cx, cz) = (out * angle.cos(), out * angle.sin());
            let y = 10 + random.next_int(80);
            let base = BlockPos::new(cx.floor() as i32, y, cz.floor() as i32);
            for dx in 0..random.next_int(6) {
                for dz in 0..random.next_int(6) {
                    world.set(base.offset(dx, 0, dz), blocks::END_STONE);
                }
            }
            match random.next_int(4) {
                0 => world.set(base.offset(0, 2, 0), blocks::OBSIDIAN),
                1 => world.set(base.offset(0, 1, 0), blocks::TORCH),
                2 => world.set(base.offset(1, 3, 1), blocks::BEDROCK),
                _ => {}
            }
        }
        for from in [
            BlockPos::new(96, 75, 0),
            BlockPos::new(-96, 75, 0),
            BlockPos::new(0, 75, 96),
            BlockPos::new(29, 75, -91),
            BlockPos::new(-77, 75, -58),
            BlockPos::new(68, 75, 68),
        ] {
            let (found, _) = searched(from, &world);
            let (expected, _) = game_exit(from, &world);
            assert_eq!(found, expected, "case {case}, from {from:?}");
        }
    }
}

// `findValidSpawnInChunk`.

/// Lines 569-662: the end stone nearest the origin by `distToCenterSqr`, with two
/// blocks above it of no full collision; an equal distance keeps the first in
/// `betweenClosed` order (x fastest, then y, then z).
#[test]
fn the_spawn_is_the_end_stone_nearest_the_origin_and_the_first_of_equals() {
    let at = ChunkPos::new(0, 0);
    let mut chunk = empty();
    // (5, 40, 1) and (1, 40, 5) are as near as each other; z = 1 comes first.
    chunk.set(5, 40, 1, blocks::END_STONE);
    chunk.set(1, 40, 5, blocks::END_STONE);
    chunk.set(9, 41, 9, blocks::END_STONE);
    assert_eq!(game_spawn(at, &chunk), Some(BlockPos::new(5, 40, 1)));
    assert_eq!(
        find_valid_spawn_in_chunk(at, &chunk),
        Some(BlockPos::new(5, 40, 1))
    );
    // A nearer one with a full block two above it is refused; with a torch above, it
    // stands.
    chunk.set(1, 35, 1, blocks::END_STONE);
    chunk.set(1, 37, 1, blocks::OBSIDIAN);
    assert_eq!(
        find_valid_spawn_in_chunk(at, &chunk),
        game_spawn(at, &chunk)
    );
    assert_eq!(
        find_valid_spawn_in_chunk(at, &chunk),
        Some(BlockPos::new(5, 40, 1))
    );
    chunk.set(1, 37, 1, blocks::TORCH);
    chunk.set(1, 36, 1, blocks::TORCH);
    assert_eq!(
        find_valid_spawn_in_chunk(at, &chunk),
        Some(BlockPos::new(1, 35, 1))
    );
    assert_eq!(
        find_valid_spawn_in_chunk(at, &chunk),
        game_spawn(at, &chunk)
    );
}

/// Offsets 5-49: the search runs from y = 30 to the top of the highest section with a
/// block in it; end stone below 30 is not looked at, nor is anything but end stone.
#[test]
fn the_spawn_is_looked_for_from_thirty_to_the_top_of_the_highest_filled_section() {
    let at = ChunkPos::new(62, 0);
    let mut chunk = empty();
    chunk.set(3, 29, 3, blocks::END_STONE);
    chunk.set(7, 60, 7, blocks::STONE_SLAB);
    chunk.set(8, 50, 8, blocks::OBSIDIAN);
    assert_eq!(find_valid_spawn_in_chunk(at, &chunk), None);
    assert_eq!(game_spawn(at, &chunk), None);
    // End stone at the top of the highest filled section is looked at.
    chunk.set(4, 63, 4, blocks::END_STONE);
    assert_eq!(
        find_valid_spawn_in_chunk(at, &chunk),
        Some(BlockPos::new(62 * 16 + 4, 63, 4))
    );
    assert_eq!(
        find_valid_spawn_in_chunk(at, &chunk),
        game_spawn(at, &chunk)
    );
}

/// `betweenClosed` takes the least and greatest of each axis (`d-javap` BlockPos,
/// 839-872): where the highest filled section is the lowest, its top, 15, is under 30,
/// and the game looks from 15 up to 30, so end stone at y = 15 is found.
#[test]
fn where_only_the_lowest_section_has_blocks_the_spawn_is_looked_for_from_its_top_up_to_thirty() {
    let at = ChunkPos::new(-3, 7);
    let mut chunk = empty();
    chunk.set(2, 15, 2, blocks::END_STONE);
    chunk.set(9, 10, 9, blocks::END_STONE);
    let expected = Some(BlockPos::new(-3 * 16 + 2, 15, 7 * 16 + 2));
    assert_eq!(game_spawn(at, &chunk), expected);
    assert_eq!(find_valid_spawn_in_chunk(at, &chunk), expected);
    // In the section above it, under 30, nothing is looked at.
    let mut chunk = empty();
    chunk.set(2, 20, 2, blocks::END_STONE);
    assert_eq!(game_spawn(at, &chunk), None);
    assert_eq!(find_valid_spawn_in_chunk(at, &chunk), None);
}

/// Offsets 18-97 of `findOrCreateValidTeleportPos`: a tentative chunk with land but no
/// end stone to stand on wants an island, and its exit is ten above the tallest block
/// over the island and the land about it, which may be the land's.
#[test]
fn land_with_nothing_to_stand_on_wants_an_island_and_the_tallest_may_be_the_land_s() {
    let mut world = World::default();
    // Obsidian at chunk 64, higher than the island's top, near the island's place.
    for x in 1024..1028 {
        for z in -3..3 {
            world.set(BlockPos::new(x, 100, z), blocks::OBSIDIAN);
        }
    }
    let from = BlockPos::new(96, 75, 0);
    let (found, _) = searched(from, &world);
    let (expected, _) = game_exit(from, &world);
    assert_eq!(found, expected);
    // Back from 1024 to 1008 (empty), on to 1024 again: no end stone, so the island
    // about (1024, 75, 0); the tallest block is the obsidian, first at (1024, 100, -3).
    assert_eq!(found.island, Some(BlockPos::new(1024, 75, 0)));
    assert_eq!(found.exit, BlockPos::new(1024, 110, -3));
}

// `findTallestBlock`.

/// Lines 474-551 with bedrock allowed: dx outer and dz inner, the first full block from
/// the top of each column; only a strictly taller one replaces it; bedrock counts.
#[test]
fn the_tallest_block_is_the_first_of_the_tallest_and_bedrock_counts_where_allowed() {
    let mut world = World::default();
    world.set(BlockPos::new(10, 50, 10), blocks::END_STONE);
    world.set(BlockPos::new(12, 60, 3), blocks::END_STONE);
    world.set(BlockPos::new(12, 60, 2), blocks::END_STONE);
    world.set(BlockPos::new(4, 60, 9), blocks::END_STONE);
    world.set(BlockPos::new(9, 80, 9), blocks::TORCH);
    let read = |pos: BlockPos| world.get(pos);
    let (min_y, _) = bounds();
    let pos = BlockPos::new(8, 70, 8);
    let found = find_tallest_block(read, min_y, max_y(), pos, 5, true);
    assert_eq!(found, BlockPos::new(4, 60, 9), "dx -4 comes before dx 4");
    assert_eq!(found, game_tallest(&mut |pos| world.get(pos), pos, 5, true));
    world.set(BlockPos::new(13, 61, 13), blocks::BEDROCK);
    let read = |pos: BlockPos| world.get(pos);
    assert_eq!(
        find_tallest_block(read, min_y, max_y(), pos, 5, true),
        BlockPos::new(13, 61, 13)
    );
}

/// Offsets 23-37 and 118-130 with bedrock not allowed: the middle column is passed
/// over, and a column's bedrock is passed over too, the column going on down to a
/// block beneath it.
#[test]
fn without_bedrock_the_middle_column_is_passed_over_and_bedrock_is_looked_through() {
    let mut world = World::default();
    let pos = BlockPos::new(-20, 70, 30);
    world.set(pos.offset(0, 20, 0), blocks::END_STONE);
    world.set(pos.offset(2, 30, 0), blocks::BEDROCK);
    world.set(pos.offset(2, 10, 0), blocks::END_STONE);
    world.set(pos.offset(-1, 5, 1), blocks::END_STONE);
    let (min_y, _) = bounds();
    let found = find_tallest_block(|at| world.get(at), min_y, max_y(), pos, 5, false);
    assert_eq!(found, pos.offset(2, 10, 0));
    assert_eq!(found, game_tallest(&mut |at| world.get(at), pos, 5, false));
}

/// Offsets 48-69 and 158-169: a column is read down while above the lowest y, so a
/// block at the lowest y is never found; with nothing found, the position itself.
#[test]
fn with_nothing_above_the_lowest_y_the_tallest_block_is_the_position_itself() {
    let mut world = World::default();
    let (min_y, _) = bounds();
    world.set(BlockPos::new(1, min_y, 1), blocks::END_STONE);
    let pos = BlockPos::new(0, 64, 0);
    assert_eq!(
        find_tallest_block(|at| world.get(at), min_y, max_y(), pos, 5, true),
        pos
    );
    assert_eq!(game_tallest(&mut |at| world.get(at), pos, 5, true), pos);
}
