//! Tests of how a coordinator begins: one that knows no region until it has been
//! handed the world store's list, and one that was made knowing its regions. The
//! rules are those of `docs/adr/0017-the-end-of-the-stripes.md`, section 2.3, and the
//! tests name its orders of events (N12, N13) where they are one.
//!
//! Every other test of the state machine is of a coordinator made
//! [`Coordinator::knowing`] its regions, which is how [`Cluster::new`] makes one.

use super::*;

/// A cluster whose coordinator `make` made at the cluster's start, for a world that
/// was never divided into stripes.
fn cluster(make: fn(CoordinatorConfig, Instant, u64) -> Coordinator) -> Cluster {
    let layout = Layout::single();
    let start = Instant::now();
    Cluster {
        coordinator: make(config(&layout), start, FIRST_EPOCH),
        layout,
        start,
        addresses: BTreeMap::new(),
        last: None,
    }
}

/// The list of a world with these living regions, each with the epoch it was last
/// opened with, of which region 0 is the home region.
fn listing(regions: &[(u32, u64)], absorbed: &[(u32, u32)], next: u32) -> RegionList {
    RegionList {
        home: RegionId(0),
        regions: regions
            .iter()
            .map(|(region, epoch)| living(*region, *epoch))
            .collect(),
        absorbed: absorbed
            .iter()
            .map(|(gone, into)| (RegionId(*gone), RegionId(*into)))
            .collect(),
        next: RegionId(next),
    }
}

#[test]
fn a_new_coordinator_knows_no_region_and_gives_a_worker_nothing() {
    let mut cluster = cluster(Coordinator::new);
    assert!(cluster.coordinator.awaits_the_list());
    assert_eq!(cluster.coordinator.home(), None);
    let table = cluster.table();
    assert_eq!(
        (table.routes.len(), table.waiting, table.home),
        (0, 0, None)
    );
    assert_eq!(table.version, FIRST_EPOCH);

    // Nobody is told how the world is divided, so there is nothing to give away:
    // not at once, and not when the grace period is over.
    assert_eq!(cluster.register(1, "a", "a:25601", &[]), Changes::default());
    assert_eq!(cluster.assignments("a"), []);
    for at in [2, LEASE, LEASE + 1, 3 * LEASE] {
        cluster.heartbeat(at, "a");
        assert_eq!(cluster.tick(at), Changes::default(), "{at}");
    }
    assert_eq!(cluster.coordinator.waiting(), []);
    assert_eq!(cluster.table(), table);

    // Nor is anything asked of a region it does not know.
    let chunks = [ChunkPos::new(0, 0)];
    assert_eq!(
        cluster.merge(3 * LEASE, 0, 1, None),
        Err(ReshapeRefusal::NoSuchRegion(RegionId(0)))
    );
    assert_eq!(
        cluster.split(3 * LEASE, 0, &chunks, None),
        Err(ReshapeRefusal::NoSuchRegion(RegionId(0)))
    );
    assert_eq!(
        cluster.move_region(3 * LEASE, 0, None, 7).unwrap_err(),
        MoveRefusal::NoSuchRegion(RegionId(0))
    );
    assert!(cluster.coordinator.awaits_the_list());
}

#[test]
fn the_first_list_brings_the_home_region_which_waits_out_the_grace_period() {
    let mut cluster = cluster(Coordinator::new);
    cluster.register(1, "a", "a:25601", &[]);

    // The region is known from the call that hands the list in, and counted as
    // waiting for an owner; it is given to nobody within the grace period.
    assert_eq!(cluster.listed(2, &stripes(1)), changes(&[], true));
    assert!(!cluster.coordinator.awaits_the_list());
    assert_eq!(cluster.coordinator.home(), Some(RegionId(0)));
    assert_eq!(cluster.coordinator.waiting(), [RegionId(0)]);
    let table = cluster.table();
    assert_eq!(
        (table.routes.len(), table.waiting, table.home),
        (0, 1, Some(RegionId(0)))
    );
    // Nor by another list or a reading that fails.
    assert_eq!(cluster.listed(3, &stripes(1)), Changes::default());
    assert_eq!(cluster.unlisted(4), Changes::default());
    cluster.heartbeat(LEASE - 1, "a");
    assert_eq!(cluster.tick(LEASE - 1), Changes::default());

    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    assert_eq!(
        cluster.assignments("a"),
        [assignment(0, FIRST_EPOCH + 1, 0)]
    );
    assert!(cluster.table().is_complete());
}

#[test]
fn a_list_that_comes_when_the_grace_period_is_over_has_its_regions_assigned_by_that_call() {
    let mut cluster = cluster(Coordinator::new);
    cluster.register(1, "a", "a:25601", &[]);
    cluster.heartbeat(LEASE, "a");
    assert_eq!(cluster.tick(LEASE), Changes::default());
    assert_eq!(cluster.listed(LEASE, &stripes(2)), changes(&["a"], true));
    assert_eq!(
        cluster.assignments("a"),
        [
            assignment(0, FIRST_EPOCH + 1, 0),
            assignment(1, FIRST_EPOCH + 2, 1)
        ]
    );
}

#[test]
fn a_region_a_worker_reports_before_any_list_is_its_until_a_list_says_otherwise() {
    let held = assignment(3, 40, 2);
    // What the list has of region 3, and whether the worker goes on running it.
    let lists = [
        (listing(&[(0, 0), (3, 40)], &[], 4), true),
        (listing(&[(0, 0)], &[(3, 0)], 4), false),
        // The store has had a region 3 and has it no longer.
        (listing(&[(0, 0)], &[], 9), false),
        // A reading that is older than the region says nothing of it.
        (listing(&[(0, 0)], &[], 3), true),
    ];
    for (list, stays) in lists {
        let mut cluster = cluster(Coordinator::new);
        // It owns the region on its word, as a part's worker does.
        assert_eq!(
            cluster.register(1, "a", "a:25601", &[held]),
            changes(&["a"], true)
        );
        assert_eq!(cluster.assignments("a"), [held]);
        assert!(cluster.coordinator.awaits_the_list());

        cluster.listed(2, &list);
        let runs = if stays { vec![held] } else { Vec::new() };
        assert_eq!(cluster.assignments("a"), runs, "{list:?}");
        assert!(!cluster.coordinator.awaits_the_list());
    }
}

// N13.
#[test]
fn a_region_nobody_reports_is_known_only_when_the_list_names_it() {
    let mut cluster = cluster(Coordinator::new);
    let held = assignment(0, 40, 0);
    cluster.register(1, "a", "a:25601", &[held]);
    cluster.register(2, "b", "b:25601", &[]);

    // The store is away for longer than the grace period. Region 1, whose worker
    // died while there was no coordinator, is in nobody's report: nothing names it.
    for step in 1..=8 {
        let at = step * LEASE / 4;
        cluster.heartbeat(at, "a");
        cluster.heartbeat(at, "b");
        assert_eq!(cluster.tick(at), Changes::default());
        assert_eq!(cluster.unlisted(at), Changes::default());
        assert!(cluster.coordinator.awaits_the_list());
    }
    assert_eq!(cluster.coordinator.waiting(), []);
    assert_eq!(cluster.assignments("b"), []);

    // The store answers. The grace period is over, so the region is given away by
    // the call that names it.
    let list = listing(&[(0, 40), (1, 33)], &[], 2);
    assert_eq!(cluster.listed(2 * LEASE, &list), changes(&["b"], true));
    assert_eq!(cluster.assignments("a"), [held]);
    assert_eq!(
        cluster.assignments("b"),
        [assignment(1, FIRST_EPOCH + 1, 1)]
    );
}

#[test]
fn a_reading_that_fails_leaves_a_new_coordinator_waiting_for_its_first_list() {
    let mut cluster = cluster(Coordinator::new);
    for at in [1, LEASE, 5 * LEASE] {
        assert_eq!(cluster.unlisted(at), Changes::default());
        assert!(cluster.coordinator.awaits_the_list());
        assert_eq!(cluster.coordinator.home(), None);
    }
    cluster.listed(5 * LEASE, &stripes(1));
    assert!(!cluster.coordinator.awaits_the_list());
    // It has had its first list for good, whatever becomes of later readings.
    cluster.unlisted(5 * LEASE + 1);
    assert!(!cluster.coordinator.awaits_the_list());
    assert_eq!(cluster.coordinator.home(), Some(RegionId(0)));
}

#[test]
fn a_coordinator_made_knowing_its_regions_has_them_without_owners_and_awaits_no_list() {
    let start = Instant::now();
    let layout = Layout::single();
    let known = [RegionId(0), RegionId(1), RegionId(6)];
    let coordinator = Coordinator::knowing(config(&layout), start, FIRST_EPOCH, &known);
    assert!(!coordinator.awaits_the_list());
    assert_eq!(coordinator.home(), None);
    assert_eq!(coordinator.waiting(), known);
    let table = coordinator.routing_table();
    assert_eq!(
        (table.routes.len(), table.waiting, table.home),
        (0, 3, None)
    );

    // Also when it is made knowing none: it is not what `new` makes.
    let coordinator = Coordinator::knowing(config(&layout), start, FIRST_EPOCH, &[]);
    assert!(!coordinator.awaits_the_list());
    assert_eq!(coordinator.waiting(), []);
}

#[test]
fn a_coordinator_made_knowing_its_regions_waits_out_the_grace_period_as_a_new_one_does() {
    let mut cluster = Cluster::new(&[0]);
    cluster.register(1, "a", "a:25601", &[]);
    cluster.heartbeat(LEASE - 1, "a");
    assert_eq!(cluster.tick(LEASE - 1), Changes::default());
    assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    assert_eq!(cluster.coordinator.waiting(), []);
}
