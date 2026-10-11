//! Players who end a stay in the world and begin another find themselves as they were:
//! the ledger bots across a leave, a lost connection and a second login
//! (`docs/adr/0020-one-stay-per-player.md`, "What a player notices"; step R1.6 of its
//! section "Building it").
//!
//! What "as they were" is, the bots judge: `clustine_botswarm::Left` has the rule and
//! where the record says so. A test here says when the bots go and come back, waits
//! until they are back, and has them play on, so that a bot that came back as somebody
//! the server is confused about is found out by everything else the ledger holds a
//! server to.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use clustine::Config;
use clustine_botswarm::{Again, Ledger, LedgerReport, Progress, ledger};
use tokio::task::JoinHandle;

use common::processes::{Cluster, turn};
use common::{VIEW_DISTANCE, config, start_with};

const PATIENCE: Duration = Duration::from_secs(60);

/// How often a test looks whether the bots are where it waits for them to be.
const LOOK: Duration = Duration::from_millis(20);

/// Every this many client ticks each bot sends something that is acknowledged, so that
/// a test can wait for the server to have answered a bot that happens not to build.
const PULSE: u32 = 5;

/// The chunk x coordinate at which the divided worlds of these tests are divided: the
/// region in the west has the spawn point, the one in the east begins at block
/// x = 48.
const BOUNDARY: i32 = 3;

/// The first block east of the boundary.
const LINE: i32 = BOUNDARY * 16;

/// A ledger scenario that plays until it is told to stop.
struct Playing {
    progress: Arc<Progress>,
    playing: JoinHandle<anyhow::Result<LedgerReport>>,
    /// What the test did, for when the bots find fault.
    deeds: Vec<String>,
    started: Instant,
}

impl Playing {
    /// Lets the bots of `scenario` join the server at `address`.
    fn begin(address: &str, scenario: Ledger) -> Self {
        let progress = Progress::new(scenario.bots);
        let playing = {
            let (address, scenario, progress) =
                (address.to_owned(), scenario.clone(), progress.clone());
            tokio::spawn(async move { ledger(&address, &scenario, &progress).await })
        };
        Self {
            progress,
            playing,
            deeds: Vec::new(),
            started: Instant::now(),
        }
    }

    fn note(&mut self, deed: &str) {
        let deed = format!("{:7.3} s: {deed}", self.started.elapsed().as_secs_f64());
        println!("{deed}");
        self.deeds.push(deed);
    }

    /// Waits until `state` holds for the bots. If they end first, they have found
    /// fault, and that is the failure.
    async fn until(&mut self, what: &str, state: impl Fn(&Progress) -> bool) {
        let waiting = Instant::now();
        while !state(&self.progress) {
            if self.playing.is_finished() {
                let ended = (&mut self.playing).await.unwrap();
                panic!(
                    "the bots ended before {what}: {:#}\nwhat was done:\n{}",
                    ended
                        .err()
                        .unwrap_or_else(|| anyhow::anyhow!("with nothing wrong")),
                    self.deeds.join("\n")
                );
            }
            assert!(
                waiting.elapsed() <= PATIENCE,
                "{PATIENCE:?} passed before {what}; what was done:\n{}\nthe bots: {:?}",
                self.deeds.join("\n"),
                self.progress.bots()
            );
            tokio::time::sleep(LOOK).await;
        }
        self.note(what);
    }

    /// Waits until every bot is on its lane and the server has acknowledged a few
    /// things each sent on its present connection: they play, and are answered.
    async fn plays(&mut self) {
        self.until("every bot plays and is answered", |progress| {
            (progress.bots().iter()).all(|bot| bot.playing && bot.acknowledged >= 3)
        })
        .await;
    }

    /// Has every bot end its stay and begin another in the way `how`, waits until all
    /// are back, each having found itself as the server has to have it, and lets them
    /// play on.
    async fn everyone_joins_again(&mut self, how: Again) {
        let before: Vec<u32> = (self.progress.bots().iter()).map(|bot| bot.joins).collect();
        self.progress.all_join_again(how);
        self.until(&format!("every bot is back after {how:?}"), |progress| {
            progress.back()
        })
        .await;
        let after: Vec<u32> = (self.progress.bots().iter()).map(|bot| bot.joins).collect();
        for (before, after) in before.iter().zip(&after) {
            assert_eq!(*after, before + 1, "{before:?} to {after:?}");
        }
        self.plays().await;
    }

    /// Sends the bots to walk between the x coordinates `between` and waits until
    /// they are there.
    async fn walks(&mut self, between: (f64, f64)) {
        self.progress.walk_between(between.0, between.1).unwrap();
        let what = format!(
            "every bot walks between x = {} and x = {}",
            between.0, between.1
        );
        self.until(&what, |progress| progress.arrived()).await;
        self.plays().await;
    }

    /// Tells the bots to stop, and returns what they and their auditor found: nothing
    /// wrong, or the test fails.
    async fn ends(self) -> LedgerReport {
        self.progress.finish();
        match self.playing.await.unwrap() {
            Ok(report) => {
                println!("{report}");
                assert!(
                    report.actions > 0 && report.blocks_audited as usize == report.blocks.len(),
                    "the bots did nothing, or not every block was audited: {report}"
                );
                report
            }
            Err(error) => panic!(
                "the bots found fault: {error:#}\nwhat was done:\n{}",
                self.deeds.join("\n")
            ),
        }
    }
}

/// Bots that play on the east side of the spawn point until they are told to stop.
fn scenario() -> Ledger {
    Ledger {
        bots: 3,
        rounds: None,
        duration: None,
        west: 18.5,
        east: 29.5,
        pulse: Some(PULSE),
        // The edge is alive and every region runs in these tests, so what a
        // connection sent before it ended is applied within a few ticks or never.
        doubt: Duration::from_secs(3),
        ..Ledger::default()
    }
}

/// A server whose world is two regions, pinned side by side, which stay as they are.
fn divided() -> Config {
    Config {
        pins: vec![BOUNDARY],
        follow: None,
        // Everything between the edge and the regions goes through the codec, as it
        // does between processes.
        serialise_link: true,
        ..config()
    }
}

/// Bots that leave in good order and join again find themselves exactly as they left:
/// in place, with what they hold. So do bots that close their connections in the
/// middle of playing, as far as the server had got with what they sent, and what they
/// had sent and not seen acknowledged is done or not done and nothing else.
#[tokio::test]
async fn ledger_bots_who_leave_and_join_again_find_themselves_as_they_were() {
    let (server, address) = start_with(config()).await;
    let mut bots = Playing::begin(&address, scenario());
    bots.plays().await;
    for how in [Again::Leave, Again::Vanish, Again::Leave, Again::Vanish] {
        bots.everyone_joins_again(how).await;
    }
    let report = bots.ends().await;
    assert_eq!(report.joins_again, 12, "{report}");
    // Everyone who played is in what the scenario hands on, and the auditor.
    assert_eq!(report.left.len(), 4, "{:?}", report.left.keys());
    server.stop().await;
}

/// A bot that logs in a second time while it is connected, and walks on as the first
/// connection meanwhile: the first connection is ended with the game's sentence for
/// it, the second stands somewhere the first stood since the login began, and nobody
/// who watches is ever shown two entities of the player.
#[tokio::test]
async fn ledger_bots_who_log_in_again_while_connected_take_the_place_of_their_first_connection() {
    let (server, address) = start_with(config()).await;
    let mut bots = Playing::begin(&address, scenario());
    bots.plays().await;
    for _ in 0..3 {
        bots.everyone_joins_again(Again::SecondLogin).await;
    }
    // And a leave after it: the stay the second login began is one like any other.
    bots.everyone_joins_again(Again::Leave).await;
    let report = bots.ends().await;
    assert_eq!((report.joins_again, report.in_doubt), (12, 0), "{report}");
    server.stop().await;
}

/// In a world of two regions: bots that walk back and forth across the boundary end
/// their stays wherever they happen to be, some of them in the middle of being handed
/// over; then all of them play in the region that does not have the spawn point, end
/// their stays there in every way and find themselves there; and at last in the home
/// region again.
#[tokio::test]
async fn ledger_bots_who_leave_in_another_region_join_again_there() {
    let (server, address) = start_with(divided()).await;
    let mut bots = Playing::begin(
        &address,
        Ledger {
            bots: 4,
            // Across the boundary, a few blocks to either side.
            west: f64::from(LINE) - 4.5,
            east: f64::from(LINE) + 4.5,
            lines: vec![LINE],
            ..scenario()
        },
    );
    bots.plays().await;
    let ways = [Again::Leave, Again::SecondLogin, Again::Vanish];
    for how in ways {
        // Somebody has just stepped across, so stays end on both sides and on the way.
        let mut crossings = bots.progress.crossings();
        let before = crossings
            .borrow_and_update()
            .map_or(0, |crossing| crossing.count);
        bots.until("a bot steps across the boundary", |_| {
            (crossings.borrow().map_or(0, |crossing| crossing.count)) > before
        })
        .await;
        bots.everyone_joins_again(how).await;
    }

    // All of them in the east, where the home region has nobody and no chunk.
    let east = (f64::from(LINE) + 6.5, f64::from(LINE) + 17.5);
    bots.walks(east).await;
    for how in ways {
        bots.everyone_joins_again(how).await;
        let places: Vec<f64> = bots.progress.bots().iter().map(|bot| bot.x).collect();
        assert!(
            places.iter().all(|x| east.0 <= *x && *x <= east.1),
            "after {how:?} the bots are at x = {places:?}"
        );
    }

    // And home again, where a join is placed by the home region itself.
    bots.walks((18.5, 29.5)).await;
    bots.everyone_joins_again(Again::Leave).await;
    let report = bots.ends().await;
    assert_eq!(report.joins_again, 28, "{report}");
    assert!(report.crossings > 0 && report.across_a_line > 0, "{report}");
    server.stop().await;
}

/// Bots that played, left with the end of their scenario and join again after the
/// server was stopped and started on the same world enter where they left, with what
/// they held, and so does their auditor. A ledger that takes them for newcomers finds
/// fault with that, which shows that it looks.
#[tokio::test]
async fn ledger_bots_find_themselves_where_they_left_after_the_server_was_stopped_and_started() {
    let directory = tempfile::tempdir().unwrap();
    let on_disk = || Config {
        world: Some(directory.path().join("world")),
        ..divided()
    };
    // Up and down across the boundary once, and from one end to the other the second
    // time, so that the bots end the first scenario in the home region and the second
    // in the other.
    let first = Ledger {
        rounds: Some(1),
        west: f64::from(LINE) - 6.5,
        east: f64::from(LINE) + 6.5,
        lines: vec![LINE],
        ..scenario()
    };

    let (server, address) = start_with(on_disk()).await;
    let progress = Progress::new(first.bots);
    let before = ledger(&address, &first, &progress).await.unwrap();
    println!("{before}");
    server.stop().await;

    // Everyone left at the west end of their lane, and none of them at the spawn
    // point; the auditor in the middle.
    for (name, left) in &before.left {
        assert!(left.since.is_empty(), "{name} left with {left:?}");
        assert!(
            left.sure.location.0 > 30.0,
            "{name} left at {:?}",
            left.sure
        );
    }

    let (server, address) = start_with(on_disk()).await;
    let returning = Ledger {
        earlier: Some(before.left.clone()),
        // To the east end and no further: they stay in the other region.
        rounds: None,
        seed: 2,
        ..first.clone()
    };
    let mut bots = Playing::begin(&address, returning);
    bots.plays().await;
    bots.progress
        .walk_between(f64::from(LINE) + 6.5, f64::from(LINE) + 17.5)
        .unwrap();
    bots.until("every bot is east of the boundary", |progress| {
        progress.arrived()
    })
    .await;
    let again = bots.ends().await;
    server.stop().await;
    for (name, left) in &again.left {
        if name != "Auditor" {
            assert!(
                left.sure.location.0 > f64::from(LINE),
                "{name} left at {:?}",
                left.sure
            );
        }
    }

    // After another stop and start, from the other region. And then the ledger that
    // believes nobody was here before, which has to object to the first bot it sees.
    let (server, address) = start_with(on_disk()).await;
    let mistaken = Ledger {
        earlier: Some(Default::default()),
        rounds: Some(1),
        ..first.clone()
    };
    let progress = Progress::new(mistaken.bots);
    let fault = ledger(&address, &mistaken, &progress)
        .await
        .expect_err("bots that were in the world before are not at the spawn point");
    let fault = format!("{fault:#}");
    assert!(
        fault.contains("someone who was never in the world") && fault.contains("could not join"),
        "{fault}"
    );
    server.stop().await;

    let (server, address) = start_with(on_disk()).await;
    let third = Ledger {
        earlier: Some(again.left.clone()),
        rounds: Some(1),
        seed: 3,
        ..first.clone()
    };
    let progress = Progress::new(third.bots);
    let after = ledger(&address, &third, &progress).await.unwrap();
    println!("{after}");
    server.stop().await;
}

/// Bots whose edge is killed under them lose their connections without a word. When
/// the edge is back they join again, and each finds itself in a state it was in since
/// the last the server had acknowledged: the place the store last wrote, which can be
/// behind what the bot last sent and is never before what it was answered. What they
/// had sent and not seen acknowledged is done or not done.
#[tokio::test]
async fn ledger_bots_whose_edge_is_killed_join_again_in_a_state_they_were_in() {
    let _turn = turn().await;
    let directory = tempfile::tempdir().unwrap();
    let mut cluster = Cluster::new(directory.path(), 2, &BOUNDARY.to_string()).await;
    cluster.start().await;
    let address = cluster.edge.0.clone();
    let mut bots = Playing::begin(
        &address,
        Ledger {
            bots: 4,
            west: f64::from(LINE) - 4.5,
            east: f64::from(LINE) + 4.5,
            lines: vec![LINE],
            may_lose_connection: true,
            // What the dead edge had passed on is applied within ticks; what it had
            // not is gone with it.
            doubt: Duration::from_secs(3),
            ..scenario()
        },
    );
    bots.plays().await;

    for round in 1..=2 {
        let mut edge = cluster.edge.1.take().expect("the edge runs");
        edge.kill().await.unwrap();
        bots.note("the edge is killed");
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
        bots.note("the edge is started again");
        bots.until("every bot has joined again", |progress| {
            (progress.bots().iter()).all(|bot| bot.joins == round + 1)
        })
        .await;
        bots.plays().await;
    }

    let report = bots.ends().await;
    assert_eq!(report.joins_again, 8, "{report}");
    cluster.kill().await;
}
