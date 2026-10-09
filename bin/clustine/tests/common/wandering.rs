//! A world without pinned regions under players, for the tests that run it whole:
//! `wanders.rs` and `crowds.rs`, the scenarios of
//! `docs/adr/0017-the-end-of-the-stripes.md`, sections 9.6 and 9.7.
//!
//! The world is served by a cluster of processes, by a `Server` in the test's own
//! process, or by one server process whose log the test reads. On it play **groups**,
//! each a ledger scenario of the bots that can be sent to walk elsewhere, and
//! **wanderers**, plain bots that are in no ledger and have no auditor.
//!
//! What the server did is read from outside, as `follows.rs` reads it on pinned
//! regions: the world store's list of regions, the routing table and the lines of the
//! coordinator's log for everything it begins by itself and for how a merge and a
//! split ended (ADR-0016, section 10), and the lines of the workers that say who was
//! handed over, what a split took and how long a region stood still. The list is also
//! read four times a second for as long as a test waits for anything, and every
//! reading is kept, for what the record says of every reading in some time.
//!
//! A test's seed is the clock's unless the variable its setup names says another; the
//! test prints it. The logs of the processes of a test that fails are kept, and the
//! failure says where.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clustine::{Config, Server};
use clustine_botswarm::{Bot, Ledger, LedgerReport, Progress, Random, audit_blocks, ledger};
use clustine_coordinator::Policy;
use clustine_rpc::{ChunkBox, RegionInfo, RegionList};
use tempfile::TempDir;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::processes::{Cluster, Table, Turn, turn, turn_alone, worker_name};
use super::{free_address, spawn_server_with_log};

/// How long anything may take that is merely waited for. It only ever runs out when
/// something hangs.
pub const PATIENCE: Duration = Duration::from_secs(60);

/// How often a state that is waited for is looked at.
pub const LOOK: Duration = Duration::from_millis(20);

/// How often the world store's list is read while a test waits: "four a second".
pub const READING: Duration = Duration::from_millis(250);

/// The moment beyond the leases that a cluster may take to be whole again after a
/// process was killed.
pub const MOMENT: Duration = Duration::from_secs(5);

/// The longest that a player may wait for an acknowledgement.
pub const LONGEST_PAUSE: Duration = Duration::from_secs(5);

/// Every this many client ticks each bot of a group sends a pulse.
pub const PULSE: u32 = 2;

/// What is allowed for when a line of a log is written, where the times of two lines,
/// or of a line and of the test's own clock, are compared.
pub const WRITING: f64 = 0.1;

/// A client tick, which is what a bot's steps are counted in.
pub const TICK: Duration = Duration::from_millis(50);

/// What a region is called on the command line and in the logs.
pub type Region = u32;

/// Whether this run of the tests is the one that repeats the end-to-end tests on a
/// world divided into regions, which `CLUSTINE_TEST_PINS` asks for. The tests of a
/// world without pins start their worlds themselves and take minutes, so they run
/// once, in the run without it.
pub fn a_repetition() -> bool {
    std::env::var_os("CLUSTINE_TEST_PINS").is_some()
}

/// The number the variable called `name` is set to, if it is set.
pub fn number_from(name: &str) -> Option<u64> {
    let set = std::env::var(name).ok()?;
    Some(set.parse().unwrap_or_else(|_| panic!("{name} is a number")))
}

/// How often the workers write a checkpoint in a run with this seed: every second or
/// two, so that merges and splits fall into checkpoints, or practically never, so that
/// the checkpoint a merge or a split begins with has everything to save that was built
/// so far.
pub fn checkpoint_seconds(seed: u64) -> u64 {
    [1, 2, 300][(seed % 3) as usize]
}

/// Seconds with three decimals, for a message.
pub fn seconds(duration: Duration) -> String {
    format!("{:.3} s", duration.as_secs_f64())
}

/// A time that is known only if something happened, for a message.
pub fn perhaps(time: Option<Duration>) -> String {
    time.map_or("-".to_owned(), seconds)
}

/// The least, the middle and the worst of `times`, for a line of a table. Of none,
/// nothing.
pub fn spread(times: &[Duration]) -> String {
    let mut sorted = times.to_vec();
    sorted.sort_unstable();
    match (sorted.first(), sorted.get(sorted.len() / 2), sorted.last()) {
        (Some(least), Some(middle), Some(worst)) => format!(
            "{:.3} / {:.3} / {:.3}",
            least.as_secs_f64(),
            middle.as_secs_f64(),
            worst.as_secs_f64()
        ),
        _ => "-".to_owned(),
    }
}

/// The x coordinates a group walks up and down between when it walks between the
/// chunks `west` and `east`: far enough from the borders of the two for everything it
/// builds to be in those chunks.
pub fn between(west: i32, east: i32) -> (f64, f64) {
    (f64::from(16 * west) + 2.5, f64::from(16 * east) + 13.5)
}

/// The x coordinates a group walks up and down between when it stands in `chunk`.
pub fn within(chunk: i32) -> (f64, f64) {
    between(chunk, chunk)
}

/// The chunk coordinate of the block coordinate `block`.
pub fn chunk_of(block: f64) -> i32 {
    (block.floor() as i32).div_euclid(16)
}

/// When a line of a log was written, in seconds since 1970, which is what a line can
/// be compared by with another and with the test's own clock.
pub fn time_of(line: &str) -> Option<f64> {
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
pub fn field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=");
    let rest = &line[line.find(&key)? + key.len()..];
    rest.split_whitespace().next()
}

/// The number a line of a log gives as `name=`.
pub fn number<T: std::str::FromStr>(line: &str, name: &str) -> Option<T> {
    field(line, name)?.parse().ok()
}

/// Something the coordinator began by itself, as the line of its log says that the
/// record has for it (ADR-0016, section 10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Begun {
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
    pub fn read(line: &str) -> Option<Self> {
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

    /// Whether this is a merge, an absorption or a split: not a move.
    pub fn reshapes(&self) -> bool {
        !matches!(self, Self::Move { .. })
    }

    /// Whether the players of `region` are stood still by this: it is one of the two
    /// regions of the merge, the region that is split, or the region that is moved.
    pub fn is_of(&self, region: Region) -> bool {
        match self {
            Self::Merge {
                survivor, absorbed, ..
            }
            | Self::Absorption { survivor, absorbed } => [*survivor, *absorbed].contains(&region),
            Self::Split { region: split, .. } => *split == region,
            Self::Move { region: moved, .. } => *moved == region,
        }
    }

    /// Whether `ended` is how a thing like this one ends: a merge of the same two
    /// regions, a split of the same region, or, for a move, the region being
    /// assigned.
    pub fn is_ended_by(&self, ended: &Ended) -> bool {
        match (self, ended) {
            (
                Self::Merge {
                    survivor, absorbed, ..
                }
                | Self::Absorption { survivor, absorbed },
                Ended::Merge {
                    survivor: into,
                    absorbed: gone,
                    ..
                },
            ) => (survivor, absorbed) == (into, gone),
            (Self::Split { region, .. }, Ended::Split { region: split, .. }) => region == split,
            (Self::Move { region, .. }, Ended::Assigned { region: given, .. }) => region == given,
            _ => false,
        }
    }

    /// In a few words, for a message.
    pub fn told(&self) -> String {
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
pub enum Ended {
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
    pub fn read(line: &str) -> Option<Self> {
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
    pub fn well(&self) -> bool {
        match self {
            Self::Merge { outcome, .. } | Self::Split { outcome, .. } => outcome.starts_with("Ok"),
            Self::Assigned { .. } => false,
        }
    }

    /// The region a split that ended well made: the number in its outcome.
    pub fn part(&self) -> Option<Region> {
        let Self::Split { outcome, .. } = self else {
            return None;
        };
        let digits: String = outcome.chars().filter(char::is_ascii_digit).collect();
        digits.parse().ok().filter(|_| self.well())
    }

    /// In a few words, for a message; nothing of a region that was assigned.
    pub fn told(&self) -> Option<String> {
        match self {
            Self::Merge {
                survivor,
                absorbed,
                outcome,
            } => Some(format!(
                "the merge of region {absorbed} into region {survivor}: {outcome}"
            )),
            Self::Split { region, outcome } => {
                Some(format!("the split of region {region}: {outcome}"))
            }
            Self::Assigned { .. } => None,
        }
    }
}

/// A worker's line `a region stood still for a merge or a split`: how long a region
/// did not tick, with how many players and how many chunks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stood {
    pub at: f64,
    pub region: Region,
    pub players: u32,
    pub held: u32,
    pub milliseconds: u64,
}

/// A worker's line `a part of the region has been split off`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SplitOff {
    pub at: f64,
    pub part: Region,
    /// How many players went with the part.
    pub players: u32,
    /// How many chunks the part holds, and how many of them were grants that no tick
    /// of the region had been told of.
    pub chunks: u32,
    pub waited: u32,
}

/// A worker's line `chunks asked for players who went are taken for the part's`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Taken {
    pub at: f64,
    pub part: Region,
    pub chunks: u32,
    /// How many of them did not go with the split: nobody's, and the part's by the
    /// first claim.
    pub free: u32,
}

/// The chunks of `region` by `list`: the smallest box that has every chunk it was
/// granted, if it lives and was granted any.
pub fn bounds(list: &RegionList, region: Region) -> Option<ChunkBox> {
    info(list, region).and_then(|info| info.bounds)
}

/// What `list` has of `region`, if it lives.
pub fn info(list: &RegionList, region: Region) -> Option<&RegionInfo> {
    list.regions.iter().find(|info| info.region.0 == region)
}

/// The living regions of `list`, in ascending order.
pub fn living(list: &RegionList) -> Vec<Region> {
    list.regions.iter().map(|info| info.region.0).collect()
}

/// The region that `list` has `region` absorbed by, if it has it absorbed.
pub fn absorbed_by(list: &RegionList, region: Region) -> Option<Region> {
    let pair = list.absorbed.iter().find(|(gone, _)| gone.0 == region);
    pair.map(|(_, into)| into.0)
}

/// Whether `bounds` are exactly the chunks from `x.0` to `x.1` and from `z.0` to
/// `z.1`.
pub fn is_box(bounds: Option<ChunkBox>, x: (i32, i32), z: (i32, i32)) -> bool {
    bounds.is_some_and(|bounds| {
        (bounds.min.x, bounds.max.x) == x && (bounds.min.z, bounds.max.z) == z
    })
}

/// Whether the chunk at `x` and `z` is within `bounds`.
pub fn has_chunk(bounds: Option<ChunkBox>, x: i32, z: i32) -> bool {
    bounds.is_some_and(|bounds| {
        bounds.min.x <= x && x <= bounds.max.x && bounds.min.z <= z && z <= bounds.max.z
    })
}

/// A group of players: one ledger scenario, which can be sent to a chunk.
pub struct Group {
    pub name: &'static str,
    pub ledger: Ledger,
    pub progress: Arc<Progress>,
    /// The scenario, until the group has left.
    pub playing: Option<JoinHandle<anyhow::Result<LedgerReport>>>,
    /// What the bots and their auditor found, once the group has left.
    pub report: Option<LedgerReport>,
}

/// What a wanderer is told to do next.
enum Errand {
    /// Walk in a straight line to `x` and `z`, at `speed` blocks a tick.
    Walk { x: f64, z: f64, speed: f64 },
    /// Leave the game.
    Leave,
}

/// A wanderer as its own task last noted it.
#[derive(Debug, Clone, Default)]
pub struct Wandering {
    /// Whether it has been placed in the world, and where.
    pub joined: bool,
    pub x: f64,
    pub z: f64,
    /// How many walks it has been sent on and how many of them it has ended.
    pub sent: u32,
    pub arrived: u32,
    /// The furthest east and west it has been, as chunk x coordinates.
    pub east: i32,
    pub west: i32,
    /// Whether it has left the game because it was told to.
    pub left: bool,
    /// Why it is gone, if it is gone for any other reason: it was disconnected.
    pub lost: Option<String>,
}

/// A plain bot that is in no ledger and has no auditor: it joins, walks where it is
/// sent and stands there, and leaves when it is told to.
pub struct Wanderer {
    pub name: String,
    errands: mpsc::UnboundedSender<Errand>,
    noted: Arc<Mutex<Wandering>>,
}

impl Wanderer {
    /// Lets a bot called `name` join the server at `address`.
    fn join(address: &str, name: &str) -> Self {
        let (errands, mut told) = mpsc::unbounded_channel();
        let noted = Arc::new(Mutex::new(Wandering::default()));
        let (address, name) = (address.to_owned(), name.to_owned());
        let wanderer = Self {
            name: name.clone(),
            errands,
            noted: noted.clone(),
        };
        tokio::spawn(async move {
            let note = |change: &dyn Fn(&mut Wandering)| {
                change(&mut noted.lock().expect("nobody panics with it"));
            };
            let wandering = async {
                let mut bot = Bot::join(&address, &name).await?;
                bot.wait_for_chunks(1, PATIENCE).await?;
                let (x, _, z) = bot.location;
                note(&|noted| {
                    (noted.joined, noted.x, noted.z) = (true, x, z);
                    (noted.east, noted.west) = (chunk_of(x), chunk_of(x));
                });
                loop {
                    match told.try_recv() {
                        Ok(Errand::Walk { x, z, speed }) => {
                            // Step by step, each the step `Bot::walk_to` takes, so
                            // that where the wanderer is can be noted on the way: the
                            // packets are those of one walk.
                            loop {
                                let (dx, dz) = (x - bot.location.0, z - bot.location.2);
                                let distance = dx.hypot(dz);
                                if distance < 1e-9 {
                                    break;
                                }
                                let step = (speed / distance).min(1.0);
                                let next = if step >= 1.0 {
                                    (x, z)
                                } else {
                                    (bot.location.0 + dx * step, bot.location.2 + dz * step)
                                };
                                // A little more than the step is long, so that
                                // rounding never makes two steps of it.
                                bot.walk_to(next.0, next.1, speed * 1.001).await?;
                                let (here, _, there) = bot.location;
                                note(&|noted| {
                                    (noted.x, noted.z) = (here, there);
                                    noted.east = noted.east.max(chunk_of(here));
                                    noted.west = noted.west.min(chunk_of(here));
                                });
                            }
                            note(&|noted| noted.arrived += 1);
                        }
                        Ok(Errand::Leave) | Err(mpsc::error::TryRecvError::Disconnected) => {
                            return anyhow::Ok(());
                        }
                        // Shorter than a tick, so that wanderers who are sent in the
                        // same turn of a test set out within a few milliseconds of
                        // each other.
                        Err(mpsc::error::TryRecvError::Empty) => {
                            bot.idle(Duration::from_millis(5)).await?;
                        }
                    }
                }
            };
            match wandering.await {
                Ok(()) => note(&|noted| noted.left = true),
                Err(error) => note(&|noted| noted.lost = Some(format!("{error:#}"))),
            }
        });
        wanderer
    }

    /// The wanderer as last noted.
    pub fn noted(&self) -> Wandering {
        self.noted.lock().expect("nobody panics with it").clone()
    }

    /// Sends the wanderer to `x` and `z`, at `speed` blocks a tick.
    pub fn walks_to(&self, x: f64, z: f64, speed: f64) {
        self.noted.lock().expect("nobody panics with it").sent += 1;
        // A wanderer that is gone is found by whoever tends the test.
        let _ = self.errands.send(Errand::Walk { x, z, speed });
    }

    /// Whether the wanderer has ended every walk it was sent on.
    pub fn has_arrived(&self) -> bool {
        let noted = self.noted();
        noted.joined && noted.arrived == noted.sent
    }

    /// Tells the wanderer to leave the game.
    pub fn leaves(&self) {
        let _ = self.errands.send(Errand::Leave);
    }
}

/// Which region the bots of a group are in, for the tests that have to know: the
/// region they are in when the test begins to count, and whether they are the ones who
/// go when that region is split. The test sees to it that this is so, by where it
/// sends its groups and by what the list says of each part.
#[derive(Debug, Clone, Copy)]
pub struct Stands {
    pub group: &'static str,
    pub first: Region,
    pub goes: bool,
}

/// What the bots noticed of one thing the coordinator began by itself, and how long
/// after the walk that caused it the thing was begun and was done.
pub struct Measured {
    pub begun: Begun,
    /// What led to it: the walk that was under way, or last done, when it was begun;
    /// of a move, the end of the split that left a worker with a region more.
    pub cause: Option<String>,
    /// How long after that it was begun, and had ended.
    pub begun_after: Option<Duration>,
    pub done_after: Option<Duration>,
    /// The longest wait for an acknowledgement among the bots whose region went on
    /// (the survivor's, those who stayed at a split, at a move everybody else), and
    /// among those who came to another region or another worker.
    pub stayed: Option<Duration>,
    pub went: Option<Duration>,
    /// What the worker's line says of how long the region that went on stood still
    /// for it, if there is such a line: milliseconds, players and chunks held.
    pub stood: Option<Stood>,
}

/// What a test's world is like.
#[derive(Debug, Clone)]
pub struct Setup {
    /// What the test's files and variables are named by: `wanders` gives the
    /// directory `clustine-wanders-…` and the variables `CLUSTINE_WANDERS_SEED` and
    /// `CLUSTINE_WANDERS_KEEP`.
    pub family: &'static str,
    /// How many workers a cluster has.
    pub workers: usize,
    /// The view distance of the edge and what the coordinator is told it is.
    pub view: i32,
    /// The coordinator's lease in seconds, or `None` for the one it has when it is
    /// not told any, which is 5.
    pub lease: Option<u64>,
    /// How long the coordinator leaves a region alone after anything it did to it.
    pub rest: Duration,
    /// Whether the cluster runs with no other beside it, for a test that measures.
    pub alone: bool,
}

/// What serves the world.
enum Under {
    /// A cluster of processes.
    Cluster(Box<Cluster>),
    /// A `Server` in the test's own process, of which only the list and the bots can
    /// be looked at. Taken out while it is stopped.
    Server(Option<Server>, Box<Config>),
    /// One server process, whose log is read and whose list cannot be.
    Process {
        server: Option<Child>,
        world: PathBuf,
        log: PathBuf,
        arguments: Vec<String>,
    },
}

/// A world whose regions follow their players, with groups of bots and wanderers on
/// it.
pub struct Wanders {
    /// Its leave to run beside the other tests' clusters.
    _turn: Turn,
    /// Where the world and the logs are; taken out when they are to be kept.
    directory: Option<TempDir>,
    under: Under,
    /// Where players connect.
    pub address: String,
    pub test: String,
    pub setup: Setup,
    pub seed: u64,
    pub random: Random,
    /// The coordinator's lease.
    pub lease: Duration,
    pub groups: Vec<Group>,
    pub wanderers: Vec<Wanderer>,
    pub started: Instant,
    /// The time of the logs at `started`, in seconds since 1970.
    pub started_at: f64,
    /// What was done to the world and what it did about it, in order.
    deeds: Vec<String>,
    /// When each group was sent somewhere, and where.
    walks: Vec<(Instant, String)>,
    /// Every reading of the world store's list that was made while the test waited.
    pub readings: Vec<(Instant, RegionList)>,
    read: Instant,
}

impl Wanders {
    /// The seed of a run of the tests of `family`: the variable if set, else the
    /// clock.
    fn seed(family: &str) -> u64 {
        let variable = format!("CLUSTINE_{}_SEED", family.to_uppercase());
        number_from(&variable).unwrap_or_else(|| {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
            now.as_nanos() as u64 % 1_000_000
        })
    }

    /// What every kind of world begins with: its turn, its seed and its directory.
    async fn begin(test: &str, setup: &Setup) -> (Turn, u64, TempDir) {
        let turn = if setup.alone {
            turn_alone().await
        } else {
            turn().await
        };
        let seed = Self::seed(setup.family);
        println!(
            "{test}: seed {seed} (set CLUSTINE_{}_SEED={seed} to run it again)",
            setup.family.to_uppercase()
        );
        let directory = tempfile::Builder::new()
            .prefix(&format!("clustine-{}-", setup.family))
            .tempdir()
            .unwrap();
        (turn, seed, directory)
    }

    fn new(
        test: &str,
        setup: Setup,
        (turn, seed, directory): (Turn, u64, TempDir),
        under: Under,
        address: String,
    ) -> Self {
        let started_at = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        Self {
            _turn: turn,
            directory: Some(directory),
            under,
            address,
            test: test.to_owned(),
            lease: Duration::from_secs(setup.lease.unwrap_or(5)),
            setup,
            seed,
            random: Random::new(seed),
            groups: Vec::new(),
            wanderers: Vec::new(),
            started: Instant::now(),
            started_at: started_at.as_secs_f64(),
            deeds: Vec::new(),
            walks: Vec::new(),
            readings: Vec::new(),
            read: Instant::now(),
        }
    }

    /// Starts a cluster without pins: the store is told nothing of how the world is
    /// divided, and the coordinator nothing of how it reshapes, so that it does what a
    /// coordinator does when told nothing, with the distances that follow from the
    /// view distance and the rest of `setup`. Returns once players can join.
    pub async fn cluster(test: &str, setup: Setup) -> Self {
        let begun = Self::begin(test, &setup).await;
        let checkpoints = checkpoint_seconds(begun.1);
        println!("{test}: the workers checkpoint every {checkpoints} s");
        let mut attempt = 0;
        let cluster = loop {
            let mut cluster = Cluster::new(begun.2.path(), setup.workers, "").await;
            cluster.worker_arguments =
                vec!["--checkpoint-interval".to_owned(), checkpoints.to_string()];
            // Nothing of `--reshape` and no distance: the default is what is tested.
            cluster.coordinator_arguments = [
                "--view-distance",
                &setup.view.to_string(),
                "--rest-seconds",
                &setup.rest.as_secs().to_string(),
            ]
            .map(str::to_owned)
            .to_vec();
            cluster.lease_seconds = setup.lease;
            cluster.view_distance = setup.view;
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
        let address = cluster.edge.0.clone();
        Self::new(
            test,
            setup,
            begun,
            Under::Cluster(Box::new(cluster)),
            address,
        )
    }

    /// The policy a coordinator that is told the view distance and the rest of `setup`
    /// and nothing else goes by.
    fn policy(setup: &Setup) -> Policy {
        Policy {
            rest: setup.rest,
            ..Policy::for_view_distance(setup.view as u32)
        }
    }

    /// Starts a `Server` in the test's own process, with its world on disk, whose
    /// regions follow their players by the distances of the view distance of `setup`.
    pub async fn server(test: &str, setup: Setup) -> Self {
        let begun = Self::begin(test, &setup).await;
        let config = Config {
            view_distance: setup.view,
            world: Some(begun.2.path().join("world")),
            checkpoint_interval: Duration::from_secs(checkpoint_seconds(begun.1)),
            pins: Vec::new(),
            follow: Some(Self::policy(&setup)),
            ..super::config()
        };
        let server = match Server::start(config.clone()).await {
            Ok(server) => server,
            Err(error) => panic!("{test}: the server did not start: {error:#}"),
        };
        let address = server.address().to_string();
        let under = Under::Server(Some(server), Box::new(config));
        Self::new(test, setup, begun, under, address)
    }

    /// Starts one server process whose log is kept, and which is told its view
    /// distance and its rest and nothing else of how it reshapes.
    pub async fn process(test: &str, setup: Setup) -> Self {
        let begun = Self::begin(test, &setup).await;
        let (world, log) = (begun.2.path().join("world"), begun.2.path().join("server"));
        let arguments: Vec<String> = [
            "--view-distance",
            &setup.view.to_string(),
            "--rest-seconds",
            &setup.rest.as_secs().to_string(),
            "--checkpoint-interval",
            &checkpoint_seconds(begun.1).to_string(),
        ]
        .map(str::to_owned)
        .to_vec();
        let address = free_address().await;
        let more: Vec<&str> = arguments.iter().map(String::as_str).collect();
        let server = spawn_server_with_log(&address, &world, &more, &log).await;
        let under = Under::Process {
            server: Some(server),
            world,
            log,
            arguments,
        };
        Self::new(test, setup, begun, under, address)
    }

    /// The cluster, for a test that kills its processes. Fails where the world is not
    /// served by one.
    pub fn processes(&mut self) -> &mut Cluster {
        match &mut self.under {
            Under::Cluster(cluster) => cluster,
            _ => panic!("{}: the world is not served by a cluster", self.test),
        }
    }

    /// The cluster, if the world is served by one.
    pub fn a_cluster(&self) -> Option<&Cluster> {
        match &self.under {
            Under::Cluster(cluster) => Some(cluster),
            _ => None,
        }
    }

    /// Whether the lines of the coordinator and of the workers can be read: not of a
    /// `Server` in the test's own process, whose log is the test's own output.
    pub fn has_logs(&self) -> bool {
        !matches!(self.under, Under::Server(..))
    }

    /// Whether the world store's list can be read: not of a server process, which
    /// prints it nowhere.
    pub fn has_a_list(&self) -> bool {
        !matches!(self.under, Under::Process { .. })
    }

    /// Notes something that was done to the world, or that it did.
    pub fn note(&mut self, deed: String) {
        let deed = format!("{:7.3} s: {deed}", self.started.elapsed().as_secs_f64());
        println!("{}: {deed}", self.test);
        self.deeds.push(deed);
    }

    /// Fails the test, saying what led up to it and where the logs are, which are kept.
    pub fn fail(&mut self, message: &str) -> ! {
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
        for wanderer in &self.wanderers {
            report.push_str(&format!("  {}: {:?}\n", wanderer.name, wanderer.noted()));
        }
        if let Some((_, list)) = self.readings.last() {
            report.push_str(&format!("the list as last read: {list:?}\n"));
        }
        report.push_str(&format!(
            "the routing table last logged: {:?}\n",
            self.table()
        ));
        report.push_str(&format!(
            "the world and the logs of the processes: {}\n",
            kept.display()
        ));
        panic!("{report}");
    }

    /// Keeps the directory with the world and the logs beyond the test.
    pub fn keep(&mut self) -> PathBuf {
        match self.directory.take() {
            Some(directory) => directory.keep(),
            None => match &self.under {
                Under::Cluster(cluster) => cluster.logs.parent().unwrap().to_owned(),
                Under::Server(_, config) => config.world.clone().unwrap_or_default(),
                Under::Process { world, .. } => world.parent().unwrap().to_owned(),
            },
        }
    }

    /// Fails if a process has ended that nobody killed, a group has, or a wanderer was
    /// disconnected: the bots of a group end by themselves only when one of them was
    /// disconnected, saw a player twice or saw one vanish, or the ledgers and the
    /// world differ. And reads the list, if it is time.
    pub async fn tend(&mut self) {
        let mut ended = None;
        match &mut self.under {
            Under::Cluster(cluster) => {
                for (name, process) in cluster.processes() {
                    let gone = process.as_mut().and_then(|child| child.try_wait().unwrap());
                    if let Some(status) = gone {
                        ended = Some(format!("{name} ended by itself ({status})"));
                    }
                }
            }
            Under::Process { server, .. } => {
                let gone = server.as_mut().and_then(|child| child.try_wait().unwrap());
                if let Some(status) = gone {
                    ended = Some(format!("the server ended by itself ({status})"));
                }
            }
            Under::Server(..) => {}
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
        let lost = self.wanderers.iter().find_map(|wanderer| {
            let lost = wanderer.noted().lost?;
            Some(format!(
                "the wanderer {} was disconnected: {lost}",
                wanderer.name
            ))
        });
        if let Some(lost) = lost {
            self.fail(&lost);
        }
        if self.read.elapsed() >= READING && self.has_a_list() {
            self.read = Instant::now();
            if let Ok(list) = self.reads().await {
                self.readings.push((Instant::now(), list));
            }
        }
    }

    /// Waits until `state` holds, looking after the processes and the players
    /// meanwhile, and returns how long that took. Fails, naming `what` was waited
    /// for, if it takes longer than anything should.
    pub async fn until(&mut self, what: &str, state: impl FnMut(&Self) -> bool) -> Duration {
        self.until_within(PATIENCE, what, state).await
    }

    /// Waits until `state` holds as [`Wanders::until`] does, for `limit` at most.
    pub async fn until_within(
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

    /// The last routing table the coordinator logged in its present life, in a
    /// cluster.
    pub fn table(&self) -> Option<Table> {
        self.a_cluster()?.present_table()
    }

    /// The world store's list, or why it cannot be read.
    async fn reads(&self) -> Result<RegionList, String> {
        match &self.under {
            Under::Cluster(cluster) => cluster.regions().await,
            Under::Server(server, _) => {
                let server = server.as_ref().ok_or("the server is stopped")?;
                server.regions().map_err(|error| format!("{error:#}"))
            }
            Under::Process { .. } => Err("a server process does not show its list".to_owned()),
        }
    }

    /// The regions of the world as the world store has them, which is what decides.
    pub async fn list(&mut self) -> RegionList {
        match self.reads().await {
            Ok(list) => list,
            Err(error) => self.fail(&format!("the world store's list cannot be read: {error}")),
        }
    }

    /// Waits until the world store's list is as `shows` wants it, and returns it.
    pub async fn until_the_list(
        &mut self,
        what: &str,
        shows: impl Fn(&RegionList) -> bool,
    ) -> RegionList {
        self.until_the_list_within(PATIENCE, what, shows).await
    }

    /// Waits until the world store's list is as `shows` wants it, for `limit` at
    /// most, and returns it.
    pub async fn until_the_list_within(
        &mut self,
        limit: Duration,
        what: &str,
        shows: impl Fn(&RegionList) -> bool,
    ) -> RegionList {
        let waiting = Instant::now();
        loop {
            self.tend().await;
            let list = self.reads().await;
            if let Ok(list) = &list
                && shows(list)
            {
                return list.clone();
            }
            if waiting.elapsed() > limit {
                self.fail(&format!(
                    "waited {limit:?} in vain until the list shows that {what}; it is {list:?}"
                ));
            }
            tokio::time::sleep(LOOK).await;
        }
    }

    /// Waits until every region the world store's list has is run by a worker the edge
    /// is linked to, the coordinator knows of no other region, and `more` holds as
    /// well. Returns the list. Of a single process, in which every region the list
    /// has runs or is about to, it waits for `more` alone.
    ///
    /// The list is read anew at every look: what a worker asked of the store before it
    /// was killed, the store may do a moment after the test first looked.
    pub async fn runs_and(&mut self, what: &str, more: impl Fn(&Self) -> bool) -> RegionList {
        let waiting = Instant::now();
        loop {
            self.tend().await;
            let list = self.reads().await;
            if let Ok(list) = &list {
                let runs = match self.a_cluster() {
                    Some(cluster) => {
                        let living = living(list);
                        let known = self.table().map(|table| table.known());
                        known.as_ref() == Some(&living)
                            && living.iter().all(|region| cluster.runs(*region))
                    }
                    None => true,
                };
                if runs && more(self) {
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
    pub async fn everything_runs(&mut self) -> RegionList {
        let what = "every region of the store's list runs on a worker the edge is linked to";
        self.runs_and(what, |_| true).await
    }

    /// Waits until the world is whole: every region runs, on a worker the edge is
    /// linked to, and every bot of a group has had something acknowledged that it
    /// sent after that was so. Returns the list of regions.
    pub async fn whole(&mut self) -> RegionList {
        let list = self.everything_runs().await;
        self.served().await;
        let loads = self.a_cluster().map(Cluster::loads);
        self.note(format!(
            "the world is whole, with the regions {:?} shared as {loads:?}",
            living(&list),
        ));
        list
    }

    /// Waits until every bot that plays has had something acknowledged that it sends
    /// from now on.
    pub async fn served(&mut self) {
        let sent = |wanders: &Self| -> Vec<Vec<i32>> {
            let playing = wanders
                .groups
                .iter()
                .filter(|group| group.playing.is_some());
            playing
                .map(|group| group.progress.bots().iter().map(|bot| bot.sent).collect())
                .collect()
        };
        let before = sent(self);
        self.until("every bot has something new acknowledged", |wanders| {
            let playing = wanders
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
    pub fn group(&self, name: &str) -> &Group {
        let found = self.groups.iter().find(|group| group.name == name);
        found.unwrap_or_else(|| panic!("there is no group {name}"))
    }

    /// Where the bots are, for a message.
    pub fn whereabouts(&self) -> String {
        let groups = self.groups.iter().filter(|group| group.playing.is_some());
        let mut places: Vec<String> = groups
            .map(|group| {
                let bots = group.progress.bots();
                // Of a few, where each is; of a crowd, where its first bot is.
                let few = if bots.len() > 3 { 1 } else { 3 };
                let xs = bots.iter().take(few).map(|bot| format!("{:.1}", bot.x));
                let xs: Vec<String> = xs.collect();
                format!("{} at x = {}", group.name, xs.join(" and "))
            })
            .collect();
        let here = |wanderer: &&Wanderer| {
            let noted = wanderer.noted();
            noted.joined && !noted.left
        };
        let wanderers: Vec<&Wanderer> = self.wanderers.iter().filter(here).collect();
        // Of a few, where each is; of many, where the first is and how many they are.
        for wanderer in wanderers
            .iter()
            .take(if wanderers.len() > 3 { 1 } else { 3 })
        {
            let noted = wanderer.noted();
            places.push(format!(
                "{} at x = {:.1}, z = {:.1}",
                wanderer.name, noted.x, noted.z
            ));
        }
        if wanderers.len() > 3 {
            places.push(format!("{} wanderers in all", wanderers.len()));
        }
        places.join(", ")
    }

    /// What a group called `name` of `bots` bots plays, with its lanes from the block
    /// row `first_lane` on, walking up and down between the x coordinates `between`:
    /// for a test to change before it lets the group join.
    pub fn scenario(
        &self,
        name: &'static str,
        bots: usize,
        first_lane: i32,
        between: (f64, f64),
    ) -> Ledger {
        Ledger {
            bots,
            rounds: None,
            duration: None,
            west: between.0,
            east: between.1,
            // A world without pins has no line anybody knows of beforehand.
            lines: Vec::new(),
            // Every group chooses differently, and all of them by the seed.
            seed: self.seed + self.groups.len() as u64,
            name_prefix: name.to_owned(),
            first_lane,
            pulse: Some(PULSE),
            ..Ledger::default()
        }
    }

    /// Lets the bots of `scenario` join as the group `name`: they walk to where the
    /// scenario begins from where players enter the world without building.
    pub fn joins_with(&mut self, name: &'static str, scenario: Ledger) {
        let progress = Progress::new(scenario.bots);
        let playing = {
            let address = self.address.clone();
            let (scenario, progress) = (scenario.clone(), progress.clone());
            tokio::spawn(async move { ledger(&address, &scenario, &progress).await })
        };
        self.note(format!(
            "group {name} joins, {} bots, to walk between x = {} and x = {}",
            scenario.bots, scenario.west, scenario.east
        ));
        self.groups.push(Group {
            name,
            ledger: scenario,
            progress,
            playing: Some(playing),
            report: None,
        });
    }

    /// Lets `bots` bots join as the group `name`, with their lanes from the block row
    /// `first_lane` on, to walk up and down between the x coordinates `between`.
    pub fn joins(&mut self, name: &'static str, bots: usize, first_lane: i32, between: (f64, f64)) {
        let scenario = self.scenario(name, bots, first_lane, between);
        self.joins_with(name, scenario);
    }

    /// Sends the group `name` to walk up and down between the x coordinates `between`.
    pub fn walks(&mut self, name: &str, between: (f64, f64), what: &str) -> Instant {
        self.group(name)
            .progress
            .walk_between(between.0, between.1)
            .unwrap();
        let sent = Instant::now();
        self.walks.push((sent, format!("{name} was sent {what}")));
        self.note(format!("{name} is sent {what}; {}", self.whereabouts()));
        sent
    }

    /// Sends the group `name` to stand in `chunk`.
    pub fn walks_to(&mut self, name: &str, chunk: i32) -> Instant {
        self.walks(name, within(chunk), &format!("to chunk {chunk}"))
    }

    /// Fails unless every bot of the group `name` is within the coordinates it was
    /// last sent to, which are `between`.
    pub fn is_between(&mut self, name: &str, between: (f64, f64)) {
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
    pub fn is_in(&mut self, name: &str, chunk: i32) {
        self.is_between(name, within(chunk));
    }

    /// How long the group `name` may take to walk to where it was last sent from
    /// where it is: three times its slowest bot's way at its own pace, and the
    /// patience on top. It only ever runs out when the bots stand.
    fn way(&self, name: &str) -> Duration {
        let group = self.group(name);
        let Some((west, east)) = group.progress.between() else {
            return PATIENCE;
        };
        let bots = group.progress.bots();
        let furthest = bots
            .iter()
            .map(|bot| (bot.x - west).abs().max((bot.x - east).abs()))
            .fold(0.0, f64::max);
        PATIENCE + 3 * TICK.mul_f64(furthest / group.ledger.speed)
    }

    /// Waits until every bot of the group `name` is within the coordinates the group
    /// was last sent to.
    pub async fn arrives(&mut self, name: &str) {
        let what = format!("every bot of group {name} is where the group was sent");
        let progress = self.group(name).progress.clone();
        let limit = self.way(name);
        self.until_within(limit, &what, |_| {
            progress.arrived() && progress.bots().iter().all(|bot| bot.playing)
        })
        .await;
        let between = progress.between().expect("it was sent somewhere");
        self.is_between(name, between);
        self.note(format!("{name} has arrived; {}", self.whereabouts()));
    }

    /// Waits until every bot of the group `name` has been in the chunk row `chunk` or
    /// beyond it, on the way east if `eastwards` and west otherwise, and returns when
    /// that was seen.
    pub async fn passes(&mut self, name: &str, chunk: i32, eastwards: bool) -> Instant {
        let what = format!("every bot of group {name} has been in chunk {chunk}");
        let progress = self.group(name).progress.clone();
        let limit = self.way(name);
        self.until_within(limit, &what, |_| {
            progress.bots().iter().all(|bot| {
                let at = chunk_of(bot.x);
                bot.playing && if eastwards { at >= chunk } else { at <= chunk }
            })
        })
        .await;
        self.note(format!(
            "{name} has been in chunk {chunk}; {}",
            self.whereabouts()
        ));
        Instant::now()
    }

    /// Waits until every bot of the group `name` has had something acknowledged that
    /// it sent from now on, which is where it is now if it has arrived.
    pub async fn plays(&mut self, name: &str) {
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
    pub async fn walks_rounds(&mut self, name: &str, rounds: u32) {
        let group = self.group(name);
        let progress = group.progress.clone();
        let (west, east) = progress.between().expect("the group walks somewhere");
        // The rounds are what is waited for, and they take their time: this only
        // ever runs out when the bots stand.
        let ticks = 2.0 * (east - west) / group.ledger.speed * f64::from(rounds);
        let limit = PATIENCE + 3 * TICK.mul_f64(ticks);
        // An end counts as reached within a block of it, where a bot is for some
        // ticks: a look that comes late misses a round and costs time, no more.
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

    /// Lets the group `name` walk up and down where it walks for `time`, by the steps
    /// of its slowest bot: as many rounds as are that long, and no fewer than one.
    pub async fn walks_for(&mut self, name: &str, time: Duration) {
        let group = self.group(name);
        let (west, east) = group.progress.between().expect("the group walks somewhere");
        let ticks_of_a_round = 2.0 * (east - west) / group.ledger.speed;
        let ticks = time.as_secs_f64() / TICK.as_secs_f64();
        self.walks_rounds(name, ((ticks / ticks_of_a_round).ceil() as u32).max(1))
            .await;
    }

    /// Tells the bots of the group `name` to stop and waits until they, and the
    /// auditor who joins then, have left. Fails unless they found everything as the
    /// ledgers say.
    pub async fn leaves(&mut self, name: &str) {
        let index = self.groups.iter().position(|group| group.name == name);
        let index = index.unwrap_or_else(|| panic!("there is no group {name}"));
        self.groups[index].progress.finish();
        let mut playing = self.groups[index].playing.take().expect("it still plays");
        self.note(format!("{name} is told to stop; {}", self.whereabouts()));
        // The auditor walks wherever the group built, at the pace the group went to
        // its lanes at.
        let ledger = &self.groups[index].ledger;
        let far = self.walks.len() as f64 * 2000.0 + f64::from(ledger.lane_spacing) * 64.0;
        let limit = 3 * PATIENCE + 3 * TICK.mul_f64(far / ledger.to_the_lane);
        let waiting = Instant::now();
        while !playing.is_finished() {
            self.tend().await;
            if waiting.elapsed() > limit {
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

    /// Lets a wanderer called `name` join, and waits until it is placed in the world.
    /// Returns which wanderer it is.
    pub async fn wanders(&mut self, name: &str) -> usize {
        let wanderer = Wanderer::join(&self.address, name);
        self.wanderers.push(wanderer);
        let index = self.wanderers.len() - 1;
        let what = format!("the wanderer {name} is placed in the world");
        self.until(&what, |wanders| wanders.wanderers[index].noted().joined)
            .await;
        let noted = self.wanderers[index].noted();
        self.note(format!(
            "the wanderer {name} joined at x = {:.1}, z = {:.1}",
            noted.x, noted.z
        ));
        index
    }

    /// Waits until the wanderer `index` has ended every walk it was sent on, for as
    /// long as a walk of `blocks` at `speed` blocks a tick may take.
    pub async fn wanderer_arrives(&mut self, index: usize, blocks: f64, speed: f64) {
        let name = self.wanderers[index].name.clone();
        let what = format!("the wanderer {name} is where it was sent");
        let limit = PATIENCE + 3 * TICK.mul_f64(blocks / speed);
        self.until_within(limit, &what, |wanders| {
            wanders.wanderers[index].has_arrived()
        })
        .await;
        let noted = self.wanderers[index].noted();
        self.note(format!(
            "{name} has arrived at x = {:.1}, z = {:.1}",
            noted.x, noted.z
        ));
    }

    /// Tells the wanderer `index` to leave the game and waits until it has.
    pub async fn wanderer_leaves(&mut self, index: usize) {
        self.wanderers[index].leaves();
        let name = self.wanderers[index].name.clone();
        let what = format!("the wanderer {name} has left");
        self.until(&what, |wanders| wanders.wanderers[index].noted().left)
            .await;
        self.note(format!("the wanderer {name} has left"));
    }

    /// The time of a line of a log on the test's own clock.
    pub fn instant_of(&self, at: f64) -> Instant {
        let since = (at - self.started_at).max(0.0);
        self.started + Duration::from_secs_f64(since)
    }

    /// The time of the logs now.
    pub fn now_at(&self) -> f64 {
        self.started_at + self.started.elapsed().as_secs_f64()
    }

    /// The time of the logs at `instant` of the test's own clock.
    pub fn at_of(&self, instant: Instant) -> f64 {
        self.started_at
            + instant
                .saturating_duration_since(self.started)
                .as_secs_f64()
    }

    /// The coordinator's log in all its lives; of a single process, the one log there
    /// is. Nothing of a `Server` in the test's own process.
    pub fn coordinator_log(&self) -> String {
        match &self.under {
            Under::Cluster(cluster) => cluster.log("coordinator"),
            Under::Process { log, .. } => std::fs::read_to_string(log).unwrap_or_default(),
            Under::Server(..) => String::new(),
        }
    }

    /// The logs of the workers in all their lives, each with the worker's name; of a
    /// single process, the one log there is.
    pub fn worker_logs(&self) -> Vec<(String, String)> {
        match &self.under {
            Under::Cluster(cluster) => (0..cluster.workers.len())
                .map(|worker| (worker_name(worker), cluster.log(&worker_name(worker))))
                .collect(),
            Under::Process { log, .. } => vec![(
                "the server".to_owned(),
                std::fs::read_to_string(log).unwrap_or_default(),
            )],
            Under::Server(..) => Vec::new(),
        }
    }

    /// Every line of the workers' logs that has `words`, in the order of their
    /// times, each with its time and the worker that wrote it.
    pub fn said_by_workers(&self, words: &str) -> Vec<(f64, String, String)> {
        let mut said: Vec<(f64, String, String)> = Vec::new();
        for (name, log) in self.worker_logs() {
            for line in log.lines().filter(|line| line.contains(words)) {
                if let Some(at) = time_of(line) {
                    said.push((at, name.clone(), line.to_owned()));
                }
            }
        }
        said.sort_by(|one, other| one.0.total_cmp(&other.0));
        said
    }

    /// Every player a worker says has arrived from another region or departed to
    /// one: each a hand-over somebody could have noticed.
    pub fn handed_over(&self) -> Vec<(f64, String)> {
        let mut lines = self.said_by_workers("player arrived from another region");
        lines.extend(self.said_by_workers("player departed to another region"));
        lines.sort_by(|one, other| one.0.total_cmp(&other.0));
        lines
            .into_iter()
            .map(|(at, worker, line)| (at, format!("{worker}: {line}")))
            .collect()
    }

    /// Fails if a worker has said, from the time `from` of the logs on, that a player
    /// arrived from another region or departed to one.
    pub fn nobody_was_handed_over(&mut self, from: f64, when: &str) {
        let handed: Vec<String> = self
            .handed_over()
            .into_iter()
            .filter(|(at, _)| *at >= from)
            .map(|(_, line)| line)
            .collect();
        if !handed.is_empty() {
            self.fail(&format!(
                "nobody was to see another region's land {when}, and so nobody was to be \
                 handed over; the workers say:\n  {}",
                handed.join("\n  ")
            ));
        }
    }

    /// How long every region stood still for a merge or a split, by the workers'
    /// lines.
    pub fn stood(&self) -> Vec<Stood> {
        let lines = self.said_by_workers("a region stood still for a merge or a split");
        let read = |(at, _, line): &(f64, String, String)| {
            Some(Stood {
                at: *at,
                region: number(line, "region")?,
                players: number(line, "players")?,
                held: number(line, "held")?,
                milliseconds: number(line, "milliseconds")?,
            })
        };
        lines.iter().filter_map(read).collect()
    }

    /// What every split took, by the lines of the workers that made them.
    pub fn split_off(&self) -> Vec<SplitOff> {
        let lines = self.said_by_workers("a part of the region has been split off");
        let read = |(at, _, line): &(f64, String, String)| {
            Some(SplitOff {
                at: *at,
                part: number(line, "part")?,
                players: number(line, "players")?,
                chunks: number(line, "chunks")?,
                waited: number(line, "waited")?,
            })
        };
        lines.iter().filter_map(read).collect()
    }

    /// What regions that were split took for their parts' of what edges asked them
    /// for, by the workers' lines.
    pub fn taken(&self) -> Vec<Taken> {
        let words = "chunks asked for players who went are taken for the part's";
        let lines = self.said_by_workers(words);
        let read = |(at, _, line): &(f64, String, String)| {
            Some(Taken {
                at: *at,
                part: number(line, "part")?,
                chunks: number(line, "chunks")?,
                free: number(line, "free")?,
            })
        };
        lines.iter().filter_map(read).collect()
    }

    /// When a worker last said that it runs `region`, by the time of the logs.
    pub fn last_ran(&self, region: Region) -> Option<f64> {
        let running = format!("running a region region={region} ");
        self.said_by_workers(&running).last().map(|(at, ..)| *at)
    }

    /// Everything the coordinator began by itself, in all its lives, in order, each
    /// with the time of its line.
    pub fn begun(&self) -> Vec<(f64, Begun)> {
        let log = self.coordinator_log();
        let lines = log.lines();
        lines
            .filter_map(|line| Some((time_of(line)?, Begun::read(line)?)))
            .collect()
    }

    /// The merges, absorptions and splits the coordinator began by itself, in order.
    pub fn reshapes(&self) -> Vec<Begun> {
        let begun = self.begun().into_iter().map(|(_, begun)| begun);
        begun.filter(Begun::reshapes).collect()
    }

    /// The merges, absorptions and splits the coordinator began by itself, in order,
    /// each with the time of its line.
    pub fn reshapes_at(&self) -> Vec<(f64, Begun)> {
        let begun = self.begun().into_iter();
        begun.filter(|(_, begun)| begun.reshapes()).collect()
    }

    /// How every merge and split ended, and every region that was assigned, in order,
    /// each with the time of its line.
    pub fn ended(&self) -> Vec<(f64, Ended)> {
        let log = self.coordinator_log();
        let lines = log.lines();
        lines
            .filter_map(|line| Some((time_of(line)?, Ended::read(line)?)))
            .collect()
    }

    /// What the coordinator began by itself and how each merge and split ended, in
    /// the order of its log, for a message.
    pub fn told(&self) -> Vec<String> {
        let mut lines: Vec<(f64, String)> = Vec::new();
        for (at, begun) in self.begun() {
            lines.push((at, format!("begun: {}", begun.told())));
        }
        for (at, ended) in self.ended() {
            if let Some(told) = ended.told() {
                lines.push((at, format!("ended: {told}")));
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
    /// whether it is the one. Of a `Server` in the test's own process nothing can be
    /// said.
    pub fn has_begun(&mut self, counted: usize, what: &str, expected: &[&dyn Fn(&Begun) -> bool]) {
        if !self.has_logs() {
            return;
        }
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

    /// How the thing `begun` at the time `at` ended, by the first line after it that
    /// is about the same regions, with the time of that line.
    pub fn end_of(&self, at: f64, begun: &Begun) -> Option<(f64, Ended)> {
        let mut ended = self.ended().into_iter();
        ended.find(|(time, ended)| *time >= at && begun.is_ended_by(ended))
    }

    /// The region the bots of `stands` were in at the time `at` of the coordinator's
    /// log, by the merges and splits that had ended well by then.
    pub fn region_at(stands: Stands, ended: &[(f64, Ended)], at: f64) -> Region {
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

    /// The bound of ADR-0016. Of everything the coordinator began by itself from the
    /// time `from` of its log on, by the four lines the record has for it, no bot's
    /// region is in more than `1 + W / rest` in any time `W`: a player who does not
    /// walk into another region's chunks is stood still at most once in a rest.
    /// `stand` says which region each group's bots are in; of the groups that go at a
    /// split, the move of their part is left out, as the record allows that one.
    pub fn nobody_is_stood_still_more_than_once_in_a_rest(&mut self, from: f64, stand: &[Stands]) {
        let ended = self.ended();
        let begun: Vec<(f64, Begun)> = self.begun();
        let begun: Vec<&(f64, Begun)> = begun.iter().filter(|(at, _)| *at >= from).collect();
        let rest = self.setup.rest;
        for stands in stand {
            let theirs: Vec<&(f64, Begun)> = begun
                .iter()
                .copied()
                .filter(|(_, begun)| !(stands.goes && matches!(begun, Begun::Move { .. })))
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
                    let may = 1.0 + time / rest.as_secs_f64();
                    let were = last - first + 1;
                    if were as f64 > may {
                        self.fail(&format!(
                            "the region of group {} was in {were} things the coordinator began \
                             by itself within {:.3} s, from {} to {}; with a rest of {rest:?} \
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

    /// The longest any bot of a group waited for an acknowledgement between `from`
    /// and now.
    pub fn longest_pause_since(&self, from: Instant) -> Duration {
        let now = Instant::now();
        let groups = self.groups.iter();
        let pauses = groups.flat_map(|group| group.progress.longest_waits(from, now));
        let lasted = pauses.flatten().map(|wait| wait.lasted(now));
        lasted.max().unwrap_or_default()
    }

    /// Fails if a bot of a group has waited longer for an acknowledgement since
    /// `from` than a player may at a merge or a split.
    pub fn nobody_waited_too_long(&mut self, from: Instant) {
        let now = Instant::now();
        // The longest wait of all, with whose it was and when it began.
        let mut longest: Option<(Duration, String, Duration)> = None;
        for group in &self.groups {
            let waits = group.progress.longest_waits(from, now);
            for (bot, wait) in waits.iter().enumerate() {
                let Some(wait) = wait else {
                    continue;
                };
                let lasted = wait.lasted(now);
                if longest.as_ref().is_none_or(|(most, ..)| lasted > *most) {
                    let whose = format!("bot {bot} of group {}", group.name);
                    let since = wait.sent.saturating_duration_since(self.started);
                    longest = Some((lasted, whose, since));
                }
            }
        }
        let Some((lasted, whose, since)) = longest else {
            return;
        };
        self.note(format!(
            "the longest any bot waited for an acknowledgement was {}: {whose}, for what it \
             sent at {}",
            seconds(lasted),
            seconds(since)
        ));
        if lasted > LONGEST_PAUSE {
            self.fail(&format!(
                "{whose} waited {} for an acknowledgement of what it sent at {}; it may wait \
                 {LONGEST_PAUSE:?}",
                seconds(lasted),
                seconds(since)
            ));
        }
    }

    /// The longest any bot of the group `name` waited for an acknowledgement among
    /// everything that was under way at some moment from `from` to `to`.
    pub fn longest_wait_of(&self, name: &str, from: Instant, to: Instant) -> Option<Duration> {
        let now = Instant::now();
        let waits = self.group(name).progress.longest_waits(from, to);
        waits.iter().flatten().map(|wait| wait.lasted(now)).max()
    }

    /// When every bot of a group had had something acknowledged that it sent after
    /// `at`: the moment by which the world was whole again for the players. Now, if a
    /// bot has not yet.
    pub fn served_after(&self, at: Instant) -> Instant {
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
    /// each thing was begun and had ended, by the coordinator's log, with what the
    /// worker says of how long the region stood still. `stand` says which region each
    /// group's bots are in.
    pub fn measured(&self, from: f64, stand: &[Stands]) -> Vec<Measured> {
        let ended = self.ended();
        let begun = self.begun();
        let stood = self.stood();
        let now = Instant::now();
        let mut measured = Vec::new();
        for (at, begun) in begun.iter().filter(|(at, _)| *at >= from) {
            let end = self.end_of(*at, begun);
            let begun_at = self.instant_of(*at);
            let done_at = end.as_ref().map(|(time, _)| self.instant_of(*time));
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
            // The line of the region that went on: the survivor's, or that of the
            // region that was split, written when it ticks again.
            let went_on = match begun {
                Begun::Merge { survivor, .. } | Begun::Absorption { survivor, .. } => {
                    Some(*survivor)
                }
                Begun::Split { region, .. } => Some(*region),
                Begun::Move { .. } => None,
            };
            let until = end.as_ref().map_or(f64::MAX, |(time, _)| *time + 2.0);
            let stood = stood.iter().copied().find(|stood| {
                Some(stood.region) == went_on && stood.at >= *at - WRITING && stood.at <= until
            });
            measured.push(Measured {
                begun: begun.clone(),
                begun_after: after(begun_at),
                done_after: done_at.and_then(after),
                cause: cause.as_ref().map(|(_, what)| what.clone()),
                stayed,
                went,
                stood,
            });
        }
        measured
    }

    /// Prints what [`Wanders::measured`] gives, as a table.
    pub fn prints(&self, measured: &[Measured]) {
        println!(
            "{}: what the coordinator began by itself (seed {}), and what the bots noticed:",
            self.test, self.seed
        );
        println!(
            "  what | after what | begun | done | those who stayed waited | those who went \
             waited | the region stood still (players, chunks held)"
        );
        for measured in measured {
            let stood = measured.stood.map_or("-".to_owned(), |stood| {
                format!(
                    "{} ms ({}, {})",
                    stood.milliseconds, stood.players, stood.held
                )
            });
            println!(
                "  {} | {} | {} | {} | {} | {} | {stood}",
                measured.begun.told(),
                measured.cause.as_deref().unwrap_or("-"),
                perhaps(measured.begun_after),
                perhaps(measured.done_after),
                perhaps(measured.stayed),
                perhaps(measured.went),
            );
        }
    }

    /// Sends a process a signal.
    pub async fn signal(process: &Child, signal: &str) {
        let pid = process.id().expect("it runs").to_string();
        let sent = Command::new("kill")
            .args([format!("-{signal}"), pid])
            .status()
            .await;
        assert!(sent.unwrap().success());
    }

    /// Ends every group that still plays, each with its auditor, and every wanderer,
    /// and fails unless all found everything as the ledgers say.
    pub async fn everybody_leaves(&mut self) {
        let playing = self.groups.iter().filter(|group| group.playing.is_some());
        let playing: Vec<&'static str> = playing.map(|group| group.name).collect();
        for name in playing {
            self.leaves(name).await;
        }
        for index in 0..self.wanderers.len() {
            let noted = self.wanderers[index].noted();
            if noted.joined && !noted.left {
                self.wanderer_leaves(index).await;
            }
        }
        self.tend().await;
    }

    /// Ends every group that still plays, each with its auditor, and fails unless all
    /// found everything as the ledgers say. Then everything that serves the world is
    /// killed and started again, and the list of regions and every block the ledgers
    /// have a word about have to be on disk as they were. Then everything is asked to
    /// stop, which it has to do cleanly.
    pub async fn finish(mut self) {
        self.everybody_leaves().await;
        for line in self.told() {
            println!("{}: {line}", self.test);
        }
        let test = self.test.clone();
        match &mut self.under {
            Under::Cluster(_) => self.the_cluster_starts_from_disk().await,
            Under::Server(server, config) => {
                let (before, config) = (server.as_ref().map(Server::regions), config.clone());
                server.take().expect("it runs").stop().await;
                let again = match Server::start(*config).await {
                    Ok(server) => server,
                    Err(error) => self.fail(&format!("the server did not start again: {error:#}")),
                };
                // A server that starts opens every region anew, with an epoch of its
                // own: the regions, their land and what was absorbed are compared.
                let without_epochs = |list: RegionList| RegionList {
                    regions: list
                        .regions
                        .into_iter()
                        .map(|info| RegionInfo { epoch: 0, ..info })
                        .collect(),
                    ..list
                };
                let after = again.regions().ok().map(without_epochs);
                let before = before.and_then(Result::ok).map(without_epochs);
                self.address = again.address().to_string();
                if let Under::Server(server, _) = &mut self.under {
                    *server = Some(again);
                }
                self.note("stopped the server and started it from its disk".to_owned());
                if before != after {
                    self.fail(&format!(
                        "the regions are others after the server was stopped and started \
                         again: {before:?} before, {after:?} after"
                    ));
                }
                self.audits_from_disk().await;
                if let Under::Server(server, _) = &mut self.under {
                    server.take().expect("it runs").stop().await;
                }
            }
            Under::Process {
                server,
                world,
                log,
                arguments,
            } => {
                let mut process = server.take().expect("it runs");
                process.kill().await.unwrap();
                let more: Vec<&str> = arguments.iter().map(String::as_str).collect();
                let again = spawn_server_with_log(&self.address, world, &more, log).await;
                *server = Some(again);
                self.note("killed the server and started it from its disk".to_owned());
                self.audits_from_disk().await;
                self.tend().await;
                let Under::Process { server, .. } = &mut self.under else {
                    unreachable!("it is a process");
                };
                let mut process = server.take().expect("it runs");
                Self::signal(&process, "TERM").await;
                let Ok(status) = tokio::time::timeout(PATIENCE, process.wait()).await else {
                    self.fail("the server did not stop at the end of the test");
                };
                if !status.unwrap().success() {
                    self.fail("the server ended with an error at the end of the test");
                }
            }
        }
        let variable = format!("CLUSTINE_{}_KEEP", self.setup.family.to_uppercase());
        if std::env::var_os(variable).is_some() {
            let kept = self.keep();
            println!("{test}: kept: {}", kept.display());
        }
    }

    /// Fails unless every block the ledgers of the groups have a word about is as
    /// they say, for somebody who joins now.
    async fn audits_from_disk(&mut self) {
        let address = self.address.clone();
        for index in 0..self.groups.len() {
            let group = &self.groups[index];
            let report = group.report.as_ref().expect("every group has left");
            if let Err(error) = audit_blocks(&address, &group.ledger, &report.blocks).await {
                let name = group.name;
                self.fail(&format!(
                    "after everything was started again from disk, what group {name} built is \
                     not as its ledgers say: {error:#}"
                ));
            }
        }
        self.note("the world is as the ledgers say after starting from disk".to_owned());
    }

    /// The end of a test on a cluster: everything but the store is killed and the
    /// list is as it was; the store is started again and the list is the same; the
    /// whole cluster is started again from disk and the blocks are audited; and every
    /// process stops cleanly when asked.
    async fn the_cluster_starts_from_disk(&mut self) {
        // The coordinator goes on merging and splitting for as long as it runs, so the
        // list is read when only the store is left, which does nothing by itself, and
        // again from the store that is started on the same disk.
        for (name, process) in self.processes().processes() {
            if name != "worldstore"
                && let Some(mut process) = process.take()
            {
                process.kill().await.unwrap();
            }
        }
        let before = self.list().await;
        self.processes().kill().await;
        self.note("killed every process".to_owned());
        self.processes().start_store();
        let waiting = Instant::now();
        let after = loop {
            match self.reads().await {
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
        self.processes().kill().await;
        self.processes().start().await;
        self.audits_from_disk().await;
        self.tend().await;
        self.stop_everything().await;
    }

    /// Asks every process of the cluster to stop and fails unless each ends without an
    /// error: the edge, then the coordinator, then the workers and the world store.
    /// With the coordinator gone a worker has nobody to hand its regions to and knows
    /// it, so it saves and stops instead of waiting for someone to take over.
    async fn stop_everything(&mut self) {
        let cluster = self.processes();
        let mut processes: Vec<(String, Option<Child>)> = vec![
            ("edge".to_owned(), cluster.edge.1.take()),
            ("coordinator".to_owned(), cluster.coordinator.1.take()),
        ];
        for (number, worker) in cluster.workers.iter_mut().enumerate() {
            processes.push((worker_name(number), worker.1.take()));
        }
        processes.push(("worldstore".to_owned(), cluster.store.1.take()));
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
