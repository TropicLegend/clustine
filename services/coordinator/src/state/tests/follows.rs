//! Tests of a coordinator that merges and splits regions by itself: the rules of
//! `docs/adr/0016-when-to-merge-and-split.md`, whose sections and scenarios (K1 to
//! K26) the tests name.

use super::*;

/// How long a region is left alone in these tests, in milliseconds.
const REST: u64 = 10_000;

/// What the coordinators of these tests go by: regions are merged when their players
/// are 2 chunks apart or nearer and split when they are more than 5 apart, so the
/// margin is 2, and a region rests for ten seconds.
fn small() -> Policy {
    Policy {
        merge_distance: 2,
        split_distance: 5,
        rest: Duration::from_millis(REST),
    }
}

/// A new coordinator that merges and splits by itself, for a world with region
/// boundaries at these chunk x coordinates.
fn following(boundaries: &[i32]) -> Cluster {
    let layout = Layout::new(boundaries.to_vec()).unwrap();
    let start = Instant::now();
    let config = CoordinatorConfig {
        layout: layout.clone(),
        spawn: SPAWN,
        lease: Duration::from_millis(LEASE),
        follow: Some(small()),
    };
    Cluster {
        coordinator: Coordinator::new(config, start, FIRST_EPOCH),
        layout,
        start,
        addresses: BTreeMap::new(),
        last: None,
    }
}

#[test]
fn the_list_is_asked_for_every_lease_and_not_again_before_it_is_answered() {
    let mut cluster = following(&[0]);
    // There has been no answer yet, so the first tick asks.
    assert_eq!(cluster.tick(0), reads());
    // Not again while that reading is asked for, however long it takes.
    for now in [250, LEASE, 3 * LEASE] {
        assert_eq!(cluster.tick(now), Changes::default(), "at {now}");
    }
    // A lease from the answer, and not a moment before.
    cluster.listed(3 * LEASE + 100, &stripes(2));
    assert_eq!(cluster.tick(4 * LEASE + 99), Changes::default());
    assert_eq!(cluster.tick(4 * LEASE + 100), reads());
    assert_eq!(cluster.tick(4 * LEASE + 350), Changes::default());
}

#[test]
fn a_reading_that_fails_is_an_answer_and_the_next_is_asked_for_a_lease_after_it() {
    let mut cluster = following(&[0]);
    assert_eq!(cluster.tick(0), reads());
    cluster.listed(0, &stripes(2));
    assert_eq!(cluster.tick(LEASE), reads());
    // The store is away, and says so a second later. Asking again at once would ask
    // at every tick of a store that is away.
    cluster.unlisted(LEASE + 1000);
    for now in [LEASE + 1000, LEASE + 1250, 2 * LEASE + 750] {
        assert_eq!(cluster.tick(now), Changes::default(), "at {now}");
    }
    assert_eq!(cluster.tick(2 * LEASE + 1000), reads());
}

#[test]
fn a_reading_that_something_else_asked_for_puts_the_next_one_off_by_a_lease() {
    let mut cluster = following(&[0]);
    assert_eq!(cluster.tick(0), reads());
    cluster.listed(0, &stripes(2));
    // The service reads the list by itself as well, at every registration.
    cluster.listed(LEASE - 250, &stripes(2));
    assert_eq!(cluster.tick(LEASE), Changes::default());
    assert_eq!(cluster.tick(2 * LEASE - 500), Changes::default());
    assert_eq!(cluster.tick(2 * LEASE - 250), reads());
}

#[test]
fn a_coordinator_that_decides_nothing_by_itself_never_asks_for_the_list_by_the_time() {
    let mut cluster = Cluster::new(&[0]);
    for now in [0, LEASE, 3 * LEASE] {
        assert_eq!(cluster.tick(now), Changes::default(), "at {now}");
    }
    cluster.listed(3 * LEASE, &stripes(2));
    cluster.unlisted(4 * LEASE);
    for now in [4 * LEASE, 5 * LEASE, 9 * LEASE] {
        assert_eq!(cluster.tick(now), Changes::default(), "at {now}");
    }
}
