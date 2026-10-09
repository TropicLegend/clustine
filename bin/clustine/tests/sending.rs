//! Players who keep a ledger of what they were told (the ledger scenario of the bots)
//! are sent elsewhere while they play, with `Progress::walk_between`. The tests of
//! regions that follow their players make groups of players meet and part with that,
//! so these tests are about the bots: that a group goes where it is sent and stays
//! there, that a test can wait for it to be there as a state, that several groups play
//! on one server at once, and that the ledgers hold through all of it: nobody is
//! disconnected, every action is acknowledged and has its effect, and an auditor who
//! joins afterwards finds every block as the ledgers say, wherever it was built.
//!
//! The server is the single process, on a world of one region and on worlds divided
//! where the bots walk across.
//!
//! What the bots choose follows from a seed, which every test prints. To run a seed
//! again, set `CLUSTINE_SENDING_SEED`.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clustine::Config;
use clustine_botswarm::{Ledger, LedgerReport, Progress, audit_blocks, ledger};
use clustine_data::blocks;
use tokio::task::JoinHandle;

use common::{config, start_with};

/// How long a group may take to get where it was sent, and to wind up at the end. The
/// slowest bot walks a chunk in under three seconds; this only ever runs out when
/// something hangs.
const PATIENCE: Duration = Duration::from_secs(60);

/// How often a state that is waited for is looked at.
const LOOK: Duration = Duration::from_millis(20);

/// Whether this run of the tests is the one that repeats the end-to-end tests on a
/// world divided into regions, which `CLUSTINE_TEST_BOUNDARIES` asks for. These tests
/// divide their worlds themselves, so they run once, in the run without it.
fn a_repetition() -> bool {
    std::env::var_os("CLUSTINE_TEST_BOUNDARIES").is_some()
}

/// The seed of this run: `CLUSTINE_SENDING_SEED` if set, else the clock.
fn seed() -> u64 {
    match std::env::var("CLUSTINE_SENDING_SEED") {
        Ok(seed) => seed.parse().expect("CLUSTINE_SENDING_SEED is a number"),
        Err(_) => {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
            now.as_nanos() as u64 % 1_000_000
        }
    }
}

/// The x coordinates a group walks up and down between when it stands in the chunk
/// with the x coordinate `chunk`: far enough from the chunk's edges for everything it
/// builds to be in that chunk.
fn within_chunk(chunk: i32) -> (f64, f64) {
    let west = f64::from(16 * chunk);
    (west + 2.5, west + 13.5)
}

/// Starts a server whose world is divided at the chunk x coordinates `boundaries`.
/// Returns it with its address and with the first block east of each boundary.
async fn server(boundaries: &[i32]) -> (clustine::Server, String, Vec<i32>) {
    let (server, address) = start_with(Config {
        boundaries: boundaries.to_vec(),
        // Everything between the edge and the regions goes through the codec, as it
        // does between processes.
        serialise_link: true,
        ..config()
    })
    .await;
    let lines = boundaries.iter().map(|chunk| chunk * 16).collect();
    (server, address, lines)
}

/// A group of players: one ledger scenario, which can be sent to a chunk.
struct Group {
    name: String,
    ledger: Ledger,
    progress: Arc<Progress>,
    playing: JoinHandle<anyhow::Result<LedgerReport>>,
}

impl Group {
    /// Lets `bots` bots join whose names begin with `name` and whose lanes begin at
    /// the block row `first_lane`, to stand in the chunk `chunk`.
    fn start(
        address: &str,
        name: &str,
        bots: usize,
        first_lane: i32,
        chunk: i32,
        lines: &[i32],
        seed: u64,
    ) -> Self {
        let (west, east) = within_chunk(chunk);
        let scenario = Ledger {
            bots,
            rounds: None,
            duration: None,
            west,
            east,
            lines: lines.to_vec(),
            seed,
            name_prefix: name.to_owned(),
            first_lane,
            ..Ledger::default()
        };
        let progress = Progress::new(bots);
        let playing = {
            let (address, scenario, progress) =
                (address.to_owned(), scenario.clone(), progress.clone());
            tokio::spawn(async move { ledger(&address, &scenario, &progress).await })
        };
        Self {
            name: name.to_owned(),
            ledger: scenario,
            progress,
            playing,
        }
    }

    /// Waits until `state` holds for the bots; if they end first, they have found
    /// fault.
    async fn until(&mut self, what: &str, state: impl Fn(&Progress) -> bool) {
        let waiting = Instant::now();
        while !state(&self.progress) {
            if self.playing.is_finished() {
                match (&mut self.playing).await.unwrap() {
                    Ok(report) => panic!(
                        "the bots of group {:?} ended before they were told to: {report}",
                        self.name
                    ),
                    Err(error) => panic!(
                        "the bots of group {:?} found fault before {what}: {error:#}",
                        self.name
                    ),
                }
            }
            assert!(
                waiting.elapsed() <= PATIENCE,
                "{PATIENCE:?} passed before {what}; the bots of group {:?}: {:?}",
                self.name,
                self.progress.bots()
            );
            tokio::time::sleep(LOOK).await;
        }
    }

    /// Sends the group to the chunk `chunk`.
    fn walk_to(&self, chunk: i32) {
        let (west, east) = within_chunk(chunk);
        self.progress.walk_between(west, east).unwrap();
    }

    /// Fails unless every bot of the group is in the chunk `chunk`, where the group
    /// was last sent, and said to have arrived.
    fn is_in(&self, chunk: i32) {
        let (west, east) = within_chunk(chunk);
        assert_eq!(self.progress.between(), Some((west, east)));
        let bots = self.progress.bots();
        let there = |x: f64| west <= x && x <= east;
        assert!(
            bots.iter()
                .all(|bot| bot.playing && bot.arrived && there(bot.x)),
            "group {:?} is to be between x = {west} and x = {east}: {bots:?}",
            self.name
        );
        assert!(self.progress.arrived());
    }

    /// Waits until every bot of the group is in the chunk `chunk`, where the group
    /// was last sent.
    async fn arrives_in(&mut self, chunk: i32) {
        let what = format!("every bot of the group was in chunk {chunk}");
        self.until(&what, Progress::arrived).await;
        self.is_in(chunk);
    }

    /// Waits until every bot of the group has had an action on a block acknowledged
    /// that it took from now on, which is where it is now if it has arrived.
    async fn plays(&mut self) {
        let sent: Vec<i32> = self.progress.bots().iter().map(|bot| bot.sent).collect();
        self.until("every bot had a new action acknowledged", |progress| {
            let bots = progress.bots();
            bots.iter()
                .zip(&sent)
                .all(|(bot, sent)| bot.acknowledged > *sent)
        })
        .await;
    }

    /// Tells the bots to stop, and fails unless they and an auditor who joins then
    /// find everything as the ledgers say. Returns what the group played by, for a
    /// later look at its blocks, with the report.
    async fn finish(self) -> (Ledger, LedgerReport) {
        self.progress.finish();
        let Ok(ended) = tokio::time::timeout(3 * PATIENCE, self.playing).await else {
            panic!(
                "the bots of group {:?} did not come to an end: {:?}",
                self.name,
                self.progress.bots()
            );
        };
        let report = match ended.unwrap() {
            Ok(report) => report,
            Err(error) => panic!("the bots of group {:?} found fault: {error:#}", self.name),
        };
        println!("group {:?}: the bots are content: {report}", self.name);
        assert_eq!(report.blocks_audited as usize, report.blocks.len());
        (self.ledger, report)
    }
}

/// How many of the blocks the ledgers of `report` have a word about are in the chunk
/// column with the x coordinate `chunk`.
fn blocks_in(report: &LedgerReport, chunk: i32) -> usize {
    report
        .blocks
        .keys()
        .filter(|(x, _, _)| x >> 4 == chunk)
        .count()
}

/// A group of two stands in a chunk, is sent three chunks further and, once it is
/// there and has played there, back; the world is divided at `boundaries`.
async fn a_group_goes_three_chunks_further_and_back(test: &str, boundaries: &[i32]) {
    const HOME: i32 = 1;
    const AWAY: i32 = 4;
    let seed = seed();
    println!("{test}: seed {seed} (set CLUSTINE_SENDING_SEED={seed} to run it again)");
    let (server, address, lines) = server(boundaries).await;
    let mut group = Group::start(&address, "", 2, 0, HOME, &lines, seed);
    group.arrives_in(HOME).await;
    group.plays().await;
    group.is_in(HOME);

    group.walk_to(AWAY);
    // Nobody is there the moment it is said, whatever the bots have noted so far.
    assert!(!group.progress.arrived());
    group.arrives_in(AWAY).await;
    group.plays().await;
    group.is_in(AWAY);

    group.walk_to(HOME);
    assert!(!group.progress.arrived());
    group.arrives_in(HOME).await;
    group.plays().await;
    group.is_in(HOME);

    let (_, report) = group.finish().await;
    assert!(report.actions > 0 && report.placed > 0, "{report}");
    assert!(
        blocks_in(&report, HOME) > 0 && blocks_in(&report, AWAY) > 0,
        "{report}"
    );
    // Each of the two stepped across every boundary on the way there, and again on
    // the way back.
    assert_eq!(report.crossings as usize, 4 * lines.len(), "{report}");
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_group_that_is_sent_three_chunks_further_and_back_keeps_its_ledger() {
    if a_repetition() {
        return;
    }
    a_group_goes_three_chunks_further_and_back("one region", &[]).await;
}

/// The same with a boundary between the two chunks: the bots are handed from one
/// region to the other on the way, building on both sides of the boundary as they
/// pass it.
#[tokio::test(flavor = "multi_thread")]
async fn a_group_that_is_sent_across_a_boundary_and_back_keeps_its_ledger() {
    if a_repetition() {
        return;
    }
    a_group_goes_three_chunks_further_and_back("two regions", &[3]).await;
}

/// Three groups, of two, one and one, play at once in the chunk players enter in, on
/// lanes side by side in the block rows z = 0 to 15. One of them is sent seven chunks
/// further, across a boundary and out of view of where it began and of the others,
/// and leaves the game there: its auditor joins while the others play on, walks
/// across their plots to its lane and on to what the group built out of view. The
/// others end later, and what the group that left built is still there then.
#[tokio::test(flavor = "multi_thread")]
async fn groups_play_side_by_side_while_one_is_sent_out_of_view_and_leaves() {
    if a_repetition() {
        return;
    }
    const HOME: i32 = 0;
    const AWAY: i32 = 7;
    let seed = seed();
    println!("three groups: seed {seed} (set CLUSTINE_SENDING_SEED={seed} to run it again)");
    let (server, address, lines) = server(&[4]).await;
    let mut a = Group::start(&address, "A", 2, 0, HOME, &lines, seed);
    let mut b = Group::start(&address, "B", 1, 8, HOME, &lines, seed + 1);
    let mut c = Group::start(&address, "C", 1, 12, HOME, &lines, seed + 2);
    for group in [&mut a, &mut b, &mut c] {
        group.arrives_in(HOME).await;
        group.plays().await;
    }

    b.walk_to(AWAY);
    b.arrives_in(AWAY).await;
    b.plays().await;
    b.is_in(AWAY);
    // Nobody else went anywhere.
    a.is_in(HOME);
    c.is_in(HOME);

    let (b_played, b_report) = b.finish().await;
    assert_eq!(b_report.crossings, 1, "{b_report}");
    assert!(
        blocks_in(&b_report, HOME) > 0 && blocks_in(&b_report, AWAY) > 0,
        "{b_report}"
    );
    // The others played while that group was audited and left, and play on.
    for group in [&mut a, &mut c] {
        group.plays().await;
        group.is_in(HOME);
    }
    let (_, a_report) = a.finish().await;
    let (_, c_report) = c.finish().await;

    // Nobody built in the block column players enter in, which every bot and every
    // auditor walked along to their lane while others were building beside it.
    let air = i32::from(blocks::AIR.0);
    for report in [&a_report, &b_report, &c_report] {
        let entered: Vec<i32> = report
            .blocks
            .iter()
            .filter(|((x, _, _), _)| *x == 0)
            .map(|(_, state)| *state)
            .collect();
        assert!(
            !entered.is_empty() && entered.iter().all(|state| *state == air),
            "the blocks of the plots at x = 0 are {entered:?}"
        );
    }
    // No two groups have a word about the same block.
    let all = a_report.blocks.len() + b_report.blocks.len() + c_report.blocks.len();
    let mut blocks = a_report.blocks.clone();
    blocks.extend(b_report.blocks.clone());
    blocks.extend(c_report.blocks.clone());
    assert_eq!(blocks.len(), all);

    // What the group that left first built is as its ledgers say after the others
    // have played on beside it.
    if let Err(error) = audit_blocks(&address, &b_played, &b_report.blocks).await {
        panic!("after the other groups had played on: {error:#}");
    }
    server.stop().await;
}
