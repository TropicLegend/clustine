//! The Clustine server. Without a subcommand every service runs in this one process;
//! with one, the process is that service of a cluster.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::error::ErrorKind;
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clustine::cluster::{
    self, COORDINATOR_PORT, CoordinatorArgs, EdgeArgs, MergeArgs, MoveArgs, SplitArgs, WORKER_PORT,
    WORLDSTORE_PORT, WorkerArgs,
};
use clustine::{Config, EdgeConfig, Server, stop_signal};
use clustine_coordinator::Policy;
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

/// Who decides when regions merge and split.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Reshape {
    /// Somebody who asks for it, with `clustine merge` and `clustine split`.
    ByHand,
    /// The coordinator as well, by where the players are.
    ByItself,
}

/// When a coordinator merges and splits regions without being asked; see
/// `docs/adr/0016-when-to-merge-and-split.md`, section 8.
#[derive(Args)]
struct Reshaping {
    /// Who decides when regions merge and split. A coordinator that does it by itself
    /// merges regions whose players come near each other and splits a region whose
    /// players go apart, and does what it is asked besides. The distances and the
    /// rest below are for that one; one that reshapes by hand checks them and does
    /// nothing with them.
    #[arg(long, value_enum, default_value_t = Reshape::ByHand)]
    reshape: Reshape,

    /// Regions with players this many chunks apart or nearer are merged by a
    /// coordinator that reshapes by itself. Unless told, twice the view distance and
    /// 6. It has to be 1 at least.
    #[arg(long)]
    merge_distance: Option<u32>,

    /// Players of one region that are more than this many chunks apart are split by
    /// a coordinator that reshapes by itself. Unless told, twice the view distance
    /// and 14. It has to be 2 more than the merge distance at least.
    #[arg(long)]
    split_distance: Option<u32>,

    /// Seconds a coordinator that reshapes by itself leaves a region alone after a
    /// merge, a split or a change of owner. A region without players is absorbed
    /// after three times as long. A day at most, so that the times the coordinator
    /// reckons from it (up to twenty-four times as long) are times it can add up.
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=86_400))]
    rest_seconds: u64,
}

impl Reshaping {
    /// What the coordinator goes by to merge and split regions by itself, or `None`
    /// for one that leaves it to whoever asks. An error says why the distances do not
    /// fit each other.
    ///
    /// The distances are checked whoever decides: a command line that is wrong is
    /// refused when it is written, not on the day somebody changes --reshape. But for
    /// that, a coordinator that reshapes by hand does nothing with the numbers.
    ///
    /// `view_distance` is the largest the edges grant, which the two distances follow
    /// from unless they are told.
    fn follow(&self, view_distance: u32) -> Result<Option<Policy>, String> {
        let mut policy = Policy::for_view_distance(view_distance);
        if let Some(distance) = self.merge_distance {
            policy.merge_distance = distance;
        }
        if let Some(distance) = self.split_distance {
            policy.split_distance = distance;
        }
        policy.rest = Duration::from_secs(self.rest_seconds);
        let policy = policy.checked()?;
        Ok(match self.reshape {
            Reshape::ByHand => None,
            Reshape::ByItself => Some(policy),
        })
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

    /// Chunk x coordinates at which regions are pinned side by side, in ascending
    /// order and separated by commas: each is simulated separately, and players are
    /// handed from one to the next as they walk. Without this the world is one home
    /// region, which holds what its players see.
    #[arg(long, value_delimiter = ',', allow_hyphen_values = true)]
    pin: Vec<i32>,

    /// Another name for --pin, from when a world was divided into stripes.
    #[arg(
        long,
        value_delimiter = ',',
        allow_negative_numbers = true,
        conflicts_with = "pin"
    )]
    boundaries: Vec<i32>,

    #[command(flatten)]
    reshaping: Reshaping,
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

        /// Largest view distance the edges grant, in chunks: what they are started with
        /// as --view-distance. Nothing checks that the two agree. A coordinator that
        /// reshapes by itself takes its two distances from it, so that a boundary
        /// between regions is in nobody's view as a rule, and uses it for nothing else.
        #[arg(long, default_value_t = EdgeConfig::DEFAULT_VIEW_DISTANCE as u32, value_parser = clap::value_parser!(u32).range(2..=32))]
        view_distance: u32,

        #[command(flatten)]
        reshaping: Reshaping,
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
        #[arg(
            long,
            value_delimiter = ',',
            allow_negative_numbers = true,
            conflicts_with = "pin"
        )]
        boundaries: Vec<i32>,

        /// Chunk x coordinates at which regions are pinned side by side, in ascending
        /// order and separated by commas: the regions --boundaries makes, without the
        /// store holding workers to how they say the world is divided. Without this
        /// and without --boundaries the world is one home region that is pinned to
        /// nothing, and every other chunk is whoever's asks for it first.
        #[arg(long, value_delimiter = ',', allow_hyphen_values = true)]
        pin: Vec<i32>,
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
        None => {
            // Refused as the parser refuses what it finds wrong itself.
            let refuse =
                |why: String| -> ! { Cli::command().error(ErrorKind::ValueValidation, why).exit() };
            let standalone = cli.standalone;
            let view_distance = standalone.players.view_distance as u32;
            let follow = match standalone.reshaping.follow(view_distance) {
                Ok(follow) => follow,
                Err(why) => refuse(why),
            };
            let pins = match pins(&standalone.pin, &standalone.boundaries) {
                Ok(pins) => pins,
                Err(why) => refuse(why),
            };
            run_standalone(standalone, pins, follow).await
        }
        Some(Service::Coordinator {
            listen,
            boundaries,
            lease_seconds,
            store,
            view_distance,
            reshaping,
        }) => {
            // Refused as the parser refuses what it finds wrong itself, with how the
            // coordinator is started below it.
            let follow = reshaping.follow(view_distance).unwrap_or_else(|why| {
                let mut command = Cli::command();
                command.build();
                let coordinator = command.find_subcommand_mut("coordinator");
                let coordinator = coordinator.expect("the coordinator is a subcommand");
                coordinator.error(ErrorKind::ValueValidation, why).exit()
            });
            cluster::coordinator(CoordinatorArgs {
                listen,
                boundaries,
                lease: Duration::from_secs(lease_seconds),
                store,
                follow,
            })
            .await
        }
        Some(Service::Worldstore {
            listen,
            world,
            boundaries,
            pin,
        }) => {
            let pins = pins(&pin, &[]).unwrap_or_else(|why| {
                let mut command = Cli::command();
                command.build();
                let store = command.find_subcommand_mut("worldstore");
                let store = store.expect("the world store is a subcommand");
                store.error(ErrorKind::ValueValidation, why).exit()
            });
            cluster::worldstore(listen, world, boundaries, pins).await
        }
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

/// The chunk x coordinates regions are pinned side by side at: those of --pin, or of
/// its other name. An error says what --pin takes, if they are not that.
fn pins(pin: &[i32], boundaries: &[i32]) -> Result<Vec<i32>, String> {
    let pins = if pin.is_empty() { boundaries } else { pin };
    if pins.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(format!("--pin takes {}", clustine_worldstore::NotAscending));
    }
    Ok(pins.to_vec())
}

/// Runs every service in this process until it is asked to stop.
async fn run_standalone(args: Standalone, pins: Vec<i32>, follow: Option<Policy>) -> Result<()> {
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
        pins,
        follow,
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

    /// What `clustine coordinator` with the arguments behind it has the coordinator
    /// go by to merge and split regions by itself, or what it complains of.
    fn follow(arguments: &[&str]) -> Result<Option<Policy>, String> {
        let words = ["clustine", "coordinator"];
        let words = words.iter().chain(arguments);
        let cli = Cli::try_parse_from(words).map_err(|error| error.to_string())?;
        match cli.service {
            Some(Service::Coordinator {
                reshaping,
                view_distance,
                ..
            }) => reshaping.follow(view_distance),
            _ => Err("not a coordinator".to_owned()),
        }
    }

    fn policy(merge_distance: u32, split_distance: u32, rest_seconds: u64) -> Policy {
        Policy {
            merge_distance,
            split_distance,
            rest: Duration::from_secs(rest_seconds),
        }
    }

    #[test]
    fn a_coordinator_reshapes_by_hand_unless_told_otherwise() {
        assert_eq!(follow(&[]), Ok(None));
        assert_eq!(follow(&["--reshape", "by-hand"]), Ok(None));
        // The other flags are taken and nothing is done with them.
        let numbers = [
            "--view-distance",
            "12",
            "--merge-distance",
            "3",
            "--split-distance",
            "5",
            "--rest-seconds",
            "5",
        ];
        assert_eq!(follow(&numbers), Ok(None));
        let complaint = follow(&["--reshape", "by-chance"]).expect_err("no such way");
        assert!(complaint.contains("by-hand, by-itself"), "{complaint}");
    }

    #[test]
    fn a_coordinator_that_reshapes_by_itself_goes_by_the_view_distance_unless_told_its_distances() {
        let by_itself = |arguments: &[&str]| {
            let words = ["--reshape", "by-itself"];
            let words: Vec<&str> = words.iter().chain(arguments).copied().collect();
            follow(&words)
        };
        // What the edges grant unless told otherwise, and a rest of ten seconds.
        assert_eq!(by_itself(&[]), Ok(Some(policy(22, 30, 10))));
        assert_eq!(by_itself(&[]), Ok(Some(Policy::for_view_distance(8))));
        // The least and the most an edge can be told to grant.
        assert_eq!(
            by_itself(&["--view-distance", "2"]),
            Ok(Some(policy(10, 18, 10)))
        );
        assert_eq!(
            by_itself(&["--view-distance", "32"]),
            Ok(Some(policy(70, 78, 10)))
        );
        // Each number that is given takes the place of what would follow, and the
        // others stay.
        assert_eq!(
            by_itself(&["--merge-distance", "3"]),
            Ok(Some(policy(3, 30, 10)))
        );
        assert_eq!(
            by_itself(&["--split-distance", "40"]),
            Ok(Some(policy(22, 40, 10)))
        );
        assert_eq!(
            by_itself(&["--rest-seconds", "1"]),
            Ok(Some(policy(22, 30, 1)))
        );
        assert_eq!(
            by_itself(&["--rest-seconds", "86400"]),
            Ok(Some(policy(22, 30, 86_400)))
        );
        assert_eq!(
            by_itself(&["--view-distance", "12", "--split-distance", "33"]),
            Ok(Some(policy(30, 33, 10)))
        );
        // As the tests of a cluster that reshapes by itself start their coordinator.
        let small = [
            "--merge-distance",
            "3",
            "--split-distance",
            "5",
            "--rest-seconds",
            "5",
        ];
        assert_eq!(by_itself(&small), Ok(Some(policy(3, 5, 5))));
        // The smallest distances there are.
        assert_eq!(
            by_itself(&["--merge-distance", "1", "--split-distance", "3"]),
            Ok(Some(policy(1, 3, 10)))
        );
    }

    #[test]
    fn a_view_distance_no_edge_grants_a_rest_under_a_second_and_what_is_no_number_are_refused() {
        for reshape in ["by-hand", "by-itself"] {
            for view_distance in ["0", "1", "33", "eight"] {
                let arguments = ["--reshape", reshape, "--view-distance", view_distance];
                let complaint = follow(&arguments).expect_err(view_distance);
                assert!(complaint.contains("--view-distance"), "{complaint}");
            }
            // A day is the longest rest there is.
            for rest in ["0", "0.5", "soon", "86401"] {
                let arguments = ["--reshape", reshape, "--rest-seconds", rest];
                let complaint = follow(&arguments).expect_err(rest);
                assert!(complaint.contains("--rest-seconds"), "{complaint}");
            }
            // A distance is a number of chunks.
            for distance in ["--merge-distance", "--split-distance"] {
                for chunks in ["2.5", "near"] {
                    let complaint = follow(&["--reshape", reshape, distance, chunks]);
                    let complaint = complaint.expect_err(chunks);
                    assert!(complaint.contains(distance), "{complaint}");
                }
            }
            // What begins with a dash is taken for another option, which there is
            // not.
            let numbers = [
                "--view-distance",
                "--rest-seconds",
                "--merge-distance",
                "--split-distance",
            ];
            for number in numbers {
                let refused = follow(&["--reshape", reshape, number, "-3"]);
                assert!(refused.is_err(), "{number} -3: {refused:?}");
            }
        }
    }

    #[test]
    fn distances_that_do_not_fit_each_other_are_refused_whoever_decides() {
        let none = "the merge distance has to be 1 at least";
        let near = |merge: u32, split: u32| {
            format!(
                "the split distance has to be at least 2 more than the merge distance, \
                 and {split} is not 2 more than {merge}"
            )
        };
        let refused: [(&[&str], String); 6] = [
            (&["--merge-distance", "0"], none.to_owned()),
            (
                &["--merge-distance", "5", "--split-distance", "6"],
                near(5, 6),
            ),
            (
                &["--merge-distance", "5", "--split-distance", "5"],
                near(5, 5),
            ),
            // Against what follows from the view distance, too.
            (&["--merge-distance", "29"], near(29, 30)),
            (&["--split-distance", "23"], near(22, 23)),
            (
                &["--view-distance", "2", "--split-distance", "11"],
                near(10, 11),
            ),
        ];
        for reshape in ["by-hand", "by-itself"] {
            for (distances, why) in &refused {
                let words = ["--reshape", reshape];
                let words: Vec<&str> = words.iter().chain(*distances).copied().collect();
                assert_eq!(follow(&words), Err(why.clone()), "{words:?}");
            }
        }
        // The nearest that fit.
        let fits = ["--reshape", "by-itself", "--merge-distance", "28"];
        assert_eq!(follow(&fits), Ok(Some(policy(28, 30, 10))));
    }

    #[test]
    fn the_help_of_the_coordinator_names_every_flag_of_how_it_reshapes() {
        let mut command = Cli::command();
        let coordinator = command
            .find_subcommand_mut("coordinator")
            .expect("the coordinator is a subcommand");
        let help = coordinator.render_long_help().to_string();
        for flag in [
            "--reshape <RESHAPE>",
            "by-hand",
            "by-itself",
            "--view-distance <VIEW_DISTANCE>",
            "--merge-distance <MERGE_DISTANCE>",
            "--split-distance <SPLIT_DISTANCE>",
            "--rest-seconds <REST_SECONDS>",
        ] {
            assert!(help.contains(flag), "{flag} is not in:\n{help}");
        }
    }
}
