//! Tests of the scheduled ticks a commit carries, as the store replays them: written
//! from `docs/adr/0024-block-behaviour-in-a-region.md`, section 4 ("The store") and
//! "Found while building", by someone who did not write the code. A commit's ticks of
//! a chunk are that chunk's ticks, whole, as the tick left them; a restore puts them
//! into the stored chunk after the commit's blocks, the last list of a chunk winning,
//! under the same rules of who held the chunk as block changes; a chunk only ticks
//! touched is touched. The tests of a chunk held only as generated are with the other
//! tests of generation, whose generator says what it made (`generation_tests.rs`).

use std::path::Path;

use clustine_data::{BlockState, blocks};
use clustine_world::{BlockPos, Chunk, ChunkPos, InChunk, ScheduledTick, Ticked};

use super::*;
use crate::disk::{MemoryDisk, Survival};
use crate::regions::{SURVIVALS, store_on};
use crate::tests::{committed, division, generator, hello, load, open};

// The world of these tests is that of [`division`]: region 0 west of x = 0, region 1
// from x = 0 on, with the home chunk at the origin.

const WEST: u32 = 0;
const EAST: u32 = 1;

/// Chunks of the eastern region.
const A: ChunkPos = ChunkPos::new(0, 0);
const B: ChunkPos = ChunkPos::new(1, 0);
const C: ChunkPos = ChunkPos::new(2, 0);
const D: ChunkPos = ChunkPos::new(3, 0);

/// A chunk of the western region.
const W: ChunkPos = ChunkPos::new(-1, 0);

/// A tick of a block with `delay` left, at a place of its own for each `n`.
fn tick(n: u8, delay: i32) -> ScheduledTick {
    ScheduledTick {
        at: InChunk::new(usize::from(n), -60, 4),
        of: Ticked::Block(u16::from(n) + 100),
        delay,
        priority: 0,
    }
}

/// A tick of a fluid with `delay` left.
fn fluid(n: u8, delay: i32) -> ScheduledTick {
    ScheduledTick {
        at: InChunk::new(usize::from(n), -61, 9),
        of: Ticked::Fluid(1),
        delay,
        priority: -1,
    }
}

/// Commits `tick` with the block changes and the ticks of chunks given.
fn commit(
    handle: &StoreHandle,
    tick: u64,
    changes: &[(BlockPos, BlockState)],
    ticks: Vec<(ChunkPos, Vec<ScheduledTick>)>,
) {
    handle.request(StoreRequest::Commit {
        travellers: Vec::new(),
        tick,
        changes: changes
            .iter()
            .copied()
            .map(clustine_world::Change::from)
            .collect(),
        state: format!("delta {tick}").into_bytes(),
        stays: Vec::new(),
        ticks,
        marks_taken: Vec::new(),
        seams: Vec::new(),
        effects: Vec::new(),
        taken: Vec::new(),
        events: Vec::new(),
        entity_changes: Vec::new(),
        arrivals: Vec::new(),
        arrivals_taken: Vec::new(),
        clock: Default::default(),
        claims: Vec::new(),
        raids: Vec::new(),
    });
}

/// The chunk the flat world has at `position`, with `blocks` set (by their place in
/// the world) and `ticks` kept, and no longer as generated: a commit of ticks clears
/// that as a commit of blocks does.
fn expected(
    position: ChunkPos,
    blocks: &[(BlockPos, BlockState)],
    ticks: Vec<ScheduledTick>,
) -> Chunk {
    let mut chunk = generator().generate(position);
    for (at, state) in blocks {
        let (x, z) = at.in_chunk();
        chunk.set(x, at.y, z, *state);
    }
    chunk.set_ticks(ticks);
    chunk.clear_generated();
    chunk
}

/// Sand in the chunk at the origin, which the first commit of
/// [`a_restore_gives_each_chunk_the_ticks_of_the_last_commit_that_listed_it`] places.
fn sand_in_a() -> (BlockPos, BlockState) {
    (BlockPos::new(3, -60, 4), blocks::SAND)
}

/// Air where the grass of `D` was, which the first commit makes too.
fn dug_in_d() -> (BlockPos, BlockState) {
    (BlockPos::new(D.x * 16 + 5, -61, 5), blocks::AIR)
}

/// Three commits of the eastern region, none of them saved into a chunk:
///
/// 1. blocks in `A` and `D`; ticks of `A`, `B` and `D`;
/// 2. no blocks; other ticks of `A` (with a tick of delay 0 scheduled in the tick,
///    kept as −1), and nothing of `B`;
/// 3. no blocks; `B`'s list, which became empty, and `C`'s, which nothing else
///    touched.
fn three_commits(east: &StoreHandle) {
    commit(
        east,
        1,
        &[sand_in_a(), dug_in_d()],
        vec![
            (A, vec![tick(1, 5)]),
            (B, vec![tick(2, 3), fluid(3, 0)]),
            (D, vec![tick(4, 9)]),
        ],
    );
    commit(east, 2, &[], vec![(A, vec![tick(5, 1), tick(6, -1)])]);
    commit(
        east,
        3,
        &[],
        vec![(B, Vec::new()), (C, vec![fluid(7, 7), tick(8, 2)])],
    );
    committed(east, 3);
}

/// What each chunk is restored with after [`three_commits`].
fn as_committed(handle: &StoreHandle, case: &str) {
    assert_eq!(
        load(handle, A),
        expected(A, &[sand_in_a()], vec![tick(5, 1), tick(6, -1)]),
        "{case}"
    );
    assert_eq!(load(handle, B), expected(B, &[], Vec::new()), "{case}");
    assert_eq!(
        load(handle, C),
        expected(C, &[], vec![fluid(7, 7), tick(8, 2)]),
        "{case}"
    );
    assert_eq!(
        load(handle, D),
        expected(D, &[dug_in_d()], vec![tick(4, 9)]),
        "{case}"
    );
}

/// A restore gives each chunk the ticks of the last commit that listed it, after the
/// commit's blocks: a later list replaces an earlier one whole, an empty list empties
/// the chunk's ticks, a chunk that a commit's ticks alone touched has them, and a
/// chunk that a later commit did not list keeps those of the one before. So it is on
/// the same store, and on a store started on what any crash leaves of it.
#[test]
fn a_restore_gives_each_chunk_the_ticks_of_the_last_commit_that_listed_it() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let east = open(&store, hello(EAST, 1));
    three_commits(&east);
    east.flush();

    let (again, restored) = store.open_region(hello(EAST, 2)).unwrap();
    assert_eq!(restored.deltas.len(), 3);
    as_committed(&again, "the same store");
    again.flush();
    drop((again, east, store));

    for (epoch, survival) in (3..).zip(SURVIVALS) {
        let case = format!("{survival:?}");
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &division()).unwrap();
        let (handle, _) = store.open_region(hello(EAST, epoch)).unwrap();
        as_committed(&handle, &case);
        handle.flush();
    }
}

/// The restored chunks are stored, as chunks a restore changes with blocks are: a
/// store started on what a crash leaves after the restore has them with their ticks
/// as well, though the commits are replayed once more.
#[test]
fn chunks_restored_with_ticks_are_stored_with_them() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let east = open(&store, hello(EAST, 1));
    three_commits(&east);
    east.flush();
    let (again, _) = store.open_region(hello(EAST, 2)).unwrap();
    again.flush();
    drop((again, east, store));

    let left = Arc::new(disk.crashed(Survival::Nothing));
    let chunks = FileChunks::new(left, Path::new("/world"));
    let mut chunks: Box<dyn Chunks> = Box::new(chunks);
    let stored = chunks.load(C).unwrap().expect("the restore stored C");
    assert_eq!(stored, expected(C, &[], vec![fluid(7, 7), tick(8, 2)]));
    assert_eq!(stored.generated(), 0);
}

/// The ticks of a chunk saved after a commit and before the next are those the save
/// had, and those of a later commit replace them: a restore applies a commit's ticks
/// over the saved chunk as it applies its blocks.
#[test]
fn ticks_committed_after_a_save_replace_those_the_chunk_was_saved_with() {
    let store = store_on(&Arc::new(MemoryDisk::default()), &division()).unwrap();
    let east = open(&store, hello(EAST, 1));
    commit(&east, 1, &[], vec![(B, vec![tick(1, 40)])]);
    let mut saved = generator().generate(B);
    saved.set_ticks(vec![tick(1, 39), tick(2, 0)]);
    saved.clear_generated();
    east.request(StoreRequest::Save {
        position: B,
        tick: 1,
        chunk: saved.clone(),
        parts: clustine_rpc::Parts::ALL,
    });
    commit(&east, 2, &[], vec![(B, vec![tick(2, -1)])]);
    committed(&east, 2);
    east.flush();
    let (again, _) = store.open_region(hello(EAST, 2)).unwrap();
    assert_eq!(load(&again, B), expected(B, &[], vec![tick(2, -1)]));
}

/// What a region committed of the ticks of a chunk it does not hold is not put into
/// the chunk, as its block changes are not.
#[test]
fn ticks_committed_for_a_chunk_the_region_does_not_hold_are_not_replayed() {
    let store = store_on(&Arc::new(MemoryDisk::default()), &division()).unwrap();
    let west = open(&store, hello(WEST, 1));
    let east = open(&store, hello(EAST, 1));
    commit(
        &west,
        1,
        &[],
        vec![(W, vec![tick(1, 4)]), (B, vec![tick(2, 4)])],
    );
    committed(&west, 1);
    west.flush();
    let (again, _) = store.open_region(hello(WEST, 2)).unwrap();
    assert_eq!(load(&again, W), expected(W, &[], vec![tick(1, 4)]));
    assert_eq!(load(&east, B), generator().generate(B));
}

/// When the world is made over for another division at start-up, the ticks the
/// regions of the old one committed go into the chunks, the last list of a chunk
/// winning, as their blocks do; the regions of the new one are restored with nothing
/// and find them in the chunks.
#[test]
fn a_world_made_over_for_another_division_has_the_ticks_its_regions_committed() {
    let disk = Arc::new(MemoryDisk::default());
    {
        let store = store_on(&disk, &division()).unwrap();
        let west = open(&store, hello(WEST, 1));
        let east = open(&store, hello(EAST, 1));
        commit(&west, 1, &[], vec![(W, vec![tick(1, 6)])]);
        three_commits(&east);
        committed(&west, 1);
        west.flush();
        east.flush();
    }
    let single = Division::side_by_side(A, &[]).unwrap();
    for survival in SURVIVALS {
        let case = format!("{survival:?}");
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &single).unwrap();
        let (whole, restored) = store.open_region(hello(0, 5)).unwrap();
        assert_eq!(
            (restored.state, restored.deltas),
            (None, Vec::new()),
            "{case}"
        );
        assert_eq!(
            load(&whole, W),
            expected(W, &[], vec![tick(1, 6)]),
            "{case}"
        );
        as_committed(&whole, &case);
        whole.flush();
        // And a store started once more, after the fold, finds the same.
        drop((whole, store));
        let again = Arc::new(left.crashed(Survival::Nothing));
        let store = store_on(&again, &single).unwrap();
        let (whole, _) = store.open_region(hello(0, 6)).unwrap();
        as_committed(&whole, &format!("{case}, again"));
        whole.flush();
    }
}
