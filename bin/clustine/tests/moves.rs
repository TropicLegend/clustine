//! Regions are moved from one worker to another on purpose, under players who keep
//! playing and keep a ledger of what they were told (the ledger scenario of the bots):
//! with `clustine move`, by telling a worker to stop, and by replacing every worker in
//! turn. `docs/adr/0009-moving-a-region.md` promises that nobody is disconnected by
//! that, that nothing a player was shown is lost, that what players did meanwhile takes
//! effect afterwards, and that every failure in the middle of a move is a crash like
//! any other, which the cluster gets over. These tests fail if any of it does not hold.
//!
//! How long players stand still is measured where they are. **The pause of a bot** at a
//! move is the longest time that anything the bot sent waited for its acknowledgement,
//! among everything that was waiting at some moment between `clustine move` being
//! started and the cluster being whole again with every bot served. So that a bot that
//! happens to be only walking is measured as well, every bot sends a pulse every other
//! client tick, which the region it is in acknowledges and which changes nothing; the
//! measure is therefore short of the truth by a tenth of a second at most. **The pause
//! of a move** is the longest pause of any bot, wherever it stood: a bot beyond the
//! boundary waits as well if it builds across it. The first test reports the pauses of
//! its moves and fails above three seconds.
//!
//! What is done when follows from a seed, which every test prints. To run a seed again,
//! set `CLUSTINE_MOVES_SEED`; things then come in the same order, though not at the
//! same instants. `CLUSTINE_MOVES_ROUNDS` sets how many rounds each test does, for a
//! longer run. `CLUSTINE_MOVES_SOAK` also runs what takes long by its nature: a worker
//! that is told to stop and has nobody to hand over to, for the 20 seconds it waits.
//! `CLUSTINE_MOVES_KEEP` keeps the processes' logs of a test that passes; those of a
//! test that fails are always kept, and the failure says where.
//!
//! The processes are those of an unoptimised build, as in every test here, so the
//! pauses are longer than those of a server built for use.

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clustine_botswarm::ledger::LineCrossing;
use clustine_botswarm::{Bot, Ledger, LedgerReport, Progress, Random, Wait, audit_blocks, ledger};
use tempfile::TempDir;
use tokio::process::Child;
use tokio::task::JoinHandle;

use common::processes::{Cluster, worker_name};
use common::{VIEW_DISTANCE, free_address, view_area};

/// How long anything may take that is merely waited for. It only ever runs out when
/// something hangs.
const PATIENCE: Duration = Duration::from_secs(60);

/// How often a state that is waited for is looked at.
const LOOK: Duration = Duration::from_millis(20);

/// The coordinator's lease when it is told none, which is how these tests start it.
const LEASE: Duration = Duration::from_secs(5);

/// The moment beyond the lease that a cluster may take to be whole again after a
/// failure in the middle of a move: the new owner restores the region, the edge links
/// to it and resumes, in an unoptimised build on a machine that does other things too.
const MOMENT: Duration = Duration::from_secs(5);

/// The longest that players may stand still when a region is moved; see ADR-0009,
/// section 5.
const LONGEST_PAUSE: Duration = Duration::from_secs(3);

/// How long a worker that is told to stop waits for someone to take its region.
const LEAVE_WITHIN: Duration = Duration::from_secs(20);

/// How long a worker that is told to stop and has someone to hand its region to may
/// take to be gone: "a few seconds".
const A_FEW_SECONDS: Duration = Duration::from_secs(8);

/// Every this many client ticks each bot sends a pulse.
const PULSE: u32 = 2;

/// The view distance of the edge where the bots are spread out, in chunks: what a
/// player usually has.
const WIDE_VIEW: i32 = 8;

/// Held by the test that is running. Each test is a cluster of processes with a lease
/// of a few seconds; several at once on a small machine starve each other until leases
/// run out by themselves, which says nothing about the server.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// What a region is called in the logs, by its number from west to east.
type Region = usize;

/// What a test's cluster and bots are like.
#[derive(Debug, Clone, Copy)]
struct Setup {
    /// The chunk x coordinates at which the world is divided.
    boundaries: &'static [i32],
    workers: usize,
    /// Whether the edge grants a view distance of [`WIDE_VIEW`] and the bots' lanes are
    /// so far apart that no two bots have the same chunks in view. Otherwise the view
    /// distance is the small one of the other tests and the lanes are side by side,
    /// which is quick to start.
    wide: bool,
    /// The block x coordinates the bots walk between.
    west: f64,
    east: f64,
}

/// Two regions that meet at block x = 48, three workers, the bots side by side and
/// walking a good way to either side of the boundary.
const SMALL: Setup = Setup {
    boundaries: &[3],
    workers: 3,
    wide: false,
    west: 33.5,
    east: 62.5,
};

/// Whether this run of the tests is the one that repeats the end-to-end tests on a
/// world divided into regions, which `CLUSTINE_TEST_BOUNDARIES` asks for. These tests
/// divide their worlds themselves and take minutes, so they run once, in the run
/// without it.
fn a_repetition() -> bool {
    std::env::var_os("CLUSTINE_TEST_BOUNDARIES").is_some()
}

/// The seed of this run: `CLUSTINE_MOVES_SEED` if set, else the clock.
fn seed() -> u64 {
    match std::env::var("CLUSTINE_MOVES_SEED") {
        Ok(seed) => seed.parse().expect("CLUSTINE_MOVES_SEED is a number"),
        Err(_) => {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
            now.as_nanos() as u64 % 1_000_000
        }
    }
}

/// How often the workers write a checkpoint in a run with this seed: every second or
/// two, so that moves fall into checkpoints, or practically never, so that the
/// checkpoint a release begins with has everything to save that was built so far.
fn checkpoint_seconds(seed: u64) -> u64 {
    [1, 2, 300][(seed % 3) as usize]
}

/// How many rounds a test does: `CLUSTINE_MOVES_ROUNDS` if set, else `default`.
fn rounds(default: u32) -> u32 {
    match std::env::var("CLUSTINE_MOVES_ROUNDS") {
        Ok(rounds) => rounds.parse().expect("CLUSTINE_MOVES_ROUNDS is a number"),
        Err(_) => default,
    }
}

/// Whether what takes long by its nature is run as well.
fn a_soak() -> bool {
    std::env::var_os("CLUSTINE_MOVES_SOAK").is_some()
}

/// The middle one of `durations`, and the longest. Of none, nothing.
fn median_and_worst(durations: &[Duration]) -> (Duration, Duration) {
    let mut sorted = durations.to_vec();
    sorted.sort_unstable();
    let median = sorted.get(sorted.len() / 2).copied().unwrap_or_default();
    (median, sorted.last().copied().unwrap_or_default())
}

/// Seconds with three decimals, for a message.
fn seconds(duration: Duration) -> String {
    format!("{:.3} s", duration.as_secs_f64())
}

/// The time of day at which a line of a log was written, in seconds, which is what
/// two lines of processes on one machine can be compared by.
fn time_of(line: &str) -> Option<f64> {
    // "2026-10-08T09:10:26.123456Z  INFO ...".
    let clock = line.split_whitespace().next()?.split('T').nth(1)?;
    let mut parts = clock.trim_end_matches('Z').split(':');
    let hours: f64 = parts.next()?.parse().ok()?;
    let minutes: f64 = parts.next()?.parse().ok()?;
    let rest: f64 = parts.next()?.parse().ok()?;
    Some(hours * 3600.0 + minutes * 60.0 + rest)
}

/// When `log` last said `what` about `region` with `epoch`.
fn last_said(log: &str, what: &str, region: Region, epoch: u64) -> Option<f64> {
    let (region, epoch) = (format!("region={region} "), format!("epoch={epoch}"));
    let line = log.lines().rev().find(|line| {
        // The epoch is the last thing on some lines and not on others.
        let with_epoch = line.ends_with(&epoch) || line.contains(&format!("{epoch} "));
        line.contains(what) && line.contains(&region) && with_epoch
    })?;
    time_of(line)
}

/// What `clustine move` came to.
#[derive(Debug, Clone)]
struct Asked {
    region: Region,
    /// The worker it asked for, if it asked for one.
    to: Option<String>,
    /// The status the command ended with, if it ended by itself.
    code: Option<i32>,
    /// What it printed, and what it complained of.
    said: String,
    complained: String,
    /// How long the command ran.
    took: Duration,
}

impl Asked {
    /// Whether the region was moved because its owner released it.
    fn released(&self) -> bool {
        self.code == Some(0) && self.said.contains("released by its owner")
    }

    /// Whether the region was moved by taking it from an owner that did not answer.
    fn taken(&self) -> bool {
        self.code == Some(0) && self.said.contains("taken from its owner")
    }

    /// The reason the coordinator gave for refusing the move, if it refused.
    fn refusal(&self) -> Option<&str> {
        let (_, reason) = self.complained.split_once("the coordinator refused: ")?;
        (self.code != Some(0)).then_some(reason.trim())
    }

    /// The worker the command says has the region now.
    fn new_owner(&self) -> Option<&str> {
        let (_, rest) = self.said.split_once(" is run by ")?;
        rest.split_whitespace().next()
    }

    /// The epoch the command says the region is run with now.
    fn new_epoch(&self) -> Option<u64> {
        let (_, rest) = self.said.split_once(" with epoch ")?;
        rest.split_whitespace().next()?.parse().ok()
    }

    /// How long the command says the move took, from asking to the new assignment.
    fn own_time(&self) -> Option<Duration> {
        let (before, _) = self.said.split_once(" ms after asking")?;
        let millis = before.split_whitespace().next_back()?.parse().ok()?;
        Some(Duration::from_millis(millis))
    }

    /// In a line, for the list of what was done.
    fn outcome(&self) -> String {
        let said = self.said.trim().replace('\n', "; ");
        let complained = self.complained.trim().replace('\n', "; ");
        format!(
            "exit status {:?} after {}; it printed \"{said}\" and complained \"{complained}\"",
            self.code,
            seconds(self.took)
        )
    }
}

/// A move that went as it should, and what the bots noticed of it.
#[derive(Debug, Clone)]
struct Moved {
    region: Region,
    /// The workers it went from and to.
    from: usize,
    to: usize,
    /// What `clustine move` gave as its own time.
    own_time: Duration,
    /// The longest pause of any bot, and of any bot that stood in the region when the
    /// move was asked for.
    pause: Duration,
    pause_in_region: Duration,
    /// Each bot's pause, for a message.
    bots: String,
    /// Where the time went, as far as the logs say.
    stages: String,
}

/// A cluster with bots playing the ledger scenario on it, and the means to move its
/// regions and to do harm to its processes.
struct Moves {
    /// Keeps the other tests waiting.
    _alone: tokio::sync::MutexGuard<'static, ()>,
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
    /// The workers that were told to stop and have not been waited for yet. That one of
    /// them ends is as it should be.
    told_to_stop: Vec<String>,
}

impl Moves {
    /// Starts a cluster and bots as `setup` says. Returns once every bot is on its lane
    /// and has been acknowledged, and the cluster is whole.
    async fn start(test: &str, setup: Setup) -> Self {
        let alone = ONE_AT_A_TIME.lock().await;
        let seed = seed();
        println!("{test}: seed {seed} (set CLUSTINE_MOVES_SEED={seed} to run it again)");
        let directory = tempfile::Builder::new()
            .prefix("clustine-moves-")
            .tempdir()
            .unwrap();
        let list: Vec<String> = setup.boundaries.iter().map(i32::to_string).collect();
        let checkpoints = checkpoint_seconds(seed);
        println!("{test}: the workers checkpoint every {checkpoints} s");
        let mut attempt = 0;
        let cluster = loop {
            let mut cluster = Cluster::new(directory.path(), setup.workers, &list.join(",")).await;
            cluster.worker_arguments =
                vec!["--checkpoint-interval".to_owned(), checkpoints.to_string()];
            // The lease a cluster has unless someone says otherwise: what the record's
            // "nobody waits for the lease" is about.
            cluster.lease_seconds = None;
            if setup.wide {
                cluster.view_distance = WIDE_VIEW;
            }
            cluster.start().await;
            // An address that was free when it was picked may have been taken by the
            // time a process listened on it: by a connection that some other process on
            // this machine made meanwhile. That says nothing about the server, so the
            // cluster is started anew, on other addresses.
            attempt += 1;
            match Self::taken_address(&mut cluster).await {
                Some(name) if attempt < 3 => {
                    println!("{test}: the address picked for {name} was taken; starting anew");
                    cluster.kill().await;
                    for name in cluster.names() {
                        let _ = std::fs::remove_file(cluster.logs.join(name));
                    }
                    let _ = std::fs::remove_dir_all(&cluster.world);
                }
                _ => break cluster,
            }
        };

        let lines: Vec<i32> = setup.boundaries.iter().map(|chunk| chunk * 16).collect();
        let mut scenario = Ledger {
            bots: 4,
            rounds: None,
            duration: None,
            west: setup.west,
            east: setup.east,
            lines: lines.clone(),
            seed,
            pulse: Some(PULSE),
            ..Ledger::default()
        };
        // Lanes side by side are still a walk from where the bots join, and the auditor
        // walks all of it again.
        scenario.to_the_lane = 3.0;
        if setup.wide {
            // Twenty chunks from lane to lane, which is as far as two players with this
            // view distance see between them: the new owner of a region has every
            // bot's chunks to load, and none of them twice. That far the bots run.
            scenario.first_lane = -480;
            scenario.lane_spacing = 320;
            scenario.to_the_lane = 8.0;
        }
        let progress = Progress::new(scenario.bots);
        let address = cluster.edge.0.clone();
        let playing = {
            let (scenario, progress) = (scenario.clone(), progress.clone());
            tokio::spawn(async move { ledger(&address, &scenario, &progress).await })
        };
        let mut moves = Self {
            _alone: alone,
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
            since: (0..setup.workers).map(|worker| (worker, 0)).collect(),
            coordinator_since: 0,
            told_to_stop: Vec::new(),
        };
        moves
            .until("every bot is on its lane and acknowledged", |moves| {
                let bots = moves.progress.bots();
                bots.iter().all(|bot| bot.playing && bot.acknowledged > 0)
            })
            .await;
        moves.whole().await;
        moves
    }

    /// Waits until every worker of a cluster that has just been started has registered,
    /// and returns the name of a process that ended instead because its address was in
    /// use, if there is one.
    async fn taken_address(cluster: &mut Cluster) -> Option<String> {
        let waiting = Instant::now();
        loop {
            let mut ended = None;
            for (name, process) in cluster.processes() {
                let gone = process.as_mut().and_then(|child| child.try_wait().unwrap());
                if gone.is_some() {
                    ended = Some(name);
                }
            }
            if let Some(name) = ended {
                let in_use = cluster.log(&name).contains("Address already in use");
                // Anything else that ends a process is for the test to find.
                return in_use.then_some(name);
            }
            let registered = (0..cluster.workers.len()).all(|worker| {
                let log = cluster.log(&worker_name(worker));
                log.contains("waiting to be given a region") || log.contains("given a region")
            });
            if registered || waiting.elapsed() > PATIENCE {
                return None;
            }
            tokio::time::sleep(LOOK).await;
        }
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
        let now = Instant::now();
        let waits = self.progress.waits();
        for (number, bot) in self.progress.bots().iter().enumerate() {
            let unanswered = waits[number]
                .iter()
                .find(|wait| wait.acknowledged.is_none())
                .map(|wait| format!("has waited {} for an answer", seconds(wait.lasted(now))));
            report.push_str(&format!(
                "  {number}: {bot:?}, {}\n",
                unanswered.unwrap_or_else(|| "waits for nothing".to_owned())
            ));
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

    /// Fails if a process has ended that nobody stopped, or the bots have. A worker
    /// does not end by itself any more when its region goes to another.
    async fn tend(&mut self) {
        let told_to_stop = self.told_to_stop.clone();
        for (name, process) in self.cluster.processes() {
            if told_to_stop.contains(&name) {
                continue;
            }
            let ended = process.as_mut().and_then(|child| child.try_wait().unwrap());
            if let Some(status) = ended {
                self.fail(&format!("{name} ended by itself ({status})"));
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

    /// Waits until `state` holds, looking after the processes meanwhile, and returns
    /// how long that took. Fails, naming `what` was waited for, if it takes longer than
    /// anything should.
    async fn until(&mut self, what: &str, mut state: impl FnMut(&Self) -> bool) -> Duration {
        let waiting = Instant::now();
        loop {
            self.tend().await;
            if state(self) {
                return waiting.elapsed();
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

    /// The epoch `region` is run with according to the routing table.
    fn epoch(&self, region: Region) -> Option<u64> {
        self.routes().remove(&region).map(|(_, epoch)| epoch)
    }

    /// The workers whose processes run and that own no region according to the routing
    /// table: those a region can be moved to.
    fn spares(&self) -> Vec<usize> {
        let routes = self.routes();
        let owners: Vec<Option<usize>> = routes
            .values()
            .map(|(address, _)| self.worker_at(address))
            .collect();
        (0..self.cluster.workers.len())
            .filter(|worker| self.cluster.workers[*worker].1.is_some())
            .filter(|worker| !owners.contains(&Some(*worker)))
            .collect()
    }

    /// What a worker has logged in its present life.
    fn log_since(&self, worker: usize) -> String {
        let log = self.cluster.log(&worker_name(worker));
        log.get(self.since[&worker]..)
            .unwrap_or_default()
            .to_owned()
    }

    /// What the coordinator has logged in its present life.
    fn coordinator_log(&self) -> String {
        let log = self.cluster.log("coordinator");
        log.get(self.coordinator_since..)
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

    /// Waits until the cluster is whole: every region runs, on a worker the edge is
    /// linked to, and every bot has had something acknowledged that it sent after that
    /// was so.
    async fn whole(&mut self) {
        self.until(
            "every region runs on a worker and the edge is linked to it",
            Self::every_region_runs,
        )
        .await;
        self.served().await;
        self.note("the cluster is whole".to_owned());
    }

    /// Waits for the cluster to be whole, and fails if that was not so within `limit`
    /// of `since`, which is when `what` was done.
    async fn whole_within(&mut self, since: Instant, limit: Duration, what: &str) {
        self.whole().await;
        let took = since.elapsed();
        self.note(format!(
            "whole and every bot served {} after {what}",
            seconds(took)
        ));
        if took > limit {
            self.fail(&format!(
                "the cluster took {} to be whole, with every bot served, after {what}; \
                 it has {limit:?} for that",
                seconds(took)
            ));
        }
    }

    /// Waits until every bot has had something acknowledged that it sends from now on.
    async fn served(&mut self) {
        let sent: Vec<i32> = self.progress.bots().iter().map(|bot| bot.sent).collect();
        self.until("every bot has something new acknowledged", |moves| {
            let bots = moves.progress.bots();
            bots.iter()
                .zip(&sent)
                .all(|(bot, sent)| bot.acknowledged > *sent)
        })
        .await;
    }

    /// Waits until a worker that was started has registered with the coordinator,
    /// whether or not it was given a region.
    async fn registered(&mut self, worker: usize) {
        self.until("a worker that was started has registered", |moves| {
            let log = moves.log_since(worker);
            log.contains("waiting to be given a region") || log.contains("given a region")
        })
        .await;
        self.note(format!("{} has registered", worker_name(worker)));
    }

    /// How many times the coordinator has let `worker` register in its present life.
    fn registrations(&self, worker: usize) -> usize {
        let registers = format!("a worker registers worker={} ", worker_name(worker));
        self.coordinator_said(&registers)
    }

    /// The region that the block column at `x` is in.
    fn region_at(&self, x: f64) -> Region {
        self.lines
            .iter()
            .filter(|line| f64::from(**line) <= x)
            .count()
    }

    /// Where the bots are, for a message.
    fn whereabouts(&self) -> String {
        let bots: Vec<String> = self
            .progress
            .bots()
            .iter()
            .map(|bot| format!("{:.1}", bot.x))
            .collect();
        format!("the bots are at x = {}", bots.join(", "))
    }

    /// Runs `clustine move` for `region`, to the worker `to` or to any, without waiting
    /// for what comes of it.
    fn asking(&mut self, region: Region, to: Option<usize>) -> JoinHandle<Asked> {
        let to = to.map(worker_name);
        self.asking_by_name(region, to)
    }

    /// Like [`Moves::asking`], for a worker by whatever name.
    fn asking_by_name(&mut self, region: Region, to: Option<String>) -> JoinHandle<Asked> {
        let mut command = self.cluster.move_command(region, to.as_deref());
        let whom = to.clone().unwrap_or_else(|| "any worker".to_owned());
        self.note(format!(
            "asked for region {region} to be moved to {whom}; {}",
            self.whereabouts()
        ));
        tokio::spawn(async move {
            let asking = Instant::now();
            let output = command.output().await.expect("the server binary runs");
            Asked {
                region,
                to,
                code: output.status.code(),
                said: String::from_utf8_lossy(&output.stdout).into_owned(),
                complained: String::from_utf8_lossy(&output.stderr).into_owned(),
                took: asking.elapsed(),
            }
        })
    }

    /// Waits for what `clustine move` comes to.
    async fn answer(&mut self, asking: JoinHandle<Asked>) -> Asked {
        self.until("`clustine move` has ended", |_| asking.is_finished())
            .await;
        let asked = asking.await.expect("asking does not panic");
        let whom = asked.to.as_deref().unwrap_or("any worker");
        self.note(format!(
            "`clustine move` of region {} to {whom}: {}",
            asked.region,
            asked.outcome()
        ));
        asked
    }

    /// Fails unless `clustine move` said that the owner released the region, and waits
    /// until the routing table that the coordinator logs has the region with the epoch
    /// the command named, or a later one: the command has its answer a moment before
    /// the table is logged, and whoever waits for the cluster to be whole has to look at
    /// the table after the move and not the one before.
    async fn released(&mut self, asked: &Asked, what: &str) {
        let Some(epoch) = asked.new_epoch().filter(|_| asked.released()) else {
            self.fail(&format!(
                "{what} was not made by its owner releasing the region: {}",
                asked.outcome()
            ));
        };
        let region = asked.region;
        self.until(
            "the routing table has the region as it was moved",
            |moves| moves.epoch(region).is_some_and(|now| now >= epoch),
        )
        .await;
    }

    /// Runs `clustine move` and waits for what comes of it.
    async fn ask(&mut self, region: Region, to: Option<usize>) -> Asked {
        let asking = self.asking(region, to);
        self.answer(asking).await
    }

    /// Each bot's pause between `from` and `to`; see the top of this file.
    fn pauses(&self, from: Instant, to: Instant) -> Vec<Option<Wait>> {
        self.progress.longest_waits(from, to)
    }

    /// The longest pause of any bot between `from` and now.
    fn longest_pause_since(&self, from: Instant) -> Duration {
        let now = Instant::now();
        let pauses = self.pauses(from, now);
        let lasted = pauses.iter().flatten().map(|wait| wait.lasted(now));
        lasted.max().unwrap_or_default()
    }

    /// Fails if any bot has waited as long as the lease for anything since `from`,
    /// which is when `what` began.
    fn nobody_waited_for_the_lease(&mut self, from: Instant, what: &str) {
        let now = Instant::now();
        let pauses = self.pauses(from, now);
        let each: Vec<String> = pauses
            .iter()
            .map(|pause| pause.map_or("-".to_owned(), |wait| seconds(wait.lasted(now))))
            .collect();
        let longest = self.longest_pause_since(from);
        self.note(format!(
            "the longest any bot waited during {what}: {} (bot by bot: {})",
            seconds(longest),
            each.join(", ")
        ));
        if longest >= LEASE {
            self.fail(&format!(
                "a bot waited {} for an acknowledgement during {what}, which is as long as \
                 the lease of {LEASE:?} that nobody was to wait for",
                seconds(longest)
            ));
        }
    }

    /// Fails if the coordinator has taken a region from a worker for being silent, or
    /// for not releasing it in time, since its log was `since` bytes long.
    fn no_lease_ran_out(&mut self, since: usize, what: &str) {
        let log = self.cluster.log("coordinator");
        let log = log.get(since..).unwrap_or_default();
        let ran_out: Vec<&str> = log
            .lines()
            .filter(|line| {
                line.contains("the lease of a worker ran out")
                    || line.contains("was not released within the lease")
            })
            .collect();
        if !ran_out.is_empty() {
            let lines = ran_out.join("\n  ");
            self.fail(&format!(
                "during {what} the coordinator waited for a lease:\n  {lines}"
            ));
        }
    }

    /// Where the time of a move went, as far as the logs of the processes say: the old
    /// owner releasing, of which the region stood still only for the last part, the
    /// coordinator assigning and the new owner hearing of it, the new owner restoring,
    /// and the edge linking to it. What is left of a pause after that is the edge
    /// resuming with the new owner.
    fn stages(&self, region: Region, from: (usize, u64), to: (usize, u64)) -> String {
        let (old, new) = (from.1, to.1);
        let old_log = self.cluster.log(&worker_name(from.0));
        let new_log = self.cluster.log(&worker_name(to.0));
        let edge_log = self.cluster.log("edge");
        let asked = last_said(&old_log, "asked to release the region", region, old);
        let released = last_said(&old_log, "released the region", region, old);
        // The runner says when it stops ticking, without saying which region it runs.
        let stopped = old_log
            .lines()
            .rev()
            .find(|line| line.contains("the region stops ticking"))
            .and_then(time_of)
            .filter(|stopped| asked.is_some_and(|asked| *stopped >= asked));
        let given = last_said(&new_log, "given a region", region, new);
        let running = last_said(&new_log, "running a region", region, new);
        let linked = last_said(&edge_log, "linked to a region", region, new);
        let between = |from: Option<f64>, to: Option<f64>| match from.zip(to) {
            Some((from, to)) => format!("{:.0} ms", (to - from) * 1000.0),
            None => "?".to_owned(),
        };
        format!(
            "release {} of which without ticking {}, assignment {}, restore {}, link {}; \
             from being asked to release to linked {}",
            between(asked, released),
            between(stopped, released),
            between(released, given),
            between(given, running),
            between(running, linked),
            between(asked, linked),
        )
    }

    /// Moves `region` with `clustine move`, to the worker `to` or to any, and waits
    /// for the cluster to be whole. Fails unless the command says that the owner
    /// released the region and the region is another worker's afterwards: the one asked
    /// for, if one was. Returns what the bots noticed.
    async fn move_region(&mut self, region: Region, to: Option<usize>) -> Moved {
        let Some(from) = self.owner(region) else {
            self.fail(&format!("region {region} has no owner to move it from"));
        };
        let old_epoch = self.epoch(region).expect("the region has an owner");
        let in_region: Vec<bool> = self
            .progress
            .bots()
            .iter()
            .map(|bot| self.region_at(bot.x) == region)
            .collect();
        let before = Instant::now();
        let asked = self.ask(region, to).await;
        self.released(&asked, &format!("the move of region {region}"))
            .await;
        self.whole().await;
        let after = Instant::now();

        let now = self.owner(region).expect("the cluster is whole");
        let new_epoch = self.epoch(region).expect("the cluster is whole");
        let named = asked.new_owner().map(str::to_owned);
        if now == from || to.is_some_and(|to| to != now) || named != Some(worker_name(now)) {
            self.fail(&format!(
                "region {region} was to move from {} to {}; `clustine move` says it is run \
                 by {named:?} and the routing table says by {}",
                worker_name(from),
                to.map_or("any other worker".to_owned(), worker_name),
                worker_name(now)
            ));
        }
        let Some(own_time) = asked.own_time() else {
            self.fail(&format!(
                "`clustine move` did not say how long it took: {}",
                asked.outcome()
            ));
        };

        let pauses = self.pauses(before, after);
        let lasted = |wait: &Option<Wait>| wait.map(|wait| wait.lasted(after));
        let pause = pauses.iter().filter_map(lasted).max().unwrap_or_default();
        let pause_in_region = pauses
            .iter()
            .zip(&in_region)
            .filter(|(_, inside)| **inside)
            .filter_map(|(wait, _)| lasted(wait))
            .max()
            .unwrap_or_default();
        let bots: Vec<String> = pauses
            .iter()
            .zip(&in_region)
            .map(|(wait, inside)| {
                let place = if *inside { "inside" } else { "outside" };
                match wait {
                    Some(wait) => format!("{} {place}", seconds(wait.lasted(after))),
                    None => format!("nothing sent {place}"),
                }
            })
            .collect();
        let moved = Moved {
            region,
            from,
            to: now,
            own_time,
            pause,
            pause_in_region,
            bots: bots.join(", "),
            stages: self.stages(region, (from, old_epoch), (now, new_epoch)),
        };
        self.note(format!(
            "region {region} moved from {} to {}: the bots stood still for {} at most \
             ({}); `clustine move` gives {} ms; {}",
            worker_name(from),
            worker_name(now),
            seconds(moved.pause),
            moved.bots,
            own_time.as_millis(),
            moved.stages
        ));
        moved
    }

    /// Sends a worker's process a signal.
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
        if signal == "TERM" {
            self.told_to_stop.push(worker_name(worker));
        }
        self.note(format!(
            "{what} {}; {}",
            worker_name(worker),
            self.whereabouts()
        ));
    }

    /// Waits for a worker that was told to stop to be gone, and fails unless it ends
    /// without an error within `limit` of `told`. Returns how long it took.
    async fn gone(&mut self, worker: usize, told: Instant, limit: Duration) -> Duration {
        let name = worker_name(worker);
        // Out of the cluster first, so that nobody takes its ending for a crash.
        let mut process: Child = self.cluster.workers[worker]
            .1
            .take()
            .unwrap_or_else(|| panic!("{name} is not running"));
        self.told_to_stop.retain(|told| *told != name);
        let waiting = Instant::now();
        let status: ExitStatus = loop {
            if let Some(status) = process.try_wait().unwrap() {
                break status;
            }
            if waiting.elapsed() > PATIENCE {
                self.fail(&format!("{name} was told to stop and did not"));
            }
            self.tend().await;
            tokio::time::sleep(LOOK).await;
        };
        let took = told.elapsed();
        self.note(format!(
            "{name} ended ({status}) {} after it was told to stop",
            seconds(took)
        ));
        if !status.success() {
            self.fail(&format!("{name} was told to stop and ended with {status}"));
        }
        if took > limit {
            self.fail(&format!(
                "{name} took {} to stop after it was told to; it has {limit:?}",
                seconds(took)
            ));
        }
        took
    }

    /// Kills a worker without warning.
    async fn kill_worker(&mut self, worker: usize, why: &str) {
        let name = worker_name(worker);
        let mut process = self.cluster.workers[worker]
            .1
            .take()
            .unwrap_or_else(|| panic!("{name} is not running"));
        process.kill().await.unwrap();
        self.note(format!("killed {name}, {why}; {}", self.whereabouts()));
    }

    /// Starts a worker that is not running.
    fn start_worker(&mut self, worker: usize) {
        let name = worker_name(worker);
        self.since.insert(worker, self.cluster.log(&name).len());
        self.cluster.start_worker(worker);
        self.note(format!("started {name}"));
    }

    /// Adds a worker to the cluster that it did not have, and starts it.
    async fn add_worker(&mut self) -> usize {
        let worker = self.cluster.workers.len();
        self.cluster.workers.push((free_address().await, None));
        self.since.insert(worker, 0);
        self.start_worker(worker);
        worker
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

    /// Starts the world store.
    fn start_store(&mut self) {
        self.cluster.start_store();
        self.note("started the world store".to_owned());
    }

    /// How many times the coordinator has logged `message` in its present life.
    fn coordinator_said(&self, message: &str) -> usize {
        self.coordinator_log().matches(message).count()
    }

    /// Asks for `region` to be moved and waits until the coordinator has told its owner
    /// to release it, which is the beginning of the middle of a move.
    async fn begin_move(&mut self, region: Region, to: Option<usize>) -> JoinHandle<Asked> {
        let begun = self.coordinator_said("a region is to be released");
        let asking = self.asking(region, to);
        self.until("the coordinator has asked for the release", |moves| {
            moves.coordinator_said("a region is to be released") > begun
        })
        .await;
        asking
    }

    /// Tells the bots to stop, and fails unless they and an auditor who joins then find
    /// everything as the ledgers say. With `from_disk`, every process is then killed
    /// and started again, and an auditor has to find the same. Then the processes are
    /// asked to stop, which each has to do cleanly.
    async fn finish(mut self, from_disk: bool) -> LedgerReport {
        // A test may be over before the bots have walked as far as the boundary, and
        // stepping across it is part of what they are to have done.
        let crossings = self.progress.crossings();
        self.until("a bot has stepped across a boundary", |_| {
            crossings.borrow().is_some()
        })
        .await;
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

        if from_disk {
            // Then the power goes: every process is killed at once. What the ledgers
            // say is what the processes that are started on the same disk have.
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
        }
        self.stop_everything().await;
        if std::env::var_os("CLUSTINE_MOVES_KEEP").is_some() {
            println!("kept: {}", self.keep().display());
        }
        report
    }

    /// Asks every process to stop and fails unless each ends without an error: the
    /// edge, then the coordinator, then the workers and the world store. With the
    /// coordinator gone a worker has nobody to hand its region to and knows it, so it
    /// saves and stops instead of waiting for someone to take over, which is not what
    /// the end of a test is about.
    async fn stop_everything(&mut self) {
        let mut processes: Vec<(String, Option<Child>)> = vec![
            ("edge".to_owned(), self.cluster.edge.1.take()),
            ("coordinator".to_owned(), self.cluster.coordinator.1.take()),
        ];
        for (number, worker) in self.cluster.workers.iter_mut().enumerate() {
            processes.push((worker_name(number), worker.1.take()));
        }
        processes.push(("worldstore".to_owned(), self.cluster.store.1.take()));
        for (name, process) in processes {
            let Some(mut process) = process else {
                continue;
            };
            let pid = process.id().expect("it runs").to_string();
            let sent = tokio::process::Command::new("kill")
                .args(["-TERM", &pid])
                .status()
                .await;
            assert!(sent.unwrap().success());
            let Ok(status) = tokio::time::timeout(PATIENCE, process.wait()).await else {
                self.fail(&format!("{name} did not stop at the end of the test"));
            };
            let status = status.unwrap();
            if !status.success() {
                self.fail(&format!(
                    "{name} ended with {status} at the end of the test"
                ));
            }
        }
    }
}

/// Fails unless the bots did what the scenario is about.
fn played(report: &LedgerReport) {
    assert!(report.actions > 0 && report.crossings > 0, "{report}");
}

/// The regions of a cluster are moved back and forth between its workers with
/// `clustine move`, many times, under bots that are spread out so far that no two of
/// them have the same chunks in view, at the view distance a player usually has: the
/// new owner has the chunks of all of them to load each time. Nobody is disconnected,
/// the ledger holds, the owner releases the region every time, and the bots stand
/// still for three seconds at most.
#[tokio::test(flavor = "multi_thread")]
async fn players_stand_still_only_briefly_while_their_regions_are_moved_back_and_forth() {
    if a_repetition() {
        return;
    }
    let wide = Setup {
        wide: true,
        ..SMALL
    };
    let mut moves = Moves::start("moves", wide).await;
    // What the bots wait for when nothing is done to the cluster, to compare with:
    // over a few rounds of everyone being served, once the bots have all arrived and
    // the regions have loaded what they see.
    moves.served().await;
    let quiet = Instant::now();
    for _ in 0..10 {
        moves.served().await;
    }
    let undisturbed = moves.longest_pause_since(quiet);
    moves.note(format!(
        "undisturbed, a bot waits {} at most for an acknowledgement",
        seconds(undisturbed)
    ));

    let mut moved: Vec<Moved> = Vec::new();
    for round in 0..rounds(8) {
        // The region that most of the bots are in, so that every move has players to
        // keep standing. They walk from one region into the other and back, so it is
        // now this one and now that one. To a worker named or left to the coordinator,
        // which comes to the same one here: there is one that runs nothing.
        let bots = moves.progress.bots();
        let inside = |region| {
            let inside = bots.iter().filter(|bot| moves.region_at(bot.x) == region);
            inside.count()
        };
        let region = match inside(0).cmp(&inside(1)) {
            std::cmp::Ordering::Greater => 0,
            std::cmp::Ordering::Less => 1,
            std::cmp::Ordering::Equal => round as usize % 2,
        };
        let to = if moves.random.one_in(2) {
            Some(moves.spares()[0])
        } else {
            None
        };
        moved.push(moves.move_region(region, to).await);
    }

    println!("the moves of this run (seed {}):", moves.seed);
    for (number, moved) in moved.iter().enumerate() {
        println!(
            "  {number}: region {} from {} to {}: pause {} (of bots inside the region {}); \
             `clustine move` gives {} ms; bots: {}; {}",
            moved.region,
            worker_name(moved.from),
            worker_name(moved.to),
            seconds(moved.pause),
            seconds(moved.pause_in_region),
            moved.own_time.as_millis(),
            moved.bots,
            moved.stages
        );
    }
    let pauses: Vec<Duration> = moved.iter().map(|moved| moved.pause).collect();
    let own_times: Vec<Duration> = moved.iter().map(|moved| moved.own_time).collect();
    let (median, worst) = median_and_worst(&pauses);
    let (own_median, own_worst) = median_and_worst(&own_times);
    let summary = format!(
        "{} moves: the pause was {} in the middle and {} at worst; `clustine move` gave \
         {} in the middle and {} at worst; undisturbed, a bot waits {}",
        moved.len(),
        seconds(median),
        seconds(worst),
        seconds(own_median),
        seconds(own_worst),
        seconds(undisturbed)
    );
    moves.note(summary);
    if worst > LONGEST_PAUSE {
        moves.fail(&format!(
            "the bots stood still for {} when a region was moved; they may for \
             {LONGEST_PAUSE:?}",
            seconds(worst)
        ));
    }
    played(&moves.finish(true).await);
}

/// A region is moved at the moments at which the most is under way: while a bot steps
/// across the boundary into or out of it; while another move of the same region, or of
/// the other region to the one worker that waits, is asked for at the same time; while
/// someone joins or leaves in it; and again right after it was moved, while its new
/// owner is still restoring it.
#[tokio::test(flavor = "multi_thread")]
async fn players_keep_playing_when_regions_are_moved_at_the_worst_moments() {
    if a_repetition() {
        return;
    }
    // The bots walk back and forth close to the boundary, so that there is always one
    // about to cross.
    let close = Setup {
        west: 43.5,
        east: 52.5,
        ..SMALL
    };
    let mut moves = Moves::start("worst moments", close).await;
    let mut crossings = moves.progress.crossings();
    let mut pauses: Vec<Duration> = Vec::new();
    // How often the second of two moves found the owner still restoring the region.
    let mut while_restoring = 0;
    for round in 0..rounds(6) {
        match moves.random.below(5) {
            // The next step across the boundary from now on, which the bot announces
            // just before it sends it.
            0 | 1 => {
                crossings.mark_unchanged();
                moves.tend().await;
                let changed = tokio::time::timeout(PATIENCE, crossings.changed()).await;
                let crossing: LineCrossing = match changed {
                    Ok(Ok(())) => crossings.borrow_and_update().expect("a crossing"),
                    _ => moves.fail("no bot stepped across the boundary any more"),
                };
                let west = moves.region_at(f64::from(crossing.line) - 1.0);
                let (leaving, entering) = if crossing.eastwards {
                    (west, west + 1)
                } else {
                    (west + 1, west)
                };
                let (region, which) = if moves.random.one_in(2) {
                    (leaving, "leaving")
                } else {
                    (entering, "entering")
                };
                // The hand-over takes a few ticks from here; the move begins somewhere
                // in them.
                let delay = Duration::from_millis(moves.random.below(120));
                tokio::time::sleep(delay).await;
                moves.note(format!(
                    "bot {} stepped across the boundary {delay:?} ago, {which} region {region}",
                    crossing.bot
                ));
                pauses.push(moves.move_region(region, None).await.pause);
            }
            // Two moves at once: of one region, or of both regions when one worker
            // waits. One of them gets the worker. The other is refused, or comes after
            // the first and finds the worker that released its region waiting.
            2 => {
                let same = moves.random.one_in(2);
                let regions = if same { [0, 0] } else { [0, 1] };
                let asking = regions.map(|region| moves.asking(region, None));
                let mut answers = Vec::new();
                for asking in asking {
                    answers.push(moves.answer(asking).await);
                }
                let released = answers.iter().filter(|asked| asked.released()).count();
                let neither = answers
                    .iter()
                    .find(|asked| !asked.released() && asked.refusal().is_none());
                if let Some(asked) = neither {
                    let outcome = asked.outcome();
                    moves.fail(&format!(
                        "of two moves asked for at once, one was neither made nor refused \
                         with a reason: {outcome}"
                    ));
                }
                if released == 0 {
                    moves.fail(
                        "of two moves asked for at once, with a worker waiting, none was made",
                    );
                }
                for asked in answers.iter().filter(|asked| asked.released()) {
                    moves.released(asked, "one of two moves at once").await;
                }
                moves.whole().await;
            }
            // Someone joins while the region they join in is on its way to another
            // worker, and leaves while it is on its way to the next.
            3 => {
                let region = moves.region_at(0.5);
                let asking = moves.asking(region, None);
                let name = format!("Guest{round}");
                let address = moves.cluster.edge.0.clone();
                let joining = tokio::spawn(async move {
                    let mut guest = Bot::join(&address, &name).await?;
                    let count = view_area((0, 0), VIEW_DISTANCE).len();
                    guest.wait_for_chunks(count, PATIENCE).await?;
                    anyhow::Ok(guest)
                });
                let asked = moves.answer(asking).await;
                moves.released(&asked, "a move while someone joined").await;
                moves
                    .until("the guest has joined or failed to", |_| {
                        joining.is_finished()
                    })
                    .await;
                let guest = match joining.await.expect("joining does not panic") {
                    Ok(guest) => guest,
                    Err(error) => moves.fail(&format!(
                        "someone who joined while the region with the spawn point was \
                         moved was not let in: {error:#}"
                    )),
                };
                moves.note("a guest joined while the region was moved".to_owned());
                moves.whole().await;
                let asking = moves.asking(region, None);
                drop(guest);
                let asked = moves.answer(asking).await;
                moves.released(&asked, "a move while someone left").await;
                moves.note("the guest left while the region was moved".to_owned());
                moves.whole().await;
            }
            // A move, and at once another of the same region: its new owner has been
            // given it a moment ago and is opening or restoring it.
            _ => {
                let region = moves.random.below(2) as usize;
                let first = moves.owner(region).expect("the cluster was whole");
                let assigned = format!("a region was assigned region={region} ");
                let before = moves.coordinator_said(&assigned);
                let asking = moves.asking(region, None);
                // Looked at more often than anything else here: the new owner takes a
                // few hundredths of a second to restore the region.
                let waiting = Instant::now();
                while moves.coordinator_said(&assigned) == before {
                    if waiting.elapsed() > PATIENCE {
                        moves.fail("a region that was to be moved was not assigned");
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                let asking_again = moves.asking(region, None);
                let asked = moves.answer(asking).await;
                moves.released(&asked, "the first of two moves").await;
                let again = moves.answer(asking_again).await;
                let what = "a move of a region that was moved a moment before, while its new \
                            owner was still restoring it or had just done so,";
                moves.released(&again, what).await;
                let between = asked.new_owner().map(str::to_owned);
                let restoring = (0..moves.cluster.workers.len())
                    .filter(|worker| Some(worker_name(*worker)) == between)
                    .any(|worker| {
                        let log = moves.log_since(worker);
                        let last = log
                            .lines()
                            .rev()
                            .find(|line| line.contains("asked to release"));
                        last.is_some_and(|line| line.contains("while opening it"))
                    });
                if restoring {
                    while_restoring += 1;
                }
                moves.note(format!(
                    "region {region} went from {} to {between:?} and on to {:?}; the one in \
                     between was {} when it was asked to release",
                    worker_name(first),
                    again.new_owner(),
                    if restoring {
                        "still opening the region"
                    } else {
                        "running the region"
                    }
                ));
                moves.whole().await;
            }
        }
    }
    let (median, worst) = median_and_worst(&pauses);
    moves.note(format!(
        "{} moves as a bot stepped across the boundary: the pause was {} in the middle and \
         {} at worst; {while_restoring} moves found the owner still opening the region",
        pauses.len(),
        seconds(median),
        seconds(worst)
    ));
    played(&moves.finish(false).await);
}

/// A move that cannot be made is refused with a reason, the command ends with an
/// error, and nothing changes for the players: a move when no worker waits, to a
/// worker that runs a region, of a region that does not exist, and to a worker that
/// does not exist.
#[tokio::test(flavor = "multi_thread")]
async fn a_move_that_cannot_be_made_is_refused_with_a_reason_and_changes_nothing() {
    if a_repetition() {
        return;
    }
    // As many workers as regions: none waits.
    let full = Setup {
        workers: 2,
        ..SMALL
    };
    let mut moves = Moves::start("refusals", full).await;
    let routes = moves.routes();
    let busy = moves.owner(1).expect("the cluster is whole");
    let began = Instant::now();
    let asked_for: [(&str, Region, Option<String>); 4] = [
        ("with no worker waiting", 0, None),
        ("to a worker that runs a region", 0, Some(worker_name(busy))),
        ("of a region that does not exist", 7, None),
        (
            "to a worker that does not exist",
            0,
            Some("worker-nobody".to_owned()),
        ),
    ];
    for (what, region, to) in asked_for {
        let asking = moves.asking_by_name(region, to);
        let asked = moves.answer(asking).await;
        let outcome = asked.outcome();
        match asked.refusal() {
            Some(reason) if !reason.is_empty() && asked.said.trim().is_empty() => {
                moves.note(format!("a move {what} is refused: {reason}"));
            }
            _ => moves.fail(&format!(
                "a move {what} was not refused with a reason and an error: {outcome}"
            )),
        }
        if moves.routes() != routes {
            moves.fail(&format!(
                "a move {what} was refused and yet changed the routing table"
            ));
        }
        moves.served().await;
    }
    // Nobody was asked to release anything, and the bots noticed nothing.
    for worker in 0..moves.cluster.workers.len() {
        if moves.log_since(worker).contains("asked to release") {
            let name = worker_name(worker);
            moves.fail(&format!(
                "{name} was asked to release its region by a refused move"
            ));
        }
    }
    let longest = moves.longest_pause_since(began);
    moves.note(format!(
        "while moves were refused, a bot waited {} at most",
        seconds(longest)
    ));
    if moves.routes() != routes {
        moves.fail("the routing table changed after moves that were refused");
    }
    played(&moves.finish(false).await);
}

/// The worker that is releasing a region is killed just after the move was asked for.
/// However far it had got, the region is run by someone a lease and a moment later,
/// and nobody is disconnected.
#[tokio::test(flavor = "multi_thread")]
async fn players_keep_playing_when_the_old_owner_is_killed_in_the_middle_of_a_move() {
    if a_repetition() {
        return;
    }
    let mut moves = Moves::start("old owner killed", SMALL).await;
    for _ in 0..rounds(1) {
        let region = moves.random.below(2) as usize;
        let owner = moves.owner(region).expect("the cluster was whole");
        let began = Instant::now();
        let asking = moves.begin_move(region, None).await;
        // Somewhere in the release: its first checkpoint, the last ticks, the second
        // checkpoint, or just after it.
        let delay = Duration::from_millis(moves.random.below(40));
        tokio::time::sleep(delay).await;
        let why = format!("which was asked to release region {region} {delay:?} before");
        moves.kill_worker(owner, &why).await;
        let asked = moves.answer(asking).await;
        if !asked.released() && !asked.taken() {
            let outcome = asked.outcome();
            moves.fail(&format!(
                "the move of a region whose owner was killed did not end with the region \
                 being another worker's: {outcome}"
            ));
        }
        moves
            .whole_within(began, LEASE + MOMENT, "a move whose old owner was killed")
            .await;
        if moves.owner(region) == Some(owner) {
            moves.fail("the region is still routed to the worker that was killed");
        }
        moves.start_worker(owner);
        moves.registered(owner).await;
    }
    played(&moves.finish(false).await);
}

/// The worker that is to release a region stops dead, without its connections closing,
/// so that it never answers: the region is taken from it when the lease is over and
/// goes on with another worker. When the first wakes up, long after, it lets go of
/// the region without harm and without ending.
#[tokio::test(flavor = "multi_thread")]
async fn players_keep_playing_when_the_old_owner_never_answers_and_wakes_up_later() {
    if a_repetition() {
        return;
    }
    let mut moves = Moves::start("old owner frozen", SMALL).await;
    for _ in 0..rounds(1) {
        let region = moves.random.below(2) as usize;
        let owner = moves.owner(region).expect("the cluster was whole");
        let began = Instant::now();
        // Frozen before it can hear of the release, or somewhere in it.
        let asking = if moves.random.one_in(2) {
            let frozen =
                format!("which runs region {region} and is about to be asked for it: froze");
            moves.signal(owner, "STOP", &frozen).await;
            moves.asking(region, None)
        } else {
            let asking = moves.begin_move(region, None).await;
            let delay = Duration::from_millis(moves.random.below(40));
            tokio::time::sleep(delay).await;
            let frozen =
                format!("which was asked to release region {region} {delay:?} before: froze");
            moves.signal(owner, "STOP", &frozen).await;
            asking
        };
        let asked = moves.answer(asking).await;
        // Released, if the release was through before the worker froze; taken, if not.
        // If the worker's own lease ran out first, the coordinator may put it either
        // way, so only an error is wrong here.
        if !asked.released() && !asked.taken() {
            let outcome = asked.outcome();
            moves.fail(&format!(
                "the move of a region whose owner froze did not end with the region being \
                 another worker's: {outcome}"
            ));
        }
        moves
            .until("the region is another worker's", |moves| {
                moves.owner(region).is_some_and(|now| now != owner)
            })
            .await;
        moves
            .whole_within(began, LEASE + MOMENT, "a move whose old owner froze")
            .await;

        let registrations = moves.registrations(owner);
        let forgotten = moves.coordinator_said("the lease of a worker ran out") > 0;
        moves.signal(owner, "CONT", "woke up").await;
        // It hears from the coordinator, or from the store, that the region is not its
        // own any more, and lets go of it. If the coordinator gave up on it while it
        // was silent, it registers again. `tend` fails if it ends instead.
        if forgotten {
            moves
                .until("the worker that woke up has registered again", |moves| {
                    moves.registrations(owner) > registrations
                })
                .await;
            moves.note(format!("{} has registered again", worker_name(owner)));
        }
        moves.whole().await;
        if moves.owner(region) == Some(owner) {
            moves.fail("the region went back to the worker that woke up, though nobody moved it");
        }
        // And it is a worker like any other again: the region can be moved to it.
        moves.move_region(region, Some(owner)).await;
    }
    played(&moves.finish(false).await);
}

/// The worker a region is moved to is killed just after it was given the region, when
/// the old owner has let go already. The region is run by someone a lease and a moment
/// later.
#[tokio::test(flavor = "multi_thread")]
async fn players_keep_playing_when_the_new_owner_is_killed_just_after_a_move() {
    if a_repetition() {
        return;
    }
    let mut moves = Moves::start("new owner killed", SMALL).await;
    for _ in 0..rounds(1) {
        let region = moves.random.below(2) as usize;
        let target = moves.spares()[0];
        let asked = moves.ask(region, Some(target)).await;
        if !asked.released() {
            let outcome = asked.outcome();
            moves.fail(&format!("the move was not made: {outcome}"));
        }
        let moved = asked.new_epoch();
        // While it opens the region, restores it, or has just begun to run it.
        let delay = Duration::from_millis(moves.random.below(150));
        tokio::time::sleep(delay).await;
        let why = format!("which was given region {region} {delay:?} before");
        moves.kill_worker(target, &why).await;
        let killed = Instant::now();
        moves
            .until("the routing table has a third owner", |moves| {
                let epoch = moves.epoch(region);
                epoch > moved && moves.owner(region).is_some_and(|now| now != target)
            })
            .await;
        moves
            .whole_within(
                killed,
                LEASE + MOMENT,
                "the new owner of a region was killed",
            )
            .await;
        moves.start_worker(target);
        moves.registered(target).await;
    }
    played(&moves.finish(false).await);
}

/// The coordinator is killed in the middle of a move and another is started, which
/// knows nothing of the move; `rounds` times. Fails unless the cluster is whole, with
/// every bot served, within `limit` of the new coordinator being started, and the new
/// coordinator moves regions like the old one.
async fn the_coordinator_is_killed_in_the_middle_of_a_move(
    test: &str,
    rounds: u32,
    limit: Duration,
) {
    let mut moves = Moves::start(test, SMALL).await;
    for _ in 0..rounds {
        let region = moves.random.below(2) as usize;
        let asking = moves.begin_move(region, None).await;
        let delay = Duration::from_millis(moves.random.below(40));
        tokio::time::sleep(delay).await;
        moves.kill_coordinator().await;
        // The command is told how the move ended, or finds the coordinator gone.
        let asked = moves.answer(asking).await;
        if !asked.released() && asked.code == Some(0) {
            let outcome = asked.outcome();
            moves.fail(&format!(
                "`clustine move` ended without an error and without saying that the region \
                 was released: {outcome}"
            ));
        }
        moves.start_coordinator();
        let started = Instant::now();
        moves
            .whole_within(
                started,
                limit,
                "a coordinator was started in place of one killed in the middle of a move",
            )
            .await;
        moves.move_region(region, None).await;
    }
    played(&moves.finish(false).await);
}

/// When the coordinator is killed in the middle of a move and another is started,
/// nobody is disconnected, nothing is lost, the region is run by someone a lease and a
/// moment later, as after every failure in the middle of a move, and the new
/// coordinator moves it like the old one.
///
/// This failed when it was written: the edge found a new coordinator only by chance
/// while it was trying to link to a region, and the region's players stood still for
/// 15 seconds and more.
#[tokio::test(flavor = "multi_thread")]
async fn players_keep_playing_when_the_coordinator_is_killed_in_the_middle_of_a_move() {
    if a_repetition() {
        return;
    }
    let limit = LEASE + MOMENT;
    the_coordinator_is_killed_in_the_middle_of_a_move("coordinator killed", rounds(4), limit).await;
}

/// The same, held to what the record says of this failure: the worker that released
/// the region says so to the new coordinator, which gives the region to a worker at
/// once, whatever its grace period says, so that nobody waits for a lease (section 1,
/// step 4, and the sixth defect of the review).
#[tokio::test(flavor = "multi_thread")]
async fn a_region_released_while_the_coordinator_was_away_does_not_wait_for_a_lease() {
    if a_repetition() {
        return;
    }
    the_coordinator_is_killed_in_the_middle_of_a_move("coordinator away", rounds(2), LEASE).await;
}

/// The world store is killed in the middle of a move and started again at once. The
/// regions are run again a lease and a moment later, with everything players were
/// shown.
#[tokio::test(flavor = "multi_thread")]
async fn players_keep_playing_when_the_world_store_is_killed_in_the_middle_of_a_move() {
    if a_repetition() {
        return;
    }
    let mut moves = Moves::start("world store killed", SMALL).await;
    for _ in 0..rounds(3) {
        let region = moves.random.below(2) as usize;
        if moves.random.one_in(3) {
            // The store is gone before the move is asked for: the owner has found out
            // and is waiting to open the region again, which it gives up.
            let owner = moves.owner(region).expect("the cluster was whole");
            moves.kill_store().await;
            moves
                .until("the owner has noticed that the store is gone", |moves| {
                    moves.log_since(owner).contains("lost the world store")
                })
                .await;
            let asked = moves.ask(region, None).await;
            moves
                .released(
                    &asked,
                    "the move of a region whose owner had lost the store",
                )
                .await;
            moves.start_store();
            let started = Instant::now();
            let what = "the world store came back after a move without it";
            moves.whole_within(started, LEASE + MOMENT, what).await;
            continue;
        }
        let asking = moves.begin_move(region, None).await;
        // While the old owner checkpoints, or the new one opens and restores.
        let delay = Duration::from_millis(moves.random.below(80));
        tokio::time::sleep(delay).await;
        moves.kill_store().await;
        let killed = Instant::now();
        moves.start_store();
        let asked = moves.answer(asking).await;
        // With the store lost on the way the old owner skips what needs it and says
        // that it has released the region all the same.
        moves
            .released(&asked, "a move during which the world store was killed")
            .await;
        let what = "the world store was killed in a move";
        moves.whole_within(killed, LEASE + MOMENT, what).await;
    }
    played(&moves.finish(true).await);
}

/// A worker that takes part in a move is told to stop just after the move was asked
/// for: the one that is releasing the region, or the one it is released for. The move
/// is made or refused with a reason, the worker is gone within a few seconds without an
/// error, the region is run by another, and nobody waits as long as the lease.
#[tokio::test(flavor = "multi_thread")]
async fn players_keep_playing_when_a_worker_is_told_to_stop_in_the_middle_of_a_move() {
    if a_repetition() {
        return;
    }
    let mut moves = Moves::start("told to stop in a move", SMALL).await;
    for _ in 0..rounds(3) {
        let region = moves.random.below(2) as usize;
        let owner = moves.owner(region).expect("the cluster was whole");
        let target = moves.spares()[0];
        let coordinator_said = moves.cluster.log("coordinator").len();
        let began = Instant::now();
        let asking = moves.begin_move(region, Some(target)).await;
        let delay = Duration::from_millis(moves.random.below(60));
        tokio::time::sleep(delay).await;
        let (stopped, stopping) = if moves.random.one_in(2) {
            (owner, format!("which is releasing region {region}"))
        } else {
            (
                target,
                format!("for which region {region} is being released"),
            )
        };
        let told = Instant::now();
        let stopping = format!("{stopping} since {delay:?}: told to stop");
        moves.signal(stopped, "TERM", &stopping).await;
        let asked = moves.answer(asking).await;
        if asked.released() {
            moves.released(&asked, "the move").await;
        } else if asked.refusal().is_none() {
            let outcome = asked.outcome();
            moves.fail(&format!(
                "a move during which a worker was told to stop was neither made by the \
                 owner releasing the region nor refused with a reason: {outcome}"
            ));
        }
        moves.gone(stopped, told, A_FEW_SECONDS).await;
        moves
            .until("the region is run by a worker that stays", |moves| {
                moves.owner(region).is_some_and(|now| now != stopped) && moves.runs(region)
            })
            .await;
        moves.whole().await;
        moves.nobody_waited_for_the_lease(began, "a move with a worker told to stop");
        moves.no_lease_ran_out(coordinator_said, "a move with a worker told to stop");
        moves.start_worker(stopped);
        moves.registered(stopped).await;
    }
    played(&moves.finish(false).await);
}

/// A worker that runs a region is told to stop, as Kubernetes does it, while another
/// waits: it hands its region to that one and is gone within a few seconds, without an
/// error, and nobody waits as long as the lease. Started again under its name it is a
/// worker like any other, to which the next one that is told to stop hands over.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_that_is_told_to_stop_hands_its_region_over_first() {
    if a_repetition() {
        return;
    }
    let mut moves = Moves::start("told to stop", SMALL).await;
    for _ in 0..rounds(3) {
        let region = moves.region_at(moves.progress.bots()[0].x);
        let owner = moves.owner(region).expect("the cluster was whole");
        let spare = moves.spares()[0];
        let epoch = moves.epoch(region);
        let coordinator_said = moves.cluster.log("coordinator").len();
        let told = Instant::now();
        let stopping = format!("which runs region {region}: told to stop");
        moves.signal(owner, "TERM", &stopping).await;
        moves.gone(owner, told, A_FEW_SECONDS).await;
        moves
            .until("the routing table names another owner", |moves| {
                moves.epoch(region) != epoch
            })
            .await;
        moves.whole().await;
        if moves.owner(region) != Some(spare) {
            let now = moves.owner(region).map(worker_name);
            let spare = worker_name(spare);
            moves.fail(&format!(
                "the region of a worker that was told to stop is run by {now:?} and not by \
                 {spare}, which waited"
            ));
        }
        moves.nobody_waited_for_the_lease(told, "a worker being told to stop");
        moves.no_lease_ran_out(coordinator_said, "a worker being told to stop");
        // It comes back under its name, as a pod that is replaced does.
        moves.start_worker(owner);
        moves.registered(owner).await;
    }
    played(&moves.finish(false).await);
}

/// A worker that is told to stop while no other waits cannot hand its region over. It
/// goes on running it, and stops at once when it is told a second time; its region is
/// run again once a worker is there. With `CLUSTINE_MOVES_SOAK` it is also left alone
/// until it gives up by itself, which takes 20 seconds.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_that_is_told_to_stop_with_nobody_to_hand_over_to_goes_on_until_told_again() {
    if a_repetition() {
        return;
    }
    let full = Setup {
        workers: 2,
        ..SMALL
    };
    let mut moves = Moves::start("told to stop twice", full).await;
    let patient = if a_soak() {
        [false, true]
    } else {
        [false, false]
    };
    for left_alone in patient {
        let region = moves.region_at(moves.progress.bots()[0].x);
        let owner = moves.owner(region).expect("the cluster was whole");
        let told = Instant::now();
        let stopping = format!("which runs region {region}, with no worker waiting: told to stop");
        moves.signal(owner, "TERM", &stopping).await;
        moves
            .until("the worker has heard that it is to stop", |moves| {
                moves.log_since(owner).contains("told to stop")
            })
            .await;
        // It has nobody to hand over to, and so it plays on: twice over, the bots have
        // something acknowledged that they sent after the worker was told.
        moves.served().await;
        moves.served().await;
        moves.note("the bots are served by a worker that was told to stop".to_owned());
        if moves.owner(region) != Some(owner) {
            moves.fail("the region left a worker that had nobody to hand it to");
        }
        if left_alone {
            // It gives up by itself after 20 seconds, and not much sooner or later.
            let took = moves.gone(owner, told, LEAVE_WITHIN + MOMENT).await;
            if took < LEAVE_WITHIN - Duration::from_secs(1) {
                moves.fail(&format!(
                    "a worker with nobody to hand over to stopped by itself after {}",
                    seconds(took)
                ));
            }
        } else {
            let again = Instant::now();
            moves
                .signal(owner, "TERM", "told to stop a second time:")
                .await;
            moves.gone(owner, again, A_FEW_SECONDS).await;
        }
        // The region stands still until a worker is there: the same one under its name,
        // as a pod that is replaced.
        moves.start_worker(owner);
        moves.whole().await;
    }
    played(&moves.finish(false).await);
}

/// A worker is told to stop while no other waits, and a worker registers a moment
/// later: the region is handed to that one then, and the first is gone without an
/// error. The spare of a cluster may be the last to arrive.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_that_is_told_to_stop_hands_over_to_a_worker_that_arrives_later() {
    if a_repetition() {
        return;
    }
    let full = Setup {
        workers: 2,
        ..SMALL
    };
    let mut moves = Moves::start("spare arrives later", full).await;
    let region = moves.region_at(moves.progress.bots()[0].x);
    let owner = moves.owner(region).expect("the cluster was whole");
    let epoch = moves.epoch(region);
    let coordinator_said = moves.cluster.log("coordinator").len();
    let told = Instant::now();
    let stopping = format!("which runs region {region}, with no worker waiting: told to stop");
    moves.signal(owner, "TERM", &stopping).await;
    moves
        .until(
            "the coordinator has heard that the worker is leaving",
            |moves| moves.coordinator_said("a worker is leaving") > 0,
        )
        .await;
    moves.served().await;
    let late = moves.add_worker().await;
    let arrived = Instant::now();
    moves.gone(owner, arrived, A_FEW_SECONDS).await;
    moves
        .until("the routing table names another owner", |moves| {
            moves.epoch(region) != epoch
        })
        .await;
    moves.whole().await;
    if moves.owner(region) != Some(late) {
        let now = moves.owner(region).map(worker_name);
        moves.fail(&format!(
            "the region of the worker that left is run by {now:?}, not by the worker that \
             arrived"
        ));
    }
    moves.nobody_waited_for_the_lease(told, "a worker leaving for one that arrived later");
    moves.no_lease_ran_out(
        coordinator_said,
        "a worker leaving for one that arrived later",
    );
    played(&moves.finish(false).await);
}

/// Every worker of a cluster with one worker more than regions is replaced in turn, as
/// a rolling restart on Kubernetes does it: told to stop, waited for, started again
/// under its name, and the next one once it has registered. The bots are spread out at
/// the view distance a player usually has. Nobody is disconnected, the ledger holds,
/// nobody waits as long as the lease, and every region is run at the end by a process
/// that was started meanwhile.
#[tokio::test(flavor = "multi_thread")]
async fn players_keep_playing_while_every_worker_is_replaced_in_turn() {
    if a_repetition() {
        return;
    }
    let wide = Setup {
        wide: true,
        ..SMALL
    };
    let mut moves = Moves::start("rolling restart", wide).await;
    for _ in 0..rounds(1) {
        let coordinator_said = moves.cluster.log("coordinator").len();
        let began = Instant::now();
        let mut stops: Vec<Duration> = Vec::new();
        for worker in 0..moves.cluster.workers.len() {
            let runs: Vec<Region> = (0..=moves.lines.len())
                .filter(|region| moves.owner(*region) == Some(worker))
                .collect();
            let told = Instant::now();
            let stopping = format!("which runs {runs:?}: told to stop");
            moves.signal(worker, "TERM", &stopping).await;
            // A worker that is itself still restoring a region it was handed a moment
            // ago has that to give up first; none has the 20 seconds to wait out.
            stops.push(moves.gone(worker, told, LEAVE_WITHIN - MOMENT).await);
            moves.start_worker(worker);
            moves.registered(worker).await;
        }
        moves.whole().await;
        moves.nobody_waited_for_the_lease(began, "the replacement of every worker");
        moves.no_lease_ran_out(coordinator_said, "the replacement of every worker");
        let (median, worst) = median_and_worst(&stops);
        moves.note(format!(
            "every worker was replaced in {}; a worker took {} in the middle and {} at \
             worst to stop",
            seconds(began.elapsed()),
            seconds(median),
            seconds(worst)
        ));
        // `runs`, which `whole` waited for, goes by what a worker has logged since it
        // was last started, and every worker was started in this round.
        for region in 0..=moves.lines.len() {
            let owner = moves.owner(region).expect("the cluster is whole");
            moves.note(format!(
                "region {region} is run by {}, which was started during the replacement",
                worker_name(owner)
            ));
        }
    }
    played(&moves.finish(false).await);
}
