//! Tests of the region runner's part of merging and splitting, written from sections 3
//! and 8 of `docs/adr/0014-merging-and-splitting.md` and the scenarios R23 to R47 and K1
//! to K12 of its section 10, by someone who has not read how the runner does it.
//!
//! A test plays the edges, as in `chunks.rs`: it attaches the worker's end of a link to
//! a runner, numbers its messages itself and reads what comes back. Every runner is
//! stepped by the test, and the world store answers on threads of its own, so every
//! wait is a loop that steps until a message or a state is there. For a merge the test
//! plays the absorbed region's worker as well: it runs that region with a runner of its
//! own, releases it, opens it with a new epoch, reads its state with
//! [`absorbable`] and keeps the handle until the outcome is there. A crash is a region
//! opened again with a higher epoch.
//!
//! The worlds are **stripes** at a boundary at 1 (region 0 west, with the home chunk;
//! region 1 east), **three stripes** at 0 and 4, of which the middle one is home, and
//! **the gap**, in which the home region holds the home chunk alone between two pinned
//! regions.
//!
//! Every link's log is held to what section 5 of ADR-0012 says a region sends, after
//! every message read and whatever the test is about; see [`check`].
//!
//! The kills (K1 to K12) have an edge of their own, [`Edge`], which keeps what an edge
//! keeps and does with a welcome, its entries and its presence answers what section 8
//! says, so that a scenario can end where the run without a death ends. The
//! differential of section 2.6 runs a scenario in two worlds, once on the runner that
//! lived and once on the regions restored from the store, and compares what the links
//! were told without what depends on the pace of the store.
//!
//! No test here failed for something the runner does otherwise than the record says:
//! there is no ignored test in this file.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use clustine_data::{BlockState, blocks, items};
use clustine_region::{Layout, RegionId};
use clustine_rpc::link::{self, EdgeEnd};
use clustine_rpc::{
    Decline, EdgeMessage, EdgeToWorker, Off, Presence, RegionHello, RegionList, Restored,
    StoreReply, StoreRequest, Welcome, WorkerToEdge,
};
use clustine_sim::api::{
    EntityState, Face, HOTBAR_SLOTS, ItemStack, Misdirected, PlayerInput, PlayerTransfer, Pose,
    RegionEvent, RemoteAction, RemoteStep,
};
use clustine_sim::{
    Durable, EdgeState, Knowledge, Part, PlayerEvent, PlayerJoin, RegionConfig, RegionState,
};
use clustine_worker::{Ended, RegionRunner, Reshape, Reshaped, Stage, Worker, absorbable};
use clustine_world::{
    BlockPos, Chunk, ChunkArea, ChunkPos, EdgeId, EntityId, EntityIds, PlayerId, Vec3,
};
use clustine_worldgen::FlatGenerator;
use clustine_worldstore::{Division, Store, StoreError, StoreHandle};
use uuid::Uuid;

const E: EdgeId = EdgeId(11);
const F: EdgeId = EdgeId(22);
const G: EdgeId = EdgeId(33);

/// On stripes: the region that survives a merge or is split, which is the home region,
/// and the region that is absorbed. They are `A` and `B` of the record.
const A: RegionId = RegionId(0);
const B: RegionId = RegionId(1);

/// With three stripes the middle one is home.
const THREE_WEST: RegionId = RegionId(0);
const THREE_HOME: RegionId = RegionId(1);
const THREE_EAST: RegionId = RegionId(2);

/// With the gap: the regions pinned to the west and to the east, and the home region.
const GAP_EAST: RegionId = RegionId(1);
const GAP_HOME: RegionId = RegionId(2);

/// The chunk players enter every world in.
const HOME: ChunkPos = ChunkPos::new(0, 0);

/// The classic flat world has its grass at y = -61, and players stand on it.
const GROUND: i32 = -61;
const FEET: f64 = -60.0;

/// Players enter the world three blocks from the eastern end of the home chunk, so that
/// they can reach blocks of the chunk east of it and walk into it.
const SPAWN: Vec3 = Vec3::new(13.5, FEET, 8.5);

/// A block of the home chunk that a player at the spawn can reach.
const NEAR: BlockPos = BlockPos::new(12, GROUND, 9);

/// The chunk east of the home chunk: on stripes the first of region 1's stripe, with
/// the gap a free chunk, with three stripes one of the home stripe.
const NEXT: ChunkPos = ChunkPos::new(1, 0);

/// A block of [`NEXT`] that a player at the spawn can reach.
const BEYOND: BlockPos = BlockPos::new(16, GROUND, 8);

/// A block of the home chunk that a player at x = 16.5 can reach.
const BEHIND: BlockPos = BlockPos::new(15, GROUND, 8);

/// The chunk west of the home chunk: on stripes one of region 0's stripe.
const BACK: ChunkPos = ChunkPos::new(-1, 0);

/// A block of [`BACK`] that a player at x = 1.5 can reach.
const BACK_BLOCK: BlockPos = BlockPos::new(-1, GROUND, 8);

/// Chunks further west on the line the tests walk along.
const SECOND: ChunkPos = ChunkPos::new(-2, 0);
const FAR: ChunkPos = ChunkPos::new(-3, 0);
const FARTHER: ChunkPos = ChunkPos::new(-4, 0);

/// On stripes: a chunk of region 1's stripe that touches nothing a test walks or digs
/// in.
const OTHER: ChunkPos = ChunkPos::new(5, 3);

/// With the gap: the free chunk east of [`NEXT`].
const FREE: ChunkPos = ChunkPos::new(2, 0);

/// An entity of no region's block of ids, as a test makes one up for an arrival.
const STRANGER: EntityId = EntityId(900_000_001);

/// How many ticks a chunk a region holds outside its pinned areas may be without use
/// before the region gives it back, unless a test says otherwise: as the processes set
/// it, so that a region that has just lost its links keeps its chunks until they are
/// back.
const RETURN_AFTER: u64 = 600;

/// How often a wait steps the runner before it gives up. With a millisecond between
/// steps, a wait that cannot succeed fails after a few seconds.
const STEPS: usize = 4000;

/// Room on a link in each direction. A link that does not keep up is dropped, so this is
/// far more than any test leaves unread.
const CAPACITY: usize = 8192;

fn hotbar() -> [Option<ItemStack>; HOTBAR_SLOTS] {
    let mut hotbar = [None; HOTBAR_SLOTS];
    hotbar[0] = Some(ItemStack {
        item: items::STONE,
        count: 64,
    });
    hotbar
}

fn config(return_after: u64) -> RegionConfig {
    RegionConfig {
        spawn: SPAWN,
        starting_hotbar: hotbar(),
        return_after,
    }
}

fn player(n: u128) -> PlayerId {
    PlayerId(Uuid::from_u128(n))
}

/// How a test's world is divided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// Region 0 is pinned to the chunks with x below 1 and has the home chunk; region 1
    /// is pinned to the rest.
    Stripes,
    /// Region 0 is pinned to the chunks with x below 0, region 1 to those from 0 to 3,
    /// with the home chunk, and region 2 to those from 4 on.
    Three,
    /// Region 0 is pinned to the chunks west of x = 0 and region 1 to those from x = 16
    /// on; region 2 is home and holds the home chunk and nothing else, and the other
    /// chunks in between are free (ADR-0011, section 9).
    Gap,
}

impl Shape {
    fn layout(self) -> Option<Layout> {
        match self {
            Self::Stripes => Some(Layout::new(vec![1]).expect("one boundary is a layout")),
            Self::Three => Some(Layout::new(vec![0, 4]).expect("two boundaries are a layout")),
            Self::Gap => None,
        }
    }

    /// The region players enter the world in.
    fn home(self) -> RegionId {
        match self {
            Self::Stripes => A,
            Self::Three => THREE_HOME,
            Self::Gap => GAP_HOME,
        }
    }
}

/// A world store and the epochs its regions have been opened with.
struct World {
    store: Store,
    shape: Shape,
    /// The highest epoch the test has issued. Like a coordinator it issues every epoch
    /// above every other, whatever region it is for.
    epoch: u64,
    /// Kept so that the directory outlives the store.
    directory: Option<tempfile::TempDir>,
}

impl World {
    fn new(shape: Shape, on_disk: bool) -> Self {
        let division = match shape.layout() {
            Some(layout) => Division::stripes(HOME, &layout),
            None => Division {
                home: HOME,
                pinned: vec![
                    ChunkArea {
                        min_x: None,
                        max_x: Some(0),
                    },
                    ChunkArea {
                        min_x: Some(16),
                        max_x: None,
                    },
                ],
                layout: None,
            },
        };
        let generator = Arc::new(FlatGenerator::classic());
        let (store, directory) = if on_disk {
            let directory = tempfile::tempdir().expect("a temporary directory");
            let store = Store::local_divided(directory.path(), generator, division)
                .expect("a new world in an empty directory");
            (store, Some(directory))
        } else {
            let store =
                Store::memory_divided(generator, division).expect("the areas do not overlap");
            (store, None)
        };
        Self {
            store,
            shape,
            epoch: 0,
            directory,
        }
    }

    fn stripes() -> Self {
        Self::new(Shape::Stripes, false)
    }

    fn hello(&self, region: RegionId, epoch: u64) -> RegionHello {
        RegionHello {
            region,
            epoch,
            // The division with a gap is that of no layout, and its store asks for none.
            layout: self.shape.layout().map_or(0, |layout| layout.fingerprint()),
        }
    }

    /// An epoch above every one issued so far.
    fn next_epoch(&mut self) -> u64 {
        self.epoch += 1;
        self.epoch
    }

    /// Opens `region` with exactly `epoch`.
    fn open_with(
        &self,
        region: RegionId,
        epoch: u64,
    ) -> Result<(StoreHandle, Restored), StoreError> {
        self.store.open_region(self.hello(region, epoch))
    }

    /// Opens `region` as its next owner. Whoever had it before has lost it.
    fn open_raw(&mut self, region: RegionId) -> (StoreHandle, Restored) {
        let epoch = self.next_epoch();
        self.open_with(region, epoch)
            .expect("a higher epoch opens the region")
    }

    /// A runner for `region` as the store has it now.
    fn open(&mut self, region: RegionId) -> RegionRunner {
        self.open_returning_after(region, RETURN_AFTER)
    }

    fn open_returning_after(&mut self, region: RegionId, ticks: u64) -> RegionRunner {
        let (handle, restored) = self.open_raw(region);
        RegionRunner::restore(config(ticks), handle, restored)
            .expect("what the store has is readable")
    }

    /// Plays the worker that is told to absorb `region`, which its owner has let go
    /// of: opens it with a new epoch, reads its state and keeps the handle (section 4
    /// of the record, step 3).
    fn open_to_absorb(&mut self, region: RegionId) -> ToAbsorb {
        let epoch = self.next_epoch();
        let (handle, restored) = self
            .open_with(region, epoch)
            .expect("a higher epoch opens the region");
        let state = absorbable(&handle, restored).expect("what the store has is readable");
        ToAbsorb {
            region,
            _handle: handle,
            epoch,
            state,
        }
    }

    /// Plays the worker that has split `part` off: says hello for it with `as_epoch`
    /// and runs it from what it has in memory.
    fn run_part(&self, region: RegionId, as_epoch: u64, part: Part) -> RegionRunner {
        let (handle, _) = self
            .open_with(region, as_epoch)
            .expect("the epoch of the split opens the part");
        RegionRunner::of_part(part, handle)
    }

    fn list(&self) -> RegionList {
        self.store.regions().expect("the store can say what it has")
    }

    /// Where a world on disk is.
    fn root(&self) -> &Path {
        self.directory
            .as_ref()
            .expect("the world is on disk")
            .path()
    }
}

/// Runs a test in a world of `shape` on a store in memory, whose commits are confirmed
/// almost at once, and on one on disk, whose commits take longer than a step, so that
/// what a tick produced is still held when the next things happen.
fn in_memory_and_on_disk(shape: Shape, test: fn(World)) {
    test(World::new(shape, false));
    test(World::new(shape, true));
}

/// What the worker of a merge's survivor holds of the region to absorb.
struct ToAbsorb {
    region: RegionId,
    /// Kept open until the outcome is there: the store declines a merge whose absorbed
    /// region has no owner with that epoch. Dropping this is what closes it.
    _handle: StoreHandle,
    epoch: u64,
    state: RegionState,
}

impl ToAbsorb {
    fn order(&self) -> Reshape {
        Reshape::Absorb {
            absorbed: self.region,
            absorbed_epoch: self.epoch,
            state: self.state.clone(),
        }
    }
}

/// A region the test plays through a handle of its own, with no runner.
struct Other {
    handle: StoreHandle,
}

impl Other {
    /// Waits for the store's next answer. The store answers on threads of its own,
    /// whatever the runner under test does meanwhile.
    fn reply(&self) -> StoreReply {
        for _ in 0..30_000 {
            match self.handle.try_reply() {
                Some(reply) => return reply,
                None => thread::sleep(Duration::from_millis(1)),
            }
        }
        panic!("the store did not answer the region the test plays");
    }

    /// Claims `chunk`: `Ok` if it is granted, or the region the store says holds it.
    fn claim(&self, chunk: ChunkPos) -> Result<(), RegionId> {
        self.handle.request(StoreRequest::Claim {
            chunks: vec![chunk],
        });
        match self.reply() {
            StoreReply::Claimed { granted, foreign } => match (&granted[..], &foreign[..]) {
                ([mine], []) if *mine == chunk => Ok(()),
                ([], [(theirs, holder)]) if *theirs == chunk => Err(*holder),
                _ => panic!("a claim of {chunk:?} was answered {granted:?} and {foreign:?}"),
            },
            other => panic!("expected the answer to a claim, got {other:?}"),
        }
    }

    /// Loads `position`, which the region the test plays has to hold.
    fn load(&self, position: ChunkPos) -> Chunk {
        self.handle.request(StoreRequest::Load { position });
        match self.reply() {
            StoreReply::Loaded {
                position: loaded,
                chunk,
            } if loaded == position => chunk,
            other => panic!("expected the chunk at {position:?}, got {other:?}"),
        }
    }

    /// Stores `chunk` at `position`. The region the test plays commits nothing, so the
    /// save is as of tick 0.
    fn save(&self, position: ChunkPos, chunk: &Chunk) {
        self.handle.request(StoreRequest::Save {
            position,
            tick: 0,
            chunk: chunk.clone(),
        });
    }
}

/// What a subscription is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Viewer,
    Guest,
}

/// What a subscription message of a link said of a chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Said {
    /// `Subscribe`, `SubscribeAsGuest` or a list of the hello.
    Asked(Kind),
    /// `Unsubscribe`.
    Ended,
}

/// What a region answered a subscription with, whatever else the message carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Snapshot,
    Elsewhere(RegionId),
    NotMine,
}

/// What a link remembers of what it said and read, to hold the next message to
/// section 5 of ADR-0012.
#[derive(Default)]
struct Book {
    /// The number of the last subscription message sent on the link.
    asked: u64,
    /// Every subscription message that named a chunk, with its number, in order. The
    /// lists of the hello are number 0.
    said: BTreeMap<ChunkPos, Vec<(u64, Said)>>,
    /// Every answer read for a chunk, with its `ask`, in order.
    answered: BTreeMap<ChunkPos, Vec<(u64, Answer)>>,
    /// Whether the hello was the first thing the link sent, once it has said hello.
    hello_first: Option<bool>,
    /// How many players the hello named.
    named: usize,
    /// Whether the link has sent anything.
    sent: bool,
    /// Where the welcome is in the log, how many entries it announced and how many
    /// presence answers follow those.
    welcome: Option<(usize, usize, usize)>,
    /// The last message read that names its tick: where it is, and the tick.
    ticked: Option<(usize, u64)>,
    outbox: Option<u64>,
    progress: Option<u64>,
}

impl Book {
    fn note(&mut self, number: u64, said: Said, chunks: &[ChunkPos]) {
        self.asked = self.asked.max(number);
        // A chunk named twice in one message is named once.
        for chunk in chunks.iter().collect::<BTreeSet<_>>() {
            self.said.entry(*chunk).or_default().push((number, said));
        }
    }

    /// Whether nothing of `chunk` may come in a `TickDelta` (rule 31): the link never
    /// named it, or the answer to its last asking was `Elsewhere` or `NotMine`. After an
    /// `Unsubscribe` events can still come for a moment, so that does not count.
    fn quiet(&self, chunk: ChunkPos) -> bool {
        match self.said.get(&chunk).and_then(|said| said.last()) {
            None => true,
            Some((_, Said::Ended)) => false,
            Some((number, Said::Asked(_))) => self.answered.get(&chunk).is_some_and(|answers| {
                answers
                    .iter()
                    .any(|(ask, answer)| ask == number && *answer != Answer::Snapshot)
            }),
        }
    }
}

/// What an edge says first on a link. It says the `since` the region last told it
/// and the last entry it has seen, which a test names, as it plays an edge that has
/// seen what the test says it has.
#[derive(Debug, Clone)]
struct Greeting {
    edge: EdgeId,
    start: u64,
    since: u64,
    seen: u64,
    players: Vec<PlayerId>,
    chunks: Vec<ChunkPos>,
    guests: Vec<ChunkPos>,
}

/// The hello of an edge that has had nothing from the region and names nothing.
fn greeting(edge: EdgeId, start: u64) -> Greeting {
    Greeting {
        edge,
        start,
        since: 0,
        seen: 0,
        players: Vec::new(),
        chunks: Vec::new(),
        guests: Vec::new(),
    }
}

impl Greeting {
    fn since(mut self, since: u64) -> Self {
        self.since = since;
        self
    }

    fn seen(mut self, seen: u64) -> Self {
        self.seen = seen;
        self
    }

    fn players(mut self, players: Vec<PlayerId>) -> Self {
        self.players = players;
        self
    }

    fn chunks(mut self, chunks: Vec<ChunkPos>) -> Self {
        self.chunks = chunks;
        self
    }

    fn guests(mut self, guests: Vec<ChunkPos>) -> Self {
        self.guests = guests;
        self
    }
}

/// The edge's end of a link, with everything read from it so far.
struct Link {
    end: EdgeEnd,
    log: Vec<WorkerToEdge>,
    closed: bool,
    book: Book,
    /// The number of the last numbered message sent with [`Link::next`]. A link that
    /// carries on an edge's numbering is told where that is.
    sent: u64,
}

impl Link {
    fn attach(runner: &RegionRunner) -> Self {
        let (edge, worker) = link::in_process::<EdgeMessage, WorkerToEdge>(CAPACITY);
        runner.links().attach(worker);
        Self::of(edge)
    }

    fn of(end: EdgeEnd) -> Self {
        Self {
            end,
            log: Vec::new(),
            closed: false,
            book: Book::default(),
            sent: 0,
        }
    }

    /// The link numbers what it sends with [`Link::next`] from `sent + 1`.
    fn after(mut self, sent: u64) -> Self {
        self.sent = sent;
        self
    }

    fn send(&mut self, message: EdgeMessage) {
        self.book.sent = true;
        self.end
            .try_send(message)
            .expect("the link is open and has room");
    }

    fn plain(&mut self, body: EdgeToWorker) {
        self.send(EdgeMessage::unnumbered(body));
    }

    fn numbered(&mut self, number: u64, body: EdgeToWorker) {
        self.sent = self.sent.max(number);
        self.send(EdgeMessage {
            number: Some(number),
            body,
        });
    }

    /// Sends `body` with the next number of the edge's numbering for the region, and
    /// returns that number.
    fn next(&mut self, body: EdgeToWorker) -> u64 {
        let number = self.sent + 1;
        self.numbered(number, body);
        number
    }

    fn hello(&mut self, greeting: Greeting) {
        self.book.hello_first = Some(!self.book.sent);
        self.book.named = greeting.players.len();
        // A chunk in both lists is a viewer's (ADR-0012, section 4.5).
        let only_guests: Vec<ChunkPos> = greeting
            .guests
            .iter()
            .filter(|chunk| !greeting.chunks.contains(chunk))
            .copied()
            .collect();
        self.book
            .note(0, Said::Asked(Kind::Viewer), &greeting.chunks);
        self.book.note(0, Said::Asked(Kind::Guest), &only_guests);
        self.plain(EdgeToWorker::Hello {
            edge: greeting.edge,
            start: greeting.start,
            since: greeting.since,
            seen: greeting.seen,
            players: greeting.players,
            chunks: greeting.chunks,
            guests: greeting.guests,
        });
    }

    fn say(&mut self, said: Said, chunks: Vec<ChunkPos>) -> u64 {
        let ask = self.book.asked + 1;
        self.book.note(ask, said, &chunks);
        self.plain(match said {
            Said::Asked(Kind::Viewer) => EdgeToWorker::Subscribe { ask, chunks },
            Said::Asked(Kind::Guest) => EdgeToWorker::SubscribeAsGuest { ask, chunks },
            Said::Ended => EdgeToWorker::Unsubscribe { ask, chunks },
        });
        ask
    }

    /// Subscribes to `chunks` for a viewer of one of this region's players, and returns
    /// the number of the message.
    fn subscribe(&mut self, chunks: Vec<ChunkPos>) -> u64 {
        self.say(Said::Asked(Kind::Viewer), chunks)
    }

    /// Subscribes to `chunks` for a viewer of another region's player.
    fn subscribe_as_guest(&mut self, chunks: Vec<ChunkPos>) -> u64 {
        self.say(Said::Asked(Kind::Guest), chunks)
    }

    fn unsubscribe(&mut self, chunks: Vec<ChunkPos>) -> u64 {
        self.say(Said::Ended, chunks)
    }

    /// Reads whatever has arrived, and notes whether the worker has closed the link.
    fn drain(&mut self) {
        loop {
            match self.end.try_recv() {
                Ok(Some(message)) => {
                    self.log.push(message);
                    check(&self.log, &mut self.book);
                }
                Ok(None) => break,
                Err(_) => {
                    self.closed = true;
                    break;
                }
            }
        }
    }

    /// Every answer read for `chunk`, with its `ask`, in the order they came.
    fn answers(&self, chunk: ChunkPos) -> Vec<(u64, Answer)> {
        self.book.answered.get(&chunk).cloned().unwrap_or_default()
    }

    /// Whether the welcome, the entries it announced and the presence answers it
    /// announced have all been read.
    fn resumed(&self) -> bool {
        self.book
            .welcome
            .is_some_and(|(at, entries, answers)| self.log.len() > at + entries + answers)
    }
}

/// The chunk, the `ask` and the kind of an answer to a subscription.
fn answer_of(message: &WorkerToEdge) -> Option<(ChunkPos, u64, Answer)> {
    match message {
        WorkerToEdge::ChunkSnapshot { position, ask, .. } => {
            Some((*position, *ask, Answer::Snapshot))
        }
        WorkerToEdge::Elsewhere { chunk, ask, region } => {
            Some((*chunk, *ask, Answer::Elsewhere(*region)))
        }
        WorkerToEdge::NotMine { chunk, ask } => Some((*chunk, *ask, Answer::NotMine)),
        _ => None,
    }
}

/// Where in what one tick produced for a link a message belongs, by the list of section
/// 5.2 of ADR-0012. The resume is the first item there and is held to its place by
/// [`check`].
fn place(message: &WorkerToEdge) -> Option<u8> {
    match message {
        WorkerToEdge::Welcome(_) | WorkerToEdge::Presence { .. } => Some(1),
        WorkerToEdge::TickDelta { .. } => Some(2),
        WorkerToEdge::ToPlayer {
            event: PlayerEvent::Spawned { .. },
            ..
        } => Some(3),
        WorkerToEdge::Outbox {
            entry: Durable::Departed { .. },
            ..
        } => Some(6),
        WorkerToEdge::Outbox { .. } => Some(4),
        WorkerToEdge::ToPlayer {
            event: PlayerEvent::Acknowledged { .. },
            ..
        } => Some(5),
        WorkerToEdge::ChunkSnapshot { .. }
        | WorkerToEdge::Elsewhere { .. }
        | WorkerToEdge::NotMine { .. } => Some(7),
        WorkerToEdge::Progress { .. } => Some(8),
        // Nothing makes these since ADR-0008, and section 5.2 gives them no place.
        WorkerToEdge::ToPlayer { .. }
        | WorkerToEdge::Remote(_)
        | WorkerToEdge::RemoteDone { .. } => None,
    }
}

/// Holds messages that one tick produced to the order of section 5.2: each in its
/// place, and the answers to subscriptions in ascending order of their chunks.
fn check_tick(of_one_tick: &[WorkerToEdge], context: &dyn Fn() -> String) {
    let mut before = 0;
    let mut chunk_before: Option<ChunkPos> = None;
    for message in of_one_tick {
        let Some(place) = place(message) else {
            continue;
        };
        assert!(
            place >= before,
            "{message:?} is behind something that section 5.2 puts behind it in a tick: {}",
            context()
        );
        if let Some((chunk, _, _)) = answer_of(message) {
            assert!(
                chunk_before.is_none_or(|before| before < chunk),
                "the answers of a tick are not in ascending order of the chunks at {chunk:?}: {}",
                context()
            );
            chunk_before = Some(chunk);
        }
        before = place;
    }
}

/// Holds the message just read to what section 5 of ADR-0012 and rule 37 of ADR-0014
/// say of every link, whatever the test is about.
///
/// - Section 5.2, item 1, and 5.3: the welcome is the first thing on a link that began
///   with a hello and the only one; exactly as many outbox entries as it announced
///   follow it, then exactly as many presence answers as it announced, which is one
///   for each player of the hello at the least, and none later.
/// - Section 5.2: ticks are published one by one, so a `TickDelta` has a higher tick
///   than everything before it and a snapshot no lower one; what lies between two
///   messages of one tick is of that tick and in the order of the list there; a
///   `TickDelta` has events; outbox numbers ascend, and progress never goes back.
/// - Rule 8: an answer carries the number of a `Subscribe`, a `SubscribeAsGuest` or a
///   hello of this link that named the chunk.
/// - Rule 9: no asking is answered twice, and no answer has a lower number than one
///   before it for the chunk.
/// - Rule 10: `Elsewhere` answers an asking that was a viewer's, `NotMine` one that
///   was a guest's.
/// - Rule 12: once a snapshot of a chunk has come, nothing more answers the chunk
///   unless the link has ended the subscription in between.
/// - Rule 31: an event comes only for a chunk the link has a subscription to that
///   waits or is served. The removal of an entity is passed over here, as the one of
///   an entity nobody will pass on comes on every link (rule 33).
fn check(log: &[WorkerToEdge], book: &mut Book) {
    let (last, _) = log.split_last().expect("a message was just read");
    let index = log.len() - 1;
    let context = || brief(log);

    match last {
        WorkerToEdge::Welcome(welcome) => {
            assert!(book.welcome.is_none(), "a second welcome: {}", context());
            let first = book
                .hello_first
                .unwrap_or_else(|| panic!("a welcome without a hello: {}", context()));
            assert!(
                !first || index == 0,
                "a welcome that is not first: {}",
                context()
            );
            let (entries, answers) = match welcome {
                Welcome::Resumed {
                    entries, presences, ..
                }
                | Welcome::Unknown {
                    entries, presences, ..
                } => (*entries as usize, *presences as usize),
                Welcome::Superseded => (0, 0),
            };
            assert!(
                matches!(welcome, Welcome::Superseded) || answers >= book.named,
                "a welcome that announces {answers} presence answers for {} players named: {}",
                book.named,
                context()
            );
            book.welcome = Some((index, entries, answers));
        }
        other => {
            assert!(
                book.hello_first != Some(true) || matches!(log[0], WorkerToEdge::Welcome(_)),
                "something before the welcome: {}",
                context()
            );
            let is_presence = matches!(other, WorkerToEdge::Presence { .. });
            match book.welcome {
                Some((at, entries, answers)) if index - at <= entries => assert!(
                    matches!(other, WorkerToEdge::Outbox { .. }),
                    "the welcome announced {entries} entries, and {other:?} is among them (then {answers} presence answers): {}",
                    context()
                ),
                Some((at, entries, answers)) if index - at <= entries + answers => assert!(
                    is_presence,
                    "{other:?} is where a presence answer belongs: {}",
                    context()
                ),
                _ => assert!(
                    !is_presence,
                    "a presence answer that is not of the resume: {}",
                    context()
                ),
            }
        }
    }

    match last {
        WorkerToEdge::Outbox { number, .. } => {
            assert!(
                book.outbox.is_none_or(|earlier| earlier < *number),
                "outbox entry {number} after entry {:?}: {}",
                book.outbox,
                context()
            );
            book.outbox = Some(*number);
        }
        WorkerToEdge::Progress { applied, .. } => {
            assert!(
                book.progress.is_none_or(|earlier| earlier <= *applied),
                "progress {applied} after progress {:?}: {}",
                book.progress,
                context()
            );
            book.progress = Some(*applied);
        }
        _ => {}
    }

    let ticked = match last {
        WorkerToEdge::TickDelta { tick, .. } => Some((*tick, true)),
        WorkerToEdge::ChunkSnapshot { tick, .. } => Some((*tick, false)),
        _ => None,
    };
    if let Some((tick, is_delta)) = ticked {
        if let Some((at, earlier)) = book.ticked {
            assert!(
                earlier < tick || (earlier == tick && !is_delta),
                "something of tick {tick} after something of tick {earlier}: {}",
                context()
            );
            if earlier == tick {
                check_tick(&log[at..=index], &context);
            }
        }
        book.ticked = Some((index, tick));
    }

    if let WorkerToEdge::TickDelta { events, .. } = last {
        assert!(!events.is_empty(), "a delta without events: {}", context());
        for event in events {
            if matches!(event, RegionEvent::EntityRemoved { .. }) {
                continue;
            }
            let [one, other] = event.chunks();
            assert!(
                !book.quiet(one) || !book.quiet(other),
                "{event:?} came for a chunk the link has no subscription to that waits or is served: {}",
                context()
            );
        }
    }

    if let Some((chunk, ask, answer)) = answer_of(last) {
        let said = book.said.get(&chunk).cloned().unwrap_or_default();
        let asking = said.iter().find(|(number, _)| *number == ask);
        let fits = matches!(
            (asking, answer),
            (Some((_, Said::Asked(_))), Answer::Snapshot)
                | (Some((_, Said::Asked(Kind::Viewer))), Answer::Elsewhere(_))
                | (Some((_, Said::Asked(Kind::Guest))), Answer::NotMine)
        );
        assert!(
            fits,
            "{answer:?} with ask {ask} for {chunk:?}, of which the link said {said:?}: {}",
            context()
        );
        let before = book.answered.entry(chunk).or_default();
        for (earlier, what) in before.iter() {
            assert!(
                *earlier < ask,
                "{answer:?} with ask {ask} for {chunk:?} after an answer with ask {earlier}: {}",
                context()
            );
            let ended_between = said
                .iter()
                .any(|(number, what)| *what == Said::Ended && *earlier < *number && *number < ask);
            assert!(
                *what != Answer::Snapshot || ended_between,
                "{answer:?} with ask {ask} for {chunk:?}, which was served with ask {earlier} and not ended since: {}",
                context()
            );
        }
        before.push((ask, answer));
    }
}

/// One line per message, as a whole chunk is too much to read in a failure.
fn brief(log: &[WorkerToEdge]) -> String {
    log.iter()
        .enumerate()
        .map(|(index, message)| match message {
            WorkerToEdge::ChunkSnapshot {
                position,
                ask,
                tick,
                entities,
                ..
            } => format!(
                "{index}: snapshot of {position:?} for ask {ask} at {tick} with {entities:?}\n"
            ),
            other => format!("{index}: {other:?}\n"),
        })
        .collect()
}

/// Steps the runner until `done`, reading the links after every step.
fn run_until(
    runner: &mut RegionRunner,
    links: &mut [&mut Link],
    what: &str,
    mut done: impl FnMut(&RegionRunner, &[&mut Link]) -> bool,
) {
    for _ in 0..STEPS {
        runner.step();
        for link in links.iter_mut() {
            link.drain();
        }
        if done(runner, links) {
            return;
        }
        // The store answers on its own threads; this gives them the processor.
        thread::sleep(Duration::from_millis(1));
    }
    let logs: String = links
        .iter()
        .enumerate()
        .map(|(index, link)| {
            format!(
                "link {index} (closed: {}):\n{}",
                link.closed,
                brief(&link.log)
            )
        })
        .collect();
    panic!(
        "never happened: {what}; the region is at tick {} and the runner at {:?}\n{logs}",
        runner.region().tick_number(),
        runner.stage()
    );
}

/// Has the runner run exactly one more tick and returns its number. A runner that is
/// as far ahead of the store as it may be does not tick when it is stepped, so this
/// steps until it has.
fn one_tick(runner: &mut RegionRunner, links: &mut [&mut Link]) -> u64 {
    let tick = runner.region().tick_number() + 1;
    for _ in 0..STEPS {
        runner.step();
        for link in links.iter_mut() {
            link.drain();
        }
        let now = runner.region().tick_number();
        assert!(now <= tick, "a step ran more than one tick");
        if now == tick {
            return tick;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("the region never ran tick {tick}");
}

/// Has the runner run `count` more ticks.
fn ticks(runner: &mut RegionRunner, links: &mut [&mut Link], count: u64) {
    for _ in 0..count {
        one_tick(runner, links);
    }
}

/// A player no region has, and an entity none has given out.
const NOBODY: u128 = 0xdead;

/// A numbered message that changes nothing but how far the edge's messages count as
/// applied: an input for a stay the region does not have, which it passes over
/// without a word (rule 36).
fn nothing() -> EdgeToWorker {
    input(
        player(NOBODY),
        EntityId(0),
        1,
        PlayerInput::Move {
            position: None,
            rotation: None,
            on_ground: true,
        },
    )
}

/// Waits until everything of the ticks so far has been published on `links[through]`:
/// the link sends a numbered message that changes nothing, and the progress that covers
/// it, which is part of a tick like everything else, shows that every earlier tick is
/// out. A subscription would do as well, as in `chunks.rs`, but would leave the region
/// holding one more chunk of its areas, which a split counts.
fn sync(runner: &mut RegionRunner, links: &mut [&mut Link], through: usize) {
    let number = links[through].next(nothing());
    wait_applied(runner, links, through, number);
}

/// [`sync`] through every link, so that each has everything of the ticks so far.
fn settle(runner: &mut RegionRunner, links: &mut [&mut Link]) {
    for through in 0..links.len() {
        sync(runner, links, through);
    }
}

/// The answer to the asking `ask` for `chunk`, with its place in the log.
fn answer_to(log: &[WorkerToEdge], chunk: ChunkPos, ask: u64) -> Option<(usize, Answer)> {
    log.iter().enumerate().find_map(|(index, message)| {
        answer_of(message)
            .filter(|(of, number, _)| *of == chunk && *number == ask)
            .map(|(_, _, answer)| (index, answer))
    })
}

/// Waits for the answer to the asking `ask` for `chunk` on `links[through]`.
fn wait_answer(
    runner: &mut RegionRunner,
    links: &mut [&mut Link],
    through: usize,
    chunk: ChunkPos,
    ask: u64,
) -> Answer {
    run_until(runner, links, "the answer to a subscription", |_, links| {
        answer_to(&links[through].log, chunk, ask).is_some()
    });
    answer_to(&links[through].log, chunk, ask)
        .expect("the wait ended on it")
        .1
}

fn position_of(log: &[WorkerToEdge], found: impl Fn(&WorkerToEdge) -> bool) -> Option<usize> {
    log.iter().position(found)
}

/// The snapshot that answered the asking `ask` for `position`: its place in the log,
/// its content and the entities in it.
fn snapshot(
    log: &[WorkerToEdge],
    position: ChunkPos,
    ask: u64,
) -> Option<(usize, &Chunk, &Vec<EntityState>)> {
    log.iter()
        .enumerate()
        .find_map(|(index, message)| match message {
            WorkerToEdge::ChunkSnapshot {
                position: at,
                ask: number,
                chunk,
                entities,
                ..
            } if *at == position && *number == ask => Some((index, chunk, entities)),
            _ => None,
        })
}

/// Every event of every tick delta, with the place of its delta in the log and its tick.
fn events(log: &[WorkerToEdge]) -> Vec<(usize, u64, &RegionEvent)> {
    log.iter()
        .enumerate()
        .flat_map(|(index, message)| match message {
            WorkerToEdge::TickDelta { tick, events } => events
                .iter()
                .map(|event| (index, *tick, event))
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect()
}

/// Where and in which tick the link was told that `entity` is gone.
fn removal(log: &[WorkerToEdge], entity: EntityId) -> Option<(usize, u64)> {
    events(log)
        .into_iter()
        .find_map(|(index, tick, event)| match event {
            RegionEvent::EntityRemoved { entity: gone, .. } if *gone == entity => {
                Some((index, tick))
            }
            _ => None,
        })
}

/// Where and in which tick the link was told that `block` changed, and to what.
fn block_change(log: &[WorkerToEdge], block: BlockPos) -> Option<(usize, u64, BlockState)> {
    events(log)
        .into_iter()
        .find_map(|(index, tick, event)| match event {
            RegionEvent::BlockChanged { position, state } if *position == block => {
                Some((index, tick, *state))
            }
            _ => None,
        })
}

/// Where and in which tick the link was told that `entity` moved to `x`.
fn moved(log: &[WorkerToEdge], entity: EntityId, x: f64) -> Option<(usize, u64)> {
    events(log)
        .into_iter()
        .find_map(|(index, tick, event)| match event {
            RegionEvent::EntityMoved {
                entity: who, pose, ..
            } if *who == entity && pose.position.x == x => Some((index, tick)),
            _ => None,
        })
}

/// The entity `id` was told they entered the world with, and where in the log.
fn spawned(log: &[WorkerToEdge], id: PlayerId) -> Option<(usize, EntityId)> {
    log.iter()
        .enumerate()
        .find_map(|(index, message)| match message {
            WorkerToEdge::ToPlayer {
                player,
                event: PlayerEvent::Spawned { entity_id, .. },
            } if *player == id => Some((index, *entity_id)),
            _ => None,
        })
}

fn acknowledged(log: &[WorkerToEdge], id: PlayerId, up_to: i32) -> Option<usize> {
    position_of(log, |message| {
        matches!(message, WorkerToEdge::ToPlayer { player, event: PlayerEvent::Acknowledged { sequence } }
            if *player == id && *sequence == up_to)
    })
}

/// The outbox entries on the link, in the order they arrived, with their place.
fn outbox(log: &[WorkerToEdge]) -> Vec<(usize, u64, &Durable)> {
    log.iter()
        .enumerate()
        .filter_map(|(index, message)| match message {
            WorkerToEdge::Outbox { number, entry } => Some((index, *number, entry)),
            _ => None,
        })
        .collect()
}

/// Where the link was told that `id` was let go, with which number, to which region
/// and how.
fn departure(
    log: &[WorkerToEdge],
    id: PlayerId,
) -> Option<(usize, u64, RegionId, &PlayerTransfer)> {
    outbox(log)
        .into_iter()
        .find_map(|(index, number, entry)| match entry {
            Durable::Departed {
                player,
                transfer,
                to,
            } if *player == id => Some((index, number, *to, transfer)),
            _ => None,
        })
}

/// The last `applied` the link was told by a `Progress`.
fn applied(log: &[WorkerToEdge]) -> Option<u64> {
    log.iter().rev().find_map(|message| match message {
        WorkerToEdge::Progress { applied, .. } => Some(*applied),
        _ => None,
    })
}

/// Where the link was first told that its messages up to `number` are applied.
fn progress_to(log: &[WorkerToEdge], number: u64) -> Option<usize> {
    position_of(
        log,
        |message| matches!(message, WorkerToEdge::Progress { applied, .. } if *applied >= number),
    )
}

fn presence(log: &[WorkerToEdge], id: PlayerId) -> Option<&Presence> {
    log.iter().find_map(|message| match message {
        WorkerToEdge::Presence { player, answer } if *player == id => Some(answer),
        _ => None,
    })
}

/// The welcome on a link, as it is.
fn welcomed(log: &[WorkerToEdge]) -> Welcome {
    log.iter()
        .find_map(|message| match message {
            WorkerToEdge::Welcome(welcome) => Some(*welcome),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no welcome: {}", brief(log)))
}

/// The `since` the welcome on a link told the edge, which an edge says in every later
/// hello to that region.
fn since(log: &[WorkerToEdge]) -> u64 {
    match welcomed(log) {
        Welcome::Unknown { since, .. } => since,
        other => panic!("{other:?} tells no since: {}", brief(log)),
    }
}

/// The entries that follow a welcome, as many as it announced, each with its number.
fn entries(log: &[WorkerToEdge]) -> Vec<(u64, Durable)> {
    let at = position_of(log, |message| matches!(message, WorkerToEdge::Welcome(_)))
        .unwrap_or_else(|| panic!("no welcome: {}", brief(log)));
    let count = match welcomed(log) {
        Welcome::Resumed { entries, .. } | Welcome::Unknown { entries, .. } => entries as usize,
        Welcome::Superseded => 0,
    };
    log[at + 1..at + 1 + count]
        .iter()
        .map(|message| match message {
            WorkerToEdge::Outbox { number, entry } => (*number, entry.clone()),
            other => panic!("{other:?} among the entries of a welcome: {}", brief(log)),
        })
        .collect()
}

/// The presence answers on a link in the order they came: each player with the
/// entity of a `Present`, or `None` for an `Absent`.
fn presences(log: &[WorkerToEdge]) -> Vec<(PlayerId, Option<EntityId>)> {
    log.iter()
        .filter_map(|message| match message {
            WorkerToEdge::Presence { player, answer } => Some((
                *player,
                match answer {
                    Presence::Present { entity, .. } => Some(*entity),
                    Presence::Absent => None,
                },
            )),
            _ => None,
        })
        .collect()
}

/// The players of `edge` in a state, each with their entity, in ascending order: what
/// the presence answers to a hello that names nobody have to be (rule 37).
fn stays(state: &RegionState, edge: EdgeId) -> Vec<(PlayerId, Option<EntityId>)> {
    state
        .players
        .iter()
        .filter(|(_, player)| player.edge == edge)
        .map(|(id, player)| (*id, Some(player.entity_id)))
        .collect()
}

fn block_in(chunk: &Chunk, block: BlockPos) -> BlockState {
    let (x, z) = block.in_chunk();
    chunk.get(x, block.y, z).expect("the block is in the chunk")
}

/// The block at `block` as the region has it, if it has the chunk loaded.
fn block_of(runner: &RegionRunner, block: BlockPos) -> Option<BlockState> {
    runner
        .region()
        .chunk(block.chunk())
        .map(|chunk| block_in(chunk, block))
}

fn join(id: PlayerId) -> EdgeToWorker {
    EdgeToWorker::PlayerJoin(PlayerJoin {
        player: id,
        name: format!("player-{}", id.0.as_u128()),
    })
}

fn input(id: PlayerId, entity: EntityId, number: u64, input: PlayerInput) -> EdgeToWorker {
    EdgeToWorker::Input {
        player: id,
        entity,
        number,
        input,
    }
}

fn leave(id: PlayerId, entity: Option<EntityId>) -> EdgeToWorker {
    EdgeToWorker::PlayerLeave { player: id, entity }
}

/// A step along the line the tests walk on.
fn move_to(x: f64) -> PlayerInput {
    PlayerInput::Move {
        position: Some(Vec3::new(x, FEET, 8.5)),
        rotation: None,
        on_ground: true,
    }
}

fn dig(position: BlockPos, sequence: i32) -> PlayerInput {
    PlayerInput::Dig { position, sequence }
}

fn breaking(id: PlayerId, sequence: i32, position: BlockPos) -> RemoteAction {
    RemoteAction {
        player: id,
        sequence,
        step: RemoteStep::Break { position },
    }
}

fn arrive(id: PlayerId, transfer: &PlayerTransfer) -> EdgeToWorker {
    EdgeToWorker::PlayerArrive {
        player: id,
        transfer: transfer.clone(),
    }
}

/// A player as another region lets them go, standing at `x` on the line the tests walk
/// along.
fn transfer(entity: EntityId, x: f64) -> PlayerTransfer {
    PlayerTransfer {
        entity_id: entity,
        name: "arriving".to_owned(),
        pose: Pose::at(Vec3::new(x, FEET, 8.5)),
        hotbar: hotbar(),
        selected_slot: 0,
        last_input: 0,
    }
}

/// A new link that has said `greeting` and has been answered in full: the welcome with
/// its entries and presence answers, an answer to every chunk of both lists and the
/// progress of the hello's tick are there.
fn greeted(runner: &mut RegionRunner, greeting: Greeting) -> Link {
    let chunks: Vec<ChunkPos> = greeting
        .chunks
        .iter()
        .chain(&greeting.guests)
        .copied()
        .collect();
    let mut link = Link::attach(runner);
    link.hello(greeting);
    run_until(
        runner,
        &mut [&mut link],
        "the answer to a hello",
        |_, links| {
            links[0].closed
                || (links[0].resumed()
                    && applied(&links[0].log).is_some()
                    && chunks
                        .iter()
                        .all(|chunk| answer_to(&links[0].log, *chunk, 0).is_some()))
        },
    );
    assert!(!link.closed, "{}", brief(&link.log));
    link
}

/// Sends a join with the link's next number and waits until the player is told they
/// entered the world; returns the entity they were told.
fn join_and_wait(runner: &mut RegionRunner, link: &mut Link, id: PlayerId) -> EntityId {
    link.next(join(id));
    run_until(
        runner,
        &mut [link],
        "a player entering the world",
        |_, links| spawned(&links[0].log, id).is_some(),
    );
    spawned(&link.log, id).expect("just waited for it").1
}

/// Waits until the link has been told that its messages up to `number` are applied.
fn wait_applied(runner: &mut RegionRunner, links: &mut [&mut Link], through: usize, number: u64) {
    run_until(runner, links, "progress up to a number", |_, links| {
        applied(&links[through].log).is_some_and(|applied| applied >= number)
    });
}

/// Steps the runner until it has ended and closed its links, and says how it ended.
fn run_to_the_end(runner: &mut RegionRunner, links: &mut [&mut Link]) -> Ended {
    run_until(
        runner,
        links,
        "the runner ending and closing its links",
        |runner, links| runner.ended().is_some() && links.iter().all(|link| link.closed),
    );
    runner.ended().expect("the wait ended on it")
}

/// Where the outcome of a merge or a split arrives: `done` sends into a channel, as
/// the worker process has it.
struct Outcome(Receiver<Reshaped>);

impl Outcome {
    /// The outcome, if `done` has been called since this was last asked.
    fn taken(&self) -> Option<Reshaped> {
        self.0.try_recv().ok()
    }
}

/// Tells the runner to reshape its region.
fn ask(runner: &mut RegionRunner, reshape: Reshape) -> Outcome {
    let (said, heard) = mpsc::channel();
    runner.reshape(
        reshape,
        Box::new(move |reshaped| {
            // The test may have stopped listening, which is its own business.
            let _ = said.send(reshaped);
        }),
    );
    Outcome(heard)
}

/// The place of a stage in the order the record gives them, with running as 0.
fn rank(stage: Option<Stage>) -> u8 {
    match stage {
        None => 0,
        Some(Stage::Preparing) => 1,
        Some(Stage::Settling) => 2,
        Some(Stage::Closing) => 3,
        Some(Stage::Committing) => 4,
    }
}

/// Steps the runner until `done` has been called, and returns the outcome. On the way
/// it holds the runner to section 3.1: a step gets at most one stage further and none
/// back, the stage is `None` again as soon as the outcome is there, and the region runs
/// no tick from the stop on but the one of the merge or the split.
fn run_to_outcome(
    runner: &mut RegionRunner,
    links: &mut [&mut Link],
    outcome: &Outcome,
) -> Reshaped {
    let mut before = runner.stage();
    let mut stopped_at = None;
    for _ in 0..STEPS {
        runner.step();
        for link in links.iter_mut() {
            link.drain();
        }
        let now = runner.stage();
        let tick = runner.region().tick_number();
        if let Some(reshaped) = outcome.taken() {
            assert_eq!(now, None, "a stage beside the outcome {reshaped:?}");
            if let Some(stopped_at) = stopped_at {
                let took = !matches!(reshaped, Reshaped::Off { .. });
                assert_eq!(
                    tick,
                    stopped_at + u64::from(took),
                    "the region ticked on its own between the stop and {reshaped:?}"
                );
            }
            return reshaped;
        }
        assert!(
            rank(now) == rank(before) || rank(now) == rank(before) + 1,
            "a step went from {before:?} to {now:?}"
        );
        if rank(now) >= rank(Some(Stage::Settling)) {
            assert_eq!(
                *stopped_at.get_or_insert(tick),
                tick,
                "a tick ran at {now:?}"
            );
        }
        before = now;
        thread::sleep(Duration::from_millis(1));
    }
    panic!(
        "no outcome; the region is at tick {} and the runner at {:?}",
        runner.region().tick_number(),
        runner.stage()
    );
}

/// Steps the runner until it is at `stage`, and no further.
fn run_to_stage(runner: &mut RegionRunner, links: &mut [&mut Link], stage: Stage) {
    let mut before = runner.stage();
    for _ in 0..STEPS {
        if before == Some(stage) {
            return;
        }
        runner.step();
        for link in links.iter_mut() {
            link.drain();
        }
        let now = runner.stage();
        assert!(
            rank(now) == rank(before) + 1 || rank(now) == rank(before),
            "a step went from {before:?} to {now:?} on the way to {stage:?}"
        );
        before = now;
        thread::sleep(Duration::from_millis(1));
    }
    panic!("the runner never came to {stage:?}; it is at {before:?}");
}

// ---------------------------------------------------------------------------------------
// R23 to R27, section 3.6 and rule 34. The hold of a block action
// ---------------------------------------------------------------------------------------

/// Region 0 on stripes with edge E's link, which sees the home chunk, and player 1,
/// who has walked to the western end of the home chunk and can reach [`BACK_BLOCK`].
fn at_the_western_end(world: &mut World) -> (RegionRunner, Link, EntityId) {
    let mut runner = world.open(A);
    let mut e = greeted(&mut runner, greeting(E, 5).chunks(vec![HOME]));
    let entity = join_and_wait(&mut runner, &mut e, player(1));
    let walked = e.next(input(player(1), entity, 1, move_to(1.5)));
    wait_applied(&mut runner, &mut [&mut e], 0, walked);
    assert_eq!(
        runner
            .region()
            .player(player(1))
            .map(|(_, pose)| pose.position.x),
        Some(1.5)
    );
    sync(&mut runner, &mut [&mut e], 0);
    (runner, e, entity)
}

#[test]
fn with_the_gap_a_dig_behind_a_subscribe_for_a_chunk_that_is_held_and_not_loaded_waits_for_the_snapshot()
 {
    in_memory_and_on_disk(
        Shape::Gap,
        with_the_gap_a_dig_behind_a_subscribe_for_a_chunk_that_is_held_and_not_loaded_waits_for_the_snapshot_in,
    );
}

/// R23.
fn with_the_gap_a_dig_behind_a_subscribe_for_a_chunk_that_is_held_and_not_loaded_waits_for_the_snapshot_in(
    mut world: World,
) {
    let mut runner = world.open_returning_after(GAP_HOME, 40);
    let mut e = greeted(&mut runner, greeting(E, 5).chunks(vec![HOME, NEXT]));
    assert_eq!(e.answers(NEXT), vec![(0, Answer::Snapshot)]);
    let entity = join_and_wait(&mut runner, &mut e, player(1));

    // The link lets go of the chunk. The region keeps it for forty ticks and no
    // longer has it loaded.
    e.unsubscribe(vec![NEXT]);
    run_until(
        &mut runner,
        &mut [&mut e],
        "the chunk being dropped from memory",
        |runner, _| runner.region().chunk(NEXT).is_none(),
    );
    sync(&mut runner, &mut [&mut e], 0);
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Held);
    assert!(runner.region().chunk(NEXT).is_none());

    // With no step in between: the asking, a dig into the chunk and a step.
    let ask = e.subscribe(vec![NEXT]);
    let dug = e.next(input(player(1), entity, 1, dig(BEYOND, 1)));
    let walked = e.next(input(player(1), entity, 2, move_to(12.5)));
    wait_applied(&mut runner, &mut [&mut e], 0, walked);
    sync(&mut runner, &mut [&mut e], 0);

    let log = &e.log;
    let (shot, chunk, _) = snapshot(log, NEXT, ask).expect("the asking is answered");
    assert_eq!(
        block_in(chunk, BEYOND),
        blocks::GRASS_BLOCK,
        "the dig was judged before the snapshot was made: {}",
        brief(log)
    );
    let (broke, _, state) = block_change(log, BEYOND).expect("the block is broken");
    assert_eq!(state, blocks::AIR);
    let (stepped, _) = moved(log, entity, 12.5).expect("the step is applied");
    assert!(shot < broke && broke <= stepped, "{}", brief(log));
    assert!(
        progress_to(log, dug).expect("waited for it") > shot,
        "the dig counted as applied before the snapshot was out: {}",
        brief(log)
    );
    assert!(acknowledged(log, player(1), 1).is_some(), "{}", brief(log));
    assert!(outbox(log).is_empty(), "{}", brief(log));
    assert_eq!(block_of(&runner, BEYOND), Some(blocks::AIR));
    let state = runner.region().state();
    assert_eq!(state.players[&player(1)].last_input, 2);
    assert_eq!(state.players[&player(1)].pose.position.x, 12.5);
}

#[test]
fn on_stripes_a_dig_behind_a_subscribe_for_an_own_chunk_that_nothing_asked_about_waits_for_the_snapshot()
 {
    in_memory_and_on_disk(
        Shape::Stripes,
        on_stripes_a_dig_behind_a_subscribe_for_an_own_chunk_that_nothing_asked_about_waits_for_the_snapshot_in,
    );
}

/// R24. Until this step such a dig was passed on without a region.
fn on_stripes_a_dig_behind_a_subscribe_for_an_own_chunk_that_nothing_asked_about_waits_for_the_snapshot_in(
    mut world: World,
) {
    let (mut runner, mut e, entity) = at_the_western_end(&mut world);
    assert_eq!(runner.region().knowledge(BACK), Knowledge::Unknown);
    assert!(runner.region().pins(BACK));

    let ask = e.subscribe(vec![BACK]);
    let dug = e.next(input(player(1), entity, 2, dig(BACK_BLOCK, 1)));
    wait_applied(&mut runner, &mut [&mut e], 0, dug);
    sync(&mut runner, &mut [&mut e], 0);

    let log = &e.log;
    let (shot, chunk, _) = snapshot(log, BACK, ask).expect("the asking is answered");
    assert_eq!(block_in(chunk, BACK_BLOCK), blocks::GRASS_BLOCK);
    let (broke, _, state) = block_change(log, BACK_BLOCK).expect("the block is broken");
    assert_eq!(state, blocks::AIR);
    assert!(shot < broke, "{}", brief(log));
    assert!(progress_to(log, dug).expect("waited for it") > shot);
    assert!(acknowledged(log, player(1), 1).is_some(), "{}", brief(log));
    assert!(
        outbox(log).is_empty(),
        "the dig was passed on: {}",
        brief(log)
    );
    assert_eq!(block_of(&runner, BACK_BLOCK), Some(blocks::AIR));
}

/// Rule 34, first item: the same behind a guest's subscription, which makes a region
/// claim a chunk of its own pinned areas as a viewer's does.
#[test]
fn a_dig_behind_a_guests_subscription_for_an_own_chunk_waits_for_the_snapshot() {
    let mut world = World::stripes();
    let (mut runner, mut e, entity) = at_the_western_end(&mut world);

    let ask = e.subscribe_as_guest(vec![BACK]);
    let dug = e.next(input(player(1), entity, 2, dig(BACK_BLOCK, 1)));
    wait_applied(&mut runner, &mut [&mut e], 0, dug);
    sync(&mut runner, &mut [&mut e], 0);

    let log = &e.log;
    let (shot, chunk, _) = snapshot(log, BACK, ask).expect("the asking is answered");
    assert_eq!(block_in(chunk, BACK_BLOCK), blocks::GRASS_BLOCK);
    let (broke, _, _) = block_change(log, BACK_BLOCK).expect("the block is broken");
    assert!(shot < broke, "{}", brief(log));
    assert!(outbox(log).is_empty(), "{}", brief(log));
    assert_eq!(block_of(&runner, BACK_BLOCK), Some(blocks::AIR));
}

/// Section 3.6: "or has asked". The asking is taken by a tick before the dig is sent,
/// so the region has asked the store for the chunk, or has had its answer, or has the
/// chunk already, as the store's pace has it. Whichever it is, the dig is judged on
/// the loaded chunk and not before the tick of the snapshot.
#[test]
fn a_dig_sent_while_the_region_has_asked_for_its_own_chunk_is_judged_on_the_loaded_chunk() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_dig_sent_while_the_region_has_asked_for_its_own_chunk_is_judged_on_the_loaded_chunk_in,
    );
}

fn a_dig_sent_while_the_region_has_asked_for_its_own_chunk_is_judged_on_the_loaded_chunk_in(
    mut world: World,
) {
    let (mut runner, mut e, entity) = at_the_western_end(&mut world);
    let ask = e.subscribe(vec![BACK]);
    one_tick(&mut runner, &mut [&mut e]);
    assert_ne!(runner.region().knowledge(BACK), Knowledge::Unknown);

    let dug = e.next(input(player(1), entity, 2, dig(BACK_BLOCK, 1)));
    wait_applied(&mut runner, &mut [&mut e], 0, dug);
    sync(&mut runner, &mut [&mut e], 0);

    let log = &e.log;
    let shot = log
        .iter()
        .find_map(|message| match message {
            WorkerToEdge::ChunkSnapshot {
                position,
                ask: number,
                tick,
                ..
            } if *position == BACK && *number == ask => Some(*tick),
            _ => None,
        })
        .expect("the asking is answered");
    let (_, broke, state) = block_change(log, BACK_BLOCK).expect("the block is broken");
    assert_eq!(state, blocks::AIR);
    assert!(
        shot <= broke,
        "the block broke in tick {broke}, before the chunk was there in tick {shot}: {}",
        brief(log)
    );
    assert!(outbox(log).is_empty(), "{}", brief(log));
    assert_eq!(block_of(&runner, BACK_BLOCK), Some(blocks::AIR));
}

#[test]
fn a_dig_into_the_other_stripe_behind_a_subscribe_for_it_is_judged_in_the_tick_that_takes_it() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_dig_into_the_other_stripe_behind_a_subscribe_for_it_is_judged_in_the_tick_that_takes_it_in,
    );
}

/// R25: the narrow hold does not reach a chunk the region is not about to serve.
fn a_dig_into_the_other_stripe_behind_a_subscribe_for_it_is_judged_in_the_tick_that_takes_it_in(
    mut world: World,
) {
    let mut runner = world.open(A);
    let mut e = greeted(&mut runner, greeting(E, 5).chunks(vec![HOME]));
    let entity = join_and_wait(&mut runner, &mut e, player(1));
    sync(&mut runner, &mut [&mut e], 0);
    assert!(!runner.region().pins(NEXT));

    let ask = e.subscribe(vec![NEXT]);
    let dug = e.next(input(player(1), entity, 1, dig(BEYOND, 1)));
    let walked = e.next(input(player(1), entity, 2, move_to(12.5)));
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut e], 0, NEXT, ask),
        Answer::Elsewhere(B)
    );
    sync(&mut runner, &mut [&mut e], 0);

    let log = &e.log;
    let (told, _) = answer_to(log, NEXT, ask).expect("waited for it");
    let entries = outbox(log);
    assert!(
        matches!(
            entries.as_slice(),
            [(at, 1, Durable::Remote { action, to: None })]
                if *action == breaking(player(1), 1, BEYOND) && *at < told
        ),
        "{}",
        brief(log)
    );
    let (stepped, _) = moved(log, entity, 12.5).expect("the step is applied");
    assert!(stepped < told, "{}", brief(log));
    assert!(progress_to(log, dug).expect("applied") < told);
    assert!(progress_to(log, walked).expect("applied") < told);
}

/// R26, first half: only an action on a block waits for a chunk.
#[test]
fn a_move_behind_a_subscribe_that_waits_is_applied_at_once() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_move_behind_a_subscribe_that_waits_is_applied_at_once_in,
    );
}

fn a_move_behind_a_subscribe_that_waits_is_applied_at_once_in(mut world: World) {
    let (mut runner, mut e, entity) = at_the_western_end(&mut world);
    let before = runner.region().tick_number();
    let ask = e.subscribe(vec![BACK]);
    e.next(input(player(1), entity, 2, move_to(2.5)));
    // A join, a leave and an arrival are about no chunk either.
    let joined = e.next(join(player(2)));
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut e], 0, BACK, ask),
        Answer::Snapshot
    );
    sync(&mut runner, &mut [&mut e], 0);

    let log = &e.log;
    let (shot, _, _) = snapshot(log, BACK, ask).expect("waited for it");
    let (stepped, tick) = moved(log, entity, 2.5).expect("the step is applied");
    assert_eq!(tick, before + 1, "not in the tick that took it");
    assert!(stepped < shot, "{}", brief(log));
    assert!(spawned(log, player(2)).expect("joined").0 < shot);
    assert!(progress_to(log, joined).expect("applied") < shot);
}

/// R26, second half, and rule 34, third item: a dig into a chunk its link has no
/// subscription to is judged in the tick that takes it, also while another link waits
/// for that very chunk.
#[test]
fn a_dig_into_a_chunk_its_link_has_no_subscription_to_is_judged_at_once() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_dig_into_a_chunk_its_link_has_no_subscription_to_is_judged_at_once_in,
    );
}

fn a_dig_into_a_chunk_its_link_has_no_subscription_to_is_judged_at_once_in(mut world: World) {
    let (mut runner, mut e, entity) = at_the_western_end(&mut world);
    let mut f = greeted(&mut runner, greeting(F, 5));
    sync(&mut runner, &mut [&mut e, &mut f], 0);

    let asked = f.subscribe(vec![BACK]);
    e.next(input(player(1), entity, 2, dig(BACK_BLOCK, 1)));
    e.next(input(player(1), entity, 3, move_to(2.5)));
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut f, &mut e], 0, BACK, asked),
        Answer::Snapshot
    );
    settle(&mut runner, &mut [&mut e, &mut f]);

    // The other link's snapshot is of a later tick than the one that judged the dig,
    // and has the block as it was: the region did not have the chunk.
    let (_, chunk, _) = snapshot(&f.log, BACK, asked).expect("waited for it");
    assert_eq!(block_in(chunk, BACK_BLOCK), blocks::GRASS_BLOCK);
    let log = &e.log;
    let entries = outbox(log);
    assert!(
        matches!(
            entries.as_slice(),
            [(_, 1, Durable::Remote { action, to: None })]
                if *action == breaking(player(1), 1, BACK_BLOCK)
        ),
        "{}",
        brief(log)
    );
    assert!(block_change(log, BACK_BLOCK).is_none());
    assert!(block_change(&f.log, BACK_BLOCK).is_none());
    assert!(moved(log, entity, 2.5).is_some());
    assert_eq!(applied(log), Some(e.sent));
}

/// The path of the manifest of a stored chunk in a world on disk.
fn manifest_of(world: &World, position: ChunkPos) -> std::path::PathBuf {
    world
        .root()
        .join("manifests/overworld")
        .join(format!("{}.{}", position.x >> 5, position.z >> 5))
        .join(format!("{}.{}.manifest", position.x, position.z))
}

/// R27: a chunk the store cannot read is waited for without end, and holds nothing.
#[test]
fn a_chunk_the_store_cannot_read_does_not_hold_a_dig() {
    let mut world = World::new(Shape::Stripes, true);
    {
        // The chunk has to be in the store to be damaged there.
        let (handle, _) = world.open_raw(A);
        let owner = Other { handle };
        let chunk = owner.load(BACK);
        owner.save(BACK, &chunk);
        owner.handle.flush();
    }
    std::fs::write(manifest_of(&world, BACK), b"not a manifest").expect("the chunk was stored");

    let (mut runner, mut e, entity) = at_the_western_end(&mut world);
    e.subscribe(vec![BACK]);
    let dug = e.next(input(player(1), entity, 2, dig(BACK_BLOCK, 1)));
    let walked = e.next(input(player(1), entity, 3, move_to(2.5)));
    wait_applied(&mut runner, &mut [&mut e], 0, walked);
    sync(&mut runner, &mut [&mut e], 0);

    let log = &e.log;
    assert_eq!(e.answers(BACK), Vec::new(), "{}", brief(log));
    assert!(progress_to(log, dug).is_some());
    assert!(moved(log, entity, 2.5).is_some(), "{}", brief(log));
    assert!(block_change(log, BACK_BLOCK).is_none(), "{}", brief(log));
    assert!(runner.region().chunk(BACK).is_none());

    // Nor does it hold one that is sent when the store has long said so.
    e.subscribe(vec![BACK]);
    let again = e.next(input(player(1), entity, 4, dig(BACK_BLOCK, 2)));
    let back = e.next(input(player(1), entity, 5, move_to(1.5)));
    wait_applied(&mut runner, &mut [&mut e], 0, back);
    assert!(progress_to(&e.log, again).is_some());
    assert!(moved(&e.log, entity, 1.5).is_some(), "{}", brief(&e.log));
}

/// Section 3.6, "What it costs": behind one held action everything its link sent
/// waits, numbered or not, and nothing of another link does.
#[test]
fn behind_a_held_dig_everything_of_its_link_waits_and_nothing_of_another_link_does() {
    in_memory_and_on_disk(
        Shape::Stripes,
        behind_a_held_dig_everything_of_its_link_waits_and_nothing_of_another_link_does_in,
    );
}

fn behind_a_held_dig_everything_of_its_link_waits_and_nothing_of_another_link_does_in(
    mut world: World,
) {
    let (mut runner, mut e, entity) = at_the_western_end(&mut world);
    let mut f = greeted(&mut runner, greeting(F, 5).chunks(vec![HOME]));
    let other = join_and_wait(&mut runner, &mut f, player(2));
    settle(&mut runner, &mut [&mut e, &mut f]);

    let before = runner.region().tick_number();
    let ask = e.subscribe(vec![BACK]);
    let dug = e.next(input(player(1), entity, 2, dig(BACK_BLOCK, 1)));
    e.next(input(player(1), entity, 3, move_to(2.5)));
    let joined = e.next(join(player(3)));
    // A guest's asking for a chunk of the other stripe is answered in the tick that
    // takes it, which shows when that was.
    let guest = e.subscribe_as_guest(vec![OTHER]);
    f.next(input(player(2), other, 1, move_to(12.5)));
    run_until(
        &mut runner,
        &mut [&mut e, &mut f],
        "everything behind the dig being taken",
        |_, links| {
            applied(&links[0].log).is_some_and(|applied| applied >= joined)
                && answer_to(&links[0].log, OTHER, guest).is_some()
        },
    );
    settle(&mut runner, &mut [&mut e, &mut f]);

    let log = &e.log;
    let (shot, _, _) = snapshot(log, BACK, ask).expect("the asking is answered");
    let (broke, _, _) = block_change(log, BACK_BLOCK).expect("the block is broken");
    let (stepped, _) = moved(log, entity, 2.5).expect("the step is applied");
    let (entered, _) = spawned(log, player(3)).expect("the player entered");
    let (refused, answer) = answer_to(log, OTHER, guest).expect("waited for it");
    assert_eq!(answer, Answer::NotMine);
    for (what, at) in [
        ("the dig", broke),
        ("the step", stepped),
        ("the join", entered),
        ("the guest's asking", refused),
    ] {
        assert!(shot < at, "{what} did not wait: {}", brief(log));
    }
    assert!(progress_to(log, dug).expect("applied") > shot);
    // The other link's step was taken by the very next tick, and this link saw it,
    // as it sees the home chunk, before its own chunk came.
    let (others, tick) = moved(log, other, 12.5).expect("the other link's step is seen");
    assert_eq!(tick, before + 1);
    assert!(others < shot, "{}", brief(log));
    assert_eq!(moved(&f.log, other, 12.5).map(|(_, tick)| tick), Some(tick));
}

/// Section 3.6: a use of an item is about the chunk of the block and that of the spot
/// beside the clicked face.
#[test]
fn a_use_of_an_item_waits_for_the_chunk_of_the_spot_beside_the_clicked_face() {
    let mut world = World::stripes();
    let (mut runner, mut e, entity) = at_the_western_end(&mut world);
    // A stone on the last block of the home chunk, whose western face looks into the
    // chunk beside it.
    let base = BlockPos::new(0, GROUND, 8);
    let stone = Face::Top.neighbour(base);
    let spot = Face::West.neighbour(stone);
    assert_eq!((stone.chunk(), spot.chunk()), (HOME, BACK));
    let placed = e.next(input(
        player(1),
        entity,
        2,
        PlayerInput::UseItemOn {
            position: base,
            face: Face::Top,
            sequence: 1,
        },
    ));
    wait_applied(&mut runner, &mut [&mut e], 0, placed);
    sync(&mut runner, &mut [&mut e], 0);
    assert_eq!(block_of(&runner, stone), Some(blocks::STONE));
    assert_eq!(runner.region().knowledge(BACK), Knowledge::Unknown);

    let ask = e.subscribe(vec![BACK]);
    let used = e.next(input(
        player(1),
        entity,
        3,
        PlayerInput::UseItemOn {
            position: stone,
            face: Face::West,
            sequence: 2,
        },
    ));
    wait_applied(&mut runner, &mut [&mut e], 0, used);
    sync(&mut runner, &mut [&mut e], 0);

    let log = &e.log;
    let (shot, chunk, _) = snapshot(log, BACK, ask).expect("the asking is answered");
    assert_eq!(block_in(chunk, spot), blocks::AIR);
    let (built, _, state) = block_change(log, spot).expect("the block is placed");
    assert_eq!(state, blocks::STONE);
    assert!(shot < built, "{}", brief(log));
    assert!(progress_to(log, used).expect("waited for it") > shot);
    assert!(outbox(log).is_empty(), "{}", brief(log));
    assert_eq!(block_of(&runner, spot), Some(blocks::STONE));
}

/// Section 3.6: a remote action is about the chunk of every position its step names.
/// An edge asks a region for a chunk as a guest before it sends an action on to it
/// (rule 48), so this is how every action reaches a region that has just come by a
/// chunk.
#[test]
fn a_remote_action_behind_a_guests_subscription_for_its_chunk_waits_for_the_snapshot() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_remote_action_behind_a_guests_subscription_for_its_chunk_waits_for_the_snapshot_in,
    );
}

fn a_remote_action_behind_a_guests_subscription_for_its_chunk_waits_for_the_snapshot_in(
    mut world: World,
) {
    let mut runner = world.open(A);
    let mut e = greeted(&mut runner, greeting(E, 5));
    sync(&mut runner, &mut [&mut e], 0);

    // Breaking, behind a guest's asking.
    let ask = e.subscribe_as_guest(vec![BACK]);
    let broken = e.next(EdgeToWorker::Remote(breaking(player(9), 4, BACK_BLOCK)));
    let after = e.next(nothing());
    wait_applied(&mut runner, &mut [&mut e], 0, after);
    let log = &e.log;
    let (shot, chunk, _) = snapshot(log, BACK, ask).expect("the asking is answered");
    assert_eq!(block_in(chunk, BACK_BLOCK), blocks::GRASS_BLOCK);
    let (broke, _, state) = block_change(log, BACK_BLOCK).expect("the block is broken");
    assert_eq!(state, blocks::AIR);
    assert!(shot < broke, "{}", brief(log));
    assert!(progress_to(log, broken).expect("waited for it") > shot);
    let entries = outbox(log);
    assert!(
        matches!(
            entries.as_slice(),
            [(at, 1, Durable::RemoteDone { player: who, sequence: 4 })]
                if *who == player(9) && *at > shot
        ),
        "{}",
        brief(log)
    );

    // Placing, behind a viewer's: the step names its target.
    let target = BlockPos::new(-17, GROUND + 1, 8);
    assert_eq!(target.chunk(), SECOND);
    let ask = e.subscribe(vec![SECOND]);
    let placed = e.next(EdgeToWorker::Remote(RemoteAction {
        player: player(9),
        sequence: 5,
        step: RemoteStep::Place {
            target,
            block: blocks::STONE,
            placer: SPAWN,
        },
    }));
    wait_applied(&mut runner, &mut [&mut e], 0, placed);
    sync(&mut runner, &mut [&mut e], 0);
    let log = &e.log;
    let (shot, chunk, _) = snapshot(log, SECOND, ask).expect("the asking is answered");
    assert_eq!(block_in(chunk, target), blocks::AIR);
    let (built, _, state) = block_change(log, target).expect("the block is placed");
    assert_eq!(state, blocks::STONE);
    assert!(shot < built, "{}", brief(log));
    assert!(
        outbox(log)
            .iter()
            .any(|(at, _, entry)| *at > shot
                && matches!(entry, Durable::RemoteDone { sequence: 5, .. })),
        "{}",
        brief(log)
    );
}

/// Section 3.6: the hold is for a chunk the region "holds and has not loaded". One it
/// has loaded for another link holds nothing: the action is judged in the tick that
/// takes it, which is the tick that makes this link's snapshot.
#[test]
fn an_action_behind_a_subscription_for_a_chunk_that_is_loaded_is_judged_in_the_tick_that_takes_it()
{
    let mut world = World::stripes();
    let mut runner = world.open(A);
    let mut e = greeted(&mut runner, greeting(E, 5).chunks(vec![BACK]));
    let mut f = greeted(&mut runner, greeting(F, 5));
    settle(&mut runner, &mut [&mut e, &mut f]);
    assert!(runner.region().chunk(BACK).is_some());

    let before = runner.region().tick_number();
    let ask = f.subscribe_as_guest(vec![BACK]);
    let broken = f.next(EdgeToWorker::Remote(breaking(player(9), 1, BACK_BLOCK)));
    wait_applied(&mut runner, &mut [&mut f, &mut e], 0, broken);
    settle(&mut runner, &mut [&mut e, &mut f]);

    let (_, tick, state) = block_change(&e.log, BACK_BLOCK).expect("the block is broken");
    assert_eq!(state, blocks::AIR);
    assert_eq!(tick, before + 1, "{}", brief(&e.log));
    let (_, chunk, _) = snapshot(&f.log, BACK, ask).expect("the asking is answered");
    assert_eq!(
        block_in(chunk, BACK_BLOCK),
        blocks::AIR,
        "the snapshot is of the tick that judged the action"
    );
}

/// Section 3.6, "Why it is narrow": a chunk the region has asked for outside its
/// pinned areas holds nothing, be it free or another region's.
#[test]
fn with_the_gap_a_dig_behind_a_subscribe_for_a_chunk_outside_the_pinned_areas_is_judged_at_once() {
    in_memory_and_on_disk(
        Shape::Gap,
        with_the_gap_a_dig_behind_a_subscribe_for_a_chunk_outside_the_pinned_areas_is_judged_at_once_in,
    );
}

fn with_the_gap_a_dig_behind_a_subscribe_for_a_chunk_outside_the_pinned_areas_is_judged_at_once_in(
    mut world: World,
) {
    // The neighbour holds the chunk beyond the next one, which a player who has
    // walked to the end of the next one can reach.
    let (handle, _) = world.open_raw(GAP_EAST);
    let neighbour = Other { handle };
    assert_eq!(neighbour.claim(FREE), Ok(()));
    let mut runner = world.open(GAP_HOME);
    let mut e = greeted(&mut runner, greeting(E, 5).chunks(vec![HOME]));
    let entity = join_and_wait(&mut runner, &mut e, player(1));
    sync(&mut runner, &mut [&mut e], 0);

    // A free chunk: the region will be granted it, and still it judges the dig in
    // the tick that takes it, in which it neither holds the chunk nor is pinned
    // there.
    let ask = e.subscribe(vec![NEXT]);
    let dug = e.next(input(player(1), entity, 1, dig(BEYOND, 1)));
    let walked = e.next(input(player(1), entity, 2, move_to(31.5)));
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut e], 0, NEXT, ask),
        Answer::Snapshot
    );
    sync(&mut runner, &mut [&mut e], 0);
    let log = &e.log;
    let (shot, chunk, _) = snapshot(log, NEXT, ask).expect("waited for it");
    assert_eq!(block_in(chunk, BEYOND), blocks::GRASS_BLOCK);
    let entries = outbox(log);
    assert!(
        matches!(
            entries.as_slice(),
            [(at, 1, Durable::Remote { action, to: None })]
                if *action == breaking(player(1), 1, BEYOND) && *at < shot
        ),
        "{}",
        brief(log)
    );
    assert!(progress_to(log, dug).expect("applied") < shot);
    assert!(progress_to(log, walked).expect("applied") < shot);
    assert_eq!(block_of(&runner, BEYOND), Some(blocks::GRASS_BLOCK));

    // Another region's chunk.
    let block = BlockPos::new(32, GROUND, 8);
    assert_eq!(block.chunk(), FREE);
    let ask = e.subscribe(vec![FREE]);
    let dug = e.next(input(player(1), entity, 3, dig(block, 2)));
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut e], 0, FREE, ask),
        Answer::Elsewhere(GAP_EAST)
    );
    sync(&mut runner, &mut [&mut e], 0);
    let log = &e.log;
    let (told, _) = answer_to(log, FREE, ask).expect("waited for it");
    let entries = outbox(log);
    assert!(
        matches!(
            entries.as_slice(),
            [_, (at, 2, Durable::Remote { action, to: None })]
                if *action == breaking(player(1), 2, block) && *at < told
        ),
        "{}",
        brief(log)
    );
    assert!(progress_to(log, dug).expect("applied") < told);
}

/// Section 3.6: a chunk the region believes another's holds nothing, also while the
/// link asks about it again.
#[test]
fn a_dig_behind_a_second_subscribe_for_a_chunk_that_was_told_elsewhere_is_not_held() {
    let mut world = World::stripes();
    let mut runner = world.open(A);
    let mut e = greeted(&mut runner, greeting(E, 5).chunks(vec![HOME, NEXT]));
    assert_eq!(e.answers(NEXT), vec![(0, Answer::Elsewhere(B))]);
    let entity = join_and_wait(&mut runner, &mut e, player(1));
    sync(&mut runner, &mut [&mut e], 0);

    let before = runner.region().tick_number();
    let ask = e.subscribe(vec![NEXT]);
    e.next(input(player(1), entity, 1, dig(BEYOND, 1)));
    e.next(input(player(1), entity, 2, move_to(12.5)));
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut e], 0, NEXT, ask),
        Answer::Elsewhere(B)
    );
    sync(&mut runner, &mut [&mut e], 0);

    let log = &e.log;
    let (told, _) = answer_to(log, NEXT, ask).expect("waited for it");
    let entries = outbox(log);
    assert!(
        matches!(
            entries.as_slice(),
            [(at, 1, Durable::Remote { action, .. })]
                if *action == breaking(player(1), 1, BEYOND) && *at < told
        ),
        "{}",
        brief(log)
    );
    assert_eq!(
        moved(log, entity, 12.5).map(|(_, tick)| tick),
        Some(before + 1)
    );
}

// ---------------------------------------------------------------------------------------
// R28 to R30, section 3.7 and rules 36 to 38. Presence: the region says whom it has
// ---------------------------------------------------------------------------------------

/// Region 0 on stripes with players 1 and 2 of edge E and one entry that E has not
/// confirmed; the link, the two entities and the `since` the region told E.
fn two_players_and_an_entry(world: &mut World) -> (RegionRunner, Link, EntityId, EntityId, u64) {
    let mut runner = world.open(A);
    let mut e = greeted(&mut runner, greeting(E, 5).chunks(vec![HOME]));
    let one = join_and_wait(&mut runner, &mut e, player(1));
    let two = join_and_wait(&mut runner, &mut e, player(2));
    let dug = e.next(input(player(1), one, 1, dig(BEYOND, 1)));
    wait_applied(&mut runner, &mut [&mut e], 0, dug);
    sync(&mut runner, &mut [&mut e], 0);
    assert_eq!(outbox(&e.log).len(), 1, "{}", brief(&e.log));
    let since = since(&e.log);
    (runner, e, one, two, since)
}

#[test]
fn a_hello_that_names_neither_of_an_edges_two_players_is_answered_with_a_present_for_each() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_hello_that_names_neither_of_an_edges_two_players_is_answered_with_a_present_for_each_in,
    );
}

/// R28, first half.
fn a_hello_that_names_neither_of_an_edges_two_players_is_answered_with_a_present_for_each_in(
    mut world: World,
) {
    let (mut runner, e, one, two, since) = two_players_and_an_entry(&mut world);
    let again = greeted(&mut runner, greeting(E, 5).since(since).chunks(vec![HOME]));
    let log = &again.log;
    assert_eq!(
        welcomed(log),
        Welcome::Resumed {
            entries: 1,
            presences: 2,
            applied: e.sent,
        },
        "{}",
        brief(log)
    );
    assert!(matches!(
        entries(log).as_slice(),
        [(1, Durable::Remote { .. })]
    ));
    assert_eq!(
        presences(log),
        vec![(player(1), Some(one)), (player(2), Some(two))]
    );
    // The answers are behind the entries, and they are whole.
    assert!(matches!(log[1], WorkerToEdge::Outbox { .. }));
    assert!(matches!(log[2], WorkerToEdge::Presence { .. }));
    assert!(
        matches!(
            presence(log, player(1)),
            Some(Presence::Present { last_input: 1, pose, selected_slot: 0, hotbar: held, .. })
                if pose.position == SPAWN && *held == hotbar()
        ),
        "{}",
        brief(log)
    );
    assert!(matches!(
        presence(log, player(2)),
        Some(Presence::Present { last_input: 0, .. })
    ));
}

/// R28, second half: the hello's players first and in its order, whatever that is,
/// then every other stay in ascending order.
#[test]
fn a_hello_is_answered_for_its_players_in_its_order_and_then_for_every_other_stay() {
    let mut world = World::stripes();
    let (mut runner, mut e, one, two, since) = two_players_and_an_entry(&mut world);
    let three = join_and_wait(&mut runner, &mut e, player(3));
    // A player of another edge is no stay of this one.
    let mut f = greeted(&mut runner, greeting(F, 5));
    join_and_wait(&mut runner, &mut f, player(4));
    settle(&mut runner, &mut [&mut e, &mut f]);

    let again = greeted(
        &mut runner,
        greeting(E, 5)
            .since(since)
            .seen(1)
            .players(vec![player(2), player(9), player(4)]),
    );
    let log = &again.log;
    assert_eq!(
        welcomed(log),
        Welcome::Resumed {
            entries: 0,
            presences: 5,
            applied: e.sent,
        },
        "{}",
        brief(log)
    );
    assert_eq!(
        presences(log),
        vec![
            (player(2), Some(two)),
            (player(9), None),
            (player(4), None),
            (player(1), Some(one)),
            (player(3), Some(three)),
        ]
    );

    // And the other edge is told of its own player only.
    let other = greeted(&mut runner, greeting(F, 5).since(self::since(&f.log)));
    assert_eq!(presences(&other.log).len(), 1, "{}", brief(&other.log));
    assert_eq!(presences(&other.log)[0].0, player(4));
}

/// R29, first half, and section 3.7: after an `Unknown` that makes the state for the
/// edge the state has nobody of it.
#[test]
fn a_first_hello_is_answered_absent_for_every_name_and_with_nothing_applied() {
    let mut world = World::stripes();
    let (mut runner, _e, _, _, _) = two_players_and_an_entry(&mut world);
    let before = runner.region().tick_number();
    let f = greeted(
        &mut runner,
        greeting(F, 5).players(vec![player(2), player(7), player(1)]),
    );
    assert_eq!(
        welcomed(&f.log),
        Welcome::Unknown {
            since: before + 1,
            entries: 0,
            presences: 3,
            applied: 0,
        }
    );
    assert_eq!(
        presences(&f.log),
        vec![(player(2), None), (player(7), None), (player(1), None)]
    );
    assert_eq!(applied(&f.log), Some(0));
}

/// Section 3.7: an `Unknown` that resets the state, for a higher start of the edge
/// and for an edge that says another `since` than the region has for it, is answered
/// `Absent` for everyone the hello names, and those players are gone.
#[test]
fn a_hello_that_resets_the_edge_is_answered_absent_for_its_players() {
    for lost_its_since in [false, true] {
        let mut world = World::stripes();
        let (mut runner, mut e, one, _, since) = two_players_and_an_entry(&mut world);
        let greeting = if lost_its_since {
            greeting(E, 5).since(since + 1000)
        } else {
            greeting(E, 6).since(since)
        };
        let before = runner.region().tick_number();
        let again = greeted(
            &mut runner,
            greeting
                .seen(1)
                .players(vec![player(2), player(1)])
                .chunks(vec![HOME]),
        );
        let log = &again.log;
        assert_eq!(
            welcomed(log),
            Welcome::Unknown {
                since: before + 1,
                entries: 0,
                presences: 2,
                applied: 0,
            },
            "{}",
            brief(log)
        );
        assert_eq!(presences(log), vec![(player(2), None), (player(1), None)]);
        assert_eq!(runner.region().player_count(), 0);
        assert!(removal(log, one).is_some(), "{}", brief(log));
        // The link the edge had before is closed by the hello of the new one.
        e.drain();
        assert!(e.closed);
    }
}

/// R29, second half, where the next message waits on the link's hold.
#[test]
fn a_welcome_says_how_far_the_region_had_applied_when_the_next_message_waits_behind_the_hello() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_welcome_says_how_far_the_region_had_applied_when_the_next_message_waits_behind_the_hello_in,
    );
}

fn a_welcome_says_how_far_the_region_had_applied_when_the_next_message_waits_behind_the_hello_in(
    mut world: World,
) {
    let (mut runner, e, one, _, since) = two_players_and_an_entry(&mut world);
    let had = e.sent;
    let mut again = Link::attach(&runner).after(had);
    again.hello(
        greeting(E, 5)
            .since(since)
            .seen(1)
            .players(vec![player(1)])
            .chunks(vec![HOME, BACK]),
    );
    let walked = again.next(input(player(1), one, 2, move_to(12.5)));
    wait_applied(&mut runner, &mut [&mut again], 0, walked);

    let log = &again.log;
    assert_eq!(
        welcomed(log),
        Welcome::Resumed {
            entries: 0,
            presences: 2,
            applied: had,
        },
        "{}",
        brief(log)
    );
    let first = log
        .iter()
        .find_map(|message| match message {
            WorkerToEdge::Progress { applied, .. } => Some(*applied),
            _ => None,
        })
        .expect("a progress comes with the tick of every hello");
    assert_eq!(first, had, "{}", brief(log));
    // The step waited for the chunks of the hello, as ever.
    let (stepped, _) = moved(log, one, 12.5).expect("the step is applied");
    for chunk in [HOME, BACK] {
        assert!(answer_to(log, chunk, 0).expect("answered").0 < stepped);
    }
}

/// R29, second half, where the next message came too late: it was sent on the edge's
/// old link in the step in which the new link said hello. The welcome is of the state
/// before that tick whether or not the tick still took the message, and the tick's
/// progress says the same or more.
#[test]
fn a_welcome_says_how_far_the_region_had_applied_before_the_tick_that_takes_the_hello() {
    let mut world = World::stripes();
    let (mut runner, mut e, one, _, since) = two_players_and_an_entry(&mut world);
    let had = e.sent;
    let late = input(player(1), one, 2, move_to(12.5));
    let number = e.next(late.clone());
    let mut again = greeted(
        &mut runner,
        greeting(E, 5).since(since).seen(1).chunks(vec![HOME]),
    );
    let log = &again.log;
    assert!(
        matches!(
            welcomed(log),
            Welcome::Resumed { entries: 0, presences: 2, applied } if applied == had
        ),
        "{}",
        brief(log)
    );
    let first = applied(log).expect("a progress comes with the tick of every hello");
    assert!(first == had || first == number, "{}", brief(log));

    // The edge has kept it and sends it again: it is applied, once.
    again.numbered(number, late);
    wait_applied(&mut runner, &mut [&mut again], 0, number);
    let state = runner.region().state();
    assert_eq!(state.players[&player(1)].last_input, 2);
    assert_eq!(state.players[&player(1)].pose.position.x, 12.5);
    assert_eq!(state.edges[&E].applied, number);
}

/// R30 and rule 36: a leave ends the stay it names, and that stay only.
#[test]
fn a_leave_that_names_the_entity_of_a_presence_answer_ends_that_stay_and_another_does_not() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_leave_that_names_the_entity_of_a_presence_answer_ends_that_stay_and_another_does_not_in,
    );
}

fn a_leave_that_names_the_entity_of_a_presence_answer_ends_that_stay_and_another_does_not_in(
    mut world: World,
) {
    let (mut runner, e, _, _, since) = two_players_and_an_entry(&mut world);
    let mut again = greeted(
        &mut runner,
        greeting(E, 5).since(since).seen(1).chunks(vec![HOME]),
    )
    .after(e.sent);
    let shown: BTreeMap<PlayerId, EntityId> = presences(&again.log)
        .into_iter()
        .filter_map(|(player, entity)| Some((player, entity?)))
        .collect();
    let (one, two) = (shown[&player(1)], shown[&player(2)]);

    // A leave for another stay of player 1 than the one the region has: one that
    // came late, or one for a stay the region never had.
    again.next(leave(player(1), Some(two)));
    let stale = again.next(leave(player(1), Some(STRANGER)));
    wait_applied(&mut runner, &mut [&mut again], 0, stale);
    sync(&mut runner, &mut [&mut again], 0);
    assert_eq!(runner.region().player_count(), 2);
    assert!(removal(&again.log, one).is_none(), "{}", brief(&again.log));

    // A leave through another edge is about another connection, whatever it names.
    let mut f = greeted(&mut runner, greeting(F, 5));
    let foreign = f.next(leave(player(1), Some(one)));
    wait_applied(&mut runner, &mut [&mut f, &mut again], 0, foreign);
    assert_eq!(runner.region().player_count(), 2);

    // The leave that names the stay ends it, and the other player stays.
    let right = again.next(leave(player(1), Some(one)));
    wait_applied(&mut runner, &mut [&mut again, &mut f], 0, right);
    sync(&mut runner, &mut [&mut again, &mut f], 0);
    assert!(runner.region().player(player(1)).is_none());
    assert!(removal(&again.log, one).is_some(), "{}", brief(&again.log));
    assert_eq!(
        runner.region().player(player(2)).map(|(entity, _)| entity),
        Some(two)
    );

    // One that names no entity is for the player whatever their entity.
    let unnamed = again.next(leave(player(2), None));
    wait_applied(&mut runner, &mut [&mut again, &mut f], 0, unnamed);
    assert_eq!(runner.region().player_count(), 0);

    // The next hello has nobody to say.
    let last = greeted(&mut runner, greeting(E, 5).since(since).seen(1));
    assert!(
        matches!(
            welcomed(&last.log),
            Welcome::Resumed { presences: 0, applied, .. } if applied == again.sent
        ),
        "{}",
        brief(&last.log)
    );
}

// ---------------------------------------------------------------------------------------
// R31 to R38, sections 3.1 to 3.4 and rules 41 to 44. A merge, seen from links
// ---------------------------------------------------------------------------------------

/// The areas the stripes of a world are pinned to, in the order of their regions.
fn areas(shape: Shape) -> Vec<ChunkArea> {
    shape
        .layout()
        .expect("a world of stripes")
        .regions()
        .map(|(_, area)| area)
        .collect()
}

/// On stripes: region 0 with player 1, and region 1 with player 2, who walked over
/// from region 0, both of edge E, which has a link to each. Each region has one entry
/// for E that E has not confirmed: region 0 the departure of player 2, and region 1
/// what is left of a dig of player 2 into the home chunk.
struct Pair {
    world: World,
    a: RegionRunner,
    b: RegionRunner,
    to_a: Link,
    to_b: Link,
    one: EntityId,
    two: EntityId,
}

fn pair(world: World) -> Pair {
    pair_with(world, |runner| runner)
}

/// What a test does to a runner before it is stepped: one of its settings.
type Tweak = fn(RegionRunner) -> RegionRunner;

/// A [`Pair`] whose runners are set up as `tweak` has it.
fn pair_with(mut world: World, tweak: Tweak) -> Pair {
    assert_eq!(world.shape, Shape::Stripes);
    // Region 0 is opened first: the store issues the blocks of entity ids in the
    // order regions are first opened, and only the home region is joined.
    let mut a = tweak(world.open(A));
    let mut b = tweak(world.open(B));
    let mut to_a = greeted(&mut a, greeting(E, 5).chunks(vec![HOME, NEXT]));
    assert_eq!(to_a.answers(NEXT), vec![(0, Answer::Elsewhere(B))]);
    let one = join_and_wait(&mut a, &mut to_a, player(1));
    let two = join_and_wait(&mut a, &mut to_a, player(2));
    to_a.next(input(player(2), two, 1, move_to(16.5)));
    run_until(
        &mut a,
        &mut [&mut to_a],
        "player 2 being let go",
        |_, links| departure(&links[0].log, player(2)).is_some(),
    );
    let (_, number, to, transfer) = departure(&to_a.log, player(2)).expect("waited for it");
    assert_eq!((number, to), (1, B));
    let transfer = transfer.clone();

    let mut to_b = greeted(&mut b, greeting(E, 5).chunks(vec![NEXT, HOME]));
    assert_eq!(to_b.answers(HOME), vec![(0, Answer::Elsewhere(A))]);
    to_b.next(arrive(player(2), &transfer));
    to_b.next(input(player(2), two, 2, dig(BEHIND, 1)));
    let walked = to_b.next(input(player(2), two, 3, move_to(17.5)));
    wait_applied(&mut b, &mut [&mut to_b], 0, walked);
    sync(&mut a, &mut [&mut to_a], 0);
    sync(&mut b, &mut [&mut to_b], 0);
    assert!(
        matches!(
            outbox(&to_b.log).as_slice(),
            [(_, 1, Durable::Remote { action, to: Some(A) })]
                if *action == breaking(player(2), 1, BEHIND)
        ),
        "{}",
        brief(&to_b.log)
    );
    assert_eq!(a.region().player_count(), 1);
    assert_eq!(
        b.region().player(player(2)).map(|(entity, _)| entity),
        Some(two)
    );
    Pair {
        world,
        a,
        b,
        to_a,
        to_b,
        one,
        two,
    }
}

impl Pair {
    /// Plays the worker of region 1 through its release and the worker of region 0 up
    /// to where it tells the runner to absorb: region 1 is opened with a new epoch and
    /// its state read.
    fn release_b(&mut self) -> ToAbsorb {
        self.b.begin_release();
        assert_eq!(
            run_to_the_end(&mut self.b, &mut [&mut self.to_b]),
            Ended::Released
        );
        self.world.open_to_absorb(B)
    }

    /// Has region 0 absorb region 1, and returns the tick of the merge.
    fn merge(&mut self) -> u64 {
        let absorbed = self.release_b();
        let outcome = ask(&mut self.a, absorbed.order());
        let reshaped = run_to_outcome(&mut self.a, &mut [&mut self.to_a], &outcome);
        assert_eq!(reshaped, Reshaped::Absorbed { absorbed: B });
        self.to_a.drain();
        assert!(self.to_a.closed);
        // The worker drops the handle of the absorbed region when it has the outcome.
        drop(absorbed);
        self.a.region().tick_number()
    }
}

/// What edge E says to region 0 of a [`Pair`] on a link made after a merge it has not
/// heard of: only what it had at region 0 (rule 43), `to_a` being the link it had.
fn hello_to_a(to_a: &Link) -> Greeting {
    greeting(E, 5)
        .since(since(&to_a.log))
        .seen(1)
        .players(vec![player(1)])
        .chunks(vec![HOME, NEXT])
}

/// What region 0 of a [`Pair`] is after absorbing region 1, by section 2.3: its own
/// state `ours` as of its last tick, with the players of `theirs`, and for edge E the
/// entry of the merge and behind it the other's entry under the next numbers.
fn merged_of(ours: &RegionState, theirs: &RegionState) -> RegionState {
    let mut merged = ours.clone();
    merged.tick = ours.tick + 1;
    for (id, player) in &theirs.players {
        merged.players.insert(*id, player.clone());
    }
    let other = &theirs.edges[&E];
    let edge = merged.edges.get_mut(&E).expect("region 0 knows the edge");
    edge.outbox.insert(
        edge.sent + 1,
        Durable::Absorbed {
            region: B,
            since: other.since,
            applied: other.applied,
            numbers: other.outbox.keys().copied().collect(),
        },
    );
    for (behind, entry) in other.outbox.values().enumerate() {
        edge.outbox
            .insert(edge.sent + 2 + behind as u64, entry.clone());
    }
    edge.sent += 1 + other.outbox.len() as u64;
    merged
}

#[test]
fn a_merge_closes_the_survivors_link_and_the_next_hello_is_told_of_it_and_of_who_is_there() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_merge_closes_the_survivors_link_and_the_next_hello_is_told_of_it_and_of_who_is_there_in,
    );
}

/// R31, but for the crash, which is the next test's.
fn a_merge_closes_the_survivors_link_and_the_next_hello_is_told_of_it_and_of_who_is_there_in(
    world: World,
) {
    let mut pair = pair(world);
    let absorbed = pair.release_b();
    let before = pair.world.list();
    assert_eq!(
        before
            .regions
            .iter()
            .map(|info| info.region)
            .collect::<Vec<_>>(),
        vec![A, B]
    );
    let theirs = absorbed.state.clone();
    assert_eq!(theirs.edges[&E].since, since(&pair.to_b.log));
    assert_eq!(theirs.edges[&E].applied, pair.to_b.sent);
    assert_eq!(theirs.edges[&E].outbox.len(), 1);

    let outcome = ask(&mut pair.a, absorbed.order());
    run_to_stage(&mut pair.a, &mut [&mut pair.to_a], Stage::Settling);
    let ours = pair.a.region().state();
    assert_eq!(ours.edges[&E].applied, pair.to_a.sent);
    assert_eq!(ours.edges[&E].sent, 1);
    let told = pair.to_a.log.len();

    // Every link is closed by the step that takes the merge, and by none before it:
    // not before the store has the record.
    let mut reshaped = None;
    for _ in 0..STEPS {
        pair.a.step();
        pair.to_a.drain();
        reshaped = outcome.taken();
        assert_eq!(
            pair.to_a.closed,
            reshaped.is_some(),
            "the link and the outcome disagree at {:?}",
            pair.a.stage()
        );
        if reshaped.is_some() {
            break;
        }
        // Once the commit is sent, the store has the record before the runner has
        // the answer; until then the world is as it was.
        if pair.a.stage() != Some(Stage::Committing) {
            assert_eq!(pair.world.list(), before, "at {:?}", pair.a.stage());
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(reshaped, Some(Reshaped::Absorbed { absorbed: B }));
    assert_eq!(pair.a.stage(), None);
    assert_eq!(pair.a.ended(), None);
    assert_eq!(
        pair.to_a.log.len(),
        told,
        "nothing is published for the tick of a merge, and no tick ran before it: {}",
        brief(&pair.to_a.log)
    );

    // The store's list has the region absorbed, and its area with the survivor.
    let list = pair.world.list();
    assert_eq!(list.home, A);
    assert_eq!(list.absorbed, vec![(B, A)]);
    assert_eq!(list.next, before.next);
    assert_eq!(list.regions.len(), 1);
    assert_eq!(list.regions[0].region, A);
    assert_eq!(list.regions[0].pinned, areas(Shape::Stripes));
    assert_eq!(list.regions[0].bounds, None);

    // The region is what section 2.3 says, as of the tick after its last, and what a
    // restored region is: no chunk loaded, nothing believed.
    let merged = merged_of(&ours, &theirs);
    assert_eq!(pair.a.region().state(), merged);
    assert_eq!(pair.a.region().tick_number(), ours.tick + 1);
    assert_eq!(pair.a.region().loaded_chunk_count(), 0);
    assert_eq!(pair.a.region().knowledge(NEXT), Knowledge::Unknown);
    assert!(pair.a.region().pins(NEXT) && pair.a.region().pins(OTHER));
    drop(absorbed);

    // A new link whose hello names only what the edge had at region 0.
    let again = greeted(&mut pair.a, hello_to_a(&pair.to_a));
    let log = &again.log;
    assert_eq!(
        welcomed(log),
        Welcome::Resumed {
            entries: 2,
            presences: 2,
            applied: ours.edges[&E].applied,
        },
        "{}",
        brief(log)
    );
    assert_eq!(
        entries(log),
        vec![
            (
                2,
                Durable::Absorbed {
                    region: B,
                    since: theirs.edges[&E].since,
                    applied: theirs.edges[&E].applied,
                    numbers: vec![1],
                }
            ),
            (
                3,
                Durable::Remote {
                    action: breaking(player(2), 1, BEHIND),
                    to: Some(A),
                }
            ),
        ]
    );
    assert_eq!(
        presences(log),
        vec![(player(1), Some(pair.one)), (player(2), Some(pair.two))]
    );
    let theirs = &theirs.players[&player(2)];
    assert_eq!(
        presence(log, player(2)),
        Some(&Presence::Present {
            entity: pair.two,
            pose: theirs.pose,
            hotbar: theirs.hotbar,
            selected_slot: theirs.selected_slot,
            last_input: 3,
            handled: theirs.handled,
        })
    );
    // The chunk that was told elsewhere is the region's own now, with the player in it.
    assert_eq!(again.answers(HOME), vec![(0, Answer::Snapshot)]);
    assert_eq!(again.answers(NEXT), vec![(0, Answer::Snapshot)]);
    let (_, _, entities) = snapshot(log, NEXT, 0).expect("asserted above");
    assert!(entities.iter().any(|state| state.entity == pair.two));
    assert_eq!(outcome.taken(), None, "the outcome is said once");
}

#[test]
fn a_crash_right_after_a_merge_restores_the_merged_state_with_no_commit_to_apply() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_crash_right_after_a_merge_restores_the_merged_state_with_no_commit_to_apply_in,
    );
}

/// R31, the crash; and section 6: to an edge, a survivor whose worker died after the
/// record and one whose worker lived look the same.
fn a_crash_right_after_a_merge_restores_the_merged_state_with_no_commit_to_apply_in(world: World) {
    let mut pair = pair(world);
    let m = pair.merge();
    let merged = pair.a.region().state();

    let (handle, restored) = pair.world.open_raw(A);
    assert_eq!(restored.tick(), m);
    assert_eq!(restored.state.as_ref().map(|state| state.tick), Some(m));
    assert!(restored.deltas.is_empty());
    assert_eq!(restored.pinned, areas(Shape::Stripes));
    assert!(restored.held.is_empty());
    let mut next = RegionRunner::restore(config(RETURN_AFTER), handle, restored)
        .expect("what the store has is readable");
    assert_eq!(next.region().state(), merged);
    assert_eq!(merged.players.len(), 2);

    let again = greeted(&mut next, hello_to_a(&pair.to_a));
    assert!(
        matches!(
            welcomed(&again.log),
            Welcome::Resumed {
                entries: 2,
                presences: 2,
                ..
            }
        ),
        "{}",
        brief(&again.log)
    );
    assert!(matches!(
        entries(&again.log).as_slice(),
        [
            (2, Durable::Absorbed { region: B, .. }),
            (3, Durable::Remote { .. })
        ]
    ));
    assert_eq!(
        presences(&again.log),
        vec![(player(1), Some(pair.one)), (player(2), Some(pair.two))]
    );

    // The absorbed region is none any more.
    let epoch = pair.world.next_epoch();
    assert!(matches!(
        pair.world.open_with(B, epoch),
        Err(StoreError::Absorbed { region: B, into: A })
    ));
}

#[test]
fn what_the_survivors_link_sent_after_the_stop_is_not_applied_by_the_merge_and_is_when_sent_again()
{
    in_memory_and_on_disk(
        Shape::Stripes,
        what_the_survivors_link_sent_after_the_stop_is_not_applied_by_the_merge_and_is_when_sent_again_in,
    );
}

/// R32 and rule 48, second item.
fn what_the_survivors_link_sent_after_the_stop_is_not_applied_by_the_merge_and_is_when_sent_again_in(
    world: World,
) {
    let mut pair = pair(world);
    let absorbed = pair.release_b();
    let outcome = ask(&mut pair.a, absorbed.order());
    run_to_stage(&mut pair.a, &mut [&mut pair.to_a], Stage::Settling);
    let had = pair.to_a.sent;
    let first = input(player(1), pair.one, 1, move_to(12.5));
    let second = input(player(1), pair.one, 2, move_to(11.5));
    pair.to_a.next(first.clone());
    // One more at each later stage, so that none of them takes from a link.
    run_to_stage(&mut pair.a, &mut [&mut pair.to_a], Stage::Closing);
    pair.to_a.next(second.clone());
    let reshaped = run_to_outcome(&mut pair.a, &mut [&mut pair.to_a], &outcome);
    assert_eq!(reshaped, Reshaped::Absorbed { absorbed: B });
    drop(absorbed);
    assert_eq!(applied(&pair.to_a.log), Some(had));
    let state = pair.a.region().state();
    assert_eq!(state.edges[&E].applied, had);
    assert_eq!(state.players[&player(1)].last_input, 0);

    let mut again = greeted(&mut pair.a, hello_to_a(&pair.to_a)).after(had);
    let log = &again.log;
    assert!(
        matches!(welcomed(log), Welcome::Resumed { applied, .. } if applied == had),
        "{}",
        brief(log)
    );
    assert_eq!(applied(log), Some(had), "{}", brief(log));

    // The edge has kept them and sends them again.
    again.next(first);
    let last = again.next(second);
    wait_applied(&mut pair.a, &mut [&mut again], 0, last);
    sync(&mut pair.a, &mut [&mut again], 0);
    let log = &again.log;
    let steps: Vec<f64> = events(log)
        .into_iter()
        .filter_map(|(_, _, event)| match event {
            RegionEvent::EntityMoved { entity, pose, .. } if *entity == pair.one => {
                Some(pose.position.x)
            }
            _ => None,
        })
        .collect();
    assert!(
        steps == [12.5, 11.5] || steps == [11.5],
        "applied {steps:?}: {}",
        brief(log)
    );
    let state = pair.a.region().state();
    assert_eq!(state.players[&player(1)].last_input, 2);
    assert_eq!(state.players[&player(1)].pose.position.x, 11.5);
}

#[test]
fn after_a_merge_a_dig_behind_a_subscribe_for_a_chunk_of_the_area_that_came_with_it_waits_for_the_snapshot()
 {
    in_memory_and_on_disk(
        Shape::Stripes,
        after_a_merge_a_dig_behind_a_subscribe_for_a_chunk_of_the_area_that_came_with_it_waits_for_the_snapshot_in,
    );
}

/// R33 on stripes, and rule 42, steps 4 and 5: the edge asks the survivor for what it
/// had at the absorbed region and, behind that on the link it has, sends on what that
/// region's players did meanwhile, under the survivor's numbers.
fn after_a_merge_a_dig_behind_a_subscribe_for_a_chunk_of_the_area_that_came_with_it_waits_for_the_snapshot_in(
    world: World,
) {
    let mut pair = pair(world);
    pair.merge();
    // The hello names nothing of region 1, so nothing of this is behind its hold.
    let mut again =
        greeted(&mut pair.a, hello_to_a(&pair.to_a).chunks(vec![HOME])).after(pair.to_a.sent);
    assert_eq!(pair.a.region().chunk(NEXT), None);

    let ask = again.subscribe(vec![NEXT]);
    let dug = again.next(input(player(2), pair.two, 4, dig(BEYOND, 2)));
    let walked = again.next(input(player(2), pair.two, 5, move_to(18.5)));
    wait_applied(&mut pair.a, &mut [&mut again], 0, walked);
    sync(&mut pair.a, &mut [&mut again], 0);

    let log = &again.log;
    let (shot, chunk, entities) = snapshot(log, NEXT, ask).expect("the asking is answered");
    assert_eq!(block_in(chunk, BEYOND), blocks::GRASS_BLOCK);
    assert!(
        entities
            .iter()
            .any(|state| state.entity == pair.two && state.pose.position.x == 17.5),
        "{}",
        brief(log)
    );
    let (broke, _, state) = block_change(log, BEYOND).expect("the block is broken");
    assert_eq!(state, blocks::AIR);
    let (stepped, _) = moved(log, pair.two, 18.5).expect("the step is applied");
    assert!(shot < broke && broke <= stepped, "{}", brief(log));
    assert!(progress_to(log, dug).expect("waited for it") > shot);
    assert!(acknowledged(log, player(2), 2).is_some(), "{}", brief(log));
    assert_eq!(block_of(&pair.a, BEYOND), Some(blocks::AIR));
    // Nothing of it was passed on: the entries are those of the welcome.
    assert_eq!(outbox(log).len(), 2, "{}", brief(log));
}

#[test]
fn with_the_gap_a_dig_behind_a_subscribe_for_a_chunk_the_absorbed_region_was_granted_waits_for_the_snapshot()
 {
    in_memory_and_on_disk(
        Shape::Gap,
        with_the_gap_a_dig_behind_a_subscribe_for_a_chunk_the_absorbed_region_was_granted_waits_for_the_snapshot_in,
    );
}

/// R33 with the gap: the chunk comes to the survivor as a grant that moved, named in
/// the store's answer, and is held from the tick of the merge.
fn with_the_gap_a_dig_behind_a_subscribe_for_a_chunk_the_absorbed_region_was_granted_waits_for_the_snapshot_in(
    mut world: World,
) {
    let mut home = world.open(GAP_HOME);
    let mut east = world.open(GAP_EAST);
    // The eastern region is granted the free chunk beside the home chunk for a viewer.
    let mut to_home = greeted(&mut home, greeting(E, 5).chunks(vec![HOME]));
    let mut to_east = greeted(&mut east, greeting(E, 5).chunks(vec![NEXT]));
    assert_eq!(to_east.answers(NEXT), vec![(0, Answer::Snapshot)]);
    assert_eq!(east.region().knowledge(NEXT), Knowledge::Held);
    let entity = join_and_wait(&mut home, &mut to_home, player(1));
    sync(&mut home, &mut [&mut to_home], 0);
    let home_since = since(&to_home.log);

    east.begin_release();
    assert_eq!(
        run_to_the_end(&mut east, &mut [&mut to_east]),
        Ended::Released
    );
    let absorbed = world.open_to_absorb(GAP_EAST);
    let outcome = ask(&mut home, absorbed.order());
    let reshaped = run_to_outcome(&mut home, &mut [&mut to_home], &outcome);
    assert_eq!(reshaped, Reshaped::Absorbed { absorbed: GAP_EAST });
    drop(absorbed);
    assert_eq!(home.region().knowledge(NEXT), Knowledge::Held);
    assert_eq!(home.region().chunk(NEXT), None);
    assert!(home.region().pins(ChunkPos::new(20, 0)));
    assert!(!home.region().pins(NEXT));
    let list = world.list();
    assert_eq!(list.absorbed, vec![(GAP_EAST, GAP_HOME)]);
    let info = list
        .regions
        .iter()
        .find(|info| info.region == GAP_HOME)
        .expect("the home region lives");
    assert_eq!(
        info.bounds.map(|bounds| (bounds.min, bounds.max)),
        Some((HOME, NEXT))
    );
    assert_eq!(info.pinned.len(), 1);

    let mut again = greeted(
        &mut home,
        greeting(E, 5)
            .since(home_since)
            .players(vec![player(1)])
            .chunks(vec![HOME]),
    )
    .after(to_home.sent);
    let ask = again.subscribe(vec![NEXT]);
    let dug = again.next(input(player(1), entity, 1, dig(BEYOND, 1)));
    wait_applied(&mut home, &mut [&mut again], 0, dug);
    sync(&mut home, &mut [&mut again], 0);
    let log = &again.log;
    let (shot, chunk, _) = snapshot(log, NEXT, ask).expect("the asking is answered");
    assert_eq!(block_in(chunk, BEYOND), blocks::GRASS_BLOCK);
    let (broke, _, _) = block_change(log, BEYOND).expect("the block is broken");
    assert!(shot < broke, "{}", brief(log));
    assert_eq!(block_of(&home, BEYOND), Some(blocks::AIR));
    assert_eq!(
        outbox(log).len(),
        1,
        "only the entry of the merge: {}",
        brief(log)
    );
}

/// R34.
#[test]
fn a_guests_subscription_to_a_chunk_of_the_absorbed_regions_area_is_served_after_the_merge() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_guests_subscription_to_a_chunk_of_the_absorbed_regions_area_is_served_after_the_merge_in,
    );
}

fn a_guests_subscription_to_a_chunk_of_the_absorbed_regions_area_is_served_after_the_merge_in(
    world: World,
) {
    let mut pair = pair(world);
    let refused = pair.to_a.subscribe_as_guest(vec![OTHER]);
    assert_eq!(
        wait_answer(&mut pair.a, &mut [&mut pair.to_a], 0, OTHER, refused),
        Answer::NotMine
    );
    pair.merge();
    assert_eq!(pair.a.region().knowledge(OTHER), Knowledge::Unknown);

    let mut again = greeted(&mut pair.a, hello_to_a(&pair.to_a).guests(vec![OTHER]));
    assert_eq!(again.answers(OTHER), vec![(0, Answer::Snapshot)]);
    let beside = ChunkPos::new(OTHER.x + 1, OTHER.z);
    let ask = again.subscribe_as_guest(vec![beside]);
    assert_eq!(
        wait_answer(&mut pair.a, &mut [&mut again], 0, beside, ask),
        Answer::Snapshot
    );
    assert_eq!(pair.a.region().knowledge(OTHER), Knowledge::Held);
}

/// R35: the survivor forgets every belief at the merge, also one about a region
/// that had nothing to do with it, and asks again.
#[test]
fn with_three_stripes_what_the_survivor_believed_of_the_third_region_is_forgotten_and_asked_again()
{
    in_memory_and_on_disk(
        Shape::Three,
        with_three_stripes_what_the_survivor_believed_of_the_third_region_is_forgotten_and_asked_again_in,
    );
}

fn with_three_stripes_what_the_survivor_believed_of_the_third_region_is_forgotten_and_asked_again_in(
    mut world: World,
) {
    let third = BACK;
    let theirs = ChunkPos::new(5, 0);
    let mut home = world.open(THREE_HOME);
    let mut east = world.open(THREE_EAST);
    let mut to_home = greeted(&mut home, greeting(E, 5).chunks(vec![HOME, third, theirs]));
    assert_eq!(
        to_home.answers(third),
        vec![(0, Answer::Elsewhere(THREE_WEST))]
    );
    assert_eq!(
        to_home.answers(theirs),
        vec![(0, Answer::Elsewhere(THREE_EAST))]
    );
    join_and_wait(&mut home, &mut to_home, player(1));
    sync(&mut home, &mut [&mut to_home], 0);
    assert_eq!(
        home.region().knowledge(third),
        Knowledge::Foreign(THREE_WEST)
    );

    east.begin_release();
    assert_eq!(run_to_the_end(&mut east, &mut []), Ended::Released);
    let absorbed = world.open_to_absorb(THREE_EAST);
    let outcome = ask(&mut home, absorbed.order());
    let reshaped = run_to_outcome(&mut home, &mut [&mut to_home], &outcome);
    assert_eq!(
        reshaped,
        Reshaped::Absorbed {
            absorbed: THREE_EAST
        }
    );
    drop(absorbed);
    assert_eq!(home.region().knowledge(third), Knowledge::Unknown);
    assert_eq!(home.region().knowledge(theirs), Knowledge::Unknown);
    assert!(!home.region().pins(third));
    assert!(home.region().pins(theirs));

    let again = greeted(
        &mut home,
        greeting(E, 5)
            .since(since(&to_home.log))
            .players(vec![player(1)])
            .chunks(vec![HOME, third, theirs]),
    );
    assert_eq!(
        again.answers(third),
        vec![(0, Answer::Elsewhere(THREE_WEST))]
    );
    assert_eq!(again.answers(theirs), vec![(0, Answer::Snapshot)]);
    assert_eq!(
        home.region().knowledge(third),
        Knowledge::Foreign(THREE_WEST)
    );
    let list = world.list();
    assert_eq!(list.absorbed, vec![(THREE_EAST, THREE_HOME)]);
    assert_eq!(
        list.regions
            .iter()
            .map(|info| info.region)
            .collect::<Vec<_>>(),
        vec![THREE_WEST, THREE_HOME]
    );
}

/// Asks `runner` for a merge or a split of which nothing comes, and holds it to
/// section 3.4: the outcome is said by the call, once; nothing was said to anyone, no
/// link was closed and no tick number was used; the store's list is as it was; and the
/// next tick takes what the link sent while the region stood still, which is a step
/// of `walker`, a player of the link's, numbered `step` among their inputs.
fn comes_to_nothing(
    world: &World,
    runner: &mut RegionRunner,
    link: &mut Link,
    walker: (PlayerId, EntityId),
    order: Reshape,
    step: u64,
) -> Off {
    let list = world.list();
    let outcome = ask(runner, order);
    run_to_stage(runner, &mut [link], Stage::Settling);
    let stopped = runner.region().tick_number();
    let state = runner.region().state();
    let from = state.players[&walker.0].pose.position.x;
    let walked = link.next(input(walker.0, walker.1, step, move_to(from - 1.0)));
    let told = link.log.len();

    let reshaped = run_to_outcome(runner, &mut [link], &outcome);
    let Reshaped::Off { why } = reshaped else {
        panic!("something came of it: {reshaped:?}");
    };
    assert!(!link.closed);
    assert_eq!(link.log.len(), told, "{}", brief(&link.log));
    assert_eq!(runner.region().state(), state);
    assert_eq!(runner.region().tick_number(), stopped);
    assert_eq!(runner.ended(), None);
    assert_eq!(world.list(), list);

    wait_applied(runner, &mut [link], 0, walked);
    assert_eq!(
        moved(&link.log, walker.1, from - 1.0).map(|(_, tick)| tick),
        Some(stopped + 1),
        "{}",
        brief(&link.log)
    );
    assert_eq!(outcome.taken(), None, "the outcome is said once");
    why
}

/// [`comes_to_nothing`] for region 0 of a [`Pair`], with player 1 walking.
fn nothing_comes_of(pair: &mut Pair, order: Reshape, step: u64) -> Off {
    comes_to_nothing(
        &pair.world,
        &mut pair.a,
        &mut pair.to_a,
        (player(1), pair.one),
        order,
        step,
    )
}

#[test]
fn a_merge_that_the_store_declines_leaves_the_region_ticking_with_its_link_as_if_nothing_had_been_asked()
 {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_merge_that_the_store_declines_leaves_the_region_ticking_with_its_link_as_if_nothing_had_been_asked_in,
    );
}

/// R36: another owner is given region 1 before the commit.
fn a_merge_that_the_store_declines_leaves_the_region_ticking_with_its_link_as_if_nothing_had_been_asked_in(
    world: World,
) {
    let mut pair = pair(world);
    let absorbed = pair.release_b();
    let order = absorbed.order();
    let (taken, restored) = pair.world.open_raw(B);
    let epoch = pair.world.epoch;
    assert_eq!(
        nothing_comes_of(&mut pair, order, 1),
        Off::Declined(Decline::NotOpened { epoch: Some(epoch) })
    );
    // Region 1 is its new owner's, with its player.
    let other = RegionRunner::restore(config(RETURN_AFTER), taken, restored)
        .expect("what the store has is readable");
    assert_eq!(
        other.region().player(player(2)).map(|(entity, _)| entity),
        Some(pair.two)
    );
    assert_eq!(pair.a.region().player_count(), 1);
    assert!(
        !pair.a.region().state().edges[&E]
            .outbox
            .values()
            .any(|entry| matches!(entry, Durable::Absorbed { .. }))
    );
}

/// Section 3.4 and ADR-0011, section 3.6: each reason the store declines a merge with
/// that a runner can be led to from outside. (`Tick` it cannot: the runner names the
/// tick. `TooLarge` of the store is behind the runner's own bound, which is next.)
#[test]
fn each_reason_the_store_declines_a_merge_with_is_the_outcome_and_the_region_ticks_on() {
    // The absorbed region has no owner: its handle was dropped before the commit.
    let mut pair = pair(World::stripes());
    let absorbed = pair.release_b();
    let order = absorbed.order();
    drop(absorbed);
    assert_eq!(
        nothing_comes_of(&mut pair, order, 1),
        Off::Declined(Decline::NotOpened { epoch: None })
    );

    // It has an owner with another epoch than the order names; and with the right
    // one the merge is done after all.
    let absorbed = pair.world.open_to_absorb(B);
    let wrong = Reshape::Absorb {
        absorbed: B,
        absorbed_epoch: absorbed.epoch + 1,
        state: absorbed.state.clone(),
    };
    assert_eq!(
        nothing_comes_of(&mut pair, wrong, 2),
        Off::Declined(Decline::NotOpened {
            epoch: Some(absorbed.epoch)
        })
    );

    // A region there is none of, and the region itself.
    for region in [RegionId(9), A] {
        let none = Reshape::Absorb {
            absorbed: region,
            absorbed_epoch: absorbed.epoch,
            state: absorbed.state.clone(),
        };
        let step = if region == A { 4 } else { 3 };
        assert_eq!(
            nothing_comes_of(&mut pair, none, step),
            Off::Declined(Decline::NoSuchRegion)
        );
    }

    let outcome = ask(&mut pair.a, absorbed.order());
    assert_eq!(
        run_to_outcome(&mut pair.a, &mut [&mut pair.to_a], &outcome),
        Reshaped::Absorbed { absorbed: B }
    );
    assert_eq!(pair.a.region().player_count(), 2);
    assert_eq!(pair.a.region().state().players[&player(1)].last_input, 4);
}

/// ADR-0011, section 3.6: the home region is never absorbed.
#[test]
fn the_home_region_is_not_absorbed() {
    let mut pair = pair(World::stripes());
    let list = pair.world.list();
    pair.a.begin_release();
    assert_eq!(
        run_to_the_end(&mut pair.a, &mut [&mut pair.to_a]),
        Ended::Released
    );
    let absorbed = pair.world.open_to_absorb(A);
    let outcome = ask(&mut pair.b, absorbed.order());
    let reshaped = run_to_outcome(&mut pair.b, &mut [&mut pair.to_b], &outcome);
    assert_eq!(
        reshaped,
        Reshaped::Off {
            why: Off::Declined(Decline::Home)
        }
    );
    assert!(!pair.to_b.closed);
    sync(&mut pair.b, &mut [&mut pair.to_b], 0);
    assert_eq!(pair.world.list().absorbed, list.absorbed);
    assert_eq!(pair.world.list().regions.len(), 2);
}

/// Section 4, step 3, and ADR-0011, section 3.6: a region whose owner died instead of
/// releasing it has commits that no checkpoint covers, and the store declines the
/// merge until [`absorbable`] has checkpointed it.
#[test]
fn a_region_whose_owner_died_is_absorbed_once_its_state_has_been_checkpointed() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_region_whose_owner_died_is_absorbed_once_its_state_has_been_checkpointed_in,
    );
}

fn a_region_whose_owner_died_is_absorbed_once_its_state_has_been_checkpointed_in(world: World) {
    let mut pair = pair(world);
    // Everything region 1 did is confirmed and published; its owner dies.
    let state = pair.b.region().state();
    let epoch = pair.world.next_epoch();
    let (handle, restored) = pair
        .world
        .open_with(B, epoch)
        .expect("a higher epoch opens the region");
    assert!(!restored.deltas.is_empty());
    let unprepared = Reshape::Absorb {
        absorbed: B,
        absorbed_epoch: epoch,
        state: state.clone(),
    };
    assert_eq!(
        nothing_comes_of(&mut pair, unprepared, 1),
        Off::Declined(Decline::Uncheckpointed { region: B })
    );

    // A tick that changed nothing has no commit, so the store's state can be of an
    // earlier tick than the runner's was.
    let read = absorbable(&handle, restored).expect("what the store has is readable");
    assert_eq!((&read.players, &read.edges), (&state.players, &state.edges));
    let outcome = ask(
        &mut pair.a,
        Reshape::Absorb {
            absorbed: B,
            absorbed_epoch: epoch,
            state: read,
        },
    );
    assert_eq!(
        run_to_outcome(&mut pair.a, &mut [&mut pair.to_a], &outcome),
        Reshaped::Absorbed { absorbed: B }
    );
    drop(handle);
    assert_eq!(pair.world.list().absorbed, vec![(B, A)]);
    assert_eq!(
        pair.a.region().state().players[&player(2)],
        state.players[&player(2)]
    );
}

/// Section 3.2: a merged state that is longer than a request may be is not sent.
#[test]
fn a_merge_too_large_to_hand_to_the_store_is_off_and_the_region_ticks_on() {
    let mut pair = pair(World::stripes());
    let absorbed = pair.release_b();
    let mut state = absorbed.state.clone();
    state
        .players
        .get_mut(&player(2))
        .expect("region 1 has the player")
        .name = "x".repeat(clustine_worker::MAX_RESHAPE_BYTES);
    let huge = Reshape::Absorb {
        absorbed: B,
        absorbed_epoch: absorbed.epoch,
        state,
    };
    assert_eq!(nothing_comes_of(&mut pair, huge, 1), Off::TooLarge);
    // The handle was not lost over it, and the merge as it is goes through.
    let outcome = ask(&mut pair.a, absorbed.order());
    assert_eq!(
        run_to_outcome(&mut pair.a, &mut [&mut pair.to_a], &outcome),
        Reshaped::Absorbed { absorbed: B }
    );
}

/// R37, rule 41 and section 3.7: an entry is written for every edge either region
/// knew. An edge only the absorbed region knew is unknown to the survivor's numbering
/// and is told so, with the entries from number 1 and its players, at every hello
/// until it says the `since` it was told. An edge only the survivor knew gets the
/// entry with nothing behind it.
#[test]
fn an_edge_that_only_one_of_the_two_regions_knew_is_told_of_the_merge_and_of_its_players() {
    in_memory_and_on_disk(
        Shape::Stripes,
        an_edge_that_only_one_of_the_two_regions_knew_is_told_of_the_merge_and_of_its_players_in,
    );
}

fn an_edge_that_only_one_of_the_two_regions_knew_is_told_of_the_merge_and_of_its_players_in(
    world: World,
) {
    let mut pair = pair(world);
    // Edge F knows region 0 only, and has a player there.
    let mut f = greeted(&mut pair.a, greeting(F, 7).chunks(vec![HOME]));
    let four = join_and_wait(&mut pair.a, &mut f, player(4));
    settle(&mut pair.a, &mut [&mut pair.to_a, &mut f]);
    // Edge G knows region 1 only: a player of its came in from a region these tests
    // do not have, and dug into the home chunk.
    let mut g = greeted(&mut pair.b, greeting(G, 9).chunks(vec![NEXT]));
    g.next(arrive(player(3), &transfer(STRANGER, 16.5)));
    let dug = g.next(input(player(3), STRANGER, 1, dig(BEHIND, 7)));
    wait_applied(&mut pair.b, &mut [&mut g, &mut pair.to_b], 0, dug);
    settle(&mut pair.b, &mut [&mut pair.to_b, &mut g]);
    assert_eq!(outbox(&g.log).len(), 1, "{}", brief(&g.log));
    let (g_since, g_sent) = (since(&g.log), g.sent);

    let absorbed = pair.release_b();
    g.drain();
    assert!(g.closed);
    let outcome = ask(&mut pair.a, absorbed.order());
    let reshaped = run_to_outcome(&mut pair.a, &mut [&mut pair.to_a, &mut f], &outcome);
    assert_eq!(reshaped, Reshaped::Absorbed { absorbed: B });
    drop(absorbed);
    assert!(pair.to_a.closed && f.closed);
    let m = pair.a.region().tick_number();

    // G says hello to the survivor for the first time, twice without having read the
    // welcome, and then as an edge that has.
    let told = Welcome::Unknown {
        since: m,
        entries: 2,
        presences: 2,
        applied: 0,
    };
    for _ in 0..2 {
        let first = greeted(
            &mut pair.a,
            greeting(G, 9).players(vec![player(8)]).chunks(vec![NEXT]),
        );
        let log = &first.log;
        assert_eq!(welcomed(log), told, "{}", brief(log));
        assert!(
            matches!(
                entries(log).as_slice(),
                [
                    (1, Durable::Absorbed { region: B, since, applied, numbers }),
                    (2, Durable::Remote { action, .. }),
                ] if *since == g_since
                    && *applied == g_sent
                    && *numbers == [1]
                    && *action == breaking(player(3), 7, BEHIND)
            ),
            "{}",
            brief(log)
        );
        assert_eq!(
            presences(log),
            vec![(player(8), None), (player(3), Some(STRANGER))]
        );
    }
    let mut known = greeted(
        &mut pair.a,
        greeting(G, 9).since(m).seen(2).chunks(vec![NEXT]),
    );
    assert_eq!(
        welcomed(&known.log),
        Welcome::Resumed {
            entries: 0,
            presences: 1,
            applied: 0,
        },
        "{}",
        brief(&known.log)
    );
    // Its messages are numbered from 1 with the survivor.
    let walked = known.next(input(player(3), STRANGER, 2, move_to(17.5)));
    assert_eq!(walked, 1);
    wait_applied(&mut pair.a, &mut [&mut known], 0, walked);
    assert_eq!(
        pair.a.region().state().players[&player(3)].pose.position.x,
        17.5
    );

    // F resumes, and reads that a region it never knew has gone into this one.
    let again = greeted(
        &mut pair.a,
        greeting(F, 7).since(since(&f.log)).chunks(vec![HOME]),
    );
    let log = &again.log;
    assert_eq!(
        welcomed(log),
        Welcome::Resumed {
            entries: 1,
            presences: 1,
            applied: f.sent,
        },
        "{}",
        brief(log)
    );
    assert_eq!(
        entries(log),
        vec![(
            1,
            Durable::Absorbed {
                region: B,
                since: 0,
                applied: 0,
                numbers: Vec::new(),
            }
        )]
    );
    assert_eq!(presences(log), vec![(player(4), Some(four))]);
}

/// Section 2.3, step 2, seen from a link: the absorbed region knew the edge with a
/// lower start than the survivor. Its players of that edge do not come in and its
/// entries are not carried over; the entry is as for an edge it did not know.
#[test]
fn a_merge_drops_what_the_absorbed_region_had_for_an_earlier_start_of_the_edge() {
    let mut pair = pair(World::stripes());
    // The edge starts anew and reaches region 0 only before the merge.
    let mut newer = greeted(&mut pair.a, greeting(E, 6).chunks(vec![HOME]));
    let again = join_and_wait(&mut pair.a, &mut newer, player(1));
    sync(&mut pair.a, &mut [&mut newer], 0);
    assert!(again.0 > pair.two.0);
    pair.to_a.drain();
    assert!(pair.to_a.closed);

    let absorbed = pair.release_b();
    assert_eq!(absorbed.state.edges[&E].start, 5);
    assert!(absorbed.state.players.contains_key(&player(2)));
    let outcome = ask(&mut pair.a, absorbed.order());
    let reshaped = run_to_outcome(&mut pair.a, &mut [&mut newer], &outcome);
    assert_eq!(reshaped, Reshaped::Absorbed { absorbed: B });
    drop(absorbed);

    let state = pair.a.region().state();
    assert_eq!(
        state.players.keys().copied().collect::<Vec<_>>(),
        vec![player(1)]
    );
    assert_eq!(state.edges[&E].start, 6);
    assert_eq!(state.edges[&E].since, since(&newer.log));
    let resumed = greeted(
        &mut pair.a,
        greeting(E, 6).since(since(&newer.log)).chunks(vec![HOME]),
    );
    assert_eq!(
        welcomed(&resumed.log),
        Welcome::Resumed {
            entries: 1,
            presences: 1,
            applied: newer.sent,
        },
        "{}",
        brief(&resumed.log)
    );
    assert_eq!(
        entries(&resumed.log),
        vec![(
            1,
            Durable::Absorbed {
                region: B,
                since: 0,
                applied: 0,
                numbers: Vec::new(),
            }
        )]
    );
    assert_eq!(presences(&resumed.log), vec![(player(1), Some(again))]);
}

/// Section 2.3, step 2, the other way round: the survivor knew the edge with a lower
/// start than the absorbed region. Its own side is reset: its players of the edge are
/// gone and its entries dropped, and it knows the edge as the absorbed region did, but
/// since the tick of the merge and with nothing applied.
#[test]
fn a_merge_resets_what_the_survivor_had_for_an_earlier_start_of_the_edge() {
    let mut pair = pair(World::stripes());
    // The edge starts anew and reaches region 1 only before the merge. A player of
    // the new start comes in there and digs into the home chunk.
    let mut newer = greeted(&mut pair.b, greeting(E, 6).chunks(vec![NEXT]));
    assert_eq!(pair.b.region().player_count(), 0);
    newer.next(arrive(player(5), &transfer(STRANGER, 16.5)));
    let dug = newer.next(input(player(5), STRANGER, 1, dig(BEHIND, 3)));
    wait_applied(&mut pair.b, &mut [&mut newer], 0, dug);
    sync(&mut pair.b, &mut [&mut newer], 0);
    assert_eq!(outbox(&newer.log).len(), 1, "{}", brief(&newer.log));
    let (b_since, b_sent) = (since(&newer.log), newer.sent);
    pair.to_b.drain();
    assert!(pair.to_b.closed);

    pair.b.begin_release();
    assert_eq!(
        run_to_the_end(&mut pair.b, &mut [&mut newer]),
        Ended::Released
    );
    let absorbed = pair.world.open_to_absorb(B);
    let outcome = ask(&mut pair.a, absorbed.order());
    let reshaped = run_to_outcome(&mut pair.a, &mut [&mut pair.to_a], &outcome);
    assert_eq!(reshaped, Reshaped::Absorbed { absorbed: B });
    drop(absorbed);
    let m = pair.a.region().tick_number();

    let state = pair.a.region().state();
    assert_eq!(
        state.players.keys().copied().collect::<Vec<_>>(),
        vec![player(5)],
        "player 1 was the old start's"
    );
    let edge = &state.edges[&E];
    assert_eq!(
        (edge.start, edge.since, edge.applied, edge.sent),
        (6, m, 0, 2)
    );
    assert!(
        matches!(
            edge.outbox.values().collect::<Vec<_>>().as_slice(),
            [
                Durable::Absorbed { region: B, since, applied, numbers },
                Durable::Remote { .. },
            ] if *since == b_since && *applied == b_sent && *numbers == [1]
        ),
        "{:?}",
        edge.outbox
    );

    // The old start is told that it has been replaced, and the new one, which the
    // survivor has never welcomed, what the survivor has for it.
    let mut old = Link::attach(&pair.a);
    old.hello(hello_to_a(&pair.to_a));
    let first = greeted(&mut pair.a, greeting(E, 6).chunks(vec![HOME]));
    old.drain();
    assert_eq!(welcomed(&old.log), Welcome::Superseded);
    assert!(old.closed);
    assert_eq!(
        welcomed(&first.log),
        Welcome::Unknown {
            since: m,
            entries: 2,
            presences: 1,
            applied: 0,
        },
        "{}",
        brief(&first.log)
    );
    assert_eq!(presences(&first.log), vec![(player(5), Some(STRANGER))]);
}

#[test]
fn a_release_asked_for_during_a_merge_waits_for_it_and_the_next_owner_has_the_merged_region() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_release_asked_for_during_a_merge_waits_for_it_and_the_next_owner_has_the_merged_region_in,
    );
}

/// R38 and section 3.8. A release is asked for again before every step, as
/// `RegionRunner::run` does.
fn a_release_asked_for_during_a_merge_waits_for_it_and_the_next_owner_has_the_merged_region_in(
    world: World,
) {
    let mut pair = pair(world);
    let absorbed = pair.release_b();
    let outcome = ask(&mut pair.a, absorbed.order());
    run_to_stage(&mut pair.a, &mut [&mut pair.to_a], Stage::Settling);
    let mut reshaped = None;
    for _ in 0..STEPS {
        pair.a.begin_release();
        pair.a.step();
        pair.to_a.drain();
        reshaped = outcome.taken();
        if reshaped.is_some() {
            break;
        }
        assert_eq!(pair.a.ended(), None);
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(reshaped, Some(Reshaped::Absorbed { absorbed: B }));
    drop(absorbed);
    assert_eq!(
        pair.a.ended(),
        None,
        "the release begins when the merge has ended"
    );
    let merged = pair.a.region().state();

    let mut ended = None;
    for _ in 0..STEPS {
        pair.a.begin_release();
        pair.a.step();
        ended = pair.a.ended();
        if ended.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(ended, Some(Ended::Released));

    let (handle, restored) = pair.world.open_raw(A);
    assert!(restored.deltas.is_empty());
    let next = RegionRunner::restore(config(RETURN_AFTER), handle, restored)
        .expect("what the store has is readable");
    let state = next.region().state();
    assert_eq!(state.players, merged.players);
    assert_eq!(state.edges, merged.edges);
    assert_eq!(state.players.len(), 2);
}

/// Section 3.1: one thing at a time. A command that comes while another is under
/// way, or while the region is being released, or when the runner has ended, is
/// answered `Busy` at once; `Prepare` is never answered; and what was under way goes
/// on to its own outcome.
#[test]
fn a_second_command_is_answered_busy_at_once_and_the_first_goes_on() {
    let mut pair = pair(World::stripes());
    let busy = Some(Reshaped::Off { why: Off::Busy });
    let split = || Reshape::SplitOff {
        chunks: vec![HOME],
        as_epoch: 99,
        part: RegionId(2),
    };

    // `Prepare` of a region that only runs: no outcome, no stage, and it ticks on.
    let prepared = ask(&mut pair.a, Reshape::Prepare);
    assert_eq!(pair.a.stage(), None);
    for _ in 0..5 {
        one_tick(&mut pair.a, &mut [&mut pair.to_a]);
        assert_eq!(pair.a.stage(), None);
    }
    sync(&mut pair.a, &mut [&mut pair.to_a], 0);
    assert_eq!(prepared.taken(), None);

    let absorbed = pair.release_b();
    let first = ask(&mut pair.a, absorbed.order());
    for stage in [
        Stage::Preparing,
        Stage::Settling,
        Stage::Closing,
        Stage::Committing,
    ] {
        run_to_stage(&mut pair.a, &mut [&mut pair.to_a], stage);
        assert_eq!(ask(&mut pair.a, split()).taken(), busy, "at {stage:?}");
        assert_eq!(
            ask(&mut pair.a, absorbed.order()).taken(),
            busy,
            "at {stage:?}"
        );
        assert_eq!(ask(&mut pair.a, Reshape::Prepare).taken(), None);
        assert_eq!(pair.a.stage(), Some(stage));
        assert_eq!(first.taken(), None);
    }
    assert_eq!(
        run_to_outcome(&mut pair.a, &mut [&mut pair.to_a], &first),
        Reshaped::Absorbed { absorbed: B }
    );
    drop(absorbed);
    assert_eq!(first.taken(), None);
    assert_eq!(prepared.taken(), None);

    // A runner that releases its region, and one that has ended.
    pair.a.begin_release();
    assert_eq!(ask(&mut pair.a, split()).taken(), busy);
    assert_eq!(ask(&mut pair.a, Reshape::Prepare).taken(), None);
    assert_eq!(run_to_the_end(&mut pair.a, &mut []), Ended::Released);
    assert_eq!(ask(&mut pair.a, split()).taken(), busy);
    assert_eq!(pair.a.stage(), None);
}

/// Section 3.1, "links attached now are closed", and rule 44: a region answers no
/// hello between the stop and the tick of the merge, and lets edges in again at once
/// afterwards.
#[test]
fn a_link_attached_while_a_region_stands_still_for_a_merge_is_closed_without_an_answer() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_link_attached_while_a_region_stands_still_for_a_merge_is_closed_without_an_answer_in,
    );
}

fn a_link_attached_while_a_region_stands_still_for_a_merge_is_closed_without_an_answer_in(
    world: World,
) {
    let mut pair = pair(world);
    let absorbed = pair.release_b();
    let outcome = ask(&mut pair.a, absorbed.order());
    let mut late = Vec::new();
    for stage in [Stage::Settling, Stage::Closing, Stage::Committing] {
        run_to_stage(&mut pair.a, &mut [&mut pair.to_a], stage);
        let mut link = Link::attach(&pair.a);
        link.hello(greeting(F, 5).chunks(vec![HOME]));
        late.push(link);
    }
    let reshaped = run_to_outcome(&mut pair.a, &mut [&mut pair.to_a], &outcome);
    assert_eq!(reshaped, Reshaped::Absorbed { absorbed: B });
    drop(absorbed);
    for link in &mut late {
        link.drain();
        assert!(link.closed);
        assert!(link.log.is_empty(), "{}", brief(&link.log));
    }
    assert_eq!(pair.a.region().edge(F), None);

    let f = greeted(&mut pair.a, greeting(F, 5).chunks(vec![HOME]));
    assert!(
        matches!(
            welcomed(&f.log),
            Welcome::Unknown {
                entries: 0,
                presences: 0,
                applied: 0,
                ..
            }
        ),
        "{}",
        brief(&f.log)
    );
}

/// Section 3.1 on a thread of its own: the outcome comes by a call into a channel,
/// and a command the region's thread never took, because it had ended, is answered
/// `Busy`.
#[test]
fn a_worker_on_its_own_thread_merges_and_says_the_outcome_by_a_call() {
    let mut pair = pair(World::stripes());
    let absorbed = pair.release_b();
    let Pair {
        world,
        a,
        mut to_a,
        one,
        two,
        ..
    } = pair;
    let links = a.links();
    let worker = Worker::spawn(a);
    let (said, heard) = mpsc::channel();
    worker.reshape(
        absorbed.order(),
        Box::new(move |reshaped| {
            let _ = said.send(reshaped);
        }),
    );
    let reshaped = heard
        .recv_timeout(Duration::from_secs(60))
        .expect("the outcome is said");
    assert_eq!(reshaped, Reshaped::Absorbed { absorbed: B });
    drop(absorbed);
    assert_eq!(world.list().absorbed, vec![(B, A)]);
    for _ in 0..STEPS {
        to_a.drain();
        if to_a.closed {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(to_a.closed);

    // An edge whose link the merge closed is let in again at once.
    let (edge, end) = link::in_process::<EdgeMessage, WorkerToEdge>(CAPACITY);
    links.attach(end);
    let mut again = Link::of(edge);
    again.hello(
        greeting(E, 5)
            .since(since(&to_a.log))
            .seen(1)
            .players(vec![player(1)])
            .chunks(vec![HOME]),
    );
    for _ in 0..STEPS * 4 {
        again.drain();
        if again.resumed() {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        presences(&again.log),
        vec![(player(1), Some(one)), (player(2), Some(two))],
        "{}",
        brief(&again.log)
    );

    worker.begin_release();
    for _ in 0..STEPS * 4 {
        if worker.is_finished() {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(worker.is_finished());
    let (said, heard) = mpsc::channel();
    worker.reshape(
        Reshape::SplitOff {
            chunks: vec![NEXT],
            as_epoch: 99,
            part: RegionId(2),
        },
        Box::new(move |reshaped| {
            let _ = said.send(reshaped);
        }),
    );
    assert_eq!(
        heard.recv_timeout(Duration::from_secs(60)).ok(),
        Some(Reshaped::Off { why: Off::Busy })
    );
    assert_eq!(worker.release(), Ended::Released);
}

// ---------------------------------------------------------------------------------------
// R39 to R47, sections 3.4 and 3.5 and rules 45 to 48. A split, seen from links
// ---------------------------------------------------------------------------------------

/// Where player 2 of a [`Whole`] stands, which is in [`FAR`], and a block there that
/// they can reach.
const FAR_X: f64 = -40.5;
const FAR_BLOCK: BlockPos = BlockPos::new(-41, GROUND, 8);

/// The home region of a world, with a link of edge E and players 1 and 2 of it.
struct Whole {
    world: World,
    a: RegionRunner,
    to_a: Link,
    one: EntityId,
    two: EntityId,
}

/// A [`Whole`] whose link sees `view`, with the players on the line the tests walk
/// along at `one_at` and `two_at`. A player who is not at the spawn has made one step.
fn whole_with(mut world: World, view: Vec<ChunkPos>, one_at: f64, two_at: f64) -> Whole {
    let a = world.open(world.shape.home());
    whole_of(world, a, view, one_at, two_at)
}

/// [`whole_with`], on a runner the test has made.
fn whole_of(
    world: World,
    mut a: RegionRunner,
    view: Vec<ChunkPos>,
    one_at: f64,
    two_at: f64,
) -> Whole {
    let mut to_a = greeted(&mut a, greeting(E, 5).chunks(view));
    let one = join_and_wait(&mut a, &mut to_a, player(1));
    let two = join_and_wait(&mut a, &mut to_a, player(2));
    if one_at != SPAWN.x {
        to_a.next(input(player(1), one, 1, move_to(one_at)));
    }
    let walked = to_a.next(input(player(2), two, 1, move_to(two_at)));
    wait_applied(&mut a, &mut [&mut to_a], 0, walked);
    sync(&mut a, &mut [&mut to_a], 0);
    let state = a.region().state();
    assert_eq!(state.players[&player(1)].pose.position.x, one_at);
    assert_eq!(state.players[&player(2)].pose.position.x, two_at);
    Whole {
        world,
        a,
        to_a,
        one,
        two,
    }
}

/// On stripes: region 0 with the three chunks [`BACK`], [`FAR`] and [`FARTHER`] in
/// view, player 1 at the spawn and player 2 in [`FAR`], where they have broken
/// [`FAR_BLOCK`]. Region 0 has one entry that edge E has not confirmed, what is left
/// of a dig of player 1 beyond the stripe.
fn whole(world: World) -> Whole {
    let mut whole = whole_with(world, vec![HOME, BACK, FAR, FARTHER], SPAWN.x, FAR_X);
    let Whole {
        a, to_a, one, two, ..
    } = &mut whole;
    to_a.next(input(player(1), *one, 1, dig(BEYOND, 1)));
    let dug = to_a.next(input(player(2), *two, 2, dig(FAR_BLOCK, 1)));
    wait_applied(a, &mut [to_a], 0, dug);
    sync(a, &mut [to_a], 0);
    assert_eq!(block_of(a, FAR_BLOCK), Some(blocks::AIR));
    assert_eq!(outbox(&to_a.log).len(), 1, "{}", brief(&to_a.log));
    whole
}

impl Whole {
    /// Splits the players standing in `chunks` off the region, as the coordinator
    /// orders it: with the next id of the store's list and an epoch above every
    /// other. Returns the new region, that epoch and the part.
    fn split(&mut self, chunks: Vec<ChunkPos>) -> (RegionId, u64, Part) {
        let as_epoch = self.world.next_epoch();
        let next = self.world.list().next;
        let outcome = ask(
            &mut self.a,
            Reshape::SplitOff {
                chunks,
                as_epoch,
                part: next,
            },
        );
        match run_to_outcome(&mut self.a, &mut [&mut self.to_a], &outcome) {
            Reshaped::Split {
                region,
                as_epoch: said,
                part,
            } => {
                assert_eq!((region, said), (next, as_epoch));
                self.to_a.drain();
                assert!(self.to_a.closed);
                (region, as_epoch, part)
            }
            other => panic!("no split: {other:?}"),
        }
    }
}

/// The positions of chunks that come with their content.
fn positions(chunks: &[(ChunkPos, Chunk)]) -> Vec<ChunkPos> {
    chunks.iter().map(|(position, _)| *position).collect()
}

#[test]
fn a_split_closes_the_link_and_the_next_hello_is_told_who_went_and_where_the_chunks_are() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_split_closes_the_link_and_the_next_hello_is_told_who_went_and_where_the_chunks_are_in,
    );
}

/// R39, with the two states of section 2.4 as the runner took them.
fn a_split_closes_the_link_and_the_next_hello_is_told_who_went_and_where_the_chunks_are_in(
    world: World,
) {
    let mut whole = whole(world);
    let before = whole.world.list();
    let as_epoch = whole.world.next_epoch();
    let outcome = ask(
        &mut whole.a,
        Reshape::SplitOff {
            chunks: vec![FAR],
            as_epoch,
            part: before.next,
        },
    );
    run_to_stage(&mut whole.a, &mut [&mut whole.to_a], Stage::Settling);
    let ours = whole.a.region().state();
    assert_eq!(ours.edges[&E].applied, whole.to_a.sent);
    let told = whole.to_a.log.len();

    let mut reshaped = None;
    for _ in 0..STEPS {
        whole.a.step();
        whole.to_a.drain();
        reshaped = outcome.taken();
        assert_eq!(
            whole.to_a.closed,
            reshaped.is_some(),
            "the link and the outcome disagree at {:?}",
            whole.a.stage()
        );
        if reshaped.is_some() {
            break;
        }
        if whole.a.stage() != Some(Stage::Committing) {
            assert_eq!(whole.world.list(), before, "at {:?}", whole.a.stage());
        }
        thread::sleep(Duration::from_millis(1));
    }
    let Some(Reshaped::Split {
        region: n,
        as_epoch: said,
        part,
    }) = reshaped
    else {
        panic!("no split: {reshaped:?}");
    };
    assert_eq!((n, said), (before.next, as_epoch));
    assert_eq!(whole.a.stage(), None);
    assert_eq!(whole.to_a.log.len(), told, "{}", brief(&whole.to_a.log));
    // A split answers no subscription.
    assert_eq!(whole.to_a.answers(FAR), vec![(0, Answer::Snapshot)]);

    // The two states, as of the tick after the region's last.
    let m = ours.tick + 1;
    let mut stays = ours.clone();
    stays.tick = m;
    let went = stays
        .players
        .remove(&player(2))
        .expect("player 2 was there");
    let edge = stays.edges.get_mut(&E).expect("the region knows the edge");
    edge.sent += 1;
    edge.outbox.insert(
        edge.sent,
        Durable::SplitOff {
            region: n,
            players: vec![(player(2), whole.two)],
        },
    );
    assert_eq!(edge.sent, 2);
    assert_eq!(whole.a.region().state(), stays);
    let goes = RegionState {
        tick: m,
        entity_ids: EntityIds {
            first: EntityId(0),
            end: EntityId(0),
        },
        next_entity_id: EntityId(0),
        players: BTreeMap::from([(player(2), went)]),
        edges: BTreeMap::from([(
            E,
            EdgeState {
                start: 5,
                since: m,
                applied: 0,
                sent: 0,
                outbox: BTreeMap::new(),
            },
        )]),
    };
    assert_eq!(part.region.state(), goes);

    // The chunks: what is nearer to the player who went than to the home chunk.
    assert_eq!(positions(&part.chunks), vec![FARTHER, FAR]);
    for chunk in [FAR, FARTHER] {
        assert_eq!(part.region.knowledge(chunk), Knowledge::Held);
        assert_eq!(whole.a.region().knowledge(chunk), Knowledge::Unknown);
    }
    for chunk in [HOME, BACK] {
        assert_eq!(part.region.knowledge(chunk), Knowledge::Unknown);
        assert_eq!(whole.a.region().knowledge(chunk), Knowledge::Held);
    }
    assert_eq!(whole.a.region().loaded_chunk_count(), 0);
    assert!(
        whole.a.region().pins(FAR),
        "the region stays pinned to its area"
    );
    let (_, chunk) = &part.chunks[1];
    assert_eq!(block_in(chunk, FAR_BLOCK), blocks::AIR);

    // A new link whose hello names both players and chunks on both sides, and sends
    // a step behind it.
    let mut again = Link::attach(&whole.a).after(whole.to_a.sent);
    again.hello(
        greeting(E, 5)
            .since(since(&whole.to_a.log))
            .seen(1)
            .players(vec![player(2), player(1)])
            .chunks(vec![HOME, FAR])
            .guests(vec![BACK, FARTHER]),
    );
    let walked = again.next(input(player(1), whole.one, 2, move_to(12.5)));
    wait_applied(&mut whole.a, &mut [&mut again], 0, walked);
    sync(&mut whole.a, &mut [&mut again], 0);
    let log = &again.log;
    assert_eq!(
        welcomed(log),
        Welcome::Resumed {
            entries: 1,
            presences: 2,
            applied: ours.edges[&E].applied,
        },
        "{}",
        brief(log)
    );
    assert_eq!(
        entries(log),
        vec![(
            2,
            Durable::SplitOff {
                region: n,
                players: vec![(player(2), whole.two)],
            }
        )]
    );
    assert_eq!(
        presences(log),
        vec![(player(2), None), (player(1), Some(whole.one))]
    );
    assert_eq!(again.answers(HOME), vec![(0, Answer::Snapshot)]);
    assert_eq!(again.answers(BACK), vec![(0, Answer::Snapshot)]);
    assert_eq!(again.answers(FAR), vec![(0, Answer::Elsewhere(n))]);
    assert_eq!(again.answers(FARTHER), vec![(0, Answer::NotMine)]);
    let (stepped, _) = moved(log, whole.one, 12.5).expect("the step is applied");
    for chunk in [HOME, BACK, FAR, FARTHER] {
        assert!(
            answer_to(log, chunk, 0).expect("asserted above").0 < stepped,
            "the step did not wait for {chunk:?}: {}",
            brief(log)
        );
    }
    assert_eq!(whole.a.region().knowledge(FAR), Knowledge::Foreign(n));
    assert_eq!(outcome.taken(), None, "the outcome is said once");
}

#[test]
fn after_a_split_the_list_has_the_new_region_and_a_crash_of_either_restores_the_state_of_the_split()
{
    in_memory_and_on_disk(
        Shape::Stripes,
        after_a_split_the_list_has_the_new_region_and_a_crash_of_either_restores_the_state_of_the_split_in,
    );
}

/// R40.
fn after_a_split_the_list_has_the_new_region_and_a_crash_of_either_restores_the_state_of_the_split_in(
    world: World,
) {
    let mut whole = whole(world);
    let before = whole.world.list();
    let (n, as_epoch, part) = whole.split(vec![FAR]);
    let m = whole.a.region().tick_number();
    let stays = whole.a.region().state();
    let goes = part.region.state();
    assert_eq!((stays.tick, goes.tick), (m, m));

    let list = whole.world.list();
    assert_eq!(n, before.next);
    assert_eq!(list.next, RegionId(n.0 + 1));
    assert_eq!(list.home, A);
    assert!(list.absorbed.is_empty());
    assert_eq!(list.regions[..2], before.regions[..]);
    assert_eq!(list.regions.len(), 3);
    let new = &list.regions[2];
    assert_eq!((new.region, new.epoch), (n, as_epoch));
    assert_eq!(
        new.bounds.map(|bounds| (bounds.min, bounds.max)),
        Some((FARTHER, FAR))
    );
    assert!(new.pinned.is_empty());

    // Nobody with a lower epoch than the split named can open the part.
    assert!(matches!(
        whole.world.open_with(n, as_epoch - 1),
        Err(StoreError::EpochRefused { seen, .. }) if seen == as_epoch
    ));

    let (handle, restored) = whole.world.open_raw(A);
    assert_eq!(restored.tick(), m);
    assert!(restored.deltas.is_empty());
    assert_eq!(restored.pinned, areas(Shape::Stripes)[..1]);
    assert!(restored.held.is_empty());
    let next = RegionRunner::restore(config(RETURN_AFTER), handle, restored)
        .expect("what the store has is readable");
    assert_eq!(next.region().state(), stays);

    let (handle, restored) = whole.world.open_raw(n);
    assert_eq!(restored.tick(), m);
    assert!(restored.deltas.is_empty());
    assert_eq!(
        restored.entity_ids,
        EntityIds {
            first: EntityId(0),
            end: EntityId(0),
        }
    );
    assert_eq!(restored.held, vec![(FARTHER, m), (FAR, m)]);
    assert!(restored.pinned.is_empty());
    let next = RegionRunner::restore(config(RETURN_AFTER), handle, restored)
        .expect("what the store has is readable");
    assert_eq!(next.region().state(), goes);
    assert_eq!(next.region().knowledge(FAR), Knowledge::Held);
    // Every player is in exactly one of the two.
    assert_eq!(stays.players.keys().collect::<Vec<_>>(), vec![&player(1)]);
    assert_eq!(goes.players.keys().collect::<Vec<_>>(), vec![&player(2)]);
}

#[test]
fn a_runner_made_of_the_part_says_whom_it_has_and_serves_the_parts_chunks() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_runner_made_of_the_part_says_whom_it_has_and_serves_the_parts_chunks_in,
    );
}

/// R41 and rule 47.
fn a_runner_made_of_the_part_says_whom_it_has_and_serves_the_parts_chunks_in(world: World) {
    let mut whole = whole(world);
    let (n, as_epoch, part) = whole.split(vec![FAR]);
    let m = whole.a.region().tick_number();
    let mut part_runner = whole.world.run_part(n, as_epoch, part);
    assert_eq!(part_runner.region().tick_number(), m);
    assert_eq!(part_runner.stage(), None);

    // A hello that names nobody: the edge has not read the entry of the split yet.
    let first = greeted(&mut part_runner, greeting(E, 5));
    assert_eq!(
        welcomed(&first.log),
        Welcome::Unknown {
            since: m,
            entries: 0,
            presences: 1,
            applied: 0,
        },
        "{}",
        brief(&first.log)
    );
    assert_eq!(presences(&first.log), vec![(player(2), Some(whole.two))]);
    assert!(
        matches!(
            presence(&first.log, player(2)),
            Some(Presence::Present { pose, last_input: 2, handled: Some(1), .. })
                if pose.position.x == FAR_X
        ),
        "{}",
        brief(&first.log)
    );

    // One that names the player and the part's chunks.
    let mut second = greeted(
        &mut part_runner,
        greeting(E, 5)
            .since(m)
            .players(vec![player(2)])
            .chunks(vec![FAR, FARTHER]),
    );
    let log = &second.log;
    assert_eq!(
        welcomed(log),
        Welcome::Resumed {
            entries: 0,
            presences: 1,
            applied: 0,
        },
        "{}",
        brief(log)
    );
    assert_eq!(presences(log), vec![(player(2), Some(whole.two))]);
    assert_eq!(second.answers(FAR), vec![(0, Answer::Snapshot)]);
    assert_eq!(second.answers(FARTHER), vec![(0, Answer::Snapshot)]);
    let (_, chunk, entities) = snapshot(log, FAR, 0).expect("asserted above");
    assert_eq!(block_in(chunk, FAR_BLOCK), blocks::AIR);
    assert!(entities.iter().any(|state| state.entity == whole.two));

    // The player goes on where they were, numbered from 1 with the new region, and
    // nobody joins it: it has no entity ids to give.
    let walked = second.next(input(player(2), whole.two, 3, move_to(FAR_X - 1.0)));
    assert_eq!(walked, 1);
    let joined = second.next(join(player(7)));
    wait_applied(&mut part_runner, &mut [&mut second], 0, joined);
    sync(&mut part_runner, &mut [&mut second], 0);
    assert!(moved(&second.log, whole.two, FAR_X - 1.0).is_some());
    assert!(
        matches!(
            outbox(&second.log).as_slice(),
            [(_, 1, Durable::Refused { player: refused })] if *refused == player(7)
        ),
        "{}",
        brief(&second.log)
    );
    assert_eq!(part_runner.region().player_count(), 1);
}

/// R43, `Nobody`: no named chunk has a player; the only named chunk with a player is
/// the home chunk; or it is one the region does not hold.
#[test]
fn a_split_of_nobody_is_off_and_the_region_ticks_on_with_its_link() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_split_of_nobody_is_off_and_the_region_ticks_on_with_its_link_in,
    );
}

fn a_split_of_nobody_is_off_and_the_region_ticks_on_with_its_link_in(world: World) {
    let mut whole = whole(world);
    assert_eq!(whole.a.region().knowledge(BACK), Knowledge::Held);
    assert_eq!(whole.a.region().knowledge(OTHER), Knowledge::Unknown);
    for (step, chunks) in [
        vec![BACK, FARTHER],
        vec![HOME],
        vec![OTHER, NEXT],
        Vec::new(),
    ]
    .into_iter()
    .enumerate()
    {
        let order = Reshape::SplitOff {
            chunks,
            as_epoch: whole.world.next_epoch(),
            part: RegionId(2),
        };
        assert_eq!(
            comes_to_nothing(
                &whole.world,
                &mut whole.a,
                &mut whole.to_a,
                (player(1), whole.one),
                order,
                step as u64 + 2,
            ),
            Off::Nobody
        );
    }
    // And then it can be split.
    let (n, _, _) = whole.split(vec![FAR]);
    assert_eq!(n, RegionId(2));
}

/// R43, `NothingStays`: a region that holds no home chunk and is pinned to nothing is
/// not split when nobody would stay; it would only get a new name.
#[test]
fn a_split_that_would_leave_a_region_with_nothing_is_off() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_split_that_would_leave_a_region_with_nothing_is_off_in,
    );
}

fn a_split_that_would_leave_a_region_with_nothing_is_off_in(world: World) {
    let mut whole = whole(world);
    let (n, as_epoch, part) = whole.split(vec![FAR]);
    let mut part_runner = whole.world.run_part(n, as_epoch, part);
    let mut to_n = greeted(
        &mut part_runner,
        greeting(E, 5).players(vec![player(2)]).chunks(vec![FAR]),
    );
    sync(&mut part_runner, &mut [&mut to_n], 0);
    let order = Reshape::SplitOff {
        chunks: vec![FAR],
        as_epoch: whole.world.next_epoch(),
        part: RegionId(n.0 + 1),
    };
    assert_eq!(
        comes_to_nothing(
            &whole.world,
            &mut part_runner,
            &mut to_n,
            (player(2), whole.two),
            order,
            3,
        ),
        Off::NothingStays
    );
    // The same of a pinned region is a split: nobody stays in region 0 when its one
    // player goes, and it keeps its area and the home chunk.
    let mut again = greeted(
        &mut whole.a,
        greeting(E, 5)
            .since(since(&whole.to_a.log))
            .seen(2)
            .players(vec![player(1)])
            .chunks(vec![HOME, BACK]),
    )
    .after(whole.to_a.sent);
    let walked = again.next(input(player(1), whole.one, 2, move_to(-15.5)));
    wait_applied(&mut whole.a, &mut [&mut again], 0, walked);
    sync(&mut whole.a, &mut [&mut again], 0);
    whole.to_a = again;
    let (second, _, part) = whole.split(vec![BACK]);
    assert_eq!(second, RegionId(n.0 + 1));
    assert_eq!(whole.a.region().player_count(), 0);
    assert_eq!(part.region.player_count(), 1);
    assert_eq!(whole.a.region().knowledge(HOME), Knowledge::Held);
}

/// R44 and section 3.4, item 3: the store gives out region ids, and a split whose
/// order named another one than the next is made again with the one the store names.
#[test]
fn a_split_that_names_a_wrong_id_is_made_with_the_one_the_store_has_next() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_split_that_names_a_wrong_id_is_made_with_the_one_the_store_has_next_in,
    );
}

fn a_split_that_names_a_wrong_id_is_made_with_the_one_the_store_has_next_in(world: World) {
    let mut whole = whole(world);
    let as_epoch = whole.world.next_epoch();
    let outcome = ask(
        &mut whole.a,
        Reshape::SplitOff {
            chunks: vec![FAR],
            as_epoch,
            part: RegionId(7),
        },
    );
    let reshaped = run_to_outcome(&mut whole.a, &mut [&mut whole.to_a], &outcome);
    let Reshaped::Split {
        region,
        as_epoch: said,
        part,
    } = reshaped
    else {
        panic!("no split: {reshaped:?}");
    };
    assert_eq!((region, said), (RegionId(2), as_epoch));
    let list = whole.world.list();
    assert_eq!(
        list.regions
            .iter()
            .map(|info| info.region)
            .collect::<Vec<_>>(),
        vec![A, B, RegionId(2)]
    );
    assert_eq!(list.next, RegionId(3));
    let state = whole.a.region().state();
    assert_eq!(
        state.edges[&E].outbox.values().last(),
        Some(&Durable::SplitOff {
            region: RegionId(2),
            players: vec![(player(2), whole.two)],
        })
    );
    assert_eq!(state.edges[&E].sent, 2);
    // The part is the store's region 2: its worker's hello is answered, and it runs.
    let mut part_runner = whole.world.run_part(region, as_epoch, part);
    let to_n = greeted(&mut part_runner, greeting(E, 5));
    assert_eq!(presences(&to_n.log), vec![(player(2), Some(whole.two))]);
}

/// Section 3.4, item 3: "A second `NotNext` is a decline like any other." Region 1 is
/// split between the first answer to region 0 and its second commit, so the id the
/// store named is taken as well.
#[test]
fn a_split_whose_second_id_is_taken_as_well_is_off_and_can_be_asked_for_again() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_split_whose_second_id_is_taken_as_well_is_off_and_can_be_asked_for_again_in,
    );
}

fn a_split_whose_second_id_is_taken_as_well_is_off_and_can_be_asked_for_again_in(world: World) {
    let mut whole = whole(world);
    let mut b = whole.world.open(B);
    let theirs = ChunkPos::new(6, 0);
    let mut to_b = greeted(&mut b, greeting(F, 5).chunks(vec![theirs]));
    let came = to_b.next(arrive(player(3), &transfer(STRANGER, 100.5)));
    wait_applied(&mut b, &mut [&mut to_b], 0, came);
    sync(&mut b, &mut [&mut to_b], 0);

    let first = ask(
        &mut whole.a,
        Reshape::SplitOff {
            chunks: vec![FAR],
            as_epoch: whole.world.next_epoch(),
            part: RegionId(7),
        },
    );
    run_to_stage(&mut whole.a, &mut [&mut whole.to_a], Stage::Committing);
    // The store has declined that and named 2; the runner has not taken the answer.
    // Region 1 is split meanwhile, and gets 2.
    let other = ask(
        &mut b,
        Reshape::SplitOff {
            chunks: vec![theirs],
            as_epoch: whole.world.next_epoch(),
            part: RegionId(2),
        },
    );
    assert!(matches!(
        run_to_outcome(&mut b, &mut [&mut to_b], &other),
        Reshaped::Split {
            region: RegionId(2),
            ..
        }
    ));
    let list = whole.world.list();

    let state = whole.a.region().state();
    let reshaped = run_to_outcome(&mut whole.a, &mut [&mut whole.to_a], &first);
    assert_eq!(
        reshaped,
        Reshaped::Off {
            why: Off::Declined(Decline::NotNext { next: RegionId(3) })
        }
    );
    assert!(!whole.to_a.closed);
    assert_eq!(whole.a.region().state(), state);
    assert_eq!(whole.world.list(), list);
    sync(&mut whole.a, &mut [&mut whole.to_a], 0);

    let (n, _, _) = whole.split(vec![FAR]);
    assert_eq!(n, RegionId(3));
    assert_eq!(whole.world.list().next, RegionId(4));
}

/// ADR-0011, section 3.7: a split whose part could never be opened is declined.
#[test]
fn a_split_with_no_epoch_for_the_part_is_declined_and_the_region_ticks_on() {
    let mut whole = whole(World::stripes());
    let order = Reshape::SplitOff {
        chunks: vec![FAR],
        as_epoch: 0,
        part: RegionId(2),
    };
    assert_eq!(
        comes_to_nothing(
            &whole.world,
            &mut whole.a,
            &mut whole.to_a,
            (player(1), whole.one),
            order,
            2,
        ),
        Off::Declined(Decline::Malformed)
    );
    assert_eq!(whole.a.region().player_count(), 2);
}

/// On stripes: region 0 with player 1 in [`BACK`], next to the chunk [`SECOND`], in
/// which player 2 stands; [`SECOND`] has been split off with player 2. Returns the
/// new region as well.
fn split_beside_a_player(world: World) -> (Whole, RegionId, u64, Part) {
    let mut whole = whole_with(world, vec![HOME, BACK, SECOND], -15.5, -20.5);
    let (n, as_epoch, part) = whole.split(vec![SECOND]);
    assert_eq!(positions(&part.chunks), vec![SECOND]);
    (whole, n, as_epoch, part)
}

/// A block of [`SECOND`] that a player at x = -15.5 can reach.
const SECOND_BLOCK: BlockPos = BlockPos::new(-17, GROUND, 8);

#[test]
fn after_a_split_what_is_sent_to_the_split_region_about_the_part_goes_on_to_the_new_region() {
    in_memory_and_on_disk(
        Shape::Stripes,
        after_a_split_what_is_sent_to_the_split_region_about_the_part_goes_on_to_the_new_region_in,
    );
}

/// R45 and rules 45 and 48 on stripes, where the part's chunks lie in the split
/// region's own pinned area: a player or an action that is sent there for such a
/// chunk is taken as if the region knew nothing of the chunk (section 2.2).
fn after_a_split_what_is_sent_to_the_split_region_about_the_part_goes_on_to_the_new_region_in(
    world: World,
) {
    let (mut whole, n, _, _) = split_beside_a_player(world);
    let mut again = Link::attach(&whole.a).after(whole.to_a.sent);
    again.hello(
        greeting(E, 5)
            .since(since(&whole.to_a.log))
            .players(vec![player(1)])
            .chunks(vec![HOME, BACK, SECOND]),
    );
    // Behind the hello: a dig of the player who stayed into a chunk of the part.
    let dug = again.next(input(player(1), whole.one, 2, dig(SECOND_BLOCK, 1)));
    wait_applied(&mut whole.a, &mut [&mut again], 0, dug);
    sync(&mut whole.a, &mut [&mut again], 0);
    let log = &again.log;
    assert_eq!(again.answers(SECOND), vec![(0, Answer::Elsewhere(n))]);
    let (told, _) = answer_to(log, SECOND, 0).expect("asserted above");
    let entries = outbox(log);
    assert!(
        matches!(
            entries.as_slice(),
            [
                (_, 1, Durable::SplitOff { .. }),
                (at, 2, Durable::Remote { action, to }),
            ] if *action == breaking(player(1), 1, SECOND_BLOCK) && *to == Some(n) && *at > told
        ),
        "{}",
        brief(log)
    );
    assert!(acknowledged(log, player(1), 1).is_none());

    // An input that names the stay that went is passed over without a word, and
    // progress covers its number.
    let stale = again.next(input(player(2), whole.two, 2, move_to(-21.5)));
    wait_applied(&mut whole.a, &mut [&mut again], 0, stale);
    assert!(whole.a.region().player(player(2)).is_none());
    assert!(moved(&again.log, whole.two, -21.5).is_none());

    // An arrival for a chunk of the part: taken in, and let go to the new region
    // when the store has said who holds the chunk.
    again.next(arrive(player(7), &transfer(STRANGER, -20.5)));
    run_until(
        &mut whole.a,
        &mut [&mut again],
        "the arrival being let go",
        |_, links| departure(&links[0].log, player(7)).is_some(),
    );
    let (_, _, to, sent_on) = departure(&again.log, player(7)).expect("waited for it");
    assert_eq!(to, n);
    assert_eq!(sent_on.entity_id, STRANGER);
    assert!(whole.a.region().player(player(7)).is_none());

    // A remote action about such a chunk goes on, without a region or to the new one.
    let block = SECOND_BLOCK.offset(0, 0, 1);
    let remote = again.next(EdgeToWorker::Remote(breaking(player(9), 5, block)));
    wait_applied(&mut whole.a, &mut [&mut again], 0, remote);
    sync(&mut whole.a, &mut [&mut again], 0);
    let entries = outbox(&again.log);
    let (_, _, last) = entries.last().expect("there are entries");
    assert!(
        match last {
            Durable::Remote { action, to } =>
                *action == breaking(player(9), 5, block) && (to.is_none() || *to == Some(n)),
            Durable::NotMine {
                what: Misdirected::Remote(action),
                holder,
            } => *action == breaking(player(9), 5, block) && *holder == n,
            _ => false,
        },
        "{}",
        brief(&again.log)
    );
    assert!(block_change(&again.log, block).is_none());
}

/// Section 3.6 and rule 34 in the one place a held action ends otherwise than with a
/// snapshot: in a region's own pinned area, where another region holds the chunk.
/// The action waits for the store's answer, and is judged when the asking has been
/// told `Elsewhere` or `NotMine`.
#[test]
fn a_dig_held_for_a_chunk_of_the_part_is_judged_when_its_asking_is_told_elsewhere_or_not_mine() {
    for (on_disk, kind) in [
        (false, Kind::Viewer),
        (false, Kind::Guest),
        (true, Kind::Viewer),
        (true, Kind::Guest),
    ] {
        let (mut whole, n, _, _) = split_beside_a_player(World::new(Shape::Stripes, on_disk));
        // The hello names nothing of the part, so that its hold is over.
        let mut again = greeted(
            &mut whole.a,
            greeting(E, 5)
                .since(since(&whole.to_a.log))
                .seen(1)
                .players(vec![player(1)])
                .chunks(vec![HOME, BACK]),
        )
        .after(whole.to_a.sent);
        assert_eq!(whole.a.region().knowledge(SECOND), Knowledge::Unknown);
        assert!(whole.a.region().pins(SECOND));

        let ask = again.say(Said::Asked(kind), vec![SECOND]);
        let dug = again.next(input(player(1), whole.one, 2, dig(SECOND_BLOCK, 1)));
        let walked = again.next(input(player(1), whole.one, 3, move_to(-14.5)));
        wait_applied(&mut whole.a, &mut [&mut again], 0, walked);
        sync(&mut whole.a, &mut [&mut again], 0);

        let log = &again.log;
        let (told, answer) = answer_to(log, SECOND, ask).expect("the asking is answered");
        assert_eq!(
            answer,
            match kind {
                Kind::Viewer => Answer::Elsewhere(n),
                Kind::Guest => Answer::NotMine,
            }
        );
        let entries = outbox(log);
        let Some((at, _, Durable::Remote { action, to })) = entries.last() else {
            panic!("the dig was not passed on: {}", brief(log));
        };
        assert_eq!(*action, breaking(player(1), 1, SECOND_BLOCK));
        assert!(
            *at > told,
            "the dig was judged before the asking was answered: {}",
            brief(log)
        );
        if kind == Kind::Viewer {
            assert_eq!(*to, Some(n), "{}", brief(log));
        }
        assert!(progress_to(log, dug).expect("waited for it") > told);
        assert!(moved(log, whole.one, -14.5).expect("the step is applied").0 > told);
    }
}

/// R45 and rule 46 with the gap, where the part's chunks lie outside every pinned
/// area of the split region: a guest is told `NotMine` at once, and what is sent
/// there for a chunk of the part is sent on with `NotMine` once the region has heard
/// who holds it.
#[test]
fn with_the_gap_what_reaches_the_split_region_for_a_chunk_of_the_part_is_sent_on_to_the_new_region()
{
    in_memory_and_on_disk(
        Shape::Gap,
        with_the_gap_what_reaches_the_split_region_for_a_chunk_of_the_part_is_sent_on_to_the_new_region_in,
    );
}

fn with_the_gap_what_reaches_the_split_region_for_a_chunk_of_the_part_is_sent_on_to_the_new_region_in(
    world: World,
) {
    // Player 2 stands in the second free chunk east of the home chunk; the first is
    // as near to the home chunk as to them, and stays.
    let mut whole = whole_with(world, vec![HOME, NEXT, FREE], SPAWN.x, 40.5);
    let (n, _, part) = whole.split(vec![FREE]);
    assert_eq!(n, RegionId(3));
    assert_eq!(positions(&part.chunks), vec![FREE]);
    assert_eq!(whole.a.region().knowledge(NEXT), Knowledge::Held);
    assert_eq!(whole.a.region().knowledge(FREE), Knowledge::Unknown);
    assert!(!whole.a.region().pins(FREE));

    // A guest's asking is answered in the tick that takes it, and the region does
    // not ask the store on its account.
    let guest = greeted(&mut whole.a, greeting(F, 5).guests(vec![FREE]));
    assert_eq!(guest.answers(FREE), vec![(0, Answer::NotMine)]);
    assert!(
        answer_to(&guest.log, FREE, 0).expect("asserted above").0
            < progress_to(&guest.log, 0).expect("a progress comes with every hello"),
        "{}",
        brief(&guest.log)
    );
    assert_eq!(whole.a.region().knowledge(FREE), Knowledge::Unknown);

    // A viewer's is told where the chunk is, a tick or two later.
    let mut again = greeted(
        &mut whole.a,
        greeting(E, 5)
            .since(since(&whole.to_a.log))
            .players(vec![player(1)])
            .chunks(vec![HOME, NEXT, FREE]),
    )
    .after(whole.to_a.sent);
    assert_eq!(again.answers(FREE), vec![(0, Answer::Elsewhere(n))]);
    assert_eq!(again.answers(NEXT), vec![(0, Answer::Snapshot)]);
    assert!(
        matches!(
            entries(&again.log).as_slice(),
            [(1, Durable::SplitOff { region, players })]
                if *region == n && *players == [(player(2), whole.two)]
        ),
        "{}",
        brief(&again.log)
    );

    // An arrival and a remote action for the chunk are sent on to the new region.
    let stray = transfer(STRANGER, 40.5);
    let block = BlockPos::new(33, GROUND, 8);
    again.next(arrive(player(7), &stray));
    let remote = again.next(EdgeToWorker::Remote(breaking(player(9), 5, block)));
    wait_applied(&mut whole.a, &mut [&mut again], 0, remote);
    sync(&mut whole.a, &mut [&mut again], 0);
    let entries = outbox(&again.log);
    assert!(
        matches!(
            &entries[1..],
            [
                (
                    _,
                    2,
                    Durable::NotMine {
                        what: Misdirected::Arrival { player: who, transfer },
                        holder: first,
                    }
                ),
                (
                    _,
                    3,
                    Durable::NotMine {
                        what: Misdirected::Remote(action),
                        holder: second,
                    }
                ),
            ] if *who == player(7)
                && *transfer == stray
                && *first == n
                && *action == breaking(player(9), 5, block)
                && *second == n
        ),
        "{}",
        brief(&again.log)
    );
    assert!(whole.a.region().player(player(7)).is_none());
}

/// Rule 45: a player who went with the part walks back into the region that was
/// split. The new region lets them go like any player who steps into a chunk it does
/// not hold, and the split region takes them in; its presence answers have the stay
/// again, though the entry of the split, which the edge has yet to confirm, still
/// names it.
#[test]
fn a_player_who_went_with_the_part_and_walks_back_is_the_split_regions_again() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_player_who_went_with_the_part_and_walks_back_is_the_split_regions_again_in,
    );
}

fn a_player_who_went_with_the_part_and_walks_back_is_the_split_regions_again_in(world: World) {
    let (mut whole, n, as_epoch, part) = split_beside_a_player(world);
    let mut part_runner = whole.world.run_part(n, as_epoch, part);
    let mut to_n = greeted(
        &mut part_runner,
        greeting(E, 5)
            .players(vec![player(2)])
            .chunks(vec![SECOND, BACK]),
    );
    assert_eq!(to_n.answers(BACK), vec![(0, Answer::Elsewhere(A))]);
    to_n.next(input(player(2), whole.two, 2, move_to(-14.5)));
    run_until(
        &mut part_runner,
        &mut [&mut to_n],
        "the player being let go",
        |_, links| departure(&links[0].log, player(2)).is_some(),
    );
    let (_, number, to, back) = departure(&to_n.log, player(2)).expect("waited for it");
    assert_eq!((number, to), (1, A));
    assert_eq!(back.entity_id, whole.two);
    assert_eq!(back.last_input, 2);
    assert_eq!(part_runner.region().player_count(), 0);
    let back = back.clone();

    let mut again = greeted(
        &mut whole.a,
        greeting(E, 5)
            .since(since(&whole.to_a.log))
            .players(vec![player(1)])
            .chunks(vec![HOME, BACK]),
    )
    .after(whole.to_a.sent);
    assert_eq!(presences(&again.log), vec![(player(1), Some(whole.one))]);
    let came = again.next(arrive(player(2), &back));
    wait_applied(&mut whole.a, &mut [&mut again], 0, came);
    sync(&mut whole.a, &mut [&mut again], 0);
    assert_eq!(
        whole
            .a
            .region()
            .player(player(2))
            .map(|(entity, pose)| (entity, pose.position.x)),
        Some((whole.two, -14.5))
    );

    let last = greeted(
        &mut whole.a,
        greeting(E, 5)
            .since(since(&whole.to_a.log))
            .players(vec![player(1)])
            .chunks(vec![HOME, BACK]),
    );
    assert!(
        matches!(
            entries(&last.log).as_slice(),
            [(1, Durable::SplitOff { region, players })]
                if *region == n && *players == [(player(2), whole.two)]
        ),
        "{}",
        brief(&last.log)
    );
    assert_eq!(
        presences(&last.log),
        vec![(player(1), Some(whole.one)), (player(2), Some(whole.two))]
    );
    assert_eq!(presences(&last.log), stays(&whole.a.region().state(), E));
}

/// R46: a region that was itself split off is split, and the second part's runner
/// serves its chunks and says whom it has.
#[test]
fn a_region_that_was_split_off_is_split_itself_and_the_second_part_serves_its_chunks() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_region_that_was_split_off_is_split_itself_and_the_second_part_serves_its_chunks_in,
    );
}

fn a_region_that_was_split_off_is_split_itself_and_the_second_part_serves_its_chunks_in(
    world: World,
) {
    let distant = ChunkPos::new(-7, 0);
    let mut whole = whole_with(world, vec![HOME, FAR, distant], SPAWN.x, FAR_X);
    let three = join_and_wait(&mut whole.a, &mut whole.to_a, player(3));
    let walked = whole.to_a.next(input(player(3), three, 1, move_to(-104.5)));
    wait_applied(&mut whole.a, &mut [&mut whole.to_a], 0, walked);
    sync(&mut whole.a, &mut [&mut whole.to_a], 0);

    let (n, as_epoch, part) = whole.split(vec![FAR, distant]);
    assert_eq!(positions(&part.chunks), vec![distant, FAR]);
    assert_eq!(part.region.player_count(), 2);
    let mut first = whole.world.run_part(n, as_epoch, part);
    let mut to_n = greeted(
        &mut first,
        greeting(E, 5)
            .players(vec![player(2), player(3)])
            .chunks(vec![FAR, distant]),
    );
    sync(&mut first, &mut [&mut to_n], 0);
    let n_since = since(&to_n.log);

    let as_epoch = whole.world.next_epoch();
    let next = whole.world.list().next;
    let outcome = ask(
        &mut first,
        Reshape::SplitOff {
            chunks: vec![distant],
            as_epoch,
            part: next,
        },
    );
    let reshaped = run_to_outcome(&mut first, &mut [&mut to_n], &outcome);
    let Reshaped::Split { region, part, .. } = reshaped else {
        panic!("no split: {reshaped:?}");
    };
    assert_eq!(region, RegionId(n.0 + 1));
    assert!(to_n.closed);
    let m = first.region().tick_number();
    assert_eq!(positions(&part.chunks), vec![distant]);

    let mut second = whole.world.run_part(region, as_epoch, part);
    let to_second = greeted(
        &mut second,
        greeting(E, 5)
            .players(vec![player(3)])
            .chunks(vec![distant]),
    );
    assert_eq!(
        welcomed(&to_second.log),
        Welcome::Unknown {
            since: m,
            entries: 0,
            presences: 1,
            applied: 0,
        }
    );
    assert_eq!(presences(&to_second.log), vec![(player(3), Some(three))]);
    assert_eq!(to_second.answers(distant), vec![(0, Answer::Snapshot)]);
    let (_, _, entities) = snapshot(&to_second.log, distant, 0).expect("asserted above");
    assert!(entities.iter().any(|state| state.entity == three));

    // The region that was split says so in its next welcome, numbered from its own
    // first entry, and has the player who stayed.
    let again = greeted(
        &mut first,
        greeting(E, 5)
            .since(n_since)
            .players(vec![player(2), player(3)])
            .chunks(vec![FAR, distant]),
    );
    assert_eq!(
        entries(&again.log),
        vec![(
            1,
            Durable::SplitOff {
                region,
                players: vec![(player(3), three)],
            }
        )]
    );
    assert_eq!(
        presences(&again.log),
        vec![(player(2), Some(whole.two)), (player(3), None)]
    );
    assert_eq!(again.answers(distant), vec![(0, Answer::Elsewhere(region))]);
    assert_eq!(
        whole
            .world
            .list()
            .regions
            .iter()
            .map(|info| info.region)
            .collect::<Vec<_>>(),
        vec![A, B, n, region]
    );
}

/// The presence answers on a link whose hello named nobody, held against the state
/// they are made of: rule 37 says they are every stay the region has for the edge.
fn assert_presence_is_the_state(runner: &RegionRunner, link: &Link) {
    assert_eq!(
        presences(&link.log),
        stays(&runner.region().state(), E),
        "{}",
        brief(&link.log)
    );
}

/// R47, first half: a region that merges and then splits.
#[test]
fn a_region_that_merges_and_then_splits_says_at_every_hello_the_stays_its_state_has() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_region_that_merges_and_then_splits_says_at_every_hello_the_stays_its_state_has_in,
    );
}

fn a_region_that_merges_and_then_splits_says_at_every_hello_the_stays_its_state_has_in(
    world: World,
) {
    let mut pair = pair(world);
    let a_since = since(&pair.to_a.log);
    pair.merge();
    let merged = greeted(
        &mut pair.a,
        greeting(E, 5).since(a_since).chunks(vec![HOME, NEXT]),
    );
    assert_presence_is_the_state(&pair.a, &merged);
    assert_eq!(presences(&merged.log).len(), 2);
    assert_eq!(pair.a.region().knowledge(NEXT), Knowledge::Held);

    // The chunk that came with the merge is split off with the player who came.
    let mut whole = Whole {
        world: pair.world,
        a: pair.a,
        to_a: merged,
        one: pair.one,
        two: pair.two,
    };
    let (n, as_epoch, part) = whole.split(vec![NEXT]);
    assert_eq!(n, RegionId(2));
    assert_eq!(positions(&part.chunks), vec![NEXT]);
    let list = whole.world.list();
    assert_eq!(list.absorbed, vec![(B, A)]);
    assert_eq!(
        list.regions
            .iter()
            .map(|info| info.region)
            .collect::<Vec<_>>(),
        vec![A, n]
    );

    let again = greeted(
        &mut whole.a,
        greeting(E, 5)
            .since(a_since)
            .seen(3)
            .chunks(vec![HOME, NEXT]),
    );
    assert_eq!(
        entries(&again.log),
        vec![(
            4,
            Durable::SplitOff {
                region: n,
                players: vec![(player(2), whole.two)],
            }
        )]
    );
    assert_presence_is_the_state(&whole.a, &again);
    assert_eq!(presences(&again.log), vec![(player(1), Some(whole.one))]);
    assert_eq!(again.answers(NEXT), vec![(0, Answer::Elsewhere(n))]);

    let mut part_runner = whole.world.run_part(n, as_epoch, part);
    let to_n = greeted(&mut part_runner, greeting(E, 5).chunks(vec![NEXT]));
    assert_presence_is_the_state(&part_runner, &to_n);
    assert_eq!(presences(&to_n.log), vec![(player(2), Some(whole.two))]);
    assert_eq!(to_n.answers(NEXT), vec![(0, Answer::Snapshot)]);
}

/// R47, second half: a region that splits and then absorbs the part again. What the
/// part did meanwhile comes back with it, and the two entries are in the outbox in
/// the order they were made.
#[test]
fn a_region_that_splits_and_absorbs_the_part_again_says_at_every_hello_the_stays_its_state_has() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_region_that_splits_and_absorbs_the_part_again_says_at_every_hello_the_stays_its_state_has_in,
    );
}

fn a_region_that_splits_and_absorbs_the_part_again_says_at_every_hello_the_stays_its_state_has_in(
    world: World,
) {
    let mut whole = whole(world);
    let a_since = since(&whole.to_a.log);
    let (n, as_epoch, part) = whole.split(vec![FAR]);
    let split_at = whole.a.region().tick_number();
    // The edge reads the entry of the split and does not get to confirm it.
    let mut again = greeted(&mut whole.a, greeting(E, 5).since(a_since).seen(1));
    assert_eq!(entries(&again.log).len(), 1);
    assert_presence_is_the_state(&whole.a, &again);
    assert_eq!(presences(&again.log), vec![(player(1), Some(whole.one))]);

    // The part runs, and its player walks a step and breaks a block.
    let mut part_runner = whole.world.run_part(n, as_epoch, part);
    let mut to_n = greeted(&mut part_runner, greeting(E, 5).chunks(vec![FAR]));
    assert_presence_is_the_state(&part_runner, &to_n);
    let block = FAR_BLOCK.offset(0, 0, 1);
    to_n.next(input(player(2), whole.two, 3, dig(block, 2)));
    let walked = to_n.next(input(player(2), whole.two, 4, move_to(FAR_X + 1.0)));
    wait_applied(&mut part_runner, &mut [&mut to_n], 0, walked);
    sync(&mut part_runner, &mut [&mut to_n], 0);

    // The part's worker releases it, and region 0 absorbs it.
    part_runner.begin_release();
    assert_eq!(
        run_to_the_end(&mut part_runner, &mut [&mut to_n]),
        Ended::Released
    );
    let absorbed = whole.world.open_to_absorb(n);
    let outcome = ask(&mut whole.a, absorbed.order());
    let reshaped = run_to_outcome(&mut whole.a, &mut [&mut again], &outcome);
    assert_eq!(reshaped, Reshaped::Absorbed { absorbed: n });
    drop(absorbed);
    assert!(again.closed);
    let list = whole.world.list();
    assert_eq!(list.absorbed, vec![(n, A)]);
    assert_eq!(list.regions.len(), 2);
    assert_eq!(list.next, RegionId(n.0 + 1));
    // The chunks are the region's again, by the grants that moved.
    for chunk in [FAR, FARTHER] {
        assert_eq!(whole.a.region().knowledge(chunk), Knowledge::Held);
    }

    let last = greeted(
        &mut whole.a,
        greeting(E, 5)
            .since(a_since)
            .seen(1)
            .chunks(vec![HOME, FAR]),
    );
    assert_eq!(
        entries(&last.log),
        vec![
            (
                2,
                Durable::SplitOff {
                    region: n,
                    players: vec![(player(2), whole.two)],
                }
            ),
            (
                3,
                Durable::Absorbed {
                    region: n,
                    since: split_at,
                    applied: to_n.sent,
                    numbers: Vec::new(),
                }
            ),
        ]
    );
    assert_presence_is_the_state(&whole.a, &last);
    assert_eq!(
        presences(&last.log),
        vec![(player(1), Some(whole.one)), (player(2), Some(whole.two))]
    );
    assert!(
        matches!(
            presence(&last.log, player(2)),
            Some(Presence::Present { pose, last_input: 4, handled: Some(2), .. })
                if pose.position.x == FAR_X + 1.0
        ),
        "{}",
        brief(&last.log)
    );
    let (_, chunk, entities) = snapshot(&last.log, FAR, 0).expect("the chunk is served");
    assert_eq!(block_in(chunk, block), blocks::AIR);
    assert_eq!(block_in(chunk, FAR_BLOCK), blocks::AIR);
    assert!(entities.iter().any(|state| state.entity == whole.two));
}

// ---------------------------------------------------------------------------------------
// Beyond the list: what the store answers while a region stands still, an edge that
// stays away, and a merge behind a merge
// ---------------------------------------------------------------------------------------

/// Section 3.2: "every claim is answered", and the answers wait in the coming tick's
/// inputs. An asking that the last tick before the stop took is answered by the store
/// while the region stands still. If the merge is taken, a chunk granted so is the
/// region's by the sim as by the store; if it is off, the next tick takes the answer
/// and the link is served: no answer is dropped by a runner that will tick again.
#[test]
fn a_claim_that_is_answered_while_the_region_stands_still_is_not_lost() {
    for (on_disk, declined) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut pair = pair(World::new(Shape::Stripes, on_disk));
        let absorbed = pair.release_b();
        let order = absorbed.order();
        if declined {
            drop(absorbed);
            pair.world.open_raw(B);
        }
        // The asking is taken by a tick, and the order comes right behind it.
        let asked = pair.to_a.subscribe(vec![BACK, SECOND]);
        one_tick(&mut pair.a, &mut [&mut pair.to_a]);
        assert_ne!(pair.a.region().knowledge(SECOND), Knowledge::Unknown);
        let outcome = ask(&mut pair.a, order);
        let reshaped = run_to_outcome(&mut pair.a, &mut [&mut pair.to_a], &outcome);
        if declined {
            assert!(
                matches!(
                    reshaped,
                    Reshaped::Off {
                        why: Off::Declined(_)
                    }
                ),
                "{reshaped:?}"
            );
            for chunk in [BACK, SECOND] {
                assert_eq!(
                    wait_answer(&mut pair.a, &mut [&mut pair.to_a], 0, chunk, asked),
                    Answer::Snapshot
                );
            }
        } else {
            assert_eq!(reshaped, Reshaped::Absorbed { absorbed: B });
            // The store has granted both; the region was told, by a tick before the
            // stop or by the tick of the merge.
            for chunk in [BACK, SECOND] {
                assert_eq!(
                    pair.a.region().knowledge(chunk),
                    Knowledge::Held,
                    "{chunk:?}, on disk: {on_disk}"
                );
            }
            let again = greeted(
                &mut pair.a,
                hello_to_a(&pair.to_a).chunks(vec![HOME, BACK, SECOND]),
            );
            assert_eq!(again.answers(SECOND), vec![(0, Answer::Snapshot)]);
        }
    }
}

/// Section 3.3, item 3: what the runner keeps for edges is made anew as for a
/// restored region, and every edge the state knows is away since the tick of the
/// merge. An edge that had been away for most of its time before the merge has all of
/// it again afterwards, and is forgotten with its players when that has passed.
#[test]
fn after_a_merge_an_edge_has_as_long_as_after_a_restore_to_come_back() {
    const GONE_AFTER: u64 = 200;
    let mut world = World::stripes();
    let mut a = world.open(A).with_gone_after(GONE_AFTER);
    let mut b = world.open(B);
    let mut to_a = greeted(&mut a, greeting(E, 5).chunks(vec![HOME]));
    let mut f = greeted(&mut a, greeting(F, 5).chunks(vec![HOME]));
    join_and_wait(&mut a, &mut f, player(4));
    settle(&mut a, &mut [&mut to_a, &mut f]);
    drop(f);
    ticks(&mut a, &mut [&mut to_a], GONE_AFTER / 2);
    assert_eq!(a.region().player_count(), 1);

    b.begin_release();
    assert_eq!(run_to_the_end(&mut b, &mut []), Ended::Released);
    let absorbed = world.open_to_absorb(B);
    let outcome = ask(&mut a, absorbed.order());
    let reshaped = run_to_outcome(&mut a, &mut [&mut to_a], &outcome);
    assert_eq!(reshaped, Reshaped::Absorbed { absorbed: B });
    drop(absorbed);
    let m = a.region().tick_number();

    // Edge E comes back at once and stays; edge F does not.
    let mut again = greeted(
        &mut a,
        greeting(E, 5).since(since(&to_a.log)).chunks(vec![HOME]),
    );
    while a.region().tick_number() < m + GONE_AFTER - 5 {
        one_tick(&mut a, &mut [&mut again]);
    }
    assert!(a.region().player(player(4)).is_some());
    assert!(a.region().edge(F).is_some());
    while a.region().tick_number() < m + GONE_AFTER + 5 {
        one_tick(&mut a, &mut [&mut again]);
    }
    assert!(a.region().player(player(4)).is_none());
    assert_eq!(a.region().edge(F), None);
    assert!(a.region().edge(E).is_some());
}

/// A2 of the record's list for an edge, on the runner: region 2 absorbs region 0, and
/// the home region then absorbs region 2, with no link in between. The home region's
/// welcome has the entry for region 2 and, among that region's entries behind it, the
/// entry for region 0 with that region's entries behind that; the numbers of each are
/// those its entries had where they came from; and the presence answers have the
/// stays of all three.
#[test]
fn a_merge_behind_a_merge_is_told_as_an_entry_among_the_entries_of_the_first() {
    in_memory_and_on_disk(
        Shape::Three,
        a_merge_behind_a_merge_is_told_as_an_entry_among_the_entries_of_the_first_in,
    );
}

fn a_merge_behind_a_merge_is_told_as_an_entry_among_the_entries_of_the_first_in(mut world: World) {
    const WESTERNER: EntityId = EntityId(900_000_010);
    const EASTERNER: EntityId = EntityId(900_000_020);
    let mut home = world.open(THREE_HOME);
    let mut east = world.open(THREE_EAST);
    let mut west = world.open(THREE_WEST);
    let mut to_home = greeted(&mut home, greeting(E, 5).chunks(vec![HOME]));
    let one = join_and_wait(&mut home, &mut to_home, player(1));
    sync(&mut home, &mut [&mut to_home], 0);

    // A player in each of the outer stripes, who came from a region these tests do
    // not have and has dug into the home stripe: one entry of each region.
    let western = BlockPos::new(0, GROUND, 8);
    let mut to_west = greeted(&mut west, greeting(E, 5).chunks(vec![BACK]));
    to_west.next(arrive(player(2), &transfer(WESTERNER, -1.5)));
    let dug = to_west.next(input(player(2), WESTERNER, 1, dig(western, 1)));
    wait_applied(&mut west, &mut [&mut to_west], 0, dug);
    sync(&mut west, &mut [&mut to_west], 0);
    let eastern = BlockPos::new(63, GROUND, 8);
    let mut to_east = greeted(&mut east, greeting(E, 5).chunks(vec![ChunkPos::new(4, 0)]));
    to_east.next(arrive(player(3), &transfer(EASTERNER, 64.5)));
    let dug = to_east.next(input(player(3), EASTERNER, 1, dig(eastern, 1)));
    wait_applied(&mut east, &mut [&mut to_east], 0, dug);
    sync(&mut east, &mut [&mut to_east], 0);
    for link in [&to_west, &to_east] {
        assert!(
            matches!(
                outbox(&link.log).as_slice(),
                [(_, 1, Durable::Remote { .. })]
            ),
            "{}",
            brief(&link.log)
        );
    }
    let (west_since, east_since) = (since(&to_west.log), since(&to_east.log));

    // Region 2 absorbs region 0.
    west.begin_release();
    assert_eq!(
        run_to_the_end(&mut west, &mut [&mut to_west]),
        Ended::Released
    );
    let absorbed = world.open_to_absorb(THREE_WEST);
    let outcome = ask(&mut east, absorbed.order());
    assert_eq!(
        run_to_outcome(&mut east, &mut [&mut to_east], &outcome),
        Reshaped::Absorbed {
            absorbed: THREE_WEST
        }
    );
    drop(absorbed);
    // The home region absorbs region 2.
    east.begin_release();
    assert_eq!(run_to_the_end(&mut east, &mut []), Ended::Released);
    let absorbed = world.open_to_absorb(THREE_EAST);
    let outcome = ask(&mut home, absorbed.order());
    assert_eq!(
        run_to_outcome(&mut home, &mut [&mut to_home], &outcome),
        Reshaped::Absorbed {
            absorbed: THREE_EAST
        }
    );
    drop(absorbed);

    let list = world.list();
    assert_eq!(
        list.absorbed,
        vec![(THREE_WEST, THREE_EAST), (THREE_EAST, THREE_HOME)]
    );
    assert_eq!(list.regions.len(), 1);
    assert_eq!(list.regions[0].pinned.len(), 3);

    let again = greeted(
        &mut home,
        greeting(E, 5)
            .since(since(&to_home.log))
            .players(vec![player(1)])
            .chunks(vec![HOME]),
    );
    let log = &again.log;
    assert_eq!(
        entries(log),
        vec![
            (
                1,
                Durable::Absorbed {
                    region: THREE_EAST,
                    since: east_since,
                    applied: to_east.sent,
                    numbers: vec![1, 2, 3],
                }
            ),
            (
                2,
                Durable::Remote {
                    action: breaking(player(3), 1, eastern),
                    to: None,
                }
            ),
            (
                3,
                Durable::Absorbed {
                    region: THREE_WEST,
                    since: west_since,
                    applied: to_west.sent,
                    numbers: vec![1],
                }
            ),
            (
                4,
                Durable::Remote {
                    action: breaking(player(2), 1, western),
                    to: None,
                }
            ),
        ],
        "{}",
        brief(log)
    );
    assert_eq!(
        presences(log),
        vec![
            (player(1), Some(one)),
            (player(2), Some(WESTERNER)),
            (player(3), Some(EASTERNER)),
        ]
    );
    // Every chunk is the one region's now.
    for chunk in [BACK, ChunkPos::new(4, 0), ChunkPos::new(-9, 5)] {
        assert!(home.region().pins(chunk));
    }
}

// ---------------------------------------------------------------------------------------
// The differential of section 2.6, on the runner: a region after a merge or a split,
// and a part, give the same answers as the region restored from the store at that tick
// ---------------------------------------------------------------------------------------

/// What a link was told, without what depends on the pace of the store: in which
/// tick a thing happened, how the ticks' messages fall between each other, and the
/// ticks a welcome and an entry name, which are said relative to the tick of the
/// merge or the split, or left out.
#[derive(Debug, PartialEq)]
struct Told {
    welcome: Welcome,
    entries: Vec<(u64, Durable)>,
    presences: Vec<(PlayerId, Presence)>,
    answers: BTreeMap<ChunkPos, Vec<(u64, Answer)>>,
    events: Vec<RegionEvent>,
    later: Vec<(u64, Durable)>,
    to_players: Vec<(PlayerId, PlayerEvent)>,
    applied: Option<u64>,
}

/// An entry without the tick it names, if it names one.
fn timeless_entry(entry: &Durable) -> Durable {
    match entry {
        Durable::Absorbed {
            region,
            applied,
            numbers,
            ..
        } => Durable::Absorbed {
            region: *region,
            since: 0,
            applied: *applied,
            numbers: numbers.clone(),
        },
        other => other.clone(),
    }
}

/// [`Told`] of a link, `m` being the tick of the merge or the split before it.
fn told(link: &Link, m: u64) -> Told {
    let log = &link.log;
    let welcome = match welcomed(log) {
        Welcome::Unknown {
            since,
            entries,
            presences,
            applied,
        } => Welcome::Unknown {
            since: since - m,
            entries,
            presences,
            applied,
        },
        other => other,
    };
    let announced = entries(log);
    Told {
        welcome,
        entries: announced
            .iter()
            .map(|(number, entry)| (*number, timeless_entry(entry)))
            .collect(),
        presences: log
            .iter()
            .filter_map(|message| match message {
                WorkerToEdge::Presence { player, answer } => Some((*player, answer.clone())),
                _ => None,
            })
            .collect(),
        answers: link.book.answered.clone(),
        events: events(log)
            .into_iter()
            .map(|(_, _, event)| event.clone())
            .collect(),
        later: outbox(log)
            .into_iter()
            .skip(announced.len())
            .map(|(_, number, entry)| (number, timeless_entry(entry)))
            .collect(),
        to_players: log
            .iter()
            .filter_map(|message| match message {
                WorkerToEdge::ToPlayer { player, event } => Some((*player, event.clone())),
                _ => None,
            })
            .collect(),
        applied: applied(log),
    }
}

/// The snapshots a link was sent: the content of each chunk and who was in it.
fn shown(link: &Link) -> BTreeMap<ChunkPos, (Chunk, Vec<EntityState>)> {
    link.log
        .iter()
        .filter_map(|message| match message {
            WorkerToEdge::ChunkSnapshot {
                position,
                chunk,
                entities,
                ..
            } => Some((*position, (chunk.clone(), entities.clone()))),
            _ => None,
        })
        .collect()
}

/// A state without its ticks: who is there, and what the region has for each edge.
type Timeless = (
    Vec<(PlayerId, clustine_sim::PlayerState)>,
    Vec<(EdgeId, u64, u64, u64, Vec<(u64, Durable)>)>,
);

fn timeless(runner: &RegionRunner) -> Timeless {
    let state = runner.region().state();
    (
        state.players.into_iter().collect(),
        state
            .edges
            .iter()
            .map(|(edge, has)| {
                (
                    *edge,
                    has.start,
                    has.applied,
                    has.sent,
                    has.outbox
                        .iter()
                        .map(|(number, entry)| (*number, timeless_entry(entry)))
                        .collect(),
                )
            })
            .collect(),
    )
}

/// Everything one run of a differential scenario gives to compare with the other.
struct Run {
    told: Vec<(&'static str, Told)>,
    shown: Vec<BTreeMap<ChunkPos, (Chunk, Vec<EntityState>)>>,
    states: Vec<Timeless>,
}

fn assert_the_same(live: &Run, restored: &Run) {
    for ((name, one), (_, other)) in live.told.iter().zip(&restored.told) {
        assert_eq!(one, other, "{name}: the region that lived is on the left");
    }
    assert!(
        live.shown == restored.shown,
        "the snapshots of {:?} and of {:?} differ",
        live.shown
            .iter()
            .map(|shown| shown.keys().collect::<Vec<_>>())
            .collect::<Vec<_>>(),
        restored
            .shown
            .iter()
            .map(|shown| shown.keys().collect::<Vec<_>>())
            .collect::<Vec<_>>()
    );
    assert_eq!(live.states, restored.states);
}

/// A merge, and then one link that resumes, asks for chunks, and passes on what
/// players of both regions do, a join and a leave. With `crash`, the survivor is
/// opened by a new owner right after the merge.
fn a_link_after_a_merge(world: World, crash: bool) -> Run {
    let mut pair = pair(world);
    let m = pair.merge();
    if crash {
        let (handle, restored) = pair.world.open_raw(A);
        pair.a = RegionRunner::restore(config(RETURN_AFTER), handle, restored)
            .expect("what the store has is readable");
    }
    let mut again = Link::attach(&pair.a).after(pair.to_a.sent);
    again.hello(hello_to_a(&pair.to_a));
    let guest = again.subscribe_as_guest(vec![OTHER]);
    again.next(input(player(1), pair.one, 1, move_to(12.5)));
    again.next(input(player(2), pair.two, 4, dig(BEYOND, 2)));
    again.next(input(player(2), pair.two, 5, move_to(18.5)));
    again.next(input(player(1), pair.one, 2, dig(NEAR, 1)));
    again.next(join(player(3)));
    let last = again.next(leave(player(1), Some(pair.one)));
    wait_applied(&mut pair.a, &mut [&mut again], 0, last);
    wait_answer(&mut pair.a, &mut [&mut again], 0, OTHER, guest);
    sync(&mut pair.a, &mut [&mut again], 0);
    assert_eq!(block_of(&pair.a, BEYOND), Some(blocks::AIR));
    assert_eq!(pair.a.region().player_count(), 2);
    Run {
        told: vec![("the survivor", told(&again, m))],
        shown: vec![shown(&again)],
        states: vec![timeless(&pair.a)],
    }
}

#[test]
fn a_region_that_has_merged_answers_its_links_as_the_region_restored_at_that_tick_does() {
    for on_disk in [false, true] {
        let live = a_link_after_a_merge(World::new(Shape::Stripes, on_disk), false);
        let restored = a_link_after_a_merge(World::new(Shape::Stripes, on_disk), true);
        assert_the_same(&live, &restored);
    }
}

/// A split, and then a link to each of the two regions that resumes and passes on
/// what its players do. With `crash`, the part in memory is dropped, and both regions
/// are opened by new owners and restored from the store's record.
fn links_after_a_split(world: World, crash: bool) -> Run {
    let mut whole = whole(world);
    let (n, as_epoch, part) = whole.split(vec![FAR]);
    let m = whole.a.region().tick_number();
    let mut part_runner = if crash {
        drop(part);
        let (handle, restored) = whole.world.open_raw(A);
        whole.a = RegionRunner::restore(config(RETURN_AFTER), handle, restored)
            .expect("what the store has is readable");
        let (handle, restored) = whole.world.open_raw(n);
        RegionRunner::restore(config(RETURN_AFTER), handle, restored)
            .expect("what the store has is readable")
    } else {
        whole.world.run_part(n, as_epoch, part)
    };

    let mut again = Link::attach(&whole.a).after(whole.to_a.sent);
    again.hello(
        greeting(E, 5)
            .since(since(&whole.to_a.log))
            .seen(1)
            .players(vec![player(1), player(2)])
            .chunks(vec![HOME, BACK, FAR])
            .guests(vec![FARTHER]),
    );
    again.next(input(player(1), whole.one, 2, move_to(12.5)));
    // An input of the stay that went, which the edge sends again as it had kept it.
    again.next(input(player(2), whole.two, 3, move_to(FAR_X - 1.0)));
    let last = again.next(input(player(1), whole.one, 3, dig(NEAR, 2)));
    wait_applied(&mut whole.a, &mut [&mut again], 0, last);
    sync(&mut whole.a, &mut [&mut again], 0);
    assert_eq!(block_of(&whole.a, NEAR), Some(blocks::AIR));

    let mut to_n = Link::attach(&part_runner);
    to_n.hello(
        greeting(E, 5)
            .players(vec![player(2)])
            .chunks(vec![FAR, FARTHER, BACK]),
    );
    let block = FAR_BLOCK.offset(0, 0, 1);
    to_n.next(input(player(2), whole.two, 3, move_to(FAR_X - 1.0)));
    to_n.next(input(player(2), whole.two, 4, dig(block, 2)));
    let last = to_n.next(join(player(7)));
    wait_applied(&mut part_runner, &mut [&mut to_n], 0, last);
    sync(&mut part_runner, &mut [&mut to_n], 0);
    assert_eq!(block_of(&part_runner, block), Some(blocks::AIR));
    assert_eq!(to_n.answers(BACK), vec![(0, Answer::Elsewhere(A))]);

    Run {
        told: vec![
            ("the region that was split", told(&again, m)),
            ("the part", told(&to_n, m)),
        ],
        shown: vec![shown(&again), shown(&to_n)],
        states: vec![timeless(&whole.a), timeless(&part_runner)],
    }
}

/// R42, and the differential for a split.
#[test]
fn a_split_region_and_its_part_answer_their_links_as_the_regions_restored_from_the_record_do() {
    for on_disk in [false, true] {
        let live = links_after_a_split(World::new(Shape::Stripes, on_disk), false);
        let restored = links_after_a_split(World::new(Shape::Stripes, on_disk), true);
        assert_the_same(&live, &restored);
    }
}

// ---------------------------------------------------------------------------------------
// K1 to K12. Every stage and every death
//
// A scenario has an edge of its own, which keeps what an edge keeps and does with a
// welcome, its entries and its presence answers what section 8 says; the regions and
// the store are the real ones. After the death every region the store's list has is
// opened with a higher epoch, and the world is held to the tables of sections 6 and 7:
// as before or as after, as a whole; every player in exactly one region's state, with
// what they were told done and nothing more; and when the edge has resumed with every
// region and sent what it kept, every player is where the run without a death has them.
// ---------------------------------------------------------------------------------------

impl Link {
    /// Sends on a link the worker may have closed by now. What is lost with the link,
    /// the edge has kept.
    fn offer(&mut self, number: Option<u64>, body: EdgeToWorker) {
        self.book.sent = true;
        let _ = self.end.try_send(EdgeMessage { number, body });
    }
}

/// What an edge keeps for a region (ADR-0008, section 5, and ADR-0015, section 1, as
/// far as walking players need it).
#[derive(Default)]
struct Port {
    link: Option<Link>,
    /// How much of the link's log has been handled.
    read: usize,
    /// Whether the link's welcome has been read, and whether its entries and presence
    /// answers have all been handled, so that numbered messages go out (rule 3).
    welcomed: bool,
    open: bool,
    entries_left: usize,
    answers_left: usize,
    /// The players this welcome has said `Present` for, and those an `Absorbed` among
    /// its entries made this region's (rule 38, last item).
    present: BTreeSet<PlayerId>,
    brought: Vec<PlayerId>,
    /// The numbers of entries behind an `Absorbed` that the edge had seen at the
    /// absorbed region (rule 42, step 6).
    seen_there: BTreeSet<u64>,
    /// The `since` of the last welcome, the last entry seen, and whether the edge has
    /// ever had an entry or a message reported applied from the region.
    since: u64,
    seen: u64,
    heard: bool,
    /// The number of the last numbered message made for the region, and those not yet
    /// reported applied.
    sent: u64,
    kept: Vec<(u64, EdgeToWorker)>,
}

/// A player's stay as an edge has it.
struct Stay {
    region: RegionId,
    entity: Option<EntityId>,
    /// The number of the last input made, and the inputs no region has reported
    /// applied.
    made: u64,
    inputs: Vec<(u64, PlayerInput)>,
    /// The chunks the player's viewer sees.
    view: Vec<ChunkPos>,
}

struct Edge {
    id: EdgeId,
    start: u64,
    ports: BTreeMap<RegionId, Port>,
    players: BTreeMap<PlayerId, Stay>,
    /// The regions that were absorbed, each with the region it stands for (rule 39).
    stands_for: BTreeMap<RegionId, RegionId>,
    /// The players the edge gave up: a test fails on any.
    lost: Vec<PlayerId>,
}

impl Edge {
    fn new(id: EdgeId, start: u64) -> Self {
        Self {
            id,
            start,
            ports: BTreeMap::new(),
            players: BTreeMap::new(),
            stands_for: BTreeMap::new(),
            lost: Vec::new(),
        }
    }

    /// The region a name means: itself, or the region it went into, once the edge has
    /// handled that `Absorbed`.
    fn living(&self, region: RegionId) -> RegionId {
        self.stands_for.get(&region).copied().unwrap_or(region)
    }

    fn port(&mut self, region: RegionId) -> &mut Port {
        self.ports
            .get_mut(&region)
            .expect("the edge has a port for the region")
    }

    /// Makes a numbered message for `region`. It is kept until the region reports it
    /// applied, and sent at once if the link's welcome is through.
    fn tell(&mut self, region: RegionId, body: EdgeToWorker) {
        let region = self.living(region);
        let port = self.ports.entry(region).or_default();
        port.sent += 1;
        port.kept.push((port.sent, body.clone()));
        if port.open {
            if let Some(link) = port.link.as_mut() {
                link.offer(Some(port.sent), body);
            }
        }
    }

    /// Asks `region` for a view that has come to it, on the link there is. Without
    /// one, the next hello names it.
    fn look(&mut self, region: RegionId, view: Vec<ChunkPos>) {
        let Some(port) = self.ports.get_mut(&region) else {
            return;
        };
        if let Some(link) = port.link.as_mut() {
            if port.welcomed && !link.closed && !view.is_empty() {
                link.subscribe(view);
            }
        }
    }

    fn join(&mut self, id: PlayerId, home: RegionId, view: Vec<ChunkPos>) {
        self.players.insert(
            id,
            Stay {
                region: home,
                entity: None,
                made: 0,
                inputs: Vec::new(),
                view: view.clone(),
            },
        );
        self.tell(home, join(id));
        self.look(home, view);
    }

    /// A step of `id` to `x`. It goes to the region the edge has the player under, once
    /// the player has been told their entity.
    fn walk(&mut self, id: PlayerId, x: f64) {
        let stay = self.players.get_mut(&id).expect("the edge has the player");
        stay.made += 1;
        let (number, step) = (stay.made, move_to(x));
        stay.inputs.push((number, step.clone()));
        if let Some(entity) = stay.entity {
            let region = stay.region;
            self.tell(region, input(id, entity, number, step));
        }
    }

    /// Makes a new link to `region` and says hello: the players it has under that
    /// region and what their viewers see (rules 2 and 43).
    fn link(&mut self, region: RegionId, runner: &RegionRunner) {
        let mine: Vec<(&PlayerId, &Stay)> = self
            .players
            .iter()
            .filter(|(_, stay)| stay.region == region)
            .collect();
        let players = mine.iter().map(|(id, _)| **id).collect();
        let chunks: BTreeSet<ChunkPos> = mine
            .iter()
            .flat_map(|(_, stay)| stay.view.iter().copied())
            .collect();
        let hello = greeting(self.id, self.start)
            .players(players)
            .chunks(chunks.into_iter().collect());
        let port = self.ports.entry(region).or_default();
        let mut link = Link::attach(runner);
        link.hello(hello.since(port.since).seen(port.seen));
        port.link = Some(link);
        port.read = 0;
        port.welcomed = false;
        port.open = false;
        port.present.clear();
        port.brought.clear();
    }

    /// Reads what has arrived on every link and does with it what an edge does.
    fn read(&mut self) {
        let regions: Vec<RegionId> = self.ports.keys().copied().collect();
        for region in regions {
            // An entry of another region can have retired this port meanwhile.
            while let Some(port) = self.ports.get_mut(&region) {
                let Some(link) = port.link.as_mut() else {
                    break;
                };
                link.drain();
                let Some(message) = link.log.get(port.read).cloned() else {
                    if link.closed {
                        port.link = None;
                        port.welcomed = false;
                        port.open = false;
                    }
                    break;
                };
                port.read += 1;
                self.handle(region, message);
            }
        }
    }

    fn handle(&mut self, region: RegionId, message: WorkerToEdge) {
        match message {
            WorkerToEdge::Welcome(welcome) => {
                let port = self.port(region);
                let (entries, answers) = match welcome {
                    Welcome::Resumed {
                        entries, presences, ..
                    } => (entries, presences),
                    Welcome::Unknown {
                        since,
                        entries,
                        presences,
                        ..
                    } => {
                        if port.heard {
                            // The region has forgotten the edge: what was kept for
                            // it is given up (ADR-0008, section 5).
                            port.kept.clear();
                            port.sent = 0;
                        }
                        port.since = since;
                        port.seen = 0;
                        port.heard = false;
                        (entries, presences)
                    }
                    Welcome::Superseded => panic!("the edge was told it has been replaced"),
                };
                port.welcomed = true;
                port.entries_left = entries as usize;
                port.answers_left = answers as usize;
            }
            WorkerToEdge::Outbox { number, entry } => {
                let port = self.port(region);
                port.entries_left = port.entries_left.saturating_sub(1);
                let new = number > port.seen && !port.seen_there.remove(&number);
                port.seen = port.seen.max(number);
                port.heard = true;
                if let Some(link) = port.link.as_mut() {
                    link.offer(None, EdgeToWorker::Confirm { number });
                }
                if new {
                    self.entry(region, number, entry);
                }
            }
            WorkerToEdge::Presence { player, answer } => {
                let port = self.port(region);
                port.answers_left = port.answers_left.saturating_sub(1);
                self.presence(region, player, answer);
            }
            WorkerToEdge::Progress { applied, inputs } => {
                let port = self.port(region);
                port.kept.retain(|(number, _)| *number > applied);
                port.heard |= applied > 0;
                for (player, last) in inputs {
                    if let Some(stay) = self.players.get_mut(&player) {
                        if stay.region == region {
                            stay.inputs.retain(|(number, _)| *number > last);
                        }
                    }
                }
            }
            WorkerToEdge::ToPlayer {
                player,
                event: PlayerEvent::Spawned { entity_id, .. },
            } => {
                let Some(stay) = self.players.get_mut(&player) else {
                    return;
                };
                if stay.entity.is_none() && stay.region == region {
                    stay.entity = Some(entity_id);
                    for (number, step) in stay.inputs.clone() {
                        self.tell(region, input(player, entity_id, number, step));
                    }
                }
            }
            _ => {}
        }
        self.through(region);
    }

    /// When a welcome's entries and presence answers have all been handled: the edge
    /// judges whom the welcome brought and said nothing of, and sends what it kept.
    fn through(&mut self, region: RegionId) {
        let Some(port) = self.ports.get_mut(&region) else {
            return;
        };
        if port.open || !port.welcomed || port.entries_left > 0 || port.answers_left > 0 {
            return;
        }
        let absent: Vec<PlayerId> = port
            .brought
            .drain(..)
            .filter(|player| !port.present.contains(player))
            .collect();
        port.present.clear();
        port.open = true;
        if let Some(link) = port.link.as_mut() {
            for (number, body) in port.kept.clone() {
                link.offer(Some(number), body);
            }
        }
        for player in absent {
            self.players.remove(&player);
            self.lost.push(player);
        }
    }

    fn entry(&mut self, region: RegionId, number: u64, entry: Durable) {
        match entry {
            // Rule 18.
            Durable::Departed {
                player,
                transfer,
                to,
            } => {
                let to = self.living(to);
                let Some(stay) = self
                    .players
                    .get_mut(&player)
                    .filter(|stay| stay.entity == Some(transfer.entity_id))
                else {
                    let position = transfer.pose.position;
                    self.tell(
                        to,
                        EdgeToWorker::Discard {
                            entity: transfer.entity_id,
                            chunk: ChunkPos::containing(position.x, position.z),
                        },
                    );
                    return;
                };
                stay.region = to;
                stay.inputs
                    .retain(|(number, _)| *number > transfer.last_input);
                let (inputs, view) = (stay.inputs.clone(), stay.view.clone());
                self.tell(to, arrive(player, &transfer));
                for (number, step) in inputs {
                    self.tell(to, input(player, transfer.entity_id, number, step));
                }
                self.look(to, view);
            }
            // Rule 42, in its order.
            Durable::Absorbed {
                region: gone,
                since,
                applied,
                numbers,
            } => {
                for target in self.stands_for.values_mut() {
                    if *target == gone {
                        *target = region;
                    }
                }
                self.stands_for.insert(gone, region);
                let theirs = self.ports.remove(&gone).unwrap_or_default();
                let shared = since != 0 && since == theirs.since;
                let seen_there = theirs.seen;
                let kept: Vec<EdgeToWorker> = if shared {
                    theirs
                        .kept
                        .into_iter()
                        .filter(|(number, _)| *number > applied)
                        .map(|(_, body)| body)
                        .collect()
                } else if theirs.heard {
                    // It had forgotten the edge, and what was kept for it is given up.
                    Vec::new()
                } else {
                    theirs.kept.into_iter().map(|(_, body)| body).collect()
                };
                let mut brought = Vec::new();
                let mut view = BTreeSet::new();
                for (id, stay) in &mut self.players {
                    if stay.region == gone {
                        stay.region = region;
                        brought.push(*id);
                        view.extend(stay.view.iter().copied());
                    }
                }
                self.look(region, view.into_iter().collect());
                for body in kept {
                    self.tell(region, body);
                }
                let port = self.port(region);
                port.brought.extend(brought);
                if shared {
                    for (behind, there) in numbers.iter().enumerate() {
                        if *there <= seen_there {
                            port.seen_there.insert(number + 1 + behind as u64);
                        }
                    }
                }
            }
            // Rule 45.
            Durable::SplitOff {
                region: part,
                players,
            } => {
                let part = self.living(part);
                for (player, entity) in players {
                    let coming_back = self.port(region).kept.iter().any(|(_, body)| {
                        matches!(body, EdgeToWorker::PlayerArrive { player: who, .. } if *who == player)
                    });
                    let Some(stay) = self.players.get_mut(&player) else {
                        continue;
                    };
                    if stay.entity != Some(entity) || stay.region != region || coming_back {
                        continue;
                    }
                    stay.region = part;
                    let (inputs, view) = (stay.inputs.clone(), stay.view.clone());
                    for (number, step) in inputs {
                        self.tell(part, input(player, entity, number, step));
                    }
                    self.look(part, view);
                }
            }
            _ => {}
        }
    }

    /// Rule 38.
    fn presence(&mut self, region: RegionId, player: PlayerId, answer: Presence) {
        match answer {
            Presence::Present {
                entity, last_input, ..
            } => {
                let Some(stay) = self
                    .players
                    .get_mut(&player)
                    .filter(|stay| stay.entity.is_none_or(|has| has == entity))
                else {
                    // A stay the edge does not have: that is the stay to end.
                    self.tell(region, leave(player, Some(entity)));
                    return;
                };
                stay.entity = Some(entity);
                stay.inputs.retain(|(number, _)| *number > last_input);
                let moved = stay.region != region;
                stay.region = region;
                let (inputs, view) = (stay.inputs.clone(), stay.view.clone());
                self.port(region).present.insert(player);
                if moved {
                    // The stay is this region's, without an arrival.
                    for (number, step) in inputs {
                        self.tell(region, input(player, entity, number, step));
                    }
                    self.look(region, view);
                }
            }
            Presence::Absent => {
                let here = self
                    .players
                    .get(&player)
                    .is_some_and(|stay| stay.region == region);
                let under_way = self.port(region).kept.iter().any(|(_, body)| match body {
                    EdgeToWorker::PlayerJoin(join) => join.player == player,
                    EdgeToWorker::PlayerArrive { player: who, .. } => *who == player,
                    _ => false,
                });
                if here && !under_way {
                    self.players.remove(&player);
                    self.lost.push(player);
                }
            }
        }
    }

    /// Whether every link's welcome is through, every region has reported applied
    /// what the edge sent it, and no input of a player is still kept.
    fn settled(&self) -> bool {
        self.ports
            .values()
            .all(|port| port.open && port.kept.is_empty())
            && self.players.values().all(|stay| stay.inputs.is_empty())
    }

    /// What the edge has, for a failure.
    fn describe(&self) -> String {
        let mut said = format!(
            "players: {:?}\nlost: {:?}\n",
            self.players
                .iter()
                .map(|(id, stay)| (id.0.as_u128(), stay.region, stay.entity, &stay.inputs))
                .collect::<Vec<_>>(),
            self.lost
        );
        for (region, port) in &self.ports {
            said += &format!(
                "{region:?}: open {}, since {}, seen {}, sent {}, kept {:?}\n{}",
                port.open,
                port.since,
                port.seen,
                port.sent,
                port.kept,
                port.link
                    .as_ref()
                    .map_or("no link\n".to_owned(), |link| brief(&link.log))
            );
        }
        said
    }
}

/// Steps every runner and has the edge read what came, until `done`.
fn play(
    edge: &mut Edge,
    runners: &mut [&mut RegionRunner],
    what: &str,
    mut done: impl FnMut(&Edge, &[&mut RegionRunner]) -> bool,
) {
    for _ in 0..STEPS {
        for runner in runners.iter_mut() {
            runner.step();
        }
        edge.read();
        if done(edge, runners) {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("never happened: {what}\n{}", edge.describe());
}

/// Where a scenario's worker dies.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kill {
    /// K1: after the absorbed region's release.
    Released,
    /// K2: after that region is opened with the epoch of the merge.
    Opened,
    /// K3, K4 and K6, K8, K9 and K11: the runner is dropped at the first step with
    /// this stage.
    At(Stage),
    /// K5 and K10: the region is opened by another owner at the first step with
    /// `Closing`, so that the runner's handle is lost before it can send the commit.
    Taken,
    /// K7 and K12: the runner is dropped at the first step after the outcome, before
    /// any link is taken up, and a part is never run.
    Done,
}

impl Kill {
    /// Whether the store has the record: sections 6 and 7 of the record.
    fn leaves_it_done(self) -> bool {
        matches!(self, Self::At(Stage::Committing) | Self::Done)
    }
}

/// The scenarios run on a store in memory and on one on disk, and the edge resumes
/// with the regions it finds in ascending order and in descending order, one after
/// the other: rule 50 says the order does not matter.
fn every_way(scenario: fn(bool, Kill, bool), kill: Kill) {
    for on_disk in [false, true] {
        for descending in [false, true] {
            scenario(on_disk, kill, descending);
        }
    }
}

/// Opens every region the store's list has with a higher epoch.
fn open_all(world: &mut World) -> Vec<(RegionId, RegionRunner)> {
    world
        .list()
        .regions
        .iter()
        .map(|info| info.region)
        .collect::<Vec<_>>()
        .into_iter()
        .map(|region| (region, world.open(region)))
        .collect()
}

/// The player `id` as the regions' states have them: exactly one has.
fn the_one(runners: &[(RegionId, RegionRunner)], id: PlayerId) -> (RegionId, u64, f64) {
    let found: Vec<(RegionId, u64, f64)> = runners
        .iter()
        .filter_map(|(region, runner)| {
            runner
                .region()
                .player_state(id)
                .map(|player| (*region, player.last_input, player.pose.position.x))
        })
        .collect();
    assert_eq!(found.len(), 1, "player {} is in {found:?}", id.0.as_u128());
    found[0]
}

/// Has the edge resume with every region, one after the other, and send what it
/// kept; then holds the world to where the players would be without a death.
fn resume_and_end(
    edge: &mut Edge,
    runners: &mut [(RegionId, RegionRunner)],
    descending: bool,
    ends: &[(PlayerId, RegionId, f64)],
) {
    let mut order: Vec<usize> = (0..runners.len()).collect();
    if descending {
        order.reverse();
    }
    for index in order {
        let region = runners[index].0;
        edge.link(region, &runners[index].1);
        let mut all: Vec<&mut RegionRunner> =
            runners.iter_mut().map(|(_, runner)| runner).collect();
        play(edge, &mut all, "a welcome being through", |edge, _| {
            edge.ports.get(&region).is_some_and(|port| port.open)
        });
    }
    let mut all: Vec<&mut RegionRunner> = runners.iter_mut().map(|(_, runner)| runner).collect();
    play(
        edge,
        &mut all,
        "everything the edge kept being applied",
        |edge, _| edge.settled(),
    );
    assert!(edge.lost.is_empty(), "{}", edge.describe());
    for (id, region, x) in ends {
        let (found, _, at) = the_one(runners, *id);
        assert_eq!((found, at), (*region, *x), "{}", edge.describe());
        assert_eq!(edge.players[id].region, *region, "{}", edge.describe());
    }
}

/// K1 to K7. Region 0 has player 1 and region 1 player 2, who walked over. Before the
/// stop each has been told a step; while the merge is under way each makes one more,
/// which no tick takes.
fn a_merge_with_a_death(on_disk: bool, kill: Kill, descending: bool) {
    let mut world = World::new(Shape::Stripes, on_disk);
    let mut a = world.open(A);
    let mut b = world.open(B);
    let mut edge = Edge::new(E, 5);
    edge.link(A, &a);
    edge.link(B, &b);
    play(
        &mut edge,
        &mut [&mut a, &mut b],
        "the first welcomes",
        |edge, _| edge.settled(),
    );
    edge.join(player(1), A, vec![HOME]);
    edge.join(player(2), A, vec![HOME, NEXT]);
    play(
        &mut edge,
        &mut [&mut a, &mut b],
        "the players entering the world",
        |edge, _| edge.players.values().all(|stay| stay.entity.is_some()),
    );
    edge.walk(player(1), 12.5);
    edge.walk(player(2), 16.5);
    edge.walk(player(2), 17.5);
    play(
        &mut edge,
        &mut [&mut a, &mut b],
        "the players' steps being applied",
        |edge, _| edge.settled(),
    );
    assert_eq!(edge.players[&player(2)].region, B);
    let before = world.list();

    // Region 1's worker releases it, and its player goes on walking.
    b.begin_release();
    play(
        &mut edge,
        &mut [&mut a, &mut b],
        "the release of region 1",
        |_, runners| runners[1].ended().is_some(),
    );
    assert_eq!(b.ended(), Some(Ended::Released));
    drop(b);
    edge.walk(player(2), 18.5);

    let mut held = None;
    let mut asked = None;
    if kill != Kill::Released {
        let absorbed = world.open_to_absorb(B);
        if kill != Kill::Opened {
            asked = Some(ask(&mut a, absorbed.order()));
        }
        held = Some(absorbed);
    }
    let lost = Some(Reshaped::Off {
        why: Off::StoreLost,
    });
    let mut other = None;
    match kill {
        Kill::Released | Kill::Opened => {}
        Kill::At(stage) => {
            play(
                &mut edge,
                &mut [&mut a],
                "the stage of the death",
                |_, runners| runners[0].stage() == Some(stage),
            );
        }
        Kill::Taken => {
            play(
                &mut edge,
                &mut [&mut a],
                "the runner closing",
                |_, runners| runners[0].stage() == Some(Stage::Closing),
            );
            other = Some(world.open_raw(A));
            let outcome = asked.take().expect("the merge was asked for");
            let mut said = None;
            play(&mut edge, &mut [&mut a], "the outcome", |_, _| {
                said = outcome.taken();
                said.is_some()
            });
            assert_eq!(said, lost);
            assert_eq!(a.stage(), None);
            assert!(a.store_is_lost());
        }
        Kill::Done => {
            let outcome = asked.take().expect("the merge was asked for");
            let mut said = None;
            play(&mut edge, &mut [&mut a], "the outcome", |_, _| {
                said = outcome.taken();
                said.is_some()
            });
            assert_eq!(said, Some(Reshaped::Absorbed { absorbed: B }));
        }
    }
    edge.walk(player(1), 11.5);
    // The worker dies: the runner and every handle it holds are dropped.
    drop(a);
    drop(held);
    drop(other);
    if let Some(outcome) = asked {
        assert_eq!(outcome.taken(), lost, "{kill:?}");
        assert_eq!(outcome.taken(), None);
    }

    // As before or as after, as a whole.
    let list = world.list();
    let done = kill.leaves_it_done();
    if done {
        assert_eq!(list.absorbed, vec![(B, A)], "{kill:?}");
        assert_eq!(list.regions.len(), 1);
        assert_eq!(list.regions[0].pinned, areas(Shape::Stripes));
    } else {
        assert!(list.absorbed.is_empty(), "{kill:?}");
        assert_eq!(list.regions.len(), 2);
        for (now, was) in list.regions.iter().zip(&before.regions) {
            assert_eq!(
                (now.region, &now.bounds, &now.pinned),
                (was.region, &was.bounds, &was.pinned)
            );
        }
    }
    let mut runners = open_all(&mut world);
    let has_the_entry = runners[0].1.region().state().edges[&E]
        .outbox
        .values()
        .any(|entry| matches!(entry, Durable::Absorbed { .. }));
    assert_eq!(has_the_entry, done, "{kill:?}");
    // Every player is in exactly one region's state, with what they were told done
    // and nothing more.
    let theirs = if done { A } else { B };
    assert_eq!(the_one(&runners, player(1)), (A, 1, 12.5), "{kill:?}");
    assert_eq!(the_one(&runners, player(2)), (theirs, 2, 17.5), "{kill:?}");

    resume_and_end(
        &mut edge,
        &mut runners,
        descending,
        &[(player(1), A, 11.5), (player(2), theirs, 18.5)],
    );
}

/// K1.
#[test]
fn a_merge_whose_worker_dies_after_the_absorbed_regions_release_leaves_the_world_as_before() {
    every_way(a_merge_with_a_death, Kill::Released);
}

/// K2.
#[test]
fn a_merge_whose_worker_dies_after_opening_the_absorbed_region_leaves_the_world_as_before() {
    every_way(a_merge_with_a_death, Kill::Opened);
}

/// K3.
#[test]
fn a_merge_whose_runner_is_dropped_while_it_settles_leaves_the_world_as_before() {
    every_way(a_merge_with_a_death, Kill::At(Stage::Settling));
}

/// K4.
#[test]
fn a_merge_whose_runner_is_dropped_while_it_closes_leaves_the_world_as_before() {
    every_way(a_merge_with_a_death, Kill::At(Stage::Closing));
}

/// K5.
#[test]
fn a_merge_whose_survivor_is_taken_by_another_owner_before_the_commit_is_off_with_the_store_lost() {
    every_way(a_merge_with_a_death, Kill::Taken);
}

/// K6: the commit was sent, and the store does what a handle asked before it closes
/// it.
#[test]
fn a_merge_whose_runner_is_dropped_once_the_commit_is_sent_has_happened() {
    every_way(a_merge_with_a_death, Kill::At(Stage::Committing));
}

/// K7.
#[test]
fn a_merge_whose_runner_is_dropped_right_after_the_outcome_has_happened() {
    every_way(a_merge_with_a_death, Kill::Done);
}

/// K8 to K12. Region 0 has player 1 at home and player 2 in [`FAR`], which is split
/// off. Before the stop each has been told a step; while the split is under way each
/// makes one more, which no tick takes.
fn a_split_with_a_death(on_disk: bool, kill: Kill, descending: bool) {
    let mut world = World::new(Shape::Stripes, on_disk);
    let mut a = world.open(A);
    let mut edge = Edge::new(E, 5);
    edge.link(A, &a);
    play(&mut edge, &mut [&mut a], "the first welcome", |edge, _| {
        edge.settled()
    });
    edge.join(player(1), A, vec![HOME]);
    edge.join(player(2), A, vec![FAR, FARTHER]);
    play(
        &mut edge,
        &mut [&mut a],
        "the players entering the world",
        |edge, _| edge.players.values().all(|stay| stay.entity.is_some()),
    );
    edge.walk(player(1), 12.5);
    edge.walk(player(2), FAR_X);
    play(
        &mut edge,
        &mut [&mut a],
        "the players' steps being applied",
        |edge, _| edge.settled(),
    );
    let before = world.list();
    let n = before.next;
    let as_epoch = world.next_epoch();

    let outcome = ask(
        &mut a,
        Reshape::SplitOff {
            chunks: vec![FAR],
            as_epoch,
            part: n,
        },
    );
    let lost = Some(Reshaped::Off {
        why: Off::StoreLost,
    });
    let mut other = None;
    let mut said = None;
    match kill {
        Kill::Released | Kill::Opened => panic!("{kill:?} is of a merge"),
        Kill::At(stage) => {
            play(
                &mut edge,
                &mut [&mut a],
                "the stage of the death",
                |_, runners| runners[0].stage() == Some(stage),
            );
        }
        Kill::Taken => {
            play(
                &mut edge,
                &mut [&mut a],
                "the runner closing",
                |_, runners| runners[0].stage() == Some(Stage::Closing),
            );
            other = Some(world.open_raw(A));
            play(&mut edge, &mut [&mut a], "the outcome", |_, _| {
                said = outcome.taken();
                said.is_some()
            });
            assert_eq!(said, lost);
            assert!(a.store_is_lost());
        }
        Kill::Done => {
            play(&mut edge, &mut [&mut a], "the outcome", |_, _| {
                said = outcome.taken();
                said.is_some()
            });
            assert!(
                matches!(&said, Some(Reshaped::Split { region, as_epoch: epoch, .. })
                    if *region == n && *epoch == as_epoch),
                "{said:?}"
            );
        }
    }
    edge.walk(player(1), 11.5);
    edge.walk(player(2), FAR_X - 1.0);
    // The worker dies, with the part in its memory if there is one.
    drop(a);
    drop(other);
    if said.take().is_none() {
        assert_eq!(outcome.taken(), lost, "{kill:?}");
    }
    assert_eq!(outcome.taken(), None);

    let list = world.list();
    let done = kill.leaves_it_done();
    assert!(list.absorbed.is_empty());
    // The two stripes are as they were, but for the epoch of another owner.
    for (now, was) in list.regions.iter().zip(&before.regions) {
        assert_eq!(
            (now.region, &now.bounds, &now.pinned),
            (was.region, &was.bounds, &was.pinned)
        );
    }
    if done {
        assert_eq!(list.regions.len(), 3, "{kill:?}");
        assert_eq!(list.next, RegionId(n.0 + 1));
        let new = &list.regions[2];
        assert_eq!((new.region, new.epoch), (n, as_epoch));
        assert_eq!(
            new.bounds.map(|bounds| (bounds.min, bounds.max)),
            Some((FARTHER, FAR))
        );
    } else {
        assert_eq!(list.regions.len(), 2, "{kill:?}");
        assert_eq!(list.next, n);
    }
    let mut runners = open_all(&mut world);
    let has_the_entry = runners[0].1.region().state().edges[&E]
        .outbox
        .values()
        .any(|entry| matches!(entry, Durable::SplitOff { .. }));
    assert_eq!(has_the_entry, done, "{kill:?}");
    let theirs = if done { n } else { A };
    assert_eq!(the_one(&runners, player(1)), (A, 1, 12.5), "{kill:?}");
    assert_eq!(the_one(&runners, player(2)), (theirs, 1, FAR_X), "{kill:?}");
    if done {
        // The part is restored from the record by whoever opens it, with its chunks.
        assert_eq!(runners[2].1.region().knowledge(FAR), Knowledge::Held);
        assert_eq!(runners[0].1.region().knowledge(FAR), Knowledge::Unknown);
    }

    resume_and_end(
        &mut edge,
        &mut runners,
        descending,
        &[(player(1), A, 11.5), (player(2), theirs, FAR_X - 1.0)],
    );
}

/// K8.
#[test]
fn a_split_whose_runner_is_dropped_while_it_settles_leaves_the_world_as_before() {
    every_way(a_split_with_a_death, Kill::At(Stage::Settling));
}

/// K9.
#[test]
fn a_split_whose_runner_is_dropped_while_it_closes_leaves_the_world_as_before() {
    every_way(a_split_with_a_death, Kill::At(Stage::Closing));
}

/// K10.
#[test]
fn a_split_whose_region_is_taken_by_another_owner_before_the_commit_is_off_with_the_store_lost() {
    every_way(a_split_with_a_death, Kill::Taken);
}

/// K11: the part is restored from the record by whoever opens it.
#[test]
fn a_split_whose_runner_is_dropped_once_the_commit_is_sent_has_happened() {
    every_way(a_split_with_a_death, Kill::At(Stage::Committing));
}

/// K12.
#[test]
fn a_split_whose_runner_is_dropped_after_the_outcome_with_the_part_never_run_has_happened() {
    every_way(a_split_with_a_death, Kill::Done);
}

/// The scenarios of the kills with nobody dying, so that what they hold a world to
/// after a death is what a world without one comes to.
#[test]
fn without_a_death_the_edge_of_the_kills_follows_a_merge_and_a_split_to_the_same_end() {
    for on_disk in [false, true] {
        // A merge.
        let mut world = World::new(Shape::Stripes, on_disk);
        let mut a = world.open(A);
        let mut b = world.open(B);
        let mut edge = Edge::new(E, 5);
        edge.link(A, &a);
        edge.link(B, &b);
        play(
            &mut edge,
            &mut [&mut a, &mut b],
            "the first welcomes",
            |edge, _| edge.settled(),
        );
        edge.join(player(1), A, vec![HOME]);
        edge.join(player(2), A, vec![HOME, NEXT]);
        play(
            &mut edge,
            &mut [&mut a, &mut b],
            "the players entering the world",
            |edge, _| edge.players.values().all(|stay| stay.entity.is_some()),
        );
        edge.walk(player(1), 12.5);
        edge.walk(player(2), 16.5);
        edge.walk(player(2), 17.5);
        play(&mut edge, &mut [&mut a, &mut b], "the steps", |edge, _| {
            edge.settled()
        });
        b.begin_release();
        play(
            &mut edge,
            &mut [&mut a, &mut b],
            "the release",
            |_, runners| runners[1].ended().is_some(),
        );
        drop(b);
        edge.walk(player(2), 18.5);
        let absorbed = world.open_to_absorb(B);
        let outcome = ask(&mut a, absorbed.order());
        play(
            &mut edge,
            &mut [&mut a],
            "the runner standing still",
            |_, runners| runners[0].stage() == Some(Stage::Settling),
        );
        edge.walk(player(1), 11.5);
        let mut said = None;
        play(&mut edge, &mut [&mut a], "the outcome", |_, _| {
            said = outcome.taken();
            said.is_some()
        });
        assert_eq!(said, Some(Reshaped::Absorbed { absorbed: B }));
        drop(absorbed);
        // The edge links again to the runner that lived.
        edge.read();
        edge.link(A, &a);
        play(&mut edge, &mut [&mut a], "the resume", |edge, _| {
            edge.settled()
        });
        assert!(edge.lost.is_empty(), "{}", edge.describe());
        assert_eq!(edge.ports.keys().collect::<Vec<_>>(), vec![&A]);
        let state = a.region().state();
        assert_eq!(state.players[&player(1)].pose.position.x, 11.5);
        assert_eq!(state.players[&player(2)].pose.position.x, 18.5);
        assert_eq!(edge.players[&player(2)].region, A);

        // And a split of what has merged: the player who came goes again.
        let n = world.list().next;
        let as_epoch = world.next_epoch();
        let outcome = ask(
            &mut a,
            Reshape::SplitOff {
                chunks: vec![NEXT],
                as_epoch,
                part: n,
            },
        );
        play(
            &mut edge,
            &mut [&mut a],
            "the runner standing still",
            |_, runners| runners[0].stage() == Some(Stage::Settling),
        );
        edge.walk(player(1), 10.5);
        edge.walk(player(2), 19.5);
        let mut said = None;
        play(&mut edge, &mut [&mut a], "the outcome", |_, _| {
            said = outcome.taken();
            said.is_some()
        });
        let Some(Reshaped::Split { region, part, .. }) = said else {
            panic!("no split: {said:?}");
        };
        let mut part_runner = world.run_part(region, as_epoch, part);
        edge.read();
        edge.link(A, &a);
        edge.link(region, &part_runner);
        play(
            &mut edge,
            &mut [&mut a, &mut part_runner],
            "the resume",
            |edge, _| edge.settled(),
        );
        assert!(edge.lost.is_empty(), "{}", edge.describe());
        assert_eq!(edge.players[&player(1)].region, A);
        assert_eq!(edge.players[&player(2)].region, region);
        assert_eq!(a.region().state().players[&player(1)].pose.position.x, 10.5);
        assert_eq!(
            part_runner.region().state().players[&player(2)]
                .pose
                .position
                .x,
            19.5
        );
        assert_eq!(a.region().player_count(), 1);
    }
}

// ---------------------------------------------------------------------------------------
// Beyond the list: what is on its way when a region stops, stays through a merge and a
// split, and the runner's bookkeeping around tick M
// ---------------------------------------------------------------------------------------

/// Section 3.1, `Settling`: the ticks that ran are published before the runner goes
/// on. So when a merge closes a link, the link has been told everything the merged
/// state counts as applied, and what the state does not count is applied when the
/// edge sends it again, under the same numbers.
#[test]
fn everything_a_tick_took_before_the_stop_is_published_before_the_merge_closes_the_link() {
    for on_disk in [false, true] {
        let mut pair = pair(World::new(Shape::Stripes, on_disk));
        let absorbed = pair.release_b();
        let had = pair.to_a.sent;
        // With no step in between: five steps of player 1 and the order.
        let step = |number: u64, one| input(player(1), one, number, move_to(13.5 - number as f64));
        for number in 1..=5 {
            pair.to_a.next(step(number, pair.one));
        }
        let outcome = ask(&mut pair.a, absorbed.order());
        let reshaped = run_to_outcome(&mut pair.a, &mut [&mut pair.to_a], &outcome);
        assert_eq!(reshaped, Reshaped::Absorbed { absorbed: B });
        drop(absorbed);

        let state = pair.a.region().state();
        let counted = state.edges[&E].applied;
        let took = state.players[&player(1)].last_input;
        assert_eq!(counted, had + took, "on disk: {on_disk}");
        assert_eq!(
            applied(&pair.to_a.log),
            Some(counted),
            "{}",
            brief(&pair.to_a.log)
        );
        if took > 0 {
            assert!(
                moved(&pair.to_a.log, pair.one, 13.5 - took as f64).is_some(),
                "{}",
                brief(&pair.to_a.log)
            );
        }

        let mut again = greeted(&mut pair.a, hello_to_a(&pair.to_a)).after(counted);
        assert!(
            matches!(welcomed(&again.log), Welcome::Resumed { applied, .. } if applied == counted)
        );
        for number in took + 1..=5 {
            again.next(step(number, pair.one));
        }
        sync(&mut pair.a, &mut [&mut again], 0);
        let state = pair.a.region().state();
        assert_eq!(state.players[&player(1)].last_input, 5);
        assert_eq!(state.players[&player(1)].pose.position.x, 8.5);
        assert_eq!(state.edges[&E].applied, again.sent);
    }
}

/// Section 3.2, "What a link sent and no tick has taken", for what a step took from a
/// link and put aside: a message that waits behind its link's hello when the runner
/// stops is not in the region's state, and is not counted as received either. If the
/// merge is taken, the edge sends it again and it is applied, once; if the merge is
/// off, it is where it was and a later tick takes it.
#[test]
fn what_waited_behind_a_hello_when_the_region_stopped_is_not_lost_whatever_comes_of_the_merge() {
    for (on_disk, steps_before, declined) in [
        (false, 0, false),
        (false, 1, false),
        (false, 2, false),
        (true, 0, false),
        (true, 1, false),
        (false, 0, true),
        (false, 1, true),
        (true, 1, true),
    ] {
        let mut pair = pair(World::new(Shape::Stripes, on_disk));
        let absorbed = pair.release_b();
        let order = absorbed.order();
        if declined {
            drop(absorbed);
            pair.world.open_raw(B);
        }
        // A new link of the edge, whose hello names chunks that the region has to
        // claim and read, and a step behind it.
        let step = input(player(1), pair.one, 1, move_to(12.5));
        let mut held = Link::attach(&pair.a).after(pair.to_a.sent);
        held.hello(hello_to_a(&pair.to_a).chunks(vec![HOME, BACK, SECOND, FAR, FARTHER]));
        let walked = held.next(step.clone());
        for _ in 0..steps_before {
            pair.a.step();
            held.drain();
        }
        let outcome = ask(&mut pair.a, order);
        let reshaped = run_to_outcome(&mut pair.a, &mut [&mut held], &outcome);
        let case = format!("on disk: {on_disk}, {steps_before} steps before, declined: {declined}");

        if declined {
            assert!(matches!(reshaped, Reshaped::Off { .. }), "{case}");
            wait_applied(&mut pair.a, &mut [&mut held], 0, walked);
        } else {
            assert_eq!(reshaped, Reshaped::Absorbed { absorbed: B }, "{case}");
            let state = pair.a.region().state();
            let counted = state.edges[&E].applied;
            assert_eq!(
                state.players[&player(1)].last_input,
                u64::from(counted >= walked),
                "what the state counts as applied and what it has applied differ ({case})"
            );
            let mut again = greeted(&mut pair.a, hello_to_a(&pair.to_a)).after(counted);
            assert!(
                matches!(
                    welcomed(&again.log),
                    Welcome::Resumed { applied, .. } if applied == counted
                ),
                "{case}: {}",
                brief(&again.log)
            );
            if counted < walked {
                again.numbered(walked, step);
            }
            sync(&mut pair.a, &mut [&mut again], 0);
            assert!(!again.closed, "{case}");
        }
        let state = pair.a.region().state();
        assert_eq!(state.players[&player(1)].last_input, 1, "{case}");
        assert_eq!(state.players[&player(1)].pose.position.x, 12.5, "{case}");
    }
}

/// The same for a split, and for an action that waits for its chunk by the narrow hold
/// of section 3.6.
#[test]
fn what_waited_for_a_chunk_when_the_region_stopped_is_applied_when_it_is_sent_again_after_the_split()
 {
    for (on_disk, steps_before) in [(false, 0), (false, 1), (false, 2), (true, 0), (true, 1)] {
        let mut whole = whole_with(
            World::new(Shape::Stripes, on_disk),
            vec![HOME, FAR],
            1.5,
            FAR_X,
        );
        // Player 1 digs into a chunk nothing has asked about, behind the asking.
        whole.to_a.subscribe(vec![BACK]);
        let dug = whole
            .to_a
            .next(input(player(1), whole.one, 2, dig(BACK_BLOCK, 1)));
        for _ in 0..steps_before {
            whole.a.step();
            whole.to_a.drain();
        }
        let had = whole.to_a.sent;
        let (n, _, _) = whole.split(vec![FAR]);
        let case = format!("on disk: {on_disk}, {steps_before} steps before");

        let state = whole.a.region().state();
        let counted = state.edges[&E].applied;
        assert!(counted == had || counted == had - 1, "{case}");
        assert_eq!(
            state.players[&player(1)].last_input,
            if counted >= dug { 2 } else { 1 },
            "{case}"
        );
        let mut again = greeted(
            &mut whole.a,
            greeting(E, 5)
                .since(since(&whole.to_a.log))
                .players(vec![player(1)])
                .chunks(vec![HOME, BACK, FAR]),
        )
        .after(counted);
        assert_eq!(
            again.answers(FAR),
            vec![(0, Answer::Elsewhere(n))],
            "{case}"
        );
        if counted < dug {
            again.numbered(dug, input(player(1), whole.one, 2, dig(BACK_BLOCK, 1)));
        }
        sync(&mut whole.a, &mut [&mut again], 0);
        // Whichever tick judged the dig, it judged it on the loaded chunk.
        assert_eq!(block_of(&whole.a, BACK_BLOCK), Some(blocks::AIR), "{case}");
        assert_eq!(
            whole.a.region().state().players[&player(1)].last_input,
            2,
            "{case}"
        );
    }
}

/// Section 3.3, item 6: the status that others read is brought up to date by the
/// tick of a merge and of a split, which no ordinary tick follows at once.
#[test]
fn the_status_others_read_is_brought_up_to_date_by_the_tick_of_a_merge_and_of_a_split() {
    let read = |runner: &RegionRunner| {
        let status = runner.status();
        (
            status.tick.load(Ordering::SeqCst),
            status.players.load(Ordering::SeqCst),
            status.chunks.load(Ordering::SeqCst),
            status.held.load(Ordering::SeqCst),
            status.crowds(),
        )
    };
    let seen = |runner: &RegionRunner| {
        let region = runner.region();
        (
            region.tick_number(),
            region.player_count() as u64,
            region.loaded_chunk_count() as u64,
            region.held_chunk_count() as u64,
            region.crowds(),
        )
    };

    let mut pair = pair(World::stripes());
    one_tick(&mut pair.a, &mut [&mut pair.to_a]);
    assert_eq!(read(&pair.a), seen(&pair.a), "before anything");
    let m = pair.merge();
    assert_eq!(read(&pair.a), seen(&pair.a), "after a merge");
    assert_eq!(read(&pair.a).0, m);
    assert_eq!(read(&pair.a).1, 2);
    assert_eq!(read(&pair.a).2, 0);

    let mut whole = whole(World::stripes());
    one_tick(&mut whole.a, &mut [&mut whole.to_a]);
    assert_eq!(read(&whole.a), seen(&whole.a), "before a split");
    let (n, as_epoch, part) = whole.split(vec![FAR]);
    assert_eq!(read(&whole.a), seen(&whole.a), "after a split");
    assert_eq!(read(&whole.a).1, 1);
    let part_runner = whole.world.run_part(n, as_epoch, part);
    assert_eq!(read(&part_runner), seen(&part_runner), "the part");
    assert_eq!(read(&part_runner).1, 1);
    assert_eq!(read(&part_runner).3, 2);
}

/// Section 3.1: a step advances at most one stage, for a release as well, so whoever
/// steps a runner by hand sees every one of them.
#[test]
fn a_release_passes_through_its_stages_a_step_at_a_time() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_release_passes_through_its_stages_a_step_at_a_time_in,
    );
}

fn a_release_passes_through_its_stages_a_step_at_a_time_in(mut world: World) {
    let mut runner = world.open(A);
    let mut e = greeted(&mut runner, greeting(E, 5).chunks(vec![HOME]));
    join_and_wait(&mut runner, &mut e, player(1));
    sync(&mut runner, &mut [&mut e], 0);
    assert_eq!(runner.stage(), None);
    runner.begin_release();
    let mut seen = Vec::new();
    for _ in 0..STEPS {
        if let Some(stage) = runner.stage() {
            if seen.last() != Some(&stage) {
                seen.push(stage);
            }
        }
        if runner.ended().is_some() {
            break;
        }
        runner.step();
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(runner.ended(), Some(Ended::Released));
    assert_eq!(
        seen,
        vec![Stage::Preparing, Stage::Settling, Stage::Closing]
    );
    assert_eq!(runner.stage(), None, "none once it has ended");
}

/// Section 3.8: a runner that is told to stop in the middle of a merge lets go of the
/// region as it is, without waiting for the store, and the outcome is that the store
/// was lost: what happened, the store's list says, and it says one of the two.
#[test]
fn a_runner_that_is_told_to_stop_in_the_middle_of_a_merge_lets_go_of_the_region_as_it_is() {
    for stage in [
        Stage::Preparing,
        Stage::Settling,
        Stage::Closing,
        Stage::Committing,
    ] {
        let mut pair = pair(World::stripes());
        let absorbed = pair.release_b();
        let before = pair.world.list();
        let outcome = ask(&mut pair.a, absorbed.order());
        run_to_stage(&mut pair.a, &mut [&mut pair.to_a], stage);
        let stop = AtomicBool::new(true);
        let ended = pair.a.run(&stop);
        let said = outcome.taken();
        pair.to_a.drain();
        assert!(pair.to_a.closed, "at {stage:?}");
        assert_eq!(pair.a.stage(), None);
        assert_eq!(pair.a.ended(), Some(ended));
        assert_eq!(outcome.taken(), None, "the outcome is said once");
        drop(absorbed);

        let list = pair.world.list();
        let happened = list.absorbed == vec![(B, A)];
        assert_eq!(list.regions.len(), if happened { 1 } else { 2 }, "{list:?}");
        match said {
            // The store's answer was there when the runner looked a last time.
            Some(Reshaped::Absorbed { absorbed: B }) => {
                assert_eq!(stage, Stage::Committing);
                assert!(happened);
            }
            Some(Reshaped::Off {
                why: Off::StoreLost,
            }) => {
                assert_eq!(ended, Ended::Abandoned, "at {stage:?}");
                if matches!(stage, Stage::Preparing | Stage::Settling) {
                    assert_eq!(list, before, "at {stage:?}");
                }
            }
            other => panic!("at {stage:?} the outcome was {other:?}"),
        }
        // Whoever runs the region next finds it before the merge or after it.
        let next = pair.world.open(A);
        assert_eq!(next.region().player_count(), if happened { 2 } else { 1 });
    }
}

/// Rules 39 and 48: an arrival that the survivor itself let go to the absorbed region
/// before the merge, and that the edge could not pass on, comes back to the survivor
/// and is taken in.
#[test]
fn a_player_the_survivor_had_let_go_to_the_absorbed_region_is_taken_in_when_the_arrival_comes_back()
{
    in_memory_and_on_disk(
        Shape::Stripes,
        a_player_the_survivor_had_let_go_to_the_absorbed_region_is_taken_in_when_the_arrival_comes_back_in,
    );
}

fn a_player_the_survivor_had_let_go_to_the_absorbed_region_is_taken_in_when_the_arrival_comes_back_in(
    mut world: World,
) {
    let mut a = world.open(A);
    let mut b = world.open(B);
    let mut to_a = greeted(&mut a, greeting(E, 5).chunks(vec![HOME, NEXT]));
    let one = join_and_wait(&mut a, &mut to_a, player(1));
    let two = join_and_wait(&mut a, &mut to_a, player(2));
    to_a.next(input(player(2), two, 1, move_to(16.5)));
    run_until(
        &mut a,
        &mut [&mut to_a],
        "player 2 being let go",
        |_, links| departure(&links[0].log, player(2)).is_some(),
    );
    let (_, _, to, on_the_way) = departure(&to_a.log, player(2)).expect("waited for it");
    assert_eq!(to, B);
    let on_the_way = on_the_way.clone();
    sync(&mut a, &mut [&mut to_a], 0);

    // Region 1 is absorbed before the edge has a link to it.
    b.begin_release();
    assert_eq!(run_to_the_end(&mut b, &mut []), Ended::Released);
    let absorbed = world.open_to_absorb(B);
    let outcome = ask(&mut a, absorbed.order());
    assert_eq!(
        run_to_outcome(&mut a, &mut [&mut to_a], &outcome),
        Reshaped::Absorbed { absorbed: B }
    );
    drop(absorbed);

    let mut again = greeted(
        &mut a,
        greeting(E, 5)
            .since(since(&to_a.log))
            .players(vec![player(1)])
            .chunks(vec![HOME]),
    )
    .after(to_a.sent);
    assert_eq!(
        entries(&again.log),
        vec![
            (
                1,
                Durable::Departed {
                    player: player(2),
                    transfer: on_the_way.clone(),
                    to: B,
                }
            ),
            (
                2,
                Durable::Absorbed {
                    region: B,
                    since: 0,
                    applied: 0,
                    numbers: Vec::new(),
                }
            ),
        ]
    );
    assert_eq!(presences(&again.log), vec![(player(1), Some(one))]);

    // The edge reads the two in order: the player is to go to region 1, and region 1
    // means this region. It asks for their view here and sends the arrival here.
    again.subscribe(vec![NEXT]);
    again.next(arrive(player(2), &on_the_way));
    let walked = again.next(input(player(2), two, 2, move_to(17.5)));
    wait_applied(&mut a, &mut [&mut again], 0, walked);
    sync(&mut a, &mut [&mut again], 0);
    assert_eq!(
        a.region()
            .player(player(2))
            .map(|(entity, pose)| (entity, pose.position.x)),
        Some((two, 17.5))
    );
    assert_eq!(outbox(&again.log).len(), 2, "{}", brief(&again.log));
    assert_eq!(a.region().knowledge(NEXT), Knowledge::Held);
}

/// A4 and A5 of the record's list for an edge, on the runner: a stay that came with a
/// merge, of a player who has left since, is ended by a leave that names it and by no
/// other; and if the player joins again, the join begins a new stay in its place.
/// What the edge then sends that names the old stay, having kept it for the absorbed
/// region, moves nobody, and the new stay's first input is applied.
#[test]
fn after_a_merge_a_join_replaces_the_stay_that_came_with_it_and_what_names_the_old_stay_is_passed_over()
 {
    in_memory_and_on_disk(
        Shape::Stripes,
        after_a_merge_a_join_replaces_the_stay_that_came_with_it_and_what_names_the_old_stay_is_passed_over_in,
    );
}

fn after_a_merge_a_join_replaces_the_stay_that_came_with_it_and_what_names_the_old_stay_is_passed_over_in(
    world: World,
) {
    let mut pair = pair(world);
    pair.merge();
    let mut again = greeted(&mut pair.a, hello_to_a(&pair.to_a)).after(pair.to_a.sent);
    assert_eq!(
        presences(&again.log),
        vec![(player(1), Some(pair.one)), (player(2), Some(pair.two))]
    );

    // A leave that names another entity than the stay's ends nothing.
    let wrong = again.next(leave(player(2), Some(pair.one)));
    wait_applied(&mut pair.a, &mut [&mut again], 0, wrong);
    assert_eq!(pair.a.region().player_count(), 2);

    // The player joins again.
    again.next(join(player(2)));
    run_until(
        &mut pair.a,
        &mut [&mut again],
        "the player entering the world anew",
        |_, links| spawned(&links[0].log, player(2)).is_some(),
    );
    let (_, new) = spawned(&again.log, player(2)).expect("waited for it");
    assert!(new.0 > pair.two.0, "the later stay has the higher entity");
    assert!(
        removal(&again.log, pair.two).is_some(),
        "{}",
        brief(&again.log)
    );
    let stay = pair.a.region().state().players[&player(2)].clone();
    assert_eq!((stay.entity_id, stay.last_input), (new, 0));
    assert_eq!(stay.pose.position, SPAWN);

    // What the edge kept of the earlier stay and sends now, and the new stay's first.
    again.next(input(player(2), pair.two, 4, move_to(18.5)));
    again.next(leave(player(2), Some(pair.two)));
    let fresh = again.next(input(player(2), new, 1, move_to(12.5)));
    wait_applied(&mut pair.a, &mut [&mut again], 0, fresh);
    sync(&mut pair.a, &mut [&mut again], 0);
    let stay = pair.a.region().state().players[&player(2)].clone();
    assert_eq!((stay.entity_id, stay.last_input), (new, 1));
    assert_eq!(stay.pose.position.x, 12.5);
    assert!(moved(&again.log, pair.two, 18.5).is_none());
    assert!(moved(&again.log, new, 12.5).is_some());
    assert!(
        again.log.iter().any(|message| matches!(
            message,
            WorkerToEdge::Progress { inputs, .. } if inputs.contains(&(player(2), 1))
        )),
        "the edge is told the new stay's input: {}",
        brief(&again.log)
    );

    // The leave that names the stay there is ends it.
    let right = again.next(leave(player(2), Some(new)));
    wait_applied(&mut pair.a, &mut [&mut again], 0, right);
    assert!(pair.a.region().player(player(2)).is_none());
}

/// A6: the same for a part. A later stay of the player arrives at the new region,
/// from wherever they joined again, and takes the place of the stay the split
/// brought; inputs and a leave that name the old entity change nothing, and an
/// arrival of the earlier stay is passed over.
#[test]
fn in_a_part_an_arrival_of_a_later_stay_replaces_the_stay_that_came_with_the_split() {
    in_memory_and_on_disk(
        Shape::Stripes,
        in_a_part_an_arrival_of_a_later_stay_replaces_the_stay_that_came_with_the_split_in,
    );
}

fn in_a_part_an_arrival_of_a_later_stay_replaces_the_stay_that_came_with_the_split_in(
    world: World,
) {
    let mut whole = whole(world);
    let (n, as_epoch, part) = whole.split(vec![FAR]);
    let mut part_runner = whole.world.run_part(n, as_epoch, part);
    let mut to_n = greeted(&mut part_runner, greeting(E, 5).chunks(vec![FAR]));
    assert_eq!(presences(&to_n.log), vec![(player(2), Some(whole.two))]);

    let later = EntityId(whole.two.0 + 100);
    to_n.next(arrive(player(2), &transfer(later, FAR_X + 2.0)));
    to_n.next(input(player(2), whole.two, 3, move_to(FAR_X - 3.0)));
    to_n.next(leave(player(2), Some(whole.two)));
    let fresh = to_n.next(input(player(2), later, 1, move_to(FAR_X + 3.0)));
    wait_applied(&mut part_runner, &mut [&mut to_n], 0, fresh);
    sync(&mut part_runner, &mut [&mut to_n], 0);
    let stay = part_runner.region().state().players[&player(2)].clone();
    assert_eq!((stay.entity_id, stay.last_input), (later, 1));
    assert_eq!(stay.pose.position.x, FAR_X + 3.0);
    assert!(
        removal(&to_n.log, whole.two).is_some(),
        "{}",
        brief(&to_n.log)
    );
    assert!(moved(&to_n.log, whole.two, FAR_X - 3.0).is_none());

    // An arrival of the earlier stay comes late: it is passed over, and the stay
    // that is there stays as it is.
    let stale = to_n.next(arrive(player(2), &transfer(whole.two, FAR_X)));
    wait_applied(&mut part_runner, &mut [&mut to_n], 0, stale);
    sync(&mut part_runner, &mut [&mut to_n], 0);
    assert_eq!(
        part_runner
            .region()
            .player(player(2))
            .map(|(entity, pose)| (entity, pose.position.x)),
        Some((later, FAR_X + 3.0))
    );
    let last = greeted(&mut part_runner, greeting(E, 5).since(since(&to_n.log)));
    assert_eq!(presences(&last.log), vec![(player(2), Some(later))]);
}

/// A7: a link that ended between the `Absorbed` and the entries behind it says in its
/// next hello that it has seen the `Absorbed`, and is sent the rest without it.
#[test]
fn a_hello_that_has_seen_the_entry_of_the_merge_is_sent_the_entries_behind_it_alone() {
    let mut pair = pair(World::stripes());
    pair.merge();
    let again = greeted(&mut pair.a, hello_to_a(&pair.to_a).seen(2));
    assert_eq!(
        entries(&again.log),
        vec![(
            3,
            Durable::Remote {
                action: breaking(player(2), 1, BEHIND),
                to: Some(A),
            }
        )]
    );
    assert_eq!(
        presences(&again.log),
        vec![(player(1), Some(pair.one)), (player(2), Some(pair.two))]
    );
    // And the entry of the merge is gone from the outbox, confirmed by that hello.
    let last = greeted(&mut pair.a, hello_to_a(&pair.to_a).seen(2));
    assert_eq!(entries(&last.log).len(), 1);
    let outbox = &pair.a.region().state().edges[&E].outbox;
    assert_eq!(outbox.keys().collect::<Vec<_>>(), vec![&3]);
}

/// A9: after a split, the region that was split forgets the edge, which had no link
/// to it for too long, while the part has had a link of that edge. The split region's
/// welcome is `Unknown` with no entries; the part still has the stay that went, and
/// a leave that names it ends it.
#[test]
fn a_part_keeps_the_stay_that_went_when_the_split_region_has_forgotten_the_edge() {
    const GONE_AFTER: u64 = 40;
    let mut world = World::stripes();
    let a = world.open(A).with_gone_after(GONE_AFTER);
    let mut whole = whole_of(world, a, vec![HOME, FAR], SPAWN.x, FAR_X);
    let (n, as_epoch, part) = whole.split(vec![FAR]);
    let m = whole.a.region().tick_number();
    let mut part_runner = whole.world.run_part(n, as_epoch, part);
    let first = greeted(&mut part_runner, greeting(E, 5));
    assert_eq!(presences(&first.log), vec![(player(2), Some(whole.two))]);

    while whole.a.region().tick_number() < m + GONE_AFTER + 5 {
        one_tick(&mut whole.a, &mut []);
    }
    assert_eq!(whole.a.region().player_count(), 0);
    let forgotten = greeted(
        &mut whole.a,
        greeting(E, 5)
            .since(since(&whole.to_a.log))
            .players(vec![player(1), player(2)]),
    );
    assert!(
        matches!(
            welcomed(&forgotten.log),
            Welcome::Unknown {
                entries: 0,
                presences: 2,
                applied: 0,
                ..
            }
        ),
        "{}",
        brief(&forgotten.log)
    );
    assert_eq!(
        presences(&forgotten.log),
        vec![(player(1), None), (player(2), None)]
    );

    let mut again = greeted(&mut part_runner, greeting(E, 5).since(since(&first.log)));
    assert_eq!(presences(&again.log), vec![(player(2), Some(whole.two))]);
    let left = again.next(leave(player(2), Some(whole.two)));
    wait_applied(&mut part_runner, &mut [&mut again], 0, left);
    assert_eq!(part_runner.region().player_count(), 0);
}

/// Section 3.2: when the commit is sent no chunk is unsaved, so what a region serves
/// from memory after the tick of a merge is, block for block, what the next owner
/// reads from the store. A block is broken by a tick that runs while the merge is
/// prepared, or is not, as the store's pace has it; the two agree either way.
#[test]
fn a_block_broken_while_a_merge_was_prepared_is_the_same_from_memory_and_from_the_store() {
    for (on_disk, steps_before) in [(false, 0), (false, 1), (true, 0), (true, 1)] {
        let mut pair = pair(World::new(Shape::Stripes, on_disk));
        let absorbed = pair.release_b();
        pair.to_a.next(input(player(1), pair.one, 1, dig(NEAR, 1)));
        for _ in 0..steps_before {
            pair.a.step();
            pair.to_a.drain();
        }
        let outcome = ask(&mut pair.a, absorbed.order());
        assert_eq!(
            run_to_outcome(&mut pair.a, &mut [&mut pair.to_a], &outcome),
            Reshaped::Absorbed { absorbed: B }
        );
        drop(absorbed);
        let broken = pair.a.region().state().players[&player(1)].last_input == 1;
        let expected = if broken {
            blocks::AIR
        } else {
            blocks::GRASS_BLOCK
        };
        let case = format!("on disk: {on_disk}, {steps_before} steps before, broken: {broken}");

        let live = greeted(&mut pair.a, hello_to_a(&pair.to_a));
        let (_, chunk, _) = snapshot(&live.log, HOME, 0).expect("the chunk is served");
        assert_eq!(block_in(chunk, NEAR), expected, "from memory ({case})");

        let mut next = pair.world.open(A);
        let restored = greeted(&mut next, hello_to_a(&pair.to_a));
        let (_, chunk, _) = snapshot(&restored.log, HOME, 0).expect("the chunk is served");
        assert_eq!(block_in(chunk, NEAR), expected, "from the store ({case})");
    }
}

/// Section 3.8, "A checkpoint by the clock": a region that checkpoints with every
/// tick merges and splits all the same, and what the next owner is restored with is
/// the region after the merge or the split, not a checkpoint from before it.
///
/// On disk it is every twenty-fifth tick: the store has one thread for the chunks and
/// checkpoints of all regions, a checkpoint on disk takes longer than a step, and a
/// region's load waits behind every checkpoint asked for before it, the other
/// region's too. With one for every tick that is seconds, which is the store's
/// business and not what this test is about.
#[test]
fn a_region_that_checkpoints_with_every_tick_merges_and_splits_all_the_same() {
    let often: [(bool, u64, Tweak); 2] = [
        (false, 1, |runner| runner.with_checkpoint_interval(1)),
        (true, 25, |runner| runner.with_checkpoint_interval(25)),
    ];
    for (on_disk, interval, tweak) in often {
        let mut pair = pair_with(World::new(Shape::Stripes, on_disk), tweak);
        let m = pair.merge();
        let mut again = greeted(&mut pair.a, hello_to_a(&pair.to_a)).after(pair.to_a.sent);
        let walked = again.next(input(player(2), pair.two, 4, move_to(18.5)));
        wait_applied(&mut pair.a, &mut [&mut again], 0, walked);
        sync(&mut pair.a, &mut [&mut again], 0);
        // Far enough for a checkpoint by the clock to have been made since the merge.
        ticks(&mut pair.a, &mut [&mut again], interval);
        sync(&mut pair.a, &mut [&mut again], 0);
        let state = pair.a.region().state();
        let (handle, restored) = pair.world.open_raw(A);
        assert!(restored.tick() > m);
        assert_eq!(restored.pinned, areas(Shape::Stripes));
        let next = RegionRunner::restore(config(RETURN_AFTER), handle, restored)
            .expect("what the store has is readable");
        assert_eq!(next.region().state().players, state.players);
        assert_eq!(next.region().state().edges, state.edges);
        assert_eq!(state.players[&player(2)].pose.position.x, 18.5);

        let mut world = World::new(Shape::Stripes, on_disk);
        let a = tweak(world.open(A));
        let mut whole = whole_of(world, a, vec![HOME, FAR], SPAWN.x, FAR_X);
        let (n, _, part) = whole.split(vec![FAR]);
        let (stays, goes) = (whole.a.region().state(), part.region.state());
        drop(part);
        for (region, state) in [(A, stays), (n, goes)] {
            let (handle, restored) = whole.world.open_raw(region);
            let next = RegionRunner::restore(config(RETURN_AFTER), handle, restored)
                .expect("what the store has is readable");
            assert_eq!(next.region().state(), state, "{region:?}");
        }
    }
}

/// Section 3.2: a chunk that the store grants in answer to a claim no tick has been
/// told of is not held by the sim when the split is worked out, so it is no chunk of
/// the part, and is the split region's by the store as by the sim. One that a tick was
/// told of before the stop goes with the part if it is nearer to who goes. Either way
/// exactly one of the two regions holds each chunk afterwards, the store says the
/// same, and nothing waits for an answer that was dropped.
#[test]
fn a_chunk_asked_for_right_before_a_split_is_one_regions_by_the_sim_as_by_the_store() {
    for (on_disk, ticks_before) in [(false, 0), (false, 1), (false, 2), (true, 1), (true, 2)] {
        let mut whole = whole_with(
            World::new(Shape::Stripes, on_disk),
            vec![HOME, FAR],
            SPAWN.x,
            FAR_X,
        );
        // One chunk beside the home chunk and one beside the player who goes.
        whole.to_a.subscribe(vec![BACK, SECOND]);
        ticks(&mut whole.a, &mut [&mut whole.to_a], ticks_before);
        let (n, as_epoch, part) = whole.split(vec![FAR]);
        let case = format!("on disk: {on_disk}, {ticks_before} ticks before");

        let ours = |chunk| whole.a.region().knowledge(chunk) == Knowledge::Held;
        let theirs = |chunk| part.region.knowledge(chunk) == Knowledge::Held;
        assert!(!theirs(BACK), "{case}");
        assert!(!(ours(SECOND) && theirs(SECOND)), "{case}");
        let (back_is_ours, second_is_ours, second_is_theirs) =
            (ours(BACK), ours(SECOND), theirs(SECOND));
        if ticks_before > 0 {
            // A tick took the asking, the store has answered every claim, and the
            // region has the answers: it holds what was not split off.
            assert!(back_is_ours, "{case}");
            assert!(second_is_ours || second_is_theirs, "{case}");
        }

        // The store says the same of the part.
        let (handle, restored) = whole
            .world
            .open_with(n, as_epoch)
            .expect("the epoch of the split opens the part");
        let granted: Vec<ChunkPos> = restored.held.iter().map(|(chunk, _)| *chunk).collect();
        assert_eq!(granted.contains(&SECOND), second_is_theirs, "{case}");
        assert!(granted.contains(&FAR) && !granted.contains(&BACK), "{case}");
        drop(handle);

        // And of the region that was split: what it is asked for it serves, or says
        // where it is.
        let again = greeted(
            &mut whole.a,
            greeting(E, 5)
                .since(since(&whole.to_a.log))
                .players(vec![player(1)])
                .chunks(vec![HOME, BACK, SECOND, FAR]),
        );
        assert_eq!(again.answers(BACK), vec![(0, Answer::Snapshot)], "{case}");
        assert_eq!(
            again.answers(SECOND),
            vec![(
                0,
                if second_is_theirs {
                    Answer::Elsewhere(n)
                } else {
                    Answer::Snapshot
                }
            )],
            "{case}"
        );
        assert_eq!(
            again.answers(FAR),
            vec![(0, Answer::Elsewhere(n))],
            "{case}"
        );
    }
}

/// Section 2.4, items 5 and 6, seen from links: the entry of a split is written for
/// each edge that has a player who goes, and for no other; and the part knows those
/// edges, since the tick of the split, and no other.
#[test]
fn a_split_is_told_to_each_edge_that_has_a_player_in_the_part_and_to_no_other() {
    in_memory_and_on_disk(
        Shape::Stripes,
        a_split_is_told_to_each_edge_that_has_a_player_in_the_part_and_to_no_other_in,
    );
}

fn a_split_is_told_to_each_edge_that_has_a_player_in_the_part_and_to_no_other_in(world: World) {
    let mut whole = whole(world);
    // Edge F has a player who stays, and edge G one who goes.
    let mut f = greeted(&mut whole.a, greeting(F, 7).chunks(vec![HOME]));
    let four = join_and_wait(&mut whole.a, &mut f, player(4));
    let mut g = greeted(&mut whole.a, greeting(G, 9).chunks(vec![HOME, FAR]));
    let five = join_and_wait(&mut whole.a, &mut g, player(5));
    let walked = g.next(input(player(5), five, 1, move_to(FAR_X - 2.0)));
    wait_applied(
        &mut whole.a,
        &mut [&mut g, &mut f, &mut whole.to_a],
        0,
        walked,
    );
    settle(&mut whole.a, &mut [&mut whole.to_a, &mut f, &mut g]);
    let (f_since, g_since) = (since(&f.log), since(&g.log));

    let as_epoch = whole.world.next_epoch();
    let next = whole.world.list().next;
    let outcome = ask(
        &mut whole.a,
        Reshape::SplitOff {
            chunks: vec![FAR],
            as_epoch,
            part: next,
        },
    );
    let reshaped = run_to_outcome(
        &mut whole.a,
        &mut [&mut whole.to_a, &mut f, &mut g],
        &outcome,
    );
    let Reshaped::Split {
        region: n, part, ..
    } = reshaped
    else {
        panic!("no split: {reshaped:?}");
    };
    assert!(whole.to_a.closed && f.closed && g.closed);
    let m = whole.a.region().tick_number();
    assert_eq!(part.region.player_count(), 2);
    assert_eq!(whole.a.region().player_count(), 2);

    // The region that was split.
    let to_f = greeted(
        &mut whole.a,
        greeting(F, 7).since(f_since).chunks(vec![HOME]),
    );
    assert_eq!(
        welcomed(&to_f.log),
        Welcome::Resumed {
            entries: 0,
            presences: 1,
            applied: f.sent,
        },
        "{}",
        brief(&to_f.log)
    );
    assert_eq!(presences(&to_f.log), vec![(player(4), Some(four))]);
    let to_g = greeted(
        &mut whole.a,
        greeting(G, 9).since(g_since).players(vec![player(5)]),
    );
    assert_eq!(
        entries(&to_g.log),
        vec![(
            1,
            Durable::SplitOff {
                region: n,
                players: vec![(player(5), five)],
            }
        )]
    );
    assert_eq!(presences(&to_g.log), vec![(player(5), None)]);
    let to_e = greeted(
        &mut whole.a,
        greeting(E, 5).since(since(&whole.to_a.log)).seen(1),
    );
    assert_eq!(
        entries(&to_e.log),
        vec![(
            2,
            Durable::SplitOff {
                region: n,
                players: vec![(player(2), whole.two)],
            }
        )]
    );
    assert_eq!(presences(&to_e.log), vec![(player(1), Some(whole.one))]);

    // The part: it knows the two edges whose players came, each with the start the
    // split region knew it with, and has never heard of the third.
    let mut part_runner = whole.world.run_part(n, as_epoch, part);
    assert_eq!(part_runner.region().edge(F), None);
    for (edge, start, id, entity) in [(E, 5, player(2), whole.two), (G, 9, player(5), five)] {
        let link = greeted(&mut part_runner, greeting(edge, start));
        assert_eq!(
            welcomed(&link.log),
            Welcome::Unknown {
                since: m,
                entries: 0,
                presences: 1,
                applied: 0,
            },
            "{edge:?}: {}",
            brief(&link.log)
        );
        assert_eq!(presences(&link.log), vec![(id, Some(entity))]);
    }
    let before = part_runner.region().tick_number();
    let stranger = greeted(&mut part_runner, greeting(F, 7).players(vec![player(4)]));
    assert!(
        matches!(
            welcomed(&stranger.log),
            Welcome::Unknown {
                since,
                entries: 0,
                presences: 1,
                applied: 0,
            } if since > before
        ),
        "{}",
        brief(&stranger.log)
    );
    assert_eq!(presences(&stranger.log), vec![(player(4), None)]);
    // An earlier start of an edge the part knows is told that it has been replaced.
    let mut old = Link::attach(&part_runner);
    old.hello(greeting(G, 8));
    run_until(
        &mut part_runner,
        &mut [&mut old],
        "the earlier start being turned away",
        |_, links| links[0].closed,
    );
    assert_eq!(welcomed(&old.log), Welcome::Superseded);
}
