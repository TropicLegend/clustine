//! The Clustine server: every service in one process, or one service per process.
//!
//! [`Server`] is the single process. [`cluster`] has the services as processes of their
//! own.
//!
//! The services are wired together the same way they are across processes: the edge
//! shares nothing with a region but a link, and a region reaches the world store through
//! messages. The world can be divided into several regions here too, each ticking on a
//! thread of its own.

pub mod cluster;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clustine_data::items;
pub use clustine_edge::EdgeConfig;
use clustine_edge::{Edge, EdgeIdentity, RegionLink, Relinks, Routing, Stopped};
use clustine_region::{Layout, RegionId};
use clustine_rpc::link::EdgeEnd;
use clustine_rpc::{RegionHello, Restored, link};
use clustine_sim::RegionConfig;
use clustine_sim::api::{HOTBAR_SLOTS, ItemStack};
use clustine_worker::{DEFAULT_RETURN_AFTER, RegionRunner, Worker};
use clustine_world::{ChunkArea, ChunkGenerator, ChunkPos, Vec3};
use clustine_worldgen::FlatGenerator;
use clustine_worldstore::{Division, Store, StoreError, StoreHandle};
use tokio::task::JoinHandle;

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

/// What `region` takes as given of who holds which chunk: the stripes of `layout`, each
/// with its region, and `None` for its own. A region runs on that for as long as its
/// runner does not ask the world store (`docs/adr/0012-the-tick-on-chunks.md`, section
/// 8).
pub(crate) fn presumed(layout: &Layout, region: RegionId) -> Vec<(ChunkArea, Option<RegionId>)> {
    layout
        .regions()
        .map(|(id, area)| (area, (id != region).then_some(id)))
        .collect()
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
    /// The chunk x coordinates at which the world is divided into regions, ascending.
    /// Each region is simulated on its own; players are handed from one to the next as
    /// they walk, and what they do to blocks on the other side of a boundary is passed
    /// on to the region that has them. Empty for a world that is one region.
    pub boundaries: Vec<i32>,
}

/// A running server. Dropping it without calling [`Server::stop`] leaves it running
/// until the runtime shuts down.
pub struct Server {
    address: SocketAddr,
    edge: JoinHandle<Stopped>,
    /// Gives the edge a new link to a region.
    relinks: Relinks,
    /// What it takes to run the regions, kept to run one of them anew.
    regions: Regions,
    /// The runner of each region and the epoch it runs it with, by region id.
    workers: Vec<(u64, Worker)>,
}

/// What every region of the world is run with.
struct Regions {
    store: Store,
    layout: Layout,
    spawn: Vec3,
    /// Ticks between two checkpoints.
    checkpoint_interval: u64,
    serialise_link: bool,
}

impl Regions {
    /// Opens `region` at the store as its owner with `epoch`, restores it and starts
    /// to run it. Returns the runner and the edge's end of a link to it.
    ///
    /// The region carries on with what the store has of it: for a world kept on disk
    /// where the server before this one left it, and after a takeover where the store
    /// had the previous runner.
    fn run(&self, region: RegionId, epoch: u64) -> Result<(Worker, RegionLink)> {
        self.has(region)?;
        let hello = RegionHello {
            region,
            epoch,
            layout: self.layout.fingerprint(),
        };
        let (store, restored) = self
            .store
            .open_region(hello)
            .with_context(|| format!("opening region {region}"))?;
        self.started(region, epoch, store, restored)
    }

    /// Fails if the layout has no such region. One made all the same would take the
    /// whole world to be its neighbours'.
    fn has(&self, region: RegionId) -> Result<()> {
        let area = self.layout.area(region);
        area.map(drop)
            .with_context(|| format!("the world has no region {region}"))
    }

    /// Opens `region` as its first owner in this process: with an epoch above every one
    /// the world has seen for it. A world on disk remembers the owners its regions have
    /// had, in this process's predecessors or in a cluster that served it before.
    /// Returns the epoch with the rest.
    fn run_first(&self, region: RegionId) -> Result<(u64, Worker, RegionLink)> {
        self.has(region)?;
        let mut epoch = 1;
        loop {
            let hello = RegionHello {
                region,
                epoch,
                layout: self.layout.fingerprint(),
            };
            match self.store.open_region(hello) {
                Ok((store, restored)) => {
                    let (worker, link) = self.started(region, epoch, store, restored)?;
                    return Ok((epoch, worker, link));
                }
                // Nobody else has the world open, so the next epoch is this process's.
                Err(StoreError::EpochRefused { seen, .. }) if seen >= epoch => {
                    epoch = seen
                        .checked_add(1)
                        .context("the region has run out of epochs")?;
                }
                Err(error) => {
                    return Err(error).with_context(|| format!("opening region {region}"));
                }
            }
        }
    }

    /// Restores `region` from what the store returned on opening it and starts to run
    /// it.
    fn started(
        &self,
        region: RegionId,
        epoch: u64,
        store: StoreHandle,
        restored: Restored,
    ) -> Result<(Worker, RegionLink)> {
        let (end, worker_end): (EdgeEnd, _) = if self.serialise_link {
            link::framed(LINK_CAPACITY)
        } else {
            link::in_process(LINK_CAPACITY)
        };
        let config = RegionConfig {
            spawn: self.spawn,
            starting_hotbar: starting_hotbar(),
            return_after: DEFAULT_RETURN_AFTER,
            presumed: presumed(&self.layout, region),
        };
        let runner = RegionRunner::restore(config, store, restored)
            .with_context(|| format!("restoring region {region}"))?
            .with_checkpoint_interval(self.checkpoint_interval);
        runner.links().attach(worker_end);
        let link = RegionLink { region, epoch, end };
        Ok((Worker::spawn(runner), link))
    }
}

impl Server {
    /// Starts all services and returns once the server accepts connections.
    pub async fn start(config: Config) -> Result<Self> {
        let spawn = spawn_point();
        let generator = generator();
        let layout = Layout::new(config.boundaries).context("dividing the world into regions")?;
        // One store for all regions, as in a cluster. It is told how the world is
        // divided when it starts, and its regions are those of the layout.
        let store = match &config.world {
            Some(directory) => {
                Store::local_divided(directory, Arc::clone(&generator), division(&layout))
                    .with_context(|| format!("opening the world in {}", directory.display()))?
            }
            None => Store::memory_divided(Arc::clone(&generator), division(&layout))
                .context("starting a world in memory")?,
        };
        let regions = Regions {
            store,
            layout: layout.clone(),
            spawn,
            checkpoint_interval: config.checkpoint_interval.as_millis() as u64 / 50,
            serialise_link: config.serialise_link,
        };

        let mut links = Vec::new();
        let mut workers = Vec::new();
        for (region, _) in layout.regions() {
            let (epoch, worker, link) = regions.run_first(region)?;
            links.push(link);
            workers.push((epoch, worker));
        }

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
        let (routing, relinks) = Routing::new(layout, spawn, identity, links);
        let edge = Edge::bind(config.bind, edge_config, routing)
            .await
            .with_context(|| format!("listening on {}", config.bind))?;
        let address = edge.local_addr()?;
        Ok(Self {
            address,
            edge: tokio::spawn(edge.run()),
            relinks,
            regions,
            workers,
        })
    }

    /// Has `region` taken over by a new runner, as when another worker is given a
    /// region whose owner is believed dead. The runner it had is not asked: the store
    /// takes the region from it, so that it can make nothing durable any more and stops
    /// without a word, and the new one carries on from what the store has. The edge is
    /// given a link to the new runner and resumes with it; nobody is disconnected.
    pub async fn take_over(&mut self, region: RegionId) -> Result<()> {
        let index = region.0 as usize;
        let epoch = match self.workers.get(index) {
            Some((epoch, _)) => epoch + 1,
            None => anyhow::bail!("the world has no region {region}"),
        };
        let (worker, link) = self.regions.run(region, epoch)?;
        let (_, replaced) = std::mem::replace(&mut self.workers[index], (epoch, worker));
        // It finds its store handle lost and ends by itself.
        tokio::task::spawn_blocking(move || replaced.stop()).await?;
        anyhow::ensure!(self.relinks.replace(link).await, "the edge is gone");
        Ok(())
    }

    /// The address the server listens on.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Waits until the server has stopped by itself, which only happens when one of its
    /// services fails.
    pub async fn stopped(&mut self) {
        let _ = (&mut self.edge).await;
    }

    /// Stops accepting connections, closes the existing ones, stops the simulation and
    /// stores what has changed in the world.
    pub async fn stop(self) {
        self.edge.abort();
        // The task was cancelled on purpose, so its result carries no information.
        let _ = self.edge.await;
        // Waits for the current ticks and for the world to be stored.
        let workers = self.workers;
        let stop = move || {
            for (_, worker) in workers {
                // A region that lost the store has stopped already and said so.
                worker.stop();
            }
        };
        let _ = tokio::task::spawn_blocking(stop).await;
    }
}
