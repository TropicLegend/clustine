//! Regions are merged and split, with `clustine merge` and `clustine split`, under
//! players who keep playing and keep a ledger of what they were told (the ledger
//! scenario of the bots), on a cluster of real processes with the view distance a
//! player usually has. These are the scenarios E1 to E6 of
//! `docs/adr/0014-merging-and-splitting.md`, section 10: the first place where the
//! simulation, the runner, the coordinator, the worker process and the edge meet over a
//! merge and a split. That record and `docs/adr/0015-the-edge-through-merges-and-splits.md`
//! promise that nobody is disconnected by either, that nothing a player was shown is
//! lost, that every player is one entity throughout, and that a worker that dies in the
//! middle leaves the regions as they were before or as they are after, as a whole.
//! These tests fail if any of it does not hold.
//!
//! Where the bots walk: side by side, a few blocks apart, between block x = 33.5 and
//! x = 62.5, so that they see each other all the time and step across x = 48 again and
//! again. The world begins as two stripes that meet there, region 0 west with the chunk
//! players enter in, region 1 east. A split names the chunk east or west of x = 48 on
//! the bots' lanes, so the players standing in it go, and while somebody stands on the
//! other side the new region and the old one meet at x = 48 again: the bots walk
//! between the two. Two tests have the bots walk on from x = 20.5, through three
//! chunks, for three regions in a row.
//!
//! A split is asked for when bots stand well inside the chunks it names and beside
//! them, and takes whoever the region finds there a moment later; if that is nobody
//! after all, it is asked for again. A lease after a split the coordinator moves one
//! of the two regions to the other worker by itself, and whatever is asked for during
//! that move is turned away and asked for again, as whoever runs the commands by hand
//! would.
//!
//! How long players stand still is measured as in the tests of moves. **The pause of a
//! bot** at a merge or a split is the longest time that anything the bot sent waited
//! for its acknowledgement, among everything that was waiting at some moment between
//! the command being started and the cluster being whole again with every bot served;
//! every bot sends a pulse every other client tick for that. The pauses are given for
//! the bots that stood in a region that went on (the survivor of a merge, the region
//! that was split) and for those that came to another (the absorbed region's, the
//! part's), by where each bot stood when the command was started. One test spreads
//! the bots out so far that no two see the same chunks, as the test of moves does,
//! runs with no other cluster beside it, and moves the region as well, to compare.
//!
//! What these tests found in the server is at the end of the file, with the sequence,
//! what was to happen and what did, and a test that goes after it.
//!
//! What is done when follows from a seed, which every test prints. To run a seed again,
//! set `CLUSTINE_MERGES_SEED`, or `CLUSTINE_CHAOS_SEED` for the tests that kill;
//! things then come in the same order, though not at the same instants.
//! `CLUSTINE_MERGES_ROUNDS` and `CLUSTINE_CHAOS_KILLS` set how many rounds a test does.
//! `CLUSTINE_MERGES_KEEP` keeps the processes' logs of a test that passes; those of a
//! test that fails are always kept, and the failure says where.
//!
//! The processes are those of an unoptimised build, as in every test here, so the
//! pauses are longer than those of a server built for use.

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clustine_botswarm::ledger::LineCrossing;
use clustine_botswarm::{Bot, Ledger, LedgerReport, Progress, Random, Wait, audit_blocks, ledger};
use clustine_data::blocks;
use clustine_protocol::packets::play::face;
use clustine_rpc::RegionList;
use tempfile::TempDir;
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

use common::processes::{Asked, Cluster, Table, Turn, ask, turn, turn_alone, worker_name};

/// How long anything may take that is merely waited for. It only ever runs out when
/// something hangs.
const PATIENCE: Duration = Duration::from_secs(60);

/// How often a state that is waited for is looked at.
const LOOK: Duration = Duration::from_millis(20);

/// The moment beyond the leases that a cluster may take to be whole again after a
/// worker was killed: the new owner restores the regions, the edge links to them and
/// resumes, in an unoptimised build on a machine that does other things too.
const MOMENT: Duration = Duration::from_secs(5);

/// The longest that players may stand still when regions are merged or split, or a
/// region is moved, on a cluster that has the machine to itself. The records name no
/// bound for a merge or a split; they say that a merge costs the absorbed region's
/// players about what a move costs, for which the test of moves allows three seconds
/// with half as many players' chunks to send again as the one region has here. It is
/// well short of the lease, which is what players wait for when something has gone
/// wrong.
const LONGEST_PAUSE: Duration = Duration::from_secs(4);

/// The same on a cluster that shares the machine with the clusters of other tests,
/// which each take their share of it at moments nobody chooses: the lease.
const LONGEST_PAUSE_BESIDE_OTHERS: Duration = Duration::from_secs(5);

/// For how long an edge keeps a player whose region is silent
/// (`EdgeConfig::DEFAULT_REGION_PATIENCE`), which is what the edge of these tests has.
const EDGE_PATIENCE: Duration = Duration::from_secs(20);

/// How long the edge stands still when it does so for long: a good part of its
/// patience.
const LONG_STANDSTILL: Duration = Duration::from_secs(8);

/// Every this many client ticks each bot sends a pulse.
const PULSE: u32 = 2;

/// The view distance of the edge, in chunks: what a player usually has.
const VIEW: i32 = 8;

/// The first block east of the line the bots walk across: the boundary of the two
/// stripes, and of the chunks a split names.
const LINE: i32 = 48;

/// How far from the border of a chunk a bot stands "well inside" it, in blocks:
/// further than it walks while a command is started.
const WELL_INSIDE: f64 = 3.0;

/// The height players stand at in a flat world.
const GROUND: i32 = -60;

/// What a region is called on the command line and in the logs.
type Region = u32;

/// What a test's cluster and bots are like.
#[derive(Debug, Clone, Copy)]
struct Setup {
    /// The chunk x coordinates at which the world is divided at first.
    boundaries: &'static [i32],
    workers: usize,
    /// Whether the bots' lanes are so far apart that no two bots have the same chunks
    /// in view, and the cluster runs with no other beside it: for the pauses that are
    /// measured and held to the bound of a move. Otherwise the lanes are side by side
    /// and the bots see each other.
    apart: bool,
    /// The coordinator's lease in seconds, or `None` for the one it has when it is not
    /// told any, which is 5.
    lease: Option<u64>,
    /// The block x coordinates the bots walk between.
    west: f64,
    east: f64,
    /// The first block east of each line the bots say they step across: the borders of
    /// the chunks they walk through, which is where regions meet, at first or later.
    lines: &'static [i32],
    /// Whether what is done follows from `CLUSTINE_CHAOS_SEED` and is done
    /// `CLUSTINE_CHAOS_KILLS` times, as in the tests of chaos.
    chaos: bool,
}

/// Two stripes that meet at block x = 48 on two workers, the bots side by side and
/// walking a good way to either side of the line.
const STRIPES: Setup = Setup {
    boundaries: &[3],
    workers: 2,
    apart: false,
    lease: None,
    west: 33.5,
    east: 62.5,
    lines: &[LINE],
    chaos: false,
};

/// The same world with the bots walking further west, through three chunks: from
/// block x = 20.5, so that they also step across x = 32.
const LONG_LANES: Setup = Setup {
    west: 20.5,
    east: 60.5,
    lines: &[32, LINE],
    ..STRIPES
};

/// Whether this run of the tests is the one that repeats the end-to-end tests on a
/// world divided into regions, which `CLUSTINE_TEST_BOUNDARIES` asks for. These tests
/// divide their worlds themselves and take minutes, so they run once, in the run
/// without it.
fn a_repetition() -> bool {
    std::env::var_os("CLUSTINE_TEST_BOUNDARIES").is_some()
}

/// The seed of this run: the variable called `name` if set, else the clock.
fn seed(name: &str) -> u64 {
    match std::env::var(name) {
        Ok(seed) => seed
            .parse()
            .unwrap_or_else(|_| panic!("{name} is a number")),
        Err(_) => {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
            now.as_nanos() as u64 % 1_000_000
        }
    }
}

/// How often the workers write a checkpoint in a run with this seed: every second or
/// two, so that merges and splits fall into checkpoints, or practically never, so that
/// the checkpoint a merge or a split begins with has everything to save that was built
/// so far.
fn checkpoint_seconds(seed: u64) -> u64 {
    [1, 2, 300][(seed % 3) as usize]
}

/// How many rounds a test does: the variable called `name` if set, else `default`.
fn rounds_from(name: &str, default: u32) -> u32 {
    match std::env::var(name) {
        Ok(rounds) => rounds
            .parse()
            .unwrap_or_else(|_| panic!("{name} is a number")),
        Err(_) => default,
    }
}

/// How many rounds a test does: `CLUSTINE_MERGES_ROUNDS` if set, else `default`.
fn rounds(default: u32) -> u32 {
    rounds_from("CLUSTINE_MERGES_ROUNDS", default)
}

/// The middle one of `durations`, and the longest. Of none, nothing.
fn median_and_worst(durations: &[Duration]) -> Option<(Duration, Duration)> {
    let mut sorted = durations.to_vec();
    sorted.sort_unstable();
    Some((*sorted.get(sorted.len() / 2)?, *sorted.last()?))
}

/// Seconds with three decimals, for a message.
fn seconds(duration: Duration) -> String {
    format!("{:.3} s", duration.as_secs_f64())
}

/// A pause that is known only if a bot stood there, for a message.
fn perhaps(pause: Option<Duration>) -> String {
    pause.map_or(
        "no time that is known, as no bot stood there".to_owned(),
        seconds,
    )
}

/// The middle one and the longest of `durations`, for a message.
fn in_the_middle_and_at_worst(durations: &[Duration]) -> String {
    match median_and_worst(durations) {
        Some((median, worst)) => format!(
            "{} in the middle and {} at worst",
            seconds(median),
            seconds(worst)
        ),
        None => "not known, as no bot stood there".to_owned(),
    }
}

/// The x coordinate of the chunk that has the block column at `x`.
fn chunk_x(x: f64) -> i32 {
    (x.floor() as i32) >> 4
}

/// The chunks along the bots' lanes, by their x coordinates, of which the test keeps
/// track whose they are: more than the bots ever walk through.
const LANES: std::ops::RangeInclusive<i32> = 0..=7;

/// Which region each bot stands in, as far as the test knows: what the pauses of a
/// merge and of a split are told apart by.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Division {
    /// Bots that walk side by side, all in one row of chunks: the region that holds
    /// each chunk of that row, by the chunk's x coordinate.
    Lanes(BTreeMap<i32, Region>),
    /// Each bot by itself, in the order of the bots: with lanes far apart nobody walks
    /// from one region into another.
    Each(Vec<Region>),
}

impl Division {
    /// The stripes a world begins with when it is divided at the chunk x coordinates
    /// `boundaries`.
    fn stripes(boundaries: &[i32]) -> Self {
        let stripe = |chunk: i32| {
            let west = boundaries.iter().filter(|boundary| **boundary <= chunk);
            west.count() as Region
        };
        Self::Lanes(LANES.map(|chunk| (chunk, stripe(chunk))).collect())
    }

    /// The region the bot numbered `bot`, which is at the block x coordinate `x`, is in.
    fn region_of(&self, bot: usize, x: f64) -> Region {
        match self {
            Self::Lanes(chunks) => chunks[&chunk_x(x)],
            Self::Each(regions) => regions[bot],
        }
    }

    /// The division once `survivor` has absorbed `absorbed`.
    fn merged(&self, survivor: Region, absorbed: Region) -> Self {
        let now = |region: &Region| {
            if *region == absorbed {
                survivor
            } else {
                *region
            }
        };
        match self {
            Self::Lanes(chunks) => Self::Lanes(
                chunks
                    .iter()
                    .map(|(chunk, region)| (*chunk, now(region)))
                    .collect(),
            ),
            Self::Each(regions) => Self::Each(regions.iter().map(now).collect()),
        }
    }

    /// The division once `part` has been split off `region` and holds, of the chunks
    /// that were the region's, those from the chunk x coordinate `from` to `to`.
    fn split(&self, region: Region, part: Region, from: i32, to: i32) -> Self {
        let Self::Lanes(chunks) = self else {
            return self.clone();
        };
        let now = |(chunk, held): (&i32, &Region)| {
            let goes = *held == region && (from..=to).contains(chunk);
            (*chunk, if goes { part } else { *held })
        };
        Self::Lanes(chunks.iter().map(now).collect())
    }
}

/// What the bots noticed of a merge, a split or a move.
#[derive(Debug, Clone)]
struct Noticed {
    /// What was done, for a message.
    what: String,
    /// What the command gave as its own time, from asking to the coordinator's answer.
    own_time: Duration,
    /// The longest pause of any bot.
    pause: Duration,
    /// The longest pause among the bots that stood in a region that went on as it was
    /// (the survivor of a merge, the region that was split, any region that was not
    /// moved), and among those whose region was absorbed, split off or moved. `None`
    /// where no bot stood.
    stayed: Option<Duration>,
    went: Option<Duration>,
    /// Each bot's pause, for a message.
    bots: String,
}

/// The pauses of several merges, splits or moves, in a line: of those who stayed, of
/// those who went, and what the command took. `stayed` and `went` say who those are.
fn summary(kind: &str, noticed: &[Noticed], stayed: &str, went: &str) -> String {
    let of = |pick: fn(&Noticed) -> Option<Duration>| -> Vec<Duration> {
        noticed.iter().filter_map(pick).collect()
    };
    let own: Vec<Duration> = noticed.iter().map(|noticed| noticed.own_time).collect();
    format!(
        "{} {kind}: {stayed} stood still for {}; {went} for {}; the command took {}",
        noticed.len(),
        in_the_middle_and_at_worst(&of(|noticed| noticed.stayed)),
        in_the_middle_and_at_worst(&of(|noticed| noticed.went)),
        in_the_middle_and_at_worst(&own),
    )
}

/// A cluster with bots playing the ledger scenario on it, and the means to merge,
/// split and move its regions and to do harm to its processes.
struct Merges {
    /// Its leave to run, beside the other tests' clusters or alone.
    _turn: Turn,
    /// Where the world and the logs are; taken out when they are to be kept.
    directory: Option<TempDir>,
    cluster: Cluster,
    test: String,
    seed: u64,
    random: Random,
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
    /// Which region each bot stands in, for the tests that tell the bots' pauses apart
    /// by it. The others leave it as it began.
    division: Division,
    /// The longest any bot may stand still here.
    longest_pause: Duration,
}

impl Merges {
    /// Starts a cluster and bots as `setup` says. Returns once every bot is on its lane
    /// and has been acknowledged, every region runs, and the workers share the regions
    /// evenly, so that the coordinator moves nothing by itself for now.
    async fn start(test: &str, setup: Setup) -> Self {
        // The cluster whose pauses are held to the bound of a move has the machine to
        // itself, as the one of the moves has.
        let turn = if setup.apart {
            turn_alone().await
        } else {
            turn().await
        };
        let seed = if setup.chaos {
            seed("CLUSTINE_CHAOS_SEED")
        } else {
            seed("CLUSTINE_MERGES_SEED")
        };
        let variable = if setup.chaos { "CHAOS" } else { "MERGES" };
        println!("{test}: seed {seed} (set CLUSTINE_{variable}_SEED={seed} to run it again)");
        let directory = tempfile::Builder::new()
            .prefix("clustine-merges-")
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
            cluster.lease_seconds = setup.lease;
            cluster.view_distance = VIEW;
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

        let mut scenario = Ledger {
            bots: 4,
            rounds: None,
            duration: None,
            west: setup.west,
            east: setup.east,
            lines: setup.lines.to_vec(),
            seed,
            pulse: Some(PULSE),
            ..Ledger::default()
        };
        // Lanes side by side are still a walk from where the bots join, and the auditor
        // walks all of it again.
        scenario.to_the_lane = 3.0;
        if setup.apart {
            // Twenty chunks from lane to lane, which is as far as two players with this
            // view distance see between them. That far the bots run.
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
        let division = Division::stripes(setup.boundaries);
        let longest_pause = if setup.apart {
            LONGEST_PAUSE
        } else {
            LONGEST_PAUSE_BESIDE_OTHERS
        };
        let mut merges = Self {
            _turn: turn,
            directory: Some(directory),
            cluster,
            test: test.to_owned(),
            seed,
            random: Random::new(seed),
            ledger: scenario,
            progress,
            scenario: Some(playing),
            started: Instant::now(),
            deeds: Vec::new(),
            since: (0..setup.workers).map(|worker| (worker, 0)).collect(),
            coordinator_since: 0,
            division,
            longest_pause,
        };
        merges
            .until("every bot is on its lane and acknowledged", |merges| {
                let bots = merges.progress.bots();
                bots.iter().all(|bot| bot.playing && bot.acknowledged > 0)
            })
            .await;
        merges.settled().await;
        merges
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
        println!("{}: {deed}", self.test);
        self.deeds.push(deed);
    }

    /// Fails the test, saying what led up to it and where the logs are, which are kept.
    fn fail(&mut self, message: &str) -> ! {
        self.note("failed".to_owned());
        let kept = self.keep();
        let mut report = format!(
            "{}: {message}\n\nseed {}; what was done:\n",
            self.test, self.seed
        );
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
            self.table()
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

    /// Fails if a process has ended that nobody killed, or the bots have. No worker
    /// ends by itself over a merge or a split, whatever comes of it.
    async fn tend(&mut self) {
        let mut ended = None;
        for (name, process) in self.cluster.processes() {
            let gone = process.as_mut().and_then(|child| child.try_wait().unwrap());
            if let Some(status) = gone {
                ended = Some(format!("{name} ended by itself ({status})"));
            }
        }
        if let Some(ended) = ended {
            self.fail(&ended);
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

    /// The last routing table the coordinator logged in its present life.
    fn table(&self) -> Option<Table> {
        self.cluster.table(self.coordinator_since)
    }

    /// The owner of every region that has one, as the coordinator last logged its
    /// routing table: the address of the worker and the epoch.
    fn routes(&self) -> BTreeMap<Region, (String, u64)> {
        self.table().map(|table| table.routes()).unwrap_or_default()
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

    /// The regions each worker runs according to the routing table.
    fn loads(&self) -> Vec<Vec<Region>> {
        let mut loads = vec![Vec::new(); self.cluster.workers.len()];
        for (region, (address, _)) in self.routes() {
            if let Some(worker) = self.worker_at(&address) {
                loads[worker].push(region);
            }
        }
        loads
    }

    /// What a worker has logged in its present life.
    fn log_since(&self, worker: usize) -> String {
        let log = self.cluster.log(&worker_name(worker));
        log.get(self.since[&worker]..)
            .unwrap_or_default()
            .to_owned()
    }

    /// How many times a worker has logged `words`, in all its lives.
    fn said(&self, worker: usize, words: &str) -> usize {
        self.cluster
            .log(&worker_name(worker))
            .matches(words)
            .count()
    }

    /// Whether `region` has an owner that is alive, has said in its present life that
    /// it runs the region with the epoch the routing table names, and is the one the
    /// edge last linked to for that region.
    fn runs(&self, region: Region) -> bool {
        let Some((address, epoch)) = self.routes().remove(&region) else {
            return false;
        };
        let Some(worker) = self.worker_at(&address) else {
            return false;
        };
        let running = format!("running a region region={region} epoch={epoch} ");
        let linked = format!("linked to a region region={region} epoch=");
        let edge = self.cluster.log("edge");
        let last_link = edge.lines().rev().find(|line| line.contains(&linked));
        self.cluster.workers[worker].1.is_some()
            && self.log_since(worker).contains(&running)
            && last_link.is_some_and(|line| line.contains(&format!("{linked}{epoch} ")))
    }

    /// Whether the loads of the workers that are there differ by one at most: then
    /// the coordinator has evened out what it would, and begins no release of its own.
    fn even(&self) -> bool {
        let loads = self.loads();
        let alive = (0..loads.len()).filter(|worker| self.cluster.workers[*worker].1.is_some());
        let counts: Vec<usize> = alive.map(|worker| loads[worker].len()).collect();
        let ends = counts.iter().max().zip(counts.iter().min());
        ends.is_some_and(|(most, fewest)| most - fewest <= 1)
    }

    /// The living regions of `list`, in ascending order.
    fn living(list: &RegionList) -> Vec<Region> {
        list.regions.iter().map(|info| info.region.0).collect()
    }

    /// The regions of the world as the world store has them, which is what decides.
    async fn list(&mut self) -> RegionList {
        match self.cluster.regions().await {
            Ok(list) => list,
            Err(error) => self.fail(&format!("the world store's list cannot be read: {error}")),
        }
    }

    /// Waits until every region the world store's list has is run by a worker the edge
    /// is linked to, the coordinator knows of no other region, and `more` holds as
    /// well. Returns the list.
    ///
    /// The list is read anew at every look: what a worker asked of the store before it
    /// was killed, the store may do a moment after the test first looked.
    async fn runs_and(&mut self, what: &str, more: impl Fn(&Self) -> bool) -> RegionList {
        let waiting = Instant::now();
        loop {
            self.tend().await;
            let list = self.cluster.regions().await;
            if let Ok(list) = &list {
                let living = Self::living(list);
                let known = self.table().map(|table| table.known());
                if known.as_ref() == Some(&living)
                    && living.iter().all(|region| self.runs(*region))
                    && more(self)
                {
                    return list.clone();
                }
            }
            if waiting.elapsed() > PATIENCE {
                self.fail(&format!(
                    "waited {PATIENCE:?} in vain until {what}; the list is {list:?}"
                ));
            }
            tokio::time::sleep(LOOK).await;
        }
    }

    /// Waits until every region the list has is run by a worker the edge is linked to.
    async fn everything_runs(&mut self) -> RegionList {
        let what = "every region of the store's list runs on a worker the edge is linked to";
        self.runs_and(what, |_| true).await
    }

    /// Waits until the cluster is whole: every region runs, on a worker the edge is
    /// linked to, and every bot has had something acknowledged that it sent after that
    /// was so. Returns the list of regions.
    async fn whole(&mut self) -> RegionList {
        let list = self.everything_runs().await;
        self.served().await;
        self.note(format!(
            "the cluster is whole, with the regions {:?}",
            Self::living(&list)
        ));
        list
    }

    /// Waits until the cluster is whole and the workers share the regions evenly, so
    /// that the coordinator begins no move of its own from here on.
    async fn settled(&mut self) -> RegionList {
        let what = "every region runs and the workers share them evenly";
        let list = self.runs_and(what, Self::even).await;
        self.served().await;
        self.note(format!(
            "the cluster is whole, with the regions {:?} shared as {:?}",
            Self::living(&list),
            self.loads()
        ));
        list
    }

    /// Waits until every bot has had something acknowledged that it sends from now on.
    async fn served(&mut self) {
        let sent: Vec<i32> = self.progress.bots().iter().map(|bot| bot.sent).collect();
        self.until("every bot has something new acknowledged", |merges| {
            let bots = merges.progress.bots();
            bots.iter()
                .zip(&sent)
                .all(|(bot, sent)| bot.acknowledged > *sent)
        })
        .await;
    }

    /// Waits until a worker that was started has registered with the coordinator,
    /// whether or not it was given a region.
    async fn registered(&mut self, worker: usize) {
        self.until("a worker that was started has registered", |merges| {
            let log = merges.log_since(worker);
            log.contains("waiting to be given a region") || log.contains("given a region")
        })
        .await;
        self.note(format!("{} has registered", worker_name(worker)));
    }

    /// The block x coordinate of every bot.
    fn xs(&self) -> Vec<f64> {
        self.progress.bots().iter().map(|bot| bot.x).collect()
    }

    /// Where the bots are, for a message.
    fn whereabouts(&self) -> String {
        let bots: Vec<String> = self.xs().iter().map(|x| format!("{x:.1}")).collect();
        format!("the bots are at x = {}", bots.join(", "))
    }

    /// The chunk the bot numbered `bot` stands in when it is at the block x coordinate
    /// `x` of its lane.
    fn chunk_of(&self, bot: usize, x: f64) -> (i32, i32) {
        let lane = self.ledger.first_lane + bot as i32 * self.ledger.lane_spacing;
        (chunk_x(x), lane >> 4)
    }

    /// Waits until the bots are where `placed` wants them, by their x coordinates.
    async fn until_the_bots(&mut self, what: &str, placed: impl Fn(&[f64]) -> bool) {
        self.until(what, |merges| placed(&merges.xs())).await;
    }

    /// Waits until, of the bots that stand in `region`, one stands well inside each of
    /// the chunks `named`, which are side by side on the bots' lanes, and one well
    /// outside all of them: then a split of the region that names those chunks has
    /// players who go, from each of them, and players who stay.
    async fn until_some_would_go_and_some_stay(&mut self, region: Region, named: &[(i32, i32)]) {
        let inside = |x: f64, chunk: i32| {
            let west = f64::from(chunk * 16);
            (west + WELL_INSIDE..west + 16.0 - WELL_INSIDE).contains(&x)
        };
        let outside = |x: f64, chunk: i32| {
            let west = f64::from(chunk * 16);
            x < west - WELL_INSIDE || x >= west + 16.0 + WELL_INSIDE
        };
        let what = format!(
            "of the bots in region {region}, one stands well inside each of the chunks \
             {named:?} and one well outside them"
        );
        self.until(&what, |merges| {
            let xs: Vec<f64> = merges.xs();
            let bots = xs.iter().enumerate();
            let there: Vec<f64> = bots
                .filter(|(bot, x)| merges.division.region_of(*bot, **x) == region)
                .map(|(_, x)| *x)
                .collect();
            named
                .iter()
                .all(|(chunk, _)| there.iter().any(|x| inside(*x, *chunk)))
                && there
                    .iter()
                    .any(|x| named.iter().all(|(chunk, _)| outside(*x, *chunk)))
        })
        .await;
    }

    /// Waits for the next step of a bot across one of the lines the bots were told of,
    /// which the bot announces just before it sends it.
    async fn next_crossing(&mut self) -> LineCrossing {
        let mut crossings = self.progress.crossings();
        crossings.mark_unchanged();
        self.tend().await;
        let changed = tokio::time::timeout(PATIENCE, crossings.changed()).await;
        match changed {
            Ok(Ok(())) => crossings.borrow_and_update().expect("a crossing"),
            _ => self.fail("no bot stepped across the line any more"),
        }
    }

    /// Lets the bots play on until `steps` more of them have stepped across a line and
    /// everyone has been served after each.
    async fn played_on(&mut self, steps: u32) {
        for _ in 0..steps {
            self.next_crossing().await;
            self.served().await;
        }
        self.note(format!(
            "the bots stepped across x = {:?} {steps} more times and were served",
            self.ledger.lines
        ));
    }

    /// Runs `command`, which asks the coordinator for `what`, without waiting for what
    /// comes of it.
    fn asking(&mut self, what: String, command: Command) -> JoinHandle<Asked> {
        self.note(format!("asked for {what}; {}", self.whereabouts()));
        ask(what, command)
    }

    /// Waits for what a command comes to.
    async fn answer(&mut self, asking: JoinHandle<Asked>) -> Asked {
        self.until("the command has ended", |_| asking.is_finished())
            .await;
        let asked = asking.await.expect("asking does not panic");
        self.note(format!("{}: {}", asked.what, asked.outcome()));
        asked
    }

    /// `clustine merge`, without waiting for what comes of it.
    fn merging(&mut self, survivor: Region, absorbed: Region) -> JoinHandle<Asked> {
        let command = self.cluster.merge_command(survivor, absorbed);
        let what = format!("region {survivor} to absorb region {absorbed}");
        self.asking(what, command)
    }

    /// `clustine split`, without waiting for what comes of it.
    fn splitting(&mut self, region: Region, chunks: &[(i32, i32)]) -> JoinHandle<Asked> {
        let command = self.cluster.split_command(region, chunks);
        let what = format!("region {region} to be split at the chunks {chunks:?}");
        self.asking(what, command)
    }

    /// Whether a command was turned away because the coordinator is moving one of its
    /// regions to another worker just now, to even the regions out: that is over in a
    /// moment, and whoever asked asks again.
    fn in_the_way_of_a_move(asked: &Asked) -> bool {
        asked.was_told_no_because("is being released")
    }

    /// Runs the command that `command` makes, anew for as long as it is turned away for
    /// a move of the coordinator's own. Returns what it came to and when it was started.
    async fn ask_until_heard(
        &mut self,
        what: &str,
        command: impl Fn(&Self) -> Command,
    ) -> (Asked, Instant) {
        let waiting = Instant::now();
        loop {
            let table = self.table();
            let before = Instant::now();
            let asking = self.asking(what.to_owned(), command(self));
            let asked = self.answer(asking).await;
            if !Self::in_the_way_of_a_move(&asked) {
                return (asked, before);
            }
            if waiting.elapsed() > PATIENCE {
                self.fail(&format!(
                    "{what} was turned away for a move for {PATIENCE:?}"
                ));
            }
            // The move ends with the region assigned, which the routing table shows.
            self.until("the coordinator's own move is over", |merges| {
                merges.table() != table
            })
            .await;
        }
    }

    /// `clustine merge`, and what comes of it and when it was started.
    async fn merge(&mut self, survivor: Region, absorbed: Region) -> (Asked, Instant) {
        let what = format!("region {survivor} to absorb region {absorbed}");
        self.ask_until_heard(&what, |merges| {
            merges.cluster.merge_command(survivor, absorbed)
        })
        .await
    }

    /// `clustine split`, and what comes of it and when it was started.
    async fn split(&mut self, region: Region, chunks: &[(i32, i32)]) -> (Asked, Instant) {
        let what = format!("region {region} to be split at the chunks {chunks:?}");
        self.ask_until_heard(&what, |merges| merges.cluster.split_command(region, chunks))
            .await
    }

    /// A worker that is running and does not run `region`, if there is one.
    fn another_worker(&self, region: Region) -> Option<usize> {
        let owner = self.owner(region);
        let workers = 0..self.cluster.workers.len();
        let mut others = workers.filter(|worker| Some(*worker) != owner);
        others.find(|worker| self.cluster.workers[*worker].1.is_some())
    }

    /// Fails unless the command says that `survivor` has absorbed `absorbed`.
    fn has_absorbed(&mut self, asked: &Asked, survivor: Region, absorbed: Region) {
        if !asked.says(&format!("region {survivor} has absorbed region {absorbed}")) {
            let outcome = asked.outcome();
            self.fail(&format!(
                "region {survivor} did not absorb region {absorbed}: {outcome}"
            ));
        }
    }

    /// The region the command says was split off `region`, if it says so.
    fn split_off(asked: &Asked, region: Region) -> Option<Region> {
        let done = asked.says(&format!("has been split off region {region}"));
        let part = asked.said.split_whitespace().nth(1)?.parse().ok()?;
        done.then_some(part)
    }

    /// Whether a split was told that nobody stands in the chunks it named, or that
    /// nobody stands anywhere else in a region that would then hold nothing: the bots
    /// walk, and the region looks where they are a moment after the test did.
    fn found_nobody(asked: &Asked) -> bool {
        asked.was_told_no_because("no player stands in a chunk named that the region holds")
            || asked.was_told_no_because("nobody would stay, and the region would hold nothing")
    }

    /// What the bots noticed of something that was begun at `before` and has left the
    /// cluster whole by now. `went` says of each bot whether it stood in what was
    /// absorbed, split off or moved. Fails if a bot stood still for longer than it may.
    fn noticed(&mut self, what: &str, asked: &Asked, before: Instant, went: &[bool]) -> Noticed {
        let after = Instant::now();
        let Some(own_time) = asked.own_time() else {
            let outcome = asked.outcome();
            self.fail(&format!(
                "the command did not say how long it took: {outcome}"
            ));
        };
        let pauses: Vec<Option<Wait>> = self.progress.longest_waits(before, after);
        let lasted = |wait: &Option<Wait>| wait.map(|wait| wait.lasted(after));
        let longest = |of_those_who_went: bool| {
            let those = pauses.iter().zip(went);
            let those = those.filter(|(_, went)| **went == of_those_who_went);
            those.filter_map(|(wait, _)| lasted(wait)).max()
        };
        let bots: Vec<String> = pauses
            .iter()
            .zip(went)
            .map(|(wait, went)| {
                let which = if *went { "went" } else { "stayed" };
                match lasted(wait) {
                    Some(pause) => format!("{} {which}", seconds(pause)),
                    None => format!("nothing sent {which}"),
                }
            })
            .collect();
        let noticed = Noticed {
            what: what.to_owned(),
            own_time,
            pause: pauses.iter().filter_map(lasted).max().unwrap_or_default(),
            stayed: longest(false),
            went: longest(true),
            bots: bots.join(", "),
        };
        self.note(format!(
            "{what}: the bots stood still for {} at most ({}); the command gives {} ms",
            seconds(noticed.pause),
            noticed.bots,
            own_time.as_millis()
        ));
        if noticed.pause > self.longest_pause {
            let longest_pause = self.longest_pause;
            self.fail(&format!(
                "the bots stood still for {} at {what}; they may for {longest_pause:?}",
                seconds(noticed.pause)
            ));
        }
        noticed
    }

    /// The longest pause of any bot between `from` and now.
    fn longest_pause_since(&self, from: Instant) -> Duration {
        let now = Instant::now();
        let pauses = self.progress.longest_waits(from, now);
        let lasted = pauses.iter().flatten().map(|wait| wait.lasted(now));
        lasted.max().unwrap_or_default()
    }

    /// Of each bot, whether it stands in `region` as far as the test knows.
    fn stand_in(&self, region: Region) -> Vec<bool> {
        let xs = self.xs();
        let bots = xs.iter().enumerate();
        bots.map(|(bot, x)| self.division.region_of(bot, *x) == region)
            .collect()
    }

    /// Has `survivor` absorb `absorbed`, waits for the cluster to be whole and returns
    /// what the bots noticed. Fails unless the merge is made and the world store's list
    /// has it.
    async fn merge_and_notice(&mut self, survivor: Region, absorbed: Region) -> Noticed {
        let went = self.stand_in(absorbed);
        let (asked, before) = self.merge(survivor, absorbed).await;
        self.has_absorbed(&asked, survivor, absorbed);
        self.division = self.division.merged(survivor, absorbed);
        let list = self.whole().await;
        let gone = list
            .absorbed
            .iter()
            .any(|(gone, into)| (gone.0, into.0) == (absorbed, survivor));
        if Self::living(&list).contains(&absorbed) || !gone {
            self.fail(&format!(
                "after the merge the list does not have region {absorbed} as absorbed by \
                 region {survivor}: {list:?}"
            ));
        }
        let what = format!("the merge of region {absorbed} into region {survivor}");
        self.noticed(&what, &asked, before, &went)
    }

    /// Splits the players standing in the chunks `named` off `region`, on the lanes of
    /// bots that walk side by side, at a moment at which bots of the region stand in
    /// each of those chunks and beside them; waits for the cluster to be whole and
    /// returns the new region and what the bots noticed. Fails unless the split is
    /// made, the list has the new region, and the worker that split the region runs
    /// the new one.
    async fn split_and_notice(
        &mut self,
        region: Region,
        named: &[(i32, i32)],
    ) -> (Region, Noticed) {
        let (asked, before, went, part) = self.split_where_bots_stand(region, named).await;
        let owner = self.owner(region);
        let list = self.whole().await;
        self.division = self.divided(&list, region, part);
        if !Self::living(&list).contains(&part) || list.next.0 <= part {
            self.fail(&format!(
                "after the split the list does not have region {part}: {list:?}"
            ));
        }
        // Unless the coordinator has moved one of the two since, a lease later.
        if self.owner(part) != owner && self.owner(region) == owner {
            let loads = self.loads();
            self.fail(&format!(
                "the new region is not run by the worker that split it off: {loads:?}"
            ));
        }
        let what = format!("the split of region {part} off region {region}");
        let noticed = self.noticed(&what, &asked, before, &went);
        (part, noticed)
    }

    /// Splits the players standing in the chunks `named` off `region` as
    /// [`Merges::split_and_notice`] does, where `named` are the easternmost chunks of
    /// the region that the bots walk through, and sees to it that the new region is
    /// those chunks of the lanes and what lies east of them. That is so when the
    /// region found players in the westernmost chunk named and in the chunk west of
    /// it. If it found them a step further instead, the regions meet a chunk further
    /// west or east; then the part is merged back and the region split again. Fails
    /// if that does not come about in a few attempts.
    async fn split_east_of(&mut self, region: Region, named: &[(i32, i32)]) -> (Region, Noticed) {
        let from = named.iter().map(|(chunk, _)| *chunk).min();
        let from = from.expect("a split names a chunk");
        let before = self.division.clone();
        for _ in 0..5 {
            let (part, noticed) = self.split_and_notice(region, named).await;
            if self.division == before.split(region, part, from, i32::MAX) {
                return (part, noticed);
            }
            self.note(format!(
                "region {region} and region {part} do not meet at x = {}, as a bot was a \
                 step further than where the test saw it; merging back to split again",
                from * 16
            ));
            self.merge_and_notice(region, part).await;
        }
        self.fail(&format!(
            "region {region} was split five times with bots well inside the chunks {named:?} \
             and west of them, and never at x = {}",
            from * 16
        ));
    }

    /// Splits the players standing in the chunks `named` off `region` at a moment at
    /// which bots of the region stand in each of those chunks and beside them, and
    /// again if the region found nobody there after all. Returns what the command
    /// came to, when it was started, which bots stood in the chunks then, and the new
    /// region. Fails unless the split is made.
    async fn split_where_bots_stand(
        &mut self,
        region: Region,
        named: &[(i32, i32)],
    ) -> (Asked, Instant, Vec<bool>, Region) {
        let waiting = Instant::now();
        loop {
            self.until_some_would_go_and_some_stay(region, named).await;
            let there = self.stand_in(region);
            let xs = self.xs();
            let bots = xs.iter().zip(there);
            let went: Vec<bool> = bots
                .map(|(x, there)| there && named.contains(&(chunk_x(*x), 0)))
                .collect();
            let (asked, before) = self.split(region, named).await;
            if let Some(part) = Self::split_off(&asked, region) {
                return (asked, before, went, part);
            }
            if !Self::found_nobody(&asked) || waiting.elapsed() > PATIENCE {
                let outcome = asked.outcome();
                self.fail(&format!("region {region} was not split: {outcome}"));
            }
        }
    }

    /// Which region each bot is in after `part` was split off `region`: by the box of
    /// chunks the list says the part was granted, which along the bots' lanes are the
    /// chunks from its western end to its eastern end. Where it ends follows from
    /// where the players stood who stayed.
    fn divided(&mut self, list: &RegionList, region: Region, part: Region) -> Division {
        let info = list.regions.iter().find(|info| info.region.0 == part);
        let Some(bounds) = info.and_then(|info| info.bounds) else {
            self.fail(&format!(
                "the list does not say which chunks region {part} was granted: {list:?}"
            ));
        };
        let (from, to) = (bounds.min.x, bounds.max.x);
        let division = self.division.split(region, part, from, to);
        self.note(format!(
            "region {part} was granted the chunks from {:?} to {:?}; along the lanes the \
             regions are {division:?}",
            (from, bounds.min.z),
            (to, bounds.max.z)
        ));
        division
    }

    /// How many bots stand in each of `first` and `second`, as far as the test knows.
    fn bots_in(&self, first: Region, second: Region) -> (usize, usize) {
        let count = |region| self.stand_in(region).iter().filter(|there| **there).count();
        (count(first), count(second))
    }

    /// Moves `region` with `clustine move` to a worker that does not run it, waits for
    /// the cluster to be whole and returns what the bots noticed. Fails unless the
    /// command says that the owner released the region.
    async fn move_and_notice(&mut self, region: Region) -> Noticed {
        let went = self.stand_in(region);
        let what = format!("region {region} to be moved to another worker");
        let (asked, before) = self
            .ask_until_heard(&what, |merges| {
                // Named when the command is made: the coordinator may have moved the
                // region itself since this was last asked.
                let to = merges.another_worker(region).map(worker_name);
                merges.cluster.move_command(region as usize, to.as_deref())
            })
            .await;
        let epoch = asked
            .said
            .split_once(" with epoch ")
            .and_then(|(_, rest)| rest.split_whitespace().next()?.parse::<u64>().ok());
        let Some(epoch) = epoch.filter(|_| asked.says("released by its owner")) else {
            let outcome = asked.outcome();
            self.fail(&format!(
                "region {region} was not moved by its owner releasing it: {outcome}"
            ));
        };
        // The command has its answer a moment before the table is logged, and whoever
        // waits for the cluster to be whole has to look at the table after the move.
        self.until(
            "the routing table has the region as it was moved",
            |merges| {
                let now = merges.routes().remove(&region);
                now.is_some_and(|(_, now)| now >= epoch)
            },
        )
        .await;
        self.whole().await;
        let what = format!("the move of region {region}");
        self.noticed(&what, &asked, before, &went)
    }

    /// Sends a process a signal.
    async fn signal(&mut self, name: &str, process: Option<u32>, signal: &str, what: &str) {
        let pid = process.unwrap_or_else(|| panic!("{name} is not running"));
        let sent = Command::new("kill")
            .args([format!("-{signal}"), pid.to_string()])
            .status()
            .await;
        assert!(sent.unwrap().success());
        self.note(format!("{what} {name}; {}", self.whereabouts()));
    }

    /// Stops the edge where it is, without it or its connections noticing, or lets it
    /// carry on.
    async fn signal_edge(&mut self, signal: &str, what: &str) {
        let pid = self.cluster.edge.1.as_ref().and_then(Child::id);
        self.signal("the edge", pid, signal, what).await;
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

    /// Tells the bots to stop, and fails unless they and an auditor who joins then find
    /// everything as the ledgers say. With `from_disk`, every process is then killed
    /// and started again, and an auditor has to find the same. Then the processes are
    /// asked to stop, which each has to do cleanly.
    async fn finish(mut self, from_disk: bool) -> LedgerReport {
        // A test may be over before the bots have walked as far as the line, and
        // stepping across it is part of what they are to have done.
        let crossings = self.progress.crossings();
        self.until("a bot has stepped across the line", |_| {
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
            // say is what the processes that are started on the same disk have, with
            // the regions as the merges and splits left them.
            let before = self.list().await;
            self.cluster.kill().await;
            self.note("killed every process".to_owned());
            self.cluster.start().await;
            let address = self.cluster.edge.0.clone();
            if let Err(error) = audit_blocks(&address, &self.ledger, &report.blocks).await {
                self.fail(&format!(
                    "after every process was killed and started again: {error:#}"
                ));
            }
            let after = self.list().await;
            let regions =
                |list: &RegionList| (Self::living(list), list.absorbed.clone(), list.next);
            if regions(&after) != regions(&before) {
                self.fail(&format!(
                    "the regions are others after every process was killed and started \
                     again: {before:?} before, {after:?} after"
                ));
            }
            self.note("the world is as the ledgers say after starting from disk".to_owned());
        }
        self.stop_everything().await;
        if std::env::var_os("CLUSTINE_MERGES_KEEP").is_some() {
            let kept = self.keep();
            println!("{}: kept: {}", self.test, kept.display());
        }
        report
    }

    /// Asks every process to stop and fails unless each ends without an error: the
    /// edge, then the coordinator, then the workers and the world store. With the
    /// coordinator gone a worker has nobody to hand its regions to and knows it, so it
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
            let sent = Command::new("kill").args(["-TERM", &pid]).status().await;
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

/// E1. Two regions are merged while bots walk, build and step across the line between
/// them: the merge is asked for just as one of them steps across. Nobody is
/// disconnected, the ledger equals the world, also from disk, every player is one
/// entity throughout, which the bots see of each other, and the longest wait of a bot
/// of each region is printed and bounded.
#[tokio::test(flavor = "multi_thread")]
async fn players_of_two_regions_keep_playing_when_one_region_absorbs_the_other() {
    if a_repetition() {
        return;
    }
    let mut merges = Merges::start("merge", STRIPES).await;
    // The bot is handed from one region to the other in the next few ticks; the
    // merge begins somewhere in them.
    let crossing = merges.next_crossing().await;
    let delay = Duration::from_millis(merges.random.below(120));
    tokio::time::sleep(delay).await;
    merges.note(format!(
        "bot {} stepped across the line {delay:?} ago, going {}",
        crossing.bot,
        if crossing.eastwards { "east" } else { "west" }
    ));
    let noticed = merges.merge_and_notice(0, 1).await;
    let list = merges.list().await;
    if Merges::living(&list) != [0] || list.regions[0].pinned.len() != 2 {
        merges.fail(&format!(
            "after the merge the list is not one region pinned to both stripes: {list:?}"
        ));
    }
    merges.note(format!(
        "{}: the survivor's bots stood still for {}, the absorbed region's for {}",
        noticed.what,
        perhaps(noticed.stayed),
        perhaps(noticed.went)
    ));
    // They walk and build on, across a line that is no more.
    merges.played_on(4).await;
    played(&merges.finish(true).await);
}

/// E2. A region is split under the same bots, with bots in the part and bots that
/// stay; they walk between the two; the part is moved to the other worker with
/// `clustine move`; and it is merged back. On the way the part is itself split, as
/// whoever splits regions by hand will do next, and absorbs what was split off it:
/// a region that players never enter the world in is split and absorbs as the home
/// region does. The bots walk through three chunks here, so that a region two chunks
/// long along their lanes can be split in the middle.
#[tokio::test(flavor = "multi_thread")]
async fn players_keep_playing_when_a_region_is_split_and_the_part_is_moved_and_merged_back() {
    if a_repetition() {
        return;
    }
    let mut merges = Merges::start("split", LONG_LANES).await;
    // One region first: a split takes the players of some chunks of a region, and
    // the eastern stripe has the bots in one chunk only.
    merges.merge_and_notice(0, 1).await;

    // The two eastern chunks of the lanes go, with bots in each, and the western one
    // stays with those in it: the two regions meet at x = 32.
    let (part, split) = merges.split_east_of(0, &[(2, 0), (3, 0)]).await;
    let (stayed, went) = merges.bots_in(0, part);
    merges.note(format!(
        "{stayed} bots are in region 0 and {went} in region {part} now"
    ));
    // Moved at once: a lease after the split the coordinator would move one of the
    // two regions to the other worker by itself.
    let moved = merges.move_and_notice(part).await;
    if merges.owner(part) == merges.owner(0) {
        let loads = merges.loads();
        merges.fail(&format!(
            "the new region was not moved to the other worker: {loads:?}"
        ));
    }
    merges.played_on(3).await;

    // The part is split in turn, by the worker it was moved to, and the bots walk
    // through three regions.
    let (second, split_again) = merges.split_east_of(part, &[(3, 0)]).await;
    merges.played_on(4).await;
    let merged_again = merges.merge_and_notice(part, second).await;
    merges.played_on(2).await;

    let merged = merges.merge_and_notice(0, part).await;
    let list = merges.list().await;
    if Merges::living(&list) != [0] {
        merges.fail(&format!(
            "after the part was merged back the list has other regions than region 0: {list:?}"
        ));
    }
    for noticed in [&split, &moved, &split_again, &merged_again, &merged] {
        merges.note(format!(
            "{}: those who stayed stood still for {}, those who went for {}",
            noticed.what,
            perhaps(noticed.stayed),
            perhaps(noticed.went)
        ));
    }
    merges.played_on(3).await;
    played(&merges.finish(true).await);
}

/// E3. A region is split and the part merged back twenty times in a row, with bots
/// walking between the two: at any moment or just as a bot steps across the line,
/// east of the line or west of it, and half the time with the next thing asked for
/// before anyone has been served after the last.
#[tokio::test(flavor = "multi_thread")]
async fn players_keep_playing_while_a_region_is_split_and_merged_back_twenty_times() {
    if a_repetition() {
        return;
    }
    let mut merges = Merges::start("twenty times", STRIPES).await;
    merges.merge_and_notice(0, 1).await;
    let mut splits: Vec<Noticed> = Vec::new();
    let mut merged: Vec<Noticed> = Vec::new();
    let mut at_once: Vec<Duration> = Vec::new();
    let mut regions_met_at_the_line = 0;
    for _ in 0..rounds(20) {
        if merges.random.one_in(2) {
            let crossing = merges.next_crossing().await;
            let delay = Duration::from_millis(merges.random.below(120));
            tokio::time::sleep(delay).await;
            merges.note(format!(
                "bot {} stepped across the line {delay:?} ago",
                crossing.bot
            ));
        }
        // The chunk east of the line, or the one west of it.
        let named = [(if merges.random.one_in(2) { 3 } else { 2 }, 0)];
        if merges.random.one_in(2) {
            // The split, and the merge the moment the split is answered: the edge has
            // not linked to the part yet, or has just, and nobody has been served.
            let (_, before, _, part) = merges.split_where_bots_stand(0, &named).await;
            let (asked, _) = merges.merge(0, part).await;
            merges.has_absorbed(&asked, 0, part);
            merges.whole().await;
            let longest = merges.longest_pause_since(before);
            merges.note(format!(
                "a split and at once the merge back: the bots stood still for {} at most",
                seconds(longest)
            ));
            let may = 2 * merges.longest_pause;
            if longest > may {
                merges.fail(&format!(
                    "the bots stood still for {} at a split and a merge in a row; they may \
                     for {may:?}",
                    seconds(longest)
                ));
            }
            at_once.push(longest);
            continue;
        }
        let (part, split) = merges.split_and_notice(0, &named).await;
        let (stayed, went) = merges.bots_in(0, part);
        if stayed > 0 && went > 0 {
            regions_met_at_the_line += 1;
        }
        splits.push(split);
        // Somebody steps from the one into the other before they are one again.
        merges.played_on(1).await;
        merged.push(merges.merge_and_notice(0, part).await);
    }
    let seed = merges.seed;
    println!("the splits and merges of this run (seed {seed}):");
    for noticed in splits.iter().chain(&merged) {
        println!(
            "  {}: pause {} ({}); the command gives {} ms",
            noticed.what,
            seconds(noticed.pause),
            noticed.bots,
            noticed.own_time.as_millis()
        );
    }
    let those_who_stay = summary("splits", &splits, "those who stay", "those who go");
    let the_survivors = summary(
        "merges",
        &merged,
        "the survivor's players",
        "the absorbed region's",
    );
    merges.note(those_who_stay);
    merges.note(the_survivors);
    merges.note(format!(
        "{} more splits were merged back at once, and the bots stood still for {} over the \
         two; after {regions_met_at_the_line} of the others the bots stood in both regions",
        at_once.len(),
        in_the_middle_and_at_worst(&at_once)
    ));
    let list = merges.list().await;
    if Merges::living(&list) != [0] {
        merges.fail(&format!(
            "after every part was merged back the list has other regions than region 0: {list:?}"
        ));
    }
    played(&merges.finish(true).await);
}

/// The pauses of merges, splits and moves side by side, on a cluster that has the
/// machine to itself, under bots that are spread out so far that no two of them have
/// the same chunks in view, as in the test of moves: whoever takes a region's players
/// in has the chunks of each of them to load. Round after round the region absorbs
/// what is beside it, is moved to the other worker, and is split again. Nobody stands
/// still for longer than at a move may.
#[tokio::test(flavor = "multi_thread")]
async fn players_stand_still_only_briefly_when_regions_are_merged_split_and_moved() {
    if a_repetition() {
        return;
    }
    let apart = Setup {
        apart: true,
        ..STRIPES
    };
    let mut merges = Merges::start("pauses", apart).await;
    let line = f64::from(LINE);
    let mut merged: Vec<Noticed> = Vec::new();
    let mut moved: Vec<Noticed> = Vec::new();
    let mut splits: Vec<Noticed> = Vec::new();
    let mut beside = 1;
    for _ in 0..rounds(5) {
        // With players on both sides, so that each merge has players of its own to
        // keep standing and players to take in.
        let both = "a bot stands in the region that absorbs and one in the region beside it";
        merges
            .until(both, |merges| {
                let there = merges.stand_in(beside);
                there.contains(&true) && there.contains(&false)
            })
            .await;
        merged.push(merges.merge_and_notice(0, beside).await);

        moved.push(merges.move_and_notice(0).await);

        // The bots east of the line go, each with what is around it; no bot is so
        // close to the line that it steps across before the region has split.
        let waiting = Instant::now();
        let (asked, before, went, part) = loop {
            let aside = "every bot stands well aside of the line, and some on either side";
            merges
                .until_the_bots(aside, |xs| {
                    xs.iter().all(|x| (*x - line).abs() >= WELL_INSIDE)
                        && xs.iter().any(|x| *x < line)
                        && xs.iter().any(|x| *x >= line)
                })
                .await;
            let xs = merges.xs();
            let went: Vec<bool> = xs.iter().map(|x| *x >= line).collect();
            let bots = xs.iter().enumerate().zip(&went);
            let named: Vec<(i32, i32)> = bots
                .filter(|(_, went)| **went)
                .map(|((bot, x), _)| merges.chunk_of(bot, *x))
                .collect();
            let (asked, before) = merges.split(0, &named).await;
            if let Some(part) = Merges::split_off(&asked, 0) {
                break (asked, before, went, part);
            }
            if !Merges::found_nobody(&asked) || waiting.elapsed() > PATIENCE {
                let outcome = asked.outcome();
                merges.fail(&format!("region 0 was not split: {outcome}"));
            }
        };
        merges.whole().await;
        let each = went.iter().map(|went| if *went { part } else { 0 });
        merges.division = Division::Each(each.collect());
        let what = format!("the split of region {part} off region 0");
        splits.push(merges.noticed(&what, &asked, before, &went));
        beside = part;
    }
    // What the bots wait for when nothing is done to the cluster, to compare with:
    // over a few rounds of everyone being served, now that the regions have long
    // loaded what the bots see. Right after the bots arrived they had not.
    merges.served().await;
    let quiet = Instant::now();
    for _ in 0..10 {
        merges.served().await;
    }
    let undisturbed = merges.longest_pause_since(quiet);

    let seed = merges.seed;
    println!("the merges, moves and splits of this run (seed {seed}):");
    for noticed in merged.iter().chain(&moved).chain(&splits) {
        println!(
            "  {}: pause {} ({}); the command gives {} ms",
            noticed.what,
            seconds(noticed.pause),
            noticed.bots,
            noticed.own_time.as_millis()
        );
    }
    let merged = summary(
        "merges",
        &merged,
        "the survivor's players",
        "the absorbed region's",
    );
    let splits = summary("splits", &splits, "those who stay", "those who go");
    let moved = summary(
        "moves",
        &moved,
        "players of other regions",
        "the region's players",
    );
    for line in [merged, splits, moved] {
        merges.note(line);
    }
    merges.note(format!(
        "undisturbed, a bot waits {} at most for an acknowledgement",
        seconds(undisturbed)
    ));
    // Not started from disk again: the other tests here do that, and this cluster
    // keeps theirs waiting.
    played(&merges.finish(false).await);
}

/// What became of a merge or a split during which a process was killed, by the world
/// store's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Found {
    /// The regions as they were before it.
    AsBefore,
    /// It was made.
    AsAfter,
}

/// A process that is killed in the middle of a merge or a split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Victim {
    Worker(usize),
    /// Killed and started again at once, on the world as it left it.
    Store,
    /// Killed, and another started at once, which knows nothing of the one before.
    Coordinator,
}

/// Which processes a test kills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Harm {
    /// The workers that take part.
    Workers,
    /// The world store or the coordinator during a merge, and the coordinator during
    /// a split.
    StoreOrCoordinator,
}

impl Merges {
    /// Kills the world store without warning and starts it again. From here on what
    /// the workers say about running a region has to be said anew.
    async fn kill_and_start_the_store(&mut self, why: &str) {
        let mut store = self.cluster.store.1.take().expect("the store is running");
        store.kill().await.unwrap();
        for worker in 0..self.cluster.workers.len() {
            let length = self.cluster.log(&worker_name(worker)).len();
            self.since.insert(worker, length);
        }
        self.cluster.start_store();
        let whereabouts = self.whereabouts();
        self.note(format!(
            "killed the world store, {why}, and started it again; {whereabouts}"
        ));
    }

    /// Kills the coordinator without warning and starts another, which knows nothing
    /// of the one before.
    async fn kill_and_start_the_coordinator(&mut self, why: &str) {
        let coordinator = self.cluster.coordinator.1.take();
        let mut coordinator = coordinator.expect("the coordinator is running");
        coordinator.kill().await.unwrap();
        self.coordinator_since = self.cluster.log("coordinator").len();
        self.cluster.start_coordinator();
        let whereabouts = self.whereabouts();
        self.note(format!(
            "killed the coordinator, {why}, and started another; {whereabouts}"
        ));
    }

    /// Kills `victim` when the worker `watched` has logged `moment` once more than
    /// `before` times, then `then` likewise if there is one, and a moment the seed
    /// chooses later. If the command `asking` ends before the worker has logged
    /// `moment`, nobody is killed. Returns the victim and when it was killed.
    async fn kill_at(
        &mut self,
        asking: &JoinHandle<Asked>,
        watched: usize,
        (moment, before): (&str, usize),
        then: Option<(&str, usize)>,
        victim: Victim,
    ) -> Option<(Victim, Instant)> {
        self.until(
            "the moment to kill has come, or the command has ended",
            |merges| merges.said(watched, moment) > before || asking.is_finished(),
        )
        .await;
        if self.said(watched, moment) <= before {
            self.note(format!(
                "the command ended before {} logged `{moment}`; nobody is killed",
                worker_name(watched)
            ));
            return None;
        }
        // What follows a split that was made comes whether or not the command has
        // ended, which it has by then as a rule.
        let mut last = moment;
        if let Some((moment, before)) = then {
            self.until("the moment to kill has come", |merges| {
                merges.said(watched, moment) > before
            })
            .await;
            last = moment;
        }
        let delay = Duration::from_millis(self.random.below(30));
        tokio::time::sleep(delay).await;
        let why = format!("{delay:?} after {} logged `{last}`", worker_name(watched));
        match victim {
            Victim::Worker(worker) => self.kill_worker(worker, &why).await,
            Victim::Store => self.kill_and_start_the_store(&why).await,
            Victim::Coordinator => self.kill_and_start_the_coordinator(&why).await,
        }
        Some((victim, Instant::now()))
    }

    /// Waits, after a process was killed in the middle of the command `asking`, for the
    /// command to be told something and for every region the list has to be run
    /// again; a worker that was killed is started again at once or only then, as the
    /// seed has it. Fails unless every region runs within two leases and a moment of
    /// the kill and the bots are served afterwards. Returns what the command came to
    /// and the list.
    async fn got_over(
        &mut self,
        asking: JoinHandle<Asked>,
        killed: Option<(Victim, Instant)>,
        lease: Duration,
    ) -> (Asked, RegionList) {
        let at_once = self.random.one_in(2);
        let worker = match killed {
            Some((Victim::Worker(worker), _)) => Some(worker),
            _ => None,
        };
        if let Some(worker) = worker.filter(|_| at_once) {
            self.start_worker(worker);
        }
        // Whoever asked is told what the coordinator finds out, or finds the
        // coordinator gone, and is not left waiting.
        let asked = self.answer(asking).await;
        let list = self.everything_runs().await;
        if let Some((victim, killed)) = killed {
            let took = killed.elapsed();
            self.note(format!(
                "every region ran again {} after {victim:?} was killed: {:?}",
                seconds(took),
                Self::living(&list)
            ));
            if took > 2 * lease + MOMENT {
                self.fail(&format!(
                    "every region was run again only {} after {victim:?} was killed; two \
                     leases and a moment are {:?}",
                    seconds(took),
                    2 * lease + MOMENT
                ));
            }
        }
        if let Some(worker) = worker {
            if !at_once {
                self.start_worker(worker);
            }
            self.registered(worker).await;
        }
        // And then the workers share the regions again, which is a move the
        // coordinator makes by itself and the next thing asked for is not to meet.
        let list_now = self.settled().await;
        if Self::living(&list_now) != Self::living(&list) {
            self.fail(&format!(
                "the regions changed after everything ran again: {list:?}, then {list_now:?}"
            ));
        }
        (asked, list_now)
    }

    /// Has region 0 absorb the region beside it, if `list` has one, or splits region 0
    /// where the bots are, if not, and kills a process at a moment the seed chooses
    /// during it: one of the workers that take part, or the world store or the
    /// coordinator, as `harm` says. Fails unless the regions are as before or as after
    /// as a whole, each is run again in time, and the command did not claim what the
    /// list does not have. Returns the list and what was found.
    async fn merge_or_split_with_a_kill(
        &mut self,
        list: &RegionList,
        lease: Duration,
        harm: Harm,
    ) -> (RegionList, Found) {
        let living = Self::living(list);
        let beside = living.iter().copied().find(|region| *region != 0);
        let other = if self.random.one_in(2) {
            Victim::Store
        } else {
            Victim::Coordinator
        };
        let (asked, now, found, what) = if let Some(absorbed) = beside {
            let (Some(survivor), Some(releasing)) = (self.owner(0), self.owner(absorbed)) else {
                self.fail("a region has no owner though the cluster was whole");
            };
            let victim = match harm {
                Harm::Workers if self.random.one_in(3) => Victim::Worker(releasing),
                Harm::Workers => Victim::Worker(survivor),
                Harm::StoreOrCoordinator => other,
            };
            let (watched, moment) = match self.random.below(4) {
                0 => (releasing, "asked to release the region"),
                1 => (survivor, "asked to have the region absorb another"),
                2 => (survivor, "handing the store a merge or a split"),
                _ => (survivor, "the merge has ended"),
            };
            let before = self.said(watched, moment);
            let asking = self.merging(0, absorbed);
            let killed = self
                .kill_at(&asking, watched, (moment, before), None, victim)
                .await;
            let (asked, now) = self.got_over(asking, killed, lease).await;
            let gone = now
                .absorbed
                .iter()
                .any(|(gone, into)| (gone.0, into.0) == (absorbed, 0));
            let found = match Self::living(&now).as_slice() {
                [0] if gone => Found::AsAfter,
                both if both == living && !gone => Found::AsBefore,
                _ => self.fail(&format!(
                    "the list is neither as before the merge nor as after it: {now:?}"
                )),
            };
            (asked, now, found, "merge")
        } else {
            let Some(owner) = self.owner(0) else {
                self.fail("region 0 has no owner though the cluster was whole");
            };
            let victim = match harm {
                Harm::Workers => Victim::Worker(owner),
                Harm::StoreOrCoordinator => other,
            };
            let part = list.next.0;
            let told = "asked to split the region";
            let handing = "handing the store a merge or a split";
            let ended = "the split has ended; opening the new region";
            let running = format!("running a region region={part} ");
            let running = (running.as_str(), self.said(owner, &running));
            let (moment, then, named) = match self.random.below(4) {
                0 => (told, None, (3, 0)),
                1 => (handing, None, (2, 0)),
                2 => (ended, None, (3, 0)),
                // When the worker has just begun to run the part.
                _ => (ended, Some(running), (2, 0)),
            };
            // Every bot is in the one region there is, wherever it stands.
            self.division = Division::stripes(&[]);
            self.until_some_would_go_and_some_stay(0, &[named]).await;
            let before = self.said(owner, moment);
            let asking = self.splitting(0, &[named]);
            let killed = self
                .kill_at(&asking, owner, (moment, before), then, victim)
                .await;
            let (asked, now) = self.got_over(asking, killed, lease).await;
            let found = match Self::living(&now).as_slice() {
                [0] if now.next == list.next => Found::AsBefore,
                [0, new] if *new == part && now.next.0 == part + 1 => Found::AsAfter,
                _ => self.fail(&format!(
                    "the list is neither as before the split nor as after it: {now:?}"
                )),
            };
            (asked, now, found, "split")
        };
        self.note(format!(
            "after the {what} the regions are {found:?}; the command was told: {}",
            asked.outcome()
        ));
        // The command agrees with the list where it claims that the thing was done.
        if asked.code == Some(0) && found == Found::AsBefore {
            self.fail(&format!(
                "the command said that the {what} was made, and the list has the regions as \
                 they were before"
            ));
        }
        (now, found)
    }
}

/// Merges and splits the two regions of a cluster again and again, each time with a
/// process killed in the middle as `harm` says, and fails unless the bots and an
/// auditor are content at the end, also with the world as it is on disk.
async fn regions_are_merged_and_split_with_kills(test: &str, harm: Harm) {
    // The shortest lease there is, as in the tests of chaos: it is what everyone
    // waits for after a kill.
    let quick = Setup {
        lease: Some(3),
        chaos: true,
        ..STRIPES
    };
    let lease = Duration::from_secs(3);
    let mut merges = Merges::start(test, quick).await;
    let mut list = merges.list().await;
    let (mut as_before, mut as_after) = (0, 0);
    for _ in 0..2 * rounds_from("CLUSTINE_CHAOS_KILLS", 3) {
        let (now, found) = merges.merge_or_split_with_a_kill(&list, lease, harm).await;
        list = now;
        match found {
            Found::AsBefore => as_before += 1,
            Found::AsAfter => as_after += 1,
        }
        merges.played_on(1).await;
    }
    merges.note(format!(
        "{as_before} times the regions were found as before, {as_after} times as after"
    ));
    played(&merges.finish(true).await);
}

/// E4. A worker is killed at a moment the seed chooses during a merge and during a
/// split, again and again: the worker of the region that absorbs or of the one that is
/// absorbed, when it has been told, when the merge is handed to the world store, or
/// when it is done; the worker that splits a region, likewise, or when it has just
/// begun to run the part. Whether the store has the merge or the split by then or
/// not, the regions are as before or as after as a whole, each is run again within two
/// leases and a moment, nobody is disconnected, and the ledger equals the world, also
/// from disk.
#[tokio::test(flavor = "multi_thread")]
async fn players_keep_playing_when_a_worker_is_killed_during_a_merge_or_a_split() {
    if a_repetition() {
        return;
    }
    regions_are_merged_and_split_with_kills("kills", Harm::Workers).await;
}

/// Beyond E1 to E6: the same with the world store or the coordinator killed in the
/// middle, and started again at once. The workers open their regions again, or tell
/// the new coordinator what they run and have split off, and the players notice a
/// pause and nothing else.
///
/// Killed during a split, the world store once in a few times has made the split and
/// dies before it says so; what that found is at the end of this file.
#[tokio::test(flavor = "multi_thread")]
async fn players_keep_playing_when_the_store_or_the_coordinator_is_killed_during_a_merge_or_a_split()
 {
    if a_repetition() {
        return;
    }
    regions_are_merged_and_split_with_kills("other kills", Harm::StoreOrCoordinator).await;
}

/// E5. The edge's process stands still, without it or its connections noticing, while
/// two regions are merged, the survivor is absorbed by a third, and that one is split;
/// and again, for a good part of its patience, while the part is merged back, the
/// region is split again and that part merged back too. When the edge carries on it
/// hears of all of it at once, in the welcomes of the regions that are left. Nobody is
/// disconnected, every bot is served again, and the ledger equals the world.
#[tokio::test(flavor = "multi_thread")]
async fn players_keep_playing_when_the_edge_stands_still_across_merges_and_splits() {
    if a_repetition() {
        return;
    }
    // Three stripes: region 0 west of block x = 32, with the chunk players enter in,
    // region 1 up to x = 48, region 2 east of it.
    let three = Setup {
        boundaries: &[2, 3],
        ..LONG_LANES
    };
    let mut merges = Merges::start("edge stopped", three).await;
    let mut beside: Vec<Region> = vec![1, 2];
    for long in [false, true] {
        // Nothing of the coordinator's own is under way, which what is asked for here
        // would have to wait for with the edge standing still; and the bots are spread
        // over the three chunks of their lanes, so that every region has players.
        merges.settled().await;
        let spread = "a bot stands in each of the three chunks of the lanes";
        merges
            .until_the_bots(spread, |xs| {
                (1..=3).all(|chunk| xs.iter().any(|x| chunk_x(*x) == chunk))
            })
            .await;
        let stopped = Instant::now();
        merges.signal_edge("STOP", "stopped").await;
        // What the regions know of the bots is what the edge passed on before it
        // stopped. The bots walk on and are a step or two further by now, so a bot
        // close to the border of its chunk may be known in the next one.
        let xs = merges.xs();
        let east = xs.iter().copied().fold(f64::MIN, f64::max);
        let mut named = vec![(chunk_x(east), 0)];
        for nearby in [chunk_x(east - 2.0), chunk_x(east + 2.0)] {
            if !named.contains(&(nearby, 0)) {
                named.push((nearby, 0));
            }
        }

        // Each region beside region 0 is absorbed: the eastern one by the one in the
        // middle first, when there are three.
        if let [middle, eastern] = beside[..] {
            let (asked, _) = merges.merge(middle, eastern).await;
            merges.has_absorbed(&asked, middle, eastern);
            beside = vec![middle];
        }
        let absorbed = beside[0];
        let (asked, _) = merges.merge(0, absorbed).await;
        merges.has_absorbed(&asked, 0, absorbed);

        // Players who came with a merge of stripes stand in chunks that the region
        // holds only once it has claimed them, a tick or two after the merge
        // (ADR-0014, section 2.2, and S31: who stands in a chunk that is not held
        // stays). A split that comes sooner finds nobody there and is asked for again;
        // it does most times here, where it follows the merge by a few milliseconds.
        let waiting = Instant::now();
        let mut part = loop {
            let (asked, _) = merges.split(0, &named).await;
            if let Some(part) = Merges::split_off(&asked, 0) {
                break part;
            }
            if !Merges::found_nobody(&asked) || waiting.elapsed() > EDGE_PATIENCE / 4 {
                let outcome = asked.outcome();
                merges.fail(&format!(
                    "region 0 was not split while the edge stood still: {outcome}"
                ));
            }
        };
        if long {
            // And once more, so that the edge has a split behind a merge to hear of
            // and a part that was absorbed before it ever linked to it.
            let (asked, _) = merges.merge(0, part).await;
            merges.has_absorbed(&asked, 0, part);
            let (asked, _) = merges.split(0, &[(1, 0), (2, 0), (3, 0)]).await;
            let Some(next) = Merges::split_off(&asked, 0) else {
                let outcome = asked.outcome();
                merges.fail(&format!(
                    "region 0 was not split again while the edge stood still: {outcome}"
                ));
            };
            part = next;
            // How long the edge stands still is what is tried here, not a wait for
            // anything to come about.
            tokio::time::sleep(LONG_STANDSTILL.saturating_sub(stopped.elapsed())).await;
        }
        beside = vec![part];
        let stood = stopped.elapsed();
        if stood >= EDGE_PATIENCE {
            merges.fail(&format!(
                "the merges and the split took {}, which is as long as the edge's patience \
                 of {EDGE_PATIENCE:?}; they were to be over well within it",
                seconds(stood)
            ));
        }
        merges.signal_edge("CONT", "let go on").await;
        merges.note(format!("the edge stood still for {}", seconds(stood)));
        let carried_on = Instant::now();
        let list = merges.whole().await;
        if Merges::living(&list) != [0, part] {
            merges.fail(&format!(
                "the list does not have region 0 and the part that was split off last: {list:?}"
            ));
        }
        let served = carried_on.elapsed();
        let longest = merges.longest_pause_since(stopped);
        merges.note(format!(
            "every bot was served {} after the edge carried on; the longest any bot waited \
             was {}",
            seconds(served),
            seconds(longest)
        ));
        // The edge has as many resumes behind it then as there are regions left, and
        // what the bots sent meanwhile to pass on.
        let may = merges.longest_pause;
        if served > may {
            merges.fail(&format!(
                "the bots were served only {} after the edge carried on; that may take \
                 {may:?}",
                seconds(served)
            ));
        }
        merges.played_on(3).await;
    }
    played(&merges.finish(true).await);
}

/// A player besides the ledger bots, who stands where the test put them until they
/// are told to leave, and then joins again at once under their name.
struct Guest {
    name: String,
    leave: Arc<AtomicBool>,
    /// Ends with the entity the guest was before it left, and the guest as it joined
    /// again.
    coming_back: JoinHandle<anyhow::Result<(i32, Bot)>>,
}

impl Guest {
    /// Has `bot` stand where it is until it is told to leave, and join the server at
    /// `address` again `pause` after it left.
    fn standing(mut bot: Bot, address: String, pause: Duration) -> Self {
        let name = bot.info.profile.name.clone();
        let leave = Arc::new(AtomicBool::new(false));
        let coming_back = tokio::spawn({
            let (leave, name) = (Arc::clone(&leave), name.clone());
            async move {
                while !leave.load(Ordering::Relaxed) {
                    bot.idle(Duration::from_millis(10)).await?;
                }
                let before = bot.info.login.entity_id;
                drop(bot);
                // Between leaving and joining again, as long as the seed has it: not a
                // wait for anything.
                tokio::time::sleep(pause).await;
                let bot = Bot::join(&address, &name).await?;
                anyhow::Ok((before, bot))
            }
        });
        Self {
            name,
            leave,
            coming_back,
        }
    }
}

impl Merges {
    /// Has a guest join, or takes the one that is there, and walks them to the block
    /// column at `x` beside the bots' lanes, on a row nobody builds on.
    async fn guest_at(&mut self, guest: Option<Bot>, x: f64) -> Bot {
        let address = self.cluster.edge.0.clone();
        let arrived = async {
            let mut guest = match guest {
                Some(guest) => guest,
                None => Bot::join(&address, "Guest").await?,
            };
            guest.wait_for_chunks(1, PATIENCE).await?;
            // Along the column players enter in first, where nobody builds, and then
            // along the row behind the last bot's plot.
            let (column, row) = (guest.location.0, 15.5);
            guest.walk_to(column, row, 0.9).await?;
            guest.walk_to(x, row, 0.9).await?;
            let own_chunk = (chunk_x(x), 0);
            guest
                .wait_until(PATIENCE, |bot| {
                    bot.center == Some(own_chunk) && bot.chunks.contains_key(&own_chunk)
                })
                .await?;
            anyhow::Ok(guest)
        };
        match arrived.await {
            Ok(guest) => guest,
            Err(error) => self.fail(&format!("the guest could not walk to x = {x}: {error:#}")),
        }
    }

    /// Fails unless the guest who has joined again is in the world where every player
    /// enters it, can place and break a block there, and is seen by someone who joins
    /// now as one entity, the one it was told it is, in that place. `before` is the
    /// entity it was before it left, and `entered` where players enter the world.
    async fn is_back(&mut self, guest: &mut Bot, before: i32, entered: (f64, f64, f64)) {
        let name = guest.info.profile.name.clone();
        let entity = guest.info.login.entity_id;
        let air = Some(i32::from(blocks::AIR.0));
        let acted = async {
            guest.wait_for_chunks(1, PATIENCE).await?;
            // Next to where players enter, where none of the bots builds; placed and
            // broken again, so that the next time finds the spot free.
            let spot = (2, GROUND, 3);
            let placed = guest
                .use_item_on(spot.0, spot.1 - 1, spot.2, face::TOP)
                .await?;
            guest
                .wait_until(PATIENCE, |bot| {
                    bot.acknowledged_sequence >= placed
                        && bot
                            .block_at(spot.0, spot.1, spot.2)
                            .is_ok_and(|block| block.is_some() && block != air)
                })
                .await?;
            let broken = guest.dig(spot.0, spot.1, spot.2).await?;
            guest
                .wait_until(PATIENCE, |bot| {
                    bot.acknowledged_sequence >= broken
                        && bot
                            .block_at(spot.0, spot.1, spot.2)
                            .is_ok_and(|block| block == air)
                })
                .await
        };
        if let Err(error) = acted.await {
            self.fail(&format!(
                "{name} could not act after joining again: {error:#}"
            ));
        }
        if guest.location != entered || guest.stats.teleports_confirmed != 1 {
            self.fail(&format!(
                "{name} joined again at {entered:?} and was put at {:?}, moved {} times",
                guest.location, guest.stats.teleports_confirmed
            ));
        }
        if entity == before {
            self.fail(&format!(
                "{name} joined again as the entity {entity} it was before it left"
            ));
        }

        let address = self.cluster.edge.0.clone();
        let uuid = guest.info.profile.uuid;
        let seen = tokio::spawn(async move {
            let mut witness = Bot::join(&address, "Witness").await?;
            // The witness is given what is around it; a second entity for one player
            // among it is the bot's to refuse, which fails this.
            let shown = witness
                .wait_until(PATIENCE, |bot| {
                    bot.entities
                        .get(&entity)
                        .is_some_and(|seen| seen.uuid == uuid && seen.position == entered)
                })
                .await;
            let entities: Vec<(i32, (f64, f64, f64))> = witness
                .entities
                .iter()
                .filter(|(_, seen)| seen.uuid == uuid)
                .map(|(id, seen)| (*id, seen.position))
                .collect();
            anyhow::Ok((shown.is_ok(), entities))
        });
        // The guest has to hear what it is sent meanwhile, and to stay.
        while !seen.is_finished() {
            self.tend().await;
            if let Err(error) = guest.idle(LOOK).await {
                self.fail(&format!(
                    "{name} was disconnected after joining again: {error:#}"
                ));
            }
        }
        match seen.await.expect("the witness does not panic") {
            Ok((true, entities)) if entities.len() == 1 => {}
            Ok((_, entities)) => self.fail(&format!(
                "{name} is entity {entity} at {entered:?}, and someone who joins sees them as \
                 {entities:?}"
            )),
            Err(error) => self.fail(&format!(
                "someone who joined to look for {name} failed: {error:#}"
            )),
        }
        self.note(format!(
            "{name} is back as entity {entity} (it was {before}), where players enter, has \
             built, and is seen as one entity"
        ));
    }

    /// Has the guest leave and join again around the command that `command` makes:
    /// before it is started, when the worker `watched` has logged `moment`, or a
    /// moment after it was started, as the seed has it. Returns what the command came
    /// to and the guest as it joined again, checked by [`Merges::is_back`].
    async fn leaves_and_joins_again(
        &mut self,
        guest: Bot,
        entered: (f64, f64, f64),
        what: &str,
        command: Command,
        watched: usize,
        moment: &str,
    ) -> (Asked, Bot) {
        let pause = Duration::from_millis(self.random.below(100));
        let address = self.cluster.edge.0.clone();
        let guest = Guest::standing(guest, address, pause);
        let before = self.said(watched, moment);
        let when = self.random.below(3);
        if when == 0 {
            guest.leave.store(true, Ordering::Relaxed);
            self.note(format!(
                "{} leaves, to join again {pause:?} later",
                guest.name
            ));
        }
        let asking = self.asking(what.to_owned(), command);
        if when == 1 {
            self.until("the worker is at it, or the command has ended", |merges| {
                merges.said(watched, moment) > before || asking.is_finished()
            })
            .await;
        }
        if when > 0 {
            let delay = Duration::from_millis(self.random.below(60));
            tokio::time::sleep(delay).await;
            guest.leave.store(true, Ordering::Relaxed);
            let after = if when == 1 {
                format!("{} logged `{moment}`", worker_name(watched))
            } else {
                "the command was started".to_owned()
            };
            self.note(format!(
                "{} leaves {delay:?} after {after}, to join again {pause:?} later",
                guest.name
            ));
        }
        let asked = self.answer(asking).await;
        let coming_back = guest.coming_back;
        self.until("the guest has joined again or failed to", |_| {
            coming_back.is_finished()
        })
        .await;
        let (was, mut back) = match coming_back.await.expect("the guest does not panic") {
            Ok(back) => back,
            Err(error) => self.fail(&format!(
                "the guest who left and joined again during {what} was not let in: {error:#}"
            )),
        };
        self.is_back(&mut back, was, entered).await;
        (asked, back)
    }
}

/// E6. A player leaves and joins again under their name while the region they stood
/// in is merged or split: while it is absorbed, while they are split off it, while it
/// absorbs another, and while others are split off it. Each time they are in the world
/// again where players enter it, as one entity, and can build; and the bots, who see
/// them come and go, never see two of them.
#[tokio::test(flavor = "multi_thread")]
async fn a_player_who_leaves_and_joins_again_during_a_merge_or_a_split_is_one_player() {
    if a_repetition() {
        return;
    }
    let mut merges = Merges::start("leaving and joining", STRIPES).await;
    let line = f64::from(LINE);
    let (east, west) = (56.5, 40.5);
    let mut guest = merges.guest_at(None, east).await;
    // Every player enters the world in the same place, which the guest was put in
    // and has walked away from.
    let Some(entered) = guest.position.as_ref().map(|put| (put.x, put.y, put.z)) else {
        merges.fail("the guest was never put anywhere");
    };
    // The region beside region 0, east of the line: the eastern stripe at first.
    let mut beside = 1;
    for _ in 0..rounds(1) {
        // In the region that is absorbed.
        let (Some(survivor), Some(releasing)) = (merges.owner(0), merges.owner(beside)) else {
            merges.fail("a region has no owner though the cluster was whole");
        };
        let moment = "asked to release the region";
        let command = merges.cluster.merge_command(0, beside);
        let what = format!("region 0 to absorb region {beside}, where the guest stands");
        let (asked, back) = merges
            .leaves_and_joins_again(guest, entered, &what, command, releasing, moment)
            .await;
        merges.has_absorbed(&asked, 0, beside);
        merges.whole().await;

        // In the chunk whose players are split off, with a bot there as well, so that
        // the split is made whether the guest is still there or not.
        guest = merges.guest_at(Some(back), east).await;
        let part = loop {
            let aside = "a bot stands well east of the line";
            merges
                .until_the_bots(aside, |xs| xs.iter().any(|x| *x >= line + WELL_INSIDE))
                .await;
            let command = merges.cluster.split_command(0, &[(3, 0)]);
            let what = "region 0 to be split where the guest stands";
            let moment = "asked to split the region";
            let (asked, back) = merges
                .leaves_and_joins_again(guest, entered, what, command, survivor, moment)
                .await;
            if let Some(part) = Merges::split_off(&asked, 0) {
                guest = back;
                break part;
            }
            if !Merges::found_nobody(&asked) {
                let outcome = asked.outcome();
                merges.fail(&format!("region 0 was not split: {outcome}"));
            }
            guest = merges.guest_at(Some(back), east).await;
        };
        // The coordinator moves one of the two regions to the other worker a lease
        // later, which the next thing asked for is not to meet.
        merges.settled().await;

        // In the region that absorbs, or again in the one that is absorbed, as the
        // split before left the line.
        guest = merges.guest_at(Some(guest), west).await;
        let Some(owner) = merges.owner(0) else {
            merges.fail("region 0 has no owner though the cluster was whole");
        };
        let moment = "handing the store a merge or a split";
        let command = merges.cluster.merge_command(0, part);
        let what = format!("region 0 to absorb region {part}, with the guest west of the line");
        let (asked, back) = merges
            .leaves_and_joins_again(guest, entered, &what, command, owner, moment)
            .await;
        merges.has_absorbed(&asked, 0, part);
        merges.whole().await;

        // In the region others are split off, in a chunk that is not named.
        guest = merges.guest_at(Some(back), west).await;
        let part = loop {
            let aside = "a bot stands well east of the line";
            merges
                .until_the_bots(aside, |xs| xs.iter().any(|x| *x >= line + WELL_INSIDE))
                .await;
            let Some(owner) = merges.owner(0) else {
                merges.fail("region 0 has no owner though the cluster was whole");
            };
            let command = merges.cluster.split_command(0, &[(3, 0)]);
            let what = "region 0 to be split east of where the guest stands";
            let moment = "handing the store a merge or a split";
            let (asked, back) = merges
                .leaves_and_joins_again(guest, entered, what, command, owner, moment)
                .await;
            if let Some(part) = Merges::split_off(&asked, 0) {
                guest = back;
                break part;
            }
            if !Merges::found_nobody(&asked) {
                let outcome = asked.outcome();
                merges.fail(&format!("region 0 was not split: {outcome}"));
            }
            guest = merges.guest_at(Some(back), west).await;
        };
        merges.settled().await;

        // The world is two regions that meet at the line again, the guest having
        // stayed west of it: the part in the place of the eastern stripe.
        beside = part;
        guest = merges.guest_at(Some(guest), east).await;
    }
    drop(guest);
    played(&merges.finish(false).await);
}

// What these tests found, and what the test below keeps found: the sequence, what the
// records asked for, and what happened instead. It has been put right since
// (`Coordinator::split_ended` owes a reading after every split that ended without a
// part), and the same is tested on the coordinator's state machine alone in
// `services/coordinator/tests/reshape.rs`. The sequence is told as it was.
//
// **The part of a split that the world store made as it was lost was given to nobody**
// (the coordinator, `Coordinator::split_ended` and `unlisted` in
// `services/coordinator/src/state.rs`).
//
// 1. A region `A` with players is split (`clustine split`). The coordinator reserves
//    `A` and orders `SplitOff`; `A`'s runner stops and hands the store the
//    `SplitCommit`.
// 2. The world store appends and syncs the record of the split, and dies before its
//    answer reaches the runner. (Here it is killed; a connection that ends at that
//    moment does the same.)
// 3. The runner ends with `Off::StoreLost` and the worker says `SplitEnded { Err(
//    StoreLost) }`. The coordinator ends the reservation, tells whoever asked "the
//    worker lost the world store on the way", and asks for the store's list, once.
// 4. That reading fails, as it will: the worker has just said that the store is gone,
//    and the store is back some tens of milliseconds later at the earliest. The
//    coordinator logs "the world store's list of regions cannot be read" and
//    `Coordinator::unlisted` leaves nothing owed. Nothing reads the list again: no
//    worker registers, as the workers lost the store and not the coordinator, and no
//    reservation is left.
// 5. The store comes back with the record: its list has the new region `N`. The
//    worker opens `A` again and restores it as of the split's tick, with `SplitOff` in
//    its outbox; the edge resumes with `A`, puts the players who went under `N`, and
//    `A` lets everyone go to `N` who walks into its chunks ("player departed to
//    another region").
// 6. Nobody runs `N` and the coordinator does not know of it, so no routing table
//    ever names it. Twenty seconds later the edge gives up on each of `N`'s players:
//    "The server fell too far behind." It stays so until something else makes the
//    coordinator read the list: a worker that registers, or the next merge or split
//    that is asked for.
//
// What the records say: ADR-0014, section 7, the row "store, after [the record is
// synced]": "as after; handles lost; [...] `A` is opened again; `Off::StoreLost`; the
// list shows `N`, which is assigned and restored from the record". Section 5.4: "On
// `Err(why)`: the reservation ends, the asker is told `Err`; the list is read, because
// `Off::StoreLost` leaves open what happened." Neither said what is to happen when
// that reading fails, which after `StoreLost` is the rule and not the exception
// (section 5.4 says it now). The coordinator asked again at every tick after a split
// whose reservation ended *without* the worker's word (`lapse_split` sets `owed`),
// and for a merge of which a worker has said what came; a split of which the worker
// said `Err` was the one case left out.
//
// Seen once in eleven runs of the test above that kills the store or the coordinator,
// when that still killed the store during splits (seed 719256: the store was killed
// 24 ms after the worker logged `handing the store a merge or a split`).

impl Merges {
    /// Waits until every region the world store's list has is run by a worker the edge
    /// is linked to, for `limit` at most, and returns the list, or the list as it was
    /// last read if that did not come about.
    async fn everything_runs_within(&mut self, limit: Duration) -> Result<RegionList, String> {
        let waiting = Instant::now();
        loop {
            self.tend().await;
            let list = self.cluster.regions().await;
            if let Ok(list) = &list {
                let living = Self::living(list);
                let known = self.table().map(|table| table.known());
                if known.as_ref() == Some(&living) && living.iter().all(|region| self.runs(*region))
                {
                    return Ok(list.clone());
                }
            }
            if waiting.elapsed() > limit {
                return Err(format!("{list:?}"));
            }
            tokio::time::sleep(LOOK).await;
        }
    }
}

/// What was found, end to end, under the bots: the world store is killed as a split
/// is handed to it, again and again, until it has the split on disk and the worker
/// has lost its answer. The region the split made has to be run by somebody within
/// two leases and a moment then, as after every other kill; it was run by nobody, and
/// its players were disconnected when the edge's patience was over.
///
/// Whether a kill lands between the store's record and its answer is a matter of a
/// few milliseconds: the first or the second try did it where this was found, and
/// fifteen to twenty-seven were needed on another day. So the test tries eight times,
/// or four times `CLUSTINE_CHAOS_KILLS`, and if the case does not come about it says
/// so and has still killed the store in that many splits; it does not fail for what
/// it could not bring about. What it is about is tested without the luck on the
/// coordinator's state machine (`services/coordinator/tests/reshape.rs`).
#[tokio::test(flavor = "multi_thread")]
async fn the_part_of_a_split_is_run_when_the_store_was_killed_as_it_made_the_split() {
    if a_repetition() {
        return;
    }
    let quick = Setup {
        lease: Some(3),
        chaos: true,
        ..STRIPES
    };
    let lease = Duration::from_secs(3);
    let mut merges = Merges::start("store killed in a split", quick).await;
    let (asked, _) = merges.merge(0, 1).await;
    merges.has_absorbed(&asked, 0, 1);
    merges.division = Division::stripes(&[]);
    merges.whole().await;

    let moment = "handing the store a merge or a split";
    let named = [(3, 0)];
    let mut found = None;
    let tries = 4 * rounds_from("CLUSTINE_CHAOS_KILLS", 2);
    for attempt in 1..=tries {
        let Some(owner) = merges.owner(0) else {
            merges.fail("region 0 has no owner though the cluster was whole");
        };
        merges.until_some_would_go_and_some_stay(0, &named).await;
        let before = merges.said(owner, moment);
        let asking = merges.splitting(0, &named);
        let killed = merges
            .kill_at(&asking, owner, (moment, before), None, Victim::Store)
            .await;
        let asked = merges.answer(asking).await;
        let Some((_, killed)) = killed else {
            merges.settled().await;
            continue;
        };
        // What the store that was started again has, once it answers.
        let waiting = Instant::now();
        let list = loop {
            match merges.cluster.regions().await {
                Ok(list) => break list,
                Err(error) if waiting.elapsed() > PATIENCE => {
                    merges.fail(&format!("the world store does not come back: {error}"))
                }
                Err(_) => tokio::time::sleep(LOOK).await,
            }
        };
        match (
            Merges::split_off(&asked, 0),
            Merges::living(&list).as_slice(),
        ) {
            // The worker had the store's answer before the store died.
            (Some(part), _) => {
                merges.settled().await;
                let (asked, _) = merges.merge(0, part).await;
                merges.has_absorbed(&asked, 0, part);
                merges.whole().await;
            }
            // The store died before it had the split.
            (None, [0]) => {
                merges.settled().await;
            }
            // The store has the split, and the worker never heard.
            (None, [0, part]) => {
                merges.note(format!(
                    "attempt {attempt}: the world store has region {part}, and the command \
                     was told: {}",
                    asked.outcome()
                ));
                found = Some((*part, killed));
                break;
            }
            _ => merges.fail(&format!(
                "the list is neither as before the split nor as after it: {list:?}"
            )),
        }
    }
    let Some((part, killed)) = found else {
        merges.note(format!(
            "in {tries} splits the store was never killed between its record of the split \
             and its answer"
        ));
        merges.served().await;
        merges.played_on(2).await;
        played(&merges.finish(true).await);
        return;
    };
    let limit = 2 * lease + MOMENT;
    match merges.everything_runs_within(limit).await {
        Ok(_) => merges.note(format!(
            "every region ran again {} after the store was killed",
            seconds(killed.elapsed())
        )),
        Err(list) => {
            let table = merges.table();
            merges.fail(&format!(
                "region {part}, which the world store made as it was killed, is run by nobody \
                 {limit:?} later, and the coordinator does not know of it; the list is \
                 {list}, and the routing table {table:?}"
            ));
        }
    }
    merges.served().await;
    merges.played_on(2).await;
    played(&merges.finish(true).await);
}
