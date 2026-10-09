//! The coordinator's part of merging and splitting, tested from the record alone:
//! `docs/adr/0014-merging-and-splitting.md`, section 5 and the scenarios Q1 to Q17 of
//! its section 10, with `docs/adr/0009-moving-a-region.md` for what that section builds
//! on. Whoever wrote these read the records, the messages and the coordinator's public
//! signatures with their comments, and neither its code nor its own tests, so that a
//! test here says what the record asks for and not what the code happens to do.
//!
//! The first part drives the state machine, [`Coordinator`], with the time handed in.
//! The second plays a whole cluster against it from a seed: workers, the world store
//! and its list, and everything that gets lost on the way. It plays five seeds;
//! `CLUSTINE_RESHAPE_RUNS` asks for more and `CLUSTINE_RESHAPE_SEED` for a certain
//! one, and with `--nocapture` each run says what it was about. The third is the
//! service, over TCP, with a list that the test hands it.
//!
//! A test that is ignored as a finding is one that fails: the coordinator does
//! something else there than the record or its own interface says. Its comment has
//! the sequence, what was to happen and what does.
//!
//! The worlds are stripes: region 0 is the westernmost and the home region, and a
//! worker named `a` is reached at `a:25600`, so a route says whose a region is.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use clustine_coordinator::{
    Asked, Asker, Changes, ClientError, Coordinator, CoordinatorConfig, MoveRefusal, Order,
    ReleaseOrder, ReshapeOrder, ReshapeRefusal, Reshaped, RoutingWatch, Undone, WorkerClient,
    WorkerEvent, serve,
};
use clustine_region::{Layout, RegionId, RoutingTable};
use clustine_rpc::{Assignment, Decline, Off, RegionInfo, RegionList, Vouch};
use clustine_world::{ChunkPos, EntityIds, Vec3};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// The lease of the coordinators that are handed their time.
const LEASE: Duration = Duration::from_secs(5);

/// The shortest time that a test tells apart.
const MOMENT: Duration = Duration::from_millis(1);

/// Every epoch a coordinator of these tests issues is above this.
const FIRST_EPOCH: u64 = 1_000;

/// Who asks for the merges and splits of these tests, unless a test says otherwise.
const ASKER: Option<u64> = Some(41);

fn region(id: u32) -> RegionId {
    RegionId(id)
}

fn config(boundaries: &[i32], lease: Duration) -> CoordinatorConfig {
    CoordinatorConfig {
        layout: Layout::new(boundaries.to_vec()).expect("the boundaries ascend"),
        spawn: Vec3::new(0.5, 64.0, 0.5),
        lease,
    }
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

/// The world store's list with `living` regions, of which none was ever opened, the
/// pairs of `absorbed`, and `next` as the id of the next region.
fn list(living: &[u32], absorbed: &[(u32, u32)], next: u32) -> RegionList {
    RegionList {
        home: region(0),
        regions: living
            .iter()
            .map(|id| RegionInfo {
                region: region(*id),
                epoch: 0,
                bounds: None,
                pinned: Vec::new(),
            })
            .collect(),
        absorbed: pairs(absorbed),
        next: region(next),
    }
}

fn pairs(absorbed: &[(u32, u32)]) -> Vec<(RegionId, RegionId)> {
    absorbed
        .iter()
        .map(|(gone, into)| (region(*gone), region(*into)))
        .collect()
}

/// The chunks a split of these tests names. The coordinator passes them on and does not
/// look at them.
fn chunks() -> Vec<ChunkPos> {
    vec![ChunkPos::new(1, 0), ChunkPos::new(1, 1)]
}

fn merge_ended(survivor: u32, absorbed: u32, outcome: Result<u32, Undone>) -> Reshaped {
    Reshaped {
        asker: ASKER,
        asked: Asked::Merge {
            survivor: region(survivor),
            absorbed: region(absorbed),
        },
        outcome: outcome.map(region),
    }
}

fn split_ended(of: u32, outcome: Result<u32, Undone>) -> Reshaped {
    Reshaped {
        asker: ASKER,
        asked: Asked::Split { region: region(of) },
        outcome: outcome.map(region),
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

/// A coordinator, the time, and the workers that go on saying that they are there.
#[derive(Debug, Clone)]
struct Cluster {
    coordinator: Coordinator,
    now: Instant,
    fingerprint: u64,
    /// The workers that send heartbeats while time passes.
    heard: Vec<String>,
    /// Those of them whose heartbeats vouch for nothing.
    vouchless: Vec<String>,
}

impl Cluster {
    /// How far apart the heartbeats and ticks are while time passes.
    const STEP: Duration = Duration::from_millis(500);

    /// A coordinator that has just been made, and knows of no worker.
    fn anew(boundaries: &[i32]) -> Self {
        let config = config(boundaries, LEASE);
        let fingerprint = config.layout.fingerprint();
        let now = Instant::now();
        Self {
            coordinator: Coordinator::new(config, now, FIRST_EPOCH),
            now,
            fingerprint,
            heard: Vec::new(),
            vouchless: Vec::new(),
        }
    }

    /// A coordinator whose grace period is over and whose workers, which registered in
    /// the order given, have been given the regions: the lowest region first, each to
    /// the worker with the fewest, and of those to the one that registered first
    /// (ADR-0009, section 7). So with as many workers as regions, the first has region
    /// 0, the second region 1 and so on.
    fn settled(boundaries: &[i32], workers: &[&str]) -> Self {
        let mut cluster = Self::anew(boundaries);
        for name in workers {
            cluster.register(name, &[]);
        }
        cluster.now += LEASE;
        cluster.beat();
        cluster.now += MOMENT;
        cluster.tick();
        for id in 0..=boundaries.len() {
            let expected = workers[id % workers.len()];
            assert_eq!(cluster.owner(id as u32).as_deref(), Some(expected));
        }
        assert!(cluster.table().is_complete());
        cluster
    }

    /// A coordinator that has just been made, to which workers have reported what they
    /// run, each region with the epoch `10 + id`. It gives nothing away for a lease.
    fn reported(boundaries: &[i32], workers: &[(&str, &[u32])]) -> Self {
        let mut cluster = Self::anew(boundaries);
        for (name, ids) in workers {
            let holding: Vec<Assignment> = ids
                .iter()
                .map(|id| held(*id, 10 + u64::from(*id)))
                .collect();
            cluster.register(name, &holding);
            for id in *ids {
                assert_eq!(cluster.owner(*id).as_deref(), Some(*name));
            }
        }
        cluster
    }

    fn register(&mut self, name: &str, holding: &[Assignment]) -> Changes {
        if !self.heard.iter().any(|heard| heard == name) {
            self.heard.push(name.to_owned());
        }
        self.coordinator
            .register(
                self.now,
                name,
                &address(name),
                holding,
                Some(self.fingerprint),
            )
            .expect("the worker has the coordinator's layout")
    }

    /// The worker registers again with what it was told to run, as one does that
    /// lost its connection.
    fn register_again(&mut self, name: &str) -> Changes {
        let holding = self.coordinator.assignments(name);
        self.register(name, &holding)
    }

    /// The worker says no more from now on.
    fn silence(&mut self, name: &str) {
        self.heard.retain(|heard| heard != name);
    }

    /// The worker's heartbeats vouch for nothing from now on.
    fn stop_vouching(&mut self, name: &str) {
        self.vouchless.push(name.to_owned());
    }

    /// A heartbeat of every worker that is heard, which vouches for all it was given.
    fn beat(&mut self) {
        for name in &self.heard {
            let vouched: Vec<(RegionId, Vouch)> = if self.vouchless.contains(name) {
                Vec::new()
            } else {
                self.coordinator
                    .assignments(name)
                    .iter()
                    .map(|assignment| (assignment.region, Vouch::Committed))
                    .collect()
            };
            self.coordinator.heartbeat(self.now, name, &vouched);
        }
    }

    fn tick(&mut self) -> Changes {
        self.coordinator.tick(self.now)
    }

    /// A moment passes, the workers are heard, and the coordinator looks.
    fn step(&mut self, time: Duration) -> Changes {
        self.now += time;
        self.beat();
        self.tick()
    }

    /// `time` passes with heartbeats and ticks every half second and at its end.
    /// Returns everything the ticks changed.
    fn run(&mut self, time: Duration) -> Changes {
        let end = self.now + time;
        let mut total = Changes::default();
        while self.now < end {
            let step = Self::STEP.min(end - self.now);
            add(&mut total, self.step(step));
        }
        total
    }

    fn listed(&mut self, list: &RegionList) -> Changes {
        self.coordinator.listed(self.now, list)
    }

    /// A reading, and a tick at the same moment: what the reading left without an
    /// owner is then given away if it can be.
    fn listed_and_looked(&mut self, list: &RegionList) -> Changes {
        let mut total = self.listed(list);
        add(&mut total, self.tick());
        total
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

    /// Whether the region is no longer run under `epoch`: it has another owner by now,
    /// or none.
    fn lost(&self, id: u32, epoch: u64) -> bool {
        self.table()
            .route(region(id))
            .is_none_or(|route| route.epoch != epoch)
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

    fn merge(&mut self, survivor: u32, absorbed: u32) -> Changes {
        self.coordinator
            .merge(self.now, region(survivor), region(absorbed), ASKER)
            .unwrap_or_else(|refusal| panic!("merging {absorbed} into {survivor}: {refusal}"))
    }

    /// The owner of the region to absorb says that it has let go of it. Returns the
    /// epoch that the survivor's owner is told to open it with.
    fn let_go(&mut self, survivor: u32, absorbed: u32) -> u64 {
        let owner = self.owner(absorbed).expect("somebody is to release it");
        let (epoch, with) = (self.epoch(absorbed), self.epoch(survivor));
        let survivors = self.owner(survivor).expect("the survivor has an owner");
        let changes = self
            .coordinator
            .released(self.now, &owner, region(absorbed), epoch);
        match changes.orders.as_slice() {
            [
                ReshapeOrder {
                    worker,
                    order:
                        Order::Absorb {
                            region: into,
                            epoch: its,
                            absorbed: gone,
                            as_epoch,
                        },
                },
            ] if *worker == survivors
                && *into == region(survivor)
                && *its == with
                && *gone == region(absorbed) =>
            {
                *as_epoch
            }
            other => panic!("expected the order to absorb {absorbed} into {survivor}: {other:?}"),
        }
    }

    /// A merge up to where the survivor's owner has been told to absorb. Returns the
    /// epoch it was told.
    fn merge_to_absorb(&mut self, survivor: u32, absorbed: u32) -> u64 {
        self.merge(survivor, absorbed);
        self.let_go(survivor, absorbed)
    }

    fn absorb_ended(&mut self, survivor: u32, absorbed: u32, outcome: Result<(), Off>) -> Changes {
        let owner = self.owner(survivor).expect("the survivor has an owner");
        self.coordinator.absorb_ended(
            self.now,
            &owner,
            region(survivor),
            region(absorbed),
            outcome,
        )
    }

    /// Asks for a split and returns the epoch and the id that the owner is told for
    /// the new region.
    fn split(&mut self, of: u32) -> (u64, u32) {
        let owner = self.owner(of).expect("the region has an owner");
        let epoch = self.epoch(of);
        let changes = self
            .coordinator
            .split(self.now, region(of), &chunks(), ASKER)
            .unwrap_or_else(|refusal| panic!("splitting {of}: {refusal}"));
        match changes.orders.as_slice() {
            [
                ReshapeOrder {
                    worker,
                    order:
                        Order::SplitOff {
                            region: split,
                            epoch: its,
                            chunks: named,
                            as_epoch,
                            part,
                        },
                },
            ] if *worker == owner
                && *split == region(of)
                && *its == epoch
                && *named == chunks() =>
            {
                (*as_epoch, part.0)
            }
            other => panic!("expected the order to split {of}: {other:?}"),
        }
    }

    fn split_ended(&mut self, of: u32, as_epoch: u64, outcome: Result<u32, Off>) -> Changes {
        let owner = self.owner(of).expect("the region has an owner");
        self.coordinator
            .split_ended(self.now, &owner, region(of), as_epoch, outcome.map(region))
    }
}

/// What a coordinator does from here on while its workers go on as they are: what
/// each tick of three leases changes, and the routing table after it.
fn what_follows(cluster: &Cluster) -> Vec<(Changes, RoutingTable)> {
    let mut cluster = cluster.clone();
    (0..30)
        .map(|_| {
            let changes = cluster.step(Cluster::STEP);
            (changes, cluster.table())
        })
        .collect()
}

/// Nothing that can be seen of the coordinator is other than it was, now or later.
fn assert_nothing_changed(before: &Cluster, after: &Cluster) {
    assert_eq!(after.table(), before.table());
    assert_eq!(after.waiting(), before.waiting());
    for name in &before.heard {
        assert_eq!(
            after.coordinator.assignments(name),
            before.coordinator.assignments(name)
        );
    }
    assert_eq!(what_follows(after), what_follows(before));
    assert_eq!(
        format!("{:?}", after.coordinator),
        format!("{:?}", before.coordinator)
    );
}

/// The merge is refused with a reason that `expected` holds of, and nothing changes.
fn assert_merge_refused_as(
    cluster: &mut Cluster,
    survivor: u32,
    absorbed: u32,
    expected: impl Fn(&ReshapeRefusal) -> bool,
) {
    let before = cluster.clone();
    let answer = cluster
        .coordinator
        .merge(cluster.now, region(survivor), region(absorbed), ASKER);
    match &answer {
        Err(refusal) if expected(refusal) => {}
        other => panic!("merging {absorbed} into {survivor}: {other:?}"),
    }
    assert_nothing_changed(&before, cluster);
}

fn assert_merge_refused(
    cluster: &mut Cluster,
    survivor: u32,
    absorbed: u32,
    expected: ReshapeRefusal,
) {
    assert_merge_refused_as(cluster, survivor, absorbed, |refusal| *refusal == expected);
}

/// The split of `chunks` off the region is refused with a reason that `expected` holds
/// of, and nothing changes.
fn assert_split_refused_as(
    cluster: &mut Cluster,
    of: u32,
    chunks: &[ChunkPos],
    expected: impl Fn(&ReshapeRefusal) -> bool,
) {
    let before = cluster.clone();
    let answer = cluster
        .coordinator
        .split(cluster.now, region(of), chunks, ASKER);
    match &answer {
        Err(refusal) if expected(refusal) => {}
        other => panic!("splitting {of}: {other:?}"),
    }
    assert_nothing_changed(&before, cluster);
}

fn assert_split_refused(cluster: &mut Cluster, of: u32, expected: ReshapeRefusal) {
    assert_split_refused_as(cluster, of, &chunks(), |refusal| *refusal == expected);
}

/// A merge or a split ended because of what became of the region: the list no longer
/// has it, or it lost its owner. Where a reading takes a region with an owner away,
/// both are so, and the record does not say which of the two is told.
fn lost_or_gone(ended: &[Reshaped], asked: Asked, id: u32) -> bool {
    matches!(
        ended,
        [Reshaped { asker, asked: of, outcome: Err(Undone::Gone(named) | Undone::Disowned(named)) }]
            if *asker == ASKER && *of == asked && *named == region(id)
    )
}

fn unfit(name: &'static str) -> impl Fn(&ReshapeRefusal) -> bool {
    move |refusal| matches!(refusal, ReshapeRefusal::Unfit { worker, .. } if worker == name)
}

/// The three stripes as the world store has them before anything was merged or split.
fn three_stripes() -> RegionList {
    list(&[0, 1, 2], &[], 3)
}

/// Three stripes, each with a worker of its own, and the list read once.
fn three_workers() -> Cluster {
    let mut cluster = Cluster::settled(&[0, 4], &["a", "b", "c"]);
    cluster.listed(&three_stripes());
    cluster
}

/// Four stripes, each with a worker of its own, and the list read once.
fn four_workers() -> Cluster {
    let mut cluster = Cluster::settled(&[0, 4, 8], &["a", "b", "c", "d"]);
    cluster.listed(&list(&[0, 1, 2, 3], &[], 4));
    cluster
}

/// A coordinator in its grace period, in which every reason to refuse is to be found at
/// once: region 0 is the home region; 1 is to absorb 2; 3 is being moved; 4 has no
/// owner; 5 has an owner without a connection; 6 and 7 have nothing against them.
fn every_reason() -> Cluster {
    let mut cluster = Cluster::reported(
        &[0, 4, 8, 12, 16, 20, 24],
        &[
            ("a", &[0]),
            ("b", &[1, 2]),
            ("c", &[3]),
            ("d", &[]),
            ("e", &[5]),
            ("f", &[6, 7]),
        ],
    );
    cluster.listed(&list(&[0, 1, 2, 3, 4, 5, 6, 7], &[], 8));
    cluster.merge(1, 2);
    cluster
        .coordinator
        .move_region(cluster.now, region(3), Some("d"), 1)
        .expect("region 3 can be moved to the worker that has none");
    cluster.coordinator.disconnected(cluster.now, "e");
    cluster.silence("e");
    assert_eq!(cluster.waiting(), [4]);
    cluster
}

// ---------------------------------------------------------------------------------
// Q1. Each refusal of a merge and of a split, with nothing changed.
// ---------------------------------------------------------------------------------

#[test]
fn a_merge_that_names_a_region_nobody_knows_is_refused() {
    let mut cluster = three_workers();
    assert_merge_refused(&mut cluster, 1, 9, ReshapeRefusal::NoSuchRegion(region(9)));
    assert_merge_refused(&mut cluster, 9, 1, ReshapeRefusal::NoSuchRegion(region(9)));
    // The survivor is looked at first.
    assert_merge_refused(&mut cluster, 8, 9, ReshapeRefusal::NoSuchRegion(region(8)));
    // That comes before two names being one.
    assert_merge_refused(&mut cluster, 9, 9, ReshapeRefusal::NoSuchRegion(region(9)));
}

#[test]
fn a_region_is_not_merged_with_itself() {
    let mut cluster = three_workers();
    assert_merge_refused(&mut cluster, 1, 1, ReshapeRefusal::Same);
    // Also the home region, which is refused as that only after this.
    assert_merge_refused(&mut cluster, 0, 0, ReshapeRefusal::Same);
}

#[test]
fn the_home_region_of_the_last_reading_is_not_absorbed() {
    let mut cluster = three_workers();
    assert_merge_refused(&mut cluster, 1, 0, ReshapeRefusal::Home);
    assert_merge_refused(&mut cluster, 2, 0, ReshapeRefusal::Home);

    // It is the list that says which region that is, each reading anew.
    let mut elsewhere = three_stripes();
    elsewhere.home = region(2);
    cluster.listed(&elsewhere);
    assert_merge_refused(&mut cluster, 1, 2, ReshapeRefusal::Home);
    cluster.merge(1, 0);
}

#[test]
fn a_merge_with_a_region_of_another_merge_is_refused_as_reserved() {
    let mut cluster = four_workers();
    cluster.merge(1, 2);
    for (survivor, absorbed, reserved) in [
        (1, 3, 1),
        (3, 1, 1),
        (2, 3, 2),
        (3, 2, 2),
        // Of two that are reserved, the survivor is named.
        (1, 2, 1),
        (2, 1, 2),
    ] {
        assert_merge_refused(
            &mut cluster,
            survivor,
            absorbed,
            ReshapeRefusal::Reserved(region(reserved)),
        );
    }

    // When the other region has been released, both are reserved still.
    cluster.let_go(1, 2);
    for (survivor, absorbed, reserved) in [(1, 3, 1), (3, 1, 1), (2, 3, 2), (3, 2, 2), (1, 2, 1)] {
        assert_merge_refused(
            &mut cluster,
            survivor,
            absorbed,
            ReshapeRefusal::Reserved(region(reserved)),
        );
    }
}

#[test]
fn a_merge_with_a_region_that_is_being_split_is_refused_as_reserved() {
    let mut cluster = four_workers();
    cluster.split(1);
    assert_merge_refused(&mut cluster, 1, 2, ReshapeRefusal::Reserved(region(1)));
    assert_merge_refused(&mut cluster, 2, 1, ReshapeRefusal::Reserved(region(1)));
}

#[test]
fn a_merge_with_a_region_that_is_being_released_is_refused() {
    let mut cluster = four_workers();
    cluster
        .coordinator
        .move_region(cluster.now, region(1), Some("d"), 1)
        .expect("region 1 can be moved");
    assert_merge_refused(&mut cluster, 1, 2, ReshapeRefusal::BeingReleased(region(1)));
    assert_merge_refused(&mut cluster, 2, 1, ReshapeRefusal::BeingReleased(region(1)));

    // Of two that are being released, the survivor is named.
    cluster
        .coordinator
        .move_region(cluster.now, region(2), Some("d"), 2)
        .expect("region 2 can be moved");
    assert_merge_refused(&mut cluster, 1, 2, ReshapeRefusal::BeingReleased(region(1)));
    assert_merge_refused(&mut cluster, 2, 1, ReshapeRefusal::BeingReleased(region(2)));
}

#[test]
fn a_merge_with_a_region_that_is_being_released_to_even_out_is_refused() {
    // One worker has all four regions, and a second one comes: the next tick begins
    // to hand the highest region over to it.
    let mut cluster = Cluster::settled(&[0, 4, 8], &["a"]);
    cluster.listed(&list(&[0, 1, 2, 3], &[], 4));
    cluster.register("b", &[]);
    let changes = cluster.step(MOMENT);
    let [release] = changes.releases.as_slice() else {
        panic!("expected one release to even out: {changes:?}");
    };
    assert_eq!((release.worker.as_str(), release.region), ("a", region(3)));

    assert_merge_refused(&mut cluster, 3, 1, ReshapeRefusal::BeingReleased(region(3)));
    assert_merge_refused(&mut cluster, 1, 3, ReshapeRefusal::BeingReleased(region(3)));
    assert_split_refused(&mut cluster, 3, ReshapeRefusal::BeingReleased(region(3)));
}

#[test]
fn a_merge_with_a_region_that_a_leaver_is_releasing_is_refused() {
    let mut cluster = four_workers();
    let changes = cluster.coordinator.leaving(cluster.now, "b");
    assert_eq!(changes.releases.len(), 1, "{changes:?}");
    assert_merge_refused(&mut cluster, 1, 2, ReshapeRefusal::BeingReleased(region(1)));
    assert_merge_refused(&mut cluster, 2, 1, ReshapeRefusal::BeingReleased(region(1)));
    assert_split_refused(&mut cluster, 1, ReshapeRefusal::BeingReleased(region(1)));
}

#[test]
fn a_merge_with_a_region_without_an_owner_is_refused() {
    let mut cluster = Cluster::reported(&[0, 4, 8, 12], &[("a", &[1]), ("b", &[2])]);
    assert_eq!(cluster.waiting(), [0, 3, 4]);
    assert_merge_refused(&mut cluster, 3, 1, ReshapeRefusal::NoOwner(region(3)));
    assert_merge_refused(&mut cluster, 1, 3, ReshapeRefusal::NoOwner(region(3)));
    // Of two without an owner, the survivor is named.
    assert_merge_refused(&mut cluster, 3, 4, ReshapeRefusal::NoOwner(region(3)));
    assert_merge_refused(&mut cluster, 4, 3, ReshapeRefusal::NoOwner(region(4)));
}

#[test]
fn a_merge_whose_worker_has_no_connection_is_refused() {
    let mut cluster = four_workers();
    cluster.coordinator.disconnected(cluster.now, "b");
    // As the survivor's owner it could not be told to absorb, and as the other's it
    // could not be told to release.
    assert_merge_refused_as(&mut cluster, 1, 2, unfit("b"));
    assert_merge_refused_as(&mut cluster, 2, 1, unfit("b"));

    // Of two such workers, the survivor's is named.
    cluster.coordinator.disconnected(cluster.now, "c");
    assert_merge_refused_as(&mut cluster, 1, 2, unfit("b"));
    assert_merge_refused_as(&mut cluster, 2, 1, unfit("c"));

    // A worker that registers again has a connection.
    cluster.register_again("b");
    cluster.register_again("c");
    cluster.merge(1, 2);
}

#[test]
fn a_merge_whose_survivor_is_run_by_a_worker_that_leaves_is_refused() {
    // The only worker there is has nobody to hand anything over to, so that it says
    // it is leaving begins no release.
    let mut cluster = Cluster::settled(&[0, 4], &["a"]);
    cluster.listed(&three_stripes());
    let changes = cluster.coordinator.leaving(cluster.now, "a");
    assert_eq!(changes.releases, []);
    assert_eq!(changes.gone, [] as [String; 0]);
    assert_merge_refused_as(&mut cluster, 1, 2, unfit("a"));
    assert_split_refused_as(&mut cluster, 1, &chunks(), unfit("a"));
}

/// Of several reasons the first of the record's table is given, whichever of the two
/// regions it holds of: here the survivor always has a reason that comes later in the
/// table than the one the region to absorb has.
#[test]
fn of_several_reasons_to_refuse_a_merge_the_first_of_the_records_order_is_given() {
    let mut cluster = every_reason();
    // The survivor is reserved; the other is unknown, then the home region.
    assert_merge_refused(&mut cluster, 1, 9, ReshapeRefusal::NoSuchRegion(region(9)));
    assert_merge_refused(&mut cluster, 1, 0, ReshapeRefusal::Home);
    // The survivor is being released; the other is reserved.
    assert_merge_refused(&mut cluster, 3, 2, ReshapeRefusal::Reserved(region(2)));
    // The survivor has no owner; the other is being released.
    assert_merge_refused(&mut cluster, 4, 3, ReshapeRefusal::BeingReleased(region(3)));
    // The survivor's owner has no connection; the other has no owner.
    assert_merge_refused(&mut cluster, 5, 4, ReshapeRefusal::NoOwner(region(4)));
}

/// The same the other way round: the survivor's reason is the earlier one.
#[test]
fn each_reason_to_refuse_a_merge_is_given_for_the_survivor_too() {
    let mut cluster = every_reason();
    assert_merge_refused(&mut cluster, 9, 1, ReshapeRefusal::NoSuchRegion(region(9)));
    assert_merge_refused(&mut cluster, 2, 3, ReshapeRefusal::Reserved(region(2)));
    assert_merge_refused(&mut cluster, 3, 4, ReshapeRefusal::BeingReleased(region(3)));
    assert_merge_refused(&mut cluster, 4, 5, ReshapeRefusal::NoOwner(region(4)));
    assert_merge_refused_as(&mut cluster, 5, 6, unfit("e"));
    assert_merge_refused_as(&mut cluster, 6, 5, unfit("e"));
    // And with nothing against either, the merge is taken on.
    cluster.merge(6, 7);
}

#[test]
fn each_reason_to_refuse_a_split_is_given_in_the_records_order() {
    let mut cluster = every_reason();
    // Each of these names no chunks as well, which is the last reason of the record.
    let refused: [(u32, ReshapeRefusal); 5] = [
        (9, ReshapeRefusal::NoSuchRegion(region(9))),
        (1, ReshapeRefusal::Reserved(region(1))),
        (2, ReshapeRefusal::Reserved(region(2))),
        (3, ReshapeRefusal::BeingReleased(region(3))),
        (4, ReshapeRefusal::NoOwner(region(4))),
    ];
    for (of, expected) in refused {
        assert_split_refused_as(&mut cluster, of, &[], |refusal| *refusal == expected);
        assert_split_refused(&mut cluster, of, expected);
    }
    assert_split_refused_as(&mut cluster, 5, &[], unfit("e"));
    assert_split_refused_as(&mut cluster, 5, &chunks(), unfit("e"));
    assert_split_refused_as(&mut cluster, 6, &[], |refusal| {
        *refusal == ReshapeRefusal::NoChunks
    });
    // And with nothing against it, the split is taken on, of the home region too.
    cluster.split(6);
    cluster.split(0);
}

#[test]
fn a_split_of_a_region_that_is_being_split_is_refused_as_reserved() {
    let mut cluster = three_workers();
    cluster.split(1);
    assert_split_refused(&mut cluster, 1, ReshapeRefusal::Reserved(region(1)));
    // Another region can be split meanwhile.
    cluster.split(2);
}

/// The new region is named by the next id of the list, so there is nothing to order
/// before the list has been read (the builder's note; section 5.4 has "after a reading
/// of the list").
#[test]
fn a_split_is_refused_until_the_list_has_been_read() {
    let mut cluster = Cluster::settled(&[0, 4], &["a", "b", "c"]);
    assert_split_refused(&mut cluster, 1, ReshapeRefusal::Unlisted);
    cluster.coordinator.unlisted(cluster.now);
    assert_split_refused(&mut cluster, 1, ReshapeRefusal::Unlisted);
    cluster.listed(&three_stripes());
    cluster.split(1);
}

// ---------------------------------------------------------------------------------
// Q2. A merge in order.
// ---------------------------------------------------------------------------------

#[test]
fn a_merge_begins_with_a_release_of_the_one_region_and_a_prepare_for_the_other() {
    let mut cluster = three_workers();
    let before = cluster.table();
    let (survivors, absorbeds) = (cluster.epoch(0), cluster.epoch(1));

    let changes = cluster.merge(0, 1);
    assert_eq!(
        changes.releases,
        [ReleaseOrder {
            worker: "b".to_owned(),
            region: region(1),
            epoch: absorbeds,
        }]
    );
    assert_eq!(
        changes.orders,
        [ReshapeOrder {
            worker: "a".to_owned(),
            order: Order::Prepare {
                region: region(0),
                epoch: survivors,
            },
        }]
    );
    assert_eq!(changes.reshaped, []);
    assert_eq!(changes.moves, []);
    // Whose the regions are has not changed by the asking.
    assert_eq!(changes.workers, [] as [String; 0]);
    assert!(!changes.routing);
    assert_eq!(cluster.table(), before);
}

#[test]
fn the_released_region_of_a_merge_is_not_assigned_and_is_opened_with_an_epoch_above_every_other() {
    let mut cluster = three_workers();
    let before = cluster.table();
    let (survivors, absorbeds) = (cluster.epoch(0), cluster.epoch(1));
    let highest = cluster.highest_epoch();
    cluster.merge(0, 1);

    let changes = cluster
        .coordinator
        .released(cluster.now, "b", region(1), absorbeds);
    let [
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
    ] = changes.orders.as_slice()
    else {
        panic!("expected the order to absorb: {changes:?}");
    };
    assert_eq!(worker, "a");
    assert_eq!(
        (*into, *epoch, *absorbed),
        (region(0), survivors, region(1))
    );
    assert!(*as_epoch > highest, "{as_epoch} is not above {highest}");

    // The region is nobody's, and stays so.
    assert_eq!(changes.workers, ["b"]);
    assert_eq!(cluster.runs("b"), [] as [u32; 0]);
    assert_eq!(cluster.waiting(), [1]);
    assert_eq!(changes.reshaped, []);
    assert_eq!(changes.moves, []);
    assert!(changes.routing);
    let table = cluster.table();
    assert!(table.version > before.version);
    assert_eq!(table.route(region(1)), None);
    assert_eq!(table.waiting, 1);
    assert!(!table.is_complete());
    assert_eq!(table.route(region(0)), before.route(region(0)));
    assert_eq!(table.route(region(2)), before.route(region(2)));
}

#[test]
fn the_word_that_an_absorb_has_ended_changes_nothing_and_has_the_list_read() {
    let mut cluster = three_workers();
    cluster.merge_to_absorb(0, 1);
    let before = cluster.table();

    let changes = cluster.absorb_ended(0, 1, Ok(()));
    assert_eq!(
        changes,
        Changes {
            read: true,
            ..Changes::default()
        }
    );
    assert_eq!(cluster.table(), before);
    assert_eq!(cluster.waiting(), [1]);
}

#[test]
fn a_merge_ends_with_the_list_that_has_the_pair() {
    let mut cluster = three_workers();
    cluster.merge_to_absorb(0, 1);
    cluster.absorb_ended(0, 1, Ok(()));
    let before = cluster.table();

    let changes = cluster.listed(&list(&[0, 2], &[(1, 0)], 3));
    assert_eq!(changes.reshaped, [merge_ended(0, 1, Ok(0))]);
    assert!(changes.routing);
    let table = cluster.table();
    assert!(table.version > before.version);
    assert_eq!(table.absorbed, pairs(&[(1, 0)]));
    assert_eq!(table.home, Some(region(0)));
    assert_eq!(
        table.routes,
        [
            before
                .route(region(0))
                .expect("the survivor has its owner")
                .clone(),
            before
                .route(region(2))
                .expect("the third region has its owner")
                .clone(),
        ]
    );
    assert_eq!(table.waiting, 0);
    assert!(table.is_complete());

    // The region is gone for good: nothing waits, nothing is assigned, and it cannot
    // be named any more.
    assert_eq!(cluster.known(), [0, 2]);
    assert_eq!(cluster.run(LEASE * 3).workers, [] as [String; 0]);
    assert_eq!(cluster.known(), [0, 2]);
    assert_eq!(cluster.runs("b"), [] as [u32; 0]);
    assert_merge_refused(&mut cluster, 0, 1, ReshapeRefusal::NoSuchRegion(region(1)));
    // And both regions are free again.
    cluster.merge(0, 2);
}

#[test]
fn whoever_asked_for_a_merge_is_told_once() {
    let mut cluster = three_workers();
    cluster.merge_to_absorb(0, 1);
    cluster.absorb_ended(0, 1, Ok(()));
    let after = list(&[0, 2], &[(1, 0)], 3);
    assert_eq!(cluster.listed(&after).reshaped, [merge_ended(0, 1, Ok(0))]);

    // Neither a second word of the worker nor another reading says it again.
    let again = cluster.absorb_ended(0, 1, Ok(()));
    assert_eq!(again.reshaped, []);
    assert!(again.read);
    assert_eq!(cluster.listed(&after).reshaped, []);
    assert_eq!(cluster.run(LEASE * 3).reshaped, []);
}

#[test]
fn a_merge_of_two_regions_of_one_worker_goes_the_same_way() {
    let mut cluster = Cluster::settled(&[0, 4], &["a"]);
    cluster.listed(&three_stripes());
    let (survivors, absorbeds) = (cluster.epoch(1), cluster.epoch(2));

    let changes = cluster.merge(1, 2);
    assert_eq!(
        changes.releases,
        [ReleaseOrder {
            worker: "a".to_owned(),
            region: region(2),
            epoch: absorbeds,
        }]
    );
    assert_eq!(
        changes.orders,
        [ReshapeOrder {
            worker: "a".to_owned(),
            order: Order::Prepare {
                region: region(1),
                epoch: survivors,
            },
        }]
    );
    let as_epoch = cluster.let_go(1, 2);
    assert!(as_epoch > absorbeds);
    assert_eq!(cluster.runs("a"), [0, 1]);
    // The only worker there is has the fewest regions, and is still not given it.
    assert_eq!(cluster.run(LEASE / 2).workers, [] as [String; 0]);
    assert_eq!(cluster.waiting(), [2]);

    cluster.absorb_ended(1, 2, Ok(()));
    let changes = cluster.listed(&list(&[0, 1], &[(2, 1)], 3));
    assert_eq!(changes.reshaped, [merge_ended(1, 2, Ok(1))]);
    assert_eq!(cluster.known(), [0, 1]);
    assert_eq!(cluster.runs("a"), [0, 1]);
}

/// ADR-0009 has a worker that registers without a region it was asked to release
/// taken at its word, and section 5.3 has that count for a merge too.
#[test]
fn registering_without_the_region_is_its_release_for_a_merge() {
    let mut cluster = three_workers();
    let survivors = cluster.epoch(0);
    let highest = cluster.highest_epoch();
    cluster.merge(0, 1);

    let changes = cluster.register("b", &[]);
    let [
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
    ] = changes.orders.as_slice()
    else {
        panic!("expected the order to absorb: {changes:?}");
    };
    assert_eq!(worker, "a");
    assert_eq!(
        (*into, *epoch, *absorbed),
        (region(0), survivors, region(1))
    );
    assert!(*as_epoch > highest);
    assert_eq!(cluster.runs("b"), [] as [u32; 0]);
    assert_eq!(cluster.waiting(), [1]);
    assert_eq!(cluster.run(LEASE / 2).workers, [] as [String; 0]);
    assert_eq!(cluster.waiting(), [1]);
}

#[test]
fn the_owner_that_registers_with_the_region_it_is_to_release_for_a_merge_is_asked_again() {
    let mut cluster = three_workers();
    let absorbeds = cluster.epoch(1);
    cluster.merge(0, 1);
    cluster.run(LEASE / 2);

    let changes = cluster.register_again("b");
    assert_eq!(
        changes.releases,
        [ReleaseOrder {
            worker: "b".to_owned(),
            region: region(1),
            epoch: absorbeds,
        }]
    );
    assert_eq!(changes.orders, []);
    assert_eq!(cluster.runs("b"), [1]);

    // The merge's time still counts from when it was asked.
    let quiet = cluster.run(LEASE / 2);
    assert_eq!(quiet.reshaped, []);
    let over = cluster.step(MOMENT);
    assert_eq!(over.reshaped, [merge_ended(0, 1, Err(Undone::NotReleased))]);
}

#[test]
fn a_release_from_another_worker_or_with_another_epoch_does_not_move_a_merge_on() {
    let mut cluster = three_workers();
    let absorbeds = cluster.epoch(1);
    cluster.merge(0, 1);
    for (name, epoch) in [("a", absorbeds), ("c", absorbeds), ("b", absorbeds - 1)] {
        let changes = cluster
            .coordinator
            .released(cluster.now, name, region(1), epoch);
        assert_eq!(changes.orders, [], "{name} with {epoch}");
        assert_eq!(cluster.owner(1).as_deref(), Some("b"));
    }
    cluster.let_go(0, 1);
}

/// "Above every epoch it has issued or heard": also one that only the list has.
#[test]
fn the_epoch_to_absorb_with_is_above_what_the_list_has_heard_of() {
    let mut cluster = three_workers();
    let mut heard = three_stripes();
    heard.regions[2].epoch = 50_000;
    cluster.listed(&heard);
    assert!(cluster.merge_to_absorb(0, 1) > 50_000);
}

// ---------------------------------------------------------------------------------
// Q3. While a merge lasts.
// ---------------------------------------------------------------------------------

#[test]
fn a_move_of_either_region_of_a_merge_is_refused() {
    let mut cluster = three_workers();
    cluster.merge(0, 1);
    let before = cluster.clone();
    assert_eq!(
        cluster
            .coordinator
            .move_region(cluster.now, region(0), None, 1),
        Err(MoveRefusal::Reserved(region(0)))
    );
    assert_eq!(
        cluster
            .coordinator
            .move_region(cluster.now, region(0), Some("c"), 1),
        Err(MoveRefusal::Reserved(region(0)))
    );
    // The other is reserved and being released at once; the record does not say which
    // of the two a mover is told.
    assert!(
        cluster
            .coordinator
            .move_region(cluster.now, region(1), Some("c"), 1)
            .is_err()
    );
    assert_nothing_changed(&before, &cluster);

    // And when it has been released, and is nobody's.
    cluster.let_go(0, 1);
    let before = cluster.clone();
    assert_eq!(
        cluster
            .coordinator
            .move_region(cluster.now, region(0), Some("c"), 1),
        Err(MoveRefusal::Reserved(region(0)))
    );
    assert!(
        cluster
            .coordinator
            .move_region(cluster.now, region(1), Some("c"), 1)
            .is_err()
    );
    assert_nothing_changed(&before, &cluster);

    // The region that is not part of it can be moved all the while.
    cluster
        .coordinator
        .move_region(cluster.now, region(2), Some("b"), 1)
        .expect("the third region is free to move");
}

#[test]
fn ticks_do_not_assign_the_region_that_waits_to_be_absorbed() {
    let mut cluster = three_workers();
    cluster.merge_to_absorb(0, 1);
    // The worker that released it has nothing to run, and is still not given it.
    let changes = cluster.run(LEASE - MOMENT);
    assert_eq!(changes.workers, [] as [String; 0]);
    assert_eq!(changes.reshaped, []);
    assert_eq!(cluster.waiting(), [1]);
    assert_eq!(cluster.runs("b"), [] as [u32; 0]);
}

/// One worker has four regions and another none. Left alone, the coordinator begins
/// to hand the highest over at the next tick; during a merge it does not, nor within
/// a lease of its end.
#[test]
fn nothing_is_evened_out_while_a_merge_lasts_nor_within_a_lease_of_its_end() {
    let mut cluster = Cluster::settled(&[0, 4, 8], &["a"]);
    cluster.listed(&list(&[0, 1, 2, 3], &[], 4));
    cluster.register("b", &[]);
    let evening_out = |changes: &Changes| {
        changes
            .releases
            .iter()
            .map(|release| (release.worker.clone(), release.region.0))
            .collect::<Vec<_>>()
    };
    // What happens without a merge.
    let mut left_alone = cluster.clone();
    assert_eq!(evening_out(&left_alone.step(MOMENT)), [("a".to_owned(), 3)]);

    cluster.merge(0, 1);
    assert_eq!(cluster.run(LEASE / 4).releases, []);
    cluster.let_go(0, 1);
    assert_eq!(cluster.run(LEASE / 4).releases, []);
    cluster.absorb_ended(0, 1, Ok(()));
    assert_eq!(cluster.run(LEASE / 4).releases, []);
    let ended = cluster.listed(&list(&[0, 2, 3], &[(1, 0)], 4));
    assert_eq!(ended.reshaped, [merge_ended(0, 1, Ok(0))]);
    assert_eq!(ended.releases, []);

    // The worker with three regions is two ahead of the one with none.
    assert_eq!(cluster.runs("a"), [0, 2, 3]);
    assert_eq!(cluster.run(LEASE - MOMENT).releases, []);
    let after = cluster.run(MOMENT * 2);
    assert_eq!(evening_out(&after), [("a".to_owned(), 3)]);
}

/// The survivor's owner may well stop vouching for it: it stops ticking to absorb.
#[test]
fn the_regions_of_a_merge_are_not_taken_for_want_of_vouching() {
    let mut cluster = three_workers();
    let (survivors, absorbeds) = (cluster.epoch(0), cluster.epoch(1));
    // Both were last vouched for when they were assigned, which is now.
    cluster.stop_vouching("a");
    cluster.stop_vouching("b");
    cluster.run(LEASE - Duration::from_secs(1));

    // Left alone, each loses its owner a second from now.
    let mut left_alone = cluster.clone();
    left_alone.run(Duration::from_secs(1) + MOMENT);
    assert!(left_alone.lost(0, survivors));
    assert!(left_alone.lost(1, absorbeds));

    cluster.merge(0, 1);
    let changes = cluster.run(LEASE - MOMENT);
    assert_eq!(changes.workers, [] as [String; 0]);
    assert_eq!(changes.reshaped, []);
    assert_eq!(cluster.owner(0).as_deref(), Some("a"));
    assert_eq!(cluster.epoch(0), survivors);
    assert_eq!(cluster.owner(1).as_deref(), Some("b"));
    assert_eq!(cluster.epoch(1), absorbeds);
}

/// "When the reservation ends, the survivor, the split region and the part count as
/// vouched for at that moment."
#[test]
fn the_survivor_counts_as_vouched_for_when_its_merge_ends() {
    let mut cluster = three_workers();
    let survivors = cluster.epoch(0);
    cluster.stop_vouching("a");
    cluster.run(LEASE - Duration::from_secs(1));
    cluster.merge_to_absorb(0, 1);
    cluster.run(LEASE - Duration::from_secs(1));
    cluster.absorb_ended(0, 1, Ok(()));
    let ended = cluster.listed(&list(&[0, 2], &[(1, 0)], 3));
    assert_eq!(ended.reshaped, [merge_ended(0, 1, Ok(0))]);

    // It was last vouched for nearly two leases ago, and has a whole lease from now.
    cluster.run(LEASE);
    assert_eq!(cluster.owner(0).as_deref(), Some("a"));
    assert_eq!(cluster.epoch(0), survivors);
    // After that it is a region like any other, and its owner still vouches for
    // nothing.
    cluster.run(MOMENT);
    assert!(cluster.lost(0, survivors));
}

// ---------------------------------------------------------------------------------
// Q4. A merge that the worker calls off.
// ---------------------------------------------------------------------------------

#[test]
fn a_merge_that_is_off_has_the_region_assigned_at_once_with_a_higher_epoch() {
    for why in [
        Off::Unreadable,
        Off::Refused,
        Off::NotRunning,
        Off::Busy,
        Off::TooLarge,
        Off::StoreLost,
        Off::Declined(Decline::NotOpened { epoch: None }),
    ] {
        let mut cluster = three_workers();
        let as_epoch = cluster.merge_to_absorb(0, 1);
        let before = cluster.table();
        let said = cluster.absorb_ended(0, 1, Err(why));
        assert!(said.read);
        assert_eq!(said.reshaped, []);
        assert_eq!(cluster.waiting(), [1]);

        // No tick is needed: it is free as a region that its owner let go of is. The
        // worker with nothing to run is the one with the fewest regions.
        let changes = cluster.listed(&three_stripes());
        assert_eq!(changes.reshaped, [merge_ended(0, 1, Err(Undone::Off(why)))]);
        assert_eq!(cluster.owner(1).as_deref(), Some("b"));
        assert!(cluster.epoch(1) > as_epoch);
        assert_eq!(changes.workers, ["b"]);
        assert!(changes.routing);
        let table = cluster.table();
        assert!(table.version > before.version);
        assert!(table.is_complete());
        assert_eq!(table.absorbed, []);
        assert_eq!(table.route(region(0)), before.route(region(0)));

        // Both are free again.
        cluster.merge(0, 1);
    }
}

/// The list decides: a worker's word only tells the coordinator to look.
#[test]
fn a_merge_that_the_worker_calls_off_and_the_list_shows_done_is_done() {
    let mut cluster = three_workers();
    cluster.merge_to_absorb(0, 1);
    cluster.absorb_ended(0, 1, Err(Off::StoreLost));
    let changes = cluster.listed(&list(&[0, 2], &[(1, 0)], 3));
    assert_eq!(changes.reshaped, [merge_ended(0, 1, Ok(0))]);
    assert_eq!(cluster.known(), [0, 2]);
}

#[test]
fn a_merge_that_the_worker_calls_done_and_the_list_does_not_show_is_off() {
    let mut cluster = three_workers();
    let as_epoch = cluster.merge_to_absorb(0, 1);
    cluster.absorb_ended(0, 1, Ok(()));
    let changes = cluster.listed(&three_stripes());
    assert_eq!(
        changes.reshaped,
        [merge_ended(0, 1, Err(Undone::Contradicted))]
    );
    assert!(cluster.owner(1).is_some());
    assert!(cluster.epoch(1) > as_epoch);
}

#[test]
fn the_word_of_any_worker_that_an_absorb_has_ended_has_the_list_read() {
    let mut cluster = three_workers();
    cluster.merge_to_absorb(0, 1);
    let changes = cluster
        .coordinator
        .absorb_ended(cluster.now, "c", region(0), region(1), Ok(()));
    assert!(changes.read);
    let changes = cluster.listed(&list(&[0, 2], &[(1, 0)], 3));
    assert_eq!(changes.reshaped, [merge_ended(0, 1, Ok(0))]);
}

/// "If the list cannot be read, it is read again at every tick until the merge's time
/// is up."
#[test]
fn a_list_that_cannot_be_read_after_the_workers_word_is_read_again_at_every_tick() {
    let mut cluster = three_workers();
    cluster.merge_to_absorb(0, 1);
    cluster.run(LEASE / 5);
    assert!(cluster.absorb_ended(0, 1, Ok(())).read);
    for _ in 0..4 {
        let failed = cluster.coordinator.unlisted(cluster.now);
        assert_eq!(failed.reshaped, []);
        assert_eq!(cluster.waiting(), [1]);
        let tick = cluster.step(Cluster::STEP);
        assert!(tick.read, "the list is not asked for again: {tick:?}");
        assert_eq!(tick.reshaped, []);
    }
    let changes = cluster.listed(&list(&[0, 2], &[(1, 0)], 3));
    assert_eq!(changes.reshaped, [merge_ended(0, 1, Ok(0))]);
}

// ---------------------------------------------------------------------------------
// Q5. The release unanswered, and the survivor losing its owner before it is.
// ---------------------------------------------------------------------------------

/// Whether the worker, which runs nothing, is passed over when regions are given out,
/// as one is that failed a region in the last six leases (ADR-0009, section 7). The
/// probe is a list that shows as many new regions as there are workers with nothing
/// to run. Each goes to the worker with the fewest, so every one of those workers is
/// given one, unless every other worker comes before it whatever their loads.
fn is_passed_over(cluster: &Cluster, name: &str, living: &[u32], absorbed: &[(u32, u32)]) -> bool {
    let mut cluster = cluster.clone();
    assert_eq!(cluster.runs(name), [] as [u32; 0]);
    let idle = cluster
        .heard
        .iter()
        .filter(|heard| cluster.runs(heard).is_empty())
        .count() as u32;
    let first = living.iter().max().expect("a region lives") + 1;
    let mut shown = living.to_vec();
    shown.extend(first..first + idle);
    cluster.listed_and_looked(&list(&shown, absorbed, first + idle));
    assert!(cluster.table().is_complete());
    cluster.runs(name).is_empty()
}

/// The worker has not failed a region: it was given one when there were regions to
/// give, or it is given one now.
fn assert_not_blamed(cluster: &Cluster, name: &str, living: &[u32], absorbed: &[(u32, u32)]) {
    assert!(
        !cluster.runs(name).is_empty() || !is_passed_over(cluster, name, living, absorbed),
        "{name} is passed over when regions are given out"
    );
}

#[test]
fn a_release_for_a_merge_that_is_not_answered_within_the_lease_ends_as_an_overdue_one() {
    let mut cluster = three_workers();
    let absorbeds = cluster.epoch(1);
    cluster.merge(0, 1);

    // One tick before the time is up, and at it: nothing.
    let changes = cluster.run(LEASE - MOMENT);
    assert_eq!(changes.reshaped, []);
    assert_eq!(changes.workers, [] as [String; 0]);
    let changes = cluster.step(MOMENT);
    assert_eq!(changes.reshaped, []);
    assert_eq!(cluster.epoch(1), absorbeds);

    // After it: the region is taken from its owner and given to another.
    let changes = cluster.step(MOMENT);
    assert_eq!(
        changes.reshaped,
        [merge_ended(0, 1, Err(Undone::NotReleased))]
    );
    let owner = cluster.owner(1).expect("it is assigned");
    assert_ne!(owner, "b");
    assert!(cluster.epoch(1) > absorbeds);
    assert!(changes.workers.contains(&"b".to_owned()));
    assert_eq!(cluster.runs("b"), [] as [u32; 0]);
    assert!(cluster.table().is_complete());

    // The owner has failed the region: it has nothing to run, and the next region
    // still goes to another.
    assert!(is_passed_over(&cluster, "b", &[0, 1, 2], &[]));
    // That lasts for six leases, in which nothing is evened out towards it either,
    // and is forgotten then.
    assert_eq!(cluster.run(LEASE * 6 - MOMENT).releases, []);
    assert!(is_passed_over(&cluster, "b", &[0, 1, 2], &[]));
    cluster.now += MOMENT * 2;
    cluster.beat();
    assert!(!is_passed_over(&cluster, "b", &[0, 1, 2], &[]));

    // Both regions are free again.
    cluster.merge(0, 1);
}

/// Section 5.3, stage 1: the owner of the region to absorb did nothing wrong when it
/// is the survivor's side that ends the reservation.
#[test]
fn the_survivors_worker_falling_silent_before_the_release_does_not_blame_the_other() {
    let mut cluster = three_workers();
    let absorbeds = cluster.epoch(1);
    // The survivor's worker is last heard two seconds before the merge is asked for.
    cluster.silence("a");
    cluster.run(Duration::from_secs(2));
    cluster.merge(0, 1);
    let changes = cluster.run(LEASE - Duration::from_secs(2) - MOMENT);
    assert_eq!(changes.reshaped, []);
    assert_eq!(cluster.owner(0).as_deref(), Some("a"));

    // Its lease is out three seconds into the merge.
    let changes = cluster.run(MOMENT * 3);
    assert_eq!(
        changes.reshaped,
        [merge_ended(0, 1, Err(Undone::Disowned(region(0))))]
    );
    // The other region is taken from its owner, which was asked to release it and may
    // have, and assigned.
    cluster.step(MOMENT);
    assert_ne!(cluster.owner(1).as_deref(), Some("b"));
    assert!(cluster.epoch(1) > absorbeds);
    assert!(cluster.owner(0).is_some());
    assert!(cluster.table().is_complete());

    // Two regions were to be given to two workers, and that owner was not passed
    // over for the other one.
    assert_not_blamed(&cluster, "b", &[0, 1, 2], &[]);
}

#[test]
fn the_survivors_worker_being_refused_by_the_store_before_the_release_does_not_blame_the_other() {
    let mut cluster = Cluster::settled(&[0, 4], &["a", "b", "c", "d"]);
    cluster.listed(&three_stripes());
    let (survivors, absorbeds) = (cluster.epoch(0), cluster.epoch(1));
    cluster.merge(0, 1);
    cluster.run(LEASE / 5);

    let mut total = cluster
        .coordinator
        .epoch_refused(cluster.now, "a", region(0), survivors + 500);
    add(&mut total, cluster.tick());
    assert_eq!(
        total.reshaped,
        [merge_ended(0, 1, Err(Undone::Disowned(region(0))))]
    );
    assert_ne!(cluster.owner(1).as_deref(), Some("b"));
    assert!(cluster.epoch(1) > absorbeds);
    assert!(cluster.epoch(0) > survivors + 500);
    assert!(cluster.table().is_complete());

    assert_not_blamed(&cluster, "b", &[0, 1, 2], &[]);
}

#[test]
fn the_survivors_worker_letting_go_of_it_before_the_release_does_not_blame_the_other() {
    let mut cluster = Cluster::settled(&[0, 4], &["a", "b", "c", "d"]);
    cluster.listed(&three_stripes());
    let survivors = cluster.epoch(0);
    cluster.merge(0, 1);
    cluster.run(LEASE / 5);
    cluster
        .coordinator
        .released(cluster.now, "a", region(0), survivors);
    cluster.tick();
    assert_ne!(cluster.owner(1).as_deref(), Some("b"));
    assert!(cluster.table().is_complete());
    assert_not_blamed(&cluster, "b", &[0, 1, 2], &[]);
}

/// A worker that was told to stop and whose connection ends is gone at once, and the
/// survivor it ran is without an owner.
#[test]
fn the_survivors_worker_going_for_good_before_the_release_does_not_blame_the_other() {
    let mut cluster = Cluster::settled(&[0, 4], &["a", "b", "c", "d"]);
    cluster.listed(&three_stripes());
    cluster.merge(0, 1);
    cluster.run(LEASE / 5);
    cluster.coordinator.leaving(cluster.now, "a");
    cluster.silence("a");
    let mut changes = cluster.coordinator.disconnected(cluster.now, "a");
    add(&mut changes, cluster.tick());
    assert_eq!(
        changes.reshaped,
        [merge_ended(0, 1, Err(Undone::Disowned(region(0))))]
    );
    assert_ne!(cluster.owner(1).as_deref(), Some("b"));
    assert!(cluster.table().is_complete());
    assert_not_blamed(&cluster, "b", &[0, 1, 2], &[]);
}

/// Nor is the owner blamed when a reading takes the very region away that it was
/// asked to release: the list no longer has it.
#[test]
fn a_reading_that_takes_the_region_to_absorb_away_does_not_blame_its_owner() {
    let mut cluster = three_workers();
    cluster.merge(0, 1);
    cluster.run(LEASE / 5);
    let after = [(1, 2)];
    cluster.listed_and_looked(&list(&[0, 2], &after, 3));
    assert_eq!(cluster.runs("b"), [] as [u32; 0]);
    assert!(!is_passed_over(&cluster, "b", &[0, 2], &after));
}

#[test]
fn a_reading_that_takes_the_survivor_away_before_the_release_does_not_blame_the_other() {
    let mut cluster = three_workers();
    let absorbeds = cluster.epoch(2);
    // Region 1 is to absorb region 2, and the list then has region 1 absorbed by the
    // home region.
    cluster.merge(1, 2);
    cluster.run(LEASE / 5);
    let after = [(1, 0)];
    let changes = cluster.listed_and_looked(&list(&[0, 2], &after, 3));
    let asked = Asked::Merge {
        survivor: region(1),
        absorbed: region(2),
    };
    assert!(lost_or_gone(&changes.reshaped, asked, 1), "{changes:?}");
    assert_eq!(cluster.known(), [0, 2]);
    assert_ne!(cluster.owner(2).as_deref(), Some("c"));
    assert!(cluster.epoch(2) > absorbeds);

    // Two regions on three workers, and the one that had the region to absorb is not
    // passed over for the third.
    assert_not_blamed(&cluster, "c", &[0, 2], &after);
}

/// "Or the absorbed region loses its owner otherwise than by step 2."
#[test]
fn the_other_regions_worker_falling_silent_before_the_release_ends_the_merge() {
    let mut cluster = three_workers();
    let absorbeds = cluster.epoch(1);
    cluster.silence("b");
    cluster.run(Duration::from_secs(2));
    cluster.merge(0, 1);
    let changes = cluster.run(LEASE - Duration::from_secs(2) + MOMENT * 2);
    assert_eq!(
        changes.reshaped,
        [merge_ended(0, 1, Err(Undone::Disowned(region(1))))]
    );
    assert_eq!(changes.orders, [], "nobody is told to absorb");
    assert!(cluster.owner(1).is_some());
    assert!(cluster.epoch(1) > absorbeds);
    assert_eq!(cluster.owner(0).as_deref(), Some("a"));
    assert!(cluster.table().is_complete());
    // The survivor is free again.
    cluster.merge(0, 2);
}

// ---------------------------------------------------------------------------------
// Q6. The absorb unanswered for a lease.
// ---------------------------------------------------------------------------------

/// A merge whose survivor's owner was told to absorb and said nothing, a moment after
/// its time is up: the tick has asked for the list and decided nothing.
fn an_absorb_unanswered_for_a_lease() -> (Cluster, u64) {
    let mut cluster = three_workers();
    cluster.merge(0, 1);
    cluster.run(LEASE / 5);
    let as_epoch = cluster.let_go(0, 1);

    // One tick before the time is up, and at it: nothing, and no reading asked for.
    let changes = cluster.run(LEASE - LEASE / 5 - MOMENT);
    assert_eq!(changes, Changes::default());
    let changes = cluster.step(MOMENT);
    assert_eq!(changes, Changes::default());

    // After it the list is read first, and the region is nobody's until then.
    let changes = cluster.step(MOMENT);
    assert!(changes.read, "{changes:?}");
    assert_eq!(changes.reshaped, []);
    assert_eq!(changes.workers, [] as [String; 0]);
    assert_eq!(cluster.waiting(), [1]);
    (cluster, as_epoch)
}

#[test]
fn an_absorb_unanswered_for_a_lease_is_done_if_the_list_has_the_pair() {
    let (mut cluster, _) = an_absorb_unanswered_for_a_lease();
    let changes = cluster.listed(&list(&[0, 2], &[(1, 0)], 3));
    assert_eq!(changes.reshaped, [merge_ended(0, 1, Ok(0))]);
    assert_eq!(cluster.known(), [0, 2]);
    assert_eq!(cluster.table().absorbed, pairs(&[(1, 0)]));
    assert_eq!(cluster.run(LEASE * 2).reshaped, []);
}

#[test]
fn an_absorb_unanswered_for_a_lease_is_off_if_the_list_lacks_the_pair() {
    let (mut cluster, as_epoch) = an_absorb_unanswered_for_a_lease();
    let changes = cluster.listed(&three_stripes());
    assert_eq!(changes.reshaped, [merge_ended(0, 1, Err(Undone::Overdue))]);
    // An epoch above the one the survivor's worker was told fences it if it is still
    // at it. The worker that released the region has nothing to run and did as it was
    // asked: it is the one with the fewest, and not passed over.
    assert_eq!(cluster.owner(1).as_deref(), Some("b"));
    assert!(cluster.epoch(1) > as_epoch);
    assert!(cluster.table().is_complete());
    assert_eq!(cluster.run(LEASE * 2).reshaped, []);
    cluster.merge(0, 1);
}

#[test]
fn an_absorb_unanswered_for_a_lease_has_the_region_assigned_if_the_list_cannot_be_read() {
    let (mut cluster, as_epoch) = an_absorb_unanswered_for_a_lease();
    let changes = cluster.coordinator.unlisted(cluster.now);
    assert_eq!(changes.reshaped, [merge_ended(0, 1, Err(Undone::Unread))]);
    let owner = cluster.owner(1).expect("it is assigned all the same");
    assert_eq!(owner, "b", "the worker with nothing to run has the fewest");
    assert!(cluster.epoch(1) > as_epoch);
    assert!(cluster.table().is_complete());

    // It had been absorbed: the worker that is given it is refused by the store and
    // says so, which reads the list, and that takes the region away.
    let said = cluster
        .coordinator
        .absorb_ended(cluster.now, &owner, region(0), region(1), Ok(()));
    assert!(said.read);
    assert_eq!(said.reshaped, []);
    let changes = cluster.listed(&list(&[0, 2], &[(1, 0)], 3));
    assert_eq!(changes.reshaped, [], "the asker has been told");
    assert!(changes.workers.contains(&owner));
    assert!(!cluster.runs(&owner).contains(&1));
    assert_eq!(cluster.known(), [0, 2]);
    assert_eq!(cluster.table().absorbed, pairs(&[(1, 0)]));
}

/// The worker's word after the time is up changes nothing about what the list says.
#[test]
fn an_absorb_that_is_answered_after_its_time_is_up_is_judged_by_the_list_all_the_same() {
    let (mut cluster, _) = an_absorb_unanswered_for_a_lease();
    let said = cluster.absorb_ended(0, 1, Err(Off::StoreLost));
    assert_eq!(said.reshaped, []);
    let changes = cluster.listed(&list(&[0, 2], &[(1, 0)], 3));
    assert_eq!(changes.reshaped, [merge_ended(0, 1, Ok(0))]);
    assert_eq!(cluster.known(), [0, 2]);
}

// ---------------------------------------------------------------------------------
// Q7. The survivor's owner registers again.
// ---------------------------------------------------------------------------------

#[test]
fn the_survivors_owner_that_registers_again_is_told_to_absorb_again_with_the_same_epoch() {
    let mut cluster = three_workers();
    let survivors = cluster.epoch(0);
    let as_epoch = cluster.merge_to_absorb(0, 1);
    let absorb = ReshapeOrder {
        worker: "a".to_owned(),
        order: Order::Absorb {
            region: region(0),
            epoch: survivors,
            absorbed: region(1),
            as_epoch,
        },
    };
    for _ in 0..3 {
        cluster.run(LEASE / 5);
        let changes = cluster.register_again("a");
        assert_eq!(changes.orders, std::slice::from_ref(&absorb));
        assert_eq!(changes.reshaped, []);
        assert_eq!(cluster.owner(0).as_deref(), Some("a"));
        assert_eq!(cluster.waiting(), [1]);
    }
    // Another worker's registration tells nobody anything.
    assert_eq!(cluster.register_again("c").orders, []);
    assert_eq!(cluster.register("b", &[]).orders, []);
}

#[test]
fn the_survivors_owner_that_registers_again_before_the_release_is_told_nothing_of_the_merge() {
    let mut cluster = three_workers();
    cluster.merge(0, 1);
    let changes = cluster.register_again("a");
    assert_eq!(changes.orders, []);
    assert_eq!(changes.reshaped, []);
    // The merge goes on.
    cluster.let_go(0, 1);
}

// ---------------------------------------------------------------------------------
// Q8. The survivor's worker falls silent when it has been told to absorb.
// ---------------------------------------------------------------------------------

/// A merge whose survivor's worker was last heard two seconds before it was asked for
/// and was told to absorb, a moment after that worker's lease is out and well before
/// the merge's time is. The tick has asked for the list.
fn the_survivors_worker_silent_while_it_absorbs() -> (Cluster, u64) {
    let mut cluster = three_workers();
    cluster.silence("a");
    cluster.run(Duration::from_secs(2));
    let as_epoch = cluster.merge_to_absorb(0, 1);
    let changes = cluster.run(LEASE - Duration::from_secs(2) - MOMENT);
    assert_eq!(changes, Changes::default());

    let changes = cluster.run(MOMENT * 3);
    assert!(changes.read, "{changes:?}");
    assert_eq!(changes.reshaped, []);
    // The region to absorb is given to nobody before the list has been read.
    assert!(cluster.waiting().contains(&1));
    assert_ne!(cluster.owner(0).as_deref(), Some("a"));
    (cluster, as_epoch)
}

#[test]
fn the_survivors_worker_falling_silent_leaves_both_regions_to_others_if_the_list_lacks_the_pair() {
    let (mut cluster, as_epoch) = the_survivors_worker_silent_while_it_absorbs();
    let changes = cluster.listed_and_looked(&three_stripes());
    assert_eq!(
        changes.reshaped,
        [merge_ended(0, 1, Err(Undone::Disowned(region(0))))]
    );
    assert_eq!(cluster.known(), [0, 1, 2]);
    assert!(cluster.table().is_complete());
    assert!(cluster.epoch(1) > as_epoch);
    assert_eq!(cluster.runs("a"), [] as [u32; 0]);
}

#[test]
fn the_survivors_worker_falling_silent_leaves_one_region_if_the_list_has_the_pair() {
    let (mut cluster, _) = the_survivors_worker_silent_while_it_absorbs();
    let changes = cluster.listed_and_looked(&list(&[0, 2], &[(1, 0)], 3));
    assert_eq!(changes.reshaped, [merge_ended(0, 1, Ok(0))]);
    assert_eq!(cluster.known(), [0, 2]);
    assert!(cluster.table().is_complete());
    assert_eq!(cluster.table().absorbed, pairs(&[(1, 0)]));
}

#[test]
fn the_survivors_worker_falling_silent_leaves_both_regions_to_others_if_the_list_cannot_be_read() {
    let (mut cluster, as_epoch) = the_survivors_worker_silent_while_it_absorbs();
    let mut changes = cluster.coordinator.unlisted(cluster.now);
    add(&mut changes, cluster.tick());
    assert_eq!(changes.reshaped, [merge_ended(0, 1, Err(Undone::Unread))]);
    assert_eq!(cluster.known(), [0, 1, 2]);
    assert!(cluster.table().is_complete());
    assert!(cluster.epoch(1) > as_epoch);
}

// ---------------------------------------------------------------------------------
// Q9. A split in order.
// ---------------------------------------------------------------------------------

#[test]
fn a_split_orders_the_next_id_of_the_list_and_an_epoch_above_every_other() {
    let mut cluster = three_workers();
    let before = cluster.clone();
    let highest = cluster.highest_epoch();
    let owners = cluster.epoch(1);

    let changes = cluster
        .coordinator
        .split(cluster.now, region(1), &chunks(), ASKER)
        .expect("the split is taken on");
    let [
        ReshapeOrder {
            worker,
            order:
                Order::SplitOff {
                    region: of,
                    epoch,
                    chunks: named,
                    as_epoch,
                    part,
                },
        },
    ] = changes.orders.as_slice()
    else {
        panic!("expected the order to split: {changes:?}");
    };
    assert_eq!(worker, "b");
    assert_eq!((*of, *epoch, *part), (region(1), owners, region(3)));
    assert_eq!(*named, chunks());
    assert!(*as_epoch > highest);
    assert_eq!(changes.releases, []);
    assert_eq!(changes.reshaped, []);
    assert_eq!(changes.workers, [] as [String; 0]);
    assert!(!changes.routing);
    assert_eq!(cluster.table(), before.table());

    // The id is that of the reading before it, whatever regions the layout has.
    let mut later = before.clone();
    later.listed(&list(&[0, 1, 2, 5, 8], &[(3, 0), (4, 8)], 9));
    assert_eq!(later.split(1).1, 9);
}

#[test]
fn the_new_region_of_a_split_is_its_workers_with_the_epoch_that_was_ordered() {
    let mut cluster = three_workers();
    let before = cluster.table();
    let (as_epoch, part) = cluster.split(1);

    let changes = cluster.split_ended(1, as_epoch, Ok(part));
    assert_eq!(changes.reshaped, [split_ended(1, Ok(part))]);
    assert!(changes.read);
    assert_eq!(changes.workers, ["b"]);
    assert!(changes.routing);
    assert_eq!(cluster.runs("b"), [1, part]);
    let table = cluster.table();
    assert!(table.version > before.version);
    let route = table.route(region(part)).expect("the part has a route");
    assert_eq!((route.epoch, route.address.as_str()), (as_epoch, "b:25600"));
    assert_eq!(table.route(region(1)), before.route(region(1)));
    assert!(table.is_complete());
    let told = cluster.coordinator.assignments("b");
    assert_eq!(told[1].region, region(part));
    assert_eq!(told[1].epoch, as_epoch);

    // The reading that follows, with the part in it, changes nothing.
    let reading = cluster.listed(&list(&[0, 1, 2, 3], &[], 4));
    assert_eq!(reading.reshaped, []);
    assert_eq!(reading.workers, [] as [String; 0]);
    assert_eq!(cluster.table().route(region(part)), Some(route));
    // The region that was split is free again, and so is the part.
    cluster.split(1);
    cluster.split(part);
}

#[test]
fn the_heartbeats_of_its_worker_keep_the_new_region_of_a_split() {
    let mut cluster = three_workers();
    let (as_epoch, part) = cluster.split(1);
    cluster.split_ended(1, as_epoch, Ok(part));
    cluster.listed(&list(&[0, 1, 2, 3], &[], 4));

    let changes = cluster.run(LEASE * 4);
    assert_eq!(changes.workers, [] as [String; 0]);
    assert_eq!(cluster.owner(part).as_deref(), Some("b"));
    assert_eq!(cluster.epoch(part), as_epoch);
}

/// "Vouched for as a new assignment is": for a lease from the worker's word, and no
/// longer without a heartbeat that names it.
#[test]
fn the_new_region_of_a_split_is_vouched_for_like_a_new_assignment() {
    let mut cluster = three_workers();
    cluster.stop_vouching("b");
    cluster.run(LEASE - Duration::from_secs(1));
    let (owners, (as_epoch, part)) = (cluster.epoch(1), cluster.split(1));
    cluster.run(LEASE - Duration::from_secs(1));
    cluster.split_ended(1, as_epoch, Ok(part));
    cluster.listed(&list(&[0, 1, 2, 3], &[], 4));

    // Both the region that was split and the part have a lease from here.
    cluster.run(LEASE);
    assert_eq!(cluster.epoch(1), owners);
    assert_eq!(cluster.owner(part).as_deref(), Some("b"));
    assert_eq!(cluster.epoch(part), as_epoch);
    cluster.run(MOMENT);
    assert!(cluster.lost(1, owners));
    assert!(cluster.lost(part, as_epoch));
}

#[test]
fn the_region_that_is_being_split_is_not_taken_for_want_of_vouching() {
    let mut cluster = three_workers();
    let owners = cluster.epoch(1);
    cluster.stop_vouching("b");
    cluster.run(LEASE - Duration::from_secs(1));
    let mut left_alone = cluster.clone();
    left_alone.run(Duration::from_secs(1) + MOMENT);
    assert!(left_alone.lost(1, owners));

    cluster.split(1);
    let changes = cluster.run(LEASE - MOMENT);
    assert_eq!(changes.workers, [] as [String; 0]);
    assert_eq!(cluster.epoch(1), owners);
}

#[test]
fn a_move_of_a_region_that_is_being_split_is_refused() {
    let mut cluster = three_workers();
    cluster.split(1);
    let before = cluster.clone();
    assert_eq!(
        cluster
            .coordinator
            .move_region(cluster.now, region(1), Some("c"), 1),
        Err(MoveRefusal::Reserved(region(1)))
    );
    assert_nothing_changed(&before, &cluster);
}

/// "The order is not sent again if it is lost: a split that is done twice makes two
/// regions."
#[test]
fn the_order_to_split_is_not_given_again_to_an_owner_that_registers_again() {
    let mut cluster = three_workers();
    let (as_epoch, part) = cluster.split(1);
    let changes = cluster.register_again("b");
    assert_eq!(changes.orders, []);
    assert_eq!(changes.reshaped, []);
    // The split is still reserved, and the worker's word still ends it.
    assert_split_refused(&mut cluster, 1, ReshapeRefusal::Reserved(region(1)));
    let changes = cluster.split_ended(1, as_epoch, Ok(part));
    assert_eq!(changes.reshaped, [split_ended(1, Ok(part))]);
}

#[test]
fn the_word_of_another_worker_does_not_end_a_split() {
    let mut cluster = three_workers();
    let (as_epoch, part) = cluster.split(1);
    let changes =
        cluster
            .coordinator
            .split_ended(cluster.now, "c", region(1), as_epoch, Err(Off::Nobody));
    assert_eq!(changes.reshaped, []);
    assert_split_refused(&mut cluster, 1, ReshapeRefusal::Reserved(region(1)));
    let changes = cluster.split_ended(1, as_epoch, Ok(part));
    assert_eq!(changes.reshaped, [split_ended(1, Ok(part))]);
}

// ---------------------------------------------------------------------------------
// Q10. A split that is off, and one of which nothing is heard.
// ---------------------------------------------------------------------------------

#[test]
fn a_split_that_is_off_ends_with_the_workers_reason_and_has_the_list_read() {
    for why in [
        Off::Nobody,
        Off::NothingStays,
        Off::NotRunning,
        Off::Busy,
        Off::TooLarge,
        Off::StoreLost,
        Off::Declined(Decline::Malformed),
    ] {
        let mut cluster = three_workers();
        let before = cluster.table();
        let (as_epoch, _) = cluster.split(1);
        let changes = cluster.split_ended(1, as_epoch, Err(why));
        assert_eq!(changes.reshaped, [split_ended(1, Err(Undone::Off(why)))]);
        assert!(
            changes.read,
            "the store may have been lost after the record"
        );
        assert_eq!(changes.workers, [] as [String; 0]);
        assert_eq!(cluster.table(), before);
        // The reading shows nothing new, and the region is free again.
        assert_eq!(cluster.listed(&three_stripes()), Changes::default());
        cluster.split(1);
    }
}

/// `Off::StoreLost` leaves open what happened: the reading that follows may show the
/// part, which nobody runs then.
#[test]
fn a_split_that_is_off_and_was_done_all_the_same_leaves_a_region_for_somebody() {
    let mut cluster = three_workers();
    let (as_epoch, part) = cluster.split(1);
    let changes = cluster.split_ended(1, as_epoch, Err(Off::StoreLost));
    assert_eq!(
        changes.reshaped,
        [split_ended(1, Err(Undone::Off(Off::StoreLost)))]
    );
    let changes = cluster.listed_and_looked(&list(&[0, 1, 2, 3], &[], 4));
    assert_eq!(changes.reshaped, []);
    assert!(cluster.owner(part).is_some());
    assert!(cluster.epoch(part) > as_epoch);
    assert!(cluster.table().is_complete());
}

/// A split of which its worker said nothing, a moment after its time is up: whoever
/// asked has been told that it is overdue, and the list is asked for.
fn a_split_unanswered_for_a_lease() -> (Cluster, u64, u32) {
    let mut cluster = three_workers();
    let (as_epoch, part) = cluster.split(1);
    // One tick before the time is up, and at it: nothing.
    assert_eq!(cluster.run(LEASE - MOMENT), Changes::default());
    assert_eq!(cluster.step(MOMENT), Changes::default());
    let changes = cluster.step(MOMENT);
    assert_eq!(changes.reshaped, [split_ended(1, Err(Undone::Overdue))]);
    assert!(changes.read);
    assert_eq!(changes.workers, [] as [String; 0]);
    (cluster, as_epoch, part)
}

#[test]
fn a_split_unanswered_for_a_lease_is_overdue_and_a_new_region_of_the_list_is_assigned() {
    let (mut cluster, as_epoch, part) = a_split_unanswered_for_a_lease();
    assert_eq!(part, 3);
    // The new region has the very id that was ordered, and that says nothing.
    let changes = cluster.listed_and_looked(&list(&[0, 1, 2, 3], &[], 4));
    assert_eq!(
        changes.reshaped,
        [],
        "the asker was told that it is overdue"
    );
    assert_eq!(cluster.known(), [0, 1, 2, 3]);
    assert!(cluster.owner(3).is_some());
    assert!(cluster.epoch(3) > as_epoch);
    assert!(cluster.table().is_complete());
    assert_eq!(cluster.run(LEASE * 2).reshaped, []);
    cluster.split(1);
}

#[test]
fn a_split_unanswered_for_a_lease_is_overdue_when_the_list_has_no_new_region() {
    let (mut cluster, _, _) = a_split_unanswered_for_a_lease();
    let changes = cluster.listed_and_looked(&three_stripes());
    assert_eq!(changes.reshaped, []);
    assert_eq!(changes.workers, [] as [String; 0]);
    assert_eq!(cluster.known(), [0, 1, 2]);
    assert_eq!(cluster.run(LEASE * 2), Changes::default());
    cluster.split(1);
}

/// "If the list cannot be read": the part may be a region that nobody runs, so the
/// list is asked for until it has been read.
#[test]
fn a_split_unanswered_for_a_lease_has_the_list_asked_for_until_it_is_read() {
    let (mut cluster, _, part) = a_split_unanswered_for_a_lease();
    for _ in 0..3 {
        assert_eq!(cluster.coordinator.unlisted(cluster.now).reshaped, []);
        let tick = cluster.step(Cluster::STEP);
        assert!(tick.read, "{tick:?}");
    }
    cluster.listed_and_looked(&list(&[0, 1, 2, 3], &[], 4));
    assert!(cluster.owner(part).is_some());
    assert!(!cluster.step(Cluster::STEP).read);
}

/// Section 5.4: a `SplitEnded` with an epoch the coordinator has no reservation for is
/// taken as a registration that reports the region.
#[test]
fn the_word_of_a_split_that_comes_after_its_time_makes_the_new_region_the_workers() {
    let (mut cluster, as_epoch, part) = a_split_unanswered_for_a_lease();
    let changes = cluster.split_ended(1, as_epoch, Ok(part));
    assert_eq!(
        changes.reshaped,
        [],
        "the asker was told that it is overdue"
    );
    assert!(changes.read);
    assert_eq!(cluster.owner(part).as_deref(), Some("b"));
    assert_eq!(cluster.epoch(part), as_epoch);
    // The reading agrees, and nothing more happens.
    let changes = cluster.listed_and_looked(&list(&[0, 1, 2, 3], &[], 4));
    assert_eq!(changes.workers, [] as [String; 0]);
    assert_eq!(cluster.epoch(part), as_epoch);
    assert_eq!(cluster.run(LEASE * 2).workers, [] as [String; 0]);
}

#[test]
fn the_word_of_a_split_that_comes_after_another_worker_was_given_the_region_changes_nothing() {
    let (mut cluster, as_epoch, part) = a_split_unanswered_for_a_lease();
    // The list shows the part, and it is given to somebody with a new epoch: to
    // another worker, which is then its owner, or to the one that made it, for which
    // it is a new assignment (section 4). Either way the late word is not honoured.
    cluster.listed_and_looked(&list(&[0, 1, 2, 3], &[], 4));
    let owner = cluster.owner(part).expect("the part is assigned");
    let epoch = cluster.epoch(part);
    assert!(epoch > as_epoch);
    let said =
        cluster
            .coordinator
            .split_ended(cluster.now, "b", region(1), as_epoch, Ok(region(part)));
    assert_eq!(said.reshaped, []);
    assert_eq!(cluster.owner(part), Some(owner));
    assert_eq!(cluster.epoch(part), epoch);
}

/// "When ... the region loses its owner or changes its epoch: the reservation ends
/// and the list is read."
#[test]
fn a_split_whose_worker_falls_silent_ends_with_that_workers_lease() {
    let mut cluster = three_workers();
    cluster.silence("b");
    cluster.run(Duration::from_secs(2));
    let (as_epoch, part) = cluster.split(1);
    assert_eq!(
        cluster.run(LEASE - Duration::from_secs(2) - MOMENT),
        Changes::default()
    );
    let changes = cluster.run(MOMENT * 3);
    assert_eq!(
        changes.reshaped,
        [split_ended(1, Err(Undone::Disowned(region(1))))]
    );
    assert!(changes.read);
    assert_ne!(cluster.owner(1).as_deref(), Some("b"));

    // The record was written before the worker died: both regions are run by others.
    cluster.listed_and_looked(&list(&[0, 1, 2, 3], &[], 4));
    assert_eq!(cluster.known(), [0, 1, 2, 3]);
    assert!(cluster.table().is_complete());
    assert!(cluster.epoch(part) > as_epoch);
    assert_eq!(cluster.runs("b"), [] as [u32; 0]);
}

#[test]
fn a_split_whose_worker_is_refused_by_the_store_ends() {
    let mut cluster = three_workers();
    let owners = cluster.epoch(1);
    cluster.split(1);
    let mut changes = cluster
        .coordinator
        .epoch_refused(cluster.now, "b", region(1), owners + 100);
    add(&mut changes, cluster.tick());
    assert_eq!(
        changes.reshaped,
        [split_ended(1, Err(Undone::Disowned(region(1))))]
    );
    assert!(changes.read);
    assert!(cluster.epoch(1) > owners + 100);
}

// ---------------------------------------------------------------------------------
// Q11. What a reading of the list does.
// ---------------------------------------------------------------------------------

#[test]
fn a_living_region_of_the_list_that_is_not_known_is_added_without_an_owner_and_assigned() {
    let mut cluster = three_workers();
    cluster.register("d", &[]);
    let before = cluster.table();
    let changes = cluster.listed_and_looked(&list(&[0, 1, 2, 5], &[(3, 5), (4, 5)], 6));
    assert_eq!(cluster.known(), [0, 1, 2, 5]);
    // The worker that runs nothing has the fewest.
    assert_eq!(cluster.owner(5).as_deref(), Some("d"));
    assert!(cluster.epoch(5) > before.routes.iter().map(|route| route.epoch).max().unwrap());
    assert!(changes.routing);
    assert_eq!(changes.workers, ["d"]);
    assert!(cluster.table().version > before.version);
    assert_eq!(changes.reshaped, []);
}

#[test]
fn a_new_coordinator_adds_a_region_of_the_list_and_keeps_it_through_its_grace_period() {
    let mut cluster = Cluster::reported(&[0, 4], &[("a", &[0]), ("b", &[1]), ("c", &[2])]);
    let listed = list(&[0, 1, 2, 3], &[], 4);
    cluster.listed_and_looked(&listed);
    assert_eq!(cluster.waiting(), [3]);
    assert_eq!(cluster.table().waiting, 1);
    assert!(!cluster.table().is_complete());

    let changes = cluster.run(LEASE - MOMENT);
    assert_eq!(changes.workers, [] as [String; 0]);
    assert_eq!(cluster.waiting(), [3]);
    cluster.run(MOMENT * 2);
    assert!(cluster.owner(3).is_some());
    assert!(cluster.table().is_complete());
}

#[test]
fn a_known_region_that_the_list_has_among_the_absorbed_is_removed_from_its_owner() {
    let mut cluster = three_workers();
    let before = cluster.table();
    let changes = cluster.listed(&list(&[0, 2], &[(1, 2)], 3));
    assert_eq!(cluster.known(), [0, 2]);
    assert_eq!(cluster.runs("b"), [] as [u32; 0]);
    assert_eq!(changes.workers, ["b"]);
    assert!(changes.routing);
    assert_eq!(changes.reshaped, []);
    let table = cluster.table();
    assert!(table.version > before.version);
    assert_eq!(table.route(region(1)), None);
    assert_eq!(table.absorbed, pairs(&[(1, 2)]));
    assert_eq!(table.waiting, 0);
    assert!(table.is_complete());
    // It does not come back.
    assert_eq!(cluster.run(LEASE * 2).workers, [] as [String; 0]);
    assert_eq!(cluster.known(), [0, 2]);
}

#[test]
fn a_known_region_below_the_next_id_that_the_list_has_nowhere_is_removed() {
    let mut cluster = three_workers();
    let changes = cluster.listed(&list(&[0, 2], &[], 3));
    assert_eq!(cluster.known(), [0, 2]);
    assert_eq!(cluster.runs("b"), [] as [u32; 0]);
    assert_eq!(changes.workers, ["b"]);

    // One without an owner as well.
    let mut cluster = Cluster::reported(&[0, 4], &[("a", &[0])]);
    assert_eq!(cluster.waiting(), [1, 2]);
    cluster.listed(&list(&[0, 2], &[], 3));
    assert_eq!(cluster.waiting(), [2]);
    assert_eq!(cluster.table().waiting, 1);
}

/// "A region at or above `next` is left alone: the reading is older than the split
/// that made it."
#[test]
fn a_known_region_at_or_above_the_next_id_of_the_list_is_left_alone() {
    let mut cluster = three_workers();
    let (as_epoch, part) = cluster.split(1);
    cluster.split_ended(1, as_epoch, Ok(part));
    let before = cluster.table();

    // A reading from before the split: three regions, and 3 the next id.
    let changes = cluster.listed_and_looked(&three_stripes());
    assert_eq!(changes.workers, [] as [String; 0]);
    assert_eq!(cluster.table(), before);
    assert_eq!(cluster.owner(part).as_deref(), Some("b"));

    // One from after it that lacks the region takes it away.
    let changes = cluster.listed(&list(&[0, 1, 2], &[], 4));
    assert_eq!(changes.workers, ["b"]);
    assert_eq!(cluster.known(), [0, 1, 2]);
    assert_eq!(cluster.runs("b"), [1]);
}

#[test]
fn the_home_region_and_the_pairs_of_a_reading_are_in_the_routing_table() {
    let mut cluster = Cluster::settled(&[0, 4], &["a", "b", "c"]);
    let before = cluster.table();
    assert_eq!(before.home, None);
    assert_eq!(before.absorbed, []);

    let changes = cluster.listed(&three_stripes());
    assert!(changes.routing);
    let first = cluster.table();
    assert_eq!(first.home, Some(region(0)));
    assert!(first.version > before.version);
    assert_eq!(first.routes, before.routes);

    // The same reading again changes nothing, and so no version.
    let changes = cluster.listed(&three_stripes());
    assert!(!changes.routing);
    assert_eq!(cluster.table(), first);

    // Every pair the store keeps is passed on, chains and all.
    let chained = [(5, 4), (4, 3), (3, 0), (7, 6), (6, 0)];
    let changes = cluster.listed(&list(&[0, 1, 2], &chained, 8));
    assert!(changes.routing);
    let second = cluster.table();
    assert_eq!(second.absorbed, pairs(&chained));
    assert!(second.version > first.version);

    let mut elsewhere = list(&[0, 1, 2], &chained, 8);
    elsewhere.home = region(2);
    assert!(cluster.listed(&elsewhere).routing);
    let third = cluster.table();
    assert_eq!(third.home, Some(region(2)));
    assert!(third.version > second.version);
}

/// The pairs are kept for good: an edge that was away needs them, and a reading that
/// fails says nothing about what was absorbed.
#[test]
fn a_reading_that_fails_leaves_the_home_region_and_the_pairs_as_they_were() {
    let mut cluster = three_workers();
    cluster.merge_to_absorb(0, 1);
    cluster.absorb_ended(0, 1, Ok(()));
    cluster.listed(&list(&[0, 2], &[(1, 0)], 3));
    let before = cluster.table();
    assert_eq!(before.absorbed, pairs(&[(1, 0)]));

    for _ in 0..3 {
        let changes = cluster.coordinator.unlisted(cluster.now);
        assert!(!changes.routing);
        assert_eq!(cluster.table(), before);
        cluster.run(LEASE);
        assert_eq!(cluster.table(), before);
    }
    // A later merge adds its pair to those that were there.
    cluster.merge_to_absorb(0, 2);
    cluster.absorb_ended(0, 2, Ok(()));
    cluster.coordinator.unlisted(cluster.now);
    assert_eq!(cluster.table().absorbed, pairs(&[(1, 0)]));
    cluster.listed(&list(&[0], &[(1, 0), (2, 0)], 3));
    assert_eq!(cluster.table().absorbed, pairs(&[(1, 0), (2, 0)]));
    assert_eq!(cluster.known(), [0]);
}

// ---------------------------------------------------------------------------------
// Q12. A region that only a worker reports.
// ---------------------------------------------------------------------------------

#[test]
fn a_region_that_a_worker_reports_and_nobody_knows_is_kept_with_that_worker() {
    let mut cluster = three_workers();
    let before = cluster.table();
    let mut holding = cluster.coordinator.assignments("b");
    holding.push(held(7, 77));
    let changes = cluster.register("b", &holding);
    assert!(changes.routing);
    assert_eq!(cluster.runs("b"), [1, 7]);
    let table = cluster.table();
    assert!(table.version > before.version);
    let route = table
        .route(region(7))
        .expect("the reported region has a route");
    assert_eq!((route.epoch, route.address.as_str()), (77, "b:25600"));
    assert!(table.is_complete());
    assert_eq!(cluster.known(), [0, 1, 2, 7]);

    // Its heartbeats keep it, and a reading that is older than it leaves it.
    cluster.run(LEASE * 2);
    cluster.listed_and_looked(&list(&[0, 1, 2], &[], 7));
    assert_eq!(cluster.owner(7).as_deref(), Some("b"));
    assert_eq!(cluster.epoch(7), 77);

    // A reading with a next id above it, and without it, takes it away.
    let changes = cluster.listed(&list(&[0, 1, 2], &[], 8));
    assert_eq!(changes.workers, ["b"]);
    assert_eq!(cluster.runs("b"), [1]);
    assert_eq!(cluster.known(), [0, 1, 2]);
}

#[test]
fn a_region_that_a_worker_reports_and_the_list_has_as_absorbed_is_not_kept() {
    let mut cluster = three_workers();
    cluster.listed(&list(&[0, 1, 2], &[(7, 0)], 8));
    let mut holding = cluster.coordinator.assignments("b");
    holding.push(held(7, 77));
    cluster.register("b", &holding);
    assert_eq!(cluster.runs("b"), [1]);
    assert_eq!(cluster.known(), [0, 1, 2]);
}

#[test]
fn a_reported_region_is_the_first_reporters_and_not_given_to_a_second() {
    let mut cluster = Cluster::reported(&[0, 4], &[("a", &[0]), ("b", &[1]), ("c", &[2])]);
    cluster.register("b", &[held(1, 11), held(7, 77)]);
    cluster.register("c", &[held(2, 12), held(7, 78)]);
    assert_eq!(cluster.runs("b"), [1, 7]);
    assert_eq!(cluster.runs("c"), [2]);
    assert_eq!(cluster.epoch(7), 77);
}

// ---------------------------------------------------------------------------------
// Q13. Evening out after a split.
// ---------------------------------------------------------------------------------

#[test]
fn nothing_is_evened_out_while_a_split_lasts_nor_within_a_lease_of_its_end() {
    let mut cluster = Cluster::settled(&[0, 4], &["a"]);
    cluster.listed(&three_stripes());
    cluster.register("b", &[]);
    let mut left_alone = cluster.clone();
    assert_eq!(left_alone.step(MOMENT).releases.len(), 1);

    let (as_epoch, part) = cluster.split(0);
    assert_eq!(cluster.run(LEASE / 2).releases, []);
    let ended = cluster.split_ended(0, as_epoch, Ok(part));
    assert_eq!(ended.releases, []);
    cluster.listed(&list(&[0, 1, 2, 3], &[], 4));

    // The part is on the worker that made it, which has four regions to the other's
    // none, and is left there for a lease.
    assert_eq!(cluster.runs("a"), [0, 1, 2, 3]);
    assert_eq!(cluster.run(LEASE - MOMENT).releases, []);
    let after = cluster.run(MOMENT * 2);
    let [release] = after.releases.as_slice() else {
        panic!("expected one release to even out: {after:?}");
    };
    // The highest region of the worker with the most: the part.
    assert_eq!(
        (release.worker.as_str(), release.region, release.epoch),
        ("a", region(part), as_epoch)
    );
}

#[test]
fn nothing_is_evened_out_within_a_lease_of_a_split_that_came_to_nothing() {
    let mut cluster = Cluster::settled(&[0, 4], &["a"]);
    cluster.listed(&three_stripes());
    cluster.register("b", &[]);
    let (as_epoch, _) = cluster.split(0);
    cluster.run(LEASE / 2);
    cluster.split_ended(0, as_epoch, Err(Off::Nobody));
    cluster.listed(&three_stripes());
    assert_eq!(cluster.run(LEASE - MOMENT).releases, []);
    assert_eq!(cluster.run(MOMENT * 2).releases.len(), 1);
}

#[test]
fn nothing_is_evened_out_within_a_lease_of_a_split_that_was_overdue() {
    let mut cluster = Cluster::settled(&[0, 4], &["a"]);
    cluster.listed(&three_stripes());
    cluster.register("b", &[]);
    cluster.split(0);
    let changes = cluster.run(LEASE + MOMENT);
    assert_eq!(changes.reshaped, [split_ended(0, Err(Undone::Overdue))]);
    assert_eq!(changes.releases, []);
    cluster.listed(&three_stripes());
    assert_eq!(cluster.run(LEASE - MOMENT).releases, []);
    assert_eq!(cluster.run(MOMENT * 2).releases.len(), 1);
}

// ---------------------------------------------------------------------------------
// Q14. Whether the routing table is complete.
// ---------------------------------------------------------------------------------

#[test]
fn the_routing_table_is_complete_when_every_known_region_has_an_owner() {
    let mut cluster = Cluster::anew(&[0, 4]);
    let table = cluster.table();
    assert_eq!(table.waiting, 3);
    assert!(!table.is_complete());
    assert_eq!(cluster.waiting(), [0, 1, 2]);

    cluster.register("a", &[held(0, 10)]);
    assert_eq!(cluster.table().waiting, 2);
    cluster.register("b", &[held(1, 11), held(2, 12)]);
    let table = cluster.table();
    assert_eq!(table.waiting, 0);
    assert!(table.is_complete());

    // A region that the list adds waits for an owner, and one that a merge has had
    // released does.
    cluster.listed(&list(&[0, 1, 2, 3], &[], 4));
    assert_eq!(cluster.table().waiting, 1);
    assert!(!cluster.table().is_complete());
    cluster.listed(&list(&[0, 1, 2], &[(3, 0)], 4));
    assert!(cluster.table().is_complete());
    let version = cluster.table().version;
    cluster.merge_to_absorb(1, 2);
    let table = cluster.table();
    assert_eq!(table.waiting, 1);
    assert!(!table.is_complete());
    assert!(table.version > version);

    // A table with fewer routes than the layout has stripes is complete when the
    // regions are fewer.
    cluster.absorb_ended(1, 2, Ok(()));
    cluster.listed(&list(&[0, 1], &[(3, 0), (2, 1)], 4));
    let table = cluster.table();
    assert_eq!(table.routes.len(), 2);
    assert_eq!(table.layout.region_count(), 3);
    assert!(table.is_complete());
}

// ---------------------------------------------------------------------------------
// The routing table through a merge and a split.
// ---------------------------------------------------------------------------------

/// The version goes up with every call that changes a route, the home region, the
/// pairs or how many regions wait, and `Changes::routing` says so.
#[test]
fn the_version_of_the_routing_table_rises_with_every_change_of_it() {
    let mut cluster = three_workers();
    let mut last = cluster.table();
    let mut versions = vec![last.version];
    let mut check = |cluster: &Cluster, changes: &Changes, what: &str| {
        let table = cluster.table();
        let mut same = table.clone();
        same.version = last.version;
        if same == last {
            assert_eq!(table.version, last.version, "{what}: nothing changed");
            assert!(!changes.routing, "{what}: nothing changed");
        } else {
            assert!(table.version > last.version, "{what}: the table changed");
            assert!(changes.routing, "{what}: the table changed");
            versions.push(table.version);
        }
        last = table;
    };

    let changes = cluster.merge(0, 1);
    check(&cluster, &changes, "a merge is asked for");
    let absorbeds = cluster.epoch(1);
    let changes = cluster
        .coordinator
        .released(cluster.now, "b", region(1), absorbeds);
    check(&cluster, &changes, "the region is released");
    let changes = cluster.absorb_ended(0, 1, Ok(()));
    check(&cluster, &changes, "the absorb has ended");
    let changes = cluster.coordinator.unlisted(cluster.now);
    check(&cluster, &changes, "the list cannot be read");
    let changes = cluster.listed(&list(&[0, 2], &[(1, 0)], 3));
    check(&cluster, &changes, "the list has the pair");
    let changes = cluster.listed(&list(&[0, 2], &[(1, 0)], 3));
    check(&cluster, &changes, "the same list again");

    let owner = cluster.owner(2).expect("region 2 has an owner");
    let changes = cluster
        .coordinator
        .split(cluster.now, region(2), &chunks(), ASKER)
        .expect("the split is taken on");
    check(&cluster, &changes, "a split is asked for");
    let Order::SplitOff { as_epoch, part, .. } = changes.orders[0].order.clone() else {
        panic!("expected the order to split: {changes:?}");
    };
    let changes =
        cluster
            .coordinator
            .split_ended(cluster.now, &owner, region(2), as_epoch, Ok(part));
    check(&cluster, &changes, "the split has ended");
    let changes = cluster.listed(&list(&[0, 2, 3], &[(1, 0)], 4));
    check(&cluster, &changes, "the list has the part");
    let changes = cluster.run(LEASE * 3);
    check(&cluster, &changes, "three leases pass");

    // The released region, the pair and the part: three changes at the least.
    assert!(versions.len() >= 4, "{versions:?}");
}

// ---------------------------------------------------------------------------------
// Q15. A coordinator made anew.
// ---------------------------------------------------------------------------------

/// The coordinator has exactly the regions `living`, each with an owner, and stays so.
fn assert_comes_to(cluster: &Cluster, living: &[u32]) {
    assert_eq!(cluster.known(), living);
    assert_eq!(cluster.waiting(), [] as [u32; 0]);
    assert!(cluster.table().is_complete());
    let mut run: Vec<u32> = cluster
        .heard
        .iter()
        .flat_map(|name| cluster.runs(name))
        .collect();
    run.sort_unstable();
    assert_eq!(run, living, "each region is one worker's");
}

#[test]
fn a_new_coordinator_with_a_merge_before_its_release_behind_it_finds_the_regions_as_before() {
    // Nothing had happened but the asking: every worker reports what it ran.
    for list_first in [true, false] {
        let mut cluster = Cluster::anew(&[0, 4]);
        if list_first {
            cluster.listed(&three_stripes());
        }
        cluster.register("a", &[held(0, 10)]);
        cluster.register("b", &[held(1, 11)]);
        cluster.register("c", &[held(2, 12)]);
        if !list_first {
            cluster.listed(&three_stripes());
        }
        assert_comes_to(&cluster, &[0, 1, 2]);
        assert_eq!(cluster.run(LEASE * 3), Changes::default());
        assert_comes_to(&cluster, &[0, 1, 2]);
        assert_eq!(
            (cluster.epoch(0), cluster.epoch(1), cluster.epoch(2)),
            (10, 11, 12)
        );
        // It knows nothing of the merge, and takes on a new one.
        cluster.merge(0, 1);
    }
}

/// "It is assigned at once only if that word was still on its way."
#[test]
fn a_new_coordinator_that_hears_the_release_of_a_merge_assigns_the_region_at_once() {
    let mut cluster = Cluster::anew(&[0, 4]);
    cluster.listed(&three_stripes());
    cluster.register("a", &[held(0, 10)]);
    cluster.register("b", &[]);
    cluster.register("c", &[held(2, 12)]);
    assert_eq!(cluster.waiting(), [1]);

    let changes = cluster
        .coordinator
        .released(cluster.now, "b", region(1), 11);
    assert_eq!(changes.orders, [], "it knows of no merge");
    assert!(cluster.owner(1).is_some());
    assert!(cluster.epoch(1) > 11);
    assert_comes_to(&cluster, &[0, 1, 2]);
}

/// "A released `B` is a region without an owner to it, and waits out the grace period
/// like any such region."
#[test]
fn a_new_coordinator_assigns_the_released_region_of_a_merge_only_after_its_grace_period() {
    for list_read in [true, false] {
        let mut cluster = Cluster::anew(&[0, 4]);
        if list_read {
            cluster.listed(&three_stripes());
        } else {
            cluster.coordinator.unlisted(cluster.now);
        }
        // The old owner let go of the region and reports nothing; the survivor's
        // worker says nothing of the region it has open to absorb.
        cluster.register("a", &[held(0, 10)]);
        cluster.register("b", &[]);
        cluster.register("c", &[held(2, 12)]);
        assert_eq!(cluster.waiting(), [1]);

        // Not before the grace period is over.
        let changes = cluster.run(LEASE - MOMENT);
        assert_eq!(changes.workers, [] as [String; 0]);
        assert_eq!(cluster.waiting(), [1]);
        // And then, which fences the absorb if the record is not written yet.
        let changes = cluster.run(MOMENT * 2);
        assert_eq!(changes.workers, ["b"]);
        assert_eq!(cluster.owner(1).as_deref(), Some("b"));
        assert!(cluster.epoch(1) > FIRST_EPOCH);
        assert_comes_to(&cluster, &[0, 1, 2]);
    }
}

#[test]
fn a_new_coordinator_with_a_merge_that_was_done_behind_it_has_one_region_fewer() {
    let after = list(&[0, 2], &[(1, 0)], 3);
    for list_first in [true, false] {
        let mut cluster = Cluster::anew(&[0, 4]);
        if list_first {
            cluster.listed(&after);
        }
        cluster.register("a", &[held(0, 10)]);
        cluster.register("b", &[]);
        cluster.register("c", &[held(2, 12)]);
        if !list_first {
            assert_eq!(cluster.waiting(), [1]);
            cluster.listed(&after);
        }
        assert_comes_to(&cluster, &[0, 2]);
        assert_eq!(cluster.table().absorbed, pairs(&[(1, 0)]));
        // Its worker's `AbsorbEnded`, if that was still on its way, finds no merge and
        // has the list read, which changes nothing.
        let said = cluster
            .coordinator
            .absorb_ended(cluster.now, "a", region(0), region(1), Ok(()));
        assert!(said.read);
        assert_eq!(said.reshaped, []);
        assert_eq!(cluster.listed(&after).workers, [] as [String; 0]);
        assert_eq!(cluster.run(LEASE * 3).workers, [] as [String; 0]);
        assert_comes_to(&cluster, &[0, 2]);
    }
}

/// The reading failed when the coordinator started, the merge had been done, and the
/// grace period is over: the region is given to a worker, which the store refuses.
#[test]
fn a_new_coordinator_that_cannot_read_the_list_gives_an_absorbed_region_away_and_takes_it_back() {
    let mut cluster = Cluster::anew(&[0, 4]);
    cluster.coordinator.unlisted(cluster.now);
    cluster.register("a", &[held(0, 10)]);
    cluster.register("b", &[]);
    cluster.register("c", &[held(2, 12)]);
    cluster.run(LEASE + MOMENT);
    assert_eq!(cluster.owner(1).as_deref(), Some("b"));

    let said = cluster
        .coordinator
        .absorb_ended(cluster.now, "b", region(0), region(1), Ok(()));
    assert!(said.read);
    let changes = cluster.listed(&list(&[0, 2], &[(1, 0)], 3));
    assert_eq!(changes.workers, ["b"]);
    assert_comes_to(&cluster, &[0, 2]);
}

#[test]
fn a_new_coordinator_with_a_split_before_its_record_behind_it_finds_the_regions_as_before() {
    let mut cluster = Cluster::anew(&[0, 4]);
    cluster.listed(&three_stripes());
    cluster.register("a", &[held(0, 10)]);
    cluster.register("b", &[held(1, 11)]);
    cluster.register("c", &[held(2, 12)]);
    assert_comes_to(&cluster, &[0, 1, 2]);
    // The worker's word that the split is off finds no split.
    let said = cluster
        .coordinator
        .split_ended(cluster.now, "b", region(1), 500, Err(Off::Nobody));
    assert_eq!(said.reshaped, []);
    assert_eq!(cluster.run(LEASE * 3).workers, [] as [String; 0]);
    assert_comes_to(&cluster, &[0, 1, 2]);
}

/// "A part is reported by the worker that runs it."
#[test]
fn a_new_coordinator_with_a_split_behind_it_takes_the_part_from_its_workers_registration() {
    for list_first in [true, false] {
        let mut cluster = Cluster::anew(&[0, 4]);
        let after = list(&[0, 1, 2, 3], &[], 4);
        if list_first {
            cluster.listed(&after);
        }
        cluster.register("a", &[held(0, 10)]);
        cluster.register("b", &[held(1, 11), held(3, 500)]);
        cluster.register("c", &[held(2, 12)]);
        if !list_first {
            cluster.listed(&after);
        }
        assert_comes_to(&cluster, &[0, 1, 2, 3]);
        assert_eq!(cluster.owner(3).as_deref(), Some("b"));
        assert_eq!(cluster.epoch(3), 500);
        assert_eq!(cluster.run(LEASE * 3).workers, [] as [String; 0]);
        assert_eq!(cluster.epoch(3), 500);
    }
}

/// The worker says `SplitEnded` again after every registration until its orders have
/// named the part (section 4); a coordinator that has no such split takes that as a
/// report of the region.
#[test]
fn a_new_coordinator_with_a_split_behind_it_takes_the_part_from_its_workers_word() {
    let mut cluster = Cluster::anew(&[0, 4]);
    cluster.listed(&three_stripes());
    cluster.register("a", &[held(0, 10)]);
    cluster.register("b", &[held(1, 11)]);
    cluster.register("c", &[held(2, 12)]);
    let changes = cluster
        .coordinator
        .split_ended(cluster.now, "b", region(1), 500, Ok(region(3)));
    assert_eq!(changes.reshaped, []);
    assert!(changes.read);
    assert_eq!(cluster.owner(3).as_deref(), Some("b"));
    assert_eq!(cluster.epoch(3), 500);
    cluster.listed(&list(&[0, 1, 2, 3], &[], 4));
    assert_comes_to(&cluster, &[0, 1, 2, 3]);
    assert_eq!(cluster.epoch(3), 500);
}

/// "Or found in the list and assigned after the grace period."
#[test]
fn a_new_coordinator_with_a_split_behind_it_whose_worker_died_finds_the_part_in_the_list() {
    let mut cluster = Cluster::anew(&[0, 4]);
    cluster.listed(&list(&[0, 1, 2, 3], &[], 4));
    cluster.register("a", &[held(0, 10)]);
    cluster.register("c", &[held(2, 12)]);
    assert_eq!(cluster.waiting(), [1, 3]);
    let changes = cluster.run(LEASE - MOMENT);
    assert_eq!(changes.workers, [] as [String; 0]);
    assert_eq!(cluster.waiting(), [1, 3]);
    cluster.run(MOMENT * 2);
    assert_comes_to(&cluster, &[0, 1, 2, 3]);
}

// ---------------------------------------------------------------------------------
// Q16. A worker that leaves while a region of its is reserved.
// ---------------------------------------------------------------------------------

fn releases_of(changes: &Changes) -> Vec<(String, u32)> {
    changes
        .releases
        .iter()
        .map(|release| (release.worker.clone(), release.region.0))
        .collect()
}

#[test]
fn the_survivor_of_a_leaving_worker_is_released_when_its_merge_has_ended() {
    let mut cluster = three_workers();
    let survivors = cluster.epoch(0);
    cluster.merge(0, 1);

    let mut during = cluster.coordinator.leaving(cluster.now, "a");
    add(&mut during, cluster.run(LEASE / 5));
    cluster.let_go(0, 1);
    add(&mut during, cluster.run(LEASE / 5));
    add(&mut during, cluster.absorb_ended(0, 1, Ok(())));
    add(&mut during, cluster.run(LEASE / 5));
    assert_eq!(releases_of(&during), [], "its region is reserved");
    assert_eq!(during.gone, [] as [String; 0]);
    assert_eq!(cluster.owner(0).as_deref(), Some("a"));

    let mut after = cluster.listed(&list(&[0, 2], &[(1, 0)], 3));
    assert_eq!(after.reshaped, [merge_ended(0, 1, Ok(0))]);
    add(&mut after, cluster.tick());
    assert_eq!(releases_of(&after), [("a".to_owned(), 0)]);
    assert_eq!(after.releases[0].epoch, survivors);

    // From here it is a leaver's release like any other.
    let changes = cluster
        .coordinator
        .released(cluster.now, "a", region(0), survivors);
    assert_eq!(changes.gone, ["a"]);
    assert_ne!(cluster.owner(0).as_deref(), Some("a"));
    assert!(cluster.epoch(0) > survivors);
}

#[test]
fn the_survivor_of_a_leaving_worker_is_released_when_its_merge_is_off() {
    let mut cluster = three_workers();
    cluster.merge_to_absorb(0, 1);
    let mut during = cluster.coordinator.leaving(cluster.now, "a");
    add(&mut during, cluster.run(LEASE / 5));
    add(
        &mut during,
        cluster.absorb_ended(0, 1, Err(Off::Unreadable)),
    );
    assert_eq!(releases_of(&during), []);

    let mut after = cluster.listed(&three_stripes());
    add(&mut after, cluster.tick());
    assert_eq!(releases_of(&after), [("a".to_owned(), 0)]);
}

#[test]
fn the_region_of_a_leaving_worker_that_a_merge_has_released_is_the_last_it_had() {
    let mut cluster = three_workers();
    let absorbeds = cluster.epoch(1);
    cluster.merge(0, 1);
    let changes = cluster.coordinator.leaving(cluster.now, "b");
    assert_eq!(changes.gone, [] as [String; 0]);

    // Its release for the merge is what the worker was asked for already; with it the
    // worker has nothing left, and is let go.
    let changes = cluster
        .coordinator
        .released(cluster.now, "b", region(1), absorbeds);
    assert_eq!(changes.gone, ["b"]);
    assert_eq!(changes.orders.len(), 1, "the merge goes on: {changes:?}");
    assert_eq!(cluster.waiting(), [1]);
}

#[test]
fn the_regions_of_a_leaving_worker_are_released_when_its_split_has_ended() {
    let mut cluster = three_workers();
    let owners = cluster.epoch(1);
    let (as_epoch, part) = cluster.split(1);
    let mut during = cluster.coordinator.leaving(cluster.now, "b");
    add(&mut during, cluster.run(LEASE / 2));
    assert_eq!(releases_of(&during), []);
    assert_eq!(during.gone, [] as [String; 0]);

    // The region that was split and the part are both the leaver's.
    let mut after = cluster.split_ended(1, as_epoch, Ok(part));
    add(&mut after, cluster.tick());
    let mut released = after.releases.clone();
    released.sort_by_key(|release| release.region);
    assert_eq!(
        released,
        [
            ReleaseOrder {
                worker: "b".to_owned(),
                region: region(1),
                epoch: owners,
            },
            ReleaseOrder {
                worker: "b".to_owned(),
                region: region(part),
                epoch: as_epoch,
            },
        ]
    );
}

#[test]
fn the_region_of_a_leaving_worker_is_released_when_its_split_was_overdue() {
    let mut cluster = three_workers();
    cluster.split(1);
    cluster.coordinator.leaving(cluster.now, "b");
    assert_eq!(releases_of(&cluster.run(LEASE - MOMENT)), []);
    let mut after = cluster.run(MOMENT * 2);
    add(&mut after, cluster.step(MOMENT));
    assert_eq!(after.reshaped, [split_ended(1, Err(Undone::Overdue))]);
    assert_eq!(releases_of(&after), [("b".to_owned(), 1)]);
}

// ---------------------------------------------------------------------------------
// Q17. A reading that shows the part while its split is reserved.
// ---------------------------------------------------------------------------------

#[test]
fn a_reading_that_shows_an_unknown_region_while_a_split_is_reserved_adds_nothing() {
    let mut cluster = three_workers();
    cluster.register("d", &[]);
    let (as_epoch, part) = cluster.split(1);
    let before = cluster.table();
    let with_part = list(&[0, 1, 2, 3], &[], 4);

    // The record is written, and the worker has yet to say so. A worker with nothing
    // to run is there, and is not given the part.
    let changes = cluster.listed_and_looked(&with_part);
    assert_eq!(changes, Changes::default());
    assert_eq!(cluster.table(), before);
    assert_eq!(cluster.known(), [0, 1, 2]);
    let changes = cluster.run(LEASE / 2);
    assert_eq!(changes, Changes::default());
    assert_eq!(cluster.listed_and_looked(&with_part), Changes::default());

    // The worker's word then makes it the worker's, with the epoch that was ordered.
    let changes = cluster.split_ended(1, as_epoch, Ok(part));
    assert_eq!(changes.reshaped, [split_ended(1, Ok(part))]);
    assert_eq!(cluster.owner(part).as_deref(), Some("b"));
    assert_eq!(cluster.epoch(part), as_epoch);
    let changes = cluster.listed_and_looked(&with_part);
    assert_eq!(changes.workers, [] as [String; 0]);
    assert_eq!(cluster.epoch(part), as_epoch);
    assert_eq!(cluster.runs("d"), [] as [u32; 0]);
}

#[test]
fn the_same_reading_when_the_reservation_has_run_out_adds_the_region_without_an_owner() {
    let mut cluster = three_workers();
    cluster.register("d", &[]);
    let (as_epoch, part) = cluster.split(1);
    let with_part = list(&[0, 1, 2, 3], &[], 4);
    assert_eq!(cluster.listed_and_looked(&with_part), Changes::default());

    let changes = cluster.run(LEASE + MOMENT);
    assert_eq!(changes.reshaped, [split_ended(1, Err(Undone::Overdue))]);
    assert!(changes.read);
    assert_eq!(cluster.known(), [0, 1, 2], "only a reading adds it");

    let changes = cluster.listed(&with_part);
    assert_eq!(changes.reshaped, []);
    assert_eq!(cluster.known(), [0, 1, 2, 3]);
    cluster.tick();
    // The worker with nothing to run is given it, with an epoch that fences the part.
    assert_eq!(cluster.owner(part).as_deref(), Some("d"));
    assert!(cluster.epoch(part) > as_epoch);
}

/// A split of one region must not hide a region that has nothing to do with it for
/// longer than the reservation: the reading after it adds what is still unknown.
#[test]
fn a_reading_while_a_split_is_reserved_still_removes_what_was_absorbed() {
    let mut cluster = three_workers();
    cluster.split(1);
    let changes = cluster.listed(&list(&[0, 1, 3], &[(2, 0)], 4));
    assert_eq!(changes.workers, ["c"]);
    assert_eq!(cluster.known(), [0, 1]);
    assert_eq!(cluster.table().absorbed, pairs(&[(2, 0)]));
}

// ---------------------------------------------------------------------------------
// A merge and a split against what else can happen meanwhile.
// ---------------------------------------------------------------------------------

#[test]
fn readings_that_show_the_regions_as_before_or_fail_leave_a_merge_as_it_is() {
    let mut cluster = three_workers();
    let (survivors, absorbeds) = (cluster.epoch(0), cluster.epoch(1));
    cluster.merge(0, 1);
    for _ in 0..2 {
        assert_eq!(
            cluster.listed_and_looked(&three_stripes()),
            Changes::default()
        );
        assert_eq!(
            cluster.coordinator.unlisted(cluster.now),
            Changes::default()
        );
        assert_eq!(cluster.tick(), Changes::default());
    }
    assert_eq!(cluster.owner(1).as_deref(), Some("b"));
    assert_eq!(cluster.epoch(1), absorbeds);

    // And when the region has been released: it is nobody's still, and the survivor's
    // owner is not told again.
    let as_epoch = cluster.let_go(0, 1);
    for _ in 0..2 {
        assert_eq!(
            cluster.listed_and_looked(&three_stripes()),
            Changes::default()
        );
        assert_eq!(
            cluster.coordinator.unlisted(cluster.now),
            Changes::default()
        );
        assert_eq!(cluster.tick(), Changes::default());
    }
    assert_eq!(cluster.waiting(), [1]);
    assert_eq!(cluster.epoch(0), survivors);

    // The merge is the one it was.
    let changes = cluster.register_again("a");
    assert_eq!(
        changes.orders,
        [ReshapeOrder {
            worker: "a".to_owned(),
            order: Order::Absorb {
                region: region(0),
                epoch: survivors,
                absorbed: region(1),
                as_epoch,
            },
        }]
    );
}

/// The list decides what happened, and the service reads it whenever a worker
/// registers: such a reading can show the merge done before its worker has said so.
#[test]
fn a_reading_that_shows_the_merge_done_before_the_workers_word_ends_it_as_done() {
    let mut cluster = three_workers();
    cluster.merge_to_absorb(0, 1);
    let changes = cluster.listed(&list(&[0, 2], &[(1, 0)], 3));
    assert_eq!(changes.reshaped, [merge_ended(0, 1, Ok(0))]);
    assert_eq!(cluster.known(), [0, 2]);
    assert!(cluster.table().is_complete());

    // The worker's word, which comes then, finds nothing to end.
    let said = cluster.absorb_ended(0, 1, Ok(()));
    assert_eq!(said.reshaped, []);
    assert_eq!(cluster.listed(&list(&[0, 2], &[(1, 0)], 3)).reshaped, []);
}

#[test]
fn readings_that_show_the_regions_as_before_or_fail_leave_a_split_as_it_is() {
    let mut cluster = three_workers();
    let (as_epoch, part) = cluster.split(1);
    for _ in 0..2 {
        assert_eq!(
            cluster.listed_and_looked(&three_stripes()),
            Changes::default()
        );
        assert_eq!(
            cluster.coordinator.unlisted(cluster.now),
            Changes::default()
        );
        assert_eq!(cluster.tick(), Changes::default());
    }
    assert_split_refused(&mut cluster, 1, ReshapeRefusal::Reserved(region(1)));
    let changes = cluster.split_ended(1, as_epoch, Ok(part));
    assert_eq!(changes.reshaped, [split_ended(1, Ok(part))]);
}

/// A reading takes the region that is being split away: the reservation that names
/// it ends (section 5.2).
#[test]
fn a_reading_that_takes_the_region_of_a_split_away_ends_the_split() {
    let mut cluster = three_workers();
    cluster.split(1);
    let changes = cluster.listed(&list(&[0, 2], &[(1, 0)], 3));
    let asked = Asked::Split { region: region(1) };
    assert!(lost_or_gone(&changes.reshaped, asked, 1), "{changes:?}");
    assert_eq!(cluster.known(), [0, 2]);
    assert_eq!(cluster.runs("b"), [] as [u32; 0]);
    // Nothing more is heard of it: not when its time would have been up, and not
    // from the worker.
    assert_eq!(cluster.run(LEASE * 2).reshaped, []);
}

/// What `Undone::Gone` says of itself in the coordinator's interface: "The world
/// store's list no longer has this region of it."
#[test]
fn whoever_asked_is_told_that_the_list_no_longer_has_the_region() {
    // Each of these is a merge or a split of which a reading takes a region away: it
    // has the region among the absorbed (gone into another region than the merge was
    // to put it into), or has it nowhere below its next id. The comment of
    // `Undone::Gone` describes this very case, and that of `Undone::Disowned` an
    // owner that was lost or an epoch that changed.
    //
    // What happened: `Gone(1)` is told in the three cases in which the region taken
    // away is the one to absorb, and `Disowned(1)` in the four in which it is the
    // region of a split or the survivor of a merge, before the release and after it.
    //
    // Section 5.2 only says that "a reservation that names it ends", so this is the
    // interface against its own comment and no more: the regions and owners come out
    // as the record has them, and whoever asked reads "lost its owner" where the
    // region is no more. Step C4 is to act on these reasons (section 5.5).
    let gone = |id: u32| Err::<RegionId, Undone>(Undone::Gone(region(id)));
    let absorbed_by_home = list(&[0, 2], &[(1, 0)], 3);
    let absorbed_by_another = list(&[0, 2], &[(1, 2)], 3);
    let nowhere = list(&[0, 2], &[], 3);
    let mut told = Vec::new();
    let mut expected = Vec::new();
    let mut tell = |what: &'static str, changes: Changes, lost: u32| {
        let outcomes: Vec<_> = changes.reshaped.iter().map(|ended| ended.outcome).collect();
        told.push((what, outcomes));
        expected.push((what, vec![gone(lost)]));
    };

    let mut cluster = three_workers();
    cluster.split(1);
    tell(
        "a split whose region was absorbed",
        cluster.listed(&absorbed_by_home),
        1,
    );
    let mut cluster = three_workers();
    cluster.split(1);
    tell(
        "a split whose region is nowhere",
        cluster.listed(&nowhere),
        1,
    );
    let mut cluster = three_workers();
    cluster.merge(1, 2);
    tell(
        "a merge before the release whose survivor was absorbed",
        cluster.listed(&absorbed_by_home),
        1,
    );
    let mut cluster = three_workers();
    cluster.merge(0, 1);
    tell(
        "a merge before the release whose other region was absorbed elsewhere",
        cluster.listed(&absorbed_by_another),
        1,
    );
    let mut cluster = three_workers();
    cluster.merge_to_absorb(1, 2);
    tell(
        "a merge after the release whose survivor was absorbed",
        cluster.listed(&absorbed_by_home),
        1,
    );
    let mut cluster = three_workers();
    cluster.merge_to_absorb(0, 1);
    tell(
        "a merge after the release whose other region was absorbed elsewhere",
        cluster.listed(&absorbed_by_another),
        1,
    );
    let mut cluster = three_workers();
    cluster.merge_to_absorb(0, 1);
    tell(
        "a merge after the release whose other region is nowhere",
        cluster.listed(&nowhere),
        1,
    );
    assert_eq!(told, expected);
}

#[test]
fn a_reading_that_takes_the_region_to_absorb_away_before_its_release_ends_the_merge() {
    let mut cluster = three_workers();
    // Region 1 is to be absorbed by the home region, and the list has it absorbed by
    // region 2 instead.
    cluster.merge(0, 1);
    let changes = cluster.listed(&list(&[0, 2], &[(1, 2)], 3));
    let asked = Asked::Merge {
        survivor: region(0),
        absorbed: region(1),
    };
    assert!(lost_or_gone(&changes.reshaped, asked, 1), "{changes:?}");
    assert_eq!(cluster.known(), [0, 2]);
    assert_eq!(cluster.runs("b"), [] as [u32; 0]);
    assert_eq!(changes.orders, [], "nobody is told to absorb it");
    // The survivor is free again.
    cluster.merge(0, 2);
}

/// The same when the region has been released and waits to be absorbed: the list has
/// it absorbed, by another region than the one that was to.
#[test]
fn a_reading_that_has_the_region_to_absorb_gone_elsewhere_ends_the_merge_as_not_done() {
    let mut cluster = three_workers();
    cluster.merge_to_absorb(0, 1);
    let changes = cluster.listed(&list(&[0, 2], &[(1, 2)], 3));
    let asked = Asked::Merge {
        survivor: region(0),
        absorbed: region(1),
    };
    assert!(lost_or_gone(&changes.reshaped, asked, 1), "{changes:?}");
    assert_eq!(cluster.known(), [0, 2]);
    assert!(cluster.table().is_complete());
    cluster.merge(0, 2);
}

/// The survivor is taken away by a reading when the other region has been released:
/// that reading is the one the record wants read first, and it has the other region
/// living, so it is assigned.
#[test]
fn a_reading_that_takes_the_survivor_away_after_the_release_has_the_other_region_assigned() {
    let mut cluster = three_workers();
    let as_epoch = cluster.merge_to_absorb(1, 2);
    let changes = cluster.listed_and_looked(&list(&[0, 2], &[(1, 0)], 3));
    let asked = Asked::Merge {
        survivor: region(1),
        absorbed: region(2),
    };
    assert!(lost_or_gone(&changes.reshaped, asked, 1), "{changes:?}");
    assert_eq!(cluster.known(), [0, 2]);
    assert!(cluster.owner(2).is_some());
    assert!(cluster.epoch(2) > as_epoch);
    assert!(cluster.table().is_complete());
}

/// A worker's word about a merge that no worker has been told to carry out yet only
/// has the list read.
#[test]
fn the_word_that_an_absorb_has_ended_before_the_release_leaves_the_merge_as_it_is() {
    let mut cluster = three_workers();
    cluster.merge(0, 1);
    let said = cluster
        .coordinator
        .absorb_ended(cluster.now, "c", region(0), region(1), Ok(()));
    assert!(said.read);
    assert_eq!(said.reshaped, []);
    assert_eq!(cluster.listed(&three_stripes()).reshaped, []);
    assert_eq!(cluster.owner(1).as_deref(), Some("b"));
    cluster.let_go(0, 1);
}

/// The epoch to absorb with is the region's from then on: its old owner, which says
/// that it still holds it, was replaced.
#[test]
fn the_old_owner_of_a_region_that_waits_to_be_absorbed_cannot_report_it_back() {
    let mut cluster = three_workers();
    let absorbeds = cluster.epoch(1);
    let as_epoch = cluster.merge_to_absorb(0, 1);
    assert!(as_epoch > absorbeds);
    let holding = [Assignment {
        region: region(1),
        epoch: absorbeds,
        entity_ids: EntityIds::block(40).expect("there are that many blocks"),
    }];
    let changes = cluster.register("b", &holding);
    assert_eq!(cluster.runs("b"), [] as [u32; 0]);
    assert_eq!(cluster.waiting(), [1]);
    assert_eq!(changes.reshaped, []);
    // The merge is the one it was.
    cluster.absorb_ended(0, 1, Ok(()));
    let changes = cluster.listed(&list(&[0, 2], &[(1, 0)], 3));
    assert_eq!(changes.reshaped, [merge_ended(0, 1, Ok(0))]);
}

/// A worker that is restarted in place registers holding nothing and is given its
/// region again with the epoch it had (ADR-0009, section 3). For the survivor of a
/// merge that is the owner registering again: it is told to absorb again.
#[test]
fn the_survivors_owner_that_comes_back_holding_nothing_is_told_to_absorb_again() {
    let mut cluster = three_workers();
    let survivors = cluster.epoch(0);
    let as_epoch = cluster.merge_to_absorb(0, 1);
    let changes = cluster.register("a", &[]);
    assert_eq!(cluster.owner(0).as_deref(), Some("a"));
    assert_eq!(cluster.epoch(0), survivors);
    assert_eq!(
        changes.orders,
        [ReshapeOrder {
            worker: "a".to_owned(),
            order: Order::Absorb {
                region: region(0),
                epoch: survivors,
                absorbed: region(1),
                as_epoch,
            },
        }]
    );
    assert_eq!(changes.reshaped, []);
}

#[test]
fn the_other_regions_worker_being_refused_by_the_store_before_the_release_ends_the_merge() {
    let mut cluster = three_workers();
    let absorbeds = cluster.epoch(1);
    cluster.merge(0, 1);
    let mut changes =
        cluster
            .coordinator
            .epoch_refused(cluster.now, "b", region(1), absorbeds + 100);
    add(&mut changes, cluster.tick());
    assert_eq!(
        changes.reshaped,
        [merge_ended(0, 1, Err(Undone::Disowned(region(1))))]
    );
    assert_eq!(changes.orders, [], "nobody is told to absorb");
    assert!(cluster.epoch(1) > absorbeds + 100);
    assert!(cluster.table().is_complete());
    // Being refused says nothing against a worker (ADR-0009): it has the fewest
    // regions now, and is given the region again.
    assert_eq!(cluster.owner(1).as_deref(), Some("b"));
    cluster.merge(0, 2);
}

/// While the reading that is to decide is under way, the region stays nobody's,
/// however many ticks pass.
#[test]
fn an_absorb_unanswered_for_a_lease_leaves_the_region_alone_until_the_list_has_been_read() {
    let (mut cluster, as_epoch) = an_absorb_unanswered_for_a_lease();
    let changes = cluster.run(LEASE * 2);
    assert_eq!(changes.workers, [] as [String; 0]);
    assert_eq!(changes.reshaped, []);
    assert_eq!(cluster.waiting(), [1]);
    let changes = cluster.listed(&three_stripes());
    assert_eq!(changes.reshaped, [merge_ended(0, 1, Err(Undone::Overdue))]);
    assert!(cluster.epoch(1) > as_epoch);
}

/// The merge was given up for overdue and the region assigned, and the survivor's
/// worker got its absorb through before the new owner opened the region: that owner
/// is refused by the store and says so, and the list takes the region away.
#[test]
fn a_merge_that_was_given_up_and_done_all_the_same_is_found_in_the_list() {
    let (mut cluster, _) = an_absorb_unanswered_for_a_lease();
    let changes = cluster.listed(&three_stripes());
    assert_eq!(changes.reshaped, [merge_ended(0, 1, Err(Undone::Overdue))]);
    let owner = cluster.owner(1).expect("it is assigned");

    let said = cluster
        .coordinator
        .absorb_ended(cluster.now, &owner, region(0), region(1), Ok(()));
    assert!(said.read);
    let changes = cluster.listed(&list(&[0, 2], &[(1, 0)], 3));
    assert_eq!(changes.reshaped, []);
    assert_eq!(cluster.known(), [0, 2]);
    assert_eq!(cluster.table().absorbed, pairs(&[(1, 0)]));
    assert!(cluster.table().is_complete());
}

/// Section 5.4: a `SplitEnded` with an epoch the coordinator has no reservation for
/// reports a region and ends nothing.
#[test]
fn the_word_of_a_split_with_another_epoch_than_was_ordered_does_not_end_the_split() {
    let mut cluster = three_workers();
    let (as_epoch, part) = cluster.split(1);
    let changes = cluster.split_ended(1, as_epoch + 77, Ok(7));
    assert_eq!(changes.reshaped, []);
    assert_split_refused(&mut cluster, 1, ReshapeRefusal::Reserved(region(1)));
    // The region it names is taken as one the worker reports.
    assert_eq!(cluster.owner(7).as_deref(), Some("b"));
    assert_eq!(cluster.epoch(7), as_epoch + 77);
    let changes = cluster.split_ended(1, as_epoch, Ok(part));
    assert_eq!(changes.reshaped, [split_ended(1, Ok(part))]);
    assert_eq!(cluster.runs("b"), [1, part, 7]);
}

#[test]
fn a_worker_that_registers_meanwhile_is_given_neither_region_of_a_merge() {
    let mut cluster = three_workers();
    cluster.merge_to_absorb(0, 1);
    let changes = cluster.register("d", &[]);
    assert_eq!(changes.orders, []);
    assert_eq!(cluster.runs("d"), [] as [u32; 0]);
    cluster.run(LEASE / 2);
    assert_eq!(cluster.runs("d"), [] as [u32; 0]);
    assert_eq!(cluster.waiting(), [1]);
}

#[test]
fn two_merges_and_a_split_of_different_regions_go_on_side_by_side() {
    let mut cluster = Cluster::settled(&[0, 4, 8, 12, 16], &["a", "b", "c", "d", "e", "f"]);
    cluster.listed(&list(&[0, 1, 2, 3, 4, 5], &[], 6));
    cluster.merge(0, 1);
    cluster.merge(2, 3);
    let (as_epoch, part) = cluster.split(4);
    let first = cluster.let_go(0, 1);
    let second = cluster.let_go(2, 3);
    assert_ne!(first, second);
    assert!(as_epoch < first && first < second);
    assert_eq!(cluster.waiting(), [1, 3]);

    // The split's word comes first, then one reading that shows one merge done and
    // the other not yet.
    let changes = cluster.split_ended(4, as_epoch, Ok(part));
    assert_eq!(changes.reshaped, [split_ended(4, Ok(part))]);
    cluster.absorb_ended(2, 3, Ok(()));
    let changes = cluster.listed(&list(&[0, 1, 2, 4, 5, 6], &[(3, 2)], 7));
    assert_eq!(changes.reshaped, [merge_ended(2, 3, Ok(2))]);
    assert_eq!(cluster.waiting(), [1]);
    assert_eq!(cluster.known(), [0, 1, 2, 4, 5, 6]);

    cluster.absorb_ended(0, 1, Ok(()));
    let changes = cluster.listed(&list(&[0, 2, 4, 5, 6], &[(3, 2), (1, 0)], 7));
    assert_eq!(changes.reshaped, [merge_ended(0, 1, Ok(0))]);
    assert_eq!(cluster.known(), [0, 2, 4, 5, 6]);
    assert!(cluster.table().is_complete());
}

/// A merge and a split asked by nobody, as step C4 will ask them: their outcome is in
/// `Changes` all the same (section 5.5).
#[test]
fn a_merge_and_a_split_that_nobody_asked_for_end_in_the_changes_too() {
    let mut cluster = three_workers();
    cluster
        .coordinator
        .merge(cluster.now, region(0), region(1), None)
        .expect("the merge is taken on");
    cluster.let_go(0, 1);
    cluster.absorb_ended(0, 1, Err(Off::Unreadable));
    let changes = cluster.listed(&three_stripes());
    assert_eq!(
        changes.reshaped,
        [Reshaped {
            asker: None,
            ..merge_ended(0, 1, Err(Undone::Off(Off::Unreadable)))
        }]
    );

    let changes = cluster
        .coordinator
        .split(cluster.now, region(2), &chunks(), None)
        .expect("the split is taken on");
    let Order::SplitOff { as_epoch, .. } = &changes.orders[0].order else {
        panic!("expected the order to split: {changes:?}");
    };
    let changes = cluster.split_ended(2, *as_epoch, Err(Off::Nobody));
    assert_eq!(
        changes.reshaped,
        [Reshaped {
            asker: None,
            ..split_ended(2, Err(Undone::Off(Off::Nobody)))
        }]
    );
}

// ---------------------------------------------------------------------------------
// However a reservation ends, its regions count as vouched for at that moment.
// ---------------------------------------------------------------------------------

/// The region is its owner's with `epoch` for a whole lease from now without being
/// vouched for, and no longer.
fn assert_has_a_lease_from_now(cluster: &mut Cluster, id: u32, epoch: u64) {
    let changes = cluster.run(LEASE - MOMENT);
    assert!(
        !cluster.lost(id, epoch),
        "region {id} was taken from its owner within a lease of the end: {changes:?}"
    );
    cluster.run(MOMENT * 2);
    assert!(cluster.lost(id, epoch), "nobody vouches for region {id}");
}

/// Three workers, of which the home region's does not vouch for it and has not for
/// four seconds, and a merge of region 1 into the home region asked for now. The
/// survivor would lose its owner a second from now if it were not reserved.
fn a_merge_whose_survivor_is_not_vouched_for() -> (Cluster, u64) {
    let mut cluster = three_workers();
    let survivors = cluster.epoch(0);
    cluster.stop_vouching("a");
    cluster.run(LEASE - Duration::from_secs(1));
    cluster.merge(0, 1);
    (cluster, survivors)
}

#[test]
fn the_survivor_has_a_lease_from_the_end_of_a_merge_that_its_worker_called_off() {
    let (mut cluster, survivors) = a_merge_whose_survivor_is_not_vouched_for();
    cluster.let_go(0, 1);
    cluster.run(LEASE - Duration::from_secs(1));
    cluster.absorb_ended(0, 1, Err(Off::Unreadable));
    let ended = cluster.listed(&three_stripes());
    assert_eq!(
        ended.reshaped,
        [merge_ended(0, 1, Err(Undone::Off(Off::Unreadable)))]
    );
    assert_has_a_lease_from_now(&mut cluster, 0, survivors);
}

#[test]
fn the_survivor_has_a_lease_from_the_end_of_a_merge_whose_release_was_overdue() {
    let (mut cluster, survivors) = a_merge_whose_survivor_is_not_vouched_for();
    let ended = cluster.run(LEASE + MOMENT);
    assert_eq!(
        ended.reshaped,
        [merge_ended(0, 1, Err(Undone::NotReleased))]
    );
    assert_has_a_lease_from_now(&mut cluster, 0, survivors);
}

#[test]
fn the_survivor_has_a_lease_from_the_end_of_a_merge_whose_absorb_was_overdue() {
    for read in [true, false] {
        let (mut cluster, survivors) = a_merge_whose_survivor_is_not_vouched_for();
        cluster.let_go(0, 1);
        let overdue = cluster.run(LEASE + MOMENT);
        assert!(overdue.read);
        let ended = if read {
            cluster.listed(&three_stripes())
        } else {
            cluster.coordinator.unlisted(cluster.now)
        };
        let why = if read {
            Undone::Overdue
        } else {
            Undone::Unread
        };
        assert_eq!(ended.reshaped, [merge_ended(0, 1, Err(why))]);
        assert_has_a_lease_from_now(&mut cluster, 0, survivors);
    }
}

#[test]
fn the_survivor_has_a_lease_from_the_end_of_a_merge_that_was_found_done_after_its_time() {
    let (mut cluster, survivors) = a_merge_whose_survivor_is_not_vouched_for();
    cluster.let_go(0, 1);
    assert!(cluster.run(LEASE + MOMENT).read);
    let ended = cluster.listed(&list(&[0, 2], &[(1, 0)], 3));
    assert_eq!(ended.reshaped, [merge_ended(0, 1, Ok(0))]);
    assert_has_a_lease_from_now(&mut cluster, 0, survivors);
}

/// Three workers, of which region 1's does not vouch for it and has not for four
/// seconds, and a split of that region asked for now.
fn a_split_of_a_region_that_is_not_vouched_for() -> (Cluster, u64, u64) {
    let mut cluster = three_workers();
    let owners = cluster.epoch(1);
    cluster.stop_vouching("b");
    cluster.run(LEASE - Duration::from_secs(1));
    let (as_epoch, _) = cluster.split(1);
    (cluster, owners, as_epoch)
}

#[test]
fn the_region_has_a_lease_from_the_end_of_a_split_that_its_worker_called_off() {
    let (mut cluster, owners, as_epoch) = a_split_of_a_region_that_is_not_vouched_for();
    cluster.run(LEASE - Duration::from_secs(1));
    let ended = cluster.split_ended(1, as_epoch, Err(Off::Nobody));
    assert_eq!(
        ended.reshaped,
        [split_ended(1, Err(Undone::Off(Off::Nobody)))]
    );
    cluster.listed(&three_stripes());
    assert_has_a_lease_from_now(&mut cluster, 1, owners);
}

#[test]
fn the_region_has_a_lease_from_the_end_of_a_split_that_was_overdue() {
    let (mut cluster, owners, _) = a_split_of_a_region_that_is_not_vouched_for();
    let ended = cluster.run(LEASE + MOMENT);
    assert_eq!(ended.reshaped, [split_ended(1, Err(Undone::Overdue))]);
    cluster.listed(&three_stripes());
    assert_has_a_lease_from_now(&mut cluster, 1, owners);
}

// ---------------------------------------------------------------------------------
// More of what can happen meanwhile.
// ---------------------------------------------------------------------------------

/// "Only the worker's word says which region a split made": the store can have
/// declined the id that was ordered, and the runner have tried again with the next.
#[test]
fn a_split_that_made_another_region_than_was_ordered_ends_with_the_one_it_made() {
    let mut cluster = three_workers();
    let (as_epoch, part) = cluster.split(1);
    let changes = cluster.split_ended(1, as_epoch, Ok(part + 1));
    assert_eq!(changes.reshaped, [split_ended(1, Ok(part + 1))]);
    assert_eq!(cluster.owner(part + 1).as_deref(), Some("b"));
    assert_eq!(cluster.epoch(part + 1), as_epoch);
    assert_eq!(cluster.known(), [0, 1, 2, part + 1]);

    // The list has the other split's region as well, which nobody is known to run.
    cluster.listed_and_looked(&list(&[0, 1, 2, 3, 4], &[], 5));
    assert_eq!(cluster.known(), [0, 1, 2, 3, 4]);
    assert_eq!(cluster.epoch(4), as_epoch);
    assert!(cluster.owner(3).is_some());
}

/// Only a reserved split leaves an unknown region of the list out; a merge has no
/// part.
#[test]
fn a_reading_that_shows_an_unknown_region_while_a_merge_is_reserved_adds_it() {
    let mut cluster = three_workers();
    cluster.register("d", &[]);
    cluster.merge_to_absorb(0, 1);
    cluster.listed_and_looked(&list(&[0, 1, 2, 3], &[], 4));
    assert_eq!(cluster.owner(3).as_deref(), Some("d"));
    // The region that waits to be absorbed is still nobody's.
    assert_eq!(cluster.waiting(), [1]);
}

#[test]
fn nothing_is_evened_out_within_a_lease_of_a_merge_that_came_to_nothing() {
    let mut cluster = Cluster::settled(&[0, 4, 8], &["a"]);
    cluster.listed(&list(&[0, 1, 2, 3], &[], 4));
    cluster.register("b", &[]);
    cluster.merge_to_absorb(0, 1);
    cluster.run(LEASE / 2);
    cluster.absorb_ended(0, 1, Err(Off::Unreadable));
    let ended = cluster.listed(&list(&[0, 1, 2, 3], &[], 4));
    assert_eq!(
        ended.reshaped,
        [merge_ended(0, 1, Err(Undone::Off(Off::Unreadable)))]
    );
    // The region went to the worker that had none; the other still has two more.
    assert_eq!(cluster.runs("a"), [0, 2, 3]);
    assert_eq!(cluster.runs("b"), [1]);
    assert_eq!(cluster.run(LEASE - MOMENT).releases, []);
    assert_eq!(releases_of(&cluster.run(MOMENT * 2)), [("a".to_owned(), 3)]);
}

/// A worker that lets go of a region by itself is taken at its word whether or not
/// it was asked (ADR-0009, section 1, step 4). For the survivor of a merge that is
/// its owner lost: the reservation ends, and the other region's owner is not blamed.
#[test]
fn the_survivors_owner_letting_go_of_it_before_the_release_ends_the_merge() {
    let mut cluster = Cluster::settled(&[0, 4], &["a", "b", "c", "d"]);
    cluster.listed(&three_stripes());
    let (survivors, absorbeds) = (cluster.epoch(0), cluster.epoch(1));
    cluster.merge(0, 1);
    let mut changes = cluster
        .coordinator
        .released(cluster.now, "a", region(0), survivors);
    add(&mut changes, cluster.tick());
    assert_eq!(
        changes.reshaped,
        [merge_ended(0, 1, Err(Undone::Disowned(region(0))))]
    );
    assert!(cluster.epoch(0) > survivors);
    assert_ne!(cluster.owner(1).as_deref(), Some("b"));
    assert!(cluster.epoch(1) > absorbeds);
    assert!(cluster.table().is_complete());
}

/// The same when the other region has been released: the list is read before that
/// region is given to anyone.
#[test]
fn the_survivors_owner_letting_go_of_it_while_it_is_to_absorb_has_the_list_read_first() {
    let mut cluster = three_workers();
    let survivors = cluster.epoch(0);
    let as_epoch = cluster.merge_to_absorb(0, 1);
    let mut changes = cluster
        .coordinator
        .released(cluster.now, "a", region(0), survivors);
    add(&mut changes, cluster.tick());
    assert!(changes.read, "{changes:?}");
    assert_eq!(changes.reshaped, []);
    assert!(cluster.waiting().contains(&1));
    assert!(cluster.lost(0, survivors));

    let ended = cluster.listed_and_looked(&three_stripes());
    assert_eq!(
        ended.reshaped,
        [merge_ended(0, 1, Err(Undone::Disowned(region(0))))]
    );
    assert!(cluster.epoch(1) > as_epoch);
    assert!(cluster.table().is_complete());
}

#[test]
fn the_owner_letting_go_of_a_region_that_is_being_split_ends_the_split() {
    let mut cluster = three_workers();
    let owners = cluster.epoch(1);
    cluster.split(1);
    let mut changes = cluster
        .coordinator
        .released(cluster.now, "b", region(1), owners);
    add(&mut changes, cluster.tick());
    assert_eq!(
        changes.reshaped,
        [split_ended(1, Err(Undone::Disowned(region(1))))]
    );
    assert!(changes.read);
    assert!(cluster.epoch(1) > owners);
}

/// A leaving worker whose connection ends is gone at once (ADR-0009, section 3), and
/// the survivor it ran is without an owner: that ends the merge like any other loss.
#[test]
fn a_leaving_worker_that_is_gone_while_it_is_to_absorb_ends_the_merge() {
    let mut cluster = three_workers();
    let survivors = cluster.epoch(0);
    let as_epoch = cluster.merge_to_absorb(0, 1);
    cluster.coordinator.leaving(cluster.now, "a");
    cluster.silence("a");
    let mut changes = cluster.coordinator.disconnected(cluster.now, "a");
    add(&mut changes, cluster.tick());
    assert!(changes.read, "{changes:?}");
    assert!(cluster.lost(0, survivors));
    assert!(cluster.waiting().contains(&1));

    let ended = cluster.listed_and_looked(&list(&[0, 2], &[(1, 0)], 3));
    assert_eq!(ended.reshaped, [merge_ended(0, 1, Ok(0))]);
    assert_eq!(cluster.known(), [0, 2]);
    assert!(cluster.table().is_complete());
    assert!(as_epoch > survivors);
}

/// The time a merge has counts from when it was asked, not from when the region was
/// released: a release that comes at the last moment leaves the absorb no time.
#[test]
fn a_release_at_the_last_moment_leaves_the_merge_its_one_lease_and_no_more() {
    let mut cluster = three_workers();
    cluster.merge(0, 1);
    cluster.run(LEASE - MOMENT);
    let as_epoch = cluster.let_go(0, 1);
    assert_eq!(cluster.step(MOMENT), Changes::default());
    let over = cluster.step(MOMENT);
    assert!(over.read, "{over:?}");
    assert_eq!(over.reshaped, []);
    let ended = cluster.listed(&three_stripes());
    assert_eq!(ended.reshaped, [merge_ended(0, 1, Err(Undone::Overdue))]);
    assert!(cluster.epoch(1) > as_epoch);
}

/// A release that comes after the merge was given up is an ordinary one: of a region
/// that is the worker's no more, and changes nothing.
#[test]
fn a_release_that_comes_after_the_merge_was_given_up_changes_nothing() {
    let mut cluster = three_workers();
    let absorbeds = cluster.epoch(1);
    cluster.merge(0, 1);
    let over = cluster.run(LEASE + MOMENT);
    assert_eq!(over.reshaped, [merge_ended(0, 1, Err(Undone::NotReleased))]);
    let owner = cluster.owner(1);
    let epoch = cluster.epoch(1);

    let late = cluster
        .coordinator
        .released(cluster.now, "b", region(1), absorbeds);
    assert_eq!(late.orders, [], "nobody is told to absorb");
    assert_eq!(late.reshaped, []);
    assert_eq!(cluster.owner(1), owner);
    assert_eq!(cluster.epoch(1), epoch);
}

/// "If the list cannot be read, it is read again at every tick until the merge's time
/// is up", and then, as it cannot be read either, the region is assigned all the same.
#[test]
fn a_merge_whose_list_cannot_be_read_until_its_time_is_up_has_the_region_assigned() {
    let mut cluster = three_workers();
    let asked = cluster.now;
    let as_epoch = cluster.merge_to_absorb(0, 1);
    cluster.run(LEASE / 5);
    assert!(cluster.absorb_ended(0, 1, Ok(())).read);
    let mut ended = Vec::new();
    loop {
        ended.extend(cluster.coordinator.unlisted(cluster.now).reshaped);
        if cluster.waiting() != [1] {
            break;
        }
        assert!(
            cluster.now <= asked + LEASE + Cluster::STEP,
            "the region is nobody's long after the merge's time"
        );
        let tick = cluster.step(Cluster::STEP);
        assert!(tick.read, "the list is not asked for again: {tick:?}");
        ended.extend(tick.reshaped);
    }
    // Not before the merge has had its lease.
    assert!(cluster.now > asked + LEASE);
    assert_eq!(cluster.owner(1).as_deref(), Some("b"));
    assert!(cluster.epoch(1) > as_epoch);
    let [told] = ended.as_slice() else {
        panic!("whoever asked is told once: {ended:?}");
    };
    assert!(told.outcome.is_err(), "{told:?}");
    assert_eq!(told.asked, merge_ended(0, 1, Ok(0)).asked);
}

/// The old owner's word of the release may come twice (it says so again when orders
/// still name what it released). The second must not free the region for anybody: it
/// has a higher epoch by then, with which the survivor's worker opens it.
#[test]
fn a_second_word_of_the_release_does_not_free_the_region_that_waits_to_be_absorbed() {
    let mut cluster = three_workers();
    let absorbeds = cluster.epoch(1);
    cluster.merge_to_absorb(0, 1);
    let before = cluster.clone();
    let again = cluster
        .coordinator
        .released(cluster.now, "b", region(1), absorbeds);
    assert_eq!(again.orders, []);
    assert_eq!(again.workers, [] as [String; 0]);
    assert_eq!(cluster.waiting(), [1]);
    assert_eq!(cluster.table(), before.table());
    assert_eq!(cluster.run(LEASE / 2).workers, [] as [String; 0]);
    assert_eq!(cluster.waiting(), [1]);
}

/// Only what is reserved waits for the reservation: the other regions of a worker
/// that is told to stop are handed over as ever.
#[test]
fn the_other_regions_of_a_leaving_worker_are_released_while_its_reserved_one_waits() {
    let mut cluster = Cluster::settled(&[0, 4, 8], &["a", "b", "c"]);
    cluster.listed(&list(&[0, 1, 2, 3], &[], 4));
    assert_eq!(cluster.runs("a"), [0, 3]);
    cluster.merge_to_absorb(0, 1);

    let mut changes = cluster.coordinator.leaving(cluster.now, "a");
    add(&mut changes, cluster.tick());
    assert_eq!(releases_of(&changes), [("a".to_owned(), 3)]);
    assert_eq!(changes.gone, [] as [String; 0]);

    // Having let go of that one, it still has the survivor, and is not let go itself.
    let epoch = cluster.epoch(3);
    let changes = cluster
        .coordinator
        .released(cluster.now, "a", region(3), epoch);
    assert_eq!(changes.gone, [] as [String; 0]);
    assert_eq!(cluster.runs("a"), [0]);
    assert_eq!(releases_of(&cluster.run(LEASE / 2)), []);
}

/// The coordinator's interface has one refusal more than the record: no epoch is
/// left to issue. It changes nothing either.
#[test]
fn a_merge_and_a_split_are_refused_when_epochs_have_run_out() {
    let mut cluster = Cluster::anew(&[0, 4]);
    cluster.coordinator = Coordinator::new(config(&[0, 4], LEASE), cluster.now, u64::MAX);
    cluster.register("a", &[held(0, 10)]);
    cluster.register("b", &[held(1, 11)]);
    cluster.register("c", &[held(2, 12)]);
    cluster.listed(&three_stripes());
    assert_merge_refused(&mut cluster, 0, 1, ReshapeRefusal::NoEpoch);
    assert_split_refused(&mut cluster, 1, ReshapeRefusal::NoEpoch);
}

/// The store can have seen a higher epoch of the region to absorb than the coordinator
/// has issued (epochs come from the clock a coordinator starts with). The survivor's
/// worker is refused then and says both: that the store has seen that epoch, and that
/// the merge is off (section 4). The first word ends like a tick, and must not give
/// the region away; the reading after the second does, above what the store has seen.
#[test]
fn the_store_refusing_the_epoch_to_absorb_with_leaves_the_region_alone_until_the_list_is_read() {
    let mut cluster = three_workers();
    let as_epoch = cluster.merge_to_absorb(0, 1);
    let seen = as_epoch + 50;

    let refused = cluster
        .coordinator
        .epoch_refused(cluster.now, "a", region(1), seen);
    assert_eq!(refused.workers, [] as [String; 0]);
    assert_eq!(refused.reshaped, []);
    assert_eq!(cluster.waiting(), [1]);
    assert_eq!(cluster.owner(0).as_deref(), Some("a"));
    assert_eq!(cluster.run(LEASE / 5).workers, [] as [String; 0]);

    assert!(cluster.absorb_ended(0, 1, Err(Off::Refused)).read);
    let ended = cluster.listed(&three_stripes());
    assert_eq!(
        ended.reshaped,
        [merge_ended(0, 1, Err(Undone::Off(Off::Refused)))]
    );
    assert_eq!(cluster.owner(1).as_deref(), Some("b"));
    assert!(cluster.epoch(1) > seen);
}

// ---------------------------------------------------------------------------------
// Nobody else is blamed either.
// ---------------------------------------------------------------------------------

/// A coordinator past its grace period whose workers reported what they run: `a` the
/// home region, `b` regions 1 and 3, `c` regions 2 and 4. When `b` has released
/// region 1, it and `a` have one region each, and of the two `a` has waited longer,
/// as `b` went behind the others when it let go. So a region that is given out then
/// goes to `a`, unless `a` is passed over for having failed one.
fn two_workers_with_as_many_regions() -> Cluster {
    let mut cluster = Cluster::reported(
        &[0, 4, 8, 12],
        &[("a", &[0]), ("b", &[1, 3]), ("c", &[2, 4])],
    );
    cluster.listed(&list(&[0, 1, 2, 3, 4], &[], 5));
    cluster.run(LEASE + MOMENT);
    cluster
}

/// A worker is at fault for a region that was taken from it because it did not vouch
/// for it, or for a release it left unanswered (ADR-0009, section 7). The survivor's
/// worker of a merge that came to nothing did neither.
#[test]
fn the_survivors_worker_is_not_blamed_for_a_merge_that_its_worker_called_off() {
    let mut cluster = two_workers_with_as_many_regions();
    cluster.merge_to_absorb(0, 1);
    cluster.absorb_ended(0, 1, Err(Off::Unreadable));
    let ended = cluster.listed(&list(&[0, 1, 2, 3, 4], &[], 5));
    assert_eq!(
        ended.reshaped,
        [merge_ended(0, 1, Err(Undone::Off(Off::Unreadable)))]
    );
    assert_eq!(cluster.owner(1).as_deref(), Some("a"));
}

#[test]
fn the_survivors_worker_is_not_blamed_for_an_absorb_that_it_left_unanswered() {
    for read in [true, false] {
        let mut cluster = two_workers_with_as_many_regions();
        cluster.merge_to_absorb(0, 1);
        assert!(cluster.run(LEASE + MOMENT).read);
        let ended = if read {
            cluster.listed(&list(&[0, 1, 2, 3, 4], &[], 5))
        } else {
            cluster.coordinator.unlisted(cluster.now)
        };
        let why = if read {
            Undone::Overdue
        } else {
            Undone::Unread
        };
        assert_eq!(ended.reshaped, [merge_ended(0, 1, Err(why))]);
        assert_eq!(cluster.owner(1).as_deref(), Some("a"));
    }
}

/// Nor is the worker of a split that was overdue: no region was taken from it. It has
/// the fewest regions here, and the part that the list shows goes to it.
#[test]
fn the_worker_of_a_split_that_was_overdue_is_not_passed_over() {
    let mut cluster = Cluster::reported(
        &[0, 4, 8, 12],
        &[("a", &[0, 3]), ("b", &[1]), ("c", &[2, 4])],
    );
    cluster.listed(&list(&[0, 1, 2, 3, 4], &[], 5));
    cluster.run(LEASE + MOMENT);
    let (as_epoch, part) = cluster.split(1);
    let ended = cluster.run(LEASE + MOMENT);
    assert_eq!(ended.reshaped, [split_ended(1, Err(Undone::Overdue))]);
    cluster.listed_and_looked(&list(&[0, 1, 2, 3, 4, 5], &[], 6));
    assert_eq!(cluster.owner(part).as_deref(), Some("b"));
    assert!(cluster.epoch(part) > as_epoch);
}

// ---------------------------------------------------------------------------------
// A generated run: a cluster played against the coordinator from a seed.
// ---------------------------------------------------------------------------------

/// Numbers from a seed (SplitMix64), the same on every machine.
struct Dice(u64);

impl Dice {
    fn roll(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut mixed = self.0;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        mixed ^ (mixed >> 31)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.roll() % bound
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    fn pick<T: Clone>(&mut self, from: &[T]) -> Option<T> {
        if from.is_empty() {
            None
        } else {
            Some(from[self.below(from.len() as u64) as usize].clone())
        }
    }
}

/// What the played world store has of a living region.
#[derive(Debug, Clone)]
struct Lane {
    /// The highest epoch the region was opened with.
    epoch: u64,
    /// The worker that has it open with that epoch, if it has not closed it.
    owner: Option<String>,
}

/// What comes of opening a region at the played store.
enum Opened {
    Yes,
    /// The store has seen an owner with a higher epoch.
    Refused {
        seen: u64,
    },
    /// The region was absorbed by `into`.
    Absorbed {
        into: u32,
    },
}

/// The world store as far as the coordinator and its workers can tell: which regions
/// there are, the highest epoch each was opened with and by whom, and the merges and
/// splits it was handed. It fences by the epoch, as ADR-0008 has it.
#[derive(Debug, Clone, Default)]
struct Store {
    living: BTreeMap<u32, Lane>,
    absorbed: Vec<(u32, u32)>,
    next: u32,
    /// Each region a split made, with the region it was split off.
    parts: Vec<(u32, u32)>,
    /// Each region a split made, with the epoch that was ordered for it and its maker.
    made: BTreeMap<u32, (u64, String)>,
    /// Every epoch a region was opened with, with the worker that opened it.
    opened: BTreeMap<(u32, u64), String>,
}

impl Store {
    fn stripes(count: u32) -> Self {
        let never_opened = Lane {
            epoch: 0,
            owner: None,
        };
        Self {
            living: (0..count).map(|id| (id, never_opened.clone())).collect(),
            next: count,
            ..Self::default()
        }
    }

    /// A hello for the region. An error is the run's: the coordinator has given a
    /// worker a region that there never was, or one epoch of a region to two workers,
    /// which the store could not tell apart.
    fn open(&mut self, id: u32, epoch: u64, worker: &str) -> Result<Opened, String> {
        if let Some((_, into)) = self.absorbed.iter().find(|(gone, _)| *gone == id) {
            return Ok(Opened::Absorbed { into: *into });
        }
        let Some(lane) = self.living.get_mut(&id) else {
            return Err(format!("{worker} opens region {id}, which there never was"));
        };
        if epoch < lane.epoch {
            return Ok(Opened::Refused { seen: lane.epoch });
        }
        let first = self
            .opened
            .entry((id, epoch))
            .or_insert_with(|| worker.to_owned());
        if first != worker {
            return Err(format!(
                "region {id} is opened with the epoch {epoch} by {worker}, and was by {first}"
            ));
        }
        lane.epoch = epoch;
        lane.owner = Some(worker.to_owned());
        Ok(Opened::Yes)
    }

    /// Whether the worker's handle of the region is good: it opened it with that epoch
    /// and nobody has opened it since.
    fn holds(&self, id: u32, epoch: u64, worker: &str) -> bool {
        self.living
            .get(&id)
            .is_some_and(|lane| lane.epoch == epoch && lane.owner.as_deref() == Some(worker))
    }

    fn close(&mut self, id: u32, epoch: u64, worker: &str) {
        if self.holds(id, epoch, worker) {
            self.living
                .get_mut(&id)
                .expect("a region that is held lives")
                .owner = None;
        }
    }

    /// The store loses every handle of a worker whose process has ended.
    fn close_all(&mut self, worker: &str) {
        for lane in self.living.values_mut() {
            if lane.owner.as_deref() == Some(worker) {
                lane.owner = None;
            }
        }
    }

    /// `AbsorbCommit`, as far as it is declined for whose the regions are.
    fn absorb(
        &mut self,
        survivor: u32,
        absorbed: u32,
        as_epoch: u64,
        worker: &str,
    ) -> Result<(), Decline> {
        if absorbed == 0 {
            return Err(Decline::Home);
        }
        let Some(lane) = self.living.get(&absorbed) else {
            return Err(Decline::NoSuchRegion);
        };
        if absorbed == survivor {
            return Err(Decline::NoSuchRegion);
        }
        if lane.epoch != as_epoch || lane.owner.as_deref() != Some(worker) {
            return Err(Decline::NotOpened {
                epoch: lane.owner.as_ref().map(|_| lane.epoch),
            });
        }
        self.living.remove(&absorbed);
        self.absorbed.push((absorbed, survivor));
        Ok(())
    }

    /// `SplitCommit`: the new region has the next id, whichever was named first (a
    /// runner that is told `NotNext` tries again with the next), and is its maker's.
    fn split(&mut self, of: u32, as_epoch: u64, worker: &str) -> u32 {
        let part = self.next;
        self.next += 1;
        self.living.insert(
            part,
            Lane {
                epoch: as_epoch,
                owner: Some(worker.to_owned()),
            },
        );
        self.opened.insert((part, as_epoch), worker.to_owned());
        self.parts.push((part, of));
        self.made.insert(part, (as_epoch, worker.to_owned()));
        part
    }

    fn list(&self) -> RegionList {
        RegionList {
            home: region(0),
            regions: self
                .living
                .iter()
                .map(|(id, lane)| RegionInfo {
                    region: region(*id),
                    epoch: lane.epoch,
                    bounds: None,
                    pinned: Vec::new(),
                })
                .collect(),
            absorbed: pairs(&self.absorbed),
            next: region(self.next),
        }
    }
}

/// What the coordinator tells a worker.
#[derive(Debug, Clone)]
enum Told {
    Orders(Vec<Assignment>),
    Release(RegionId, u64),
    Reshape(Order),
}

/// What a worker tells the coordinator, besides its heartbeats.
#[derive(Debug, Clone)]
enum Said {
    Released(RegionId, u64),
    EpochRefused(RegionId, u64),
    AbsorbEnded(RegionId, RegionId, Result<(), Off>),
    SplitEnded(RegionId, u64, Result<RegionId, Off>),
    Leaving,
}

/// A worker process as section 4 of the record and ADR-0009 describe it, without
/// regions to tick: it opens what it is ordered to run, lets go of what it is asked
/// to, absorbs and splits at the played store, and says what came of each.
#[derive(Debug, Clone, Default)]
struct Process {
    alive: bool,
    /// Whether it has registered with the coordinator that is there and has its
    /// connection still.
    connected: bool,
    /// What it runs, each region under the assignment it holds it with.
    runs: BTreeMap<u32, Assignment>,
    /// The assignments it let go of, which it never takes up again.
    released: BTreeSet<(u32, u64)>,
    /// The parts it made that no orders have named yet, each with the region it was
    /// split off and the epoch that was ordered.
    parts: BTreeMap<u32, (u32, u64)>,
    /// Orders to absorb or to split that it has taken and not carried out: its store
    /// is slow.
    slow: VecDeque<Order>,
    /// Whether it was told to stop, which it says after every registration.
    leaving: bool,
    /// Whether it gets nothing made durable, and so vouches for nothing.
    stalled: bool,
}

/// What may go wrong in a run.
#[derive(Debug, Clone, Copy)]
struct Faults {
    /// Connections are lost with what was on them, workers are slow to do what they
    /// are told or cannot vouch for what they run, some are told to stop, and readings
    /// of the list fail.
    losses: bool,
    /// Workers die, in the middle of a merge or a split too, and come back or do not.
    deaths: bool,
    /// The coordinator is made anew.
    anew: bool,
}

/// A cluster played against a coordinator: workers, the world store, the connections
/// between them and the coordinator with what is on its way, and the service's part of
/// reading the list. After every call of the coordinator the run checks what must
/// hold at all times, and at the end what must hold when everything has settled.
struct Run {
    seed: u64,
    dice: Dice,
    faults: Faults,
    /// How far the time moves when it does: a quarter of a second, a second or a tenth,
    /// by the seed. With a second, a lease is soon over and much is overdue.
    pace: Duration,
    config: CoordinatorConfig,
    coordinator: Coordinator,
    started: Instant,
    now: Instant,
    store: Store,
    workers: BTreeMap<String, Process>,
    /// What is on its way on each worker's connection, in either direction.
    to_workers: BTreeMap<String, VecDeque<Told>>,
    to_coordinator: BTreeMap<String, VecDeque<Said>>,
    /// Whether the list is to be read, and the reading that is on its way back to the
    /// coordinator: the list, or nothing if it failed.
    wanted: bool,
    reading: Option<Option<RegionList>>,
    /// The next id of the list as it was last handed in.
    listed_next: Option<u32>,
    /// The merges and splits the coordinator has taken on and not ended.
    asked: BTreeMap<u64, Asked>,
    askers: u64,
    /// Who asked for the split that each epoch for a new region was ordered with.
    splitting: BTreeMap<u64, u64>,
    /// When a merge or a split last ended, as far as whoever asked was told.
    ended: Option<Instant>,
    /// Which worker each epoch of each region was ever assigned to.
    issued: BTreeMap<(u32, u64), String>,
    /// The last owner and epoch of each region under the coordinator that is there.
    last: BTreeMap<u32, (String, u64)>,
    /// The epoch each region was to be opened with to be absorbed: whoever is given
    /// the region afterwards has a higher one.
    fences: BTreeMap<u32, u64>,
    /// The orders to absorb that were given, to tell a repeated one from a new one.
    absorbs: BTreeSet<(u32, u32, u64)>,
    /// The highest epoch seen anywhere.
    highest: u64,
    table: Option<RoutingTable>,
    /// What happened last, for the message of a failed check.
    story: VecDeque<String>,
    /// How often each thing worth telling happened, to see what a run was about.
    seen: BTreeMap<String, u32>,
}

impl Run {
    const WORKERS: [&'static str; 4] = ["w0", "w1", "w2", "w3"];
    const BOUNDARIES: [i32; 4] = [0, 4, 8, 12];
    /// How far the time moves at a time while the cluster settles.
    const PACE: Duration = Duration::from_millis(250);

    fn new(seed: u64, faults: Faults) -> Self {
        let config = config(&Self::BOUNDARIES, LEASE);
        let now = Instant::now();
        let mut run = Self {
            seed,
            dice: Dice(seed),
            faults,
            pace: Duration::from_millis([250, 1_000, 100][(seed % 3) as usize]),
            coordinator: Coordinator::new(config.clone(), now, FIRST_EPOCH),
            config,
            started: now,
            now,
            store: Store::stripes(Self::BOUNDARIES.len() as u32 + 1),
            workers: BTreeMap::new(),
            to_workers: BTreeMap::new(),
            to_coordinator: BTreeMap::new(),
            wanted: true,
            reading: None,
            listed_next: None,
            asked: BTreeMap::new(),
            askers: 0,
            splitting: BTreeMap::new(),
            ended: None,
            issued: BTreeMap::new(),
            last: BTreeMap::new(),
            fences: BTreeMap::new(),
            absorbs: BTreeSet::new(),
            highest: FIRST_EPOCH,
            table: None,
            story: VecDeque::new(),
            seen: BTreeMap::new(),
        };
        for name in Self::WORKERS {
            let process = Process {
                alive: true,
                ..Process::default()
            };
            run.workers.insert(name.to_owned(), process);
            run.to_workers.insert(name.to_owned(), VecDeque::new());
            run.to_coordinator.insert(name.to_owned(), VecDeque::new());
        }
        run
    }

    fn note(&mut self, line: String) {
        if self.story.len() == 300 {
            self.story.pop_front();
        }
        let at = self.now.duration_since(self.started).as_secs_f64();
        self.story.push_back(format!("{at:>8.3}s  {line}"));
    }

    fn count(&mut self, what: impl Into<String>) {
        *self.seen.entry(what.into()).or_default() += 1;
    }

    /// A check of the run has failed: the test fails with the seed and what led there.
    fn fail(&self, what: String) -> ! {
        let story: Vec<&str> = self.story.iter().map(String::as_str).collect();
        panic!(
            "seed {}: {what}\n\nthe store: {:?}\n\nwhat led there:\n{}\n\nseed {}: {what}",
            self.seed,
            self.store,
            story.join("\n"),
            self.seed,
        );
    }

    fn ensure(&self, holds: bool, what: impl FnOnce() -> String) {
        if !holds {
            self.fail(what());
        }
    }

    fn names() -> Vec<String> {
        Self::WORKERS
            .iter()
            .map(|name| (*name).to_owned())
            .collect()
    }

    fn process(&mut self, name: &str) -> &mut Process {
        self.workers.get_mut(name).expect("a worker of the run")
    }

    fn connected(&self, name: &str) -> bool {
        self.workers[name].alive && self.workers[name].connected
    }

    /// The coordinator's word for a worker goes onto its connection, if it has one.
    fn tell(&mut self, name: &str, told: Told) {
        if self.connected(name) {
            self.to_workers
                .get_mut(name)
                .expect("a worker of the run")
                .push_back(told);
        } else {
            self.note(format!("    lost, as {name} has no connection: {told:?}"));
        }
    }

    /// A worker's word goes onto its connection, if it has one.
    fn say(&mut self, name: &str, said: Said) {
        if self.connected(name) {
            self.to_coordinator
                .get_mut(name)
                .expect("a worker of the run")
                .push_back(said);
        } else {
            self.note(format!("    {name} has no connection to say {said:?}"));
        }
    }

    /// What a call of the coordinator changed is passed on as the service does, and
    /// what must always hold is checked.
    fn apply(&mut self, what: String, changes: Changes) {
        self.note(format!("{what} -> {changes:?}"));
        if changes.read {
            // A reading that was asked for before this call does not do.
            self.reading = None;
            self.wanted = true;
        }
        for name in &changes.workers {
            let orders = self.coordinator.assignments(name);
            self.tell(name, Told::Orders(orders));
        }
        for release in &changes.releases {
            let owned = self
                .coordinator
                .assignments(&release.worker)
                .iter()
                .any(|held| held.region == release.region && held.epoch == release.epoch);
            self.ensure(owned, || format!("{release:?} is not of its owner"));
            self.tell(
                &release.worker,
                Told::Release(release.region, release.epoch),
            );
        }
        for order in &changes.orders {
            self.check_order(order);
            self.tell(&order.worker, Told::Reshape(order.order.clone()));
        }
        for ended in &changes.reshaped {
            self.check_ended(ended);
        }
        for name in &changes.gone {
            self.exit(name);
        }
        self.check(&what, &changes);
    }

    fn check_order(&mut self, order: &ReshapeOrder) {
        let runs = |region: RegionId, epoch: u64| {
            self.coordinator
                .assignments(&order.worker)
                .iter()
                .any(|held| held.region == region && held.epoch == epoch)
        };
        match &order.order {
            Order::Prepare { region, epoch } => {
                self.ensure(runs(*region, *epoch), || {
                    format!("{order:?} is not of its owner")
                });
            }
            Order::Absorb {
                region,
                epoch,
                absorbed,
                as_epoch,
            } => {
                self.ensure(runs(*region, *epoch), || {
                    format!("{order:?} is not of its owner")
                });
                self.ensure(self.coordinator.waiting().contains(absorbed), || {
                    format!("{order:?}: the region to absorb is somebody's, or unknown")
                });
                if self.absorbs.insert((region.0, absorbed.0, *as_epoch)) {
                    self.ensure(*as_epoch > self.highest, || {
                        format!("{order:?}: the epoch is not above {}", self.highest)
                    });
                    self.fences.insert(absorbed.0, *as_epoch);
                    // It was released first: the owner it was taken from does not
                    // have it open. (An owner from before that one may, if it was
                    // taken for dead and lives; the new epoch fences it.)
                    let open = self
                        .last
                        .get(&absorbed.0)
                        .is_some_and(|(owner, epoch)| self.store.holds(absorbed.0, *epoch, owner));
                    self.ensure(!open, || {
                        format!("{order:?}: the region to absorb is open with its owner")
                    });
                }
            }
            Order::SplitOff {
                region,
                epoch,
                as_epoch,
                part,
                ..
            } => {
                self.ensure(runs(*region, *epoch), || {
                    format!("{order:?} is not of its owner")
                });
                self.ensure(*as_epoch > self.highest, || {
                    format!("{order:?}: the epoch is not above {}", self.highest)
                });
                self.ensure(Some(part.0) == self.listed_next, || {
                    format!("{order:?}: the list's next id is {:?}", self.listed_next)
                });
            }
        }
    }

    /// Whoever asked is told once, and is told that it was done only if the store has
    /// it: the list decides.
    fn check_ended(&mut self, ended: &Reshaped) {
        let asker = ended
            .asker
            .expect("every merge and split of a run has an asker");
        let asked = self.asked.remove(&asker);
        self.ended = Some(self.now);
        self.ensure(asked == Some(ended.asked), || {
            format!("{ended:?} is told, and {asked:?} was asked and not yet told")
        });
        match (ended.asked, ended.outcome) {
            (Asked::Merge { survivor, absorbed }, Ok(into)) => {
                self.ensure(
                    into == survivor && self.store.absorbed.contains(&(absorbed.0, survivor.0)),
                    || format!("{ended:?}, and the store has no such pair"),
                );
                self.count("merges done");
            }
            (Asked::Split { region }, Ok(part)) => {
                self.ensure(self.store.parts.contains(&(part.0, region.0)), || {
                    format!("{ended:?}, and the store made no such region")
                });
                self.count("splits done");
            }
            (Asked::Merge { .. }, Err(why)) => self.count(format!("merges undone: {}", kind(why))),
            (Asked::Split { .. }, Err(why)) => self.count(format!("splits undone: {}", kind(why))),
        }
    }

    /// What holds after every call: a region is one worker's, an epoch of a region was
    /// only ever one worker's, epochs only rise, the routing table says what the
    /// workers are told, and its version rises with every change of it.
    fn check(&mut self, what: &str, changes: &Changes) {
        let table = self.coordinator.routing_table();
        let mut owners: BTreeMap<u32, (String, u64)> = BTreeMap::new();
        for name in Self::names() {
            for held in self.coordinator.assignments(&name) {
                let id = held.region.0;
                if let Some((other, _)) = owners.insert(id, (name.clone(), held.epoch)) {
                    self.fail(format!("{what}: region {id} is {other}'s and {name}'s"));
                }
                let first = self
                    .issued
                    .entry((id, held.epoch))
                    .or_insert_with(|| name.clone())
                    .clone();
                self.ensure(first == name, || {
                    format!(
                        "{what}: region {id} is {name}'s with the epoch {}, which was {first}'s",
                        held.epoch
                    )
                });
            }
        }
        let routed: BTreeMap<u32, (String, u64)> = table
            .routes
            .iter()
            .map(|route| {
                let name = route.address.trim_end_matches(":25600").to_owned();
                (route.region.0, (name, route.epoch))
            })
            .collect();
        self.ensure(
            routed == owners && routed.len() == table.routes.len(),
            || format!("{what}: the routes {routed:?} are not the assignments {owners:?}"),
        );

        for (id, (name, epoch)) in &owners {
            if let Some((before, last)) = self.last.get(id) {
                self.ensure(epoch >= last && (name == before || epoch > last), || {
                    format!(
                        "{what}: region {id} was {before}'s with {last}, and is {name}'s with \
                         {epoch}"
                    )
                });
            }
            if let Some(fence) = self.fences.get(id) {
                self.ensure(epoch > fence, || {
                    format!(
                        "{what}: region {id} is {name}'s with {epoch}, not above the epoch \
                         {fence} that it was to be absorbed with"
                    )
                });
            }
            self.highest = self.highest.max(*epoch);
        }
        for order in &changes.orders {
            if let Order::Absorb { as_epoch, .. } | Order::SplitOff { as_epoch, .. } = &order.order
            {
                self.highest = self.highest.max(*as_epoch);
            }
        }
        self.last.extend(owners.clone());

        let waiting = self.coordinator.waiting();
        self.ensure(
            table.waiting as usize == waiting.len()
                && waiting.iter().all(|region| !owners.contains_key(&region.0)),
            || {
                format!(
                    "{what}: {} regions wait by the table, and those without an owner are \
                     {waiting:?}",
                    table.waiting
                )
            },
        );
        self.ensure(table.is_complete() == waiting.is_empty(), || {
            format!("{what}: the table is complete with {waiting:?} waiting, or the reverse")
        });

        // While a split is reserved, its part is its maker's or unknown: a reading
        // that shows it must not have it given to another worker (section 5.2).
        for (part, (as_epoch, maker)) in &self.store.made.clone() {
            let reserved = self
                .splitting
                .get(as_epoch)
                .is_some_and(|asker| self.asked.contains_key(asker));
            if !reserved {
                continue;
            }
            let owner = owners.get(part);
            if what.starts_with("the list is") {
                self.count("readings that showed the part of a reserved split");
            }
            self.ensure(
                owner.is_none_or(|(name, epoch)| name == maker && epoch == as_epoch)
                    && !waiting.contains(&region(*part)),
                || {
                    format!(
                        "{what}: the part {part} of a split that is reserved still, made by \
                         {maker} with {as_epoch}, is {owner:?}'s, or waits for an owner"
                    )
                },
            );
        }

        if let Some(before) = &self.table {
            let mut same = table.clone();
            same.version = before.version;
            if same == *before {
                self.ensure(table.version == before.version && !changes.routing, || {
                    format!("{what}: the table did not change, and is said to have")
                });
            } else {
                self.ensure(table.version > before.version && changes.routing, || {
                    format!(
                        "{what}: the table changed from {before:?} to {table:?} and its version \
                         did not rise, or nobody is told"
                    )
                });
            }
        }
        self.table = Some(table);
    }

    // What the workers do.

    /// The worker takes the next thing the coordinator told it.
    fn deliver(&mut self, name: &str) {
        let Some(told) = self
            .to_workers
            .get_mut(name)
            .expect("a worker of the run")
            .pop_front()
        else {
            return;
        };
        self.note(format!("  {name} is told {told:?}"));
        match told {
            Told::Orders(orders) => self.take_orders(name, &orders),
            Told::Release(region, epoch) => self.let_go(name, region, epoch),
            Told::Reshape(Order::Prepare { .. }) => {}
            Told::Reshape(order) => {
                if self.faults.losses && self.dice.chance(20) {
                    self.note(format!("  {name}'s store is slow"));
                    self.process(name).slow.push_back(order);
                } else {
                    self.reshape(name, order);
                }
            }
        }
    }

    /// The worker drops what its orders no longer name and opens what is new to it.
    /// Assignments are told apart by region and epoch (section 4).
    fn take_orders(&mut self, name: &str, orders: &[Assignment]) {
        for order in orders {
            let id = order.region.0;
            let Some((_, as_epoch)) = self.workers[name].parts.get(&id).copied() else {
                continue;
            };
            self.process(name).parts.remove(&id);
            if order.epoch != as_epoch {
                // A new assignment of the part, which the coordinator found in the
                // list: the part in memory is dropped, and the region opened below.
                self.process(name).runs.remove(&id);
                self.store.close(id, as_epoch, name);
            }
        }
        for (id, held) in self.workers[name].runs.clone() {
            let ordered = orders
                .iter()
                .any(|order| order.region.0 == id && order.epoch == held.epoch);
            // A part is not dropped for being absent until orders have named it once.
            if !ordered && !self.workers[name].parts.contains_key(&id) {
                self.process(name).runs.remove(&id);
                self.store.close(id, held.epoch, name);
            }
        }
        for order in orders {
            let (id, epoch) = (order.region.0, order.epoch);
            if self.workers[name]
                .runs
                .get(&id)
                .is_some_and(|held| held.epoch == epoch)
            {
                self.process(name).runs.insert(id, *order);
                continue;
            }
            if self.workers[name].released.contains(&(id, epoch)) {
                // Orders that still name what it released: it says so again.
                self.say(name, Said::Released(order.region, epoch));
                continue;
            }
            match self.store.open(id, epoch, name) {
                Ok(Opened::Yes) => {
                    self.process(name).runs.insert(id, *order);
                }
                Ok(Opened::Refused { seen }) => {
                    self.say(name, Said::EpochRefused(order.region, seen));
                }
                Ok(Opened::Absorbed { into }) => {
                    self.say(name, Said::AbsorbEnded(region(into), order.region, Ok(())));
                }
                Err(wrong) => self.fail(wrong),
            }
        }
    }

    /// The worker lets go of the region if it holds it with that epoch, and says that
    /// it has either way (ADR-0009, section 1, step 3).
    fn let_go(&mut self, name: &str, region: RegionId, epoch: u64) {
        let id = region.0;
        if self.workers[name]
            .runs
            .get(&id)
            .is_some_and(|held| held.epoch == epoch)
        {
            self.process(name).runs.remove(&id);
            self.process(name).parts.remove(&id);
            self.store.close(id, epoch, name);
        }
        self.process(name).released.insert((id, epoch));
        self.say(name, Said::Released(region, epoch));
    }

    /// The worker carries out an order to absorb or to split (section 4).
    fn reshape(&mut self, name: &str, order: Order) {
        self.note(format!("  {name} sets about {order:?}"));
        match order {
            Order::Prepare { .. } => {}
            Order::Absorb {
                region: into,
                epoch,
                absorbed,
                as_epoch,
            } => {
                let outcome = self.absorb(name, into.0, epoch, absorbed, as_epoch);
                if let Some(outcome) = outcome {
                    self.say(name, Said::AbsorbEnded(into, absorbed, outcome));
                }
            }
            Order::SplitOff {
                region: of,
                epoch,
                as_epoch,
                ..
            } => {
                let outcome = self.split_off(name, of.0, epoch, as_epoch);
                if let Some(outcome) = outcome {
                    self.say(name, Said::SplitEnded(of, as_epoch, outcome.map(region)));
                }
            }
        }
    }

    /// Whether the worker runs the region with that epoch and its handle is good.
    fn runs(&self, name: &str, id: u32, epoch: u64) -> Result<(), Off> {
        if !self.workers[name]
            .runs
            .get(&id)
            .is_some_and(|held| held.epoch == epoch)
        {
            Err(Off::NotRunning)
        } else if !self.store.holds(id, epoch, name) {
            Err(Off::StoreLost)
        } else {
            Ok(())
        }
    }

    /// What the worker says of an absorb; nothing, if it died of it.
    fn absorb(
        &mut self,
        name: &str,
        into: u32,
        epoch: u64,
        absorbed: RegionId,
        as_epoch: u64,
    ) -> Option<Result<(), Off>> {
        if let Err(off) = self.runs(name, into, epoch) {
            return Some(Err(off));
        }
        match self.store.open(absorbed.0, as_epoch, name) {
            // The order came twice, and the merge has happened already.
            Ok(Opened::Absorbed { into: went }) if went == into => Some(Ok(())),
            Ok(Opened::Absorbed { .. }) => Some(Err(Off::Unreadable)),
            Ok(Opened::Refused { seen }) => {
                self.say(name, Said::EpochRefused(absorbed, seen));
                Some(Err(Off::Refused))
            }
            Ok(Opened::Yes) => {
                if self.dice.chance(10) {
                    self.store.close(absorbed.0, as_epoch, name);
                    return Some(Err(Off::Unreadable));
                }
                if self.faults.deaths && self.dice.chance(8) {
                    self.kill(name, "before the record of its merge");
                    return None;
                }
                match self.store.absorb(into, absorbed.0, as_epoch, name) {
                    Ok(()) => {
                        if self.faults.deaths && self.dice.chance(10) {
                            self.kill(name, "after the record of its merge");
                            return None;
                        }
                        Some(Ok(()))
                    }
                    Err(decline) => {
                        self.store.close(absorbed.0, as_epoch, name);
                        Some(Err(Off::Declined(decline)))
                    }
                }
            }
            Err(wrong) => self.fail(wrong),
        }
    }

    /// What the worker says of a split; nothing, if it died of it.
    fn split_off(
        &mut self,
        name: &str,
        of: u32,
        epoch: u64,
        as_epoch: u64,
    ) -> Option<Result<u32, Off>> {
        if let Err(off) = self.runs(name, of, epoch) {
            return Some(Err(off));
        }
        if self.dice.chance(25) {
            return Some(Err(Off::Nobody));
        }
        if self.faults.deaths && self.dice.chance(5) {
            self.kill(name, "before the record of its split");
            return None;
        }
        let part = self.store.split(of, as_epoch, name);
        let mut held = self.workers[name].runs[&of];
        held.region = region(part);
        held.epoch = as_epoch;
        self.process(name).runs.insert(part, held);
        self.process(name).parts.insert(part, (of, as_epoch));
        if self.faults.deaths && self.dice.chance(10) {
            self.kill(name, "after the record of its split");
            return None;
        }
        Some(Ok(part))
    }

    /// The worker finds a handle of its lost, because another worker opened the
    /// region, and tries to open it again: it is refused, drops the region and says so.
    fn find_fenced(&mut self, name: &str) {
        for (id, held) in self.workers[name].runs.clone() {
            if self.store.holds(id, held.epoch, name) {
                continue;
            }
            match self.store.open(id, held.epoch, name) {
                Ok(Opened::Yes) => {}
                Ok(Opened::Refused { seen }) => {
                    self.note(format!("  {name} finds region {id} fenced"));
                    self.process(name).runs.remove(&id);
                    self.process(name).parts.remove(&id);
                    self.say(name, Said::EpochRefused(held.region, seen));
                }
                Ok(Opened::Absorbed { into }) => {
                    self.note(format!("  {name} finds region {id} absorbed"));
                    self.process(name).runs.remove(&id);
                    self.process(name).parts.remove(&id);
                    self.say(name, Said::AbsorbEnded(region(into), held.region, Ok(())));
                }
                Err(wrong) => self.fail(wrong),
            }
        }
    }

    /// The worker's process ends: the store loses its handles, and the service sees
    /// its connection end.
    fn kill(&mut self, name: &str, when: &str) {
        self.note(format!("{name} dies {when}"));
        self.count(format!("workers that died {when}"));
        self.store.close_all(name);
        let had_connection = self.connected(name);
        *self.process(name) = Process::default();
        self.drop_connection(name, had_connection);
    }

    /// The coordinator has closed the connection of a worker that said it is leaving:
    /// the worker stops, closing what it may still have open.
    fn exit(&mut self, name: &str) {
        self.note(format!("{name} may go, and does"));
        self.store.close_all(name);
        *self.process(name) = Process::default();
        self.to_workers.get_mut(name).expect("a worker").clear();
        self.to_coordinator.get_mut(name).expect("a worker").clear();
    }

    /// The worker's connection ends, with everything that was on it. The worker goes
    /// on running what it runs, and registers again when it gets to it. Half the time
    /// the service has not seen the connection end by then: the coordinator takes the
    /// worker to have one, and what it tells it meanwhile is lost.
    fn cut(&mut self, name: &str) {
        if self.connected(name) {
            let seen = self.dice.chance(50);
            self.note(format!(
                "{name} loses its connection; the service sees it: {seen}"
            ));
            self.drop_connection(name, seen);
        }
    }

    fn drop_connection(&mut self, name: &str, known: bool) {
        self.process(name).connected = false;
        self.to_workers.get_mut(name).expect("a worker").clear();
        self.to_coordinator.get_mut(name).expect("a worker").clear();
        if known {
            let changes = self.coordinator.disconnected(self.now, name);
            self.apply(format!("{name}'s connection has ended"), changes);
        }
    }

    /// The worker registers with what it runs. The service reads the list then.
    fn register(&mut self, name: &str) {
        let holding: Vec<Assignment> = self.workers[name].runs.values().copied().collect();
        let fingerprint = self.config.layout.fingerprint();
        let registered =
            self.coordinator
                .register(self.now, name, &address(name), &holding, Some(fingerprint));
        let changes = match registered {
            Ok(changes) => changes,
            Err(refusal) => self.fail(format!("{name} is refused: {refusal}")),
        };
        self.process(name).connected = true;
        // Its first answer is its orders, whether or not they changed.
        if !changes.workers.iter().any(|changed| changed == name) {
            let orders = self.coordinator.assignments(name);
            self.tell(name, Told::Orders(orders));
        }
        self.apply(format!("{name} registers with {holding:?}"), changes);
        // A part that no orders have named yet is said again after every registration.
        for (part, (of, as_epoch)) in self.workers[name].parts.clone() {
            self.say(
                name,
                Said::SplitEnded(region(of), as_epoch, Ok(region(part))),
            );
        }
        // And so does a worker that is still to stop.
        if self.workers[name].leaving {
            self.say(name, Said::Leaving);
        }
        self.wanted = true;
    }

    /// The coordinator takes the next thing the worker said.
    fn hear(&mut self, name: &str) {
        let Some(said) = self
            .to_coordinator
            .get_mut(name)
            .expect("a worker of the run")
            .pop_front()
        else {
            return;
        };
        let now = self.now;
        let changes = match said.clone() {
            Said::Released(region, epoch) => self.coordinator.released(now, name, region, epoch),
            Said::EpochRefused(region, seen) => {
                self.highest = self.highest.max(seen);
                self.coordinator.epoch_refused(now, name, region, seen)
            }
            Said::AbsorbEnded(region, absorbed, outcome) => self
                .coordinator
                .absorb_ended(now, name, region, absorbed, outcome),
            Said::SplitEnded(region, as_epoch, outcome) => self
                .coordinator
                .split_ended(now, name, region, as_epoch, outcome),
            Said::Leaving => self.coordinator.leaving(now, name),
        };
        self.apply(format!("{name} says {said:?}"), changes);
    }

    // What the service does.

    /// The list is read, if it is to be: the reading is on its way from then on.
    fn read(&mut self) {
        if self.wanted && self.reading.is_none() {
            self.wanted = false;
            let failed = self.faults.losses && self.dice.chance(20);
            self.reading = Some((!failed).then(|| self.store.list()));
        }
    }

    /// The reading that is on its way arrives.
    fn hand_in(&mut self) {
        match self.reading.take() {
            Some(Some(list)) => {
                self.listed_next = Some(list.next.0);
                let changes = self.coordinator.listed(self.now, &list);
                self.apply(format!("the list is {list:?}"), changes);
            }
            Some(None) => {
                let changes = self.coordinator.unlisted(self.now);
                self.apply("the list cannot be read".to_owned(), changes);
            }
            None => {}
        }
    }

    /// The reading before a merge or a split is looked at, which the request waits
    /// for. Returns whether it could be read.
    fn read_for_a_request(&mut self) -> bool {
        self.reading = None;
        self.wanted = true;
        self.read();
        let read = matches!(self.reading, Some(Some(_)));
        self.hand_in();
        read
    }

    fn known(&self) -> Vec<RegionId> {
        let table = self.coordinator.routing_table();
        let mut known: Vec<RegionId> = table.routes.iter().map(|route| route.region).collect();
        known.extend(self.coordinator.waiting());
        known
    }

    fn ask_for_a_merge(&mut self) {
        let known = self.known();
        let (Some(survivor), Some(absorbed)) = (self.dice.pick(&known), self.dice.pick(&known))
        else {
            return;
        };
        if !self.read_for_a_request() {
            return;
        }
        self.askers += 1;
        let asker = self.askers;
        match self
            .coordinator
            .merge(self.now, survivor, absorbed, Some(asker))
        {
            Ok(changes) => {
                self.ensure(survivor != absorbed && absorbed != region(0), || {
                    format!("a merge of {absorbed} into {survivor} is taken on")
                });
                self.asked
                    .insert(asker, Asked::Merge { survivor, absorbed });
                self.apply(
                    format!("{asker} asks to merge {absorbed} into {survivor}"),
                    changes,
                );
            }
            Err(refusal) => {
                self.note(format!(
                    "a merge of {absorbed} into {survivor} is refused: {refusal}"
                ));
                self.apply("a merge is refused".to_owned(), Changes::default());
            }
        }
    }

    fn ask_for_a_split(&mut self) {
        let known = self.known();
        let Some(of) = self.dice.pick(&known) else {
            return;
        };
        if !self.read_for_a_request() {
            return;
        }
        self.askers += 1;
        let asker = self.askers;
        match self.coordinator.split(self.now, of, &chunks(), Some(asker)) {
            Ok(changes) => {
                self.asked.insert(asker, Asked::Split { region: of });
                for order in &changes.orders {
                    if let Order::SplitOff { as_epoch, .. } = &order.order {
                        self.splitting.insert(*as_epoch, asker);
                    }
                }
                self.apply(format!("{asker} asks to split {of}"), changes);
            }
            Err(refusal) => {
                self.note(format!("a split of {of} is refused: {refusal}"));
                self.apply("a split is refused".to_owned(), Changes::default());
            }
        }
    }

    fn ask_for_a_move(&mut self) {
        let known = self.known();
        let Some(moved) = self.dice.pick(&known) else {
            return;
        };
        match self.coordinator.move_region(self.now, moved, None, 0) {
            Ok((begun, changes)) => {
                self.apply(format!("region {moved} is moved: {begun:?}"), changes)
            }
            Err(refusal) => {
                self.note(format!("a move of {moved} is refused: {refusal}"));
                self.apply("a move is refused".to_owned(), Changes::default());
            }
        }
    }

    /// Time passes: the workers look after their handles and say that they are there,
    /// and the coordinator looks at its leases.
    fn pass(&mut self, time: Duration) {
        self.now += time;
        for name in Self::names() {
            if !self.workers[&name].alive {
                continue;
            }
            self.find_fenced(&name);
            if !self.workers[&name].connected {
                continue;
            }
            let vouched: Vec<(RegionId, Vouch)> = self.workers[&name]
                .runs
                .values()
                .filter(|_| !self.workers[&name].stalled)
                .map(|held| (held.region, Vouch::Committed))
                .collect();
            if !self.coordinator.heartbeat(self.now, &name, &vouched) {
                // The coordinator does not know it: it has to register again.
                self.note(format!("{name} is not known to the coordinator"));
                self.drop_connection(&name, false);
            }
        }
        let changes = self.coordinator.tick(self.now);
        // A release that a tick begins of a worker that is not leaving is to even the
        // regions out: not while a merge or a split is under way, nor within a lease of
        // one having ended (section 5.5).
        for release in &changes.releases {
            if self.workers[&release.worker].leaving {
                continue;
            }
            self.count("releases to even out");
            let since = self.ended.map(|ended| self.now - ended);
            self.ensure(
                self.asked.is_empty() && since.is_none_or(|since| since >= LEASE),
                || {
                    format!(
                        "{release:?} is to even out, with {:?} under way and the last of \
                         them ended {since:?} ago",
                        self.asked
                    )
                },
            );
        }
        if changes != Changes::default() {
            self.apply("a tick".to_owned(), changes);
        } else {
            self.check("a tick", &changes);
        }
    }

    /// A coordinator that knows nothing takes the place of the one that was. Every
    /// connection is lost with it, and whoever asked for something hears no more.
    fn replace_the_coordinator(&mut self) {
        self.note("the coordinator is made anew".to_owned());
        self.count(if self.asked.is_empty() {
            "coordinators made anew"
        } else {
            "coordinators made anew in the middle of a merge or a split"
        });
        self.coordinator = Coordinator::new(self.config.clone(), self.now, self.highest + 1_000);
        self.highest += 1_000;
        for name in Self::names() {
            self.process(&name).connected = false;
            self.to_workers.get_mut(&name).expect("a worker").clear();
            self.to_coordinator
                .get_mut(&name)
                .expect("a worker")
                .clear();
        }
        self.asked.clear();
        self.last.clear();
        self.table = None;
        self.reading = None;
        self.wanted = true;
        self.listed_next = None;
    }

    /// One thing happens, chosen by the dice among what can.
    fn step(&mut self) {
        let names = Self::names();
        let name = self.dice.pick(&names).expect("there are workers");
        let alive = self.workers[&name].alive;
        match self.dice.below(100) {
            0..=27 => {
                let waiting: Vec<String> = names
                    .iter()
                    .filter(|name| !self.to_workers[*name].is_empty())
                    .cloned()
                    .collect();
                if let Some(name) = self.dice.pick(&waiting) {
                    self.deliver(&name);
                }
            }
            28..=55 => {
                let waiting: Vec<String> = names
                    .iter()
                    .filter(|name| !self.to_coordinator[*name].is_empty())
                    .cloned()
                    .collect();
                if let Some(name) = self.dice.pick(&waiting) {
                    self.hear(&name);
                }
            }
            56..=67 => self.pass(self.pace),
            68..=72 => self.read(),
            73..=77 => self.hand_in(),
            78..=80 => {
                if let Some(order) = self.process(&name).slow.pop_front() {
                    self.reshape(&name, order);
                }
            }
            81..=85 => {
                if alive && !self.workers[&name].connected {
                    self.register(&name);
                }
            }
            86..=88 => self.ask_for_a_merge(),
            89..=91 => self.ask_for_a_split(),
            92 => self.ask_for_a_move(),
            93 if self.faults.losses && alive => self.cut(&name),
            94 if self.faults.losses && alive => {
                if self.dice.chance(40) {
                    self.note(format!("{name} is told to stop"));
                    self.process(&name).leaving = true;
                    self.say(&name, Said::Leaving);
                } else {
                    let stalled = !self.workers[&name].stalled;
                    self.note(format!("{name} vouches for nothing: {stalled}"));
                    self.process(&name).stalled = stalled;
                }
            }
            95..=96 if !alive => {
                self.note(format!("{name} is started again"));
                self.process(&name).alive = true;
            }
            95..=96 if self.faults.deaths => self.kill(&name, "of nothing in particular"),
            97 if self.faults.anew && self.dice.chance(40) => self.replace_the_coordinator(),
            _ => self.pass(self.pace / 5),
        }
    }

    /// Everything that is on its way arrives, and everything that waits is done.
    fn drain(&mut self) {
        for _ in 0..10_000 {
            let mut quiet = true;
            for name in Self::names() {
                if !self.workers[&name].alive {
                    continue;
                }
                if !self.workers[&name].connected {
                    self.register(&name);
                    quiet = false;
                }
                if let Some(order) = self.process(&name).slow.pop_front() {
                    self.reshape(&name, order);
                    quiet = false;
                }
                if !self.to_workers[&name].is_empty() {
                    self.deliver(&name);
                    quiet = false;
                }
                if !self.to_coordinator[&name].is_empty() {
                    self.hear(&name);
                    quiet = false;
                }
            }
            if self.wanted || self.reading.is_some() {
                self.read();
                self.hand_in();
                quiet = false;
            }
            if quiet {
                return;
            }
        }
        self.fail("the cluster never comes to rest".to_owned());
    }

    /// The run: `steps` things happen, then nothing goes wrong any more and the
    /// cluster is given time to settle, and then it has to be in order.
    fn play(mut self, steps: u32) -> BTreeMap<String, u32> {
        for _ in 0..steps {
            self.step();
        }

        self.note("from here on nothing goes wrong".to_owned());
        self.faults = Faults {
            losses: false,
            deaths: false,
            anew: false,
        };
        // Every worker is there again, vouches for what it runs, and at least one of
        // them is not on its way out: the regions need somebody to go to.
        for name in Self::names() {
            self.process(&name).alive = true;
            self.process(&name).stalled = false;
        }
        if self.workers.values().all(|process| process.leaving) {
            self.cut("w0");
            self.process("w0").leaving = false;
        }
        // The list is read once more, as it is whenever a worker registers: a split
        // that its worker made after its time was up, and died of, is in nobody's word.
        self.wanted = true;
        // Long enough for every lease, every merge and split, the grace period of a
        // new coordinator, the memory of a fault, and the regions to be evened out one
        // after the other.
        for _ in 0..(20 * LEASE.as_millis() / Self::PACE.as_millis()) {
            self.drain();
            self.pass(Self::PACE);
        }
        self.drain();
        self.settled();
        self.seen
    }

    /// When everything has settled: the coordinator has exactly the store's regions,
    /// each with one owner, which runs it and has it open with the epoch the routing
    /// table names; whoever asked has been told; and the table has what the list has.
    fn settled(&self) {
        let table = self.coordinator.routing_table();
        self.ensure(self.asked.is_empty(), || {
            format!(
                "at the end {:?} were never told what came of it",
                self.asked
            )
        });
        let waiting = self.coordinator.waiting();
        self.ensure(waiting.is_empty(), || {
            format!("at the end {waiting:?} have no owner")
        });
        let routed: Vec<u32> = table.routes.iter().map(|route| route.region.0).collect();
        let living: Vec<u32> = self.store.living.keys().copied().collect();
        self.ensure(routed == living, || {
            format!("at the end the coordinator has {routed:?} and the store {living:?}")
        });
        for route in &table.routes {
            let name = route.address.trim_end_matches(":25600");
            let id = route.region.0;
            let process = &self.workers[name];
            self.ensure(
                process.alive
                    && process.connected
                    && process.runs.get(&id).map(|held| held.epoch) == Some(route.epoch)
                    && self.store.holds(id, route.epoch, name),
                || {
                    format!(
                        "at the end {route:?} is not run by {name}, which is {process:?}, or \
                         the store has {:?}",
                        self.store.living[&id]
                    )
                },
            );
        }
        for (name, process) in &self.workers {
            let told: Vec<u32> = self
                .coordinator
                .assignments(name)
                .iter()
                .map(|held| held.region.0)
                .collect();
            let runs: Vec<u32> = process.runs.keys().copied().collect();
            self.ensure(runs == told, || {
                format!("at the end {name} runs {runs:?} and is to run {told:?}")
            });
        }
        self.ensure(
            table.home == Some(region(0)) && table.absorbed == pairs(&self.store.absorbed),
            || {
                format!(
                    "at the end the table has the home region {:?} and the pairs {:?}",
                    table.home, table.absorbed
                )
            },
        );
    }
}

/// The seeds of the generated runs: five, or as many as `CLUSTINE_RESHAPE_RUNS` says,
/// or the one that `CLUSTINE_RESHAPE_SEED` names.
fn seeds() -> Vec<u64> {
    let number = |name: &str| {
        std::env::var(name).ok().map(|value| {
            value
                .parse::<u64>()
                .unwrap_or_else(|_| panic!("{name} is to be a number, and is {value:?}"))
        })
    };
    match (
        number("CLUSTINE_RESHAPE_SEED"),
        number("CLUSTINE_RESHAPE_RUNS"),
    ) {
        (Some(seed), _) => vec![seed],
        (None, runs) => (1..=runs.unwrap_or(5)).collect(),
    }
}

/// The name of a reason without what it names.
fn kind(why: Undone) -> String {
    let words = format!("{why:?}");
    words
        .split(['(', ' '])
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// Plays a run for each seed, says what the runs were about (to be read with
/// `--nocapture`), and makes sure that they were about merges and splits.
fn play(faults: Faults) {
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    let seeds = seeds();
    for seed in &seeds {
        for (what, times) in Run::new(*seed, faults).play(1_500) {
            *seen.entry(what).or_default() += times;
        }
    }
    println!("{} runs with {faults:?}:", seeds.len());
    for (what, times) in &seen {
        println!("    {times:>6} {what}");
    }
    if seeds.len() >= 5 {
        assert!(seen.contains_key("merges done"), "{seen:?}");
        assert!(seen.contains_key("splits done"), "{seen:?}");
    }
}

#[test]
fn a_generated_run_in_which_nothing_goes_wrong_ends_in_order() {
    play(Faults {
        losses: false,
        deaths: false,
        anew: false,
    });
}

#[test]
fn a_generated_run_with_lost_connections_slow_workers_and_failing_readings_ends_in_order() {
    play(Faults {
        losses: true,
        deaths: false,
        anew: false,
    });
}

#[test]
fn a_generated_run_with_workers_that_die_ends_in_order() {
    play(Faults {
        losses: true,
        deaths: true,
        anew: false,
    });
}

#[test]
fn a_generated_run_with_workers_that_die_and_coordinators_made_anew_ends_in_order() {
    play(Faults {
        losses: true,
        deaths: true,
        anew: true,
    });
}

// ---------------------------------------------------------------------------------
// The service, over TCP, with a list that the test hands it.
// ---------------------------------------------------------------------------------

/// A lease that never runs out in a test: for what is to go in order.
const LONG_LEASE: Duration = Duration::from_secs(120);

/// A lease that a test waits out: for what is to happen when nobody answers. The
/// service reads the real clock, so this is time that passes; the test waits for the
/// answer and not for the time. The workers of a test are heard thirty times in it,
/// and what a test does before it stops answering takes a few milliseconds of it.
const SHORT_LEASE: Duration = Duration::from_secs(3);

/// How often the workers of these tests say that they are there.
const HEARTBEAT: Duration = Duration::from_millis(100);

/// How long a test waits for the service to say something before it gives up. Nothing
/// waits for this to pass; it only keeps a test that would hang from doing so.
const PATIENCE: Duration = Duration::from_secs(60);

/// What the service reads as the world store's list: whatever the test put there
/// last, or nothing, which is a store that does not answer. A test can hold a reading
/// back on its way to the service, after it has looked at the list.
#[derive(Clone)]
struct Lists(Arc<Reading>);

struct Reading {
    list: Mutex<Option<RegionList>>,
    /// Whether readings are held back, and how many are under way.
    held: Mutex<(bool, u32)>,
    let_go: Condvar,
    /// The most readings that were ever under way at once.
    most: AtomicU32,
    /// Told of every reading when it has looked at the list.
    looked: mpsc::UnboundedSender<()>,
}

impl Lists {
    fn set(&self, list: Option<RegionList>) {
        *self.0.list.lock().expect("no test panics with the list") = list;
    }

    /// Readings that have looked at the list wait from now on.
    fn hold(&self) {
        self.0.held.lock().expect("no test panics with the gate").0 = true;
    }

    /// The readings that wait go on, and no other waits.
    fn let_go(&self) {
        self.0.held.lock().expect("no test panics with the gate").0 = false;
        self.0.let_go.notify_all();
    }

    fn most_at_once(&self) -> u32 {
        self.0.most.load(Ordering::SeqCst)
    }

    fn reader(&self) -> impl Fn() -> io::Result<RegionList> + Send + Sync + 'static {
        let reading = Arc::clone(&self.0);
        move || {
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

/// A coordinator service on a port of its own.
struct Service {
    address: String,
    lists: Lists,
    fingerprint: u64,
    /// Hears of every reading of the list when it has looked at it.
    looked: mpsc::UnboundedReceiver<()>,
}

impl Service {
    /// Starts a service for a world of stripes with a list to read. It runs until the
    /// test ends.
    async fn start(boundaries: &[i32], lease: Duration, list: Option<RegionList>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port is free");
        let address = listener
            .local_addr()
            .expect("the listener has an address")
            .to_string();
        let config = config(boundaries, lease);
        let fingerprint = config.layout.fingerprint();
        let (looked, hears) = mpsc::unbounded_channel();
        let lists = Lists(Arc::new(Reading {
            list: Mutex::new(list),
            held: Mutex::new((false, 0)),
            let_go: Condvar::new(),
            most: AtomicU32::new(0),
            looked,
        }));
        tokio::spawn(serve(listener, config, lists.reader()));
        Self {
            address,
            lists,
            fingerprint,
            looked: hears,
        }
    }

    /// Holds readings back from now on and waits for one that has looked at the list
    /// as it is now, which `cause` brings about. No reading is under way when this is
    /// called: the service reads when it starts, when a worker registers, for a
    /// request, and when the coordinator asks for it, and each of those has been
    /// answered by then.
    async fn hold_a_reading(&mut self, cause: impl std::future::Future<Output = ()>) {
        while self.looked.try_recv().is_ok() {}
        self.lists.hold();
        cause.await;
        within(self.looked.recv())
            .await
            .expect("the service reads the list");
    }

    /// A worker that registers with the regions it runs already, each with the epoch
    /// `10 + id`, so that nothing waits for the grace period of a new coordinator.
    async fn worker(&self, name: &str, ids: &[u32]) -> WorkerClient {
        let holding: Vec<Assignment> = ids
            .iter()
            .map(|id| held(*id, 10 + u64::from(*id)))
            .collect();
        let (client, orders) = within(WorkerClient::register_with_heartbeat(
            &self.address,
            name,
            &address(name),
            &holding,
            Some(self.fingerprint),
            HEARTBEAT,
        ))
        .await
        .expect("the worker registers");
        assert_eq!(orders.assignments, holding);
        client
    }

    async fn watch(&self) -> RoutingWatch {
        within(RoutingWatch::connect(&self.address))
            .await
            .expect("the service takes an edge")
    }

    async fn ask_to_merge(&self, survivor: u32, absorbed: u32) -> Asker {
        within(Asker::merge(
            &self.address,
            region(survivor),
            region(absorbed),
        ))
        .await
        .expect("the service takes the request")
    }

    async fn ask_to_split(&self, of: u32) -> Asker {
        within(Asker::split(&self.address, region(of), &chunks()))
            .await
            .expect("the service takes the request")
    }
}

/// What the future comes to, or a failed test if the service says nothing for a
/// minute.
async fn within<T>(waited: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(PATIENCE, waited)
        .await
        .expect("the service says something within a minute")
}

async fn event(worker: &mut WorkerClient) -> WorkerEvent {
    within(worker.event())
        .await
        .expect("the worker has its connection")
}

/// The next thing the worker is told, which is to be new orders.
async fn orders(worker: &mut WorkerClient) -> Vec<(u32, u64)> {
    match event(worker).await {
        WorkerEvent::Orders(orders) => orders
            .assignments
            .iter()
            .map(|held| (held.region.0, held.epoch))
            .collect(),
        other => panic!("expected orders: {other:?}"),
    }
}

/// The first routing table from here on of which `wanted` holds.
async fn table_where(
    watch: &mut RoutingWatch,
    wanted: impl Fn(&RoutingTable) -> bool,
) -> RoutingTable {
    loop {
        let table = within(watch.next())
            .await
            .expect("the edge has its connection");
        if wanted(&table) {
            return table;
        }
    }
}

async fn answer(asker: Asker) -> Result<RegionId, String> {
    within(asker.answer())
        .await
        .expect("the coordinator answers before it closes the connection")
}

/// Three stripes whose workers `a`, `b` and `c` run one each, with the epochs 10, 11
/// and 12, and an edge that has seen all of it and the home region of the list.
async fn three_workers_of(
    service: &Service,
) -> (
    WorkerClient,
    WorkerClient,
    WorkerClient,
    RoutingWatch,
    RoutingTable,
) {
    let a = service.worker("a", &[0]).await;
    let b = service.worker("b", &[1]).await;
    let c = service.worker("c", &[2]).await;
    let mut watch = service.watch().await;
    let table = table_where(&mut watch, |table| {
        table.routes.len() == 3 && table.home == Some(region(0))
    })
    .await;
    assert!(table.is_complete());
    (a, b, c, watch, table)
}

/// The merge of region 1 into region 0 up to where the survivor's worker has been
/// told to absorb. Returns the epoch it was told.
async fn merge_to_absorb(a: &mut WorkerClient, b: &mut WorkerClient) -> u64 {
    assert_eq!(
        event(b).await,
        WorkerEvent::Release {
            region: region(1),
            epoch: 11
        }
    );
    assert_eq!(
        event(a).await,
        WorkerEvent::Prepare {
            region: region(0),
            epoch: 10
        }
    );
    b.released(region(1), 11);
    assert_eq!(orders(b).await, []);
    match event(a).await {
        WorkerEvent::Absorb {
            region: into,
            epoch: 10,
            absorbed,
            as_epoch,
        } if into == region(0) && absorbed == region(1) => as_epoch,
        other => panic!("expected the order to absorb: {other:?}"),
    }
}

#[tokio::test]
async fn the_service_reads_the_list_when_it_starts() {
    // The list has a region more than the layout, and one that was absorbed.
    let listed = list(&[0, 1, 2, 4], &[(3, 0)], 5);
    let service = Service::start(&[0, 4], LONG_LEASE, Some(listed)).await;
    let mut watch = service.watch().await;
    let table = table_where(&mut watch, |table| table.home.is_some()).await;
    assert_eq!(table.home, Some(region(0)));
    assert_eq!(table.absorbed, pairs(&[(3, 0)]));
    assert_eq!(table.routes, []);
    assert_eq!(table.waiting, 4);
    assert!(!table.is_complete());
}

#[tokio::test]
async fn the_service_runs_the_stripes_without_the_store_and_reads_the_list_when_a_worker_registers()
{
    let service = Service::start(&[0, 4], LONG_LEASE, None).await;
    let _a = service.worker("a", &[0, 1]).await;
    let mut watch = service.watch().await;
    let table = table_where(&mut watch, |table| table.routes.len() == 2).await;
    assert_eq!(table.home, None);
    assert_eq!(table.waiting, 1);

    // The store is there now, and has region 1 absorbed. Nothing tells the service
    // but a worker that registers.
    service.lists.set(Some(list(&[0, 2], &[(1, 0)], 3)));
    let _b = service.worker("b", &[2]).await;
    // A reading that was under way for the first worker may bring the list before the
    // second has registered: the table that is waited for has both.
    let table = table_where(&mut watch, |table| {
        table.home.is_some() && table.route(region(2)).is_some()
    })
    .await;
    assert_eq!(table.absorbed, pairs(&[(1, 0)]));
    let routed: Vec<u32> = table.routes.iter().map(|route| route.region.0).collect();
    assert_eq!(routed, [0, 2]);
    assert!(table.is_complete());
}

#[tokio::test]
async fn the_service_merges_two_regions_when_somebody_asks() {
    let service = Service::start(&[0, 4], LONG_LEASE, Some(three_stripes())).await;
    let (mut a, mut b, _c, mut watch, before) = three_workers_of(&service).await;

    let asker = service.ask_to_merge(0, 1).await;
    let as_epoch = merge_to_absorb(&mut a, &mut b).await;
    assert!(as_epoch > 12);
    let released = table_where(&mut watch, |table| table.route(region(1)).is_none()).await;
    assert!(released.version > before.version);
    assert_eq!(released.waiting, 1);
    assert!(!released.is_complete());
    assert_eq!(released.absorbed, []);

    // The store has the record, and the worker says so.
    service.lists.set(Some(list(&[0, 2], &[(1, 0)], 3)));
    a.absorb_ended(region(0), region(1), Ok(()));
    assert_eq!(answer(asker).await, Ok(region(0)));
    let after = table_where(&mut watch, |table| !table.absorbed.is_empty()).await;
    assert!(after.version > released.version);
    assert_eq!(after.absorbed, pairs(&[(1, 0)]));
    assert_eq!(
        after.routes,
        [
            before.route(region(0)).expect("it had a route").clone(),
            before.route(region(2)).expect("it had a route").clone(),
        ]
    );
    assert_eq!(after.waiting, 0);
    assert!(after.is_complete());

    // The region is no more: nobody can ask for it again.
    assert!(answer(service.ask_to_merge(0, 1).await).await.is_err());
}

#[tokio::test]
async fn the_service_refuses_a_merge_at_once_and_in_words() {
    let service = Service::start(&[0, 4], LONG_LEASE, Some(three_stripes())).await;
    let (_a, _b, _c, _watch, _) = three_workers_of(&service).await;
    for (survivor, absorbed) in [(1, 0), (1, 1), (1, 9), (9, 1)] {
        let refused = answer(service.ask_to_merge(survivor, absorbed).await).await;
        let reason = refused.expect_err("the merge is refused");
        assert!(!reason.is_empty());
    }
    let refused = within(Asker::split(&service.address, region(1), &[]))
        .await
        .expect("the service takes the request");
    assert!(answer(refused).await.is_err());
    assert!(answer(service.ask_to_split(9).await).await.is_err());
}

/// "The request waits for that reading, and is refused if it fails."
#[tokio::test]
async fn the_service_refuses_to_merge_and_to_split_when_the_list_cannot_be_read() {
    let service = Service::start(&[0, 4], LONG_LEASE, Some(three_stripes())).await;
    let (mut a, mut b, _c, _watch, _) = three_workers_of(&service).await;
    service.lists.set(None);
    assert!(answer(service.ask_to_merge(0, 1).await).await.is_err());
    assert!(answer(service.ask_to_split(1).await).await.is_err());

    // Nothing was begun: with the store back, the same merge is taken on and the
    // workers are told for the first time.
    service.lists.set(Some(three_stripes()));
    let _asker = service.ask_to_merge(0, 1).await;
    merge_to_absorb(&mut a, &mut b).await;
}

/// The reading is made after the request came: here the list has changed since the
/// service last had a reason to read it, and the request is judged by what it is now.
#[tokio::test]
async fn the_service_reads_the_list_before_it_looks_at_a_request() {
    let service = Service::start(&[0, 4], LONG_LEASE, Some(three_stripes())).await;
    let (_a, mut b, _c, _watch, _) = three_workers_of(&service).await;

    let mut elsewhere = three_stripes();
    elsewhere.home = region(1);
    service.lists.set(Some(elsewhere));
    assert!(answer(service.ask_to_merge(0, 1).await).await.is_err());

    // And a split is ordered with the next id of that reading.
    service
        .lists
        .set(Some(list(&[0, 1, 2], &[(3, 0), (4, 0)], 5)));
    let _asker = service.ask_to_split(1).await;
    match event(&mut b).await {
        WorkerEvent::SplitOff { part, .. } => assert_eq!(part, region(5)),
        other => panic!("expected the order to split: {other:?}"),
    }
}

#[tokio::test]
async fn the_service_gives_the_region_away_and_tells_the_reason_when_a_merge_is_off() {
    let service = Service::start(&[0, 4], LONG_LEASE, Some(three_stripes())).await;
    let (mut a, mut b, _c, mut watch, _) = three_workers_of(&service).await;
    let asker = service.ask_to_merge(0, 1).await;
    let as_epoch = merge_to_absorb(&mut a, &mut b).await;

    a.absorb_ended(region(0), region(1), Err(Off::Unreadable));
    let reason = answer(asker).await.expect_err("the merge is off");
    assert!(!reason.is_empty());
    // The region is given away at once, though the coordinator is new and its grace
    // period two minutes long: to the worker with nothing to run.
    let after = table_where(&mut watch, |table| {
        table
            .route(region(1))
            .is_some_and(|route| route.epoch != 11)
    })
    .await;
    let route = after.route(region(1)).expect("it has a route");
    assert!(route.epoch > as_epoch);
    assert_eq!(route.address, "b:25600");
    assert_eq!(orders(&mut b).await, [(1, route.epoch)]);
    assert!(after.is_complete());
    assert_eq!(after.absorbed, []);
}

#[tokio::test]
async fn the_service_ends_a_merge_whose_release_is_not_answered_within_the_lease() {
    let service = Service::start(&[0, 4], SHORT_LEASE, Some(three_stripes())).await;
    let (mut a, mut b, _c, mut watch, _) = three_workers_of(&service).await;
    let asker = service.ask_to_merge(0, 1).await;
    assert_eq!(
        event(&mut b).await,
        WorkerEvent::Release {
            region: region(1),
            epoch: 11
        }
    );
    assert_eq!(
        event(&mut a).await,
        WorkerEvent::Prepare {
            region: region(0),
            epoch: 10
        }
    );

    // The owner does not answer. The asker's connection is silent for the lease and
    // is not closed for that.
    assert!(answer(asker).await.is_err());
    let after = table_where(&mut watch, |table| {
        table
            .route(region(1))
            .is_some_and(|route| route.epoch != 11)
    })
    .await;
    let route = after.route(region(1)).expect("it has a route");
    assert!(route.epoch > 12);
    assert_ne!(route.address, "b:25600");
    assert_eq!(orders(&mut b).await, []);
}

/// "When a reservation ends without the worker's word": the list is read, and decides.
#[tokio::test]
async fn the_service_finds_a_merge_done_whose_worker_never_said_so() {
    let service = Service::start(&[0, 4], SHORT_LEASE, Some(three_stripes())).await;
    let (mut a, mut b, _c, mut watch, _) = three_workers_of(&service).await;
    let asker = service.ask_to_merge(0, 1).await;
    merge_to_absorb(&mut a, &mut b).await;

    // The record is written and the worker's word is lost.
    service.lists.set(Some(list(&[0, 2], &[(1, 0)], 3)));
    assert_eq!(answer(asker).await, Ok(region(0)));
    let after = table_where(&mut watch, |table| !table.absorbed.is_empty()).await;
    assert_eq!(after.absorbed, pairs(&[(1, 0)]));
    assert!(after.is_complete());
    assert_eq!(after.route(region(1)), None);
}

#[tokio::test]
async fn the_service_gives_up_a_merge_whose_worker_never_said_anything_and_the_list_does_not_show()
{
    let service = Service::start(&[0, 4], SHORT_LEASE, Some(three_stripes())).await;
    let (mut a, mut b, _c, mut watch, _) = three_workers_of(&service).await;
    let asker = service.ask_to_merge(0, 1).await;
    let as_epoch = merge_to_absorb(&mut a, &mut b).await;

    assert!(answer(asker).await.is_err());
    let after = table_where(&mut watch, |table| {
        table
            .route(region(1))
            .is_some_and(|route| route.epoch != 11)
    })
    .await;
    assert!(after.route(region(1)).expect("it has a route").epoch > as_epoch);
    assert!(after.is_complete());
}

#[tokio::test]
async fn the_service_splits_a_region_when_somebody_asks() {
    let service = Service::start(&[0, 4], LONG_LEASE, Some(three_stripes())).await;
    let (_a, mut b, _c, mut watch, before) = three_workers_of(&service).await;

    let asker = service.ask_to_split(1).await;
    let as_epoch = match event(&mut b).await {
        WorkerEvent::SplitOff {
            region: of,
            epoch: 11,
            chunks: named,
            as_epoch,
            part,
        } if of == region(1) && named == chunks() && part == region(3) => as_epoch,
        other => panic!("expected the order to split: {other:?}"),
    };
    assert!(as_epoch > 12);

    service.lists.set(Some(list(&[0, 1, 2, 3], &[], 4)));
    b.split_ended(region(1), as_epoch, Ok(region(3)));
    assert_eq!(answer(asker).await, Ok(region(3)));
    assert_eq!(orders(&mut b).await, [(1, 11), (3, as_epoch)]);
    let after = table_where(&mut watch, |table| table.route(region(3)).is_some()).await;
    let route = after.route(region(3)).expect("the part has a route");
    assert_eq!((route.epoch, route.address.as_str()), (as_epoch, "b:25600"));
    assert!(after.version > before.version);
    assert!(after.is_complete());
    assert_eq!(after.routes.len(), 4);
}

#[tokio::test]
async fn the_service_tells_whoever_asked_that_a_split_is_off() {
    let service = Service::start(&[0, 4], LONG_LEASE, Some(three_stripes())).await;
    let (_a, mut b, _c, _watch, _) = three_workers_of(&service).await;
    let asker = service.ask_to_split(1).await;
    let WorkerEvent::SplitOff { as_epoch, .. } = event(&mut b).await else {
        panic!("expected the order to split");
    };
    b.split_ended(region(1), as_epoch, Err(Off::Nobody));
    let reason = answer(asker).await.expect_err("the split is off");
    assert!(!reason.is_empty());

    // The region is free again.
    let _asker = service.ask_to_split(1).await;
    assert!(matches!(event(&mut b).await, WorkerEvent::SplitOff { .. }));
}

/// The part is in the list and has the id that was ordered, and whoever asked is
/// still told that the split is overdue; the region is given to somebody.
#[tokio::test]
async fn the_service_calls_a_split_overdue_whose_worker_never_said_anything() {
    let service = Service::start(&[0, 4], SHORT_LEASE, Some(three_stripes())).await;
    let (_a, mut b, _c, mut watch, _) = three_workers_of(&service).await;
    let asker = service.ask_to_split(1).await;
    let WorkerEvent::SplitOff { as_epoch, part, .. } = event(&mut b).await else {
        panic!("expected the order to split");
    };
    assert_eq!(part, region(3));

    service.lists.set(Some(list(&[0, 1, 2, 3], &[], 4)));
    assert!(answer(asker).await.is_err());
    let after = table_where(&mut watch, |table| table.route(region(3)).is_some()).await;
    assert!(after.route(region(3)).expect("it has a route").epoch > as_epoch);
    assert!(after.is_complete());
}

/// Q17 on the service: a reading that lands between the store's record and the
/// worker's word. The list has the part and one absorbed region more, by which the
/// edge sees that the reading was taken in.
#[tokio::test]
async fn the_service_leaves_the_part_of_a_split_to_its_worker_when_a_reading_shows_it_first() {
    let service = Service::start(&[0, 4], LONG_LEASE, Some(three_stripes())).await;
    let (_a, mut b, _c, mut watch, _) = three_workers_of(&service).await;
    let asker = service.ask_to_split(1).await;
    let WorkerEvent::SplitOff { as_epoch, part, .. } = event(&mut b).await else {
        panic!("expected the order to split");
    };
    assert_eq!(part, region(3));

    // The record is written. A worker with nothing to run registers, which has the
    // list read.
    service.lists.set(Some(list(&[0, 1, 2, 3], &[(4, 0)], 5)));
    let mut d = service.worker("d", &[]).await;
    let read = table_where(&mut watch, |table| !table.absorbed.is_empty()).await;
    assert_eq!(read.route(region(3)), None);
    assert_eq!(read.waiting, 0, "the part is not a region that waits");

    b.split_ended(region(1), as_epoch, Ok(region(3)));
    assert_eq!(answer(asker).await, Ok(region(3)));
    let after = table_where(&mut watch, |table| table.route(region(3)).is_some()).await;
    let route = after.route(region(3)).expect("the part has a route");
    assert_eq!((route.epoch, route.address.as_str()), (as_epoch, "b:25600"));
    assert_eq!(orders(&mut b).await, [(1, 11), (3, as_epoch)]);

    // The worker with nothing to run was given nothing: the next thing it hears, when
    // it says that it is leaving, is that the coordinator has closed its connection,
    // which it does at once for a worker that owns nothing.
    d.leaving();
    assert!(matches!(within(d.event()).await, Err(ClientError::Lost)));
}

/// "A reading comes back like something a client said", "readings are asked for one
/// at a time and applied in the order they were asked", and the list decides what
/// happened: so a reading that looked at the list before the worker said that its
/// absorb has ended must not be taken for the answer to that word. Here such a
/// reading is on its way with the regions as before, while the record is written and
/// the worker says so.
#[tokio::test]
async fn the_service_does_not_judge_a_merge_by_a_reading_from_before_its_workers_word() {
    let mut service = Service::start(&[0, 4], LONG_LEASE, Some(three_stripes())).await;
    let (mut a, mut b, _c, mut watch, _) = three_workers_of(&service).await;
    let asker = service.ask_to_merge(0, 1).await;
    merge_to_absorb(&mut a, &mut b).await;

    // A worker registers, for which the list is read; the reading sees both regions
    // living and is held back.
    let address = service.address.clone();
    let fingerprint = service.fingerprint;
    let mut late = None;
    service
        .hold_a_reading(async {
            let registered = within(WorkerClient::register_with_heartbeat(
                &address,
                "d",
                "d:25600",
                &[],
                Some(fingerprint),
                HEARTBEAT,
            ))
            .await
            .expect("the worker registers");
            late = Some(registered.0);
        })
        .await;

    service.lists.set(Some(list(&[0, 2], &[(1, 0)], 3)));
    a.absorb_ended(region(0), region(1), Ok(()));
    // An edge that connects now is answered when the service has taken the worker's
    // word, which was said before, as far as a test can tell from outside. Should the
    // word come later than the reading all the same, the test passes without having
    // tried anything.
    let mut edge = service.watch().await;
    within(edge.next()).await.expect("the edge is answered");

    service.lists.let_go();
    assert_eq!(answer(asker).await, Ok(region(0)));
    let after = table_where(&mut watch, |table| !table.absorbed.is_empty()).await;
    assert_eq!(after.route(region(1)), None);
    assert!(after.is_complete());
    assert_eq!(service.lists.most_at_once(), 1);
    drop(late);
}

/// The same for a request: it "waits for that reading", which is one made for it. A
/// reading that was on its way when the request came says how things were before.
#[tokio::test]
async fn the_service_does_not_judge_a_request_by_a_reading_from_before_it() {
    let mut service = Service::start(&[0, 4], LONG_LEASE, Some(three_stripes())).await;
    let (_a, mut b, _c, _watch, _) = three_workers_of(&service).await;

    let address = service.address.clone();
    let fingerprint = service.fingerprint;
    let mut late = None;
    service
        .hold_a_reading(async {
            let registered = within(WorkerClient::register_with_heartbeat(
                &address,
                "d",
                "d:25600",
                &[],
                Some(fingerprint),
                HEARTBEAT,
            ))
            .await
            .expect("the worker registers");
            late = Some(registered.0);
        })
        .await;

    // The store has split a region off meanwhile, so the next id is another, and the
    // home region is region 2 now.
    let mut now = list(&[0, 1, 2, 3], &[], 4);
    now.home = region(2);
    service.lists.set(Some(now));
    let merge = service.ask_to_merge(0, 2).await;
    let split = service.ask_to_split(1).await;
    service.lists.let_go();

    assert!(
        answer(merge).await.is_err(),
        "the home region is not absorbed"
    );
    match event(&mut b).await {
        WorkerEvent::SplitOff { part, .. } => assert_eq!(part, region(4)),
        other => panic!("expected the order to split: {other:?}"),
    }
    assert_eq!(service.lists.most_at_once(), 1);
    drop((split, late));
}

/// Found by the end-to-end tests of merges and splits under the bots
/// (`bin/clustine/tests/merges.rs`, which has the whole sequence), and written there
/// by their writer: after a split of which the worker says that it lost the store,
/// and a reading of the list that fails, the coordinator has to ask for the list
/// again by itself, and then gives the new region to somebody. It did not, and a part
/// the store had made before it died was run by nobody.
#[test]
fn the_coordinator_reads_the_list_again_after_a_split_whose_worker_lost_the_store() {
    let lease = Duration::from_secs(5);
    let look = Duration::from_millis(250);
    let list = |living: &[u32], next: u32| RegionList {
        home: RegionId(0),
        regions: living
            .iter()
            .map(|id| RegionInfo {
                region: RegionId(*id),
                epoch: 0,
                bounds: None,
                pinned: Vec::new(),
            })
            .collect(),
        absorbed: Vec::new(),
        next: RegionId(next),
    };
    let config = CoordinatorConfig {
        layout: Layout::new(Vec::new()).expect("a world of one region"),
        spawn: Vec3::new(0.5, 64.0, 0.5),
        lease,
    };
    let start = Instant::now();
    let mut coordinator = Coordinator::new(config, start, 1_000);
    coordinator
        .register(start, "a", "a:25600", &[], None)
        .expect("the worker is let in");
    coordinator.listed(start, &list(&[0], 1));
    // A new coordinator gives nothing away for a lease, and the worker goes on saying
    // that it is there.
    let mut now = start + lease;
    coordinator.heartbeat(now, "a", &[]);
    now += Duration::from_millis(1);
    coordinator.tick(now);
    let regions = |coordinator: &Coordinator| -> Vec<u32> {
        let held = coordinator.assignments("a");
        held.iter().map(|held| held.region.0).collect()
    };
    assert_eq!(regions(&coordinator), [0], "the worker runs the one region");

    // Steps 1 to 3: the split is asked for and ordered, and the worker says that it
    // lost the store over it.
    let ordered = coordinator
        .split(now, RegionId(0), &[ChunkPos::new(3, 0)], Some(41))
        .expect("the split is taken on");
    let [
        ReshapeOrder {
            order: Order::SplitOff { as_epoch, part, .. },
            ..
        },
    ] = ordered.orders.as_slice()
    else {
        panic!("expected the order to split: {ordered:?}");
    };
    assert_eq!(*part, RegionId(1));
    let said = coordinator.split_ended(now, "a", RegionId(0), *as_epoch, Err(Off::StoreLost));
    assert!(said.read, "the list is to be read: {said:?}");
    // Step 4: the store is still away.
    coordinator.unlisted(now);

    // Step 5: the store is back, and has the split. Nobody tells the coordinator, so
    // it has to ask again; within a lease is soon enough for this test.
    let mut asked_again = false;
    for _ in 0..lease.as_millis() / look.as_millis() {
        now += look;
        coordinator.heartbeat(now, "a", &[(RegionId(0), Vouch::Committed)]);
        if coordinator.tick(now).read {
            asked_again = true;
            break;
        }
    }
    assert!(
        asked_again,
        "the reading that followed `SplitEnded {{ Err(StoreLost) }}` failed, and the \
         coordinator did not ask for the list again within a lease: a region the split \
         made stays unknown to it"
    );
    coordinator.listed(now, &list(&[0, 1], 2));
    assert_eq!(
        regions(&coordinator),
        [0, 1],
        "the region the split made is given to the worker that is there"
    );
}
