//! What a worker process tells the coordinator of where its players are
//! (`docs/adr/0016-when-to-merge-and-split.md`, section 2.2). The coordinator is
//! played by the test, which is how it sees every word a worker says and in which
//! order; the world store, the worker and the edge are the processes they are in a
//! cluster.

mod common;

use std::time::Duration;

use clustine_botswarm::Bot;
use clustine_region::{RegionId, RegionRoute, RoutingTable};
use clustine_rpc::link::End;
use clustine_rpc::{Assignment, FromCoordinator, PlayersOf, ToCoordinator, tcp};
use clustine_world::{ChunkPos, EntityId, EntityIds, Vec3};
use tokio::net::TcpListener;
use tokio::time::timeout;

use common::VIEW_DISTANCE;
use common::processes::{Cluster, turn};

const PATIENCE: Duration = Duration::from_secs(30);

/// The chunk x coordinate at which the world is divided: players enter it in region
/// 0, which has what is west of block x = 64, and nobody is in region 1.
const BOUNDARY: i32 = 4;

/// Where every player enters the world.
const SPAWN: Vec3 = Vec3::new(0.5, -60.0, 0.5);

const ORIGIN: ChunkPos = ChunkPos::new(0, 0);

/// A chunk of region 0 that is not the one players enter in, and a block in it.
const APART: ChunkPos = ChunkPos::new(2, 0);
const IN_APART: (f64, f64) = (40.5, 0.5);

/// The coordinator's end of the connection of a worker or of an edge.
type Heard = End<FromCoordinator, ToCoordinator>;

/// What `waited` comes to, which it has to within [`PATIENCE`]; `what` says what it
/// is, for when it does not.
async fn within<T>(what: &str, waited: impl Future<Output = T>) -> T {
    match timeout(PATIENCE, waited).await {
        Ok(come) => come,
        Err(_) => panic!("waited in vain for {what}"),
    }
}

/// A coordinator as the test plays it: it listens where the processes look for one,
/// and the test decides what it answers.
struct Played {
    listener: TcpListener,
}

impl Played {
    /// The next connection and the first thing said on it.
    async fn connected(&self) -> (Heard, ToCoordinator) {
        let accepted = within("a connection", self.listener.accept()).await;
        let (stream, _) = accepted.expect("accepting a connection");
        let mut heard: Heard = tcp::link(stream, 256);
        let first = within("its first word", heard.recv()).await;
        (heard, first.expect("whoever connects says why"))
    }

    /// Lets the next worker register and tells it to run `assignments`. Returns its
    /// connection and what it said it holds, each region with its epoch.
    async fn registers(&self, assignments: &[Assignment]) -> (Heard, Vec<(u32, u64)>) {
        let (heard, first) = self.connected().await;
        let ToCoordinator::RegisterWorker { holding, .. } = first else {
            panic!("expected a worker to register, and heard {first:?}");
        };
        let orders = FromCoordinator::Assigned {
            spawn: SPAWN,
            assignments: assignments.to_vec(),
        };
        heard.send(orders).await.expect("the worker listens");
        let holding = holding.iter().map(|held| (held.region.0, held.epoch));
        (heard, holding.collect())
    }

    /// The routing table numbered `version`, in which the worker at `address` runs
    /// every one of `assignments`.
    fn table(&self, version: u64, address: &str, assignments: &[Assignment]) -> FromCoordinator {
        let routes = assignments.iter().map(|assignment| RegionRoute {
            region: assignment.region,
            epoch: assignment.epoch,
            address: address.to_owned(),
        });
        FromCoordinator::Routing(RoutingTable {
            version,
            spawn: SPAWN,
            routes: routes.collect(),
            home: Some(RegionId(0)),
            absorbed: Vec::new(),
            waiting: 0,
        })
    }
}

fn assignment(region: u32, epoch: u64) -> Assignment {
    Assignment {
        region: RegionId(region),
        epoch,
        // A worker goes by what the world store says of them.
        entity_ids: EntityIds {
            first: EntityId(0),
            end: EntityId(0),
        },
    }
}

/// What the worker says next besides that it is there.
async fn said(worker: &mut Heard) -> ToCoordinator {
    loop {
        match within("the worker's next word", worker.recv()).await {
            Some(ToCoordinator::Heartbeat { .. }) => {}
            Some(said) => return said,
            None => panic!("the worker has gone"),
        }
    }
}

/// The next time the worker says where its players are. Whatever else it says before
/// that is not expected here.
async fn report(worker: &mut Heard) -> Vec<PlayersOf> {
    match said(worker).await {
        ToCoordinator::Players { regions } => regions,
        other => panic!("expected where the players are, and heard {other:?}"),
    }
}

/// Reads what the worker says of its players until `wanted` holds of it, and returns
/// that report.
async fn report_in_which(
    worker: &mut Heard,
    what: &str,
    wanted: impl Fn(&[PlayersOf]) -> bool,
) -> Vec<PlayersOf> {
    let reading = async {
        loop {
            let regions = report(worker).await;
            if wanted(&regions) {
                return regions;
            }
        }
    };
    within(what, reading).await
}

/// The regions a report names, each with the epoch it is said to be run with.
fn named(regions: &[PlayersOf]) -> Vec<(u32, u64)> {
    let named = regions.iter().map(|of| (of.region.0, of.epoch));
    named.collect()
}

/// Whether a report names `region` and says that its players are in `crowds` and
/// nowhere else.
fn sees(regions: &[PlayersOf], region: u32, crowds: &[(ChunkPos, u32)]) -> bool {
    let mut of = regions.iter().filter(|of| of.region == RegionId(region));
    of.next().is_some_and(|of| of.crowds == crowds) && of.next().is_none()
}

/// A worker says at every look which regions it runs and where the players of each
/// are: none of a region it is still opening, every region with its epoch and a tick
/// once it runs, also one without players. What it says behind the word that a region
/// was split is of the regions as they are after the split, and a new connection to
/// the coordinator is told all of it again.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_says_where_the_players_of_the_regions_it_runs_are() {
    let _turn = turn().await;
    let directory = tempfile::tempdir().unwrap();
    // The address is one that nothing listened on a moment ago. Another test's
    // process can have taken it since, and then another address is tried.
    let (mut cluster, listener) = loop {
        let cluster = Cluster::new(directory.path(), 1, &BOUNDARY.to_string()).await;
        match TcpListener::bind(&cluster.coordinator.0).await {
            Ok(listener) => break (cluster, listener),
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {}
            Err(error) => panic!("listening as the coordinator: {error}"),
        }
    };
    // It says nothing of how the world is divided: the regions are the store's.
    let played = Played { listener };
    let both = [assignment(0, 3), assignment(1, 5)];

    // The worker is given both regions while there is no world store to open them at.
    cluster.start_worker(0);
    let (mut worker, holding) = played.registers(&both).await;
    assert!(holding.is_empty(), "{holding:?}");
    // It says where its players are all the same, and names no region: a region that
    // is being opened is not reported.
    let opening = report(&mut worker).await;
    assert!(opening.is_empty(), "{opening:?}");

    // With the store there it runs them. Each is named with the epoch it was given
    // out with, and neither has players.
    cluster.start_store();
    let runs_both = |regions: &[PlayersOf]| named(regions) == [(0, 3), (1, 5)];
    let first = report_in_which(&mut worker, "both regions to run", runs_both).await;
    for of in &first {
        assert!(of.crowds.is_empty(), "nobody has joined: {first:?}");
    }
    // The regions tick, and a later look says a later tick of each.
    let ticked = |regions: &[PlayersOf]| {
        runs_both(regions)
            && regions
                .iter()
                .zip(&first)
                .all(|(now, then)| now.tick > then.tick)
    };
    report_in_which(&mut worker, "both regions to have ticked", ticked).await;

    // An edge, which is told where the regions are, and a player. Players enter the
    // world in region 0.
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
    let (edge, first_word) = played.connected().await;
    assert_eq!(first_word, ToCoordinator::WatchRouting);
    let address = cluster.workers[0].0.clone();
    edge.send(played.table(1, &address, &both)).await.unwrap();
    let joining = async {
        loop {
            if let Ok(bot) = Bot::join(&cluster.edge.0, "Alice").await {
                return bot;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    let mut bot = within("a player to be let in", joining).await;
    let at_the_origin = |regions: &[PlayersOf]| sees(regions, 0, &[(ORIGIN, 1)]);
    let joined = report_in_which(&mut worker, "the player to be seen", at_the_origin).await;
    assert_eq!(named(&joined), [(0, 3), (1, 5)]);
    assert!(sees(&joined, 1, &[]), "{joined:?}");

    // The player walks to another chunk of the region, and is said to be there.
    bot.walk_to(IN_APART.0, IN_APART.1, 2.0).await.unwrap();
    let walked = |regions: &[PlayersOf]| sees(regions, 0, &[(APART, 1)]);
    report_in_which(&mut worker, "the player to be seen apart", walked).await;

    // The region is split there. Until the worker says what came of it, it names the
    // regions it had. A look that falls between the split and the loop's hearing of
    // it may find the player gone already; none finds them back afterwards.
    let split = FromCoordinator::SplitOff {
        region: RegionId(0),
        epoch: 3,
        chunks: vec![APART],
        as_epoch: 9,
        part: RegionId(2),
    };
    worker.send(split).await.unwrap();
    let ended = loop {
        match said(&mut worker).await {
            ToCoordinator::Players { regions } => {
                assert_eq!(named(&regions), [(0, 3), (1, 5)]);
                let there = sees(&regions, 0, &[(APART, 1)]);
                assert!(there || sees(&regions, 0, &[]), "{regions:?}");
            }
            ended => break ended,
        }
    };
    let split_off = ToCoordinator::SplitEnded {
        region: RegionId(0),
        as_epoch: 9,
        outcome: Ok(RegionId(2)),
    };
    assert_eq!(ended, split_off, "{}", cluster.all_logs());
    // The edge is told where the new region is, as a coordinator does.
    let all = [both[0], both[1], assignment(2, 9)];
    edge.send(played.table(2, &address, &all)).await.unwrap();

    // Every word behind that one is of after the split: the player is no longer in
    // region 0, and is in the new region as soon as that is named, which it is once
    // the store has answered for it.
    let gone = loop {
        let regions = report(&mut worker).await;
        assert!(sees(&regions, 0, &[]), "{regions:?}");
        if named(&regions) != [(0, 3), (1, 5)] {
            break regions;
        }
    };
    assert_eq!(named(&gone), [(0, 3), (1, 5), (2, 9)]);
    assert!(sees(&gone, 2, &[(APART, 1)]), "{gone:?}");
    assert!(sees(&gone, 1, &[]), "{gone:?}");

    // The coordinator goes away and is back, knowing nothing of the split. The worker
    // registers again with what it holds, says the split again before anything else,
    // and where its players are from then on as it did.
    drop(worker);
    let (mut worker, holding) = played.registers(&both).await;
    assert_eq!(holding, [(0, 3), (1, 5), (2, 9)]);
    assert_eq!(said(&mut worker).await, split_off);
    let again = report(&mut worker).await;
    assert_eq!(named(&again), [(0, 3), (1, 5), (2, 9)]);
    assert!(sees(&again, 0, &[]), "{again:?}");
    assert!(sees(&again, 2, &[(APART, 1)]), "{again:?}");
    for (now, then) in again.iter().zip(&gone) {
        assert!(now.tick > then.tick, "{again:?} after {gone:?}");
    }

    // Without a coordinator a worker that is told to stop does not wait for anybody
    // to take its regions.
    drop((worker, edge, played));
    drop(bot);
    cluster.terminate().await;
}
