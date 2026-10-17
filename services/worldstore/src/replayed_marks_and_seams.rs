//! Tests of what a commit carries of post-processing and of seams, as the store replays
//! it: written from sections 4 to 6 of `docs/adr/0024-block-behaviour-in-a-region.md`
//! by someone who did not write the code. A commit names the chunks whose marks were
//! taken in its tick, and carries the seam entries of each chunk whose entries changed,
//! whole. A restore puts them into the stored chunks after the commit's blocks: a chunk
//! whose marks were taken has none, so that it is not post-processed again, and a
//! chunk's seams are those of the last commit that listed it. A crash between the
//! post-processing and a save of the chunk loses neither.

use std::path::Path;

use clustine_data::{BlockState, blocks};
use clustine_world::{BlockPos, Chunk, ChunkPos, InChunk, Seam, SeamKind};

use super::*;
use crate::disk::{MemoryDisk, Survival};
use crate::regions::{SURVIVALS, store_on};
use crate::tests::{committed, division, generator, hello, load, open};

// The world of these tests is that of [`division`]: region 0 west of x = 0, region 1
// from x = 0 on, with the home chunk at the origin.

const WEST: u32 = 0;
const EAST: u32 = 1;

/// Chunks of the eastern region.
const A: ChunkPos = ChunkPos::new(1, 0);
const B: ChunkPos = ChunkPos::new(2, 0);
const C: ChunkPos = ChunkPos::new(3, 0);

/// A chunk of the western region.
const W: ChunkPos = ChunkPos::new(-1, 0);

/// Where generation left a fence in `chunk`, which post-processing connects.
fn fence_in(chunk: ChunkPos) -> BlockPos {
    BlockPos::new(chunk.x * 16 + 4, -60, 4)
}

fn in_chunk(position: BlockPos) -> InChunk {
    let (x, z) = position.in_chunk();
    InChunk::new(x, position.y, z)
}

/// The flat world's chunk at `position` with a fence marked for post-processing, as
/// generation would leave it.
fn marked(position: ChunkPos) -> Chunk {
    let mut chunk = generator().generate(position);
    let fence = fence_in(position);
    let (x, z) = fence.in_chunk();
    chunk.set(x, fence.y, z, blocks::OAK_FENCE);
    chunk.set_marks(vec![in_chunk(fence), InChunk::new(9, -61, 9)]);
    chunk
}

/// The fence of `position` connected, as post-processing set it.
fn connected(position: ChunkPos) -> (BlockPos, BlockState) {
    let fence = blocks::OAK_FENCE
        .with_property("east", "true")
        .expect("a fence connects east");
    (fence_in(position), fence)
}

/// An entry of a seam reaching `at` from the west, from a block that became `source`.
fn seam(at: BlockPos, source: BlockState) -> Seam {
    Seam {
        at,
        direction: 4,
        kind: SeamKind::Shape,
        source,
        flags: 2,
        depth: 511,
    }
}

/// Saves `chunk` at `position` as of tick 0: before any of the tests' commits.
fn stored(handle: &StoreHandle, position: ChunkPos, chunk: Chunk) {
    handle.request(StoreRequest::Save {
        position,
        tick: 0,
        chunk,
    });
}

/// Commits `tick` with the block changes, the marks taken and the seams given.
fn commit(
    handle: &StoreHandle,
    tick: u64,
    changes: &[(BlockPos, BlockState)],
    marks_taken: Vec<ChunkPos>,
    seams: Vec<(ChunkPos, Vec<Seam>)>,
) {
    handle.request(StoreRequest::Commit {
        tick,
        changes: changes
            .iter()
            .copied()
            .map(clustine_world::Change::from)
            .collect(),
        state: format!("delta {tick}").into_bytes(),
        stays: Vec::new(),
        ticks: Vec::new(),
        marks_taken,
        seams,
        effects: Vec::new(),
        taken: Vec::new(),
        events: Vec::new(),
    });
}

/// `chunk` with `blocks` set, its marks taken, `seams` as its entries, and no longer as
/// generated.
fn expected(mut chunk: Chunk, blocks: &[(BlockPos, BlockState)], seams: Vec<Seam>) -> Chunk {
    for (at, state) in blocks {
        let (x, z) = at.in_chunk();
        chunk.set(x, at.y, z, *state);
    }
    chunk.set_marks(Vec::new());
    chunk.set_seams(seams);
    chunk.clear_generated();
    chunk
}

fn s1() -> Seam {
    seam(BlockPos::new(48, -60, 8), blocks::AIR)
}
fn s2() -> Seam {
    seam(BlockPos::new(48, -60, 9), blocks::AIR)
}
fn s3() -> Seam {
    seam(BlockPos::new(64, -61, 2), blocks::DIRT)
}

/// The eastern region post-processes `A` and `B` and keeps seams in them, over three
/// commits, none of them saved into a chunk:
///
/// 1. `A` post-processed (its fence connected, its marks taken); seams `s1` and `s2`
///    kept in `A`, `s3` in `B`;
/// 2. `B` post-processed; `A`'s seams are now `s2` alone;
/// 3. `B`'s seams were all made: its list is empty.
fn three_commits(east: &StoreHandle) {
    stored(east, A, marked(A));
    stored(east, B, marked(B));
    commit(
        east,
        1,
        &[connected(A)],
        vec![A],
        vec![(A, vec![s1(), s2()]), (B, vec![s3()])],
    );
    commit(east, 2, &[connected(B)], vec![B], vec![(A, vec![s2()])]);
    commit(east, 3, &[], Vec::new(), vec![(B, Vec::new())]);
    committed(east, 3);
}

/// What each chunk is restored with after [`three_commits`].
fn as_committed(handle: &StoreHandle, case: &str) {
    let a = load(handle, A);
    assert_eq!(a.marks(), [], "{case}");
    assert_eq!(
        a,
        expected(marked(A), &[connected(A)], vec![s2()]),
        "{case}"
    );
    let b = load(handle, B);
    assert_eq!(b.marks(), [], "{case}");
    assert_eq!(
        b,
        expected(marked(B), &[connected(B)], Vec::new()),
        "{case}"
    );
    assert_eq!(load(handle, C), generator().generate(C), "{case}");
}

/// A restore gives a chunk whose marks a commit took no marks, and each chunk the seams
/// of the last commit that listed it: so on the same store, and on a store started on
/// what any crash leaves of it.
#[test]
fn a_restore_gives_chunks_without_the_marks_taken_and_with_the_last_seams() {
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

/// A crash between a chunk's post-processing and its save: the store has the chunk as
/// generation left it, marked, and the commit that took its marks. A restore replays
/// the commit, so the chunk comes back post-processed and without marks, and is not
/// post-processed again; and a store started on what a crash leaves after that restore
/// has it so too.
#[test]
fn a_crash_between_post_processing_and_a_save_does_not_post_process_the_chunk_again() {
    let disk = Arc::new(MemoryDisk::default());
    {
        let store = store_on(&disk, &division()).unwrap();
        let east = open(&store, hello(EAST, 1));
        stored(&east, A, marked(A));
        east.flush();
        // The tick in which `A` started ticking: its fence connected, its marks taken,
        // a seam kept. The region's process ends before `A` is saved again.
        commit(&east, 7, &[connected(A)], vec![A], vec![(A, vec![s1()])]);
        committed(&east, 7);
        east.flush();
    }
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let case = format!("{survival:?}");
        let left = Arc::new(disk.crashed(survival));
        {
            let store = store_on(&left, &division()).unwrap();
            let (handle, restored) = store.open_region(hello(EAST, epoch)).unwrap();
            assert!(
                restored.deltas.iter().any(|delta| delta.tick == 7),
                "{case}: the commit is replayed"
            );
            let a = load(&handle, A);
            assert_eq!(a.marks(), [], "{case}");
            assert_eq!(
                a,
                expected(marked(A), &[connected(A)], vec![s1()]),
                "{case}"
            );
            assert_eq!(a.generated(), 0, "{case}");
            handle.flush();
        }
        // And again after a crash that follows the restore.
        let again = Arc::new(left.crashed(Survival::Nothing));
        let store = store_on(&again, &division()).unwrap();
        let (handle, _) = store.open_region(hello(EAST, epoch + 10)).unwrap();
        let a = load(&handle, A);
        assert_eq!(a.marks(), [], "{case}, again");
        assert_eq!(a.seams(), [s1()], "{case}, again");
        handle.flush();
    }
}

/// The restored chunks are stored as the restore made them: the chunk files a crash
/// leaves after the restore have the marks taken and the seams.
#[test]
fn chunks_restored_with_marks_taken_and_seams_are_stored_so() {
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
    let a = chunks.load(A).unwrap().expect("the restore stored A");
    assert_eq!(a, expected(marked(A), &[connected(A)], vec![s2()]));
    let b = chunks.load(B).unwrap().expect("the restore stored B");
    assert_eq!(b, expected(marked(B), &[connected(B)], Vec::new()));
}

/// Seams committed after a save replace those the chunk was saved with, and a commit
/// that names a chunk's marks taken after a save of it still marked takes them.
#[test]
fn what_is_committed_after_a_save_replaces_what_the_chunk_was_saved_with() {
    let store = store_on(&Arc::new(MemoryDisk::default()), &division()).unwrap();
    let east = open(&store, hello(EAST, 1));
    let mut saved = marked(A);
    saved.set_seams(vec![s1(), s2()]);
    saved.clear_generated();
    east.request(StoreRequest::Save {
        position: A,
        tick: 1,
        chunk: saved.clone(),
    });
    commit(&east, 2, &[connected(A)], vec![A], vec![(A, vec![s3()])]);
    committed(&east, 2);
    east.flush();
    let (again, _) = store.open_region(hello(EAST, 2)).unwrap();
    assert_eq!(
        load(&again, A),
        expected(marked(A), &[connected(A)], vec![s3()])
    );
}

/// What a region committed of a chunk it does not hold, marks taken or seams, is not
/// put into the chunk, as its block changes are not.
#[test]
fn marks_taken_and_seams_of_a_chunk_the_region_does_not_hold_are_not_replayed() {
    let store = store_on(&Arc::new(MemoryDisk::default()), &division()).unwrap();
    let west = open(&store, hello(WEST, 1));
    let east = open(&store, hello(EAST, 1));
    stored(&east, B, marked(B));
    east.flush();
    stored(&west, W, marked(W));
    commit(
        &west,
        1,
        &[],
        vec![W, B],
        vec![(W, vec![s1()]), (B, vec![s3()])],
    );
    committed(&west, 1);
    west.flush();
    let (again, _) = store.open_region(hello(WEST, 2)).unwrap();
    assert_eq!(load(&again, W), expected(marked(W), &[], vec![s1()]));
    assert_eq!(load(&east, B), marked(B));
}

/// When the world is made over for another division at start-up, the marks taken and
/// the seams the regions of the old one committed go into the chunks, as their blocks
/// do; the region of the new one finds them there.
#[test]
fn a_world_made_over_for_another_division_has_the_marks_taken_and_seams_committed() {
    let disk = Arc::new(MemoryDisk::default());
    {
        let store = store_on(&disk, &division()).unwrap();
        let west = open(&store, hello(WEST, 1));
        let east = open(&store, hello(EAST, 1));
        stored(&west, W, marked(W));
        commit(&west, 1, &[connected(W)], vec![W], vec![(W, vec![s1()])]);
        three_commits(&east);
        committed(&west, 1);
        west.flush();
        east.flush();
    }
    let single = Division::side_by_side(ChunkPos::new(0, 0), &[]).unwrap();
    for survival in SURVIVALS {
        let case = format!("{survival:?}");
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &single).unwrap();
        let (whole, _) = store.open_region(hello(0, 5)).unwrap();
        assert_eq!(
            load(&whole, W),
            expected(marked(W), &[connected(W)], vec![s1()]),
            "{case}"
        );
        as_committed(&whole, &case);
        whole.flush();
    }
}
