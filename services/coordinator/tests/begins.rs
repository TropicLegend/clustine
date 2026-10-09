//! How a coordinator begins without stripes, tested from the record alone:
//! `docs/adr/0017-the-end-of-the-stripes.md`, sections 2.3, 2.4, 5.3 and 6.6 and the
//! scenarios Q1 to Q7, Q9, Q10 and Q12 to Q14 of its section 9.4, with
//! `docs/adr/0016-when-to-merge-and-split.md`, section 5, for what a coordinator that
//! decides by itself begins and when, and `docs/adr/0014-merging-and-splitting.md`,
//! section 5, for the list. Whoever wrote these read the records, the messages and the
//! coordinator's public signatures with their comments, and neither its code nor its
//! own tests, so that a test here says what the record asks for and not what the code
//! happens to do. Q8 is of a later step (the fingerprint goes with the layout) and Q11
//! is the edge's process, in `bin/clustine/tests/edge_start.rs`.
//!
//! The first part drives the state machine, [`Coordinator`], with the time handed in.
//! The second is the service, over TCP and in its own process, with a list that the
//! test hands it. The service reads the real clock, so what it does at its ticks is
//! told by what it reads and by what its clients are sent, and no test waits for time
//! to pass: where a scenario says that something is not done at a tick, the test
//! waits for something that only a tick brings about and looks then.
//!
//! Each test is marked with the scenario it is (`// Q12.3.`), or with the section
//! whose sentence it tests. The words of release in Q12 have the epochs 1040, 1041,
//! 1042 and 1012 where the record has 40, 41, 42 and 12: every epoch a coordinator of
//! these tests issues is above 1000, so that "above 40" would say nothing.
//!
//! A test that is ignored as a finding is one that fails: the coordinator does
//! something else there than the record says, or than one of two places of the record
//! that do not agree. Its comment has the sequence, what was to happen and what does.
//!
//! The world has no boundary. Region 0 is the home region of every list, and a worker
//! named `a` is reached at `a:25600`, so a route says whose a region is.

use std::collections::BTreeMap;
use std::io;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use clustine_coordinator::{
    Asked, Asker, Changes, ClientError, Coordinator, CoordinatorConfig, MoveAnswer, Mover, Order,
    Orders, Policy, Reach, ReleaseOrder, ReshapeOrder, ReshapeRefusal, Reshaped, RoutingWatch,
    Undone, WorkerClient, WorkerEvent, serve, serve_local, serve_with,
};
use clustine_region::{Layout, RegionId, RoutingTable};
use clustine_rpc::{Assignment, PlayersOf, RegionInfo, RegionList, Vouch};
use clustine_world::{ChunkArea, ChunkPos, EntityIds, Vec3};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

/// The lease of the coordinators that are handed their time, and so their grace
/// period and how often one that decides by itself has the list read.
const LEASE: Duration = Duration::from_secs(5);

/// The shortest time that a test tells apart.
const MOMENT: Duration = Duration::from_millis(1);

/// How far apart the heartbeats and ticks are while time passes.
const STEP: Duration = Duration::from_millis(500);

/// How often a coordinator that decides by itself is to be ticked.
const LOOK: Duration = Coordinator::LOOK;

/// How long after a tick the workers' reports are taken: a worker's looks do not fall
/// on the coordinator's.
const LAG: Duration = Duration::from_millis(100);

/// How old a sighting may be, and longer than which a thing has to be wanted.
const FRESH: Duration = Duration::from_secs(1);

/// Every epoch a coordinator of these tests issues is above this.
const FIRST_EPOCH: u64 = 1_000;

/// Who asks for the merges and splits that a test asks for by hand.
const ASKER: Option<u64> = Some(41);

/// Where players enter the world: in the chunk at the origin.
const SPAWN: Vec3 = Vec3::new(0.5, 64.0, 0.5);

/// The epoch of Q12's word of release, the record's 40.
const WORD: u64 = 1_040;

/// The epoch just above it, the record's 41: somebody has run the region since.
const SINCE: u64 = 1_041;

/// The epoch of the second word of Q12.8, the record's 42.
const SECOND: u64 = 1_042;

/// An epoch below the word's, the record's 12.
const BELOW: u64 = 1_012;

fn region(id: u32) -> RegionId {
    RegionId(id)
}

/// A world without a boundary, which is what is left of the layout until it goes.
fn config(lease: Duration, follow: Option<Policy>) -> CoordinatorConfig {
    CoordinatorConfig {
        layout: Layout::single(),
        spawn: SPAWN,
        lease,
        follow,
    }
}

/// The distances 2 and 5, and the rest given.
fn by_itself(rest: Duration) -> Option<Policy> {
    Some(
        Policy {
            merge_distance: 2,
            split_distance: 5,
            rest,
        }
        .checked()
        .expect("the distances fit each other"),
    )
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

/// The world store's list with region 0 as the home region, the `living` regions,
/// each with the highest epoch it was opened with, the pairs of `absorbed`, and `next`
/// as the id of the next region. No region is pinned to an area.
fn list(living: &[(u32, u64)], absorbed: &[(u32, u32)], next: u32) -> RegionList {
    RegionList {
        home: region(0),
        regions: living
            .iter()
            .map(|(id, epoch)| RegionInfo {
                region: region(*id),
                epoch: *epoch,
                bounds: None,
                pinned: Vec::new(),
            })
            .collect(),
        absorbed: absorbed
            .iter()
            .map(|(gone, into)| (region(*gone), region(*into)))
            .collect(),
        next: region(next),
    }
}

/// The list of a world that has just begun: the home region and nothing else.
fn home_alone() -> RegionList {
    list(&[(0, 0)], &[], 1)
}

/// The list of Q12.1: the home region, and region 5 living with `epoch`.
fn with_region_five(epoch: u64) -> RegionList {
    list(&[(0, 0), (5, epoch)], &[], 6)
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

/// How a coordinator of these tests is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Made {
    /// With `Coordinator::new`: it knows no region and has a lease of grace.
    New,
    /// With `Coordinator::alone`: its workers are in its own process.
    Alone,
}

/// A coordinator, the time, the workers that go on saying that they are there, and,
/// for a coordinator that decides by itself, where the players are and what a reading
/// of the list comes to.
#[derive(Debug, Clone)]
struct World {
    coordinator: Coordinator,
    /// When the coordinator was made.
    made: Instant,
    now: Instant,
    fingerprint: u64,
    /// The workers that send heartbeats while time passes, which vouch for all they
    /// were given, in the order they registered.
    heard: Vec<String>,
    /// The workers that say where the players of their regions are at every look.
    reporting: Vec<String>,
    /// Where the players are: of each region the chunks with players in them.
    crowds: BTreeMap<u32, Vec<(ChunkPos, u32)>>,
    /// The tick of the last report, which every report is above.
    reported: u64,
    /// Whether a reading that the coordinator asks for is answered in the call that
    /// asks, as a store in the same process answers.
    answering: bool,
    /// What such a reading comes to: the list, or nothing, which is a store that does
    /// not answer.
    list: Option<RegionList>,
}

impl World {
    /// A coordinator that has just been made, and knows of no worker and of no region.
    fn made(kind: Made, follow: Option<Policy>) -> Self {
        let config = config(LEASE, follow);
        let now = Instant::now();
        let coordinator = match kind {
            Made::New => Coordinator::new(config, now, FIRST_EPOCH),
            Made::Alone => Coordinator::alone(config, now, FIRST_EPOCH),
        };
        Self::around(coordinator, now, follow.is_some())
    }

    /// A coordinator that was made knowing these regions, as the tests of what a
    /// coordinator does with its regions make theirs.
    fn knowing(ids: &[u32]) -> Self {
        let now = Instant::now();
        let regions: Vec<RegionId> = ids.iter().copied().map(region).collect();
        let coordinator = Coordinator::knowing(config(LEASE, None), now, FIRST_EPOCH, &regions);
        Self::around(coordinator, now, false)
    }

    fn around(coordinator: Coordinator, now: Instant, answering: bool) -> Self {
        Self {
            fingerprint: coordinator.config().layout.fingerprint(),
            coordinator,
            made: now,
            now,
            heard: Vec::new(),
            reporting: Vec::new(),
            crowds: BTreeMap::new(),
            reported: 0,
            answering,
            list: None,
        }
    }

    /// The time is `since` after the coordinator was made.
    fn at(&mut self, since: Duration) {
        let when = self.made + since;
        assert!(when >= self.now, "the time of a test does not go back");
        self.now = when;
    }

    /// When the grace period of a coordinator made with `new` ends: a tick then is
    /// outside it.
    fn end_of_grace(&self) -> Instant {
        self.made + LEASE
    }

    /// Answers the reading a call asks for, if the world answers readings. Returns the
    /// changes with those of the reading.
    fn take(&mut self, changes: Changes) -> Changes {
        let mut total = changes;
        if total.read && self.answering {
            let answer = match self.list.clone() {
                Some(list) => self.coordinator.listed(self.now, &list),
                None => self.coordinator.unlisted(self.now),
            };
            // The reading was asked for and has been answered.
            total.read = false;
            let more = self.take(answer);
            add(&mut total, more);
        }
        total
    }

    fn register(&mut self, name: &str, holding: &[Assignment]) -> Changes {
        if !self.heard.iter().any(|heard| heard == name) {
            self.heard.push(name.to_owned());
        }
        let changes = self
            .coordinator
            .register(
                self.now,
                name,
                &address(name),
                holding,
                Some(self.fingerprint),
            )
            .expect("the worker has the coordinator's layout");
        self.take(changes)
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
        self.reporting.retain(|heard| heard != name);
    }

    fn released(&mut self, name: &str, id: u32, epoch: u64) -> Changes {
        let changes = self.coordinator.released(self.now, name, region(id), epoch);
        self.take(changes)
    }

    fn listed(&mut self, list: &RegionList) -> Changes {
        let changes = self.coordinator.listed(self.now, list);
        self.take(changes)
    }

    fn unlisted(&mut self) -> Changes {
        let changes = self.coordinator.unlisted(self.now);
        self.take(changes)
    }

    /// A heartbeat of every worker that is heard, which vouches for all it was given.
    fn beat(&mut self) {
        for name in &self.heard {
            let vouched: Vec<(RegionId, Vouch)> = self
                .coordinator
                .assignments(name)
                .iter()
                .map(|assignment| (assignment.region, Vouch::Committed))
                .collect();
            self.coordinator.heartbeat(self.now, name, &vouched);
        }
    }

    /// Every worker that reports says where the players of each region it runs are,
    /// with a tick above the one it said last.
    fn report(&mut self) {
        for name in self.reporting.clone() {
            let runs = self.coordinator.assignments(&name);
            let mut words = Vec::new();
            for assignment in runs {
                self.reported += 1;
                words.push(PlayersOf {
                    region: assignment.region,
                    epoch: assignment.epoch,
                    tick: self.reported,
                    crowds: self
                        .crowds
                        .get(&assignment.region.0)
                        .cloned()
                        .unwrap_or_default(),
                });
            }
            assert!(
                self.coordinator.players(self.now, &name, &words),
                "{name} is registered"
            );
        }
    }

    fn tick(&mut self) -> Changes {
        let changes = self.coordinator.tick(self.now);
        self.take(changes)
    }

    /// A moment passes, the workers are heard, and the coordinator looks.
    fn step(&mut self, time: Duration) -> Changes {
        self.now += time;
        self.beat();
        self.tick()
    }

    /// A look of a coordinator that decides by itself: [`LAG`] after the tick before,
    /// the workers report; a [`LOOK`] after the tick before, they are heard and the
    /// coordinator looks.
    fn look(&mut self) -> Changes {
        self.now += LAG;
        self.report();
        self.now += LOOK - LAG;
        self.beat();
        self.tick()
    }

    /// What a look whose tick is at `when` comes to, in a world of its own that is as
    /// this one until then: the workers report where they would in a look, and the
    /// coordinator looks at `when`. `when` is no more than a look and a moment away.
    fn look_at(&self, when: Instant) -> (World, Changes) {
        let mut world = self.clone();
        assert!(when > world.now + LAG, "there is the time for a report");
        world.now += LAG;
        world.report();
        world.now = when;
        world.beat();
        let changes = world.tick();
        (world, changes)
    }

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

    fn waiting(&self) -> Vec<u32> {
        self.coordinator
            .waiting()
            .iter()
            .map(|region| region.0)
            .collect()
    }

    /// The regions the coordinator knows: those with a route and those that wait.
    fn known(&self) -> Vec<u32> {
        let table = self.table();
        let mut known: Vec<u32> = table.routes.iter().map(|route| route.region.0).collect();
        known.extend(self.waiting());
        known.sort_unstable();
        assert_eq!(table.waiting as usize, self.waiting().len());
        known
    }

    /// Whether the coordinator knows a worker of this name. Asking is a heartbeat that
    /// vouches for nothing, so a test asks when nothing depends on the worker's being
    /// silent any more.
    fn is_registered(&mut self, name: &str) -> bool {
        self.coordinator.heartbeat(self.now, name, &[])
    }
}

/// Holds that the region, which the coordinator knows without an owner within its
/// grace period, **is given an owner by the first tick at or after the end of the
/// grace period and by nothing before**: not by a reading that succeeds or fails, a
/// worker that registers again or says that the store refused it, nor by any tick
/// before the end. Then a tick just before the end, one at it and one just after it,
/// each in a world of its own. Returns the world after the tick at the end.
///
/// `list` is the list as the coordinator was last handed it, if it was handed one.
fn waits_out_the_grace_period(world: &World, id: u32, list: Option<&RegionList>) -> World {
    let mut world = world.clone();
    let end = world.end_of_grace();
    assert!(world.now < end, "the grace period is not over");
    assert_eq!(world.owner(id), None);
    let names = world.heard.clone();
    if let Some(list) = list {
        world.listed(list);
        assert_eq!(world.owner(id), None, "a reading gave region {id} away");
    }
    world.unlisted();
    assert_eq!(world.owner(id), None, "a failed reading gave it away");
    for name in &names {
        world.register_again(name);
        assert_eq!(world.owner(id), None, "a registration gave it away");
    }
    if let Some(name) = names.first() {
        // The word of a worker that does not own the region, which ends like a tick.
        let changes = world
            .coordinator
            .epoch_refused(world.now, name, region(id), 1);
        world.take(changes);
        assert_eq!(world.owner(id), None, "a refusal by the store gave it away");
    }
    while world.now + STEP < end {
        world.step(STEP);
        assert_eq!(
            world.owner(id),
            None,
            "a tick {:?} before the end gave it away",
            end - world.now
        );
    }
    let mut before = world.clone();
    before.now = end - MOMENT;
    before.beat();
    before.tick();
    assert_eq!(before.owner(id), None, "a tick just before the end");
    let mut after = world.clone();
    after.now = end + MOMENT;
    after.beat();
    after.tick();
    assert!(after.owner(id).is_some(), "a tick just after the end");
    world.now = end;
    world.beat();
    world.tick();
    assert!(world.owner(id).is_some(), "the tick at the end");
    world
}

// ---------------------------------------------------------------------------------
// Q1. A coordinator that knows no region.
// ---------------------------------------------------------------------------------

// Q1.
#[test]
fn a_coordinator_made_anew_knows_no_region() {
    for kind in [Made::New, Made::Alone] {
        let world = World::made(kind, None);
        let table = world.table();
        assert_eq!(table.routes, []);
        assert_eq!(table.waiting, 0);
        assert_eq!(table.home, None);
        assert_eq!(world.coordinator.home(), None);
        assert_eq!(world.known(), [0; 0]);
        assert!(world.coordinator.awaits_the_list());
        assert_eq!(world.coordinator.keeps_its_workers(), kind == Made::Alone);
    }
}

// Q1.
#[test]
fn a_worker_that_registers_holding_nothing_is_assigned_nothing_also_after_the_grace_period() {
    for kind in [Made::New, Made::Alone] {
        let mut world = World::made(kind, None);
        let empty = world.table();
        world.register("a", &[]);
        assert_eq!(world.runs("a"), [0; 0]);
        // Just before the end of the grace period, at it, just after it, and long
        // after: the state machine has nothing to give, and asks for nothing. The
        // service has the list read, not the state machine (section 2.3).
        for since in [LEASE - MOMENT, LEASE, LEASE + MOMENT, 3 * LEASE] {
            world.at(since);
            world.beat();
            assert_eq!(world.tick(), Changes::default(), "{kind:?} at {since:?}");
            assert_eq!(world.runs("a"), [0; 0]);
            assert_eq!(world.table(), empty);
            assert!(world.coordinator.awaits_the_list());
        }
        // A second worker changes nothing of that.
        world.register("b", &[]);
        world.step(STEP);
        assert_eq!((world.runs("a"), world.runs("b")), (vec![], vec![]));
        assert_eq!(world.known(), [0; 0]);
    }
}

// Q1.
#[test]
fn a_merge_and_a_split_are_refused_for_regions_that_no_list_has_named() {
    let chunks = [ChunkPos::new(1, 0)];
    for kind in [Made::New, Made::Alone] {
        let mut world = World::made(kind, None);
        for since in [Duration::ZERO, LEASE + MOMENT] {
            world.at(since);
            world.register("a", &[]);
            let before = world.table();
            assert_eq!(
                world
                    .coordinator
                    .merge(world.now, region(0), region(1), ASKER),
                Err(ReshapeRefusal::NoSuchRegion(region(0)))
            );
            assert_eq!(
                world
                    .coordinator
                    .split(world.now, region(0), &chunks, ASKER),
                Err(ReshapeRefusal::NoSuchRegion(region(0)))
            );
            assert_eq!(world.table(), before);
            assert_eq!(world.coordinator.under_way(), []);
        }
    }
    // A region that a worker reports is known, and the other region of a merge with
    // it is not: the survivor is looked at first.
    let mut world = World::made(Made::New, None);
    world.register("a", &[held(3, 13)]);
    assert_eq!(
        world
            .coordinator
            .merge(world.now, region(3), region(1), ASKER),
        Err(ReshapeRefusal::NoSuchRegion(region(1)))
    );
    assert_eq!(
        world
            .coordinator
            .merge(world.now, region(1), region(3), ASKER),
        Err(ReshapeRefusal::NoSuchRegion(region(1)))
    );
}

// ---------------------------------------------------------------------------------
// Q2. The first list, and the grace period.
// ---------------------------------------------------------------------------------

// Q2.
#[test]
fn the_first_list_makes_the_home_region_known_without_an_owner() {
    let mut world = World::made(Made::New, None);
    world.register("a", &[]);
    let before = world.table();
    world.at(MOMENT);
    let changes = world.listed(&home_alone());
    assert!(
        changes.routing,
        "the table has a home region and one that waits"
    );
    assert_eq!(changes.workers, [""; 0]);
    assert_eq!(world.known(), [0]);
    assert_eq!(world.waiting(), [0]);
    assert_eq!(world.owner(0), None);
    assert_eq!(world.coordinator.home(), Some(region(0)));
    assert!(!world.coordinator.awaits_the_list());
    let table = world.table();
    assert_eq!(table.home, Some(region(0)));
    assert_eq!(table.waiting, 1);
    assert_eq!(table.routes, []);
    assert!(table.version > before.version);
    assert_eq!(world.runs("a"), [0; 0]);
}

// Q2.
#[test]
fn the_home_region_is_assigned_by_the_first_tick_at_or_after_the_end_of_the_grace_period_and_by_nothing_before()
 {
    // The worker is there before the list is, or comes after it.
    for registers_first in [true, false] {
        let mut world = World::made(Made::New, None);
        if registers_first {
            world.register("a", &[]);
        }
        world.at(STEP);
        world.listed(&home_alone());
        if !registers_first {
            world.register("a", &[]);
        }
        assert_eq!(world.owner(0), None);
        let world = waits_out_the_grace_period(&world, 0, Some(&home_alone()));
        assert_eq!(world.owner(0).as_deref(), Some("a"));
        assert!(world.epoch(0) > FIRST_EPOCH);
        assert_eq!(world.known(), [0]);
        let table = world.table();
        assert_eq!((table.waiting, table.home), (0, Some(region(0))));
    }
}

// Q2: what the tick that ends the wait says it changed.
#[test]
fn the_tick_at_the_end_of_the_grace_period_says_whom_it_gave_the_home_region() {
    let mut world = World::made(Made::New, None);
    world.register("a", &[]);
    world.listed(&home_alone());
    world.at(LEASE - MOMENT);
    world.beat();
    assert_eq!(world.tick(), Changes::default());
    world.at(LEASE);
    world.beat();
    let changes = world.tick();
    assert_eq!(changes.workers, ["a"]);
    assert!(changes.routing);
    assert_eq!(world.runs("a"), [0]);
}

// ---------------------------------------------------------------------------------
// Q3. A coordinator that is alone has no grace period.
// ---------------------------------------------------------------------------------

// Q3.
#[test]
fn a_coordinator_that_is_alone_assigns_by_the_call_that_hands_in_the_list() {
    // At the instant it was made, a moment later and just before a lease is over.
    for since in [Duration::ZERO, MOMENT, LEASE - MOMENT] {
        let mut world = World::made(Made::Alone, None);
        world.register("a", &[]);
        assert_eq!(world.runs("a"), [0; 0]);
        world.at(since);
        let changes = world.listed(&home_alone());
        assert_eq!(world.owner(0).as_deref(), Some("a"), "{since:?}");
        assert!(world.epoch(0) > FIRST_EPOCH);
        assert_eq!(changes.workers, ["a"]);
        assert!(changes.routing);
        let table = world.table();
        assert_eq!((table.waiting, table.home), (0, Some(region(0))));
        assert!(!world.coordinator.awaits_the_list());
    }
}

// Q3.
#[test]
fn a_coordinator_that_is_alone_assigns_by_the_first_tick_after_a_worker_has_registered() {
    let mut world = World::made(Made::Alone, None);
    world.listed(&home_alone());
    assert_eq!((world.known(), world.waiting()), (vec![0], vec![0]));
    // Nobody is there to be given it.
    world.at(MOMENT);
    world.tick();
    assert_eq!(world.owner(0), None);
    // "By the first tick or list after one has registered": the registration says
    // what the worker runs, which is nothing yet.
    world.register("a", &[]);
    assert_eq!(world.runs("a"), [0; 0]);
    let changes = world.tick();
    assert_eq!(world.owner(0).as_deref(), Some("a"));
    assert_eq!(changes.workers, ["a"]);
}

// Q3.
#[test]
fn a_coordinator_that_is_alone_assigns_by_the_first_list_after_a_worker_has_registered() {
    let mut world = World::made(Made::Alone, None);
    world.listed(&home_alone());
    world.at(MOMENT);
    world.register("a", &[]);
    assert_eq!(world.runs("a"), [0; 0]);
    let changes = world.listed(&home_alone());
    assert_eq!(world.owner(0).as_deref(), Some("a"));
    assert_eq!(changes.workers, ["a"]);
}

// Q3: the other half. With `new` the same calls give nothing away.
#[test]
fn a_coordinator_made_with_new_gives_nothing_away_by_the_same_calls() {
    let mut world = World::made(Made::New, None);
    world.register("a", &[]);
    world.at(MOMENT);
    world.listed(&home_alone());
    world.tick();
    world.listed(&home_alone());
    assert_eq!(world.owner(0), None);
}

// Section 2.3: "no grace period: ... it assigns at once, evens out at once".
#[test]
fn a_coordinator_that_is_alone_evens_out_at_once() {
    let mut world = World::made(Made::Alone, None);
    world.register("a", &[]);
    world.listed(&list(&[(0, 0), (1, 0), (2, 0)], &[], 3));
    assert_eq!(world.runs("a"), [0, 1, 2]);
    world.register("b", &[]);
    let epoch = world.epoch(2);
    // At the instant it was made: the highest region of the worker that has two more
    // than the other is to be released.
    let changes = world.tick();
    assert_eq!(
        changes.releases,
        [ReleaseOrder {
            worker: "a".to_owned(),
            region: region(2),
            epoch,
        }]
    );
}

// Section 2.3: the other half. With `new` nothing is evened out before the end of the
// grace period, and the first tick at or after it begins to.
#[test]
fn a_coordinator_made_with_new_evens_nothing_out_within_its_grace_period() {
    let mut world = World::made(Made::New, None);
    world.register("a", &[held(0, 10), held(1, 11), held(2, 12)]);
    world.register("b", &[]);
    let release = [ReleaseOrder {
        worker: "a".to_owned(),
        region: region(2),
        epoch: 12,
    }];
    while world.now + STEP < world.end_of_grace() {
        assert_eq!(world.step(STEP).releases, []);
    }
    for (since, evens_out) in [
        (LEASE - MOMENT, false),
        (LEASE, true),
        (LEASE + MOMENT, true),
    ] {
        let mut world = world.clone();
        world.at(since);
        world.beat();
        let changes = world.tick();
        if evens_out {
            assert_eq!(changes.releases, release, "{since:?} after it was made");
        } else {
            assert_eq!(changes.releases, [], "{since:?} after it was made");
        }
    }
}

// ---------------------------------------------------------------------------------
// Q4. A region on a worker's word, and what the list says of it.
// ---------------------------------------------------------------------------------

/// A coordinator made anew to which `a` has reported region 3 before any list.
fn region_three_on_a_workers_word() -> World {
    let mut world = World::made(Made::New, None);
    world.at(MOMENT);
    world.register("a", &[held(3, 13)]);
    assert_eq!(world.coordinator.assignments("a"), [held(3, 13)]);
    assert_eq!(world.owner(3).as_deref(), Some("a"));
    assert_eq!(world.epoch(3), 13);
    assert_eq!(world.known(), [3]);
    let table = world.table();
    assert_eq!((table.waiting, table.home), (0, None));
    // To be told of a region is not to be handed a list.
    assert!(world.coordinator.awaits_the_list());
    world
}

// Q4.
#[test]
fn a_worker_that_reports_a_region_before_any_list_owns_it_on_its_word() {
    let mut world = region_three_on_a_workers_word();
    // And keeps it through the grace period and after it, for as long as it vouches.
    for since in [LEASE - MOMENT, LEASE, LEASE + MOMENT, 2 * LEASE] {
        world.at(since);
        world.beat();
        world.tick();
        assert_eq!(world.coordinator.assignments("a"), [held(3, 13)]);
    }
}

// Q4: "one that reports what it runs is believed, as a part's worker is today", and
// so whoever reports a region first keeps it, whatever the epochs.
#[test]
fn a_second_worker_that_reports_the_same_region_before_any_list_is_not_given_it() {
    let mut world = region_three_on_a_workers_word();
    world.register("b", &[held(3, 99)]);
    assert_eq!(world.coordinator.assignments("a"), [held(3, 13)]);
    assert_eq!(world.runs("b"), [0; 0]);
    assert_eq!(world.epoch(3), 13);
}

// Q4.
#[test]
fn a_list_that_shows_the_reported_region_living_leaves_it_with_its_worker() {
    // The store has it with the epoch the worker runs it with, or never opened: the
    // worker's epoch is the higher either way.
    for listed in [13, 0] {
        let mut world = region_three_on_a_workers_word();
        world.at(2 * MOMENT);
        let changes = world.listed(&list(&[(0, 0), (3, listed)], &[], 4));
        assert_eq!(world.coordinator.assignments("a"), [held(3, 13)]);
        assert_eq!(world.owner(3).as_deref(), Some("a"));
        assert_eq!(changes.workers, [""; 0]);
        assert_eq!((world.known(), world.waiting()), (vec![0, 3], vec![0]));
        assert_eq!(world.coordinator.home(), Some(region(0)));
        let world = waits_out_the_grace_period(&world, 0, None);
        assert_eq!(world.coordinator.assignments("a")[1], held(3, 13));
    }
}

// Q4.
#[test]
fn a_list_that_shows_the_reported_region_absorbed_takes_it_away() {
    let mut world = region_three_on_a_workers_word();
    world.at(2 * MOMENT);
    let changes = world.listed(&list(&[(0, 0)], &[(3, 0)], 4));
    assert_eq!(world.runs("a"), [0; 0]);
    assert_eq!(world.owner(3), None);
    assert_eq!(world.known(), [0]);
    assert_eq!(changes.workers, ["a"]);
    assert!(changes.routing);
    assert_eq!(world.table().absorbed, [(region(3), region(0))]);
}

// Q4.
#[test]
fn a_list_that_has_its_next_id_above_the_reported_region_and_no_such_region_takes_it_away() {
    for next in [4, 9] {
        let mut world = region_three_on_a_workers_word();
        world.at(2 * MOMENT);
        let changes = world.listed(&list(&[(0, 0)], &[], next));
        assert_eq!(world.runs("a"), [0; 0], "next: {next}");
        assert_eq!(world.known(), [0]);
        assert_eq!(changes.workers, ["a"]);
    }
}

// Q4: the other half, of ADR-0014, section 5.2. A list whose next id is not above
// the region is older than the region, and says nothing of it.
#[test]
fn a_list_whose_next_id_is_not_above_the_reported_region_leaves_it_alone() {
    for next in [3, 1] {
        let mut world = region_three_on_a_workers_word();
        world.at(2 * MOMENT);
        world.listed(&list(&[(0, 0)], &[], next));
        assert_eq!(
            world.coordinator.assignments("a"),
            [held(3, 13)],
            "next: {next}"
        );
        assert_eq!(world.known(), [0, 3]);
    }
}

// ---------------------------------------------------------------------------------
// Q6. N13: a coordinator that starts anew while the store is away.
// ---------------------------------------------------------------------------------

/// A coordinator made anew to which `a` reports region 0 and whose every reading
/// fails, until `since` after it was made. Region 1, whose worker died, is reported
/// by nobody.
fn the_store_is_away_until(since: Duration) -> World {
    let mut world = World::made(Made::New, None);
    world.at(MOMENT);
    world.register("a", &[held(0, 10)]);
    world.unlisted();
    while world.now + STEP < world.made + since {
        world.step(STEP);
        world.unlisted();
        assert_eq!(world.known(), [0], "nobody has named region 1");
        assert_eq!(world.coordinator.assignments("a"), [held(0, 10)]);
        assert!(world.coordinator.awaits_the_list());
        assert_eq!(world.coordinator.home(), None);
    }
    world.at(since);
    world.beat();
    world
}

// Q6.
#[test]
fn a_region_that_nobody_reports_is_not_known_while_no_list_can_be_read_and_is_assigned_by_the_list_once_the_grace_period_is_over()
 {
    // At the end of the grace period, just after it, and long after it.
    for since in [LEASE, LEASE + MOMENT, 3 * LEASE] {
        let mut world = the_store_is_away_until(since);
        let changes = world.listed(&list(&[(0, 10), (1, 11)], &[], 2));
        assert_eq!(world.known(), [0, 1]);
        assert_eq!(world.owner(1).as_deref(), Some("a"), "{since:?}");
        assert!(world.epoch(1) > 11);
        assert_eq!(world.coordinator.assignments("a")[0], held(0, 10));
        assert_eq!(changes.workers, ["a"]);
        assert!(changes.routing);
        assert_eq!(world.table().waiting, 0);
    }
}

// Q6: just before the end of the grace period the list names the region and does not
// give it away.
#[test]
fn a_list_that_is_read_just_before_the_end_of_the_grace_period_leaves_the_region_to_the_tick_at_its_end()
 {
    let mut world = the_store_is_away_until(LEASE - MOMENT);
    let listed = list(&[(0, 10), (1, 11)], &[], 2);
    world.listed(&listed);
    assert_eq!((world.known(), world.waiting()), (vec![0, 1], vec![1]));
    world.at(LEASE);
    world.beat();
    world.tick();
    assert_eq!(world.owner(1).as_deref(), Some("a"));
    assert!(world.epoch(1) > 11);
}

// Q6: with a second worker that runs nothing, the region goes to that one.
#[test]
fn the_region_that_the_list_names_goes_to_the_worker_with_the_fewest() {
    let mut world = the_store_is_away_until(LEASE + MOMENT);
    world.register("b", &[]);
    world.listed(&list(&[(0, 10), (1, 11)], &[], 2));
    assert_eq!(world.owner(1).as_deref(), Some("b"));
    assert_eq!(world.coordinator.assignments("a"), [held(0, 10)]);
}

// ---------------------------------------------------------------------------------
// Q7. Deciding by itself: nothing before the first list, and N14's line.
// ---------------------------------------------------------------------------------

/// A coordinator that decides by itself, with a rest of ten seconds, to which `a` has
/// reported regions 1 and 2, whose players stand within the merge distance of each
/// other and far from where players enter. It has been handed no list.
fn two_reported_regions_that_want_to_merge() -> World {
    let mut world = World::made(Made::New, by_itself(Duration::from_secs(10)));
    world.register("a", &[held(1, 11), held(2, 12)]);
    world.reporting.push("a".to_owned());
    world.crowds.insert(1, vec![(ChunkPos::new(100, 0), 1)]);
    world.crowds.insert(2, vec![(ChunkPos::new(102, 0), 1)]);
    world
}

// Q7.
#[test]
fn nothing_is_begun_by_itself_before_the_first_list_whatever_workers_report() {
    let mut world = two_reported_regions_that_want_to_merge();
    // Every reading fails. A minute: the grace period, the rest of both regions and
    // the time for which a thing has to be wanted are long over.
    while world.now < world.made + Duration::from_secs(60) {
        let changes = world.look();
        let since = world.now - world.made;
        assert_eq!(world.coordinator.under_way(), [], "{since:?}");
        assert_eq!(changes.releases, [], "{since:?}");
        assert_eq!(changes.orders, [], "{since:?}");
        assert_eq!(changes.reshaped, [], "{since:?}");
        assert_eq!(world.runs("a"), [1, 2]);
        assert!(world.coordinator.awaits_the_list());
    }
}

// Q7: the other half. With the list, the same reports have the two merged.
#[test]
fn the_same_reports_have_the_regions_merged_once_the_list_has_been_read() {
    let mut world = two_reported_regions_that_want_to_merge();
    while world.now < world.made + Duration::from_secs(20) {
        world.look();
    }
    assert_eq!(world.coordinator.under_way(), []);
    // The store answers from now on. The home region is given to `a`, which reports
    // it from the next look on.
    world.list = Some(list(&[(0, 0), (1, 11), (2, 12)], &[], 3));
    // A lease until the timer has the list read, and time to spare.
    let limit = world.now + 3 * LEASE;
    while world.coordinator.under_way().is_empty() {
        world.look();
        assert!(world.now < limit, "nothing was begun with the list read");
    }
    assert_eq!(
        world.coordinator.under_way(),
        [Asked::Merge {
            survivor: region(1),
            absorbed: region(2),
        }]
    );
}

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
        /// The lines that the coordinator's crate wrote with this message: each as
        /// the message and then its fields in the order they were given,
        /// `name=value` with a space between.
        pub fn with(&self, message: &str) -> Vec<String> {
            let lines = self
                .0
                .lock()
                .expect("no test panics while it holds the lines");
            lines
                .iter()
                .filter(|(target, _, line)| {
                    target.starts_with("clustine_coordinator")
                        && (line == message || line.starts_with(&format!("{message} ")))
                })
                .map(|(_, _, line)| line.clone())
                .collect()
        }

        /// The levels of the lines that the coordinator's crate wrote with this
        /// message, whatever fields follow it.
        pub fn of(&self, message: &str) -> Vec<Level> {
            let lines = self
                .0
                .lock()
                .expect("no test panics while it holds the lines");
            lines
                .iter()
                .filter(|(target, _, line)| {
                    target.starts_with("clustine_coordinator")
                        && (line == message || line.starts_with(&format!("{message} ")))
                })
                .map(|(_, level, _)| *level)
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

/// The lines that are written while `what` runs.
///
/// The tests of this file run side by side, and all but a few without anybody who
/// listens. Whether a line has a listener is remembered where it is written, from the
/// first time it is written by any thread, so `what` is run once for nothing: after
/// that every line of it is known, and what is remembered of each is worked out anew
/// with this test listening.
fn lines_of(what: impl Fn()) -> log::Lines {
    tracing::subscriber::with_default(log::Lines::default(), &what);
    let lines = log::Lines::default();
    tracing::subscriber::with_default(lines.clone(), || {
        tracing::callsite::rebuild_interest_cache();
        what();
    });
    lines
}

/// N14's line, which a coordinator that decides by itself writes for a world of
/// pinned regions.
const PINNED: &str = "the world has regions that are pinned to an area: a region that is split \
                      off here cannot grow. Start the coordinator with --reshape by-hand to keep \
                      pinned regions as they are";

/// The list of a world of two regions side by side, each pinned to its area, as
/// `clustine worldstore --pin 4` makes it.
fn two_pinned() -> RegionList {
    let mut listed = list(&[(0, 0), (1, 0)], &[], 2);
    listed.regions[0].pinned = vec![ChunkArea {
        min_x: None,
        max_x: Some(4),
    }];
    listed.regions[1].pinned = vec![ChunkArea {
        min_x: Some(4),
        max_x: None,
    }];
    listed
}

// Q7, N14.
#[test]
fn a_list_that_shows_a_pinned_region_is_a_warning_in_the_log_once() {
    let lines = lines_of(|| {
        let mut world = World::made(Made::New, by_itself(LEASE));
        world.register("a", &[]);
        world.listed(&two_pinned());
        // A later list that shows the same regions pinned, within the grace period
        // and after it, and the ticks between them.
        world.at(STEP);
        world.listed(&two_pinned());
        world.at(2 * LEASE);
        world.beat();
        world.tick();
        world.listed(&two_pinned());
    });
    assert_eq!(lines.of(PINNED), [tracing::Level::WARN]);
}

// Q7, N14: "at the first reading that shows a pinned region", which need not be the
// first reading.
#[test]
fn the_warning_is_written_at_the_first_list_that_shows_a_pinned_region() {
    let lines = lines_of(|| {
        let mut world = World::made(Made::New, by_itself(LEASE));
        world.listed(&list(&[(0, 0), (1, 0)], &[], 2));
    });
    assert_eq!(lines.of(PINNED).len(), 0);
    let lines = lines_of(|| {
        let mut world = World::made(Made::New, by_itself(LEASE));
        world.listed(&list(&[(0, 0), (1, 0)], &[], 2));
        world.at(STEP);
        world.listed(&two_pinned());
        world.at(2 * STEP);
        world.listed(&two_pinned());
    });
    assert_eq!(lines.of(PINNED), [tracing::Level::WARN]);
}

// Section 1, point 4, and N14: the line is of a world that "is reshaped by itself",
// and tells whoever reads it to start the coordinator `by-hand`. One that is started
// so has nothing to say.
#[test]
fn a_coordinator_that_decides_nothing_by_itself_says_nothing_of_pinned_regions() {
    let lines = lines_of(|| {
        let mut world = World::made(Made::New, None);
        world.listed(&two_pinned());
        world.at(STEP);
        world.listed(&two_pinned());
    });
    assert_eq!(lines.of(PINNED).len(), 0);
}

// ---------------------------------------------------------------------------------
// Q12. A release before the first list (section 2.3, N18).
// ---------------------------------------------------------------------------------

/// Q12's beginning: a coordinator made with `new`, within its grace period, handed no
/// list; the workers register in the order given, holding nothing; `a` says it
/// released region 5 with [`WORD`]. That call changes nothing.
fn a_word_before_the_first_list(workers: &[&str]) -> World {
    let mut world = World::made(Made::New, None);
    world.at(STEP);
    for name in workers {
        world.register(name, &[]);
    }
    world.at(2 * STEP);
    let before = world.table();
    let changes = world.released("a", 5, WORD);
    assert_eq!(changes, Changes::default());
    assert_eq!(world.known(), [0; 0], "region 5 is not known");
    assert_eq!(world.table(), before);
    assert_eq!((before.routes.len(), before.waiting), (0, 0));
    for name in workers {
        assert_eq!(world.runs(name), [0; 0]);
    }
    assert!(world.coordinator.awaits_the_list());
    world
}

// Q12.
#[test]
fn a_word_of_release_before_the_first_list_changes_nothing() {
    let mut world = a_word_before_the_first_list(&["a", "b"]);
    // Nor do the ticks that follow it, within the grace period and after it.
    for since in [LEASE - MOMENT, LEASE, LEASE + MOMENT] {
        world.at(since);
        world.beat();
        assert_eq!(world.tick(), Changes::default());
        assert_eq!(world.known(), [0; 0]);
    }
}

// Q12.1.
#[test]
fn a_region_the_first_list_has_with_the_words_epoch_or_a_lower_one_is_given_away_by_that_call_within_the_grace_period()
 {
    for listed in [WORD, BELOW] {
        let mut world = a_word_before_the_first_list(&["a", "b"]);
        world.at(3 * STEP);
        let changes = world.listed(&with_region_five(listed));
        // `a`, which registered first, is behind `b` for having let go of it.
        assert_eq!(world.owner(5).as_deref(), Some("b"), "listed with {listed}");
        assert!(
            world.epoch(5) > WORD,
            "{} is not above the word's",
            world.epoch(5)
        );
        assert_eq!(changes.workers, ["b"]);
        assert!(changes.routing);
        assert!(!world.coordinator.awaits_the_list());
        // Region 0 has no owner, and waits for the end of the grace period.
        assert_eq!((world.known(), world.waiting()), (vec![0, 5], vec![0]));
        assert_eq!(world.table().waiting, 1);
        let world = waits_out_the_grace_period(&world, 0, Some(&with_region_five(listed)));
        assert_eq!(world.owner(0).as_deref(), Some("a"));
        assert_eq!(world.owner(5).as_deref(), Some("b"));
    }
}

// Q12.2.
#[test]
fn the_worker_that_released_the_region_is_given_it_if_it_is_alone() {
    let mut world = a_word_before_the_first_list(&["a"]);
    world.at(3 * STEP);
    let changes = world.listed(&with_region_five(WORD));
    assert_eq!(world.owner(5).as_deref(), Some("a"));
    assert!(world.epoch(5) > WORD);
    assert_eq!(changes.workers, ["a"]);
    assert_eq!(world.waiting(), [0]);
}

// Q12.3.
#[test]
fn a_word_is_dropped_if_the_list_has_the_region_with_a_higher_epoch() {
    let mut world = a_word_before_the_first_list(&["a", "b"]);
    world.at(3 * STEP);
    let listed = with_region_five(SINCE);
    let changes = world.listed(&listed);
    assert_eq!(world.owner(5), None);
    assert_eq!((world.known(), world.waiting()), (vec![0, 5], vec![0, 5]));
    assert_eq!(changes.workers, [""; 0]);
    let world = waits_out_the_grace_period(&world, 5, Some(&listed));
    assert!(
        world.epoch(5) > SINCE,
        "{} is not above the list's",
        world.epoch(5)
    );
    // A word that is dropped has put nobody behind anybody: the lower region goes to
    // the worker that registered first.
    assert_eq!(world.owner(0).as_deref(), Some("a"));
    assert_eq!(world.owner(5).as_deref(), Some("b"));
}

// Q12.4.
#[test]
fn a_word_is_dropped_and_the_region_stays_unknown_if_the_list_does_not_have_it_living() {
    let lists = [
        // Absorbed by region 2.
        list(&[(0, 0), (2, 0)], &[(5, 2)], 6),
        // Below the next id, and neither living nor absorbed.
        list(&[(0, 0)], &[], 9),
        // Of no table the store has: the next id is not above it.
        list(&[(0, 0)], &[], 4),
    ];
    for listed in lists {
        let mut world = a_word_before_the_first_list(&["a", "b"]);
        world.at(3 * STEP);
        world.listed(&listed);
        let living: Vec<u32> = listed.regions.iter().map(|info| info.region.0).collect();
        assert_eq!(world.known(), living, "{listed:?}");
        assert_eq!(world.runs("a"), [0; 0]);
        assert_eq!(world.runs("b"), [0; 0]);
        // Then or after any later call: readings, registrations and ticks through the
        // grace period,
        let mut world = waits_out_the_grace_period(&world, 0, Some(&listed));
        assert_eq!(world.known(), living);
        // in which nobody was put behind anybody,
        assert_eq!(world.owner(0).as_deref(), Some("a"));
        // the word said once more, which nothing keeps any longer,
        let before = world.table();
        assert_eq!(world.released("a", 5, WORD), Changes::default());
        assert_eq!(world.table(), before);
        // and the ticks and the readings of two leases more.
        while world.now < world.made + 3 * LEASE {
            world.step(STEP);
            world.listed(&listed);
            assert_eq!(world.known(), living);
        }
    }
}

// Q12.5.
#[test]
fn a_word_is_dropped_if_a_worker_registered_holding_the_region_before_the_list() {
    let mut world = a_word_before_the_first_list(&["a", "b"]);
    world.at(3 * STEP);
    world.register("b", &[held(5, SINCE)]);
    assert_eq!(world.coordinator.assignments("b"), [held(5, SINCE)]);
    world.at(4 * STEP);
    let changes = world.listed(&with_region_five(SINCE));
    assert_eq!(world.coordinator.assignments("b"), [held(5, SINCE)]);
    assert_eq!(world.owner(5).as_deref(), Some("b"));
    assert_eq!(world.epoch(5), SINCE);
    assert_eq!(changes.workers, [""; 0]);
    assert_eq!(world.waiting(), [0]);
    // And it stays so: the word waits for no later list either.
    let world = waits_out_the_grace_period(&world, 0, Some(&with_region_five(SINCE)));
    assert_eq!(world.coordinator.assignments("b"), [held(5, SINCE)]);
    assert_eq!(world.owner(0).as_deref(), Some("a"));
}

// Q12.5, the other way round: the region is known and owned when the word is said,
// so there is no word to keep.
#[test]
fn a_word_about_a_region_that_another_worker_has_reported_is_not_kept() {
    let mut world = World::made(Made::New, None);
    world.at(STEP);
    world.register("a", &[]);
    world.register("b", &[held(5, SINCE)]);
    world.at(2 * STEP);
    let before = world.table();
    assert_eq!(world.released("a", 5, WORD), Changes::default());
    assert_eq!(world.table(), before);
    world.at(3 * STEP);
    let changes = world.listed(&with_region_five(SINCE));
    assert_eq!(world.coordinator.assignments("b"), [held(5, SINCE)]);
    assert_eq!(changes.workers, [""; 0]);
    let world = waits_out_the_grace_period(&world, 0, Some(&with_region_five(SINCE)));
    assert_eq!(world.owner(0).as_deref(), Some("a"));
    assert_eq!(world.coordinator.assignments("b"), [held(5, SINCE)]);
}

// Q12.5, with the worker that said the word as the one that registers holding the
// region: it owns it by its report, and its word of before is dropped.
#[test]
fn a_word_is_dropped_if_its_own_worker_registered_holding_the_region_before_the_list() {
    let mut world = a_word_before_the_first_list(&["a", "b"]);
    world.at(3 * STEP);
    world.register("a", &[held(5, WORD)]);
    assert_eq!(world.coordinator.assignments("a"), [held(5, WORD)]);
    world.at(4 * STEP);
    let changes = world.listed(&with_region_five(WORD));
    assert_eq!(world.coordinator.assignments("a"), [held(5, WORD)]);
    assert_eq!(world.epoch(5), WORD);
    assert_eq!(changes.workers, [""; 0]);
    assert_eq!(world.waiting(), [0]);
}

// Q12.6.
#[test]
fn a_word_is_dropped_if_its_worker_was_forgotten_before_the_list() {
    let mut world = a_word_before_the_first_list(&["a", "b"]);
    // Its lease runs out, which is after the end of the grace period.
    world.silence("a");
    while world.now < world.made + LEASE + 3 * STEP {
        world.step(STEP);
    }
    assert_eq!(world.known(), [0; 0]);
    let changes = world.listed(&with_region_five(WORD));
    // Nothing fails. Region 5 is given out like any region of a list, which after the
    // grace period is by that call, as region 0 is.
    assert_eq!(world.known(), [0, 5]);
    assert_eq!(world.owner(0).as_deref(), Some("b"));
    assert_eq!(world.owner(5).as_deref(), Some("b"));
    assert_eq!(changes.workers, ["b"]);
    assert_eq!(world.runs("a"), [0; 0]);
    assert!(!world.is_registered("a"));
}

// Q12.6, by the way Q12.7 names: a worker that says it leaves while it owns nothing
// is forgotten by that call. Within the grace period this shows that the region is
// not let go: it waits like any region of a list.
#[test]
fn the_word_of_a_worker_that_has_left_is_dropped_and_the_region_waits_out_the_grace_period() {
    let mut world = a_word_before_the_first_list(&["a", "b"]);
    world.at(3 * STEP);
    let changes = world.coordinator.leaving(world.now, "a");
    assert_eq!(changes.gone, ["a"]);
    world.silence("a");
    world.at(4 * STEP);
    let listed = with_region_five(WORD);
    world.listed(&listed);
    assert_eq!((world.known(), world.waiting()), (vec![0, 5], vec![0, 5]));
    let world = waits_out_the_grace_period(&world, 5, Some(&listed));
    assert_eq!(world.owner(5).as_deref(), Some("b"));
    assert_eq!(world.owner(0).as_deref(), Some("b"));
}

// Q12.7.
#[test]
fn a_region_that_was_let_go_and_that_nobody_can_be_given_goes_to_the_next_worker_within_the_grace_period()
 {
    let mut world = a_word_before_the_first_list(&["a"]);
    // `a` has lost its connection, and is registered still.
    world.at(3 * STEP);
    world.coordinator.disconnected(world.now, "a");
    world.at(4 * STEP);
    world.listed(&with_region_five(WORD));
    assert_eq!((world.known(), world.waiting()), (vec![0, 5], vec![0, 5]));
    assert_eq!(world.table().waiting, 2);
    assert_eq!(world.table().routes, []);
    // No tick finds anybody for it.
    world.at(5 * STEP);
    world.tick();
    assert_eq!(world.waiting(), [0, 5]);
    // `b` registers, and is given region 5 by that call or the next tick, well within
    // the grace period. Region 0 was not let go, and waits.
    world.at(6 * STEP);
    world.register("b", &[]);
    world.tick();
    assert!(world.now < world.end_of_grace());
    assert_eq!(world.owner(5).as_deref(), Some("b"));
    assert!(world.epoch(5) > WORD);
    assert_eq!(world.owner(0), None);
    assert_eq!(world.waiting(), [0]);
}

// Q12.8.
#[test]
fn two_words_are_judged_in_the_order_they_were_said() {
    // `b` registers before `c` or after it: either way both `a` and `c` are behind it.
    for workers in [["a", "b", "c"], ["a", "c", "b"]] {
        let mut world = a_word_before_the_first_list(&workers);
        assert_eq!(world.released("c", 5, SECOND), Changes::default());
        assert_eq!(world.known(), [0; 0]);
        world.at(3 * STEP);
        let listed = with_region_five(WORD);
        let changes = world.listed(&listed);
        assert_eq!(world.owner(5).as_deref(), Some("b"), "{workers:?}");
        assert!(
            world.epoch(5) > SECOND,
            "{} is not above the second word's",
            world.epoch(5)
        );
        assert_eq!(changes.workers, ["b"]);
        // `a` went behind the others first, and `c` then behind `a`: of the two, which
        // run nothing, `a` is given the next region.
        let world = waits_out_the_grace_period(&world, 0, Some(&listed));
        assert_eq!(world.owner(0).as_deref(), Some("a"), "{workers:?}");
    }
}

// Q12.8.
#[test]
fn the_same_word_said_twice_is_one_word() {
    let mut world = a_word_before_the_first_list(&["a", "b"]);
    assert_eq!(world.released("a", 5, WORD), Changes::default());
    world.at(3 * STEP);
    world.listed(&with_region_five(WORD));
    assert_eq!(world.owner(5).as_deref(), Some("b"));
    assert!(world.epoch(5) > WORD);
    assert_eq!(world.waiting(), [0]);

    // Said again behind another worker's word, it is still where it was said first.
    // Kept a second time, it would be judged behind `c`'s and put `a` behind `c`.
    let mut world = a_word_before_the_first_list(&["a", "c", "b"]);
    assert_eq!(world.released("c", 5, SECOND), Changes::default());
    assert_eq!(world.released("a", 5, WORD), Changes::default());
    world.at(3 * STEP);
    let listed = with_region_five(WORD);
    world.listed(&listed);
    assert_eq!(world.owner(5).as_deref(), Some("b"));
    assert!(world.epoch(5) > SECOND);
    let world = waits_out_the_grace_period(&world, 0, Some(&listed));
    assert_eq!(world.owner(0).as_deref(), Some("a"));
}

// Q12.9.
#[test]
fn a_reading_that_fails_keeps_the_word() {
    let mut world = a_word_before_the_first_list(&["a", "b"]);
    world.at(3 * STEP);
    world.unlisted();
    assert!(world.coordinator.awaits_the_list());
    assert_eq!(world.known(), [0; 0]);
    world.at(4 * STEP);
    world.beat();
    world.tick();
    world.unlisted();
    world.at(5 * STEP);
    let changes = world.listed(&with_region_five(WORD));
    assert_eq!(world.owner(5).as_deref(), Some("b"));
    assert!(world.epoch(5) > WORD);
    assert_eq!(changes.workers, ["b"]);
    assert_eq!(world.waiting(), [0]);
}

// Section 2.3: "The words that were kept are gone with the first list, whatever
// became of each." A word that the first list dropped is not judged by the second.
#[test]
fn a_word_that_the_first_list_dropped_is_not_judged_by_a_later_list() {
    let mut world = a_word_before_the_first_list(&["a", "b"]);
    world.at(3 * STEP);
    // Of no table the store has yet: the next id is not above region 5.
    world.listed(&list(&[(0, 0)], &[], 4));
    assert_eq!(world.known(), [0]);
    world.at(4 * STEP);
    let listed = with_region_five(WORD);
    world.listed(&listed);
    assert_eq!((world.known(), world.waiting()), (vec![0, 5], vec![0, 5]));
    let world = waits_out_the_grace_period(&world, 5, Some(&listed));
    assert_eq!(world.owner(0).as_deref(), Some("a"));
    assert_eq!(world.owner(5).as_deref(), Some("b"));
}

// Section 2.3: every word is kept, not the last one alone.
#[test]
fn the_words_of_two_regions_are_both_judged_by_the_first_list() {
    let mut world = a_word_before_the_first_list(&["a", "b"]);
    assert_eq!(world.released("a", 6, SECOND), Changes::default());
    world.at(3 * STEP);
    let changes = world.listed(&list(&[(0, 0), (5, WORD), (6, BELOW)], &[], 7));
    // Both are let go and given away within the grace period: region 5 to `b`, as
    // `a` is behind it, and region 6 to `a`, which runs nothing then.
    assert_eq!(world.owner(5).as_deref(), Some("b"));
    assert_eq!(world.owner(6).as_deref(), Some("a"));
    assert!(world.epoch(5) > WORD);
    assert!(world.epoch(6) > SECOND);
    assert_eq!(changes.workers, ["a", "b"]);
    assert_eq!(world.waiting(), [0]);
}

// Q12.2 with a coordinator that is alone, which is what the single process has: the
// one worker says what it let go of before the store has been read.
#[test]
fn a_coordinator_that_is_alone_keeps_a_word_as_well_and_gives_the_region_back() {
    let mut world = World::made(Made::Alone, None);
    world.register("a", &[]);
    assert_eq!(world.released("a", 5, WORD), Changes::default());
    assert_eq!(world.known(), [0; 0]);
    world.at(MOMENT);
    world.listed(&with_region_five(BELOW));
    assert_eq!(world.runs("a"), [0, 5]);
    assert!(world.epoch(5) > WORD);
}

// N18: "If the store is away, the word waits with everything else for the first
// reading that succeeds." Here that is after the grace period, so the list's regions
// are all given away by the call: the lowest first, to `b`, as `a` is behind it for
// its word, and then the region that was let go to `a`, which runs nothing then.
// "No epoch issued from then on is at or below" the word's: also not the one that
// region 0 is given with, which is issued before region 5's.
#[test]
fn a_word_waits_for_a_store_that_is_away_for_longer_than_the_grace_period() {
    let mut world = a_word_before_the_first_list(&["a", "b"]);
    while world.now < world.made + 2 * LEASE {
        world.step(STEP);
        world.unlisted();
        assert_eq!(world.known(), [0; 0]);
    }
    let changes = world.listed(&with_region_five(BELOW));
    assert_eq!(world.owner(0).as_deref(), Some("b"));
    assert_eq!(world.owner(5).as_deref(), Some("a"));
    assert!(
        world.epoch(5) > WORD,
        "{} is not above the word's",
        world.epoch(5)
    );
    assert!(
        world.epoch(0) > WORD,
        "{} is not above the word's",
        world.epoch(0)
    );
    assert_eq!(changes.workers, ["a", "b"]);
    assert_eq!(world.table().waiting, 0);
}

// Section 2.3: a word is judged "if `name` is still registered", which a worker is
// that lost its connection and registered again.
#[test]
fn a_word_is_judged_although_its_worker_has_registered_again_meanwhile() {
    let mut world = a_word_before_the_first_list(&["a", "b"]);
    world.at(3 * STEP);
    world.coordinator.disconnected(world.now, "a");
    world.register_again("a");
    assert_eq!(world.known(), [0; 0]);
    world.at(4 * STEP);
    world.listed(&with_region_five(WORD));
    assert_eq!(world.owner(5).as_deref(), Some("b"));
    assert!(world.epoch(5) > WORD);
    assert_eq!(world.waiting(), [0]);
}

// Q12.10.
#[test]
fn after_the_first_list_no_word_is_kept() {
    let mut world = World::made(Made::New, None);
    world.at(STEP);
    world.register("a", &[]);
    world.register("b", &[]);
    world.listed(&list(&[(0, 0)], &[], 5));
    world.at(2 * STEP);
    let before = world.table();
    assert_eq!(world.released("a", 5, WORD), Changes::default());
    assert_eq!(world.table(), before);
    world.at(3 * STEP);
    let listed = with_region_five(WORD);
    world.listed(&listed);
    assert_eq!((world.known(), world.waiting()), (vec![0, 5], vec![0, 5]));
    // Region 5 waits out the grace period like any region of a list, and nobody is
    // behind anybody.
    let world = waits_out_the_grace_period(&world, 5, Some(&listed));
    assert_eq!(world.owner(0).as_deref(), Some("a"));
    assert_eq!(world.owner(5).as_deref(), Some("b"));
}

// Q12.11.
#[test]
fn a_coordinator_made_knowing_its_regions_keeps_no_word() {
    let mut world = World::knowing(&[0, 1]);
    assert!(!world.coordinator.awaits_the_list());
    world.at(STEP);
    world.register("a", &[]);
    world.register("b", &[]);
    world.at(2 * STEP);
    let before = world.table();
    assert_eq!(world.released("a", 5, WORD), Changes::default());
    assert_eq!(world.table(), before);
    assert_eq!(world.known(), [0, 1]);
    world.at(3 * STEP);
    let listed = list(&[(0, 0), (1, 0), (5, WORD)], &[], 6);
    world.listed(&listed);
    // Region 5 is added without an owner and is not let go.
    assert_eq!(
        (world.known(), world.waiting()),
        (vec![0, 1, 5], vec![0, 1, 5])
    );
    let world = waits_out_the_grace_period(&world, 5, Some(&listed));
    // Nobody is behind anybody: the regions go to `a`, `b` and `a`.
    assert_eq!(world.runs("a"), [0, 5]);
    assert_eq!(world.runs("b"), [1]);
}

// Q12.12.
#[test]
fn the_word_of_a_name_that_is_not_registered_is_not_kept() {
    let mut world = World::made(Made::New, None);
    world.at(STEP);
    world.register("a", &[]);
    world.register("b", &[]);
    world.at(2 * STEP);
    let before = world.table();
    assert_eq!(
        world.coordinator.released(world.now, "z", region(5), WORD),
        Changes::default()
    );
    assert_eq!(world.table(), before);
    world.at(3 * STEP);
    let listed = with_region_five(WORD);
    world.listed(&listed);
    assert_eq!((world.known(), world.waiting()), (vec![0, 5], vec![0, 5]));
    let mut world = waits_out_the_grace_period(&world, 5, Some(&listed));
    assert_eq!(world.owner(0).as_deref(), Some("a"));
    assert_eq!(world.owner(5).as_deref(), Some("b"));
    // To say it was not to register.
    assert!(!world.is_registered("z"));
}

// Q12.12: "A region the coordinator knows without an owner (a list named it) and that
// is released so: as today, by the case there is." Read as: released by a registered
// worker with an epoch that is not below the region's, which lets it go at once.
#[test]
fn a_region_that_a_list_named_is_let_go_by_a_registered_workers_word_as_before() {
    for word in [WORD, SECOND] {
        let mut world = World::made(Made::New, None);
        world.at(STEP);
        world.register("a", &[]);
        world.register("b", &[]);
        world.listed(&with_region_five(WORD));
        assert_eq!(world.waiting(), [0, 5]);
        world.at(2 * STEP);
        let changes = world.released("a", 5, word);
        assert_eq!(world.owner(5).as_deref(), Some("b"), "the word has {word}");
        assert!(world.epoch(5) > word);
        assert_eq!(changes.workers, ["b"]);
        assert_eq!(world.waiting(), [0]);
    }
}

// Q12.12: the other reading of "released so", by a name that is not registered, and
// the word with an epoch below the region's. Neither is a case there is: nothing
// changes.
#[test]
fn a_region_that_a_list_named_is_not_let_go_by_a_stranger_nor_by_a_word_below_its_epoch() {
    let mut world = World::made(Made::New, None);
    world.at(STEP);
    world.register("a", &[]);
    world.register("b", &[]);
    let listed = with_region_five(WORD);
    world.listed(&listed);
    world.at(2 * STEP);
    let before = world.table();
    assert_eq!(
        world.coordinator.released(world.now, "z", region(5), WORD),
        Changes::default()
    );
    assert_eq!(world.released("a", 5, WORD - 1), Changes::default());
    assert_eq!(world.table(), before);
    let world = waits_out_the_grace_period(&world, 5, Some(&listed));
    assert_eq!(world.owner(0).as_deref(), Some("a"));
    assert_eq!(world.owner(5).as_deref(), Some("b"));
}

/// What the log says when a word is kept, when it is taken, and when it is dropped
/// (section 2.3).
const KEPT: &str =
    "a worker released a region this coordinator does not know yet; the first list will say";
const TAKEN: &str = "a worker released a region before this coordinator knew of it";
const DROPPED: &str = "a worker released a region it does not own with that epoch";

// Q12, section 2.3: the lines of the log.
#[test]
fn the_log_says_that_a_word_is_kept_and_what_the_first_list_made_of_it() {
    // Q12.1: kept, and taken by the list.
    let lines = lines_of(|| {
        let mut world = a_word_before_the_first_list(&["a", "b"]);
        world.listed(&with_region_five(WORD));
    });
    assert_eq!(lines.of(KEPT).len(), 1);
    assert_eq!(lines.of(TAKEN).len(), 1);
    assert_eq!(lines.of(DROPPED).len(), 0);
    // Q12.3: kept, and dropped by the list.
    let lines = lines_of(|| {
        let mut world = a_word_before_the_first_list(&["a", "b"]);
        world.listed(&with_region_five(SINCE));
    });
    assert_eq!(lines.of(KEPT).len(), 1);
    assert_eq!(lines.of(TAKEN).len(), 0);
    assert_eq!(lines.of(DROPPED).len(), 1);
}

// ---------------------------------------------------------------------------------
// Q13. A coordinator that is alone gives nobody up (sections 2.3 and 6.6).
// ---------------------------------------------------------------------------------

/// A coordinator with one worker, `a`, that owns region 0, and the epoch it owns it
/// with: handed the home region by the first list if the coordinator is alone, and
/// by its own report if it was made with `new`, which gives nothing away for a lease.
fn one_worker_that_owns_a_region(kind: Made) -> (World, u64) {
    let mut world = World::made(kind, None);
    match kind {
        Made::Alone => {
            world.register("a", &[]);
            world.listed(&home_alone());
        }
        Made::New => {
            world.register("a", &[held(0, 10)]);
        }
    }
    assert_eq!(world.owner(0).as_deref(), Some("a"));
    let epoch = world.epoch(0);
    (world, epoch)
}

/// Holds that `a` has neither lost a region nor failed one: of two workers that run
/// one region each, a new region goes to the one that registered first, unless that
/// one lost or released a region, or is at fault.
fn assert_first_in_line(world: &World) {
    let mut world = world.clone();
    world.register("b", &[held(8, 18)]);
    assert_eq!((world.runs("a"), world.runs("b")), (vec![0], vec![8]));
    world.listed(&list(&[(0, 0), (8, 18), (9, 0)], &[], 10));
    assert_eq!(world.owner(9).as_deref(), Some("a"));
}

// Q13.
#[test]
fn a_coordinator_that_is_alone_keeps_a_worker_of_which_nothing_is_heard() {
    // A tick just before a lease of silence is over, at it, just after it, and after
    // ten leases.
    for silent in [LEASE - MOMENT, LEASE, LEASE + MOMENT, 10 * LEASE] {
        let (mut world, epoch) = one_worker_that_owns_a_region(Made::Alone);
        world.at(silent);
        let changes = world.tick();
        assert_eq!(changes, Changes::default(), "after {silent:?}");
        assert_eq!(world.runs("a"), [0]);
        assert_eq!(world.owner(0).as_deref(), Some("a"));
        assert_eq!(world.epoch(0), epoch);
        assert_eq!(world.table().waiting, 0);
        assert_first_in_line(&world);
        assert!(world.is_registered("a"));
    }
}

// Q13.
#[test]
fn a_coordinator_that_is_alone_keeps_a_region_that_nobody_vouches_for() {
    let (mut world, epoch) = one_worker_that_owns_a_region(Made::Alone);
    // Heartbeats that name no region, and ticks, for ten leases.
    while world.now < world.made + 10 * LEASE {
        world.now += STEP;
        assert!(world.coordinator.heartbeat(world.now, "a", &[]));
        assert_eq!(world.tick(), Changes::default());
        assert_eq!(world.epoch(0), epoch);
    }
    assert_eq!(world.owner(0).as_deref(), Some("a"));
    assert_first_in_line(&world);
}

// Q13: the same made with `new`. A worker is forgotten when it has been silent for
// longer than a lease.
#[test]
fn a_coordinator_made_with_new_forgets_a_worker_that_is_silent_for_longer_than_a_lease() {
    for (silent, kept) in [
        (LEASE - MOMENT, true),
        (LEASE, true),
        (LEASE + MOMENT, false),
        (10 * LEASE, false),
    ] {
        let (mut world, epoch) = one_worker_that_owns_a_region(Made::New);
        world.at(silent);
        world.tick();
        if kept {
            assert_eq!(world.owner(0).as_deref(), Some("a"), "after {silent:?}");
            assert_eq!(world.epoch(0), epoch);
        } else {
            // The worker is forgotten, and the region taken.
            assert_eq!(world.runs("a"), [0; 0], "after {silent:?}");
            assert_eq!(world.owner(0), None);
            assert_eq!((world.known(), world.waiting()), (vec![0], vec![0]));
            assert!(!world.is_registered("a"));
        }
    }
}

// Q13: the same made with `new`, with heartbeats that name no region. The worker
// stays, and the region is taken from it for want of vouching.
#[test]
fn a_coordinator_made_with_new_takes_a_region_that_nobody_vouches_for() {
    let (mut world, epoch) = one_worker_that_owns_a_region(Made::New);
    while world.now < world.made + 2 * LEASE {
        world.now += STEP;
        assert!(world.coordinator.heartbeat(world.now, "a", &[]));
        world.tick();
        if world.now <= world.made + LEASE {
            assert_eq!(
                world.epoch(0),
                epoch,
                "it was vouched for a lease ago or less"
            );
        }
    }
    // Nobody else is there, so it has it back, opened anew.
    let has = world.table().route(region(0)).map(|route| route.epoch);
    assert_ne!(has, Some(epoch));
}

// Section 6.6, the second line of its table, for a region whose worker says that it
// waits for the world store: such a region is vouched for only for so long, and is
// then taken like one that nobody vouches for. Not in one process.
#[test]
fn a_coordinator_that_is_alone_keeps_a_region_whose_worker_waits_for_the_store() {
    for (kind, kept) in [(Made::Alone, true), (Made::New, false)] {
        let (mut world, epoch) = one_worker_that_owns_a_region(kind);
        let waiting = [(region(0), Vouch::WaitingForStore)];
        while world.now < world.made + Coordinator::STORE_PATIENCE + 2 * LEASE {
            world.now += STEP;
            assert!(world.coordinator.heartbeat(world.now, "a", &waiting));
            world.tick();
        }
        let has = world.table().route(region(0)).map(|route| route.epoch);
        if kept {
            assert_eq!(has, Some(epoch), "{kind:?}");
        } else {
            assert_ne!(has, Some(epoch), "{kind:?}");
        }
    }
}

// Section 2.3: "it still takes the regions of one that said it leaves and whose
// connection then ended, which is no silence".
#[test]
fn a_coordinator_that_is_alone_forgets_a_worker_that_said_it_leaves_and_whose_connection_ended() {
    let (mut world, epoch) = one_worker_that_owns_a_region(Made::Alone);
    world.at(STEP);
    let changes = world.coordinator.leaving(world.now, "a");
    // Nobody is there to take its region, so it has it still.
    assert_eq!(changes.gone, [""; 0]);
    assert_eq!(world.owner(0).as_deref(), Some("a"));
    world.at(2 * STEP);
    let changes = world.coordinator.disconnected(world.now, "a");
    assert_eq!(changes.workers, ["a"]);
    assert_eq!(world.owner(0), None);
    assert_eq!((world.known(), world.waiting()), (vec![0], vec![0]));
    assert!(!world.is_registered("a"));

    // The other half: one whose connection ended without that word is kept, for as
    // long as it takes it to come back.
    let (mut world, _) = one_worker_that_owns_a_region(Made::Alone);
    world.at(2 * STEP);
    world.coordinator.disconnected(world.now, "a");
    world.at(10 * LEASE);
    assert_eq!(world.tick(), Changes::default());
    assert_eq!(world.owner(0).as_deref(), Some("a"));
    assert_eq!(world.epoch(0), epoch);
}

/// Two workers, of which `a` runs region 0 and is asked to release it for `b`, never
/// answers, and has it taken by the first tick more than a lease later. Returns the
/// world after that tick.
fn a_release_that_is_never_answered(kind: Made) -> World {
    let mut world = World::made(kind, None);
    world.register("a", &[held(0, 10)]);
    world.register("b", &[]);
    world.at(STEP);
    let asked = world.now;
    world
        .coordinator
        .move_region(asked, region(0), Some("b"), 7)
        .expect("nothing speaks against the move");
    // Until a lease has passed, and when exactly a lease has, it is `a`'s.
    while world.now + STEP <= asked + LEASE {
        world.step(STEP);
        assert_eq!(world.coordinator.assignments("a"), [held(0, 10)]);
    }
    assert_eq!(world.now, asked + LEASE);
    let changes = world.step(MOMENT);
    assert_eq!(world.owner(0).as_deref(), Some("b"), "{kind:?}");
    assert!(world.epoch(0) > 10);
    assert_eq!(changes.moves.len(), 1);
    assert!(!changes.moves[0].released, "it was taken, not let go");
    world
}

// Section 2.3, and "Found while building": a coordinator that is alone notes no
// failure for a release that was not answered either. A worker that failed a region
// comes after every worker that did not, however few regions it has; one of which no
// failure was noted is given the next region for running the fewest.
#[test]
fn a_coordinator_that_is_alone_notes_no_failure_for_a_release_that_was_not_answered() {
    let next = list(&[(0, 10), (9, 0)], &[], 10);
    let mut world = a_release_that_is_never_answered(Made::Alone);
    world.listed(&next);
    assert_eq!(world.owner(9).as_deref(), Some("a"));

    // The other half: made with `new`, the same worker is at fault, and passed over.
    let mut world = a_release_that_is_never_answered(Made::New);
    world.listed(&next);
    assert_eq!(world.owner(9).as_deref(), Some("b"));
}

/// The merge that Q13 has begun by the distances: region 1 into the home region.
const MERGE: Asked = Asked::Merge {
    survivor: RegionId(0),
    absorbed: RegionId(1),
};

/// Q13's second half up to the merge: a coordinator that decides by itself with a
/// rest of one lease, and one worker that owns regions 0 and 1 and reports players of
/// the two within the merge distance, and near where players enter, at every look. A
/// worker of a coordinator that is alone says nothing else; one of a coordinator made
/// with `new` sends heartbeats as well, so that it is not forgotten.
///
/// Returns the world at the tick that began the merge, which the worker never
/// answers, and the epochs of the two regions then.
fn a_merge_by_the_distances_on_one_worker(kind: Made) -> (World, u64, u64) {
    let mut world = World::made(kind, by_itself(LEASE));
    let listed = list(&[(0, 0), (1, 0)], &[], 2);
    world.list = Some(listed.clone());
    world.register("a", &[]);
    if kind == Made::Alone {
        world.heard.clear();
    }
    world.reporting.push("a".to_owned());
    world.crowds.insert(0, vec![(ChunkPos::new(1, 0), 1)]);
    world.crowds.insert(1, vec![(ChunkPos::new(2, 0), 1)]);
    world.listed(&listed);
    // The grace period of one made with `new`, the rest of both regions, and the
    // second for which the merge has to be wanted.
    let limit = world.now + 2 * LEASE + 2 * FRESH;
    loop {
        let before = world.clone();
        let changes = world.look();
        if world.coordinator.under_way().is_empty() {
            assert_eq!(changes.releases, []);
            assert_eq!(changes.orders, []);
            assert!(world.now < limit, "no merge was begun by the distances");
            continue;
        }
        let (home, other) = (before.epoch(0), before.epoch(1));
        assert_eq!(world.coordinator.under_way(), [MERGE]);
        assert_eq!(
            changes.releases,
            [ReleaseOrder {
                worker: "a".to_owned(),
                region: region(1),
                epoch: other,
            }]
        );
        assert_eq!(
            changes.orders,
            [ReshapeOrder {
                worker: "a".to_owned(),
                order: Order::Prepare {
                    region: region(0),
                    epoch: home,
                },
            }]
        );
        return (world, home, other);
    }
}

/// Holds that the tick whose changes these are ended the merge as one whose region
/// was not released, and gave region 1 back to the worker with a higher epoch.
fn assert_ended_unreleased(world: &World, changes: &Changes, home: u64, other: u64) {
    assert_eq!(
        changes.reshaped,
        [Reshaped {
            asker: None,
            asked: MERGE,
            outcome: Err(Undone::NotReleased),
        }]
    );
    assert_eq!(world.coordinator.under_way(), []);
    assert_eq!(world.runs("a"), [0, 1], "the worker is registered");
    assert_eq!(world.owner(1).as_deref(), Some("a"));
    assert!(
        world.epoch(1) > other,
        "{} is not above {other}",
        world.epoch(1)
    );
    assert_eq!(
        world.epoch(0),
        home,
        "the survivor was only told to prepare"
    );
}

/// The looks from the tick that began the merge until it has ended: it is under way
/// just before a lease has passed and when exactly a lease has, and has ended just
/// after. Returns when the merge ended in the world it is given, which is at the
/// first look more than a lease after it was asked.
fn the_release_is_never_answered(world: &mut World, home: u64, other: u64) -> Instant {
    let asked = world.now;
    while world.now + LOOK < asked + LEASE {
        let changes = world.look();
        assert_eq!(changes.reshaped, []);
        assert_eq!(world.coordinator.under_way(), [MERGE]);
    }
    for when in [asked + LEASE - MOMENT, asked + LEASE] {
        let (still, changes) = world.look_at(when);
        assert_eq!(
            changes.reshaped,
            [],
            "{:?} after it was asked",
            when - asked
        );
        assert_eq!(still.coordinator.under_way(), [MERGE]);
        assert_eq!(still.epoch(1), other);
    }
    let (after, changes) = world.look_at(asked + LEASE + MOMENT);
    assert_ended_unreleased(&after, &changes, home, other);

    let changes = world.look();
    assert_eq!(world.now, asked + LEASE);
    assert_eq!(changes.reshaped, []);
    let changes = world.look();
    assert_ended_unreleased(world, &changes, home, other);
    world.now
}

/// Holds that nothing is begun until `when`, with looks up to the one before it and a
/// look just before it, and that the merge is begun by a look just after it. Returns
/// what a look at `when` itself has under way.
fn begun_again_at(world: &mut World, when: Instant) -> Vec<Asked> {
    while world.now + LOOK < when {
        let changes = world.look();
        let before = when - world.now;
        assert_eq!(world.coordinator.under_way(), [], "{before:?} before");
        assert_eq!(changes.releases, [], "{before:?} before");
        assert_eq!(changes.orders, [], "{before:?} before");
    }
    let (before, changes) = world.look_at(when - MOMENT);
    assert_eq!(before.coordinator.under_way(), [], "just before");
    assert_eq!((changes.releases, changes.orders), (vec![], vec![]));
    let (after, changes) = world.look_at(when + MOMENT);
    assert_eq!(after.coordinator.under_way(), [MERGE], "just after");
    assert_eq!(changes.releases.len(), 1);
    let (at, _) = world.look_at(when);
    at.coordinator.under_way()
}

// Section 2.3: "no grace period: ... it assigns at once, evens out at once and decides
// at once." A worker reports regions 0 and 1 and their players, and the list is read,
// when the coordinator is made; the rest is two seconds. One that is alone begins the
// merge when the regions have rested; one made with `new` when its grace period is
// over, which is the one time that holds back what it assigns, evens out and begins.
#[test]
fn a_coordinator_that_is_alone_decides_at_once_and_one_made_with_new_after_its_grace_period() {
    let rest = Duration::from_secs(2);
    for (kind, begins) in [(Made::Alone, rest), (Made::New, LEASE)] {
        let mut world = World::made(kind, by_itself(rest));
        let listed = list(&[(0, 10), (1, 11)], &[], 2);
        world.list = Some(listed.clone());
        world.register("a", &[held(0, 10), held(1, 11)]);
        world.reporting.push("a".to_owned());
        world.crowds.insert(0, vec![(ChunkPos::new(1, 0), 1)]);
        world.crowds.insert(1, vec![(ChunkPos::new(2, 0), 1)]);
        world.listed(&listed);
        let when = world.made + begins;
        assert_eq!(begun_again_at(&mut world, when), [MERGE], "{kind:?}");
    }
}

// Q13: "and it notes no failure".
#[test]
fn a_coordinator_that_is_alone_begins_a_merge_again_three_rests_after_its_release_was_not_answered()
{
    let (mut world, home, other) = a_merge_by_the_distances_on_one_worker(Made::Alone);
    let ended = the_release_is_never_answered(&mut world, home, other);
    // Both regions are left alone as after any merge that came to nothing: three
    // rests (ADR-0016, section 5.5), which here are three leases and well before the
    // six for which a worker that failed a region is at fault.
    let again = ended + 3 * LEASE;
    assert_eq!(world.coordinator.alone_until(region(0)), Some(again));
    assert_eq!(world.coordinator.alone_until(region(1)), Some(again));
    // A region is free when the time is not before the one it is left alone until.
    assert_eq!(begun_again_at(&mut world, again), [MERGE]);
}

// Q13: "the same if nothing at all is heard of the worker for ten leases and then
// one tick comes, followed by its reports".
#[test]
fn a_coordinator_that_is_alone_and_was_held_up_for_ten_leases_in_a_merge_goes_on_as_if_it_had_not_been()
 {
    let (mut world, home, other) = a_merge_by_the_distances_on_one_worker(Made::Alone);
    let asked = world.now;
    world.now = asked + 10 * LEASE;
    let changes = world.tick();
    assert_ended_unreleased(&world, &changes, home, other);
    let ended = world.now;
    let again = ended + 3 * LEASE;
    assert_eq!(world.coordinator.alone_until(region(0)), Some(again));
    assert_eq!(world.coordinator.alone_until(region(1)), Some(again));
    assert_eq!(begun_again_at(&mut world, again), [MERGE]);
}

// Q13: "made with `new` (and heartbeats, so that the worker is not forgotten): the
// same up to the higher epoch, and then nothing is begun with either region until
// six leases after the first merge ended." Whether a look exactly six leases after
// begins it the record does not say, and this does not either.
#[test]
fn a_coordinator_made_with_new_begins_nothing_with_the_regions_of_a_worker_at_fault_for_six_leases()
{
    let (mut world, home, other) = a_merge_by_the_distances_on_one_worker(Made::New);
    let ended = the_release_is_never_answered(&mut world, home, other);
    // The regions are left alone for three rests here as well, so what holds the
    // merge back from then on is the fault.
    assert_eq!(
        world.coordinator.alone_until(region(0)),
        Some(ended + 3 * LEASE)
    );
    assert_eq!(
        world.coordinator.alone_until(region(1)),
        Some(ended + 3 * LEASE)
    );
    let forgotten = ended + Coordinator::FAULT_MEMORY * LEASE;
    begun_again_at(&mut world, forgotten);
}

/// A coordinator that is alone and decides by hand, whose one worker owns regions 0
/// and 1, and the epochs of the two.
fn one_worker_that_owns_two_regions() -> (World, u64, u64) {
    let mut world = World::made(Made::Alone, None);
    world.register("a", &[]);
    world.listed(&list(&[(0, 0), (1, 0)], &[], 2));
    assert_eq!(world.runs("a"), [0, 1]);
    let epochs = (world.epoch(0), world.epoch(1));
    (world, epochs.0, epochs.1)
}

/// What a tick at `when` changes, in a world of its own.
fn tick_at(world: &World, when: Instant) -> (World, Changes) {
    let mut world = world.clone();
    world.now = when;
    let changes = world.tick();
    (world, changes)
}

// Section 6.6: "a merge at its first stage", asked for by hand. The region is taken
// from the worker and, by the same tick, given back to it with another epoch.
#[test]
fn in_one_process_a_merge_whose_release_is_not_answered_gives_the_region_back_by_the_tick_that_ends_it()
 {
    let (mut world, home, other) = one_worker_that_owns_two_regions();
    world.at(STEP);
    let asked = world.now;
    let changes = world
        .coordinator
        .merge(asked, region(0), region(1), ASKER)
        .expect("nothing speaks against the merge");
    assert_eq!(changes.releases.len(), 1);
    for when in [asked + LEASE - MOMENT, asked + LEASE] {
        let (still, changes) = tick_at(&world, when);
        assert_eq!(changes.reshaped, []);
        assert_eq!(still.coordinator.under_way(), [MERGE]);
        assert_eq!(still.epoch(1), other);
    }
    let (after, changes) = tick_at(&world, asked + LEASE + MOMENT);
    assert_eq!(
        changes.reshaped,
        [Reshaped {
            asker: ASKER,
            asked: MERGE,
            outcome: Err(Undone::NotReleased),
        }]
    );
    assert_eq!(after.coordinator.under_way(), []);
    assert_eq!(after.owner(1).as_deref(), Some("a"));
    assert!(after.epoch(1) > other);
    assert_eq!(after.epoch(0), home);
}

/// Section 6.6's "merge at its second stage", asked for by hand: the region to absorb
/// is released and has no owner, the worker was told to absorb it and never says
/// what came of it, and more than a lease has passed. Returns the world after the
/// tick that asks for the list, the survivor's epoch and the epoch to absorb with.
fn in_one_process_an_absorb_that_is_never_answered() -> (World, u64, u64) {
    let (mut world, home, other) = one_worker_that_owns_two_regions();
    world.at(STEP);
    let asked = world.now;
    world
        .coordinator
        .merge(asked, region(0), region(1), ASKER)
        .expect("nothing speaks against the merge");
    world.at(2 * STEP);
    let changes = world.released("a", 1, other);
    let as_epoch = match changes.orders.as_slice() {
        [
            ReshapeOrder {
                worker,
                order:
                    Order::Absorb {
                        region: into,
                        epoch,
                        absorbed,
                        as_epoch,
                    },
            },
        ] if worker == "a" && *into == region(0) && *epoch == home && *absorbed == region(1) => {
            *as_epoch
        }
        other => panic!("expected the order to absorb: {other:?}"),
    };
    assert_eq!(world.owner(1), None, "it waits to be absorbed");
    for when in [asked + LEASE - MOMENT, asked + LEASE] {
        let (still, changes) = tick_at(&world, when);
        assert_eq!(changes, Changes::default());
        assert_eq!(still.coordinator.under_way(), [MERGE]);
    }
    // Nothing is taken from a runner. The list is read, and says.
    world.now = asked + LEASE + MOMENT;
    let changes = world.tick();
    assert!(changes.read, "the list is read first");
    assert_eq!(changes.reshaped, []);
    assert_eq!(world.owner(1), None);
    assert_eq!(world.epoch(0), home);
    (world, home, as_epoch)
}

// Section 6.6: "it was absorbed, and the merge has ended well".
#[test]
fn in_one_process_an_absorb_that_is_overdue_has_ended_well_if_the_list_shows_it() {
    let (mut world, home, _) = in_one_process_an_absorb_that_is_never_answered();
    let changes = world.listed(&list(&[(0, 0)], &[(1, 0)], 2));
    assert_eq!(
        changes.reshaped,
        [Reshaped {
            asker: ASKER,
            asked: MERGE,
            outcome: Ok(region(0)),
        }]
    );
    assert_eq!(world.known(), [0]);
    assert_eq!(world.runs("a"), [0]);
    assert_eq!(world.epoch(0), home);
    assert_eq!(world.coordinator.under_way(), []);
}

// Section 6.6: "it lives, and the merge ends as overdue and the region is given out,
// to the same worker, with another epoch", which is above the one to absorb with.
#[test]
fn in_one_process_an_absorb_that_is_overdue_ends_so_and_the_region_is_given_out_if_the_list_shows_it_living()
 {
    let (mut world, home, as_epoch) = in_one_process_an_absorb_that_is_never_answered();
    let changes = world.listed(&list(&[(0, 0), (1, 0)], &[], 2));
    assert_eq!(
        changes.reshaped,
        [Reshaped {
            asker: ASKER,
            asked: MERGE,
            outcome: Err(Undone::Overdue),
        }]
    );
    assert_eq!(world.runs("a"), [0, 1]);
    assert!(world.epoch(1) > as_epoch);
    assert_eq!(world.epoch(0), home);
    assert_eq!(world.coordinator.under_way(), []);
}

// Section 6.6: "a split: the worker has not said within the lease what came of it.
// Whoever asked is told that it is overdue, the list is read, and no region is
// taken."
#[test]
fn in_one_process_a_split_that_is_not_answered_is_overdue_and_takes_no_region() {
    let (mut world, epoch) = one_worker_that_owns_a_region(Made::Alone);
    world.at(STEP);
    let asked = world.now;
    let split = Asked::Split { region: region(0) };
    let changes = world
        .coordinator
        .split(asked, region(0), &[ChunkPos::new(40, 0)], ASKER)
        .expect("nothing speaks against the split");
    assert_eq!(changes.orders.len(), 1);
    for when in [asked + LEASE - MOMENT, asked + LEASE] {
        let (still, changes) = tick_at(&world, when);
        assert_eq!(changes, Changes::default());
        assert_eq!(still.coordinator.under_way(), [split]);
    }
    world.now = asked + LEASE + MOMENT;
    let changes = world.tick();
    assert_eq!(
        changes.reshaped,
        [Reshaped {
            asker: ASKER,
            asked: split,
            outcome: Err(Undone::Overdue),
        }]
    );
    assert!(changes.read, "the list is read");
    assert_eq!(changes.workers, [""; 0]);
    assert_eq!(world.owner(0).as_deref(), Some("a"));
    assert_eq!(world.epoch(0), epoch);
    assert_eq!(world.coordinator.under_way(), []);
    // The list shows no part, and nothing changes; had the split been made, the part
    // would be reported by the worker or found in the list.
    world.listed(&home_alone());
    assert_eq!(world.runs("a"), [0]);
    assert_eq!(world.epoch(0), epoch);
}

// ---------------------------------------------------------------------------------
// Q14, the state machine's half.
// ---------------------------------------------------------------------------------

// Q14.
#[test]
fn a_coordinator_made_knowing_its_regions_does_not_await_the_list() {
    let mut world = World::knowing(&[0, 1]);
    assert!(!world.coordinator.awaits_the_list());
    assert!(!world.coordinator.keeps_its_workers());
    assert_eq!((world.known(), world.waiting()), (vec![0, 1], vec![0, 1]));
    assert_eq!(world.coordinator.home(), None);
    // A reading that fails leaves it so, and so does one that succeeds.
    world.unlisted();
    assert!(!world.coordinator.awaits_the_list());
    world.listed(&list(&[(0, 0), (1, 0)], &[], 2));
    assert!(!world.coordinator.awaits_the_list());
    assert_eq!(world.coordinator.home(), Some(region(0)));
}

// Section 2.3: a reading that fails leaves a coordinator waiting for its first list,
// and the first that succeeds ends the wait.
#[test]
fn a_coordinator_awaits_the_list_until_it_is_handed_one() {
    for kind in [Made::New, Made::Alone] {
        let mut world = World::made(kind, None);
        world.register("a", &[held(3, 13)]);
        world.unlisted();
        world.at(2 * LEASE);
        world.beat();
        world.tick();
        world.unlisted();
        assert!(world.coordinator.awaits_the_list());
        world.listed(&list(&[(0, 0), (3, 13)], &[], 4));
        assert!(!world.coordinator.awaits_the_list());
        world.unlisted();
        assert!(!world.coordinator.awaits_the_list());
    }
}

// ---------------------------------------------------------------------------------
// The service, over TCP and in its own process, with a list that the test hands it.
// ---------------------------------------------------------------------------------

/// A lease that never runs out in a test: for what is to happen without waiting for
/// one. It is longer than a test waits for anything, so what a coordinator does only
/// when a lease is over fails the test that waits for it.
const LONG_LEASE: Duration = Duration::from_secs(120);

/// A lease that a test waits out: for what is to happen, or not to, when a lease is
/// over. The service reads the real clock, so this is time that passes; the test
/// waits for what the end of it brings about and not for the time. The workers of a
/// test are heard thirty times in it.
const SHORT_LEASE: Duration = Duration::from_secs(3);

/// How often the workers of these tests say that they are there.
const HEARTBEAT: Duration = Duration::from_millis(100);

/// A worker that says nothing by itself: its next heartbeat is an hour away.
const NEVER: Duration = Duration::from_secs(3600);

/// How long a test waits for the service to say something before it gives up. Nothing
/// waits for this to pass; it only keeps a test that would hang from doing so.
const PATIENCE: Duration = Duration::from_secs(60);

/// What the service reads as the world store's list: whatever the test put there
/// last, or nothing, which is a store that does not answer. A test can hold a reading
/// back on its way to the service, after it has looked at the list, and is told of
/// every reading and when it was begun.
#[derive(Clone)]
struct Lists(Arc<Reading>);

struct Reading {
    list: Mutex<Option<RegionList>>,
    /// Whether readings are held back, and how many are under way.
    held: Mutex<(bool, u32)>,
    let_go: Condvar,
    /// The most readings that were ever under way at once.
    most: AtomicU32,
    /// When each reading was begun.
    begun: Mutex<Vec<Instant>>,
    /// Told of every reading when it has looked at the list.
    looked: mpsc::UnboundedSender<()>,
}

impl Lists {
    fn new(list: Option<RegionList>, held: bool) -> (Self, mpsc::UnboundedReceiver<()>) {
        let (looked, hears) = mpsc::unbounded_channel();
        let lists = Self(Arc::new(Reading {
            list: Mutex::new(list),
            held: Mutex::new((held, 0)),
            let_go: Condvar::new(),
            most: AtomicU32::new(0),
            begun: Mutex::new(Vec::new()),
            looked,
        }));
        (lists, hears)
    }

    fn set(&self, list: Option<RegionList>) {
        *self.0.list.lock().expect("no test panics with the list") = list;
    }

    /// The readings that wait go on, and no other waits.
    fn let_go(&self) {
        self.0.held.lock().expect("no test panics with the gate").0 = false;
        self.0.let_go.notify_all();
    }

    fn most_at_once(&self) -> u32 {
        self.0.most.load(Ordering::SeqCst)
    }

    /// When each reading so far was begun.
    fn begun(&self) -> Vec<Instant> {
        self.0
            .begun
            .lock()
            .expect("no test panics with the times")
            .clone()
    }

    /// How many readings have been begun.
    fn count(&self) -> usize {
        self.begun().len()
    }

    fn reader(&self) -> impl Fn() -> io::Result<RegionList> + Send + Sync + 'static {
        let reading = Arc::clone(&self.0);
        move || {
            reading
                .begun
                .lock()
                .expect("no test panics with the times")
                .push(Instant::now());
            let read = reading
                .list
                .lock()
                .expect("no test panics with the list")
                .clone();
            let mut held = reading.held.lock().expect("no test panics with the gate");
            held.1 += 1;
            reading.most.fetch_max(held.1, Ordering::SeqCst);
            // Nobody listens when the test has ended.
            let _ = reading.looked.send(());
            while held.0 {
                held = reading
                    .let_go
                    .wait(held)
                    .expect("no test panics with the gate");
            }
            held.1 -= 1;
            read.ok_or_else(|| io::Error::other("the world store does not answer"))
        }
    }
}

/// A coordinator that is served, and the way to it.
struct Served {
    reach: Reach,
    /// Where it listens, if it is reached over TCP.
    listens: Option<String>,
    lists: Lists,
    fingerprint: u64,
    /// Hears of every reading of the list when it has looked at it.
    looked: mpsc::UnboundedReceiver<()>,
    serving: tokio::task::JoinHandle<()>,
}

impl Drop for Served {
    fn drop(&mut self) {
        // The service ends with its test, and no thread waits at the gate for good.
        self.serving.abort();
        self.lists.let_go();
    }
}

impl Served {
    /// Serves the coordinator that `make` makes on a port of its own, with `list` to
    /// read. It is made when the service is first polled, on the clock its ticks
    /// follow, as `serve` makes its own.
    async fn over_tcp(
        make: impl FnOnce(Instant) -> Coordinator + Send + 'static,
        list: Option<RegionList>,
        held: bool,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port is free");
        let listens = listener
            .local_addr()
            .expect("the listener has an address")
            .to_string();
        let (lists, looked) = Lists::new(list, held);
        let reader = lists.reader();
        let serving = tokio::spawn(async move {
            let now = tokio::time::Instant::now().into_std();
            let served = serve_with(listener, make(now), reader).await;
            served.expect("the listener is of use");
        });
        Self {
            reach: Reach::Tcp(listens.clone()),
            listens: Some(listens),
            lists,
            fingerprint: Layout::single().fingerprint(),
            looked,
            serving,
        }
    }

    /// Serves a coordinator as `clustine coordinator` does, with `serve`, which makes
    /// its coordinator itself.
    async fn as_a_process(config: CoordinatorConfig, list: Option<RegionList>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port is free");
        let listens = listener
            .local_addr()
            .expect("the listener has an address")
            .to_string();
        let (lists, looked) = Lists::new(list, false);
        let fingerprint = config.layout.fingerprint();
        let reader = lists.reader();
        let serving = tokio::spawn(async move {
            let served = serve(listener, config, reader).await;
            served.expect("the listener is of use");
        });
        Self {
            reach: Reach::Tcp(listens.clone()),
            listens: Some(listens),
            lists,
            fingerprint,
            looked,
            serving,
        }
    }

    /// Serves a coordinator in this process, as the single process does: it is alone
    /// with its workers.
    fn local(config: CoordinatorConfig, list: Option<RegionList>, held: bool) -> Self {
        let (lists, looked) = Lists::new(list, held);
        let fingerprint = config.layout.fingerprint();
        let (local, serving) = serve_local(config, lists.reader());
        Self {
            reach: Reach::Local(local),
            listens: None,
            lists,
            fingerprint,
            looked,
            serving: tokio::spawn(serving),
        }
    }

    /// Waits for the next reading to have looked at the list.
    async fn reading(&mut self) {
        within(self.looked.recv())
            .await
            .expect("the service reads the list");
    }

    /// A worker that registers with what it runs already and says every `heartbeat`
    /// that it is there, and what it is answered.
    async fn worker(
        &self,
        name: &str,
        holding: &[Assignment],
        heartbeat: Duration,
    ) -> (WorkerClient, Orders) {
        within(WorkerClient::register_with_heartbeat(
            &self.reach,
            name,
            &address(name),
            holding,
            Some(self.fingerprint),
            heartbeat,
        ))
        .await
        .expect("the worker registers")
    }

    async fn watch(&self) -> RoutingWatch {
        within(RoutingWatch::connect(&self.reach))
            .await
            .expect("the service takes an edge")
    }
}

/// What the future comes to, or a failed test if the service says nothing for a
/// minute.
async fn within<T>(waited: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(PATIENCE, waited)
        .await
        .expect("the service says something within a minute")
}

/// The next orders the worker is told, each region with its epoch. Whatever else it
/// is told before them is not expected here.
async fn orders(worker: &mut WorkerClient) -> Vec<(u32, u64)> {
    match within(worker.event()).await {
        Ok(WorkerEvent::Orders(orders)) => orders
            .assignments
            .iter()
            .map(|held| (held.region.0, held.epoch))
            .collect(),
        other => panic!("expected orders: {other:?}"),
    }
}

/// Reads the worker's orders until they name `id` with another epoch than `but`, and
/// returns that epoch and every orders read, the last ones among them.
async fn orders_naming(
    worker: &mut WorkerClient,
    id: u32,
    but: u64,
) -> (u64, Vec<Vec<(u32, u64)>>) {
    let mut read = Vec::new();
    loop {
        let told = orders(worker).await;
        read.push(told.clone());
        let named = told
            .iter()
            .find(|(region, epoch)| *region == id && *epoch != but);
        if let Some((_, epoch)) = named {
            return (*epoch, read);
        }
    }
}

async fn table(watch: &mut RoutingWatch) -> RoutingTable {
    within(watch.next())
        .await
        .expect("the edge has its connection")
}

/// The first routing table from here on of which `wanted` holds.
async fn table_where(
    watch: &mut RoutingWatch,
    wanted: impl Fn(&RoutingTable) -> bool,
) -> RoutingTable {
    loop {
        let table = table(watch).await;
        if wanted(&table) {
            return table;
        }
    }
}

/// A worker `name` that runs `id`, registers, lets an edge see it, and is then gone
/// without a word; returns when the routing table no longer has its route. **Only a
/// tick brings that about**: a worker whose connection has ended keeps its regions
/// until its lease has run out, and leases are looked at by ticks alone. So between
/// the registration and the return at least a lease of ticks has run.
///
/// Returns how many readings had been begun when the worker had registered and the
/// reading of its registration had looked at the list, if one could: with a reading
/// held back, none can.
async fn a_lease_of_ticks(
    served: &mut Served,
    watch: &mut RoutingWatch,
    name: &str,
    id: u32,
    reads_at_registration: bool,
) -> usize {
    let holding = [held(id, 10 + u64::from(id))];
    while served.looked.try_recv().is_ok() {}
    let (worker, orders) = served.worker(name, &holding, HEARTBEAT).await;
    assert_eq!(orders.assignments, holding);
    if reads_at_registration {
        served.reading().await;
    }
    let theirs = address(name);
    let routed = |table: &RoutingTable| {
        table
            .route(region(id))
            .is_some_and(|route| route.address == theirs)
    };
    table_where(watch, routed).await;
    let registered = served.lists.count();
    drop(worker);
    table_where(watch, |table| !routed(table)).await;
    registered
}

// ---------------------------------------------------------------------------------
// Q5. When the service reads the list.
// ---------------------------------------------------------------------------------

// Q5.
#[tokio::test]
async fn the_service_reads_when_it_begins_to_serve_and_again_at_its_ticks_while_no_reading_succeeds()
 {
    let made = |now| Coordinator::new(config(SHORT_LEASE, None), now, FIRST_EPOCH);
    let mut served = Served::over_tcp(made, None, false).await;
    // Nobody connects, registers or asks for anything: it decides by hand, so nothing
    // but the rule that it reads until it has a list has it read a second time.
    for _ in 0..5 {
        served.reading().await;
    }
    assert_eq!(served.lists.most_at_once(), 1);
}

// Q5: "once more at every tick at which no reading is under way".
#[tokio::test]
async fn a_tick_begins_no_reading_while_one_is_under_way() {
    let made = |now| Coordinator::new(config(SHORT_LEASE, None), now, FIRST_EPOCH);
    let mut served = Served::over_tcp(made, None, true).await;
    // The reading it begins to serve with is held back.
    served.reading().await;
    let mut watch = served.watch().await;
    a_lease_of_ticks(&mut served, &mut watch, "a", 7, false).await;
    assert_eq!(served.lists.count(), 1, "a tick began a second reading");
    // A worker registered meanwhile, so the reading is made again when it is back,
    // and then at the ticks as before.
    served.lists.let_go();
    served.reading().await;
    served.reading().await;
    assert_eq!(served.lists.most_at_once(), 1);
}

// Q5: "after the first reading that succeeds it reads at no tick".
#[tokio::test]
async fn after_the_first_list_the_service_of_a_coordinator_that_decides_by_hand_reads_at_no_tick() {
    let made = |now| Coordinator::new(config(SHORT_LEASE, None), now, FIRST_EPOCH);
    // Region 7 is not below the list's next id, so the list says nothing of it.
    let mut served = Served::over_tcp(made, Some(home_alone()), false).await;
    let mut watch = served.watch().await;
    table_where(&mut watch, |table| table.home == Some(region(0))).await;
    assert_eq!(served.lists.count(), 1, "the first reading succeeded");
    // A registration is an event, and has the list read once.
    let registered = a_lease_of_ticks(&mut served, &mut watch, "a", 7, true).await;
    assert_eq!(registered, 2);
    assert_eq!(served.lists.count(), 2, "a tick began a reading");
}

// Q5: "deciding by itself: the same until the first list". The list is read at every
// tick and not once in a lease, as ADR-0016's timer alone would have it: a lease has
// twenty-four ticks here, and the timer one reading, or two that begin a lease apart.
#[tokio::test]
async fn the_service_of_a_coordinator_that_decides_by_itself_reads_at_its_ticks_until_the_first_list()
 {
    let lease = 2 * SHORT_LEASE;
    let made = move |now| Coordinator::new(config(lease, by_itself(lease)), now, FIRST_EPOCH);
    let mut served = Served::over_tcp(made, None, false).await;
    for _ in 0..3 {
        served.reading().await;
    }
    let mut watch = served.watch().await;
    let registered = a_lease_of_ticks(&mut served, &mut watch, "a", 7, true).await;
    let read = served.lists.count() - registered;
    assert!(read >= 4, "{read} readings in a lease of ticks");
    assert_eq!(served.lists.most_at_once(), 1);
}

/// Holds that a served coordinator that decides by itself, with a lease of `lease`
/// and a store that answers, has the list read on a timer once it has a first list:
/// each reading is begun a lease or more after the one before it.
async fn reads_once_in_a_lease(mut served: Served, lease: Duration) {
    // An edge, which is no reason to read, sees when the first list is in. Whatever
    // was read until then was read to have a first list.
    let mut watch = served.watch().await;
    table_where(&mut watch, |table| table.home == Some(region(0))).await;
    let first = served.lists.count();
    // Nobody registers or asks: the timer alone has it read from here on.
    while served.lists.count() < first + 2 {
        served.reading().await;
    }
    let begun = served.lists.begun();
    for later in first..first + 2 {
        let apart = begun[later] - begun[later - 1];
        assert!(apart >= lease, "two readings {apart:?} apart: {begun:?}");
    }
}

// Q5: "and then as ADR-0016, section 7": on a timer, when the last answer is a lease
// old.
#[tokio::test]
async fn the_service_of_a_coordinator_that_decides_by_itself_reads_once_in_a_lease_after_the_first_list()
 {
    let lease = Duration::from_secs(1);
    let made = move |now| Coordinator::new(config(lease, by_itself(lease)), now, FIRST_EPOCH);
    let served = Served::over_tcp(made, Some(home_alone()), false).await;
    reads_once_in_a_lease(served, lease).await;
}

// Section 6.6: "the list on a timer, every lease, by itself", in the single process
// "from the store in the process, which answers at once".
#[tokio::test]
async fn the_coordinator_of_serve_local_that_decides_by_itself_reads_once_in_a_lease_after_the_first_list()
 {
    let lease = Duration::from_secs(1);
    let config = config(lease, by_itself(lease));
    let served = Served::local(config, Some(home_alone()), false);
    reads_once_in_a_lease(served, lease).await;
}

// N12: "If the store is away for good, nothing starts, and the coordinator's log says
// `the world store's list of regions cannot be read`."
#[tokio::test]
async fn a_served_coordinator_whose_store_is_away_says_so_in_its_log() {
    // The service runs on this thread, so what it writes is written here.
    let lines = log::Lines::default();
    let _listening = tracing::subscriber::set_default(lines.clone());
    tracing::callsite::rebuild_interest_cache();
    let mut served = Served::as_a_process(config(SHORT_LEASE, None), None).await;
    for _ in 0..3 {
        served.reading().await;
    }
    // The third reading was begun when the second had come back and failed.
    let said = lines.of("the world store's list of regions cannot be read");
    assert!(!said.is_empty(), "nothing was said of the store");
}

/// The time on the wall clock, in milliseconds since the Unix epoch: what `serve`
/// and `serve_local` take for the first epoch of their coordinators.
fn unix_milliseconds() -> u64 {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is not before 1970");
    since.as_millis() as u64
}

// Q5 and N12, with `serve` itself, which makes its coordinator with `new` (section
// 5.3): "The coordinator knows no region; workers register and are given nothing ...
// The service reads again at its every tick until the store answers, then the home
// region is assigned, when the grace period is over."
#[tokio::test]
async fn a_coordinator_that_is_served_as_a_process_reads_until_the_store_answers_and_then_assigns_the_home_region()
 {
    let (begun, clock) = (Instant::now(), unix_milliseconds());
    let mut served = Served::as_a_process(config(SHORT_LEASE, None), None).await;
    for _ in 0..3 {
        served.reading().await;
    }
    let mut watch = served.watch().await;
    let knows_nothing = table(&mut watch).await;
    assert_eq!(knows_nothing.routes, []);
    assert_eq!((knows_nothing.waiting, knows_nothing.home), (0, None));
    let (mut a, answer) = served.worker("a", &[], HEARTBEAT).await;
    assert_eq!(answer.assignments, []);

    // The store answers.
    served.lists.set(Some(home_alone()));
    let (given, read) = orders_naming(&mut a, 0, 0).await;
    // The coordinator was made after the test began, so its grace period was not
    // over a lease after that.
    assert!(
        begun.elapsed() >= SHORT_LEASE,
        "it was given the region within the grace period"
    );
    assert_eq!(read, [[(0, given)]]);
    // Its first epoch is from the wall clock.
    assert!(given > clock, "{given} is not above {clock}");
    let table = table_where(&mut watch, |table| table.route(region(0)).is_some()).await;
    assert_eq!((table.waiting, table.home), (0, Some(region(0))));
    assert_eq!(table.route(region(0)).map(|route| route.epoch), Some(given));
}

// ---------------------------------------------------------------------------------
// Q14. A coordinator made knowing its regions is read for on events only.
// ---------------------------------------------------------------------------------

// Q14.
#[tokio::test]
async fn the_service_of_a_coordinator_made_knowing_its_regions_reads_when_it_is_served_and_at_registrations_and_at_no_tick()
 {
    let made = |now| {
        let regions = [region(0), region(1)];
        Coordinator::knowing(config(SHORT_LEASE, None), now, FIRST_EPOCH, &regions)
    };
    let mut served = Served::over_tcp(made, None, false).await;
    // One call when it is served.
    served.reading().await;
    assert_eq!(served.lists.count(), 1);
    // One for each registration.
    let (_a, orders) = served.worker("a", &[held(0, 10)], HEARTBEAT).await;
    assert_eq!(orders.assignments, [held(0, 10)]);
    served.reading().await;
    assert_eq!(served.lists.count(), 2);
    let mut watch = served.watch().await;
    let registered = a_lease_of_ticks(&mut served, &mut watch, "b", 1, true).await;
    assert_eq!(registered, 3);
    // None at any tick, of which a lease and more have run.
    assert_eq!(served.lists.count(), 3, "a tick began a reading");
    assert_eq!(served.lists.most_at_once(), 1);
}

// Q14: "one made with `new` and served the same way: a call at every tick at which
// none is under way."
#[tokio::test]
async fn the_service_of_a_coordinator_made_with_new_and_served_the_same_way_reads_at_its_ticks() {
    let made = |now| Coordinator::new(config(SHORT_LEASE, None), now, FIRST_EPOCH);
    let mut served = Served::over_tcp(made, None, false).await;
    served.reading().await;
    let (_a, _) = served.worker("a", &[held(0, 10)], HEARTBEAT).await;
    served.reading().await;
    let mut watch = served.watch().await;
    let registered = a_lease_of_ticks(&mut served, &mut watch, "b", 1, true).await;
    let read = served.lists.count() - registered;
    assert!(read >= 1, "{read} readings in a lease of ticks");
    assert_eq!(served.lists.most_at_once(), 1);
}

// ---------------------------------------------------------------------------------
// Q9. Reaching a coordinator in its own process.
// ---------------------------------------------------------------------------------

/// A coordinator that is alone with its workers, served over TCP: what `serve_local`
/// serves, at the other door.
async fn alone_over_tcp(lease: Duration, list: Option<RegionList>, held: bool) -> Served {
    let made = move |now| Coordinator::alone(config(lease, None), now, FIRST_EPOCH);
    Served::over_tcp(made, list, held).await
}

/// What Q9 says of a coordinator that is reached through `served`, whichever way
/// that is. Returns what the coordinator said in words, to compare.
async fn reached(served: &Served) -> Vec<String> {
    let mut said = Vec::new();
    let mut watch = served.watch().await;
    let first = table(&mut watch).await;

    // A worker registers and is answered, with where players enter and what it runs.
    let (mut a, answer) = served.worker("a", &[], HEARTBEAT).await;
    assert_eq!(answer.spawn, SPAWN);
    assert_eq!(answer.assignments, []);
    // It is told its orders: the home region of the list.
    let (given, _) = orders_naming(&mut a, 0, 0).await;
    assert!(given > 0);
    // It is heard: it lets go of the region, and is given it again, opened anew.
    a.released(region(0), given);
    let (again, _) = orders_naming(&mut a, 0, given).await;
    assert!(again > given, "{again} is not above {given}");

    // The edge is sent every table: none between two that it reads is left out.
    let mut last = first;
    let mut routed = Vec::new();
    while last.route(region(0)).map(|route| route.epoch) != Some(again) {
        let next = table(&mut watch).await;
        assert!(
            next.version == last.version || next.version == last.version + 1,
            "the table {} was followed by {}",
            last.version,
            next.version
        );
        routed.extend(next.route(region(0)).map(|route| route.epoch));
        last = next;
    }
    assert!(
        routed.contains(&given),
        "the table of the first owner was left out"
    );
    assert_eq!(last.home, Some(region(0)));
    assert_eq!(last.waiting, 0);

    // The service closes a connection: of a worker that leaves and owns nothing,
    let (mut b, answer) = served.worker("b", &[], HEARTBEAT).await;
    assert_eq!(answer.assignments, []);
    b.leaving();
    for _ in 0..2 {
        match within(b.event()).await {
            Err(ClientError::Lost) => {}
            other => panic!("expected the connection to be lost: {other:?}"),
        }
    }
    // of whoever asked for a move, after its last answer,
    let mut mover = within(Mover::ask(&served.reach, region(9), None))
        .await
        .expect("the service takes the request");
    match within(mover.next()).await {
        Ok(MoveAnswer::Refused { reason }) => said.push(reason),
        other => panic!("expected the move to be refused: {other:?}"),
    }
    match within(mover.next()).await {
        Err(ClientError::Lost) => {}
        other => panic!("expected the connection to be lost: {other:?}"),
    }
    // and of whoever asked for a merge, after its one answer.
    let asker = within(Asker::merge(&served.reach, region(8), region(9)))
        .await
        .expect("the service takes the request");
    match within(asker.answer()).await {
        Ok(Err(words)) => said.push(words),
        other => panic!("expected the merge to be refused in words: {other:?}"),
    }
    // The worker that is still there is served on.
    a.released(region(0), again);
    let (third, _) = orders_naming(&mut a, 0, again).await;
    assert!(third > again);
    said
}

// Q9.
#[tokio::test]
async fn a_coordinator_in_its_own_process_serves_its_clients_as_one_over_tcp() {
    let local = Served::local(config(SHORT_LEASE, None), Some(home_alone()), false);
    let in_process = reached(&local).await;
    let tcp = alone_over_tcp(SHORT_LEASE, Some(home_alone()), false).await;
    let over_tcp = reached(&tcp).await;
    assert_eq!(in_process, over_tcp);
    assert_eq!(in_process.len(), 2);
}

// Section 5.3: "when every `LocalCoordinator` is gone nobody can come any more, and
// those who are there are served on."
#[tokio::test]
async fn those_who_are_there_are_served_on_when_the_way_to_a_local_coordinator_is_gone() {
    let (lists, _looked) = Lists::new(Some(home_alone()), false);
    let (local, serving) = serve_local(config(SHORT_LEASE, None), lists.reader());
    let serving = tokio::spawn(serving);
    let registered = WorkerClient::register_with_heartbeat(
        local,
        "a",
        "a:25600",
        &[],
        Some(Layout::single().fingerprint()),
        HEARTBEAT,
    );
    // The one way to it went into that call, and is gone when the call returns.
    let (mut a, _) = within(registered).await.expect("the worker registers");
    let (given, _) = orders_naming(&mut a, 0, 0).await;
    a.released(region(0), given);
    let (again, _) = orders_naming(&mut a, 0, given).await;
    assert!(again > given);
    serving.abort();
}

// Section 5.3, and the comment at `LocalCoordinator`: a way to a coordinator that
// nobody serves any more leads nowhere, as an address does that nobody listens on.
#[tokio::test]
async fn nobody_comes_to_a_local_coordinator_that_is_served_no_more() {
    let (lists, _looked) = Lists::new(None, false);
    let (local, serving) = serve_local(config(SHORT_LEASE, None), lists.reader());
    drop(serving);
    let registered = WorkerClient::register_with_heartbeat(
        local.clone(),
        "a",
        "a:25600",
        &[],
        Some(Layout::single().fingerprint()),
        HEARTBEAT,
    );
    assert!(within(registered).await.is_err());
    assert!(
        within(RoutingWatch::connect(Reach::Local(local)))
            .await
            .is_err()
    );
}

// Section 5.3: "The line `the coordinator is serving`, which `run` writes with `lease`
// and `first_epoch`, is written by those who know the first epoch, `serve_from` and
// `serve_local`, and `serve_with` writes it without."
#[tokio::test]
async fn whoever_serves_a_coordinator_says_so_once_and_with_the_first_epoch_if_it_knows_it() {
    const SERVING: &str = "the coordinator is serving";
    // The services run on this thread, so what they write is written here.
    let lines = log::Lines::default();
    let _listening = tracing::subscriber::set_default(lines.clone());
    tracing::callsite::rebuild_interest_cache();

    // Each has served an edge its table by the time the test looks.
    let served = Served::as_a_process(config(LONG_LEASE, None), Some(home_alone())).await;
    table(&mut served.watch().await).await;
    let said = lines.with(SERVING);
    assert_eq!(said.len(), 1, "`serve`: {said:?}");
    assert!(
        said[0].contains("lease=") && said[0].contains("first_epoch="),
        "{said:?}"
    );

    let served = Served::local(config(LONG_LEASE, None), Some(home_alone()), false);
    table(&mut served.watch().await).await;
    let said = lines.with(SERVING);
    assert_eq!(said.len(), 2, "`serve_local`: {said:?}");
    assert!(
        said[1].contains("lease=") && said[1].contains("first_epoch="),
        "{said:?}"
    );

    let served = alone_over_tcp(LONG_LEASE, Some(home_alone()), false).await;
    table(&mut served.watch().await).await;
    let said = lines.with(SERVING);
    assert_eq!(said.len(), 3, "`serve_with`: {said:?}");
    assert!(!said[2].contains("first_epoch="), "{said:?}");
}

// ---------------------------------------------------------------------------------
// Q10. `serve_local`'s coordinator waits for no lease.
// ---------------------------------------------------------------------------------

// Q10.
#[tokio::test]
async fn the_coordinator_of_serve_local_assigns_a_region_of_the_first_list_without_waiting_for_a_lease()
 {
    // The lease is longer than the test waits for anything.
    let clock = unix_milliseconds();
    let mut served = Served::local(config(LONG_LEASE, None), Some(home_alone()), true);
    served.reading().await;
    let (mut a, answer) = served.worker("a", &[], HEARTBEAT).await;
    assert_eq!(answer.assignments, []);
    served.lists.let_go();
    let (given, read) = orders_naming(&mut a, 0, 0).await;
    assert_eq!(read.len(), 1, "it is told once: {read:?}");
    // Section 5.3: its first epoch is from the wall clock, as `serve` takes it.
    assert!(given > clock, "{given} is not above {clock}");
}

// Q10: the other half. Served the same way, a coordinator made with `new` gives the
// region to nobody: it waits for a lease that is longer than this test.
#[tokio::test]
async fn a_coordinator_made_with_new_and_served_the_same_way_assigns_nothing_by_the_first_list() {
    let made = |now| Coordinator::new(config(LONG_LEASE, None), now, FIRST_EPOCH);
    let mut served = Served::over_tcp(made, Some(home_alone()), true).await;
    served.reading().await;
    let (_a, answer) = served.worker("a", &[], HEARTBEAT).await;
    assert_eq!(answer.assignments, []);
    let mut watch = served.watch().await;
    served.lists.let_go();
    // The list is in, and the region waits.
    let table = table_where(&mut watch, |table| table.home == Some(region(0))).await;
    assert_eq!(table.routes, []);
    assert_eq!(table.waiting, 1);
}

// ---------------------------------------------------------------------------------
// Q12, through the service.
// ---------------------------------------------------------------------------------

// Q12: "a worker registers, says `Released`, is given nothing; the answer is let go,
// and the worker's orders name the region before any tick has run."
//
// The worker also reports a region of its own, 7, and lets go of that one behind the
// word of region 5: the orders that answer that are how the test knows that the
// service has heard both before the list is let through. The list has region 7
// absorbed, so nothing of it is left.
#[tokio::test]
async fn the_service_gives_a_region_that_was_released_before_the_first_list_away_when_the_list_is_let_go()
 {
    // No tick gives anything away in this test: the grace period is the lease.
    let made = |now| Coordinator::new(config(LONG_LEASE, None), now, FIRST_EPOCH);
    let mut served = Served::over_tcp(made, None, true).await;
    served.reading().await;
    let (mut a, answer) = served.worker("a", &[held(7, 1_017)], HEARTBEAT).await;
    assert_eq!(answer.assignments, [held(7, 1_017)]);
    a.released(region(5), WORD);
    a.released(region(7), 1_017);
    // It is given nothing for the word of region 5: its next orders are those for
    // having let go of region 7.
    let told = orders(&mut a).await;
    assert!(
        told.iter().all(|(id, epoch)| *id == 7 && *epoch > 1_017),
        "{told:?}"
    );

    served
        .lists
        .set(Some(list(&[(0, 0), (5, WORD)], &[(7, 0)], 8)));
    served.lists.let_go();
    let (given, read) = orders_naming(&mut a, 5, 0).await;
    assert!(given > WORD, "{given} is not above the word's");
    // Region 0 was not let go, and waits out the grace period.
    for told in &read {
        assert!(told.iter().all(|(id, _)| *id != 0), "{read:?}");
    }
    assert_eq!(read.last(), Some(&vec![(5, given)]));
}

// N18: "If the store is away, the word waits with everything else for the first
// reading that succeeds", which the service makes at a tick. The lease, and so the
// grace period, is four ticks: the region is given away by the reading of the first
// tick after the store answers, and region 0, which nobody let go of, is not.
#[tokio::test]
async fn the_service_gives_a_region_that_was_released_while_the_store_was_away_away_when_it_answers()
 {
    let lease = Duration::from_secs(20);
    let made = move |now| Coordinator::new(config(lease, None), now, FIRST_EPOCH);
    let mut served = Served::over_tcp(made, None, false).await;
    served.reading().await;
    let (mut a, _) = served.worker("a", &[held(7, 1_017)], HEARTBEAT).await;
    a.released(region(5), WORD);
    a.released(region(7), 1_017);
    // The orders for having let go of region 7: both words have been heard.
    let told = orders(&mut a).await;
    assert!(told.iter().all(|(id, _)| *id == 7), "{told:?}");

    served
        .lists
        .set(Some(list(&[(0, 0), (5, WORD)], &[(7, 0)], 8)));
    let (given, read) = orders_naming(&mut a, 5, 0).await;
    assert!(given > WORD, "{given} is not above the word's");
    for told in &read {
        assert!(told.iter().all(|(id, _)| *id != 0), "{read:?}");
    }
    assert_eq!(read.last(), Some(&vec![(5, given)]));
}

// ---------------------------------------------------------------------------------
// Q13, the service's half.
// ---------------------------------------------------------------------------------

/// A client that connects to the service and never says what it is. Returns when the
/// service has closed the connection.
async fn closed_without_a_word(served: &Served) {
    let listens = served.listens.as_ref().expect("it is reached over TCP");
    let mut stream = within(TcpStream::connect(listens))
        .await
        .expect("the service listens");
    let mut byte = [0u8; 1];
    // The end of the connection, or an error that says the same; never a byte.
    let read = within(stream.read(&mut byte)).await;
    assert!(!matches!(read, Ok(1)), "the service says nothing to it");
}

// Q13.
#[tokio::test]
async fn the_service_of_a_coordinator_that_is_alone_closes_no_workers_connection_for_silence() {
    let lease = Duration::from_secs(1);
    let served = alone_over_tcp(lease, None, false).await;
    let (mut a, answer) = served.worker("a", &[held(0, 10)], NEVER).await;
    assert_eq!(answer.assignments, [held(0, 10)]);
    // A client that never says what it is is still closed, a lease after it came:
    // the worker has been silent for longer than that by then, twice over.
    closed_without_a_word(&served).await;
    closed_without_a_word(&served).await;
    // The worker has its connection: it lets go of the region, is heard, and is
    // given it again.
    a.released(region(0), 10);
    let (again, _) = orders_naming(&mut a, 0, 10).await;
    assert!(again > 10);
}

// Q13: the other half. The service of a coordinator made with `new` closes the
// connection of a worker that has been silent for a lease.
#[tokio::test]
async fn the_service_of_a_coordinator_made_with_new_closes_the_connection_of_a_silent_worker() {
    let lease = Duration::from_secs(1);
    let made = move |now| Coordinator::new(config(lease, None), now, FIRST_EPOCH);
    let served = Served::over_tcp(made, None, false).await;
    let (mut a, _) = served.worker("a", &[held(0, 10)], NEVER).await;
    closed_without_a_word(&served).await;
    loop {
        match within(a.event()).await {
            Err(ClientError::Lost) => break,
            Ok(_) => {}
            Err(other) => panic!("expected the connection to be lost: {other:?}"),
        }
    }
}
