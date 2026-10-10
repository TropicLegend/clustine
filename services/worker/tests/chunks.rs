//! Tests of the region runner on chunk sets, written from sections 4 and 5 of
//! `docs/adr/0012-the-tick-on-chunks.md` and the scenarios R1 to R10, R13 and R15 to R22
//! of its section 8, by someone who has not read how the runner does it.
//!
//! A test plays the edges, as in `specification.rs`: it attaches the worker's end of a
//! link to a runner, numbers its messages itself and reads what comes back. The runner
//! is stepped by the test, and the world store answers on threads of its own, so every
//! wait is a loop that steps until a message or a state is there. The store is divided:
//! into **stripes** at a boundary at 1, of which region 0 is under test, or **with a
//! gap**, where the home region, region 2, is under test and holds the home chunk alone.
//! The test plays the other regions through handles of their own. A crash is the region
//! opened again with a higher epoch.
//!
//! Every link's log is held to what section 5 says a region sends, after every message
//! read and whatever the test is about; see [`check`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::thread;
use std::time::Duration;

use clustine_data::{BlockState, blocks, items};
use clustine_region::RegionId;
use clustine_rpc::link::{self, EdgeEnd};
use clustine_rpc::{
    EdgeMessage, EdgeToWorker, Presence, RegionHello, Restored, StoreReply, StoreRequest, Welcome,
    WorkerToEdge,
};
use clustine_sim::api::{
    EntityKind, EntityState, HOTBAR_SLOTS, ItemStack, Misdirected, PlayerInput, PlayerTransfer,
    Pose, RegionEvent, RemoteAction, RemoteStep,
};
use clustine_sim::{Durable, Knowledge, PlayerEvent, PlayerJoin, RegionConfig};
use clustine_worker::{Ended, RegionRunner, Worker};
use clustine_world::{
    BlockPos, Chunk, ChunkArea, ChunkGenerator, ChunkPos, EdgeId, EntityId, PlayerId, Vec3,
};
use clustine_worldgen::FlatGenerator;
use clustine_worldstore::{Division, Store, StoreHandle};
use uuid::Uuid;

const E: EdgeId = EdgeId(11);
const F: EdgeId = EdgeId(22);
const G: EdgeId = EdgeId(33);

thread_local! {
    /// Since when the region knows each edge of the test that is running, as the last
    /// welcome that a link of that edge has read said. An edge that keeps to section
    /// 5.1 says it in its next hello, and the links see to that by themselves.
    static SINCE: std::cell::RefCell<BTreeMap<EdgeId, u64>> =
        const { std::cell::RefCell::new(BTreeMap::new()) };
}

/// The chunk players enter both worlds in. On stripes it is the easternmost chunk of
/// the region under test; with the gap it is all the region under test holds.
const HOME: ChunkPos = ChunkPos::new(0, 0);

/// The classic flat world has its grass at y = -61, and players stand on it.
const GROUND: i32 = -61;
const FEET: f64 = -60.0;

/// Players enter the world three blocks from the eastern end of the home chunk, so that
/// they can reach blocks of the chunk east of it and walk into it.
const SPAWN: Vec3 = Vec3::new(13.5, FEET, 8.5);

/// A block of the home chunk that a player at the spawn can reach.
const NEAR: BlockPos = BlockPos::new(12, GROUND, 9);

/// The chunk east of the home chunk. On stripes it is the first of the other stripe;
/// with the gap it is free.
const NEXT: ChunkPos = ChunkPos::new(1, 0);

/// A block of [`NEXT`] that a player at the spawn can reach.
const BEYOND: BlockPos = BlockPos::new(16, GROUND, 8);

/// Chunks of the stripe of the region under test, away from the home chunk.
const OWN: ChunkPos = ChunkPos::new(-3, 2);
const OWN_TOO: ChunkPos = ChunkPos::new(-5, -1);

/// A chunk of the other stripe that touches nothing a test walks or digs in.
const OTHER: ChunkPos = ChunkPos::new(5, 3);

/// Chunks in the gap, which are nobody's until they are claimed. [`NEXT`] is one too.
const FREE: ChunkPos = ChunkPos::new(2, 0);
const FREE_FAR: ChunkPos = ChunkPos::new(7, 4);

/// With the gap: a chunk of the area region 1 is pinned to.
const EAST: ChunkPos = ChunkPos::new(20, 0);

/// The region the tests call the neighbour, in both worlds.
const NEIGHBOUR: RegionId = RegionId(1);

/// With the gap: the region pinned to the west, which a test plays where it needs a
/// third region.
const WESTERN: RegionId = RegionId(0);

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

/// Of every chunk the region asks the store whether it is its own.
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
    /// Region 0 is pinned to the chunks west of x = 0 and region 1 to those from x = 16
    /// on; region 2 is home and holds the home chunk and nothing else, and the other
    /// chunks in between are free (ADR-0011, section 9).
    Gap,
}

/// A world store, the region under test and the epochs its regions were opened with.
struct World {
    store: Store,
    shape: Shape,
    epoch: u64,
    /// The epochs the test has opened the other regions with.
    others: BTreeMap<RegionId, u64>,
    /// Kept so that the directory outlives the store.
    directory: Option<tempfile::TempDir>,
}

impl World {
    fn new(shape: Shape, on_disk: bool) -> Self {
        // A new world knows no edge, whatever a test before on this thread was told.
        SINCE.with(|since| since.borrow_mut().clear());
        let division = match shape {
            Shape::Stripes => Division::side_by_side(HOME, &[1]).expect("one cut ascends"),
            Shape::Gap => Division {
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
            others: BTreeMap::new(),
            directory,
        }
    }

    fn stripes() -> Self {
        Self::new(Shape::Stripes, false)
    }

    fn gap() -> Self {
        Self::new(Shape::Gap, false)
    }

    /// The region under test.
    fn region(&self) -> RegionId {
        match self.shape {
            Shape::Stripes => RegionId(0),
            Shape::Gap => RegionId(2),
        }
    }

    fn hello(&self, region: RegionId, epoch: u64) -> RegionHello {
        RegionHello { region, epoch }
    }

    /// Opens the region under test as its next owner. Whoever had it before has lost it.
    fn open_raw(&mut self) -> (StoreHandle, Restored) {
        self.epoch += 1;
        self.store
            .open_region(self.hello(self.region(), self.epoch))
            .expect("a higher epoch opens the region")
    }

    /// A runner for the region under test as the store has it now, which gives back a
    /// chunk at the end of the first tick in which nothing uses it.
    fn open(&mut self) -> RegionRunner {
        self.open_returning_after(0)
    }

    fn open_returning_after(&mut self, ticks: u64) -> RegionRunner {
        let (handle, restored) = self.open_raw();
        RegionRunner::restore(config(ticks), handle, restored)
            .expect("what the store has is readable")
    }

    /// The neighbour, played by the test through a handle of its own.
    fn neighbour(&mut self) -> Other {
        self.other(NEIGHBOUR)
    }

    /// Another region than the one under test, played by the test.
    fn other(&mut self, region: RegionId) -> Other {
        assert_ne!(region, self.region(), "that is the region under test");
        let epoch = self.others.entry(region).or_insert(0);
        *epoch += 1;
        let epoch = *epoch;
        let (handle, _) = self
            .store
            .open_region(self.hello(region, epoch))
            .expect("a higher epoch opens the region");
        Other { handle }
    }

    /// Where a world on disk is.
    fn root(&self) -> &Path {
        self.directory
            .as_ref()
            .expect("the world is on disk")
            .path()
    }
}

/// Runs a test on stripes, on a store in memory, whose commits are confirmed almost at
/// once, and on one on disk, whose commits take longer than a step, so that what a tick
/// produced is still held when the next things happen.
fn on_stripes_in_memory_and_on_disk(test: fn(World)) {
    test(World::new(Shape::Stripes, false));
    test(World::new(Shape::Stripes, true));
}

/// As [`on_stripes_in_memory_and_on_disk`], with the gap.
fn on_the_gap_in_memory_and_on_disk(test: fn(World)) {
    test(World::new(Shape::Gap, false));
    test(World::new(Shape::Gap, true));
}

/// A region the test plays: a handle of its own at the store.
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

    /// Gives `chunk` back and waits for the answer to a flush behind the return, which
    /// comes only when the return is through (ADR-0011, section 3.3).
    fn give_back(&self, chunk: ChunkPos) {
        self.handle.request(StoreRequest::Return {
            chunks: vec![chunk],
        });
        self.handle.request(StoreRequest::Flush);
        match self.reply() {
            StoreReply::Flushed => {}
            other => panic!("expected the answer to a flush, got {other:?}"),
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
/// section 5.
#[derive(Default)]
struct Book {
    /// The edge that said hello on the link, once one has.
    edge: Option<EdgeId>,
    /// The number of the last subscription message sent on the link.
    asked: u64,
    /// Every subscription message that named a chunk, with its number, in order. The
    /// lists of the hello are number 0.
    said: BTreeMap<ChunkPos, Vec<(u64, Said)>>,
    /// Every answer read for a chunk, with its `ask`, in order.
    answered: BTreeMap<ChunkPos, Vec<(u64, Answer)>>,
    /// The entities the link has sent inputs for: those of its edge's own players.
    own: BTreeSet<EntityId>,
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

/// The edge's end of a link, with everything read from it so far.
struct Link {
    end: EdgeEnd,
    log: Vec<WorkerToEdge>,
    closed: bool,
    book: Book,
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
        }
    }

    fn send(&mut self, message: EdgeMessage) {
        self.book.sent = true;
        // Only the edge of a player sends what they do, with the entity they have.
        if let EdgeToWorker::Input { entity, .. } = &message.body {
            self.book.own.insert(*entity);
        }
        self.end
            .try_send(message)
            .expect("the link is open and has room");
    }

    fn plain(&mut self, body: EdgeToWorker) {
        self.send(EdgeMessage::unnumbered(body));
    }

    fn numbered(&mut self, number: u64, body: EdgeToWorker) {
        self.send(EdgeMessage {
            number: Some(number),
            body,
        });
    }

    /// Says hello as an edge does that has seen nothing of the region's outbox and
    /// keeps the `since` it was last told.
    fn hello(
        &mut self,
        edge: EdgeId,
        start: u64,
        players: Vec<PlayerId>,
        chunks: Vec<ChunkPos>,
        guests: Vec<ChunkPos>,
    ) {
        let since = SINCE.with(|since| since.borrow().get(&edge).copied().unwrap_or(0));
        self.hello_saying(edge, start, since, 0, players, chunks, guests);
    }

    /// Says hello with the `since` and the `seen` the test names.
    #[allow(clippy::too_many_arguments)]
    fn hello_saying(
        &mut self,
        edge: EdgeId,
        start: u64,
        since: u64,
        seen: u64,
        players: Vec<PlayerId>,
        chunks: Vec<ChunkPos>,
        guests: Vec<ChunkPos>,
    ) {
        self.book.edge = Some(edge);
        self.book.hello_first = Some(!self.book.sent);
        self.book.named = players.len();
        // A chunk in both lists is a viewer's (section 4.5).
        let only_guests: Vec<ChunkPos> = guests
            .iter()
            .filter(|chunk| !chunks.contains(chunk))
            .copied()
            .collect();
        self.book.note(0, Said::Asked(Kind::Viewer), &chunks);
        self.book.note(0, Said::Asked(Kind::Guest), &only_guests);
        self.plain(EdgeToWorker::Hello {
            edge,
            start,
            since,
            seen,
            players,
            chunks,
            guests,
        });
    }

    /// The number of the next subscription message on this link: they are counted from
    /// 1, apart from the numbers of what changes the region.
    fn next_ask(&self) -> u64 {
        self.book.asked + 1
    }

    /// Sends a subscription message with the number the test names, be it in order or
    /// not.
    fn say(&mut self, ask: u64, said: Said, chunks: Vec<ChunkPos>) {
        self.book.note(ask, said, &chunks);
        self.plain(subscription(ask, said, chunks));
    }

    /// Subscribes to `chunks` for a viewer of one of this region's players, and returns
    /// the number of the message.
    fn subscribe(&mut self, chunks: Vec<ChunkPos>) -> u64 {
        let ask = self.next_ask();
        self.say(ask, Said::Asked(Kind::Viewer), chunks);
        ask
    }

    /// Subscribes to `chunks` for a viewer of another region's player.
    fn subscribe_as_guest(&mut self, chunks: Vec<ChunkPos>) -> u64 {
        let ask = self.next_ask();
        self.say(ask, Said::Asked(Kind::Guest), chunks);
        ask
    }

    fn unsubscribe(&mut self, chunks: Vec<ChunkPos>) -> u64 {
        let ask = self.next_ask();
        self.say(ask, Said::Ended, chunks);
        ask
    }

    /// Subscribes as [`Link::subscribe`] does, on a link the worker may have closed by
    /// now, which the next read shows.
    fn try_subscribe(&mut self, chunks: Vec<ChunkPos>) {
        let ask = self.next_ask();
        let said = Said::Asked(Kind::Viewer);
        self.book.note(ask, said, &chunks);
        self.book.sent = true;
        let _ = self
            .end
            .try_send(EdgeMessage::unnumbered(subscription(ask, said, chunks)));
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

    /// Holds the link to rule 9 of section 5.4 once everything of the ticks so far is
    /// published: every asking that the link has not overtaken or ended is answered
    /// with its number, or its subscription was served when the message came. A chunk
    /// the store cannot read is the one exception.
    fn assert_answered(&self, unreadable: &[ChunkPos]) {
        for (chunk, said) in &self.book.said {
            if unreadable.contains(chunk) {
                continue;
            }
            let Some((number, Said::Asked(_))) = said.last() else {
                continue;
            };
            let answers = self.answers(*chunk);
            let answered = answers.iter().any(|(ask, _)| ask == number);
            let served = answers.iter().any(|(ask, answer)| {
                *answer == Answer::Snapshot
                    && ask < number
                    && !said
                        .iter()
                        .any(|(ended, what)| *what == Said::Ended && ask < ended && ended < number)
            });
            assert!(
                answered || served,
                "the asking {number} for {chunk:?} was neither answered nor overtaken (answers: {answers:?}): {}",
                brief(&self.log)
            );
        }
    }
}

/// The subscription message that says `said` of `chunks` with the number `ask`.
fn subscription(ask: u64, said: Said, chunks: Vec<ChunkPos>) -> EdgeToWorker {
    match said {
        Said::Asked(Kind::Viewer) => EdgeToWorker::Subscribe { ask, chunks },
        Said::Asked(Kind::Guest) => EdgeToWorker::SubscribeAsGuest { ask, chunks },
        Said::Ended => EdgeToWorker::Unsubscribe { ask, chunks },
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
/// 5.2. The resume is the first item there and is held to its place by [`check`].
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

/// Holds the message just read to what section 5 says of every link, whatever the test
/// is about.
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
            // One for each player the hello named, and one for every other stay the
            // region has for the edge (ADR-0014, section 3.7).
            assert!(
                matches!(welcome, Welcome::Superseded) || answers >= book.named,
                "a welcome that announces {answers} presence answers for {} players named: {}",
                book.named,
                context()
            );
            book.welcome = Some((index, entries, answers));
            if let (Welcome::Unknown { since, .. }, Some(edge)) = (welcome, book.edge) {
                SINCE.with(|told| told.borrow_mut().insert(edge, *since));
            }
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

    // Whom the region says it has for the edge is the edge's own as well: a stay that
    // a merge or a split put there, for which this link has sent nothing yet.
    if let WorkerToEdge::Presence {
        answer: Presence::Present { entity, .. },
        ..
    } = last
    {
        book.own.insert(*entity);
    }
    if let WorkerToEdge::TickDelta { events, .. } = last {
        assert!(!events.is_empty(), "a delta without events: {}", context());
        for event in events {
            if matches!(event, RegionEvent::EntityRemoved { .. }) {
                continue;
            }
            // Where its own player moved to, an edge is told wherever that is: it asks
            // for what the player sees by it. Only a player's own edge moves them.
            if matches!(event, RegionEvent::EntityMoved { entity, .. } if book.own.contains(entity))
            {
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
        "never happened: {what}; the region is at tick {}\n{logs}",
        runner.region().tick_number()
    );
}

/// Steps the runner until it has run the tick numbered `tick`.
fn run_to_tick(runner: &mut RegionRunner, links: &mut [&mut Link], tick: u64) {
    if runner.region().tick_number() >= tick {
        return;
    }
    run_until(runner, links, "the region reaching a tick", |runner, _| {
        runner.region().tick_number() >= tick
    });
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

/// Waits until everything of the ticks so far has been published on `links[through]`:
/// the link subscribes to a chunk of the neighbour's pinned area that nothing else
/// asks about, and the answer, which is part of a tick like everything else, shows
/// that every earlier tick is out.
fn sync(runner: &mut RegionRunner, links: &mut [&mut Link], through: usize) {
    static MARK: AtomicI32 = AtomicI32::new(0);
    let position = ChunkPos::new(1000 + MARK.fetch_add(1, Ordering::Relaxed), 77);
    let ask = links[through].subscribe(vec![position]);
    run_until(runner, links, "the answer that marks a tick", |_, links| {
        answer_to(&links[through].log, position, ask).is_some()
    });
    links[through].unsubscribe(vec![position]);
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

/// The first snapshot of `position`: its place in the log, its tick and its content.
fn snapshot(
    log: &[WorkerToEdge],
    position: ChunkPos,
) -> Option<(usize, u64, &Chunk, &Vec<EntityState>)> {
    log.iter()
        .enumerate()
        .find_map(|(index, message)| match message {
            WorkerToEdge::ChunkSnapshot {
                position: at,
                tick,
                chunk,
                entities,
                ..
            } if *at == position => Some((index, *tick, chunk, entities)),
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

/// Where and in which tick the link was told that `entity` is gone, and from which
/// chunk.
fn removal(log: &[WorkerToEdge], entity: EntityId) -> Option<(usize, u64, ChunkPos)> {
    events(log)
        .into_iter()
        .find_map(|(index, tick, event)| match event {
            RegionEvent::EntityRemoved {
                entity: gone,
                chunk,
            } if *gone == entity => Some((index, tick, *chunk)),
            _ => None,
        })
}

/// Where the link was told that `block` changed, and to what.
fn block_change(log: &[WorkerToEdge], block: BlockPos) -> Option<(usize, BlockState)> {
    events(log)
        .into_iter()
        .find_map(|(index, _, event)| match event {
            RegionEvent::BlockChanged { position, state } if *position == block => {
                Some((index, *state))
            }
            _ => None,
        })
}

/// Where the link was told that the entity of `id` came into existence.
fn entity_spawn(log: &[WorkerToEdge], id: PlayerId) -> Option<usize> {
    events(log)
        .into_iter()
        .find_map(|(index, _, event)| match event {
            RegionEvent::EntitySpawned(state) => {
                let EntityKind::Player { player, .. } = &state.kind;
                (*player == id).then_some(index)
            }
            _ => None,
        })
}

/// Where the link was told that `entity` moved, and to which x, in order.
fn moves(log: &[WorkerToEdge], entity: EntityId) -> Vec<(usize, f64)> {
    events(log)
        .into_iter()
        .filter_map(|(index, _, event)| match event {
            RegionEvent::EntityMoved {
                entity: who, pose, ..
            } if *who == entity => Some((index, pose.position.x)),
            _ => None,
        })
        .collect()
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

/// Where the link was told that `id` was let go, and to which region.
fn departure(log: &[WorkerToEdge], id: PlayerId) -> Option<(usize, RegionId, &PlayerTransfer)> {
    outbox(log)
        .into_iter()
        .find_map(|(index, _, entry)| match entry {
            Durable::Departed {
                player,
                transfer,
                to,
            } if *player == id => Some((index, *to, transfer)),
            _ => None,
        })
}

/// The last `applied` the link was told.
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

fn block_in(chunk: &Chunk, block: BlockPos) -> BlockState {
    let (x, z) = block.in_chunk();
    chunk.get(x, block.y, z).expect("the block is in the chunk")
}

fn join(id: PlayerId) -> EdgeToWorker {
    EdgeToWorker::PlayerJoin(PlayerJoin {
        player: id,
        name: format!("player-{}", id.0.as_u128()),
    })
}

/// The entity of the `n`th player to enter the region of `runner`, counted from 1, with
/// everyone who entered it anew: a region gives out the ids of its block in order. An
/// edge names the entity in a player's inputs once it was told that the player
/// spawned; a test that sends inputs behind a join without waiting for that knows the
/// entity this way.
fn nth_entity(runner: &RegionRunner, n: i32) -> EntityId {
    EntityId(runner.region().state().entity_ids.first.0 + n - 1)
}

fn input(id: PlayerId, entity: EntityId, number: u64, input: PlayerInput) -> EdgeToWorker {
    EdgeToWorker::Input {
        player: id,
        entity,
        number,
        input,
    }
}

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

/// A player as another region lets them go, standing at `x` on the line the tests walk
/// along, with an entity of that region's.
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

/// A new link of `edge` that has said hello with these lists and has been answered in
/// full: the welcome, an answer to every chunk of both lists and the progress of the
/// hello's tick are there.
fn greeted(
    runner: &mut RegionRunner,
    edge: EdgeId,
    start: u64,
    players: Vec<PlayerId>,
    chunks: Vec<ChunkPos>,
    guests: Vec<ChunkPos>,
) -> Link {
    let mut link = Link::attach(runner);
    link.hello(edge, start, players, chunks.clone(), guests.clone());
    run_until(
        runner,
        &mut [&mut link],
        "the answer to a hello",
        |_, links| {
            links[0].closed
                || (applied(&links[0].log).is_some()
                    && chunks
                        .iter()
                        .chain(&guests)
                        .all(|chunk| answer_to(&links[0].log, *chunk, 0).is_some()))
        },
    );
    assert!(!link.closed, "{}", brief(&link.log));
    link
}

/// A new link of `edge` that sees the home chunk.
fn established(runner: &mut RegionRunner, edge: EdgeId, start: u64) -> Link {
    greeted(runner, edge, start, Vec::new(), vec![HOME], Vec::new())
}

/// A new link of `edge` that is subscribed to nothing.
fn linked(runner: &mut RegionRunner, edge: EdgeId, start: u64) -> Link {
    greeted(runner, edge, start, Vec::new(), Vec::new(), Vec::new())
}

/// Sends a join with `number` and waits until the player is told they entered the world.
fn join_and_wait(
    runner: &mut RegionRunner,
    link: &mut Link,
    number: u64,
    id: PlayerId,
) -> EntityId {
    link.numbered(number, join(id));
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

/// Has the region the test plays claim `chunk` until it is granted, stepping the runner
/// in between: a return is not through when it is asked for.
fn claim_until_granted(
    runner: &mut RegionRunner,
    links: &mut [&mut Link],
    other: &Other,
    chunk: ChunkPos,
) {
    run_until(runner, links, "a claim being granted", |_, _| {
        other.claim(chunk).is_ok()
    });
}

/// Steps the runner until the region knows `knowledge` of `chunk`.
fn wait_knowledge(
    runner: &mut RegionRunner,
    links: &mut [&mut Link],
    chunk: ChunkPos,
    knowledge: Knowledge,
) {
    run_until(
        runner,
        links,
        "the region coming to know something of a chunk",
        |runner, _| runner.region().knowledge(chunk) == knowledge,
    );
}

/// Has the runner run `count` more ticks.
fn ticks(runner: &mut RegionRunner, links: &mut [&mut Link], count: u64) {
    for _ in 0..count {
        one_tick(runner, links);
    }
}

// ---------------------------------------------------------------------------------------
// R1. A viewer's subscription: a snapshot, or `Elsewhere` and nothing more
// ---------------------------------------------------------------------------------------

#[test]
fn a_viewers_subscription_to_an_own_chunk_is_answered_with_a_snapshot_that_has_its_ask() {
    on_stripes_in_memory_and_on_disk(
        a_viewers_subscription_to_an_own_chunk_is_answered_with_a_snapshot_that_has_its_ask_in,
    );
}

fn a_viewers_subscription_to_an_own_chunk_is_answered_with_a_snapshot_that_has_its_ask_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut e = linked(&mut runner, E, 5);
    // The asking gets a number that an answer cannot have by chance.
    sync(&mut runner, &mut [&mut e], 0);
    assert_eq!(runner.region().knowledge(OWN), Knowledge::Unknown);

    let ask = e.subscribe(vec![OWN]);
    assert!(ask > 1);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut e], 0, OWN, ask),
        Answer::Snapshot
    );
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(e.answers(OWN), vec![(ask, Answer::Snapshot)]);
    assert_eq!(runner.region().knowledge(OWN), Knowledge::Held);
    let (_, _, chunk, entities) = snapshot(&e.log, OWN).expect("waited for it");
    let grass = BlockPos::new(OWN.x * 16 + 3, GROUND, OWN.z * 16 + 3);
    assert_eq!(block_in(chunk, grass), blocks::GRASS_BLOCK);
    assert!(entities.is_empty());
    e.assert_answered(&[]);
}

#[test]
fn a_viewers_subscription_to_a_chunk_of_the_other_stripe_is_told_elsewhere_with_its_ask() {
    on_stripes_in_memory_and_on_disk(
        a_viewers_subscription_to_a_chunk_of_the_other_stripe_is_told_elsewhere_with_its_ask_in,
    );
}

fn a_viewers_subscription_to_a_chunk_of_the_other_stripe_is_told_elsewhere_with_its_ask_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut e = linked(&mut runner, E, 5);
    sync(&mut runner, &mut [&mut e], 0);

    let ask = e.subscribe(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut e], 0, NEXT, ask),
        Answer::Elsewhere(NEIGHBOUR)
    );
    // The subscription stays, and with it what the region believes; nothing is loaded
    // for it, and nothing more is said of it however long the link waits.
    ticks(&mut runner, &mut [&mut e], 10);
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(e.answers(NEXT), vec![(ask, Answer::Elsewhere(NEIGHBOUR))]);
    assert_eq!(
        runner.region().knowledge(NEXT),
        Knowledge::Foreign(NEIGHBOUR)
    );
    assert_eq!(runner.region().loaded_chunk_count(), 0);
    assert!(events(&e.log).is_empty(), "{}", brief(&e.log));
    e.assert_answered(&[]);
}

#[test]
fn no_event_of_a_chunk_comes_after_its_elsewhere() {
    let mut world = World::stripes();
    let mut runner = world.open();
    let mut e = established(&mut runner, E, 5);
    let mut watcher = linked(&mut runner, F, 5);
    let ask = watcher.subscribe(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut watcher, &mut e], 0, NEXT, ask),
        Answer::Elsewhere(NEIGHBOUR)
    );

    // A player walks into the chunk. Whoever sees the chunk they come from sees the
    // step; the link that was told the chunk is elsewhere sees nothing of it.
    let entity = join_and_wait(&mut runner, &mut e, 1, player(1));
    e.numbered(2, input(player(1), entity, 1, move_to(16.5)));
    run_until(
        &mut runner,
        &mut [&mut e, &mut watcher],
        "the player being let go",
        |_, links| departure(&links[0].log, player(1)).is_some(),
    );
    settle(&mut runner, &mut [&mut e, &mut watcher]);
    assert_eq!(
        moves(&e.log, entity).last().map(|(_, x)| *x),
        Some(16.5),
        "the step is an event of the chunk it left too: {}",
        brief(&e.log)
    );
    assert!(events(&watcher.log).is_empty(), "{}", brief(&watcher.log));
    assert_eq!(
        watcher.answers(NEXT),
        vec![(ask, Answer::Elsewhere(NEIGHBOUR))]
    );
}

#[test]
fn a_second_subscribe_for_a_chunk_that_was_told_elsewhere_is_answered_with_the_second_number() {
    on_stripes_in_memory_and_on_disk(
        a_second_subscribe_for_a_chunk_that_was_told_elsewhere_is_answered_with_the_second_number_in,
    );
}

fn a_second_subscribe_for_a_chunk_that_was_told_elsewhere_is_answered_with_the_second_number_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut e = linked(&mut runner, E, 5);
    let first = e.subscribe(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut e], 0, NEXT, first),
        Answer::Elsewhere(NEIGHBOUR)
    );
    settle(&mut runner, &mut [&mut e]);

    let second = e.subscribe(vec![NEXT]);
    assert!(second > first);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut e], 0, NEXT, second),
        Answer::Elsewhere(NEIGHBOUR)
    );
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(
        e.answers(NEXT),
        vec![
            (first, Answer::Elsewhere(NEIGHBOUR)),
            (second, Answer::Elsewhere(NEIGHBOUR))
        ]
    );
    assert_eq!(
        runner.region().knowledge(NEXT),
        Knowledge::Foreign(NEIGHBOUR)
    );
    e.assert_answered(&[]);
}

// ---------------------------------------------------------------------------------------
// Section 4.4 and rule 14: asking again when the region has heard otherwise since
// ---------------------------------------------------------------------------------------

/// The region comes to hold a chunk through another link's asking. The link that was
/// told `Elsewhere` hears nothing of the chunk until it asks, and then gets the
/// snapshot.
#[test]
fn a_link_told_elsewhere_hears_nothing_of_a_chunk_the_region_came_to_hold_until_it_asks_again() {
    on_the_gap_in_memory_and_on_disk(
        a_link_told_elsewhere_hears_nothing_of_a_chunk_the_region_came_to_hold_until_it_asks_again_in,
    );
}

fn a_link_told_elsewhere_hears_nothing_of_a_chunk_the_region_came_to_hold_until_it_asks_again_in(
    mut world: World,
) {
    let neighbour = world.neighbour();
    assert_eq!(neighbour.claim(NEXT), Ok(()));
    let mut runner = world.open();
    let mut first = established(&mut runner, E, 5);
    let mut second = linked(&mut runner, F, 5);
    let asked = first.subscribe(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut first, &mut second], 0, NEXT, asked),
        Answer::Elsewhere(NEIGHBOUR)
    );

    // The neighbour gives the chunk back. Nobody tells the region (rule 13): the second
    // link is told what the region believes, and has it ask the store by asking again.
    neighbour.give_back(NEXT);
    let stale = second.subscribe(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut first, &mut second], 1, NEXT, stale),
        Answer::Elsewhere(NEIGHBOUR)
    );
    let again = second.subscribe(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut first, &mut second], 1, NEXT, again),
        Answer::Snapshot
    );
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Held);

    // Something happens in the chunk, which the first link's own player does.
    join_and_wait(&mut runner, &mut first, 1, player(1));
    first.numbered(
        2,
        input(player(1), nth_entity(&runner, 1), 1, dig(BEYOND, 1)),
    );
    run_until(
        &mut runner,
        &mut [&mut first, &mut second],
        "the link that is served seeing the block change",
        |_, links| block_change(&links[1].log, BEYOND).is_some(),
    );
    settle(&mut runner, &mut [&mut first, &mut second]);
    assert_eq!(
        first.answers(NEXT),
        vec![(asked, Answer::Elsewhere(NEIGHBOUR))],
        "{}",
        brief(&first.log)
    );
    assert!(block_change(&first.log, BEYOND).is_none());
    assert!(
        acknowledged(&first.log, player(1), 1).is_some(),
        "the region holds the chunk and has handled the dig itself: {}",
        brief(&first.log)
    );

    // Now it asks, and is answered with what the region has come to know.
    let late = first.subscribe(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut first, &mut second], 0, NEXT, late),
        Answer::Snapshot
    );
    let (_, _, chunk, _) = snapshot(&first.log, NEXT).expect("waited for it");
    assert_eq!(block_in(chunk, BEYOND), blocks::AIR);
    let further = BEYOND.offset(0, 0, 1);
    first.numbered(
        3,
        input(player(1), nth_entity(&runner, 1), 2, dig(further, 2)),
    );
    run_until(
        &mut runner,
        &mut [&mut first, &mut second],
        "both links seeing the next block change",
        |_, links| {
            block_change(&links[0].log, further).is_some()
                && block_change(&links[1].log, further).is_some()
        },
    );
    first.assert_answered(&[]);
    second.assert_answered(&[]);
}

/// The region comes to believe another region than it told the link. Asked again, it
/// answers with that, without asking the store: the belief is newer than the doubt
/// (section 1.3). Only when the link doubts what it was told last does the region ask.
#[test]
fn a_link_told_elsewhere_that_asks_again_is_told_the_region_the_region_has_come_to_believe() {
    let mut world = World::gap();
    let neighbour = world.neighbour();
    let western = world.other(WESTERN);
    assert_eq!(neighbour.claim(NEXT), Ok(()));
    let mut runner = world.open();
    let mut first = linked(&mut runner, E, 5);
    let mut second = linked(&mut runner, F, 5);
    let asked = first.subscribe(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut first, &mut second], 0, NEXT, asked),
        Answer::Elsewhere(NEIGHBOUR)
    );

    // The chunk goes from the neighbour to the region in the west, and the second link
    // has the region find out.
    neighbour.give_back(NEXT);
    assert_eq!(western.claim(NEXT), Ok(()));
    let stale = second.subscribe(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut first, &mut second], 1, NEXT, stale),
        Answer::Elsewhere(NEIGHBOUR)
    );
    let again = second.subscribe(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut first, &mut second], 1, NEXT, again),
        Answer::Elsewhere(WESTERN)
    );
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Foreign(WESTERN));
    ticks(&mut runner, &mut [&mut first, &mut second], 5);
    settle(&mut runner, &mut [&mut first, &mut second]);
    assert_eq!(
        first.answers(NEXT),
        vec![(asked, Answer::Elsewhere(NEIGHBOUR))],
        "the first link hears nothing until it asks: {}",
        brief(&first.log)
    );

    // The western region gives the chunk back as well. The first link doubts the
    // neighbour, which the region no longer believes anyway: it answers with what it
    // believes now and does not ask the store, which would grant it the chunk.
    western.give_back(NEXT);
    let late = first.subscribe(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut first, &mut second], 0, NEXT, late),
        Answer::Elsewhere(WESTERN)
    );
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Foreign(WESTERN));

    // Asking once more doubts the western region, and that is asked of the store.
    let last = first.subscribe(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut first, &mut second], 0, NEXT, last),
        Answer::Snapshot
    );
    settle(&mut runner, &mut [&mut first, &mut second]);
    assert_eq!(
        second.answers(NEXT),
        vec![
            (stale, Answer::Elsewhere(NEIGHBOUR)),
            (again, Answer::Elsewhere(WESTERN))
        ],
        "the second link has not asked again and hears nothing: {}",
        brief(&second.log)
    );
    first.assert_answered(&[]);
    second.assert_answered(&[]);
}

// ---------------------------------------------------------------------------------------
// R2. A guest's subscription: a snapshot or `NotMine`
// ---------------------------------------------------------------------------------------

#[test]
fn a_guests_subscription_to_a_chunk_of_the_own_stripe_is_answered_with_a_snapshot() {
    on_stripes_in_memory_and_on_disk(
        a_guests_subscription_to_a_chunk_of_the_own_stripe_is_answered_with_a_snapshot_in,
    );
}

fn a_guests_subscription_to_a_chunk_of_the_own_stripe_is_answered_with_a_snapshot_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut e = linked(&mut runner, E, 5);
    sync(&mut runner, &mut [&mut e], 0);

    let ask = e.subscribe_as_guest(vec![OWN]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut e], 0, OWN, ask),
        Answer::Snapshot
    );
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(e.answers(OWN), vec![(ask, Answer::Snapshot)]);
    // In an area the region is pinned to, a guest is a reason to claim.
    assert_eq!(runner.region().knowledge(OWN), Knowledge::Held);
    e.assert_answered(&[]);
}

#[test]
fn a_guests_subscription_to_a_chunk_of_the_other_stripe_is_told_not_mine() {
    on_stripes_in_memory_and_on_disk(
        a_guests_subscription_to_a_chunk_of_the_other_stripe_is_told_not_mine_in,
    );
}

fn a_guests_subscription_to_a_chunk_of_the_other_stripe_is_told_not_mine_in(mut world: World) {
    let mut runner = world.open();
    let mut e = linked(&mut runner, E, 5);
    sync(&mut runner, &mut [&mut e], 0);

    let ask = e.subscribe_as_guest(vec![OTHER]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut e], 0, OTHER, ask),
        Answer::NotMine
    );
    // The subscription is forgotten: nothing more comes for it, and the region has not
    // asked about the chunk because of it.
    ticks(&mut runner, &mut [&mut e], 5);
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(e.answers(OTHER), vec![(ask, Answer::NotMine)]);
    assert_eq!(runner.region().knowledge(OTHER), Knowledge::Unknown);
    e.assert_answered(&[]);
}

#[test]
fn with_the_gap_a_guests_subscription_to_a_free_chunk_is_told_not_mine_and_the_chunk_stays_free() {
    on_the_gap_in_memory_and_on_disk(
        with_the_gap_a_guests_subscription_to_a_free_chunk_is_told_not_mine_and_the_chunk_stays_free_in,
    );
}

fn with_the_gap_a_guests_subscription_to_a_free_chunk_is_told_not_mine_and_the_chunk_stays_free_in(
    mut world: World,
) {
    let neighbour = world.neighbour();
    let mut runner = world.open();
    let mut e = linked(&mut runner, E, 5);

    // A free chunk and one of the neighbour's area: the region is pinned to nothing, so
    // a guest is no reason to claim either.
    let ask = e.subscribe_as_guest(vec![NEXT, EAST]);
    for chunk in [NEXT, EAST] {
        assert_eq!(
            wait_answer(&mut runner, &mut [&mut e], 0, chunk, ask),
            Answer::NotMine
        );
    }
    ticks(&mut runner, &mut [&mut e], 5);
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(e.answers(NEXT), vec![(ask, Answer::NotMine)]);
    assert_eq!(e.answers(EAST), vec![(ask, Answer::NotMine)]);
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Unknown);
    assert_eq!(neighbour.claim(NEXT), Ok(()));
}

// ---------------------------------------------------------------------------------------
// R3 and R4. With the gap: a free chunk is taken for a viewer, and one the neighbour
// holds is elsewhere until the link asks again
// ---------------------------------------------------------------------------------------

#[test]
fn with_the_gap_a_free_chunk_is_served_to_a_viewer_and_is_the_regions_after_a_crash() {
    on_the_gap_in_memory_and_on_disk(
        with_the_gap_a_free_chunk_is_served_to_a_viewer_and_is_the_regions_after_a_crash_in,
    );
}

fn with_the_gap_a_free_chunk_is_served_to_a_viewer_and_is_the_regions_after_a_crash_in(
    mut world: World,
) {
    let neighbour = world.neighbour();
    let mut runner = world.open();
    let mut e = linked(&mut runner, E, 5);
    let ask = e.subscribe(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut e], 0, NEXT, ask),
        Answer::Snapshot
    );
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(e.answers(NEXT), vec![(ask, Answer::Snapshot)]);
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Held);
    assert_eq!(neighbour.claim(NEXT), Err(world.region()));

    // The crash: the store says what the region holds, and the next owner serves it.
    let (handle, restored) = world.open_raw();
    let held: Vec<ChunkPos> = restored.held.iter().map(|(chunk, _)| *chunk).collect();
    assert_eq!(held, vec![HOME, NEXT]);
    assert!(restored.pinned.is_empty());
    let mut next = RegionRunner::restore(config(0), handle, restored).expect("readable");
    assert_eq!(next.region().knowledge(NEXT), Knowledge::Held);
    let again = greeted(&mut next, E, 5, Vec::new(), vec![NEXT], Vec::new());
    assert_eq!(again.answers(NEXT), vec![(0, Answer::Snapshot)]);
}

#[test]
fn with_the_gap_a_chunk_the_neighbour_held_is_served_once_it_is_returned_and_asked_for_again() {
    on_the_gap_in_memory_and_on_disk(
        with_the_gap_a_chunk_the_neighbour_held_is_served_once_it_is_returned_and_asked_for_again_in,
    );
}

fn with_the_gap_a_chunk_the_neighbour_held_is_served_once_it_is_returned_and_asked_for_again_in(
    mut world: World,
) {
    let neighbour = world.neighbour();
    assert_eq!(neighbour.claim(NEXT), Ok(()));
    let mut runner = world.open();
    let mut e = linked(&mut runner, E, 5);
    let first = e.subscribe(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut e], 0, NEXT, first),
        Answer::Elsewhere(NEIGHBOUR)
    );

    // The return is through when the flush behind it is answered. The region does not
    // learn of it by itself.
    neighbour.give_back(NEXT);
    ticks(&mut runner, &mut [&mut e], 5);
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(e.answers(NEXT), vec![(first, Answer::Elsewhere(NEIGHBOUR))]);
    assert_eq!(
        runner.region().knowledge(NEXT),
        Knowledge::Foreign(NEIGHBOUR)
    );

    let second = e.subscribe(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut e], 0, NEXT, second),
        Answer::Snapshot
    );
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(
        e.answers(NEXT),
        vec![
            (first, Answer::Elsewhere(NEIGHBOUR)),
            (second, Answer::Snapshot)
        ]
    );
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Held);
    assert_eq!(neighbour.claim(NEXT), Err(world.region()));
    e.assert_answered(&[]);
}

// ---------------------------------------------------------------------------------------
// R5. A guest keeps a chunk, and the time before a return
// ---------------------------------------------------------------------------------------

/// With the gap: edge E's link with a viewer's subscription to the home chunk and to
/// the free chunk east of it, both served, and player 1 in the home chunk.
fn granted_for_a_viewer(runner: &mut RegionRunner) -> (Link, EntityId) {
    let mut e = greeted(runner, E, 5, Vec::new(), vec![HOME, NEXT], Vec::new());
    assert_eq!(e.answers(NEXT), vec![(0, Answer::Snapshot)]);
    let entity = join_and_wait(runner, &mut e, 1, player(1));
    (e, entity)
}

/// The chunk of the snapshot that answered the asking `ask` for `position`.
fn snapshot_for(log: &[WorkerToEdge], position: ChunkPos, ask: u64) -> Option<&Chunk> {
    log.iter().find_map(|message| match message {
        WorkerToEdge::ChunkSnapshot {
            position: at,
            ask: number,
            chunk,
            ..
        } if *at == position && *number == ask => Some(chunk),
        _ => None,
    })
}

#[test]
fn a_chunk_granted_for_a_viewer_stays_served_and_the_regions_for_as_long_as_a_guest_is_subscribed()
{
    on_the_gap_in_memory_and_on_disk(
        a_chunk_granted_for_a_viewer_stays_served_and_the_regions_for_as_long_as_a_guest_is_subscribed_in,
    );
}

fn a_chunk_granted_for_a_viewer_stays_served_and_the_regions_for_as_long_as_a_guest_is_subscribed_in(
    mut world: World,
) {
    let neighbour = world.neighbour();
    let mut runner = world.open();
    let (mut e, _) = granted_for_a_viewer(&mut runner);

    e.subscribe_as_guest(vec![NEXT]);
    for _ in 0..30 {
        one_tick(&mut runner, &mut [&mut e]);
        assert_eq!(runner.region().knowledge(NEXT), Knowledge::Held);
        assert_eq!(neighbour.claim(NEXT), Err(world.region()));
    }
    e.numbered(
        2,
        input(player(1), nth_entity(&runner, 1), 1, dig(BEYOND, 1)),
    );
    run_until(
        &mut runner,
        &mut [&mut e],
        "an event of the chunk reaching the guest",
        |_, links| block_change(&links[0].log, BEYOND).is_some(),
    );
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(
        e.answers(NEXT),
        vec![(0, Answer::Snapshot)],
        "neither a second snapshot nor a NotMine: {}",
        brief(&e.log)
    );
    assert_eq!(neighbour.claim(NEXT), Err(world.region()));

    // Nothing uses the chunk any more, and with no time before a return it goes.
    e.unsubscribe(vec![NEXT]);
    claim_until_granted(&mut runner, &mut [&mut e], &neighbour, NEXT);
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Unknown);
    assert_eq!(block_in(&neighbour.load(NEXT), BEYOND), blocks::AIR);
    e.assert_answered(&[]);
}

/// Section 1.3: a chunk is returned at the end of a tick if nothing uses it then and
/// nothing used it at the end of any of the `return_after` ticks before.
#[test]
fn a_guests_chunk_goes_with_the_fortieth_tick_after_the_first_tick_without_use() {
    let mut world = World::gap();
    let neighbour = world.neighbour();
    let mut runner = world.open_returning_after(40);
    let (mut e, _) = granted_for_a_viewer(&mut runner);
    e.subscribe_as_guest(vec![NEXT]);
    ticks(&mut runner, &mut [&mut e], 5);

    // The `Unsubscribe` is on the link when the region has run `before` ticks, so the
    // tick after is the first in which nothing uses the chunk.
    let before = runner.region().tick_number();
    e.unsubscribe(vec![NEXT]);
    for tick in before + 1..=before + 40 {
        assert_eq!(one_tick(&mut runner, &mut [&mut e]), tick);
        assert_eq!(
            runner.region().knowledge(NEXT),
            Knowledge::Held,
            "given back {} ticks after the unsubscribe",
            tick - before
        );
        assert_eq!(neighbour.claim(NEXT), Err(world.region()));
    }
    one_tick(&mut runner, &mut [&mut e]);
    assert_eq!(
        runner.region().knowledge(NEXT),
        Knowledge::Unknown,
        "not given back with the fortieth tick after the first without use"
    );
    assert_eq!(runner.region().knowledge(HOME), Knowledge::Held);
    claim_until_granted(&mut runner, &mut [&mut e], &neighbour, NEXT);
}

#[test]
fn a_guest_that_comes_back_for_a_tick_starts_the_time_before_a_return_anew() {
    let mut world = World::gap();
    let neighbour = world.neighbour();
    let mut runner = world.open_returning_after(40);
    let (mut e, _) = granted_for_a_viewer(&mut runner);
    e.unsubscribe(vec![NEXT]);
    ticks(&mut runner, &mut [&mut e], 20);

    // A guest for the length of one tick, half-way through the time.
    e.subscribe_as_guest(vec![NEXT]);
    one_tick(&mut runner, &mut [&mut e]);
    let before = runner.region().tick_number();
    e.unsubscribe(vec![NEXT]);
    for tick in before + 1..=before + 40 {
        assert_eq!(one_tick(&mut runner, &mut [&mut e]), tick);
        assert_eq!(
            runner.region().knowledge(NEXT),
            Knowledge::Held,
            "given back {} ticks after the guest left",
            tick - before
        );
    }
    assert_eq!(neighbour.claim(NEXT), Err(world.region()));
    one_tick(&mut runner, &mut [&mut e]);
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Unknown);
    claim_until_granted(&mut runner, &mut [&mut e], &neighbour, NEXT);
}

/// What counts is what uses a chunk at the end of a tick. A subscription that one tick
/// takes and ends is on the chunk at the end of no tick, and leaves the count alone.
#[test]
fn a_subscription_made_and_ended_within_one_tick_does_not_start_the_time_before_a_return_anew() {
    let mut world = World::gap();
    let mut runner = world.open_returning_after(40);
    let (mut e, _) = granted_for_a_viewer(&mut runner);
    let before = runner.region().tick_number();
    e.unsubscribe(vec![NEXT]);
    ticks(&mut runner, &mut [&mut e], 20);
    e.subscribe(vec![NEXT]);
    e.unsubscribe(vec![NEXT]);
    one_tick(&mut runner, &mut [&mut e]);
    e.subscribe_as_guest(vec![NEXT]);
    e.unsubscribe(vec![NEXT]);
    run_to_tick(&mut runner, &mut [&mut e], before + 40);
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Held);
    one_tick(&mut runner, &mut [&mut e]);
    assert_eq!(
        runner.region().knowledge(NEXT),
        Knowledge::Unknown,
        "not given back with the fortieth tick after the first without use"
    );
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(e.answers(NEXT), vec![(0, Answer::Snapshot)]);
}

// ---------------------------------------------------------------------------------------
// R6. A chunk leaves saved
// ---------------------------------------------------------------------------------------

#[test]
fn a_chunk_that_leaves_the_region_is_loaded_by_the_next_holder_with_the_block_broken() {
    on_the_gap_in_memory_and_on_disk(
        a_chunk_that_leaves_the_region_is_loaded_by_the_next_holder_with_the_block_broken_in,
    );
}

fn a_chunk_that_leaves_the_region_is_loaded_by_the_next_holder_with_the_block_broken_in(
    mut world: World,
) {
    let neighbour = world.neighbour();
    let mut runner = world.open();
    let (mut e, _) = granted_for_a_viewer(&mut runner);
    e.numbered(
        2,
        input(player(1), nth_entity(&runner, 1), 1, dig(BEYOND, 1)),
    );
    run_until(
        &mut runner,
        &mut [&mut e],
        "the dig being acknowledged",
        |_, links| acknowledged(&links[0].log, player(1), 1).is_some(),
    );
    assert_eq!(
        block_change(&e.log, BEYOND).map(|(_, state)| state),
        Some(blocks::AIR)
    );

    e.unsubscribe(vec![NEXT]);
    claim_until_granted(&mut runner, &mut [&mut e], &neighbour, NEXT);
    assert_eq!(block_in(&neighbour.load(NEXT), BEYOND), blocks::AIR);
}

/// The change is made in the tick right before the one that takes the last ticket, so
/// on disk its commit is not confirmed when the chunk is saved and given back.
#[test]
fn a_change_of_the_tick_before_the_last_ticket_goes_is_in_the_chunk_the_next_holder_loads() {
    on_the_gap_in_memory_and_on_disk(
        a_change_of_the_tick_before_the_last_ticket_goes_is_in_the_chunk_the_next_holder_loads_in,
    );
}

fn a_change_of_the_tick_before_the_last_ticket_goes_is_in_the_chunk_the_next_holder_loads_in(
    mut world: World,
) {
    let neighbour = world.neighbour();
    let mut runner = world.open();
    let (mut e, _) = granted_for_a_viewer(&mut runner);
    e.numbered(
        2,
        input(player(1), nth_entity(&runner, 1), 1, dig(BEYOND, 1)),
    );
    one_tick(&mut runner, &mut [&mut e]);
    assert_eq!(
        runner
            .region()
            .chunk(NEXT)
            .map(|chunk| block_in(chunk, BEYOND)),
        Some(blocks::AIR),
        "the tick that took the dig broke the block"
    );
    e.unsubscribe(vec![NEXT]);
    claim_until_granted(&mut runner, &mut [&mut e], &neighbour, NEXT);
    assert_eq!(block_in(&neighbour.load(NEXT), BEYOND), blocks::AIR);
}

/// Section 3: a loaded chunk is dropped at the start of the tick that takes its last
/// ticket, before anything of that tick can change it. A dig that the same tick takes
/// finds no block, is acknowledged, and is in no stored chunk.
#[test]
fn a_dig_in_the_tick_that_takes_the_last_ticket_changes_nothing_and_the_chunk_leaves_as_it_was() {
    on_the_gap_in_memory_and_on_disk(
        a_dig_in_the_tick_that_takes_the_last_ticket_changes_nothing_and_the_chunk_leaves_as_it_was_in,
    );
}

fn a_dig_in_the_tick_that_takes_the_last_ticket_changes_nothing_and_the_chunk_leaves_as_it_was_in(
    mut world: World,
) {
    let neighbour = world.neighbour();
    let mut runner = world.open();
    let (mut e, _) = granted_for_a_viewer(&mut runner);
    let mut watcher = established(&mut runner, F, 5);
    e.numbered(
        2,
        input(player(1), nth_entity(&runner, 1), 1, dig(BEYOND, 1)),
    );
    e.unsubscribe(vec![NEXT]);
    run_until(
        &mut runner,
        &mut [&mut e, &mut watcher],
        "the dig being acknowledged",
        |_, links| acknowledged(&links[0].log, player(1), 1).is_some(),
    );
    claim_until_granted(&mut runner, &mut [&mut e, &mut watcher], &neighbour, NEXT);
    assert_eq!(
        block_in(&neighbour.load(NEXT), BEYOND),
        blocks::GRASS_BLOCK,
        "a change nobody was told of is in the stored chunk"
    );
    settle(&mut runner, &mut [&mut e, &mut watcher]);
    assert!(block_change(&e.log, BEYOND).is_none());
    assert!(block_change(&watcher.log, BEYOND).is_none());
}

/// Returns with a short time before them: the chunk is changed, let go of, and asked
/// for again after a few ticks, in which the return was asked for, or called off by the
/// new claim, or not made yet. However it went, the snapshot has every change.
#[test]
fn a_chunk_that_is_let_go_of_and_asked_for_again_comes_back_with_every_change() {
    for on_disk in [false, true] {
        for return_after in 0..=2 {
            for pause in 0..=4 {
                a_chunk_that_is_let_go_of_and_asked_for_again_comes_back_with_every_change_in(
                    World::new(Shape::Gap, on_disk),
                    return_after,
                    pause,
                );
            }
        }
    }
}

fn a_chunk_that_is_let_go_of_and_asked_for_again_comes_back_with_every_change_in(
    mut world: World,
    return_after: u64,
    pause: u64,
) {
    let case = format!("with {return_after} ticks before a return and {pause} between");
    let neighbour = world.neighbour();
    let mut runner = world.open_returning_after(return_after);
    let (mut e, _) = granted_for_a_viewer(&mut runner);
    let blocks_dug: Vec<BlockPos> = (0..3).map(|round| BEYOND.offset(0, 0, round)).collect();
    for (round, block) in blocks_dug.iter().enumerate() {
        let sequence = 1 + round as i32;
        e.numbered(
            2 + round as u64,
            input(
                player(1),
                nth_entity(&runner, 1),
                1 + round as u64,
                dig(*block, sequence),
            ),
        );
        run_until(
            &mut runner,
            &mut [&mut e],
            "the dig being shown and acknowledged",
            |_, links| {
                block_change(&links[0].log, *block).is_some()
                    && acknowledged(&links[0].log, player(1), sequence).is_some()
            },
        );
        e.unsubscribe(vec![NEXT]);
        ticks(&mut runner, &mut [&mut e], pause);
        let ask = e.subscribe(vec![NEXT]);
        assert_eq!(
            wait_answer(&mut runner, &mut [&mut e], 0, NEXT, ask),
            Answer::Snapshot,
            "{case}"
        );
        let chunk = snapshot_for(&e.log, NEXT, ask).expect("waited for it");
        for dug in &blocks_dug[..=round] {
            assert_eq!(
                block_in(chunk, *dug),
                blocks::AIR,
                "{dug:?} is back in round {round}, {case}"
            );
        }
        for whole in &blocks_dug[round + 1..] {
            assert_eq!(block_in(chunk, *whole), blocks::GRASS_BLOCK, "{case}");
        }
    }
    e.unsubscribe(vec![NEXT]);
    claim_until_granted(&mut runner, &mut [&mut e], &neighbour, NEXT);
    let stored = neighbour.load(NEXT);
    for dug in &blocks_dug {
        assert_eq!(block_in(&stored, *dug), blocks::AIR, "{case}");
    }
    e.assert_answered(&[]);
}

/// Opens the region under test again after a crash and has whoever holds `chunk` then
/// load it: the test's neighbour, if its claim is granted, and else the region itself,
/// for a new link of edge E. Returns the chunk and the region's next runner.
fn loaded_by_whoever_holds_it(
    world: &mut World,
    neighbour: &Other,
    chunk: ChunkPos,
) -> (Chunk, RegionRunner) {
    let (handle, restored) = world.open_raw();
    let ours = restored.held.iter().any(|(held, _)| *held == chunk);
    let mut next = RegionRunner::restore(config(0), handle, restored).expect("readable");
    match neighbour.claim(chunk) {
        Ok(()) => {
            assert!(!ours, "the store granted a chunk it says the region holds");
            (neighbour.load(chunk), next)
        }
        Err(holder) => {
            assert_eq!(holder, world.region());
            assert!(
                ours,
                "the region holds a chunk the store did not tell it of"
            );
            let link = greeted(&mut next, E, 5, Vec::new(), vec![chunk], Vec::new());
            let loaded = snapshot_for(&link.log, chunk, 0)
                .unwrap_or_else(|| panic!("no snapshot: {}", brief(&link.log)))
                .clone();
            (loaded, next)
        }
    }
}

/// The store may have dropped the return with the old session, or have done it. How
/// many ticks the region ran behind the `Unsubscribe` decides which.
#[test]
fn after_a_crash_behind_the_unsubscribe_whoever_holds_the_chunk_loads_it_with_the_block_broken() {
    for on_disk in [false, true] {
        for ticks_behind in 0..=3 {
            let mut world = World::new(Shape::Gap, on_disk);
            let neighbour = world.neighbour();
            let mut runner = world.open();
            let (mut e, _) = granted_for_a_viewer(&mut runner);
            e.numbered(
                2,
                input(player(1), nth_entity(&runner, 1), 1, dig(BEYOND, 1)),
            );
            run_until(
                &mut runner,
                &mut [&mut e],
                "the dig being acknowledged",
                |_, links| acknowledged(&links[0].log, player(1), 1).is_some(),
            );
            e.unsubscribe(vec![NEXT]);
            ticks(&mut runner, &mut [&mut e], ticks_behind);

            let (chunk, next) = loaded_by_whoever_holds_it(&mut world, &neighbour, NEXT);
            assert_eq!(
                block_in(&chunk, BEYOND),
                blocks::AIR,
                "{ticks_behind} ticks behind the unsubscribe, on disk: {on_disk}"
            );
            assert_eq!(
                next.region()
                    .player_state(player(1))
                    .and_then(|state| state.handled),
                Some(1)
            );
        }
    }
}

/// The dig is not confirmed when the region crashes. Whether its tick is part of what
/// the region is restored with or not, the stored chunk agrees with the restored state.
#[test]
fn after_a_crash_the_stored_chunk_agrees_with_what_the_restored_region_has_handled() {
    for on_disk in [false, true] {
        for ticks_behind in 0..=3 {
            let mut world = World::new(Shape::Gap, on_disk);
            let neighbour = world.neighbour();
            let mut runner = world.open();
            let (mut e, _) = granted_for_a_viewer(&mut runner);
            e.numbered(
                2,
                input(player(1), nth_entity(&runner, 1), 1, dig(BEYOND, 1)),
            );
            one_tick(&mut runner, &mut [&mut e]);
            e.unsubscribe(vec![NEXT]);
            ticks(&mut runner, &mut [&mut e], ticks_behind);

            let (chunk, next) = loaded_by_whoever_holds_it(&mut world, &neighbour, NEXT);
            let handled = next
                .region()
                .player_state(player(1))
                .and_then(|state| state.handled);
            let expected = if handled == Some(1) {
                blocks::AIR
            } else {
                blocks::GRASS_BLOCK
            };
            assert_eq!(
                block_in(&chunk, BEYOND),
                expected,
                "the restored region has handled {handled:?}; {ticks_behind} ticks behind the unsubscribe, on disk: {on_disk}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// R7 to R9. Players and blocks at the line between two stripes
// ---------------------------------------------------------------------------------------

#[test]
fn a_player_who_walks_into_a_chunk_believed_the_neighbours_is_let_go_in_the_tick_of_the_step() {
    on_stripes_in_memory_and_on_disk(
        a_player_who_walks_into_a_chunk_believed_the_neighbours_is_let_go_in_the_tick_of_the_step_in,
    );
}

fn a_player_who_walks_into_a_chunk_believed_the_neighbours_is_let_go_in_the_tick_of_the_step_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut e = greeted(&mut runner, E, 5, Vec::new(), vec![HOME, NEXT], Vec::new());
    assert_eq!(e.answers(NEXT), vec![(0, Answer::Elsewhere(NEIGHBOUR))]);
    let entity = join_and_wait(&mut runner, &mut e, 1, player(1));

    e.numbered(2, input(player(1), entity, 1, move_to(16.5)));
    wait_applied(&mut runner, &mut [&mut e], 0, 2);
    let (at, to, transfer) = departure(&e.log, player(1))
        .unwrap_or_else(|| panic!("not let go in the tick of the step: {}", brief(&e.log)));
    assert_eq!(to, NEIGHBOUR);
    assert_eq!(transfer.entity_id, entity);
    assert_eq!(transfer.pose.position.x, 16.5);
    assert_eq!(transfer.last_input, 1);
    let told = progress_to(&e.log, 2).expect("waited for it");
    assert!(at < told, "{}", brief(&e.log));
    assert!(runner.region().player(player(1)).is_none());
}

#[test]
fn a_player_who_walks_into_the_other_stripe_is_let_go_also_when_nothing_had_asked_about_it() {
    on_stripes_in_memory_and_on_disk(
        a_player_who_walks_into_the_other_stripe_is_let_go_also_when_nothing_had_asked_about_it_in,
    );
}

fn a_player_who_walks_into_the_other_stripe_is_let_go_also_when_nothing_had_asked_about_it_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut e = established(&mut runner, E, 5);
    let entity = join_and_wait(&mut runner, &mut e, 1, player(1));
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Unknown);

    e.numbered(2, input(player(1), entity, 1, move_to(16.5)));
    run_until(
        &mut runner,
        &mut [&mut e],
        "the player being let go",
        |_, links| departure(&links[0].log, player(1)).is_some(),
    );
    let (at, to, transfer) = departure(&e.log, player(1)).expect("waited for it");
    assert_eq!(to, NEIGHBOUR);
    assert_eq!(transfer.entity_id, entity);
    assert_eq!(transfer.pose.position.x, 16.5);
    assert_eq!(transfer.last_input, 1);
    // The region had to ask first: the step was applied and told as such before.
    let told = progress_to(&e.log, 2).expect("the step was applied");
    assert!(told < at, "{}", brief(&e.log));
    assert!(runner.region().player(player(1)).is_none());

    // Nobody stands in the chunk or looks at it any more, so the belief goes.
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Unknown);
}

/// An entity of another region's block of ids, as an arriving player brings it.
const STRANGER: EntityId = EntityId(5_000_001);
const STRANGER_TOO: EntityId = EntityId(5_000_002);

fn arrive(id: PlayerId, transfer: &PlayerTransfer) -> EdgeToWorker {
    EdgeToWorker::PlayerArrive {
        player: id,
        transfer: transfer.clone(),
    }
}

#[test]
fn an_arrival_for_a_chunk_believed_the_neighbours_is_sent_on_and_one_for_an_own_chunk_taken_in() {
    on_stripes_in_memory_and_on_disk(
        an_arrival_for_a_chunk_believed_the_neighbours_is_sent_on_and_one_for_an_own_chunk_taken_in_in,
    );
}

fn an_arrival_for_a_chunk_believed_the_neighbours_is_sent_on_and_one_for_an_own_chunk_taken_in_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut e = greeted(&mut runner, E, 5, Vec::new(), vec![HOME, NEXT], Vec::new());
    assert_eq!(e.answers(NEXT), vec![(0, Answer::Elsewhere(NEIGHBOUR))]);

    let misdirected = transfer(STRANGER, 16.5);
    e.numbered(1, arrive(player(7), &misdirected));
    wait_applied(&mut runner, &mut [&mut e], 0, 1);
    settle(&mut runner, &mut [&mut e]);
    let entries = outbox(&e.log);
    assert!(
        matches!(
            entries.as_slice(),
            [(
                _,
                1,
                Durable::NotMine {
                    what: Misdirected::Arrival { player: who, transfer },
                    holder: NEIGHBOUR,
                }
            )] if *who == player(7) && *transfer == misdirected
        ),
        "{}",
        brief(&e.log)
    );
    assert!(runner.region().player(player(7)).is_none());
    assert!(events(&e.log).is_empty(), "{}", brief(&e.log));

    // An own chunk that nothing has asked about: the player is taken in, and the region
    // asks for the chunk because they stand in it.
    let landing = ChunkPos::new(-3, 0);
    assert_eq!(runner.region().knowledge(landing), Knowledge::Unknown);
    e.numbered(2, arrive(player(8), &transfer(STRANGER_TOO, -39.5)));
    wait_applied(&mut runner, &mut [&mut e], 0, 2);
    assert_eq!(
        runner.region().player(player(8)).map(|(entity, _)| entity),
        Some(STRANGER_TOO)
    );
    wait_knowledge(&mut runner, &mut [&mut e], landing, Knowledge::Held);
    ticks(&mut runner, &mut [&mut e], 5);
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(outbox(&e.log).len(), 1, "{}", brief(&e.log));
    assert!(runner.region().player(player(8)).is_some());
}

/// Rule 21: an arrival for a chunk the region has had nothing to do with is taken in,
/// and the player is let go to the holder when the store has said who that is.
#[test]
fn an_arrival_for_a_chunk_of_the_other_stripe_that_nothing_asked_about_is_taken_in_and_then_let_go()
{
    let mut world = World::stripes();
    let mut runner = world.open();
    let mut e = established(&mut runner, E, 5);
    let misdirected = transfer(STRANGER, 16.5);
    e.numbered(1, arrive(player(7), &misdirected));
    run_until(
        &mut runner,
        &mut [&mut e],
        "the player being let go",
        |_, links| departure(&links[0].log, player(7)).is_some(),
    );
    settle(&mut runner, &mut [&mut e]);
    let entries = outbox(&e.log);
    assert!(
        matches!(
            entries.as_slice(),
            [(
                _,
                1,
                Durable::Departed {
                    player: who,
                    transfer,
                    to: NEIGHBOUR,
                }
            )] if *who == player(7) && transfer.entity_id == STRANGER && transfer.pose == misdirected.pose
        ),
        "{}",
        brief(&e.log)
    );
    assert!(runner.region().player(player(7)).is_none());
}

fn breaking(id: PlayerId, sequence: i32, position: BlockPos) -> RemoteAction {
    RemoteAction {
        player: id,
        sequence,
        step: RemoteStep::Break { position },
    }
}

#[test]
fn a_dig_in_the_other_stripe_is_passed_on_without_a_region_until_a_viewer_has_asked() {
    on_stripes_in_memory_and_on_disk(
        a_dig_in_the_other_stripe_is_passed_on_without_a_region_until_a_viewer_has_asked_in,
    );
}

fn a_dig_in_the_other_stripe_is_passed_on_without_a_region_until_a_viewer_has_asked_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut e = established(&mut runner, E, 5);

    // Right after joining: the join and the dig are taken by one tick.
    e.numbered(1, join(player(1)));
    e.numbered(
        2,
        input(player(1), nth_entity(&runner, 1), 1, dig(BEYOND, 1)),
    );
    wait_applied(&mut runner, &mut [&mut e], 0, 2);
    let told = progress_to(&e.log, 2).expect("waited for it");
    let entries = outbox(&e.log);
    assert!(
        matches!(
            entries.as_slice(),
            [(at, 1, Durable::Remote { action, to: None })]
                if *action == breaking(player(1), 1, BEYOND) && *at < told
        ),
        "{}",
        brief(&e.log)
    );
    settle(&mut runner, &mut [&mut e]);
    assert!(acknowledged(&e.log, player(1), 1).is_none());
    assert_eq!(
        runner.region().knowledge(NEXT),
        Knowledge::Unknown,
        "a click is no reason to ask"
    );

    // With a viewer's subscription answered before the dig, the region names the holder.
    let ask = e.subscribe(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut e], 0, NEXT, ask),
        Answer::Elsewhere(NEIGHBOUR)
    );
    let further = BEYOND.offset(0, 0, 1);
    e.numbered(
        3,
        input(player(1), nth_entity(&runner, 1), 2, dig(further, 2)),
    );
    wait_applied(&mut runner, &mut [&mut e], 0, 3);
    let told = progress_to(&e.log, 3).expect("waited for it");
    let entries = outbox(&e.log);
    assert!(
        matches!(
            entries.as_slice(),
            [_, (at, 2, Durable::Remote { action, to: Some(NEIGHBOUR) })]
                if *action == breaking(player(1), 2, further) && *at < told
        ),
        "{}",
        brief(&e.log)
    );
    settle(&mut runner, &mut [&mut e]);
    assert!(acknowledged(&e.log, player(1), 2).is_none());
    assert_eq!(outbox(&e.log).len(), 2);
}

// ---------------------------------------------------------------------------------------
// R10. The hello's two lists and the hold
// ---------------------------------------------------------------------------------------

#[test]
fn a_hello_with_chunks_and_guests_in_both_stripes_holds_the_link_until_each_is_answered() {
    on_stripes_in_memory_and_on_disk(
        a_hello_with_chunks_and_guests_in_both_stripes_holds_the_link_until_each_is_answered_in,
    );
}

fn a_hello_with_chunks_and_guests_in_both_stripes_holds_the_link_until_each_is_answered_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut e = Link::attach(&runner);
    e.hello(E, 5, Vec::new(), vec![HOME, NEXT], vec![OWN, OTHER]);
    e.numbered(1, join(player(1)));
    run_until(
        &mut runner,
        &mut [&mut e],
        "the player entering the world",
        |_, links| spawned(&links[0].log, player(1)).is_some(),
    );
    settle(&mut runner, &mut [&mut e]);

    assert_eq!(e.answers(HOME), vec![(0, Answer::Snapshot)]);
    assert_eq!(e.answers(NEXT), vec![(0, Answer::Elsewhere(NEIGHBOUR))]);
    assert_eq!(e.answers(OWN), vec![(0, Answer::Snapshot)]);
    assert_eq!(e.answers(OTHER), vec![(0, Answer::NotMine)]);
    let log = &e.log;
    let (entered, _) = spawned(log, player(1)).expect("waited for it");
    let shown = entity_spawn(log, player(1)).expect("the home chunk is served");
    for chunk in [HOME, NEXT, OWN, OTHER] {
        let (answered, _) = answer_to(log, chunk, 0).expect("asserted above");
        assert!(
            answered < shown && answered < entered,
            "the join was applied before {chunk:?} was answered: {}",
            brief(log)
        );
    }
    let (_, _, _, entities) = snapshot(log, HOME).expect("asserted above");
    assert!(entities.is_empty(), "the snapshot is of before the join");
    assert!(runner.region().player(player(1)).is_some());
    e.assert_answered(&[]);
}

/// What the link says of subscriptions behind its hello is held like everything else,
/// and taken in the order it was sent when the hold ends: with no step of the runner
/// between them, as in R19.
#[test]
fn subscription_messages_behind_a_hello_are_held_and_then_taken_in_the_order_they_were_sent() {
    on_stripes_in_memory_and_on_disk(
        subscription_messages_behind_a_hello_are_held_and_then_taken_in_the_order_they_were_sent_in,
    );
}

fn subscription_messages_behind_a_hello_are_held_and_then_taken_in_the_order_they_were_sent_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut e = Link::attach(&runner);
    e.hello(E, 5, Vec::new(), vec![HOME, NEXT], Vec::new());
    e.subscribe(vec![OWN]);
    e.unsubscribe(vec![OWN]);
    e.subscribe_as_guest(vec![OTHER]);
    let last = e.subscribe(vec![OWN, OTHER, NEXT]);
    assert_eq!(last, 4);
    e.numbered(1, join(player(1)));
    run_until(
        &mut runner,
        &mut [&mut e],
        "everything behind the hello being answered",
        |_, links| {
            links[0].closed
                || (spawned(&links[0].log, player(1)).is_some()
                    && [OWN, OTHER, NEXT]
                        .iter()
                        .all(|chunk| answer_to(&links[0].log, *chunk, last).is_some()))
        },
    );
    assert!(!e.closed, "{}", brief(&e.log));
    settle(&mut runner, &mut [&mut e]);

    assert_eq!(e.answers(HOME), vec![(0, Answer::Snapshot)]);
    assert_eq!(e.answers(OWN), vec![(last, Answer::Snapshot)]);
    // The guest's asking was overtaken by the viewer's before any tick took it.
    assert_eq!(e.answers(OTHER), vec![(last, Answer::Elsewhere(NEIGHBOUR))]);
    // The hello's asking is answered, and the `Subscribe` behind it asks again.
    assert_eq!(
        e.answers(NEXT),
        vec![
            (0, Answer::Elsewhere(NEIGHBOUR)),
            (last, Answer::Elsewhere(NEIGHBOUR))
        ]
    );
    let log = &e.log;
    let held_until = [HOME, NEXT]
        .iter()
        .map(|chunk| answer_to(log, *chunk, 0).expect("asserted above").0)
        .max()
        .expect("two chunks");
    for chunk in [OWN, OTHER, NEXT] {
        let (answered, _) = answer_to(log, chunk, last).expect("asserted above");
        assert!(held_until < answered, "{}", brief(log));
    }
    let (_, home_tick, _, _) = snapshot(log, HOME).expect("asserted above");
    let (_, own_tick, _, _) = snapshot(log, OWN).expect("asserted above");
    assert!(
        home_tick < own_tick,
        "the subscription behind the hello was taken before the hello's chunks were answered"
    );
    e.assert_answered(&[]);
}

/// Section 4.5, third case: the edge has lost its `since` after the region took a
/// message from it. It is reset as for a higher start, and its new link is held and
/// counts from nothing received.
#[test]
fn an_edge_reset_for_a_lost_since_is_held_until_its_hello_is_answered_and_then_taken_from_1() {
    on_stripes_in_memory_and_on_disk(
        an_edge_reset_for_a_lost_since_is_held_until_its_hello_is_answered_and_then_taken_from_1_in,
    );
}

fn an_edge_reset_for_a_lost_since_is_held_until_its_hello_is_answered_and_then_taken_from_1_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut first = established(&mut runner, E, 5);
    let Welcome::Unknown { since: told, .. } = welcomed(&first.log) else {
        panic!(
            "a first hello is welcomed as unknown: {}",
            brief(&first.log)
        );
    };
    let old = join_and_wait(&mut runner, &mut first, 1, player(1));
    wait_applied(&mut runner, &mut [&mut first], 0, 1);
    let mut watcher = established(&mut runner, F, 5);

    let mut second = Link::attach(&runner);
    second.hello_saying(
        E,
        5,
        0,
        0,
        vec![player(1)],
        vec![HOME, NEXT],
        vec![OWN, OTHER],
    );
    second.numbered(1, join(player(2)));
    run_until(
        &mut runner,
        &mut [&mut second, &mut watcher, &mut first],
        "the new link's player entering the world",
        |_, links| links[0].closed || spawned(&links[0].log, player(2)).is_some(),
    );
    assert!(!second.closed, "{}", brief(&second.log));
    settle(&mut runner, &mut [&mut second, &mut watcher]);

    let log = &second.log;
    match welcomed(log) {
        Welcome::Unknown {
            since,
            entries: 0,
            presences: 1,
            applied: 0,
        } => assert!(since > told),
        other => panic!("welcomed {other:?}: {}", brief(log)),
    }
    assert_eq!(presence(log, player(1)), Some(&Presence::Absent));
    assert!(removal(&watcher.log, old).is_some());
    assert!(runner.region().player(player(1)).is_none());
    assert!(runner.region().player(player(2)).is_some());
    assert_eq!(applied(log), Some(1), "{}", brief(log));
    assert_eq!(second.answers(HOME), vec![(0, Answer::Snapshot)]);
    assert_eq!(
        second.answers(NEXT),
        vec![(0, Answer::Elsewhere(NEIGHBOUR))]
    );
    assert_eq!(second.answers(OWN), vec![(0, Answer::Snapshot)]);
    assert_eq!(second.answers(OTHER), vec![(0, Answer::NotMine)]);
    let (entered, _) = spawned(log, player(2)).expect("waited for it");
    for chunk in [HOME, NEXT, OWN, OTHER] {
        let (answered, _) = answer_to(log, chunk, 0).expect("asserted above");
        assert!(answered < entered, "{}", brief(log));
    }
    first.drain();
    assert!(first.closed, "the edge's other link is closed");
}

/// The path of the manifest of a stored chunk in a world on disk.
fn manifest_of(world: &World, position: ChunkPos) -> std::path::PathBuf {
    world
        .root()
        .join("manifests/overworld")
        .join(format!("{}.{}", position.x >> 5, position.z >> 5))
        .join(format!("{}.{}.manifest", position.x, position.z))
}

/// Section 4.5: the hold is let go of for a chunk when the store says that it cannot
/// be read. Such a chunk is the one subscription that is answered with silence
/// (section 4.4), and it holds nothing up.
#[test]
fn a_chunk_of_a_hello_that_the_store_cannot_read_ends_the_hold_and_is_answered_with_nothing() {
    let mut world = World::new(Shape::Stripes, true);
    {
        // The chunk has to be in the store to be damaged there.
        let (handle, _) = world.open_raw();
        let owner = Other { handle };
        let chunk = owner.load(OWN);
        owner.save(OWN, &chunk);
        owner.handle.flush();
    }
    std::fs::write(manifest_of(&world, OWN), b"not a manifest").expect("the chunk was stored");

    let mut runner = world.open();
    let mut e = Link::attach(&runner);
    e.hello(E, 5, Vec::new(), vec![HOME, OWN, NEXT], vec![OTHER]);
    e.numbered(1, join(player(1)));
    run_until(
        &mut runner,
        &mut [&mut e],
        "the player entering the world",
        |_, links| spawned(&links[0].log, player(1)).is_some(),
    );
    ticks(&mut runner, &mut [&mut e], 5);
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(e.answers(HOME), vec![(0, Answer::Snapshot)]);
    assert_eq!(e.answers(NEXT), vec![(0, Answer::Elsewhere(NEIGHBOUR))]);
    assert_eq!(e.answers(OTHER), vec![(0, Answer::NotMine)]);
    assert_eq!(e.answers(OWN), Vec::new(), "{}", brief(&e.log));
    let (entered, _) = spawned(&e.log, player(1)).expect("waited for it");
    for chunk in [HOME, NEXT, OTHER] {
        let (answered, _) = answer_to(&e.log, chunk, 0).expect("asserted above");
        assert!(answered < entered, "{}", brief(&e.log));
    }
    e.assert_answered(&[OWN]);

    // An ordinary subscription to it is answered with nothing as well, and other
    // chunks go on being served.
    e.subscribe(vec![OWN]);
    let ask = e.subscribe(vec![OWN_TOO]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut e], 0, OWN_TOO, ask),
        Answer::Snapshot
    );
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(e.answers(OWN), Vec::new(), "{}", brief(&e.log));

    // A hello that names it later is not held by it either.
    let mut later = Link::attach(&runner);
    later.hello(F, 5, Vec::new(), vec![OWN], Vec::new());
    later.numbered(1, join(player(2)));
    run_until(
        &mut runner,
        &mut [&mut later, &mut e],
        "the later link's player entering the world",
        |_, links| spawned(&links[0].log, player(2)).is_some(),
    );
    settle(&mut runner, &mut [&mut later, &mut e]);
    assert_eq!(later.answers(OWN), Vec::new(), "{}", brief(&later.log));
}

// ---------------------------------------------------------------------------------------
// R13. `Elsewhere` and `NotMine` are part of a tick
// ---------------------------------------------------------------------------------------

/// Section 5.2 for the tick of a hello whose lists the region can answer at once: the
/// resume, then the answers in ascending order of their chunks, then the progress, and
/// nothing else. A chunk in both lists is a viewer's, and is answered once.
#[test]
fn in_the_tick_of_a_hello_the_resume_comes_first_then_the_answers_to_its_lists_and_the_progress() {
    on_stripes_in_memory_and_on_disk(
        in_the_tick_of_a_hello_the_resume_comes_first_then_the_answers_to_its_lists_and_the_progress_in,
    );
}

fn in_the_tick_of_a_hello_the_resume_comes_first_then_the_answers_to_its_lists_and_the_progress_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut believer = believing(&mut runner);
    let mut e = Link::attach(&runner);
    e.hello(
        E,
        5,
        vec![player(9)],
        vec![NEXT, NEXT],
        vec![NEXT, OTHER, OTHER],
    );
    run_until(
        &mut runner,
        &mut [&mut e, &mut believer],
        "the progress of the hello's tick",
        |_, links| applied(&links[0].log).is_some(),
    );
    ticks(&mut runner, &mut [&mut e, &mut believer], 5);
    assert!(
        matches!(
            e.log.as_slice(),
            [
                WorkerToEdge::Welcome(Welcome::Unknown { entries: 0, .. }),
                WorkerToEdge::Presence {
                    answer: Presence::Absent,
                    ..
                },
                WorkerToEdge::Elsewhere {
                    chunk: NEXT,
                    ask: 0,
                    region: NEIGHBOUR
                },
                WorkerToEdge::NotMine {
                    chunk: OTHER,
                    ask: 0
                },
                WorkerToEdge::Progress { applied: 0, .. },
            ]
        ),
        "{}",
        brief(&e.log)
    );
}

/// Edge F's link with a viewer's subscription to the first chunk of the other stripe
/// that was told elsewhere. For as long as the link is there the region believes the
/// neighbour to hold the chunk, so that a later subscription to it is answered in the
/// tick that takes it.
fn believing(runner: &mut RegionRunner) -> Link {
    let mut believer = linked(runner, F, 5);
    let ask = believer.subscribe(vec![NEXT]);
    assert_eq!(
        wait_answer(runner, &mut [&mut believer], 0, NEXT, ask),
        Answer::Elsewhere(NEIGHBOUR)
    );
    believer
}

/// The place and the tick of the delta that has an event `found` says yes to.
fn delta_with(log: &[WorkerToEdge], found: impl Fn(&RegionEvent) -> bool) -> Option<(usize, u64)> {
    events(log)
        .into_iter()
        .find(|(_, _, event)| found(event))
        .map(|(index, tick, _)| (index, tick))
}

fn moved_to(entity: EntityId, x: f64) -> impl Fn(&RegionEvent) -> bool {
    move |event| {
        matches!(event, RegionEvent::EntityMoved { entity: who, pose, .. }
            if *who == entity && pose.position.x == x)
    }
}

/// Whether an event shows `entity` at `x` to a link: as a move, or, to a link that sees
/// the chunk the entity came into and not the one it came from, as an entity that is
/// new to it.
fn shown_at(entity: EntityId, x: f64) -> impl Fn(&RegionEvent) -> bool {
    move |event| match event {
        RegionEvent::EntityMoved {
            entity: who, pose, ..
        } => *who == entity && pose.position.x == x,
        RegionEvent::EntitySpawned(state) => state.entity == entity && state.pose.position.x == x,
        _ => false,
    }
}

#[test]
fn elsewhere_and_not_mine_come_at_their_place_in_their_tick_between_the_ticks_around_it() {
    on_stripes_in_memory_and_on_disk(
        elsewhere_and_not_mine_come_at_their_place_in_their_tick_between_the_ticks_around_it_in,
    );
}

fn elsewhere_and_not_mine_come_at_their_place_in_their_tick_between_the_ticks_around_it_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut believer = believing(&mut runner);
    let mut e = established(&mut runner, E, 5);
    let digger = join_and_wait(&mut runner, &mut e, 1, player(1));
    let walker = join_and_wait(&mut runner, &mut e, 2, player(3));
    wait_applied(&mut runner, &mut [&mut e, &mut believer], 0, 2);

    // The tick before: a step.
    e.numbered(3, input(player(1), digger, 1, move_to(13.0)));
    let before = one_tick(&mut runner, &mut [&mut e, &mut believer]);

    // The tick in question takes all of this. It has a commit, events, a player who
    // enters the world, an action that is passed on, an acknowledgement, a departure,
    // and an answer to each of two subscriptions.
    e.numbered(4, join(player(2)));
    e.numbered(5, input(player(1), digger, 2, dig(NEAR, 1)));
    e.numbered(6, input(player(1), digger, 3, dig(BEYOND, 2)));
    e.numbered(7, input(player(3), walker, 1, move_to(16.5)));
    let elsewhere = e.subscribe(vec![NEXT]);
    let not_mine = e.subscribe_as_guest(vec![OTHER]);
    let tick = one_tick(&mut runner, &mut [&mut e, &mut believer]);
    assert_eq!(tick, before + 1);

    // The tick after: another step.
    e.numbered(8, input(player(1), digger, 4, move_to(12.0)));
    let after = one_tick(&mut runner, &mut [&mut e, &mut believer]);
    wait_applied(&mut runner, &mut [&mut e, &mut believer], 0, 8);
    settle(&mut runner, &mut [&mut e, &mut believer]);

    let log = &e.log;
    let fail = || brief(log);
    let (earlier, earlier_tick) =
        delta_with(log, moved_to(digger, 13.0)).unwrap_or_else(|| panic!("{}", fail()));
    let earlier_told = progress_to(log, 3).unwrap_or_else(|| panic!("{}", fail()));
    let (delta, delta_tick) = delta_with(
        log,
        |event| matches!(event, RegionEvent::BlockChanged { position, .. } if *position == NEAR),
    )
    .unwrap_or_else(|| panic!("{}", fail()));
    let shown = entity_spawn(log, player(2)).unwrap_or_else(|| panic!("{}", fail()));
    let (stepped, _) =
        delta_with(log, moved_to(walker, 16.5)).unwrap_or_else(|| panic!("{}", fail()));
    let (entered, _) = spawned(log, player(2)).unwrap_or_else(|| panic!("{}", fail()));
    let passed_on = position_of(log, |message| {
        matches!(
            message,
            WorkerToEdge::Outbox {
                entry: Durable::Remote {
                    to: Some(NEIGHBOUR),
                    ..
                },
                ..
            }
        )
    })
    .unwrap_or_else(|| panic!("{}", fail()));
    let handled = acknowledged(log, player(1), 1).unwrap_or_else(|| panic!("{}", fail()));
    let (let_go, to, _) = departure(log, player(3)).unwrap_or_else(|| panic!("{}", fail()));
    let (told_elsewhere, answer) =
        answer_to(log, NEXT, elsewhere).unwrap_or_else(|| panic!("{}", fail()));
    assert_eq!(answer, Answer::Elsewhere(NEIGHBOUR));
    let (told_not_mine, answer) =
        answer_to(log, OTHER, not_mine).unwrap_or_else(|| panic!("{}", fail()));
    assert_eq!(answer, Answer::NotMine);
    let told = progress_to(log, 7).unwrap_or_else(|| panic!("{}", fail()));
    let (later, later_tick) =
        delta_with(log, moved_to(digger, 12.0)).unwrap_or_else(|| panic!("{}", fail()));

    assert_eq!(to, NEIGHBOUR);
    assert_eq!(
        (earlier_tick, delta_tick, later_tick),
        (before, tick, after),
        "{}",
        fail()
    );
    assert_eq!(
        (shown, stepped),
        (delta, delta),
        "the events of a tick are one delta: {}",
        fail()
    );
    let order = [
        earlier,
        earlier_told,
        delta,
        entered,
        passed_on,
        handled,
        let_go,
        told_elsewhere,
        told_not_mine,
        told,
        later,
    ];
    assert!(
        order.windows(2).all(|pair| pair[0] < pair[1]),
        "the tick before, then delta, spawned, remote, acknowledged, departed, elsewhere, not mine, progress, then the tick after: {order:?}\n{}",
        fail()
    );
    // Everything between the delta and the progress of the tick is of that tick: the
    // two answers stand side by side, right before the progress.
    assert_eq!(told_not_mine, told_elsewhere + 1, "{}", fail());
    assert_eq!(told, told_not_mine + 1, "{}", fail());
    e.assert_answered(&[]);
}

/// Section 5.2, item 7, with the gap, where the chunks the region holds lie between
/// those of the two pinned regions: the answers of one tick are in ascending order of
/// their chunks, whatever each of them is. Everything the hello names can be answered
/// in the tick that takes it: the region holds and has loaded two of the chunks for
/// another link, and believes of two others what it told that link.
#[test]
fn the_answers_of_one_tick_come_in_ascending_order_of_their_chunks_whatever_their_kind() {
    on_the_gap_in_memory_and_on_disk(
        the_answers_of_one_tick_come_in_ascending_order_of_their_chunks_whatever_their_kind_in,
    );
}

fn the_answers_of_one_tick_come_in_ascending_order_of_their_chunks_whatever_their_kind_in(
    mut world: World,
) {
    let west = ChunkPos::new(-1, 0);
    let mut runner = world.open();
    let mut keeper = greeted(
        &mut runner,
        F,
        5,
        Vec::new(),
        vec![west, HOME, NEXT, EAST],
        Vec::new(),
    );
    assert_eq!(keeper.answers(west), vec![(0, Answer::Elsewhere(WESTERN))]);
    assert_eq!(
        keeper.answers(EAST),
        vec![(0, Answer::Elsewhere(NEIGHBOUR))]
    );
    settle(&mut runner, &mut [&mut keeper]);

    let mut e = Link::attach(&runner);
    e.hello(E, 5, Vec::new(), vec![EAST, HOME, west], vec![FREE, NEXT]);
    run_until(
        &mut runner,
        &mut [&mut e, &mut keeper],
        "the progress of the hello's tick",
        |_, links| applied(&links[0].log).is_some(),
    );
    ticks(&mut runner, &mut [&mut e, &mut keeper], 5);
    let log = &e.log;
    assert!(
        matches!(
            log.as_slice(),
            [
                WorkerToEdge::Welcome(Welcome::Unknown { entries: 0, .. }),
                WorkerToEdge::Elsewhere {
                    chunk: ChunkPos { x: -1, z: 0 },
                    ask: 0,
                    region: WESTERN
                },
                WorkerToEdge::ChunkSnapshot {
                    position: HOME,
                    ask: 0,
                    ..
                },
                WorkerToEdge::ChunkSnapshot {
                    position: NEXT,
                    ask: 0,
                    ..
                },
                WorkerToEdge::NotMine {
                    chunk: FREE,
                    ask: 0
                },
                WorkerToEdge::Elsewhere {
                    chunk: EAST,
                    ask: 0,
                    region: NEIGHBOUR
                },
                WorkerToEdge::Progress { applied: 0, .. },
            ]
        ),
        "{}",
        brief(log)
    );
    let (_, home_tick, _, _) = snapshot(log, HOME).expect("asserted above");
    let (_, next_tick, _, _) = snapshot(log, NEXT).expect("asserted above");
    assert_eq!(home_tick, next_tick);
}

/// A tick that has only an answer to a subscription has no commit to wait for, and is
/// published as soon as the ticks before it are, not sooner.
#[test]
fn an_elsewhere_of_a_tick_without_a_commit_does_not_overtake_the_tick_before_it() {
    on_stripes_in_memory_and_on_disk(
        an_elsewhere_of_a_tick_without_a_commit_does_not_overtake_the_tick_before_it_in,
    );
}

fn an_elsewhere_of_a_tick_without_a_commit_does_not_overtake_the_tick_before_it_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut believer = believing(&mut runner);
    let mut e = established(&mut runner, E, 5);
    settle(&mut runner, &mut [&mut e, &mut believer]);

    e.numbered(1, join(player(1)));
    let joined = one_tick(&mut runner, &mut [&mut e, &mut believer]);
    let elsewhere = e.subscribe(vec![NEXT]);
    let not_mine = e.subscribe_as_guest(vec![OTHER]);
    one_tick(&mut runner, &mut [&mut e, &mut believer]);
    e.numbered(
        2,
        input(player(1), nth_entity(&runner, 1), 1, move_to(13.0)),
    );
    let moved = one_tick(&mut runner, &mut [&mut e, &mut believer]);
    wait_applied(&mut runner, &mut [&mut e, &mut believer], 0, 2);
    settle(&mut runner, &mut [&mut e, &mut believer]);

    let log = &e.log;
    let (entity_at, entity) = spawned(log, player(1)).expect("waited for it");
    let (shown, shown_tick) = delta_with(
        log,
        |event| matches!(event, RegionEvent::EntitySpawned(state) if state.entity == entity),
    )
    .expect("the home chunk is served");
    let joined_told = progress_to(log, 1).expect("waited for it");
    let (told_elsewhere, _) = answer_to(log, NEXT, elsewhere).expect("settled");
    let (told_not_mine, _) = answer_to(log, OTHER, not_mine).expect("settled");
    let (step, step_tick) = delta_with(log, moved_to(entity, 13.0)).expect("waited for it");
    assert_eq!((shown_tick, step_tick), (joined, moved), "{}", brief(log));
    let order = [
        shown,
        entity_at,
        joined_told,
        told_elsewhere,
        told_not_mine,
        step,
    ];
    assert!(
        order.windows(2).all(|pair| pair[0] < pair[1]),
        "{order:?}\n{}",
        brief(log)
    );
}

#[test]
fn a_runner_whose_store_was_taken_after_the_tick_ran_sends_neither_its_elsewhere_nor_its_not_mine()
{
    on_stripes_in_memory_and_on_disk(
        a_runner_whose_store_was_taken_after_the_tick_ran_sends_neither_its_elsewhere_nor_its_not_mine_in,
    );
}

fn a_runner_whose_store_was_taken_after_the_tick_ran_sends_neither_its_elsewhere_nor_its_not_mine_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut believer = believing(&mut runner);
    let mut e = established(&mut runner, E, 5);
    settle(&mut runner, &mut [&mut e, &mut believer]);

    // The tick has a commit, because a player joins in it.
    e.numbered(1, join(player(1)));
    let elsewhere = e.subscribe(vec![NEXT]);
    let not_mine = e.subscribe_as_guest(vec![OTHER]);
    one_tick(&mut runner, &mut [&mut e, &mut believer]);
    let nothing_of_the_tick = |link: &Link| {
        spawned(&link.log, player(1)).is_none()
            && answer_to(&link.log, NEXT, elsewhere).is_none()
            && answer_to(&link.log, OTHER, not_mine).is_none()
    };
    // The runner looks at what the store has confirmed before it runs a tick, so the
    // commit it has just asked for cannot be confirmed to it yet.
    assert!(
        nothing_of_the_tick(&e),
        "the tick is out before its commit can have been confirmed: {}",
        brief(&e.log)
    );

    // Another owner takes the region. The commit is never confirmed to this runner.
    let _other = world.open_raw();
    run_until(
        &mut runner,
        &mut [&mut e, &mut believer],
        "the runner finding its store lost and closing its links",
        |runner, links| runner.store_is_lost() && links[0].closed && links[1].closed,
    );
    assert!(nothing_of_the_tick(&e), "{}", brief(&e.log));
}

// ---------------------------------------------------------------------------------------
// R15. After a crash
// ---------------------------------------------------------------------------------------

#[test]
fn after_a_crash_a_hello_with_the_same_chunks_is_answered_as_before_and_the_player_is_present() {
    on_stripes_in_memory_and_on_disk(
        after_a_crash_a_hello_with_the_same_chunks_is_answered_as_before_and_the_player_is_present_in,
    );
}

fn after_a_crash_a_hello_with_the_same_chunks_is_answered_as_before_and_the_player_is_present_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut e = greeted(
        &mut runner,
        E,
        5,
        Vec::new(),
        vec![HOME, OWN, NEXT],
        vec![OWN_TOO, OTHER],
    );
    let entity = join_and_wait(&mut runner, &mut e, 1, player(1));
    wait_applied(&mut runner, &mut [&mut e], 0, 1);
    assert_eq!(
        runner.region().knowledge(NEXT),
        Knowledge::Foreign(NEIGHBOUR)
    );

    // The crash. The region has forgotten what it believed and what it had claimed of
    // its stripe: the store tells it only what it was granted beyond its areas.
    let mut next = world.open();
    drop(runner);
    for chunk in [OWN, OWN_TOO, NEXT, OTHER] {
        assert_eq!(next.region().knowledge(chunk), Knowledge::Unknown);
    }
    let mut again = Link::attach(&next);
    again.hello(
        E,
        5,
        vec![player(1)],
        vec![HOME, OWN, NEXT],
        vec![OWN_TOO, OTHER],
    );
    again.numbered(2, input(player(1), entity, 1, move_to(12.5)));
    wait_applied(&mut next, &mut [&mut again], 0, 2);
    settle(&mut next, &mut [&mut again]);

    let log = &again.log;
    assert_eq!(
        welcomed(log),
        Welcome::Resumed {
            entries: 0,
            presences: 1,
            applied: 1,
        },
        "{}",
        brief(log)
    );
    assert!(
        matches!(
            presence(log, player(1)),
            Some(Presence::Present { entity: present, .. }) if *present == entity
        ),
        "{}",
        brief(log)
    );
    assert_eq!(again.answers(HOME), vec![(0, Answer::Snapshot)]);
    assert_eq!(again.answers(OWN), vec![(0, Answer::Snapshot)]);
    assert_eq!(again.answers(NEXT), vec![(0, Answer::Elsewhere(NEIGHBOUR))]);
    assert_eq!(again.answers(OWN_TOO), vec![(0, Answer::Snapshot)]);
    assert_eq!(again.answers(OTHER), vec![(0, Answer::NotMine)]);
    let (_, _, _, entities) = snapshot(log, HOME).expect("asserted above");
    assert!(entities.iter().any(|state| state.entity == entity));
    // What the edge sends again is applied only once the region knows of every chunk
    // of the hello who holds it.
    let (step, _) = delta_with(log, moved_to(entity, 12.5)).expect("the step was applied");
    for chunk in [HOME, OWN, NEXT, OWN_TOO, OTHER] {
        let (answered, _) = answer_to(log, chunk, 0).expect("asserted above");
        assert!(answered < step, "{}", brief(log));
    }
    again.assert_answered(&[]);
}

// ---------------------------------------------------------------------------------------
// R16 and section 4.7. Release, stopping and what the next owner is restored with
// ---------------------------------------------------------------------------------------

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

/// The chunks the store says a region was granted.
fn held(restored: &Restored) -> Vec<ChunkPos> {
    restored.held.iter().map(|(chunk, _)| *chunk).collect()
}

/// With the gap: a region with the two free chunks east of the home chunk granted for
/// a viewer, released. Returns what the next owner is restored with.
fn released_with_two_granted_chunks(
    world: &mut World,
    return_after: u64,
) -> (StoreHandle, Restored) {
    let mut runner = world.open_returning_after(return_after);
    let mut e = greeted(
        &mut runner,
        E,
        5,
        Vec::new(),
        vec![HOME, NEXT, FREE],
        Vec::new(),
    );
    assert_eq!(e.answers(FREE), vec![(0, Answer::Snapshot)]);
    runner.begin_release();
    assert_eq!(run_to_the_end(&mut runner, &mut [&mut e]), Ended::Released);
    world.open_raw()
}

#[test]
fn the_next_owner_of_a_released_region_returns_its_chunks_with_its_forty_first_tick() {
    on_the_gap_in_memory_and_on_disk(
        the_next_owner_of_a_released_region_returns_its_chunks_with_its_forty_first_tick_in,
    );
}

fn the_next_owner_of_a_released_region_returns_its_chunks_with_its_forty_first_tick_in(
    mut world: World,
) {
    let neighbour = world.neighbour();
    let (handle, restored) = released_with_two_granted_chunks(&mut world, 40);
    assert_eq!(held(&restored), vec![HOME, NEXT, FREE]);
    let mut next = RegionRunner::restore(config(40), handle, restored).expect("readable");
    let restored_at = next.region().tick_number();

    // The ticks before the restore count as ticks in which the chunks were used.
    for tick in restored_at + 1..=restored_at + 40 {
        assert_eq!(one_tick(&mut next, &mut []), tick);
        for chunk in [NEXT, FREE] {
            assert_eq!(
                next.region().knowledge(chunk),
                Knowledge::Held,
                "{chunk:?} was given back with tick {} of the next owner",
                tick - restored_at
            );
        }
    }
    assert_eq!(neighbour.claim(NEXT), Err(world.region()));
    one_tick(&mut next, &mut []);
    for chunk in [NEXT, FREE] {
        assert_eq!(
            next.region().knowledge(chunk),
            Knowledge::Unknown,
            "{chunk:?} was not given back with the forty-first tick"
        );
    }
    claim_until_granted(&mut next, &mut [], &neighbour, NEXT);
    claim_until_granted(&mut next, &mut [], &neighbour, FREE);
    // The home chunk is never given back.
    ticks(&mut next, &mut [], 45);
    assert_eq!(next.region().knowledge(HOME), Knowledge::Held);
    assert_eq!(neighbour.claim(HOME), Err(world.region()));
}

#[test]
fn a_hello_that_names_the_next_owners_chunks_before_their_time_has_come_keeps_them() {
    on_the_gap_in_memory_and_on_disk(
        a_hello_that_names_the_next_owners_chunks_before_their_time_has_come_keeps_them_in,
    );
}

fn a_hello_that_names_the_next_owners_chunks_before_their_time_has_come_keeps_them_in(
    mut world: World,
) {
    let neighbour = world.neighbour();
    let (handle, restored) = released_with_two_granted_chunks(&mut world, 40);
    let mut next = RegionRunner::restore(config(40), handle, restored).expect("readable");
    let restored_at = next.region().tick_number();
    run_to_tick(&mut next, &mut [], restored_at + 40);

    // The hello is taken by the forty-first tick, the one that would give the chunks
    // back: they are used at its end.
    let mut back = Link::attach(&next);
    back.hello(E, 5, Vec::new(), vec![NEXT], vec![FREE]);
    run_to_tick(&mut next, &mut [&mut back], restored_at + 100);
    settle(&mut next, &mut [&mut back]);
    assert_eq!(back.answers(NEXT), vec![(0, Answer::Snapshot)]);
    assert_eq!(back.answers(FREE), vec![(0, Answer::Snapshot)]);
    for chunk in [NEXT, FREE] {
        assert_eq!(next.region().knowledge(chunk), Knowledge::Held);
        assert_eq!(neighbour.claim(chunk), Err(world.region()));
    }
}

/// A release ends with the links closed, which a region that gives a chunk back at
/// the end of the first tick without use must not take for a reason to return it: a
/// region that is released keeps its chunks.
#[test]
fn a_region_that_is_released_keeps_its_chunks_also_with_no_time_before_a_return() {
    on_the_gap_in_memory_and_on_disk(
        a_region_that_is_released_keeps_its_chunks_also_with_no_time_before_a_return_in,
    );
}

fn a_region_that_is_released_keeps_its_chunks_also_with_no_time_before_a_return_in(
    mut world: World,
) {
    let neighbour = world.neighbour();
    let (handle, restored) = released_with_two_granted_chunks(&mut world, 0);
    assert_eq!(held(&restored), vec![HOME, NEXT, FREE]);
    assert_eq!(neighbour.claim(NEXT), Err(world.region()));

    // The next owner starts the time anew, which is none here: what nobody comes to
    // use goes with its first tick.
    let mut next = RegionRunner::restore(config(0), handle, restored).expect("readable");
    for chunk in [HOME, NEXT, FREE] {
        assert_eq!(next.region().knowledge(chunk), Knowledge::Held);
    }
    one_tick(&mut next, &mut []);
    for chunk in [NEXT, FREE] {
        assert_eq!(next.region().knowledge(chunk), Knowledge::Unknown);
    }
    assert_eq!(next.region().knowledge(HOME), Knowledge::Held);
    claim_until_granted(&mut next, &mut [], &neighbour, NEXT);
    claim_until_granted(&mut next, &mut [], &neighbour, FREE);
}

/// What the released runner had asked and not heard, it drops; a grant among it is in
/// `Restored::held`. Free chunks are asked for one per step before and while the
/// region is released, so that the answers to some claims come while it still ticks
/// and those to others after it has stopped, or never.
#[test]
fn what_a_released_region_held_or_had_asked_for_is_what_the_next_owner_is_told_it_holds() {
    for on_disk in [false, true] {
        for lead in 0..=3 {
            let mut world = World::new(Shape::Gap, on_disk);
            let mut runner = world.open_returning_after(40);
            let mut e = established(&mut runner, E, 5);
            let wanted: Vec<ChunkPos> = (1..=12).map(|x| ChunkPos::new(x, 3)).collect();
            let mut asked = 0;
            for chunk in &wanted[..lead] {
                e.subscribe(vec![*chunk]);
                asked += 1;
                runner.step();
                e.drain();
            }
            runner.begin_release();
            for _ in 0..STEPS {
                if asked < wanted.len() {
                    e.try_subscribe(vec![wanted[asked]]);
                    asked += 1;
                }
                runner.step();
                e.drain();
                if runner.ended().is_some() {
                    break;
                }
                thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(runner.ended(), Some(Ended::Released));

            let mut expected = vec![HOME];
            expected.extend(wanted.iter().filter(|chunk| {
                matches!(
                    runner.region().knowledge(**chunk),
                    Knowledge::Held | Knowledge::Asked
                )
            }));
            expected.sort();
            let (handle, restored) = world.open_raw();
            assert_eq!(
                held(&restored),
                expected,
                "{lead} chunks asked for before the release, on disk: {on_disk}"
            );

            // The next owner knows nothing of what was asked: it holds what it is told.
            let mut next = RegionRunner::restore(config(0), handle, restored).expect("readable");
            for chunk in &expected {
                assert_eq!(next.region().knowledge(*chunk), Knowledge::Held);
            }
            one_tick(&mut next, &mut []);
            for chunk in &wanted {
                assert_eq!(next.region().knowledge(*chunk), Knowledge::Unknown);
            }
            assert_eq!(next.region().knowledge(HOME), Knowledge::Held);
        }
    }
}

/// Reads a link of a region that runs on its own thread until `done`.
fn wait_for(link: &mut Link, what: &str, done: impl Fn(&Link) -> bool) {
    // A worker ticks every 50 ms, so this allows it far longer than a stepped runner.
    for _ in 0..10 * STEPS {
        link.drain();
        if done(link) {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("never happened: {what}\n{}", brief(&link.log));
}

/// Section 4.7: a runner that is told to stop checkpoints and flushes, and returns
/// nothing on its way out, although its links are closed by then.
#[test]
fn a_runner_that_is_told_to_stop_returns_nothing_on_its_way_out() {
    let mut world = World::gap();
    let neighbour = world.neighbour();
    let runner = world.open();
    let links = runner.links();
    let worker = Worker::spawn(runner);
    let (edge, end) = link::in_process::<EdgeMessage, WorkerToEdge>(CAPACITY);
    links.attach(end);
    let mut e = Link::of(edge);
    e.hello(E, 5, Vec::new(), vec![HOME, NEXT], Vec::new());
    wait_for(&mut e, "the snapshots of a running worker", |link| {
        answer_to(&link.log, HOME, 0).is_some() && answer_to(&link.log, NEXT, 0).is_some()
    });
    assert_eq!(e.answers(NEXT), vec![(0, Answer::Snapshot)]);

    assert_eq!(worker.stop(), Ended::Stopped);
    e.drain();
    assert!(e.closed, "its links are closed by the time it has stopped");
    assert_eq!(neighbour.claim(NEXT), Err(world.region()));
    let (handle, restored) = world.open_raw();
    assert_eq!(held(&restored), vec![HOME, NEXT]);

    let mut next = RegionRunner::restore(config(0), handle, restored).expect("readable");
    one_tick(&mut next, &mut []);
    assert_eq!(next.region().knowledge(NEXT), Knowledge::Unknown);
    claim_until_granted(&mut next, &mut [], &neighbour, NEXT);
}

// ---------------------------------------------------------------------------------------
// R17 and rule 33. Entities nobody will pass on
// ---------------------------------------------------------------------------------------

/// How often `log` says that `entity` is gone.
fn removals(log: &[WorkerToEdge], entity: EntityId) -> usize {
    events(log)
        .into_iter()
        .filter(|(_, _, event)| {
            matches!(event, RegionEvent::EntityRemoved { entity: gone, .. } if *gone == entity)
        })
        .count()
}

/// On stripes: edge E with player 1, who stays in the home chunk, and player 2, who has
/// walked into the other stripe without the edge confirming the departure; and edge F,
/// whose link is subscribed to nothing. Returns both links and both entities.
fn with_an_unconfirmed_departure(runner: &mut RegionRunner) -> (Link, Link, EntityId, EntityId) {
    let mut e = established(runner, E, 5);
    let mut bystander = linked(runner, F, 5);
    let stays = join_and_wait(runner, &mut e, 1, player(1));
    let leaves = join_and_wait(runner, &mut e, 2, player(2));
    e.numbered(3, input(player(2), leaves, 1, move_to(16.5)));
    run_until(
        runner,
        &mut [&mut e, &mut bystander],
        "the player being let go",
        |_, links| departure(&links[0].log, player(2)).is_some(),
    );
    settle(runner, &mut [&mut e, &mut bystander]);
    assert!(
        events(&bystander.log).is_empty(),
        "{}",
        brief(&bystander.log)
    );
    (e, bystander, stays, leaves)
}

#[test]
fn the_removal_of_a_departed_entity_whose_edge_started_anew_reaches_a_link_subscribed_to_nothing() {
    on_stripes_in_memory_and_on_disk(
        the_removal_of_a_departed_entity_whose_edge_started_anew_reaches_a_link_subscribed_to_nothing_in,
    );
}

fn the_removal_of_a_departed_entity_whose_edge_started_anew_reaches_a_link_subscribed_to_nothing_in(
    mut world: World,
) {
    let mut runner = world.open();
    let (mut old, mut bystander, stays, leaves) = with_an_unconfirmed_departure(&mut runner);

    let mut new = Link::attach(&runner);
    new.hello(E, 6, Vec::new(), Vec::new(), Vec::new());
    run_until(
        &mut runner,
        &mut [&mut bystander, &mut new, &mut old],
        "the link that is subscribed to nothing seeing the departed entity removed",
        |_, links| removal(&links[0].log, leaves).is_some(),
    );
    settle(&mut runner, &mut [&mut bystander, &mut new]);
    for link in [&bystander, &new] {
        assert_eq!(
            removal(&link.log, leaves).map(|(_, _, chunk)| chunk),
            Some(NEXT),
            "{}",
            brief(&link.log)
        );
        assert_eq!(
            removals(&link.log, leaves),
            1,
            "no entity is reported twice"
        );
        // The player who stayed is removed with the edge as well, in the home chunk,
        // which neither link is subscribed to.
        assert!(removal(&link.log, stays).is_none(), "{}", brief(&link.log));
    }
    assert!(runner.region().player(player(1)).is_none());
    assert!(old.closed);
}

/// On stripes: edge E has passed on an arrival for a chunk it was told is elsewhere,
/// and has not confirmed the `NotMine` it was answered with; edge F's link is
/// subscribed to nothing.
fn with_an_arrival_that_was_sent_on(runner: &mut RegionRunner) -> (Link, Link) {
    let mut e = greeted(runner, E, 5, Vec::new(), vec![NEXT], Vec::new());
    let mut bystander = linked(runner, F, 5);
    e.numbered(1, arrive(player(7), &transfer(STRANGER, 16.5)));
    wait_applied(runner, &mut [&mut e, &mut bystander], 0, 1);
    assert!(
        matches!(
            outbox(&e.log).as_slice(),
            [(
                _,
                1,
                Durable::NotMine {
                    what: Misdirected::Arrival { .. },
                    holder: NEIGHBOUR
                }
            )]
        ),
        "{}",
        brief(&e.log)
    );
    settle(runner, &mut [&mut e, &mut bystander]);
    assert!(removal(&bystander.log, STRANGER).is_none());
    (e, bystander)
}

#[test]
fn the_entity_of_an_arrival_that_was_sent_on_is_removed_on_every_link_when_its_edge_starts_anew() {
    on_stripes_in_memory_and_on_disk(
        the_entity_of_an_arrival_that_was_sent_on_is_removed_on_every_link_when_its_edge_starts_anew_in,
    );
}

fn the_entity_of_an_arrival_that_was_sent_on_is_removed_on_every_link_when_its_edge_starts_anew_in(
    mut world: World,
) {
    let mut runner = world.open();
    let (mut old, mut bystander) = with_an_arrival_that_was_sent_on(&mut runner);

    let mut new = Link::attach(&runner);
    new.hello(E, 6, Vec::new(), Vec::new(), Vec::new());
    run_until(
        &mut runner,
        &mut [&mut bystander, &mut new, &mut old],
        "the link that is subscribed to nothing seeing the arrival's entity removed",
        |_, links| removal(&links[0].log, STRANGER).is_some(),
    );
    settle(&mut runner, &mut [&mut bystander, &mut new]);
    for link in [&bystander, &new] {
        assert_eq!(
            removal(&link.log, STRANGER).map(|(_, _, chunk)| chunk),
            Some(NEXT),
            "{}",
            brief(&link.log)
        );
        assert_eq!(removals(&link.log, STRANGER), 1);
    }
}

#[test]
fn the_entity_of_an_arrival_that_was_sent_on_is_removed_when_its_edge_stays_away_too_long() {
    let mut world = World::stripes();
    let mut runner = world.open().with_gone_after(20);
    let (e, mut bystander) = with_an_arrival_that_was_sent_on(&mut runner);

    drop(e);
    run_until(
        &mut runner,
        &mut [&mut bystander],
        "the link that is subscribed to nothing seeing the arrival's entity removed",
        |_, links| removal(&links[0].log, STRANGER).is_some(),
    );
    settle(&mut runner, &mut [&mut bystander]);
    assert_eq!(
        removal(&bystander.log, STRANGER).map(|(_, _, chunk)| chunk),
        Some(NEXT)
    );
    assert_eq!(removals(&bystander.log, STRANGER), 1);
    assert!(runner.region().edge(E).is_none(), "the edge is forgotten");
}

#[test]
fn the_entity_of_an_arrival_that_was_sent_on_and_confirmed_is_not_reported_removed() {
    let mut world = World::stripes();
    let mut runner = world.open();
    let (mut old, mut bystander) = with_an_arrival_that_was_sent_on(&mut runner);

    // The edge has passed the arrival on to the holder: the entity lives there.
    old.plain(EdgeToWorker::Confirm { number: 1 });
    settle(&mut runner, &mut [&mut old, &mut bystander]);
    let mut new = greeted(&mut runner, E, 6, Vec::new(), Vec::new(), Vec::new());
    ticks(&mut runner, &mut [&mut bystander, &mut new], 5);
    settle(&mut runner, &mut [&mut bystander, &mut new]);
    assert!(removal(&bystander.log, STRANGER).is_none());
    assert!(removal(&new.log, STRANGER).is_none());
}

/// Rule 33, second sentence: if the region has come to hold the chunk the entity was
/// last seen in, the removal comes like any other event, on the links subscribed to
/// that chunk, and on no other.
#[test]
fn once_the_region_holds_the_chunk_the_removal_of_a_departed_entity_goes_to_its_subscribers_only() {
    on_the_gap_in_memory_and_on_disk(
        once_the_region_holds_the_chunk_the_removal_of_a_departed_entity_goes_to_its_subscribers_only_in,
    );
}

fn once_the_region_holds_the_chunk_the_removal_of_a_departed_entity_goes_to_its_subscribers_only_in(
    mut world: World,
) {
    let neighbour = world.neighbour();
    assert_eq!(neighbour.claim(NEXT), Ok(()));
    let mut runner = world.open();
    let mut e = greeted(&mut runner, E, 5, Vec::new(), vec![HOME, NEXT], Vec::new());
    assert_eq!(e.answers(NEXT), vec![(0, Answer::Elsewhere(NEIGHBOUR))]);
    let leaves = join_and_wait(&mut runner, &mut e, 1, player(1));
    e.numbered(2, input(player(1), leaves, 1, move_to(16.5)));
    run_until(
        &mut runner,
        &mut [&mut e],
        "the player being let go to the neighbour",
        |_, links| departure(&links[0].log, player(1)).is_some(),
    );
    assert_eq!(
        departure(&e.log, player(1)).map(|(_, to, _)| to),
        Some(NEIGHBOUR)
    );

    // The neighbour gives the chunk back, and another edge's viewer has the region
    // take it. The departure is still in the outbox of edge E.
    neighbour.give_back(NEXT);
    let mut viewer = linked(&mut runner, F, 5);
    let stale = viewer.subscribe(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut viewer, &mut e], 0, NEXT, stale),
        Answer::Elsewhere(NEIGHBOUR)
    );
    let again = viewer.subscribe(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut viewer, &mut e], 0, NEXT, again),
        Answer::Snapshot
    );
    let mut bystander = linked(&mut runner, G, 5);
    let mut at_home = established(&mut runner, EdgeId(44), 5);
    settle(
        &mut runner,
        &mut [&mut viewer, &mut bystander, &mut at_home, &mut e],
    );

    let mut new = Link::attach(&runner);
    new.hello(E, 6, Vec::new(), Vec::new(), Vec::new());
    run_until(
        &mut runner,
        &mut [&mut viewer, &mut bystander, &mut at_home, &mut new, &mut e],
        "the link that sees the chunk seeing the departed entity removed",
        |_, links| removal(&links[0].log, leaves).is_some(),
    );
    settle(
        &mut runner,
        &mut [&mut viewer, &mut bystander, &mut at_home, &mut new],
    );
    assert_eq!(
        removal(&viewer.log, leaves).map(|(_, _, chunk)| chunk),
        Some(NEXT)
    );
    assert_eq!(removals(&viewer.log, leaves), 1);
    for link in [&bystander, &at_home, &new] {
        assert!(
            removal(&link.log, leaves).is_none(),
            "the region holds the chunk, and this link is not subscribed to it: {}",
            brief(&link.log)
        );
    }
}

// ---------------------------------------------------------------------------------------
// R18. A link that ends takes its tickets with it
// ---------------------------------------------------------------------------------------

#[test]
fn when_a_link_ends_the_regions_granted_chunks_without_a_player_are_returned() {
    on_the_gap_in_memory_and_on_disk(
        when_a_link_ends_the_regions_granted_chunks_without_a_player_are_returned_in,
    );
}

fn when_a_link_ends_the_regions_granted_chunks_without_a_player_are_returned_in(mut world: World) {
    let neighbour = world.neighbour();
    let mut runner = world.open();
    let mut e = greeted(
        &mut runner,
        E,
        5,
        Vec::new(),
        vec![HOME, NEXT, FREE],
        vec![FREE_FAR],
    );
    // A guest of a free chunk is told that it is not the region's, and has no ticket.
    assert_eq!(e.answers(FREE_FAR), vec![(0, Answer::NotMine)]);
    join_and_wait(&mut runner, &mut e, 1, player(1));
    e.numbered(
        2,
        input(player(1), nth_entity(&runner, 1), 1, move_to(16.5)),
    );
    wait_applied(&mut runner, &mut [&mut e], 0, 2);
    assert!(departure(&e.log, player(1)).is_none());

    // The runner notices the link's end with its next step, so the tick after is the
    // first in which no ticket is on the chunks, and with no time before a return the
    // one that gives back what nobody stands in.
    drop(e);
    one_tick(&mut runner, &mut []);
    assert_eq!(runner.region().knowledge(FREE), Knowledge::Unknown);
    claim_until_granted(&mut runner, &mut [], &neighbour, FREE);
    ticks(&mut runner, &mut [], 10);
    for chunk in [HOME, NEXT] {
        assert_eq!(runner.region().knowledge(chunk), Knowledge::Held);
        assert_eq!(neighbour.claim(chunk), Err(world.region()));
    }
    assert_eq!(
        runner
            .region()
            .player(player(1))
            .map(|(_, pose)| pose.position.x),
        Some(16.5),
        "the player stands in the chunk that was kept"
    );
}

#[test]
fn the_chunks_of_a_link_that_ended_go_with_the_forty_first_tick_after_it_ended() {
    let mut world = World::gap();
    let neighbour = world.neighbour();
    let mut runner = world.open_returning_after(40);
    let e = greeted(&mut runner, E, 5, Vec::new(), vec![HOME, NEXT], vec![FREE]);
    // The guest of a free chunk is told that it is not the region's, and has no ticket
    // on it; a viewer of another link then has the region take that chunk.
    assert_eq!(e.answers(FREE), vec![(0, Answer::NotMine)]);
    let mut other = linked(&mut runner, F, 5);
    let ask = other.subscribe(vec![FREE]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut other], 0, FREE, ask),
        Answer::Snapshot
    );

    let left = runner.region().tick_number();
    drop(e);
    drop(other);
    for tick in left + 1..=left + 40 {
        assert_eq!(one_tick(&mut runner, &mut []), tick);
        for chunk in [NEXT, FREE] {
            assert_eq!(
                runner.region().knowledge(chunk),
                Knowledge::Held,
                "{chunk:?} was given back {} ticks after the links ended",
                tick - left
            );
        }
    }
    assert_eq!(neighbour.claim(NEXT), Err(world.region()));
    one_tick(&mut runner, &mut []);
    for chunk in [NEXT, FREE] {
        assert_eq!(
            runner.region().knowledge(chunk),
            Knowledge::Unknown,
            "{chunk:?} was not given back with the forty-first tick after the links ended"
        );
    }
    claim_until_granted(&mut runner, &mut [], &neighbour, NEXT);
    claim_until_granted(&mut runner, &mut [], &neighbour, FREE);
}

#[test]
fn with_forty_ticks_before_a_return_a_new_hello_within_that_time_keeps_every_chunk() {
    on_the_gap_in_memory_and_on_disk(
        with_forty_ticks_before_a_return_a_new_hello_within_that_time_keeps_every_chunk_in,
    );
}

fn with_forty_ticks_before_a_return_a_new_hello_within_that_time_keeps_every_chunk_in(
    mut world: World,
) {
    let neighbour = world.neighbour();
    let mut runner = world.open_returning_after(40);
    let e = greeted(
        &mut runner,
        E,
        5,
        Vec::new(),
        vec![HOME, NEXT, FREE],
        Vec::new(),
    );
    let left = runner.region().tick_number();
    drop(e);
    run_to_tick(&mut runner, &mut [], left + 40);

    // The hello is taken by the tick that would have given the chunks back. One is
    // named as a guest's this time, which keeps a chunk as well.
    let mut back = Link::attach(&runner);
    back.hello(E, 5, Vec::new(), vec![HOME, NEXT], vec![FREE]);
    run_to_tick(&mut runner, &mut [&mut back], left + 120);
    settle(&mut runner, &mut [&mut back]);
    for chunk in [HOME, NEXT, FREE] {
        assert_eq!(back.answers(chunk), vec![(0, Answer::Snapshot)]);
        assert_eq!(runner.region().knowledge(chunk), Knowledge::Held);
        assert_eq!(neighbour.claim(chunk), Err(world.region()));
    }
}

/// The link's end and the `NotMine` for its guest's subscription both take the ticket
/// back. Were it taken back twice or not at all, the chunk would not come and go with
/// the next link as if the guest had never asked.
#[test]
fn a_link_that_ends_around_the_step_in_which_its_guest_is_told_not_mine_leaves_no_ticket_behind() {
    for on_disk in [false, true] {
        for steps_before_the_end in 0..=2 {
            let mut world = World::new(Shape::Gap, on_disk);
            let neighbour = world.neighbour();
            let mut runner = world.open();
            let mut guest = linked(&mut runner, E, 5);
            let mut other = linked(&mut runner, F, 5);
            guest.subscribe_as_guest(vec![NEXT]);
            // With no step, the runner finds the message and the end of the link
            // together: the link ends in the step in which its guest is told.
            for _ in 0..steps_before_the_end {
                runner.step();
            }
            drop(guest);
            ticks(&mut runner, &mut [&mut other], 3);
            assert_eq!(runner.region().knowledge(NEXT), Knowledge::Unknown);

            let ask = other.subscribe(vec![NEXT]);
            assert_eq!(
                wait_answer(&mut runner, &mut [&mut other], 0, NEXT, ask),
                Answer::Snapshot
            );
            other.unsubscribe(vec![NEXT]);
            one_tick(&mut runner, &mut [&mut other]);
            assert_eq!(
                runner.region().knowledge(NEXT),
                Knowledge::Unknown,
                "something still uses the chunk; the link ended {steps_before_the_end} steps behind its asking, on disk: {on_disk}"
            );
            claim_until_granted(&mut runner, &mut [&mut other], &neighbour, NEXT);
        }
    }
}

/// Section 1.2: a player who stands in a chunk is a need. With no ticket on it the
/// chunk is held and not loaded; a guest is then served it, and keeps it when the
/// player has walked on.
#[test]
fn a_guest_is_served_a_chunk_the_region_holds_because_a_player_stands_in_it_and_keeps_it() {
    on_the_gap_in_memory_and_on_disk(
        a_guest_is_served_a_chunk_the_region_holds_because_a_player_stands_in_it_and_keeps_it_in,
    );
}

fn a_guest_is_served_a_chunk_the_region_holds_because_a_player_stands_in_it_and_keeps_it_in(
    mut world: World,
) {
    let neighbour = world.neighbour();
    let mut runner = world.open();
    let mut e = established(&mut runner, E, 5);
    let entity = join_and_wait(&mut runner, &mut e, 1, player(1));
    let mut guest = linked(&mut runner, F, 5);

    // Nobody holds the chunk and no player is near: a guest is no reason to claim.
    let early = guest.subscribe_as_guest(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut guest, &mut e], 0, NEXT, early),
        Answer::NotMine
    );

    e.numbered(2, input(player(1), entity, 1, move_to(16.5)));
    wait_knowledge(
        &mut runner,
        &mut [&mut guest, &mut e],
        NEXT,
        Knowledge::Held,
    );
    ticks(&mut runner, &mut [&mut guest, &mut e], 3);
    assert!(
        runner.region().chunk(NEXT).is_none(),
        "a player does not load a chunk by standing in it"
    );
    let ask = guest.subscribe_as_guest(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut guest, &mut e], 0, NEXT, ask),
        Answer::Snapshot
    );
    let (_, _, _, entities) = snapshot(&guest.log, NEXT).expect("waited for it");
    assert!(entities.iter().any(|state| state.entity == entity));

    // The player walks home. The guest's ticket is what keeps the chunk now.
    e.numbered(3, input(player(1), entity, 2, move_to(13.5)));
    wait_applied(&mut runner, &mut [&mut guest, &mut e], 1, 3);
    for _ in 0..10 {
        one_tick(&mut runner, &mut [&mut guest, &mut e]);
        assert_eq!(runner.region().knowledge(NEXT), Knowledge::Held);
        assert_eq!(neighbour.claim(NEXT), Err(world.region()));
    }
    settle(&mut runner, &mut [&mut guest, &mut e]);
    assert_eq!(
        guest.answers(NEXT),
        vec![(early, Answer::NotMine), (ask, Answer::Snapshot)]
    );
    guest.unsubscribe(vec![NEXT]);
    claim_until_granted(&mut runner, &mut [&mut guest, &mut e], &neighbour, NEXT);
}

/// A player who stands in a chunk keeps it without any ticket. When their edge has
/// stayed away until it is gone, they are removed, nothing uses the chunk any more,
/// and it goes back.
#[test]
fn when_an_edge_is_gone_the_chunk_that_only_its_player_kept_is_returned() {
    let mut world = World::gap();
    let neighbour = world.neighbour();
    let mut runner = world.open().with_gone_after(20);
    let mut e = established(&mut runner, E, 5);
    let entity = join_and_wait(&mut runner, &mut e, 1, player(1));
    let mut watcher = greeted(&mut runner, F, 5, Vec::new(), vec![HOME], Vec::new());
    e.numbered(2, input(player(1), entity, 1, move_to(16.5)));
    wait_knowledge(
        &mut runner,
        &mut [&mut e, &mut watcher],
        NEXT,
        Knowledge::Held,
    );
    wait_applied(&mut runner, &mut [&mut e, &mut watcher], 0, 2);

    let left = runner.region().tick_number();
    drop(e);
    run_to_tick(&mut runner, &mut [&mut watcher], left + 15);
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Held);
    assert_eq!(neighbour.claim(NEXT), Err(world.region()));
    claim_until_granted(&mut runner, &mut [&mut watcher], &neighbour, NEXT);
    assert!(runner.region().player(player(1)).is_none());
    assert!(runner.region().edge(E).is_none());
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Unknown);
    // The player was last seen in a chunk that the link is not subscribed to, and
    // which the region held then: the link that sees only the home chunk is not told.
    settle(&mut runner, &mut [&mut watcher]);
    assert!(
        removal(&watcher.log, entity).is_none(),
        "{}",
        brief(&watcher.log)
    );
}

/// A link that ends while it is still held behind its hello had a viewer's ticket on a
/// free chunk, for which the region has asked the store or been granted the chunk by
/// then. The ticket goes with the link, what the link sent behind the hello is
/// dropped, and the chunk goes back.
#[test]
fn a_link_that_ends_while_it_is_held_behind_its_hello_leaves_no_ticket_behind() {
    for on_disk in [false, true] {
        for steps_before_the_end in 0..=3 {
            let mut world = World::new(Shape::Gap, on_disk);
            let neighbour = world.neighbour();
            let mut runner = world.open();
            let mut held_back = Link::attach(&runner);
            held_back.hello(E, 5, Vec::new(), vec![NEXT], vec![FREE]);
            held_back.numbered(1, join(player(1)));
            for _ in 0..steps_before_the_end {
                runner.step();
            }
            drop(held_back);
            claim_until_granted(&mut runner, &mut [], &neighbour, NEXT);
            assert_eq!(runner.region().knowledge(NEXT), Knowledge::Unknown);
            assert_eq!(neighbour.claim(FREE), Ok(()));
            // Whether the join was applied is up to how far the store had come; what
            // is not up to it is that the chunks were let go of.
            ticks(&mut runner, &mut [], 3);
            assert_eq!(runner.region().loaded_chunk_count(), 0);
        }
    }
}

#[test]
fn a_link_that_ends_takes_its_viewers_ticket_and_with_it_what_the_region_believed_of_the_chunk() {
    let mut world = World::stripes();
    let mut runner = world.open();
    let believer = believing(&mut runner);
    assert_eq!(
        runner.region().knowledge(NEXT),
        Knowledge::Foreign(NEIGHBOUR)
    );
    drop(believer);
    one_tick(&mut runner, &mut []);
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Unknown);
}

// ---------------------------------------------------------------------------------------
// R19. An answer carries the number of the last asking
// ---------------------------------------------------------------------------------------

#[test]
fn of_subscribe_unsubscribe_and_subscribe_with_no_step_in_between_only_the_last_is_answered() {
    on_stripes_in_memory_and_on_disk(
        of_subscribe_unsubscribe_and_subscribe_with_no_step_in_between_only_the_last_is_answered_in,
    );
}

fn of_subscribe_unsubscribe_and_subscribe_with_no_step_in_between_only_the_last_is_answered_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut e = linked(&mut runner, E, 5);
    assert_eq!(e.subscribe(vec![OWN, NEXT]), 1);
    assert_eq!(e.unsubscribe(vec![OWN, NEXT]), 2);
    assert_eq!(e.subscribe(vec![OWN, NEXT]), 3);
    run_until(
        &mut runner,
        &mut [&mut e],
        "the answers to the last asking",
        |_, links| {
            answer_to(&links[0].log, OWN, 3).is_some()
                && answer_to(&links[0].log, NEXT, 3).is_some()
        },
    );
    ticks(&mut runner, &mut [&mut e], 5);
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(e.answers(OWN), vec![(3, Answer::Snapshot)]);
    assert_eq!(e.answers(NEXT), vec![(3, Answer::Elsewhere(NEIGHBOUR))]);
    e.assert_answered(&[]);
}

#[test]
fn a_viewers_asking_overtaken_by_a_guests_before_the_answer_gets_one_not_mine_and_no_elsewhere() {
    on_stripes_in_memory_and_on_disk(
        a_viewers_asking_overtaken_by_a_guests_before_the_answer_gets_one_not_mine_and_no_elsewhere_in,
    );
}

fn a_viewers_asking_overtaken_by_a_guests_before_the_answer_gets_one_not_mine_and_no_elsewhere_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut e = linked(&mut runner, E, 5);
    assert_eq!(e.subscribe(vec![NEXT]), 1);
    assert_eq!(e.subscribe_as_guest(vec![NEXT]), 2);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut e], 0, NEXT, 2),
        Answer::NotMine
    );
    ticks(&mut runner, &mut [&mut e], 5);
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(e.answers(NEXT), vec![(2, Answer::NotMine)]);
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Unknown);
}

#[test]
fn a_guests_asking_overtaken_by_a_viewers_gets_one_snapshot_with_the_later_number() {
    on_stripes_in_memory_and_on_disk(
        a_guests_asking_overtaken_by_a_viewers_gets_one_snapshot_with_the_later_number_in,
    );
}

fn a_guests_asking_overtaken_by_a_viewers_gets_one_snapshot_with_the_later_number_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut e = linked(&mut runner, E, 5);
    assert_eq!(e.subscribe_as_guest(vec![OWN]), 1);
    assert_eq!(e.subscribe(vec![OWN]), 2);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut e], 0, OWN, 2),
        Answer::Snapshot
    );
    ticks(&mut runner, &mut [&mut e], 5);
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(e.answers(OWN), vec![(2, Answer::Snapshot)]);
}

// The same with messages that cross ticks: the first asking is taken by a tick, so that
// the chunk is asked of the store, and the second comes before the store's answer can
// have been taken.

#[test]
fn on_stripes_a_guests_asking_that_follows_a_viewers_a_tick_later_is_the_one_that_is_answered() {
    on_stripes_in_memory_and_on_disk(
        on_stripes_a_guests_asking_that_follows_a_viewers_a_tick_later_is_the_one_that_is_answered_in,
    );
}

fn on_stripes_a_guests_asking_that_follows_a_viewers_a_tick_later_is_the_one_that_is_answered_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut e = linked(&mut runner, E, 5);
    e.subscribe(vec![OWN, NEXT]);
    one_tick(&mut runner, &mut [&mut e]);
    for chunk in [OWN, NEXT] {
        assert_eq!(runner.region().knowledge(chunk), Knowledge::Asked);
    }
    let as_guest = e.subscribe_as_guest(vec![OWN, NEXT]);
    run_until(
        &mut runner,
        &mut [&mut e],
        "the answers to the guest's asking",
        |_, links| {
            answer_to(&links[0].log, OWN, as_guest).is_some()
                && answer_to(&links[0].log, NEXT, as_guest).is_some()
        },
    );
    ticks(&mut runner, &mut [&mut e], 5);
    settle(&mut runner, &mut [&mut e]);
    // The region is pinned to its stripe, where a guest is a reason to claim; in the
    // other stripe the store's answer finds nothing that wants the chunk.
    assert_eq!(e.answers(OWN), vec![(as_guest, Answer::Snapshot)]);
    assert_eq!(e.answers(NEXT), vec![(as_guest, Answer::NotMine)]);
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Unknown);
    e.assert_answered(&[]);
}

#[test]
fn with_the_gap_a_guests_asking_that_follows_a_viewers_a_tick_later_is_the_one_that_is_answered() {
    on_the_gap_in_memory_and_on_disk(
        with_the_gap_a_guests_asking_that_follows_a_viewers_a_tick_later_is_the_one_that_is_answered_in,
    );
}

fn with_the_gap_a_guests_asking_that_follows_a_viewers_a_tick_later_is_the_one_that_is_answered_in(
    mut world: World,
) {
    let neighbour = world.neighbour();
    assert_eq!(neighbour.claim(FREE), Ok(()));
    let mut runner = world.open();
    let mut e = linked(&mut runner, E, 5);
    e.subscribe(vec![NEXT, FREE]);
    one_tick(&mut runner, &mut [&mut e]);
    for chunk in [NEXT, FREE] {
        assert_eq!(runner.region().knowledge(chunk), Knowledge::Asked);
    }
    let as_guest = e.subscribe_as_guest(vec![NEXT, FREE]);
    run_until(
        &mut runner,
        &mut [&mut e],
        "the answers to the guest's asking",
        |_, links| {
            answer_to(&links[0].log, NEXT, as_guest).is_some()
                && answer_to(&links[0].log, FREE, as_guest).is_some()
        },
    );
    // The free chunk was claimed for the viewer and is granted whatever has become of
    // the viewer: the region holds it, and the guest is served and keeps it. The chunk
    // the neighbour holds is nothing the region wants to know of any more.
    for _ in 0..10 {
        one_tick(&mut runner, &mut [&mut e]);
        assert_eq!(runner.region().knowledge(NEXT), Knowledge::Held);
        assert_eq!(neighbour.claim(NEXT), Err(world.region()));
    }
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(e.answers(NEXT), vec![(as_guest, Answer::Snapshot)]);
    assert_eq!(e.answers(FREE), vec![(as_guest, Answer::NotMine)]);
    assert_eq!(runner.region().knowledge(FREE), Knowledge::Unknown);
    e.assert_answered(&[]);
}

#[test]
fn askings_of_several_ticks_before_the_stores_answer_share_one_answer_with_the_last_number() {
    let mut world = World::stripes();
    let mut runner = world.open();
    let mut e = linked(&mut runner, E, 5);
    e.subscribe(vec![OWN, NEXT]);
    one_tick(&mut runner, &mut [&mut e]);
    for chunk in [OWN, NEXT] {
        assert_eq!(runner.region().knowledge(chunk), Knowledge::Asked);
    }
    let last = e.subscribe(vec![OWN, NEXT]);
    run_until(
        &mut runner,
        &mut [&mut e],
        "the answers to the last asking",
        |_, links| {
            answer_to(&links[0].log, OWN, last).is_some()
                && answer_to(&links[0].log, NEXT, last).is_some()
        },
    );
    ticks(&mut runner, &mut [&mut e], 5);
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(e.answers(OWN), vec![(last, Answer::Snapshot)]);
    assert_eq!(e.answers(NEXT), vec![(last, Answer::Elsewhere(NEIGHBOUR))]);
}

#[test]
fn an_asking_that_is_ended_before_the_stores_answer_is_not_answered() {
    let mut world = World::stripes();
    let mut runner = world.open();
    let mut e = linked(&mut runner, E, 5);
    e.subscribe(vec![OWN, NEXT]);
    one_tick(&mut runner, &mut [&mut e]);
    for chunk in [OWN, NEXT] {
        assert_eq!(runner.region().knowledge(chunk), Knowledge::Asked);
    }
    e.unsubscribe(vec![OWN, NEXT]);
    ticks(&mut runner, &mut [&mut e], 10);
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(e.answers(OWN), Vec::new(), "{}", brief(&e.log));
    assert_eq!(e.answers(NEXT), Vec::new(), "{}", brief(&e.log));
    // The answers were taken all the same: a chunk leaves `Asked` only by one. The
    // chunk of the own stripe is the region's and not loaded; the other is forgotten.
    assert_eq!(runner.region().knowledge(OWN), Knowledge::Held);
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Unknown);
    assert_eq!(runner.region().loaded_chunk_count(), 0);
}

/// A player of the link who takes a step with every tick, so that every tick has a
/// commit and nothing of it is on a link before the store has confirmed it.
struct Pacer {
    id: PlayerId,
    entity: EntityId,
    /// The number of the last message of the edge.
    message: u64,
    /// The number of the last input of the player.
    input: u64,
    /// The western end of the stretch of the home chunk the player paces.
    west: f64,
}

impl Pacer {
    /// Joins `id` through `link`, which has sent nothing numbered yet.
    fn join(runner: &mut RegionRunner, link: &mut Link, id: PlayerId) -> Self {
        let entity = join_and_wait(runner, link, 1, id);
        Self {
            id,
            entity,
            message: 1,
            input: 0,
            west: 11.0,
        }
    }

    /// The numbers of the player's next input and of the message that carries it.
    fn next(&mut self) -> (u64, u64) {
        self.message += 1;
        self.input += 1;
        (self.message, self.input)
    }

    /// Sends a step through `links[0]` and runs the tick that takes it. Returns the
    /// tick and where the player stands after it.
    fn tick(&mut self, runner: &mut RegionRunner, links: &mut [&mut Link]) -> (u64, f64) {
        let (message, number) = self.next();
        let x = self.west + (number % 4) as f64 * 0.25;
        links[0].numbered(message, input(self.id, self.entity, number, move_to(x)));
        (one_tick(runner, links), x)
    }

    /// Runs ticks, each with a step, until `done` says so right after one.
    fn until(
        &mut self,
        runner: &mut RegionRunner,
        links: &mut [&mut Link],
        what: &str,
        done: impl Fn(&RegionRunner) -> bool,
    ) {
        for _ in 0..STEPS {
            self.tick(runner, links);
            if done(runner) {
                return;
            }
        }
        panic!("never happened: {what}");
    }
}

/// ADR-0013's reading of rules 8 and 9: a `SubscribeAsGuest` that reaches the region
/// after the tick that made the snapshot changes the kind of a served subscription and
/// is not answered, while the snapshot in flight carries the number from before.
#[test]
fn a_change_of_kind_between_the_tick_that_made_the_snapshot_and_its_publication_is_not_answered() {
    on_the_gap_in_memory_and_on_disk(
        a_change_of_kind_between_the_tick_that_made_the_snapshot_and_its_publication_is_not_answered_in,
    );
}

fn a_change_of_kind_between_the_tick_that_made_the_snapshot_and_its_publication_is_not_answered_in(
    mut world: World,
) {
    let neighbour = world.neighbour();
    let mut runner = world.open();
    let mut e = established(&mut runner, E, 5);
    let mut pacer = Pacer::join(&mut runner, &mut e, player(1));

    let ask = e.subscribe(vec![NEXT]);
    pacer.until(
        &mut runner,
        &mut [&mut e],
        "the chunk being loaded",
        |runner| runner.region().chunk(NEXT).is_some(),
    );
    // The tick that took the chunk in made the snapshot, and has a commit that cannot
    // have been confirmed to the runner yet.
    assert!(
        answer_to(&e.log, NEXT, ask).is_none(),
        "the snapshot is out before its tick's commit can have been confirmed: {}",
        brief(&e.log)
    );
    e.subscribe_as_guest(vec![NEXT]);
    for _ in 0..3 {
        pacer.tick(&mut runner, &mut [&mut e]);
    }
    e.subscribe(vec![NEXT]);
    for _ in 0..3 {
        pacer.tick(&mut runner, &mut [&mut e]);
    }
    e.subscribe_as_guest(vec![NEXT]);

    // It is served as a guest's: events come, and the chunk is kept.
    let (message, number) = pacer.next();
    e.numbered(
        message,
        input(pacer.id, pacer.entity, number, move_to(13.5)),
    );
    let (message, number) = pacer.next();
    e.numbered(
        message,
        input(pacer.id, pacer.entity, number, dig(BEYOND, 1)),
    );
    run_until(
        &mut runner,
        &mut [&mut e],
        "an event of the chunk reaching the guest",
        |_, links| block_change(&links[0].log, BEYOND).is_some(),
    );
    ticks(&mut runner, &mut [&mut e], 5);
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(
        e.answers(NEXT),
        vec![(ask, Answer::Snapshot)],
        "{}",
        brief(&e.log)
    );
    assert_eq!(neighbour.claim(NEXT), Err(world.region()));
    e.assert_answered(&[]);
}

#[test]
fn an_elsewhere_in_flight_when_its_subscription_is_made_a_guests_is_followed_by_a_not_mine() {
    on_stripes_in_memory_and_on_disk(
        an_elsewhere_in_flight_when_its_subscription_is_made_a_guests_is_followed_by_a_not_mine_in,
    );
}

fn an_elsewhere_in_flight_when_its_subscription_is_made_a_guests_is_followed_by_a_not_mine_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut e = established(&mut runner, E, 5);
    let mut pacer = Pacer::join(&mut runner, &mut e, player(1));

    let ask = e.subscribe(vec![NEXT]);
    pacer.until(
        &mut runner,
        &mut [&mut e],
        "the store's answer being taken",
        |runner| runner.region().knowledge(NEXT) == Knowledge::Foreign(NEIGHBOUR),
    );
    assert!(
        answer_to(&e.log, NEXT, ask).is_none(),
        "the answer is out before its tick's commit can have been confirmed: {}",
        brief(&e.log)
    );
    let as_guest = e.subscribe_as_guest(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut e], 0, NEXT, as_guest),
        Answer::NotMine
    );
    ticks(&mut runner, &mut [&mut e], 5);
    settle(&mut runner, &mut [&mut e]);
    // An edge passes the first over: its number is below that of the edge's last
    // message about the chunk (rule 8).
    assert_eq!(
        e.answers(NEXT),
        vec![
            (ask, Answer::Elsewhere(NEIGHBOUR)),
            (as_guest, Answer::NotMine)
        ]
    );
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Unknown);
}

#[test]
fn a_snapshot_in_flight_when_the_link_unsubscribes_has_the_number_from_before() {
    on_the_gap_in_memory_and_on_disk(
        a_snapshot_in_flight_when_the_link_unsubscribes_has_the_number_from_before_in,
    );
}

fn a_snapshot_in_flight_when_the_link_unsubscribes_has_the_number_from_before_in(mut world: World) {
    let mut runner = world.open();
    let mut e = established(&mut runner, E, 5);
    let mut pacer = Pacer::join(&mut runner, &mut e, player(1));

    let ask = e.subscribe(vec![NEXT]);
    pacer.until(
        &mut runner,
        &mut [&mut e],
        "the chunk being loaded",
        |runner| runner.region().chunk(NEXT).is_some(),
    );
    assert!(answer_to(&e.log, NEXT, ask).is_none(), "{}", brief(&e.log));
    e.unsubscribe(vec![NEXT]);
    pacer.tick(&mut runner, &mut [&mut e]);
    let again = e.subscribe(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut e], 0, NEXT, again),
        Answer::Snapshot
    );
    ticks(&mut runner, &mut [&mut e], 5);
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(
        e.answers(NEXT),
        vec![(ask, Answer::Snapshot), (again, Answer::Snapshot)],
        "{}",
        brief(&e.log)
    );
    e.assert_answered(&[]);
}

// ---------------------------------------------------------------------------------------
// R20. A served subscription that changes its kind
// ---------------------------------------------------------------------------------------

#[test]
fn a_served_subscription_that_changes_its_kind_gets_no_answer_and_its_events_go_on_in_every_tick() {
    on_stripes_in_memory_and_on_disk(
        a_served_subscription_that_changes_its_kind_gets_no_answer_and_its_events_go_on_in_every_tick_in,
    );
}

fn a_served_subscription_that_changes_its_kind_gets_no_answer_and_its_events_go_on_in_every_tick_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut e = established(&mut runner, E, 5);
    let mut pacer = Pacer::join(&mut runner, &mut e, player(1));
    let mut steps = Vec::new();
    for round in 0..14 {
        match round {
            // To a guest's, back, a viewer's once more, which changes nothing, and to a
            // guest's and back within one tick.
            2 | 9 => {
                e.subscribe_as_guest(vec![HOME]);
            }
            5 | 7 => {
                e.subscribe(vec![HOME]);
            }
            11 => {
                e.subscribe_as_guest(vec![HOME]);
                e.subscribe(vec![HOME]);
            }
            _ => {}
        }
        steps.push(pacer.tick(&mut runner, &mut [&mut e]));
    }
    wait_applied(&mut runner, &mut [&mut e], 0, pacer.message);
    settle(&mut runner, &mut [&mut e]);

    assert_eq!(
        e.answers(HOME),
        vec![(0, Answer::Snapshot)],
        "{}",
        brief(&e.log)
    );
    let told = events(&e.log);
    for (tick, x) in steps {
        assert!(
            told.iter()
                .any(|(_, of, event)| *of == tick && moved_to(pacer.entity, x)(event)),
            "the step of tick {tick} is missing: {}",
            brief(&e.log)
        );
    }
    e.assert_answered(&[]);
}

#[test]
fn a_subscribe_for_a_chunk_told_elsewhere_then_made_a_guests_and_told_not_mine_begins_anew() {
    on_stripes_in_memory_and_on_disk(
        a_subscribe_for_a_chunk_told_elsewhere_then_made_a_guests_and_told_not_mine_begins_anew_in,
    );
}

fn a_subscribe_for_a_chunk_told_elsewhere_then_made_a_guests_and_told_not_mine_begins_anew_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut e = linked(&mut runner, E, 5);
    let first = e.subscribe(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut e], 0, NEXT, first),
        Answer::Elsewhere(NEIGHBOUR)
    );
    let as_guest = e.subscribe_as_guest(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut e], 0, NEXT, as_guest),
        Answer::NotMine
    );
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Unknown);

    let again = e.subscribe(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut e], 0, NEXT, again),
        Answer::Elsewhere(NEIGHBOUR)
    );
    ticks(&mut runner, &mut [&mut e], 5);
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(
        e.answers(NEXT),
        vec![
            (first, Answer::Elsewhere(NEIGHBOUR)),
            (as_guest, Answer::NotMine),
            (again, Answer::Elsewhere(NEIGHBOUR))
        ]
    );
    assert_eq!(
        runner.region().knowledge(NEXT),
        Knowledge::Foreign(NEIGHBOUR)
    );
    e.assert_answered(&[]);
}

// ---------------------------------------------------------------------------------------
// R21. Numbers out of order
// ---------------------------------------------------------------------------------------

const VIEWER: Said = Said::Asked(Kind::Viewer);
const GUEST: Said = Said::Asked(Kind::Guest);

fn wait_closed(runner: &mut RegionRunner, link: &mut Link, what: &str) {
    run_until(runner, &mut [link], what, |_, links| links[0].closed);
}

#[test]
fn a_subscription_message_whose_number_is_not_above_the_one_before_on_its_link_ends_the_link() {
    let mut world = World::stripes();
    let mut runner = world.open();
    // The number counts per link, whatever the kind of the message and whatever chunk
    // it names.
    let pairs = [
        ((2, VIEWER), (2, VIEWER)),
        ((5, VIEWER), (3, VIEWER)),
        ((2, VIEWER), (2, Said::Ended)),
        ((4, VIEWER), (1, GUEST)),
        ((3, GUEST), (3, VIEWER)),
        ((3, Said::Ended), (2, VIEWER)),
        ((7, GUEST), (6, Said::Ended)),
    ];
    for (first, second) in pairs {
        for with_hello in [false, true] {
            let mut link = Link::attach(&runner);
            if with_hello {
                link.hello(G, 5, Vec::new(), Vec::new(), Vec::new());
            }
            link.say(first.0, first.1, vec![OWN]);
            link.say(second.0, second.1, vec![OWN_TOO]);
            wait_closed(
                &mut runner,
                &mut link,
                &format!("the link closing over {second:?} behind {first:?}"),
            );
        }
    }

    // The lists of a hello are the message with the number 0, and the count begins at
    // 1 on a link without one as well.
    for with_hello in [false, true] {
        for said in [VIEWER, GUEST, Said::Ended] {
            let mut link = Link::attach(&runner);
            if with_hello {
                link.hello(G, 5, Vec::new(), vec![HOME], Vec::new());
            }
            link.say(0, said, vec![OWN]);
            wait_closed(&mut runner, &mut link, "the link closing over a number 0");
        }
    }

    // A tick between the two messages changes nothing about it.
    let mut link = linked(&mut runner, G, 5);
    link.say(4, VIEWER, vec![OWN]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut link], 0, OWN, 4),
        Answer::Snapshot
    );
    link.say(4, Said::Ended, vec![OWN]);
    wait_closed(
        &mut runner,
        &mut link,
        "the link closing over a number again",
    );
}

#[test]
fn numbers_that_rise_by_more_than_one_are_in_order() {
    let mut world = World::stripes();
    let mut runner = world.open();
    let mut link = linked(&mut runner, E, 5);
    link.say(3, VIEWER, vec![OWN]);
    link.say(10, VIEWER, vec![OWN_TOO]);
    link.say(11, Said::Ended, vec![OWN]);
    link.say(40, GUEST, vec![OWN, NEXT]);
    run_until(&mut runner, &mut [&mut link], "the answers", |_, links| {
        links[0].closed
            || [(OWN, 40), (OWN_TOO, 10), (NEXT, 40)]
                .iter()
                .all(|(chunk, ask)| answer_to(&links[0].log, *chunk, *ask).is_some())
    });
    assert!(!link.closed, "{}", brief(&link.log));
    settle(&mut runner, &mut [&mut link]);
    assert_eq!(link.answers(OWN), vec![(40, Answer::Snapshot)]);
    assert_eq!(link.answers(OWN_TOO), vec![(10, Answer::Snapshot)]);
    assert_eq!(link.answers(NEXT), vec![(40, Answer::NotMine)]);
}

#[test]
fn a_hello_after_a_subscription_message_ends_the_link_only_if_it_names_a_chunk() {
    let mut world = World::stripes();
    let mut runner = world.open();

    // Among the viewer's chunks, among the guest's, and after a tick has passed.
    let mut link = Link::attach(&runner);
    link.subscribe(vec![OWN]);
    link.hello(E, 5, Vec::new(), vec![HOME], Vec::new());
    wait_closed(
        &mut runner,
        &mut link,
        "the link closing over a hello with a chunk",
    );

    let mut link = Link::attach(&runner);
    link.unsubscribe(vec![OWN]);
    link.hello(E, 5, Vec::new(), Vec::new(), vec![HOME]);
    wait_closed(
        &mut runner,
        &mut link,
        "the link closing over a hello with a guest's chunk",
    );

    let mut link = Link::attach(&runner);
    let ask = link.subscribe(vec![OWN]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut link], 0, OWN, ask),
        Answer::Snapshot
    );
    link.hello(E, 5, Vec::new(), vec![OWN_TOO], Vec::new());
    wait_closed(
        &mut runner,
        &mut link,
        "the link closing over a late hello with a chunk",
    );

    // A hello that names no chunk leaves the subscriptions and their count alone.
    let mut link = Link::attach(&runner);
    let before = link.subscribe(vec![OWN]);
    link.hello(F, 5, Vec::new(), Vec::new(), Vec::new());
    let after = link.subscribe(vec![OWN_TOO, NEXT]);
    assert_eq!((before, after), (1, 2));
    run_until(
        &mut runner,
        &mut [&mut link],
        "the welcome and the answers",
        |_, links| {
            links[0].closed
                || (applied(&links[0].log).is_some()
                    && answer_to(&links[0].log, OWN, before).is_some()
                    && answer_to(&links[0].log, OWN_TOO, after).is_some()
                    && answer_to(&links[0].log, NEXT, after).is_some())
        },
    );
    assert!(!link.closed, "{}", brief(&link.log));
    assert!(matches!(welcomed(&link.log), Welcome::Unknown { .. }));
    settle(&mut runner, &mut [&mut link]);
    assert_eq!(link.answers(OWN), vec![(before, Answer::Snapshot)]);
    assert_eq!(link.answers(OWN_TOO), vec![(after, Answer::Snapshot)]);
    assert_eq!(
        link.answers(NEXT),
        vec![(after, Answer::Elsewhere(NEIGHBOUR))]
    );
}

// ---------------------------------------------------------------------------------------
// R22. The crossing at a hand-over
// ---------------------------------------------------------------------------------------

/// On stripes: edge E's link with the home chunk and player 1, nothing asked about the
/// other stripe; then, sent together, the player's step into the other stripe and a
/// `Subscribe` for the chunk they step into. Returns the link, the entity and the
/// number of the `Subscribe`.
fn stepping_across_with_a_subscribe(runner: &mut RegionRunner) -> (Link, EntityId, u64) {
    let mut e = established(runner, E, 5);
    let entity = join_and_wait(runner, &mut e, 1, player(1));
    wait_applied(runner, &mut [&mut e], 0, 1);
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Unknown);
    e.numbered(2, input(player(1), entity, 1, move_to(16.5)));
    let ask = e.subscribe(vec![NEXT]);
    (e, entity, ask)
}

#[test]
fn at_a_hand_over_the_departure_comes_before_the_elsewhere_in_the_tick_of_the_stores_answer() {
    on_stripes_in_memory_and_on_disk(
        at_a_hand_over_the_departure_comes_before_the_elsewhere_in_the_tick_of_the_stores_answer_in,
    );
}

fn at_a_hand_over_the_departure_comes_before_the_elsewhere_in_the_tick_of_the_stores_answer_in(
    mut world: World,
) {
    let mut runner = world.open();
    let (mut e, entity, ask) = stepping_across_with_a_subscribe(&mut runner);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut e], 0, NEXT, ask),
        Answer::Elsewhere(NEIGHBOUR)
    );
    ticks(&mut runner, &mut [&mut e], 5);
    settle(&mut runner, &mut [&mut e]);

    let log = &e.log;
    let (let_go, to, transfer) =
        departure(log, player(1)).unwrap_or_else(|| panic!("not let go: {}", brief(log)));
    assert_eq!(to, NEIGHBOUR);
    assert_eq!(transfer.entity_id, entity);
    assert_eq!(transfer.pose.position.x, 16.5);
    let (told, _) = answer_to(log, NEXT, ask).expect("waited for it");
    assert!(let_go < told, "{}", brief(log));
    // One tick: between the two is nothing but what a tick has between its departures
    // and its answers.
    assert!(
        log[let_go + 1..told]
            .iter()
            .all(|message| matches!(place(message), Some(6 | 7))),
        "{}",
        brief(log)
    );
    // It is the tick of the store's answer, not that of the step.
    let stepped = progress_to(log, 2).expect("the step was applied");
    assert!(stepped < let_go, "{}", brief(log));
    assert_eq!(e.answers(NEXT), vec![(ask, Answer::Elsewhere(NEIGHBOUR))]);
    assert!(runner.region().player(player(1)).is_none());
}

#[test]
fn an_unsubscribe_as_soon_as_the_departure_is_read_leaves_nothing_more_to_come_for_the_chunk() {
    on_stripes_in_memory_and_on_disk(
        an_unsubscribe_as_soon_as_the_departure_is_read_leaves_nothing_more_to_come_for_the_chunk_in,
    );
}

fn an_unsubscribe_as_soon_as_the_departure_is_read_leaves_nothing_more_to_come_for_the_chunk_in(
    mut world: World,
) {
    let mut runner = world.open();
    let (mut e, _, ask) = stepping_across_with_a_subscribe(&mut runner);
    run_until(
        &mut runner,
        &mut [&mut e],
        "the player being let go",
        |_, links| departure(&links[0].log, player(1)).is_some(),
    );
    // What the edge does by rule 5: the subscription waited, so it ends it.
    let seen = e.log.len();
    e.unsubscribe(vec![NEXT]);
    ticks(&mut runner, &mut [&mut e], 10);
    settle(&mut runner, &mut [&mut e]);

    // The `Elsewhere` of the departure's tick was on the link with the departure, and
    // has the number of the `Subscribe`, which is below that of the `Unsubscribe`.
    let (let_go, _, _) = departure(&e.log, player(1)).expect("waited for it");
    assert_eq!(e.answers(NEXT), vec![(ask, Answer::Elsewhere(NEIGHBOUR))]);
    let (told, _) = answer_to(&e.log, NEXT, ask).expect("asserted above");
    assert!(let_go < told && told < seen, "{}", brief(&e.log));
    assert!(
        e.log[seen..].iter().all(|message| match message {
            WorkerToEdge::TickDelta { events, .. } =>
                events.iter().all(|event| !event.chunks().contains(&NEXT)),
            other => answer_of(other).is_none_or(|(chunk, _, _)| chunk != NEXT),
        }),
        "{}",
        brief(&e.log[seen..])
    );
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Unknown);
}

#[test]
fn a_player_whose_chunks_subscription_ended_before_the_stores_answer_is_let_go_without_elsewhere() {
    let mut world = World::stripes();
    let mut runner = world.open();
    let (mut e, _, _) = stepping_across_with_a_subscribe(&mut runner);
    one_tick(&mut runner, &mut [&mut e]);
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Asked);
    e.unsubscribe(vec![NEXT]);
    run_until(
        &mut runner,
        &mut [&mut e],
        "the player being let go",
        |_, links| departure(&links[0].log, player(1)).is_some(),
    );
    ticks(&mut runner, &mut [&mut e], 5);
    settle(&mut runner, &mut [&mut e]);
    assert_eq!(
        departure(&e.log, player(1)).map(|(_, to, _)| to),
        Some(NEIGHBOUR)
    );
    assert_eq!(e.answers(NEXT), Vec::new(), "{}", brief(&e.log));
}

// ---------------------------------------------------------------------------------------
// Rule 31. Events for a subscription that waits
// ---------------------------------------------------------------------------------------

#[test]
fn events_come_for_a_waiting_subscription_to_a_chunk_the_region_does_not_hold() {
    on_stripes_in_memory_and_on_disk(
        events_come_for_a_waiting_subscription_to_a_chunk_the_region_does_not_hold_in,
    );
}

fn events_come_for_a_waiting_subscription_to_a_chunk_the_region_does_not_hold_in(mut world: World) {
    let mut runner = world.open();
    let mut e = established(&mut runner, E, 5);
    let entity = join_and_wait(&mut runner, &mut e, 1, player(1));
    let mut watcher = linked(&mut runner, F, 5);
    settle(&mut runner, &mut [&mut e, &mut watcher]);

    // One tick takes the step into the chunk and the subscription to it. The region
    // knows nothing of the chunk; the subscription waits, and the step is an event of
    // the chunk.
    e.numbered(2, input(player(1), entity, 1, move_to(16.5)));
    let ask = watcher.subscribe(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut watcher, &mut e], 0, NEXT, ask),
        Answer::Elsewhere(NEIGHBOUR)
    );
    ticks(&mut runner, &mut [&mut watcher, &mut e], 5);
    settle(&mut runner, &mut [&mut watcher, &mut e]);

    let log = &watcher.log;
    let (step, _) = delta_with(log, shown_at(entity, 16.5))
        .unwrap_or_else(|| panic!("the step was not told: {}", brief(log)));
    let (told, _) = answer_to(log, NEXT, ask).expect("waited for it");
    assert!(step < told, "{}", brief(log));
    assert!(
        events(log).iter().all(|(index, _, _)| *index < told),
        "{}",
        brief(log)
    );
    assert!(departure(&e.log, player(1)).is_some());
}

#[test]
fn with_the_gap_a_chunks_events_come_while_it_is_waited_for_and_its_snapshot_has_who_walked_in() {
    on_the_gap_in_memory_and_on_disk(
        with_the_gap_a_chunks_events_come_while_it_is_waited_for_and_its_snapshot_has_who_walked_in_in,
    );
}

fn with_the_gap_a_chunks_events_come_while_it_is_waited_for_and_its_snapshot_has_who_walked_in_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut e = established(&mut runner, E, 5);
    let entity = join_and_wait(&mut runner, &mut e, 1, player(1));
    let mut watcher = linked(&mut runner, F, 5);
    settle(&mut runner, &mut [&mut e, &mut watcher]);

    e.numbered(2, input(player(1), entity, 1, move_to(16.5)));
    let ask = watcher.subscribe(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut watcher, &mut e], 0, NEXT, ask),
        Answer::Snapshot
    );
    settle(&mut runner, &mut [&mut watcher, &mut e]);

    let log = &watcher.log;
    let (step, _) = delta_with(log, shown_at(entity, 16.5))
        .unwrap_or_else(|| panic!("the step was not told: {}", brief(log)));
    let (at, _, _, entities) = snapshot(log, NEXT).expect("waited for it");
    assert!(step < at, "{}", brief(log));
    assert!(
        entities
            .iter()
            .any(|state| state.entity == entity && state.pose.position.x == 16.5),
        "{}",
        brief(log)
    );
    // The chunk was free: the player is the region's still, in a chunk it has taken.
    assert!(runner.region().player(player(1)).is_some());
    assert!(departure(&e.log, player(1)).is_none());
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Held);
}

// ---------------------------------------------------------------------------------------
// A stale delivery of a chunk ("Found while building", step C2b.3, item 1)
// ---------------------------------------------------------------------------------------

/// A block of the chunk east of the home chunk, high in the air, that a region the test
/// plays sets to mark the chunk as changed by it: where it is inside the chunk.
const MARK: (usize, i32, usize) = (5, 100, 5);

/// A chunk is asked for, let go of and given back, taken and changed by the neighbour,
/// given back, and asked for again. How far the first asking got decides what the
/// store still answers of it; the chunk that is served is the neighbour's in each case.
#[test]
fn a_chunk_that_the_neighbour_changed_between_two_grants_is_served_as_the_neighbour_left_it() {
    for on_disk in [false, true] {
        let mut world = World::new(Shape::Gap, on_disk);
        let neighbour = world.neighbour();
        let mut runner = world.open();
        let mut e = linked(&mut runner, E, 5);
        for round in 0..3 {
            let first = e.subscribe(vec![NEXT]);
            match round {
                // The claim is out, the load is asked for, or the chunk is served.
                0 => {
                    one_tick(&mut runner, &mut [&mut e]);
                }
                1 => wait_knowledge(&mut runner, &mut [&mut e], NEXT, Knowledge::Held),
                _ => {
                    wait_answer(&mut runner, &mut [&mut e], 0, NEXT, first);
                }
            }
            e.unsubscribe(vec![NEXT]);
            claim_until_granted(&mut runner, &mut [&mut e], &neighbour, NEXT);
            let mut chunk = neighbour.load(NEXT);
            chunk.set(MARK.0, MARK.1 + round, MARK.2, blocks::GLASS);
            neighbour.save(NEXT, &chunk);
            neighbour.give_back(NEXT);

            let again = e.subscribe(vec![NEXT]);
            assert_eq!(
                wait_answer(&mut runner, &mut [&mut e], 0, NEXT, again),
                Answer::Snapshot
            );
            let served = snapshot_for(&e.log, NEXT, again).expect("waited for it");
            for earlier in 0..=round {
                assert_eq!(
                    served.get(MARK.0, MARK.1 + earlier, MARK.2),
                    Some(blocks::GLASS),
                    "round {round}, on disk: {on_disk}"
                );
            }
            e.unsubscribe(vec![NEXT]);
            claim_until_granted(&mut runner, &mut [&mut e], &neighbour, NEXT);
            neighbour.give_back(NEXT);
        }
        e.assert_answered(&[]);
    }
}

/// Keeps the store's thread for chunks busy: the western region, which the test plays,
/// asks for many chunks of its own area, and whatever is asked of that thread afterwards
/// is done behind them.
struct Flood {
    western: Other,
    asked: usize,
    answered: usize,
}

impl Flood {
    fn begin(world: &mut World, count: usize) -> Self {
        let western = world.other(WESTERN);
        for index in 0..count {
            let position = ChunkPos::new(-2 - (index % 512) as i32, 300 + (index / 512) as i32);
            western.handle.request(StoreRequest::Load { position });
        }
        Self {
            western,
            asked: count,
            answered: 0,
        }
    }

    /// Reads the answers that are there, and says whether more are to come: whether
    /// the thread for chunks is still busy with what was asked before anything else.
    fn running(&mut self) -> bool {
        while let Some(reply) = self.western.handle.try_reply() {
            assert!(matches!(reply, StoreReply::Loaded { .. }), "{reply:?}");
            self.answered += 1;
        }
        self.answered < self.asked
    }
}

/// Has a test that needs the store to be busy for long enough try again with more
/// work for it. `attempt` says whether the store was still busy when it mattered.
fn with_a_busy_store(attempt: fn(usize) -> bool) {
    let mut count = 2_000;
    while !attempt(count) {
        count *= 4;
        assert!(
            count <= 32_000,
            "the store could not be kept busy for long enough"
        );
    }
}

/// The record: an answer that comes before the region has asked again finds no request,
/// and is dropped. Here the store's answer to the first asking is held back behind
/// other work until the region has dropped the asking and given the chunk back, and
/// stays unread on the region's handle while the neighbour takes the chunk, changes it
/// and gives it back. The region must not take it for the chunk.
#[test]
fn what_the_store_read_for_a_dropped_asking_is_not_served_after_the_neighbour_changed_the_chunk() {
    with_a_busy_store(|count| {
        let mut world = World::gap();
        let neighbour = world.neighbour();
        let mut runner = world.open();
        let mut e = linked(&mut runner, E, 5);
        let mut flood = Flood::begin(&mut world, count);

        // The region is granted the chunk and asks for it to be loaded, behind the
        // flood; then it drops the asking and, with no time before a return, gives the
        // chunk back at once.
        e.subscribe(vec![NEXT]);
        wait_knowledge(&mut runner, &mut [&mut e], NEXT, Knowledge::Held);
        e.unsubscribe(vec![NEXT]);
        one_tick(&mut runner, &mut [&mut e]);
        assert_eq!(runner.region().knowledge(NEXT), Knowledge::Unknown);
        if !flood.running() {
            return false;
        }

        // The runner is not stepped until the neighbour is done, so it reads nothing.
        let mut granted = false;
        for _ in 0..30_000 {
            flood.running();
            if neighbour.claim(NEXT).is_ok() {
                granted = true;
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(granted, "the return never came through");
        let mut chunk = neighbour.load(NEXT);
        chunk.set(MARK.0, MARK.1, MARK.2, blocks::GLASS);
        neighbour.save(NEXT, &chunk);
        neighbour.give_back(NEXT);

        let again = e.subscribe(vec![NEXT]);
        assert_eq!(
            wait_answer(&mut runner, &mut [&mut e], 0, NEXT, again),
            Answer::Snapshot
        );
        let served = snapshot_for(&e.log, NEXT, again).expect("waited for it");
        assert_eq!(
            served.get(MARK.0, MARK.1, MARK.2),
            Some(blocks::GLASS),
            "the chunk is served as the store read it before the neighbour changed it"
        );
        ticks(&mut runner, &mut [&mut e], 5);
        settle(&mut runner, &mut [&mut e]);
        assert_eq!(e.answers(NEXT), vec![(again, Answer::Snapshot)]);
        assert_eq!(
            runner
                .region()
                .chunk(NEXT)
                .and_then(|chunk| chunk.get(MARK.0, MARK.1, MARK.2)),
            Some(blocks::GLASS)
        );
        true
    });
}

/// The record: the runner hands the tick only the answer to the latest request. Here
/// the chunk is asked for, let go of, returned, claimed again, which calls the return
/// off, and asked for again, all while the store is busy with other work, so that both
/// answers come after the second request. From outside the two cannot be told apart,
/// as nothing changed the chunk in between; what shows is that the subscription is
/// served once, with the number of the last asking, and that the chunk is the region's.
#[test]
fn a_chunk_asked_of_the_store_twice_before_it_answers_is_served_once_for_the_last_asking() {
    with_a_busy_store(|count| {
        let mut world = World::gap();
        let neighbour = world.neighbour();
        let mut runner = world.open();
        let mut e = linked(&mut runner, E, 5);
        let mut flood = Flood::begin(&mut world, count);

        e.subscribe(vec![NEXT]);
        wait_knowledge(&mut runner, &mut [&mut e], NEXT, Knowledge::Held);
        e.unsubscribe(vec![NEXT]);
        one_tick(&mut runner, &mut [&mut e]);
        assert_eq!(runner.region().knowledge(NEXT), Knowledge::Unknown);
        let again = e.subscribe(vec![NEXT]);
        wait_knowledge(&mut runner, &mut [&mut e], NEXT, Knowledge::Held);
        // The load is asked for in the tick that takes the grant.
        one_tick(&mut runner, &mut [&mut e]);
        if !flood.running() {
            return false;
        }
        assert_eq!(e.answers(NEXT), Vec::new());

        run_until(
            &mut runner,
            &mut [&mut e],
            "the snapshot for the last asking",
            |_, links| {
                flood.running();
                answer_to(&links[0].log, NEXT, again).is_some()
            },
        );
        ticks(&mut runner, &mut [&mut e], 5);
        settle(&mut runner, &mut [&mut e]);
        assert_eq!(e.answers(NEXT), vec![(again, Answer::Snapshot)]);
        assert_eq!(runner.region().knowledge(NEXT), Knowledge::Held);
        assert!(runner.region().chunk(NEXT).is_some());
        // The return that was called off has freed nothing.
        assert_eq!(neighbour.claim(NEXT), Err(world.region()));
        e.assert_answered(&[]);
        true
    });
}

// ---------------------------------------------------------------------------------------
// Section 1.2 on two links, and section 4.8
// ---------------------------------------------------------------------------------------

#[test]
fn a_chunk_that_a_viewer_and_a_guest_of_two_links_see_stays_served_until_both_have_let_go() {
    on_the_gap_in_memory_and_on_disk(
        a_chunk_that_a_viewer_and_a_guest_of_two_links_see_stays_served_until_both_have_let_go_in,
    );
}

fn a_chunk_that_a_viewer_and_a_guest_of_two_links_see_stays_served_until_both_have_let_go_in(
    mut world: World,
) {
    let neighbour = world.neighbour();
    let mut runner = world.open();
    let (mut viewer, _) = granted_for_a_viewer(&mut runner);
    let mut guest = linked(&mut runner, F, 5);
    let ask = guest.subscribe_as_guest(vec![NEXT]);
    assert_eq!(
        wait_answer(&mut runner, &mut [&mut guest, &mut viewer], 0, NEXT, ask),
        Answer::Snapshot
    );

    // The viewer lets go. The guest is no reason to claim a chunk of open land, and it
    // is one to keep it: the chunk stays loaded, and the guest goes on seeing it.
    viewer.unsubscribe(vec![NEXT]);
    ticks(&mut runner, &mut [&mut guest, &mut viewer], 5);
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Held);
    assert_eq!(neighbour.claim(NEXT), Err(world.region()));
    viewer.numbered(
        2,
        input(player(1), nth_entity(&runner, 1), 1, dig(BEYOND, 1)),
    );
    run_until(
        &mut runner,
        &mut [&mut guest, &mut viewer],
        "the guest seeing the block change",
        |_, links| block_change(&links[0].log, BEYOND).is_some(),
    );
    settle(&mut runner, &mut [&mut guest, &mut viewer]);
    assert!(block_change(&viewer.log, BEYOND).is_none());
    assert_eq!(guest.answers(NEXT), vec![(ask, Answer::Snapshot)]);

    guest.unsubscribe(vec![NEXT]);
    claim_until_granted(
        &mut runner,
        &mut [&mut guest, &mut viewer],
        &neighbour,
        NEXT,
    );
    assert_eq!(block_in(&neighbour.load(NEXT), BEYOND), blocks::AIR);
}

#[test]
fn the_status_counts_the_chunks_the_store_has_granted_and_says_where_the_players_are() {
    let mut world = World::gap();
    let mut runner = world.open();
    let status = runner.status();
    let (mut e, _) = granted_for_a_viewer(&mut runner);
    wait_applied(&mut runner, &mut [&mut e], 0, 1);
    assert_eq!(status.held.load(Ordering::SeqCst), 2);
    assert_eq!(status.crowds(), vec![(HOME, 1)]);

    // The player walks into the chunk east of home, and the link lets go of it: the
    // player standing there keeps it.
    e.numbered(
        2,
        input(player(1), nth_entity(&runner, 1), 1, move_to(16.5)),
    );
    wait_applied(&mut runner, &mut [&mut e], 0, 2);
    assert_eq!(status.crowds(), vec![(NEXT, 1)]);
    e.unsubscribe(vec![NEXT]);
    ticks(&mut runner, &mut [&mut e], 5);
    assert_eq!(status.held.load(Ordering::SeqCst), 2);

    // Back home, nothing uses the chunk, and it goes.
    e.numbered(
        3,
        input(player(1), nth_entity(&runner, 1), 2, move_to(13.5)),
    );
    wait_applied(&mut runner, &mut [&mut e], 0, 3);
    ticks(&mut runner, &mut [&mut e], 2);
    assert_eq!(status.held.load(Ordering::SeqCst), 1);
    assert_eq!(status.crowds(), vec![(HOME, 1)]);
    assert_eq!(runner.region().knowledge(NEXT), Knowledge::Unknown);
}

// ---------------------------------------------------------------------------------------
// Generated runs: links that say what they like of their subscriptions, in any order a
// link may, while a player paces and digs. Whatever the store's pace, every link's log
// keeps to section 5, every asking ends as the division of the world calls for, a
// snapshot is the chunk after its tick, what a served link was told adds up to the
// chunk as the region has it, no ticket is left behind, and a chunk that leaves the
// region is stored with every change.
// ---------------------------------------------------------------------------------------

/// Numbers from a seed, the same on every machine.
struct Dice(u64);

impl Dice {
    fn new(seed: u64) -> Self {
        // The generator must not start from 0, which it would never leave.
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn roll(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.roll() % bound
    }

    /// Some of `chunks`, and at least one.
    fn some_of(&mut self, chunks: &[ChunkPos]) -> Vec<ChunkPos> {
        let mut chosen: Vec<ChunkPos> = chunks
            .iter()
            .copied()
            .filter(|_| self.below(3) == 0)
            .collect();
        if chosen.is_empty() {
            chosen.push(chunks[self.below(chunks.len() as u64) as usize]);
        }
        chosen
    }
}

/// Whose a chunk is by the division of a world, in a run in which no other region
/// claims anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Whose {
    /// The region under test is pinned to it, or it is the home chunk: the store
    /// grants it to the region whenever it asks.
    Own,
    /// Nobody's until the region under test claims it.
    Free,
    /// Another region is pinned to it.
    Another(RegionId),
}

fn whose(shape: Shape, chunk: ChunkPos) -> Whose {
    match shape {
        Shape::Stripes if chunk.x < 1 => Whose::Own,
        Shape::Stripes => Whose::Another(NEIGHBOUR),
        Shape::Gap if chunk == HOME => Whose::Own,
        Shape::Gap if chunk.x < 0 => Whose::Another(WESTERN),
        Shape::Gap if chunk.x >= 16 => Whose::Another(NEIGHBOUR),
        Shape::Gap => Whose::Free,
    }
}

impl Link {
    /// The link's last asking for `chunk`, if it has not ended it since.
    fn asking(&self, chunk: ChunkPos) -> Option<(u64, Kind)> {
        match self.book.said.get(&chunk)?.last()? {
            (number, Said::Asked(kind)) => Some((*number, *kind)),
            (_, Said::Ended) => None,
        }
    }

    /// Whether the subscription that the last asking for `chunk` is about is served: a
    /// snapshot answered that asking, or an earlier one that the link has not ended
    /// since.
    fn served(&self, chunk: ChunkPos) -> bool {
        let Some((number, _)) = self.asking(chunk) else {
            return false;
        };
        let said = &self.book.said[&chunk];
        self.answers(chunk).iter().any(|(ask, answer)| {
            *answer == Answer::Snapshot
                && *ask <= number
                && !said
                    .iter()
                    .any(|(ended, what)| *what == Said::Ended && ask < ended && *ended < number)
        })
    }

    /// Whether the last asking for `chunk` was answered with `answer`.
    fn told(&self, chunk: ChunkPos, answer: Answer) -> bool {
        self.asking(chunk)
            .is_some_and(|(number, _)| self.answers(chunk).contains(&(number, answer)))
    }

    /// Whether the link's last asking for `chunk` has ended as the division calls for
    /// (section 4.4): a chunk the region is granted is served, to a viewer and to a
    /// guest; one that another region holds is `Elsewhere` to a viewer and `NotMine`
    /// to a guest; a free one is served to a viewer, and to a guest served if the
    /// region holds it for someone and else `NotMine`.
    fn settled(&self, shape: Shape, chunk: ChunkPos) -> bool {
        let Some((_, kind)) = self.asking(chunk) else {
            return true;
        };
        match (whose(shape, chunk), kind) {
            (Whose::Own, _) | (Whose::Free, Kind::Viewer) => self.served(chunk),
            (Whose::Free, Kind::Guest) => self.served(chunk) || self.told(chunk, Answer::NotMine),
            (Whose::Another(region), Kind::Viewer) => self.told(chunk, Answer::Elsewhere(region)),
            (Whose::Another(_), Kind::Guest) => self.told(chunk, Answer::NotMine),
        }
    }

    /// Whether the last asking for `chunk` has been answered, with whatever, or was
    /// about a subscription that was served already.
    fn answered(&self, chunk: ChunkPos) -> bool {
        self.asking(chunk).is_none_or(|(number, _)| {
            self.served(chunk) || self.answers(chunk).iter().any(|(ask, _)| *ask == number)
        })
    }

    /// Whether the link has a subscription to `chunk` that the region has not ended.
    fn subscribed(&self, chunk: ChunkPos) -> bool {
        self.asking(chunk).is_some() && !self.told(chunk, Answer::NotMine)
    }
}

/// Where a link that takes everything it is told believes `entity` to be, if anywhere:
/// where a snapshot or an event last showed it, unless an event said it is gone or a
/// snapshot of the chunk it was believed in does not have it.
fn believed_at(log: &[WorkerToEdge], entity: EntityId) -> Option<Vec3> {
    let mut at: Option<Vec3> = None;
    for message in log {
        match message {
            WorkerToEdge::ChunkSnapshot {
                position, entities, ..
            } => match entities.iter().find(|state| state.entity == entity) {
                Some(state) => at = Some(state.pose.position),
                None if at.is_some_and(|at| ChunkPos::containing(at.x, at.z) == *position) => {
                    at = None;
                }
                None => {}
            },
            WorkerToEdge::TickDelta { events, .. } => {
                for event in events {
                    match event {
                        RegionEvent::EntitySpawned(state) if state.entity == entity => {
                            at = Some(state.pose.position);
                        }
                        RegionEvent::EntityMoved {
                            entity: who, pose, ..
                        } if *who == entity => at = Some(pose.position),
                        RegionEvent::EntityRemoved { entity: who, .. } if *who == entity => {
                            at = None;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    at
}

/// The blocks a generated run follows, as they were in a chunk at some moment.
type Blocks = BTreeMap<BlockPos, BlockState>;

fn blocks_of(chunk: &Chunk, position: ChunkPos, followed: &[BlockPos]) -> Blocks {
    followed
        .iter()
        .filter(|block| block.chunk() == position)
        .map(|block| (*block, block_in(chunk, *block)))
        .collect()
}

/// What a generated run knows of the followed blocks of the region's loaded chunks.
struct Seen {
    followed: Vec<BlockPos>,
    /// The followed blocks as the world is generated.
    generated: Blocks,
    /// The ticks of the run in which links spoke and the player dug.
    first: u64,
    last: u64,
    /// The blocks of each chunk after each of those ticks after which it was loaded.
    after: BTreeMap<(u64, ChunkPos), Blocks>,
    /// Each block as it was when its chunk was last seen loaded, or as the neighbour
    /// has stored it since.
    latest: Blocks,
}

impl Seen {
    /// What a run follows: `followed`, in the home chunk and the chunk east of it.
    fn of(followed: Vec<BlockPos>, runner: &RegionRunner) -> Self {
        let generator = FlatGenerator::classic();
        let generated: Blocks = [HOME, NEXT]
            .into_iter()
            .flat_map(|position| blocks_of(&generator.generate(position), position, &followed))
            .collect();
        Self {
            followed,
            latest: generated.clone(),
            generated,
            first: runner.region().tick_number() + 1,
            last: runner.region().tick_number(),
            after: BTreeMap::new(),
        }
    }

    /// Notes the followed blocks of the chunks that are loaded after `tick`. A block
    /// changes only when the player digs it, in a loaded chunk; so one that is other
    /// than it was last seen, and is not air where there was grass, is a change that
    /// was lost between two loads of its chunk.
    fn note(&mut self, runner: &RegionRunner, tick: u64) {
        for position in [HOME, NEXT] {
            if let Some(chunk) = runner.region().chunk(position) {
                let blocks = blocks_of(chunk, position, &self.followed);
                for (block, state) in &blocks {
                    let before = self.latest[block];
                    assert!(
                        *state == before
                            || (before == blocks::GRASS_BLOCK && *state == blocks::AIR),
                        "after tick {tick} {block:?} is {state:?}, and it was {before:?}"
                    );
                }
                self.latest.extend(blocks.clone());
                self.after.insert((tick, position), blocks);
            }
        }
        self.last = tick;
    }

    fn latest_of(&self, position: ChunkPos) -> Blocks {
        self.latest
            .iter()
            .filter(|(block, _)| block.chunk() == position)
            .map(|(block, state)| (*block, *state))
            .collect()
    }

    /// Holds every snapshot on `link` to rule 11: it is the chunk after its tick.
    fn check_snapshots(&self, link: &Link) {
        for message in &link.log {
            let WorkerToEdge::ChunkSnapshot {
                position,
                tick,
                chunk,
                ..
            } = message
            else {
                continue;
            };
            if ![HOME, NEXT].contains(position) {
                continue;
            }
            let expected = if *tick < self.first {
                // Nothing was dug before the run began.
                self.generated
                    .iter()
                    .filter(|(block, _)| block.chunk() == *position)
                    .map(|(block, state)| (*block, *state))
                    .collect()
            } else if *tick > self.last {
                // Nothing was dug after it ended.
                self.latest_of(*position)
            } else {
                self.after
                    .get(&(*tick, *position))
                    .cloned()
                    .unwrap_or_else(|| {
                        panic!(
                            "a snapshot of {position:?} at tick {tick}, after which it was not loaded: {}",
                            brief(&link.log)
                        )
                    })
            };
            assert_eq!(
                blocks_of(chunk, *position, &self.followed),
                expected,
                "the snapshot of {position:?} at tick {tick} is not the chunk after that tick: {}",
                brief(&link.log)
            );
        }
    }
}

/// How many ticks of a generated run have links speaking and the player digging.
const ROUNDS: usize = 120;

/// How many blocks of the chunk east of the home chunk the neighbour of a generated
/// run can mark as stored by it, one above the other, where the world has air.
const MARKS: i32 = 16;

fn mark(index: i32) -> BlockPos {
    BlockPos::new(
        NEXT.x * 16 + MARK.0 as i32,
        MARK.1 + index,
        NEXT.z * 16 + MARK.2 as i32,
    )
}

/// One generated run. With `beside_a_neighbour`, which needs the gap, the neighbour
/// takes free chunks whenever the region does not hold them, puts a mark into the one
/// the player digs in, and gives them back, as it likes.
fn a_generated_run(shape: Shape, on_disk: bool, seed: u64, beside_a_neighbour: bool) {
    // What the run does is printed, and shown if it fails.
    eprintln!(
        "a generated run on {shape:?}, on disk: {on_disk}, with seed {seed}, beside a neighbour: {beside_a_neighbour}"
    );
    let mut dice = Dice::new(seed);
    let mut world = World::new(shape, on_disk);
    let return_after = match shape {
        Shape::Stripes => 0,
        Shape::Gap => seed % 3,
    };
    let neighbour = world.neighbour();
    let mut runner = world.open_returning_after(return_after);
    if seed % 2 == 1 {
        // Checkpoints every few ticks, which save what is loaded and changed, and
        // must leave alone what the region has given back.
        runner = runner.with_checkpoint_interval(2 + seed % 7);
    }
    let west = ChunkPos::new(-1, 0);
    let chunks = match shape {
        Shape::Stripes => [HOME, west, OWN, NEXT, OTHER],
        Shape::Gap => [HOME, NEXT, FREE, EAST, west],
    };
    // The player digs in the home chunk and, where the region can come to hold it, in
    // the chunk east of it.
    let mut followed: Vec<BlockPos> = (11..=13)
        .flat_map(|x| [9, 10].map(|z| BlockPos::new(x, GROUND, z)))
        .collect();
    if shape == Shape::Gap {
        followed.extend((6..=11).map(|z| BlockPos::new(16, GROUND, z)));
    }
    let diggable = followed.len();
    followed.extend((0..MARKS).map(mark));
    // What the neighbour holds, and how many marks it has made.
    let mut theirs: BTreeSet<ChunkPos> = BTreeSet::new();
    let mut marks = 0;

    let edges = [E, F, G];
    let mut links = vec![
        established(&mut runner, E, 5),
        linked(&mut runner, F, 5),
        linked(&mut runner, G, 5),
    ];
    let mut pacer = Pacer::join(&mut runner, &mut links[0], player(1));
    if shape == Shape::Gap {
        pacer.west = 13.0;
    }
    let mut sequence = 0;
    let mut seen = Seen::of(followed, &runner);
    // With the gap, and nobody else taking chunks, the player walks between the home
    // chunk and the free chunk east of it, which the region takes and keeps because
    // they stand in it, whatever the links say.
    let wanders = shape == Shape::Gap && !beside_a_neighbour;
    let spots = [13.5, 16.5, 19.5];
    let mut spot = 0;

    for _ in 0..ROUNDS {
        let coming = runner.region().tick_number() + 1;
        for index in 0..links.len() {
            // Now and then a link other than the player's ends, and a new one of its
            // edge says hello with lists of its own.
            if index > 0 && dice.below(40) == 0 {
                let viewers = dice.some_of(&chunks);
                let guests = dice.some_of(&chunks);
                eprintln!(
                    "before tick {coming}: link {index} ends, and a new one says hello with {viewers:?} and the guests {guests:?}"
                );
                seen.check_snapshots(&links[index]);
                let mut link = Link::attach(&runner);
                link.hello(edges[index], 5, Vec::new(), viewers, guests);
                links[index] = link;
                continue;
            }
            for _ in 0..[0, 0, 0, 1, 1, 2][dice.below(6) as usize] {
                // Beside a neighbour links let go more often, so that chunks are free
                // for it to take.
                let said = if beside_a_neighbour {
                    [VIEWER, GUEST, Said::Ended, Said::Ended][dice.below(4) as usize]
                } else {
                    [VIEWER, GUEST, Said::Ended][dice.below(3) as usize]
                };
                let named = dice.some_of(&chunks);
                let ask = links[index].next_ask();
                eprintln!(
                    "before tick {coming}: link {index} says {said:?} with {ask} of {named:?}"
                );
                links[index].say(ask, said, named);
            }
        }
        if beside_a_neighbour && dice.below(2) == 0 {
            let chunk = [NEXT, FREE][dice.below(2) as usize];
            if theirs.contains(&chunk) {
                // It keeps a chunk for a while, in which the region is told so.
                if dice.below(4) == 0 {
                    eprintln!("before tick {coming}: the neighbour gives {chunk:?} back");
                    theirs.remove(&chunk);
                    neighbour.give_back(chunk);
                }
            } else {
                match neighbour.claim(chunk) {
                    Ok(()) => {
                        eprintln!("before tick {coming}: the neighbour is granted {chunk:?}");
                        theirs.insert(chunk);
                        if chunk == NEXT {
                            // The chunk left the region with every change, and the
                            // neighbour stores one of its own.
                            let mut stored = neighbour.load(NEXT);
                            assert_eq!(
                                blocks_of(&stored, NEXT, &seen.followed),
                                seen.latest_of(NEXT),
                                "the chunk left the region without every change"
                            );
                            if marks < MARKS {
                                let (x, z) = mark(marks).in_chunk();
                                stored.set(x, mark(marks).y, z, blocks::GLASS);
                                neighbour.save(NEXT, &stored);
                                seen.latest.insert(mark(marks), blocks::GLASS);
                                marks += 1;
                            }
                        }
                    }
                    Err(holder) => assert_eq!(holder, world.region()),
                }
            }
        }
        if dice.below(6) == 0 {
            let block = seen.followed[dice.below(diggable as u64) as usize];
            sequence += 1;
            let (message, number) = pacer.next();
            eprintln!("before tick {coming}: the player digs {block:?}");
            links[0].numbered(
                message,
                input(pacer.id, pacer.entity, number, dig(block, sequence)),
            );
        }
        let walks = wanders && dice.below(3) == 0;
        if walks {
            spot = match (spot, dice.below(2)) {
                (1, 0) => 0,
                (1, _) => 2,
                _ => 1,
            };
            let (message, number) = pacer.next();
            eprintln!(
                "before tick {coming}: the player steps to x = {}",
                spots[spot]
            );
            links[0].numbered(
                message,
                input(pacer.id, pacer.entity, number, move_to(spots[spot])),
            );
        }
        let mut all: Vec<&mut Link> = links.iter_mut().collect();
        // A player who stays in the home chunk paces in most ticks, so that they have a
        // commit; some ticks have none.
        let tick = if wanders || dice.below(4) == 0 {
            one_tick(&mut runner, &mut all)
        } else {
            pacer.tick(&mut runner, &mut all).0
        };
        assert_eq!(tick, coming);
        seen.note(&runner, tick);
        if wanders {
            let (_, pose) = runner
                .region()
                .player(pacer.id)
                .expect("no chunk the player walks into is another region's");
            let stands = ChunkPos::containing(pose.position.x, pose.position.z);
            let knowledge = runner.region().knowledge(stands);
            assert!(
                matches!(knowledge, Knowledge::Held | Knowledge::Asked),
                "after tick {tick} the player stands in {stands:?}, of which the region knows {knowledge:?}"
            );
        }
    }

    let mut all: Vec<&mut Link> = links.iter_mut().collect();
    // What the player did last can still wait for its chunk: a dig into a chunk that
    // the player's link asks for and the region is about to serve is judged when the
    // chunk is there (ADR-0014, section 3.6), and what the link sent behind it waits
    // with it. The run is followed until the region has applied all of it, so that
    // nothing is dug after what is noted here.
    let applied = |runner: &RegionRunner| runner.region().edge(E).map_or(0, |edge| edge.applied);
    for _ in 0..STEPS {
        if applied(&runner) >= pacer.message {
            break;
        }
        let tick = one_tick(&mut runner, &mut all);
        seen.note(&runner, tick);
        // While the player's link waits, ticks have nothing to commit and do not wait
        // for the store, which answers on its own threads; this gives them the
        // processor.
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        applied(&runner) >= pacer.message,
        "the region has applied {} of the {} messages of the player's edge; of the chunks it knows {:?}, and has loaded {:?}: {}",
        applied(&runner),
        pacer.message,
        chunks.map(|chunk| runner.region().knowledge(chunk)),
        chunks.map(|chunk| runner.region().chunk(chunk).is_some()),
        brief(&all[0].log)
    );
    if beside_a_neighbour {
        // The neighbour gives back what it has, and every link asks for the free
        // chunks as a viewer, twice: a region that believed the neighbour learns
        // otherwise only when a link that was told so asks again, and may have told
        // another link what it believed in the meantime (rule 13).
        for chunk in std::mem::take(&mut theirs) {
            neighbour.give_back(chunk);
        }
        for _ in 0..2 {
            for link in all.iter_mut() {
                link.subscribe(vec![NEXT, FREE]);
            }
            run_until(
                &mut runner,
                &mut all,
                "the askings for the free chunks being answered",
                |_, links| {
                    links
                        .iter()
                        .all(|link| link.answered(NEXT) && link.answered(FREE))
                },
            );
            settle(&mut runner, &mut all);
        }
    }

    // Nobody says anything more, and everything that was asked comes to its end.
    wait_applied(&mut runner, &mut all, 0, pacer.message);
    run_until(
        &mut runner,
        &mut all,
        "every asking ending as the division of the world calls for",
        |_, links| {
            links
                .iter()
                .all(|link| chunks.iter().all(|chunk| link.settled(shape, *chunk)))
        },
    );
    settle(&mut runner, &mut all);
    ticks(&mut runner, &mut all, return_after + 3);
    settle(&mut runner, &mut all);

    let truly = runner
        .region()
        .player(pacer.id)
        .expect("the player never walked into another region's chunk")
        .1
        .position;
    let stands = ChunkPos::containing(truly.x, truly.z);
    for (index, link) in links.iter().enumerate() {
        assert!(!link.closed, "link {index} was closed");
        link.assert_answered(&[]);
        seen.check_snapshots(link);
        for chunk in chunks {
            assert!(
                link.settled(shape, chunk),
                "link {index} and {chunk:?}: {}",
                brief(&link.log)
            );
        }
        // Rule 11: from the snapshot on, every event of the chunk comes. So the
        // snapshot and what followed it add up to the chunk as the region has it.
        for position in [HOME, NEXT] {
            if !link.served(position) {
                continue;
            }
            let loaded = runner.region().chunk(position).unwrap_or_else(|| {
                panic!("link {index} is served {position:?}, which is not loaded")
            });
            let (at, chunk) = link
                .log
                .iter()
                .enumerate()
                .rev()
                .find_map(|(at, message)| match message {
                    WorkerToEdge::ChunkSnapshot {
                        position: of,
                        chunk,
                        ..
                    } if *of == position => Some((at, chunk)),
                    _ => None,
                })
                .expect("a served subscription has a snapshot");
            let mut replica = blocks_of(chunk, position, &seen.followed);
            for (told, _, event) in events(&link.log) {
                if let RegionEvent::BlockChanged {
                    position: block,
                    state,
                } = event
                    && told > at
                    && replica.contains_key(block)
                {
                    replica.insert(*block, *state);
                }
            }
            assert_eq!(
                replica,
                blocks_of(loaded, position, &seen.followed),
                "what link {index} was told of {position:?} is not what the region has: {}",
                brief(&link.log)
            );
        }
        // The same for the player: a link that is served the chunk they stand in knows
        // where they are, and one that is served a chunk they have left knows that.
        let believed = believed_at(&link.log, pacer.entity);
        if link.served(stands) {
            assert_eq!(
                believed,
                Some(truly),
                "where link {index} was told the player stands: {}",
                brief(&link.log)
            );
        } else if let Some(at) = believed {
            let chunk = ChunkPos::containing(at.x, at.z);
            assert!(
                !link.served(chunk),
                "link {index} is served {chunk:?} and was not told that the player left it: {}",
                brief(&link.log)
            );
        }
    }

    // What the region knows follows from its tickets and the store's answers alone.
    for chunk in chunks {
        let knowledge = runner.region().knowledge(chunk);
        let loaded = runner.region().chunk(chunk).is_some();
        let subscribed = links.iter().any(|link| link.subscribed(chunk));
        let viewed = links.iter().any(|link| {
            link.subscribed(chunk) && matches!(link.asking(chunk), Some((_, Kind::Viewer)))
        });
        let expected = match whose(shape, chunk) {
            Whose::Own | Whose::Free if subscribed || chunk == stands => Knowledge::Held,
            // The home chunk is never given back, and neither is a chunk of a pinned
            // area once the region has claimed it.
            Whose::Own if knowledge == Knowledge::Held => Knowledge::Held,
            Whose::Own | Whose::Free => Knowledge::Unknown,
            Whose::Another(region) if viewed => Knowledge::Foreign(region),
            Whose::Another(_) => Knowledge::Unknown,
        };
        assert_eq!(knowledge, expected, "of {chunk:?}");
        assert_eq!(
            loaded,
            subscribed && expected == Knowledge::Held,
            "whether {chunk:?} is loaded"
        );
    }

    // The player walks home, and everything is let go of: one link says so, the others
    // end. Nothing stays loaded, no belief stays, and with the gap every free chunk
    // goes back.
    links.truncate(1);
    while spot > 0 {
        spot -= 1;
        let (message, number) = pacer.next();
        links[0].numbered(
            message,
            input(pacer.id, pacer.entity, number, move_to(spots[spot])),
        );
        wait_applied(&mut runner, &mut [&mut links[0]], 0, message);
    }
    links[0].unsubscribe(chunks.to_vec());
    let mut all: Vec<&mut Link> = links.iter_mut().collect();
    ticks(&mut runner, &mut all, return_after + 3);
    assert_eq!(runner.region().loaded_chunk_count(), 0);
    for chunk in chunks {
        let knowledge = runner.region().knowledge(chunk);
        match whose(shape, chunk) {
            Whose::Own => assert_ne!(knowledge, Knowledge::Asked, "of {chunk:?}"),
            Whose::Free | Whose::Another(_) => {
                assert_eq!(knowledge, Knowledge::Unknown, "of {chunk:?}")
            }
        }
    }
    if shape == Shape::Gap {
        for chunk in [NEXT, FREE] {
            claim_until_granted(&mut runner, &mut all, &neighbour, chunk);
        }
        let stored = neighbour.load(NEXT);
        assert_eq!(
            blocks_of(&stored, NEXT, &seen.followed),
            seen.latest_of(NEXT),
            "the chunk left the region without every change"
        );
    }

    // And after a crash the home chunk is as the region last had it.
    let mut next = world.open();
    drop(runner);
    let again = greeted(&mut next, E, 5, vec![pacer.id], vec![HOME], Vec::new());
    let chunk = snapshot_for(&again.log, HOME, 0).expect("the hello's chunk is answered");
    assert_eq!(
        blocks_of(chunk, HOME, &seen.followed),
        seen.latest_of(HOME),
        "the home chunk after a crash"
    );
}

#[test]
fn generated_runs_on_stripes_keep_to_section_5_and_end_as_the_division_calls_for() {
    for seed in 0..10 {
        a_generated_run(Shape::Stripes, false, seed, false);
    }
}

#[test]
fn generated_runs_on_stripes_on_disk_keep_to_section_5_and_end_as_the_division_calls_for() {
    for seed in 100..104 {
        a_generated_run(Shape::Stripes, true, seed, false);
    }
}

#[test]
fn generated_runs_with_the_gap_keep_to_section_5_and_leave_every_chunk_saved() {
    for seed in 0..12 {
        a_generated_run(Shape::Gap, false, seed, false);
    }
}

#[test]
fn generated_runs_with_the_gap_on_disk_keep_to_section_5_and_leave_every_chunk_saved() {
    for seed in 100..104 {
        a_generated_run(Shape::Gap, true, seed, false);
    }
}

#[test]
fn generated_runs_beside_a_neighbour_that_takes_chunks_keep_to_section_5_and_lose_no_change() {
    for seed in 0..12 {
        a_generated_run(Shape::Gap, false, seed, true);
    }
}

#[test]
fn generated_runs_on_disk_beside_a_neighbour_keep_to_section_5_and_lose_no_change() {
    for seed in 100..104 {
        a_generated_run(Shape::Gap, true, seed, true);
    }
}

// ---------------------------------------------------------------------------------------
// The harness itself: logs made by hand that break a rule of section 5 are found out,
// so that a test which passes has shown something.
// ---------------------------------------------------------------------------------------

/// Reads `log` message by message as a link does that has said `said`.
fn read(said: &[(u64, Said, ChunkPos)], log: Vec<WorkerToEdge>) {
    let mut book = Book::default();
    for (number, said, chunk) in said {
        book.note(*number, *said, &[*chunk]);
    }
    for read in 1..=log.len() {
        check(&log[..read], &mut book);
    }
}

fn elsewhere(chunk: ChunkPos, ask: u64) -> WorkerToEdge {
    WorkerToEdge::Elsewhere {
        chunk,
        ask,
        region: NEIGHBOUR,
    }
}

fn not_mine(chunk: ChunkPos, ask: u64) -> WorkerToEdge {
    WorkerToEdge::NotMine { chunk, ask }
}

fn snapshot_of(position: ChunkPos, ask: u64, tick: u64) -> WorkerToEdge {
    WorkerToEdge::ChunkSnapshot {
        position,
        ask,
        tick,
        chunk: FlatGenerator::classic().generate(position),
        entities: Vec::new(),
    }
}

fn delta(tick: u64, block: BlockPos) -> WorkerToEdge {
    WorkerToEdge::TickDelta {
        tick,
        events: vec![RegionEvent::BlockChanged {
            position: block,
            state: blocks::AIR,
        }],
    }
}

#[test]
fn the_harness_takes_a_log_that_keeps_to_section_5() {
    read(
        &[
            (1, VIEWER, NEXT),
            (1, VIEWER, HOME),
            (2, GUEST, NEXT),
            (3, VIEWER, OWN),
            (4, Said::Ended, HOME),
            (5, GUEST, HOME),
        ],
        vec![
            elsewhere(NEXT, 1),
            delta(4, NEAR),
            snapshot_of(OWN, 3, 4),
            snapshot_of(HOME, 1, 4),
            not_mine(NEXT, 2),
            WorkerToEdge::Progress {
                applied: 0,
                inputs: Vec::new(),
            },
            delta(5, NEAR),
            snapshot_of(HOME, 5, 6),
        ],
    );
}

#[test]
#[should_panic(expected = "of which the link said")]
fn the_harness_finds_an_elsewhere_for_an_asking_that_was_a_guests() {
    read(&[(1, GUEST, NEXT)], vec![elsewhere(NEXT, 1)]);
}

#[test]
#[should_panic(expected = "of which the link said")]
fn the_harness_finds_a_not_mine_for_an_asking_that_was_a_viewers() {
    read(&[(1, VIEWER, NEXT)], vec![not_mine(NEXT, 1)]);
}

#[test]
#[should_panic(expected = "of which the link said")]
fn the_harness_finds_an_answer_with_a_number_the_link_did_not_ask_with() {
    read(
        &[(1, VIEWER, NEXT), (2, Said::Ended, NEXT)],
        vec![elsewhere(NEXT, 2)],
    );
}

#[test]
#[should_panic(expected = "after an answer with ask")]
fn the_harness_finds_an_asking_that_is_answered_twice() {
    read(
        &[(1, VIEWER, NEXT)],
        vec![elsewhere(NEXT, 1), elsewhere(NEXT, 1)],
    );
}

#[test]
#[should_panic(expected = "after an answer with ask")]
fn the_harness_finds_an_answer_with_a_lower_number_than_the_one_before() {
    read(
        &[(1, VIEWER, NEXT), (2, VIEWER, NEXT)],
        vec![elsewhere(NEXT, 2), elsewhere(NEXT, 1)],
    );
}

#[test]
#[should_panic(expected = "not ended since")]
fn the_harness_finds_a_served_subscription_that_is_answered_again() {
    read(
        &[(1, VIEWER, HOME), (2, GUEST, HOME)],
        vec![snapshot_of(HOME, 1, 3), snapshot_of(HOME, 2, 5)],
    );
}

#[test]
#[should_panic(expected = "no subscription to that waits or is served")]
fn the_harness_finds_an_event_of_a_chunk_that_was_told_elsewhere() {
    read(
        &[(1, VIEWER, NEXT)],
        vec![elsewhere(NEXT, 1), delta(7, BEYOND)],
    );
}

#[test]
#[should_panic(expected = "no subscription to that waits or is served")]
fn the_harness_finds_an_event_of_a_chunk_the_link_never_named() {
    read(&[(1, VIEWER, HOME)], vec![delta(7, BEYOND)]);
}

#[test]
#[should_panic(expected = "not in ascending order of the chunks")]
fn the_harness_finds_the_answers_of_a_tick_out_of_the_order_of_their_chunks() {
    read(
        &[(1, VIEWER, HOME), (1, VIEWER, OWN), (1, VIEWER, NEXT)],
        vec![
            snapshot_of(OWN, 1, 4),
            elsewhere(NEXT, 1),
            snapshot_of(HOME, 1, 4),
        ],
    );
}

#[test]
#[should_panic(expected = "puts behind it in a tick")]
fn the_harness_finds_a_progress_among_the_answers_of_a_tick() {
    read(
        &[(1, VIEWER, HOME), (1, VIEWER, OWN)],
        vec![
            snapshot_of(OWN, 1, 4),
            WorkerToEdge::Progress {
                applied: 0,
                inputs: Vec::new(),
            },
            snapshot_of(HOME, 1, 4),
        ],
    );
}

#[test]
#[should_panic(expected = "after something of tick")]
fn the_harness_finds_a_delta_behind_a_snapshot_of_its_tick() {
    read(
        &[(1, VIEWER, HOME)],
        vec![snapshot_of(HOME, 1, 4), delta(4, NEAR)],
    );
}
