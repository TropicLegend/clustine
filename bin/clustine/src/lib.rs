//! The Clustine server: every service in one process, or one service per process.
//!
//! [`Server`] is the single process. [`cluster`] has the services as processes of their
//! own.
//!
//! The services are wired together the same way they are across processes: the edge
//! shares nothing with a region but a link, a region reaches the world store through
//! messages, and a coordinator says which regions run. The single process is those
//! services in one process, with channels where the processes have sockets.

pub mod cluster;

use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clustine_coordinator::{CoordinatorConfig, Policy, Reach, WorkerClient, serve_local};
use clustine_data::items;
pub use clustine_edge::EdgeConfig;
use clustine_edge::{Edge, EdgeIdentity, Routing};
use clustine_region::{Layout, RegionId};
use clustine_rpc::RegionList;
use clustine_sim::api::{HOTBAR_SLOTS, ItemStack};
use clustine_world::{ChunkGenerator, ChunkPos, Vec3};
use clustine_worldgen::FlatGenerator;
use clustine_worldstore::{Division, Store, StoreError};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tracing::warn;

use crate::cluster::edge::{Linking, keep_linked, whole_world};
use crate::cluster::worker::{self, Opener, Outside, Refusal, Serving, Setup, Stop};

/// Messages that may wait in each direction between the edge and a region. A region
/// never waits for the edge, so this has to cover the chunks of many players joining at
/// the same moment.
pub(crate) const LINK_CAPACITY: usize = 16 * 1024;

/// The blocks a player has at hand when entering the world. In creative mode they can
/// take any other item from the creative inventory.
const STARTING_HOTBAR: [i32; HOTBAR_SLOTS] = [
    items::STONE,
    items::COBBLESTONE,
    items::DIRT,
    items::GRASS_BLOCK,
    items::OAK_PLANKS,
    items::OAK_LOG,
    items::BRICKS,
    items::GLASS,
    items::GLOWSTONE,
];

/// What players have in their hotbar when they enter the world.
pub(crate) fn starting_hotbar() -> [Option<ItemStack>; HOTBAR_SLOTS] {
    STARTING_HOTBAR.map(|item| Some(ItemStack { item, count: 1 }))
}

/// What makes the chunks nobody has changed. Every service that needs it has to use the
/// same, or the world would not fit together.
pub(crate) fn generator() -> Arc<dyn ChunkGenerator> {
    Arc::new(FlatGenerator::classic())
}

/// Where players enter the world: above the middle of the block at the origin.
pub(crate) fn spawn_point() -> Vec3 {
    Vec3::new(0.5, f64::from(FlatGenerator::classic().surface_y()), 0.5)
}

/// How the world store is told the world is divided: the stripes of `layout` as its
/// pinned regions, with the chunk of [`spawn_point`] as the one players enter in.
pub(crate) fn division(layout: &Layout) -> Division {
    let spawn = spawn_point();
    Division::stripes(ChunkPos::containing(spawn.x, spawn.z), layout)
}

/// Resolves when the process is asked to stop: by an interrupt from the terminal or,
/// where there is such a thing, by the signal that asks a process to terminate, which
/// is what Kubernetes sends.
pub async fn stop_signal() {
    let interrupted = async {
        // If the signal cannot be listened for, only the other one stops the process.
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminated = async {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                terminate.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminated = std::future::pending::<()>();
    tokio::select! {
        _ = interrupted => {}
        _ = terminated => {}
    }
}

/// Settings of a single-process server.
#[derive(Debug, Clone)]
pub struct Config {
    /// The address players connect to. Port 0 picks a free port.
    pub bind: SocketAddr,
    /// The text shown below the server's name in the client's server list.
    pub description: String,
    /// The player limit shown in the server list.
    pub max_players: u32,
    /// How often clients have to prove they are still there; see
    /// [`EdgeConfig::DEFAULT_KEEP_ALIVE_INTERVAL`].
    pub keep_alive_interval: Duration,
    /// The largest view distance granted to a client, in chunks.
    pub view_distance: i32,
    /// How long a client may take to log in, to send what is waited for, or to take
    /// what is sent to it; see [`EdgeConfig::DEFAULT_CLIENT_TIMEOUT`].
    pub client_timeout: Duration,
    /// Packets of at least this many bytes are compressed. `None` turns compression off.
    pub compression_threshold: Option<usize>,
    /// How long a player is kept while the region they are in does not confirm what
    /// they do; see [`EdgeConfig::region_patience`].
    pub region_patience: Duration,
    /// The directory the world is kept in. It is created if it does not exist. With
    /// `None` the world only lasts as long as the server runs.
    pub world: Option<PathBuf>,
    /// How often every changed chunk that is still loaded is saved. In between, changes
    /// to such chunks are only in the write-ahead log.
    pub checkpoint_interval: Duration,
    /// Serialise every message between the edge and the regions, as a deployment with
    /// separate processes does. Slower; meant for testing that boundary.
    pub serialise_link: bool,
    /// The chunk x coordinates at which regions are pinned side by side, ascending:
    /// each is simulated on its own, players are handed from one to the next as they
    /// walk, and what they do to blocks on the other side of a boundary is passed on
    /// to the region that has them. Empty for a world that is one home region, which
    /// holds what its players see.
    pub pins: Vec<i32>,
    /// What the regions are merged and split by, or `None` for a server that does
    /// neither.
    pub follow: Option<Policy>,
}

/// The name of the one worker of a single process, and what stands for its address in
/// the routing table: the edge links to it in this process and connects to nothing.
const WORKER: &str = "local";
const HERE: &str = "in this process";

/// How long [`Server::take_over`] waits for a region that does not run yet.
const TAKE_OVER_PATIENCE: Duration = Duration::from_secs(10);

/// The world store of a single process, for as long as the server runs. Whoever uses
/// it holds the lock for as long as the call lasts, so that [`Server::stop`], which
/// takes the store out, has waited for every call that was under way and is the last
/// to speak to it.
type Kept = Arc<Mutex<Option<Store>>>;

/// Why the store of a server that is stopping does not answer.
fn stopping() -> io::Error {
    io::Error::other("the server is stopping")
}

/// A running server: the services of a cluster in one process, joined by channels
/// where the processes use TCP (`docs/adr/0017-the-end-of-the-stripes.md`, section 6).
/// A coordinator's service, the loop of one worker and an edge with its link-keeper
/// run as they do in processes of their own, so regions merge, split and are taken
/// over here by the very code that does it there.
///
/// Dropping it without calling [`Server::stop`] leaves it running until the runtime
/// shuts down.
pub struct Server {
    address: SocketAddr,
    /// The edge and what keeps it linked to the regions.
    edge: JoinHandle<()>,
    /// The loop of the one worker, with what it ended for.
    worker: JoinHandle<Result<()>>,
    coordinator: JoinHandle<()>,
    store: Kept,
    /// The regions the worker serves, each with the epoch it runs it with.
    serving: watch::Receiver<Serving>,
    /// Where the worker says what the store refused it; see [`Server::take_over`].
    refusals: mpsc::UnboundedSender<Refusal>,
    stop: mpsc::UnboundedSender<Stop>,
}

impl Server {
    /// Starts all services and returns once the server accepts connections. Every
    /// region of the world runs by then: a world that cannot be restored does not
    /// start, and the error says why.
    pub async fn start(config: Config) -> Result<Self> {
        let spawn = spawn_point();
        let home = ChunkPos::containing(spawn.x, spawn.z);
        let division = if config.pins.is_empty() {
            Division::open(home)
        } else {
            Division::side_by_side(home, &config.pins)
                .map_err(|error| anyhow!("the pins have to be {error}"))?
        };
        let store = match &config.world {
            Some(directory) => Store::local_divided(directory, generator(), division)
                .with_context(|| format!("opening the world in {}", directory.display()))?,
            None => Store::memory_divided(generator(), division)
                .context("starting a world in memory")?,
        };
        let store: Kept = Arc::new(Mutex::new(Some(store)));

        // The coordinator, which learns which regions there are from the store's list
        // and has nobody to wait for: its one worker is in this process.
        cluster::coordinator::say_how_it_reshapes(config.follow.as_ref());
        let listed = Arc::clone(&store);
        let lists = move || {
            let store = listed.lock().unwrap_or_else(PoisonError::into_inner);
            let store = store.as_ref().ok_or_else(stopping)?;
            store.regions().map_err(|error| match error {
                StoreError::Io(error) => error,
                other => io::Error::other(other),
            })
        };
        let coordinator_config = CoordinatorConfig {
            // Until nothing carries a layout any more. It has no boundary, and a
            // coordinator knows no region by one that has none.
            layout: Layout::single(),
            spawn,
            lease: CoordinatorConfig::DEFAULT_LEASE,
            follow: config.follow,
        };
        let (local, serving_coordinator) = serve_local(coordinator_config, lists);
        let coordinator = tokio::spawn(serving_coordinator);
        let reach = Reach::Local(local);

        // The one worker: the loop of a worker's process, which opens its regions at
        // the store of this process and shows what it serves in a watch.
        let opened = Arc::clone(&store);
        let open: Opener = Arc::new(move |hello| {
            let store = opened.lock().unwrap_or_else(PoisonError::into_inner);
            match store.as_ref() {
                Some(store) => store.open_region(hello),
                None => Err(StoreError::Io(stopping())),
            }
        });
        let registered = WorkerClient::register(&reach, WORKER, HERE, &[], None)
            .await
            .context("registering the worker with the coordinator of this process")?;
        let (serving_sender, serving) = watch::channel(Serving::default());
        let (refusals, refused) = mpsc::unbounded_channel();
        let (stop, stopped) = mpsc::unbounded_channel();
        let setup = Setup {
            name: WORKER.to_owned(),
            advertise: HERE.to_owned(),
            checkpoint_interval: (config.checkpoint_interval.as_millis()
                / clustine_worker::TICK.as_millis()) as u64,
        };
        let outside = Outside {
            registered,
            coordinator: reach.clone(),
            store: open,
            serving: serving_sender,
            refusals: (refusals.clone(), refused),
            stop: stopped,
        };
        let mut worker = tokio::spawn(worker::run(setup, outside));

        // The first routing table that names the home region with a worker and has no
        // region waiting, and then every region of the table running: the worker shows
        // each with the epoch of its route when it is restored and ticks. A loop that
        // ends before that could not restore a region, and says why.
        let running = async {
            let (mut tables, mut table, home) = whole_world(&reach).await;
            let mut shown = serving.clone();
            loop {
                let runs = |route: &clustine_region::RegionRoute| {
                    let shown = shown.borrow();
                    let served = shown.get(&route.region);
                    served.is_some_and(|(hello, _)| hello.epoch == route.epoch)
                };
                if table.routes.iter().all(runs) {
                    return Ok((tables, table, home));
                }
                tokio::select! {
                    changed = shown.changed() => {
                        if changed.is_err() {
                            bail!("the worker of this process has ended");
                        }
                    }
                    // The latest table is the one to go by, should a route change
                    // before its region runs.
                    next = tables.next() => {
                        table = next.context("the coordinator of this process is gone")?;
                    }
                }
            }
        };
        let (tables, table, home) = tokio::select! {
            running = running => running,
            ended = &mut worker => Err(match ended {
                Ok(Ok(())) => anyhow!("the worker of this process ended before its regions ran"),
                Ok(Err(error)) => error,
                Err(error) => anyhow!(error).context("the worker of this process failed"),
            }),
        }
        .inspect_err(|_| {
            coordinator.abort();
            worker.abort();
        })?;

        let edge_config = EdgeConfig {
            description: config.description,
            max_players: config.max_players,
            keep_alive_interval: config.keep_alive_interval,
            view_distance: config.view_distance,
            client_timeout: config.client_timeout,
            compression_threshold: config.compression_threshold,
            region_patience: config.region_patience,
        };
        let identity = EdgeIdentity::starting_now("edge");
        let (routing, relinks) = Routing::new(home, table.spawn, identity, Vec::new());
        let edge = Edge::bind(config.bind, edge_config, routing)
            .await
            .with_context(|| format!("listening on {}", config.bind))
            .inspect_err(|_| {
                coordinator.abort();
                worker.abort();
            })?;
        let address = edge.local_addr()?;
        let linking = Linking::Local {
            serving: serving.clone(),
            serialise: config.serialise_link,
        };
        // Either ends only if the edge is gone, which nothing here brings about.
        let edge = tokio::spawn(async move {
            tokio::select! {
                _ = edge.run() => {}
                () = keep_linked(&reach, linking, home, tables, table, relinks) => {}
            }
        });
        Ok(Self {
            address,
            edge,
            worker,
            coordinator,
            store,
            serving,
            refusals,
            stop,
        })
    }

    /// The world store's list of regions.
    pub fn regions(&self) -> Result<RegionList> {
        let store = self.store.lock().unwrap_or_else(PoisonError::into_inner);
        let store = store.as_ref().context("the server is stopping")?;
        Ok(store.regions()?)
    }

    /// Has `region` taken over by a new runner, as when another worker is given a
    /// region whose owner is believed dead. The runner it has is not asked: the region
    /// is opened with a higher epoch while that runner still runs, which takes the
    /// region from it at the store, so that it can make nothing durable any more and
    /// stops without a word; the new one carries on from what the store has. The edge
    /// links to the new runner and resumes with it; nobody is disconnected.
    ///
    /// It is done by saying, as the worker, that the store refused the region for a
    /// higher epoch, which the store did not say. From there on everything goes the
    /// way it goes in a cluster: the coordinator gives the region out anew, to the one
    /// worker there is, whose loop opens a region that is named with another epoch
    /// before it stops the runner it has.
    pub async fn take_over(&mut self, region: RegionId) -> Result<()> {
        let listed = self.regions()?;
        if !listed.regions.iter().any(|info| info.region == region) {
            bail!("the world has no region {region}");
        }
        // A region that is being opened runs in a moment; one that was absorbed or is
        // being released meanwhile never does.
        let epoch_of = |serving: &Serving| serving.get(&region).map(|(hello, _)| hello.epoch);
        let runs = self.serving.wait_for(|serving| epoch_of(serving).is_some());
        let had = match timeout(TAKE_OVER_PATIENCE, runs).await {
            Ok(Ok(serving)) => epoch_of(&serving).expect("it was just found to run"),
            Ok(Err(_)) => bail!("the worker of this process has ended"),
            Err(_) => bail!("region {region} does not run"),
        };
        let seen = had
            .checked_add(1)
            .context("the region has run out of epochs")?;
        let said = self.refusals.send((region, seen));
        said.ok().context("the worker of this process has ended")?;
        let taken = |serving: &Serving| epoch_of(serving).is_some_and(|epoch| epoch > had);
        let taken_over = self.serving.wait_for(taken).await;
        taken_over
            .map(|_| ())
            .ok()
            .context("the worker of this process has ended")
    }

    /// The address the server listens on.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Waits until the server has stopped by itself, which only happens when one of its
    /// services fails.
    pub async fn stopped(&mut self) {
        tokio::select! {
            _ = &mut self.edge => {}
            _ = &mut self.worker => {}
            _ = &mut self.coordinator => {}
        }
    }

    /// Stops accepting connections, closes the existing ones, stops the simulation and
    /// stores what has changed in the world. When it returns, nothing of the server
    /// holds the world store any more and nothing is being written: a server that is
    /// started on the same directory right away finds the world as the last confirmed
    /// tick left it.
    pub async fn stop(self) {
        // The edge first, which closes every client and every link.
        self.edge.abort();
        // The tasks are ended on purpose, so how they ended carries no information.
        let _ = self.edge.await;
        // The worker's loop stops every runner and waits for it: each has stored what
        // changed, or, in the middle of a merge or a split, let go as it was.
        let _ = self.stop.send(Stop::AtOnce);
        match self.worker.await {
            Ok(Ok(())) | Err(_) => {}
            Ok(Err(error)) => warn!(%error, "the worker of this process ended badly"),
        }
        self.coordinator.abort();
        let _ = self.coordinator.await;
        // Taking the store out waits for a hello or a reading of the list that was
        // under way, on whichever thread; the store is then waited for until it is at
        // rest with all that was asked of it, which a runner that let go in the middle
        // did not wait for.
        let store = self.store;
        let rest = move || {
            let store = store.lock().unwrap_or_else(PoisonError::into_inner).take();
            if let Some(store) = store
                && let Err(error) = store.flush()
            {
                warn!(%error, "the world store did not come to rest");
            }
        };
        let _ = tokio::task::spawn_blocking(rest).await;
    }
}
