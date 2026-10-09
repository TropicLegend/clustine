//! A worker as a process of its own: the regions it is given, opened at the world
//! store and run, and what it tells the coordinator of them.

use std::collections::BTreeMap;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::task::Poll;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use clustine_coordinator::{ClientError, Orders, Reach, WorkerClient, WorkerEvent};
use clustine_region::RegionId;
use clustine_rpc::{
    Assignment, EdgeMessage, Off, PlayersOf, RegionHello, Restored, Vouch, WorkerToEdge, tcp,
};
use clustine_sim::{Part, RegionConfig, RegionState};
use clustine_worker::{
    DEFAULT_RETURN_AFTER, Ended, Links, RegionRunner, RegionStatus, Reshape, Reshaped,
    RestoreError, Standstill, Worker, absorbable,
};
use clustine_world::{EntityId, EntityIds};
use clustine_worldstore::{StoreError, StoreHandle};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio::time::sleep;
use tracing::{debug, info, warn};

use super::{RETRY, sleep_until_some};
use crate::{LINK_CAPACITY, starting_hotbar};

/// How often a worker looks at its regions: whether each still ticks, whether it has
/// lost the world store, and where its players are. What the worker vouches for to the
/// coordinator follows from the first two, and the last it tells the coordinator each
/// time.
const LOOK: Duration = Duration::from_millis(250);

/// A worker vouches for a region as committed if it has ticked within this long. A region
/// whose commits go unanswered stops ticking within a few ticks, so ticking is what
/// shows that the store confirms what the region does.
const TICKED_WITHIN: Duration = Duration::from_secs(1);

/// Settings of a worker process.
#[derive(Debug, Clone)]
pub struct WorkerArgs {
    /// Host and port of the coordinator.
    pub coordinator: String,
    /// Host and port of the world store.
    pub store: String,
    /// The address edges connect to.
    pub listen: SocketAddr,
    /// Host and port under which edges reach `listen`.
    pub advertise: String,
    /// Identifies the worker to the coordinator across restarts.
    pub name: String,
    /// How often every changed chunk that is still loaded is saved.
    pub checkpoint_interval: Duration,
}

/// A region this worker holds, and what it takes to run it: one the coordinator has
/// given it, or one it has split off another.
#[derive(Debug, Clone)]
struct Held {
    assignment: Assignment,
    hello: RegionHello,
    config: RegionConfig,
}

type Opening = Pin<Box<dyn Future<Output = Result<(StoreHandle, Restored), StoreError>> + Send>>;

/// How often a worker that is releasing a region looks whether the release is done.
/// The region's players stand still meanwhile, so this is not left to [`LOOK`].
const RELEASE_LOOK: Duration = Duration::from_millis(5);

/// How long a worker that has been told to stop waits for its regions to be handed to
/// other workers before it stops anyway. Kubernetes gives it 30 seconds in all.
const LEAVE_WITHIN: Duration = Duration::from_secs(20);

/// The entity ids of a region that was split off another: none, as the world store
/// says of such a region and as the coordinator's orders name it. Players enter the
/// world in the home region, which is never the part of a split.
const NO_ENTITY_IDS: EntityIds = EntityIds {
    first: EntityId(0),
    end: EntityId(0),
};

/// What a worker is doing about a region it holds.
enum Phase {
    /// It waits for the world store to open the region, for the first time or again.
    Opening { held: Held, opening: Opening },
    /// The region was split off another one here a moment ago, and waits in memory
    /// for the world store to answer the hello for it. Nothing of it is shown to
    /// anyone before that, so a part that is lost with this worker has shown nobody
    /// what the store's record of the split does not have.
    Starting {
        held: Held,
        part: Box<Part>,
        opening: Opening,
    },
    /// The region is restored and ticks.
    Running {
        held: Held,
        running: Worker,
        /// Where the links of edges are attached.
        links: Links,
        status: Arc<RegionStatus>,
        /// The region's tick at the last look, and when it was last seen to have changed.
        tick: u64,
        ticked: Instant,
    },
    /// The coordinator asked for the region to be released, and the runner is at it;
    /// see `docs/adr/0009-moving-a-region.md`.
    Releasing {
        held: Held,
        running: Worker,
        status: Arc<RegionStatus>,
    },
}

impl Phase {
    /// What the worker says of the region to the coordinator in this phase, if it
    /// vouches for it.
    fn vouch(&self) -> Option<(RegionId, Vouch)> {
        match self {
            Phase::Opening { held, .. } | Phase::Starting { held, .. } => {
                Some((held.assignment.region, Vouch::WaitingForStore))
            }
            Phase::Running { held, ticked, .. } if ticked.elapsed() < TICKED_WITHIN => {
                Some((held.assignment.region, Vouch::Committed))
            }
            // A region that has stopped ticking is not vouched for: the store does not
            // confirm what it does, and yet has not let go of it. One that stands still
            // for a merge or a split is reserved by the coordinator meanwhile, which
            // counts as vouched for.
            Phase::Running { .. } => None,
            // It stops ticking on purpose, and the coordinator knows.
            Phase::Releasing { .. } => None,
        }
    }

    fn held(&self) -> &Held {
        match self {
            Phase::Opening { held, .. }
            | Phase::Starting { held, .. }
            | Phase::Running { held, .. }
            | Phase::Releasing { held, .. } => held,
        }
    }
}

/// The regions a worker holds, each with what it is doing about it. A region that it
/// has open only to have another absorb it is not among them: it is not vouched for,
/// not served to edges and not reported when the worker registers.
type Regions = BTreeMap<RegionId, Phase>;

/// Why a region that was to be absorbed is not there to be.
enum Unabsorbable {
    /// The world store did not let this worker open it.
    Store(StoreError),
    /// What the world store has of it cannot be read.
    Restore(RestoreError),
}

/// A region that is to be absorbed, opened at the world store and read: its handle and
/// its whole state.
type Fetched = Result<(StoreHandle, RegionState), Unabsorbable>;

type Fetching = Pin<Box<dyn Future<Output = Fetched> + Send>>;

/// A merge or a split that a region of this worker is in the middle of; see
/// `docs/adr/0014-merging-and-splitting.md`, section 4.
struct Reshaping {
    /// Tells it from an earlier one of the same region. The outcome of that one can
    /// still be on its way when this one has begun: a runner that is stopped says
    /// that nothing came of what it was at.
    number: u64,
    /// What the worker held the region as when it was asked.
    held: Held,
    doing: Doing,
}

/// How far a worker is with a merge or a split.
enum Doing {
    /// The region to absorb is being opened at the world store and read. The runner
    /// knows nothing of the merge yet.
    Fetching {
        absorbed: RegionId,
        as_epoch: u64,
        fetching: Fetching,
    },
    /// The runner has been handed the merge.
    Absorbing {
        absorbed: RegionId,
        as_epoch: u64,
        /// The region to absorb, kept open until the outcome is there: the store
        /// declines a merge whose absorbed region has no owner with that epoch.
        #[allow(dead_code)] // Held, never looked at.
        handle: StoreHandle,
    },
    /// The runner has been handed the split.
    Splitting { as_epoch: u64 },
}

impl Reshaping {
    /// Whether this is the merge that an order to absorb `absorbed`, opened with
    /// `as_epoch`, asks for.
    fn absorbs(&self, region: RegionId, epoch: u64) -> bool {
        match &self.doing {
            Doing::Fetching {
                absorbed, as_epoch, ..
            }
            | Doing::Absorbing {
                absorbed, as_epoch, ..
            } => (*absorbed, *as_epoch) == (region, epoch),
            Doing::Splitting { .. } => false,
        }
    }
}

/// The merges and splits under way, by the region that absorbs or is split, which has
/// one at a time. It is kept apart from what the worker is doing about the region: a
/// region can be asked to be released, or lose the store, while its runner is at it,
/// and the outcome comes all the same.
type Reshapes = BTreeMap<RegionId, Reshaping>;

/// The outcome of a merge or a split as a region's thread sends it: the region, the
/// number of its [`Reshaping`], and what the runner says.
type Reshapings = mpsc::UnboundedSender<(RegionId, u64, Reshaped)>;

/// What the worker's loop has for the coordinator that is said in the order it was
/// made: what came of a merge or a split, and where the players are.
///
/// The two go down one queue because the coordinator takes what it is told of the
/// players behind the word of a merge or a split to be of the regions as they are after
/// it (`docs/adr/0016-when-to-merge-and-split.md`, section 2.2). Down two queues a
/// report read before a split could follow the word of it.
#[derive(Debug, Clone, PartialEq)]
enum Outcome {
    /// For [`WorkerClient::absorb_ended`].
    Merge {
        region: RegionId,
        absorbed: RegionId,
        outcome: Result<(), Off>,
    },
    /// For [`WorkerClient::split_ended`].
    Split {
        region: RegionId,
        as_epoch: u64,
        outcome: Result<RegionId, Off>,
    },
    /// For [`WorkerClient::players`]: where the players of the regions were that the
    /// worker ran at a look. Unlike the other two it is only good on the connection it
    /// was read under, and is dropped on any other.
    Players {
        /// The number of the registration under which it was read.
        registration: u64,
        regions: Vec<PlayersOf>,
    },
}

/// What a worker waited for and got; see [`settled`].
enum Settled {
    Opened(Result<(StoreHandle, Restored), StoreError>),
    Released(Ended),
    /// The region that this one is to absorb has been opened and read, or cannot be.
    Fetched(Fetched),
}

/// Resolves when what the worker waits for with one of its regions has come about: the
/// world store has answered the opening of the region, or of the region it is to
/// absorb, or a release has ended. Never while it waits for none of them. The phase of
/// that region has to be left once this has resolved, or, for a region to absorb, that
/// stage of the merge.
async fn settled(regions: &mut Regions, reshapes: &mut Reshapes) -> (RegionId, Settled) {
    loop {
        let mut releasing = false;
        for (region, phase) in regions.iter() {
            if let Phase::Releasing { status, .. } = phase {
                if let Some(ended) = status.ended() {
                    return (*region, Settled::Released(ended));
                }
                releasing = true;
            }
        }
        let answered = std::future::poll_fn(|context| {
            for (region, phase) in regions.iter_mut() {
                if let Phase::Opening { opening, .. } | Phase::Starting { opening, .. } = phase
                    && let Poll::Ready(opened) = opening.as_mut().poll(context)
                {
                    return Poll::Ready((*region, Settled::Opened(opened)));
                }
            }
            for (region, reshaping) in reshapes.iter_mut() {
                if let Doing::Fetching { fetching, .. } = &mut reshaping.doing
                    && let Poll::Ready(fetched) = fetching.as_mut().poll(context)
                {
                    return Poll::Ready((*region, Settled::Fetched(fetched)));
                }
            }
            Poll::Pending
        });
        tokio::select! {
            answered = answered => return answered,
            () = sleep(RELEASE_LOOK), if releasing => {}
        }
    }
}

/// What the worker vouches for to the coordinator.
fn vouches(regions: &Regions) -> Vec<(RegionId, Vouch)> {
    regions.values().filter_map(Phase::vouch).collect()
}

/// What the worker holds, as it says when it registers again.
fn holdings(regions: &Regions) -> Vec<(Assignment, u64)> {
    let held = regions.values().map(Phase::held);
    held.map(|held| (held.assignment, held.hello.layout))
        .collect()
}

/// Where the players of the regions this worker runs are, for the coordinator: of every
/// region that is restored, also of one without players, and of none that is being
/// opened, started from a split or released. See
/// `docs/adr/0016-when-to-merge-and-split.md`, sections 2.1 and 2.2.
///
/// A region that stands still is among them, with the tick it stopped at. The tick is
/// read before the crowds, as the runner stores them: what is said with a tick is of
/// that tick, of the one before it if the look fell between the runner's two stores, or
/// of a later one. Nothing rests on which.
fn players(regions: &Regions) -> Vec<PlayersOf> {
    let mut sighted = Vec::new();
    for (region, phase) in regions {
        if let Phase::Running { held, status, .. } = phase {
            let tick = status.tick.load(Ordering::Relaxed);
            sighted.push(PlayersOf {
                region: *region,
                epoch: held.assignment.epoch,
                tick,
                crowds: status.crowds(),
            });
        }
    }
    sighted
}

/// Queues where the players of `regions` are for the coordinator, behind what came of
/// every merge and split the loop has heard of, and under `registration`, the number
/// of the registration whose connection there is. Without a connection it queues
/// nothing: the queue has no bound, and four reports a second for as long as a
/// coordinator is away would fill it.
fn report(endings: &mpsc::UnboundedSender<Outcome>, registration: Option<u64>, regions: &Regions) {
    if let Some(registration) = registration {
        let regions = players(regions);
        // Nobody takes it if the task that holds the connection has ended.
        let _ = endings.send(Outcome::Players {
            registration,
            regions,
        });
    }
}

/// The regions edges can link to: those that are restored and tick. A region that
/// absorbs another or is split stays among them, under the hello it had: an edge
/// whose link the merge or the split closed is let in again at once.
fn served(regions: &Regions) -> Serving {
    let mut serving = BTreeMap::new();
    for (region, phase) in regions {
        if let Phase::Running { held, links, .. } = phase {
            serving.insert(*region, (held.hello, links.clone()));
        }
    }
    Arc::new(serving)
}

/// What edges can link to at a worker: for each region it runs, what a link has to ask
/// for and where it is attached.
pub(crate) type Serving = Arc<BTreeMap<RegionId, (RegionHello, Links)>>;

/// Whether `orders` name the region of `assignment` with its epoch. Assignments are
/// told apart by those two and not by their entity ids, which nothing reads: the
/// orders that first name a region this worker split off another name it without any.
fn names(orders: &Orders, assignment: &Assignment) -> bool {
    let mut ordered = orders.assignments.iter();
    ordered.any(|ordered| (ordered.region, ordered.epoch) == (assignment.region, assignment.epoch))
}

/// The region `region` as this worker runs it with `epoch`, with the thread it runs
/// on, if it does and the region is in the middle of nothing; or why the region can
/// neither absorb another nor be split now.
fn free_to_reshape<'a>(
    regions: &'a Regions,
    reshapes: &Reshapes,
    region: RegionId,
    epoch: u64,
) -> Result<(&'a Held, &'a Worker), Off> {
    match regions.get(&region) {
        Some(Phase::Running { held, running, .. }) if held.assignment.epoch == epoch => {
            if reshapes.contains_key(&region) {
                Err(Off::Busy)
            } else {
                Ok((held, running))
            }
        }
        Some(Phase::Releasing { held, .. }) if held.assignment.epoch == epoch => Err(Off::Busy),
        // Not this worker's, not with that epoch, or not open at the store yet.
        _ => Err(Off::NotRunning),
    }
}

/// The call a runner makes with the outcome of the merge or the split numbered
/// `number` of `region`. It is made on the region's thread and must not wait, so all
/// it does is pass the outcome on to the worker, which acts on it at once: nothing a
/// player waits for hangs on [`LOOK`].
fn passing_on(
    outcomes: &Reshapings,
    region: RegionId,
    number: u64,
) -> Box<dyn FnOnce(Reshaped) + Send> {
    let outcomes = outcomes.clone();
    Box::new(move |outcome| {
        // Nobody takes it if the worker is stopping.
        let _ = outcomes.send((region, number, outcome));
    })
}

/// What the task that holds the connection to the coordinator passes on.
enum Word {
    /// New orders, a region to release, or what is to be done for a merge or a split.
    Event(WorkerEvent),
    /// The coordinator refuses the worker, for the reason given.
    Refused(String),
    /// A worker that is leaving may exit: the coordinator has closed its connection, or
    /// cannot be reached.
    Dismissed,
}

/// What a worker is, wherever it runs.
#[derive(Debug, Clone)]
pub(crate) struct Setup {
    /// Its name at the coordinator.
    pub name: String,
    /// Where edges are told to reach it.
    pub advertise: String,
    /// Ticks between two checkpoints of each region.
    pub checkpoint_interval: u64,
}

/// How a worker opens a region at the world store; see [`Outside::store`].
pub(crate) type Opener =
    Arc<dyn Fn(RegionHello) -> Result<(StoreHandle, Restored), StoreError> + Send + Sync>;

/// A region the world store did not let a worker open, with the epoch the store has
/// seen; see [`Outside::refusals`].
pub(crate) type Refusal = (RegionId, u64);

/// Everything a worker's loop reaches the outside through.
pub(crate) struct Outside {
    /// The registration the worker begins with: its client and its first orders.
    pub registered: (WorkerClient, Orders),
    /// Where it registers again when that connection ends.
    pub coordinator: Reach,
    /// Opens a region at the world store, or one that another is to absorb. It
    /// blocks, and is called on a thread that may. `StoreError::Io` says that the
    /// store cannot be reached yet, and the loop tries again; any other error is
    /// the store's refusal. The loop says nothing when it tries again: where the
    /// store is, and so what there is to say of it, only this knows.
    pub store: Opener,
    /// Where the loop shows the regions it serves: those that are restored and
    /// tick, each with its hello and where links to it are attached. Whoever lets
    /// edges in reads it. The loop lets go of it when it ends, before it stops its
    /// regions: a watch that is closed is a worker that serves nothing any more.
    pub serving: watch::Sender<Serving>,
    /// What the store refused, each a region and the epoch the store has seen, on
    /// its way to the coordinator as `EpochRefused`. The loop puts its own in at
    /// `refusals.0`; whoever holds a clone of that end can say one too.
    pub refusals: (
        mpsc::UnboundedSender<Refusal>,
        mpsc::UnboundedReceiver<Refusal>,
    ),
    /// The word to stop: `Leave` has the worker say that it leaves and go on until
    /// it is relieved, for twenty seconds at most; `AtOnce` stops it. A loop that
    /// nobody is left to tell goes on.
    pub stop: mpsc::UnboundedReceiver<Stop>,
}

/// How a worker's loop is told to stop; see [`Outside::stop`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stop {
    Leave,
    AtOnce,
}

/// Runs a worker until the process is asked to stop: registers with the coordinator,
/// opens the regions it is given at the world store, restores and runs each on a thread
/// of its own, and accepts the links of edges while it does.
///
/// A worker that loses the world store keeps its regions: a region that has lost it
/// closes its links, and the worker opens it again when the store answers and carries on
/// with what the store has. If the store says that a region has had a later owner, the
/// worker tells the coordinator and does not take that assignment up again. It releases
/// a region when the coordinator asks for that, and drops one that the coordinator has
/// given to another worker; either way it goes on with the others and with whatever it
/// is given next.
///
/// When the coordinator asks for it, the worker has a region it runs absorb another,
/// which it opens at the world store for that and never runs, or splits the players
/// in certain chunks off a region as a new one, which it runs from memory as soon as
/// the store has answered the hello for it. It tells the coordinator what came of
/// either. See `docs/adr/0014-merging-and-splitting.md`, section 4.
///
/// Asked to stop, it tells the coordinator that it is leaving and goes on until the
/// coordinator has moved its regions to other workers and closed the connection, for
/// [`LEAVE_WITHIN`] at most. A second signal stops it at once.
pub async fn worker(args: WorkerArgs) -> Result<()> {
    let stop = crate::stop_signal();
    tokio::pin!(stop);
    let (coordinator, first) = tokio::select! {
        registered = register(&args) => registered?,
        _ = &mut stop => return Ok(()),
    };
    // Only now, so that whoever takes "it listens" for "it is ready", as Kubernetes
    // does, takes a worker for ready that the coordinator knows.
    let listener = TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("listening on {}", args.listen))?;

    // Edges are let in to the regions that are restored, and to no others; and to none
    // once the loop has ended and shows nothing more, which closes the listener before
    // the regions are stopped, as it always was. An edge whose link a stopping region
    // closes would otherwise be let in again to a region that is gone.
    let (serving, served_to) = watch::channel(Serving::default());
    let mut shown = served_to.clone();
    let accepting = tokio::spawn(async move {
        tokio::select! {
            () = accept_edges(listener, served_to) => {}
            () = async { while shown.changed().await.is_ok() {} } => {}
        }
    });
    let address = args.store;
    let store: Opener = Arc::new(move |hello| {
        let opened = StoreHandle::connect(&address, hello);
        // Said here, where the store has an address. It is the loop that tries again.
        if let Err(StoreError::Io(error)) = &opened {
            info!(%error, store = %address, "the world store cannot be reached yet");
        }
        opened
    });
    // The first signal has the worker say that it leaves, and the second stops it at
    // once. The second is listened for when the first has come, and none after it.
    let (stopping, stopped) = mpsc::unbounded_channel();
    let signals = async {
        stop.await;
        // Nobody takes either if the loop has ended.
        let _ = stopping.send(Stop::Leave);
        crate::stop_signal().await;
        let _ = stopping.send(Stop::AtOnce);
        std::future::pending::<()>().await;
    };
    let setup = Setup {
        name: args.name,
        advertise: args.advertise,
        checkpoint_interval: (args.checkpoint_interval.as_millis()
            / clustine_worker::TICK.as_millis()) as u64,
    };
    let outside = Outside {
        registered: (coordinator, first),
        coordinator: Reach::Tcp(args.coordinator),
        store,
        serving,
        // Nobody else holds it: only the world store refuses this worker a region.
        refusals: mpsc::unbounded_channel(),
        stop: stopped,
    };
    let outcome = tokio::select! {
        outcome = run(setup, outside) => outcome,
        () = signals => unreachable!("it listens for nothing more and never ends"),
    };
    accepting.abort();
    outcome
}

/// Runs a worker until it is told to stop or cannot go on: what [`worker`] says of
/// one, from its first orders on. It is given everything it reaches the outside
/// through, and names no address, no listener and no signal; see
/// `docs/adr/0017-the-end-of-the-stripes.md`, section 6.2.
pub(crate) async fn run(setup: Setup, outside: Outside) -> Result<()> {
    let Outside {
        registered: (coordinator, first),
        coordinator: reach,
        store,
        serving,
        refusals: (refusals, refused),
        mut stop,
    } = outside;
    // What the task that holds the connection to the coordinator is told, and tells.
    let (vouching, vouched) = watch::channel(Vec::new());
    let (holding, held) = watch::channel(Vec::new());
    let (leaving, left) = watch::channel(false);
    let (splitting, split) = watch::channel(Vec::new());
    let (releases, released) = mpsc::unbounded_channel();
    let (endings, ended) = mpsc::unbounded_channel();
    // And what that task shows the loop: under which registration it holds a
    // connection, if it holds one. None until the task has begun.
    let (registering, registration) = watch::channel(None);
    let (words_in, mut words) = mpsc::unbounded_channel();
    // The receiver is right here.
    let _ = words_in.send(Word::Event(WorkerEvent::Orders(first)));
    let registered = tokio::spawn(stay_registered(
        coordinator,
        reach,
        setup.clone(),
        Reports {
            vouched,
            held,
            left,
            split,
            refused,
            released,
            ended,
            registration: registering,
        },
        words_in,
    ));

    let checkpoint_interval = setup.checkpoint_interval;
    let mut regions = Regions::new();
    // Assignments this worker is not to take up again while the coordinator still names
    // them: the world store did not let it open the region, it released the region, or
    // the region was taken from it.
    let mut declined: Vec<Assignment> = Vec::new();
    // The assignments this worker has released, to say so again if the coordinator has
    // not heard.
    let mut let_go: Vec<Assignment> = Vec::new();
    // Regions that were taken from this worker and are being stopped. That can take as
    // long as the store takes, and the other regions are not to wait for it.
    let mut stopping: JoinSet<Ended> = JoinSet::new();
    // The runners of regions that orders named with another epoch than this worker ran
    // them with, each with the merge or the split it was in the middle of. Such a
    // region is opened with the new epoch first, which fences its runner at the store,
    // and the runner is stopped only when that opening has ended, however it ended: it
    // goes on until then, what it had confirmed is what the next runner is restored
    // with, and what it had only applied was shown to nobody. That is the order in
    // which a region goes to another worker while its owner lives.
    let mut aside: BTreeMap<RegionId, (Worker, Option<Reshaping>)> = BTreeMap::new();
    // The merges and splits under way, and where the regions' threads send what came
    // of them: outcomes arrive by a call, not by being looked for.
    let mut reshapes = Reshapes::new();
    let mut reshapes_begun: u64 = 0;
    let (outcomes_in, mut outcomes) = mpsc::unbounded_channel();
    // The regions this worker has split off others that no orders have named yet, each
    // with the region it was split off and the epoch it is run with. Orders that were
    // on their way when the split happened do not know of such a region, and it is not
    // dropped for that.
    let mut unnamed: BTreeMap<RegionId, (RegionId, u64)> = BTreeMap::new();
    let mut look = tokio::time::interval(LOOK);
    // Set when the worker has been told to stop: when it stops waiting.
    let mut leave_by: Option<Instant> = None;
    let outcome = loop {
        // A part that this worker no longer holds has nothing left to wait for.
        unnamed.retain(|part, _| regions.contains_key(part));
        // A runner that was kept aside is stopped when its region is no longer being
        // opened: the store has answered the hello that fenced it, or the opening was
        // given up, because the region is no longer named or was asked to be released.
        let opened: Vec<RegionId> = aside
            .keys()
            .filter(|region| !matches!(regions.get(region), Some(Phase::Opening { .. })))
            .copied()
            .collect();
        for region in opened {
            let (running, reshaping) = aside.remove(&region).expect("it was just found");
            stopping.spawn_blocking(move || {
                let ended = running.stop();
                drop(reshaping);
                ended
            });
        }
        // What the others are told follows from the regions as they are now.
        vouching.send_if_modified(|said| replace(said, vouches(&regions)));
        holding.send_if_modified(|said| replace(said, holdings(&regions)));
        splitting.send_if_modified(|said| {
            let parts = unnamed.iter();
            let parts = parts.map(|(part, (region, as_epoch))| (*region, *as_epoch, *part));
            replace(said, parts.collect())
        });
        serving.send_if_modified(|said| {
            let now = served(&regions);
            // The links of a region stay the same for as long as it runs.
            let same = said.len() == now.len()
                && said
                    .iter()
                    .zip(now.iter())
                    .all(|(old, new)| old.0 == new.0 && old.1.0 == new.1.0);
            if !same {
                *said = now;
            }
            !same
        });
        tokio::select! {
            told = told(&mut stop) => match told {
                Stop::Leave if leave_by.is_none() => {
                    info!("told to stop; asking for what this worker runs to be moved first");
                    leave_by = Some(Instant::now() + LEAVE_WITHIN);
                    leaving.send_replace(true);
                }
                // It has said so already, and waits.
                Stop::Leave => {}
                Stop::AtOnce => {
                    info!("told to stop again; stopping at once");
                    break Ok(());
                }
            },
            () = sleep_until_some(leave_by), if leave_by.is_some() => {
                warn!("nobody took over in time; stopping with what this worker runs");
                break Ok(());
            }
            Some(stopped) = stopping.join_next() => {
                if let Err(error) = stopped {
                    break Err(error.into());
                }
            }
            word = words.recv() => {
                let event = match word {
                    Some(Word::Event(event)) => event,
                    Some(Word::Refused(reason)) => {
                        break Err(anyhow!("the coordinator refused: {reason}"));
                    }
                    Some(Word::Dismissed) => break Ok(()),
                    None => break Err(anyhow!("lost the coordinator for good")),
                };
                match event {
                    WorkerEvent::Release { region, epoch } => {
                        let asked = regions
                            .get(&region)
                            .is_some_and(|phase| phase.held().assignment.epoch == epoch);
                        // Only a region that was asked for is taken out to look at.
                        let found = if asked { regions.remove(&region) } else { None };
                        match found {
                            // A runner that is in the middle of a merge or a split
                            // releases the region when that has ended; what came of
                            // it is said then, as for any other.
                            Some(Phase::Running { held, running, status, .. }) => {
                                info!(%region, epoch, "asked to release the region");
                                running.begin_release();
                                regions.insert(region, Phase::Releasing { held, running, status });
                            }
                            // The region is not open yet, so there is nothing to bring
                            // up to date. Should the store still answer, the handle is
                            // dropped with the future, which closes the region. A part
                            // that waited in memory is dropped with it: the store has
                            // it by the record of the split.
                            Some(Phase::Opening { held, .. } | Phase::Starting { held, .. }) => {
                                info!(%region, epoch, "asked to release the region while opening it");
                                let _ = releases.send((region, epoch));
                                let_go.push(held.assignment);
                                declined.push(held.assignment);
                            }
                            // Under way already.
                            Some(releasing @ Phase::Releasing { .. }) => {
                                regions.insert(region, releasing);
                            }
                            // Not this worker's, or not with that epoch: then it is as
                            // released as it can be, and the coordinator is told so.
                            None => {
                                let _ = releases.send((region, epoch));
                            }
                        }
                    }
                    // A merge or a split is coming: what is saved now is not saved
                    // while players stand still for it. Not answered.
                    WorkerEvent::Prepare { region, epoch } => {
                        if let Some(Phase::Running { held, running, .. }) = regions.get(&region)
                            && held.assignment.epoch == epoch
                        {
                            debug!(
                                %region,
                                epoch,
                                "asked to checkpoint a region before a merge or a split"
                            );
                            running.reshape(Reshape::Prepare, Box::new(|_| {}));
                        }
                    }
                    WorkerEvent::Absorb { region, epoch, absorbed, as_epoch } => {
                        let under_way = reshapes
                            .get(&region)
                            .is_some_and(|reshaping| reshaping.absorbs(absorbed, as_epoch));
                        match free_to_reshape(&regions, &reshapes, region, epoch) {
                            // The order is given again when this worker registers
                            // again while the merge lasts.
                            Err(_) if under_way => {
                                debug!(%region, %absorbed, as_epoch, "asked again for a merge under way");
                            }
                            Err(why) => {
                                info!(
                                    %region,
                                    epoch,
                                    %absorbed,
                                    ?why,
                                    "asked to have a region absorb another, which it cannot now"
                                );
                                let outcome = Err(why);
                                let _ = endings.send(Outcome::Merge { region, absorbed, outcome });
                            }
                            Ok((held, running)) => {
                                info!(
                                    %region,
                                    epoch,
                                    %absorbed,
                                    as_epoch,
                                    "asked to have the region absorb another; opening that one"
                                );
                                // If the order to prepare was lost, this is where the
                                // checkpoint is made while the region ticks.
                                running.reshape(Reshape::Prepare, Box::new(|_| {}));
                                let hello = RegionHello {
                                    region: absorbed,
                                    epoch: as_epoch,
                                    layout: held.hello.layout,
                                };
                                let fetching = Box::pin(fetch(store.clone(), hello));
                                reshapes_begun += 1;
                                let reshaping = Reshaping {
                                    number: reshapes_begun,
                                    held: held.clone(),
                                    doing: Doing::Fetching { absorbed, as_epoch, fetching },
                                };
                                reshapes.insert(region, reshaping);
                            }
                        }
                    }
                    WorkerEvent::SplitOff { region, epoch, chunks, as_epoch, part } => {
                        match free_to_reshape(&regions, &reshapes, region, epoch) {
                            Err(why) => {
                                info!(
                                    %region,
                                    epoch,
                                    as_epoch,
                                    ?why,
                                    "asked to split a region, which it cannot be now"
                                );
                                let outcome = Err(why);
                                let _ = endings.send(Outcome::Split { region, as_epoch, outcome });
                            }
                            Ok((held, running)) => {
                                info!(
                                    %region,
                                    epoch,
                                    chunks = chunks.len(),
                                    as_epoch,
                                    %part,
                                    "asked to split the region"
                                );
                                reshapes_begun += 1;
                                let number = reshapes_begun;
                                let split = Reshape::SplitOff { chunks, as_epoch, part };
                                running.reshape(split, passing_on(&outcomes_in, region, number));
                                let reshaping = Reshaping {
                                    number,
                                    held: held.clone(),
                                    doing: Doing::Splitting { as_epoch },
                                };
                                reshapes.insert(region, reshaping);
                            }
                        }
                    }
                    WorkerEvent::Orders(next) => {
                        let ordered = |assignment: &Assignment| names(&next, assignment);
                        // The coordinator still believes this worker to have what it
                        // has released: the word of it was lost.
                        let_go.retain(ordered);
                        for released in &let_go {
                            let _ = releases.send((released.region, released.epoch));
                        }
                        declined.retain(ordered);
                        // A part that orders name, with whichever epoch, is from now
                        // on a region like any other.
                        unnamed.retain(|part, _| {
                            let mut named = next.assignments.iter();
                            !named.any(|named| named.region == *part)
                        });

                        // What is another worker's now, or nobody's. That is not this
                        // worker's fault, and the store sees to it that it can do no
                        // harm; it lets go. A part that orders name with another epoch
                        // than the one it was split off with is among them: the
                        // coordinator found it in the store's list and gave it out
                        // anew, to this very worker, which opens it like any region it
                        // is given.
                        let taken: Vec<RegionId> = regions
                            .iter()
                            .filter(|(region, phase)| {
                                !ordered(&phase.held().assignment) && !unnamed.contains_key(region)
                            })
                            .map(|(region, _)| *region)
                            .collect();
                        for region in taken {
                            let dropped = regions.remove(&region).expect("it was there a moment ago");
                            let assignment = dropped.held().assignment;
                            // An opening that is given up for yet another epoch ends
                            // here, and the runner that was kept aside for it with it.
                            if let Some((running, reshaping)) = aside.remove(&region) {
                                stopping.spawn_blocking(move || {
                                    let ended = running.stop();
                                    drop(reshaping);
                                    ended
                                });
                            }
                            warn!(
                                %region,
                                epoch = assignment.epoch,
                                "the coordinator has taken the region; letting go of it"
                            );
                            // A merge or a split it was in the middle of is the next
                            // owner's to find done or not.
                            let reshaping = reshapes.remove(&region);
                            if reshaping.is_some() {
                                warn!(%region, "it was in the middle of a merge or a split");
                            }
                            if let Phase::Running { running, .. } | Phase::Releasing { running, .. } = dropped {
                                let named_again = next
                                    .assignments
                                    .iter()
                                    .any(|named| named.region == region);
                                if named_again {
                                    // It is this worker's still, with another epoch:
                                    // opened first, below, and stopped when the store
                                    // has answered that.
                                    aside.insert(region, (running, reshaping));
                                    continue;
                                }
                                // As it is: saved if the store still listens to this
                                // worker, and left to the next owner if not. A region
                                // that was open to be absorbed is closed behind it, so
                                // that a merge the runner had handed the store is not
                                // declined for that.
                                stopping.spawn_blocking(move || {
                                    let ended = running.stop();
                                    drop(reshaping);
                                    ended
                                });
                            }
                        }

                        // What is new. A worker that is leaving takes up nothing.
                        let offered: Vec<Assignment> = next
                            .assignments
                            .iter()
                            .filter(|offered| {
                                let declined = declined.iter().any(|declined| {
                                    (declined.region, declined.epoch) == (offered.region, offered.epoch)
                                });
                                !regions.contains_key(&offered.region) && !declined
                            })
                            .filter(|_| leave_by.is_none())
                            .copied()
                            .collect();
                        if regions.is_empty() && offered.is_empty() {
                            info!("registered; waiting to be given a region");
                        }
                        for assignment in offered {
                            let held = hold(&next, assignment);
                            info!(
                                region = %assignment.region,
                                epoch = assignment.epoch,
                                "given a region"
                            );
                            let opening = Box::pin(open_region(store.clone(), held.hello));
                            regions.insert(assignment.region, Phase::Opening { held, opening });
                        }
                    }
                }
            }
            (region, settled) = settled(&mut regions, &mut reshapes) => match settled {
                Settled::Opened(opened) => {
                    let (held, part) = match regions.remove(&region) {
                        Some(Phase::Opening { held, .. }) => (held, None),
                        Some(Phase::Starting { held, part, .. }) => (held, Some(part)),
                        _ => unreachable!("only an opening resolves"),
                    };
                    match opened {
                        Ok((store, restored)) => {
                            let runner = match part {
                                // What the store says of the region is not looked at:
                                // it is what the record of the split has, which is
                                // what the part in memory is, and the part has its
                                // chunks at hand.
                                Some(part) => RegionRunner::of_part(*part, store),
                                None => match RegionRunner::restore(held.config.clone(), store, restored) {
                                    Ok(runner) => runner,
                                    Err(error) => {
                                        let context = format!("restoring region {region}");
                                        break Err(anyhow::Error::new(error).context(context));
                                    }
                                },
                            };
                            let runner = runner.with_checkpoint_interval(checkpoint_interval);
                            // Only the runner knows for how long its region did not
                            // tick, and only this loop the region's number: so the
                            // line is written here, on the runner's thread. It is what
                            // a player of the region felt, less the moment their edge
                            // takes to find the region again.
                            let runner = runner.with_standstills(Box::new(move |stood: Standstill| {
                                info!(
                                    %region,
                                    players = stood.players,
                                    held = stood.held,
                                    milliseconds = stood.milliseconds,
                                    "a region stood still for a merge or a split"
                                );
                            }));
                            let tick = runner.region().tick_number();
                            let status = runner.status();
                            let links = runner.links();
                            let running = Worker::spawn(runner);
                            info!(
                                %region,
                                epoch = held.assignment.epoch,
                                tick,
                                address = %setup.advertise,
                                "running a region"
                            );
                            let phase = Phase::Running {
                                held,
                                running,
                                links,
                                status,
                                tick,
                                // A region that has just been restored is as good as one
                                // that has just ticked.
                                ticked: Instant::now(),
                            };
                            regions.insert(region, phase);
                        }
                        Err(StoreError::EpochRefused { seen, .. }) => {
                            warn!(
                                %region,
                                epoch = held.assignment.epoch,
                                seen,
                                "the world store has seen a later owner of the region; dropping it"
                            );
                            // The task tells the coordinator, which is still there.
                            let _ = refusals.send((region, seen));
                            declined.push(held.assignment);
                        }
                        // The region is no more, and the coordinator does not know yet:
                        // this makes it read the store's list, which says so.
                        Err(StoreError::Absorbed { into, .. }) => {
                            warn!(
                                %region,
                                epoch = held.assignment.epoch,
                                %into,
                                "the world store has the region as absorbed by another; dropping it"
                            );
                            let (region, absorbed, outcome) = (into, region, Ok(()));
                            let _ = endings.send(Outcome::Merge { region, absorbed, outcome });
                            declined.push(held.assignment);
                        }
                        Err(error) => {
                            let context = format!("opening region {region} at the world store");
                            break Err(anyhow::Error::new(error).context(context));
                        }
                    }
                }
                Settled::Released(ended) => {
                    let Some(Phase::Releasing { held, running, .. }) = regions.remove(&region) else {
                        unreachable!("only a release ends");
                    };
                    let epoch = held.assignment.epoch;
                    // Before the thread is waited for: edges are to be turned away from
                    // a region that is no longer this worker's.
                    serving.send_replace(served(&regions));
                    // The region's thread has ended by itself; this only waits for it to
                    // be gone.
                    if let Err(error) = tokio::task::spawn_blocking(move || running.stop()).await {
                        break Err(error.into());
                    }
                    // With the store lost on the way the region is as released as it can
                    // be: what was confirmed the store has, and the rest was never shown.
                    info!(%region, epoch, ?ended, "released the region");
                    let _ = releases.send((region, epoch));
                    let_go.push(held.assignment);
                    declined.push(held.assignment);
                }
                Settled::Fetched(fetched) => {
                    let Some(Reshaping {
                        number,
                        held,
                        doing: Doing::Fetching { absorbed, as_epoch, .. },
                    }) = reshapes.remove(&region)
                    else {
                        unreachable!("only a region that is being fetched arrives");
                    };
                    let epoch = held.assignment.epoch;
                    let outcome = match fetched {
                        // The region may have been asked to be released, or have lost
                        // the store, while the other was being opened.
                        Ok((handle, state)) => match free_to_reshape(&regions, &reshapes, region, epoch)
                        {
                            Ok((_, running)) => {
                                debug!(%region, %absorbed, as_epoch, "the region to absorb is open");
                                let absorb = Reshape::Absorb {
                                    absorbed,
                                    absorbed_epoch: as_epoch,
                                    state,
                                };
                                running.reshape(absorb, passing_on(&outcomes_in, region, number));
                                let doing = Doing::Absorbing { absorbed, as_epoch, handle };
                                reshapes.insert(region, Reshaping { number, held, doing });
                                None
                            }
                            // Dropping the handle leaves the other region without an
                            // owner, which the coordinator is told next.
                            Err(why) => Some(Err(why)),
                        },
                        // The order came twice, and the merge has happened already.
                        Err(Unabsorbable::Store(StoreError::Absorbed { into, .. }))
                            if into == region =>
                        {
                            Some(Ok(()))
                        }
                        // Somebody else has been given the region since.
                        Err(Unabsorbable::Store(StoreError::EpochRefused { seen, .. })) => {
                            let _ = refusals.send((absorbed, seen));
                            Some(Err(Off::Refused))
                        }
                        // The worker does not end over a region it was only to absorb.
                        Err(Unabsorbable::Store(error)) => {
                            warn!(
                                %region,
                                %absorbed,
                                as_epoch,
                                %error,
                                "the region to absorb could not be opened"
                            );
                            Some(Err(Off::Unreadable))
                        }
                        Err(Unabsorbable::Restore(error)) => {
                            warn!(
                                %region,
                                %absorbed,
                                as_epoch,
                                %error,
                                "the region to absorb could not be read"
                            );
                            Some(Err(Off::Unreadable))
                        }
                    };
                    if let Some(outcome) = outcome {
                        info!(
                            %region,
                            %absorbed,
                            ?outcome,
                            "the merge has ended before the region's runner heard of it"
                        );
                        let _ = endings.send(Outcome::Merge { region, absorbed, outcome });
                    }
                }
            },
            Some((region, number, outcome)) = outcomes.recv() => {
                // Of a merge or a split this worker has let go of since, with its
                // region: what the store has is for whoever runs the region next to
                // find, and a part that came of it is restored from the record. The
                // region may be in the middle of another one by now, which is left
                // alone.
                let current = reshapes
                    .get(&region)
                    .is_some_and(|reshaping| reshaping.number == number);
                if !current {
                    debug!(
                        %region,
                        ?outcome,
                        "passing over what came of a merge or a split that was let go of"
                    );
                    continue;
                }
                let Some(Reshaping { held, doing, .. }) = reshapes.remove(&region) else {
                    unreachable!("it was there a moment ago");
                };
                // The region stood still for it. If it goes on, it is as good as one
                // that has just ticked.
                if let Some(Phase::Running { ticked, .. }) = regions.get_mut(&region) {
                    *ticked = Instant::now();
                }
                match (doing, outcome) {
                    // The handle of the absorbed region is dropped here. The store
                    // has lost that region's owner if the merge happened; if not,
                    // this leaves the region without one, which the coordinator is
                    // told next.
                    (Doing::Absorbing { absorbed, .. }, outcome) => {
                        let outcome = match outcome {
                            Reshaped::Absorbed { .. } => Ok(()),
                            Reshaped::Off { why } => Err(why),
                            // No runner answers a merge with a part. What the store
                            // has, its list says.
                            Reshaped::Split { .. } => Err(Off::StoreLost),
                        };
                        info!(%region, %absorbed, ?outcome, "the merge has ended");
                        let _ = endings.send(Outcome::Merge { region, absorbed, outcome });
                    }
                    // The runner says the epoch it was told, which is the order's.
                    (
                        Doing::Splitting { as_epoch },
                        Reshaped::Split { region: part, part: memory, .. },
                    ) => {
                        info!(
                            %region,
                            %part,
                            epoch = as_epoch,
                            "the split has ended; opening the new region"
                        );
                        // At once: the coordinator answers whoever asked, and tells
                        // the edges where the new region is, while it is being opened.
                        let outcome = Ok(part);
                        let _ = endings.send(Outcome::Split { region, as_epoch, outcome });
                        let hello = RegionHello {
                            region: part,
                            epoch: as_epoch,
                            layout: held.hello.layout,
                        };
                        let assignment = Assignment {
                            region: part,
                            epoch: as_epoch,
                            entity_ids: NO_ENTITY_IDS,
                        };
                        let held = Held { assignment, hello, config: held.config };
                        let opening = Box::pin(open_region(store.clone(), hello));
                        let starting = Phase::Starting { held, part: Box::new(memory), opening };
                        // The store gives an id to one region, so nothing was there.
                        let there = regions.insert(part, starting);
                        if let Some(Phase::Running { running, .. } | Phase::Releasing { running, .. }) = there {
                            stopping.spawn_blocking(move || running.stop());
                        }
                        unnamed.insert(part, (region, as_epoch));
                    }
                    (Doing::Splitting { as_epoch }, outcome) => {
                        let outcome = match outcome {
                            Reshaped::Off { why } => Err(why),
                            // No runner answers a split so. What the store has, its
                            // list says.
                            Reshaped::Absorbed { .. } | Reshaped::Split { .. } => {
                                Err(Off::StoreLost)
                            }
                        };
                        info!(%region, as_epoch, ?outcome, "the split has ended");
                        let _ = endings.send(Outcome::Split { region, as_epoch, outcome });
                    }
                    // The runner knew of nothing whose outcome this could be.
                    (fetching @ Doing::Fetching { .. }, _) => {
                        reshapes.insert(region, Reshaping { number, held, doing: fetching });
                    }
                }
            }
            _ = look.tick() => {
                let mut lost = Vec::new();
                for (region, phase) in regions.iter_mut() {
                    if let Phase::Running { status, tick, ticked, .. } = phase {
                        let now = status.tick.load(Ordering::Relaxed);
                        if now != *tick {
                            *tick = now;
                            *ticked = Instant::now();
                        }
                        if status.store_lost.load(Ordering::Relaxed) {
                            lost.push(*region);
                        }
                    }
                }
                // Where the players are, of every region that was running when this
                // look began: one that has just been found to have lost the store is
                // said with the tick it stopped at. The number is copied out, so that
                // the task is not kept from changing it meanwhile.
                let registered = *registration.borrow();
                report(&endings, registered, &regions);
                let mut failed = None;
                for region in lost {
                    let Some(Phase::Running { held, running, .. }) = regions.remove(&region) else {
                        unreachable!("only a running region loses the store");
                    };
                    warn!(%region, "lost the world store; opening the region again");
                    serving.send_replace(served(&regions));
                    // The region has stopped by itself and closed its links; this only
                    // waits for its thread to be gone. A merge or a split it was in
                    // the middle of has had its outcome by then, which is taken when
                    // its turn comes: that the store was lost on the way.
                    let stopped = tokio::task::spawn_blocking(move || running.stop()).await;
                    if let Err(error) = stopped {
                        failed = Some(error);
                        break;
                    }
                    let opening = Box::pin(open_region(store.clone(), held.hello));
                    regions.insert(region, Phase::Opening { held, opening });
                }
                if let Some(error) = failed {
                    break Err(error.into());
                }
            }
        }
    };
    info!("shutting down");
    // Whoever lets edges in sees by this that the loop serves nothing any more.
    drop(serving);
    registered.abort();
    // Each waits for its current tick and for the world store to have what changed, all
    // at the same time. A release that is under way is let go of as it is, and so is a
    // merge or a split.
    let mut stops = JoinSet::new();
    for phase in regions.into_values() {
        if let Phase::Running { running, .. } | Phase::Releasing { running, .. } = phase {
            stops.spawn_blocking(move || running.stop());
        }
    }
    // A runner that was kept aside for an opening that has not ended is stopped with
    // the others.
    for (running, reshaping) in aside.into_values() {
        stops.spawn_blocking(move || {
            let ended = running.stop();
            drop(reshaping);
            ended
        });
    }
    while let Some(stopped) = stops.join_next().await {
        stopped?;
    }
    // What was being stopped before is waited for as well.
    while let Some(stopped) = stopping.join_next().await {
        stopped?;
    }
    // Only now are the regions that were open to be absorbed closed: a merge that a
    // runner had handed the store before it stopped is not declined for that.
    drop(reshapes);
    outcome
}

/// Makes `said` what is to be said `now`. Returns whether that changed it.
fn replace<T: PartialEq>(said: &mut T, now: T) -> bool {
    let changed = *said != now;
    if changed {
        *said = now;
    }
    changed
}

/// Resolves when the worker is told to stop, with how. Never once nobody is left to
/// tell it.
async fn told(stop: &mut mpsc::UnboundedReceiver<Stop>) -> Stop {
    match stop.recv().await {
        Some(how) => how,
        None => std::future::pending().await,
    }
}

/// What it takes to run the region of `assignment` under `orders`. Whether the world
/// has such a region is the store's to say: it refuses the hello of one it does not
/// have, which ends the worker as a hello for another layout does.
fn hold(orders: &Orders, assignment: Assignment) -> Held {
    Held {
        assignment,
        hello: RegionHello {
            region: assignment.region,
            epoch: assignment.epoch,
            layout: orders.layout.fingerprint(),
        },
        // The region's entity ids are the store's to say, not the coordinator's, and so
        // are the chunks it holds and the areas it is pinned to.
        config: RegionConfig {
            spawn: orders.spawn,
            starting_hotbar: starting_hotbar(),
            return_after: DEFAULT_RETURN_AFTER,
        },
    }
}

/// Opens the region at the world store, trying until the store can be reached. An
/// error is the store's refusal.
async fn open_region(
    store: Opener,
    hello: RegionHello,
) -> Result<(StoreHandle, Restored), StoreError> {
    loop {
        let open = store.clone();
        let opened = tokio::task::spawn_blocking(move || open(hello))
            .await
            .map_err(io::Error::other)?;
        match opened {
            // Whoever gave `store` has said so.
            Err(StoreError::Io(_)) => sleep(RETRY).await,
            opened => return opened,
        }
    }
}

/// Opens the region `hello` names at the world store in order to have another absorb
/// it, trying until the store can be reached, and reads its whole state.
///
/// If the region's owner did not finish releasing it, this waits for the store to
/// have a checkpoint of it, without which the store declines the merge.
async fn fetch(store: Opener, hello: RegionHello) -> Fetched {
    let (handle, restored) = open_region(store, hello)
        .await
        .map_err(Unabsorbable::Store)?;
    let reading = tokio::task::spawn_blocking(move || {
        let state = absorbable(&handle, restored);
        (handle, state)
    });
    match reading.await {
        Ok((handle, Ok(state))) => Ok((handle, state)),
        Ok((_, Err(error))) => Err(Unabsorbable::Restore(error)),
        Err(error) => Err(Unabsorbable::Store(io::Error::other(error).into())),
    }
}

/// Registers with the coordinator, trying until it can be reached.
async fn register(args: &WorkerArgs) -> Result<(WorkerClient, Orders)> {
    loop {
        let registered =
            WorkerClient::register(&args.coordinator, &args.name, &args.advertise, &[], None);
        match registered.await {
            Ok(registered) => return Ok(registered),
            Err(ClientError::Refused(reason)) => bail!("the coordinator refused: {reason}"),
            Err(error) => {
                info!(%error, coordinator = %args.coordinator, "the coordinator cannot be reached yet");
                sleep(RETRY).await;
            }
        }
    }
}

/// What the worker has to say to the coordinator, as it changes.
struct Reports {
    /// What the worker vouches for.
    vouched: watch::Receiver<Vec<(RegionId, Vouch)>>,
    /// The regions the worker holds, each with the fingerprint of the layout it is
    /// part of.
    held: watch::Receiver<Vec<(Assignment, u64)>>,
    /// Whether the worker has been told to stop.
    left: watch::Receiver<bool>,
    /// The regions the worker has split off others and that no orders have named yet:
    /// the region each was split off, the epoch it is run with, and the new region.
    split: watch::Receiver<Vec<(RegionId, u64, RegionId)>>,
    /// Regions the world store did not let the worker open, with the epoch it has seen.
    refused: mpsc::UnboundedReceiver<(RegionId, u64)>,
    /// Regions the worker has let go of, with the epoch it held them with.
    released: mpsc::UnboundedReceiver<(RegionId, u64)>,
    /// What came of the merges and splits the worker was asked for, and where its
    /// players are, in the order the worker's loop made them.
    ended: mpsc::UnboundedReceiver<Outcome>,
    /// Where the task shows the loop the number of the registration whose connection
    /// it holds, or that it holds none. The loop reads where the players are under
    /// that number, and reads nothing for the coordinator without one.
    registration: watch::Sender<Option<u64>>,
}

/// Holds the worker's connection to the coordinator: says what the worker vouches for,
/// what the store refused, what the worker released, what came of a merge or a split,
/// where its players are and that it is leaving, and passes on what the coordinator
/// says, or last of all why it refuses the worker.
///
/// A coordinator that goes away knows nothing when it is back, so the worker registers
/// again and tells it what it holds. The region keeps running meanwhile. A new
/// connection vouches for nothing by itself and knows of no leaving, so both are said
/// again on it. So is every split whose new region the orders that answer the
/// registration do not name: an order to split is given once, and nothing else tells
/// the coordinator which region came of it and whose it is.
///
/// Where the players are is said under the registration it was read under, or not at
/// all (`docs/adr/0016-when-to-merge-and-split.md`, section 2.2). The registrations are
/// counted for that, and the loop is shown the number of the one whose connection there
/// is. A report with another number waited in the queue while a connection was lost.
/// It may be from before a split that is said again, from the watch, on the next
/// connection, and behind that word it would be taken for one of after the split. So it
/// is dropped. Only reports ever are: what came of a merge or a split is said on
/// whichever connection there is.
///
/// A worker that is leaving does not register again: the coordinator closes the
/// connection of one that owns nothing any more, which is how the worker knows that it
/// may exit, and if the coordinator cannot be reached there is nobody to hand anything
/// to.
async fn stay_registered(
    mut coordinator: WorkerClient,
    reach: Reach,
    setup: Setup,
    mut reports: Reports,
    words: mpsc::UnboundedSender<Word>,
) {
    coordinator.vouch(reports.vouched.borrow_and_update().clone());
    if *reports.left.borrow_and_update() {
        coordinator.leaving();
    }
    // The worker registered for the first time before this task began.
    let mut registration: u64 = 1;
    reports.registration.send_replace(Some(registration));
    loop {
        tokio::select! {
            next = coordinator.event() => match next {
                Ok(event) => {
                    if words.send(Word::Event(event)).is_err() {
                        return;
                    }
                }
                Err(_) if *reports.left.borrow() => {
                    info!("the coordinator has let this worker go");
                    let _ = words.send(Word::Dismissed);
                    return;
                }
                Err(error) => {
                    warn!(%error, "lost the coordinator; carrying on and registering again");
                    // From now on the loop reads nothing for the coordinator, and what
                    // it has read under this registration is dropped when its turn
                    // comes.
                    reports.registration.send_replace(None);
                    coordinator = loop {
                        // Told to stop meanwhile: nobody is there to take anything over.
                        tokio::select! {
                            () = sleep(RETRY) => {}
                            changed = reports.left.changed() => {
                                if changed.is_err() || *reports.left.borrow() {
                                    let _ = words.send(Word::Dismissed);
                                    return;
                                }
                            }
                        }
                        let held = reports.held.borrow().clone();
                        let holding: Vec<_> = held.iter().map(|(held, _)| *held).collect();
                        // Every region of a worker is of the one layout it was told.
                        let layout = held.first().map(|(_, layout)| *layout);
                        let registered = WorkerClient::register(
                            &reach,
                            &setup.name,
                            &setup.advertise,
                            &holding,
                            layout,
                        );
                        match registered.await {
                            Ok((coordinator, next)) => {
                                info!("registered again");
                                coordinator.vouch(reports.vouched.borrow_and_update().clone());
                                let parts = reports.split.borrow().clone();
                                for (region, as_epoch, part) in parts {
                                    let mut named = next.assignments.iter();
                                    if !named.any(|named| named.region == part) {
                                        coordinator.split_ended(region, as_epoch, Ok(part));
                                    }
                                }
                                // Only now, when the splits have been said. A split
                                // among them had left its region's status as it is
                                // after it before the loop put it into the watch, so
                                // whatever the loop reads under the new number is of
                                // after every split said above. Shown the number
                                // earlier, the loop could read a region and then hear
                                // of its split, and that report would follow the
                                // word of the split said here.
                                registration += 1;
                                reports.registration.send_replace(Some(registration));
                                if words.send(Word::Event(WorkerEvent::Orders(next))).is_err() {
                                    return;
                                }
                                break coordinator;
                            }
                            Err(ClientError::Refused(reason)) => {
                                let _ = words.send(Word::Refused(reason));
                                return;
                            }
                            Err(error) => debug!(%error, "the coordinator cannot be reached"),
                        }
                    };
                }
            },
            changed = reports.vouched.changed() => {
                if changed.is_err() {
                    return;
                }
                coordinator.vouch(reports.vouched.borrow_and_update().clone());
            }
            changed = reports.left.changed() => {
                if changed.is_err() {
                    return;
                }
                if *reports.left.borrow_and_update() {
                    coordinator.leaving();
                }
            }
            Some((region, seen)) = reports.refused.recv() => {
                coordinator.epoch_refused(region, seen);
            }
            Some((region, epoch)) = reports.released.recv() => {
                coordinator.released(region, epoch);
            }
            Some(ended) = reports.ended.recv() => match ended {
                Outcome::Merge { region, absorbed, outcome } => {
                    coordinator.absorb_ended(region, absorbed, outcome);
                }
                Outcome::Split { region, as_epoch, outcome } => {
                    coordinator.split_ended(region, as_epoch, outcome);
                }
                Outcome::Players { registration: under, regions } => {
                    if under == registration {
                        coordinator.players(regions);
                    } else {
                        debug!(
                            under,
                            registration,
                            "dropping where the players were under an earlier registration"
                        );
                    }
                }
            },
        }
    }
}

/// Accepts the connections of edges and attaches each as a link to the region it asks
/// for, if `serving` has that region at that moment.
async fn accept_edges(listener: TcpListener, serving: watch::Receiver<Serving>) {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                warn!(%error, "accepting a connection failed");
                sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let serving = serving.clone();
        tokio::spawn(async move {
            match greet_edge(stream, serving).await {
                Ok(true) => info!(%peer, "an edge connected"),
                // Something that only looked whether anyone listens, as Kubernetes does.
                Ok(false) => debug!(%peer, "a connection ended without a word"),
                Err(error) => {
                    info!(%peer, error = format!("{error:#}"), "turned a connection away")
                }
            }
        });
    }
}

/// Makes a connection a link of a region this worker runs, if it is about that region
/// as it is now and the region is restored. Returns false if the other side went away
/// without saying what it wanted.
async fn greet_edge(stream: TcpStream, serving: watch::Receiver<Serving>) -> Result<bool> {
    let incoming = match tcp::accept(stream).await {
        Ok(incoming) => incoming,
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(false),
        Err(error) => return Err(error).context("no greeting"),
    };
    let asked = incoming.hello();
    // As of now, and not of when the connection was made: a region can have been
    // restored, or lost, while the greeting was on its way.
    let serving = serving.borrow().clone();
    let Some((region, links)) = serving.get(&asked.region).cloned() else {
        let reason = format!(
            "this worker does not run region {} at the moment",
            asked.region
        );
        incoming.refuse(reason).await?;
        bail!("region {} is not running here", asked.region);
    };
    if asked != region {
        let reason = format!(
            "this worker runs region {} with epoch {} of layout {:016x}",
            region.region, region.epoch, region.layout
        );
        incoming.refuse(reason).await?;
        bail!(
            "it asked for region {} with epoch {} of layout {:016x}",
            asked.region,
            asked.epoch,
            asked.layout
        );
    }
    let link = incoming
        .welcome::<WorkerToEdge, EdgeMessage>(LINK_CAPACITY)
        .await?;
    links.attach(link);
    Ok(true)
}

#[cfg(test)]
mod tests {
    //! What a worker tells the coordinator of where its players are
    //! (`docs/adr/0016-when-to-merge-and-split.md`, section 2.2): what it reads of its
    //! regions, and what the task that holds its connection makes of the queue. The
    //! coordinator is played by the tests, which is how they see what it is told and
    //! in which order.

    use clustine_botswarm::Bot;
    use clustine_edge::{Edge, EdgeConfig, EdgeIdentity, RegionLink, Routing};
    use clustine_region::Layout;
    use clustine_rpc::link::{self, End};
    use clustine_rpc::{FromCoordinator, ToCoordinator};
    use clustine_world::ChunkPos;
    use clustine_worldstore::Store;
    use tokio::time::timeout;

    use super::*;
    use crate::{division, generator, spawn_point};

    const PATIENCE: Duration = Duration::from_secs(30);

    /// What `waited` comes to, which it has to within [`PATIENCE`].
    async fn within<T>(waited: impl Future<Output = T>) -> T {
        timeout(PATIENCE, waited)
            .await
            .expect("what the test waits for comes about")
    }

    /// How the world of these tests is divided: at chunk x 4, so that players enter it
    /// in region 0 and nobody is in region 1.
    fn layout() -> Layout {
        Layout::new(vec![4]).expect("one boundary divides a world")
    }

    /// Such a world, kept in memory.
    fn world() -> Store {
        Store::memory_divided(generator(), division(&layout())).expect("a world in memory")
    }

    fn assignment(region: u32, epoch: u64) -> Assignment {
        Assignment {
            region: RegionId(region),
            epoch,
            // Nothing reads them: a region's entity ids are the store's to say.
            entity_ids: NO_ENTITY_IDS,
        }
    }

    /// What a worker that is given `region` with `epoch` holds it as.
    fn held(region: u32, epoch: u64) -> Held {
        let assignment = assignment(region, epoch);
        let orders = Orders {
            layout: layout(),
            spawn: spawn_point(),
            assignments: vec![assignment],
        };
        hold(&orders, assignment)
    }

    /// Opens `region` at `world` with `epoch` and restores it, as a worker does with a
    /// region it is given.
    fn restored(world: &Store, region: u32, epoch: u64) -> (Held, RegionRunner) {
        let held = held(region, epoch);
        let (store, restored) = world.open_region(held.hello).expect("the store opens it");
        let runner = RegionRunner::restore(held.config.clone(), store, restored)
            .expect("what the store has can be read");
        (held, runner)
    }

    /// Runs `region` of `world` with `epoch`: what a worker's loop keeps of a region it
    /// runs, and an edge's end of a link to it.
    fn running(world: &Store, region: u32, epoch: u64) -> (Phase, RegionLink) {
        let (held, runner) = restored(world, region, epoch);
        let (end, worker_end) = link::in_process(LINK_CAPACITY);
        let (status, links) = (runner.status(), runner.links());
        links.attach(worker_end);
        let tick = runner.region().tick_number();
        let phase = Phase::Running {
            held,
            running: Worker::spawn(runner),
            links,
            status,
            tick,
            ticked: Instant::now(),
        };
        let link = RegionLink {
            region: RegionId(region),
            epoch,
            end,
        };
        (phase, link)
    }

    /// The status of a region that runs.
    fn status_of(phase: &Phase) -> Arc<RegionStatus> {
        match phase {
            Phase::Running { status, .. } => Arc::clone(status),
            _ => panic!("the region does not run"),
        }
    }

    /// Stops the threads of `regions`, which nothing else does once a test is over.
    async fn stop(regions: Regions) {
        for phase in regions.into_values() {
            if let Phase::Running { running, .. } | Phase::Releasing { running, .. } = phase {
                let stopped = tokio::task::spawn_blocking(move || running.stop());
                stopped.await.expect("a region stops");
            }
        }
    }

    /// Something a region is opened by that never answers.
    fn never() -> Opening {
        Box::pin(std::future::pending())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_region_that_runs_is_reported_with_its_epoch_its_tick_and_its_crowds_also_one_without_players()
     {
        let world = world();
        let (home, home_link) = running(&world, 0, 3);
        let (empty, empty_link) = running(&world, 1, 5);
        let statuses = [status_of(&home), status_of(&empty)];
        let regions = Regions::from([(RegionId(0), home), (RegionId(1), empty)]);

        // A player, let in by an edge that is linked to both regions. Players enter
        // the world in the chunk at the origin, which is region 0's.
        let identity = EdgeIdentity::starting_now("edge");
        let links = vec![home_link, empty_link];
        let (routing, _relinks) = Routing::new(RegionId(0), spawn_point(), identity, links);
        let config = EdgeConfig {
            description: "a test".to_owned(),
            max_players: 7,
            keep_alive_interval: EdgeConfig::DEFAULT_KEEP_ALIVE_INTERVAL,
            view_distance: 2,
            client_timeout: EdgeConfig::DEFAULT_CLIENT_TIMEOUT,
            compression_threshold: None,
            region_patience: EdgeConfig::DEFAULT_REGION_PATIENCE,
        };
        let anywhere = SocketAddr::from(([127, 0, 0, 1], 0));
        let edge = Edge::bind(anywhere, config, routing)
            .await
            .expect("an edge listens");
        let address = edge.local_addr().expect("it has an address").to_string();
        let edge = tokio::spawn(edge.run());
        let _bot = Bot::join(&address, "Alice").await.expect("a player joins");
        let origin = ChunkPos::new(0, 0);
        within(async {
            while statuses[0].crowds() != [(origin, 1)] {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        let ticks = || {
            statuses
                .each_ref()
                .map(|status| status.tick.load(Ordering::Relaxed))
        };
        let before = ticks();
        let said = players(&regions);
        let after = ticks();

        // Both regions, in the order of their ids, each with the epoch it is run with.
        let named: Vec<_> = said.iter().map(|of| (of.region, of.epoch)).collect();
        assert_eq!(named, [(RegionId(0), 3), (RegionId(1), 5)]);
        assert_eq!(said[0].crowds, [(origin, 1)]);
        assert!(said[1].crowds.is_empty(), "{:?}", said[1].crowds);
        // The regions tick while they are read, so a tick is known only to be one
        // they were at meanwhile.
        for (of, (before, after)) in said.iter().zip(before.into_iter().zip(after)) {
            assert!((before..=after).contains(&of.tick), "{of:?}");
        }

        // And at a later look, a later tick of each.
        within(async {
            while ticks().iter().zip(&after).any(|(now, then)| now <= then) {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        let later = players(&regions);
        for (later, earlier) in later.iter().zip(&said) {
            assert!(later.tick > earlier.tick, "{later:?} after {earlier:?}");
        }

        edge.abort();
        stop(regions).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_region_that_is_being_opened_released_or_started_from_a_split_is_not_reported() {
        let world = world();
        // What a split would have made of a region: any region will do, since nothing
        // looks at a part before the store has answered for it.
        let (_, apart) = restored(&world, 1, 5);
        let part = Part {
            region: apart.region().clone(),
            chunks: Vec::new(),
        };
        drop(apart);

        let (runs, _link) = running(&world, 0, 3);
        let (released, _link) = running(&world, 1, 6);
        let Phase::Running {
            held: released,
            running,
            status,
            ..
        } = released
        else {
            unreachable!("it runs");
        };
        running.begin_release();
        let releasing = Phase::Releasing {
            held: released,
            running,
            status,
        };
        let opening = Phase::Opening {
            held: held(2, 7),
            opening: never(),
        };
        let starting = Phase::Starting {
            held: held(3, 9),
            part: Box::new(part),
            opening: never(),
        };
        let regions = Regions::from([
            (RegionId(0), runs),
            (RegionId(1), releasing),
            (RegionId(2), opening),
            (RegionId(3), starting),
        ]);

        let said = players(&regions);
        let named: Vec<_> = said.iter().map(|of| (of.region, of.epoch)).collect();
        assert_eq!(named, [(RegionId(0), 3)]);

        stop(regions).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn where_the_players_are_is_queued_under_the_registration_there_is_and_not_without_one() {
        let world = world();
        let (runs, _link) = running(&world, 0, 3);
        let regions = Regions::from([(RegionId(0), runs)]);
        let (endings, mut ended) = mpsc::unbounded_channel();

        // No connection to the coordinator: nothing, however often it is looked.
        report(&endings, None, &regions);
        report(&endings, None, &regions);
        assert!(ended.try_recv().is_err());

        // One for each look while there is one, under its number.
        report(&endings, Some(4), &regions);
        let Ok(Outcome::Players {
            registration,
            regions: said,
        }) = ended.try_recv()
        else {
            panic!("a report is queued");
        };
        assert_eq!(registration, 4);
        let named: Vec<_> = said.iter().map(|of| (of.region, of.epoch)).collect();
        assert_eq!(named, [(RegionId(0), 3)]);
        assert!(ended.try_recv().is_err());

        // A worker that runs nothing says so: the whole of what it knows each time.
        report(&endings, Some(5), &Regions::new());
        let nothing = Outcome::Players {
            registration: 5,
            regions: Vec::new(),
        };
        assert_eq!(ended.try_recv().ok(), Some(nothing));

        stop(regions).await;
    }

    /// The coordinator's end of a worker's connection.
    type Heard = End<FromCoordinator, ToCoordinator>;

    /// A coordinator as a test plays it: it listens where a worker looks for one, and
    /// the test decides what it answers.
    struct Played {
        listener: TcpListener,
        address: String,
    }

    impl Played {
        async fn listening() -> Self {
            let anywhere = SocketAddr::from(([127, 0, 0, 1], 0));
            let listener = TcpListener::bind(anywhere).await.expect("a free port");
            let address = listener
                .local_addr()
                .expect("it has an address")
                .to_string();
            Self { listener, address }
        }

        /// Lets the next worker register and tells it to run `assignments`. Returns
        /// its connection and what it said it holds.
        async fn registers(&self, assignments: Vec<Assignment>) -> (Heard, Vec<Assignment>) {
            let (stream, _) = within(self.listener.accept()).await.expect("a connection");
            let mut heard: Heard = tcp::link(stream, 256);
            let Some(ToCoordinator::RegisterWorker { holding, .. }) = within(heard.recv()).await
            else {
                panic!("a worker registers before it says anything else");
            };
            let orders = FromCoordinator::Assigned {
                layout: layout(),
                spawn: spawn_point(),
                assignments,
            };
            heard.send(orders).await.expect("the worker listens");
            (heard, holding)
        }
    }

    /// What the worker says next besides that it is there.
    async fn said(heard: &mut Heard) -> ToCoordinator {
        loop {
            match within(heard.recv()).await {
                Some(ToCoordinator::Heartbeat { .. }) => {}
                Some(said) => return said,
                None => panic!("the worker has gone"),
            }
        }
    }

    /// The loop's side of the task that holds a worker's connection: where the loop
    /// puts what the task is to say, and where the task shows it what it needs.
    struct Looping {
        holding: watch::Sender<Vec<(Assignment, u64)>>,
        splitting: watch::Sender<Vec<(RegionId, u64, RegionId)>>,
        endings: mpsc::UnboundedSender<Outcome>,
        registration: watch::Receiver<Option<u64>>,
        words: mpsc::UnboundedReceiver<Word>,
        /// What the worker vouches for and whether it is leaving, which no test here
        /// changes. The task ends when either is gone.
        #[allow(dead_code)] // Held, never looked at.
        vouching: watch::Sender<Vec<(RegionId, Vouch)>>,
        #[allow(dead_code)] // Held, never looked at.
        leaving: watch::Sender<bool>,
    }

    impl Looping {
        /// Waits until the task shows `registration` as the one it holds a connection
        /// of, or none.
        async fn shown(&mut self, registration: Option<u64>) {
            let shown = self.registration.wait_for(|shown| *shown == registration);
            within(shown).await.expect("the task is there");
        }

        /// Queues where the players are as a look under `registration` found them:
        /// `crowds` in region 0, which is run with epoch 4 and was at `tick`.
        fn sees(&self, registration: u64, tick: u64, crowds: &[(ChunkPos, u32)]) -> ToCoordinator {
            let regions = vec![PlayersOf {
                region: RegionId(0),
                epoch: 4,
                tick,
                crowds: crowds.to_vec(),
            }];
            let report = Outcome::Players {
                registration,
                regions: regions.clone(),
            };
            self.endings.send(report).expect("the task is there");
            ToCoordinator::Players { regions }
        }
    }

    /// Registers a worker with `played`, which gives it nothing to run, and starts the
    /// task that holds its connection. Returns both ends of what a worker has then.
    async fn registered(played: &Played) -> (Looping, Heard) {
        let args = WorkerArgs {
            coordinator: played.address.clone(),
            // Nothing of a worker but its word to the coordinator is there.
            store: "127.0.0.1:1".to_owned(),
            listen: SocketAddr::from(([127, 0, 0, 1], 0)),
            advertise: "worker:1".to_owned(),
            name: "worker".to_owned(),
            checkpoint_interval: Duration::from_secs(300),
        };
        let (client, (heard, _)) = tokio::join!(register(&args), played.registers(Vec::new()));
        let (client, _) = client.expect("the worker is registered");

        let (vouching, vouched) = watch::channel(Vec::new());
        let (holding, held) = watch::channel(Vec::new());
        let (leaving, left) = watch::channel(false);
        let (splitting, split) = watch::channel(Vec::new());
        // The store refuses this worker nothing and it releases nothing: nobody ever
        // puts anything into these two, and the task listens to the others without
        // them.
        let (_, refused) = mpsc::unbounded_channel();
        let (_, released) = mpsc::unbounded_channel();
        let (endings, ended) = mpsc::unbounded_channel();
        let (registering, registration) = watch::channel(None);
        let (words_in, words) = mpsc::unbounded_channel();
        let reports = Reports {
            vouched,
            held,
            left,
            split,
            refused,
            released,
            ended,
            registration: registering,
        };
        let setup = Setup {
            name: args.name,
            advertise: args.advertise,
            checkpoint_interval: 6000,
        };
        let reach = Reach::Tcp(args.coordinator);
        tokio::spawn(stay_registered(client, reach, setup, reports, words_in));
        let looping = Looping {
            holding,
            splitting,
            endings,
            registration,
            words,
            vouching,
            leaving,
        };
        (looping, heard)
    }

    const ORIGIN: ChunkPos = ChunkPos::new(0, 0);
    const FAR: ChunkPos = ChunkPos::new(-40, 0);

    #[tokio::test]
    async fn where_the_players_are_is_said_behind_what_came_of_the_merges_and_splits_before_it() {
        let played = Played::listening().await;
        let (mut looping, mut heard) = registered(&played).await;
        looping.shown(Some(1)).await;

        // As the loop makes them: a look, a split, a look, a merge, a look.
        let split = Outcome::Split {
            region: RegionId(0),
            as_epoch: 7,
            outcome: Ok(RegionId(2)),
        };
        let merge = Outcome::Merge {
            region: RegionId(0),
            absorbed: RegionId(1),
            outcome: Err(Off::Busy),
        };
        let before = looping.sees(1, 20, &[(ORIGIN, 1), (FAR, 2)]);
        looping.endings.send(split).expect("the task is there");
        let between = looping.sees(1, 21, &[(ORIGIN, 1)]);
        looping.endings.send(merge).expect("the task is there");
        let after = looping.sees(1, 26, &[(ORIGIN, 1)]);

        // And as the coordinator hears them.
        let split = ToCoordinator::SplitEnded {
            region: RegionId(0),
            as_epoch: 7,
            outcome: Ok(RegionId(2)),
        };
        let merge = ToCoordinator::AbsorbEnded {
            region: RegionId(0),
            absorbed: RegionId(1),
            outcome: Err(Off::Busy),
        };
        for expected in [before, split, between, merge, after] {
            assert_eq!(said(&mut heard).await, expected);
        }
    }

    /// K25 of the record: a report from before a split is in the queue when the
    /// connection ends. Said on the next connection, it would follow the word of the
    /// split that the task says from its watch, and be taken for one of after it.
    #[tokio::test]
    async fn what_was_read_before_the_connection_was_lost_is_dropped_and_what_came_of_a_split_is_said()
     {
        let played = Played::listening().await;
        let (mut looping, heard) = registered(&played).await;
        looping.shown(Some(1)).await;

        // The coordinator goes away, and the loop is shown that there is none.
        drop(heard);
        looping.shown(None).await;

        // What the loop had made by then, in its order: a look that found the players
        // who are about to go, and what came of the split, with which it holds the new
        // region and says so in its watches.
        looping.sees(1, 20, &[(ORIGIN, 1), (FAR, 2)]);
        let split = Outcome::Split {
            region: RegionId(0),
            as_epoch: 7,
            outcome: Ok(RegionId(2)),
        };
        looping.endings.send(split).expect("the task is there");
        let fingerprint = layout().fingerprint();
        let holds = [assignment(0, 4), assignment(2, 7)];
        looping
            .holding
            .send_replace(holds.map(|held| (held, fingerprint)).to_vec());
        looping
            .splitting
            .send_replace(vec![(RegionId(0), 7, RegionId(2))]);

        // The coordinator is back and knows the region it had given out, not the new
        // one.
        let (mut heard, holding) = played.registers(vec![assignment(0, 4)]).await;
        assert_eq!(holding, holds);
        let split = ToCoordinator::SplitEnded {
            region: RegionId(0),
            as_epoch: 7,
            outcome: Ok(RegionId(2)),
        };
        // From the watch, and then from the queue, where the report was before it.
        assert_eq!(said(&mut heard).await, split);
        assert_eq!(said(&mut heard).await, split);

        // What the loop reads under the new registration is said.
        looping.shown(Some(2)).await;
        let after = looping.sees(2, 26, &[(ORIGIN, 1)]);
        assert_eq!(said(&mut heard).await, after);
        // And the loop was passed the orders that answered the registration.
        let Some(Word::Event(WorkerEvent::Orders(orders))) = within(looping.words.recv()).await
        else {
            panic!("the new orders are passed on");
        };
        assert_eq!(orders.assignments, [assignment(0, 4)]);
    }

    /// A worker's loop as [`run`] is given one: whether it has ended and how, and
    /// where it is told to stop.
    struct Run {
        ended: tokio::task::JoinHandle<Result<()>>,
        stop: mpsc::UnboundedSender<Stop>,
    }

    impl Run {
        /// How the loop ends, which it has to within [`PATIENCE`].
        async fn ended(self) -> Result<()> {
            within(self.ended).await.expect("the loop does not panic")
        }
    }

    /// Registers a worker with `played`, which gives it nothing to run, and runs its
    /// loop. Returns the loop and the coordinator's end of its connection.
    async fn run_by(played: &Played) -> (Run, Heard) {
        let registering = WorkerClient::register(&played.address, "worker", "worker:1", &[], None);
        let (registered, (heard, _)) = tokio::join!(registering, played.registers(Vec::new()));
        let setup = Setup {
            name: "worker".to_owned(),
            advertise: "worker:1".to_owned(),
            checkpoint_interval: 6000,
        };
        // Nobody links to a worker that runs nothing.
        let (serving, _) = watch::channel(Serving::default());
        let (stop, stopped) = mpsc::unbounded_channel();
        let outside = Outside {
            registered: registered.expect("the worker is registered"),
            coordinator: Reach::Tcp(played.address.clone()),
            store: Arc::new(|_| unreachable!("the worker is given no region to open")),
            serving,
            refusals: mpsc::unbounded_channel(),
            stop: stopped,
        };
        let ended = tokio::spawn(run(setup, outside));
        (Run { ended, stop }, heard)
    }

    #[tokio::test]
    async fn a_loop_that_is_told_to_stop_at_once_ends() {
        let played = Played::listening().await;
        let (run, _heard) = run_by(&played).await;

        run.stop.send(Stop::AtOnce).expect("the loop is there");
        run.ended().await.expect("it ends as one that was stopped");
    }

    #[tokio::test]
    async fn a_loop_that_is_told_to_leave_says_so_and_ends_when_the_coordinator_lets_it_go() {
        let played = Played::listening().await;
        let (run, mut heard) = run_by(&played).await;

        run.stop.send(Stop::Leave).expect("the loop is there");
        // Among where its players are, which it goes on saying.
        while said(&mut heard).await != ToCoordinator::Leaving {}

        // The coordinator closes the connection of a worker that owns nothing.
        drop(heard);
        run.ended().await.expect("it ends as one that was let go");
    }

    #[tokio::test]
    async fn a_loop_that_is_leaving_ends_when_it_is_told_to_stop_at_once() {
        let played = Played::listening().await;
        let (run, mut heard) = run_by(&played).await;

        run.stop.send(Stop::Leave).expect("the loop is there");
        while said(&mut heard).await != ToCoordinator::Leaving {}

        // Told to leave once more, which it does already, and then to stop. The
        // coordinator is still there and has not let it go.
        run.stop.send(Stop::Leave).expect("the loop is there");
        run.stop.send(Stop::AtOnce).expect("the loop is there");
        run.ended().await.expect("it ends as one that was stopped");
        drop(heard);
    }

    #[tokio::test]
    async fn a_loop_that_nobody_is_left_to_tell_to_stop_goes_on() {
        let played = Played::listening().await;
        let (Run { ended, stop }, mut heard) = run_by(&played).await;
        drop(stop);

        // It answers what the coordinator asks of it, time after time: a region it
        // does not hold is as released as it can be.
        for region in [RegionId(7), RegionId(8), RegionId(9)] {
            let release = FromCoordinator::Release { region, epoch: 1 };
            heard.send(release).await.expect("the worker listens");
            let released = ToCoordinator::Released { region, epoch: 1 };
            while said(&mut heard).await != released {}
        }
        assert!(!ended.is_finished());
        ended.abort();
    }

    // The scenarios P7 and P9 of `docs/adr/0017-the-end-of-the-stripes.md`, section 9.5:
    // the loop as a single process runs it, put together by the tests. They were
    // written from the record by someone who had not read `run`.

    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};

    use clustine_coordinator::{CoordinatorConfig, serve_local};
    use clustine_edge::Relinks;
    use clustine_rpc::{EdgeMessage, EdgeToWorker, Welcome, WorkerToEdge};
    use clustine_world::EdgeId;
    use clustine_worldstore::Division;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const HOME: RegionId = RegionId(0);
    const AIR: Option<i32> = Some(clustine_data::blocks::AIR.0 as i32);

    /// The height of the block players stand on.
    const FLOOR: i32 = -61;

    /// How often a state that is waited for is looked at.
    const LOOKING: Duration = Duration::from_millis(20);

    /// A world that is one home region, pinned to nothing, kept in memory.
    fn open_world() -> Store {
        Store::memory_divided(generator(), Division::open(ORIGIN)).expect("a world in memory")
    }

    /// How a loop opens its regions at a store in its own process.
    fn opener_of(world: &Store) -> Opener {
        let world = world.clone();
        Arc::new(move |hello| world.open_region(hello))
    }

    /// The highest epoch the store has seen `region` opened with.
    fn opened_with(world: &Store, region: RegionId) -> u64 {
        let list = world.regions().expect("the store lists its regions");
        let info = list.regions.iter().find(|info| info.region == region);
        info.expect("the region lives").epoch
    }

    /// A worker's loop with what the single process gives it (section 6.1): a
    /// coordinator in its process, alone with it, that reads the list of `world` and
    /// is reached through `Reach::Local`; a way to open regions; and a watch of what
    /// it serves.
    struct Alone {
        run: Run,
        /// What the loop shows of the regions it serves.
        serving: watch::Receiver<Serving>,
        /// Where a refusal of the store is said for the loop, as `Server::take_over`
        /// says one (section 6.4).
        refusals: mpsc::UnboundedSender<Refusal>,
        coordinator: tokio::task::JoinHandle<()>,
    }

    impl Alone {
        async fn start(world: &Store, store: Opener) -> Self {
            let lists = {
                let world = world.clone();
                move || {
                    let list = world.regions();
                    list.map_err(|error| io::Error::other(error.to_string()))
                }
            };
            let config = CoordinatorConfig {
                layout: Layout::single(),
                spawn: spawn_point(),
                lease: CoordinatorConfig::DEFAULT_LEASE,
                follow: None,
            };
            let (local, served) = serve_local(config, lists);
            let coordinator = tokio::spawn(served);
            let reach = Reach::Local(local);
            let setup = Setup {
                name: "local".to_owned(),
                advertise: "in this process".to_owned(),
                checkpoint_interval: 6000,
            };
            let registering =
                WorkerClient::register(&reach, &setup.name, &setup.advertise, &[], None);
            let registered = within(registering).await.expect("the worker is registered");
            let (serving, shown) = watch::channel(Serving::default());
            let (stop, stopped) = mpsc::unbounded_channel();
            let (refusals, refused) = mpsc::unbounded_channel();
            let outside = Outside {
                registered,
                coordinator: reach,
                store,
                serving,
                refusals: (refusals.clone(), refused),
                stop: stopped,
            };
            let ended = tokio::spawn(run(setup, outside));
            Self {
                run: Run { ended, stop },
                serving: shown,
                refusals,
                coordinator,
            }
        }

        /// What the loop shows of `region`, if it serves it with an epoch above
        /// `above`: its hello, and where links to it are attached.
        fn shows(&self, region: RegionId, above: u64) -> Option<(RegionHello, Links)> {
            let serving = self.serving.borrow();
            let (hello, links) = serving.get(&region)?;
            (hello.epoch > above).then(|| (*hello, links.clone()))
        }

        /// Waits until the loop shows `region` with an epoch above `above`, while
        /// `bots` stay connected.
        async fn shown(
            &self,
            bots: &mut [&mut Bot],
            region: RegionId,
            above: u64,
        ) -> (RegionHello, Links) {
            let waiting = Instant::now();
            loop {
                if let Some(shown) = self.shows(region, above) {
                    return shown;
                }
                assert!(
                    !self.run.ended.is_finished(),
                    "the loop ended before it served the region"
                );
                assert!(
                    waiting.elapsed() <= PATIENCE,
                    "the loop does not serve region {region} with an epoch above {above}"
                );
                idle(bots).await;
            }
        }

        /// Tells the loop to stop at once and waits for it to return, by when it has
        /// let go of its watch.
        async fn stopped(self) {
            let stop = self.run.stop.clone();
            stop.send(Stop::AtOnce).expect("the loop is there");
            self.run
                .ended()
                .await
                .expect("it ends as one that was stopped");
            assert!(
                self.serving.has_changed().is_err(),
                "a loop that has ended serves nothing any more"
            );
            self.coordinator.abort();
        }
    }

    /// Lets a moment pass, in which `bots` stay connected.
    async fn idle(bots: &mut [&mut Bot]) {
        if bots.is_empty() {
            sleep(LOOKING).await;
        }
        for bot in bots.iter_mut() {
            bot.idle(LOOKING).await.expect("the player stays connected");
        }
    }

    /// An edge of a test's own, which the test keeps linked.
    struct Linked {
        address: String,
        relinks: Relinks,
        edge: tokio::task::JoinHandle<clustine_edge::Stopped>,
    }

    /// Starts an edge called `name` with `links`.
    async fn edge_with(name: &str, links: Vec<RegionLink>) -> Linked {
        let identity = EdgeIdentity::starting_now(name);
        let (routing, relinks) = Routing::new(HOME, spawn_point(), identity, links);
        let config = EdgeConfig {
            description: "a test".to_owned(),
            max_players: 7,
            keep_alive_interval: EdgeConfig::DEFAULT_KEEP_ALIVE_INTERVAL,
            view_distance: 2,
            client_timeout: EdgeConfig::DEFAULT_CLIENT_TIMEOUT,
            compression_threshold: None,
            region_patience: EdgeConfig::DEFAULT_REGION_PATIENCE,
        };
        let anywhere = SocketAddr::from(([127, 0, 0, 1], 0));
        let edge = Edge::bind(anywhere, config, routing)
            .await
            .expect("an edge listens");
        let address = edge.local_addr().expect("it has an address").to_string();
        Linked {
            address,
            relinks,
            edge: tokio::spawn(edge.run()),
        }
    }

    /// A link to the region a loop serves with `hello`, attached where the loop says
    /// links to it are. Its messages are serialised, as between processes.
    fn link_to(hello: RegionHello, links: &Links) -> RegionLink {
        let (end, worker_end) = link::framed(LINK_CAPACITY);
        links.attach(worker_end);
        RegionLink {
            region: hello.region,
            epoch: hello.epoch,
            end,
        }
    }

    /// Joins at `address` and waits for the four chunks that meet where players enter
    /// the world, which have every block these tests touch. What is done to a block
    /// of a chunk that has not been sent is acknowledged without effect.
    async fn joined(address: &str, name: &str) -> Bot {
        let mut bot = Bot::join(address, name).await.expect("a player joins");
        let sent = |bot: &Bot| {
            let corners = [(0, 0), (-1, 0), (0, -1), (-1, -1)];
            corners.iter().all(|(x, z)| floor(bot, *x, *z).is_some())
        };
        let waited = bot.wait_until(PATIENCE, sent).await;
        waited.expect("the player is sent the chunks around where they enter");
        bot
    }

    /// The block of the floor at `x` and `z`, as `bot` has been shown it.
    fn floor(bot: &Bot, x: i32, z: i32) -> Option<i32> {
        bot.block_at(x, FLOOR, z).expect("a height of the world")
    }

    /// `bot` breaks the block of the floor at `x` and `z` and waits until that is
    /// acknowledged, by when it has to have been shown the block gone.
    async fn digs(bot: &mut Bot, x: i32, z: i32) -> i32 {
        let sequence = bot.dig(x, FLOOR, z).await.expect("the player is connected");
        let acknowledged = |bot: &Bot| bot.acknowledged_sequence >= sequence;
        let waited = bot.wait_until(PATIENCE, acknowledged).await;
        waited.expect("what the player did is acknowledged");
        assert_eq!(floor(bot, x, z), AIR, "the block at {x}, {z}");
        sequence
    }

    /// Fails unless every acknowledgement `bot` has had came once: no number twice,
    /// and none behind a higher one.
    fn acknowledged_once(bot: &Bot) {
        let numbers: Vec<i32> = bot.acknowledgements.iter().map(|(n, _)| *n).collect();
        assert!(
            numbers.windows(2).all(|pair| pair[0] < pair[1]),
            "an action was acknowledged twice, or behind a later one: {numbers:?}"
        );
    }

    // P7.
    //
    /// The worker's loop, given a store in its process and a coordinator through
    /// `Reach::Local`, runs the region it is assigned and shows it in its watch; a
    /// link attached there is welcomed. Told to stop at once, it returns, and the
    /// store has what its region had done.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_loop_with_its_store_and_its_coordinator_in_its_process_runs_its_region_and_leaves_the_store_what_it_did()
     {
        let world = open_world();
        let alone = Alone::start(&world, opener_of(&world)).await;

        // The coordinator reads the store's list and gives out its one region, at
        // once; the loop shows it when it runs, with the hello it opened it with.
        let (hello, links) = alone.shown(&mut [], HOME, 0).await;
        assert_eq!(hello.region, HOME);
        assert_eq!(opened_with(&world, HOME), hello.epoch);
        assert_eq!(alone.serving.borrow().len(), 1);

        // A link that is attached there is welcomed: as that of an edge the region
        // has not heard of.
        let (mut end, worker_end) = link::in_process::<EdgeMessage, WorkerToEdge>(LINK_CAPACITY);
        links.attach(worker_end);
        let stranger = EdgeToWorker::Hello {
            edge: EdgeId::from_name("a stranger"),
            start: 1,
            since: 0,
            seen: 0,
            players: Vec::new(),
            chunks: Vec::new(),
            guests: Vec::new(),
        };
        end.send(EdgeMessage::unnumbered(stranger))
            .await
            .expect("the link is open");
        let welcome = within(end.recv()).await;
        assert!(
            matches!(
                welcome,
                Some(WorkerToEdge::Welcome(Welcome::Unknown { .. }))
            ),
            "{welcome:?}"
        );
        drop(end);

        // And so is an edge's, through which somebody plays.
        let edge = edge_with("edge", vec![link_to(hello, &links)]).await;
        let mut alice = joined(&edge.address, "Alice").await;
        digs(&mut alice, 2, 1).await;
        digs(&mut alice, -1, 2).await;
        acknowledged_once(&alice);

        alone.stopped().await;
        edge.edge.abort();
        drop(alice);

        // The store has what the region had done, and nobody holds the region any
        // more: another loop is given it, opens it with a higher epoch, and whoever
        // joins is shown what was dug.
        let again = Alone::start(&world, opener_of(&world)).await;
        let (next, links) = again.shown(&mut [], HOME, hello.epoch).await;
        assert_eq!(opened_with(&world, HOME), next.epoch);
        let edge = edge_with("another edge", vec![link_to(next, &links)]).await;
        let bob = joined(&edge.address, "Bob").await;
        for (x, z) in [(2, 1), (-1, 2)] {
            assert_eq!(floor(&bob, x, z), AIR, "the block at {x}, {z}");
        }
        assert_ne!(floor(&bob, 3, 3), AIR);
        again.stopped().await;
        edge.edge.abort();
    }

    /// What stands between a worker's loop and a world store that is served at an
    /// address. Every connection the loop makes to the store goes through it, and a
    /// test can have it hold back what the store says on the connections there are,
    /// while the store hears, does and makes durable everything it is asked.
    struct Between {
        address: String,
        /// The connections with a number below this have what the store says held
        /// back; the first connection is number 0.
        below: watch::Sender<u64>,
        /// How many connections were made.
        made: Arc<AtomicU64>,
        /// How many bytes the store has said on connections while they were held.
        kept: Arc<AtomicUsize>,
    }

    impl Between {
        /// Listens, and connects whoever comes to the store at `store`.
        async fn before(store: SocketAddr) -> Self {
            let anywhere = SocketAddr::from(([127, 0, 0, 1], 0));
            let listener = TcpListener::bind(anywhere).await.expect("a free port");
            let address = listener.local_addr().expect("it has an address");
            let (below, held) = watch::channel(0);
            let (made, kept) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicUsize::new(0)));
            let between = Self {
                address: address.to_string(),
                below,
                made: Arc::clone(&made),
                kept: Arc::clone(&kept),
            };
            tokio::spawn(async move {
                loop {
                    let Ok((near, _)) = listener.accept().await else {
                        return;
                    };
                    let Ok(far) = TcpStream::connect(store).await else {
                        continue;
                    };
                    let number = made.fetch_add(1, Ordering::SeqCst);
                    let _ = (near.set_nodelay(true), far.set_nodelay(true));
                    let (mut asked, mut answered) = near.into_split();
                    let (mut answers, mut asks) = far.into_split();
                    // What the loop asks reaches the store as it comes.
                    tokio::spawn(async move {
                        let _ = tokio::io::copy(&mut asked, &mut asks).await;
                        let _ = asks.shutdown().await;
                    });
                    // What the store says waits for as long as the connection is held,
                    // and so does the end of the connection.
                    let (mut held, kept) = (held.clone(), Arc::clone(&kept));
                    tokio::spawn(async move {
                        let mut bytes = vec![0; 64 * 1024];
                        loop {
                            let read = answers.read(&mut bytes).await.unwrap_or(0);
                            if number < *held.borrow() {
                                kept.fetch_add(read.max(1), Ordering::SeqCst);
                            }
                            if held.wait_for(|below| number >= *below).await.is_err() {
                                return;
                            }
                            if read == 0 || answered.write_all(&bytes[..read]).await.is_err() {
                                break;
                            }
                        }
                        let _ = answered.shutdown().await;
                    });
                }
            });
            between
        }

        /// How a loop opens its regions through this.
        fn opener(&self) -> Opener {
            let address = self.address.clone();
            Arc::new(move |hello| StoreHandle::connect(&address, hello))
        }

        /// Holds back, from now on, what the store says on the connections there are.
        fn hold(&self) {
            self.below.send_replace(self.made.load(Ordering::SeqCst));
        }

        /// Lets everything that was held back go on its way.
        fn let_go(&self) {
            self.below.send_replace(0);
        }

        /// How much the store has said that was held back, counting the end of a
        /// connection as something said.
        fn kept(&self) -> usize {
            self.kept.load(Ordering::SeqCst)
        }
    }

    // P9. A `Server` has no way to hold back what its store answers, so this is of
    // the worker's loop as P7 drives it: the store is served at an address, as a
    // cluster's is, and the loop reaches it through something that can hold its
    // answers back. The takeover is said as `Server::take_over` says it (section 6.4,
    // point 2), and the test links the edge to the new runner as the link-keeper
    // would. What it cannot see is how the old runner ended, `StoreLost` or `Stopped`:
    // the loop shows nobody the status of a runner.
    //
    /// A region that is taken over is fenced while it runs. A player breaks a block;
    /// the store makes the tick that applied it durable, and its answer is held back,
    /// so the runner cannot confirm the tick and the player is not acknowledged. The
    /// region is taken over. The player is acknowledged once, by the new runner, while
    /// the old one's answers are still held back; the block is gone; and when the
    /// answers are let go, nothing is acknowledged again.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_region_is_fenced_while_its_runner_waits_for_the_store_and_the_next_runner_acknowledges_once()
     {
        let world = open_world();
        let anywhere = SocketAddr::from(([127, 0, 0, 1], 0));
        let listener = std::net::TcpListener::bind(anywhere).expect("a free port");
        let served = clustine_worldstore::serve(world.clone(), listener).expect("it is served");
        let between = Between::before(served.local_addr()).await;
        let alone = Alone::start(&world, between.opener()).await;
        let (first, links) = alone.shown(&mut [], HOME, 0).await;
        let edge = edge_with("edge", vec![link_to(first, &links)]).await;
        let mut alice = joined(&edge.address, "Alice").await;
        // While the store's answers arrive, a block that is broken is acknowledged.
        digs(&mut alice, 2, 1).await;

        // From here the runner hears nothing of the store. A region that nobody does
        // anything in commits nothing, so the first thing the store says is its
        // answer to the commit of the tick that broke the block: by then that tick
        // is durable, and the runner does not know.
        between.hold();
        let kept = between.kept();
        let sequence = alice.dig(-1, FLOOR, 2).await.expect("connected");
        let waiting = Instant::now();
        while between.kept() == kept {
            assert!(waiting.elapsed() <= PATIENCE, "the store answered nothing");
            idle(&mut [&mut alice]).await;
        }
        assert!(
            alice.acknowledged_sequence < sequence,
            "a tick the store's answer was held back of was shown as confirmed"
        );
        assert_ne!(floor(&alice, -1, 2), AIR);

        // The takeover: the store is said to have seen an epoch one above.
        alone
            .refusals
            .send((HOME, first.epoch + 1))
            .expect("the loop is there");
        let (second, links) = alone.shown(&mut [&mut alice], HOME, first.epoch).await;
        assert!(second.epoch > first.epoch + 1, "{second:?} after {first:?}");
        assert_eq!(opened_with(&world, HOME), second.epoch);
        assert!(edge.relinks.replace(link_to(second, &links)).await);

        // Acknowledged by the new runner: the old one has still not heard the store.
        let acknowledged = |bot: &Bot| bot.acknowledged_sequence >= sequence;
        let waited = alice.wait_until(PATIENCE, acknowledged).await;
        waited.expect("the new runner acknowledges what the old one had applied");
        // And the block is gone. The player is shown that behind the acknowledgement
        // here, and not before it as when a tick is published: no runner ever
        // published the tick that broke the block, so the edge learns of the action
        // from the answer about its player and of the block from the snapshot of the
        // chunk, which comes later (ADR-0008, section 5, points 5 and 6 of resuming).
        let gone = |bot: &Bot| floor(bot, -1, 2) == AIR;
        let waited = alice.wait_until(PATIENCE, gone).await;
        waited.expect("the player is shown that the block is gone");

        // The old runner hears what the store had said, and that it has lost it.
        between.let_go();
        digs(&mut alice, 3, -1).await;
        digs(&mut alice, -2, -2).await;
        acknowledged_once(&alice);
        assert_eq!(alice.stats.teleports_confirmed, 1);
        let bob = joined(&edge.address, "Bob").await;
        for (x, z) in [(2, 1), (-1, 2), (3, -1), (-2, -2)] {
            assert_eq!(floor(&bob, x, z), AIR, "the block at {x}, {z}");
        }

        // The loop returns only when every runner it ever had has ended, the one
        // that was fenced among them.
        alone.stopped().await;
        edge.edge.abort();
        drop((alice, bob));

        // And the store has each block broken once: whoever runs the region next
        // shows the same.
        let again = Alone::start(&world, opener_of(&world)).await;
        let (third, links) = again.shown(&mut [], HOME, second.epoch).await;
        let edge = edge_with("another edge", vec![link_to(third, &links)]).await;
        let carol = joined(&edge.address, "Carol").await;
        for (x, z) in [(2, 1), (-1, 2), (3, -1), (-2, -2)] {
            assert_eq!(floor(&carol, x, z), AIR, "the block at {x}, {z}");
        }
        again.stopped().await;
        edge.edge.abort();
        served.stop();
    }

    // P9, and section 6.2 of the record: "a region that is named with another epoch
    // is opened before its runner is stopped."
    //
    /// The hello that takes a region from its runner is held back on its way to the
    /// store. Until the store has answered it, the runner goes on: the region is in
    /// no `Serving`, as one that is being opened, and the player whose edge is linked
    /// to the runner breaks a block and is acknowledged. When the hello is let
    /// through, the loop shows the region with the new epoch, and the new runner has
    /// what the old one had confirmed in the meantime.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_runner_whose_region_is_named_with_another_epoch_goes_on_until_the_store_has_answered_the_hello()
     {
        let world = open_world();
        // Hellos that are held back say so here, and wait for a word each.
        let holding = Arc::new(AtomicBool::new(false));
        let (entered, mut waits) = mpsc::unbounded_channel();
        let (let_through, word) = std::sync::mpsc::channel::<()>();
        let word = std::sync::Mutex::new(word);
        let store: Opener = {
            let (world, holding) = (world.clone(), Arc::clone(&holding));
            Arc::new(move |hello| {
                if holding.load(Ordering::SeqCst) {
                    let _ = entered.send(hello);
                    let _ = word.lock().expect("no test panics with it").recv();
                }
                world.open_region(hello)
            })
        };
        let alone = Alone::start(&world, store).await;
        let (first, links) = alone.shown(&mut [], HOME, 0).await;
        let edge = edge_with("edge", vec![link_to(first, &links)]).await;
        let mut alice = joined(&edge.address, "Alice").await;
        digs(&mut alice, 2, 1).await;

        holding.store(true, Ordering::SeqCst);
        alone
            .refusals
            .send((HOME, first.epoch + 1))
            .expect("the loop is there");
        let waiting = Instant::now();
        let hello = loop {
            if let Ok(hello) = waits.try_recv() {
                break hello;
            }
            assert!(
                waiting.elapsed() <= PATIENCE,
                "the loop did not open the region it was named with another epoch"
            );
            idle(&mut [&mut alice]).await;
        };
        assert_eq!(hello.region, HOME);
        assert!(hello.epoch > first.epoch + 1, "{hello:?} after {first:?}");
        // The store has not heard of it, and the old runner is its region's owner.
        assert_eq!(opened_with(&world, HOME), first.epoch);

        // The region is being opened, for everything else in the loop.
        let waiting = Instant::now();
        while alone.serving.borrow().contains_key(&HOME) {
            assert!(
                waiting.elapsed() <= PATIENCE,
                "a region that is being opened is shown as served"
            );
            idle(&mut [&mut alice]).await;
        }
        // And its runner runs: what the player does is applied, confirmed by the
        // store and acknowledged.
        digs(&mut alice, -1, 2).await;
        digs(&mut alice, 3, -1).await;
        assert!(alone.shows(HOME, 0).is_none());
        assert_eq!(opened_with(&world, HOME), first.epoch);

        holding.store(false, Ordering::SeqCst);
        let_through.send(()).expect("the hello waits");
        let (second, links) = alone.shown(&mut [&mut alice], HOME, first.epoch).await;
        assert_eq!(second, hello);
        assert_eq!(opened_with(&world, HOME), second.epoch);
        assert!(edge.relinks.replace(link_to(second, &links)).await);

        // The new runner has what the old one had confirmed, and the player goes on.
        digs(&mut alice, -2, -2).await;
        acknowledged_once(&alice);
        assert_eq!(alice.stats.teleports_confirmed, 1);
        let dug = [(2, 1), (-1, 2), (3, -1), (-2, -2)];
        for (x, z) in dug {
            assert_eq!(floor(&alice, x, z), AIR, "the block at {x}, {z}");
        }
        let bob = joined(&edge.address, "Bob").await;
        for (x, z) in dug {
            assert_eq!(floor(&bob, x, z), AIR, "the block at {x}, {z}");
        }
        alone.stopped().await;
        edge.edge.abort();
    }
}
