//! Tests of step B6 in the simulation: the post-processing of a chunk generation left
//! marks in, and the seam entries of updates that cannot be made where they are made.
//! Written from sections 5, 6 and 6a of `docs/adr/0024-block-behaviour-in-a-region.md`
//! and its "Found while building", by someone who did not write the code.
//!
//! Player 1 stands at (0.5, -60, 0.5) on a stone floor at y = -61 in chunk (0, 0). The
//! region is pinned to the whole world, so every chunk a test loads is held. "L" in a
//! comment is the tick in which a test's chunks were loaded, and so the tick in which a
//! chunk whose eight neighbours came with it starts ticking.

use clustine_blocks::data::is_class;
use clustine_data::BlockClass;
use clustine_world::{ScheduledTick, Seam, SeamKind, Ticked};

use super::*;

fn at(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

fn block_at(region: &Region, position: BlockPos) -> Option<BlockState> {
    let (x, z) = position.in_chunk();
    region.chunk(position.chunk())?.get(x, position.y, z)
}

fn in_chunk(position: BlockPos) -> InChunk {
    let (x, z) = position.in_chunk();
    InChunk::new(x, position.y, z)
}

/// The chunks from `x.0` to `x.1` and from `z.0` to `z.1`, both ends included.
fn chunks(x: (i32, i32), z: (i32, i32)) -> Vec<ChunkPos> {
    (x.0..=x.1)
        .flat_map(|x| (z.0..=z.1).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// The 3×3 around the origin, with which the origin's chunk ticks and no other.
fn around_the_origin() -> Vec<ChunkPos> {
    chunks((-1, 1), (-1, 1))
}

/// The floor, with `blocks` set (by their position in the world), `ticks` kept and
/// `marks` made at the positions given.
fn marked(
    position: ChunkPos,
    blocks: &[(BlockPos, BlockState)],
    ticks: &[ScheduledTick],
    marks: &[BlockPos],
) -> Chunk {
    let mut chunk = floor();
    for (at, state) in blocks {
        assert_eq!(at.chunk(), position, "{at:?} is not in {position:?}");
        let (x, z) = at.in_chunk();
        chunk.set(x, at.y, z, *state);
    }
    chunk.set_ticks(ticks.to_vec());
    let mut marks: Vec<InChunk> = marks
        .iter()
        .inspect(|at| assert_eq!(at.chunk(), position, "{at:?} is not in {position:?}"))
        .map(|at| in_chunk(*at))
        .collect();
    marks.sort();
    chunk.set_marks(marks);
    chunk
}

/// The floor with `blocks` set, nothing kept and nothing marked.
fn with_blocks(position: ChunkPos, blocks: &[(BlockPos, BlockState)]) -> Chunk {
    marked(position, blocks, &[], &[])
}

/// Gives the region viewer's tickets on `chunks` in one tick and the chunks in the
/// next, and returns what that next tick, L, reported.
fn load(region: &mut Region, chunks: Vec<(ChunkPos, Chunk)>) -> TickOutput {
    let positions = chunks.iter().map(|(position, _)| *position).collect();
    checked(region, &tickets(positions, vec![]));
    let output = checked(
        region,
        &TickInputs {
            chunks_loaded: chunks,
            ..TickInputs::default()
        },
    );
    assert_eq!(output.chunk_requests, [], "asked for again");
    output
}

/// A region that player 1 has entered, with `positions` loaded: the floor everywhere,
/// and the given chunk where one is given. Returns what tick L reported.
fn loaded_with(positions: &[ChunkPos], given: Vec<(ChunkPos, Chunk)>) -> (Region, TickOutput) {
    let mut region = joined_in(ChunkArea::EVERYWHERE, &[1]);
    let mut given: BTreeMap<ChunkPos, Chunk> = given.into_iter().collect();
    let chunks = positions
        .iter()
        .map(|position| (*position, given.remove(position).unwrap_or_else(floor)))
        .collect();
    assert!(given.is_empty(), "every chunk given is loaded");
    let output = load(&mut region, chunks);
    (region, output)
}

fn idle_tick(region: &mut Region) -> TickOutput {
    checked(region, &TickInputs::default())
}

/// The marks of the chunk at `position` as it would be saved.
fn saved_marks(region: &Region, position: ChunkPos) -> Vec<InChunk> {
    let saved = region.saved_chunk(position).expect("the chunk is loaded");
    saved.marks().to_vec()
}

fn level_at(region: &Region, position: BlockPos) -> Option<u8> {
    let state = block_at(region, position)?;
    if !is_class(state, BlockClass::LiquidBlock) {
        return None;
    }
    state.property("level").and_then(|level| level.parse().ok())
}

fn with(state: BlockState, property: &str, value: &str) -> BlockState {
    state
        .with_property(property, value)
        .unwrap_or_else(|| panic!("{state:?} has {property}={value}"))
}

/// The id of water among the fluids, as a fluid tick names it.
fn water_fluid() -> u8 {
    let index = clustine_data::FLUIDS
        .iter()
        .position(|name| *name == "minecraft:water")
        .expect("water is a fluid");
    u8::try_from(index).expect("a fluid id fits in a byte")
}

/// A tick of water at `position`, kept with `delay` left.
fn water_tick(position: BlockPos, delay: i32) -> ScheduledTick {
    ScheduledTick {
        at: in_chunk(position),
        of: Ticked::Fluid(water_fluid()),
        delay,
        priority: 0,
    }
}

/// The positions whose block a tick set, each with the state, in the order it did.
fn set_in(output: &TickOutput) -> Vec<(BlockPos, BlockState)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockChanged { position, state } => Some((*position, *state)),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// A. Post-processing (section 6 and 6a)
// ---------------------------------------------------------------------------------------

/// A chunk at the edge of what is loaded does not tick and is not post-processed:
/// tick after tick its marks stay, its marked blocks keep their shape and its marked
/// water does not move. When its missing neighbours come, it is post-processed in the
/// very tick it starts ticking.
#[test]
fn a_marked_chunk_is_not_post_processed_before_it_ticks_and_is_as_it_starts() {
    let edge = ChunkPos::new(1, 0);
    let (fence, stone, water) = (at(20, -60, 4), at(21, -60, 4), at(24, -60, 10));
    let chunk = marked(
        edge,
        &[
            (fence, blocks::OAK_FENCE),
            (stone, blocks::STONE),
            (water, blocks::WATER),
        ],
        &[],
        &[fence, water],
    );
    let (mut region, output) = loaded_with(&around_the_origin(), vec![(edge, chunk)]);
    assert_eq!(output.marks_taken, []);
    for _ in 0..3 {
        let output = idle_tick(&mut region);
        assert_eq!(output.marks_taken, []);
        assert_eq!(block_at(&region, fence), Some(blocks::OAK_FENCE));
        assert_eq!(block_at(&region, at(25, -60, 10)), Some(blocks::AIR));
        assert_eq!(
            saved_marks(&region, edge),
            [in_chunk(fence), in_chunk(water)]
        );
    }

    let more: Vec<_> = chunks((2, 2), (-1, 1))
        .into_iter()
        .map(|p| (p, floor()))
        .collect();
    let output = load(&mut region, more);
    assert_eq!(output.marks_taken, [edge]);
    assert_eq!(
        block_at(&region, fence),
        Some(with(blocks::OAK_FENCE, "east", "true"))
    );
    assert_eq!(level_at(&region, at(25, -60, 10)), Some(1));
    assert_eq!(saved_marks(&region, edge), []);
}

/// Once post-processed, a chunk is not post-processed again: not in the ticks after,
/// and not when it stops ticking and starts again. A fence that post-processing
/// connected and that was then set back by hand stays as it was set.
#[test]
fn a_chunk_is_post_processed_once_even_when_it_starts_ticking_again() {
    let origin = ChunkPos::new(0, 0);
    let (fence, stone) = (at(4, -60, 4), at(5, -60, 4));
    let chunk = marked(
        origin,
        &[(fence, blocks::OAK_FENCE), (stone, blocks::STONE)],
        &[],
        &[fence],
    );
    let (mut region, output) = loaded_with(&around_the_origin(), vec![(origin, chunk)]);
    assert_eq!(output.marks_taken, [origin]);
    assert_eq!(
        block_at(&region, fence),
        Some(with(blocks::OAK_FENCE, "east", "true"))
    );
    assert_eq!(idle_tick(&mut region).marks_taken, []);

    // Set back without telling anyone, so that only post-processing would connect it.
    let mut scratch = TickOutput::default();
    region.set_block_updated(
        fence,
        blocks::OAK_FENCE,
        super::super::updates::UPDATE_KNOWN_SHAPE | super::super::updates::UPDATE_CLIENTS,
        super::super::updates::UPDATE_LIMIT,
        &mut scratch,
    );
    assert_eq!(block_at(&region, fence), Some(blocks::OAK_FENCE));

    // The western column goes, so the origin stops ticking; then it comes back.
    let west = chunks((-1, -1), (-1, 1));
    checked(&mut region, &tickets(vec![], west.clone()));
    idle_tick(&mut region);
    assert!(region.chunk(ChunkPos::new(-1, 0)).is_none(), "let go");
    let output = load(
        &mut region,
        west.into_iter().map(|p| (p, floor())).collect(),
    );
    assert_eq!(output.marks_taken, []);
    assert_eq!(block_at(&region, fence), Some(blocks::OAK_FENCE));
    assert_eq!(saved_marks(&region, origin), []);
}

/// A fence and a wall at marks take their shape from their neighbours.
#[test]
fn post_processing_gives_a_fence_and_a_wall_their_connections() {
    let origin = ChunkPos::new(0, 0);
    let (fence, east_of_fence) = (at(4, -60, 4), at(5, -60, 4));
    let (wall, north_of_wall) = (at(10, -60, 10), at(10, -60, 9));
    let chunk = marked(
        origin,
        &[
            (fence, blocks::OAK_FENCE),
            (east_of_fence, blocks::STONE),
            (wall, blocks::COBBLESTONE_WALL),
            (north_of_wall, blocks::STONE),
        ],
        &[],
        &[fence, wall],
    );
    let (region, output) = loaded_with(&around_the_origin(), vec![(origin, chunk)]);
    assert_eq!(output.marks_taken, [origin]);
    let fence_now = block_at(&region, fence).unwrap();
    assert_eq!(fence_now.property("east"), Some("true"));
    assert_eq!(fence_now.property("west"), Some("false"));
    let wall_now = block_at(&region, wall).unwrap();
    assert!(is_class(wall_now, BlockClass::WallBlock), "{wall_now:?}");
    assert_eq!(wall_now.property("north"), Some("low"), "{wall_now:?}");
    assert_eq!(wall_now.property("south"), Some("none"), "{wall_now:?}");
}

/// Water at a mark flows in the tick the chunk is post-processed: its fluid ticks
/// there and then.
#[test]
fn water_at_a_mark_flows_as_the_chunk_is_post_processed() {
    let origin = ChunkPos::new(0, 0);
    let water = at(8, -60, 8);
    let chunk = marked(origin, &[(water, blocks::WATER)], &[], &[water]);
    let (region, output) = loaded_with(&around_the_origin(), vec![(origin, chunk)]);
    assert_eq!(output.marks_taken, [origin]);
    for beside in [at(7, -60, 8), at(9, -60, 8), at(8, -60, 7), at(8, -60, 9)] {
        assert_eq!(level_at(&region, beside), Some(1), "{beside:?}");
        assert!(set_in(&output).iter().any(|(p, _)| *p == beside));
    }
    assert_eq!(block_at(&region, water), Some(blocks::WATER));
}

/// A plant at a mark that cannot stay where it is goes, and goes without telling its
/// neighbours: a rail resting on it, which a neighbour update would remove, stays.
#[test]
fn a_plant_that_cannot_stay_goes_and_no_neighbour_is_told() {
    let origin = ChunkPos::new(0, 0);
    // A dandelion on stone cannot stay; the rail above it is not marked.
    let (plant, rail) = (at(6, -60, 6), at(6, -59, 6));
    let chunk = marked(
        origin,
        &[(plant, blocks::DANDELION), (rail, blocks::RAIL)],
        &[],
        &[plant],
    );
    let (mut region, output) = loaded_with(&around_the_origin(), vec![(origin, chunk)]);
    assert_eq!(output.marks_taken, [origin]);
    assert_eq!(block_at(&region, plant), Some(blocks::AIR));
    assert_eq!(block_at(&region, rail), Some(blocks::RAIL));
    // Nor is it told later: nothing was left to tell it.
    for _ in 0..3 {
        idle_tick(&mut region);
    }
    assert_eq!(block_at(&region, rail), Some(blocks::RAIL));
}

/// What post-processing changes is set without the neighbours' shapes being asked
/// again: a marked fence connects to an unmarked fence beside it, and that fence does
/// not connect back.
#[test]
fn post_processing_does_not_reshape_the_neighbours_of_what_it_changes() {
    let origin = ChunkPos::new(0, 0);
    let (other, fence, stone) = (at(3, -60, 4), at(4, -60, 4), at(5, -60, 4));
    let chunk = marked(
        origin,
        &[
            (other, blocks::OAK_FENCE),
            (fence, blocks::OAK_FENCE),
            (stone, blocks::STONE),
        ],
        &[],
        &[fence],
    );
    let (region, output) = loaded_with(&around_the_origin(), vec![(origin, chunk)]);
    assert_eq!(output.marks_taken, [origin]);
    let fence_now = block_at(&region, fence).unwrap();
    assert_eq!(fence_now.property("east"), Some("true"));
    assert_eq!(fence_now.property("west"), Some("true"));
    assert_eq!(block_at(&region, other), Some(blocks::OAK_FENCE));
    assert_eq!(set_in(&output), [(fence, fence_now)]);
}

/// A liquid block at a mark ticks its fluid but is not reshaped: a still source shut
/// in by stone, whose own tick does nothing, has no tick scheduled afterwards, where
/// `LiquidBlock.updateShape` of a source would schedule one.
#[test]
fn a_liquid_block_at_a_mark_is_not_reshaped() {
    let origin = ChunkPos::new(0, 0);
    let water = at(8, -60, 8);
    let walls = [
        at(7, -60, 8),
        at(9, -60, 8),
        at(8, -60, 7),
        at(8, -60, 9),
        at(8, -59, 8),
    ];
    let mut blocks_set: Vec<(BlockPos, BlockState)> =
        walls.iter().map(|wall| (*wall, blocks::STONE)).collect();
    blocks_set.push((water, blocks::WATER));
    let chunk = marked(origin, &blocks_set, &[], &[water]);
    let (region, output) = loaded_with(&around_the_origin(), vec![(origin, chunk)]);
    assert_eq!(output.marks_taken, [origin]);
    assert_eq!(block_at(&region, water), Some(blocks::WATER));
    let pending: Vec<_> = region
        .scheduled_ticks(origin)
        .into_iter()
        .filter(|tick| tick.at == in_chunk(water))
        .collect();
    assert_eq!(pending, []);
    assert_eq!(output.ticks_pending, []);
}

/// Post-processing comes after the tick's due fluid ticks (section 6a): a source whose
/// kept tick is due in L spreads onto a marked block of air, and post-processing then
/// finds water at the mark and ticks it, so the water is two blocks out by the end of L.
/// Run before the fluid ticks, the mark would have held air, and nothing would be.
#[test]
fn post_processing_comes_after_the_ticks_due_fluid_ticks() {
    let origin = ChunkPos::new(0, 0);
    let (source, mark, beyond) = (at(8, -60, 8), at(9, -60, 8), at(10, -60, 8));
    let chunk = marked(
        origin,
        &[(source, blocks::WATER)],
        &[water_tick(source, 0)],
        &[mark],
    );
    let (region, output) = loaded_with(&around_the_origin(), vec![(origin, chunk)]);
    assert_eq!(output.marks_taken, [origin]);
    assert_eq!(level_at(&region, mark), Some(1));
    assert_eq!(level_at(&region, beyond), Some(2));
}

/// The tick's output names each chunk post-processed in it once, in ascending order,
/// and no chunk that started ticking without marks; the saved forms of those it names
/// have no marks.
#[test]
fn marks_taken_names_each_chunk_post_processed_once_in_ascending_order() {
    let (a, b) = (ChunkPos::new(0, 0), ChunkPos::new(1, 0));
    let (fence_a, fence_b) = (at(4, -60, 4), at(20, -60, 4));
    let one = marked(a, &[(fence_a, blocks::OAK_FENCE)], &[], &[fence_a]);
    let two = marked(
        b,
        &[
            (fence_b, blocks::OAK_FENCE),
            (at(21, -60, 4), blocks::STONE),
        ],
        &[],
        &[fence_b, at(25, -60, 5), at(30, 0, 15)],
    );
    // Four chunks tick: (0, 0), (1, 0), (0, 1) and (1, 1); the last two have no marks.
    let (mut region, output) = loaded_with(&chunks((-1, 2), (-1, 2)), vec![(b, two), (a, one)]);
    assert_eq!(output.marks_taken, [a, b]);
    assert_eq!(saved_marks(&region, a), []);
    assert_eq!(saved_marks(&region, b), []);
    assert_eq!(idle_tick(&mut region).marks_taken, []);
}

/// A region restored with its chunks loaded, as the warm copies of a merge or a split
/// are, post-processes a marked chunk among them in its first tick: the chunk starts
/// ticking in it there.
#[test]
fn a_region_restored_with_a_marked_chunk_and_its_neighbours_post_processes_it_at_once() {
    let origin = ChunkPos::new(0, 0);
    let (fence, stone) = (at(4, -60, 4), at(5, -60, 4));
    let chunks: Vec<(ChunkPos, Chunk)> = around_the_origin()
        .into_iter()
        .map(|p| {
            let chunk = if p == origin {
                marked(
                    p,
                    &[(fence, blocks::OAK_FENCE), (stone, blocks::STONE)],
                    &[],
                    &[fence],
                )
            } else {
                floor()
            };
            (p, chunk)
        })
        .collect();
    let mut region = pinned_with(ChunkArea::EVERYWHERE, 0, chunks);
    let output = idle_tick(&mut region);
    assert_eq!(output.marks_taken, [origin]);
    assert_eq!(
        block_at(&region, fence),
        Some(with(blocks::OAK_FENCE, "east", "true"))
    );
}

// ---------------------------------------------------------------------------------------
// B. Seams (section 5), in one region
// ---------------------------------------------------------------------------------------

/// The wall of the torches and ladders of these tests: at the eastern edge of chunk
/// (1, 0), with the chunk east of it, (2, 0), not loaded.
const NEAR: ChunkPos = ChunkPos::new(1, 0);
const FAR: ChunkPos = ChunkPos::new(2, 0);

fn wall_torch(facing: &str) -> BlockState {
    with(blocks::WALL_TORCH, "facing", facing)
}

/// The entries of the chunk at `position` that reach `target`, of `kind`.
fn entries(region: &Region, position: ChunkPos, target: BlockPos, kind: SeamKind) -> Vec<Seam> {
    region
        .seams(position)
        .into_iter()
        .filter(|seam| seam.at == target && seam.kind == kind)
        .collect()
}

/// A wall torch and a ladder whose wall goes while their own chunk is not loaded stay
/// where they were, and go when the chunk comes, in the tick it comes.
#[test]
fn a_torch_and_a_ladder_whose_wall_went_while_their_chunk_was_away_go_when_it_comes() {
    let (wall, torch) = (at(31, -60, 8), at(32, -60, 8));
    let (other_wall, ladder) = (at(31, -60, 12), at(32, -60, 12));
    let (mut region, _) = loaded_with(
        &around_the_origin(),
        vec![(
            NEAR,
            with_blocks(NEAR, &[(wall, blocks::STONE), (other_wall, blocks::STONE)]),
        )],
    );
    let mut output = TickOutput::default();
    region.set_block(wall, blocks::AIR, &mut output);
    region.set_block(other_wall, blocks::AIR, &mut output);
    idle_tick(&mut region);

    let far = with_blocks(
        FAR,
        &[
            (torch, wall_torch("east")),
            (ladder, with(blocks::LADDER, "facing", "east")),
        ],
    );
    let output = load(&mut region, vec![(FAR, far)]);
    assert_eq!(block_at(&region, torch), Some(blocks::AIR));
    assert_eq!(block_at(&region, ladder), Some(blocks::AIR));
    let gone: Vec<_> = set_in(&output).into_iter().map(|(p, _)| p).collect();
    assert!(gone.contains(&torch) && gone.contains(&ladder), "{gone:?}");
}

/// An update that cannot be made is kept in the chunk of the block that changed, with
/// the block it reaches, the direction from that block to the one that changed, its
/// kind, the update's flags and the depth left; a shape update with the block that
/// changed as it is now, a neighbour told with the block that was there, which is the
/// block the game's `Level.setBlock` tells the neighbours of.
/// A player's break is `UPDATE_ALL` at the full depth: the shape update it makes goes
/// with `UPDATE_CLIENTS` (the game takes out `UPDATE_NEIGHBORS` and
/// `UPDATE_SUPPRESS_DROPS`) and one level down.
#[test]
fn an_entry_is_kept_in_the_chunk_of_the_block_that_changed_with_what_the_update_was() {
    let (wall, torch) = (at(31, -60, 8), at(32, -60, 8));
    let (mut region, _) = loaded_with(
        &around_the_origin(),
        vec![(NEAR, with_blocks(NEAR, &[(wall, blocks::STONE)]))],
    );
    let mut output = TickOutput::default();
    region.set_block(wall, blocks::AIR, &mut output);

    let shape = entries(&region, NEAR, torch, SeamKind::Shape);
    assert_eq!(
        shape,
        [Seam {
            at: torch,
            direction: Face::West as u8,
            kind: SeamKind::Shape,
            source: blocks::AIR,
            flags: super::super::updates::UPDATE_CLIENTS,
            depth: super::super::updates::UPDATE_LIMIT - 1,
        }]
    );
    let told = entries(&region, NEAR, torch, SeamKind::Neighbour);
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0].direction, Face::West as u8);
    assert_eq!(told[0].source, blocks::STONE);
    // Kept with the chunk that changed, and in its saved form.
    let saved = region.saved_chunk(NEAR).unwrap();
    assert!(saved.seams().contains(&shape[0]));
    assert!(saved.seams().contains(&told[0]));
    for position in around_the_origin() {
        if position != NEAR {
            assert_eq!(region.seams(position), [], "{position:?}");
        }
    }
}

/// An update of a block that is loaded but has a neighbour in a chunk that is not is
/// kept too: the block beside the wall inside its own chunk, whose east neighbour is
/// in the chunk not loaded, is reached by updates that wait.
#[test]
fn an_update_of_a_loaded_block_beside_a_chunk_not_loaded_is_kept_too() {
    let wall = at(31, -60, 8);
    let (mut region, _) = loaded_with(
        &around_the_origin(),
        vec![(NEAR, with_blocks(NEAR, &[(wall, blocks::STONE)]))],
    );
    let mut output = TickOutput::default();
    region.set_block(wall, blocks::AIR, &mut output);
    let north = at(31, -60, 7);
    let shape = entries(&region, NEAR, north, SeamKind::Shape);
    assert_eq!(shape.len(), 1, "{:?}", region.seams(NEAR));
    assert_eq!(shape[0].direction, Face::South as u8);
    // The block west of the wall has all its neighbours loaded: nothing waits for it.
    assert_eq!(entries(&region, NEAR, at(30, -60, 8), SeamKind::Shape), []);
    assert_eq!(
        entries(&region, NEAR, at(30, -60, 8), SeamKind::Neighbour),
        []
    );
}

/// One entry per block reached, direction and kind: a later update of the same key
/// replaces the earlier, and the latest is what is kept.
#[test]
fn a_later_update_with_the_same_key_replaces_the_entry() {
    let (wall, torch) = (at(31, -60, 8), at(32, -60, 8));
    let (mut region, _) = loaded_with(
        &around_the_origin(),
        vec![(NEAR, with_blocks(NEAR, &[(wall, blocks::STONE)]))],
    );
    let mut output = TickOutput::default();
    region.set_block(wall, blocks::AIR, &mut output);
    let keys_after_one: Vec<_> = region.seams(NEAR).iter().map(Seam::key).collect();
    region.set_block(wall, blocks::DIRT, &mut output);
    let keys_after_two: Vec<_> = region.seams(NEAR).iter().map(Seam::key).collect();
    assert_eq!(keys_after_one, keys_after_two);
    let shape = entries(&region, NEAR, torch, SeamKind::Shape);
    assert_eq!(shape.len(), 1);
    assert_eq!(shape[0].source, blocks::DIRT);
    let told = entries(&region, NEAR, torch, SeamKind::Neighbour);
    assert_eq!(told.len(), 1);
    // Told of the air the dirt took the place of.
    assert_eq!(told[0].source, blocks::AIR);
}

/// No entry is kept for a position outside the world's height: a block at the top of
/// the world and one at its bottom, at the chunk's edge, change; the updates that
/// would reach above the top and below the bottom are not kept, those beside are.
#[test]
fn no_entry_is_kept_for_a_position_outside_the_world() {
    let (top, bottom) = (at(31, 319, 8), at(31, -64, 3));
    let (mut region, _) = loaded_with(
        &around_the_origin(),
        vec![(
            NEAR,
            with_blocks(NEAR, &[(top, blocks::STONE), (bottom, blocks::STONE)]),
        )],
    );
    let mut output = TickOutput::default();
    region.set_block(top, blocks::AIR, &mut output);
    region.set_block(bottom, blocks::AIR, &mut output);
    let seams = region.seams(NEAR);
    assert!(
        seams.iter().all(|seam| (-64..320).contains(&seam.at.y)),
        "{seams:?}"
    );
    assert_eq!(
        entries(&region, NEAR, at(32, 319, 8), SeamKind::Shape).len(),
        1
    );
    assert_eq!(
        entries(&region, NEAR, at(32, -64, 3), SeamKind::Shape).len(),
        1
    );
}

/// When the chunks come, the entries are made per chunk in the order of their keys,
/// the chunks in ascending position, whatever order they were made in, and then
/// forgotten: nothing is made again in the ticks after.
#[test]
fn entries_are_made_in_key_order_chunks_ascending_and_forgotten() {
    // Two torches east of chunk (1, 0), one west of chunk (-1, 0).
    let (east_wall, east_torch) = (at(31, -60, 8), at(32, -60, 8));
    let (east_wall_2, east_torch_2) = (at(31, -60, 4), at(32, -60, 4));
    let (west_wall, west_torch) = (at(-16, -60, 8), at(-17, -60, 8));
    let west_near = ChunkPos::new(-1, 0);
    let west_far = ChunkPos::new(-2, 0);
    let (mut region, _) = loaded_with(
        &around_the_origin(),
        vec![
            (
                NEAR,
                with_blocks(
                    NEAR,
                    &[(east_wall, blocks::STONE), (east_wall_2, blocks::STONE)],
                ),
            ),
            (
                west_near,
                with_blocks(west_near, &[(west_wall, blocks::STONE)]),
            ),
        ],
    );
    let mut scratch = TickOutput::default();
    // Made in the opposite of the order they are to be made in.
    region.set_block(east_wall, blocks::AIR, &mut scratch);
    region.set_block(east_wall_2, blocks::AIR, &mut scratch);
    region.set_block(west_wall, blocks::AIR, &mut scratch);
    assert!(!region.seams(NEAR).is_empty());
    assert!(!region.seams(west_near).is_empty());
    let output = load(
        &mut region,
        vec![
            (
                west_far,
                with_blocks(west_far, &[(west_torch, wall_torch("west"))]),
            ),
            (
                FAR,
                with_blocks(
                    FAR,
                    &[
                        (east_torch, wall_torch("east")),
                        (east_torch_2, wall_torch("east")),
                    ],
                ),
            ),
        ],
    );
    let gone: Vec<BlockPos> = set_in(&output)
        .into_iter()
        .filter(|(p, state)| {
            *state == blocks::AIR && [west_torch, east_torch, east_torch_2].contains(p)
        })
        .map(|(p, _)| p)
        .collect();
    assert_eq!(gone, [west_torch, east_torch_2, east_torch]);
    assert_eq!(region.seams(NEAR), []);
    assert_eq!(region.seams(west_near), []);
    assert_eq!(output.seams_changed, [west_near, NEAR]);

    let after = idle_tick(&mut region);
    assert_eq!(after.seams_changed, []);
    assert_eq!(set_in(&after), []);
}

/// The tick's output names the chunk whose entries changed: in the tick a player's
/// break keeps them, and in the tick they are made. In between it is not named.
#[test]
fn seams_changed_names_the_chunk_when_entries_are_added_and_when_they_run() {
    let (wall, torch) = (at(31, -60, 8), at(32, -60, 8));
    let (mut region, _) = loaded_with(
        &around_the_origin(),
        vec![(NEAR, with_blocks(NEAR, &[(wall, blocks::STONE)]))],
    );
    let output = checked(&mut region, &moves(vec![walk(1, 29.5, 8.5)]));
    assert_eq!(output.seams_changed, []);
    let output = checked(&mut region, &moves(vec![dig(1, 31, -60, 8, 1)]));
    assert_eq!(block_at(&region, wall), Some(blocks::AIR), "dug");
    assert_eq!(output.seams_changed, [NEAR]);
    assert_eq!(idle_tick(&mut region).seams_changed, []);

    let output = load(
        &mut region,
        vec![(FAR, with_blocks(FAR, &[(torch, wall_torch("east"))]))],
    );
    assert_eq!(block_at(&region, torch), Some(blocks::AIR));
    assert_eq!(output.seams_changed, [NEAR]);
}

// ---------------------------------------------------------------------------------------
// C. Seams between two regions, and a merge that joins them (section 5)
// ---------------------------------------------------------------------------------------

/// Region 0 holds the chunks with x below 1, region 1 those from 1 on.
const WESTERN: ChunkArea = ChunkArea {
    min_x: None,
    max_x: Some(1),
};
const EASTERN: ChunkArea = ChunkArea {
    min_x: Some(1),
    max_x: None,
};

/// A region pinned to `area` that has never run, restored with viewer's tickets on
/// `chunks` and those chunks loaded.
fn pinned_with(area: ChunkArea, ids: u32, chunks: Vec<(ChunkPos, Chunk)>) -> Region {
    let tickets: Vec<_> = chunks
        .iter()
        .map(|(position, _)| (*position, Ticket::Viewer))
        .collect();
    // A region knows it holds what the store says it holds as it is opened.
    let holdings = Holdings {
        held: chunks.iter().map(|(position, _)| *position).collect(),
        pinned: vec![area],
    };
    let state = RegionState::new(EntityIds::block(ids).unwrap());
    Region::restore_loaded(config(), state, holdings, &tickets, chunks, Vec::new())
}

/// A break at the line between two regions reaches a torch of the other region: the
/// update is kept in the chunk of the region that broke, the torch stays in the other,
/// and once a merge gives one region both chunks the update is made and the torch goes.
#[test]
fn an_update_across_the_line_between_two_regions_waits_for_a_merge_and_is_made_then() {
    let (wall, torch) = (at(15, -60, 8), at(16, -60, 8));
    let line = ChunkPos::new(0, 0);
    let beyond = ChunkPos::new(1, 0);
    let western_chunks: Vec<_> = chunks((-2, 0), (-1, 1))
        .into_iter()
        .map(|p| {
            let chunk = if p == line {
                with_blocks(p, &[(wall, blocks::STONE)])
            } else {
                floor()
            };
            (p, chunk)
        })
        .collect();
    let eastern_chunks: Vec<_> = chunks((1, 2), (-1, 1))
        .into_iter()
        .map(|p| {
            let chunk = if p == beyond {
                with_blocks(p, &[(torch, wall_torch("east"))])
            } else {
                floor()
            };
            (p, chunk)
        })
        .collect();
    let mut west = pinned_with(WESTERN, 0, western_chunks);
    let mut east = pinned_with(EASTERN, 1, eastern_chunks);
    idle_tick(&mut west);
    idle_tick(&mut east);
    // The western region has asked for the chunk across the line and had no answer, so
    // the update waits as a seam entry in the chunk it came from; into a chunk it knew
    // nothing of and did not want it would be an effect for the eastern region
    // (ADR-0025, sections 4 and 6).
    checked(&mut west, &tickets(vec![beyond], vec![]));
    assert_eq!(west.knowledge(beyond), Knowledge::Asked);

    let mut scratch = TickOutput::default();
    west.set_block(wall, blocks::AIR, &mut scratch);
    let output = idle_tick(&mut west);
    assert_eq!(output.seams_changed, []);
    let kept = entries(&west, line, torch, SeamKind::Shape);
    assert_eq!(kept.len(), 1, "{:?}", west.seams(line));
    assert!(west.saved_chunk(line).unwrap().seams().contains(&kept[0]));
    for _ in 0..3 {
        idle_tick(&mut east);
        idle_tick(&mut west);
    }
    assert_eq!(block_at(&east, torch), Some(wall_torch("east")));

    // The western region absorbs the eastern one and loads both sides.
    let theirs: Vec<(ChunkPos, Chunk)> = chunks((1, 2), (-1, 1))
        .into_iter()
        .map(|p| (p, east.saved_chunk(p).expect("loaded in the east")))
        .collect();
    let state = west.absorb(RegionId(1), &east.state());
    let theirs_held: Vec<ChunkPos> = theirs.iter().map(|(p, _)| *p).collect();
    let mut ours = west.take_absorbed(state, &theirs_held, &[EASTERN]);
    assert!(
        ours.iter()
            .any(|(p, chunk)| *p == line && chunk.seams().contains(&kept[0])),
        "the chunk goes on with its seams"
    );
    ours.extend(theirs);
    let output = load(&mut west, ours);
    assert_eq!(block_at(&west, torch), Some(blocks::AIR));
    assert_eq!(entries(&west, line, torch, SeamKind::Shape), []);
    assert!(output.seams_changed.contains(&line));
}
