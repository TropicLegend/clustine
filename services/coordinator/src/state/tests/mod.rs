use clustine_rpc::RegionInfo;
use clustine_world::EntityId;

use super::*;

/// The lease of the coordinators in these tests, in milliseconds. The tests give
/// every time in milliseconds since the first coordinator was created.
const LEASE: u64 = 10_000;

/// What a coordinator is given as its first epoch unless a test says otherwise.
const FIRST_EPOCH: u64 = 1000;

/// What can be seen of a coordinator from outside.
#[derive(Debug, Clone, PartialEq)]
struct View {
    /// The assignments of every worker that has ever registered.
    assignments: BTreeMap<String, Vec<Assignment>>,
    table: RoutingTable,
}

impl View {
    fn assignments(&self, name: &str) -> &[Assignment] {
        self.assignments.get(name).map_or(&[], Vec::as_slice)
    }
}

/// A coordinator and what the workers told it. Every call goes through here and is
/// checked: what the coordinator says it changed must be exactly what can be seen to
/// have changed, and what can be seen must be in order.
struct Cluster {
    coordinator: Coordinator,
    /// The regions the coordinator is made knowing.
    regions: Vec<RegionId>,
    start: Instant,
    /// The address each worker gave when it last registered.
    addresses: BTreeMap<String, String>,
    /// What the last call that may change something said it changed.
    last: Option<Changes>,
}

impl Cluster {
    /// A new coordinator that knows the regions of a world with region boundaries
    /// at these chunk x coordinates: one more than there are boundaries.
    fn new(boundaries: &[i32]) -> Self {
        Self::with_first_epoch(boundaries, FIRST_EPOCH)
    }

    fn with_first_epoch(boundaries: &[i32], first_epoch: u64) -> Self {
        let regions = stripes_of(boundaries);
        let start = Instant::now();
        Self {
            coordinator: coordinator(&regions, start, first_epoch),
            regions,
            start,
            addresses: BTreeMap::new(),
            last: None,
        }
    }

    /// Replaces the coordinator by a new one, which knows nothing of what was.
    fn restart(&mut self, at: u64, first_epoch: u64) {
        self.coordinator = coordinator(&self.regions, self.at(at), first_epoch);
    }

    fn at(&self, milliseconds: u64) -> Instant {
        self.start + Duration::from_millis(milliseconds)
    }

    /// Registers a worker.
    fn register(&mut self, at: u64, name: &str, address: &str, holding: &[Assignment]) -> Changes {
        let before = self.view();
        let now = self.at(at);
        let mut releases = self.coordinator.releases.values();
        let asked = releases.any(|release| release.from == name);
        // Or it has a part in a merge or a split, which what it reports can end.
        let reshaping = self.coordinator.merges.values().any(|merge| {
            let releasing =
                matches!(&merge.stage, MergeStage::Releasing { from, .. } if from == name);
            releasing || merge.owner == name
        }) || self
            .coordinator
            .splits
            .values()
            .any(|split| split.owner == name);
        let changes = self.coordinator.register(now, name, address, holding);
        self.addresses.insert(name.to_owned(), address.to_owned());
        self.verify(&before, &changes);
        // A registration is nobody else's business, unless the worker was asked
        // to release a region and comes back without it.
        let own = changes.workers.iter().all(|worker| worker == name);
        assert!(asked || reshaping || own);
        changes
    }

    /// A heartbeat that vouches for everything the worker owns as committed, which
    /// is what a worker that is in order says.
    fn heartbeat(&mut self, at: u64, name: &str) -> bool {
        let regions: Vec<(RegionId, Vouch)> = self
            .assignments(name)
            .iter()
            .map(|held| (held.region, Vouch::Committed))
            .collect();
        self.heartbeat_with(at, name, &regions)
    }

    fn heartbeat_with(&mut self, at: u64, name: &str, regions: &[(RegionId, Vouch)]) -> bool {
        let before = self.view();
        let known = self.coordinator.heartbeat(self.at(at), name, regions);
        assert_eq!(self.view(), before, "a heartbeat changed something");
        known
    }

    /// The worker says where the players of `regions` are.
    fn players(&mut self, at: u64, name: &str, regions: &[PlayersOf]) -> bool {
        let before = self.view();
        let known = self.coordinator.players(self.at(at), name, regions);
        assert_eq!(self.view(), before, "word of players changed something");
        known
    }

    fn epoch_refused(&mut self, at: u64, name: &str, region: u32, seen: u64) -> Changes {
        let before = self.view();
        let now = self.at(at);
        let changes = self
            .coordinator
            .epoch_refused(now, name, RegionId(region), seen);
        self.verify(&before, &changes);
        changes
    }

    fn tick(&mut self, at: u64) -> Changes {
        let before = self.view();
        let changes = self.coordinator.tick(self.at(at));
        self.verify(&before, &changes);
        changes
    }

    fn move_region(
        &mut self,
        at: u64,
        region: u32,
        to: Option<&str>,
        mover: u64,
    ) -> Result<(MoveBegun, Changes), MoveRefusal> {
        let before = self.view();
        let releases = self.coordinator.releases.len();
        let now = self.at(at);
        let result = self
            .coordinator
            .move_region(now, RegionId(region), to, mover);
        // Whether or not it begins, asking for a move changes no owner.
        assert_eq!(self.view(), before, "asking for a move changed something");
        match &result {
            Ok((begun, changes)) => {
                self.verify(&before, changes);
                let order = ReleaseOrder {
                    worker: begun.from.clone(),
                    region: RegionId(region),
                    epoch: self.coordinator.releases[&RegionId(region)].epoch,
                };
                assert_eq!(changes.releases, [order]);
                assert!(changes.moves.is_empty() && changes.gone.is_empty());
            }
            Err(_) => assert_eq!(self.coordinator.releases.len(), releases),
        }
        result
    }

    fn released(&mut self, at: u64, name: &str, region: u32, epoch: u64) -> Changes {
        let before = self.view();
        let now = self.at(at);
        let changes = self
            .coordinator
            .released(now, name, RegionId(region), epoch);
        self.verify(&before, &changes);
        changes
    }

    fn leaving(&mut self, at: u64, name: &str) -> Changes {
        let before = self.view();
        let changes = self.coordinator.leaving(self.at(at), name);
        self.verify(&before, &changes);
        // To say that one leaves takes no region from anybody.
        assert_eq!(self.view(), before, "leaving changed an owner");
        changes
    }

    fn disconnected(&mut self, at: u64, name: &str) -> Changes {
        let before = self.view();
        let changes = self.coordinator.disconnected(self.at(at), name);
        self.verify(&before, &changes);
        changes
    }

    fn listed(&mut self, at: u64, list: &RegionList) -> Changes {
        let before = self.view();
        let changes = self.coordinator.listed(self.at(at), list);
        self.verify(&before, &changes);
        // The table has what the list had, and every reservation that waited for
        // a reading has had it.
        let table = self.table();
        assert_eq!(table.home, Some(list.home));
        assert_eq!(table.absorbed, list.absorbed);
        assert!(!changes.read, "{changes:?}");
        changes
    }

    fn unlisted(&mut self, at: u64) -> Changes {
        let before = self.view();
        let changes = self.coordinator.unlisted(self.at(at));
        self.verify(&before, &changes);
        assert!(!changes.read, "{changes:?}");
        changes
    }

    fn merge(
        &mut self,
        at: u64,
        survivor: u32,
        absorbed: u32,
        asker: Option<u64>,
    ) -> Result<Changes, ReshapeRefusal> {
        let before = self.view();
        let reserved = self.reservations();
        let (survivor, absorbed) = (RegionId(survivor), RegionId(absorbed));
        let now = self.at(at);
        let result = self.coordinator.merge(now, survivor, absorbed, asker);
        // Whether or not it begins, asking for a merge changes no owner.
        assert_eq!(self.view(), before, "asking for a merge changed something");
        match &result {
            Ok(changes) => self.verify(&before, changes),
            Err(_) => assert_eq!(self.reservations(), reserved),
        }
        result
    }

    fn split(
        &mut self,
        at: u64,
        region: u32,
        chunks: &[ChunkPos],
        asker: Option<u64>,
    ) -> Result<Changes, ReshapeRefusal> {
        let before = self.view();
        let reserved = self.reservations();
        let now = self.at(at);
        let result = self.coordinator.split(now, RegionId(region), chunks, asker);
        assert_eq!(self.view(), before, "asking for a split changed something");
        match &result {
            Ok(changes) => self.verify(&before, changes),
            Err(_) => assert_eq!(self.reservations(), reserved),
        }
        result
    }

    fn absorb_ended(
        &mut self,
        at: u64,
        name: &str,
        region: u32,
        absorbed: u32,
        outcome: Result<(), Off>,
    ) -> Changes {
        let before = self.view();
        let (region, absorbed) = (RegionId(region), RegionId(absorbed));
        let now = self.at(at);
        let changes = self
            .coordinator
            .absorb_ended(now, name, region, absorbed, outcome);
        self.verify(&before, &changes);
        // It decides nothing: the list does.
        assert_eq!(
            self.view(),
            before,
            "a worker's word of a merge changed something"
        );
        assert!(changes.read && changes.reshaped.is_empty(), "{changes:?}");
        changes
    }

    fn split_ended(
        &mut self,
        at: u64,
        name: &str,
        region: u32,
        as_epoch: u64,
        outcome: Result<u32, Off>,
    ) -> Changes {
        let before = self.view();
        let now = self.at(at);
        let outcome = outcome.map(RegionId);
        let changes = self
            .coordinator
            .split_ended(now, name, RegionId(region), as_epoch, outcome);
        self.verify(&before, &changes);
        assert!(changes.read, "{changes:?}");
        changes
    }

    /// The regions that are part of a merge or a split under way.
    fn reservations(&self) -> Vec<RegionId> {
        let known = self.coordinator.regions.keys();
        let reserved = known.filter(|region| self.coordinator.reserved(**region));
        reserved.copied().collect()
    }

    fn assignments(&self, name: &str) -> Vec<Assignment> {
        self.coordinator.assignments(name)
    }

    fn table(&self) -> RoutingTable {
        self.coordinator.routing_table()
    }

    fn view(&self) -> View {
        let assignments = self
            .addresses
            .keys()
            .map(|name| (name.clone(), self.coordinator.assignments(name)))
            .collect();
        View {
            assignments,
            table: self.coordinator.routing_table(),
        }
    }

    /// Checks that `changes` is exactly what a call changed, given what could be
    /// seen before it, and that what can be seen now is in order.
    fn verify(&mut self, before: &View, changes: &Changes) {
        self.last = Some(changes.clone());
        let now = self.view();
        let changed: Vec<String> = now
            .assignments
            .keys()
            .filter(|name| before.assignments(name) != now.assignments(name))
            .cloned()
            .collect();
        assert_eq!(changes.workers, changed);
        // The table has a new version exactly when anything else of it is new.
        let unchanged = RoutingTable {
            version: now.table.version,
            ..before.table.clone()
        };
        assert_eq!(changes.routing, unchanged != now.table);
        assert_eq!(
            now.table.version,
            before.table.version + u64::from(changes.routing)
        );

        // The table sends edges to the owner of each region that has one.
        let mut routes: Vec<RegionRoute> = Vec::new();
        for (name, assignments) in &now.assignments {
            routes.extend(assignments.iter().map(|assignment| RegionRoute {
                dimension: Default::default(),
                region: assignment.region,
                epoch: assignment.epoch,
                address: self.addresses[name].clone(),
            }));
        }
        routes.sort_by_key(|route| route.region);
        assert_eq!(now.table.routes, routes);
        // And counts the regions that have none.
        let known = &self.coordinator.regions;
        assert_eq!(now.table.waiting as usize, known.len() - routes.len());
        assert_eq!(self.coordinator.waiting().len(), known.len() - routes.len());
        assert_eq!(now.table.is_complete(), known.len() == routes.len());

        let live: Vec<Assignment> = now.assignments.values().flatten().copied().collect();
        for (index, one) in live.iter().enumerate() {
            assert!(known.contains_key(&one.region), "{one:?}");
            for other in &live[index + 1..] {
                assert_ne!(one.region, other.region, "a region has two owners");
            }
        }

        // A merge or a split never outlives the owners and epochs it began with,
        // unless the list is being read to end it; a region that waits to be
        // absorbed has no owner; and no region of one is being released as well.
        let coordinator = &self.coordinator;
        for (absorbed, merge) in &coordinator.merges {
            let lapsed = matches!(merge.stage, MergeStage::Lapsed { .. });
            assert!(
                lapsed || coordinator.broken_merge(*absorbed).is_none(),
                "{merge:?}"
            );
            assert!(!lapsed || coordinator.reading, "{merge:?}");
            if matches!(
                merge.stage,
                MergeStage::Absorbing { .. } | MergeStage::Ended { .. }
            ) {
                assert!(now.table.route(*absorbed).is_none(), "{merge:?}");
            }
            for region in [absorbed, &merge.survivor] {
                assert!(!coordinator.releases.contains_key(region), "{merge:?}");
                assert!(!coordinator.splits.contains_key(region), "{merge:?}");
            }
            let others = coordinator
                .merges
                .iter()
                .filter(|(other, _)| *other != absorbed);
            for (other, second) in others {
                assert_ne!(merge.survivor, second.survivor, "{merge:?}");
                assert_ne!(merge.survivor, *other, "{merge:?}");
            }
        }
        for (region, split) in &coordinator.splits {
            let held = coordinator.holds(*region, &split.owner, split.epoch);
            assert!(held, "{split:?} outlived its owner");
            assert!(!coordinator.releases.contains_key(region), "{split:?}");
        }
        // Whoever asked for a merge or a split hears of it once, when it is over.
        for reshaped in &changes.reshaped {
            let region = match reshaped.asked {
                Asked::Merge { absorbed, .. } => absorbed,
                Asked::Split { region } => region,
            };
            assert!(!coordinator.merges.contains_key(&region), "{reshaped:?}");
            assert!(!coordinator.splits.contains_key(&region), "{reshaped:?}");
        }
        // An order names the owner and the epoch of the region it is about.
        for order in &changes.orders {
            let (Order::Prepare { region, epoch }
            | Order::Absorb { region, epoch, .. }
            | Order::SplitOff { region, epoch, .. }) = &order.order;
            assert!(
                coordinator.holds(*region, &order.worker, *epoch),
                "{order:?}"
            );
        }

        // A release never outlives its owner or its epoch, and no worker is the
        // target of its own.
        for (region, release) in &self.coordinator.releases {
            let held = Assignment {
                region: *region,
                epoch: release.epoch,
                entity_ids: ids(0),
            };
            let owned = self.coordinator.assignments(&release.from);
            let same =
                |owned: &Assignment| (owned.region, owned.epoch) == (held.region, held.epoch);
            assert!(owned.iter().any(same), "{release:?} outlived its owner");
            assert_ne!(release.from, release.to, "{release:?}");
        }
        // Nothing is left over for the next call to hand out.
        let Pending {
            ended,
            orders,
            gone,
            reshapes,
            reshaped,
            read,
            relisted,
        } = &self.coordinator.pending;
        assert!(ended.is_empty() && orders.is_empty() && gone.is_empty());
        assert!(reshapes.is_empty() && reshaped.is_empty() && !read && !relisted);
        // A worker that left is forgotten, and one that is told to release owns
        // what it is told to release: for a move, or for a merge.
        for name in &changes.gone {
            assert!(!self.coordinator.workers.contains_key(name), "{name}");
        }
        for order in &changes.releases {
            let asked = match self.coordinator.releases.get(&order.region) {
                Some(release) => (&release.from, release.epoch),
                None => match &self.coordinator.merges[&order.region].stage {
                    MergeStage::Releasing { from, epoch } => (from, *epoch),
                    other => panic!("{order:?} for a merge that is {other:?}"),
                },
            };
            assert_eq!((&order.worker, order.epoch), asked);
        }
        // Whoever asked for a move is told who has the region now.
        for outcome in &changes.moves {
            let route = now.table.route(outcome.region);
            let owner = outcome.owner.as_ref();
            assert_eq!(
                owner.map(|(_, epoch)| *epoch),
                route.map(|route| route.epoch)
            );
            if let Some((name, epoch)) = owner {
                let has = now.assignments(name).iter();
                let mut has = has.map(|held| (held.region, held.epoch));
                assert!(
                    has.any(|held| held == (outcome.region, *epoch)),
                    "{outcome:?}"
                );
            }
        }
    }
}

/// A worker that is to be told to release a region.
fn order(worker: &str, region: u32, epoch: u64) -> ReleaseOrder {
    ReleaseOrder {
        worker: worker.to_owned(),
        region: RegionId(region),
        epoch,
    }
}

/// How the move that `mover` asked for ended.
fn outcome(mover: u64, region: u32, owner: Option<(&str, u64)>, released: bool) -> MoveOutcome {
    MoveOutcome {
        mover,
        region: RegionId(region),
        owner: owner.map(|(name, epoch)| (name.to_owned(), epoch)),
        released,
    }
}

fn begun(from: &str, to: &str) -> MoveBegun {
    MoveBegun {
        from: from.to_owned(),
        to: to.to_owned(),
    }
}

/// What a call says that asks nobody but `orders` to do anything.
fn asks(orders: &[ReleaseOrder]) -> Changes {
    Changes {
        releases: orders.to_vec(),
        ..Changes::default()
    }
}

fn coordinator(regions: &[RegionId], now: Instant, first_epoch: u64) -> Coordinator {
    Coordinator::knowing(config(), now, first_epoch, regions)
}

/// What the coordinators of these tests are created with.
fn config() -> CoordinatorConfig {
    CoordinatorConfig {
        lease: Duration::from_millis(LEASE),
        follow: None,
    }
}

/// The regions a coordinator of these tests is made knowing: one for every stripe
/// of a world with these boundaries, numbered from 0, as a coordinator knew them
/// before it learnt its regions from the world store's list.
fn stripes_of(boundaries: &[i32]) -> Vec<RegionId> {
    (0..=boundaries.len() as u32).map(RegionId).collect()
}

fn ids(block: u32) -> EntityIds {
    EntityIds::block(block).unwrap()
}

fn assignment(region: u32, epoch: u64, block: u32) -> Assignment {
    Assignment {
        region: RegionId(region),
        epoch,
        entity_ids: ids(block),
    }
}

fn route(region: u32, epoch: u64, address: &str) -> RegionRoute {
    RegionRoute {
        dimension: Default::default(),
        region: RegionId(region),
        epoch,
        address: address.to_owned(),
    }
}

fn changes(workers: &[&str], routing: bool) -> Changes {
    Changes {
        workers: workers.iter().map(|name| (*name).to_owned()).collect(),
        routing,
        ..Changes::default()
    }
}

/// The world store's list of a world that is divided into `count` stripes, none
/// of which has been opened, merged or split: the first is the home region.
fn stripes(count: u32) -> RegionList {
    let regions = (0..count).map(|region| living(region, 0)).collect();
    RegionList {
        homes: Vec::new(),
        spawn: clustine_world::BlockPos::new(0, -60, 0),
        home: RegionId(0),
        regions,
        absorbed: Vec::new(),
        next: RegionId(count),
    }
}

/// What the list has of a living region that was last opened with `epoch`. Where
/// it lies does not concern the coordinator yet.
fn living(region: u32, epoch: u64) -> RegionInfo {
    RegionInfo {
        dimension: Default::default(),
        region: RegionId(region),
        epoch,
        bounds: None,
        pinned: Vec::new(),
    }
}

/// The tests of a coordinator that merges and splits regions by itself.
mod follows;

/// The tests of how a coordinator begins: knowing no region, or made knowing
/// its regions.
mod begins;

#[test]
fn two_workers_are_given_the_two_regions_in_the_order_they_registered() {
    let mut cluster = Cluster::new(&[0]);
    // Not in the order of their names.
    assert_eq!(cluster.register(1, "b", "b:25601", &[]), Changes::default());
    assert_eq!(cluster.register(2, "a", "a:25601", &[]), Changes::default());
    let table = cluster.table();
    assert_eq!(table.version, FIRST_EPOCH);
    assert!(table.routes.is_empty());
    assert!(!table.is_complete());

    assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
    assert_eq!(
        cluster.assignments("b"),
        [assignment(0, FIRST_EPOCH + 1, 0)]
    );
    assert_eq!(
        cluster.assignments("a"),
        [assignment(1, FIRST_EPOCH + 2, 1)]
    );
    let table = cluster.table();
    assert!(table.is_complete());
    assert_eq!(table.version, FIRST_EPOCH + 1);
    assert_eq!(
        table.routes,
        [
            route(0, FIRST_EPOCH + 1, "b:25601"),
            route(1, FIRST_EPOCH + 2, "a:25601"),
        ]
    );

    // There is nothing left to do.
    assert_eq!(cluster.tick(LEASE + 1), Changes::default());
    assert_eq!(cluster.table(), table);
}

#[test]
fn the_table_fills_up_with_a_new_version_for_each_call_that_adds_to_it() {
    let mut cluster = Cluster::new(&[0]);
    // Nobody is there to run anything.
    assert_eq!(cluster.tick(1), Changes::default());

    // A worker that ran one of the regions under the coordinator before this one.
    let held = assignment(0, 7, 0);
    assert_eq!(
        cluster.register(1, "a", "a:25601", &[held]),
        changes(&["a"], true)
    );
    let table = cluster.table();
    assert_eq!(table.version, FIRST_EPOCH + 1);
    assert_eq!(table.routes, [route(0, 7, "a:25601")]);
    assert!(!table.is_complete());

    // The other region goes to the worker that has none.
    cluster.register(2, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["b"], true));
    let table = cluster.table();
    assert_eq!(table.version, FIRST_EPOCH + 2);
    assert_eq!(
        table.routes,
        [route(0, 7, "a:25601"), route(1, FIRST_EPOCH + 1, "b:25601"),]
    );
    assert!(table.is_complete());
}

/// The boundaries of a world with five regions.
const FIVE: [i32; 4] = [-16, -8, 0, 8];

/// The numbers of the regions that the worker `name` runs.
fn regions_of(cluster: &Cluster, name: &str) -> Vec<u32> {
    let held = cluster.assignments(name);
    held.iter().map(|held| held.region.0).collect()
}

#[test]
fn fewer_workers_than_regions_are_given_every_region_spread_evenly() {
    let mut cluster = Cluster::new(&FIVE);
    cluster.register(0, "a", "a:25601", &[]);
    cluster.register(0, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
    // The lowest region first, each to the worker with the fewest so far, and of
    // two with equally many to the one that registered first.
    assert_eq!(regions_of(&cluster, "a"), [0, 2, 4]);
    assert_eq!(regions_of(&cluster, "b"), [1, 3]);
    let table = cluster.table();
    assert!(table.is_complete());
    // One call, one new version, and an epoch of its own for each region.
    assert_eq!(table.version, FIRST_EPOCH + 1);
    let epochs: Vec<u64> = table.routes.iter().map(|route| route.epoch).collect();
    assert_eq!(epochs, Vec::from_iter(FIRST_EPOCH + 1..=FIRST_EPOCH + 5));
    assert_eq!(cluster.tick(LEASE), Changes::default());

    // A single worker runs the whole world.
    let mut cluster = Cluster::new(&FIVE);
    cluster.register(0, "a", "a:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    assert_eq!(regions_of(&cluster, "a"), [0, 1, 2, 3, 4]);
}

#[test]
fn the_regions_of_a_worker_whose_lease_ran_out_are_spread_over_those_that_remain() {
    let mut cluster = Cluster::new(&[-16, -8, 0, 8, 16]);
    for name in ["a", "b", "c"] {
        cluster.register(0, name, &format!("{name}:25601"), &[]);
    }
    assert_eq!(cluster.tick(LEASE), changes(&["a", "b", "c"], true));
    assert_eq!(regions_of(&cluster, "c"), [2, 5]);

    for name in ["a", "b"] {
        assert!(cluster.heartbeat(2 * LEASE, name));
    }
    assert_eq!(cluster.tick(2 * LEASE + 1), changes(&["a", "b", "c"], true));
    // One each, and not both to the worker that comes first.
    assert_eq!(regions_of(&cluster, "a"), [0, 2, 3]);
    assert_eq!(regions_of(&cluster, "b"), [1, 4, 5]);
    assert!(cluster.table().is_complete());
    assert!(!cluster.heartbeat(2 * LEASE + 1, "c"));

    // With one worker more than the other, the next region to lose its owner does
    // not go to the one that has more, although that one registered first.
    let mut cluster = Cluster::new(&FIVE);
    for name in ["a", "b", "c"] {
        cluster.register(0, name, &format!("{name}:25601"), &[]);
    }
    assert_eq!(cluster.tick(LEASE), changes(&["a", "b", "c"], true));
    assert_eq!(regions_of(&cluster, "a"), [0, 3]);
    assert_eq!(regions_of(&cluster, "b"), [1, 4]);
    assert_eq!(regions_of(&cluster, "c"), [2]);
    for name in ["a", "c"] {
        assert!(cluster.heartbeat(2 * LEASE, name));
    }
    assert_eq!(cluster.tick(2 * LEASE + 1), changes(&["a", "b", "c"], true));
    assert_eq!(regions_of(&cluster, "a"), [0, 3, 4]);
    assert_eq!(regions_of(&cluster, "c"), [1, 2]);
}

/// The worker each region that is being released is meant for.
fn targets(cluster: &Cluster) -> Vec<(u32, &str)> {
    let releases = cluster.coordinator.releases.iter();
    Vec::from_iter(releases.map(|(region, release)| (region.0, release.to.as_str())))
}

/// Lets `leases` leases go by from `from` on, a quarter of a lease at a time, with
/// every worker heard from and vouching for what it runs: no tick changes or
/// begins anything.
fn nothing_happens(cluster: &mut Cluster, from: u64, leases: u64, workers: &[&str]) {
    for quarter in 1..=4 * leases {
        let now = from + quarter * LEASE / 4;
        for name in workers {
            assert!(cluster.heartbeat(now, name));
        }
        assert_eq!(cluster.tick(now), Changes::default(), "at {now}");
    }
}

#[test]
fn a_second_worker_is_given_one_of_the_two_regions_of_the_first_at_the_next_tick() {
    let mut cluster = Cluster::new(&[0]);
    cluster.register(0, "a", "a:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    assert_eq!(regions_of(&cluster, "a"), [0, 1]);
    assert!(cluster.heartbeat(LEASE, "a"));
    let table = cluster.table();

    // A worker that registers is given nothing that has an owner. The next tick
    // asks the first for one of its regions, the one with the highest number.
    assert_eq!(
        cluster.register(LEASE, "b", "b:25601", &[]),
        Changes::default()
    );
    assert!(cluster.coordinator.releases.is_empty());
    // Nor does a call that ends like a tick begin anything: only a tick does.
    assert_eq!(
        cluster.epoch_refused(LEASE, "b", 0, FIRST_EPOCH),
        Changes::default()
    );
    assert_eq!(
        cluster.tick(LEASE + 1),
        asks(&[order("a", 1, FIRST_EPOCH + 2)])
    );
    assert_eq!(targets(&cluster), [(1, "b")]);
    // Nothing has changed hands yet, and the first is asked only once.
    assert_eq!(cluster.table(), table);
    assert_eq!(cluster.tick(LEASE + 2), Changes::default());

    // It is a release like any other: nobody hears how it ended.
    assert_eq!(
        cluster.released(LEASE + 3, "a", 1, FIRST_EPOCH + 2),
        changes(&["a", "b"], true)
    );
    assert_eq!(
        cluster.assignments("a"),
        [assignment(0, FIRST_EPOCH + 1, 0)]
    );
    assert_eq!(
        cluster.assignments("b"),
        [assignment(1, FIRST_EPOCH + 3, 2)]
    );
    // One each, and that is how it stays.
    nothing_happens(&mut cluster, LEASE + 3, 3, &["a", "b"]);
    assert!(cluster.coordinator.releases.is_empty());
}

#[test]
fn three_regions_on_one_worker_and_a_newcomer_end_as_two_and_one_and_stay_so() {
    let mut cluster = Cluster::new(&[-8, 8]);
    cluster.register(0, "a", "a:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    assert!(cluster.heartbeat(LEASE, "a"));
    cluster.register(LEASE, "b", "b:25601", &[]);
    assert_eq!(
        cluster.tick(LEASE + 1),
        asks(&[order("a", 2, FIRST_EPOCH + 3)])
    );
    assert_eq!(
        cluster.released(LEASE + 2, "a", 2, FIRST_EPOCH + 3),
        changes(&["a", "b"], true)
    );
    assert_eq!(regions_of(&cluster, "a"), [0, 1]);
    assert_eq!(regions_of(&cluster, "b"), [2]);

    // One move, and no more: with another the two would only change places.
    nothing_happens(&mut cluster, LEASE + 2, 3, &["a", "b"]);
    assert!(cluster.coordinator.releases.is_empty());
    assert_eq!(regions_of(&cluster, "a"), [0, 1]);
}

#[test]
fn a_difference_of_one_region_is_left_alone() {
    let mut cluster = Cluster::new(&[-8, 8]);
    cluster.register(0, "a", "a:25601", &[]);
    cluster.register(0, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
    assert_eq!(regions_of(&cluster, "a"), [0, 2]);
    assert_eq!(regions_of(&cluster, "b"), [1]);
    nothing_happens(&mut cluster, LEASE, 2, &["a", "b"]);

    // Nor is a worker that runs nothing given the only region of another.
    let mut cluster = a_runs_and_b_waits();
    nothing_happens(&mut cluster, LEASE, 2, &["a", "b"]);
    assert!(cluster.coordinator.releases.is_empty());
}

#[test]
fn of_the_workers_with_the_most_regions_the_one_that_waited_least_gives_up_its_highest() {
    let mut cluster = Cluster::new(&[-8, 0, 8]);
    cluster.register(0, "a", "a:25601", &[]);
    cluster.register(0, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
    assert_eq!(regions_of(&cluster, "b"), [1, 3]);
    let last = cluster.assignments("b")[1].epoch;
    for name in ["a", "b"] {
        assert!(cluster.heartbeat(LEASE, name));
    }

    cluster.register(LEASE, "c", "c:25601", &[]);
    assert_eq!(cluster.tick(LEASE + 1), asks(&[order("b", 3, last)]));
    assert_eq!(targets(&cluster), [(3, "c")]);
    assert_eq!(
        cluster.released(LEASE + 2, "b", 3, last),
        changes(&["b", "c"], true)
    );
    // Two, one and one: nothing more is to be done.
    nothing_happens(&mut cluster, LEASE + 2, 2, &["a", "b", "c"]);
    assert_eq!(regions_of(&cluster, "a"), [0, 2]);
    assert_eq!(regions_of(&cluster, "c"), [3]);
}

/// For how long a worker that failed a region is at fault, in the milliseconds the
/// tests give times in.
const MEMORY: u64 = Coordinator::FAULT_MEMORY as u64 * LEASE;

/// When `a` fails its region in [`a_fails_and_b_has_both`].
const FAILED: u64 = 2 * LEASE + 1;

/// A world of two regions, of which `a` and `b` ran one each until `a`, which is
/// still there, did not vouch for its own any longer: at `FAILED` it was taken
/// from it and given to `b`.
fn a_fails_and_b_has_both() -> Cluster {
    let mut cluster = Cluster::new(&[0]);
    cluster.register(0, "a", "a:25601", &[]);
    cluster.register(0, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
    assert!(cluster.heartbeat_with(2 * LEASE, "a", &[]));
    assert!(cluster.heartbeat(2 * LEASE, "b"));
    // Not back to the worker that failed it, although that one runs nothing now
    // and the other runs a region.
    assert_eq!(cluster.tick(FAILED), changes(&["a", "b"], true));
    assert!(cluster.assignments("a").is_empty());
    assert_eq!(
        cluster.assignments("b"),
        [
            assignment(0, FIRST_EPOCH + 3, 2),
            assignment(1, FIRST_EPOCH + 2, 1)
        ]
    );
    cluster
}

#[test]
fn a_worker_that_did_not_vouch_is_passed_over_for_six_leases_and_then_counts_again() {
    let mut cluster = a_fails_and_b_has_both();
    // Registering again makes no difference: a worker that lost its connection
    // does that too.
    assert_eq!(
        cluster.register(FAILED, "a", "a:25601", &[]),
        Changes::default()
    );
    // Nothing is evened out towards it either, with two regions against none.
    nothing_happens(&mut cluster, FAILED, 5, &["a", "b"]);
    for name in ["a", "b"] {
        assert!(cluster.heartbeat(FAILED + MEMORY - 1, name));
    }
    assert_eq!(cluster.tick(FAILED + MEMORY - 1), Changes::default());

    // Six leases after it failed, it is a worker like any other.
    assert_eq!(MEMORY, 6 * LEASE);
    assert_eq!(
        cluster.tick(FAILED + MEMORY),
        asks(&[order("b", 1, FIRST_EPOCH + 2)])
    );
    assert_eq!(targets(&cluster), [(1, "a")]);
}

#[test]
fn a_worker_at_fault_is_given_the_region_back_when_nobody_else_can_take_it() {
    // The other worker has no connection.
    let mut cluster = a_runs_and_b_waits();
    assert_eq!(cluster.disconnected(LEASE, "b"), Changes::default());
    for name in ["a", "b"] {
        assert!(cluster.heartbeat_with(2 * LEASE, name, &[]));
    }
    assert_eq!(cluster.tick(2 * LEASE + 1), changes(&["a"], true));
    assert_eq!(cluster.assignments("a"), [assignment(0, NEXT_OWNER, 1)]);

    // Both are at fault, one after the other: the region goes to the one of them
    // that has waited longer since.
    let mut cluster = a_runs_and_b_waits();
    let mut now = LEASE;
    for (lost, next) in [("a", "b"), ("b", "a"), ("a", "b")] {
        now += LEASE + 1;
        for name in ["a", "b"] {
            assert!(cluster.heartbeat_with(now, name, &[]));
        }
        assert_eq!(cluster.tick(now), changes(&["a", "b"], true), "{lost}");
        assert_eq!(cluster.assignments(next).len(), 1, "{next}");
    }
}

#[test]
fn a_worker_that_did_not_answer_a_release_is_passed_over_as_well() {
    let mut cluster = Cluster::new(&[0]);
    cluster.register(0, "a", "a:25601", &[]);
    cluster.register(0, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
    cluster.register(LEASE, "c", "c:25601", &[]);
    cluster.move_region(LEASE + 1, 0, Some("c"), 1).unwrap();
    for name in ["a", "b", "c"] {
        assert!(cluster.heartbeat(2 * LEASE + 1, name));
    }
    // The owner is there and vouches, but does not answer.
    let expected = Changes {
        moves: vec![outcome(1, 0, Some(("c", FIRST_EPOCH + 3)), false)],
        ..changes(&["a", "c"], true)
    };
    assert_eq!(cluster.tick(2 * LEASE + 2), expected);

    // The worker that was given its region dies. The region goes to the worker
    // that runs one, not to the one that runs nothing and has waited longer.
    for name in ["a", "b"] {
        assert!(cluster.heartbeat(3 * LEASE + 2, name));
    }
    assert_eq!(cluster.tick(3 * LEASE + 2), changes(&["b", "c"], true));
    assert_eq!(regions_of(&cluster, "b"), [0, 1]);
    assert!(cluster.assignments("a").is_empty());

    // A worker that releases when it is asked has failed nothing: it is given
    // the next region that loses its owner.
    let mut cluster = Cluster::new(&[0]);
    cluster.register(0, "a", "a:25601", &[]);
    cluster.register(0, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
    cluster.register(LEASE, "c", "c:25601", &[]);
    cluster.move_region(LEASE + 1, 0, Some("c"), 1).unwrap();
    cluster.released(LEASE + 2, "a", 0, FIRST_EPOCH + 1);
    for name in ["a", "b"] {
        assert!(cluster.heartbeat(2 * LEASE + 1, name));
    }
    assert_eq!(cluster.tick(2 * LEASE + 1), changes(&["a", "c"], true));
    assert_eq!(regions_of(&cluster, "a"), [0]);
}

#[test]
fn a_worker_whose_epoch_the_store_refused_is_not_at_fault() {
    let mut cluster = Cluster::new(&[0]);
    cluster.register(0, "a", "a:25601", &[]);
    cluster.register(0, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
    for name in ["a", "b"] {
        assert!(cluster.heartbeat(LEASE, name));
    }
    // What the store has seen is news to the coordinator, not a failure of the
    // worker: it is given the region again, having fewer than the other.
    assert_eq!(
        cluster.epoch_refused(LEASE + 1, "a", 0, FIRST_EPOCH + 50),
        changes(&["a"], true)
    );
    assert_eq!(
        cluster.assignments("a"),
        [assignment(0, FIRST_EPOCH + 51, 2)]
    );
}

#[test]
fn workers_that_are_in_order_are_evened_out_while_another_is_at_fault() {
    let mut cluster = Cluster::new(&[-8, 8]);
    cluster.register(0, "a", "a:25601", &[]);
    cluster.register(0, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
    assert_eq!(regions_of(&cluster, "a"), [0, 2]);
    // One of them stops vouching, and the other has all three regions.
    assert!(cluster.heartbeat_with(2 * LEASE, "a", &[]));
    assert!(cluster.heartbeat(2 * LEASE, "b"));
    assert_eq!(cluster.tick(FAILED), changes(&["a", "b"], true));
    assert_eq!(regions_of(&cluster, "b"), [0, 1, 2]);
    let held = cluster.assignments("b");

    // A third worker comes. A region is moved to it, and not to the one at
    // fault, which has waited longer and runs as little.
    cluster.register(FAILED, "c", "c:25601", &[]);
    assert_eq!(
        cluster.tick(FAILED + 1),
        asks(&[order("b", 2, held[2].epoch)])
    );
    assert_eq!(targets(&cluster), [(2, "c")]);
    assert_eq!(
        cluster.released(FAILED + 2, "b", 2, held[2].epoch),
        changes(&["b", "c"], true)
    );
    // Two and one among those that are in order: the third does not count.
    nothing_happens(&mut cluster, FAILED + 2, 5, &["a", "b", "c"]);
    for name in ["a", "b", "c"] {
        assert!(cluster.heartbeat(FAILED + MEMORY - 1, name));
    }
    assert_eq!(cluster.tick(FAILED + MEMORY - 1), Changes::default());
    // Until its fault is forgotten.
    assert_eq!(
        cluster.tick(FAILED + MEMORY),
        asks(&[order("b", 1, held[1].epoch)])
    );
    assert_eq!(targets(&cluster), [(1, "a")]);
}

#[test]
fn nothing_is_evened_out_from_a_worker_at_fault() {
    let mut cluster = Cluster::new(&[-8, 0, 8]);
    cluster.register(0, "a", "a:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    // It vouches for three of its four regions. The fourth is taken from it
    // and given back, as nobody else is there.
    let three: Vec<(RegionId, Vouch)> =
        Vec::from_iter((0..3).map(|region| (RegionId(region), Vouch::Committed)));
    assert!(cluster.heartbeat_with(2 * LEASE, "a", &three));
    assert_eq!(cluster.tick(FAILED), changes(&["a"], true));
    assert_eq!(regions_of(&cluster, "a"), [0, 1, 2, 3]);
    let last = cluster.assignments("a")[3].epoch;

    // A worker that comes now is not handed any of them as long as the fault is
    // remembered: the workers at fault are left out of evening out altogether.
    cluster.register(FAILED, "b", "b:25601", &[]);
    nothing_happens(&mut cluster, FAILED, 5, &["a", "b"]);
    for name in ["a", "b"] {
        assert!(cluster.heartbeat(FAILED + MEMORY - 1, name));
    }
    assert_eq!(cluster.tick(FAILED + MEMORY - 1), Changes::default());
    assert_eq!(cluster.tick(FAILED + MEMORY), asks(&[order("a", 3, last)]));
}

#[test]
fn a_move_by_name_to_a_worker_at_fault_is_done_and_one_by_choice_passes_it_over() {
    let mut cluster = a_fails_and_b_has_both();
    cluster.register(FAILED, "c", "c:25601", &[]);
    // By choice the region goes to the worker that has not failed one, although
    // the other has waited longer.
    let (moving, _) = cluster.move_region(FAILED, 1, None, 7).unwrap();
    assert_eq!(moving, begun("b", "c"));
    // Whoever names the worker at fault knows better.
    let (moving, _) = cluster.move_region(FAILED, 0, Some("a"), 8).unwrap();
    assert_eq!(moving, begun("b", "a"));
    let expected = Changes {
        moves: vec![outcome(8, 0, Some(("a", FIRST_EPOCH + 4)), true)],
        ..changes(&["a", "b"], true)
    };
    assert_eq!(
        cluster.released(FAILED + 1, "b", 0, FIRST_EPOCH + 3),
        expected
    );
    assert_eq!(regions_of(&cluster, "a"), [0]);
}

#[test]
fn nothing_is_evened_out_during_the_grace_period() {
    let mut cluster = Cluster::new(&[0]);
    // One worker ran both regions under the coordinator before this one, and
    // another ran nothing.
    let held = [assignment(0, 5, 0), assignment(1, 6, 1)];
    cluster.register(0, "a", "a:25601", &held);
    cluster.register(0, "b", "b:25601", &[]);
    for now in [0, 1, LEASE / 2, LEASE - 1] {
        assert_eq!(cluster.tick(now), Changes::default(), "at {now}");
        assert!(cluster.coordinator.releases.is_empty());
    }
    // Workers that have not found the new coordinator yet may be among those
    // with the most regions. After a lease they have had their chance.
    assert_eq!(cluster.tick(LEASE), asks(&[order("a", 1, 6)]));
    assert_eq!(targets(&cluster), [(1, "b")]);
}

#[test]
fn nothing_is_evened_out_while_another_release_is_under_way() {
    let mut cluster = Cluster::new(&[-8, 8]);
    cluster.register(0, "a", "a:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    let held = cluster.assignments("a");
    assert!(cluster.heartbeat(LEASE, "a"));
    cluster.register(LEASE, "b", "b:25601", &[]);
    cluster.register(LEASE, "c", "c:25601", &[]);

    // Somebody asks for a move before the next tick. While that lasts, the
    // coordinator begins none of its own, although one worker has three regions
    // and another neither has one nor is being given one.
    let (moving, _) = cluster.move_region(LEASE + 1, 0, Some("b"), 1).unwrap();
    assert_eq!(moving, begun("a", "b"));
    for now in [LEASE + 2, LEASE + 3] {
        assert_eq!(cluster.tick(now), Changes::default());
    }
    assert_eq!(targets(&cluster), [(0, "b")]);

    // When it has ended, the next tick begins one, and the tick after that
    // begins no second one beside it.
    let expected = Changes {
        moves: vec![outcome(1, 0, Some(("b", FIRST_EPOCH + 4)), true)],
        ..changes(&["a", "b"], true)
    };
    assert_eq!(cluster.released(LEASE + 4, "a", 0, held[0].epoch), expected);
    assert_eq!(
        cluster.tick(LEASE + 5),
        asks(&[order("a", 2, held[2].epoch)])
    );
    assert_eq!(targets(&cluster), [(2, "c")]);
    assert_eq!(cluster.tick(LEASE + 6), Changes::default());
    assert_eq!(
        cluster.released(LEASE + 7, "a", 2, held[2].epoch),
        changes(&["a", "c"], true)
    );
    nothing_happens(&mut cluster, LEASE + 7, 2, &["a", "b", "c"]);
    for (name, region) in [("a", 1), ("b", 0), ("c", 2)] {
        assert_eq!(regions_of(&cluster, name), [region], "{name}");
    }
}

#[test]
fn nothing_is_evened_out_from_or_to_a_worker_without_a_connection() {
    // The worker that has both regions could not be told to release one.
    let mut cluster = Cluster::new(&[0]);
    cluster.register(0, "a", "a:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    let held = cluster.assignments("a");
    assert_eq!(cluster.disconnected(LEASE, "a"), Changes::default());
    cluster.register(LEASE, "b", "b:25601", &[]);
    nothing_happens(&mut cluster, LEASE, 2, &["a", "b"]);
    assert!(cluster.coordinator.releases.is_empty());
    // When it is back, it is.
    assert_eq!(
        cluster.register(3 * LEASE, "a", "a:25601", &held),
        Changes::default()
    );
    assert_eq!(
        cluster.tick(3 * LEASE),
        asks(&[order("a", 1, held[1].epoch)])
    );

    // The worker that has nothing would not hear that it was given a region.
    let mut cluster = Cluster::new(&[0]);
    cluster.register(0, "a", "a:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    cluster.register(LEASE, "b", "b:25601", &[]);
    assert_eq!(cluster.disconnected(LEASE, "b"), Changes::default());
    nothing_happens(&mut cluster, LEASE, 2, &["a", "b"]);
    assert!(cluster.coordinator.releases.is_empty());
}

#[test]
fn nothing_is_evened_out_towards_a_leaving_worker() {
    let mut cluster = Cluster::new(&[-8, 0, 8]);
    // One worker ran three of the regions under the coordinator before this one,
    // and another the fourth.
    let three = [
        assignment(0, 5, 0),
        assignment(1, 6, 1),
        assignment(2, 7, 2),
    ];
    cluster.register(0, "a", "a:25601", &three);
    cluster.register(0, "b", "b:25601", &[assignment(3, 8, 3)]);
    // The one with fewer leaves. Its region is to go to the other one, not the
    // other way round.
    assert_eq!(cluster.leaving(1, "b"), asks(&[order("b", 3, 8)]));
    for name in ["a", "b"] {
        assert!(cluster.heartbeat(LEASE, name));
    }
    assert_eq!(cluster.tick(LEASE), Changes::default());
    assert_eq!(targets(&cluster), [(3, "a")]);

    // A leaving worker that owns something is being asked to release it whenever
    // another worker is there, so that alone holds the coordinator back. With
    // those releases out of the way it still begins nothing: neither towards the
    // worker that leaves, nor from it when it is the one with more regions.
    let now = cluster.at(LEASE);
    cluster.coordinator.releases.clear();
    cluster.coordinator.even_out(now);
    assert!(cluster.coordinator.releases.is_empty());
    assert!(cluster.coordinator.pending.orders.is_empty());

    let mut cluster = Cluster::new(&[-8, 0, 8]);
    cluster.register(0, "a", "a:25601", &three);
    cluster.register(0, "b", "b:25601", &[]);
    let asked = Vec::from_iter(
        three
            .iter()
            .map(|held| order("a", held.region.0, held.epoch)),
    );
    assert_eq!(cluster.leaving(1, "a"), asks(&asked));
    let now = cluster.at(LEASE);
    cluster.coordinator.releases.clear();
    cluster.coordinator.even_out(now);
    assert!(cluster.coordinator.releases.is_empty());
    assert!(cluster.coordinator.pending.orders.is_empty());
}

#[test]
fn a_single_region_goes_to_the_first_worker_and_the_others_wait() {
    let mut cluster = Cluster::new(&[]);
    cluster.register(0, "a", "a:25601", &[]);
    cluster.register(0, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    assert_eq!(
        cluster.assignments("a"),
        [assignment(0, FIRST_EPOCH + 1, 0)]
    );
    assert!(cluster.assignments("b").is_empty());
    let table = cluster.table();
    assert!(table.is_complete());
    assert_eq!(table.routes, [route(0, FIRST_EPOCH + 1, "a:25601")]);

    assert!(cluster.heartbeat(LEASE, "a"));
    assert!(cluster.heartbeat(LEASE, "b"));
    assert_eq!(cluster.tick(2 * LEASE), Changes::default());
}

/// A waiting worker whose connection is gone may be dead, and only its lease running
/// out would show. A region given to it would stand still for that long, so it is
/// given to one that is there, or kept until the worker is back.
#[test]
fn a_region_is_not_given_to_a_waiting_worker_that_has_no_connection() {
    let mut cluster = Cluster::new(&[]);
    // The first to arrive would be the first to be given the region.
    cluster.register(0, "a", "a:25601", &[]);
    cluster.register(0, "b", "b:25601", &[]);
    assert_eq!(cluster.disconnected(0, "a"), Changes::default());
    assert_eq!(cluster.tick(LEASE), changes(&["b"], true));
    assert_eq!(
        cluster.assignments("b"),
        [assignment(0, FIRST_EPOCH + 1, 0)]
    );
    assert!(cluster.assignments("a").is_empty());

    // With nobody else there, the region waits for the worker to be back.
    let mut cluster = Cluster::new(&[]);
    cluster.register(0, "a", "a:25601", &[]);
    assert!(cluster.heartbeat(LEASE, "a"));
    assert_eq!(cluster.disconnected(LEASE, "a"), Changes::default());
    assert_eq!(cluster.tick(LEASE), Changes::default());
    assert!(cluster.table().routes.is_empty());
    cluster.register(LEASE + 1, "a", "a:25601", &[]);
    assert_eq!(cluster.tick(LEASE + 1), changes(&["a"], true));
}

#[test]
fn a_waiting_worker_takes_over_when_the_lease_of_an_owner_runs_out() {
    let mut cluster = Cluster::new(&[0]);
    for name in ["a", "b", "c"] {
        cluster.register(0, name, &format!("{name}:25601"), &[]);
    }
    assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
    assert!(cluster.assignments("c").is_empty());

    // All three are heard from once more, then only two of them.
    for name in ["a", "b", "c"] {
        assert!(cluster.heartbeat(LEASE, name));
    }
    for name in ["b", "c"] {
        assert!(cluster.heartbeat(2 * LEASE, name));
    }
    // Silence for exactly one lease is not too long.
    assert_eq!(cluster.tick(2 * LEASE), Changes::default());
    let version = cluster.table().version;

    assert_eq!(cluster.tick(2 * LEASE + 1), changes(&["a", "c"], true));
    assert!(cluster.assignments("a").is_empty());
    assert_eq!(
        cluster.assignments("b"),
        [assignment(1, FIRST_EPOCH + 2, 1)]
    );
    // A higher epoch than any before, and entity ids nobody has had.
    assert_eq!(
        cluster.assignments("c"),
        [assignment(0, FIRST_EPOCH + 3, 2)]
    );
    let table = cluster.table();
    assert_eq!(table.version, version + 1);
    assert_eq!(
        table.routes,
        [
            route(0, FIRST_EPOCH + 3, "c:25601"),
            route(1, FIRST_EPOCH + 2, "b:25601"),
        ]
    );
    // The worker that was forgotten has to register again.
    assert!(!cluster.heartbeat(2 * LEASE + 1, "a"));
}

#[test]
fn heartbeats_keep_a_lease() {
    let mut cluster = Cluster::new(&[]);
    cluster.register(0, "a", "a:25601", &[]);
    cluster.register(0, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    let held = cluster.assignments("a");

    // One heartbeat per lease is just enough.
    let mut now = 0;
    for _ in 0..50 {
        now += LEASE;
        assert!(cluster.heartbeat(now, "a"));
        assert!(cluster.heartbeat(now, "b"));
        assert_eq!(cluster.tick(now), Changes::default());
    }
    assert_eq!(cluster.assignments("a"), held);

    // Without them the region goes to the worker that waited.
    assert!(cluster.heartbeat(now + LEASE, "b"));
    assert_eq!(cluster.tick(now + LEASE + 1), changes(&["a", "b"], true));
    assert_eq!(
        cluster.assignments("b"),
        [assignment(0, FIRST_EPOCH + 2, 1)]
    );
}

/// The store waits of the tests, in the milliseconds the tests give times in.
const PATIENCE: u64 = Coordinator::STORE_PATIENCE.as_millis() as u64;

const COMMITTED: [(RegionId, Vouch); 1] = [(RegionId(0), Vouch::Committed)];
const WAITING: [(RegionId, Vouch); 1] = [(RegionId(0), Vouch::WaitingForStore)];

/// A world of one region, which `a` runs from `LEASE` on with the epoch after the
/// first, and `b`, which waits. Both are heard from at `LEASE`.
fn a_runs_and_b_waits() -> Cluster {
    let mut cluster = Cluster::new(&[]);
    cluster.register(0, "a", "a:25601", &[]);
    cluster.register(0, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    assert_eq!(
        cluster.assignments("a"),
        [assignment(0, FIRST_EPOCH + 1, 0)]
    );
    for name in ["a", "b"] {
        assert!(cluster.heartbeat_with(LEASE, name, &[]));
    }
    cluster
}

#[test]
fn a_region_not_vouched_for_loses_its_owner_a_lease_after_the_last_vouch() {
    let mut cluster = a_runs_and_b_waits();
    assert!(cluster.heartbeat_with(LEASE + 3000, "a", &COMMITTED));

    // Both go on being heard from, but `a` no longer names the region.
    let mut now = LEASE + 3000;
    while now < 2 * LEASE + 3000 {
        now += 1000;
        for name in ["a", "b"] {
            assert!(cluster.heartbeat_with(now, name, &[]));
        }
        assert_eq!(cluster.tick(now), Changes::default(), "at {now}");
    }
    // A lease after the vouch is not too long; a moment more is.
    assert_eq!(cluster.tick(2 * LEASE + 3001), changes(&["a", "b"], true));
    assert!(cluster.assignments("a").is_empty());
    assert_eq!(
        cluster.assignments("b"),
        [assignment(0, FIRST_EPOCH + 2, 1)]
    );
    // The worker that lost it is still registered, and waits.
    assert!(cluster.heartbeat_with(2 * LEASE + 3001, "a", &COMMITTED));
    assert!(cluster.assignments("a").is_empty());
}

#[test]
fn a_freshly_assigned_region_is_not_taken_away_within_its_first_lease() {
    let mut cluster = a_runs_and_b_waits();
    // The new owner is opening and restoring the region, and says nothing of it.
    for now in [LEASE + 1, LEASE + LEASE / 2, 2 * LEASE - 1, 2 * LEASE] {
        for name in ["a", "b"] {
            assert!(cluster.heartbeat_with(now, name, &[]));
        }
        assert_eq!(cluster.tick(now), Changes::default(), "at {now}");
    }
    assert_eq!(cluster.tick(2 * LEASE + 1), changes(&["a", "b"], true));
}

#[test]
fn a_region_reported_at_a_registration_counts_as_vouched_for_then() {
    let mut cluster = Cluster::new(&[]);
    let held = assignment(0, 7, 0);
    cluster.register(0, "a", "a:25601", &[held]);
    cluster.register(3000, "a", "a:25601", &[held]);
    assert!(cluster.heartbeat_with(LEASE + 3000, "a", &[]));
    assert_eq!(cluster.tick(LEASE + 3000), Changes::default());
    assert_eq!(cluster.tick(LEASE + 3001), changes(&["a"], true));
}

#[test]
fn waiting_for_the_store_keeps_a_region_for_thirty_seconds_and_no_longer() {
    let mut cluster = a_runs_and_b_waits();
    // The run begins with the first heartbeat that says so, and every one of them
    // until thirty seconds after it counts, however long that is in leases.
    let start = LEASE + 2000;
    let mut now = start;
    while now <= start + PATIENCE {
        assert!(cluster.heartbeat_with(now, "a", &WAITING));
        assert!(cluster.heartbeat_with(now, "b", &[]));
        assert_eq!(cluster.tick(now), Changes::default(), "at {now}");
        now += LEASE / 2;
    }
    assert!(cluster.heartbeat_with(start + PATIENCE, "a", &WAITING));
    assert_eq!(cluster.tick(start + PATIENCE), Changes::default());

    // After that the region goes, although `a` still says it waits.
    assert!(cluster.heartbeat_with(start + PATIENCE + 1, "a", &WAITING));
    assert!(cluster.heartbeat_with(start + PATIENCE + 1, "b", &[]));
    assert_eq!(
        cluster.tick(start + PATIENCE + 1),
        changes(&["a", "b"], true)
    );
    assert_eq!(
        cluster.assignments("b"),
        [assignment(0, FIRST_EPOCH + 2, 1)]
    );
}

#[test]
fn a_committed_vouch_ends_a_run_of_waiting_for_the_store() {
    let mut cluster = a_runs_and_b_waits();
    let start = LEASE + 2000;
    let mut now = start;
    let beat = |cluster: &mut Cluster, now: u64, vouches: &[(RegionId, Vouch)]| {
        assert!(cluster.heartbeat_with(now, "a", vouches));
        assert!(cluster.heartbeat_with(now, "b", &[]));
        assert_eq!(cluster.tick(now), Changes::default(), "at {now}");
    };
    while now < start + PATIENCE - 5000 {
        beat(&mut cluster, now, &WAITING);
        now += 5000;
    }
    // The store answers for a moment, and then the worker waits again.
    beat(&mut cluster, now, &COMMITTED);
    let again = now + 5000;
    now = again;
    while now <= again + PATIENCE {
        beat(&mut cluster, now, &WAITING);
        now += 5000;
    }
    assert!(cluster.heartbeat_with(again + PATIENCE + 1, "b", &[]));
    assert_eq!(
        cluster.tick(again + PATIENCE + 1),
        changes(&["a", "b"], true)
    );
}

#[test]
fn registering_again_does_not_end_a_run_of_waiting_for_the_store() {
    let mut cluster = a_runs_and_b_waits();
    let held = cluster.assignments("a");
    let start = LEASE + 2000;
    assert!(cluster.heartbeat_with(start, "a", &WAITING));
    // The worker lost its connection while it waited, and is back.
    cluster.register(start + PATIENCE - 1, "a", "a:25601", &held);
    assert!(cluster.heartbeat_with(start + PATIENCE, "a", &WAITING));
    assert!(cluster.heartbeat_with(start + PATIENCE, "b", &[]));
    assert_eq!(cluster.tick(start + PATIENCE), Changes::default());
    assert_eq!(
        cluster.tick(start + PATIENCE + 1),
        changes(&["a", "b"], true)
    );
}

#[test]
fn a_silent_worker_loses_its_regions_after_a_lease_however_it_vouched() {
    for vouches in [COMMITTED, WAITING] {
        let mut cluster = a_runs_and_b_waits();
        assert!(cluster.heartbeat_with(LEASE + 1000, "a", &vouches));
        assert!(cluster.heartbeat_with(2 * LEASE + 1000, "b", &[]));
        assert_eq!(cluster.tick(2 * LEASE + 1000), Changes::default());
        assert_eq!(cluster.tick(2 * LEASE + 1001), changes(&["a", "b"], true));
        // It is forgotten, not just left waiting.
        assert!(!cluster.heartbeat_with(2 * LEASE + 1001, "a", &vouches));
    }
}

#[test]
fn a_vouch_for_a_region_of_another_worker_does_nothing() {
    let mut cluster = Cluster::new(&[0]);
    cluster.register(0, "a", "a:25601", &[]);
    cluster.register(0, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
    // `b` runs region 1 and names region 0 as well, and a region nobody knows.
    let both = [
        (RegionId(0), Vouch::Committed),
        (RegionId(1), Vouch::Committed),
        (RegionId(9), Vouch::Committed),
    ];
    assert!(cluster.heartbeat_with(2 * LEASE, "a", &[]));
    assert!(cluster.heartbeat_with(2 * LEASE, "b", &both));
    assert_eq!(cluster.tick(2 * LEASE + 1), changes(&["a", "b"], true));
    // It goes to the worker that has not failed it, which keeps its own.
    assert_eq!(
        cluster.assignments("b"),
        [
            assignment(0, FIRST_EPOCH + 3, 2),
            assignment(1, FIRST_EPOCH + 2, 1)
        ]
    );
    assert!(cluster.assignments("a").is_empty());
}

#[test]
fn a_worker_that_lost_a_region_for_not_vouching_waits_behind_the_others() {
    let mut cluster = Cluster::new(&[]);
    for name in ["a", "b", "c"] {
        cluster.register(0, name, &format!("{name}:25601"), &[]);
    }
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    // Each owner in turn stops vouching, and the region goes to the worker that
    // has waited longest without failing: `b` and `c` before `a` gets it again.
    let mut now = LEASE;
    for (lost, next) in [("a", "b"), ("b", "c"), ("c", "a")] {
        now += LEASE + 1;
        for name in ["a", "b", "c"] {
            assert!(cluster.heartbeat_with(now, name, &[]));
        }
        let mut told = [lost, next];
        told.sort_unstable();
        assert_eq!(cluster.tick(now), changes(&told, true));
        assert_eq!(cluster.assignments(next).len(), 1, "{next}");
    }
}

#[test]
fn a_refused_epoch_raises_later_epochs_above_it_and_the_region_is_reassigned() {
    let mut cluster = a_runs_and_b_waits();
    let stale = cluster.assignments("a");
    let version = cluster.table().version;
    let seen = FIRST_EPOCH + 50;

    // The worker has dropped the region, which goes at once to the worker that has
    // waited longest, with an epoch above the one the store has seen. Being refused
    // says nothing against a worker, so that is the same one, as it registered
    // first. Edges see one new table.
    assert_eq!(
        cluster.epoch_refused(LEASE + 100, "a", 0, seen),
        changes(&["a"], true)
    );
    let again = assignment(0, seen + 1, 1);
    assert_eq!(cluster.assignments("a"), [again]);
    assert!(cluster.assignments("b").is_empty());
    assert_eq!(cluster.table().version, version + 1);
    assert_eq!(cluster.table().routes, [route(0, seen + 1, "a:25601")]);
    // What it held before is not honoured again: the store has seen a later owner.
    assert_eq!(
        cluster.register(LEASE + 100, "a", "a:25601", &stale),
        Changes::default()
    );
    assert_eq!(cluster.assignments("a"), [again]);

    // Again, with the epoch the store has seen in the meantime.
    assert_eq!(
        cluster.epoch_refused(LEASE + 300, "a", 0, seen + 9),
        changes(&["a"], true)
    );
    assert_eq!(cluster.assignments("a"), [assignment(0, seen + 10, 2)]);
    // The new owner has its first lease to open the region.
    assert!(cluster.heartbeat_with(2 * LEASE + 300, "a", &[]));
    assert!(cluster.heartbeat_with(2 * LEASE + 300, "b", &[]));
    assert_eq!(cluster.tick(2 * LEASE + 300), Changes::default());
}

#[test]
fn a_region_dropped_for_a_refused_epoch_waits_for_the_grace_period_to_end() {
    let mut cluster = Cluster::new(&[]);
    let held = assignment(0, 5, 0);
    cluster.register(0, "a", "a:25601", &[held]);
    assert_eq!(cluster.epoch_refused(1, "a", 0, 77), changes(&["a"], true));
    assert!(cluster.table().routes.is_empty());
    assert_eq!(cluster.tick(LEASE - 1), Changes::default());
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    assert_eq!(
        cluster.assignments("a"),
        [assignment(0, FIRST_EPOCH + 1, 1)]
    );
}

#[test]
fn a_refusal_takes_nothing_from_an_owner_it_does_not_concern_but_raises_epochs() {
    let mut cluster = Cluster::new(&[0]);
    // The coordinator is new, so the region that this worker does not report
    // stays without an owner for a lease.
    let epoch = FIRST_EPOCH + 1;
    let held = [assignment(0, epoch, 0)];
    cluster.register(0, "a", "a:25601", &held);

    // A refusal under an earlier assignment of the same worker, of a region that
    // another worker runs or nobody does, from a worker that is not registered,
    // and of a region the coordinator does not know.
    let refusals = [
        ("a", 0, epoch),
        ("a", 0, epoch - 1),
        ("b", 0, epoch + 5),
        ("a", 1, epoch + 20),
        ("nobody", 1, epoch + 30),
        ("a", 7, epoch + 40),
    ];
    for (name, region, seen) in refusals {
        assert_eq!(
            cluster.epoch_refused(1, name, region, seen),
            Changes::default(),
            "{name} {region} {seen}"
        );
        assert_eq!(cluster.assignments("a"), held);
    }
    // Every one of them counts for what is issued from now on.
    cluster.register(LEASE, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["b"], true));
    assert_eq!(cluster.assignments("b")[0].epoch, epoch + 41);

    // A region nobody runs does not go back below what the store has seen.
    let mut cluster = Cluster::new(&[]);
    assert_eq!(cluster.epoch_refused(0, "a", 0, 77), Changes::default());
    assert_eq!(
        cluster.register(0, "a", "a:25601", &[assignment(0, 76, 0)]),
        Changes::default()
    );
}

#[test]
fn a_late_heartbeat_counts_if_it_comes_before_the_tick() {
    let mut cluster = Cluster::new(&[]);
    cluster.register(0, "a", "a:25601", &[]);
    cluster.register(0, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));

    // Both were silent for three leases, but nobody looked.
    assert!(cluster.heartbeat(3 * LEASE, "a"));
    assert!(cluster.heartbeat(3 * LEASE, "b"));
    assert_eq!(cluster.tick(3 * LEASE), Changes::default());
    assert_eq!(cluster.tick(4 * LEASE), Changes::default());
}

#[test]
fn a_call_with_an_earlier_time_does_not_shorten_a_lease() {
    let mut cluster = Cluster::new(&[]);
    cluster.register(0, "a", "a:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));

    // The service may well make its calls a little out of order.
    assert!(cluster.heartbeat(2 * LEASE, "a"));
    assert!(cluster.heartbeat(LEASE, "a"));
    cluster.register(LEASE, "a", "a:25601", &[]);
    assert_eq!(cluster.tick(3 * LEASE), Changes::default());
    assert_eq!(cluster.tick(0), Changes::default());
}

/// What a worker says of the players of a region it runs as `held`: a tick of the
/// region, and the chunks with players in them, each with how many.
fn players_of(held: Assignment, tick: u64, crowds: &[((i32, i32), u32)]) -> PlayersOf {
    let crowd = |((x, z), count): &((i32, i32), u32)| (ChunkPos::new(*x, *z), *count);
    PlayersOf {
        region: held.region,
        epoch: held.epoch,
        tick,
        crowds: crowds.iter().map(crowd).collect(),
        redstone: Vec::new(),
    }
}

#[test]
fn a_worker_that_says_where_its_players_are_is_known_and_one_that_never_registered_is_not() {
    let mut cluster = a_runs_and_b_waits();
    let held = cluster.assignments("a")[0];
    let report = [players_of(held, 40, &[((3, -2), 2), ((4, -2), 1)])];
    // Whether it names what it runs, nothing at all, or a region that is not its
    // own.
    assert!(cluster.players(LEASE + 250, "a", &report));
    assert!(cluster.players(LEASE + 250, "a", &[]));
    assert!(cluster.players(LEASE + 250, "b", &report));
    assert!(cluster.players(LEASE + 250, "b", &[]));
    // A worker the coordinator does not know has to register again, as after a
    // heartbeat.
    assert!(!cluster.players(LEASE + 250, "c", &report));
    assert!(!cluster.players(LEASE + 250, "c", &[]));
    // Nothing comes of any of it.
    assert_eq!(cluster.tick(LEASE + 250), Changes::default());
    assert_eq!(cluster.assignments("a"), [held]);
}

#[test]
fn a_worker_that_only_says_where_its_players_are_stays_registered_and_vouches_for_nothing() {
    let mut cluster = a_runs_and_b_waits();
    let held = cluster.assignments("a")[0];
    // `a` says four times a second where the players of its region are, and sends
    // no heartbeat. `b` sends heartbeats.
    let mut now = LEASE;
    while now < 2 * LEASE {
        now += 250;
        let report = [players_of(held, now / 50, &[((0, 0), 1)])];
        assert!(cluster.players(now, "a", &report));
        assert!(cluster.heartbeat(now, "b"));
        assert_eq!(cluster.tick(now), Changes::default(), "at {now}");
    }
    // The region was vouched for when it was assigned, at `LEASE`, and not since:
    // to say where its players are is not to vouch for it. A lease after that was
    // not too long; a moment more is.
    let report = [players_of(held, now / 50 + 1, &[((0, 0), 1)])];
    assert!(cluster.players(2 * LEASE + 1, "a", &report));
    assert!(cluster.heartbeat(2 * LEASE + 1, "b"));
    assert_eq!(cluster.tick(2 * LEASE + 1), changes(&["a", "b"], true));
    assert!(cluster.assignments("a").is_empty());
    assert_eq!(
        cluster.assignments("b"),
        [assignment(0, FIRST_EPOCH + 2, 1)]
    );

    // The worker is heard from all the same, and stays registered for as long as
    // it says so once per lease, which a worker that says nothing does not.
    let mut now = 2 * LEASE + 1;
    for _ in 0..3 {
        now += LEASE;
        assert!(cluster.players(now, "a", &[]));
        assert!(cluster.heartbeat(now, "b"));
        assert_eq!(cluster.tick(now), Changes::default(), "at {now}");
    }
    // Then it says nothing more. A lease of that is not too long; a moment more
    // is, and it has to register again.
    assert!(cluster.heartbeat(now + LEASE, "b"));
    assert_eq!(cluster.tick(now + LEASE), Changes::default());
    assert!(cluster.coordinator.workers.contains_key("a"));
    assert!(cluster.heartbeat(now + LEASE + 1, "b"));
    assert_eq!(cluster.tick(now + LEASE + 1), Changes::default());
    assert!(!cluster.players(now + LEASE + 1, "a", &[]));
    assert!(!cluster.heartbeat(now + LEASE + 1, "a"));
}

#[test]
fn word_of_players_with_an_earlier_time_does_not_shorten_a_lease() {
    let mut cluster = a_runs_and_b_waits();
    // `b` was silent for two leases, but nobody looked. And the service may well
    // make its calls a little out of order.
    assert!(cluster.players(3 * LEASE, "b", &[]));
    assert!(cluster.players(LEASE, "b", &[]));
    for at in [3 * LEASE, 4 * LEASE] {
        assert!(cluster.heartbeat(at, "a"));
        assert_eq!(cluster.tick(at), Changes::default(), "at {at}");
    }
    assert!(cluster.players(4 * LEASE, "b", &[]));
}

#[test]
fn a_waiting_worker_that_is_silent_is_forgotten_without_anybody_being_told() {
    let mut cluster = Cluster::new(&[]);
    for name in ["a", "b", "c"] {
        cluster.register(0, name, &format!("{name}:25601"), &[]);
    }
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));

    assert!(cluster.heartbeat(LEASE, "a"));
    assert!(cluster.heartbeat(LEASE, "c"));
    assert_eq!(cluster.tick(2 * LEASE), Changes::default());
    assert!(!cluster.heartbeat(2 * LEASE, "b"));

    // It registers again, and is now behind the worker that kept waiting.
    cluster.register(2 * LEASE, "b", "b:25601", &[]);
    assert!(cluster.heartbeat(2 * LEASE, "c"));
    assert_eq!(cluster.tick(3 * LEASE), changes(&["a", "c"], true));
    assert!(cluster.assignments("b").is_empty());
}

#[test]
fn nothing_new_is_assigned_during_the_grace_period_but_holdings_are_honoured() {
    let mut cluster = Cluster::new(&[0]);
    assert_eq!(
        cluster.register(0, "idle", "idle:25601", &[]),
        Changes::default()
    );
    let held = assignment(1, 700, 9);
    assert_eq!(
        cluster.register(1, "busy", "busy:25601", &[held]),
        changes(&["busy"], true)
    );
    assert_eq!(cluster.assignments("busy"), [held]);
    assert_eq!(cluster.table().routes, [route(1, 700, "busy:25601")]);
    assert_eq!(cluster.table().version, FIRST_EPOCH + 1);

    for now in [1, LEASE / 2, LEASE - 1] {
        assert_eq!(cluster.tick(now), Changes::default());
        assert!(cluster.assignments("idle").is_empty());
    }
    assert_eq!(cluster.tick(LEASE), changes(&["idle"], true));
    assert_eq!(
        cluster.assignments("idle"),
        [assignment(0, FIRST_EPOCH + 1, 0)]
    );
    assert_eq!(cluster.assignments("busy"), [held]);
}

#[test]
fn workers_keep_what_they_held_under_a_new_coordinator_whatever_the_order() {
    // What the coordinator that is gone had given them.
    let held = [("a", assignment(0, 41, 3)), ("b", assignment(1, 57, 0))];
    for order in [[0, 1], [1, 0]] {
        // The first epoch of the new one is no help: it is below what they hold.
        let mut cluster = Cluster::with_first_epoch(&[0], 5);
        for index in order {
            let (name, holding) = held[index];
            let address = format!("{name}:25601");
            assert_eq!(
                cluster.register(1, name, &address, &[holding]),
                changes(&[name], true)
            );
        }
        cluster.register(2, "c", "c:25601", &[]);
        assert_eq!(cluster.assignments("a"), [held[0].1]);
        assert_eq!(cluster.assignments("b"), [held[1].1]);
        let table = cluster.table();
        assert!(table.is_complete());
        assert_eq!(table.version, 7);
        assert_eq!(
            table.routes,
            [route(0, 41, "a:25601"), route(1, 57, "b:25601")]
        );

        // The grace period ends, and nothing is taken from them.
        for name in ["a", "b", "c"] {
            assert!(cluster.heartbeat(LEASE, name));
        }
        assert_eq!(cluster.tick(LEASE), Changes::default());

        // What is assigned from now on is above every epoch that was reported and
        // has none of the entity ids that were: blocks 0 and 3 are left out.
        let mut now = LEASE;
        let fresh = [(58, 1), (59, 2), (60, 4)];
        for (round, (epoch, block)) in fresh.into_iter().enumerate() {
            let (lost, next) = [("b", "c"), ("c", "d"), ("d", "e")][round];
            cluster.register(now, next, &format!("{next}:25601"), &[]);
            now += LEASE + 1;
            assert!(cluster.heartbeat(now, "a"));
            assert!(cluster.heartbeat(now, next));
            let mut told = [lost, next];
            told.sort_unstable();
            assert_eq!(cluster.tick(now), changes(&told, true));
            assert_eq!(cluster.assignments(next), [assignment(1, epoch, block)]);
        }
        assert_eq!(cluster.assignments("a"), [held[0].1]);
    }
}

#[test]
fn a_new_coordinator_carries_on_above_the_first_epoch_it_is_given() {
    let mut cluster = Cluster::new(&[]);
    cluster.register(0, "a", "a:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    let old = cluster.table();

    // The clock has moved on by the time another coordinator starts.
    cluster.restart(2 * LEASE, FIRST_EPOCH + 500);
    let table = cluster.table();
    assert!(table.version > old.version);
    assert!(table.routes.is_empty());
    assert!(!cluster.heartbeat(2 * LEASE, "a"));

    // The grace period starts again. Nothing is heard of the worker that ran the
    // region, so in the end another one gets it.
    cluster.register(2 * LEASE, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(3 * LEASE - 1), Changes::default());
    assert_eq!(cluster.tick(3 * LEASE), changes(&["b"], true));
    assert_eq!(
        cluster.assignments("b"),
        [assignment(0, FIRST_EPOCH + 501, 0)]
    );
    assert_eq!(cluster.table().version, FIRST_EPOCH + 501);
}

#[test]
fn a_holding_for_a_region_someone_else_owns_is_not_honoured() {
    let mut cluster = Cluster::new(&[0]);
    let held = assignment(0, 7, 0);
    assert_eq!(
        cluster.register(0, "a", "a:25601", &[held]),
        changes(&["a"], true)
    );
    // Neither with a lower epoch than the owner's nor with a higher one.
    let claims = [
        ("b", assignment(0, 5, 1)),
        ("c", assignment(0, FIRST_EPOCH + 50, 2)),
    ];
    for (name, claim) in claims {
        assert_eq!(
            cluster.register(1, name, &format!("{name}:25601"), &[claim]),
            Changes::default()
        );
        assert!(cluster.assignments(name).is_empty());
    }
    assert_eq!(cluster.assignments("a"), [held]);
    assert_eq!(cluster.table().routes, [route(0, 7, "a:25601")]);

    // The two wait like any other worker. What they reported is in use somewhere
    // all the same, so what is issued stays clear of it.
    assert_eq!(cluster.tick(LEASE), changes(&["b"], true));
    assert_eq!(
        cluster.assignments("b"),
        [assignment(1, FIRST_EPOCH + 51, 3)]
    );
    assert!(cluster.assignments("c").is_empty());
}

#[test]
fn a_holding_does_not_take_a_region_back_to_an_epoch_it_has_left_behind() {
    let mut cluster = Cluster::new(&[]);
    cluster.register(0, "a", "a:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    let stale = cluster.assignments("a");

    // The worker is cut off and goes on running the region. Another takes over
    // and is lost in turn, with nobody waiting.
    cluster.register(LEASE, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(2 * LEASE), changes(&["a", "b"], true));
    assert_eq!(cluster.tick(3 * LEASE + 1), changes(&["b"], true));
    assert!(cluster.table().routes.is_empty());

    // Back again, the first worker cannot carry on where it was: storage has seen
    // its successor. It gets the region like a new owner.
    assert_eq!(
        cluster.register(3 * LEASE + 1, "a", "a:25601", &stale),
        Changes::default()
    );
    assert_eq!(cluster.tick(3 * LEASE + 1), changes(&["a"], true));
    assert_eq!(
        cluster.assignments("a"),
        [assignment(0, FIRST_EPOCH + 3, 2)]
    );
}

#[test]
fn a_forgotten_worker_carries_on_with_what_nobody_was_given_since() {
    let mut cluster = Cluster::new(&[]);
    cluster.register(0, "a", "a:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    let held = cluster.assignments("a");
    assert_eq!(cluster.tick(2 * LEASE + 1), changes(&["a"], true));
    assert!(cluster.table().routes.is_empty());

    assert_eq!(
        cluster.register(2 * LEASE + 1, "a", "a:25601", &held),
        changes(&["a"], true)
    );
    assert_eq!(cluster.assignments("a"), held);
    assert_eq!(cluster.tick(2 * LEASE + 1), Changes::default());
}

#[test]
fn a_holding_is_honoured_whatever_entity_ids_it_has() {
    // The world store issues entity ids now: the coordinator only passes on what it
    // is told, and decides nothing by it.
    let mut cluster = Cluster::new(&[0]);
    let held = assignment(0, 3, 4);
    cluster.register(0, "a", "a:25601", &[held]);
    let few = EntityIds {
        first: EntityId(ids(4).first.0 + 5),
        end: EntityId(ids(4).first.0 + 10),
    };
    let claim = Assignment {
        region: RegionId(1),
        epoch: 4,
        entity_ids: few,
    };
    assert_eq!(
        cluster.register(1, "b", "b:25601", &[claim]),
        changes(&["b"], true)
    );
    assert_eq!(cluster.assignments("a"), [held]);
    assert_eq!(cluster.assignments("b"), [claim]);
}

/// Until regions split, a holding for a region the coordinator did not know was
/// turned away. Such a region is one that was split off another now, and nobody
/// but its worker may know of it.
#[test]
fn a_holding_for_a_region_the_coordinator_does_not_know_is_honoured_on_the_workers_word() {
    let mut cluster = Cluster::new(&[]);
    let claim = assignment(1, FIRST_EPOCH + 20, 0);
    assert_eq!(
        cluster.register(0, "a", "a:25601", &[claim]),
        changes(&["a"], true)
    );
    assert_eq!(cluster.assignments("a"), [claim]);
    // The region the coordinator knew still waits for an owner.
    assert!(!cluster.table().is_complete());

    // Its epoch and its entity ids are left out from now on.
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    assert_eq!(
        cluster.assignments("a"),
        [assignment(0, FIRST_EPOCH + 21, 1), claim]
    );
    assert!(cluster.table().is_complete());

    // A reading of the list that is older than the region leaves it alone. It
    // names the home region, which is new to the table.
    let older = stripes(1);
    assert_eq!(cluster.listed(LEASE + 1, &older), changes(&[], true));
    assert_eq!(cluster.assignments("a").len(), 2);
    // One that has no such region, and would have if there were one, takes it
    // away again.
    let list = RegionList {
        next: RegionId(2),
        ..older
    };
    assert_eq!(cluster.listed(LEASE + 2, &list), changes(&["a"], true));
    assert_eq!(
        cluster.assignments("a"),
        [assignment(0, FIRST_EPOCH + 21, 1)]
    );
    // The worker may claim it again all the same: the next reading says.
    let again = [assignment(0, FIRST_EPOCH + 21, 1), claim];
    assert_eq!(
        cluster.register(LEASE + 3, "a", "a:25601", &again),
        changes(&["a"], true)
    );
    // Not once the list has it as absorbed: then the holding is turned away.
    let list = RegionList {
        absorbed: vec![(RegionId(1), RegionId(0))],
        ..list
    };
    assert_eq!(cluster.listed(LEASE + 4, &list), changes(&["a"], true));
    assert_eq!(
        cluster.register(LEASE + 5, "a", "a:25601", &again),
        Changes::default()
    );
    assert_eq!(cluster.assignments("a"), again[..1]);
}

#[test]
fn a_worker_keeps_every_region_it_reports() {
    let mut cluster = Cluster::new(&[0]);
    // The same region several times: the highest epoch is the one that counts.
    let holding = [
        assignment(1, 30, 2),
        assignment(0, 12, 5),
        assignment(0, 14, 6),
        assignment(0, 13, 7),
    ];
    assert_eq!(
        cluster.register(0, "a", "a:25601", &holding),
        changes(&["a"], true)
    );
    assert_eq!(cluster.assignments("a"), [holding[2], holding[0]]);
    let table = cluster.table();
    assert!(table.is_complete());
    // One call, one new version.
    assert_eq!(table.version, FIRST_EPOCH + 1);

    // Nothing is left to give to a worker that runs nothing. Once the coordinator
    // is no longer new, it asks for one of the two for that worker.
    cluster.register(0, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE - 1), Changes::default());
    assert_eq!(cluster.tick(LEASE), asks(&[order("a", 1, 30)]));
    assert_eq!(cluster.assignments("a"), [holding[2], holding[0]]);
}

#[test]
fn what_a_worker_reports_of_its_own_region_replaces_what_was_known() {
    let mut cluster = Cluster::new(&[]);
    cluster.register(0, "a", "a:25601", &[assignment(0, 12, 5)]);
    let version = cluster.table().version;

    // Other entity ids are news to the worker alone.
    let other_ids = assignment(0, 12, 6);
    assert_eq!(
        cluster.register(1, "a", "a:25601", &[other_ids]),
        changes(&["a"], false)
    );
    assert_eq!(cluster.assignments("a"), [other_ids]);
    assert_eq!(cluster.table().version, version);

    // A higher epoch is news to the edges as well.
    let later = assignment(0, 15, 6);
    assert_eq!(
        cluster.register(2, "a", "a:25601", &[later]),
        changes(&["a"], true)
    );
    assert_eq!(cluster.assignments("a"), [later]);
    assert_eq!(cluster.table().version, version + 1);

    // A lower one is behind the worker itself, and stays there.
    assert_eq!(
        cluster.register(3, "a", "a:25601", &[other_ids]),
        Changes::default()
    );
    assert_eq!(cluster.assignments("a"), [later]);
}

#[test]
fn registering_again_replaces_the_address_and_moves_the_table_on() {
    let mut cluster = Cluster::new(&[]);
    cluster.register(0, "a", "old:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    let held = cluster.assignments("a");
    let version = cluster.table().version;

    assert_eq!(
        cluster.register(LEASE + 1, "a", "new:25601", &held),
        changes(&[], true)
    );
    assert_eq!(cluster.assignments("a"), held);
    let table = cluster.table();
    assert_eq!(table.routes, [route(0, FIRST_EPOCH + 1, "new:25601")]);
    assert_eq!(table.version, version + 1);

    // From the same address nothing changes, and what the worker owns stays even
    // if it does not say that it holds it.
    assert_eq!(
        cluster.register(LEASE + 2, "a", "new:25601", &[]),
        Changes::default()
    );
    assert_eq!(cluster.assignments("a"), held);
    assert_eq!(cluster.table(), table);

    // To register is to be heard from, but only a region it reports is vouched for.
    // So the region goes a lease after it was last reported, and as nobody else is
    // waiting, it comes back to the same worker with a new epoch.
    assert_eq!(cluster.tick(2 * LEASE + 1), Changes::default());
    assert_eq!(cluster.tick(2 * LEASE + 2), changes(&["a"], true));
    assert_eq!(
        cluster.assignments("a"),
        [assignment(0, FIRST_EPOCH + 2, 1)]
    );
    // The worker was last heard from when it registered.
    assert_eq!(cluster.tick(2 * LEASE + 3), changes(&["a"], true));
    assert!(cluster.assignments("a").is_empty());
}

#[test]
fn a_waiting_worker_keeps_its_place_when_it_registers_again() {
    let mut cluster = Cluster::new(&[]);
    for name in ["a", "b", "c"] {
        cluster.register(0, name, &format!("{name}:25601"), &[]);
    }
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    let version = cluster.table().version;

    // No edge is sent to a waiting worker, so its address is no news to them.
    assert_eq!(
        cluster.register(LEASE, "b", "elsewhere:25601", &[]),
        Changes::default()
    );
    assert_eq!(cluster.table().version, version);
    assert!(cluster.heartbeat(LEASE, "c"));

    assert_eq!(cluster.tick(2 * LEASE), changes(&["a", "b"], true));
    assert_eq!(
        cluster.table().routes,
        [route(0, FIRST_EPOCH + 2, "elsewhere:25601")]
    );
}

#[test]
fn a_region_stays_without_an_owner_when_the_epochs_run_out() {
    let mut cluster = Cluster::new(&[0]);
    // No epoch is above this one.
    let held = assignment(0, u64::MAX, 0);
    assert_eq!(
        cluster.register(0, "a", "a:25601", &[held]),
        changes(&["a"], true)
    );
    cluster.register(0, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), Changes::default());
    assert!(cluster.assignments("b").is_empty());
    assert_eq!(cluster.assignments("a"), [held]);
}

#[test]
fn reported_entity_ids_are_never_issued_whatever_blocks_they_are_in() {
    let size = EntityIds::BLOCK_SIZE;
    let indices = |first, end| {
        block_indices(EntityIds {
            first: EntityId(first),
            end: EntityId(end),
        })
    };
    for block in [0, 1, 7, EntityIds::BLOCK_COUNT - 1] {
        assert_eq!(block_indices(ids(block)), block..block + 1);
    }
    assert_eq!(indices(size - 1, size + 1), 0..2);
    assert_eq!(indices(3 * size, 5 * size), 3..5);
    assert_eq!(indices(0, 1), 0..1);
    // Every key up to 2^64 is in a block of 2^32 keys, those beyond the regions' too.
    assert_eq!(indices(0, u64::MAX), 0..u32::MAX);
    // Without an id there is nothing to keep clear of.
    for (first, end) in [(5, 5), (9, 2), (u64::MAX, 0)] {
        assert!(indices(first, end).is_empty());
    }

    // A worker holds a region with ids that are not a block as the coordinator
    // would make it. The next owner gets the first block that is clear of them.
    let mut cluster = Cluster::new(&[]);
    let held = Assignment {
        region: RegionId(0),
        epoch: 3,
        entity_ids: EntityIds {
            first: EntityId(size / 2),
            end: EntityId(2 * size + 1),
        },
    };
    cluster.register(0, "a", "a:25601", &[held]);
    assert_eq!(cluster.assignments("a"), [held]);
    cluster.register(LEASE, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(2 * LEASE), changes(&["a", "b"], true));
    assert_eq!(
        cluster.assignments("b"),
        [assignment(0, FIRST_EPOCH + 1, 3)]
    );
}

/// The epoch `a` runs the region with in [`a_runs_and_b_waits`], and the one the
/// next owner gets.
const FIRST_OWNER: u64 = FIRST_EPOCH + 1;
const NEXT_OWNER: u64 = FIRST_EPOCH + 2;

#[test]
fn a_move_asks_the_owner_to_release_and_the_region_goes_to_the_target_when_it_has() {
    // The owner has not vouched for the region once: it may still be restoring it.
    // That makes no difference to the coordinator.
    let mut cluster = a_runs_and_b_waits();
    let version = cluster.table().version;
    let (moving, asked) = cluster.move_region(LEASE + 100, 0, None, 7).unwrap();
    assert_eq!(moving, begun("a", "b"));
    assert_eq!(asked, asks(&[order("a", 0, FIRST_OWNER)]));
    assert_eq!(cluster.table().version, version);

    // No lease is waited for: the region changes hands when its owner says so.
    let expected = Changes {
        moves: vec![outcome(7, 0, Some(("b", NEXT_OWNER)), true)],
        ..changes(&["a", "b"], true)
    };
    assert_eq!(cluster.released(LEASE + 150, "a", 0, FIRST_OWNER), expected);
    assert!(cluster.assignments("a").is_empty());
    assert_eq!(cluster.assignments("b"), [assignment(0, NEXT_OWNER, 1)]);
    assert_eq!(cluster.table().version, version + 1);
    assert_eq!(cluster.table().routes, [route(0, NEXT_OWNER, "b:25601")]);
    assert!(cluster.coordinator.releases.is_empty());

    // The new owner has a lease to open and restore it, and the old one waits.
    for name in ["a", "b"] {
        assert!(cluster.heartbeat_with(2 * LEASE + 150, name, &[]));
    }
    assert_eq!(cluster.tick(2 * LEASE + 150), Changes::default());
    // Saying it again changes nothing.
    assert_eq!(
        cluster.released(2 * LEASE + 150, "a", 0, FIRST_OWNER),
        Changes::default()
    );
}

#[test]
fn a_move_to_a_named_worker_goes_to_that_one_and_not_to_the_one_that_waited_longest() {
    let mut cluster = a_runs_and_b_waits();
    cluster.register(LEASE, "c", "c:25601", &[]);
    let (moving, _) = cluster.move_region(LEASE + 1, 0, Some("c"), 1).unwrap();
    assert_eq!(moving, begun("a", "c"));
    let expected = Changes {
        moves: vec![outcome(1, 0, Some(("c", NEXT_OWNER)), true)],
        ..changes(&["a", "c"], true)
    };
    assert_eq!(cluster.released(LEASE + 2, "a", 0, FIRST_OWNER), expected);
    assert!(cluster.assignments("b").is_empty());
}

#[test]
fn a_move_is_refused_with_the_reason_and_changes_nothing() {
    let mut cluster = Cluster::new(&[0]);
    // The coordinator is new: the region that nobody reports has no owner yet.
    cluster.register(0, "a", "a:25601", &[assignment(0, FIRST_OWNER, 0)]);
    assert_eq!(
        cluster.move_region(1, 1, None, 1),
        Err(MoveRefusal::NoOwner(RegionId(1)))
    );
    // Nobody else is there.
    assert_eq!(
        cluster.move_region(1, 0, None, 1),
        Err(MoveRefusal::NoTarget(RegionId(0)))
    );
    assert_eq!(
        cluster.move_region(1, 5, None, 1),
        Err(MoveRefusal::NoSuchRegion(RegionId(5)))
    );

    cluster.register(LEASE, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["b"], true));
    cluster.register(LEASE, "c", "c:25601", &[]);
    let unfit = |cluster: &mut Cluster, region: u32, worker: &str, why: &'static str| {
        let refusal = MoveRefusal::NotATarget {
            region: RegionId(region),
            worker: worker.to_owned(),
            why,
        };
        assert_eq!(
            cluster.move_region(LEASE + 1, region, Some(worker), 1),
            Err(refusal)
        );
    };
    unfit(&mut cluster, 0, "nobody", "is not registered");
    unfit(&mut cluster, 0, "a", "owns the region");

    // One of the others has lost its connection, and the other one leaves, for
    // which its region is to be released to the only worker left.
    assert_eq!(cluster.disconnected(LEASE + 1, "c"), Changes::default());
    unfit(&mut cluster, 0, "c", "has no connection to the coordinator");
    let second = cluster.assignments("b")[0].epoch;
    assert_eq!(
        cluster.leaving(LEASE + 1, "b"),
        asks(&[order("b", 1, second)])
    );
    unfit(&mut cluster, 0, "b", "is leaving");
    assert_eq!(
        cluster.move_region(LEASE + 1, 0, None, 1),
        Err(MoveRefusal::NoTarget(RegionId(0)))
    );
    let refusal = MoveRefusal::BeingReleased {
        region: RegionId(1),
        from: "b".to_owned(),
        to: "a".to_owned(),
    };
    assert_eq!(
        cluster.move_region(LEASE + 2, 1, None, 1),
        Err(refusal.clone())
    );
    assert_eq!(
        cluster.move_region(LEASE + 2, 1, Some("c"), 1),
        Err(refusal.clone())
    );

    // Whoever asked is told in words.
    assert_eq!(
        refusal.to_string(),
        "region 1 is being released already, by b for a"
    );
    let refusal = MoveRefusal::NotATarget {
        region: RegionId(0),
        worker: "c".to_owned(),
        why: "is leaving",
    };
    assert_eq!(
        refusal.to_string(),
        "region 0 cannot be moved to c, which is leaving"
    );
    assert_eq!(
        MoveRefusal::NoTarget(RegionId(0)).to_string(),
        "no other worker is there that region 0 could be moved to"
    );
}

#[test]
fn a_region_is_moved_to_a_worker_that_runs_one_already_by_name_and_by_choice() {
    // By name, although another worker runs nothing.
    let mut cluster = Cluster::new(&[0]);
    cluster.register(0, "a", "a:25601", &[]);
    cluster.register(0, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
    cluster.register(LEASE, "c", "c:25601", &[]);
    let (moving, asked) = cluster.move_region(LEASE + 1, 0, Some("b"), 1).unwrap();
    assert_eq!(moving, begun("a", "b"));
    assert_eq!(asked, asks(&[order("a", 0, FIRST_EPOCH + 1)]));
    let expected = Changes {
        moves: vec![outcome(1, 0, Some(("b", FIRST_EPOCH + 3)), true)],
        ..changes(&["a", "b"], true)
    };
    assert_eq!(
        cluster.released(LEASE + 2, "a", 0, FIRST_EPOCH + 1),
        expected
    );
    assert_eq!(
        cluster.assignments("b"),
        [
            assignment(0, FIRST_EPOCH + 3, 2),
            assignment(1, FIRST_EPOCH + 2, 1)
        ]
    );
    assert!(cluster.assignments("a").is_empty());
    assert!(cluster.assignments("c").is_empty());

    // By choice, when every other worker runs one: the one that has waited
    // longest of those with the fewest.
    let mut cluster = Cluster::new(&[-8, 0, 8]);
    for name in ["a", "b", "c"] {
        cluster.register(0, name, &format!("{name}:25601"), &[]);
    }
    assert_eq!(cluster.tick(LEASE), changes(&["a", "b", "c"], true));
    assert_eq!(regions_of(&cluster, "a"), [0, 3]);
    let (moving, _) = cluster.move_region(LEASE + 1, 0, None, 2).unwrap();
    assert_eq!(moving, begun("a", "b"));
    let expected = Changes {
        moves: vec![outcome(2, 0, Some(("b", FIRST_EPOCH + 5)), true)],
        ..changes(&["a", "b"], true)
    };
    assert_eq!(
        cluster.released(LEASE + 2, "a", 0, FIRST_EPOCH + 1),
        expected
    );
    assert_eq!(regions_of(&cluster, "b"), [0, 1]);
    // The next by choice goes to the worker with the fewest then, whichever
    // registered first.
    let (moving, _) = cluster.move_region(LEASE + 3, 1, None, 3).unwrap();
    assert_eq!(moving, begun("b", "c"));
    let (moving, _) = cluster.move_region(LEASE + 3, 3, None, 4).unwrap();
    assert_eq!(moving, begun("a", "b"));
}

#[test]
fn a_reserved_target_counts_as_having_the_region_when_the_next_target_is_chosen() {
    let mut cluster = Cluster::new(&[0]);
    cluster.register(0, "a", "a:25601", &[]);
    cluster.register(0, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
    cluster.register(LEASE, "c", "c:25601", &[]);
    cluster.register(LEASE, "d", "d:25601", &[]);
    let (moving, _) = cluster.move_region(LEASE + 100, 0, None, 1).unwrap();
    assert_eq!(moving, begun("a", "c"));
    // A second move does not pick the same worker, which has waited longest of
    // the two that own nothing: it is being given a region just now.
    let (moving, _) = cluster.move_region(LEASE + 100, 1, None, 2).unwrap();
    assert_eq!(moving, begun("b", "d"));
    // Each gets the region it was reserved for.
    let expected = Changes {
        moves: vec![outcome(2, 1, Some(("d", FIRST_EPOCH + 3)), true)],
        ..changes(&["b", "d"], true)
    };
    assert_eq!(
        cluster.released(LEASE + 200, "b", 1, FIRST_EPOCH + 2),
        expected
    );
    let expected = Changes {
        moves: vec![outcome(1, 0, Some(("c", FIRST_EPOCH + 4)), true)],
        ..changes(&["a", "c"], true)
    };
    assert_eq!(
        cluster.released(LEASE + 200, "a", 0, FIRST_EPOCH + 1),
        expected
    );

    // The same when a region without an owner is given away while a release
    // lasts.
    let mut cluster = Cluster::new(&[0]);
    cluster.register(0, "a", "a:25601", &[]);
    cluster.register(0, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
    cluster.register(LEASE, "c", "c:25601", &[]);
    let (moving, _) = cluster.move_region(LEASE + 100, 0, None, 1).unwrap();
    assert_eq!(moving, begun("a", "c"));
    // The other owner dies. The worker that owns nothing counts as having one
    // region, like the worker that is asked to release, which registered before
    // it and is given this one.
    assert!(cluster.heartbeat(2 * LEASE, "a"));
    assert!(cluster.heartbeat(2 * LEASE, "c"));
    assert_eq!(cluster.tick(2 * LEASE + 1), changes(&["a", "b"], true));
    assert_eq!(regions_of(&cluster, "a"), [0, 1]);
    assert!(cluster.assignments("c").is_empty());
    // The release goes on, and ends as it was meant to.
    let expected = Changes {
        moves: vec![outcome(1, 0, Some(("c", FIRST_EPOCH + 4)), true)],
        ..changes(&["a", "c"], true)
    };
    assert_eq!(
        cluster.released(2 * LEASE + 2, "a", 0, FIRST_EPOCH + 1),
        expected
    );
    assert_eq!(regions_of(&cluster, "a"), [1]);
    assert_eq!(regions_of(&cluster, "c"), [0]);
}

#[test]
fn a_reserved_target_can_be_named_for_another_region_and_be_given_one_meanwhile() {
    let mut cluster = Cluster::new(&[0]);
    cluster.register(0, "a", "a:25601", &[]);
    cluster.register(0, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
    cluster.register(LEASE, "c", "c:25601", &[]);
    for (region, from, mover) in [(0, "a", 1), (1, "b", 2)] {
        let (moving, _) = cluster
            .move_region(LEASE + 100, region, Some("c"), mover)
            .unwrap();
        assert_eq!(moving, begun(from, "c"));
    }
    // One owner lets go, and the other does not answer: both regions end up
    // with the worker they were meant for.
    let expected = Changes {
        moves: vec![outcome(2, 1, Some(("c", FIRST_EPOCH + 3)), true)],
        ..changes(&["b", "c"], true)
    };
    assert_eq!(
        cluster.released(LEASE + 200, "b", 1, FIRST_EPOCH + 2),
        expected
    );
    assert_eq!(cluster.coordinator.releases[&RegionId(0)].to, "c");
    for name in ["a", "b", "c"] {
        assert!(cluster.heartbeat(2 * LEASE + 100, name));
    }
    // That leaves it with two regions and the others with none, which the same
    // tick begins to even out, whoever asked for it to be so: one of them is to
    // go to the worker that has waited longer since it lost its own.
    let expected = Changes {
        releases: vec![order("c", 1, FIRST_EPOCH + 3)],
        moves: vec![outcome(1, 0, Some(("c", FIRST_EPOCH + 4)), false)],
        ..changes(&["a", "c"], true)
    };
    assert_eq!(cluster.tick(2 * LEASE + 101), expected);
    assert_eq!(regions_of(&cluster, "c"), [0, 1]);
    assert_eq!(cluster.coordinator.releases[&RegionId(1)].to, "b");
}

#[test]
fn a_release_is_dropped_when_its_owner_dies_and_takes_nothing_from_the_next_one() {
    let mut cluster = a_runs_and_b_waits();
    cluster.register(LEASE, "c", "c:25601", &[]);
    let (moving, _) = cluster.move_region(LEASE + 100, 0, None, 9).unwrap();
    assert_eq!(moving, begun("a", "b"));

    // The owner's lease runs out before the release does. The region is assigned
    // like any that lost its owner, and whoever asked is told that the owner did
    // not let go of it.
    assert!(cluster.heartbeat(2 * LEASE, "b"));
    assert!(cluster.heartbeat(2 * LEASE, "c"));
    let expected = Changes {
        moves: vec![outcome(9, 0, Some(("b", NEXT_OWNER)), false)],
        ..changes(&["a", "b"], true)
    };
    assert_eq!(cluster.tick(2 * LEASE + 1), expected);
    assert!(cluster.coordinator.releases.is_empty());

    // When the release would have run out, the region stays with its new owner,
    // and what the old one says of it now changes nothing.
    for name in ["b", "c"] {
        assert!(cluster.heartbeat(2 * LEASE + 101, name));
    }
    assert_eq!(cluster.tick(2 * LEASE + 101), Changes::default());
    assert_eq!(
        cluster.released(2 * LEASE + 101, "a", 0, FIRST_OWNER),
        Changes::default()
    );
    assert_eq!(cluster.assignments("b"), [assignment(0, NEXT_OWNER, 1)]);
}

#[test]
fn a_release_is_dropped_when_the_store_refuses_the_owners_epoch_and_its_target_is_free() {
    let mut cluster = a_runs_and_b_waits();
    let (moving, _) = cluster.move_region(LEASE + 100, 0, None, 9).unwrap();
    assert_eq!(moving, begun("a", "b"));

    // The region is given to the worker that registered first, as after any
    // refusal, which is the same one. It is another tenure, which nobody asked to
    // have released.
    let seen = FIRST_EPOCH + 50;
    let expected = Changes {
        moves: vec![outcome(9, 0, Some(("a", seen + 1)), false)],
        ..changes(&["a"], true)
    };
    assert_eq!(cluster.epoch_refused(LEASE + 200, "a", 0, seen), expected);
    assert!(cluster.coordinator.releases.is_empty());
    for name in ["a", "b"] {
        assert!(cluster.heartbeat(2 * LEASE + 101, name));
    }
    assert_eq!(cluster.tick(2 * LEASE + 101), Changes::default());
    assert_eq!(cluster.assignments("a"), [assignment(0, seen + 1, 1)]);

    // The worker that was reserved can be picked again.
    let (moving, _) = cluster.move_region(2 * LEASE + 101, 0, None, 10).unwrap();
    assert_eq!(moving, begun("a", "b"));
}

#[test]
fn a_region_that_is_being_released_is_not_taken_for_want_of_vouching() {
    let mut cluster = a_runs_and_b_waits();
    assert!(cluster.heartbeat_with(LEASE + 3000, "a", &COMMITTED));
    // The owner is asked shortly before its last vouch is a lease old, and stops
    // ticking, as it should. It is heard from all the while.
    let asked = 2 * LEASE + 2000;
    cluster.move_region(asked, 0, None, 3).unwrap();
    for now in [asked, asked + LEASE] {
        for name in ["a", "b"] {
            assert!(cluster.heartbeat_with(now, name, &[]));
        }
    }
    // Without the release this tick would have taken the region.
    assert_eq!(cluster.tick(2 * LEASE + 3001), Changes::default());
    assert_eq!(cluster.tick(asked + LEASE), Changes::default());
    assert_eq!(cluster.assignments("a"), [assignment(0, FIRST_OWNER, 0)]);
}

#[test]
fn a_release_that_is_not_answered_within_a_lease_of_being_asked_ends_like_a_death() {
    let mut cluster = a_runs_and_b_waits();
    let held = cluster.assignments("a");
    let asked = LEASE + 500;
    cluster.move_region(asked, 0, None, 4).unwrap();

    // The owner loses its connection and is asked again when it is back. That
    // does not give it more time.
    assert_eq!(cluster.disconnected(asked + 100, "a"), Changes::default());
    assert_eq!(
        cluster.register(2 * LEASE, "a", "a:25601", &held),
        asks(&[order("a", 0, FIRST_OWNER)])
    );
    assert!(cluster.heartbeat(2 * LEASE, "a"));
    assert!(cluster.heartbeat(2 * LEASE, "b"));

    // A lease is not too long; a moment more is. The region goes to the worker
    // that was reserved for it, and whoever asked is told that it was not let go.
    assert_eq!(cluster.tick(asked + LEASE), Changes::default());
    let expected = Changes {
        moves: vec![outcome(4, 0, Some(("b", NEXT_OWNER)), false)],
        ..changes(&["a", "b"], true)
    };
    assert_eq!(cluster.tick(asked + LEASE + 1), expected);
    assert_eq!(cluster.assignments("b"), [assignment(0, NEXT_OWNER, 1)]);
    // The worker that did not answer is still registered, and waits.
    assert!(cluster.heartbeat(asked + LEASE + 1, "a"));
    assert!(cluster.assignments("a").is_empty());
}

#[test]
fn an_owner_that_registers_again_is_asked_again_and_without_the_region_it_has_released() {
    let mut cluster = a_runs_and_b_waits();
    let held = cluster.assignments("a");
    cluster.move_region(LEASE + 100, 0, None, 4).unwrap();

    // Whether `Release` reached it nobody knows, so it is told again, as often as
    // it registers with the region.
    for now in [LEASE + 200, LEASE + 300] {
        assert_eq!(
            cluster.register(now, "a", "a:25601", &held),
            asks(&[order("a", 0, FIRST_OWNER)])
        );
    }
    assert_eq!(cluster.assignments("a"), held);

    // It released the region, and `Released` was lost with the connection. What it
    // holds when it is back says the same.
    let expected = Changes {
        moves: vec![outcome(4, 0, Some(("b", NEXT_OWNER)), true)],
        ..changes(&["a", "b"], true)
    };
    assert_eq!(cluster.register(LEASE + 400, "a", "a:25601", &[]), expected);
    assert!(cluster.assignments("a").is_empty());
    assert_eq!(cluster.assignments("b"), [assignment(0, NEXT_OWNER, 1)]);

    // Without a release, a worker that registers without its region keeps it, as
    // ever: only its vouching says whether it runs it.
    assert_eq!(
        cluster.register(LEASE + 500, "b", "b:25601", &[]),
        Changes::default()
    );
    assert_eq!(cluster.assignments("b"), [assignment(0, NEXT_OWNER, 1)]);
}

#[test]
fn a_holding_with_another_epoch_is_not_the_region_the_owner_was_asked_to_release() {
    let mut cluster = a_runs_and_b_waits();
    cluster.move_region(LEASE + 100, 0, None, 4).unwrap();
    // The worker runs something older, which is not honoured; it does not hold
    // what it was asked to release.
    let stale = assignment(0, FIRST_OWNER - 1, 0);
    let expected = Changes {
        moves: vec![outcome(4, 0, Some(("b", NEXT_OWNER)), true)],
        ..changes(&["a", "b"], true)
    };
    assert_eq!(
        cluster.register(LEASE + 200, "a", "a:25601", &[stale]),
        expected
    );

    // A worker that reports its region with a later epoch than the coordinator
    // knows has another tenure, which the release was not of.
    let mut cluster = a_runs_and_b_waits();
    cluster.move_region(LEASE + 100, 0, None, 5).unwrap();
    let later = assignment(0, FIRST_OWNER + 20, 0);
    let expected = Changes {
        moves: vec![outcome(5, 0, Some(("a", FIRST_OWNER + 20)), false)],
        ..changes(&["a"], true)
    };
    assert_eq!(
        cluster.register(LEASE + 200, "a", "a:25601", &[later]),
        expected
    );
    assert!(cluster.coordinator.releases.is_empty());
}

#[test]
fn released_from_anyone_but_the_owner_with_its_epoch_changes_nothing() {
    let mut cluster = a_runs_and_b_waits();
    cluster.move_region(LEASE + 100, 0, None, 4).unwrap();
    let said = [
        ("b", 0, FIRST_OWNER),
        ("a", 0, FIRST_OWNER - 1),
        ("a", 0, FIRST_OWNER + 1),
        ("a", 1, FIRST_OWNER),
        ("nobody", 0, FIRST_OWNER),
    ];
    for (name, region, epoch) in said {
        assert_eq!(
            cluster.released(LEASE + 200, name, region, epoch),
            Changes::default(),
            "{name} {region} {epoch}"
        );
    }
    assert_eq!(cluster.assignments("a"), [assignment(0, FIRST_OWNER, 0)]);
    assert_eq!(cluster.coordinator.releases.len(), 1);

    // It is a sign of life all the same: both were last heard from a lease ago
    // otherwise.
    assert_eq!(cluster.tick(2 * LEASE + 1), Changes::default());
    assert!(cluster.heartbeat_with(2 * LEASE + 1, "a", &[]));
    assert!(cluster.heartbeat_with(2 * LEASE + 1, "b", &[]));
}

#[test]
fn a_region_released_without_being_asked_is_assigned_at_once_even_by_a_new_coordinator() {
    // The coordinator before this one asked for the release. This one knows
    // nothing of it and gives nothing away yet, but the owner itself says that
    // the region is free.
    let mut cluster = Cluster::new(&[]);
    let held = assignment(0, 7, 0);
    assert_eq!(
        cluster.register(1, "a", "a:25601", &[held]),
        changes(&["a"], true)
    );
    cluster.register(2, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(3), Changes::default());
    assert_eq!(cluster.released(4, "a", 0, 7), changes(&["a", "b"], true));
    assert_eq!(
        cluster.assignments("b"),
        [assignment(0, FIRST_EPOCH + 1, 1)]
    );
}

#[test]
fn a_region_released_with_nobody_to_take_it_goes_to_the_first_that_waits_at_a_tick() {
    let mut cluster = Cluster::new(&[0]);
    let held = assignment(0, 7, 0);
    cluster.register(1, "a", "a:25601", &[held]);
    assert_eq!(cluster.released(2, "a", 0, 7), changes(&["a"], true));
    assert!(cluster.table().routes.is_empty());

    // The grace period does not count for it: its owner said that it is free. The
    // worker that released it is the first that waits, being the only one. The
    // other region, of which nobody said anything, waits for the grace period.
    assert_eq!(cluster.tick(3), changes(&["a"], true));
    assert_eq!(
        cluster.assignments("a"),
        [assignment(0, FIRST_EPOCH + 1, 1)]
    );
    cluster.register(4, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE - 1), Changes::default());
    assert_eq!(cluster.tick(LEASE), changes(&["b"], true));
}

#[test]
fn a_region_released_before_a_new_coordinator_knew_of_it_is_assigned_at_once() {
    // The owner was asked by the coordinator before this one, and had let go
    // before it found this one: it registers holding nothing and says so.
    let mut cluster = Cluster::new(&[]);
    assert_eq!(cluster.register(1, "a", "a:25601", &[]), Changes::default());
    assert_eq!(cluster.register(2, "b", "b:25601", &[]), Changes::default());
    assert_eq!(cluster.released(3, "a", 0, 7), changes(&["b"], true));
    assert_eq!(
        cluster.assignments("b"),
        [assignment(0, FIRST_EPOCH + 1, 0)]
    );
    assert!(cluster.assignments("a").is_empty());
}

#[test]
fn a_region_released_before_anyone_waits_goes_to_the_next_that_registers_and_ticks() {
    let mut cluster = Cluster::new(&[]);
    cluster.register(1, "a", "a:25601", &[]);
    // It may be given back to the worker that let go of it, if that is all there is.
    assert_eq!(cluster.released(2, "a", 0, 7), changes(&["a"], true));
    assert_eq!(
        cluster.assignments("a"),
        [assignment(0, FIRST_EPOCH + 1, 0)]
    );
}

#[test]
fn a_release_from_before_does_not_free_a_region_that_has_an_owner_or_a_later_epoch() {
    let mut cluster = Cluster::new(&[]);
    cluster.register(1, "a", "a:25601", &[]);
    cluster.register(2, "b", "b:25601", &[assignment(0, 9, 0)]);
    // Another worker reported it first.
    assert_eq!(cluster.released(3, "a", 0, 7), Changes::default());
    assert_eq!(cluster.released(3, "a", 0, 9), Changes::default());
    assert_eq!(cluster.assignments("b"), [assignment(0, 9, 0)]);

    // Its owner lets go with nobody fit to take it; an epoch from before that owner
    // says nothing about the region as it is now.
    cluster.disconnected(4, "a");
    cluster.released(5, "b", 0, 9);
    cluster.disconnected(6, "b");
    assert_eq!(cluster.released(7, "a", 0, 7), Changes::default());
    // Nor does a worker that is not registered free anything.
    assert_eq!(cluster.released(7, "nobody", 0, 9), Changes::default());
    assert!(cluster.table().routes.is_empty());
}

#[test]
fn a_worker_that_released_a_region_waits_behind_the_others() {
    let mut cluster = Cluster::new(&[]);
    for name in ["a", "b", "c"] {
        cluster.register(0, name, &format!("{name}:25601"), &[]);
    }
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    // Each owner in turn lets go, and the region goes to the worker that has
    // waited longest: `b` and `c` before `a` gets it again.
    let mut epoch = FIRST_OWNER;
    for (releases, next) in [("a", "b"), ("b", "c"), ("c", "a")] {
        let mut told = [releases, next];
        told.sort_unstable();
        assert_eq!(
            cluster.released(LEASE + 1, releases, 0, epoch),
            changes(&told, true)
        );
        epoch += 1;
        assert_eq!(cluster.assignments(next)[0].epoch, epoch, "{next}");
    }
}

#[test]
fn the_region_goes_to_another_target_when_the_reserved_one_is_no_longer_one() {
    let mut cluster = a_runs_and_b_waits();
    cluster.register(LEASE, "c", "c:25601", &[]);
    let (moving, _) = cluster.move_region(LEASE + 100, 0, Some("b"), 5).unwrap();
    assert_eq!(moving, begun("a", "b"));
    assert_eq!(cluster.disconnected(LEASE + 200, "b"), Changes::default());

    // Whoever asked is told whom the region went to, which no routing table says.
    let expected = Changes {
        moves: vec![outcome(5, 0, Some(("c", NEXT_OWNER)), true)],
        ..changes(&["a", "c"], true)
    };
    assert_eq!(cluster.released(LEASE + 300, "a", 0, FIRST_OWNER), expected);
    assert!(cluster.assignments("b").is_empty());
}

#[test]
fn whoever_asked_is_told_when_the_region_ends_up_without_an_owner() {
    let mut cluster = a_runs_and_b_waits();
    cluster.move_region(LEASE + 100, 0, None, 5).unwrap();
    assert_eq!(cluster.disconnected(LEASE + 200, "b"), Changes::default());
    let expected = Changes {
        moves: vec![outcome(5, 0, None, true)],
        ..changes(&["a"], true)
    };
    assert_eq!(cluster.released(LEASE + 300, "a", 0, FIRST_OWNER), expected);
    assert!(cluster.table().routes.is_empty());
    assert!(cluster.coordinator.releases.is_empty());

    // When the owner does not answer and nobody else is left, there is somebody
    // to name after all.
    let mut cluster = a_runs_and_b_waits();
    cluster.move_region(LEASE + 100, 0, None, 6).unwrap();
    cluster.leaving(LEASE + 200, "b");
    assert!(cluster.heartbeat(2 * LEASE, "a"));
    let told = cluster.tick(2 * LEASE + 101);
    assert_eq!(told.moves, [outcome(6, 0, Some(("a", NEXT_OWNER)), false)]);
    // Nobody else waits, so the tick that took the region gave it back to the
    // worker it was taken from, with a new epoch.
    assert_eq!(cluster.assignments("a"), [assignment(0, NEXT_OWNER, 1)]);
}

#[test]
fn a_worker_without_a_connection_is_no_target_until_it_registers_again() {
    let mut cluster = a_runs_and_b_waits();
    assert_eq!(cluster.disconnected(LEASE + 1, "b"), Changes::default());
    assert_eq!(
        cluster.move_region(LEASE + 2, 0, None, 1),
        Err(MoveRefusal::NoTarget(RegionId(0)))
    );
    // Nor is the owner asked to release its region for it when it leaves.
    assert_eq!(cluster.leaving(LEASE + 2, "a"), Changes::default());

    // An earlier connection that ends after the worker is back is none of the
    // coordinator's business: the service only tells it of the latest.
    assert_eq!(
        cluster.register(LEASE + 3, "b", "b:25601", &[]),
        asks(&[order("a", 0, FIRST_OWNER)])
    );
    assert_eq!(cluster.coordinator.releases[&RegionId(0)].to, "b");
}

#[test]
fn a_leaving_worker_that_owns_nothing_is_forgotten_at_once() {
    let mut cluster = a_runs_and_b_waits();
    let gone = |name: &str| Changes {
        gone: vec![name.to_owned()],
        ..Changes::default()
    };
    assert_eq!(cluster.leaving(LEASE + 1, "b"), gone("b"));
    assert!(!cluster.heartbeat(LEASE + 1, "b"));
    // One the coordinator does not know has nothing to wait for either.
    assert_eq!(cluster.leaving(LEASE + 1, "nobody"), gone("nobody"));
    assert_eq!(cluster.leaving(LEASE + 1, "b"), gone("b"));
}

#[test]
fn the_region_of_a_leaving_worker_is_released_as_soon_as_a_worker_is_there_to_take_it() {
    let mut cluster = Cluster::new(&[]);
    cluster.register(0, "a", "a:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));

    // Nobody is there, so the worker goes on running its region.
    assert_eq!(cluster.leaving(LEASE + 1, "a"), Changes::default());
    assert!(cluster.heartbeat(LEASE + 2, "a"));
    assert_eq!(cluster.tick(LEASE + 2), Changes::default());

    // The spare registers a moment later, and the worker is asked at once, and
    // only once.
    assert_eq!(
        cluster.register(LEASE + 3, "b", "b:25601", &[]),
        asks(&[order("a", 0, FIRST_OWNER)])
    );
    assert_eq!(cluster.tick(LEASE + 3), Changes::default());
    assert_eq!(cluster.leaving(LEASE + 3, "a"), Changes::default());

    // When it owns nothing any more, it is forgotten.
    let expected = Changes {
        gone: vec!["a".to_owned()],
        ..changes(&["a", "b"], true)
    };
    assert_eq!(cluster.released(LEASE + 4, "a", 0, FIRST_OWNER), expected);
    assert_eq!(cluster.assignments("b"), [assignment(0, NEXT_OWNER, 1)]);
    assert!(!cluster.heartbeat(LEASE + 4, "a"));
}

#[test]
fn a_worker_that_leaves_while_another_waits_is_asked_to_release_at_once() {
    let mut cluster = a_runs_and_b_waits();
    assert_eq!(
        cluster.leaving(LEASE + 1, "a"),
        asks(&[order("a", 0, FIRST_OWNER)])
    );
    // Nobody asked for this move, so nobody is told how it ends.
    let expected = Changes {
        gone: vec!["a".to_owned()],
        ..changes(&["a", "b"], true)
    };
    assert_eq!(cluster.released(LEASE + 2, "a", 0, FIRST_OWNER), expected);
}

#[test]
fn a_leaving_worker_with_two_regions_hands_both_over_and_is_let_go() {
    let mut cluster = Cluster::new(&[-8, 0, 8]);
    cluster.register(0, "a", "a:25601", &[]);
    cluster.register(0, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
    assert_eq!(regions_of(&cluster, "a"), [0, 2]);
    let held = cluster.assignments("a");

    // It is asked to release both at once, for the only other worker there is,
    // which runs two regions itself.
    let both = [order("a", 0, held[0].epoch), order("a", 2, held[1].epoch)];
    assert_eq!(cluster.leaving(LEASE + 1, "a"), asks(&both));
    let meant =
        |cluster: &Cluster, region: u32| cluster.coordinator.releases[&RegionId(region)].to.clone();
    assert_eq!(
        (meant(&cluster, 0), meant(&cluster, 2)),
        ("b".into(), "b".into())
    );

    // It lets go of one and is still there, as it has another.
    assert_eq!(
        cluster.released(LEASE + 2, "a", 2, held[1].epoch),
        changes(&["a", "b"], true)
    );
    assert_eq!(regions_of(&cluster, "a"), [0]);
    assert!(cluster.heartbeat(LEASE + 2, "a"));
    // With the last one it is forgotten.
    let expected = Changes {
        gone: vec!["a".to_owned()],
        ..changes(&["a", "b"], true)
    };
    assert_eq!(cluster.released(LEASE + 3, "a", 0, held[0].epoch), expected);
    assert_eq!(regions_of(&cluster, "b"), [0, 1, 2, 3]);
    assert!(!cluster.heartbeat(LEASE + 3, "a"));
    assert!(cluster.coordinator.releases.is_empty());
    assert!(cluster.table().is_complete());

    // With two workers that run nothing, each is the target of one of them.
    let mut cluster = Cluster::new(&[0]);
    cluster.register(0, "a", "a:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    let held = cluster.assignments("a");
    cluster.register(LEASE, "b", "b:25601", &[]);
    cluster.register(LEASE, "c", "c:25601", &[]);
    let both = [order("a", 0, held[0].epoch), order("a", 1, held[1].epoch)];
    assert_eq!(cluster.leaving(LEASE + 1, "a"), asks(&both));
    assert_eq!(
        (meant(&cluster, 0), meant(&cluster, 1)),
        ("b".into(), "c".into())
    );
    // It answers neither. Both regions are taken from it when the lease is out,
    // and it is forgotten.
    for name in ["a", "b", "c"] {
        assert!(cluster.heartbeat(2 * LEASE + 1, name));
    }
    assert_eq!(cluster.tick(2 * LEASE + 1), Changes::default());
    let expected = Changes {
        gone: vec!["a".to_owned()],
        ..changes(&["a", "b", "c"], true)
    };
    assert_eq!(cluster.tick(2 * LEASE + 2), expected);
    assert_eq!(regions_of(&cluster, "b"), [0]);
    assert_eq!(regions_of(&cluster, "c"), [1]);
}

#[test]
fn a_leaving_worker_is_asked_although_the_only_target_is_reserved_for_another_release() {
    let mut cluster = Cluster::new(&[0]);
    cluster.register(0, "a", "a:25601", &[]);
    cluster.register(0, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
    cluster.register(LEASE, "c", "c:25601", &[]);
    let (moving, _) = cluster.move_region(LEASE + 100, 1, None, 1).unwrap();
    assert_eq!(moving, begun("b", "c"));
    // The owner that is asked has lost its connection, so the worker that is
    // reserved for its region is all there is for the one that leaves.
    assert_eq!(cluster.disconnected(LEASE + 150, "b"), Changes::default());
    assert_eq!(
        cluster.leaving(LEASE + 200, "a"),
        asks(&[order("a", 0, FIRST_EPOCH + 1)])
    );
    assert_eq!(cluster.coordinator.releases[&RegionId(0)].to, "c");

    let expected = Changes {
        gone: vec!["a".to_owned()],
        ..changes(&["a", "c"], true)
    };
    assert_eq!(
        cluster.released(LEASE + 300, "a", 0, FIRST_EPOCH + 1),
        expected
    );
    assert_eq!(regions_of(&cluster, "c"), [0]);
    // The other release goes on.
    assert_eq!(cluster.coordinator.releases[&RegionId(1)].to, "c");
}

#[test]
fn leaving_belongs_to_one_registration_and_not_to_a_name() {
    let mut cluster = Cluster::new(&[]);
    cluster.register(0, "a", "a:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    let held = cluster.assignments("a");
    assert_eq!(cluster.leaving(LEASE + 1, "a"), Changes::default());

    // The worker registers again and has not said that it leaves since. So it is
    // not asked to release anything when a spare turns up.
    assert_eq!(
        cluster.register(LEASE + 2, "a", "a:25601", &held),
        Changes::default()
    );
    assert_eq!(
        cluster.register(LEASE + 3, "b", "b:25601", &[]),
        Changes::default()
    );
    assert_eq!(cluster.tick(LEASE + 3), Changes::default());

    // It says so again, hands over and is forgotten.
    assert_eq!(
        cluster.leaving(LEASE + 4, "a"),
        asks(&[order("a", 0, FIRST_OWNER)])
    );
    let told = cluster.released(LEASE + 5, "a", 0, FIRST_OWNER);
    assert_eq!(told.gone, ["a"]);

    // The pod that replaces it has its name. It is a worker like any other: the
    // next one to leave hands over to it.
    assert_eq!(
        cluster.register(LEASE + 6, "a", "a:25601", &[]),
        Changes::default()
    );
    assert_eq!(
        cluster.leaving(LEASE + 7, "b"),
        asks(&[order("b", 0, NEXT_OWNER)])
    );
    let expected = Changes {
        gone: vec!["b".to_owned()],
        ..changes(&["a", "b"], true)
    };
    assert_eq!(cluster.released(LEASE + 8, "b", 0, NEXT_OWNER), expected);
    assert_eq!(
        cluster.assignments("a"),
        [assignment(0, FIRST_EPOCH + 3, 2)]
    );
}

#[test]
fn a_leaving_worker_is_not_given_a_region_again_that_was_taken_from_it() {
    let mut cluster = Cluster::new(&[]);
    cluster.register(0, "a", "a:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    assert_eq!(cluster.leaving(LEASE, "a"), Changes::default());

    // It is there but does not vouch for its region, which is not being released,
    // as there is nobody to release it for. A worker that is not leaving would be
    // given the region again, being the only one that waits.
    assert!(cluster.heartbeat_with(2 * LEASE + 1, "a", &[]));
    let expected = Changes {
        gone: vec!["a".to_owned()],
        ..changes(&["a"], true)
    };
    assert_eq!(cluster.tick(2 * LEASE + 1), expected);
    assert!(cluster.table().routes.is_empty());
}

#[test]
fn a_leaving_worker_that_does_not_release_in_time_loses_its_region_and_is_forgotten() {
    let mut cluster = a_runs_and_b_waits();
    cluster.leaving(LEASE + 1, "a");
    for name in ["a", "b"] {
        assert!(cluster.heartbeat(2 * LEASE, name));
    }
    assert_eq!(cluster.tick(2 * LEASE + 1), Changes::default());
    let expected = Changes {
        gone: vec!["a".to_owned()],
        ..changes(&["a", "b"], true)
    };
    assert_eq!(cluster.tick(2 * LEASE + 2), expected);
    assert_eq!(cluster.assignments("b"), [assignment(0, NEXT_OWNER, 1)]);
}

#[test]
fn a_leaving_worker_whose_connection_ends_is_gone_at_once_and_any_other_keeps_its_region() {
    let mut cluster = Cluster::new(&[0]);
    cluster.register(0, "a", "a:25601", &[]);
    cluster.register(0, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
    // The other worker is there to hand over to, although it runs a region.
    assert_eq!(
        cluster.leaving(LEASE + 1, "a"),
        asks(&[order("a", 0, FIRST_EPOCH + 1)])
    );
    assert!(cluster.heartbeat(LEASE + 1, "b"));

    // A lost connection alone takes nothing from a worker that is not leaving.
    assert_eq!(cluster.disconnected(LEASE + 2, "b"), Changes::default());
    // The one that is leaving will not be back: no lease is waited for.
    assert_eq!(cluster.disconnected(LEASE + 2, "a"), changes(&["a"], true));
    assert!(!cluster.heartbeat(LEASE + 2, "a"));
    assert_eq!(cluster.table().routes.len(), 1);

    // Its region is given away like any without an owner, once a worker with a
    // connection is there.
    assert_eq!(cluster.tick(LEASE + 2), Changes::default());
    cluster.register(LEASE + 3, "c", "c:25601", &[]);
    assert_eq!(cluster.tick(LEASE + 3), changes(&["c"], true));
    assert_eq!(
        cluster.assignments("c"),
        [assignment(0, FIRST_EPOCH + 3, 2)]
    );
}

#[test]
fn the_region_of_a_leaving_worker_that_vanishes_goes_at_once_to_another_worker() {
    let mut cluster = Cluster::new(&[0]);
    cluster.register(0, "a", "a:25601", &[]);
    cluster.register(0, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
    cluster.register(LEASE, "c", "c:25601", &[]);
    // Both owners are asked to release for the one worker that does not leave.
    // One of them goes on running, and then its process dies.
    assert_eq!(
        cluster.leaving(LEASE + 1, "a"),
        asks(&[order("a", 0, FIRST_EPOCH + 1)])
    );
    assert_eq!(
        cluster.leaving(LEASE + 1, "b"),
        asks(&[order("b", 1, FIRST_EPOCH + 2)])
    );
    assert_eq!(
        cluster.disconnected(LEASE + 2, "a"),
        changes(&["a", "c"], true)
    );
    // The release went with its owner, and the region to the worker it was
    // meant for all the same, as nobody else is there. The other release goes on.
    assert_eq!(
        cluster.assignments("c"),
        [assignment(0, FIRST_EPOCH + 3, 2)]
    );
    let releases = &cluster.coordinator.releases;
    assert_eq!(Vec::from_iter(releases.keys()), [&RegionId(1)]);
    assert_eq!(releases[&RegionId(1)].to, "c");
}

/// The workers that can be given a region, each with how many regions it has,
/// counting those it is the reserved target of, and its place among the workers:
/// the least of these is the worker that is given the next region. Counted afresh
/// from what the coordinator holds, for the randomised test to check its choices by.
fn fit_workers(coordinator: &Coordinator) -> BTreeMap<String, (usize, u64)> {
    let fit = coordinator
        .workers
        .iter()
        .filter(|(_, worker)| worker.connected && !worker.leaving);
    fit.map(|(name, worker)| {
        let owned = coordinator.assignments(name).len();
        let targets = coordinator.releases.values();
        let reserved = targets.filter(|release| release.to == *name).count();
        (name.clone(), (owned + reserved, worker.arrival))
    })
    .collect()
}

/// Numbers that look random and are the same in every run (xorshift64*).
struct Generator(u64);

impl Generator {
    fn draw(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A number below `bound`.
    fn below(&mut self, bound: u64) -> u64 {
        self.draw() % bound
    }

    /// True once in `times` times.
    fn once_in(&mut self, times: u64) -> bool {
        self.below(times) == 0
    }
}

/// Workers register, send heartbeats or fall silent, hear what they are to run or
/// miss it, and start afresh, and now and then the coordinator is replaced, all at
/// random. Regions are moved, workers release them when asked, unasked or not at
/// all, say that they leave and lose their connections. [`Cluster`] checks every
/// call; this adds what only shows over time, that whoever is given a region or
/// picked as a target is the worker with the fewest regions then, that ticks even
/// regions out, one release at a time, and that a worker that failed a region is
/// passed over for both as long as that is remembered.
#[test]
fn epochs_only_rise_and_nothing_is_shared_whatever_workers_do() {
    const WORKERS: [&str; 6] = ["a", "b", "c", "d", "e", "f"];
    let (mut issued, mut resumed, mut turned_away, mut lost) = (0, 0, 0, 0);
    let (mut restarts, mut dropped) = (0, 0);
    let (mut moves, mut unmoved, mut let_go, mut overdue) = (0, 0, 0, 0);
    let (mut left, mut vanished, mut asked_to_leave, mut cut_off) = (0, 0, 0, 0);
    let (mut compared, mut meant, mut shared, mut handed_on) = (0, 0, 0, 0);
    let (mut evened, mut even, mut faulted, mut passed_over) = (0, 0, 0, 0);
    // For how long a worker that failed a region is at fault.
    let memory = u64::from(Coordinator::FAULT_MEMORY) * LEASE;

    for seed in 1..=24_u64 {
        let mut random = Generator(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        // In every other run the workers only report what a coordinator gave them.
        let honest = seed % 2 == 0;
        let mut cluster = Cluster::new(&[-8, 0, 8]);
        let mut now = 0;
        let mut grace_ends = LEASE;
        // The regions that a worker said it let go of and nobody was given since:
        // the grace period does not hold these back.
        let mut free: BTreeSet<RegionId> = BTreeSet::new();
        // What each worker runs: what it was to run when it last heard of it.
        let mut running: BTreeMap<String, Vec<Assignment>> = BTreeMap::new();
        // What each worker has been asked to release and has not answered.
        let mut asked: BTreeMap<String, Vec<(RegionId, u64)>> = BTreeMap::new();
        // The highest epoch any of the coordinators issued or was told of.
        let mut highest_epoch = FIRST_EPOCH;
        // What the present coordinator has seen: the blocks of entity ids it issued
        // or was told of, the latest owner of each region with its epoch, and who
        // waits to hear how a move ended.
        let mut used_blocks: BTreeSet<u32> = BTreeSet::new();
        let mut tenures: BTreeMap<RegionId, (String, u64)> = BTreeMap::new();
        let mut movers: BTreeSet<u64> = BTreeSet::new();
        // When each worker that the present coordinator knows last failed a
        // region: when one was taken from it that it neither released nor was
        // refused by the store, while it was still registered afterwards.
        let mut faults: BTreeMap<String, u64> = BTreeMap::new();

        for step in 0..4000_u64 {
            now += random.below(LEASE / 8);
            let name = WORKERS[random.below(6) as usize];
            let before = cluster.view();
            let releases_before = cluster.coordinator.releases.clone();
            let leaving_before: BTreeSet<String> = cluster
                .coordinator
                .workers
                .iter()
                .filter(|(_, worker)| worker.leaving)
                .map(|(name, _)| name.clone())
                .collect();
            let fit_before = fit_workers(&cluster.coordinator);
            // The workers that are at fault as of this step, going by what was
            // known before it.
            let at_fault = |faults: &BTreeMap<String, u64>| -> BTreeSet<String> {
                let recent = faults.iter().filter(|(_, failed)| now < **failed + memory);
                recent.map(|(name, _)| name.clone()).collect()
            };
            let faulty_before = at_fault(&faults);
            cluster.last = None;
            // What the worker reports to hold, if this step is a registration.
            let mut reported: Option<Vec<Assignment>> = None;
            // The region its owner lets go of in this step, if one does.
            let mut let_go_of: Option<RegionId> = None;
            // Whether this step is a tick.
            let mut ticked = false;
            // Whether this step is a call that ends like a tick, in which regions
            // are taken from workers that failed them, and the worker and region
            // of which that is not true: what the store refused it.
            let mut settled = false;
            let mut excused: Option<(&str, RegionId)> = None;
            // Whether what is assigned in this step has to respect the grace
            // period of a new coordinator.
            let mut graceful = true;

            let roll = random.below(120);
            if roll < 30 {
                // The worker vouches for what it believes it runs, mostly as being
                // committed, now and then as waiting for the store, or not at all.
                let mut vouches = Vec::new();
                for held in running.get(name).into_iter().flatten() {
                    match random.below(8) {
                        0 => {}
                        1 => vouches.push((held.region, Vouch::WaitingForStore)),
                        _ => vouches.push((held.region, Vouch::Committed)),
                    }
                }
                if !honest && random.once_in(8) {
                    vouches.push((RegionId(random.below(6) as u32), Vouch::Committed));
                }
                cluster.heartbeat_with(now, name, &vouches);
            } else if roll < 60 {
                let mut holding = running.get(name).cloned().unwrap_or_default();
                if !honest && random.once_in(4) {
                    // Something made up: any region, even one that does not exist,
                    // an epoch near the latest, and ids that may well be in use.
                    let live: Vec<&Assignment> = before.assignments.values().flatten().collect();
                    let entity_ids = if live.is_empty() || random.once_in(2) {
                        ids(random.below(40) as u32)
                    } else {
                        live[random.below(live.len() as u64) as usize].entity_ids
                    };
                    holding.push(Assignment {
                        region: RegionId(random.below(6) as u32),
                        epoch: highest_epoch - 3 + random.below(7),
                        entity_ids,
                    });
                }
                let address = format!("{name}:{}", 25601 + random.below(3));
                cluster.register(now, name, &address, &holding);
                let has = cluster.assignments(name);
                // Whatever the worker has that it did not have, it reported.
                for gained in has.iter().filter(|a| !before.assignments(name).contains(a)) {
                    assert!(holding.contains(gained), "{gained:?} came from nowhere");
                    resumed += 1;
                }
                turned_away += holding.iter().filter(|held| !has.contains(held)).count();
                for reported in &holding {
                    highest_epoch = highest_epoch.max(reported.epoch);
                    used_blocks.insert(reported.entity_ids.index());
                }
                // Most of the time the answer reaches the worker.
                if !random.once_in(4) {
                    running.insert(name.to_owned(), has);
                }
                // A region it was asked to release and came back without went to
                // another worker at once, whatever the grace period says.
                graceful = false;
                reported = Some(holding);
            } else if roll < 88 {
                let changes = cluster.tick(now);
                ticked = true;
                settled = true;
                for worker in WORKERS {
                    let had = before.assignments(worker);
                    let has = cluster.assignments(worker);
                    lost += had.iter().filter(|held| !has.contains(held)).count();
                }
                // Not every worker gets to hear that something has changed for it.
                for worker in changes.workers {
                    if random.once_in(2) {
                        let has = cluster.assignments(&worker);
                        running.insert(worker, has);
                    }
                }
            } else if roll < 91 {
                // The store refuses to let the worker open one of its regions: it
                // has seen a higher epoch, which some coordinator issued.
                let held = running.get(name).and_then(|held| held.first()).copied();
                let (region, epoch) = match held {
                    Some(held) => (held.region, held.epoch),
                    None => (RegionId(random.below(6) as u32), highest_epoch - 3),
                };
                let seen = if honest {
                    if epoch >= highest_epoch {
                        continue;
                    }
                    epoch + 1 + random.below(highest_epoch - epoch)
                } else {
                    highest_epoch - 3 + random.below(7)
                };
                let owned = before
                    .assignments(name)
                    .iter()
                    .any(|held| held.region == region && held.epoch < seen);
                cluster.epoch_refused(now, name, region.0, seen);
                highest_epoch = highest_epoch.max(seen);
                let has = cluster.assignments(name);
                assert!(
                    !has.iter()
                        .any(|held| held.region == region && held.epoch < seen)
                );
                settled = true;
                if owned {
                    dropped += 1;
                    excused = Some((name, region));
                }
                // The worker has dropped the region.
                if let Some(held) = running.get_mut(name) {
                    held.retain(|held| held.region != region);
                }
            } else if roll < 94 {
                // The worker catches up with what it is to run.
                running.insert(name.to_owned(), cluster.assignments(name));
            } else if roll < 97 {
                // The worker's process is replaced by a new one, which runs nothing
                // and has been asked nothing.
                running.remove(name);
                asked.remove(name);
            } else if roll < 100 {
                if random.once_in(3) {
                    // The coordinator is replaced by a new one, whose clock tells
                    // it no more than that it is not behind the old one.
                    cluster.restart(now, highest_epoch);
                    grace_ends = now + LEASE;
                    free.clear();
                    used_blocks.clear();
                    tenures.clear();
                    movers.clear();
                    faults.clear();
                    restarts += 1;
                    continue;
                }
            } else if roll < 106 {
                // Somebody asks for a region to be moved, now and then for one that
                // does not exist, and sometimes to a certain worker.
                let region = random.below(5) as u32;
                let to = random.once_in(3).then_some(name);
                match cluster.move_region(now, region, to, step) {
                    Ok((begun, _)) => {
                        let had = before.assignments(&begun.from).iter();
                        assert!(had.map(|held| held.region.0).any(|held| held == region));
                        // The target can be given a region, however many it runs.
                        // If nobody named it, no other has fewer, nor has one
                        // with as many waited longer.
                        assert!(fit_before.contains_key(&begun.to), "{begun:?}");
                        assert_ne!(begun.from, begun.to);
                        // One that failed a region of late comes after all
                        // those that did not.
                        let others = fit_before.iter();
                        let chosen = others
                            .filter(|(other, _)| **other != begun.from)
                            .min_by_key(|(other, load)| (faulty_before.contains(*other), **load))
                            .map(|(other, _)| other.as_str());
                        match to {
                            Some(to) => assert_eq!(to, begun.to),
                            None => assert_eq!(chosen, Some(begun.to.as_str())),
                        }
                        movers.insert(step);
                        moves += 1;
                    }
                    Err(_) => unmoved += 1,
                }
            } else if roll < 112 {
                // The worker lets go of a region: one it was asked to release,
                // whether or not it runs it, or one it runs without being asked,
                // or, if it makes things up, anything.
                let pending = asked.get_mut(name).and_then(Vec::pop);
                let own = running.get(name).and_then(|held| held.first());
                let own = own.map(|held| (held.region, held.epoch));
                let made_up = (
                    RegionId(random.below(5) as u32),
                    highest_epoch - 3 + random.below(7),
                );
                let said = match (pending, own) {
                    _ if !honest && random.once_in(4) => Some(made_up),
                    (Some(pending), _) => Some(pending),
                    (None, Some(own)) if random.once_in(2) => Some(own),
                    _ => None,
                };
                let Some((region, epoch)) = said else {
                    continue;
                };
                let owned = |held: &Assignment| (held.region, held.epoch) == (region, epoch);
                let owned = before.assignments(name).iter().any(owned);
                let changes = cluster.released(now, name, region.0, epoch);
                if owned {
                    // It no longer has the region, whoever has it now: the
                    // worker it was to be released for, if that can be given it.
                    let has = cluster.assignments(name);
                    assert!(has.iter().all(|held| held.region != region), "{has:?}");
                    let target = releases_before.get(&region).map(|release| &release.to);
                    if let Some(target) = target.filter(|to| fit_before.contains_key(*to)) {
                        let has = cluster.assignments(target);
                        assert!(has.iter().any(|held| held.region == region), "{has:?}");
                    }
                    let_go += 1;
                    let_go_of = Some(region);
                    free.insert(region);
                } else if before.table.route(region).is_none() {
                    // Nobody owns it: the worker may have been its last owner,
                    // before this coordinator knew of it.
                    free.insert(region);
                } else {
                    assert_eq!(changes, Changes::default());
                }
                if let Some(held) = running.get_mut(name) {
                    held.retain(|held| (held.region, held.epoch) != (region, epoch));
                }
                graceful = false;
            } else if roll < 116 {
                let known = cluster.coordinator.workers.contains_key(name);
                cluster.leaving(now, name);
                asked_to_leave += usize::from(known);
            } else {
                // A connection ends: that of a worker that is leaving, if there is
                // one, half of the time, as such a worker soon exits.
                let leaver = leaving_before.first().filter(|_| random.once_in(2));
                let name = leaver.map_or(name, String::as_str);
                let owned = !before.assignments(name).is_empty();
                let known = cluster.coordinator.workers.contains_key(name);
                cluster.disconnected(now, name);
                settled = true;
                if leaving_before.contains(name) {
                    // It is gone, with what it owned.
                    assert!(!cluster.coordinator.workers.contains_key(name));
                    vanished += usize::from(owned);
                } else {
                    assert_eq!(cluster.assignments(name), before.assignments(name));
                    cut_off += usize::from(known);
                }
            }

            let said = cluster.last.take().unwrap_or_default();
            let after = cluster.view();

            // Who failed a region in this step. The coordinator remembers that
            // of a worker for as long as it knows the worker, and nothing else.
            if settled {
                for worker in WORKERS {
                    let has = after.assignments(worker);
                    let gone = |held: &&Assignment| {
                        let same =
                            |has: &Assignment| (has.region, has.epoch) == (held.region, held.epoch);
                        excused != Some((worker, held.region)) && !has.iter().any(same)
                    };
                    if before.assignments(worker).iter().any(|held| gone(&held)) {
                        faults.insert(worker.to_owned(), now);
                        faulted += 1;
                    }
                }
            }
            faults.retain(|name, _| cluster.coordinator.workers.contains_key(name));
            for (name, worker) in &cluster.coordinator.workers {
                let failed = faults.get(name).map(|failed| cluster.at(*failed));
                assert_eq!(worker.failed, failed, "{name}");
            }
            // A worker that came to be at fault in this step was so for some of
            // what the step did and not for the rest.
            let faulty_after = at_fault(&faults);
            let faulty = |name: &str| {
                let before = faulty_before.contains(name);
                (before == faulty_after.contains(name)).then_some(before)
            };

            // What workers were given in this step, as opposed to what they said
            // they held, in the order it was issued: it is above every epoch
            // before, and has entity ids nobody was given.
            let mut gained: Vec<(&str, Assignment)> = Vec::new();
            for worker in WORKERS {
                let had = before.assignments(worker);
                let held = reported.as_deref().filter(|_| worker == name);
                for has in after.assignments(worker) {
                    if !had.contains(has) && !held.is_some_and(|held| held.contains(has)) {
                        gained.push((worker, *has));
                    }
                }
            }
            gained.sort_by_key(|(_, assignment)| assignment.epoch);
            for (worker, assignment) in &gained {
                assert!(
                    !graceful || now >= grace_ends || free.contains(&assignment.region),
                    "assigned during the grace period"
                );
                free.remove(&assignment.region);
                assert!(assignment.epoch > highest_epoch, "{assignment:?}");
                highest_epoch = assignment.epoch;
                let block = assignment.entity_ids.index();
                assert_eq!(assignment.entity_ids, ids(block));
                assert!(used_blocks.insert(block), "{assignment:?}");
                issued += 1;
                // A leaving worker is never given a region.
                assert!(
                    !leaving_before.contains(*worker) || reported.is_some() && *worker == name,
                    "{worker} is leaving and was given {assignment:?}"
                );
            }
            // Whoever was given a region in this step had the fewest regions of the
            // workers that could be given it at that moment, and was at fault only
            // if all of them were: a worker that is not at fault comes before one
            // that is, whatever the two run. What the others had
            // then is known for sure only if they lost nothing in this step and no
            // release that was meant for them ended in it, so each of them is
            // counted between the least and the most it can have had. A region
            // that goes to the target of its release was counted as that worker's
            // before; one that was let go of or released does not go back to the
            // worker it was taken from.
            let coordinator = &cluster.coordinator;
            let fit = fit_workers(coordinator);
            let lasted = |region: &RegionId, release: &Release| {
                coordinator.releases.get(region).is_some_and(|now| {
                    (now.epoch, &now.from, &now.to) == (release.epoch, &release.from, &release.to)
                })
            };
            for (index, (worker, assignment)) in gained.iter().enumerate() {
                let region = assignment.region;
                assert!(
                    fit.contains_key(*worker),
                    "{worker} was given {assignment:?}"
                );
                let release = releases_before.get(&region);
                if release.is_some_and(|release| release.to == *worker) {
                    meant += 1;
                    continue;
                }
                let handed_over = release.is_some() || let_go_of == Some(region);
                let owned = |held: &&Assignment| held.region == region;
                let old = WORKERS
                    .into_iter()
                    .find(|old| before.assignments(old).iter().any(|held| owned(&held)));
                let least = |other: &str| {
                    let later = gained[index..].iter();
                    let reserved = releases_before.iter();
                    after.assignments(other).len()
                        - later.filter(|(given, _)| *given == other).count()
                        + reserved
                            .filter(|(region, release)| lasted(region, release))
                            .filter(|(_, release)| release.to == other)
                            .count()
                };
                let most = |other: &str| {
                    let had = before.assignments(other).iter();
                    let has = after.assignments(other);
                    let ended = releases_before.iter();
                    least(other)
                        + had.filter(|held| !has.contains(held)).count()
                        + ended
                            .filter(|(ended, release)| **ended != region && !lasted(ended, release))
                            .filter(|(_, release)| release.to == other)
                            .count()
                };
                let had = least(worker);
                shared += usize::from(had > 0);
                for (other, (_, arrival)) in &fit {
                    if other == worker || handed_over && old == Some(other.as_str()) {
                        continue;
                    }
                    match (faulty(worker), faulty(other)) {
                        (Some(mine), Some(theirs)) if mine == theirs => {}
                        (Some(false), Some(true)) => {
                            passed_over += usize::from(least(other) < had);
                            continue;
                        }
                        (Some(true), Some(false)) => panic!(
                            "{worker} is at fault and was given {assignment:?}, \
                                 although {other} is not"
                        ),
                        _ => continue,
                    }
                    assert!(
                        had <= most(other),
                        "{worker} had {had} regions and was given {assignment:?}, \
                             although {other} had {} at most",
                        most(other)
                    );
                    if least(other) == most(other) && had == most(worker) {
                        let waited = fit[*worker].1;
                        assert!(
                            had < least(other) || waited < *arrival,
                            "{worker} was given {assignment:?} before {other}, \
                                 which had as few regions and had waited longer"
                        );
                        compared += 1;
                    }
                }
            }
            // The releases that nobody asked for and that were begun in this step:
            // for workers that leave, or to even regions out.
            let leaves = |name: &str| {
                let worker = coordinator.workers.get(name);
                worker.is_some_and(|worker| worker.leaving)
            };
            let (begun, evening): (Vec<_>, Vec<_>) = coordinator
                .releases
                .iter()
                .filter(|(_, release)| release.mover.is_none())
                .filter(|(region, release)| {
                    let before = releases_before.get(region);
                    before.is_none_or(|before| before.epoch != release.epoch)
                })
                .partition(|(_, release)| leaves(&release.from));

            // A release to even out is begun by a tick alone, when the coordinator
            // is no longer new and no other release is under way. It takes the
            // highest region of a worker with the most regions among those that
            // can be given one and are not at fault, the one of them that has
            // waited least, for the one of them with the fewest, which has fewer
            // by two regions or more. All faults of the step are known by then.
            let owned = |name: &str| after.assignments(name).len();
            let sound: Vec<(&String, u64)> = fit
                .iter()
                .filter(|(name, _)| !faulty_after.contains(*name))
                .map(|(name, (_, arrival))| (name, *arrival))
                .collect();
            for (region, release) in &evening {
                assert!(ticked && now >= grace_ends, "{release:?}");
                assert_eq!(coordinator.releases.len(), 1, "{release:?}");
                let heavy = sound
                    .iter()
                    .max_by_key(|(name, arrival)| (owned(name), *arrival))
                    .map(|(name, _)| *name);
                let light = sound
                    .iter()
                    .min_by_key(|(name, arrival)| (owned(name), *arrival))
                    .map(|(name, _)| *name);
                assert_eq!(heavy, Some(&release.from), "{release:?}");
                assert_eq!(light, Some(&release.to), "{release:?}");
                assert!(owned(&release.from) >= owned(&release.to) + 2);
                let highest = after.assignments(&release.from).last();
                assert_eq!(highest.map(|held| held.region), Some(**region));
                evened += 1;
            }
            // And no tick leaves two of those workers further apart than by one
            // region without having begun to do something about it.
            if ticked && now >= grace_ends && coordinator.releases.is_empty() {
                let most = sound.iter().map(|(name, _)| owned(name)).max();
                let fewest = sound.iter().map(|(name, _)| owned(name)).min();
                assert!(most <= fewest.map(|fewest| fewest + 1), "{sound:?}");
                even += 1;
            }

            // The releases that were begun for workers that leave are begun in the
            // order of their regions, each for the worker with the fewest regions,
            // and each counting for its target from then on.
            for (index, (_, release)) in begun.iter().enumerate() {
                let later = |other: &str| {
                    let later = begun[index..].iter();
                    later.filter(|(_, later)| later.to == other).count()
                };
                let chosen = fit
                    .iter()
                    .filter(|(other, _)| **other != release.from)
                    .min_by_key(|(other, (load, arrival))| {
                        let at_fault = faulty_after.contains(*other);
                        (at_fault, load - later(other), *arrival)
                    })
                    .map(|(other, _)| other);
                assert_eq!(chosen, Some(&release.to), "{release:?}");
                handed_on += 1;
            }

            // Whoever asked for a move hears once how it ended, and that is when
            // the release ends.
            for outcome in &said.moves {
                assert!(movers.remove(&outcome.mover), "{outcome:?}");
                overdue += usize::from(!outcome.released);
            }
            let waiting: BTreeSet<u64> = cluster
                .coordinator
                .releases
                .values()
                .filter_map(|release| release.mover)
                .collect();
            assert_eq!(movers, waiting);
            left += said.gone.len();
            // Not every worker gets to hear that it is to release something.
            for order in said.releases {
                if !random.once_in(3) {
                    let orders = asked.entry(order.worker).or_default();
                    orders.push((order.region, order.epoch));
                }
            }

            // A region never goes back to a lower epoch, and unless workers make
            // things up, another owner means a higher one.
            for (worker, assignments) in &after.assignments {
                for assignment in assignments {
                    if let Some((owner, epoch)) = tenures.get(&assignment.region) {
                        assert!(assignment.epoch >= *epoch, "{assignment:?}");
                        if honest && owner != worker {
                            assert!(assignment.epoch > *epoch, "{assignment:?}");
                        }
                    }
                    let tenure = (worker.clone(), assignment.epoch);
                    tenures.insert(assignment.region, tenure);
                }
            }
        }
    }

    // All of it has in fact happened, and often.
    let counts = [
        issued,
        resumed,
        turned_away,
        lost,
        restarts,
        dropped,
        moves,
        unmoved,
        let_go,
        overdue,
        left,
        vanished,
        asked_to_leave,
        cut_off,
        compared,
        meant,
        shared,
        handed_on,
        evened,
        even,
        faulted,
        passed_over,
    ];
    assert!(counts.iter().all(|count| *count >= 100), "{counts:?}");
}

/// Short for [`FIRST_EPOCH`], where every epoch of a test is spelt out.
const E: u64 = FIRST_EPOCH;

fn prepare(worker: &str, region: u32, epoch: u64) -> ReshapeOrder {
    ReshapeOrder {
        worker: worker.to_owned(),
        order: Order::Prepare {
            region: RegionId(region),
            epoch,
        },
    }
}

fn absorb(worker: &str, region: u32, epoch: u64, absorbed: u32, as_epoch: u64) -> ReshapeOrder {
    ReshapeOrder {
        worker: worker.to_owned(),
        order: Order::Absorb {
            region: RegionId(region),
            epoch,
            absorbed: RegionId(absorbed),
            as_epoch,
        },
    }
}

/// The chunks every split of these tests names.
const CHUNKS: [ChunkPos; 2] = [ChunkPos::new(7, 1), ChunkPos::new(8, 1)];

fn split_off(worker: &str, region: u32, epoch: u64, as_epoch: u64, part: u32) -> ReshapeOrder {
    ReshapeOrder {
        worker: worker.to_owned(),
        order: Order::SplitOff {
            region: RegionId(region),
            epoch,
            chunks: CHUNKS.to_vec(),
            as_epoch,
            part: RegionId(part),
        },
    }
}

/// How the merge that `asker` asked for ended.
fn merged(
    asker: Option<u64>,
    survivor: u32,
    absorbed: u32,
    outcome: Result<u32, Undone>,
) -> Reshaped {
    Reshaped {
        asker,
        asked: Asked::Merge {
            survivor: RegionId(survivor),
            absorbed: RegionId(absorbed),
        },
        outcome: outcome.map(RegionId),
    }
}

/// How the split that `asker` asked for ended.
fn was_split(asker: Option<u64>, region: u32, outcome: Result<u32, Undone>) -> Reshaped {
    Reshaped {
        asker,
        asked: Asked::Split {
            region: RegionId(region),
        },
        outcome: outcome.map(RegionId),
    }
}

/// What a call says that only asks for the list to be read.
fn reads() -> Changes {
    Changes {
        read: true,
        ..Changes::default()
    }
}

/// A world of three stripes with a worker each, `a`, `b` and `c` in the order of
/// the regions, at the time `LEASE`, and the list as the world store has it then,
/// which the coordinator has been handed. The regions have the epochs `E + 1` to
/// `E + 3`.
fn three_stripes() -> (Cluster, RegionList) {
    let mut cluster = Cluster::new(&[0, 4]);
    for name in ["a", "b", "c"] {
        cluster.register(0, name, &format!("{name}:25601"), &[]);
    }
    assert_eq!(cluster.tick(LEASE), changes(&["a", "b", "c"], true));
    let list = RegionList {
        regions: vec![living(0, E + 1), living(1, E + 2), living(2, E + 3)],
        ..stripes(3)
    };
    // The home region is new to the table.
    assert_eq!(cluster.listed(LEASE, &list), changes(&[], true));
    beat(&mut cluster, LEASE);
    (cluster, list)
}

/// Every worker of [`three_stripes`] is heard from and vouches for what it runs.
fn beat(cluster: &mut Cluster, at: u64) {
    for name in ["a", "b", "c"] {
        assert!(cluster.heartbeat(at, name));
    }
}

/// The list of [`three_stripes`] after region 0 has absorbed region 1.
fn after_the_merge(list: &RegionList) -> RegionList {
    RegionList {
        regions: vec![living(0, E + 1), living(2, E + 3)],
        absorbed: vec![(RegionId(1), RegionId(0))],
        ..list.clone()
    }
}

/// [`three_stripes`] with region 0 asked at `LEASE + 1` to absorb region 1, whose
/// owner let go of it at `LEASE + 2`: the survivor's owner has been told to open
/// it with the epoch `E + 4`. Somebody numbered 7 asked.
fn absorbing() -> (Cluster, RegionList) {
    let (mut cluster, list) = three_stripes();
    cluster.merge(LEASE + 1, 0, 1, Some(7)).unwrap();
    cluster.released(LEASE + 2, "b", 1, E + 2);
    (cluster, list)
}

#[test]
fn a_merge_has_the_one_region_released_and_then_absorbed_and_the_list_says_when_it_is_done() {
    let (mut cluster, list) = three_stripes();
    let version = cluster.table().version;

    // The owner of the one is asked to release it, that of the other to prepare.
    let asked = cluster.merge(LEASE + 1, 0, 1, Some(7)).unwrap();
    let expected = Changes {
        releases: vec![order("b", 1, E + 2)],
        orders: vec![prepare("a", 0, E + 1)],
        ..Changes::default()
    };
    assert_eq!(asked, expected);

    // When it has let go, the region is nobody's and is given to nobody: the
    // survivor's owner is to open it, with an epoch above every other.
    let released = cluster.released(LEASE + 2, "b", 1, E + 2);
    let expected = Changes {
        orders: vec![absorb("a", 0, E + 1, 1, E + 4)],
        ..changes(&["b"], true)
    };
    assert_eq!(released, expected);
    assert!(cluster.assignments("b").is_empty());
    let table = cluster.table();
    assert_eq!(table.version, version + 1);
    assert_eq!(
        table.routes,
        [route(0, E + 1, "a:25601"), route(2, E + 3, "c:25601")]
    );
    assert_eq!(table.waiting, 1);
    assert!(!table.is_complete());
    assert_eq!(cluster.coordinator.waiting(), [RegionId(1)]);
    // Ticks leave it so, although a worker waits that runs nothing.
    for quarter in 1..=3 {
        assert_eq!(
            cluster.tick(LEASE + quarter * LEASE / 4),
            Changes::default()
        );
    }

    // The worker's word that it is done decides nothing: the list is asked for.
    let said = cluster.absorb_ended(LEASE + 3 * LEASE / 4, "a", 0, 1, Ok(()));
    assert_eq!(said, reads());
    assert_eq!(cluster.table(), table);

    // The list has the region as absorbed: it is gone, and whoever asked is told.
    let merged_list = after_the_merge(&list);
    let done = cluster.listed(LEASE + 3 * LEASE / 4 + 1, &merged_list);
    let expected = Changes {
        routing: true,
        reshaped: vec![merged(Some(7), 0, 1, Ok(0))],
        ..Changes::default()
    };
    assert_eq!(done, expected);
    let table = cluster.table();
    assert_eq!(table.version, version + 2);
    assert_eq!(table.absorbed, [(RegionId(1), RegionId(0))]);
    assert_eq!(table.home, Some(RegionId(0)));
    assert_eq!(table.routes.len(), 2);
    assert!(table.is_complete());
    assert!(cluster.coordinator.merges.is_empty());

    // Nothing is left to do, and the region can be merged or moved again.
    beat(&mut cluster, 2 * LEASE);
    assert_eq!(cluster.tick(2 * LEASE), Changes::default());
    assert_eq!(
        cluster.merge(2 * LEASE, 0, 1, None),
        Err(ReshapeRefusal::NoSuchRegion(RegionId(1)))
    );
    assert!(cluster.move_region(2 * LEASE, 0, None, 9).is_ok());
}

#[test]
fn the_merges_and_splits_under_way_are_told_from_when_they_begin_until_they_end() {
    let (mut cluster, list) = three_stripes();
    assert!(cluster.coordinator.under_way().is_empty());
    let merge = Asked::Merge {
        survivor: RegionId(0),
        absorbed: RegionId(1),
    };
    let split = Asked::Split {
        region: RegionId(2),
    };

    // Whoever asked, and at every stage of a merge. A refusal begins nothing.
    cluster.split(LEASE + 1, 2, &CHUNKS, Some(8)).unwrap();
    assert_eq!(cluster.coordinator.under_way(), [split]);
    cluster.merge(LEASE + 1, 0, 1, None).unwrap();
    assert!(cluster.merge(LEASE + 1, 0, 2, None).is_err());
    assert_eq!(cluster.coordinator.under_way(), [merge, split]);
    cluster.released(LEASE + 2, "b", 1, E + 2);
    cluster.absorb_ended(LEASE + 3, "a", 0, 1, Ok(()));
    assert_eq!(cluster.coordinator.under_way(), [merge, split]);

    // The merge ends with the list, and the split with its worker's word.
    cluster.listed(LEASE + 4, &after_the_merge(&list));
    assert_eq!(cluster.coordinator.under_way(), [split]);
    cluster.split_ended(LEASE + 5, "c", 2, E + 4, Err(Off::Nobody));
    assert!(cluster.coordinator.under_way().is_empty());
}

#[test]
fn a_merge_is_refused_for_the_first_reason_that_holds_and_changes_nothing() {
    let (mut cluster, _) = three_stripes();
    let refused = |cluster: &mut Cluster, survivor, absorbed| {
        cluster
            .merge(LEASE + 1, survivor, absorbed, None)
            .unwrap_err()
    };
    let (zero, one, two) = (RegionId(0), RegionId(1), RegionId(2));

    // A region nobody knows comes before everything, the survivor first.
    assert_eq!(
        refused(&mut cluster, 8, 9),
        ReshapeRefusal::NoSuchRegion(RegionId(8))
    );
    assert_eq!(
        refused(&mut cluster, 1, 9),
        ReshapeRefusal::NoSuchRegion(RegionId(9))
    );
    assert_eq!(
        refused(&mut cluster, 9, 9),
        ReshapeRefusal::NoSuchRegion(RegionId(9))
    );
    assert_eq!(refused(&mut cluster, 1, 1), ReshapeRefusal::Same);
    // The home region absorbs, and is never absorbed.
    assert_eq!(refused(&mut cluster, 1, 0), ReshapeRefusal::Home);
    assert_eq!(refused(&mut cluster, 0, 0), ReshapeRefusal::Same);

    // A region that is being moved.
    cluster.move_region(LEASE + 1, 2, Some("a"), 5).unwrap();
    assert_eq!(
        refused(&mut cluster, 1, 2),
        ReshapeRefusal::BeingReleased(two)
    );
    assert_eq!(
        refused(&mut cluster, 2, 1),
        ReshapeRefusal::BeingReleased(two)
    );
    cluster.released(LEASE + 1, "c", 2, E + 3);
    assert_eq!(regions_of(&cluster, "a"), [0, 2]);

    // A worker that could not be told, or that will not be there. The owner of
    // the region to absorb only has to hear that it is to let go.
    cluster.disconnected(LEASE + 1, "b");
    let unfit = |worker: &str, why| ReshapeRefusal::Unfit {
        worker: worker.to_owned(),
        why,
    };
    let cut_off = "has no connection to the coordinator";
    assert_eq!(refused(&mut cluster, 1, 2), unfit("b", cut_off));
    assert_eq!(refused(&mut cluster, 2, 1), unfit("b", cut_off));
    cluster.register(LEASE + 1, "b", "b:25601", &[assignment(1, E + 2, 1)]);
    cluster.leaving(LEASE + 1, "b");
    // It is being released for the leaver at once, as there is a target.
    assert_eq!(
        refused(&mut cluster, 1, 2),
        ReshapeRefusal::BeingReleased(one)
    );
    cluster.released(LEASE + 1, "b", 1, E + 2);

    // A region that is part of a merge under way, as either part of another.
    cluster.merge(LEASE + 1, 0, 1, None).unwrap();
    assert_eq!(refused(&mut cluster, 0, 2), ReshapeRefusal::Reserved(zero));
    assert_eq!(refused(&mut cluster, 2, 1), ReshapeRefusal::Reserved(one));
    assert_eq!(refused(&mut cluster, 1, 2), ReshapeRefusal::Reserved(one));
    // Being reserved comes after being the same and being the home region.
    assert_eq!(refused(&mut cluster, 1, 1), ReshapeRefusal::Same);
    assert_eq!(refused(&mut cluster, 1, 0), ReshapeRefusal::Home);
    assert_eq!(refused(&mut cluster, 2, 0), ReshapeRefusal::Home);
    assert_eq!(
        cluster.split(LEASE + 1, 0, &CHUNKS, None),
        Err(ReshapeRefusal::Reserved(zero))
    );
    assert_eq!(
        cluster.move_region(LEASE + 1, 1, None, 5),
        Err(MoveRefusal::Reserved(one))
    );
}

#[test]
fn a_leaving_survivor_is_unfit_and_a_region_without_an_owner_is_not_merged() {
    let (mut cluster, _) = three_stripes();
    // The owner of region 2 says that it leaves, with nobody to hand over to: the
    // other two have no connection.
    cluster.disconnected(LEASE + 1, "a");
    cluster.disconnected(LEASE + 1, "b");
    assert_eq!(cluster.leaving(LEASE + 1, "c"), Changes::default());
    let unfit = |worker: &str, why| ReshapeRefusal::Unfit {
        worker: worker.to_owned(),
        why,
    };
    let cut_off = "has no connection to the coordinator";
    assert_eq!(
        cluster.merge(LEASE + 1, 2, 1, None),
        Err(unfit("c", "is leaving"))
    );
    assert_eq!(
        cluster.split(LEASE + 1, 2, &CHUNKS, None),
        Err(unfit("c", "is leaving"))
    );
    assert_eq!(
        cluster.merge(LEASE + 1, 0, 2, None),
        Err(unfit("a", cut_off))
    );
    assert_eq!(
        cluster.split(LEASE + 1, 0, &CHUNKS, None),
        Err(unfit("a", cut_off))
    );
    // What a split is to take is looked at after who is to do it.
    assert_eq!(
        cluster.split(LEASE + 1, 0, &[], None),
        Err(unfit("a", cut_off))
    );
    assert_eq!(
        cluster.split(LEASE + 1, 9, &[], None),
        Err(ReshapeRefusal::NoSuchRegion(RegionId(9)))
    );

    // The lease of one of them runs out, and nobody can be given its region.
    for name in ["a", "c"] {
        assert!(cluster.heartbeat(2 * LEASE, name));
    }
    let later = 2 * LEASE + 1;
    assert_eq!(cluster.tick(later), changes(&["b"], true));
    let no_owner = Err(ReshapeRefusal::NoOwner(RegionId(1)));
    // That comes before whether the other owner can be told.
    assert_eq!(cluster.merge(later, 0, 1, None), no_owner);
    assert_eq!(cluster.merge(later, 1, 2, None), no_owner);
    assert_eq!(cluster.split(later, 1, &CHUNKS, None), no_owner);
}

#[test]
fn a_split_wants_chunks_and_a_reading_of_the_list() {
    let mut cluster = Cluster::new(&[0]);
    cluster.register(0, "a", "a:25601", &[]);
    cluster.tick(LEASE);
    assert_eq!(
        cluster.split(LEASE, 0, &[], None),
        Err(ReshapeRefusal::NoChunks)
    );
    // The new region is named by the list, which nobody has read.
    assert_eq!(
        cluster.split(LEASE, 0, &CHUNKS, None),
        Err(ReshapeRefusal::Unlisted)
    );
    // A reading that failed does not help.
    assert_eq!(cluster.unlisted(LEASE), Changes::default());
    assert_eq!(
        cluster.split(LEASE, 0, &CHUNKS, None),
        Err(ReshapeRefusal::Unlisted)
    );
    cluster.listed(LEASE, &stripes(2));
    assert!(cluster.split(LEASE, 0, &CHUNKS, None).is_ok());

    // A merge is asked of the regions the coordinator knows, with or without it.
    let mut cluster = Cluster::new(&[0]);
    cluster.register(0, "a", "a:25601", &[]);
    cluster.tick(LEASE);
    assert!(cluster.merge(LEASE, 1, 0, None).is_ok());
}

#[test]
fn what_a_refusal_and_an_outcome_say_in_words() {
    let unfit = ReshapeRefusal::Unfit {
        worker: "a".to_owned(),
        why: "is leaving",
    };
    assert_eq!(
        unfit.to_string(),
        "the worker a, which would have to do it, is leaving"
    );
    assert_eq!(
        ReshapeRefusal::Reserved(RegionId(3)).to_string(),
        "region 3 is part of a merge or a split that is under way"
    );
    assert_eq!(
        Undone::Off(Off::Nobody).to_string(),
        "no player stands in a chunk named that the region holds"
    );
    let declined = Off::Declined(Decline::NotOpened { epoch: Some(12) });
    assert_eq!(
        Undone::Off(declined).to_string(),
        "the world store declined: the region to absorb has an owner with epoch 12"
    );
    assert_eq!(
        Undone::Gone(RegionId(4)).to_string(),
        "the world store's list of regions no longer has region 4"
    );
}

#[test]
fn the_regions_of_a_merge_are_not_taken_for_want_of_vouching_and_count_as_vouched_at_its_end() {
    let (mut cluster, _) = three_stripes();
    // Nobody vouches for regions 0 and 1 after the time `LEASE`; region 2 is
    // vouched for throughout.
    let silent = |cluster: &mut Cluster, at| {
        for name in ["a", "b"] {
            assert!(cluster.heartbeat_with(at, name, &[]));
        }
        assert!(cluster.heartbeat(at, "c"));
    };
    let asked = 2 * LEASE - 10;
    silent(&mut cluster, asked);
    cluster.merge(asked, 0, 1, None).unwrap();

    // Both have gone a lease without being vouched for, and stay where they are.
    silent(&mut cluster, 2 * LEASE + 5);
    assert_eq!(cluster.tick(2 * LEASE + 5), Changes::default());

    // The merge has had its lease: the region that was not released is taken
    // from its owner, which has failed it, and given to the first of the others.
    let late = asked + LEASE + 1;
    silent(&mut cluster, late);
    let ended = cluster.tick(late);
    let expected = Changes {
        reshaped: vec![merged(None, 0, 1, Err(Undone::NotReleased))],
        ..changes(&["a", "b"], true)
    };
    assert_eq!(ended, expected);
    assert_eq!(regions_of(&cluster, "a"), [0, 1]);
    assert_eq!(
        cluster.coordinator.workers["b"].failed,
        Some(cluster.at(late))
    );
    // The survivor counts as vouched for at that moment, and so has another
    // lease, like the region its owner was given just now.
    silent(&mut cluster, late + LEASE);
    assert_eq!(cluster.tick(late + LEASE), Changes::default());
    silent(&mut cluster, late + LEASE + 1);
    assert_eq!(cluster.tick(late + LEASE + 1), changes(&["a", "c"], true));
    assert_eq!(regions_of(&cluster, "c"), [0, 1, 2]);
}

#[test]
fn a_merge_that_the_worker_calls_off_ends_with_the_region_assigned_at_once() {
    let (mut cluster, list) = absorbing();
    let why = Off::Declined(Decline::Uncheckpointed {
        region: RegionId(1),
    });
    assert_eq!(
        cluster.absorb_ended(LEASE + 3, "a", 0, 1, Err(why)),
        reads()
    );
    // Nothing is given away on the worker's word.
    assert!(cluster.assignments("b").is_empty());
    assert_eq!(cluster.tick(LEASE + 4), Changes::default());

    // The list still has the region, opened with the epoch for the merge. It
    // goes to the worker with the fewest, with an epoch above that one.
    let list = RegionList {
        regions: vec![living(0, E + 1), living(1, E + 4), living(2, E + 3)],
        ..list
    };
    let off = cluster.listed(LEASE + 5, &list);
    let expected = Changes {
        reshaped: vec![merged(Some(7), 0, 1, Err(Undone::Off(why)))],
        ..changes(&["b"], true)
    };
    assert_eq!(off, expected);
    assert_eq!(cluster.assignments("b"), [assignment(1, E + 5, 3)]);
    assert!(cluster.table().is_complete());
    assert!(cluster.table().absorbed.is_empty());
}

#[test]
fn a_worker_that_says_done_against_the_list_is_not_believed() {
    let (mut cluster, list) = absorbing();
    cluster.absorb_ended(LEASE + 3, "a", 0, 1, Ok(()));
    let off = cluster.listed(LEASE + 4, &list);
    let expected = Changes {
        reshaped: vec![merged(Some(7), 0, 1, Err(Undone::Contradicted))],
        ..changes(&["b"], true)
    };
    assert_eq!(off, expected);
    assert_eq!(regions_of(&cluster, "b"), [1]);
}

#[test]
fn a_merge_waits_for_a_list_that_cannot_be_read_and_asks_again_at_every_tick() {
    let (mut cluster, list) = absorbing();
    assert_eq!(cluster.absorb_ended(LEASE + 3, "a", 0, 1, Ok(())), reads());
    // The reading is under way: a tick does not ask for another.
    assert_eq!(cluster.tick(LEASE + 4), Changes::default());
    // It failed. Nothing is decided, and the next tick asks again; once.
    assert_eq!(cluster.unlisted(LEASE + 5), Changes::default());
    assert_eq!(cluster.tick(LEASE + 6), reads());
    assert_eq!(cluster.tick(LEASE + 7), Changes::default());
    assert_eq!(cluster.unlisted(LEASE + 8), Changes::default());
    assert_eq!(cluster.tick(LEASE + 9), reads());
    assert!(cluster.assignments("b").is_empty());

    let done = cluster.listed(LEASE + 10, &after_the_merge(&list));
    assert_eq!(done.reshaped, [merged(Some(7), 0, 1, Ok(0))]);
    assert_eq!(cluster.tick(LEASE + 11), Changes::default());
}

#[test]
fn a_merge_that_is_not_done_within_the_lease_ends_by_what_the_list_says() {
    // The list has the region as absorbed: the merge was done, and only the
    // worker's word was lost.
    let (mut cluster, list) = absorbing();
    let late = 2 * LEASE + 2;
    beat(&mut cluster, 2 * LEASE);
    assert_eq!(cluster.tick(2 * LEASE + 1), Changes::default());
    assert_eq!(cluster.tick(late), reads());
    // Not before the list has been read is the region given to anyone.
    assert!(cluster.assignments("b").is_empty());
    assert_eq!(cluster.tick(late + 1), Changes::default());
    assert_eq!(
        cluster.merge(late + 1, 0, 2, None),
        Err(ReshapeRefusal::Reserved(RegionId(0)))
    );
    let done = cluster.listed(late + 2, &after_the_merge(&list));
    let expected = Changes {
        routing: true,
        reshaped: vec![merged(Some(7), 0, 1, Ok(0))],
        ..Changes::default()
    };
    assert_eq!(done, expected);

    // The list has the region as living: it is assigned, above the epoch the
    // survivor's owner was to open it with, which fences an absorb on its way.
    let (mut cluster, list) = absorbing();
    beat(&mut cluster, 2 * LEASE);
    assert_eq!(cluster.tick(late), reads());
    let off = cluster.listed(late + 2, &list);
    let expected = Changes {
        reshaped: vec![merged(Some(7), 0, 1, Err(Undone::Overdue))],
        ..changes(&["b"], true)
    };
    assert_eq!(off, expected);
    assert_eq!(cluster.assignments("b"), [assignment(1, E + 5, 3)]);

    // The list cannot be read either: the region is assigned all the same.
    let (mut cluster, list) = absorbing();
    beat(&mut cluster, 2 * LEASE);
    assert_eq!(cluster.tick(late), reads());
    let off = cluster.unlisted(late + 2);
    let expected = Changes {
        reshaped: vec![merged(Some(7), 0, 1, Err(Undone::Unread))],
        ..changes(&["b"], true)
    };
    assert_eq!(off, expected);
    assert_eq!(cluster.assignments("b"), [assignment(1, E + 5, 3)]);
    // It had been absorbed: the world store refuses the worker that was given
    // it, which says so, and the list that is read for that takes it away.
    assert_eq!(cluster.absorb_ended(late + 3, "b", 0, 1, Ok(())), reads());
    let gone = cluster.listed(late + 4, &after_the_merge(&list));
    assert_eq!(gone, changes(&["b"], true));
    assert!(cluster.assignments("b").is_empty());
    assert!(cluster.table().is_complete());
}

#[test]
fn a_survivors_owner_that_registers_again_is_told_again_to_absorb() {
    let (mut cluster, _) = absorbing();
    let held = cluster.assignments("a");
    let again = cluster.register(LEASE + 3, "a", "a:25601", &held);
    let expected = Changes {
        orders: vec![absorb("a", 0, E + 1, 1, E + 4)],
        ..Changes::default()
    };
    assert_eq!(again, expected);
    // Also when it comes back without the region, as a process that was replaced
    // does: it is to run it still, and to absorb.
    assert_eq!(cluster.register(LEASE + 4, "a", "a:25601", &[]), expected);
    // Another worker's registration tells nobody anything.
    let held = cluster.assignments("c");
    assert_eq!(
        cluster.register(LEASE + 5, "c", "c:25601", &held),
        Changes::default()
    );
    // Nor is it told again once it has said what came of it.
    cluster.absorb_ended(LEASE + 6, "a", 0, 1, Err(Off::Busy));
    let held = cluster.assignments("a");
    assert_eq!(
        cluster.register(LEASE + 7, "a", "a:25601", &held),
        Changes::default()
    );
}

#[test]
fn the_owner_of_the_region_to_absorb_is_asked_again_and_without_it_has_released_it() {
    let (mut cluster, _) = three_stripes();
    cluster.merge(LEASE + 1, 0, 1, None).unwrap();
    // It lost its connection and may not have heard.
    let held = cluster.assignments("b");
    let again = cluster.register(LEASE + 2, "b", "b:25601", &held);
    assert_eq!(again, asks(&[order("b", 1, E + 2)]));
    // The survivor's owner is not told to prepare a second time.
    let held = cluster.assignments("a");
    assert_eq!(
        cluster.register(LEASE + 3, "a", "a:25601", &held),
        Changes::default()
    );
    // It comes back without the region: that is its word that it let go.
    let released = cluster.register(LEASE + 4, "b", "b:25601", &[]);
    let expected = Changes {
        orders: vec![absorb("a", 0, E + 1, 1, E + 4)],
        ..changes(&["b"], true)
    };
    assert_eq!(released, expected);
    // What it says afterwards changes nothing.
    assert_eq!(
        cluster.released(LEASE + 5, "b", 1, E + 2),
        Changes::default()
    );
    assert_eq!(
        cluster.released(LEASE + 5, "b", 1, E + 4),
        Changes::default()
    );
    assert!(cluster.assignments("b").is_empty());
}

#[test]
fn one_worker_that_owns_both_regions_merges_them() {
    let mut cluster = Cluster::new(&[0]);
    cluster.register(0, "a", "a:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    let list = stripes(2);
    cluster.listed(LEASE, &list);
    let asked = cluster.merge(LEASE + 1, 0, 1, Some(3)).unwrap();
    let expected = Changes {
        releases: vec![order("a", 1, E + 2)],
        orders: vec![prepare("a", 0, E + 1)],
        ..Changes::default()
    };
    assert_eq!(asked, expected);
    let released = cluster.released(LEASE + 2, "a", 1, E + 2);
    let expected = Changes {
        orders: vec![absorb("a", 0, E + 1, 1, E + 3)],
        ..changes(&["a"], true)
    };
    assert_eq!(released, expected);
    assert_eq!(regions_of(&cluster, "a"), [0]);
    // It registers again without the region it is to absorb, which it never
    // reports: it is told once, not twice.
    let held = cluster.assignments("a");
    assert_eq!(
        cluster.register(LEASE + 3, "a", "a:25601", &held).orders,
        [absorb("a", 0, E + 1, 1, E + 3)]
    );
    cluster.absorb_ended(LEASE + 4, "a", 0, 1, Ok(()));
    let list = RegionList {
        regions: vec![living(0, E + 1)],
        absorbed: vec![(RegionId(1), RegionId(0))],
        ..list
    };
    let done = cluster.listed(LEASE + 5, &list);
    assert_eq!(done.reshaped, [merged(Some(3), 0, 1, Ok(0))]);
    assert!(cluster.table().is_complete());
}

#[test]
fn a_merge_ends_when_the_survivor_changes_hands_and_the_list_is_read_before_anything_is_assigned() {
    // At the second stage the survivor's worker falls silent. Its lease runs
    // out before the merge's does.
    let (mut cluster, list) = absorbing();
    let late = 2 * LEASE + 1;
    for name in ["b", "c"] {
        assert!(cluster.heartbeat(2 * LEASE, name));
    }
    // The survivor goes to another worker at once; the other region waits for
    // the list.
    let lost = cluster.tick(late);
    let expected = Changes {
        read: true,
        ..changes(&["a", "b"], true)
    };
    assert_eq!(lost, expected);
    assert_eq!(cluster.assignments("b"), [assignment(0, E + 5, 3)]);
    assert_eq!(cluster.coordinator.waiting(), [RegionId(1)]);
    // The list has the pair: the worker had done it before it died.
    let done = cluster.listed(late + 1, &after_the_merge(&list));
    assert_eq!(done.reshaped, [merged(Some(7), 0, 1, Ok(0))]);
    assert!(cluster.table().is_complete());

    // The same, and the list has both regions: the other one is assigned too.
    let (mut cluster, list) = absorbing();
    for name in ["b", "c"] {
        assert!(cluster.heartbeat(2 * LEASE, name));
    }
    cluster.tick(late);
    let off = cluster.listed(late + 1, &list);
    let disowned = Err(Undone::Disowned(RegionId(0)));
    let expected = Changes {
        reshaped: vec![merged(Some(7), 0, 1, disowned)],
        ..changes(&["c"], true)
    };
    assert_eq!(off, expected);
    assert_eq!(regions_of(&cluster, "b"), [0]);
    assert_eq!(regions_of(&cluster, "c"), [1, 2]);

    // At the first stage: the region that was to be absorbed is taken from its
    // owner like one that was not released, and both are assigned.
    let (mut cluster, _) = three_stripes();
    cluster.merge(LEASE + 1, 0, 1, Some(7)).unwrap();
    for name in ["b", "c"] {
        assert!(cluster.heartbeat(2 * LEASE, name));
    }
    let lost = cluster.tick(late);
    let expected = Changes {
        reshaped: vec![merged(Some(7), 0, 1, disowned)],
        ..changes(&["a", "b", "c"], true)
    };
    assert_eq!(lost, expected);
    assert!(cluster.table().is_complete());
    // Its owner was asked to let go and still had the time to: it has not failed
    // the region, and is given the survivor like any worker that runs nothing.
    assert_eq!(cluster.coordinator.workers["b"].failed, None);
    assert_eq!(regions_of(&cluster, "b"), [0]);
    assert_eq!(regions_of(&cluster, "c"), [1, 2]);
}

/// A worker that has failed a region is passed over for six leases. That is for
/// the owner that did not release in time, and for no other.
#[test]
fn only_a_merge_whose_time_is_up_counts_against_the_owner_that_was_to_release() {
    // The time is up: the owner has failed the region.
    let (mut cluster, _) = three_stripes();
    cluster.merge(LEASE + 1, 0, 1, None).unwrap();
    beat(&mut cluster, 2 * LEASE);
    let late = 2 * LEASE + 2;
    let ended = cluster.tick(late);
    assert_eq!(
        ended.reshaped,
        [merged(None, 0, 1, Err(Undone::NotReleased))]
    );
    assert_eq!(
        cluster.coordinator.workers["b"].failed,
        Some(cluster.at(late))
    );
    assert_eq!(regions_of(&cluster, "a"), [0, 1]);
    assert!(cluster.assignments("b").is_empty());

    // The survivor's owner says by itself that it lets go of the survivor, well
    // within the merge's time. The merge is off, and the other region is taken
    // from its owner all the same, which was told to release it and may be in the
    // middle of that; but that owner has not failed it, and is passed over for
    // nothing.
    let (mut cluster, _) = three_stripes();
    cluster.merge(LEASE + 1, 0, 1, Some(7)).unwrap();
    let ended = cluster.released(LEASE + 2, "a", 0, E + 1);
    let disowned = Err(Undone::Disowned(RegionId(0)));
    let expected = Changes {
        reshaped: vec![merged(Some(7), 0, 1, disowned)],
        ..changes(&["a", "b"], true)
    };
    assert_eq!(ended, expected);
    assert_eq!(cluster.coordinator.workers["b"].failed, None);
    // The survivor went to the first of the others, and the other region to the
    // worker with the fewest then.
    assert_eq!(regions_of(&cluster, "b"), [0]);
    assert_eq!(regions_of(&cluster, "a"), [1]);
    assert_eq!(regions_of(&cluster, "c"), [2]);
    // What it says of the region it was to release changes nothing any more.
    assert_eq!(
        cluster.released(LEASE + 3, "b", 1, E + 2),
        Changes::default()
    );
}

#[test]
fn a_merge_ends_when_the_owner_of_the_region_to_absorb_dies_before_it_has_let_go() {
    let (mut cluster, _) = three_stripes();
    cluster.merge(LEASE + 1, 0, 1, Some(7)).unwrap();
    for name in ["a", "c"] {
        assert!(cluster.heartbeat(2 * LEASE, name));
    }
    // Its lease runs out a moment before the merge's would. The region is
    // assigned like any whose owner died.
    let lost = cluster.tick(2 * LEASE + 1);
    let disowned = Err(Undone::Disowned(RegionId(1)));
    let expected = Changes {
        reshaped: vec![merged(Some(7), 0, 1, disowned)],
        ..changes(&["a", "b"], true)
    };
    assert_eq!(lost, expected);
    assert_eq!(regions_of(&cluster, "a"), [0, 1]);
    assert!(cluster.coordinator.merges.is_empty());
}

#[test]
fn a_region_whose_epoch_the_store_refused_for_a_merge_is_assigned_above_it() {
    let (mut cluster, list) = absorbing();
    // Somebody has opened the region since, with an epoch nobody here issued.
    let refused = cluster.epoch_refused(LEASE + 3, "a", 1, E + 50);
    assert_eq!(refused, Changes::default());
    // An order that is given again still names the epoch the merge was begun
    // with: the survivor's worker is not to take the region from that somebody.
    let held = cluster.assignments("a");
    assert_eq!(
        cluster.register(LEASE + 3, "a", "a:25601", &held).orders,
        [absorb("a", 0, E + 1, 1, E + 4)]
    );
    assert_eq!(
        cluster.absorb_ended(LEASE + 4, "a", 0, 1, Err(Off::Refused)),
        reads()
    );
    let list = RegionList {
        regions: vec![living(0, E + 1), living(1, E + 50), living(2, E + 3)],
        ..list
    };
    let off = cluster.listed(LEASE + 5, &list);
    assert_eq!(
        off.reshaped,
        [merged(Some(7), 0, 1, Err(Undone::Off(Off::Refused)))]
    );
    assert_eq!(cluster.assignments("b"), [assignment(1, E + 51, 3)]);
}

#[test]
fn a_merge_whose_region_went_elsewhere_is_not_said_to_be_done() {
    let (mut cluster, list) = absorbing();
    // Somebody else had region 2 absorb the region meanwhile.
    let elsewhere = RegionList {
        regions: vec![living(0, E + 1), living(2, E + 3)],
        absorbed: vec![(RegionId(1), RegionId(2))],
        ..list
    };
    let gone = cluster.listed(LEASE + 3, &elsewhere);
    let expected = Changes {
        routing: true,
        reshaped: vec![merged(Some(7), 0, 1, Err(Undone::Gone(RegionId(1))))],
        ..Changes::default()
    };
    assert_eq!(gone, expected);
    assert!(cluster.table().is_complete());
}

#[test]
fn nothing_is_merged_or_split_once_the_epochs_have_run_out() {
    // A worker reports a region with the last epoch there is.
    let last = part(9, u64::MAX);
    let mut cluster = Cluster::new(&[0]);
    cluster.register(0, "a", "a:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    cluster.listed(LEASE, &stripes(2));
    cluster.register(LEASE, "b", "b:25601", &[last]);
    assert_eq!(
        cluster.merge(LEASE, 0, 1, None),
        Err(ReshapeRefusal::NoEpoch)
    );
    assert_eq!(
        cluster.split(LEASE, 1, &CHUNKS, None),
        Err(ReshapeRefusal::NoEpoch)
    );

    // They run out between the asking and the release: the region that was let
    // go of cannot be opened anew, by anyone.
    let mut cluster = Cluster::new(&[0]);
    cluster.register(0, "a", "a:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    cluster.listed(LEASE, &stripes(2));
    cluster.merge(LEASE, 0, 1, Some(7)).unwrap();
    cluster.register(LEASE, "b", "b:25601", &[last]);
    let ended = cluster.released(LEASE, "a", 1, E + 2);
    let expected = Changes {
        reshaped: vec![merged(Some(7), 0, 1, Err(Undone::NoEpoch))],
        ..changes(&["a"], true)
    };
    assert_eq!(ended, expected);
    assert_eq!(cluster.coordinator.waiting(), [RegionId(1)]);
    assert_eq!(cluster.tick(LEASE), Changes::default());
}

/// The list is read for other reasons too, such as a worker that registers.
#[test]
fn a_reading_while_a_merge_is_under_way_and_not_done_decides_nothing() {
    let (mut cluster, list) = three_stripes();
    cluster.merge(LEASE + 1, 0, 1, Some(7)).unwrap();
    assert_eq!(cluster.listed(LEASE + 2, &list), Changes::default());
    cluster.released(LEASE + 3, "b", 1, E + 2);
    let table = cluster.table();
    // The survivor's worker has opened the region, and nothing more yet.
    let opened = RegionList {
        regions: vec![living(0, E + 1), living(1, E + 4), living(2, E + 3)],
        ..list.clone()
    };
    for at in [LEASE + 4, LEASE + 5] {
        assert_eq!(cluster.listed(at, &opened), Changes::default());
        assert_eq!(cluster.unlisted(at), Changes::default());
    }
    assert_eq!(cluster.table(), table);
    assert_eq!(cluster.tick(LEASE + 6), Changes::default());
    // One that shows it done ends it, without the worker's word.
    let done = cluster.listed(LEASE + 7, &after_the_merge(&list));
    assert_eq!(done.reshaped, [merged(Some(7), 0, 1, Ok(0))]);
    // Its word then only has the list read once more.
    assert_eq!(cluster.absorb_ended(LEASE + 8, "a", 0, 1, Ok(())), reads());
    assert_eq!(
        cluster.listed(LEASE + 9, &after_the_merge(&list)),
        Changes::default()
    );
}

#[test]
fn a_reading_that_takes_the_survivor_away_ends_the_merge() {
    // Somebody else has merged the survivor into a third region meanwhile.
    let gone = |list: &RegionList| RegionList {
        regions: vec![living(0, E + 1), living(2, E + 3)],
        absorbed: vec![(RegionId(1), RegionId(0))],
        ..list.clone()
    };
    let taken_away = Err(Undone::Gone(RegionId(1)));

    // At the first stage the other region is taken from its owner all the same,
    // and goes to the worker that has just lost the survivor.
    let (mut cluster, list) = three_stripes();
    cluster.merge(LEASE + 1, 1, 2, Some(7)).unwrap();
    let ended = cluster.listed(LEASE + 2, &gone(&list));
    let expected = Changes {
        reshaped: vec![merged(Some(7), 1, 2, taken_away)],
        ..changes(&["b", "c"], true)
    };
    assert_eq!(ended, expected);
    assert_eq!(regions_of(&cluster, "b"), [2]);
    assert!(cluster.table().is_complete());
    // Its owner has not failed it: the survivor's side ended the merge.
    assert_eq!(cluster.coordinator.workers["c"].failed, None);

    // At the second it has no owner, and is assigned by the reading that ended
    // the merge.
    let (mut cluster, list) = three_stripes();
    cluster.merge(LEASE + 1, 1, 2, Some(7)).unwrap();
    cluster.released(LEASE + 2, "c", 2, E + 3);
    let ended = cluster.listed(LEASE + 3, &gone(&list));
    let expected = Changes {
        reshaped: vec![merged(Some(7), 1, 2, taken_away)],
        ..changes(&["b"], true)
    };
    assert_eq!(ended, expected);
    assert_eq!(cluster.assignments("b"), [assignment(2, E + 5, 3)]);
    assert!(cluster.table().is_complete());
}

/// What the worker `name` runs of a region that was split off another: it has the
/// epoch its worker was told, and no entity ids.
fn part(region: u32, epoch: u64) -> Assignment {
    Assignment {
        region: RegionId(region),
        epoch,
        entity_ids: NO_ENTITY_IDS,
    }
}

/// The list of [`three_stripes`] after region 2 was split, the new region having
/// been opened with the epoch `E + 4`.
fn after_the_split(list: &RegionList) -> RegionList {
    let mut regions = list.regions.clone();
    regions.push(living(3, E + 4));
    RegionList {
        regions,
        next: RegionId(4),
        ..list.clone()
    }
}

#[test]
fn a_split_names_the_next_region_of_the_list_and_the_new_region_is_its_workers() {
    let (mut cluster, list) = three_stripes();
    let asked = cluster.split(LEASE + 1, 2, &CHUNKS, Some(7)).unwrap();
    let expected = Changes {
        orders: vec![split_off("c", 2, E + 3, E + 4, 3)],
        ..Changes::default()
    };
    assert_eq!(asked, expected);
    // The region is reserved, and its owner is not asked a second time.
    assert_eq!(
        cluster.split(LEASE + 1, 2, &CHUNKS, None),
        Err(ReshapeRefusal::Reserved(RegionId(2)))
    );
    assert_eq!(
        cluster.move_region(LEASE + 1, 2, None, 5),
        Err(MoveRefusal::Reserved(RegionId(2)))
    );
    let held = cluster.assignments("c");
    assert_eq!(
        cluster.register(LEASE + 1, "c", "c:25601", &held),
        Changes::default()
    );

    // The worker's word makes the new region its, with the epoch it was told.
    let done = cluster.split_ended(LEASE + 2, "c", 2, E + 4, Ok(3));
    let expected = Changes {
        read: true,
        reshaped: vec![was_split(Some(7), 2, Ok(3))],
        ..changes(&["c"], true)
    };
    assert_eq!(done, expected);
    assert_eq!(
        cluster.assignments("c"),
        [assignment(2, E + 3, 2), part(3, E + 4)]
    );
    let table = cluster.table();
    assert_eq!(table.route(RegionId(3)), Some(&route(3, E + 4, "c:25601")));
    assert!(table.is_complete());
    // The list that is read for it has nothing new.
    assert_eq!(
        cluster.listed(LEASE + 3, &after_the_split(&list)),
        Changes::default()
    );
    // Said again, as a worker does until its orders name the region: the list is
    // read, and that is all.
    assert_eq!(
        cluster.split_ended(LEASE + 4, "c", 2, E + 4, Ok(3)),
        reads()
    );

    // The new region is kept by its worker's heartbeats like any other, and lost
    // without them.
    beat(&mut cluster, 2 * LEASE);
    assert_eq!(cluster.tick(2 * LEASE + 3), Changes::default());
    let vouches = [(RegionId(2), Vouch::Committed)];
    for at in [3 * LEASE, 3 * LEASE + 1] {
        for name in ["a", "b"] {
            assert!(cluster.heartbeat(at, name));
        }
        assert!(cluster.heartbeat_with(at, "c", &vouches));
    }
    assert_eq!(cluster.tick(3 * LEASE), Changes::default());
    assert_eq!(cluster.tick(3 * LEASE + 1), changes(&["a", "c"], true));
    assert_eq!(regions_of(&cluster, "a"), [0, 3]);
}

#[test]
fn a_split_that_came_to_nothing_leaves_the_region_as_it_was() {
    let (mut cluster, list) = three_stripes();
    cluster.split(LEASE + 1, 2, &CHUNKS, Some(7)).unwrap();
    // An answer for another order, or from another worker, is not this one's.
    assert_eq!(
        cluster.split_ended(LEASE + 2, "c", 2, E + 9, Err(Off::Nobody)),
        reads()
    );
    assert_eq!(
        cluster.split_ended(LEASE + 2, "b", 2, E + 4, Err(Off::Nobody)),
        reads()
    );
    let off = cluster.split_ended(LEASE + 2, "c", 2, E + 4, Err(Off::Nobody));
    let expected = Changes {
        read: true,
        reshaped: vec![was_split(Some(7), 2, Err(Undone::Off(Off::Nobody)))],
        ..Changes::default()
    };
    assert_eq!(off, expected);
    assert_eq!(cluster.listed(LEASE + 3, &list), Changes::default());
    // It can be asked again, and gets another epoch for the new region.
    let asked = cluster.split(LEASE + 4, 2, &CHUNKS, None).unwrap();
    assert_eq!(asked.orders, [split_off("c", 2, E + 3, E + 5, 3)]);
}

#[test]
fn a_split_without_the_workers_word_is_overdue_after_a_lease_whatever_the_list_shows() {
    let late = 2 * LEASE + 2;
    let lapsing = || {
        let (mut cluster, list) = three_stripes();
        cluster.split(LEASE + 1, 2, &CHUNKS, Some(7)).unwrap();
        beat(&mut cluster, 2 * LEASE);
        assert_eq!(cluster.tick(2 * LEASE + 1), Changes::default());
        // Whoever asked is told at once, and the list is asked for.
        let expected = Changes {
            read: true,
            reshaped: vec![was_split(Some(7), 2, Err(Undone::Overdue))],
            ..Changes::default()
        };
        assert_eq!(cluster.tick(late), expected);
        // The region is free again.
        assert!(cluster.coordinator.splits.is_empty());
        (cluster, list)
    };

    // The list has a region with the id that was ordered, which nobody is known
    // to run: it is assigned. That it is this split's nobody can tell, as the
    // store may have made this one's under the next id; nobody is told anything.
    let (mut cluster, list) = lapsing();
    let found = cluster.listed(late + 1, &after_the_split(&list));
    assert_eq!(found, changes(&["a"], true));
    assert_eq!(cluster.assignments("a")[1], assignment(3, E + 5, 3));
    assert!(cluster.table().is_complete());
    assert_eq!(cluster.tick(late + 2), Changes::default());

    // The list has none.
    let (mut cluster, list) = lapsing();
    assert_eq!(cluster.listed(late + 1, &list), Changes::default());
    assert_eq!(cluster.tick(late + 2), Changes::default());

    // The list cannot be read: it is asked for at every tick until it can be, as
    // there may be a region that nobody runs.
    let (mut cluster, list) = lapsing();
    assert_eq!(cluster.unlisted(late + 1), Changes::default());
    assert_eq!(cluster.tick(late + 2), reads());
    assert_eq!(cluster.tick(late + 3), Changes::default());
    assert_eq!(cluster.unlisted(late + 4), Changes::default());
    assert_eq!(cluster.tick(late + 5), reads());
    let found = cluster.listed(late + 6, &after_the_split(&list));
    assert_eq!(found, changes(&["a"], true));
    assert_eq!(cluster.tick(late + 7), Changes::default());
}

#[test]
fn a_split_ends_when_its_region_changes_hands() {
    let (mut cluster, list) = three_stripes();
    cluster.split(LEASE + 1, 2, &CHUNKS, Some(7)).unwrap();
    // The world store refuses the owner's epoch: the worker has dropped the
    // region, and is given it anew, with another epoch. The split was of the
    // one it had.
    // Whoever asked is told that nobody knows what came of it.
    let refused = cluster.epoch_refused(LEASE + 2, "c", 2, E + 20);
    let disowned = Err(Undone::Disowned(RegionId(2)));
    let expected = Changes {
        read: true,
        reshaped: vec![was_split(Some(7), 2, disowned)],
        ..changes(&["c"], true)
    };
    assert_eq!(refused, expected);
    assert_eq!(cluster.assignments("c"), [assignment(2, E + 21, 3)]);
    // Its answer comes when the reservation is over. It is taken as a report:
    // nobody else is known to run the new region.
    let late = cluster.split_ended(LEASE + 3, "c", 2, E + 4, Ok(3));
    let expected = Changes {
        read: true,
        ..changes(&["c"], true)
    };
    assert_eq!(late, expected);
    // The list has nothing to add to that, and says nothing to whoever asked.
    let found = cluster.listed(LEASE + 4, &after_the_split(&list));
    assert_eq!(found, Changes::default());
    assert_eq!(cluster.assignments("c")[1], part(3, E + 4));

    // When a reading of the list takes the region away, it is gone, not disowned.
    let (mut cluster, list) = three_stripes();
    cluster.split(LEASE + 1, 2, &CHUNKS, Some(7)).unwrap();
    let gone = RegionList {
        regions: vec![living(0, E + 1), living(1, E + 2)],
        absorbed: vec![(RegionId(2), RegionId(1))],
        ..list
    };
    let ended = cluster.listed(LEASE + 2, &gone);
    let expected = Changes {
        reshaped: vec![was_split(Some(7), 2, Err(Undone::Gone(RegionId(2))))],
        ..changes(&["c"], true)
    };
    assert_eq!(ended, expected);
    assert_eq!(cluster.tick(LEASE + 3), Changes::default());
}

#[test]
fn a_reading_while_a_split_is_reserved_leaves_the_new_region_to_the_worker_that_made_it() {
    let (mut cluster, list) = three_stripes();
    cluster.split(LEASE + 1, 2, &CHUNKS, None).unwrap();
    // The store has the record, and the worker's word is still on its way.
    let split_list = after_the_split(&list);
    assert_eq!(cluster.listed(LEASE + 2, &split_list), Changes::default());
    assert!(cluster.table().route(RegionId(3)).is_none());
    assert!(cluster.table().is_complete());
    assert_eq!(cluster.tick(LEASE + 3), Changes::default());

    let done = cluster.split_ended(LEASE + 4, "c", 2, E + 4, Ok(3));
    assert_eq!(done.workers, ["c"]);
    assert_eq!(cluster.assignments("c")[1], part(3, E + 4));

    // Had the reservation run out instead, the same reading would have added the
    // region, without an owner, and it would have been assigned.
    let (mut cluster, _) = three_stripes();
    cluster.split(LEASE + 1, 2, &CHUNKS, None).unwrap();
    beat(&mut cluster, 2 * LEASE);
    let lapsed = cluster.tick(2 * LEASE + 2);
    assert_eq!(lapsed.reshaped, [was_split(None, 2, Err(Undone::Overdue))]);
    assert!(lapsed.read);
    let found = cluster.listed(2 * LEASE + 3, &split_list);
    assert_eq!(found, changes(&["a"], true));
    assert_eq!(cluster.assignments("a")[1], assignment(3, E + 5, 3));
}

#[test]
fn word_of_a_split_that_nobody_reserved_is_taken_as_a_report_of_the_new_region() {
    let (mut cluster, _) = three_stripes();
    let said = cluster.split_ended(LEASE + 1, "c", 2, E + 30, Ok(5));
    let expected = Changes {
        read: true,
        ..changes(&["c"], true)
    };
    assert_eq!(said, expected);
    assert_eq!(cluster.assignments("c")[1], part(5, E + 30));
    // No epoch is issued at or below the one it has.
    assert_eq!(
        cluster.split(LEASE + 1, 0, &CHUNKS, None).unwrap().orders,
        [split_off("a", 0, E + 1, E + 31, 3)]
    );

    // Another worker that says the same of that region is not believed, nor is
    // one that says it of a region somebody runs with a higher epoch.
    assert_eq!(
        cluster.split_ended(LEASE + 2, "b", 1, E + 40, Ok(5)),
        reads()
    );
    assert_eq!(
        cluster.split_ended(LEASE + 2, "c", 2, E + 2, Ok(5)),
        reads()
    );
    assert_eq!(cluster.assignments("c")[1], part(5, E + 30));
    // That nothing came of a split nobody knows of only has the list read, and
    // so has whatever a worker says that is not registered.
    assert_eq!(
        cluster.split_ended(LEASE + 3, "b", 1, E + 41, Err(Off::Busy)),
        reads()
    );
    assert_eq!(
        cluster.split_ended(LEASE + 3, "z", 1, E + 41, Ok(9)),
        reads()
    );
    assert_eq!(cluster.absorb_ended(LEASE + 3, "z", 1, 2, Ok(())), reads());
}

#[test]
fn the_list_adds_the_regions_nobody_runs_and_takes_away_those_that_are_no_more() {
    let (mut cluster, list) = three_stripes();
    let version = cluster.table().version;
    // A region that was split off, whose worker died before it said so.
    let found = cluster.listed(LEASE + 1, &after_the_split(&list));
    assert_eq!(found, changes(&["a"], true));
    assert_eq!(cluster.assignments("a")[1], assignment(3, E + 5, 3));
    assert_eq!(cluster.table().version, version + 1);

    // Region 2 went into region 1: its owner loses it, and the table has the pair.
    let merged_list = RegionList {
        regions: vec![living(0, E + 1), living(1, E + 2), living(3, E + 5)],
        absorbed: vec![(RegionId(2), RegionId(1))],
        next: RegionId(4),
        ..list
    };
    let gone = cluster.listed(LEASE + 2, &merged_list);
    assert_eq!(gone, changes(&["c"], true));
    assert!(cluster.assignments("c").is_empty());
    let table = cluster.table();
    assert_eq!(table.absorbed, [(RegionId(2), RegionId(1))]);
    assert_eq!(table.version, version + 2);
    assert!(table.is_complete());
    // The same list again changes nothing, and the table keeps its version.
    assert_eq!(cluster.listed(LEASE + 3, &merged_list), Changes::default());
    assert_eq!(cluster.table(), table);

    // A reading that failed changes nothing either: the table keeps the pairs.
    assert_eq!(cluster.unlisted(LEASE + 4), Changes::default());
    assert_eq!(cluster.table(), table);

    // A region that is being moved and is no more: whoever asked is told.
    cluster.move_region(LEASE + 5, 3, Some("b"), 9).unwrap();
    let merged_list = RegionList {
        regions: vec![living(0, E + 1), living(1, E + 2)],
        absorbed: vec![(RegionId(2), RegionId(1)), (RegionId(3), RegionId(0))],
        ..merged_list
    };
    let gone = cluster.listed(LEASE + 6, &merged_list);
    let expected = Changes {
        moves: vec![outcome(9, 3, None, false)],
        ..changes(&["a"], true)
    };
    assert_eq!(gone, expected);
}

#[test]
fn a_new_coordinator_assigns_what_the_list_shows_when_its_grace_period_is_over() {
    let mut cluster = Cluster::new(&[0]);
    cluster.register(0, "a", "a:25601", &[]);
    // Three regions more than the coordinator knows; one of them was opened with an
    // epoch far above the coordinator's.
    let list = RegionList {
        regions: vec![living(0, 0), living(1, 0), living(4, E + 70), living(6, 3)],
        absorbed: vec![(RegionId(2), RegionId(0)), (RegionId(3), RegionId(5))],
        next: RegionId(7),
        ..stripes(2)
    };
    assert_eq!(cluster.listed(1, &list), changes(&[], true));
    let table = cluster.table();
    assert_eq!(table.waiting, 4);
    assert_eq!(table.home, Some(RegionId(0)));
    assert_eq!(cluster.tick(LEASE - 1), Changes::default());

    // A worker that ran one of them under the coordinator before reports it.
    let held = part(6, 3);
    assert_eq!(
        cluster.register(LEASE - 1, "b", "b:25601", &[held]),
        changes(&["b"], true)
    );
    // One that reports a region with an epoch below the one the store has was
    // replaced, whoever replaced it.
    assert_eq!(
        cluster.register(LEASE - 1, "c", "c:25601", &[part(4, E + 69)]),
        Changes::default()
    );

    // The lowest first, each to the worker with the fewest then.
    assert_eq!(cluster.tick(LEASE), changes(&["a", "c"], true));
    assert_eq!(regions_of(&cluster, "a"), [0, 4]);
    assert_eq!(regions_of(&cluster, "b"), [6]);
    assert_eq!(regions_of(&cluster, "c"), [1]);
    // Every epoch it issues is above the highest of the list.
    let epochs: Vec<u64> = cluster.table().routes.iter().map(|r| r.epoch).collect();
    assert_eq!(epochs, [E + 71, E + 72, E + 73, 3]);
    assert!(cluster.table().is_complete());
}

#[test]
fn a_new_coordinator_finds_a_merge_that_was_under_way_as_the_list_and_the_workers_have_it() {
    // Region 1 was released for region 0 to absorb it, and the survivor's worker
    // has opened it with the epoch `E + 4`. Then the coordinator was replaced.
    let mut cluster = Cluster::with_first_epoch(&[0, 4], E + 100);
    let list = RegionList {
        regions: vec![living(0, E + 1), living(1, E + 4), living(2, E + 3)],
        ..stripes(3)
    };
    cluster.register(1, "a", "a:25601", &[assignment(0, E + 1, 0)]);
    cluster.register(1, "c", "c:25601", &[assignment(2, E + 3, 2)]);
    cluster.register(1, "b", "b:25601", &[]);
    assert_eq!(cluster.listed(2, &list), changes(&[], true));
    // The worker that released it says so again only to a coordinator whose
    // orders name the region, which this one's never did. If its word was still
    // on its way, it is of an epoch the region has left behind.
    assert_eq!(cluster.released(3, "b", 1, E + 2), Changes::default());
    // So the region waits out the grace period like any without an owner.
    assert_eq!(cluster.tick(LEASE - 1), Changes::default());
    assert!(!cluster.table().is_complete());
    beat(&mut cluster, LEASE);
    assert_eq!(cluster.tick(LEASE), changes(&["b"], true));
    assert_eq!(cluster.assignments("b"), [assignment(1, E + 101, 1)]);

    // Had the survivor's worker absorbed it meanwhile, its word, which is for a
    // merge this coordinator has no note of, would have the list read, and the
    // region would be gone.
    assert_eq!(cluster.absorb_ended(LEASE + 1, "a", 0, 1, Ok(())), reads());
    let gone = cluster.listed(LEASE + 2, &after_the_merge(&list));
    assert_eq!(gone, changes(&["b"], true));
    assert!(cluster.table().is_complete());

    // A coordinator that has not read the list yet takes the word of the worker
    // that released the region: it is free, and assigned at once, to that very
    // worker if it has the fewest. That fences the absorb if it is not done yet.
    let mut cluster = Cluster::with_first_epoch(&[0, 4], E + 100);
    cluster.register(1, "a", "a:25601", &[assignment(0, E + 1, 0)]);
    cluster.register(1, "b", "b:25601", &[]);
    assert_eq!(cluster.released(3, "b", 1, E + 2), changes(&["b"], true));
    assert_eq!(cluster.assignments("b"), [assignment(1, E + 101, 1)]);
}

#[test]
fn a_new_coordinator_learns_of_a_region_that_was_split_off_from_its_worker_or_from_the_list() {
    let list = after_the_split(&RegionList {
        regions: vec![living(0, E + 1), living(1, E + 2), living(2, E + 3)],
        ..stripes(3)
    });
    // Its worker lives and reports it.
    let mut cluster = Cluster::with_first_epoch(&[0, 4], E + 100);
    let held = [assignment(2, E + 3, 2), part(3, E + 4)];
    assert_eq!(
        cluster.register(1, "c", "c:25601", &held),
        changes(&["c"], true)
    );
    assert_eq!(cluster.assignments("c"), held);
    assert_eq!(cluster.listed(2, &list), changes(&[], true));
    assert_eq!(cluster.assignments("c"), held);

    // Its worker died: the list has the region, and it is assigned when the
    // grace period is over.
    let mut cluster = Cluster::with_first_epoch(&[0, 4], E + 100);
    cluster.register(1, "a", "a:25601", &[]);
    assert_eq!(cluster.listed(2, &list), changes(&[], true));
    assert_eq!(cluster.table().waiting, 4);
    assert_eq!(cluster.tick(LEASE - 1), Changes::default());
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    assert_eq!(regions_of(&cluster, "a"), [0, 1, 2, 3]);
}

#[test]
fn nothing_is_evened_out_during_a_split_or_within_a_lease_of_its_end() {
    let mut cluster = Cluster::new(&[0]);
    cluster.register(0, "a", "a:25601", &[]);
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    cluster.listed(LEASE, &stripes(2));
    assert!(cluster.heartbeat(LEASE, "a"));
    cluster.split(LEASE + 1, 1, &CHUNKS, None).unwrap();
    // A worker that runs nothing, beside one that runs two regions.
    cluster.register(LEASE + 2, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(LEASE + 3), Changes::default());

    let ended = LEASE + 4;
    cluster.split_ended(ended, "a", 1, E + 3, Ok(2));
    assert_eq!(regions_of(&cluster, "a"), [0, 1, 2]);
    for quarter in 0..4 {
        let now = ended + quarter * LEASE / 4;
        for name in ["a", "b"] {
            assert!(cluster.heartbeat(now, name));
        }
        assert_eq!(cluster.tick(now), Changes::default(), "at {now}");
    }
    assert_eq!(cluster.tick(ended + LEASE - 1), Changes::default());
    // A lease after it ended, the new region is the first to go.
    assert_eq!(cluster.tick(ended + LEASE), asks(&[order("a", 2, E + 3)]));
}

#[test]
fn nothing_is_evened_out_during_a_merge_or_within_a_lease_of_its_end() {
    let mut cluster = Cluster::new(&[-4, 0, 4]);
    for name in ["a", "b"] {
        cluster.register(0, name, &format!("{name}:25601"), &[]);
    }
    assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
    assert_eq!(regions_of(&cluster, "a"), [0, 2]);
    let list = stripes(4);
    cluster.listed(LEASE, &list);
    cluster.merge(LEASE + 1, 0, 1, None).unwrap();
    cluster.register(LEASE + 1, "c", "c:25601", &[]);
    beat(&mut cluster, LEASE + 1);
    // Two regions on one worker and none on another, and nothing is moved.
    assert_eq!(cluster.tick(LEASE + 2), Changes::default());
    cluster.released(LEASE + 3, "b", 1, E + 2);
    assert_eq!(cluster.tick(LEASE + 4), Changes::default());
    cluster.absorb_ended(LEASE + 5, "a", 0, 1, Ok(()));
    assert_eq!(cluster.tick(LEASE + 6), Changes::default());
    let ended = LEASE + 7;
    let list = RegionList {
        regions: vec![living(0, 0), living(2, 0), living(3, 0)],
        absorbed: vec![(RegionId(1), RegionId(0))],
        ..list
    };
    assert_eq!(cluster.listed(ended, &list).reshaped.len(), 1);
    for quarter in 0..4 {
        let now = ended + quarter * LEASE / 4;
        beat(&mut cluster, now);
        assert_eq!(cluster.tick(now), Changes::default(), "at {now}");
    }
    beat(&mut cluster, ended + LEASE);
    assert_eq!(cluster.tick(ended + LEASE), asks(&[order("a", 2, E + 3)]));
}

#[test]
fn a_leaving_workers_region_is_released_when_the_merge_it_is_part_of_has_ended() {
    let (mut cluster, list) = absorbing();
    // The survivor's owner is told to stop in the middle of it.
    assert_eq!(cluster.leaving(LEASE + 3, "a"), Changes::default());
    assert_eq!(cluster.tick(LEASE + 4), Changes::default());
    cluster.absorb_ended(LEASE + 5, "a", 0, 1, Ok(()));
    let done = cluster.listed(LEASE + 6, &after_the_merge(&list));
    let expected = Changes {
        routing: true,
        releases: vec![order("a", 0, E + 1)],
        reshaped: vec![merged(Some(7), 0, 1, Ok(0))],
        ..Changes::default()
    };
    assert_eq!(done, expected);
    // For the worker that has run nothing since it let go of the other region.
    assert_eq!(targets(&cluster), [(0, "b")]);

    // The same for a region that is split.
    let (mut cluster, _) = three_stripes();
    cluster.split(LEASE + 1, 2, &CHUNKS, None).unwrap();
    assert_eq!(cluster.leaving(LEASE + 2, "c"), Changes::default());
    let done = cluster.split_ended(LEASE + 3, "c", 2, E + 4, Ok(3));
    assert_eq!(done.releases, [order("c", 2, E + 3), order("c", 3, E + 4)]);
}

/// The world store of the randomised test below, as far as the coordinator and
/// the workers of that test can tell: the regions there are, each with the highest
/// epoch it was opened with, the regions that were absorbed, and the next id.
struct Store {
    living: BTreeMap<RegionId, u64>,
    absorbed: Vec<(RegionId, RegionId)>,
    next: u32,
}

/// Why the store does not let a worker open a region.
enum Shut {
    /// The region went into this one.
    Absorbed(RegionId),
    /// It has seen an owner with this epoch, which is higher.
    Seen(u64),
}

impl Store {
    fn list(&self) -> RegionList {
        let regions = self.living.iter();
        RegionList {
            homes: Vec::new(),
            spawn: clustine_world::BlockPos::new(0, -60, 0),
            home: RegionId(0),
            regions: regions.map(|(id, epoch)| living(id.0, *epoch)).collect(),
            absorbed: self.absorbed.clone(),
            next: RegionId(self.next),
        }
    }

    /// A worker opens `region` with `epoch`, or goes on running it.
    fn open(&mut self, region: RegionId, epoch: u64) -> Result<(), Shut> {
        if let Some((_, into)) = self.absorbed.iter().find(|(gone, _)| *gone == region) {
            return Err(Shut::Absorbed(*into));
        }
        let seen = self
            .living
            .get_mut(&region)
            .expect("no worker makes a region up");
        if *seen > epoch {
            return Err(Shut::Seen(*seen));
        }
        *seen = epoch;
        Ok(())
    }
}

/// What a worker of the randomised test was told to do and has not done yet.
#[derive(Debug)]
enum Told {
    Release(RegionId, u64),
    Reshape(Order),
}

/// What a worker of the randomised test tells the coordinator on finding the
/// store shut, or of a merge or a split.
enum Says {
    EpochRefused(RegionId, u64),
    AbsorbEnded(RegionId, RegionId, Result<(), Off>),
    SplitEnded(RegionId, u64, Result<RegionId, Off>),
}

/// The workers of the randomised test.
const CREW: [&str; 4] = ["a", "b", "c", "d"];

/// A coordinator, a world store and four workers that do what workers do
/// (ADR-0014, section 4), as far as the coordinator can tell: they open what they
/// are given at the store, release, absorb and split when told, run a region they
/// split off from memory, and say what came of it.
struct World {
    cluster: Cluster,
    store: Store,
    random: Generator,
    now: u64,
    /// Whether messages get lost and workers call things off; not while the
    /// world is left to settle.
    lossy: bool,
    /// The workers whose processes run and have a connection.
    alive: BTreeSet<&'static str>,
    /// What each worker runs: what it has open at the store.
    running: BTreeMap<&'static str, Vec<Assignment>>,
    /// The regions each worker split off that its orders have not named yet,
    /// each with the region it was split off and the epoch it was to have.
    parts: BTreeMap<&'static str, Vec<(RegionId, u64, RegionId)>>,
    inbox: BTreeMap<&'static str, Vec<Told>>,
    /// The list as it was read for the coordinator, or that it could not be,
    /// until it is handed in.
    reading: Option<Option<RegionList>>,
    /// Who waits to hear of a merge or a split.
    askers: BTreeMap<u64, Asked>,
    /// The highest epoch any coordinator issued.
    highest: u64,
    /// How many merges and splits were done, and how the others ended.
    merged: usize,
    split: usize,
    undone: BTreeMap<String, usize>,
}

impl World {
    fn new(seed: u64) -> Self {
        let store = Store {
            living: (0..3).map(|region| (RegionId(region), 0)).collect(),
            absorbed: Vec::new(),
            next: 3,
        };
        Self {
            cluster: Cluster::new(&[0, 4]),
            store,
            random: Generator(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15)),
            now: 0,
            lossy: true,
            alive: BTreeSet::new(),
            running: CREW.iter().map(|name| (*name, Vec::new())).collect(),
            parts: CREW.iter().map(|name| (*name, Vec::new())).collect(),
            inbox: CREW.iter().map(|name| (*name, Vec::new())).collect(),
            reading: None,
            askers: BTreeMap::new(),
            highest: FIRST_EPOCH,
            merged: 0,
            split: 0,
            undone: BTreeMap::new(),
        }
    }

    /// Whether something gets lost, or is called off, this once in `times`.
    fn lost(&mut self, times: u64) -> bool {
        self.lossy && self.random.once_in(times)
    }

    /// The list is read for the coordinator; now and then it cannot be.
    fn read(&mut self) -> Option<RegionList> {
        (!self.lost(10)).then(|| self.store.list())
    }

    /// Deals with what a call into the coordinator said, and with what the
    /// workers say in turn when they hear of it.
    fn said(&mut self, changes: Changes) {
        let mut says = Vec::new();
        self.take(&changes, &mut says);
        while let Some((name, say)) = says.pop() {
            let changes = self.tell(name, say);
            self.take(&changes, &mut says);
        }
    }

    /// The worker `name` tells the coordinator something.
    fn tell(&mut self, name: &'static str, say: Says) -> Changes {
        let now = self.now;
        match say {
            Says::EpochRefused(region, seen) => {
                self.cluster.epoch_refused(now, name, region.0, seen)
            }
            Says::AbsorbEnded(region, absorbed, outcome) => self
                .cluster
                .absorb_ended(now, name, region.0, absorbed.0, outcome),
            Says::SplitEnded(region, as_epoch, outcome) => {
                let outcome = outcome.map(|part| part.0);
                self.cluster
                    .split_ended(now, name, region.0, as_epoch, outcome)
            }
        }
    }

    /// The same, with what comes of it.
    fn say(&mut self, name: &'static str, say: Says) {
        let changes = self.tell(name, say);
        self.said(changes);
    }

    /// What [`World::said`] does with one call.
    fn take(&mut self, changes: &Changes, says: &mut Vec<(&'static str, Says)>) {
        for held in self.cluster.view().assignments.values().flatten() {
            self.highest = self.highest.max(held.epoch);
        }
        for told in &changes.orders {
            if let Order::Absorb { as_epoch, .. } | Order::SplitOff { as_epoch, .. } = &told.order {
                self.highest = self.highest.max(*as_epoch);
            }
        }
        for reshaped in &changes.reshaped {
            self.ended(reshaped);
        }
        for name in CREW {
            // The coordinator has closed the connection of a worker that left.
            if changes.gone.iter().any(|gone| gone == name) {
                self.stop(name);
            }
            let heard = changes.workers.iter().any(|worker| worker == name);
            if heard && self.alive.contains(name) && !self.lost(8) {
                self.hear(name, says);
            }
        }
        let releases = changes.releases.iter().map(|told| {
            let release = Told::Release(told.region, told.epoch);
            (told.worker.as_str(), release)
        });
        let reshapes = changes.orders.iter();
        let reshapes =
            reshapes.map(|told| (told.worker.as_str(), Told::Reshape(told.order.clone())));
        for (worker, told) in releases.chain(reshapes).collect::<Vec<_>>() {
            let name = CREW.into_iter().find(|name| *name == worker);
            let name = name.expect("only the crew is told anything");
            if self.alive.contains(name) && !self.lost(8) {
                self.inbox.get_mut(name).unwrap().push(told);
            }
        }
        if changes.read {
            // A reading that was under way was asked for too early.
            self.reading = Some(self.read());
        }
    }

    /// Whoever asked for a merge or a split is told once, what was asked, and
    /// nothing is said to be done that the store does not have.
    fn ended(&mut self, reshaped: &Reshaped) {
        if let Some(asker) = reshaped.asker {
            let asked = self.askers.remove(&asker);
            assert_eq!(asked, Some(reshaped.asked), "{reshaped:?}");
        }
        match (reshaped.asked, reshaped.outcome) {
            (Asked::Merge { survivor, absorbed }, Ok(region)) => {
                assert_eq!(region, survivor);
                let pair = (absorbed, survivor);
                assert!(self.store.absorbed.contains(&pair), "{reshaped:?}");
                self.merged += 1;
            }
            (Asked::Split { .. }, Ok(part)) => {
                let mut gone = self.store.absorbed.iter().map(|(gone, _)| gone);
                let made = self.store.living.contains_key(&part) || gone.any(|gone| *gone == part);
                assert!(made, "{reshaped:?}");
                self.split += 1;
            }
            (asked, Err(why)) => {
                // A merge that ended before anybody was told to absorb was not
                // done.
                if let (Asked::Merge { survivor, absorbed }, Undone::NotReleased) = (asked, why) {
                    assert!(!self.store.absorbed.contains(&(absorbed, survivor)));
                }
                let kind = format!("{why:?}");
                let kind = kind.split('(').next().unwrap_or_default().to_owned();
                *self.undone.entry(kind).or_default() += 1;
            }
        }
    }

    /// The worker `name` hears what it is to run: it opens what is new to it,
    /// lets go of what it is no longer to run, and goes on running a region it
    /// split off until its orders have named that once.
    fn hear(&mut self, name: &'static str, says: &mut Vec<(&'static str, Says)>) {
        let ordered = self.cluster.assignments(name);
        let named = |part: RegionId| ordered.iter().any(|held| held.region == part);
        let parts = self.parts.get_mut(name).unwrap();
        parts.retain(|(_, _, made)| !named(*made));
        let mut runs: Vec<Assignment> = parts
            .iter()
            .map(|(_, as_epoch, made)| part(made.0, *as_epoch))
            .collect();
        for held in ordered {
            match self.store.open(held.region, held.epoch) {
                Ok(()) => runs.push(held),
                Err(Shut::Absorbed(into)) => {
                    says.push((name, Says::AbsorbEnded(into, held.region, Ok(()))));
                }
                Err(Shut::Seen(seen)) => {
                    says.push((name, Says::EpochRefused(held.region, seen)));
                }
            }
        }
        self.running.insert(name, runs);
    }

    /// The process of the worker `name` is gone, with what it had in memory.
    fn stop(&mut self, name: &'static str) {
        self.alive.remove(name);
        self.running.get_mut(name).unwrap().clear();
        self.parts.get_mut(name).unwrap().clear();
        self.inbox.get_mut(name).unwrap().clear();
    }

    /// The worker `name` registers, reporting what it runs, hears its orders in
    /// answer, and says again of every region it split off and has not been
    /// given yet that it did. The service has the list read.
    fn register(&mut self, name: &'static str) {
        let holding = self.running[name].clone();
        let address = format!("{name}:25601");
        let changes = self.cluster.register(self.now, name, &address, &holding);
        self.alive.insert(name);
        self.said(changes);
        let mut says = Vec::new();
        self.hear(name, &mut says);
        for (region, as_epoch, made) in self.parts[name].clone() {
            says.push((name, Says::SplitEnded(region, as_epoch, Ok(made))));
        }
        for (name, say) in says {
            self.say(name, say);
        }
        self.reading = Some(self.read());
    }

    /// The worker `name` finds out which of its regions the store has shut on it,
    /// as a runner does with its next commit, and lets go of those; it vouches
    /// for the others, and registers again if the coordinator does not know it.
    fn beat(&mut self, name: &'static str) {
        let mut says = Vec::new();
        for held in self.running[name].clone() {
            let say = match self.store.open(held.region, held.epoch) {
                Ok(()) => continue,
                Err(Shut::Absorbed(into)) => Says::AbsorbEnded(into, held.region, Ok(())),
                Err(Shut::Seen(seen)) => Says::EpochRefused(held.region, seen),
            };
            says.push(say);
            self.running
                .get_mut(name)
                .unwrap()
                .retain(|run| *run != held);
            let parts = self.parts.get_mut(name).unwrap();
            parts.retain(|(_, _, made)| *made != held.region);
        }
        for say in says {
            self.say(name, say);
        }
        let runs = self.running[name].iter();
        let vouches: Vec<(RegionId, Vouch)> =
            runs.map(|held| (held.region, Vouch::Committed)).collect();
        if !self.cluster.heartbeat_with(self.now, name, &vouches) {
            self.register(name);
        }
    }

    /// Whether the worker `name` runs `region` with `epoch` and the store still
    /// takes its word for it.
    fn runs(&self, name: &str, region: RegionId, epoch: u64) -> bool {
        let held = |held: &Assignment| (held.region, held.epoch) == (region, epoch);
        self.running[name].iter().any(held) && self.store.living.get(&region) == Some(&epoch)
    }

    /// The worker `name` does the first thing it was told and has not done.
    fn act(&mut self, name: &'static str) {
        let inbox = self.inbox.get_mut(name).unwrap();
        if inbox.is_empty() {
            return;
        }
        let said = match inbox.remove(0) {
            Told::Release(region, epoch) => {
                let held = |held: &Assignment| (held.region, held.epoch) == (region, epoch);
                self.running.get_mut(name).unwrap().retain(|run| !held(run));
                // Now and then the word that it has let go is lost.
                if self.lost(8) {
                    return;
                }
                self.cluster.released(self.now, name, region.0, epoch)
            }
            Told::Reshape(Order::Prepare { .. }) => return,
            Told::Reshape(Order::Absorb {
                region,
                epoch,
                absorbed,
                as_epoch,
            }) => {
                let outcome = self.absorb(name, region, epoch, absorbed, as_epoch);
                if self.lost(6) {
                    return;
                }
                let now = self.now;
                self.cluster
                    .absorb_ended(now, name, region.0, absorbed.0, outcome)
            }
            Told::Reshape(Order::SplitOff {
                region,
                epoch,
                as_epoch,
                ..
            }) => {
                let outcome = self.split_off(name, region, epoch, as_epoch);
                if self.lost(6) {
                    return;
                }
                let outcome = outcome.map(|part| part.0);
                self.cluster
                    .split_ended(self.now, name, region.0, as_epoch, outcome)
            }
        };
        self.said(said);
    }

    /// The worker `name` has `region` absorb `absorbed`, as far as the store lets
    /// it.
    fn absorb(
        &mut self,
        name: &'static str,
        region: RegionId,
        epoch: u64,
        absorbed: RegionId,
        as_epoch: u64,
    ) -> Result<(), Off> {
        if !self.runs(name, region, epoch) {
            return Err(Off::NotRunning);
        }
        match self.store.open(absorbed, as_epoch) {
            // The order came twice.
            Err(Shut::Absorbed(into)) if into == region => return Ok(()),
            Err(Shut::Absorbed(_)) => return Err(Off::Unreadable),
            Err(Shut::Seen(seen)) => {
                self.say(name, Says::EpochRefused(absorbed, seen));
                return Err(Off::Refused);
            }
            Ok(()) => {}
        }
        if self.lost(5) {
            return Err(Off::Busy);
        }
        self.store.living.remove(&absorbed);
        self.store.absorbed.push((absorbed, region));
        Ok(())
    }

    /// The worker `name` splits `region`, as far as the store lets it, and runs
    /// the new region from memory. The store names it, whatever the order said.
    fn split_off(
        &mut self,
        name: &'static str,
        region: RegionId,
        epoch: u64,
        as_epoch: u64,
    ) -> Result<RegionId, Off> {
        if !self.runs(name, region, epoch) {
            return Err(Off::NotRunning);
        }
        // While the world settles, nothing new is made.
        if !self.lossy || self.random.once_in(3) {
            return Err(Off::Nobody);
        }
        let made = RegionId(self.store.next);
        self.store.next += 1;
        self.store.living.insert(made, as_epoch);
        self.running
            .get_mut(name)
            .unwrap()
            .push(part(made.0, as_epoch));
        self.parts
            .get_mut(name)
            .unwrap()
            .push((region, as_epoch, made));
        Ok(made)
    }

    /// The reading that is under way comes back, if one is.
    fn hand_in(&mut self) {
        let Some(reading) = self.reading.take() else {
            return;
        };
        let changes = match reading {
            Some(list) => self.cluster.listed(self.now, &list),
            None => self.cluster.unlisted(self.now),
        };
        self.said(changes);
    }

    /// Somebody asks for two regions to be merged or for one to be split: mostly
    /// regions the coordinator knows. The service reads the list first.
    fn ask(&mut self, asker: u64) {
        self.reading = Some(self.read());
        let read = self.reading.as_ref().is_some_and(Option::is_some);
        self.hand_in();
        if !read {
            return;
        }
        let known: Vec<u32> = self
            .cluster
            .coordinator
            .regions
            .keys()
            .map(|id| id.0)
            .collect();
        let mut pick = || match self.random.below(8) {
            0 => self.random.below(u64::from(self.store.next) + 1) as u32,
            _ => known[self.random.below(known.len() as u64) as usize],
        };
        let (one, other) = (pick(), pick());
        let (begun, asked) = if self.random.once_in(2) {
            let asked = Asked::Merge {
                survivor: RegionId(one),
                absorbed: RegionId(other),
            };
            (self.cluster.merge(self.now, one, other, Some(asker)), asked)
        } else {
            let asked = Asked::Split {
                region: RegionId(one),
            };
            let begun = self.cluster.split(self.now, one, &CHUNKS, Some(asker));
            (begun, asked)
        };
        match begun {
            Ok(changes) => {
                self.askers.insert(asker, asked);
                self.said(changes);
            }
            Err(_) => *self.undone.entry("refused".to_owned()).or_default() += 1,
        }
    }

    /// One thing happens, at random.
    fn step(&mut self, step: u64) {
        self.now += self.random.below(LEASE / 40);
        let name = CREW[self.random.below(4) as usize];
        let alive = self.alive.contains(name);
        match self.random.below(100) {
            0..30 if alive => self.beat(name),
            30..45 => {
                let changes = self.cluster.tick(self.now);
                self.said(changes);
            }
            45..65 if alive => self.act(name),
            65..77 => self.hand_in(),
            77..87 => self.ask(step),
            87..90 if alive => self.register(name),
            // The process dies, and its connection ends with it.
            90..92 if alive => {
                self.stop(name);
                let changes = self.cluster.disconnected(self.now, name);
                self.said(changes);
            }
            92..97 if !alive => self.register(name),
            97 if alive => {
                let changes = self.cluster.leaving(self.now, name);
                self.said(changes);
            }
            // The coordinator is replaced by one that knows nothing. Nobody
            // hears of what was asked of the old one; the new one reads the list.
            98 if self.random.once_in(4) => {
                self.cluster.restart(self.now, self.highest);
                self.askers.clear();
                *self.undone.entry("restarts".to_owned()).or_default() += 1;
                self.reading = Some(self.read());
            }
            _ => {}
        }
    }

    /// Every worker is there and does what it is told, nothing gets lost, and
    /// nobody asks for anything, for eight leases: after that the coordinator has
    /// the regions the store has, each with an owner that runs it, and nothing
    /// is under way.
    fn settle(&mut self) {
        self.lossy = false;
        for name in CREW {
            self.register(name);
        }
        for _ in 0..64 {
            self.now += LEASE / 8;
            for name in CREW {
                self.beat(name);
                while !self.inbox[name].is_empty() {
                    self.act(name);
                }
                let mut says = Vec::new();
                self.hear(name, &mut says);
                assert!(
                    says.is_empty(),
                    "a worker heard what it had not heard before"
                );
            }
            self.hand_in();
            let changes = self.cluster.tick(self.now);
            self.said(changes);
        }
        self.hand_in();

        let coordinator = &self.cluster.coordinator;
        assert!(coordinator.merges.is_empty(), "{:?}", coordinator.merges);
        assert!(coordinator.splits.is_empty(), "{:?}", coordinator.splits);
        assert!(!coordinator.owed && !coordinator.reading);
        assert!(self.askers.is_empty(), "{:?}", self.askers);
        let known: Vec<&RegionId> = coordinator.regions.keys().collect();
        let there: Vec<&RegionId> = self.store.living.keys().collect();
        assert_eq!(known, there);
        let table = self.cluster.table();
        assert!(table.is_complete(), "{table:?}");
        assert_eq!(table.absorbed, self.store.absorbed);
        for name in CREW {
            let held = |held: &Assignment| (held.region, held.epoch);
            let mut runs: Vec<_> = self.running[name].iter().map(held).collect();
            runs.sort_unstable();
            let owns: Vec<_> = self.cluster.assignments(name).iter().map(held).collect();
            assert_eq!(runs, owns, "{name}");
            for (region, epoch) in owns {
                assert_eq!(self.store.living[&region], epoch, "{name}");
            }
        }
    }
}

/// Regions are merged and split at random while workers come and go, messages
/// get lost, the list cannot always be read and the coordinator is replaced now
/// and then. [`Cluster`] checks every call. This adds that whoever asked hears
/// of it once, that nothing is said to be done that the store does not have, and
/// that the coordinator ends up with the regions the store has, whatever
/// happened on the way.
#[test]
fn regions_merge_and_split_whatever_gets_lost_and_end_up_as_the_store_has_them() {
    let (mut merged, mut split) = (0, 0);
    let mut undone: BTreeMap<String, usize> = BTreeMap::new();
    for seed in 1..=40_u64 {
        let mut world = World::new(seed);
        for step in 0..3000 {
            world.step(step);
        }
        world.settle();
        merged += world.merged;
        split += world.split;
        for (kind, count) in world.undone {
            *undone.entry(kind).or_default() += count;
        }
    }
    // All of it has in fact happened, and often.
    assert!(merged >= 100 && split >= 100, "{merged} {split} {undone:?}");
    for kind in [
        "Off",
        "NotReleased",
        "Overdue",
        "Disowned",
        "Unread",
        "refused",
        "restarts",
    ] {
        let count = undone.get(kind).copied().unwrap_or(0);
        assert!(count >= 20, "{kind}: {merged} {split} {undone:?}");
    }
}
