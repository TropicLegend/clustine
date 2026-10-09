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

/// A coordinator `make` made that decides by itself, by the distances 2 and 5 with a
/// rest of `rest` milliseconds, with the worker `a`, which registered at 0 running
/// regions 0 and 1 already. The list, which is handed in at 0 and whenever the
/// coordinator asks for it, has the two.
struct Lone {
    cluster: Cluster,
    list: RegionList,
    /// The time of the last look, in milliseconds, and the tick the regions are at.
    now: u64,
    tick: u64,
}

impl Lone {
    /// How far apart the looks are, in milliseconds.
    const LOOK: u64 = 250;

    fn begin(make: fn(CoordinatorConfig, Instant, u64) -> Coordinator, rest: u64) -> Self {
        let layout = Layout::single();
        let start = Instant::now();
        let config = CoordinatorConfig {
            follow: Some(Policy {
                merge_distance: 2,
                split_distance: 5,
                rest: Duration::from_millis(rest),
            }),
            ..config(&layout)
        };
        let mut cluster = Cluster {
            coordinator: make(config, start, FIRST_EPOCH),
            layout,
            start,
            addresses: BTreeMap::new(),
            last: None,
        };
        let held = [assignment(0, 5, 0), assignment(1, 6, 1)];
        cluster.register(0, "a", "a:25601", &held);
        let list = listing(&[(0, 5), (1, 6)], &[], 2);
        cluster.listed(0, &list);
        Self {
            cluster,
            list,
            now: 0,
            tick: 0,
        }
    }

    /// A quarter of a second passes and the coordinator ticks; before that, unless
    /// the worker is `silent`, it vouches for its regions and says that the players
    /// of the two stand in chunks side by side. Returns what the tick said.
    fn look(&mut self, silent: bool) -> Changes {
        self.now += Self::LOOK;
        self.tick += 1;
        if !silent {
            assert!(self.cluster.heartbeat(self.now, "a"));
            let reports: Vec<PlayersOf> = self
                .cluster
                .assignments("a")
                .into_iter()
                .map(|held| {
                    let chunk = i32::try_from(held.region.0).unwrap();
                    players_of(held, self.tick, &[((chunk, 0), 1)])
                })
                .collect();
            assert!(self.cluster.players(self.now, "a", &reports));
        }
        let said = self.cluster.tick(self.now);
        if said.read {
            let list = self.list.clone();
            self.cluster.listed(self.now, &list);
        }
        said
    }

    /// Looks until the merge of region 1 into region 0 is begun, which has to be by
    /// `limit`, and returns when that was.
    fn until_merging(&mut self, limit: u64) -> u64 {
        while self.now < limit {
            self.look(false);
            if !self.cluster.coordinator.under_way().is_empty() {
                let merge = Asked::Merge {
                    survivor: RegionId(0),
                    absorbed: RegionId(1),
                };
                assert_eq!(self.cluster.coordinator.under_way(), [merge]);
                return self.now;
            }
        }
        panic!("no merge was begun until {limit}");
    }
}

// Q3.
#[test]
fn a_coordinator_that_is_alone_gives_the_regions_of_a_list_away_without_a_grace_period() {
    // A worker is there when the list is handed in: by that call.
    let mut cluster = cluster(Coordinator::alone);
    assert!(cluster.coordinator.awaits_the_list());
    assert!(cluster.coordinator.keeps_its_workers());
    assert_eq!(cluster.register(1, "a", "a:25601", &[]), Changes::default());
    assert_eq!(cluster.listed(2, &stripes(1)), changes(&["a"], true));
    assert_eq!(
        cluster.assignments("a"),
        [assignment(0, FIRST_EPOCH + 1, 0)]
    );

    // No worker is there: by the first tick or list after one has registered. To
    // register gives nothing away by itself, as with any coordinator.
    for by_a_list in [false, true] {
        let mut cluster = self::cluster(Coordinator::alone);
        assert_eq!(cluster.listed(1, &stripes(1)), changes(&[], true));
        assert_eq!(cluster.tick(2), Changes::default());
        assert_eq!(cluster.register(3, "a", "a:25601", &[]), Changes::default());
        let given = if by_a_list {
            cluster.listed(4, &stripes(1))
        } else {
            cluster.tick(4)
        };
        assert_eq!(given, changes(&["a"], true));
        assert_eq!(
            cluster.assignments("a"),
            [assignment(0, FIRST_EPOCH + 1, 0)]
        );
    }

    // Neither of the others is like that.
    assert!(
        !self::cluster(Coordinator::new)
            .coordinator
            .keeps_its_workers()
    );
    assert!(!Cluster::new(&[]).coordinator.keeps_its_workers());
}

#[test]
fn a_coordinator_that_is_alone_evens_out_without_a_grace_period() {
    let mut cluster = cluster(Coordinator::alone);
    let held = [assignment(0, 5, 0), assignment(1, 6, 1)];
    cluster.register(1, "a", "a:25601", &held);
    cluster.register(2, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(3), asks(&[order("a", 1, 6)]));

    // One made with `new` waits for a lease with that.
    let mut cluster = self::cluster(Coordinator::new);
    cluster.register(1, "a", "a:25601", &held);
    cluster.register(2, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(3), Changes::default());
    for name in ["a", "b"] {
        cluster.heartbeat(LEASE, name);
    }
    assert_eq!(cluster.tick(LEASE), asks(&[order("a", 1, 6)]));
}

#[test]
fn a_coordinator_that_is_alone_begins_a_merge_by_itself_without_a_grace_period() {
    // The regions rest for a second from when they were reported, and the merge has
    // to have been wanted for longer than a second: nothing else holds it back.
    let mut lone = Lone::begin(Coordinator::alone, 1_000);
    let begun = lone.until_merging(LEASE);
    assert!(begun <= 1_000 + 2 * Lone::LOOK, "{begun}");

    // One made with `new` begins nothing before its grace period is over.
    let mut lone = Lone::begin(Coordinator::new, 1_000);
    assert_eq!(lone.until_merging(2 * LEASE), LEASE);
}

// Q13.
#[test]
fn a_coordinator_that_is_alone_does_not_forget_a_worker_that_is_silent() {
    for alone in [true, false] {
        let make = if alone {
            Coordinator::alone
        } else {
            Coordinator::new
        };
        let mut cluster = cluster(make);
        let held = assignment(0, 5, 0);
        cluster.register(1, "a", "a:25601", &[held]);

        // Nothing at all is heard of the worker for ten leases.
        let said = cluster.tick(10 * LEASE + 1);
        if alone {
            assert_eq!(said, Changes::default());
            assert_eq!(cluster.assignments("a"), [held]);
            assert!(cluster.heartbeat(10 * LEASE + 1, "a"));
            assert_eq!(cluster.coordinator.workers["a"].failed, None);
            assert_eq!(cluster.coordinator.workers["a"].arrival, 0);
        } else {
            assert_eq!(said, changes(&["a"], true));
            assert_eq!(cluster.assignments("a"), []);
            assert!(!cluster.heartbeat(10 * LEASE + 1, "a"));
        }
    }
}

// Q13.
#[test]
fn a_coordinator_that_is_alone_takes_no_region_that_is_not_vouched_for() {
    for alone in [true, false] {
        let make = if alone {
            Coordinator::alone
        } else {
            Coordinator::new
        };
        let mut cluster = cluster(make);
        let held = assignment(0, 5, 0);
        cluster.register(1, "a", "a:25601", &[held]);

        // The worker is heard for ten leases, and names no region. Each tick could
        // take the region, and a tick that gives it back hides that it was taken.
        let mut taken = Vec::new();
        for step in 1..=40 {
            let at = step * LEASE / 4;
            assert!(cluster.heartbeat_with(at, "a", &[]));
            let said = cluster.tick(at);
            if said != Changes::default() {
                taken.push(at);
            }
        }
        let worker = &cluster.coordinator.workers["a"];
        if alone {
            assert_eq!(taken, [] as [u64; 0]);
            assert_eq!(cluster.assignments("a"), [held]);
            assert_eq!((worker.failed, worker.arrival), (None, 0));
        } else {
            // A lease and a moment after it registered with the region, and again
            // whenever it has had the region for longer than a lease since.
            assert_eq!(taken[0], LEASE + LEASE / 4);
            assert!(taken.len() > 1, "{taken:?}");
            assert_ne!(cluster.assignments("a"), [held]);
            assert!(worker.failed.is_some());
        }
    }
}

#[test]
fn a_coordinator_that_is_alone_keeps_a_region_whose_owner_waits_for_the_store_for_too_long() {
    let mut cluster = cluster(Coordinator::alone);
    let held = assignment(0, 5, 0);
    cluster.register(1, "a", "a:25601", &[held]);
    let patience = u64::try_from(Coordinator::STORE_PATIENCE.as_millis()).unwrap();
    let waiting = [(RegionId(0), Vouch::WaitingForStore)];
    let mut at = 1;
    while at < 2 * patience {
        at += LEASE / 4;
        assert!(cluster.heartbeat_with(at, "a", &waiting));
        assert_eq!(cluster.tick(at), Changes::default(), "{at}");
    }
    assert_eq!(cluster.assignments("a"), [held]);
}

/// A worker that says it leaves is the one way left in which such a coordinator
/// forgets a worker, and what that worker ran must not stay with nobody.
#[test]
fn a_worker_of_a_coordinator_that_is_alone_that_leaves_and_is_gone_loses_its_region() {
    let mut cluster = cluster(Coordinator::alone);
    let held = assignment(0, 5, 0);
    cluster.register(1, "a", "a:25601", &[held]);
    // Nobody is there to release it for, so it keeps the region while it is there.
    assert_eq!(cluster.leaving(2, "a"), Changes::default());
    assert_eq!(cluster.assignments("a"), [held]);

    assert_eq!(cluster.disconnected(3, "a"), changes(&["a"], true));
    assert_eq!(cluster.assignments("a"), []);
    assert_eq!(cluster.coordinator.waiting(), [RegionId(0)]);
    assert!(!cluster.heartbeat(4, "a"));

    // Whoever comes is given it at the next tick.
    cluster.register(5, "b", "b:25601", &[]);
    assert_eq!(cluster.tick(6), changes(&["b"], true));
    assert_eq!(
        cluster.assignments("b"),
        [assignment(0, FIRST_EPOCH + 1, 1)]
    );
}

/// What became of the merge of region 1 into region 0 that `lone` began at `asked`
/// and whose release the worker never answers: when it ended, and when the merge
/// was begun again. With `silent` nothing at all is heard of the worker for ten
/// leases from when it was asked, and then a tick comes.
fn a_merge_that_is_not_released(mut lone: Lone, asked: u64, silent: bool) -> (u64, u64) {
    let held = lone.cluster.assignments("a");
    let ended = loop {
        let said = if silent {
            lone.now = asked + 10 * LEASE - Lone::LOOK;
            lone.look(true)
        } else {
            lone.look(false)
        };
        if let [reshaped] = said.reshaped.as_slice() {
            assert_eq!(reshaped.outcome, Err(Undone::NotReleased));
            // By that same tick the region is the worker's again, with a higher
            // epoch: nobody else is there to be given it.
            assert_eq!(said.workers, ["a"]);
            let now = lone.cluster.assignments("a");
            assert_eq!(now[0], held[0]);
            assert_eq!(now[1].region, held[1].region);
            assert!(now[1].epoch > held[1].epoch, "{now:?}");
            break lone.now;
        }
        assert_eq!(said.reshaped, []);
        assert!(lone.now <= asked + LEASE, "{}", lone.now);
    };
    if !silent {
        assert_eq!(ended, asked + LEASE + Lone::LOOK);
    }
    let again = lone.until_merging(ended + 20 * LEASE);
    (ended, again)
}

// Q13.
#[test]
fn a_coordinator_that_is_alone_notes_no_failure_when_a_merge_is_not_released_in_time() {
    // A rest of one lease, so three leases after an attempt that failed.
    let long = 3 * LEASE;
    for silent in [false, true] {
        let mut lone = Lone::begin(Coordinator::alone, LEASE);
        let asked = lone.until_merging(2 * LEASE);
        let (ended, again) = a_merge_that_is_not_released(lone, asked, silent);
        // Nothing but the time the two regions are left alone for after an attempt
        // that failed holds the next one back.
        assert!(again <= ended + long + 2 * Lone::LOOK, "{silent} {again}");
        assert!(again < ended + u64::from(Coordinator::FAULT_MEMORY) * LEASE);
    }

    // In a cluster the owner has failed the region and is at fault for six leases,
    // and nothing is begun with a region whose owner is.
    let mut lone = Lone::begin(Coordinator::new, LEASE);
    let asked = lone.until_merging(2 * LEASE);
    let (ended, again) = a_merge_that_is_not_released(lone, asked, false);
    assert_eq!(again, ended + u64::from(Coordinator::FAULT_MEMORY) * LEASE);
}

#[test]
fn a_coordinator_that_is_alone_notes_no_failure_when_a_release_is_not_answered_in_time() {
    for alone in [true, false] {
        let make = if alone {
            Coordinator::alone
        } else {
            Coordinator::new
        };
        let mut cluster = cluster(make);
        let held = [assignment(0, 5, 0), assignment(1, 6, 1)];
        cluster.register(1, "a", "a:25601", &held);
        cluster.register(2, "b", "b:25601", &[]);
        for name in ["a", "b"] {
            cluster.heartbeat(LEASE, name);
        }
        cluster.move_region(LEASE, 1, None, 7).unwrap();
        for name in ["a", "b"] {
            cluster.heartbeat(2 * LEASE + 1, name);
        }
        // The release is overdue and ends all the same: the region goes to the other.
        let said = cluster.tick(2 * LEASE + 1);
        assert_eq!(said.workers, ["a", "b"]);
        assert_eq!(cluster.assignments("a"), [held[0]]);
        let failed = cluster.coordinator.workers["a"].failed;
        assert_eq!(failed.is_none(), alone);
    }
}

/// The epoch with which the worker of these tests says it released region 5: above
/// the first epoch of the coordinator, so that an epoch above it is issued only if
/// the word was believed.
const SAID: u64 = FIRST_EPOCH + 40;

/// A coordinator made with `new` that has been handed no list, and the workers
/// `a`, `b` and `c` of `names`, which registered in that order holding nothing. `a`
/// has said that it released region 5 with [`SAID`]. All of it within the grace
/// period.
fn a_word_is_kept(names: &[&str]) -> Cluster {
    let mut cluster = cluster(Coordinator::new);
    for (at, name) in (1..).zip(names) {
        let address = format!("{name}:25601");
        assert_eq!(
            cluster.register(at, name, &address, &[]),
            Changes::default()
        );
    }
    let before = cluster.table();
    // That call changes nothing.
    assert_eq!(cluster.released(10, "a", 5, SAID), Changes::default());
    assert_eq!(cluster.table(), before);
    assert_eq!(cluster.coordinator.waiting(), []);
    assert_eq!(cluster.coordinator.words, [word("a", 5, SAID)]);
    assert!(cluster.coordinator.awaits_the_list());
    cluster
}

fn word(worker: &str, region: u32, epoch: u64) -> Word {
    Word {
        worker: worker.to_owned(),
        region: RegionId(region),
        epoch,
    }
}

/// The list of a world whose home region 0 was never opened and whose region 5 was
/// last opened with `epoch`.
fn with_region_5(epoch: u64) -> RegionList {
    listing(&[(0, 0), (5, epoch)], &[], 6)
}

/// The place of a worker among the workers: one with a higher number is behind.
fn place(cluster: &Cluster, name: &str) -> u64 {
    cluster.coordinator.workers[name].arrival
}

// Q12.1, Q12.2 and N18.
#[test]
fn a_region_released_before_the_first_list_is_given_away_by_the_list_that_names_it() {
    // Whether the list has the epoch the worker released it with or a lower one.
    for listed in [SAID, 12] {
        let mut cluster = a_word_is_kept(&["a", "b"]);
        let list = with_region_5(listed);
        // By that very call, though the grace period is not over, and to the other
        // worker: the one that released it is behind all the others.
        assert_eq!(cluster.listed(20, &list), changes(&["b"], true));
        assert_eq!(cluster.assignments("b"), [assignment(5, SAID + 1, 0)]);
        assert!(place(&cluster, "a") > place(&cluster, "b"));
        // The home region waits out the grace period, like any region of a list.
        assert_eq!(cluster.coordinator.waiting(), [RegionId(0)]);
        assert_eq!(cluster.coordinator.words, []);
        cluster.heartbeat(LEASE - 1, "a");
        cluster.heartbeat(LEASE - 1, "b");
        assert_eq!(cluster.tick(LEASE - 1), Changes::default());
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
    }

    // With nobody else there, to the worker that released it.
    let mut cluster = a_word_is_kept(&["a"]);
    assert_eq!(
        cluster.listed(20, &with_region_5(SAID)),
        changes(&["a"], true)
    );
    assert_eq!(cluster.assignments("a"), [assignment(5, SAID + 1, 0)]);
}

// Q12.3.
#[test]
fn a_release_that_was_kept_is_dropped_if_the_list_has_the_region_with_a_higher_epoch() {
    let mut cluster = a_word_is_kept(&["a", "b"]);
    // Somebody has run the region since: the word is stale.
    assert_eq!(
        cluster.listed(20, &with_region_5(SAID + 1)),
        changes(&[], true)
    );
    assert_eq!(cluster.coordinator.words, []);
    assert_eq!(cluster.coordinator.waiting(), [RegionId(0), RegionId(5)]);
    // `a` has let go of nothing as far as this coordinator knows, and is where it was.
    assert!(place(&cluster, "a") < place(&cluster, "b"));

    // The region waits out the grace period like any region of the list.
    for name in ["a", "b"] {
        cluster.heartbeat(LEASE - 1, name);
    }
    assert_eq!(cluster.tick(LEASE - 1), Changes::default());
    assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
    assert_eq!(cluster.assignments("a"), [assignment(0, SAID + 2, 0)]);
    assert_eq!(cluster.assignments("b"), [assignment(5, SAID + 3, 1)]);
}

// Q12.4.
#[test]
fn a_release_that_was_kept_is_dropped_if_the_list_does_not_have_the_region_living() {
    let lists = [
        // It was absorbed.
        listing(&[(0, 0), (2, 0)], &[(5, 2)], 6),
        // The store has had such a region and has it no longer.
        listing(&[(0, 0)], &[], 9),
        // The store has never had one: it was rolled back, or is another store.
        listing(&[(0, 0)], &[], 4),
    ];
    for list in lists {
        let mut cluster = a_word_is_kept(&["a", "b"]);
        cluster.listed(20, &list);
        assert_eq!(cluster.coordinator.words, [], "{list:?}");
        let known = |cluster: &Cluster| cluster.coordinator.regions.contains_key(&RegionId(5));
        assert!(!known(&cluster), "{list:?}");
        assert!(place(&cluster, "a") < place(&cluster, "b"));

        // Nor after any later call: the word is gone, and not judged a second time.
        for name in ["a", "b"] {
            cluster.heartbeat(LEASE, name);
        }
        cluster.tick(LEASE);
        cluster.listed(LEASE + 1, &list);
        assert!(!known(&cluster), "{list:?}");
    }
}

// Q12.5.
#[test]
fn a_release_that_was_kept_is_dropped_if_a_worker_registers_holding_the_region_meanwhile() {
    let mut cluster = a_word_is_kept(&["a", "b"]);
    let held = assignment(5, SAID + 1, 3);
    // It owns the region by its report.
    assert_eq!(
        cluster.register(11, "b", "b:25601", &[held]),
        changes(&["b"], true)
    );
    assert_eq!(
        cluster.listed(20, &with_region_5(SAID + 1)),
        changes(&[], true)
    );
    assert_eq!(cluster.assignments("b"), [held]);
    assert_eq!(cluster.coordinator.words, []);
    assert!(place(&cluster, "a") < place(&cluster, "b"));

    // Also with an epoch that the word's is not below: the region has an owner.
    let mut cluster = a_word_is_kept(&["a", "b"]);
    let held = assignment(5, SAID - 1, 3);
    cluster.register(11, "b", "b:25601", &[held]);
    cluster.listed(20, &with_region_5(SAID - 1));
    assert_eq!(cluster.assignments("b"), [held]);
    assert!(place(&cluster, "a") < place(&cluster, "b"));
}

// Q12.6.
#[test]
fn a_release_that_was_kept_is_dropped_if_its_worker_is_no_longer_registered() {
    // Its lease ran out. The grace period is over by then, so the region is given
    // away by the list, like any region of a list.
    let mut cluster = a_word_is_kept(&["a", "b"]);
    cluster.heartbeat(LEASE + 11, "b");
    assert_eq!(cluster.tick(LEASE + 11), Changes::default());
    assert!(!cluster.heartbeat(LEASE + 11, "a"));
    assert_eq!(cluster.coordinator.words, [word("a", 5, SAID)]);
    assert_eq!(
        cluster.listed(LEASE + 12, &with_region_5(SAID)),
        changes(&["b"], true)
    );
    assert_eq!(cluster.coordinator.words, []);
    assert_eq!(
        cluster.assignments("b"),
        [assignment(0, SAID + 1, 0), assignment(5, SAID + 2, 1)]
    );

    // It said that it leaves, and was forgotten by that call, as it owned nothing.
    // The grace period is not over: the region is not let go, and waits for it.
    //
    // Scenario Q12.7 of the record has such a region let go. As the rule is written
    // (section 2.3: "`name` is no longer registered"), and with a worker that owns
    // nothing forgotten as soon as it says that it leaves, it is not.
    let mut cluster = a_word_is_kept(&["a", "b"]);
    let left = cluster.leaving(11, "a");
    assert_eq!(left.gone, ["a"]);
    assert_eq!(cluster.listed(20, &with_region_5(SAID)), changes(&[], true));
    assert_eq!(cluster.coordinator.words, []);
    assert_eq!(cluster.coordinator.waiting(), [RegionId(0), RegionId(5)]);
    cluster.heartbeat(LEASE - 1, "b");
    assert_eq!(cluster.tick(LEASE - 1), Changes::default());
    assert_eq!(cluster.tick(LEASE), changes(&["b"], true));
}

// Q12.7, with a worker that has lost its connection where the record has one that
// leaves: see the test before this one.
#[test]
fn a_region_that_is_let_go_by_the_first_list_waits_for_a_worker_and_not_for_the_grace_period() {
    let mut cluster = a_word_is_kept(&["a"]);
    assert_eq!(cluster.disconnected(11, "a"), Changes::default());
    assert_eq!(cluster.listed(20, &with_region_5(SAID)), changes(&[], true));
    // Known, without an owner and let go: nobody is there who could be given it.
    assert_eq!(cluster.table().waiting, 2);
    assert!(cluster.coordinator.regions[&RegionId(5)].let_go);
    assert!(!cluster.coordinator.regions[&RegionId(0)].let_go);
    assert_eq!(cluster.coordinator.regions[&RegionId(5)].epoch, SAID);

    // Whoever comes is given it by the next tick, within the grace period, and not
    // the home region with it.
    assert_eq!(
        cluster.register(30, "b", "b:25601", &[]),
        Changes::default()
    );
    assert_eq!(cluster.tick(31), changes(&["b"], true));
    assert_eq!(cluster.assignments("b"), [assignment(5, SAID + 1, 0)]);
    assert_eq!(cluster.coordinator.waiting(), [RegionId(0)]);
}

// Q12.8.
#[test]
fn releases_that_were_kept_are_judged_in_the_order_they_were_said() {
    let mut cluster = a_word_is_kept(&["a", "b", "c"]);
    assert_eq!(cluster.released(11, "c", 5, SAID + 2), Changes::default());
    // The same word said again is one word, wherever it is said.
    assert_eq!(cluster.released(12, "a", 5, SAID), Changes::default());
    assert_eq!(cluster.released(13, "c", 5, SAID + 2), Changes::default());
    let kept = [word("a", 5, SAID), word("c", 5, SAID + 2)];
    assert_eq!(cluster.coordinator.words, kept);

    // Both hold: the list's epoch is not above the first, and the first is not above
    // the second. So both workers are behind `b`, and no epoch at or below the
    // higher of the two is issued.
    assert_eq!(
        cluster.listed(20, &with_region_5(SAID)),
        changes(&["b"], true)
    );
    assert_eq!(cluster.assignments("b"), [assignment(5, SAID + 3, 0)]);
    assert!(place(&cluster, "b") < place(&cluster, "a"));
    assert!(place(&cluster, "a") < place(&cluster, "c"));

    // Said the other way round, the second is stale by the time it is judged: the
    // region has had the epoch of the first. `a` keeps its place before `b`, and is
    // given the region.
    let mut cluster = self::cluster(Coordinator::new);
    for (at, name) in (1..).zip(["a", "b", "c"]) {
        cluster.register(at, name, &format!("{name}:25601"), &[]);
    }
    cluster.released(10, "c", 5, SAID + 2);
    cluster.released(11, "a", 5, SAID);
    assert_eq!(
        cluster.listed(20, &with_region_5(SAID)),
        changes(&["a"], true)
    );
    assert_eq!(cluster.assignments("a"), [assignment(5, SAID + 3, 0)]);
    assert!(place(&cluster, "a") < place(&cluster, "b"));
    assert!(place(&cluster, "b") < place(&cluster, "c"));
}

/// The list's epochs are raised before the words are judged and regions are given
/// away after it, by the one `assign` that ends the call.
#[test]
fn releases_that_were_kept_are_judged_between_the_lists_epochs_and_giving_regions_away() {
    // The grace period is over, so every region of the list is given away by the
    // call. A word that is believed still decides who is given which: its worker is
    // behind the others before the first region is given.
    let mut cluster = a_word_is_kept(&["a", "b"]);
    for name in ["a", "b"] {
        cluster.heartbeat(LEASE, name);
    }
    assert_eq!(cluster.tick(LEASE), Changes::default());
    assert_eq!(
        cluster.listed(LEASE, &with_region_5(SAID)),
        changes(&["a", "b"], true)
    );
    assert_eq!(cluster.assignments("b"), [assignment(0, SAID + 1, 0)]);
    assert_eq!(cluster.assignments("a"), [assignment(5, SAID + 2, 1)]);

    // And one that is not believed changes nothing of that: `a` is first.
    let mut cluster = a_word_is_kept(&["a", "b"]);
    for name in ["a", "b"] {
        cluster.heartbeat(LEASE, name);
    }
    cluster.tick(LEASE);
    cluster.listed(LEASE, &with_region_5(SAID + 1));
    assert_eq!(cluster.assignments("a"), [assignment(0, SAID + 2, 0)]);
    assert_eq!(cluster.assignments("b"), [assignment(5, SAID + 3, 1)]);

    // A word is judged by the list's epoch, not by the one the coordinator had of
    // the region before. `b` reports the region with an epoch below the word's,
    // says that it leaves and is gone: the region is known without an owner. The
    // list has it with an epoch above the word's, so somebody has run it since.
    let mut cluster = a_word_is_kept(&["a", "b"]);
    let held = assignment(5, SAID - 5, 3);
    cluster.register(11, "b", "b:25601", &[held]);
    assert_eq!(cluster.leaving(12, "b"), asks(&[order("b", 5, SAID - 5)]));
    assert_eq!(cluster.disconnected(13, "b"), changes(&["b"], true));
    assert_eq!(cluster.coordinator.words, [word("a", 5, SAID)]);
    cluster.listed(20, &with_region_5(SAID + 1));
    let region = &cluster.coordinator.regions[&RegionId(5)];
    assert_eq!((region.epoch, region.let_go), (SAID + 1, false));
    assert_eq!(cluster.assignments("a"), []);
}

// Q12.9.
#[test]
fn a_reading_that_fails_keeps_the_releases_that_were_kept() {
    let mut cluster = a_word_is_kept(&["a", "b"]);
    for at in [11, 12, 13] {
        assert_eq!(cluster.unlisted(at), Changes::default());
        assert_eq!(cluster.tick(at), Changes::default());
        assert_eq!(cluster.coordinator.words, [word("a", 5, SAID)]);
    }
    assert_eq!(
        cluster.listed(20, &with_region_5(SAID)),
        changes(&["b"], true)
    );
    assert_eq!(cluster.assignments("b"), [assignment(5, SAID + 1, 0)]);
    assert_eq!(cluster.coordinator.words, []);
}

// Q12.10.
#[test]
fn no_release_is_kept_once_the_first_list_has_been_handed_in() {
    let mut cluster = cluster(Coordinator::new);
    cluster.register(1, "a", "a:25601", &[]);
    cluster.register(2, "b", "b:25601", &[]);
    cluster.listed(3, &listing(&[(0, 0)], &[], 5));
    // A region the coordinator does not know now is one the store does not have.
    assert_eq!(cluster.released(10, "a", 5, SAID), Changes::default());
    assert_eq!(cluster.coordinator.words, []);
    // Nor while a later reading fails.
    cluster.unlisted(11);
    assert_eq!(cluster.released(12, "a", 6, SAID), Changes::default());
    assert_eq!(cluster.coordinator.words, []);

    // The store makes region 5, and it waits out the grace period like any other.
    assert_eq!(cluster.listed(20, &with_region_5(SAID)), changes(&[], true));
    assert_eq!(cluster.coordinator.waiting(), [RegionId(0), RegionId(5)]);
    assert!(place(&cluster, "a") < place(&cluster, "b"));
}

// Q12.11.
#[test]
fn a_coordinator_made_knowing_its_regions_keeps_no_release() {
    let mut cluster = Cluster::new(&[0]);
    cluster.register(1, "a", "a:25601", &[]);
    cluster.register(2, "b", "b:25601", &[]);
    assert_eq!(cluster.released(10, "a", 5, SAID), Changes::default());
    assert_eq!(cluster.coordinator.words, []);

    // The first list it is handed adds the region without an owner, and does not
    // let go of it.
    let list = listing(&[(0, 0), (1, 0), (5, SAID)], &[], 6);
    assert_eq!(cluster.listed(20, &list), changes(&[], true));
    assert_eq!(
        cluster.coordinator.waiting(),
        [RegionId(0), RegionId(1), RegionId(5)]
    );
    assert!(!cluster.coordinator.regions[&RegionId(5)].let_go);
}

// Q12.12.
#[test]
fn only_a_registered_workers_release_of_a_region_that_is_not_known_is_kept() {
    let mut cluster = cluster(Coordinator::new);
    cluster.register(1, "a", "a:25601", &[]);
    cluster.register(2, "b", "b:25601", &[assignment(3, 7, 0)]);
    // From a name that is not registered.
    assert_eq!(cluster.released(10, "nobody", 5, SAID), Changes::default());
    // Of a region the coordinator knows, which has an owner: as ever, nothing.
    assert_eq!(cluster.released(10, "a", 3, 7), Changes::default());
    assert_eq!(cluster.coordinator.words, []);

    // Of a region the coordinator knows without an owner, as a list named it: by
    // the case there has always been, at once.
    cluster.listed(11, &listing(&[(0, 0), (3, 7), (5, SAID)], &[], 6));
    assert_eq!(cluster.coordinator.waiting(), [RegionId(0), RegionId(5)]);
    // The home region, to nobody yet; region 5 to the worker with fewer regions
    // that is not the one that let go of it.
    cluster.register(12, "c", "c:25601", &[]);
    assert_eq!(cluster.released(13, "a", 5, SAID), changes(&["c"], true));
    assert_eq!(cluster.assignments("c"), [assignment(5, SAID + 1, 1)]);
    assert_eq!(cluster.coordinator.words, []);
}

/// Nothing that is kept grows without what is said growing, and the first list
/// leaves none of it, whatever became of each word.
#[test]
fn the_releases_that_are_kept_are_each_kept_once_and_are_gone_with_the_first_list() {
    let mut cluster = a_word_is_kept(&["a", "b"]);
    for round in 0..100 {
        for region in [5, 6, 7] {
            cluster.released(11 + round, "a", region, SAID);
            cluster.released(11 + round, "b", region, SAID);
        }
    }
    assert_eq!(cluster.coordinator.words.len(), 6);
    // Believed, stale, and of a region the store does not have.
    let list = listing(&[(0, 0), (5, SAID), (6, SAID + 1)], &[], 7);
    cluster.listed(200, &list);
    assert_eq!(cluster.coordinator.words, []);
    // And none is kept from then on.
    cluster.released(201, "a", 9, SAID);
    assert_eq!(cluster.coordinator.words, []);
}

/// A word is heard like anything else a worker says: a worker that says nothing
/// else is not forgotten for it.
#[test]
fn a_worker_whose_release_is_kept_has_been_heard_from() {
    let mut cluster = a_word_is_kept(&["a"]);
    assert_eq!(cluster.released(LEASE, "a", 5, SAID), Changes::default());
    assert_eq!(cluster.tick(2 * LEASE), Changes::default());
    assert!(cluster.heartbeat(2 * LEASE, "a"));
}
