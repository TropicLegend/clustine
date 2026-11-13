use std::sync::Mutex;
use std::time::Duration;

use clustine_region::RegionId;
use clustine_rpc::link::{self, EdgeEnd, LinkError};
use clustine_rpc::{RegionHello, RegionList};
use clustine_sim::api::{
    EntityKind, EntityState, Inventory, PlayerInput, PlayerJoin, Pose, RegionEvent,
};
use clustine_sim::{PlayerTransfer, RemoteAction};
use clustine_world::{ChunkArea, Dimension, EntityIds, Vec3};
use clustine_worldgen::FlatGenerator;
use clustine_worldstore::{Pins, Store, StoreHandle};
use tokio::time::timeout;
use uuid::Uuid;

use super::*;

/// The two kinds of link: a direct one, and one that serialises every message the
/// way a link between two processes does.
const KINDS: [fn(usize) -> (TestEdge, WorkerEnd); 2] = [in_process, framed];

/// A welcome as [`TestEdge`] hands it on: without its numbers, which most tests
/// are not about. [`TestEdge::welcomed`] has it as it was said.
const UNKNOWN: WorkerToEdge = WorkerToEdge::Welcome(Welcome::Unknown {
    since: 0,
    entries: 0,
    presences: 0,
    applied: 0,
});
const RESUMED: WorkerToEdge = WorkerToEdge::Welcome(Welcome::Resumed {
    entries: 0,
    presences: 0,
    applied: 0,
});

/// An edge's end of a link that numbers what it sends, as an edge does.
struct TestEdge {
    end: EdgeEnd,
    /// Which edge this is, and which start of it.
    edge: EdgeId,
    start: u64,
    /// Since when the region knows this edge, as its last welcome said; 0 for an
    /// edge that has read none. Said in its hellos.
    since: u64,
    /// The number of the last numbered message sent.
    sent: AtomicU64,
    /// The number of the last subscription message sent on this link, which are
    /// counted apart and per link.
    asked: AtomicU64,
    /// What the worker said about resuming and progress, which `recv` and `try_recv`
    /// set aside: most tests are about what else a region says.
    aside: Vec<WorkerToEdge>,
    /// The welcome read on this link, as it was said.
    welcomed: Option<Welcome>,
}

impl TestEdge {
    /// An edge of its own that has said hello, with nothing to resume.
    fn new(end: EdgeEnd) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let name = format!("test-edge-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        let edge = Self::silent(end, EdgeId::from_name(&name), 1);
        // Before anything else, as an edge does; there is room for it on a new link.
        edge.try_send(edge.hello(0, &[], &[])).unwrap();
        edge
    }

    /// The start `start` of `edge` on a new link, on which nothing has been said yet.
    fn silent(end: EdgeEnd, edge: EdgeId, start: u64) -> Self {
        Self {
            end,
            edge,
            start,
            since: 0,
            sent: AtomicU64::new(0),
            asked: AtomicU64::new(0),
            aside: Vec::new(),
            welcomed: None,
        }
    }

    /// The same start of the same edge on another link, numbering on from where it
    /// was. It has not said hello there yet. It has read every welcome the region
    /// `runner` runs has made for it, and so knows since when the region knows it.
    fn again(&self, end: EdgeEnd, runner: &RegionRunner) -> Self {
        let mut again = Self::silent(end, self.edge, self.start);
        again.since = runner
            .region()
            .edge(self.edge)
            .map_or(0, |state| state.since);
        again
            .sent
            .store(self.sent.load(Ordering::Relaxed), Ordering::Relaxed);
        again
    }

    /// What this edge says first on a link, with `chunks` as those it asks for as
    /// a viewer and none as a guest.
    fn hello(&self, seen: u64, players: &[PlayerId], chunks: &[ChunkPos]) -> EdgeToWorker {
        EdgeToWorker::Hello {
            edge: self.edge,
            start: self.start,
            since: self.since,
            seen,
            players: players.to_vec(),
            chunks: chunks.to_vec(),
            guests: Vec::new(),
        }
    }

    /// `body` with the number this edge gives it: the next of those that change
    /// the region, or, for a subscription message, the next of the link's askings,
    /// whatever number it was made with.
    fn numbered(&self, body: EdgeToWorker) -> EdgeMessage {
        let number = body
            .is_numbered()
            .then(|| self.sent.fetch_add(1, Ordering::Relaxed) + 1);
        let next = || self.asked.fetch_add(1, Ordering::Relaxed) + 1;
        let body = match body {
            EdgeToWorker::Subscribe { chunks, .. } => {
                let ask = next();
                EdgeToWorker::Subscribe { ask, chunks }
            }
            EdgeToWorker::SubscribeAsGuest { chunks, .. } => {
                let ask = next();
                EdgeToWorker::SubscribeAsGuest { ask, chunks }
            }
            EdgeToWorker::Unsubscribe { chunks, .. } => {
                let ask = next();
                EdgeToWorker::Unsubscribe { ask, chunks }
            }
            other => other,
        };
        EdgeMessage { number, body }
    }

    /// The number of the last subscription message this edge sent on its link.
    fn asked(&self) -> u64 {
        self.asked.load(Ordering::Relaxed)
    }

    async fn send(&self, body: EdgeToWorker) -> Result<(), LinkError> {
        self.end.send(self.numbered(body)).await
    }

    fn try_send(&self, body: EdgeToWorker) -> Result<(), LinkError> {
        self.end.try_send(self.numbered(body))
    }

    /// Sends a message as it is, numbered or not.
    async fn send_as_is(&self, message: EdgeMessage) -> Result<(), LinkError> {
        self.end.send(message).await
    }

    /// Sends `body` under `number`, whatever this edge has sent before.
    async fn send_as(&self, number: u64, body: EdgeToWorker) {
        let number = Some(number);
        self.send_as_is(EdgeMessage { number, body }).await.unwrap();
    }

    /// Whether `message` is set aside by `recv` and `try_recv`.
    fn about_resuming(message: &WorkerToEdge) -> bool {
        matches!(
            message,
            WorkerToEdge::Welcome(_)
                | WorkerToEdge::Presence { .. }
                | WorkerToEdge::Progress { .. }
                | WorkerToEdge::Clock { .. }
        )
    }

    /// Takes note of a welcome as it was said, and hands every message on with a
    /// welcome's numbers taken out.
    fn plain(&mut self, message: WorkerToEdge) -> WorkerToEdge {
        match message {
            WorkerToEdge::Welcome(welcome) => {
                self.welcomed = Some(welcome);
                match welcome {
                    Welcome::Resumed { .. } => RESUMED,
                    Welcome::Unknown { .. } => UNKNOWN,
                    Welcome::Superseded => WorkerToEdge::Welcome(Welcome::Superseded),
                }
            }
            other => other,
        }
    }

    async fn recv(&mut self) -> Option<WorkerToEdge> {
        loop {
            let message = self.end.recv().await?;
            let message = self.plain(message);
            if !Self::about_resuming(&message) {
                return Some(message);
            }
            self.aside.push(message);
        }
    }

    fn try_recv(&mut self) -> Result<Option<WorkerToEdge>, LinkError> {
        loop {
            match self.end.try_recv()?.map(|message| self.plain(message)) {
                Some(message) if Self::about_resuming(&message) => self.aside.push(message),
                other => return Ok(other),
            }
        }
    }

    /// Everything the edge has been sent and has not looked at yet, in the order it
    /// came, with nothing set aside but the clock, which keeps time of its own.
    fn everything(&mut self) -> Vec<WorkerToEdge> {
        let mut messages = Vec::new();
        while let Ok(Some(message)) = self.end.try_recv() {
            let message = self.plain(message);
            if matches!(message, WorkerToEdge::Clock { .. }) {
                self.aside.push(message);
                continue;
            }
            messages.push(message);
        }
        messages
    }
}

/// A direct link.
fn in_process(capacity: usize) -> (TestEdge, WorkerEnd) {
    let (edge, worker) = link::in_process(capacity);
    (TestEdge::new(edge), worker)
}

/// A link that serialises every message, as one between two processes does.
fn framed(capacity: usize) -> (TestEdge, WorkerEnd) {
    let (edge, worker) = link::framed(capacity);
    (TestEdge::new(edge), worker)
}

/// A store's handle whose answers to commits can be held back, and which can be
/// lost on demand. It notes what the runner asks for.
struct Gate {
    inner: StoreHandle,
    control: Arc<GateControl>,
}

#[derive(Default)]
struct GateControl {
    holding: AtomicBool,
    /// Whether the answers to flushes are held back, which is apart from those to
    /// commits: a release waits for the one while the other goes on.
    holding_flushes: AtomicBool,
    /// Whether the answers to claims are held back, and whether those to loads are.
    holding_claims: AtomicBool,
    holding_loads: AtomicBool,
    /// Whether the answer to a merge or a split is held back.
    holding_reshapes: AtomicBool,
    /// Whether what the store says of stays is held back: that one may enter, and
    /// that some are dead.
    holding_stays: AtomicBool,
    /// The commits asked for that said something of a stay, in that order: the
    /// tick of each and its notes.
    noted: Mutex<Vec<(u64, Vec<clustine_sim::StayNote>)>>,
    /// Everything the runner asked for, in that order.
    asked: Mutex<Vec<Asked>>,
    lost: AtomicBool,
    /// The answers held back, in the order the store gave them.
    kept: Mutex<VecDeque<StoreReply>>,
    /// The ticks of the commits asked for, in that order.
    commits: Mutex<Vec<u64>>,
    /// How many flushes were asked for.
    flushes: AtomicU64,
    /// The commits asked for that carried the ticks of chunks, in that order.
    ticked: Mutex<Vec<TickedCommit>>,
    /// Every commit asked for, whole, in that order.
    committed: Mutex<Vec<StoreRequest>>,
    /// The saves asked for, in that order: the chunk, the tick and what was saved.
    saved: Mutex<Vec<(ChunkPos, u64, Chunk)>>,
    /// The parts each save asked for named, in that order.
    saved_parts: Mutex<Vec<(ChunkPos, Parts)>>,
}

/// A commit that carried the ticks of chunks, as [`GateControl`] notes it: its
/// tick, how many block changes it had, and the ticks.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TickedCommit {
    tick: u64,
    changes: usize,
    ticks: Vec<(ChunkPos, Vec<clustine_world::ScheduledTick>)>,
}

impl GateControl {
    fn hold(&self) {
        self.holding.store(true, Ordering::SeqCst);
    }

    fn release(&self) {
        self.holding.store(false, Ordering::SeqCst);
    }

    fn hold_flushes(&self) {
        self.holding_flushes.store(true, Ordering::SeqCst);
    }

    fn release_flushes(&self) {
        self.holding_flushes.store(false, Ordering::SeqCst);
    }

    fn hold_claims(&self) {
        self.holding_claims.store(true, Ordering::SeqCst);
    }

    fn release_claims(&self) {
        self.holding_claims.store(false, Ordering::SeqCst);
    }

    fn hold_loads(&self) {
        self.holding_loads.store(true, Ordering::SeqCst);
    }

    fn release_loads(&self) {
        self.holding_loads.store(false, Ordering::SeqCst);
    }

    fn hold_reshapes(&self) {
        self.holding_reshapes.store(true, Ordering::SeqCst);
    }

    fn hold_stays(&self) {
        self.holding_stays.store(true, Ordering::SeqCst);
    }

    fn release_stays(&self) {
        self.holding_stays.store(false, Ordering::SeqCst);
    }

    /// The commits asked for so far that said something of a stay.
    /// The notes the store was given, by commit. A place's time since rest is left
    /// out: it counts the ticks a test ran until something held, which these tests
    /// do not judge, and the region's tests check it.
    fn noted(&self) -> Vec<(u64, Vec<clustine_sim::StayNote>)> {
        let mut noted = self.noted.lock().unwrap().clone();
        for (_, notes) in &mut noted {
            for note in notes {
                if let clustine_sim::StayNote::Has { place, .. } = note {
                    place.time_since_rest = 0;
                }
            }
        }
        noted
    }

    /// Loses the handle, as the store does when it gives the region to another
    /// owner or cannot be reached.
    fn lose(&self) {
        self.lost.store(true, Ordering::SeqCst);
    }

    /// Whether `reply` is to be held back as things are.
    fn holds(&self, reply: &StoreReply) -> bool {
        match reply {
            StoreReply::Committed { .. } => self.holding.load(Ordering::SeqCst),
            StoreReply::Flushed => self.holding_flushes.load(Ordering::SeqCst),
            StoreReply::Claimed { .. } => self.holding_claims.load(Ordering::SeqCst),
            StoreReply::Loaded { .. } => self.holding_loads.load(Ordering::SeqCst),
            StoreReply::Absorbed { .. }
            | StoreReply::Split { .. }
            | StoreReply::Declined { .. } => self.holding_reshapes.load(Ordering::SeqCst),
            StoreReply::Enter { .. } | StoreReply::Dead { .. } => {
                self.holding_stays.load(Ordering::SeqCst)
            }
            _ => false,
        }
    }

    /// How many of the answers held back `counts`.
    fn kept_of(&self, counts: impl Fn(&StoreReply) -> bool) -> usize {
        let kept = self.kept.lock().unwrap();
        kept.iter().filter(|reply| counts(reply)).count()
    }

    /// How many answers to claims are held back.
    fn kept_claims(&self) -> usize {
        self.kept_of(|reply| matches!(reply, StoreReply::Claimed { .. }))
    }

    /// How many answers to loads are held back.
    fn kept_loads(&self) -> usize {
        self.kept_of(|reply| matches!(reply, StoreReply::Loaded { .. }))
    }

    /// How many answers to a merge or a split are held back.
    fn kept_reshapes(&self) -> usize {
        self.kept_of(|reply| {
            matches!(
                reply,
                StoreReply::Absorbed { .. }
                    | StoreReply::Split { .. }
                    | StoreReply::Declined { .. }
            )
        })
    }

    /// What the runner has asked for since this was last called, in that order.
    fn asked(&self) -> Vec<Asked> {
        mem::take(&mut self.asked.lock().unwrap())
    }

    /// How many answers to commits are held back.
    fn kept(&self) -> usize {
        let kept = self.kept.lock().unwrap();
        let commits = kept
            .iter()
            .filter(|reply| matches!(reply, StoreReply::Committed { .. }));
        commits.count()
    }

    /// How many answers to flushes are held back.
    fn kept_flushes(&self) -> usize {
        let kept = self.kept.lock().unwrap();
        let flushes = kept.iter().filter(|reply| **reply == StoreReply::Flushed);
        flushes.count()
    }

    fn commits(&self) -> Vec<u64> {
        self.commits.lock().unwrap().clone()
    }

    fn flushes(&self) -> u64 {
        self.flushes.load(Ordering::SeqCst)
    }
}

/// Something a runner asked the store for, without what it carried.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Asked {
    Load(ChunkPos),
    Save(ChunkPos),
    Commit(u64),
    Return(Vec<ChunkPos>),
    Claim(Vec<ChunkPos>),
    Checkpoint(u64),
    Flush,
    /// A merge, with its tick.
    Absorb(u64),
    /// A split, with its tick and the id it names for the new region.
    Split(u64, RegionId),
}

impl Asked {
    /// None for what a runner asks of its borders (copies, use and couplings), which
    /// these tests do not hold to an order; `tests/borders.rs` looks at those.
    fn of(request: &StoreRequest) -> Option<Self> {
        Some(match request {
            StoreRequest::Load { position } => Self::Load(*position),
            StoreRequest::Save { position, .. } => Self::Save(*position),
            StoreRequest::Commit { tick, .. } => Self::Commit(*tick),
            StoreRequest::Return { chunks, .. } => Self::Return(chunks.clone()),
            StoreRequest::Claim { chunks } => Self::Claim(chunks.clone()),
            StoreRequest::Checkpoint { tick, .. } => Self::Checkpoint(*tick),
            StoreRequest::Flush => Self::Flush,
            StoreRequest::AbsorbCommit { tick, .. } => Self::Absorb(*tick),
            StoreRequest::SplitCommit { tick, region, .. } => Self::Split(*tick, *region),
            StoreRequest::Copies { .. }
            | StoreRequest::Beside { .. }
            | StoreRequest::Dropped { .. }
            | StoreRequest::JoinsAt { .. }
            | StoreRequest::EndCouplings
            | StoreRequest::Lend { .. }
            | StoreRequest::LendRefused { .. }
            | StoreRequest::Lending { .. }
            | StoreRequest::Counts(_)
            | StoreRequest::Village(_)
            | StoreRequest::TraderChance { .. }
            | StoreRequest::GatewayExits { .. }
            | StoreRequest::Portals { .. } => return None,
        })
    }
}

impl RegionStore for Gate {
    fn request(&self, request: StoreRequest) {
        if let Some(asked) = Asked::of(&request) {
            self.control.asked.lock().unwrap().push(asked);
        }
        if let StoreRequest::Commit { tick, stays, .. } = &request {
            self.control.commits.lock().unwrap().push(*tick);
            self.control.committed.lock().unwrap().push(request.clone());
            if !stays.is_empty() {
                let mut noted = self.control.noted.lock().unwrap();
                noted.push((*tick, stays.clone()));
            }
        }
        if let StoreRequest::Commit {
            tick,
            changes,
            ticks,
            ..
        } = &request
            && !ticks.is_empty()
        {
            self.control.ticked.lock().unwrap().push(TickedCommit {
                tick: *tick,
                changes: changes.len(),
                ticks: ticks.clone(),
            });
        }
        if let StoreRequest::Save {
            position,
            tick,
            chunk,
            parts,
        } = &request
        {
            let saved = (*position, *tick, chunk.clone());
            self.control.saved.lock().unwrap().push(saved);
            let parts = (*position, *parts);
            self.control.saved_parts.lock().unwrap().push(parts);
        }
        if request == StoreRequest::Flush {
            self.control.flushes.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.request(request);
    }

    fn try_reply(&self) -> Option<StoreReply> {
        if RegionStore::is_lost(self) {
            return None;
        }
        let mut kept = self.control.kept.lock().unwrap();
        let free = kept.iter().position(|reply| !self.control.holds(reply));
        if let Some(reply) = free.and_then(|index| kept.remove(index)) {
            return Some(reply);
        }
        loop {
            let reply = self.inner.try_reply()?;
            if !self.control.holds(&reply) {
                return Some(reply);
            }
            kept.push_back(reply);
        }
    }

    fn is_lost(&self) -> bool {
        self.control.lost.load(Ordering::SeqCst) || self.inner.is_lost()
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

/// Where players enter the flat world.
const SPAWN: Vec3 = Vec3::new(0.5, -60.0, 0.5);

const ORIGIN: ChunkPos = ChunkPos::new(0, 0);

/// The eastern one of the two regions of [`Divided::stripes`], which begins where
/// the chunks with x = 1 do.
const EAST: RegionId = RegionId(1);

/// What a region is restored with beside what the store says. It knows of a chunk
/// only what the store tells it, and gives one back that nothing has used for
/// `return_after` ticks.
fn settings(return_after: u64) -> RegionSettings {
    RegionSettings {
        starting_inventory: Inventory::default(),
        return_after,
        lowest_y: -64,
        // What these tests count is a region's claims and loads for viewers' and
        // guests' tickets alone (ADR-0012); rings would add to them.
        rings: false,
    }
}

/// What [`settings`] makes of a region of the flat world, whose spawn [`SPAWN`]
/// is the middle of.
fn config(return_after: u64) -> RegionConfig {
    settings(return_after).with_spawn(BlockPos::new(0, -60, 0), flat_world(), None)
}

/// A flat world's regions get the game's flat sea level and seed 0, a generated
/// world's its seed and the terrain's sea level.
#[test]
fn a_region_is_given_its_world_s_seed_and_sea_level() {
    let flat = settings(0).with_spawn(BlockPos::new(0, -60, 0), flat_world(), None);
    assert_eq!((flat.game_seed, flat.sea_level), (0, -63));
    let seeded = settings(0).with_spawn(BlockPos::new(0, 64, 0), 7, Some(3));
    assert_eq!((seeded.game_seed, seeded.sea_level), (3, 63));
}

/// A flat world's saplings grow into the game's trees and its grass takes bone meal,
/// as a game's superflat world's do: its regions have the features of its seed 0.
#[test]
fn a_flat_world_s_regions_have_the_features_of_its_seed() {
    let flat = settings(0).with_spawn(BlockPos::new(0, -60, 0), flat_world(), None);
    assert!(flat.features.0.is_some());
}

/// The number the store gives the classic flat world of these tests, whose other
/// dimensions are air: its identity names the generators of all three.
fn flat_world() -> u64 {
    let generators = clustine_worldstore::Generators::from(Arc::new(FlatGenerator::classic()));
    clustine_worldstore::world_number(&generators.identity())
}

/// A store of a flat world that is one region and only lasts as long as the store.
fn memory() -> Store {
    Store::memory(Arc::new(FlatGenerator::classic()))
}

/// The flat world's chunks under the identity of the overworld of [`Seeded::SEED`]
/// with its features: a worker learns of the world's features from the seed the
/// store reads in the identity alone, so the test need not make the seed's land.
struct Seeded(clustine_world::generation::GeneratorSettings);

impl Seeded {
    const SEED: i64 = 13579;

    fn new() -> Self {
        use clustine_worldgen::stages::Stages as _;
        let terrain = Arc::new(clustine_worldgen::TerrainGenerator::new(
            Self::SEED,
            clustine_world::Dimension::Overworld,
        ));
        Self(clustine_worldgen::FeatureStages::new(terrain).settings())
    }
}

impl clustine_world::ChunkGenerator for Seeded {
    fn generate(&self, position: ChunkPos) -> Chunk {
        FlatGenerator::classic().generate(position)
    }

    fn settings(&self) -> clustine_world::generation::GeneratorSettings {
        self.0.clone()
    }
}

#[test]
fn the_regions_of_a_world_with_a_seed_share_the_features_of_its_seed() {
    let store = Store::memory(Arc::new(Seeded::new()));
    let (handle, restored) = store.open_region(owner()).unwrap();
    assert_eq!(restored.seed, Some(Seeded::SEED));
    let configured = || {
        settings(0)
            .of_region(
                restored.dimension,
                restored.spawn,
                restored.world,
                restored.seed,
            )
            .features
            .0
            .expect("a world with a seed has features")
    };
    let (first, second) = (configured(), configured());
    assert_eq!(first.seed(), Seeded::SEED);
    // Built once and shared, not read again for every region.
    assert!(Arc::ptr_eq(&first, &second));
    // And a runner is restored with them.
    let runner = RegionRunner::restore(settings(0), handle, restored).unwrap();
    drop(runner);
}

/// Has `store` hand `chunks` to the owner of its one region, opened as [`owner`].
fn handed_out(store: &Store, chunks: &[ChunkPos]) {
    let (handle, _) = store.open_region(owner()).unwrap();
    for &position in chunks {
        handle.request(StoreRequest::Load { position });
    }
    let mut waiting = chunks.len();
    while waiting > 0 {
        match handle.try_reply() {
            Some(StoreReply::Loaded { .. }) => waiting -= 1,
            Some(StoreReply::Generating { .. } | StoreReply::Dead { .. }) => {}
            Some(other) => panic!("expected a chunk, got {other:?}"),
            None => std::thread::yield_now(),
        }
    }
}

/// A store of a flat world that is one region, kept in `directory`.
fn on_disk(directory: &std::path::Path) -> Store {
    Store::local(directory, Arc::new(FlatGenerator::classic())).unwrap()
}

/// The hello of the owner of the one region of [`memory`] and [`on_disk`].
fn owner() -> RegionHello {
    RegionHello {
        region: RegionId(0),
        epoch: 1,
    }
}

/// A runner for the region as `store` has it, without links. It is told which
/// region it runs, as the runner of a home region has to be: the store names the
/// region that held a player's last place when they join again, and a runner that
/// does not know that this is its own would let the player go to it.
fn opened(store: &Store, settings: RegionSettings) -> RegionRunner {
    let (handle, restored) = store.open_region(owner()).unwrap();
    let runner = RegionRunner::restore(settings, handle, restored).unwrap();
    runner.with_id(owner().region)
}

/// A runner as [`opened`] makes it, with a gate before its store.
fn gated(store: &Store, settings: RegionSettings) -> (RegionRunner, Arc<GateControl>) {
    let (runner, gate) = gated_as(store, owner(), settings);
    (runner.with_id(owner().region), gate)
}

/// A runner for the region that `hello` names, with a gate before its store. It is
/// not told which region it runs.
fn gated_as(
    store: &Store,
    hello: RegionHello,
    settings: RegionSettings,
) -> (RegionRunner, Arc<GateControl>) {
    let (inner, restored) = store.open_region(hello).unwrap();
    let rings = settings.rings;
    let config = settings.of_restored(&restored);
    let control = Arc::new(GateControl::default());

    let gate = Gate {
        inner,
        control: Arc::clone(&control),
    };
    let holdings = holdings(&restored);
    let region = Region::restore(config, restored_state(restored).unwrap(), holdings);

    let mut runner = RegionRunner::with_store(region, Box::new(gate));
    runner.with_rings = rings;
    (runner, control)
}

/// A runner for the one region of a flat world that only lasts as long as the
/// runner, with `link` as its first link.
fn runner(link: WorkerEnd) -> RegionRunner {
    let runner = opened(&memory(), settings(0));
    runner.links().attach(link);
    runner
}

/// A runner for the western one of the two regions of [`Divided::stripes`], in a
/// world that only lasts as long as the runner, with `link` as its first link.
fn west(link: WorkerEnd) -> RegionRunner {
    let runner = Divided::stripes().runner(RegionId(0), 1);
    runner.links().attach(link);
    runner
}

/// Has `edge` ask the region of `runner` for [`BESIDE`] as a viewer and waits for
/// the answer, as an edge does whose player can see across the line. From then on
/// the region knows that the chunk is [`EAST`]'s: a player who steps into it is let
/// go in the tick of the step, and what one does to a block of it is passed on with
/// the region named. Without it the region would learn that only when it has asked
/// the store because a player stands there, a tick or two after the step.
fn look_east(runner: &mut RegionRunner, edge: &mut TestEdge) {
    edge.try_send(asking_for(vec![BESIDE])).unwrap();
    let elsewhere = WorkerToEdge::Elsewhere {
        chunk: BESIDE,
        ask: edge.asked(),
        region: EAST,
    };
    assert_eq!(step_for(runner, edge), elsewhere);
}

fn player() -> PlayerId {
    PlayerId(Uuid::from_u128(1))
}

fn other_player() -> PlayerId {
    PlayerId(Uuid::from_u128(2))
}

fn third_player() -> PlayerId {
    PlayerId(Uuid::from_u128(3))
}

/// The attempt that a join of `player` names in these tests.
fn attempt_of(player: PlayerId) -> u64 {
    7000 + player.0.as_u128() as u64
}

fn join(player: PlayerId, name: &str) -> EdgeToWorker {
    EdgeToWorker::PlayerJoin(PlayerJoin {
        appearance: Default::default(),
        player,
        name: name.to_owned(),
        attempt: attempt_of(player),
    })
}

/// A player as another region hands them over, standing in the chunk at the origin.
fn transfer(entity_id: EntityId) -> PlayerTransfer {
    PlayerTransfer {
        digging: Default::default(),
        appearance: Default::default(),
        posture: Default::default(),
        forwards: 0,
        health: 20.0,
        dead: false,
        dimension: Default::default(),
        travel: None,
        seen_credits: false,
        portal: Default::default(),
        unit: None,
        sleeping: None,
        respawn: None,
        last_death: None,
        mode: Default::default(),
        experience: Default::default(),
        attack_ticks: None,
        using: None,
        cooldowns: Vec::new(),
        effects: Vec::new(),
        time_since_rest: 0,
        entity_id,
        name: "Jeb".to_owned(),
        pose: Pose::at(Vec3::new(10.5, -60.0, 0.5)),
        inventory: Inventory::default(),
        selected_slot: 4,
        ender: Default::default(),
        last_input: 7,
        hops: 0,
        flying: false,
        attempt: None,
        riding: None,
        keys: Default::default(),
        kept: Vec::new(),
        raid_omen_position: None,
        crafting: Default::default(),
        vitals: Default::default(),
    }
}

/// Numbers inputs the way an edge does: in the order they are made.
fn next_number() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The entity a player has in most of these tests, which their inputs name: the
/// players join a region that gives out the first block of entity ids, in the
/// order of [`player`], [`other_player`] and [`third_player`]. A test whose player
/// has another entity says so with [`of_entity`].
fn usual_entity(player: PlayerId) -> EntityId {
    EntityId(u64::try_from(player.0.as_u128()).expect("a player of these tests"))
}

/// `state` as a region is restored with it: with the fresh block of keys the store
/// gave at the hello, and that block's first key as the next, whatever the stored
/// state said (`docs/adr/0028-entities.md`, section 1).
fn with_keys(mut state: RegionState, block: EntityIds) -> RegionState {
    state.entity_ids = block;
    state.next_entity_id = block.first;
    state
}

/// `region` as a runner restored with it from a hello that gave it `block` has it:
/// a split's part is made in memory with no keys, and a restore takes the block.
fn with_keys_of_region(region: &Region, block: EntityIds) -> Region {
    let mut region = region.clone();
    region.take_keys(block);
    region
}

/// `input`, which has to be an input, as of the stay with `entity`.
fn of_entity(entity: EntityId, input: EdgeToWorker) -> EdgeToWorker {
    match input {
        EdgeToWorker::Input {
            player,
            number,
            input,
            ..
        } => EdgeToWorker::Input {
            player,
            entity,
            number,
            input,
        },
        other => panic!("{other:?} is no input"),
    }
}

/// A step along the x axis as the input with the given number.
fn walk_as(player: PlayerId, number: u64, x: f64) -> EdgeToWorker {
    EdgeToWorker::Input {
        player,
        entity: usual_entity(player),
        number,
        input: PlayerInput::Move {
            position: Some(Vec3::new(x, -60.0, 0.5)),
            rotation: None,
            on_ground: true,
        },
    }
}

fn walk(player: PlayerId, x: f64) -> EdgeToWorker {
    walk_as(player, next_number(), x)
}

fn dig_by(player: PlayerId, x: i32, sequence: i32) -> EdgeToWorker {
    EdgeToWorker::Input {
        player,
        entity: usual_entity(player),
        number: next_number(),
        input: PlayerInput::Dig {
            position: BlockPos::new(x, -61, 0),
            sequence,
        },
    }
}

fn dig(x: i32) -> EdgeToWorker {
    dig_by(player(), x, 1)
}

/// A viewer's subscription to `chunks`. The [`TestEdge`] that sends it numbers it.
fn asking_for(chunks: Vec<ChunkPos>) -> EdgeToWorker {
    EdgeToWorker::Subscribe { ask: 0, chunks }
}

/// A guest's subscription to `chunks`, numbered as [`asking_for`] is.
fn asking_as_guest_for(chunks: Vec<ChunkPos>) -> EdgeToWorker {
    EdgeToWorker::SubscribeAsGuest { ask: 0, chunks }
}

/// The end of the subscriptions to `chunks`, numbered as [`asking_for`] is.
fn done_with(chunks: Vec<ChunkPos>) -> EdgeToWorker {
    EdgeToWorker::Unsubscribe { ask: 0, chunks }
}

fn square(radius: i32) -> Vec<ChunkPos> {
    (-radius..=radius)
        .flat_map(|x| (-radius..=radius).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// Where `player` stands along the x axis, if they are in the region.
fn x_of(runner: &RegionRunner, player: PlayerId) -> Option<f64> {
    Some(runner.region().player(player)?.1.position.x)
}

/// Waits until the store has confirmed every tick that has run, and publishes them.
/// No tick runs meanwhile.
fn settle(runner: &mut RegionRunner) {
    for _ in 0..20_000 {
        if !runner.take_replies() {
            return;
        }
        runner.publish_committed();
        if runner.pending.is_empty() {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("the store did not confirm what the region committed");
}

/// Runs one tick with what has arrived, and publishes it once the store has it.
fn step(runner: &mut RegionRunner) {
    runner.step();
    settle(runner);
}

/// Steps `runner` until `done` holds, giving the store thread and links that
/// serialise time to deliver.
fn step_until(runner: &mut RegionRunner, mut done: impl FnMut(&RegionRunner) -> bool) {
    // As many as `settle` waits: on a machine busy with builds the store thread and
    // the links took more than two thousand.
    for _ in 0..20_000 {
        step(runner);
        if done(runner) {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("the condition never became true");
}

/// Waits, without running a tick, until the store has said of every stay the region
/// holds as entering that it may enter, or that it is dead: the coming tick places
/// each, or removes it. The store answers on a thread of its own, behind the answer
/// to the commit that named the stay (ADR-0020, section 4, steps 5 and 6).
fn wait_for_the_stays(runner: &mut RegionRunner) {
    for _ in 0..20_000 {
        runner.take_replies();
        runner.publish_committed();
        let entering = runner.region().state().entering;
        let answered = entering.iter().all(|(player, stay)| {
            let entered = runner.inputs.entered.iter();
            let mut dead = runner.inputs.dead.iter();
            // The store also names the stays that a join has replaced, the lower
            // ones of the player, to every region: that is not an answer for the
            // stay that joined.
            entered
                .filter(|answer| answer.player == *player)
                .any(|answer| answer.entity == stay.entity_id)
                || dead.any(|(of, above, _)| of == player && stay.entity_id < *above)
        });
        if answered {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("the store did not answer for a stay that is entering");
}

/// Runs the tick that takes the joins that have arrived, and waits for the store's
/// answers to it. A join places nobody: the region holds the stay as entering and
/// names it to the store, and the tick after this one places the player. What a
/// test sends before that tick is taken in the tick the player enters in.
fn take_joins(runner: &mut RegionRunner) {
    step(runner);
    wait_for_the_stays(runner);
}

/// Runs the two ticks of a login for the joins that have arrived: the one that
/// takes them and the one that places the players, with the store's answer between.
fn step_in(runner: &mut RegionRunner) {
    take_joins(runner);
    step(runner);
}

/// Steps `runner` until `edge` has been sent something, and returns that.
fn step_for(runner: &mut RegionRunner, edge: &mut TestEdge) -> WorkerToEdge {
    for _ in 0..2000 {
        if let Ok(Some(message)) = edge.try_recv() {
            return message;
        }
        step(runner);
        thread::sleep(Duration::from_millis(1));
    }
    panic!("the worker sent nothing");
}

/// What an edge was told, without the ticks that said nothing but where somebody
/// moved to: an edge hears of its own players' steps also where it watches no
/// chunk, which is beside the point of a test about something else.
fn but_for_moves(told: Vec<WorkerToEdge>) -> Vec<WorkerToEdge> {
    let a_move = |event: &RegionEvent| matches!(event, RegionEvent::EntityMoved { .. });
    let only_moves = |message: &WorkerToEdge| matches!(message, WorkerToEdge::TickDelta { events, .. } if events.iter().all(a_move));
    told.into_iter()
        .filter(|message| !only_moves(message))
        .collect()
}

/// Everything `edge` has been sent and has not looked at yet, but for what is said
/// about resuming and progress.
fn received(edge: &mut TestEdge) -> Vec<WorkerToEdge> {
    let mut messages = Vec::new();
    while let Ok(Some(message)) = edge.try_recv() {
        messages.push(message);
    }
    messages
}

async fn next(edge: &mut TestEdge) -> WorkerToEdge {
    timeout(Duration::from_secs(10), edge.recv())
        .await
        .expect("the worker sent nothing")
        .expect("the worker closed the link")
}

/// Whether the worker has closed the link of `edge`, after whatever it sent before.
async fn closed(edge: &mut TestEdge) -> bool {
    let ended = async { while edge.recv().await.is_some() {} };
    timeout(Duration::from_secs(10), ended).await.is_ok()
}

/// The events of the delta that `message` has to be.
fn events(message: WorkerToEdge) -> Vec<RegionEvent> {
    match message {
        WorkerToEdge::TickDelta { events, .. } => events,
        other => panic!("expected a delta, got {other:?}"),
    }
}

/// The chunk that `message` has to be a snapshot of, and the entities in it.
fn snapshot(message: WorkerToEdge) -> (ChunkPos, Vec<EntityState>) {
    match message {
        WorkerToEdge::ChunkSnapshot {
            position, entities, ..
        } => (position, entities),
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

/// The outbox of `edge` as the region has it: the number of the last entry made and
/// the numbers of those not confirmed.
fn outbox(runner: &RegionRunner, edge: &TestEdge) -> (u64, Vec<u64>) {
    let state = runner.region().edge(edge.edge).unwrap();
    (state.sent, state.outbox.keys().copied().collect())
}

/// A stand-in edge joins and subscribes, over a direct and over a serialising link.
#[tokio::test(flavor = "multi_thread")]
async fn a_joining_player_is_spawned_and_sent_the_chunks_around() {
    for (mut edge, worker_end) in [in_process(256), framed(256)] {
        let worker = Worker::spawn(runner(worker_end));

        edge.send(join(player(), "Notch")).await.unwrap();
        edge.send(asking_for(square(1))).await.unwrap();

        let mut spawned = None;
        let mut snapshots = BTreeSet::new();
        let mut entities = Vec::new();
        while spawned.is_none() || snapshots.len() < 9 {
            match next(&mut edge).await {
                WorkerToEdge::ToPlayer { player: to, event } => {
                    assert_eq!(to, player());
                    spawned = Some(event);
                }
                WorkerToEdge::ChunkSnapshot {
                    position,
                    chunk,
                    entities: in_chunk,
                    ..
                } => {
                    assert_eq!(chunk.surface_heights(), [4; 256]);
                    assert!(snapshots.insert(position), "{position:?} sent twice");
                    // Entities come with the chunk they are in.
                    assert!(in_chunk.iter().all(|entity| entity.chunk() == position));
                    entities.extend(in_chunk);
                }
                // Whether the join is also reported as an event depends on whether
                // the subscription arrived within the same tick.
                WorkerToEdge::TickDelta { events, .. } => {
                    assert!(matches!(events[..], [RegionEvent::EntitySpawned(_)]));
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(
            spawned,
            Some(PlayerEvent::Spawned {
                last_death: None,
                mode: Default::default(),
                riding: None,
                attempt: attempt_of(player()),
                entity_id: EntityId(1),
                pose: Pose::at(SPAWN),
                flying: false,
                inventory: Inventory::default(),
                selected_slot: 0,
            })
        );
        assert_eq!(snapshots, square(1).into_iter().collect());
        // The player's own entity is in the snapshot of the chunk it stands in.
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].entity, EntityId(1));

        worker.stop();
    }
}

#[tokio::test]
async fn movement_is_published_for_subscribed_chunks_and_to_the_edge_whose_player_moved() {
    let (mut edge, worker_end) = in_process(256);
    let mut runner = runner(worker_end);

    edge.send(join(player(), "Notch")).await.unwrap();
    edge.send(asking_for(vec![ChunkPos::new(0, 0)]))
        .await
        .unwrap();
    step_until(&mut runner, |runner| {
        let region = runner.region();
        region.player(player()).is_some() && region.loaded_chunk_count() == 1
    });
    step(&mut runner);
    received(&mut edge);

    // Within the subscribed chunk, then out of it: both can be seen from it.
    for x in [5.0, 20.0] {
        edge.send(walk(player(), x)).await.unwrap();
        step(&mut runner);
        let Ok(Some(WorkerToEdge::TickDelta { events, .. })) = edge.try_recv() else {
            panic!("expected a delta for the move to x = {x}");
        };
        assert!(matches!(
            events[..],
            [RegionEvent::EntityMoved { pose, .. }] if pose.position.x == x
        ));
    }

    // From one chunk it has not subscribed to into another: the edge is told all
    // the same, because the player is its own, and it is by where the region says
    // they are that it asks for what they see. Were it not told, it would go on
    // asking for the chunks around x = 20 whatever the player did out there.
    edge.send(walk(player(), 40.0)).await.unwrap();
    step(&mut runner);
    let Ok(Some(WorkerToEdge::TickDelta { events, .. })) = edge.try_recv() else {
        panic!("expected a delta for the edge's own player beyond what it watches");
    };
    assert!(matches!(
        events[..],
        [RegionEvent::EntityMoved { pose, previous_chunk, .. }]
            if pose.position.x == 40.0 && previous_chunk == ChunkPos::new(1, 0)
    ));
}

/// An entity that walks into view from somewhere the edge was not watching is
/// introduced in full, because the edge has never heard of it.
#[tokio::test]
async fn an_entity_entering_the_subscribed_area_is_introduced() {
    // Room for every message of a slow wait below: a runner gives up on an edge whose
    // queue is full (`publish`), and on a machine busy with builds the chunk took long
    // enough to load that the link was closed before the walk (2026-10-30).
    let (mut edge, worker_end) = in_process(65_536);
    let mut runner = runner(worker_end);

    // The edge watches a chunk far from where the player enters the world.
    edge.send(join(player(), "Notch")).await.unwrap();
    edge.send(asking_for(vec![ChunkPos::new(5, 0)]))
        .await
        .unwrap();
    step_until(&mut runner, |runner| {
        let region = runner.region();
        region.player(player()).is_some() && region.loaded_chunk_count() == 1
    });
    step(&mut runner);
    let seen = received(&mut edge);
    assert!(
        seen.iter().all(|message| match message {
            WorkerToEdge::ChunkSnapshot { entities, .. } => entities.is_empty(),
            WorkerToEdge::ToPlayer { .. } => true,
            _ => false,
        }),
        "{seen:?}"
    );

    edge.send(walk(player(), 85.0)).await.unwrap();
    step(&mut runner);
    let Ok(Some(WorkerToEdge::TickDelta { events, .. })) = edge.try_recv() else {
        panic!("expected a delta");
    };
    let [RegionEvent::EntitySpawned(state)] = &events[..] else {
        panic!("expected the entity to be introduced, got {events:?}");
    };
    assert_eq!(state.entity, EntityId(1));
    assert_eq!(state.pose.position.x, 85.0);
    assert!(matches!(&state.kind, EntityKind::Player { name, .. } if name == "Notch"));

    // Leaving the watched chunk again is an ordinary move, and the player leaving
    // the game out there is none of the edge's business.
    edge.send(walk(player(), 120.0)).await.unwrap();
    step(&mut runner);
    assert!(matches!(
        edge.try_recv(),
        Ok(Some(WorkerToEdge::TickDelta { events, .. }))
            if matches!(events[..], [RegionEvent::EntityMoved { .. }])
    ));
    edge.send(EdgeToWorker::PlayerLeave {
        player: player(),
        entity: None,
        attempt: Some(attempt_of(player())),
    })
    .await
    .unwrap();
    step(&mut runner);
    assert_eq!(edge.try_recv(), Ok(None));
}

/// Two edges watch different parts of the region. Neither hears what happens in the
/// other's part, and neither is sent the other's chunk.
#[tokio::test(flavor = "multi_thread")]
async fn each_link_hears_only_of_the_chunks_it_subscribed_to() {
    for connect in KINDS {
        let (mut near, near_end) = connect(256);
        let (mut far, far_end) = connect(256);
        let mut runner = runner(near_end);
        runner.links().attach(far_end);
        let (origin, distant) = (ChunkPos::new(0, 0), ChunkPos::new(5, 0));

        near.send(asking_for(vec![origin])).await.unwrap();
        far.send(asking_for(vec![distant])).await.unwrap();
        assert_eq!(snapshot(step_for(&mut runner, &mut near)).0, origin);
        assert_eq!(snapshot(step_for(&mut runner, &mut far)).0, distant);

        // A player enters the world at the origin, which only one edge watches.
        near.send(join(player(), "Notch")).await.unwrap();
        let seen = events(step_for(&mut runner, &mut near));
        assert!(matches!(seen[..], [RegionEvent::EntitySpawned(_)]));
        assert!(matches!(
            step_for(&mut runner, &mut near),
            WorkerToEdge::ToPlayer { .. }
        ));

        // They walk over to what the other edge watches. That it is told who they
        // are, and before anything else, shows that it had not heard of them.
        near.send(walk(player(), 85.0)).await.unwrap();
        let seen = events(step_for(&mut runner, &mut near));
        assert!(matches!(seen[..], [RegionEvent::EntityMoved { .. }]));
        let seen = events(step_for(&mut runner, &mut far));
        let [RegionEvent::EntitySpawned(state)] = &seen[..] else {
            panic!("expected the entity to be introduced, got {seen:?}");
        };
        assert_eq!(state.pose.position.x, 85.0);

        // A step over there is for the far edge, which watches the chunk, and for
        // the near edge, whose player it is and which watches nothing there: it
        // follows its own players wherever they are. When they come back it is
        // told as of someone it does not know, as it shows nobody out there.
        near.send(walk(player(), 86.0)).await.unwrap();
        let seen = events(step_for(&mut runner, &mut far));
        assert!(matches!(seen[..], [RegionEvent::EntityMoved { .. }]));
        let seen = events(step_for(&mut runner, &mut near));
        assert!(matches!(seen[..], [RegionEvent::EntityMoved { .. }]));
        near.send(walk(player(), 5.0)).await.unwrap();
        let seen = events(step_for(&mut runner, &mut near));
        assert!(matches!(seen[..], [RegionEvent::EntitySpawned(_)]));
        let seen = events(step_for(&mut runner, &mut far));
        assert!(matches!(seen[..], [RegionEvent::EntityMoved { .. }]));

        // A step at the origin is for the near edge alone: the far edge watches
        // nothing there, and the player is not its own.
        near.send(walk(player(), 6.0)).await.unwrap();
        let seen = events(step_for(&mut runner, &mut near));
        assert!(matches!(seen[..], [RegionEvent::EntityMoved { .. }]));
        step(&mut runner);
        assert_eq!(far.try_recv(), Ok(None));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn what_concerns_a_player_goes_to_their_link_only() {
    for connect in KINDS {
        let (mut first, first_end) = connect(256);
        let (mut second, second_end) = connect(256);
        let mut runner = runner(first_end);
        runner.links().attach(second_end);

        // One after the other, however long a link takes over a message: what the
        // players do names the entities they are given, which are given out in the
        // order the joins are taken.
        first.send(join(player(), "Notch")).await.unwrap();
        step_until(&mut runner, |runner| runner.region().player_count() == 1);
        second.send(join(other_player(), "Jeb")).await.unwrap();
        step_until(&mut runner, |runner| runner.region().player_count() == 2);
        // The region claims the chunk its players stand in. A block of a chunk it
        // does not know to hold yet it would pass on, to whoever serves the edge
        // the chunk, in place of acknowledging it.
        step_until(&mut runner, |runner| {
            runner.region().knowledge(ORIGIN) == Knowledge::Held
        });
        first.send(dig_by(player(), 1, 3)).await.unwrap();
        second.send(dig_by(other_player(), 2, 7)).await.unwrap();

        // Neither edge is subscribed to anything, so this is all there is on a link.
        let expected = [(&mut first, player(), 3), (&mut second, other_player(), 7)];
        for (edge, player, sequence) in expected {
            let message = step_for(&mut runner, edge);
            assert!(
                matches!(
                    message,
                    WorkerToEdge::ToPlayer { player: to, event: PlayerEvent::Spawned { .. } }
                        if to == player
                ),
                "{message:?}"
            );
            let event = PlayerEvent::Acknowledged { sequence };
            assert_eq!(
                step_for(&mut runner, edge),
                WorkerToEdge::ToPlayer { player, event }
            );
        }
    }
}

/// Edges connect whenever they like, also long after the region has started.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_attached_while_the_runner_runs_is_served() {
    for connect in KINDS {
        let runner = opened(&memory(), settings(0));
        let (links, status) = (runner.links(), runner.status());
        let worker = Worker::spawn(runner);

        let (mut first, first_end) = connect(256);
        links.attach(first_end);
        first.send(join(player(), "Notch")).await.unwrap();
        assert!(matches!(
            next(&mut first).await,
            WorkerToEdge::ToPlayer { .. }
        ));

        // From another thread, as whatever accepts connections attaches them.
        let (mut second, second_end) = connect(256);
        let attach = {
            let links = links.clone();
            move || links.attach(second_end)
        };
        thread::spawn(attach).join().unwrap();
        second.send(asking_for(vec![ORIGIN])).await.unwrap();
        let (position, entities) = snapshot(next(&mut second).await);
        assert_eq!(position, ORIGIN);
        // The player of the other edge stands there.
        assert_eq!(entities.len(), 1);
        assert!(status.tick.load(Ordering::Relaxed) > 0);
        assert_eq!(status.players.load(Ordering::Relaxed), 1);

        // A link that comes too late is closed rather than left waiting.
        assert_eq!(worker.stop(), Ended::Stopped);
        let (mut late, late_end) = connect(256);
        links.attach(late_end);
        assert!(closed(&mut late).await);
    }
}

/// Joins a player, subscribes to the chunk they stand in and waits until the player
/// has entered the world and the chunk is loaded.
async fn joined(edge: &TestEdge, runner: &mut RegionRunner) {
    edge.send(join(player(), "Notch")).await.unwrap();
    edge.send(asking_for(vec![ChunkPos::new(0, 0)]))
        .await
        .unwrap();
    step_until(runner, |runner| {
        let region = runner.region();
        region.player(player()).is_some() && region.loaded_chunk_count() == 1
    });
}

/// A changed chunk that nobody needs any more is stored, and comes back changed.
/// The chunk west of the home chunk, which stays loaded whoever needs it.
#[tokio::test]
async fn changes_survive_a_chunk_being_unloaded() {
    let (edge, worker_end) = in_process(256);
    let mut runner = runner(worker_end);
    let west = ChunkPos::new(-1, 0);
    joined(&edge, &mut runner).await;
    edge.send(asking_for(vec![west])).await.unwrap();
    step_until(&mut runner, |runner| runner.region().chunk(west).is_some());

    edge.send(dig(-1)).await.unwrap();
    step(&mut runner);
    let changed = runner.region().chunk(west).unwrap().clone();
    assert_eq!(changed.get(15, -61, 0), Some(clustine_data::blocks::AIR));

    edge.send(done_with(vec![west])).await.unwrap();
    step(&mut runner);
    assert!(runner.region().chunk(west).is_none());

    edge.send(asking_for(vec![west])).await.unwrap();
    step_until(&mut runner, |runner| runner.region().chunk(west).is_some());
    assert_eq!(runner.region().chunk(west), Some(&changed));
}

/// A chunk that two edges watch is needed until both have let go of it. Only then is
/// it stored, with everything that has happened to it by then. The chunk west of
/// the home chunk, which stays loaded whoever needs it.
#[tokio::test(flavor = "multi_thread")]
async fn a_chunk_is_kept_until_its_last_subscriber_lets_go_and_is_saved_then() {
    for connect in KINDS {
        let (first, first_end) = connect(256);
        let (mut second, second_end) = connect(256);
        let mut runner = runner(first_end);
        runner.links().attach(second_end);
        let west = ChunkPos::new(-1, 0);
        let subscribe = || asking_for(vec![west]);
        let unsubscribe = || done_with(vec![west]);
        let dug = |runner: &RegionRunner, x: i32| {
            let chunk = runner.region().chunk(west);
            let block = chunk.and_then(|chunk| chunk.get((x & 15) as usize, -61, 0));
            block == Some(clustine_data::blocks::AIR)
        };

        joined(&first, &mut runner).await;
        first.send(subscribe()).await.unwrap();
        step_until(&mut runner, |runner| runner.region().chunk(west).is_some());
        second.send(subscribe()).await.unwrap();
        assert_eq!(snapshot(step_for(&mut runner, &mut second)).0, west);

        first.send(dig(-1)).await.unwrap();
        step_until(&mut runner, |runner| dug(runner, -1));
        first.send(unsubscribe()).await.unwrap();
        step_until(&mut runner, |runner| {
            !runner.links[&LinkId(0)].subscriptions.contains_key(&west)
        });
        assert!(runner.region().chunk(west).is_some());

        // The player is still there and changes the chunk once more.
        first.send(dig(-2)).await.unwrap();
        step_until(&mut runner, |runner| dug(runner, -2));
        second.send(unsubscribe()).await.unwrap();
        step_until(&mut runner, |runner| runner.region().chunk(west).is_none());

        second.send(subscribe()).await.unwrap();
        step_until(&mut runner, |runner| runner.region().chunk(west).is_some());
        assert!(dug(&runner, -1) && dug(&runner, -2));
    }
}

/// Stopping stores what is still loaded and the region's state, so that another
/// runner on the same world carries on with both.
#[tokio::test]
async fn stopping_stores_changed_chunks_and_the_state() {
    let directory = tempfile::tempdir().unwrap();
    let (edge, worker_end) = in_process(256);
    let mut first = opened(&on_disk(directory.path()), settings(0));
    first.links().attach(worker_end);
    joined(&edge, &mut first).await;
    edge.send(dig(1)).await.unwrap();
    edge.send(dig(2)).await.unwrap();
    step(&mut first);
    let changed = first.region().chunk(ORIGIN).unwrap().clone();
    // The player is still there and the chunk still loaded when the runner stops.
    assert_eq!(first.run(&AtomicBool::new(true)), Ended::Stopped);
    let state = first.region().state();
    drop((first, edge));

    let (edge, worker_end) = in_process(256);
    let mut second = opened(&on_disk(directory.path()), settings(0));
    // With a fresh block of keys (`docs/adr/0028-entities.md`, section 1).
    let block = second.region().state().entity_ids;
    assert_ne!(block, state.entity_ids);
    assert_eq!(second.region().state(), with_keys(state, block));
    second.links().attach(worker_end);
    edge.send(asking_for(vec![ORIGIN])).await.unwrap();
    step_until(&mut second, |runner| {
        runner.region().loaded_chunk_count() == 1
    });
    assert_eq!(second.region().chunk(ORIGIN), Some(&changed));
    assert_eq!(changed.get(2, -61, 0), Some(clustine_data::blocks::AIR));
}

/// Changes to a chunk that stays loaded reach the stored world at the next
/// checkpoint, which also empties the log.
#[tokio::test]
async fn checkpoints_save_loaded_chunks_and_empty_the_log() {
    let directory = tempfile::tempdir().unwrap();
    let (edge, worker_end) = in_process(256);
    let mut runner = opened(&on_disk(directory.path()), settings(0)).with_checkpoint_interval(50);
    runner.links().attach(worker_end);
    joined(&edge, &mut runner).await;
    // The log is in segments, which a checkpoint removes once it covers them.
    let log_length = || -> u64 {
        std::fs::read_dir(directory.path().join("log"))
            .unwrap()
            .map(|segment| segment.unwrap().metadata().unwrap().len())
            .sum()
    };
    let stored = directory.path().join("chunks/overworld/0.0/0.0.chunk");

    // Just after a checkpoint, so that the next one is 50 ticks away.
    step_until(&mut runner, |runner| {
        runner.region().tick_number() % 50 == 1
    });
    edge.send(dig(1)).await.unwrap();
    step(&mut runner);
    runner.store.flush();
    assert!(log_length() > 0, "the change was not logged");
    // The chunk as generated may be stored already, since the store writes what
    // it handed out with the next sync of any region (ADR-0021, S2); the dug one
    // is not.
    let before = std::fs::read(&stored).ok();

    step_until(&mut runner, |runner| {
        runner.region().tick_number() % 50 == 0
    });
    runner.store.flush();
    let after = std::fs::read(&stored).ok();
    assert!(after.is_some(), "the checkpoint did not save the chunk");
    assert_ne!(after, before, "the checkpoint did not save the change");
    assert_eq!(log_length(), 0);
}

#[tokio::test]
async fn unsubscribed_chunks_are_unloaded() {
    let (edge, worker_end) = in_process(256);
    let mut runner = runner(worker_end);

    edge.send(asking_for(square(1))).await.unwrap();
    step_until(&mut runner, |runner| {
        runner.region().loaded_chunk_count() == 9
    });

    let mut kept = square(1);
    let dropped = kept.split_off(4);
    edge.send(done_with(dropped.clone())).await.unwrap();
    step(&mut runner);
    // The four kept, and the home chunk among those let go of, which stays loaded
    // whoever needs it.
    assert_eq!(runner.region().loaded_chunk_count(), 5);
    for position in kept {
        assert!(runner.region().chunk(position).is_some());
    }
    for position in dropped {
        let loaded = runner.region().chunk(position).is_some();
        assert_eq!(loaded, position == ORIGIN, "{position:?}");
    }
}

#[tokio::test]
async fn subscribing_twice_sends_one_snapshot() {
    let (mut edge, worker_end) = in_process(256);
    let mut runner = runner(worker_end);
    // Not the home chunk, which stays loaded whoever needs it.
    let position = ChunkPos::new(-1, 0);

    for _ in 0..2 {
        edge.send(asking_for(vec![position])).await.unwrap();
    }
    step_until(&mut runner, |runner| {
        runner.region().chunk(position).is_some()
    });
    step(&mut runner);

    assert!(matches!(
        edge.try_recv(),
        Ok(Some(WorkerToEdge::ChunkSnapshot { .. }))
    ));
    assert_eq!(edge.try_recv(), Ok(None));

    // One unsubscribe ends the subscription, whatever the number of subscribes.
    edge.send(done_with(vec![position])).await.unwrap();
    step(&mut runner);
    assert!(runner.region().chunk(position).is_none());
}

/// A region shows the chunks it holds. Of a chunk another region holds it says who
/// that is, loads nothing, and tells the edge nothing more: what an edge wants to
/// see of it, it has to ask the region there for.
#[tokio::test(flavor = "multi_thread")]
async fn a_subscription_to_a_chunk_of_another_region_is_answered_elsewhere_and_told_no_events() {
    for connect in KINDS {
        let (mut edge, worker_end) = connect(256);
        let mut runner = west(worker_end);
        let (inside, outside, far) = (
            ChunkPos::new(0, 0),
            ChunkPos::new(1, 0),
            ChunkPos::new(7, -3),
        );

        edge.send(asking_for(vec![outside, inside, far]))
            .await
            .unwrap();
        // The region asks the store about all three. Whose the two chunks beyond the
        // line are it says in the tick the answer is in, in the order of the chunks.
        // Its own chunk has to be loaded first.
        let elsewhere = |chunk| WorkerToEdge::Elsewhere {
            chunk,
            ask: 1,
            region: EAST,
        };
        assert_eq!(step_for(&mut runner, &mut edge), elsewhere(outside));
        assert_eq!(step_for(&mut runner, &mut edge), elsewhere(far));
        let answer = step_for(&mut runner, &mut edge);
        assert!(
            matches!(answer, WorkerToEdge::ChunkSnapshot { position, ask: 1, .. } if position == inside),
            "{answer:?}"
        );
        let link = &runner.links[&LinkId(0)];
        assert!(link.waiting.is_empty());
        let conditions: Vec<_> = link.subscriptions.iter().collect();
        let told = |condition| Subscription {
            kind: Ticket::Viewer,
            ask: 1,
            condition,
            taken: None,
        };
        assert_eq!(
            conditions,
            [
                (&inside, &told(Condition::Served)),
                (&outside, &told(Condition::Elsewhere(EAST))),
                (&far, &told(Condition::Elsewhere(EAST))),
            ]
        );
        assert_eq!(runner.region().loaded_chunk_count(), 1);

        // Nor is the edge told what happens out there: the next it hears is about
        // the chunk it does get.
        for (entity, chunk) in [(EntityId(8), outside), (EntityId(9), inside)] {
            edge.send(EdgeToWorker::Discard {
                player: player(),
                entity,
                hops: 1,
                chunk,
            })
            .await
            .unwrap();
        }
        let removed = RegionEvent::EntityRemoved {
            entity: EntityId(9),
            chunk: inside,
        };
        assert_eq!(events(step_for(&mut runner, &mut edge)), [removed]);

        // Asked again, the region says it again, with the number of that asking.
        edge.send(asking_for(vec![outside])).await.unwrap();
        let again = WorkerToEdge::Elsewhere {
            chunk: outside,
            ask: 2,
            region: EAST,
        };
        assert_eq!(step_for(&mut runner, &mut edge), again);

        // Letting go of it changes nothing of what is loaded.
        edge.send(done_with(vec![outside])).await.unwrap();
        edge.send(EdgeToWorker::Discard {
            player: player(),
            entity: EntityId(10),
            hops: 1,
            chunk: inside,
        })
        .await
        .unwrap();
        step_for(&mut runner, &mut edge);
        assert_eq!(runner.region().loaded_chunk_count(), 1);
        let link = &runner.links[&LinkId(0)];
        assert!(!link.subscriptions.contains_key(&outside));
    }
}

/// Numbers are what lets an edge send messages again without any being applied
/// twice. An edge whose numbers on one link have a gap or go back, or are where none
/// belong, has lost track, and so has one that says hello twice. The runner stops
/// listening to it; its players stay, for the edge to come back to.
#[tokio::test]
async fn a_link_whose_messages_are_out_of_order_is_closed() {
    let subscription = asking_for(vec![ORIGIN]);
    let wrong = |edge: &TestEdge| {
        [
            // The join was number 1.
            EdgeMessage {
                number: Some(3),
                body: walk_as(player(), 1, 5.0),
            },
            EdgeMessage {
                number: Some(1),
                body: walk_as(player(), 1, 5.0),
            },
            EdgeMessage {
                number: Some(2),
                body: subscription.clone(),
            },
            EdgeMessage::unnumbered(walk_as(player(), 1, 5.0)),
            EdgeMessage::unnumbered(edge.hello(0, &[], &[])),
        ]
    };
    for case in 0..5 {
        let (mut edge, worker_end) = in_process(256);
        let mut runner = runner(worker_end);
        joined(&edge, &mut runner).await;
        assert_eq!(runner.links.len(), 1);

        let wrong = wrong(&edge).into_iter().nth(case).unwrap();
        edge.send_as_is(wrong).await.unwrap();
        step_until(&mut runner, |runner| runner.links.is_empty());
        step(&mut runner);
        assert_eq!(runner.region().player_count(), 1, "case {case}");
        assert_eq!(x_of(&runner, player()), Some(SPAWN.x), "case {case}");
        // The edge finds its link closed after what it was told before.
        assert!(closed(&mut edge).await);
    }
}

/// The region has to know whose doing a change is, so an edge says hello before
/// anything numbered. One that does not is out of order.
#[tokio::test]
async fn a_link_that_sends_something_numbered_before_saying_hello_is_closed() {
    let (edge, worker_end) = link::in_process(256);
    let mut runner = runner(worker_end);
    // Unnumbered messages concern nobody's doing and are taken.
    let subscribe = EdgeMessage {
        number: None,
        body: EdgeToWorker::Subscribe {
            ask: 1,
            chunks: vec![ChunkPos::new(0, 0)],
        },
    };

    edge.send(subscribe).await.unwrap();
    step(&mut runner);
    assert_eq!(runner.links.len(), 1);

    let join = EdgeMessage {
        number: Some(1),
        body: join(player(), "Notch"),
    };
    edge.send(join).await.unwrap();
    step_until(&mut runner, |runner| runner.links.is_empty());
    // The subscribed chunk's load comes from a thread of its own and may land a step
    // after the link closed; with no ticket left it is let go when it does.
    step_until(&mut runner, |runner| {
        runner.region().loaded_chunk_count() == 0
    });
    assert_eq!(runner.region().player_count(), 0);
}

/// An outbox entry stays in the outbox until the edge confirms it, however often
/// it was sent.
#[tokio::test]
async fn an_outbox_entry_stays_until_the_edge_confirms_it() {
    let (mut edge, worker_end) = in_process(256);
    let mut runner = west(worker_end);
    look_east(&mut runner, &mut edge);
    edge.send(join(player(), "Notch")).await.unwrap();
    take_joins(&mut runner);
    // The step is taken in the tick the player enters in.
    edge.send(walk(player(), 14.5)).await.unwrap();
    step(&mut runner);
    assert_eq!(x_of(&runner, player()), Some(14.5));
    assert_eq!(outbox(&runner, &edge), (0, vec![]));
    received(&mut edge);

    // A block beyond the region, and a step beyond it.
    edge.send(dig(16)).await.unwrap();
    edge.send(walk(player(), 20.0)).await.unwrap();
    step(&mut runner);
    assert_eq!(outbox(&runner, &edge), (2, vec![1, 2]));
    assert!(matches!(
        received(&mut edge)[..],
        [
            WorkerToEdge::Outbox {
                number: 1,
                entry: Durable::Remote { .. },
            },
            WorkerToEdge::Outbox {
                number: 2,
                entry: Durable::Departed { .. },
            },
        ]
    ));
    // Nobody but the edge confirms anything.
    step(&mut runner);
    assert_eq!(outbox(&runner, &edge), (2, vec![1, 2]));

    edge.send(EdgeToWorker::Confirm { number: 1 })
        .await
        .unwrap();
    step(&mut runner);
    assert_eq!(outbox(&runner, &edge), (2, vec![2]));
    edge.send(EdgeToWorker::Confirm { number: 2 })
        .await
        .unwrap();
    step(&mut runner);
    assert_eq!(outbox(&runner, &edge), (2, vec![]));
    // How far the edge's messages are applied is noted too, and nothing was sent
    // again meanwhile.
    assert_eq!(runner.region().edge(edge.edge).unwrap().applied, 4);
    assert_eq!(received(&mut edge), []);
}

/// A link that ends takes its subscriptions with it and nothing else: the players
/// are the edge's, which is expected back.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_that_ends_leaves_its_players_in_the_region() {
    for connect in KINDS {
        let (edge, worker_end) = connect(256);
        let mut runner = runner(worker_end);
        joined(&edge, &mut runner).await;
        assert_eq!(runner.region().player_count(), 1);
        let known = edge.edge;

        // Found out before the tick, so the tick is already one without the link:
        // its chunk is no longer needed. That chunk is the home chunk, which stays
        // loaded whoever needs it, and the only one.
        drop(edge);
        step_until(&mut runner, |runner| runner.links.is_empty());
        assert_eq!(runner.region().player_count(), 1);
        assert_eq!(runner.region().loaded_chunk_count(), 1);
        assert!(runner.region().chunk(ORIGIN).is_some());
        assert_eq!(runner.edges[&known].link, None);

        let ticks = runner.region().tick_number();
        step(&mut runner);
        assert_eq!(runner.region().tick_number(), ticks + 1);
        assert_eq!(runner.region().player_count(), 1);
    }
}

#[tokio::test]
async fn an_edge_that_does_not_keep_up_loses_its_link_and_the_runner_goes_on() {
    // Room for the answer to the hello, which is two messages, and nobody reads.
    let (mut edge, worker_end) = in_process(2);
    let mut runner = runner(worker_end);
    step(&mut runner);
    assert_eq!(runner.links.len(), 1);
    edge.try_send(join(player(), "Notch")).unwrap();
    edge.try_send(asking_for(square(1))).unwrap();

    // The tick that took the join has run when it turns out that the edge cannot
    // be told of it. The player enters the world all the same, in the tick that has
    // the store's answer, and stays; the chunks are let go with the next tick, all
    // but the home chunk, which stays loaded whoever needs it.
    take_joins(&mut runner);
    assert!(runner.links.is_empty());
    step(&mut runner);
    assert_eq!(runner.region().player_count(), 1);
    step_until(&mut runner, |runner| {
        let region = runner.region();
        region.loaded_chunk_count() == 1 && region.chunk(ORIGIN).is_some()
    });
    let ticks = runner.region().tick_number();
    step(&mut runner);
    assert_eq!(runner.region().tick_number(), ticks + 1);

    // The edge finds its link closed after what did fit.
    assert!(matches!(
        edge.everything()[..],
        [UNKNOWN, WorkerToEdge::Progress { .. }]
    ));
    assert!(closed(&mut edge).await);
}

/// An edge that loses its link connects again and finds its players as they were,
/// with what the old link still carried applied.
#[tokio::test]
async fn an_edge_that_is_back_on_a_new_link_has_its_players_still() {
    let (old, old_end) = in_process(256);
    let mut runner = runner(old_end);
    joined(&old, &mut runner).await;
    let Some((entity, _)) = runner.region().player(player()) else {
        panic!("the player has joined");
    };

    // The last the old link carried is something the player did.
    old.send(walk(player(), 3.0)).await.unwrap();
    let (edge_end, new_end) = link::in_process(256);
    let mut new = old.again(edge_end, &runner);
    drop(old);
    runner.links().attach(new_end);
    new.send(new.hello(0, &[player()], &[ORIGIN]))
        .await
        .unwrap();
    step(&mut runner);

    assert_eq!(runner.links.len(), 1);
    assert_eq!(runner.region().player_count(), 1);
    let walked = Pose {
        position: Vec3::new(3.0, -60.0, 0.5),
        on_ground: true,
        ..Pose::at(SPAWN)
    };
    assert_eq!(runner.region().player(player()), Some((entity, walked)));
    // The answer is of the region as it was before the tick that took the step.
    let told = new.everything();
    assert_eq!(told[0], RESUMED);
    let WorkerToEdge::Presence {
        answer:
            Presence::Present {
                entity: present,
                pose,
                ..
            },
        ..
    } = &told[1]
    else {
        panic!("expected the player to be present, got {told:?}");
    };
    assert_eq!((*present, *pose), (entity, Pose::at(SPAWN)));
    // The chunk was let go of and asked for within the same tick, so it stayed.
    assert_eq!(runner.region().loaded_chunk_count(), 1);
}

/// The region can also learn of a player's new connection before it learns that the
/// old one is gone, when the link they had lingers or is served later.
#[tokio::test]
async fn a_player_who_joins_through_another_link_is_taken_over_by_it() {
    let (first, first_end) = in_process(256);
    let (mut second, second_end) = in_process(256);
    let mut runner = runner(first_end);
    runner.links().attach(second_end);
    let entity = |runner: &RegionRunner| Some(runner.region().player(player())?.0);

    first.send(join(player(), "Notch")).await.unwrap();
    step_in(&mut runner);
    assert_eq!(entity(&runner), Some(EntityId(1)));

    // The new entity does not take the step that came through the first link.
    first.send(walk(player(), 3.0)).await.unwrap();
    second.send(join(player(), "Notch")).await.unwrap();
    step_in(&mut runner);
    assert_eq!(
        runner.region().player(player()),
        Some((EntityId(2), Pose::at(SPAWN)))
    );
    assert_eq!(runner.region().player_count(), 1);
    assert!(matches!(
        received(&mut second)[..],
        [WorkerToEdge::ToPlayer {
            event: PlayerEvent::Spawned {
                entity_id: EntityId(2),
                ..
            },
            ..
        }]
    ));

    // What the link they had still says about them no longer counts.
    first.send(walk(player(), 5.0)).await.unwrap();
    first
        .send(EdgeToWorker::PlayerLeave {
            player: player(),
            entity: None,
            attempt: Some(attempt_of(player())),
        })
        .await
        .unwrap();
    step(&mut runner);
    assert_eq!(
        runner.region().player(player()),
        Some((EntityId(2), Pose::at(SPAWN)))
    );

    // The other way round: they join through the link that is served first, and
    // the link they belong to ends within the same tick.
    first.send(join(player(), "Notch")).await.unwrap();
    drop(second);
    step_in(&mut runner);
    assert_eq!(runner.links.len(), 1);
    assert_eq!(entity(&runner), Some(EntityId(3)));
}

#[tokio::test]
async fn only_the_link_a_player_belongs_to_acts_for_them() {
    let (owner, owner_end) = in_process(256);
    let (stranger, stranger_end) = in_process(256);
    let mut runner = runner(owner_end);
    runner.links().attach(stranger_end);
    let leave = || EdgeToWorker::PlayerLeave {
        player: player(),
        entity: None,
        attempt: Some(attempt_of(player())),
    };

    owner.send(join(player(), "Notch")).await.unwrap();
    step_in(&mut runner);
    stranger.send(walk(player(), 5.0)).await.unwrap();
    stranger.send(leave()).await.unwrap();
    step(&mut runner);
    assert_eq!(
        runner.region().player(player()),
        Some((EntityId(1), Pose::at(SPAWN)))
    );

    owner.send(leave()).await.unwrap();
    step(&mut runner);
    assert_eq!(runner.region().player_count(), 0);
}

/// The region handles what players did only after all of them have joined or left.
/// A player who is back within the tick must not begin with what they did before
/// they left, least of all with the numbers of those inputs.
#[tokio::test]
async fn a_player_who_is_back_within_the_tick_starts_afresh() {
    let (edge, worker_end) = in_process(256);
    let mut runner = runner(worker_end);
    edge.send(join(player(), "Notch")).await.unwrap();
    step_in(&mut runner);

    edge.send(walk_as(player(), 900, 3.0)).await.unwrap();
    edge.send(EdgeToWorker::PlayerLeave {
        player: player(),
        entity: None,
        attempt: Some(attempt_of(player())),
    })
    .await
    .unwrap();
    edge.send(join(player(), "Notch")).await.unwrap();
    step_in(&mut runner);
    assert_eq!(
        runner.region().player(player()),
        Some((EntityId(2), Pose::at(SPAWN)))
    );

    // Their new connection numbers what they do from the start again, and what
    // they do is of the stay that began with the second join.
    let anew = of_entity(EntityId(2), walk_as(player(), 1, 4.0));
    edge.send(anew).await.unwrap();
    step(&mut runner);
    let (_, pose) = runner.region().player(player()).unwrap();
    assert_eq!(pose.position.x, 4.0);
}

/// The eastern neighbour lets a player go, who arrives here with the entity they
/// had there, and walks back.
#[tokio::test(flavor = "multi_thread")]
async fn players_arrive_with_their_entity_and_depart_through_their_link() {
    for connect in KINDS {
        let (mut edge, worker_end) = connect(256);
        let (mut bystander, bystander_end) = connect(256);
        let mut runner = west(worker_end);
        runner.links().attach(bystander_end);
        look_east(&mut runner, &mut edge);
        let status = runner.status();
        // An id that only another region can have given out.
        let entity = EntityIds::block(3).unwrap().first;
        let arriving = transfer(entity);

        edge.send(EdgeToWorker::PlayerArrive {
            player: player(),
            transfer: arriving.clone(),
        })
        .await
        .unwrap();
        step_until(&mut runner, |runner| runner.region().player_count() == 1);
        assert_eq!(
            runner.region().player(player()),
            Some((entity, arriving.pose))
        );
        assert_eq!(status.arrivals.load(Ordering::Relaxed), 1);

        // The input the neighbour applied last is sent again and changes nothing;
        // the one after it takes the player back east.
        let walk_to = |number, x| of_entity(entity, walk_as(player(), number, x));
        edge.send(walk_to(7, 12.0)).await.unwrap();
        edge.send(walk_to(8, 20.0)).await.unwrap();
        let mut message = step_for(&mut runner, &mut edge);
        // The ticks the player was here, which the run decides: their time since
        // rest counts them on, and so does the count a dig goes by.
        if let WorkerToEdge::Outbox {
            entry: Durable::Departed { transfer, .. },
            ..
        } = &mut message
        {
            assert!(transfer.time_since_rest > arriving.time_since_rest);
            transfer.time_since_rest = arriving.time_since_rest;
            assert!(transfer.digging.game_ticks > arriving.digging.game_ticks);
            transfer.digging.game_ticks = arriving.digging.game_ticks;
        }
        let leaving = PlayerTransfer {
            pose: Pose {
                position: Vec3::new(20.0, -60.0, 0.5),
                on_ground: true,
                ..arriving.pose
            },
            last_input: 8,
            // Handed on once more (ADR-0020, section 6).
            hops: arriving.hops + 1,
            ..arriving
        };
        let departed = WorkerToEdge::Outbox {
            number: 1,
            entry: Durable::Departed {
                player: player(),
                transfer: leaving,
                to: EAST,
            },
        };
        assert_eq!(message, departed);
        assert_eq!(runner.region().player_count(), 0);
        assert_eq!(status.departures.load(Ordering::Relaxed), 1);
        // The region is done with the player, and nobody else was told anything.
        assert_eq!(bystander.try_recv(), Ok(None));
    }
}

/// What a player does to a block of another region goes to the player's edge to be
/// passed on. What reaches this region that way is answered to the link it came
/// through, after the tick's changes: done, or with what is left for a third region.
#[tokio::test(flavor = "multi_thread")]
async fn actions_on_blocks_of_other_regions_go_through_the_edges() {
    for connect in KINDS {
        let (mut edge, worker_end) = connect(256);
        let (mut other, other_end) = connect(256);
        let mut runner = west(worker_end);
        runner.links().attach(other_end);
        look_east(&mut runner, &mut edge);
        let origin = ChunkPos::new(0, 0);
        let subscribe = || asking_for(vec![origin]);

        // A player of this region, close to where it ends at x = 16, breaks a block
        // beyond that.
        edge.send(join(player(), "Notch")).await.unwrap();
        edge.send(subscribe()).await.unwrap();
        other.send(subscribe()).await.unwrap();
        step_until(&mut runner, |runner| {
            let region = runner.region();
            region.player(player()).is_some() && region.chunk(origin).is_some()
        });
        edge.send(walk(player(), 14.5)).await.unwrap();
        step_until(&mut runner, |runner| {
            runner
                .region()
                .player(player())
                .is_some_and(|(_, pose)| pose.position.x == 14.5)
        });
        // Both edges watch the chunk and so are told of that step. Once they have
        // been, everything sent before it has arrived as well, however long a link
        // takes over it, and what follows is all there is to come.
        for link in [&mut edge, &mut other] {
            loop {
                let WorkerToEdge::TickDelta { events, .. } = step_for(&mut runner, link) else {
                    continue;
                };
                let there = |event: &RegionEvent| matches!(event, RegionEvent::EntityMoved { pose, .. } if pose.position.x == 14.5);
                if events.iter().any(there) {
                    break;
                }
            }
        }
        edge.send(dig_by(player(), 16, 7)).await.unwrap();
        let request = RemoteAction {
            player: player(),
            // The entity of the player who acts.
            entity: EntityId(1),
            sequence: 7,
            step: RemoteStep::Break {
                position: BlockPos::new(16, -61, 0),
            },
        };
        assert_eq!(
            step_for(&mut runner, &mut edge),
            WorkerToEdge::Outbox {
                number: 1,
                entry: Durable::Remote {
                    action: request,
                    to: Some(EAST),
                },
            }
        );
        // Nobody is told that it was handled, and the other edge hears nothing.
        step(&mut runner);
        assert_eq!(received(&mut edge), []);
        assert_eq!(received(&mut other), []);

        // The other edge passes on what a player of another region did to a block
        // of this one. It hears what that changed, then that it is done.
        let block = BlockPos::new(15, -61, 0);
        other
            .send(EdgeToWorker::Remote(RemoteAction {
                player: other_player(),
                entity: EntityId(4040),
                sequence: 3,
                step: RemoteStep::Break { position: block },
            }))
            .await
            .unwrap();
        // The block breaks for every client but the breaker's, then changes.
        let changed = [
            RegionEvent::BlockDestroyed {
                position: block,
                state: clustine_data::blocks::GRASS_BLOCK,
                breaker: Some(other_player()),
            },
            RegionEvent::BlockChanged {
                position: block,
                state: clustine_data::blocks::AIR,
                seen: true,
            },
        ];
        assert_eq!(events(step_for(&mut runner, &mut other)), changed);
        assert_eq!(
            step_for(&mut runner, &mut other),
            WorkerToEdge::Outbox {
                number: 1,
                entry: Durable::RemoteDone {
                    player: other_player(),
                    // That of the action it answers.
                    entity: EntityId(4040),
                    sequence: 3,
                    shown: Vec::new(),
                },
            }
        );
        // The first edge watches the chunk too and is told of the change, but not
        // that anything is done: it did not ask.
        assert_eq!(events(step_for(&mut runner, &mut edge)), changed);
        step(&mut runner);
        assert_eq!(received(&mut edge), []);

        // A block to be placed beyond this region against one of this region's is
        // found to have something to be placed against, and passed on.
        let stone = clustine_data::blocks::STONE;
        let against = BlockPos::new(15, -62, 0);
        let target = BlockPos::new(16, -62, 0);
        let placer = Vec3::new(17.5, -60.0, 0.5);
        other
            .send(EdgeToWorker::Remote(RemoteAction {
                player: other_player(),
                entity: EntityId(4040),
                sequence: 4,
                step: RemoteStep::PlaceAgainst {
                    against,
                    target,
                    block: stone,
                    placer,
                },
            }))
            .await
            .unwrap();
        assert_eq!(
            step_for(&mut runner, &mut other),
            WorkerToEdge::Outbox {
                number: 2,
                entry: Durable::Remote {
                    action: RemoteAction {
                        player: other_player(),
                        entity: EntityId(4040),
                        sequence: 4,
                        step: RemoteStep::Place {
                            target,
                            block: stone,
                            placer,
                        },
                    },
                    to: Some(EAST),
                },
            }
        );
        step(&mut runner);
        assert_eq!(received(&mut edge), []);
        assert_eq!(received(&mut other), []);
    }
}

/// A transfer can still be on its way when the player has long connected anew and
/// joined. The region keeps the player it has: theirs is the later stay, which is
/// the one with the higher entity id.
#[tokio::test]
async fn an_arrival_does_not_take_a_player_from_the_link_they_belong_to() {
    let (first, first_end) = in_process(256);
    let (second, second_end) = in_process(256);
    let mut runner = runner(first_end);
    runner.links().attach(second_end);
    let status = runner.status();
    let leave = || EdgeToWorker::PlayerLeave {
        player: player(),
        entity: None,
        attempt: Some(attempt_of(player())),
    };

    // The stay that is still on its way had the first entity the region gave out,
    // and the one that began since has the second.
    first.send(join(player(), "Notch")).await.unwrap();
    first.send(leave()).await.unwrap();
    first.send(join(player(), "Notch")).await.unwrap();
    step_in(&mut runner);
    second
        .send(EdgeToWorker::PlayerArrive {
            player: player(),
            transfer: transfer(EntityId(1)),
        })
        .await
        .unwrap();
    step(&mut runner);
    assert_eq!(
        runner.region().player(player()),
        Some((EntityId(2), Pose::at(SPAWN)))
    );
    assert_eq!(status.arrivals.load(Ordering::Relaxed), 0);

    // The player is still the first link's to take out of the region.
    second.send(leave()).await.unwrap();
    step(&mut runner);
    assert_eq!(runner.region().player_count(), 1);
    first.send(leave()).await.unwrap();
    step(&mut runner);
    assert_eq!(runner.region().player_count(), 0);
}

/// A player steps out of the region, but their edge has no link and never comes
/// back for them. When the region forgets the edge, everyone is told that the
/// entity is gone: it was seen to leave, and nobody will pass it on.
#[tokio::test]
async fn an_edge_that_stays_away_is_gone_with_its_players_and_departures() {
    let (mut edge, edge_end) = in_process(256);
    let (mut watcher, watcher_end) = in_process(256);
    let mut runner = west(edge_end).with_gone_after(5);
    runner.links().attach(watcher_end);
    look_east(&mut runner, &mut edge);
    edge.send(join(player(), "Notch")).await.unwrap();
    edge.send(join(other_player(), "Jeb")).await.unwrap();
    watcher.send(asking_for(vec![ORIGIN])).await.unwrap();
    step_until(&mut runner, |runner| {
        runner.region().loaded_chunk_count() == 1 && runner.region().player_count() == 2
    });
    step(&mut runner);
    received(&mut watcher);
    let entity = |runner: &RegionRunner, player| runner.region().player(player).unwrap().0;
    let (leaving, staying) = (entity(&runner, player()), entity(&runner, other_player()));
    let known = edge.edge;

    // The last the link carries is the step out of the region.
    edge.send(walk(player(), 20.0)).await.unwrap();
    drop(edge);
    let away_since = runner.region().tick_number();
    step(&mut runner);
    assert!(runner.edges[&known].link.is_none());
    assert_eq!(runner.region().edge(known).unwrap().outbox.len(), 1);
    assert_eq!(runner.region().player_count(), 1);
    // The entity lives on as far as anyone knows.
    let seen = received(&mut watcher);
    let [WorkerToEdge::TickDelta { events, .. }] = &seen[..] else {
        panic!("expected a delta, got {seen:?}");
    };
    assert!(matches!(events[..], [RegionEvent::EntityMoved { .. }]));

    // Five ticks without a link pass, and the sixth forgets the edge.
    while runner.region().tick_number() < away_since + 5 {
        step(&mut runner);
        assert_eq!(runner.region().player_count(), 1);
        assert_eq!(received(&mut watcher), []);
    }
    step(&mut runner);
    assert_eq!(runner.region().tick_number(), away_since + 6);
    assert_eq!(runner.region().player_count(), 0);
    assert!(runner.region().edge(known).is_none());
    assert!(!runner.edges.contains_key(&known));
    let seen = received(&mut watcher);
    let [WorkerToEdge::TickDelta { events, .. }] = &seen[..] else {
        panic!("expected a delta, got {seen:?}");
    };
    let mut removed: Vec<_> = events
        .iter()
        .map(|event| match event {
            RegionEvent::EntityRemoved { entity, chunk } => (*entity, *chunk),
            other => panic!("expected a removal, got {other:?}"),
        })
        .collect();
    removed.sort();
    let mut expected = vec![(leaving, ChunkPos::new(1, 0)), (staying, ORIGIN)];
    expected.sort();
    assert_eq!(removed, expected);

    // An edge with a link is never gone, however little it says.
    let ticks = runner.region().tick_number();
    while runner.region().tick_number() < ticks + 10 {
        step(&mut runner);
    }
    assert!(runner.region().edge(watcher.edge).is_some());
}

#[tokio::test]
async fn a_refused_player_is_told_through_the_outbox_of_their_edge() {
    let (mut edge, worker_end) = in_process(256);
    // A region with a single entity id to give out.
    let entity_ids = EntityIds {
        first: EntityId(1),
        end: EntityId(2),
    };
    let store = clustine_worldstore::spawn(Arc::new(FlatGenerator::classic()));
    let region = Region::new(config(0), entity_ids, Holdings::default());
    let mut runner = RegionRunner::with_store(region, Box::new(store)).without_rings();
    runner.links().attach(worker_end);

    // The refusal is of the tick that took the joins. The player who was given the
    // one id enters the world a tick later, when the store has answered.
    edge.send(join(player(), "Notch")).await.unwrap();
    edge.send(join(other_player(), "Jeb")).await.unwrap();
    take_joins(&mut runner);
    let refused = WorkerToEdge::Outbox {
        number: 1,
        entry: Durable::Refused {
            player: other_player(),
            attempt: attempt_of(other_player()),
        },
    };
    assert_eq!(received(&mut edge), [refused]);
    step(&mut runner);
    let told = received(&mut edge);
    assert!(
        matches!(
            &told[..],
            [WorkerToEdge::ToPlayer { player: entered, event: PlayerEvent::Spawned { .. } }]
                if *entered == player()
        ),
        "{told:?}"
    );
}

#[tokio::test]
async fn the_status_follows_the_region() {
    let (mut edge, worker_end) = in_process(256);
    let mut runner = west(worker_end);
    let status = runner.status();
    let read = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
    let population = || (read(&status.players), read(&status.chunks));
    let traffic = || (read(&status.arrivals), read(&status.departures));
    assert_eq!(
        (read(&status.tick), population(), traffic()),
        (0, (0, 0), (0, 0))
    );
    // Knowing whose a chunk is does not make it the region's.
    look_east(&mut runner, &mut edge);
    assert_eq!((population(), read(&status.held)), ((0, 0), 0));

    // Joining is not arriving.
    joined(&edge, &mut runner).await;
    assert_eq!(read(&status.tick), runner.region().tick_number());
    assert_eq!((population(), traffic()), ((1, 1), (0, 0)));

    // Neither is arriving a second time.
    for _ in 0..2 {
        edge.send(EdgeToWorker::PlayerArrive {
            player: other_player(),
            transfer: transfer(EntityIds::block(3).unwrap().first),
        })
        .await
        .unwrap();
        step(&mut runner);
        assert_eq!((population(), traffic()), ((2, 1), (1, 0)));
    }

    // Both step out of the region; one of them comes back.
    edge.send(walk(player(), 20.0)).await.unwrap();
    let arrived = EntityIds::block(3).unwrap().first;
    let out = of_entity(arrived, walk_as(other_player(), 8, 20.0));
    edge.send(out).await.unwrap();
    step(&mut runner);
    assert_eq!((population(), traffic()), ((0, 1), (1, 2)));
    edge.send(EdgeToWorker::PlayerArrive {
        player: other_player(),
        transfer: transfer(EntityIds::block(3).unwrap().first),
    })
    .await
    .unwrap();
    edge.send(done_with(vec![ChunkPos::new(0, 0)]))
        .await
        .unwrap();
    step(&mut runner);
    // The chunk let go of is the home chunk, which stays loaded whoever needs it.
    assert_eq!((population(), traffic()), ((1, 1), (2, 2)));
    assert_eq!(read(&status.tick), runner.region().tick_number());
    // The store has granted the region the one chunk its players stood in and its
    // edge looked at. It is of the region's stripe, so it stays the region's when
    // nothing is loaded any more.
    assert_eq!(read(&status.held), 1);
    assert_eq!(status.crowds(), [(ChunkPos::new(0, 0), 1)]);
}

#[test]
fn ticks_keep_their_pace() {
    let (_edge, worker_end) = in_process(256);
    let mut runner = runner(worker_end);
    let stop = AtomicBool::new(false);
    thread::scope(|scope| {
        scope.spawn(|| runner.run(&stop));
        thread::sleep(TICK * 10 + TICK / 2);
        stop.store(true, Ordering::Relaxed);
    });
    // Eleven ticks fit into ten and a half tick lengths; a loaded machine may run
    // fewer, but a runner that does not pace itself would run thousands.
    let ticks = runner.region().tick_number();
    assert!((5..=12).contains(&ticks), "{ticks} ticks");
}

/// Waits until the store has answered `count` commits that the gate holds back.
fn wait_for_kept(runner: &mut RegionRunner, gate: &GateControl, count: usize) {
    for _ in 0..20_000 {
        assert!(runner.take_replies());
        if gate.kept() >= count {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("the store did not answer the commits");
}

/// What a tick did is shown to nobody before the store has it, and the region does
/// not run far ahead of what the store has.
#[tokio::test]
async fn nothing_of_a_tick_is_published_before_its_commit_is_confirmed() {
    let (mut edge, worker_end) = in_process(256);
    let (mut runner, gate) = gated(&memory(), settings(0));
    runner.links().attach(worker_end);

    // The player has entered the world before the store's answers are held back:
    // the store's word that they may enter comes behind the answer to the commit
    // of their join, so no tick places them while that answer is out.
    edge.send(join(player(), "Notch")).await.unwrap();
    step_in(&mut runner);
    let before = runner.region().tick_number();
    let told = edge.everything();
    assert_eq!(told[0], UNKNOWN);
    assert!(
        told.iter().any(|message| matches!(
            message,
            WorkerToEdge::ToPlayer {
                event: PlayerEvent::Spawned { .. },
                ..
            }
        )),
        "{told:?}"
    );
    gate.hold();

    // Each tick has something of the player's to apply, so each is to be committed.
    for tick in 1..=20 {
        edge.send(walk(player(), tick as f64)).await.unwrap();
        runner.step();
        let ran = before + tick.min(MAX_TICKS_AHEAD as u64);
        assert_eq!(runner.region().tick_number(), ran);
        assert_eq!(runner.status().tick.load(Ordering::Relaxed), ran);
    }
    // The store has every one of them on disk, and still nobody is told anything.
    wait_for_kept(&mut runner, &gate, MAX_TICKS_AHEAD);
    runner.step();
    assert_eq!(gate.commits(), (1..=before + 8).collect::<Vec<_>>());
    assert_eq!(runner.region().tick_number(), before + 8);
    assert_eq!(edge.everything(), []);
    // Not even the player's position, which is eight steps along.
    assert_eq!(x_of(&runner, player()), Some(8.0));

    // Once the commits are answered, everything arrives in the order it happened,
    // and the region goes on with what has come in meanwhile.
    gate.release();
    step(&mut runner);
    assert_eq!(runner.region().tick_number(), before + 9);
    assert_eq!(x_of(&runner, player()), Some(20.0));
    let told = but_for_moves(edge.everything());
    let applied: Vec<_> = told
        .iter()
        .map(|message| match message {
            WorkerToEdge::Progress { applied, .. } => *applied,
            other => panic!("expected progress, got {other:?}"),
        })
        .collect();
    // The join was the edge's first message. Eight steps in a tick each, then the
    // other twelve in one.
    assert_eq!(applied, [2, 3, 4, 5, 6, 7, 8, 9, 21]);
}

/// A tick in which nothing changed but its number is not worth a commit. It counts
/// as committed once the tick before it is, and not earlier.
#[tokio::test]
async fn a_tick_that_changes_nothing_sends_no_commit_and_is_published() {
    let (mut edge, worker_end) = in_process(256);
    let (mut runner, gate) = gated(&memory(), settings(0));
    runner.links().attach(worker_end);

    // The hello makes the edge known, which is a change. Loading a chunk and showing
    // it are none: chunks are not part of the region's state.
    edge.send(asking_for(vec![ORIGIN])).await.unwrap();
    let (position, _) = snapshot(step_for(&mut runner, &mut edge));
    assert_eq!(position, ORIGIN);
    assert!(runner.region().tick_number() > 1);
    assert_eq!(gate.commits(), [1]);

    // Behind a tick that is not confirmed, ticks that changed nothing wait as well,
    // and count towards how far ahead the region may be. The tick that takes a join
    // names the stay to the store, which is a change; the store's word that the
    // player may enter comes behind the answer to that commit and is held back
    // with it, so no later tick has anything to do.
    gate.hold();
    gate.hold_stays();
    edge.send(join(player(), "Notch")).await.unwrap();
    let before = runner.region().tick_number();
    runner.step();
    edge.send(done_with(vec![ORIGIN])).await.unwrap();
    for _ in 0..20 {
        runner.step();
    }
    wait_for_kept(&mut runner, &gate, 1);
    assert_eq!(runner.region().tick_number(), before + 8);
    assert_eq!(gate.commits(), [1, before + 1]);
    assert_eq!(edge.everything(), []);

    // A join tells its edge nothing but that it was applied.
    gate.release();
    step(&mut runner);
    let told = edge.everything();
    assert!(
        matches!(told[..], [WorkerToEdge::Progress { applied: 1, .. }]),
        "{told:?}"
    );
    assert_eq!(gate.commits(), [1, before + 1]);

    // The tick that places the player changes the region, and is committed.
    gate.release_stays();
    step_until(&mut runner, |runner| {
        runner.region().player(player()).is_some()
    });
    let entered = runner.region().tick_number();
    assert_eq!(gate.commits(), [1, before + 1, entered]);
    let told = edge.everything();
    assert!(
        matches!(
            told[..],
            [WorkerToEdge::ToPlayer {
                event: PlayerEvent::Spawned { .. },
                ..
            }]
        ),
        "{told:?}"
    );
}

/// The owner of a region dies after the store has a tick and before any edge has
/// heard of it. Whoever carries on with the region tells the edge what it missed:
/// what is in the outbox, and where its players are as of what is on disk.
#[tokio::test]
async fn a_runner_restored_after_a_commit_that_was_never_published_tells_it_on_hello() {
    let world = Divided::stripes();
    let (mut edge, worker_end) = in_process(256);
    let (mut runner, gate) = world.gated(RegionId(0), settings(0));
    runner.links().attach(worker_end);
    look_east(&mut runner, &mut edge);
    edge.send(join(player(), "Notch")).await.unwrap();
    edge.send(join(other_player(), "Jeb")).await.unwrap();
    step_in(&mut runner);
    let entity = runner.region().player(other_player()).unwrap().0;
    edge.everything();

    // One steps out of the region and the other aside, in a tick that the store
    // gets and the edge does not.
    gate.hold();
    edge.send(walk(player(), 20.0)).await.unwrap();
    let aside = walk(other_player(), 5.0);
    let EdgeToWorker::Input { number: input, .. } = aside else {
        unreachable!("a step is an input");
    };
    edge.send(aside).await.unwrap();
    runner.step();
    wait_for_kept(&mut runner, &gate, 1);
    assert_eq!(edge.everything(), []);
    let tick = runner.region().tick_number();
    let committed = runner.region().state();
    drop(runner);
    assert!(closed(&mut edge).await);

    let mut restored = world.runner(RegionId(0), 1);
    // The store gives a fresh block of keys at every hello, and the region counts
    // from its first (`docs/adr/0028-entities.md`, section 1).
    let block = restored.region().state().entity_ids;
    assert_ne!(block, committed.entity_ids);
    assert_eq!(restored.region().state(), with_keys(committed, block));
    let (edge_end, worker_end) = link::in_process(256);
    let mut again = edge.again(edge_end, &restored);
    restored.links().attach(worker_end);
    again
        .send(again.hello(0, &[player(), other_player()], &[]))
        .await
        .unwrap();
    step(&mut restored);
    assert_eq!(restored.region().tick_number(), tick + 1);

    let told = again.everything();
    let [
        RESUMED,
        WorkerToEdge::Outbox {
            number: 1,
            entry: Durable::Departed { player: gone, .. },
        },
        WorkerToEdge::Presence {
            player: first,
            answer: Presence::Absent,
        },
        WorkerToEdge::Presence {
            player: second,
            answer,
        },
        WorkerToEdge::Progress { applied: 4, inputs },
    ] = &told[..]
    else {
        panic!("unexpected {told:?}");
    };
    assert_eq!(
        (*gone, *first, *second),
        (player(), player(), other_player())
    );
    assert_eq!(*inputs, []);
    let pose = Pose {
        position: Vec3::new(5.0, -60.0, 0.5),
        on_ground: true,
        ..Pose::at(SPAWN)
    };
    assert_eq!(
        *answer,
        Presence::Present {
            riding: None,
            entity,
            pose,
            inventory: Inventory::default(),
            selected_slot: 0,
            last_input: input,
            handled: None,
            flying: false,
            attempt: None,
            last_death: None,
            mode: Default::default(),
        }
    );
}

/// A region comes back from disk as it was committed: its players and whose they
/// are, what it owes each edge and how far it has got with what each edge sent. It
/// takes the edge's messages up from there.
#[tokio::test]
async fn a_restored_region_carries_on_where_the_store_has_it() {
    let directory = tempfile::tempdir().unwrap();
    let (mut edge, worker_end) = in_process(256);
    let mut first = Divided::stripes_in(directory.path()).runner(RegionId(0), 1);
    first.links().attach(worker_end);
    look_east(&mut first, &mut edge);
    edge.send(join(player(), "Notch")).await.unwrap();
    take_joins(&mut first);
    // The step is taken in the tick the player enters in.
    edge.send(walk(player(), 14.5)).await.unwrap();
    step(&mut first);
    // A block beyond the region, which leaves an entry in the outbox.
    edge.send(dig(16)).await.unwrap();
    step(&mut first);
    let state = first.region().state();
    assert_eq!(state.players[&player()].edge, edge.edge);
    assert_eq!(state.edges[&edge.edge].applied, 3);
    assert_eq!(state.edges[&edge.edge].outbox.len(), 1);
    // It ticks on without anything changing, and is gone without a word: neither a
    // checkpoint nor a goodbye.
    step(&mut first);
    step(&mut first);
    assert_eq!(first.region().tick_number(), state.tick + 2);
    drop(first);

    let mut second = Divided::stripes_in(directory.path()).runner(RegionId(0), 1);
    // Ticks go on from the last the store has, with a fresh block of keys
    // (`docs/adr/0028-entities.md`, section 1).
    let block = second.region().state().entity_ids;
    assert_ne!(block, state.entity_ids);
    assert_eq!(second.region().state(), with_keys(state.clone(), block));
    assert_eq!(second.status().tick.load(Ordering::Relaxed), state.tick);
    assert_eq!(second.status().players.load(Ordering::Relaxed), 1);
    step(&mut second);
    assert_eq!(second.region().tick_number(), state.tick + 1);

    // The edge is back and sends again what it has not heard to be applied. What the
    // region has is dropped, and the next is taken.
    let (edge_end, worker_end) = link::in_process(256);
    let again = edge.again(edge_end, &second);
    second.links().attach(worker_end);
    again.send(again.hello(0, &[], &[])).await.unwrap();
    again.send_as(3, walk(player(), 3.0)).await;
    step(&mut second);
    assert_eq!(second.links.len(), 1);
    assert_eq!(x_of(&second, player()), Some(14.5));
    again.send_as(4, walk(player(), 13.0)).await;
    step(&mut second);
    assert_eq!(x_of(&second, player()), Some(13.0));
    assert_eq!(second.region().edge(edge.edge).unwrap().applied, 4);

    // A message that leaves a gap ends the link.
    again.send_as(6, walk(player(), 12.0)).await;
    step(&mut second);
    assert!(second.links.is_empty());
    assert_eq!(x_of(&second, player()), Some(13.0));

    // And so does a first message beyond the next one the region expects.
    let (edge_end, worker_end) = link::in_process(256);
    let ahead = edge.again(edge_end, &second);
    second.links().attach(worker_end);
    ahead.send(ahead.hello(0, &[], &[])).await.unwrap();
    ahead.send_as(6, walk(player(), 12.0)).await;
    step(&mut second);
    assert!(second.links.is_empty());
    assert_eq!(second.region().edge(edge.edge).unwrap().applied, 4);
}

/// A start of an edge that a later one has replaced is told so at once, and its
/// link is closed.
#[tokio::test]
async fn an_edge_with_a_lower_start_than_the_region_knows_is_superseded() {
    // Whether the region has run a tick with the later start or not.
    for stepped in [true, false] {
        let id = EdgeId::from_name("superseded");
        let (edge_end, worker_end) = link::in_process(256);
        let mut later = TestEdge::silent(edge_end, id, 5);
        let mut runner = runner(worker_end);
        later.send(later.hello(0, &[], &[])).await.unwrap();
        later.send(join(player(), "Notch")).await.unwrap();
        if stepped {
            step_in(&mut runner);
        }

        // The later start's player has entered the world by the end, whichever
        // tick took their join.
        let (edge_end, worker_end) = link::in_process(256);
        let mut earlier = TestEdge::silent(edge_end, id, 3);
        runner.links().attach(worker_end);
        earlier.send(earlier.hello(0, &[], &[])).await.unwrap();
        earlier.send(join(other_player(), "Jeb")).await.unwrap();
        step_in(&mut runner);

        let superseded = WorkerToEdge::Welcome(Welcome::Superseded);
        assert_eq!(earlier.everything(), [superseded]);
        assert!(closed(&mut earlier).await);
        // The later start is untouched by it.
        assert_eq!(runner.links.len(), 1);
        assert_eq!(runner.region().edge(id).unwrap().start, 5);
        assert_eq!(runner.region().player_count(), 1);
        assert!(runner.region().player(player()).is_some());
        assert_eq!(later.everything()[0], UNKNOWN);
    }
}

/// An edge the region knows says hello on a new link. It is told that the region
/// carries on, then what it has not seen of its outbox, then where each player it
/// asks about is, before anything the tick itself has to say. Its old link is closed.
#[tokio::test]
async fn an_edge_that_resumes_is_told_what_it_missed_before_anything_else() {
    let (mut edge, edge_end) = in_process(256);
    let (stranger, stranger_end) = in_process(256);
    let mut runner = west(edge_end);
    runner.links().attach(stranger_end);
    look_east(&mut runner, &mut edge);
    edge.send(join(player(), "Notch")).await.unwrap();
    edge.send(join(third_player(), "Dinnerbone")).await.unwrap();
    stranger.send(join(other_player(), "Jeb")).await.unwrap();
    take_joins(&mut runner);
    // The step is taken in the tick the players enter in.
    edge.send(walk(player(), 14.5)).await.unwrap();
    step(&mut runner);
    // Two entries: a block beyond the region, then a step beyond it.
    edge.send(dig(16)).await.unwrap();
    step(&mut runner);
    edge.send(walk(player(), 20.0)).await.unwrap();
    step(&mut runner);
    assert_eq!(outbox(&runner, &edge), (2, vec![1, 2]));
    let entity = runner.region().player(third_player()).unwrap().0;

    // The edge has seen the first entry. With the hello comes something its
    // remaining player did.
    let (edge_end, worker_end) = link::in_process(256);
    let mut again = edge.again(edge_end, &runner);
    runner.links().attach(worker_end);
    let asked = [player(), third_player(), other_player()];
    again.send(again.hello(1, &asked, &[])).await.unwrap();
    let step_aside = of_entity(entity, walk(third_player(), 2.0));
    let EdgeToWorker::Input { number: input, .. } = step_aside else {
        unreachable!("a step is an input");
    };
    again.send(step_aside).await.unwrap();
    step(&mut runner);

    let present = Presence::Present {
        riding: None,
        entity,
        // As of before the tick that took the step.
        pose: Pose::at(SPAWN),
        inventory: Inventory::default(),
        selected_slot: 0,
        last_input: 0,
        handled: None,
        flying: false,
        // No input of the stay was applied in that state, so it still carries the
        // attempt of its join.
        attempt: Some(attempt_of(third_player())),
        last_death: None,
        mode: Default::default(),
    };
    let told = but_for_moves(again.everything());
    let [
        RESUMED,
        WorkerToEdge::Outbox {
            number: 2,
            entry: Durable::Departed { .. },
        },
        WorkerToEdge::Presence {
            player: first,
            answer: Presence::Absent,
        },
        WorkerToEdge::Presence {
            player: second,
            answer,
        },
        // Of another edge, so not this one's to ask about.
        WorkerToEdge::Presence {
            player: third,
            answer: Presence::Absent,
        },
        WorkerToEdge::Progress { applied: 6, inputs },
    ] = &told[..]
    else {
        panic!("unexpected {told:?}");
    };
    assert_eq!([*first, *second, *third], asked);
    assert_eq!(*answer, present);
    assert_eq!(*inputs, [(third_player(), input)]);
    // The hello confirmed the entry it had seen.
    assert_eq!(outbox(&runner, &again), (2, vec![2]));
    // Nothing went to the link the edge had before, which is closed.
    received(&mut edge);
    assert!(closed(&mut edge).await);
    assert_eq!(runner.links.len(), 2);
}

/// An edge that has started anew is unknown to the region, which removes what the
/// earlier start left behind: its players, and what it had sent that was still
/// waiting for a tick.
#[tokio::test]
async fn an_edge_with_a_higher_start_is_unknown_and_what_its_earlier_start_left_is_dropped() {
    let id = EdgeId::from_name("restarted");
    let (edge_end, worker_end) = link::in_process(256);
    let mut old = TestEdge::silent(edge_end, id, 1);
    let (mut watcher, watcher_end) = in_process(256);
    let mut runner = runner(worker_end);
    runner.links().attach(watcher_end);
    old.send(old.hello(0, &[], &[])).await.unwrap();
    old.send(join(player(), "Notch")).await.unwrap();
    watcher.send(asking_for(vec![ORIGIN])).await.unwrap();
    step_until(&mut runner, |runner| {
        let region = runner.region();
        region.player(player()).is_some() && region.loaded_chunk_count() == 1
    });
    step(&mut runner);
    received(&mut watcher);
    let entity = runner.region().player(player()).unwrap().0;

    // The earlier start sends more, and the later one says hello before a tick has
    // taken any of it.
    old.send(join(other_player(), "Jeb")).await.unwrap();
    old.send(walk(player(), 3.0)).await.unwrap();
    old.send(EdgeToWorker::Discard {
        player: player(),
        entity: EntityId(77),
        hops: 1,
        chunk: ORIGIN,
    })
    .await
    .unwrap();
    let (edge_end, worker_end) = link::in_process(256);
    let mut new = TestEdge::silent(edge_end, id, 2);
    runner.links().attach(worker_end);
    new.send(new.hello(4, &[player()], &[])).await.unwrap();
    step(&mut runner);

    assert_eq!(runner.region().player_count(), 0);
    let state = runner.region().edge(id).unwrap();
    assert_eq!((state.start, state.applied, state.sent), (2, 0, 0));
    assert_eq!(
        new.everything(),
        [
            UNKNOWN,
            WorkerToEdge::Presence {
                player: player(),
                answer: Presence::Absent,
            },
            WorkerToEdge::Progress {
                applied: 0,
                inputs: vec![],
            },
        ]
    );
    // Those watching see the earlier start's player go, and nothing else.
    let removed = RegionEvent::EntityRemoved {
        entity,
        chunk: ORIGIN,
    };
    let seen = received(&mut watcher);
    let [WorkerToEdge::TickDelta { events, .. }] = &seen[..] else {
        panic!("expected a delta, got {seen:?}");
    };
    assert_eq!(*events, [removed]);
    assert!(closed(&mut old).await);

    // The later start numbers from 1.
    new.send(join(player(), "Notch")).await.unwrap();
    step_in(&mut runner);
    assert_eq!(runner.region().player_count(), 1);
    assert_eq!(runner.region().edge(id).unwrap().applied, 1);
}

/// An edge the region has forgotten sends what it kept under numbers that mean
/// nothing to the region any more, before it has read that it is unknown, and
/// numbers from 1 again once it has. The link outlasts that, or the edge would
/// never read the answer.
#[tokio::test]
async fn an_edge_the_region_has_forgotten_is_unknown_and_starts_its_numbers_again() {
    let (mut edge, worker_end) = in_process(256);
    let mut runner = runner(worker_end);
    assert_eq!(edge.sent.load(Ordering::Relaxed), 0);
    // As if it had sent thirty messages to a region that no longer knows of them.
    edge.send_as(31, join(player(), "Notch")).await;
    edge.send_as(32, walk(player(), 3.0)).await;
    step(&mut runner);
    assert_eq!(runner.links.len(), 1);
    assert_eq!(runner.region().player_count(), 0);
    assert_eq!(
        edge.everything(),
        [
            UNKNOWN,
            WorkerToEdge::Progress {
                applied: 0,
                inputs: vec![],
            },
        ]
    );

    edge.send_as(1, join(other_player(), "Jeb")).await;
    step_in(&mut runner);
    assert_eq!(runner.region().player_count(), 1);
    assert_eq!(runner.region().edge(edge.edge).unwrap().applied, 1);
    // From here on its numbers have to follow each other again.
    edge.send_as(3, walk(other_player(), 3.0)).await;
    step(&mut runner);
    assert!(runner.links.is_empty());
}

/// A resume holds the line: what an edge sends behind a hello that names chunks
/// acts on those chunks, so it waits until they are loaded and shown.
#[tokio::test]
async fn what_follows_a_hello_with_chunks_waits_for_their_snapshots() {
    let (edge_end, worker_end) = link::in_process(256);
    let mut edge = TestEdge::silent(edge_end, EdgeId::from_name("holding"), 1);
    // The chunk was handed out before, as what a hello names was shown to its
    // players: a chunk that is still being made holds no link (ADR-0021, section 7).
    let store = memory();
    handed_out(&store, &[ORIGIN]);
    let (handle, restored) = store
        .open_region(RegionHello {
            region: owner().region,
            epoch: owner().epoch + 1,
        })
        .unwrap();
    let mut runner = RegionRunner::restore(settings(0), handle, restored)
        .unwrap()
        .with_id(owner().region);
    runner.links().attach(worker_end);
    // A player arrives and digs in the tick they arrive in. One who joins is placed
    // only when the store has answered, a tick or more after the join is taken.
    let entity = EntityIds::block(3).unwrap().first;
    let arriving = PlayerTransfer {
        pose: Pose::at(SPAWN),
        ..transfer(entity)
    };
    edge.send(edge.hello(0, &[], &[ORIGIN])).await.unwrap();
    edge.send(EdgeToWorker::PlayerArrive {
        player: player(),
        transfer: arriving,
    })
    .await
    .unwrap();
    edge.send(of_entity(entity, dig(1))).await.unwrap();
    // An ordinary subscription holds nothing, but it waits its turn behind the rest.
    edge.send(asking_for(vec![ChunkPos::new(0, 1)]))
        .await
        .unwrap();

    // The chunk has been asked for by the first tick and cannot be there before the
    // second.
    step(&mut runner);
    assert_eq!(runner.region().loaded_chunk_count(), 0);
    assert_eq!(runner.links[&LinkId(0)].held.len(), 3);
    let mut loaded_without_the_player = false;
    step_until(&mut runner, |runner| {
        let loaded = runner.region().loaded_chunk_count() >= 1;
        loaded_without_the_player |= loaded && runner.region().player_count() == 0;
        runner.region().player_count() == 1
    });
    assert!(loaded_without_the_player);
    // The player dug into a chunk that was there.
    let chunk = runner.region().chunk(ORIGIN).unwrap();
    assert_eq!(chunk.get(1, -61, 0), Some(clustine_data::blocks::AIR));
    assert_eq!(runner.region().edge(edge.edge).unwrap().applied, 2);
    assert!(runner.links[&LinkId(0)].held.is_empty());

    // On the wire, the snapshot comes before anything of the tick that took what
    // was held.
    let told = edge.everything();
    let place = |wanted: fn(&WorkerToEdge) -> bool| told.iter().position(wanted).unwrap();
    let shown = place(|message| matches!(message, WorkerToEdge::ChunkSnapshot { .. }));
    let acknowledged = place(|message| matches!(message, WorkerToEdge::ToPlayer { .. }));
    assert!(shown < acknowledged, "{told:?}");
    assert_eq!(told[0], UNKNOWN);
}

/// What a link sent while it was held does not count as received. If the link ends
/// before the hold does, the edge sends it again on its next link, where it is
/// applied once.
#[tokio::test]
async fn what_was_held_on_a_link_that_ended_is_sent_again_and_applied_once() {
    let id = EdgeId::from_name("held and lost");
    let (edge_end, worker_end) = link::in_process(256);
    let first = TestEdge::silent(edge_end, id, 1);
    let mut runner = runner(worker_end);
    first.send(first.hello(0, &[], &[ORIGIN])).await.unwrap();
    first.send(join(player(), "Notch")).await.unwrap();
    // The chunk has been asked for by the first tick and cannot be there before the
    // second, so the join is held when the link ends.
    step(&mut runner);
    assert_eq!(runner.links[&LinkId(0)].held.len(), 1);
    let (edge_end, worker_end) = link::in_process(256);
    let second = first.again(edge_end, &runner);
    drop(first);
    step(&mut runner);
    assert!(runner.links.is_empty());
    assert_eq!(runner.edges[&id].received, 0);

    runner.links().attach(worker_end);
    second.send(second.hello(0, &[], &[ORIGIN])).await.unwrap();
    second.send_as(1, join(player(), "Notch")).await;
    step_until(&mut runner, |runner| runner.region().player_count() == 1);
    second.send_as(2, walk(player(), 3.0)).await;
    step(&mut runner);
    assert_eq!(x_of(&runner, player()), Some(3.0));
    let state = runner.region().state();
    assert_eq!(state.edges[&id].applied, 2);
    // One player has entered the world, once.
    assert_eq!(state.next_entity_id.0, state.entity_ids.first.0 + 1);
}

/// Within what a tick has to say to an edge, the order is fixed: what happened,
/// who entered the world, what the outbox got that is not a departure, what was
/// acknowledged, who was let go, the chunks, and how far the region has got. The
/// numbers of outbox entries ascend.
#[tokio::test]
async fn what_a_tick_tells_an_edge_is_in_a_fixed_order() {
    let (mut edge, edge_end) = in_process(256);
    let (other, other_end) = in_process(256);
    let mut runner = west(edge_end);
    runner.links().attach(other_end);
    look_east(&mut runner, &mut edge);
    let beside = ChunkPos::new(0, 1);
    edge.send(join(player(), "Notch")).await.unwrap();
    edge.send(join(other_player(), "Jeb")).await.unwrap();
    take_joins(&mut runner);
    // The step is taken in the tick the players enter in.
    edge.send(walk(player(), 14.5)).await.unwrap();
    edge.send(asking_for(vec![ORIGIN])).await.unwrap();
    // Another edge has the chunk beside loaded, so that this one is shown it in the
    // very tick it asks for it.
    other.send(asking_for(vec![beside])).await.unwrap();
    step_until(&mut runner, |runner| {
        runner.region().loaded_chunk_count() == 2
    });
    step(&mut runner);
    assert_eq!(x_of(&runner, player()), Some(14.5));
    // A third player has joined, and the store has said that they may enter.
    edge.send(join(third_player(), "Dinnerbone")).await.unwrap();
    take_joins(&mut runner);
    edge.everything();

    // All in one tick: the third player enters the world; the first breaks a block
    // beyond the region; the second breaks one of the region's and steps out of it.
    edge.send(dig_by(player(), 16, 5)).await.unwrap();
    edge.send(dig_by(other_player(), 1, 6)).await.unwrap();
    edge.send(walk(other_player(), 20.0)).await.unwrap();
    edge.send(asking_for(vec![beside])).await.unwrap();
    step(&mut runner);

    let told = edge.everything();
    let [
        WorkerToEdge::TickDelta { .. },
        WorkerToEdge::ToPlayer {
            player: entered,
            event: PlayerEvent::Spawned { .. },
        },
        WorkerToEdge::Outbox {
            number: 1,
            entry: Durable::Remote { .. },
        },
        WorkerToEdge::ToPlayer {
            player: acknowledged,
            event: PlayerEvent::Acknowledged { sequence: 6 },
        },
        WorkerToEdge::Outbox {
            number: 2,
            entry: Durable::Departed { player: gone, .. },
        },
        WorkerToEdge::ChunkSnapshot { position, .. },
        WorkerToEdge::Progress { applied: 7, inputs },
    ] = &told[..]
    else {
        panic!("unexpected {told:?}");
    };
    assert_eq!(
        (*entered, *acknowledged, *gone),
        (third_player(), other_player(), other_player())
    );
    assert_eq!(*position, beside);
    // The player who was let go is no longer the region's to report on.
    assert!(matches!(inputs[..], [(of, _)] if of == player()));
}

/// An edge hears how far the region has got with what it sent, once that is on
/// disk, and nothing while nothing of its own changes.
#[tokio::test]
async fn progress_follows_what_an_edge_sent_and_nothing_else() {
    let (mut edge, edge_end) = in_process(256);
    let (mut other, other_end) = in_process(256);
    let mut runner = runner(edge_end);
    runner.links().attach(other_end);
    let progress = |applied, inputs| WorkerToEdge::Progress { applied, inputs };
    let unknown = UNKNOWN;
    step(&mut runner);
    // An edge that has said hello hears where the region is, even if nowhere.
    assert_eq!(edge.everything(), [unknown.clone(), progress(0, vec![])]);
    assert_eq!(other.everything(), [unknown, progress(0, vec![])]);

    edge.send(join(player(), "Notch")).await.unwrap();
    take_joins(&mut runner);
    let told = edge.everything();
    assert_eq!(told.last(), Some(&progress(1, vec![])));
    // The tick in which the player enters the world took nothing of the edge's.
    step(&mut runner);
    let told = edge.everything();
    assert!(
        matches!(
            told[..],
            [WorkerToEdge::ToPlayer {
                event: PlayerEvent::Spawned { .. },
                ..
            }]
        ),
        "{told:?}"
    );

    let aside = walk(player(), 3.0);
    let EdgeToWorker::Input { number, .. } = aside else {
        unreachable!("a step is an input");
    };
    edge.send(aside).await.unwrap();
    step(&mut runner);
    let told = but_for_moves(edge.everything());
    assert_eq!(told, [progress(2, vec![(player(), number)])]);

    // An input the region has already is dropped by the region, but the message that
    // carried it counts.
    edge.send(walk_as(player(), number, 9.0)).await.unwrap();
    step(&mut runner);
    assert_eq!(x_of(&runner, player()), Some(3.0));
    assert_eq!(edge.everything(), [progress(3, vec![])]);

    step(&mut runner);
    assert_eq!(edge.everything(), []);
    assert_eq!(other.everything(), []);
}

/// A region whose store handle is lost stops for good: it may have been given to
/// another owner, and what it did last may never have reached the disk.
#[tokio::test]
async fn a_region_whose_store_handle_is_lost_stops_and_publishes_nothing_held() {
    let store = memory();
    let (mut edge, worker_end) = in_process(256);
    let (mut runner, gate) = gated(&store, settings(0));
    let (links, status) = (runner.links(), runner.status());
    links.attach(worker_end);
    step(&mut runner);
    edge.everything();

    // A tick the store has, and the edge has not heard of yet.
    gate.hold();
    edge.send(join(player(), "Notch")).await.unwrap();
    runner.step();
    wait_for_kept(&mut runner, &gate, 1);
    assert_eq!(runner.pending.len(), 1);
    assert!(!status.store_lost.load(Ordering::Relaxed));

    // Someone opens the region again, which takes it from this runner.
    let (_handle, restored) = store.open_region(owner()).unwrap();
    assert_eq!(restored.tick(), runner.region().tick_number());
    for _ in 0..20_000 {
        runner.step();
        if runner.store_is_lost() {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(runner.store_is_lost());
    assert!(status.store_lost.load(Ordering::Relaxed));
    assert!(runner.links.is_empty());

    // That the commit was confirmed after all changes nothing.
    gate.release();
    let ticks = runner.region().tick_number();
    runner.step();
    assert_eq!(runner.region().tick_number(), ticks);
    assert_eq!(runner.run(&AtomicBool::new(false)), Ended::StoreLost);
    assert_eq!(edge.everything(), []);
    assert!(closed(&mut edge).await);

    // A link that comes now is closed too.
    let (mut late, late_end) = in_process(256);
    links.attach(late_end);
    runner.step();
    assert!(closed(&mut late).await);
}

/// What the store has of a region is what the region wrote. If it cannot be read,
/// the region is not started from something else instead.
#[test]
fn a_region_whose_stored_state_cannot_be_read_is_not_restored() {
    let restored = |state, deltas| Restored {
        world_spawn: clustine_world::BlockPos::new(0, -60, 0),
        game_mode: clustine_world::GameMode::Creative,
        force_game_mode: false,
        strongholds: Vec::new(),
        homes: Vec::new(),
        dimension: Default::default(),
        faces: Vec::new(),
        spawn: Some(clustine_world::BlockPos::new(0, -60, 0)),
        held: Vec::new(),
        pinned: Vec::new(),
        entity_ids: EntityIds::block(0).unwrap(),
        state,
        deltas,
        issued: EntityId(0),
        world: 0,
        seed: None,
        in_use: Vec::new(),
        offset: None,
        couplings: Vec::new(),
        handshakes: Vec::new(),
        held_tick: None,
        given_up: false,
        lends: clustine_rpc::Lends::default(),
        clock: Default::default(),
    };
    // Of this build by its first two bytes, and nothing postcard can read behind.
    let unreadable = |tick| clustine_rpc::TickState {
        tick,
        state: vec![0, STATE_FORMAT, 0xff, 0xff, 0xff],
    };
    assert!(matches!(
        restored_state(restored(Some(unreadable(4)), vec![])),
        Err(RestoreError::State { tick: 4, .. })
    ));
    assert!(matches!(
        restored_state(restored(None, vec![unreadable(7)])),
        Err(RestoreError::Delta { tick: 7, .. })
    ));
    let state = restored_state(restored(None, vec![])).unwrap();
    assert_eq!(state, RegionState::new(EntityIds::block(0).unwrap()));
}

/// An edge and a region share a numbering only since the welcome that the edge has
/// read. An edge that says hello again without having read it is told the same
/// once more, and one that says what it was told is resumed.
#[tokio::test]
async fn an_edge_that_never_read_its_welcome_is_told_again_since_when_it_is_known() {
    let (mut edge, worker_end) = in_process(256);
    let mut runner = runner(worker_end);
    step(&mut runner);
    edge.everything();
    let Some(Welcome::Unknown {
        since,
        entries: 0,
        presences: 0,
        applied: 0,
    }) = edge.welcomed
    else {
        panic!("{:?}", edge.welcomed);
    };
    assert_eq!(since, runner.region().tick_number());
    assert_eq!(runner.region().edge(edge.edge).unwrap().since, since);

    // Another link of the same start that knows of no welcome.
    let (edge_end, worker_end) = link::in_process(256);
    let mut unread = TestEdge::silent(edge_end, edge.edge, edge.start);
    runner.links().attach(worker_end);
    unread.send(unread.hello(0, &[], &[])).await.unwrap();
    step(&mut runner);
    step(&mut runner);
    unread.everything();
    let again = Welcome::Unknown {
        since,
        entries: 0,
        presences: 0,
        applied: 0,
    };
    assert_eq!(unread.welcomed, Some(again));
    // What it sends from 1 is taken.
    unread.send(join(player(), "Notch")).await.unwrap();
    step_in(&mut runner);
    assert!(runner.region().player(player()).is_some());

    // One that says since when is resumed, and its player is there.
    let (edge_end, worker_end) = link::in_process(256);
    let mut read = unread.again(edge_end, &runner);
    assert_eq!(read.since, since);
    runner.links().attach(worker_end);
    read.send(read.hello(0, &[player()], &[])).await.unwrap();
    step(&mut runner);
    read.everything();
    // The one presence answer is for the player its hello named, and the one
    // message the region has applied is the join.
    let resumed = Welcome::Resumed {
        entries: 0,
        presences: 1,
        applied: 1,
    };
    assert_eq!(read.welcomed, Some(resumed));
    assert!(runner.region().player(player()).is_some());
}

/// An edge that says another `since` than the region has for it, after the region
/// took messages from it, has lost track. Nothing it kept can be trusted to fit,
/// so the region resets it as for a higher start: its players go, and it is known
/// anew from that tick.
#[tokio::test]
async fn an_edge_that_lost_its_since_after_the_region_took_its_messages_is_reset() {
    let (mut edge, worker_end) = in_process(256);
    let mut runner = runner(worker_end);
    edge.send(join(player(), "Notch")).await.unwrap();
    step_in(&mut runner);
    edge.everything();
    let Some(Welcome::Unknown { since, .. }) = edge.welcomed else {
        panic!("{:?}", edge.welcomed);
    };
    let (entity, _) = runner.region().player(player()).unwrap();
    let before = runner.region().edge(edge.edge).unwrap().applied;

    let (edge_end, worker_end) = link::in_process(256);
    let mut lost = TestEdge::silent(edge_end, edge.edge, edge.start);
    runner.links().attach(worker_end);
    lost.send(lost.hello(0, &[player()], &[ORIGIN]))
        .await
        .unwrap();
    step(&mut runner);
    step_until(&mut runner, |runner| {
        runner.links.values().all(|link| link.hold.is_empty())
    });
    let told = lost.everything();
    // The state the reset makes has applied nothing, whatever the one before had.
    assert_eq!(before, 1);
    let Some(Welcome::Unknown {
        since: anew,
        entries: 0,
        presences: 1,
        applied: 0,
    }) = lost.welcomed
    else {
        panic!("{:?}", lost.welcomed);
    };
    assert!(anew > since, "{anew} {since}");
    assert_eq!(runner.region().edge(edge.edge).unwrap().since, anew);
    // The player is gone, and is said to be absent.
    assert!(runner.region().player(player()).is_none());
    assert!(told.contains(&WorkerToEdge::Presence {
        player: player(),
        answer: Presence::Absent,
    }));
    let removed = told.iter().any(|message| {
        matches!(message, WorkerToEdge::TickDelta { events, .. }
                if events.iter().any(|event| matches!(event,
                    RegionEvent::EntityRemoved { entity: gone, .. } if *gone == entity)))
    });
    assert!(removed, "{told:?}");
    // What the edge numbers from 1 is taken.
    lost.send(join(other_player(), "Alex")).await.unwrap();
    step_in(&mut runner);
    assert!(runner.region().player(other_player()).is_some());
    assert_eq!(runner.region().edge(edge.edge).unwrap().applied, 1);
}

/// A state and a delta that have something of everything a state is made of.
fn a_state_and_a_delta() -> (RegionState, StateDelta) {
    let ids = EntityIds::block(0).unwrap();
    let mut region = Region::new(config(0), ids, Holdings::default());
    let edge = EdgeId(7);
    let join = PlayerChange::Join(
        edge,
        PlayerJoin {
            appearance: Default::default(),
            player: player(),
            name: "Steve".to_owned(),
            attempt: 21,
        },
    );
    // The edge looks at two chunks of the neighbour, and the store says whose they
    // are before anyone does anything about them.
    let neighbours = [BESIDE, ChunkPos::new(2, 0)];
    let mut inputs = TickInputs {
        edges: vec![EdgeEvent::Started { edge, start: 3 }],
        tickets_added: neighbours.map(|chunk| (chunk, Ticket::Viewer)).to_vec(),
        ..TickInputs::default()
    };
    // Two join in the first tick, which holds their stays as entering and names
    // them to the store. The second of them stays, so that the state has a player.
    inputs.change(join);
    inputs.change(PlayerChange::Join(
        edge,
        PlayerJoin {
            player: other_player(),
            name: "Alex".to_owned(),
            attempt: 22,
            // Not the defaults, so that the bytes show where they go.
            appearance: clustine_sim::Appearance {
                skin_parts: 0x15,
                left_handed: true,
            },
        },
    ));
    let output = region.tick(&inputs);
    assert_eq!(output.claims, neighbours);
    // The two who joined have the first two ids of the block.
    let (first, second) = (ids.first, EntityId(ids.first.0 + 1));
    let entering = |player, entity| clustine_sim::StayNote::Entering { player, entity };
    assert_eq!(
        output.stays,
        [entering(player(), first), entering(other_player(), second)]
    );
    // The store has no place for either, and says so before the second tick, which
    // puts them into the world at the spawn point.
    let nowhere = |player, entity| Entered {
        unit: None,
        player,
        entity,
        place: None,
        holder: None,
    };
    let mut inputs = TickInputs {
        applied: vec![(edge, 2)],
        foreign: neighbours.map(|chunk| (chunk, EAST)).to_vec(),
        entered: vec![nowhere(player(), first), nowhere(other_player(), second)],
        ..TickInputs::default()
    };
    // One who arrives for a chunk of the neighbour and is sent on to it, which is
    // an outbox entry: a `NotMine`.
    let beyond = Vec3::new(40.5, -60.0, 0.5);
    let arriving = PlayerTransfer {
        digging: Default::default(),
        appearance: Default::default(),
        posture: Default::default(),
        forwards: 0,
        health: 20.0,
        dead: false,
        dimension: Default::default(),
        travel: None,
        seen_credits: false,
        portal: Default::default(),
        unit: None,
        sleeping: None,
        respawn: None,
        last_death: None,
        mode: Default::default(),
        experience: Default::default(),
        attack_ticks: None,
        using: None,
        cooldowns: Vec::new(),
        effects: Vec::new(),
        time_since_rest: 0,
        entity_id: EntityId(77),
        name: "Notch".to_owned(),
        pose: Pose::at(beyond),
        inventory: Inventory::default(),
        selected_slot: 0,
        ender: Default::default(),
        last_input: 5,
        hops: 2,
        flying: true,
        attempt: Some(23),
        riding: None,
        keys: Default::default(),
        kept: Vec::new(),
        raid_omen_position: None,
        crafting: Default::default(),
        vitals: Default::default(),
    };
    inputs.change(PlayerChange::Arrive(edge, third_player(), arriving));
    // A step into a chunk of the neighbour: the player is let go, which is a
    // `Departed`.
    let walk = |x| PlayerInput::Move {
        position: Some(Vec3::new(x, -60.0, 0.5)),
        rotation: None,
        on_ground: true,
    };
    inputs.input(edge, player(), first, 1, walk(beyond.x));
    // The one who stays walks up to the neighbour and breaks a block of it, which
    // is passed on: a `Remote`.
    inputs.input(edge, other_player(), second, 1, walk(14.5));
    let dig = PlayerInput::Dig {
        position: BlockPos::new(16, -61, 0),
        sequence: 9,
    };
    inputs.input(edge, other_player(), second, 2, dig);
    let output = region.tick(&inputs);
    // The three entries whose shapes say where something goes.
    let kinds = output.durable.iter().map(|(_, _, entry)| match entry {
        Durable::NotMine { .. } => "not mine",
        Durable::Remote { .. } => "remote",
        Durable::Departed { .. } => "departed",
        _ => "another",
    });
    let kinds: Vec<_> = kinds.collect();
    assert_eq!(kinds, ["not mine", "remote", "departed"]);

    // The two entries of a merge and of a split, which no tick makes yet
    // (ADR-0014, section 9): they are put behind the others by hand, into the
    // state's outbox and among what the delta adds, so that the delta still
    // turns the state before the tick into this one. So are the entry for a stay
    // that the server ended and a stay that is entering, neither of which this tick
    // made (ADR-0020, sections 4 and 6).
    let (mut state, mut delta) = (region.state(), output.delta);
    let entering = clustine_sim::EnteringState {
        appearance: Default::default(),
        entity_id: EntityId(78),
        name: "Jeb".to_owned(),
        edge,
        attempt: 25,
    };
    state.entering.insert(third_player(), entering.clone());
    delta.entering.push((third_player(), Some(entering)));
    let ended = Durable::Ended {
        player: third_player(),
        entity: EntityId(76),
        attempt: Some(24),
    };
    let absorbed = Durable::Absorbed {
        region: EAST,
        since: 4,
        applied: 6,
        numbers: vec![1, 3],
    };
    let split_off = Durable::SplitOff {
        region: RegionId(9),
        players: vec![(third_player(), EntityId(77), Some(23))],
    };
    let known = state
        .edges
        .get_mut(&edge)
        .expect("the state knows the edge");
    let change = delta.edges.iter_mut().find(|(id, _)| *id == edge);
    let change = change.and_then(|(_, change)| change.as_mut());
    let change = change.expect("the edge changed in the tick");
    for entry in [absorbed, split_off, ended] {
        known.sent += 1;
        known.outbox.insert(known.sent, entry.clone());
        change.added.push((known.sent, entry));
    }
    change.sent = known.sent;
    // And an ask for a lend across a pinned line, in both
    // (`docs/adr/0027-lending-at-a-line.md`, section 2).
    let asked = ChunkPos::new(-1, 3);
    state.lends_asked.insert(asked, 9);
    delta.lends_asked.push((asked, Some(9)));
    (state, delta)
}

/// What is stored of a region is read back by its first two bytes. Postcard writes
/// neither names nor kinds, so bytes of one shape can read as another, and a build
/// that changes the shape of a state has to say so with [`STATE_FORMAT`]. This test
/// has the bytes of a state and of a delta written out. **If it fails, a shape has
/// changed: raise `STATE_FORMAT` and write the new bytes down here.**
///
/// The bytes of format 4 differ from those of format 3 by what ADR-0020 added: a
/// player and a transfer have their hand-overs, whether they fly and the attempt of
/// their join; a remote action and the entry that says it is done name an entity;
/// an entry of a split names each stay's attempt; there is an entry for a stay the
/// server ended; and a state has its entering stays, a delta those that changed.
/// Format 5 has the same bytes as format 4. The number was raised because a state
/// of format 4 has stays the world store has no record of, which a build that
/// keeps the players' places with the store cannot take up (ADR-0020, "Building
/// it"). The state written down is the one it was. The delta is of another tick
/// than before, in the same shape: a tick that places two players whose joins the
/// tick before took gives out no entity id and says that two entering stays are
/// no more, where one tick used to take a join and place the player. Format 7
/// writes entity keys as `u64` (`docs/adr/0028-entities.md`, section 1), which
/// postcard writes as a plain varint where an `i32` was zigzagged. Format 8 keeps a
/// player's whole inventory, 41 slots with their count in front, where it kept the
/// nine of the hotbar (`docs/groundwork/n2-plan.md`, section 2). Format 9 keeps what
/// a player rides and the keys they hold, eight bytes more a player, none of them set
/// here (`docs/groundwork/n5-plan.md`). Format 10 keeps, in what a player rides, the
/// state of the check of their vehicle's moves (`VehicleCheck`, step N5f); nobody
/// rides here, so only the number differs. Format 11 keeps the moves of a vehicle
/// kept for a player who waits for it, a byte more a player, and a transfer carries
/// what its player rides, the keys they hold and those moves, nine bytes more a
/// transfer (step N5h). Format 14 keeps the world's clock, two numbers at the end of
/// a state, and a delta has the clock it took where it jumped, a byte more a delta
/// that did not (step N7a2). Format 15 keeps a player's experience and the tick
/// their attack strength was last reset in (`docs/groundwork/n7-plan.md`, 7.7).
/// Format 16 keeps the bed a player sleeps in and the respawn point a bed set
/// (`docs/groundwork/n7d-plan.md`). Format 17 keeps a stack's components after its
/// count (`docs/groundwork/n9-plan.md`, section 3.1), a byte more a stack; no slot
/// holds anything here, so only the number differs. Format 18 keeps N9's
/// projectiles among the entities a state or a delta carries (an arrow, the spit, a
/// rocket, a cloud, fangs: `docs/groundwork/n9-plan.md`, section 4.1); no entity is
/// carried here, so only the number differs. Format 19 keeps a player's use of an
/// item and their cooldowns (`docs/groundwork/n9-plan.md`, section 5.5), two bytes
/// more a player in a state and a delta, none of them set here. Format 20 keeps a
/// player's effects and the tick their time since rest counts from (section 5.4 of
/// the same plan), two bytes more a player. Format 21 has N9's entity effects among
/// the effects a state keeps (section 8.3 of the same plan); none is kept here, so
/// only the number differs. Format 22 keeps a merchant's payments and result with a
/// player's menu (`docs/groundwork/n10-plan.md`, section 4.1); no menu is open here,
/// so only the number differs. Format 23 keeps where a player's raid omen began
/// (section 9.5 of the same plan): one byte more a player, none set here. Format 24
/// has the entry that says a remote action is done carry the blocks its player is
/// shown after a click, a byte more such an entry; none is in an outbox here, so
/// only the number differs. Format 25 keeps a player's portal clock and cooldown,
/// their travel, whether they saw the credits and whether they watch them, five
/// bytes more a player where none is set, six more a transfer in an outbox (its
/// dimension, travel, credits, portal state and unit), and the End's dragon fight,
/// a byte more a state (`docs/groundwork/p-plan.md`, section 15). Format 26 keeps
/// the stays and the entities a portal sent that wait to be placed, two bytes more a
/// state where there are none, and a delta says them and the dragon fight, three
/// bytes more (section 16.7 of the same plan). Format 27 keeps each player's health
/// and whether they are dead, five bytes more a player (section 19.6). Format 28 has
/// a placing in an outbox say what a respawn's placing owes the client, a byte more
/// such an entry, none of which is in an outbox here; and a transfer counts the
/// times a waiting respawn was sent on, a byte more a transfer (section 19.7).
/// Format 29 keeps where a player last died, a byte more a player where they have
/// not, and a death in an outbox says where it was (section 19.9). Format 30 has a
/// placing in an outbox say whether a gateway made it, a byte more such an entry,
/// none of which is in an outbox here (section 19.8). Format 32 keeps each player's
/// game mode and the one before it (`docs/groundwork/v-plan.md`, section 4.1).
/// Format 33 keeps each player's dig and the origin of their `gameTicks` (section
/// 4.5 of the same plan), thirteen bytes more a player in a state and a delta, none
/// of them set here; a transfer in an outbox carries the dig and the count. Format
/// 34 keeps each player's inventory screen: its grid, result, cursor and drag, seven
/// bytes more a player when all are empty (section 4.7). Format 35 gives an open
/// menu the crafting table's grid, none of them open here. Format 36 keeps each
/// player's body (V4): food, air, fire, freezing, the fall, absorption, the hurt
/// cooldown, who hurt them last, their motion and arrows and the combat tracker.
#[test]
fn the_bytes_of_a_state_and_of_a_delta_are_as_written_down() {
    let (state, delta) = a_state_and_a_delta();
    let hex =
        |bytes: Vec<u8>| -> String { bytes.iter().map(|byte| format!("{byte:02x}")).collect() };
    assert_eq!(STATE_FORMAT, 36);
    assert_eq!(
        hex(stored(&state)),
        concat!(
            "002402018080808010030110000000000000000000000000000000020204416c6578000000000000",
            "2d400000000000004ec0000000000000e03f00000000000000000129000000000000000000000000",
            "0000000000000000000000000000000000000000000000000000000000001b000000000000000000",
            "00000000000000000000000000000000000002000700000000000000000000000000000000000000",
            "000000000100000000020000000000000000a0410015010000000200000000000000000000000100",
            "000000000000280000a0400000000000d80427000000000000000000000000000000000000000000",
            "00000000000000000000000000000000000000000000000000010703010206060104001000000000",
            "0000000000000000000000034d054e6f74636800000000004044400000000000004ec00000000000",
            "00e03f00000000000000000029000000000000000000000000000000000000000000000000000000",
            "0000000000000000000000000000001b000000000000000000000000000000000000000000000000",
            "00000005020101170000000000000000000000000000000000000001000000000000000000000000",
            "a0410000000000000000000000000000000000000000000100000000000000280000a04000000000",
            "00d80427000000000000000000000000000000000000000000000000000000000000000000000000",
            "00000000000000000000010202100000000000000000000000000000000202120020790001010300",
            "10000000000000000000000000000000010105537465766500000000004044400000000000004ec0",
            "000000000000e03f0000000000000000012900000000000000000000000000000000000000000000",
            "00000000000000000000000000000000000000001b00000000000000000000000000000000000000",
            "00000000000000000101000000000000000000000000000000000000000000010000000001000000",
            "00000000a0410000000000000000000200000000000000000000000100000000000000280000a040",
            "0000000000d804270000000000000000000000000000000000000000000000000000000000000000",
            "00000000000000000000000000000104050104060201030506090110000000000000000000000000",
            "000000034d0117060710000000000000000000000000000000034c01180110000000000000000000",
            "000000000000034e034a656207190000010106090202000000",
        )
    );
    assert_eq!(
        hex(stored(&delta)),
        concat!(
            "00240200000210000000000000000000000000000000010010000000000000000000000000000000",
            "02010204416c65780000000000002d400000000000004ec0000000000000e03f0000000000000000",
            "01290000000000000000000000000000000000000000000000000000000000000000000000000000",
            "000000001b0000000000000000000000000000000000000000000000000000000200070000000000",
            "0000000000000000000000000000000000000100000000020000000000000000a041001501000000",
            "0200000000000000000000000100000000000000280000a0400000000000d8042700000000000000",
            "00000000000000000000000000000000000000000000000000000000000000000000000000000001",
            "07010301020600000601040010000000000000000000000000000000034d054e6f74636800000000",
            "004044400000000000004ec0000000000000e03f0000000000000000002900000000000000000000",
            "00000000000000000000000000000000000000000000000000000000000000001b00000000000000",
            "00000000000000000000000000000000000000000502010117000000000000000000000000000000",
            "0000000001000000000000000000000000a041000000000000000000000000000000000000000000",
            "0100000000000000280000a0400000000000d8042700000000000000000000000000000000000000",
            "00000000000000000000000000000000000000000000000000000001020210000000000000000000",
            "00000000000002021200207900010103001000000000000000000000000000000001010553746576",
            "6500000000004044400000000000004ec0000000000000e03f000000000000000001290000000000",
            "000000000000000000000000000000000000000000000000000000000000000000000000001b0000",
            "00000000000000000000000000000000000000000000000000010100000000000000000000000000",
            "000000000000000001000000000100000000000000a0410000000000000000000200000000000000",
            "000000000100000000000000280000a0400000000000d80427000000000000000000000000000000",
            "00000000000000000000000000000000000000000000000000000000000000010405010406020103",
            "0506090110000000000000000000000000000000034d011706071000000000000000000000000000",
            "0000034c011803100000000000000000000000000000000100100000000000000000000000000000",
            "0002001000000000000000000000000000000003014e034a6562071900000101060109000000",
        )
    );
}

/// What an earlier build stored cannot be read. It is dropped up to the last such
/// item: the region is as one that never ran, at that item's tick, and what this
/// build stored behind it is applied.
#[test]
fn what_an_earlier_build_stored_of_a_region_is_dropped_and_the_rest_applied() {
    let ids = EntityIds::block(0).unwrap();
    let restored = |state, deltas| Restored {
        world_spawn: clustine_world::BlockPos::new(0, -60, 0),
        game_mode: clustine_world::GameMode::Creative,
        force_game_mode: false,
        strongholds: Vec::new(),
        homes: Vec::new(),
        dimension: Default::default(),
        faces: Vec::new(),
        spawn: Some(clustine_world::BlockPos::new(0, -60, 0)),
        held: Vec::new(),
        pinned: Vec::new(),
        entity_ids: ids,
        state,
        deltas,
        issued: EntityId(0),
        world: 0,
        seed: None,
        in_use: Vec::new(),
        offset: None,
        couplings: Vec::new(),
        handshakes: Vec::new(),
        held_tick: None,
        given_up: false,
        lends: clustine_rpc::Lends::default(),
        clock: Default::default(),
    };
    let (state, delta) = a_state_and_a_delta();
    let item = |tick, state| clustine_rpc::TickState { tick, state };
    // As builds before the number wrote them: the postcard alone, which begins
    // with the tick.
    let bare_state = postcard::to_stdvec(&state).unwrap();
    let bare_delta = postcard::to_stdvec(&delta).unwrap();
    let fresh = |tick| {
        let mut fresh = RegionState::new(ids);
        fresh.tick = tick;
        fresh
    };

    // A state from before, alone and with a delta from before behind it.
    let alone = restored(Some(item(2, bare_state.clone())), vec![]);
    assert_eq!(restored_state(alone).unwrap(), fresh(2));
    let both = restored(
        Some(item(2, bare_state.clone())),
        vec![item(3, bare_delta.clone())],
    );
    assert_eq!(restored_state(both).unwrap(), fresh(3));

    // A delta of this build behind one from before is applied to what is left.
    let later = StateDelta {
        tick: 4,
        ..StateDelta::default()
    };
    let mixed = restored(
        Some(item(2, stored(&state))),
        vec![item(3, bare_delta), item(4, stored(&later))],
    );
    // The clock starts again with what was dropped, and the delta's tick moves it on.
    assert_eq!(
        restored_state(mixed).unwrap(),
        RegionState {
            clock: clustine_sim::WorldClock {
                game_time: 1,
                day: 1
            },
            ..fresh(4)
        }
    );

    // What this build stored is read as it is, but for its keys: those of the
    // block the store gave (`docs/adr/0028-entities.md`, section 1).
    let whole = restored(Some(item(2, stored(&state))), vec![]);
    assert_eq!(
        restored_state(whole).unwrap(),
        with_keys(state.clone(), ids)
    );

    // A lower number is from before as well; a higher one is a later build's.
    let mut lower = stored(&state);
    lower[1] = STATE_FORMAT - 1;
    let lower = restored(Some(item(2, lower)), vec![]);
    assert_eq!(restored_state(lower).unwrap(), fresh(2));
    let mut higher = stored(&state);
    higher[1] = STATE_FORMAT + 1;
    let higher = restored(Some(item(2, higher)), vec![]);
    assert!(matches!(
        restored_state(higher),
        Err(RestoreError::Format { tick: 2, format }) if format == STATE_FORMAT + 1
    ));
}

/// Opens the one region of `store` as the owner that comes after the one of
/// [`owner`], as the worker does that a released region is given to.
fn opened_next(store: &Store) -> (StoreHandle, Restored) {
    let hello = RegionHello {
        epoch: 2,
        ..owner()
    };
    store.open_region(hello).unwrap()
}

/// Steps a runner that is releasing its region until it has ended, and says how.
fn released(runner: &mut RegionRunner) -> Ended {
    for _ in 0..20_000 {
        runner.step();
        if let Some(ended) = runner.ended() {
            return ended;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("the release did not end");
}

/// Steps a runner that is releasing its region until it has stopped ticking.
fn step_until_it_ticks_no_more(runner: &mut RegionRunner) {
    for _ in 0..20_000 {
        runner.step();
        if runner.phase == Phase::Settling {
            return;
        }
        assert_eq!(runner.phase, Phase::Preparing);
        thread::sleep(Duration::from_millis(1));
    }
    panic!("the store did not answer the flush behind the first checkpoint");
}

/// Waits until the store has answered a flush that the gate holds back.
fn wait_for_kept_flush(runner: &mut RegionRunner, gate: &GateControl) {
    for _ in 0..20_000 {
        assert!(runner.take_replies());
        if gate.kept_flushes() >= 1 {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("the store did not answer the flush");
}

/// The x coordinates of the moves among `messages`, in the order they were told.
fn moves(messages: &[WorkerToEdge]) -> Vec<f64> {
    let mut moves = Vec::new();
    for message in messages {
        if let WorkerToEdge::TickDelta { events, .. } = message {
            for event in events {
                if let RegionEvent::EntityMoved { pose, .. } = event {
                    moves.push(pose.position.x);
                }
            }
        }
    }
    moves
}

/// The ticks that `messages` tell of, in the order they were told.
fn ticks(messages: &[WorkerToEdge]) -> Vec<u64> {
    let tick = |message: &WorkerToEdge| match message {
        WorkerToEdge::TickDelta { tick, .. } => Some(*tick),
        _ => None,
    };
    messages.iter().filter_map(tick).collect()
}

/// A released region is in the store as it was after its last tick: the next owner
/// gets a state file and no commits to apply to it, and every chunk as it was.
#[tokio::test]
async fn a_released_region_is_restored_from_a_state_file_alone() {
    for on_disk_too in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let world = match on_disk_too {
            true => Divided::stripes_in(directory.path()),
            false => Divided::stripes(),
        };
        let (mut edge, worker_end) = in_process(256);
        let mut first = world.runner(RegionId(0), 1);
        let status = first.status();
        first.links().attach(worker_end);
        look_east(&mut first, &mut edge);
        joined(&edge, &mut first).await;
        edge.send(join(other_player(), "Jeb")).await.unwrap();
        take_joins(&mut first);
        edge.send(dig(1)).await.unwrap();
        edge.send(walk(player(), 14.5)).await.unwrap();
        step(&mut first);
        // A block beyond the region, which leaves an entry in the outbox.
        edge.send(dig(16)).await.unwrap();
        step(&mut first);
        assert_eq!(status.ended(), None);

        // Something more happens while the release is under way, and whether the
        // region still takes it or not, the store ends up with the region as it is.
        first.begin_release();
        edge.send(dig(2)).await.unwrap();
        assert_eq!(released(&mut first), Ended::Released);
        assert_eq!(status.ended(), Some(Ended::Released));
        assert!(!status.store_lost.load(Ordering::Relaxed));
        assert!(!first.store_is_lost());
        assert!(first.links.is_empty());
        let state = first.region().state();
        let chunk = first.region().chunk(ORIGIN).unwrap().clone();
        assert_eq!(chunk.get(1, -61, 0), Some(clustine_data::blocks::AIR));
        assert_eq!(state.players.len(), 2);
        assert_eq!(state.edges[&edge.edge].outbox.len(), 1);

        // The edge was told of every tick up to the last before its link closed.
        let told = edge.everything();
        let applied = told.iter().rev().find_map(|message| match message {
            WorkerToEdge::Progress { applied, .. } => Some(*applied),
            _ => None,
        });
        assert_eq!(applied, Some(state.edges[&edge.edge].applied));
        assert!(closed(&mut edge).await);

        // The old runner is still there, which a restore must not depend on.
        let (handle, restored) = world.open(RegionId(0), 2);
        assert_eq!(restored.deltas, []);
        assert_eq!(
            restored.state.as_ref().map(|stored| stored.tick),
            Some(state.tick)
        );
        let block = restored.entity_ids;
        let mut second = RegionRunner::restore(settings(0), handle, restored).unwrap();
        assert_eq!(second.region().state(), with_keys(state.clone(), block));
        let (again, worker_end) = in_process(256);
        second.links().attach(worker_end);
        again.send(asking_for(vec![ORIGIN])).await.unwrap();
        step_until(&mut second, |runner| {
            runner.region().loaded_chunk_count() == 1
        });
        assert_eq!(second.region().chunk(ORIGIN), Some(&chunk));

        // Released for good: no tick, whatever is asked of the runner.
        first.begin_release();
        first.step();
        assert_eq!(first.region().state(), state);
        assert_eq!(first.run(&AtomicBool::new(false)), Ended::Released);
    }
}

/// What a release publishes last is followed at once by the link being closed. An
/// edge gets all of it all the same, also over a link that serialises, where a
/// message is on its way for a while.
#[tokio::test(flavor = "multi_thread")]
async fn what_a_release_publishes_reaches_the_edge_before_its_link_is_closed() {
    for connect in KINDS {
        let store = memory();
        let (mut edge, worker_end) = connect(256);
        let (mut runner, gate) = gated(&store, settings(0));
        runner.links().attach(worker_end);
        joined(&edge, &mut runner).await;

        // The step is taken in a tick whose commit is confirmed only once the region
        // has stopped ticking, so that it is the release that publishes it.
        gate.hold();
        edge.send(walk(player(), 3.0)).await.unwrap();
        for _ in 0..20_000 {
            runner.step();
            if x_of(&runner, player()) == Some(3.0) {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(x_of(&runner, player()), Some(3.0));
        runner.begin_release();
        step_until_it_ticks_no_more(&mut runner);
        gate.release();
        assert_eq!(released(&mut runner), Ended::Released);

        let mut told = Vec::new();
        let everything = async {
            while let Some(message) = edge.end.recv().await {
                told.push(message);
            }
        };
        timeout(Duration::from_secs(10), everything)
            .await
            .expect("the link was not closed");
        assert_eq!(moves(&told).last(), Some(&3.0));
        let applied = runner.region().edge(edge.edge).unwrap().applied;
        assert!(
            matches!(told.last(), Some(WorkerToEdge::Progress { applied: told, .. }) if *told == applied),
            "{told:?}"
        );
    }
}

/// The first checkpoint of a release can be minutes of changed chunks, and players
/// do not stand still for it: the region ticks on and serves its links until the
/// store has it. From then on it takes nothing more, and what an edge sends is the
/// next owner's to apply when the edge sends it again.
#[tokio::test]
async fn a_region_ticks_on_during_the_first_checkpoint_of_a_release_and_not_after() {
    let store = memory();
    let (mut edge, worker_end) = in_process(256);
    let (mut runner, gate) = gated(&store, settings(0));
    runner.links().attach(worker_end);
    joined(&edge, &mut runner).await;
    step(&mut runner);
    edge.everything();

    gate.hold_flushes();
    runner.begin_release();
    assert_eq!(runner.ended(), None);
    // The store has done all of it, which the runner does not get to know.
    wait_for_kept_flush(&mut runner, &gate);
    for x in [3.0, 4.0] {
        let before = runner.region().tick_number();
        edge.send(walk(player(), x)).await.unwrap();
        step(&mut runner);
        assert_eq!(runner.region().tick_number(), before + 1);
        assert_eq!(x_of(&runner, player()), Some(x));
        let told = edge.everything();
        assert_eq!(moves(&told), [x]);
        let applied = edge.sent.load(Ordering::Relaxed);
        assert!(
            matches!(told.last(), Some(WorkerToEdge::Progress { applied: told, .. }) if *told == applied),
            "{told:?}"
        );
    }
    // A link that comes meanwhile is served as well.
    let (mut other, other_end) = in_process(256);
    runner.links().attach(other_end);
    step(&mut runner);
    assert_eq!(other.everything()[0], UNKNOWN);
    assert_eq!(runner.phase, Phase::Preparing);

    // The flush is answered, and the step that finds it so takes nothing from a
    // link any more.
    let late = walk(player(), 5.0);
    edge.send(late.clone()).await.unwrap();
    let number = edge.sent.load(Ordering::Relaxed);
    gate.release_flushes();
    let ticks = runner.region().tick_number();
    assert_eq!(released(&mut runner), Ended::Released);
    assert_eq!(runner.region().tick_number(), ticks);
    assert_eq!(x_of(&runner, player()), Some(4.0));
    assert_eq!(runner.region().edge(edge.edge).unwrap().applied, number - 1);
    assert_eq!(edge.everything(), []);
    assert!(closed(&mut edge).await);
    assert!(closed(&mut other).await);
    let state = runner.region().state();

    // The edge has kept what it was not told to be applied, and sends it again
    // under the number it had.
    let (handle, restored) = opened_next(&store);
    assert_eq!(restored.deltas, []);
    let block = restored.entity_ids;
    let mut next = RegionRunner::restore(settings(0), handle, restored).unwrap();
    assert_eq!(next.region().state(), with_keys(state, block));
    let (edge_end, worker_end) = link::in_process(256);
    let mut again = edge.again(edge_end, &runner);
    next.links().attach(worker_end);
    again
        .send(again.hello(0, &[player()], &[ORIGIN]))
        .await
        .unwrap();
    again.send_as(number, late).await;
    step_until(&mut next, |runner| x_of(runner, player()) == Some(5.0));
    step(&mut next);
    assert_eq!(next.region().edge(edge.edge).unwrap().applied, number);
    assert_eq!(again.everything()[0], RESUMED);
}

/// Ticks that ran before the region stopped ticking are owed to the edges once the
/// store has them, and not before. A release waits for that, and closes the links
/// only after it has published them, oldest first.
#[tokio::test]
async fn a_release_publishes_the_ticks_that_ran_once_they_are_confirmed_and_in_order() {
    let store = memory();
    let (mut edge, worker_end) = in_process(256);
    let (mut runner, gate) = gated(&store, settings(0));
    runner.links().attach(worker_end);
    joined(&edge, &mut runner).await;
    step(&mut runner);
    edge.everything();

    // Two ticks before the release begins and one during its first checkpoint, none
    // of which the runner hears to be on disk.
    gate.hold();
    gate.hold_flushes();
    let first = runner.region().tick_number() + 1;
    for x in [3.0, 4.0] {
        edge.send(walk(player(), x)).await.unwrap();
        runner.step();
    }
    runner.begin_release();
    edge.send(walk(player(), 5.0)).await.unwrap();
    runner.step();
    assert_eq!(runner.region().tick_number(), first + 2);
    assert_eq!(gate.commits().last(), Some(&(first + 2)));
    gate.release_flushes();
    step_until_it_ticks_no_more(&mut runner);
    let last = runner.region().tick_number();

    // The store has all three, and for as long as the runner is not told, nobody
    // else is, and the release goes no further.
    wait_for_kept(&mut runner, &gate, 3);
    for _ in 0..20 {
        runner.step();
    }
    assert_eq!(runner.phase, Phase::Settling);
    assert_eq!(runner.ended(), None);
    assert_eq!(runner.region().tick_number(), last);
    assert_eq!(runner.links.len(), 1);
    assert_eq!(edge.everything(), []);

    gate.release();
    assert_eq!(released(&mut runner), Ended::Released);
    let told = edge.everything();
    assert_eq!(moves(&told), [3.0, 4.0, 5.0]);
    assert_eq!(ticks(&told), [first, first + 1, first + 2]);
    let number = edge.sent.load(Ordering::Relaxed);
    let applied: Vec<_> = told
        .iter()
        .filter_map(|message| match message {
            WorkerToEdge::Progress { applied, .. } => Some(*applied),
            _ => None,
        })
        .collect();
    assert_eq!(applied, [number - 2, number - 1, number]);
    assert!(closed(&mut edge).await);

    let (_handle, restored) = opened_next(&store);
    assert_eq!(restored.deltas, []);
    let block = restored.entity_ids;
    let state = restored_state(restored).unwrap();
    assert_eq!(state, with_keys(runner.region().state(), block));
    assert_eq!(state.players[&player()].pose.position.x, 5.0);
}

/// A release whose store handle is lost has nothing left to do that it could do:
/// the store has what it confirmed, and the rest was never shown. It shows nothing
/// more, closes its links and ends, whenever that happens.
#[tokio::test]
async fn a_release_whose_store_handle_is_lost_ends_as_lost_and_publishes_nothing_more() {
    for case in [
        "before it begins",
        "during the first checkpoint",
        "while it waits for commits",
        "during the last checkpoint",
    ] {
        let store = memory();
        let (mut edge, worker_end) = in_process(256);
        let (mut runner, gate) = gated(&store, settings(0));
        let (links, status) = (runner.links(), runner.status());
        links.attach(worker_end);
        joined(&edge, &mut runner).await;
        step(&mut runner);
        edge.everything();

        // A tick the store has, and the edge has not heard of.
        gate.hold();
        edge.send(walk(player(), 3.0)).await.unwrap();
        runner.step();
        wait_for_kept(&mut runner, &gate, 1);
        match case {
            "before it begins" => {
                gate.lose();
                runner.begin_release();
                // Known at once, without a step.
                assert_eq!(runner.ended(), Some(Ended::StoreLost));
            }
            "during the first checkpoint" => {
                gate.hold_flushes();
                runner.begin_release();
                runner.step();
                assert_eq!(runner.phase, Phase::Preparing);
                gate.lose();
            }
            "while it waits for commits" => {
                runner.begin_release();
                step_until_it_ticks_no_more(&mut runner);
                runner.step();
                assert_eq!(runner.phase, Phase::Settling);
                gate.lose();
            }
            _ => {
                // The tick is confirmed and published; only the last flush is not
                // answered.
                runner.begin_release();
                step_until_it_ticks_no_more(&mut runner);
                gate.hold_flushes();
                gate.release();
                for _ in 0..20_000 {
                    runner.step();
                    if runner.phase == Phase::Closing {
                        break;
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                assert_eq!(runner.phase, Phase::Closing);
                wait_for_kept_flush(&mut runner, &gate);
                assert_eq!(moves(&edge.everything()), [3.0], "{case}");
                gate.lose();
            }
        }
        assert_eq!(released(&mut runner), Ended::StoreLost, "{case}");
        assert_eq!(runner.ended(), Some(Ended::StoreLost), "{case}");
        assert_eq!(status.ended(), Some(Ended::StoreLost), "{case}");
        assert!(status.store_lost.load(Ordering::Relaxed), "{case}");
        assert!(runner.store_is_lost(), "{case}");
        assert!(runner.links.is_empty(), "{case}");

        // That the store answers after all changes nothing, and the runner returns
        // by itself rather than wait for anything.
        gate.release();
        gate.release_flushes();
        let ticks = runner.region().tick_number();
        runner.step();
        assert_eq!(runner.run(&AtomicBool::new(false)), Ended::StoreLost);
        assert_eq!(runner.region().tick_number(), ticks, "{case}");
        assert_eq!(edge.everything(), [], "{case}");
        assert!(closed(&mut edge).await, "{case}");

        let (mut late, late_end) = in_process(256);
        links.attach(late_end);
        runner.step();
        assert_eq!(late.everything(), [], "{case}");
        assert!(closed(&mut late).await, "{case}");
    }
}

/// Once a region has stopped ticking it takes no new links: there is nothing it
/// could tell them. Their edges find them closed and look for the region again.
#[tokio::test]
async fn a_link_attached_once_a_released_region_ticks_no_more_is_closed_and_not_served() {
    let store = memory();
    let (mut edge, worker_end) = in_process(256);
    let (mut runner, gate) = gated(&store, settings(0));
    let links = runner.links();
    links.attach(worker_end);
    joined(&edge, &mut runner).await;
    step(&mut runner);
    edge.everything();

    // The release waits for a commit, so that it lasts.
    gate.hold();
    edge.send(walk(player(), 3.0)).await.unwrap();
    runner.step();
    runner.begin_release();
    step_until_it_ticks_no_more(&mut runner);

    let (mut during, during_end) = in_process(256);
    links.attach(during_end);
    during.send(asking_for(vec![ORIGIN])).await.unwrap();
    runner.step();
    assert_eq!(runner.phase, Phase::Settling);
    assert_eq!(during.everything(), []);
    assert!(closed(&mut during).await);
    // The link it had stays for what it is still owed.
    assert_eq!(runner.links.len(), 1);
    assert!(runner.region().edge(during.edge).is_none());

    gate.release();
    assert_eq!(released(&mut runner), Ended::Released);
    assert_eq!(moves(&edge.everything()), [3.0]);
    assert!(closed(&mut edge).await);

    let (mut after, after_end) = in_process(256);
    links.attach(after_end);
    runner.step();
    assert_eq!(after.everything(), []);
    assert!(closed(&mut after).await);
}

/// The checkpoints of the interval go on while a release is under way, also in the
/// very tick the release begins after. The store ends up with the region as it is
/// all the same.
#[tokio::test]
async fn checkpoints_of_the_interval_that_fall_into_a_release_do_no_harm() {
    for held in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let store = on_disk(directory.path());
        let (edge, worker_end) = in_process(256);
        let (runner, gate) = gated(&store, settings(0));
        // A checkpoint with every tick.
        let mut runner = runner.with_checkpoint_interval(1);
        runner.links().attach(worker_end);
        joined(&edge, &mut runner).await;
        edge.send(dig(1)).await.unwrap();
        step(&mut runner);

        if held {
            gate.hold_flushes();
        }
        runner.begin_release();
        // Held, the region takes all of these; otherwise as many as it gets to.
        for x in 2..6 {
            edge.send(dig(x)).await.unwrap();
            step(&mut runner);
        }
        if held {
            assert_eq!(runner.phase, Phase::Preparing);
            let chunk = runner.region().chunk(ORIGIN).unwrap();
            assert_eq!(chunk.get(5, -61, 0), Some(clustine_data::blocks::AIR));
            gate.release_flushes();
        }
        assert_eq!(released(&mut runner), Ended::Released);
        let state = runner.region().state();
        let chunk = runner.region().chunk(ORIGIN).unwrap().clone();

        let (handle, restored) = opened_next(&store);
        assert_eq!(restored.deltas, []);
        let settings = settings(0);
        let block = restored.entity_ids;
        let mut next = RegionRunner::restore(settings, handle, restored).unwrap();
        assert_eq!(next.region().state(), with_keys(state.clone(), block));
        let (again, worker_end) = in_process(256);
        next.links().attach(worker_end);
        again.send(asking_for(vec![ORIGIN])).await.unwrap();
        step_until(&mut next, |runner| {
            runner.region().loaded_chunk_count() == 1
        });
        assert_eq!(next.region().chunk(ORIGIN), Some(&chunk));
    }
}

/// Waits until the region thread of `worker` has ended by itself.
fn wait_until_finished(worker: &Worker) {
    for _ in 0..20_000 {
        if worker.is_finished() {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("the region thread did not end");
}

/// A region on a thread of its own is released by asking its worker, which says how
/// that went once the thread is gone.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_releases_its_region_and_its_thread_ends() {
    // Waiting for the release, and asking for it and looking later.
    for blocking in [true, false] {
        let store = memory();
        let runner = opened(&store, settings(0));
        let (links, status) = (runner.links(), runner.status());
        let worker = Worker::spawn(runner);
        let (mut edge, worker_end) = in_process(256);
        links.attach(worker_end);
        edge.send(join(player(), "Notch")).await.unwrap();
        edge.send(asking_for(vec![ORIGIN])).await.unwrap();
        // A player who has entered the world acts on a chunk they have been shown.
        let (mut entered, mut shown) = (false, false);
        while !(entered && shown) {
            match next(&mut edge).await {
                WorkerToEdge::ToPlayer { .. } => entered = true,
                WorkerToEdge::ChunkSnapshot { .. } => shown = true,
                _ => {}
            }
        }
        edge.send(dig_by(player(), 1, 9)).await.unwrap();
        let acknowledged = PlayerEvent::Acknowledged { sequence: 9 };
        loop {
            match next(&mut edge).await {
                WorkerToEdge::ToPlayer { event, .. } if event == acknowledged => break,
                _ => {}
            }
        }
        assert_eq!(status.ended(), None);
        assert!(!worker.is_finished());

        let ended = if blocking {
            worker.release()
        } else {
            worker.begin_release();
            wait_until_finished(&worker);
            assert_eq!(status.ended(), Some(Ended::Released));
            // There is nothing left to stop, and stopping says how it ended.
            worker.stop()
        };
        assert_eq!(ended, Ended::Released);
        assert_eq!(status.ended(), Some(Ended::Released));
        assert!(!status.store_lost.load(Ordering::Relaxed));
        assert!(closed(&mut edge).await);
        let (mut late, late_end) = in_process(256);
        links.attach(late_end);
        assert!(closed(&mut late).await);

        let (handle, restored) = opened_next(&store);
        assert_eq!(restored.deltas, []);
        assert!(restored.state.is_some());
        let settings = settings(0);
        let mut next = RegionRunner::restore(settings, handle, restored).unwrap();
        assert_eq!(next.region().player_count(), 1);
        assert_eq!(
            next.region().tick_number(),
            status.tick.load(Ordering::Relaxed)
        );
        let (again, worker_end) = in_process(256);
        next.links().attach(worker_end);
        again.send(asking_for(vec![ORIGIN])).await.unwrap();
        step_until(&mut next, |runner| {
            runner.region().loaded_chunk_count() == 1
        });
        let chunk = next.region().chunk(ORIGIN).unwrap();
        assert_eq!(chunk.get(1, -61, 0), Some(clustine_data::blocks::AIR));
    }
}

/// A release waits for the store for as long as the store neither answers nor
/// closes. Whoever cannot wait any longer stops the worker, which lets go of the
/// region at once and as it is; that is a crash, which the next owner recovers
/// from. A worker that is not releasing anything stops as it always has.
#[tokio::test(flavor = "multi_thread")]
async fn stopping_a_worker_ends_a_release_that_waits_for_the_store() {
    let store = memory();
    let (runner, gate) = gated(&store, settings(0));
    let (links, status) = (runner.links(), runner.status());
    let worker = Worker::spawn(runner);
    let (mut edge, worker_end) = in_process(256);
    links.attach(worker_end);
    edge.send(join(player(), "Notch")).await.unwrap();
    while !matches!(next(&mut edge).await, WorkerToEdge::ToPlayer { .. }) {}

    // The answer to the first flush never comes.
    gate.hold_flushes();
    worker.begin_release();
    for _ in 0..20_000 {
        if gate.flushes() >= 1 {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(gate.flushes(), 1);
    assert_eq!(status.ended(), None);
    assert!(!worker.is_finished());

    assert_eq!(worker.stop(), Ended::Abandoned);
    assert_eq!(status.ended(), Some(Ended::Abandoned));
    assert!(!status.store_lost.load(Ordering::Relaxed));
    assert!(closed(&mut edge).await);
    // Nothing more was asked of the store.
    assert_eq!(gate.flushes(), 1);

    // The player was shown to be in the world, so the next owner has them.
    let (_handle, restored) = opened_next(&store);
    let state = restored_state(restored).unwrap();
    assert!(state.players.contains_key(&player()));

    let runner = opened(&memory(), settings(0));
    let status = runner.status();
    let worker = Worker::spawn(runner);
    assert_eq!(worker.stop(), Ended::Stopped);
    assert_eq!(status.ended(), Some(Ended::Stopped));
}

// -----------------------------------------------------------------------------------
// A region on the chunks the world store grants it: section 4 of
// `docs/adr/0012-the-tick-on-chunks.md`.
// -----------------------------------------------------------------------------------

/// The chunk east of the one players enter the world in: the first of the eastern
/// stripe in [`Divided::stripes`], and nobody's in [`Divided::gap`].
const BESIDE: ChunkPos = ChunkPos::new(1, 0);

/// The home region of [`Divided::gap`].
const HOME: RegionId = RegionId(2);

/// The homes of the Nether and of the End in [`Divided::stripes`]. Every world is
/// made with them after the overworld's regions (`docs/groundwork/p-plan.md`,
/// decision 2); they hold their dimensions' chunks (0, 0) and are never run here,
/// but they are in every list of regions.
const NETHER_HOME: RegionId = RegionId(2);
const END_HOME: RegionId = RegionId(3);

/// The id the store of [`Divided::stripes`] gives the first region a split makes:
/// the first after the homes.
const FIRST_PART: RegionId = RegionId(4);

/// The same for [`Divided::gap`], whose three regions come before the homes, which
/// are 3 and 4.
const GAP_FIRST_PART: RegionId = RegionId(5);

/// The ids of the regions `list` names, in order.
fn ids(list: &RegionList) -> Vec<RegionId> {
    list.regions.iter().map(|info| info.region).collect()
}

/// What the constants above say of the worlds the tests make is what their stores
/// say.
#[test]
fn the_homes_of_the_nether_and_the_end_come_after_the_regions_of_the_overworld() {
    let stripes = Divided::stripes().store.regions().unwrap();
    assert_eq!(
        stripes.homes,
        [
            (Dimension::Overworld, RegionId(0)),
            (Dimension::Nether, NETHER_HOME),
            (Dimension::End, END_HOME)
        ]
    );
    assert_eq!(ids(&stripes), [RegionId(0), EAST, NETHER_HOME, END_HOME]);
    assert_eq!(stripes.next, FIRST_PART);
    let gap = Divided::gap().store.regions().unwrap();
    assert_eq!(
        gap.homes,
        [
            (Dimension::Overworld, HOME),
            (Dimension::Nether, RegionId(3)),
            (Dimension::End, RegionId(4))
        ]
    );
    assert_eq!(gap.next, GAP_FIRST_PART);
}

/// A world that is divided, in memory unless said otherwise, of which a test runs
/// one region and plays another through a handle of its own.
struct Divided {
    store: Store,
}

impl Divided {
    fn new(division: Pins) -> Self {
        let generator = Arc::new(FlatGenerator::classic());
        let store = Store::memory_divided(generator, division).unwrap();
        Self { store }
    }

    /// The division of [`Divided::stripes`].
    fn line_at_one() -> Pins {
        Pins::side_by_side(&[1]).unwrap()
    }

    /// Two stripes with the line at x = 1: region 0 west of it, which has the chunk
    /// players enter the world in, and [`EAST`].
    fn stripes() -> Self {
        Self::new(Self::line_at_one())
    }

    /// The world of [`Divided::stripes`] as it is kept in `directory`.
    fn stripes_in(directory: &std::path::Path) -> Self {
        let generator = Arc::new(FlatGenerator::classic());
        let division = Self::line_at_one();
        let store = Store::local_divided(directory, generator, division).unwrap();
        Self { store }
    }

    /// The division with a gap of ADR-0011: region 0 is pinned to the chunks west of
    /// x = 0 and [`EAST`] to those from x = 16 on. [`HOME`] is pinned to nothing
    /// and holds the chunk at the origin, where players enter the world; the other
    /// chunks in between are nobody's.
    fn gap() -> Self {
        let west = ChunkArea {
            min_x: None,
            max_x: Some(0),
        };
        let east = ChunkArea {
            min_x: Some(16),
            max_x: None,
        };
        Self::new(Pins {
            areas: vec![west, east],
        })
    }

    fn hello(&self, region: RegionId, epoch: u64) -> RegionHello {
        RegionHello { region, epoch }
    }

    /// Opens `region` as its owner with `epoch`, for a test that plays the region
    /// itself.
    fn open(&self, region: RegionId, epoch: u64) -> (StoreHandle, Restored) {
        self.store.open_region(self.hello(region, epoch)).unwrap()
    }

    /// A runner for `region` as the store has it, opened with `epoch`, that gives a
    /// chunk back as soon as nothing uses it and is told which region it runs.
    fn runner(&self, region: RegionId, epoch: u64) -> RegionRunner {
        let (handle, restored) = self.open(region, epoch);
        let runner = RegionRunner::restore(settings(0), handle, restored).unwrap();
        runner.with_id(region)
    }

    /// A runner for `region` as its first owner, with a gate before its store,
    /// that is told which region it runs.
    fn gated(
        &self,
        region: RegionId,
        settings: RegionSettings,
    ) -> (RegionRunner, Arc<GateControl>) {
        let (runner, gate) = gated_as(&self.store, self.hello(region, 1), settings);
        (runner.with_id(region), gate)
    }
}

/// Asks the store through `handle` to grant `chunks`, and returns what it answers:
/// those it grants, and those another region holds.
fn claim(handle: &StoreHandle, chunks: &[ChunkPos]) -> (Vec<ChunkPos>, Vec<(ChunkPos, RegionId)>) {
    handle.request(StoreRequest::Claim {
        chunks: chunks.to_vec(),
    });
    for _ in 0..20_000 {
        match handle.try_reply() {
            Some(StoreReply::Claimed { granted, foreign }) => return (granted, foreign),
            // The store tells every region that is open which stays a join has
            // replaced (ADR-0020, section 5). A test that plays a region through
            // its handle has nobody, and passes the word over.
            Some(StoreReply::Dead { .. }) => {}
            // Nor the counts of mobs it publishes behind a whole report.
            Some(StoreReply::Counts(_)) => {}
            Some(other) => panic!("the store answered a claim with {other:?}"),
            None => thread::sleep(Duration::from_millis(1)),
        }
    }
    panic!("the store did not answer the claim");
}

/// Claims `chunk` through `handle` until the store grants it, stepping `runner` in
/// between: a chunk that a region gives back is free once the store has got to the
/// return, which nothing tells anyone.
fn claim_until_granted(runner: &mut RegionRunner, handle: &StoreHandle, chunk: ChunkPos) {
    for _ in 0..2000 {
        if claim(handle, &[chunk]).0 == [chunk] {
            return;
        }
        step(runner);
    }
    panic!("the chunk was never given back");
}

/// What an answer to a subscription says, without the content of a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Told {
    Snapshot,
    Elsewhere(RegionId),
    NotMine,
}

/// The chunk `message` answers a subscription to, the number of the asking it
/// answers and what it says, if it is such an answer.
fn told(message: &WorkerToEdge) -> Option<(ChunkPos, u64, Told)> {
    match message {
        WorkerToEdge::ChunkSnapshot { position, ask, .. } => {
            Some((*position, *ask, Told::Snapshot))
        }
        WorkerToEdge::Elsewhere { chunk, ask, region } => {
            Some((*chunk, *ask, Told::Elsewhere(*region)))
        }
        WorkerToEdge::NotMine { chunk, ask } => Some((*chunk, *ask, Told::NotMine)),
        _ => None,
    }
}

/// Steps `runner` until `edge` has been given `count` answers to subscriptions, and
/// returns them in ascending order of their chunks. What else it is told meanwhile
/// is passed over.
fn answers(
    runner: &mut RegionRunner,
    edge: &mut TestEdge,
    count: usize,
) -> Vec<(ChunkPos, u64, Told)> {
    let mut answers = Vec::new();
    for _ in 0..2000 {
        answers.extend(received(edge).iter().filter_map(told));
        if answers.len() >= count {
            answers.sort();
            return answers;
        }
        step(runner);
        thread::sleep(Duration::from_millis(1));
    }
    panic!("of {count} answers only these came: {answers:?}");
}

/// Subscribes `edge` to a chunk in the column `x` that nobody else needs, which has
/// to be one the region is granted, and steps `runner` until its snapshot is there.
/// Returns what the edge was told meanwhile, but for that snapshot.
///
/// The store answers claims in the order they were made and loads in the order they
/// were asked, and a tick is published whole and after those before it. So whatever
/// the askings before this one lead to has been told by then, and a test can say
/// that something did not come.
fn told_until_marked(runner: &mut RegionRunner, edge: &mut TestEdge, x: i32) -> Vec<WorkerToEdge> {
    static NEXT: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(1000);
    let mark = ChunkPos::new(x, NEXT.fetch_add(1, Ordering::Relaxed));
    edge.try_send(asking_for(vec![mark])).unwrap();
    let mut before = Vec::new();
    for _ in 0..2000 {
        let batch = received(edge);
        let marked = batch
            .iter()
            .any(|message| told(message).is_some_and(|told| told.0 == mark));
        before.extend(
            batch
                .into_iter()
                .filter(|message| told(message).is_none_or(|told| told.0 != mark)),
        );
        if marked {
            edge.try_send(done_with(vec![mark])).unwrap();
            return before;
        }
        step(runner);
        thread::sleep(Duration::from_millis(1));
    }
    panic!("the marking snapshot never came");
}

/// Waits until the gate holds back `count` answers of the kind that `kept` counts.
fn wait_for_kept_answers(
    runner: &mut RegionRunner,
    gate: &GateControl,
    kept: fn(&GateControl) -> usize,
    count: usize,
) {
    for _ in 0..20_000 {
        assert!(runner.take_replies());
        if kept(gate) >= count {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("the store did not answer");
}

/// A viewer's subscription is answered with the chunk if the region holds it, which
/// it finds out by asking the store, and else with who does. Of a chunk that is
/// elsewhere nothing more is said until the edge asks again.
#[tokio::test]
async fn a_viewer_is_sent_what_the_region_holds_and_told_who_holds_the_rest() {
    let world = Divided::stripes();
    let (mut edge, worker_end) = in_process(256);
    let mut runner = world.runner(RegionId(0), 1);
    runner.links().attach(worker_end);
    assert_eq!(runner.region().knowledge(ORIGIN), Knowledge::Unknown);

    edge.send(asking_for(vec![ORIGIN, BESIDE])).await.unwrap();
    assert_eq!(
        answers(&mut runner, &mut edge, 2),
        [
            (ORIGIN, 1, Told::Snapshot),
            (BESIDE, 1, Told::Elsewhere(EAST))
        ]
    );
    assert_eq!(runner.region().knowledge(ORIGIN), Knowledge::Held);
    assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Foreign(EAST));
    assert_eq!(runner.region().loaded_chunk_count(), 1);

    // What happens in the chunk that is elsewhere is not told.
    for (entity, chunk) in [(EntityId(8), BESIDE), (EntityId(9), ORIGIN)] {
        edge.send(EdgeToWorker::Discard {
            player: player(),
            entity,
            hops: 1,
            chunk,
        })
        .await
        .unwrap();
    }
    let removed = RegionEvent::EntityRemoved {
        entity: EntityId(9),
        chunk: ORIGIN,
    };
    assert_eq!(events(step_for(&mut runner, &mut edge)), [removed]);

    // Asked again, the region asks the store again, and answers with the number of
    // that asking.
    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    assert_eq!(
        answers(&mut runner, &mut edge, 1),
        [(BESIDE, 2, Told::Elsewhere(EAST))]
    );
    assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Foreign(EAST));
}

/// A guest is served what the region holds, and what it may take without taking it
/// from anyone: a chunk of an area it is pinned to. Of any other chunk it is told
/// that it is not the region's, and the region does not ask for it.
#[tokio::test]
async fn a_guest_is_sent_what_the_region_holds_and_told_that_the_rest_is_not_its() {
    let world = Divided::stripes();
    let (mut edge, worker_end) = in_process(256);
    let mut runner = world.runner(RegionId(0), 1);
    runner.links().attach(worker_end);
    let own = ChunkPos::new(-1, 0);

    edge.send(asking_as_guest_for(vec![own, BESIDE]))
        .await
        .unwrap();
    assert_eq!(
        answers(&mut runner, &mut edge, 2),
        [(own, 1, Told::Snapshot), (BESIDE, 1, Told::NotMine)]
    );
    // The subscription that was told so is over.
    assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Unknown);
    let subscribed: Vec<_> = runner.links[&LinkId(0)].subscriptions.keys().collect();
    assert_eq!(subscribed, [&own]);

    // On open land a guest is no reason to take a chunk: it stays free.
    let world = Divided::gap();
    let (mut edge, worker_end) = in_process(256);
    let mut runner = world.runner(HOME, 1);
    runner.links().attach(worker_end);
    edge.send(asking_as_guest_for(vec![BESIDE])).await.unwrap();
    assert_eq!(
        answers(&mut runner, &mut edge, 1),
        [(BESIDE, 1, Told::NotMine)]
    );
    let (neighbour, _) = world.open(EAST, 1);
    assert_eq!(claim(&neighbour, &[BESIDE]), (vec![BESIDE], vec![]));
}

/// A subscription that was told elsewhere, then made a guest's and told that the
/// chunk is not the region's, is over. Asking again begins anew.
#[tokio::test]
async fn a_subscription_that_ended_with_not_mine_begins_anew_when_it_is_asked_for_again() {
    let world = Divided::stripes();
    let (mut edge, worker_end) = in_process(256);
    let mut runner = world.runner(RegionId(0), 1);
    runner.links().attach(worker_end);

    for (message, told) in [
        (asking_for(vec![BESIDE]), Told::Elsewhere(EAST)),
        (asking_as_guest_for(vec![BESIDE]), Told::NotMine),
        (asking_for(vec![BESIDE]), Told::Elsewhere(EAST)),
    ] {
        edge.send(message).await.unwrap();
        let ask = edge.asked();
        assert_eq!(answers(&mut runner, &mut edge, 1), [(BESIDE, ask, told)]);
    }
}

/// A chunk outside the pinned areas is the region's from a viewer's asking for it
/// until no link is subscribed to it any more, whatever kind the subscription is
/// by then. Changing the kind of a subscription that is served costs no answer
/// and loses no event.
#[tokio::test]
async fn a_chunk_stays_with_its_region_while_a_link_is_subscribed_to_it_as_viewer_or_as_guest() {
    let world = Divided::gap();
    let (mut edge, worker_end) = in_process(256);
    let mut runner = world.runner(HOME, 1);
    runner.links().attach(worker_end);
    let status = runner.status();
    let (neighbour, _) = world.open(EAST, 1);
    let taken = (vec![], vec![(BESIDE, HOME)]);

    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    assert_eq!(
        answers(&mut runner, &mut edge, 1),
        [(BESIDE, 1, Told::Snapshot)]
    );
    // The home chunk and this one.
    assert_eq!(status.held.load(Ordering::Relaxed), 2);
    assert_eq!(claim(&neighbour, &[BESIDE]), taken);

    // As a guest's, and as a viewer's again.
    for (entity, message) in [
        (EntityId(8), asking_as_guest_for(vec![BESIDE])),
        (EntityId(9), asking_for(vec![BESIDE])),
    ] {
        edge.send(message).await.unwrap();
        let discard = EdgeToWorker::Discard {
            player: player(),
            entity,
            hops: 1,
            chunk: BESIDE,
        };
        edge.send(discard).await.unwrap();
        let told: Vec<_> = told_until_marked(&mut runner, &mut edge, 15)
            .into_iter()
            .map(events)
            .collect();
        let removed = RegionEvent::EntityRemoved {
            entity,
            chunk: BESIDE,
        };
        assert_eq!(told, [vec![removed]]);
        assert_eq!(claim(&neighbour, &[BESIDE]), taken);
    }

    // Once the link lets go of it, nothing uses the chunk, and it is given back.
    edge.send(done_with(vec![BESIDE])).await.unwrap();
    claim_until_granted(&mut runner, &neighbour, BESIDE);
    assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Unknown);
}

/// A link that ends takes its tickets with it, so what the region held for it
/// alone is given back.
#[tokio::test]
async fn the_chunks_of_a_link_that_ended_are_given_back() {
    let world = Divided::gap();
    let (mut edge, worker_end) = in_process(256);
    let mut runner = world.runner(HOME, 1);
    runner.links().attach(worker_end);
    let (neighbour, _) = world.open(EAST, 1);
    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    assert_eq!(
        answers(&mut runner, &mut edge, 1),
        [(BESIDE, 1, Told::Snapshot)]
    );

    drop(edge);
    claim_until_granted(&mut runner, &neighbour, BESIDE);
    // The chunk players enter in is never given back.
    assert_eq!(runner.region().knowledge(ORIGIN), Knowledge::Held);
}

/// A region that was told who holds a chunk does not learn by itself that the
/// chunk has become free. The edge's asking again has it ask the store again.
#[tokio::test]
async fn asking_again_finds_a_chunk_that_its_holder_has_given_back() {
    let world = Divided::gap();
    let (mut edge, worker_end) = in_process(256);
    let mut runner = world.runner(HOME, 1);
    runner.links().attach(worker_end);
    let (neighbour, _) = world.open(EAST, 1);
    assert_eq!(claim(&neighbour, &[BESIDE]), (vec![BESIDE], vec![]));

    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    assert_eq!(
        answers(&mut runner, &mut edge, 1),
        [(BESIDE, 1, Told::Elsewhere(EAST))]
    );

    // The neighbour gives the chunk back, and the store has got to it.
    neighbour.request(StoreRequest::Return {
        chunks: vec![BESIDE],
        unreadable: Vec::new(),
    });
    neighbour.flush();
    let told = told_until_marked(&mut runner, &mut edge, 15);
    assert!(told.is_empty(), "{told:?}");
    assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Foreign(EAST));

    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    let ask = edge.asked();
    assert_eq!(
        answers(&mut runner, &mut edge, 1),
        [(BESIDE, ask, Told::Snapshot)]
    );
}

/// Several askings for one chunk before the region gets to answer share one
/// answer, which carries the number of the last and is of the kind the
/// subscription has by then.
#[tokio::test]
async fn an_answer_carries_the_number_of_the_last_asking() {
    let world = Divided::stripes();
    let (mut edge, worker_end) = in_process(256);
    let mut runner = world.runner(RegionId(0), 1);
    runner.links().attach(worker_end);
    let (again, turned) = (ChunkPos::new(-1, 0), ChunkPos::new(-2, 0));

    // All of it before the region ticks once.
    for message in [
        // Ended and made again.
        asking_for(vec![again]),
        done_with(vec![again]),
        asking_for(vec![again]),
        // A viewer's turned into a guest's: no `Elsewhere` comes for it.
        asking_for(vec![BESIDE]),
        asking_as_guest_for(vec![BESIDE]),
        // A guest's turned into a viewer's.
        asking_as_guest_for(vec![turned]),
        asking_for(vec![turned]),
    ] {
        edge.send(message).await.unwrap();
    }
    let mut told: Vec<_> = told_until_marked(&mut runner, &mut edge, 0)
        .iter()
        .filter_map(told)
        .collect();
    told.sort();
    assert_eq!(
        told,
        [
            (turned, 7, Told::Snapshot),
            (again, 3, Told::Snapshot),
            (BESIDE, 5, Told::NotMine)
        ]
    );
    let link = &runner.links[&LinkId(0)];
    assert_eq!(link.subscriptions[&turned].kind, Ticket::Viewer);
    assert!(!link.subscriptions.contains_key(&BESIDE));
}

/// Answers carry the numbers of subscription messages, so an edge that numbers
/// them out of order has lost track, as with the numbers of what changes the
/// region. A hello's chunks are the message with the number 0.
#[tokio::test]
async fn a_link_whose_subscription_messages_are_out_of_order_is_closed() {
    let hello = |edge: &TestEdge, chunks: &[ChunkPos], guests: &[ChunkPos]| {
        let EdgeToWorker::Hello {
            edge,
            start,
            since,
            seen,
            players,
            ..
        } = edge.hello(0, &[], &[])
        else {
            unreachable!("a hello is a hello");
        };
        EdgeToWorker::Hello {
            edge,
            start,
            since,
            seen,
            players,
            chunks: chunks.to_vec(),
            guests: guests.to_vec(),
        }
    };
    let subscribe = |ask| EdgeToWorker::Subscribe {
        ask,
        chunks: vec![ORIGIN],
    };
    let as_guest = |ask| EdgeToWorker::SubscribeAsGuest {
        ask,
        chunks: vec![ORIGIN],
    };
    let unsubscribe = |ask| EdgeToWorker::Unsubscribe {
        ask,
        chunks: vec![ORIGIN],
    };
    let wrong = |edge: &TestEdge| {
        [
            vec![subscribe(0)],
            vec![subscribe(2), unsubscribe(2)],
            vec![subscribe(2), as_guest(1)],
            vec![unsubscribe(1), subscribe(1)],
            vec![subscribe(1), hello(edge, &[ORIGIN], &[])],
            vec![unsubscribe(1), hello(edge, &[], &[ORIGIN])],
            vec![hello(edge, &[ORIGIN], &[]), subscribe(0)],
        ]
    };
    for case in 0..7 {
        let (end, worker_end) = link::in_process(256);
        let mut edge = TestEdge::silent(end, EdgeId::from_name("out-of-order"), 1);
        let mut runner = runner(worker_end);
        for message in wrong(&edge).into_iter().nth(case).unwrap() {
            let message = EdgeMessage::unnumbered(message);
            edge.send_as_is(message).await.unwrap();
        }
        step_until(&mut runner, |runner| runner.links.is_empty());
        assert!(closed(&mut edge).await, "case {case}");
    }

    // Numbers that only ascend are in order, with gaps or without, and a hello that
    // names no chunk can come behind a subscription.
    let (end, worker_end) = link::in_process(256);
    let mut edge = TestEdge::silent(end, EdgeId::from_name("in-order"), 1);
    let mut runner = runner(worker_end);
    for message in [
        subscribe(1),
        hello(&edge, &[], &[]),
        as_guest(5),
        unsubscribe(6),
    ] {
        let message = EdgeMessage::unnumbered(message);
        edge.send_as_is(message).await.unwrap();
    }
    step(&mut runner);
    assert!(edge.everything().contains(&UNKNOWN));
    assert_eq!(runner.links[&LinkId(0)].asked, Some(6));
}

/// What an edge sends behind its hello is applied only when the region knows of
/// every chunk the hello names whose it is: each has been answered, a viewer's and
/// a guest's alike, with the number 0.
#[tokio::test]
async fn a_hello_holds_its_link_until_each_of_its_chunks_is_answered() {
    let world = Divided::stripes();
    let (end, worker_end) = link::in_process(256);
    let mut edge = TestEdge::silent(end, EdgeId::from_name("held"), 1);
    let mut runner = world.runner(RegionId(0), 1);
    runner.links().attach(worker_end);
    let (own, far) = (ChunkPos::new(-1, 0), ChunkPos::new(2, 0));

    let hello = EdgeToWorker::Hello {
        edge: edge.edge,
        start: edge.start,
        since: 0,
        seen: 0,
        players: Vec::new(),
        chunks: vec![ORIGIN, BESIDE],
        // One of its own, one of the other stripe, and one that is a viewer's too.
        guests: vec![own, far, ORIGIN],
    };
    edge.send(hello).await.unwrap();
    edge.send(join(player(), "Notch")).await.unwrap();

    let all = BTreeSet::from([
        (ORIGIN, 0, Told::Snapshot),
        (BESIDE, 0, Told::Elsewhere(EAST)),
        (own, 0, Told::Snapshot),
        (far, 0, Told::NotMine),
    ]);
    let mut answered = BTreeSet::new();
    let mut once = 0;
    step_until(&mut runner, |runner| {
        for answer in edge.everything().iter().filter_map(told) {
            answered.insert(answer);
            once += 1;
        }
        let joined = runner.region().player_count() == 1;
        assert!(!joined || answered == all, "{answered:?}");
        joined
    });
    assert_eq!(once, 4, "each is answered once");
    // A chunk in both lists is a viewer's.
    let link = &runner.links[&LinkId(0)];
    assert_eq!(link.subscriptions[&ORIGIN].kind, Ticket::Viewer);
    assert_eq!(link.subscriptions[&own].kind, Ticket::Guest);
}

/// A player who steps into a chunk the region knows nothing of stays the region's
/// until the store has said whose the chunk is. If it is another region's they are
/// let go in the tick of that answer, before the edge is told of the chunk.
#[tokio::test]
async fn a_player_in_a_chunk_nobody_asked_about_is_let_go_when_the_store_has_answered() {
    let world = Divided::stripes();
    let (mut edge, worker_end) = in_process(256);
    let mut runner = world.runner(RegionId(0), 1);
    runner.links().attach(worker_end);
    joined(&edge, &mut runner).await;
    edge.everything();

    // The step and the edge's asking for the chunk it leads into come together.
    edge.send(walk(player(), 16.5)).await.unwrap();
    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    let ask = edge.asked();
    runner.step();
    assert_eq!(x_of(&runner, player()), Some(16.5));
    assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Asked);

    step_until(&mut runner, |runner| runner.region().player_count() == 0);
    let log = edge.everything();
    let departed = log.iter().position(|message| {
        matches!(
            message,
            WorkerToEdge::Outbox {
                entry: Durable::Departed { to: EAST, transfer, .. },
                ..
            } if transfer.pose.position.x == 16.5
        )
    });
    let elsewhere = WorkerToEdge::Elsewhere {
        chunk: BESIDE,
        ask,
        region: EAST,
    };
    let elsewhere = log.iter().position(|message| *message == elsewhere);
    assert!(departed.is_some() && departed < elsewhere, "{log:?}");
}

/// What a player does to a block of a chunk the region has no answer about is
/// passed on without a region named, which leaves it to the edge to know who serves
/// it the chunk; once the store has said who holds the chunk, with that region.
#[tokio::test]
async fn a_block_of_a_chunk_is_passed_on_with_its_holder_only_once_the_store_has_named_it() {
    let world = Divided::stripes();
    let (mut edge, worker_end) = in_process(256);
    let mut runner = world.runner(RegionId(0), 1);
    runner.links().attach(worker_end);
    joined(&edge, &mut runner).await;
    edge.send(walk(player(), 14.5)).await.unwrap();
    step(&mut runner);
    edge.everything();

    let passed_on = |message: WorkerToEdge| match message {
        WorkerToEdge::Outbox {
            entry: Durable::Remote { action, to },
            ..
        } => (action.sequence, to),
        other => panic!("expected an action passed on, got {other:?}"),
    };
    edge.send(dig_by(player(), 16, 1)).await.unwrap();
    assert_eq!(passed_on(step_for(&mut runner, &mut edge)), (1, None));
    // A click is no reason to ask whose the chunk is.
    assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Unknown);

    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    let ask = edge.asked();
    assert_eq!(
        answers(&mut runner, &mut edge, 1),
        [(BESIDE, ask, Told::Elsewhere(EAST))]
    );
    edge.send(dig_by(player(), 16, 2)).await.unwrap();
    assert_eq!(passed_on(step_for(&mut runner, &mut edge)), (2, Some(EAST)));
}

/// A player who arrives for a chunk the region believes another's is sent on, and
/// is on their way as one the region let go: if the edge that was to pass them on
/// starts anew, or stays away until the region forgets it, every link is told that
/// the entity is gone.
#[tokio::test]
async fn an_arrival_that_was_sent_on_and_then_dropped_is_reported_removed_to_every_link() {
    for stays_away in [false, true] {
        let world = Divided::stripes();
        let (mut edge, worker_end) = in_process(256);
        let (mut watcher, watcher_end) = in_process(256);
        let mut runner = world.runner(RegionId(0), 1).with_gone_after(20);
        runner.links().attach(worker_end);
        runner.links().attach(watcher_end);

        edge.send(asking_for(vec![BESIDE])).await.unwrap();
        assert_eq!(
            answers(&mut runner, &mut edge, 1),
            [(BESIDE, 1, Told::Elsewhere(EAST))]
        );
        let arriving = PlayerTransfer {
            pose: Pose::at(Vec3::new(16.5, -60.0, 0.5)),
            ..transfer(EntityId(77))
        };
        edge.send(EdgeToWorker::PlayerArrive {
            player: other_player(),
            transfer: arriving.clone(),
        })
        .await
        .unwrap();
        let sent_on = WorkerToEdge::Outbox {
            number: 1,
            entry: Durable::NotMine {
                what: Misdirected::Arrival {
                    player: other_player(),
                    transfer: arriving,
                },
                holder: EAST,
            },
        };
        assert_eq!(step_for(&mut runner, &mut edge), sent_on);
        assert_eq!(runner.region().player_count(), 0);

        // The edge does not pass the player on. The watcher is subscribed to
        // nothing and hears all the same that the entity is gone.
        let (end, anew_end) = link::in_process(256);
        let anew = TestEdge::silent(end, edge.edge, edge.start + 1);
        if stays_away {
            drop(edge);
        } else {
            anew.send(anew.hello(0, &[], &[])).await.unwrap();
            runner.links().attach(anew_end);
        }
        let removed = RegionEvent::EntityRemoved {
            entity: EntityId(77),
            chunk: BESIDE,
        };
        assert_eq!(events(step_for(&mut runner, &mut watcher)), [removed]);
        assert_eq!(runner.region().edge(anew.edge).is_none(), stays_away);
    }
}

/// The status has how many chunks the store has granted the region and where its
/// players are, for the coordinator to merge and split regions by.
#[tokio::test]
async fn the_status_counts_the_chunks_the_region_was_granted_and_says_where_its_players_are() {
    let world = Divided::gap();
    let (edge, worker_end) = in_process(256);
    let mut runner = world.runner(HOME, 1);
    runner.links().attach(worker_end);
    let status = runner.status();
    // The home chunk was granted to the home region when the world was made.
    assert_eq!(status.held.load(Ordering::Relaxed), 1);
    assert!(status.crowds().is_empty());

    joined(&edge, &mut runner).await;
    assert_eq!(status.crowds(), [(ORIGIN, 1)]);

    // A second player, and the first walks into a chunk that is nobody's: the
    // region takes it.
    edge.send(join(other_player(), "Jeb")).await.unwrap();
    edge.send(walk(player(), 16.5)).await.unwrap();
    step_until(&mut runner, |runner| {
        let region = runner.region();
        region.player(other_player()).is_some() && region.knowledge(BESIDE) == Knowledge::Held
    });
    assert_eq!(status.crowds(), [(ORIGIN, 1), (BESIDE, 1)]);
    assert_eq!(status.held.load(Ordering::Relaxed), 2);
    assert_eq!(runner.region().player_count(), 2);
}

/// What a region holds is the store's to say when the region is opened: the next
/// owner holds what the owner before was granted, and loads it without asking.
#[tokio::test]
async fn the_next_owner_of_a_region_holds_what_the_store_had_granted_it() {
    let world = Divided::gap();
    let (mut edge, worker_end) = in_process(256);
    let mut runner = world.runner(HOME, 1);
    runner.links().attach(worker_end);
    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    assert_eq!(
        answers(&mut runner, &mut edge, 1),
        [(BESIDE, 1, Told::Snapshot)]
    );

    // The next owner keeps a chunk for long, so that what it holds can be looked at.
    let hello = world.hello(HOME, 2);
    let (mut next, gate) = gated_as(&world.store, hello, settings(1000));
    drop(runner);
    assert_eq!(next.region().knowledge(ORIGIN), Knowledge::Held);
    assert_eq!(next.region().knowledge(BESIDE), Knowledge::Held);
    assert_eq!(
        next.region().knowledge(ChunkPos::new(2, 0)),
        Knowledge::Unknown
    );
    assert_eq!(next.status().held.load(Ordering::Relaxed), 2);

    let (mut edge, worker_end) = in_process(256);
    next.links().attach(worker_end);
    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    assert_eq!(
        answers(&mut next, &mut edge, 1),
        [(BESIDE, 1, Told::Snapshot)]
    );
    let asked = gate.asked();
    assert!(asked.contains(&Asked::Load(BESIDE)), "{asked:?}");
    assert!(
        asked.iter().all(|asked| !matches!(asked, Asked::Claim(_))),
        "{asked:?}"
    );
}

/// A chunk is given back only when nothing has used it for as many ticks as the
/// region is made with, so that a region whose edge is away for a moment does not
/// shed everything it holds.
#[tokio::test]
async fn a_chunk_is_given_back_only_after_nothing_has_used_it_for_a_while() {
    let world = Divided::gap();
    let (mut edge, worker_end) = in_process(256);
    let (mut runner, gate) = world.gated(HOME, settings(30));
    runner.links().attach(worker_end);
    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    assert_eq!(
        answers(&mut runner, &mut edge, 1),
        [(BESIDE, 1, Told::Snapshot)]
    );
    gate.asked();

    // The tick that takes the ticket back is the first in which nothing uses the
    // chunk.
    edge.send(done_with(vec![BESIDE])).await.unwrap();
    step(&mut runner);
    let unused_from = runner.region().tick_number();
    let returned = Asked::Return(vec![BESIDE]);
    step_until(&mut runner, |_| gate.asked().contains(&returned));
    assert_eq!(runner.region().tick_number(), unused_from + 30);
    assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Unknown);
}

/// What the store is asked after a tick is in a fixed order, and a chunk that is
/// given back has been saved before, with every change the region made to it.
#[tokio::test]
async fn after_a_tick_the_store_is_asked_in_a_fixed_order_and_a_chunk_is_saved_before_it_is_returned()
 {
    let world = Divided::gap();
    let (mut edge, worker_end) = in_process(256);
    let (mut runner, gate) = world.gated(HOME, settings(0));
    runner.links().attach(worker_end);
    let (loaded, claimed) = (ChunkPos::new(2, 0), ChunkPos::new(3, 0));

    // A player changes a block of a chunk the region was granted for the link.
    joined(&edge, &mut runner).await;
    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    edge.send(walk(player(), 14.5)).await.unwrap();
    answers(&mut runner, &mut edge, 2);
    edge.send(dig(16)).await.unwrap();
    step(&mut runner);
    let changed = runner.region().chunk(BESIDE).unwrap();
    assert_eq!(changed.get(0, -61, 0), Some(clustine_data::blocks::AIR));

    // The store's answer to a claim of another chunk is on its way.
    gate.hold_claims();
    edge.send(asking_for(vec![loaded])).await.unwrap();
    step(&mut runner);
    wait_for_kept_answers(&mut runner, &gate, GateControl::kept_claims, 1);
    gate.asked();

    // One tick with everything: the answer comes, so the chunk is asked of
    // storage; a player joins, which is a commit; the changed chunk is let go of
    // and given back; a third chunk is asked for; and it is time for a checkpoint.
    runner.checkpoint_interval = 1;
    edge.send(join(other_player(), "Jeb")).await.unwrap();
    edge.send(done_with(vec![BESIDE])).await.unwrap();
    edge.send(asking_for(vec![claimed])).await.unwrap();
    gate.release_claims();
    runner.step();
    let tick = runner.region().tick_number();
    assert_eq!(
        gate.asked(),
        [
            // Before the tick that drops the chunk.
            Asked::Save(BESIDE),
            Asked::Load(loaded),
            Asked::Commit(tick),
            Asked::Return(vec![BESIDE]),
            Asked::Claim(vec![claimed]),
            Asked::Checkpoint(tick),
        ]
    );
}

/// A region loads and saves only what it holds. If the store says otherwise, the
/// two disagree, and the runner stops as it does when the store is lost.
#[tokio::test]
async fn a_runner_that_is_told_it_does_not_hold_a_chunk_it_loads_gives_up() {
    let world = Divided::stripes();
    let (mut edge, worker_end) = in_process(256);
    // The region is told on opening that it was granted a chunk of the other stripe,
    // of which the store knows nothing: the test makes the disagreement up, as
    // nothing a region and a store do brings one about.
    let (handle, mut restored) = world.open(RegionId(0), 1);
    restored.held.push((BESIDE, 0));
    let mut runner = RegionRunner::restore(settings(0), handle, restored).unwrap();
    runner.links().attach(worker_end);
    let status = runner.status();

    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    step_until(&mut runner, |runner| runner.store_is_lost());
    assert_eq!(runner.ended(), Some(Ended::StoreLost));
    assert!(status.store_lost.load(Ordering::Relaxed));
    assert_eq!(status.ended(), Some(Ended::StoreLost));
    assert!(closed(&mut edge).await);
    let before = runner.region().tick_number();
    runner.step();
    assert_eq!(runner.region().tick_number(), before);
}

/// A chunk the region has asked for is asked until the store answers. A runner that
/// is releasing its region and still ticks takes the answer into its next tick
/// like any other; what it was granted is the next owner's.
#[tokio::test]
async fn an_answer_to_a_claim_that_comes_while_a_release_still_ticks_is_taken_into_the_next_tick() {
    let world = Divided::gap();
    let (mut edge, worker_end) = in_process(256);
    let (mut runner, gate) = world.gated(HOME, settings(0));
    runner.links().attach(worker_end);

    gate.hold_claims();
    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    step(&mut runner);
    assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Asked);
    wait_for_kept_answers(&mut runner, &gate, GateControl::kept_claims, 1);

    // The release waits for the store to have its first checkpoint, and ticks on.
    gate.hold_flushes();
    runner.begin_release();
    gate.release_claims();
    runner.step();
    assert_eq!(runner.phase, Phase::Preparing);
    assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Held);
    assert_eq!(
        answers(&mut runner, &mut edge, 1),
        [(BESIDE, 1, Told::Snapshot)]
    );
    assert_eq!(runner.phase, Phase::Preparing);

    gate.release_flushes();
    assert_eq!(released(&mut runner), Ended::Released);
    let (_handle, restored) = world.open(HOME, 2);
    let held: Vec<_> = restored.held.iter().map(|(chunk, _)| *chunk).collect();
    assert_eq!(held, [ORIGIN, BESIDE]);
}

/// The region asks storage for a chunk, drops the request with the chunk's last
/// ticket and gives the chunk back; another region changes it; the region is
/// granted it again and asks for it again. What storage read for the first request
/// is not the chunk any more, and is not taken for the answer to the second.
#[tokio::test]
async fn what_storage_read_for_a_request_the_region_dropped_is_not_taken_for_a_later_one() {
    let world = Divided::gap();
    let (mut edge, worker_end) = in_process(256);
    let (mut runner, gate) = world.gated(HOME, settings(0));
    runner.links().attach(worker_end);
    let (neighbour, _) = world.open(EAST, 1);
    let held = |runner: &RegionRunner| runner.region().knowledge(BESIDE) == Knowledge::Held;

    // The first request, whose answer is on its way for a long time.
    gate.hold_loads();
    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    step_until(&mut runner, held);
    wait_for_kept_answers(&mut runner, &gate, GateControl::kept_loads, 1);
    edge.send(done_with(vec![BESIDE])).await.unwrap();

    // The neighbour takes the chunk when it is free, changes it and gives it back.
    claim_until_granted(&mut runner, &neighbour, BESIDE);
    neighbour.request(StoreRequest::Load { position: BESIDE });
    let mut chunk = loop {
        match neighbour.try_reply() {
            Some(StoreReply::Loaded { chunk, .. }) => break chunk,
            Some(other) => panic!("the store answered a load with {other:?}"),
            None => thread::sleep(Duration::from_millis(1)),
        }
    };
    let air = clustine_data::blocks::AIR;
    assert_ne!(chunk.get(5, -61, 5), Some(air));
    chunk.set(5, -61, 5, air);
    neighbour.request(StoreRequest::Save {
        position: BESIDE,
        tick: 0,
        chunk,
        parts: clustine_rpc::Parts::ALL,
    });
    neighbour.request(StoreRequest::Return {
        chunks: vec![BESIDE],
        unreadable: Vec::new(),
    });
    neighbour.flush();

    // The second request. Both answers come, the older one first.
    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    step_until(&mut runner, held);
    wait_for_kept_answers(&mut runner, &gate, GateControl::kept_loads, 2);
    gate.release_loads();
    let snapshot = step_for(&mut runner, &mut edge);
    let WorkerToEdge::ChunkSnapshot {
        position, chunk, ..
    } = snapshot
    else {
        panic!("expected a snapshot, got {snapshot:?}");
    };
    assert_eq!(position, BESIDE);
    assert_eq!(chunk.get(5, -61, 5), Some(air));
    assert_eq!(runner.region().chunk(BESIDE), Some(&chunk));
}

/// That a chunk is elsewhere is part of what a tick produced, like everything else
/// a link is told: it is not on the link before the commit of that tick and of
/// those before it is confirmed, and a runner that has lost the region by then
/// does not say it at all.
#[tokio::test]
async fn what_a_link_is_told_of_a_chunk_waits_for_the_commits_like_everything_else() {
    for confirmed in [true, false] {
        let world = Divided::stripes();
        let (mut edge, worker_end) = in_process(256);
        let (mut runner, gate) = world.gated(RegionId(0), settings(0));
        runner.links().attach(worker_end);
        step(&mut runner);
        // Two players join. The first has entered the world. Of the second the
        // region has the store's answer to the commit of the join, and not yet its
        // word that they may enter, which comes behind that answer.
        edge.send(join(player(), "Notch")).await.unwrap();
        step_in(&mut runner);
        gate.hold_stays();
        edge.send(join(other_player(), "Jeb")).await.unwrap();
        step(&mut runner);
        edge.everything();

        // A tick with a commit, in which the region asks about the chunk.
        gate.hold();
        edge.send(walk(player(), 3.0)).await.unwrap();
        edge.send(asking_for(vec![BESIDE])).await.unwrap();
        runner.step();
        assert_eq!(x_of(&runner, player()), Some(3.0));
        // The store's answer is there, and the tick that takes it has a commit of
        // its own: the second player enters the world in it.
        gate.release_stays();
        for _ in 0..20_000 {
            assert!(runner.take_replies());
            if !runner.inputs.foreign.is_empty() && !runner.inputs.entered.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        // And something of the edge's to apply, so that it has progress to tell.
        edge.send(walk(player(), 4.0)).await.unwrap();
        runner.step();
        assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Foreign(EAST));
        assert_eq!(runner.region().player_count(), 2);
        wait_for_kept(&mut runner, &gate, 2);
        let early = edge.everything();
        assert!(early.is_empty(), "{early:?}");

        let entered = |log: &[WorkerToEdge], who: PlayerId| {
            log.iter().position(|message| {
                    matches!(message, WorkerToEdge::ToPlayer { player, .. } if *player == who)
                })
        };
        let stepped = |log: &[WorkerToEdge]| {
            log.iter()
                .position(|message| matches!(message, WorkerToEdge::TickDelta { .. }))
        };
        let elsewhere = WorkerToEdge::Elsewhere {
            chunk: BESIDE,
            ask: 1,
            region: EAST,
        };
        if confirmed {
            gate.release();
            step(&mut runner);
            let log = edge.everything();
            let told = log.iter().position(|message| *message == elsewhere);
            let progress = log
                .iter()
                .rposition(|message| matches!(message, WorkerToEdge::Progress { .. }));
            // Behind everything of the tick before, and in its own tick behind who
            // entered the world and before the progress.
            let order = [stepped(&log), entered(&log, other_player()), told, progress];
            assert!(order.iter().all(Option::is_some), "{log:?}");
            assert!(order.is_sorted(), "{order:?} in {log:?}");
        } else {
            // Another owner takes the region.
            let _next = world.open(RegionId(0), 2);
            step_until(&mut runner, |runner| runner.store_is_lost());
            let mut log = Vec::new();
            while let Some(message) = edge.end.recv().await {
                log.push(message);
            }
            assert!(log.is_empty(), "{log:?}");
        }
    }
}

/// What a region changed in a chunk is in the store when it gives the chunk back:
/// whoever is granted the chunk next loads it as the region left it.
#[tokio::test]
async fn a_chunk_that_is_given_back_is_loaded_by_the_next_holder_as_the_region_left_it() {
    let world = Divided::gap();
    let (mut edge, worker_end) = in_process(256);
    let mut runner = world.runner(HOME, 1);
    runner.links().attach(worker_end);
    let (neighbour, _) = world.open(EAST, 1);
    let air = clustine_data::blocks::AIR;

    joined(&edge, &mut runner).await;
    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    edge.send(walk(player(), 14.5)).await.unwrap();
    answers(&mut runner, &mut edge, 2);
    edge.send(dig(16)).await.unwrap();
    step(&mut runner);
    let changed = runner.region().chunk(BESIDE).unwrap().clone();
    assert_eq!(changed.get(0, -61, 0), Some(air));

    edge.send(done_with(vec![BESIDE])).await.unwrap();
    claim_until_granted(&mut runner, &neighbour, BESIDE);
    neighbour.request(StoreRequest::Load { position: BESIDE });
    let loaded = loop {
        match neighbour.try_reply() {
            Some(StoreReply::Loaded { chunk, .. }) => break chunk,
            Some(other @ (StoreReply::Unreadable { .. } | StoreReply::Withdrawn { .. })) => {
                panic!("the store answered a load with {other:?}")
            }
            // The two regions now hold chunks side by side, and the store tells the
            // neighbour what goes with that (to couple, how far its marks go) when it
            // will, before or after it answers the load: no answers to it.
            Some(_) => {}
            None => thread::sleep(Duration::from_millis(1)),
        }
    };
    assert_eq!(loaded, changed);
}

/// A chunk has one ticket for each link that is subscribed to it, of the kind of
/// that link's subscription: it stays loaded and the region's until the last of
/// them has let go.
#[tokio::test]
async fn a_chunk_that_two_links_are_subscribed_to_stays_until_both_have_let_go() {
    let world = Divided::gap();
    let (mut viewer, viewer_end) = in_process(256);
    let (mut guest, guest_end) = in_process(256);
    let mut runner = world.runner(HOME, 1);
    runner.links().attach(viewer_end);
    runner.links().attach(guest_end);
    let (neighbour, _) = world.open(EAST, 1);

    viewer.send(asking_for(vec![BESIDE])).await.unwrap();
    assert_eq!(
        answers(&mut runner, &mut viewer, 1),
        [(BESIDE, 1, Told::Snapshot)]
    );
    // The guest is served what the region holds for the viewer.
    guest.send(asking_as_guest_for(vec![BESIDE])).await.unwrap();
    assert_eq!(
        answers(&mut runner, &mut guest, 1),
        [(BESIDE, 1, Told::Snapshot)]
    );

    // The viewer lets go. The guest's subscription keeps the chunk, and goes on
    // being told what happens in it.
    viewer.send(done_with(vec![BESIDE])).await.unwrap();
    let discard = EdgeToWorker::Discard {
        player: player(),
        entity: EntityId(9),
        hops: 1,
        chunk: BESIDE,
    };
    guest.send(discard).await.unwrap();
    let told: Vec<_> = told_until_marked(&mut runner, &mut guest, 15)
        .into_iter()
        .map(events)
        .collect();
    let removed = RegionEvent::EntityRemoved {
        entity: EntityId(9),
        chunk: BESIDE,
    };
    assert_eq!(told, [vec![removed]]);
    assert!(runner.region().chunk(BESIDE).is_some());
    assert_eq!(claim(&neighbour, &[BESIDE]), (vec![], vec![(BESIDE, HOME)]));

    guest.send(done_with(vec![BESIDE])).await.unwrap();
    claim_until_granted(&mut runner, &neighbour, BESIDE);
    assert!(runner.region().chunk(BESIDE).is_none());
}

// -----------------------------------------------------------------------------------
// What waits for its chunk, and whom a region says it has: sections 3.6 and 3.7 of
// `docs/adr/0014-merging-and-splitting.md`.
// -----------------------------------------------------------------------------------

/// The first chunk of the western stripe west of the one players enter the world
/// in, which a player who stands there can reach into.
const WEST_OF_HOME: ChunkPos = ChunkPos::new(-1, 0);

/// Whether the block that [`dig`] breaks with `x` is broken, if its chunk is loaded.
fn dug(runner: &RegionRunner, x: i32) -> Option<bool> {
    let position = BlockPos::new(x, -61, 0);
    let (in_x, in_z) = position.in_chunk();
    let chunk = runner.region().chunk(position.chunk())?;
    Some(chunk.get(in_x, position.y, in_z) == Some(clustine_data::blocks::AIR))
}

/// Reads what `edge` is sent up to the word of a block that changed, however long
/// its link takes over it, and returns whether the snapshot of `chunk` came before.
async fn shown_before_it_changed(edge: &mut TestEdge, chunk: ChunkPos) -> bool {
    let mut shown = false;
    loop {
        match next(edge).await {
            WorkerToEdge::ChunkSnapshot { position, .. } => shown |= position == chunk,
            WorkerToEdge::TickDelta { events, .. } => {
                let changed =
                    |event: &RegionEvent| matches!(event, RegionEvent::BlockChanged { .. });
                if events.iter().any(changed) {
                    return shown;
                }
            }
            _ => {}
        }
    }
}

/// A chunk of the region's own stripe that nothing has asked about is the region's
/// to serve, though it does not know so yet. A dig that comes right behind the
/// subscription to it is judged when the chunk is there, and not passed on without
/// a region as for a chunk nobody is known to hold; what the link sent behind the
/// dig waits with it.
#[tokio::test]
async fn a_dig_behind_a_subscription_waits_for_a_chunk_of_the_regions_own_area() {
    // Over a direct link, on which all three are there when the runner next looks.
    let (mut edge, worker_end) = in_process(256);
    let mut runner = west(worker_end);
    joined(&edge, &mut runner).await;
    settle(&mut runner);
    edge.everything();
    assert_eq!(runner.region().knowledge(WEST_OF_HOME), Knowledge::Unknown);
    let applied = runner.region().edge(edge.edge).unwrap().applied;

    edge.send(asking_for(vec![WEST_OF_HOME])).await.unwrap();
    edge.send(dig(-1)).await.unwrap();
    edge.send(walk(player(), 3.0)).await.unwrap();
    step_until(&mut runner, |runner| {
        runner.links[&LinkId(0)].held.len() == 2
    });
    // Neither is taken until the chunk is claimed, read and shown.
    let mut waited = 0;
    step_until(&mut runner, |runner| {
        let loaded = runner.region().chunk(WEST_OF_HOME).is_some();
        if !loaded {
            waited += 1;
            assert_eq!(runner.links[&LinkId(0)].held.len(), 2);
            assert_eq!(runner.region().edge(edge.edge).unwrap().applied, applied);
            assert_eq!(x_of(runner, player()), Some(SPAWN.x));
        }
        loaded
    });
    assert!(waited > 0);
    step_until(&mut runner, |runner| x_of(runner, player()) == Some(3.0));
    assert_eq!(dug(&runner, -1), Some(true));
    assert_eq!(
        runner.region().edge(edge.edge).unwrap().applied,
        applied + 2
    );
    assert!(runner.links[&LinkId(0)].held.is_empty());
    // Nothing was passed on to anyone.
    assert_eq!(outbox(&runner, &edge), (0, vec![]));

    // The edge is shown the chunk before it is told of the block.
    settle(&mut runner);
    assert!(shown_before_it_changed(&mut edge, WEST_OF_HOME).await);
}

/// The same for a chunk the region holds and has not loaded: one it was granted
/// for a viewer who has let go of it since.
#[tokio::test]
async fn a_dig_behind_a_subscription_waits_for_a_chunk_the_region_holds_and_has_not_loaded() {
    let world = Divided::gap();
    let (mut edge, worker_end) = in_process(256);
    let (mut runner, _) = world.gated(HOME, settings(40));
    runner.links().attach(worker_end);
    joined(&edge, &mut runner).await;
    edge.everything();
    edge.send(walk(player(), 14.5)).await.unwrap();
    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    assert_eq!(
        answers(&mut runner, &mut edge, 1),
        [(BESIDE, edge.asked(), Told::Snapshot)]
    );
    edge.send(done_with(vec![BESIDE])).await.unwrap();
    step_until(&mut runner, |runner| {
        runner.region().chunk(BESIDE).is_none()
    });
    assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Held);
    settle(&mut runner);
    edge.everything();
    let applied = runner.region().edge(edge.edge).unwrap().applied;

    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    edge.send(dig(16)).await.unwrap();
    edge.send(walk(player(), 13.5)).await.unwrap();
    step(&mut runner);
    assert_eq!(runner.links[&LinkId(0)].held.len(), 2);
    assert_eq!(runner.region().edge(edge.edge).unwrap().applied, applied);
    step_until(&mut runner, |runner| x_of(runner, player()) == Some(13.5));
    assert_eq!(dug(&runner, 16), Some(true));
    assert_eq!(outbox(&runner, &edge), (0, vec![]));
    settle(&mut runner);
    assert!(shown_before_it_changed(&mut edge, BESIDE).await);
}

/// The hold is narrow. A chunk of another region's stripe is not this region's to
/// serve, whatever it knows of it: a dig into it is judged in the tick that takes
/// it and goes on without a region named, and a move behind it is applied in that
/// tick. So is a move behind a subscription that waits, and a dig into a chunk the
/// link has not asked for.
#[tokio::test]
async fn what_a_region_is_not_about_to_serve_holds_nothing() {
    let (mut edge, worker_end) = in_process(256);
    let mut runner = west(worker_end);
    joined(&edge, &mut runner).await;
    edge.send(walk(player(), 14.5)).await.unwrap();
    step(&mut runner);
    settle(&mut runner);
    edge.everything();

    // Another region's chunk, asked for and not yet answered.
    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    edge.send(dig_by(player(), 16, 3)).await.unwrap();
    edge.send(walk(player(), 13.5)).await.unwrap();
    step(&mut runner);
    assert!(runner.links[&LinkId(0)].held.is_empty());
    assert_eq!(x_of(&runner, player()), Some(13.5));
    let state = runner.region().edge(edge.edge).unwrap();
    let entries: Vec<_> = state.outbox.values().collect();
    assert!(
        matches!(entries[..], [Durable::Remote { to: None, .. }]),
        "{entries:?}"
    );

    // The region's own chunk: a move behind the subscription that waits for it,
    // and on another link a dig into it without a subscription.
    let (other, other_end) = in_process(256);
    runner.links().attach(other_end);
    other.send(join(other_player(), "Jeb")).await.unwrap();
    step_in(&mut runner);
    edge.send(asking_for(vec![WEST_OF_HOME])).await.unwrap();
    edge.send(walk(player(), 3.0)).await.unwrap();
    other
        .send(of_entity(EntityId(2), dig_by(other_player(), -1, 1)))
        .await
        .unwrap();
    step(&mut runner);
    assert!(runner.region().chunk(WEST_OF_HOME).is_none());
    assert_eq!(x_of(&runner, player()), Some(3.0));
    let applied = |edge: &TestEdge| runner.region().edge(edge.edge).unwrap().applied;
    assert_eq!(applied(&other), other.sent.load(Ordering::Relaxed));
    assert_eq!(applied(&edge), edge.sent.load(Ordering::Relaxed));
    assert!(runner.links.values().all(|link| link.held.is_empty()));
}

/// What a player of another region does to a block waits like a player's own
/// action: a remote action behind a guest's subscription to a chunk of the
/// region's own area is taken when the chunk is there, and done.
#[tokio::test]
async fn a_remote_action_behind_a_guests_subscription_waits_for_the_chunk() {
    let (edge, worker_end) = in_process(256);
    let mut runner = west(worker_end);
    let action = RemoteAction {
        player: player(),
        entity: EntityId(4040),
        sequence: 4,
        step: RemoteStep::Break {
            position: BlockPos::new(-1, -61, 0),
        },
    };

    edge.send(asking_as_guest_for(vec![WEST_OF_HOME]))
        .await
        .unwrap();
    edge.send(EdgeToWorker::Remote(action)).await.unwrap();
    step(&mut runner);
    assert_eq!(runner.links[&LinkId(0)].held.len(), 1);
    assert_eq!(runner.region().edge(edge.edge).unwrap().applied, 0);

    step_until(&mut runner, |runner| {
        runner.region().edge(edge.edge).unwrap().applied == 1
    });
    assert_eq!(dug(&runner, -1), Some(true));
    let state = runner.region().edge(edge.edge).unwrap();
    let entries: Vec<_> = state.outbox.values().collect();
    let done = Durable::RemoteDone {
        player: player(),
        entity: EntityId(4040),
        sequence: 4,
        shown: Vec::new(),
    };
    assert_eq!(entries, [&done]);
}

/// A chunk the store cannot read is waited for without end, and holds nothing: a
/// dig into it is taken at once.
#[tokio::test]
async fn a_chunk_that_cannot_be_read_holds_no_dig() {
    let (edge, worker_end) = in_process(256);
    let mut runner = west(worker_end);
    joined(&edge, &mut runner).await;
    // As the store's word that it cannot read the chunk leaves it.
    runner.unreadable.insert(WEST_OF_HOME);

    edge.send(asking_for(vec![WEST_OF_HOME])).await.unwrap();
    edge.send(dig(-1)).await.unwrap();
    step(&mut runner);
    assert!(runner.links[&LinkId(0)].held.is_empty());
    let applied = runner.region().edge(edge.edge).unwrap().applied;
    assert_eq!(applied, edge.sent.load(Ordering::Relaxed));
}

/// The presence answers among `told`: the players in the order they were answered
/// for, each with their entity if they are present.
fn presences(told: &[WorkerToEdge]) -> Vec<(PlayerId, Option<EntityId>)> {
    let answer = |message: &WorkerToEdge| match message {
        WorkerToEdge::Presence { player, answer } => {
            let entity = match answer {
                Presence::Present { entity, .. } => Some(*entity),
                Presence::Absent => None,
                // Only while a stay waits for the store's answer. The tests that
                // read presences here say hello when their players have entered;
                // those of `stays` have a reading of their own for the rest.
                Presence::Entering { .. } => panic!("no region here says {answer:?}"),
            };
            Some((*player, entity))
        }
        _ => None,
    };
    told.iter().filter_map(answer).collect()
}

/// A region says whom it has for an edge at every hello, named or not: an answer
/// for each player of the hello, in the hello's order, and then one for every
/// other stay of the edge, in ascending order. The welcome counts them.
#[tokio::test]
async fn a_hello_is_answered_with_every_stay_the_region_has_for_the_edge() {
    let (first, first_end) = in_process(256);
    let (bystander, bystander_end) = in_process(256);
    let mut runner = runner(first_end);
    runner.links().attach(bystander_end);
    // In this order, so that the entities do not ascend with the players.
    first.send(join(other_player(), "Jeb")).await.unwrap();
    first.send(join(player(), "Notch")).await.unwrap();
    // Somebody else's player is nobody's business but that edge's.
    bystander
        .send(join(third_player(), "Dinnerbone"))
        .await
        .unwrap();
    step_in(&mut runner);
    let applied = first.sent.load(Ordering::Relaxed);

    // A hello that names neither.
    let (end, worker_end) = link::in_process(256);
    let mut again = first.again(end, &runner);
    runner.links().attach(worker_end);
    again.send(again.hello(0, &[], &[])).await.unwrap();
    step(&mut runner);
    let told = again.everything();
    let welcome = Welcome::Resumed {
        entries: 0,
        presences: 2,
        applied,
    };
    assert_eq!(again.welcomed, Some(welcome));
    assert_eq!(
        presences(&told),
        [
            (player(), Some(EntityId(2))),
            (other_player(), Some(EntityId(1)))
        ]
    );
    assert!(
        matches!(
            told[1..3],
            [WorkerToEdge::Presence { .. }, WorkerToEdge::Presence { .. }]
        ),
        "{told:?}"
    );

    // One that names one of them, and somebody who is not there, or not this
    // edge's.
    for stranger in [PlayerId(Uuid::from_u128(9)), third_player()] {
        let (end, worker_end) = link::in_process(256);
        let mut again = first.again(end, &runner);
        runner.links().attach(worker_end);
        let named = [other_player(), stranger];
        again.send(again.hello(0, &named, &[])).await.unwrap();
        step(&mut runner);
        let told = again.everything();
        let welcome = Welcome::Resumed {
            entries: 0,
            presences: 3,
            applied,
        };
        assert_eq!(again.welcomed, Some(welcome));
        assert_eq!(
            presences(&told),
            [
                (other_player(), Some(EntityId(1))),
                (stranger, None),
                (player(), Some(EntityId(2)))
            ]
        );
    }

    // An edge the region does not know is answered for the names of its hello, and
    // a start that resets the edge likewise: whoever is there goes with that tick.
    let (end, worker_end) = link::in_process(256);
    let mut later = TestEdge::silent(end, first.edge, first.start + 1);
    runner.links().attach(worker_end);
    later.send(later.hello(0, &[player()], &[])).await.unwrap();
    step(&mut runner);
    let told = later.everything();
    let welcome = Welcome::Unknown {
        since: runner.region().tick_number(),
        entries: 0,
        presences: 1,
        applied: 0,
    };
    assert_eq!(later.welcomed, Some(welcome));
    assert_eq!(presences(&told), [(player(), None)]);
    assert_eq!(runner.region().player_count(), 1);
}

/// A leave that names the entity a presence answer showed ends that stay, and one
/// that names another entity ends nothing.
#[tokio::test]
async fn a_leave_ends_the_stay_a_presence_answer_showed_and_no_other() {
    let (first, first_end) = in_process(256);
    let mut runner = runner(first_end);
    first.send(join(player(), "Notch")).await.unwrap();
    step_in(&mut runner);

    let (end, worker_end) = link::in_process(256);
    let mut again = first.again(end, &runner);
    runner.links().attach(worker_end);
    again.send(again.hello(0, &[], &[])).await.unwrap();
    step(&mut runner);
    let shown = presences(&again.everything());
    let [(shown_player, Some(entity))] = shown[..] else {
        panic!("{shown:?}");
    };
    assert_eq!(shown_player, player());

    let leave = |entity| EdgeToWorker::PlayerLeave {
        player: player(),
        entity: Some(entity),
        attempt: None,
    };
    again.send(leave(EntityId(entity.0 + 1))).await.unwrap();
    step(&mut runner);
    assert_eq!(runner.region().player_count(), 1);
    again.send(leave(entity)).await.unwrap();
    step(&mut runner);
    assert_eq!(runner.region().player_count(), 0);
}

/// An arrival that takes the place of an earlier stay of its player is a player
/// who came in from another region, and counted as one; the arrival of the very
/// stay the region has is not.
#[tokio::test]
async fn an_arrival_that_replaces_an_earlier_stay_is_counted_and_the_same_stay_is_not() {
    let (edge, worker_end) = in_process(256);
    let mut runner = runner(worker_end);
    let status = runner.status();
    edge.send(join(player(), "Notch")).await.unwrap();
    step_in(&mut runner);
    assert_eq!(runner.region().player(player()).unwrap().0, EntityId(1));

    let arrive = |entity| EdgeToWorker::PlayerArrive {
        player: player(),
        transfer: transfer(EntityId(entity)),
    };
    edge.send(arrive(50)).await.unwrap();
    step(&mut runner);
    assert_eq!(runner.region().player(player()).unwrap().0, EntityId(50));
    assert_eq!(status.arrivals.load(Ordering::Relaxed), 1);

    edge.send(arrive(50)).await.unwrap();
    step(&mut runner);
    assert_eq!(status.arrivals.load(Ordering::Relaxed), 1);
}

// -----------------------------------------------------------------------------------
// Merging and splitting: sections 3.1 to 3.5 and 3.8 of
// `docs/adr/0014-merging-and-splitting.md`, and the builder's own tests of its
// section 10, which need the gate before the store.
// -----------------------------------------------------------------------------------

/// The chunk two to the west of the one players enter the world in.
const FAR_WEST: ChunkPos = ChunkPos::new(-2, 0);

/// A chunk of the eastern stripe, away from the line.
const FAR_EAST: ChunkPos = ChunkPos::new(2, 0);

/// A call for the outcome of a merge or a split, and where the outcome shows.
fn outcome() -> (Box<dyn FnOnce(Reshaped) + Send>, Receiver<Reshaped>) {
    let (said, outcome) = mpsc::channel();
    let done = move |reshaped| {
        // Whoever asked may have stopped looking.
        let _ = said.send(reshaped);
    };
    (Box::new(done), outcome)
}

/// Steps `runner` until the merge or the split it was told to make has an outcome.
fn reshaped(runner: &mut RegionRunner, outcome: &Receiver<Reshaped>) -> Reshaped {
    for _ in 0..20_000 {
        if let Ok(outcome) = outcome.try_recv() {
            return outcome;
        }
        runner.step();
        thread::sleep(Duration::from_millis(1));
    }
    panic!("the merge or the split had no outcome");
}

/// Steps `runner` until it is at `stage` of what it was told to do.
fn step_to(runner: &mut RegionRunner, stage: Stage) {
    for _ in 0..20_000 {
        if runner.stage() == Some(stage) {
            return;
        }
        runner.step();
        thread::sleep(Duration::from_millis(1));
    }
    panic!("the runner never came to {stage:?}");
}

/// What the runner asked of the store since this was last looked at, without the
/// commits of its ticks.
fn asked_beside_commits(gate: &GateControl) -> Vec<Asked> {
    let asked = gate.asked().into_iter();
    asked
        .filter(|asked| !matches!(asked, Asked::Commit(_)))
        .collect()
}

/// A gate before `inner`, for a runner that is not made by opening a region.
fn gate_before(inner: StoreHandle) -> (Box<dyn RegionStore>, Arc<GateControl>) {
    let control = Arc::new(GateControl::default());
    let gate = Gate {
        inner,
        control: Arc::clone(&control),
    };
    (Box::new(gate), control)
}

/// The eastern stripe as a test has it that plays the worker that is told to
/// absorb it: opened with the epoch 2, read, and kept open.
struct East {
    /// Kept open until the merge has an outcome: the store declines a merge whose
    /// absorbed region has no owner with the epoch named.
    handle: StoreHandle,
    state: RegionState,
    /// An edge that only this region knew, which has [`third_player`] here.
    stranger: EdgeId,
}

impl East {
    fn absorb(&self) -> Reshape {
        Reshape::Absorb {
            absorbed: EAST,
            absorbed_epoch: 2,
            state: self.state.clone(),
        }
    }
}

/// Someone who arrives in [`FAR_EAST`] with `entity`.
fn arriving_in_the_east(player: PlayerId, entity: u64) -> EdgeToWorker {
    EdgeToWorker::PlayerArrive {
        player,
        transfer: PlayerTransfer {
            pose: Pose::at(Vec3::new(40.5, -60.0, 0.5)),
            ..transfer(EntityId(entity))
        },
    }
}

/// Runs the eastern stripe of `world` with a runner of its own, releases it and
/// opens it again as whoever absorbs it does. While it ran, [`other_player`] of
/// `edge` arrived there, and something they did to a block west of the line was
/// passed on, which left an entry in the region's outbox for the edge that nobody
/// has confirmed; and [`third_player`] arrived through an edge that no other
/// region knows.
async fn released_east(world: &Divided, edge: &TestEdge) -> East {
    let mut east = world.runner(EAST, 1);
    let (end, worker_end) = link::in_process(256);
    let there = TestEdge::silent(end, edge.edge, edge.start);
    east.links().attach(worker_end);
    let (stranger, stranger_end) = in_process(256);
    east.links().attach(stranger_end);

    there.send(there.hello(0, &[], &[])).await.unwrap();
    there
        .send(arriving_in_the_east(other_player(), 40))
        .await
        .unwrap();
    let beyond = RemoteAction {
        player: other_player(),
        entity: EntityId(4040),
        sequence: 3,
        step: RemoteStep::Break {
            position: BlockPos::new(3, -61, 0),
        },
    };
    there.send(EdgeToWorker::Remote(beyond)).await.unwrap();
    stranger
        .send(arriving_in_the_east(third_player(), 41))
        .await
        .unwrap();
    step_until(&mut east, |runner| {
        runner.region().knowledge(FAR_EAST) == Knowledge::Held
    });
    assert_eq!(east.region().player_count(), 2);
    assert_eq!(outbox(&east, &there), (1, vec![1]));
    east.begin_release();
    assert_eq!(released(&mut east), Ended::Released);

    let (handle, restored) = world.open(EAST, 2);
    let block = restored.entity_ids;
    let state = absorbable(&handle, restored).unwrap();
    assert_eq!(state, with_keys(east.region().state(), block));
    East {
        handle,
        state,
        stranger: stranger.edge,
    }
}

/// The eastern stripe of `world` as it is when it has never run, opened and read
/// as whoever absorbs it does.
fn untouched_east(world: &Divided) -> East {
    let (handle, restored) = world.open(EAST, 2);
    let state = absorbable(&handle, restored).unwrap();
    assert_eq!(state.tick, 0);
    East {
        handle,
        state,
        stranger: EdgeId::from_name("nobody"),
    }
}

/// A link of the edge of `before` that says hello to `runner` as an edge does that
/// had a link to the region before a merge or a split closed it: it names
/// `players` and `chunks`, and has seen nothing of the outbox.
async fn linked_again(
    runner: &RegionRunner,
    before: &TestEdge,
    players: &[PlayerId],
    chunks: &[ChunkPos],
) -> TestEdge {
    let (end, worker_end) = link::in_process(256);
    let again = before.again(end, runner);
    runner.links().attach(worker_end);
    again.send(again.hello(0, players, chunks)).await.unwrap();
    again
}

/// The entries of the outbox among `told`, with their numbers.
fn entries(told: &[WorkerToEdge]) -> Vec<(u64, Durable)> {
    let entry = |message: &WorkerToEdge| match message {
        WorkerToEdge::Outbox { number, entry } => Some((*number, entry.clone())),
        _ => None,
    };
    told.iter().filter_map(entry).collect()
}

/// The chunks `told` has snapshots of, in the order they came.
fn shown(told: &[WorkerToEdge]) -> Vec<ChunkPos> {
    let position = |message: &WorkerToEdge| match message {
        WorkerToEdge::ChunkSnapshot { position, .. } => Some(*position),
        _ => None,
    };
    told.iter().filter_map(position).collect()
}

/// A merge is one tick of the survivor in which nothing else happens, and it is
/// taken when the store has it. Then the survivor has the players and the entries
/// of both regions and is pinned to both areas, has closed its links without a
/// word, and begins as a restored region does: it believes nothing of any chunk,
/// and answers the next hello with what the merge made among the welcome's
/// entries and with every stay it has for the edge. The chunks it had loaded are
/// served again without the store, and the resume is three ticks.
#[tokio::test]
async fn a_region_that_absorbs_another_closes_its_links_and_begins_anew_with_both_states() {
    for on_disk_too in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let world = match on_disk_too {
            true => Divided::stripes_in(directory.path()),
            false => Divided::stripes(),
        };
        let (mut edge, worker_end) = in_process(256);
        let (mut west, gate) = world.gated(RegionId(0), settings(0));
        west.links().attach(worker_end);
        look_east(&mut west, &mut edge);
        joined(&edge, &mut west).await;
        let east = released_east(&world, &edge).await;
        step(&mut west);
        edge.everything();
        gate.asked();
        let before = west.region().state();
        let chunk = west.region().chunk(ORIGIN).unwrap().clone();
        assert_eq!(west.region().knowledge(BESIDE), Knowledge::Foreign(EAST));

        let (done, outcome) = outcome();
        west.reshape(east.absorb(), done);
        assert_eq!(west.stage(), Some(Stage::Preparing));
        let absorbed = Reshaped::Absorbed { absorbed: EAST };
        assert_eq!(reshaped(&mut west, &outcome), absorbed);
        assert_eq!((west.stage(), west.ended()), (None, None));
        drop(east.handle);

        // What it cost at the store: a checkpoint while the region ticks, one when
        // it has stopped, and the merge as the tick after its last.
        let merged = west.region().state();
        let tick = merged.tick;
        assert_eq!(
            asked_beside_commits(&gate),
            [
                Asked::Checkpoint(before.tick),
                Asked::Flush,
                Asked::Checkpoint(tick - 1),
                Asked::Flush,
                Asked::Absorb(tick),
            ]
        );
        assert_eq!(west.status().tick.load(Ordering::Relaxed), tick);
        assert_eq!(west.status().players.load(Ordering::Relaxed), 3);
        assert_eq!(west.status().chunks.load(Ordering::Relaxed), 0);

        // Both regions' players, and the store's list has one region of the
        // overworld, beside the homes of the other dimensions.
        let players: Vec<_> = merged.players.keys().copied().collect();
        assert_eq!(players, [player(), other_player(), third_player()]);
        let list = world.store.regions().unwrap();
        assert_eq!(list.absorbed, [(EAST, RegionId(0))]);
        assert_eq!(ids(&list), [RegionId(0), NETHER_HOME, END_HOME]);
        assert_eq!(list.regions[0].pinned.len(), 2);

        // The links are closed, and nothing was said on them of the merge.
        assert_eq!(edge.everything(), []);
        assert!(closed(&mut edge).await);
        assert!(west.links.is_empty());
        // As after a restore: nothing is loaded, nothing believed, and the areas
        // that came with the merge are the region's.
        assert_eq!(west.region().loaded_chunk_count(), 0);
        assert_eq!(west.region().knowledge(BESIDE), Knowledge::Unknown);
        assert!(west.region().pins(BESIDE));
        assert_eq!(west.region().knowledge(ORIGIN), Knowledge::Held);
        assert_eq!(west.committed, tick);
        assert!(west.pending.is_empty() && west.unsaved.is_empty() && west.loads.is_empty());

        // The edge is back, names what it had here, and sends what its player did
        // meanwhile behind its hello. Three ticks: the hello and the tickets, the
        // chunks from memory and their snapshots, and what was held.
        let mut again = linked_again(&west, &edge, &[player()], &[ORIGIN]).await;
        again.send(walk(player(), 3.0)).await.unwrap();
        for _ in 0..3 {
            west.step();
        }
        assert_eq!(west.region().tick_number(), tick + 3);
        assert_eq!(x_of(&west, player()), Some(3.0));
        // The store is asked for no chunk the region had loaded. What it is asked
        // is whose the chunk is that the absorbed region's players stand in: of
        // an area that came with the merge the store names no chunks, and the
        // region claims each as it is wanted.
        assert_eq!(asked_beside_commits(&gate), [Asked::Claim(vec![FAR_EAST])]);
        settle(&mut west);

        let told = again.everything();
        let welcome = Welcome::Resumed {
            entries: 2,
            presences: 2,
            applied: before.edges[&edge.edge].applied,
        };
        assert_eq!(again.welcomed, Some(welcome));
        let theirs = &east.state.edges[&edge.edge];
        let entry = Durable::Absorbed {
            region: EAST,
            since: theirs.since,
            applied: theirs.applied,
            numbers: vec![1],
        };
        assert_eq!(theirs.applied, 2);
        assert_eq!(entries(&told), [(1, entry), (2, theirs.outbox[&1].clone())]);
        assert_eq!(
            presences(&told),
            [
                (player(), Some(EntityId(1))),
                (other_player(), Some(EntityId(40)))
            ]
        );
        let as_it_was = |message: &WorkerToEdge| matches!(message, WorkerToEdge::ChunkSnapshot { chunk: shown, .. } if *shown == chunk);
        assert_eq!(shown(&told), [ORIGIN]);
        assert!(told.iter().any(as_it_was));

        // An edge that only the absorbed region knew is told since when this one
        // knows it, which is the tick of the merge, and whom it has here.
        let (end, worker_end) = link::in_process(256);
        let mut stranger = TestEdge::silent(end, east.stranger, 1);
        west.links().attach(worker_end);
        stranger.send(stranger.hello(0, &[], &[])).await.unwrap();
        step(&mut west);
        let told = stranger.everything();
        let welcome = Welcome::Unknown {
            since: tick,
            entries: 1,
            presences: 1,
            applied: 0,
        };
        assert_eq!(stranger.welcomed, Some(welcome));
        let theirs = &east.state.edges[&east.stranger];
        let entry = Durable::Absorbed {
            region: EAST,
            since: theirs.since,
            applied: 1,
            numbers: vec![],
        };
        assert_eq!(entries(&told), [(1, entry)]);
        assert_eq!(presences(&told), [(third_player(), Some(EntityId(41)))]);

        // Whoever opens the region next finds the merged state as of that tick.
        settle(&mut west);
        let now = west.region().state();
        let (_handle, restored) = world.open(RegionId(0), 2);
        let stored = restored.state.clone().unwrap();
        assert_eq!(stored.tick, tick);
        let as_of_the_merge = Restored {
            deltas: Vec::new(),
            ..restored.clone()
        };
        // With the fresh block of keys of the hello (`docs/adr/0028-entities.md`,
        // section 1).
        let block = restored.entity_ids;
        // The store's clock is later than the merge's, by the ticks after it.
        let merged = RegionState {
            clock: merged.clock.later(restored.clock),
            ..merged
        };
        assert_eq!(
            restored_state(as_of_the_merge).unwrap(),
            with_keys(merged, block)
        );
        assert_eq!(restored.pinned.len(), 2);
        // A tick that changed nothing has no commit, and is not counted by the store.
        let mut stored = restored_state(restored).unwrap();
        assert!(tick < stored.tick && stored.tick <= now.tick);
        assert!(stored.clock <= now.clock);
        stored.tick = now.tick;
        stored.clock = now.clock;
        assert_eq!(stored, with_keys(now, block));
    }
}

/// The chunks a region has loaded at a merge are kept in memory, and handed to the
/// tick that asks storage for them. One that the region gives back is forgotten:
/// another region can hold and change it, so when the region is granted it again
/// it asks the store.
#[tokio::test]
async fn a_warm_chunk_is_served_from_memory_and_forgotten_when_the_region_gives_it_back() {
    let world = Divided::gap();
    let (mut edge, worker_end) = in_process(256);
    let (mut runner, gate) = world.gated(HOME, settings(0));
    runner.links().attach(worker_end);
    joined(&edge, &mut runner).await;
    edge.everything();
    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    assert_eq!(
        answers(&mut runner, &mut edge, 1),
        [(BESIDE, edge.asked(), Told::Snapshot)]
    );
    let east = untouched_east(&world);

    let (done, outcome) = outcome();
    runner.reshape(east.absorb(), done);
    let absorbed = Reshaped::Absorbed { absorbed: EAST };
    assert_eq!(reshaped(&mut runner, &outcome), absorbed);
    let warm: Vec<_> = runner.warm.keys().copied().collect();
    assert_eq!(warm, [ORIGIN, BESIDE]);
    assert!(runner.region().pins(ChunkPos::new(16, 0)));
    gate.asked();

    // The edge is back for the home chunk alone. Nothing uses the other, which
    // the region gives back, as nothing keeps it.
    let mut again = linked_again(&runner, &edge, &[player()], &[ORIGIN]).await;
    assert_eq!(
        answers(&mut runner, &mut again, 1),
        [(ORIGIN, 0, Told::Snapshot)]
    );
    step_until(&mut runner, |runner| {
        runner.region().knowledge(BESIDE) == Knowledge::Unknown
    });
    assert!(runner.warm.is_empty());
    let loads = |asked: Vec<Asked>| -> Vec<Asked> {
        let loads = asked.into_iter();
        loads
            .filter(|asked| matches!(asked, Asked::Load(_)))
            .collect()
    };
    assert_eq!(loads(gate.asked()), []);

    again.send(asking_for(vec![BESIDE])).await.unwrap();
    assert_eq!(
        answers(&mut runner, &mut again, 1),
        [(BESIDE, again.asked(), Told::Snapshot)]
    );
    assert_eq!(loads(gate.asked()), [Asked::Load(BESIDE)]);
}

/// When the store has answered the flush behind the second checkpoint, it has
/// answered every load asked before. The runner looks all the same, and hands the
/// store no merge while a chunk is on its way to it; the chunk that then comes is
/// kept like those that were loaded.
#[tokio::test]
async fn a_merge_is_not_handed_to_the_store_while_a_chunk_is_being_loaded() {
    let world = Divided::stripes();
    let (edge, worker_end) = in_process(256);
    let (mut west, gate) = world.gated(RegionId(0), settings(0));
    west.links().attach(worker_end);
    joined(&edge, &mut west).await;
    let east = untouched_east(&world);

    gate.hold_loads();
    edge.send(asking_for(vec![WEST_OF_HOME])).await.unwrap();
    step_until(&mut west, |runner| runner.loads.contains_key(&WEST_OF_HOME));
    wait_for_kept_answers(&mut west, &gate, GateControl::kept_loads, 1);
    gate.asked();

    let (done, outcome) = outcome();
    west.reshape(east.absorb(), done);
    step_to(&mut west, Stage::Closing);
    step_until(&mut west, |runner| {
        runner.flushes_answered == runner.flushes_asked
    });
    for _ in 0..20 {
        west.step();
    }
    assert_eq!(west.stage(), Some(Stage::Closing));
    assert!(outcome.try_recv().is_err());
    let asked = gate.asked();
    assert!(
        !asked.iter().any(|asked| matches!(asked, Asked::Absorb(_))),
        "{asked:?}"
    );

    gate.release_loads();
    let absorbed = Reshaped::Absorbed { absorbed: EAST };
    assert_eq!(reshaped(&mut west, &outcome), absorbed);
    let warm: Vec<_> = west.warm.keys().copied().collect();
    assert_eq!(warm, [WEST_OF_HOME, ORIGIN]);
    gate.asked();

    // Both are shown to the edge that is back without the store being asked.
    let chunks = [ORIGIN, WEST_OF_HOME];
    let mut again = linked_again(&west, &edge, &[player()], &chunks).await;
    assert_eq!(
        answers(&mut west, &mut again, 2),
        [
            (WEST_OF_HOME, 0, Told::Snapshot),
            (ORIGIN, 0, Told::Snapshot)
        ]
    );
    assert_eq!(asked_beside_commits(&gate), []);
}

/// A runner that is stopped while the store has the merge and has not answered
/// lets go of the region as it is. It has said nothing of the merge to anyone, and
/// says of its outcome that the store knows: here the store did it, and whoever
/// opens the region next finds it merged.
#[tokio::test]
async fn a_runner_stopped_while_the_store_has_a_merge_is_abandoned_and_has_said_nothing() {
    let world = Divided::stripes();
    let (mut edge, worker_end) = in_process(256);
    let (mut west, gate) = world.gated(RegionId(0), settings(0));
    west.links().attach(worker_end);
    joined(&edge, &mut west).await;
    let east = released_east(&world, &edge).await;
    step(&mut west);
    edge.everything();

    gate.hold_reshapes();
    let (done, outcome) = outcome();
    west.reshape(east.absorb(), done);
    step_to(&mut west, Stage::Committing);
    let tick = west.region().tick_number();
    wait_for_kept_answers(&mut west, &gate, GateControl::kept_reshapes, 1);
    for _ in 0..5 {
        west.step();
    }
    assert_eq!(west.stage(), Some(Stage::Committing));
    assert!(outcome.try_recv().is_err());

    assert_eq!(west.run(&AtomicBool::new(true)), Ended::Abandoned);
    assert_eq!(west.ended(), Some(Ended::Abandoned));
    let off = Reshaped::Off {
        why: Off::StoreLost,
    };
    assert_eq!(outcome.try_recv(), Ok(off));
    assert_eq!(west.region().tick_number(), tick);
    assert_eq!(edge.everything(), []);
    assert!(closed(&mut edge).await);

    let list = world.store.regions().unwrap();
    assert_eq!(list.absorbed, [(EAST, RegionId(0))]);
    let (_handle, restored) = world.open(RegionId(0), 2);
    assert_eq!(restored.deltas, []);
    let state = restored_state(restored).unwrap();
    assert_eq!(state.tick, tick + 1);
    assert_eq!(state.players.len(), 3);
}

/// A region that has stopped ticking for a merge, with two things taken from links
/// that no tick has taken: a step of [`player`] that came when the region was as
/// far ahead of the store as it may be, and the join of [`other_player`], which
/// waited behind the hello of a second edge until the region's last tick had shown
/// that edge its chunk.
struct Stalled {
    west: RegionRunner,
    gate: Arc<GateControl>,
    edge: TestEdge,
    second: TestEdge,
    /// The number of the step, and the step.
    late: (u64, EdgeToWorker),
    /// The region it was told to absorb, which is kept open.
    east: East,
    outcome: Receiver<Reshaped>,
    /// The number of the region's last tick.
    tick: u64,
}

/// Tells the western stripe of `world` to absorb the eastern one, as opened with
/// `absorbed_epoch`, which is 2 if the store is to do it, and stops it so.
async fn stalled(world: &Divided, absorbed_epoch: u64) -> Stalled {
    let (edge, worker_end) = in_process(256);
    let (mut west, gate) = world.gated(RegionId(0), settings(0));
    west.links().attach(worker_end);
    joined(&edge, &mut west).await;
    step(&mut west);
    let east = untouched_east(world);
    let reshape = Reshape::Absorb {
        absorbed: EAST,
        absorbed_epoch,
        state: east.state.clone(),
    };

    // Eight ticks that the store does not confirm, the last of which takes the
    // second edge's hello and, as the chunk is loaded, answers it at once.
    gate.hold();
    gate.hold_flushes();
    for x in 1..MAX_TICKS_AHEAD {
        edge.send(walk(player(), x as f64)).await.unwrap();
        west.step();
    }
    let (end, worker_end) = link::in_process(256);
    let second = TestEdge::silent(end, EdgeId::from_name("second"), 1);
    west.links().attach(worker_end);
    second.send(second.hello(0, &[], &[ORIGIN])).await.unwrap();
    second.send(join(other_player(), "Jeb")).await.unwrap();
    edge.send(walk(player(), 8.0)).await.unwrap();
    west.step();
    assert_eq!(west.pending.len(), MAX_TICKS_AHEAD);
    let tick = west.region().tick_number();
    assert!(west.links.values().all(|link| link.held.is_empty()));
    assert_eq!(west.edges[&second.edge].received, 1);
    assert_eq!(west.region().edge(second.edge).unwrap().applied, 0);

    let (done, outcome) = outcome();
    west.reshape(reshape, done);
    let late = walk(player(), 20.0);
    edge.send(late.clone()).await.unwrap();
    let number = edge.sent.load(Ordering::Relaxed);
    // This step takes it from the link, and does not tick.
    west.step();
    assert_eq!(west.region().tick_number(), tick);
    assert_eq!(west.edges[&edge.edge].received, number);
    assert_eq!(west.region().edge(edge.edge).unwrap().applied, number - 1);

    wait_for_kept_flush(&mut west, &gate);
    gate.release_flushes();
    step_to(&mut west, Stage::Settling);
    assert_eq!(west.region().tick_number(), tick);
    gate.release();
    Stalled {
        west,
        gate,
        edge,
        second,
        late: (number, late),
        east,
        outcome,
        tick,
    }
}

/// What a link sent and no tick took when the region stopped is not in the state
/// the merge is made of. With the tick of the merge it is dropped and not counted
/// as received, so that its edge, which was not told that it was applied, sends
/// it again on its next link, where it is applied once.
#[tokio::test]
async fn what_no_tick_took_before_a_merge_is_not_counted_and_is_applied_when_sent_again() {
    let world = Divided::stripes();
    let mut stalled = stalled(&world, 2).await;
    let west = &mut stalled.west;
    let absorbed = Reshaped::Absorbed { absorbed: EAST };
    assert_eq!(reshaped(west, &stalled.outcome), absorbed);
    assert_eq!(west.region().tick_number(), stalled.tick + 1);
    // The absorbed region is no more, and its handle with it.
    assert!(stalled.east.handle.is_lost());
    let (number, late) = stalled.late;

    assert_eq!(x_of(west, player()), Some(8.0));
    assert_eq!(west.region().player_count(), 1);
    let known = &west.edges[&stalled.edge.edge];
    assert_eq!((known.received, known.applied), (number - 1, number - 1));
    assert_eq!(west.edges[&stalled.second.edge].received, 0);
    assert_eq!(west.inputs, TickInputs::default());

    // The welcome says how far the region had got, and each edge sends again what
    // is beyond that.
    let mut again = linked_again(west, &stalled.edge, &[player()], &[ORIGIN]).await;
    again.send_as(number, late).await;
    let mut second = linked_again(west, &stalled.second, &[], &[ORIGIN]).await;
    second.send_as(1, join(other_player(), "Jeb")).await;
    step_until(west, |runner| {
        x_of(runner, player()) == Some(20.0) && runner.region().player_count() == 2
    });
    settle(west);
    assert_eq!(west.region().edge(again.edge).unwrap().applied, number);
    assert_eq!(west.region().edge(second.edge).unwrap().applied, 1);
    again.everything();
    second.everything();
    let applied = |welcome: Option<Welcome>| match welcome {
        Some(Welcome::Resumed { applied, .. }) => applied,
        other => panic!("welcomed {other:?}"),
    };
    assert_eq!(applied(again.welcomed), number - 1);
    assert_eq!(applied(second.welcomed), 0);
}

/// If the store declines the merge, what a link sent while the region stood still
/// is where it was, and the region's next tick takes it. That tick is the one a
/// merge would have been: no tick number was used, and no link was closed or told
/// anything.
#[tokio::test]
async fn after_a_merge_that_is_declined_the_region_ticks_on_with_what_its_links_sent() {
    let world = Divided::stripes();
    // Not the epoch the region to absorb is open with.
    let mut stalled = stalled(&world, 3).await;
    let west = &mut stalled.west;
    let declined = Decline::NotOpened { epoch: Some(2) };
    let off = Reshaped::Off {
        why: Off::Declined(declined),
    };
    assert_eq!(reshaped(west, &stalled.outcome), off);
    assert_eq!((west.stage(), west.ended()), (None, None));
    assert_eq!(west.region().tick_number(), stalled.tick);
    assert_eq!(west.links.len(), 2);
    assert_eq!(world.store.regions().unwrap().absorbed, []);
    assert!(!stalled.east.handle.is_lost());

    step(west);
    assert_eq!(west.region().tick_number(), stalled.tick + 1);
    assert_eq!(x_of(west, player()), Some(20.0));
    // The join is taken too: the region holds the stay as entering, and has named
    // it to the store.
    assert!(west.region().entering_state(other_player()).is_some());
    assert_eq!(stalled.gate.commits().last(), Some(&(stalled.tick + 1)));
    // What the edges were told is what ticks tell: no entry of a merge.
    let told = stalled.edge.everything();
    assert_eq!(entries(&told), []);
    assert_eq!(moves(&told).last(), Some(&20.0));
    assert!(stalled.second.everything().contains(&UNKNOWN));
    // The player enters the world in the tick that has the store's answer.
    wait_for_the_stays(west);
    step(west);
    assert!(west.region().player(other_player()).is_some());
}

/// Two players of one edge in the western stripe: [`player`] where players enter
/// the world and [`other_player`] in [`FAR_WEST`]. The edge is subscribed to the
/// chunks they stand in and to the one between, and all three are loaded.
async fn two_players_apart(runner: &mut RegionRunner, edge: &mut TestEdge) {
    edge.send(join(player(), "Notch")).await.unwrap();
    edge.send(join(other_player(), "Jeb")).await.unwrap();
    take_joins(runner);
    // The step is taken in the tick the players enter in.
    edge.send(asking_for(vec![ORIGIN, WEST_OF_HOME, FAR_WEST]))
        .await
        .unwrap();
    edge.send(walk(other_player(), -20.5)).await.unwrap();
    step_until(runner, |runner| {
        runner.region().loaded_chunk_count() == 3 && x_of(runner, other_player()) == Some(-20.5)
    });
    step(runner);
    edge.everything();
}

/// A split of `chunks`, with the id the store of two stripes gives out next.
fn split_off(chunks: &[ChunkPos]) -> Reshape {
    Reshape::SplitOff {
        chunks: chunks.to_vec(),
        as_epoch: 5,
        part: FIRST_PART,
    }
}

/// A split is one tick as well. The players standing in the chunks named go, with
/// the chunks nearer to them than to anyone who stays; the region tells their edge
/// so in its outbox, closes its links, and begins anew without the part. The part
/// is a region as any worker would restore it from the store's record, and a
/// runner made of it serves its chunks from memory.
#[tokio::test]
async fn a_split_makes_a_region_of_the_players_in_the_chunks_named_and_both_begin_anew() {
    for on_disk_too in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let world = match on_disk_too {
            true => Divided::stripes_in(directory.path()),
            false => Divided::stripes(),
        };
        let (mut edge, worker_end) = in_process(256);
        let (mut west, gate) = world.gated(RegionId(0), settings(0));
        west.links().attach(worker_end);
        two_players_apart(&mut west, &mut edge).await;
        gate.asked();
        let before = west.region().state();
        let far = west.region().chunk(FAR_WEST).unwrap().clone();

        let (done, outcome) = outcome();
        west.reshape(split_off(&[FAR_WEST]), done);
        let Reshaped::Split {
            region: new,
            as_epoch,
            part,
        } = reshaped(&mut west, &outcome)
        else {
            panic!("no split");
        };
        assert_eq!((new, as_epoch), (FIRST_PART, 5));
        let tick = west.region().tick_number();
        assert_eq!(
            asked_beside_commits(&gate),
            [
                Asked::Checkpoint(before.tick),
                Asked::Flush,
                Asked::Checkpoint(tick - 1),
                Asked::Flush,
                Asked::Split(tick, new),
            ]
        );

        // The part: the player who stood there, the chunk, as it was.
        assert_eq!(part.chunks, [(FAR_WEST, far)]);
        assert_eq!(part.region.tick_number(), tick);
        assert_eq!(part.region.player_count(), 1);
        assert_eq!(part.region.player(other_player()).unwrap().0, EntityId(2));
        assert_eq!(part.region.knowledge(FAR_WEST), Knowledge::Held);
        assert_eq!(part.region.knowledge(WEST_OF_HOME), Knowledge::Unknown);
        // The region: without them, and knowing nothing of the chunk.
        assert_eq!(west.region().player_count(), 1);
        assert_eq!(west.region().knowledge(FAR_WEST), Knowledge::Unknown);
        assert_eq!(west.region().knowledge(WEST_OF_HOME), Knowledge::Held);
        assert_eq!(west.region().loaded_chunk_count(), 0);
        let warm: Vec<_> = west.warm.keys().copied().collect();
        assert_eq!(warm, [WEST_OF_HOME, ORIGIN]);
        assert_eq!(west.status().players.load(Ordering::Relaxed), 1);
        assert_eq!(edge.everything(), []);
        assert!(closed(&mut edge).await);

        // The store has the new region, to be opened with the epoch named.
        let list = world.store.regions().unwrap();
        let listed: Vec<_> = list
            .regions
            .iter()
            .map(|info| (info.region, info.epoch))
            .collect();
        assert_eq!(
            listed,
            [
                (RegionId(0), 1),
                (EAST, 0),
                (NETHER_HOME, 0),
                (END_HOME, 0),
                (new, 5)
            ]
        );
        let (handle, restored) = world.store.open_region(world.hello(new, 5)).unwrap();
        let holdings = holdings(&restored);
        let block = restored.entity_ids;
        let config = settings(0).of_restored(&restored);
        let state = restored_state(restored).unwrap();
        // The part was made with no keys, and takes the block its hello gave, as a
        // runner restored with it does (`docs/adr/0028-entities.md`, section 1).
        let mut region = Region::restore(config, state, holdings);
        region.take_keys(block);
        assert_eq!(region, with_keys_of_region(&part.region, block));

        // The edge is back at the region that was split, with what it had there.
        let both = [player(), other_player()];
        let chunks = [ORIGIN, WEST_OF_HOME, FAR_WEST];
        let mut again = linked_again(&west, &edge, &both, &chunks).await;
        again.send(walk(player(), 3.0)).await.unwrap();
        assert_eq!(
            answers(&mut west, &mut again, 3),
            [
                (FAR_WEST, 0, Told::Elsewhere(new)),
                (WEST_OF_HOME, 0, Told::Snapshot),
                (ORIGIN, 0, Told::Snapshot),
            ]
        );
        step_until(&mut west, |runner| x_of(runner, player()) == Some(3.0));
        let welcome = Welcome::Resumed {
            entries: 1,
            presences: 2,
            applied: before.edges[&edge.edge].applied,
        };
        assert_eq!(again.welcomed, Some(welcome));
        let went = Durable::SplitOff {
            region: new,
            players: vec![(other_player(), EntityId(2), None)],
        };
        let state = west.region().edge(edge.edge).unwrap();
        assert_eq!(state.outbox.values().collect::<Vec<_>>(), [&went]);
        assert_eq!(
            presences(&again.aside),
            [(player(), Some(EntityId(1))), (other_player(), None)]
        );
        let loads = |asked: Vec<Asked>| asked.iter().any(|asked| matches!(asked, Asked::Load(_)));
        assert!(!loads(gate.asked()));

        // And at the part, which it has never said anything to: it is told since
        // when the part knows it and whom the part has, and shown the chunk without
        // the store being asked for it. Three ticks again.
        let (store, gate) = gate_before(handle);
        let mut part = RegionRunner::of_part_with(part, store).without_rings();
        let (end, worker_end) = link::in_process(256);
        let mut there = TestEdge::silent(end, edge.edge, edge.start);
        part.links().attach(worker_end);
        there.send(there.hello(0, &[], &[FAR_WEST])).await.unwrap();
        there.send(walk(other_player(), -22.5)).await.unwrap();
        for _ in 0..3 {
            part.step();
        }
        assert_eq!(x_of(&part, other_player()), Some(-22.5));
        settle(&mut part);
        let told = there.everything();
        let welcome = Welcome::Unknown {
            since: tick,
            entries: 0,
            presences: 1,
            applied: 0,
        };
        assert_eq!(there.welcomed, Some(welcome));
        assert_eq!(presences(&told), [(other_player(), Some(EntityId(2)))]);
        assert_eq!(shown(&told), [FAR_WEST]);
        assert_eq!(asked_beside_commits(&gate), []);
    }
}

/// The store gives out the ids of regions, and another split can have taken the
/// one a command names. The runner makes the split again with the id the store
/// says is next, and what it tells the edge names the region the store has.
#[tokio::test]
async fn a_split_that_names_another_id_than_the_next_is_made_with_the_next() {
    let world = Divided::stripes();
    let (mut edge, worker_end) = in_process(256);
    let (mut west, gate) = world.gated(RegionId(0), settings(0));
    west.links().attach(worker_end);
    two_players_apart(&mut west, &mut edge).await;
    gate.asked();

    let (done, outcome) = outcome();
    let reshape = Reshape::SplitOff {
        chunks: vec![FAR_WEST],
        as_epoch: 5,
        part: RegionId(9),
    };
    west.reshape(reshape, done);
    let Reshaped::Split { region, part, .. } = reshaped(&mut west, &outcome) else {
        panic!("no split");
    };
    assert_eq!(region, FIRST_PART);
    assert_eq!(part.region.player_count(), 1);
    let tick = west.region().tick_number();
    let asked = gate.asked();
    let splits: Vec<_> = asked
        .iter()
        .filter(|asked| matches!(asked, Asked::Split(..)))
        .collect();
    assert_eq!(
        splits,
        [
            &Asked::Split(tick, RegionId(9)),
            &Asked::Split(tick, FIRST_PART)
        ]
    );
    let state = west.region().edge(edge.edge).unwrap();
    let went = Durable::SplitOff {
        region: FIRST_PART,
        players: vec![(other_player(), EntityId(2), None)],
    };
    assert_eq!(state.outbox.values().collect::<Vec<_>>(), [&went]);
    let list = world.store.regions().unwrap();
    assert_eq!(
        ids(&list),
        [RegionId(0), EAST, NETHER_HOME, END_HOME, FIRST_PART]
    );
    assert_eq!(list.next, RegionId(FIRST_PART.0 + 1));
}

/// Nothing comes of a split that nobody stands in the chunks of, nor of a merge or
/// a split that is too large to hand to the store. The store is not asked, the
/// region ticks on from its last tick, and its link stays and is told nothing.
#[tokio::test]
async fn a_split_of_nobody_and_one_that_is_too_large_leave_the_region_as_it_was() {
    let world = Divided::stripes();
    let (mut edge, worker_end) = in_process(256);
    let (mut west, gate) = world.gated(RegionId(0), settings(0));
    west.links().attach(worker_end);
    two_players_apart(&mut west, &mut edge).await;
    let east = untouched_east(&world);
    let list = world.store.regions().unwrap();

    let attempts = [
        (
            split_off(&[WEST_OF_HOME, BESIDE]),
            MAX_RESHAPE_BYTES,
            Off::Nobody,
        ),
        // The home chunk never leaves the region that is joined.
        (split_off(&[ORIGIN]), MAX_RESHAPE_BYTES, Off::Nobody),
        (split_off(&[FAR_WEST]), 64, Off::TooLarge),
        (east.absorb(), 64, Off::TooLarge),
    ];
    let mut x = 3.0;
    for (reshape, limit, why) in attempts {
        gate.asked();
        west.max_reshape_bytes = limit;
        let (done, outcome) = outcome();
        west.reshape(reshape, done);
        step_to(&mut west, Stage::Settling);
        let tick = west.region().tick_number();
        // Sent while the region stands still.
        x += 1.0;
        edge.send(walk(player(), x)).await.unwrap();
        assert_eq!(reshaped(&mut west, &outcome), Reshaped::Off { why });
        assert_eq!((west.stage(), west.ended()), (None, None));
        let asked = gate.asked();
        assert!(
            !asked
                .iter()
                .any(|asked| matches!(asked, Asked::Absorb(_) | Asked::Split(..))),
            "{asked:?}"
        );
        assert_eq!(west.region().tick_number(), tick);
        assert_eq!(west.links.len(), 1);
        assert_eq!(edge.everything(), []);

        step(&mut west);
        assert_eq!(west.region().tick_number(), tick + 1);
        assert_eq!(x_of(&west, player()), Some(x));
        assert_eq!(moves(&edge.everything()), [x]);
    }
    assert_eq!(world.store.regions().unwrap(), list);
}

/// One thing at a time. A runner that is in the middle of a merge says of a second
/// one that it is busy, at once, and so does one that is releasing its region or
/// has ended. A release that is asked for during a merge waits for it, and the
/// next owner is restored with the merged state. A checkpoint to prepare a merge
/// is made only by a runner that does nothing else, and has no outcome.
#[tokio::test]
async fn a_runner_does_one_thing_at_a_time_and_a_release_waits_for_a_merge() {
    let world = Divided::stripes();
    let (edge, worker_end) = in_process(256);
    let (mut west, gate) = world.gated(RegionId(0), settings(0));
    west.links().attach(worker_end);
    joined(&edge, &mut west).await;
    let east = untouched_east(&world);
    let busy = Reshaped::Off { why: Off::Busy };
    let never = || -> Box<dyn FnOnce(Reshaped) + Send> {
        Box::new(|outcome: Reshaped| panic!("a checkpoint has an outcome: {outcome:?}"))
    };

    gate.asked();
    west.reshape(Reshape::Prepare, never());
    let tick = west.region().tick_number();
    // And the word when the store has it, by which the merge that follows knows
    // that it need not make a checkpoint of its own before it stops.
    assert_eq!(
        asked_beside_commits(&gate),
        [Asked::Checkpoint(tick), Asked::Flush]
    );
    assert_eq!(west.stage(), None);

    let (done, outcome) = outcome();
    west.reshape(east.absorb(), done);
    let (second_done, second) = self::outcome();
    west.reshape(split_off(&[ORIGIN]), second_done);
    assert_eq!(second.try_recv(), Ok(busy.clone()));
    gate.asked();
    west.reshape(Reshape::Prepare, never());
    assert_eq!(asked_beside_commits(&gate), []);
    west.begin_release();
    assert_eq!(west.stage(), Some(Stage::Preparing));

    let absorbed = Reshaped::Absorbed { absorbed: EAST };
    assert_eq!(reshaped(&mut west, &outcome), absorbed);
    assert_eq!((west.stage(), west.ended()), (None, None));
    let merged = west.region().state();

    // Asked again, as `run` does before every step.
    west.begin_release();
    assert_eq!(west.stage(), Some(Stage::Preparing));
    let (done, during_release) = self::outcome();
    west.reshape(split_off(&[ORIGIN]), done);
    assert_eq!(during_release.try_recv(), Ok(busy.clone()));
    assert_eq!(released(&mut west), Ended::Released);
    let (done, after) = self::outcome();
    west.reshape(split_off(&[ORIGIN]), done);
    assert_eq!(after.try_recv(), Ok(busy));

    // The ticks that ran while the release was prepared changed nothing but their
    // number.
    let (_handle, restored) = world.open(RegionId(0), 2);
    assert_eq!(restored.deltas, []);
    let block = restored.entity_ids;
    let mut state = restored_state(restored).unwrap();
    assert!(state.tick >= merged.tick);
    assert!(state.clock >= merged.clock);
    state.tick = merged.tick;
    state.clock = merged.clock;
    assert_eq!(state, with_keys(merged, block));
}

/// Steps `runner`, which may be standing still with ticks that are not confirmed,
/// until the store has answered every flush it was asked for.
fn step_until_flushed(runner: &mut RegionRunner) {
    for _ in 0..20_000 {
        runner.step();
        if runner.flushes_answered == runner.flushes_asked {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("the store did not answer the flushes");
}

/// The chunk at `position` as the store has it, read through `handle`.
fn stored_chunk(handle: &StoreHandle, position: ChunkPos) -> Chunk {
    handle.request(StoreRequest::Load { position });
    for _ in 0..20_000 {
        match handle.try_reply() {
            Some(StoreReply::Loaded { chunk, .. }) => return chunk,
            Some(other) => panic!("the store answered a load with {other:?}"),
            None => thread::sleep(Duration::from_millis(1)),
        }
    }
    panic!("the store did not answer the load");
}

/// A call for the outcome of an order to prepare, which has none.
fn no_outcome() -> Box<dyn FnOnce(Reshaped) + Send> {
    Box::new(|outcome: Reshaped| panic!("a checkpoint has an outcome: {outcome:?}"))
}

/// Whether `message` is what the tick `tick` did, as an edge is told.
fn delta_of(message: &WorkerToEdge, tick: u64) -> bool {
    matches!(message, WorkerToEdge::TickDelta { tick: of, .. } if *of == tick)
}

/// The last checkpoint is asked for as the region stops, behind the commits of its
/// ticks and without waiting for the store to confirm them: the store holds it
/// back itself until they are durable. The merge is handed to the store only when
/// the store has that checkpoint and every tick is confirmed and published as
/// well, and then the store has the state in place of every commit, and the chunk
/// with what the last tick changed.
#[tokio::test]
async fn the_last_checkpoint_is_asked_for_at_the_stop_and_the_merge_waits_for_the_ticks_too() {
    for on_disk_too in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let world = match on_disk_too {
            true => Divided::stripes_in(directory.path()),
            false => Divided::stripes(),
        };
        let (mut edge, worker_end) = in_process(256);
        let (mut west, gate) = world.gated(RegionId(0), settings(0));
        west.links().attach(worker_end);
        joined(&edge, &mut west).await;
        let east = untouched_east(&world);
        edge.everything();

        // A tick that changes a block runs while the first checkpoint is made,
        // and the store's word that it has the commit does not arrive.
        gate.hold_flushes();
        gate.hold();
        let (done, outcome) = outcome();
        west.reshape(east.absorb(), done);
        edge.send(dig(1)).await.unwrap();
        west.step();
        let tick = west.region().tick_number();
        assert_eq!(west.stage(), Some(Stage::Preparing));
        assert_eq!(west.pending.len(), 1);
        wait_for_kept(&mut west, &gate, 1);
        gate.asked();

        // Not before the store has answered the flush: a step that does not find
        // the answer runs another tick.
        wait_for_kept_flush(&mut west, &gate);
        gate.release_flushes();
        step_to(&mut west, Stage::Settling);
        assert_eq!(west.region().tick_number(), tick);
        assert_eq!(
            asked_beside_commits(&gate),
            [Asked::Save(ORIGIN), Asked::Checkpoint(tick), Asked::Flush]
        );
        assert_eq!(west.pending.len(), 1);

        // The store has the checkpoint. The tick is not known to be confirmed, so
        // nobody is told of it and the merge is not worked out.
        step_until_flushed(&mut west);
        for _ in 0..20 {
            west.step();
        }
        assert_eq!(west.stage(), Some(Stage::Settling));
        assert_eq!(west.pending.len(), 1);
        assert_eq!(asked_beside_commits(&gate), []);
        assert!(outcome.try_recv().is_err());
        assert_eq!(edge.everything(), []);

        gate.release();
        let absorbed = Reshaped::Absorbed { absorbed: EAST };
        assert_eq!(reshaped(&mut west, &outcome), absorbed);
        assert_eq!(asked_beside_commits(&gate), [Asked::Absorb(tick + 1)]);
        // The tick was published before the links were closed.
        let told = edge.everything();
        assert!(told.iter().any(|told| delta_of(told, tick)), "{told:?}");
        drop(east.handle);

        // What the next owner finds: the merged state and no commit beside it,
        // and the block dug.
        let (handle, restored) = world.open(RegionId(0), 2);
        assert_eq!(restored.deltas, []);
        assert_eq!(restored.state.as_ref().unwrap().tick, tick + 1);
        let chunk = stored_chunk(&handle, ORIGIN);
        assert_eq!(chunk.get(1, -61, 0), Some(clustine_data::blocks::AIR));
    }
}

/// A release asks for its last checkpoint as the region stops as well, and ends
/// only when the store has it and every tick is confirmed and published.
#[tokio::test]
async fn a_release_asks_for_its_last_checkpoint_at_the_stop_and_ends_when_its_ticks_are_confirmed()
{
    let world = Divided::stripes();
    let (mut edge, worker_end) = in_process(256);
    let (mut west, gate) = world.gated(RegionId(0), settings(0));
    west.links().attach(worker_end);
    joined(&edge, &mut west).await;
    edge.everything();

    gate.hold_flushes();
    gate.hold();
    west.begin_release();
    edge.send(dig(1)).await.unwrap();
    west.step();
    let tick = west.region().tick_number();
    assert_eq!(west.pending.len(), 1);
    wait_for_kept(&mut west, &gate, 1);
    gate.asked();

    // Not before the store has answered the flush: a step that does not find the
    // answer runs another tick.
    wait_for_kept_flush(&mut west, &gate);
    gate.release_flushes();
    step_to(&mut west, Stage::Settling);
    assert_eq!(
        asked_beside_commits(&gate),
        [Asked::Save(ORIGIN), Asked::Checkpoint(tick), Asked::Flush]
    );
    step_until_flushed(&mut west);
    for _ in 0..20 {
        west.step();
    }
    assert_eq!((west.stage(), west.ended()), (Some(Stage::Settling), None));
    assert_eq!(edge.everything(), []);

    gate.release();
    assert_eq!(released(&mut west), Ended::Released);
    assert_eq!(asked_beside_commits(&gate), []);
    let told = edge.everything();
    assert!(told.iter().any(|told| delta_of(told, tick)), "{told:?}");

    let (handle, restored) = world.open(RegionId(0), 2);
    assert_eq!(restored.deltas, []);
    assert_eq!(restored.state.as_ref().unwrap().tick, tick);
    let chunk = stored_chunk(&handle, ORIGIN);
    assert_eq!(chunk.get(1, -61, 0), Some(clustine_data::blocks::AIR));
}

/// A region that was told to prepare makes no checkpoint of its own before it
/// stops for the merge, if the store has the one it was told to make and few
/// chunks have changed since: it stops in the step that follows the order, and
/// the one checkpoint it asks for is the one it stands still for. What changed
/// since the order to prepare, and was not confirmed when the region stopped, the
/// store has all the same when the merge is worked out.
#[tokio::test]
async fn a_prepared_region_stops_for_a_merge_at_once_and_checkpoints_once() {
    for on_disk_too in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let world = match on_disk_too {
            true => Divided::stripes_in(directory.path()),
            false => Divided::stripes(),
        };
        let (edge, worker_end) = in_process(256);
        let (mut west, gate) = world.gated(RegionId(0), settings(0));
        west.links().attach(worker_end);
        joined(&edge, &mut west).await;
        let east = untouched_east(&world);

        west.reshape(Reshape::Prepare, no_outcome());
        assert_eq!(west.stage(), None);
        step_until(&mut west, |runner| {
            runner.flushes_answered == runner.flushes_asked
        });
        // A chunk changes after that, by a tick the store has not confirmed
        // when the merge is taken.
        gate.hold();
        edge.send(dig(1)).await.unwrap();
        west.step();
        assert_eq!((west.unsaved.len(), west.pending.len()), (1, 1));
        let tick = west.region().tick_number();
        gate.asked();

        let (done, outcome) = outcome();
        west.reshape(east.absorb(), done);
        assert_eq!(west.stage(), Some(Stage::Preparing));
        assert_eq!(asked_beside_commits(&gate), []);
        west.step();
        assert_eq!(west.stage(), Some(Stage::Settling));
        assert_eq!(west.region().tick_number(), tick);
        gate.release();
        let absorbed = Reshaped::Absorbed { absorbed: EAST };
        assert_eq!(reshaped(&mut west, &outcome), absorbed);
        assert_eq!(
            asked_beside_commits(&gate),
            [
                Asked::Save(ORIGIN),
                Asked::Checkpoint(tick),
                Asked::Flush,
                Asked::Absorb(tick + 1),
            ]
        );
        assert!(west.pending.is_empty() && west.unsaved.is_empty() && west.loads.is_empty());
        drop(east.handle);

        let (handle, restored) = world.open(RegionId(0), 2);
        assert_eq!(restored.deltas, []);
        assert_eq!(restored.state.as_ref().unwrap().tick, tick + 1);
        let chunk = stored_chunk(&handle, ORIGIN);
        assert_eq!(chunk.get(1, -61, 0), Some(clustine_data::blocks::AIR));
    }
}

/// A merge that is taken while the checkpoint of the order to prepare is still
/// being made waits for that one as the region ticks, and asks for no other.
#[tokio::test]
async fn a_merge_waits_for_the_checkpoint_of_the_order_to_prepare_and_asks_for_no_other() {
    let world = Divided::stripes();
    let (edge, worker_end) = in_process(256);
    let (mut west, gate) = world.gated(RegionId(0), settings(0));
    west.links().attach(worker_end);
    joined(&edge, &mut west).await;
    let east = untouched_east(&world);
    gate.asked();

    gate.hold_flushes();
    west.reshape(Reshape::Prepare, no_outcome());
    let prepared_at = west.region().tick_number();
    let (done, outcome) = outcome();
    west.reshape(east.absorb(), done);
    assert_eq!(
        asked_beside_commits(&gate),
        [Asked::Checkpoint(prepared_at), Asked::Flush]
    );
    wait_for_kept_flush(&mut west, &gate);
    for _ in 0..5 {
        step(&mut west);
        assert_eq!(west.stage(), Some(Stage::Preparing));
    }
    assert_eq!(west.region().tick_number(), prepared_at + 5);
    assert_eq!(asked_beside_commits(&gate), []);

    gate.release_flushes();
    west.step();
    assert_eq!(west.stage(), Some(Stage::Settling));
    let tick = west.region().tick_number();
    assert_eq!(tick, prepared_at + 5);
    let absorbed = Reshaped::Absorbed { absorbed: EAST };
    assert_eq!(reshaped(&mut west, &outcome), absorbed);
    assert_eq!(
        asked_beside_commits(&gate),
        [
            Asked::Checkpoint(tick),
            Asked::Flush,
            Asked::Absorb(tick + 1)
        ]
    );
}

/// Where more than a few chunks have changed since the order to prepare, the
/// region makes a checkpoint of its own as it ticks before it stops, as one that
/// was never told to prepare does; and it makes one, however much changes while
/// that one is made.
#[tokio::test]
async fn a_prepared_region_that_has_changed_much_since_checkpoints_once_more_before_it_stops() {
    let world = Divided::stripes();
    let (edge, worker_end) = in_process(256);
    let (mut west, gate) = world.gated(RegionId(0), settings(0));
    west.links().attach(worker_end);
    joined(&edge, &mut west).await;
    let east = untouched_east(&world);
    // One chunk is more than a few.
    west.few_unsaved = 0;
    west.fresh_for = 0;

    west.reshape(Reshape::Prepare, no_outcome());
    step_until(&mut west, |runner| {
        runner.flushes_answered == runner.flushes_asked
    });
    edge.send(dig(1)).await.unwrap();
    step(&mut west);
    assert_eq!(west.unsaved.len(), 1);
    let first = west.region().tick_number();
    gate.asked();

    gate.hold_flushes();
    let (done, outcome) = outcome();
    west.reshape(east.absorb(), done);
    assert_eq!(asked_beside_commits(&gate), []);
    // The step that finds the store prepared asks for the checkpoint and ticks.
    edge.send(dig(2)).await.unwrap();
    step(&mut west);
    assert_eq!(west.stage(), Some(Stage::Preparing));
    assert_eq!(west.region().tick_number(), first + 1);
    assert_eq!(
        asked_beside_commits(&gate),
        [Asked::Save(ORIGIN), Asked::Checkpoint(first), Asked::Flush]
    );
    // The chunk has changed again, which is more than a few again.
    assert_eq!(west.unsaved.len(), 1);
    wait_for_kept_flush(&mut west, &gate);
    step(&mut west);
    assert_eq!(west.stage(), Some(Stage::Preparing));

    gate.release_flushes();
    west.step();
    assert_eq!(west.stage(), Some(Stage::Settling));
    let tick = west.region().tick_number();
    assert_eq!(tick, first + 2);
    let absorbed = Reshaped::Absorbed { absorbed: EAST };
    assert_eq!(reshaped(&mut west, &outcome), absorbed);
    assert_eq!(
        asked_beside_commits(&gate),
        [
            Asked::Save(ORIGIN),
            Asked::Checkpoint(tick),
            Asked::Flush,
            Asked::Absorb(tick + 1),
        ]
    );
}

/// A region whose checkpoint for an order to prepare was made a moment ago stops
/// for the merge at once, though more than a few chunks have changed since, and
/// an order to prepare that comes again in that moment asks for nothing. A crowd
/// changes as many chunks again while any checkpoint is made, and the players of
/// the region that is absorbed stand still for every one of them.
#[tokio::test]
async fn a_region_prepared_a_moment_ago_stops_at_once_however_much_has_changed() {
    let world = Divided::stripes();
    let (edge, worker_end) = in_process(256);
    let (mut west, gate) = world.gated(RegionId(0), settings(0));
    west.links().attach(worker_end);
    joined(&edge, &mut west).await;
    let east = untouched_east(&world);
    // One chunk is more than a few; a moment is as long as it ever is.
    west.few_unsaved = 0;

    west.reshape(Reshape::Prepare, no_outcome());
    step_until(&mut west, |runner| {
        runner.flushes_answered == runner.flushes_asked
    });
    edge.send(dig(1)).await.unwrap();
    step(&mut west);
    assert_eq!(west.unsaved.len(), 1);
    gate.asked();

    west.reshape(Reshape::Prepare, no_outcome());
    assert_eq!(asked_beside_commits(&gate), []);
    let (done, outcome) = outcome();
    west.reshape(east.absorb(), done);
    assert_eq!(asked_beside_commits(&gate), []);
    // The step that finds the store prepared stops, and ticks no more.
    let tick = west.region().tick_number();
    step_to(&mut west, Stage::Settling);
    assert_eq!(west.region().tick_number(), tick);
    let absorbed = Reshaped::Absorbed { absorbed: EAST };
    assert_eq!(reshaped(&mut west, &outcome), absorbed);
    assert_eq!(
        asked_beside_commits(&gate),
        [
            Asked::Save(ORIGIN),
            Asked::Checkpoint(tick),
            Asked::Flush,
            Asked::Absorb(tick + 1),
        ]
    );
}

/// An order to prepare that comes again asks the store for nothing while the
/// checkpoint of the one before is being made, nor when few chunks have changed
/// since; it does when more have. And it is good for one merge or split: the one
/// after that makes its own checkpoint before it stops, unless it is told again.
#[tokio::test]
async fn an_order_to_prepare_is_not_repeated_needlessly_and_is_good_for_one_merge_or_split() {
    let world = Divided::stripes();
    let (edge, worker_end) = in_process(256);
    let (mut west, gate) = world.gated(RegionId(0), settings(0));
    west.links().attach(worker_end);
    joined(&edge, &mut west).await;
    gate.asked();

    gate.hold_flushes();
    west.reshape(Reshape::Prepare, no_outcome());
    let tick = west.region().tick_number();
    assert_eq!(
        asked_beside_commits(&gate),
        [Asked::Checkpoint(tick), Asked::Flush]
    );
    west.reshape(Reshape::Prepare, no_outcome());
    assert_eq!(asked_beside_commits(&gate), []);

    gate.release_flushes();
    step_until(&mut west, |runner| {
        runner.flushes_answered == runner.flushes_asked
    });
    edge.send(dig(1)).await.unwrap();
    step(&mut west);
    assert_eq!(west.unsaved.len(), 1);
    gate.asked();
    west.reshape(Reshape::Prepare, no_outcome());
    assert_eq!(asked_beside_commits(&gate), []);

    west.few_unsaved = 0;
    west.fresh_for = 0;
    west.reshape(Reshape::Prepare, no_outcome());
    let tick = west.region().tick_number();
    assert_eq!(
        asked_beside_commits(&gate),
        [Asked::Save(ORIGIN), Asked::Checkpoint(tick), Asked::Flush]
    );
    step_until(&mut west, |runner| {
        runner.flushes_answered == runner.flushes_asked
    });
    gate.asked();

    // A split that finds nobody in the chunks it names: the region stops for it
    // at once, and ticks on.
    let nobody = Reshaped::Off { why: Off::Nobody };
    let (done, outcome) = outcome();
    west.reshape(split_off(&[FAR_WEST]), done);
    assert_eq!(asked_beside_commits(&gate), []);
    west.step();
    assert_eq!(west.stage(), Some(Stage::Settling));
    let tick = west.region().tick_number();
    assert_eq!(reshaped(&mut west, &outcome), nobody);
    assert_eq!(
        asked_beside_commits(&gate),
        [Asked::Checkpoint(tick), Asked::Flush]
    );

    // The next one was not told to prepare.
    step(&mut west);
    gate.asked();
    let before = west.region().tick_number();
    let (done, outcome) = self::outcome();
    west.reshape(split_off(&[FAR_WEST]), done);
    assert_eq!(
        asked_beside_commits(&gate),
        [Asked::Checkpoint(before), Asked::Flush]
    );
    assert_eq!(reshaped(&mut west, &outcome), nobody);
    let tick = west.region().tick_number();
    assert_eq!(
        asked_beside_commits(&gate),
        [Asked::Checkpoint(tick), Asked::Flush]
    );
}

/// Waits until the region of `status` is at `tick`.
fn wait_for_tick(status: &RegionStatus, tick: u64) {
    for _ in 0..20_000 {
        if status.tick.load(Ordering::Relaxed) == tick {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("the region never came to tick {tick}");
}

/// A region on a thread of its own takes a merge when it is handed in, looks at
/// the store's answers as they come and stops when the store has the checkpoint:
/// none of it waits for the next tick. Here no tick is due for an hour after the
/// first, and the merge is made all the same, as the tick after that one;
/// whether the region was told to prepare or makes its own first checkpoint.
#[tokio::test]
async fn a_worker_takes_a_merge_and_carries_it_through_between_two_ticks() {
    for prepared in [false, true] {
        let world = Divided::stripes();
        let (edge, worker_end) = in_process(256);
        let (mut west, gate) = world.gated(RegionId(0), settings(0));
        west.links().attach(worker_end);
        joined(&edge, &mut west).await;
        let east = untouched_east(&world);
        west.pace = Duration::from_secs(3600);
        let status = west.status();
        // The thread's first tick runs at once.
        let tick = west.region().tick_number() + 1;
        let worker = Worker::spawn(west);
        wait_for_tick(&status, tick);
        gate.asked();

        if prepared {
            worker.reshape(Reshape::Prepare, no_outcome());
        }
        let (done, outcome) = outcome();
        worker.reshape(east.absorb(), done);
        let absorbed = Reshaped::Absorbed { absorbed: EAST };
        let patience = Duration::from_secs(20);
        assert_eq!(outcome.recv_timeout(patience), Ok(absorbed), "{prepared}");
        assert_eq!(status.tick.load(Ordering::Relaxed), tick + 1);
        // Either way two checkpoints of the region as that one tick left it: the
        // one it was told to make or made first, and the one it stood still for.
        assert_eq!(
            asked_beside_commits(&gate),
            [
                Asked::Checkpoint(tick),
                Asked::Flush,
                Asked::Checkpoint(tick),
                Asked::Flush,
                Asked::Absorb(tick + 1),
            ],
            "{prepared}"
        );
        let list = world.store.regions().unwrap();
        assert_eq!(list.absorbed, [(EAST, RegionId(0))]);

        // So is a release: the thread is told at once, and no tick runs for it.
        worker.begin_release();
        wait_until_finished(&worker);
        assert_eq!(worker.stop(), Ended::Released);
        assert_eq!(status.tick.load(Ordering::Relaxed), tick + 1);
        drop(edge);
    }
}

/// What a link sent is the next tick's to take, also where a release is taken up
/// and carried through between two ticks: nothing is taken from a link for it, as
/// nothing is when a release is stepped by hand. A region that is released before
/// that tick has not applied it, and the edge sends it again to the next owner.
#[tokio::test]
async fn a_worker_that_is_released_between_two_ticks_takes_nothing_from_its_links() {
    let world = Divided::stripes();
    let (mut edge, worker_end) = in_process(256);
    let (mut west, gate) = world.gated(RegionId(0), settings(0));
    west.links().attach(worker_end);
    joined(&edge, &mut west).await;
    edge.everything();
    let applied = west.region().edge(edge.edge).unwrap().applied;
    west.pace = Duration::from_secs(3600);
    let status = west.status();
    let tick = west.region().tick_number() + 1;
    let worker = Worker::spawn(west);
    wait_for_tick(&status, tick);
    gate.asked();

    edge.send(dig(1)).await.unwrap();
    worker.begin_release();
    wait_until_finished(&worker);
    assert_eq!(worker.stop(), Ended::Released);
    assert_eq!(status.tick.load(Ordering::Relaxed), tick);
    assert_eq!(
        gate.asked(),
        [
            Asked::Checkpoint(tick),
            Asked::Flush,
            Asked::Checkpoint(tick),
            Asked::Flush,
        ]
    );
    assert!(closed(&mut edge).await);

    let (handle, restored) = world.open(RegionId(0), 2);
    assert_eq!(restored.deltas, []);
    let state = restored_state(restored).unwrap();
    assert_eq!(state.tick, tick);
    assert_eq!(state.edges[&edge.edge].applied, applied);
    let chunk = stored_chunk(&handle, ORIGIN);
    assert_ne!(chunk.get(1, -61, 0), Some(clustine_data::blocks::AIR));
}

/// A runner whose store handle is lost in the middle of a merge stops for good, as
/// it does at any time, and says of the merge that the store knows what came of
/// it. So does one that is dropped.
#[tokio::test]
async fn a_merge_whose_runner_loses_the_store_or_is_dropped_leaves_the_outcome_to_the_store() {
    let lost = Reshaped::Off {
        why: Off::StoreLost,
    };
    for stage in [Stage::Preparing, Stage::Settling, Stage::Closing] {
        let world = Divided::stripes();
        let (mut edge, worker_end) = in_process(256);
        let (mut west, gate) = world.gated(RegionId(0), settings(0));
        west.links().attach(worker_end);
        joined(&edge, &mut west).await;
        let east = untouched_east(&world);

        let (done, outcome) = outcome();
        west.reshape(east.absorb(), done);
        step_to(&mut west, stage);
        gate.lose();
        west.step();
        assert_eq!(outcome.try_recv(), Ok(lost.clone()));
        assert_eq!(west.ended(), Some(Ended::StoreLost));
        assert_eq!(west.stage(), None);
        assert!(closed(&mut edge).await);
        assert_eq!(world.store.regions().unwrap().absorbed, []);

        let again = world.hello(RegionId(0), 2);
        let (mut west, _) = gated_as(&world.store, again, settings(0));
        let (done, outcome) = self::outcome();
        west.reshape(east.absorb(), done);
        step_to(&mut west, stage);
        drop(west);
        assert_eq!(outcome.try_recv(), Ok(lost.clone()));
    }
}

/// Chunks that nobody has asked for 600 ticks after the merge or the split they
/// are kept from are forgotten.
#[tokio::test]
async fn warm_chunks_are_forgotten_when_nobody_has_asked_for_them_for_600_ticks() {
    let world = Divided::stripes();
    let (edge, worker_end) = in_process(256);
    let (mut west, _) = world.gated(RegionId(0), settings(0));
    west.links().attach(worker_end);
    joined(&edge, &mut west).await;
    // A chunk besides the home chunk, which the region asks for again at once
    // after the merge, as it loads it whoever needs it.
    let beyond = ChunkPos::new(-1, 0);
    edge.send(asking_for(vec![beyond])).await.unwrap();
    step_until(&mut west, |runner| runner.region().chunk(beyond).is_some());
    let east = untouched_east(&world);
    let (done, outcome) = outcome();
    west.reshape(east.absorb(), done);
    let absorbed = Reshaped::Absorbed { absorbed: EAST };
    assert_eq!(reshaped(&mut west, &outcome), absorbed);
    let tick = west.region().tick_number();
    assert_eq!(WARM_FOR, 600);

    step_until(&mut west, |runner| {
        runner.region().tick_number() == tick + WARM_FOR - 1
    });
    let warm: Vec<_> = west.warm.keys().copied().collect();
    assert_eq!(warm, [beyond]);
    step(&mut west);
    assert_eq!(west.region().tick_number(), tick + WARM_FOR);
    assert!(west.warm.is_empty());
    // The chunk is still the region's, and comes from the store when it is asked for.
    assert_eq!(west.region().knowledge(beyond), Knowledge::Held);
}

/// A worker takes a merge on the thread of its region and calls with the outcome
/// from there. One whose thread has ended calls with the word that nothing came of
/// it.
#[tokio::test]
async fn a_worker_reshapes_its_region_on_its_thread_and_says_so_once() {
    let world = Divided::stripes();
    let (edge, worker_end) = in_process(256);
    let west = world.runner(RegionId(0), 1);
    west.links().attach(worker_end);
    let status = west.status();
    let east = untouched_east(&world);
    let worker = Worker::spawn(west);
    edge.send(join(player(), "Notch")).await.unwrap();

    let never = Box::new(|outcome: Reshaped| panic!("a checkpoint has an outcome: {outcome:?}"));
    worker.reshape(Reshape::Prepare, never);
    let (done, outcome) = outcome();
    worker.reshape(east.absorb(), done);
    let absorbed = Reshaped::Absorbed { absorbed: EAST };
    let patience = Duration::from_secs(10);
    assert_eq!(outcome.recv_timeout(patience), Ok(absorbed));
    assert_eq!(
        ids(&world.store.regions().unwrap()),
        [RegionId(0), NETHER_HOME, END_HOME]
    );
    // It ticks on.
    let tick = status.tick.load(Ordering::Relaxed);
    for _ in 0..20_000 {
        if status.tick.load(Ordering::Relaxed) > tick {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(status.tick.load(Ordering::Relaxed) > tick);

    worker.begin_release();
    wait_until_finished(&worker);
    let (done, outcome) = self::outcome();
    worker.reshape(split_off(&[ORIGIN]), done);
    let busy = Reshaped::Off { why: Off::Busy };
    assert_eq!(outcome.recv_timeout(patience), Ok(busy));
    assert_eq!(worker.stop(), Ended::Released);
}

/// A region whose owner died instead of releasing it has commits that no checkpoint
/// covers, and the store declines to have it absorbed as it is. Whoever is told to
/// absorb it reads its state with [`absorbable`], which has the store keep that
/// state in place of those commits; then the merge is made.
#[tokio::test]
async fn a_region_whose_owner_died_is_checkpointed_before_it_is_absorbed() {
    let world = Divided::stripes();
    let mut east = world.runner(EAST, 1);
    let (there, worker_end) = in_process(256);
    east.links().attach(worker_end);
    there
        .send(arriving_in_the_east(other_player(), 40))
        .await
        .unwrap();
    step_until(&mut east, |runner| runner.region().player_count() == 1);
    settle(&mut east);
    let players = east.region().state().players;
    drop(east);

    let (handle, restored) = world.open(EAST, 2);
    assert!(!restored.deltas.is_empty());
    let (mut west, _) = world.gated(RegionId(0), settings(0));
    let absorb = |state: RegionState| Reshape::Absorb {
        absorbed: EAST,
        absorbed_epoch: 2,
        state,
    };

    // As it was read, without a word to the store.
    let unchecked = restored_state(restored.clone()).unwrap();
    let (done, outcome) = outcome();
    west.reshape(absorb(unchecked), done);
    let declined = Decline::Uncheckpointed { region: EAST };
    let off = Reshaped::Off {
        why: Off::Declined(declined),
    };
    assert_eq!(reshaped(&mut west, &outcome), off);

    let state = absorbable(&handle, restored).unwrap();
    assert_eq!(state.players, players);
    let restored_tick = state.tick;
    let (done, outcome) = self::outcome();
    west.reshape(absorb(state), done);
    let absorbed = Reshaped::Absorbed { absorbed: EAST };
    assert_eq!(reshaped(&mut west, &outcome), absorbed);
    // On the survivor's ticks (`digging.rs`).
    let merged = west.region().state();
    let by = i64::try_from(merged.tick).unwrap() - 1 - i64::try_from(restored_tick).unwrap();
    let moved: BTreeMap<_, _> = (players.into_iter())
        .map(|(id, mut player)| {
            player.game_origin += by;
            (id, player)
        })
        .collect();
    assert_eq!(merged.players, moved);
}

/// What the store delivers for the coming tick is kept at a merge only if the
/// region held the chunk by what its ticks were told. A chunk it has given back
/// since it asked for it, and is granted again by an answer that no tick has
/// taken, can have been another region's in between: what was read for the first
/// request is not kept, and the chunk is asked of the store when it is wanted.
#[tokio::test]
async fn a_chunk_read_before_the_region_gave_it_back_is_not_kept_when_it_is_granted_again() {
    let world = Divided::gap();
    let (edge, worker_end) = in_process(256);
    let (mut runner, gate) = world.gated(HOME, settings(0));
    runner.links().attach(worker_end);
    joined(&edge, &mut runner).await;

    // The chunk is granted and asked of the store, whose answer does not come.
    gate.hold_loads();
    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    step_until(&mut runner, |runner| runner.loads.contains_key(&BESIDE));
    wait_for_kept_answers(&mut runner, &gate, GateControl::kept_loads, 1);
    // The link lets go, and the region gives the chunk back; then the link asks
    // again, and the store's word that the chunk is the region's does not come.
    edge.send(done_with(vec![BESIDE])).await.unwrap();
    step_until(&mut runner, |runner| {
        runner.region().knowledge(BESIDE) == Knowledge::Unknown
    });
    gate.hold_claims();
    edge.send(asking_for(vec![BESIDE])).await.unwrap();
    step_until(&mut runner, |runner| {
        runner.region().knowledge(BESIDE) == Knowledge::Asked
    });
    wait_for_kept_answers(&mut runner, &gate, GateControl::kept_claims, 1);

    let east = untouched_east(&world);
    let (done, outcome) = outcome();
    runner.reshape(east.absorb(), done);
    step_to(&mut runner, Stage::Closing);
    gate.release_loads();
    gate.release_claims();
    let absorbed = Reshaped::Absorbed { absorbed: EAST };
    assert_eq!(reshaped(&mut runner, &outcome), absorbed);
    // Granted by the answer that waited, and not kept from the answer before it.
    assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Held);
    let warm: Vec<_> = runner.warm.keys().copied().collect();
    assert_eq!(warm, [ORIGIN]);
    gate.asked();

    let chunks = [ORIGIN, BESIDE];
    let mut again = linked_again(&runner, &edge, &[player()], &chunks).await;
    assert_eq!(
        answers(&mut runner, &mut again, 2),
        [(ORIGIN, 0, Told::Snapshot), (BESIDE, 0, Told::Snapshot)]
    );
    let asked = gate.asked();
    let loads: Vec<_> = asked
        .iter()
        .filter(|asked| matches!(asked, Asked::Load(_)))
        .collect();
    assert_eq!(loads, [&Asked::Load(BESIDE)]);
}

/// A chunk that is still warm when its region is split goes where the split puts
/// it: a part that is split again before any edge has asked it for anything hands
/// the second part its chunks, and the runner of that one serves them from memory.
#[tokio::test]
async fn a_chunk_that_is_still_warm_goes_with_the_part_when_its_region_is_split_again() {
    let world = Divided::stripes();
    let (mut edge, worker_end) = in_process(256);
    let (mut west, _) = world.gated(RegionId(0), settings(0));
    west.links().attach(worker_end);
    two_players_apart(&mut west, &mut edge).await;
    let further = ChunkPos::new(-4, 0);
    edge.send(join(third_player(), "Dinnerbone")).await.unwrap();
    take_joins(&mut west);
    edge.send(asking_for(vec![further])).await.unwrap();
    edge.send(walk(third_player(), -60.5)).await.unwrap();
    step_until(&mut west, |runner| {
        runner.region().chunk(further).is_some() && x_of(runner, third_player()) == Some(-60.5)
    });

    // Two of the three players go, each with the chunk they stand in.
    let (done, outcome) = outcome();
    west.reshape(split_off(&[FAR_WEST, further]), done);
    let Reshaped::Split { region, part, .. } = reshaped(&mut west, &outcome) else {
        panic!("no split");
    };
    let positions = |part: &Part| -> Vec<ChunkPos> {
        let chunks = part.chunks.iter();
        chunks.map(|(position, _)| *position).collect()
    };
    assert_eq!(positions(&part), [further, FAR_WEST]);
    let (handle, _) = world.store.open_region(world.hello(region, 5)).unwrap();
    let mut first = RegionRunner::of_part(part, handle).without_rings();

    let (done, outcome) = self::outcome();
    let second = RegionId(FIRST_PART.0 + 1);
    let again = Reshape::SplitOff {
        chunks: vec![further],
        as_epoch: 6,
        part: second,
    };
    first.reshape(again, done);
    let Reshaped::Split { region, part, .. } = reshaped(&mut first, &outcome) else {
        panic!("no second split");
    };
    assert_eq!(region, second);
    assert_eq!(positions(&part), [further]);
    assert_eq!(part.region.player(third_player()).unwrap().0, EntityId(3));
    let warm: Vec<_> = first.warm.keys().copied().collect();
    assert_eq!(warm, [FAR_WEST]);
    assert_eq!(first.region().player_count(), 1);

    let (handle, _) = world.store.open_region(world.hello(region, 6)).unwrap();
    let (store, gate) = gate_before(handle);
    let mut second = RegionRunner::of_part_with(part, store).without_rings();
    let (end, worker_end) = link::in_process(256);
    let mut there = TestEdge::silent(end, edge.edge, edge.start);
    second.links().attach(worker_end);
    there
        .send(there.hello(0, &[third_player()], &[further]))
        .await
        .unwrap();
    assert_eq!(
        answers(&mut second, &mut there, 1),
        [(further, 0, Told::Snapshot)]
    );
    assert_eq!(asked_beside_commits(&gate), []);
    assert_eq!(
        presences(&there.aside),
        [(third_player(), Some(EntityId(3)))]
    );
}

// -----------------------------------------------------------------------------------
// The grants that wait at a split go by nearness: section 3.6.1 of
// `docs/adr/0017-the-end-of-the-stripes.md`.
// -----------------------------------------------------------------------------------

/// Listens to the log and keeps each line: the message, and behind it the fields
/// the line was given, as `name=value` with a space before each, in their order.
#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<String>>>);

#[derive(Default)]
struct Line {
    message: String,
    fields: String,
}

impl tracing::field::Visit for Line {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.fields
                .push_str(&format!(" {}={value:?}", field.name()));
        }
    }
}

impl tracing::Subscriber for Lines {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut line = Line::default();
        event.record(&mut line);
        self.0.lock().unwrap().push(line.message + &line.fields);
    }

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}

/// Runs `what`, and returns what it returns with the lines that the thread it ran
/// on wrote to the log meanwhile. A runner that is stepped by hand writes its
/// lines on the thread that steps it.
fn logged<T>(what: impl FnOnce() -> T) -> (T, Vec<String>) {
    // Whether anybody listens to a line is remembered where the line is written,
    // when the first thread gets there. While the process has a single listener,
    // only that thread's own is asked, and the tests run side by side, most of
    // them with nobody listening. Once there are two, every listener there is is
    // asked; so one is kept for good here, which no thread ever writes to.
    static KEPT: std::sync::OnceLock<tracing::Dispatch> = std::sync::OnceLock::new();
    KEPT.get_or_init(|| tracing::Dispatch::new(Lines::default()));
    let lines = Lines::default();
    let returned = tracing::subscriber::with_default(lines.clone(), what);
    let lines = lines.0.lock().unwrap();
    (returned, lines.clone())
}

/// The line a runner writes when it has taken a split, if it is among `lines`.
fn line_of_the_split(lines: &[String]) -> Option<&str> {
    let of_the_split = |line: &&String| line.starts_with("a part of the region has been split");
    lines.iter().find(of_the_split).map(String::as_str)
}

/// Has `edge` ask for `chunks` as a viewer, steps `runner` until a tick has claimed
/// them, and waits for the store's answer, which `gate` holds back: the grants wait
/// for a tick from here on, and are not among its inputs before the gate lets the
/// answer go.
async fn asked_and_not_told(
    runner: &mut RegionRunner,
    gate: &GateControl,
    edge: &TestEdge,
    chunks: &[ChunkPos],
) {
    let claims = gate.kept_claims();
    gate.hold_claims();
    edge.send(asking_for(chunks.to_vec())).await.unwrap();
    let asked = |runner: &RegionRunner| {
        let known = |chunk: &ChunkPos| runner.region().knowledge(*chunk);
        chunks.iter().all(|chunk| known(chunk) == Knowledge::Asked)
    };
    step_until(runner, asked);
    wait_for_kept_answers(runner, gate, GateControl::kept_claims, claims + 1);
}

/// The chunks the store's list has `region` granted within, both corners; `None`
/// if it is granted none. What a region holds of an area it is pinned to is not
/// among them.
fn bounds_of(store: &Store, region: RegionId) -> Option<(ChunkPos, ChunkPos)> {
    let list = store.regions().unwrap();
    let info = list.regions.iter().find(|info| info.region == region);
    let bounds = info.expect("the region is listed").bounds?;
    Some((bounds.min, bounds.max))
}

/// Two chunks west of [`FAR_WEST`], and so nearer to who stands there than to
/// where players enter the world; and the chunk south of that place.
const AHEAD: ChunkPos = ChunkPos::new(-4, 0);
const AT_HOME: ChunkPos = ChunkPos::new(0, 1);

/// A chunk the store has granted the region in answer to a claim of its last
/// ticks, of which no tick was told before the region stopped, counts as held when
/// the split is worked out. One that is nearer to who goes is the part's, by both
/// regions and by the store's list, and one that is not is the region's. Each of
/// the two is in memory what a runner that is started anew restores from the
/// store's record, and the line that tells of the split counts the grant.
#[tokio::test]
async fn a_grant_that_no_tick_was_told_of_goes_with_the_part_if_it_is_nearer_to_who_goes() {
    for on_disk_too in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let world = match on_disk_too {
            true => Divided::stripes_in(directory.path()),
            false => Divided::stripes(),
        };
        let (mut edge, worker_end) = in_process(256);
        let (mut west, gate) = world.gated(RegionId(0), settings(0));
        west.links().attach(worker_end);
        two_players_apart(&mut west, &mut edge).await;
        asked_and_not_told(&mut west, &gate, &edge, &[AHEAD, AT_HOME]).await;

        // The answer reaches the runner when its region has stopped ticking.
        let (done, outcome) = outcome();
        west.reshape(split_off(&[FAR_WEST]), done);
        step_to(&mut west, Stage::Settling);
        assert_eq!(west.inputs.granted, []);
        gate.release_claims();
        let before = west.region().tick_number();
        let (reshaped, lines) = logged(|| reshaped(&mut west, &outcome));
        let Reshaped::Split {
            region: new, part, ..
        } = reshaped
        else {
            panic!("no split: {reshaped:?}");
        };
        let tick = west.region().tick_number();
        assert_eq!(tick, before + 1, "no tick ran in between");
        let line = format!(
            "a part of the region has been split off tick={tick} part={} players=1 \
                 chunks=2 waited=1",
            new.0
        );
        assert_eq!(line_of_the_split(&lines), Some(line.as_str()));

        // By both regions: each chunk is one's, and the other knows nothing of it.
        assert_eq!(part.region.knowledge(AHEAD), Knowledge::Held);
        assert_eq!(west.region().knowledge(AHEAD), Knowledge::Unknown);
        assert_eq!(west.region().knowledge(AT_HOME), Knowledge::Held);
        assert_eq!(part.region.knowledge(AT_HOME), Knowledge::Unknown);
        assert_eq!(part.region.held_chunk_count(), 2);
        assert_eq!(west.region().held_chunk_count(), 3);
        // The answer went into the split, and waits for no tick.
        assert_eq!(west.inputs.granted, []);
        // By the store's list: the part is granted both of its chunks. The region
        // is granted none, as all it holds is of the area it is pinned to.
        assert_eq!(bounds_of(&world.store, new), Some((AHEAD, FAR_WEST)));
        assert_eq!(bounds_of(&world.store, RegionId(0)), None);

        // And after a restart: of the runners, and on disk of the store as well,
        // from what it wrote. The part is restored as it is in memory.
        drop(west);
        let world = match on_disk_too {
            true => {
                drop(world);
                Divided::stripes_in(directory.path())
            }
            false => world,
        };
        let (handle, restored) = world.open(new, 5);
        let mut held = holdings(&restored).held;
        held.sort();
        assert_eq!(held, [AHEAD, FAR_WEST]);
        // With the block of keys its hello gave (`docs/adr/0028-entities.md`,
        // section 1).
        let block = restored.entity_ids;
        let next = RegionRunner::restore(settings(0), handle, restored).unwrap();
        assert_eq!(*next.region(), with_keys_of_region(&part.region, block));
        // The next owner of the region that was split is told nothing of the chunk
        // ahead, and when it asks, that it is the part's. A pinned region is
        // restored without the chunks of its own area and claims them anew.
        let (handle, restored) = world.open(RegionId(0), 2);
        assert!(!holdings(&restored).held.contains(&AHEAD));
        assert_eq!(
            claim(&handle, &[AHEAD, AT_HOME]),
            (vec![AT_HOME], vec![(AHEAD, new)])
        );
    }
}

/// If nothing comes of the split, the grants it was worked out with are where they
/// were: the next tick takes them, and the link is served.
#[tokio::test]
async fn a_grant_that_waited_while_a_split_came_to_nothing_is_taken_by_the_next_tick() {
    let world = Divided::stripes();
    let (mut edge, worker_end) = in_process(256);
    let (mut west, gate) = world.gated(RegionId(0), settings(0));
    west.links().attach(worker_end);
    two_players_apart(&mut west, &mut edge).await;
    asked_and_not_told(&mut west, &gate, &edge, &[AHEAD]).await;

    // Nobody stands in the chunk named.
    let (done, outcome) = outcome();
    west.reshape(split_off(&[WEST_OF_HOME]), done);
    step_to(&mut west, Stage::Settling);
    gate.release_claims();
    let off = Reshaped::Off { why: Off::Nobody };
    assert_eq!(reshaped(&mut west, &outcome), off);
    assert_eq!(west.inputs.granted, [AHEAD]);
    assert_eq!(west.region().knowledge(AHEAD), Knowledge::Asked);

    let asked = edge.asked();
    assert_eq!(
        answers(&mut west, &mut edge, 1),
        [(AHEAD, asked, Told::Snapshot)]
    );
    assert_eq!(west.region().knowledge(AHEAD), Knowledge::Held);
    assert_eq!(west.inputs.granted, []);
}

/// Where [`other_player`] stands in [`two_players_apart_in_the_gap`], and the
/// chunk two further east, which is nearer to that than to where players enter the
/// world. Both are nobody's until they are claimed.
const OUT_EAST: ChunkPos = ChunkPos::new(6, 0);
const BEYOND: ChunkPos = ChunkPos::new(8, 0);

/// Two players of one edge in the home region of [`Divided::gap`], which is pinned
/// to nothing: [`player`] where players enter the world and [`other_player`] in
/// [`OUT_EAST`]. The edge is subscribed to the chunks they stand in, and both are
/// loaded.
async fn two_players_apart_in_the_gap(runner: &mut RegionRunner, edge: &mut TestEdge) {
    edge.send(join(player(), "Notch")).await.unwrap();
    edge.send(join(other_player(), "Jeb")).await.unwrap();
    edge.send(asking_for(vec![ORIGIN, OUT_EAST])).await.unwrap();
    step_until(runner, |runner| runner.region().player_count() == 2);
    // The home region of this world gives out the ids of another block than the
    // one the stripes begin with.
    let (entity, _) = runner.region().player(other_player()).unwrap();
    let walked = of_entity(entity, walk(other_player(), 100.5));
    edge.send(walked).await.unwrap();
    step_until(runner, |runner| {
        runner.region().loaded_chunk_count() == 2 && x_of(runner, other_player()) == Some(100.5)
    });
    step(runner);
    edge.everything();
}

/// A split of `chunks` off the home region of [`Divided::gap`], with the id the
/// store of that world gives out next.
fn split_off_in_the_gap(chunks: &[ChunkPos]) -> Reshape {
    Reshape::SplitOff {
        chunks: chunks.to_vec(),
        as_epoch: 5,
        part: GAP_FIRST_PART,
    }
}

/// The twin of
/// [`a_chunk_read_before_the_region_gave_it_back_is_not_kept_when_it_is_granted_again`]
/// for a split, and for a chunk that goes to the part. What the store delivered
/// for a request from before the region gave the chunk back is kept by neither
/// region when the chunk is granted again by an answer that no tick has taken:
/// another region can have held and changed it in between. The part holds the
/// chunk and reads it from the store when it is wanted.
#[tokio::test]
async fn a_chunk_read_before_it_was_given_back_is_not_kept_when_its_new_grant_goes_to_a_part() {
    let world = Divided::gap();
    let (mut edge, worker_end) = in_process(256);
    let (mut runner, gate) = world.gated(HOME, settings(0));
    runner.links().attach(worker_end);
    two_players_apart_in_the_gap(&mut runner, &mut edge).await;

    // The chunk is granted and asked of the store, whose answer does not come.
    gate.hold_loads();
    edge.send(asking_for(vec![BEYOND])).await.unwrap();
    step_until(&mut runner, |runner| runner.loads.contains_key(&BEYOND));
    wait_for_kept_answers(&mut runner, &gate, GateControl::kept_loads, 1);
    // The link lets go, and the region gives the chunk back; then the link asks
    // again, and the store's word that the chunk is the region's does not come.
    edge.send(done_with(vec![BEYOND])).await.unwrap();
    step_until(&mut runner, |runner| {
        runner.region().knowledge(BEYOND) == Knowledge::Unknown
    });
    asked_and_not_told(&mut runner, &gate, &edge, &[BEYOND]).await;

    let (done, outcome) = outcome();
    runner.reshape(split_off_in_the_gap(&[OUT_EAST]), done);
    step_to(&mut runner, Stage::Closing);
    gate.release_loads();
    gate.release_claims();
    let Reshaped::Split { region, part, .. } = reshaped(&mut runner, &outcome) else {
        panic!("no split");
    };
    // Granted by the answer that waited, to the part; and not kept from the answer
    // before it, neither with the part nor by the runner of the region.
    assert_eq!(part.region.knowledge(BEYOND), Knowledge::Held);
    assert_eq!(runner.region().knowledge(BEYOND), Knowledge::Unknown);
    let with_the_part: Vec<_> = part.chunks.iter().map(|(position, _)| *position).collect();
    assert_eq!(with_the_part, [OUT_EAST]);
    let warm: Vec<_> = runner.warm.keys().copied().collect();
    assert_eq!(warm, [ORIGIN]);

    // The part serves the chunk its player stands in from memory, and asks the
    // store for the other.
    let (handle, _) = world.store.open_region(world.hello(region, 5)).unwrap();
    let (store, gate) = gate_before(handle);
    let mut second = RegionRunner::of_part_with(part, store).without_rings();
    let (end, worker_end) = link::in_process(256);
    let mut there = TestEdge::silent(end, edge.edge, edge.start);
    second.links().attach(worker_end);
    there
        .send(there.hello(0, &[other_player()], &[OUT_EAST, BEYOND]))
        .await
        .unwrap();
    assert_eq!(
        answers(&mut second, &mut there, 2),
        [(OUT_EAST, 0, Told::Snapshot), (BEYOND, 0, Told::Snapshot)]
    );
    let asked = gate.asked();
    let loads: Vec<_> = asked
        .iter()
        .filter(|asked| matches!(asked, Asked::Load(_)))
        .collect();
    assert_eq!(loads, [&Asked::Load(BEYOND)]);
}

/// A split that the store has another id for is worked out again from the same
/// answers, which are still where they were: on land that is nobody's until it is
/// claimed, the grant ahead of the player who goes is the part's under the id the
/// store gave, and the one beside those who stay is the region's.
#[tokio::test]
async fn a_split_that_is_made_again_with_another_id_takes_the_grants_that_wait_with_it() {
    let world = Divided::gap();
    let (mut edge, worker_end) = in_process(256);
    let (mut runner, gate) = world.gated(HOME, settings(0));
    runner.links().attach(worker_end);
    two_players_apart_in_the_gap(&mut runner, &mut edge).await;
    asked_and_not_told(&mut runner, &gate, &edge, &[BEYOND, BESIDE]).await;

    // The id named is the home region's own.
    let (done, outcome) = outcome();
    let own = Reshape::SplitOff {
        chunks: vec![OUT_EAST],
        as_epoch: 5,
        part: HOME,
    };
    runner.reshape(own, done);
    step_to(&mut runner, Stage::Settling);
    gate.release_claims();
    gate.asked();
    let Reshaped::Split { region, part, .. } = reshaped(&mut runner, &outcome) else {
        panic!("no split");
    };
    assert_eq!(region, GAP_FIRST_PART);
    let tick = runner.region().tick_number();
    let asked = gate.asked();
    let splits: Vec<_> = asked
        .iter()
        .filter(|asked| matches!(asked, Asked::Split(..)))
        .collect();
    assert_eq!(
        splits,
        [&Asked::Split(tick, HOME), &Asked::Split(tick, region)]
    );

    assert_eq!(part.region.knowledge(BEYOND), Knowledge::Held);
    assert_eq!(runner.region().knowledge(BEYOND), Knowledge::Unknown);
    assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Held);
    assert_eq!(part.region.knowledge(BESIDE), Knowledge::Unknown);
    assert_eq!(bounds_of(&world.store, region), Some((OUT_EAST, BEYOND)));
    assert_eq!(bounds_of(&world.store, HOME), Some((ORIGIN, BESIDE)));
    // Each of the two is in memory what a runner that is started anew restores
    // from the store's record: the region holds what it was told it holds and the
    // grant that waited beside it, and the part its two chunks.
    let split = runner.region().clone();
    drop(runner);
    for (id, epoch, in_memory) in [(HOME, 2, &split), (region, 5, &part.region)] {
        let (handle, restored) = world.open(id, epoch);
        // Each with the block of keys its hello gave (`docs/adr/0028-entities.md`,
        // section 1).
        let block = restored.entity_ids;
        let next = RegionRunner::restore(settings(0), handle, restored).unwrap();
        assert_eq!(
            *next.region(),
            with_keys_of_region(in_memory, block),
            "{id:?}"
        );
    }
    assert_eq!(split.held_chunk_count(), 2);
    assert_eq!(part.region.held_chunk_count(), 2);
}

// -----------------------------------------------------------------------------------
// What an edge asks for before it has heard of a split: section 3.6.2 of
// `docs/adr/0017-the-end-of-the-stripes.md`.
// -----------------------------------------------------------------------------------

/// A chunk further east than [`BEYOND`], nobody's as well.
const FURTHER: ChunkPos = ChunkPos::new(9, 0);

/// The home region of [`Divided::gap`] with [`other_player`] split off it where
/// they stand in [`OUT_EAST`], the part's id, the edge whose link the split has
/// closed, and the number of the entry that tells it of the split.
async fn split_in_the_gap(
    world: &Divided,
) -> (RegionRunner, Arc<GateControl>, TestEdge, RegionId, u64) {
    let (mut edge, worker_end) = in_process(256);
    let (mut runner, gate) = world.gated(HOME, settings(0));
    runner.links().attach(worker_end);
    two_players_apart_in_the_gap(&mut runner, &mut edge).await;
    let (done, outcome) = outcome();
    runner.reshape(split_off_in_the_gap(&[OUT_EAST]), done);
    let Reshaped::Split { region, .. } = reshaped(&mut runner, &outcome) else {
        panic!("no split");
    };
    let state = runner.region().edge(edge.edge).unwrap();
    let entries: Vec<_> = state.outbox.iter().collect();
    let [(number, Durable::SplitOff { region: named, .. })] = entries[..] else {
        panic!("the outbox has not the one entry of the split: {entries:?}");
    };
    assert_eq!(*named, region);
    let number = *number;
    (runner, gate, edge, region, number)
}

/// The chunks the runner has claimed at the store since the gate was last asked
/// what it was asked for.
fn claimed(gate: &GateControl) -> BTreeSet<ChunkPos> {
    let claims = gate.asked().into_iter().filter_map(|asked| match asked {
        Asked::Claim(chunks) => Some(chunks),
        _ => None,
    });
    claims.flatten().collect()
}

/// The lines about chunks taken for a part's among `lines`.
fn lines_of_the_parts(lines: &[String]) -> Vec<&str> {
    let about = |line: &&String| line.starts_with("chunks asked for players who went");
    lines.iter().filter(about).map(String::as_str).collect()
}

/// An edge that has not read that one of its players was split off links again
/// and names what both of them see as this region's to show. What is on the
/// part's side of the split's line and the region knows nothing of, it takes for
/// the part's: the chunk that went, and the chunk ahead that nobody holds, which
/// the region would otherwise be granted and the player handed back into. It
/// claims neither and says of each that the part holds it. A chunk on its own
/// side it claims as ever.
#[tokio::test]
async fn what_an_edge_asks_for_those_who_went_is_not_claimed_before_it_has_heard_of_the_split() {
    let world = Divided::gap();
    let (mut runner, gate, edge, part, _) = split_in_the_gap(&world).await;
    gate.asked();

    let (edge_end, worker_end) = link::in_process(256);
    let mut again = edge.again(edge_end, &runner);
    runner.links().attach(worker_end);
    let sees = [ORIGIN, BESIDE, OUT_EAST, BEYOND];
    let hello = again.hello(0, &[player(), other_player()], &sees);
    again.send(hello).await.unwrap();
    let ((), lines) = logged(|| step(&mut runner));
    let link = runner.links.keys().next().unwrap().0;
    assert_eq!(
        lines_of_the_parts(&lines),
        [format!(
            "chunks asked for players who went are taken for the part's link={link} \
                 part={} chunks=2 free=1",
            part.0
        )]
    );
    for chunk in [OUT_EAST, BEYOND] {
        assert_eq!(runner.region().knowledge(chunk), Knowledge::Foreign(part));
    }
    assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Asked);
    assert_eq!(claimed(&gate), BTreeSet::from([BESIDE]));
    // The edge is told of the split before it is told who holds the chunks.
    let said = again.everything();
    let told = |wanted: &dyn Fn(&WorkerToEdge) -> bool| said.iter().position(wanted);
    let of_the_split = told(&|message| {
        matches!(
            message,
            WorkerToEdge::Outbox {
                entry: Durable::SplitOff { .. },
                ..
            }
        )
    });
    for chunk in [OUT_EAST, BEYOND] {
        let elsewhere = WorkerToEdge::Elsewhere {
            chunk,
            ask: 0,
            region: part,
        };
        let of_the_chunk = told(&|message| *message == elsewhere);
        assert!(
            of_the_split.is_some() && of_the_split < of_the_chunk,
            "{said:?}"
        );
    }

    // A chunk it asks for afterwards, still without having confirmed the entry,
    // is taken the same way; and none of it is ever claimed while it is so.
    again.send(asking_for(vec![FURTHER])).await.unwrap();
    let ((), lines) = logged(|| {
        step_until(&mut runner, |runner| {
            runner.region().knowledge(FURTHER) != Knowledge::Unknown
        })
    });
    assert_eq!(runner.region().knowledge(FURTHER), Knowledge::Foreign(part));
    assert_eq!(lines_of_the_parts(&lines).len(), 1, "{lines:?}");
    assert_eq!(claimed(&gate), BTreeSet::new());
}

/// A belief the runner put in is its subscription's. An edge that lost its link
/// before it read of the split links again within the same tick and names the
/// same chunks: the belief that the first link's end takes back is kept for the
/// second link's subscription, and the row ahead is still not claimed. When that
/// link ends as well and nobody asks again, the belief goes with it.
#[tokio::test]
async fn a_belief_is_kept_for_an_edge_that_links_again_before_it_has_heard_and_goes_with_its_link()
{
    let world = Divided::gap();
    let (mut runner, gate, edge, part, _) = split_in_the_gap(&world).await;
    let sees = [ORIGIN, OUT_EAST, BEYOND];
    let players = [player(), other_player()];

    let (edge_end, worker_end) = link::in_process(256);
    let first = edge.again(edge_end, &runner);
    runner.links().attach(worker_end);
    first.send(first.hello(0, &players, &sees)).await.unwrap();
    step(&mut runner);
    assert_eq!(runner.region().knowledge(BEYOND), Knowledge::Foreign(part));
    gate.asked();

    // The link is lost and another is there before the next tick.
    let (edge_end, worker_end) = link::in_process(256);
    let second = edge.again(edge_end, &runner);
    runner.links().attach(worker_end);
    drop(first);
    second.send(second.hello(0, &players, &sees)).await.unwrap();
    step(&mut runner);
    step(&mut runner);
    for chunk in [OUT_EAST, BEYOND] {
        assert_eq!(runner.region().knowledge(chunk), Knowledge::Foreign(part));
    }
    assert_eq!(claimed(&gate), BTreeSet::new());

    // With the last link that asked for them the beliefs go, and nothing is
    // claimed for chunks nobody asks for.
    drop(second);
    step_until(&mut runner, |runner| {
        runner.region().knowledge(BEYOND) == Knowledge::Unknown
    });
    assert_eq!(runner.region().knowledge(OUT_EAST), Knowledge::Unknown);
    assert_eq!(claimed(&gate), BTreeSet::new());
}

/// An edge that says with its hello that it has the entry of the split has read
/// it, on a link before this one, and has moved the view of whoever went: what it
/// names as a viewer's is the view of somebody who stayed, and is claimed as ever.
/// The entry is still in the region's state then, as the tick that takes the
/// hello is the one that takes it out.
#[tokio::test]
async fn what_an_edge_asks_for_when_it_has_heard_of_the_split_is_claimed_as_ever() {
    let world = Divided::gap();
    let (mut runner, gate, edge, _, number) = split_in_the_gap(&world).await;
    gate.asked();

    let (edge_end, worker_end) = link::in_process(256);
    let again = edge.again(edge_end, &runner);
    runner.links().attach(worker_end);
    let hello = again.hello(number, &[player()], &[ORIGIN, BEYOND]);
    again.send(hello).await.unwrap();
    let ((), lines) = logged(|| step(&mut runner));
    assert_eq!(lines_of_the_parts(&lines), [""; 0]);
    assert_eq!(runner.region().knowledge(BEYOND), Knowledge::Asked);
    assert_eq!(claimed(&gate), BTreeSet::from([BEYOND]));
}

/// When the edge has confirmed the entry, the region has nothing left to go by:
/// the split is forgotten with the entry, and what the edge asks for is claimed
/// as ever, also on what was the part's side. A confirmation that is taken in the
/// same step as the asking counts already, though only the coming tick takes the
/// entry out of the region's state.
#[tokio::test]
async fn a_split_that_every_edge_has_confirmed_is_forgotten() {
    let world = Divided::gap();
    let (mut runner, gate, edge, _, number) = split_in_the_gap(&world).await;
    assert_eq!(runner.parted.len(), 1);

    let (edge_end, worker_end) = link::in_process(256);
    let again = edge.again(edge_end, &runner);
    runner.links().attach(worker_end);
    // Only what the region holds, so that nothing the edge says next waits behind
    // the hello for longer than the chunk takes to be shown.
    let hello = again.hello(0, &[player(), other_player()], &[ORIGIN]);
    again.send(hello).await.unwrap();
    step_until(&mut runner, |runner| {
        runner.links.values().all(|link| link.hold.is_empty())
    });
    assert_eq!(runner.parted.len(), 1);

    gate.asked();
    again.send(EdgeToWorker::Confirm { number }).await.unwrap();
    again.send(asking_for(vec![BEYOND])).await.unwrap();
    let ((), lines) = logged(|| {
        step_until(&mut runner, |runner| {
            runner.region().knowledge(BEYOND) != Knowledge::Unknown
        })
    });
    assert_eq!(lines_of_the_parts(&lines), [""; 0]);
    assert!(claimed(&gate).contains(&BEYOND));
    step(&mut runner);
    assert!(runner.region().edge(edge.edge).unwrap().outbox.is_empty());
    assert!(runner.parted.is_empty());
}

// -----------------------------------------------------------------------------------
// How long a region stood still for a merge or a split: section 5.6 of
// `docs/adr/0017-the-end-of-the-stripes.md`.
// -----------------------------------------------------------------------------------

/// A time the tests below have a region stand still for at least, by sleeping
/// while it has stopped. What it is told has to be no less; how much more it is,
/// is the machine's.
const STOOD: Duration = Duration::from_millis(30);

/// The western stripe of `world` on a runner that is told to say how long its
/// region stood still, with a link, the gate before its store, and where what it
/// says shows.
fn west_that_tells(
    world: &Divided,
) -> (
    RegionRunner,
    TestEdge,
    Arc<GateControl>,
    Receiver<Standstill>,
) {
    let (edge, worker_end) = in_process(256);
    let (west, gate) = world.gated(RegionId(0), settings(0));
    let (said, told) = mpsc::channel();
    let tell = move |standstill| {
        // Whoever listened may have stopped looking.
        let _ = said.send(standstill);
    };
    let west = west.with_standstills(Box::new(tell));
    west.links().attach(worker_end);
    (west, edge, gate, told)
}

/// The players and the held chunks of the region of `runner` as it is.
fn players_and_held(runner: &RegionRunner) -> (u64, u64) {
    let region = runner.region();
    (
        region.player_count() as u64,
        region.held_chunk_count() as u64,
    )
}

/// Steps `runner`, whose merge or split has just had its outcome, once more, which
/// runs its next tick, and returns the one thing it tells then. Nothing was told
/// before that tick, and nothing is in the ticks that follow.
fn told_as_it_ticks_on(runner: &mut RegionRunner, told: &Receiver<Standstill>) -> Standstill {
    assert_eq!(runner.stage(), None);
    let before: Vec<Standstill> = told.try_iter().collect();
    assert_eq!(before, [], "told before the region had ticked on");
    let tick = runner.region().tick_number();
    runner.step();
    assert_eq!(runner.region().tick_number(), tick + 1);
    let at_the_tick: Vec<Standstill> = told.try_iter().collect();
    let [standstill] = at_the_tick[..] else {
        panic!("told {at_the_tick:?} at the first tick after");
    };
    for _ in 0..5 {
        step(runner);
    }
    assert_eq!(told.try_iter().count(), 0, "told a second time");
    standstill
}

/// A split that is made: the runner says nothing when it has the outcome, and with
/// its next tick, once, how long the region did not tick, with the players and the
/// chunks it had when it stopped, those that went with the part among them.
#[tokio::test]
async fn a_runner_tells_how_long_its_region_stood_still_for_a_split_once_it_ticks_on() {
    let world = Divided::stripes();
    let (mut west, mut edge, _gate, told) = west_that_tells(&world);
    two_players_apart(&mut west, &mut edge).await;

    let (done, outcome) = outcome();
    west.reshape(split_off(&[FAR_WEST]), done);
    // The region still ticks until the store has the first checkpoint.
    step_to(&mut west, Stage::Settling);
    let as_it_stopped = players_and_held(&west);
    assert_eq!(as_it_stopped, (2, 3));
    thread::sleep(STOOD);
    let Reshaped::Split { .. } = reshaped(&mut west, &outcome) else {
        panic!("no split");
    };
    assert_eq!(players_and_held(&west), (1, 2));

    let standstill = told_as_it_ticks_on(&mut west, &told);
    assert_eq!((standstill.players, standstill.held), as_it_stopped);
    assert!(
        u128::from(standstill.milliseconds) >= STOOD.as_millis(),
        "{standstill:?}"
    );
}

/// A merge that is made: the same, with the survivor's players and chunks as they
/// were before those of the absorbed region came.
#[tokio::test]
async fn a_runner_tells_how_long_its_region_stood_still_for_a_merge_once_it_ticks_on() {
    let world = Divided::stripes();
    let (mut west, mut edge, _gate, told) = west_that_tells(&world);
    look_east(&mut west, &mut edge);
    joined(&edge, &mut west).await;
    let east = released_east(&world, &edge).await;
    step(&mut west);

    let (done, outcome) = outcome();
    west.reshape(east.absorb(), done);
    step_to(&mut west, Stage::Settling);
    let as_it_stopped = players_and_held(&west);
    assert_eq!(as_it_stopped.0, 1);
    let absorbed = Reshaped::Absorbed { absorbed: EAST };
    assert_eq!(reshaped(&mut west, &outcome), absorbed);
    // Two players came. No chunk did: what the absorbed region held was of the
    // area it was pinned to, which is the survivor's to claim from here on.
    assert_eq!(players_and_held(&west), (3, as_it_stopped.1));

    let standstill = told_as_it_ticks_on(&mut west, &told);
    assert_eq!((standstill.players, standstill.held), as_it_stopped);
}

/// A merge that the store declines and a split that is off when it is worked out
/// have stood the region still as well, from the first checkpoint on: each is told
/// with the first tick after, like one that was made.
#[tokio::test]
async fn a_runner_tells_of_a_merge_that_is_declined_and_of_a_split_that_is_off() {
    // Not the epoch the region to absorb is open with.
    let world = Divided::stripes();
    let (mut west, mut edge, _gate, told) = west_that_tells(&world);
    two_players_apart(&mut west, &mut edge).await;
    let east = untouched_east(&world);
    let reshape = Reshape::Absorb {
        absorbed: EAST,
        absorbed_epoch: 3,
        state: east.state.clone(),
    };
    let (done, outcome) = outcome();
    west.reshape(reshape, done);
    let declined = Reshaped::Off {
        why: Off::Declined(Decline::NotOpened { epoch: Some(2) }),
    };
    assert_eq!(reshaped(&mut west, &outcome), declined);
    let standstill = told_as_it_ticks_on(&mut west, &told);
    assert_eq!((standstill.players, standstill.held), (2, 3));

    // The same runner, a while later: nobody stands in the chunk named.
    let (done, outcome) = self::outcome();
    west.reshape(split_off(&[WEST_OF_HOME]), done);
    step_to(&mut west, Stage::Settling);
    thread::sleep(STOOD);
    let off = Reshaped::Off { why: Off::Nobody };
    assert_eq!(reshaped(&mut west, &outcome), off);
    let standstill = told_as_it_ticks_on(&mut west, &told);
    assert_eq!((standstill.players, standstill.held), (2, 3));
    assert!(
        u128::from(standstill.milliseconds) >= STOOD.as_millis(),
        "{standstill:?}"
    );
}

/// A runner that is not asked to tell notes nothing when its region stops. One
/// that is asked tells nothing of a release, after which the region never ticks
/// on, and nothing if it ends in the middle of a split: with the store lost, or
/// stopped while the store has the split.
#[tokio::test]
async fn a_runner_tells_nothing_of_a_release_nor_when_it_ends_in_the_middle() {
    let world = Divided::stripes();
    let (mut edge, worker_end) = in_process(256);
    let (mut west, _gate) = world.gated(RegionId(0), settings(0));
    west.links().attach(worker_end);
    two_players_apart(&mut west, &mut edge).await;
    let (done, _outcome) = outcome();
    west.reshape(split_off(&[FAR_WEST]), done);
    step_to(&mut west, Stage::Settling);
    assert!(west.stopped.is_none());

    let world = Divided::stripes();
    let (mut west, mut edge, _gate, told) = west_that_tells(&world);
    two_players_apart(&mut west, &mut edge).await;
    west.begin_release();
    step_to(&mut west, Stage::Settling);
    assert!(west.stopped.is_none());
    assert_eq!(released(&mut west), Ended::Released);
    west.step();
    assert_eq!(told.try_iter().count(), 0);

    let world = Divided::stripes();
    let (mut west, mut edge, gate, told) = west_that_tells(&world);
    two_players_apart(&mut west, &mut edge).await;
    let (done, outcome) = self::outcome();
    west.reshape(split_off(&[FAR_WEST]), done);
    step_to(&mut west, Stage::Settling);
    assert!(west.stopped.is_some());
    gate.lose();
    west.step();
    let lost = Reshaped::Off {
        why: Off::StoreLost,
    };
    assert_eq!(outcome.try_recv(), Ok(lost.clone()));
    assert_eq!(west.ended(), Some(Ended::StoreLost));
    west.step();
    assert_eq!(told.try_iter().count(), 0);

    let world = Divided::stripes();
    let (mut west, mut edge, gate, told) = west_that_tells(&world);
    two_players_apart(&mut west, &mut edge).await;
    gate.hold_reshapes();
    let (done, outcome) = self::outcome();
    west.reshape(split_off(&[FAR_WEST]), done);
    step_to(&mut west, Stage::Committing);
    assert_eq!(west.run(&AtomicBool::new(true)), Ended::Abandoned);
    assert_eq!(outcome.try_recv(), Ok(lost));
    west.step();
    assert_eq!(told.try_iter().count(), 0);
}

// -----------------------------------------------------------------------------------
// The split that leaves no land ahead of those who go, and the standstill: the
// scenarios R1 to R12 of section 9.5a of `docs/adr/0017-the-end-of-the-stripes.md`,
// written from its sections 3.6 and 5.6 by someone who has not read how the runner
// does it. They are a module of their own so that no name here stands in the way
// of one above.
//
// A test that is marked `ignore` with "finding" says what the record says and
// fails: it stays as it is until the runner or the record is changed.
// -----------------------------------------------------------------------------------

mod stripes_end;

mod busy_circuits;
mod entities;
mod generation;
mod lends;
mod scheduled_ticks;
mod stays;
