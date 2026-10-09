//! A coordinator that decides by itself when regions merge and split, in generated
//! runs: the properties R1 to R7 of `docs/adr/0016-when-to-merge-and-split.md`, section
//! 11, written from the record alone. Whoever wrote these read the record, the messages,
//! the pure rule (`policy.rs`) and the coordinator's public signatures with their
//! comments, and not the state machine, so that a property here says what the record
//! asks for and not what the code happens to do.
//!
//! - `follows/model.rs` is the world of section 11: players that are points and walk a
//!   chunk in two seconds, regions that are sets of players, the store's list.
//! - `follows/judge.rs` has the properties, each as a check of what is begun.
//! - `follows/stage.rs` plays the model alone, with a script in the coordinator's place:
//!   the model's own invariants, runs played as the record asks, and for every property
//!   a run in which it is broken on purpose.
//! - This file plays a cluster against the coordinator from a seed: workers that do
//!   what they are ordered after a delay, say what came of it and report their regions'
//!   true crowds at every step behind the word of an outcome, and what the variants add
//!   (reports that are a step old, hand-overs, workers that die or leave, readings that
//!   fail, a coordinator made anew). And a script plays the same cluster through what
//!   players who walk at random seldom bring up: K2 (b), K4, K10, K12 and K22.
//!
//! Whoever writes these runs cannot put a fault into a state machine they do not read.
//! That the runs catch the faults step C4.6 names is shown three ways: by the scripted
//! runs of `follows/stage.rs`; by a decider in the test that asks by hand in the place
//! of a coordinator which decides nothing, carefully or with a fault
//! (`Cluster::decide_in_its_place`); and by misleading the coordinator itself, with
//! other numbers than the judge goes by or a list that is not the model's, so that
//! what it rightly does by what it was told is a fault to the judge.
//!
//! In every step the model first says what came of what and hands in a reading of the
//! list if one is due, then gives its reports, and then calls `tick` once, all at one
//! time of the test's clock. That call is the look of the step.
//!
//! Each kind of run plays eight seeds; `CLUSTINE_FOLLOWS_RUNS` asks for more and
//! `CLUSTINE_FOLLOWS_SEED` for a certain one, and with `--nocapture` each kind says what
//! its runs were about. A run that fails says its seed and what led there.
//!
//! Where the model is simpler than section 11 has it, or says more:
//!
//! - A player moves along both axes at once, which is one chunk by the record's
//!   distance, and once in two seconds at most.
//! - A region that no worker runs stands still: its players neither move nor leave,
//!   nobody is handed into it or out of it, and nobody joins while it is the home
//!   region. A hand-over is from any region to any other, wherever their players are:
//!   the model has no chunks that a region holds.
//! - A worker says what came of an order in the step in which it does it, and its word
//!   reaches the coordinator at once; what the coordinator tells a worker takes zero to
//!   four steps, in the order it was told, and what the look of a step tells is done in
//!   the next step at the earliest. A worker without a connection reports nothing.
//! - A reading of the list that is asked for is handed in in the same step if it was
//!   asked for before the reports are given, and in the next otherwise.
//! - A split answers `Off(Nobody)` or is made; no other answer of a worker's comes of
//!   one, but that a worker which does not run the region says so.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Instant;

use clustine_coordinator::{
    Asked, Changes, Coordinator, CoordinatorConfig, Order, Policy, Wanted, named,
};
use clustine_region::RegionId;
use clustine_rpc::{Assignment, Off, PlayersOf, Vouch};
use clustine_world::{ChunkPos, Vec3};

#[path = "follows/judge.rs"]
mod judge;
#[path = "follows/model.rs"]
mod model;
#[path = "follows/stage.rs"]
mod stage;

use judge::{Begun, Breach, Judge, Property};
use model::{
    DELAY, Dice, HOME, Known, LEASE, LOOK, ORIGIN, Player, Stale, World, crowds, policy, steps,
};

/// Every epoch a coordinator of these runs issues is above this.
const FIRST_EPOCH: u64 = 1_000;

const WORKERS: [&str; 4] = ["w0", "w1", "w2", "w3"];

/// For how many steps the players walk, join and leave while things go wrong; for how
/// many more they do while nothing goes wrong any more, which is more than the eight
/// leases R5 asks for; and for how many steps the run is watched after R5's deadline.
const ACTIVE: u64 = 1_000;
const CALM: u64 = 8 * 20 + 12;
const HUSH: u64 = 200;

/// How many players a run has at most.
const CROWD: usize = 14;

fn address(name: &str) -> String {
    format!("{name}:25600")
}

/// A coordinator that knows `regions` stripes from the start, numbered from 0, as
/// every coordinator did before it learnt its regions from the world store's list
/// (`docs/adr/0017-the-end-of-the-stripes.md`, section 2.3). These tests are about
/// what a coordinator does with regions it knows.
fn knowing(config: CoordinatorConfig, regions: u32, now: Instant, first_epoch: u64) -> Coordinator {
    let stripes: Vec<RegionId> = (0..regions).map(RegionId).collect();
    Coordinator::knowing(config, now, first_epoch, &stripes)
}

/// What a run adds to the model of section 11.
#[derive(Debug, Clone, Copy)]
struct Variant {
    name: &'static str,
    /// How many regions the world begins with: one, the home region, or three.
    regions: u32,
    /// Whether the regions it begins with other than the home region are pinned.
    pinned: bool,
    /// Reports that are a step old when they are given.
    lagging: bool,
    /// Players handed from one region to another, with one stale report (K10).
    handovers: bool,
    /// Workers that die and are replaced.
    deaths: bool,
    /// Workers that leave.
    leavers: bool,
    /// Readings of the list that fail.
    failing: bool,
    /// The coordinator is made anew.
    anew: bool,
    /// Whether the coordinator decides by itself.
    follow: bool,
    /// What the coordinator is told to go by, if not by the distances and the rest
    /// that the judge holds it to: to show that a coordinator which goes by others is
    /// caught.
    told: Option<Policy>,
    /// How the store's list misleads the coordinator, if it does, for the same.
    list: Option<Misleads>,
    /// Whether the crowds of every report are given in descending order (R6).
    reversed: bool,
    /// Whether the test asks by hand for what a coordinator that decides by itself
    /// would begin, in the place of one that decides nothing, and how well it does
    /// that (`Cluster::decide_in_its_place`).
    decider: Option<Decider>,
}

/// How the list of a run misleads the coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Misleads {
    /// It has no region pinned, where the model has: to the judge, a coordinator that
    /// goes by it absorbs pinned regions for being empty.
    Unpinned,
    /// No reading succeeds once the players stand: to the judge, a coordinator that
    /// does not get the list read and begins nothing, as it then must.
    Lost,
}

/// How the test decides in the coordinator's place: as the record asks, or with one of
/// the faults that step C4.6 is to catch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decider {
    /// What has stood for a second, of regions that have rested.
    Careful,
    /// The fault "no rest": whether a region has rested is not looked at.
    Restless,
    /// The fault "no standing": what is wanted is begun at the look that first wants it.
    Hasty,
    /// The fault "a group that goes on its first look": a split that has stood names
    /// every group there is at its look.
    Greedy,
    /// The fault "the list never read on the timer", where what holds back for the age
    /// of the last reading went with the timer: how old the list is is not looked at.
    Unread,
}

impl Variant {
    const QUIET: Self = Self {
        name: "nothing goes wrong",
        regions: 3,
        pinned: false,
        lagging: false,
        handovers: false,
        deaths: false,
        leavers: false,
        failing: false,
        anew: false,
        follow: true,
        told: None,
        list: None,
        reversed: false,
        decider: None,
    };
    const ONE_HOME: Self = Self {
        name: "one home region, reports a step old",
        regions: 1,
        lagging: true,
        ..Self::QUIET
    };
    const HANDED: Self = Self {
        name: "hand-overs with a stale report",
        lagging: true,
        handovers: true,
        ..Self::QUIET
    };
    const PINNED: Self = Self {
        name: "pinned regions",
        pinned: true,
        lagging: true,
        handovers: true,
        ..Self::QUIET
    };
    const DEATHS: Self = Self {
        name: "workers that die or leave, readings that fail",
        lagging: true,
        handovers: true,
        deaths: true,
        leavers: true,
        failing: true,
        ..Self::QUIET
    };
    const ANEW: Self = Self {
        name: "a coordinator made anew",
        pinned: true,
        anew: true,
        ..Self::DEATHS
    };
    const ALL: [Self; 6] = [
        Self::QUIET,
        Self::ONE_HOME,
        Self::HANDED,
        Self::PINNED,
        Self::DEATHS,
        Self::ANEW,
    ];
}

/// What happens in a step besides what the workers and the coordinator do.
#[derive(Debug, Clone)]
enum Act {
    /// A player joins, at the origin.
    Join,
    Leave(Player),
    /// A player takes a step towards a chunk, if they may move yet.
    Walk(Player, ChunkPos),
    Hand(Player, RegionId, Stale),
    /// A worker's process dies.
    Kill(&'static str),
    /// A worker's process is started, anew.
    Start(&'static str),
    /// A worker is told to stop, and says that it is leaving.
    Stop(&'static str),
    /// A coordinator that knows nothing takes the place of the one that was.
    Anew,
}

/// What the coordinator tells a worker.
#[derive(Debug, Clone)]
enum Told {
    Orders(Vec<Assignment>),
    Release(RegionId, u64),
    Reshape(Order),
}

/// What a worker tells the coordinator, besides its heartbeats and its reports.
#[derive(Debug, Clone)]
enum Said {
    Released(RegionId, u64),
    EpochRefused(RegionId, u64),
    AbsorbEnded(RegionId, RegionId, Result<(), Off>),
    SplitEnded(RegionId, u64, Result<RegionId, Off>),
    Leaving,
}

/// What comes of opening a region at the store.
enum Opened {
    Yes,
    /// The store has seen an owner with a higher epoch.
    Refused {
        seen: u64,
    },
    /// The region was absorbed by `into`.
    Absorbed {
        into: RegionId,
    },
}

/// What the store has of who runs a living region: the highest epoch it was opened
/// with, and the worker that has it open with that epoch, if it has not closed it. A
/// region is run by that worker and by no other.
#[derive(Debug, Clone)]
struct Lane {
    epoch: u64,
    holder: Option<&'static str>,
}

/// A worker process: it opens what it is ordered to run, lets go of what it is asked to,
/// absorbs and splits, each after a delay, and says what came of it.
#[derive(Debug, Clone, Default)]
struct Process {
    alive: bool,
    /// Whether it has registered with the coordinator that is there.
    connected: bool,
    /// Whether it was told to stop, which it says after every registration.
    leaving: bool,
    /// What it runs, each region under the assignment it holds it with.
    runs: BTreeMap<RegionId, Assignment>,
    /// The parts it made that no orders have named yet, each with the region it was
    /// split off and the epoch that was ordered.
    parts: BTreeMap<RegionId, (RegionId, u64)>,
    /// The assignments it let go of, which it never takes up again.
    released: BTreeSet<(RegionId, u64)>,
    /// What it was told and has not done, each with the step at which it does it.
    inbox: VecDeque<(u64, Told)>,
    /// The step at which it registers, if it is to.
    registers: Option<u64>,
}

/// A cluster played against a coordinator: the world of the model, the workers that
/// run its regions, and the service's part of reading the list.
struct Cluster {
    seed: u64,
    variant: Variant,
    dice: Dice,
    config: CoordinatorConfig,
    coordinator: Coordinator,
    started: Instant,
    world: World,
    judge: Judge,
    lanes: BTreeMap<RegionId, Lane>,
    workers: BTreeMap<&'static str, Process>,
    /// What the coordinator has after its last call.
    knows: Known,
    /// Whether what a worker is told in this step can still be done in it: only while
    /// the workers say what came of what.
    early: bool,
    /// Whether the list is to be read.
    wanted: bool,
    /// The regions that are being released, for a move.
    releasing: BTreeSet<RegionId>,
    /// The regions whose report of this step is a step old, for a hand-over.
    old: BTreeSet<RegionId>,
    /// Where the players walk to.
    sites: Vec<ChunkPos>,
    goals: BTreeMap<Player, ChunkPos>,
    /// The highest epoch seen anywhere.
    highest: u64,
    /// Whether no call is to say anything any more, but to read the list (R5).
    hushed: bool,
    /// A number for every call of the coordinator and its answer, in order (R6).
    record: Vec<u64>,
    /// What happened last, for the message of a failed check.
    story: VecDeque<String>,
    /// What had happened last when a property was first broken, if one was.
    led: Option<Vec<String>>,
    /// The step of the last call that said anything but to read the list.
    last_word: u64,
    /// How many steps the coordinator went on saying things after `s'`, and how many
    /// R5 allowed it.
    spent: Option<(u64, u64)>,
    seen: BTreeMap<&'static str, u32>,
}

impl Cluster {
    /// The cluster of a seed: the players a world begins with are around the origin,
    /// and in a world of three regions also far east of it and further still, each lot
    /// in a region of its own; they walk between a handful of sites.
    fn new(seed: u64, variant: Variant) -> Self {
        let mut dice = Dice(seed);
        let mut players: Vec<(u32, i32, i32)> = Vec::new();
        let wide = if variant.regions == 1 {
            for _ in 0..6 {
                players.push((0, dice.around(18), dice.around(18)));
            }
            (-30, 30)
        } else {
            assert_eq!(
                variant.regions, 3,
                "a world begins with one region or three"
            );
            for (region, x, z) in [(0, 0, 0), (1, 26, -4), (2, 54, 6)] {
                for _ in 0..2 + dice.below(2) {
                    players.push((region, x + dice.around(2), z + dice.around(2)));
                }
            }
            (-10, 62)
        };
        let mut sites = vec![ORIGIN];
        for _ in 0..5 {
            let span = u64::try_from(wide.1 - wide.0).expect("east of west");
            let x = wide.0 + i32::try_from(dice.below(span + 1)).expect("a small number");
            sites.push(ChunkPos::new(x, dice.around(24)));
        }
        let mut cluster = Self::with(variant, &players);
        cluster.seed = seed;
        cluster.dice = dice;
        cluster.sites = sites;
        cluster
    }

    /// A cluster whose world begins with the players given, each at a chunk of a region,
    /// for a run that a script plays.
    fn with(variant: Variant, players: &[(u32, i32, i32)]) -> Self {
        let world = World::new(variant.regions, variant.pinned, players);
        // The coordinator knows the regions of the world from the start. Where their
        // chunks are does not matter to it or to the model: the list says which
        // regions there are, and nothing here goes by where a region's chunks are.
        let config = CoordinatorConfig {
            spawn: Vec3::new(0.5, 64.0, 0.5),
            lease: LEASE,
            follow: variant.follow.then(|| variant.told.unwrap_or_else(policy)),
        };
        let (seed, dice, sites) = (0, Dice(0), vec![ORIGIN]);
        let started = Instant::now();
        let mut cluster = Self {
            seed,
            variant,
            dice,
            coordinator: knowing(config.clone(), variant.regions, started, FIRST_EPOCH),
            config,
            started,
            lanes: world
                .regions
                .keys()
                .map(|region| {
                    let never_opened = Lane {
                        epoch: 0,
                        holder: None,
                    };
                    (*region, never_opened)
                })
                .collect(),
            world,
            judge: Judge::default(),
            workers: WORKERS
                .iter()
                .map(|name| (*name, Process::default()))
                .collect(),
            knows: Known::default(),
            early: false,
            // The service reads the list when it starts.
            wanted: true,
            releasing: BTreeSet::new(),
            old: BTreeSet::new(),
            sites,
            goals: BTreeMap::new(),
            highest: FIRST_EPOCH,
            hushed: false,
            record: Vec::new(),
            story: VecDeque::new(),
            led: None,
            last_word: 0,
            spent: None,
            seen: BTreeMap::new(),
        };
        // Three workers are there from the start, and the fourth is a spare.
        for name in &WORKERS[..3] {
            let process = cluster.process(name);
            process.alive = true;
            process.registers = Some(1);
        }
        cluster.knows = cluster.known();
        cluster
    }

    fn now(&self) -> Instant {
        let step = u32::try_from(self.world.step).expect("a run is not that long");
        self.started + LOOK * step
    }

    fn process(&mut self, name: &str) -> &mut Process {
        self.workers.get_mut(name).expect("a worker of the run")
    }

    fn note(&mut self, line: String) {
        if self.story.len() == 400 {
            self.story.pop_front();
        }
        self.story
            .push_back(format!("{:>6}  {line}", self.world.step));
    }

    fn count(&mut self, what: &'static str) {
        *self.seen.entry(what).or_default() += 1;
    }

    /// The model or the way the run is played is wrong, or the coordinator has done
    /// what no cluster can follow: the test fails with the seed and what led there.
    fn fail(&self, what: String) -> ! {
        let story: Vec<&str> = self.story.iter().map(String::as_str).collect();
        panic!(
            "seed {seed}, {name}: {what}\n\nwhat led there:\n{}\n\nseed {seed}, {name} \
             (CLUSTINE_FOLLOWS_SEED={seed}): {what}",
            story.join("\n"),
            seed = self.seed,
            name = self.variant.name,
        );
    }

    /// What the coordinator has, as far as it can be seen from outside.
    fn known(&self) -> Known {
        let table = self.coordinator.routing_table();
        Known {
            routes: table
                .routes
                .iter()
                .map(|route| {
                    let name = route.address.trim_end_matches(":25600").to_owned();
                    (route.region, (name, route.epoch))
                })
                .collect(),
            waiting: self.coordinator.waiting(),
            under_way: self.coordinator.under_way(),
        }
    }

    /// Whether the worker has the region open with that epoch, and so runs it.
    fn holds(&self, name: &str, region: RegionId, epoch: u64) -> bool {
        self.lanes
            .get(&region)
            .is_some_and(|lane| lane.epoch == epoch && lane.holder == Some(name))
            && self.workers[name]
                .runs
                .get(&region)
                .is_some_and(|held| held.epoch == epoch)
    }

    /// The regions that some worker runs: their players can move.
    fn running(&self) -> BTreeSet<RegionId> {
        self.lanes
            .iter()
            .filter(|(region, lane)| {
                lane.holder
                    .is_some_and(|name| self.holds(name, **region, lane.epoch))
            })
            .map(|(region, _)| *region)
            .collect()
    }

    // The coordinator's calls, and what follows from each.

    /// A call of the coordinator that returns what it changed.
    fn call(
        &mut self,
        what: String,
        look: bool,
        call: impl FnOnce(&mut Coordinator, Instant) -> Changes,
    ) {
        let now = self.now();
        let changes = call(&mut self.coordinator, now);
        self.after(what, look, changes);
    }

    /// What a call changed is checked by the properties and passed on as the service
    /// does. `look` is whether the call is the `tick` of a step.
    fn after(&mut self, what: String, look: bool, changes: Changes) {
        let step = self.world.step;
        let before = std::mem::take(&mut self.knows);
        let known = self.known();
        // The answer of a call is what it says it changed and what the coordinator has
        // afterwards.
        self.record
            .push(number(&format!("{what} -> {changes:?}, {known:?}")));
        if changes != Changes::default() {
            self.note(format!("{what} -> {changes:?}"));
        }
        let only_read = Changes {
            read: changes.read,
            ..Changes::default()
        };
        if changes != only_read {
            self.last_word = step;
        }

        // What the coordinator has begun by itself: nobody asks by hand in these runs.
        let begun: Vec<Asked> = known
            .under_way
            .iter()
            .filter(|asked| !before.under_way.contains(asked))
            .copied()
            .collect();
        if !look && !begun.is_empty() {
            self.judge.breach(
                Property::Call,
                step,
                format!("{what} begins {begun:?}, and nothing is decided in any call but `tick`"),
            );
        }
        let mut splits = 0;
        for asked in &begun {
            let what = match asked {
                Asked::Merge { survivor, absorbed } => Begun::Merge {
                    survivor: *survivor,
                    absorbed: *absorbed,
                },
                Asked::Split { region } => {
                    let named = changes.orders.iter().find_map(|order| match &order.order {
                        Order::SplitOff {
                            region: of, chunks, ..
                        } if of == region => Some(chunks.clone()),
                        _ => None,
                    });
                    let Some(chunks) = named else {
                        self.judge.breach(
                            Property::Call,
                            step,
                            format!("{asked:?} is under way, and no worker is told to split"),
                        );
                        continue;
                    };
                    Begun::Split {
                        region: *region,
                        chunks,
                    }
                }
            };
            let kind = self.judge.begun(&self.world, &before, &what, splits);
            if matches!(what, Begun::Split { .. }) {
                splits += 1;
            }
            self.note(format!("    begun by itself, {kind:?}: {what:?}"));
        }
        for release in &changes.releases {
            // The release of a region that is to be absorbed is part of its merge.
            let for_a_merge = known.under_way.iter().any(
                |asked| matches!(asked, Asked::Merge { absorbed, .. } if *absorbed == release.region),
            );
            if for_a_merge {
                continue;
            }
            let first = self.releasing.insert(release.region);
            let leaver = WORKERS
                .iter()
                .any(|name| *name == release.worker && self.workers[name].leaving);
            if leaver {
                // A leaving worker's regions are released whether they rest or not.
                self.count("releases of a leaving worker's regions");
            } else if look && first && self.variant.decider.is_none() {
                // (Where the test decides in its place, the coordinator decides nothing
                // by itself and evens out as it always has, a lease after a merge or a
                // split and whether the region rests or not.)
                let what = Begun::EvenOut {
                    region: release.region,
                };
                self.judge.begun(&self.world, &before, &what, 0);
            }
        }
        for ended in &changes.reshaped {
            self.judge.ended(step, ended.asked, ended.outcome.is_ok());
        }
        self.judge.routes(step, &known.routes);
        self.judge.under_way(step, &known.under_way);
        if !self.variant.follow && self.variant.decider.is_none() {
            self.judge.begins_nothing(step, &changes, &known.under_way);
        }
        if self.hushed {
            self.judge.hushed(step, &what, &changes);
        }
        // A release has ended when the region is no longer its owner's.
        self.releasing.retain(|region| {
            before.routes.contains_key(region)
                && before.routes.get(region) == known.routes.get(region)
        });
        for (_, epoch) in known.routes.values() {
            self.highest = self.highest.max(*epoch);
        }
        for order in &changes.orders {
            if let Order::Absorb { as_epoch, .. } | Order::SplitOff { as_epoch, .. } = &order.order
            {
                self.highest = self.highest.max(*as_epoch);
            }
        }
        self.knows = known;

        if changes.read {
            self.wanted = true;
        }
        for name in &changes.workers {
            let orders = self.coordinator.assignments(name);
            self.tell(name, Told::Orders(orders));
        }
        for release in &changes.releases {
            self.tell(
                &release.worker,
                Told::Release(release.region, release.epoch),
            );
        }
        for order in &changes.orders {
            self.tell(&order.worker, Told::Reshape(order.order.clone()));
        }
        for name in &changes.gone {
            self.exit(name);
        }
        if self.led.is_none() && !self.judge.breaches.is_empty() {
            self.led = Some(self.story.iter().cloned().collect());
        }
    }

    /// The coordinator's word for a worker goes onto its connection, if it has one, and
    /// the worker does what it says zero to four steps later, in the order it was told.
    fn tell(&mut self, name: &str, told: Told) {
        let step = self.world.step;
        let soonest = if self.early { step } else { step + 1 };
        let delay = self.dice.below(DELAY + u64::from(self.early));
        let process = self.process(name);
        if !process.alive || !process.connected {
            self.note(format!("    lost, as {name} has no connection: {told:?}"));
            return;
        }
        let behind = process.inbox.back().map_or(0, |(due, _)| *due);
        process
            .inbox
            .push_back(((soonest + delay).max(behind), told));
    }

    /// A worker's word reaches the coordinator at once, if it has a connection.
    fn say(&mut self, name: &'static str, said: Said) {
        if !self.workers[name].connected {
            self.note(format!("    {name} has no connection to say {said:?}"));
            return;
        }
        let what = format!("{name} says {said:?}");
        match said {
            Said::Released(region, epoch) => self.call(what, false, |coordinator, now| {
                coordinator.released(now, name, region, epoch)
            }),
            Said::EpochRefused(region, seen) => {
                self.highest = self.highest.max(seen);
                self.call(what, false, |coordinator, now| {
                    coordinator.epoch_refused(now, name, region, seen)
                });
            }
            Said::AbsorbEnded(region, absorbed, outcome) => {
                self.call(what, false, |coordinator, now| {
                    coordinator.absorb_ended(now, name, region, absorbed, outcome)
                });
            }
            Said::SplitEnded(region, as_epoch, outcome) => {
                if let Ok(part) = outcome {
                    self.judge.told_part(part);
                }
                self.call(what, false, |coordinator, now| {
                    coordinator.split_ended(now, name, region, as_epoch, outcome)
                });
            }
            Said::Leaving => self.call(what, false, |coordinator, now| {
                coordinator.leaving(now, name)
            }),
        }
    }

    // What the workers do.

    /// A hello for the region at the store, which fences by the epoch (ADR-0008).
    fn open(&mut self, name: &'static str, region: RegionId, epoch: u64) -> Opened {
        if let Some((_, into)) = self.world.absorbed.iter().find(|(gone, _)| *gone == region) {
            return Opened::Absorbed { into: *into };
        }
        let Some(lane) = self.lanes.get(&region).cloned() else {
            self.fail(format!(
                "{name} is to open region {region}, which there never was"
            ));
        };
        if epoch < lane.epoch {
            return Opened::Refused { seen: lane.epoch };
        }
        // Whoever had it open has lost it, and stops running it.
        if let Some(holder) = lane.holder
            && holder != name
        {
            let process = self.process(holder);
            process.runs.remove(&region);
            process.parts.remove(&region);
        }
        self.lanes.insert(
            region,
            Lane {
                epoch,
                holder: Some(name),
            },
        );
        Opened::Yes
    }

    fn close(&mut self, name: &str, region: RegionId, epoch: u64) {
        if let Some(lane) = self.lanes.get_mut(&region)
            && lane.epoch == epoch
            && lane.holder == Some(name)
        {
            lane.holder = None;
        }
    }

    /// The worker does the next thing it was told.
    fn deliver(&mut self, name: &'static str, told: Told) {
        self.note(format!("  {name} does as it was told: {told:?}"));
        match told {
            Told::Orders(orders) => self.take_orders(name, &orders),
            Told::Release(region, epoch) => {
                if self.workers[name]
                    .runs
                    .get(&region)
                    .is_some_and(|held| held.epoch == epoch)
                {
                    let process = self.process(name);
                    process.runs.remove(&region);
                    process.parts.remove(&region);
                    self.close(name, region, epoch);
                }
                self.process(name).released.insert((region, epoch));
                self.say(name, Said::Released(region, epoch));
            }
            // A checkpoint, which nothing here sees.
            Told::Reshape(Order::Prepare { .. }) => {}
            Told::Reshape(Order::Absorb {
                region,
                epoch,
                absorbed,
                as_epoch,
            }) => {
                let outcome = if !self.holds(name, region, epoch) {
                    Err(Off::NotRunning)
                } else {
                    match self.open(name, absorbed, as_epoch) {
                        // The order came twice, and the merge has happened already.
                        Opened::Absorbed { into } if into == region => Ok(()),
                        Opened::Absorbed { .. } => Err(Off::Unreadable),
                        Opened::Refused { seen } => {
                            self.say(name, Said::EpochRefused(absorbed, seen));
                            Err(Off::Refused)
                        }
                        Opened::Yes => {
                            self.world.merge(region, absorbed);
                            self.lanes.remove(&absorbed);
                            self.count("merges the workers made");
                            if self.dies_of_it(name) {
                                return;
                            }
                            Ok(())
                        }
                    }
                };
                self.say(name, Said::AbsorbEnded(region, absorbed, outcome));
            }
            Told::Reshape(Order::SplitOff {
                region,
                epoch,
                chunks,
                as_epoch,
                ..
            }) => {
                let outcome = if !self.holds(name, region, epoch) {
                    Err(Off::NotRunning)
                } else {
                    // Whoever stands in the chunks named at this moment goes, under the
                    // store's next id, whatever id the order named.
                    self.world.split(region, &chunks)
                };
                if let Ok(part) = outcome {
                    self.lanes.insert(
                        part,
                        Lane {
                            epoch: as_epoch,
                            holder: Some(name),
                        },
                    );
                    let process = self.process(name);
                    let mut held = process.runs[&region];
                    held.region = part;
                    held.epoch = as_epoch;
                    process.runs.insert(part, held);
                    process.parts.insert(part, (region, as_epoch));
                    self.count("splits the workers made");
                    if self.dies_of_it(name) {
                        return;
                    }
                } else {
                    self.count("splits that found nobody or no runner");
                }
                self.say(name, Said::SplitEnded(region, as_epoch, outcome));
            }
        }
    }

    /// Where workers die, one in eight dies when it has made a merge or a split and
    /// before it says so: the list has what it did, and nobody has its word (K7).
    /// Whether this one does.
    fn dies_of_it(&mut self, name: &'static str) -> bool {
        let others = WORKERS
            .iter()
            .any(|other| *other != name && self.workers[other].alive);
        let dies =
            self.variant.deaths && self.world.step < ACTIVE && others && self.dice.chance(125);
        if dies {
            self.note(format!("{name} dies before it says what it has done"));
            self.kill(name);
            self.count("workers that died of a merge or a split before saying so");
        }
        dies
    }

    /// The worker drops what its orders no longer name and opens what is new to it.
    fn take_orders(&mut self, name: &'static str, orders: &[Assignment]) {
        for order in orders {
            let Some((_, as_epoch)) = self.workers[name].parts.get(&order.region).copied() else {
                continue;
            };
            self.process(name).parts.remove(&order.region);
            if order.epoch != as_epoch {
                // A new assignment of the part, which the coordinator found in the
                // list: the part in memory is dropped, and the region opened below.
                self.process(name).runs.remove(&order.region);
                self.close(name, order.region, as_epoch);
            }
        }
        for (region, held) in self.workers[name].runs.clone() {
            let ordered = orders
                .iter()
                .any(|order| order.region == region && order.epoch == held.epoch);
            // A part is not dropped for being absent until orders have named it once.
            if !ordered && !self.workers[name].parts.contains_key(&region) {
                self.process(name).runs.remove(&region);
                self.close(name, region, held.epoch);
            }
        }
        for order in orders {
            let (region, epoch) = (order.region, order.epoch);
            if self.workers[name]
                .runs
                .get(&region)
                .is_some_and(|held| held.epoch == epoch)
            {
                self.process(name).runs.insert(region, *order);
                continue;
            }
            if self.workers[name].released.contains(&(region, epoch)) {
                // Orders that still name what it released: it says so again.
                self.say(name, Said::Released(region, epoch));
                continue;
            }
            match self.open(name, region, epoch) {
                Opened::Yes => {
                    self.process(name).runs.insert(region, *order);
                }
                Opened::Refused { seen } => self.say(name, Said::EpochRefused(region, seen)),
                Opened::Absorbed { into } => {
                    self.say(name, Said::AbsorbEnded(into, region, Ok(())));
                }
            }
        }
    }

    /// The worker registers with what it runs. The service reads the list then.
    fn register(&mut self, name: &'static str) {
        let holding: Vec<Assignment> = self.workers[name].runs.values().copied().collect();
        let now = self.now();
        let changes = self
            .coordinator
            .register(now, name, &address(name), &holding);
        let process = self.process(name);
        process.connected = true;
        process.registers = None;
        // A release of one of its regions that still holds is asked for again by this
        // call; one that is not was dropped, as a worker that registers is not leaving.
        self.releasing.retain(|region| {
            self.knows
                .routes
                .get(region)
                .is_none_or(|(owner, _)| owner != name)
        });
        // Its first answer is its orders, whether or not they changed.
        if !changes.workers.iter().any(|changed| changed == name) {
            let orders = self.coordinator.assignments(name);
            self.tell(name, Told::Orders(orders));
        }
        self.after(format!("{name} registers with {holding:?}"), false, changes);
        // A part that no orders have named yet is said again after every registration
        // (section 2.2), and so does a worker say that it is still to stop.
        for (part, (of, as_epoch)) in self.workers[name].parts.clone() {
            self.say(name, Said::SplitEnded(of, as_epoch, Ok(part)));
        }
        if self.workers[name].leaving {
            self.say(name, Said::Leaving);
        }
        self.wanted = true;
    }

    /// The worker's connection is gone, with what was on it, and it registers again.
    fn lose_connection(&mut self, name: &str, registers: u64) {
        let process = self.process(name);
        process.connected = false;
        process.inbox.clear();
        process.registers = Some(registers);
    }

    /// The worker's process ends: the store loses its handles, the regions it ran stand
    /// still, and the service sees its connection end.
    fn kill(&mut self, name: &'static str) {
        let had_connection = self.workers[name].connected;
        *self.process(name) = Process::default();
        for lane in self.lanes.values_mut() {
            if lane.holder == Some(name) {
                lane.holder = None;
            }
        }
        if had_connection {
            self.call(
                format!("{name}'s connection has ended"),
                false,
                |coordinator, now| coordinator.disconnected(now, name),
            );
        }
    }

    /// The coordinator has closed the connection of a worker that said it is leaving:
    /// the worker stops.
    fn exit(&mut self, name: &str) {
        self.note(format!("{name} may go, and does"));
        let Some(name) = WORKERS.iter().copied().find(|worker| *worker == name) else {
            self.fail(format!("{name}, which is no worker of the run, may go"));
        };
        *self.process(name) = Process::default();
        for lane in self.lanes.values_mut() {
            if lane.holder == Some(name) {
                lane.holder = None;
            }
        }
    }

    fn act(&mut self, act: Act) {
        let step = self.world.step;
        if !matches!(act, Act::Walk(..)) {
            self.note(format!("{act:?}"));
        }
        match act {
            Act::Join => {
                let player = self.world.join();
                let site = self.dice.pick(&self.sites).expect("there are sites");
                self.goals.insert(player, site);
            }
            Act::Leave(player) => {
                self.world.leave(player);
                self.goals.remove(&player);
            }
            Act::Walk(player, goal) => self.world.walk_towards(player, goal),
            Act::Hand(player, to, stale) => {
                let from = self.world.players[&player].region;
                self.world.hand(player, to);
                self.old.insert(match stale {
                    Stale::Both => from,
                    Stale::Neither => to,
                });
                self.count("hand-overs");
            }
            Act::Kill(name) => {
                self.kill(name);
                self.count("workers that died");
            }
            Act::Start(name) => {
                let process = self.process(name);
                process.alive = true;
                process.registers = Some(step);
            }
            Act::Stop(name) => {
                self.process(name).leaving = true;
                self.say(name, Said::Leaving);
                self.count("workers told to stop");
            }
            Act::Anew => {
                self.highest += 1_000;
                let regions = self.variant.regions;
                self.coordinator = knowing(self.config.clone(), regions, self.now(), self.highest);
                for name in WORKERS {
                    let registers = step + self.dice.below(3);
                    self.lose_connection(name, registers);
                }
                self.judge.anew();
                self.releasing.clear();
                self.knows = self.known();
                self.wanted = true;
                self.count("coordinators made anew");
            }
        }
    }

    /// A step of the run, in the order section 11 fixes.
    fn step(&mut self, acts: Vec<Act>) {
        self.world.advance();
        let step = self.world.step;
        self.old.clear();
        self.early = true;
        for act in acts {
            self.act(act);
        }

        // The workers say what came of what: each registers if it is to, and does what
        // it was told and is due, which can make more that is due.
        for _ in 0..64 {
            let mut quiet = true;
            for name in WORKERS {
                if !self.workers[name].alive {
                    continue;
                }
                if !self.workers[name].connected
                    && self.workers[name].registers.is_some_and(|at| at <= step)
                {
                    self.register(name);
                    quiet = false;
                }
                while self.workers[name]
                    .inbox
                    .front()
                    .is_some_and(|(due, _)| *due <= step)
                {
                    let (_, told) = self
                        .process(name)
                        .inbox
                        .pop_front()
                        .expect("there is something in it");
                    self.deliver(name, told);
                    quiet = false;
                }
            }
            if quiet {
                break;
            }
        }
        self.early = false;

        // A reading of the list is handed in if one is due.
        for _ in 0..3 {
            if !std::mem::take(&mut self.wanted) {
                break;
            }
            let lost = self.variant.list == Some(Misleads::Lost) && step > ACTIVE + CALM;
            let failed = lost || self.variant.failing && step < ACTIVE && self.dice.chance(200);
            if failed {
                self.count("readings that failed");
                self.call(
                    "the list cannot be read".to_owned(),
                    false,
                    |coordinator, now| coordinator.unlisted(now),
                );
            } else {
                let mut list = self.world.list();
                for info in &mut list.regions {
                    info.epoch = self.lanes[&info.region].epoch;
                    if self.variant.list == Some(Misleads::Unpinned) {
                        info.pinned.clear();
                    }
                }
                self.judge
                    .listed(step, self.world.regions.keys().copied(), &self.knows);
                self.call(
                    format!("the list is {list:?}"),
                    false,
                    |coordinator, now| coordinator.listed(now, &list),
                );
            }
        }

        // The reports, behind the word of every outcome: of every region a worker
        // runs, its true crowds, or those of a step ago.
        let running = self.running();
        for (region, land) in &mut self.world.regions {
            if running.contains(region) {
                land.tick += 1;
            }
        }
        self.world.remember();
        let now = self.now();
        let known = self.knows.clone();
        for name in WORKERS {
            if !self.workers[name].alive || !self.workers[name].connected {
                continue;
            }
            let runs: Vec<(RegionId, u64)> = self.workers[name]
                .runs
                .values()
                .filter(|held| self.holds(name, held.region, held.epoch))
                .map(|held| (held.region, held.epoch))
                .collect();
            let vouched: Vec<(RegionId, Vouch)> = runs
                .iter()
                .map(|(region, _)| (*region, Vouch::Committed))
                .collect();
            if !self.coordinator.heartbeat(now, name, &vouched) {
                self.note(format!("{name} is not known to the coordinator"));
                self.lose_connection(name, step + 1);
                continue;
            }
            let all_old = self.variant.lagging && self.dice.chance(200);
            let mut said = Vec::new();
            let mut given = Vec::new();
            for (region, epoch) in runs {
                let members = self
                    .world
                    .report(region, all_old || self.old.contains(&region));
                let tick = self.world.regions[&region].tick;
                let mut crowds = crowds(&members);
                if self.variant.reversed {
                    crowds.reverse();
                }
                said.push(PlayersOf {
                    region,
                    epoch,
                    tick,
                    crowds,
                });
                given.push((region, epoch, tick, members));
            }
            let heard = self.coordinator.players(now, name, &said);
            self.record.push(number(&format!(
                "{name} says where its players are: {heard}"
            )));
            for (region, epoch, tick, members) in given {
                self.judge
                    .given(step, &known, heard, name, region, epoch, tick, members);
            }
            if !heard {
                self.note(format!("{name} is not known to the coordinator"));
                self.lose_connection(name, step + 1);
            }
        }
        self.judge.look(step, &known);

        // The look.
        self.call("a tick".to_owned(), true, |coordinator, now| {
            coordinator.tick(now)
        });
        if let Some(decider) = self.variant.decider {
            self.decide_in_its_place(decider);
        }
        self.world.check();
    }

    /// The test decides in the place of a coordinator that decides nothing by itself:
    /// it asks by hand, with nobody as asker, for what one that does might begin at
    /// this look, and the judge takes it for begun by itself. Whoever writes these
    /// runs does not read the state machine and cannot put a fault into it; a fault
    /// can be put into this, to show that generated runs catch it.
    ///
    /// It is no build of section 5 and not meant as one. The careful decider asks for
    /// one thing at a time, only at a look that is plain like the five before it, for
    /// what `decide` wanted unchanged at all six, and only when the regions have rested
    /// by the judge's own reckoning. That is less than the record allows and nothing it
    /// forbids, so R1 to R4 hold of it; it absorbs nothing and is in no hurry, so R5
    /// does not.
    fn decide_in_its_place(&mut self, decider: Decider) {
        let step = self.world.step;
        // A coordinator that decides nothing reads the list on events only, so the
        // last reading is often older than two `LIST_EVERY`, and nothing is asked for
        // then.
        let listed = decider == Decider::Unread || self.judge.listed_lately(step);
        if !self.knows.under_way.is_empty() || !listed {
            return;
        }
        let Some(first) = step.checked_sub(model::FRESH + 1) else {
            return;
        };
        let mut looks = Vec::new();
        for at in first..=step {
            match self.judge.look_at(at) {
                Some(look) if look.plain => looks.push(look),
                _ => return,
            }
        }
        let (now, earlier) = looks.split_last().expect("six looks");
        let same = |one: &Wanted, other: &Wanted| match (one, other) {
            (
                Wanted::Split { region, groups, .. },
                Wanted::Split {
                    region: of,
                    groups: those,
                    ..
                },
            ) => region == of && (groups == those || decider == Decider::Greedy),
            (
                Wanted::Merge {
                    survivor, absorbed, ..
                },
                Wanted::Merge {
                    survivor: one,
                    absorbed: other,
                    ..
                },
            ) => (survivor, absorbed) == (one, other) || (survivor, absorbed) == (other, one),
            _ => false,
        };
        let stood = now.wanted.iter().find(|wanted| {
            let regions = match wanted {
                Wanted::Split { region, .. } => vec![*region],
                Wanted::Merge {
                    survivor, absorbed, ..
                } => vec![*survivor, *absorbed],
            };
            let rested = decider == Decider::Restless
                || regions
                    .iter()
                    .all(|region| self.judge.rested(*region, step));
            let stood = decider == Decider::Hasty
                || earlier
                    .iter()
                    .all(|look| look.wanted.iter().any(|before| same(before, wanted)));
            rested && stood
        });
        let Some(stood) = stood.cloned() else {
            return;
        };
        let now = self.now();
        let answer = match &stood {
            Wanted::Merge {
                survivor, absorbed, ..
            } => self.coordinator.merge(now, *survivor, *absorbed, None),
            Wanted::Split { region, groups, .. } => {
                let groups: Vec<&[ChunkPos]> =
                    groups.iter().map(|group| group.as_slice()).collect();
                let chunks = named(&policy(), &groups);
                self.coordinator.split(now, *region, &chunks, None)
            }
        };
        // What is under way after the call and was not before is taken for begun by
        // itself, as after a tick.
        match answer {
            Ok(changes) => self.after(format!("the test asks for {stood:?}"), true, changes),
            Err(refusal) => self.note(format!("the test is refused {stood:?}: {refusal}")),
        }
    }

    /// What happens by chance in the next step: the players walk towards their goals,
    /// some join at the origin and some leave, and what the variant adds while things
    /// may go wrong.
    fn chances(&mut self, moving: bool, faults: bool) -> Vec<Act> {
        let mut acts = Vec::new();
        let running = self.running();
        let players: Vec<(Player, RegionId, ChunkPos)> = self
            .world
            .players
            .iter()
            .filter(|(_, walker)| running.contains(&walker.region))
            .map(|(player, walker)| (*player, walker.region, walker.at))
            .collect();
        if moving {
            for (player, _, at) in &players {
                let goal = *self.goals.entry(*player).or_insert(*at);
                if *at != goal {
                    if self.dice.chance(800) {
                        acts.push(Act::Walk(*player, goal));
                    }
                } else if self.dice.chance(25) {
                    let site = self.dice.pick(&self.sites).expect("there are sites");
                    let near =
                        ChunkPos::new(site.x + self.dice.around(2), site.z + self.dice.around(2));
                    self.goals.insert(*player, near);
                }
            }
            if self.dice.chance(15) && self.world.players.len() < CROWD && running.contains(&HOME) {
                acts.push(Act::Join);
            }
            if self.dice.chance(8)
                && let Some((player, _, _)) = self.dice.pick(&players)
            {
                acts.push(Act::Leave(player));
            }
        }
        if !faults {
            return acts;
        }
        if self.variant.handovers
            && self.dice.chance(25)
            && let Some((player, from, _)) = self.dice.pick(&players)
            && !acts
                .iter()
                .any(|act| matches!(act, Act::Leave(gone) if *gone == player))
        {
            let others: Vec<RegionId> = running
                .iter()
                .filter(|region| **region != from)
                .copied()
                .collect();
            if let Some(to) = self.dice.pick(&others) {
                let stale = if self.dice.chance(500) {
                    Stale::Both
                } else {
                    Stale::Neither
                };
                acts.push(Act::Hand(player, to, stale));
            }
        }
        let alive: Vec<&'static str> = WORKERS
            .iter()
            .copied()
            .filter(|name| self.workers[name].alive)
            .collect();
        let staying: Vec<&'static str> = alive
            .iter()
            .copied()
            .filter(|name| !self.workers[name].leaving)
            .collect();
        let dead: Vec<&'static str> = WORKERS
            .iter()
            .copied()
            .filter(|name| !self.workers[name].alive)
            .collect();
        if self.variant.deaths
            && self.dice.chance(6)
            && alive.len() > 1
            && let Some(name) = self.dice.pick(&alive)
        {
            acts.push(Act::Kill(name));
        } else if self.variant.leavers
            && self.dice.chance(4)
            && staying.len() > 1
            && let Some(name) = self.dice.pick(&staying)
            && self.workers[name].connected
        {
            acts.push(Act::Stop(name));
        } else if (self.variant.deaths || self.variant.leavers)
            && self.dice.chance(20)
            && let Some(name) = self.dice.pick(&dead)
        {
            acts.push(Act::Start(name));
        }
        if self.variant.anew && self.dice.chance(3) {
            acts.push(Act::Anew);
        }
        acts
    }

    /// The run: the players walk while things go wrong, then while nothing does any
    /// more, and then they stand, and R5 says by when everything is to be as the
    /// distances want it and nothing more to be said.
    fn play(mut self) -> Played {
        while self.world.step < ACTIVE {
            let acts = self.chances(true, true);
            self.step(acts);
        }
        // From here on nothing goes wrong: every worker's process is there, and those
        // that were told to stop go when they own nothing.
        self.note("from here on nothing goes wrong".to_owned());
        let dead: Vec<Act> = WORKERS
            .iter()
            .copied()
            .filter(|name| !self.workers[name].alive)
            .map(Act::Start)
            .collect();
        self.step(dead);
        while self.world.step < ACTIVE + CALM {
            let acts = self.chances(true, false);
            self.step(acts);
        }
        self.note("from here on the players stand".to_owned());
        self.judge.stand_still(self.world.step);
        // A lease, the longest delay and two steps later, everything that was under
        // way has ended.
        for _ in 0..steps(LEASE) + DELAY + 2 {
            self.step(Vec::new());
        }
        let runs: Vec<u32> = WORKERS
            .iter()
            .filter(|name| self.workers[**name].alive && self.workers[**name].connected)
            .map(|name| self.workers[*name].runs.len() as u32)
            .collect();
        let alone = self
            .knows
            .routes
            .keys()
            .chain(&self.knows.waiting)
            .filter_map(|region| self.alone_until(*region))
            .max();
        self.judge.settle(&self.world, &runs, alone);
        self.note(format!("what is left to do: {:?}", self.judge.settling));
        loop {
            let deadline = self
                .judge
                .settling
                .as_ref()
                .expect("the run has settled")
                .deadline();
            if self.world.step >= deadline {
                break;
            }
            self.step(Vec::new());
        }
        self.judge
            .end(&self.world, &self.knows, !self.releasing.is_empty());
        // How much of the time R5 allows was used: from `s'` to the last call that said
        // anything but to read the list, against from `s'` to the deadline.
        let settling = self.judge.settling.clone().expect("the run has settled");
        self.spent = Some((
            self.last_word.max(settling.at) - settling.at,
            settling.deadline() - settling.at,
        ));
        self.hushed = true;
        for _ in 0..HUSH {
            self.step(Vec::new());
        }
        // (A coordinator that cannot get the list read does not learn what became of a
        // merge, and need not agree with the model.)
        if self.variant.list != Some(Misleads::Lost) {
            self.in_order();
        }
        self.finish()
    }

    /// The first step at or after the time before which the coordinator begins nothing
    /// with the region by itself, if it has noted such a time.
    fn alone_until(&self, region: RegionId) -> Option<u64> {
        let until = self.coordinator.alone_until(region)?;
        let since = until.saturating_duration_since(self.started);
        let steps = since.as_millis().div_ceil(LOOK.as_millis());
        Some(u64::try_from(steps).expect("a time of a run"))
    }

    /// Steps in which the players stand.
    fn pass(&mut self, steps: u64) {
        for _ in 0..steps {
            self.step(Vec::new());
        }
    }

    /// The player walks to a chunk, a chunk in two seconds.
    fn walk(&mut self, player: Player, goal: ChunkPos) {
        while self.world.players[&player].at != goal {
            self.step(vec![Act::Walk(player, goal)]);
        }
    }

    /// Steps in which the players stand, until something holds of the cluster, and
    /// whether it came to hold in that many steps.
    fn until(&mut self, steps: u64, holds: impl Fn(&Self) -> bool) -> bool {
        for _ in 0..steps {
            if holds(self) {
                return true;
            }
            self.step(Vec::new());
        }
        holds(self)
    }

    /// What came of the run.
    fn finish(mut self) -> Played {
        Played {
            seed: self.seed,
            name: self.variant.name,
            steps: self.world.step,
            breaches: std::mem::take(&mut self.judge.breaches),
            seen: self
                .judge
                .seen
                .iter()
                .chain(&self.seen)
                .map(|(what, times)| (*what, *times))
                .collect(),
            record: std::mem::take(&mut self.record),
            led: self.led.take(),
            spent: self.spent,
            story: self.story.into_iter().collect(),
        }
    }

    /// When nothing has gone wrong for a long time, the model and the coordinator agree
    /// whatever the coordinator decides: it has exactly the regions that live, each with
    /// one owner, which runs it with the epoch the routing table names. Otherwise the
    /// way the run is played is wrong, and what the properties say of it means nothing.
    fn in_order(&self) {
        let living: Vec<RegionId> = self.world.regions.keys().copied().collect();
        let routed: Vec<RegionId> = self.knows.routes.keys().copied().collect();
        if living != routed || !self.knows.waiting.is_empty() {
            self.fail(format!(
                "at the end the coordinator has {routed:?} with {:?} waiting, and {living:?} live",
                self.knows.waiting
            ));
        }
        for (region, (name, epoch)) in &self.knows.routes {
            if !self.holds(name, *region, *epoch) {
                self.fail(format!(
                    "at the end region {region} is {name}'s with {epoch}, which runs {:?}; the store \
                     has {:?}",
                    self.workers[name.as_str()].runs,
                    self.lanes[region]
                ));
            }
        }
    }
}

/// A number for a line of a run's record (FNV-1a).
fn number(line: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in line.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// What came of a run.
struct Played {
    seed: u64,
    name: &'static str,
    steps: u64,
    breaches: Vec<Breach>,
    seen: BTreeMap<&'static str, u32>,
    record: Vec<u64>,
    /// What had happened last when a property was first broken, and at the end.
    led: Option<Vec<String>>,
    /// How many steps the coordinator went on saying things after `s'`, and how many
    /// R5 allowed it, if the run came that far.
    spent: Option<(u64, u64)>,
    story: Vec<String>,
}

impl Played {
    /// How often something happened in the run.
    fn seen(&self, what: &str) -> u32 {
        self.seen.get(what).copied().unwrap_or(0)
    }

    /// Fails if one of these properties was broken, or any if none is named.
    fn assert_holds(&self, properties: &[Property]) {
        if let Some(wrong) = self.wrong(properties) {
            panic!("{wrong}");
        }
    }

    /// What is wrong with the run, if one of these properties was broken, or any if
    /// none is named: the seed, the breaches, and what led to the first there was.
    fn wrong(&self, properties: &[Property]) -> Option<String> {
        let broken: Vec<&Breach> = self
            .breaches
            .iter()
            .filter(|breach| properties.is_empty() || properties.contains(&breach.property))
            .collect();
        let first = broken.first()?;
        let lines: Vec<String> = broken
            .iter()
            .take(12)
            .map(|breach| {
                format!(
                    "{:?} at step {}: {}",
                    breach.property, breach.step, breach.what
                )
            })
            .collect();
        Some(format!(
            "seed {seed}, {name}: {} breaches, the first of {:?} at step {}\n\n{}\n\nwhat led \
             to the first breach of the run:\n{}\n\nseed {seed}, {name} \
             (CLUSTINE_FOLLOWS_SEED={seed}): {:?} at step {}: {}",
            broken.len(),
            first.property,
            first.step,
            lines.join("\n"),
            self.led.as_ref().unwrap_or(&self.story).join("\n"),
            first.property,
            first.step,
            first.what,
            seed = self.seed,
            name = self.name,
        ))
    }
}

/// The seeds of the generated runs: eight, or as many as `CLUSTINE_FOLLOWS_RUNS` says,
/// or the one that `CLUSTINE_FOLLOWS_SEED` names.
fn seeds() -> Vec<u64> {
    let number = |name: &str| {
        std::env::var(name).ok().map(|value| {
            value
                .parse::<u64>()
                .unwrap_or_else(|_| panic!("{name} is to be a number, and is {value:?}"))
        })
    };
    match (
        number("CLUSTINE_FOLLOWS_SEED"),
        number("CLUSTINE_FOLLOWS_RUNS"),
    ) {
        (Some(seed), _) => vec![seed],
        (None, runs) => (1..=runs.unwrap_or(8)).collect(),
    }
}

/// Plays a run of this kind for each seed, says what the runs were about (to be read
/// with `--nocapture`), and holds them to R1 to R5.
fn play(variant: Variant) {
    let seeds = seeds();
    let mut seen: BTreeMap<&'static str, u32> = BTreeMap::new();
    let mut steps = 0;
    // Every seed is played, so that a failure says how many runs it is of.
    let mut failed: Vec<(u64, Vec<Property>)> = Vec::new();
    let mut first = None;
    // The steps used and allowed after `s'` of the run that used the largest share.
    let mut tightest: (u64, u64, u64) = (0, 1, 0);
    for seed in &seeds {
        let played = Cluster::new(*seed, variant).play();
        if let Some(wrong) = played.wrong(&[]) {
            let mut broken: Vec<Property> = played
                .breaches
                .iter()
                .map(|breach| breach.property)
                .collect();
            broken.sort_unstable();
            broken.dedup();
            failed.push((*seed, broken));
            first.get_or_insert(wrong);
        }
        if let Some((used, allowed)) = played.spent
            && used * tightest.1 > tightest.0 * allowed
        {
            tightest = (used, allowed, *seed);
        }
        for (what, times) in played.seen {
            *seen.entry(what).or_default() += times;
        }
        steps += played.steps;
    }
    println!("{} runs, {steps} steps, {}:", seeds.len(), variant.name);
    for (what, times) in &seen {
        println!("    {times:>7} {what}");
    }
    println!(
        "    R5: the run that came nearest its deadline (seed {}) was quiet {} steps after \
         s', of {} allowed",
        tightest.2, tightest.0, tightest.1
    );
    if let Some(first) = first {
        panic!(
            "{first}\n\n{} of {} runs break a property, by seed: {failed:?}",
            failed.len(),
            seeds.len()
        );
    }
    // A run of a kind is about what the kind is for.
    if seeds.len() >= 8 {
        for what in [
            "merges by the distances made",
            "splits made",
            "merges by the distances that R4 (e) checked",
            "splits that R4 (e) checked",
        ] {
            assert!(seen.contains_key(what), "no {what} in {seen:?}");
        }
    }
}

#[test]
fn runs_in_which_nothing_goes_wrong_hold_r1_to_r5() {
    play(Variant::QUIET);
}

#[test]
fn runs_of_a_world_that_begins_as_one_home_region_hold_r1_to_r5() {
    play(Variant::ONE_HOME);
}

#[test]
fn runs_with_players_handed_over_on_one_stale_report_hold_r1_to_r5() {
    play(Variant::HANDED);
}

#[test]
fn runs_of_a_world_of_pinned_regions_hold_r1_to_r5() {
    play(Variant::PINNED);
}

#[test]
fn runs_with_workers_that_die_or_leave_and_readings_that_fail_hold_r1_to_r5() {
    play(Variant::DEATHS);
}

#[test]
fn runs_with_a_coordinator_made_anew_hold_r1_to_r5() {
    play(Variant::ANEW);
}

/// The cluster and the properties, tried on merges and splits that are not the
/// coordinator's own choice. The test asks for them by hand, as
/// `Cluster::decide_in_its_place` says, the workers of the model carry them out through
/// the coordinator's paths for a merge and a split, and the judge takes them for begun
/// by itself. R1 to R4 hold of what is asked for like that, whatever else goes wrong
/// in the run: so a breach in the runs below is of the decider's fault and not of how
/// the runs are played.
#[test]
fn merges_and_splits_asked_for_in_the_coordinators_place_hold_r1_to_r4() {
    let mut seen: BTreeMap<&'static str, u32> = BTreeMap::new();
    for variant in [
        Variant::QUIET,
        Variant::HANDED,
        Variant::DEATHS,
        Variant::ANEW,
    ] {
        for seed in seeds() {
            let in_its_place = Variant {
                follow: false,
                decider: Some(Decider::Careful),
                ..variant
            };
            let played = Cluster::new(seed, in_its_place).play();
            played.assert_holds(&[
                Property::R1,
                Property::R2,
                Property::R3,
                Property::R4a,
                Property::R4b,
                Property::R4c,
                Property::R4d,
                Property::R4e,
                Property::Call,
            ]);
            for (what, times) in played.seen {
                *seen.entry(what).or_default() += times;
            }
        }
    }
    println!("asked for in the coordinator's place:");
    for (what, times) in &seen {
        println!("    {times:>7} {what}");
    }
    if seeds().len() >= 8 {
        for what in [
            "merges by the distances made",
            "splits made",
            "merges by the distances that R4 (e) checked",
            "splits that R4 (e) checked",
        ] {
            assert!(seen.contains_key(what), "no {what} in {seen:?}");
        }
    }
}

/// Three of the six faults of step C4.6, put into the decider that asks in the
/// coordinator's place, are caught in generated runs by the property that the record's
/// table names for each. A fourth is said and not asserted. The other two, and these
/// as well, are caught in the scripted runs of `follows/stage.rs`.
#[test]
fn a_decider_without_rest_without_standing_or_without_the_list_is_caught_in_generated_runs() {
    let caught = |decider: Decider, variant: Variant| {
        let mut broken: BTreeMap<Property, u32> = BTreeMap::new();
        for seed in seeds() {
            let faulty = Variant {
                follow: false,
                decider: Some(decider),
                ..variant
            };
            for breach in Cluster::new(seed, faulty).play().breaches {
                *broken.entry(breach.property).or_default() += 1;
            }
        }
        broken.remove(&Property::R5);
        println!("{decider:?}, {}: {broken:?}", variant.name);
        broken
    };
    let enough = seeds().len() >= 8;
    // No rest: R1, and R2 by its count, in any run in which a region is in two things.
    let broken = caught(Decider::Restless, Variant::QUIET);
    assert!(!enough || broken.contains_key(&Property::R1), "{broken:?}");
    assert!(!enough || broken.contains_key(&Property::R2), "{broken:?}");
    assert!(
        broken
            .keys()
            .all(|property| [Property::R1, Property::R2].contains(property))
    );
    // No standing: R4 (e), on the stale hand-over of K10 at plain looks.
    let broken = caught(Decider::Hasty, Variant::HANDED);
    assert!(!enough || broken.contains_key(&Property::R4e), "{broken:?}");
    assert!(!broken.contains_key(&Property::R4a), "{broken:?}");
    // The list not read: R3's clause on the age of the last reading, in a run that is
    // quiet for more than two `LIST_EVERY` before something is wanted.
    let broken = caught(Decider::Unread, Variant::QUIET);
    assert!(!enough || broken.contains_key(&Property::R3), "{broken:?}");
    assert!(broken.keys().all(|property| *property == Property::R3));
    // A group that goes on its first look: R4 (e), where a run has it. It takes a
    // region with a split that has stood and a group that is new at the look at which
    // the split is begun (K22), which players who walk at random bring up in one run
    // of many; the scripted run of K22 is what catches this fault, and how often these
    // runs do is only said.
    caught(Decider::Greedy, Variant::HANDED);
}

/// The state machine itself is caught where it does other than the judge holds it to.
/// Nobody can put a fault into it from here, but it can be misled, so that what it
/// rightly does by what it was told is to the judge one of the faults of step C4.6:
///
/// - told half the rest, it begins with a region again after five seconds ("no rest");
/// - told a merge distance one chunk longer, it merges regions whose players were
///   never near enough, and that have not stood by the judge's distances;
/// - handed a list that has no region pinned, it absorbs pinned regions for being
///   empty;
/// - with no reading of the list that succeeds once the players stand, it begins
///   nothing, and the end is not reached ("the list never read").
#[test]
fn a_coordinator_that_is_misled_is_caught_in_generated_runs() {
    let caught = |misled: Variant| {
        let mut broken: BTreeMap<Property, u32> = BTreeMap::new();
        for seed in seeds() {
            for breach in Cluster::new(seed, misled).play().breaches {
                *broken.entry(breach.property).or_default() += 1;
            }
        }
        println!(
            "a coordinator told {:?} with a list that is {:?}: {broken:?}",
            misled.told, misled.list
        );
        broken
    };
    let enough = seeds().len() >= 8;
    let broken = caught(Variant {
        told: Some(Policy {
            rest: policy().rest / 2,
            ..policy()
        }),
        ..Variant::QUIET
    });
    assert!(!enough || broken.contains_key(&Property::R1), "{broken:?}");
    assert!(!enough || broken.contains_key(&Property::R2), "{broken:?}");
    let broken = caught(Variant {
        told: Some(Policy {
            merge_distance: policy().merge_distance + 1,
            ..policy()
        }),
        ..Variant::QUIET
    });
    assert!(!enough || broken.contains_key(&Property::R4a), "{broken:?}");
    assert!(!enough || broken.contains_key(&Property::R4e), "{broken:?}");
    let broken = caught(Variant {
        list: Some(Misleads::Unpinned),
        ..Variant::PINNED
    });
    assert!(!enough || broken.contains_key(&Property::R3), "{broken:?}");
    assert!(!enough || broken.contains_key(&Property::R4b), "{broken:?}");
    let broken = caught(Variant {
        list: Some(Misleads::Lost),
        ..Variant::QUIET
    });
    assert!(!enough || broken.contains_key(&Property::R5), "{broken:?}");
    assert!(broken.keys().all(|property| *property == Property::R5));
}

/// R6: the same calls give the same answers, and crowds given in another order do. A
/// run is played twice from its seed, and once more with the crowds of every report
/// in descending order; every call of the three has the same answer.
#[test]
fn the_same_calls_give_the_same_answers_and_crowds_given_in_another_order_do() {
    // That the comparison can fail: other calls are answered otherwise.
    let one = Cluster::new(1, Variant::QUIET).play();
    let other = Cluster::new(2, Variant::QUIET).play();
    assert_ne!(one.record, other.record);

    for variant in [Variant::QUIET, Variant::ANEW] {
        for seed in seeds().into_iter().take(3) {
            let first = Cluster::new(seed, variant).play();
            let again = Cluster::new(seed, variant).play();
            let reversed = Cluster::new(
                seed,
                Variant {
                    reversed: true,
                    ..variant
                },
            )
            .play();
            for (other, how) in [
                (&again, "played again"),
                (&reversed, "with the crowds reversed"),
            ] {
                let differs = first
                    .record
                    .iter()
                    .zip(&other.record)
                    .position(|(one, other)| one != other);
                assert!(
                    differs.is_none() && first.record.len() == other.record.len(),
                    "seed {seed}, {} (CLUSTINE_FOLLOWS_SEED={seed}): {how}, the run has {} \
                     calls where it had {}, and the first that is answered otherwise is \
                     number {differs:?}",
                    variant.name,
                    other.record.len(),
                    first.record.len(),
                );
            }
        }
    }
}

/// R7: with `follow: None` the same runs begin nothing.
#[test]
fn a_coordinator_that_decides_nothing_by_itself_begins_nothing_in_the_same_runs() {
    for variant in Variant::ALL {
        for seed in seeds() {
            let by_hand = Variant {
                follow: false,
                ..variant
            };
            let played = Cluster::new(seed, by_hand).play();
            played.assert_holds(&[Property::R7, Property::Call]);
            for what in [
                "merges the workers made",
                "splits the workers made",
                "splits that found nobody or no runner",
            ] {
                assert!(
                    !played.seen.contains_key(what),
                    "seed {seed}, {}: {what}, where nobody asks for any",
                    variant.name
                );
            }
        }
    }
}

/// The steps of the runs are the times of the record (section 3).
#[test]
fn the_times_of_the_runs_are_those_of_the_record() {
    let policy = policy();
    assert_eq!(LOOK, Coordinator::LOOK);
    assert_eq!(steps(policy.rest), model::REST);
    assert_eq!(steps(std::time::Duration::from_secs(1)), model::FRESH);
    assert_eq!(model::EMPTY_FOR, 3 * steps(policy.rest));
    assert_eq!(steps(LEASE), model::LIST_EVERY);
    assert_eq!((policy.merge_distance, policy.split_distance), (6, 12));
    assert_eq!(policy.margin(), 3);
    assert_eq!(policy.checked(), Ok(policy));
    // The lease is longer than the longest delay, and the calm before the players
    // stand is eight leases and more.
    assert!(steps(LEASE) > DELAY);
    assert!(CALM >= 8 * steps(LEASE));
}

// ---------------------------------------------------------------------------------
// Runs that a script plays against the coordinator: what the distances 6 and 12 and
// players who walk at random seldom bring up (K2 (b), K12), and the two runs in which
// R4 (e) has to have been able to check what was begun (K10, K22).
// ---------------------------------------------------------------------------------

/// How long a script waits at its start: for the grace period of a new coordinator, for
/// the workers to open what they are given, and for the rest of regions that were given
/// their owners.
fn settled() -> u64 {
    steps(LEASE) + DELAY + 2 + model::REST
}

/// K4, across the whole band and back: a merge when the two have been within the merge
/// distance for a second, a split when they have been further apart than the split
/// distance for a second, and a merge again, each at least a rest after the one before.
#[test]
fn a_player_who_walks_across_the_band_and_back_is_merged_split_and_merged_again() {
    let variant = Variant {
        name: "K4, across the band and back",
        regions: 2,
        ..Variant::QUIET
    };
    let mut cluster = Cluster::with(variant, &[(0, 0, 0), (1, 8, 0)]);
    cluster.pass(settled());
    let of = |cluster: &Cluster| cluster.world.players[&1].region;
    cluster.walk(1, ChunkPos::new(6, 0));
    let merged = cluster.until(3 * model::REST, |cluster| of(cluster) == HOME);
    cluster.walk(1, ChunkPos::new(13, 0));
    let split = cluster.until(3 * model::REST, |cluster| of(cluster) != HOME);
    // The part is on the worker that made it, which has two regions now and the others
    // none: it is moved when it has rested, and merged when it has rested again (K3).
    cluster.walk(1, ChunkPos::new(6, 0));
    let again = cluster.until(5 * model::REST, |cluster| of(cluster) == HOME);
    cluster.pass(model::REST);
    cluster.in_order();
    let played = cluster.finish();
    assert!(
        merged && split && again,
        "merged: {merged}, split: {split}, merged again: {again}\n{}",
        played.story.join("\n")
    );
    played.assert_holds(&[]);
    assert_eq!(played.seen("merges by the distances begun"), 2);
    assert_eq!(played.seen("splits begun"), 1);
    // Each began at plain looks, and R4 (e) has checked that it had stood.
    assert_eq!(
        played.seen("merges by the distances that R4 (e) checked"),
        2
    );
    assert_eq!(played.seen("splits that R4 (e) checked"), 1);
}

/// K10, in both: player 1 of region 1 is handed to region 2, and for one report is in
/// both sightings. A merge is wanted at that look and at no other, and none is begun.
/// When the player walks up to the region they left, the merge is wanted for good.
#[test]
fn a_player_who_is_in_two_reports_for_one_look_merges_nothing() {
    let variant = Variant {
        name: "K10, in both reports",
        ..Variant::QUIET
    };
    let mut cluster = Cluster::with(variant, &[(1, 30, 0), (1, 40, 0), (2, 48, 0)]);
    cluster.pass(settled());
    cluster.step(vec![Act::Hand(1, RegionId(2), Stale::Both)]);
    cluster.pass(model::REST);
    let on_one_look: u32 = [
        "merges by the distances begun",
        "absorptions begun",
        "splits begun",
    ]
    .iter()
    .map(|what| cluster.judge.seen.get(what).copied().unwrap_or(0))
    .sum();
    cluster.walk(1, ChunkPos::new(36, 0));
    let merged = cluster.until(3 * model::REST, |cluster| cluster.world.regions.len() == 2);
    cluster.pass(model::REST);
    cluster.in_order();
    let played = cluster.finish();
    assert_eq!(
        on_one_look,
        0,
        "something was begun on the one stale report\n{}",
        played.story.join("\n")
    );
    assert!(merged, "no merge\n{}", played.story.join("\n"));
    played.assert_holds(&[]);
    // The look of the stale report and those before it were plain, so that a merge
    // begun at it would have been checked; the merge that was begun later was.
    assert_eq!(played.seen("merges by the distances begun"), 1);
    assert_eq!(
        played.seen("merges by the distances that R4 (e) checked"),
        1
    );
}

/// K22. The home region has a player at the origin, a group far east that is to go
/// from the first report on (player 1), players far north (player 2) and a player
/// between those and the origin (player 3). At the look at which the home region's
/// rest ends, player 3 has just been handed to region 1 and is in neither report: the
/// players in the north are a group to go, at that look for the first time. The split
/// takes the group in the east, and they stay.
#[test]
fn a_group_that_appears_as_its_region_becomes_free_stays_when_the_one_that_has_stood_goes() {
    let variant = Variant {
        name: "K22",
        regions: 2,
        ..Variant::QUIET
    };
    let mut cluster = Cluster::with(
        variant,
        &[(0, 0, 0), (0, 20, 0), (0, 0, -20), (0, 0, -10), (1, -40, 0)],
    );
    // The home region rests from when it is given its owner, at the end of the grace
    // period.
    cluster.pass(steps(LEASE) + DELAY + 2);
    let free = cluster
        .alone_until(HOME)
        .unwrap_or(cluster.world.step + model::REST);
    let early_enough = cluster.world.step + model::FRESH + 2 < free;
    while cluster.world.step + 1 < free {
        cluster.pass(1);
    }
    cluster.step(vec![Act::Hand(3, RegionId(1), Stale::Neither)]);
    // A group that has stood goes at this look, or a look or two later if a reading of
    // the list is asked for just now; its worker has done it a second later at most.
    let split = cluster.until(DELAY + 3, |cluster| cluster.world.regions.len() == 3);
    let (east, north) = (
        cluster.world.players[&1].region,
        cluster.world.players[&2].region,
    );
    cluster.pass(2);
    let played = cluster.finish();
    assert!(early_enough, "the home region's rest ends at step {free}");
    assert!(
        split,
        "no split when the home region's rest ended at step {free}\n{}",
        played.story.join("\n")
    );
    assert_ne!(east, HOME, "the group that has stood goes");
    assert_eq!(north, HOME, "the group of this look stays");
    played.assert_holds(&[]);
    assert_eq!(played.seen("splits begun"), 1);
    assert_eq!(played.seen("splits that R4 (e) checked"), 1);
}

/// The players of K2 (b) with the distances 6 and 12: two sets of players of region 1,
/// 22 apart, and two players of region 2 between them, each within 6 of one set and 10
/// from each other.
const JOINED: [(u32, i32, i32); 4] = [(1, 100, 0), (1, 122, 0), (2, 106, 0), (2, 116, 0)];

/// K2 (b): everything is one cluster, region 1 is whole, and the two merge. One merge;
/// splitting first would have been three.
#[test]
fn a_region_whose_groups_players_of_another_region_join_is_merged_with_it_and_not_split() {
    let variant = Variant {
        name: "K2 (b)",
        ..Variant::QUIET
    };
    let mut cluster = Cluster::with(variant, &JOINED);
    cluster.pass(settled());
    let merged = cluster.until(3 * model::REST, |cluster| cluster.world.regions.len() == 2);
    cluster.pass(2 * model::REST);
    cluster.in_order();
    let played = cluster.finish();
    assert!(merged, "no merge\n{}", played.story.join("\n"));
    played.assert_holds(&[]);
    assert_eq!(played.seen("splits begun"), 0);
    assert_eq!(played.seen("merges by the distances begun"), 1);
}

/// K12: the worker of the region in between dies when its region has been reported.
/// The sighting stays and goes on joining the groups of the region around it, which is
/// neither split nor merged; when the region in between has a new owner, has been
/// reported and has rested, the two merge.
#[test]
fn a_region_is_not_split_while_the_region_that_joins_its_groups_is_silent() {
    let variant = Variant {
        name: "K12",
        ..Variant::QUIET
    };
    let mut cluster = Cluster::with(variant, &JOINED);
    cluster.pass(steps(LEASE) + 2 * DELAY + 2);
    let owner = cluster
        .knows
        .routes
        .get(&RegionId(2))
        .map(|(name, _)| name.clone());
    let owner = WORKERS
        .iter()
        .copied()
        .find(|name| Some(*name) == owner.as_deref())
        .expect("the region in between has an owner by now");
    cluster.step(vec![Act::Kill(owner)]);
    let merged = cluster.until(steps(LEASE) + 6 * model::REST, |cluster| {
        cluster.world.regions.len() == 2
    });
    cluster.pass(model::REST);
    cluster.in_order();
    let played = cluster.finish();
    assert!(merged, "no merge\n{}", played.story.join("\n"));
    played.assert_holds(&[]);
    assert_eq!(played.seen("splits begun"), 0);
    assert_eq!(played.seen("merges by the distances begun"), 1);
}
