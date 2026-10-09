//! When an edge's process lets players in, and which region it sends them to, tested
//! from the record alone: `docs/adr/0017-the-end-of-the-stripes.md`, sections 2.4 and
//! 5.4 and the scenario Q11 of its section 9.4. Whoever wrote these read the record
//! and the messages, and not the code of the edge's process.
//!
//! The edge is the process it is in a cluster. The coordinator is played by the test,
//! which is how it decides what every routing table says, and so are the workers: a
//! worker here is a listener that takes the edge's link, welcomes it and answers
//! nothing else, which is enough to see where the edge sends a player who joins.
//!
//! **How a test knows that the edge has read a table that it is not to act on.** An
//! edge says nothing to its coordinator after its first word. So the test sends the
//! table, ends the connection, and waits for the edge to come again: it reads what it
//! was sent in the order it was sent, so by then it has read the table.

mod common;

use std::time::Duration;

use clustine_botswarm::Bot;
use clustine_region::{RegionId, RegionRoute, RoutingTable};
use clustine_rpc::link::End;
use clustine_rpc::{
    EdgeMessage, EdgeToWorker, FromCoordinator, Presence, ToCoordinator, Welcome, WorkerToEdge, tcp,
};
use clustine_world::Vec3;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::timeout;

use common::VIEW_DISTANCE;
use common::processes::{Cluster, turn};

const PATIENCE: Duration = Duration::from_secs(30);

/// Where every player enters the world.
const SPAWN: Vec3 = Vec3::new(0.5, -60.0, 0.5);

/// What the edge logs when it begins to listen for players.
const LISTENING: &str = "listening";

/// What the edge logs when a table names another home region than the edge has
/// (section 5.4).
const ANOTHER_HOME: &str =
    "the world has another home region now; this edge has to be started anew";

/// The coordinator's end of an edge's connection.
type Watched = End<FromCoordinator, ToCoordinator>;

/// What `waited` comes to, which it has to within [`PATIENCE`]; `what` says what it
/// is, for when it does not.
async fn within<T>(what: &str, waited: impl Future<Output = T>) -> T {
    match timeout(PATIENCE, waited).await {
        Ok(come) => come,
        Err(_) => panic!("waited in vain for {what}"),
    }
}

/// The routing table numbered `version`, with the home region `home`, a route for
/// each of `routes` (the region, its epoch and where its worker is) and `waiting`
/// regions without an owner.
fn table(
    version: u64,
    home: Option<u32>,
    routes: &[(u32, u64, &str)],
    waiting: u32,
) -> FromCoordinator {
    let routes = routes.iter().map(|(region, epoch, address)| RegionRoute {
        region: RegionId(*region),
        epoch: *epoch,
        address: (*address).to_owned(),
    });
    FromCoordinator::Routing(RoutingTable {
        version,
        spawn: SPAWN,
        routes: routes.collect(),
        home: home.map(RegionId),
        absorbed: Vec::new(),
        waiting,
    })
}

/// An edge's process and the coordinator the test plays to it.
struct Stage {
    cluster: Cluster,
    coordinator: TcpListener,
    /// Where the edge is to listen for players.
    players: String,
}

impl Stage {
    /// Starts an edge that looks for its coordinator where the test listens.
    async fn new(directory: &std::path::Path) -> Self {
        // The address is one that nothing listened on a moment ago. Another test's
        // process can have taken it since, and then another address is tried.
        let (mut cluster, coordinator) = loop {
            let cluster = Cluster::new(directory, 0, "").await;
            match TcpListener::bind(&cluster.coordinator.0).await {
                Ok(coordinator) => break (cluster, coordinator),
                Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {}
                Err(error) => panic!("listening as the coordinator: {error}"),
            }
        };
        let edge = cluster.spawn(
            "edge",
            &[
                "edge",
                "--coordinator",
                &cluster.coordinator.0,
                "--bind",
                &cluster.edge.0,
                "--view-distance",
                &VIEW_DISTANCE.to_string(),
            ],
        );
        cluster.edge.1 = Some(edge);
        let players = cluster.edge.0.clone();
        Self {
            cluster,
            coordinator,
            players,
        }
    }

    /// The edge's next connection to the coordinator, on which it has asked for the
    /// routing table.
    async fn watched(&self) -> Watched {
        let accepted = within("the edge to connect", self.coordinator.accept()).await;
        let (stream, _) = accepted.expect("accepting a connection");
        let mut edge: Watched = tcp::link(stream, 256);
        let first = within("the edge's first word", edge.recv()).await;
        assert_eq!(first, Some(ToCoordinator::WatchRouting));
        edge
    }

    fn log(&self) -> String {
        self.cluster.log("edge")
    }

    /// Whether the edge takes a connection where players are to connect.
    async fn listens(&self) -> bool {
        TcpStream::connect(&self.players).await.is_ok()
    }

    /// Waits until the edge takes a player's connection and has said so in its log.
    async fn until_it_listens(&self) {
        let listening = async {
            while !self.listens().await {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        };
        within("the edge to listen for players", listening).await;
        self.cluster.wait_for_log("edge", LISTENING, 1).await;
    }
}

/// What the edge has said on its links to the workers the test plays, each with the
/// name of the worker it was said to.
struct Said {
    sender: mpsc::UnboundedSender<(&'static str, EdgeToWorker)>,
    heard: mpsc::UnboundedReceiver<(&'static str, EdgeToWorker)>,
}

impl Said {
    fn new() -> Self {
        let (sender, heard) = mpsc::unbounded_channel();
        Self { sender, heard }
    }

    /// A player called `name` connects to the edge and logs in. Returns the worker
    /// that the edge tells of their joining. No worker here lets anybody into the
    /// world, so the player gets no further, and leaves when this returns.
    async fn joins(&mut self, players: &str, name: &str) -> &'static str {
        let joining = Bot::join(players, name);
        tokio::pin!(joining);
        let told = async {
            loop {
                match self.heard.recv().await {
                    Some((worker, EdgeToWorker::PlayerJoin(join))) if join.name == name => {
                        return worker;
                    }
                    Some(_) => {}
                    None => unreachable!("the test holds a sender"),
                }
            }
        };
        let either = async {
            tokio::select! {
                worker = told => worker,
                joined = &mut joining => panic!(
                    "{name} was let in or turned away before any worker was told: {:?}",
                    joined.map(|_| ())
                ),
            }
        };
        within("a worker to be told of a player who joins", either).await
    }
}

/// A worker as the test plays it: where an edge links to it.
struct Worker {
    name: &'static str,
    listener: TcpListener,
    address: String,
}

impl Worker {
    async fn new(name: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        Self {
            name,
            listener,
            address,
        }
    }

    /// Takes the next link the edge makes to this worker and welcomes it as a region
    /// does that knows nothing of the edge. Returns the region and the epoch the edge
    /// said hello with. What the edge says on the link from then on goes to `said`,
    /// and the link stays for as long as the edge keeps it.
    async fn linked(&self, said: &Said) -> (u32, u64) {
        let accepted = within("the edge to link to a worker", self.listener.accept()).await;
        let (stream, _) = accepted.expect("accepting a link");
        let incoming = within("the edge's greeting", tcp::accept(stream)).await;
        let incoming = incoming.expect("the edge says which region it wants");
        let hello = incoming.hello();
        let welcomed = incoming.welcome::<WorkerToEdge, EdgeMessage>(256).await;
        let mut link = welcomed.expect("the edge listens");
        let first = within("the edge's hello", link.recv()).await;
        let Some(EdgeToWorker::Hello { players, .. }) = first.map(|message| message.body) else {
            panic!("the edge did not say hello on its link to {}", self.name);
        };
        let welcome = Welcome::Unknown {
            since: 1,
            entries: 0,
            presences: players.len() as u32,
            applied: 0,
        };
        link.send(WorkerToEdge::Welcome(welcome)).await.unwrap();
        for player in players {
            let answer = Presence::Absent;
            let absent = WorkerToEdge::Presence { player, answer };
            link.send(absent).await.unwrap();
        }
        let (name, sender) = (self.name, said.sender.clone());
        tokio::spawn(async move {
            while let Some(message) = link.recv().await {
                if sender.send((name, message.body)).is_err() {
                    break;
                }
            }
        });
        (hello.region.0, hello.epoch)
    }
}

// Q11: "it does not listen for players while the table has no home region, or no
// route for it, or a region waiting; it does when all three hold."
#[tokio::test(flavor = "multi_thread")]
async fn an_edge_listens_for_players_only_once_a_table_names_the_home_region_routes_it_and_has_no_region_waiting()
 {
    let _turn = turn().await;
    let directory = tempfile::tempdir().unwrap();
    let mut stage = Stage::new(directory.path()).await;
    // Workers that need not exist: nothing is asked of them here.
    let (a, b) = ("127.0.0.1:1", "127.0.0.1:2");
    let unfit = [
        (
            "of a coordinator that knows no region: no home region, no route, nothing waiting",
            table(1, None, &[], 0),
        ),
        (
            "that names the home region, which waits for a worker",
            table(2, Some(0), &[], 1),
        ),
        (
            "that names the home region and has no route for it, with nothing waiting",
            table(3, Some(0), &[], 0),
        ),
        (
            "that routes the home region while another region waits",
            table(4, Some(0), &[(0, 5, a)], 1),
        ),
        (
            "that routes another region than the home region, with nothing waiting",
            table(5, Some(0), &[(3, 5, b)], 0),
        ),
        (
            "that routes the regions its workers report and names no home region",
            table(6, None, &[(0, 5, a), (3, 5, b)], 0),
        ),
        (
            "that routes the home region, names none, and has a region waiting",
            table(7, None, &[(0, 5, a)], 1),
        ),
    ];
    let mut edge = stage.watched().await;
    for (what, table) in unfit {
        edge.send(table).await.unwrap();
        // The edge reads the table, finds the connection ended, and comes again.
        drop(edge);
        edge = stage.watched().await;
        assert!(
            !stage.listens().await,
            "the edge listens for players by a table {what}:\n{}",
            stage.log()
        );
        assert!(
            !stage.log().contains(LISTENING),
            "the edge says that it listens, by a table {what}:\n{}",
            stage.log()
        );
    }

    // All three hold: the home region is named and routed, and no region waits.
    let before = stage.log().len();
    let fit = table(8, Some(0), &[(0, 5, a), (3, 5, b)], 0);
    edge.send(fit).await.unwrap();
    stage.until_it_listens().await;
    let log = stage.log();
    assert_eq!(log.matches(LISTENING).count(), 1, "{log}");
    let said = log.find(LISTENING).expect("it was counted");
    assert!(
        said >= before,
        "it said so before the table was sent:\n{log}"
    );
    stage.cluster.kill().await;
}

// Q11: "a later table with `home: None` is taken for its routes; a join goes to the
// home region of the first table." And section 5.4: a later table with another home
// region is taken as well, the edge says once, as a warning, that it has to be
// started anew, and goes on with the home region it has.
#[tokio::test(flavor = "multi_thread")]
async fn a_join_goes_to_the_home_region_of_the_first_table_whatever_later_tables_say_of_home() {
    let _turn = turn().await;
    let directory = tempfile::tempdir().unwrap();
    let mut stage = Stage::new(directory.path()).await;
    let mut said = Said::new();
    let other = Worker::new("the worker of region 0").await;
    let first = Worker::new("the first worker of the home region").await;
    let second = Worker::new("the second worker of the home region").await;
    let another = Worker::new("the second worker of region 0").await;

    // The home region is region 2, and not the region that had the chunk players
    // enter in when a world was divided into stripes: only the table says so.
    let edge = stage.watched().await;
    let routes = [
        (0, 5, other.address.as_str()),
        (2, 7, first.address.as_str()),
    ];
    edge.send(table(1, Some(2), &routes, 0)).await.unwrap();
    // A hello to a worker is region and epoch.
    assert_eq!(other.linked(&said).await, (0, 5));
    assert_eq!(first.linked(&said).await, (2, 7));
    stage.until_it_listens().await;
    assert_eq!(said.joins(&stage.players, "Alice").await, first.name);

    // A coordinator that has started anew and not read the list yet names no home
    // region. Its routes are as good as any: the edge links to the home region's new
    // owner, and sends the next player there.
    let routes = [
        (0, 5, other.address.as_str()),
        (2, 8, second.address.as_str()),
    ];
    edge.send(table(2, None, &routes, 0)).await.unwrap();
    assert_eq!(second.linked(&said).await, (2, 8));
    assert_eq!(said.joins(&stage.players, "Bob").await, second.name);

    // A table that names another home region is taken for its routes as well, and so
    // is the one after it: the edge links to each new owner of region 0.
    let routes = [
        (0, 6, another.address.as_str()),
        (2, 8, second.address.as_str()),
    ];
    edge.send(table(3, Some(0), &routes, 0)).await.unwrap();
    assert_eq!(another.linked(&said).await, (0, 6));
    let routes = [
        (0, 9, other.address.as_str()),
        (2, 8, second.address.as_str()),
    ];
    edge.send(table(4, Some(0), &routes, 0)).await.unwrap();
    assert_eq!(other.linked(&said).await, (0, 9));
    // It says so once, as a warning, and goes on with the home region it has.
    let log = stage.log();
    let warnings: Vec<&str> = log
        .lines()
        .filter(|line| line.contains(ANOTHER_HOME))
        .collect();
    assert_eq!(warnings.len(), 1, "{log}");
    assert!(warnings[0].contains(" WARN "), "{log}");
    assert_eq!(said.joins(&stage.players, "Carol").await, second.name);

    // It listened all along, and said so once.
    assert_eq!(stage.log().matches(LISTENING).count(), 1);
    drop(edge);
    stage.cluster.kill().await;
}
