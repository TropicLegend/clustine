//! The store's scenarios of the end of the stripes: T1 to T5, T7, T9 and T10 of section
//! 9.3 of `docs/adr/0017-the-end-of-the-stripes.md`, written from that record by
//! someone who did not write what they test. T6 is the command line's and is not
//! here. T8 is at the end, and was written with the step that took the layout out of a
//! hello, by its builder.
//!
//! Every test names its scenario. Where the record leaves something open, the comment
//! at the test says how it was read.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::TryRecvError;
use std::time::Duration;

use clustine_data::{BlockState, blocks};
use clustine_format::{RegionFile, TableFile, TableRegion};
use clustine_rpc::{ChunkBox, RegionInfo, RegionList, SplitPart, TickState};
use clustine_world::{Chunk, ChunkArea, ChunkPos, EntityId, EntityIds};
use tracing::Level;

use super::*;
use crate::disk::{Fault, MemoryDisk, Survival};
use crate::regions::{
    SURVIVALS, absorb, give_back, hello_of, put, split, store_on, whole, world_of_today,
};
use crate::rest::{Rig, left_by};
use crate::tests::{HELD, Switched, delta, generator, log, switched_for};
use crate::unpinned::{Built, NO_ENTITY_IDS, logged};

/// Where players enter the worlds of these tests: east of [`CUT`] and away from the
/// origin, so that the home region of a world that is cut there is not region 0.
const HOME: ChunkPos = ChunkPos::new(6, -3);

const ORIGIN: ChunkPos = ChunkPos::new(0, 0);

/// The chunk x coordinate the worlds of two stripes, and of two pinned regions, are
/// cut at.
const CUT: i32 = 4;

const TABLE: &str = "/world/regions/table";

const LAYOUT: &str = "/world/layout";

/// The first line of section 2.2, as the record has it.
const MADE_OVER: &str =
    "the world was divided otherwise before; what its regions had is in the stored chunks now";

/// The second, which the record wants at the level of a warning.
const BEGIN_ANEW: &str = "the regions of this world begin anew: whoever is in it has to join again. Stop the workers and the edges of a cluster before its world store is started with other pins";

/// How long a test waits for an answer the record promises. Nothing here waits for
/// time to pass: this only makes an answer that never comes a failure, and not a test
/// that never ends.
const PATIENCE: Duration = Duration::from_secs(60);

/// A world that is one home region, pinned to nothing.
fn following() -> Division {
    Division::open(HOME)
}

/// Regions pinned side by side, cut at these chunk x coordinates.
fn pinned_at(cuts: &[i32]) -> Division {
    Division::side_by_side(HOME, cuts).expect("cuts that ascend")
}

/// The two stripes of a world cut at [`CUT`], as a store was told before there were
/// pins: the regions [`pinned_at`] pins there, which is all that was ever kept of them.
fn stripes() -> Division {
    pinned_at(&[CUT])
}

fn area(min_x: Option<i32>, max_x: Option<i32>) -> ChunkArea {
    ChunkArea { min_x, max_x }
}

fn has_ids(ids: EntityIds) -> bool {
    ids.first.0 < ids.end.0
}

fn ticks(restored: &Restored) -> Vec<u64> {
    restored.deltas.iter().map(|delta| delta.tick).collect()
}

/// The smallest box around `chunks`.
fn around(chunks: &[ChunkPos]) -> Option<ChunkBox> {
    let xs = || chunks.iter().map(|chunk| chunk.x);
    let zs = || chunks.iter().map(|chunk| chunk.z);
    Some(ChunkBox {
        min: ChunkPos::new(xs().min()?, zs().min()?),
        max: ChunkPos::new(xs().max()?, zs().max()?),
    })
}

/// The list of a world that has the regions of `told` and nothing else: one home
/// region that holds the home chunk if `told` pins nothing, and else a region for
/// each area, of which the one with the home chunk is home. `epochs` are the highest
/// epochs of the regions by id, 0 for a region that has none.
fn begun(told: &Division, epochs: &[u64], next: u32) -> RegionList {
    let epoch = |id: usize| epochs.get(id).copied().unwrap_or(0);
    if told.pinned.is_empty() {
        let home = RegionInfo {
            region: RegionId(0),
            epoch: epoch(0),
            bounds: around(&[told.home]),
            pinned: Vec::new(),
        };
        return RegionList {
            home: RegionId(0),
            regions: vec![home],
            absorbed: Vec::new(),
            next: RegionId(next),
        };
    }
    let home = told.pinned.iter().position(|area| area.contains(told.home));
    let home = home.expect("regions side by side cover the world");
    let region = |(id, area): (usize, &ChunkArea)| RegionInfo {
        region: RegionId(id as u32),
        epoch: epoch(id),
        bounds: None,
        pinned: vec![*area],
    };
    RegionList {
        home: RegionId(home as u32),
        regions: told.pinned.iter().enumerate().map(region).collect(),
        absorbed: Vec::new(),
        next: RegionId(next),
    }
}

/// The table file of the world on `disk`, if it has one.
fn table_of(disk: &MemoryDisk) -> Option<TableFile> {
    let bytes = disk.read(Path::new(TABLE)).expect("a disk in memory");
    bytes.map(|bytes| TableFile::decode(&bytes).expect("a table file that is in place is whole"))
}

/// Whether the table was made from `told`: the same areas and the same home chunk.
fn is_of(table: &TableFile, told: &Division) -> bool {
    table.division == told.pinned && table.home_chunk == told.home
}

fn file_at(disk: &MemoryDisk, path: &str) -> Option<Vec<u8>> {
    disk.read(Path::new(path)).expect("a disk in memory")
}

/// Whether `lines` say that a world was made over: both lines of section 2.2, once
/// each and the second as a warning. Anything between that and neither line fails.
fn says(lines: &[(Level, String)], case: &str) -> bool {
    let said = |text: &str| lines.iter().filter(|(_, line)| line == text).count();
    let counted = (said(MADE_OVER), said(BEGIN_ANEW));
    assert!(counted == (0, 0) || counted == (1, 1), "{case}: {lines:?}");
    let warned = |(level, line): &&(Level, String)| *level == Level::WARN && line == BEGIN_ANEW;
    assert_eq!(
        lines.iter().filter(warned).count(),
        counted.1,
        "{case}: {lines:?}"
    );
    counted == (1, 1)
}

/// The next answer to `handle` other than that a commit is done.
fn next(handle: &StoreHandle, case: &str) -> StoreReply {
    loop {
        match handle.replies.recv_timeout(PATIENCE) {
            Ok(StoreReply::Committed { .. }) => {}
            Ok(reply) => return reply,
            Err(error) => panic!("{case}: the store did not answer: {error}"),
        }
    }
}

/// Waits until the commit of `tick` is confirmed; answers before it are passed over.
fn confirmed(handle: &StoreHandle, tick: u64, case: &str) {
    loop {
        match handle.replies.recv_timeout(PATIENCE) {
            Ok(StoreReply::Committed { tick: done }) if done == tick => return,
            Ok(_) => {}
            Err(error) => panic!("{case}: the commit of tick {tick} is not confirmed: {error}"),
        }
    }
}

/// Claims `chunks` and waits for the answer: those granted, in ascending order, and
/// those of other regions.
fn claimed(
    handle: &StoreHandle,
    chunks: &[ChunkPos],
    case: &str,
) -> (Vec<ChunkPos>, Vec<(ChunkPos, RegionId)>) {
    handle.request(StoreRequest::Claim {
        chunks: chunks.to_vec(),
    });
    match next(handle, case) {
        StoreReply::Claimed {
            mut granted,
            foreign,
        } => {
            granted.sort();
            (granted, foreign)
        }
        other => panic!("{case}: expected the answer to a claim, got {other:?}"),
    }
}

/// Saves the chunk at `position` as `built` has it now, as of `tick`.
fn save(handle: &StoreHandle, built: &Built, position: ChunkPos, tick: u64) {
    handle.request(StoreRequest::Save {
        position,
        tick,
        chunk: built.chunk(position),
    });
}

/// The chunk at `position` is `expected`, as the region of `handle` loads it.
fn loads(handle: &StoreHandle, position: ChunkPos, expected: &Chunk, case: &str) {
    handle.request(StoreRequest::Load { position });
    match next(handle, case) {
        StoreReply::Loaded {
            position: loaded,
            chunk,
        } => {
            assert_eq!(loaded, position, "{case}");
            // Not `assert_eq`, which would print both chunks whole.
            assert!(
                chunk == *expected,
                "{case}: the chunk at {position:?} is not as it was built"
            );
        }
        other => panic!("{case}: the chunk at {position:?} is not loaded: {other:?}"),
    }
}

/// The chunk at `position` is as `built` has it, as the region of `handle` loads it.
fn as_built(handle: &StoreHandle, built: &Built, position: ChunkPos, case: &str) {
    loads(handle, position, &built.chunk(position), case);
}

/// Every chunk something was built in, as `built` has it. Made once for a world: the
/// tests that kill a start look at each of them thousands of times.
fn chunks_of(built: &Built) -> Vec<(ChunkPos, Chunk)> {
    let chunk = |position| (position, built.chunk(position));
    built.positions().into_iter().map(chunk).collect()
}

/// `Store::flush`, on a thread of its own: it has to return, and a store that never
/// comes to rest is to fail a test and not to hold it for ever.
fn at_rest(store: &Store) -> Result<(), StoreError> {
    let (done, waited) = mpsc::channel();
    let store = store.clone();
    thread::spawn(move || {
        let rested = store.flush();
        // Gone before the test goes on, so that this clone is not what keeps the
        // store's threads.
        drop(store);
        let _ = done.send(rested);
    });
    waited
        .recv_timeout(PATIENCE)
        .expect("`Store::flush` returns")
}

/// A handle's own `flush`, which has to return as well.
fn flushed(handle: StoreHandle) -> StoreHandle {
    let (done, waited) = mpsc::channel();
    thread::spawn(move || {
        handle.flush();
        let _ = done.send(handle);
    });
    waited
        .recv_timeout(PATIENCE)
        .expect("the handle's `flush` returns")
}

/// A disk in memory that notes every file it is told to change, so that a test can
/// say what a start wrote. Syncs change no file and are not noted, and neither is
/// removing a file that is not there or cutting one to the length it has.
struct Noting {
    disk: MemoryDisk,
    changed: Mutex<Vec<PathBuf>>,
}

impl Noting {
    fn of(disk: MemoryDisk) -> Arc<Self> {
        Arc::new(Self {
            disk,
            changed: Mutex::default(),
        })
    }

    fn note(&self, path: &Path) {
        let mut changed = self.changed.lock().expect("nothing panics with the names");
        changed.push(path.to_owned());
    }

    fn changed(&self) -> Vec<PathBuf> {
        let changed = self.changed.lock().expect("nothing panics with the names");
        changed.clone()
    }

    /// A store on this disk for a world divided as `told` says, as
    /// [`store_on`] starts one.
    fn started(self: &Arc<Self>, told: &Division) -> Result<Store, StoreError> {
        let root = Path::new("/world");
        let chunks = FileChunks::new(self.clone(), root);
        start(self.clone(), root, Box::new(chunks), generator(), told)
    }
}

impl Disk for Noting {
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
        self.note(path);
        self.disk.write(path, contents)
    }
    fn append(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        self.note(path);
        self.disk.append(path, contents)
    }
    fn truncate(&self, path: &Path, length: u64) -> io::Result<()> {
        let had = self.disk.read(path)?.map(|contents| contents.len() as u64);
        if had != Some(length) {
            self.note(path);
        }
        self.disk.truncate(path, length)
    }
    fn sync(&self, path: &Path) -> io::Result<()> {
        self.disk.sync(path)
    }
    fn sync_directory(&self, directory: &Path) -> io::Result<()> {
        self.disk.sync_directory(directory)
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.note(from);
        self.note(to);
        self.disk.rename(from, to)
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        if self.disk.exists(path)? {
            self.note(path);
        }
        self.disk.remove(path)
    }
}

// Section 2.1: the two constructors, which the stores of T1, T4 and T7 are started
// with. T6 is the same lists on the command line, and is not here.
#[test]
fn a_division_is_open_or_side_by_side_at_cuts_that_ascend() {
    let open = Division::open(HOME);
    assert_eq!((open.home, open.pinned), (HOME, Vec::new()));

    let cut_at = |cuts: &[i32]| Division::side_by_side(HOME, cuts).map(|told| told.pinned);
    assert_eq!(
        cut_at(&[4]),
        Ok(vec![area(None, Some(4)), area(Some(4), None)])
    );
    assert_eq!(
        cut_at(&[0, 4]),
        Ok(vec![
            area(None, Some(0)),
            area(Some(0), Some(4)),
            area(Some(4), None)
        ])
    );
    assert_eq!(
        cut_at(&[-2, 0, 5]),
        Ok(vec![
            area(None, Some(-2)),
            area(Some(-2), Some(0)),
            area(Some(0), Some(5)),
            area(Some(5), None)
        ])
    );
    for cuts in [&[4, 4][..], &[5, 4], &[-2, 0, 0], &[0, -2, 5]] {
        assert_eq!(
            Division::side_by_side(HOME, cuts),
            Err(NotAscending),
            "{cuts:?}"
        );
    }

    // They are the areas stripes with the same boundaries had, with the same home
    // chunk: what a store compares a world with.
    for cuts in [&[4][..], &[0, 4], &[-2, 0, 5]] {
        let told = Division::side_by_side(HOME, cuts).unwrap();
        let stripes: Vec<ChunkArea> = (0..=cuts.len())
            .map(|index| {
                let west = index.checked_sub(1).map(|west| cuts[west]);
                area(west, cuts.get(index).copied())
            })
            .collect();
        assert_eq!((told.home, &told.pinned), (HOME, &stripes), "{cuts:?}");
        // And a store that is started with them has that many pinned regions, of
        // which the one with the home chunk is home.
        let store = Store::memory_divided(generator(), told.clone()).unwrap();
        let regions = cuts.len() as u32 + 1;
        assert_eq!(
            store.regions().unwrap(),
            begun(&told, &[], regions),
            "{cuts:?}"
        );
    }
}

// T1. The table file is read from what any crash would leave once the start has
// returned: the record has a new world's table written by its start.
#[test]
fn a_new_world_without_pins_is_one_home_region_that_holds_the_home_chunk() {
    let told = following();
    let disk = Arc::new(MemoryDisk::default());
    let (store, lines) = logged(|| store_on(&disk, &told));
    let store = store.unwrap();
    // T9: a new world is not one that was made over.
    assert!(!says(&lines, "a new world"));
    assert_eq!(store.regions().unwrap(), begun(&told, &[], 1));

    for survival in SURVIVALS {
        let table = table_of(&disk.crashed(survival));
        let table = table.unwrap_or_else(|| panic!("{survival:?}: the world has no table"));
        assert_eq!(table.division, Vec::new(), "{survival:?}");
        assert_eq!(
            (table.next_region, table.home_chunk, table.home_region),
            (1, HOME, 0),
            "{survival:?}"
        );
        let home = TableRegion {
            id: 0,
            pinned: Vec::new(),
            grants: vec![(HOME, 0)],
        };
        assert_eq!(table.regions, vec![home], "{survival:?}");
        assert_eq!(table.absorbed, Vec::new(), "{survival:?}");
    }

    let (handle, restored) = store.open_region(hello_of(&told, 0, 1)).unwrap();
    assert!(has_ids(restored.entity_ids), "{:?}", restored.entity_ids);
    assert_eq!(restored.held, [(HOME, 0)]);
    assert_eq!(restored.pinned, Vec::new());
    assert_eq!((restored.state, restored.deltas), (None, Vec::new()));
    // It holds the home chunk, and nothing else.
    as_built(&handle, &Built::default(), HOME, "the home chunk");
    let beside = ChunkPos::new(HOME.x + 1, HOME.z);
    handle.request(StoreRequest::Load { position: beside });
    let not_held = StoreReply::NotHeld {
        position: beside,
        holder: None,
    };
    assert!(next(&handle, "a chunk beside the home chunk") == not_held);
    assert_eq!(store.regions().unwrap(), begun(&told, &[1], 1));
}

// T1, on the two kinds of store a process starts.
#[test]
fn a_new_world_without_pins_is_the_same_in_memory_and_in_a_directory() {
    let told = following();
    let directory = tempfile::tempdir().unwrap();
    let in_memory = Store::memory_divided(generator(), told.clone()).unwrap();
    let in_files = Store::local_divided(directory.path(), generator(), told.clone()).unwrap();
    let mut issued = Vec::new();
    for (kind, store) in [("in memory", &in_memory), ("in a directory", &in_files)] {
        assert_eq!(store.regions().unwrap(), begun(&told, &[], 1), "{kind}");
        let (_, restored) = store.open_region(hello_of(&told, 0, 1)).unwrap();
        assert!(has_ids(restored.entity_ids), "{kind}");
        assert_eq!(restored.held, [(HOME, 0)], "{kind}");
        assert_eq!(restored.pinned, Vec::new(), "{kind}");
        issued.push(restored.entity_ids);
    }

    // Started again without pins, the world in the directory is found as it was.
    at_rest(&in_files).unwrap();
    drop(in_files);
    let (again, lines) =
        logged(|| Store::local_divided(directory.path(), generator(), told.clone()));
    let again = again.unwrap();
    assert!(!says(&lines, "a world without pins, started again"));
    assert_eq!(again.regions().unwrap(), begun(&told, &[1], 1));
    let (_, restored) = again.open_region(hello_of(&told, 0, 2)).unwrap();
    assert_eq!(restored.entity_ids, issued[1]);
    assert_eq!(restored.held, [(HOME, 0)]);
}

/// Twenty chunks around the home chunk, in ascending order.
fn twenty() -> Vec<ChunkPos> {
    let square = (HOME.x - 2..=HOME.x + 2)
        .flat_map(|x| (HOME.z - 2..=HOME.z + 2).map(move |z| ChunkPos::new(x, z)));
    square.filter(|chunk| *chunk != HOME).take(20).collect()
}

/// Starts a world without pins on `disk`, and has region 0 commit three ticks and
/// claim the chunks of [`twenty`]. Returns whether the claim was answered, which it is
/// not if the disk failed before.
fn claims_twenty(disk: &Arc<MemoryDisk>) -> bool {
    let told = following();
    let Ok(store) = store_on(disk, &told) else {
        return false;
    };
    let Ok((handle, _)) = store.open_region(hello_of(&told, 0, 1)) else {
        return false;
    };
    for tick in 1..=3 {
        log(&handle, tick, &[]);
    }
    // Not in the order they are held in.
    let chunks = twenty().into_iter().rev().collect();
    handle.request(StoreRequest::Claim { chunks });
    // The list is made when everything asked before is durable and answered, or is
    // never going to be (ADR-0011, sections 3.2 and 5).
    let _ = store.regions();
    let answers: Vec<StoreReply> = handle.replies.try_iter().collect();
    let claim = answers.iter().position(|answer| {
        let StoreReply::Claimed { granted, foreign } = answer else {
            return false;
        };
        let mut granted = granted.clone();
        granted.sort();
        assert_eq!((&granted, foreign), (&twenty(), &Vec::new()));
        true
    });
    if let Some(claim) = claim {
        // Behind the answers to the commits asked for before it.
        let commits: Vec<StoreReply> = (1..=3).map(|tick| StoreReply::Committed { tick }).collect();
        assert_eq!(answers[..claim], commits);
    }
    claim.is_some()
}

// T2. "After any kill" is read as: whatever a crash keeps of the disk once the claim
// was answered, and with the store killed at every write and sync up to that answer.
// A claim that was not answered is granted or not (ADR-0011, section 4.3). The tick
// the chunks are held from is that of the region's last commit (ADR-0011, section
// 3.2).
#[test]
fn the_home_region_is_granted_the_twenty_chunks_it_claims_and_holds_them_after_any_kill() {
    let told = following();
    let operations = {
        let disk = Arc::new(MemoryDisk::default());
        assert!(claims_twenty(&disk));
        disk.operations()
    };
    let mut held: Vec<(ChunkPos, u64)> = twenty().into_iter().map(|chunk| (chunk, 3)).collect();
    held.push((HOME, 0));
    held.sort();
    let mut grown = twenty();
    grown.push(HOME);

    for n in 1..=operations + 1 {
        for fault in [Fault::Stop(n), Fault::Fail(n), Fault::Fails(n, 2)] {
            let disk = Arc::new(MemoryDisk::failing(fault));
            let answered = claims_twenty(&disk);
            for survival in SURVIVALS {
                let case = format!("{fault:?}, {survival:?}");
                let left = Arc::new(disk.crashed(survival));
                let store =
                    store_on(&left, &told).unwrap_or_else(|error| panic!("{case}: {error}"));
                let list = store.regions().unwrap();
                let (_, restored) = store
                    .open_region(hello_of(&told, 0, 2))
                    .unwrap_or_else(|error| panic!("{case}: {error}"));
                assert!(has_ids(restored.entity_ids), "{case}");
                assert_eq!(restored.pinned, Vec::new(), "{case}");
                if answered {
                    assert_eq!(restored.held, held, "{case}");
                    assert_eq!(ticks(&restored), [1, 2, 3], "{case}");
                    let epoch = list.regions[0].epoch;
                    let mut expected = begun(&told, &[epoch], 1);
                    expected.regions[0].bounds = around(&grown);
                    assert_eq!(list, expected, "{case}");
                } else {
                    // The world is one home region that holds the home chunk all the
                    // same, with all, some or none of what it claimed.
                    assert!(restored.held.contains(&(HOME, 0)), "{case}");
                    let known = |(chunk, tick): &(ChunkPos, u64)| held.contains(&(*chunk, *tick));
                    assert!(
                        restored.held.iter().all(known),
                        "{case}: {:?}",
                        restored.held
                    );
                    assert_eq!((list.home, list.next), (RegionId(0), RegionId(1)), "{case}");
                    assert_eq!(list.regions.len(), 1, "{case}");
                    assert_eq!(list.regions[0].pinned, Vec::new(), "{case}");
                    assert_eq!(list.absorbed, Vec::new(), "{case}");
                }
            }
        }
    }
}

// T2. That a chunk is free is seen in three ways: the region is no longer restored
// with it, after any kill once its flush behind the return was answered; it cannot
// load it, and nobody holds it; and another region, a part split off for this, is
// granted it, while the home chunk stays region 0's.
#[test]
fn a_return_frees_the_nineteen_chunks_and_never_the_home_chunk() {
    let told = following();
    let case = "a return";
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &told).unwrap();
    let (handle, _) = store.open_region(hello_of(&told, 0, 1)).unwrap();
    for tick in 1..=3 {
        log(&handle, tick, &[]);
    }
    let twenty = twenty();
    assert_eq!(
        claimed(&handle, &twenty, case),
        (twenty.clone(), Vec::new())
    );

    let (kept, returned) = twenty.split_last().expect("twenty chunks");
    let mut back = returned.to_vec();
    back.push(HOME);
    give_back(&handle, &back);
    // A flush behind a return is answered when the return is durable.
    let handle = flushed(handle);
    assert!(!handle.is_lost());

    for chunk in returned {
        handle.request(StoreRequest::Load { position: *chunk });
        let free = StoreReply::NotHeld {
            position: *chunk,
            holder: None,
        };
        assert!(next(&handle, case) == free, "{chunk:?} is not free");
    }
    as_built(&handle, &Built::default(), HOME, case);
    as_built(&handle, &Built::default(), *kept, case);
    let mut expected = begun(&told, &[1], 1);
    expected.regions[0].bounds = around(&[HOME, *kept]);
    assert_eq!(store.regions().unwrap(), expected);

    let mut held = vec![(HOME, 0), (*kept, 3)];
    held.sort();
    for survival in SURVIVALS {
        let left = Arc::new(disk.crashed(survival));
        let after = store_on(&left, &told).unwrap();
        let (_, restored) = after.open_region(hello_of(&told, 0, 2)).unwrap();
        assert_eq!(restored.held, held, "{survival:?}");
    }

    // Whoever asks first is granted what was given back, and is told whose the home
    // chunk is.
    log(&handle, 4, &[]);
    handle.request(StoreRequest::Checkpoint {
        tick: 4,
        state: whole("state", 4),
    });
    let handle = flushed(handle);
    let split_off = StoreReply::Split {
        region: RegionId(1),
    };
    assert_eq!(split(&handle, 5, &[*kept], 1, 1), split_off);
    let (part, _) = store.open_region(hello_of(&told, 1, 1)).unwrap();
    assert_eq!(
        claimed(&part, &back, case),
        (returned.to_vec(), vec![(HOME, RegionId(0))])
    );
}

/// Starts a world without pins on `disk`, has region 0 claim the chunks of
/// [`twenty`], give all but the last of them back with the home chunk, and ask for a
/// flush behind that. Returns `None` if the claim was not answered, as the disk had
/// failed by then, and else whether the flush was.
fn returns_nineteen(disk: &Arc<MemoryDisk>) -> Option<bool> {
    let told = following();
    let store = store_on(disk, &told).ok()?;
    let (handle, _) = store.open_region(hello_of(&told, 0, 1)).ok()?;
    for tick in 1..=3 {
        log(&handle, tick, &[]);
    }
    let twenty = twenty();
    handle.request(StoreRequest::Claim {
        chunks: twenty.clone(),
    });
    // At rest, the store has answered whatever it is going to answer.
    let _ = at_rest(&store);
    let granted = |answer: StoreReply| matches!(answer, StoreReply::Claimed { .. });
    if !handle.replies.try_iter().any(granted) {
        return None;
    }
    let mut back = twenty[..19].to_vec();
    back.push(HOME);
    give_back(&handle, &back);
    handle.request(StoreRequest::Flush);
    let _ = at_rest(&store);
    let flushed = |answer: StoreReply| answer == StoreReply::Flushed;
    Some(handle.replies.try_iter().any(flushed))
}

// T2, with the store killed at every write and sync up to the return: whatever is
// left, region 0 holds the home chunk and the chunk it kept; of the nineteen it holds
// none once the flush behind the return was answered, and before that those the disk
// kept the return of or not (ADR-0011, section 4.3: "the region's still, or free").
#[test]
fn whenever_the_store_is_killed_a_return_never_frees_the_home_chunk() {
    let told = following();
    let operations = {
        let disk = Arc::new(MemoryDisk::default());
        assert_eq!(returns_nineteen(&disk), Some(true));
        disk.operations()
    };
    let twenty = twenty();
    let kept = [(HOME, 0), (twenty[19], 3)];
    for n in 1..=operations + 1 {
        for fault in [Fault::Stop(n), Fault::Fail(n), Fault::Fails(n, 2)] {
            let disk = Arc::new(MemoryDisk::failing(fault));
            let Some(flushed) = returns_nineteen(&disk) else {
                continue;
            };
            for survival in SURVIVALS {
                let case = format!("{fault:?}, {survival:?}");
                let left = Arc::new(disk.crashed(survival));
                let store =
                    store_on(&left, &told).unwrap_or_else(|error| panic!("{case}: {error}"));
                let (_, restored) = store
                    .open_region(hello_of(&told, 0, 2))
                    .unwrap_or_else(|error| panic!("{case}: {error}"));
                let held = &restored.held;
                assert!(
                    kept.iter().all(|chunk| held.contains(chunk)),
                    "{case}: {held:?}"
                );
                if flushed {
                    assert_eq!(held[..], kept, "{case}");
                } else {
                    let claimed = |(chunk, tick): &(ChunkPos, u64)| {
                        kept.contains(&(*chunk, *tick)) || (twenty.contains(chunk) && *tick == 3)
                    };
                    assert!(held.iter().all(claimed), "{case}: {held:?}");
                }
            }
        }
    }
}

/// What a world had before a store was started on it with another division.
struct Before {
    /// Every chunk something was built in, with all that was built in it and
    /// confirmed, in ascending order.
    built: Vec<(ChunkPos, Chunk)>,
    /// The next region id of its table; 0 if it had no table.
    next: u32,
    /// What the region file of each of its regions said, by id.
    files: Vec<RegionFile>,
}

// The chunks of the world of two stripes: of the western stripe, of the part that is
// split off it, and of the eastern stripe, which has the home chunk.
const WEST_SAVED: ChunkPos = ChunkPos::new(-2, 1);
const WEST_LIVE: ChunkPos = ChunkPos::new(3, 0);
const PART: [ChunkPos; 2] = [ChunkPos::new(1, 5), ChunkPos::new(1, 6)];
const MERGED: ChunkPos = ChunkPos::new(-5, -5);
const EAST_SAVED: ChunkPos = ChunkPos::new(5, 0);
const EAST_LIVE: ChunkPos = ChunkPos::new(4, 9);

/// Lives in a new world of two stripes cut at [`CUT`], as T3 asks: both stripes have
/// commits in the log, a state file and a saved chunk, and a part is split off the
/// western one that holds two granted chunks and has changed blocks in both, one of
/// them saved since and one not. A second part is split off the western stripe,
/// builds, and is absorbed by it again, so that the world has an absorbed pair and a
/// stripe that was granted a chunk. The stripes are opened with the epochs 3 and 4,
/// the part that stays, region 2, with 5, and the one that goes, region 3, with 6.
/// Every commit is confirmed when this returns.
fn lives_on_stripes(store: &Store) -> Before {
    let told = stripes();
    let mut built = Built::default();
    let (west, was_west) = store.open_region(hello_of(&told, 0, 3)).unwrap();
    let (east, was_east) = store.open_region(hello_of(&told, 1, 4)).unwrap();
    assert!(has_ids(was_west.entity_ids) && has_ids(was_east.entity_ids));

    // The western stripe, up to a checkpoint that covers all it has committed, which
    // a split needs.
    let changes = [
        built.set(WEST_SAVED, (4, 100, 9), blocks::GLASS),
        built.set(PART[0], (0, -61, 15), blocks::AIR),
    ];
    log(&west, 1, &changes);
    save(&west, &built, WEST_SAVED, 1);
    save(&west, &built, PART[0], 1);
    west.request(StoreRequest::Checkpoint {
        tick: 1,
        state: whole("west", 1),
    });
    west.flush();
    let split_off = StoreReply::Split {
        region: RegionId(2),
    };
    assert_eq!(split(&west, 2, &PART, 5, 2), split_off);
    let (part, was_part) = store.open_region(hello_of(&told, 2, 5)).unwrap();
    assert_eq!(was_part.entity_ids, NO_ENTITY_IDS);
    assert_eq!(was_part.held, [(PART[0], 2), (PART[1], 2)]);

    // The part that is absorbed again, with what it built under a checkpoint, which
    // a merge needs of both regions.
    let split_off = StoreReply::Split {
        region: RegionId(3),
    };
    assert_eq!(split(&west, 3, &[MERGED], 6, 3), split_off);
    let (merged, was_merged) = store.open_region(hello_of(&told, 3, 6)).unwrap();
    assert_eq!(was_merged.held, [(MERGED, 3)]);
    log(
        &merged,
        4,
        &[built.set(MERGED, (11, 100, 11), blocks::GLASS)],
    );
    save(&merged, &built, MERGED, 4);
    merged.request(StoreRequest::Checkpoint {
        tick: 4,
        state: whole("merged part", 4),
    });
    merged.flush();
    let absorbed = StoreReply::Absorbed {
        absorbed: RegionId(3),
        chunks: vec![MERGED],
        pinned: Vec::new(),
    };
    assert_eq!(absorb(&west, 3, 6, 5), absorbed);

    // What stays in the log: of the western stripe, and of the part.
    let changes = [
        built.set(WEST_LIVE, (15, 101, 0), blocks::STONE),
        built.set(MERGED, (12, -61, 12), blocks::AIR),
    ];
    log(&west, 6, &changes);
    let changes = [
        built.set(PART[0], (7, 100, 7), blocks::GLASS),
        built.set(PART[1], (8, -61, 8), blocks::AIR),
    ];
    log(&part, 3, &changes);
    log(&part, 4, &[built.set(PART[1], (9, 102, 9), blocks::STONE)]);
    save(&part, &built, PART[1], 4);

    // The eastern stripe: a checkpoint, and a commit behind it.
    let changes = [
        built.set(EAST_SAVED, (2, 100, 2), blocks::GLASS),
        built.set(HOME, (3, -61, 4), blocks::AIR),
    ];
    log(&east, 1, &changes);
    save(&east, &built, EAST_SAVED, 1);
    save(&east, &built, HOME, 1);
    east.request(StoreRequest::Checkpoint {
        tick: 1,
        state: whole("east", 1),
    });
    let changes = [
        built.set(HOME, (3, 100, 4), blocks::GLASS),
        built.set(EAST_LIVE, (0, 100, 0), blocks::STONE),
    ];
    log(&east, 2, &changes);

    for handle in [&west, &east, &part] {
        handle.flush();
        assert!(!handle.is_lost());
    }
    let file = |epoch, entity_ids| RegionFile { epoch, entity_ids };
    Before {
        built: chunks_of(&built),
        next: 4,
        files: vec![
            file(3, was_west.entity_ids),
            file(4, was_east.entity_ids),
            file(5, NO_ENTITY_IDS),
        ],
    }
}

/// The world of [`lives_on_stripes`] on a disk in memory, at rest.
fn world_of_stripes() -> (Arc<MemoryDisk>, Before) {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &stripes()).unwrap();
    let before = lives_on_stripes(&store);
    at_rest(&store).unwrap();
    (disk, before)
}

/// The world of [`lives_on_stripes`] is as it was left: the list with the epochs the
/// regions were opened with and the absorbed pair, each region restored with its
/// state, its commits since, its grants and its entity ids when it is opened with the
/// epoch it had, the region that was absorbed refused as that, and every chunk as it
/// was built, as the region that holds it loads it. The hellos are those of a world
/// divided as `from` says.
fn as_lived_on_stripes(store: &Store, from: &Division, before: &Before, case: &str) {
    let (west, east) = (area(None, Some(CUT)), area(Some(CUT), None));
    let info = |region: usize, bounds, pinned| RegionInfo {
        region: RegionId(region as u32),
        epoch: before.files[region].epoch,
        bounds,
        pinned,
    };
    let expected = RegionList {
        home: RegionId(1),
        regions: vec![
            info(0, around(&[MERGED]), vec![west]),
            info(1, None, vec![east]),
            info(2, around(&PART), Vec::new()),
        ],
        absorbed: vec![(RegionId(3), RegionId(0))],
        next: RegionId(4),
    };
    let list = store
        .regions()
        .unwrap_or_else(|error| panic!("{case}: {error}"));
    assert_eq!(list, expected, "{case}");

    let state = |tick, name: &str| {
        Some(TickState {
            tick,
            state: whole(name, tick),
        })
    };
    let deltas = |ticks: &[u64]| -> Vec<TickState> {
        let of = |tick: &u64| TickState {
            tick: *tick,
            state: delta(*tick),
        };
        ticks.iter().map(of).collect()
    };
    let expected = [
        Restored {
            entity_ids: before.files[0].entity_ids,
            state: state(5, "merged"),
            deltas: deltas(&[6]),
            held: vec![(MERGED, 5)],
            pinned: vec![west],
            issued: EntityId(0),
        },
        Restored {
            entity_ids: before.files[1].entity_ids,
            state: state(1, "east"),
            deltas: deltas(&[2]),
            held: Vec::new(),
            pinned: vec![east],
            issued: EntityId(0),
        },
        Restored {
            entity_ids: NO_ENTITY_IDS,
            state: state(2, "part"),
            deltas: deltas(&[3, 4]),
            held: vec![(PART[0], 2), (PART[1], 2)],
            pinned: Vec::new(),
            issued: EntityId(0),
        },
    ];
    let mut handles = Vec::new();
    for (id, expected) in expected.iter().enumerate() {
        let hello = hello_of(from, id as u32, before.files[id].epoch);
        let (handle, restored) = store
            .open_region(hello)
            .unwrap_or_else(|error| panic!("{case}: region {id}: {error}"));
        assert_eq!(&restored, expected, "{case}: region {id}");
        handles.push(handle);
    }
    let refused = store.open_region(hello_of(from, 3, 1000)).err();
    assert!(
        matches!(
            &refused,
            Some(StoreError::Absorbed {
                region: RegionId(3),
                into: RegionId(0)
            })
        ),
        "{case}: {refused:?}"
    );
    // In ascending order, as the chunks that were built are kept.
    let holders = [
        (MERGED, 0),
        (WEST_SAVED, 0),
        (PART[0], 2),
        (PART[1], 2),
        (WEST_LIVE, 0),
        (EAST_LIVE, 1),
        (EAST_SAVED, 1),
        (HOME, 1),
    ];
    assert_eq!(holders.len(), before.built.len());
    for ((position, chunk), (held, holder)) in before.built.iter().zip(holders) {
        assert_eq!(*position, held);
        loads(&handles[holder], *position, chunk, case);
    }
}

// The chunks of the world without pins that is lived in for T4: what the home region
// claims east and west of where the world is cut afterwards, and its two parts.
const HERE: ChunkPos = ChunkPos::new(5, -3);
const THERE: ChunkPos = ChunkPos::new(2, 0);
const EAST_PART: [ChunkPos; 2] = [ChunkPos::new(8, 1), ChunkPos::new(9, 1)];
const WEST_PART: [ChunkPos; 2] = [ChunkPos::new(-3, 2), ChunkPos::new(3, 2)];

/// Lives in a new world without pins, as T4 asks: the home region, opened with epoch
/// 3, claims chunks, builds in them and has a state file, and a part is split off it
/// east of [`CUT`], which builds in its chunks and leaves that in the log. A second
/// part is split off west of the cut, so that the world has used more ids than the
/// two pinned regions it is given afterwards, and one of its parts has a state file.
/// The parts are regions 1 and 2, with the epochs 5 and 6.
fn lives_without_pins(store: &Store) -> Before {
    let told = following();
    let case = "the world without pins";
    let mut built = Built::default();
    let (home, was_home) = store.open_region(hello_of(&told, 0, 3)).unwrap();
    assert!(has_ids(was_home.entity_ids));
    let mut wanted = vec![HERE, THERE];
    wanted.extend(EAST_PART);
    wanted.extend(WEST_PART);
    wanted.sort();
    assert_eq!(claimed(&home, &wanted, case), (wanted.clone(), Vec::new()));

    let changes = [
        built.set(HOME, (3, -61, 4), blocks::AIR),
        built.set(HERE, (1, 100, 1), blocks::GLASS),
        built.set(THERE, (2, 100, 2), blocks::STONE),
        built.set(EAST_PART[0], (5, -61, 5), blocks::AIR),
        built.set(WEST_PART[0], (6, 100, 6), blocks::GLASS),
    ];
    log(&home, 1, &changes);
    for position in [HOME, HERE, THERE, EAST_PART[0], WEST_PART[0]] {
        save(&home, &built, position, 1);
    }
    home.request(StoreRequest::Checkpoint {
        tick: 1,
        state: whole("home", 1),
    });
    home.flush();
    let split_off = |region| StoreReply::Split {
        region: RegionId(region),
    };
    assert_eq!(split(&home, 2, &EAST_PART, 5, 1), split_off(1));
    assert_eq!(split(&home, 3, &WEST_PART, 6, 2), split_off(2));
    let (east, was_east) = store.open_region(hello_of(&told, 1, 5)).unwrap();
    let (west, was_west) = store.open_region(hello_of(&told, 2, 6)).unwrap();
    // Section 2.1: no other region of such a world ever has a block of entity ids.
    assert_eq!(was_east.entity_ids, NO_ENTITY_IDS);
    assert_eq!(was_west.entity_ids, NO_ENTITY_IDS);

    let changes = [
        built.set(HOME, (3, 100, 4), blocks::GLASS),
        built.set(THERE, (9, -61, 9), blocks::AIR),
    ];
    log(&home, 4, &changes);
    let changes = [
        built.set(EAST_PART[0], (7, 100, 7), blocks::STONE),
        built.set(EAST_PART[1], (8, -61, 8), blocks::AIR),
    ];
    log(&east, 3, &changes);
    log(
        &west,
        4,
        &[built.set(WEST_PART[1], (0, 100, 15), blocks::GLASS)],
    );
    save(&west, &built, WEST_PART[1], 4);
    west.request(StoreRequest::Checkpoint {
        tick: 4,
        state: whole("west part", 4),
    });
    log(
        &west,
        5,
        &[built.set(WEST_PART[0], (10, -61, 10), blocks::AIR)],
    );

    for handle in [&home, &east, &west] {
        handle.flush();
        assert!(!handle.is_lost());
    }
    let file = |epoch, entity_ids| RegionFile { epoch, entity_ids };
    Before {
        built: chunks_of(&built),
        next: 3,
        files: vec![
            file(3, was_home.entity_ids),
            file(5, NO_ENTITY_IDS),
            file(6, NO_ENTITY_IDS),
        ],
    }
}

/// The world of [`lives_without_pins`] on a disk in memory, at rest.
fn world_without_pins() -> (Arc<MemoryDisk>, Before) {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &following()).unwrap();
    let before = lives_without_pins(&store);
    at_rest(&store).unwrap();
    (disk, before)
}

/// A world with a `layout` file that says `layout`, and no table: two stripes cut at
/// x = 0 with the home chunk at the origin, as [`world_of_today`] makes it.
fn world_from_before(layout: &[u8]) -> (Arc<MemoryDisk>, Before) {
    let disk = world_of_today();
    put(&disk, LAYOUT, layout);
    assert!(table_of(&disk).is_none());
    let west = ChunkPos::new(-1, 0);
    let mut built = Built::default();
    built.set(west, (13, -61, 4), blocks::AIR);
    built.set(ORIGIN, (3, -61, 4), blocks::AIR);
    built.set(ORIGIN, (3, 100, 4), blocks::GLASS);
    let file = |epoch, block| RegionFile {
        epoch,
        entity_ids: EntityIds::block(block).expect("one of the first blocks"),
    };
    let before = Before {
        built: chunks_of(&built),
        next: 0,
        files: vec![file(3, 0), file(4, 1)],
    };
    (disk, before)
}

/// What a `layout` file can say: the layout the world was last served with, another,
/// one region, nothing, and nothing a store ever wrote. The first three are what a
/// store of then wrote for stripes cut at x = 0, for stripes cut at [`CUT`] and for a
/// world that was not divided.
fn layouts() -> Vec<Vec<u8>> {
    vec![
        b"9c191507aacacf62\n".to_vec(),
        b"9c191107aacac896\n".to_vec(),
        b"4d25767f9dce13f5\n".to_vec(),
        Vec::new(),
        b"no fingerprint at all\n".to_vec(),
    ]
}

/// The divisions a world from before is started with in T5: without pins, with the
/// home chunk where it was and elsewhere; the very stripes the world had, as pins;
/// and other pins.
fn divisions() -> Vec<Division> {
    vec![
        following(),
        Division::open(ORIGIN),
        Division::side_by_side(ORIGIN, &[0]).unwrap(),
        pinned_at(&[-8, 16]),
    ]
}

/// The world behind `store` is as section 2.2 says a world is that was made over for
/// `told`, and nothing was opened in since:
///
/// - the list is that of `told` alone (T1's, for a world without pins), with the
///   epochs the region files of the same ids had, but for `next`, which is at least
///   the old table's;
/// - a hello for a region the world had and has no more, living or absorbed, is
///   refused as for a region the table does not have (T9);
/// - every region is opened with the epoch its id was last opened with (T9), and is
///   restored without a state, without commits and without grants but the home
///   chunk, with the entity ids its id had, or with a block of its own that no other
///   region has or had (ADR-0011, section 2);
/// - every block that was confirmed is in its chunk, as the region that holds the
///   chunk loads it: region 0 once it has claimed it, in a world without pins.
fn begun_anew(store: &Store, told: &Division, before: &Before, case: &str) {
    let list = store
        .regions()
        .unwrap_or_else(|error| panic!("{case}: {error}"));
    let made = told.pinned.len().max(1);
    assert!(
        list.next.0 as usize >= made.max(before.next as usize),
        "{case}: the next region is {}",
        list.next
    );
    let epochs: Vec<u64> = before.files.iter().map(|file| file.epoch).collect();
    assert_eq!(list, begun(told, &epochs, list.next.0), "{case}");

    for gone in made..before.files.len().max(before.next as usize) {
        let gone = RegionId(gone as u32);
        let refused = store.open_region(hello_of(told, gone.0, 1000)).err();
        assert!(
            matches!(&refused, Some(StoreError::UnknownRegion { region }) if *region == gone),
            "{case}: region {gone}: {refused:?}"
        );
    }

    let had_ids = |id: usize| {
        let ids = before.files.get(id).map(|file| file.entity_ids);
        ids.filter(|ids| has_ids(*ids))
    };
    let mut issued: Vec<EntityIds> = (0..before.files.len()).filter_map(had_ids).collect();
    let mut handles = Vec::new();
    for info in &list.regions {
        let id = info.region.0 as usize;
        let hello = hello_of(told, info.region.0, info.epoch.max(1));
        let (handle, restored) = store
            .open_region(hello)
            .unwrap_or_else(|error| panic!("{case}: region {id}: {error}"));
        assert_eq!(
            (&restored.state, &restored.deltas),
            (&None, &Vec::new()),
            "{case}: region {id}"
        );
        let held = if told.pinned.is_empty() {
            vec![(told.home, 0)]
        } else {
            Vec::new()
        };
        assert_eq!(restored.held, held, "{case}: region {id}");
        let pinned: Vec<ChunkArea> = told.pinned.get(id).copied().into_iter().collect();
        assert_eq!(restored.pinned, pinned, "{case}: region {id}");
        match had_ids(id) {
            Some(ids) => assert_eq!(restored.entity_ids, ids, "{case}: region {id}"),
            None => {
                let ids = restored.entity_ids;
                assert!(
                    has_ids(ids) && !issued.contains(&ids),
                    "{case}: region {id} is issued {ids:?}"
                );
                issued.push(ids);
            }
        }
        handles.push(handle);
    }

    for (position, chunk) in &before.built {
        let holder = if told.pinned.is_empty() {
            let granted = claimed(&handles[0], &[*position], case);
            assert_eq!(granted, (vec![*position], Vec::new()), "{case}");
            0
        } else {
            let holder = told.pinned.iter().position(|area| area.contains(*position));
            holder.expect("regions side by side cover the world")
        };
        loads(&handles[holder], *position, chunk, case);
    }
}

/// Starts a store for `told` on `left`, which is what a crash left of a world that is
/// being made over for `told` or is to be, and checks all that T3 says of every such
/// start:
///
/// - it says in its log that it made the world over, with both lines, if the table it
///   found was not that of `told`, and neither line if it was (T9);
/// - no `layout` file is left, also not in what a crash would keep from then on, and
///   the file had not gone before there was a table (T5);
/// - a second start, on what a crash would keep once the first has returned, says
///   nothing, has the same list and writes nothing: no file is changed by it;
/// - after either start the world is as [`begun_anew`] says.
fn starts_made_over(left: &Arc<MemoryDisk>, told: &Division, before: &Before, case: &str) {
    let table = table_of(left);
    assert!(
        table.is_some() || file_at(left, LAYOUT).is_some(),
        "{case}: the layout file has gone and no table is there"
    );
    let found_made_over = table.is_some_and(|table| is_of(&table, told));
    let (store, lines) = logged(|| store_on(left, told));
    let store = store.unwrap_or_else(|error| panic!("{case}: {error}"));
    assert_eq!(says(&lines, case), !found_made_over, "{case}: {lines:?}");
    assert_eq!(file_at(left, LAYOUT), None, "{case}");

    // What a crash would keep now that the start has returned: the world is made
    // over before any hello is taken, and nothing of it is still to become durable.
    let again = Noting::of(left.crashed(Survival::Nothing));
    assert_eq!(file_at(&again.disk, LAYOUT), None, "{case}");
    let list = store
        .regions()
        .unwrap_or_else(|error| panic!("{case}: {error}"));
    let table = file_at(left, TABLE);
    let case_again = format!("{case}, a second start");
    let (second, lines) = logged(|| again.started(told));
    let second = second.unwrap_or_else(|error| panic!("{case_again}: {error}"));
    assert!(!says(&lines, &case_again), "{case_again}: {lines:?}");
    assert_eq!(again.changed(), Vec::<PathBuf>::new(), "{case_again}");
    assert_eq!(second.regions().unwrap(), list, "{case_again}");
    assert_eq!(file_at(&again.disk, TABLE), table, "{case_again}");

    begun_anew(&second, told, before, &case_again);
    begun_anew(&store, told, before, case);
}

/// How many times a start for `told` on `world` changes or syncs the disk, until the
/// store is at rest.
fn operations_of_a_start(world: &MemoryDisk, told: &Division) -> u64 {
    let disk = Arc::new(world.crashed(Survival::Everything));
    let store = store_on(&disk, told).expect("the start, on a disk that works");
    at_rest(&store).expect("the store at rest, on a disk that works");
    disk.operations()
}

/// Starts a store for `told` on `world`, which was divided otherwise, killed at every
/// change and sync of that start: stopped there for good, or with that one or two
/// failing. Whatever a crash keeps of each, a start on it is as [`starts_made_over`]
/// says. A start that returns although the disk failed has made the world over, and
/// has to say so.
fn made_over_at_every_kill_point(world: &MemoryDisk, told: &Division, before: &Before) {
    let operations = operations_of_a_start(world, told);
    for n in 1..=operations + 1 {
        for fault in [Fault::Stop(n), Fault::Fail(n), Fault::Fails(n, 2)] {
            let disk = Arc::new(world.crashed(Survival::Everything).with(fault));
            let (killed, lines) = logged(|| store_on(&disk, told));
            if killed.is_ok() {
                assert!(says(&lines, &format!("{fault:?}")), "{fault:?}: {lines:?}");
            }
            // It fails or not; either way it has done what it has.
            drop(killed);
            for survival in SURVIVALS {
                let left = Arc::new(disk.crashed(survival));
                starts_made_over(&left, told, before, &format!("{fault:?}, {survival:?}"));
            }
        }
    }
}

// T3, and T9 for the lines of its log. The world of stripes is taken as any crash
// would leave it once it was at rest.
#[test]
fn a_world_of_stripes_started_without_pins_is_one_home_region_with_all_that_was_built() {
    let (world, before) = world_of_stripes();
    for survival in SURVIVALS {
        let left = Arc::new(world.crashed(survival));
        starts_made_over(&left, &following(), &before, &format!("{survival:?}"));
    }
}

// T3: "killed at every write and sync of that start and with everything a crash can
// keep". Beside a store that stops for good at a point, one is tried whose disk fails
// there once or twice and works again, as the store's other tests of kills have it.
#[test]
fn a_world_of_stripes_is_made_over_for_no_pins_whenever_the_start_is_killed() {
    let (world, before) = world_of_stripes();
    made_over_at_every_kill_point(&world, &following(), &before);
    // And on the world with nothing that was not durable: the chunk its part saved
    // last is under no checkpoint, and is in the log alone.
    let durable = world.crashed(Survival::Nothing);
    made_over_at_every_kill_point(&durable, &following(), &before);
}

// T3, in a directory, where a process keeps a world.
#[test]
fn a_world_of_stripes_in_a_directory_is_made_over_for_no_pins() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::local_divided(directory.path(), generator(), stripes()).unwrap();
    let before = lives_on_stripes(&store);
    at_rest(&store).unwrap();
    drop(store);

    let told = following();
    let (store, lines) =
        logged(|| Store::local_divided(directory.path(), generator(), told.clone()));
    let store = store.unwrap();
    assert!(says(&lines, "in a directory"), "{lines:?}");
    assert!(!directory.path().join("layout").exists());
    begun_anew(&store, &told, &before, "in a directory");
}

// T9: "After T3's start, a hello for region 1 of the world of stripes is refused as
// for a region the table does not have, and one for region 0 with the epoch stripe 0
// was last opened with is taken and restores a region without a state." The hellos
// are a region and an epoch, so a worker that lived through the start (N16) says
// them as one that was started since.
#[test]
fn after_stripes_are_made_over_a_hello_for_stripe_1_is_refused_and_one_for_stripe_0_begins_anew() {
    let (world, before) = world_of_stripes();
    let told = following();
    let case = "after the start";
    let left = Arc::new(world.crashed(Survival::Nothing));
    let (store, lines) = logged(|| store_on(&left, &told));
    let store = store.unwrap();
    assert!(says(&lines, case), "{case}: {lines:?}");

    // The eastern stripe, the part that was split off the western one, and the
    // part that it had absorbed again.
    for gone in [1, 2, 3] {
        for epoch in [1, 4, 5, 6, 1000] {
            let refused = store.open_region(hello_of(&told, gone, epoch)).err();
            assert!(
                matches!(
                    &refused,
                    Some(StoreError::UnknownRegion { region }) if *region == RegionId(gone)
                ),
                "{case}: region {gone} with epoch {epoch}: {refused:?}"
            );
        }
    }

    // Region 0 has the epoch stripe 0 had (section 2.2): a lower one is refused,
    // and that one is taken.
    let refused = store.open_region(hello_of(&told, 0, 2)).err();
    assert!(
        matches!(
            &refused,
            Some(StoreError::EpochRefused {
                region: RegionId(0),
                offered: 2,
                seen: 3
            })
        ),
        "{case}: {refused:?}"
    );
    let (_, restored) = store
        .open_region(hello_of(&told, 0, 3))
        .unwrap_or_else(|error| panic!("{case}: {error}"));
    assert_eq!(
        (restored.state, restored.deltas),
        (None, Vec::new()),
        "{case}"
    );
    assert_eq!(restored.held, [(HOME, 0)], "{case}");
    assert_eq!(restored.pinned, Vec::new(), "{case}");
    assert_eq!(restored.entity_ids, before.files[0].entity_ids, "{case}");
}

// T4, and T9 for the lines of its log: two pinned regions, ids 0 and 1, home the one
// with the home chunk, the parts gone, their blocks in the chunks. The ids the world
// had used are not given out again, and region 1, whose id was a part's, has that
// part's epoch and is issued entity ids of its own (ADR-0011, section 2).
#[test]
fn a_world_without_pins_started_with_a_pin_is_two_pinned_regions_with_all_that_was_built() {
    let (world, before) = world_without_pins();
    let told = pinned_at(&[CUT]);
    for survival in SURVIVALS {
        let left = Arc::new(world.crashed(survival));
        starts_made_over(&left, &told, &before, &format!("{survival:?}"));
    }

    // Said once more without the helpers, for what T4 names.
    let left = Arc::new(world.crashed(Survival::Nothing));
    let store = store_on(&left, &told).unwrap();
    let list = store.regions().unwrap();
    assert_eq!(list.home, RegionId(1));
    let regions: Vec<(RegionId, Vec<ChunkArea>)> = list
        .regions
        .iter()
        .map(|info| (info.region, info.pinned.clone()))
        .collect();
    let pinned = vec![
        (RegionId(0), vec![area(None, Some(CUT))]),
        (RegionId(1), vec![area(Some(CUT), None)]),
    ];
    assert_eq!(regions, pinned);
    assert_eq!((list.absorbed, list.next), (Vec::new(), RegionId(3)));
    let refused = store.open_region(hello_of(&told, 1, 4)).err();
    assert!(
        matches!(&refused, Some(StoreError::EpochRefused { seen: 5, .. })),
        "{refused:?}"
    );
}

// T4, killed as T3 is: "the other way round".
#[test]
fn a_world_without_pins_is_made_over_for_a_pin_whenever_the_start_is_killed() {
    let (world, before) = world_without_pins();
    made_over_at_every_kill_point(&world, &pinned_at(&[CUT]), &before);
}

// T5, and T9 for the lines of its log: whatever the file says, and whatever the
// store is told, the world is made over. The third division is the very stripes the
// world had: with the first layout file, a store of before this step kept such a
// world as it was.
#[test]
fn a_world_with_a_layout_file_and_no_table_is_made_over_whatever_the_file_says() {
    for layout in layouts() {
        for told in divisions() {
            let (world, before) = world_from_before(&layout);
            for survival in SURVIVALS {
                let case = format!(
                    "{:?}, {told:?}, {survival:?}",
                    String::from_utf8_lossy(&layout)
                );
                let left = Arc::new(world.crashed(survival));
                assert!(file_at(&left, LAYOUT).is_some(), "{case}");
                starts_made_over(&left, &told, &before, &case);
            }
        }
    }
}

// T5: "made over as T3", so killed at every write and sync as well, and "the file is
// gone when the table is durable". That is read both ways: whatever a kill leaves,
// the file is not gone unless a table is there, and once a start has returned the
// file is gone for good ([`starts_made_over`] checks both).
#[test]
fn a_world_with_a_layout_file_is_made_over_for_no_pins_whenever_the_start_is_killed() {
    let layouts = layouts();
    for (layout, told) in [
        (&layouts[0], following()),
        (&layouts[4], Division::open(ORIGIN)),
    ] {
        let (world, before) = world_from_before(layout);
        made_over_at_every_kill_point(&world, &told, &before);
    }
}

// T5, as the test before, for pins: the stripes the world had, and others.
#[test]
fn a_world_with_a_layout_file_is_made_over_for_pins_whenever_the_start_is_killed() {
    let layouts = layouts();
    for (layout, told) in [
        (&layouts[0], Division::side_by_side(ORIGIN, &[0]).unwrap()),
        (&layouts[3], pinned_at(&[-8, 16])),
    ] {
        let (world, before) = world_from_before(layout);
        made_over_at_every_kill_point(&world, &told, &before);
    }
}

// T9, where a start fails and its process lives. A finding.
//
// The sequence: a world that was divided otherwise (the world of stripes, with its
// table, or the world with a `layout` file and none) is started without pins on a
// disk on which one sync fails, once: the sync of `regions/` that makes the table of
// the new division durable, when the file has been renamed into place. That start
// returns the error and has said neither line. Nothing crashed, so the table stays
// where it is. The store is started again, finds a world that is divided as it is
// told, and says neither line as well.
//
// What the record says: section 2.2, "The store says so in its log once", and "The
// store therefore writes a second line whenever it makes a world over", as "nobody
// who was in the world stays in it"; T9, "Whenever a start makes a world over (T3,
// T4, T5), the log has both lines of section 2.2, once each". It does not say which
// start that is when one fails on the way and the next finds the table in place.
// Read here as: one of the two says so, as nothing else tells whoever runs the
// cluster that everybody has to join again.
//
// What happened: the world is made over, as the other tests of T3 and T5 find on
// what such a start leaves, and no log has either line. A start that fails one step
// later, at removing a region file or the `layout` file, has said both.
#[test]
fn a_world_that_is_made_over_by_a_start_that_fails_is_said_to_be_by_that_start_or_the_next() {
    let (stripes, _) = world_of_stripes();
    let (from_before, _) = world_from_before(&layouts()[0]);
    // Every fault after which nobody said so, with how many times that start changes
    // or syncs the disk when nothing fails.
    let mut unsaid = Vec::new();
    for (name, world) in [("stripes", &stripes), ("a layout file", &from_before)] {
        let told = following();
        let operations = operations_of_a_start(world, &told);
        for n in 1..=operations + 1 {
            for fault in [Fault::Fail(n), Fault::Fails(n, 2)] {
                let case = format!("{name}, {fault:?} of {operations}");
                let disk = Arc::new(world.crashed(Survival::Everything).with(fault));
                let (failed, lines) = logged(|| store_on(&disk, &told));
                let said = says(&lines, &case);
                drop(failed);
                // No crash: the process goes on, and the store is started again.
                let left = Arc::new(disk.crashed(Survival::Everything));
                let (store, lines) = logged(|| store_on(&left, &told));
                store.unwrap_or_else(|error| panic!("{case}: {error}"));
                if !said && !says(&lines, &case) {
                    unsaid.push(case);
                }
            }
        }
    }
    assert!(
        unsaid.is_empty(),
        "neither start says that the world was made over: {unsaid:?}"
    );
}

fn names_the_table(path: &Path) -> bool {
    let name = path.file_name().map(|name| name.to_string_lossy());
    name.is_some_and(|name| name.starts_with("table"))
}

// T7, and T9: such a start has neither line. "Found as it was" is: the list with the
// epochs the regions had, and every region restored with its state, its commits, its
// grants and its entity ids, the part among them. "The table file not written" is
// seen on a disk that notes what is written, and not by comparing the file alone.
#[test]
fn a_world_of_stripes_started_with_its_boundary_as_a_pin_is_found_as_it_was() {
    let (world, before) = world_of_stripes();
    let told = pinned_at(&[CUT]);
    for survival in SURVIVALS {
        let case = format!("{survival:?}");
        let left = Noting::of(world.crashed(survival));
        let table = file_at(&left.disk, TABLE);
        assert!(table.is_some(), "{case}");
        let (store, lines) = logged(|| left.started(&told));
        let store = store.unwrap_or_else(|error| panic!("{case}: {error}"));
        assert!(!says(&lines, &case), "{case}: {lines:?}");
        let written = left.changed();
        assert!(
            !written.iter().any(|path| names_the_table(path)),
            "{case}: {written:?}"
        );
        assert_eq!(file_at(&left.disk, TABLE), table, "{case}");
        as_lived_on_stripes(&store, &told, &before, &case);
    }
}

// T7, in a directory.
#[test]
fn a_world_of_stripes_in_a_directory_is_found_as_it_was_by_a_pin() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::local_divided(directory.path(), generator(), stripes()).unwrap();
    let before = lives_on_stripes(&store);
    at_rest(&store).unwrap();
    drop(store);

    let told = pinned_at(&[CUT]);
    let table = std::fs::read(directory.path().join("regions/table")).unwrap();
    let (store, lines) =
        logged(|| Store::local_divided(directory.path(), generator(), told.clone()));
    let store = store.unwrap();
    assert!(!says(&lines, "in a directory"), "{lines:?}");
    assert_eq!(
        std::fs::read(directory.path().join("regions/table")).unwrap(),
        table
    );
    as_lived_on_stripes(&store, &told, &before, "in a directory");
}

// T7, killed: a start that finds the world as it was leaves it so wherever it dies,
// and says nothing of a world made over.
#[test]
fn a_world_of_stripes_is_found_as_it_was_by_a_pin_whenever_the_start_is_killed() {
    let (world, before) = world_of_stripes();
    let told = pinned_at(&[CUT]);
    let operations = operations_of_a_start(&world, &told);
    for n in 1..=operations + 1 {
        for fault in [Fault::Stop(n), Fault::Fail(n), Fault::Fails(n, 2)] {
            let disk = Arc::new(world.crashed(Survival::Everything).with(fault));
            let (killed, lines) = logged(|| store_on(&disk, &told));
            assert!(!says(&lines, &format!("{fault:?}")), "{fault:?}: {lines:?}");
            drop(killed);
            for survival in SURVIVALS {
                let case = format!("{fault:?}, {survival:?}");
                let left = Arc::new(disk.crashed(survival));
                let (store, lines) = logged(|| store_on(&left, &told));
                let store = store.unwrap_or_else(|error| panic!("{case}: {error}"));
                assert!(!says(&lines, &case), "{case}: {lines:?}");
                as_lived_on_stripes(&store, &told, &before, &case);
            }
        }
    }
}

// Section 2.2, which T7 and the second start of T3 rest on: "making a world over is
// the only thing a start writes besides a new world's table". So a start that finds
// the world as it was changes no file at all: of a world of stripes started with its
// boundary as a pin, and of a world without pins started without.
#[test]
fn a_start_that_finds_the_world_as_it_was_writes_nothing() {
    let (stripes, _) = world_of_stripes();
    let (without_pins, _) = world_without_pins();
    for (name, world, told) in [
        ("stripes", &stripes, pinned_at(&[CUT])),
        ("no pins", &without_pins, following()),
    ] {
        for survival in SURVIVALS {
            let case = format!("{name}, {survival:?}");
            let left = Noting::of(world.crashed(survival));
            let (store, lines) = logged(|| left.started(&told));
            store.unwrap_or_else(|error| panic!("{case}: {error}"));
            assert!(!says(&lines, &case), "{case}: {lines:?}");
            assert_eq!(left.changed(), Vec::<PathBuf>::new(), "{case}");
        }
    }
}

// Section 2.2, the second row of its table: "other areas or another home chunk". The
// world of stripes started with its boundary as a pin and the home chunk elsewhere is
// made over although the areas are those it had: the two stripes begin anew, and the
// home region is the one that has the new home chunk.
#[test]
fn a_world_of_stripes_started_with_its_boundary_as_a_pin_and_another_home_chunk_is_made_over() {
    let (world, before) = world_of_stripes();
    let told = Division::side_by_side(ORIGIN, &[CUT]).unwrap();
    for survival in SURVIVALS {
        let left = Arc::new(world.crashed(survival));
        starts_made_over(&left, &told, &before, &format!("{survival:?}"));
    }
    let left = Arc::new(world.crashed(Survival::Nothing));
    let store = store_on(&left, &told).unwrap();
    assert_eq!(store.regions().unwrap().home, RegionId(0));
}

/// Has region 0 of a world without pins claim a chunk far from home and splits it
/// off under the next id of the list, as the epoch 1. The part has to be what a part
/// is: opened with the epoch of its split (ADR-0011, section 3.7), without entity
/// ids (section 2.1), holding its chunk from the tick of the split. Returns its id.
fn splits_off_a_part(store: &Store, told: &Division, epoch: u64, case: &str) -> RegionId {
    let next = store.regions().unwrap().next;
    let (home, _) = store
        .open_region(hello_of(told, 0, epoch))
        .unwrap_or_else(|error| panic!("{case}: {error}"));
    let far = ChunkPos::new(told.home.x + 30, told.home.z);
    assert_eq!(
        claimed(&home, &[far], case),
        (vec![far], Vec::new()),
        "{case}"
    );
    log(&home, 1, &[]);
    home.request(StoreRequest::Checkpoint {
        tick: 1,
        state: whole("state", 1),
    });
    let home = flushed(home);
    assert_eq!(
        split(&home, 2, &[far], 1, next.0),
        StoreReply::Split { region: next },
        "{case}"
    );
    let (_, restored) = store
        .open_region(hello_of(told, next.0, 1))
        .unwrap_or_else(|error| panic!("{case}: the part, region {next}: {error}"));
    assert_eq!(restored.entity_ids, NO_ENTITY_IDS, "{case}: region {next}");
    assert_eq!(restored.held, [(far, 2)], "{case}");
    let state = TickState {
        tick: 2,
        state: whole("part", 2),
    };
    assert_eq!(restored.state, Some(state), "{case}");
    next
}

// T3, of section 2.2: "ids below the old table's next id are not given to regions
// made later". The world of stripes had used the ids up to 3.
#[test]
fn a_part_that_is_split_off_after_stripes_were_made_over_has_an_id_the_world_never_used() {
    let (world, before) = world_of_stripes();
    let told = following();
    let left = Arc::new(world.crashed(Survival::Nothing));
    let store = store_on(&left, &told).unwrap();
    let part = splits_off_a_part(&store, &told, 3, "after stripes");
    assert!(part.0 >= before.next, "the part is region {part}");
    // And the store that starts on it finds both, with the next id above them.
    at_rest(&store).unwrap();
    let after = store_on(&Arc::new(left.crashed(Survival::Nothing)), &told).unwrap();
    let list = after.regions().unwrap();
    let regions: Vec<RegionId> = list.regions.iter().map(|info| info.region).collect();
    assert_eq!(regions, [RegionId(0), part]);
    assert_eq!(list.next, RegionId(part.0 + 1));
}

// T5, and what follows it: a world with a `layout` file has no table whose next id
// could be kept, and the region files of both its stripes stay, as each has a block
// of entity ids (ADR-0011, section 2). The record does not say what the next id of
// such a world is once it is made over for no pins. Whatever it is, the first part
// that is split off afterwards has to be a part like any other: section 2.1 has "no
// other region of such a world ever has one", a block of entity ids, and ADR-0011,
// section 3.7, has the part opened by an ordinary hello with the epoch of its split.
#[test]
fn a_part_that_is_split_off_in_a_world_from_before_is_a_part_like_any_other() {
    let (world, _) = world_from_before(&layouts()[0]);
    let told = Division::open(ORIGIN);
    let left = Arc::new(world.crashed(Survival::Nothing));
    let store = store_on(&left, &told).unwrap();
    splits_off_a_part(&store, &told, 3, "a world from before");
}

/// The bytes with one bit of the checksum at their end turned over.
fn damaged(bytes: &[u8]) -> Vec<u8> {
    let mut bytes = bytes.to_vec();
    let last = bytes.last_mut().expect("a file with a checksum");
    *last ^= 0x01;
    bytes
}

// Section 2.2: "A world that cannot be opened says so and is not touched, as today:
// a table or a region file that cannot be read ends the start with
// `StoreError::Damaged` or `StoreError::Table` before anything is written". Tried
// with divisions that would make each world over, and with the one that finds the
// world of stripes as it was.
#[test]
fn a_world_whose_table_or_region_file_cannot_be_read_is_not_touched() {
    let (stripes, _) = world_of_stripes();
    let (from_before, _) = world_from_before(&layouts()[0]);
    let region_file = "/world/regions/1.region";
    let mut impossible = table_of(&stripes).expect("the world of stripes has a table");
    impossible.home_region = 7;
    let table = file_at(&stripes, TABLE).unwrap();
    let cases = [
        ("the table", &stripes, TABLE, damaged(&table), false),
        (
            "a table that cannot be",
            &stripes,
            TABLE,
            impossible.encode(),
            true,
        ),
        (
            "a region file",
            &stripes,
            region_file,
            damaged(&file_at(&stripes, region_file).unwrap()),
            false,
        ),
        (
            "a region file beside a layout file",
            &from_before,
            region_file,
            damaged(&file_at(&from_before, region_file).unwrap()),
            false,
        ),
    ];
    for (name, world, path, contents, of_the_table) in cases {
        for told in [following(), pinned_at(&[CUT]), pinned_at(&[-8, 16])] {
            let case = format!("{name}, {told:?}");
            let disk = world.crashed(Survival::Everything);
            put(&disk, path, &contents);
            let left = Noting::of(disk);
            let (store, lines) = logged(|| left.started(&told));
            let refused = store.err();
            let said_so = match &refused {
                Some(StoreError::Damaged { path: at, .. }) => {
                    !of_the_table && at == Path::new(path)
                }
                Some(StoreError::Table(_)) => of_the_table,
                _ => false,
            };
            assert!(said_so, "{case}: {refused:?}");
            assert!(!says(&lines, &case), "{case}: {lines:?}");
            assert_eq!(left.changed(), Vec::<PathBuf>::new(), "{case}");
        }
    }
}

// Section 2.1: "`Store::memory`, `Store::local`, `spawn` and `spawn_local` go on
// meaning what they mean: one region, region 0, pinned to the whole world, with the
// home chunk at the origin."
#[test]
fn a_store_that_is_told_no_division_has_one_region_pinned_to_the_whole_world() {
    let directory = tempfile::tempdir().unwrap();
    let in_memory = Store::memory(generator());
    let in_files = Store::local(directory.path(), generator()).unwrap();
    let whole_world = RegionInfo {
        region: RegionId(0),
        epoch: 0,
        bounds: None,
        pinned: vec![ChunkArea::EVERYWHERE],
    };
    let expected = RegionList {
        home: RegionId(0),
        regions: vec![whole_world],
        absorbed: Vec::new(),
        next: RegionId(1),
    };
    for (kind, store) in [("in memory", in_memory), ("in a directory", in_files)] {
        assert_eq!(store.regions().unwrap(), expected, "{kind}");
        let hello = RegionHello {
            region: RegionId(0),
            epoch: 1,
        };
        let (handle, restored) = store.open_region(hello).unwrap();
        assert_eq!(restored.pinned, [ChunkArea::EVERYWHERE], "{kind}");
        assert_eq!(restored.held, Vec::new(), "{kind}");
        as_built(&handle, &Built::default(), ORIGIN, kind);
        as_built(&handle, &Built::default(), HELD, kind);
    }
}

/// T10.1 and, after it, T10.3, in a world divided as `told` says, for the region
/// that holds the chunk the generator is held for once it has claimed it.
fn waits_for_the_thread_for_chunks(told: &Division, region: u32) {
    let case = format!("{told:?}");
    let rig = Rig::new(told);
    let (handle, _) = rig.store.open_region(hello_of(told, region, 1)).unwrap();
    assert_eq!(claimed(&handle, &[HELD], &case), (vec![HELD], Vec::new()));

    // Without waiting for any answer.
    for tick in 1..=3 {
        log(&handle, tick, &[]);
    }
    handle.request(StoreRequest::Load { position: HELD });
    handle.request(StoreRequest::Checkpoint {
        tick: 3,
        state: whole("state", 3),
    });
    drop(handle);

    // The thread for chunks is in the generator, and stays there.
    rig.held.wait();
    let barrier = rig.store.barrier();
    // The list is asked for behind the barrier, so the commit thread has had the
    // barrier's turn when it is answered.
    rig.store.regions().unwrap();
    assert!(
        matches!(barrier.try_recv(), Err(TryRecvError::Empty)),
        "{case}"
    );
    for survival in SURVIVALS {
        let (_, restored) = left_by(&rig.disk, survival, told, 2);
        let restored = &restored[region as usize];
        assert_eq!(ticks(restored), [1, 2, 3], "{case}, {survival:?}");
        assert_eq!(restored.state, None, "{case}, {survival:?}");
    }
    assert!(
        matches!(barrier.try_recv(), Err(TryRecvError::Empty)),
        "{case}"
    );

    rig.held.wait();
    let rested = barrier.recv_timeout(PATIENCE);
    assert!(matches!(rested, Ok(Ok(()))), "{case}: {rested:?}");
    // "With nothing that was not durable", and with all of it.
    for survival in [Survival::Nothing, Survival::Everything] {
        let (_, restored) = left_by(&rig.disk, survival, told, 2);
        let restored = &restored[region as usize];
        let state = TickState {
            tick: 3,
            state: whole("state", 3),
        };
        assert_eq!(restored.state, Some(state), "{case}, {survival:?}");
        assert_eq!(ticks(restored), [0u64; 0], "{case}, {survival:?}");
    }

    // T10.3: nothing is written afterwards.
    let done = rig.disk.operations();
    assert_eq!(rig.ended(), done, "{case}");
}

// T10.1 and T10.3, in a world without pins.
#[test]
fn the_store_is_at_rest_only_when_the_thread_for_chunks_is_and_writes_nothing_after() {
    waits_for_the_thread_for_chunks(&following(), 0);
}

// T10.1 and T10.3, for a pinned region.
#[test]
fn a_store_of_pinned_regions_is_at_rest_only_when_the_thread_for_chunks_is() {
    waits_for_the_thread_for_chunks(&pinned_at(&[CUT]), 1);
}

// T10.2 and T10.3. "What a crash would leave" is every way a crash can leave it.
#[test]
fn a_split_through_a_handle_that_is_dropped_at_once_is_durable_when_the_store_is_at_rest() {
    let told = following();
    let case = "a split";
    let rig = Rig::new(&told);
    let (handle, _) = rig.store.open_region(hello_of(&told, 0, 1)).unwrap();
    let stays = ChunkPos::new(7, -3);
    let goes = [ChunkPos::new(20, 4), ChunkPos::new(21, 5)];
    let wanted = [stays, goes[0], goes[1]];
    assert_eq!(
        claimed(&handle, &wanted, case),
        (wanted.to_vec(), Vec::new())
    );
    // A checkpoint that covers every commit of the region, which a split needs.
    log(&handle, 1, &[]);
    handle.request(StoreRequest::Checkpoint {
        tick: 1,
        state: whole("state", 1),
    });
    let handle = flushed(handle);

    handle.request(StoreRequest::SplitCommit {
        tick: 2,
        state: whole("rest", 2),
        part: SplitPart {
            chunks: goes.to_vec(),
            state: whole("part", 2),
        },
        as_epoch: 1,
        region: RegionId(1),
    });
    drop(handle);
    at_rest(&rig.store).unwrap();
    let done = rig.disk.operations();

    let info = |region, chunks: &[ChunkPos]| RegionInfo {
        region: RegionId(region),
        epoch: 0,
        bounds: around(chunks),
        pinned: Vec::new(),
    };
    let expected = RegionList {
        home: RegionId(0),
        regions: vec![info(0, &[HOME, stays]), info(1, &goes)],
        absorbed: Vec::new(),
        next: RegionId(2),
    };
    let state = |name: &str| {
        Some(TickState {
            tick: 2,
            state: whole(name, 2),
        })
    };
    for survival in SURVIVALS {
        let (list, restored) = left_by(&rig.disk, survival, &told, 2);
        assert_eq!(list, expected, "{survival:?}");
        assert_eq!(
            restored[1].held,
            [(goes[0], 2), (goes[1], 2)],
            "{survival:?}"
        );
        assert_eq!(restored[1].state, state("part"), "{survival:?}");
        assert_eq!(restored[1].entity_ids, NO_ENTITY_IDS, "{survival:?}");
        assert_eq!(restored[0].held, [(HOME, 0), (stays, 0)], "{survival:?}");
        assert_eq!(restored[0].state, state("rest"), "{survival:?}");
    }

    // T10.3.
    assert_eq!(rig.ended(), done);
}

// T10.4.
#[test]
fn the_answers_to_a_claim_and_a_flush_are_out_when_the_store_is_at_rest() {
    let told = following();
    let store = Store::memory_divided(generator(), told.clone()).unwrap();
    let (handle, _) = store.open_region(hello_of(&told, 0, 1)).unwrap();
    let chunks = vec![ChunkPos::new(7, -3), ChunkPos::new(7, -2)];
    handle.request(StoreRequest::Claim {
        chunks: chunks.clone(),
    });
    handle.request(StoreRequest::Flush);
    at_rest(&store).unwrap();

    // Without anything more being waited for.
    let claimed = StoreReply::Claimed {
        granted: chunks,
        foreign: Vec::new(),
    };
    assert_eq!(handle.try_reply(), Some(claimed));
    assert_eq!(handle.try_reply(), Some(StoreReply::Flushed));
    assert_eq!(handle.try_reply(), None);

    // And an answer that the thread for chunks gives.
    handle.request(StoreRequest::Load { position: HOME });
    at_rest(&store).unwrap();
    let loaded = StoreReply::Loaded {
        position: HOME,
        chunk: generator().generate(HOME),
    };
    assert!(handle.try_reply() == Some(loaded));
    assert_eq!(handle.try_reply(), None);
}

// T10.5, and of section 5.5: "Dropping a handle that asked for nothing since writes
// nothing."
#[test]
fn the_store_at_rest_closes_nothing_and_serves_on() {
    let told = pinned_at(&[CUT]);
    let case = "served on";
    let rig = Rig::new(&told);
    let (asking, _) = rig.store.open_region(hello_of(&told, 0, 1)).unwrap();
    let (other, _) = rig.store.open_region(hello_of(&told, 1, 1)).unwrap();
    log(&asking, 1, &[]);
    at_rest(&rig.store).unwrap();

    log(&other, 1, &[]);
    confirmed(&other, 1, case);
    let other = flushed(other);
    assert!(!other.is_lost() && !asking.is_lost());
    at_rest(&rig.store).unwrap();
    at_rest(&rig.store.clone()).unwrap();

    // The handle that asked before is served on as well.
    log(&asking, 2, &[]);
    confirmed(&asking, 2, case);
    at_rest(&rig.store).unwrap();

    // What both asked for, before the first rest and after it, is what a crash would
    // leave.
    let (_, restored) = left_by(&rig.disk, Survival::Nothing, &told, 2);
    assert_eq!(
        (ticks(&restored[0]), ticks(&restored[1])),
        (vec![1, 2], vec![1])
    );

    let done = rig.disk.operations();
    drop((asking, other));
    assert_eq!(rig.ended(), done);
}

/// Two pinned regions on a disk with switches, each opened with epoch 1, of which
/// region 0 has a commit of tick 1 that is confirmed; and nothing of the log is left
/// to sync.
fn two_on_switches(told: &Division) -> (Store, Arc<Switched>, [StoreHandle; 2]) {
    let (store, disk) = switched_for(told);
    let (one, _) = store.open_region(hello_of(told, 0, 1)).unwrap();
    let (other, _) = store.open_region(hello_of(told, 1, 1)).unwrap();
    log(&one, 1, &[]);
    confirmed(&one, 1, "before the disk fails");
    one.flush();
    other.flush();
    (store, disk, [one, other])
}

/// What region 0 has committed in what a crash would leave of `disk`.
fn committed_on(disk: &MemoryDisk, told: &Division, survival: Survival) -> Vec<u64> {
    let left = Arc::new(disk.crashed(survival));
    let store = store_on(&left, told).unwrap_or_else(|error| panic!("{survival:?}: {error}"));
    let (_, restored) = store
        .open_region(hello_of(told, 0, 2))
        .unwrap_or_else(|error| panic!("{survival:?}: {error}"));
    assert_eq!(restored.state, None, "{survival:?}");
    ticks(&restored)
}

// T10.6, the first half: "on a disk that fails the sync of a group once: `flush`
// returns `Ok`, every handle is lost, and what the group wrote is not in what a crash
// would leave". The barrier is asked for once behind the sync that fails and once
// while the commit thread is in it.
#[test]
fn after_a_sync_of_a_group_that_failed_once_the_store_is_at_rest_with_every_handle_lost() {
    use std::sync::atomic::Ordering::SeqCst;
    let told = pinned_at(&[CUT]);
    for asked_in_the_sync in [false, true] {
        let case = format!("asked in the sync: {asked_in_the_sync}");
        let (store, disk, [one, other]) = two_on_switches(&told);
        disk.failing_syncs.store(true, SeqCst);
        disk.holding_syncs.store(true, SeqCst);
        log(&one, 2, &[]);
        // The sync of the group is there, and is going to fail; the next one works.
        disk.held.wait();
        disk.failing_syncs.store(false, SeqCst);
        let barrier = asked_in_the_sync.then(|| store.barrier());
        disk.held.wait();
        let rested = match barrier {
            Some(barrier) => barrier
                .recv_timeout(PATIENCE)
                .expect("the barrier is answered"),
            None => at_rest(&store),
        };
        assert!(rested.is_ok(), "{case}: {rested:?}");
        assert!(one.is_lost() && other.is_lost(), "{case}");
        // The commit was never confirmed.
        let answers: Vec<StoreReply> = one.replies.try_iter().collect();
        assert_eq!(answers, Vec::new(), "{case}");
        for survival in SURVIVALS {
            let left = committed_on(&disk.disk, &told, survival);
            assert_eq!(left, [1], "{case}, {survival:?}");
        }
        // And the store serves again.
        assert!(store.regions().is_ok(), "{case}");
        assert!(at_rest(&store).is_ok(), "{case}");
    }
}

// T10.6, the second half: "on a disk that fails everything from that sync on: `flush`
// returns `StoreError::Io`, as `regions` does, and returns". The disk with switches
// fails every write to the log and every sync of it from that sync on. And section
// 5.5 has the error "for as long as what a failed write left in the log is not
// durably gone": once the disk works again, the store is at rest without it.
#[test]
fn on_a_disk_that_fails_from_a_sync_on_the_store_is_at_rest_and_says_that_its_log_is_not() {
    use std::sync::atomic::Ordering::SeqCst;
    let told = pinned_at(&[CUT]);
    let (store, disk, [one, other]) = two_on_switches(&told);
    disk.failing_syncs.store(true, SeqCst);
    disk.holding_syncs.store(true, SeqCst);
    log(&one, 2, &[]);
    disk.held.wait();
    disk.failing_appends.store(true, SeqCst);
    disk.held.wait();

    for attempt in 0..2 {
        let rested = at_rest(&store);
        assert!(
            matches!(rested, Err(StoreError::Io(_))),
            "{attempt}: {rested:?}"
        );
        let listed = store.regions();
        assert!(
            matches!(listed, Err(StoreError::Io(_))),
            "{attempt}: {listed:?}"
        );
    }
    assert!(one.is_lost() && other.is_lost());

    disk.failing_syncs.store(false, SeqCst);
    disk.failing_appends.store(false, SeqCst);
    let rested = at_rest(&store);
    assert!(rested.is_ok(), "{rested:?}");
    assert!(store.regions().is_ok());
    for survival in SURVIVALS {
        assert_eq!(
            committed_on(&disk.disk, &told, survival),
            [1],
            "{survival:?}"
        );
    }
}

/// What the store of [`rests_on`] had done when it was at rest.
struct Rested {
    rested: Result<(), StoreError>,
    /// Whether the list could be read right afterwards.
    listed: Result<(), StoreError>,
    /// The ticks whose commits were confirmed.
    confirmed: Vec<u64>,
    /// Whether the claim was answered.
    claimed: bool,
    /// Whether the handle was lost when the store was at rest; `None` if the region
    /// could not be opened, so that nothing was asked.
    lost: Option<bool>,
    /// Whether the store wrote or synced anything after it was at rest and had made
    /// the list.
    quiet: bool,
}

/// The chunk beside the home chunk that the region of [`rests_on`] claims.
const BESIDE: ChunkPos = ChunkPos::new(HOME.x + 1, HOME.z);

/// What the region of [`rests_on`] builds in the home chunk in its first tick.
fn built_at_home() -> (Built, (i32, i32, i32, BlockState)) {
    let mut built = Built::default();
    let change = built.set(HOME, (1, 100, 1), blocks::GLASS);
    (built, change)
}

/// Starts a world without pins on `disk` and asks of it, through one handle and
/// without waiting for an answer: a commit that builds, a save of what it built, a
/// checkpoint of it, a claim and a second commit. Then `Store::flush`. `None` if the
/// store could not be started.
fn rests_on(disk: Arc<MemoryDisk>) -> Option<Rested> {
    let told = following();
    let (built, change) = built_at_home();
    let rig = Rig::on(disk, &told).ok()?;
    let handle = rig.store.open_region(hello_of(&told, 0, 1)).ok();
    let handle = handle.map(|(handle, _)| handle);
    if let Some(handle) = &handle {
        log(handle, 1, &[change]);
        save(handle, &built, HOME, 1);
        handle.request(StoreRequest::Checkpoint {
            tick: 1,
            state: whole("state", 1),
        });
        handle.request(StoreRequest::Claim {
            chunks: vec![BESIDE],
        });
        log(handle, 2, &[]);
    }
    let rested = at_rest(&rig.store);
    let lost = handle.as_ref().map(StoreHandle::is_lost);
    // What was sent to the handle, lost since or not.
    let answers: Vec<StoreReply> = handle
        .iter()
        .flat_map(|handle| handle.replies.try_iter())
        .collect();
    let listed = rig.store.regions().map(|_| ());
    let done = rig.disk.operations();
    drop(handle);
    let quiet = rig.ended() == done;
    let confirmed = answers.iter().filter_map(|answer| match answer {
        StoreReply::Committed { tick } => Some(*tick),
        _ => None,
    });
    Some(Rested {
        rested,
        listed,
        confirmed: confirmed.collect(),
        claimed: answers
            .iter()
            .any(|answer| matches!(answer, StoreReply::Claimed { .. })),
        lost,
        quiet,
    })
}

// T10.6, wherever the disk fails. The scenario names "the sync of a group" and
// "everything from that sync on"; this tries every write and sync of a store's short
// life in its place, failing once, twice, or from there on, and holds the store to
// what section 5.5 says of `flush` whatever failed:
//
// - it returns, with `Ok` or with `StoreError::Io`, and on a disk that goes on
//   failing with what `regions` returns;
// - when it returns `Ok`, each request "has been done, or will never be", and every
//   answer is out. So what a crash would leave has every commit that was confirmed
//   and the grant of the claim if it was answered; it is the same however the crash
//   leaves the disk, as nothing is left that is neither done nor never to be; what a
//   commit built that is left is in its chunk; and a handle that is not lost was
//   answered everything;
// - and a store that is at rest and then let go of writes nothing more.
//
// A handle that is lost need not have been answered for all that is left. When the
// thread for chunks fails at the save or the checkpoint, it loses the handle, and the
// commit thread may have taken the claim and the second commit before it hears of
// that: they are made durable and not answered, as a lost handle is answered nothing
// more. The record says "every answer the store owes" and not whether such an answer
// is owed; it is read as not owed, as what was not answered may be there or not
// after any loss of a handle (ADR-0011, section 4.3).
#[test]
fn wherever_the_disk_fails_the_store_comes_to_rest_and_has_all_that_it_answered() {
    let told = following();
    let (built, _) = built_at_home();
    let operations = {
        let disk = Arc::new(MemoryDisk::default());
        let whole = rests_on(disk.clone()).expect("a new world on a disk that works");
        assert!(whole.rested.is_ok() && whole.listed.is_ok() && whole.quiet);
        assert_eq!((whole.confirmed, whole.claimed), (vec![1, 2], true));
        disk.operations()
    };
    for n in 1..=operations + 1 {
        for fault in [Fault::Fail(n), Fault::Fails(n, 2), Fault::Stop(n)] {
            let case = format!("{fault:?}");
            let disk = Arc::new(MemoryDisk::failing(fault));
            let Some(was) = rests_on(disk.clone()) else {
                continue;
            };
            let io = |result: &Result<(), StoreError>| {
                assert!(
                    matches!(result, Ok(()) | Err(StoreError::Io(_))),
                    "{case}: {result:?}"
                );
                result.is_ok()
            };
            let (rested, listed) = (io(&was.rested), io(&was.listed));
            if matches!(fault, Fault::Stop(_)) {
                assert_eq!(rested, listed, "{case}: `flush` and `regions` differ");
            }
            if !rested {
                continue;
            }
            assert!(was.quiet, "{case}: the store wrote after it was at rest");
            let answered = (was.confirmed.clone(), was.claimed);
            match was.lost {
                // Nothing was asked: the hello failed.
                None => assert_eq!(answered, (Vec::new(), false), "{case}"),
                Some(false) => assert_eq!(answered, (vec![1, 2], true), "{case}"),
                Some(true) => {}
            }
            // The commits and whether the chunk is granted, as the first way a crash
            // can leave the disk has them.
            let mut left_with = None;
            for survival in SURVIVALS {
                let case = format!("{case}, {survival:?}");
                let left = Arc::new(disk.crashed(survival));
                let store =
                    store_on(&left, &told).unwrap_or_else(|error| panic!("{case}: {error}"));
                let (handle, restored) = store
                    .open_region(hello_of(&told, 0, 2))
                    .unwrap_or_else(|error| panic!("{case}: {error}"));
                let state = restored.state.as_ref().map(|state| state.tick);
                let there = |tick: &u64| {
                    state.is_some_and(|state| state >= *tick) || ticks(&restored).contains(tick)
                };
                let commits: Vec<u64> = [1, 2].into_iter().filter(there).collect();
                let granted = restored.held.iter().any(|(chunk, _)| *chunk == BESIDE);
                assert!(
                    was.confirmed.iter().all(|tick| commits.contains(tick)),
                    "{case}: confirmed {:?}, left {commits:?}",
                    was.confirmed
                );
                assert!(granted || !was.claimed, "{case}: the claim");
                if was.lost != Some(true) {
                    assert_eq!((&commits, granted), (&answered.0, answered.1), "{case}");
                }
                if commits.contains(&1) {
                    as_built(&handle, &built, HOME, &case);
                }
                let left_with = left_with.get_or_insert((commits.clone(), granted));
                assert_eq!(*left_with, (commits, granted), "{case}");
            }
        }
    }
}

// T8: "A hello is a region and an epoch; one for a region the table does not have,
// for one that was absorbed, and with a lower epoch is refused as today." The world
// of stripes has all three: its table has no region 4 or beyond, region 3 was
// absorbed by region 0, and regions 0, 1 and 2 were opened with the epochs 3, 4 and
// 5. "As today" is read as: with the error that names what is wrong, with nothing
// written for it, the same over a connection, and without taking the region from
// whoever has it.
#[test]
fn a_hello_is_a_region_and_an_epoch_and_is_refused_for_what_it_was_refused_for_before() {
    // The two are all a hello has: this would not compile if it named anything more.
    let RegionHello { region, epoch } = hello_of(&stripes(), 1, 4);
    assert_eq!((region, epoch), (RegionId(1), 4));
    let hello = |region: u32, epoch: u64| RegionHello {
        region: RegionId(region),
        epoch,
    };

    let (world, before) = world_of_stripes();
    let told = stripes();
    let left = Noting::of(world.crashed(Survival::Nothing));
    let store = left.started(&told).unwrap();
    let list = store.regions().unwrap();
    let written = left.changed();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let server = serve(store.clone(), listener).unwrap();
    let address = server.local_addr().to_string();
    // What a hello is answered in the store's process, and over a connection.
    let both = |region: u32, epoch: u64| {
        let here = store.open_region(hello(region, epoch)).err();
        let there = StoreHandle::connect(&address, hello(region, epoch)).err();
        let case = format!("region {region} with epoch {epoch}: {here:?}, {there:?}");
        (here.expect(&case), there.expect(&case), case)
    };

    // A region the table does not have. Over a connection that is said in words.
    for region in [4, 5, u32::MAX] {
        for epoch in [0, 1, 1000] {
            let (here, there, case) = both(region, epoch);
            assert!(
                matches!(&here, StoreError::UnknownRegion { region: unknown } if *unknown == RegionId(region)),
                "{case}"
            );
            assert!(
                matches!(&there, StoreError::Refused(reason) if *reason == here.to_string()),
                "{case}"
            );
        }
    }
    // One that was absorbed, whatever the epoch: it is told which region it went into.
    for epoch in [0, 6, 7, 1000] {
        let (here, there, case) = both(3, epoch);
        for refused in [here, there] {
            assert!(
                matches!(
                    refused,
                    StoreError::Absorbed {
                        region: RegionId(3),
                        into: RegionId(0)
                    }
                ),
                "{case}"
            );
        }
    }
    // A lower epoch than the region was last opened with: it is told the epoch.
    for (region, seen) in [(0, 3), (1, 4), (2, 5)] {
        for offered in [0, seen - 1] {
            let (here, there, case) = both(region, offered);
            for refused in [here, there] {
                assert!(
                    matches!(
                        refused,
                        StoreError::EpochRefused { region: of, offered: with, seen: last }
                            if (of, with, last) == (RegionId(region), offered, seen)
                    ),
                    "{case}"
                );
            }
        }
    }
    // Nothing was written for any of them, and the regions are what they were.
    assert_eq!(left.changed(), written);
    assert_eq!(store.regions().unwrap(), list);

    // The epoch a region was last opened with is taken, and restores what it had.
    as_lived_on_stripes(&store, &told, &before, "after the refusals");

    // A hello that is refused takes no region from whoever has it.
    let (owner, _) = store.open_region(hello(1, 9)).unwrap();
    let (here, there, case) = both(1, 8);
    for refused in [here, there] {
        assert!(
            matches!(refused, StoreError::EpochRefused { seen: 9, .. }),
            "{case}"
        );
    }
    let (_, _, case) = both(4, 9);
    owner.request(StoreRequest::Flush);
    assert_eq!(next(&owner, &case), StoreReply::Flushed, "{case}");
    assert!(!owner.is_lost(), "{case}");
}
