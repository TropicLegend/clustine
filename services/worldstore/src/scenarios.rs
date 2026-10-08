//! The scenarios of section 9 of `docs/adr/0011-the-world-store-and-regions.md`, F4 and
//! 1 to 22, written from that record by someone who did not write the store, and more
//! tests where a mistake in the order of things is likeliest: returns that are called
//! off and made again, a split or a merge of a chunk whose return is under way, groups
//! that failed and what a crash keeps of them, checkpoints that are under way when a
//! merge or a split is written, the region file of a part, and a world made over.
//!
//! What a test expects is what the record says. The thread for chunks is held with a
//! [`Gate`], which stops it before a save, before it makes saves durable, or between
//! that and the message that says so; the log is made to fail with the switches of
//! `tests.rs` or with a [`Picky`] disk; and the store is killed by starting another on
//! what a crash leaves of the simulated disk.

use std::sync::atomic::AtomicUsize;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use clustine_data::{BlockState, blocks};
use clustine_format::{LogRecord, RegionFile, TableFile, read_log};
use clustine_rpc::{ChunkBox, Decline, SplitPart};
use clustine_world::{Chunk, ChunkArea, ChunkPos, EntityId, EntityIds};

use super::*;
use crate::disk::{MemoryDisk, Survival, replace};
use crate::regions::{SURVIVALS, claim, gap, give_back, hello_of, store_on};
use crate::tests::{
    Switched, any_reply, committed, delta, division, generator, hello, load, log, open, open_later,
    reply, save, switched_for,
};

const ROOT: &str = "/world";

/// The chunk players enter both worlds of these tests in.
const ORIGIN: ChunkPos = ChunkPos::new(0, 0);

/// Chunks in the gap of [`gap`], which nobody holds until they are claimed.
const FREE: ChunkPos = ChunkPos::new(5, 5);
const FREE_TOO: ChunkPos = ChunkPos::new(6, 5);
const FREE_THREE: ChunkPos = ChunkPos::new(9, -2);

/// Chunks of the western area of both worlds, and of the eastern area of the world
/// with a gap.
const WEST: ChunkPos = ChunkPos::new(-3, 0);
const WEST_TOO: ChunkPos = ChunkPos::new(-3, 1);
const EAST: ChunkPos = ChunkPos::new(20, 0);

/// A chunk of the eastern area that no test looks at: a save of it is what the thread
/// for chunks is held at when it is to do nothing at all.
const BUSY_EAST: ChunkPos = ChunkPos::new(1000, 1000);

/// The areas regions 0 and 1 of the world with a gap are pinned to.
fn west_area() -> ChunkArea {
    gap().pinned[0]
}

fn east_area() -> ChunkArea {
    gap().pinned[1]
}

/// A block of a chunk, by where it is inside the chunk, and what a test sets it to.
/// The flat world has air there.
type Mark = (usize, i32, usize, BlockState);

const GLASS: Mark = (7, 100, 7, blocks::GLASS);
const STONE: Mark = (7, 100, 7, blocks::STONE);
const OTHER: Mark = (2, 110, 9, blocks::STONE);
const THIRD: Mark = (12, 120, 3, blocks::GLASS);

/// The change of a commit that sets `mark` in the chunk at `position`.
fn change(position: ChunkPos, mark: Mark) -> (i32, i32, i32, BlockState) {
    let (x, y, z, state) = mark;
    (
        position.x * 16 + x as i32,
        y,
        position.z * 16 + z as i32,
        state,
    )
}

/// The chunk at `position` as the generator makes it, with `marks` set.
fn built(position: ChunkPos, marks: &[Mark]) -> Chunk {
    let mut chunk = generator().generate(position);
    for (x, y, z, state) in marks {
        chunk.set(*x, *y, *z, *state);
    }
    chunk
}

/// A whole state as a test makes it up.
fn whole(name: &str, tick: u64) -> Vec<u8> {
    format!("{name} {tick}").into_bytes()
}

/// The tick and the bytes of the whole state a region is restored with.
fn state_of(restored: &Restored) -> Option<(u64, Vec<u8>)> {
    let state = restored.state.as_ref();
    state.map(|state| (state.tick, state.state.clone()))
}

/// The ticks of the commits a region is restored with, each of which has to have the
/// change of state its commit had.
fn ticks(restored: &Restored) -> Vec<u64> {
    for restored in &restored.deltas {
        assert_eq!(restored.state, delta(restored.tick), "{restored:?}");
    }
    restored.deltas.iter().map(|delta| delta.tick).collect()
}

/// Areas in an order of their own, to compare what a region is pinned to with: the
/// record does not say in which order a region that absorbed another has its areas.
fn areas(areas: &[ChunkArea]) -> Vec<(Option<i32>, Option<i32>)> {
    let mut areas: Vec<_> = areas.iter().map(|area| (area.min_x, area.max_x)).collect();
    areas.sort();
    areas
}

/// A store for the world with a gap on a simulated disk of its own.
fn gap_store() -> (Store, Arc<MemoryDisk>) {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    (store, disk)
}

/// Opens a region of a world divided as `division` says.
fn opened(store: &Store, division: &Division, region: u32, epoch: u64) -> (StoreHandle, Restored) {
    store
        .open_region(hello_of(division, region, epoch))
        .unwrap_or_else(|error| panic!("region {region} with epoch {epoch}: {error}"))
}

/// Opens a region of the world with a gap.
fn opened_gap(store: &Store, region: u32, epoch: u64) -> (StoreHandle, Restored) {
    opened(store, &gap(), region, epoch)
}

/// Opens a region of the world with a gap while the thread for chunks is held. The
/// commit thread has to answer such a hello by itself; a test that waited for the
/// answer for good would never let the thread for chunks go.
fn opened_while_held(store: &Store, region: u32, epoch: u64) -> (StoreHandle, Restored) {
    let waiting = open_later(store, hello_of(&gap(), region, epoch));
    let answer = waiting
        .recv_timeout(Duration::from_secs(30))
        .unwrap_or_else(|_| panic!("the hello for region {region} was not answered"));
    let (opened, restored) =
        answer.unwrap_or_else(|error| panic!("region {region} with epoch {epoch}: {error}"));
    (StoreHandle::local(opened, store.messages.clone()), restored)
}

/// What a region of the world with a gap was granted, as the store says when the
/// region is opened with `epoch`. The region is given up again.
fn held(store: &Store, region: u32, epoch: u64) -> Vec<(ChunkPos, u64)> {
    opened_gap(store, region, epoch).1.held
}

/// Who holds the chunk, as the store tells the region `own` of `handle` that asks to
/// load it. A region that does not hold the chunk is told by the commit thread, also
/// while the thread for chunks is held.
fn holder(handle: &StoreHandle, own: u32, position: ChunkPos) -> Option<RegionId> {
    handle.request(StoreRequest::Load { position });
    match reply(handle) {
        StoreReply::Loaded { position: of, .. } if of == position => Some(RegionId(own)),
        StoreReply::NotHeld {
            position: of,
            holder,
        } if of == position => holder,
        other => panic!("expected the chunk or who holds it, got {other:?}"),
    }
}

fn not_held(position: ChunkPos, holder: Option<u32>) -> StoreReply {
    StoreReply::NotHeld {
        position,
        holder: holder.map(RegionId),
    }
}

fn checkpoint(handle: &StoreHandle, tick: u64, name: &str) {
    handle.request(StoreRequest::Checkpoint {
        tick,
        state: whole(name, tick),
    });
}

/// Has the region of `handle` absorb `absorbed` with the state "merged", and waits for
/// the answer.
fn merge(handle: &StoreHandle, absorbed: u32, absorbed_epoch: u64, tick: u64) -> StoreReply {
    handle.request(StoreRequest::AbsorbCommit {
        absorbed: RegionId(absorbed),
        absorbed_epoch,
        tick,
        state: whole("merged", tick),
    });
    reply(handle)
}

/// The request to split `chunks` off, with the states "rest" and "part".
fn split_of(tick: u64, chunks: &[ChunkPos], as_epoch: u64) -> StoreRequest {
    StoreRequest::SplitCommit {
        tick,
        state: whole("rest", tick),
        part: SplitPart {
            chunks: chunks.to_vec(),
            state: whole("part", tick),
        },
        as_epoch,
    }
}

/// Splits `chunks` off the region of `handle`, and waits for the answer.
fn split(handle: &StoreHandle, tick: u64, chunks: &[ChunkPos], as_epoch: u64) -> StoreReply {
    handle.request(split_of(tick, chunks, as_epoch));
    reply(handle)
}

fn declined(reason: Decline) -> StoreReply {
    StoreReply::Declined { reason }
}

fn parted(region: u32) -> StoreReply {
    StoreReply::Split {
        region: RegionId(region),
    }
}

/// The block of entity ids of a region that a split made.
const NO_ENTITY_IDS: EntityIds = EntityIds {
    first: EntityId(0),
    end: EntityId(0),
};

/// The numbers of the segments of the log on `disk`.
fn segments(disk: &dyn Disk) -> Vec<u64> {
    let names = disk.list(Path::new("/world/log")).unwrap();
    let mut numbers: Vec<u64> = names
        .iter()
        .map(|name| name.strip_suffix(".wal").unwrap().parse().unwrap())
        .collect();
    numbers.sort_unstable();
    numbers
}

fn segment_path(number: u64) -> PathBuf {
    PathBuf::from(format!("/world/log/{number:020}.wal"))
}

/// The records of the log on `disk`, segment by segment.
fn records(disk: &dyn Disk) -> Vec<LogRecord> {
    segments(disk)
        .into_iter()
        .flat_map(|number| {
            let bytes = disk.read(&segment_path(number)).unwrap().unwrap();
            read_log(&bytes).unwrap().0
        })
        .collect()
}

/// The table file of the world on `disk`.
fn table_file(disk: &dyn Disk) -> TableFile {
    let bytes = disk.read(Path::new("/world/regions/table")).unwrap();
    TableFile::decode(&bytes.expect("the world has a table")).unwrap()
}

fn exists(disk: &dyn Disk, path: &str) -> bool {
    disk.exists(Path::new(path)).unwrap()
}

/// Writes `contents` to the file at `path` of `disk`, for good.
fn put(disk: &MemoryDisk, path: &Path, contents: &[u8]) {
    replace(disk, path, contents).unwrap();
    disk.sync_directory(path.parent().unwrap()).unwrap();
}

/// Every answer the handle still gets, up to the store letting go of it, which it has
/// to do.
fn answers_until_lost(handle: &StoreHandle) -> Vec<StoreReply> {
    let mut answers = Vec::new();
    loop {
        match handle.replies.recv_timeout(Duration::from_secs(60)) {
            Ok(reply) => answers.push(reply),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("the store did not let go of the handle; it was answered {answers:?}")
            }
        }
    }
    assert!(handle.is_lost());
    answers
}

/// What a store says of its world: the list of regions, without their epochs, which
/// are the tests' own, and what each living region is restored with.
#[derive(Debug, PartialEq)]
struct World {
    list: RegionList,
    restored: Vec<(RegionId, Restored)>,
}

/// Reads the list of `store` and opens every living region with `epoch`.
fn world(store: &Store, division: &Division, epoch: u64, case: &str) -> World {
    let mut list = store
        .regions()
        .unwrap_or_else(|error| panic!("{case}: {error}"));
    let mut restored = Vec::new();
    for info in &mut list.regions {
        info.epoch = 0;
        let (handle, region) = store
            .open_region(hello_of(division, info.region.0, epoch))
            .unwrap_or_else(|error| panic!("{case}: region {}: {error}", info.region));
        // What it had to replay is in the stored chunks before the next one is opened.
        handle.flush();
        restored.push((info.region, region));
    }
    World { list, restored }
}

/// Starts a store on what each kind of crash leaves of `disk` and has `check` look at
/// it.
fn after_every_crash(
    disk: &MemoryDisk,
    division: &Division,
    point: &str,
    check: impl Fn(&Store, &str),
) {
    for survival in SURVIVALS {
        let case = format!("{point}, {survival:?}");
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, division).unwrap_or_else(|error| panic!("{case}: {error}"));
        check(&store, &case);
    }
}

/// "The store starts, and starts again on what that start left with the same result"
/// (section 4.3), for every kind of crash of `disk`.
fn starts_again_alike(disk: &MemoryDisk, division: &Division, point: &str) {
    for survival in SURVIVALS {
        let case = format!("{point}, {survival:?}");
        let left = Arc::new(disk.crashed(survival));
        let first = {
            let store = store_on(&left, division).unwrap_or_else(|error| panic!("{case}: {error}"));
            world(&store, division, 1000, &case)
        };
        let case = format!("{case}, started again");
        let again = Arc::new(left.crashed(Survival::Nothing));
        let store = store_on(&again, division).unwrap_or_else(|error| panic!("{case}: {error}"));
        assert_eq!(world(&store, division, 1001, &case), first, "{case}");
    }
}

/// Where the thread for chunks is made to stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// Before it saves this chunk.
    BeforeSave(ChunkPos),
    /// Before it makes the saves so far durable, which is the first thing it does for
    /// a return and for a checkpoint.
    BeforeSync,
    /// When it has made them durable and has not yet told the commit thread: between
    /// a return's sync and its message.
    AfterSync,
}

/// Holds the thread for chunks where a test says and lets it go on when the test says.
/// The test waits for the thread to be there, not for time to pass.
#[derive(Default)]
struct Gate {
    state: Mutex<Halt>,
    changed: Condvar,
}

#[derive(Default)]
struct Halt {
    /// Where the thread is to stop when it next gets there.
    at: Option<Stop>,
    /// Whether it has stopped and waits to be let go.
    there: bool,
}

impl Gate {
    /// Has the thread stop the next time it gets to `stop`.
    fn hold_at(&self, stop: Stop) {
        let mut state = self.state.lock().unwrap();
        assert_eq!(state.at, None, "the thread never got to the stop before");
        state.at = Some(stop);
    }

    /// Returns when the thread has stopped.
    fn wait_until_there(&self) {
        let state = self.state.lock().unwrap();
        let (state, patience) = self
            .changed
            .wait_timeout_while(state, Duration::from_secs(60), |state| !state.there)
            .unwrap();
        assert!(
            !patience.timed_out(),
            "the thread for chunks did not get to {:?}",
            state.at
        );
    }

    fn let_go(&self) {
        let mut state = self.state.lock().unwrap();
        assert!(state.there, "the thread for chunks is not held");
        state.there = false;
        self.changed.notify_all();
    }

    /// Lets the thread go on from where it is held as far as `stop`.
    fn on_to(&self, stop: Stop) {
        self.hold_at(stop);
        self.let_go();
        self.wait_until_there();
    }

    /// For the thread for chunks: stops here if this is where it is to stop.
    fn pass(&self, stop: Stop) {
        let mut state = self.state.lock().unwrap();
        if state.at != Some(stop) {
            return;
        }
        state.at = None;
        state.there = true;
        self.changed.notify_all();
        while state.there {
            state = self.changed.wait(state).unwrap();
        }
    }
}

/// Keeps chunks as `chunks` does, and stops where its [`Gate`] says.
struct Stopping<C> {
    chunks: C,
    gate: Arc<Gate>,
}

impl<C: Chunks> Chunks for Stopping<C> {
    fn load(&mut self, position: ChunkPos) -> Result<Option<Chunk>, StoreError> {
        self.chunks.load(position)
    }

    fn save(&mut self, position: ChunkPos, tick: u64, chunk: &Chunk) -> Result<(), StoreError> {
        self.gate.pass(Stop::BeforeSave(position));
        self.chunks.save(position, tick, chunk)
    }

    fn sync(&mut self) -> Result<(), StoreError> {
        self.gate.pass(Stop::BeforeSync);
        let done = self.chunks.sync();
        self.gate.pass(Stop::AfterSync);
        done
    }
}

/// A store on `disk`, with its chunks in files there, whose thread for chunks stops
/// where the gate says.
fn gated<D: Disk + 'static>(disk: &Arc<D>, division: &Division) -> (Store, Arc<Gate>) {
    let gate = Arc::new(Gate::default());
    let root = Path::new(ROOT);
    let files: Arc<dyn Disk> = disk.clone();
    let chunks = Stopping {
        chunks: FileChunks::new(Arc::clone(&files), root),
        gate: Arc::clone(&gate),
    };
    let store = start(files, root, Box::new(chunks), generator(), division).unwrap();
    (store, gate)
}

/// A store for the world with a gap on a disk of its own, with a gate.
fn gated_gap() -> (Store, Arc<Gate>, Arc<MemoryDisk>) {
    let disk = Arc::new(MemoryDisk::default());
    let (store, gate) = gated(&disk, &gap());
    (store, gate, disk)
}

/// What a [`Picky`] disk can be told to fail at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Write,
    Append,
    Truncate,
    Sync,
    SyncDirectory,
    Rename,
}

/// A disk in memory that fails what it is told to, for the files it is told: an
/// operation on a path that ends as a rule says fails until the disk is mended. A
/// rename is told by the name it gives. `tests.rs` has switches for the log's appends
/// and syncs; this can also fail cutting the log back, and the writing of one file.
#[derive(Default)]
struct Picky {
    disk: MemoryDisk,
    failing: Mutex<Vec<(Op, &'static str)>>,
    failed: AtomicUsize,
}

impl Picky {
    fn fail(&self, op: Op, ending: &'static str) {
        self.failing.lock().unwrap().push((op, ending));
    }

    fn mend(&self) {
        self.failing.lock().unwrap().clear();
    }

    /// How many operations have failed so far.
    fn failures(&self) -> usize {
        self.failed.load(Ordering::SeqCst)
    }

    fn tried(&self, op: Op, path: &Path) -> io::Result<()> {
        let failing = self.failing.lock().unwrap();
        let path = path.to_string_lossy();
        let fails = |(failed, ending): &(Op, &str)| *failed == op && path.ends_with(ending);
        if failing.iter().any(fails) {
            self.failed.fetch_add(1, Ordering::SeqCst);
            return Err(io::Error::other("the disk does not do this just now"));
        }
        Ok(())
    }
}

impl Disk for Picky {
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
        self.tried(Op::Write, path)?;
        self.disk.write(path, contents)
    }
    fn append(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        if let Err(error) = self.tried(Op::Append, path) {
            // Half of it gets there, as when the disk fills up.
            self.disk.append(path, &contents[..contents.len() / 2])?;
            return Err(error);
        }
        self.disk.append(path, contents)
    }
    fn truncate(&self, path: &Path, length: u64) -> io::Result<()> {
        self.tried(Op::Truncate, path)?;
        self.disk.truncate(path, length)
    }
    fn sync(&self, path: &Path) -> io::Result<()> {
        self.tried(Op::Sync, path)?;
        self.disk.sync(path)
    }
    fn sync_directory(&self, directory: &Path) -> io::Result<()> {
        self.tried(Op::SyncDirectory, directory)?;
        self.disk.sync_directory(directory)
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.tried(Op::Rename, to)?;
        self.disk.rename(from, to)
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        self.disk.remove(path)
    }
}

/// A store for the world with a gap on a disk that fails what it is told to.
fn picky() -> (Store, Arc<Picky>) {
    let disk = Arc::new(Picky::default());
    let root = Path::new(ROOT);
    let chunks = FileChunks::new(disk.clone(), root);
    let store = start(disk.clone(), root, Box::new(chunks), generator(), &gap()).unwrap();
    (store, disk)
}

// Of a failed write or sync. F2 and F3 are asserted as the record has them by
// `what_a_failed_sync_left_in_the_log_is_durably_gone_before_anyone_is_welcomed`,
// `nobody_is_served_while_the_log_cannot_be_cut_back_for_good` and
// `hellos_fail_until_cutting_the_log_back_succeeds` in `tests.rs`, and F1 for commits
// and flushes by the two tests there that are named for it. Here are F4, F1 for a
// claim, and F3 for a segment that cannot be cut back, which those do not make happen.

/// F4: a claim in a group that failed, then the same chunk claimed by another region
/// and answered, then a crash: the store starts, and the chunk is the second region's,
/// also on a machine that lost the truncation.
#[test]
fn a_chunk_claimed_in_a_group_that_failed_is_the_next_claimants_after_any_crash() {
    let (store, disk) = switched_for(&gap());
    let (west, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 1);
    west.flush();
    east.flush();

    disk.failing_syncs.store(true, Ordering::SeqCst);
    west.request(StoreRequest::Claim { chunks: vec![FREE] });
    assert_eq!(answers_until_lost(&west), []);
    assert_eq!(answers_until_lost(&east), []);
    // Had the store died here, the claim would count or not; it was never answered,
    // and the chunk is nobody else's.
    for survival in SURVIVALS {
        let left = Arc::new(disk.disk.crashed(survival));
        let store = store_on(&left, &gap()).unwrap_or_else(|error| panic!("{survival:?}: {error}"));
        let west = held(&store, 0, 9);
        assert!(
            west.is_empty() || west == [(FREE, 0)],
            "{survival:?}: {west:?}"
        );
        assert_eq!(held(&store, 1, 9), [], "{survival:?}");
    }
    disk.failing_syncs.store(false, Ordering::SeqCst);

    let (east, restored) = opened_gap(&store, 1, 2);
    assert_eq!(restored.held, []);
    assert_eq!(claim(&east, &[FREE]), (vec![FREE], vec![]));
    after_every_crash(
        &disk.disk,
        &gap(),
        "the second claim answered",
        |store, case| {
            assert_eq!(held(store, 1, 9), [(FREE, 0)], "{case}");
            assert_eq!(held(store, 0, 9), [], "{case}");
        },
    );
    starts_again_alike(&disk.disk, &gap(), "the second claim answered");
}

/// F4 for a record that could not be written: half of the `Granted` got to the disk.
#[test]
fn a_claim_whose_record_could_not_be_written_does_not_come_back_either() {
    let (store, disk) = switched_for(&gap());
    let (west, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 1);
    west.flush();
    east.flush();

    disk.failing_appends.store(true, Ordering::SeqCst);
    west.request(StoreRequest::Claim { chunks: vec![FREE] });
    assert_eq!(answers_until_lost(&west), []);
    assert_eq!(answers_until_lost(&east), []);
    disk.failing_appends.store(false, Ordering::SeqCst);

    let (east, _) = opened_gap(&store, 1, 2);
    assert_eq!(claim(&east, &[FREE]), (vec![FREE], vec![]));
    let (west, _) = opened_gap(&store, 0, 2);
    assert_eq!(claim(&west, &[FREE]), (vec![], vec![(FREE, RegionId(1))]));
    after_every_crash(
        &disk.disk,
        &gap(),
        "the second claim answered",
        |store, case| {
            assert_eq!(held(store, 1, 9), [(FREE, 0)], "{case}");
            assert_eq!(held(store, 0, 9), [], "{case}");
        },
    );
    starts_again_alike(&disk.disk, &gap(), "the second claim answered");
}

/// F1 with claims, and F4 and scenario 3 for a group of two regions: commits, claims
/// and a flush of a group whose sync fails are not answered, both handles are lost, and
/// once the regions are opened again nothing of the group counts, whatever a crash
/// keeps: the claims are decided anew, the other way round.
#[test]
fn nothing_of_a_group_of_two_regions_that_failed_counts_whatever_a_crash_keeps() {
    let (store, disk) = switched_for(&gap());
    let (west, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 1);
    log(&west, 1, &[]);
    committed(&west, 1);
    log(&east, 1, &[]);
    committed(&east, 1);
    west.flush();
    east.flush();

    // The sync of a commit is held, so that what follows is one group.
    disk.holding_syncs.store(true, Ordering::SeqCst);
    log(&west, 2, &[change(WEST, GLASS)]);
    disk.held.wait();
    log(&east, 2, &[change(EAST, GLASS)]);
    west.request(StoreRequest::Claim { chunks: vec![FREE] });
    log(&west, 3, &[change(FREE, GLASS)]);
    east.request(StoreRequest::Claim {
        chunks: vec![FREE, FREE_TOO],
    });
    east.request(StoreRequest::Flush);
    disk.failing_syncs.store(true, Ordering::SeqCst);
    disk.held.wait();
    assert_eq!(
        answers_until_lost(&west),
        [StoreReply::Committed { tick: 2 }]
    );
    assert_eq!(answers_until_lost(&east), []);
    disk.failing_syncs.store(false, Ordering::SeqCst);

    // In the group the western region was the first to ask for the chunk. Now the
    // eastern one is.
    let (east, restored) = opened_gap(&store, 1, 2);
    assert_eq!((ticks(&restored), restored.held), (vec![1], vec![]));
    assert_eq!(claim(&east, &[FREE]), (vec![FREE], vec![]));
    let (west, restored) = opened_gap(&store, 0, 2);
    assert_eq!((ticks(&restored), restored.held), (vec![1, 2], vec![]));
    assert_eq!(
        claim(&west, &[FREE, FREE_TOO]),
        (vec![FREE_TOO], vec![(FREE, RegionId(1))])
    );
    west.flush();
    east.flush();
    after_every_crash(&disk.disk, &gap(), "claimed anew", |store, case| {
        let (west, restored) = opened_gap(store, 0, 3);
        assert_eq!(ticks(&restored), [1, 2], "{case}");
        assert_eq!(restored.held, [(FREE_TOO, 2)], "{case}");
        assert_eq!(load(&west, WEST), built(WEST, &[GLASS]), "{case}");
        let (east, restored) = opened_gap(store, 1, 3);
        assert_eq!(ticks(&restored), [1], "{case}");
        assert_eq!(restored.held, [(FREE, 1)], "{case}");
        assert_eq!(load(&east, EAST), built(EAST, &[]), "{case}");
        assert_eq!(load(&east, FREE), built(FREE, &[]), "{case}");
    });
    starts_again_alike(&disk.disk, &gap(), "claimed anew");
}

/// Section 4.1, step 2: a return whose record was in a group that failed is given
/// back, and the chunk stays its region's, also for a machine that lost the truncation.
#[test]
fn a_return_of_a_group_that_failed_is_given_back_for_good() {
    let (store, disk) = switched_for(&gap());
    let (west, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 1);
    assert_eq!(claim(&west, &[FREE]), (vec![FREE], vec![]));
    save(&west, FREE, &built(FREE, &[GLASS]));
    west.flush();
    east.flush();

    disk.failing_syncs.store(true, Ordering::SeqCst);
    give_back(&west, &[FREE]);
    west.request(StoreRequest::Flush);
    // The flush behind the return is not answered: its group failed.
    assert_eq!(answers_until_lost(&west), []);
    assert_eq!(answers_until_lost(&east), []);
    disk.failing_syncs.store(false, Ordering::SeqCst);

    let (east, _) = opened_gap(&store, 1, 2);
    assert_eq!(claim(&east, &[FREE]), (vec![], vec![(FREE, RegionId(0))]));
    let (west, restored) = opened_gap(&store, 0, 2);
    assert_eq!(restored.held, [(FREE, 0)]);
    assert_eq!(load(&west, FREE), built(FREE, &[GLASS]));
    after_every_crash(&disk.disk, &gap(), "given back", |store, case| {
        assert_eq!(held(store, 0, 9), [(FREE, 0)], "{case}");
        assert_eq!(held(store, 1, 9), [], "{case}");
    });
    starts_again_alike(&disk.disk, &gap(), "given back");
}

/// Looks at whether region 0 of the world with a gap has absorbed region 1 at `tick`
/// in the world of `store`, and sees to it that it has or has not as a whole: the
/// survivor has the merged state and both areas and the other is refused, or both are
/// regions as before.
fn merged_as_a_whole(store: &Store, tick: u64, case: &str) -> bool {
    let list = store.regions().unwrap();
    let living: Vec<u32> = list.regions.iter().map(|info| info.region.0).collect();
    let merged = !list.absorbed.is_empty();
    let (_, survivor) = opened_gap(store, 0, 50);
    if merged {
        assert_eq!(list.absorbed, [(RegionId(1), RegionId(0))], "{case}");
        assert_eq!(living, [0, 2], "{case}");
        assert_eq!(
            state_of(&survivor),
            Some((tick, whole("merged", tick))),
            "{case}"
        );
        assert_eq!(survivor.deltas, [], "{case}");
        assert_eq!(
            areas(&survivor.pinned),
            areas(&[west_area(), east_area()]),
            "{case}"
        );
        let refused = store.open_region(hello_of(&gap(), 1, 50));
        assert!(
            matches!(
                refused,
                Err(StoreError::Absorbed {
                    region: RegionId(1),
                    into: RegionId(0)
                })
            ),
            "{case}: {:?}",
            refused.err()
        );
    } else {
        assert_eq!(living, [0, 1, 2], "{case}");
        assert_eq!(survivor.state, None, "{case}");
        assert_eq!(survivor.pinned, [west_area()], "{case}");
        let (_, other) = opened_gap(store, 1, 50);
        assert_eq!(other.pinned, [east_area()], "{case}");
    }
    merged
}

/// F4 for a merge: an `Absorbed` whose sync failed, then the region that was to be
/// absorbed going on by itself, then a crash. Before the truncation is durable the
/// merge may count, as a whole; once a hello was welcomed it has not happened, also
/// for a machine that lost the truncation.
#[test]
fn a_merge_whose_sync_failed_has_not_happened_once_a_hello_is_welcomed() {
    let (store, disk) = switched_for(&gap());
    let (west, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 1);
    west.flush();
    east.flush();

    disk.failing_syncs.store(true, Ordering::SeqCst);
    west.request(StoreRequest::AbsorbCommit {
        absorbed: RegionId(1),
        absorbed_epoch: 1,
        tick: 1,
        state: whole("merged", 1),
    });
    assert_eq!(answers_until_lost(&west), []);
    assert_eq!(answers_until_lost(&east), []);
    for survival in SURVIVALS {
        let case = format!("died before the cut was durable, {survival:?}");
        let left = Arc::new(disk.disk.crashed(survival));
        let store = store_on(&left, &gap()).unwrap_or_else(|error| panic!("{case}: {error}"));
        let merged = merged_as_a_whole(&store, 1, &case);
        // A record counts if it is whole: nothing of it was durable, and all of it is
        // there where cutting it off never reached the disk.
        match survival {
            Survival::Nothing => assert!(!merged, "{case}"),
            Survival::Untruncated => assert!(merged, "{case}"),
            Survival::Torn | Survival::Everything => {}
        }
    }
    disk.failing_syncs.store(false, Ordering::SeqCst);

    // The eastern region goes on by itself.
    let (east, restored) = opened_gap(&store, 1, 2);
    assert_eq!(restored.state, None);
    log(&east, 1, &[change(EAST, GLASS)]);
    committed(&east, 1);
    assert_eq!(claim(&east, &[FREE]), (vec![FREE], vec![]));
    let (west, restored) = opened_gap(&store, 0, 2);
    assert_eq!((restored.state, restored.pinned), (None, vec![west_area()]));
    assert_eq!(claim(&west, &[FREE]), (vec![], vec![(FREE, RegionId(1))]));
    after_every_crash(&disk.disk, &gap(), "gone on by itself", |store, case| {
        assert!(!merged_as_a_whole(store, 1, case), "{case}");
        let (east, restored) = opened_gap(store, 1, 60);
        assert_eq!(ticks(&restored), [1], "{case}");
        assert_eq!(restored.held, [(FREE, 1)], "{case}");
        assert_eq!(load(&east, EAST), built(EAST, &[GLASS]), "{case}");
    });
    starts_again_alike(&disk.disk, &gap(), "gone on by itself");
}

/// The group is ended before a merge is looked at. If that fails, the merge is not
/// done either, and what was claimed in the group is not granted.
#[test]
fn a_merge_asked_for_behind_a_group_that_fails_is_not_done() {
    let (store, disk) = switched_for(&gap());
    let (west, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 1);
    west.flush();
    east.flush();

    disk.failing_syncs.store(true, Ordering::SeqCst);
    west.request(StoreRequest::Claim { chunks: vec![FREE] });
    west.request(StoreRequest::AbsorbCommit {
        absorbed: RegionId(1),
        absorbed_epoch: 1,
        tick: 1,
        state: whole("merged", 1),
    });
    assert_eq!(answers_until_lost(&west), []);
    assert_eq!(answers_until_lost(&east), []);
    disk.failing_syncs.store(false, Ordering::SeqCst);

    assert!(!merged_as_a_whole(&store, 1, "in the store that failed"));
    let (east, _) = opened_gap(&store, 1, 50);
    assert_eq!(claim(&east, &[FREE]), (vec![FREE], vec![]));
    after_every_crash(&disk.disk, &gap(), "claimed by the other", |store, case| {
        assert!(!merged_as_a_whole(store, 1, case), "{case}");
        assert_eq!(held(store, 1, 70), [(FREE, 0)], "{case}");
        assert_eq!(held(store, 0, 70), [], "{case}");
    });
}

/// F4 for a split: a `Split` whose sync failed has made no region, the chunk it named
/// is the old region's, and the next split is given the id, with its own chunks, also
/// for a machine that lost the truncation.
#[test]
fn a_split_whose_sync_failed_has_not_happened_and_the_next_one_takes_its_id() {
    let (store, disk) = switched_for(&gap());
    let (west, _) = opened_gap(&store, 0, 1);
    assert_eq!(claim(&west, &[FREE]), (vec![FREE], vec![]));
    west.flush();

    disk.failing_syncs.store(true, Ordering::SeqCst);
    west.request(split_of(1, &[FREE], 4));
    assert_eq!(answers_until_lost(&west), []);
    for survival in SURVIVALS {
        let case = format!("died before the cut was durable, {survival:?}");
        let left = Arc::new(disk.disk.crashed(survival));
        let store = store_on(&left, &gap()).unwrap_or_else(|error| panic!("{case}: {error}"));
        let list = store.regions().unwrap();
        let split = list.regions.len() == 4;
        let (_, rest) = opened_gap(&store, 0, 50);
        if split {
            let (_, part) = opened_gap(&store, 3, 50);
            assert_eq!(state_of(&part), Some((1, whole("part", 1))), "{case}");
            assert_eq!(part.held, [(FREE, 1)], "{case}");
            assert_eq!(state_of(&rest), Some((1, whole("rest", 1))), "{case}");
            assert_eq!(rest.held, [], "{case}");
        } else {
            assert_eq!(list.regions.len(), 3, "{case}");
            assert_eq!((rest.state, rest.held), (None, vec![(FREE, 0)]), "{case}");
        }
        match survival {
            Survival::Nothing => assert!(!split, "{case}"),
            Survival::Untruncated => assert!(split, "{case}"),
            Survival::Torn | Survival::Everything => {}
        }
    }
    disk.failing_syncs.store(false, Ordering::SeqCst);

    let (west, restored) = opened_gap(&store, 0, 2);
    assert_eq!((restored.state, restored.held), (None, vec![(FREE, 0)]));
    assert_eq!(store.regions().unwrap().regions.len(), 3);
    assert!(matches!(
        store.open_region(hello_of(&gap(), 3, 4)),
        Err(StoreError::UnknownRegion {
            region: RegionId(3)
        })
    ));
    // Another part than the one that failed, with another epoch.
    assert_eq!(split(&west, 1, &[WEST], 6), parted(3));
    after_every_crash(&disk.disk, &gap(), "split anew", |store, case| {
        let list = store.regions().unwrap();
        let living: Vec<u32> = list.regions.iter().map(|info| info.region.0).collect();
        assert_eq!(living, [0, 1, 2, 3], "{case}");
        let refused = store.open_region(hello_of(&gap(), 3, 5));
        assert!(
            matches!(refused, Err(StoreError::EpochRefused { seen: 6, .. })),
            "{case}: {:?}",
            refused.err()
        );
        let (_, part) = opened_gap(store, 3, 6);
        assert_eq!(state_of(&part), Some((1, whole("part", 1))), "{case}");
        assert_eq!(part.held, [(WEST, 1)], "{case}");
        let (_, rest) = opened_gap(store, 0, 9);
        assert_eq!(state_of(&rest), Some((1, whole("rest", 1))), "{case}");
        assert_eq!(rest.held, [(FREE, 0)], "{case}");
    });
    starts_again_alike(&disk.disk, &gap(), "split anew");
}

/// F3 for a segment that cannot be cut back, while it could be synced: every hello and
/// the list are answered with an I/O error; once it can, the next hello is welcomed and
/// restored with exactly what was confirmed.
#[test]
fn nobody_is_served_while_the_segment_cannot_be_cut_back() {
    let (store, disk) = picky();
    let (west, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 1);
    log(&west, 1, &[]);
    committed(&west, 1);
    west.flush();
    east.flush();

    disk.fail(Op::Sync, ".wal");
    disk.fail(Op::Truncate, ".wal");
    log(&west, 2, &[]);
    west.request(StoreRequest::Claim { chunks: vec![FREE] });
    assert_eq!(answers_until_lost(&west), []);
    assert_eq!(answers_until_lost(&east), []);
    // Syncing works again, cutting back does not.
    disk.mend();
    disk.fail(Op::Truncate, ".wal");
    for _ in 0..2 {
        for (region, epoch) in [(0, 1), (1, 7), (2, 1)] {
            let refused = store.open_region(hello_of(&gap(), region, epoch));
            assert!(
                matches!(refused, Err(StoreError::Io(_))),
                "region {region}: {:?}",
                refused.err()
            );
        }
        assert!(matches!(store.regions(), Err(StoreError::Io(_))));
    }
    assert!(disk.failures() > 2);

    disk.mend();
    assert_eq!(store.regions().unwrap().regions.len(), 3);
    let (_west, restored) = opened_gap(&store, 0, 2);
    assert_eq!((ticks(&restored), restored.held), (vec![1], vec![]));
    after_every_crash(&disk.disk, &gap(), "cut back", |store, case| {
        let (_, restored) = opened_gap(store, 0, 3);
        assert_eq!(
            (ticks(&restored), restored.held),
            (vec![1], vec![]),
            "{case}"
        );
    });
}

// Of regions and chunks.

/// Scenario 1: a region loads and saves a chunk of its stripe without claiming; the
/// other gets `NotHeld` with the holder for both, keeps its handle, and the chunk is
/// unchanged.
#[test]
fn a_region_loads_and_saves_in_its_stripe_and_the_other_is_told_who_holds_the_chunk() {
    let directory = tempfile::tempdir().unwrap();
    for store in crate::tests::stores(directory.path()) {
        let west = open(&store, hello(0, 1));
        let east = open(&store, hello(1, 1));
        for (owner, other, region, position) in [
            (&east, &west, 1, ChunkPos::new(3, 2)),
            (&west, &east, 0, WEST),
            (&east, &west, 1, ORIGIN),
        ] {
            assert_eq!(load(owner, position), built(position, &[]));
            save(owner, position, &built(position, &[GLASS]));
            assert_eq!(load(owner, position), built(position, &[GLASS]));

            other.request(StoreRequest::Load { position });
            assert_eq!(reply(other), not_held(position, Some(region)));
            save(other, position, &built(position, &[OTHER]));
            assert_eq!(reply(other), not_held(position, Some(region)));
            other.request(StoreRequest::Flush);
            assert_eq!(reply(other), StoreReply::Flushed);
            assert!(!other.is_lost());
            assert_eq!(load(owner, position), built(position, &[GLASS]));
        }
    }
}

/// Scenario 1 where nobody holds the chunk: `NotHeld` names no holder, and the save
/// that was refused has left nothing for whoever is granted the chunk later.
#[test]
fn a_chunk_that_nobody_holds_is_loaded_and_saved_by_nobody() {
    let (store, _disk) = gap_store();
    let (west, _) = opened_gap(&store, 0, 1);
    let (home, restored) = opened_gap(&store, 2, 1);
    assert_eq!(restored.held, [(ORIGIN, 0)]);
    assert_eq!(restored.pinned, []);

    west.request(StoreRequest::Load { position: FREE });
    assert_eq!(reply(&west), not_held(FREE, None));
    save(&west, FREE, &built(FREE, &[GLASS]));
    assert_eq!(reply(&west), not_held(FREE, None));
    // The home region holds the home chunk and nothing else.
    assert_eq!(load(&home, ORIGIN), built(ORIGIN, &[]));
    assert_eq!(holder(&home, 2, FREE), None);
    assert_eq!(holder(&home, 2, WEST), Some(RegionId(0)));
    assert_eq!(holder(&home, 2, EAST), Some(RegionId(1)));
    assert_eq!(holder(&west, 0, ORIGIN), Some(RegionId(2)));

    assert_eq!(claim(&home, &[FREE]), (vec![FREE], vec![]));
    assert_eq!(load(&home, FREE), built(FREE, &[]));
    assert_eq!(holder(&west, 0, FREE), Some(RegionId(2)));
}

/// Scenario 2: a claim of a chunk in the region's own stripe is granted, writes nothing
/// to the log and adds nothing to `Restored::held`; one in the other's stripe is
/// `foreign` with that region.
#[test]
fn a_claim_in_the_regions_own_stripe_is_granted_without_a_record() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let west = open(&store, hello(0, 1));
    let east = open(&store, hello(1, 1));
    log(&east, 1, &[]);
    committed(&east, 1);
    store.regions().unwrap();
    let before = records(disk.as_ref());
    assert!(!before.is_empty());

    let (own, others) = (ChunkPos::new(3, 2), ChunkPos::new(-4, 2));
    assert_eq!(
        claim(&east, &[own, others, ORIGIN, own]),
        (vec![own, ORIGIN], vec![(others, RegionId(0))])
    );
    assert_eq!(
        claim(&west, &[others, own]),
        (vec![others], vec![(own, RegionId(1))])
    );
    let list = store.regions().unwrap();
    assert_eq!(records(disk.as_ref()), before);
    for info in &list.regions {
        assert_eq!(info.bounds, None, "{info:?}");
    }

    drop((west, east));
    let (_, restored) = opened(&store, &division(), 1, 2);
    assert_eq!(restored.held, []);
    let (_, restored) = opened(&store, &division(), 0, 2);
    assert_eq!(restored.held, []);
    after_every_crash(
        &disk,
        &division(),
        "claimed in the stripes",
        |store, case| {
            for region in [0, 1] {
                let (_, restored) = opened(store, &division(), region, 3);
                assert_eq!(restored.held, [], "{case}");
            }
        },
    );
}

/// Scenario 3: a claim of a chunk in the gap is granted once, with one record; the
/// second region to ask gets `foreign`; after the store is started again
/// `Restored::held` has the chunk for the first and not for the second.
#[test]
fn a_free_chunk_is_granted_to_the_first_region_that_asks_and_to_no_other() {
    let (store, disk) = gap_store();
    let (west, restored) = opened_gap(&store, 0, 1);
    assert_eq!(restored.held, []);
    let (east, _) = opened_gap(&store, 1, 1);

    // Each chunk once, in the order of the request.
    assert_eq!(
        claim(&west, &[FREE_THREE, FREE, FREE_THREE]),
        (vec![FREE_THREE, FREE], vec![])
    );
    assert_eq!(
        claim(&east, &[FREE_TOO, FREE, ORIGIN]),
        (
            vec![FREE_TOO],
            vec![(FREE, RegionId(0)), (ORIGIN, RegionId(2))]
        )
    );
    // Asked for again, it is granted as before and nothing is written.
    assert_eq!(claim(&west, &[FREE]), (vec![FREE], vec![]));
    assert_eq!(claim(&east, &[FREE]), (vec![], vec![(FREE, RegionId(0))]));
    store.regions().unwrap();
    let granted: Vec<LogRecord> = records(disk.as_ref())
        .into_iter()
        .filter(|record| matches!(record, LogRecord::Granted { .. }))
        .collect();
    assert_eq!(
        granted,
        [
            LogRecord::Granted {
                region: 0,
                tick: 0,
                chunks: vec![FREE_THREE, FREE]
            },
            LogRecord::Granted {
                region: 1,
                tick: 0,
                chunks: vec![FREE_TOO]
            }
        ]
    );

    after_every_crash(&disk, &gap(), "claimed in the gap", |store, case| {
        assert_eq!(held(store, 0, 2), [(FREE, 0), (FREE_THREE, 0)], "{case}");
        assert_eq!(held(store, 1, 2), [(FREE_TOO, 0)], "{case}");
        assert_eq!(held(store, 2, 2), [(ORIGIN, 0)], "{case}");
        // And the second is still told so.
        let (east, _) = opened_gap(store, 1, 3);
        assert_eq!(
            claim(&east, &[FREE]),
            (vec![], vec![(FREE, RegionId(0))]),
            "{case}"
        );
    });
    starts_again_alike(&disk, &gap(), "claimed in the gap");
}

/// Scenario 3: two claims in one group of the same chunk by two regions are answered
/// one each way, and the store that is started again agrees.
#[test]
fn two_claims_of_one_chunk_in_one_group_are_answered_one_each_way() {
    let (store, disk) = switched_for(&gap());
    let (west, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 1);
    west.flush();
    east.flush();

    // Both arrive while the log is being synced, and are written in one group.
    disk.holding_syncs.store(true, Ordering::SeqCst);
    log(&west, 1, &[]);
    disk.held.wait();
    east.request(StoreRequest::Claim { chunks: vec![FREE] });
    west.request(StoreRequest::Claim { chunks: vec![FREE] });
    disk.held.wait();
    assert_eq!(
        reply(&east),
        StoreReply::Claimed {
            granted: vec![FREE],
            foreign: vec![]
        }
    );
    assert_eq!(
        reply(&west),
        StoreReply::Claimed {
            granted: vec![],
            foreign: vec![(FREE, RegionId(1))]
        }
    );
    after_every_crash(&disk.disk, &gap(), "claimed together", |store, case| {
        assert_eq!(held(store, 1, 2), [(FREE, 0)], "{case}");
        assert_eq!(held(store, 0, 2), [], "{case}");
    });
}

/// Scenario 4: a claim is answered only after the commits asked for before it are
/// answered, and not before its group is synced; from the moment it is answered a
/// crash that keeps nothing unsynced has it.
#[test]
fn a_claim_is_answered_behind_the_commits_before_it_and_is_durable_by_then() {
    let (store, disk) = switched_for(&gap());
    let (west, _) = opened_gap(&store, 0, 1);
    // The opening is durable by itself, so that the first commit is a group of its own.
    west.flush();

    disk.holding_syncs.store(true, Ordering::SeqCst);
    log(&west, 1, &[]);
    disk.held.wait();
    // These wait for the sync of the first commit, and are the next group.
    log(&west, 2, &[]);
    west.request(StoreRequest::Claim { chunks: vec![FREE] });
    log(&west, 3, &[]);
    disk.holding_syncs.store(true, Ordering::SeqCst);
    disk.held.wait();
    disk.held.wait();
    // The store is in the sync of that group now: nothing of it is answered.
    assert_eq!(west.try_reply(), Some(StoreReply::Committed { tick: 1 }));
    assert_eq!(west.try_reply(), None);
    disk.held.wait();

    assert_eq!(any_reply(&west), StoreReply::Committed { tick: 2 });
    assert_eq!(
        any_reply(&west),
        StoreReply::Claimed {
            granted: vec![FREE],
            foreign: vec![]
        }
    );
    // Answered, so a machine that loses everything unsynced now has the grant, from
    // the tick of the last commit before the claim.
    let left = Arc::new(disk.disk.crashed(Survival::Nothing));
    let again = store_on(&left, &gap()).unwrap();
    assert_eq!(held(&again, 0, 2), [(FREE, 2)]);
    assert_eq!(any_reply(&west), StoreReply::Committed { tick: 3 });
}

/// Scenario 4 without a commit beside it: a claim by itself is in the log when it is
/// answered.
#[test]
fn a_claim_by_itself_is_durable_when_it_is_answered() {
    let (store, disk) = gap_store();
    let (west, _) = opened_gap(&store, 0, 1);
    for (round, chunk) in [FREE, FREE_TOO, FREE_THREE].into_iter().enumerate() {
        // A flush asked for behind the claim waits for it, as behind a commit.
        west.request(StoreRequest::Claim {
            chunks: vec![chunk],
        });
        west.request(StoreRequest::Flush);
        let claimed = StoreReply::Claimed {
            granted: vec![chunk],
            foreign: vec![],
        };
        assert_eq!(any_reply(&west), claimed);
        let left = Arc::new(disk.crashed(Survival::Nothing));
        let again = store_on(&left, &gap()).unwrap();
        let held = held(&again, 0, 2);
        assert_eq!(held.len(), round + 1, "{held:?}");
        assert!(held.contains(&(chunk, 0)), "{held:?}");
        assert_eq!(any_reply(&west), StoreReply::Flushed);
    }
}

/// Scenario 5: the tick of a grant is that of the last commit the region had sent; a
/// region opened again with a higher epoch, restored up to `t`, that changes a block of
/// a claimed chunk in tick `t + 1` and is opened once more finds the change in the
/// chunk.
#[test]
fn a_grant_has_the_tick_of_the_last_commit_and_what_follows_it_is_replayed() {
    let (store, disk) = gap_store();
    let (west, _) = opened_gap(&store, 0, 1);
    // Before any commit the region holds the chunk from tick 0.
    assert_eq!(claim(&west, &[FREE_THREE]), (vec![FREE_THREE], vec![]));
    for tick in 1..=3 {
        log(&west, tick, &[]);
    }
    // The claim follows the commits at once; whether they are answered yet or not,
    // the last of them is the tick. The region itself may have run on to any tick.
    assert_eq!(claim(&west, &[FREE]), (vec![FREE], vec![]));
    log(&west, 4, &[]);
    committed(&west, 4);
    assert_eq!(claim(&west, &[FREE_TOO]), (vec![FREE_TOO], vec![]));
    drop(west);

    let (west, restored) = opened_gap(&store, 0, 2);
    assert_eq!(restored.held, [(FREE, 3), (FREE_TOO, 4), (FREE_THREE, 0)]);
    assert_eq!(restored.tick(), 4);
    // Restored up to tick 4, it changes the chunk in tick 5, and is opened once more.
    log(&west, 5, &[change(FREE, GLASS), change(FREE_TOO, STONE)]);
    committed(&west, 5);
    drop(west);
    let (west, restored) = opened_gap(&store, 0, 3);
    assert_eq!(ticks(&restored), [1, 2, 3, 4, 5]);
    assert_eq!(load(&west, FREE), built(FREE, &[GLASS]));
    assert_eq!(load(&west, FREE_TOO), built(FREE_TOO, &[STONE]));
    west.flush();
    after_every_crash(&disk, &gap(), "changed after the grant", |store, case| {
        let (west, restored) = opened_gap(store, 0, 4);
        assert_eq!(
            restored.held,
            [(FREE, 3), (FREE_TOO, 4), (FREE_THREE, 0)],
            "{case}"
        );
        assert_eq!(load(&west, FREE), built(FREE, &[GLASS]), "{case}");
        assert_eq!(load(&west, FREE_TOO), built(FREE_TOO, &[STONE]), "{case}");
    });
}

/// Section 3.4: of the block changes of the commits a region is restored with, only
/// those are put into the stored chunks that are in a chunk the region holds now, from
/// a tick above the one it holds the chunk from.
#[test]
fn only_changes_to_chunks_a_region_holds_from_ticks_after_the_grant_are_replayed() {
    let (store, disk) = gap_store();
    let (west, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 1);
    log(
        &west,
        1,
        &[
            // Its own by being pinned: replayed.
            change(WEST, GLASS),
            // Not its own when it commits this, and granted in this very tick.
            change(FREE, STONE),
            // Never its own.
            change(FREE_TOO, STONE),
            change(EAST, STONE),
            change(ORIGIN, STONE),
        ],
    );
    assert_eq!(claim(&west, &[FREE]), (vec![FREE], vec![]));
    log(&west, 2, &[change(FREE, OTHER)]);
    committed(&west, 2);
    drop(west);

    let check = |store: &Store, epoch: u64, case: &str| {
        let (west, restored) = opened_gap(store, 0, epoch);
        assert_eq!(ticks(&restored), [1, 2], "{case}");
        assert_eq!(restored.held, [(FREE, 1)], "{case}");
        assert_eq!(load(&west, WEST), built(WEST, &[GLASS]), "{case}");
        assert_eq!(load(&west, FREE), built(FREE, &[OTHER]), "{case}");
        west.flush();
        let (east, _) = opened_gap(store, 1, epoch);
        assert_eq!(load(&east, EAST), built(EAST, &[]), "{case}");
        let (home, _) = opened_gap(store, 2, epoch);
        assert_eq!(load(&home, ORIGIN), built(ORIGIN, &[]), "{case}");
        // Granted now, the chunk the region changed without holding it is as it was.
        assert_eq!(
            claim(&home, &[FREE_TOO]),
            (vec![FREE_TOO], vec![]),
            "{case}"
        );
        assert_eq!(load(&home, FREE_TOO), built(FREE_TOO, &[]), "{case}");
    };
    drop(east);
    after_every_crash(&disk, &gap(), "before it is opened again", |store, case| {
        check(store, 2, case)
    });
    check(&store, 2, "in the store that took the commits");
}

/// A chunk that a region changed without holding it, and was granted later, is not
/// changed when the region is opened again: its commit is from before the grant.
#[test]
fn a_change_from_before_a_later_grant_is_not_replayed_into_the_chunk() {
    let (store, _disk) = gap_store();
    let (west, _) = opened_gap(&store, 0, 1);
    log(&west, 1, &[change(FREE, STONE)]);
    log(&west, 2, &[]);
    assert_eq!(claim(&west, &[FREE]), (vec![FREE], vec![]));
    drop(west);
    let (west, restored) = opened_gap(&store, 0, 2);
    assert_eq!(restored.held, [(FREE, 2)]);
    assert_eq!(load(&west, FREE), built(FREE, &[]));
}

/// Scenario 6, built over: a region changes a block of a chunk, saves the chunk and
/// returns it, without a checkpoint; another claims the chunk, sets the same block
/// otherwise, saves, makes a checkpoint and returns; the first claims the chunk again
/// and is then opened anew: the block is as the second left it.
#[test]
fn what_a_later_holder_built_over_is_not_put_back_by_the_first_holders_log() {
    let (store, disk) = gap_store();
    let (first, _) = opened_gap(&store, 0, 1);
    let (second, _) = opened_gap(&store, 1, 1);
    assert_eq!(claim(&first, &[FREE]), (vec![FREE], vec![]));
    log(&first, 1, &[change(FREE, STONE)]);
    save(&first, FREE, &built(FREE, &[STONE]));
    give_back(&first, &[FREE]);
    first.flush();

    assert_eq!(claim(&second, &[FREE]), (vec![FREE], vec![]));
    assert_eq!(load(&second, FREE), built(FREE, &[STONE]));
    log(&second, 1, &[change(FREE, GLASS)]);
    save(&second, FREE, &built(FREE, &[GLASS]));
    checkpoint(&second, 1, "second");
    give_back(&second, &[FREE]);
    second.flush();

    assert_eq!(claim(&first, &[FREE]), (vec![FREE], vec![]));
    drop(first);
    let (first, restored) = opened_gap(&store, 0, 2);
    assert_eq!(ticks(&restored), [1]);
    assert_eq!(restored.held, [(FREE, 1)]);
    assert_eq!(load(&first, FREE), built(FREE, &[GLASS]));
    first.flush();
    after_every_crash(&disk, &gap(), "built over", |store, case| {
        let (first, restored) = opened_gap(store, 0, 3);
        assert_eq!(ticks(&restored), [1], "{case}");
        assert_eq!(load(&first, FREE), built(FREE, &[GLASS]), "{case}");
    });
}

/// Scenario 6 the other way round as well, and with neither region making a
/// checkpoint: each is opened anew while its commit to the chunk is in its log, once
/// while the other holds the chunk and once when it has the chunk back. What the last
/// holder saved stays.
#[test]
fn a_chunk_that_goes_to_and_fro_is_as_its_last_holder_saved_it() {
    let (store, disk) = gap_store();
    let (first, _) = opened_gap(&store, 0, 1);
    let (second, _) = opened_gap(&store, 1, 1);
    assert_eq!(claim(&first, &[FREE]), (vec![FREE], vec![]));
    log(&first, 1, &[change(FREE, STONE)]);
    save(&first, FREE, &built(FREE, &[STONE]));
    give_back(&first, &[FREE]);
    first.flush();

    assert_eq!(claim(&second, &[FREE]), (vec![FREE], vec![]));
    log(&second, 1, &[change(FREE, GLASS)]);
    save(&second, FREE, &built(FREE, &[GLASS]));
    second.flush();
    // The first is opened anew while the second holds the chunk: nothing of its log
    // goes into a chunk it does not hold.
    drop(first);
    let (first, restored) = opened_gap(&store, 0, 2);
    assert_eq!((ticks(&restored), restored.held), (vec![1], vec![]));
    first.flush();
    assert_eq!(load(&second, FREE), built(FREE, &[GLASS]));

    give_back(&second, &[FREE]);
    second.flush();
    // The first has the chunk back, from the tick it was restored up to, sets the
    // block a third way and gives the chunk back once more.
    assert_eq!(claim(&first, &[FREE]), (vec![FREE], vec![]));
    assert_eq!(load(&first, FREE), built(FREE, &[GLASS]));
    log(&first, 2, &[change(FREE, (7, 100, 7, blocks::AIR))]);
    save(&first, FREE, &built(FREE, &[]));
    give_back(&first, &[FREE]);
    first.flush();

    // The second claims it again and is opened anew with its commit still in its log.
    assert_eq!(claim(&second, &[FREE]), (vec![FREE], vec![]));
    drop(second);
    let (second, restored) = opened_gap(&store, 1, 2);
    assert_eq!(
        (ticks(&restored), restored.held),
        (vec![1], vec![(FREE, 1)])
    );
    assert_eq!(load(&second, FREE), built(FREE, &[]));
    second.flush();
    after_every_crash(&disk, &gap(), "to and fro", |store, case| {
        let (second, _) = opened_gap(store, 1, 3);
        assert_eq!(load(&second, FREE), built(FREE, &[]), "{case}");
        let (_, restored) = opened_gap(store, 0, 3);
        assert_eq!(
            (ticks(&restored), restored.held),
            (vec![1, 2], vec![]),
            "{case}"
        );
    });
}

/// Scenario 7: a chunk that is returned and not saved again loses nothing that was
/// saved before; a claim that arrives before the return is through is `foreign`, one
/// after it granted; a flush behind the return is answered only once a crash would
/// keep the return. The thread for chunks is held before the return's sync, and
/// between that and its message.
#[test]
fn a_chunk_is_its_regions_until_its_return_is_through_and_a_flush_waits_for_that() {
    for stop in [Stop::BeforeSync, Stop::AfterSync] {
        let (store, gate, disk) = gated_gap();
        let (first, _) = opened_gap(&store, 0, 1);
        let (second, _) = opened_gap(&store, 1, 1);
        assert_eq!(claim(&first, &[FREE]), (vec![FREE], vec![]));
        save(&first, FREE, &built(FREE, &[GLASS]));
        first.flush();

        gate.hold_at(stop);
        give_back(&first, &[FREE]);
        first.request(StoreRequest::Flush);
        gate.wait_until_there();
        assert_eq!(
            claim(&second, &[FREE]),
            (vec![], vec![(FREE, RegionId(0))]),
            "{stop:?}"
        );
        assert_eq!(holder(&second, 1, FREE), Some(RegionId(0)), "{stop:?}");
        // The flush is not answered, and a crash of any kind leaves the chunk the
        // region's.
        assert_eq!(first.try_reply(), None, "{stop:?}");
        after_every_crash(&disk, &gap(), &format!("held {stop:?}"), |store, case| {
            assert_eq!(held(store, 0, 2), [(FREE, 0)], "{case}");
        });

        gate.let_go();
        assert_eq!(reply(&first), StoreReply::Flushed, "{stop:?}");
        after_every_crash(
            &disk,
            &gap(),
            &format!("through, {stop:?}"),
            |store, case| {
                assert_eq!(held(store, 0, 2), [], "{case}");
                let (second, _) = opened_gap(store, 1, 2);
                assert_eq!(claim(&second, &[FREE]), (vec![FREE], vec![]), "{case}");
                assert_eq!(load(&second, FREE), built(FREE, &[GLASS]), "{case}");
            },
        );
        assert_eq!(claim(&second, &[FREE]), (vec![FREE], vec![]), "{stop:?}");
        assert_eq!(load(&second, FREE), built(FREE, &[GLASS]), "{stop:?}");
        assert_eq!(holder(&first, 0, FREE), Some(RegionId(1)), "{stop:?}");
        // The grant is behind the return in the log (section 6).
        let log = records(disk.as_ref());
        let place = |record: LogRecord| log.iter().position(|logged| *logged == record);
        let returned = place(LogRecord::Returned {
            region: 0,
            chunks: vec![FREE],
        });
        let granted = place(LogRecord::Granted {
            region: 1,
            tick: 0,
            chunks: vec![FREE],
        });
        assert!(
            returned.is_some() && returned < granted,
            "{stop:?}: {log:?}"
        );
    }
}

/// Scenario 8: a region that claims a chunk again while its return of it is under way
/// keeps it, with the tick it had, and no record says that it was returned.
#[test]
fn a_chunk_claimed_again_while_its_return_is_under_way_is_kept_with_its_tick() {
    for stop in [Stop::BeforeSync, Stop::AfterSync] {
        let (store, gate, disk) = gated_gap();
        let (first, _) = opened_gap(&store, 0, 1);
        let (second, _) = opened_gap(&store, 1, 1);
        log(&first, 1, &[]);
        log(&first, 2, &[]);
        assert_eq!(claim(&first, &[FREE]), (vec![FREE], vec![]));
        log(&first, 3, &[]);
        log(&first, 4, &[]);
        first.flush();

        gate.hold_at(stop);
        give_back(&first, &[FREE]);
        gate.wait_until_there();
        assert_eq!(claim(&first, &[FREE]), (vec![FREE], vec![]), "{stop:?}");
        gate.let_go();
        first.flush();

        assert_eq!(
            claim(&second, &[FREE]),
            (vec![], vec![(FREE, RegionId(0))]),
            "{stop:?}"
        );
        store.regions().unwrap();
        let returned = |record: &LogRecord| matches!(record, LogRecord::Returned { .. });
        assert!(!records(disk.as_ref()).iter().any(returned), "{stop:?}");
        after_every_crash(&disk, &gap(), &format!("kept, {stop:?}"), |store, case| {
            assert_eq!(held(store, 0, 2), [(FREE, 2)], "{case}");
        });
        drop(first);
        assert_eq!(held(&store, 0, 2), [(FREE, 2)], "{stop:?}");
    }
}

/// Starts a store on what every kind of crash leaves of `disk`, and sees to it that
/// `chunk` is still region 0's there, held from tick 0, that the region loads it with
/// `marks` once it is opened, and that region 1 is told so when it claims the chunk.
fn the_chunk_is_kept_with(disk: &MemoryDisk, chunk: ChunkPos, marks: &[Mark], point: &str) {
    after_every_crash(disk, &gap(), point, |store, case| {
        let (owner, restored) = opened_gap(store, 0, 2);
        assert!(restored.held.contains(&(chunk, 0)), "{case}: {restored:?}");
        assert_eq!(load(&owner, chunk), built(chunk, marks), "{case}");
        let (other, _) = opened_gap(store, 1, 2);
        assert_eq!(
            claim(&other, &[chunk]),
            (vec![], vec![(chunk, RegionId(0))]),
            "{case}"
        );
    });
}

/// Starts a store on what every kind of crash leaves of `disk`, and sees to it that
/// `chunk` is nobody's there, and that whoever claims it loads it with `marks`.
fn the_chunk_is_free_with(disk: &MemoryDisk, chunk: ChunkPos, marks: &[Mark], point: &str) {
    after_every_crash(disk, &gap(), point, |store, case| {
        for region in [0, 1, 2] {
            let held = held(store, region, 2);
            assert!(!held.iter().any(|(held, _)| *held == chunk), "{case}");
        }
        let (other, _) = opened_gap(store, 1, 3);
        assert_eq!(claim(&other, &[chunk]), (vec![chunk], vec![]), "{case}");
        assert_eq!(load(&other, chunk), built(chunk, marks), "{case}");
    });
}

/// Scenario 9, a return that was called off frees nothing later. With the thread for
/// chunks held: a region returns `c`, claims it again, commits a change to it, saves it
/// and returns it again. Wherever the thread is let go to and the store killed, up to
/// the second return's message, `c` is the region's and the change is in what it loads
/// after opening again; let go to the end, `c` is free and the stored chunk has the
/// change.
#[test]
fn a_return_that_was_called_off_frees_nothing_later() {
    let (store, gate, disk) = gated_gap();
    let (owner, _) = opened_gap(&store, 0, 1);
    let (other, _) = opened_gap(&store, 1, 1);
    assert_eq!(claim(&owner, &[FREE]), (vec![FREE], vec![]));
    owner.flush();

    gate.hold_at(Stop::BeforeSync);
    give_back(&owner, &[FREE]);
    gate.wait_until_there();
    assert_eq!(claim(&owner, &[FREE]), (vec![FREE], vec![]));
    log(&owner, 1, &[change(FREE, GLASS)]);
    committed(&owner, 1);
    save(&owner, FREE, &built(FREE, &[GLASS]));
    give_back(&owner, &[FREE]);
    owner.request(StoreRequest::Flush);

    for (stop, point) in [
        (
            Stop::AfterSync,
            "between the sync and the message of the return that was called off",
        ),
        (
            Stop::BeforeSave(FREE),
            "as far as the first return, before the save",
        ),
        (Stop::BeforeSync, "saved, before the second return's sync"),
        (
            Stop::AfterSync,
            "between the sync and the message of the second return",
        ),
    ] {
        gate.on_to(stop);
        // What the thread for chunks has told the commit thread so far is dealt with,
        // and what that wrote is durable.
        store.regions().unwrap();
        assert_eq!(holder(&other, 1, FREE), Some(RegionId(0)), "{point}");
        the_chunk_is_kept_with(&disk, FREE, &[GLASS], point);
        assert_eq!(owner.try_reply(), None, "{point}");
    }

    gate.let_go();
    assert_eq!(reply(&owner), StoreReply::Flushed);
    assert_eq!(holder(&other, 1, FREE), None);
    the_chunk_is_free_with(&disk, FREE, &[GLASS], "both returns through");
    assert_eq!(claim(&other, &[FREE]), (vec![FREE], vec![]));
    assert_eq!(load(&other, FREE), built(FREE, &[GLASS]));
    starts_again_alike(&disk, &gap(), "both returns through");
}

/// Scenario 9 with the first return held between its sync and its message when the
/// region claims the chunk again: the message that comes later frees nothing either.
#[test]
fn a_return_called_off_after_its_sync_frees_nothing_later() {
    let (store, gate, disk) = gated_gap();
    let (owner, _) = opened_gap(&store, 0, 1);
    let (other, _) = opened_gap(&store, 1, 1);
    assert_eq!(claim(&owner, &[FREE]), (vec![FREE], vec![]));
    owner.flush();

    gate.hold_at(Stop::AfterSync);
    give_back(&owner, &[FREE]);
    gate.wait_until_there();
    assert_eq!(claim(&owner, &[FREE]), (vec![FREE], vec![]));
    log(&owner, 1, &[change(FREE, GLASS)]);
    committed(&owner, 1);
    save(&owner, FREE, &built(FREE, &[GLASS]));
    give_back(&owner, &[FREE]);
    owner.request(StoreRequest::Flush);

    gate.on_to(Stop::BeforeSave(FREE));
    store.regions().unwrap();
    assert_eq!(holder(&other, 1, FREE), Some(RegionId(0)));
    the_chunk_is_kept_with(&disk, FREE, &[GLASS], "as far as the first return");
    // A third time, while the second return is under way: it is called off as well.
    gate.on_to(Stop::BeforeSync);
    assert_eq!(claim(&owner, &[FREE]), (vec![FREE], vec![]));
    log(&owner, 2, &[change(FREE, OTHER)]);
    committed(&owner, 2);
    gate.let_go();
    assert_eq!(reply(&owner), StoreReply::Flushed);
    assert_eq!(holder(&other, 1, FREE), Some(RegionId(0)));
    the_chunk_is_kept_with(&disk, FREE, &[GLASS, OTHER], "both returns called off");

    // Saved and returned without anything in the way, it is free.
    save(&owner, FREE, &built(FREE, &[GLASS, OTHER]));
    give_back(&owner, &[FREE]);
    owner.flush();
    the_chunk_is_free_with(&disk, FREE, &[GLASS, OTHER], "returned at last");
    assert_eq!(holder(&other, 1, FREE), None);
}

/// Section 3.3: a return of a chunk that is being returned already takes the place of
/// the earlier one, which then frees nothing: the save between the two is durable
/// before the chunk is anybody else's.
#[test]
fn a_chunk_returned_twice_is_free_only_when_the_second_return_is_through() {
    let (store, gate, disk) = gated_gap();
    let (owner, _) = opened_gap(&store, 0, 1);
    let (other, _) = opened_gap(&store, 1, 1);
    assert_eq!(claim(&owner, &[FREE]), (vec![FREE], vec![]));
    owner.flush();

    gate.hold_at(Stop::BeforeSync);
    give_back(&owner, &[FREE]);
    gate.wait_until_there();
    log(&owner, 1, &[change(FREE, GLASS)]);
    committed(&owner, 1);
    save(&owner, FREE, &built(FREE, &[GLASS]));
    give_back(&owner, &[FREE]);
    owner.request(StoreRequest::Flush);

    for (stop, point) in [
        (Stop::BeforeSave(FREE), "as far as the first return"),
        (Stop::AfterSync, "before the second return's message"),
    ] {
        gate.on_to(stop);
        store.regions().unwrap();
        assert_eq!(holder(&other, 1, FREE), Some(RegionId(0)), "{point}");
        the_chunk_is_kept_with(&disk, FREE, &[GLASS], point);
    }
    gate.let_go();
    assert_eq!(reply(&owner), StoreReply::Flushed);
    assert_eq!(holder(&other, 1, FREE), None);
    the_chunk_is_free_with(&disk, FREE, &[GLASS], "the second return through");
}

/// Scenario 9 with two returns of different chunks interleaved: the return of one
/// chunk is called off and made again while the return of another, asked for between
/// them, goes through. Each frees its own chunk and no other.
#[test]
fn returns_of_different_chunks_free_each_its_own_while_one_is_called_off() {
    let (store, gate, disk) = gated_gap();
    let (owner, _) = opened_gap(&store, 0, 1);
    let (other, _) = opened_gap(&store, 1, 1);
    assert_eq!(
        claim(&owner, &[FREE, FREE_TOO]),
        (vec![FREE, FREE_TOO], vec![])
    );
    save(&owner, FREE_TOO, &built(FREE_TOO, &[STONE]));
    owner.flush();

    gate.hold_at(Stop::BeforeSync);
    give_back(&owner, &[FREE]);
    gate.wait_until_there();
    give_back(&owner, &[FREE_TOO]);
    assert_eq!(claim(&owner, &[FREE]), (vec![FREE], vec![]));
    log(&owner, 1, &[change(FREE, GLASS)]);
    committed(&owner, 1);
    save(&owner, FREE, &built(FREE, &[GLASS]));
    give_back(&owner, &[FREE]);
    owner.request(StoreRequest::Flush);

    // Between the sync and the message of the return of the other chunk.
    gate.on_to(Stop::AfterSync);
    gate.on_to(Stop::AfterSync);
    store.regions().unwrap();
    assert_eq!(holder(&other, 1, FREE), Some(RegionId(0)));
    assert_eq!(holder(&other, 1, FREE_TOO), Some(RegionId(0)));
    the_chunk_is_kept_with(&disk, FREE, &[GLASS], "before the other's message");
    the_chunk_is_kept_with(&disk, FREE_TOO, &[STONE], "before the other's message");

    // The first two returns are through: the other chunk is free, this one is kept.
    gate.on_to(Stop::BeforeSave(FREE));
    store.regions().unwrap();
    assert_eq!(holder(&other, 1, FREE), Some(RegionId(0)));
    assert_eq!(holder(&other, 1, FREE_TOO), None);
    the_chunk_is_kept_with(&disk, FREE, &[GLASS], "the other chunk returned");
    the_chunk_is_free_with(&disk, FREE_TOO, &[STONE], "the other chunk returned");

    gate.let_go();
    assert_eq!(reply(&owner), StoreReply::Flushed);
    assert_eq!(holder(&other, 1, FREE), None);
    the_chunk_is_free_with(&disk, FREE, &[GLASS], "all through");
    the_chunk_is_free_with(&disk, FREE_TOO, &[STONE], "all through");
    // One record for each chunk that was freed, and none for the return called off.
    let returned: Vec<LogRecord> = records(disk.as_ref())
        .into_iter()
        .filter(|record| matches!(record, LogRecord::Returned { .. }))
        .collect();
    assert_eq!(
        returned,
        [
            LogRecord::Returned {
                region: 0,
                chunks: vec![FREE_TOO]
            },
            LogRecord::Returned {
                region: 0,
                chunks: vec![FREE]
            }
        ]
    );
}

/// A claim calls off the return of the chunk it names and not that of another chunk
/// named in the same return.
#[test]
fn a_claim_calls_off_the_return_of_its_chunk_and_not_of_one_returned_with_it() {
    for stop in [Stop::BeforeSync, Stop::AfterSync] {
        let (store, gate, disk) = gated_gap();
        let (owner, _) = opened_gap(&store, 0, 1);
        let (other, _) = opened_gap(&store, 1, 1);
        assert_eq!(
            claim(&owner, &[FREE, FREE_TOO]),
            (vec![FREE, FREE_TOO], vec![])
        );
        save(&owner, FREE_TOO, &built(FREE_TOO, &[STONE]));
        owner.flush();

        gate.hold_at(stop);
        give_back(&owner, &[FREE, FREE_TOO]);
        gate.wait_until_there();
        assert_eq!(claim(&owner, &[FREE]), (vec![FREE], vec![]), "{stop:?}");
        gate.let_go();
        owner.flush();

        assert_eq!(holder(&other, 1, FREE), Some(RegionId(0)), "{stop:?}");
        assert_eq!(holder(&other, 1, FREE_TOO), None, "{stop:?}");
        let returned: Vec<LogRecord> = records(disk.as_ref())
            .into_iter()
            .filter(|record| matches!(record, LogRecord::Returned { .. }))
            .collect();
        let expected = LogRecord::Returned {
            region: 0,
            chunks: vec![FREE_TOO],
        };
        assert_eq!(returned, [expected], "{stop:?}");
        the_chunk_is_kept_with(&disk, FREE, &[], &format!("{stop:?}"));
        the_chunk_is_free_with(&disk, FREE_TOO, &[STONE], &format!("{stop:?}"));
    }
}

/// Section 3.3, step 1: the home chunk and chunks the region has no grant for are left
/// out of a return, and the others named with them are returned.
#[test]
fn the_home_chunk_and_chunks_that_were_not_granted_are_left_out_of_a_return() {
    let (store, disk) = gap_store();
    let (west, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 1);
    let (home, _) = opened_gap(&store, 2, 1);
    assert_eq!(claim(&home, &[FREE]), (vec![FREE], vec![]));
    assert_eq!(claim(&east, &[FREE_TOO]), (vec![FREE_TOO], vec![]));

    // The home chunk, a chunk of another region's area, a chunk another region was
    // granted, a chunk that is nobody's, and one that is its own to give back.
    give_back(&home, &[ORIGIN, WEST, FREE_TOO, FREE_THREE, FREE]);
    home.flush();
    // A chunk held by being pinned, and another region's.
    give_back(&west, &[WEST, FREE_TOO]);
    west.flush();

    assert_eq!(load(&home, ORIGIN), built(ORIGIN, &[]));
    assert_eq!(load(&west, WEST), built(WEST, &[]));
    assert_eq!(holder(&west, 0, FREE_TOO), Some(RegionId(1)));
    assert_eq!(holder(&west, 0, FREE_THREE), None);
    assert_eq!(holder(&west, 0, FREE), None);
    assert!(!home.is_lost() && !west.is_lost());
    after_every_crash(&disk, &gap(), "returned", |store, case| {
        assert_eq!(held(store, 0, 2), [], "{case}");
        assert_eq!(held(store, 1, 2), [(FREE_TOO, 0)], "{case}");
        assert_eq!(held(store, 2, 2), [(ORIGIN, 0)], "{case}");
    });
}

/// Scenario 10: a `Returned` in the log for a chunk the region has no grant of, made
/// by hand, does not keep the store from starting, and changes nothing. Nor does one
/// of a region the table does not have ("Found while building", C1.4).
#[test]
fn a_returned_record_for_a_chunk_that_was_not_granted_changes_nothing() {
    let (store, disk) = gap_store();
    let (west, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 1);
    assert_eq!(claim(&west, &[FREE]), (vec![FREE], vec![]));
    assert_eq!(claim(&east, &[FREE_TOO]), (vec![FREE_TOO], vec![]));
    west.flush();
    east.flush();
    store.regions().unwrap();
    drop((west, east));

    let plain = Arc::new(disk.crashed(Survival::Everything));
    let expected = {
        let store = store_on(&plain, &gap()).unwrap();
        world(&store, &gap(), 5, "without the record")
    };
    let record = |region, chunks: &[ChunkPos]| LogRecord::Returned {
        region,
        chunks: chunks.to_vec(),
    };
    let stray = [
        // Another region's grant, a chunk that is nobody's, chunks of pinned areas,
        // the home region's and a region's that never was.
        record(1, &[FREE]),
        record(0, &[FREE_TOO, FREE_THREE]),
        record(0, &[WEST, EAST]),
        record(2, &[FREE, WEST]),
        record(77, &[FREE, FREE_TOO]),
    ];
    let with = Arc::new(disk.crashed(Survival::Everything));
    let next = segments(with.as_ref()).last().unwrap() + 1;
    let bytes: Vec<u8> = stray.iter().flat_map(LogRecord::encode).collect();
    put(&with, &segment_path(next), &bytes);
    let store = store_on(&with, &gap()).unwrap();
    assert_eq!(world(&store, &gap(), 5, "with the record"), expected);
    assert_eq!(expected.restored[0].1.held, [(FREE, 0)]);
    assert_eq!(expected.restored[1].1.held, [(FREE_TOO, 0)]);
    drop(store);
    starts_again_alike(&with, &gap(), "with the record");

    // A chunk the region was granted is returned by the record that names it, with one
    // it was not granted beside it.
    let mixed = Arc::new(disk.crashed(Survival::Everything));
    let bytes = record(0, &[FREE_THREE, FREE, WEST]).encode();
    put(&mixed, &segment_path(next), &bytes);
    let store = store_on(&mixed, &gap()).unwrap();
    assert_eq!(held(&store, 0, 5), []);
    assert_eq!(held(&store, 1, 5), [(FREE_TOO, 0)]);
    let (east, _) = opened_gap(&store, 1, 6);
    assert_eq!(claim(&east, &[FREE]), (vec![FREE], vec![]));
    let (west, _) = opened_gap(&store, 0, 6);
    assert_eq!(load(&west, WEST), built(WEST, &[]));
}

/// The world of scenario 11 up to the last checkpoint: three regions that have claimed
/// and committed, of which all but the home region have made a checkpoint of all they
/// committed. Returns the handles of the western, the eastern and the home region.
fn before_the_last_checkpoint(store: &Store) -> [StoreHandle; 3] {
    let handles = [0, 1, 2].map(|region| opened_gap(store, region, 1).0);
    let [west, east, home] = &handles;
    assert_eq!(claim(west, &[FREE]), (vec![FREE], vec![]));
    log(west, 1, &[change(WEST, GLASS)]);
    assert_eq!(claim(east, &[FREE_TOO]), (vec![FREE_TOO], vec![]));
    log(east, 1, &[change(FREE_TOO, GLASS)]);
    log(home, 1, &[change(ORIGIN, GLASS)]);
    save(west, WEST, &built(WEST, &[GLASS]));
    checkpoint(west, 1, "west");
    save(east, FREE_TOO, &built(FREE_TOO, &[GLASS]));
    checkpoint(east, 1, "east");
    for handle in &handles {
        handle.flush();
    }
    handles
}

/// Scenario 11: after checkpoints of every region, the table file has what the log
/// said of the regions, the log has no segment below the file's `from`, and a store
/// started on that world, whatever a crash keeps, has the same list and the same
/// `held`.
#[test]
fn after_checkpoints_of_every_region_the_table_is_in_its_file_and_the_log_begins_behind_it() {
    let (store, disk) = gap_store();
    let [west, east, home] = before_the_last_checkpoint(&store);
    // The home region's commit is behind its checkpoint, so its segment is needed, and
    // the table file is as the new world had it.
    let table = table_file(disk.as_ref());
    assert_eq!(table.regions[0].grants, []);
    let first = segments(disk.as_ref());
    assert!(
        first.iter().any(|number| *number >= table.from),
        "{first:?}"
    );

    save(&home, ORIGIN, &built(ORIGIN, &[GLASS]));
    checkpoint(&home, 1, "home");
    home.flush();
    store.regions().unwrap();
    let table = table_file(disk.as_ref());
    let grants: Vec<_> = table
        .regions
        .iter()
        .map(|region| (region.id, region.grants.clone()))
        .collect();
    assert_eq!(
        grants,
        [
            (0, vec![(FREE, 0)]),
            (1, vec![(FREE_TOO, 0)]),
            (2, vec![(ORIGIN, 0)])
        ]
    );
    assert_eq!((table.next_region, table.home_region), (3, 2));
    let left = segments(disk.as_ref());
    assert!(left.iter().all(|number| *number >= table.from), "{left:?}");
    assert!(first.iter().all(|number| *number < table.from), "{first:?}");

    drop((west, east, home));
    let expected = world(&store, &gap(), 2, "in the store that ran");
    let states: Vec<_> = expected
        .restored
        .iter()
        .map(|(_, restored)| (state_of(restored), restored.deltas.len()))
        .collect();
    assert_eq!(
        states,
        [
            (Some((1, whole("west", 1))), 0),
            (Some((1, whole("east", 1))), 0),
            (Some((1, whole("home", 1))), 0)
        ]
    );
    after_every_crash(&disk, &gap(), "checkpoints of all", |store, case| {
        assert_eq!(world(store, &gap(), 3, case), expected, "{case}");
    });
    starts_again_alike(&disk, &gap(), "checkpoints of all");
}

/// Scenario 11 after a crash that brings a removed segment back ("Found while
/// building", C1.4): the segments below the table file's `from` are put back by hand,
/// as a machine can find them whose removal never reached the disk. They change
/// nothing, at this start or after the regions have gone on.
#[test]
fn segments_from_before_the_table_file_that_a_crash_brings_back_change_nothing() {
    let (store, disk) = gap_store();
    let [west, east, home] = before_the_last_checkpoint(&store);
    store.regions().unwrap();
    let old: Vec<(u64, Vec<u8>)> = segments(disk.as_ref())
        .into_iter()
        .map(|number| (number, disk.read(&segment_path(number)).unwrap().unwrap()))
        .collect();
    save(&home, ORIGIN, &built(ORIGIN, &[GLASS]));
    checkpoint(&home, 1, "home");
    home.flush();
    store.regions().unwrap();
    let from = table_file(disk.as_ref()).from;
    assert!(old.iter().all(|(number, _)| *number < from), "{from}");
    drop((west, east, home));
    let expected = world(&store, &gap(), 2, "in the store that ran");
    drop(store);

    let back = Arc::new(disk.crashed(Survival::Everything));
    for (number, bytes) in &old {
        put(&back, &segment_path(*number), bytes);
    }
    let store = store_on(&back, &gap()).unwrap();
    assert_eq!(world(&store, &gap(), 3, "segments back"), expected);

    // The regions go on: the chunk changes hands, and the old `Granted` of it is still
    // in the segment that came back.
    let (west, _) = opened_gap(&store, 0, 4);
    let (east, _) = opened_gap(&store, 1, 4);
    give_back(&west, &[FREE]);
    west.flush();
    assert_eq!(claim(&east, &[FREE]), (vec![FREE], vec![]));
    log(&east, 2, &[]);
    committed(&east, 2);
    after_every_crash(&back, &gap(), "gone on", |store, case| {
        assert_eq!(held(store, 0, 5), [], "{case}");
        assert_eq!(held(store, 1, 5), [(FREE, 1), (FREE_TOO, 0)], "{case}");
        assert_eq!(held(store, 2, 5), [(ORIGIN, 0)], "{case}");
    });
    starts_again_alike(&back, &gap(), "gone on");
}

/// Scenario 12: a merge is declined, each time with its reason and nothing changed.
#[test]
fn a_merge_that_may_not_be_is_declined_with_its_reason_and_changes_nothing() {
    let (store, disk) = gap_store();
    let (west, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 4);
    assert_eq!(claim(&east, &[FREE]), (vec![FREE], vec![]));
    let before = store.regions().unwrap();
    let unchanged = |case: &str| {
        assert_eq!(store.regions().unwrap(), before, "{case}");
    };

    // The region itself, and regions there are none of.
    assert_eq!(merge(&west, 0, 1, 1), declined(Decline::NoSuchRegion));
    assert_eq!(merge(&west, 3, 1, 1), declined(Decline::NoSuchRegion));
    assert_eq!(merge(&west, 77, 1, 1), declined(Decline::NoSuchRegion));
    // The home region.
    assert_eq!(merge(&west, 2, 1, 1), declined(Decline::Home));
    unchanged("the region itself, no region, home");

    // Open with another epoch than named, lower or higher.
    for named in [3, 5] {
        assert_eq!(
            merge(&west, 1, named, 1),
            declined(Decline::NotOpened { epoch: Some(4) })
        );
    }
    unchanged("another epoch");

    // The region to absorb has a commit behind its checkpoint.
    log(&east, 1, &[]);
    assert_eq!(
        merge(&west, 1, 4, 1),
        declined(Decline::Uncheckpointed {
            region: RegionId(1)
        })
    );
    checkpoint(&east, 1, "east");
    east.flush();
    // The survivor has: none at all, and one that covers only the first of two.
    log(&west, 1, &[]);
    log(&west, 2, &[]);
    for covered in [false, true] {
        if covered {
            checkpoint(&west, 1, "west");
            west.flush();
        }
        assert_eq!(
            merge(&west, 1, 4, 3),
            declined(Decline::Uncheckpointed {
                region: RegionId(0)
            })
        );
    }
    checkpoint(&west, 2, "west");
    west.flush();
    unchanged("commits behind a checkpoint");

    // A tick not above one the session named in a commit.
    for tick in [0, 1, 2] {
        assert_eq!(
            merge(&west, 1, 4, tick),
            declined(Decline::Tick { named: 2 })
        );
    }
    // Or in a checkpoint.
    checkpoint(&west, 6, "west");
    west.flush();
    for tick in [3, 6] {
        assert_eq!(
            merge(&west, 1, 4, tick),
            declined(Decline::Tick { named: 6 })
        );
    }
    // Or was restored up to.
    drop(west);
    let (west, restored) = opened_gap(&store, 0, 1);
    assert_eq!(restored.tick(), 6);
    assert_eq!(merge(&west, 1, 4, 6), declined(Decline::Tick { named: 6 }));
    unchanged("ticks that were named");

    // Not open at all: given up by its owner.
    drop(east);
    assert_eq!(
        merge(&west, 1, 4, 7),
        declined(Decline::NotOpened { epoch: None })
    );
    unchanged("given up");

    // Nothing has changed for either, in this store or after a crash, and the handle
    // is as it was: the merge that may be, is.
    assert!(!west.is_lost());
    let merges = |record: &LogRecord| matches!(record, LogRecord::Absorbed { .. });
    assert!(!records(disk.as_ref()).iter().any(merges));
    after_every_crash(&disk, &gap(), "declined", |store, case| {
        let found = world(store, &gap(), 9, case);
        assert_eq!(found.list.absorbed, [], "{case}");
        let states: Vec<_> = found
            .restored
            .iter()
            .map(|(_, restored)| state_of(restored))
            .collect();
        let expected = [
            Some((6, whole("west", 6))),
            Some((1, whole("east", 1))),
            None,
        ];
        assert_eq!(states, expected, "{case}");
        assert_eq!(found.restored[1].1.held, [(FREE, 0)], "{case}");
    });
    let (_east, _) = opened_gap(&store, 1, 4);
    assert_eq!(
        merge(&west, 1, 4, 7),
        StoreReply::Absorbed {
            absorbed: RegionId(1),
            chunks: vec![FREE]
        }
    );
}

/// Scenario 12: a merge is declined for a tick that a checkpoint named which is still
/// with the thread for chunks, and so is a split.
#[test]
fn a_merge_or_a_split_is_declined_for_a_tick_that_a_checkpoint_under_way_has_named() {
    let (store, gate, disk) = gated_gap();
    let (west, _) = opened_gap(&store, 0, 1);
    let (_east, _) = opened_gap(&store, 1, 1);
    let before = store.regions().unwrap();

    gate.hold_at(Stop::BeforeSync);
    checkpoint(&west, 7, "early");
    gate.wait_until_there();
    for tick in [6, 7] {
        assert_eq!(
            merge(&west, 1, 1, tick),
            declined(Decline::Tick { named: 7 })
        );
        assert_eq!(
            split(&west, tick, &[WEST], 1),
            declined(Decline::Tick { named: 7 })
        );
    }
    assert_eq!(store.regions().unwrap(), before);
    assert!(!exists(disk.as_ref(), "/world/regions/0.state"));
    gate.let_go();
    west.flush();
    // In place, it has still named the tick.
    assert_eq!(merge(&west, 1, 1, 7), declined(Decline::Tick { named: 7 }));
    drop(west);
    let (_, restored) = opened_gap(&store, 0, 2);
    assert_eq!(state_of(&restored), Some((7, whole("early", 7))));
}

/// The record's order of the reasons ("Found while building", C1.5): a merge that has
/// several against it is declined for the first of `NoSuchRegion`, `Home`,
/// `NotOpened`, `Uncheckpointed` (the survivor before the region to absorb), `Tick`,
/// `TooLarge`.
#[test]
fn a_merge_with_several_reasons_against_it_is_declined_for_the_first_of_them() {
    let (store, _disk) = gap_store();
    let (west, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 4);
    // Both have a commit behind their checkpoints, and tick 1 is named.
    log(&west, 1, &[]);
    log(&east, 1, &[]);
    let uncheckpointed = |region| {
        declined(Decline::Uncheckpointed {
            region: RegionId(region),
        })
    };

    // No such region, before a commit behind a checkpoint and a tick that was named.
    assert_eq!(merge(&west, 0, 9, 1), declined(Decline::NoSuchRegion));
    assert_eq!(merge(&west, 77, 9, 0), declined(Decline::NoSuchRegion));
    // The home region, which nobody has opened either.
    assert_eq!(merge(&west, 2, 9, 0), declined(Decline::Home));
    // Not opened with that epoch, before the commits and the tick.
    assert_eq!(
        merge(&west, 1, 3, 0),
        declined(Decline::NotOpened { epoch: Some(4) })
    );
    // The survivor's commit before the other's, and both before the tick.
    assert_eq!(merge(&west, 1, 4, 0), uncheckpointed(0));
    checkpoint(&west, 1, "west");
    west.flush();
    assert_eq!(merge(&west, 1, 4, 0), uncheckpointed(1));
    checkpoint(&east, 1, "east");
    east.flush();
    assert_eq!(merge(&west, 1, 4, 1), declined(Decline::Tick { named: 1 }));

    // A state that no record of the log has room for: the tick first.
    let too_large = |tick| StoreRequest::AbsorbCommit {
        absorbed: RegionId(1),
        absorbed_epoch: 4,
        tick,
        state: vec![0; 64 * 1024 * 1024],
    };
    west.request(too_large(1));
    assert_eq!(reply(&west), declined(Decline::Tick { named: 1 }));
    west.request(too_large(2));
    assert_eq!(reply(&west), declined(Decline::TooLarge));
    // Declined, the handle is as it was, and a merge that may be, is.
    assert_eq!(
        merge(&west, 1, 4, 2),
        StoreReply::Absorbed {
            absorbed: RegionId(1),
            chunks: vec![]
        }
    );
}

/// Scenario 13, after `Absorbed`: the survivor is restored with the state and tick of
/// the request and no deltas; its `held` has what the other was granted, with the
/// merge's tick; it is pinned to both areas; the absorbed region's handle is lost and
/// its hello is refused with `Absorbed { into }`; the list has it among `absorbed` and
/// not among `regions`; the survivor loads and saves chunks of both areas.
#[test]
fn a_merge_gives_the_survivor_everything_the_absorbed_region_had() {
    let (store, disk) = gap_store();
    let (west, west_restored) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 4);
    // The survivor has a grant of its own, from its tick 1; the other has two, and has
    // saved a chunk of its area and one it was granted.
    log(&west, 1, &[]);
    assert_eq!(claim(&west, &[FREE_THREE]), (vec![FREE_THREE], vec![]));
    checkpoint(&west, 1, "west");
    west.flush();
    assert_eq!(
        claim(&east, &[FREE_TOO, FREE]),
        (vec![FREE_TOO, FREE], vec![])
    );
    log(&east, 1, &[change(EAST, GLASS), change(FREE, GLASS)]);
    save(&east, EAST, &built(EAST, &[GLASS]));
    save(&east, FREE, &built(FREE, &[GLASS]));
    checkpoint(&east, 1, "east");
    east.flush();

    assert_eq!(
        merge(&west, 1, 4, 5),
        StoreReply::Absorbed {
            absorbed: RegionId(1),
            chunks: vec![FREE, FREE_TOO]
        }
    );
    assert!(east.is_lost());
    assert!(!west.is_lost());
    // Nothing the absorbed region's old owner asks is done or answered.
    save(&east, EAST, &built(EAST, &[OTHER]));
    east.request(StoreRequest::Load { position: EAST });
    assert_eq!(answers_until_lost(&east), []);

    west.flush();
    // The absorbed region's state goes; its region file has a block of entity ids,
    // which is never issued again, and stays.
    assert!(!exists(disk.as_ref(), "/world/regions/1.state"));
    assert!(exists(disk.as_ref(), "/world/regions/1.region"));
    // One record is the merge, with the epoch of the owner that asked.
    let record = LogRecord::Absorbed {
        region: 0,
        epoch: 1,
        absorbed: 1,
        tick: 5,
        state: whole("merged", 5),
    };
    let merges = |record: &&LogRecord| matches!(record, LogRecord::Absorbed { .. });
    let log = records(disk.as_ref());
    assert_eq!(log.iter().filter(merges).collect::<Vec<_>>(), [&record]);

    let both = areas(&[west_area(), east_area()]);
    let check = |store: &Store, epoch: u64, case: &str| {
        let list = store.regions().unwrap();
        assert_eq!(list.home, RegionId(2), "{case}");
        assert_eq!(list.absorbed, [(RegionId(1), RegionId(0))], "{case}");
        let living: Vec<u32> = list.regions.iter().map(|info| info.region.0).collect();
        assert_eq!(living, [0, 2], "{case}");
        let survivor = &list.regions[0];
        assert_eq!(areas(&survivor.pinned), both, "{case}");
        let bounds = ChunkBox {
            min: ChunkPos::new(5, -2),
            max: ChunkPos::new(9, 5),
        };
        assert_eq!(survivor.bounds, Some(bounds), "{case}");

        for offered in [1, 4, epoch, 1000] {
            let refused = store.open_region(hello_of(&gap(), 1, offered));
            assert!(
                matches!(
                    refused,
                    Err(StoreError::Absorbed {
                        region: RegionId(1),
                        into: RegionId(0)
                    })
                ),
                "{case}: {:?}",
                refused.err()
            );
        }
        let (west, restored) = opened_gap(store, 0, epoch);
        assert_eq!(state_of(&restored), Some((5, whole("merged", 5))), "{case}");
        assert_eq!(restored.deltas, [], "{case}");
        assert_eq!(
            restored.held,
            [(FREE, 5), (FREE_TOO, 5), (FREE_THREE, 1)],
            "{case}"
        );
        assert_eq!(areas(&restored.pinned), both, "{case}");
        assert_eq!(restored.entity_ids, west_restored.entity_ids, "{case}");
        // The survivor loads chunks of both areas and what the other was granted, as
        // the other left them.
        assert_eq!(load(&west, EAST), built(EAST, &[GLASS]), "{case}");
        assert_eq!(load(&west, WEST), built(WEST, &[]), "{case}");
        assert_eq!(load(&west, FREE), built(FREE, &[GLASS]), "{case}");
        assert_eq!(load(&west, FREE_TOO), built(FREE_TOO, &[]), "{case}");
    };
    after_every_crash(&disk, &gap(), "merged", |store, case| check(store, 2, case));
    starts_again_alike(&disk, &gap(), "merged");

    // And saves them.
    let east_too = ChunkPos::new(21, 0);
    for chunk in [east_too, WEST_TOO, FREE_THREE] {
        save(&west, chunk, &built(chunk, &[OTHER]));
        assert_eq!(load(&west, chunk), built(chunk, &[OTHER]));
    }
    drop(west);
    check(&store, 2, "in the store that merged");
}

/// Scenario 13 over a connection: the hello for an absorbed region is answered with
/// `StoreWelcome::Absorbed`, and the survivor is restored with what it was granted.
#[test]
fn a_hello_for_an_absorbed_region_is_refused_over_a_connection_with_the_survivor() {
    let (store, _disk) = gap_store();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let server = serve(store.clone(), listener).unwrap();
    let address = server.local_addr().to_string();
    let (west, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 1);
    assert_eq!(claim(&east, &[FREE]), (vec![FREE], vec![]));
    assert_eq!(
        merge(&west, 1, 1, 3),
        StoreReply::Absorbed {
            absorbed: RegionId(1),
            chunks: vec![FREE]
        }
    );

    let mut connection = std::net::TcpStream::connect(&address).unwrap();
    let said = clustine_rpc::StoreHello::Region(hello_of(&gap(), 1, 2));
    clustine_rpc::wire::blocking::write(&mut connection, &said).unwrap();
    let said: Option<clustine_rpc::StoreWelcome> =
        clustine_rpc::wire::blocking::read(&mut connection).unwrap();
    let expected = clustine_rpc::StoreWelcome::Absorbed { into: RegionId(0) };
    assert_eq!(said, Some(expected));

    drop(west);
    let (_remote, restored) = StoreHandle::connect(&address, hello_of(&gap(), 0, 2)).unwrap();
    assert_eq!(state_of(&restored), Some((3, whole("merged", 3))));
    assert_eq!(restored.held, [(FREE, 3)]);
    assert_eq!(areas(&restored.pinned), areas(&[west_area(), east_area()]));
    assert_eq!(regions(&address).unwrap(), store.regions().unwrap());
}

/// Scenario 14: commits of the survivor after a merge are restored as deltas on the
/// merged state; a checkpoint after them leaves no record of the merge needed.
#[test]
fn commits_after_a_merge_are_deltas_on_the_merged_state_until_a_checkpoint() {
    let (store, disk) = gap_store();
    let (west, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 1);
    assert_eq!(claim(&east, &[FREE]), (vec![FREE], vec![]));
    east.flush();
    assert_eq!(
        merge(&west, 1, 1, 5),
        StoreReply::Absorbed {
            absorbed: RegionId(1),
            chunks: vec![FREE]
        }
    );
    // What came with the merge is held from its tick, or from the beginning: changes
    // behind it are replayed into the chunks of both areas and into the granted one.
    log(
        &west,
        6,
        &[
            change(EAST, GLASS),
            change(FREE, STONE),
            change(WEST, OTHER),
        ],
    );
    log(&west, 7, &[change(EAST, OTHER)]);
    committed(&west, 7);
    drop(west);

    let deltas = |store: &Store, epoch: u64, case: &str| {
        let (west, restored) = opened_gap(store, 0, epoch);
        assert_eq!(state_of(&restored), Some((5, whole("merged", 5))), "{case}");
        assert_eq!(ticks(&restored), [6, 7], "{case}");
        assert_eq!(load(&west, EAST), built(EAST, &[GLASS, OTHER]), "{case}");
        assert_eq!(load(&west, FREE), built(FREE, &[STONE]), "{case}");
        assert_eq!(load(&west, WEST), built(WEST, &[OTHER]), "{case}");
        west.flush();
    };
    after_every_crash(&disk, &gap(), "committed on", |store, case| {
        deltas(store, 2, case)
    });
    deltas(&store, 2, "in the store that merged");
    let merges = |record: &LogRecord| matches!(record, LogRecord::Absorbed { .. });
    assert!(records(disk.as_ref()).iter().any(merges));

    // A checkpoint of the survivor: its state file is the latest whole state, no
    // region needs the record of the merge, and the table file has what it said.
    let (west, _) = opened_gap(&store, 0, 3);
    save(&west, EAST, &built(EAST, &[GLASS, OTHER]));
    save(&west, FREE, &built(FREE, &[STONE]));
    save(&west, WEST, &built(WEST, &[OTHER]));
    checkpoint(&west, 7, "after");
    west.flush();
    store.regions().unwrap();
    assert!(!records(disk.as_ref()).iter().any(merges));
    let table = table_file(disk.as_ref());
    assert_eq!(table.absorbed, [(1, 0)]);
    let ids: Vec<u32> = table.regions.iter().map(|region| region.id).collect();
    assert_eq!(ids, [0, 2]);
    assert_eq!(table.regions[0].grants, [(FREE, 5)]);
    assert_eq!(
        areas(&table.regions[0].pinned),
        areas(&[west_area(), east_area()])
    );
    assert!(
        segments(disk.as_ref())
            .iter()
            .all(|number| *number >= table.from)
    );
    drop(west);

    let checkpointed = |store: &Store, epoch: u64, case: &str| {
        let list = store.regions().unwrap();
        assert_eq!(list.absorbed, [(RegionId(1), RegionId(0))], "{case}");
        let (west, restored) = opened_gap(store, 0, epoch);
        assert_eq!(state_of(&restored), Some((7, whole("after", 7))), "{case}");
        assert_eq!(restored.deltas, [], "{case}");
        assert_eq!(restored.held, [(FREE, 5)], "{case}");
        assert_eq!(load(&west, EAST), built(EAST, &[GLASS, OTHER]), "{case}");
        let refused = store.open_region(hello_of(&gap(), 1, epoch));
        assert!(
            matches!(refused, Err(StoreError::Absorbed { .. })),
            "{case}"
        );
    };
    after_every_crash(&disk, &gap(), "checkpointed", |store, case| {
        checkpointed(store, 4, case)
    });
    starts_again_alike(&disk, &gap(), "checkpointed");
    checkpointed(&store, 4, "in the store that merged");
}

/// Scenario 15: a checkpoint with the merge's or the split's own tick, or a lower one,
/// asked for afterwards, is not put in place, and the region is restored with the
/// state of the record all the same; so is the part.
#[test]
fn a_checkpoint_with_the_tick_of_a_merge_or_a_split_is_not_put_in_place() {
    let (store, disk) = gap_store();
    let (west, _) = opened_gap(&store, 0, 1);
    let (_east, _) = opened_gap(&store, 1, 1);
    assert_eq!(
        merge(&west, 1, 1, 5),
        StoreReply::Absorbed {
            absorbed: RegionId(1),
            chunks: vec![]
        }
    );
    checkpoint(&west, 5, "impostor");
    checkpoint(&west, 4, "older");
    west.flush();
    assert!(!exists(disk.as_ref(), "/world/regions/0.state"));
    let merged = |store: &Store, case: &str| {
        let (_, restored) = opened_gap(store, 0, 2);
        assert_eq!(state_of(&restored), Some((5, whole("merged", 5))), "{case}");
        assert_eq!(restored.deltas, [], "{case}");
    };
    after_every_crash(&disk, &gap(), "merged", merged);
    drop(west);
    merged(&store, "in the store that merged");

    let (west, _) = opened_gap(&store, 0, 3);
    assert_eq!(split(&west, 6, &[WEST], 2), parted(3));
    let (part, _) = opened_gap(&store, 3, 2);
    checkpoint(&west, 6, "impostor");
    checkpoint(&part, 6, "impostor");
    checkpoint(&part, 5, "older");
    west.flush();
    part.flush();
    assert!(!exists(disk.as_ref(), "/world/regions/0.state"));
    assert!(!exists(disk.as_ref(), "/world/regions/3.state"));
    let parts = |store: &Store, case: &str| {
        let (_, restored) = opened_gap(store, 0, 4);
        assert_eq!(state_of(&restored), Some((6, whole("rest", 6))), "{case}");
        let (_, restored) = opened_gap(store, 3, 4);
        assert_eq!(state_of(&restored), Some((6, whole("part", 6))), "{case}");
        assert_eq!(restored.held, [(WEST, 6)], "{case}");
    };
    after_every_crash(&disk, &gap(), "split", parts);
    drop((west, part));
    parts(&store, "in the store that split");

    // A checkpoint above the record's tick is put in place, for each of the two.
    let (west, _) = opened_gap(&store, 0, 5);
    let (part, _) = opened_gap(&store, 3, 5);
    checkpoint(&west, 7, "above");
    checkpoint(&part, 8, "above");
    west.flush();
    part.flush();
    after_every_crash(&disk, &gap(), "checkpoints above", |store, case| {
        let (_, restored) = opened_gap(store, 0, 6);
        assert_eq!(state_of(&restored), Some((7, whole("above", 7))), "{case}");
        let (_, restored) = opened_gap(store, 3, 6);
        assert_eq!(state_of(&restored), Some((8, whole("above", 8))), "{case}");
        assert_eq!(restored.held, [(WEST, 6)], "{case}");
    });
}

/// Scenarios 12 and 15 for the survivor: a checkpoint that is still with the thread
/// for chunks when the merge is written, which names a lower tick as it must, is not
/// put in place when it comes back.
#[test]
fn a_checkpoint_of_the_survivor_under_way_when_the_merge_is_written_is_not_put_in_place() {
    let (store, gate, disk) = gated_gap();
    let (west, _) = opened_gap(&store, 0, 1);
    let (_east, _) = opened_gap(&store, 1, 1);

    gate.hold_at(Stop::BeforeSync);
    checkpoint(&west, 3, "early");
    gate.wait_until_there();
    assert_eq!(
        merge(&west, 1, 1, 4),
        StoreReply::Absorbed {
            absorbed: RegionId(1),
            chunks: vec![]
        }
    );
    gate.let_go();
    west.flush();
    assert!(!exists(disk.as_ref(), "/world/regions/0.state"));
    let merged = |store: &Store, case: &str| {
        let (_, restored) = opened_gap(store, 0, 2);
        assert_eq!(state_of(&restored), Some((4, whole("merged", 4))), "{case}");
        assert_eq!(restored.deltas, [], "{case}");
        assert!(merged_as_a_whole(store, 4, case), "{case}");
    };
    after_every_crash(&disk, &gap(), "merged", merged);
    starts_again_alike(&disk, &gap(), "merged");
    drop(west);
    merged(&store, "in the store that merged");
}

/// Scenario 22: a checkpoint of the absorbed region that is still under way when the
/// merge is written is not put in place. What that region asked the thread for chunks
/// to do before the merge is done before anything the survivor asks after it.
#[test]
fn a_checkpoint_of_the_absorbed_region_under_way_when_the_merge_is_written_is_not_put_in_place() {
    let (store, gate, disk) = gated_gap();
    let (west, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 1);

    gate.hold_at(Stop::BeforeSave(EAST));
    save(&east, EAST, &built(EAST, &[GLASS]));
    checkpoint(&east, 9, "east, too late");
    gate.wait_until_there();
    assert_eq!(
        merge(&west, 1, 1, 4),
        StoreReply::Absorbed {
            absorbed: RegionId(1),
            chunks: vec![]
        }
    );
    assert!(east.is_lost());
    west.request(StoreRequest::Load { position: EAST });
    gate.let_go();
    assert_eq!(
        reply(&west),
        StoreReply::Loaded {
            position: EAST,
            chunk: built(EAST, &[GLASS])
        }
    );
    west.flush();
    assert!(!exists(disk.as_ref(), "/world/regions/1.state"));
    assert!(!exists(disk.as_ref(), "/world/regions/0.state"));
    after_every_crash(&disk, &gap(), "merged", |store, case| {
        assert!(merged_as_a_whole(store, 4, case), "{case}");
    });
    starts_again_alike(&disk, &gap(), "merged");
    assert!(merged_as_a_whole(&store, 4, "in the store that merged"));
}

/// Scenarios 12 and 15 for a split: a checkpoint of the region that is under way when
/// the split is written is not put in place, and neither is one of the part with the
/// split's tick, asked for while the thread for chunks is still held.
#[test]
fn a_checkpoint_under_way_when_the_split_is_written_is_not_put_in_place_for_either_region() {
    let (store, gate, disk) = gated_gap();
    let (west, _) = opened_gap(&store, 0, 1);

    gate.hold_at(Stop::BeforeSync);
    checkpoint(&west, 3, "early");
    gate.wait_until_there();
    assert_eq!(split(&west, 4, &[WEST], 2), parted(3));
    let (part, restored) = opened_while_held(&store, 3, 2);
    assert_eq!(state_of(&restored), Some((4, whole("part", 4))));
    checkpoint(&part, 4, "impostor");
    checkpoint(&part, 3, "older");
    checkpoint(&west, 4, "impostor");
    gate.let_go();
    west.flush();
    part.flush();
    assert!(!exists(disk.as_ref(), "/world/regions/0.state"));
    assert!(!exists(disk.as_ref(), "/world/regions/3.state"));
    let parts = |store: &Store, case: &str| {
        let (_, restored) = opened_gap(store, 0, 3);
        assert_eq!(state_of(&restored), Some((4, whole("rest", 4))), "{case}");
        let (_, restored) = opened_gap(store, 3, 3);
        assert_eq!(state_of(&restored), Some((4, whole("part", 4))), "{case}");
        assert_eq!(restored.held, [(WEST, 4)], "{case}");
    };
    after_every_crash(&disk, &gap(), "split", parts);
    starts_again_alike(&disk, &gap(), "split");
    drop((west, part));
    parts(&store, "in the store that split");
}

/// Scenario 16: a split is declined, each time with its reason and nothing changed,
/// for: a commit behind the checkpoint; a tick not above one the session named; no
/// chunks; a chunk not held; the home chunk; epoch 0.
#[test]
fn a_split_that_may_not_be_is_declined_with_its_reason_and_changes_nothing() {
    let (store, disk) = gap_store();
    let (west, _) = opened_gap(&store, 0, 1);
    let (home, _) = opened_gap(&store, 2, 1);
    assert_eq!(claim(&west, &[FREE]), (vec![FREE], vec![]));
    assert_eq!(claim(&home, &[FREE_TOO]), (vec![FREE_TOO], vec![]));
    let before = store.regions().unwrap();

    log(&west, 1, &[]);
    assert_eq!(
        split(&west, 2, &[WEST], 1),
        declined(Decline::Uncheckpointed {
            region: RegionId(0)
        })
    );
    checkpoint(&west, 1, "west");
    west.flush();
    for tick in [0, 1] {
        assert_eq!(
            split(&west, tick, &[WEST], 1),
            declined(Decline::Tick { named: 1 })
        );
    }
    assert_eq!(split(&west, 2, &[], 1), declined(Decline::Malformed));
    assert_eq!(split(&west, 2, &[WEST], 0), declined(Decline::Malformed));
    // A chunk of another region's area, one that is nobody's, one another region was
    // granted, and one among chunks it does hold.
    for chunk in [EAST, FREE_THREE, FREE_TOO] {
        assert_eq!(
            split(&west, 2, &[chunk], 1),
            declined(Decline::NotHeld { chunk })
        );
        assert_eq!(
            split(&west, 2, &[WEST, chunk, FREE], 1),
            declined(Decline::NotHeld { chunk })
        );
    }
    // The home chunk, by the region that holds it, alone or with another.
    assert_eq!(split(&home, 1, &[ORIGIN], 1), declined(Decline::Home));
    assert_eq!(
        split(&home, 1, &[FREE_TOO, ORIGIN], 1),
        declined(Decline::Home)
    );

    assert_eq!(store.regions().unwrap(), before);
    assert!(!west.is_lost() && !home.is_lost());
    let splits = |record: &LogRecord| matches!(record, LogRecord::Split { .. });
    assert!(!records(disk.as_ref()).iter().any(splits));
    after_every_crash(&disk, &gap(), "declined", |store, case| {
        let found = world(store, &gap(), 9, case);
        assert_eq!(found.list.regions.len(), 3, "{case}");
        assert_eq!(found.restored[0].1.held, [(FREE, 0)], "{case}");
        let state = state_of(&found.restored[0].1);
        assert_eq!(state, Some((1, whole("west", 1))), "{case}");
    });
    // The handle is as it was, and no id was used up.
    assert_eq!(split(&west, 2, &[WEST, FREE], 1), parted(3));
    assert_eq!(split(&home, 1, &[FREE_TOO], 1), parted(4));
}

/// The record's order of the reasons ("Found while building", C1.5): a split that has
/// several against it is declined for the first of `Uncheckpointed`, `Tick`,
/// `Malformed`, `NotHeld`, `Home`, `TooLarge`.
#[test]
fn a_split_with_several_reasons_against_it_is_declined_for_the_first_of_them() {
    let (store, _disk) = gap_store();
    let (west, _) = opened_gap(&store, 0, 1);
    let (home, _) = opened_gap(&store, 2, 1);
    log(&west, 1, &[]);
    // A commit behind the checkpoint, before a tick that was named, no chunks, epoch 0.
    assert_eq!(
        split(&west, 1, &[], 0),
        declined(Decline::Uncheckpointed {
            region: RegionId(0)
        })
    );
    checkpoint(&west, 1, "west");
    west.flush();
    // The tick, before what is malformed, not held or home.
    assert_eq!(
        split(&west, 1, &[], 0),
        declined(Decline::Tick { named: 1 })
    );
    assert_eq!(
        split(&west, 1, &[EAST, ORIGIN], 1),
        declined(Decline::Tick { named: 1 })
    );
    // Malformed, before a chunk that is not held and the home chunk.
    assert_eq!(split(&west, 2, &[EAST], 0), declined(Decline::Malformed));
    assert_eq!(split(&west, 2, &[ORIGIN], 0), declined(Decline::Malformed));
    assert_eq!(split(&home, 1, &[ORIGIN], 0), declined(Decline::Malformed));
    // The home chunk asked for by a region that does not hold it is a chunk not held.
    assert_eq!(
        split(&west, 2, &[ORIGIN], 1),
        declined(Decline::NotHeld { chunk: ORIGIN })
    );
    // A chunk not held beside the home chunk, by the home region.
    assert_eq!(
        split(&home, 1, &[ORIGIN, FREE], 1),
        declined(Decline::NotHeld { chunk: FREE })
    );

    // States that no record of the log has room for: every other reason first.
    let too_large = |tick, chunks: &[ChunkPos]| StoreRequest::SplitCommit {
        tick,
        state: vec![0; 64 * 1024 * 1024],
        part: SplitPart {
            chunks: chunks.to_vec(),
            state: Vec::new(),
        },
        as_epoch: 1,
    };
    home.request(too_large(1, &[ORIGIN]));
    assert_eq!(reply(&home), declined(Decline::Home));
    west.request(too_large(2, &[FREE]));
    assert_eq!(reply(&west), declined(Decline::NotHeld { chunk: FREE }));
    west.request(too_large(2, &[WEST]));
    assert_eq!(reply(&west), declined(Decline::TooLarge));
    // The part's state counts as the region's does.
    west.request(StoreRequest::SplitCommit {
        tick: 2,
        state: Vec::new(),
        part: SplitPart {
            chunks: vec![WEST],
            state: vec![0; 64 * 1024 * 1024],
        },
        as_epoch: 1,
    });
    assert_eq!(reply(&west), declined(Decline::TooLarge));
    assert_eq!(split(&west, 2, &[WEST], 1), parted(3));
}

/// Scenario 17, after `Split { region: N }`: `N` is higher than every id there was;
/// the list has it with `as_epoch`, not pinned, its bounds around the part's chunks; a
/// hello for `N` with `as_epoch` is accepted and restored with the part's state at the
/// split's tick, `held` the part's chunks with that tick, the empty block of entity
/// ids; a hello with a lower epoch is `EpochRefused { seen: as_epoch }`; the old region
/// is restored with its state of the request, and gets `NotHeld { holder: Some(N) }`
/// for the part's chunks.
#[test]
fn a_split_makes_a_region_of_the_part_that_only_its_epoch_opens() {
    let (store, disk) = gap_store();
    let (west, _) = opened_gap(&store, 0, 1);
    log(&west, 1, &[]);
    assert_eq!(
        claim(&west, &[FREE, FREE_THREE]),
        (vec![FREE, FREE_THREE], vec![])
    );
    save(&west, WEST, &built(WEST, &[GLASS]));
    checkpoint(&west, 1, "west");
    west.flush();
    // Chunks of its area and one it was granted, not in their order and one twice.
    let part = [WEST_TOO, FREE, WEST, WEST_TOO];
    assert_eq!(split(&west, 5, &part, 7), parted(3));
    assert!(!west.is_lost());

    let check = |store: &Store, epochs: [u64; 2], case: &str| {
        let list = store.regions().unwrap();
        let living: Vec<u32> = list.regions.iter().map(|info| info.region.0).collect();
        assert_eq!(living, [0, 1, 2, 3], "{case}");
        let made = &list.regions[3];
        assert!(made.epoch >= 7, "{case}: {made:?}");
        assert_eq!(made.pinned, [], "{case}");
        let bounds = ChunkBox {
            min: ChunkPos::new(-3, 0),
            max: ChunkPos::new(5, 5),
        };
        assert_eq!(made.bounds, Some(bounds), "{case}");
        let rest = ChunkBox {
            min: FREE_THREE,
            max: FREE_THREE,
        };
        assert_eq!(list.regions[0].bounds, Some(rest), "{case}");
        assert_eq!(list.regions[0].pinned, [west_area()], "{case}");

        for lower in [0, 1, 6] {
            let refused = store.open_region(hello_of(&gap(), 3, lower));
            assert!(
                matches!(
                    refused,
                    Err(StoreError::EpochRefused {
                        region: RegionId(3),
                        offered,
                        seen: 7
                    }) if offered == lower
                ),
                "{case}: {:?}",
                refused.err()
            );
        }
        let (made, restored) = opened_gap(store, 3, epochs[1]);
        assert_eq!(state_of(&restored), Some((5, whole("part", 5))), "{case}");
        assert_eq!(restored.deltas, [], "{case}");
        assert_eq!(
            restored.held,
            [(WEST, 5), (WEST_TOO, 5), (FREE, 5)],
            "{case}"
        );
        assert_eq!(restored.pinned, [], "{case}");
        assert_eq!(restored.entity_ids, NO_ENTITY_IDS, "{case}");

        let (rest, restored) = opened_gap(store, 0, epochs[0]);
        assert_eq!(state_of(&restored), Some((5, whole("rest", 5))), "{case}");
        assert_eq!(restored.deltas, [], "{case}");
        assert_eq!(restored.held, [(FREE_THREE, 1)], "{case}");
        for chunk in [WEST, WEST_TOO, FREE] {
            rest.request(StoreRequest::Load { position: chunk });
            assert_eq!(reply(&rest), not_held(chunk, Some(3)), "{case}");
            save(&rest, chunk, &built(chunk, &[OTHER]));
            assert_eq!(reply(&rest), not_held(chunk, Some(3)), "{case}");
        }
        // The part has the chunks as the old region left them, and the old region
        // what is left of its area.
        assert_eq!(load(&made, WEST), built(WEST, &[GLASS]), "{case}");
        assert_eq!(load(&made, FREE), built(FREE, &[]), "{case}");
        assert_eq!(holder(&made, 3, FREE_THREE), Some(RegionId(0)), "{case}");
        let beside = ChunkPos::new(-4, 0);
        assert_eq!(load(&rest, beside), built(beside, &[]), "{case}");
        assert_eq!(holder(&made, 3, beside), Some(RegionId(0)), "{case}");
    };
    // In the list the part has the epoch of the split before anyone has opened it.
    assert_eq!(store.regions().unwrap().regions[3].epoch, 7);
    west.flush();
    // One record is the split, with each chunk of the part once.
    let record = LogRecord::Split {
        region: 0,
        epoch: 1,
        tick: 5,
        state: whole("rest", 5),
        part: 3,
        part_epoch: 7,
        chunks: vec![WEST, WEST_TOO, FREE],
        part_state: whole("part", 5),
    };
    let splits = |record: &&LogRecord| matches!(record, LogRecord::Split { .. });
    let log = records(disk.as_ref());
    assert_eq!(log.iter().filter(splits).collect::<Vec<_>>(), [&record]);
    after_every_crash(&disk, &gap(), "split", |store, case| {
        assert_eq!(store.regions().unwrap().regions[3].epoch, 7, "{case}");
        check(store, [2, 7], case)
    });
    starts_again_alike(&disk, &gap(), "split");
    drop(west);
    check(&store, [2, 7], "in the store that split");
    // A higher epoch opens the part as it opens any region.
    check(&store, [3, 8], "with a higher epoch");
}

/// Scenario 17 over a connection: the part is opened from another process with the
/// epoch of the split, and with no lower one.
#[test]
fn the_part_of_a_split_is_opened_over_a_connection_with_its_epoch() {
    let (store, _disk) = gap_store();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let server = serve(store.clone(), listener).unwrap();
    let address = server.local_addr().to_string();
    let (west, _) = opened_gap(&store, 0, 1);
    assert_eq!(split(&west, 5, &[WEST, WEST_TOO], 7), parted(3));

    let refused = StoreHandle::connect(&address, hello_of(&gap(), 3, 6));
    assert!(
        matches!(refused, Err(StoreError::EpochRefused { seen: 7, .. })),
        "{:?}",
        refused.err()
    );
    let (remote, restored) = StoreHandle::connect(&address, hello_of(&gap(), 3, 7)).unwrap();
    assert_eq!(state_of(&restored), Some((5, whole("part", 5))));
    assert_eq!(restored.held, [(WEST, 5), (WEST_TOO, 5)]);
    assert_eq!(restored.pinned, []);
    assert_eq!(restored.entity_ids, NO_ENTITY_IDS);
    assert_eq!(load(&remote, WEST), built(WEST, &[]));
    assert_eq!(regions(&address).unwrap(), store.regions().unwrap());
}

/// Scenario 18: the hello for `N` is answered while the thread for chunks is held; so
/// is any hello of a region with no commit behind its checkpoint. One with a block
/// change to replay is answered only when the thread is let go.
#[test]
fn a_hello_with_nothing_to_replay_is_answered_while_the_thread_for_chunks_is_held() {
    let (store, gate, _disk) = gated_gap();
    let (west, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 1);
    let (home, _) = opened_gap(&store, 2, 1);
    // The home region has a commit that its checkpoint covers; the eastern one has a
    // block change behind its checkpoint.
    log(&home, 1, &[change(ORIGIN, GLASS)]);
    save(&home, ORIGIN, &built(ORIGIN, &[GLASS]));
    checkpoint(&home, 1, "home");
    home.flush();
    log(&east, 1, &[change(EAST, GLASS)]);
    committed(&east, 1);
    east.flush();

    gate.hold_at(Stop::BeforeSave(BUSY_EAST));
    save(&east, BUSY_EAST, &built(BUSY_EAST, &[]));
    gate.wait_until_there();

    assert_eq!(split(&west, 1, &[WEST], 3), parted(3));
    let (_part, restored) = opened_while_held(&store, 3, 3);
    assert_eq!(state_of(&restored), Some((1, whole("part", 1))));
    assert_eq!(restored.held, [(WEST, 1)]);
    let (_home, restored) = opened_while_held(&store, 2, 2);
    assert_eq!(state_of(&restored), Some((1, whole("home", 1))));
    assert_eq!(restored.deltas, []);
    let (_west, restored) = opened_while_held(&store, 0, 2);
    assert_eq!(state_of(&restored), Some((1, whole("rest", 1))));

    let waiting = open_later(&store, hello_of(&gap(), 1, 2));
    // The commit thread has dealt with the hello when it has made the list.
    store.regions().unwrap();
    assert!(matches!(waiting.try_recv(), Err(mpsc::TryRecvError::Empty)));
    gate.let_go();
    let (opened, restored) = waiting.recv().unwrap().unwrap();
    assert_eq!(ticks(&restored), [1]);
    let east = StoreHandle::local(opened, store.messages.clone());
    assert_eq!(load(&east, EAST), built(EAST, &[GLASS]));
}

/// Scenario 18 as "Found while building" (C1.5) has it: a region whose commits behind
/// its checkpoint changed no block, or only blocks of chunks it does not hold or holds
/// from a later tick, has nothing to replay and is answered by the commit thread.
#[test]
fn a_hello_whose_commits_put_nothing_into_a_chunk_it_holds_does_not_wait_either() {
    let (store, gate, _disk) = gated_gap();
    let (west, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 1);
    log(&west, 1, &[]);
    log(&west, 2, &[change(EAST, GLASS), change(FREE_TOO, GLASS)]);
    log(&west, 3, &[change(FREE, GLASS)]);
    assert_eq!(claim(&west, &[FREE]), (vec![FREE], vec![]));
    west.flush();

    gate.hold_at(Stop::BeforeSave(BUSY_EAST));
    save(&east, BUSY_EAST, &built(BUSY_EAST, &[]));
    gate.wait_until_there();
    let (_west, restored) = opened_while_held(&store, 0, 2);
    assert_eq!(ticks(&restored), [1, 2, 3]);
    assert_eq!(restored.held, [(FREE, 3)]);
    gate.let_go();
    east.flush();
}

/// Starts a store on what every kind of crash leaves of `disk`, and sees to it that
/// `chunk` is the part's there, region 3's, held from `tick`.
fn the_chunk_is_the_parts(disk: &MemoryDisk, chunk: ChunkPos, tick: u64, epoch: u64, point: &str) {
    after_every_crash(disk, &gap(), point, |store, case| {
        let (_, restored) = opened_gap(store, 3, epoch);
        assert!(
            restored.held.contains(&(chunk, tick)),
            "{case}: {restored:?}"
        );
        for region in [0, 1, 2] {
            let held = held(store, region, 50);
            assert!(!held.iter().any(|(held, _)| *held == chunk), "{case}");
        }
        let (other, _) = opened_gap(store, 1, 51);
        assert_eq!(
            claim(&other, &[chunk]),
            (vec![], vec![(chunk, RegionId(3))]),
            "{case}"
        );
    });
    starts_again_alike(disk, &gap(), point);
}

/// No record of the log on `disk` says that `chunk` was returned.
fn no_record_returns(disk: &MemoryDisk, chunk: ChunkPos) {
    for record in records(disk) {
        if let LogRecord::Returned { chunks, .. } = &record {
            assert!(!chunks.contains(&chunk), "{record:?}");
        }
    }
}

/// Scenario 19, a split of a chunk that is being returned. With the thread for chunks
/// held, before the return's sync or between that and its message: a region returns
/// two chunks and then splits a part off that has one of them. The split is answered;
/// when the thread is let go, that chunk is the part's, another region's claim of it
/// is `foreign` with the part, the other chunk is free, and the store starts again on
/// what is left.
#[test]
fn a_chunk_that_is_being_returned_goes_with_the_part_it_is_split_off_in() {
    for stop in [Stop::BeforeSync, Stop::AfterSync] {
        let (store, gate, disk) = gated_gap();
        let (owner, _) = opened_gap(&store, 0, 1);
        let (other, _) = opened_gap(&store, 1, 1);
        assert_eq!(
            claim(&owner, &[FREE, FREE_TOO]),
            (vec![FREE, FREE_TOO], vec![])
        );
        owner.flush();

        gate.hold_at(stop);
        give_back(&owner, &[FREE, FREE_TOO]);
        gate.wait_until_there();
        assert_eq!(split(&owner, 1, &[FREE], 4), parted(3), "{stop:?}");
        assert_eq!(holder(&other, 1, FREE), Some(RegionId(3)), "{stop:?}");
        assert_eq!(holder(&other, 1, FREE_TOO), Some(RegionId(0)), "{stop:?}");
        the_chunk_is_the_parts(&disk, FREE, 1, 4, &format!("held {stop:?}"));

        gate.let_go();
        owner.flush();
        assert_eq!(holder(&other, 1, FREE), Some(RegionId(3)), "{stop:?}");
        assert_eq!(holder(&other, 1, FREE_TOO), None, "{stop:?}");
        assert_eq!(
            claim(&other, &[FREE, FREE_TOO]),
            (vec![FREE_TOO], vec![(FREE, RegionId(3))]),
            "{stop:?}"
        );
        no_record_returns(&disk, FREE);
        the_chunk_is_the_parts(&disk, FREE, 1, 4, &format!("let go, {stop:?}"));
        after_every_crash(&disk, &gap(), "let go", |store, case| {
            assert_eq!(held(store, 0, 60), [], "{case}");
            assert_eq!(held(store, 1, 60), [(FREE_TOO, 0)], "{case}");
        });
    }
}

/// Scenarios 9 and 19 together: a return that was called off and made again, both of
/// them under way, and then a split of the chunk. Neither return frees the chunk,
/// wherever the thread for chunks is let go to.
#[test]
fn a_chunk_returned_twice_and_then_split_off_stays_with_the_part() {
    let (store, gate, disk) = gated_gap();
    let (owner, _) = opened_gap(&store, 0, 1);
    let (other, _) = opened_gap(&store, 1, 1);
    assert_eq!(claim(&owner, &[FREE]), (vec![FREE], vec![]));
    owner.flush();

    gate.hold_at(Stop::BeforeSync);
    give_back(&owner, &[FREE]);
    gate.wait_until_there();
    assert_eq!(claim(&owner, &[FREE]), (vec![FREE], vec![]));
    give_back(&owner, &[FREE]);
    assert_eq!(split(&owner, 1, &[FREE], 4), parted(3));
    owner.request(StoreRequest::Flush);

    for (stop, point) in [
        (
            Stop::AfterSync,
            "between the first return's sync and message",
        ),
        (Stop::BeforeSync, "the first return through"),
        (
            Stop::AfterSync,
            "between the second return's sync and message",
        ),
    ] {
        gate.on_to(stop);
        store.regions().unwrap();
        assert_eq!(holder(&other, 1, FREE), Some(RegionId(3)), "{point}");
        the_chunk_is_the_parts(&disk, FREE, 1, 4, point);
    }
    gate.let_go();
    assert_eq!(reply(&owner), StoreReply::Flushed);
    assert_eq!(holder(&other, 1, FREE), Some(RegionId(3)));
    assert_eq!(holder(&owner, 0, FREE), Some(RegionId(3)));
    no_record_returns(&disk, FREE);
    the_chunk_is_the_parts(&disk, FREE, 1, 4, "both returns through");
}

/// Scenarios 9 and 19 together, the other way round: a chunk is split off while its
/// return is under way, and the part gives it back. The old region's return frees
/// nothing; the part's does, when it is through.
#[test]
fn a_chunk_split_off_while_it_was_being_returned_is_freed_only_by_the_part() {
    let (store, gate, disk) = gated_gap();
    let (owner, _) = opened_gap(&store, 0, 1);
    let (other, _) = opened_gap(&store, 1, 1);
    assert_eq!(claim(&owner, &[FREE]), (vec![FREE], vec![]));
    save(&owner, FREE, &built(FREE, &[GLASS]));
    owner.flush();

    gate.hold_at(Stop::BeforeSync);
    give_back(&owner, &[FREE]);
    gate.wait_until_there();
    assert_eq!(split(&owner, 1, &[FREE], 4), parted(3));
    let (part, restored) = opened_while_held(&store, 3, 4);
    assert_eq!(restored.held, [(FREE, 1)]);
    give_back(&part, &[FREE]);
    part.request(StoreRequest::Flush);

    // The old region's return is through, the part's is not.
    for stop in [Stop::BeforeSync, Stop::AfterSync] {
        gate.on_to(stop);
        store.regions().unwrap();
        assert_eq!(holder(&other, 1, FREE), Some(RegionId(3)), "{stop:?}");
        the_chunk_is_the_parts(&disk, FREE, 1, 4, &format!("the part's return {stop:?}"));
        assert_eq!(part.try_reply(), None, "{stop:?}");
    }
    gate.let_go();
    assert_eq!(reply(&part), StoreReply::Flushed);
    assert_eq!(holder(&other, 1, FREE), None);
    after_every_crash(&disk, &gap(), "returned by the part", |store, case| {
        for region in [0, 1, 2, 3] {
            assert_eq!(
                held(store, region, 50).len(),
                usize::from(region == 2),
                "{case}"
            );
        }
        let (other, _) = opened_gap(store, 1, 51);
        assert_eq!(claim(&other, &[FREE]), (vec![FREE], vec![]), "{case}");
        assert_eq!(load(&other, FREE), built(FREE, &[GLASS]), "{case}");
    });
    starts_again_alike(&disk, &gap(), "returned by the part");
}

/// Section 3.6, step 4: the absorbed region's grants are the survivor's, "also those a
/// return of which was under way". The return's message, which comes when the region
/// is gone, frees nothing.
#[test]
fn a_chunk_that_is_being_returned_when_its_region_is_absorbed_goes_to_the_survivor() {
    for stop in [Stop::BeforeSync, Stop::AfterSync] {
        let (store, gate, disk) = gated_gap();
        let (west, _) = opened_gap(&store, 0, 1);
        let (east, _) = opened_gap(&store, 1, 5);
        let (home, _) = opened_gap(&store, 2, 1);
        assert_eq!(
            claim(&east, &[FREE, FREE_TOO]),
            (vec![FREE, FREE_TOO], vec![])
        );
        assert_eq!(claim(&west, &[FREE_THREE]), (vec![FREE_THREE], vec![]));
        east.flush();
        west.flush();

        // A return of the survivor's own is under way as well, and goes through.
        gate.hold_at(stop);
        give_back(&west, &[FREE_THREE]);
        gate.wait_until_there();
        give_back(&east, &[FREE]);
        assert_eq!(
            merge(&west, 1, 5, 1),
            StoreReply::Absorbed {
                absorbed: RegionId(1),
                chunks: vec![FREE, FREE_TOO]
            },
            "{stop:?}"
        );
        assert_eq!(holder(&home, 2, FREE), Some(RegionId(0)), "{stop:?}");
        assert_eq!(holder(&home, 2, FREE_THREE), Some(RegionId(0)), "{stop:?}");
        gate.let_go();
        west.flush();
        assert_eq!(holder(&home, 2, FREE), Some(RegionId(0)), "{stop:?}");
        assert_eq!(holder(&home, 2, FREE_THREE), None, "{stop:?}");
        assert_eq!(
            claim(&home, &[FREE]),
            (vec![], vec![(FREE, RegionId(0))]),
            "{stop:?}"
        );
        no_record_returns(&disk, FREE);
        after_every_crash(&disk, &gap(), &format!("{stop:?}"), |store, case| {
            assert!(merged_as_a_whole(store, 1, case), "{case}");
            assert_eq!(held(store, 0, 60), [(FREE, 1), (FREE_TOO, 1)], "{case}");
        });
        starts_again_alike(&disk, &gap(), &format!("{stop:?}"));
    }
}

/// Scenario 20: a part split off a pinned region and then returned by the part chunk
/// by chunk is the pinned region's again, without a grant.
#[test]
fn a_part_that_is_returned_chunk_by_chunk_is_the_pinned_regions_again() {
    let (store, disk) = gap_store();
    let (west, _) = opened_gap(&store, 0, 1);
    assert_eq!(split(&west, 1, &[WEST, WEST_TOO], 1), parted(3));
    let (part, _) = opened_gap(&store, 3, 1);
    assert_eq!(holder(&west, 0, WEST), Some(RegionId(3)));

    save(&part, WEST, &built(WEST, &[GLASS]));
    give_back(&part, &[WEST]);
    part.flush();
    assert_eq!(load(&west, WEST), built(WEST, &[GLASS]));
    assert_eq!(holder(&part, 3, WEST), Some(RegionId(0)));
    assert_eq!(holder(&west, 0, WEST_TOO), Some(RegionId(3)));
    // Claimed by the pinned region, it is granted as any chunk of its area is.
    assert_eq!(
        claim(&west, &[WEST, WEST_TOO]),
        (vec![WEST], vec![(WEST_TOO, RegionId(3))])
    );
    after_every_crash(&disk, &gap(), "one chunk returned", |store, case| {
        assert_eq!(held(store, 0, 2), [], "{case}");
        assert_eq!(held(store, 3, 2), [(WEST_TOO, 1)], "{case}");
        let (west, _) = opened_gap(store, 0, 3);
        assert_eq!(load(&west, WEST), built(WEST, &[GLASS]), "{case}");
        assert_eq!(holder(&west, 0, WEST_TOO), Some(RegionId(3)), "{case}");
    });

    save(&part, WEST_TOO, &built(WEST_TOO, &[STONE]));
    give_back(&part, &[WEST_TOO]);
    part.flush();
    assert_eq!(load(&west, WEST_TOO), built(WEST_TOO, &[STONE]));
    let returned = |store: &Store, epoch: u64, case: &str| {
        // The part is a region still, with nothing.
        let list = store.regions().unwrap();
        assert_eq!(list.regions.len(), 4, "{case}");
        assert_eq!(list.regions[3].bounds, None, "{case}");
        assert_eq!(list.regions[0].bounds, None, "{case}");
        assert_eq!(held(store, 3, epoch), [], "{case}");
        let (west, restored) = opened_gap(store, 0, epoch);
        assert_eq!(restored.held, [], "{case}");
        assert_eq!(load(&west, WEST), built(WEST, &[GLASS]), "{case}");
        assert_eq!(load(&west, WEST_TOO), built(WEST_TOO, &[STONE]), "{case}");
    };
    after_every_crash(&disk, &gap(), "both returned", |store, case| {
        returned(store, 2, case)
    });
    starts_again_alike(&disk, &gap(), "both returned");
    drop((west, part));
    returned(&store, 2, "in the store that split");
}

/// Scenario 20: a part that the pinned region it was split off absorbs is in that
/// region's `held`, with the merge's tick, and the part's files are gone.
#[test]
fn a_part_absorbed_by_the_pinned_region_it_was_split_off_is_among_what_that_was_granted() {
    let (store, disk) = gap_store();
    let (west, _) = opened_gap(&store, 0, 1);
    assert_eq!(split(&west, 1, &[WEST_TOO, WEST], 4), parted(3));
    let (part, _) = opened_gap(&store, 3, 4);
    save(&part, WEST, &built(WEST, &[GLASS]));
    part.flush();
    assert!(exists(disk.as_ref(), "/world/regions/3.region"));

    assert_eq!(
        merge(&west, 3, 4, 2),
        StoreReply::Absorbed {
            absorbed: RegionId(3),
            chunks: vec![WEST, WEST_TOO]
        }
    );
    assert!(part.is_lost());
    assert_eq!(load(&west, WEST), built(WEST, &[GLASS]));
    west.flush();
    // A region that a split made has no entity ids, and its region file goes with it.
    assert!(!exists(disk.as_ref(), "/world/regions/3.region"));
    assert!(!exists(disk.as_ref(), "/world/regions/3.state"));
    let absorbed = |store: &Store, epoch: u64, case: &str| {
        let list = store.regions().unwrap();
        assert_eq!(list.absorbed, [(RegionId(3), RegionId(0))], "{case}");
        assert_eq!(list.regions.len(), 3, "{case}");
        assert_eq!(list.regions[0].pinned, [west_area()], "{case}");
        let refused = store.open_region(hello_of(&gap(), 3, epoch));
        assert!(
            matches!(
                refused,
                Err(StoreError::Absorbed {
                    region: RegionId(3),
                    into: RegionId(0)
                })
            ),
            "{case}: {:?}",
            refused.err()
        );
        let (_, restored) = opened_gap(store, 0, epoch);
        assert_eq!(state_of(&restored), Some((2, whole("merged", 2))), "{case}");
        assert_eq!(restored.held, [(WEST, 2), (WEST_TOO, 2)], "{case}");
        assert_eq!(restored.pinned, [west_area()], "{case}");
    };
    after_every_crash(&disk, &gap(), "absorbed", |store, case| {
        absorbed(store, 5, case)
    });
    starts_again_alike(&disk, &gap(), "absorbed");
    drop(west);
    absorbed(&store, 5, "in the store that merged");
}

/// Scenario 21: ids are not used again. After a merge and a restart of the store the
/// next split gets a higher id than any before, whether the log still has the records
/// of the split and the merge or the table file has taken them over.
#[test]
fn the_id_of_an_absorbed_region_is_not_given_to_the_next_part() {
    for checkpointed in [false, true] {
        let (store, disk) = gap_store();
        let (west, _) = opened_gap(&store, 0, 1);
        assert_eq!(split(&west, 1, &[WEST], 1), parted(3));
        let (_part, _) = opened_gap(&store, 3, 1);
        assert_eq!(
            merge(&west, 3, 1, 2),
            StoreReply::Absorbed {
                absorbed: RegionId(3),
                chunks: vec![WEST]
            }
        );
        if checkpointed {
            checkpoint(&west, 3, "after");
            west.flush();
            store.regions().unwrap();
            let table = table_file(disk.as_ref());
            assert_eq!((table.next_region, table.absorbed), (4, vec![(3, 0)]));
            let made = |record: &LogRecord| {
                matches!(record, LogRecord::Split { .. } | LogRecord::Absorbed { .. })
            };
            assert!(!records(disk.as_ref()).iter().any(made));
        }
        west.flush();
        after_every_crash(&disk, &gap(), "merged", |store, case| {
            let (west, restored) = opened_gap(store, 0, 2);
            let tick = restored.tick() + 1;
            assert_eq!(split(&west, tick, &[WEST_TOO], 1), parted(4), "{case}");
            assert_eq!(split(&west, tick + 1, &[WEST], 1), parted(5), "{case}");
        });
        // The store that ran counts on as well.
        let tick = if checkpointed { 4 } else { 3 };
        assert_eq!(split(&west, tick, &[WEST_TOO], 1), parted(4));
    }
}

/// Scenario 21 for a pinned region that is absorbed: its id is not the next part's
/// either, after a restart of the store.
#[test]
fn the_id_of_an_absorbed_pinned_region_is_not_given_to_the_next_part() {
    let (store, disk) = gap_store();
    let (west, _) = opened_gap(&store, 0, 1);
    let (_east, _) = opened_gap(&store, 1, 1);
    assert_eq!(
        merge(&west, 1, 1, 1),
        StoreReply::Absorbed {
            absorbed: RegionId(1),
            chunks: vec![]
        }
    );
    west.flush();
    after_every_crash(&disk, &gap(), "merged", |store, case| {
        let (west, _) = opened_gap(store, 0, 2);
        assert_eq!(split(&west, 2, &[EAST], 1), parted(3), "{case}");
        let living: Vec<u32> = store
            .regions()
            .unwrap()
            .regions
            .iter()
            .map(|info| info.region.0)
            .collect();
        assert_eq!(living, [0, 2, 3], "{case}");
    });
}

/// Section 3.7, step 5, and 4.2, step 4: the region file of a split's part cannot be
/// written, in each of the ways writing it can fail. The split is answered all the
/// same, and a hello with a lower epoch than the split named is refused: in the store
/// that split, while the disk fails and when it does not any more, by a store that
/// starts on what a crash leaves before the file is there, and after the table file
/// has taken the place of the split's record.
#[test]
fn the_region_file_of_a_part_that_could_not_be_written_is_written_later() {
    for (op, ending) in [
        (Op::Write, "regions/3.region.tmp"),
        (Op::Sync, "regions/3.region.tmp"),
        (Op::Rename, "regions/3.region"),
        (Op::SyncDirectory, "/regions"),
    ] {
        let case = format!("{op:?} of {ending}");
        let (store, disk) = picky();
        let (west, _) = opened_gap(&store, 0, 1);
        west.flush();
        let lower_is_refused = |store: &Store, case: &str| {
            let refused = store.open_region(hello_of(&gap(), 3, 5));
            assert!(
                matches!(
                    refused,
                    Err(StoreError::EpochRefused {
                        region: RegionId(3),
                        offered: 5,
                        seen: 6
                    })
                ),
                "{case}: {:?}",
                refused.err()
            );
        };

        disk.fail(op, ending);
        assert_eq!(split(&west, 1, &[WEST], 6), parted(3), "{case}");
        assert!(disk.failures() > 0, "{case}: the file is written otherwise");
        lower_is_refused(&store, &case);
        // The store dies before the file is there: the next start writes it.
        after_every_crash(&disk.disk, &gap(), &case, |store, case| {
            lower_is_refused(store, case);
            let (_, restored) = opened_gap(store, 3, 6);
            assert_eq!(state_of(&restored), Some((1, whole("part", 1))), "{case}");
            assert_eq!(restored.held, [(WEST, 1)], "{case}");
            assert_eq!(restored.entity_ids, NO_ENTITY_IDS, "{case}");
        });
        starts_again_alike(&disk.disk, &gap(), &case);

        // The disk works again, and the store goes on: both regions make a checkpoint,
        // after which the table file has the part and no segment has the split.
        disk.mend();
        lower_is_refused(&store, &case);
        let (part, restored) = opened_gap(&store, 3, 6);
        assert_eq!(restored.entity_ids, NO_ENTITY_IDS, "{case}");
        for (handle, name) in [(&part, "part"), (&west, "west")] {
            log(handle, 2, &[]);
            checkpoint(handle, 2, name);
            handle.flush();
        }
        store.regions().unwrap();
        let table = table_file(&disk.disk);
        assert_eq!(table.next_region, 4, "{case}");
        let ids: Vec<u32> = table.regions.iter().map(|region| region.id).collect();
        assert_eq!(ids, [0, 1, 2, 3], "{case}");
        let splits = |record: &LogRecord| matches!(record, LogRecord::Split { .. });
        assert!(!records(&disk.disk).iter().any(splits), "{case}");
        lower_is_refused(&store, &case);

        // The file is durably there, with the epoch and without entity ids.
        let left = disk.disk.crashed(Survival::Nothing);
        let bytes = left.read(Path::new("/world/regions/3.region")).unwrap();
        let file = RegionFile::decode(&bytes.expect("the part has a region file")).unwrap();
        assert_eq!((file.epoch, file.entity_ids), (6, NO_ENTITY_IDS), "{case}");
        after_every_crash(&disk.disk, &gap(), &case, |store, case| {
            lower_is_refused(store, case);
            let (_, restored) = opened_gap(store, 3, 6);
            assert_eq!(state_of(&restored), Some((2, whole("part", 2))), "{case}");
            assert_eq!(restored.held, [(WEST, 1)], "{case}");
            assert_eq!(restored.entity_ids, NO_ENTITY_IDS, "{case}");
        });
        starts_again_alike(&disk.disk, &gap(), &case);
    }
}

/// A world of the division with a gap in which a region was absorbed, a part was split
/// off, and all of them have commits that are in no saved chunk. Returns the disk, and
/// what the western and the eastern region were restored with when first opened.
fn merged_and_split() -> (Arc<MemoryDisk>, [Restored; 2]) {
    let (store, disk) = gap_store();
    let (west, west_was) = opened_gap(&store, 0, 3);
    let (east, east_was) = opened_gap(&store, 1, 5);
    let (home, _) = opened_gap(&store, 2, 1);
    assert_eq!(claim(&east, &[FREE_TOO]), (vec![FREE_TOO], vec![]));
    log(&east, 1, &[change(EAST, GLASS)]);
    save(&east, EAST, &built(EAST, &[GLASS]));
    checkpoint(&east, 1, "east");
    east.flush();

    assert_eq!(claim(&west, &[FREE]), (vec![FREE], vec![]));
    assert_eq!(split(&west, 1, &[WEST, FREE], 2), parted(3));
    let (part, _) = opened_gap(&store, 3, 2);
    assert_eq!(
        merge(&west, 1, 5, 2),
        StoreReply::Absorbed {
            absorbed: RegionId(1),
            chunks: vec![FREE_TOO]
        }
    );
    log(
        &west,
        3,
        &[
            // Its area, the area that came with the merge, and what the merge granted.
            change(WEST_TOO, GLASS),
            change(EAST, OTHER),
            change(FREE_TOO, STONE),
            // The part's, and nobody's: these are in no chunk afterwards.
            change(WEST, STONE),
            change(FREE_THREE, STONE),
        ],
    );
    log(
        &part,
        2,
        &[
            change(WEST, GLASS),
            change(FREE, OTHER),
            // The old region's.
            change(WEST_TOO, STONE),
        ],
    );
    log(
        &home,
        1,
        &[change(ORIGIN, THIRD), change(FREE_THREE, GLASS)],
    );
    committed(&west, 3);
    committed(&part, 2);
    committed(&home, 1);
    let list = store.regions().unwrap();
    assert_eq!(list.absorbed, [(RegionId(1), RegionId(0))]);
    assert_eq!(list.regions.len(), 3);
    (disk, [west_was, east_was])
}

/// Looks at a world of [`merged_and_split`] that a store was started on with the two
/// stripes of [`division`]: it has the stripes and nothing else, their states are
/// dropped, their entity ids are as before, and the chunks have what the old regions
/// committed to chunks they held, and nothing else. Returns the western region's
/// handle.
fn made_over(store: &Store, was: &[Restored; 2], case: &str) -> StoreHandle {
    let stripes = division();
    let list = store.regions().unwrap();
    assert_eq!(list.home, RegionId(1), "{case}");
    assert_eq!(list.absorbed, [], "{case}");
    let found: Vec<_> = list
        .regions
        .iter()
        .map(|info| (info.region.0, info.pinned.clone(), info.bounds))
        .collect();
    let expected = [
        (0, vec![stripes.pinned[0]], None),
        (1, vec![stripes.pinned[1]], None),
    ];
    assert_eq!(found, expected, "{case}");
    for gone in [2, 3] {
        let refused = store.open_region(hello_of(&stripes, gone, 100));
        assert!(
            matches!(refused, Err(StoreError::UnknownRegion { region }) if region.0 == gone),
            "{case}: {:?}",
            refused.err()
        );
    }
    let (west, restored) = opened(store, &stripes, 0, 100);
    assert_eq!((restored.state, restored.deltas.len()), (None, 0), "{case}");
    assert_eq!(restored.held, [], "{case}");
    assert_eq!(restored.entity_ids, was[0].entity_ids, "{case}");
    let (east, restored) = opened(store, &stripes, 1, 100);
    assert_eq!((restored.state, restored.deltas.len()), (None, 0), "{case}");
    assert_eq!(restored.held, [], "{case}");
    assert_eq!(restored.entity_ids, was[1].entity_ids, "{case}");

    assert_eq!(load(&west, WEST), built(WEST, &[GLASS]), "{case}");
    assert_eq!(load(&west, WEST_TOO), built(WEST_TOO, &[GLASS]), "{case}");
    assert_eq!(load(&east, EAST), built(EAST, &[GLASS, OTHER]), "{case}");
    assert_eq!(load(&east, FREE_TOO), built(FREE_TOO, &[STONE]), "{case}");
    assert_eq!(load(&east, FREE), built(FREE, &[OTHER]), "{case}");
    assert_eq!(load(&east, ORIGIN), built(ORIGIN, &[THIRD]), "{case}");
    assert_eq!(load(&east, FREE_THREE), built(FREE_THREE, &[]), "{case}");
    west
}

/// A changed division after a merge and a split: the world is made over, the region
/// that was absorbed and the part are dropped with the rest, and the ids that were
/// used are not used again but for the stripes: the next part gets the id the old
/// table would have given, also after the world is made over a second time.
#[test]
fn a_world_with_an_absorbed_region_and_a_part_is_made_over_for_another_division() {
    let (disk, was) = merged_and_split();
    for survival in SURVIVALS {
        let case = format!("{survival:?}");
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &division()).unwrap_or_else(|error| panic!("{case}: {error}"));
        let west = made_over(&store, &was, &case);
        assert_eq!(split(&west, 1, &[WEST], 1), parted(4), "{case}");
        west.flush();
        drop((west, store));

        // Started again with the same division, the world is as it was left.
        let again = Arc::new(left.crashed(Survival::Nothing));
        let store = store_on(&again, &division()).unwrap();
        let list = store.regions().unwrap();
        let living: Vec<u32> = list.regions.iter().map(|info| info.region.0).collect();
        assert_eq!(living, [0, 1, 4], "{case}");
        let (part, restored) = opened(&store, &division(), 4, 1);
        assert_eq!(restored.held, [(WEST, 1)], "{case}");
        assert_eq!(load(&part, WEST), built(WEST, &[GLASS]), "{case}");
        drop((part, store));

        // Started with the division it had at first, it is made over once more: the
        // home region is made after the pinned ones again, and holds the home chunk.
        let back = Arc::new(again.crashed(Survival::Nothing));
        let store = store_on(&back, &gap()).unwrap();
        let list = store.regions().unwrap();
        let living: Vec<u32> = list.regions.iter().map(|info| info.region.0).collect();
        assert_eq!((list.home, living), (RegionId(2), vec![0, 1, 2]), "{case}");
        assert_eq!(list.absorbed, [], "{case}");
        for gone in [3, 4] {
            let refused = store.open_region(hello_of(&gap(), gone, 100));
            assert!(
                matches!(refused, Err(StoreError::UnknownRegion { .. })),
                "{case}: {:?}",
                refused.err()
            );
        }
        assert_eq!(held(&store, 2, 100), [(ORIGIN, 0)], "{case}");
        let (west, restored) = opened_gap(&store, 0, 200);
        assert_eq!((restored.state, restored.held), (None, vec![]), "{case}");
        assert_eq!(load(&west, WEST), built(WEST, &[GLASS]), "{case}");
        assert_eq!(split(&west, 1, &[WEST], 1), parted(5), "{case}");
    }
}

/// The same world made over by a store that is killed at every change and sync of its
/// start: whatever a crash leaves of that, the next start makes the world over, to the
/// same result.
#[test]
fn a_world_with_an_absorbed_region_and_a_part_is_made_over_at_every_kill_point() {
    let (disk, was) = merged_and_split();
    let changes = {
        let probe = Arc::new(disk.crashed(Survival::Everything));
        drop(store_on(&probe, &division()).unwrap());
        probe.operations()
    };
    assert!(changes > 3, "{changes}");
    for n in 1..=changes {
        let stopped = Arc::new(
            disk.crashed(Survival::Everything)
                .with(crate::disk::Fault::Stop(n)),
        );
        drop(store_on(&stopped, &division()));
        for survival in SURVIVALS {
            let case = format!("stopped at {n}, {survival:?}");
            let left = Arc::new(stopped.crashed(survival));
            let store =
                store_on(&left, &division()).unwrap_or_else(|error| panic!("{case}: {error}"));
            let west = made_over(&store, &was, &case);
            assert_eq!(split(&west, 1, &[WEST], 1), parted(4), "{case}");
        }
    }
}

/// Open question 8 of the record: a stripe whose id was a part's before the division
/// changed takes over the part's highest epoch, and is issued entity ids, which the
/// part had none of, when it is next opened.
#[test]
fn a_stripe_whose_id_was_a_parts_keeps_the_epoch_and_is_issued_entity_ids() {
    let (disk, was) = merged_and_split();
    let four = Division::stripes(ORIGIN, &Layout::new(vec![0, 16, 32]).unwrap());
    let left = Arc::new(disk.crashed(Survival::Everything));
    let store = store_on(&left, &four).unwrap();
    let list = store.regions().unwrap();
    let living: Vec<u32> = list.regions.iter().map(|info| info.region.0).collect();
    assert_eq!((list.home, living), (RegionId(1), vec![0, 1, 2, 3]));
    assert_eq!(list.absorbed, []);

    let refused = store.open_region(hello_of(&four, 3, 1));
    assert!(
        matches!(refused, Err(StoreError::EpochRefused { seen: 2, .. })),
        "{:?}",
        refused.err()
    );
    let (_fourth, restored) = opened(&store, &four, 3, 2);
    assert_eq!((restored.state, restored.held), (None, vec![]));
    assert_eq!(restored.pinned, [four.pinned[3]]);
    let ids = restored.entity_ids;
    assert!(ids.first.0 < ids.end.0, "{ids:?}");
    let mut issued = vec![ids];
    for region in [0, 1, 2] {
        let (_, restored) = opened(&store, &four, region, 100);
        assert!(!issued.contains(&restored.entity_ids), "region {region}");
        issued.push(restored.entity_ids);
    }
    assert_eq!(issued[1], was[0].entity_ids);
    assert_eq!(issued[2], was[1].entity_ids);
}

/// Another home chunk makes the world over as other areas do: the region that was
/// absorbed is a region again, pinned to its area, the part is dropped, the home region
/// holds the new home chunk and not the old one, and the next part gets the id the old
/// table would have given.
#[test]
fn a_world_with_an_absorbed_region_and_a_part_is_made_over_for_another_home_chunk() {
    let (disk, was) = merged_and_split();
    let moved = Division {
        home: FREE_THREE,
        ..gap()
    };
    for survival in SURVIVALS {
        let case = format!("{survival:?}");
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &moved).unwrap_or_else(|error| panic!("{case}: {error}"));
        let list = store.regions().unwrap();
        assert_eq!(
            (list.home, &list.absorbed),
            (RegionId(2), &vec![]),
            "{case}"
        );
        let found: Vec<_> = list
            .regions
            .iter()
            .map(|info| (info.region.0, info.pinned.clone(), info.bounds))
            .collect();
        let home_chunk = ChunkBox {
            min: FREE_THREE,
            max: FREE_THREE,
        };
        let expected = [
            (0, vec![west_area()], None),
            (1, vec![east_area()], None),
            (2, vec![], Some(home_chunk)),
        ];
        assert_eq!(found, expected, "{case}");
        let refused = store.open_region(hello_of(&moved, 3, 100));
        assert!(
            matches!(refused, Err(StoreError::UnknownRegion { .. })),
            "{case}: {:?}",
            refused.err()
        );

        let (west, restored) = opened(&store, &moved, 0, 100);
        assert_eq!((restored.state, restored.held), (None, vec![]), "{case}");
        assert_eq!(restored.entity_ids, was[0].entity_ids, "{case}");
        let (east, restored) = opened(&store, &moved, 1, 100);
        assert_eq!((restored.state, restored.held), (None, vec![]), "{case}");
        assert_eq!(restored.pinned, [east_area()], "{case}");
        assert_eq!(restored.entity_ids, was[1].entity_ids, "{case}");
        let (home, restored) = opened(&store, &moved, 2, 100);
        assert_eq!(restored.state, None, "{case}");
        assert_eq!(restored.held, [(FREE_THREE, 0)], "{case}");

        assert_eq!(load(&west, WEST), built(WEST, &[GLASS]), "{case}");
        assert_eq!(load(&west, WEST_TOO), built(WEST_TOO, &[GLASS]), "{case}");
        assert_eq!(load(&east, EAST), built(EAST, &[GLASS, OTHER]), "{case}");
        assert_eq!(load(&home, FREE_THREE), built(FREE_THREE, &[]), "{case}");
        // The old home chunk and what the regions were granted are nobody's, and have
        // what was committed to them.
        assert_eq!(holder(&west, 0, ORIGIN), None, "{case}");
        let free = vec![ORIGIN, FREE, FREE_TOO];
        assert_eq!(claim(&home, &free), (free.clone(), vec![]), "{case}");
        assert_eq!(load(&home, ORIGIN), built(ORIGIN, &[THIRD]), "{case}");
        assert_eq!(load(&home, FREE), built(FREE, &[OTHER]), "{case}");
        assert_eq!(load(&home, FREE_TOO), built(FREE_TOO, &[STONE]), "{case}");
        assert_eq!(split(&west, 1, &[WEST], 1), parted(4), "{case}");
    }
}

// More of the order of things, beyond the list of section 9.

/// Sections 3.3 and 3.8: what comes back from the thread for chunks counts only for
/// the session that opened the region last. An owner's return is under way when the
/// next owner opens the region, changes the chunk, saves it and returns it in turn,
/// which is the first return of its session as the other was of the old one. The old
/// return's message frees nothing; the chunk is free when the new one is through.
#[test]
fn a_return_of_a_replaced_owner_frees_nothing_when_its_message_comes() {
    let (store, gate, disk) = gated_gap();
    let (old, _) = opened_gap(&store, 0, 1);
    let (other, _) = opened_gap(&store, 1, 1);
    assert_eq!(claim(&old, &[FREE]), (vec![FREE], vec![]));
    old.flush();

    gate.hold_at(Stop::BeforeSync);
    give_back(&old, &[FREE]);
    gate.wait_until_there();
    // Nothing is to be replayed, so the hello is answered while the thread is held.
    let (new, restored) = opened_while_held(&store, 0, 2);
    assert_eq!(restored.held, [(FREE, 0)]);
    assert!(old.is_lost());
    log(&new, 1, &[change(FREE, GLASS)]);
    committed(&new, 1);
    save(&new, FREE, &built(FREE, &[GLASS]));
    give_back(&new, &[FREE]);
    new.request(StoreRequest::Flush);

    for (stop, point) in [
        (Stop::AfterSync, "before the old owner's message"),
        (Stop::BeforeSave(FREE), "the old owner's return through"),
        (Stop::AfterSync, "before the new owner's message"),
    ] {
        gate.on_to(stop);
        store.regions().unwrap();
        assert_eq!(holder(&other, 1, FREE), Some(RegionId(0)), "{point}");
        the_chunk_is_kept_with(&disk, FREE, &[GLASS], point);
    }
    gate.let_go();
    assert_eq!(reply(&new), StoreReply::Flushed);
    assert_eq!(holder(&other, 1, FREE), None);
    the_chunk_is_free_with(&disk, FREE, &[GLASS], "the new owner's return through");
}

/// Section 3.8: what an owner asked for before the hello of the next is done first,
/// and is the next owner's; what it asks afterwards is not done and not answered, be
/// it a claim, a return, a merge or a split.
#[test]
fn a_replaced_owner_has_what_it_claimed_before_the_hello_and_nothing_it_asks_after_it() {
    let (store, disk) = switched_for(&gap());
    let (old, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 1);
    old.flush();
    east.flush();

    // The hello arrives while the log is being synced, between two claims.
    disk.holding_syncs.store(true, Ordering::SeqCst);
    log(&old, 1, &[]);
    disk.held.wait();
    old.request(StoreRequest::Claim { chunks: vec![FREE] });
    let waiting = open_later(&store, hello_of(&gap(), 0, 2));
    old.request(StoreRequest::Claim {
        chunks: vec![FREE_TOO],
    });
    disk.held.wait();
    let (opened, restored) = waiting.recv().unwrap().unwrap();
    let new = StoreHandle::local(opened, store.messages.clone());
    assert_eq!(
        (ticks(&restored), restored.held),
        (vec![1], vec![(FREE, 1)])
    );

    let before = store.regions().unwrap();
    give_back(&old, &[FREE]);
    old.request(split_of(2, &[WEST], 1));
    old.request(StoreRequest::AbsorbCommit {
        absorbed: RegionId(1),
        absorbed_epoch: 1,
        tick: 2,
        state: whole("merged", 2),
    });
    old.request(StoreRequest::Flush);
    let claimed = StoreReply::Claimed {
        granted: vec![FREE],
        foreign: vec![],
    };
    assert_eq!(
        answers_until_lost(&old),
        [StoreReply::Committed { tick: 1 }, claimed]
    );
    new.flush();
    assert_eq!(store.regions().unwrap(), before);
    assert!(!east.is_lost() && !new.is_lost());
    assert_eq!(holder(&east, 1, FREE), Some(RegionId(0)));
    assert_eq!(holder(&east, 1, FREE_TOO), None);
    after_every_crash(&disk.disk, &gap(), "replaced", |store, case| {
        let found = world(store, &gap(), 9, case);
        assert_eq!(found.list.regions.len(), 3, "{case}");
        assert_eq!(found.list.absorbed, [], "{case}");
        assert_eq!(found.restored[0].1.held, [(FREE, 1)], "{case}");
        assert_eq!(found.restored[0].1.state, None, "{case}");
    });
}

/// Section 4.1, step 2: what a group that failed did to the table is undone in reverse
/// order. A return goes through and another region is granted the chunk in the same
/// group, whose sync fails: the grant is taken back and the return given back, so that
/// the chunk is its first region's, from the tick it had, whatever a crash keeps.
#[test]
fn a_return_and_a_grant_of_its_chunk_in_a_group_that_failed_are_both_undone() {
    let disk = Arc::new(Switched::default());
    let (store, gate) = gated(&disk, &gap());
    let (first, _) = opened_gap(&store, 0, 1);
    let (second, _) = opened_gap(&store, 1, 1);
    log(&first, 1, &[]);
    log(&first, 2, &[]);
    assert_eq!(claim(&first, &[FREE]), (vec![FREE], vec![]));
    save(&first, FREE, &built(FREE, &[GLASS]));
    first.flush();
    second.flush();

    // The return is with the thread for chunks, and behind it a save that says when
    // its message has been sent.
    gate.hold_at(Stop::BeforeSync);
    give_back(&first, &[FREE]);
    save(&first, WEST, &built(WEST, &[]));
    gate.wait_until_there();
    // The commit thread is held in the sync of a commit. The return's message arrives
    // meanwhile, and behind it the other region's claim: they are one group.
    disk.holding_syncs.store(true, Ordering::SeqCst);
    log(&second, 1, &[]);
    disk.held.wait();
    gate.on_to(Stop::BeforeSave(WEST));
    second.request(StoreRequest::Claim { chunks: vec![FREE] });
    disk.failing_syncs.store(true, Ordering::SeqCst);
    disk.held.wait();
    assert_eq!(
        answers_until_lost(&second),
        [StoreReply::Committed { tick: 1 }]
    );
    gate.let_go();
    assert_eq!(answers_until_lost(&first), []);
    disk.failing_syncs.store(false, Ordering::SeqCst);

    let (first, restored) = opened_gap(&store, 0, 2);
    assert_eq!(restored.held, [(FREE, 2)]);
    assert_eq!(load(&first, FREE), built(FREE, &[GLASS]));
    let (second, restored) = opened_gap(&store, 1, 2);
    assert_eq!(restored.held, []);
    assert_eq!(claim(&second, &[FREE]), (vec![], vec![(FREE, RegionId(0))]));
    after_every_crash(&disk.disk, &gap(), "undone", |store, case| {
        assert_eq!(held(store, 0, 3), [(FREE, 2)], "{case}");
        assert_eq!(held(store, 1, 3), [], "{case}");
    });
    starts_again_alike(&disk.disk, &gap(), "undone");
}

/// Section 3.3, step 2: a return whose saves cannot be made durable loses the handle,
/// and frees nothing.
#[test]
fn a_return_whose_saves_cannot_be_made_durable_loses_the_handle_and_frees_nothing() {
    let (store, disk) = picky();
    let (first, _) = opened_gap(&store, 0, 1);
    let (second, _) = opened_gap(&store, 1, 1);
    assert_eq!(claim(&first, &[FREE]), (vec![FREE], vec![]));
    save(&first, FREE, &built(FREE, &[GLASS]));
    // The directory the chunk's manifest is in cannot be synced.
    disk.fail(Op::SyncDirectory, "manifests/overworld/0.0");
    give_back(&first, &[FREE]);
    first.request(StoreRequest::Flush);
    assert_eq!(answers_until_lost(&first), []);
    assert!(disk.failures() > 0);
    assert!(!second.is_lost());
    assert_eq!(claim(&second, &[FREE]), (vec![], vec![(FREE, RegionId(0))]));
    disk.mend();

    let (first, restored) = opened_gap(&store, 0, 2);
    assert_eq!(restored.held, [(FREE, 0)]);
    after_every_crash(&disk.disk, &gap(), "not returned", |store, case| {
        assert_eq!(held(store, 0, 3), [(FREE, 0)], "{case}");
        assert_eq!(held(store, 1, 3), [], "{case}");
    });
    // Saved and returned again with the disk in order, it is free and as it was saved.
    save(&first, FREE, &built(FREE, &[GLASS]));
    give_back(&first, &[FREE]);
    first.flush();
    assert_eq!(claim(&second, &[FREE]), (vec![FREE], vec![]));
    assert_eq!(load(&second, FREE), built(FREE, &[GLASS]));
}

/// Sections 2, 3.6 and 5: regions absorbed one after the other are listed oldest
/// first, each with the region it went into, which may have been absorbed since; a
/// region that was not pinned becomes so by absorbing one that was; and the home
/// region stays the home region, which nobody absorbs.
#[test]
fn regions_absorbed_one_after_the_other_end_up_with_the_last_survivor() {
    let (store, disk) = gap_store();
    let (west, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 1);
    let (home, _) = opened_gap(&store, 2, 1);
    assert_eq!(split(&west, 1, &[WEST], 1), parted(3));
    let (part, _) = opened_gap(&store, 3, 1);
    assert_eq!(claim(&part, &[FREE]), (vec![FREE], vec![]));
    log(&part, 2, &[change(WEST, GLASS)]);
    save(&part, WEST, &built(WEST, &[GLASS]));
    checkpoint(&part, 2, "part");
    part.flush();
    assert_eq!(
        merge(&west, 3, 1, 2),
        StoreReply::Absorbed {
            absorbed: RegionId(3),
            chunks: vec![WEST, FREE]
        }
    );
    // The home region, which is pinned to nothing, absorbs the western one.
    assert_eq!(
        merge(&home, 0, 1, 4),
        StoreReply::Absorbed {
            absorbed: RegionId(0),
            chunks: vec![WEST, FREE]
        }
    );
    assert!(west.is_lost() && part.is_lost());
    assert_eq!(merge(&east, 2, 1, 1), declined(Decline::Home));
    assert_eq!(split(&home, 5, &[ORIGIN], 1), declined(Decline::Home));
    home.flush();

    let check = |store: &Store, epoch: u64, case: &str| {
        let list = store.regions().unwrap();
        let living: Vec<u32> = list.regions.iter().map(|info| info.region.0).collect();
        assert_eq!((list.home, living), (RegionId(2), vec![1, 2]), "{case}");
        assert_eq!(
            list.absorbed,
            [(RegionId(3), RegionId(0)), (RegionId(0), RegionId(2))],
            "{case}"
        );
        let survivor = &list.regions[1];
        assert_eq!(survivor.pinned, [west_area()], "{case}");
        let bounds = ChunkBox {
            min: ChunkPos::new(-3, 0),
            max: ChunkPos::new(5, 5),
        };
        assert_eq!(survivor.bounds, Some(bounds), "{case}");
        for (gone, into) in [(3, 0), (0, 2)] {
            let refused = store.open_region(hello_of(&gap(), gone, epoch));
            assert!(
                matches!(
                    refused,
                    Err(StoreError::Absorbed { region, into: named })
                        if region.0 == gone && named.0 == into
                ),
                "{case}: {:?}",
                refused.err()
            );
        }
        let (home, restored) = opened_gap(store, 2, epoch);
        assert_eq!(state_of(&restored), Some((4, whole("merged", 4))), "{case}");
        assert_eq!(restored.held, [(WEST, 4), (ORIGIN, 0), (FREE, 4)], "{case}");
        assert_eq!(restored.pinned, [west_area()], "{case}");
        assert_eq!(load(&home, WEST), built(WEST, &[GLASS]), "{case}");
        assert_eq!(load(&home, WEST_TOO), built(WEST_TOO, &[]), "{case}");
        assert_eq!(load(&home, FREE), built(FREE, &[]), "{case}");
        assert_eq!(load(&home, ORIGIN), built(ORIGIN, &[]), "{case}");
        assert_eq!(holder(&home, 2, EAST), Some(RegionId(1)), "{case}");
    };
    after_every_crash(&disk, &gap(), "absorbed in turn", |store, case| {
        check(store, 2, case)
    });
    starts_again_alike(&disk, &gap(), "absorbed in turn");
    drop(home);
    check(&store, 2, "in the store that merged");
}

/// Section 2, entity ids: no block is issued twice. A region that is opened for the
/// first time after another was absorbed, whose region file stays, gets a block of its
/// own, in the store that merged and in one that starts on its world.
#[test]
fn the_entity_ids_of_an_absorbed_region_are_not_issued_again() {
    let (store, disk) = gap_store();
    let (west, west_was) = opened_gap(&store, 0, 1);
    let (_east, east_was) = opened_gap(&store, 1, 1);
    assert_ne!(west_was.entity_ids, east_was.entity_ids);
    assert_eq!(
        merge(&west, 1, 1, 1),
        StoreReply::Absorbed {
            absorbed: RegionId(1),
            chunks: vec![]
        }
    );
    west.flush();
    let check = |store: &Store, case: &str| {
        let (_, home) = opened_gap(store, 2, 1);
        let ids = home.entity_ids;
        assert!(ids.first.0 < ids.end.0, "{case}: {ids:?}");
        assert!(
            ids != west_was.entity_ids && ids != east_was.entity_ids,
            "{case}"
        );
        let (_, west) = opened_gap(store, 0, 2);
        assert_eq!(west.entity_ids, west_was.entity_ids, "{case}");
    };
    after_every_crash(&disk, &gap(), "merged", check);
    check(&store, "in the store that merged");
}

/// Section 1.1, sizes: a claim and a return of more chunks than a record may name take
/// several records, of at most 65 536 chunks each, and all of the chunks are granted
/// and returned.
#[test]
fn a_claim_and_a_return_of_very_many_chunks_take_several_records() {
    let (store, disk) = gap_store();
    let (west, _) = opened_gap(&store, 0, 1);
    let many: Vec<ChunkPos> = (0..70_000)
        .map(|index| ChunkPos::new(1 + index % 14, 10 + index / 14))
        .collect();
    let (granted, foreign) = claim(&west, &many);
    assert!(granted == many && foreign.is_empty());
    store.regions().unwrap();
    let sizes = |disk: &MemoryDisk, returned: bool| -> Vec<usize> {
        records(disk)
            .iter()
            .filter_map(|record| match record {
                LogRecord::Granted { chunks, .. } if !returned => Some(chunks.len()),
                LogRecord::Returned { chunks, .. } if returned => Some(chunks.len()),
                _ => None,
            })
            .collect()
    };
    let granted = sizes(&disk, false);
    assert!(granted.len() > 1 && granted.iter().all(|size| *size <= 65_536));
    assert_eq!(granted.iter().sum::<usize>(), many.len(), "{granted:?}");
    let left = Arc::new(disk.crashed(Survival::Nothing));
    assert_eq!(
        held(&store_on(&left, &gap()).unwrap(), 0, 2).len(),
        many.len()
    );

    give_back(&west, &many);
    west.flush();
    let returned = sizes(&disk, true);
    assert!(returned.len() > 1 && returned.iter().all(|size| *size <= 65_536));
    assert_eq!(returned.iter().sum::<usize>(), many.len(), "{returned:?}");
    after_every_crash(&disk, &gap(), "all returned", |store, case| {
        assert_eq!(held(store, 0, 2), [], "{case}");
    });
}

/// Section 5: the list is made when the group is ended, so that it has nothing that
/// is not durable. A claim asked for just before the list is in it, and in what a
/// crash leaves at that moment.
#[test]
fn the_list_has_nothing_that_a_crash_would_take_back() {
    let (store, disk) = gap_store();
    let (west, _) = opened_gap(&store, 0, 1);
    let mut max = FREE;
    for chunk in [FREE, FREE_TOO, FREE_THREE] {
        west.request(StoreRequest::Claim {
            chunks: vec![chunk],
        });
        let list = store.regions().unwrap();
        max = ChunkPos::new(max.x.max(chunk.x), 5);
        let min = ChunkPos::new(5, chunk.z.min(5));
        assert_eq!(list.regions[0].bounds, Some(ChunkBox { min, max }));
        let left = Arc::new(disk.crashed(Survival::Nothing));
        let again = store_on(&left, &gap()).unwrap();
        assert_eq!(again.regions().unwrap(), list);
    }
}

/// Section 4.2, step 2: a record of the log that does not fit the table ends the start
/// with `StoreError::Table`: a `Granted` of a chunk that is granted already, a region
/// that is not there, a part's chunk that the region does not hold.
#[test]
fn a_record_of_the_log_that_does_not_fit_the_table_starts_no_store() {
    let (store, disk) = gap_store();
    let (west, _) = opened_gap(&store, 0, 1);
    assert_eq!(claim(&west, &[FREE]), (vec![FREE], vec![]));
    west.flush();
    store.regions().unwrap();
    drop(west);
    let part = |part, chunks: &[ChunkPos]| LogRecord::Split {
        region: 0,
        epoch: 1,
        tick: 1,
        state: Vec::new(),
        part,
        part_epoch: 1,
        chunks: chunks.to_vec(),
        part_state: Vec::new(),
    };
    let misfits = [
        (
            "granted to another region already",
            LogRecord::Granted {
                region: 1,
                tick: 0,
                chunks: vec![FREE],
            },
        ),
        (
            "granted to this region already",
            LogRecord::Granted {
                region: 0,
                tick: 3,
                chunks: vec![FREE_TOO, FREE],
            },
        ),
        (
            "granted to a region that is not there",
            LogRecord::Granted {
                region: 9,
                tick: 0,
                chunks: vec![FREE_TOO],
            },
        ),
        (
            "a part with a chunk of another region's area",
            part(3, &[WEST, EAST]),
        ),
        ("a part with a chunk that is nobody's", part(3, &[FREE_TOO])),
        (
            "absorbing a region that is not there",
            LogRecord::Absorbed {
                region: 0,
                epoch: 1,
                absorbed: 9,
                tick: 1,
                state: Vec::new(),
            },
        ),
    ];
    for (case, record) in misfits {
        let left = Arc::new(disk.crashed(Survival::Everything));
        let next = segments(left.as_ref()).last().unwrap() + 1;
        put(&left, &segment_path(next), &record.encode());
        let started = store_on(&left, &gap());
        assert!(
            matches!(started, Err(StoreError::Table(_))),
            "{case}: {:?}",
            started.err().map(|error| error.to_string())
        );
    }
    // A record that does fit, written the same way, is taken.
    let left = Arc::new(disk.crashed(Survival::Everything));
    let next = segments(left.as_ref()).last().unwrap() + 1;
    put(&left, &segment_path(next), &part(3, &[WEST, FREE]).encode());
    let store = store_on(&left, &gap()).unwrap();
    assert_eq!(held(&store, 3, 1), [(WEST, 1), (FREE, 1)]);
    assert_eq!(held(&store, 0, 2), []);
}

/// Sections 3.6 and 3.7, step 1: the group is ended before a merge or a split is
/// looked at, so what either region claimed just before is granted, answered and part
/// of it.
#[test]
fn what_was_claimed_just_before_a_merge_or_a_split_is_part_of_it() {
    let (store, disk) = switched_for(&gap());
    let (west, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 1);
    let (home, _) = opened_gap(&store, 2, 1);
    for handle in [&west, &east, &home] {
        handle.flush();
    }

    // Both claims and the merge arrive while the log is being synced for a commit of
    // the home region.
    disk.holding_syncs.store(true, Ordering::SeqCst);
    log(&home, 1, &[]);
    disk.held.wait();
    east.request(StoreRequest::Claim { chunks: vec![FREE] });
    west.request(StoreRequest::Claim {
        chunks: vec![FREE_TOO],
    });
    west.request(StoreRequest::AbsorbCommit {
        absorbed: RegionId(1),
        absorbed_epoch: 1,
        tick: 1,
        state: whole("merged", 1),
    });
    disk.held.wait();
    let claimed = |chunk| StoreReply::Claimed {
        granted: vec![chunk],
        foreign: vec![],
    };
    assert_eq!(any_reply(&west), claimed(FREE_TOO));
    assert_eq!(
        any_reply(&west),
        StoreReply::Absorbed {
            absorbed: RegionId(1),
            chunks: vec![FREE]
        }
    );
    assert_eq!(answers_until_lost(&east), [claimed(FREE)]);

    // A claim and, right behind it, a split of what it asks for.
    west.request(StoreRequest::Claim {
        chunks: vec![FREE_THREE],
    });
    west.request(split_of(2, &[FREE_THREE, FREE, FREE_TOO], 3));
    assert_eq!(any_reply(&west), claimed(FREE_THREE));
    assert_eq!(any_reply(&west), parted(3));
    after_every_crash(&disk.disk, &gap(), "merged and split", |store, case| {
        let list = store.regions().unwrap();
        assert_eq!(list.absorbed, [(RegionId(1), RegionId(0))], "{case}");
        assert_eq!(list.regions.len(), 3, "{case}");
        assert_eq!(
            held(store, 3, 3),
            [(FREE, 2), (FREE_TOO, 2), (FREE_THREE, 2)],
            "{case}"
        );
        assert_eq!(held(store, 0, 60), [], "{case}");
    });
    starts_again_alike(&disk.disk, &gap(), "merged and split");
}

/// Section 3.4 for the two regions of a split: what each commits afterwards goes into
/// the chunks it holds when it is opened again, and not into the other's.
#[test]
fn after_a_split_each_regions_commits_go_into_its_own_chunks_and_no_others() {
    let (store, disk) = gap_store();
    let (west, _) = opened_gap(&store, 0, 1);
    assert_eq!(claim(&west, &[FREE]), (vec![FREE], vec![]));
    assert_eq!(split(&west, 1, &[WEST, FREE], 2), parted(3));
    let (part, _) = opened_gap(&store, 3, 2);
    log(
        &part,
        2,
        &[
            change(WEST, GLASS),
            change(FREE, GLASS),
            change(WEST_TOO, STONE),
        ],
    );
    log(
        &west,
        2,
        &[
            change(WEST_TOO, GLASS),
            change(WEST, STONE),
            change(FREE, STONE),
        ],
    );
    committed(&part, 2);
    committed(&west, 2);
    drop((west, part));

    let check = |store: &Store, case: &str| {
        let (part, restored) = opened_gap(store, 3, 5);
        assert_eq!(state_of(&restored), Some((1, whole("part", 1))), "{case}");
        assert_eq!(ticks(&restored), [2], "{case}");
        part.flush();
        let (west, restored) = opened_gap(store, 0, 5);
        assert_eq!(state_of(&restored), Some((1, whole("rest", 1))), "{case}");
        assert_eq!(ticks(&restored), [2], "{case}");
        assert_eq!(load(&west, WEST_TOO), built(WEST_TOO, &[GLASS]), "{case}");
        assert_eq!(load(&part, WEST), built(WEST, &[GLASS]), "{case}");
        assert_eq!(load(&part, FREE), built(FREE, &[GLASS]), "{case}");
    };
    after_every_crash(&disk, &gap(), "committed on", check);
    check(&store, "in the store that split");
}

/// Section 3.4: a hello that the commit thread answers by itself overtakes nothing.
/// What the owner before had the thread for chunks do, and that thread has not got to
/// yet, is done before anything the new owner asks.
#[test]
fn what_the_owner_before_had_saved_is_done_before_what_the_next_owner_asks() {
    let (store, gate, _disk) = gated_gap();
    let (old, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 1);
    gate.hold_at(Stop::BeforeSave(BUSY_EAST));
    save(&east, BUSY_EAST, &built(BUSY_EAST, &[]));
    gate.wait_until_there();
    save(&old, WEST, &built(WEST, &[GLASS]));

    let (new, _) = opened_while_held(&store, 0, 2);
    assert!(old.is_lost());
    new.request(StoreRequest::Load { position: WEST });
    save(&new, WEST, &built(WEST, &[GLASS, OTHER]));
    new.request(StoreRequest::Load { position: WEST });
    gate.let_go();
    let loaded = |marks: &[Mark]| StoreReply::Loaded {
        position: WEST,
        chunk: built(WEST, marks),
    };
    assert_eq!(reply(&new), loaded(&[GLASS]));
    assert_eq!(reply(&new), loaded(&[GLASS, OTHER]));
}

/// Section 4.2, step 2: "the next segment's number is at least `from`, also when no
/// segment is left". After checkpoints of every region the table file stands for the
/// whole log; what a store that starts then writes for the table is read by the next.
#[test]
fn a_store_that_starts_on_a_log_the_table_file_stands_for_writes_no_segment_before_it() {
    let (store, disk) = gap_store();
    let [west, east, home] = before_the_last_checkpoint(&store);
    save(&home, ORIGIN, &built(ORIGIN, &[GLASS]));
    checkpoint(&home, 1, "home");
    home.flush();
    store.regions().unwrap();
    let from = table_file(disk.as_ref()).from;
    assert!(from > 1, "{from}");
    drop((west, east, home, store));

    for survival in SURVIVALS {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &gap()).unwrap();
        let (west, _) = opened_gap(&store, 0, 2);
        assert_eq!(claim(&west, &[FREE_THREE]), (vec![FREE_THREE], vec![]));
        let granted = LogRecord::Granted {
            region: 0,
            tick: 1,
            chunks: vec![FREE_THREE],
        };
        let written: Vec<u64> = segments(left.as_ref())
            .into_iter()
            .filter(|number| {
                let bytes = left.read(&segment_path(*number)).unwrap().unwrap();
                read_log(&bytes).unwrap().0.contains(&granted)
            })
            .collect();
        assert!(
            written.len() == 1 && written[0] >= from,
            "{survival:?}: in {written:?}, from {from}"
        );
        drop((west, store));
        let again = Arc::new(left.crashed(Survival::Nothing));
        let store = store_on(&again, &gap()).unwrap();
        assert_eq!(
            held(&store, 0, 3),
            [(FREE, 0), (FREE_THREE, 1)],
            "{survival:?}"
        );
    }
}

/// Section 3.6, step 4, and ADR-0010 as this record changes it: the store keeps the
/// latest 4096 absorbed regions. A hello for one of them is refused with the region it
/// went into; one for a region it has forgotten is refused as for a region that never
/// was. The ids go on all the same.
#[test]
fn the_store_forgets_all_but_the_latest_4096_absorbed_regions() {
    let (store, disk) = gap_store();
    let (west, _) = opened_gap(&store, 0, 1);
    let rounds = 4097;
    for round in 0..rounds {
        let part = 3 + round;
        let tick = u64::from(round) * 2 + 1;
        assert_eq!(split(&west, tick, &[WEST], 1), parted(part));
        let (_part, _) = opened_gap(&store, part, 1);
        assert_eq!(
            merge(&west, part, 1, tick + 1),
            StoreReply::Absorbed {
                absorbed: RegionId(part),
                chunks: vec![WEST]
            }
        );
    }
    west.flush();
    let check = |store: &Store, case: &str| {
        let list = store.regions().unwrap();
        assert_eq!(list.absorbed.len(), 4096, "{case}");
        assert_eq!(list.absorbed[0], (RegionId(4), RegionId(0)), "{case}");
        let last = (RegionId(3 + 4096), RegionId(0));
        assert_eq!(list.absorbed[4095], last, "{case}");
        assert_eq!(list.regions.len(), 3, "{case}");
        let forgotten = store.open_region(hello_of(&gap(), 3, 9));
        assert!(
            matches!(
                forgotten,
                Err(StoreError::UnknownRegion {
                    region: RegionId(3)
                })
            ),
            "{case}: {:?}",
            forgotten.err()
        );
        let kept = store.open_region(hello_of(&gap(), 4, 9));
        assert!(
            matches!(
                kept,
                Err(StoreError::Absorbed {
                    region: RegionId(4),
                    into: RegionId(0)
                })
            ),
            "{case}: {:?}",
            kept.err()
        );
    };
    check(&store, "in the store that merged");
    after_every_crash(&disk, &gap(), "merged very often", |store, case| {
        check(store, case);
        let (west, restored) = opened_gap(store, 0, 2);
        assert_eq!(restored.held, [(WEST, u64::from(rounds) * 2)], "{case}");
        let tick = restored.tick() + 1;
        assert_eq!(split(&west, tick, &[WEST], 1), parted(3 + rounds), "{case}");
    });
}

/// Section 4.1: a return that waits for its region's group, which fails, is not made:
/// the chunk is the region's when it is opened again, and is free once it is returned
/// by the new owner.
#[test]
fn a_return_that_waits_for_a_group_that_fails_is_not_made() {
    let (store, disk) = switched_for(&gap());
    let (west, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 1);
    assert_eq!(claim(&west, &[FREE]), (vec![FREE], vec![]));
    west.flush();
    east.flush();

    disk.failing_syncs.store(true, Ordering::SeqCst);
    log(&west, 1, &[]);
    give_back(&west, &[FREE]);
    west.request(StoreRequest::Flush);
    assert_eq!(answers_until_lost(&west), []);
    assert_eq!(answers_until_lost(&east), []);
    disk.failing_syncs.store(false, Ordering::SeqCst);

    let (east, _) = opened_gap(&store, 1, 2);
    assert_eq!(claim(&east, &[FREE]), (vec![], vec![(FREE, RegionId(0))]));
    let (west, restored) = opened_gap(&store, 0, 2);
    assert_eq!((ticks(&restored), restored.held), (vec![], vec![(FREE, 0)]));
    after_every_crash(&disk.disk, &gap(), "not returned", |store, case| {
        assert_eq!(held(store, 0, 3), [(FREE, 0)], "{case}");
        assert_eq!(held(store, 1, 3), [], "{case}");
    });
    give_back(&west, &[FREE]);
    west.flush();
    assert_eq!(claim(&east, &[FREE]), (vec![FREE], vec![]));
}

/// Sections 4.1, step 3, and 3.3: what the thread for chunks sends later for a session
/// that a failed group has lost is dropped. A return is between its sync and its
/// message when another region's commit cannot be synced; the region is opened again,
/// changes the chunk, saves it and returns it, which is the first return of the new
/// session as the other was of the old one. The old message frees nothing.
#[test]
fn a_return_under_way_when_a_group_fails_frees_nothing_when_its_message_comes() {
    let disk = Arc::new(Switched::default());
    let (store, gate) = gated(&disk, &gap());
    let (old, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 1);
    assert_eq!(claim(&old, &[FREE]), (vec![FREE], vec![]));
    old.flush();
    east.flush();

    gate.hold_at(Stop::AfterSync);
    give_back(&old, &[FREE]);
    gate.wait_until_there();
    disk.failing_syncs.store(true, Ordering::SeqCst);
    log(&east, 1, &[]);
    assert_eq!(answers_until_lost(&east), []);
    disk.failing_syncs.store(false, Ordering::SeqCst);

    // Nobody is welcomed before every owner is lost.
    let (new, restored) = opened_while_held(&store, 0, 2);
    assert!(old.is_lost());
    assert_eq!(restored.held, [(FREE, 0)]);
    let (other, _) = opened_while_held(&store, 1, 2);
    log(&new, 1, &[change(FREE, GLASS)]);
    committed(&new, 1);
    save(&new, FREE, &built(FREE, &[GLASS]));
    give_back(&new, &[FREE]);
    new.request(StoreRequest::Flush);

    for (stop, point) in [
        (Stop::BeforeSave(FREE), "the lost session's return through"),
        (Stop::AfterSync, "before the new session's message"),
    ] {
        gate.on_to(stop);
        store.regions().unwrap();
        assert_eq!(holder(&other, 1, FREE), Some(RegionId(0)), "{point}");
        the_chunk_is_kept_with(&disk.disk, FREE, &[GLASS], point);
    }
    gate.let_go();
    assert_eq!(reply(&new), StoreReply::Flushed);
    assert_eq!(holder(&other, 1, FREE), None);
    the_chunk_is_free_with(
        &disk.disk,
        FREE,
        &[GLASS],
        "the new session's return through",
    );
}

/// Section 3.5: the table file is written while a return is under way. It has the
/// grant, and the `Returned` that follows is in a segment the next start reads.
#[test]
fn a_return_under_way_when_the_table_file_is_written_is_read_from_the_log_behind_it() {
    let (store, gate, disk) = gated_gap();
    let (west, _) = opened_gap(&store, 0, 1);
    assert_eq!(claim(&west, &[FREE]), (vec![FREE], vec![]));
    log(&west, 1, &[]);
    west.flush();

    // The thread for chunks is let go through the checkpoint and held at the return.
    gate.hold_at(Stop::AfterSync);
    checkpoint(&west, 1, "west");
    give_back(&west, &[FREE]);
    west.request(StoreRequest::Flush);
    gate.wait_until_there();
    gate.on_to(Stop::BeforeSync);
    store.regions().unwrap();
    let table = table_file(disk.as_ref());
    assert!(table.from > 1, "{table:?}");
    assert_eq!(table.regions[0].grants, [(FREE, 0)]);
    after_every_crash(&disk, &gap(), "the table file written", |store, case| {
        let (_, restored) = opened_gap(store, 0, 2);
        assert_eq!(state_of(&restored), Some((1, whole("west", 1))), "{case}");
        assert_eq!(restored.held, [(FREE, 0)], "{case}");
    });

    gate.let_go();
    assert_eq!(reply(&west), StoreReply::Flushed);
    let returned = LogRecord::Returned {
        region: 0,
        chunks: vec![FREE],
    };
    assert!(records(disk.as_ref()).contains(&returned));
    let written = segments(disk.as_ref());
    assert!(
        written.iter().all(|number| *number >= table.from),
        "{written:?}"
    );
    after_every_crash(&disk, &gap(), "returned", |store, case| {
        assert_eq!(held(store, 0, 2), [], "{case}");
        let (east, _) = opened_gap(store, 1, 2);
        assert_eq!(claim(&east, &[FREE]), (vec![FREE], vec![]), "{case}");
    });
    starts_again_alike(&disk, &gap(), "returned");
}

/// Section 3.5: a grant that is written in the group that puts a state file in place
/// and writes the table file is in the file or in a segment from its `from` on, not in
/// both and not in neither: the next start grants the chunk once.
#[test]
fn a_grant_in_the_group_that_writes_the_table_file_is_granted_once_at_the_next_start() {
    let disk = Arc::new(Switched::default());
    let (store, gate) = gated(&disk, &gap());
    let (west, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 1);
    assert_eq!(claim(&west, &[FREE]), (vec![FREE], vec![]));
    log(&west, 1, &[]);
    west.flush();
    east.flush();

    // The checkpoint is with the thread for chunks, and behind it a save that says
    // when its message has been sent.
    gate.hold_at(Stop::BeforeSync);
    checkpoint(&west, 1, "west");
    save(&west, WEST, &built(WEST, &[]));
    gate.wait_until_there();
    // The commit thread is held in the sync of a claim. The checkpoint's message
    // arrives meanwhile, and behind it another claim: they are one group.
    disk.holding_syncs.store(true, Ordering::SeqCst);
    east.request(StoreRequest::Claim {
        chunks: vec![FREE_THREE],
    });
    disk.held.wait();
    gate.on_to(Stop::BeforeSave(WEST));
    east.request(StoreRequest::Claim {
        chunks: vec![FREE_TOO],
    });
    disk.held.wait();
    let claimed = |chunk| StoreReply::Claimed {
        granted: vec![chunk],
        foreign: vec![],
    };
    assert_eq!(any_reply(&east), claimed(FREE_THREE));
    assert_eq!(any_reply(&east), claimed(FREE_TOO));
    gate.let_go();
    west.flush();
    store.regions().unwrap();
    let table = table_file(&disk.disk);
    assert!(table.from > 1, "{table:?}");

    after_every_crash(&disk.disk, &gap(), "claimed", |store, case| {
        let (_, restored) = opened_gap(store, 0, 2);
        assert_eq!(state_of(&restored), Some((1, whole("west", 1))), "{case}");
        assert_eq!(restored.held, [(FREE, 0)], "{case}");
        assert_eq!(
            held(store, 1, 2),
            [(FREE_TOO, 0), (FREE_THREE, 0)],
            "{case}"
        );
    });
    starts_again_alike(&disk.disk, &gap(), "claimed");
}

/// Section 4.3: "a record counts if it is whole. A kill during a write leaves a record
/// cut off, which does not count." A merge or a split whose record a kill cut off,
/// made by hand, has not happened, and does not stand in the way of the next one; one
/// whose record is whole has happened, although nobody was ever answered.
#[test]
fn a_merge_or_a_split_whose_record_was_cut_off_has_not_happened() {
    let (store, disk) = gap_store();
    let (west, _) = opened_gap(&store, 0, 1);
    let (east, _) = opened_gap(&store, 1, 1);
    assert_eq!(claim(&east, &[FREE]), (vec![FREE], vec![]));
    west.flush();
    east.flush();
    store.regions().unwrap();
    drop((west, east, store));

    let merge_record = LogRecord::Absorbed {
        region: 0,
        epoch: 1,
        absorbed: 1,
        tick: 1,
        state: whole("merged", 1),
    };
    let split_record = LogRecord::Split {
        region: 0,
        epoch: 1,
        tick: 1,
        state: whole("rest", 1),
        part: 3,
        part_epoch: 4,
        chunks: vec![WEST],
        part_state: whole("part", 1),
    };
    // What a store that died while it wrote `bytes` behind its log left.
    let killed = |bytes: &[u8]| {
        let left = Arc::new(disk.crashed(Survival::Everything));
        let path = segment_path(*segments(left.as_ref()).last().unwrap());
        left.append(&path, bytes).unwrap();
        left.sync(&path).unwrap();
        left
    };
    for (name, record) in [("merge", &merge_record), ("split", &split_record)] {
        let bytes = record.encode();
        for cut in [1, bytes.len() / 2, bytes.len() - 1] {
            let case = format!("a {name} cut off at {cut}");
            let left = killed(&bytes[..cut]);
            let store = store_on(&left, &gap()).unwrap_or_else(|error| panic!("{case}: {error}"));
            let list = store.regions().unwrap();
            assert_eq!((list.regions.len(), list.absorbed.len()), (3, 0), "{case}");
            assert!(!merged_as_a_whole(&store, 1, &case), "{case}");

            // The regions go on, and merge and split in earnest.
            let (west, _) = opened_gap(&store, 0, 50);
            let (_east, restored) = opened_gap(&store, 1, 50);
            assert_eq!(restored.held, [(FREE, 0)], "{case}");
            let absorbed = StoreReply::Absorbed {
                absorbed: RegionId(1),
                chunks: vec![FREE],
            };
            assert_eq!(merge(&west, 1, 50, 1), absorbed, "{case}");
            assert_eq!(split(&west, 2, &[FREE], 1), parted(3), "{case}");
            west.flush();
            after_every_crash(&left, &gap(), &case, |store, case| {
                let list = store.regions().unwrap();
                assert_eq!(list.absorbed, [(RegionId(1), RegionId(0))], "{case}");
                assert_eq!(held(store, 3, 60), [(FREE, 2)], "{case}");
                let (_, rest) = opened_gap(store, 0, 60);
                assert_eq!(state_of(&rest), Some((2, whole("rest", 2))), "{case}");
            });
        }
    }

    // Whole, each has happened.
    let left = killed(&merge_record.encode());
    let store = store_on(&left, &gap()).unwrap();
    assert!(merged_as_a_whole(&store, 1, "a whole merge"));
    assert_eq!(held(&store, 0, 60), [(FREE, 1)]);
    let left = killed(&split_record.encode());
    let store = store_on(&left, &gap()).unwrap();
    assert_eq!(store.regions().unwrap().regions.len(), 4);
    let refused = store.open_region(hello_of(&gap(), 3, 3));
    assert!(
        matches!(refused, Err(StoreError::EpochRefused { seen: 4, .. })),
        "{:?}",
        refused.err()
    );
    let (_, part) = opened_gap(&store, 3, 4);
    assert_eq!(state_of(&part), Some((1, whole("part", 1))));
    assert_eq!(part.held, [(WEST, 1)]);
    assert_eq!(part.entity_ids, NO_ENTITY_IDS);
    starts_again_alike(&left, &gap(), "a whole split");
}
