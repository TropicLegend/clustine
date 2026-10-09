//! Tests of the store at rest: [`Store::flush`], the barrier that goes through the
//! thread for chunks and back. See section 5.5 of
//! `docs/adr/0017-the-end-of-the-stripes.md`.
//!
//! A runner that is stopped in the middle of a release, a merge or a split lets go of
//! its handle while the store still works for it. These tests ask as such a runner
//! does, without waiting for an answer and with the handle dropped at once, and look
//! at what a crash would leave at the moment `flush` returns, and at whether the store
//! writes anything after it.

use std::path::Path;
use std::sync::Barrier;
use std::sync::atomic::Ordering;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use clustine_data::blocks;
use clustine_format::RegionFile;
use clustine_rpc::{ChunkBox, RegionInfo, RegionList, SplitPart};
use clustine_world::{Chunk, ChunkPos};

use super::*;
use crate::disk::{Fault, MemoryDisk, Survival};
use crate::regions::{
    SURVIVALS, claim, gap, give_back, hello_of, listed, state_of, store_on, whole,
};
use crate::tests::{HELD, Switched, committed, generator, load, log, save, switched_for};
use crate::unpinned::Built;

/// Generates what [`generator`] does, but stops before the chunk at [`HELD`] until it
/// is let go on, which keeps the thread for chunks busy while what it is given queues
/// up. And whoever has the other end of `_alive` learns when it is dropped: the store
/// has the only one, and the thread for chunks lets go of it when it ends, which it
/// does after the commit thread has.
struct Watched {
    held: Arc<Barrier>,
    _alive: Sender<()>,
}

impl ChunkGenerator for Watched {
    fn generate(&self, position: ChunkPos) -> Chunk {
        if position == HELD {
            // Once to say that the store has got here, once to be let go on.
            self.held.wait();
            self.held.wait();
        }
        generator().generate(position)
    }

    fn settings(&self) -> String {
        generator().settings()
    }
}

/// A store with a generator that can be held, on a disk that tells what a crash would
/// leave of it and counts what is done to it.
pub(crate) struct Rig {
    pub(crate) store: Store,
    pub(crate) disk: Arc<MemoryDisk>,
    /// Waited on by the test once the generator is asked for the chunk at [`HELD`], to
    /// know that the thread for chunks is busy, and once more to let it go on.
    pub(crate) held: Arc<Barrier>,
    /// Ends when both threads of the store have.
    pub(crate) ended: Receiver<()>,
}

impl Rig {
    pub(crate) fn new(division: &Division) -> Rig {
        let disk = Arc::new(MemoryDisk::default());
        Self::on(disk, division).expect("a new world on a disk that works")
    }

    pub(crate) fn on(disk: Arc<MemoryDisk>, division: &Division) -> Result<Rig, StoreError> {
        let held = Arc::new(Barrier::new(2));
        let (alive, ended) = mpsc::channel();
        let watched = Watched {
            held: Arc::clone(&held),
            _alive: alive,
        };
        let root = Path::new("/world");
        let chunks = FileChunks::new(disk.clone(), root);
        let generator = Arc::new(watched);
        let store = start(disk.clone(), root, Box::new(chunks), generator, division)?;
        Ok(Rig {
            store,
            disk,
            held,
            ended,
        })
    }

    /// Lets go of the store and waits until both of its threads have ended, which
    /// they do when no handle is left either. Returns how many times the disk was
    /// changed or synced by then.
    pub(crate) fn ended(self) -> u64 {
        let Rig {
            store, disk, ended, ..
        } = self;
        drop(store);
        // Bounded only so that a handle a test forgot is a failure and not a test
        // that never ends.
        match ended.recv_timeout(Duration::from_secs(60)) {
            Err(RecvTimeoutError::Disconnected) => disk.operations(),
            other => panic!("the threads of the store have not ended: {other:?}"),
        }
    }
}

const ORIGIN: ChunkPos = ChunkPos::new(0, 0);

/// Two regions side by side: 0 west of x = 0, and 1 east of it, which holds the origin
/// and the chunk at [`HELD`].
fn two() -> Division {
    Division::side_by_side(ORIGIN, &[0]).expect("one cut")
}

/// Chunks of the eastern region, and one of the western.
const NEAR: ChunkPos = ChunkPos::new(3, 3);
const FAR: ChunkPos = ChunkPos::new(40, -2);
const WEST: ChunkPos = ChunkPos::new(-4, 1);

/// What a store that starts on what a crash leaves of `disk` says of the world: the
/// list without its epochs, and what each region is restored with when it is opened
/// with `epoch`, in the order of the list.
pub(crate) fn left_by(
    disk: &MemoryDisk,
    survival: Survival,
    division: &Division,
    epoch: u64,
) -> (RegionList, Vec<Restored>) {
    let left = Arc::new(disk.crashed(survival));
    let store = store_on(&left, division).unwrap_or_else(|error| panic!("{survival:?}: {error}"));
    let list = store.regions().unwrap();
    let restored = list.regions.iter().map(|info| {
        let (_, restored) = store
            .open_region(hello_of(division, info.region.0, epoch))
            .unwrap_or_else(|error| panic!("{survival:?}: region {}: {error}", info.region));
        restored
    });
    let restored = restored.collect();
    (listed(&list), restored)
}

fn ticks(restored: &Restored) -> Vec<u64> {
    restored.deltas.iter().map(|delta| delta.tick).collect()
}

/// A checkpoint that waits behind a thread for chunks that is busy, asked through a
/// handle that is gone, as a runner that is abandoned in the middle of a release
/// leaves it. The barrier is not answered by the commit thread alone; and when it is
/// answered, the state of the checkpoint is in place for good.
#[test]
fn a_checkpoint_behind_a_busy_thread_for_chunks_is_waited_for() {
    let rig = Rig::new(&two());
    let (east, _) = rig.store.open_region(hello_of(&two(), 1, 1)).unwrap();
    let mut built = Built::default();
    for tick in 1..=3 {
        let block = (tick as usize, 100, 1);
        log(&east, tick, &[built.set(NEAR, block, blocks::STONE)]);
    }
    // Without waiting for any answer: a load that holds the thread for chunks, and
    // behind it the save and the checkpoint.
    east.request(StoreRequest::Load { position: HELD });
    rig.held.wait();
    save(&east, NEAR, &built.chunk(NEAR));
    east.request(StoreRequest::Checkpoint {
        tick: 3,
        state: whole("state", 3),
    });
    drop(east);

    let barrier = rig.store.barrier();
    // The list is asked for behind the barrier, so the commit thread has had the
    // barrier's turn when the list is here; and it has nothing more to do until the
    // thread for chunks goes on, so the disk is as a crash would find it now.
    rig.store.regions().unwrap();
    assert!(barrier.try_recv().is_err());
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let (_, restored) = left_by(&rig.disk, survival, &two(), epoch);
        assert_eq!(state_of(&restored[1]), None, "{survival:?}");
        assert_eq!(ticks(&restored[1]), [1, 2, 3], "{survival:?}");
    }

    rig.held.wait();
    Store::rested(&barrier).unwrap();
    let done = rig.disk.operations();
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let case = format!("{survival:?}");
        let left = Arc::new(rig.disk.crashed(survival));
        let store = store_on(&left, &two()).unwrap();
        let (east, restored) = store.open_region(hello_of(&two(), 1, epoch)).unwrap();
        assert_eq!(state_of(&restored), Some((3, whole("state", 3))), "{case}");
        assert_eq!(ticks(&restored), [], "{case}");
        // And what the region saved before it, which a checkpoint makes durable.
        assert_eq!(load(&east, NEAR), built.chunk(NEAR), "{case}");
    }
    // Nothing is written when the store is let go of.
    assert_eq!(rig.ended(), done);
}

/// A return goes the same way, and is in the log only when the commit thread has
/// heard from the thread for chunks: the chunk is free when the barrier is answered,
/// and for good.
#[test]
fn a_return_behind_a_busy_thread_for_chunks_is_waited_for() {
    let rig = Rig::new(&gap());
    let free = ChunkPos::new(5, 5);
    let (west, _) = rig.store.open_region(hello_of(&gap(), 0, 1)).unwrap();
    let (east, _) = rig.store.open_region(hello_of(&gap(), 1, 1)).unwrap();
    assert_eq!(claim(&west, &[free]).0, [free]);

    east.request(StoreRequest::Load { position: HELD });
    rig.held.wait();
    let mut built = Built::default();
    log(&west, 1, &[built.set(free, (7, 100, 7), blocks::GLASS)]);
    save(&west, free, &built.chunk(free));
    give_back(&west, &[free]);
    drop((west, east));

    let barrier = rig.store.barrier();
    let granted = Some(ChunkBox {
        min: free,
        max: free,
    });
    assert_eq!(rig.store.regions().unwrap().regions[0].bounds, granted);
    assert!(barrier.try_recv().is_err());
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let (list, _) = left_by(&rig.disk, survival, &gap(), epoch);
        assert_eq!(list.regions[0].bounds, granted, "{survival:?}");
    }

    rig.held.wait();
    Store::rested(&barrier).unwrap();
    let done = rig.disk.operations();
    assert_eq!(rig.store.regions().unwrap().regions[0].bounds, None);
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let case = format!("{survival:?}");
        let left = Arc::new(rig.disk.crashed(survival));
        let store = store_on(&left, &gap()).unwrap();
        assert_eq!(store.regions().unwrap().regions[0].bounds, None, "{case}");
        // Whoever claims it next finds what was built in it.
        let (home, _) = store.open_region(hello_of(&gap(), 2, epoch)).unwrap();
        assert_eq!(claim(&home, &[free]).0, [free], "{case}");
        assert_eq!(load(&home, free), built.chunk(free), "{case}");
    }
    assert_eq!(rig.ended(), done);
}

/// A split is done whole on the commit thread. Handed to the store through a handle
/// that is dropped at once, as by a runner that is stopped while its split is being
/// written, it has happened when `flush` returns, with the file of the new region.
#[test]
fn a_split_through_a_handle_that_is_dropped_at_once_is_done_when_the_store_is_at_rest() {
    let rig = Rig::new(&two());
    let (east, _) = rig.store.open_region(hello_of(&two(), 1, 1)).unwrap();
    east.request(StoreRequest::SplitCommit {
        tick: 1,
        state: whole("rest", 1),
        part: SplitPart {
            chunks: vec![NEAR, FAR],
            state: whole("part", 1),
        },
        as_epoch: 5,
        region: RegionId(2),
    });
    drop(east);
    rig.store.flush().unwrap();

    let done = rig.disk.operations();
    let part = RegionInfo {
        region: RegionId(2),
        epoch: 0,
        bounds: Some(ChunkBox {
            min: ChunkPos::new(NEAR.x, FAR.z),
            max: ChunkPos::new(FAR.x, NEAR.z),
        }),
        pinned: Vec::new(),
    };
    for survival in SURVIVALS {
        let case = format!("{survival:?}");
        let (list, restored) = left_by(&rig.disk, survival, &two(), 5);
        assert_eq!((list.regions.len(), list.next), (3, RegionId(3)), "{case}");
        assert_eq!(list.regions[2], part, "{case}");
        assert_eq!(restored[2].held, [(NEAR, 1), (FAR, 1)], "{case}");
        assert_eq!(state_of(&restored[2]), Some((1, whole("part", 1))));
        assert_eq!(state_of(&restored[1]), Some((1, whole("rest", 1))));
        // The file of the new region is there, with the epoch it was made with.
        let file = rig.disk.crashed(survival);
        let file = file.read(Path::new("/world/regions/2.region")).unwrap();
        let file = RegionFile::decode(&file.expect(&case)).unwrap();
        assert_eq!(file.epoch, 5, "{case}");
    }
    assert_eq!(rig.ended(), done);
}

/// So is a merge, of which both runners may be stopped: the survivor's and that of the
/// region it absorbs.
#[test]
fn a_merge_through_handles_that_are_dropped_at_once_is_done_when_the_store_is_at_rest() {
    let rig = Rig::new(&two());
    let (west, _) = rig.store.open_region(hello_of(&two(), 0, 1)).unwrap();
    let (east, _) = rig.store.open_region(hello_of(&two(), 1, 4)).unwrap();
    // The eastern region is home, which is never absorbed: it is the one that
    // survives.
    east.request(StoreRequest::AbsorbCommit {
        absorbed: RegionId(0),
        absorbed_epoch: 1,
        tick: 1,
        state: whole("merged", 1),
    });
    drop((east, west));
    rig.store.flush().unwrap();

    let done = rig.disk.operations();
    for survival in SURVIVALS {
        let case = format!("{survival:?}");
        let (list, restored) = left_by(&rig.disk, survival, &two(), 5);
        assert_eq!(list.absorbed, [(RegionId(0), RegionId(1))], "{case}");
        assert_eq!(list.regions.len(), 1, "{case}");
        assert_eq!(restored[0].pinned.len(), 2, "{case}");
        assert_eq!(state_of(&restored[0]), Some((1, whole("merged", 1))));
    }
    assert_eq!(rig.ended(), done);
}

/// A store whose two threads can both be held: on a disk that holds the commit thread
/// in a sync of the log, and with the generator that holds the thread for chunks.
/// Returns the store, the disk, and what the generator waits on.
pub(crate) fn held_twice(division: &Division) -> (Store, Arc<Switched>, Arc<Barrier>) {
    let disk = Arc::new(Switched::default());
    let held = Arc::new(Barrier::new(2));
    // Nobody waits for these threads to end.
    let (alive, _) = mpsc::channel();
    let watched = Watched {
        held: Arc::clone(&held),
        _alive: alive,
    };
    let root = Path::new("/world");
    let chunks = FileChunks::new(disk.clone(), root);
    let generator = Arc::new(watched);
    let store = start(disk.clone(), root, Box::new(chunks), generator, division).unwrap();
    (store, disk, held)
}

/// What a handle asked for behind a commit waits on the commit thread until the
/// commit is durable. If the barrier comes in the same group, it goes to the thread
/// for chunks behind what waited, and not ahead of it.
#[test]
fn what_waits_for_the_group_of_the_barrier_is_passed_on_before_it() {
    let (store, disk, held) = held_twice(&two());
    let east = store.open_region(hello_of(&two(), 1, 1)).unwrap().0;
    // So that the sync that is held is the one meant here; see `Switched`.
    east.flush();

    disk.holding_syncs.store(true, Ordering::SeqCst);
    log(&east, 1, &[]);
    disk.held.wait();
    // A commit, a save that waits for it, a load that waits behind the save, and the
    // barrier: all of them are there when the commit thread goes on.
    log(&east, 2, &[]);
    save(&east, NEAR, &generator().generate(NEAR));
    east.request(StoreRequest::Load { position: HELD });
    let barrier = store.barrier();
    disk.held.wait();

    // The load has got to the thread for chunks, which is busy with it; and the
    // barrier is behind it, as the list, which is asked for now, is behind whatever
    // that thread had said before.
    held.wait();
    store.regions().unwrap();
    assert!(barrier.try_recv().is_err());
    held.wait();
    Store::rested(&barrier).unwrap();
    let answers = [
        StoreReply::Committed { tick: 1 },
        StoreReply::Committed { tick: 2 },
        StoreReply::Loaded {
            position: HELD,
            chunk: generator().generate(HELD),
        },
    ];
    for answer in answers {
        assert_eq!(east.try_reply(), Some(answer));
    }
    assert_eq!(east.try_reply(), None);
}

/// What the thread for chunks says of the jobs before the barrier is acted on by the
/// commit thread, and the barrier is answered only when that is durable too: here the
/// record of a return, whose sync is held.
#[test]
fn the_barrier_is_answered_only_when_what_came_back_with_it_is_durable() {
    let (store, disk, held) = held_twice(&gap());
    let free = ChunkPos::new(5, 5);
    let west = store.open_region(hello_of(&gap(), 0, 1)).unwrap().0;
    let east = store.open_region(hello_of(&gap(), 1, 1)).unwrap().0;
    assert_eq!(claim(&west, &[free]).0, [free]);
    west.flush();
    east.flush();
    let granted = Some(ChunkBox {
        min: free,
        max: free,
    });
    let bounds_left = |survival: Survival| {
        let left = Arc::new(disk.disk.crashed(survival));
        let store = store_on(&left, &gap()).unwrap();
        store.regions().unwrap().regions[0].bounds
    };

    // The commit thread is held three times in a sync of the log, each time armed
    // while it is held in the one before, so that what happens between them is known.
    let hold_the_next = || disk.holding_syncs.store(true, Ordering::SeqCst);
    let entered = || disk.held.wait();
    let let_go = || disk.held.wait();

    // The thread for chunks is held twice, by a load before the return and by one
    // behind the barrier, so that it says nothing to the commit thread before that is
    // held, and has said everything when it is let go.
    let busy = || held.wait();
    let go_on = || held.wait();

    // First in the sync of a commit, while these queue up as one group: a load, the
    // return, the barrier, a commit and one more load.
    hold_the_next();
    log(&east, 1, &[]);
    entered();
    east.request(StoreRequest::Load { position: HELD });
    give_back(&west, &[free]);
    let barrier = store.barrier();
    log(&east, 2, &[]);
    east.request(StoreRequest::Load { position: HELD });
    hold_the_next();
    let_go();

    // Then in the sync of that group, with all of it passed on to the thread for
    // chunks, which is busy with the first load. Let go on, that thread says that the
    // return is through and that the barrier has passed, and is busy with the second
    // load: both messages are there for the commit thread, and nothing else comes.
    busy();
    entered();
    go_on();
    busy();
    hold_the_next();
    let_go();

    // And last in the sync of the group it takes them in, which has the record of
    // the return: the barrier is not answered, and a crash would take the return
    // back.
    entered();
    assert!(barrier.try_recv().is_err());
    assert_eq!(bounds_left(Survival::Nothing), granted);
    let_go();
    Store::rested(&barrier).unwrap();
    for survival in SURVIVALS {
        assert_eq!(bounds_left(survival), None, "{survival:?}");
    }
    go_on();
}

/// Every answer the store owes for what was asked before is in its handle's queue when
/// `flush` returns, in the order of the requests; and nothing is closed by it: a
/// handle that lives is served on, and the store can be waited for again, through any
/// clone of it.
#[test]
fn the_answers_are_out_and_handles_and_clones_are_served_on() {
    let rig = Rig::new(&gap());
    let free = ChunkPos::new(5, 5);
    let bystander = rig.store.open_region(hello_of(&gap(), 1, 1)).unwrap().0;
    let (west, _) = rig.store.open_region(hello_of(&gap(), 0, 1)).unwrap();
    log(&west, 1, &[]);
    west.request(StoreRequest::Claim { chunks: vec![free] });
    west.request(StoreRequest::Flush);
    rig.store.flush().unwrap();
    // Nothing more is waited for: they are there.
    let claimed = StoreReply::Claimed {
        granted: vec![free],
        foreign: Vec::new(),
    };
    let answers = [
        StoreReply::Committed { tick: 1 },
        claimed,
        StoreReply::Flushed,
    ];
    for answer in answers {
        assert_eq!(west.try_reply(), Some(answer));
    }
    assert_eq!(west.try_reply(), None);
    assert!(!west.is_lost() && !bystander.is_lost());

    // The handles are served on.
    log(&bystander, 1, &[]);
    committed(&bystander, 1);
    bystander.flush();
    log(&west, 2, &[]);
    committed(&west, 2);
    // Again, and through a clone, and from several threads at once while a handle
    // goes on asking.
    rig.store.flush().unwrap();
    let clone = rig.store.clone();
    clone.flush().unwrap();
    let waiting: Vec<_> = (0..4)
        .map(|_| {
            let store = rig.store.clone();
            thread::spawn(move || (0..20).try_for_each(|_| store.flush()))
        })
        .collect();
    for tick in 2..=40 {
        log(&bystander, tick, &[]);
        committed(&bystander, tick);
    }
    for thread in waiting {
        thread.join().unwrap().unwrap();
    }
    drop(clone);

    // And a store that has had nothing to do since is at rest without writing.
    bystander.flush();
    west.flush();
    let before = rig.disk.operations();
    rig.store.flush().unwrap();
    assert_eq!(rig.disk.operations(), before);
    drop((bystander, west));
    assert_eq!(rig.ended(), before);
}

/// After a group that could not be made durable every handle is lost, and nothing of
/// the group is answered or done: the store is at rest all the same. For as long as
/// what was cut off the log is not durably gone, `flush` says so, as the list does.
#[test]
fn a_store_whose_log_cannot_be_synced_is_at_rest_and_says_so() {
    let (store, disk) = switched_for(&two());
    let owner = store.open_region(hello_of(&two(), 1, 1)).unwrap().0;
    let bystander = store.open_region(hello_of(&two(), 0, 1)).unwrap().0;
    log(&owner, 1, &[]);
    committed(&owner, 1);
    owner.flush();
    bystander.flush();

    disk.failing_syncs.store(true, Ordering::SeqCst);
    log(&owner, 2, &[(3, 100, 4, blocks::STONE)]);
    owner.request(StoreRequest::Checkpoint {
        tick: 2,
        state: whole("state", 2),
    });
    // Neither the sync of the group nor that of the log as it is cut back succeeds.
    assert!(matches!(store.flush(), Err(StoreError::Io(_))));
    assert!(owner.is_lost() && bystander.is_lost());
    assert!(matches!(store.regions(), Err(StoreError::Io(_))));
    assert!(matches!(store.flush(), Err(StoreError::Io(_))));
    // It has returned each time, and does with the handles gone.
    drop((owner, bystander));
    assert!(matches!(store.flush(), Err(StoreError::Io(_))));

    // Once the disk works again, the store finds its rest, and the commit that was
    // never answered is in nothing a crash would leave.
    disk.failing_syncs.store(false, Ordering::SeqCst);
    store.flush().unwrap();
    store.regions().unwrap();
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let (_, restored) = left_by(&disk.disk, survival, &two(), epoch);
        assert_eq!(state_of(&restored[1]), None, "{survival:?}");
        assert_eq!(ticks(&restored[1]), [1], "{survival:?}");
    }
}

/// What runners that are stopped in the middle leave to the store, all at once and
/// with no answer waited for: a split, commits, a save and a checkpoint of one region,
/// and a claim, a commit and a return of another.
fn abandoned(store: &Store) {
    let free = ChunkPos::new(5, 5);
    if let Ok((west, _)) = store.open_region(hello_of(&gap(), 0, 1)) {
        west.request(StoreRequest::SplitCommit {
            tick: 1,
            state: whole("rest", 1),
            part: SplitPart {
                chunks: vec![WEST],
                state: whole("part", 1),
            },
            as_epoch: 1,
            region: RegionId(3),
        });
        let mut built = Built::default();
        for tick in 2..=4 {
            let block = (tick as usize, 100, 1);
            let position = ChunkPos::new(-9, 0);
            log(&west, tick, &[built.set(position, block, blocks::STONE)]);
            save(&west, position, &built.chunk(position));
        }
        west.request(StoreRequest::Checkpoint {
            tick: 4,
            state: whole("state", 4),
        });
    }
    if let Ok((home, _)) = store.open_region(hello_of(&gap(), 2, 1)) {
        home.request(StoreRequest::Claim { chunks: vec![free] });
        log(&home, 1, &[]);
        give_back(&home, &[free]);
        home.request(StoreRequest::Flush);
    }
}

/// On a disk that fails at any point, once, twice in a row or for good: `flush`
/// returns, with an error only as the list has one; nothing is written once it has
/// returned and the store is let go of; and a store starts on what any crash would
/// leave then.
#[test]
fn a_store_on_a_disk_that_fails_at_any_point_comes_to_rest() {
    let operations = {
        let rig = Rig::new(&gap());
        abandoned(&rig.store);
        rig.store.flush().unwrap();
        // Without a fault all of it is done.
        let list = rig.store.regions().unwrap();
        assert_eq!((list.regions.len(), list.next), (4, RegionId(4)));
        let done = rig.disk.operations();
        for (epoch, survival) in (2..).zip(SURVIVALS) {
            let (list, restored) = left_by(&rig.disk, survival, &gap(), epoch);
            assert_eq!(list.regions[3].region, RegionId(3), "{survival:?}");
            assert_eq!(
                list.regions[2].bounds.map(|bounds| bounds.max),
                Some(ORIGIN)
            );
            assert_eq!(state_of(&restored[0]), Some((4, whole("state", 4))));
            assert_eq!(ticks(&restored[0]), [], "{survival:?}");
        }
        assert_eq!(rig.ended(), done);
        done
    };
    for n in 1..=operations + 1 {
        for fault in [Fault::Stop(n), Fault::Fail(n), Fault::Fails(n, 2)] {
            let case = format!("{fault:?}");
            let disk = Arc::new(MemoryDisk::failing(fault));
            // A store that cannot make its world has nothing to come to rest with.
            let Ok(rig) = Rig::on(disk.clone(), &gap()) else {
                continue;
            };
            abandoned(&rig.store);
            match rig.store.flush() {
                // The list is answered as the barrier is, when nothing was asked
                // between them.
                Ok(()) => assert!(rig.store.regions().is_ok(), "{case}"),
                Err(error) => assert!(matches!(error, StoreError::Io(_)), "{case}: {error}"),
            }
            let done = rig.disk.operations();
            assert_eq!(rig.ended(), done, "{case}");
            for survival in SURVIVALS {
                let left = Arc::new(disk.crashed(survival));
                let started = store_on(&left, &gap()).and_then(|store| store.regions());
                assert!(started.is_ok(), "{case}, {survival:?}: {:?}", started.err());
            }
        }
    }
}
