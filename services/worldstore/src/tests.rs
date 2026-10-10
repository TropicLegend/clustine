//! Tests of the store in this process. `tcp.rs` has those of a store in another one,
//! and `kill.rs` those of a store that dies.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Barrier, Mutex};
use std::time::Duration;

use clustine_data::{BlockState, blocks};
use clustine_format::{LogRecord, read_log};
use clustine_rpc::TickState;
use clustine_world::{BlockPos, Chunk, ChunkPos, EntityIds};
use clustine_worldgen::FlatGenerator;

use super::*;
use crate::disk::{Fault, MemoryDisk, Survival};

pub(crate) fn generator() -> Arc<dyn ChunkGenerator> {
    Arc::new(FlatGenerator::classic())
}

/// Waits for the next answer to `store` other than that a commit is done, which most
/// tests here do not look at.
pub(crate) fn reply(store: &StoreHandle) -> StoreReply {
    for _ in 0..30_000 {
        match store.try_reply() {
            Some(StoreReply::Committed { .. }) => {}
            Some(reply) => return reply,
            None => thread::sleep(Duration::from_millis(1)),
        }
    }
    panic!("the store did not answer");
}

/// Waits for the next answer to `store`, whatever it is.
pub(crate) fn any_reply(store: &StoreHandle) -> StoreReply {
    for _ in 0..30_000 {
        match store.try_reply() {
            Some(reply) => return reply,
            None => thread::sleep(Duration::from_millis(1)),
        }
    }
    panic!("the store did not answer");
}

/// Asks for a chunk and waits for it.
pub(crate) fn load(store: &StoreHandle, position: ChunkPos) -> Chunk {
    store.request(StoreRequest::Load { position });
    match reply(store) {
        StoreReply::Loaded {
            position: loaded,
            chunk,
        } => {
            assert_eq!(loaded, position);
            chunk
        }
        other => panic!("expected the chunk, got {other:?}"),
    }
}

pub(crate) fn save(store: &StoreHandle, position: ChunkPos, chunk: &Chunk) {
    store.request(StoreRequest::Save {
        position,
        tick: 5,
        chunk: chunk.clone(),
    });
}

pub(crate) fn edited() -> Chunk {
    let mut chunk = generator().generate(ChunkPos::new(0, 0));
    chunk.set(3, -61, 4, blocks::AIR);
    chunk.set(3, 100, 4, blocks::GLASS);
    chunk
}

/// The hello of an owner of one of two regions: 0 is west of x = 0, 1 east of it.
pub(crate) fn hello(region: u32, epoch: u64) -> RegionHello {
    RegionHello {
        region: RegionId(region),
        epoch,
    }
}

/// How the stores of these tests divide the world: into the two regions [`hello`]
/// says hello for, with the home chunk at the origin, which is the eastern region's.
pub(crate) fn division() -> Division {
    Division::side_by_side(ChunkPos::new(0, 0), &[0]).unwrap()
}

/// A store in memory for the two regions of [`division`].
pub(crate) fn memory() -> Store {
    Store::memory_divided(generator(), division()).unwrap()
}

/// A store for the two regions of [`division`] that keeps the world in `directory`.
pub(crate) fn local(directory: &Path) -> Store {
    Store::local_divided(directory, generator(), division()).unwrap()
}

/// A store of each kind.
pub(crate) fn stores(directory: &Path) -> [Store; 2] {
    [memory(), local(directory)]
}

/// Opens a region and returns its handle.
pub(crate) fn open(store: &Store, hello: RegionHello) -> StoreHandle {
    store.open_region(hello).unwrap().0
}

/// What the region's state after `tick` changed, as a test makes it up.
pub(crate) fn delta(tick: u64) -> Vec<u8> {
    format!("delta {tick}").into_bytes()
}

/// Commits the block changes of `tick`, with [`delta`] as the change of state.
pub(crate) fn log(store: &StoreHandle, tick: u64, changes: &[(i32, i32, i32, BlockState)]) {
    store.request(StoreRequest::Commit {
        state: delta(tick),
        tick,
        changes: changes
            .iter()
            .map(|(x, y, z, state)| (BlockPos::new(*x, *y, *z), *state))
            .collect(),
    });
}

/// Waits until the commit of `tick` is answered; answers before it are passed over.
pub(crate) fn committed(store: &StoreHandle, tick: u64) {
    loop {
        if any_reply(store) == (StoreReply::Committed { tick }) {
            return;
        }
    }
}

/// The ticks and changes of state a region is restored with.
fn deltas(restored: &Restored) -> Vec<(u64, Vec<u8>)> {
    restored
        .deltas
        .iter()
        .map(|delta| (delta.tick, delta.state.clone()))
        .collect()
}

/// The records of the log in the world in `directory`, segment by segment.
fn logged(directory: &Path) -> Vec<LogRecord> {
    let mut segments: Vec<_> = fs::read_dir(directory.join("log"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    segments.sort();
    segments
        .iter()
        .flat_map(|segment| read_log(&fs::read(segment).unwrap()).unwrap().0)
        .collect()
}

#[test]
fn chunks_that_were_never_stored_are_generated() {
    let directory = tempfile::tempdir().unwrap();
    let stores = [
        spawn(generator()),
        spawn_local(directory.path(), generator()).unwrap(),
    ];
    for store in stores {
        let position = ChunkPos::new(7, -9);
        assert_eq!(load(&store, position), generator().generate(position));
    }
}

#[test]
fn a_saved_chunk_is_what_is_loaded_from_then_on() {
    let directory = tempfile::tempdir().unwrap();
    let stores = [
        spawn(generator()),
        spawn_local(directory.path(), generator()).unwrap(),
    ];
    for store in stores {
        let position = ChunkPos::new(-2, 5);
        save(&store, position, &edited());
        assert_eq!(load(&store, position), edited());
        // Other chunks are not affected.
        let other = ChunkPos::new(-2, 6);
        assert_eq!(load(&store, other), generator().generate(other));
    }
}

/// Generating the chunk instead would overwrite what was built there once it is saved,
/// and saying nothing would leave the region waiting for it.
#[test]
fn a_stored_chunk_that_cannot_be_read_is_answered_as_unreadable() {
    let directory = tempfile::tempdir().unwrap();
    let position = ChunkPos::new(3, 4);
    let store = spawn_local(directory.path(), generator()).unwrap();
    save(&store, position, &edited());
    // The flush has the chunk written: until then it is held by the store, which
    // would answer the load from what it holds and not read the file.
    store.flush();
    let manifest = directory
        .path()
        .join("manifests/overworld/0.0/3.4.manifest");
    std::fs::write(&manifest, b"not a manifest").unwrap();

    store.request(StoreRequest::Load { position });
    assert_eq!(reply(&store), StoreReply::Unreadable { position });
}

#[test]
fn a_world_on_disk_outlives_the_store() {
    let directory = tempfile::tempdir().unwrap();
    let position = ChunkPos::new(100, -100);
    {
        let store = spawn_local(directory.path(), generator()).unwrap();
        save(&store, position, &edited());
        // The flush is what puts the chunk in the files: a save alone only notes it,
        // and it would go with the store.
        store.flush();
    }
    let store = spawn_local(directory.path(), generator()).unwrap();
    assert_eq!(load(&store, position), edited());
}

#[test]
fn flush_waits_for_everything_before_it() {
    let directory = tempfile::tempdir().unwrap();
    let store = spawn_local(directory.path(), generator()).unwrap();
    for x in 0..50 {
        save(&store, ChunkPos::new(x, 0), &edited());
    }
    // A load that is still unanswered does not confuse the flush.
    store.request(StoreRequest::Load {
        position: ChunkPos::new(0, 0),
    });
    // The saves are in the files once it returns, and not before: it is the flush
    // that writes them, all together.
    store.flush();
    let manifests = directory.path().join("manifests/overworld");
    let stored: usize = fs::read_dir(manifests)
        .unwrap()
        .map(|region| fs::read_dir(region.unwrap().path()).unwrap().count())
        .sum();
    assert_eq!(stored, 50);
}

#[test]
fn equal_sections_are_stored_once() {
    let directory = tempfile::tempdir().unwrap();
    let store = spawn_local(directory.path(), generator()).unwrap();
    for x in 0..20 {
        save(&store, ChunkPos::new(x, 3), &edited());
    }
    // Section files are written when the saves are flushed, each once for all the
    // chunks of the flush that have the section.
    store.flush();
    // The edited chunk has two sections that are not plain air.
    let blobs: usize = fs::read_dir(directory.path().join("blobs"))
        .unwrap()
        .map(|prefix| fs::read_dir(prefix.unwrap().path()).unwrap().count())
        .sum();
    assert_eq!(blobs, 2);
}

#[test]
fn commits_are_answered_in_order() {
    let directory = tempfile::tempdir().unwrap();
    for store in stores(directory.path()) {
        let owner = open(&store, hello(1, 1));
        for tick in 1..=20 {
            log(&owner, tick, &[(3, 100, tick as i32, blocks::STONE)]);
        }
        for tick in 1..=20 {
            assert_eq!(any_reply(&owner), StoreReply::Committed { tick });
        }
        owner.flush();
        assert_eq!(owner.try_reply(), None);
    }
}

/// Commits that were never put in a saved chunk, as after a crash, are in the world
/// when the region is opened again, and the region is restored with their states.
#[test]
fn committed_changes_are_in_the_chunks_when_the_region_is_opened_again() {
    let directory = tempfile::tempdir().unwrap();
    {
        let store = spawn_local(directory.path(), generator()).unwrap();
        // The same block twice: the later change wins. And a second chunk.
        log(
            &store,
            1,
            &[(3, -61, 4, blocks::STONE), (40, -61, 4, blocks::AIR)],
        );
        log(
            &store,
            2,
            &[(3, -61, 4, blocks::AIR), (3, 100, 4, blocks::GLASS)],
        );
        committed(&store, 2);
        // The store is dropped without the chunks ever being saved.
    }

    for _ in 0..2 {
        let store = Store::local(directory.path(), generator()).unwrap();
        let (owner, restored) = store
            .open_region(RegionHello {
                epoch: 2,
                ..whole_world()
            })
            .unwrap();
        assert_eq!(restored.state, None);
        assert_eq!(deltas(&restored), [(1, delta(1)), (2, delta(2))]);
        assert_eq!(load(&owner, ChunkPos::new(0, 0)), edited());
        let other = load(&owner, ChunkPos::new(2, 0));
        assert_eq!(other.get(8, -61, 4), Some(blocks::AIR));
        // The commits stay in the log until a checkpoint covers them, however often
        // the region is opened.
    }
}

/// A chunk saved in the middle of the commits ends up right all the same.
#[test]
fn recovery_copes_with_chunks_saved_in_between() {
    let directory = tempfile::tempdir().unwrap();
    let origin = ChunkPos::new(0, 0);
    {
        let store = spawn_local(directory.path(), generator()).unwrap();
        log(&store, 1, &[(3, -61, 4, blocks::STONE)]);
        let mut saved = generator().generate(origin);
        saved.set(3, -61, 4, blocks::STONE);
        save(&store, origin, &saved);
        log(
            &store,
            2,
            &[(3, -61, 4, blocks::AIR), (3, 100, 4, blocks::GLASS)],
        );
        store.flush();
    }
    let store = spawn_local(directory.path(), generator()).unwrap();
    assert_eq!(load(&store, origin), edited());
}

/// A checkpoint covers the commits up to its tick. Those after it may be in the log
/// already, as the region runs ahead of what is committed, and are kept.
#[test]
fn a_checkpoint_keeps_the_commits_after_it() {
    let directory = tempfile::tempdir().unwrap();
    let origin = ChunkPos::new(0, 0);
    for store in stores(directory.path()) {
        let owner = open(&store, hello(1, 1));
        log(&owner, 1, &[(3, -61, 4, blocks::AIR)]);
        log(&owner, 2, &[(3, 100, 4, blocks::GLASS)]);
        log(&owner, 3, &[(5, 100, 5, blocks::STONE)]);
        // The chunk as of tick 2, and the state after it.
        save(&owner, origin, &edited());
        owner.request(StoreRequest::Checkpoint {
            tick: 2,
            state: b"state 2".to_vec(),
        });
        log(&owner, 4, &[(6, 100, 6, blocks::STONE)]);
        owner.flush();
        drop(owner);

        let (owner, restored) = store.open_region(hello(1, 1)).unwrap();
        let state = TickState {
            tick: 2,
            state: b"state 2".to_vec(),
        };
        assert_eq!(restored.state, Some(state));
        assert_eq!(deltas(&restored), [(3, delta(3)), (4, delta(4))]);
        let mut expected = edited();
        expected.set(5, 100, 5, blocks::STONE);
        expected.set(6, 100, 6, blocks::STONE);
        assert_eq!(load(&owner, origin), expected);
    }
}

#[test]
fn a_checkpoint_that_covers_every_commit_leaves_no_log_behind() {
    let directory = tempfile::tempdir().unwrap();
    let segments = || fs::read_dir(directory.path().join("log")).unwrap().count();
    let store = local(directory.path());
    let owner = open(&store, hello(1, 1));
    log(&owner, 1, &[(3, -61, 4, blocks::AIR)]);
    log(&owner, 2, &[(3, 100, 4, blocks::GLASS)]);
    owner.flush();
    assert_eq!(segments(), 1);

    save(&owner, ChunkPos::new(0, 0), &edited());
    owner.request(StoreRequest::Checkpoint {
        tick: 2,
        state: Vec::new(),
    });
    owner.flush();
    assert_eq!(segments(), 0);

    // Committing goes on after a checkpoint, in a segment of its own.
    log(&owner, 3, &[(3, -61, 4, blocks::STONE)]);
    owner.flush();
    assert_eq!(segments(), 1);
    let records = logged(directory.path());
    assert!(matches!(
        records[..],
        [LogRecord::Commit {
            region: 1,
            tick: 3,
            ..
        }]
    ));
}

#[test]
fn commits_carry_the_region_and_the_epoch_of_the_owner() {
    let directory = tempfile::tempdir().unwrap();
    let store = local(directory.path());
    let owner = open(&store, hello(1, 7));
    log(&owner, 3, &[(3, -61, 4, blocks::AIR)]);
    owner.flush();
    let expected = [
        LogRecord::Opened {
            region: 1,
            epoch: 7,
            restored: 0,
        },
        LogRecord::Commit {
            region: 1,
            tick: 3,
            epoch: 7,
            changes: vec![(BlockPos::new(3, -61, 4), blocks::AIR)],
            state: delta(3),
        },
    ];
    assert_eq!(logged(directory.path()), expected);
}

/// The process can die in the middle of appending to the log. Whatever is left of the
/// last record is ignored and the records before it are recovered.
#[test]
fn a_log_cut_off_in_the_middle_of_a_record_is_recovered_up_to_there() {
    let directory = tempfile::tempdir().unwrap();
    let origin = ChunkPos::new(0, 0);
    let segment = directory.path().join("log/00000000000000000001.wal");
    {
        let store = spawn_local(directory.path(), generator()).unwrap();
        log(&store, 1, &[(3, -61, 4, blocks::AIR)]);
        log(&store, 2, &[(3, 100, 4, blocks::GLASS)]);
        log(&store, 3, &[(5, 100, 5, blocks::STONE)]);
        store.flush();
    }
    let complete = fs::read(&segment).unwrap();
    let marker = LogRecord::Opened {
        region: 0,
        epoch: 1,
        restored: 0,
    }
    .encode()
    .len();
    // Three records of the same length after the marker; cut at the start of, just
    // into, and just before the end of the third.
    let record = (complete.len() - marker) / 3;
    for length in [2 * record, 2 * record + 1, 3 * record - 1] {
        let length = marker + length;
        fs::write(&segment, &complete[..length]).unwrap();
        // Opening the region from an earlier round has saved the chunk, and put what
        // it was restored with in the log; start from scratch.
        let _ = fs::remove_dir_all(directory.path().join("manifests"));
        for name in fs::read_dir(directory.path().join("log")).unwrap() {
            let path = name.unwrap().path();
            if path != segment {
                fs::remove_file(path).unwrap();
            }
        }
        let store = Store::local(directory.path(), generator()).unwrap();
        let (owner, restored) = store.open_region(whole_world()).unwrap();
        assert_eq!(load(&owner, origin), edited(), "cut at {length}");
        assert_eq!(deltas(&restored), [(1, delta(1)), (2, delta(2))]);
    }
}

/// Each region has a lane of its own, and a checkpoint only says something about the
/// region that makes it.
#[test]
fn regions_commit_and_checkpoint_independently() {
    let directory = tempfile::tempdir().unwrap();
    let (west_chunk, east_chunk) = (ChunkPos::new(-1, 0), ChunkPos::new(0, 0));
    let mut dug = generator().generate(west_chunk);
    dug.set(13, -61, 4, blocks::AIR);
    {
        let store = local(directory.path());
        let west = open(&store, hello(0, 1));
        let east = open(&store, hello(1, 1));
        log(&west, 1, &[(-3, -61, 4, blocks::AIR)]);
        log(
            &east,
            1,
            &[(3, -61, 4, blocks::AIR), (3, 100, 4, blocks::GLASS)],
        );
        // The west saves what it changed and makes a checkpoint.
        save(&west, west_chunk, &dug);
        west.request(StoreRequest::Checkpoint {
            tick: 1,
            state: b"west".to_vec(),
        });
        west.flush();
        east.flush();
        // The server dies before the east has saved anything.
    }

    let store = local(directory.path());
    let (west, restored_west) = store.open_region(hello(0, 1)).unwrap();
    let (east, restored_east) = store.open_region(hello(1, 1)).unwrap();
    assert_eq!(load(&east, east_chunk), edited());
    assert_eq!(load(&west, west_chunk), dug);
    assert_eq!(restored_west.state.unwrap().state, b"west");
    assert_eq!(restored_west.deltas, []);
    assert_eq!(restored_east.state, None);
    assert_eq!(deltas(&restored_east), [(1, delta(1))]);
}

/// When a world is opened with another division than it was last run with, what the
/// regions of the old one committed goes into the chunks: the regions of the new one
/// know nothing of it.
#[test]
fn a_world_opened_with_another_division_has_what_the_regions_of_the_old_one_committed() {
    let directory = tempfile::tempdir().unwrap();
    {
        let store = local(directory.path());
        let west = open(&store, hello(0, 3));
        let east = open(&store, hello(1, 4));
        log(&west, 1, &[(-3, -61, 4, blocks::AIR)]);
        log(
            &east,
            1,
            &[(3, -61, 4, blocks::AIR), (3, 100, 4, blocks::GLASS)],
        );
        east.request(StoreRequest::Checkpoint {
            tick: 0,
            state: b"east".to_vec(),
        });
        west.flush();
        east.flush();
    }

    // A single region this time. Its epochs go on from those of the region that had the
    // same number.
    let single = RegionHello {
        epoch: 5,
        ..whole_world()
    };
    for _ in 0..2 {
        let store = Store::local(directory.path(), generator()).unwrap();
        let (owner, restored) = store.open_region(single).unwrap();
        assert_eq!(restored.state, None);
        assert_eq!(restored.deltas, []);
        assert_eq!(load(&owner, ChunkPos::new(0, 0)), edited());
        let west_chunk = load(&owner, ChunkPos::new(-1, 0));
        assert_eq!(west_chunk.get(13, -61, 4), Some(blocks::AIR));
    }
    // Nothing of the old regions is left to be restored when the old division comes
    // back.
    let store = local(directory.path());
    let (_, restored) = store.open_region(hello(1, 4)).unwrap();
    assert_eq!((restored.state, restored.deltas), (None, Vec::new()));
    // The world is divided as the store was told when it started: the list has the two
    // regions side by side, and there is no file `layout`.
    let list = store.regions().unwrap();
    let area = |min_x, max_x| ChunkArea { min_x, max_x };
    let listed: Vec<_> = list
        .regions
        .iter()
        .map(|info| (info.region, info.pinned.clone()))
        .collect();
    assert_eq!(
        listed,
        [
            (RegionId(0), vec![area(None, Some(0))]),
            (RegionId(1), vec![area(Some(0), None)])
        ]
    );
    assert_eq!(list.home, RegionId(1));
    assert!(!directory.path().join("layout").exists());
}

#[test]
fn a_record_cut_off_at_the_end_of_the_log_loses_only_itself() {
    let directory = tempfile::tempdir().unwrap();
    let (west_chunk, east_chunk) = (ChunkPos::new(-1, 0), ChunkPos::new(0, 0));
    {
        let store = local(directory.path());
        let west = open(&store, hello(0, 1));
        let east = open(&store, hello(1, 1));
        for (handle, x) in [(&west, -3), (&east, 3)] {
            log(handle, 1, &[(x, -61, 4, blocks::AIR)]);
            log(handle, 2, &[(x, 100, 4, blocks::GLASS)]);
            handle.flush();
        }
    }
    // The server died while the east was appending its second record.
    let segment = directory.path().join("log/00000000000000000001.wal");
    let complete = fs::read(&segment).unwrap();
    fs::write(&segment, &complete[..complete.len() - 1]).unwrap();

    let store = local(directory.path());
    let (west, restored_west) = store.open_region(hello(0, 1)).unwrap();
    let (east, restored_east) = store.open_region(hello(1, 1)).unwrap();
    assert_eq!(deltas(&restored_west), [(1, delta(1)), (2, delta(2))]);
    assert_eq!(deltas(&restored_east), [(1, delta(1))]);
    let mut dug = generator().generate(west_chunk);
    dug.set(13, -61, 4, blocks::AIR);
    dug.set(13, 100, 4, blocks::GLASS);
    assert_eq!(load(&west, west_chunk), dug);
    let mut dug = generator().generate(east_chunk);
    dug.set(3, -61, 4, blocks::AIR);
    assert_eq!(load(&east, east_chunk), dug);
}

#[test]
fn answers_reach_only_the_handle_that_asked() {
    let directory = tempfile::tempdir().unwrap();
    for store in stores(directory.path()) {
        let west = open(&store, hello(0, 1));
        let east = open(&store, hello(1, 1));
        let (here, there) = (ChunkPos::new(-4, 2), ChunkPos::new(6, 2));
        let loaded = |position| StoreReply::Loaded {
            position,
            chunk: generator().generate(position),
        };
        west.request(StoreRequest::Load { position: here });
        east.request(StoreRequest::Load { position: there });
        west.request(StoreRequest::Flush);
        east.request(StoreRequest::Load { position: there });
        east.request(StoreRequest::Flush);

        assert_eq!(reply(&east), loaded(there));
        assert_eq!(reply(&east), loaded(there));
        assert_eq!(reply(&east), StoreReply::Flushed);
        assert_eq!(east.try_reply(), None);
        // The east asked last, so the west has been answered by now.
        assert_eq!(west.try_reply(), Some(loaded(here)));
        assert_eq!(west.try_reply(), Some(StoreReply::Flushed));
        assert_eq!(west.try_reply(), None);
    }
}

/// Each region is run by a thread of its own, which opens the region and uses it.
#[test]
fn regions_are_opened_and_used_from_threads_of_their_own() {
    let directory = tempfile::tempdir().unwrap();
    for store in stores(directory.path()) {
        thread::scope(|scope| {
            for (region, x) in [(0, -1), (1, 0)] {
                let store = store.clone();
                scope.spawn(move || {
                    let handle = open(&store, hello(region, 1));
                    let position = ChunkPos::new(x, 0);
                    let mut built = generator().generate(position);
                    built.set(1, 80, 1, blocks::STONE);
                    save(&handle, position, &built);
                    assert_eq!(load(&handle, position), built);
                    handle.flush();
                });
            }
        });
    }
}

/// A world that was last opened before regions had a state has a log per region,
/// `logs/<region>.wal`, or, from before there were regions, a single one, `wal`. Both
/// hold block changes alone.
#[test]
fn the_logs_of_a_world_from_before_regions_had_a_state_are_carried_over() {
    let origin = ChunkPos::new(0, 0);
    let record = |tick, (x, y, z), state| LogRecord::Changes {
        tick,
        epoch: 1,
        changes: vec![(BlockPos::new(x, y, z), state)],
    };
    let mut first = record(1, (3, -61, 4), blocks::AIR).encode();
    first.extend(record(2, (3, 100, 4), blocks::GLASS).encode());
    // And a record the server died in the middle of.
    first.extend(&record(3, (5, 100, 5), blocks::STONE).encode()[..9]);
    let other = record(1, (-3, -61, 4), blocks::AIR).encode();

    for old in ["wal", "logs/0.wal"] {
        let directory = tempfile::tempdir().unwrap();
        drop(spawn_local(directory.path(), generator()).unwrap());
        fs::create_dir_all(directory.path().join("logs")).unwrap();
        fs::write(directory.path().join(old), &first).unwrap();
        fs::write(directory.path().join("logs/1.wal"), &other).unwrap();

        let store = spawn_local(directory.path(), generator()).unwrap();
        assert_eq!(load(&store, origin), edited());
        let west = load(&store, ChunkPos::new(-1, 0));
        assert_eq!(west.get(13, -61, 4), Some(blocks::AIR));
        assert!(!directory.path().join("wal").exists());
        assert!(!directory.path().join("logs").exists());

        // The changes are in saved chunks, which are there without any log.
        drop(store);
        let store = spawn_local(directory.path(), generator()).unwrap();
        assert_eq!(load(&store, origin), edited());
    }
}

/// An owner can go away without a checkpoint while the store runs on, as when its
/// process dies. What it committed is then in the log only.
#[test]
fn a_region_opened_again_has_what_its_last_owner_committed() {
    let directory = tempfile::tempdir().unwrap();
    let origin = ChunkPos::new(0, 0);
    let manifest = directory
        .path()
        .join("manifests/overworld/0.0/0.0.manifest");
    let store = local(directory.path());
    let first = open(&store, hello(1, 1));
    log(&first, 1, &[(3, -61, 4, blocks::STONE)]);
    log(
        &first,
        2,
        &[(3, -61, 4, blocks::AIR), (3, 100, 4, blocks::GLASS)],
    );
    first.flush();
    drop(first);
    assert!(!manifest.exists());

    // The commits are applied to the stored chunks before the region is handed out,
    // and an opening writes what it applied: the manifest is there without a flush.
    let (second, restored) = store.open_region(hello(1, 1)).unwrap();
    assert_eq!(deltas(&restored), [(1, delta(1)), (2, delta(2))]);
    assert!(manifest.exists());
    assert_eq!(load(&second, origin), edited());
}

#[test]
fn an_owner_with_a_higher_epoch_replaces_the_one_there_is() {
    let directory = tempfile::tempdir().unwrap();
    let origin = ChunkPos::new(0, 0);
    for store in stores(directory.path()) {
        let old = open(&store, hello(1, 1));
        // Done, because the region is still its own.
        save(&old, origin, &edited());
        assert!(!old.is_lost());
        // Whether or not this is answered before the new owner is there, the answer is
        // not given out afterwards.
        old.request(StoreRequest::Load { position: origin });
        let new = open(&store, hello(1, 2));
        assert!(old.is_lost() && !new.is_lost());

        // Nothing the old owner asks for is done any more, and none of it answered.
        save(&old, origin, &generator().generate(origin));
        old.request(StoreRequest::Load { position: origin });
        log(&old, 1, &[(3, 100, 4, blocks::STONE)]);
        // Returns although the store does not answer it.
        old.flush();
        // Once this is answered, the store has been through all of the above.
        new.flush();
        assert_eq!(old.try_reply(), None);
        assert_eq!(load(&new, origin), edited());

        // The region is not the old owner's to give up either, nor to open again.
        drop(old);
        assert!(matches!(
            store.open_region(hello(1, 1)),
            Err(StoreError::EpochRefused { seen: 2, .. })
        ));
        assert!(!new.is_lost());
        assert_eq!(load(&new, origin), edited());
    }
}

/// Generates what [`generator`] does, but stops before the chunk at [`HELD`] until it
/// is let go on. The thread for chunks is busy meanwhile, and what it is given queues
/// up.
pub(crate) struct Held(pub(crate) Barrier);

pub(crate) const HELD: ChunkPos = ChunkPos::new(1000, 1000);

impl ChunkGenerator for Held {
    fn generate(&self, position: ChunkPos) -> Chunk {
        if position == HELD {
            // Once to say that the store has got here, once to be let go on.
            self.0.wait();
            self.0.wait();
        }
        generator().generate(position)
    }

    fn settings(&self) -> String {
        generator().settings()
    }
}

/// The hello of a new owner takes its turn among the requests. What the old owner asked
/// for before it is done; what is behind it, already waiting or not, is not.
#[test]
fn what_a_replaced_owner_has_queued_or_asks_for_later_is_not_done() {
    let directory = tempfile::tempdir().unwrap();
    let origin = ChunkPos::new(0, 0);
    let mut dug = generator().generate(origin);
    dug.set(3, -61, 4, blocks::AIR);
    // What the old owner tries once it has been replaced.
    let mut overwritten = generator().generate(origin);
    overwritten.set(9, 90, 9, blocks::STONE);
    let meddle = |old: &StoreHandle| {
        save(old, origin, &overwritten);
        log(old, 9, &[(5, 100, 5, blocks::STONE)]);
        old.request(StoreRequest::Checkpoint {
            tick: 9,
            state: b"meddled".to_vec(),
        });
        old.request(StoreRequest::Load { position: origin });
    };

    let held = Arc::new(Held(Barrier::new(2)));
    let store = Store::local_divided(directory.path(), held.clone(), division()).unwrap();
    let old = open(&store, hello(1, 1));
    log(&old, 1, &[(3, -61, 4, blocks::AIR)]);

    // While the thread for chunks is busy, the hello of a new owner arrives, and after
    // it more requests of the old one.
    old.request(StoreRequest::Load { position: HELD });
    held.0.wait();
    let (answer, answered) = mpsc::channel();
    let open = Message::Open {
        hello: hello(1, 2),
        reply_to: store.messages.clone(),
        answer,
    };
    store.messages.send(open).unwrap();
    meddle(&old);
    held.0.wait();
    let (opened, restored) = answered.recv().unwrap().unwrap();
    let new = StoreHandle::local(opened, store.messages.clone());

    // The new owner hears nothing that was meant for the old one, finds what the old
    // one committed before the hello, and nothing of what was waiting behind it.
    assert_eq!(deltas(&restored), [(1, delta(1))]);
    new.request(StoreRequest::Flush);
    assert_eq!(reply(&new), StoreReply::Flushed);
    assert_eq!(load(&new, origin), dug);
    // Not even the chunk the store was busy with is answered.
    assert_eq!(old.try_reply(), None);

    // The same goes for what the old owner asks for from now on.
    log(&new, 2, &[(3, 100, 4, blocks::GLASS)]);
    new.flush();
    meddle(&old);
    old.flush();
    new.flush();
    assert_eq!(old.try_reply(), None);
    assert_eq!(load(&new, origin), dug);

    // After a crash the world is as the new owner left it.
    drop((old, new, store));
    let store = local(directory.path());
    let (owner, restored) = store.open_region(hello(1, 3)).unwrap();
    assert_eq!(restored.state, None);
    assert_eq!(deltas(&restored), [(1, delta(1)), (2, delta(2))]);
    assert_eq!(load(&owner, origin), edited());
}

/// An owner that comes back with its own epoch, having lost its handle before the store
/// noticed, takes its region over from itself. A lower epoch is refused, and told the
/// highest there has been.
#[test]
fn an_owner_gives_way_to_its_own_epoch_or_a_higher_one_and_refuses_a_lower_one() {
    let directory = tempfile::tempdir().unwrap();
    let origin = ChunkPos::new(0, 0);
    for store in stores(directory.path()) {
        let owner = open(&store, hello(1, 5));
        save(&owner, origin, &edited());
        let Err(error) = store.open_region(hello(1, 4)) else {
            panic!("the region was opened with a lower epoch");
        };
        assert!(
            matches!(
                error,
                StoreError::EpochRefused {
                    region: RegionId(1),
                    offered: 4,
                    seen: 5,
                }
            ),
            "{error}"
        );
        // The owner is none the worse for it.
        assert_eq!(load(&owner, origin), edited());
        assert!(!owner.is_lost());

        let again = open(&store, hello(1, 5));
        assert!(owner.is_lost() && !again.is_lost());
        log(&owner, 1, &[(3, 100, 4, blocks::STONE)]);
        owner.flush();
        assert_eq!(load(&again, origin), edited());
        drop(owner);
        assert!(!again.is_lost());

        // Another region has an owner and epochs of its own.
        let neighbour = open(&store, hello(0, 1));
        let position = ChunkPos::new(-1, 0);
        assert_eq!(load(&neighbour, position), generator().generate(position));
    }
}

#[test]
fn a_region_given_up_is_opened_again_with_the_same_epoch_or_a_higher_one() {
    let directory = tempfile::tempdir().unwrap();
    let origin = ChunkPos::new(0, 0);
    for store in stores(directory.path()) {
        let refused = |epoch, highest| {
            matches!(
                store.open_region(hello(1, epoch)),
                Err(StoreError::EpochRefused {
                    region: RegionId(1),
                    offered,
                    seen,
                }) if offered == epoch && seen == highest
            )
        };
        let owner = open(&store, hello(1, 5));
        save(&owner, origin, &edited());
        drop(owner);
        assert!(refused(4, 5));

        let owner = open(&store, hello(1, 5));
        assert_eq!(load(&owner, origin), edited());
        drop(owner);

        let owner = open(&store, hello(1, 7));
        assert_eq!(load(&owner, origin), edited());
        drop(owner);
        // The highest epoch counts, not the one the region was opened with first.
        assert!(refused(6, 7));
    }
}

/// The epochs and the entity ids are on disk, and a store that is started again knows
/// them.
#[test]
fn entity_ids_and_the_highest_epoch_outlive_the_store() {
    let directory = tempfile::tempdir().unwrap();
    let (west_ids, east_ids) = {
        let store = local(directory.path());
        let (_west, west) = store.open_region(hello(0, 3)).unwrap();
        let (_east, east) = store.open_region(hello(1, 1)).unwrap();
        (west.entity_ids, east.entity_ids)
    };
    assert_ne!(west_ids, east_ids);
    for ids in [west_ids, east_ids] {
        assert!((0..EntityIds::BLOCK_COUNT).any(|index| EntityIds::block(index) == Some(ids)));
    }

    let store = local(directory.path());
    assert!(matches!(
        store.open_region(hello(0, 2)),
        Err(StoreError::EpochRefused { seen: 3, .. })
    ));
    let (_east, east) = store.open_region(hello(1, 9)).unwrap();
    assert_eq!(east.entity_ids, east_ids);
    let (_west, west) = store.open_region(hello(0, 3)).unwrap();
    assert_eq!(west.entity_ids, west_ids);

    // In memory, a region opened for the first time gets a block of its own all the same.
    let store = memory();
    let (_, first) = store.open_region(hello(1, 1)).unwrap();
    let (_, second) = store.open_region(hello(0, 1)).unwrap();
    let (_, again) = store.open_region(hello(1, 2)).unwrap();
    assert_ne!(first.entity_ids, second.entity_ids);
    assert_eq!(first.entity_ids, again.entity_ids);
}

#[test]
fn a_world_is_tied_to_its_generator() {
    struct Other;
    impl ChunkGenerator for Other {
        fn generate(&self, position: ChunkPos) -> Chunk {
            FlatGenerator::classic().generate(position)
        }
        fn settings(&self) -> String {
            "something else".to_owned()
        }
    }

    let directory = tempfile::tempdir().unwrap();
    drop(spawn_local(directory.path(), generator()).unwrap());
    // Opening it again with the same generator is fine.
    drop(spawn_local(directory.path(), generator()).unwrap());
    let Err(error) = spawn_local(directory.path(), Arc::new(Other)) else {
        panic!("the world was opened with another generator");
    };
    assert!(
        matches!(
            error,
            StoreError::Incompatible {
                setting: "generator",
                ..
            }
        ),
        "{error}"
    );
}

#[test]
fn a_damaged_chunk_is_not_replaced_by_a_generated_one() {
    let directory = tempfile::tempdir().unwrap();
    let position = ChunkPos::new(1, 1);
    let store = spawn_local(directory.path(), generator()).unwrap();
    save(&store, position, &edited());
    // There is a manifest to damage only once the save is flushed, and only from
    // then on does a load read it.
    store.flush();

    let manifest = directory
        .path()
        .join("manifests/overworld/0.0/1.1.manifest");
    let mut bytes = fs::read(&manifest).unwrap();
    bytes[10] ^= 0xFF;
    fs::write(&manifest, bytes).unwrap();

    store.request(StoreRequest::Load { position });
    assert_eq!(reply(&store), StoreReply::Unreadable { position });
    // The store carries on with other chunks.
    let other = ChunkPos::new(2, 2);
    assert_eq!(load(&store, other), generator().generate(other));
}

/// Keeps chunks in memory, but a save of the chunk at [`HELD`] waits until the test
/// lets it go on, keeping the thread for chunks busy.
struct HeldSaves {
    chunks: chunks::MemoryChunks,
    barrier: Arc<Barrier>,
}

impl Chunks for HeldSaves {
    fn load(&mut self, position: ChunkPos) -> Result<Option<Chunk>, StoreError> {
        self.chunks.load(position)
    }

    fn save(&mut self, position: ChunkPos, tick: u64, chunk: &Chunk) -> Result<(), StoreError> {
        if position == HELD {
            // Once to say that the save has started, once to be let go on.
            self.barrier.wait();
            self.barrier.wait();
        }
        self.chunks.save(position, tick, chunk)
    }

    fn sync(&mut self) -> Result<(), StoreError> {
        Ok(())
    }
}

/// A store in memory whose saves of [`HELD`] wait for the barrier.
fn held_saves() -> (Store, Arc<Barrier>, Arc<MemoryDisk>) {
    let barrier = Arc::new(Barrier::new(2));
    let disk = Arc::new(MemoryDisk::default());
    let chunks = HeldSaves {
        chunks: chunks::MemoryChunks::default(),
        barrier: Arc::clone(&barrier),
    };
    let store = start(
        disk.clone(),
        Path::new("/world"),
        Box::new(chunks),
        generator(),
        &division(),
    )
    .unwrap();
    (store, barrier, disk)
}

/// Saving and loading chunks happens apart from committing, so that a checkpoint of many
/// chunks never keeps a commit waiting.
#[test]
fn commits_are_answered_while_a_save_is_under_way() {
    let (store, barrier, _) = held_saves();
    let west = open(&store, hello(0, 1));
    let east = open(&store, hello(1, 1));
    // The chunk is the eastern region's, and only its holder saves it.
    save(&east, HELD, &edited());
    barrier.wait();

    // The save is under way and does not end before it is let go.
    log(&east, 1, &[(3, 100, 4, blocks::STONE)]);
    committed(&east, 1);
    log(&west, 1, &[(-3, 100, 4, blocks::STONE)]);
    committed(&west, 1);
    west.request(StoreRequest::Checkpoint {
        tick: 1,
        state: Vec::new(),
    });
    log(&west, 2, &[(-3, 100, 5, blocks::STONE)]);
    committed(&west, 2);
    log(&east, 2, &[(3, 100, 5, blocks::STONE)]);
    committed(&east, 2);

    barrier.wait();
    west.flush();
    assert_eq!(load(&east, HELD), edited());
}

/// A load that follows a save of the same chunk finds what was saved, also when the save
/// is still waiting for its turn, or for the commit before it.
#[test]
fn a_load_after_a_save_finds_what_was_saved_even_while_the_save_waits() {
    let directory = tempfile::tempdir().unwrap();
    let held = Arc::new(Held(Barrier::new(2)));
    let store = Store::local_divided(directory.path(), held.clone(), division()).unwrap();
    let owner = open(&store, hello(1, 1));
    let origin = ChunkPos::new(0, 0);

    // The thread for chunks is busy; a commit, a save after it, and a load.
    owner.request(StoreRequest::Load { position: HELD });
    held.0.wait();
    log(&owner, 1, &[(3, -61, 4, blocks::AIR)]);
    save(&owner, origin, &edited());
    owner.request(StoreRequest::Load { position: origin });
    let mut other = edited();
    other.set(1, 1, 1, blocks::GLASS);
    log(&owner, 2, &[(1, 1, 1, blocks::GLASS)]);
    save(&owner, origin, &other);
    owner.request(StoreRequest::Load { position: origin });
    held.0.wait();

    assert!(matches!(reply(&owner), StoreReply::Loaded { position, .. } if position == HELD));
    assert_eq!(
        reply(&owner),
        StoreReply::Loaded {
            position: origin,
            chunk: edited()
        }
    );
    assert_eq!(
        reply(&owner),
        StoreReply::Loaded {
            position: origin,
            chunk: other
        }
    );
}

/// A disk in memory with switches that make writes to the log, or syncs of it, fail, or
/// hold a sync of it until the test lets it go on.
pub(crate) struct Switched {
    pub(crate) disk: MemoryDisk,
    pub(crate) failing_appends: AtomicBool,
    pub(crate) failing_syncs: AtomicBool,
    /// Holds the next sync of the log, whatever it is of. A hello is answered before
    /// the group it is in ends, and its record in the log is synced only then: a test
    /// that sets this right after a hello may hold that sync instead of the one it
    /// means, with what it meant to be two groups in one. So a handle is flushed first:
    /// a flush is answered behind the sync of its group, and the store writes nothing
    /// after it that it is not asked for.
    pub(crate) holding_syncs: AtomicBool,
    /// Waited on twice by a sync that is held: once to say it is there, once to go on.
    pub(crate) held: Barrier,
    /// The paths of the log that were synced, or were to be.
    synced: Mutex<Vec<PathBuf>>,
}

impl Default for Switched {
    fn default() -> Self {
        Self {
            disk: MemoryDisk::default(),
            failing_appends: AtomicBool::new(false),
            failing_syncs: AtomicBool::new(false),
            holding_syncs: AtomicBool::new(false),
            held: Barrier::new(2),
            synced: Mutex::new(Vec::new()),
        }
    }
}

impl Switched {
    fn of_log(path: &Path) -> bool {
        path.extension().is_some_and(|extension| extension == "wal")
    }
}

impl Disk for Switched {
    fn read(&self, path: &Path) -> io::Result<Option<Vec<u8>>> {
        self.disk.read(path)
    }
    fn read_at(&self, path: &Path, offset: u64, length: usize) -> io::Result<Vec<u8>> {
        self.disk.read_at(path, offset, length)
    }
    fn exists(&self, path: &Path) -> io::Result<bool> {
        self.disk.exists(path)
    }
    fn list(&self, directory: &Path) -> io::Result<Vec<String>> {
        self.disk.list(directory)
    }
    fn create_dir_all(&self, directory: &Path) -> io::Result<()> {
        self.disk.create_dir_all(directory)
    }
    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        self.disk.write(path, contents)
    }
    fn append(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        if Self::of_log(path) && self.failing_appends.load(Ordering::SeqCst) {
            // Half of it gets there, as when the disk fills up.
            self.disk.append(path, &contents[..contents.len() / 2])?;
            return Err(io::Error::other("the disk is full"));
        }
        self.disk.append(path, contents)
    }
    fn truncate(&self, path: &Path, length: u64) -> io::Result<()> {
        self.disk.truncate(path, length)
    }
    fn sync(&self, path: &Path) -> io::Result<()> {
        if Self::of_log(path) {
            self.synced.lock().unwrap().push(path.to_owned());
            // Whether it fails is decided before it is held.
            let failing = self.failing_syncs.load(Ordering::SeqCst);
            if self.holding_syncs.swap(false, Ordering::SeqCst) {
                self.held.wait();
                self.held.wait();
            }
            if failing {
                return Err(io::Error::other("the disk went away"));
            }
        }
        self.disk.sync(path)
    }
    fn sync_directory(&self, directory: &Path) -> io::Result<()> {
        self.disk.sync_directory(directory)
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.disk.rename(from, to)
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        self.disk.remove(path)
    }
}

fn switched() -> (Store, Arc<Switched>) {
    switched_for(&division())
}

/// A store on a disk with switches, for a world divided as `division` says.
pub(crate) fn switched_for(division: &Division) -> (Store, Arc<Switched>) {
    let disk = Arc::new(Switched::default());
    let root = Path::new("/world");
    let chunks = FileChunks::new(disk.clone(), root);
    let store = start(disk.clone(), root, Box::new(chunks), generator(), division).unwrap();
    (store, disk)
}

/// A commit that did not reach the disk is never answered, and loses the handle: its
/// owner opens the region again, restored from what is on disk. Neither is the save
/// after it done.
#[test]
fn a_commit_whose_sync_fails_is_not_answered_and_loses_the_handle() {
    let (store, disk) = switched();
    let origin = ChunkPos::new(0, 0);
    let owner = open(&store, hello(1, 1));
    let bystander = open(&store, hello(0, 1));
    log(&owner, 1, &[(3, -61, 4, blocks::AIR)]);
    committed(&owner, 1);
    let segment = Path::new("/world/log/00000000000000000001.wal");
    let durable = disk.read(segment).unwrap().unwrap().len();

    disk.failing_syncs.store(true, Ordering::SeqCst);
    disk.synced.lock().unwrap().clear();
    log(&owner, 2, &[(3, 100, 4, blocks::GLASS)]);
    let mut saved = generator().generate(origin);
    saved.set(3, -61, 4, blocks::AIR);
    saved.set(3, 100, 4, blocks::GLASS);
    save(&owner, origin, &saved);
    owner.flush();
    assert!(owner.is_lost());
    // A region that wrote nothing in that group is lost as well: what it asked for in
    // it may have gone with it unanswered (ADR-0011, section 4.1).
    bystander.flush();
    assert!(bystander.is_lost());
    disk.failing_syncs.store(false, Ordering::SeqCst);

    let (again, restored) = store.open_region(hello(1, 1)).unwrap();
    assert_eq!(deltas(&restored), [(1, delta(1))]);
    let mut expected = generator().generate(origin);
    expected.set(3, -61, 4, blocks::AIR);
    assert_eq!(load(&again, origin), expected);

    // The sync that failed is not tried again for what it was to make durable. The
    // segment is synced once more, after it was cut back, so that what was cut off is
    // gone for good: it is no longer than what was durable, and what follows goes to
    // another segment.
    log(&again, 2, &[(3, 100, 4, blocks::GLASS)]);
    committed(&again, 2);
    assert_eq!(disk.read(segment).unwrap().unwrap().len(), durable);
    let synced = disk.synced.lock().unwrap().clone();
    assert_eq!(synced[0], segment);
    let last = synced.iter().rposition(|path| path == segment).unwrap();
    assert!(last > 0, "{synced:?}");
    assert!(last + 1 < synced.len(), "{synced:?}");
}

/// A hello that arrives while commits of the old owner wait for their sync is looked at
/// only once they are durable, or have failed: the new owner is restored with what is
/// on disk, and nothing that a failed sync takes away again.
#[test]
fn a_new_owner_is_restored_only_with_what_is_durable() {
    let (store, disk) = switched();
    let old = open(&store, hello(1, 1));
    // The opening is durable by itself, so that the sync held is the first commit's.
    old.flush();
    // The sync of the first commit is held, so that what follows waits for one group.
    disk.holding_syncs.store(true, Ordering::SeqCst);
    log(&old, 1, &[(3, -61, 4, blocks::AIR)]);
    disk.held.wait();
    log(&old, 2, &[(3, 100, 4, blocks::GLASS)]);
    let (answer, answered) = mpsc::channel();
    let open = Message::Open {
        hello: hello(1, 2),
        reply_to: store.messages.clone(),
        answer,
    };
    store.messages.send(open).unwrap();
    // The syncs of the next group fail.
    disk.failing_syncs.store(true, Ordering::SeqCst);
    disk.held.wait();

    // The hello is looked at while syncs still fail: what was cut off the log is not
    // durably gone, and until it is nobody is given a region (ADR-0011, section 4.1).
    assert!(matches!(answered.recv().unwrap(), Err(StoreError::Io(_))));
    // The first commit was answered before the old owner was lost, the second never is.
    // Its answers end once the store has let go of it.
    let replies: Vec<_> = old.replies.iter().collect();
    assert_eq!(replies, [StoreReply::Committed { tick: 1 }]);
    assert!(old.is_lost());
    disk.failing_syncs.store(false, Ordering::SeqCst);
    let (new, restored) = store.open_region(hello(1, 2)).unwrap();
    assert_eq!(deltas(&restored), [(1, delta(1))]);
    drop(new);
    let (_, again) = store.open_region(hello(1, 3)).unwrap();
    assert_eq!(again.deltas, restored.deltas);
}

/// A record that was only written in part is cut off, so that it hides nothing.
#[test]
fn a_commit_that_cannot_be_written_is_cut_off_and_loses_the_handle() {
    let (store, disk) = switched();
    let owner = open(&store, hello(1, 1));
    let other = open(&store, hello(0, 1));
    log(&owner, 1, &[(3, -61, 4, blocks::AIR)]);
    committed(&owner, 1);
    let segment = Path::new("/world/log/00000000000000000001.wal");
    let length = disk.read(segment).unwrap().unwrap().len();

    disk.failing_appends.store(true, Ordering::SeqCst);
    log(&owner, 2, &[(3, 100, 4, blocks::GLASS)]);
    owner.flush();
    assert!(owner.is_lost());
    disk.failing_appends.store(false, Ordering::SeqCst);
    assert_eq!(disk.read(segment).unwrap().unwrap().len(), length);

    // The other region is lost too, and both commit on once they have opened their
    // regions again.
    other.flush();
    assert!(other.is_lost());
    let other = open(&store, hello(0, 1));
    log(&other, 1, &[(-3, -61, 4, blocks::AIR)]);
    committed(&other, 1);
    let (again, restored) = store.open_region(hello(1, 1)).unwrap();
    assert_eq!(deltas(&restored), [(1, delta(1))]);
    log(&again, 2, &[(3, 100, 4, blocks::GLASS)]);
    committed(&again, 2);
    drop((owner, other, again));
    let (_, restored) = store.open_region(hello(1, 2)).unwrap();
    assert_eq!(deltas(&restored), [(1, delta(1)), (2, delta(2))]);
}

/// F1 of ADR-0011: after a sync of the log has failed, every handle is lost, also that
/// of a region that asked for nothing, and nothing of that group is answered.
#[test]
fn a_failed_sync_loses_every_handle_and_answers_nothing_of_its_group() {
    let (store, disk) = switched();
    let owner = open(&store, hello(1, 1));
    let bystander = open(&store, hello(0, 1));
    // Both openings are durable by themselves, in the one log, so that the sync held is
    // the first commit's. The owner's flush does that; the bystander asks for nothing.
    owner.flush();
    // The sync of the first commit is held, so that what follows is one group.
    disk.holding_syncs.store(true, Ordering::SeqCst);
    log(&owner, 1, &[(3, -61, 4, blocks::AIR)]);
    disk.held.wait();
    log(&owner, 2, &[(3, 100, 4, blocks::GLASS)]);
    owner.request(StoreRequest::Flush);
    // A hello ends the group, and is looked at when the group's sync has failed.
    let neighbour = open_later(&store, hello(0, 2));
    disk.failing_syncs.store(true, Ordering::SeqCst);
    disk.held.wait();

    // The hello ended the group and met a log that could not be cut back for good.
    assert!(matches!(neighbour.recv().unwrap(), Err(StoreError::Io(_))));
    let replies: Vec<_> = owner.replies.iter().collect();
    assert_eq!(replies, [StoreReply::Committed { tick: 1 }]);
    assert!(owner.is_lost());
    // The bystander asked for nothing at all.
    assert_eq!(bystander.replies.iter().count(), 0);
    assert!(bystander.is_lost());
    disk.failing_syncs.store(false, Ordering::SeqCst);

    // The same for a group that a commit and a flush of two regions are in.
    let owner = open(&store, hello(1, 1));
    let other = open(&store, hello(0, 2));
    // The openings are durable by themselves here too.
    owner.flush();
    disk.holding_syncs.store(true, Ordering::SeqCst);
    log(&owner, 2, &[(3, 100, 4, blocks::GLASS)]);
    disk.held.wait();
    log(&other, 1, &[(-3, 100, 4, blocks::GLASS)]);
    other.request(StoreRequest::Flush);
    owner.request(StoreRequest::Flush);
    disk.failing_syncs.store(true, Ordering::SeqCst);
    disk.held.wait();
    let replies: Vec<_> = owner.replies.iter().collect();
    assert_eq!(replies, [StoreReply::Committed { tick: 2 }]);
    assert_eq!(other.replies.iter().count(), 0);
    assert!(owner.is_lost() && other.is_lost());
    disk.failing_syncs.store(false, Ordering::SeqCst);
    let (_, restored) = store.open_region(hello(0, 2)).unwrap();
    assert_eq!(restored.deltas, []);
    let (_, restored) = store.open_region(hello(1, 1)).unwrap();
    assert_eq!(deltas(&restored), [(1, delta(1)), (2, delta(2))]);
}

/// Says hello without waiting for the answer, which arrives on what is returned.
#[allow(clippy::type_complexity)]
pub(crate) fn open_later(
    store: &Store,
    hello: RegionHello,
) -> Receiver<Result<(Opened, Restored), StoreError>> {
    let (answer, answered) = mpsc::channel();
    let open = Message::Open {
        hello,
        reply_to: store.messages.clone(),
        answer,
    };
    store.messages.send(open).unwrap();
    answered
}

/// F1 of ADR-0011, for a record that cannot be written: every handle is lost, also
/// that of the region whose record it was not, and nothing of the group is answered.
#[test]
fn a_failed_append_loses_every_handle_and_answers_nothing_of_its_group() {
    let (store, disk) = switched();
    let owner = open(&store, hello(1, 1));
    let other = open(&store, hello(0, 1));
    // Both openings are durable by themselves, so that the sync held is the first
    // commit's.
    owner.flush();
    disk.holding_syncs.store(true, Ordering::SeqCst);
    log(&owner, 1, &[(3, -61, 4, blocks::AIR)]);
    disk.held.wait();
    // One group: a commit that cannot be written, and what the other region asks for
    // behind it.
    log(&owner, 2, &[(3, 100, 4, blocks::GLASS)]);
    log(&other, 1, &[(-3, 100, 4, blocks::GLASS)]);
    other.request(StoreRequest::Flush);
    disk.failing_appends.store(true, Ordering::SeqCst);
    disk.held.wait();

    let replies: Vec<_> = owner.replies.iter().collect();
    assert_eq!(replies, [StoreReply::Committed { tick: 1 }]);
    assert_eq!(other.replies.iter().count(), 0);
    assert!(owner.is_lost() && other.is_lost());
    disk.failing_appends.store(false, Ordering::SeqCst);
    let (_, restored) = store.open_region(hello(0, 1)).unwrap();
    assert_eq!(restored.deltas, []);
    let (_, restored) = store.open_region(hello(1, 1)).unwrap();
    assert_eq!(deltas(&restored), [(1, delta(1))]);
}

/// A store on what a crash leaves of `disk`.
fn restarted(disk: &MemoryDisk, survival: Survival) -> Store {
    let left = Arc::new(disk.crashed(survival));
    let chunks = FileChunks::new(left.clone(), Path::new("/world"));
    let root = Path::new("/world");
    start(left, root, Box::new(chunks), generator(), &division()).unwrap()
}

/// F2 of ADR-0011: once the store has welcomed a hello after a failed sync, the segment
/// is durably no longer than it was when it was last synced with success, also on a
/// machine that loses a truncation it was not made to write out.
#[test]
fn what_a_failed_sync_left_in_the_log_is_durably_gone_before_anyone_is_welcomed() {
    let (store, disk) = switched();
    let segment = Path::new("/world/log/00000000000000000001.wal");
    let owner = open(&store, hello(1, 1));
    log(&owner, 1, &[(3, -61, 4, blocks::AIR)]);
    committed(&owner, 1);
    let durable = disk.read(segment).unwrap().unwrap().len();

    disk.failing_syncs.store(true, Ordering::SeqCst);
    log(&owner, 2, &[(3, 100, 4, blocks::GLASS)]);
    owner.flush();
    assert!(owner.is_lost());
    // Cut back, but not for good: a crash now can bring the second commit back.
    assert_eq!(disk.read(segment).unwrap().unwrap().len(), durable);
    let left = disk.disk.crashed(Survival::Untruncated);
    assert!(left.read(segment).unwrap().unwrap().len() > durable);
    assert!(matches!(
        store.open_region(hello(1, 2)),
        Err(StoreError::Io(_))
    ));
    let left = disk.disk.crashed(Survival::Untruncated);
    assert!(left.read(segment).unwrap().unwrap().len() > durable);

    disk.failing_syncs.store(false, Ordering::SeqCst);
    let (_new, restored) = store.open_region(hello(1, 2)).unwrap();
    assert_eq!(deltas(&restored), [(1, delta(1))]);
    for survival in [
        Survival::Nothing,
        Survival::Torn,
        Survival::Everything,
        Survival::Untruncated,
    ] {
        let left = disk.disk.crashed(survival);
        let length = left.read(segment).unwrap().unwrap().len();
        assert_eq!(length, durable, "{survival:?}");
        // And a store that starts on it has the first commit and not the second.
        let (_, restored) = restarted(&disk.disk, survival)
            .open_region(hello(1, 3))
            .unwrap();
        assert_eq!(deltas(&restored), [(1, delta(1))], "{survival:?}");
    }
}

/// F3 of ADR-0011: while the segment cannot be cut back and synced, every hello is
/// answered with an I/O error, in this process and over a connection, which is closed
/// without a welcome; once it can, the next hello is welcomed and restored with exactly
/// what was confirmed.
#[test]
fn nobody_is_served_while_the_log_cannot_be_cut_back_for_good() {
    let (store, disk) = switched();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let server = serve(store.clone(), listener).unwrap();
    let address = server.local_addr().to_string();
    let owner = open(&store, hello(1, 1));
    let bystander = open(&store, hello(0, 1));
    log(&owner, 1, &[(3, -61, 4, blocks::AIR)]);
    committed(&owner, 1);

    disk.failing_syncs.store(true, Ordering::SeqCst);
    log(&owner, 2, &[(3, 100, 4, blocks::GLASS)]);
    owner.flush();
    assert!(owner.is_lost() && bystander.is_lost());
    // Whichever region is asked for, with whatever epoch, and however often.
    for _ in 0..2 {
        for hello in [hello(1, 1), hello(0, 9), hello(1, 2)] {
            let Err(error) = store.open_region(hello) else {
                panic!("{hello:?} was welcomed");
            };
            assert!(matches!(error, StoreError::Io(_)), "{error}");
            let Err(error) = StoreHandle::connect(&address, hello) else {
                panic!("{hello:?} was welcomed over a connection");
            };
            assert!(matches!(error, StoreError::Io(_)), "{error}");
        }
        // Nor is anyone told which regions there are, in this process or another.
        assert!(matches!(store.regions(), Err(StoreError::Io(_))));
        assert!(matches!(regions(&address), Err(StoreError::Io(_))));
    }
    // The connection ends without a word.
    let mut connection = std::net::TcpStream::connect(&address).unwrap();
    let said = clustine_rpc::StoreHello::Region(hello(1, 2));
    clustine_rpc::wire::blocking::write(&mut connection, &said).unwrap();
    let said: Option<clustine_rpc::StoreWelcome> =
        clustine_rpc::wire::blocking::read(&mut connection).unwrap();
    assert_eq!(said, None);

    // A connection that asks for the list is closed without it as well.
    let mut connection = std::net::TcpStream::connect(&address).unwrap();
    let said = clustine_rpc::StoreHello::Regions;
    clustine_rpc::wire::blocking::write(&mut connection, &said).unwrap();
    let list: Option<RegionList> = clustine_rpc::wire::blocking::read(&mut connection).unwrap();
    assert_eq!(list, None);

    disk.failing_syncs.store(false, Ordering::SeqCst);
    assert_eq!(store.regions().unwrap().regions.len(), 2);
    assert_eq!(regions(&address).unwrap(), store.regions().unwrap());
    let (remote, restored) = StoreHandle::connect(&address, hello(1, 2)).unwrap();
    assert_eq!(restored.state, None);
    assert_eq!(deltas(&restored), [(1, delta(1))]);
    log(&remote, 2, &[(3, 100, 4, blocks::GLASS)]);
    committed(&remote, 2);
    let (_, restored) = store.open_region(hello(0, 1)).unwrap();
    assert_eq!((restored.state, restored.deltas), (None, Vec::new()));
}

/// F3 of ADR-0011 with faults that end by themselves: the sync of a group fails, and
/// with it the next changes and syncs, which are those that cut the segment back. Each
/// hello that meets one is answered with an I/O error; the first that meets none is
/// welcomed and restored with exactly what was confirmed.
#[test]
fn hellos_fail_until_cutting_the_log_back_succeeds() {
    // How many changes and syncs there are up to the commit that is to fail.
    let before = {
        let disk = Arc::new(MemoryDisk::default());
        let store = store_on_memory(&disk);
        let owner = open(&store, hello(1, 1));
        // The opening is durable by itself, so that the commits are a group each and
        // the count does not depend on what the commit thread took together.
        owner.flush();
        log(&owner, 1, &[(3, -61, 4, blocks::AIR)]);
        committed(&owner, 1);
        disk.operations()
    };
    for count in 1..=5 {
        // The append of the second commit works, its sync and what follows do not.
        let disk = Arc::new(MemoryDisk::failing(Fault::Fails(before + 2, count)));
        let store = store_on_memory(&disk);
        let owner = open(&store, hello(1, 1));
        owner.flush();
        log(&owner, 1, &[(3, -61, 4, blocks::AIR)]);
        committed(&owner, 1);
        log(&owner, 2, &[(3, 100, 4, blocks::GLASS)]);
        owner.flush();
        assert!(owner.is_lost(), "{count}");

        let mut refused = 0;
        let restored = loop {
            match store.open_region(hello(1, 2)) {
                Ok((_, restored)) => break restored,
                Err(StoreError::Io(_)) => refused += 1,
                Err(error) => panic!("{count}: {error}"),
            }
            assert!(refused <= count, "{count}");
        };
        // The sync of the group and the first attempt to cut it back take two faults;
        // each further one is met by a hello.
        assert_eq!(refused, count.saturating_sub(2), "{count}");
        assert_eq!(deltas(&restored), [(1, delta(1))], "{count}");
        // And that is what is on the disk, whatever a crash keeps.
        for survival in [Survival::Nothing, Survival::Untruncated] {
            let (_, restored) = restarted(&disk, survival).open_region(hello(1, 3)).unwrap();
            assert_eq!(deltas(&restored), [(1, delta(1))], "{count}, {survival:?}");
        }
    }
}

/// A store on `disk` that keeps its chunks in memory, so that the only changes and
/// syncs of the disk are those of the commit thread.
fn store_on_memory(disk: &Arc<MemoryDisk>) -> Store {
    start(
        disk.clone(),
        Path::new("/world"),
        Box::new(chunks::MemoryChunks::default()),
        generator(),
        &division(),
    )
    .unwrap()
}

/// A checkpoint waits for the saves asked for before it, and is not put in place by an
/// owner that has been replaced in the meantime.
#[test]
fn a_checkpoint_waits_for_the_saves_before_it() {
    let (store, barrier, disk) = held_saves();
    let state = Path::new("/world/regions/1.state");
    let owner = open(&store, hello(1, 1));
    log(&owner, 1, &[(3, 100, 4, blocks::STONE)]);
    save(&owner, HELD, &edited());
    owner.request(StoreRequest::Checkpoint {
        tick: 1,
        state: b"one".to_vec(),
    });
    barrier.wait();
    // The save is under way; a later commit is answered, and the state is not there.
    log(&owner, 2, &[(3, 100, 5, blocks::STONE)]);
    committed(&owner, 2);
    assert!(!disk.exists(state).unwrap());

    barrier.wait();
    owner.flush();
    let written = clustine_format::StateFile::decode(&disk.read(state).unwrap().unwrap());
    assert_eq!(written.unwrap().state, b"one");
    let (_, restored) = store.open_region(hello(1, 1)).unwrap();
    assert_eq!(restored.state.as_ref().unwrap().tick, 1);
    assert_eq!(deltas(&restored), [(2, delta(2))]);
}

#[test]
fn a_checkpoint_of_a_replaced_owner_is_not_put_in_place() {
    let (store, barrier, _) = held_saves();
    let old = open(&store, hello(1, 1));
    log(&old, 1, &[(3, 100, 4, blocks::STONE)]);
    log(&old, 2, &[(3, 100, 5, blocks::STONE)]);
    committed(&old, 2);
    save(&old, HELD, &edited());
    old.request(StoreRequest::Checkpoint {
        tick: 2,
        state: b"old".to_vec(),
    });
    barrier.wait();
    // The new owner is restored while the old one's checkpoint waits for its save; the
    // thread for chunks hands the region over only after it, but the old checkpoint
    // does not count any more.
    let opening = thread::spawn({
        let store = store.clone();
        move || store.open_region(hello(1, 2)).unwrap()
    });
    while !old.is_lost() {
        thread::yield_now();
    }
    barrier.wait();
    let (new, restored) = opening.join().unwrap();
    assert_eq!(restored.state, None);
    assert_eq!(deltas(&restored), [(1, delta(1)), (2, delta(2))]);
    new.flush();
    drop(new);
    let (_, restored) = store.open_region(hello(1, 2)).unwrap();
    assert_eq!(restored.state, None);
    assert_eq!(deltas(&restored), [(1, delta(1)), (2, delta(2))]);
}
