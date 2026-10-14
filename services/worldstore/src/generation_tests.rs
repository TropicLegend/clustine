//! Tests of generation off the thread for chunks, written from ADR-0021 (sections 6
//! and 7, and section 9, item 7) by someone who did not write the store's side of it:
//! the new text of ADR-0017's test T10, items 1 to 1j and 3, and S1 to S8.
//!
//! The generator here is the test's own. Its `request` says when it is entered for a
//! chunk and answers a chunk that is held only when the test lets it go, so that a
//! chunk can be "being generated" while the test looks at what the thread for chunks
//! does meanwhile. It remembers nothing between requests, and the test holds the only
//! handle to it besides the store's.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::mpsc::TryRecvError;
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use clustine_data::blocks;
use clustine_rpc::RegionList;
use clustine_world::generation::{Done, Failed, Wanted};
use clustine_world::{Chunk, ChunkPos};

use super::*;
use crate::disk::{Fault, MemoryDisk, Survival};
use crate::regions::{checkpoint, claim, gap, give_back, whole};
use crate::tests::{committed, generator, hello, log, open};

// The world of these tests is `gap()`: region 0 pinned west of x = 0, region 1 pinned
// from x = 16 on, and the home region 2 with the home chunk at the origin; the chunks
// in between are nobody's until a region claims them.

const WEST: u32 = 0;
const EAST: u32 = 1;
const HOME: u32 = 2;

const ROOT: &str = "/world";
const ORIGIN: ChunkPos = ChunkPos::new(0, 0);

/// Chunks of the eastern region, never stored unless a test stores them.
const A: ChunkPos = ChunkPos::new(20, 0);
const B: ChunkPos = ChunkPos::new(21, 0);
const C: ChunkPos = ChunkPos::new(22, 0);

/// Chunks nobody holds until they are claimed.
const FREE: ChunkPos = ChunkPos::new(5, 0);
const S: ChunkPos = ChunkPos::new(3, 0);
const D: ChunkPos = ChunkPos::new(4, 0);
const E: ChunkPos = ChunkPos::new(6, 0);

/// A chunk of the western region.
const R: ChunkPos = ChunkPos::new(-5, 0);

/// The version of the test's generator, but where S8 has two.
const VERSION: u16 = 1;

/// How long a test waits for something that is to happen before it says that it did
/// not. Nothing here waits for time to pass: this only bounds a wait for a state.
const PATIENCE: Duration = Duration::from_secs(60);

// The generator's settings: one identity for both versions of S8.
type Settings = clustine_world::generation::GeneratorSettings;

fn settings(version: u16) -> Settings {
    Settings {
        identity: IDENTITY.to_owned(),
        version,
    }
}

const IDENTITY: &str = "a generator that a test holds";

/// What the test's generator makes of `position`: the flat world with a block of its
/// own for the position and one for the version, so that chunks of other positions or
/// versions are told apart, marked as made by `version`.
fn made(position: ChunkPos, version: u16, with_failure: bool) -> Chunk {
    static FLAT: OnceLock<Chunk> = OnceLock::new();
    let mut chunk = FLAT.get_or_init(|| generator().generate(ORIGIN)).clone();
    let (x, z) = (position.x.rem_euclid(16), position.z.rem_euclid(16));
    chunk.set(x as usize, 70, z as usize, blocks::GLASS);
    chunk.set(0, 60 + i32::from(version), 0, blocks::STONE);
    // Said last: a change takes it back.
    chunk.mark_generated(version, with_failure);
    chunk
}

/// The chunk with a change in it, which is no longer as generated.
fn changed(chunk: &Chunk) -> Chunk {
    let mut chunk = chunk.clone();
    chunk.set(1, 80, 1, blocks::STONE);
    chunk
}

/// What the test knows of its generator and how it steers it.
struct Control {
    version: u16,
    toys: Mutex<Toys>,
    changed: Condvar,
}

#[derive(Default)]
struct Toys {
    /// Positions whose requests are answered only when the test lets them go.
    held: BTreeSet<ChunkPos>,
    /// Positions the generator fails for.
    failing: BTreeSet<ChunkPos>,
    /// Positions whose chunks come out marked "with a failure".
    marked: BTreeSet<ChunkPos>,
    /// The requests that are held, in the order they came.
    waiting: Vec<Waiting>,
    numbered: u64,
    /// The position of every request, in the order they came.
    entered: Vec<ChunkPos>,
    /// The position of every request that was dropped before it was answered.
    dropped: Vec<ChunkPos>,
    /// Whether the store has dropped the generator.
    gone: bool,
}

struct Waiting {
    number: u64,
    position: ChunkPos,
    done: Done,
}

impl Control {
    fn toys(&self) -> MutexGuard<'_, Toys> {
        self.toys.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn hold(&self, positions: &[ChunkPos]) {
        self.toys().held.extend(positions);
    }

    fn fail(&self, position: ChunkPos) {
        self.toys().failing.insert(position);
    }

    fn mark(&self, position: ChunkPos) {
        self.toys().marked.insert(position);
    }

    fn answer(&self, toys: &Toys, position: ChunkPos) -> Result<Chunk, Failed> {
        if toys.failing.contains(&position) {
            return Err(Failed);
        }
        Ok(made(
            position,
            self.version,
            toys.marked.contains(&position),
        ))
    }

    /// Answers every request for `position` that is held, and every later one at once.
    fn let_go(&self, position: ChunkPos) {
        let answers: Vec<_> = {
            let mut toys = self.toys();
            toys.held.remove(&position);
            let (going, staying) = std::mem::take(&mut toys.waiting)
                .into_iter()
                .partition(|waiting: &Waiting| waiting.position == position);
            toys.waiting = staying;
            going
                .into_iter()
                .map(|waiting| (waiting.done, self.answer(&toys, position)))
                .collect()
        };
        // Outside the lock: whoever is called may ask the generator again.
        for (done, answer) in answers {
            done(answer);
        }
    }

    fn forget(&self, number: u64) {
        let mut toys = self.toys();
        // A request that was answered is not under way any more: dropping it after
        // that says nothing.
        if let Some(at) = toys.waiting.iter().position(|w| w.number == number) {
            let waiting = toys.waiting.remove(at);
            toys.dropped.push(waiting.position);
            self.changed.notify_all();
        }
    }

    fn entered(&self, position: ChunkPos) -> usize {
        let toys = self.toys();
        toys.entered.iter().filter(|p| **p == position).count()
    }

    fn dropped(&self, position: ChunkPos) -> usize {
        let toys = self.toys();
        toys.dropped.iter().filter(|p| **p == position).count()
    }

    fn wait_until(&self, what: &str, until: impl Fn(&Toys) -> bool) {
        let deadline = Instant::now() + PATIENCE;
        let mut toys = self.toys();
        while !until(&toys) {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "the generator waited in vain for {what}");
            toys = self
                .changed
                .wait_timeout(toys, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn wait_entered(&self, position: ChunkPos, times: usize) {
        self.wait_until(&format!("{times} requests for {position:?}"), |toys| {
            toys.entered.iter().filter(|p| **p == position).count() >= times
        });
    }

    fn wait_dropped(&self, position: ChunkPos, times: usize) {
        self.wait_until(&format!("{times} drops of {position:?}"), |toys| {
            toys.dropped.iter().filter(|p| **p == position).count() >= times
        });
    }

    /// Waits until the store has dropped its generator, which the thread for chunks
    /// does last (ADR-0021, section 7, on T10's item 3): both of its threads have
    /// ended then.
    fn wait_gone(&self) {
        self.wait_until("the store to drop it", |toys| toys.gone);
    }
}

/// The store's generator. The test keeps the [`Control`], never this.
struct Toy(Arc<Control>);

impl ChunkGenerator for Toy {
    fn generate(&self, position: ChunkPos) -> Chunk {
        let answer = self.0.answer(&self.0.toys(), position);
        answer.unwrap_or_else(|Failed| panic!("the test's generator fails for {position:?}"))
    }

    fn settings(&self) -> Settings {
        settings(self.0.version)
    }

    fn request(&self, position: ChunkPos, done: Done) -> Wanted {
        let mut toys = self.0.toys();
        toys.entered.push(position);
        self.0.changed.notify_all();
        if !toys.held.contains(&position) {
            let answer = self.0.answer(&toys, position);
            drop(toys);
            done(answer);
            return Wanted::nothing();
        }
        toys.numbered += 1;
        let number = toys.numbered;
        toys.waiting.push(Waiting {
            number,
            position,
            done,
        });
        let control = Arc::clone(&self.0);
        Wanted::new(move || control.forget(number))
    }
}

impl Drop for Toy {
    fn drop(&mut self) {
        self.0.toys().gone = true;
        self.0.changed.notify_all();
    }
}

fn toy_of(version: u16) -> (Arc<Toy>, Arc<Control>) {
    let control = Arc::new(Control {
        version,
        toys: Mutex::default(),
        changed: Condvar::new(),
    });
    (Arc::new(Toy(Arc::clone(&control))), control)
}

/// A store of the world `gap()` on `disk`, with its chunks in files there.
fn store_on(disk: &Arc<MemoryDisk>, generator: Arc<Toy>) -> Store {
    let root = Path::new(ROOT);
    let chunks = FileChunks::new(disk.clone(), root);
    start(disk.clone(), root, Box::new(chunks), generator, &gap()).unwrap()
}

/// A store on a fresh disk with a generator of [`VERSION`].
fn fresh() -> (Store, Arc<Control>, Arc<MemoryDisk>) {
    let disk = Arc::new(MemoryDisk::default());
    let (toy, control) = toy_of(VERSION);
    (store_on(&disk, toy), control, disk)
}

// Answers.

/// The next answer to `handle` but that a commit is done.
fn answer(handle: &StoreHandle) -> StoreReply {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match handle.replies.recv_timeout(left) {
            Ok(StoreReply::Committed { .. }) => {}
            Ok(reply) => return reply,
            Err(error) => panic!("the store did not answer: {error}"),
        }
    }
}

/// Asserts that `handle` has no answer queued but that commits are done.
fn quiet(handle: &StoreHandle) {
    loop {
        match handle.replies.try_recv() {
            Ok(StoreReply::Committed { .. }) => {}
            Ok(reply) => panic!("an answer that should not be there yet: {reply:?}"),
            Err(_) => return,
        }
    }
}

fn loaded(position: ChunkPos, chunk: Chunk) -> StoreReply {
    StoreReply::Loaded { position, chunk }
}

fn generating(position: ChunkPos) -> StoreReply {
    StoreReply::Generating { position }
}

/// Asks for a flush through `handle` and waits for its answer. Used as a barrier: a
/// job given to the thread for chunks after another job is done after it if both can
/// be done (ADR-0021, section 7, rule 1), and the answers of both go out in that order.
/// So once the flush of another region is answered, whatever the earlier job could
/// have answered is in its handle's queue.
fn flushed(handle: &StoreHandle) {
    handle.request(StoreRequest::Flush);
    assert_eq!(answer(handle), StoreReply::Flushed);
}

fn load(handle: &StoreHandle, position: ChunkPos) {
    handle.request(StoreRequest::Load { position });
}

fn save(handle: &StoreHandle, position: ChunkPos, chunk: &Chunk) {
    handle.request(StoreRequest::Save {
        position,
        tick: 5,
        chunk: chunk.clone(),
    });
}

/// Loads a chunk that is not stored and that the generator answers at once, and
/// returns what the handle was answered with.
fn generated(handle: &StoreHandle, position: ChunkPos) -> Chunk {
    load(handle, position);
    assert_eq!(answer(handle), generating(position));
    match answer(handle) {
        StoreReply::Loaded {
            position: at,
            chunk,
        } if at == position => chunk,
        other => panic!("expected the chunk at {position:?}, got {other:?}"),
    }
}

// What a crash would leave.

/// The chunk stored at `position` in what a crash with `survival` leaves of `disk`.
fn stored_with(disk: &MemoryDisk, survival: Survival, position: ChunkPos) -> Option<Chunk> {
    let left = Arc::new(disk.crashed(survival));
    FileChunks::new(left, Path::new(ROOT))
        .load(position)
        .unwrap()
}

/// The chunk at `position` that is durable on `disk`.
fn stored(disk: &MemoryDisk, position: ChunkPos) -> Option<Chunk> {
    stored_with(disk, Survival::Nothing, position)
}

/// Whether anything was written for the chunk at `position`, durable or not.
fn written(disk: &MemoryDisk, position: ChunkPos) -> bool {
    stored_with(disk, Survival::Everything, position).is_some()
}

/// How many chunk files a crash with `survival` leaves, temporary ones not counted.
fn chunk_files(disk: &MemoryDisk, survival: Survival) -> usize {
    let chunks = Path::new(ROOT).join("chunks");
    disk.crashed(survival)
        .paths()
        .iter()
        .filter(|path| path.starts_with(&chunks))
        .filter(|path| path.extension().is_some_and(|ext| ext == "chunk"))
        .count()
}

/// What a store started on what a crash leaves of `disk` now restores `region` with.
fn restored_after_crash(disk: &MemoryDisk, region: u32) -> Restored {
    let left = Arc::new(disk.crashed(Survival::Nothing));
    let (toy, _) = toy_of(VERSION);
    let store = store_on(&left, toy);
    store.open_region(hello(region, 100)).unwrap().1
}

fn ticks(restored: &Restored) -> Vec<u64> {
    restored.deltas.iter().map(|delta| delta.tick).collect()
}

fn state_tick(restored: &Restored) -> Option<u64> {
    restored.state.as_ref().map(|state| state.tick)
}

fn checkpoint_of(handle: &StoreHandle, tick: u64) {
    handle.request(StoreRequest::Checkpoint {
        tick,
        state: whole("state", tick),
    });
}

/// The block of a chunk that the commits of these tests change, in world coordinates.
fn block_in(position: ChunkPos) -> (i32, i32, i32) {
    (position.x * 16 + 3, 100, position.z * 16 + 4)
}

/// Whether the region of `region` in `list` was granted `position`, as far as the list
/// can say: its box has it.
fn boxed(list: &RegionList, region: u32, position: ChunkPos) -> bool {
    let info = list
        .regions
        .iter()
        .find(|info| info.region == RegionId(region))
        .expect("the region is listed");
    info.bounds.is_some_and(|bounds| {
        (bounds.min.x..=bounds.max.x).contains(&position.x)
            && (bounds.min.z..=bounds.max.z).contains(&position.z)
    })
}

// T10, item 1, as ADR-0021 has it.

/// T10, item 1: the barrier waits for the thread for chunks and for the generator. A
/// checkpoint behind a load that is being generated is not done, and what a crash
/// leaves then has the commits and not the chunk; once the chunk is let go, the
/// barrier is answered and what a crash leaves has the state and the chunk.
#[test]
fn the_barrier_waits_for_a_chunk_being_generated_and_for_the_checkpoint_behind_it() {
    let (store, control, disk) = fresh();
    control.hold(&[A]);
    let witness = open(&store, hello(HOME, 1));
    let x = open(&store, hello(EAST, 1));
    for tick in 1..=3 {
        log(&x, tick, &[]);
    }
    committed(&x, 3);
    // Without waiting for any answer, and dropped as a runner that is abandoned is.
    load(&x, A);
    x.request(StoreRequest::Checkpoint {
        tick: 3,
        state: whole("state", 3),
    });
    drop(x);
    let barrier = store.barrier();
    store.regions().unwrap();
    // Given after the barrier and done, so the barrier would have passed before it.
    flushed(&witness);
    control.wait_entered(A, 1);
    assert!(
        matches!(barrier.try_recv(), Err(TryRecvError::Empty)),
        "the barrier passed a chunk being generated"
    );
    let restored = restored_after_crash(&disk, EAST);
    assert_eq!(
        (state_tick(&restored), ticks(&restored)),
        (None, vec![1, 2, 3])
    );
    assert_eq!(stored(&disk, A), None);

    control.let_go(A);
    Store::rested(&barrier).unwrap();
    let restored = restored_after_crash(&disk, EAST);
    assert_eq!((state_tick(&restored), ticks(&restored)), (Some(3), vec![]));
    let chunk = stored(&disk, A).expect("the chunk handed out is durable");
    assert_eq!(chunk, made(A, VERSION, false));
    assert_eq!(chunk.generated(), VERSION);
}

/// T10, item 1a: loads of other chunks do not wait, whether they are generated or
/// stored.
#[test]
fn loads_of_other_chunks_do_not_wait_for_a_chunk_being_generated() {
    let (store, control, _disk) = fresh();
    let x = open(&store, hello(EAST, 1));
    control.hold(&[A, B]);
    load(&x, A);
    load(&x, B);
    assert_eq!(answer(&x), generating(A));
    assert_eq!(answer(&x), generating(B));
    control.wait_entered(B, 1);
    control.let_go(B);
    assert_eq!(answer(&x), loaded(B, made(B, VERSION, false)));
    control.let_go(A);
    assert_eq!(answer(&x), loaded(A, made(A, VERSION, false)));

    // A stored chunk is answered while another is held.
    let kept = changed(&made(C, VERSION, false));
    save(&x, C, &kept);
    flushed(&x);
    let held = ChunkPos::new(23, 0);
    control.hold(&[held]);
    load(&x, held);
    load(&x, C);
    assert_eq!(answer(&x), generating(held));
    assert_eq!(answer(&x), loaded(C, kept));
    control.let_go(held);
    assert_eq!(answer(&x), loaded(held, made(held, VERSION, false)));
}

/// T10, item 1b, first half, and S2's "unless a chunk is stored by then": two loads
/// of one chunk are answered in order with equal chunks, and one chunk is stored.
#[test]
fn two_loads_of_a_chunk_being_generated_are_answered_in_order_with_equal_chunks() {
    let (store, control, disk) = fresh();
    let x = open(&store, hello(EAST, 1));
    control.hold(&[A]);
    load(&x, A);
    load(&x, A);
    assert_eq!(answer(&x), generating(A));
    control.wait_entered(A, 1);
    control.let_go(A);
    let mut chunks = Vec::new();
    while chunks.len() < 2 {
        match answer(&x) {
            // The second load may have entered the generator again.
            StoreReply::Generating { position } => assert_eq!(position, A),
            StoreReply::Loaded { position, chunk } => {
                assert_eq!(position, A);
                chunks.push(chunk);
            }
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(chunks[0], made(A, VERSION, false));
    assert_eq!(chunks[1], chunks[0]);
    assert_eq!(control.dropped(A), 0);
    assert!((1..=2).contains(&control.entered(A)));
    flushed(&x);
    assert_eq!(chunk_files(&disk, Survival::Everything), 1);
    assert_eq!(stored(&disk, A), Some(made(A, VERSION, false)));
}

/// T10, item 1b, second half: a save of another chunk by the same handle, and a load
/// of that chunk after it, wait for the load being generated, and the load finds the
/// save without entering the generator.
#[test]
fn a_save_behind_a_chunk_being_generated_keeps_its_place() {
    let (store, control, _disk) = fresh();
    let witness = open(&store, hello(HOME, 1));
    let x = open(&store, hello(EAST, 1));
    control.hold(&[A]);
    let kept = changed(&made(C, VERSION, false));
    load(&x, A);
    save(&x, C, &kept);
    load(&x, C);
    assert_eq!(answer(&x), generating(A));
    flushed(&witness);
    quiet(&x);
    assert_eq!(control.entered(C), 0);
    control.let_go(A);
    assert_eq!(answer(&x), loaded(A, made(A, VERSION, false)));
    assert_eq!(answer(&x), loaded(C, kept));
    assert_eq!(control.entered(C), 0);
}

/// The same handle's later save, checkpoint and flush wait for its load that is being
/// generated (ADR-0021, section 7, the table): none of them is durable or answered
/// until the chunk is let go.
#[test]
fn the_same_handles_later_save_checkpoint_and_flush_wait_for_a_chunk_being_generated() {
    let (store, control, disk) = fresh();
    let witness = open(&store, hello(HOME, 1));
    let x = open(&store, hello(EAST, 1));
    log(&x, 1, &[]);
    committed(&x, 1);
    control.hold(&[A]);
    let kept = changed(&made(C, VERSION, false));
    load(&x, A);
    save(&x, C, &kept);
    checkpoint_of(&x, 1);
    x.request(StoreRequest::Flush);
    assert_eq!(answer(&x), generating(A));
    flushed(&witness);
    quiet(&x);
    assert_eq!(stored(&disk, C), None);
    assert_eq!(state_tick(&restored_after_crash(&disk, EAST)), None);

    control.let_go(A);
    assert_eq!(answer(&x), loaded(A, made(A, VERSION, false)));
    assert_eq!(answer(&x), StoreReply::Flushed);
    assert_eq!(stored(&disk, C), Some(kept));
    assert_eq!(stored(&disk, A), Some(made(A, VERSION, false)));
    assert_eq!(state_tick(&restored_after_crash(&disk, EAST)), Some(1));
}

/// T10, item 1c: when the generator fails, the load is answered `Generating` and then
/// `Unreadable`, nothing is stored, and a later load asks the generator again.
#[test]
fn a_chunk_the_generator_fails_for_is_answered_unreadable_and_not_stored() {
    let (store, control, disk) = fresh();
    let x = open(&store, hello(EAST, 1));
    control.hold(&[A]);
    control.fail(A);
    load(&x, A);
    assert_eq!(answer(&x), generating(A));
    control.wait_entered(A, 1);
    control.let_go(A);
    assert_eq!(answer(&x), StoreReply::Unreadable { position: A });
    flushed(&x);
    assert!(!written(&disk, A));
    assert_eq!(chunk_files(&disk, Survival::Everything), 0);

    load(&x, A);
    assert_eq!(answer(&x), generating(A));
    assert_eq!(answer(&x), StoreReply::Unreadable { position: A });
    assert_eq!(control.entered(A), 2);
}

/// T10, item 1d, and S6: a handle that is lost while its load is being generated has
/// the request dropped, is answered nothing more, and nothing is stored for it.
#[test]
fn a_lost_handles_load_being_generated_is_dropped_unanswered_and_not_noted() {
    let (store, control, disk) = fresh();
    let x = open(&store, hello(EAST, 1));
    control.hold(&[A]);
    load(&x, A);
    assert_eq!(answer(&x), generating(A));
    control.wait_entered(A, 1);
    // Another owner takes the region over, which loses `x`.
    let new = open(&store, hello(EAST, 2));
    assert!(x.is_lost());
    // A job for the thread to look at the lost handle's load by.
    flushed(&new);
    control.wait_dropped(A, 1);
    control.let_go(A);
    flushed(&new);
    // Nothing more is answered; the store may have closed the lost handle's way back.
    assert!(x.replies.try_recv().is_err());
    assert!(!written(&disk, A));
}

/// T10, item 1e: another region's loads, saves, checkpoint, flush and return are done
/// while a chunk of this one is being generated, and are durable.
#[test]
fn another_region_does_not_wait_for_a_chunk_being_generated() {
    let (store, control, disk) = fresh();
    let x = open(&store, hello(EAST, 1));
    let y = open(&store, hello(HOME, 1));
    let w = open(&store, hello(WEST, 1));
    assert_eq!(claim(&y, &[S, D, E]).0, [S, D, E]);
    let kept = changed(&made(S, VERSION, false));
    save(&y, S, &kept);
    flushed(&y);

    control.hold(&[A]);
    load(&x, A);
    assert_eq!(answer(&x), generating(A));

    load(&y, S);
    assert_eq!(answer(&y), loaded(S, kept));
    assert_eq!(generated(&y, D), made(D, VERSION, false));
    let home = changed(&made(ORIGIN, VERSION, false));
    log(&y, 1, &[]);
    save(&y, ORIGIN, &home);
    checkpoint_of(&y, 1);
    flushed(&y);
    assert_eq!(stored(&disk, ORIGIN), Some(home));
    assert_eq!(stored(&disk, D), Some(made(D, VERSION, false)));
    assert_eq!(state_tick(&restored_after_crash(&disk, HOME)), Some(1));

    give_back(&y, &[E]);
    flushed(&y);
    assert!(!boxed(&store.regions().unwrap(), HOME, E));
    // Nobody's any more: another region is granted it.
    assert_eq!(claim(&w, &[E]).0, [E]);

    // All of that while the chunk was held.
    quiet(&x);
    control.let_go(A);
    assert_eq!(answer(&x), loaded(A, made(A, VERSION, false)));
}

/// A merge, and the checkpoints and flushes it stands still for, do not wait for
/// another region's chunk being generated (ADR-0021, section 7, "A checkpoint that a
/// merge or a split waits for never waits behind another region's generation").
#[test]
fn a_merge_does_not_wait_for_another_regions_chunk_being_generated() {
    let (store, control, _disk) = fresh();
    let survivor = open(&store, hello(WEST, 1));
    let other = open(&store, hello(EAST, 1));
    let x = open(&store, hello(HOME, 1));
    assert_eq!(claim(&x, &[FREE]).0, [FREE]);
    control.hold(&[FREE]);
    load(&x, FREE);
    assert_eq!(answer(&x), generating(FREE));

    checkpoint(&other, 2);
    checkpoint(&survivor, 2);
    survivor.request(StoreRequest::AbsorbCommit {
        absorbed: RegionId(EAST),
        absorbed_epoch: 1,
        tick: 10,
        state: whole("merged", 10),
    });
    let done = StoreReply::Absorbed {
        absorbed: RegionId(EAST),
        chunks: Vec::new(),
        pinned: gap().pinned[1..].to_vec(),
    };
    assert_eq!(answer(&survivor), done);
    quiet(&x);
    control.let_go(FREE);
    assert_eq!(answer(&x), loaded(FREE, made(FREE, VERSION, false)));
}

/// T10, item 1f: the barrier of `Store::flush` waits for every job, a load being
/// generated among them, also when nobody else has asked for anything.
#[test]
fn the_barrier_of_flush_waits_for_a_chunk_being_generated() {
    let (store, control, _disk) = fresh();
    let x = open(&store, hello(EAST, 1));
    let y = open(&store, hello(HOME, 1));
    control.hold(&[A]);
    load(&x, A);
    assert_eq!(answer(&x), generating(A));
    let barrier = store.barrier();
    flushed(&y);
    assert!(matches!(barrier.try_recv(), Err(TryRecvError::Empty)));
    control.let_go(A);
    Store::rested(&barrier).unwrap();
    // Every answer it owes has gone out by then.
    assert!(matches!(
        x.replies.try_recv(),
        Ok(StoreReply::Loaded { position: A, .. })
    ));
}

/// T10, item 1g: a restore waits for every job before it and for the chunks it has
/// generated, and keeps nobody else waiting meanwhile. The chunk it changed is stored
/// with the change and not as generated.
#[test]
fn a_restore_waits_for_everything_before_it_and_keeps_nobody_waiting() {
    let (store, control, disk) = fresh();
    let x = open(&store, hello(EAST, 1));
    let y = open(&store, hello(HOME, 1));
    let z = open(&store, hello(WEST, 1));
    let (bx, by, bz) = block_in(R);
    log(&z, 1, &[(bx, by, bz, blocks::GLASS)]);
    committed(&z, 1);

    control.hold(&[A, R]);
    load(&x, A);
    assert_eq!(answer(&x), generating(A));
    let (sent, answered) = mpsc::channel();
    store
        .messages
        .send(Message::Open {
            hello: hello(WEST, 2),
            reply_to: store.messages.clone(),
            answer: sent,
        })
        .unwrap();
    flushed(&y);
    assert!(matches!(answered.try_recv(), Err(TryRecvError::Empty)));

    control.let_go(A);
    assert_eq!(answer(&x), loaded(A, made(A, VERSION, false)));
    control.wait_entered(R, 1);
    log(&y, 1, &[]);
    checkpoint_of(&y, 1);
    flushed(&y);
    assert_eq!(state_tick(&restored_after_crash(&disk, HOME)), Some(1));
    assert!(matches!(answered.try_recv(), Err(TryRecvError::Empty)));

    control.let_go(R);
    let (opened, restored) = answered.recv_timeout(PATIENCE).unwrap().unwrap();
    drop(StoreHandle::local(opened, store.messages.clone()));
    assert_eq!(ticks(&restored), [1]);
    let chunk = stored(&disk, R).expect("the restored chunk is durable");
    let (ix, iz) = ((bx - R.x * 16) as usize, (bz - R.z * 16) as usize);
    assert_eq!(chunk.get(ix, by, iz), Some(blocks::GLASS));
    assert_eq!(chunk.generated(), 0);
}

/// T10, item 1h: a restore over a load of its chunk that a lost handle has under way
/// drops that request, asks the generator again, and stores the chunk with the change.
#[test]
fn a_restore_over_a_request_under_way_asks_the_generator_again() {
    let (store, control, disk) = fresh();
    let x = open(&store, hello(EAST, 1));
    let (bx, by, bz) = block_in(A);
    log(&x, 1, &[(bx, by, bz, blocks::GLASS)]);
    committed(&x, 1);
    control.hold(&[A]);
    load(&x, A);
    assert_eq!(answer(&x), generating(A));

    let (sent, answered) = mpsc::channel();
    store
        .messages
        .send(Message::Open {
            hello: hello(EAST, 2),
            reply_to: store.messages.clone(),
            answer: sent,
        })
        .unwrap();
    control.wait_dropped(A, 1);
    control.wait_entered(A, 2);
    control.let_go(A);
    let (opened, restored) = answered.recv_timeout(PATIENCE).unwrap().unwrap();
    let new = StoreHandle::local(opened, store.messages.clone());
    assert_eq!(ticks(&restored), [1]);
    let chunk = stored(&disk, A).expect("the restored chunk is durable");
    assert_eq!(chunk.get(3, by, 4), Some(blocks::GLASS));
    assert_eq!(chunk.generated(), 0);
    flushed(&new);
    assert!(x.is_lost());
    // Nothing more is answered; the store may have closed the lost handle's way back.
    assert!(x.replies.try_recv().is_err());
}

/// T10, item 1i, and S6: a return of a chunk whose load is being generated withdraws
/// the load: the request is dropped, the handle is answered `Withdrawn`, the chunk is
/// free and nothing is stored. Claimed and loaded again, it is generated again.
#[test]
fn a_return_withdraws_the_load_of_its_chunk_being_generated() {
    let (store, control, disk) = fresh();
    let x = open(&store, hello(HOME, 1));
    assert_eq!(claim(&x, &[FREE]).0, [FREE]);
    control.hold(&[FREE]);
    load(&x, FREE);
    give_back(&x, &[FREE]);
    assert_eq!(answer(&x), generating(FREE));
    assert_eq!(answer(&x), StoreReply::Withdrawn { position: FREE });
    control.wait_dropped(FREE, 1);
    flushed(&x);
    assert!(!written(&disk, FREE));
    assert!(!boxed(&store.regions().unwrap(), HOME, FREE));

    assert_eq!(claim(&x, &[FREE]).0, [FREE]);
    load(&x, FREE);
    assert_eq!(answer(&x), generating(FREE));
    control.wait_entered(FREE, 2);
    control.let_go(FREE);
    assert_eq!(answer(&x), loaded(FREE, made(FREE, VERSION, false)));
    // The answered request is not taken for one that nobody waits for.
    assert_eq!(control.dropped(FREE), 1);
}

/// T10, item 1j, first half, and S7: `close` does not return while a chunk is being
/// generated; once it is let go, `close` returns and the chunk is durable.
#[test]
fn close_waits_for_a_chunk_being_generated_and_then_writes_it() {
    let (store, control, disk) = fresh();
    let x = open(&store, hello(EAST, 1));
    let y = open(&store, hello(HOME, 1));
    control.hold(&[A]);
    load(&x, A);
    assert_eq!(answer(&x), generating(A));
    let (sent, closed) = mpsc::channel();
    let closing = store.clone();
    let closer = thread::spawn(move || sent.send(closing.close().is_ok()).unwrap());
    // A job after the close's: were the close done, its answer would be on its way.
    // This can only miss a close that returned too early, never fail one that waits.
    flushed(&y);
    assert!(matches!(closed.try_recv(), Err(TryRecvError::Empty)));
    assert_eq!(stored(&disk, A), None);
    control.let_go(A);
    assert!(closed.recv_timeout(PATIENCE).unwrap(), "close failed");
    closer.join().unwrap();
    assert_eq!(stored(&disk, A), Some(made(A, VERSION, false)));
}

/// T10, item 1j, second half, and item 3: with a load being generated, the store and
/// every handle are dropped; once the chunk is let go both threads end, and nothing
/// was done to the disk meanwhile: the note is not written.
#[test]
fn the_threads_end_once_everything_is_dropped_and_the_request_is_answered() {
    let (store, control, disk) = fresh();
    let x = open(&store, hello(EAST, 1));
    control.hold(&[A]);
    load(&x, A);
    assert_eq!(answer(&x), generating(A));
    store.regions().unwrap();
    let operations = disk.operations();
    drop((x, store));
    control.let_go(A);
    control.wait_gone();
    assert_eq!(disk.operations(), operations);
    assert!(!written(&disk, A));
}

/// Test 7j: with nothing under way, the thread for chunks ends when the last sender is
/// gone, notes that are pending unwritten (T10, item 3).
#[test]
fn the_threads_end_once_everything_is_dropped_with_nothing_under_way() {
    let (store, control, disk) = fresh();
    let x = open(&store, hello(EAST, 1));
    generated(&x, A);
    store.flush().unwrap();
    let operations = disk.operations();
    drop((x, store));
    control.wait_gone();
    assert_eq!(disk.operations(), operations);
    assert!(!written(&disk, A));
}

// S1 to S8.

/// S1 and S2: what a region was handed is what a later load is answered with, from
/// the note, without the generator being asked again; a save takes its place.
#[test]
fn a_chunk_handed_out_is_what_later_loads_are_answered_with_until_a_save() {
    let (store, control, _disk) = fresh();
    let x = open(&store, hello(EAST, 1));
    let first = generated(&x, A);
    assert_eq!(first, made(A, VERSION, false));
    load(&x, A);
    assert_eq!(answer(&x), loaded(A, first.clone()));
    assert_eq!(control.entered(A), 1);
    let kept = changed(&first);
    save(&x, A, &kept);
    load(&x, A);
    assert_eq!(answer(&x), loaded(A, kept));
    assert_eq!(control.entered(A), 1);
}

/// What another handle does in [`a_note_is_durable_after_the_next_sync_of_any_handle`].
#[derive(Debug, Clone, Copy)]
enum Syncing {
    Checkpoint,
    Return,
    Flush,
    Restore,
}

/// S2: a note is durable after the next sync of any handle, a checkpoint, a return, a
/// flush or a restore of another region, and not before: neither the answer nor
/// `Store::flush` writes it.
#[test]
fn a_note_is_durable_after_the_next_sync_of_any_handle() {
    for sync in [
        Syncing::Checkpoint,
        Syncing::Return,
        Syncing::Flush,
        Syncing::Restore,
    ] {
        let (store, _control, disk) = fresh();
        let x = open(&store, hello(EAST, 1));
        let y = open(&store, hello(HOME, 1));
        let z = open(&store, hello(WEST, 1));
        assert_eq!(claim(&y, &[FREE]).0, [FREE]);
        let (bx, by, bz) = block_in(R);
        log(&z, 1, &[(bx, by, bz, blocks::GLASS)]);
        log(&y, 1, &[]);
        committed(&z, 1);
        committed(&y, 1);

        let chunk = generated(&x, A);
        store.flush().unwrap();
        assert_eq!(chunk_files(&disk, Survival::Everything), 0, "{sync:?}");

        match sync {
            Syncing::Checkpoint => checkpoint_of(&y, 1),
            Syncing::Return => give_back(&y, &[FREE]),
            Syncing::Flush => flushed(&y),
            Syncing::Restore => drop(open(&store, hello(WEST, 2))),
        }
        store.flush().unwrap();
        let stored = stored(&disk, A);
        assert_eq!(stored, Some(chunk), "{sync:?}");
        // S4: marked as generated by the generator's version.
        assert_eq!(stored.unwrap().generated(), VERSION, "{sync:?}");
    }
}

/// The loads that make `count` notes for a region of the east, from the `first`.
fn many(first: usize, count: usize) -> Vec<ChunkPos> {
    (first..first + count)
        .map(|i| ChunkPos::new(16 + (i % 64) as i32, (i / 64) as i32))
        .collect()
}

/// Loads every chunk of `positions`, all generated at once, and waits for the answers.
fn load_all(handle: &StoreHandle, positions: &[ChunkPos]) {
    for position in positions {
        load(handle, *position);
    }
    let mut chunks = 0;
    while chunks < positions.len() {
        match answer(handle) {
            StoreReply::Generating { .. } => {}
            StoreReply::Loaded { .. } => chunks += 1,
            other => panic!("{other:?}"),
        }
    }
}

/// S2: 511 notes are not written by themselves, the 512th has them all written.
#[test]
fn notes_are_written_when_512_are_pending_and_not_before() {
    let (store, _control, disk) = fresh();
    let x = open(&store, hello(EAST, 1));
    load_all(&x, &many(0, 511));
    store.flush().unwrap();
    assert_eq!(chunk_files(&disk, Survival::Everything), 0);
    load_all(&x, &many(511, 1));
    store.flush().unwrap();
    assert_eq!(chunk_files(&disk, Survival::Nothing), 512);
}

/// S2: notes do not count towards the 128 saves that have what is pending written:
/// 128 notes and 127 saves write nothing, the 128th save writes all of them.
#[test]
fn notes_do_not_count_towards_the_limit_of_saves() {
    let (store, _control, disk) = fresh();
    let x = open(&store, hello(EAST, 1));
    load_all(&x, &many(0, 128));
    store.flush().unwrap();
    assert_eq!(chunk_files(&disk, Survival::Everything), 0);
    let saved = many(128, 128);
    for position in &saved[..127] {
        save(&x, *position, &changed(&made(*position, VERSION, false)));
    }
    store.flush().unwrap();
    assert_eq!(chunk_files(&disk, Survival::Everything), 0);
    save(&x, saved[127], &changed(&made(saved[127], VERSION, false)));
    store.flush().unwrap();
    assert_eq!(chunk_files(&disk, Survival::Nothing), 256);
}

/// S5 and section 7's table: a note never takes the place of a save. A save behind a
/// load being generated replaces the note; a load behind a save finds the save.
#[test]
fn a_note_never_takes_the_place_of_a_save() {
    let (store, control, disk) = fresh();
    let x = open(&store, hello(EAST, 1));
    control.hold(&[A]);
    let kept = changed(&made(A, VERSION, false));
    load(&x, A);
    save(&x, A, &kept);
    assert_eq!(answer(&x), generating(A));
    control.wait_entered(A, 1);
    control.let_go(A);
    assert_eq!(answer(&x), loaded(A, made(A, VERSION, false)));
    flushed(&x);
    assert_eq!(stored(&disk, A), Some(kept));

    let other = changed(&made(B, VERSION, false));
    save(&x, B, &other);
    load(&x, B);
    assert_eq!(answer(&x), loaded(B, other.clone()));
    assert_eq!(control.entered(B), 0);
    flushed(&x);
    assert_eq!(stored(&disk, B), Some(other));
}

/// What [`a_failed_sync_loses_the_handles_that_saved_and_keeps_the_notes`] does up to
/// the sync that fails: one region is handed a generated chunk, another saves one.
fn before_the_failed_sync(disk: &Arc<MemoryDisk>) -> (Store, [StoreHandle; 3]) {
    let (toy, _control) = toy_of(VERSION);
    let store = store_on(disk, toy);
    let x = open(&store, hello(EAST, 1));
    let y = open(&store, hello(HOME, 1));
    let w = open(&store, hello(WEST, 1));
    generated(&x, A);
    save(&y, ORIGIN, &changed(&made(ORIGIN, VERSION, false)));
    store.flush().unwrap();
    (store, [x, y, w])
}

/// S5: a sync that fails loses the handle that saved and drops its save, as today, and
/// keeps the notes, which the next sync writes; the handle that was only handed a
/// generated chunk is not lost.
#[test]
fn a_failed_sync_loses_the_handles_that_saved_and_keeps_the_notes() {
    // The first change or sync after those of the run up to it is the first of the
    // sync of the chunks: that one fails.
    let operations = {
        let disk = Arc::new(MemoryDisk::default());
        let _kept = before_the_failed_sync(&disk);
        disk.operations()
    };
    let disk = Arc::new(MemoryDisk::failing(Fault::Fail(operations + 1)));
    let (_store, [x, y, w]) = before_the_failed_sync(&disk);
    assert_eq!(
        disk.operations(),
        operations,
        "the run up to the sync differs"
    );

    // The flush of a region that saved nothing is answered, also after a failure.
    flushed(&w);
    assert!(disk.operations() > operations, "the flush did not sync");
    assert!(y.is_lost());
    assert!(!x.is_lost());
    assert!(!w.is_lost());

    flushed(&w);
    assert_eq!(stored(&disk, A), Some(made(A, VERSION, false)));
    assert_eq!(stored(&disk, ORIGIN), None);
}

/// S7: `Store::flush` writes no note, `close` writes what is pending.
#[test]
fn close_writes_what_is_pending_and_flush_does_not() {
    let (store, _control, disk) = fresh();
    let x = open(&store, hello(EAST, 1));
    let chunk = generated(&x, A);
    store.flush().unwrap();
    assert!(!written(&disk, A));
    store.close().unwrap();
    assert_eq!(stored(&disk, A), Some(chunk));
    // It closes nothing.
    assert_eq!(generated(&x, B), made(B, VERSION, false));
}

/// S7 in a world in a directory: `flush` leaves no file for the chunk, `close` does,
/// and a store started on the world again loads it without generating it.
#[test]
fn close_writes_what_is_pending_in_a_world_on_disk() {
    let directory = tempfile::tempdir().unwrap();
    let path = FileChunks::new(Arc::new(MemoryDisk::default()), directory.path()).chunk_path(A);
    let (toy, control) = toy_of(VERSION);
    let store = Store::local_divided(directory.path(), toy, gap()).unwrap();
    let x = open(&store, hello(EAST, 1));
    let chunk = generated(&x, A);
    store.flush().unwrap();
    assert!(!path.exists());
    store.close().unwrap();
    assert!(path.exists());
    drop((x, store));
    control.wait_gone();

    let (toy, control) = toy_of(VERSION);
    let store = Store::local_divided(directory.path(), toy, gap()).unwrap();
    let x = open(&store, hello(EAST, 2));
    load(&x, A);
    assert_eq!(answer(&x), loaded(A, chunk));
    assert_eq!(control.entered(A), 0);
}

/// A world on a disk in memory that a generator of version 1 has stored `A` in, as
/// it made it, marked "with a failure" if `with_failure`, and changed after if
/// `change`.
fn marked_world(with_failure: bool, change: bool) -> (Arc<MemoryDisk>, Chunk) {
    let disk = Arc::new(MemoryDisk::default());
    let (toy, control) = toy_of(1);
    if with_failure {
        control.mark(A);
    }
    let store = store_on(&disk, toy);
    let x = open(&store, hello(EAST, 1));
    let mut chunk = generated(&x, A);
    assert_eq!(chunk.with_failure(), with_failure);
    if change {
        chunk = changed(&chunk);
        save(&x, A, &chunk);
    }
    store.close().unwrap();
    drop((x, store));
    control.wait_gone();
    assert_eq!(stored(&disk, A).as_ref(), Some(&chunk));
    (Arc::new(disk.crashed(Survival::Nothing)), chunk)
}

/// S8: a chunk stored with a failure by version 1 is made again by version 2, which
/// is answered and stored in its place.
#[test]
fn a_chunk_with_a_failure_is_made_again_by_a_newer_generator() {
    let (disk, _) = marked_world(true, false);
    let (toy, control) = toy_of(2);
    let store = store_on(&disk, toy);
    let x = open(&store, hello(EAST, 2));
    assert_eq!(generated(&x, A), made(A, 2, false));
    assert_eq!(control.entered(A), 1);
    store.close().unwrap();
    let stored = stored(&disk, A).unwrap();
    assert_eq!(stored, made(A, 2, false));
    assert_eq!(stored.generated(), 2);
}

/// S8: not by the same version, nor where the chunk has no failure.
#[test]
fn a_chunk_is_not_made_again_by_the_same_version_or_without_a_failure() {
    for (version, with_failure) in [(1, true), (2, false)] {
        let (disk, chunk) = marked_world(with_failure, false);
        let (toy, control) = toy_of(version);
        let store = store_on(&disk, toy);
        let x = open(&store, hello(EAST, 2));
        load(&x, A);
        assert_eq!(answer(&x), loaded(A, chunk), "{version} {with_failure}");
        assert_eq!(control.entered(A), 0);
    }
}

/// S8: a chunk with a failure that was changed has lost the mark and is never made
/// again.
#[test]
fn a_changed_chunk_is_not_made_again() {
    let (disk, chunk) = marked_world(true, true);
    assert_eq!(chunk.generated(), 0);
    let (toy, control) = toy_of(2);
    let store = store_on(&disk, toy);
    let x = open(&store, hello(EAST, 2));
    load(&x, A);
    assert_eq!(answer(&x), loaded(A, chunk));
    assert_eq!(control.entered(A), 0);
}

/// S8: if the newer generator fails for the chunk outright, the stored chunk is
/// answered as it is, and stays stored.
#[test]
fn a_chunk_the_newer_generator_fails_for_is_answered_as_stored() {
    let (disk, chunk) = marked_world(true, false);
    let (toy, control) = toy_of(2);
    control.fail(A);
    let store = store_on(&disk, toy);
    let x = open(&store, hello(EAST, 2));
    load(&x, A);
    assert_eq!(answer(&x), generating(A));
    assert_eq!(answer(&x), loaded(A, chunk.clone()));
    store.close().unwrap();
    assert_eq!(stored(&disk, A), Some(chunk));
}

// Section 9, item 7, besides T10 and S1 to S8.

/// Two regions and one chunk: generated for `x`, returned, claimed, changed and saved
/// by `y`, returned, claimed and loaded by `x` again: `x` gets `y`'s chunk.
#[test]
fn a_chunk_generated_for_one_region_and_changed_by_another_comes_back_changed() {
    let (store, control, _disk) = fresh();
    let x = open(&store, hello(HOME, 1));
    let y = open(&store, hello(WEST, 1));
    assert_eq!(claim(&x, &[FREE]).0, [FREE]);
    let first = generated(&x, FREE);
    give_back(&x, &[FREE]);
    flushed(&x);

    assert_eq!(claim(&y, &[FREE]).0, [FREE]);
    load(&y, FREE);
    assert_eq!(answer(&y), loaded(FREE, first.clone()));
    let kept = changed(&first);
    save(&y, FREE, &kept);
    give_back(&y, &[FREE]);
    flushed(&y);

    assert_eq!(claim(&x, &[FREE]).0, [FREE]);
    load(&x, FREE);
    assert_eq!(answer(&x), loaded(FREE, kept));
    assert_eq!(control.entered(FREE), 1);
}

/// Two regions and one chunk, with the first load withdrawn: `x`'s load is held when
/// `x` returns the chunk, `y` claims it and has it generated and changes it, and `y`'s
/// change is what is stored.
#[test]
fn a_withdrawn_load_leaves_the_chunk_to_the_next_holder() {
    let (store, control, disk) = fresh();
    let x = open(&store, hello(HOME, 1));
    let y = open(&store, hello(WEST, 1));
    assert_eq!(claim(&x, &[FREE]).0, [FREE]);
    control.hold(&[FREE]);
    load(&x, FREE);
    give_back(&x, &[FREE]);
    assert_eq!(answer(&x), generating(FREE));
    assert_eq!(answer(&x), StoreReply::Withdrawn { position: FREE });
    flushed(&x);
    control.wait_dropped(FREE, 1);

    assert_eq!(claim(&y, &[FREE]).0, [FREE]);
    load(&y, FREE);
    assert_eq!(answer(&y), generating(FREE));
    control.wait_entered(FREE, 2);
    control.let_go(FREE);
    let first = made(FREE, VERSION, false);
    assert_eq!(answer(&y), loaded(FREE, first.clone()));
    let kept = changed(&first);
    save(&y, FREE, &kept);
    flushed(&y);
    assert_eq!(stored(&disk, FREE), Some(kept));
}

/// A restore whose commits change a chunk that was handed out and never durable: on the
/// same store it finds the note, after a crash the chunk is generated again; either way
/// the change is in the stored chunk.
#[test]
fn a_restore_puts_its_changes_into_a_chunk_that_was_handed_out_and_never_durable() {
    let (store, control, disk) = fresh();
    let x = open(&store, hello(EAST, 1));
    generated(&x, A);
    let (bx, by, bz) = block_in(A);
    log(&x, 1, &[(bx, by, bz, blocks::GLASS)]);
    committed(&x, 1);
    let crashed = Arc::new(disk.crashed(Survival::Nothing));
    assert_eq!(stored(&crashed, A), None);

    let new = open(&store, hello(EAST, 2));
    flushed(&new);
    let chunk = stored(&disk, A).expect("the restored chunk is durable");
    assert_eq!(chunk.get(3, by, 4), Some(blocks::GLASS));
    assert_eq!(control.entered(A), 1);

    let (toy, control) = toy_of(VERSION);
    let store = store_on(&crashed, toy);
    let (new, restored) = store.open_region(hello(EAST, 2)).unwrap();
    assert_eq!(ticks(&restored), [1]);
    flushed(&new);
    let chunk = stored(&crashed, A).expect("the restored chunk is durable");
    assert_eq!(chunk.get(3, by, 4), Some(blocks::GLASS));
    assert_eq!(chunk.generated(), 0);
    assert_eq!(control.entered(A), 1);
}

/// A restore over a chunk the generator fails for leaves that chunk out and opens the
/// region; the chunk is not stored. (The error it logs with the number of changes lost
/// is written on the thread for chunks, which a test cannot listen to alone.)
#[test]
fn a_restore_over_a_chunk_the_generator_fails_for_leaves_it_out() {
    let (store, control, disk) = fresh();
    control.fail(A);
    let x = open(&store, hello(EAST, 1));
    let (bx, by, bz) = block_in(A);
    log(&x, 1, &[(bx, by, bz, blocks::GLASS)]);
    committed(&x, 1);
    let (new, restored) = store.open_region(hello(EAST, 2)).unwrap();
    assert_eq!(ticks(&restored), [1]);
    flushed(&new);
    assert!(!written(&disk, A));
    load(&new, A);
    assert_eq!(answer(&new), generating(A));
    assert_eq!(answer(&new), StoreReply::Unreadable { position: A });
}
