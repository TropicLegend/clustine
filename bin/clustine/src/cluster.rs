//! The services as processes of their own, which together are a cluster.
//!
//! A coordinator divides the world into regions and decides which worker runs which. A
//! worker registers with it, is given a region, opens that region at the world store
//! and runs it. An edge learns from the coordinator where each region's worker is,
//! connects to all of them and then lets players in.
//!
//! A worker that loses the world store keeps its region and restores it from the store
//! once that is back. The edge is not there yet: when a region's link ends, it closes
//! every player's connection and starts over once the world is whole again. Nothing
//! players were shown is lost, because a region shows nothing that the world store does
//! not have. A coordinator that goes away is merely missed until it is back.

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
use clustine_coordinator::{ClientError, CoordinatorConfig, Orders, RoutingWatch, WorkerClient};
use clustine_edge::{Edge, EdgeConfig, EdgeIdentity, RegionLink, Routing};
use clustine_region::{Layout, RegionId, RoutingTable};
use clustine_rpc::{Assignment, EdgeMessage, RegionHello, Restored, Vouch, WorkerToEdge, tcp};
use clustine_sim::RegionConfig;
use clustine_worker::{Links, RegionRunner, RegionStatus, Worker};
use clustine_worldstore::{Store, StoreError, StoreHandle};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::time::sleep;
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

/// What a worker is doing about its region.
enum Phase {
    /// It has none. `refused` is the assignment the world store did not let it open, if
    /// that is why: the coordinator's next orders are waited for, and that assignment is
    /// not taken up again.
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
        }
    }

    /// Resolves when the world store has answered the opening of the region, and never
    /// in another phase. The phase has to be left once it has resolved.
    async fn opened(&mut self) -> Result<(StoreHandle, Restored), StoreError> {
        match self {
            Phase::Opening { opening, .. } => opening.as_mut().await,
            _ => std::future::pending().await,
        }
    }
}

/// Runs a worker until the process is asked to stop: registers with the coordinator,
/// opens the region it is given at the world store, restores and runs it, and accepts
/// the links of edges while it does.
///
/// A worker that loses the world store keeps its region: it closes the region's links,
/// opens the region again when the store answers and carries on with what the store
/// has. If the store says that the region has had a later owner, the worker tells the
/// coordinator and waits for its next orders. Fails if the coordinator gives the region
/// to another worker; the process is then meant to be started again, to register afresh.
pub async fn worker(args: WorkerArgs) -> Result<()> {
    let listener = TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("listening on {}", args.listen))?;

    let stop = crate::stop_signal();
    tokio::pin!(stop);
    let (coordinator, first) = tokio::select! {
        registered = register(&args) => registered?,
        _ = &mut stop => return Ok(()),
    };

    // What the task that holds the connection to the coordinator is told, and tells.
    let (vouches, vouched) = watch::channel(Vec::new());
    let (holding, held) = watch::channel(None);
    let (refusals, refused) = mpsc::unbounded_channel();
    let (orders_in, mut orders) = mpsc::unbounded_channel();
    // The receiver is right here.
    let _ = orders_in.send(Ok(first));
    let registered = tokio::spawn(stay_registered(
        coordinator,
        args.clone(),
        Reports {
            vouched,
            held,
            refused,
        },
        orders_in,
    ));
    // Edges are let in only while there is a restored region to link them to.
    let (serving, served) = watch::channel(None);
    let accepting = tokio::spawn(accept_edges(listener, served));

    let checkpoint_interval =
        (args.checkpoint_interval.as_millis() / clustine_worker::TICK.as_millis()) as u64;
    let mut phase = Phase::Idle { refused: None };
    let mut look = tokio::time::interval(LOOK);
    let outcome = loop {
        tokio::select! {
            _ = &mut stop => break Ok(()),
            next = orders.recv() => {
                let next = match next {
                    Some(Ok(next)) => next,
                    Some(Err(reason)) => break Err(anyhow!("the coordinator refused: {reason}")),
                    None => break Err(anyhow!("lost the coordinator for good")),
                };
                match &phase {
                    Phase::Idle { refused } => {
                        // A worker runs one region for now.
                        let offered = next
                            .assignments
                            .iter()
                            .find(|offered| Some(**offered) != *refused);
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
                    Phase::Opening { held, .. } | Phase::Running { held, .. } => {
                        if !next.assignments.contains(&held.assignment) {
                            break Err(anyhow!(
                                "the coordinator has given region {} to another worker",
                                held.assignment.region
                            ));
                        }
                    }
                }
            }
            opened = phase.opened() => {
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
    if let Phase::Running { running, .. } = phase {
        // Waits for the current tick and for the world store to have what changed.
        tokio::task::spawn_blocking(move || running.stop()).await?;
    }
    outcome
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
    /// Regions the world store did not let the worker open, with the epoch it has seen.
    refused: mpsc::UnboundedReceiver<(RegionId, u64)>,
}

/// Holds the worker's connection to the coordinator: says what the worker vouches for
/// and what the store refused, and passes on the coordinator's orders, or last of all
/// why it refuses the worker.
///
/// A coordinator that goes away knows nothing when it is back, so the worker registers
/// again and tells it what it holds. The region keeps running meanwhile. A new
/// connection vouches for nothing by itself, so what the worker vouches for is said
/// again on it.
async fn stay_registered(
    mut coordinator: WorkerClient,
    args: WorkerArgs,
    mut reports: Reports,
    orders: mpsc::UnboundedSender<Result<Orders, String>>,
) {
    coordinator.vouch(reports.vouched.borrow_and_update().clone());
    loop {
        tokio::select! {
            next = coordinator.next() => match next {
                Ok(next) => {
                    if orders.send(Ok(next)).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    warn!(%error, "lost the coordinator; carrying on and registering again");
                    coordinator = loop {
                        sleep(RETRY).await;
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
                                if orders.send(Ok(next)).is_err() {
                                    return;
                                }
                                break coordinator;
                            }
                            Err(ClientError::Refused(reason)) => {
                                let _ = orders.send(Err(reason));
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
            Some((region, seen)) = reports.refused.recv() => {
                coordinator.epoch_refused(region, seen);
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

/// Runs an edge until the process is asked to stop. Players are let in while every
/// region has a worker that the edge is connected to; whenever that ends, everyone is
/// disconnected and the edge starts over.
pub async fn edge(args: EdgeArgs) -> Result<()> {
    let stop = crate::stop_signal();
    tokio::pin!(stop);
    loop {
        tokio::select! {
            _ = &mut stop => return Ok(()),
            ended = serve_players(&args) => match ended {
                Ok(reason) => warn!(reason, "disconnected everyone; starting over"),
                Err(error) => info!(error = format!("{error:#}"), "the world is not whole yet"),
            },
        }
        tokio::select! {
            _ = &mut stop => return Ok(()),
            _ = sleep(RETRY) => {}
        }
    }
}

/// Connects to every region and lets players in for as long as the regions stay with
/// the workers they are with. Returns why that ended; an error means it did not get as
/// far as letting players in.
async fn serve_players(args: &EdgeArgs) -> Result<&'static str> {
    let mut watch = RoutingWatch::connect(&args.coordinator)
        .await
        .with_context(|| format!("reaching the coordinator at {}", args.coordinator))?;
    let mut table = watch.next().await?;
    while !table.is_complete() {
        info!(
            with_worker = table.routes.len(),
            regions = table.layout.region_count(),
            "waiting for every region to have a worker"
        );
        table = watch.next().await?;
    }

    let layout = table.layout.fingerprint();
    // The players and everything the edge knew are gone when it starts over, and what
    // it sends to regions is numbered anew; to the regions that is a new start.
    let identity = EdgeIdentity::starting_now(&args.name);
    let mut links = Vec::new();
    // The routes of a complete table are the regions of the layout in their order.
    for route in &table.routes {
        let hello = RegionHello {
            region: route.region,
            epoch: route.epoch,
            layout,
        };
        let link = tcp::connect::<EdgeMessage, WorkerToEdge>(&route.address, hello, LINK_CAPACITY)
            .await
            .with_context(|| {
                format!("connecting to region {} at {}", route.region, route.address)
            })?;
        links.push(RegionLink {
            epoch: route.epoch,
            end: link,
        });
    }
    let routing = Routing {
        layout: table.layout.clone(),
        spawn: table.spawn,
        identity,
        links,
    };
    let edge = Edge::bind(args.bind, args.edge.clone(), routing)
        .await
        .with_context(|| format!("listening on {}", args.bind))?;
    info!(address = %edge.local_addr()?, regions = table.routes.len(), "listening");

    // Dropping the edge's future, which happens when the other branch wins, closes
    // every connection.
    Ok(tokio::select! {
        _ = edge.run() => "a region is gone",
        _ = routes_change(&args.coordinator, watch, &table) => "a region has another worker now",
    })
}

/// Resolves when the coordinator reports other routes than those of `table`. A
/// coordinator that goes away is waited for: the regions are where they were.
async fn routes_change(coordinator: &str, mut watch: RoutingWatch, table: &RoutingTable) {
    loop {
        match watch.next().await {
            Ok(next) if next.routes == table.routes && next.layout == table.layout => {}
            Ok(_) => return,
            Err(error) => {
                warn!(%error, "lost the coordinator; carrying on with the regions as they are");
                watch = loop {
                    sleep(RETRY).await;
                    if let Ok(watch) = RoutingWatch::connect(coordinator).await {
                        break watch;
                    }
                };
            }
        }
    }
}
