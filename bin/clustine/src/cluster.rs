//! The services as processes of their own, which together are a cluster.
//!
//! A coordinator divides the world into regions and decides which worker runs which. A
//! worker registers with it, is given a region, opens that region at the world store
//! and runs it. An edge learns from the coordinator where each region's worker is,
//! connects to all of them and then lets players in.
//!
//! Nothing here is fault-tolerant yet. When a worker or the world store goes away, the
//! edge closes every player's connection and starts over once the world is whole again;
//! what players built is not lost, because it is in the world store's logs. A
//! coordinator that goes away is merely missed until it is back.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clustine_coordinator::{ClientError, CoordinatorConfig, Orders, RoutingWatch, WorkerClient};
use clustine_edge::{Edge, EdgeConfig, Routing};
use clustine_region::{Layout, RoutingTable};
use clustine_rpc::{Assignment, EdgeToWorker, RegionHello, WorkerToEdge, tcp};
use clustine_sim::{Region, RegionConfig};
use clustine_worker::{Links, RegionRunner, RegionStatus, Worker};
use clustine_worldstore::{Store, StoreError, StoreHandle};
use tokio::net::{TcpListener, TcpStream};
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

/// Runs a worker until the process is asked to stop: registers with the coordinator,
/// runs the region it is given and accepts the links of edges.
///
/// Fails if the region is given to another worker or the world store is lost. The
/// process is then meant to be started again, to register afresh.
pub async fn worker(args: WorkerArgs) -> Result<()> {
    let listener = TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("listening on {}", args.listen))?;

    let stop = crate::stop_signal();
    tokio::pin!(stop);
    let (coordinator, orders, assignment) = tokio::select! {
        assigned = be_assigned(&args) => assigned?,
        _ = &mut stop => return Ok(()),
    };
    let Assignment {
        region,
        epoch,
        entity_ids,
    } = assignment;
    let area = orders
        .layout
        .area(region)
        .context("the coordinator named a region that its layout does not have")?;
    let hello = RegionHello {
        region,
        epoch,
        layout: orders.layout.fingerprint(),
    };

    let store = tokio::select! {
        opened = open_region(&args.store, hello) => opened?,
        _ = &mut stop => return Ok(()),
    };
    let state = Region::new(RegionConfig {
        spawn: orders.spawn,
        area,
        entity_ids,
        starting_hotbar: starting_hotbar(),
    });
    let ticks = (args.checkpoint_interval.as_millis() / clustine_worker::TICK.as_millis()) as u64;
    let runner = RegionRunner::without_links(state, store).with_checkpoint_interval(ticks);
    let links = runner.links();
    let status = runner.status();
    let running = Worker::spawn(runner);
    info!(%region, epoch, address = %args.advertise, "running a region");

    let outcome = tokio::select! {
        _ = &mut stop => Ok(()),
        _ = accept_edges(&listener, hello, links) => Ok(()),
        lost = keep_assignment(coordinator, &args, assignment, hello.layout) => Err(lost),
        _ = store_lost(&status) => Err(anyhow!(
            "the world store is gone or has given region {region} to another worker"
        )),
    };
    info!("shutting down");
    // Waits for the current tick and for the world store to have what changed.
    tokio::task::spawn_blocking(move || running.stop()).await?;
    outcome
}

/// Opens the region at the world store, trying until the store can be reached.
async fn open_region(address: &str, hello: RegionHello) -> Result<StoreHandle> {
    loop {
        let store = address.to_owned();
        let opened =
            tokio::task::spawn_blocking(move || StoreHandle::connect(&store, hello)).await?;
        match opened {
            Ok(store) => return Ok(store),
            Err(StoreError::Io(error)) => {
                info!(%error, store = %address, "the world store cannot be reached yet");
                sleep(RETRY).await;
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "opening region {} at the world store {address}",
                        hello.region
                    )
                });
            }
        }
    }
}

/// Registers with the coordinator, trying until it can be reached, and waits to be
/// given a region.
async fn be_assigned(args: &WorkerArgs) -> Result<(WorkerClient, Orders, Assignment)> {
    let (mut coordinator, mut orders) = loop {
        let registered =
            WorkerClient::register(&args.coordinator, &args.name, &args.advertise, &[], None);
        match registered.await {
            Ok(registered) => break registered,
            Err(ClientError::Refused(reason)) => bail!("the coordinator refused: {reason}"),
            Err(error) => {
                info!(%error, coordinator = %args.coordinator, "the coordinator cannot be reached yet");
                sleep(RETRY).await;
            }
        }
    };
    loop {
        // A worker runs one region for now.
        if let Some(assignment) = orders.assignments.first() {
            return Ok((coordinator, orders.clone(), *assignment));
        }
        info!("registered; waiting to be given a region");
        orders = coordinator
            .next()
            .await
            .context("the coordinator went away before giving this worker a region")?;
    }
}

/// Stays registered for as long as the coordinator leaves this worker its region.
/// Returns why it no longer does.
///
/// A coordinator that goes away knows nothing when it is back, so the worker tells it
/// what it runs. The region keeps running meanwhile.
async fn keep_assignment(
    mut coordinator: WorkerClient,
    args: &WorkerArgs,
    assignment: Assignment,
    layout: u64,
) -> anyhow::Error {
    let taken = || {
        anyhow!(
            "the coordinator has given region {} to another worker",
            assignment.region
        )
    };
    loop {
        match coordinator.next().await {
            Ok(orders) if orders.assignments.contains(&assignment) => {}
            Ok(_) => return taken(),
            Err(error) => {
                warn!(%error, "lost the coordinator; carrying on and registering again");
                coordinator = loop {
                    sleep(RETRY).await;
                    let holding = [assignment];
                    let registered = WorkerClient::register(
                        &args.coordinator,
                        &args.name,
                        &args.advertise,
                        &holding,
                        Some(layout),
                    );
                    match registered.await {
                        Ok((coordinator, orders)) if orders.assignments.contains(&assignment) => {
                            info!("registered again");
                            break coordinator;
                        }
                        Ok(_) => return taken(),
                        Err(ClientError::Refused(reason)) => {
                            return anyhow!("the coordinator refused: {reason}");
                        }
                        Err(error) => debug!(%error, "the coordinator cannot be reached"),
                    }
                };
            }
        }
    }
}

/// Resolves once the region can no longer store anything.
async fn store_lost(status: &Arc<RegionStatus>) {
    while !status.store_lost.load(Ordering::Relaxed) {
        sleep(Duration::from_millis(200)).await;
    }
}

/// Accepts the connections of edges and attaches them to the region as links.
async fn accept_edges(listener: &TcpListener, region: RegionHello, links: Links) {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                warn!(%error, "accepting a connection failed");
                sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let links = links.clone();
        tokio::spawn(async move {
            match greet_edge(stream, region, links).await {
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

/// Makes a connection a link of the region, if it is about this region as it is now.
/// Returns false if the other side went away without saying what it wanted.
async fn greet_edge(stream: TcpStream, region: RegionHello, links: Links) -> Result<bool> {
    let incoming = match tcp::accept(stream).await {
        Ok(incoming) => incoming,
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(false),
        Err(error) => return Err(error).context("no greeting"),
    };
    let asked = incoming.hello();
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
        .welcome::<WorkerToEdge, EdgeToWorker>(LINK_CAPACITY)
        .await?;
    links.attach(link);
    Ok(true)
}

/// Settings of an edge process.
#[derive(Debug, Clone)]
pub struct EdgeArgs {
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
    let mut links = Vec::new();
    // The routes of a complete table are the regions of the layout in their order.
    for route in &table.routes {
        let hello = RegionHello {
            region: route.region,
            epoch: route.epoch,
            layout,
        };
        let link = tcp::connect::<EdgeToWorker, WorkerToEdge>(&route.address, hello, LINK_CAPACITY)
            .await
            .with_context(|| {
                format!("connecting to region {} at {}", route.region, route.address)
            })?;
        links.push(link);
    }
    let routing = Routing {
        layout: table.layout.clone(),
        spawn: table.spawn,
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
