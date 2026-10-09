//! The Clustine server. Without a subcommand every service runs in this one process;
//! with one, the process is that service of a cluster.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand};
use clustine::cluster::{
    self, COORDINATOR_PORT, CoordinatorArgs, EdgeArgs, MergeArgs, MoveArgs, SplitArgs, WORKER_PORT,
    WORLDSTORE_PORT, WorkerArgs,
};
use clustine::{Config, EdgeConfig, Server, stop_signal};
use clustine_region::RegionId;
use clustine_world::ChunkPos;
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

        /// Host and port of the world store, whose list tells the coordinator which
        /// regions there are once regions have been merged and split. While the store
        /// cannot be reached, the coordinator goes by --boundaries and by what the
        /// workers report, and refuses to merge and to split.
        #[arg(long, default_value_t = format!("127.0.0.1:{WORLDSTORE_PORT}"))]
        store: String,
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
    /// Asks the coordinator to have one region absorb another, which is no more
    /// afterwards. The players of both are in the one that is left.
    Merge {
        /// Host and port of the coordinator.
        #[arg(long, default_value_t = format!("127.0.0.1:{COORDINATOR_PORT}"))]
        coordinator: String,

        /// The region that absorbs the other and goes on.
        #[arg(long)]
        survivor: u32,

        /// The region that is absorbed. The region players enter the world in never
        /// is.
        #[arg(long)]
        absorbed: u32,
    },
    /// Asks the coordinator to split the players standing in certain chunks off a
    /// region, as a new region that the same worker runs.
    Split {
        /// Host and port of the coordinator. It has to be named before --chunks, which
        /// takes everything behind it for a chunk.
        #[arg(long, default_value_t = format!("127.0.0.1:{COORDINATOR_PORT}"))]
        coordinator: String,

        /// The region to split.
        #[arg(long)]
        region: u32,

        /// The chunks whose players are split off, each as its coordinates x,z: a
        /// block's coordinates divided by 16 and rounded down. Whoever stands in one of
        /// them goes, and with them every chunk of the region that is nearer to them
        /// than to anyone who stays.
        #[arg(
            long,
            required = true,
            num_args = 1..,
            value_name = "X,Z",
            value_parser = chunk,
            allow_hyphen_values = true
        )]
        chunks: Vec<ChunkPos>,
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
            store,
        }) => {
            cluster::coordinator(CoordinatorArgs {
                listen,
                boundaries,
                lease: Duration::from_secs(lease_seconds),
                store,
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
        Some(Service::Merge {
            coordinator,
            survivor,
            absorbed,
        }) => {
            cluster::merge_regions(MergeArgs {
                coordinator,
                survivor: RegionId(survivor),
                absorbed: RegionId(absorbed),
            })
            .await
        }
        Some(Service::Split {
            coordinator,
            region,
            chunks,
        }) => {
            cluster::split_region(SplitArgs {
                coordinator,
                region: RegionId(region),
                chunks,
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

/// A chunk as `clustine split` is told it: its x and z coordinates with a comma between
/// them.
fn chunk(written: &str) -> Result<ChunkPos, String> {
    // A coordinate may be negative, so whatever follows --chunks is taken for a chunk,
    // another option too.
    if written.starts_with("--") {
        return Err(format!(
            "`{written}` was taken for a chunk; name every other option before --chunks"
        ));
    }
    let coordinate = |part: &str| part.trim().parse::<i32>().ok();
    let pair = written.split_once(',');
    pair.and_then(|(x, z)| Some(ChunkPos::new(coordinate(x)?, coordinate(z)?)))
        .ok_or_else(|| format!("`{written}` is not a chunk; write its coordinates as x,z"))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// What `clustine split --region 1` makes of the arguments behind it: the
    /// coordinator and the chunks, or what it complains of.
    fn split(arguments: &[&str]) -> Result<(String, Vec<ChunkPos>), String> {
        let words = ["clustine", "split", "--region", "1"];
        let words = words.iter().chain(arguments);
        let cli = Cli::try_parse_from(words).map_err(|error| error.to_string())?;
        match cli.service {
            Some(Service::Split {
                coordinator,
                chunks,
                ..
            }) => Ok((coordinator, chunks)),
            _ => Err("not a split".to_owned()),
        }
    }

    #[test]
    fn the_chunks_of_a_split_are_coordinates_which_may_be_negative() {
        let (coordinator, chunks) =
            split(&["--chunks", "3,4", "-3,4", "5,-6", "-7,-8"]).expect("these are chunks");
        assert_eq!(coordinator, format!("127.0.0.1:{COORDINATOR_PORT}"));
        let expected = [(3, 4), (-3, 4), (5, -6), (-7, -8)];
        assert_eq!(chunks, expected.map(|(x, z)| ChunkPos::new(x, z)));

        let (coordinator, chunks) =
            split(&["--coordinator", "there:1", "--chunks", "-1,-1"]).expect("this is a chunk");
        assert_eq!(coordinator, "there:1");
        assert_eq!(chunks, [ChunkPos::new(-1, -1)]);
    }

    #[test]
    fn a_split_names_at_least_one_chunk_and_nothing_else_as_one() {
        assert!(split(&[]).is_err());
        for not_a_chunk in ["3", "3,", ",4", "3,4,5", "a,b", "3.5,4"] {
            let complaint = split(&["--chunks", not_a_chunk]).expect_err(not_a_chunk);
            assert!(complaint.contains("is not a chunk"), "{complaint}");
        }
        // An option behind the chunks would be a chunk to the parser, and is said to
        // be in the wrong place rather than to be no chunk.
        let complaint = split(&["--chunks", "3,4", "--coordinator", "there:1"])
            .expect_err("the coordinator is named behind the chunks");
        assert!(complaint.contains("before --chunks"), "{complaint}");
    }
}
