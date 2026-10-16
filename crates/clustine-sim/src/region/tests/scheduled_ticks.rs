//! Tests of the scheduled ticks a region runs (step B4 of ADR-0024, parts a and b:
//! sections 2, 3, 4 and 6a), written from the specification of the step by someone
//! who did not write the code.
//!
//! Player 1 stands at (0.5, -60, 0.5) on a stone floor at y = -61 in chunk (0, 0).
//! The chunks a test loads are all held (the region is pinned to the whole world)
//! and lie within three chunks of the origin, which the fixtures know from the start.
//! "L" in a comment is the tick in which a test's chunks were loaded, and so the tick
//! in which a chunk whose eight neighbours came with it starts ticking.

use clustine_blocks::data::is_class;
use clustine_data::BlockClass;
use clustine_world::{ScheduledTick, Ticked};

use super::*;

fn at(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

/// The state the game would print as `text`.
fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a block state"))
}

/// What a tick of `block` is of: the block, by its id.
fn of(block: BlockState) -> Ticked {
    let id = block.block_id().expect("a block state");
    Ticked::Block(u16::try_from(id).expect("a block id fits in 16 bits"))
}

/// A tick of `block` at `position` as a chunk keeps it: with the delay left.
fn kept(position: BlockPos, block: BlockState, delay: i32, priority: i8) -> ScheduledTick {
    ScheduledTick {
        at: in_chunk(position),
        of: of(block),
        delay,
        priority,
    }
}

fn block_at(region: &Region, position: BlockPos) -> Option<BlockState> {
    let (x, z) = position.in_chunk();
    region.chunk(position.chunk())?.get(x, position.y, z)
}

/// The chunks from `x` to `x_end` and from `z` to `z_end`, both ends included.
fn chunks(x: (i32, i32), z: (i32, i32)) -> Vec<ChunkPos> {
    (x.0..=x.1)
        .flat_map(|x| (z.0..=z.1).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// The 3×3 around the origin, with which the origin's chunk ticks and no other.
fn around_the_origin() -> Vec<ChunkPos> {
    chunks((-1, 1), (-1, 1))
}

/// The floor, with `blocks` set (by their position in the world) and `ticks` kept.
fn floor_of(
    position: ChunkPos,
    blocks: &[(BlockPos, BlockState)],
    ticks: &[ScheduledTick],
) -> Chunk {
    let mut chunk = floor();
    for (at, state) in blocks {
        assert_eq!(at.chunk(), position, "{at:?} is not in {position:?}");
        let (x, z) = at.in_chunk();
        chunk.set(x, at.y, z, *state);
    }
    chunk.set_ticks(ticks.to_vec());
    chunk
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

/// A region that player 1 has entered, with `chunks` loaded: the floor everywhere,
/// and the given chunk where one is given.
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

/// The 3×3 around the origin loaded, the origin's chunk with `blocks` and `ticks`.
fn ticking_with(
    blocks: &[(BlockPos, BlockState)],
    ticks: &[ScheduledTick],
) -> (Region, TickOutput) {
    let origin = ChunkPos::new(0, 0);
    loaded_with(
        &around_the_origin(),
        vec![(origin, floor_of(origin, blocks, ticks))],
    )
}

fn idle_tick(region: &mut Region) -> TickOutput {
    checked(region, &TickInputs::default())
}

/// The positions whose block a tick set to air, in the order it did.
fn emptied(output: &TickOutput) -> Vec<BlockPos> {
    block_changes(output)
        .into_iter()
        .filter_map(|event| match event {
            RegionEvent::BlockChanged { position, state } if state == blocks::AIR => Some(position),
            _ => None,
        })
        .collect()
}

/// The ticks the saved form of the chunk at `position` keeps, as (where, delay).
fn saved_ticks(region: &Region, position: ChunkPos) -> Vec<(InChunk, i32)> {
    let saved = region.saved_chunk(position).expect("the chunk is loaded");
    saved
        .ticks()
        .iter()
        .map(|tick| (tick.at, tick.delay))
        .collect()
}

fn in_chunk(position: BlockPos) -> InChunk {
    let (x, z) = position.in_chunk();
    InChunk::new(x, position.y, z)
}

// ---------------------------------------------------------------------------------------
// A. Which chunks tick
// ---------------------------------------------------------------------------------------

/// A tick a chunk brings runs as many ticks after the chunk starts ticking as the delay
/// it was kept with, and not a tick earlier.
#[test]
fn a_kept_tick_runs_as_many_ticks_after_its_chunk_starts_ticking_as_its_delay() {
    let sand = at(5, -55, 5);
    let (mut region, _) = ticking_with(&[(sand, blocks::SAND)], &[kept(sand, blocks::SAND, 3, 0)]);
    for _ in 0..2 {
        idle_tick(&mut region);
        assert_eq!(block_at(&region, sand), Some(blocks::SAND));
    }
    let output = idle_tick(&mut region);
    assert_eq!(block_at(&region, sand), Some(blocks::AIR));
    assert_eq!(block_at(&region, at(5, -60, 5)), Some(blocks::SAND));
    assert_eq!(emptied(&output), [sand]);
}

/// A kept tick of delay 0 is due in the very tick its chunk starts ticking, and the
/// chunks that start ticking come before the due ticks are collected (section 6a).
#[test]
fn a_kept_tick_of_delay_zero_runs_in_the_tick_its_chunk_starts_ticking() {
    let sand = at(5, -55, 5);
    let (region, output) = ticking_with(&[(sand, blocks::SAND)], &[kept(sand, blocks::SAND, 0, 0)]);
    assert_eq!(block_at(&region, sand), Some(blocks::AIR));
    assert_eq!(block_at(&region, at(5, -60, 5)), Some(blocks::SAND));
    assert_eq!(emptied(&output), [sand]);
}

/// A chunk at the edge of what is loaded misses a neighbour, so it does not tick; its
/// tick waits, kept as it was, and runs in the tick the missing neighbours come.
#[test]
fn a_chunk_at_the_edge_of_the_loaded_three_by_three_does_not_tick_until_its_neighbours_come() {
    let east = ChunkPos::new(1, 0);
    let sand = at(20, -55, 5);
    let chunk = floor_of(
        east,
        &[(sand, blocks::SAND)],
        &[kept(sand, blocks::SAND, 0, 0)],
    );
    let (mut region, _) = loaded_with(&around_the_origin(), vec![(east, chunk)]);
    for _ in 0..5 {
        let output = idle_tick(&mut region);
        assert_eq!(output.ticks_changed, []);
    }
    assert_eq!(block_at(&region, sand), Some(blocks::SAND));
    assert_eq!(saved_ticks(&region, east), [(in_chunk(sand), 0)]);

    let output = load(
        &mut region,
        chunks((2, 2), (-1, 1))
            .into_iter()
            .map(|position| (position, floor()))
            .collect(),
    );
    assert_eq!(block_at(&region, sand), Some(blocks::AIR));
    assert_eq!(emptied(&output), [sand]);
}

/// A chunk loaded alone keeps its tick, which does not age while the chunk does not
/// tick (section 3); once its neighbours come, it runs its delay later.
#[test]
fn a_tick_kept_in_a_chunk_without_its_neighbours_waits_and_then_runs_with_its_delay() {
    let origin = ChunkPos::new(0, 0);
    let sand = at(5, -55, 5);
    let chunk = floor_of(
        origin,
        &[(sand, blocks::SAND)],
        &[kept(sand, blocks::SAND, 2, 0)],
    );
    let (mut region, _) = loaded_with(&[origin], vec![(origin, chunk)]);
    for _ in 0..10 {
        idle_tick(&mut region);
    }
    assert_eq!(block_at(&region, sand), Some(blocks::SAND));
    assert_eq!(saved_ticks(&region, origin), [(in_chunk(sand), 2)]);

    let neighbours = around_the_origin()
        .into_iter()
        .filter(|position| *position != origin)
        .map(|position| (position, floor()))
        .collect();
    load(&mut region, neighbours);
    idle_tick(&mut region);
    assert_eq!(
        block_at(&region, sand),
        Some(blocks::SAND),
        "a tick runs early"
    );
    let output = idle_tick(&mut region);
    assert_eq!(emptied(&output), [sand]);
    assert_eq!(block_at(&region, at(5, -60, 5)), Some(blocks::SAND));
}

// ---------------------------------------------------------------------------------------
// C. The delay left
// ---------------------------------------------------------------------------------------

/// A tick of delay 2 scheduled into a loaded chunk that does not tick is kept as 1, as
/// if written at the end of the tick it was scheduled in, and stays so; when the chunk
/// starts ticking in L it is due in L + 1.
#[test]
fn a_tick_scheduled_into_a_chunk_that_does_not_tick_is_kept_with_one_less_than_its_delay() {
    let origin = ChunkPos::new(0, 0);
    let (sand, stone) = (at(2, -59, 3), at(2, -60, 3));
    let chunk = floor_of(origin, &[(stone, blocks::STONE), (sand, blocks::SAND)], &[]);
    let (mut region, _) = loaded_with(&[origin], vec![(origin, chunk)]);
    let output = checked(&mut region, &moves(vec![dig(1, 2, -60, 3, 1)]));
    assert_eq!(output.ticks_changed, [origin]);
    assert_eq!(saved_ticks(&region, origin), [(in_chunk(sand), 1)]);
    for _ in 0..4 {
        idle_tick(&mut region);
        assert_eq!(saved_ticks(&region, origin), [(in_chunk(sand), 1)]);
    }
    assert_eq!(block_at(&region, sand), Some(blocks::SAND));

    let neighbours = around_the_origin()
        .into_iter()
        .filter(|position| *position != origin)
        .map(|position| (position, floor()))
        .collect();
    let output = load(&mut region, neighbours);
    assert_eq!(emptied(&output), [], "a tick runs early");
    let output = idle_tick(&mut region);
    assert_eq!(emptied(&output), [sand]);
}

/// Written at the end of tick T, a tick due in D is kept as D − T − 1: a tick of
/// delay 2 scheduled in T is kept as 1 at the end of T, as 0 at the end of T + 1, and
/// has run at T + 2.
#[test]
fn a_queued_tick_is_saved_with_the_delay_left_at_the_end_of_each_tick() {
    let origin = ChunkPos::new(0, 0);
    let (sand, stone) = (at(2, -58, 3), at(2, -59, 3));
    let (mut region, _) = ticking_with(&[(stone, blocks::STONE), (sand, blocks::SAND)], &[]);
    checked(&mut region, &moves(vec![dig(1, 2, -59, 3, 1)]));
    assert_eq!(saved_ticks(&region, origin), [(in_chunk(sand), 1)]);
    idle_tick(&mut region);
    assert_eq!(saved_ticks(&region, origin), [(in_chunk(sand), 0)]);
    idle_tick(&mut region);
    let left: Vec<_> = saved_ticks(&region, origin)
        .into_iter()
        .filter(|(position, _)| *position == in_chunk(sand))
        .collect();
    assert_eq!(left, []);
    assert_eq!(block_at(&region, sand), Some(blocks::AIR));
}

/// The saved form lists the ticks in the order they would run: by due, then by
/// priority, then in the order they came or were scheduled.
#[test]
fn a_saved_chunk_lists_its_ticks_in_queue_order() {
    let origin = ChunkPos::new(0, 0);
    let (a, b, c) = (at(9, -60, 9), at(10, -60, 9), at(11, -60, 9));
    let ticks = [
        kept(a, blocks::STONE, 4, 0),
        kept(b, blocks::STONE, 4, -1),
        kept(c, blocks::STONE, 2, 5),
    ];
    let blocks = [(a, blocks::STONE), (b, blocks::STONE), (c, blocks::STONE)];
    let (region, _) = ticking_with(&blocks, &ticks);
    assert_eq!(
        saved_ticks(&region, origin),
        [(in_chunk(c), 1), (in_chunk(b), 3), (in_chunk(a), 3)]
    );

    // Two scheduled in one tick, in the order the digs came, which is not the order of
    // their positions.
    let (first, second) = (at(3, -59, 3), at(2, -59, 3));
    let mut region = ticking_floor_with(&[
        (first, blocks::STONE),
        (first.offset(0, 1, 0), blocks::SAND),
        (second, blocks::STONE),
        (second.offset(0, 1, 0), blocks::SAND),
    ]);
    checked(
        &mut region,
        &moves(vec![dig(1, 3, -59, 3, 1), dig(1, 2, -59, 3, 2)]),
    );
    assert_eq!(
        saved_ticks(&region, origin),
        [
            (in_chunk(first.offset(0, 1, 0)), 1),
            (in_chunk(second.offset(0, 1, 0)), 1)
        ]
    );
}

/// A neighbour let go stops the chunk ticking: its tick is written into it with the
/// delay left then, the chunk is named among those whose ticks changed, and the tick
/// waits until the neighbour is back.
#[test]
fn a_chunk_whose_neighbour_is_let_go_keeps_its_ticks_and_runs_them_when_it_ticks_again() {
    let origin = ChunkPos::new(0, 0);
    let corner = ChunkPos::new(1, 1);
    let (sand, stone) = (at(2, -58, 3), at(2, -59, 3));
    let (mut region, _) = ticking_with(&[(stone, blocks::STONE), (sand, blocks::SAND)], &[]);
    // Scheduled in T, due in T + 2.
    checked(&mut region, &moves(vec![dig(1, 2, -59, 3, 1)]));
    let output = checked(&mut region, &tickets(vec![], vec![corner]));
    assert!(region.chunk(corner).is_none(), "the corner is let go");
    assert_eq!(output.ticks_changed, [origin]);
    for _ in 0..5 {
        let output = idle_tick(&mut region);
        assert_eq!(emptied(&output), []);
        assert_eq!(output.ticks_changed, []);
    }
    assert_eq!(saved_ticks(&region, origin), [(in_chunk(sand), 0)]);
    assert_eq!(region.chunk(origin).unwrap().ticks().len(), 1);

    let output = load(&mut region, vec![(corner, floor())]);
    assert_eq!(emptied(&output), [sand]);
}

/// A chunk that stops ticking with nothing pending is as it was.
#[test]
fn a_chunk_that_stops_ticking_with_no_ticks_pending_is_not_named() {
    let (mut region, _) = ticking_with(&[], &[]);
    let output = checked(&mut region, &tickets(vec![], vec![ChunkPos::new(-1, -1)]));
    assert_eq!(output.ticks_changed, []);
    assert_eq!(region.saved_chunk(ChunkPos::new(0, 0)).unwrap().ticks(), []);
}

/// A merge hands the chunks out as the store would have them, their ticks written in;
/// loaded again, they carry on with the delays left.
#[test]
fn a_merge_hands_out_chunks_with_their_ticks_written_in() {
    let origin = ChunkPos::new(0, 0);
    let (sand, stone) = (at(2, -58, 3), at(2, -59, 3));
    let (mut region, _) = ticking_with(&[(stone, blocks::STONE), (sand, blocks::SAND)], &[]);
    checked(&mut region, &moves(vec![dig(1, 2, -59, 3, 1)]));
    let other = RegionState::new(EntityIds::block(7).unwrap());
    let state = region.absorb(RegionId(7), &other);
    let handed = region.take_absorbed(state, &[], &[]);
    let (_, chunk) = handed
        .iter()
        .find(|(position, _)| *position == origin)
        .expect("the origin's chunk was loaded");
    assert_eq!(chunk.ticks(), [kept(sand, blocks::SAND, 1, 0)]);
    let others: Vec<_> = handed
        .iter()
        .filter(|(position, chunk)| *position != origin && !chunk.ticks().is_empty())
        .collect();
    assert!(others.is_empty(), "{others:?}");

    // Loaded again, all nine at once: due one tick after they start ticking.
    let output = load(&mut region, handed);
    assert_eq!(emptied(&output), []);
    let output = idle_tick(&mut region);
    assert_eq!(emptied(&output), [sand]);
}

/// A split hands out the chunks that stay and the part's, each with its ticks
/// written in as the delays left at the split.
#[test]
fn a_split_hands_out_chunks_with_their_ticks_written_in() {
    let origin = ChunkPos::new(0, 0);
    let going = ChunkPos::new(2, 0);
    let (here, there) = (at(9, -61, 9), at(40, -61, 9));
    let given = vec![
        (
            origin,
            floor_of(origin, &[], &[kept(here, blocks::STONE, 5, 0)]),
        ),
        (
            going,
            floor_of(going, &[], &[kept(there, blocks::STONE, 5, 0)]),
        ),
    ];
    // Both tick: x from -1 to 3.
    let (mut region, _) = loaded_with(&chunks((-1, 3), (-1, 1)), given);
    // Player 1 walks into chunk (3, 0), which takes (2, 0) with them; (1, 0) is as near
    // the home chunk as them and stays.
    checked(&mut region, &moves(vec![walk(1, 56.5, 8.5)]));
    let splitting = region
        .split(&[ChunkPos::new(3, 0)], RegionId(9), &[])
        .expect("player 1 goes");
    assert!(splitting.chunks.contains(&going) && !splitting.chunks.contains(&origin));
    let (kept_chunks, part) = region.take_split(splitting, &[]);
    // Due in L + 5, written at the end of L + 1: 3 left.
    let of_here = kept_chunks.iter().find(|(position, _)| *position == origin);
    assert_eq!(
        of_here.expect("the origin stays").1.ticks(),
        [kept(here, blocks::STONE, 3, 0)]
    );
    let of_there = part.chunks.iter().find(|(position, _)| *position == going);
    assert_eq!(
        of_there.expect("(2, 0) goes").1.ticks(),
        [kept(there, blocks::STONE, 3, 0)]
    );
}

// ---------------------------------------------------------------------------------------
// B. The queues and their order
// ---------------------------------------------------------------------------------------

/// Two ticks due together, of the same priority and both scheduled, run in the order
/// they were scheduled, whichever position is the lower.
#[test]
fn two_ticks_due_together_run_in_the_order_they_were_scheduled() {
    for (first, second) in [(3, 2), (2, 3)] {
        let pillar = |x: i32| {
            [
                (at(x, -59, 3), blocks::STONE),
                (at(x, -58, 3), blocks::SAND),
            ]
        };
        let blocks: Vec<_> = pillar(first).into_iter().chain(pillar(second)).collect();
        let mut region = ticking_floor_with(&blocks);
        checked(
            &mut region,
            &moves(vec![dig(1, first, -59, 3, 1), dig(1, second, -59, 3, 2)]),
        );
        idle_tick(&mut region);
        let output = idle_tick(&mut region);
        assert_eq!(
            emptied(&output),
            [at(first, -58, 3), at(second, -58, 3)],
            "dug {first} first"
        );
    }
}

/// A tick that came with a chunk as it started ticking runs before one scheduled since
/// that is due in the same tick with the same priority, though the scheduled one's
/// position is the lower.
#[test]
fn a_tick_that_came_with_its_chunk_runs_before_one_scheduled_since_at_the_same_due() {
    let stored = at(9, -55, 9);
    let (scheduled, stone) = (at(2, -58, 3), at(2, -59, 3));
    let (mut region, _) = ticking_with(
        &[
            (stored, blocks::SAND),
            (stone, blocks::STONE),
            (scheduled, blocks::SAND),
        ],
        &[kept(stored, blocks::SAND, 3, 0)],
    );
    // In L + 1, with a delay of 2: due in L + 3, as the kept one.
    checked(&mut region, &moves(vec![dig(1, 2, -59, 3, 1)]));
    let output = idle_tick(&mut region);
    assert_eq!(emptied(&output), []);
    let output = idle_tick(&mut region);
    assert_eq!(emptied(&output), [stored, scheduled]);
}

/// Of two ticks due together, the one with the lower priority value runs first,
/// whatever order the chunk kept them in.
#[test]
fn of_two_ticks_due_together_the_lower_priority_value_runs_first() {
    let (a, b) = (at(5, -55, 5), at(6, -55, 5));
    let (mut region, _) = ticking_with(
        &[(a, blocks::SAND), (b, blocks::SAND)],
        &[kept(a, blocks::SAND, 1, 0), kept(b, blocks::SAND, 1, -1)],
    );
    let output = idle_tick(&mut region);
    assert_eq!(emptied(&output), [b, a]);
}

/// Ticks kept by the same chunk with the same due and priority run in the order the
/// chunk lists them.
#[test]
fn kept_ticks_of_the_same_due_and_priority_run_in_the_order_the_chunk_lists_them() {
    let (a, b) = (at(5, -55, 5), at(6, -55, 5));
    let (_, output) = ticking_with(
        &[(a, blocks::SAND), (b, blocks::SAND)],
        &[kept(b, blocks::SAND, 0, 0), kept(a, blocks::SAND, 0, 0)],
    );
    assert_eq!(emptied(&output), [b, a]);
}

/// A tick of a block at a position that has one pending already is dropped: two
/// changes beside a sand block, in two ticks, give it one tick, due when the first
/// said.
#[test]
fn a_second_schedule_of_the_same_block_at_the_same_place_is_dropped() {
    let origin = ChunkPos::new(0, 0);
    let sand = at(2, -60, 3);
    let (west, east) = (at(1, -60, 3), at(3, -60, 3));
    let mut region = ticking_floor_with(&[
        (sand, blocks::SAND),
        (west, blocks::STONE),
        (east, blocks::STONE),
    ]);
    let ticks_of_the_sand = |region: &Region| -> Vec<i32> {
        let saved = saved_ticks(region, origin);
        let of_sand = saved.into_iter().filter(|(at, _)| *at == in_chunk(sand));
        of_sand.map(|(_, delay)| delay).collect()
    };
    checked(&mut region, &moves(vec![dig(1, 1, -60, 3, 1)]));
    assert_eq!(ticks_of_the_sand(&region), [1]);
    checked(&mut region, &moves(vec![dig(1, 3, -60, 3, 2)]));
    assert_eq!(ticks_of_the_sand(&region), [0]);

    // Both in one tick.
    let mut region = ticking_floor_with(&[
        (sand, blocks::SAND),
        (west, blocks::STONE),
        (east, blocks::STONE),
    ]);
    checked(
        &mut region,
        &moves(vec![dig(1, 1, -60, 3, 1), dig(1, 3, -60, 3, 2)]),
    );
    assert_eq!(ticks_of_the_sand(&region), [1]);
}

/// A tick the chunk came with counts as pending: a schedule of the same block there
/// is dropped, and the kept due stands.
#[test]
fn a_schedule_where_a_kept_tick_of_the_same_block_waits_is_dropped() {
    let origin = ChunkPos::new(0, 0);
    let (sand, west) = (at(2, -60, 3), at(1, -60, 3));
    let (mut region, _) = ticking_with(
        &[(sand, blocks::SAND), (west, blocks::STONE)],
        &[kept(sand, blocks::SAND, 5, 0)],
    );
    checked(&mut region, &moves(vec![dig(1, 1, -60, 3, 1)]));
    // Due in L + 5, written at the end of L + 1.
    assert_eq!(saved_ticks(&region, origin), [(in_chunk(sand), 3)]);
}

/// At most 65,536 block ticks run in a tick; the rest stay due and run in the next.
#[test]
fn no_more_than_65536_block_ticks_run_in_one_tick() {
    for (others, falls_late) in [(65_535, false), (65_536, true)] {
        let sand = at(5, -55, 5);
        // Ticks of stone where there is air, which do nothing, ahead of the sand's.
        let mut ticks: Vec<ScheduledTick> = (0..others)
            .map(|index: i32| {
                let at = at(index % 16, -50 + index / 256, (index / 16) % 16);
                kept(at, blocks::STONE, 0, 0)
            })
            .collect();
        ticks.push(kept(sand, blocks::SAND, 0, 0));
        let (mut region, output) = ticking_with(&[(sand, blocks::SAND)], &ticks);
        if falls_late {
            assert_eq!(emptied(&output), [], "with {others} ahead of it");
            let output = idle_tick(&mut region);
            assert_eq!(emptied(&output), [sand], "with {others} ahead of it");
        } else {
            assert_eq!(emptied(&output), [sand], "with {others} ahead of it");
        }
    }
}

/// A tick scheduled while ticks run waits for a later tick: due ticks are collected
/// before any runs. Sand that falls onto sand whose support it took does not make the
/// lower one fall in the same tick.
#[test]
fn a_tick_of_delay_zero_kept_with_a_chunk_does_not_let_a_tick_scheduled_in_its_run_run_too() {
    // The lower sand rests on stone at -59 that has its own tick; the upper sand is
    // above the lower. When the lower falls in L, the upper is scheduled for L + 2 by
    // the change below it, and nothing of it runs in L.
    let (upper, lower) = (at(5, -54, 5), at(5, -55, 5));
    let (mut region, output) = ticking_with(
        &[(upper, blocks::SAND), (lower, blocks::SAND)],
        &[kept(lower, blocks::SAND, 0, 0)],
    );
    assert_eq!(emptied(&output), [lower]);
    assert_eq!(block_at(&region, upper), Some(blocks::SAND));
    idle_tick(&mut region);
    assert_eq!(block_at(&region, upper), Some(blocks::SAND));
    let output = idle_tick(&mut region);
    assert_eq!(emptied(&output), [upper]);
    assert_eq!(block_at(&region, at(5, -59, 5)), Some(blocks::SAND));
}

/// A fluid tick runs only where its block still holds exactly that fluid
/// (`ServerLevel.tickFluid`): one of flowing water at a still source does nothing, and
/// is gone once it has run.
#[test]
fn a_tick_of_flowing_water_at_a_source_runs_and_does_nothing() {
    let origin = ChunkPos::new(0, 0);
    let water = at(5, -60, 5);
    let fluid = ScheduledTick {
        at: in_chunk(water),
        of: Ticked::Fluid(1),
        delay: 0,
        priority: 0,
    };
    let (mut region, output) = ticking_with(&[(water, blocks::WATER)], &[fluid]);
    assert_eq!(block_changes(&output), []);
    for _ in 0..3 {
        let output = idle_tick(&mut region);
        assert_eq!(block_changes(&output), []);
    }
    let saved = region.saved_chunk(origin).unwrap();
    assert_eq!(saved.ticks(), []);
}

// ---------------------------------------------------------------------------------------
// D. The chunks whose ticks changed
// ---------------------------------------------------------------------------------------

/// The chunks where a tick was scheduled or ran are named, ascending, and a tick in
/// which nothing changed names none.
#[test]
fn the_chunks_whose_ticks_changed_are_named_in_ascending_order() {
    let (west, east) = (ChunkPos::new(0, 0), ChunkPos::new(1, 0));
    let (here, there) = (at(5, -55, 5), at(21, -55, 5));
    let given = vec![
        (
            east,
            floor_of(
                east,
                &[(there, blocks::SAND)],
                &[kept(there, blocks::SAND, 2, 0)],
            ),
        ),
        (
            west,
            floor_of(
                west,
                &[(here, blocks::SAND)],
                &[kept(here, blocks::SAND, 2, 0)],
            ),
        ),
    ];
    // Both tick: x from -1 to 2.
    let (mut region, _) = loaded_with(&chunks((-1, 2), (-1, 1)), given);
    assert_eq!(idle_tick(&mut region).ticks_changed, []);
    let output = idle_tick(&mut region);
    assert_eq!(emptied(&output), [here, there]);
    assert_eq!(output.ticks_changed, [west, east]);
    // What landed may have scheduled ticks of its own, due two ticks on.
    assert_eq!(idle_tick(&mut region).ticks_changed, []);
}

/// The chunks that tick with ticks pending are named in every tick, ascending, since
/// their saved forms age; a chunk that keeps ticks without ticking is not, nor is a
/// chunk that ticks with none.
#[test]
fn the_chunks_that_tick_with_ticks_pending_are_named_in_every_tick() {
    let (west, east, edge) = (
        ChunkPos::new(0, 0),
        ChunkPos::new(1, 0),
        ChunkPos::new(2, 0),
    );
    let (here, there, beyond) = (at(5, -55, 5), at(21, -55, 5), at(37, -55, 5));
    let sand = |chunk, position| {
        (
            chunk,
            floor_of(
                chunk,
                &[(position, blocks::SAND)],
                &[kept(position, blocks::SAND, 5, 0)],
            ),
        )
    };
    let given = vec![sand(east, there), sand(edge, beyond), sand(west, here)];
    // West and east tick; the edge, at x 2, has no neighbours to its east.
    let (mut region, _) = loaded_with(&chunks((-1, 2), (-1, 1)), given);
    for _ in 0..3 {
        let output = idle_tick(&mut region);
        assert_eq!(output.ticks_changed, []);
        assert_eq!(output.ticks_pending, [west, east]);
    }
    assert_eq!(idle_tick(&mut ticking_floor_with(&[])).ticks_pending, []);
}

/// The region `region` would be restored as from its state, its tickets and its
/// loaded chunks' saved forms as of its last tick.
fn restored(region: &Region) -> Region {
    let mut tickets = Vec::new();
    for (position, counts) in &region.tickets {
        let kinds = [
            (Ticket::Viewer, counts.viewers),
            (Ticket::Guest, counts.guests),
            (Ticket::ViewerRing, counts.viewer_rings),
            (Ticket::GuestRing, counts.guest_rings),
        ];
        for (kind, count) in kinds {
            tickets.extend((0..count).map(|_| (*position, kind)));
        }
    }
    let held = region
        .land
        .known
        .iter()
        .filter(|(_, known)| matches!(known, Known::Held { .. }))
        .map(|(position, _)| *position)
        .collect();
    let holdings = Holdings {
        held,
        pinned: region.land.pinned.clone(),
    };
    let chunks = region
        .chunks
        .keys()
        .map(|position| {
            let saved = region.saved_chunk(*position).expect("loaded");
            (*position, saved)
        })
        .collect();
    Region::restore_loaded(
        region.config.clone(),
        region.state(),
        holdings,
        &tickets,
        chunks,
    )
}

/// A region restored from its state and its chunks' saved forms goes on as the
/// region they were taken from: the ticks pending in its chunks, and those they
/// schedule as they run, run in the same ticks and change the same blocks, and the
/// chunks are saved alike after every tick (ADR-0024, sections 3 and 4).
#[test]
fn a_region_restored_from_its_saved_chunks_runs_their_ticks_as_the_original_does() {
    // Two sands over air, the lower due to fall in three ticks, and gravel due in
    // five: the upper sand falls only when the lower has gone from under it.
    let (lower, upper, gravel) = (at(5, -55, 5), at(5, -54, 5), at(9, -55, 9));
    let blocks = [
        (lower, blocks::SAND),
        (upper, blocks::SAND),
        (gravel, blocks::GRAVEL),
    ];
    let ticks = [
        kept(lower, blocks::SAND, 3, 0),
        kept(gravel, blocks::GRAVEL, 5, 0),
    ];
    let (mut original, _) = ticking_with(&blocks, &ticks);
    idle_tick(&mut original);
    let mut copy = restored(&original);
    let origin = ChunkPos::new(0, 0);
    let mut fell = Vec::new();
    for _ in 0..8 {
        let theirs = idle_tick(&mut original);
        let ours = idle_tick(&mut copy);
        assert_eq!(emptied(&ours), emptied(&theirs), "tick {}", theirs.tick);
        assert_eq!(copy.saved_chunk(origin), original.saved_chunk(origin));
        fell.extend(emptied(&theirs));
    }
    assert_eq!(fell, [lower, gravel, upper]);
}

/// What step B4c commits of a chunk's ticks is what its saved form keeps: the delays
/// left at the end of the tick, in queue order; and a chunk that is not loaded has
/// none. Once the chunk's last tick has run it is named no more among those that tick
/// with ticks pending, and its ticks are empty.
#[test]
fn the_scheduled_ticks_of_a_chunk_are_those_its_saved_form_keeps() {
    let origin = ChunkPos::new(0, 0);
    let (a, b) = (at(5, -60, 5), at(6, -60, 5));
    let (mut region, _) = ticking_with(
        &[(a, blocks::SAND), (b, blocks::SAND)],
        &[kept(b, blocks::SAND, 3, 0), kept(a, blocks::SAND, 1, 0)],
    );
    let saved = |region: &Region| region.saved_chunk(origin).unwrap().ticks().to_vec();
    // Written at the end of L, the tick the chunk started ticking in.
    assert_eq!(
        region.scheduled_ticks(origin),
        [kept(a, blocks::SAND, 0, 0), kept(b, blocks::SAND, 2, 0)]
    );
    assert_eq!(region.scheduled_ticks(origin), saved(&region));
    assert_eq!(region.scheduled_ticks(ChunkPos::new(9, 9)), []);

    // a runs in L + 1, b is left; b runs in L + 3, and the chunk has none.
    let output = idle_tick(&mut region);
    assert_eq!(output.ticks_changed, [origin]);
    assert_eq!(output.ticks_pending, [origin]);
    assert_eq!(
        region.scheduled_ticks(origin),
        [kept(b, blocks::SAND, 1, 0)]
    );
    assert_eq!(region.scheduled_ticks(origin), saved(&region));
    let output = idle_tick(&mut region);
    assert_eq!(output.ticks_changed, []);
    assert_eq!(output.ticks_pending, [origin]);
    assert_eq!(
        region.scheduled_ticks(origin),
        [kept(b, blocks::SAND, 0, 0)]
    );
    let output = idle_tick(&mut region);
    assert_eq!(output.ticks_changed, [origin]);
    assert_eq!(output.ticks_pending, []);
    assert_eq!(region.scheduled_ticks(origin), []);
    assert_eq!(saved(&region), []);
}

/// A tick scheduled by a player's dig names the chunk in that tick.
#[test]
fn a_scheduled_tick_names_its_chunk() {
    let (sand, stone) = (at(2, -58, 3), at(2, -59, 3));
    let mut region = ticking_floor_with(&[(stone, blocks::STONE), (sand, blocks::SAND)]);
    let output = checked(&mut region, &moves(vec![dig(1, 2, -59, 3, 1)]));
    assert_eq!(output.ticks_changed, [ChunkPos::new(0, 0)]);
    assert_eq!(idle_tick(&mut region).ticks_changed, []);
    assert_eq!(idle_tick(&mut region).ticks_changed, [ChunkPos::new(0, 0)]);
}

// ---------------------------------------------------------------------------------------
// E. What a block tick does
// ---------------------------------------------------------------------------------------

/// The region after player 1 has broken `dug` under a falling block, two ticks on,
/// when the block's tick has run.
fn after_a_fall(blocks: &[(BlockPos, BlockState)], dug: BlockPos) -> (Region, TickOutput) {
    let mut region = ticking_floor_with(blocks);
    checked(&mut region, &moves(vec![dig(1, dug.x, dug.y, dug.z, 1)]));
    idle_tick(&mut region);
    let output = idle_tick(&mut region);
    (region, output)
}

#[test]
fn sand_that_falls_onto_a_bottom_slab_is_lost() {
    let (sand, stone, slab) = (at(2, -58, 3), at(2, -59, 3), at(2, -60, 3));
    let (region, output) = after_a_fall(
        &[
            (slab, blocks::STONE_SLAB),
            (stone, blocks::STONE),
            (sand, blocks::SAND),
        ],
        stone,
    );
    assert_eq!(block_at(&region, sand), Some(blocks::AIR));
    assert_eq!(block_at(&region, stone), Some(blocks::AIR));
    assert_eq!(block_at(&region, slab), Some(blocks::STONE_SLAB));
    assert_eq!(block_changes(&output), [changed(2, -58, 3, blocks::AIR)]);
}

#[test]
fn sand_that_falls_onto_a_top_slab_rests_on_it() {
    let (sand, stone, slab) = (at(2, -58, 3), at(2, -59, 3), at(2, -60, 3));
    let top = state("minecraft:stone_slab[type=top]");
    let (region, _) = after_a_fall(
        &[(slab, top), (stone, blocks::STONE), (sand, blocks::SAND)],
        stone,
    );
    assert_eq!(block_at(&region, sand), Some(blocks::AIR));
    assert_eq!(block_at(&region, stone), Some(blocks::SAND));
    assert_eq!(block_at(&region, slab), Some(top));
}

#[test]
fn sand_that_falls_into_water_takes_the_waters_place() {
    let (sand, stone, water, under) = (at(2, -58, 3), at(2, -59, 3), at(2, -61, 3), at(2, -62, 3));
    let (region, _) = after_a_fall(
        &[
            (under, blocks::STONE),
            (water, blocks::WATER),
            (stone, blocks::STONE),
            (sand, blocks::SAND),
        ],
        stone,
    );
    assert_eq!(block_at(&region, sand), Some(blocks::AIR));
    assert_eq!(block_at(&region, at(2, -60, 3)), Some(blocks::AIR));
    assert_eq!(block_at(&region, water), Some(blocks::SAND));
}

#[test]
fn sand_that_falls_onto_short_grass_takes_its_place() {
    let (sand, stone, grass) = (at(2, -58, 3), at(2, -59, 3), at(2, -60, 3));
    let (region, _) = after_a_fall(
        &[
            (grass, blocks::SHORT_GRASS),
            (stone, blocks::STONE),
            (sand, blocks::SAND),
        ],
        stone,
    );
    assert_eq!(block_at(&region, sand), Some(blocks::AIR));
    assert_eq!(block_at(&region, stone), Some(blocks::AIR));
    assert_eq!(block_at(&region, grass), Some(blocks::SAND));
}

/// Gravel falls down a shaft through the floor in the one tick, thirteen blocks to the
/// bottom of the world.
#[test]
fn gravel_falls_down_a_tall_shaft_at_once() {
    let gravel = at(5, -50, 5);
    let mut blocks = vec![(gravel, blocks::GRAVEL)];
    for y in -63..=-61 {
        blocks.push((at(5, y, 5), blocks::AIR));
    }
    blocks.push((at(5, -64, 5), blocks::STONE));
    let (region, output) = ticking_with(&blocks, &[kept(gravel, blocks::GRAVEL, 0, 0)]);
    assert_eq!(block_at(&region, gravel), Some(blocks::AIR));
    assert_eq!(block_at(&region, at(5, -63, 5)), Some(blocks::GRAVEL));
    assert_eq!(
        block_changes(&output),
        [
            changed(5, -50, 5, blocks::AIR),
            changed(5, -63, 5, blocks::GRAVEL)
        ]
    );
}

#[test]
fn concrete_powder_and_an_anvil_fall_as_sand_does() {
    for block in [blocks::WHITE_CONCRETE_POWDER, blocks::ANVIL] {
        let (high, stone, low) = (at(2, -58, 3), at(2, -59, 3), at(2, -60, 3));
        let (region, _) = after_a_fall(&[(stone, blocks::STONE), (high, block)], stone);
        assert_eq!(block_at(&region, high), Some(blocks::AIR), "{block:?}");
        assert_eq!(block_at(&region, low), Some(block), "{block:?}");
    }
}

/// A falling block whose support stands does nothing at its tick.
#[test]
fn sand_that_is_supported_stays_at_its_tick() {
    let sand = at(5, -60, 5);
    let (mut region, output) =
        ticking_with(&[(sand, blocks::SAND)], &[kept(sand, blocks::SAND, 0, 0)]);
    assert_eq!(block_changes(&output), []);
    idle_tick(&mut region);
    assert_eq!(block_at(&region, sand), Some(blocks::SAND));
}

fn leaves(distance: u8) -> BlockState {
    blocks::OAK_LEAVES
        .with_property("distance", &distance.to_string())
        .expect("leaves have a distance")
}

/// Leaves at their tick take one more than the nearest log's or leaves' distance
/// beside them, and 7 with nothing near.
#[test]
fn leaves_at_their_tick_take_their_distance() {
    let (log, beside_log) = (at(5, -60, 5), at(6, -60, 5));
    let (near_leaves, beside_leaves) = (at(10, -59, 10), at(10, -60, 10));
    let alone = at(13, -60, 2);
    let (region, _) = ticking_with(
        &[
            (log, blocks::OAK_LOG),
            (beside_log, leaves(7)),
            (near_leaves, leaves(3)),
            (beside_leaves, leaves(7)),
            (alone, leaves(1)),
        ],
        &[
            kept(beside_log, blocks::OAK_LEAVES, 0, 0),
            kept(beside_leaves, blocks::OAK_LEAVES, 0, 0),
            kept(alone, blocks::OAK_LEAVES, 0, 0),
        ],
    );
    assert_eq!(block_at(&region, beside_log), Some(leaves(1)));
    assert_eq!(block_at(&region, beside_leaves), Some(leaves(4)));
    assert_eq!(block_at(&region, alone), Some(leaves(7)));
}

#[test]
fn a_dirt_path_at_its_tick_becomes_dirt() {
    let path = at(5, -61, 5);
    let (region, _) = ticking_with(
        &[(path, blocks::DIRT_PATH)],
        &[kept(path, blocks::DIRT_PATH, 0, 0)],
    );
    assert_eq!(block_at(&region, path), Some(blocks::DIRT));
}

#[test]
fn farmland_under_stone_becomes_dirt_at_its_tick() {
    let (farmland, stone) = (at(5, -61, 5), at(5, -60, 5));
    let (region, _) = ticking_with(
        &[(farmland, blocks::FARMLAND), (stone, blocks::STONE)],
        &[kept(farmland, blocks::FARMLAND, 0, 0)],
    );
    assert_eq!(block_at(&region, farmland), Some(blocks::DIRT));
}

/// Farmland that can stay is left as it is at its (scheduled) tick: drying out is the
/// work of random ticks, which are not this step's.
#[test]
fn farmland_under_air_stays_at_its_tick() {
    let farmland = at(8, -61, 8);
    let (region, output) = ticking_with(
        &[(farmland, blocks::FARMLAND)],
        &[kept(farmland, blocks::FARMLAND, 0, 0)],
    );
    assert_eq!(block_at(&region, farmland), Some(blocks::FARMLAND));
    assert_eq!(block_changes(&output), []);
}

fn destroyed_at(output: &TickOutput, position: BlockPos) -> Vec<BlockState> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockDestroyed {
                position: at,
                state,
                ..
            } if *at == position => Some(*state),
            _ => None,
        })
        .collect()
}

/// A cactus whose sand is broken does not break with it: at its tick, which the change
/// below it scheduled, it breaks and is shown breaking.
#[test]
fn a_cactus_whose_sand_is_broken_breaks_at_its_tick() {
    let (cactus, sand) = (at(2, -59, 3), at(2, -60, 3));
    let mut region = ticking_floor_with(&[(sand, blocks::SAND), (cactus, blocks::CACTUS)]);
    let output = checked(&mut region, &moves(vec![dig(1, 2, -60, 3, 1)]));
    assert_eq!(block_at(&region, cactus), Some(blocks::CACTUS));
    assert_eq!(destroyed_at(&output, cactus), []);
    let mut destroyed = Vec::new();
    for _ in 0..3 {
        let output = idle_tick(&mut region);
        destroyed.extend(destroyed_at(&output, cactus));
    }
    assert_eq!(block_at(&region, cactus), Some(blocks::AIR));
    assert_eq!(destroyed, [blocks::CACTUS]);
}

#[test]
fn a_cactus_or_sugar_cane_on_stone_breaks_at_its_tick() {
    for block in [blocks::CACTUS, blocks::SUGAR_CANE] {
        let at_floor = at(5, -60, 5);
        let (region, output) = ticking_with(&[(at_floor, block)], &[kept(at_floor, block, 0, 0)]);
        assert_eq!(block_at(&region, at_floor), Some(blocks::AIR), "{block:?}");
        assert_eq!(destroyed_at(&output, at_floor), [block]);
    }
}

#[test]
fn sugar_cane_on_sand_beside_water_stays_at_its_tick() {
    let (cane, sand, water) = (at(5, -60, 5), at(5, -61, 5), at(6, -61, 5));
    let (region, output) = ticking_with(
        &[
            (sand, blocks::SAND),
            (water, blocks::WATER),
            (cane, blocks::SUGAR_CANE),
        ],
        &[kept(cane, blocks::SUGAR_CANE, 0, 0)],
    );
    assert_eq!(block_at(&region, cane), Some(blocks::SUGAR_CANE));
    assert_eq!(block_changes(&output), []);
}

/// A block whose tick means nothing, as stone's, is left as it is.
#[test]
fn a_tick_of_any_other_block_does_nothing() {
    let stone = at(5, -60, 5);
    let (_, output) = ticking_with(
        &[(stone, blocks::STONE)],
        &[kept(stone, blocks::STONE, 0, 0)],
    );
    let of_blocks: Vec<_> = output
        .events
        .iter()
        .filter(|event| {
            matches!(
                event,
                RegionEvent::BlockChanged { .. } | RegionEvent::BlockDestroyed { .. }
            )
        })
        .collect();
    assert_eq!(of_blocks, Vec::<&RegionEvent>::new());
}

// ---------------------------------------------------------------------------------------
// F. Fluids (step B5)
// ---------------------------------------------------------------------------------------

/// A source's tick of still water, due at once.
fn source_tick(position: BlockPos, fluid: u8) -> ScheduledTick {
    ScheduledTick {
        at: in_chunk(position),
        of: Ticked::Fluid(fluid),
        delay: 0,
        priority: 0,
    }
}

/// The level of the liquid block at `position`, or `None` where there is none.
fn level_at(region: &Region, position: BlockPos) -> Option<u8> {
    let state = block_at(region, position)?;
    if !is_class(state, BlockClass::LiquidBlock) {
        return None;
    }
    state.property("level").and_then(|level| level.parse().ok())
}

/// Water from a source on flat ground spreads seven blocks, one level lower with each
/// block of the way (a diamond of `FlowingFluid.spreadToSides`), and no further.
#[test]
fn water_on_flat_ground_spreads_seven_blocks_one_level_at_a_time() {
    let source = at(8, -60, 8);
    let (mut region, _) = ticking_with(
        &[(source, blocks::WATER)],
        &[source_tick(source, clustine_blocks::fluid::WATER)],
    );
    for _ in 0..60 {
        idle_tick(&mut region);
    }
    for x in -9..=9_i32 {
        for z in -9..=9_i32 {
            let position = at(8 + x, -60, 8 + z);
            let distance = (x.abs() + z.abs()) as u8;
            let expected = (distance <= 7).then_some(distance);
            assert_eq!(level_at(&region, position), expected, "{x}, {z}");
        }
    }
}

/// Water looks up to four blocks for a way down and flows only towards the nearest.
#[test]
fn water_flows_only_towards_the_nearest_way_down() {
    let source = at(8, -60, 8);
    let hole = at(11, -61, 8);
    let (mut region, _) = ticking_with(
        &[(source, blocks::WATER), (hole, blocks::AIR)],
        &[source_tick(source, clustine_blocks::fluid::WATER)],
    );
    for _ in 0..40 {
        idle_tick(&mut region);
    }
    assert_eq!(level_at(&region, at(9, -60, 8)), Some(1));
    assert_eq!(level_at(&region, at(10, -60, 8)), Some(2));
    assert_eq!(level_at(&region, at(11, -60, 8)), Some(3));
    // It falls into the hole, which fills with falling water.
    assert_eq!(level_at(&region, hole), Some(8));
    for beside in [at(7, -60, 8), at(8, -60, 7), at(8, -60, 9)] {
        assert_eq!(level_at(&region, beside), None, "{beside:?}");
    }
}

/// Two sources with a block between them on something solid make a third.
#[test]
fn water_between_two_sources_on_solid_ground_becomes_a_source() {
    let (west, east) = (at(7, -60, 8), at(9, -60, 8));
    let (mut region, _) = ticking_with(
        &[(west, blocks::WATER), (east, blocks::WATER)],
        &[
            source_tick(west, clustine_blocks::fluid::WATER),
            source_tick(east, clustine_blocks::fluid::WATER),
        ],
    );
    for _ in 0..20 {
        idle_tick(&mut region);
    }
    assert_eq!(level_at(&region, at(8, -60, 8)), Some(0));
}

/// Lava that flows next to water becomes cobblestone, and the hiss is heard.
#[test]
fn lava_meeting_water_becomes_cobblestone_or_obsidian() {
    let lava = at(4, -60, 8);
    let water = at(7, -60, 8);
    let (mut region, _) = ticking_with(
        &[(lava, blocks::LAVA), (water, blocks::WATER)],
        &[source_tick(lava, clustine_blocks::fluid::LAVA)],
    );
    let mut fizzed = false;
    for _ in 0..200 {
        let output = idle_tick(&mut region);
        fizzed |= output
            .events
            .iter()
            .any(|event| matches!(event, RegionEvent::Fizzed { .. }));
    }
    assert!(fizzed);
    // The lava flows east at level 2 and then 4, which water beside it hardens. The
    // water had no tick of its own, and stays a source.
    assert_eq!(
        block_at(&region, at(5, -60, 8)),
        blocks::LAVA.with_property("level", "2")
    );
    assert_eq!(block_at(&region, at(6, -60, 8)), Some(blocks::COBBLESTONE));
    assert_eq!(block_at(&region, water), Some(blocks::WATER));
}

// ---------------------------------------------------------------------------------------
// G. Post-processing (step B6)
// ---------------------------------------------------------------------------------------

/// A marked fence beside stone takes its shape when its chunk starts ticking, and
/// marked water ticks: the chunk's marks are taken and the tick names it.
#[test]
fn a_chunk_with_marks_is_post_processed_as_it_starts_ticking() {
    let origin = ChunkPos::new(0, 0);
    let (fence, wall, water) = (at(4, -60, 4), at(5, -60, 4), at(10, -60, 10));
    let mut chunk = floor_of(
        origin,
        &[
            (fence, blocks::OAK_FENCE),
            (wall, blocks::STONE),
            (water, blocks::WATER),
        ],
        &[],
    );
    chunk.set_marks(vec![in_chunk(fence), in_chunk(water)]);
    let (mut region, output) = loaded_with(&around_the_origin(), vec![(origin, chunk)]);
    assert_eq!(output.marks_taken, [origin]);
    assert_eq!(
        block_at(&region, fence),
        blocks::OAK_FENCE.with_property("east", "true")
    );
    assert!(region.saved_chunk(origin).unwrap().marks().is_empty());
    // The water's tick ran there and then, and it has spread.
    assert_eq!(level_at(&region, at(11, -60, 10)), Some(1));
    // Once taken, the marks are not taken again.
    assert_eq!(idle_tick(&mut region).marks_taken, []);
}

/// A chunk whose neighbours are not all loaded keeps its marks until they are.
#[test]
fn marks_wait_until_the_chunk_ticks() {
    let edge = ChunkPos::new(1, 0);
    let fence = at(20, -60, 4);
    let mut chunk = floor_of(
        edge,
        &[(fence, blocks::OAK_FENCE), (at(21, -60, 4), blocks::STONE)],
        &[],
    );
    chunk.set_marks(vec![in_chunk(fence)]);
    let (mut region, output) = loaded_with(&around_the_origin(), vec![(edge, chunk)]);
    assert_eq!(output.marks_taken, []);
    assert_eq!(block_at(&region, fence), Some(blocks::OAK_FENCE));
    assert_eq!(region.saved_chunk(edge).unwrap().marks().len(), 1);
    let more: Vec<_> = chunks((2, 2), (-1, 1))
        .into_iter()
        .map(|p| (p, floor()))
        .collect();
    let output = load(&mut region, more);
    assert_eq!(output.marks_taken, [edge]);
    assert_eq!(
        block_at(&region, fence),
        blocks::OAK_FENCE.with_property("east", "true")
    );
}

// ---------------------------------------------------------------------------------------
// H. Seams (step B6)
// ---------------------------------------------------------------------------------------

/// An update that would reach a chunk the region has not loaded waits as a seam entry
/// in the chunk of the block that changed, and is made once that chunk is loaded: a
/// wall torch whose wall went while its chunk was away goes when it comes.
#[test]
fn an_update_into_a_chunk_not_loaded_is_made_when_it_comes() {
    let (wall, torch) = (at(31, -60, 8), at(32, -60, 8));
    let near = ChunkPos::new(1, 0);
    let far = ChunkPos::new(2, 0);
    let (mut region, _) = loaded_with(
        &around_the_origin(),
        vec![(near, floor_of(near, &[(wall, blocks::STONE)], &[]))],
    );
    let mut output = TickOutput::default();
    region.set_block(wall, blocks::AIR, &mut output);
    let seams = region.seams(near);
    assert!(
        seams
            .iter()
            .any(|seam| seam.at == torch && seam.kind == clustine_world::SeamKind::Shape),
        "{seams:?}"
    );
    assert_eq!(idle_tick(&mut region).seams_changed, []);
    let standing = blocks::WALL_TORCH.with_property("facing", "east").unwrap();
    let output = load(
        &mut region,
        vec![(far, floor_of(far, &[(torch, standing)], &[]))],
    );
    assert_eq!(block_at(&region, torch), Some(blocks::AIR));
    assert!(output.seams_changed.contains(&near));
    assert!(region.seams(near).iter().all(|seam| seam.at != torch));
}

// ---------------------------------------------------------------------------------------
// I. The shapes of what grows (step B7)
// ---------------------------------------------------------------------------------------

/// The state of the block called `name` with `properties`.
fn named(name: &str, properties: &[(&str, &str)]) -> BlockState {
    let id = clustine_data::block_id_by_name(name).unwrap();
    let mut state = clustine_data::BLOCKS[id].default_state;
    for (property, value) in properties {
        state = state.with_property(property, value).unwrap();
    }
    state
}

/// Kelp that grows becomes body under its new head, and the body at the top becomes a
/// head again when the head above it goes (`GrowingPlant*Block.updateShape`).
#[test]
fn kelp_under_a_new_head_becomes_body_and_body_left_on_top_a_head() {
    let (low, high, top) = (at(5, -60, 5), at(5, -59, 5), at(5, -58, 5));
    let water = [(top, blocks::WATER), (at(5, -57, 5), blocks::WATER)];
    let mut placed = vec![(low, named("minecraft:kelp", &[]))];
    placed.push((high, blocks::WATER));
    placed.extend(water);
    let (mut region, _) = ticking_with(&placed, &[]);
    let mut output = TickOutput::default();
    region.set_block(high, named("minecraft:kelp", &[]), &mut output);
    assert_eq!(
        block_at(&region, low).unwrap().block().unwrap().name,
        "minecraft:kelp_plant"
    );
    region.set_block(top, named("minecraft:kelp", &[]), &mut output);
    assert_eq!(
        block_at(&region, high).unwrap().block().unwrap().name,
        "minecraft:kelp_plant"
    );
    region.set_block(top, blocks::WATER, &mut output);
    let head = block_at(&region, high).unwrap();
    assert_eq!(head.block().unwrap().name, "minecraft:kelp");
    let age: u32 = head.property("age").unwrap().parse().unwrap();
    assert!(age < 25);
}

/// Kelp whose ground goes is gone a tick later, and its water stays.
#[test]
fn kelp_whose_ground_goes_is_destroyed_at_its_tick() {
    let (ground, kelp) = (at(5, -61, 5), at(5, -60, 5));
    let (mut region, _) = ticking_with(
        &[
            (kelp, named("minecraft:kelp", &[])),
            (at(5, -59, 5), blocks::WATER),
        ],
        &[],
    );
    let mut output = TickOutput::default();
    region.set_block(ground, blocks::WATER, &mut output);
    assert_eq!(
        block_at(&region, kelp).unwrap().block().unwrap().name,
        "minecraft:kelp"
    );
    idle_tick(&mut region);
    idle_tick(&mut region);
    // Kelp holds water, which it leaves.
    assert_eq!(block_at(&region, kelp), Some(blocks::WATER));
}

/// A stalactite's tip becomes frustum when a tip grows below it, and base where it
/// hangs from stone.
#[test]
fn a_stalactite_s_tip_becomes_frustum_when_a_tip_grows_below_it() {
    let tip = |thickness| {
        named(
            "minecraft:pointed_dripstone",
            &[("vertical_direction", "down"), ("thickness", thickness)],
        )
    };
    let (ceiling, upper, lower, lowest) =
        (at(5, -56, 5), at(5, -57, 5), at(5, -58, 5), at(5, -59, 5));
    let (mut region, _) = ticking_with(&[(ceiling, blocks::STONE), (upper, tip("tip"))], &[]);
    let mut output = TickOutput::default();
    region.set_block(lower, tip("tip"), &mut output);
    assert_eq!(block_at(&region, upper), Some(tip("frustum")));
    region.set_block(lowest, tip("tip"), &mut output);
    assert_eq!(block_at(&region, upper), Some(tip("base")));
    assert_eq!(block_at(&region, lower), Some(tip("frustum")));
}

/// A chorus plant joins a chorus flower set beside it.
#[test]
fn a_chorus_plant_joins_a_flower_beside_it() {
    let (plant, above) = (at(5, -60, 5), at(5, -59, 5));
    let (mut region, _) = ticking_with(&[(plant, named("minecraft:chorus_plant", &[]))], &[]);
    let mut output = TickOutput::default();
    region.set_block(above, named("minecraft:chorus_flower", &[]), &mut output);
    assert_eq!(
        block_at(&region, plant).unwrap().property("up"),
        Some("true")
    );
}

/// A coral block a player places out of water dies within 60 to 99 ticks
/// (`CoralBlock.getStateForPlacement`), without a neighbour changing.
#[test]
fn a_coral_block_placed_on_land_dies_within_a_hundred_ticks() {
    let pos = at(5, -60, 5);
    let (mut region, _) = ticking_with(&[], &[]);
    let mut output = TickOutput::default();
    let coral = named("minecraft:tube_coral_block", &[]);
    assert!(region.change_all(&[(pos, coral)], &mut output));
    let mut died = None;
    for tick in 1..=100 {
        idle_tick(&mut region);
        if block_at(&region, pos) != Some(coral) {
            died = Some(tick);
            break;
        }
    }
    let died = died.expect("it died");
    assert!((60..=100).contains(&died), "{died}");
    assert_eq!(
        block_at(&region, pos),
        Some(named("minecraft:dead_tube_coral_block", &[]))
    );
}

/// The leaves of a felled tree are gone within the game's time: once the log goes,
/// each leaf's tick sets its distance on to 7, and then a random tick removes it. A
/// section has three positions drawn each tick, so each leaf is drawn about once in
/// 4096 / 3 ticks; that all eight of these go within 15,000 ticks fails about once
/// in a thousand runs of a stream that is fair, and that none goes within the first
/// ten is near certain.
#[test]
fn the_leaves_of_a_felled_tree_are_gone_within_the_games_time() {
    let log = at(8, -60, 8);
    let oak_leaves = named("minecraft:oak_leaves", &[("distance", "1")]);
    let around = [
        at(7, -60, 8),
        at(9, -60, 8),
        at(8, -60, 7),
        at(8, -60, 9),
        at(7, -60, 7),
        at(9, -60, 9),
        at(7, -60, 9),
        at(9, -60, 7),
    ];
    let mut placed: Vec<_> = around
        .iter()
        .map(|leaf| {
            let distance = if leaf.x == 8 || leaf.z == 8 { "1" } else { "2" };
            (
                *leaf,
                oak_leaves.with_property("distance", distance).unwrap(),
            )
        })
        .collect();
    placed.push((log, named("minecraft:oak_log", &[])));
    let (mut region, _) = ticking_with(&placed, &[]);
    let mut output = TickOutput::default();
    region.set_block(log, blocks::AIR, &mut output);
    let left = |region: &Region| {
        around
            .iter()
            .filter(|leaf| block_at(region, **leaf) != Some(blocks::AIR))
            .count()
    };
    for _ in 0..10 {
        idle_tick(&mut region);
    }
    assert_eq!(left(&region), 8);
    let mut ticks = 10;
    while left(&region) > 0 && ticks < 15_000 {
        idle_tick(&mut region);
        ticks += 1;
    }
    assert_eq!(left(&region), 0, "after {ticks} ticks");
}
