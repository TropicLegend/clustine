//! Where an End gateway without an exit sends what goes through it: the game's
//! `TheEndGatewayBlockEntity.findOrCreateValidTeleportPos` (`d-javap`, lines 340-392),
//! run by the store over each chunk's committed form (`docs/groundwork/p-plan.md`,
//! section 19.8, with its review's items 2 and 5).
//!
//! The game reads the level as it goes, loading and generating chunks where it looks.
//! The store cannot: it reads a chunk at a time on its commit thread. So the search is a
//! machine of steps with no I/O. [`GatewaySearch::step`] says which chunk it needs next,
//! or its result; [`GatewaySearch::feed`] hands it that chunk as it stands. It asks for
//! each chunk once, in the order the game first reads them, and keeps what it was fed.
//!
//! Where the chunk it lands in has no end stone to stand on, the game places an island
//! (`end_island`) and looks for the tallest block over it. The search lays the island's
//! blocks over its own reading in memory; making them in the world is the exit's
//! holder's (19.8's review, item 2), which [`GatewayExit::island`] tells where.

use std::collections::BTreeMap;

use clustine_data::{BlockState, blocks};
use clustine_noise::LegacyRandom;
use clustine_world::{BlockPos, Chunk, ChunkPos};

use crate::blocks::same_block;
use crate::features::end::end_island::island_blocks;

/// How far out the first point is (`findExitPortalXZPosTentative`, offset 22-30:
/// `sipush 1024` and `scale(1024.0)`).
const REACH: f64 = 1024.0;

/// How far each step back or on goes (offsets 72 and 123: `scale(-16.0)`,
/// `scale(16.0)`).
const STRIDE: f64 = 16.0;

/// How many steps back, then how many on, at most (offsets 35 and 86: `bipush 16`).
const STEPS: u32 = 16;

/// The lowest y `findValidSpawnInChunk` looks at (offset 13: `bipush 30`).
const LOWEST_SPAWN: i32 = 30;

/// The y the island is wanted at (`findOrCreateValidTeleportPos`, offset 31:
/// `ldc2_w 75.0`).
const ISLAND_Y: f64 = 75.0;

/// The radius of `findTallestBlock`'s square (offset 113: `bipush 16`), with bedrock
/// allowed (offset 115: `iconst_1`).
const TALLEST_RADIUS: i32 = 16;

/// How far above the tallest block the exit is (`findExitPosition`'s `above(10)`, as the
/// plan's 19.8 step 4 has it).
const EXIT_ABOVE: i32 = 10;

/// What the search found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatewayExit {
    /// Ten above the tallest block about the spawn or the island.
    pub exit: BlockPos,
    /// The island's origin, where the game would have placed one: `end_island` there
    /// with `RandomSource.create(island.asLong())`.
    pub island: Option<BlockPos>,
}

/// What the search needs next, or what it found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The chunk at this position, as it stands (generated where it was never made).
    Need(ChunkPos),
    Done(GatewayExit),
}

/// Where the search stands.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    /// Stepping back while the chunk is not empty, with this many steps left.
    Back(u32),
    /// Stepping on while the chunk is empty, with this many steps left.
    On(u32),
    /// Looking in the tentative chunk for end stone to stand on.
    Spawn,
    /// Looking for the tallest block about this position.
    Tallest(BlockPos),
    Done(GatewayExit),
}

/// The game's search for a gateway's exit, a chunk at a time.
#[derive(Debug)]
pub struct GatewaySearch {
    min_y: i32,
    /// The level's `getMaxY()`: its top block, `min_y + height - 1`.
    max_y: i32,
    /// The gateway's x and z normalised, as the game's `Vec3.normalize`.
    direction: (f64, f64),
    /// The tentative position, x and z (its y stays 0 throughout).
    point: (f64, f64),
    phase: Phase,
    chunks: BTreeMap<ChunkPos, Chunk>,
    /// The island's blocks, which stand over what the chunks hold.
    laid: BTreeMap<BlockPos, BlockState>,
    island: Option<BlockPos>,
}

impl GatewaySearch {
    /// A search for the gateway at `from`, in a level whose lowest block is at `min_y`
    /// and that is `height` blocks tall.
    pub fn new(from: BlockPos, min_y: i32, height: u32) -> Self {
        // `findExitPortalXZPosTentative`, offsets 0-33: `new Vec3(x, 0, z).normalize()`
        // then `scale(1024.0)`.
        let direction = normalise(f64::from(from.x), f64::from(from.z));
        let max_y = i32::try_from(i64::from(min_y) + i64::from(height) - 1)
            .expect("a level's top is within the integers");
        Self {
            min_y,
            max_y,
            direction,
            point: (direction.0 * REACH, direction.1 * REACH),
            phase: Phase::Back(STEPS),
            chunks: BTreeMap::new(),
            laid: BTreeMap::new(),
            island: None,
        }
    }

    /// What the search needs next, or its result.
    pub fn step(&mut self) -> Step {
        loop {
            match self.phase {
                // Offsets 39-83: `while (!isChunkEmpty(pos) && steps-- > 0)`, the
                // emptiness read first, so the last point stepped to is always read.
                Phase::Back(left) => {
                    let Some(empty) = self.empty_at_point() else {
                        return Step::Need(self.point_chunk());
                    };
                    if !empty && left > 0 {
                        self.phase = Phase::Back(left - 1);
                        self.advance(-STRIDE);
                    } else {
                        self.phase = Phase::On(STEPS);
                    }
                }
                // Offsets 86-134: `while (isChunkEmpty(pos) && steps-- > 0)`.
                Phase::On(left) => {
                    let Some(empty) = self.empty_at_point() else {
                        return Step::Need(self.point_chunk());
                    };
                    if empty && left > 0 {
                        self.phase = Phase::On(left - 1);
                        self.advance(STRIDE);
                    } else {
                        self.phase = Phase::Spawn;
                    }
                }
                Phase::Spawn => {
                    let at = self.point_chunk();
                    let chunk = self
                        .chunks
                        .get(&at)
                        .expect("the tentative point's chunk was read in the last step");
                    let origin = match find_valid_spawn_in_chunk(at, chunk) {
                        Some(spawn) => spawn,
                        None => self.lay_island(),
                    };
                    self.phase = Phase::Tallest(origin);
                }
                Phase::Tallest(origin) => {
                    // `findTallestBlock` reads every column of the square from the top,
                    // so all the 3 x 3 chunks about the origin: dx outer, dz inner.
                    let centre = origin.chunk();
                    for dx in -1..=1 {
                        for dz in -1..=1 {
                            let chunk = ChunkPos::new(centre.x + dx, centre.z + dz);
                            if !self.chunks.contains_key(&chunk) {
                                return Step::Need(chunk);
                            }
                        }
                    }
                    let tallest = find_tallest_block(
                        |pos| self.block(pos),
                        self.min_y,
                        self.max_y,
                        origin,
                        TALLEST_RADIUS,
                        true,
                    );
                    let exit = GatewayExit {
                        exit: tallest.offset(0, EXIT_ABOVE, 0),
                        island: self.island,
                    };
                    self.phase = Phase::Done(exit);
                }
                Phase::Done(exit) => return Step::Done(exit),
            }
        }
    }

    /// The chunk it asked for, as it stands. A chunk fed twice keeps the first.
    pub fn feed(&mut self, position: ChunkPos, chunk: &Chunk) {
        self.chunks.entry(position).or_insert_with(|| chunk.clone());
    }

    /// Moves the tentative point `by` along the direction: `pos.add(dir.scale(by))`.
    fn advance(&mut self, by: f64) {
        self.point = (
            self.point.0 + self.direction.0 * by,
            self.point.1 + self.direction.1 * by,
        );
    }

    /// The chunk of the tentative point, as the game's `getChunk(Level, Vec3)` finds it
    /// (offsets 0-23): `Mth.floor(x / 16.0)` and `Mth.floor(z / 16.0)`, each a
    /// `Math.floor` then a cast to `int` that saturates as Rust's `as` does.
    fn point_chunk(&self) -> ChunkPos {
        ChunkPos::new(
            (self.point.0 / 16.0).floor() as i32,
            (self.point.1 / 16.0).floor() as i32,
        )
    }

    /// Whether the tentative point's chunk is empty, or `None` if it has not been fed.
    fn empty_at_point(&self) -> Option<bool> {
        self.chunks.get(&self.point_chunk()).map(is_chunk_empty)
    }

    /// Lays the island where none was found to stand on, and returns its origin
    /// (`findOrCreateValidTeleportPos`, offsets 23-92): `BlockPos.containing(x + 0.5,
    /// 75.0, z + 0.5)` of the tentative point, which rounds x and z to the nearest
    /// integer (halves upwards) rather than down. The game places `end_island` there
    /// with `RandomSource.create(pos.asLong())` (lambda at 743-757), which is a
    /// `LegacyRandomSource` of the seed (`RandomSource.create(long)`, `n7-javap`), and
    /// searches for the tallest block from the same position.
    fn lay_island(&mut self) -> BlockPos {
        let origin = BlockPos::new(
            (self.point.0 + 0.5).floor() as i32,
            ISLAND_Y.floor() as i32,
            (self.point.1 + 0.5).floor() as i32,
        );
        for (pos, state) in island_writes(origin) {
            self.laid.insert(pos, state);
        }
        self.island = Some(origin);
        origin
    }

    /// The block at `pos` in the search's reading: the island's where it laid one,
    /// else the fed chunk's, air outside the level's height.
    fn block(&self, pos: BlockPos) -> BlockState {
        if let Some(state) = self.laid.get(&pos) {
            return *state;
        }
        let (x, z) = pos.in_chunk();
        self.chunks
            .get(&pos.chunk())
            .expect("the tallest block is looked for only in chunks that were fed")
            .get(x, pos.y, z)
            .unwrap_or(blocks::AIR)
    }
}

/// The blocks of the island a gateway's exit is made on, about `origin`, in the order
/// `EndIslandFeature` sets them, each whatever stood there: its random is
/// `RandomSource.create(origin.asLong())`, a `LegacyRandomSource` of the seed. The
/// search lays them in its reading, and the region that holds the exit sets them.
pub fn island_writes(origin: BlockPos) -> Vec<(BlockPos, BlockState)> {
    let mut random = LegacyRandom::from_seed(as_long(origin) as u64);
    island_blocks(&mut random)
        .into_iter()
        .map(|(x, y, z)| (origin.offset(x, y, z), blocks::END_STONE))
        .collect()
}

/// `Vec3.normalize` of `(x, 0, z)` (`n7-javap` Vec3, offsets 0-66): divided by its
/// length, or nought where the length is under `1.0E-5f` widened to a double.
fn normalise(x: f64, z: f64) -> (f64, f64) {
    // The y of nought adds nothing to the sum.
    let length = (x * x + z * z).sqrt();
    if length < f64::from(1.0e-5_f32) {
        return (0.0, 0.0);
    }
    (x / length, z / length)
}

/// `BlockPos.asLong`: x in the top 26 bits, z in the next 26, y in the low 12.
fn as_long(pos: BlockPos) -> i64 {
    let x = i64::from(pos.x) & 0x3FF_FFFF;
    let y = i64::from(pos.y) & 0xFFF;
    let z = i64::from(pos.z) & 0x3FF_FFFF;
    x << 38 | z << 12 | y
}

/// `isChunkEmpty`: `getHighestFilledSectionIndex() == -1`, no section with a block
/// that is not air (`LevelChunkSection.hasOnlyAir`, cave and void air being air).
fn is_chunk_empty(chunk: &Chunk) -> bool {
    highest_filled_section(chunk).is_none()
}

/// `ChunkAccess.getHighestFilledSectionIndex` (`n9-javap`, offsets 0-34): the highest
/// section with a block that is not air.
fn highest_filled_section(chunk: &Chunk) -> Option<usize> {
    chunk
        .sections()
        .iter()
        .rposition(|section| section.non_air_count() != 0)
}

/// `findValidSpawnInChunk` (lines 569-662): the end stone in `chunk` with two blocks
/// above it whose collision shapes are not full, from y 30 to the top of the chunk's
/// highest filled section, nearest the origin by `distToCenterSqr(0, 0, 0)`; the first
/// strictly nearest in `BlockPos.betweenClosed` order wins.
pub fn find_valid_spawn_in_chunk(at: ChunkPos, chunk: &Chunk) -> Option<BlockPos> {
    // Offsets 23-32: `getHighestSectionPosition() + 16 - 1`, the section's lowest block
    // or, for an empty chunk, the chunk's lowest y (`ChunkAccess`, offsets 0-25).
    let highest = match highest_filled_section(chunk) {
        Some(section) => chunk.min_y() + section as i32 * 16,
        None => chunk.min_y(),
    };
    let top = highest + 16 - 1;
    // `betweenClosed(BlockPos, BlockPos)` takes the least and greatest of each axis
    // (`d-javap` BlockPos, 839-869), so a top below 30 searches from the top up to 30.
    let (low, high) = (LOWEST_SPAWN.min(top), LOWEST_SPAWN.max(top));
    let block = |x: usize, y: i32, z: usize| chunk.get(x, y, z).unwrap_or(blocks::AIR);
    let mut best: Option<(BlockPos, f64)> = None;
    // `betweenClosed` counts x fastest, then y, then z.
    for z in 0..16_usize {
        for y in low..=high {
            for x in 0..16_usize {
                if !same_block(block(x, y, z), blocks::END_STONE)
                    || block(x, y + 1, z).props().collision_is_full_block()
                    || block(x, y + 2, z).props().collision_is_full_block()
                {
                    continue;
                }
                let pos = BlockPos::new(at.x * 16 + x as i32, y, at.z * 16 + z as i32);
                let distance = dist_to_center_sqr(pos);
                // Offsets 166-176: taken where none was yet or `distance < best`.
                if best.is_none_or(|(_, nearest)| distance < nearest) {
                    best = Some((pos, distance));
                }
            }
        }
    }
    best.map(|(pos, _)| pos)
}

/// `Vec3i.distToCenterSqr(0, 0, 0)` (`n7-javap`, offsets 0-57).
fn dist_to_center_sqr(pos: BlockPos) -> f64 {
    let x = f64::from(pos.x) + 0.5;
    let y = f64::from(pos.y) + 0.5;
    let z = f64::from(pos.z) + 0.5;
    x * x + y * y + z * z
}

/// `findTallestBlock` (lines 474-551) over `level`, whose blocks run from `min_y` to
/// `max_y`: over the square of `radius` about `pos`, dx outer and dz inner, each column
/// scanned down from `max_y` while above the best so far (above `min_y` where there is
/// none, so `min_y` itself is never looked at), the first block whose collision shape is
/// full ending the column; it replaces the best, which only a strictly taller block can
/// then. Bedrock counts only where `allow_bedrock`, and without it the middle column is
/// skipped. With nothing found, `pos` itself.
pub fn find_tallest_block(
    level: impl Fn(BlockPos) -> BlockState,
    min_y: i32,
    max_y: i32,
    pos: BlockPos,
    radius: i32,
    allow_bedrock: bool,
) -> BlockPos {
    let mut best: Option<BlockPos> = None;
    for dx in -radius..=radius {
        for dz in -radius..=radius {
            // Offsets 23-37.
            if dx == 0 && dz == 0 && !allow_bedrock {
                continue;
            }
            let floor = best.map_or(min_y, |best| best.y);
            let mut y = max_y;
            while y > floor {
                let at = BlockPos::new(pos.x + dx, y, pos.z + dz);
                let state = level(at);
                // Offsets 107-137: a full collision shape that is not bedrock, unless
                // bedrock is allowed, ends the column; anything else goes on down.
                if state.props().collision_is_full_block()
                    && (allow_bedrock || !same_block(state, blocks::BEDROCK))
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

#[cfg(test)]
mod tests {
    use clustine_noise::Random;
    use clustine_world::{Biome, Section};

    use super::*;

    const MIN_Y: i32 = 0;
    const HEIGHT: u32 = 256;

    fn empty() -> Chunk {
        Chunk::from_sections(MIN_Y, vec![Section::filled(blocks::AIR, Biome(0)); 16])
    }

    /// A chunk with one block of stone at (0, 1, 0) in it: not empty, nothing to
    /// stand on above y 30.
    fn not_empty() -> Chunk {
        let mut chunk = empty();
        chunk.set(0, 1, 0, blocks::STONE);
        chunk
    }

    /// Runs `search` to its end, feeding each chunk it asks for from `world` (an empty
    /// chunk where `world` has none), and returns the chunks asked for in order.
    fn run(
        search: &mut GatewaySearch,
        world: impl Fn(ChunkPos) -> Option<Chunk>,
    ) -> (Vec<ChunkPos>, GatewayExit) {
        let mut asked = Vec::new();
        for _ in 0..1000 {
            match search.step() {
                Step::Need(at) => {
                    assert!(!asked.contains(&at), "{at:?} asked for twice");
                    asked.push(at);
                    search.feed(at, &world(at).unwrap_or_else(empty));
                }
                Step::Done(exit) => return (asked, exit),
            }
        }
        panic!("the search did not end");
    }

    #[test]
    fn the_first_point_is_1024_out_along_the_gateway_s_direction() {
        // (3, 4) is five from the middle: the direction (0.6, 0.8), the point
        // (614.4, 819.2), in chunk (38, 51).
        let mut search = GatewaySearch::new(BlockPos::new(3, 70, 4), MIN_Y, HEIGHT);
        assert_eq!(search.direction, (3.0 / 5.0, 4.0 / 5.0));
        assert_eq!(search.point, (0.6 * 1024.0, 0.8 * 1024.0));
        assert_eq!(search.step(), Step::Need(ChunkPos::new(38, 51)));
        // Asked again before it is fed, it asks for the same.
        assert_eq!(search.step(), Step::Need(ChunkPos::new(38, 51)));

        // Westwards and northwards, the chunk is rounded down: -1024 is in chunk -64.
        let mut search = GatewaySearch::new(BlockPos::new(-5, 70, 0), MIN_Y, HEIGHT);
        assert_eq!(search.step(), Step::Need(ChunkPos::new(-64, 0)));

        // On the diagonal (1, -1): the point (724.08, -724.08), x / 16 = 45.25 and
        // z / 16 = -45.25, floored to chunk (45, -46). A step back of 16 along it is
        // 11.31 on each axis: (712.76, -712.76), 44.55 and -44.55, chunk (44, -45).
        let mut search = GatewaySearch::new(BlockPos::new(1, 70, -1), MIN_Y, HEIGHT);
        assert_eq!(search.step(), Step::Need(ChunkPos::new(45, -46)));
        search.feed(ChunkPos::new(45, -46), &not_empty());
        assert_eq!(search.step(), Step::Need(ChunkPos::new(44, -45)));
    }

    #[test]
    fn a_gateway_at_the_middle_has_no_direction_and_looks_at_the_middle() {
        let mut search = GatewaySearch::new(BlockPos::new(0, 70, 0), MIN_Y, HEIGHT);
        assert_eq!(search.direction, (0.0, 0.0));
        assert_eq!(search.step(), Step::Need(ChunkPos::new(0, 0)));
    }

    #[test]
    fn it_steps_back_over_land_then_on_over_void_and_stands_on_the_land() {
        // Eastwards: the points are 1024, 1008, ... exactly, in chunks 64, 63, ...
        let mut search = GatewaySearch::new(BlockPos::new(16, 70, 0), MIN_Y, HEIGHT);
        // Land in chunks 62 to 64 (end stone to stand on in 62 at (995, 40, 5) and
        // (1000, 40, 2)), void at 61 and before.
        let world = |at: ChunkPos| {
            if at.z != 0 || !(62..=64).contains(&at.x) {
                return None;
            }
            let mut chunk = not_empty();
            if at.x == 62 {
                // 995 - 992 = 3, 1000 - 992 = 8.
                chunk.set(3, 40, 5, blocks::END_STONE);
                chunk.set(8, 40, 2, blocks::END_STONE);
            }
            Some(chunk)
        };
        let (asked, exit) = run(&mut search, world);
        // Back over 64, 63 and 62 to the void at 61; on again to 62, read already. Then
        // the 3 x 3 about the spawn (995, 40, 5) in chunk 62, those not read yet, dx
        // outer and dz inner.
        let chunk = ChunkPos::new;
        assert_eq!(
            asked,
            [
                chunk(64, 0),
                chunk(63, 0),
                chunk(62, 0),
                chunk(61, 0),
                chunk(61, -1),
                chunk(61, 1),
                chunk(62, -1),
                chunk(62, 1),
                chunk(63, -1),
                chunk(63, 1),
            ]
        );
        // The spawn nearest the middle is (995, 40, 5). About it, dx outer from -16:
        // the stone at (992, 1, 0) first, then the spawn's own column (dx 0), taller;
        // the end stone at (1000, 40, 2) is as tall, not taller, and does not replace
        // it.
        assert_eq!(exit.exit, BlockPos::new(995, 50, 5));
        assert_eq!(exit.island, None);
    }

    #[test]
    fn it_steps_back_sixteen_times_at_most() {
        let mut search = GatewaySearch::new(BlockPos::new(16, 70, 0), MIN_Y, HEIGHT);
        let mut stepped = Vec::new();
        // Land everywhere along the way: back from 64 to 48, 16 steps, and no further.
        loop {
            match search.step() {
                Step::Need(at) => {
                    stepped.push(at.x);
                    search.feed(at, &not_empty());
                }
                Step::Done(exit) => {
                    // Nothing to stand on: an island at the last point, 1024 - 256.
                    assert_eq!(exit.island, Some(BlockPos::new(768, 75, 0)));
                    break;
                }
            }
        }
        let back: Vec<i32> = (48..=64).rev().collect();
        assert_eq!(stepped[..17], back[..]);
        assert_eq!(search.point, (768.0, 0.0));
    }

    #[test]
    fn it_steps_on_sixteen_times_at_most_and_makes_an_island_in_the_void() {
        let from = BlockPos::new(16, 70, 0);
        let mut search = GatewaySearch::new(from, MIN_Y, HEIGHT);
        let (asked, exit) = run(&mut search, |_| None);
        // All void: 64 read in the step back, then on from 64 to 80.
        let on: Vec<ChunkPos> = (64..=80).map(|x| ChunkPos::new(x, 0)).collect();
        assert_eq!(asked[..17], on[..]);
        assert_eq!(search.point, (1280.0, 0.0));
        // The island at `containing(1280.5, 75, 0.5)`.
        let island = BlockPos::new(1280, 75, 0);
        assert_eq!(exit.island, Some(island));
        // The 3 x 3 about it, those not read on the way: all but (79, 0) and (80, 0).
        assert_eq!(
            asked[17..],
            [
                ChunkPos::new(79, -1),
                ChunkPos::new(79, 1),
                ChunkPos::new(80, -1),
                ChunkPos::new(80, 1),
                ChunkPos::new(81, -1),
                ChunkPos::new(81, 0),
                ChunkPos::new(81, 1),
            ]
        );
        // The island's blocks, from `RandomSource.create(asLong)`, laid in memory.
        let seed = as_long(island);
        assert_eq!(seed, 1280 << 38 | 75);
        let expected = island_blocks(&mut LegacyRandom::from_seed(seed as u64));
        assert_eq!(search.laid.len(), expected.len());
        for (x, y, z) in &expected {
            assert_eq!(
                search.laid.get(&island.offset(*x, *y, *z)),
                Some(&blocks::END_STONE)
            );
        }
        // The tallest block: the top layer at y 75, whose first column in dx-outer,
        // dz-inner order is at dx = -radius and the least dz with dx² + dz² within
        // (radius + 1)²: dz² <= 2 radius + 1.
        let radius = LegacyRandom::from_seed(seed as u64).next_i32_bounded(3) + 4;
        let reach = 2 * radius + 1;
        let dz = (0..=radius)
            .rev()
            .find(|dz| dz * dz <= reach)
            .expect("0 does");
        assert_eq!(exit.exit, BlockPos::new(1280 - radius, 85, -dz));
    }

    #[test]
    fn the_spawn_is_the_end_stone_nearest_the_middle_with_room_above() {
        let at = ChunkPos::new(0, 0);
        let mut chunk = empty();
        // Below y 30: not looked at, however near.
        chunk.set(0, 29, 0, blocks::END_STONE);
        // Nearer than the rest, but under a full block, then with one two above.
        chunk.set(0, 31, 0, blocks::END_STONE);
        chunk.set(0, 32, 0, blocks::STONE);
        chunk.set(1, 31, 0, blocks::END_STONE);
        chunk.set(1, 33, 0, blocks::STONE);
        assert_eq!(find_valid_spawn_in_chunk(at, &chunk), None);
        // A block above whose collision is not full leaves room: an end rod.
        chunk.set(5, 40, 5, blocks::END_ROD);
        chunk.set(5, 39, 5, blocks::END_STONE);
        assert_eq!(
            find_valid_spawn_in_chunk(at, &chunk),
            Some(BlockPos::new(5, 39, 5))
        );
        // Two equally near: (2, 35, 1) and (1, 35, 2). `betweenClosed` counts z
        // slowest, so (2, 35, 1) comes first and the second, not strictly nearer, does
        // not replace it.
        chunk.set(1, 35, 2, blocks::END_STONE);
        chunk.set(2, 35, 1, blocks::END_STONE);
        assert_eq!(
            find_valid_spawn_in_chunk(at, &chunk),
            Some(BlockPos::new(2, 35, 1))
        );
        // A strictly nearer one later in the order does.
        chunk.set(1, 34, 1, blocks::END_STONE);
        assert_eq!(
            find_valid_spawn_in_chunk(at, &chunk),
            Some(BlockPos::new(1, 34, 1))
        );
    }

    #[test]
    fn the_spawn_is_looked_for_up_to_the_top_of_the_highest_filled_section() {
        let at = ChunkPos::new(2, -3);
        let mut chunk = empty();
        // End stone at y 47 (section 2, y 32 to 47): looked at. A block in section 3
        // would raise the top; without it, 48 is not looked at.
        chunk.set(4, 47, 4, blocks::END_STONE);
        assert_eq!(
            find_valid_spawn_in_chunk(at, &chunk),
            Some(BlockPos::new(36, 47, -44))
        );
        // An empty chunk's top is its lowest y plus 15, below 30: the range is turned
        // round to 15 to 30 and holds nothing.
        assert_eq!(find_valid_spawn_in_chunk(at, &empty()), None);
    }

    #[test]
    fn the_tallest_block_is_the_first_strictly_tallest_and_falls_back_to_the_position() {
        let pos = BlockPos::new(100, 64, 100);
        let world = |blocks: Vec<(BlockPos, BlockState)>| {
            move |at: BlockPos| {
                blocks
                    .iter()
                    .find(|(here, _)| *here == at)
                    .map_or(blocks::AIR, |(_, state)| *state)
            }
        };
        // Nothing: the position itself.
        assert_eq!(find_tallest_block(world(vec![]), 0, 255, pos, 2, true), pos);
        // Two of equal height: the first in dx-outer, dz-inner order.
        let level = world(vec![
            (BlockPos::new(101, 70, 99), blocks::STONE),
            (BlockPos::new(99, 70, 101), blocks::STONE),
        ]);
        assert_eq!(
            find_tallest_block(&level, 0, 255, pos, 2, true),
            BlockPos::new(99, 70, 101)
        );
        // A strictly taller one later replaces it; a block that is not full above it
        // in its column does not count, and the scan goes on down past it.
        let level = world(vec![
            (BlockPos::new(99, 70, 101), blocks::STONE),
            (BlockPos::new(102, 90, 102), blocks::END_ROD),
            (BlockPos::new(102, 71, 102), blocks::STONE),
        ]);
        assert_eq!(
            find_tallest_block(&level, 0, 255, pos, 2, true),
            BlockPos::new(102, 71, 102)
        );
        // Bedrock counts only where allowed; without it the middle column is skipped.
        let level = world(vec![
            (BlockPos::new(99, 70, 99), blocks::STONE),
            (BlockPos::new(101, 80, 101), blocks::BEDROCK),
            (BlockPos::new(100, 90, 100), blocks::STONE),
        ]);
        assert_eq!(
            find_tallest_block(&level, 0, 255, pos, 2, true),
            BlockPos::new(100, 90, 100)
        );
        assert_eq!(
            find_tallest_block(&level, 0, 255, pos, 2, false),
            BlockPos::new(99, 70, 99)
        );
        // The lowest y is never looked at.
        let level = world(vec![(BlockPos::new(100, 0, 100), blocks::STONE)]);
        assert_eq!(find_tallest_block(&level, 0, 255, pos, 2, true), pos);
    }

    #[test]
    fn a_chunk_of_cave_air_is_empty_and_fed_chunks_are_kept_once() {
        let mut cave = empty();
        cave.set(3, 3, 3, blocks::CAVE_AIR);
        assert!(is_chunk_empty(&cave));
        assert!(!is_chunk_empty(&not_empty()));

        let mut search = GatewaySearch::new(BlockPos::new(16, 70, 0), MIN_Y, HEIGHT);
        search.feed(ChunkPos::new(64, 0), &not_empty());
        search.feed(ChunkPos::new(64, 0), &empty());
        assert!(!is_chunk_empty(&search.chunks[&ChunkPos::new(64, 0)]));
        assert_eq!(search.step(), Step::Need(ChunkPos::new(63, 0)));
    }
}
