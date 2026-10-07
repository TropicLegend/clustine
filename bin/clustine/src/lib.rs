//! Single-binary mode: runs every Clustine service in one process.
//!
//! The services are wired together the same way they will be across processes: the edge
//! and the worker only share a link, and the worker reaches the world store through
//! messages.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clustine_data::items;
use clustine_edge::Edge;
pub use clustine_edge::EdgeConfig;
use clustine_rpc::link;
use clustine_sim::api::{HOTBAR_SLOTS, ItemStack};
use clustine_sim::{Region, RegionConfig};
use clustine_worker::{RegionRunner, Worker};
use clustine_world::{EntityId, Vec3};
use clustine_worldgen::FlatGenerator;
use tokio::task::JoinHandle;

/// Messages that may wait in each direction between the edge and the worker. The worker
/// never waits for the edge, so this has to cover the chunks of many players joining at
/// the same moment.
const LINK_CAPACITY: usize = 16 * 1024;

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
    /// Serialise every message between the edge and the worker, as a deployment with
    /// separate processes does. Slower; meant for testing that boundary.
    pub serialise_link: bool,
}

/// A running server. Dropping it without calling [`Server::stop`] leaves it running
/// until the runtime shuts down.
pub struct Server {
    address: SocketAddr,
    edge: JoinHandle<()>,
    worker: Worker,
}

impl Server {
    /// Starts all services and returns once the server accepts connections.
    pub async fn start(config: Config) -> Result<Self> {
        let generator = FlatGenerator::classic();
        // Players enter above the middle of the block at the origin.
        let spawn = Vec3::new(0.5, f64::from(generator.surface_y()), 0.5);
        let generator = Arc::new(generator);
        let store = match &config.world {
            Some(directory) => clustine_worldstore::spawn_local(directory, generator)
                .with_context(|| format!("opening the world in {}", directory.display()))?,
            None => clustine_worldstore::spawn(generator),
        };

        let (edge_end, worker_end) = if config.serialise_link {
            link::framed(LINK_CAPACITY)
        } else {
            link::in_process(LINK_CAPACITY)
        };
        let region = Region::new(RegionConfig {
            spawn,
            // Clients reject entity id 0.
            first_entity_id: EntityId(1),
            starting_hotbar: STARTING_HOTBAR.map(|item| Some(ItemStack { item, count: 1 })),
        });

        let edge_config = EdgeConfig {
            description: config.description,
            max_players: config.max_players,
            keep_alive_interval: config.keep_alive_interval,
            view_distance: config.view_distance,
            client_timeout: config.client_timeout,
            compression_threshold: config.compression_threshold,
        };
        let edge = Edge::bind(config.bind, edge_config, edge_end)
            .await
            .with_context(|| format!("listening on {}", config.bind))?;
        let address = edge.local_addr()?;
        Ok(Self {
            address,
            edge: tokio::spawn(edge.run()),
            worker: Worker::spawn(
                RegionRunner::new(region, worker_end, store)
                    .with_checkpoint_interval(config.checkpoint_interval.as_millis() as u64 / 50),
            ),
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
        // Waits for the current tick and for the world to be stored.
        let worker = self.worker;
        let _ = tokio::task::spawn_blocking(move || worker.stop()).await;
    }
}
