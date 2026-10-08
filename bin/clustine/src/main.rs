//! The Clustine server. Without a subcommand every service runs in this one process;
//! with one, the process is that service of a cluster.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand};
use clustine::cluster::{
    self, COORDINATOR_PORT, CoordinatorArgs, EdgeArgs, MoveArgs, WORKER_PORT, WORLDSTORE_PORT,
    WorkerArgs,
};
use clustine::{Config, EdgeConfig, Server, stop_signal};
use clustine_region::RegionId;
use tracing::info;

/// A Minecraft: Java Edition server. Without a subcommand, all of it in one process.
#[derive(Parser)]
#[command(version, args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    service: Option<Service>,

    #[command(flatten)]
    standalone: Standalone,
}

/// What players are offered and granted.
#[derive(Args)]
struct Players {
    /// Text shown in the client's server list.
    #[arg(long, default_value = "A Clustine server")]
    description: String,

    /// Player limit shown in the server list.
    #[arg(long, default_value_t = 20)]
    max_players: u32,

    /// Packets of at least this many bytes are compressed. A negative number turns
    /// compression off.
    #[arg(long, default_value_t = EdgeConfig::DEFAULT_COMPRESSION_THRESHOLD as i64, allow_negative_numbers = true)]
    compression_threshold: i64,

    /// Largest view distance granted to a client, in chunks.
    #[arg(long, default_value_t = EdgeConfig::DEFAULT_VIEW_DISTANCE, value_parser = clap::value_parser!(i32).range(2..=32))]
    view_distance: i32,
}

impl Players {
    fn edge_config(self) -> EdgeConfig {
        EdgeConfig {
            description: self.description,
            max_players: self.max_players,
            keep_alive_interval: EdgeConfig::DEFAULT_KEEP_ALIVE_INTERVAL,
            view_distance: self.view_distance,
            client_timeout: EdgeConfig::DEFAULT_CLIENT_TIMEOUT,
            region_patience: EdgeConfig::DEFAULT_REGION_PATIENCE,
            compression_threshold: usize::try_from(self.compression_threshold).ok(),
        }
    }
}

/// Everything in one process.
#[derive(Args)]
struct Standalone {
    /// Address to listen on. There is no authentication yet, so keep this on localhost.
    #[arg(long, default_value = "127.0.0.1:25565")]
    bind: SocketAddr,

    #[command(flatten)]
    players: Players,

    /// Directory the world is kept in; created if it does not exist.
    #[arg(long, default_value = "world")]
    world: PathBuf,

    /// Seconds between two saves of all changed chunks that are still loaded.
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..))]
    checkpoint_interval: u64,

    /// Chunk x coordinates at which to divide the world into regions that are simulated
    /// separately, in ascending order and separated by commas. Without this the world
    /// is one region.
    #[arg(long, value_delimiter = ',', allow_negative_numbers = true)]
    boundaries: Vec<i32>,
}

/// One service of a cluster. The services reach each other without authentication, so
/// their ports must only be reachable from within the cluster.
#[derive(Subcommand)]
enum Service {
    /// Divides the world into regions and decides which worker runs which.
    Coordinator {
        /// Address workers and edges connect to.
        #[arg(long, default_value_t = SocketAddr::from(([127, 0, 0, 1], COORDINATOR_PORT)))]
        listen: SocketAddr,

        /// Chunk x coordinates at which to divide the world into regions, in ascending
        /// order and separated by commas. The workers that are there share the regions.
        #[arg(long, value_delimiter = ',', allow_negative_numbers = true)]
        boundaries: Vec<i32>,

        /// Seconds a worker may be silent before its region is given to another. Workers
        /// make themselves heard once a second, so this has to be several seconds. A
        /// coordinator also waits this long after it has started before it gives any
        /// region away, so that workers that are running can say what they run.
        #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u64).range(3..))]
        lease_seconds: u64,
    },
    /// Keeps the world on disk for the workers.
    Worldstore {
        /// Address workers connect to.
        #[arg(long, default_value_t = SocketAddr::from(([127, 0, 0, 1], WORLDSTORE_PORT)))]
        listen: SocketAddr,

        /// Directory the world is kept in; created if it does not exist.
        #[arg(long, default_value = "world")]
        world: PathBuf,

        /// Chunk x coordinates at which the world is divided into regions, in ascending
        /// order and separated by commas: the same as the coordinator is given. The
        /// store keeps which region holds which chunk, and refuses workers that divide
        /// the world otherwise. A world that was divided otherwise before is made over:
        /// its regions start anew, and what was built in it stays.
        #[arg(long, value_delimiter = ',', allow_negative_numbers = true)]
        boundaries: Vec<i32>,
    },
    /// Simulates the region the coordinator gives it.
    Worker {
        /// Host and port of the coordinator.
        #[arg(long, default_value_t = format!("127.0.0.1:{COORDINATOR_PORT}"))]
        coordinator: String,

        /// Host and port of the world store.
        #[arg(long, default_value_t = format!("127.0.0.1:{WORLDSTORE_PORT}"))]
        store: String,

        /// Address edges connect to.
        #[arg(long, default_value_t = SocketAddr::from(([127, 0, 0, 1], WORKER_PORT)))]
        listen: SocketAddr,

        /// Host and port under which edges reach this worker. By default what --listen
        /// says, which is only right if the edges run on the same machine.
        #[arg(long)]
        advertise: Option<String>,

        /// Name of this worker, by which the coordinator knows it again after a restart.
        #[arg(long)]
        name: String,

        /// Seconds between two saves of all changed chunks that are still loaded.
        #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..))]
        checkpoint_interval: u64,
    },
    /// Asks the coordinator to move a region to another worker while players stay in
    /// it.
    Move {
        /// Host and port of the coordinator.
        #[arg(long, default_value_t = format!("127.0.0.1:{COORDINATOR_PORT}"))]
        coordinator: String,

        /// The region to move: regions are numbered from 0, from west to east.
        #[arg(long)]
        region: u32,

        /// Name of the worker to move it to. Without this, the worker that runs the
        /// fewest regions.
        #[arg(long)]
        to: Option<String>,
    },
    /// Lets players in and shows them the world the workers simulate.
    Edge {
        /// Name of this edge, by which regions know it again after a restart.
        #[arg(long, default_value = "edge")]
        name: String,

        /// Host and port of the coordinator.
        #[arg(long, default_value_t = format!("127.0.0.1:{COORDINATOR_PORT}"))]
        coordinator: String,

        /// Address players connect to. There is no authentication yet, so do not make
        /// this reachable from the internet.
        #[arg(long, default_value = "127.0.0.1:25565")]
        bind: SocketAddr,

        #[command(flatten)]
        players: Players,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    // Logs go to standard error, which leaves standard output to what was asked for.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    match cli.service {
        None => standalone(cli.standalone).await,
        Some(Service::Coordinator {
            listen,
            boundaries,
            lease_seconds,
        }) => {
            cluster::coordinator(CoordinatorArgs {
                listen,
                boundaries,
                lease: Duration::from_secs(lease_seconds),
            })
            .await
        }
        Some(Service::Worldstore {
            listen,
            world,
            boundaries,
        }) => cluster::worldstore(listen, world, boundaries).await,
        Some(Service::Worker {
            coordinator,
            store,
            listen,
            advertise,
            name,
            checkpoint_interval,
        }) => {
            cluster::worker(WorkerArgs {
                coordinator,
                store,
                listen,
                advertise: advertise.unwrap_or_else(|| listen.to_string()),
                name,
                checkpoint_interval: Duration::from_secs(checkpoint_interval),
            })
            .await
        }
        Some(Service::Move {
            coordinator,
            region,
            to,
        }) => {
            cluster::move_region(MoveArgs {
                coordinator,
                region: RegionId(region),
                to,
            })
            .await
        }
        Some(Service::Edge {
            name,
            coordinator,
            bind,
            players,
        }) => {
            cluster::edge(EdgeArgs {
                name,
                coordinator,
                bind,
                edge: players.edge_config(),
            })
            .await
        }
    }
}

/// Runs every service in this process until it is asked to stop.
async fn standalone(args: Standalone) -> Result<()> {
    let edge = args.players.edge_config();
    let mut server = Server::start(Config {
        bind: args.bind,
        description: edge.description,
        max_players: edge.max_players,
        keep_alive_interval: edge.keep_alive_interval,
        view_distance: edge.view_distance,
        client_timeout: edge.client_timeout,
        region_patience: edge.region_patience,
        compression_threshold: edge.compression_threshold,
        world: Some(args.world),
        checkpoint_interval: Duration::from_secs(args.checkpoint_interval),
        serialise_link: false,
        boundaries: args.boundaries,
    })
    .await?;
    info!(address = %server.address(), "listening");

    tokio::select! {
        _ = stop_signal() => {}
        _ = server.stopped() => {
            server.stop().await;
            bail!("the server stopped unexpectedly");
        }
    }
    info!("shutting down");
    server.stop().await;
    Ok(())
}
