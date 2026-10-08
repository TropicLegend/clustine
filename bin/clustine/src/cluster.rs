//! The services as processes of their own, which together are a cluster.
//!
//! A coordinator divides the world into regions and decides which worker runs which. A
//! worker registers with it, is given a region, opens that region at the world store
//! and runs it. An edge learns from the coordinator where each region's worker is,
//! connects to all of them and then lets players in.
//!
//! A worker that loses the world store keeps its region and restores it from the store
//! once that is back. An edge whose link to a region ends keeps the region's players,
//! links to whoever runs the region then and resumes with it. Nothing players were
//! shown is lost on the way, because a region shows nothing that the world store does
//! not have. A coordinator that goes away is merely missed until it is back.

use std::collections::BTreeMap;
use std::future::Future;
use std::io;
use std::mem;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use clustine_coordinator::{
    ClientError, CoordinatorConfig, MoveAnswer, Mover, Orders, RoutingWatch, WorkerClient,
    WorkerEvent,
};
use clustine_edge::{Edge, EdgeConfig, EdgeIdentity, RegionLink, Relinks, Routing, Stopped};
use clustine_region::{Layout, RegionId, RoutingTable};
use clustine_rpc::{Assignment, EdgeMessage, RegionHello, Restored, Vouch, WorkerToEdge, tcp};
use clustine_sim::RegionConfig;
use clustine_worker::{Ended, Links, RegionRunner, RegionStatus, Worker};
use clustine_worldstore::{Store, StoreError, StoreHandle};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout};
use tracing::{debug, info, warn};

use crate::{LINK_CAPACITY, generator, spawn_point, starting_hotbar};

/// The ports the services listen on unless told otherwise.
pub const COORDINATOR_PORT: u16 = 25600;
pub const WORKER_PORT: u16 = 25601;
pub const WORLDSTORE_PORT: u16 = 25602;

/// How long to wait before trying again to reach a service that is not there.
const RETRY: Duration = Duration::from_secs(1);

/// Settings of a coordinator process.
#[derive(Debug, Clone)]
pub struct CoordinatorArgs {
    pub listen: SocketAddr,
    /// The chunk x coordinates at which the world is divided into regions, ascending.
    pub boundaries: Vec<i32>,
    /// How long a worker may be silent before its region is given to another.
    pub lease: Duration,
}

/// Runs a coordinator until the process is asked to stop.
pub async fn coordinator(args: CoordinatorArgs) -> Result<()> {
    let layout = Layout::new(args.boundaries).context("dividing the world into regions")?;
    let listener = TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("listening on {}", args.listen))?;
    info!(address = %listener.local_addr()?, regions = layout.region_count(), "coordinating");
    let config = CoordinatorConfig {
        layout,
        spawn: spawn_point(),
        lease: args.lease,
    };
    tokio::select! {
        served = clustine_coordinator::serve(listener, config) => served.context("coordinating"),
        _ = crate::stop_signal() => Ok(()),
    }
}

/// Runs a world store for the world in the directory `world` until the process is
/// asked to stop.
pub async fn worldstore(listen: SocketAddr, world: PathBuf) -> Result<()> {
    let store = Store::local(&world, generator())
        .with_context(|| format!("opening the world in {}", world.display()))?;
    let listener =
        std::net::TcpListener::bind(listen).with_context(|| format!("listening on {listen}"))?;
    let server = clustine_worldstore::serve(store, listener).context("serving the world")?;
    info!(address = %server.local_addr(), world = %world.display(), "serving the world");
    crate::stop_signal().await;
    info!("shutting down");
    // Closes the connections; what their owners asked for before is done first.
    tokio::task::spawn_blocking(move || server.stop()).await?;
    Ok(())
}

/// How often a worker looks at its region: whether it still ticks and whether it has
/// lost the world store. What it vouches for to the coordinator follows from that.
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

/// A region the coordinator has given this worker, and what it takes to run it.
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

/// How long a worker that has been told to stop waits for its region to be handed to
/// another worker before it stops anyway. Kubernetes gives it 30 seconds in all.
const LEAVE_WITHIN: Duration = Duration::from_secs(20);

/// What a worker is doing about its region.
enum Phase {
    /// It has none. `refused` is an assignment it is not to take up again: the world
    /// store did not let it open the region, it released the region, or the region was
    /// taken from it. The coordinator's next orders are waited for.
    Idle { refused: Option<Assignment> },
    /// It waits for the world store to open the region, for the first time or again.
    Opening { held: Held, opening: Opening },
    /// The region is restored and ticks.
    Running {
        held: Held,
        running: Worker,
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
    /// What the worker vouches for to the coordinator in this phase.
    fn vouches(&self) -> Vec<(RegionId, Vouch)> {
        match self {
            Phase::Idle { .. } => Vec::new(),
            Phase::Opening { held, .. } => vec![(held.assignment.region, Vouch::WaitingForStore)],
            Phase::Running { held, ticked, .. } if ticked.elapsed() < TICKED_WITHIN => {
                vec![(held.assignment.region, Vouch::Committed)]
            }
            // A region that has stopped ticking is not vouched for: the store does not
            // confirm what it does, and yet has not let go of it.
            Phase::Running { .. } => Vec::new(),
            // It stops ticking on purpose, and the coordinator knows.
            Phase::Releasing { .. } => Vec::new(),
        }
    }

    /// The assignment the worker holds in this phase, if any.
    fn held(&self) -> Option<Assignment> {
        match self {
            Phase::Idle { .. } => None,
            Phase::Opening { held, .. }
            | Phase::Running { held, .. }
            | Phase::Releasing { held, .. } => Some(held.assignment),
        }
    }

    /// Resolves when what the worker waits for in this phase has come about: the world
    /// store has answered the opening of the region, or a release has ended. Never in
    /// another phase. The phase has to be left once this has resolved.
    async fn settled(&mut self) -> Settled {
        match self {
            Phase::Opening { opening, .. } => Settled::Opened(opening.as_mut().await),
            Phase::Releasing { status, .. } => loop {
                if let Some(ended) = status.ended() {
                    return Settled::Released(ended);
                }
                sleep(RELEASE_LOOK).await;
            },
            _ => std::future::pending().await,
        }
    }
}

/// What a worker waited for and got; see [`Phase::settled`].
enum Settled {
    Opened(Result<(StoreHandle, Restored), StoreError>),
    Released(Ended),
}

/// What the task that holds the connection to the coordinator passes on.
enum Word {
    /// New orders, or a region to release.
    Event(WorkerEvent),
    /// The coordinator refuses the worker, for the reason given.
    Refused(String),
    /// A worker that is leaving may exit: the coordinator has closed its connection, or
    /// cannot be reached.
    Dismissed,
}

/// Runs a worker until the process is asked to stop: registers with the coordinator,
/// opens the region it is given at the world store, restores and runs it, and accepts
/// the links of edges while it does.
///
/// A worker that loses the world store keeps its region: it closes the region's links,
/// opens the region again when the store answers and carries on with what the store
/// has. If the store says that the region has had a later owner, the worker tells the
/// coordinator and waits for its next orders. It releases its region when the
/// coordinator asks for that, and drops it when the coordinator has given it to another
/// worker; either way it waits to be given a region again.
///
/// Asked to stop, it tells the coordinator that it is leaving and goes on until the
/// coordinator has moved its region to another worker and closed the connection, for
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

    // What the task that holds the connection to the coordinator is told, and tells.
    let (vouches, vouched) = watch::channel(Vec::new());
    let (holding, held) = watch::channel(None);
    let (leaving, left) = watch::channel(false);
    let (refusals, refused) = mpsc::unbounded_channel();
    let (releases, released) = mpsc::unbounded_channel();
    let (words_in, mut words) = mpsc::unbounded_channel();
    // The receiver is right here.
    let _ = words_in.send(Word::Event(WorkerEvent::Orders(first)));
    let registered = tokio::spawn(stay_registered(
        coordinator,
        args.clone(),
        Reports {
            vouched,
            held,
            left,
            refused,
            released,
        },
        words_in,
    ));
    // Edges are let in only while there is a restored region to link them to.
    let (serving, served) = watch::channel(None);
    let accepting = tokio::spawn(accept_edges(listener, served));

    let checkpoint_interval =
        (args.checkpoint_interval.as_millis() / clustine_worker::TICK.as_millis()) as u64;
    let mut phase = Phase::Idle { refused: None };
    // The assignment this worker released last, to say so again if the coordinator has
    // not heard.
    let mut let_go: Option<Assignment> = None;
    let mut look = tokio::time::interval(LOOK);
    // Set when the worker has been told to stop: when it stops waiting, and what
    // listens for its being told again.
    let mut leave_by: Option<Instant> = None;
    let mut again: Option<Pin<Box<dyn Future<Output = ()> + Send>>> = None;
    let outcome = loop {
        tokio::select! {
            _ = &mut stop, if leave_by.is_none() => {
                info!("told to stop; asking for what this worker runs to be moved first");
                leave_by = Some(Instant::now() + LEAVE_WITHIN);
                again = Some(Box::pin(crate::stop_signal()));
                leaving.send_replace(true);
            }
            () = told_again(&mut again), if again.is_some() => {
                info!("told to stop again; stopping at once");
                break Ok(());
            }
            () = sleep_until_some(leave_by), if leave_by.is_some() => {
                warn!("nobody took over in time; stopping with what this worker runs");
                break Ok(());
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
                        let asked = |held: &Held| {
                            (held.assignment.region, held.assignment.epoch) == (region, epoch)
                        };
                        match mem::replace(&mut phase, Phase::Idle { refused: None }) {
                            Phase::Running { held, running, status, .. } if asked(&held) => {
                                info!(%region, epoch, "asked to release the region");
                                running.begin_release();
                                phase = Phase::Releasing { held, running, status };
                            }
                            // The region is not open yet, so there is nothing to bring
                            // up to date. Should the store still answer, the handle is
                            // dropped with the future, which closes the region.
                            Phase::Opening { held, .. } if asked(&held) => {
                                info!(%region, epoch, "asked to release the region while opening it");
                                let _ = releases.send((region, epoch));
                                holding.send_replace(None);
                                let_go = Some(held.assignment);
                                phase = Phase::Idle { refused: Some(held.assignment) };
                            }
                            // Under way already, or not this worker's: then it is as
                            // released as it can be, and the coordinator is told so.
                            other => {
                                let releasing = matches!(&other, Phase::Releasing { held, .. } if asked(held));
                                phase = other;
                                if !releasing {
                                    let _ = releases.send((region, epoch));
                                }
                            }
                        }
                        vouches.send_replace(phase.vouches());
                    }
                    WorkerEvent::Orders(next) => {
                        // The coordinator still believes this worker to have what it
                        // has released: the word of it was lost.
                        if let Some(released) = let_go
                            && phase.held().is_none()
                            && next.assignments.contains(&released)
                        {
                            let _ = releases.send((released.region, released.epoch));
                        }
                        match phase.held() {
                            None => {
                                let Phase::Idle { refused } = &phase else {
                                    unreachable!("only an idle worker holds nothing");
                                };
                                // A worker runs one region for now, and one that is
                                // leaving takes up none.
                                let offered = next
                                    .assignments
                                    .iter()
                                    .find(|offered| Some(**offered) != *refused)
                                    .filter(|_| leave_by.is_none());
                                let Some(assignment) = offered else {
                                    info!("registered; waiting to be given a region");
                                    continue;
                                };
                                let held = match hold(&next, *assignment) {
                                    Ok(held) => held,
                                    Err(error) => break Err(error),
                                };
                                info!(
                                    region = %assignment.region,
                                    epoch = assignment.epoch,
                                    "given a region"
                                );
                                holding.send_replace(Some((held.assignment, held.hello.layout)));
                                let opening = Box::pin(open_region(args.store.clone(), held.hello));
                                phase = Phase::Opening { held, opening };
                                vouches.send_replace(phase.vouches());
                            }
                            Some(assignment) if next.assignments.contains(&assignment) => {}
                            // The region is another worker's now, or nobody's. That is
                            // not this worker's fault, and the store sees to it that it
                            // can do no harm; it lets go and waits for a region again.
                            Some(assignment) => {
                                warn!(
                                    region = %assignment.region,
                                    epoch = assignment.epoch,
                                    "the coordinator has taken the region; letting go of it"
                                );
                                serving.send_replace(None);
                                holding.send_replace(None);
                                let dropped = mem::replace(
                                    &mut phase,
                                    Phase::Idle { refused: Some(assignment) },
                                );
                                if let Phase::Running { running, .. }
                                | Phase::Releasing { running, .. } = dropped
                                {
                                    // As it is: saved if the store still listens to
                                    // this worker, and left to the next owner if not.
                                    let stopped =
                                        tokio::task::spawn_blocking(move || running.stop()).await;
                                    if let Err(error) = stopped {
                                        break Err(error.into());
                                    }
                                }
                                vouches.send_replace(phase.vouches());
                            }
                        }
                    }
                }
            }
            settled = phase.settled() => match settled {
                Settled::Opened(opened) => {
                    let Phase::Opening { held, .. } =
                        mem::replace(&mut phase, Phase::Idle { refused: None })
                    else {
                        unreachable!("only an opening resolves");
                    };
                    let region = held.assignment.region;
                    match opened {
                        Ok((store, restored)) => {
                            let tick = restored.tick();
                            let runner = match RegionRunner::restore(held.config.clone(), store, restored) {
                                Ok(runner) => runner.with_checkpoint_interval(checkpoint_interval),
                                Err(error) => {
                                    let context = format!("restoring region {region}");
                                    break Err(anyhow::Error::new(error).context(context));
                                }
                            };
                            let status = runner.status();
                            serving.send_replace(Some((held.hello, runner.links())));
                            let running = Worker::spawn(runner);
                            info!(
                                %region,
                                epoch = held.assignment.epoch,
                                tick,
                                address = %args.advertise,
                                "running a region"
                            );
                            phase = Phase::Running {
                                held,
                                running,
                                status,
                                tick,
                                // A region that has just been restored is as good as one
                                // that has just ticked.
                                ticked: Instant::now(),
                            };
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
                            holding.send_replace(None);
                            phase = Phase::Idle { refused: Some(held.assignment) };
                        }
                        Err(error) => {
                            let context =
                                format!("opening region {region} at the world store {}", args.store);
                            break Err(anyhow::Error::new(error).context(context));
                        }
                    }
                    vouches.send_replace(phase.vouches());
                }
                Settled::Released(ended) => {
                    let Phase::Releasing { held, running, .. } =
                        mem::replace(&mut phase, Phase::Idle { refused: None })
                    else {
                        unreachable!("only a release ends");
                    };
                    let (region, epoch) = (held.assignment.region, held.assignment.epoch);
                    serving.send_replace(None);
                    // The region's thread has ended by itself; this only waits for it to
                    // be gone.
                    if let Err(error) = tokio::task::spawn_blocking(move || running.stop()).await {
                        break Err(error.into());
                    }
                    // With the store lost on the way the region is as released as it can
                    // be: what was confirmed the store has, and the rest was never shown.
                    info!(%region, epoch, ?ended, "released the region");
                    let _ = releases.send((region, epoch));
                    holding.send_replace(None);
                    let_go = Some(held.assignment);
                    phase = Phase::Idle { refused: Some(held.assignment) };
                    vouches.send_replace(phase.vouches());
                }
            },
            _ = look.tick() => {
                let lost = match &mut phase {
                    Phase::Running { status, tick, ticked, .. } => {
                        let now = status.tick.load(Ordering::Relaxed);
                        if now != *tick {
                            *tick = now;
                            *ticked = Instant::now();
                        }
                        status.store_lost.load(Ordering::Relaxed)
                    }
                    _ => false,
                };
                if lost {
                    let Phase::Running { held, running, .. } =
                        mem::replace(&mut phase, Phase::Idle { refused: None })
                    else {
                        unreachable!("only a running region loses the store");
                    };
                    warn!(
                        region = %held.assignment.region,
                        "lost the world store; opening the region again"
                    );
                    serving.send_replace(None);
                    // The region has stopped by itself and closed its links; this only
                    // waits for its thread to be gone.
                    let stopped = tokio::task::spawn_blocking(move || running.stop()).await;
                    if let Err(error) = stopped {
                        break Err(error.into());
                    }
                    let opening = Box::pin(open_region(args.store.clone(), held.hello));
                    phase = Phase::Opening { held, opening };
                }
                vouches.send_replace(phase.vouches());
            }
        }
    };
    info!("shutting down");
    accepting.abort();
    registered.abort();
    if let Phase::Running { running, .. } | Phase::Releasing { running, .. } = phase {
        // Waits for the current tick and for the world store to have what changed. A
        // release that is under way is let go of as it is.
        tokio::task::spawn_blocking(move || running.stop()).await?;
    }
    outcome
}

/// Resolves when the process is told to stop once more, if `again` listens for that.
async fn told_again(again: &mut Option<Pin<Box<dyn Future<Output = ()> + Send>>>) {
    match again {
        Some(again) => again.as_mut().await,
        None => std::future::pending().await,
    }
}

/// What it takes to run the region of `assignment` under `orders`.
fn hold(orders: &Orders, assignment: Assignment) -> Result<Held> {
    let area = orders
        .layout
        .area(assignment.region)
        .context("the coordinator named a region that its layout does not have")?;
    Ok(Held {
        assignment,
        hello: RegionHello {
            region: assignment.region,
            epoch: assignment.epoch,
            layout: orders.layout.fingerprint(),
        },
        // The region's entity ids are the store's to say, not the coordinator's.
        config: RegionConfig {
            spawn: orders.spawn,
            area,
            starting_hotbar: starting_hotbar(),
        },
    })
}

/// Opens the region at the world store, trying until the store can be reached. An
/// error is the store's refusal.
async fn open_region(
    address: String,
    hello: RegionHello,
) -> Result<(StoreHandle, Restored), StoreError> {
    loop {
        let store = address.clone();
        let opened = tokio::task::spawn_blocking(move || StoreHandle::connect(&store, hello))
            .await
            .map_err(io::Error::other)?;
        match opened {
            Err(StoreError::Io(error)) => {
                info!(%error, store = %address, "the world store cannot be reached yet");
                sleep(RETRY).await;
            }
            opened => return opened,
        }
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
    /// The region the worker holds, if any, and the fingerprint of the layout it is
    /// part of.
    held: watch::Receiver<Option<(Assignment, u64)>>,
    /// Whether the worker has been told to stop.
    left: watch::Receiver<bool>,
    /// Regions the world store did not let the worker open, with the epoch it has seen.
    refused: mpsc::UnboundedReceiver<(RegionId, u64)>,
    /// Regions the worker has let go of, with the epoch it held them with.
    released: mpsc::UnboundedReceiver<(RegionId, u64)>,
}

/// Holds the worker's connection to the coordinator: says what the worker vouches for,
/// what the store refused, what the worker released and that it is leaving, and passes
/// on what the coordinator says, or last of all why it refuses the worker.
///
/// A coordinator that goes away knows nothing when it is back, so the worker registers
/// again and tells it what it holds. The region keeps running meanwhile. A new
/// connection vouches for nothing by itself and knows of no leaving, so both are said
/// again on it.
///
/// A worker that is leaving does not register again: the coordinator closes the
/// connection of one that owns nothing any more, which is how the worker knows that it
/// may exit, and if the coordinator cannot be reached there is nobody to hand anything
/// to.
async fn stay_registered(
    mut coordinator: WorkerClient,
    args: WorkerArgs,
    mut reports: Reports,
    words: mpsc::UnboundedSender<Word>,
) {
    coordinator.vouch(reports.vouched.borrow_and_update().clone());
    if *reports.left.borrow_and_update() {
        coordinator.leaving();
    }
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
                        let held = *reports.held.borrow();
                        let holding: Vec<_> = held.iter().map(|(held, _)| *held).collect();
                        let registered = WorkerClient::register(
                            &args.coordinator,
                            &args.name,
                            &args.advertise,
                            &holding,
                            held.map(|(_, layout)| layout),
                        );
                        match registered.await {
                            Ok((coordinator, next)) => {
                                info!("registered again");
                                coordinator.vouch(reports.vouched.borrow_and_update().clone());
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
        }
    }
}

/// Accepts the connections of edges and attaches them as links to the region that
/// `serving` names, for as long as it names one.
async fn accept_edges(
    listener: TcpListener,
    serving: watch::Receiver<Option<(RegionHello, Links)>>,
) {
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

/// Makes a connection a link of the region this worker runs, if it is about that region
/// as it is now and the region is restored. Returns false if the other side went away
/// without saying what it wanted.
async fn greet_edge(
    stream: TcpStream,
    serving: watch::Receiver<Option<(RegionHello, Links)>>,
) -> Result<bool> {
    let incoming = match tcp::accept(stream).await {
        Ok(incoming) => incoming,
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(false),
        Err(error) => return Err(error).context("no greeting"),
    };
    let asked = incoming.hello();
    // As of now, and not of when the connection was made: a region can have been
    // restored, or lost, while the greeting was on its way.
    let serving = serving.borrow().clone();
    let Some((region, links)) = serving else {
        let reason = "this worker has no region running at the moment".to_owned();
        incoming.refuse(reason).await?;
        bail!("no region is running");
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

/// Settings of an edge process.
#[derive(Debug, Clone)]
pub struct EdgeArgs {
    /// Name of this edge, by which regions know it again after a restart.
    pub name: String,
    /// Host and port of the coordinator.
    pub coordinator: String,
    /// The address players connect to.
    pub bind: SocketAddr,
    pub edge: EdgeConfig,
}

/// How long connecting to a worker may take before the edge tries again later. A
/// worker that is gone may not refuse the connection but swallow it.
const LINK_TIMEOUT: Duration = Duration::from_secs(2);

/// Runs an edge until the process is asked to stop. Players are let in once every
/// region has had a worker, and stay when a region changes hands or cannot be reached
/// for a while: the edge links to whoever runs it then and resumes.
///
/// Fails if another process has taken this edge's name.
pub async fn edge(args: EdgeArgs) -> Result<()> {
    let stop = crate::stop_signal();
    tokio::pin!(stop);
    let (watch, table) = tokio::select! {
        _ = &mut stop => return Ok(()),
        table = whole_world(&args.coordinator) => table,
    };

    // One start for as long as the process lives: what the edge keeps for the regions
    // lives as long, and so do the numbers it gives its messages.
    let identity = EdgeIdentity::starting_now(&args.name);
    let (routing, relinks) = Routing::new(table.layout.clone(), table.spawn, identity, Vec::new());
    let edge = Edge::bind(args.bind, args.edge.clone(), routing)
        .await
        .with_context(|| format!("listening on {}", args.bind))?;
    info!(address = %edge.local_addr()?, regions = table.routes.len(), "listening");

    // Dropping the edge's future, which happens when another branch wins, closes every
    // connection.
    tokio::select! {
        _ = &mut stop => Ok(()),
        stopped = edge.run() => match stopped {
            Stopped::Superseded => bail!(
                "another edge runs under the name {}; this one must not come back as it",
                args.name
            ),
            Stopped::Abandoned => bail!("the edge stopped unexpectedly"),
        },
        () = keep_linked(&args.coordinator, watch, table, relinks) => {
            bail!("the edge is gone")
        }
    }
}

/// Waits until the coordinator has a worker for every region, trying until it can be
/// reached. Returns the connection and the table.
async fn whole_world(coordinator: &str) -> (RoutingWatch, RoutingTable) {
    loop {
        let watching = async {
            let mut watch = RoutingWatch::connect(coordinator).await?;
            loop {
                let table = watch.next().await?;
                if table.is_complete() {
                    return Ok::<_, ClientError>((watch, table));
                }
                info!(
                    with_worker = table.routes.len(),
                    regions = table.layout.region_count(),
                    "waiting for every region to have a worker"
                );
            }
        };
        match watching.await {
            Ok(whole) => return whole,
            Err(error) => {
                info!(%error, coordinator, "the coordinator cannot be reached yet");
                sleep(RETRY).await;
            }
        }
    }
}

/// How often the edge tries to link to a region's owner while that is new: the owner
/// of a region that was moved takes links a moment after the routing table names it, and
/// its players stand still until the edge is through.
const LINK_RETRY_AT_FIRST: Duration = Duration::from_millis(100);

/// For how long it tries that often, before it tries every [`RETRY`].
const LINK_RETRY_EAGERLY_FOR: Duration = Duration::from_secs(2);

/// What the link-keeper knows of one region.
#[derive(Default)]
struct LinkState {
    /// The epoch of the owner the edge has a link to.
    linked: Option<u64>,
    /// The epoch of the owner an attempt to link is under way to.
    trying: Option<u64>,
    /// Since when the edge has been without a link to the owner the table names.
    since: Option<Instant>,
    /// When to try again, if the last attempt failed.
    again: Option<Instant>,
}

/// Keeps the edge linked to whoever runs each region, for as long as the edge is
/// there: links to every region of `table`, and links again when the coordinator names
/// another owner or the edge says that a link has ended. Each region is tried by itself
/// and at once, so that a worker that does not answer keeps nobody but its own region
/// waiting, and a new route is taken up also while the old one is still being tried. A
/// worker that cannot be reached is tried again, often at first; a coordinator that
/// goes away is waited for, with the regions where they were.
async fn keep_linked(
    coordinator: &str,
    watch: RoutingWatch,
    mut table: RoutingTable,
    mut relinks: Relinks,
) {
    let layout = table.layout.fingerprint();
    let mut watch = Some(watch);
    let mut regions: BTreeMap<RegionId, LinkState> = BTreeMap::new();
    // The attempts under way: each ends with the region, the epoch it was for, and the
    // link if there is one.
    let mut attempts = JoinSet::new();
    loop {
        // An attempt for every region that is not linked to the owner the table names,
        // has none under way to that owner, and is not waiting to try again.
        let now = Instant::now();
        for route in &table.routes {
            let state = regions.entry(route.region).or_default();
            if state.linked == Some(route.epoch) {
                state.since = None;
                continue;
            }
            state.since.get_or_insert(now);
            let due = state.again.is_none_or(|again| again <= now);
            if state.trying == Some(route.epoch) || !due {
                continue;
            }
            state.trying = Some(route.epoch);
            state.again = None;
            let (region, epoch, address) = (route.region, route.epoch, route.address.clone());
            let hello = RegionHello {
                region,
                epoch,
                layout,
            };
            attempts.spawn(async move {
                let connecting =
                    tcp::connect::<EdgeMessage, WorkerToEdge>(&address, hello, LINK_CAPACITY);
                let link = match timeout(LINK_TIMEOUT, connecting).await {
                    Ok(Ok(end)) => Some(end),
                    // A worker that restores its region takes no links until it is done.
                    Ok(Err(error)) => {
                        debug!(%region, %address, %error, "a region cannot be linked to yet");
                        None
                    }
                    Err(_) => {
                        debug!(%region, %address, "a worker did not answer in time");
                        None
                    }
                };
                (region, epoch, address, link)
            });
        }
        let again = regions.values().filter_map(|state| state.again).min();

        tokio::select! {
            Some(ended) = attempts.join_next() => {
                // An attempt neither panics nor is cancelled.
                let Ok((region, epoch, address, link)) = ended else {
                    continue;
                };
                let state = regions.entry(region).or_default();
                if state.trying == Some(epoch) {
                    state.trying = None;
                }
                let current = table.route(region).is_some_and(|route| route.epoch == epoch);
                match link {
                    // The table has moved on meanwhile; the link is closed by dropping it.
                    Some(_) if !current => {}
                    Some(end) => {
                        if !relinks.replace(RegionLink { region, epoch, end }).await {
                            return;
                        }
                        info!(%region, epoch, %address, "linked to a region");
                        state.linked = Some(epoch);
                        state.since = None;
                    }
                    None if current => {
                        let eager = state
                            .since
                            .is_some_and(|since| since.elapsed() < LINK_RETRY_EAGERLY_FOR);
                        let wait = if eager { LINK_RETRY_AT_FIRST } else { RETRY };
                        state.again = Some(Instant::now() + wait);
                    }
                    None => {}
                }
            }
            ended = relinks.ended() => match ended {
                // A link to an owner the edge has left behind already ended.
                Some((region, epoch)) => {
                    let state = regions.entry(region).or_default();
                    if state.linked == Some(epoch) {
                        state.linked = None;
                        state.again = None;
                    }
                }
                None => return,
            },
            next = next_table(&mut watch), if watch.is_some() => match next {
                Some(next) if next.layout == table.layout => {
                    // A region with a new owner is tried at once, whatever the last
                    // attempt at the old one came to.
                    for route in &next.routes {
                        let changed = table
                            .route(route.region)
                            .is_none_or(|old| old.epoch != route.epoch);
                        if changed && let Some(state) = regions.get_mut(&route.region) {
                            state.again = None;
                            state.since = None;
                        }
                    }
                    table = next;
                }
                Some(_) => warn!("the coordinator divides the world differently now; keeping to the layout this edge started with"),
                None => {
                    warn!("lost the coordinator; carrying on with the regions as they are");
                    watch = None;
                }
            },
            () = sleep_until_some(again), if again.is_some() => {}
            // A coordinator to look for.
            () = sleep(RETRY), if watch.is_none() => {
                watch = RoutingWatch::connect(coordinator).await.ok();
            }
        }
    }
}

/// Sleeps until `when`, which the caller has made sure is there.
async fn sleep_until_some(when: Option<Instant>) {
    if let Some(when) = when {
        tokio::time::sleep_until(when.into()).await;
    }
}

/// The next routing table, or `None` if the connection to the coordinator is lost.
async fn next_table(watch: &mut Option<RoutingWatch>) -> Option<RoutingTable> {
    watch.as_mut()?.next().await.ok()
}

/// Settings of `clustine move`.
#[derive(Debug, Clone)]
pub struct MoveArgs {
    /// Host and port of the coordinator.
    pub coordinator: String,
    pub region: RegionId,
    /// The worker to move the region to, or any worker that waits.
    pub to: Option<String>,
}

/// Asks the coordinator to move a region and says what came of it; see
/// `docs/adr/0009-moving-a-region.md`. Fails if the move is refused or the coordinator
/// goes away before it says how it ended.
///
/// The time it prints is from asking to the region being another worker's. How long
/// players stood still it cannot know: that also takes the new owner restoring the
/// region and the edges resuming with it.
pub async fn move_region(args: MoveArgs) -> Result<()> {
    let asked = Instant::now();
    let mut mover = Mover::ask(&args.coordinator, args.region, args.to.as_deref())
        .await
        .with_context(|| format!("reaching the coordinator at {}", args.coordinator))?;
    loop {
        let answer = mover
            .next()
            .await
            .context("the coordinator went away before it said what came of the move")?;
        match answer {
            MoveAnswer::Refused { reason } => bail!("the coordinator refused: {reason}"),
            MoveAnswer::Begun { from, to } => {
                println!("{from} is releasing region {} for {to}", args.region);
            }
            MoveAnswer::Done {
                to,
                epoch,
                released,
            } => {
                let how = if released {
                    "released by its owner"
                } else {
                    "taken from its owner, which did not release it in time"
                };
                println!(
                    "region {} is run by {to} with epoch {epoch} now, {how}, {} ms after asking",
                    args.region,
                    asked.elapsed().as_millis()
                );
                return Ok(());
            }
        }
    }
}
