//! The coordinator deciding by itself when regions merge and split, tested from the
//! record alone: `docs/adr/0016-when-to-merge-and-split.md`, the scenarios F1 to F50 of
//! its section 11 and the orders of events K1 to K26 of its section 9 as far as the
//! state machine alone decides them. Whoever wrote these read the record, the pure rule
//! (`policy.rs`), the messages and the coordinator's public signatures with their
//! comments, and neither the state machine's code nor its own tests, so that a test
//! here says what the record asks for and not what the code happens to do.
//!
//! Every test drives [`Coordinator`] with the time handed in; none waits for time to
//! pass. A [`World`] is a coordinator, the workers that go on saying that they are
//! there, where the players truly are, and the world store's list. **The workers'
//! reports do not fall on the instants of the coordinator's ticks**: a step is a
//! report of every region [`LAG`] after the tick before, and the tick a [`LOOK`] after
//! that one, so that the time of a report and the time of a tick are never the same
//! instant unless a test says so. A rule that compares the two can otherwise be built
//! or left out without a test noticing (section 11, F36).
//!
//! Each test is marked with the scenario it is: `// F12.`, `// K4.`, or the section
//! whose sentence it tests. Where the record gives more things than numbers under a
//! range, they are counted like this: F7 is standing and standing anew, F8 the three
//! regions in a row, F9 the place a merge keeps and loses, F10 a pair of which one
//! was absorbed and the ten pairs; F30 to F35 are the first six things of "empty
//! regions", F36 is the one the record numbers itself, F37 the second absorption
//! into one survivor, and F38 what follows an absorption that ended well and one
//! that came to nothing. Many tests are followed by "the other half": the same
//! sequence without the one thing the test is about, in which what was held back is
//! begun, so that the test shows what it says it does.
//!
//! A test that is ignored as a finding is one that fails: the coordinator does
//! something else there than the record says. Its comment has the sequence, what was
//! to happen and what does.
//!
//! The worlds are stripes: region 0 is the westernmost and the home region unless a
//! test says otherwise, the chunk players enter in is the origin, and a worker named
//! `a` is reached at `a:25600`, so a route says whose a region is. The distances are 2
//! and 5 and the margin 2, the rest ten seconds and the lease five, unless a test
//! needs others. The coordinator goes by nothing but the reports for where players
//! are, so the tests put them wherever a scenario needs them, whatever the stripes.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use clustine_coordinator::{
    Asked, Changes, Coordinator, CoordinatorConfig, Order, Policy, ReleaseOrder, ReshapeOrder,
    Reshaped, Undone, named,
};
use clustine_region::{RegionId, RoutingTable};
use clustine_rpc::{Assignment, Decline, Off, PlayersOf, RegionInfo, RegionList, Vouch};
use clustine_world::{ChunkArea, ChunkPos, EntityIds, Vec3};

/// The lease of the coordinators of these tests, and so how often the list is read
/// (`LIST_EVERY`).
const LEASE: Duration = Duration::from_secs(5);

/// How often a coordinator that decides by itself is to be ticked.
const LOOK: Duration = Coordinator::LOOK;

/// How old a sighting may be, and longer than which a thing has to be wanted.
const FRESH: Duration = Duration::from_secs(1);

/// How long a region is left alone after a merge, a split or a change of owner.
const REST: Duration = Duration::from_secs(10);

/// How long a region is without players before it is absorbed: three rests.
const EMPTY_FOR: Duration = Duration::from_secs(30);

/// How long a region is left alone after an attempt that failed: three rests.
const LONG: Duration = Duration::from_secs(30);

/// How many merges and splits are under way at one time.
const AT_ONCE: usize = 4;

/// The shortest time that a test tells apart.
const MOMENT: Duration = Duration::from_millis(1);

/// How long after a tick the workers' reports are taken in a step: a worker's looks do
/// not fall on the coordinator's.
const LAG: Duration = Duration::from_millis(100);

/// Every epoch a coordinator of these tests issues is above this.
const FIRST_EPOCH: u64 = 1_000;

/// Who asks for the merges and splits that a test asks for by hand.
const ASKER: Option<u64> = Some(41);

fn region(id: u32) -> RegionId {
    RegionId(id)
}

/// A coordinator that knows `regions` stripes from the start, numbered from 0, as
/// every coordinator did before it learnt its regions from the world store's list
/// (`docs/adr/0017-the-end-of-the-stripes.md`, section 2.3). These tests are about
/// what a coordinator does with regions it knows.
fn knowing(config: CoordinatorConfig, regions: u32, now: Instant, first_epoch: u64) -> Coordinator {
    let stripes: Vec<RegionId> = (0..regions).map(RegionId).collect();
    Coordinator::knowing(config, now, first_epoch, &stripes)
}

/// The chunk `x` of the row z = 0, in which most of these tests' players stand.
fn at(x: i32) -> ChunkPos {
    ChunkPos::new(x, 0)
}

/// The distances 2 and 5, so the margin is 2, and a rest of ten seconds.
fn follow() -> Option<Policy> {
    Some(
        Policy {
            merge_distance: 2,
            split_distance: 5,
            rest: REST,
        }
        .checked()
        .expect("the distances fit each other"),
    )
}

/// The chunks a split names for groups that stand in the chunks given, with the
/// distances of [`follow`].
fn around(groups: &[&[ChunkPos]]) -> Vec<ChunkPos> {
    named(&follow().expect("a policy"), groups)
}

fn address(name: &str) -> String {
    format!("{name}:25600")
}

/// What a worker reports of a region it runs already.
fn held(id: u32, epoch: u64) -> Assignment {
    Assignment {
        region: region(id),
        epoch,
        entity_ids: EntityIds::block(id + 1).expect("there are that many blocks"),
    }
}

fn add(total: &mut Changes, more: Changes) {
    total.workers.extend(more.workers);
    total.routing |= more.routing;
    total.releases.extend(more.releases);
    total.moves.extend(more.moves);
    total.gone.extend(more.gone);
    total.orders.extend(more.orders);
    total.reshaped.extend(more.reshaped);
    total.read |= more.read;
}

/// Something the coordinator began by itself at a tick.
#[derive(Clone, PartialEq, Eq)]
enum Begun {
    /// A merge, by the distances or an absorption: the owner of `absorbed` was told to
    /// release it and the survivor's to prepare, as for a merge asked for by hand.
    Merge { survivor: u32, absorbed: u32 },
    /// A split: the region's owner was told to split the players in `chunks` off as
    /// the region `part`.
    Split {
        region: u32,
        part: u32,
        chunks: Vec<ChunkPos>,
    },
    /// The region's owner was told to prepare, and no merge was begun that the region
    /// survives: it is said before a split.
    Prepare(u32),
    /// The region's owner was told to release it, and not for a merge: it is moved to
    /// even regions out, or because its worker leaves.
    Move(u32),
    /// Anything else a tick told a worker.
    Other(String),
}

/// In as few words as say it: a split names dozens of chunks, and a test that fails
/// prints everything that was begun.
impl std::fmt::Debug for Begun {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Merge { survivor, absorbed } => {
                write!(formatter, "Merge({absorbed} into {survivor})")
            }
            Self::Split {
                region,
                part,
                chunks,
            } => {
                write!(
                    formatter,
                    "Split({region}, part {part}, {} chunks, around",
                    chunks.len()
                )?;
                // The chunks that are named with all of their neighbours say which
                // groups were named, give or take.
                for chunk in chunks {
                    let inside = (-1..=1).all(|x| {
                        (-1..=1).all(|z| chunks.contains(&ChunkPos::new(chunk.x + x, chunk.z + z)))
                    });
                    if inside {
                        write!(formatter, " {},{}", chunk.x, chunk.z)?;
                    }
                }
                formatter.write_str(")")
            }
            Self::Prepare(region) => write!(formatter, "Prepare({region})"),
            Self::Move(region) => write!(formatter, "Move({region})"),
            Self::Other(order) => write!(formatter, "Other({order})"),
        }
    }
}

fn merge(survivor: u32, absorbed: u32) -> Begun {
    Begun::Merge { survivor, absorbed }
}

fn split(of: u32, part: u32, groups: &[&[ChunkPos]]) -> Begun {
    Begun::Split {
        region: of,
        part,
        chunks: around(groups),
    }
}

/// What a tick did.
#[derive(Debug, Clone)]
struct Look {
    /// What the coordinator began at it.
    begun: Vec<Begun>,
    /// What the tick changed, and what the reading changed that it asked for, if that
    /// was answered at once.
    changes: Changes,
}

/// How the readings of the list that the coordinator asks for are answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Readings {
    /// With the list, in the call that asks.
    AtOnce,
    /// With a failure, in the call that asks.
    Failing,
    /// Not until the test answers.
    Held,
}

/// A coordinator, the time, the workers that go on saying that they are there, where
/// the players truly are, and the world store's list.
#[derive(Debug, Clone)]
struct World {
    coordinator: Coordinator,
    /// When the coordinator was made.
    made: Instant,
    now: Instant,
    /// The workers that send heartbeats and reports, in the order they registered.
    heard: Vec<String>,
    /// The workers that say where their players are and send no heartbeats.
    beatless: BTreeSet<String>,
    /// The regions no heartbeat vouches for.
    unvouched: BTreeSet<u32>,
    /// The regions no report is made of.
    mute: BTreeSet<u32>,
    /// The regions that stand still: their reports repeat the tick they had.
    still: BTreeSet<u32>,
    /// Chunks that the reports of a region name with no players in them.
    nobody_in: BTreeMap<u32, Vec<ChunkPos>>,
    /// Whether the reports name the chunks in descending order.
    backwards: bool,
    /// Where the players truly are: of each region the chunks with players in them.
    crowds: BTreeMap<u32, BTreeMap<ChunkPos, u32>>,
    /// The tick of the last report of each region.
    reported: BTreeMap<u32, u64>,
    /// The world store's list.
    list: RegionList,
    readings: Readings,
    /// How many readings the coordinator has asked for.
    reads: u32,
    /// When the list was last handed in.
    listed: Option<Instant>,
    /// Whether a reading was asked for that the test has yet to answer.
    unanswered: bool,
    /// What workers were told to release and have not answered.
    releases: Vec<ReleaseOrder>,
    /// What workers were told about merges and splits and have not answered.
    orders: Vec<ReshapeOrder>,
    /// Whether the workers do what they are told at the next step.
    obedient: bool,
    /// Everything the coordinator began by itself, with the time since it was made.
    began: Vec<(Duration, Begun)>,
    /// Every merge and split that ended, with the time since the coordinator was made.
    ended: Vec<(Duration, Reshaped)>,
    /// Every call's answer in words, if the test wants them compared.
    told: Option<Vec<String>>,
}

impl World {
    /// A coordinator that has just been made, for a world of `regions` stripes. The
    /// list has every stripe pinned to its area, as the store has them.
    fn anew(follow: Option<Policy>, regions: u32) -> Self {
        Self::anew_at(follow, regions, Vec3::new(0.5, 64.0, 0.5))
    }

    /// As [`World::anew`], with players entering the world at `spawn`.
    fn anew_at(follow: Option<Policy>, regions: u32, spawn: Vec3) -> Self {
        // Each stripe is four chunks wide, but for the first and the last, which have
        // no end in the west and in the east.
        let boundaries: Vec<i32> = (1..regions as i32).map(|stripe| stripe * 4).collect();
        let area = |stripe: usize| ChunkArea {
            min_x: stripe.checked_sub(1).map(|west| boundaries[west]),
            max_x: boundaries.get(stripe).copied(),
        };
        let list = RegionList {
            home: region(0),
            regions: (0..regions)
                .map(|id| RegionInfo {
                    region: region(id),
                    epoch: 0,
                    bounds: None,
                    pinned: vec![area(id as usize)],
                })
                .collect(),
            absorbed: Vec::new(),
            next: region(regions),
        };
        let config = CoordinatorConfig {
            spawn,
            lease: LEASE,
            follow,
        };
        let now = Instant::now();
        Self {
            coordinator: knowing(config, regions, now, FIRST_EPOCH),
            made: now,
            now,
            heard: Vec::new(),
            beatless: BTreeSet::new(),
            unvouched: BTreeSet::new(),
            mute: BTreeSet::new(),
            still: BTreeSet::new(),
            nobody_in: BTreeMap::new(),
            backwards: false,
            crowds: BTreeMap::new(),
            reported: BTreeMap::new(),
            list,
            readings: Readings::AtOnce,
            reads: 0,
            listed: None,
            unanswered: false,
            releases: Vec::new(),
            orders: Vec::new(),
            obedient: true,
            began: Vec::new(),
            ended: Vec::new(),
            told: None,
        }
    }

    /// The workers register in the order given, the list is read, the grace period
    /// passes and the regions are given away: the lowest first, each to the worker
    /// with the fewest, and of those to the one that registered first. Every region
    /// rests from the end of this call.
    fn settle(&mut self, workers: &[&str]) {
        for name in workers {
            self.register(name, &[]);
        }
        // A coordinator that decides by itself asks for the list at its first tick;
        // one that does not is handed it, as the service reads it at a registration.
        self.tick();
        if self.reads == 0 {
            self.hand_in();
        }
        let lease = self.coordinator.config().lease;
        self.now += lease + MOMENT;
        self.tick();
        let regions = self.list.regions.len();
        for id in 0..regions {
            let expected = workers[id % workers.len()];
            assert_eq!(self.owner(id as u32).as_deref(), Some(expected));
        }
        assert!(self.table().is_complete());
    }

    /// Steps until no region rests any more.
    fn rest(&mut self) {
        // A coordinator that decides nothing by itself notes no rest: there the time
        // passes all the same, so that the two can be driven alike.
        let until = self
            .known()
            .into_iter()
            .filter_map(|id| self.alone_until(id))
            .max()
            .unwrap_or(self.now + REST);
        let began = self.run_to(until);
        assert_eq!(began, [], "nothing is begun while every region rests");
    }

    /// Steps for `time` and holds that nothing is begun in it.
    fn quiet(&mut self, time: Duration) {
        let began = self.run(time);
        assert_eq!(began, [], "{}", self.story());
    }

    /// Steps for `time` and holds that nothing is begun in it but that regions are
    /// told to prepare.
    fn quiet_but_for_prepare(&mut self, time: Duration) {
        let began = but_for_prepare(self.run(time));
        assert_eq!(began, [], "{}", self.story());
    }

    /// Steps until the region is run under another epoch than it is now, or is run
    /// again, and holds that nothing is begun until then, nor at the tick at which it
    /// is. Returns when that was.
    fn until_given(&mut self, id: u32, limit: Duration) -> Instant {
        let had = self.table().route(region(id)).map(|route| route.epoch);
        let end = self.now + limit;
        while self.now + LOOK <= end {
            let look = self.step();
            assert_eq!(look.begun, [], "{}", self.story());
            let has = self.table().route(region(id)).map(|route| route.epoch);
            if has.is_some() && has != had {
                return self.now;
            }
        }
        panic!(
            "region {id} was not given away in {limit:?}\n{}",
            self.story()
        );
    }

    /// A world of `regions` stripes whose regions the workers were given as
    /// [`World::settle`] says, and have rested.
    fn rested(follow: Option<Policy>, regions: u32, workers: &[&str]) -> Self {
        let mut world = Self::anew(follow, regions);
        world.settle(workers);
        world.rest();
        world
    }

    /// A coordinator that has just been made, to which workers report what they run,
    /// each region with the epoch `10 + id`: so a test says who runs what. The list
    /// is read at once. Nothing has rested and the grace period is not over.
    fn reported(follow: Option<Policy>, regions: u32, workers: &[(&str, &[u32])]) -> Self {
        let mut world = Self::anew(follow, regions);
        for (name, ids) in workers {
            let holding: Vec<Assignment> = ids
                .iter()
                .map(|id| held(*id, 10 + u64::from(*id)))
                .collect();
            world.register(name, &holding);
            for id in *ids {
                assert_eq!(world.owner(*id).as_deref(), Some(*name));
            }
        }
        world.tick();
        if world.reads == 0 {
            world.hand_in();
        }
        world
    }

    /// As [`World::reported`], and stepped until the grace period is over and every
    /// region has rested.
    fn running(follow: Option<Policy>, regions: u32, workers: &[(&str, &[u32])]) -> Self {
        let mut world = Self::reported(follow, regions, workers);
        let until = world.made + REST.max(LEASE + LOOK);
        let began = world.run_to(until);
        assert_eq!(began, [], "nothing is begun while every region rests");
        world
    }

    // ----- The workers ---------------------------------------------------------------

    fn register(&mut self, name: &str, holding: &[Assignment]) -> Changes {
        if !self.heard.iter().any(|heard| heard == name) {
            self.heard.push(name.to_owned());
        }
        let changes = self
            .coordinator
            .register(self.now, name, &address(name), holding);
        self.take("register", changes)
    }

    /// The worker registers again with what it was told to run, as one does that lost
    /// its connection.
    fn register_again(&mut self, name: &str) -> Changes {
        let holding = self.coordinator.assignments(name);
        self.register(name, &holding)
    }

    /// The worker says no more from now on.
    fn silence(&mut self, name: &str) {
        self.heard.retain(|heard| heard != name);
    }

    /// A heartbeat of every worker that is heard, which vouches for all it was given
    /// but for the regions that a test has nobody vouch for.
    fn beat(&mut self) {
        for name in &self.heard {
            if self.beatless.contains(name) {
                continue;
            }
            let vouched: Vec<(RegionId, Vouch)> = self
                .coordinator
                .assignments(name)
                .iter()
                .filter(|assignment| !self.unvouched.contains(&assignment.region.0))
                .map(|assignment| (assignment.region, Vouch::Committed))
                .collect();
            self.coordinator.heartbeat(self.now, name, &vouched);
        }
    }

    /// What the region's worker would say of it now: the players where they truly are,
    /// and a tick above the one it said last. A part's sighting is made with tick 0,
    /// so the first is 1.
    fn word_of(&mut self, id: u32, epoch: u64) -> PlayersOf {
        let tick = self.reported.entry(id).or_insert(0);
        if *tick == 0 || !self.still.contains(&id) {
            *tick += 1;
        }
        let tick = *tick;
        let mut crowds: Vec<(ChunkPos, u32)> = self
            .crowds
            .get(&id)
            .map(|crowds| {
                crowds
                    .iter()
                    .map(|(chunk, count)| (*chunk, *count))
                    .collect()
            })
            .unwrap_or_default();
        // A chunk that is named with nobody in it: no worker says that, and the
        // coordinator is to leave it out (section 2.3).
        if let Some(nobody) = self.nobody_in.get(&id) {
            crowds.extend(nobody.iter().map(|chunk| (*chunk, 0)));
        }
        if self.backwards {
            crowds.reverse();
        }
        PlayersOf {
            region: region(id),
            epoch,
            tick,
            crowds,
        }
    }

    /// Every worker that is heard says where the players of each region it runs are,
    /// but for the regions that a test has nobody report.
    fn report(&mut self) {
        for name in self.heard.clone() {
            let runs = self.coordinator.assignments(&name);
            let words: Vec<PlayersOf> = runs
                .iter()
                .filter(|assignment| !self.mute.contains(&assignment.region.0))
                .map(|assignment| (assignment.region.0, assignment.epoch))
                .collect::<Vec<_>>()
                .into_iter()
                .map(|(id, epoch)| self.word_of(id, epoch))
                .collect();
            self.coordinator.players(self.now, &name, &words);
        }
    }

    /// The owners of these regions report them, and nobody reports anything else.
    fn report_of(&mut self, ids: &[u32]) {
        for id in ids {
            let owner = self.owner(*id).expect("the region has an owner");
            let word = self.word_of(*id, self.epoch(*id));
            assert!(self.coordinator.players(self.now, &owner, &[word]));
        }
    }

    // ----- Where the players are -----------------------------------------------------

    /// The players of the region are in these chunks from now on: `(x, z, how many)`.
    fn put(&mut self, id: u32, crowds: &[(i32, i32, u32)]) {
        self.crowds.insert(
            id,
            crowds
                .iter()
                .map(|(x, z, count)| (ChunkPos::new(*x, *z), *count))
                .collect(),
        );
    }

    /// Further players of the region, in the chunk `x` of the row z = 0.
    fn join(&mut self, id: u32, x: i32, count: u32) {
        *self.crowds.entry(id).or_default().entry(at(x)).or_insert(0) += count;
    }

    // ----- The list ------------------------------------------------------------------

    /// The list has these regions pinned to no area from now on, as parts are.
    fn unpin(&mut self, ids: &[u32]) {
        for info in &mut self.list.regions {
            if ids.contains(&info.region.0) {
                info.pinned.clear();
            }
        }
    }

    /// Hands the list in as it is, asked for or not.
    fn hand_in(&mut self) -> Changes {
        self.unanswered = false;
        self.listed = Some(self.now);
        let list = self.list.clone();
        let changes = self.coordinator.listed(self.now, &list);
        self.take("listed", changes)
    }

    /// Says that the list could not be read.
    fn fail_reading(&mut self) -> Changes {
        self.unanswered = false;
        let changes = self.coordinator.unlisted(self.now);
        self.take("unlisted", changes)
    }

    // ----- What the coordinator says -------------------------------------------------

    /// Notes what a call changed: what workers were told, what ended, and a reading
    /// that it asks for, which is answered as the test has it. Returns the changes with
    /// those of the reading.
    fn take(&mut self, call: &str, changes: Changes) -> Changes {
        if let Some(told) = &mut self.told {
            told.push(format!("{:?} {call}: {changes:?}", self.now - self.made));
        }
        // "Nothing is decided in any other call" than a tick (section 5): but for
        // what somebody asks for, no other call has a region prepare or split.
        if !matches!(call, "tick" | "merge" | "split") {
            let decided = changes
                .orders
                .iter()
                .any(|told| matches!(told.order, Order::Prepare { .. } | Order::SplitOff { .. }));
            assert!(!decided, "`{call}` begins something: {changes:?}");
        }
        self.releases.extend(changes.releases.iter().cloned());
        self.orders.extend(changes.orders.iter().cloned());
        for ended in &changes.reshaped {
            self.ended.push((self.now - self.made, ended.clone()));
        }
        let mut total = changes;
        if total.read {
            self.reads += 1;
            match self.readings {
                Readings::AtOnce => add(&mut total, self.hand_in()),
                Readings::Failing => add(&mut total, self.fail_reading()),
                Readings::Held => self.unanswered = true,
            }
        }
        total
    }

    /// What a tick began, by what it told the workers and by what is under way after
    /// it that was not before. A merge that the coordinator begins by itself has to go
    /// the way of one asked for by hand: a release of the one region and a `Prepare`
    /// for the other.
    fn begun(&self, before: &[Asked], changes: &Changes) -> Vec<Begun> {
        let mut releases: Vec<&ReleaseOrder> = changes.releases.iter().collect();
        let mut orders: Vec<&ReshapeOrder> = changes.orders.iter().collect();
        let mut begun = Vec::new();
        for asked in self.coordinator.under_way() {
            if before.contains(&asked) {
                continue;
            }
            match asked {
                Asked::Merge { survivor, absorbed } => {
                    let release = releases
                        .iter()
                        .position(|release| release.region == absorbed)
                        .unwrap_or_else(|| panic!("no release for {asked:?}: {changes:?}"));
                    let release = releases.remove(release);
                    assert_eq!(Some(&release.worker), self.owner(absorbed.0).as_ref());
                    assert_eq!(release.epoch, self.epoch(absorbed.0));
                    let prepare = orders
                        .iter()
                        .position(|told| {
                            told.order
                                == Order::Prepare {
                                    region: survivor,
                                    epoch: self.epoch(survivor.0),
                                }
                        })
                        .unwrap_or_else(|| panic!("no `Prepare` for {asked:?}: {changes:?}"));
                    let prepare = orders.remove(prepare);
                    assert_eq!(Some(&prepare.worker), self.owner(survivor.0).as_ref());
                    begun.push(merge(survivor.0, absorbed.0));
                }
                Asked::Split { region: of } => {
                    let order = orders
                        .iter()
                        .position(|told| {
                            matches!(&told.order, Order::SplitOff { region, .. } if *region == of)
                        })
                        .unwrap_or_else(|| panic!("no order for {asked:?}: {changes:?}"));
                    let order = orders.remove(order);
                    assert_eq!(Some(&order.worker), self.owner(of.0).as_ref());
                    let Order::SplitOff {
                        epoch,
                        chunks,
                        as_epoch,
                        part,
                        ..
                    } = &order.order
                    else {
                        unreachable!("it was looked for as an order to split");
                    };
                    assert_eq!(*epoch, self.epoch(of.0));
                    assert!(*as_epoch > self.highest_epoch());
                    begun.push(Begun::Split {
                        region: of.0,
                        part: part.0,
                        chunks: chunks.clone(),
                    });
                }
            }
        }
        for told in orders {
            match &told.order {
                Order::Prepare { region, epoch } => {
                    assert_eq!(Some(&told.worker), self.owner(region.0).as_ref());
                    assert_eq!(*epoch, self.epoch(region.0));
                    begun.push(Begun::Prepare(region.0));
                }
                other => begun.push(Begun::Other(format!("{other:?}"))),
            }
        }
        for release in releases {
            begun.push(Begun::Move(release.region.0));
        }
        begun
    }

    /// The workers are heard and the coordinator looks, at this instant.
    fn tick(&mut self) -> Look {
        self.beat();
        let before = self.coordinator.under_way();
        let changes = self.coordinator.tick(self.now);
        let begun = self.begun(&before, &changes);
        for one in &begun {
            self.began.push((self.now - self.made, one.clone()));
        }
        let changes = self.take("tick", changes);
        Look { begun, changes }
    }

    /// A step: [`LAG`] after the tick before, the workers do what they were told, if
    /// they are obedient, and report; a [`LOOK`] after the tick before, the
    /// coordinator looks.
    fn step(&mut self) -> Look {
        self.now += LAG;
        if self.obedient {
            self.obey();
        }
        self.report();
        self.now += LOOK - LAG;
        self.tick()
    }

    /// Steps for as long as the next tick is not after `end`. Returns everything the
    /// ticks began.
    fn run_to(&mut self, end: Instant) -> Vec<Begun> {
        let mut begun = Vec::new();
        while self.now + LOOK <= end {
            begun.extend(self.step().begun);
        }
        begun
    }

    /// Steps for `time`, which is a number of looks. Returns everything the ticks
    /// began.
    fn run(&mut self, time: Duration) -> Vec<Begun> {
        self.run_to(self.now + time)
    }

    /// Steps for as long as the next tick is before `end`: the tick at `end`, or the
    /// first after it, is left to the test.
    fn run_up_to(&mut self, end: Instant) -> Vec<Begun> {
        let mut begun = Vec::new();
        while self.now + LOOK < end {
            begun.extend(self.step().begun);
        }
        begun
    }

    /// Steps until a tick begins something, for `limit` at most. Returns when that
    /// was and what it was.
    fn next_begun(&mut self, limit: Duration) -> (Instant, Vec<Begun>) {
        let end = self.now + limit;
        while self.now + LOOK <= end {
            let look = self.step();
            if !look.begun.is_empty() {
                return (self.now, look.begun);
            }
        }
        panic!("nothing was begun in {limit:?}\n{}", self.story());
    }

    /// As [`World::next_begun`], and passes over the ticks that only tell regions to
    /// prepare.
    fn next_but_prepare(&mut self, limit: Duration) -> (Instant, Vec<Begun>) {
        let end = self.now + limit;
        while self.now + LOOK <= end {
            let begun = but_for_prepare(self.step().begun);
            if !begun.is_empty() {
                return (self.now, begun);
            }
        }
        panic!("nothing was begun in {limit:?}\n{}", self.story());
    }

    /// How the merge or the split ended that ended last, and when.
    fn last_ended(&self) -> (Instant, Reshaped) {
        let (when, ended) = self.ended.last().expect("something has ended");
        (self.made + *when, ended.clone())
    }

    /// The workers report, a little before `when` as in a step if there is the time,
    /// and the coordinator looks at `when`.
    fn tick_at(&mut self, when: Instant) -> Look {
        assert!(when >= self.now, "the time of a test does not go back");
        if when >= self.now + (LOOK - LAG) {
            self.now = when - (LOOK - LAG);
            if self.obedient {
                self.obey();
            }
            self.report();
        }
        self.now = when;
        self.tick()
    }

    /// What a tick at `when` would begin, with a report before it as in a step. The
    /// world is left as it is.
    fn would_begin_at(&self, when: Instant) -> Vec<Begun> {
        self.clone().tick_at(when).begun
    }

    /// What ticks just before `when`, at it and just after it would begin, each in a
    /// world of its own that is as this one until then.
    fn around_instant(&self, when: Instant) -> [Vec<Begun>; 3] {
        [
            self.would_begin_at(when - MOMENT),
            self.would_begin_at(when),
            self.would_begin_at(when + MOMENT),
        ]
    }

    // ----- What the workers do with what they are told -------------------------------

    /// The players of `absorbed` are the survivor's, and the list has the one absorbed
    /// by the other, with what it was pinned to.
    fn merged(&mut self, survivor: u32, absorbed: u32) {
        let moved = self.crowds.remove(&absorbed).unwrap_or_default();
        let into = self.crowds.entry(survivor).or_default();
        for (chunk, count) in moved {
            *into.entry(chunk).or_insert(0) += count;
        }
        self.reported.remove(&absorbed);
        let gone = self
            .list
            .regions
            .iter()
            .position(|info| info.region == region(absorbed))
            .expect("the list has the region that is absorbed");
        let gone = self.list.regions.remove(gone);
        let stays = self
            .list
            .regions
            .iter_mut()
            .find(|info| info.region == region(survivor))
            .expect("the list has the survivor");
        stays.pinned.extend(gone.pinned);
        self.list
            .absorbed
            .push((region(absorbed), region(survivor)));
    }

    /// Whoever stands in a chunk named is of a new region, which has the list's next
    /// id whatever id the order named. Returns it, or nothing if nobody stands there.
    fn parted(&mut self, of: u32, chunks: &[ChunkPos], as_epoch: u64) -> Option<u32> {
        let stays = self.crowds.entry(of).or_default();
        let goes: BTreeMap<ChunkPos, u32> = stays
            .iter()
            .filter(|(chunk, _)| chunks.contains(chunk))
            .map(|(chunk, count)| (*chunk, *count))
            .collect();
        if goes.is_empty() {
            return None;
        }
        stays.retain(|chunk, _| !goes.contains_key(chunk));
        let part = self.list.next.0;
        self.crowds.insert(part, goes);
        self.list.regions.push(RegionInfo {
            region: region(part),
            epoch: as_epoch,
            bounds: None,
            pinned: Vec::new(),
        });
        self.list.next = region(part + 1);
        Some(part)
    }

    /// The owner of the region says that it has let go of it, as it was told.
    fn let_go(&mut self, id: u32) -> Changes {
        let release = self
            .releases
            .iter()
            .position(|release| release.region == region(id))
            .unwrap_or_else(|| panic!("nobody was told to release region {id}"));
        let release = self.releases.remove(release);
        let changes =
            self.coordinator
                .released(self.now, &release.worker, release.region, release.epoch);
        self.take("released", changes)
    }

    /// The order to absorb `absorbed` that a worker has not answered yet.
    fn order_to_absorb(&mut self, absorbed: u32) -> (String, RegionId) {
        let order = self
            .orders
            .iter()
            .position(|told| {
                matches!(&told.order, Order::Absorb { absorbed: gone, .. } if *gone == region(absorbed))
            })
            .unwrap_or_else(|| panic!("nobody was told to absorb region {absorbed}"));
        let order = self.orders.remove(order);
        let Order::Absorb { region: into, .. } = order.order else {
            unreachable!("it was looked for as an order to absorb");
        };
        (order.worker, into)
    }

    /// The survivor's worker says that it has absorbed the region, and the list has
    /// it so from now on.
    fn absorb(&mut self, absorbed: u32) -> Changes {
        let (worker, into) = self.order_to_absorb(absorbed);
        self.merged(into.0, absorbed);
        let changes =
            self.coordinator
                .absorb_ended(self.now, &worker, into, region(absorbed), Ok(()));
        self.take("absorb_ended", changes)
    }

    /// The survivor's worker says that nothing came of absorbing the region, and why.
    fn absorb_off(&mut self, absorbed: u32, why: Off) -> Changes {
        let (worker, into) = self.order_to_absorb(absorbed);
        let changes =
            self.coordinator
                .absorb_ended(self.now, &worker, into, region(absorbed), Err(why));
        self.take("absorb_ended", changes)
    }

    /// The merge that is under way is done: the one worker lets go, the other absorbs.
    fn merge_through(&mut self, absorbed: u32) -> Changes {
        let mut total = self.let_go(absorbed);
        add(&mut total, self.absorb(absorbed));
        total
    }

    /// The merge that is under way comes to nothing at the survivor's worker.
    fn merge_off(&mut self, absorbed: u32, why: Off) -> Changes {
        let mut total = self.let_go(absorbed);
        add(&mut total, self.absorb_off(absorbed, why));
        total
    }

    /// The order to split `of` that its worker has not answered yet.
    fn order_to_split(&mut self, of: u32) -> (String, Vec<ChunkPos>, u64, u32) {
        let order = self
            .orders
            .iter()
            .position(
                |told| matches!(&told.order, Order::SplitOff { region: split, .. } if *split == region(of)),
            )
            .unwrap_or_else(|| panic!("nobody was told to split region {of}"));
        let order = self.orders.remove(order);
        let Order::SplitOff {
            chunks,
            as_epoch,
            part,
            ..
        } = order.order
        else {
            unreachable!("it was looked for as an order to split");
        };
        (order.worker, chunks, as_epoch, part.0)
    }

    /// The region's worker splits it as it was told, with whoever stands in the chunks
    /// named, and says so. Returns the new region.
    fn split_through(&mut self, of: u32) -> (u32, Changes) {
        let (worker, chunks, as_epoch, _) = self.order_to_split(of);
        let part = self
            .parted(of, &chunks, as_epoch)
            .expect("somebody stands in a chunk named");
        let changes =
            self.coordinator
                .split_ended(self.now, &worker, region(of), as_epoch, Ok(region(part)));
        (part, self.take("split_ended", changes))
    }

    /// The region's worker says that nothing came of splitting it, and why.
    fn split_off(&mut self, of: u32, why: Off) -> Changes {
        let (worker, _, as_epoch, _) = self.order_to_split(of);
        let changes =
            self.coordinator
                .split_ended(self.now, &worker, region(of), as_epoch, Err(why));
        self.take("split_ended", changes)
    }

    /// The workers do everything they were told and have not done: they let go of
    /// what they were to release, absorb, and split with whoever stands in the chunks
    /// named, or answer that nobody does.
    fn obey(&mut self) {
        for _ in 0..64 {
            if let Some(release) = self.releases.first().cloned() {
                self.let_go(release.region.0);
            } else if let Some(told) = self.orders.first().cloned() {
                match told.order {
                    Order::Prepare { .. } => {
                        self.orders.remove(0);
                    }
                    Order::Absorb { absorbed, .. } => {
                        self.absorb(absorbed.0);
                    }
                    Order::SplitOff {
                        region: of,
                        ref chunks,
                        ..
                    } => {
                        let somebody = self.crowds.get(&of.0).is_some_and(|crowds| {
                            crowds.keys().any(|chunk| chunks.contains(chunk))
                        });
                        if somebody {
                            self.split_through(of.0);
                        } else {
                            self.split_off(of.0, Off::Nobody);
                        }
                    }
                }
            } else {
                return;
            }
        }
        panic!("the workers are told more and more\n{}", self.story());
    }

    // ----- What can be seen of the coordinator ---------------------------------------

    fn table(&self) -> RoutingTable {
        self.coordinator.routing_table()
    }

    /// The name of the worker the routing table sends edges to for the region.
    fn owner(&self, id: u32) -> Option<String> {
        let table = self.table();
        let route = table.route(region(id))?;
        let name = route
            .address
            .strip_suffix(":25600")
            .expect("an address of these tests");
        // What the workers are told has to agree with what the edges are.
        let told = self.coordinator.assignments(name);
        assert!(
            told.iter().any(
                |assignment| assignment.region == region(id) && assignment.epoch == route.epoch
            ),
            "the route {route:?} is not among {name}'s assignments {told:?}"
        );
        Some(name.to_owned())
    }

    fn epoch(&self, id: u32) -> u64 {
        self.table()
            .route(region(id))
            .unwrap_or_else(|| panic!("region {id} has no owner"))
            .epoch
    }

    /// The regions the worker is to run.
    fn runs(&self, name: &str) -> Vec<u32> {
        self.coordinator
            .assignments(name)
            .iter()
            .map(|assignment| assignment.region.0)
            .collect()
    }

    /// The regions the coordinator knows: those with a route and those that wait.
    fn known(&self) -> Vec<u32> {
        let table = self.table();
        let mut known: Vec<u32> = table.routes.iter().map(|route| route.region.0).collect();
        known.extend(self.coordinator.waiting().iter().map(|region| region.0));
        known.sort_unstable();
        known
    }

    /// The highest epoch of any route.
    fn highest_epoch(&self) -> u64 {
        self.table()
            .routes
            .iter()
            .map(|route| route.epoch)
            .max()
            .unwrap_or(FIRST_EPOCH)
            .max(FIRST_EPOCH)
    }

    fn alone_until(&self, id: u32) -> Option<Instant> {
        self.coordinator.alone_until(region(id))
    }

    fn under_way(&self) -> Vec<Asked> {
        self.coordinator.under_way()
    }

    /// The time since the coordinator was made, for a failing test's words.
    fn clock(&self) -> Duration {
        self.now - self.made
    }

    /// What the coordinator began and what ended, in words, for a test that fails.
    fn story(&self) -> String {
        let mut story = format!("at {:?}\n", self.clock());
        for (when, begun) in &self.began {
            story.push_str(&format!("  {when:?} begun {begun:?}\n"));
        }
        for (when, ended) in &self.ended {
            story.push_str(&format!("  {when:?} ended {ended:?}\n"));
        }
        story
    }
}

/// How a merge that the coordinator began by itself ended.
fn merge_ended(survivor: u32, absorbed: u32, outcome: Result<u32, Undone>) -> Reshaped {
    Reshaped {
        asker: None,
        asked: Asked::Merge {
            survivor: region(survivor),
            absorbed: region(absorbed),
        },
        outcome: outcome.map(region),
    }
}

/// How a split that the coordinator began by itself ended.
fn split_ended(of: u32, outcome: Result<u32, Undone>) -> Reshaped {
    Reshaped {
        asker: None,
        asked: Asked::Split { region: region(of) },
        outcome: outcome.map(region),
    }
}

/// Three stripes of one worker that have rested: region 1 has a player in chunk 100
/// and region 2 one in chunk 102, which is the merge distance from it. Neither is
/// near the chunk players enter in, and nothing of it has been reported yet.
fn pair() -> World {
    let mut world = World::rested(follow(), 3, &["a"]);
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(102, 0, 1)]);
    world
}

/// What a worker would say of the region if its one player stood in the chunk `x`.
fn word(world: &World, id: u32, tick: u64, x: i32) -> PlayersOf {
    PlayersOf {
        region: region(id),
        epoch: world.epoch(id),
        tick,
        crowds: vec![(at(x), 1)],
    }
}

/// Nothing just before, and the one thing at the instant and just after it: what
/// [`World::around_instant`] gives of a thing that is begun as soon as a time has come.
fn from_the_instant_on(begun: Begun) -> [Vec<Begun>; 3] {
    [Vec::new(), vec![begun.clone()], vec![begun]]
}

/// Nothing just before and at the instant, and the one thing just after it: what
/// [`World::around_instant`] gives of a thing that is begun when more than a time has
/// passed.
fn after_the_instant(begun: Begun) -> [Vec<Begun>; 3] {
    [Vec::new(), Vec::new(), vec![begun]]
}

// ---------------------------------------------------------------------------------
// F1 to F6. Hearing (section 2.3).
// ---------------------------------------------------------------------------------

// F1.
#[test]
fn a_report_from_a_worker_that_does_not_own_the_region_changes_nothing() {
    let mut world = World::running(follow(), 3, &[("a", &[0, 1]), ("b", &[2])]);
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(110, 0, 1)]);
    for step in 0..20 {
        world.now += LAG;
        world.report();
        // `b`, which has just been heard, says with the epoch that `a` runs region 1
        // with, and with a tick far above `a`'s, that the player of region 1 stands
        // two chunks from its own.
        let before = format!("{:?}", world.coordinator);
        let lie = word(&world, 1, 1_000_000 + step, 108);
        assert!(world.coordinator.players(world.now, "b", &[lie]));
        assert_eq!(format!("{:?}", world.coordinator), before);
        world.now += LOOK - LAG;
        assert_eq!(world.tick().begun, []);
    }
    // What `a` says of it is taken as before, though its ticks are below the lie's.
    world.put(1, &[(108, 0, 1)]);
    world.quiet(LOOK);
    let first = world.now;
    assert_eq!(
        world.next_begun(2 * FRESH),
        (first + FRESH + LOOK, vec![merge(1, 2)])
    );
}

// F1.
#[test]
fn a_report_with_another_epoch_than_the_region_has_changes_nothing() {
    let mut world = World::rested(follow(), 3, &["a"]);
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(110, 0, 1)]);
    for step in 0..20 {
        world.now += LAG;
        world.report();
        let before = format!("{:?}", world.coordinator);
        for epoch in [world.epoch(1) - 1, world.epoch(1) + 1, 0, u64::MAX] {
            let mut lie = word(&world, 1, 1_000_000 + step, 108);
            lie.epoch = epoch;
            assert!(world.coordinator.players(world.now, "a", &[lie]));
        }
        // And what it says of a region that nobody knows.
        let stranger = PlayersOf {
            region: region(99),
            epoch: 7,
            tick: 1,
            crowds: vec![(at(101), 4)],
        };
        assert!(world.coordinator.players(world.now, "a", &[stranger]));
        assert_eq!(format!("{:?}", world.coordinator), before);
        world.now += LOOK - LAG;
        assert_eq!(world.tick().begun, []);
    }
    world.put(1, &[(108, 0, 1)]);
    world.quiet(LOOK);
    let first = world.now;
    assert_eq!(
        world.next_begun(2 * FRESH),
        (first + FRESH + LOOK, vec![merge(1, 2)])
    );
}

/// Four stripes of one worker, rested. Region 2 has a player in chunk 108 and one in
/// chunk 117, nine apart, and region 1 has one far from both. Players of region 1 in
/// the chunks 110 and 115 would join the two (section 11, D9): each is within the
/// merge distance of one of them, and the two are the split distance apart.
fn two_groups_and_a_region_that_could_join_them() -> World {
    let mut world = World::rested(follow(), 4, &["a"]);
    world.put(1, &[(300, 0, 1)]);
    world.put(2, &[(108, 0, 1), (117, 0, 1)]);
    world
}

// F2.
#[test]
fn a_report_about_a_reserved_region_is_passed_over() {
    let mut world = two_groups_and_a_region_that_could_join_them();
    assert_eq!(world.step().begun, [Begun::Prepare(2)]);
    let first = world.now;
    // Somebody asks for region 1 to absorb region 3, and the workers take their time.
    world.obedient = false;
    let changes = world
        .coordinator
        .merge(world.now, region(1), region(3), ASKER)
        .expect("nothing speaks against the merge");
    world.take("merge", changes);
    // While it is reserved, region 1's worker says that it has players between the
    // two groups of region 2. Were that taken, it would be a sighting that is not
    // fresh and joins them, and region 2 would not be split (K12).
    world.put(1, &[(110, 0, 1), (115, 0, 1), (300, 0, 1)]);
    world.quiet(FRESH);
    assert_eq!(world.now, first + FRESH);
    assert_eq!(world.step().begun, [split(2, 4, &[&[at(117)]])]);
}

// F2. The other half: the same report of a region that is not reserved is taken, so
// the test above shows what it says it does.
#[test]
fn the_same_report_about_a_region_that_is_not_reserved_keeps_the_other_from_being_split() {
    let mut world = two_groups_and_a_region_that_could_join_them();
    assert_eq!(world.step().begun, [Begun::Prepare(2)]);
    world.put(1, &[(110, 0, 1), (115, 0, 1), (300, 0, 1)]);
    let began = world.run(REST);
    assert!(
        !began
            .iter()
            .any(|begun| matches!(begun, Begun::Split { region: 2, .. })),
        "{began:?}"
    );
}

// F3.
#[test]
fn a_region_that_repeats_a_tick_is_in_nothing_after_a_second() {
    let mut world = pair();
    world.quiet(LOOK);
    // Region 2 stands still from its first report with the player near region 1's:
    // every report of it has the tick that the sighting has.
    world.still.insert(2);
    world.quiet(2 * REST);
    // Nor is a report taken that has a tick below it.
    world.reported.insert(2, 1);
    world.quiet(REST);
    // It ticks on, from the tick it stood at.
    world.reported.insert(2, 1_000);
    world.still.clear();
    world.quiet(LOOK);
    let first = world.now;
    assert_eq!(
        world.next_begun(2 * FRESH),
        (first + FRESH + LOOK, vec![merge(1, 2)])
    );
}

// F4.
#[test]
fn a_worker_that_is_not_registered_is_told_so_when_it_reports() {
    let mut world = pair();
    let lie = word(&world, 2, 1_000_000, 102);
    assert!(
        !world
            .coordinator
            .players(world.now, "nobody", std::slice::from_ref(&lie))
    );
    assert!(!world.coordinator.players(world.now, "nobody", &[]));
    assert!(world.coordinator.players(world.now, "a", &[]));
    world.quiet(LOOK);
    // And one that was forgotten for being silent.
    world.silence("a");
    world.now += LEASE + MOMENT;
    world.tick();
    assert!(!world.coordinator.players(world.now, "a", &[lie]));
}

// F5.
#[test]
fn a_coordinator_that_decides_nothing_by_itself_hears_reports_and_does_nothing() {
    let mut world = World::rested(None, 3, &["a"]);
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(102, 0, 1)]);
    let reads = world.reads;
    let table = world.table();
    for look in 0..240 {
        world.now += LAG;
        world.report();
        // It keeps nothing of a report: the worker has just been heard, and what it
        // says of its players changes nothing at all.
        let before = format!("{:?}", world.coordinator);
        let words = [
            word(&world, 1, 1_000 + look, 100),
            word(&world, 2, 1_000 + look, 102),
        ];
        assert!(world.coordinator.players(world.now, "a", &words));
        assert_eq!(format!("{:?}", world.coordinator), before);
        world.now += LOOK - LAG;
        let look = world.tick();
        assert_eq!(look.begun, []);
        assert_eq!(look.changes, Changes::default());
    }
    assert_eq!(world.reads, reads);
    assert_eq!(world.table(), table);
    for id in 0..3 {
        assert_eq!(world.alone_until(id), None);
    }
}

// F5. Section 2.3: to say where the players are is to be heard from, and vouches for
// nothing, with `follow` and without.
#[test]
fn a_worker_that_only_reports_is_not_forgotten_and_loses_its_regions_a_lease_after_the_last_heartbeat()
 {
    for policy in [follow(), None] {
        let mut world = World::rested(policy, 3, &["a"]);
        let vouched = world.now;
        let epoch = world.epoch(1);
        world.beatless.insert("a".to_owned());
        assert_eq!(world.run_up_to(vouched + LEASE), []);
        // Just before a lease is out, and when it is, the region is its owner's.
        for when in [vouched + LEASE - MOMENT, vouched + LEASE] {
            let mut still = world.clone();
            still.tick_at(when);
            assert_eq!(still.epoch(1), epoch);
        }
        // Just after, it is taken from the worker, which is still registered and is
        // given it anew, with another epoch, as there is nobody else.
        world.tick_at(vouched + LEASE + MOMENT);
        assert!(world.coordinator.players(world.now, "a", &[]));
        assert_eq!(world.owner(1).as_deref(), Some("a"));
        assert!(world.epoch(1) > epoch);
        // And a region that is given an epoch rests (section 5.4).
        let rests = policy.map(|_| world.now + REST);
        assert_eq!(world.alone_until(1), rests);
    }
}

// F6.
#[test]
fn an_owner_that_registers_again_with_the_epoch_it_had_keeps_the_sighting_fresh_and_begins_no_rest()
{
    let mut world = pair();
    world.quiet(FRESH + LOOK);
    let alone = [world.alone_until(1), world.alone_until(2)];
    // The worker reports, loses its connection and registers again a moment later.
    // It says no more until the look at which the merge has stood.
    world.now += LAG;
    world.report();
    world.now += MOMENT;
    let changes = world.coordinator.disconnected(world.now, "a");
    world.take("disconnected", changes);
    world.now += MOMENT;
    world.register_again("a");
    assert_eq!([world.alone_until(1), world.alone_until(2)], alone);
    world.now += LOOK - LAG - 2 * MOMENT;
    assert_eq!(world.tick().begun, [merge(1, 2)]);
}

// F6.
#[test]
fn an_owner_that_reports_an_epoch_the_coordinator_did_not_have_begins_a_rest() {
    let mut world = pair();
    world.quiet(FRESH + LOOK);
    world.now += LAG;
    world.report();
    // The worker runs region 2 with a later epoch than the coordinator has for it.
    let mut holding = world.coordinator.assignments("a");
    let epoch = world.highest_epoch() + 5;
    holding[2].epoch = epoch;
    world.register("a", &holding);
    let reported = world.now;
    assert_eq!(world.epoch(2), epoch);
    assert_eq!(world.alone_until(2), Some(reported + REST));
    // The sighting is of the epoch before and is not fresh until the worker reports
    // again; the merge is begun when the region has rested.
    assert_eq!(world.run_up_to(reported + REST), []);
    assert_eq!(
        world.around_instant(reported + REST),
        from_the_instant_on(merge(1, 2))
    );
}

// ---------------------------------------------------------------------------------
// F7 to F10. Merging (sections 4.2 and 5.3).
// ---------------------------------------------------------------------------------

// F7.
#[test]
fn nothing_is_begun_at_the_first_look_and_a_merge_when_it_has_stood() {
    let mut world = pair();
    let look = world.step();
    assert_eq!(look.begun, []);
    assert_eq!(look.changes.releases, []);
    assert_eq!(look.changes.orders, []);
    let first = world.now;
    world.quiet(3 * LOOK);
    // It has stood when it has been wanted for more than a second.
    assert_eq!(
        world.around_instant(first + FRESH),
        after_the_instant(merge(1, 2))
    );
    // With ticks a look apart that is the sixth.
    world.quiet(LOOK);
    let (owner, epochs) = (world.owner(2), [world.epoch(1), world.epoch(2)]);
    let look = world.step();
    assert_eq!(world.now, first + FRESH + LOOK);
    assert_eq!(look.begun, [merge(1, 2)]);
    assert_eq!(
        look.changes.releases,
        [ReleaseOrder {
            worker: owner.clone().expect("region 2 has an owner"),
            region: region(2),
            epoch: epochs[1],
        }]
    );
    assert_eq!(
        look.changes.orders,
        [ReshapeOrder {
            worker: owner.expect("region 2 has an owner"),
            order: Order::Prepare {
                region: region(1),
                epoch: epochs[0],
            },
        }]
    );
    assert_eq!(
        world.under_way(),
        [Asked::Merge {
            survivor: region(1),
            absorbed: region(2),
        }]
    );
    // It ends as a merge that somebody asked for does, with nobody as asker.
    let changes = world.merge_through(2);
    assert_eq!(changes.reshaped, [merge_ended(1, 2, Ok(1))]);
    assert_eq!(world.known(), [0, 1]);
    assert_eq!(world.under_way(), []);
}

// F7.
#[test]
fn a_tick_at_which_a_merge_is_not_wanted_begins_the_second_of_standing_anew() {
    let mut world = pair();
    world.quiet(4 * LOOK);
    // One report has them three chunks apart, and the next two again.
    world.put(2, &[(103, 0, 1)]);
    world.quiet(LOOK);
    world.put(2, &[(102, 0, 1)]);
    world.quiet(LOOK);
    let anew = world.now;
    world.quiet(3 * LOOK);
    assert_eq!(
        world.around_instant(anew + FRESH),
        after_the_instant(merge(1, 2))
    );
}

// F7. Section 4.2: which survives.
#[test]
fn the_home_region_survives_then_the_region_with_more_players_then_the_lower() {
    // The home region with one player against a region with five.
    let mut world = World::rested(follow(), 3, &["a"]);
    world.put(0, &[(0, 0, 1)]);
    world.put(1, &[(2, 0, 5)]);
    assert_eq!(world.next_begun(2 * FRESH).1, [merge(0, 1)]);
    // The home region without any, for the chunk players enter in.
    let mut world = World::rested(follow(), 3, &["a"]);
    world.put(2, &[(-2, 2, 3)]);
    assert_eq!(world.next_begun(2 * FRESH).1, [merge(0, 2)]);
    // Of two others the one with more players.
    let mut world = World::rested(follow(), 3, &["a"]);
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(101, 1, 1), (103, 0, 1)]);
    assert_eq!(world.next_begun(2 * FRESH).1, [merge(2, 1)]);
    // Of two with as many the lower.
    let mut world = pair();
    assert_eq!(world.next_begun(2 * FRESH).1, [merge(1, 2)]);
}

// F7. Section 4.2: "The choice is made when the merge is begun, from the sightings
// of that moment."
#[test]
fn the_survivor_is_chosen_by_the_sightings_of_the_tick_that_begins_the_merge() {
    let mut world = pair();
    world.put(1, &[(100, 0, 3)]);
    world.quiet(FRESH);
    // In the last report before the merge has stood, region 2 has more.
    world.put(2, &[(102, 0, 4)]);
    world.quiet(LOOK);
    assert_eq!(world.step().begun, [merge(2, 1)]);
}

/// Five stripes of one worker that have just been given away, so that every region
/// rests for ten seconds. Region 1 has four players in the chunks 100 and 104 and
/// survives whatever it merges with. Region 2 has one player in the chunk `first`
/// from the first look on, and region 3 one in the chunk `second` from two looks
/// later; the players of region 4 are far away. Returns the world after region 3's
/// first look, and when the rest ends.
fn two_wait_for_one(first: i32, second: i32) -> (World, Instant) {
    let mut world = World::anew(follow(), 5);
    world.settle(&["a"]);
    let rests_until = world.now + REST;
    for id in 0..5 {
        assert_eq!(world.alone_until(id), Some(rests_until));
    }
    world.put(1, &[(100, 0, 2), (104, 0, 2)]);
    world.put(2, &[(first, 0, 1)]);
    world.put(3, &[(200, 0, 1)]);
    world.put(4, &[(300, 0, 1)]);
    world.quiet(2 * LOOK);
    world.put(3, &[(second, 0, 1)]);
    world.quiet(LOOK);
    (world, rests_until)
}

// F8. K1.
#[test]
fn the_merge_that_was_wanted_first_is_begun_first_and_the_other_after_the_rest_before_one_that_came_later()
 {
    // Region 2 is two chunks from region 1 and came first; region 3 is one chunk
    // from it and came later.
    let (mut world, rests_until) = two_wait_for_one(98, 105);
    assert_eq!(world.run_up_to(rests_until), []);
    assert_eq!(
        world.around_instant(rests_until),
        from_the_instant_on(merge(1, 2))
    );
    // While that merge lasts, region 4 comes nearer than either: into a chunk that
    // region 1 has players in.
    world.obedient = false;
    assert_eq!(world.tick_at(rests_until).begun, [merge(1, 2)]);
    world.put(4, &[(100, 0, 1)]);
    world.quiet(2 * LOOK);
    world.merge_through(2);
    let ended = world.now;
    world.obedient = true;
    assert_eq!(world.alone_until(1), Some(ended + REST));
    // The survivor rests; then the merge that has waited since before the first one
    // is begun, and not the nearer one.
    assert_eq!(world.run_up_to(ended + REST), []);
    assert_eq!(
        world.around_instant(ended + REST),
        from_the_instant_on(merge(1, 3))
    );
    // The one that came later has its turn a rest after that has ended, which is at
    // the workers' next look.
    assert_eq!(world.tick_at(ended + REST).begun, [merge(1, 3)]);
    assert_eq!(
        world.next_begun(REST + FRESH),
        (ended + REST + REST + LOOK, vec![merge(1, 4)])
    );
}

// F8. K1.
#[test]
fn of_two_merges_wanted_since_the_same_tick_the_nearer_is_begun_first_then_the_lower() {
    // Both from the first look: region 2 at two chunks, region 3 at one.
    let mut world = World::anew(follow(), 5);
    world.settle(&["a"]);
    let rests_until = world.now + REST;
    world.put(1, &[(100, 0, 2), (104, 0, 2)]);
    world.put(2, &[(98, 0, 1)]);
    world.put(3, &[(105, 0, 1)]);
    world.put(4, &[(300, 0, 1)]);
    assert_eq!(world.run_up_to(rests_until), []);
    assert_eq!(world.tick_at(rests_until).begun, [merge(1, 3)]);
    // The other after the rest, which begins when the workers have done the first.
    assert_eq!(
        world.next_begun(REST + FRESH),
        (rests_until + REST + LOOK, vec![merge(1, 2)])
    );

    // As near as each other: the one with the lower ids.
    let mut world = World::anew(follow(), 5);
    world.settle(&["a"]);
    let rests_until = world.now + REST;
    world.put(1, &[(100, 0, 2), (104, 0, 2)]);
    world.put(3, &[(98, 0, 1)]);
    world.put(2, &[(106, 0, 1)]);
    world.put(4, &[(300, 0, 1)]);
    assert_eq!(world.run_up_to(rests_until), []);
    assert_eq!(world.tick_at(rests_until).begun, [merge(1, 2)]);
}

// F9.
#[test]
fn a_merge_keeps_its_place_through_a_tick_at_which_it_is_not_wanted_while_both_regions_are_fresh() {
    // Region 2 came first and is further; region 3 came later and is nearer.
    let (mut world, rests_until) = two_wait_for_one(98, 105);
    world.quiet(4 * LOOK);
    // One report has region 2's player far off, and the next has them back.
    world.put(2, &[(50, 0, 1)]);
    world.quiet(LOOK);
    world.put(2, &[(98, 0, 1)]);
    assert_eq!(world.run_up_to(rests_until), []);
    assert_eq!(world.would_begin_at(rests_until), [merge(1, 2)]);
}

/// Region 2 came first and is nearer, region 3 came later and is further. Region 2's
/// player is far off at every tick from one to the tick `away` after it, with both
/// sightings fresh, and back at the tick a second and a look after the first of
/// those. Returns what is begun when the rest ends.
fn first_when_the_one_that_came_first_was_away_for(away: Duration) -> Vec<Begun> {
    let (mut world, rests_until) = two_wait_for_one(99, 106);
    world.quiet(4 * LOOK);
    world.put(2, &[(50, 0, 1)]);
    world.quiet(LOOK);
    let missed = world.now;
    world.quiet(3 * LOOK);
    assert_eq!(world.tick_at(missed + away).begun, []);
    world.put(2, &[(99, 0, 1)]);
    assert_eq!(world.tick_at(missed + FRESH + LOOK).begun, []);
    assert_eq!(world.run_up_to(rests_until), []);
    world.would_begin_at(rests_until)
}

// F9.
#[test]
fn a_merge_loses_its_place_when_it_has_not_been_wanted_for_more_than_a_second_with_both_regions_fresh()
 {
    assert_eq!(
        first_when_the_one_that_came_first_was_away_for(FRESH - MOMENT),
        [merge(1, 2)]
    );
    assert_eq!(
        first_when_the_one_that_came_first_was_away_for(FRESH),
        [merge(1, 2)]
    );
    // It is the nearer of the two, and behind the one that came later all the same.
    assert_eq!(
        first_when_the_one_that_came_first_was_away_for(FRESH + MOMENT),
        [merge(1, 3)]
    );
}

// F10.
#[test]
fn nothing_more_is_begun_for_a_pair_of_which_the_list_shows_one_absorbed_by_the_other() {
    for noted in [true, false] {
        let mut world = World::anew(follow(), 3);
        world.settle(&["a"]);
        world.put(1, &[(100, 0, 1)]);
        world.put(2, &[(102, 0, 1)]);
        // Their merge has stood and waits for the rest to end.
        world.quiet(2 * FRESH);
        if noted {
            // Somebody asks for that very merge, which is done at rest (K17).
            let changes = world
                .coordinator
                .merge(world.now, region(1), region(2), ASKER)
                .expect("a merge by hand is not held back by a rest");
            world.take("merge", changes);
            world.merge_through(2);
        } else {
            // The list has it, and the coordinator knows of no merge.
            world.merged(1, 2);
            world.hand_in();
        }
        assert_eq!(world.known(), [0, 1]);
        world.quiet(3 * REST);
    }
}

// F10.
#[test]
fn of_ten_merges_that_have_stood_four_are_begun_and_the_others_as_those_end() {
    let mut world = World::rested(follow(), 21, &["a"]);
    for pair in 0..10 {
        let x = 100 + 20 * pair as i32;
        world.put(2 * pair + 1, &[(x, 0, 1)]);
        world.put(2 * pair + 2, &[(x + 1, 0, 1)]);
    }
    world.obedient = false;
    world.quiet(FRESH + LOOK);
    assert_eq!(
        world.step().begun,
        [merge(1, 2), merge(3, 4), merge(5, 6), merge(7, 8)]
    );
    assert_eq!(world.under_way().len(), AT_ONCE);
    world.quiet(FRESH);
    world.merge_through(2);
    assert_eq!(world.step().begun, [merge(9, 10)]);
    world.quiet(FRESH);
    world.merge_through(4);
    world.merge_through(6);
    assert_eq!(world.step().begun, [merge(11, 12), merge(13, 14)]);
    assert_eq!(world.under_way().len(), AT_ONCE);
    // The workers do what is left, and the last three are begun together.
    world.obedient = true;
    assert_eq!(
        world.step().begun,
        [merge(15, 16), merge(17, 18), merge(19, 20)]
    );
    world.quiet(REST - LOOK);
    assert_eq!(world.known().len(), 11);
}

// ---------------------------------------------------------------------------------
// F11. Each thing that holds a merge back (sections 5.1 and 5.2), one test each.
// ---------------------------------------------------------------------------------

// F11. What every test of it rests on: without the thing, the merge is begun.
#[test]
fn a_merge_that_nothing_holds_back_is_begun_a_second_and_a_look_after_the_first_look() {
    let mut world = pair();
    world.quiet(LOOK);
    let first = world.now;
    assert_eq!(
        world.next_begun(2 * FRESH),
        (first + FRESH + LOOK, vec![merge(1, 2)])
    );
}

// F11: reserved.
#[test]
fn a_merge_with_a_region_that_is_reserved_is_not_begun() {
    let mut world = pair();
    world.quiet(LOOK);
    // Somebody asks for a split of region 2, and its worker takes three seconds to
    // find that nobody stands in the chunks named.
    world.obedient = false;
    let changes = world
        .coordinator
        .split(world.now, region(2), &[at(500)], ASKER)
        .expect("nothing speaks against the split");
    world.take("split", changes);
    assert_eq!(world.under_way(), [Asked::Split { region: region(2) }]);
    world.quiet(3 * FRESH);
    world.split_off(2, Off::Nobody);
    let ended = world.now;
    world.obedient = true;
    // That was "not yet", after which the region rests (section 5.5).
    assert_eq!(world.alone_until(2), Some(ended + REST));
    assert_eq!(world.run_up_to(ended + REST), []);
    assert_eq!(
        world.around_instant(ended + REST),
        from_the_instant_on(merge(1, 2))
    );
}

/// Three stripes, each with a worker of its own, and a fourth worker that runs
/// nothing; rested, with the players of [`pair`].
fn pair_of_three_workers() -> World {
    let mut world = World::running(
        follow(),
        3,
        &[("a", &[0]), ("b", &[1]), ("c", &[2]), ("d", &[])],
    );
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(102, 0, 1)]);
    world
}

// F11: being released. K3.
#[test]
fn a_merge_with_a_region_that_is_being_moved_waits_for_the_move_and_for_the_rest_of_its_new_owner()
{
    let mut world = pair_of_three_workers();
    world.quiet(LOOK);
    world.obedient = false;
    let (_, changes) = world
        .coordinator
        .move_region(world.now, region(2), Some("d"), 7)
        .expect("the region can be moved");
    world.take("move_region", changes);
    world.quiet(3 * FRESH);
    world.let_go(2);
    let given = world.now;
    world.obedient = true;
    assert_eq!(world.owner(2).as_deref(), Some("d"));
    assert_eq!(world.alone_until(2), Some(given + REST));
    assert_eq!(world.run_up_to(given + REST), []);
    assert_eq!(
        world.around_instant(given + REST),
        from_the_instant_on(merge(1, 2))
    );
}

// F11: being released. K3: "If the release is not answered in a lease, the region is
// taken and assigned as today, and rests from then."
#[test]
fn a_merge_with_a_region_whose_release_is_not_answered_waits_for_the_rest_of_whoever_is_given_it() {
    let mut world = pair_of_three_workers();
    world.quiet(LOOK);
    world.obedient = false;
    let (_, changes) = world
        .coordinator
        .move_region(world.now, region(2), Some("d"), 7)
        .expect("the region can be moved");
    world.take("move_region", changes);
    let asked = world.now;
    assert_eq!(world.run_to(asked + LEASE), []);
    let given = world.until_given(2, FRESH);
    world.releases.clear();
    world.obedient = true;
    assert_eq!(world.alone_until(2), Some(given + REST));
    assert_eq!(world.run_up_to(given + REST), []);
    assert_eq!(
        world.around_instant(given + REST),
        from_the_instant_on(merge(1, 2))
    );
}

// F11: no owner. K7, K23: the sightings stay meanwhile, and nothing is begun with
// either region before its new owner has reported it and it has rested.
#[test]
fn a_merge_of_regions_without_an_owner_is_not_begun_before_they_have_one_that_has_rested() {
    let mut world = pair();
    world.quiet(LOOK);
    world.silence("a");
    let changes = world.coordinator.disconnected(world.now, "a");
    world.take("disconnected", changes);
    world.quiet(2 * LEASE);
    assert_eq!(world.table().routes, []);
    world.register("b", &[]);
    if world.owner(1).is_none() {
        world.quiet(LOOK);
    }
    let given = world.now;
    assert_eq!(world.owner(1).as_deref(), Some("b"));
    assert_eq!(world.owner(2).as_deref(), Some("b"));
    assert_eq!(world.alone_until(1), Some(given + REST));
    assert_eq!(world.alone_until(2), Some(given + REST));
    assert_eq!(world.run_up_to(given + REST), []);
    assert_eq!(
        world.around_instant(given + REST),
        from_the_instant_on(merge(1, 2))
    );
}

// F11: an owner without a connection.
#[test]
fn a_merge_with_a_region_whose_owner_has_no_connection_is_not_begun() {
    let mut world = World::running(follow(), 3, &[("a", &[0, 1]), ("b", &[2])]);
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(102, 0, 1)]);
    world.quiet(FRESH + LOOK);
    // Both report once more, so both sightings are fresh at the look at which the
    // merge has stood; then `b` loses its connection.
    world.now += LAG;
    world.report();
    let mut connected = world.clone();
    let changes = world.coordinator.disconnected(world.now, "b");
    world.take("disconnected", changes);
    world.silence("b");
    world.now += LOOK - LAG;
    assert_eq!(world.tick().begun, []);
    connected.now += LOOK - LAG;
    assert_eq!(connected.tick().begun, [merge(1, 2)]);
    // When its lease is out the region goes to the other worker, and rests.
    let given = world.until_given(2, LEASE + FRESH);
    assert_eq!(world.owner(2).as_deref(), Some("a"));
    assert_eq!(world.run_up_to(given + REST), []);
    assert_eq!(
        world.around_instant(given + REST),
        from_the_instant_on(merge(1, 2))
    );
}

// F11: an owner that leaves. K16. With nobody to take its regions they stay its, and
// only its leaving holds the merge back.
#[test]
fn a_merge_of_regions_whose_worker_leaves_is_not_begun() {
    let mut world = pair();
    world.quiet(FRESH + LOOK);
    let mut stays = world.clone();
    let changes = world.coordinator.leaving(world.now, "a");
    let changes = world.take("leaving", changes);
    assert_eq!(changes.releases, []);
    assert_eq!(world.step().begun, []);
    assert_eq!(stays.step().begun, [merge(1, 2)]);
    world.quiet(2 * REST);
}

/// The worker of one of the two regions says that it leaves, at the look before the
/// one at which their merge has stood. Its regions are released at once, as there is
/// a worker to take them (K16), and the merge is begun when they have rested with it.
fn the_merge_waits_for_the_regions_of_the_leaver(leaver: &str, stays: &str, its: &[u32]) {
    let mut world = World::running(follow(), 3, &[("a", &[0, 1]), ("b", &[2])]);
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(102, 0, 1)]);
    world.quiet(FRESH + LOOK);
    let changes = world.coordinator.leaving(world.now, leaver);
    let changes = world.take("leaving", changes);
    let released: Vec<u32> = changes
        .releases
        .iter()
        .map(|release| release.region.0)
        .collect();
    assert_eq!(released, its);
    // The look at which the merge would have been begun.
    world.obedient = false;
    assert_eq!(world.step().begun, []);
    world.obedient = true;
    world.now += LAG;
    world.obey();
    let given = world.now;
    world.silence(leaver);
    for id in 0..3 {
        assert_eq!(world.owner(id).as_deref(), Some(stays));
    }
    for id in its {
        assert_eq!(world.alone_until(*id), Some(given + REST));
    }
    world.report();
    world.now += LOOK - LAG;
    assert_eq!(world.tick().begun, []);
    assert_eq!(world.run_up_to(given + REST), []);
    assert_eq!(
        world.around_instant(given + REST),
        from_the_instant_on(merge(1, 2))
    );
}

// F11: either owner leaving.
#[test]
fn a_merge_whose_survivor_is_run_by_a_worker_that_leaves_waits_until_it_has_been_moved_and_rested()
{
    the_merge_waits_for_the_regions_of_the_leaver("a", "b", &[0, 1]);
}

// F11: either owner leaving.
#[test]
fn a_merge_whose_other_region_is_run_by_a_worker_that_leaves_waits_until_it_has_been_moved_and_rested()
 {
    the_merge_waits_for_the_regions_of_the_leaver("b", "a", &[2]);
}

/// `b` runs one of the two regions and region 3, which it stops vouching for: the
/// region is taken from it, and it is at fault for six leases. The merge, which has
/// stood all the while, is begun when that is forgotten.
fn the_merge_waits_until_the_fault_is_forgotten(of_a: u32, of_b: u32) {
    let mut world = World::running(follow(), 4, &[("a", &[0, of_a]), ("b", &[of_b, 3])]);
    world.unvouched.insert(3);
    let failed = world.until_given(3, LEASE + FRESH);
    assert_eq!(world.owner(3).as_deref(), Some("a"));
    world.unvouched.clear();
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(102, 0, 1)]);
    let forgotten = failed + Coordinator::FAULT_MEMORY * LEASE;
    assert_eq!(world.run_to(forgotten - FRESH), []);
    // Nothing is evened out at the tick that begins it, although `a` has two regions
    // more than `b` from then on.
    let (when, begun) = world.next_begun(2 * FRESH);
    assert_eq!(begun, [merge(1, 2)]);
    assert!(when <= forgotten + LOOK, "{}", world.story());
}

// F11: either owner at fault.
#[test]
fn a_merge_whose_survivor_is_run_by_a_worker_at_fault_is_not_begun_until_the_fault_is_forgotten() {
    the_merge_waits_until_the_fault_is_forgotten(2, 1);
}

// F11: either owner at fault.
#[test]
fn a_merge_whose_other_region_is_run_by_a_worker_at_fault_is_not_begun_until_the_fault_is_forgotten()
 {
    the_merge_waits_until_the_fault_is_forgotten(1, 2);
}

// F11: at rest.
#[test]
fn a_merge_is_not_begun_while_its_regions_rest_and_is_when_the_rest_is_over() {
    let mut world = World::anew(follow(), 3);
    world.settle(&["a"]);
    let rests_until = world.now + REST;
    assert_eq!(world.alone_until(1), Some(rests_until));
    assert_eq!(world.alone_until(2), Some(rests_until));
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(102, 0, 1)]);
    assert_eq!(world.run_up_to(rests_until), []);
    assert_eq!(
        world.around_instant(rests_until),
        from_the_instant_on(merge(1, 2))
    );
}

/// The merge of [`pair`] has stood at a tick a second and a moment after the first
/// that wanted it. Region 2 was last reported `age` before that tick. Returns what
/// the tick begins.
fn begun_with_a_sighting_of_the_age(age: Duration) -> Vec<Begun> {
    let mut world = pair();
    world.quiet(LOOK);
    let first = world.now;
    let stood = first + FRESH + MOMENT;
    world.now = stood - age;
    world.report_of(&[2]);
    world.mute.insert(2);
    for look in 1..=3 {
        assert_eq!(world.tick_at(first + look * LOOK).begun, []);
    }
    assert_eq!(world.tick_at(first + 900 * MOMENT).begun, []);
    world.tick_at(stood).begun
}

// F11: a sighting more than `FRESH` old.
#[test]
fn a_merge_is_not_begun_with_a_region_whose_sighting_is_more_than_a_second_old() {
    assert_eq!(
        begun_with_a_sighting_of_the_age(FRESH - MOMENT),
        [merge(1, 2)]
    );
    assert_eq!(begun_with_a_sighting_of_the_age(FRESH), [merge(1, 2)]);
    assert_eq!(begun_with_a_sighting_of_the_age(FRESH + MOMENT), []);
}

// F11: the grace period.
#[test]
fn a_merge_is_not_begun_in_the_grace_period_of_a_new_coordinator() {
    // A rest of one second, so that it is the grace period that holds the merge back
    // and not the rest that begins when a worker reports a region (K6).
    let short = Policy {
        rest: Duration::from_secs(1),
        ..follow().expect("a policy")
    };
    let mut world = World::reported(Some(short), 3, &[("a", &[0, 1, 2])]);
    assert_eq!(
        world.alone_until(1),
        Some(world.made + Duration::from_secs(1))
    );
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(102, 0, 1)]);
    let over = world.made + LEASE;
    assert_eq!(world.run_up_to(over), []);
    let [before, at_it, after] = world.around_instant(over);
    assert_eq!(before, []);
    assert_eq!(after, [merge(1, 2)]);
    // Whether the grace period is over at the very instant a lease has passed, the
    // record does not say. It is the one in which no region is given away, so a
    // region without an owner tells: what is begun then goes with that.
    let mut twin = World::reported(None, 3, &[("a", &[0, 1])]);
    twin.now = twin.made + LEASE;
    twin.tick();
    let over_at_it = twin.owner(2).is_some();
    assert_eq!(!at_it.is_empty(), over_at_it);
}

// F11: the list never read.
#[test]
fn nothing_is_begun_before_the_list_has_been_read() {
    let mut world = World::anew(follow(), 3);
    world.readings = Readings::Failing;
    world.settle(&["a"]);
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(102, 0, 1)]);
    world.quiet(2 * REST);
    assert!(world.reads >= 4);
    assert_eq!(world.table().home, None);
    // The next reading succeeds.
    world.readings = Readings::AtOnce;
    while world.listed.is_none() {
        assert_eq!(world.step().begun, []);
    }
    let read = world.now;
    // Which region is home is known from the tick after that; nothing is wanted
    // before (section 4.2), so the merge has stood more than a second after that tick.
    assert_eq!(
        world.next_begun(2 * FRESH),
        (read + LOOK + FRESH + LOOK, vec![merge(1, 2)])
    );
}

// F11: the list never read.
#[test]
fn nothing_is_begun_while_the_first_reading_of_the_list_is_not_answered() {
    let mut world = World::anew(follow(), 3);
    world.readings = Readings::Held;
    world.settle(&["a"]);
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(102, 0, 1)]);
    world.quiet(3 * REST);
    assert_eq!(world.reads, 1);
}

/// The world of [`pair`] in which every reading fails from now on, and the merge is
/// first wanted `wanted` after the last reading that succeeded. Returns it after
/// that look, and when that reading was.
fn pair_wanted_after_the_last_good_reading(wanted: Duration) -> (World, Instant) {
    let mut world = World::rested(follow(), 3, &["a"]);
    let read = world.listed.expect("the list has been read");
    world.readings = Readings::Failing;
    assert_eq!(world.run_to(read + wanted - LOOK), []);
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(102, 0, 1)]);
    world.quiet(LOOK);
    assert_eq!(world.now, read + wanted);
    (world, read)
}

// F11: the last good reading more than two `LIST_EVERY` old.
#[test]
fn a_merge_is_begun_when_the_last_good_reading_is_exactly_two_leases_old_and_not_when_it_is_older()
{
    // It has stood for the first time at the tick two leases after the reading.
    let (mut world, read) = pair_wanted_after_the_last_good_reading(2 * LEASE - FRESH - LOOK);
    let reads = world.reads;
    world.quiet(FRESH);
    assert_eq!(world.reads, reads);
    assert_eq!(world.now, read + 2 * LEASE - LOOK);
    let [before, at_it, after] = world.around_instant(read + 2 * LEASE);
    assert_eq!(before, [merge(1, 2)]);
    assert_eq!(at_it, [merge(1, 2)]);
    assert_eq!(after, []);
    // That took two readings that failed: one a lease after the good one, and one a
    // lease after that, which is asked for at this very tick.
    assert_eq!(world.step().begun, [merge(1, 2)]);
    assert_eq!(world.reads, reads + 1);
}

// F11: the last good reading more than two `LIST_EVERY` old.
#[test]
fn a_merge_that_has_stood_later_than_two_leases_after_the_last_good_reading_waits_for_the_next() {
    // It has stood for the first time a look after that.
    let (mut world, read) = pair_wanted_after_the_last_good_reading(2 * LEASE - FRESH);
    assert_eq!(world.run_to(read + 3 * LEASE - LOOK), []);
    // The third reading is asked for a lease after the second failed, and succeeds.
    world.readings = Readings::AtOnce;
    assert_eq!(world.step().begun, []);
    assert_eq!(world.listed, Some(read + 3 * LEASE));
    assert_eq!(world.step().begun, [merge(1, 2)]);
}

// F11: "One reading that failed holds nothing back if every reading is answered at
// the tick that asks for it".
#[test]
fn one_reading_that_fails_at_the_tick_that_asks_for_it_holds_nothing_back() {
    let (mut world, read) = pair_wanted_after_the_last_good_reading(LEASE + FRESH);
    assert_eq!(
        world.next_begun(2 * FRESH),
        (read + LEASE + 2 * FRESH + LOOK, vec![merge(1, 2)])
    );
}

// F11: a region the coordinator knows that has no sighting, wherever it is.
#[test]
fn nothing_is_begun_while_a_region_has_never_been_reported() {
    let mut world = World::anew(follow(), 4);
    world.mute.insert(3);
    world.settle(&["a"]);
    world.rest();
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(102, 0, 1)]);
    world.quiet(REST);
    // Its first report, of players far from everybody: the merge, which has stood
    // for seconds, is begun at the next tick.
    world.mute.clear();
    world.put(3, &[(1000, 1000, 2)]);
    assert_eq!(world.step().begun, [merge(1, 2)]);
}

// F11: readings that are answered late (section 7).
#[test]
fn a_merge_that_has_stood_is_not_begun_between_two_leases_after_the_last_good_reading_and_a_late_answer()
 {
    let mut world = World::rested(follow(), 3, &["a"]);
    let read = world.listed.expect("the list has been read");
    let reads = world.reads;
    let seconds = Duration::from_secs;
    world.readings = Readings::Held;
    // The next reading is asked for a lease after the last answer, and fails a second
    // after it was asked for.
    assert_eq!(world.run_to(read + LEASE - LOOK), []);
    assert_eq!(world.reads, reads);
    world.quiet(LOOK);
    assert_eq!(world.reads, reads + 1);
    assert_eq!(world.run_to(read + seconds(6)), []);
    assert!(world.unanswered);
    world.fail_reading();
    // The merge is first wanted nine and a quarter seconds after the good reading
    // and has stood ten and a half seconds after it: later than two leases.
    assert_eq!(world.run_to(read + seconds(9)), []);
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(102, 0, 1)]);
    // The one after is asked for a lease after the failure, and succeeds a second
    // after it was asked for.
    assert_eq!(world.run_to(read + seconds(11) - LOOK), []);
    assert_eq!(world.reads, reads + 1);
    world.quiet(LOOK);
    assert_eq!(world.reads, reads + 2);
    assert_eq!(world.run_to(read + seconds(12)), []);
    assert!(world.unanswered);
    world.hand_in();
    // The merge is begun at the first tick after that answer.
    assert_eq!(world.step().begun, [merge(1, 2)]);
}

// F11: readings that are answered late. The other half: the same merge with every
// reading answered at once is begun when it has stood.
#[test]
fn the_same_merge_is_begun_when_it_has_stood_if_the_readings_are_answered_at_once() {
    let mut world = World::rested(follow(), 3, &["a"]);
    let read = world.listed.expect("the list has been read");
    let seconds = Duration::from_secs;
    assert_eq!(world.run_to(read + seconds(9)), []);
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(102, 0, 1)]);
    assert_eq!(
        world.next_begun(2 * FRESH),
        (read + seconds(10) + 2 * LOOK, vec![merge(1, 2)])
    );
}

// ---------------------------------------------------------------------------------
// F12 to F15. Splitting (sections 4.3, 5.3 and 5.6).
// ---------------------------------------------------------------------------------

/// Three stripes of one worker, rested. Region 1 has two players in chunk 100 and one
/// in chunk 106, which is more than the split distance from them; region 2's are far
/// from both. Nothing of it has been reported yet.
fn apart() -> World {
    let mut world = World::rested(follow(), 3, &["a"]);
    world.put(1, &[(100, 0, 2), (106, 0, 1)]);
    world.put(2, &[(300, 0, 1)]);
    world
}

/// What was begun, without the orders to prepare.
fn but_for_prepare(began: Vec<Begun>) -> Vec<Begun> {
    began
        .into_iter()
        .filter(|begun| !matches!(begun, Begun::Prepare(_)))
        .collect()
}

// F12.
#[test]
fn a_free_region_is_told_to_prepare_at_the_first_look_that_wants_a_split_and_to_split_when_the_group_has_stood()
 {
    let mut world = apart();
    let epoch = world.epoch(1);
    let look = world.step();
    let first = world.now;
    assert_eq!(look.begun, [Begun::Prepare(1)]);
    assert_eq!(
        look.changes.orders,
        [ReshapeOrder {
            worker: "a".to_owned(),
            order: Order::Prepare {
                region: region(1),
                epoch,
            },
        }]
    );
    assert_eq!(look.changes.releases, []);
    assert_eq!(world.under_way(), []);
    world.quiet(3 * LOOK);
    assert_eq!(
        world.around_instant(first + FRESH),
        after_the_instant(split(1, 3, &[&[at(106)]]))
    );
    world.quiet(LOOK);
    let highest = world.highest_epoch();
    let look = world.step();
    assert_eq!(world.now, first + FRESH + LOOK);
    // The chunks of D5: the group with fewer players goes, and what is named is the
    // square of the margin around its chunk, ascending, each chunk once. The part is
    // to have the next id of the list.
    let square: Vec<ChunkPos> = (104..=108)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect();
    let as_epoch = match look.changes.orders.as_slice() {
        [
            ReshapeOrder {
                worker,
                order:
                    Order::SplitOff {
                        region: of,
                        epoch: its,
                        chunks,
                        as_epoch,
                        part,
                    },
            },
        ] if worker == "a"
            && *of == region(1)
            && *its == epoch
            && *chunks == square
            && *part == world.list.next =>
        {
            *as_epoch
        }
        other => panic!("expected the order to split region 1: {other:?}"),
    };
    assert!(as_epoch > highest);
    assert_eq!(look.changes.releases, []);
    assert_eq!(world.under_way(), [Asked::Split { region: region(1) }]);
    // It ends as a split that somebody asked for does, with nobody as asker.
    let (part, changes) = world.split_through(1);
    assert_eq!(part, 3);
    assert_eq!(changes.reshaped, [split_ended(1, Ok(3))]);
    assert_eq!(world.owner(3).as_deref(), Some("a"));
    assert_eq!(world.epoch(3), as_epoch);
    assert_eq!(world.under_way(), []);
}

// F12. Section 4.3: which group stays.
#[test]
fn the_group_with_the_most_players_stays_and_of_two_with_as_many_the_one_with_the_lowest_chunk() {
    let mut world = World::rested(follow(), 2, &["a"]);
    world.put(1, &[(100, 0, 1), (106, 0, 2)]);
    assert_eq!(
        but_for_prepare(world.run(2 * FRESH)),
        [split(1, 2, &[&[at(100)]])]
    );
    let mut world = World::rested(follow(), 2, &["a"]);
    world.put(1, &[(100, 0, 1), (106, 0, 1)]);
    assert_eq!(
        but_for_prepare(world.run(2 * FRESH)),
        [split(1, 2, &[&[at(106)]])]
    );
}

// F12. Section 4.3 and D6: in the home region the group with the chunk players enter
// in stays, whoever is there.
#[test]
fn in_the_home_region_whoever_is_far_from_the_chunk_players_enter_in_goes() {
    // It has one player at the origin and five far off.
    let mut world = World::rested(follow(), 2, &["a"]);
    world.put(0, &[(0, 0, 1), (6, 0, 5)]);
    world.put(1, &[(300, 0, 1)]);
    assert_eq!(
        but_for_prepare(world.run(2 * FRESH)),
        [split(0, 2, &[&[at(6)]])]
    );
    // It has nobody at the origin: its only players are six chunks from it.
    let mut world = World::rested(follow(), 2, &["a"]);
    world.put(0, &[(0, 6, 3)]);
    world.put(1, &[(300, 0, 1)]);
    assert_eq!(
        but_for_prepare(world.run(2 * FRESH)),
        [split(0, 2, &[&[ChunkPos::new(0, 6)]])]
    );
    // Five chunks from it they are the home region's own.
    let mut world = World::rested(follow(), 2, &["a"]);
    world.put(0, &[(0, 5, 3)]);
    world.put(1, &[(300, 0, 1)]);
    world.quiet(REST);
}

/// Three stripes of one worker, rested, in which two regions are to be split: each
/// has two players in one chunk and one six chunks from them.
fn two_apart() -> World {
    let mut world = World::rested(follow(), 3, &["a"]);
    world.put(1, &[(100, 0, 2), (106, 0, 1)]);
    world.put(2, &[(200, 0, 2), (206, 0, 1)]);
    world
}

// F13.
#[test]
fn one_split_is_under_way_in_the_world_at_a_time() {
    let mut world = two_apart();
    world.obedient = false;
    let look = world.step();
    assert_eq!(look.begun, [Begun::Prepare(1), Begun::Prepare(2)]);
    world.quiet(FRESH);
    assert_eq!(world.step().begun, [split(1, 3, &[&[at(106)]])]);
    // Both groups have stood since the same tick, and region 2 is free.
    world.quiet(3 * FRESH);
    let (part, _) = world.split_through(1);
    assert_eq!(part, 3);
    assert_eq!(world.step().begun, [split(2, 4, &[&[at(206)]])]);
}

// F13.
#[test]
fn no_split_is_begun_beside_one_that_somebody_asked_for() {
    let mut world = apart();
    world.obedient = false;
    let changes = world
        .coordinator
        .split(world.now, region(2), &[at(500)], ASKER)
        .expect("nothing speaks against the split");
    world.take("split", changes);
    // How many are under way does not come into whether a region is told to prepare
    // (section 5.6).
    assert_eq!(world.step().begun, [Begun::Prepare(1)]);
    world.quiet(3 * FRESH);
    world.split_off(2, Off::Nobody);
    assert_eq!(world.step().begun, [split(1, 3, &[&[at(106)]])]);
}

// F14.
#[test]
fn no_split_is_begun_while_a_reading_of_the_list_is_asked_for() {
    let mut world = World::rested(follow(), 4, &["a"]);
    let read = world.listed.expect("the list has been read");
    world.readings = Readings::Held;
    // A split and a merge are wanted from the same look, a second before the tick
    // that asks for the next reading; both have stood a look after that tick.
    assert_eq!(world.run_to(read + LEASE - FRESH - LOOK), []);
    world.put(1, &[(100, 0, 2), (106, 0, 1)]);
    world.put(2, &[(200, 0, 1)]);
    world.put(3, &[(202, 0, 1)]);
    assert_eq!(world.step().begun, [Begun::Prepare(1)]);
    world.quiet(FRESH);
    assert_eq!(world.now, read + LEASE);
    assert!(world.unanswered);
    // The merge is not held back by the reading, and the split is.
    assert_eq!(world.step().begun, [merge(2, 3)]);
    world.quiet(2 * FRESH);
    world.hand_in();
    assert_eq!(world.step().begun, [split(1, 4, &[&[at(106)]])]);
}

// F14.
#[test]
fn no_split_is_begun_while_a_reading_is_owed_after_a_split_that_ended_without_a_part() {
    let mut world = two_apart();
    world.obedient = false;
    world.quiet_but_for_prepare(FRESH + LOOK);
    assert_eq!(world.step().begun, [split(1, 3, &[&[at(106)]])]);
    // The worker lost the store on the way, and the store is still away.
    world.readings = Readings::Failing;
    let reads = world.reads;
    world.split_off(1, Off::StoreLost);
    assert_eq!(world.reads, reads + 1);
    // The list is asked for at every tick until it has been read, as the store may
    // have made a part that nobody runs; region 2 is free and is not split meanwhile.
    for asked in 2..=9 {
        assert_eq!(world.step().begun, []);
        assert_eq!(world.reads, reads + asked);
    }
    world.readings = Readings::AtOnce;
    assert_eq!(world.step().begun, []);
    assert_eq!(world.step().begun, [split(2, 3, &[&[at(206)]])]);
}

// F14.
#[test]
fn after_a_split_whose_reading_failed_the_next_split_is_begun_and_names_the_id_last_read() {
    let mut world = two_apart();
    world.obedient = false;
    world.quiet_but_for_prepare(FRESH + LOOK);
    assert_eq!(world.step().begun, [split(1, 3, &[&[at(106)]])]);
    world.readings = Readings::Failing;
    let (part, changes) = world.split_through(1);
    assert_eq!(part, 3);
    assert_eq!(changes.reshaped, [split_ended(1, Ok(3))]);
    // Nothing is owed: the worker said which region the split made. The next split
    // names region 3 once more, which is the next id of the last reading; the runner
    // is to try again with the id the store says (section 5.3).
    assert_eq!(world.step().begun, [split(2, 3, &[&[at(206)]])]);
    world.readings = Readings::AtOnce;
    let (part, _) = world.split_through(2);
    assert_eq!(part, 4);
    assert_eq!(world.known(), [0, 1, 2, 3, 4]);
}

// F15. K12.
#[test]
fn a_region_whose_groups_a_sighting_that_is_not_fresh_joins_is_not_split_until_that_region_reports_its_players_elsewhere()
 {
    let mut world = World::rested(follow(), 3, &["a"]);
    // Two groups nine apart, and two players of region 2 between them: each within
    // the merge distance of one group, and the split distance from each other.
    world.put(1, &[(100, 0, 2), (109, 0, 1)]);
    world.put(2, &[(102, 0, 1), (107, 0, 1)]);
    // At the one look that has region 2 fresh, all are one cluster.
    assert_eq!(world.step().begun, []);
    world.mute.insert(2);
    world.quiet(2 * REST);
    // Region 2 reports again, and its players are elsewhere.
    world.mute.clear();
    world.put(2, &[(300, 0, 2)]);
    assert_eq!(world.step().begun, [Begun::Prepare(1)]);
    let first = world.now;
    assert_eq!(
        world.next_begun(2 * FRESH),
        (first + FRESH + LOOK, vec![split(1, 3, &[&[at(109)]])])
    );
}

// F15. The other half (D15): while region 2's sighting is fresh, the two are merged.
#[test]
fn a_region_whose_groups_a_fresh_region_joins_is_merged_with_it_and_not_split() {
    let mut world = World::rested(follow(), 3, &["a"]);
    world.put(1, &[(100, 0, 2), (109, 0, 1)]);
    world.put(2, &[(102, 0, 1), (107, 0, 1)]);
    world.quiet(LOOK);
    let first = world.now;
    assert_eq!(
        world.next_begun(2 * FRESH),
        (first + FRESH + LOOK, vec![merge(1, 2)])
    );
    world.quiet(2 * REST);
}

// ---------------------------------------------------------------------------------
// F16 to F20. Groups (sections 4.3 and 5.3).
// ---------------------------------------------------------------------------------

// F16.
#[test]
fn one_split_takes_every_group_that_has_stood_and_the_part_is_split_further_when_it_has_rested() {
    let mut world = World::rested(follow(), 2, &["a"]);
    world.put(1, &[(100, 0, 3), (110, 0, 1), (120, 0, 1)]);
    assert_eq!(world.step().begun, [Begun::Prepare(1)]);
    let first = world.now;
    let (begun, both) = world.next_begun(2 * FRESH);
    assert_eq!(begun, first + FRESH + LOOK);
    assert_eq!(both, [split(1, 2, &[&[at(110)], &[at(120)]])]);
    let Begun::Split { chunks, .. } = &both[0] else {
        unreachable!("it is a split");
    };
    assert_eq!(chunks.len(), 50);
    // The worker makes it at its next look, and the part has both groups.
    world.quiet(LOOK);
    let made = begun + LAG;
    assert_eq!(world.known(), [0, 1, 2]);
    assert_eq!(
        world.crowds[&2].keys().copied().collect::<Vec<_>>(),
        [at(110), at(120)]
    );
    assert_eq!(world.alone_until(2), Some(made + REST));
    // The part is told to prepare within a second of the end of its rest, and split
    // when it has rested: one group stays, the one with the lowest chunk, and the
    // other goes. Nothing is begun with the first region.
    assert_eq!(world.run_up_to(made + REST - FRESH), []);
    assert_eq!(world.step().begun, [Begun::Prepare(2)]);
    assert_eq!(world.run_up_to(made + REST), []);
    assert_eq!(
        world.around_instant(made + REST),
        from_the_instant_on(split(2, 3, &[&[at(120)]]))
    );
    assert_eq!(world.step().begun, [split(2, 3, &[&[at(120)]])]);
    world.quiet(2 * REST);
    assert_eq!(world.known(), [0, 1, 2, 3]);
}

// F16. K9: however many groups there are.
#[test]
fn six_groups_that_part_from_a_region_within_a_rest_go_in_one_split() {
    let mut world = World::anew(follow(), 2);
    world.settle(&["a"]);
    let rests_until = world.now + REST;
    world.put(1, &[(100, 0, 9)]);
    let far: Vec<ChunkPos> = (1..=6).map(|group| at(100 + 10 * group)).collect();
    for (group, chunk) in far.iter().enumerate() {
        world.quiet(2 * LOOK);
        world.join(1, chunk.x, 1 + group as u32);
    }
    assert_eq!(
        but_for_prepare(world.run_up_to(rests_until)),
        [],
        "{}",
        world.story()
    );
    let groups: Vec<&[ChunkPos]> = far.iter().map(std::slice::from_ref).collect();
    assert_eq!(world.tick_at(rests_until).begun, [split(1, 2, &groups)]);
    // The part has six groups: its largest stays and five go on, a rest later.
    let began = but_for_prepare(world.run(REST + FRESH));
    assert_eq!(began, [split(2, 3, &groups[..5])]);
}

/// Region 1 rests, and a group of it in chunk 110 has stood for seconds. A second
/// group, larger than that one, is first seen in chunk 120 at the tick `before` the
/// end of the rest. Returns the world at the last tick before the end of the rest,
/// and that end.
fn a_second_group_appears(before: Duration) -> (World, Instant) {
    let mut world = World::anew(follow(), 2);
    world.settle(&["a"]);
    let rests_until = world.now + REST;
    world.put(1, &[(100, 0, 5), (110, 0, 1)]);
    world.run_up_to(rests_until - FRESH - LOOK);
    world.join(1, 120, 2);
    let seen = world.tick_at(rests_until - before).begun;
    assert_eq!(but_for_prepare(seen), []);
    for look in [3, 2, 1] {
        let began = world.tick_at(rests_until - look * LOOK).begun;
        assert_eq!(but_for_prepare(began), []);
    }
    (world, rests_until)
}

// F17. K22.
#[test]
fn a_group_that_appeared_a_second_or_less_before_the_split_is_not_named_with_one_that_has_stood() {
    for before in [FRESH - MOMENT, FRESH] {
        let (mut world, rests_until) = a_second_group_appears(before);
        assert_eq!(
            world.tick_at(rests_until).begun,
            [split(1, 2, &[&[at(110)]])]
        );
    }
    // More than a second before, it has stood by itself and goes with the other.
    let (mut world, rests_until) = a_second_group_appears(FRESH + MOMENT);
    assert_eq!(
        world.tick_at(rests_until).begun,
        [split(1, 2, &[&[at(110)], &[at(120)]])]
    );
}

// F17. K22.
#[test]
fn a_group_that_was_not_named_stays_in_the_regions_sighting_and_goes_after_the_rest() {
    let (mut world, rests_until) = a_second_group_appears(FRESH);
    assert_eq!(
        world.tick_at(rests_until).begun,
        [split(1, 2, &[&[at(110)]])]
    );
    world.quiet_but_for_prepare(LOOK);
    let made = rests_until + LAG;
    assert_eq!(
        world.crowds[&1].keys().copied().collect::<Vec<_>>(),
        [at(100), at(120)]
    );
    assert_eq!(world.alone_until(1), Some(made + REST));
    assert_eq!(but_for_prepare(world.run_up_to(made + REST)), []);
    assert_eq!(
        world.around_instant(made + REST),
        from_the_instant_on(split(1, 3, &[&[at(120)]]))
    );
}

/// A group of region 1 that moves `by` chunks at every tick, away from the others.
/// Returns what each of twenty looks begins.
fn a_group_in_flight(by: i32) -> Vec<Vec<Begun>> {
    let mut world = World::rested(follow(), 2, &["a"]);
    world.obedient = false;
    (0..20)
        .map(|look| {
            world.put(1, &[(100, 0, 3), (110 + by * look, 0, 1)]);
            world.step().begun
        })
        .collect()
}

// F18.
#[test]
fn a_group_that_moves_a_chunk_at_every_tick_keeps_its_time_and_is_split_off_when_it_has_stood() {
    for by in [1, 2] {
        let began = a_group_in_flight(by);
        assert_eq!(began[0], [Begun::Prepare(1)]);
        for begun in &began[1..5] {
            assert_eq!(*begun, []);
        }
        // The chunks are named from the report of the tick that begins the split.
        assert_eq!(began[5], [split(1, 2, &[&[at(110 + by * 5)]])]);
    }
    // One that moves further than the margin at every tick is a new group each time.
    let began = a_group_in_flight(3);
    assert_eq!(began[0], [Begun::Prepare(1)]);
    for begun in &began[1..] {
        assert_eq!(*begun, []);
    }
}

// F19.
#[test]
fn a_group_that_is_joined_from_further_than_the_margin_begins_its_second_anew() {
    let mut world = World::rested(follow(), 2, &["a"]);
    world.put(1, &[(100, 0, 5), (110, 0, 1)]);
    assert_eq!(world.step().begun, [Begun::Prepare(1)]);
    world.quiet(FRESH);
    // In the report of the tick at which it has stood, a player is of it who stands
    // three chunks from where it was: nothing of it is named at that tick.
    world.join(1, 113, 1);
    assert_eq!(world.step().begun, []);
    let anew = world.now;
    world.quiet(3 * LOOK);
    assert_eq!(
        world.around_instant(anew + FRESH),
        after_the_instant(split(1, 2, &[&[at(110), at(113)]]))
    );
}

// F19. The other half: joined from within the margin, it is the group it was.
#[test]
fn a_group_that_is_joined_from_within_the_margin_keeps_its_time() {
    let mut world = World::rested(follow(), 2, &["a"]);
    world.put(1, &[(100, 0, 5), (110, 0, 1)]);
    assert_eq!(world.step().begun, [Begun::Prepare(1)]);
    world.quiet(FRESH);
    world.join(1, 112, 1);
    assert_eq!(world.step().begun, [split(1, 2, &[&[at(110), at(112)]])]);
}

// F20.
#[test]
fn of_two_regions_to_split_the_one_whose_group_has_gone_longer_is_first() {
    let mut world = World::anew(follow(), 3);
    world.settle(&["a"]);
    let rests_until = world.now + REST;
    // The group of the higher region parts two looks before that of the lower.
    world.put(2, &[(200, 0, 2), (206, 0, 1)]);
    world.quiet(2 * LOOK);
    world.put(1, &[(100, 0, 2), (106, 0, 1)]);
    assert_eq!(but_for_prepare(world.run_up_to(rests_until)), []);
    world.obedient = false;
    assert_eq!(
        world.tick_at(rests_until).begun,
        [split(2, 3, &[&[at(206)]])]
    );
    world.quiet(FRESH);
    world.split_through(2);
    assert_eq!(world.step().begun, [split(1, 4, &[&[at(106)]])]);
}

// F20.
#[test]
fn of_two_regions_whose_groups_have_gone_as_long_the_lower_is_split_first() {
    let mut world = World::anew(follow(), 3);
    world.settle(&["a"]);
    let rests_until = world.now + REST;
    world.put(2, &[(200, 0, 2), (206, 0, 1)]);
    world.put(1, &[(100, 0, 2), (106, 0, 1)]);
    assert_eq!(but_for_prepare(world.run_up_to(rests_until)), []);
    world.obedient = false;
    assert_eq!(
        world.tick_at(rests_until).begun,
        [split(1, 3, &[&[at(106)]])]
    );
    world.quiet(FRESH);
    world.split_through(1);
    assert_eq!(world.step().begun, [split(2, 4, &[&[at(206)]])]);
}

// ---------------------------------------------------------------------------------
// F21 to F23. Turns, and `Prepare` (sections 5.3 and 5.6).
// ---------------------------------------------------------------------------------

// F21. K21.
#[test]
fn a_region_that_was_split_last_is_merged_before_it_is_split_again_and_one_that_was_merged_last_is_split_first()
 {
    let mut world = World::rested(follow(), 4, &["a"]);
    world.put(1, &[(100, 0, 5), (110, 0, 1)]);
    world.put(2, &[(300, 0, 1)]);
    world.put(3, &[(400, 0, 1)]);
    let (_, begun) = world.next_begun(FRESH);
    assert_eq!(begun, [Begun::Prepare(1)]);
    let (first, begun) = world.next_begun(2 * FRESH);
    assert_eq!(begun, [split(1, 4, &[&[at(110)]])]);
    world.quiet(LOOK);
    // While it rests, a second group parts, and two regions come near the players
    // who stay, one after the other.
    world.join(1, 120, 1);
    world.put(2, &[(98, 0, 1)]);
    world.quiet(LOOK);
    world.put(3, &[(102, 0, 1)]);
    // It was split last, so the merge that has waited longest has its turn.
    let rested = first + LAG + REST;
    assert_eq!(but_for_prepare(world.run_up_to(rested)), []);
    assert_eq!(but_for_prepare(world.would_begin_at(rested - MOMENT)), []);
    assert_eq!(world.would_begin_at(rested), [merge(1, 2)]);
    let (second, begun) = world.next_begun(LOOK);
    assert_eq!(begun, [merge(1, 2)]);
    world.quiet(LOOK);
    // It was merged last, so it is split, although the other merge has stood all the
    // while; and that merge is not begun at the same tick.
    let rested = second + LAG + REST;
    assert_eq!(but_for_prepare(world.run_up_to(rested)), []);
    assert_eq!(but_for_prepare(world.would_begin_at(rested - MOMENT)), []);
    let again = split(1, 5, &[&[at(120)]]);
    assert_eq!(world.would_begin_at(rested), std::slice::from_ref(&again));
    let (third, begun) = world.next_begun(LOOK);
    assert_eq!(begun, [again]);
    world.quiet(LOOK);
    // A third group parts while it rests after that split. It was split last, and
    // the merge is first.
    world.join(1, 84, 1);
    let rested = third + LAG + REST;
    assert_eq!(but_for_prepare(world.run_up_to(rested)), []);
    assert_eq!(world.would_begin_at(rested), [merge(1, 3)]);
    let (fourth, begun) = world.next_begun(LOOK);
    assert_eq!(begun, [merge(1, 3)]);
    // Then the split again.
    let rested = fourth + LAG + REST;
    assert_eq!(but_for_prepare(world.run_up_to(rested)), []);
    assert_eq!(world.would_begin_at(rested), [split(1, 6, &[&[at(84)]])]);
}

// F22.
#[test]
fn a_split_that_found_nobody_was_the_regions_turn_as_well() {
    let mut world = World::rested(follow(), 3, &["a"]);
    world.put(1, &[(100, 0, 5), (110, 0, 1)]);
    world.put(2, &[(300, 0, 1)]);
    world.obedient = false;
    world.quiet_but_for_prepare(FRESH + LOOK);
    // The region was in neither a merge nor a split yet, so it is split first.
    assert_eq!(world.step().begun, [split(1, 3, &[&[at(110)]])]);
    world.split_off(1, Off::Nobody);
    let ended = world.now;
    world.obedient = true;
    assert_eq!(world.alone_until(1), Some(ended + REST));
    // While it rests another region comes near the players who stay. The group is
    // where it was and has stood, and so has the merge.
    world.put(2, &[(98, 0, 1)]);
    assert_eq!(but_for_prepare(world.run_up_to(ended + REST)), []);
    assert_eq!(
        but_for_prepare(world.would_begin_at(ended + REST - MOMENT)),
        []
    );
    assert_eq!(world.would_begin_at(ended + REST), [merge(1, 2)]);
}

// F22.
#[test]
fn a_merge_that_came_to_nothing_leaves_the_turn_where_it_was() {
    let mut world = World::rested(follow(), 3, &["a"]);
    world.put(1, &[(100, 0, 5), (110, 0, 1)]);
    world.put(2, &[(300, 0, 1)]);
    world.quiet_but_for_prepare(FRESH + LOOK);
    assert_eq!(world.step().begun, [split(1, 3, &[&[at(110)]])]);
    world.quiet(LOOK);
    // It was split last. A second group parts and a region comes near while it
    // rests, and the merge has its turn.
    world.join(1, 120, 1);
    world.put(2, &[(98, 0, 1)]);
    let began = world.next_begun(REST + FRESH).1;
    assert_eq!(began, [Begun::Prepare(1)]);
    world.obedient = false;
    let began = world.next_begun(REST + FRESH).1;
    assert_eq!(began, [merge(1, 2)]);
    // The merge comes to nothing, and both regions are left alone for `LONG`.
    world.merge_off(2, Off::Busy);
    let ended = world.now;
    world.obedient = true;
    assert_eq!(world.alone_until(1), Some(ended + LONG));
    assert_eq!(world.alone_until(2), Some(ended + LONG));
    // The region was still split last: it is the merge's turn again, not the split's.
    assert_eq!(but_for_prepare(world.run_up_to(ended + LONG)), []);
    assert_eq!(
        but_for_prepare(world.would_begin_at(ended + LONG - MOMENT)),
        []
    );
    assert_eq!(world.would_begin_at(ended + LONG), [merge(1, 2)]);
}

// F22.
#[test]
fn an_absorption_leaves_the_turn_where_it_was() {
    let mut world = World::anew(follow(), 3);
    world.unpin(&[1]);
    world.settle(&["a"]);
    let settled = world.now;
    world.rest();
    world.put(2, &[(300, 0, 1)]);
    // The home region's only players are far from the chunk players enter in: they
    // are split off, and it is left without players and was split last.
    world.put(0, &[(10, 0, 1)]);
    world.quiet_but_for_prepare(FRESH + LOOK);
    assert_eq!(world.step().begun, [split(0, 3, &[&[at(10)]])]);
    // Region 1 has had no players since its first report, a moment after it was
    // given away, and is absorbed by the home region half a minute after that.
    let due = settled + LAG + EMPTY_FOR;
    assert_eq!(world.run_up_to(due), []);
    assert_eq!(world.step().begun, [merge(0, 1)]);
    // The workers do it, and the home region's first report after it has nobody.
    world.quiet(2 * LOOK);
    assert_eq!(world.known(), [0, 2, 3]);
    // Then a group of it stands far off, and region 2 comes near the chunk players
    // enter in. It is free, both have stood a second later, and it was split last:
    // the merge has its turn. An absorption is no merge that it was merged last by.
    world.put(0, &[(20, 0, 1)]);
    world.put(2, &[(2, 0, 1)]);
    assert_eq!(world.step().begun, [Begun::Prepare(0)]);
    world.quiet(FRESH);
    assert_eq!(world.step().begun, [merge(0, 2)]);
}

// F23.
#[test]
fn a_region_is_told_to_prepare_when_its_rest_ends_within_a_second_and_not_before() {
    let mut world = World::anew(follow(), 2);
    world.settle(&["a"]);
    let rests_until = world.now + REST;
    world.put(1, &[(100, 0, 2), (106, 0, 1)]);
    assert_eq!(world.run_up_to(rests_until - FRESH), []);
    assert_eq!(
        world.around_instant(rests_until - FRESH),
        from_the_instant_on(Begun::Prepare(1))
    );
    // Once, and the split at the end of the rest.
    assert_eq!(
        world.tick_at(rests_until - FRESH).begun,
        [Begun::Prepare(1)]
    );
    assert_eq!(world.run_up_to(rests_until), []);
    assert_eq!(
        world.around_instant(rests_until),
        from_the_instant_on(split(1, 2, &[&[at(106)]]))
    );
}

// F23. K26.
#[test]
fn a_region_is_not_told_to_prepare_again_however_long_its_split_waits() {
    let mut world = apart();
    world.obedient = false;
    // Somebody asks for a split of region 2 again and again, each of which finds
    // nobody three seconds later: region 1's split waits for the one split there is
    // in the world, for more than three rests.
    let ask = |world: &mut World| {
        let changes = world
            .coordinator
            .split(world.now, region(2), &[at(500)], ASKER)
            .expect("a split by hand is not held back by a rest");
        world.take("split", changes);
    };
    ask(&mut world);
    assert_eq!(world.step().begun, [Begun::Prepare(1)]);
    for _ in 0..12 {
        world.quiet(3 * FRESH);
        world.split_off(2, Off::Nobody);
        ask(&mut world);
    }
    world.quiet(3 * FRESH);
    world.split_off(2, Off::Nobody);
    assert_eq!(world.step().begun, [split(1, 3, &[&[at(106)]])]);
}

// F23.
#[test]
fn a_region_is_told_to_prepare_again_after_a_split_of_it_has_begun() {
    let mut world = World::rested(follow(), 2, &["a"]);
    world.put(1, &[(100, 0, 5), (110, 0, 1)]);
    assert_eq!(world.step().begun, [Begun::Prepare(1)]);
    world.quiet(FRESH);
    assert_eq!(world.step().begun, [split(1, 2, &[&[at(110)]])]);
    let made = world.now + LAG;
    world.quiet(LOOK);
    // A second group parts while it rests.
    world.join(1, 120, 1);
    assert_eq!(world.run_up_to(made + REST - FRESH), []);
    assert_eq!(
        world.around_instant(made + REST - FRESH),
        from_the_instant_on(Begun::Prepare(1))
    );
}

// F23.
#[test]
fn a_region_is_told_to_prepare_again_after_a_merge_of_it_has_begun() {
    let mut world = World::rested(follow(), 3, &["a"]);
    world.put(1, &[(100, 0, 5), (110, 0, 1)]);
    assert_eq!(world.step().begun, [Begun::Prepare(1)]);
    // Before the group has stood, somebody has the region absorb another.
    let changes = world
        .coordinator
        .merge(world.now, region(1), region(2), ASKER)
        .expect("nothing speaks against the merge");
    world.take("merge", changes);
    world.quiet(LOOK);
    let ended = world.now - (LOOK - LAG);
    assert_eq!(world.known(), [0, 1]);
    assert_eq!(world.alone_until(1), Some(ended + REST));
    assert_eq!(world.run_up_to(ended + REST - FRESH), []);
    assert_eq!(
        world.around_instant(ended + REST - FRESH),
        from_the_instant_on(Begun::Prepare(1))
    );
}

// F23. K26.
#[test]
fn a_group_that_parts_and_comes_back_at_every_tick_has_its_region_told_to_prepare_once_in_a_rest() {
    let mut world = World::rested(follow(), 2, &["a"]);
    let mut told = Vec::new();
    for look in 0..160 {
        let x = if look % 2 == 0 { 106 } else { 105 };
        world.put(1, &[(100, 0, 2), (x, 0, 1)]);
        for begun in world.step().begun {
            assert_eq!(begun, Begun::Prepare(1));
            told.push(world.now);
        }
    }
    // It is forgotten at a tick at which no split is wanted and it is more than a
    // rest old, which is the look after a rest has passed, and said at the look after.
    let first = told[0];
    let again = REST + 2 * LOOK;
    assert_eq!(
        told,
        [first, first + again, first + 2 * again, first + 3 * again]
    );
}

/// A group parts for one look and comes back; the region is told to prepare. The
/// group is back until a tick `last` after that, and parts again at the tick a rest
/// and a look after. Returns what that tick begins.
fn told_again_when_no_split_was_last_wanted(last: Duration) -> Vec<Begun> {
    let mut world = World::rested(follow(), 2, &["a"]);
    world.put(1, &[(100, 0, 2), (106, 0, 1)]);
    assert_eq!(world.step().begun, [Begun::Prepare(1)]);
    let told = world.now;
    world.put(1, &[(100, 0, 2), (105, 0, 1)]);
    assert_eq!(world.run_up_to(told + REST), []);
    assert_eq!(world.tick_at(told + last).begun, []);
    world.put(1, &[(100, 0, 2), (106, 0, 1)]);
    world.tick_at(told + REST + LOOK).begun
}

// F23. Section 5.6: it is forgotten "at a tick at which no split is wanted of the
// region and `prepared` is more than a rest old".
#[test]
fn what_a_region_was_told_is_forgotten_when_no_split_is_wanted_more_than_a_rest_later() {
    assert_eq!(told_again_when_no_split_was_last_wanted(REST - MOMENT), []);
    assert_eq!(told_again_when_no_split_was_last_wanted(REST), []);
    assert_eq!(
        told_again_when_no_split_was_last_wanted(REST + MOMENT),
        [Begun::Prepare(1)]
    );
}

// F23. Section 5.6: the conditions of section 5.1 hold of `Prepare` as well, and a
// split that is begun at the tick at which they come to hold is begun without one.
#[test]
fn a_region_is_not_told_to_prepare_while_a_region_has_never_been_reported() {
    let mut world = World::anew(follow(), 3);
    world.mute.insert(2);
    world.settle(&["a"]);
    world.rest();
    world.put(1, &[(100, 0, 2), (106, 0, 1)]);
    world.quiet(REST);
    world.mute.clear();
    world.put(2, &[(300, 0, 1)]);
    assert_eq!(world.step().begun, [split(1, 3, &[&[at(106)]])]);
}

// ---------------------------------------------------------------------------------
// K2, K4, K5 and K25, which the state machine alone decides and F does not name.
// ---------------------------------------------------------------------------------

// K2 (a).
#[test]
fn a_region_with_another_near_its_far_group_is_split_and_the_part_merges_with_that_region_when_it_has_rested()
 {
    let mut world = World::rested(follow(), 3, &["a"]);
    world.put(1, &[(100, 0, 3), (110, 0, 1)]);
    world.put(2, &[(112, 0, 2)]);
    assert_eq!(world.step().begun, [Begun::Prepare(1)]);
    world.quiet(FRESH);
    // No merge with region 2 is wanted meanwhile: the places near it are of the
    // group that goes.
    assert_eq!(world.step().begun, [split(1, 3, &[&[at(110)]])]);
    let made = world.now + LAG;
    assert_eq!(world.run_up_to(made + REST), []);
    assert_eq!(
        world.around_instant(made + REST),
        from_the_instant_on(merge(2, 3))
    );
}

// K2 (b).
#[test]
fn a_region_whose_groups_another_region_joins_is_merged_with_it_once_and_never_split() {
    let mut world = World::rested(follow(), 3, &["a"]);
    world.put(1, &[(100, 0, 3), (109, 0, 1)]);
    world.put(2, &[(102, 0, 1), (107, 0, 1)]);
    assert_eq!(world.run(3 * REST), [merge(1, 2)]);
    assert_eq!(world.known(), [0, 1]);
}

// K4.
#[test]
fn players_who_cross_the_band_between_the_distances_and_back_are_merged_and_split_once_each_time() {
    let mut world = pair();
    // Two apart, in two regions: one merge.
    let (merged, begun) = world.next_begun(2 * FRESH);
    assert_eq!(begun, [merge(1, 2)]);
    // Three to five apart, in one region: nothing, however long.
    for x in [103, 104, 105] {
        world.put(1, &[(100, 0, 1), (x, 0, 1)]);
        world.quiet(2 * REST);
    }
    // Six apart: one split, of the one with the higher chunk.
    world.put(1, &[(100, 0, 1), (106, 0, 1)]);
    assert_eq!(world.step().begun, [Begun::Prepare(1)]);
    world.quiet(FRESH);
    assert_eq!(world.step().begun, [split(1, 3, &[&[at(106)]])]);
    assert!(world.now >= merged + REST);
    // Five to three apart, in two regions: nothing, however long.
    world.quiet(LOOK);
    for x in [105, 104, 103] {
        world.put(3, &[(x, 0, 1)]);
        world.quiet(2 * REST);
    }
    // Two apart: one merge.
    world.put(3, &[(102, 0, 1)]);
    world.quiet(FRESH + LOOK);
    assert_eq!(world.step().begun, [merge(1, 3)]);
}

// K4: "one stop every ten seconds" is the worst anybody can do to a region.
#[test]
fn a_player_who_flies_across_the_band_and_back_has_the_regions_merged_and_split_a_rest_apart() {
    let mut world = World::rested(follow(), 2, &["a"]);
    let mut began: Vec<(Instant, Begun)> = Vec::new();
    // The player is two chunks from the others for a look and seven for the next,
    // for two minutes, in whichever region has them.
    for look in 0..480 {
        let x = if look % 16 < 8 { 102 } else { 107 };
        let parts: Vec<u32> = world.known().into_iter().filter(|id| *id > 1).collect();
        match parts.as_slice() {
            [] => world.put(1, &[(100, 0, 3), (x, 0, 1)]),
            [part] => world.put(*part, &[(x, 0, 1)]),
            several => panic!("more than one part: {several:?}"),
        }
        for begun in world.step().begun {
            if !matches!(begun, Begun::Prepare(_)) {
                began.push((world.now, begun));
            }
        }
    }
    assert!(began.len() >= 6, "{began:?}");
    assert!(
        began
            .iter()
            .any(|(_, begun)| matches!(begun, Begun::Merge { .. })),
        "{began:?}"
    );
    for pair in began.windows(2) {
        assert!(pair[1].0 >= pair[0].0 + REST, "{began:?}");
    }
}

// K5.
#[test]
fn a_region_whose_players_walked_away_as_it_was_absorbed_is_merged_all_the_same_and_split_after_the_rest()
 {
    let mut world = pair();
    world.obedient = false;
    world.quiet(FRESH + LOOK);
    assert_eq!(world.step().begun, [merge(1, 2)]);
    // The order is given; the player walks off before the worker has done it.
    world.put(2, &[(120, 0, 1)]);
    world.merge_through(2);
    let ended = world.now;
    world.obedient = true;
    assert_eq!(world.alone_until(1), Some(ended + REST));
    assert_eq!(world.run_up_to(ended + REST - FRESH), []);
    assert_eq!(
        world.tick_at(ended + REST - FRESH).begun,
        [Begun::Prepare(1)]
    );
    assert_eq!(world.run_up_to(ended + REST), []);
    assert_eq!(
        world.around_instant(ended + REST),
        from_the_instant_on(split(1, 3, &[&[at(120)]]))
    );
}

// K25: the worker says twice what came of a split, once from what it remembers and
// once from its queue. The second word finds no split noted and has the list read.
#[test]
fn a_second_word_of_a_split_that_has_ended_changes_nothing_and_has_the_list_read() {
    let mut world = apart();
    world.obedient = false;
    world.quiet_but_for_prepare(FRESH + LOOK);
    assert_eq!(world.step().begun, [split(1, 3, &[&[at(106)]])]);
    let as_epoch = world
        .orders
        .iter()
        .find_map(|told| match told.order {
            Order::SplitOff { as_epoch, .. } => Some(as_epoch),
            _ => None,
        })
        .expect("the order to split");
    world.split_through(1);
    let (table, reads) = (world.table(), world.reads);
    let alone = [world.alone_until(1), world.alone_until(3)];
    world.now += MOMENT;
    let changes = world
        .coordinator
        .split_ended(world.now, "a", region(1), as_epoch, Ok(region(3)));
    assert!(changes.read);
    assert_eq!(changes.reshaped, []);
    world.take("split_ended", changes);
    assert_eq!(world.reads, reads + 1);
    assert_eq!(world.table(), table);
    assert_eq!([world.alone_until(1), world.alone_until(3)], alone);
}

// ---------------------------------------------------------------------------------
// F24 to F29. What comes of it (sections 2.4, 5.4 and 5.5).
// ---------------------------------------------------------------------------------

// F24.
#[test]
fn after_a_merge_the_survivor_is_in_nothing_for_a_rest_and_then_is() {
    let mut world = World::rested(follow(), 4, &["a"]);
    world.put(1, &[(100, 0, 3)]);
    world.put(2, &[(102, 0, 1)]);
    world.put(3, &[(300, 0, 1)]);
    world.obedient = false;
    world.quiet(FRESH + LOOK);
    assert_eq!(world.step().begun, [merge(1, 2)]);
    // The workers take two seconds over it. The rest counts from its end.
    world.quiet(2 * FRESH);
    world.merge_through(2);
    let ended = world.now;
    world.obedient = true;
    assert_eq!(world.alone_until(1), Some(ended + REST));
    // A group of it parts, and another region comes near: both have stood long
    // before the rest is over, and neither is begun before.
    world.join(1, 120, 1);
    world.put(3, &[(104, 0, 1)]);
    assert_eq!(but_for_prepare(world.run_up_to(ended + REST)), []);
    // It was merged last, so the split is first.
    assert_eq!(
        but_for_prepare(world.would_begin_at(ended + REST - MOMENT)),
        []
    );
    assert_eq!(
        world.would_begin_at(ended + REST),
        [split(1, 4, &[&[at(120)]])]
    );
    assert_eq!(
        world.would_begin_at(ended + REST + MOMENT),
        [split(1, 4, &[&[at(120)]])]
    );
}

// F25.
#[test]
fn after_a_split_the_region_and_the_part_are_in_nothing_for_a_rest_and_then_are() {
    let mut world = World::rested(follow(), 4, &["a"]);
    world.put(1, &[(100, 0, 5), (110, 0, 1), (112, 0, 1)]);
    world.put(2, &[(300, 0, 1)]);
    world.put(3, &[(400, 0, 1)]);
    world.obedient = false;
    world.quiet_but_for_prepare(FRESH + LOOK);
    assert_eq!(world.step().begun, [split(1, 4, &[&[at(110), at(112)]])]);
    world.quiet(2 * FRESH);
    let (part, _) = world.split_through(1);
    let ended = world.now;
    world.obedient = true;
    assert_eq!(part, 4);
    assert_eq!(world.alone_until(1), Some(ended + REST));
    assert_eq!(world.alone_until(4), Some(ended + REST));
    // A region comes near those who stayed, and another near those who went.
    world.put(2, &[(98, 0, 1)]);
    world.put(3, &[(114, 0, 1)]);
    assert_eq!(world.run_up_to(ended + REST), []);
    assert_eq!(
        world.around_instant(ended + REST),
        [
            vec![],
            vec![merge(1, 2), merge(4, 3)],
            vec![merge(1, 2), merge(4, 3)]
        ]
    );
}

// F26.
#[test]
fn after_a_split_that_found_nobody_the_region_rests_and_is_split_with_the_chunks_of_the_newest_report()
 {
    let mut world = World::rested(follow(), 2, &["a"]);
    world.obedient = false;
    world.put(1, &[(100, 0, 5), (110, 0, 1)]);
    world.quiet_but_for_prepare(FRESH + LOOK);
    assert_eq!(world.step().begun, [split(1, 2, &[&[at(110)]])]);
    let changes = world.split_off(1, Off::Nobody);
    assert_eq!(
        changes.reshaped,
        [split_ended(1, Err(Undone::Off(Off::Nobody)))]
    );
    let ended = world.now;
    assert_eq!(world.alone_until(1), Some(ended + REST));
    // The group walks on, a chunk in two seconds.
    for look in 1..40 {
        world.put(1, &[(100, 0, 5), (110 + look / 8, 0, 1)]);
        assert_eq!(but_for_prepare(world.step().begun), []);
    }
    assert_eq!(world.now, ended + REST - LOOK);
    world.put(1, &[(100, 0, 5), (115, 0, 1)]);
    assert_eq!(world.step().begun, [split(1, 2, &[&[at(115)]])]);
}

// F26.
#[test]
fn the_third_split_in_a_row_that_finds_nobody_leaves_the_region_alone_for_long() {
    let mut world = World::rested(follow(), 2, &["a"]);
    world.obedient = false;
    world.put(1, &[(100, 0, 5), (110, 0, 1)]);
    let mut due = None;
    // The third is a failure, and the count of such answers begins anew: the sixth
    // is the second failure in a row, and twice as long.
    for (attempt, alone) in [REST, REST, LONG, REST, REST, 2 * LONG]
        .into_iter()
        .enumerate()
    {
        let (when, begun) = world.next_but_prepare(2 * LONG + FRESH);
        assert_eq!(begun, [split(1, 2, &[&[at(110)]])], "attempt {attempt}");
        if let Some(due) = due {
            assert_eq!(when, due, "attempt {attempt}");
        }
        world.split_off(1, Off::Nobody);
        assert_eq!(
            world.alone_until(1),
            Some(world.now + alone),
            "attempt {attempt}"
        );
        due = Some(world.now + alone);
    }
}

// F26. Section 5.5: which answers are "not yet", after which the region rests, and
// which are failures, after which it is left alone for `LONG`.
#[test]
fn each_answer_of_a_split_has_the_region_rest_or_leaves_it_alone_for_long_as_the_record_says() {
    let not_yet = [
        Off::Nobody,
        Off::NothingStays,
        Off::Busy,
        Off::NotRunning,
        Off::Declined(Decline::NotNext { next: region(7) }),
    ];
    let failed = [
        Off::TooLarge,
        Off::StoreLost,
        Off::Declined(Decline::TooLarge),
        Off::Declined(Decline::NotHeld { chunk: at(106) }),
        Off::Declined(Decline::Malformed),
        Off::Declined(Decline::Uncheckpointed { region: region(1) }),
        Off::Declined(Decline::Tick { named: 5 }),
    ];
    let answers = not_yet
        .into_iter()
        .map(|why| (why, REST))
        .chain(failed.into_iter().map(|why| (why, LONG)));
    for (why, alone) in answers {
        let mut world = apart();
        world.obedient = false;
        world.quiet_but_for_prepare(FRESH + LOOK);
        assert_eq!(world.step().begun, [split(1, 3, &[&[at(106)]])]);
        let changes = world.split_off(1, why);
        assert_eq!(
            changes.reshaped,
            [split_ended(1, Err(Undone::Off(why)))],
            "{why:?}"
        );
        let ended = world.now;
        assert_eq!(world.alone_until(1), Some(ended + alone), "{why:?}");
        // The group is where it was, and is split off when that time has passed.
        assert_eq!(
            but_for_prepare(world.run_up_to(ended + alone)),
            [],
            "{why:?}"
        );
        assert_eq!(
            but_for_prepare(world.would_begin_at(ended + alone - MOMENT)),
            [],
            "{why:?}"
        );
        assert_eq!(
            but_for_prepare(world.would_begin_at(ended + alone)),
            [split(1, 3, &[&[at(106)]])],
            "{why:?}"
        );
    }
}

// F26. Section 5.5: a split of which nobody knows what came, because its worker said
// nothing within the lease, comes to nothing "otherwise".
#[test]
fn a_split_that_is_not_answered_within_the_lease_leaves_the_region_alone_for_long() {
    let mut world = apart();
    world.obedient = false;
    world.quiet_but_for_prepare(FRESH + LOOK);
    assert_eq!(world.step().begun, [split(1, 3, &[&[at(106)]])]);
    let asked = world.now;
    assert_eq!(world.run_up_to(asked + LEASE), []);
    // It has one lease from when it was asked: it is still under way just before
    // that is up and when it is, and ends just after.
    for when in [asked + LEASE - MOMENT, asked + LEASE] {
        let mut world = world.clone();
        assert_eq!(world.tick_at(when).begun, []);
        assert_eq!(world.under_way(), [Asked::Split { region: region(1) }]);
        assert_eq!(world.ended, []);
    }
    world.tick_at(asked + LEASE + MOMENT);
    assert_eq!(world.under_way(), []);
    let (ended, how) = world.last_ended();
    assert_eq!(how, split_ended(1, Err(Undone::Overdue)));
    assert_eq!(ended, asked + LEASE + MOMENT);
    assert_eq!(world.alone_until(1), Some(ended + LONG));
}

// F27.
#[test]
fn each_reason_a_worker_gives_for_a_merge_that_is_off_leaves_both_regions_alone_for_long() {
    for why in [
        Off::NotRunning,
        Off::Busy,
        Off::TooLarge,
        Off::StoreLost,
        Off::Unreadable,
        Off::Refused,
        Off::Declined(Decline::TooLarge),
        Off::Declined(Decline::Uncheckpointed { region: region(1) }),
        Off::Declined(Decline::Tick { named: 5 }),
        Off::Declined(Decline::NoSuchRegion),
        Off::Declined(Decline::Home),
        Off::Declined(Decline::NotOpened { epoch: None }),
    ] {
        let mut world = pair();
        world.obedient = false;
        world.quiet(FRESH + LOOK);
        assert_eq!(world.step().begun, [merge(1, 2)]);
        let changes = world.merge_off(2, why);
        assert_eq!(
            changes.reshaped,
            [merge_ended(1, 2, Err(Undone::Off(why)))],
            "{why:?}"
        );
        let ended = world.now;
        assert_eq!(world.known(), [0, 1, 2]);
        assert_eq!(world.alone_until(1), Some(ended + LONG), "{why:?}");
        assert_eq!(world.alone_until(2), Some(ended + LONG), "{why:?}");
        // Their players are where they were, and it is tried again when that time
        // has passed.
        assert_eq!(world.run_up_to(ended + LONG), [], "{why:?}");
        assert_eq!(
            world.around_instant(ended + LONG),
            from_the_instant_on(merge(1, 2)),
            "{why:?}"
        );
    }
}

/// The merge of [`pair`] is begun and no worker has done anything about it yet.
/// Region 1 survives and region 2 is to be absorbed.
fn a_merge_under_way() -> World {
    let mut world = pair();
    world.obedient = false;
    world.quiet(FRESH + LOOK);
    assert_eq!(world.step().begun, [merge(1, 2)]);
    world
}

/// The merge that ended last came to nothing as `why` has it, and each of `regions`
/// is left alone for `LONG` from then.
fn assert_left_alone_for_long(world: &World, why: Undone, regions: &[u32]) {
    let (ended, how) = world.last_ended();
    assert_eq!(how, merge_ended(1, 2, Err(why)), "{}", world.story());
    for id in regions {
        assert_eq!(world.alone_until(*id), Some(ended + LONG), "region {id}");
    }
}

// F27: `NotReleased`.
#[test]
fn a_merge_whose_region_is_not_released_within_the_lease_leaves_both_alone_for_long() {
    let mut world = a_merge_under_way();
    let asked = world.now;
    assert_eq!(world.run_to(asked + LEASE), []);
    world.quiet(LOOK);
    assert_left_alone_for_long(&world, Undone::NotReleased, &[1, 2]);
}

// F27: `Overdue`.
#[test]
fn a_merge_whose_worker_says_nothing_within_the_lease_leaves_both_alone_for_long() {
    let mut world = a_merge_under_way();
    let asked = world.now;
    world.let_go(2);
    assert_eq!(world.run_to(asked + LEASE), []);
    world.quiet(LOOK);
    assert_left_alone_for_long(&world, Undone::Overdue, &[1, 2]);
}

// F27: `Unread`.
#[test]
fn a_merge_whose_worker_says_nothing_and_whose_reading_fails_leaves_both_alone_for_long() {
    let mut world = a_merge_under_way();
    let asked = world.now;
    world.let_go(2);
    assert_eq!(world.run_to(asked + LEASE), []);
    world.readings = Readings::Failing;
    world.quiet(LOOK);
    assert_left_alone_for_long(&world, Undone::Unread, &[1, 2]);
}

// F27: `Contradicted`.
#[test]
fn a_merge_that_the_worker_calls_done_and_the_list_does_not_show_leaves_both_alone_for_long() {
    let mut world = a_merge_under_way();
    world.let_go(2);
    let (worker, into) = world.order_to_absorb(2);
    let changes = world
        .coordinator
        .absorb_ended(world.now, &worker, into, region(2), Ok(()));
    world.take("absorb_ended", changes);
    assert_left_alone_for_long(&world, Undone::Contradicted, &[1, 2]);
}

// F27: `Disowned`. K18.
#[test]
fn a_merge_whose_survivor_changes_its_epoch_leaves_both_alone_for_long() {
    let mut world = a_merge_under_way();
    world.let_go(2);
    // The survivor's worker runs it with a later epoch than the merge began with.
    let mut holding = world.coordinator.assignments("a");
    let at_it = holding
        .iter()
        .position(|held| held.region == region(1))
        .expect("the worker runs region 1");
    holding[at_it].epoch = world.highest_epoch() + 5;
    world.register("a", &holding);
    // The reservation ends by asking for the list first, which is answered at once.
    if !world.under_way().is_empty() {
        world.quiet(LOOK);
    }
    assert_left_alone_for_long(&world, Undone::Disowned(region(1)), &[1, 2]);
}

// F27: `Gone`.
#[test]
fn a_merge_whose_survivor_the_list_no_longer_has_leaves_the_other_region_alone_for_long() {
    let mut world = a_merge_under_way();
    // The list has region 1 absorbed by the home region, of which nobody told the
    // coordinator.
    world.merged(0, 1);
    world.hand_in();
    if !world.under_way().is_empty() {
        world.quiet(LOOK);
    }
    assert_eq!(world.known(), [0, 2]);
    let (ended, how) = world.last_ended();
    assert!(
        matches!(
            how.outcome,
            Err(Undone::Gone(gone) | Undone::Disowned(gone)) if gone == region(1)
        ),
        "{how:?}"
    );
    assert_eq!(world.alone_until(2), Some(ended + LONG));
}

// F27.
#[test]
fn each_failure_in_a_row_doubles_the_time_up_to_eight_times_and_a_merge_that_ends_well_makes_it_long_again()
 {
    let mut world = World::rested(follow(), 4, &["a"]);
    world.put(1, &[(100, 0, 2)]);
    world.put(2, &[(102, 0, 1)]);
    world.put(3, &[(300, 0, 1)]);
    world.obedient = false;
    let (_, begun) = world.next_begun(2 * FRESH);
    assert_eq!(begun, [merge(1, 2)]);
    for times in [1, 2, 4, 8, 8] {
        world.merge_off(2, Off::TooLarge);
        let due = world.now + times * LONG;
        assert_eq!(world.alone_until(1), Some(due));
        assert_eq!(world.alone_until(2), Some(due));
        // It is tried again when that time has passed, and not a moment sooner.
        assert_eq!(world.run_up_to(due), [], "{times} times `LONG`");
        assert_eq!(
            world.around_instant(due),
            from_the_instant_on(merge(1, 2)),
            "{times} times `LONG`"
        );
        assert_eq!(world.tick_at(due).begun, [merge(1, 2)]);
    }
    // The sixth attempt is made, and the survivor rests.
    world.merge_through(2);
    let ended = world.now;
    assert_eq!(world.alone_until(1), Some(ended + REST));
    // Its next failure is its first in a row again.
    world.put(3, &[(104, 0, 1)]);
    assert_eq!(
        world.next_begun(REST + FRESH),
        (ended + REST, vec![merge(1, 3)])
    );
    world.merge_off(3, Off::TooLarge);
    let ended = world.now;
    assert_eq!(world.alone_until(1), Some(ended + LONG));
    assert_eq!(world.alone_until(3), Some(ended + LONG));
}

// F27. Section 5.5: what comes of a merge is noted "for those somebody asked for by
// hand as for its own".
#[test]
fn a_merge_by_hand_that_comes_to_nothing_leaves_both_regions_alone_as_one_begun_by_itself_does() {
    let mut world = World::rested(follow(), 3, &["a"]);
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(200, 0, 1)]);
    world.obedient = false;
    for times in [1, 2, 4] {
        world.quiet(FRESH);
        let changes = world
            .coordinator
            .merge(world.now, region(1), region(2), ASKER)
            .expect("a merge by hand is not held back by a region being left alone");
        world.take("merge", changes);
        world.merge_off(2, Off::Busy);
        assert_eq!(world.alone_until(1), Some(world.now + times * LONG));
        assert_eq!(world.alone_until(2), Some(world.now + times * LONG));
    }
}

// F28.
#[test]
fn a_region_that_is_given_an_owner_or_an_epoch_rests_from_then() {
    // Assigned, to a worker that waited.
    let mut world = World::anew(follow(), 3);
    world.settle(&["a", "b"]);
    let given = world.now;
    for id in 0..3 {
        assert_eq!(world.alone_until(id), Some(given + REST));
    }
    world.rest();
    // Handed over after a release that somebody asked for.
    world.quiet(FRESH);
    let (_, changes) = world
        .coordinator
        .move_region(world.now, region(2), Some("b"), 7)
        .expect("the region can be moved");
    world.take("move_region", changes);
    world.let_go(2);
    assert_eq!(world.owner(2).as_deref(), Some("b"));
    assert_eq!(world.alone_until(2), Some(world.now + REST));
    // Taken from a worker that does not vouch for it, and given to the other.
    let alone = world.alone_until(1);
    world.unvouched.insert(1);
    let taken = world.until_given(1, LEASE + FRESH);
    assert_ne!(world.alone_until(1), alone);
    assert_eq!(world.alone_until(1), Some(taken + REST));
    // Taken on a worker's word at a registration (K6).
    let world = World::reported(follow(), 3, &[("a", &[0, 1, 2])]);
    for id in 0..3 {
        assert_eq!(world.alone_until(id), Some(world.made + REST));
    }
}

/// Region 1 has five players in chunk 100 and a group in the chunks 110 and 115;
/// region 2 has a player in chunk 108 and one in chunk 117, which that group joins:
/// each of its chunks is within the merge distance of one of them (D9).
fn a_group_that_joins_another_regions_players() -> World {
    let mut world = World::rested(follow(), 3, &["a"]);
    world.put(1, &[(100, 0, 5), (110, 0, 1), (115, 0, 1)]);
    world.put(2, &[(108, 0, 1), (117, 0, 1)]);
    world
}

// F29.
#[test]
fn after_a_split_the_crowds_in_the_chunks_named_count_as_the_parts() {
    let mut world = a_group_that_joins_another_regions_players();
    world.obedient = false;
    // Region 1 is surely apart, and region 2 whole, by the places of the group.
    world.quiet_but_for_prepare(FRESH + LOOK);
    assert_eq!(world.step().begun, [split(1, 3, &[&[at(110), at(115)]])]);
    let (part, _) = world.split_through(1);
    let made = world.now;
    world.obedient = true;
    assert_eq!(part, 3);
    // The part's worker says nothing of it for three seconds. Its sighting was made
    // of the crowds in the chunks named and is not fresh: it still joins the players
    // of region 2, of which no split is wanted. Had the crowds gone from region 1's
    // sighting to nowhere, region 2 would be told to prepare at the next look.
    world.mute.insert(3);
    world.quiet(3 * FRESH);
    // It reports them where they were. Region 2 is whole by places that are known
    // now, and is merged with the part when that has rested.
    world.mute.clear();
    assert_eq!(world.run_up_to(made + REST), []);
    assert_eq!(
        world.around_instant(made + REST),
        from_the_instant_on(merge(2, 3))
    );
}

// F29. The other half: when the part reports its players elsewhere, region 2 is
// split, so the test above shows what it says it does.
#[test]
fn a_region_whose_players_the_part_no_longer_joins_is_split() {
    let mut world = a_group_that_joins_another_regions_players();
    world.obedient = false;
    world.quiet_but_for_prepare(FRESH + LOOK);
    assert_eq!(world.step().begun, [split(1, 3, &[&[at(110), at(115)]])]);
    world.split_through(1);
    world.obedient = true;
    // Neither worker says anything of the region or of the part for three seconds:
    // the crowds have gone from the one sighting to the other, and are in one.
    world.mute.extend([1, 3]);
    world.quiet(3 * FRESH);
    world.mute.remove(&3);
    world.put(3, &[(500, 0, 2)]);
    assert_eq!(world.step().begun, [Begun::Prepare(2)]);
    world.mute.clear();
    world.quiet(FRESH);
    assert_eq!(world.step().begun, [split(2, 4, &[&[at(117)]])]);
}

// F29.
#[test]
fn after_a_merge_the_absorbed_regions_crowds_count_as_the_survivors_which_had_a_sighting() {
    let mut world = World::rested(follow(), 4, &["a"]);
    world.put(1, &[(300, 0, 3)]);
    world.put(2, &[(108, 0, 1), (117, 0, 1)]);
    world.put(3, &[(110, 0, 1), (115, 0, 1)]);
    // One look, at which regions 2 and 3 are one cluster; then somebody has region 1,
    // far away, absorb region 3, and its worker says nothing of it afterwards.
    world.quiet(LOOK);
    let changes = world
        .coordinator
        .merge(world.now, region(1), region(3), ASKER)
        .expect("nothing speaks against the merge");
    world.take("merge", changes);
    world.mute.insert(1);
    // Region 3's crowds are in region 1's sighting, which is not fresh, and still
    // join the players of region 2: no split of it is wanted.
    world.quiet(3 * FRESH);
    assert_eq!(world.known(), [0, 1, 2]);
    let (ended, _) = world.last_ended();
    // Region 1 reports them where they were: it is itself to be split when it has
    // rested, and region 2 is still whole.
    world.mute.clear();
    assert_eq!(but_for_prepare(world.run_up_to(ended + REST)), []);
    assert_eq!(
        world.around_instant(ended + REST),
        from_the_instant_on(split(1, 4, &[&[at(110), at(115)]]))
    );
}

// ---------------------------------------------------------------------------------
// F30 to F38. Empty regions (sections 4.4 and 5.5).
// ---------------------------------------------------------------------------------

/// `regions` stripes of one worker that have just been given away, of which the list
/// has those in `unpinned` pinned to no area, as it has parts. Returns it with the
/// time of the first report, which is the first without players of every region
/// that a test puts nobody in.
fn with_unpinned(regions: u32, unpinned: &[u32]) -> (World, Instant) {
    let mut world = World::anew(follow(), regions);
    world.unpin(unpinned);
    world.settle(&["a"]);
    let first = world.now + LAG;
    (world, first)
}

// F30. F31: not before `EMPTY_FOR`.
#[test]
fn a_region_that_has_had_no_players_for_three_rests_is_absorbed_by_the_home_region_without_players()
{
    let (mut world, first) = with_unpinned(4, &[3]);
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(200, 0, 1)]);
    let due = first + EMPTY_FOR;
    assert_eq!(world.run_up_to(due), []);
    // The builder's reading of "for `EMPTY_FOR`": that long or longer.
    assert_eq!(world.around_instant(due), from_the_instant_on(merge(0, 3)));
    // It goes the way of every merge, and ends like one, with nobody as asker.
    let look = world.step();
    assert_eq!(look.begun, [merge(0, 3)]);
    world.quiet(LOOK);
    assert_eq!(world.last_ended().1, merge_ended(0, 3, Ok(0)));
    assert_eq!(world.known(), [0, 1, 2]);
    world.quiet(4 * EMPTY_FOR);
}

// F30.
#[test]
fn a_region_without_players_is_absorbed_by_the_lowest_region_without_players_below_it_if_the_home_region_has_players()
 {
    let (mut world, first) = with_unpinned(5, &[4]);
    world.put(0, &[(0, 0, 2)]);
    world.put(1, &[(100, 0, 1)]);
    let due = first + EMPTY_FOR;
    assert_eq!(world.run_up_to(due), []);
    assert_eq!(world.around_instant(due), from_the_instant_on(merge(2, 4)));
}

// F31.
#[test]
fn a_region_without_players_is_left_if_no_region_without_players_is_the_home_region_or_below_it() {
    // Region 2 has no players and is pinned to no area. The home region and region 1
    // have players, and region 3, which has none, is above it.
    let (mut world, _) = with_unpinned(4, &[2]);
    world.put(0, &[(0, 0, 2)]);
    world.put(1, &[(100, 0, 1)]);
    world.quiet(4 * EMPTY_FOR);
    assert_eq!(world.known(), [0, 1, 2, 3]);
}

// F31.
#[test]
fn a_report_with_a_player_in_between_begins_the_three_rests_anew() {
    let (mut world, first) = with_unpinned(4, &[3]);
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(200, 0, 1)]);
    assert_eq!(world.run_to(first + 2 * REST), []);
    // One report of it has a player, far from everybody, and the next has none.
    world.put(3, &[(500, 0, 1)]);
    world.quiet(LOOK);
    world.put(3, &[]);
    world.quiet(LOOK);
    let emptied = world.now - (LOOK - LAG);
    assert_eq!(world.run_up_to(emptied + EMPTY_FOR), []);
    assert_eq!(
        world.around_instant(emptied + EMPTY_FOR),
        from_the_instant_on(merge(0, 3))
    );
}

// F32.
#[test]
fn the_home_region_is_never_the_one_absorbed_whichever_region_it_is() {
    // The list has region 1 as the home region. Region 0 has no players and is
    // pinned to no area: it goes into the home region, which has none either,
    // although that has the higher id.
    let mut world = World::anew(follow(), 3);
    world.list.home = region(1);
    world.unpin(&[0]);
    world.put(2, &[(100, 0, 1)]);
    world.settle(&["a"]);
    let due = world.now + LAG + EMPTY_FOR;
    assert_eq!(world.run_up_to(due), []);
    assert_eq!(world.around_instant(due), from_the_instant_on(merge(1, 0)));

    // The home region has no players and is pinned to no area, and region 0, which
    // has none either, is below it: the home region stays.
    let mut world = World::anew(follow(), 3);
    world.list.home = region(1);
    world.unpin(&[1]);
    world.put(2, &[(100, 0, 1)]);
    world.settle(&["a"]);
    world.quiet(4 * EMPTY_FOR);
    assert_eq!(world.known(), [0, 1, 2]);

    // And by the distances it survives with fewer players, whichever region it is.
    let mut world = World::anew(follow(), 3);
    world.list.home = region(1);
    world.settle(&["a"]);
    world.rest();
    world.put(1, &[(0, 0, 1)]);
    world.put(0, &[(2, 0, 5)]);
    assert_eq!(world.next_begun(2 * FRESH).1, [merge(1, 0)]);
}

// F33.
#[test]
fn a_region_the_list_shows_pinned_is_never_absorbed_for_being_empty() {
    let mut world = World::anew(follow(), 4);
    world.settle(&["a"]);
    world.quiet(10 * EMPTY_FOR);
    assert_eq!(world.known(), [0, 1, 2, 3]);
}

// F33.
#[test]
fn a_region_the_list_shows_pinned_is_merged_by_the_distances_like_any_other_and_is_a_survivor() {
    // By the distances: every merge of the tests above is of pinned regions; here
    // once more, with what the list has of the survivor afterwards.
    let mut world = pair();
    assert_eq!(world.next_begun(2 * FRESH).1, [merge(1, 2)]);
    world.quiet(LOOK);
    let pinned = &world.list.regions[1].pinned;
    assert_eq!(
        pinned.len(),
        2,
        "the survivor is pinned to the areas of both"
    );
    // As a survivor: region 3 is pinned to no area and has no players; the home
    // region has players, and regions 1 and 2, which have none, are pinned.
    let (mut world, first) = with_unpinned(4, &[3]);
    world.put(0, &[(0, 0, 1)]);
    let due = first + EMPTY_FOR;
    assert_eq!(world.run_up_to(due), []);
    assert_eq!(world.tick_at(due).begun, [merge(1, 3)]);
    // And it is never absorbed itself, nor is region 2.
    world.quiet(10 * EMPTY_FOR);
    assert_eq!(world.known(), [0, 1, 2]);
}

// F34.
#[test]
fn a_region_that_had_a_player_in_a_report_a_second_ago_or_less_is_no_survivor() {
    let (mut world, first) = with_unpinned(3, &[2]);
    world.put(0, &[(0, 0, 1)]);
    world.put(1, &[(100, 0, 1)]);
    // Region 2 is due, and there is nobody to take it.
    assert_eq!(world.run_to(first + EMPTY_FOR + REST), []);
    // The home region's player leaves: this report is its first without one.
    world.put(0, &[]);
    world.now += LAG;
    world.report();
    let emptied = world.now;
    world.now += LOOK - LAG;
    assert_eq!(world.tick().begun, []);
    assert_eq!(world.run_up_to(emptied + FRESH), []);
    assert_eq!(
        world.around_instant(emptied + FRESH),
        after_the_instant(merge(0, 2))
    );
}

// F35.
#[test]
fn of_five_empty_regions_due_at_one_tick_the_highest_goes_into_the_lowest_and_the_one_in_the_middle_waits()
 {
    let (mut world, first) = with_unpinned(6, &[1, 2, 3, 4, 5]);
    world.put(0, &[(0, 0, 1)]);
    world.obedient = false;
    let due = first + EMPTY_FOR;
    assert_eq!(world.run_up_to(due), []);
    assert_eq!(world.would_begin_at(due - MOMENT), []);
    assert_eq!(world.tick_at(due).begun, [merge(2, 4), merge(1, 5)]);
    // Region 3 waits for a survivor: both that it could go into are in a merge.
    world.quiet(2 * FRESH);
    world.merge_through(4);
    world.merge_through(5);
    let ended = world.now;
    world.obedient = true;
    // It goes into the lowest when that has been reported without players for more
    // than a second after its absorption ended.
    let reported = ended + LAG;
    assert_eq!(world.run_up_to(reported + FRESH), []);
    assert_eq!(
        world.around_instant(reported + FRESH),
        after_the_instant(merge(1, 3))
    );
    // A region that has been a survivor begins its three rests anew: region 2 goes
    // into region 1 half a minute later, and that one is left, as the home region
    // has players and nothing without players is below it.
    let began = world.run(4 * EMPTY_FOR);
    assert_eq!(began, [merge(1, 3), merge(1, 2)], "{}", world.story());
    assert_eq!(world.known(), [0, 1]);
}

// F35.
#[test]
fn of_nine_empty_regions_due_at_one_tick_four_are_begun() {
    let (mut world, first) = with_unpinned(10, &[1, 2, 3, 4, 5, 6, 7, 8, 9]);
    world.put(0, &[(0, 0, 1)]);
    world.obedient = false;
    let due = first + EMPTY_FOR;
    assert_eq!(world.run_up_to(due), []);
    assert_eq!(
        world.tick_at(due).begun,
        [merge(4, 6), merge(3, 7), merge(2, 8), merge(1, 9)]
    );
    assert_eq!(world.under_way().len(), AT_ONCE);
    world.quiet(2 * FRESH);
}

// F35. Section 4.4: "none is left once the home region has no players."
#[test]
fn every_empty_region_that_is_pinned_to_no_area_goes_when_the_home_region_has_no_players() {
    let (mut world, first) = with_unpinned(6, &[1, 2, 3, 4, 5]);
    world.obedient = false;
    let due = first + EMPTY_FOR;
    assert_eq!(world.run_up_to(due), []);
    // The highest into the home region, the next into the lowest, and so on: one for
    // each survivor.
    assert_eq!(
        world.tick_at(due).begun,
        [merge(2, 3), merge(1, 4), merge(0, 5)]
    );
    world.obedient = true;
    world.run(4 * EMPTY_FOR);
    assert_eq!(world.known(), [0], "{}", world.story());
}

// F36.
#[test]
fn a_region_that_a_merge_is_wanted_of_is_no_survivor() {
    let (mut world, first) = with_unpinned(4, &[2, 3]);
    world.put(1, &[(100, 0, 1)]);
    // Regions 2 and 3 are due at one tick. The higher goes into the home region,
    // and nothing is there for the other to go into: region 1 has a player.
    let due = first + EMPTY_FOR;
    assert_eq!(world.run_up_to(due), []);
    assert_eq!(world.step().begun, [merge(0, 3)]);
    // The workers do it. A tenth of a second after that tick, between two ticks,
    // the home region is reported for the first time since, without players, and
    // region 1 with its player within the merge distance of the chunk players
    // enter in.
    world.put(1, &[(2, 0, 1)]);
    assert_eq!(world.step().begun, []);
    let wanted = world.now;
    assert_eq!(world.known(), [0, 1, 2]);
    // A second later the home region has had no players for more than a second,
    // and the merge with it has been wanted for exactly a second: it has not stood,
    // and region 2 is not absorbed by the home region for that.
    for _ in 0..4 {
        assert_eq!(world.step().begun, [], "{}", world.story());
    }
    assert_eq!(world.now, wanted + FRESH);
    assert_eq!(world.step().begun, [merge(0, 1)]);
}

// F36.
#[test]
fn an_empty_region_goes_into_a_second_candidate_while_a_merge_is_wanted_of_the_home_region() {
    let (mut world, first) = with_unpinned(5, &[3, 4]);
    world.put(1, &[(200, 0, 1)]);
    world.put(2, &[(100, 0, 1)]);
    let due = first + EMPTY_FOR;
    assert_eq!(world.run_up_to(due), []);
    assert_eq!(world.step().begun, [merge(0, 4)]);
    // As above; and region 1, which is below region 3, has no players from that
    // report on.
    world.put(2, &[(2, 0, 1)]);
    world.put(1, &[]);
    assert_eq!(world.step().begun, []);
    let wanted = world.now;
    for _ in 0..3 {
        assert_eq!(world.step().begun, [], "{}", world.story());
    }
    // Both have had no players for more than a second, and the home region is no
    // survivor.
    assert_eq!(world.step().begun, [merge(1, 3)]);
    assert_eq!(world.now, wanted + FRESH);
    assert_eq!(world.step().begun, [merge(0, 2)]);
}

// F37.
#[test]
fn a_second_absorption_into_one_survivor_is_begun_a_second_after_the_first_ended_and_not_a_rest_after()
 {
    // The home region has players; region 1 has none and is pinned; regions 2 and 3
    // have none, are pinned to no area and are due at one tick.
    let (mut world, first) = with_unpinned(4, &[2, 3]);
    world.put(0, &[(0, 0, 1)]);
    world.obedient = false;
    let due = first + EMPTY_FOR;
    assert_eq!(world.run_up_to(due), []);
    assert_eq!(world.step().begun, [merge(1, 3)]);
    // Region 2 waits while its one survivor is in a merge.
    world.quiet(2 * FRESH);
    world.merge_through(3);
    let ended = world.now;
    world.obedient = true;
    let alone = world.alone_until(1);
    // The survivor is reported without players a tenth of a second later, and has
    // been for more than a second a second after that.
    let reported = ended + LAG;
    assert_eq!(world.run_up_to(reported + FRESH), []);
    assert_eq!(
        world.around_instant(reported + FRESH),
        after_the_instant(merge(1, 2))
    );
    assert_eq!(world.alone_until(1), alone);
}

// F37.
#[test]
fn a_region_is_made_a_survivor_while_it_rests_for_another_reason() {
    let (mut world, first) = with_unpinned(3, &[2]);
    world.put(0, &[(0, 0, 1)]);
    let due = first + EMPTY_FOR;
    assert_eq!(world.run_to(due - 3 * FRESH), []);
    // Three seconds before region 2 is due, region 1's worker reports it with an
    // epoch the coordinator did not have: it rests for ten seconds from then.
    let mut holding = world.coordinator.assignments("a");
    holding[1].epoch = world.highest_epoch() + 5;
    world.register("a", &holding);
    assert_eq!(world.alone_until(1), Some(world.now + REST));
    assert!(world.now + REST > due + FRESH);
    assert_eq!(world.run_up_to(due), []);
    assert_eq!(world.around_instant(due), from_the_instant_on(merge(1, 2)));
}

// F37. Sections 2.4 and 4.4: the region that is absorbed has to be free, and one that
// is given an epoch begins its three rests anew.
#[test]
fn an_empty_region_that_is_given_an_epoch_begins_its_three_rests_anew() {
    let (mut world, first) = with_unpinned(3, &[2]);
    world.put(1, &[(100, 0, 1)]);
    let due = first + EMPTY_FOR;
    assert_eq!(world.run_to(due - 3 * FRESH), []);
    let mut holding = world.coordinator.assignments("a");
    holding[2].epoch = world.highest_epoch() + 5;
    world.register("a", &holding);
    let anew = world.now + LAG;
    assert_eq!(world.run_up_to(anew + EMPTY_FOR), []);
    assert_eq!(
        world.around_instant(anew + EMPTY_FOR),
        from_the_instant_on(merge(0, 2))
    );
}

/// The home region has no players and absorbs region 2, which has none either;
/// region 1's player is far off. Returns the world at the tick that begins it, with
/// the home region's `alone_until` of then.
fn the_home_region_absorbs() -> (World, Option<Instant>) {
    let (mut world, first) = with_unpinned(3, &[2]);
    world.put(1, &[(100, 0, 1)]);
    let due = first + EMPTY_FOR;
    assert_eq!(world.run_up_to(due), []);
    let alone = world.alone_until(0);
    assert_eq!(world.step().begun, [merge(0, 2)]);
    (world, alone)
}

// F38.
#[test]
fn after_an_absorption_a_merge_by_the_distances_with_the_survivor_is_begun_without_a_rest() {
    let (mut world, alone) = the_home_region_absorbs();
    // The workers do it, and the survivor's first report afterwards has nobody.
    world.quiet(LOOK);
    assert_eq!(world.last_ended().1, merge_ended(0, 2, Ok(0)));
    let ended = world.last_ended().0;
    assert_eq!(world.alone_until(0), alone);
    // Region 1's player comes near the chunk players enter in.
    world.put(1, &[(2, 0, 1)]);
    world.quiet(LOOK);
    let wanted = world.now;
    let (when, begun) = world.next_begun(2 * FRESH);
    assert_eq!((when, begun), (wanted + FRESH + LOOK, vec![merge(0, 1)]));
    assert!(when < ended + 2 * FRESH);
}

// F38. K13.
#[test]
fn a_survivor_whose_first_report_after_the_absorption_has_a_player_rests_from_that_report() {
    let (mut world, _) = the_home_region_absorbs();
    // Somebody came into one of the two regions as the absorption began, and
    // region 1's player stands near the chunk players enter in.
    world.put(0, &[(0, 0, 1)]);
    world.put(1, &[(2, 0, 1)]);
    world.quiet(LOOK);
    let reported = world.now - (LOOK - LAG);
    assert_eq!(world.last_ended().0, reported);
    assert_eq!(world.alone_until(0), Some(reported + REST));
    assert_eq!(world.run_up_to(reported + REST), []);
    assert_eq!(
        world.around_instant(reported + REST),
        from_the_instant_on(merge(0, 1))
    );
}

// F38. K13: "however late it comes".
#[test]
fn the_first_report_after_the_absorption_decides_however_long_after_the_end_it_is_taken() {
    let (mut world, alone) = the_home_region_absorbs();
    world.put(0, &[(0, 0, 1)]);
    world.put(1, &[(2, 0, 1)]);
    // The survivor's worker restores a region and reports nothing for five seconds.
    world.mute.insert(0);
    world.quiet(LEASE);
    assert_eq!(world.alone_until(0), alone);
    world.mute.clear();
    world.quiet(LOOK);
    let reported = world.now - (LOOK - LAG);
    assert_eq!(world.alone_until(0), Some(reported + REST));
    assert_eq!(world.run_up_to(reported + REST), []);
    assert_eq!(
        world.around_instant(reported + REST),
        from_the_instant_on(merge(0, 1))
    );
}

// F38.
#[test]
fn a_player_in_the_second_report_after_the_absorption_begins_no_rest() {
    let (mut world, alone) = the_home_region_absorbs();
    world.quiet(LOOK);
    world.put(0, &[(0, 0, 1)]);
    world.put(1, &[(2, 0, 1)]);
    world.quiet(LOOK);
    let wanted = world.now;
    assert_eq!(world.alone_until(0), alone);
    assert_eq!(
        world.next_begun(2 * FRESH),
        (wanted + FRESH + LOOK, vec![merge(0, 1)])
    );
}

// F38.
#[test]
fn after_an_absorption_that_came_to_nothing_the_absorbed_region_is_left_alone_and_the_survivor_is_not()
 {
    let (mut world, alone) = the_home_region_absorbs();
    world.obedient = false;
    let changes = world.merge_off(2, Off::Busy);
    assert_eq!(
        changes.reshaped,
        [merge_ended(0, 2, Err(Undone::Off(Off::Busy)))]
    );
    let ended = world.now;
    assert_eq!(world.alone_until(2), Some(ended + LONG));
    assert_eq!(world.alone_until(0), alone);
    // What the distances want of the survivor is not held back: region 1's player
    // comes near, and the merge is begun when it has stood.
    world.quiet(LOOK);
    world.put(1, &[(2, 0, 1)]);
    world.quiet(LOOK);
    let wanted = world.now;
    assert_eq!(
        world.next_begun(2 * FRESH),
        (wanted + FRESH + LOOK, vec![merge(0, 1)])
    );
    // No failure was counted for the survivor: when this merge comes to nothing, it
    // is its first, and it is left alone for `LONG` and not for twice that.
    world.merge_off(1, Off::Busy);
    assert_eq!(world.alone_until(0), Some(world.now + LONG));
    assert_eq!(world.alone_until(1), Some(world.now + LONG));
}

// F38. Section 5.5: "each empty region waits for its own failures only, and a
// survivor that cannot absorb costs one attempt for each of them in `LONG`, then in
// twice that."
#[test]
fn an_empty_region_whose_absorption_fails_is_tried_again_ever_more_rarely() {
    let (mut world, _) = the_home_region_absorbs();
    world.obedient = false;
    world.merge_off(2, Off::Busy);
    let ended = world.now;
    assert_eq!(world.alone_until(2), Some(ended + LONG));
    // It was given an owner again, and is reported without players a tenth of a
    // second later; that is three rests before it is due again, which is later than
    // `LONG`.
    let (when, begun) = world.next_begun(2 * LONG);
    assert_eq!(begun, [merge(0, 2)]);
    assert_eq!(when, ended + LOOK + EMPTY_FOR);
    world.merge_off(2, Off::Busy);
    let ended = world.now;
    assert_eq!(world.alone_until(2), Some(ended + 2 * LONG));
    assert_eq!(world.run_up_to(ended + 2 * LONG), []);
    assert_eq!(
        world.around_instant(ended + 2 * LONG),
        from_the_instant_on(merge(0, 2))
    );
}

// ---------------------------------------------------------------------------------
// F39 to F43. Evening out (section 6).
// ---------------------------------------------------------------------------------

/// `regions` stripes that one worker, `a`, runs, rested, with these players; then a
/// worker that runs nothing registers for each name of `late`.
fn one_worker_has_everything(
    policy: Option<Policy>,
    crowds: &[&[(i32, i32, u32)]],
    late: &[&str],
) -> World {
    let mut world = World::anew(policy, crowds.len() as u32);
    world.settle(&["a"]);
    for (id, crowd) in crowds.iter().enumerate() {
        world.put(id as u32, crowd);
    }
    world.rest();
    for name in late {
        world.register(name, &[]);
    }
    world
}

// F39.
#[test]
fn a_region_at_rest_is_not_moved_to_even_out_and_another_of_that_worker_is_at_once() {
    let mut world = one_worker_has_everything(
        follow(),
        &[&[(0, 0, 3)], &[(100, 0, 2)], &[(200, 0, 2)], &[]],
        &[],
    );
    // Region 3 has the fewest players and the highest id. Its worker reports it with
    // an epoch the coordinator did not have, so it rests.
    let mut holding = world.coordinator.assignments("a");
    holding[3].epoch = world.highest_epoch() + 5;
    world.register("a", &holding);
    assert_eq!(world.alone_until(3), Some(world.now + REST));
    world.register("b", &[]);
    let (owner, epoch) = (world.owner(2), world.epoch(2));
    let look = world.step();
    assert_eq!(look.begun, [Begun::Move(2)]);
    assert_eq!(
        look.changes.releases,
        [ReleaseOrder {
            worker: owner.expect("region 2 has an owner"),
            region: region(2),
            epoch,
        }]
    );
    // It is a release like any other: the worker lets go, and the region is the
    // other worker's.
    assert_eq!(world.step().begun, [Begun::Move(1)]);
    assert_eq!(world.owner(2).as_deref(), Some("b"));
    world.quiet(2 * REST);
    assert_eq!(world.runs("a"), [0, 3]);
    assert_eq!(world.runs("b"), [1, 2]);
}

// F39. Section 6: "a region is not released to even out before its `alone_until`."
#[test]
fn nothing_is_moved_to_even_out_while_every_region_of_the_worker_rests() {
    let mut world = World::anew(follow(), 2);
    world.settle(&["a"]);
    let rests_until = world.now + REST;
    world.register("b", &[]);
    assert_eq!(world.run_up_to(rests_until), []);
    assert_eq!(
        world.around_instant(rests_until),
        from_the_instant_on(Begun::Move(1))
    );
}

// F40.
#[test]
fn the_region_with_the_fewest_players_is_moved_first_and_one_without_a_sighting_last() {
    let mut world = World::anew(follow(), 5);
    world.mute.insert(4);
    world.settle(&["a"]);
    world.put(0, &[(0, 0, 4)]);
    world.put(1, &[(100, 0, 3)]);
    world.put(2, &[(200, 0, 1)]);
    world.put(3, &[(300, 0, 2)]);
    world.rest();
    for name in ["b", "c", "d", "e"] {
        world.register(name, &[]);
    }
    // One release at a time, each at the tick after the one before has ended: what a
    // worker was just given rests, and the others' regions do not.
    let began: Vec<Vec<Begun>> = (0..6).map(|_| world.step().begun).collect();
    assert_eq!(
        began,
        [
            vec![Begun::Move(2)],
            vec![Begun::Move(3)],
            vec![Begun::Move(1)],
            vec![Begun::Move(0)],
            vec![],
            vec![]
        ]
    );
    // The region nobody has reported is the one that stays.
    assert_eq!(world.runs("a"), [4]);
    world.quiet(2 * REST);
}

// F41.
#[test]
fn a_region_of_a_merge_that_is_wanted_is_not_moved_to_even_out_whether_it_has_stood_or_not() {
    // Regions 2 and 3 have the fewest players and are near each other.
    let crowds: [&[(i32, i32, u32)]; 4] =
        [&[(0, 0, 5)], &[(100, 0, 3)], &[(200, 0, 1)], &[(202, 0, 1)]];
    // Not stood: the other worker registers before the first look that wants it.
    let mut world = World::anew(follow(), 4);
    world.settle(&["a"]);
    world.put(0, crowds[0]);
    world.put(1, crowds[1]);
    world.rest();
    world.put(2, crowds[2]);
    world.put(3, crowds[3]);
    world.register("b", &[]);
    assert_eq!(world.step().begun, [Begun::Move(1)]);
    // Neither is moved before their merge is begun.
    let began = world.run(2 * FRESH);
    assert!(began.contains(&merge(2, 3)), "{began:?}");
    assert!(
        !began.contains(&Begun::Move(2)) && !began.contains(&Begun::Move(3)),
        "{began:?}"
    );

    // Stood, and held back by a region that nobody has reported, which does not
    // hold evening out back (section 5.1).
    let mut world = World::anew(follow(), 5);
    world.mute.insert(4);
    world.settle(&["a"]);
    for (id, crowd) in crowds.iter().enumerate() {
        world.put(id as u32, crowd);
    }
    world.rest();
    world.quiet(2 * FRESH);
    world.register("b", &[]);
    assert_eq!(world.step().begun, [Begun::Move(1)]);
}

// F41.
#[test]
fn a_region_of_a_split_that_is_wanted_is_not_moved_to_even_out() {
    // Regions 2 and 3 have the fewest players, and region 3 the higher id; its
    // players are apart from the look at which the other worker is there.
    let mut world = one_worker_has_everything(
        follow(),
        &[&[(0, 0, 5)], &[(100, 0, 3)], &[(200, 0, 2)], &[]],
        &[],
    );
    world.put(3, &[(300, 0, 1), (306, 0, 1)]);
    world.register("b", &[]);
    assert_eq!(world.step().begun, [Begun::Prepare(3), Begun::Move(2)]);
}

// F42. And section 6: "Nor within a lease of a merge or a split having ended" goes.
#[test]
fn nothing_is_evened_out_while_a_merge_is_under_way_and_a_region_that_does_not_rest_is_moved_as_soon_as_it_has_ended()
 {
    let mut world = World::rested(follow(), 4, &["a"]);
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(102, 0, 1)]);
    world.obedient = false;
    world.quiet(FRESH + LOOK);
    assert_eq!(world.step().begun, [merge(1, 2)]);
    world.register("b", &[]);
    world.quiet(3 * FRESH);
    world.merge_through(2);
    let ended = world.now;
    world.obedient = true;
    // The survivor rests. Of the other two, neither of which has players, the higher.
    assert_eq!(world.step().begun, [Begun::Move(3)]);
    assert_eq!(world.now, ended + LOOK);
    world.quiet(2 * REST);
    assert_eq!(world.runs("a"), [0, 1]);
    assert_eq!(world.runs("b"), [3]);
}

/// One region, the home region, of the worker `a`, and a worker `b` that runs
/// nothing. `stay` players stand at the chunk players enter in and `go` six chunks
/// from it: they are split off, and `a` has two regions more than `b`. Returns the
/// world when the workers have made the split, and when that was.
fn a_part_on_the_worker_that_made_it(stay: u32, go: u32) -> (World, Instant) {
    let mut world = World::anew(follow(), 1);
    world.settle(&["a", "b"]);
    world.rest();
    world.put(0, &[(0, 0, stay), (6, 0, go)]);
    let (begun, began) = world.next_but_prepare(2 * FRESH);
    assert_eq!(began, [split(0, 1, &[&[at(6)]])]);
    world.quiet(LOOK);
    let made = begun + LAG;
    assert_eq!(world.runs("a"), [0, 1]);
    assert_eq!(world.alone_until(0), Some(made + REST));
    assert_eq!(world.alone_until(1), Some(made + REST));
    (world, made)
}

// F39, F40. What the end-to-end test E2 is after, on the state machine: the lease is
// half the rest here, so a build that evened out a lease after a split has ended
// moves a region five seconds too soon.
#[test]
fn a_part_is_moved_to_even_out_a_rest_after_its_split_and_not_a_lease_after() {
    let (mut world, made) = a_part_on_the_worker_that_made_it(2, 1);
    assert_eq!(world.run_up_to(made + REST), []);
    assert_eq!(
        world.around_instant(made + REST),
        from_the_instant_on(Begun::Move(1))
    );
    assert_eq!(world.step().begun, [Begun::Move(1)]);
    world.quiet(2 * REST);
    assert_eq!(world.runs("a"), [0]);
    assert_eq!(world.runs("b"), [1]);
}

// F40. And which of the two is moved is the one with the fewer players, not the one
// with the higher id.
#[test]
fn the_region_a_part_left_is_moved_to_even_out_if_it_has_fewer_players_than_the_part() {
    let (mut world, made) = a_part_on_the_worker_that_made_it(1, 3);
    assert_eq!(world.run_up_to(made + REST), []);
    assert_eq!(
        world.around_instant(made + REST),
        from_the_instant_on(Begun::Move(0))
    );
}

// F43.
#[test]
fn a_coordinator_that_decides_nothing_by_itself_evens_out_as_it_did() {
    // The region with the highest id, whatever players it has and whatever is
    // wanted of it, and although its worker has just reported it with a new epoch.
    let mut world = one_worker_has_everything(
        None,
        &[&[(0, 0, 1)], &[(100, 0, 1)], &[], &[(102, 0, 9)]],
        &[],
    );
    let mut holding = world.coordinator.assignments("a");
    holding[3].epoch = world.highest_epoch() + 5;
    world.register("a", &holding);
    world.register("b", &[]);
    assert_eq!(world.step().begun, [Begun::Move(3)]);
    assert_eq!(world.step().begun, [Begun::Move(2)]);
    world.quiet(2 * REST);
}

// F43.
#[test]
fn a_coordinator_that_decides_nothing_by_itself_evens_nothing_out_within_a_lease_of_a_merge() {
    let mut world = World::rested(None, 4, &["a"]);
    let changes = world
        .coordinator
        .merge(world.now, region(1), region(2), ASKER)
        .expect("nothing speaks against the merge");
    world.take("merge", changes);
    world.merge_through(2);
    let ended = world.now;
    world.register("b", &[]);
    assert_eq!(world.run_up_to(ended + LEASE), []);
    let (when, begun) = world.next_begun(FRESH);
    assert_eq!(begun, [Begun::Move(3)]);
    assert!(when <= ended + LEASE + LOOK);
}

// ---------------------------------------------------------------------------------
// F44 to F47. The list (section 7).
// ---------------------------------------------------------------------------------

// F44.
#[test]
fn the_list_is_asked_for_at_the_first_tick_and_a_lease_after_every_answer() {
    let mut world = World::anew(follow(), 2);
    world.register("a", &[]);
    let look = world.tick();
    assert!(look.changes.read);
    assert_eq!(world.reads, 1);
    world.settle(&["a"]);
    world.rest();
    let (read, reads) = (world.listed.expect("the list has been read"), world.reads);
    assert_eq!(world.run_up_to(read + LEASE), []);
    assert_eq!(world.reads, reads);
    for (when, asked) in [
        (read + LEASE - MOMENT, 0),
        (read + LEASE, 1),
        (read + LEASE + MOMENT, 1),
    ] {
        let mut world = world.clone();
        assert_eq!(world.tick_at(when).changes.read, asked == 1);
        assert_eq!(world.reads, reads + asked);
    }
    // Twelve in a minute, each answered at the tick that asks.
    world.quiet(12 * LEASE);
    assert_eq!(world.reads, reads + 12);
    // The timer counts from the last answer, also from one that was a failure.
    world.readings = Readings::Failing;
    world.quiet(12 * LEASE);
    assert_eq!(world.reads, reads + 24);
}

// F44.
#[test]
fn a_coordinator_that_decides_nothing_by_itself_never_asks_for_the_list_by_the_time() {
    let mut world = World::anew(None, 2);
    world.register("a", &[]);
    let look = world.tick();
    assert!(!look.changes.read);
    world.settle(&["a"]);
    world.quiet(12 * LEASE);
    assert_eq!(world.reads, 0);
}

// F45.
#[test]
fn the_list_is_not_asked_for_again_while_a_reading_is_asked_for() {
    let mut world = World::rested(follow(), 2, &["a"]);
    let (read, reads) = (world.listed.expect("the list has been read"), world.reads);
    world.readings = Readings::Held;
    assert_eq!(world.run_to(read + LEASE), []);
    assert_eq!(world.reads, reads + 1);
    assert!(world.unanswered);
    world.quiet(12 * LEASE);
    assert_eq!(world.reads, reads + 1);
    // It is answered between two ticks, and the next is asked for a lease later.
    world.now += LAG;
    world.hand_in();
    let answered = world.now;
    world.now += LOOK - LAG;
    world.tick();
    assert_eq!(world.run_up_to(answered + LEASE), []);
    assert_eq!(world.reads, reads + 1);
    world.quiet(LOOK);
    assert_eq!(world.reads, reads + 2);
}

// F46.
#[test]
fn a_region_that_the_list_adds_is_assigned_and_holds_everything_back_until_it_has_been_reported() {
    let mut world = World::rested(follow(), 4, &["a"]);
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(102, 0, 1)]);
    world.put(3, &[(300, 0, 1)]);
    world.quiet(LOOK);
    // The list has a region that nobody runs: a part whose worker died before it
    // said so. A reading shows it.
    world.list.regions.push(RegionInfo {
        region: region(4),
        epoch: 500,
        bounds: None,
        pinned: Vec::new(),
    });
    world.list.next = region(5);
    world.mute.insert(4);
    world.hand_in();
    if world.owner(4).is_none() {
        world.quiet(LOOK);
    }
    let given = world.now;
    assert_eq!(world.owner(4).as_deref(), Some("a"));
    assert!(world.epoch(4) > 500);
    assert_eq!(world.alone_until(4), Some(given + REST));
    // Its worker restores it for five seconds and says nothing of it: the merge of
    // the other two, which has stood, is not begun.
    world.quiet(LEASE);
    // Its first report, with a player near region 3's: the merge that waited is
    // begun at the next tick, and the one with the new region when that has rested.
    world.mute.clear();
    world.put(4, &[(302, 0, 1)]);
    assert_eq!(world.step().begun, [merge(1, 2)]);
    assert_eq!(world.run_up_to(given + REST), []);
    assert_eq!(
        world.around_instant(given + REST),
        from_the_instant_on(merge(3, 4))
    );
}

// F46. K19.
#[test]
fn a_reading_between_a_splits_record_and_the_workers_word_does_not_add_the_part() {
    let mut world = apart();
    world.obedient = false;
    world.quiet_but_for_prepare(FRESH + LOOK);
    assert_eq!(world.step().begun, [split(1, 3, &[&[at(106)]])]);
    // The store has made the part, and the worker has yet to say so.
    let (worker, chunks, as_epoch, _) = world.order_to_split(1);
    let part = world
        .parted(1, &chunks, as_epoch)
        .expect("somebody stands in a chunk named");
    assert_eq!(part, 3);
    world.hand_in();
    assert_eq!(world.known(), [0, 1, 2]);
    assert!(world.table().is_complete());
    assert_eq!(world.under_way(), [Asked::Split { region: region(1) }]);
    world.quiet(FRESH);
    assert_eq!(world.known(), [0, 1, 2]);
    // The worker's word names it, and it is that worker's with the epoch it was told.
    let changes =
        world
            .coordinator
            .split_ended(world.now, &worker, region(1), as_epoch, Ok(region(3)));
    let changes = world.take("split_ended", changes);
    assert_eq!(changes.reshaped, [split_ended(1, Ok(3))]);
    assert_eq!(world.known(), [0, 1, 2, 3]);
    assert_eq!(world.owner(3).as_deref(), Some("a"));
    assert_eq!(world.epoch(3), as_epoch);
}

// F46. K19: "one that is not is added by the next reading after the reservation".
#[test]
fn a_region_that_a_reading_showed_during_a_split_and_that_was_not_its_part_is_added_by_the_reading_after()
 {
    let mut world = two_apart();
    world.obedient = false;
    world.quiet_but_for_prepare(FRESH + LOOK);
    assert_eq!(world.step().begun, [split(1, 3, &[&[at(106)]])]);
    // The list has a region 3 that nobody runs, and it is not this split's.
    world.list.regions.push(RegionInfo {
        region: region(3),
        epoch: 500,
        bounds: None,
        pinned: Vec::new(),
    });
    world.list.next = region(4);
    world.mute.insert(3);
    world.hand_in();
    assert_eq!(world.known(), [0, 1, 2]);
    // The split found nobody. The reading that follows adds the region, which is
    // given to a worker.
    world.split_off(1, Off::Nobody);
    if world.owner(3).is_none() {
        world.quiet(LOOK);
    }
    assert_eq!(world.known(), [0, 1, 2, 3]);
    assert_eq!(world.owner(3).as_deref(), Some("a"));
    // Until it has been reported nothing is begun: not the split of region 2, which
    // has stood and is free.
    world.quiet(LEASE);
    world.mute.clear();
    world.put(3, &[(500, 0, 1)]);
    assert_eq!(world.step().begun, [split(2, 4, &[&[at(206)]])]);
}

// F47.
#[test]
fn a_region_that_a_reading_shows_pinned_no_longer_is_absorbed_and_one_that_it_shows_pinned_is_not()
{
    // Pinned, and without players for longer than three rests; then a reading has
    // it pinned to no area.
    let mut world = World::anew(follow(), 3);
    world.settle(&["a"]);
    world.put(1, &[(100, 0, 1)]);
    world.quiet(EMPTY_FOR + REST);
    world.unpin(&[2]);
    let read = world.listed;
    while world.listed == read {
        assert_eq!(world.step().begun, []);
    }
    assert_eq!(world.step().begun, [merge(0, 2)]);

    // Pinned to no area; before it is due, a reading has it pinned.
    let (mut world, first) = with_unpinned(3, &[2]);
    world.put(1, &[(100, 0, 1)]);
    assert_eq!(world.run_to(first + 2 * REST), []);
    let area = world.list.regions[1].pinned.clone();
    world.list.regions[2].pinned = area;
    world.quiet(4 * EMPTY_FOR);
    assert_eq!(world.known(), [0, 1, 2]);
}

// F47.
#[test]
fn whether_a_region_is_pinned_is_what_the_last_reading_that_succeeded_had() {
    // The store would say that it is pinned, and cannot be read from five seconds
    // before the region is due: it is absorbed by what the last good reading had.
    let (mut world, first) = with_unpinned(3, &[2]);
    world.put(1, &[(100, 0, 1)]);
    let due = first + EMPTY_FOR;
    assert_eq!(world.run_to(due - LEASE), []);
    let area = world.list.regions[1].pinned.clone();
    world.list.regions[2].pinned = area;
    world.readings = Readings::Failing;
    let read = world.listed.expect("the list has been read");
    assert!(read + 2 * LEASE > due + LOOK);
    assert_eq!(world.run_up_to(due), []);
    assert_eq!(world.listed, Some(read));
    assert_eq!(world.would_begin_at(due), [merge(0, 2)]);
}

// ---------------------------------------------------------------------------------
// F48 to F50, and K6, K10, K16 and K17.
// ---------------------------------------------------------------------------------

// F48. K6.
#[test]
fn a_new_coordinator_begins_nothing_with_a_region_for_a_rest_after_its_worker_reported_it() {
    let mut world = World::reported(follow(), 3, &[("a", &[0, 1, 2])]);
    for id in 0..3 {
        assert_eq!(world.alone_until(id), Some(world.made + REST));
    }
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(102, 0, 1)]);
    assert_eq!(world.run_up_to(world.made + REST), []);
    assert_eq!(
        world.around_instant(world.made + REST),
        from_the_instant_on(merge(1, 2))
    );
}

// F48. K6, K23.
#[test]
fn a_new_coordinator_begins_nothing_before_every_region_has_been_reported() {
    // Nobody reports region 3 when the coordinator is made. It is given away when
    // the grace period is over, and its worker restores it until thirteen seconds
    // after the coordinator was made.
    let mut world = World::reported(follow(), 4, &[("a", &[0, 1, 2])]);
    world.mute.insert(3);
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(102, 0, 1)]);
    assert_eq!(world.owner(3), None);
    let given = world.until_given(3, LEASE + FRESH);
    assert!(given <= world.made + LEASE + LOOK);
    assert_eq!(world.alone_until(3), Some(given + REST));
    // The other two have rested ten seconds after it was made, and their merge has
    // stood for seconds.
    assert_eq!(world.run_to(world.made + REST + 3 * FRESH), []);
    world.mute.clear();
    assert_eq!(world.step().begun, [merge(1, 2)]);
}

// F48. Section 2.4: no sighting is made for a region that took in another.
#[test]
fn a_new_coordinator_begins_nothing_before_a_region_has_been_reported_that_the_first_reading_shows_as_having_taken_in_another()
 {
    let mut world = World::anew(follow(), 3);
    // The store has region 1, which the coordinator knows, absorbed by the home region.
    world.merged(0, 1);
    world.register("a", &[held(0, 10), held(2, 12)]);
    world.mute.insert(0);
    world.put(2, &[(100, 0, 2), (106, 0, 1)]);
    world.tick();
    assert_eq!(world.reads, 1);
    assert_eq!(world.known(), [0, 2]);
    // Region 2 has rested ten seconds after its worker reported it, and its group
    // has stood for seconds. The home region's worker restores it and reports
    // nothing of it for twelve seconds: until then nothing is begun, and the region
    // is not told to prepare either.
    world.quiet(REST + 2 * FRESH);
    world.mute.clear();
    assert_eq!(world.step().begun, [split(2, 3, &[&[at(106)]])]);
}

// F48. Section 2.4: "If it has none, or does not know that region, none is made, and
// the crowds go with the absorbed region."
#[test]
fn no_sighting_is_made_for_a_survivor_that_has_none_when_a_reading_shows_what_it_took_in() {
    let mut world = World::anew(follow(), 6);
    world.mute.insert(1);
    world.settle(&["a"]);
    world.rest();
    world.put(3, &[(110, 0, 1), (115, 0, 1)]);
    world.put(4, &[(400, 0, 1)]);
    world.put(5, &[(402, 0, 1)]);
    world.quiet(2 * FRESH);
    // Somebody has region 1, which no worker has reported yet, absorb region 3.
    let changes = world
        .coordinator
        .merge(world.now, region(1), region(3), ASKER)
        .expect("nothing speaks against the merge");
    world.take("merge", changes);
    world.quiet(LEASE);
    assert_eq!(world.known(), [0, 1, 2, 4, 5]);
    // Region 1 is still without a sighting, and holds everything back: the merge of
    // the regions 4 and 5 is begun at the tick after its first report.
    world.mute.clear();
    assert_eq!(world.step().begun, [merge(4, 5)]);
}

/// Region 1 has a player in chunk 100 and one in chunk 104, region 2 one in chunk
/// 108. The one in chunk 104 walks into chunk 106, which is region 2's, and is handed
/// over: for one look they are in region 2's report, which is the newer, and in
/// region 1's, which is the older. Returns the world after that look.
fn a_player_in_two_sightings() -> World {
    let mut world = World::rested(follow(), 3, &["a"]);
    world.put(1, &[(100, 0, 1), (104, 0, 1)]);
    world.put(2, &[(108, 0, 1)]);
    world.quiet(REST);
    world.now += LAG;
    world.put(2, &[(106, 0, 1), (108, 0, 1)]);
    world.report_of(&[0, 2]);
    world.now += LOOK - LAG;
    assert_eq!(world.tick().begun, []);
    world
}

// F49. K10.
#[test]
fn a_player_who_is_in_two_sightings_for_one_report_merges_nothing() {
    let mut world = a_player_in_two_sightings();
    // Region 1's next report is without them.
    world.put(1, &[(100, 0, 1)]);
    world.quiet(2 * REST);
}

// F49. K10: "or `A`'s sighting stops being fresh, and either ends the run."
#[test]
fn a_player_who_is_in_two_sightings_merges_nothing_if_the_older_one_is_not_replaced() {
    let mut world = a_player_in_two_sightings();
    world.mute.insert(1);
    world.quiet(2 * REST);
}

// F49. The other half: K10, "a player who stands on a boundary ... can be in both
// reports again and again, and the two regions are then merged".
#[test]
fn a_player_who_is_in_two_sightings_for_more_than_a_second_merges_the_two_regions() {
    let mut world = a_player_in_two_sightings();
    let first = world.now;
    assert_eq!(
        world.next_begun(2 * FRESH),
        (first + FRESH + LOOK, vec![merge(1, 2)])
    );
}

// F50. K17.
#[test]
fn what_is_asked_by_hand_is_done_at_rest_and_rests_afterwards() {
    let mut world = World::anew(follow(), 4);
    world.settle(&["a", "b"]);
    let rests_until = world.now + REST;
    world.put(2, &[(200, 0, 1), (201, 0, 1)]);
    world.obedient = false;
    world.quiet(FRESH);
    for id in 0..4 {
        assert_eq!(world.alone_until(id), Some(rests_until));
    }
    // A merge.
    let changes = world
        .coordinator
        .merge(world.now, region(1), region(3), ASKER)
        .expect("a merge by hand is not held back by a rest");
    world.take("merge", changes);
    world.quiet(FRESH);
    world.merge_through(3);
    assert_eq!(world.known(), [0, 1, 2]);
    assert_eq!(world.alone_until(1), Some(world.now + REST));
    // A split.
    let changes = world
        .coordinator
        .split(world.now, region(2), &[at(201)], ASKER)
        .expect("a split by hand is not held back by a rest");
    world.take("split", changes);
    world.quiet(FRESH);
    let (part, _) = world.split_through(2);
    assert_eq!(part, 4);
    assert_eq!(world.alone_until(2), Some(world.now + REST));
    assert_eq!(world.alone_until(4), Some(world.now + REST));
    // A move.
    assert_eq!(world.owner(0).as_deref(), Some("a"));
    world
        .coordinator
        .move_region(world.now, region(0), Some("b"), 9)
        .map(|(_, changes)| world.take("move_region", changes))
        .expect("a move by hand is not held back by a rest");
    world.quiet(FRESH);
    world.let_go(0);
    assert_eq!(world.owner(0).as_deref(), Some("b"));
    assert_eq!(world.alone_until(0), Some(world.now + REST));
}

// F50. K17: "A group split off by hand within `D_m` of the others is merged back
// when both have rested".
#[test]
fn a_group_that_is_split_off_by_hand_near_the_others_is_merged_back_when_both_have_rested() {
    let mut world = World::rested(follow(), 2, &["a"]);
    world.put(1, &[(100, 0, 1), (102, 0, 1)]);
    world.quiet(FRESH);
    let changes = world
        .coordinator
        .split(world.now, region(1), &[at(102)], ASKER)
        .expect("nothing speaks against the split");
    world.take("split", changes);
    let (part, _) = world.split_through(1);
    let ended = world.now;
    assert_eq!(part, 2);
    assert_eq!(world.run_up_to(ended + REST), []);
    assert_eq!(
        world.around_instant(ended + REST),
        from_the_instant_on(merge(1, 2))
    );
}

// F50. K17: "two regions merged by hand whose players are more than `D_s` apart are
// split again."
#[test]
fn two_regions_that_are_merged_by_hand_far_apart_are_split_again_when_the_survivor_has_rested() {
    let mut world = World::rested(follow(), 3, &["a"]);
    world.put(1, &[(100, 0, 2)]);
    world.put(2, &[(110, 0, 1)]);
    world.quiet(FRESH);
    let changes = world
        .coordinator
        .merge(world.now, region(1), region(2), ASKER)
        .expect("nothing speaks against the merge");
    world.take("merge", changes);
    world.merge_through(2);
    let ended = world.now;
    assert_eq!(but_for_prepare(world.run_up_to(ended + REST)), []);
    assert_eq!(
        world.around_instant(ended + REST),
        from_the_instant_on(split(1, 3, &[&[at(110)]]))
    );
}

// F50. K16.
#[test]
fn the_regions_of_a_worker_that_leaves_are_released_while_they_rest() {
    let mut world = World::anew(follow(), 4);
    world.settle(&["a", "b"]);
    let rests_until = world.now + REST;
    world.quiet(FRESH);
    let changes = world.coordinator.leaving(world.now, "b");
    let changes = world.take("leaving", changes);
    let released: Vec<u32> = changes
        .releases
        .iter()
        .map(|release| release.region.0)
        .collect();
    assert_eq!(released, [1, 3]);
    assert!(world.now < rests_until);
    // The workers do it, and the leaver is forgotten.
    world.now += LAG;
    world.obey();
    world.silence("b");
    assert_eq!(world.runs("a"), [0, 1, 2, 3]);
    world.quiet(2 * REST);
}

// ---------------------------------------------------------------------------------
// K7, K11, K18, K20 and K23, and what the sections say that F does not name.
// ---------------------------------------------------------------------------------

// K7.
#[test]
fn when_the_survivors_worker_dies_both_regions_are_given_owners_and_left_alone_for_long() {
    let mut world = World::running(follow(), 3, &[("a", &[0, 1]), ("b", &[2])]);
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(102, 0, 1)]);
    world.obedient = false;
    world.quiet(FRESH + LOOK);
    assert_eq!(world.step().begun, [merge(1, 2)]);
    // The other worker lets go; the survivor's dies before it has absorbed.
    world.let_go(2);
    let changes = world.coordinator.disconnected(world.now, "a");
    world.take("disconnected", changes);
    world.silence("a");
    world.orders.clear();
    world.obedient = true;
    let began = world.run(LEASE + FRESH);
    assert_eq!(began, [], "{}", world.story());
    let (ended, how) = world.last_ended();
    assert!(
        matches!(how.outcome, Err(Undone::Disowned(_) | Undone::Overdue)),
        "{how:?}"
    );
    assert_eq!(how.asker, None);
    for id in 0..3 {
        assert_eq!(world.owner(id).as_deref(), Some("b"));
    }
    assert_eq!(world.alone_until(1), Some(ended + LONG));
    assert_eq!(world.alone_until(2), Some(ended + LONG));
    // Their players are where they were, and it is tried again after that.
    assert_eq!(world.run_up_to(ended + LONG), []);
    assert_eq!(
        world.around_instant(ended + LONG),
        from_the_instant_on(merge(1, 2))
    );
}

// K18.
#[test]
fn a_split_whose_region_changes_its_epoch_before_the_workers_word_leaves_it_alone_for_long() {
    let mut world = apart();
    world.obedient = false;
    world.quiet_but_for_prepare(FRESH + LOOK);
    assert_eq!(world.step().begun, [split(1, 3, &[&[at(106)]])]);
    let mut holding = world.coordinator.assignments("a");
    holding[1].epoch = world.highest_epoch() + 5;
    world.register("a", &holding);
    if !world.under_way().is_empty() {
        world.quiet(LOOK);
    }
    let (ended, how) = world.last_ended();
    assert_eq!(how, split_ended(1, Err(Undone::Disowned(region(1)))));
    assert_eq!(world.alone_until(1), Some(ended + LONG));
    // The order is not given again.
    world.orders.clear();
    world.obedient = true;
    assert_eq!(but_for_prepare(world.run_up_to(ended + LONG)), []);
    assert_eq!(
        but_for_prepare(world.would_begin_at(ended + LONG)),
        [split(1, 3, &[&[at(106)]])]
    );
}

// K20, K11.
#[test]
fn a_merge_that_the_list_shows_done_before_the_worker_says_so_ends_there_and_the_survivor_rests() {
    let mut world = World::rested(follow(), 4, &["a"]);
    world.put(1, &[(100, 0, 2)]);
    world.put(2, &[(102, 0, 1)]);
    world.put(3, &[(104, 0, 1)]);
    world.obedient = false;
    world.quiet(FRESH + LOOK);
    // Regions 1 and 2, and 2 and 3, have stood since the same tick: the pair with
    // the lower ids is begun, and the other waits for region 2.
    assert_eq!(world.step().begun, [merge(1, 2)]);
    world.let_go(2);
    // The store has it done, and a reading says so before the worker does.
    let (worker, into) = world.order_to_absorb(2);
    world.merged(1, 2);
    let changes = world.hand_in();
    assert_eq!(changes.reshaped, [merge_ended(1, 2, Ok(1))]);
    let ended = world.now;
    assert_eq!(world.alone_until(1), Some(ended + REST));
    // K11: a report that was read before the merge is taken now, without the player
    // it took in.
    let before = PlayersOf {
        region: region(1),
        epoch: world.epoch(1),
        tick: 1_000,
        crowds: vec![(at(100), 2)],
    };
    world.reported.insert(1, 1_000);
    assert!(world.coordinator.players(world.now, "a", &[before]));
    // The worker's word finds no merge noted and has the list read once more.
    let reads = world.reads;
    let changes = world
        .coordinator
        .absorb_ended(world.now, &worker, into, region(2), Ok(()));
    assert!(changes.read);
    assert_eq!(changes.reshaped, []);
    world.take("absorb_ended", changes);
    assert_eq!(world.reads, reads + 1);
    // Nothing is begun with the survivor for a rest; then region 3, which has
    // waited for the region it took in, is merged with it.
    world.obedient = true;
    assert_eq!(world.run_up_to(ended + REST), []);
    assert_eq!(
        world.around_instant(ended + REST),
        from_the_instant_on(merge(1, 3))
    );
}

// K23. Section 5.1: "A region that was sighted once and has been silent since does
// not stop anything here".
#[test]
fn a_region_that_was_reported_once_and_is_silent_holds_nothing_back_that_its_players_are_not_near()
{
    let mut world = World::rested(follow(), 4, &["a"]);
    world.put(3, &[(300, 0, 1)]);
    world.quiet(FRESH);
    world.mute.insert(3);
    world.quiet(REST);
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(102, 0, 1)]);
    world.quiet(LOOK);
    let first = world.now;
    assert_eq!(
        world.next_begun(2 * FRESH),
        (first + FRESH + LOOK, vec![merge(1, 2)])
    );
}

// K1. Section 5.3, "Waiting": when the region that was waited for is the one
// absorbed, the merge that waited for it waits for the survivor, since when it did.
#[test]
fn a_merge_that_waited_for_a_region_keeps_its_place_with_the_region_that_absorbed_it() {
    let mut world = World::anew(follow(), 5);
    world.settle(&["a"]);
    let rests_until = world.now + REST;
    // Region 1 has five players and absorbs region 2, which is two chunks east of
    // it. Region 4 comes two chunks east of region 2, then region 3 two chunks west
    // of region 1: neither is near the other region of the two.
    world.put(1, &[(98, 0, 5)]);
    world.put(2, &[(100, 0, 1)]);
    world.put(3, &[(300, 0, 1)]);
    world.put(4, &[(400, 0, 1)]);
    world.quiet(2 * LOOK);
    world.put(4, &[(102, 0, 1)]);
    world.quiet(2 * LOOK);
    world.put(3, &[(96, 0, 1)]);
    assert_eq!(world.run_up_to(rests_until), []);
    assert_eq!(world.tick_at(rests_until).begun, [merge(1, 2)]);
    let ended = rests_until + LAG;
    // Both are two chunks from region 1's players now. Region 4 has waited longer,
    // for the region that was absorbed; region 3 has the lower id.
    assert_eq!(world.run_up_to(ended + REST), []);
    assert_eq!(
        world.around_instant(ended + REST),
        from_the_instant_on(merge(1, 4))
    );
}

// Section 5.5: a part "was not split last".
#[test]
fn a_part_was_not_split_last_and_is_split_before_it_is_merged() {
    let mut world = World::rested(follow(), 3, &["a"]);
    world.put(1, &[(100, 0, 5), (110, 0, 2), (120, 0, 1)]);
    world.put(2, &[(300, 0, 1)]);
    world.quiet_but_for_prepare(FRESH + LOOK);
    assert_eq!(world.step().begun, [split(1, 3, &[&[at(110)], &[at(120)]])]);
    let made = world.now + LAG;
    world.quiet(LOOK);
    // Region 2 comes near the players of the part who are to stay in it.
    world.put(2, &[(108, 0, 1)]);
    assert_eq!(but_for_prepare(world.run_up_to(made + REST)), []);
    assert_eq!(
        world.would_begin_at(made + REST),
        [split(3, 4, &[&[at(120)]])]
    );
}

// Section 5.3, "Turns": the bit is set by a split and cleared by a merge "whoever
// asked for it".
#[test]
fn a_split_and_a_merge_by_hand_are_the_regions_turns_as_well() {
    let mut world = World::rested(follow(), 4, &["a"]);
    world.put(1, &[(100, 0, 5), (101, 0, 1)]);
    world.put(2, &[(300, 0, 1)]);
    world.quiet(FRESH);
    // Somebody has the player in chunk 101 split off, who walks away at once.
    let changes = world
        .coordinator
        .split(world.now, region(1), &[at(101)], ASKER)
        .expect("nothing speaks against the split");
    world.take("split", changes);
    let (part, _) = world.split_through(1);
    let ended = world.now;
    assert_eq!(part, 4);
    world.put(4, &[(500, 0, 1)]);
    // A group parts and a region comes near while it rests: it was split last, by
    // hand, so the merge is first.
    world.join(1, 110, 1);
    world.put(2, &[(98, 0, 1)]);
    assert_eq!(but_for_prepare(world.run_up_to(ended + REST - FRESH)), []);
    let mut merged_by_hand = world.clone();
    assert_eq!(but_for_prepare(world.run_up_to(ended + REST)), []);
    assert_eq!(world.would_begin_at(ended + REST), [merge(1, 2)]);

    // The same, but somebody has it absorb region 3 a second before the rest ends:
    // it was merged last, by hand, so the split is first when it has rested again.
    let world = &mut merged_by_hand;
    let changes = world
        .coordinator
        .merge(world.now, region(1), region(3), ASKER)
        .expect("nothing speaks against the merge");
    world.take("merge", changes);
    world.merge_through(3);
    let ended = world.now;
    assert_eq!(but_for_prepare(world.run_up_to(ended + REST)), []);
    assert_eq!(
        world.would_begin_at(ended + REST),
        [split(1, 5, &[&[at(110)]])]
    );
}

// Section 5.1: "fewer than `AT_ONCE` merges and splits are under way, whoever asked
// for them"; and section 5.3: the split comes before the merges.
#[test]
fn merges_and_splits_by_hand_count_towards_the_four_under_way_and_a_split_has_the_last_place() {
    let mut world = World::rested(follow(), 12, &["a"]);
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(102, 0, 1)]);
    world.put(3, &[(200, 0, 2), (206, 0, 1)]);
    world.obedient = false;
    // Somebody asks for four merges of regions without players.
    for (survivor, absorbed) in [(4, 5), (6, 7), (8, 9), (10, 11)] {
        let changes = world
            .coordinator
            .merge(world.now, region(survivor), region(absorbed), ASKER)
            .expect("nothing speaks against the merge");
        world.take("merge", changes);
    }
    assert_eq!(world.under_way().len(), AT_ONCE);
    // The merge and the split have stood, and neither is begun.
    world.quiet_but_for_prepare(3 * FRESH);
    // One ends: there is room for one, and the split is first.
    world.merge_through(5);
    assert_eq!(world.step().begun, [split(3, 12, &[&[at(206)]])]);
    world.quiet(FRESH);
    world.merge_through(7);
    assert_eq!(world.step().begun, [merge(1, 2)]);
}

// Section 7: "A merge ends by a reading of the list, and that reading has what the
// survivor is pinned to from then on."
#[test]
fn a_part_that_absorbed_a_pinned_region_is_pinned_and_is_not_absorbed_for_being_empty() {
    for (unpinned, left) in [(vec![2], vec![0, 2]), (vec![1, 2], vec![0])] {
        let mut world = World::anew(follow(), 3);
        world.unpin(&unpinned);
        world.settle(&["a"]);
        world.rest();
        // Region 2, which is pinned to no area, has more players and absorbs
        // region 1.
        world.put(1, &[(100, 0, 1)]);
        world.put(2, &[(102, 0, 3)]);
        assert_eq!(world.next_begun(2 * FRESH).1, [merge(2, 1)]);
        world.quiet(LOOK);
        // Its players leave. It stays if region 1 was pinned, and is absorbed by the
        // home region if it was not.
        world.put(2, &[]);
        world.run(4 * EMPTY_FOR);
        assert_eq!(world.known(), left, "{}", world.story());
    }
}

// Section 4.4: the region that is absorbed has a sighting that is fresh.
#[test]
fn an_empty_region_whose_worker_is_silent_is_not_absorbed_until_it_is_reported_again() {
    let (mut world, first) = with_unpinned(3, &[2]);
    world.put(1, &[(100, 0, 1)]);
    let due = first + EMPTY_FOR;
    assert_eq!(world.run_to(due - REST), []);
    world.mute.insert(2);
    assert_eq!(world.run_to(due + REST), []);
    // Its run of reports without players was not broken by the silence.
    world.mute.clear();
    assert_eq!(world.step().begun, [merge(0, 2)]);
}

// Section 2.3: a report is passed over for its tick only if the sighting "is of this
// owner and epoch".
#[test]
fn the_report_of_a_new_owner_is_taken_although_its_tick_is_below_the_one_the_sighting_has() {
    let mut world = pair_of_three_workers();
    world.quiet(LOOK);
    let (_, changes) = world
        .coordinator
        .move_region(world.now, region(2), Some("d"), 7)
        .expect("the region can be moved");
    world.take("move_region", changes);
    world.let_go(2);
    let given = world.now;
    // The new owner's runner begins its ticks anew.
    world.reported.insert(2, 0);
    assert_eq!(world.run_up_to(given + REST), []);
    assert_eq!(
        world.around_instant(given + REST),
        from_the_instant_on(merge(1, 2))
    );
}

// ---------------------------------------------------------------------------------
// What the sections say that neither F nor K names.
// ---------------------------------------------------------------------------------

// Section 5.3: "Two groups of this tick can continue the same one, a group that has
// parted in two, and both then have its time."
#[test]
fn the_halves_of_a_group_that_parts_in_two_have_its_time() {
    let mut world = World::anew(follow(), 2);
    world.settle(&["a"]);
    let rests_until = world.now + REST;
    // One group in three chunks, each three from the next, which stands for seconds
    // while the region rests.
    world.put(1, &[(100, 0, 5), (110, 0, 1), (113, 0, 1), (116, 0, 1)]);
    assert_eq!(but_for_prepare(world.run_up_to(rests_until)), []);
    // In the last report before the rest ends, the player in the middle has left the
    // game: two groups, six apart.
    world.put(1, &[(100, 0, 5), (110, 0, 1), (116, 0, 1)]);
    assert_eq!(
        world.tick_at(rests_until).begun,
        [split(1, 2, &[&[at(110)], &[at(116)]])]
    );
}

// Section 5.3: the time begins anew "for a group that was the one to stay a tick ago".
#[test]
fn a_group_that_was_the_one_to_stay_a_tick_ago_begins_its_second_anew() {
    let mut world = World::rested(follow(), 2, &["a"]);
    world.put(1, &[(100, 0, 2), (110, 0, 1)]);
    assert_eq!(world.step().begun, [Begun::Prepare(1)]);
    world.quiet(FRESH);
    // In the report of the tick at which the group in chunk 110 has stood, it has
    // more players than the other: it is the one to stay, and the other, which was
    // to stay at every tick so far, is the one to go.
    world.put(1, &[(100, 0, 2), (110, 0, 3)]);
    assert_eq!(world.step().begun, []);
    let anew = world.now;
    world.quiet(3 * LOOK);
    assert_eq!(
        world.around_instant(anew + FRESH),
        after_the_instant(split(1, 2, &[&[at(100)]]))
    );
}

// Section 5.5: after an absorption that ends well the survivor's "counters, and
// whether it was split last, are as they were".
#[test]
fn an_absorption_that_ends_well_leaves_the_survivors_failures_as_they_were() {
    let (mut world, first) = with_unpinned(3, &[2]);
    let rests_until = world.now + REST;
    // Region 1's player stands near the chunk players enter in. The merge with the
    // home region is begun when both have rested, and comes to nothing.
    world.put(1, &[(2, 0, 1)]);
    world.obedient = false;
    assert_eq!(world.run_up_to(rests_until), []);
    assert_eq!(world.tick_at(rests_until).begun, [merge(0, 1)]);
    world.merge_off(1, Off::Busy);
    let failed = world.now;
    assert_eq!(world.alone_until(0), Some(failed + LONG));
    // The player walks off. Region 2 is due a little later, and the home region is
    // its survivor although it is left alone.
    world.put(1, &[(100, 0, 1)]);
    world.obedient = true;
    let due = first + EMPTY_FOR;
    assert!(due < failed + LONG);
    assert_eq!(world.run_up_to(due), []);
    assert_eq!(world.step().begun, [merge(0, 2)]);
    world.quiet(LOOK);
    assert_eq!(world.last_ended().1, merge_ended(0, 2, Ok(0)));
    assert_eq!(world.alone_until(0), Some(failed + LONG));
    // The player comes back, and the merge is begun when the home region is no
    // longer left alone. It comes to nothing again: that is its second failure in a
    // row, as the absorption was no merge that ended well for it.
    world.put(1, &[(2, 0, 1)]);
    world.obedient = false;
    assert_eq!(world.next_begun(LONG), (failed + LONG, vec![merge(0, 1)]));
    world.merge_off(1, Off::Busy);
    assert_eq!(world.alone_until(0), Some(world.now + 2 * LONG));
    assert_eq!(world.alone_until(1), Some(world.now + 2 * LONG));
}

// Section 5.5: "The counters go back to 0 only when a split of the region, or a merge
// it survived that was not an absorption, ends well".
#[test]
fn a_split_that_ends_well_makes_the_regions_next_failure_its_first_again() {
    let mut world = World::rested(follow(), 3, &["a"]);
    world.put(1, &[(100, 0, 5)]);
    world.put(2, &[(102, 0, 1)]);
    world.obedient = false;
    let (_, begun) = world.next_begun(2 * FRESH);
    assert_eq!(begun, [merge(1, 2)]);
    world.merge_off(2, Off::Busy);
    let failed = world.now;
    // Region 2's player walks off, and a group of region 1 parts. It is split off
    // when region 1 is no longer left alone.
    world.put(2, &[(300, 0, 1)]);
    world.join(1, 110, 1);
    world.obedient = true;
    assert_eq!(
        world.next_but_prepare(LONG),
        (failed + LONG, vec![split(1, 3, &[&[at(110)]])])
    );
    world.quiet(LOOK);
    assert_eq!(world.last_ended().1, split_ended(1, Ok(3)));
    // Region 2's player comes back; the merge is begun when region 1 has rested and
    // comes to nothing. For region 1 that is the first failure in a row, for region 2
    // the second.
    world.put(2, &[(102, 0, 1)]);
    world.obedient = false;
    let (_, begun) = world.next_begun(REST + FRESH);
    assert_eq!(begun, [merge(1, 2)]);
    world.merge_off(2, Off::Busy);
    assert_eq!(world.alone_until(1), Some(world.now + LONG));
    assert_eq!(world.alone_until(2), Some(world.now + 2 * LONG));
}

// Section 5.5: "The third such answer in a row is a failure".
#[test]
fn a_split_that_ends_well_begins_the_count_of_splits_that_found_nobody_anew() {
    let mut world = World::rested(follow(), 2, &["a"]);
    world.obedient = false;
    world.put(1, &[(100, 0, 5), (110, 0, 1)]);
    for _ in 0..2 {
        let (_, begun) = world.next_but_prepare(REST + 2 * FRESH);
        assert_eq!(begun, [split(1, 2, &[&[at(110)]])]);
        world.split_off(1, Off::Nobody);
        assert_eq!(world.alone_until(1), Some(world.now + REST));
    }
    // The third finds the group, and another parts afterwards.
    let (_, begun) = world.next_but_prepare(REST + 2 * FRESH);
    assert_eq!(begun, [split(1, 2, &[&[at(110)]])]);
    let (part, _) = world.split_through(1);
    assert_eq!(part, 2);
    world.join(1, 120, 1);
    for _ in 0..2 {
        let (_, begun) = world.next_but_prepare(REST + 2 * FRESH);
        assert_eq!(begun, [split(1, 3, &[&[at(120)]])]);
        world.split_off(1, Off::Nobody);
        assert_eq!(world.alone_until(1), Some(world.now + REST));
    }
    let (_, begun) = world.next_but_prepare(REST + 2 * FRESH);
    assert_eq!(begun, [split(1, 3, &[&[at(120)]])]);
    world.split_off(1, Off::Nobody);
    assert_eq!(world.alone_until(1), Some(world.now + LONG));
}

// Section 5.4: a rest is set "if that is later than what it has".
#[test]
fn a_rest_does_not_shorten_the_time_a_region_is_left_alone_for() {
    let mut world = a_merge_under_way();
    world.merge_off(2, Off::Busy);
    let failed = world.now;
    world.obedient = true;
    world.quiet(LEASE);
    // Region 1's worker reports it with an epoch the coordinator did not have.
    let mut holding = world.coordinator.assignments("a");
    holding[1].epoch = world.highest_epoch() + 5;
    world.register("a", &holding);
    assert_eq!(world.alone_until(1), Some(failed + LONG));
    // And again two seconds before that time is up: it rests from then.
    assert_eq!(world.run_to(failed + LONG - 2 * FRESH), []);
    let mut holding = world.coordinator.assignments("a");
    holding[1].epoch = world.highest_epoch() + 5;
    world.register("a", &holding);
    let reported = world.now;
    assert_eq!(world.alone_until(1), Some(reported + REST));
    assert_eq!(world.run_up_to(reported + REST), []);
    assert_eq!(
        world.around_instant(reported + REST),
        from_the_instant_on(merge(1, 2))
    );
}

// Section 2.3: of the crowds of a sighting "a count of 0 is left out".
#[test]
fn a_chunk_that_a_report_names_with_no_players_in_it_is_no_place() {
    // Region 2's reports name the chunk two from region 1's player, with nobody.
    let mut world = pair();
    world.put(2, &[(200, 0, 1)]);
    world.nobody_in.insert(2, vec![at(102)]);
    world.quiet(2 * REST);
    // A region whose reports name a chunk with nobody has no players, and so has
    // its survivor.
    let (mut world, first) = with_unpinned(3, &[2]);
    world.put(1, &[(100, 0, 1)]);
    world.nobody_in.insert(2, vec![at(500)]);
    world.nobody_in.insert(0, vec![at(0)]);
    let due = first + EMPTY_FOR;
    assert_eq!(world.run_up_to(due), []);
    assert_eq!(world.around_instant(due), from_the_instant_on(merge(0, 2)));
}

// Section 5.6: `Prepare` is said only while the list is no more than two leases old,
// and a split that is begun at the tick at which it is read again has none.
#[test]
fn a_region_is_not_told_to_prepare_while_the_last_good_reading_is_more_than_two_leases_old() {
    let mut world = World::rested(follow(), 2, &["a"]);
    let read = world.listed.expect("the list has been read");
    world.readings = Readings::Failing;
    assert_eq!(world.run_to(read + 2 * LEASE), []);
    world.put(1, &[(100, 0, 2), (106, 0, 1)]);
    assert_eq!(world.run_to(read + 3 * LEASE - LOOK), []);
    world.readings = Readings::AtOnce;
    assert_eq!(world.step().begun, []);
    assert_eq!(world.listed, Some(read + 3 * LEASE));
    assert_eq!(world.step().begun, [split(1, 2, &[&[at(106)]])]);
}

// Section 4: the chunk players enter in is that of the coordinator's spawn point.
#[test]
fn the_chunk_players_enter_in_is_where_the_coordinator_was_told_they_enter() {
    // Block 1608.5, -24.5 is in the chunk 100, -2.
    let mut world = World::anew_at(follow(), 3, Vec3::new(1608.5, 64.0, -24.5));
    world.settle(&["a"]);
    world.rest();
    world.put(1, &[(2, 0, 1)]);
    world.put(2, &[(102, -4, 1)]);
    assert_eq!(world.next_begun(2 * FRESH).1, [merge(0, 2)]);
    world.quiet(2 * REST);
    assert_eq!(world.known(), [0, 1]);
}

// Section 5.5: "When an absorption ends, well or not, that is noted of its survivor."
#[test]
fn a_survivor_whose_first_report_after_an_absorption_that_failed_has_a_player_rests_from_that_report()
 {
    let (mut world, _) = the_home_region_absorbs();
    world.obedient = false;
    // Somebody came into the home region as the absorption began, and it comes to
    // nothing.
    world.put(0, &[(0, 0, 1)]);
    world.put(1, &[(2, 0, 1)]);
    world.merge_off(2, Off::Busy);
    world.obedient = true;
    world.quiet(LOOK);
    let reported = world.now - (LOOK - LAG);
    assert_eq!(world.alone_until(0), Some(reported + REST));
    assert_eq!(world.run_up_to(reported + REST), []);
    assert_eq!(
        world.around_instant(reported + REST),
        from_the_instant_on(merge(0, 1))
    );
}

/// Region 1 was split by itself and rests until the time returned. While it rests, a
/// second group of it parts and region 2 comes near the players who stay: when the
/// rest ends, it was split last and a merge of it has stood.
fn split_last_with_a_merge_that_has_stood() -> (World, Instant) {
    let mut world = World::rested(follow(), 5, &["a"]);
    world.put(1, &[(100, 0, 5), (110, 0, 1)]);
    world.put(2, &[(300, 0, 1)]);
    world.put(3, &[(400, 0, 3)]);
    world.obedient = false;
    world.quiet_but_for_prepare(FRESH + LOOK);
    assert_eq!(world.step().begun, [split(1, 5, &[&[at(110)]])]);
    let (part, _) = world.split_through(1);
    assert_eq!(part, 5);
    let rests_until = world.now + REST;
    world.obedient = true;
    world.join(1, 120, 1);
    world.put(2, &[(98, 0, 1)]);
    (world, rests_until)
}

// Section 5.3, "The order": the split is of the regions that "are not passed over
// for their turn": when the region whose group has gone longest is passed over,
// another region is split at that tick.
#[test]
fn a_region_that_is_passed_over_for_its_turn_leaves_the_one_split_to_another_region() {
    let (mut world, rests_until) = split_last_with_a_merge_that_has_stood();
    // A group of region 3 is first seen a second and a look before the rest ends: it
    // has stood, for the first time, at the tick that ends it.
    assert_eq!(
        but_for_prepare(world.run_to(rests_until - FRESH - 2 * LOOK)),
        []
    );
    world.join(3, 406, 1);
    assert_eq!(but_for_prepare(world.run_up_to(rests_until)), []);
    assert_eq!(
        world.tick_at(rests_until).begun,
        [merge(1, 2), split(3, 6, &[&[at(406)]])]
    );
}

// Section 5.3, "Turns": a region is passed over only for a merge "whose two regions
// are free".
#[test]
fn a_region_that_was_split_last_is_split_again_if_the_other_region_of_its_merge_is_not_free() {
    let (mut world, rests_until) = split_last_with_a_merge_that_has_stood();
    // Three seconds before the rest ends, region 2's worker reports it with an
    // epoch the coordinator did not have: it rests for seven seconds longer.
    assert_eq!(but_for_prepare(world.run_to(rests_until - 3 * FRESH)), []);
    let mut holding = world.coordinator.assignments("a");
    let at_it = holding
        .iter()
        .position(|held| held.region == region(2))
        .expect("the worker runs region 2");
    holding[at_it].epoch = world.highest_epoch() + 5;
    world.register("a", &holding);
    let reported = world.now;
    assert_eq!(but_for_prepare(world.run_up_to(rests_until)), []);
    // The merge has stood again and cannot be begun: it is the split that is.
    assert_eq!(
        world.tick_at(rests_until).begun,
        [split(1, 6, &[&[at(120)]])]
    );
    // And the merge when both have rested: region 1 from the split, which the
    // workers make at their next look.
    assert!(rests_until + LAG + REST > reported + REST);
    assert_eq!(
        but_for_prepare(world.run_up_to(rests_until + LAG + REST)),
        []
    );
    assert_eq!(
        world.would_begin_at(rests_until + LAG + REST),
        [merge(1, 2)]
    );
}

// Section 5.3, "Waiting": the merge of the other region with the survivor "takes the
// earlier of the two times if it has one".
#[test]
fn a_merge_that_waited_for_both_regions_of_a_merge_waits_since_the_earlier_of_the_two_times() {
    let mut world = World::anew(follow(), 5);
    world.settle(&["a"]);
    let rests_until = world.now + REST;
    // Region 1 has five players and absorbs region 2, two chunks east of it.
    world.put(1, &[(98, 0, 5)]);
    world.put(2, &[(100, 0, 1)]);
    world.put(3, &[(300, 0, 1)]);
    world.put(4, &[(400, 0, 1)]);
    world.quiet(2 * LOOK);
    // Region 4 comes near region 2; then region 3 near region 1; then a second
    // player of region 4 near region 1. Regions 3 and 4 are not near each other.
    world.put(4, &[(102, 0, 1)]);
    world.quiet(2 * LOOK);
    world.put(3, &[(96, -1, 1)]);
    world.quiet(2 * LOOK);
    world.put(4, &[(102, 0, 1), (98, 2, 1)]);
    assert_eq!(world.run_up_to(rests_until), []);
    assert_eq!(world.tick_at(rests_until).begun, [merge(1, 2)]);
    let ended = rests_until + LAG;
    // Region 4 has waited for region 1 since later than region 3 has, and for the
    // region that region 1 absorbed since earlier: it is first, though both are as
    // near and region 3 has the lower id.
    assert_eq!(world.run_up_to(ended + REST), []);
    assert_eq!(
        world.around_instant(ended + REST),
        from_the_instant_on(merge(1, 4))
    );
}

// Section 5.3: the time begins anew "for two groups that have come together".
#[test]
fn two_groups_that_have_come_together_begin_their_second_anew() {
    let mut world = World::anew(follow(), 2);
    world.settle(&["a"]);
    let rests_until = world.now + REST;
    // Two groups, six apart, that stand for seconds while the region rests.
    world.put(1, &[(100, 0, 5), (110, 0, 1), (116, 0, 1)]);
    assert_eq!(but_for_prepare(world.run_up_to(rests_until)), []);
    // In the last report before the rest ends, a player stands between them: one
    // group, of which a chunk is further than the margin from either as it was.
    world.join(1, 113, 1);
    assert_eq!(world.tick_at(rests_until).begun, []);
    world.quiet(3 * LOOK);
    assert_eq!(
        world.around_instant(rests_until + FRESH),
        after_the_instant(split(1, 2, &[&[at(110), at(113), at(116)]]))
    );
}

// Section 5.3, "The order": the merges that have stood come before the absorptions
// when there is room for one more only.
#[test]
fn a_merge_that_has_stood_has_the_last_place_before_an_absorption() {
    let (mut world, first) = with_unpinned(12, &[11]);
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(200, 0, 1)]);
    world.obedient = false;
    let due = first + EMPTY_FOR;
    assert_eq!(world.run_to(due - 2 * FRESH), []);
    // Two seconds before region 11 is due, somebody asks for four merges, and
    // region 2's player comes near region 1's.
    for (survivor, absorbed) in [(3, 4), (5, 6), (7, 8), (9, 10)] {
        let changes = world
            .coordinator
            .merge(world.now, region(survivor), region(absorbed), ASKER)
            .expect("nothing speaks against the merge");
        world.take("merge", changes);
    }
    world.put(2, &[(102, 0, 1)]);
    assert_eq!(world.run_to(due + FRESH), []);
    // One of the four ends, with the home region as a survivor for region 11 and the
    // merge of the regions 1 and 2 stood: the merge is begun.
    world.merge_through(4);
    assert_eq!(world.step().begun, [merge(1, 2)]);
    world.quiet(LOOK);
    world.merge_through(6);
    assert_eq!(world.step().begun, [merge(0, 11)]);
}

// Section 5.5: "The first report that is taken of the survivor after that decides":
// one that is passed over, because the region stands still, is not it.
#[test]
fn a_report_of_the_survivor_that_is_passed_over_is_not_the_first_after_the_absorption() {
    let (mut world, alone) = the_home_region_absorbs();
    // The home region waits for the store when it has absorbed, and its reports
    // repeat the tick they had, for two seconds.
    world.still.insert(0);
    world.quiet(2 * FRESH);
    assert_eq!(world.last_ended().1, merge_ended(0, 2, Ok(0)));
    assert_eq!(world.alone_until(0), alone);
    // It ticks on, and its report has a player who came as it absorbed.
    world.still.clear();
    world.put(0, &[(0, 0, 1)]);
    world.put(1, &[(2, 0, 1)]);
    world.quiet(LOOK);
    let reported = world.now - (LOOK - LAG);
    assert_eq!(world.alone_until(0), Some(reported + REST));
    assert_eq!(world.run_up_to(reported + REST), []);
    assert_eq!(
        world.around_instant(reported + REST),
        from_the_instant_on(merge(0, 1))
    );
}

// Section 6: evening out "runs after what section 5 begins in the same tick", and
// begins nothing while a merge is under way: an absorption is one.
#[test]
fn nothing_is_evened_out_at_the_tick_that_begins_an_absorption() {
    let (mut world, first) = with_unpinned(3, &[2]);
    world.put(1, &[(100, 0, 1)]);
    world.obedient = false;
    let due = first + EMPTY_FOR;
    assert_eq!(world.run_up_to(due), []);
    // A worker that runs nothing registers a moment before the tick at which region
    // 2 is due.
    world.now = due - MOMENT;
    world.register("b", &[]);
    assert_eq!(world.tick_at(due).begun, [merge(0, 2)]);
    world.quiet(2 * FRESH);
    // When it has ended, the one worker has two regions and the other none. The
    // survivor does not rest after an absorption and has no players, so it is the
    // one that is moved, at the next tick.
    world.merge_through(2);
    world.obedient = true;
    assert_eq!(world.step().begun, [Begun::Move(0)]);
}

// Section 7: the timer counts from "the last answer, `listed` or `unlisted`", also
// from a reading that the service made by itself and that `tick` did not ask for.
#[test]
fn the_list_is_asked_for_a_lease_after_a_reading_that_nobody_asked_the_coordinator_for() {
    let mut world = World::rested(follow(), 2, &["a"]);
    let (read, reads) = (world.listed.expect("the list has been read"), world.reads);
    assert_eq!(world.run_to(read + 2 * FRESH), []);
    world.now += LAG;
    world.hand_in();
    let handed_in = world.now;
    world.now += LOOK - LAG;
    world.tick();
    assert_eq!(world.run_up_to(handed_in + LEASE), []);
    assert_eq!(world.reads, reads);
    world.quiet(LOOK);
    assert_eq!(world.reads, reads + 1);
}

// Section 3: how often `tick` is to be called.
#[test]
fn a_coordinator_that_decides_by_itself_is_to_be_ticked_four_times_a_second() {
    assert_eq!(Coordinator::LOOK, Duration::from_millis(250));
}

// Section 2.4, the first row of the table: when a merge or a split of the region
// begins, whoever asked, its `empty_since` is forgotten.
#[test]
fn an_empty_region_of_which_somebody_asked_for_a_split_begins_its_three_rests_anew() {
    let (mut world, first) = with_unpinned(3, &[2]);
    world.put(1, &[(100, 0, 1)]);
    let due = first + EMPTY_FOR;
    assert_eq!(world.run_to(due - LEASE), []);
    // Five seconds before it is due, somebody asks for a split of it, which finds
    // nobody at the workers' next look.
    let changes = world
        .coordinator
        .split(world.now, region(2), &[at(500)], ASKER)
        .expect("nothing speaks against the split");
    world.take("split", changes);
    world.quiet(LOOK);
    assert_eq!(world.last_ended().1.outcome, Err(Undone::Off(Off::Nobody)));
    // Its first report after that is the first of a new run without players.
    let anew = world.now - (LOOK - LAG);
    assert_eq!(world.run_up_to(anew + EMPTY_FOR), []);
    assert_eq!(
        world.around_instant(anew + EMPTY_FOR),
        from_the_instant_on(merge(0, 2))
    );
}

// Section 2.4, the fifth row of the table: `prepared` is forgotten when the region is
// given an epoch that the coordinator did not have for it.
#[test]
fn a_region_that_is_given_an_epoch_is_told_to_prepare_again_before_its_split() {
    let mut world = apart();
    assert_eq!(world.step().begun, [Begun::Prepare(1)]);
    let mut holding = world.coordinator.assignments("a");
    holding[1].epoch = world.highest_epoch() + 5;
    world.register("a", &holding);
    let reported = world.now;
    assert_eq!(world.run_up_to(reported + REST - FRESH), []);
    assert_eq!(
        world.around_instant(reported + REST - FRESH),
        from_the_instant_on(Begun::Prepare(1))
    );
    assert_eq!(
        world.tick_at(reported + REST - FRESH).begun,
        [Begun::Prepare(1)]
    );
    assert_eq!(world.run_up_to(reported + REST), []);
    assert_eq!(
        world.around_instant(reported + REST),
        from_the_instant_on(split(1, 3, &[&[at(106)]]))
    );
}

// Section 2.4, the second row of the table: the crowds of a region that a reading
// shows absorbed go to "the living region it went into, by the pairs of that
// reading", which may be two merges on.
#[test]
fn the_crowds_of_a_region_that_was_absorbed_twice_over_count_as_those_of_the_region_that_lives() {
    let mut world = World::rested(follow(), 5, &["a"]);
    world.put(1, &[(300, 0, 3)]);
    world.put(2, &[(400, 0, 1)]);
    world.put(3, &[(110, 0, 1), (115, 0, 1)]);
    world.put(4, &[(108, 0, 1), (117, 0, 1)]);
    // One look, at which regions 3 and 4 are one cluster. Then a reading has region
    // 3 absorbed by region 2 and region 2 by region 1, of which nobody told the
    // coordinator, and region 1's worker says nothing of it afterwards.
    world.quiet(LOOK);
    world.merged(2, 3);
    world.merged(1, 2);
    world.hand_in();
    world.mute.insert(1);
    assert_eq!(world.known(), [0, 1, 4]);
    // The players between region 4's are in region 1's sighting and still join
    // them: no split of region 4 is wanted, neither while that sighting is fresh,
    // for which it is region 1 that is told to prepare, nor after.
    let began = world.run(3 * FRESH);
    assert!(
        !began.iter().any(|begun| matches!(
            begun,
            Begun::Prepare(4) | Begun::Split { region: 4, .. } | Begun::Merge { .. }
        )),
        "{began:?}"
    );
    // Region 1 reports them elsewhere, and region 4 is to be split.
    world.mute.clear();
    world.put(1, &[(300, 0, 6)]);
    assert_eq!(world.step().begun, [Begun::Prepare(4)]);
}

// K8.
#[test]
fn while_the_store_is_away_nothing_is_begun_and_what_was_wanted_stands_anew_when_it_is_back() {
    let mut world = World::rested(follow(), 4, &["a"]);
    world.put(1, &[(100, 0, 1)]);
    world.put(2, &[(102, 0, 1)]);
    world.put(3, &[(200, 0, 2), (206, 0, 1)]);
    // From the first look on the store is away: every region waits for it and
    // stands still, and no reading of the list succeeds.
    assert_eq!(world.step().begun, [Begun::Prepare(3)]);
    world.still.extend([0, 1, 2, 3]);
    world.readings = Readings::Failing;
    world.quiet(4 * LEASE);
    // It is back: the regions tick on and report, and the list is read again within
    // a lease. The merge and the split have stood a second and a look after the
    // first tick at which every region is fresh and the list is no more than two
    // leases old.
    world.still.clear();
    world.readings = Readings::AtOnce;
    let read = world.listed;
    while world.listed == read {
        assert_eq!(world.step().begun, []);
    }
    let back = world.now;
    let began = world.run(2 * FRESH);
    assert_eq!(
        began,
        [merge(1, 2), split(3, 4, &[&[at(206)]])],
        "{}",
        world.story()
    );
    assert!(world.began.last().expect("something was begun").0 > back - world.made);
}

// K9, K21: "a hundred that each come near one and the same region, and not near each
// other, are taken one every ten seconds, in the order in which they came."
#[test]
fn regions_that_come_near_one_region_are_taken_in_one_in_a_rest_in_the_order_in_which_they_came() {
    let mut world = World::anew(follow(), 7);
    world.settle(&["a"]);
    // Region 1 has twenty players in a row of chunks, four apart.
    world.put(
        1,
        &[
            (100, 0, 4),
            (104, 0, 4),
            (108, 0, 4),
            (112, 0, 4),
            (116, 0, 4),
        ],
    );
    for id in 2..7 {
        world.put(id, &[(1000 * id as i32, 0, 1)]);
    }
    // The others come two chunks from one of them each, four from each other, one
    // every two looks, and not in the order of their ids.
    let came = [5, 3, 6, 2, 4];
    for (place, id) in came.into_iter().enumerate() {
        let z = if place % 2 == 0 { 2 } else { -2 };
        world.put(id, &[(100 + 4 * place as i32, z, 1)]);
        world.quiet(2 * LOOK);
    }
    let mut began: Vec<(Instant, Begun)> = Vec::new();
    while began.len() < came.len() {
        assert!(world.clock() < 8 * REST, "{}", world.story());
        for begun in world.step().begun {
            began.push((world.now, begun));
        }
    }
    let order: Vec<Begun> = began.iter().map(|(_, begun)| begun.clone()).collect();
    let expected: Vec<Begun> = came.iter().map(|id| merge(1, *id)).collect();
    assert_eq!(order, expected);
    for pair in began.windows(2) {
        assert!(pair[1].0 >= pair[0].0 + REST);
    }
}

// ---------------------------------------------------------------------------------
// Long scripted sequences: with `follow: None` nothing in them begins anything, and
// the same calls give the same answers.
// ---------------------------------------------------------------------------------

/// A coordinator made with `policy` whose every answer is kept in words, for a world
/// of stripes that the workers were just given.
fn recorded(
    policy: Option<Policy>,
    regions: u32,
    unpinned: &[u32],
    workers: &[&str],
    backwards: bool,
) -> World {
    let mut world = World::anew(policy, regions);
    world.told = Some(Vec::new());
    world.backwards = backwards;
    world.unpin(unpinned);
    world.settle(workers);
    world
}

/// Regions gather around one, which is then left by groups in three directions.
fn gathering_and_parting(policy: Option<Policy>, backwards: bool) -> World {
    let seconds = Duration::from_secs;
    let mut world = recorded(policy, 5, &[], &["a"], backwards);
    world.put(1, &[(100, 0, 3), (100, 1, 1)]);
    world.put(2, &[(102, 0, 1)]);
    world.put(3, &[(98, -1, 1)]);
    world.put(4, &[(300, 0, 2), (303, 2, 1)]);
    world.run(seconds(45));
    world.join(1, 110, 1);
    world.run(seconds(3));
    world.join(1, 90, 2);
    world
        .crowds
        .entry(1)
        .or_default()
        .insert(ChunkPos::new(100, 12), 1);
    world.run(seconds(60));
    world
}

/// Regions without players, some pinned to no area, and a second worker that
/// registers late; a player comes to the chunk players enter in and leaves again.
fn clearing_away(policy: Option<Policy>, backwards: bool) -> World {
    let seconds = Duration::from_secs;
    let mut world = recorded(policy, 7, &[2, 3, 4, 5], &["a"], backwards);
    world.put(6, &[(300, 0, 1)]);
    world.run(seconds(20));
    world.register("b", &[]);
    world.run(seconds(25));
    world.put(6, &[(1, 1, 1)]);
    world.run(seconds(20));
    world.put(0, &[]);
    world.put(6, &[]);
    world.run(seconds(80));
    world
}

/// A player who flies back and forth across the band between the distances, in
/// whichever region has them.
fn flying(policy: Option<Policy>, backwards: bool) -> World {
    let mut world = recorded(policy, 2, &[], &["a"], backwards);
    for look in 0..400 {
        let x = if look % 16 < 8 { 102 } else { 107 };
        let parts: Vec<u32> = world.known().into_iter().filter(|id| *id > 1).collect();
        match parts.as_slice() {
            [] => world.put(1, &[(100, 0, 3), (x, 0, 1)]),
            [part] => world.put(*part, &[(x, 0, 1)]),
            several => panic!("more than one part: {several:?}"),
        }
        world.step();
    }
    world
}

/// Two regions to merge and one to split, while the store is away for a while, a
/// worker dies and another takes its place.
fn through_faults(policy: Option<Policy>, backwards: bool) -> World {
    let seconds = Duration::from_secs;
    let mut world = recorded(policy, 4, &[], &["a", "b"], backwards);
    world.put(1, &[(100, 0, 2)]);
    world.put(2, &[(102, 0, 1)]);
    world.put(3, &[(200, 0, 2), (206, 0, 1)]);
    world.run(seconds(8));
    world.readings = Readings::Failing;
    world.run(seconds(14));
    let changes = world.coordinator.disconnected(world.now, "b");
    world.take("disconnected", changes);
    world.silence("b");
    world.run(seconds(4));
    world.readings = Readings::AtOnce;
    world.register("c", &[]);
    world.run(seconds(70));
    world
}

/// A scripted sequence: what the players and the workers do, for a coordinator made
/// with the policy given, and with the crowds of its reports backwards or not.
type Script = fn(Option<Policy>, bool) -> World;

/// The scripted sequences, each with how many merges and splits it is to bring about
/// at least when the coordinator decides by itself.
fn scripts() -> Vec<(&'static str, Script, usize, usize)> {
    vec![
        ("gathering and parting", gathering_and_parting, 2, 2),
        ("clearing away", clearing_away, 3, 0),
        ("flying", flying, 2, 2),
        ("through faults", through_faults, 1, 1),
    ]
}

// What the tests below rest on: the sequences are of things that a coordinator
// which decides by itself does begin.
#[test]
fn a_coordinator_that_decides_by_itself_merges_and_splits_in_the_scripted_sequences() {
    for (name, script, at_least_merges, at_least_splits) in scripts() {
        let world = script(follow(), false);
        let merges = world
            .began
            .iter()
            .filter(|(_, begun)| matches!(begun, Begun::Merge { .. }))
            .count();
        let splits = world
            .began
            .iter()
            .filter(|(_, begun)| matches!(begun, Begun::Split { .. }))
            .count();
        assert!(merges >= at_least_merges, "{name}\n{}", world.story());
        assert!(splits >= at_least_splits, "{name}\n{}", world.story());
        assert!(
            !world
                .began
                .iter()
                .any(|(_, begun)| matches!(begun, Begun::Other(_))),
            "{name}\n{}",
            world.story()
        );
        // Nothing is left under way, and everything that ended was the coordinator's.
        assert_eq!(world.under_way(), [], "{name}");
        assert!(
            world.ended.iter().all(|(_, ended)| ended.asker.is_none()),
            "{name}"
        );
    }
}

// Section 5: "With `follow: None` it does none of this, and nothing of sections 6
// and 7."
#[test]
fn a_coordinator_that_decides_nothing_by_itself_begins_nothing_in_the_scripted_sequences() {
    for (name, script, _, _) in scripts() {
        let world = script(None, false);
        // Evening out is not asked for by anybody either, and is done as it was.
        let began: Vec<&Begun> = world
            .began
            .iter()
            .map(|(_, begun)| begun)
            .filter(|begun| !matches!(begun, Begun::Move(_)))
            .collect();
        assert!(began.is_empty(), "{name}: {began:?}");
        assert_eq!(world.ended, [], "{name}");
        assert_eq!(world.reads, 0, "{name}");
        assert_eq!(world.under_way(), [], "{name}");
        for id in world.known() {
            assert_eq!(world.alone_until(id), None, "{name}");
        }
        assert_eq!(world.table().absorbed, [], "{name}");
    }
}

// R6.
#[test]
fn the_same_calls_give_the_same_answers() {
    for (name, script, _, _) in scripts() {
        let (once, again) = (script(follow(), false), script(follow(), false));
        assert!(once.told.as_ref().is_some_and(|told| told.len() > 300));
        assert_eq!(once.told, again.told, "{name}");
        assert_eq!(once.began, again.began, "{name}");
        assert_eq!(once.ended, again.ended, "{name}");
        assert_eq!(once.table(), again.table(), "{name}");
    }
}

// R6.
#[test]
fn crowds_that_are_given_in_another_order_give_the_same_answers() {
    for (name, script, _, _) in scripts() {
        let (forwards, backwards) = (script(follow(), false), script(follow(), true));
        assert_eq!(forwards.told, backwards.told, "{name}");
        assert_eq!(forwards.table(), backwards.table(), "{name}");
    }
}

// The same calls give the same answers also from the middle: a coordinator that is
// copied goes on as the one it was copied from.
#[test]
fn a_copy_of_a_coordinator_goes_on_as_the_one_it_was_copied_from() {
    let mut world = World::rested(follow(), 5, &["a"]);
    world.told = Some(Vec::new());
    world.put(1, &[(100, 0, 3), (110, 0, 1)]);
    world.put(2, &[(102, 0, 1)]);
    world.put(3, &[(98, 0, 1)]);
    world.run(3 * FRESH);
    let mut copy = world.clone();
    world.run(6 * REST);
    copy.run(6 * REST);
    assert!(world.began.len() >= 4, "{}", world.story());
    assert_eq!(world.told, copy.told);
    assert_eq!(world.table(), copy.table());
}

// ---------------------------------------------------------------------------------
// What the coordinator begins by itself goes down the paths of what is asked by hand
// (section 1, point 8, and section 10).
// ---------------------------------------------------------------------------------

/// The changes without who asked.
fn whoever_asked(mut changes: Changes) -> Changes {
    for ended in &mut changes.reshaped {
        ended.asker = None;
    }
    changes
}

// Section 1, point 8: what the workers are told, what their answers do, and the
// routing table and the rests afterwards.
#[test]
fn a_merge_begun_by_itself_goes_the_way_of_one_asked_for_by_hand() {
    for done in [true, false] {
        let mut itself = pair();
        itself.obedient = false;
        itself.quiet(FRESH + LOOK);
        // At the next tick the merge has stood. In a twin, somebody asks for it at
        // that very instant.
        itself.now += LAG;
        itself.report();
        itself.now += LOOK - LAG;
        itself.beat();
        let mut by_hand = itself.clone();
        let begun = itself.coordinator.tick(itself.now);
        let asked = by_hand
            .coordinator
            .merge(by_hand.now, region(1), region(2), ASKER)
            .expect("nothing speaks against the merge");
        assert_eq!(begun.releases.len(), 1);
        assert_eq!(begun.orders.len(), 1);
        assert_eq!(begun.releases, asked.releases);
        assert_eq!(begun.orders, asked.orders);
        assert_eq!(begun.workers, asked.workers);
        assert_eq!(begun.routing, asked.routing);
        assert_eq!(itself.under_way(), by_hand.under_way());
        itself.take("tick", begun);
        by_hand.take("merge", asked);
        assert_eq!(by_hand.tick().begun, []);
        // The workers answer both alike.
        assert_eq!(itself.let_go(2), by_hand.let_go(2));
        assert_eq!(itself.table(), by_hand.table());
        let (ended, asked_ended) = if done {
            (itself.absorb(2), by_hand.absorb(2))
        } else {
            (
                itself.absorb_off(2, Off::TooLarge),
                by_hand.absorb_off(2, Off::TooLarge),
            )
        };
        assert_eq!(ended.reshaped.len(), 1);
        assert_eq!(ended.reshaped[0].asker, None);
        assert_eq!(asked_ended.reshaped[0].asker, ASKER);
        assert_eq!(ended, whoever_asked(asked_ended));
        assert_eq!(itself.table(), by_hand.table());
        assert_eq!(itself.known(), by_hand.known());
        for id in itself.known() {
            assert_eq!(itself.alone_until(id), by_hand.alone_until(id));
        }
        // And both go on alike.
        itself.obedient = true;
        by_hand.obedient = true;
        itself.told = Some(Vec::new());
        by_hand.told = Some(Vec::new());
        itself.run(2 * LONG);
        by_hand.run(2 * LONG);
        assert_eq!(itself.told, by_hand.told);
    }
}

// Section 1, point 8.
#[test]
fn a_split_begun_by_itself_goes_the_way_of_one_asked_for_by_hand() {
    for answer in [None, Some(Off::Nobody), Some(Off::StoreLost)] {
        let mut itself = apart();
        itself.obedient = false;
        itself.quiet_but_for_prepare(FRESH + LOOK);
        itself.now += LAG;
        itself.report();
        itself.now += LOOK - LAG;
        itself.beat();
        let mut by_hand = itself.clone();
        let begun = itself.coordinator.tick(itself.now);
        let asked = by_hand
            .coordinator
            .split(by_hand.now, region(1), &around(&[&[at(106)]]), ASKER)
            .expect("nothing speaks against the split");
        assert_eq!(begun.orders.len(), 1);
        assert_eq!(begun.orders, asked.orders);
        assert_eq!(begun.releases, asked.releases);
        assert_eq!(begun.workers, asked.workers);
        assert_eq!(begun.routing, asked.routing);
        assert_eq!(itself.under_way(), by_hand.under_way());
        itself.take("tick", begun);
        by_hand.take("split", asked);
        assert_eq!(by_hand.tick().begun, []);
        let (ended, asked_ended) = match answer {
            None => (itself.split_through(1).1, by_hand.split_through(1).1),
            Some(why) => (itself.split_off(1, why), by_hand.split_off(1, why)),
        };
        assert_eq!(ended.reshaped.len(), 1);
        assert_eq!(ended.reshaped[0].asker, None);
        assert_eq!(ended, whoever_asked(asked_ended));
        assert_eq!(itself.table(), by_hand.table());
        assert_eq!(itself.known(), by_hand.known());
        for id in itself.known() {
            assert_eq!(itself.alone_until(id), by_hand.alone_until(id));
        }
        itself.obedient = true;
        by_hand.obedient = true;
        itself.told = Some(Vec::new());
        by_hand.told = Some(Vec::new());
        itself.run(2 * LONG);
        by_hand.run(2 * LONG);
        assert_eq!(itself.told, by_hand.told);
    }
}

// ---------------------------------------------------------------------------------
// The lines of the coordinator's log (section 10).
// ---------------------------------------------------------------------------------

/// The lines of the log that are written while a test runs, on its thread.
mod log {
    use std::fmt::Debug;
    use std::sync::{Arc, Mutex};

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};

    /// Each line with the module it was written under and its level.
    #[derive(Clone, Default)]
    pub struct Lines(Arc<Mutex<Vec<(String, Level, String)>>>);

    impl Lines {
        /// The lines with this message that were written with `info!` under the
        /// module: each as the message and then its fields in the order they were
        /// given, `name=value` with a space between.
        pub fn of(&self, module: &str, message: &str) -> Vec<String> {
            let lines = self
                .0
                .lock()
                .expect("no test panics while it holds the lines");
            lines
                .iter()
                .filter(|(target, level, line)| {
                    target == module
                        && *level == Level::INFO
                        && (line == message || line.starts_with(&format!("{message} ")))
                })
                .map(|(_, _, line)| line.clone())
                .collect()
        }
    }

    #[derive(Default)]
    struct Line {
        message: String,
        fields: Vec<String>,
    }

    impl Line {
        fn put(&mut self, field: &Field, value: String) {
            if field.name() == "message" {
                self.message = value;
            } else {
                self.fields.push(format!("{}={value}", field.name()));
            }
        }
    }

    impl Visit for Line {
        fn record_str(&mut self, field: &Field, value: &str) {
            self.put(field, value.to_owned());
        }

        fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
            self.put(field, format!("{value:?}"));
        }
    }

    impl Subscriber for Lines {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }

        fn record(&self, _: &Id, _: &Record<'_>) {}

        fn record_follows_from(&self, _: &Id, _: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut line = Line::default();
            event.record(&mut line);
            let mut words = vec![line.message];
            words.extend(line.fields);
            let metadata = event.metadata();
            self.0
                .lock()
                .expect("no test panics while it holds the lines")
                .push((
                    metadata.target().to_owned(),
                    *metadata.level(),
                    words.join(" "),
                ));
        }

        fn enter(&self, _: &Id) {}

        fn exit(&self, _: &Id) {}
    }
}

/// The module that the coordinator's lines are written under.
const STATE: &str = "clustine_coordinator::state";

/// The lines that are written while `what` runs.
///
/// The tests of this file run side by side, and all but these few without anybody
/// who listens. Whether a line has a listener is remembered where it is written, from
/// the first time it is written by any thread, so `what` is run once for nothing:
/// after that every line of it is known, and what is remembered of each is worked
/// out anew with this test listening.
fn lines_of(what: impl Fn()) -> log::Lines {
    tracing::subscriber::with_default(log::Lines::default(), &what);
    let lines = log::Lines::default();
    tracing::subscriber::with_default(lines.clone(), || {
        tracing::callsite::rebuild_interest_cache();
        what();
    });
    lines
}

// Section 10: the first line of the table.
#[test]
fn a_merge_that_is_begun_by_the_distances_is_a_line_of_the_log() {
    let lines = lines_of(|| {
        let mut world = World::rested(follow(), 3, &["a"]);
        world.put(1, &[(100, 0, 3)]);
        world.put(2, &[(101, 2, 1)]);
        assert_eq!(world.run(2 * FRESH), [merge(1, 2)]);
    });
    assert_eq!(
        lines.of(STATE, "a merge is begun by the distances"),
        ["a merge is begun by the distances survivor=1 absorbed=2 gap=2"]
    );
    // The line that `merge` writes for whoever asked is written for it as well.
    assert_eq!(lines.of(STATE, "a region is to absorb another").len(), 1);
    assert_eq!(lines.of(STATE, "an absorption is begun by itself"), [""; 0]);
    assert_eq!(lines.of(STATE, "a split is begun by itself"), [""; 0]);
}

// Section 10: the second line of the table.
#[test]
fn an_absorption_is_a_line_of_the_log() {
    let lines = lines_of(|| {
        let (mut world, _) = with_unpinned(4, &[3]);
        world.put(1, &[(100, 0, 1)]);
        world.put(2, &[(200, 0, 1)]);
        assert_eq!(world.run(EMPTY_FOR + FRESH), [merge(0, 3)]);
    });
    assert_eq!(
        lines.of(STATE, "an absorption is begun by itself"),
        ["an absorption is begun by itself survivor=0 absorbed=3"]
    );
    assert_eq!(lines.of(STATE, "a region is to absorb another").len(), 1);
    assert_eq!(
        lines.of(STATE, "a merge is begun by the distances"),
        [""; 0]
    );
}

// Section 10: the third line of the table.
#[test]
fn a_split_that_is_begun_by_itself_is_a_line_of_the_log() {
    let lines = lines_of(|| {
        let mut world = World::rested(follow(), 2, &["a"]);
        world.put(1, &[(100, 0, 3), (110, 0, 1), (120, 0, 1), (121, 0, 1)]);
        assert_eq!(
            but_for_prepare(world.run(2 * FRESH)),
            [split(1, 2, &[&[at(110)], &[at(120), at(121)]])]
        );
    });
    // Two groups, of one chunk and of two: 25 chunks and 30.
    assert_eq!(
        lines.of(STATE, "a split is begun by itself"),
        ["a split is begun by itself region=1 part=2 groups=2 chunks=55"]
    );
    assert_eq!(lines.of(STATE, "a region is to be split").len(), 1);
}

// Section 10: the fourth line of the table, which is "left as it is, with `follow`
// and without".
#[test]
fn a_release_to_even_out_is_a_line_of_the_log_with_follow_and_without() {
    for policy in [follow(), None] {
        let lines = lines_of(|| {
            let mut world = one_worker_has_everything(policy, &[&[(0, 0, 1)], &[], &[]], &["b"]);
            assert_eq!(world.step().begun, [Begun::Move(2)]);
        });
        assert_eq!(
            lines.of(STATE, "a region is moved to even regions out"),
            ["a region is moved to even regions out region=2 from=a to=b"]
        );
    }
}

// Section 10: "No message is part of another, so a test can count by the message
// alone", and none of the lines is written for what somebody asks for.
#[test]
fn what_is_asked_by_hand_is_no_line_of_what_the_coordinator_begins_by_itself() {
    let lines = lines_of(|| {
        let mut world = World::rested(follow(), 4, &["a"]);
        world.put(1, &[(100, 0, 2), (101, 0, 1)]);
        world.quiet(FRESH);
        let changes = world
            .coordinator
            .merge(world.now, region(1), region(2), ASKER)
            .expect("nothing speaks against the merge");
        world.take("merge", changes);
        world.merge_through(2);
        let changes = world
            .coordinator
            .split(world.now, region(1), &[at(101)], ASKER)
            .expect("nothing speaks against the split");
        world.take("split", changes);
        world.split_through(1);
        world.put(4, &[(300, 0, 1)]);
        world.quiet(2 * REST);
    });
    assert_eq!(lines.of(STATE, "a region is to absorb another").len(), 1);
    assert_eq!(lines.of(STATE, "a region is to be split").len(), 1);
    for message in [
        "a merge is begun by the distances",
        "an absorption is begun by itself",
        "a split is begun by itself",
        "a region is moved to even regions out",
    ] {
        assert_eq!(lines.of(STATE, message), [""; 0], "{message}");
    }
}
