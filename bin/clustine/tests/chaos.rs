//! Workers and the world store are killed, without warning and again and again, under
//! players who keep playing and keep a ledger of what they were told (the ledger
//! scenario of the bots). Milestone M3 promises that nobody is disconnected by that,
//! that every block change a client was shown or told was handled is on disk, that a
//! player keeps entity, position, hotbar and held slot, and that nobody sees another
//! player vanish or twice. These tests fail if any of it does not hold.
//!
//! What is killed when follows from a seed, which every test prints. To run a seed
//! again, set `CLUSTINE_CHAOS_SEED`; the kills then come in the same order, though not
//! at the same instants, so a failure that needs a rare coincidence may take several
//! runs to come back. `CLUSTINE_CHAOS_KILLS` sets how many rounds of killing each test
//! does, for a longer run. `CLUSTINE_CHAOS_KEEP` keeps the processes' logs of a test
//! that passes; those of a test that fails are always kept, and the failure says where.
//!
//! How often the workers write a checkpoint follows from the seed as well; see
//! `checkpoint_seconds`.
//!
//! The edge is left alone: an edge that dies takes its players with it. The coordinator
//! is killed in a test of its own, as it is merely to be missed while it is away.

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clustine::Config;
use clustine_botswarm::ledger::LineCrossing;
use clustine_botswarm::{Bot, Ledger, LedgerReport, Progress, Random, audit_blocks, ledger};
use clustine_data::blocks;
use clustine_protocol::packets::play::face;
use clustine_region::RegionId;
use tempfile::TempDir;
use tokio::task::JoinHandle;

use common::processes::{Cluster, Turn, turn, worker_name};
use common::{VIEW_DISTANCE, config, start_with, view_area};

/// How long the cluster may take to be whole again after a kill, and the bots to wind
/// up at the end. A takeover takes the lease, which is three seconds in these tests,
/// and a moment; this only ever runs out when something hangs.
const PATIENCE: Duration = Duration::from_secs(60);

/// How long the world store stays away when it stays away for long: a good part of
/// the 20 seconds after which the edge gives up on a player whose region is silent.
const LONG_ABSENCE: Duration = Duration::from_secs(8);

/// How often a state that is waited for is looked at.
const LOOK: Duration = Duration::from_millis(20);

/// What a region is called in the logs, by its number from west to east.
type Region = usize;

/// A cluster with one worker more than it has regions, bots playing the ledger
/// scenario on it, and the means to kill its processes.
struct Chaos {
    /// Its leave to run beside the other tests' clusters.
    _turn: Turn,
    /// Where the world and the logs are; taken out when they are to be kept.
    directory: Option<TempDir>,
    cluster: Cluster,
    seed: u64,
    random: Random,
    /// The first block east of each boundary, in ascending order.
    lines: Vec<i32>,
    /// What the bots play.
    ledger: Ledger,
    progress: Arc<Progress>,
    scenario: Option<JoinHandle<anyhow::Result<LedgerReport>>>,
    started: Instant,
    /// What was done to the cluster and what it did about it, in order.
    deeds: Vec<String>,
    /// How long each worker's log was when the worker was last started or last lost
    /// the world store: what it says after that is about its present life.
    since: BTreeMap<usize, usize>,
    /// How long the coordinator's log was when the coordinator was last started.
    coordinator_since: usize,
}

/// Whether this run of the tests is the one that repeats the end-to-end tests on a
/// world divided into regions, which `CLUSTINE_TEST_BOUNDARIES` asks for. These tests
/// divide their worlds themselves and take minutes, so they run once, in the run
/// without it.
fn a_repetition() -> bool {
    std::env::var_os("CLUSTINE_TEST_BOUNDARIES").is_some()
}

/// The seed of this run: `CLUSTINE_CHAOS_SEED` if set, else the clock.
fn seed() -> u64 {
    match std::env::var("CLUSTINE_CHAOS_SEED") {
        Ok(seed) => seed.parse().expect("CLUSTINE_CHAOS_SEED is a number"),
        Err(_) => {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
            now.as_nanos() as u64 % 1_000_000
        }
    }
}

/// How often the workers write a checkpoint in a run with this seed: every second or
/// two, so that kills fall into checkpoints and regions are restored from a state file
/// and what came after it, or practically never, so that they are restored from the log
/// alone.
fn checkpoint_seconds(seed: u64) -> u64 {
    [1, 2, 300][(seed % 3) as usize]
}

/// How many rounds of killing a test does: `CLUSTINE_CHAOS_KILLS` if set, else
/// `default`.
fn rounds(default: u32) -> u32 {
    match std::env::var("CLUSTINE_CHAOS_KILLS") {
        Ok(rounds) => rounds.parse().expect("CLUSTINE_CHAOS_KILLS is a number"),
        Err(_) => default,
    }
}

impl Chaos {
    /// Starts a cluster whose world is divided at the chunk x coordinates `boundaries`,
    /// and bots that walk between the block x coordinates `west` and `east` on it.
    /// Returns once every bot is on its lane and has been acknowledged.
    async fn start(test: &str, boundaries: &[i32], west: f64, east: f64) -> Self {
        // One worker for each region and one to spare.
        Self::start_with(test, boundaries, boundaries.len() + 2, west, east).await
    }

    /// The same with `workers` workers, however many regions there are.
    async fn start_with(
        test: &str,
        boundaries: &[i32],
        workers: usize,
        west: f64,
        east: f64,
    ) -> Self {
        let turn = turn().await;
        let seed = seed();
        println!("{test}: seed {seed} (set CLUSTINE_CHAOS_SEED={seed} to run it again)");
        let directory = tempfile::Builder::new()
            .prefix("clustine-chaos-")
            .tempdir()
            .unwrap();
        let list: Vec<String> = boundaries.iter().map(i32::to_string).collect();
        let mut cluster = Cluster::new(directory.path(), workers, &list.join(",")).await;
        let checkpoints = checkpoint_seconds(seed);
        println!("{test}: the workers checkpoint every {checkpoints} s");
        cluster.worker_arguments =
            vec!["--checkpoint-interval".to_owned(), checkpoints.to_string()];
        cluster.start().await;

        let lines: Vec<i32> = boundaries.iter().map(|chunk| chunk * 16).collect();
        let scenario = Ledger {
            bots: 4,
            rounds: None,
            duration: None,
            west,
            east,
            lines: lines.clone(),
            seed,
            ..Ledger::default()
        };
        let progress = Progress::new(scenario.bots);
        let address = cluster.edge.0.clone();
        let playing = {
            let (scenario, progress) = (scenario.clone(), progress.clone());
            tokio::spawn(async move { ledger(&address, &scenario, &progress).await })
        };
        let mut chaos = Self {
            _turn: turn,
            directory: Some(directory),
            cluster,
            seed,
            random: Random::new(seed),
            lines,
            ledger: scenario,
            progress,
            scenario: Some(playing),
            started: Instant::now(),
            deeds: Vec::new(),
            since: (0..workers).map(|worker| (worker, 0)).collect(),
            coordinator_since: 0,
        };
        chaos
            .until("every bot is on its lane and acknowledged", |chaos| {
                let bots = chaos.progress.bots();
                bots.iter().all(|bot| bot.playing && bot.acknowledged > 0)
            })
            .await;
        chaos.whole().await;
        chaos
    }

    /// Notes something that was done to the cluster, or that it did.
    fn note(&mut self, deed: String) {
        let deed = format!("{:7.3} s: {deed}", self.started.elapsed().as_secs_f64());
        println!("{deed}");
        self.deeds.push(deed);
    }

    /// Fails the test, saying what led up to it and where the logs are, which are kept.
    fn fail(&mut self, message: &str) -> ! {
        self.note("failed".to_owned());
        let kept = self.keep();
        let mut report = format!("{message}\n\nseed {}; what was done:\n", self.seed);
        for deed in &self.deeds {
            report.push_str(&format!("  {deed}\n"));
        }
        report.push_str("the bots when it failed:\n");
        for (number, bot) in self.progress.bots().iter().enumerate() {
            report.push_str(&format!("  {number}: {bot:?}\n"));
        }
        report.push_str(&format!(
            "the routing table last logged: {:?}\n",
            self.routes()
        ));
        report.push_str("the logs of the processes:\n");
        for name in self.cluster.names() {
            report.push_str(&format!("  {}\n", kept.join("logs").join(name).display()));
        }
        panic!("{report}");
    }

    /// Keeps the directory with the world and the logs beyond the test.
    fn keep(&mut self) -> PathBuf {
        match self.directory.take() {
            Some(directory) => directory.keep(),
            None => self.cluster.logs.parent().unwrap().to_owned(),
        }
    }

    /// Looks after the processes the way whatever runs them in production does: a
    /// worker that has ended by itself, as one does whose region went to another, is
    /// started again. Fails if anything else has ended, or the bots have.
    async fn tend(&mut self) {
        for number in 0..self.cluster.workers.len() {
            let Some(worker) = &mut self.cluster.workers[number].1 else {
                continue;
            };
            if let Some(status) = worker.try_wait().unwrap() {
                self.note(format!(
                    "{} ended by itself ({status}); starting it again",
                    worker_name(number)
                ));
                self.start_worker(number);
            }
        }
        for (name, process) in self.cluster.processes() {
            if name.starts_with("worker") {
                continue;
            }
            let ended = process.as_mut().and_then(|child| child.try_wait().unwrap());
            if let Some(status) = ended {
                self.fail(&format!("the {name} ended by itself ({status})"));
            }
        }
        if self.scenario.as_ref().is_some_and(JoinHandle::is_finished) {
            let ended = self.scenario.take().unwrap().await.unwrap();
            match ended {
                Ok(report) => {
                    self.fail(&format!("the bots stopped before being told to: {report}"))
                }
                Err(error) => self.fail(&format!("the bots found fault: {error:#}")),
            }
        }
    }

    /// Waits until `state` holds, looking after the processes meanwhile. Fails, naming
    /// `what` was waited for, if that takes longer than anything should.
    async fn until(&mut self, what: &str, mut state: impl FnMut(&Self) -> bool) {
        let waiting = Instant::now();
        loop {
            self.tend().await;
            if state(self) {
                return;
            }
            if waiting.elapsed() > PATIENCE {
                self.fail(&format!("waited {PATIENCE:?} in vain until {what}"));
            }
            tokio::time::sleep(LOOK).await;
        }
    }

    /// The owner of every region that has one, as the coordinator last logged its
    /// routing table in its present life: the address of the worker and the epoch.
    fn routes(&self) -> BTreeMap<Region, (String, u64)> {
        let log = self.cluster.log("coordinator");
        let log = log.get(self.coordinator_since..).unwrap_or_default();
        let Some(line) = log
            .lines()
            .rev()
            .find(|line| line.contains("the routing table changed"))
        else {
            return BTreeMap::new();
        };
        let regions = line.split("regions=").nth(1).unwrap_or_default();
        regions
            .split(", ")
            .filter_map(|entry| {
                // "region 1 at 127.0.0.1:4000 with epoch 7" or "region 1 without an owner".
                let words: Vec<&str> = entry.split_whitespace().collect();
                match words.as_slice() {
                    ["region", region, "at", address, "with", "epoch", epoch, ..] => Some((
                        region.parse().ok()?,
                        ((*address).to_owned(), epoch.parse().ok()?),
                    )),
                    _ => None,
                }
            })
            .collect()
    }

    /// The number of the worker that listens on `address`.
    fn worker_at(&self, address: &str) -> Option<usize> {
        let workers = &self.cluster.workers;
        workers.iter().position(|worker| worker.0 == address)
    }

    /// The worker that runs `region` according to the routing table.
    fn owner(&self, region: Region) -> Option<usize> {
        let (address, _) = self.routes().remove(&region)?;
        self.worker_at(&address)
    }

    /// What a worker has logged in its present life.
    fn log_since(&self, worker: usize) -> String {
        let log = self.cluster.log(&worker_name(worker));
        log.get(self.since[&worker]..)
            .unwrap_or_default()
            .to_owned()
    }

    /// Whether `region` has an owner that is alive and has restored the region in its
    /// present life.
    fn runs(&self, region: Region) -> bool {
        let Some((address, epoch)) = self.routes().remove(&region) else {
            return false;
        };
        let Some(worker) = self.worker_at(&address) else {
            return false;
        };
        let running = format!("running a region region={region} epoch={epoch} ");
        self.cluster.workers[worker].1.is_some() && self.log_since(worker).contains(&running)
    }

    /// Whether every region runs, and its owner is the one the edge last linked to.
    fn every_region_runs(&self) -> bool {
        let routes = self.routes();
        let edge = self.cluster.log("edge");
        (0..=self.lines.len()).all(|region| {
            let linked = format!("linked to a region region={region} epoch=");
            let last_link = edge.lines().rev().find(|line| line.contains(&linked));
            let epoch = routes.get(&region).map(|(_, epoch)| *epoch);
            self.runs(region)
                && last_link
                    .zip(epoch)
                    .is_some_and(|(line, epoch)| line.contains(&format!("{linked}{epoch} ")))
        })
    }

    /// Waits until the cluster is whole again: every region runs, on a worker the edge
    /// is linked to, and every bot has had an action acknowledged that it took after
    /// that was so.
    async fn whole(&mut self) {
        self.until(
            "every region runs on a worker and the edge is linked to it",
            Self::every_region_runs,
        )
        .await;
        self.served().await;
        self.note("the cluster is whole".to_owned());
    }

    /// Waits until every bot has had an action acknowledged that it takes from now on.
    async fn served(&mut self) {
        let sent: Vec<i32> = self.progress.bots().iter().map(|bot| bot.sent).collect();
        self.until("every bot has a new action acknowledged", |chaos| {
            let bots = chaos.progress.bots();
            bots.iter()
                .zip(&sent)
                .all(|(bot, sent)| bot.acknowledged > *sent)
        })
        .await;
    }

    /// Waits until a worker that was started has registered with the coordinator,
    /// whether or not it was given a region.
    async fn registered(&mut self, worker: usize) {
        self.until("a worker that was started again has registered", |chaos| {
            let log = chaos.log_since(worker);
            log.contains("waiting to be given a region") || log.contains("given a region")
        })
        .await;
    }

    /// The region that the block column at `x` is in.
    fn region_at(&self, x: f64) -> Region {
        self.lines
            .iter()
            .filter(|line| f64::from(**line) <= x)
            .count()
    }

    /// One of the regions that have a bot in them, chosen by the seed.
    fn region_with_players(&mut self) -> Region {
        let mut regions: Vec<Region> = self
            .progress
            .bots()
            .iter()
            .map(|bot| self.region_at(bot.x))
            .collect();
        regions.sort_unstable();
        regions.dedup();
        regions[self.random.below(regions.len() as u64) as usize]
    }

    /// Kills a worker without warning.
    async fn kill_worker(&mut self, worker: usize, why: &str) {
        let name = worker_name(worker);
        let mut process = self.cluster.workers[worker]
            .1
            .take()
            .unwrap_or_else(|| panic!("{name} is not running"));
        process.kill().await.unwrap();
        let bots: Vec<String> = self
            .progress
            .bots()
            .iter()
            .map(|bot| format!("{:.1}", bot.x))
            .collect();
        self.note(format!(
            "killed {name}, {why}; the bots are at x = {}",
            bots.join(", ")
        ));
    }

    /// Starts a worker that is not running.
    fn start_worker(&mut self, worker: usize) {
        let name = worker_name(worker);
        self.since.insert(worker, self.cluster.log(&name).len());
        self.cluster.start_worker(worker);
        self.note(format!("started {name}"));
    }

    /// Kills the worker that runs `region` and waits for the cluster to be whole
    /// again, with a spare worker. `at_once` starts the worker again right away, before
    /// the coordinator has missed it; otherwise it is started once another worker has
    /// the region.
    async fn kill_owner(&mut self, region: Region, at_once: bool, why: &str) {
        let Some(worker) = self.owner(region) else {
            self.fail(&format!("region {region} has no owner to kill"));
        };
        self.kill_worker(worker, &format!("which ran region {region} {why}"))
            .await;
        if at_once {
            self.start_worker(worker);
        }
        self.whole().await;
        if !at_once {
            self.start_worker(worker);
        }
        self.registered(worker).await;
    }

    /// Kills the worker that runs `region`, and then the worker that is given the
    /// region next: while it restores the region, or just when it has and the edge has
    /// linked to it. The first is started again in its place.
    async fn kill_owner_and_heir(&mut self, region: Region, during_restore: bool) {
        let Some(worker) = self.owner(region) else {
            self.fail(&format!("region {region} has no owner to kill"));
        };
        self.kill_worker(worker, &format!("which ran region {region}"))
            .await;
        self.until("another worker is given the region", |chaos| {
            chaos.owner(region).is_some_and(|heir| heir != worker)
        })
        .await;
        let heir = self
            .owner(region)
            .expect("the region was given to a worker");
        let when = if during_restore {
            "been given"
        } else {
            self.until(
                "the worker that was given the region runs it",
                Self::every_region_runs,
            )
            .await;
            "taken over"
        };
        self.kill_worker(heir, &format!("which had just {when} region {region}"))
            .await;
        self.start_worker(worker);
        self.whole().await;
        self.start_worker(heir);
        self.registered(heir).await;
        self.registered(worker).await;
    }

    /// Stops a worker where it is, without it noticing, or lets it carry on.
    async fn signal(&mut self, worker: usize, signal: &str, what: &str) {
        let process = self.cluster.workers[worker].1.as_ref();
        let pid = process
            .and_then(|process| process.id())
            .expect("a running worker");
        let sent = tokio::process::Command::new("kill")
            .args([format!("-{signal}"), pid.to_string()])
            .status()
            .await;
        assert!(sent.unwrap().success());
        self.note(format!("{what} {}", worker_name(worker)));
    }

    /// Stops the worker that runs `region` dead, without it or its connections
    /// noticing, until another worker has restored the region, and with `served` until
    /// the players are served by that one. Then lets it carry on, and waits for it to
    /// have let go of the region and for the cluster to be whole.
    async fn freeze_owner(&mut self, region: Region, served: bool) {
        let Some(worker) = self.owner(region) else {
            self.fail(&format!("region {region} has no owner to freeze"));
        };
        let frozen = format!("which ran region {region}: froze");
        self.signal(worker, "STOP", &frozen).await;
        self.until("another worker has restored the region", |chaos| {
            chaos.owner(region).is_some_and(|heir| heir != worker) && chaos.runs(region)
        })
        .await;
        if served {
            self.whole().await;
        }
        let length = self.cluster.log(&worker_name(worker)).len();
        self.since.insert(worker, length);
        self.signal(worker, "CONT", "woke up").await;
        // It lets go of the region, or ends and is started again to register afresh.
        self.until(
            "the worker that woke up has let go of its region",
            |chaos| {
                let log = chaos.log_since(worker);
                [
                    "dropping it",
                    "waiting to be given a region",
                    "given a region",
                ]
                .iter()
                .any(|said| log.contains(said))
            },
        )
        .await;
        self.whole().await;
    }

    /// Joins as `name` and waits for the chunks around the spawn point.
    async fn join(&mut self, name: &str) -> Bot {
        let address = self.cluster.edge.0.clone();
        let joined = async {
            let mut bot = Bot::join(&address, name).await?;
            let count = view_area((0, 0), VIEW_DISTANCE).len();
            bot.wait_for_chunks(count, PATIENCE).await?;
            anyhow::Ok(bot)
        };
        match joined.await {
            Ok(bot) => bot,
            Err(error) => self.fail(&format!("{name} could not join: {error:#}")),
        }
    }

    /// Kills the coordinator without warning.
    async fn kill_coordinator(&mut self) {
        let coordinator = self.cluster.coordinator.1.take();
        let mut coordinator = coordinator.expect("the coordinator is running");
        coordinator.kill().await.unwrap();
        self.note("killed the coordinator".to_owned());
    }

    /// Starts a coordinator, which knows nothing of the one before.
    fn start_coordinator(&mut self) {
        self.coordinator_since = self.cluster.log("coordinator").len();
        self.cluster.start_coordinator();
        self.note("started the coordinator".to_owned());
    }

    /// Kills the world store without warning. From here on what the workers say about
    /// running a region has to be said anew.
    async fn kill_store(&mut self) {
        let mut store = self.cluster.store.1.take().expect("the store is running");
        store.kill().await.unwrap();
        for worker in 0..self.cluster.workers.len() {
            let length = self.cluster.log(&worker_name(worker)).len();
            // A worker that is not running starts with a mark of its own.
            if self.cluster.workers[worker].1.is_some() {
                self.since.insert(worker, length);
            }
        }
        self.note("killed the world store".to_owned());
    }

    /// Starts the world store, either right away or once every worker that ran a
    /// region when it died has noticed that it is gone.
    async fn start_store(&mut self, at_once: bool) {
        if !at_once {
            let owners: Vec<usize> = (0..=self.lines.len())
                .filter_map(|region| self.owner(region))
                .filter(|worker| self.cluster.workers[*worker].1.is_some())
                .collect();
            self.until("the workers have noticed that the store is gone", |chaos| {
                owners.iter().all(|worker| {
                    // One whose process has been replaced since has nothing to notice.
                    chaos.log_since(*worker).contains("lost the world store")
                        || !chaos
                            .cluster
                            .log(&worker_name(*worker))
                            .get(..chaos.since[worker])
                            .is_some_and(|before| !before.is_empty())
                })
            })
            .await;
        }
        self.cluster.start_store();
        self.note("started the world store".to_owned());
    }

    /// Tells the bots to stop, and fails unless they and an auditor who joins then find
    /// everything as the ledgers say, and unless an auditor finds the same once every
    /// process has been killed and started again. Then the processes are asked to
    /// stop, which each has to do cleanly.
    async fn finish(mut self) -> LedgerReport {
        self.tend().await;
        self.progress.finish();
        let mut scenario = self.scenario.take().expect("the bots are still playing");
        let waiting = Instant::now();
        while !scenario.is_finished() {
            if waiting.elapsed() > 3 * PATIENCE {
                self.fail("the bots did not come to an end");
            }
            tokio::time::sleep(LOOK).await;
        }
        let report = match (&mut scenario).await.unwrap() {
            Ok(report) => report,
            Err(error) => self.fail(&format!("the bots found fault: {error:#}")),
        };
        self.note(format!("the bots are content: {report}"));
        self.tend().await;

        // Then the power goes: every process is killed at once. What the ledgers say
        // is what the processes that are started on the same disk have.
        self.cluster.kill().await;
        self.note("killed every process".to_owned());
        self.cluster.start().await;
        let address = self.cluster.edge.0.clone();
        if let Err(error) = audit_blocks(&address, &self.ledger, &report.blocks).await {
            self.fail(&format!(
                "after every process was killed and started again: {error:#}"
            ));
        }
        self.note("the world is as the ledgers say after starting from disk".to_owned());
        self.cluster.terminate().await;
        if std::env::var_os("CLUSTINE_CHAOS_KEEP").is_some() {
            println!("kept: {}", self.keep().display());
        }
        report
    }
}

/// The worker that runs a region with players in it is killed, again and again.
/// Sometimes it is back before the coordinator has missed it, which is a process that
/// crashed and was started again; sometimes a worker that was waiting takes the region
/// and the killed one becomes the spare; and sometimes the worker that takes the region
/// is killed in turn.
#[tokio::test(flavor = "multi_thread")]
async fn players_keep_playing_while_the_workers_that_run_their_regions_are_killed() {
    if a_repetition() {
        return;
    }
    let mut chaos = Chaos::start("workers", &[3], 33.5, 62.5).await;
    for _ in 0..rounds(4) {
        let region = chaos.region_with_players();
        match chaos.random.below(5) {
            0 | 1 => chaos.kill_owner(region, true, "with players in it").await,
            2 | 3 => chaos.kill_owner(region, false, "with players in it").await,
            _ => {
                let during_restore = chaos.random.one_in(2);
                chaos.kill_owner_and_heir(region, during_restore).await;
            }
        }
    }
    let report = chaos.finish().await;
    assert!(report.actions > 0 && report.crossings > 0, "{report}");
}

/// Two workers run three regions between them. One of the two is killed, again and
/// again: the other then runs all three, and when the killed one is back it is given
/// one of them again, so that neither runs everything for long.
#[tokio::test(flavor = "multi_thread")]
async fn players_keep_playing_on_fewer_workers_than_regions_while_one_is_killed() {
    if a_repetition() {
        return;
    }
    let mut chaos = Chaos::start_with("few workers", &[2, 3], 2, 20.5, 60.5).await;
    let regions = 3;
    for _ in 0..rounds(3) {
        // Regions are evened out, so with both workers there neither runs everything.
        chaos
            .until("both workers run a region", |chaos| {
                let first = chaos.owner(0);
                (1..regions).any(|region| chaos.owner(region) != first) && chaos.every_region_runs()
            })
            .await;
        let region = chaos.region_with_players();
        let Some(killed) = chaos.owner(region) else {
            chaos.fail(&format!("region {region} has no owner to kill"));
        };
        chaos
            .kill_worker(killed, &format!("which ran region {region}, among others"))
            .await;
        // The one that is left runs all of them.
        chaos
            .until("the other worker runs every region", |chaos| {
                (0..regions).all(|region| chaos.owner(region).is_some_and(|owner| owner != killed))
            })
            .await;
        chaos.whole().await;
        chaos.start_worker(killed);
        chaos.registered(killed).await;
        // And hands one over once the other is back.
        chaos
            .until(
                "the worker that came back is given a region again",
                |chaos| (0..regions).any(|region| chaos.owner(region) == Some(killed)),
            )
            .await;
        chaos.whole().await;
    }
    let report = chaos.finish().await;
    assert!(report.actions > 0 && report.crossings > 0, "{report}");
}

/// The world store is killed and comes back, again and again: sometimes at once,
/// sometimes only after the workers have found it gone, and sometimes after a good
/// part of the time the edge has patience for.
#[tokio::test(flavor = "multi_thread")]
async fn players_keep_playing_while_the_world_store_is_killed_and_comes_back() {
    if a_repetition() {
        return;
    }
    let mut chaos = Chaos::start("world store", &[3], 33.5, 62.5).await;
    for _ in 0..rounds(4) {
        chaos.kill_store().await;
        match chaos.random.below(5) {
            0 | 1 => chaos.start_store(true).await,
            2 | 3 => chaos.start_store(false).await,
            _ => {
                // How long the store is away is what is tried here, not a wait for
                // anything to come about.
                tokio::time::sleep(LONG_ABSENCE).await;
                chaos.note(format!(
                    "the world store has been away for {LONG_ABSENCE:?}"
                ));
                chaos.start_store(false).await;
            }
        }
        chaos.whole().await;
    }
    let report = chaos.finish().await;
    assert!(report.actions > 0 && report.crossings > 0, "{report}");
}

/// Workers and the world store are killed as the seed has it, on a world of three
/// regions: one or the other, or one while the cluster is still getting over the
/// other.
#[tokio::test(flavor = "multi_thread")]
async fn players_keep_playing_while_workers_and_the_world_store_are_killed_at_random() {
    if a_repetition() {
        return;
    }
    let mut chaos = Chaos::start("mixed", &[2, 3], 20.5, 60.5).await;
    for _ in 0..rounds(5) {
        let region = chaos.region_with_players();
        match chaos.random.below(6) {
            5 => {
                let during_restore = chaos.random.one_in(2);
                chaos.kill_owner_and_heir(region, during_restore).await;
            }
            0 => chaos.kill_owner(region, false, "with players in it").await,
            1 => chaos.kill_owner(region, true, "with players in it").await,
            2 => {
                chaos.kill_store().await;
                let at_once = chaos.random.one_in(2);
                chaos.start_store(at_once).await;
                chaos.whole().await;
            }
            // The store dies, and while it is away a worker does too.
            3 => {
                chaos.kill_store().await;
                let worker = chaos.owner(region).expect("the cluster was whole");
                chaos
                    .kill_worker(
                        worker,
                        &format!("which ran region {region} and had lost the store"),
                    )
                    .await;
                chaos.start_store(true).await;
                chaos.whole().await;
                chaos.start_worker(worker);
                chaos.registered(worker).await;
            }
            // A worker dies, and the store while its region is on its way to another.
            _ => {
                let worker = chaos.owner(region).expect("the cluster was whole");
                chaos
                    .kill_worker(worker, &format!("which ran region {region}"))
                    .await;
                chaos.kill_store().await;
                chaos.start_store(true).await;
                chaos.whole().await;
                chaos.start_worker(worker);
                chaos.registered(worker).await;
            }
        }
    }
    let report = chaos.finish().await;
    assert!(report.actions > 0 && report.crossings > 0, "{report}");
}

/// A worker is killed just as a bot steps across the boundary of its region: the
/// worker the bot is leaving, or the one it is walking into; or the world store is,
/// which both need for handing the bot over. The bots walk back and forth close to
/// the boundary, so that there is always one about to cross.
#[tokio::test(flavor = "multi_thread")]
async fn players_keep_playing_when_a_worker_is_killed_while_one_of_them_crosses() {
    if a_repetition() {
        return;
    }
    let mut chaos = Chaos::start("crossing", &[3], 43.5, 52.5).await;
    let mut crossings = chaos.progress.crossings();
    for _ in 0..rounds(6) {
        // The next step across the boundary from now on, which the bot announces just
        // before it sends it.
        crossings.mark_unchanged();
        chaos.tend().await;
        let changed = tokio::time::timeout(PATIENCE, crossings.changed()).await;
        let crossing: LineCrossing = match changed {
            Ok(Ok(())) => crossings.borrow_and_update().expect("a crossing"),
            _ => chaos.fail("no bot stepped across the boundary any more"),
        };
        let west = chaos.region_at(f64::from(crossing.line) - 1.0);
        let (leaving, entering) = if crossing.eastwards {
            (west, west + 1)
        } else {
            (west + 1, west)
        };
        let (region, which) = match chaos.random.below(3) {
            0 => (leaving, "leaving"),
            1 => (entering, "entering"),
            _ => {
                // What a region has decided about the bot is on its way to the disk.
                chaos.kill_store().await;
                let at_once = chaos.random.one_in(2);
                chaos.start_store(at_once).await;
                chaos.whole().await;
                continue;
            }
        };
        // The hand-over takes a few ticks from here; the kill lands somewhere in them.
        let delay = Duration::from_millis(chaos.random.below(120));
        tokio::time::sleep(delay).await;
        let at_once = chaos.random.one_in(2);
        let why = format!(
            "which bot {} was {which} {delay:?} after it stepped across",
            crossing.bot
        );
        chaos.kill_owner(region, at_once, &why).await;
    }
    let report = chaos.finish().await;
    assert!(report.actions > 0 && report.crossings > 0, "{report}");
}

/// A worker stops dead for longer than the lease without dying, as one does whose
/// machine hangs or that is cut off, and then carries on where it was: by then another
/// worker has restored its region. What the one that woke up believes must not reach
/// anyone; the world store refuses it, and it lets go.
#[tokio::test(flavor = "multi_thread")]
async fn players_keep_playing_when_a_worker_wakes_up_after_its_region_went_to_another() {
    if a_repetition() {
        return;
    }
    let mut chaos = Chaos::start("frozen worker", &[3], 33.5, 62.5).await;
    for _ in 0..rounds(3) {
        let region = chaos.region_with_players();
        chaos.freeze_owner(region, false).await;
    }
    let report = chaos.finish().await;
    assert!(report.actions > 0 && report.crossings > 0, "{report}");
}

/// A worker stops dead and stays so, without its connections closing, as when its
/// machine is cut off. Another worker takes its region, and the players are served by
/// that one long before the first wakes up.
#[tokio::test(flavor = "multi_thread")]
async fn players_are_served_by_another_worker_while_the_one_that_ran_their_region_hangs() {
    if a_repetition() {
        return;
    }
    let mut chaos = Chaos::start("hanging worker", &[3], 33.5, 62.5).await;
    for _ in 0..rounds(3) {
        let region = chaos.region_with_players();
        chaos.freeze_owner(region, true).await;
    }
    let report = chaos.finish().await;
    assert!(report.actions > 0 && report.crossings > 0, "{report}");
}

/// The coordinator is killed and another is started, which knows nothing: the workers
/// tell it what they run. Meanwhile the players are served as before, and afterwards a
/// worker that dies is replaced as before.
#[tokio::test(flavor = "multi_thread")]
async fn players_keep_playing_while_the_coordinator_is_killed_and_comes_back() {
    if a_repetition() {
        return;
    }
    let mut chaos = Chaos::start("coordinator", &[3], 33.5, 62.5).await;
    for _ in 0..rounds(2) {
        chaos.kill_coordinator().await;
        chaos.served().await;
        chaos.note("the bots are served without a coordinator".to_owned());
        chaos.start_coordinator();
        chaos.whole().await;
        let region = chaos.region_with_players();
        let at_once = chaos.random.one_in(3);
        chaos
            .kill_owner(region, at_once, "under the new coordinator")
            .await;
    }
    let report = chaos.finish().await;
    assert!(report.actions > 0 && report.crossings > 0, "{report}");
}

/// Players come and go while the region they do it in has no worker: one leaves and
/// another joins right after the worker of the region with the spawn point is killed.
/// The one who joins is let in once another worker has the region, finds the one who
/// left gone, and plays.
#[tokio::test(flavor = "multi_thread")]
async fn players_join_and_leave_while_the_region_they_do_it_in_has_no_worker() {
    if a_repetition() {
        return;
    }
    let mut chaos = Chaos::start("coming and going", &[3], 33.5, 62.5).await;
    let address = chaos.cluster.edge.0.clone();
    let spawn_region = chaos.region_at(0.5);
    let air = Some(i32::from(blocks::AIR.0));
    // Everyone who has left, none of whom may be seen again.
    let mut gone: Vec<String> = Vec::new();
    for round in 0..rounds(3) {
        let guest_name = format!("Guest{round}");
        let guest = chaos.join(&guest_name).await;
        let worker = chaos.owner(spawn_region).expect("the cluster was whole");
        let why = format!("which ran region {spawn_region}, where {guest_name} is");
        chaos.kill_worker(worker, &why).await;
        drop(guest);
        let visitor_name = format!("Visitor{round}");
        let joining = {
            let (address, name) = (address.clone(), visitor_name.clone());
            tokio::spawn(async move { Bot::join(&address, &name).await })
        };
        chaos.note(format!("{guest_name} left and {visitor_name} is joining"));
        chaos.whole().await;
        chaos.start_worker(worker);
        chaos.registered(worker).await;

        let mut visitor = match joining.await.unwrap() {
            Ok(visitor) => visitor,
            Err(error) => chaos.fail(&format!(
                "{visitor_name}, who joined while the region had no worker, was not let in: {error:#}"
            )),
        };
        gone.push(guest_name);
        // The visitor is given the world, without those who left, and can build in it.
        let played = async {
            let count = view_area((0, 0), VIEW_DISTANCE).len();
            visitor.wait_for_chunks(count, PATIENCE).await?;
            // Next to the spawn point, where none of the other bots builds; placed and
            // broken again, so that the next visitor finds the spot free.
            let placed = visitor.use_item_on(2, -61, 3, face::TOP).await?;
            visitor
                .wait_until(PATIENCE, |bot| {
                    bot.acknowledged_sequence >= placed
                        && bot
                            .block_at(2, -60, 3)
                            .is_ok_and(|block| block.is_some() && block != air)
                })
                .await?;
            let broken = visitor.dig(2, -60, 3).await?;
            visitor
                .wait_until(PATIENCE, |bot| {
                    bot.acknowledged_sequence >= broken
                        && bot.block_at(2, -60, 3).is_ok_and(|block| block == air)
                })
                .await
        };
        if let Err(error) = played.await {
            chaos.fail(&format!("{visitor_name} could not play: {error:#}"));
        }
        if let Some(ghost) = gone.iter().find(|name| visitor.seen_player(name).is_some()) {
            let seen = visitor.seen_player(ghost);
            chaos.fail(&format!(
                "{visitor_name} sees {ghost}, who has left: {seen:?}"
            ));
        }
        chaos.note(format!("{visitor_name} is in and has built"));
        drop(visitor);
        gone.push(visitor_name);
    }
    let report = chaos.finish().await;
    assert!(report.actions > 0 && report.crossings > 0, "{report}");
}

/// A player whose region has lost its worker leaves and joins again under their name,
/// before another worker has the region: what a person does when the game stands
/// still. They are let in once the region runs again, and can play.
#[tokio::test(flavor = "multi_thread")]
async fn a_player_who_left_joins_again_while_their_region_has_no_worker() {
    if a_repetition() {
        return;
    }
    let mut chaos = Chaos::start("leaving and coming back", &[3], 33.5, 62.5).await;
    let address = chaos.cluster.edge.0.clone();
    let spawn_region = chaos.region_at(0.5);
    let air = Some(i32::from(blocks::AIR.0));
    for round in 0..rounds(3) {
        let name = format!("Guest{round}");
        let guest = chaos.join(&name).await;
        let worker = chaos.owner(spawn_region).expect("the cluster was whole");
        let why = format!("which ran region {spawn_region}, where {name} is");
        chaos.kill_worker(worker, &why).await;
        drop(guest);
        let joining = {
            let (address, name) = (address.clone(), name.clone());
            tokio::spawn(async move { Bot::join(&address, &name).await })
        };
        chaos.note(format!("{name} left and is joining again"));
        chaos.whole().await;
        chaos.start_worker(worker);
        chaos.registered(worker).await;

        let mut back = match joining.await.unwrap() {
            Ok(back) => back,
            Err(error) => chaos.fail(&format!(
                "{name}, who joined again while the region had no worker, was not let in: {error:#}"
            )),
        };
        let played = async {
            let count = view_area((0, 0), VIEW_DISTANCE).len();
            back.wait_for_chunks(count, PATIENCE).await?;
            let placed = back.use_item_on(2, -61, 3, face::TOP).await?;
            back.wait_until(PATIENCE, |bot| {
                bot.acknowledged_sequence >= placed
                    && bot
                        .block_at(2, -60, 3)
                        .is_ok_and(|block| block.is_some() && block != air)
            })
            .await?;
            let broken = back.dig(2, -60, 3).await?;
            back.wait_until(PATIENCE, |bot| {
                bot.acknowledged_sequence >= broken
                    && bot.block_at(2, -60, 3).is_ok_and(|block| block == air)
            })
            .await
        };
        if let Err(error) = played.await {
            chaos.fail(&format!(
                "{name} could not play after joining again: {error:#}"
            ));
        }
        chaos.note(format!("{name} is back and has built"));
        drop(back);
    }
    let report = chaos.finish().await;
    assert!(report.actions > 0 && report.crossings > 0, "{report}");
}

/// In a single process a region changes hands the moment the server is told so, with
/// no lease to wait for, so it can happen far more often than a worker can be killed:
/// dozens of times, as a bot steps across the boundary or whenever, and half the time
/// before anyone has been served by the runner that took over last. What holds for
/// killed workers holds here.
#[tokio::test(flavor = "multi_thread")]
async fn players_keep_playing_while_their_regions_change_hands_over_and_over() {
    if a_repetition() {
        return;
    }
    let _turn = turn().await;
    let seed = seed();
    println!("changing hands: seed {seed} (set CLUSTINE_CHAOS_SEED={seed} to run it again)");
    let directory = tempfile::tempdir().unwrap();
    let (mut server, address) = start_with(Config {
        pins: vec![3],
        // Everything between the edge and the regions goes through the codec, as it
        // does between processes.
        serialise_link: true,
        world: Some(directory.path().join("world")),
        checkpoint_interval: Duration::from_secs(checkpoint_seconds(seed)),
        ..config()
    })
    .await;
    let scenario = Ledger {
        bots: 4,
        rounds: None,
        duration: None,
        west: 43.5,
        east: 52.5,
        lines: vec![48],
        seed,
        ..Ledger::default()
    };
    let progress = Progress::new(scenario.bots);
    let playing = {
        let (address, scenario, progress) = (address.clone(), scenario.clone(), progress.clone());
        tokio::spawn(async move { ledger(&address, &scenario, &progress).await })
    };

    let started = Instant::now();
    let mut deeds: Vec<String> = Vec::new();
    // Waits until `state` holds for the bots; if they end first, they have found fault.
    let until = async |deeds: &[String], what: &str, state: &dyn Fn(&Progress) -> bool| {
        let waiting = Instant::now();
        while !state(&progress) {
            assert!(
                !playing.is_finished() && waiting.elapsed() <= PATIENCE,
                "the bots ended, or {PATIENCE:?} passed, before {what}; seed {seed}; \
                 what was done:\n{}\nthe bots: {:?}",
                deeds.join("\n"),
                progress.bots()
            );
            tokio::time::sleep(LOOK).await;
        }
    };
    until(&deeds, "every bot was on its lane", &|progress| {
        let bots = progress.bots();
        bots.iter().all(|bot| bot.playing && bot.acknowledged > 0)
    })
    .await;

    let mut random = Random::new(seed);
    let mut crossings = progress.crossings();
    for _ in 0..rounds(40) {
        let on_crossing = random.one_in(2);
        if on_crossing {
            crossings.mark_unchanged();
            let crossed = tokio::time::timeout(PATIENCE, crossings.changed()).await;
            assert!(
                crossed.is_ok(),
                "no bot stepped across the boundary any more"
            );
            // The hand-over of the bot takes a few ticks from here.
            tokio::time::sleep(Duration::from_millis(random.below(120))).await;
        }
        let region = RegionId(random.below(2) as u32);
        let taken = server.take_over(region).await;
        deeds.push(format!(
            "{:7.3} s: region {region} taken over{}: {taken:?}",
            started.elapsed().as_secs_f64(),
            if on_crossing { " as a bot crossed" } else { "" }
        ));
        assert!(taken.is_ok(), "seed {seed}:\n{}", deeds.join("\n"));
        if random.one_in(2) {
            let sent: Vec<i32> = progress.bots().iter().map(|bot| bot.sent).collect();
            until(
                &deeds,
                "every bot had a new action acknowledged",
                &|progress| {
                    let bots = progress.bots();
                    bots.iter()
                        .zip(&sent)
                        .all(|(bot, sent)| bot.acknowledged > *sent)
                },
            )
            .await;
        }
    }

    progress.finish();
    let report = match playing.await.unwrap() {
        Ok(report) => report,
        Err(error) => panic!(
            "the bots found fault: {error:#}\n\nseed {seed}; what was done:\n{}",
            deeds.join("\n")
        ),
    };
    println!("the bots are content: {report}");
    assert!(report.actions > 0 && report.crossings > 0, "{report}");

    server.stop().await;
}

/// A world one of whose regions has changed hands is served again by a server that is
/// started on it afterwards.
#[tokio::test(flavor = "multi_thread")]
async fn a_world_whose_region_changed_hands_is_served_by_the_next_server() {
    if a_repetition() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let on_disk = || Config {
        pins: vec![3],
        world: Some(directory.path().join("world")),
        ..config()
    };
    let (mut server, _) = start_with(on_disk()).await;
    server.take_over(RegionId(0)).await.unwrap();
    server.stop().await;

    let started = clustine::Server::start(on_disk()).await;
    match started {
        Ok(server) => server.stop().await,
        Err(error) => panic!("the next server did not start: {error:#}"),
    }
}
