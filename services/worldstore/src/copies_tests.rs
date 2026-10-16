//! Tests of border copies in the store (step E3b of
//! `docs/adr/0025-effects-between-regions.md`, section 3): bases, the chain of their
//! updates, who uses a copied chunk, and what a hello forgets.
//!
//! The world is the one with a gap (`regions::gap`): region 0 west of x = 0, region 1
//! from chunk x = 16 on, and the home region 2 at the origin. Region 1 copies a chunk
//! of region 0.

use std::path::Path;
use std::thread;
use std::time::Duration;

use clustine_data::{BlockState, blocks};
use clustine_rpc::CopyUse;
use clustine_world::{BlockPos, Chunk, ChunkPos};

use super::*;
use crate::chunks::FileChunks;
use crate::disk::MemoryDisk;
use crate::regions::{claim, gap, put, store_on};
use crate::tests::{generator, load};

/// A chunk of region 0, and one of the gap.
const WEST: ChunkPos = ChunkPos::new(-3, 0);
const FREE: ChunkPos = ChunkPos::new(5, 5);

fn hello(region: u32, epoch: u64) -> RegionHello {
    RegionHello {
        region: RegionId(region),
        epoch,
    }
}

fn opened(store: &Store, region: u32) -> StoreHandle {
    store.open_region(hello(region, 1)).unwrap().0
}

/// The `n`th block of a test in `chunk`.
fn block(chunk: ChunkPos, n: i32) -> BlockPos {
    BlockPos::new(chunk.x * 16 + n, 100, chunk.z * 16 + 1)
}

fn commit(handle: &StoreHandle, tick: u64, changes: &[(BlockPos, BlockState)]) {
    handle.request(StoreRequest::Commit {
        tick,
        changes: changes.to_vec(),
        state: format!("tick {tick}").into_bytes(),
        stays: Vec::new(),
        ticks: Vec::new(),
        marks_taken: Vec::new(),
        seams: Vec::new(),
        effects: Vec::new(),
        taken: Vec::new(),
    });
}

fn copies(handle: &StoreHandle, wanted: &[ChunkPos], gone: &[ChunkPos]) {
    handle.request(StoreRequest::Copies {
        wanted: wanted.to_vec(),
        gone: gone.to_vec(),
    });
}

/// What is on the stream of `handle` up to and with the next mark, passing over every
/// other answer. A base is made on the thread for chunks, and comes on the stream
/// whenever that has done it: so this waits for the mark, not for a flush.
fn up_to_mark(handle: &StoreHandle) -> Vec<StoreReply> {
    let mut streamed = Vec::new();
    for _ in 0..30_000 {
        match handle.try_reply() {
            Some(StoreReply::Mark { .. }) => return streamed,
            Some(
                reply @ (StoreReply::CopyBase { .. }
                | StoreReply::CopyUpdate { .. }
                | StoreReply::CopyUsed { .. }
                | StoreReply::Effects { .. }),
            ) => streamed.push(reply),
            Some(_) => {}
            None => thread::sleep(Duration::from_millis(1)),
        }
    }
    panic!("no mark came: {streamed:?}");
}

/// What is on the stream of `handle` before the answer to a flush, with nothing that
/// waits for the thread for chunks being asked for.
fn streamed_before_flush(handle: &StoreHandle) -> Vec<StoreReply> {
    handle.request(StoreRequest::Flush);
    let mut streamed = Vec::new();
    while let Ok(reply) = handle.replies.recv() {
        match reply {
            StoreReply::Flushed => return streamed,
            StoreReply::Mark { .. } | StoreReply::Committed { .. } => {}
            StoreReply::Loaded { .. } | StoreReply::Generating { .. } => {}
            other => streamed.push(other),
        }
    }
    panic!("the handle was lost");
}

/// The chunk at `position` as generated, with `changes` in it.
fn with(position: ChunkPos, changes: &[(BlockPos, BlockState)]) -> Chunk {
    let mut chunk = generator().generate(position);
    for (block, state) in changes {
        let (x, z) = block.in_chunk();
        chunk.set(x, block.y, z, *state);
    }
    chunk
}

#[test]
fn a_copy_is_based_on_its_chunk_with_the_holders_commits_and_kept_up_to_date() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let holder = opened(&store, 0);
    let copier = opened(&store, 1);
    let first = (block(WEST, 0), blocks::GLASS);
    commit(&holder, 1, &[first]);
    streamed_before_flush(&holder);

    copies(&copier, &[WEST], &[]);
    let streamed = up_to_mark(&copier);
    let [
        StoreReply::CopyBase {
            position,
            sequence,
            last_change,
            chunk,
            used,
        },
    ] = &streamed[..]
    else {
        panic!("expected a base, got {streamed:?}");
    };
    assert_eq!(*position, WEST);
    // The chain begins where the copy was first asked for.
    assert_eq!(last_change, sequence);
    assert_eq!(chunk.as_ref(), Some(&with(WEST, &[first])));
    // The holder has not loaded it, and nobody uses it.
    assert_eq!(*used, CopyUse::Nobody);
    let based = *sequence;

    // Each later commit in the chunk is an update naming the change before.
    let second = (block(WEST, 1), blocks::STONE);
    let elsewhere = (block(ChunkPos::new(-4, 0), 0), blocks::STONE);
    commit(&holder, 2, &[second, elsewhere]);
    let third = (block(WEST, 0), blocks::AIR);
    commit(&holder, 3, &[third]);
    streamed_before_flush(&holder);
    let streamed = streamed_before_flush(&copier);
    let [
        StoreReply::CopyUpdate {
            position: at_second,
            sequence: second_sequence,
            previous: before_second,
            changes: second_changes,
        },
        StoreReply::CopyUpdate {
            previous: before_third,
            changes: third_changes,
            ..
        },
    ] = &streamed[..]
    else {
        panic!("expected two updates, got {streamed:?}");
    };
    assert_eq!(*at_second, WEST);
    assert_eq!(*before_second, based);
    assert!(*second_sequence > based);
    assert_eq!(second_changes, &[second]);
    assert_eq!(before_third, second_sequence);
    assert_eq!(third_changes, &[third]);

    // Its use is told as it changes.
    load(&holder, WEST);
    assert_eq!(
        streamed_before_flush(&copier),
        [StoreReply::CopyUsed {
            position: WEST,
            used: CopyUse::Uncoupled(RegionId(0)),
        }]
    );
    holder.request(StoreRequest::Dropped { chunks: vec![WEST] });
    streamed_before_flush(&holder);
    assert_eq!(
        streamed_before_flush(&copier),
        [StoreReply::CopyUsed {
            position: WEST,
            used: CopyUse::Nobody,
        }]
    );

    // A copy given up is told nothing more.
    copies(&copier, &[], &[WEST]);
    commit(&holder, 4, &[(block(WEST, 2), blocks::GLASS)]);
    load(&holder, WEST);
    streamed_before_flush(&holder);
    assert_eq!(streamed_before_flush(&copier), []);
}

#[test]
fn a_region_that_copies_a_chunk_copied_already_joins_its_chain() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let holder = opened(&store, 0);
    let first = opened(&store, 1);
    let second = opened(&store, 2);
    copies(&first, &[WEST], &[]);
    up_to_mark(&first);
    let change = (block(WEST, 0), blocks::GLASS);
    commit(&holder, 1, &[change]);
    streamed_before_flush(&holder);
    let streamed = streamed_before_flush(&first);
    let [StoreReply::CopyUpdate { sequence, .. }] = &streamed[..] else {
        panic!("expected an update, got {streamed:?}");
    };

    // Something else is written before the second copy is asked for, so that its
    // group is not the update's.
    commit(
        &second,
        1,
        &[(block(ChunkPos::new(0, 0), 0), blocks::GLASS)],
    );
    streamed_before_flush(&second);
    copies(&second, &[WEST], &[]);
    let streamed = up_to_mark(&second);
    let [
        StoreReply::CopyBase {
            sequence: based,
            last_change,
            chunk,
            ..
        },
    ] = &streamed[..]
    else {
        panic!("expected a base, got {streamed:?}");
    };
    assert_eq!(last_change, sequence);
    assert!(based > sequence);
    assert_eq!(chunk.as_ref(), Some(&with(WEST, &[change])));
    commit(&holder, 2, &[(block(WEST, 1), blocks::STONE)]);
    streamed_before_flush(&holder);
    for copier in [&first, &second] {
        let streamed = streamed_before_flush(copier);
        assert!(
            matches!(&streamed[..], [StoreReply::CopyUpdate { previous, .. }] if previous == sequence),
            "{streamed:?}"
        );
    }
}

#[test]
fn a_copy_of_land_nobody_holds_is_the_stored_chunk_and_one_that_cannot_be_read_is_absent() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let copier = opened(&store, 1);
    copies(&copier, &[FREE], &[]);
    let streamed = up_to_mark(&copier);
    assert!(
        matches!(&streamed[..], [StoreReply::CopyBase { chunk: Some(chunk), used: CopyUse::Nobody, .. }]
            if *chunk == generator().generate(FREE)),
        "{streamed:?}"
    );
    // A claim of it is told as a use.
    let other = opened(&store, 0);
    claim(&other, &[FREE]);
    assert_eq!(
        streamed_before_flush(&copier),
        [StoreReply::CopyUsed {
            position: FREE,
            used: CopyUse::Uncoupled(RegionId(0)),
        }]
    );

    let path = FileChunks::new(disk.clone(), Path::new("/world")).chunk_path(WEST);
    put(&disk, path.to_str().unwrap(), b"not a chunk");
    copies(&copier, &[WEST], &[]);
    let streamed = up_to_mark(&copier);
    assert!(
        matches!(
            &streamed[..],
            [StoreReply::CopyBase {
                position: WEST,
                chunk: None,
                ..
            }]
        ),
        "{streamed:?}"
    );
}

#[test]
fn a_region_opened_again_asks_for_its_copies_again() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let holder = opened(&store, 0);
    let copier = opened(&store, 1);
    copies(&copier, &[WEST], &[]);
    up_to_mark(&copier);
    let (again, _) = store.open_region(hello(1, 2)).unwrap();
    commit(&holder, 1, &[(block(WEST, 0), blocks::GLASS)]);
    streamed_before_flush(&holder);
    assert_eq!(streamed_before_flush(&again), []);
    // Asked for again, it is based anew, with what was committed meanwhile.
    copies(&again, &[WEST], &[]);
    let streamed = up_to_mark(&again);
    assert!(
        matches!(&streamed[..], [StoreReply::CopyBase { chunk: Some(chunk), .. }]
            if *chunk == with(WEST, &[(block(WEST, 0), blocks::GLASS)])),
        "{streamed:?}"
    );
}

#[test]
fn a_copy_is_what_its_holder_restores_with_after_saves_and_a_checkpoint() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let holder = opened(&store, 0);
    let copier = opened(&store, 1);
    let changes: Vec<(BlockPos, BlockState)> =
        (0..4).map(|n| (block(WEST, n), blocks::GLASS)).collect();
    commit(&holder, 1, &changes[..2]);
    holder.request(StoreRequest::Save {
        position: WEST,
        tick: 1,
        chunk: with(WEST, &changes[..2]),
    });
    holder.request(StoreRequest::Checkpoint {
        tick: 1,
        state: b"holder 1".to_vec(),
    });
    commit(&holder, 2, &changes[2..3]);
    commit(&holder, 3, &changes[3..]);
    streamed_before_flush(&holder);

    copies(&copier, &[WEST], &[]);
    let streamed = up_to_mark(&copier);
    let restored = store.open_region(hello(0, 2)).unwrap().0;
    let loaded = load(&restored, WEST);
    assert_eq!(loaded, with(WEST, &changes));
    assert!(
        matches!(&streamed[..], [StoreReply::CopyBase { chunk: Some(chunk), .. }] if *chunk == loaded),
        "{streamed:?}"
    );
}
