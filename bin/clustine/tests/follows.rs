//! Regions follow their players: a coordinator that is started with `--reshape
//! by-itself` merges regions whose players come near each other and splits a region
//! whose players part, unasked, on a cluster of real processes under players who keep
//! playing and keep a ledger of what they were told (the ledger scenario of the bots).
//! These are the scenarios E1 to E8 of `docs/adr/0016-when-to-merge-and-split.md`,
//! section 11, written from that record: the first place where the rules, the
//! coordinator's state machine, the worker's reports and the paths of a merge and a
//! split meet with nobody asking for anything.
//!
//! The world is two stripes that meet at block x = 64, region 0 west with the chunk
//! players enter in, region 1 east, on two workers, with the view distance a player
//! usually has. The coordinator merges at 3 chunks and splits beyond 5, and leaves a
//! region alone for 5 seconds after anything it did to it. The players are **groups**:
//! `A`, two bots, `B`, one, and in one test `C`, one, each a ledger scenario of its own
//! on lanes of its own in the chunk row z = 0. A group **stands in a chunk** when its
//! bots walk up and down within it, and is **sent** to another by being given other
//! coordinates to walk between; the test then waits until every bot of it is there.
//! All chunks below are in the row z = 0 and are given by their x.
//!
//! What the coordinator did is read from outside: the world store's list of regions,
//! the routing table the coordinator logs, and the lines its log has for everything it
//! begins by itself and for how a merge and a split ended (section 10 of the record).
//! What is expected is the record's, not the code's.
//!
//! Every test ends its groups, each of which then has an auditor join and compare the
//! world with the ledgers, and looks at every block once more after every process was
//! killed and started on the same disk. An auditor is a player like any other for the
//! coordinator's distances, so whatever a test counts, it has counted before it ends a
//! group.
//!
//! What these tests found in the server is at the end of the file, with the sequence,
//! what the record says and what happened, and a test that goes after it on the
//! coordinator's state machine.
//!
//! What follows from a seed is what the bots choose, how often the workers checkpoint
//! and, in the tests that kill, when and whom. Every test prints its seed; to run one
//! again, set `CLUSTINE_FOLLOWS_SEED`. `CLUSTINE_FOLLOWS_ROUNDS` sets how many rounds
//! the test of rounds does and `CLUSTINE_FOLLOWS_KILLS` how many rounds the tests that
//! kill do. `CLUSTINE_FOLLOWS_KEEP` keeps the processes' logs of a test that passes;
//! those of a test that fails are always kept, and the failure says where.
//!
//! The processes are those of an unoptimised build, as in every test here, so the
//! pauses are longer than those of a server built for use.

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clustine_botswarm::{Ledger, LedgerReport, Progress, Random, audit_blocks, ledger};
use clustine_coordinator::{Asked, Coordinator, CoordinatorConfig, Order, Policy, Reshaped};
use clustine_region::{Layout, RegionId};
use clustine_rpc::{ChunkBox, Crowds, PlayersOf, RegionInfo, RegionList, Vouch};
use clustine_world::{ChunkArea, ChunkPos, Vec3};
use tempfile::TempDir;
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

use common::processes::{Cluster, Table, Turn, turn, worker_name};

/// How long anything may take that is merely waited for, as in the tests of merges by
/// hand. It only ever runs out when something hangs.
const PATIENCE: Duration = Duration::from_secs(60);

/// How often a state that is waited for is looked at.
const LOOK: Duration = Duration::from_millis(20);

/// The moment beyond the leases that a cluster may take to be whole again after a
/// process was killed, as in the tests of merges by hand.
const MOMENT: Duration = Duration::from_secs(5);

/// The longest that a player may wait for an acknowledgement when regions are merged
/// or split on a cluster that shares the machine with the clusters of other tests: what
/// the tests of merges by hand allow.
const LONGEST_PAUSE: Duration = Duration::from_secs(5);

/// Every this many client ticks each bot sends a pulse.
const PULSE: u32 = 2;

/// The view distance of the edge, in chunks: what a player usually has.
const VIEW: i32 = 8;

/// The chunk x coordinate at which the world is divided: the stripes meet at block
/// x = 64. A player of the east stripe in chunk 3 would be within the merge distance
/// of the chunk players enter in.
const BOUNDARY: i32 = 4;

/// The distances the coordinator is started with, in chunks, and its rest. The record
/// says what these are chosen for.
const MERGE_DISTANCE: u32 = 3;
const SPLIT_DISTANCE: u32 = 5;
const REST: Duration = Duration::from_secs(5);

/// What the record allows for when a line of a log is written, where the times of two
/// lines are compared.
const WRITING: f64 = 0.1;

/// A client tick, which is what a bot's steps are counted in.
const TICK: Duration = Duration::from_millis(50);

/// What a region is called on the command line and in the logs.
type Region = u32;

/// Whether this run of the tests is the one that repeats the end-to-end tests on a
/// world divided into regions, which `CLUSTINE_TEST_BOUNDARIES` asks for. These tests
/// divide their worlds themselves and take minutes, so they run once, in the run
/// without it.
fn a_repetition() -> bool {
    std::env::var_os("CLUSTINE_TEST_BOUNDARIES").is_some()
}

/// The number the variable called `name` is set to, if it is set.
fn number_from(name: &str) -> Option<u64> {
    let set = std::env::var(name).ok()?;
    Some(set.parse().unwrap_or_else(|_| panic!("{name} is a number")))
}

/// The seed of this run: `CLUSTINE_FOLLOWS_SEED` if set, else the clock.
fn seed() -> u64 {
    number_from("CLUSTINE_FOLLOWS_SEED").unwrap_or_else(|| {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        now.as_nanos() as u64 % 1_000_000
    })
}

/// How often the workers write a checkpoint in a run with this seed: every second or
/// two, so that merges and splits fall into checkpoints, or practically never, so that
/// the checkpoint a merge or a split begins with has everything to save that was built
/// so far.
fn checkpoint_seconds(seed: u64) -> u64 {
    [1, 2, 300][(seed % 3) as usize]
}

/// Seconds with three decimals, for a message.
fn seconds(duration: Duration) -> String {
    format!("{:.3} s", duration.as_secs_f64())
}

/// A time that is known only if something happened, for a message.
fn perhaps(time: Option<Duration>) -> String {
    time.map_or("-".to_owned(), seconds)
}

/// The x coordinates a group walks up and down between when it walks between the
/// chunks `west` and `east`: far enough from the borders of the two for everything it
/// builds to be in those chunks.
fn between(west: i32, east: i32) -> (f64, f64) {
    (f64::from(16 * west) + 2.5, f64::from(16 * east) + 13.5)
}

/// The x coordinates a group walks up and down between when it stands in `chunk`.
fn within(chunk: i32) -> (f64, f64) {
    between(chunk, chunk)
}

/// When a line of a log was written, in seconds since 1970, which is what a line can
/// be compared by with another and with the test's own clock.
fn time_of(line: &str) -> Option<f64> {
    // "2026-10-08T09:10:26.123456Z  INFO ...".
    let stamp = line.split_whitespace().next()?;
    let (date, clock) = stamp.trim_end_matches('Z').split_once('T')?;
    let mut date = date.split('-').map(str::parse::<i64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let mut clock = clock.split(':').map(str::parse::<f64>);
    let (hours, minutes, rest) = (
        clock.next()?.ok()?,
        clock.next()?.ok()?,
        clock.next()?.ok()?,
    );
    // The days from 1970 to the date, with the year begun in March so that the day a
    // leap year adds is its last.
    let (year, month) = if month <= 2 {
        (year - 1, month + 9)
    } else {
        (year, month - 3)
    };
    let (era, of_era) = (year.div_euclid(400), year.rem_euclid(400));
    let of_year = (153 * month + 2) / 5 + day - 1;
    let days = era * 146_097 + of_era * 365 + of_era / 4 - of_era / 100 + of_year - 719_468;
    Some(days as f64 * 86_400.0 + hours * 3600.0 + minutes * 60.0 + rest)
}

/// What a line of a log gives as `name=`, up to the next space.
fn field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=");
    let rest = &line[line.find(&key)? + key.len()..];
    rest.split_whitespace().next()
}

/// The number a line of a log gives as `name=`.
fn number<T: std::str::FromStr>(line: &str, name: &str) -> Option<T> {
    field(line, name)?.parse().ok()
}

/// Something the coordinator began by itself, as the line of its log says that the
/// record has for it (section 10).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Begun {
    /// "a merge is begun by the distances".
    Merge {
        survivor: Region,
        absorbed: Region,
        gap: u32,
    },
    /// "an absorption is begun by itself".
    Absorption { survivor: Region, absorbed: Region },
    /// "a split is begun by itself". `part` is the id the split names, which the
    /// store need not give.
    Split {
        region: Region,
        part: Region,
        groups: u32,
        chunks: u32,
    },
    /// "a region is moved to even regions out".
    Move {
        region: Region,
        from: String,
        to: String,
    },
}

impl Begun {
    /// What `line` says was begun, if it is one of the four lines.
    fn read(line: &str) -> Option<Self> {
        if line.contains("a merge is begun by the distances") {
            Some(Self::Merge {
                survivor: number(line, "survivor")?,
                absorbed: number(line, "absorbed")?,
                gap: number(line, "gap")?,
            })
        } else if line.contains("an absorption is begun by itself") {
            Some(Self::Absorption {
                survivor: number(line, "survivor")?,
                absorbed: number(line, "absorbed")?,
            })
        } else if line.contains("a split is begun by itself") {
            Some(Self::Split {
                region: number(line, "region")?,
                part: number(line, "part")?,
                groups: number(line, "groups")?,
                chunks: number(line, "chunks")?,
            })
        } else if line.contains("a region is moved to even regions out") {
            Some(Self::Move {
                region: number(line, "region")?,
                from: field(line, "from")?.to_owned(),
                to: field(line, "to")?.to_owned(),
            })
        } else {
            None
        }
    }

    /// Whether this is a merge or a split: not a move.
    fn reshapes(&self) -> bool {
        !matches!(self, Self::Move { .. })
    }

    /// Whether the players of `region` are stood still by this: it is one of the two
    /// regions of the merge, the region that is split, or the region that is moved.
    fn is_of(&self, region: Region) -> bool {
        match self {
            Self::Merge {
                survivor, absorbed, ..
            }
            | Self::Absorption { survivor, absorbed } => [*survivor, *absorbed].contains(&region),
            Self::Split { region: split, .. } => *split == region,
            Self::Move { region: moved, .. } => *moved == region,
        }
    }

    /// In a few words, for a message.
    fn told(&self) -> String {
        match self {
            Self::Merge {
                survivor,
                absorbed,
                gap,
            } => format!("the merge of region {absorbed} into region {survivor}, {gap} apart"),
            Self::Absorption { survivor, absorbed } => {
                format!("the absorption of region {absorbed} by region {survivor}")
            }
            Self::Split { region, part, .. } => {
                format!("the split of region {part} off region {region}")
            }
            Self::Move { region, from, to } => {
                format!("the move of region {region} from {from} to {to}")
            }
        }
    }
}

/// How something ended, as the coordinator's log says.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ended {
    /// "a merge has ended", of a merge by the distances, an absorption or a merge that
    /// somebody asked for.
    Merge {
        survivor: Region,
        absorbed: Region,
        outcome: String,
    },
    /// "a worker says what came of a split".
    Split { region: Region, outcome: String },
    /// "a region was assigned": how a move ends, and a takeover.
    Assigned { region: Region, worker: String },
}

impl Ended {
    /// What `line` says has ended, if it is one of those lines.
    fn read(line: &str) -> Option<Self> {
        let outcome = || Some(line.split_once(" outcome=")?.1.trim().to_owned());
        if line.contains("a merge has ended") {
            Some(Self::Merge {
                survivor: number(line, "survivor")?,
                absorbed: number(line, "absorbed")?,
                outcome: outcome()?,
            })
        } else if line.contains("a worker says what came of a split") {
            Some(Self::Split {
                region: number(line, "region")?,
                outcome: outcome()?,
            })
        } else if line.contains("a region was assigned") {
            Some(Self::Assigned {
                region: number(line, "region")?,
                worker: field(line, "worker")?.to_owned(),
            })
        } else {
            None
        }
    }

    /// Whether a merge or a split ended well.
    fn well(&self) -> bool {
        match self {
            Self::Merge { outcome, .. } | Self::Split { outcome, .. } => outcome.starts_with("Ok"),
            Self::Assigned { .. } => false,
        }
    }

    /// The region a split that ended well made: the number in its outcome.
    fn part(&self) -> Option<Region> {
        let Self::Split { outcome, .. } = self else {
            return None;
        };
        let digits: String = outcome.chars().filter(char::is_ascii_digit).collect();
        digits.parse().ok().filter(|_| self.well())
    }
}

/// A group of players: one ledger scenario, which can be sent to a chunk.
struct Group {
    name: &'static str,
    ledger: Ledger,
    progress: Arc<Progress>,
    /// The scenario, until the group has left.
    playing: Option<JoinHandle<anyhow::Result<LedgerReport>>>,
    /// What the bots and their auditor found, once the group has left.
    report: Option<LedgerReport>,
}

/// Which region the bots of a group are in, for the tests that have to know: the
/// region they are in when the test begins to count, and whether they are the ones who
/// go when that region is split. The test sees to it that this is so, by where it
/// sends its groups and by what the list says of each part.
#[derive(Debug, Clone, Copy)]
struct Stands {
    group: &'static str,
    first: Region,
    goes: bool,
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
    /// Killed, and another started at once with the same arguments, which knows
    /// nothing of the one before.
    Coordinator,
}

/// Whom a test kills during a merge: the worker of the region that absorbs, the worker
/// of the region that is absorbed, the world store or the coordinator. During a split
/// the two workers are one, the worker of the region that is split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Harm {
    Survivor,
    Absorbed,
    Store,
    Coordinator,
}

/// What the bots noticed of one thing the coordinator began by itself, and how long
/// after the walk that caused it the thing was begun and was done.
struct Measured {
    what: String,
    /// What led to it: the walk that was under way, or last done, when it was begun;
    /// of a move, the end of the split that left a worker with a region more.
    cause: Option<String>,
    /// How long after that it was begun, and had ended.
    begun_after: Option<Duration>,
    done_after: Option<Duration>,
    /// The longest wait for an acknowledgement among the bots whose region went on
    /// (the survivor's, those who stayed at a split, at a move everybody else), and
    /// among those who came to another region or another worker.
    stayed: Option<Duration>,
    went: Option<Duration>,
}

/// A cluster whose coordinator merges and splits by itself, with groups of bots on it.
struct Follows {
    /// Its leave to run beside the other tests' clusters.
    _turn: Turn,
    /// Where the world and the logs are; taken out when they are to be kept.
    directory: Option<TempDir>,
    cluster: Cluster,
    test: String,
    seed: u64,
    random: Random,
    /// The coordinator's lease.
    lease: Duration,
    /// The first block east of every boundary a group may ever step across.
    lines: Vec<i32>,
    groups: Vec<Group>,
    started: Instant,
    /// The time of the logs at `started`, in seconds since 1970.
    started_at: f64,
    /// What was done to the cluster and what it did about it, in order.
    deeds: Vec<String>,
    /// When each group was sent somewhere, and where.
    walks: Vec<(Instant, String)>,
}

impl Follows {
    /// Starts a cluster of two workers on the two stripes whose coordinator merges and
    /// splits by itself, with the lease `lease` in seconds, or the one a coordinator
    /// has when it is not told any, which is 5. `lines` are the boundaries the groups
    /// of this test may step across. Returns once players can join.
    async fn start(test: &str, lease: Option<u64>, lines: &[i32]) -> Self {
        let turn = turn().await;
        let seed = seed();
        println!("{test}: seed {seed} (set CLUSTINE_FOLLOWS_SEED={seed} to run it again)");
        let directory = tempfile::Builder::new()
            .prefix("clustine-follows-")
            .tempdir()
            .unwrap();
        let checkpoints = checkpoint_seconds(seed);
        println!("{test}: the workers checkpoint every {checkpoints} s");
        let mut attempt = 0;
        let cluster = loop {
            let mut cluster = Cluster::new(directory.path(), 2, &BOUNDARY.to_string()).await;
            cluster.worker_arguments =
                vec!["--checkpoint-interval".to_owned(), checkpoints.to_string()];
            cluster.coordinator_arguments = [
                "--reshape",
                "by-itself",
                "--merge-distance",
                &MERGE_DISTANCE.to_string(),
                "--split-distance",
                &SPLIT_DISTANCE.to_string(),
                "--rest-seconds",
                &REST.as_secs().to_string(),
            ]
            .map(str::to_owned)
            .to_vec();
            cluster.lease_seconds = lease;
            cluster.view_distance = VIEW;
            cluster.start().await;
            // An address that was free when it was picked may have been taken by the
            // time a process listened on it, which says nothing about the server: the
            // cluster is started anew, on other addresses.
            attempt += 1;
            match cluster.taken_address(PATIENCE, LOOK).await {
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
        let started_at = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        Self {
            _turn: turn,
            directory: Some(directory),
            cluster,
            test: test.to_owned(),
            seed,
            random: Random::new(seed),
            lease: Duration::from_secs(lease.unwrap_or(5)),
            lines: lines.to_vec(),
            groups: Vec::new(),
            started: Instant::now(),
            started_at: started_at.as_secs_f64(),
            deeds: Vec::new(),
            walks: Vec::new(),
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
        report.push_str("what the coordinator began by itself, and how it ended:\n");
        for line in self.told() {
            report.push_str(&format!("  {line}\n"));
        }
        report.push_str("the bots when it failed:\n");
        let now = Instant::now();
        for group in &self.groups {
            let waits = group.progress.waits();
            for (number, bot) in group.progress.bots().iter().enumerate() {
                let unanswered = waits[number]
                    .iter()
                    .find(|wait| wait.acknowledged.is_none())
                    .map(|wait| format!("has waited {} for an answer", seconds(wait.lasted(now))));
                report.push_str(&format!(
                    "  {} {number}: {bot:?}, {}\n",
                    group.name,
                    unanswered.unwrap_or_else(|| "waits for nothing".to_owned())
                ));
            }
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

    /// Fails if a process has ended that nobody killed, or a group has: its bots end
    /// by themselves only when one of them was disconnected, saw a player twice or
    /// saw one vanish, or the ledgers and the world differ.
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
        for index in 0..self.groups.len() {
            let group = &mut self.groups[index];
            if group.playing.as_ref().is_some_and(JoinHandle::is_finished) {
                let name = group.name;
                let ended = group.playing.take().unwrap().await.unwrap();
                match ended {
                    Ok(report) => self.fail(&format!(
                        "the bots of group {name} stopped before being told to: {report}"
                    )),
                    Err(error) => {
                        self.fail(&format!("the bots of group {name} found fault: {error:#}"))
                    }
                }
            }
        }
    }

    /// Waits until `state` holds, looking after the processes and the groups
    /// meanwhile, and returns how long that took. Fails, naming `what` was waited
    /// for, if it takes longer than anything should.
    async fn until(&mut self, what: &str, state: impl FnMut(&Self) -> bool) -> Duration {
        self.until_within(PATIENCE, what, state).await
    }

    /// Waits until `state` holds as [`Follows::until`] does, for `limit` at most.
    async fn until_within(
        &mut self,
        limit: Duration,
        what: &str,
        mut state: impl FnMut(&Self) -> bool,
    ) -> Duration {
        let waiting = Instant::now();
        loop {
            self.tend().await;
            if state(self) {
                return waiting.elapsed();
            }
            if waiting.elapsed() > limit {
                self.fail(&format!("waited {limit:?} in vain until {what}"));
            }
            tokio::time::sleep(LOOK).await;
        }
    }

    /// The last routing table the coordinator logged in its present life.
    fn table(&self) -> Option<Table> {
        self.cluster.present_table()
    }

    /// The living regions of `list`, in ascending order.
    fn living(list: &RegionList) -> Vec<Region> {
        list.regions.iter().map(|info| info.region.0).collect()
    }

    /// The region that `list` has `region` absorbed by, if it has it absorbed.
    fn absorbed_by(list: &RegionList, region: Region) -> Option<Region> {
        let pair = list.absorbed.iter().find(|(gone, _)| gone.0 == region);
        pair.map(|(_, into)| into.0)
    }

    /// The regions of the world as the world store has them, which is what decides.
    async fn list(&mut self) -> RegionList {
        match self.cluster.regions().await {
            Ok(list) => list,
            Err(error) => self.fail(&format!("the world store's list cannot be read: {error}")),
        }
    }

    /// Waits until the world store's list is as `shows` wants it, and returns it.
    async fn until_the_list(
        &mut self,
        what: &str,
        shows: impl Fn(&RegionList) -> bool,
    ) -> RegionList {
        let waiting = Instant::now();
        loop {
            self.tend().await;
            let list = self.cluster.regions().await;
            if let Ok(list) = &list
                && shows(list)
            {
                return list.clone();
            }
            if waiting.elapsed() > PATIENCE {
                self.fail(&format!(
                    "waited {PATIENCE:?} in vain until the list shows that {what}; it is {list:?}"
                ));
            }
            tokio::time::sleep(LOOK).await;
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
                    && living.iter().all(|region| self.cluster.runs(*region))
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
            "the cluster is whole, with the regions {:?} shared as {:?}",
            Self::living(&list),
            self.cluster.loads()
        ));
        list
    }

    /// Waits until the cluster is whole and the workers share the regions evenly, so
    /// that the coordinator has no region to move from here on.
    async fn settled(&mut self) -> RegionList {
        let what = "every region runs and the workers share them evenly";
        let list = self.runs_and(what, |follows| follows.cluster.even()).await;
        self.served().await;
        self.note(format!(
            "the cluster is whole, with the regions {:?} shared evenly as {:?}",
            Self::living(&list),
            self.cluster.loads()
        ));
        list
    }

    /// Waits until every bot that plays has had something acknowledged that it sends
    /// from now on.
    async fn served(&mut self) {
        let sent = |follows: &Self| -> Vec<Vec<i32>> {
            let playing = follows
                .groups
                .iter()
                .filter(|group| group.playing.is_some());
            playing
                .map(|group| group.progress.bots().iter().map(|bot| bot.sent).collect())
                .collect()
        };
        let before = sent(self);
        self.until("every bot has something new acknowledged", |follows| {
            let playing = follows
                .groups
                .iter()
                .filter(|group| group.playing.is_some());
            playing.zip(&before).all(|(group, before)| {
                let bots = group.progress.bots();
                bots.iter()
                    .zip(before)
                    .all(|(bot, sent)| bot.acknowledged > *sent)
            })
        })
        .await;
    }

    /// The group called `name`.
    fn group(&self, name: &str) -> &Group {
        let found = self.groups.iter().find(|group| group.name == name);
        found.unwrap_or_else(|| panic!("there is no group {name}"))
    }

    /// Where the bots are, for a message.
    fn whereabouts(&self) -> String {
        let groups = self.groups.iter().filter(|group| group.playing.is_some());
        let groups: Vec<String> = groups
            .map(|group| {
                let bots = group.progress.bots();
                let xs: Vec<String> = bots.iter().map(|bot| format!("{:.1}", bot.x)).collect();
                format!("{} at x = {}", group.name, xs.join(" and "))
            })
            .collect();
        groups.join(", ")
    }

    /// Lets `bots` bots join as the group `name`, with their lanes from the block row
    /// `first_lane` on, to walk up and down between the x coordinates `between`: they
    /// walk there from where players enter the world without building.
    fn joins(&mut self, name: &'static str, bots: usize, first_lane: i32, between: (f64, f64)) {
        let scenario = Ledger {
            bots,
            rounds: None,
            duration: None,
            west: between.0,
            east: between.1,
            lines: self.lines.clone(),
            // Every group chooses differently, and all of them by the seed.
            seed: self.seed + self.groups.len() as u64,
            name_prefix: name.to_owned(),
            first_lane,
            pulse: Some(PULSE),
            ..Ledger::default()
        };
        let progress = Progress::new(bots);
        let playing = {
            let address = self.cluster.edge.0.clone();
            let (scenario, progress) = (scenario.clone(), progress.clone());
            tokio::spawn(async move { ledger(&address, &scenario, &progress).await })
        };
        self.groups.push(Group {
            name,
            ledger: scenario,
            progress,
            playing: Some(playing),
            report: None,
        });
        self.note(format!(
            "group {name} joins, {bots} bots, to walk between x = {} and x = {}",
            between.0, between.1
        ));
    }

    /// Sends the group `name` to walk up and down between the x coordinates `between`.
    fn walks(&mut self, name: &str, between: (f64, f64), what: &str) {
        self.group(name)
            .progress
            .walk_between(between.0, between.1)
            .unwrap();
        self.walks
            .push((Instant::now(), format!("{name} was sent {what}")));
        self.note(format!("{name} is sent {what}; {}", self.whereabouts()));
    }

    /// Sends the group `name` to stand in `chunk`.
    fn walks_to(&mut self, name: &str, chunk: i32) {
        self.walks(name, within(chunk), &format!("to chunk {chunk}"));
    }

    /// Fails unless every bot of the group `name` is within the coordinates it was
    /// last sent to, which are `between`.
    fn is_between(&mut self, name: &str, between: (f64, f64)) {
        let progress = self.group(name).progress.clone();
        let bots = progress.bots();
        let there = |x: f64| between.0 <= x && x <= between.1;
        let all = bots
            .iter()
            .all(|bot| bot.playing && bot.arrived && there(bot.x));
        if progress.between() != Some(between) || !all {
            self.fail(&format!(
                "group {name} is to be between x = {} and x = {}: {bots:?}",
                between.0, between.1
            ));
        }
    }

    /// Fails unless every bot of the group `name` stands in `chunk`, where the group
    /// was last sent.
    fn is_in(&mut self, name: &str, chunk: i32) {
        self.is_between(name, within(chunk));
    }

    /// Waits until every bot of the group `name` is within the coordinates the group
    /// was last sent to.
    async fn arrives(&mut self, name: &str) {
        let what = format!("every bot of group {name} is where the group was sent");
        let progress = self.group(name).progress.clone();
        self.until(&what, |_| progress.arrived()).await;
        let between = progress.between().expect("it was sent somewhere");
        self.is_between(name, between);
        self.note(format!("{name} has arrived; {}", self.whereabouts()));
    }

    /// Waits until every bot of the group `name` has had something acknowledged that
    /// it sent from now on, which is where it is now if it has arrived.
    async fn plays(&mut self, name: &str) {
        let progress = self.group(name).progress.clone();
        let sent: Vec<i32> = progress.bots().iter().map(|bot| bot.sent).collect();
        let what = format!("every bot of group {name} has something new acknowledged");
        self.until(&what, |_| {
            let bots = progress.bots();
            bots.iter()
                .zip(&sent)
                .all(|(bot, sent)| bot.acknowledged > *sent)
        })
        .await;
    }

    /// Waits until the slowest bot of the group `name`, which is its first, has
    /// walked from one end of where the group walks to the other and back `rounds`
    /// times.
    ///
    /// This is how the tests let time pass where the record wants nothing to happen
    /// for a while: by the bots' own steps. A bot takes a step every client tick
    /// whatever the server does, so a round is as long as the way is; on a machine
    /// that is slow the bots are slow with it, and the round with them.
    async fn walks_rounds(&mut self, name: &str, rounds: u32) {
        let group = self.group(name);
        let progress = group.progress.clone();
        let (west, east) = progress.between().expect("the group walks somewhere");
        // The rounds are what is waited for, and they take their time: this only
        // ever runs out when the bots stand.
        let ticks = 2.0 * (east - west) / group.ledger.speed * f64::from(rounds);
        let limit = PATIENCE + 3 * TICK.mul_f64(ticks);
        // An end counts as reached within a block of it, where a bot is for six ticks
        // and more: a look that comes late misses a round and costs time, no more.
        let mut walked = 0;
        let mut east_next = true;
        let what = format!("the slowest bot of group {name} has walked {rounds} rounds");
        self.until_within(limit, &what, |_| {
            let x = progress.bots()[0].x;
            if east_next && x >= east - 1.0 {
                east_next = false;
            } else if !east_next && x <= west + 1.0 {
                east_next = true;
                walked += 1;
            }
            walked >= rounds
        })
        .await;
        self.note(format!(
            "the slowest bot of {name} walked {rounds} rounds between x = {west} and x = {east}"
        ));
    }

    /// Lets the group `name` walk up and down where it walks for a minute, by the
    /// steps of its slowest bot.
    async fn walks_for_a_minute(&mut self, name: &str) {
        let group = self.group(name);
        let (west, east) = group.progress.between().expect("the group walks somewhere");
        let ticks_of_a_round = 2.0 * (east - west) / group.ledger.speed;
        let a_minute = Duration::from_secs(60).as_secs_f64() / TICK.as_secs_f64();
        self.walks_rounds(name, (a_minute / ticks_of_a_round).ceil() as u32)
            .await;
    }

    /// Tells the bots of the group `name` to stop and waits until they, and the
    /// auditor who joins then, have left. Fails unless they found everything as the
    /// ledgers say.
    async fn leaves(&mut self, name: &str) {
        let index = self.groups.iter().position(|group| group.name == name);
        let index = index.unwrap_or_else(|| panic!("there is no group {name}"));
        self.groups[index].progress.finish();
        let mut playing = self.groups[index].playing.take().expect("it still plays");
        self.note(format!("{name} is told to stop; {}", self.whereabouts()));
        let waiting = Instant::now();
        while !playing.is_finished() {
            self.tend().await;
            if waiting.elapsed() > 3 * PATIENCE {
                self.fail(&format!("the bots of group {name} did not come to an end"));
            }
            tokio::time::sleep(LOOK).await;
        }
        let report = match (&mut playing).await.unwrap() {
            Ok(report) => report,
            Err(error) => self.fail(&format!("the bots of group {name} found fault: {error:#}")),
        };
        self.note(format!("{name} has left, content: {report}"));
        if report.actions == 0 || report.blocks_audited as usize != report.blocks.len() {
            self.fail(&format!(
                "group {name} did nothing, or not every block was audited: {report}"
            ));
        }
        self.groups[index].report = Some(report);
    }

    /// The time of a line of the coordinator's log on the test's own clock.
    fn instant_of(&self, at: f64) -> Instant {
        let since = (at - self.started_at).max(0.0);
        self.started + Duration::from_secs_f64(since)
    }

    /// Everything the coordinator began by itself, in all its lives, in order, each
    /// with the time of its line.
    fn begun(&self) -> Vec<(f64, Begun)> {
        let log = self.cluster.log("coordinator");
        let lines = log.lines();
        lines
            .filter_map(|line| Some((time_of(line)?, Begun::read(line)?)))
            .collect()
    }

    /// The merges, absorptions and splits the coordinator began by itself, in order.
    fn reshapes(&self) -> Vec<Begun> {
        let begun = self.begun().into_iter().map(|(_, begun)| begun);
        begun.filter(Begun::reshapes).collect()
    }

    /// How every merge and split ended, and every region that was assigned, in order,
    /// each with the time of its line.
    fn ended(&self) -> Vec<(f64, Ended)> {
        let log = self.cluster.log("coordinator");
        let lines = log.lines();
        lines
            .filter_map(|line| Some((time_of(line)?, Ended::read(line)?)))
            .collect()
    }

    /// What the coordinator began by itself and how each merge and split ended, in
    /// the order of its log, for a message.
    fn told(&self) -> Vec<String> {
        let mut lines: Vec<(f64, String)> = Vec::new();
        for (at, begun) in self.begun() {
            lines.push((at, format!("begun: {}", begun.told())));
        }
        for (at, ended) in self.ended() {
            match ended {
                Ended::Merge {
                    survivor,
                    absorbed,
                    outcome,
                } => lines.push((
                    at,
                    format!(
                        "ended: the merge of region {absorbed} into region {survivor}: {outcome}"
                    ),
                )),
                Ended::Split { region, outcome } => {
                    lines.push((
                        at,
                        format!("ended: the split of region {region}: {outcome}"),
                    ));
                }
                Ended::Assigned { .. } => {}
            }
        }
        lines.sort_by(|one, other| one.0.total_cmp(&other.0));
        let told = lines.into_iter().map(|(at, line)| {
            let since = at - self.started_at;
            format!("{since:7.3} s: {line}")
        });
        told.collect()
    }

    /// Fails unless the merges, absorptions and splits the coordinator has begun by
    /// itself beyond the first `counted` are `expected`, where `expected` says of each
    /// whether it is the one.
    fn has_begun(&mut self, counted: usize, what: &str, expected: &[&dyn Fn(&Begun) -> bool]) {
        let reshapes = self.reshapes();
        let new = reshapes.get(counted..).unwrap_or_default();
        let as_expected =
            new.len() == expected.len() && new.iter().zip(expected).all(|(begun, is)| is(begun));
        if !as_expected {
            self.fail(&format!(
                "the coordinator was to begin {what} and nothing else by itself; it began {new:?}"
            ));
        }
    }

    /// The start of the record's table: `A` stands in chunk 0 while `B` joins and
    /// walks to chunk 6. Nothing is wanted: `B` is 6 from `A` and from the chunk
    /// players enter in, and was never within 3 of either while it was in region 1.
    /// Fails unless the stripes are still there.
    async fn the_start(&mut self) {
        self.joins("A", 2, 0, within(0));
        self.arrives("A").await;
        self.plays("A").await;
        // The workers have registered one after the other, and the first may have
        // been given both stripes; they share them before anybody else comes.
        self.settled().await;
        self.joins("B", 1, 8, within(6));
        self.arrives("B").await;
        self.plays("B").await;
        // "Nothing" is no state to wait for, so `B` walks its rounds in chunk 6
        // meanwhile: three of them are eleven seconds of its steps. A merge that was
        // wanted wrongly would have stood after one second and been begun when the
        // regions had rested, five seconds after they were given their owners, and
        // they were given them before anybody could join.
        self.walks_rounds("B", 3).await;
        self.stripes().await;
    }

    /// Fails unless the world is the two stripes it began as, each run by a worker,
    /// and the coordinator has begun no merge and no split.
    async fn stripes(&mut self) {
        let list = self.whole().await;
        let known = self.table().map(|table| table.known());
        if Self::living(&list) != [0, 1] || !list.absorbed.is_empty() || known != Some(vec![0, 1]) {
            self.fail(&format!(
                "the list and the routing table were to have the two stripes: {list:?}, {known:?}"
            ));
        }
        self.has_begun(0, "nothing", &[]);
    }

    /// Step 1 of the record's table: `A` walks to chunk 3, 3 from `B` in chunk 6, and
    /// region 0 absorbs region 1. Fails unless the list has it so, region 0 then holds
    /// every chunk, and that merge is the one thing the coordinator has begun since
    /// its first `counted` merges and splits.
    async fn step_1(&mut self, counted: usize) {
        self.walks_to("A", 3);
        let absorbed = "region 1 is absorbed by region 0";
        self.until_the_list(absorbed, |list| Self::absorbed_by(list, 1) == Some(0))
            .await;
        let list = self.whole().await;
        self.arrives("A").await;
        // Both stripes are region 0's, which is how it holds every chunk.
        if Self::living(&list) != [0] || list.regions[0].pinned.len() != 2 {
            self.fail(&format!(
                "after step 1 the list is not region 0 pinned to both stripes: {list:?}"
            ));
        }
        let merge = |begun: &Begun| {
            *begun
                == Begun::Merge {
                    survivor: 0,
                    absorbed: 1,
                    gap: 3,
                }
        };
        self.has_begun(counted, "one merge, of region 1 into region 0", &[&merge]);
    }

    /// Fails unless `list` has `part` as the region that `B` in chunk 6 was split off
    /// with when everybody else stood in chunk 0: the chunks nearer to chunk 6 than to
    /// chunk 0, which are those with x at least 4 and z between `-(x - 1)` and `x - 1`.
    fn is_the_part_from_chunk_4(&mut self, list: &RegionList, part: Region) {
        let info = list.regions.iter().find(|info| info.region.0 == part);
        let bounds = info.and_then(|info| info.bounds);
        let as_told = bounds.is_some_and(|bounds| {
            let widest = bounds.max.x - 1;
            bounds.min.x == 4
                && bounds.max.x >= 6
                && -widest <= bounds.min.z
                && bounds.max.z <= widest
        });
        if !as_told || info.is_some_and(|info| !info.pinned.is_empty()) {
            self.fail(&format!(
                "region {part} was to be granted the chunks from x = 4 on that are nearer to \
                 chunk 6 than to chunk 0, and to be pinned to nothing: {list:?}"
            ));
        }
    }

    /// Step 2 of the record's table: `A` walks to chunk 0, `B` in chunk 6 is then
    /// more than 5 from everybody and from the chunk players enter in, and is split
    /// off. Waits for the list to show a new region and returns it. Fails unless that
    /// region has the chunks from 4 on and none below, and, with `only_since`, unless
    /// one split of region 0 is the one thing the coordinator has begun since its
    /// first so many merges and splits.
    async fn step_2(&mut self, only_since: Option<usize>) -> Region {
        let before = self.list().await;
        self.walks_to("A", 0);
        let next = before.next;
        self.until_the_list("there is a new region", |list| list.next != next)
            .await;
        let list = self.whole().await;
        let part = before.next.0;
        if Self::living(&list) != [0, part] || list.next.0 != part + 1 {
            self.fail(&format!(
                "after step 2 the list was to have region 0 and the new region {part}: {list:?}"
            ));
        }
        self.is_the_part_from_chunk_4(&list, part);
        // A split is wanted only once every bot of `A` is in chunk 0: `B` is within 5
        // of a bot that is still in chunk 1.
        self.arrives("A").await;
        if let Some(counted) = only_since {
            // The chunks from 4 to 8 of the rows from -2 to 2, around the one group.
            let split = |begun: &Begun| {
                matches!(
                    begun,
                    Begun::Split {
                        region: 0,
                        groups: 1,
                        chunks: 25,
                        ..
                    }
                )
            };
            self.has_begun(counted, "one split, of region 0", &[&split]);
        }
        part
    }

    /// Step 3 of the record's table: `A` walks to chunk 3, which is region 0's and 3
    /// from `B` in the part, and region 0 absorbs the part. Fails unless the list has
    /// it so and region 0 is the one region left.
    async fn step_3(&mut self, part: Region) {
        self.walks_to("A", 3);
        let absorbed = format!("region {part} is absorbed by region 0");
        self.until_the_list(&absorbed, |list| Self::absorbed_by(list, part) == Some(0))
            .await;
        let list = self.whole().await;
        self.arrives("A").await;
        if Self::living(&list) != [0] {
            self.fail(&format!(
                "after step 3 the list has other regions than region 0: {list:?}"
            ));
        }
    }

    /// The region the bots of `stands` were in at the time `at` of the coordinator's
    /// log, by the merges and splits that had ended well by then.
    fn region_at(stands: Stands, ended: &[(f64, Ended)], at: f64) -> Region {
        let mut region = stands.first;
        for (_, ended) in ended.iter().filter(|(time, _)| *time <= at) {
            match ended {
                Ended::Merge {
                    survivor, absorbed, ..
                } if ended.well() && *absorbed == region => region = *survivor,
                Ended::Split { region: split, .. } if stands.goes && *split == region => {
                    region = ended.part().unwrap_or(region);
                }
                _ => {}
            }
        }
        region
    }

    /// E8, the bound. Of everything the coordinator began by itself from the time
    /// `from` of its log on, by the four lines the record has for it, no bot's region
    /// is in more than `1 + W / rest` in any time `W`: a player who does not walk into
    /// another region's chunks is stood still at most once in a rest. `stand` says
    /// which region each group's bots are in.
    fn nobody_is_stood_still_more_than_once_in_a_rest(&mut self, from: f64, stand: &[Stands]) {
        let ended = self.ended();
        let begun: Vec<(f64, Begun)> = self.begun();
        let begun: Vec<&(f64, Begun)> = begun.iter().filter(|(at, _)| *at >= from).collect();
        for stands in stand {
            let theirs: Vec<&(f64, Begun)> = begun
                .iter()
                .copied()
                .filter(|(at, begun)| begun.is_of(Self::region_at(*stands, &ended, *at)))
                .collect();
            self.note(format!(
                "the regions of group {} were in {} things the coordinator began by itself",
                stands.group,
                theirs.len()
            ));
            for (first, (since, earlier)) in theirs.iter().enumerate() {
                for (last, (at, later)) in theirs.iter().enumerate().skip(first + 1) {
                    let time = at - since + WRITING;
                    let may = 1.0 + time / REST.as_secs_f64();
                    let were = last - first + 1;
                    if were as f64 > may {
                        self.fail(&format!(
                            "the region of group {} was in {were} things the coordinator began \
                             by itself within {:.3} s, from {} to {}; with a rest of {REST:?} \
                             it may be in {may:.2}",
                            stands.group,
                            at - since,
                            earlier.told(),
                            later.told()
                        ));
                    }
                }
            }
        }
    }

    /// The longest any bot waited for an acknowledgement between `from` and now.
    fn longest_pause_since(&self, from: Instant) -> Duration {
        let now = Instant::now();
        let groups = self.groups.iter();
        let pauses = groups.flat_map(|group| group.progress.longest_waits(from, now));
        let lasted = pauses.flatten().map(|wait| wait.lasted(now));
        lasted.max().unwrap_or_default()
    }

    /// When every bot had had something acknowledged that it sent after `at`: the
    /// moment by which the cluster was whole again for the players. Now, if a bot has
    /// not yet.
    fn served_after(&self, at: Instant) -> Instant {
        let now = Instant::now();
        let mut served = at;
        for group in &self.groups {
            for waits in group.progress.waits() {
                let later = waits.iter().find(|wait| wait.sent > at);
                let answered = later.and_then(|wait| wait.acknowledged);
                served = served.max(answered.unwrap_or(now));
            }
        }
        served
    }

    /// What the bots noticed of everything the coordinator began by itself from the
    /// time `from` of its log on, as the tests of merges by hand measure it: the
    /// longest that anything a bot sent waited for its acknowledgement, among
    /// everything that was waiting at some moment between the thing being begun and
    /// every bot being served after it ended. And how long after the walk before it
    /// each thing was begun and had ended, by the coordinator's log. `stand` says
    /// which region each group's bots are in.
    fn measured(&self, from: f64, stand: &[Stands]) -> Vec<Measured> {
        let ended = self.ended();
        let begun = self.begun();
        let now = Instant::now();
        let mut measured = Vec::new();
        for (at, begun) in begun.iter().filter(|(at, _)| *at >= from) {
            let end = ended.iter().find(|(time, ended)| {
                *time >= *at
                    && match (begun, ended) {
                        (
                            Begun::Merge {
                                survivor, absorbed, ..
                            }
                            | Begun::Absorption { survivor, absorbed },
                            Ended::Merge {
                                survivor: into,
                                absorbed: gone,
                                ..
                            },
                        ) => (survivor, absorbed) == (into, gone),
                        (Begun::Split { region, .. }, Ended::Split { region: split, .. }) => {
                            region == split
                        }
                        (Begun::Move { region, .. }, Ended::Assigned { region: given, .. }) => {
                            region == given
                        }
                        _ => false,
                    }
            });
            let begun_at = self.instant_of(*at);
            let done_at = end.map(|(time, _)| self.instant_of(*time));
            let served = self.served_after(done_at.unwrap_or(begun_at));
            let (mut stayed, mut went): (Option<Duration>, Option<Duration>) = (None, None);
            for stands in stand {
                let region = Self::region_at(*stands, &ended, *at);
                let goes = match begun {
                    Begun::Merge { absorbed, .. } | Begun::Absorption { absorbed, .. } => {
                        region == *absorbed
                    }
                    Begun::Split { region: split, .. } => stands.goes && region == *split,
                    Begun::Move { region: moved, .. } => region == *moved,
                };
                // At a move everybody else is somebody whose region went on; of a
                // merge and a split only the players of the regions it is about.
                let concerned = begun.is_of(region) || matches!(begun, Begun::Move { .. });
                if !concerned {
                    continue;
                }
                let waits = self
                    .group(stands.group)
                    .progress
                    .longest_waits(begun_at, served);
                let longest = waits.iter().flatten().map(|wait| wait.lasted(now)).max();
                let of = if goes { &mut went } else { &mut stayed };
                *of = (*of).max(longest);
            }
            let walk = self.walks.iter().rev().find(|(sent, _)| *sent <= begun_at);
            let walk = walk.map(|(sent, what)| (*sent, what.clone()));
            let split = ended.iter().rev().find(|(time, ended)| {
                *time <= *at && matches!(ended, Ended::Split { .. }) && ended.well()
            });
            let split = split.map(|(time, _)| (self.instant_of(*time), "a split ended".to_owned()));
            let cause = if matches!(begun, Begun::Move { .. }) {
                split
            } else {
                walk
            };
            let after = |later: Instant| {
                let (since, _) = cause.as_ref()?;
                Some(later.saturating_duration_since(*since))
            };
            measured.push(Measured {
                what: begun.told(),
                begun_after: after(begun_at),
                done_after: done_at.and_then(after),
                cause: cause.as_ref().map(|(_, what)| what.clone()),
                stayed,
                went,
            });
        }
        measured
    }

    /// Sends a process a signal.
    async fn signal(process: &Child, signal: &str) {
        let pid = process.id().expect("it runs").to_string();
        let sent = Command::new("kill")
            .args([format!("-{signal}"), pid])
            .status()
            .await;
        assert!(sent.unwrap().success());
    }

    /// Ends every group that still plays, each with its auditor, and fails unless all
    /// found everything as the ledgers say. Then every process is killed and started
    /// again, and the list of regions and every block the ledgers have a word about
    /// have to be on disk as they were. Then the processes are asked to stop, which
    /// each has to do cleanly.
    async fn finish(mut self) {
        let playing = self.groups.iter().filter(|group| group.playing.is_some());
        let playing: Vec<&'static str> = playing.map(|group| group.name).collect();
        for name in playing {
            self.leaves(name).await;
        }
        self.tend().await;
        for line in self.told() {
            println!("{}: {line}", self.test);
        }

        // Then the power goes. The coordinator goes on merging and splitting for as
        // long as it runs, so the list is read when only the store is left, which
        // does nothing by itself, and again from the store that is started on the
        // same disk.
        for (name, process) in self.cluster.processes() {
            if name != "worldstore"
                && let Some(mut process) = process.take()
            {
                process.kill().await.unwrap();
            }
        }
        let before = self.list().await;
        self.cluster.kill().await;
        self.note("killed every process".to_owned());
        self.cluster.start_store();
        let waiting = Instant::now();
        let after = loop {
            match self.cluster.regions().await {
                Ok(list) => break list,
                Err(error) if waiting.elapsed() > PATIENCE => {
                    self.fail(&format!("the world store does not come back: {error}"))
                }
                Err(_) => tokio::time::sleep(LOOK).await,
            }
        };
        if after != before {
            self.fail(&format!(
                "the regions are others after the world store was killed and started again: \
                 {before:?} before, {after:?} after"
            ));
        }
        self.cluster.kill().await;
        self.cluster.start().await;
        let address = self.cluster.edge.0.clone();
        for index in 0..self.groups.len() {
            let group = &self.groups[index];
            let report = group.report.as_ref().expect("every group has left");
            if let Err(error) = audit_blocks(&address, &group.ledger, &report.blocks).await {
                let name = group.name;
                self.fail(&format!(
                    "after every process was killed and started again, what group {name} built \
                     is not as its ledgers say: {error:#}"
                ));
            }
        }
        self.note("the world is as the ledgers say after starting from disk".to_owned());
        self.tend().await;
        self.stop_everything().await;
        if std::env::var_os("CLUSTINE_FOLLOWS_KEEP").is_some() {
            let kept = self.keep();
            println!("{}: kept: {}", self.test, kept.display());
        }
    }

    /// Asks every process to stop and fails unless each ends without an error: the
    /// edge, then the coordinator, then the workers and the world store. With the
    /// coordinator gone a worker has nobody to hand its regions to and knows it, so it
    /// saves and stops instead of waiting for someone to take over.
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
            Self::signal(&process, "TERM").await;
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

/// E1. The start and step 1: `A` stands in chunk 0 while `B` joins and walks to chunk
/// 6, and the stripes stay; `A` walks to chunk 3, and region 0 absorbs region 1. From
/// the start to the end of step 1 the coordinator began one merge, of these two, and
/// no split. With the lease a coordinator has when it is not told any.
#[tokio::test(flavor = "multi_thread")]
async fn two_stripes_are_merged_when_a_group_of_the_one_comes_within_three_chunks_of_the_other() {
    if a_repetition() {
        return;
    }
    let mut follows = Follows::start("stripes merged", None, &[16 * BOUNDARY]).await;
    follows.the_start().await;
    follows.step_1(0).await;
    // They play on in the one region, `A` beside the line that is no more and
    // reaching across it.
    follows.plays("A").await;
    follows.plays("B").await;
    follows.finish().await;
}

/// E2. After the start and step 1, step 2: `A` walks back to chunk 0 and stands, and
/// `B` in chunk 6 is split off region 0, into a new region with the chunks from x = 4
/// on. Then the new region is moved to the other worker and region 0 is not, no sooner
/// than a rest after the split ended.
///
/// The lease is 3 s and the rest 5 s, which is what this is about: evening out as it
/// was before the coordinator decided by itself moved a region a lease after any
/// merge or split, and would move the part two seconds too soon here. Which of the
/// two regions is moved does not tell the two apart, as the record says.
#[tokio::test(flavor = "multi_thread")]
async fn a_group_that_is_left_behind_is_split_off_and_its_region_is_moved_only_after_a_rest() {
    if a_repetition() {
        return;
    }
    let mut follows = Follows::start("split and moved", Some(3), &[16 * BOUNDARY]).await;
    follows.the_start().await;
    follows.step_1(0).await;

    let Some(made_by) = follows.cluster.owner(0) else {
        follows.fail("region 0 has no owner though the cluster was whole");
    };
    let moves_before = follows.begun().len() - follows.reshapes().len();
    let part = follows.step_2(Some(1)).await;
    follows
        .until("the new region is run by the other worker", |follows| {
            let owner = follows.cluster.owner(part);
            owner.is_some_and(|owner| owner != made_by) && follows.cluster.runs(part)
        })
        .await;
    follows.whole().await;
    // `A` has stood all the while.
    follows.is_in("A", 0);
    follows.is_in("B", 6);

    // One release to even out, of the part, from the worker that made it.
    let begun = follows.begun();
    let moves: Vec<&(f64, Begun)> = begun
        .iter()
        .filter(|(_, begun)| !begun.reshapes())
        .collect();
    let (from, to) = (worker_name(made_by), worker_name(1 - made_by));
    let the_move = Begun::Move {
        region: part,
        from,
        to,
    };
    let last = moves.last().map(|(at, begun)| (*at, begun.clone()));
    let Some((moved_at, moved)) = last else {
        follows.fail("the coordinator's log has no line for the move of the new region");
    };
    if moves.len() != moves_before + 1
        || moved != the_move
        || follows.cluster.owner(0) != Some(made_by)
    {
        let loads = follows.cluster.loads();
        follows.fail(&format!(
            "the coordinator was to move region {part} to the other worker, and nothing else: \
             it moved {moves:?}, and the workers run {loads:?}"
        ));
    }
    let ended = follows.ended();
    let said = ended.iter().rev().find(|(_, ended)| {
        matches!(ended, Ended::Split { region: 0, .. }) && ended.part() == Some(part)
    });
    let Some((said_at, _)) = said else {
        follows.fail("the coordinator's log has no line for what came of the split of region 0");
    };
    let rested = moved_at - said_at;
    follows.note(format!(
        "the release of region {part} was begun {rested:.3} s after the split ended"
    ));
    if rested + WRITING < REST.as_secs_f64() {
        follows.fail(&format!(
            "region {part} was released to even out {rested:.3} s after the split that made it \
             ended; it was to be left alone for a rest of {REST:?}"
        ));
    }
    follows.plays("A").await;
    follows.plays("B").await;
    follows.finish().await;
}

/// How many rounds a test does: the variable called `name` if set, else `default`.
fn rounds_from(name: &str, default: u32) -> u32 {
    number_from(name).map_or(default, |rounds| rounds as u32)
}

/// E3 and E8. After the start and the steps 1 and 2, step 3 and then ten rounds of the
/// steps 2 and 3 with the same bots: `A` walks back and forth between chunk 0 and
/// chunk 3, `B` stands in chunk 6 and is split off and merged back each time. A step
/// is done when the list shows what it was to bring. From step 3 on, eleven merges and
/// ten splits end well and no more; what came to nothing is printed.
///
/// Over the same rounds, the bound: of the merges, absorptions, splits and releases to
/// even out that the coordinator began by itself, no bot's region is in more than
/// `1 + W / rest` in any time `W`. `A` walks only in the chunks 0 to 3, which are
/// region 0's throughout, and `B` stays in chunk 6: nobody is handed over, so nothing
/// is excepted. And no bot waits longer for an acknowledgement than it may at a merge
/// or a split.
///
/// Whether the part of a round is moved to even out before it is merged depends on
/// how fast the bots walk, and is not counted. What each merge, split and move cost
/// the bots is printed.
#[tokio::test(flavor = "multi_thread")]
async fn regions_follow_two_groups_through_ten_rounds_and_stand_nobody_still_twice_in_a_rest() {
    if a_repetition() {
        return;
    }
    let mut follows = Follows::start("ten rounds", None, &[16 * BOUNDARY]).await;
    follows.the_start().await;
    follows.step_1(0).await;
    let mut part = follows.step_2(Some(1)).await;

    // What is counted begins here. `B` was handed over to region 1 on its way to
    // chunk 6 and has been in region 0 since step 1, and in the part since step 2.
    let from = follows.started_at + follows.started.elapsed().as_secs_f64();
    let counting = Instant::now();
    let ended_before = follows.ended().len();
    let stand = [
        Stands {
            group: "A",
            first: 0,
            goes: false,
        },
        Stands {
            group: "B",
            first: part,
            goes: true,
        },
    ];
    let rounds = rounds_from("CLUSTINE_FOLLOWS_ROUNDS", 10);
    follows.step_3(part).await;
    for round in 1..=rounds {
        part = follows.step_2(None).await;
        follows.step_3(part).await;
        follows.note(format!("round {round} of {rounds} is done"));
    }
    follows.is_in("A", 3);
    follows.is_in("B", 6);

    // E3: what ended well, and what came to nothing.
    let ended = follows.ended();
    let ended: Vec<&Ended> = ended[ended_before..]
        .iter()
        .map(|(_, ended)| ended)
        .collect();
    let well = |merge: bool| {
        let of_a_kind = ended.iter().filter(|ended| match ended {
            Ended::Merge { .. } => merge,
            Ended::Split { .. } => !merge,
            Ended::Assigned { .. } => false,
        });
        of_a_kind.filter(|ended| ended.well()).count() as u32
    };
    let (merges, splits) = (well(true), well(false));
    let nothing: Vec<String> = ended
        .iter()
        .filter(|ended| !ended.well() && !matches!(ended, Ended::Assigned { .. }))
        .map(|ended| format!("{ended:?}"))
        .collect();
    follows.note(format!(
        "{merges} merges and {splits} splits ended well; what came to nothing: {nothing:?}"
    ));
    let list = follows.list().await;
    if (merges, splits) != (rounds + 1, rounds)
        || Follows::living(&list) != [0]
        || list.absorbed.len() as u32 != rounds + 2
        || list.next.0 != rounds + 3
    {
        follows.fail(&format!(
            "from step 3 on, {} merges and {rounds} splits were to end well and no more: \
             {merges} merges and {splits} splits did, and the list is {list:?}",
            rounds + 1
        ));
    }

    // E8: the bound, and the longest wait.
    follows.nobody_is_stood_still_more_than_once_in_a_rest(from, &stand);
    let longest = follows.longest_pause_since(counting);
    follows.note(format!(
        "the longest any bot waited for an acknowledgement in these rounds was {}",
        seconds(longest)
    ));
    if longest > LONGEST_PAUSE {
        follows.fail(&format!(
            "a bot waited {} for an acknowledgement; at a merge or a split it may wait \
             {LONGEST_PAUSE:?}",
            seconds(longest)
        ));
    }

    // What it cost, for whoever reads the output.
    let seed = follows.seed;
    println!(
        "what the coordinator began by itself in this run (seed {seed}), and what the bots noticed:"
    );
    println!(
        "  what | after what | begun | done | those who stayed waited | those who went waited"
    );
    for measured in follows.measured(from, &stand) {
        println!(
            "  {} | {} | {} | {} | {} | {}",
            measured.what,
            measured.cause.as_deref().unwrap_or("-"),
            perhaps(measured.begun_after),
            perhaps(measured.done_after),
            perhaps(measured.stayed),
            perhaps(measured.went)
        );
    }
    follows.finish().await;
}

/// E4. Players who walk along each other without crossing a distance are left alone.
/// `A` walks up and down between the chunks 0 and 1 and `B` between 5 and 6, in two
/// regions, 4 to 6 apart and never 3, for a minute: no merge and no split. Then `A`
/// walks up and down between 2 and 3: one merge, when a bot of `A` and `B` have been
/// 3 apart or nearer for a second. Then `A` walks up and down between 1 and 2 and `B`
/// goes on as it was, in one region, 3 to 5 apart, for a minute: no split and no
/// merge.
///
/// "Nothing for a minute" is bounded by the rounds the slowest bot of `A` walks: as
/// many as are a minute of its steps. Each round takes `A` through every distance
/// from `B` that its walk has, and a thing that was wanted wrongly would stand after
/// a second and be begun within a rest, five seconds, so it would show many times
/// over.
#[tokio::test(flavor = "multi_thread")]
async fn groups_that_walk_along_each_other_are_merged_once_and_never_split() {
    if a_repetition() {
        return;
    }
    let mut follows = Follows::start("along each other", None, &[16 * BOUNDARY]).await;
    follows.joins("A", 2, 0, within(0));
    follows.arrives("A").await;
    follows.plays("A").await;
    follows.settled().await;
    follows.joins("B", 1, 8, between(5, 6));
    follows.arrives("B").await;
    follows.plays("B").await;

    follows.walks("A", between(0, 1), "to walk between the chunks 0 and 1");
    follows.arrives("A").await;
    follows.walks_for_a_minute("A").await;
    follows.stripes().await;

    follows.walks("A", between(2, 3), "to walk between the chunks 2 and 3");
    let absorbed = "region 1 is absorbed by region 0";
    follows
        .until_the_list(absorbed, |list| Follows::absorbed_by(list, 1) == Some(0))
        .await;
    follows.whole().await;
    let merge = |begun: &Begun| {
        matches!(
            begun,
            Begun::Merge { survivor: 0, absorbed: 1, gap } if *gap <= MERGE_DISTANCE
        )
    };
    follows.has_begun(0, "one merge, of region 1 into region 0", &[&merge]);

    follows.walks("A", between(1, 2), "to walk between the chunks 1 and 2");
    follows.arrives("A").await;
    follows.walks_for_a_minute("A").await;
    let list = follows.whole().await;
    follows.has_begun(1, "nothing more", &[]);
    if Follows::living(&list) != [0] || list.next.0 != 2 {
        follows.fail(&format!(
            "the one region was to stay one region, and none was to be made: {list:?}"
        ));
    }
    follows.is_between("A", between(1, 2));
    follows.is_between("B", between(5, 6));
    follows.finish().await;
}

/// E5. A region that its players have left is absorbed at nobody's expense. `A`
/// stands in chunk 0, `B` joins and walks west to chunk -6 and is split off, into
/// region 2 with the chunks up to -4. `B` leaves the game, and `C` joins and stands
/// in chunk 0. The list then shows region 2 absorbed, and what is checked is which
/// regions that merge was of: region 2 went into region 1, the east stripe, which is
/// pinned, never had a player and has the lower id; region 0, where `A` and `C`
/// stand, was in no merge.
///
/// One thing is done here that the record does not say: before `B` leaves, it walks
/// on to chunk -8, which is the part's as well. Leaving brings `B`'s auditor in, who
/// walks from the chunk players enter in to where `B` began, through region 0's
/// chunks as far as chunk -3 and as region 0's player until the part has taken them
/// over at chunk -4. With `B` in chunk -6 that is a player of region 0 within 3 of a
/// player of region 2 for a second or so, and whether the merge of the two is wanted
/// for long enough to be begun is a matter of milliseconds. From chunk -8 it is 4.
#[tokio::test(flavor = "multi_thread")]
async fn a_region_that_its_players_left_is_absorbed_by_a_region_without_players() {
    if a_repetition() {
        return;
    }
    // The part and region 0 meet west of chunk -3, which stays.
    let lines = [16 * -3, 16 * BOUNDARY];
    let mut follows = Follows::start("left empty", None, &lines).await;
    follows.joins("A", 2, 0, within(0));
    follows.arrives("A").await;
    follows.plays("A").await;
    follows.settled().await;

    follows.joins("B", 1, 8, within(-6));
    follows
        .until_the_list("there is a new region", |list| list.next.0 != 2)
        .await;
    let list = follows.whole().await;
    follows.arrives("B").await;
    // The chunks nearer to chunk -6 than to chunk 0: chunk -3 is 3 from both and
    // stays.
    let part = list.regions.iter().find(|info| info.region.0 == 2);
    let bounds = part.and_then(|info| info.bounds);
    let as_told = bounds.is_some_and(|bounds| {
        let widest = -bounds.min.x - 1;
        bounds.max.x == -4
            && bounds.min.x <= -6
            && -widest <= bounds.min.z
            && bounds.max.z <= widest
    });
    if Follows::living(&list) != [0, 1, 2] || !as_told {
        follows.fail(&format!(
            "region 2 was to be split off with the chunks up to x = -4 that are nearer to \
             chunk -6 than to chunk 0: {list:?}"
        ));
    }
    let split = |begun: &Begun| {
        matches!(
            begun,
            Begun::Split {
                region: 0,
                groups: 1,
                chunks: 25,
                ..
            }
        )
    };
    follows.has_begun(0, "one split, of region 0", &[&split]);

    follows.walks_to("B", -8);
    follows.arrives("B").await;
    follows.plays("B").await;
    follows.leaves("B").await;
    follows.joins("C", 1, 12, within(0));
    follows.arrives("C").await;
    follows.plays("C").await;

    let list = follows
        .until_the_list("region 2 is absorbed", |list| {
            Follows::absorbed_by(list, 2).is_some()
        })
        .await;
    // By where the test knows its bots to be, neither of the two has a bot in it.
    follows.is_in("A", 0);
    follows.is_in("C", 0);
    let absorption = |begun: &Begun| {
        *begun
            == Begun::Absorption {
                survivor: 1,
                absorbed: 2,
            }
    };
    follows.has_begun(1, "one absorption, of region 2 by region 1", &[&absorption]);
    let of_region_0 = follows.ended().into_iter().any(|(_, ended)| {
        matches!(ended, Ended::Merge { survivor, absorbed, .. } if survivor == 0 || absorbed == 0)
    });
    if Follows::absorbed_by(&list, 2) != Some(1) || of_region_0 {
        follows.fail(&format!(
            "region 2 was to be absorbed by region 1, and region 0 to be in no merge: {list:?}"
        ));
    }
    let list = follows.whole().await;
    if Follows::living(&list) != [0, 1] {
        follows.fail(&format!(
            "the list was to have the regions 0 and 1 and no other: {list:?}"
        ));
    }
    follows.plays("A").await;
    follows.plays("C").await;
    follows.finish().await;
}

/// What a worker logs at the moments of a merge at which a process is killed: the
/// worker of the absorbed region when it is told to release it, and the survivor's
/// when it is told to absorb, when it hands the merge to the world store and when the
/// merge is done.
const RELEASING: &str = "asked to release the region";
const ABSORBING: &str = "asked to have the region absorb another";
const HANDING: &str = "handing the store a merge or a split";
const MERGED: &str = "the merge has ended";

/// And of a split: when the worker is told, when it hands the split to the store, and
/// when the split is made and it opens the new region.
const SPLITTING: &str = "asked to split the region";
const OPENING: &str = "the split has ended; opening the new region";

impl Follows {
    /// Waits until a worker that was started has registered with the coordinator,
    /// whether or not it was given a region.
    async fn registered(&mut self, worker: usize) {
        self.until("a worker that was started has registered", |follows| {
            let log = follows.cluster.log_since(worker);
            log.contains("waiting to be given a region") || log.contains("given a region")
        })
        .await;
        self.note(format!("{} has registered", worker_name(worker)));
    }

    /// Waits until the worker `watched` has logged `moment` more than `before` times,
    /// and `then` likewise if there is one; then kills `victim` a moment later that
    /// the seed chooses. Returns when it was killed.
    async fn kills_at(
        &mut self,
        watched: usize,
        (moment, before): (&str, usize),
        then: Option<(&str, usize)>,
        victim: Victim,
    ) -> Instant {
        let mut last = moment;
        let what = format!("{} has logged `{moment}`", worker_name(watched));
        self.until(&what, |follows| {
            follows.cluster.said(watched, moment) > before
        })
        .await;
        if let Some((moment, before)) = then {
            let what = format!("{} has logged `{moment}`", worker_name(watched));
            self.until(&what, |follows| {
                follows.cluster.said(watched, moment) > before
            })
            .await;
            last = moment;
        }
        // How long after the line the process dies is what the seed varies here, not
        // a wait for anything to come about.
        let delay = Duration::from_millis(self.random.below(30));
        tokio::time::sleep(delay).await;
        let why = format!("{delay:?} after {} logged `{last}`", worker_name(watched));
        let whereabouts = self.whereabouts();
        match victim {
            Victim::Worker(worker) => {
                self.cluster.kill_worker(worker).await;
                let name = worker_name(worker);
                self.note(format!("killed {name}, {why}; {whereabouts}"));
            }
            Victim::Store => {
                self.cluster.kill_and_start_the_store().await;
                self.note(format!(
                    "killed the world store, {why}, and started it again; {whereabouts}"
                ));
            }
            Victim::Coordinator => {
                self.cluster.kill_and_start_the_coordinator().await;
                self.note(format!(
                    "killed the coordinator, {why}, and started another; {whereabouts}"
                ));
            }
        }
        Instant::now()
    }

    /// Waits, after `victim` was killed at `killed`, for every region the list has to
    /// be run again; a worker that was killed is started again at once or only then,
    /// as the seed has it. Fails unless every region runs within two leases and a
    /// moment of the kill, which is what the tests of merges by hand allow. Returns
    /// the list as it was when everything ran.
    async fn gets_over(&mut self, victim: Victim, killed: Instant) -> RegionList {
        let at_once = self.random.one_in(2);
        let worker = match victim {
            Victim::Worker(worker) => Some(worker),
            _ => None,
        };
        if let Some(worker) = worker.filter(|_| at_once) {
            self.cluster.start_worker_again(worker);
            self.note(format!("started {}", worker_name(worker)));
        }
        let list = self.everything_runs().await;
        let took = killed.elapsed();
        self.note(format!(
            "every region ran again {} after {victim:?} was killed: {:?}",
            seconds(took),
            Self::living(&list)
        ));
        let may = 2 * self.lease + MOMENT;
        if took > may {
            self.fail(&format!(
                "every region was run again only {} after {victim:?} was killed; two leases \
                 and a moment are {may:?}",
                seconds(took)
            ));
        }
        if let Some(worker) = worker {
            if !at_once {
                self.cluster.start_worker_again(worker);
                self.note(format!("started {}", worker_name(worker)));
            }
            self.registered(worker).await;
        }
        list
    }

    /// The process that `harm` is, when `survivor` and `absorbed` are the workers of
    /// the two regions of a merge, or both the worker of the region that is split.
    fn victim(harm: Harm, survivor: usize, absorbed: usize) -> Victim {
        match harm {
            Harm::Survivor => Victim::Worker(survivor),
            Harm::Absorbed => Victim::Worker(absorbed),
            Harm::Store => Victim::Store,
            Harm::Coordinator => Victim::Coordinator,
        }
    }

    /// Step 2 with a process killed in the middle: `A` walks to chunk 0 from chunk 3
    /// and stands, the coordinator splits `B` off region 0 by itself, and `harm` is
    /// done at a logged moment of that split which the seed chooses. Fails unless
    /// every region runs again in time, the split is whole or not at all by the
    /// list, and afterwards, which can take as long as the coordinator leaves a
    /// region alone after an attempt that failed and a rest, the regions are what
    /// the record's table says after step 2. Returns the new region.
    async fn step_2_with_a_kill(&mut self, harm: Harm) -> Region {
        let before = self.settled().await;
        let Some(owner) = self.cluster.owner(0) else {
            self.fail("region 0 has no owner though the cluster was whole");
        };
        let victim = Self::victim(harm, owner, owner);
        let part = before.next.0;
        let running = format!("running a region region={part} ");
        let (moment, then) = match self.random.below(4) {
            0 => (SPLITTING, None),
            1 => (HANDING, None),
            2 => (OPENING, None),
            // When the worker has just begun to run the part.
            _ => (OPENING, Some(running.as_str())),
        };
        let said = (moment, self.cluster.said(owner, moment));
        let then = then.map(|moment| (moment, self.cluster.said(owner, moment)));
        self.walks_to("A", 0);
        let killed = self.kills_at(owner, said, then, victim).await;
        let now = self.gets_over(victim, killed).await;
        let found = match Self::living(&now).as_slice() {
            [0] if now.next == before.next => Found::AsBefore,
            [0, new] if *new == part && now.next.0 == part + 1 => Found::AsAfter,
            _ => self.fail(&format!(
                "the list is neither as before the split nor as after it: {now:?}"
            )),
        };
        self.note(format!("after the split the regions are {found:?}"));

        let waiting = Instant::now();
        let next = before.next;
        self.until_the_list("there is a new region", |list| list.next != next)
            .await;
        let list = self.settled().await;
        self.note(format!(
            "the regions are as after step 2 {} after everything ran again",
            seconds(waiting.elapsed())
        ));
        // `A` stands where it arrives.
        self.arrives("A").await;
        self.is_in("A", 0);
        self.is_in("B", 6);
        if Self::living(&list) != [0, part] {
            self.fail(&format!(
                "after step 2 the list was to have region 0 and the new region {part}: {list:?}"
            ));
        }
        self.is_the_part_from_chunk_4(&list, part);
        part
    }

    /// Step 3 with a process killed in the middle: `A` walks to chunk 3 and stands,
    /// the coordinator has region 0 absorb `part` by itself, and `harm` is done at a
    /// logged moment of that merge which the seed chooses, to the worker that the
    /// routing table has for the region then. Fails unless every region runs again in
    /// time, the merge is whole or not at all by the list, and afterwards the regions
    /// are what the record's table says after step 3.
    async fn step_3_with_a_kill(&mut self, part: Region, harm: Harm) {
        // The part was made by region 0's worker; the two are run by different
        // workers once the workers share the regions evenly.
        self.settled().await;
        let (Some(survivor), Some(releasing)) = (self.cluster.owner(0), self.cluster.owner(part))
        else {
            self.fail("a region has no owner though the cluster was whole");
        };
        let victim = Self::victim(harm, survivor, releasing);
        let (watched, moment) = match self.random.below(4) {
            0 => (releasing, RELEASING),
            1 => (survivor, ABSORBING),
            2 => (survivor, HANDING),
            _ => (survivor, MERGED),
        };
        let said = (moment, self.cluster.said(watched, moment));
        self.walks_to("A", 3);
        let killed = self.kills_at(watched, said, None, victim).await;
        let now = self.gets_over(victim, killed).await;
        let gone = Self::absorbed_by(&now, part) == Some(0);
        let found = match Self::living(&now).as_slice() {
            [0] if gone => Found::AsAfter,
            [0, other] if *other == part && !gone => Found::AsBefore,
            _ => self.fail(&format!(
                "the list is neither as before the merge nor as after it: {now:?}"
            )),
        };
        self.note(format!("after the merge the regions are {found:?}"));

        let waiting = Instant::now();
        let absorbed = format!("region {part} is absorbed by region 0");
        self.until_the_list(&absorbed, |list| Self::absorbed_by(list, part) == Some(0))
            .await;
        let list = self.settled().await;
        self.note(format!(
            "the regions are as after step 3 {} after everything ran again",
            seconds(waiting.elapsed())
        ));
        // The merge is begun when the first bot of `A` is in chunk 3; the other
        // follows, and `A` stands where it arrives.
        self.arrives("A").await;
        self.is_in("A", 3);
        self.is_in("B", 6);
        if Self::living(&list) != [0] {
            self.fail(&format!(
                "after step 3 the list has other regions than region 0: {list:?}"
            ));
        }
    }
}

/// The start and step 1, and then rounds of step 2 and step 3, each with what
/// `harm` says for its round done in the middle: a process killed at a logged moment
/// of a split and of a merge that the coordinator began by itself. The lease is the
/// shortest there is, as in the tests of merges by hand that kill: it is what
/// everybody waits for after a kill.
async fn regions_follow_their_players_with_kills(test: &str, harm: impl Fn(u32) -> (Harm, Harm)) {
    let mut follows = Follows::start(test, Some(3), &[16 * BOUNDARY]).await;
    follows.the_start().await;
    follows.step_1(0).await;
    for round in 0..rounds_from("CLUSTINE_FOLLOWS_KILLS", 2) {
        let (at_the_split, at_the_merge) = harm(round);
        let part = follows.step_2_with_a_kill(at_the_split).await;
        follows.step_3_with_a_kill(part, at_the_merge).await;
    }
    follows.plays("A").await;
    follows.plays("B").await;
    follows.finish().await;
}

/// E6. A worker is killed at a logged moment of a merge and of a split that the
/// coordinator began by itself: the worker of the region that is split, and the
/// worker of the survivor or of the absorbed region, by the routing table. Every
/// region runs again within two leases and a moment, the merge or the split is whole
/// or not at all by the list, and afterwards the regions are again what the record's
/// table says after that step, with nobody disconnected and the ledgers equal to the
/// world.
///
/// The record says that the last can take as long as a region is left alone after an
/// attempt that failed, and a rest. How long it took is noted and not held to that:
/// it took twice as long in two runs of ten, which is what the end of this file is
/// about.
#[tokio::test(flavor = "multi_thread")]
async fn regions_follow_their_players_when_a_worker_is_killed_during_a_merge_or_a_split() {
    if a_repetition() {
        return;
    }
    regions_follow_their_players_with_kills("kills", |round| {
        // The worker that splits is the survivor's, as the part is not there yet.
        let at_the_merge = [Harm::Survivor, Harm::Absorbed][(round % 2) as usize];
        (Harm::Survivor, at_the_merge)
    })
    .await;
}

/// E7. The same with the coordinator killed and started again with the same
/// arguments, which knows nothing of what the one before had begun, and with the
/// world store killed and started again.
#[tokio::test(flavor = "multi_thread")]
async fn regions_follow_their_players_when_the_coordinator_or_the_store_is_killed_during_a_merge_or_a_split()
 {
    if a_repetition() {
        return;
    }
    regions_follow_their_players_with_kills("other kills", |round| {
        if round % 2 == 0 {
            (Harm::Coordinator, Harm::Store)
        } else {
            (Harm::Store, Harm::Coordinator)
        }
    })
    .await;
}

// What these tests found, and what the test below keeps: the sequence, what the record
// said, and what was decided.
//
// **A split that the store made is counted as an attempt that failed when its worker
// died before it could say so** (`Coordinator::lapse_split` and `note_split_ended` in
// `services/coordinator/src/state.rs` and `state/follow.rs`).
//
// 1. The home region has players at the chunk players enter in and a group more than
//    the split distance away. The coordinator begins a split by itself and orders
//    `SplitOff`; the region's runner hands the store the split, and the store makes
//    it: its list has the new region.
// 2. The worker dies before its `SplitEnded` reaches the coordinator. (Here it is
//    killed some milliseconds after it logged `handing the store a merge or a split`.)
// 3. A lease later the region is no longer that owner's. The reservation ends as
//    `Disowned` ("a split no longer holds, as the region is not that owner's"), the
//    list is read and shows the part, and both regions are given to the other worker,
//    which restores them. So far as ADR-0014 has it, and nobody is disconnected.
// 4. The coordinator notes the split as one that came to nothing: the region has a
//    failure counted and is left alone for `LONG`, three rests, not for one.
//
// The record said two things of it. Section 9, K7, said of a merge whose worker died
// that it rests if it was made and is left alone for `LONG` if not, and "the same"
// of the worker of a region that is being split. Section 5.5 lists `Disowned` among
// the ways a split comes to nothing, and the coordinator goes by that: nothing but
// the worker's word says that a split was made, as a region with the id that was
// ordered can be another split's (ADR-0014). **The record was changed, not the
// coordinator**: K7 now says of a split what section 5.5 says.
//
// What a player notices: for `LONG` after a worker died in the middle of a split,
// half a minute with the numbers of section 3, nothing is merged into the region
// that was split and nobody else is split off it, although the split was made. That
// is the home region as a rule. And because a failure was counted, the next attempt
// that does fail is left alone for twice `LONG`. That is how this was seen: the test
// above that kills workers (seed 554989) killed the worker as it handed the store
// the split of step 2 and the survivor's worker in the merge of step 3 after it, and
// `A` and `B` stood three chunks apart in two regions for 30 s until the merge was
// tried again, where one failure costs 15 s. They saw each other across the boundary
// all that time.

/// On the coordinator's state machine alone, with the time handed in: a region is
/// split by itself, the store makes the split, and the region's worker dies before
/// it says so. The region is left alone for `LONG` from when the reservation ended,
/// as after any split that came to nothing, whatever the list shows (K7).
#[test]
fn a_region_that_was_split_as_its_worker_died_is_left_alone_as_after_a_split_that_failed() {
    let start = Instant::now();
    let config = CoordinatorConfig {
        // One stripe: region 0 is the home region and holds everything.
        layout: Layout::new(Vec::new()).expect("a world of one stripe"),
        spawn: Vec3::new(0.5, 64.0, 0.5),
        lease: Duration::from_secs(3),
        follow: Some(Policy {
            merge_distance: MERGE_DISTANCE,
            split_distance: SPLIT_DISTANCE,
            rest: REST,
        }),
    };
    let mut coordinator = Coordinator::new(config, start, 1_000);
    let home = RegionId(0);
    let mut list = RegionList {
        home,
        regions: vec![RegionInfo {
            region: home,
            epoch: 0,
            bounds: None,
            pinned: vec![ChunkArea::EVERYWHERE],
        }],
        absorbed: Vec::new(),
        next: RegionId(1),
    };
    for name in ["a", "b"] {
        let registered = coordinator.register(start, name, &format!("{name}:25600"), &[], None);
        registered.expect("it divides the world as the coordinator does");
    }
    // Two players stand in the chunk players enter in and one in chunk 6, as `A`
    // and `B` do after step 2 of the record's table.
    let mut crowds: BTreeMap<RegionId, Crowds> = BTreeMap::new();
    crowds.insert(
        home,
        vec![(ChunkPos::new(0, 0), 2), (ChunkPos::new(6, 0), 1)],
    );
    let mut alive = vec!["a", "b"];
    let (mut now, mut tick) = (start, 0);
    // One look, a quarter of a second after the last: every worker that is alive
    // vouches for its regions and says where their players are, the coordinator
    // ticks, and the list is read if it asks for it.
    let mut look = |coordinator: &mut Coordinator,
                    list: &RegionList,
                    crowds: &BTreeMap<RegionId, Crowds>,
                    alive: &[&str]| {
        now += Coordinator::LOOK;
        tick += 1;
        for name in alive {
            let held = coordinator.assignments(name);
            let vouched: Vec<(RegionId, Vouch)> = held
                .iter()
                .map(|held| (held.region, Vouch::Committed))
                .collect();
            coordinator.heartbeat(now, name, &vouched);
            let players: Vec<PlayersOf> = held
                .iter()
                .map(|held| PlayersOf {
                    region: held.region,
                    epoch: held.epoch,
                    tick,
                    crowds: crowds.get(&held.region).cloned().unwrap_or_default(),
                })
                .collect();
            coordinator.players(now, name, &players);
        }
        let mut changes = coordinator.tick(now);
        if changes.read {
            let more = coordinator.listed(now, list);
            changes.orders.extend(more.orders);
            changes.reshaped.extend(more.reshaped);
        }
        (now, changes)
    };

    // The region is given to `a` when the coordinator's grace is over, rests, and is
    // split when the group has stood.
    let mut ordered = None;
    for _ in 0..200 {
        let (_, changes) = look(&mut coordinator, &list, &crowds, &alive);
        ordered = changes
            .orders
            .into_iter()
            .find_map(|told| match told.order {
                Order::SplitOff { part, as_epoch, .. } => Some((told.worker, part, as_epoch)),
                _ => None,
            });
        if ordered.is_some() {
            break;
        }
    }
    let (worker, part, as_epoch) = ordered.expect("the coordinator splits the region by itself");
    assert_eq!(worker, "a");
    assert_eq!(coordinator.under_way(), [Asked::Split { region: home }]);

    // The store makes the split, and `a` dies before it says so.
    list.regions.push(RegionInfo {
        region: part,
        epoch: as_epoch,
        bounds: Some(ChunkBox {
            min: ChunkPos::new(4, -7),
            max: ChunkPos::new(8, 7),
        }),
        pinned: Vec::new(),
    });
    list.next = RegionId(part.0 + 1);
    crowds.insert(home, vec![(ChunkPos::new(0, 0), 2)]);
    crowds.insert(part, vec![(ChunkPos::new(6, 0), 1)]);
    let (died, _) = look(&mut coordinator, &list, &crowds, &alive);
    coordinator.disconnected(died, "a");
    alive.retain(|name| *name != "a");

    // A lease later both regions are `b`'s: the split's reservation has ended
    // without the worker's word, and the list has shown the part.
    let runs = |coordinator: &Coordinator, region: RegionId| {
        let held = coordinator.assignments("b");
        held.iter().any(|held| held.region == region)
    };
    let mut ended = Vec::new();
    let (mut lapsed, mut parted, mut given) = (None, None, None);
    for _ in 0..200 {
        let (at, changes) = look(&mut coordinator, &list, &crowds, &alive);
        if !changes.reshaped.is_empty() {
            lapsed.get_or_insert(at);
        }
        ended.extend(changes.reshaped);
        if runs(&coordinator, part) {
            parted.get_or_insert(at);
        }
        if runs(&coordinator, home) && runs(&coordinator, part) {
            given = Some(at);
            break;
        }
    }
    let given = given.expect("the other worker is given both regions");
    assert!(
        matches!(
            ended.as_slice(),
            [Reshaped {
                asker: None,
                outcome: Err(_),
                ..
            }]
        ),
        "{ended:?}"
    );

    // K7 and section 5.5: nothing but the worker's word says that the split was
    // made, so the region is left alone for `LONG`, three rests, from when the
    // reservation ended, and being given an owner after that shortens nothing.
    let lapsed = lapsed.expect("the reservation ended");
    assert!(lapsed <= given);
    assert_eq!(
        coordinator.alone_until(home),
        Some(lapsed + 3 * REST),
        "the reservation ended {:?} after the start and the region was given an owner {:?} \
         after it",
        lapsed - start,
        given - start
    );
    // The part is a region that was given an owner, and rests as one from then.
    let parted = parted.expect("the part was given an owner");
    assert_eq!(coordinator.alone_until(part), Some(parted + REST));
}

/// The times of two lines of a log are compared across midnight and across the end
/// of a month, and what a line gives as a field is read to the next space.
#[test]
fn the_lines_of_a_log_are_read_as_they_are_written() {
    let line = |stamp: &str| format!("{stamp}  INFO clustine_coordinator::state: a line");
    let at = |stamp: &str| time_of(&line(stamp)).unwrap();
    assert_eq!(at("1970-01-01T00:00:00.000000Z"), 0.0);
    assert_eq!(
        at("2026-10-09T00:00:01.500000Z") - at("2026-10-08T23:59:59.250000Z"),
        2.25
    );
    assert_eq!(
        at("2028-03-01T00:00:00.000000Z") - at("2028-02-28T00:00:00.000000Z"),
        172_800.0
    );
    assert_eq!(time_of("no line of a log"), None);

    let split = "2026-10-09T10:00:00.000000Z  INFO clustine_coordinator::state: a split is begun \
                 by itself region=0 part=2 groups=1 chunks=25";
    let begun = Begun::Split {
        region: 0,
        part: 2,
        groups: 1,
        chunks: 25,
    };
    assert_eq!(Begun::read(split), Some(begun.clone()));
    assert!(begun.reshapes() && begun.is_of(0) && !begun.is_of(2));
    let moved = "2026-10-09T10:00:00.000000Z  INFO clustine_coordinator::state: a region is \
                 moved to even regions out region=2 from=worker-0 to=worker-1";
    let begun = Begun::read(moved).unwrap();
    assert!(!begun.reshapes() && begun.is_of(2));
    let said = "2026-10-09T10:00:00.000000Z  INFO clustine_coordinator::state: a worker says \
                what came of a split worker=worker-0 region=0 as_epoch=7 outcome=Ok(RegionId(2))";
    let ended = Ended::read(said).unwrap();
    assert!(ended.well());
    assert_eq!(ended.part(), Some(2));
    let off = said.replace("Ok(RegionId(2))", "Err(Nobody)");
    let ended = Ended::read(&off).unwrap();
    assert!(!ended.well());
    assert_eq!(ended.part(), None);
}
