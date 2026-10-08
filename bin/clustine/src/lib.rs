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
use clustine_edge::{Edge, EdgeIdentity, RegionLink, Routing};
use clustine_region::Layout;
use clustine_rpc::{RegionHello, link};
use clustine_sim::api::{HOTBAR_SLOTS, ItemStack};
use clustine_sim::{Region, RegionConfig};
use clustine_worker::{RegionRunner, Worker};
use clustine_world::{ChunkGenerator, EntityIds, Vec3};
use clustine_worldgen::FlatGenerator;
use clustine_worldstore::Store;
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
    edge: JoinHandle<()>,
    /// One per region.
    workers: Vec<Worker>,
}

impl Server {
    /// Starts all services and returns once the server accepts connections.
    pub async fn start(config: Config) -> Result<Self> {
        let spawn = spawn_point();
        let generator = generator();
        let layout = Layout::new(config.boundaries).context("dividing the world into regions")?;
        // One store for all regions, as in a cluster.
        let store = match &config.world {
            Some(directory) => Store::local(directory, Arc::clone(&generator))
                .with_context(|| format!("opening the world in {}", directory.display()))?,
            None => Store::memory(Arc::clone(&generator)),
        };
        let checkpoint_interval = config.checkpoint_interval.as_millis() as u64 / 50;

        let mut links = Vec::new();
        let mut runners = Vec::new();
        for (region, area) in layout.regions() {
            let hello = RegionHello {
                region,
                // Nobody else ever runs a region of this process's world.
                epoch: 1,
                layout: layout.fingerprint(),
            };
            // What the region is restored with is not used yet, but for its tick.
            let (store, restored) = store
                .open_region(hello)
                .with_context(|| format!("opening region {region}"))?;
            let (edge_end, worker_end) = if config.serialise_link {
                link::framed(LINK_CAPACITY)
            } else {
                link::in_process(LINK_CAPACITY)
            };
            let config = RegionConfig {
                spawn,
                area,
                starting_hotbar: starting_hotbar(),
            };
            let entity_ids = EntityIds::block(region.0).context("too many regions")?;
            let state = Region::new(config, entity_ids);
            links.push(RegionLink {
                // Nobody else ever runs a region of this process's world.
                epoch: 1,
                end: edge_end,
            });
            runners.push(
                RegionRunner::new(state, worker_end, store)
                    .with_checkpoint_interval(checkpoint_interval)
                    .continuing_from(restored.tick()),
            );
        }

        let edge_config = EdgeConfig {
            description: config.description,
            max_players: config.max_players,
            keep_alive_interval: config.keep_alive_interval,
            view_distance: config.view_distance,
            client_timeout: config.client_timeout,
            compression_threshold: config.compression_threshold,
        };
        let routing = Routing {
            layout,
            spawn,
            identity: EdgeIdentity::starting_now("edge"),
            links,
        };
        let edge = Edge::bind(config.bind, edge_config, routing)
            .await
            .with_context(|| format!("listening on {}", config.bind))?;
        let address = edge.local_addr()?;
        Ok(Self {
            address,
            edge: tokio::spawn(edge.run()),
            workers: runners.into_iter().map(Worker::spawn).collect(),
        })
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
        let _ =
            tokio::task::spawn_blocking(move || workers.into_iter().for_each(Worker::stop)).await;
    }
}
