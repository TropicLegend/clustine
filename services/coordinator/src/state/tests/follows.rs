//! Tests of a coordinator that merges and splits regions by itself: the rules of
//! `docs/adr/0016-when-to-merge-and-split.md`, whose sections and scenarios (K1 to
//! K26) the tests name.
//!
//! Regions are told apart from chunks by what they are called: `put(1, ..)` is about
//! region 1, and `(100, 0)` is the chunk with x 100 and z 0. Region 0 is the home
//! region and the chunk players enter in is `(0, 0)`.

use clustine_world::ChunkArea;

use super::follow::{Going, Kept};
use super::*;
use crate::policy::named;

/// How long a region is left alone in these tests, in milliseconds.
const REST: u64 = 10_000;

/// How long a region is without players before it is absorbed, and how long one is
/// left alone after an attempt that failed.
const EMPTY_FOR: u64 = 3 * REST;
const LONG: u64 = 3 * REST;

/// How far apart the looks of these tests are: the ticks of the coordinator, and the
/// reports of the workers.
const LOOK: u64 = 250;

/// When a [`World`] is ready: every region has been assigned, at `LEASE`, has been
/// reported and has rested.
const READY: u64 = LEASE + REST;

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
    following_by(small(), boundaries)
}

/// [`following`] for a coordinator that goes by `policy`.
fn following_by(policy: Policy, boundaries: &[i32]) -> Cluster {
    let layout = Layout::new(boundaries.to_vec()).unwrap();
    let start = Instant::now();
    let config = CoordinatorConfig {
        layout: layout.clone(),
        spawn: SPAWN,
        lease: Duration::from_millis(LEASE),
        follow: Some(policy),
    };
    Cluster {
        coordinator: Coordinator::knowing(config, start, FIRST_EPOCH, &stripes_of(&layout)),
        layout,
        start,
        addresses: BTreeMap::new(),
        last: None,
    }
}

/// What the list has of a stripe: a region that is pinned to an area. Which area
/// does not concern the coordinator.
fn stripe(region: u32) -> RegionInfo {
    RegionInfo {
        pinned: vec![ChunkArea::EVERYWHERE],
        ..living(region, 0)
    }
}

/// The chunks a split names for groups in these chunks, with the margin of 2.
fn around(chunks: &[(i32, i32)]) -> Vec<ChunkPos> {
    let chunks: Vec<ChunkPos> = chunks.iter().map(|(x, z)| ChunkPos::new(*x, *z)).collect();
    named(&small(), &[chunks.as_slice()])
}

fn merge_of(survivor: u32, absorbed: u32) -> Asked {
    Asked::Merge {
        survivor: RegionId(survivor),
        absorbed: RegionId(absorbed),
    }
}

fn split_of(region: u32) -> Asked {
    Asked::Split {
        region: RegionId(region),
    }
}

/// Chunks with players in them, each with how many, as the tests write them.
type Where = Vec<((i32, i32), u32)>;

/// What the world store does when the list is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Store {
    /// It answers at once, at the tick that asked.
    Answers,
    /// It says at once that the list cannot be read.
    Fails,
    /// It does not answer; the test hands a reading in when it wants one.
    Silent,
}

/// A cluster under a coordinator that decides by itself, and what a test needs to
/// drive it one look at a time: workers that vouch for their regions and say where
/// the players are, a world store whose list is read whenever it is asked for, and
/// the players, which the test puts where it wants them.
struct World {
    cluster: Cluster,
    /// The time of the last look, in milliseconds.
    now: u64,
    /// The list as the world store has it.
    list: RegionList,
    /// What the store does when the list is asked for.
    store: Store,
    /// Where the players of each region are, as its worker says at every look.
    crowds: BTreeMap<u32, Where>,
    /// The tick the regions are at, which goes up by one with every look.
    tick: u64,
    /// The regions whose workers say nothing of their players.
    silent: BTreeSet<u32>,
    /// The workers that say nothing at all any more.
    dead: BTreeSet<String>,
    /// What the tick of the last look said.
    said: Changes,
}

impl World {
    /// A world of stripes with these boundaries and these workers, which have
    /// registered at 0. The list has been read once and has every stripe pinned;
    /// nothing has been assigned yet, as the coordinator is new.
    fn begin(boundaries: &[i32], workers: &[&str]) -> Self {
        Self::begin_by(small(), boundaries, workers)
    }

    /// [`World::begin`] under a coordinator that goes by `policy`.
    fn begin_by(policy: Policy, boundaries: &[i32], workers: &[&str]) -> Self {
        let mut cluster = following_by(policy, boundaries);
        for name in workers {
            cluster.register(0, name, &format!("{name}:25601"), &[]);
        }
        let count = u32::try_from(boundaries.len()).unwrap() + 1;
        let list = RegionList {
            regions: (0..count).map(stripe).collect(),
            ..stripes(count)
        };
        assert_eq!(cluster.tick(0), reads());
        cluster.listed(0, &list);
        Self {
            cluster,
            now: 0,
            list,
            store: Store::Answers,
            crowds: BTreeMap::new(),
            tick: 0,
            silent: BTreeSet::new(),
            dead: BTreeSet::new(),
            said: Changes::default(),
        }
    }

    /// [`World::begin`], and then looks until [`READY`]: the regions were assigned
    /// at `LEASE`, the lowest to the worker named first, and have rested. Nobody
    /// plays.
    fn new(boundaries: &[i32], workers: &[&str]) -> Self {
        let mut world = Self::begin(boundaries, workers);
        world.quiet_until(READY);
        assert!(world.cluster.table().is_complete());
        world
    }

    /// The same without pinned regions: what the world is like off stripes, where a
    /// region without players is absorbed.
    fn unpinned(boundaries: &[i32], workers: &[&str]) -> Self {
        let mut world = Self::begin(boundaries, workers);
        for info in &mut world.list.regions {
            info.pinned.clear();
        }
        world.read();
        world.quiet_until(READY);
        world
    }

    /// The players of `region` are in these chunks from the next look on.
    fn put(&mut self, region: u32, crowds: &[((i32, i32), u32)]) {
        self.crowds.insert(region, crowds.to_vec());
    }

    /// The store's list is handed in as it is.
    fn read(&mut self) -> Changes {
        let list = self.list.clone();
        self.cluster.listed(self.now, &list)
    }

    /// Every worker vouches for its regions and says where their players are, at
    /// the time of the last look and `later` milliseconds.
    fn report(&mut self, later: u64) {
        let names = self.cluster.addresses.keys();
        let names: Vec<String> = names
            .filter(|name| !self.dead.contains(*name))
            .cloned()
            .collect();
        for name in names {
            self.cluster.heartbeat(self.now + later, &name);
            let reports: Vec<PlayersOf> = self
                .cluster
                .assignments(&name)
                .into_iter()
                .filter(|held| !self.silent.contains(&held.region.0))
                .map(|held| {
                    let crowds = self.crowds.get(&held.region.0);
                    players_of(held, self.tick, crowds.map_or(&[], Vec::as_slice))
                })
                .collect();
            self.cluster.players(self.now + later, &name, &reports);
        }
    }

    /// The coordinator's tick at the time of the last look. Returns what it began:
    /// the merges and splits that are under way after it and were not before.
    fn decide(&mut self) -> Vec<Asked> {
        let before = self.cluster.coordinator.under_way();
        self.said = self.cluster.tick(self.now);
        if self.said.read {
            match self.store {
                Store::Answers => {
                    self.read();
                }
                Store::Fails => {
                    self.cluster.unlisted(self.now);
                }
                Store::Silent => {}
            }
        }
        let mut after = self.cluster.coordinator.under_way();
        after.retain(|asked| !before.contains(asked));
        after
    }

    /// One look, a quarter of a second after the last: the regions tick on, the
    /// workers report, and the coordinator ticks. Returns what it began.
    fn look(&mut self) -> Vec<Asked> {
        self.now += LOOK;
        self.tick += 1;
        self.report(0);
        self.decide()
    }

    /// As [`World::look`], but the workers report 100 ms after the last look and not
    /// at the instant of this one: a worker's reports do not fall on the
    /// coordinator's ticks.
    fn look_after_reports(&mut self) -> Vec<Asked> {
        self.tick += 1;
        self.report(100);
        self.now += LOOK;
        self.decide()
    }

    /// The owner of `region` reports it with an epoch the coordinator did not have
    /// for it, so that the region rests from now.
    fn rests_from_now(&mut self, region: u32) {
        let (name, _) = self.held(region);
        let mut holding = self.cluster.assignments(&name);
        for held in &mut holding {
            if held.region == RegionId(region) {
                held.epoch = self.cluster.coordinator.last_epoch + 1;
            }
        }
        let address = self.cluster.addresses[&name].clone();
        self.cluster.register(self.now, &name, &address, &holding);
        assert_eq!(self.alone_until(region), Some(self.now + REST));
    }

    /// Looks until something is begun, which has to be at `limit` at the latest, and
    /// returns when that was and what.
    fn until_begun(&mut self, limit: u64) -> (u64, Vec<Asked>) {
        while self.now < limit {
            let begun = self.look();
            if !begun.is_empty() {
                return (self.now, begun);
            }
        }
        panic!("nothing was begun until {limit}");
    }

    /// Looks until `until`, during which nothing is begun and no worker is asked to
    /// release a region.
    fn quiet_until(&mut self, until: u64) {
        while self.now < until {
            let begun = self.look();
            assert!(begun.is_empty(), "at {}: {begun:?}", self.now);
            let releases = &self.said.releases;
            assert!(releases.is_empty(), "at {}: {releases:?}", self.now);
        }
    }

    /// The worker that was asked to release `absorbed` for a merge has done so, the
    /// survivor's worker has absorbed it, and the list shows that. The players of
    /// the one are the other's from now on.
    fn merge_ends(&mut self, survivor: u32, absorbed: u32) -> Changes {
        let merge = self.cluster.coordinator.merges[&RegionId(absorbed)].clone();
        assert_eq!(merge.survivor, RegionId(survivor));
        let MergeStage::Releasing { from, epoch } = merge.stage else {
            panic!("{merge:?}");
        };
        self.cluster.released(self.now, &from, absorbed, epoch);
        self.cluster
            .absorb_ended(self.now, &merge.owner, survivor, absorbed, Ok(()));
        let absorbed_id = RegionId(absorbed);
        self.list.regions.retain(|info| info.region != absorbed_id);
        self.list.absorbed.push((absorbed_id, RegionId(survivor)));
        let crowds = self.crowds.remove(&absorbed).unwrap_or_default();
        self.crowds.entry(survivor).or_default().extend(crowds);
        self.read()
    }

    /// As [`World::merge_ends`], but the survivor's worker says that nothing came of
    /// it, and the list has both regions still.
    fn merge_fails(&mut self, survivor: u32, absorbed: u32, why: Off) -> Changes {
        let merge = self.cluster.coordinator.merges[&RegionId(absorbed)].clone();
        let MergeStage::Releasing { from, epoch } = merge.stage else {
            panic!("{merge:?}");
        };
        self.cluster.released(self.now, &from, absorbed, epoch);
        self.cluster
            .absorb_ended(self.now, &merge.owner, survivor, absorbed, Err(why));
        self.read()
    }

    /// The worker that was told to split `region` says what came of it, and the list
    /// is read. A part is in the list from then on. Where the players are after it
    /// is for the test to say.
    fn split_ends(&mut self, region: u32, outcome: Result<u32, Off>) -> Changes {
        let split = self.cluster.coordinator.splits[&RegionId(region)].clone();
        if let Ok(part) = outcome {
            self.list.regions.push(living(part, split.as_epoch));
            self.list.next = RegionId(part + 1);
        }
        let now = self.now;
        let said = self
            .cluster
            .split_ended(now, &split.owner, region, split.as_epoch, outcome);
        self.read();
        said
    }

    /// The `SplitOff` of the last look, if it had one: the region, the chunks and
    /// the part it names.
    fn split_off(&self) -> Option<(RegionId, Vec<ChunkPos>, RegionId)> {
        self.said.orders.iter().find_map(|told| match &told.order {
            Order::SplitOff {
                region,
                chunks,
                part,
                ..
            } => Some((*region, chunks.clone(), *part)),
            _ => None,
        })
    }

    /// The regions whose owners the last look told to prepare.
    fn prepared(&self) -> Vec<u32> {
        let orders = self.said.orders.iter();
        let prepared = orders.filter_map(|told| match &told.order {
            Order::Prepare { region, .. } => Some(region.0),
            _ => None,
        });
        prepared.collect()
    }

    fn kept(&self, region: u32) -> &Kept {
        &self.cluster.coordinator.regions[&RegionId(region)].kept
    }

    /// The crowds of the sighting of `region`, if it has one.
    fn sighted(&self, region: u32) -> Option<Where> {
        let sighting = self.kept(region).sighting.as_ref()?;
        let crowds = sighting.crowds.iter();
        Some(
            crowds
                .map(|(chunk, players)| ((chunk.x, chunk.z), *players))
                .collect(),
        )
    }

    fn fresh(&self, region: u32, at: u64) -> bool {
        let coordinator = &self.cluster.coordinator;
        coordinator.fresh(RegionId(region), self.cluster.at(at))
    }

    /// Before when `region` is left alone, in milliseconds.
    fn alone_until(&self, region: u32) -> Option<u64> {
        let until = self.cluster.coordinator.alone_until(RegionId(region))?;
        let since = until.duration_since(self.cluster.start);
        Some(u64::try_from(since.as_millis()).unwrap())
    }

    /// The owner of `region` and its assignment.
    fn held(&self, region: u32) -> (String, Assignment) {
        let names = self.cluster.addresses.keys();
        let mut held = names.flat_map(|name| {
            let assignments = self.cluster.assignments(name).into_iter();
            assignments.map(move |held| (name.clone(), held))
        });
        held.find(|(_, held)| held.region == RegionId(region))
            .unwrap_or_else(|| panic!("region {region} has no owner"))
    }
}

// What is heard and kept: sections 2.3 and 2.4.

#[test]
fn a_coordinator_that_decides_nothing_by_itself_keeps_nothing_of_where_players_are() {
    let mut cluster = a_runs_and_b_waits();
    let held = cluster.assignments("a")[0];
    let report = [players_of(held, 40, &[((3, -2), 2)])];
    assert!(cluster.players(LEASE + 250, "a", &report));
    cluster.tick(LEASE + 250);
    let kept = &cluster.coordinator.regions[&RegionId(0)].kept;
    assert!(kept.sighting.is_none() && kept.held.is_none() && kept.alone_until.is_none());
    assert_eq!(cluster.coordinator.alone_until(RegionId(0)), None);
    assert!(cluster.coordinator.noted.answered.is_none());
}

#[test]
fn what_a_region_s_owner_says_of_its_players_is_kept_by_chunk_and_nobody_else_s_word_is() {
    let mut world = World::new(&[4], &["a", "b"]);
    let (zero, one) = (world.held(0).1, world.held(1).1);
    assert_eq!(
        (world.held(0).0.as_str(), world.held(1).0.as_str()),
        ("a", "b")
    );
    let now = READY + 100;
    let tick = world.tick + 1;

    // In whatever order they come: by chunk, each once with the players of every
    // entry that names it, and no chunk without players.
    let said = [players_of(
        zero,
        tick,
        &[((3, 1), 2), ((-1, 0), 1), ((3, 1), 1), ((2, 2), 0)],
    )];
    assert!(world.cluster.players(now, "a", &said));
    assert_eq!(world.sighted(0).unwrap(), [((-1, 0), 1), ((3, 1), 3)]);
    let sighting = world.kept(0).sighting.clone().unwrap();
    assert_eq!((sighting.owner.as_str(), sighting.epoch), ("a", zero.epoch));
    assert_eq!(
        (sighting.tick, sighting.taken),
        (tick, Some(world.cluster.at(now)))
    );

    // Not from a worker that does not own the region, and not with another epoch
    // than the owner runs it with.
    let before = world.sighted(1);
    let of_another = [players_of(one, tick + 5, &[((9, 9), 9)])];
    assert!(world.cluster.players(now, "a", &of_another));
    let stale = Assignment {
        epoch: one.epoch - 1,
        ..one
    };
    let with_another_epoch = [players_of(stale, tick + 5, &[((9, 9), 9)])];
    assert!(world.cluster.players(now, "b", &with_another_epoch));
    assert_eq!(world.sighted(1), before);
    assert_eq!(world.kept(1).sighting.as_ref().unwrap().tick, world.tick);
    // A worker the coordinator does not know is told so, and nothing is kept.
    assert!(!world.cluster.players(now, "c", &said));
}

#[test]
fn a_region_that_says_nothing_new_is_not_heard_anew_and_its_sighting_stops_being_fresh() {
    let mut world = World::new(&[4], &["a"]);
    let held = world.held(0).1;
    let tick = world.tick + 1;
    let said = [players_of(held, tick, &[((1, 0), 1)])];
    world.cluster.players(READY + 100, "a", &said);
    // Fresh for a second, and not a moment longer.
    assert!(world.fresh(0, READY + 100) && world.fresh(0, READY + 1100));
    assert!(!world.fresh(0, READY + 1101));

    // The region stands still: its worker repeats the tick, or an earlier one, and
    // whatever it says with that is passed over.
    for (later, tick) in [(350, tick), (600, tick - 1)] {
        let again = [players_of(held, tick, &[((7, 7), 7)])];
        world.cluster.players(READY + later, "a", &again);
    }
    assert_eq!(world.sighted(0).unwrap(), [((1, 0), 1)]);
    assert!(!world.fresh(0, READY + 1101));

    // The next tick of the region is heard.
    let on = [players_of(held, tick + 1, &[((2, 0), 1)])];
    world.cluster.players(READY + 1200, "a", &on);
    assert_eq!(world.sighted(0).unwrap(), [((2, 0), 1)]);
    assert!(world.fresh(0, READY + 2200) && !world.fresh(0, READY + 2201));
}

#[test]
fn a_report_about_a_region_in_a_merge_or_a_split_is_passed_over_until_that_has_ended() {
    let mut world = World::new(&[4, 8], &["a"]);
    world.put(0, &[((0, 0), 1)]);
    world.put(1, &[((100, 0), 1)]);
    world.put(2, &[((200, 0), 2)]);
    world.look();
    assert!(world.fresh(0, world.now) && world.fresh(1, world.now));

    // Somebody asks for a merge and for a split. From then on the sightings of
    // their regions are not fresh, whatever is said of them.
    world.cluster.merge(world.now, 0, 1, Some(7)).unwrap();
    world.cluster.split(world.now, 2, &CHUNKS, Some(8)).unwrap();
    for region in 0..3 {
        assert!(!world.fresh(region, world.now), "region {region}");
        assert!(
            world
                .kept(region)
                .sighting
                .as_ref()
                .unwrap()
                .taken
                .is_none()
        );
    }
    world.put(0, &[((50, 50), 5)]);
    world.put(2, &[((60, 60), 6)]);
    world.look();
    assert_eq!(world.sighted(0).unwrap(), [((0, 0), 1)]);
    assert_eq!(world.sighted(2).unwrap(), [((200, 0), 2)]);
    assert!(!world.fresh(0, world.now) && !world.fresh(2, world.now));

    // The first report after the end is taken, and is fresh.
    world.split_ends(2, Err(Off::Nobody));
    world.merge_ends(0, 1);
    assert!(!world.fresh(0, world.now) && !world.fresh(2, world.now));
    world.look();
    assert_eq!(world.sighted(0).unwrap(), [((50, 50), 5), ((100, 0), 1)]);
    assert_eq!(world.sighted(2).unwrap(), [((60, 60), 6)]);
    assert!(world.fresh(0, world.now) && world.fresh(2, world.now));
}

#[test]
fn since_when_a_region_is_without_players_is_the_first_of_its_reports_without_any_in_a_row() {
    let mut world = World::new(&[4], &["a"]);
    // It has had none since its first report, a look after it was assigned.
    let first = world.cluster.at(LEASE + LOOK);
    assert_eq!(world.kept(1).empty_since, Some(first));
    world.put(1, &[((100, 0), 1)]);
    world.look();
    assert_eq!(world.kept(1).empty_since, None);
    world.look();
    world.put(1, &[]);
    world.look();
    let left = world.cluster.at(world.now);
    world.look();
    world.look();
    assert_eq!(world.kept(1).empty_since, Some(left));
    // A chunk that is named with nobody in it is no player.
    world.put(1, &[((100, 0), 0)]);
    world.look();
    assert_eq!(world.kept(1).empty_since, Some(left));
}

#[test]
fn a_region_rests_when_it_is_given_an_owner_or_an_epoch_and_not_when_its_owner_is_back() {
    let mut world = World::begin(&[4], &["a"]);
    assert_eq!(world.alone_until(0), None);
    world.quiet_until(LEASE);
    // Assigned at the end of the grace period: a rest from then.
    assert_eq!(world.alone_until(0), Some(LEASE + REST));
    assert_eq!(world.alone_until(1), Some(LEASE + REST));
    world.quiet_until(READY + 5000);
    let (zero, one) = (world.held(0).1, world.held(1).1);
    let since = world.kept(1).empty_since;
    assert!(since.is_some());

    // The owner lost its connection and registers again with what it runs, a
    // moment after its last report. Nothing has happened to the regions: no rest
    // begins, their sightings are as fresh as they were, and the time a region has
    // been without players goes on.
    world.cluster.disconnected(world.now + 50, "a");
    world
        .cluster
        .register(world.now + 100, "a", "a:25601", &[zero, one]);
    assert_eq!(world.alone_until(0), Some(LEASE + REST));
    assert_eq!(world.alone_until(1), Some(LEASE + REST));
    assert!(world.fresh(0, world.now + 100) && world.fresh(1, world.now + 100));
    assert_eq!(world.kept(1).empty_since, since);

    // It reports region 1 with an epoch the coordinator did not have for it: that
    // region rests from then, its sighting is of another epoch and so not fresh,
    // and the time it has been without players is forgotten. Region 0 is as it was.
    let later = Assignment {
        epoch: one.epoch + 10,
        ..one
    };
    world
        .cluster
        .register(world.now + 200, "a", "a:25601", &[zero, later]);
    assert_eq!(world.alone_until(1), Some(world.now + 200 + REST));
    assert_eq!(world.alone_until(0), Some(LEASE + REST));
    assert!(world.fresh(0, world.now + 200) && !world.fresh(1, world.now + 200));
    assert!(world.sighted(1).is_some());
    assert_eq!(world.kept(1).empty_since, None);
}

#[test]
fn a_region_that_loses_its_owner_keeps_its_sighting_which_is_fresh_no_longer() {
    let mut world = World::new(&[4], &["a", "b"]);
    world.put(1, &[((100, 0), 3)]);
    world.look();
    assert!(world.fresh(1, world.now));
    let alone = world.alone_until(1);

    // Its worker dies: the lease runs out, and nobody else is there to take it but
    // the other worker, which is given it and has yet to say a word of it.
    let died = world.now;
    world.cluster.disconnected(died, "b");
    world.dead.insert("b".to_owned());
    world.put(1, &[((101, 0), 3)]);
    while world.held(1).0 == "b" {
        world.look();
    }
    assert!(world.now > died + LEASE);
    // What was last heard of its players stays; they are still somewhere near
    // there. It is not of the region's owner any more, and so not fresh. The
    // region rests from when it was given its new owner.
    assert_eq!(world.sighted(1).unwrap(), [((100, 0), 3)]);
    assert!(!world.fresh(1, world.now));
    assert!(world.alone_until(1) > alone);
    assert_eq!(world.alone_until(1), Some(world.now + REST));
    // The new owner's first report is taken whatever its tick: it is of another
    // owner and epoch.
    world.look();
    assert_eq!(world.sighted(1).unwrap(), [((101, 0), 3)]);
    assert!(world.fresh(1, world.now));
}

#[test]
fn the_crowds_of_a_region_the_list_shows_absorbed_go_to_the_region_that_took_it_in_the_end() {
    let mut world = World::new(&[4, 8, 12], &["a"]);
    world.put(0, &[((0, 0), 1)]);
    world.put(1, &[((100, 0), 2)]);
    world.put(2, &[((200, 0), 3)]);
    world.put(3, &[((300, 0), 4), ((0, 0), 1)]);
    world.look();
    let taken = world.kept(0).sighting.as_ref().unwrap().taken;

    // Somebody else merged them, and the list says so: region 2 went into region
    // 1, which went into region 0 since. Their players are still somewhere near
    // where they were, in the region that took them.
    world
        .list
        .regions
        .retain(|info| info.region.0 == 0 || info.region.0 == 3);
    world.list.absorbed = vec![(RegionId(1), RegionId(0)), (RegionId(2), RegionId(1))];
    world.read();
    assert_eq!(
        world.sighted(0).unwrap(),
        [((0, 0), 1), ((100, 0), 2), ((200, 0), 3)]
    );
    // Nothing else of the sighting changes: no worker has said anything new.
    assert_eq!(world.kept(0).sighting.as_ref().unwrap().taken, taken);
    assert!(!world.cluster.coordinator.regions.contains_key(&RegionId(1)));

    // A chunk that both have players in has the players of both.
    world.list.regions.retain(|info| info.region.0 == 0);
    world.list.absorbed.push((RegionId(3), RegionId(0)));
    world.read();
    assert_eq!(
        world.sighted(0).unwrap(),
        [((0, 0), 2), ((100, 0), 2), ((200, 0), 3), ((300, 0), 4)]
    );
}

#[test]
fn no_sighting_is_made_for_a_region_that_took_in_another_and_has_none_of_its_own() {
    // Section 2.4: a new coordinator knows the stripes, and the first reading shows
    // one of them absorbed by the home region, of which no worker has said a word.
    let mut world = World::begin(&[4, 8], &["a"]);
    world.silent.insert(0);
    world.put(1, &[((100, 0), 2)]);
    world.put(2, &[((200, 0), 1), ((207, 0), 1)]);
    world.quiet_until(READY);
    assert!(world.sighted(0).is_none() && world.sighted(1).is_some());

    world.list.regions.retain(|info| info.region.0 != 1);
    world.list.absorbed = vec![(RegionId(1), RegionId(0))];
    world.read();
    // The crowds go with the absorbed region.
    assert!(world.sighted(0).is_none());

    // Region 2 is surely apart, has stood and has rested, and is not split: the
    // players that region 0 took in are in no sighting, and nothing is begun
    // anywhere while a region has never been heard of (section 5.1).
    world.quiet_until(READY + 5000);
    assert!(world.prepared().is_empty());
    // The first report of the region that took the other in ends the wait.
    world.silent.clear();
    world.put(0, &[((100, 0), 2)]);
    assert_eq!(world.look(), [split_of(2)]);
}

#[test]
fn when_a_split_is_made_the_crowds_in_the_chunks_it_named_are_the_part_s() {
    let mut world = World::new(&[4], &["a"]);
    world.put(0, &[((0, 0), 2), ((3, 0), 1), ((4, 1), 1), ((5, 0), 1)]);
    world.look();
    // Asked by hand, with chunks of somebody's choosing: (3, 0) is not among them.
    let chunks = [
        ChunkPos::new(4, 1),
        ChunkPos::new(5, 0),
        ChunkPos::new(6, 0),
    ];
    world.cluster.split(world.now, 0, &chunks, Some(7)).unwrap();
    let as_epoch = world.cluster.coordinator.splits[&RegionId(0)].as_epoch;
    world.split_ends(0, Ok(2));

    assert_eq!(world.sighted(0).unwrap(), [((0, 0), 2), ((3, 0), 1)]);
    assert_eq!(world.sighted(2).unwrap(), [((4, 1), 1), ((5, 0), 1)]);
    // The part's sighting is of its owner and epoch, has the tick 0 and is not
    // fresh: nobody has said where its players are now.
    let sighting = world.kept(2).sighting.clone().unwrap();
    assert_eq!((sighting.owner.as_str(), sighting.epoch), ("a", as_epoch));
    assert_eq!((sighting.tick, sighting.taken), (0, None));
    assert!(!world.fresh(2, world.now) && !world.fresh(0, world.now));
    // A report of the part with the tick 0 says nothing new; the next one does.
    let part = world.held(2).1;
    let now = world.now;
    world.cluster.players(now, "a", &[players_of(part, 0, &[])]);
    assert_eq!(world.sighted(2).unwrap(), [((4, 1), 1), ((5, 0), 1)]);
    world
        .cluster
        .players(now, "a", &[players_of(part, 1, &[((5, 0), 3)])]);
    assert_eq!(world.sighted(2).unwrap(), [((5, 0), 3)]);
    assert!(world.fresh(2, world.now));
}

#[test]
fn crowds_stay_where_they_were_heard_when_a_split_ends_without_a_part() {
    let mut world = World::new(&[4], &["a"]);
    world.put(0, &[((0, 0), 2), ((5, 0), 1)]);
    world.look();
    world
        .cluster
        .split(world.now, 0, &around(&[(5, 0)]), None)
        .unwrap();
    world.split_ends(0, Err(Off::Nobody));
    assert_eq!(world.sighted(0).unwrap(), [((0, 0), 2), ((5, 0), 1)]);
    assert_eq!(world.cluster.coordinator.regions.len(), 2);
}

#[test]
fn nothing_is_kept_for_a_region_that_is_gone() {
    let mut world = World::unpinned(&[4, 8], &["a"]);
    // Region 1 and region 2 are near each other and far apart in themselves: a
    // merge of the two is wanted, and a split of each.
    world.put(0, &[((0, 0), 1)]);
    world.put(1, &[((100, 0), 2), ((110, 0), 1)]);
    world.put(2, &[((102, 0), 2), ((120, 0), 1)]);
    world.look();
    let noted = &world.cluster.coordinator.noted;
    assert_eq!(
        noted.merges.keys().collect::<Vec<_>>(),
        [&(RegionId(1), RegionId(2))]
    );
    assert_eq!(
        noted.going.keys().collect::<Vec<_>>(),
        [&RegionId(1), &RegionId(2)]
    );

    // The list has region 2 no longer, and does not say what became of it.
    world.list.regions.retain(|info| info.region.0 != 2);
    world.read();
    let coordinator = &world.cluster.coordinator;
    assert!(!coordinator.regions.contains_key(&RegionId(2)));
    assert!(coordinator.noted.merges.is_empty());
    assert_eq!(
        coordinator.noted.going.keys().collect::<Vec<_>>(),
        [&RegionId(1)]
    );
    assert_eq!(coordinator.alone_until(RegionId(2)), None);
    // And the groups of a region are those of the last tick, however many ticks
    // there were and wherever its players went.
    for step in 0..40 {
        world.put(1, &[((100, 0), 2), ((110 + 7 * step, 0), 1)]);
        world.look();
    }
    let going: &Vec<Going> = &world.cluster.coordinator.noted.going[&RegionId(1)];
    assert_eq!(going.len(), 1);
}

// What comes of a merge and of a split, whoever asked: sections 5.4 and 5.5.

#[test]
fn a_merge_that_ends_well_rests_the_survivor_and_what_is_asked_by_hand_is_done_at_rest() {
    let mut world = World::new(&[4, 8], &["a"]);
    world.put(1, &[((100, 0), 1)]);
    world.put(2, &[((200, 0), 1)]);
    world.look();
    // K17: asked by hand, and done although the regions rest.
    world.cluster.merge(world.now, 1, 2, Some(7)).unwrap();
    world.now += 400;
    world.merge_ends(1, 2);
    assert_eq!(world.alone_until(1), Some(world.now + REST));
    world.cluster.merge(world.now, 0, 1, Some(7)).unwrap();
    world.now += 400;
    world.merge_ends(0, 1);
    assert_eq!(world.alone_until(0), Some(world.now + REST));
    assert!(!world.kept(0).split_last);
}

#[test]
fn every_merge_that_comes_to_nothing_leaves_both_regions_alone_for_twice_as_long_as_the_last() {
    let mut world = World::new(&[4, 8], &["a"]);
    world.put(1, &[((100, 0), 1)]);
    world.put(2, &[((200, 0), 1)]);
    world.look();
    // By hand, so that the next attempt does not wait for the regions to be left
    // alone no longer: `LONG`, twice that, four times, eight times, and no more.
    for long in [LONG, 2 * LONG, 4 * LONG, 8 * LONG, 8 * LONG] {
        world.now += 1000;
        world.cluster.merge(world.now, 1, 2, Some(7)).unwrap();
        let ended = world.merge_fails(1, 2, Off::TooLarge);
        let why = Err(Undone::Off(Off::TooLarge));
        assert_eq!(ended.reshaped, [merged(Some(7), 1, 2, why)]);
        assert_eq!(world.alone_until(1), Some(world.now + long), "{long}");
        assert_eq!(world.alone_until(2), Some(world.now + long), "{long}");
    }
    assert_eq!((world.kept(1).failures, world.kept(2).failures), (5, 5));

    // A merge that ends well clears the count of its survivor, whose next failure
    // costs `LONG` again; the other region of that one has its own count.
    world.now += 8 * LONG;
    world.cluster.merge(world.now, 1, 2, Some(7)).unwrap();
    world.merge_ends(1, 2);
    assert_eq!(world.kept(1).failures, 0);
    world.now += REST;
    world.cluster.merge(world.now, 0, 1, Some(7)).unwrap();
    world.merge_fails(0, 1, Off::Unreadable);
    assert_eq!(world.alone_until(0), Some(world.now + LONG));
    assert_eq!(world.alone_until(1), Some(world.now + LONG));
}

#[test]
fn a_merge_that_ends_without_its_worker_s_word_leaves_the_regions_that_are_still_there_alone() {
    let mut world = World::new(&[4, 8], &["a", "b"]);
    world.look();
    // The owner of the region to absorb does not release it within the lease.
    let asked = world.now;
    world.cluster.merge(asked, 0, 1, Some(7)).unwrap();
    let ended = loop {
        world.look();
        if !world.said.reshaped.is_empty() {
            break world.now;
        }
    };
    assert_eq!(
        world.said.reshaped,
        [merged(Some(7), 0, 1, Err(Undone::NotReleased))]
    );
    assert!(ended > asked + LEASE);
    assert_eq!(world.alone_until(0), Some(ended + LONG));
    assert_eq!(world.alone_until(1), Some(ended + LONG));

    // A reading takes the survivor of a merge away: the other region is left
    // alone, and nothing is kept of the one that is gone.
    world.cluster.merge(world.now, 0, 2, Some(8)).unwrap();
    world.list.regions.retain(|info| info.region.0 != 0);
    world.list.home = RegionId(1);
    let ended = world.read();
    let why = Err(Undone::Gone(RegionId(0)));
    assert_eq!(ended.reshaped, [merged(Some(8), 0, 2, why)]);
    assert_eq!(world.alone_until(2), Some(world.now + LONG));
    assert_eq!(world.cluster.coordinator.alone_until(RegionId(0)), None);
}

#[test]
fn a_split_that_ends_well_rests_the_region_and_its_part_and_was_the_region_s_turn() {
    let mut world = World::new(&[4], &["a"]);
    world.put(0, &[((0, 0), 2), ((5, 0), 1)]);
    world.look();
    world
        .cluster
        .split(world.now, 0, &around(&[(5, 0)]), Some(7))
        .unwrap();
    world.now += 300;
    world.split_ends(0, Ok(2));
    assert_eq!(world.alone_until(0), Some(world.now + REST));
    // The part rests as a region that is given an owner, and was not split last:
    // the split was of the region it left.
    assert_eq!(world.alone_until(2), Some(world.now + REST));
    assert!(world.kept(0).split_last && !world.kept(2).split_last);
    assert_eq!((world.kept(0).failures, world.kept(0).not_yet), (0, 0));
}

#[test]
fn a_split_that_was_not_yet_rests_the_region_and_the_third_in_a_row_is_a_failure() {
    let mut world = World::new(&[4], &["a"]);
    world.put(0, &[((0, 0), 2), ((5, 0), 1)]);
    world.look();
    let answers = [
        Off::Nobody,
        Off::NothingStays,
        Off::Busy,
        Off::NotRunning,
        Off::Declined(Decline::NotNext { next: RegionId(9) }),
        Off::Nobody,
    ];
    let chunks = around(&[(5, 0)]);
    for (count, why) in answers.into_iter().enumerate() {
        world.now += 2 * LONG;
        world.cluster.split(world.now, 0, &chunks, Some(7)).unwrap();
        world.split_ends(0, Err(why));
        // The third and the sixth: left alone as after a failure, the second of
        // them for twice as long, and the count of such answers begins anew.
        let alone = match count {
            2 => LONG,
            5 => 2 * LONG,
            _ => REST,
        };
        assert_eq!(world.alone_until(0), Some(world.now + alone), "{why:?}");
        assert!(world.kept(0).split_last);
    }
    assert_eq!((world.kept(0).failures, world.kept(0).not_yet), (2, 0));
    // Only an attempt that ends well clears the counts.
    world.now += 2 * LONG;
    world.cluster.split(world.now, 0, &chunks, Some(7)).unwrap();
    world.split_ends(0, Ok(2));
    assert_eq!((world.kept(0).failures, world.kept(0).not_yet), (0, 0));
}

#[test]
fn a_split_that_comes_to_nothing_otherwise_leaves_the_region_alone_and_was_its_turn_too() {
    let mut world = World::new(&[4], &["a"]);
    world.put(0, &[((0, 0), 2), ((5, 0), 1)]);
    world.look();
    let chunks = around(&[(5, 0)]);
    let answers = [
        Off::TooLarge,
        Off::StoreLost,
        Off::Declined(Decline::Malformed),
    ];
    for (count, why) in answers.into_iter().enumerate() {
        world.now += 8 * LONG;
        world.cluster.split(world.now, 0, &chunks, Some(7)).unwrap();
        world.split_ends(0, Err(why));
        let long = LONG << count;
        assert_eq!(world.alone_until(0), Some(world.now + long), "{why:?}");
        assert!(world.kept(0).split_last);
    }
    // And one whose worker never says what came of it.
    let asked = world.now + 8 * LONG;
    world.now = asked;
    world.cluster.heartbeat(asked, "a");
    world.cluster.split(asked, 0, &chunks, Some(7)).unwrap();
    let ended = loop {
        world.look();
        if !world.said.reshaped.is_empty() {
            break world.now;
        }
    };
    assert_eq!(
        world.said.reshaped,
        [was_split(Some(7), 0, Err(Undone::Overdue))]
    );
    assert_eq!(world.alone_until(0), Some(ended + 8 * LONG));
}

// The list on a timer: section 7.

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

// Merges by the distances: sections 4.2 and 5.3.

#[test]
fn a_merge_is_begun_when_it_has_been_wanted_for_longer_than_a_second() {
    let mut world = World::new(&[4], &["a"]);
    world.put(0, &[((0, 0), 1)]);
    world.put(1, &[((2, 0), 1)]);
    // Wanted from the look at `READY + 250`, and nothing is begun on one look, nor
    // when it has been wanted for exactly a second.
    world.quiet_until(READY + 1250);
    assert!(world.said.orders.is_empty());
    assert_eq!(world.look(), [merge_of(0, 1)]);
    // What a merge that somebody asked for has said: the one region is to be
    // released, and the survivor's owner is to prepare.
    assert_eq!(world.said.releases, [order("a", 1, E + 2)]);
    assert_eq!(world.said.orders, [prepare("a", 0, E + 1)]);
    assert!(!world.cluster.coordinator.merges[&RegionId(1)].absorption);
    assert!(world.said.reshaped.is_empty());

    // It ends like one that somebody asked for, with nobody as asker.
    let ended = world.merge_ends(0, 1);
    assert_eq!(ended.reshaped, [merged(None, 0, 1, Ok(0))]);
}

#[test]
fn a_look_that_does_not_want_a_merge_begins_its_second_anew() {
    let mut world = World::new(&[4], &["a"]);
    world.put(0, &[((0, 0), 1)]);
    world.put(1, &[((2, 0), 1)]);
    world.quiet_until(READY + 1000);
    // One report has the player a chunk further off, and the next has them back.
    world.put(1, &[((3, 0), 1)]);
    assert!(world.look().is_empty());
    world.put(1, &[((2, 0), 1)]);
    // Wanted again from `READY + 1500`, and begun when that has stood.
    world.quiet_until(READY + 2500);
    assert_eq!(world.look(), [merge_of(0, 1)]);
}

#[test]
fn a_merge_is_kept_by_its_two_regions_and_its_survivor_is_chosen_when_it_is_begun() {
    let mut world = World::new(&[4, 8], &["a"]);
    world.put(1, &[((100, 0), 2)]);
    world.put(2, &[((102, 0), 1)]);
    world.quiet_until(READY + 1000);
    // Somebody joins the region that would have been absorbed. The merge has been
    // wanted all along, whichever of the two would have survived.
    world.put(2, &[((102, 0), 3)]);
    world.quiet_until(READY + 1250);
    assert_eq!(world.look(), [merge_of(2, 1)]);
}

#[test]
fn a_player_who_is_in_two_sightings_for_one_report_merges_nothing() {
    // K10: a player walks from region 1 into a chunk that region 2 holds, and for
    // one report is in the older sighting of the one and the newer of the other.
    let mut world = World::new(&[4, 8], &["a"]);
    world.put(1, &[((100, 0), 1), ((104, 0), 1)]);
    world.put(2, &[((110, 0), 1)]);
    world.quiet_until(READY + 2000);
    world.put(2, &[((110, 0), 1), ((105, 0), 1)]);
    assert!(world.look().is_empty());
    let wanted = &world.cluster.coordinator.noted.merges;
    assert!(wanted[&(RegionId(1), RegionId(2))].since.is_some());
    world.put(1, &[((100, 0), 1)]);
    world.quiet_until(READY + 6000);
}

/// Region 2 has three players at (100, 0) and rests from now. Regions 1, 3 and 4
/// have nobody yet; one player each is put near region 2 by the tests.
fn one_region_that_rests_among_three() -> World {
    let mut world = World::new(&[4, 8, 12, 16], &["a"]);
    world.put(2, &[((100, 0), 3)]);
    world.look();
    world.rests_from_now(2);
    world
}

#[test]
fn of_two_merges_with_a_region_the_one_that_was_wanted_first_is_begun_first() {
    // K1. Region 1 comes within the merge distance of region 2, and two seconds
    // later region 3 comes nearer still. Region 2 rests meanwhile.
    let mut world = one_region_that_rests_among_three();
    let rested = world.now + REST;
    world.put(1, &[((102, 0), 1)]);
    world.quiet_until(rested - REST + 2000);
    world.put(3, &[((99, 0), 1)]);
    world.quiet_until(rested - LOOK);
    // Both have stood long since. The first to be wanted is begun, although the
    // other has the smaller gap, and the other is passed over: region 2 is in
    // something begun at this tick.
    assert_eq!(world.look(), [merge_of(2, 1)]);
    assert_eq!(world.now, rested);

    // While the merge lasts, a further region comes nearer than either.
    world.put(4, &[((103, 0), 1)]);
    world.quiet_until(rested + 1000);
    world.merge_ends(2, 1);
    let rested = world.now + REST;
    assert_eq!(world.alone_until(2), Some(rested));
    // The merge that has waited since before is begun when the survivor has
    // rested, before the one that came to want the survivor later, however near.
    world.quiet_until(rested - LOOK);
    assert_eq!(world.look(), [merge_of(2, 3)]);
    world.merge_ends(2, 3);
    let rested = world.now + REST;
    world.quiet_until(rested - LOOK);
    assert_eq!(world.look(), [merge_of(2, 4)]);
}

#[test]
fn of_two_merges_that_have_waited_since_the_same_tick_the_nearer_is_begun_first() {
    let mut world = one_region_that_rests_among_three();
    let rested = world.now + REST;
    world.put(3, &[((102, 0), 1)]);
    world.put(1, &[((99, 0), 1)]);
    world.quiet_until(rested - LOOK);
    assert_eq!(world.look(), [merge_of(2, 1)]);
}

#[test]
fn a_merge_goes_with_the_region_that_took_the_one_it_waited_for() {
    // K1, where the region that both wait for is the one absorbed: region 2 has
    // one player, regions 1 and 3 two each. What region 3 waited for with region 2
    // it waits for with region 1.
    let mut world = World::new(&[4, 8, 12, 16], &["a"]);
    world.put(2, &[((100, 0), 1)]);
    world.look();
    world.rests_from_now(2);
    let rested = world.now + REST;
    world.put(1, &[((102, 0), 2)]);
    world.quiet_until(rested - REST + 2000);
    world.put(3, &[((98, 0), 2)]);
    world.quiet_until(rested - LOOK);
    assert_eq!(world.look(), [merge_of(1, 2)]);
    let waited = world.cluster.coordinator.noted.merges[&(RegionId(2), RegionId(3))];
    world.put(4, &[((103, 0), 1)]);
    world.quiet_until(rested + 500);
    world.merge_ends(1, 2);
    let merges = &world.cluster.coordinator.noted.merges;
    assert_eq!(merges[&(RegionId(1), RegionId(3))].waiting, waited.waiting);
    assert!(
        !merges
            .keys()
            .any(|(lower, higher)| lower.0 == 2 || higher.0 == 2)
    );

    let rested = world.now + REST;
    world.quiet_until(rested - LOOK);
    assert_eq!(world.look(), [merge_of(1, 3)]);
}

#[test]
fn a_merge_keeps_its_place_through_a_look_that_misleads_and_loses_it_after_a_second() {
    for (away, first) in [(LOOK, 1), (1250, 1), (1500, 3)] {
        let mut world = one_region_that_rests_among_three();
        let rested = world.now + REST;
        world.put(1, &[((102, 0), 1)]);
        world.quiet_until(rested - REST + 2000);
        world.put(3, &[((99, 0), 1)]);
        world.quiet_until(rested - REST + 4000);
        // The reports of region 1 have its player far off for a while, and then
        // back: at one look, at five, which are a second from the first to the
        // last, or at six. Both its sighting and region 2's are fresh all the
        // while.
        world.put(1, &[((150, 0), 1)]);
        world.quiet_until(rested - REST + 4000 + away);
        world.put(1, &[((102, 0), 1)]);
        world.quiet_until(rested - LOOK);
        // For a second or less the merge keeps its place before the one that came
        // later; when it has not been wanted for more than a second, it is the one
        // that came later.
        assert_eq!(world.look(), [merge_of(2, first)], "away for {away}");
    }
}

#[test]
fn nothing_more_is_wanted_of_two_regions_when_the_list_shows_the_one_absorbed_by_the_other() {
    let mut world = one_region_that_rests_among_three();
    world.put(1, &[((102, 0), 1)]);
    world.quiet_until(world.now + 3000);
    assert_eq!(world.cluster.coordinator.noted.merges.len(), 1);
    // Somebody else merged them, and the list says so.
    world.crowds.remove(&1);
    world.put(2, &[((100, 0), 3), ((102, 0), 1)]);
    world.list.regions.retain(|info| info.region.0 != 1);
    world.list.absorbed.push((RegionId(1), RegionId(2)));
    world.read();
    // A merge of a region with itself is forgotten.
    assert!(world.cluster.coordinator.noted.merges.is_empty());
    world.quiet_until(world.now + 2 * REST);
}

#[test]
fn four_merges_are_under_way_at_a_time_and_the_others_are_begun_as_those_end() {
    // K9: ten pairs of regions, each pair far from every other.
    let boundaries: Vec<i32> = (1..=20).map(|region| 4 * region).collect();
    let mut world = World::new(&boundaries, &["a"]);
    for pair in 0..10_u32 {
        let x = 100 * (i32::try_from(pair).unwrap() + 1);
        world.put(2 * pair + 1, &[((x, 0), 1)]);
        world.put(2 * pair + 2, &[((x + 2, 0), 1)]);
    }
    world.quiet_until(READY + 1250);
    // Of merges that have waited since the same tick with the same gap, the lower
    // regions first.
    let first = [
        merge_of(1, 2),
        merge_of(3, 4),
        merge_of(5, 6),
        merge_of(7, 8),
    ];
    assert_eq!(world.look(), first);
    assert!(world.look().is_empty());
    world.merge_ends(3, 4);
    assert_eq!(world.look(), [merge_of(9, 10)]);
    world.merge_ends(1, 2);
    world.merge_ends(9, 10);
    assert_eq!(world.look(), [merge_of(11, 12), merge_of(13, 14)]);
    // One that somebody asked for counts among the four.
    world.merge_ends(5, 6);
    world.cluster.split(world.now, 0, &CHUNKS, Some(7)).unwrap();
    assert!(world.look().is_empty());
    world.merge_ends(7, 8);
    assert_eq!(world.look(), [merge_of(15, 16)]);
}

// What has to hold of the world and of a region: sections 5.1 and 5.2.

/// A world of three regions in which a merge of region 2 into region 1 is wanted
/// from the first look after [`READY`]: region 1 has two players and region 2 one,
/// two chunks from them and far from the chunk players enter in.
fn two_regions_near_each_other(workers: &[&str]) -> World {
    let mut world = World::new(&[4, 8], workers);
    world.put(1, &[((100, 0), 2)]);
    world.put(2, &[((102, 0), 1)]);
    world
}

#[test]
fn nothing_is_begun_with_a_region_that_is_reserved_or_being_released() {
    // Region 0 is `a`'s, region 1 is `b`'s and region 2 is `a`'s.
    let mut world = two_regions_near_each_other(&["a", "b"]);
    assert_eq!(
        (world.held(1).0.as_str(), world.held(2).0.as_str()),
        ("b", "a")
    );
    // Somebody has region 2 split, and the split takes its time.
    world.cluster.split(READY, 2, &CHUNKS, Some(7)).unwrap();
    world.quiet_until(READY + 5000);
    world.split_ends(2, Err(Off::Nobody));
    // It rests then, and the merge has to stand anew: a reserved region's sighting
    // is not fresh, so the merge was not wanted meanwhile.
    world.quiet_until(READY + 5000 + REST - LOOK);
    assert_eq!(world.look(), [merge_of(1, 2)]);
    world.merge_ends(1, 2);

    // Somebody has a region moved, and its owner takes its time.
    let mut world = two_regions_near_each_other(&["a", "b"]);
    world.cluster.move_region(READY, 2, Some("b"), 9).unwrap();
    world.quiet_until(READY + 5000);
    let epoch = world.held(2).1.epoch;
    world.cluster.released(world.now, "a", 2, epoch);
    // K3: the merge waits for the move and then for the rest of the new owner.
    assert_eq!(world.held(2).0, "b");
    world.quiet_until(READY + 5000 + REST - LOOK);
    assert_eq!(world.look(), [merge_of(1, 2)]);
}

#[test]
fn nothing_is_begun_with_a_region_whose_owner_has_no_connection_or_is_leaving() {
    let mut world = two_regions_near_each_other(&["a", "b"]);
    // The owner of region 1 loses its connection. What it said last is fresh for a
    // second, and if its reports still came, nothing would be begun with its
    // region either: it could not be told.
    world.cluster.disconnected(READY, "b");
    world.quiet_until(READY + 5000);
    let held = world.held(1).1;
    world.cluster.register(world.now, "b", "b:25601", &[held]);
    // Nothing has happened to the region, so it does not rest: the merge has stood
    // all along, and is begun at the next look.
    assert_eq!(world.look(), [merge_of(1, 2)]);

    // A worker that leaves: none of its regions takes part in anything from the
    // moment it says so. Nobody else is there to take them.
    let mut world = two_regions_near_each_other(&["a"]);
    world.cluster.leaving(READY, "a");
    world.quiet_until(READY + 5000);
    assert!(world.said.orders.is_empty());
    // It registers again, and is not leaving any more.
    let holding = world.cluster.assignments("a");
    world.cluster.register(world.now, "a", "a:25601", &holding);
    assert_eq!(world.look(), [merge_of(1, 2)]);
}

#[test]
fn a_leaving_worker_s_regions_are_released_whether_they_rest_or_not() {
    // K16.
    let mut world = two_regions_near_each_other(&["a", "b"]);
    world.rests_from_now(1);
    let leaving = world.cluster.leaving(READY, "b");
    assert_eq!(leaving.releases.len(), 1);
    assert_eq!(leaving.releases[0].region, RegionId(1));
    // Released, and given to the other worker: from then it rests, and takes part
    // again when its new owner has reported and it has rested.
    let epoch = world.held(1).1.epoch;
    world.cluster.released(READY, "b", 1, epoch);
    assert_eq!(world.held(1).0, "a");
    world.quiet_until(READY + REST - LOOK);
    assert_eq!(world.look(), [merge_of(1, 2)]);
}

#[test]
fn nothing_is_begun_with_a_region_whose_owner_is_at_fault() {
    // `a` has the regions 0, 2 and 4, `b` the regions 1 and 3. `b` is asked to
    // release region 3 and does not answer within the lease: it is at fault.
    let mut world = World::new(&[4, 8, 12, 16], &["a", "b"]);
    world.put(1, &[((100, 0), 2)]);
    world.put(2, &[((102, 0), 1)]);
    world.cluster.move_region(READY, 3, Some("a"), 9).unwrap();
    while world.held(3).0 == "b" {
        let begun = world.look();
        // The merge has stood before the fault. It is not begun while a release
        // is under way... and then the owner of region 1 is at fault.
        if !begun.is_empty() {
            world.merge_fails(1, 2, Off::Busy);
        }
    }
    let failed = world.now;
    assert!(failed > READY + LEASE);
    let memory = u64::from(Coordinator::FAULT_MEMORY) * LEASE;
    // Evening out keeps away from a worker at fault as well, so nothing happens at
    // all until the fault is forgotten.
    let alone = world
        .alone_until(1)
        .unwrap()
        .max(world.alone_until(2).unwrap());
    assert!(alone < failed + memory);
    world.quiet_until(failed + memory - LOOK);
    assert_eq!(world.look(), [merge_of(1, 2)]);
}

#[test]
fn nothing_is_begun_with_a_region_whose_sighting_is_more_than_a_second_old() {
    let mut world = two_regions_near_each_other(&["a"]);
    world.quiet_until(READY + 500);
    // The worker says nothing of region 2 any more. Its last report was at `READY +
    // 500`; the merge was wanted from `READY + 250` and stops being wanted when that
    // report is more than a second old, at `READY + 1750`. It had stood by then,
    // and would have been begun at `READY + 1500` with a report a second old.
    world.silent.insert(2);
    world.quiet_until(READY + 1250);
    assert_eq!(world.look(), [merge_of(1, 2)]);

    // With the last report a look earlier, it has not stood before the sighting
    // stops being fresh, and nothing is begun however long the region is silent.
    let mut world = two_regions_near_each_other(&["a"]);
    world.quiet_until(READY + 250);
    world.silent.insert(2);
    world.quiet_until(READY + 9000);
    assert!(
        world
            .cluster
            .coordinator
            .noted
            .merges
            .values()
            .all(|waited| waited.since.is_none())
    );
    world.silent.clear();
    world.quiet_until(READY + 9000 + 1250);
    assert_eq!(world.look(), [merge_of(1, 2)]);
}

#[test]
fn nothing_is_begun_while_a_region_the_coordinator_knows_has_never_been_reported() {
    // K23. Region 0 is far from everything that is wanted, and no worker has said
    // a word of it.
    let mut world = World::begin(&[4, 8], &["a"]);
    world.silent.insert(0);
    world.put(1, &[((100, 0), 2)]);
    world.put(2, &[((102, 0), 1)]);
    world.quiet_until(READY + 5000);
    world.silent.clear();
    assert_eq!(world.look(), [merge_of(1, 2)]);
}

#[test]
fn a_region_that_was_reported_once_and_is_silent_holds_back_only_what_its_players_could() {
    // K23, the other half: with an old sighting its players count where they were,
    // and everything else goes on.
    let mut world = World::begin(&[4, 8, 12], &["a"]);
    world.put(3, &[((300, 0), 1)]);
    world.quiet_until(READY);
    world.silent.insert(3);
    world.put(1, &[((100, 0), 2)]);
    world.put(2, &[((102, 0), 1)]);
    world.quiet_until(READY + 1250);
    assert_eq!(world.look(), [merge_of(1, 2)]);
}

#[test]
fn a_new_coordinator_begins_nothing_for_a_lease_nor_with_a_region_that_has_not_rested() {
    // K6. The workers kept running while there was no coordinator, and register
    // with what they run a moment after the new one was made.
    let mut world = World::begin(&[4, 8], &[]);
    let holding = [
        assignment(0, 7, 0),
        assignment(1, 8, 1),
        assignment(2, 9, 2),
    ];
    world.cluster.register(500, "a", "a:25601", &holding);
    assert_eq!(world.alone_until(1), Some(500 + REST));
    world.put(1, &[((100, 0), 2)]);
    world.put(2, &[((102, 0), 1)]);
    // The merge stands from the first reports on. The grace period ends at
    // `LEASE`, and the regions rest until half a second after it.
    world.quiet_until(LEASE + 250);
    assert_eq!(world.look(), [merge_of(1, 2)]);
    assert_eq!(world.now, 500 + REST);

    // With a rest shorter than the lease it is the grace period that is waited for.
    let short = Policy {
        rest: Duration::from_secs(2),
        ..small()
    };
    let mut world = World::begin_by(short, &[4, 8], &[]);
    world.cluster.register(500, "a", "a:25601", &holding);
    world.put(1, &[((100, 0), 2)]);
    world.put(2, &[((102, 0), 1)]);
    world.quiet_until(LEASE - LOOK);
    assert_eq!(world.look(), [merge_of(1, 2)]);
}

#[test]
fn nothing_is_wanted_before_the_list_has_been_read() {
    let mut cluster = following(&[4, 8]);
    let holding = [
        assignment(0, 7, 0),
        assignment(1, 8, 1),
        assignment(2, 9, 2),
    ];
    cluster.register(0, "a", "a:25601", &holding);
    let mut world = World {
        cluster,
        now: 0,
        list: stripes(3),
        store: Store::Silent,
        crowds: BTreeMap::new(),
        tick: 0,
        silent: BTreeSet::new(),
        dead: BTreeSet::new(),
        said: Changes::default(),
    };
    world.put(1, &[((100, 0), 2)]);
    world.put(2, &[((102, 0), 1)]);
    // Without the list the coordinator does not know which region is home, and
    // wants nothing: nothing has stood either when the list is read at last.
    world.quiet_until(3 * LEASE);
    assert!(world.cluster.coordinator.noted.merges.is_empty());
    world.read();
    world.quiet_until(3 * LEASE + 1250);
    assert_eq!(world.look(), [merge_of(1, 2)]);
}

#[test]
fn nothing_is_begun_when_the_last_reading_that_succeeded_is_more_than_two_leases_old() {
    // K8. Readings are answered at the tick that asks for them; from `READY` on
    // they fail. The last one that succeeded was at `READY`.
    let mut world = World::new(&[4, 8], &["a"]);
    assert_eq!(
        world.cluster.coordinator.noted.listed,
        Some(world.cluster.at(READY))
    );
    world.store = Store::Fails;
    // One reading that fails, at `READY + LEASE`, holds nothing back: the next one
    // is asked for when the last good one is exactly two leases old.
    world.quiet_until(READY + 2 * LEASE - 1500);
    world.put(1, &[((100, 0), 2)]);
    world.put(2, &[((102, 0), 1)]);
    world.quiet_until(READY + 2 * LEASE - LOOK);
    assert_eq!(world.look(), [merge_of(1, 2)]);

    // Not a moment later: the same with the merge wanted a look later, so that it
    // has stood when the last good reading is two leases and a look old. That took
    // two readings that failed.
    let mut world = World::new(&[4, 8], &["a"]);
    world.store = Store::Fails;
    world.quiet_until(READY + 2 * LEASE - 1250);
    world.put(1, &[((100, 0), 2)]);
    world.put(2, &[((102, 0), 1)]);
    world.quiet_until(READY + 3 * LEASE - LOOK);
    // The third reading succeeds, at `READY + 3 * LEASE`, and what has stood is
    // begun at the tick after it.
    world.store = Store::Answers;
    assert!(world.look().is_empty());
    assert!(world.said.read);
    assert_eq!(world.look(), [merge_of(1, 2)]);
}

#[test]
fn a_reading_that_fails_late_holds_back_for_as_long_as_it_and_the_next_one_take() {
    // Section 7. The store answers a second after it is asked: the reading that is
    // asked for at `READY + LEASE` fails at `READY + LEASE + 1000`, and the next
    // one, asked for a lease after that failure, succeeds a second later.
    let mut world = World::new(&[4, 8], &["a"]);
    world.store = Store::Silent;
    world.quiet_until(READY + LEASE);
    assert!(world.said.read);
    world.quiet_until(READY + LEASE + 1000);
    world.cluster.unlisted(world.now);
    world.quiet_until(READY + 2 * LEASE);
    // A merge comes to have stood after the last good reading is two leases old.
    world.put(1, &[((100, 0), 2)]);
    world.put(2, &[((102, 0), 1)]);
    world.quiet_until(READY + 2 * LEASE + 1000);
    assert!(world.said.read);
    world.quiet_until(READY + 2 * LEASE + 2000);
    world.read();
    assert_eq!(world.look(), [merge_of(1, 2)]);
}

// Splits: sections 4.3, 5.3 and 5.6.

#[test]
fn a_region_is_told_to_prepare_when_a_group_parts_and_is_split_when_the_group_has_stood() {
    let mut world = World::new(&[4], &["a"]);
    world.put(0, &[((0, 0), 2), ((6, 0), 1)]);
    // The first look that shows it: `Prepare`, and nothing is begun on one look.
    assert!(world.look().is_empty());
    assert_eq!(world.said.orders, [prepare("a", 0, E + 1)]);
    while world.now < READY + 1250 {
        assert!(world.look().is_empty());
        assert!(world.said.orders.is_empty(), "at {}", world.now);
    }
    // A second and a look later: the chunks within the margin of the group, and the
    // next id of the list.
    assert_eq!(world.look(), [split_of(0)]);
    let chunks = around(&[(6, 0)]);
    assert_eq!(chunks.len(), 25);
    let order = Order::SplitOff {
        region: RegionId(0),
        epoch: E + 1,
        chunks,
        as_epoch: E + 3,
        part: RegionId(2),
    };
    let worker = "a".to_owned();
    assert_eq!(world.said.orders, [ReshapeOrder { worker, order }]);
    assert!(world.said.releases.is_empty() && world.said.reshaped.is_empty());

    // It ends like one that somebody asked for, with nobody as asker.
    let ended = world.split_ends(0, Ok(2));
    assert_eq!(ended.reshaped, [was_split(None, 0, Ok(2))]);
}

#[test]
fn one_split_is_under_way_in_the_whole_world_whoever_asked_for_it() {
    let mut world = World::new(&[4, 8], &["a"]);
    world.put(1, &[((100, 0), 2), ((110, 0), 1)]);
    world.put(2, &[((200, 0), 2), ((210, 0), 1)]);
    world.quiet_until(READY + 1250);
    // Both groups have gone since the same tick: the lower region.
    assert_eq!(world.look(), [split_of(1)]);
    world.quiet_until(READY + 3500);
    world.split_ends(1, Ok(3));
    world.put(1, &[((100, 0), 2)]);
    world.put(3, &[((110, 0), 1)]);
    // The reading that follows the split is in, and names the next id.
    assert_eq!(world.look(), [split_of(2)]);
    assert_eq!(world.split_off().unwrap().2, RegionId(4));

    // Beside one that somebody asked for, of another region.
    let mut world = World::new(&[4, 8], &["a"]);
    world.put(1, &[((100, 0), 2), ((110, 0), 1)]);
    world.cluster.split(READY, 2, &CHUNKS, Some(7)).unwrap();
    world.quiet_until(READY + 4000);
    world.split_ends(2, Err(Off::Nobody));
    assert_eq!(world.look(), [split_of(1)]);
}

#[test]
fn no_split_is_begun_while_a_reading_of_the_list_is_asked_for_or_owed() {
    let mut world = World::new(&[4, 8, 12], &["a"]);
    world.put(1, &[((100, 0), 2), ((110, 0), 1)]);
    world.put(2, &[((200, 0), 2), ((210, 0), 1)]);
    world.put(3, &[((300, 0), 2), ((310, 0), 1)]);
    world.quiet_until(READY + 1250);
    assert_eq!(world.look(), [split_of(1)]);
    assert_eq!(world.split_off().unwrap().2, RegionId(4));

    // The worker says that the split is made, and the reading that follows it is
    // not answered for a while: no split is begun before it is in.
    world.store = Store::Silent;
    let as_epoch = world.cluster.coordinator.splits[&RegionId(1)].as_epoch;
    world
        .cluster
        .split_ended(world.now, "a", 1, as_epoch, Ok(4));
    world.put(1, &[((100, 0), 2)]);
    world.put(4, &[((110, 0), 1)]);
    world.quiet_until(READY + 3500);
    // It fails. Nothing is owed after a split that was made, so the next split is
    // begun, and names the id last read, which is taken: the runner's second try
    // is what that rests on.
    world.cluster.unlisted(world.now);
    assert_eq!(world.look(), [split_of(2)]);
    assert_eq!(world.split_off().unwrap().2, RegionId(4));

    // This one ends without the worker's word that it was made: the part may be a
    // region that nobody runs, and no split is begun before a reading has shown.
    world.store = Store::Fails;
    let second = world.cluster.coordinator.splits[&RegionId(2)].as_epoch;
    let lost = Err(Off::StoreLost);
    world.cluster.split_ended(world.now, "a", 2, second, lost);
    world.cluster.unlisted(world.now);
    assert!(world.cluster.coordinator.owed);
    world.quiet_until(READY + 6000);
    assert!(world.said.read);
    world.store = Store::Answers;
    world.list.regions.push(living(4, as_epoch));
    world.list.next = RegionId(5);
    assert!(world.look().is_empty());
    assert_eq!(world.look(), [split_of(3)]);
    assert_eq!(world.split_off().unwrap().2, RegionId(5));
}

#[test]
fn a_region_is_not_split_while_a_sighting_that_is_not_fresh_joins_its_groups() {
    // K12. Two groups of region 1, 9 apart, and two players of region 2 between
    // them, each within 2 of one group and 5 from each other.
    let mut world = World::new(&[4, 8], &["a"]);
    world.put(1, &[((100, 0), 1), ((109, 0), 1)]);
    world.put(2, &[((102, 0), 1), ((107, 0), 1)]);
    assert!(world.look().is_empty());
    // Region 2 falls silent. For a second a merge of the two is wanted, which is
    // not long enough; after that region 1 is neither surely whole nor surely
    // apart, and nothing is wanted of it: no merge, no split and no `Prepare`.
    world.silent.insert(2);
    while world.now < READY + 6000 {
        assert!(world.look().is_empty());
        assert!(world.said.orders.is_empty(), "at {}", world.now);
    }
    assert!(world.cluster.coordinator.noted.going.is_empty());

    // Region 2 reports its players elsewhere: region 1 is surely apart.
    world.silent.clear();
    world.put(2, &[((300, 0), 2)]);
    assert!(world.look().is_empty());
    assert_eq!(world.prepared(), [1]);
    world.quiet_until(READY + 7250);
    assert_eq!(world.look(), [split_of(1)]);
}

#[test]
fn one_split_takes_every_group_that_has_stood_and_the_part_is_split_further() {
    // K9: the group with the most players stays, and both others go, as one region.
    let mut world = World::new(&[4, 8], &["a"]);
    world.put(1, &[((100, 0), 3), ((110, 0), 1), ((120, 0), 1)]);
    world.quiet_until(READY + 1250);
    assert_eq!(world.look(), [split_of(1)]);
    let (region, chunks, part) = world.split_off().unwrap();
    assert_eq!((region, part), (RegionId(1), RegionId(3)));
    assert_eq!(chunks, around(&[(110, 0), (120, 0)]));
    assert_eq!(chunks.len(), 50);

    // The crowds of both are the part's.
    world.split_ends(1, Ok(3));
    assert_eq!(world.sighted(1).unwrap(), [((100, 0), 3)]);
    assert_eq!(world.sighted(3).unwrap(), [((110, 0), 1), ((120, 0), 1)]);
    world.put(1, &[((100, 0), 3)]);
    world.put(3, &[((110, 0), 1), ((120, 0), 1)]);

    // The part is a region like any other: it is surely apart as soon as its
    // worker has reported it, and is split when it has rested. Of two groups with
    // as many players the one with the lower chunk stays. The region it left is
    // not split again.
    let rested = world.now + REST;
    world.quiet_until(rested - LOOK);
    assert_eq!(world.look(), [split_of(3)]);
    let (_, chunks, part) = world.split_off().unwrap();
    assert_eq!((chunks, part), (around(&[(120, 0)]), RegionId(4)));
    world.split_ends(3, Ok(4));
    world.put(3, &[((110, 0), 1)]);
    world.put(4, &[((120, 0), 1)]);
    world.quiet_until(world.now + 2 * REST);
}

#[test]
fn a_group_that_appears_as_the_region_becomes_free_is_not_named_with_one_that_has_stood() {
    // K22. The home region rests, and a group far east has stood for seconds.
    let mut world = World::new(&[4], &["a"]);
    world.rests_from_now(0);
    let rested = READY + REST;
    world.put(0, &[((0, 0), 2), ((20, 0), 1)]);
    world.quiet_until(rested - LOOK);
    // At the look at which the rest ends, a second group is there, far north: the
    // player who joined it to the others is in neither sighting for one report.
    world.put(0, &[((0, 0), 2), ((20, 0), 1), ((0, 20), 2)]);
    assert_eq!(world.look(), [split_of(0)]);
    assert_eq!(world.split_off().unwrap().1, around(&[(20, 0)]));

    // The group that had not stood stays, and is in the region's sighting still.
    world.split_ends(0, Ok(2));
    assert_eq!(world.sighted(0).unwrap(), [((0, 0), 2), ((0, 20), 2)]);
    assert_eq!(world.sighted(2).unwrap(), [((20, 0), 1)]);
    world.put(0, &[((0, 0), 2), ((0, 20), 2)]);
    world.put(2, &[((20, 0), 1)]);
    // If it is still apart when the region has rested, it goes then.
    let rested = world.now + REST;
    world.quiet_until(rested - LOOK);
    assert_eq!(world.look(), [split_of(0)]);
    let (_, chunks, part) = world.split_off().unwrap();
    assert_eq!((chunks, part), (around(&[(0, 20)]), RegionId(3)));
}

#[test]
fn a_group_that_moves_within_the_margin_at_every_look_keeps_its_time() {
    let mut world = World::new(&[4], &["a"]);
    // Two chunks at every look, which is the margin.
    for step in 0..5 {
        world.put(0, &[((0, 0), 2), ((10 + 2 * step, 0), 1)]);
        assert!(world.look().is_empty(), "step {step}");
    }
    world.put(0, &[((0, 0), 2), ((20, 0), 1)]);
    assert_eq!(world.look(), [split_of(0)]);
    assert_eq!(world.now, READY + 1500);
    // The chunks are named from the sighting of the tick that begins the split.
    assert_eq!(world.split_off().unwrap().1, around(&[(20, 0)]));

    // One that moves further than the margin at every look is a new group each
    // time, and never stands.
    let mut world = World::new(&[4], &["a"]);
    for step in 0..40 {
        world.put(0, &[((0, 0), 2), ((10 + 3 * step, 0), 1)]);
        assert!(world.look().is_empty(), "step {step}");
    }
}

#[test]
fn a_group_that_is_joined_from_further_than_the_margin_begins_its_time_anew() {
    let mut world = World::new(&[4], &["a"]);
    world.rests_from_now(0);
    let rested = READY + REST;
    world.put(0, &[((0, 0), 2), ((10, 0), 1)]);
    world.quiet_until(rested - LOOK);
    // At the look at which the split would be begun, somebody is four chunks from
    // the group: one group with it, as that is within the split distance, with a
    // chunk further than the margin from where the group was. Nothing of it is
    // named at that look.
    world.put(0, &[((0, 0), 2), ((10, 0), 1), ((14, 0), 1)]);
    world.quiet_until(rested + 1000);
    assert_eq!(world.look(), [split_of(0)]);
    assert_eq!(world.split_off().unwrap().1, around(&[(10, 0), (14, 0)]));

    // Somebody who turns up within the margin of a group that has stood is of it.
    let mut world = World::new(&[4], &["a"]);
    world.rests_from_now(0);
    world.put(0, &[((0, 0), 2), ((10, 0), 1)]);
    world.quiet_until(rested - LOOK);
    world.put(0, &[((0, 0), 2), ((10, 0), 1), ((12, 0), 1)]);
    assert_eq!(world.look(), [split_of(0)]);
    assert_eq!(world.split_off().unwrap().1, around(&[(10, 0), (12, 0)]));
}

#[test]
fn of_two_regions_to_split_the_one_whose_group_has_gone_longer_is_first() {
    let mut world = World::new(&[4, 8], &["a"]);
    world.rests_from_now(1);
    world.rests_from_now(2);
    let rested = READY + REST;
    world.put(2, &[((200, 0), 2), ((210, 0), 1)]);
    world.quiet_until(READY + 500);
    world.put(1, &[((100, 0), 2), ((110, 0), 1)]);
    world.quiet_until(rested - LOOK);
    assert_eq!(world.look(), [split_of(2)]);
}

// Turns, and what comes of a split that found nobody: sections 5.3 and 5.5.

/// The home region has two players where players enter and one ten chunks east,
/// who is a group to go, and region 1 has a player two chunks from the former: a
/// split of the home region and a merge with region 1 are both wanted. At the look
/// after this both have stood.
fn a_split_and_a_merge_of_one_region() -> World {
    let mut world = World::new(&[4, 8], &["a"]);
    world.put(0, &[((0, 0), 2), ((10, 0), 1)]);
    world.put(1, &[((0, -2), 1)]);
    world.quiet_until(READY + 1250);
    world
}

#[test]
fn a_region_that_was_split_last_is_merged_before_it_is_split_again_and_the_other_way_round() {
    // K21. A region that has been in neither is split first: splits are the
    // scarcer.
    let mut world = a_split_and_a_merge_of_one_region();
    assert_eq!(world.look(), [split_of(0)]);
    world.split_ends(0, Ok(3));
    // While it rests, another group leaves.
    world.put(0, &[((0, 0), 2), ((0, 10), 1)]);
    world.put(3, &[((10, 0), 1)]);
    let rested = world.now + REST;
    world.quiet_until(rested - LOOK);
    // It was split last, and a merge of it has stood whose regions are free: the
    // region that has waited beside it is taken in, and the group has to wait.
    assert_eq!(world.look(), [merge_of(0, 1)]);
    assert!(world.split_off().is_none());
    world.merge_ends(0, 1);

    // While it rests, a further region arrives. It was merged last: the split.
    world.put(2, &[((-2, 0), 1)]);
    let rested = world.now + REST;
    world.quiet_until(rested - LOOK);
    assert_eq!(world.look(), [split_of(0)]);
    assert_eq!(world.split_off().unwrap().1, around(&[(0, 10)]));
    world.split_ends(0, Ok(4));
    world.put(0, &[((0, 0), 2), ((0, -2), 1)]);
    world.put(4, &[((0, 10), 1)]);
    let rested = world.now + REST;
    world.quiet_until(rested - LOOK);
    assert_eq!(world.look(), [merge_of(0, 2)]);
}

#[test]
fn a_split_that_found_nobody_was_the_region_s_turn_at_splitting() {
    let mut world = a_split_and_a_merge_of_one_region();
    assert_eq!(world.look(), [split_of(0)]);
    world.split_ends(0, Err(Off::Nobody));
    // It has stopped the region for a few ticks: the region rests as after one
    // that was made, and the merge that has stood is begun then, not a second
    // split.
    let rested = world.now + REST;
    assert_eq!(world.alone_until(0), Some(rested));
    world.quiet_until(rested - LOOK);
    assert_eq!(world.look(), [merge_of(0, 1)]);
    assert!(world.split_off().is_none());
}

#[test]
fn a_merge_that_comes_to_nothing_leaves_the_turn_where_it_was() {
    let mut world = a_split_and_a_merge_of_one_region();
    assert_eq!(world.look(), [split_of(0)]);
    world.split_ends(0, Err(Off::Nobody));
    let rested = world.now + REST;
    world.quiet_until(rested - LOOK);
    assert_eq!(world.look(), [merge_of(0, 1)]);
    world.merge_fails(0, 1, Off::Busy);
    // Both regions are left alone for `LONG`, and it is the merge's turn still.
    assert!(world.kept(0).split_last);
    let alone = world.now + LONG;
    assert_eq!(world.alone_until(0), Some(alone));
    world.quiet_until(alone - LOOK);
    assert_eq!(world.look(), [merge_of(0, 1)]);
}

#[test]
fn after_a_split_that_found_nobody_the_next_one_names_the_chunks_of_the_newest_report() {
    // K5.
    let mut world = World::new(&[4], &["a"]);
    world.put(0, &[((0, 0), 2), ((10, 0), 1)]);
    world.quiet_until(READY + 1250);
    assert_eq!(world.look(), [split_of(0)]);
    assert_eq!(world.split_off().unwrap().1, around(&[(10, 0)]));
    // The player had moved on. "Not yet": the region rests and is then split with
    // what the report of that moment says.
    world.split_ends(0, Err(Off::Nobody));
    world.put(0, &[((0, 0), 2), ((14, 0), 1)]);
    let rested = world.now + REST;
    world.quiet_until(rested - LOOK);
    assert_eq!(world.look(), [split_of(0)]);
    assert_eq!(world.split_off().unwrap().1, around(&[(14, 0)]));
    world.split_ends(0, Err(Off::Nobody));
    let rested = world.now + REST;
    world.quiet_until(rested - LOOK);
    assert_eq!(world.look(), [split_of(0)]);
    // The third such answer in a row is a failure.
    world.split_ends(0, Err(Off::Nobody));
    let alone = world.now + LONG;
    world.quiet_until(alone - LOOK);
    assert_eq!(world.look(), [split_of(0)]);
}

// `Prepare` before a split: section 5.6.

#[test]
fn a_region_that_rests_is_told_to_prepare_once_within_a_second_of_the_end_of_its_rest() {
    let mut world = World::new(&[4, 8], &["a"]);
    world.rests_from_now(0);
    let rested = READY + REST;
    world.put(0, &[((0, 0), 2), ((10, 0), 1)]);
    // Not nine seconds early.
    while world.now < rested - 1250 {
        assert!(world.look().is_empty());
        assert!(world.said.orders.is_empty(), "at {}", world.now);
    }
    assert!(world.look().is_empty());
    assert_eq!(world.said.orders, [prepare("a", 0, world.held(0).1.epoch)]);

    // The split has to wait for the one split there is in the world, which
    // somebody asked for. It is not said again, however long that takes.
    world.cluster.split(world.now, 1, &CHUNKS, Some(7)).unwrap();
    while world.now < rested + 4000 {
        assert!(world.look().is_empty());
        assert!(world.said.orders.is_empty(), "at {}", world.now);
    }
    world.split_ends(1, Err(Off::Nobody));
    assert_eq!(world.look(), [split_of(0)]);

    // Said again before the split that follows: a split that begins forgets it.
    world.split_ends(0, Err(Off::Nobody));
    let rested = world.now + REST;
    while world.now < rested - 1250 {
        assert!(world.look().is_empty());
        assert!(world.said.orders.is_empty(), "at {}", world.now);
    }
    assert!(world.look().is_empty());
    assert_eq!(world.prepared(), [0]);
}

#[test]
fn a_group_that_parts_and_comes_back_has_prepare_said_once_in_a_rest_at_most() {
    // K26. At every other look the player is a chunk beyond the split distance.
    let mut world = World::new(&[4], &["a"]);
    let mut said = Vec::new();
    for step in 0..120 {
        let x = if step % 2 == 0 { 6 } else { 5 };
        world.put(0, &[((0, 0), 2), ((x, 0), 1)]);
        assert!(world.look().is_empty());
        if !world.prepared().is_empty() {
            said.push(world.now - READY);
        }
    }
    // Forgotten at the first tick that wants no split when it is more than a rest
    // old, and said again at the next tick that wants one.
    assert_eq!(said, [250, 250 + REST + 500, 250 + 2 * (REST + 500)]);
}

#[test]
fn a_merge_that_begins_forgets_the_last_prepare_and_it_is_said_again_before_the_split() {
    let mut world = a_split_and_a_merge_of_one_region();
    assert_eq!(world.look(), [split_of(0)]);
    world.split_ends(0, Err(Off::Nobody));
    let rested = world.now + REST;
    world.quiet_until(rested - 1250);
    assert!(world.look().is_empty());
    assert_eq!(world.prepared(), [0]);
    world.quiet_until(rested - LOOK);
    // It is the turn of a merge. The survivor is told to prepare for that, and
    // what it was told for the split is forgotten.
    assert_eq!(world.look(), [merge_of(0, 1)]);
    assert_eq!(world.kept(0).prepared, None);
    world.merge_ends(0, 1);
    let rested = world.now + REST;
    world.quiet_until(rested - 1250);
    assert!(world.look().is_empty());
    assert_eq!(world.prepared(), [0]);
    world.quiet_until(rested - LOOK);
    assert_eq!(world.look(), [split_of(0)]);
}

// Regions without players: section 4.4, and what comes of an absorption (5.5).

/// When a region that has had no player since its first report, a look after it was
/// assigned, has been without players for `EMPTY_FOR`.
const DUE: u64 = LEASE + LOOK + EMPTY_FOR;

#[test]
fn a_region_without_players_is_absorbed_by_the_home_region_after_three_rests() {
    let mut world = World::unpinned(&[4], &["a"]);
    world.quiet_until(DUE - LOOK);
    assert_eq!(world.look(), [merge_of(0, 1)]);
    assert!(world.cluster.coordinator.merges[&RegionId(1)].absorption);
    assert_eq!(world.said.releases, [order("a", 1, E + 2)]);
    assert_eq!(world.said.orders, [prepare("a", 0, E + 1)]);

    // It ends like any merge, with nobody as asker. The survivor does not rest for
    // it: nobody stood still.
    let ended = world.merge_ends(0, 1);
    assert_eq!(ended.reshaped, [merged(None, 0, 1, Ok(0))]);
    assert_eq!(world.alone_until(0), Some(LEASE + REST));
    // The home region is never absorbed, however long nobody is there.
    world.quiet_until(DUE + 2 * EMPTY_FOR);
}

#[test]
fn a_region_begins_its_time_without_players_anew_when_a_report_has_one() {
    let mut world = World::unpinned(&[4], &["a"]);
    world.quiet_until(READY + 5000);
    world.put(1, &[((100, 0), 1)]);
    world.look();
    world.put(1, &[]);
    // Its first report without players after that is at `READY + 5500`.
    world.quiet_until(READY + 5500 + EMPTY_FOR - LOOK);
    assert_eq!(world.look(), [merge_of(0, 1)]);
}

#[test]
fn with_players_at_home_a_region_without_goes_into_the_lowest_such_region_below_it_or_stays() {
    let mut world = World::unpinned(&[4, 8], &["a"]);
    world.put(0, &[((0, 0), 1)]);
    world.quiet_until(DUE - LOOK);
    // Region 2 has region 1 to go into. Region 1 has no survivor: the home region
    // has players, and no region without players has a lower id.
    assert_eq!(world.look(), [merge_of(1, 2)]);
    assert!(world.cluster.coordinator.merges[&RegionId(2)].absorption);
    world.merge_ends(1, 2);
    world.quiet_until(DUE + 2 * EMPTY_FOR);
    assert!(world.cluster.table().route(RegionId(1)).is_some());
}

#[test]
fn a_pinned_region_is_never_absorbed_for_being_empty_and_can_absorb_and_be_merged() {
    // Two stripes and a region that is pinned to nothing, as a part is.
    let mut world = World::begin(&[4, 8], &["a"]);
    world.list.regions[2].pinned.clear();
    world.read();
    assert!(world.kept(1).pinned && !world.kept(2).pinned);
    world.put(0, &[((0, 0), 1)]);
    world.quiet_until(DUE - LOOK);
    // The stripe without players is the survivor of the region without.
    assert_eq!(world.look(), [merge_of(1, 2)]);
    world.merge_ends(1, 2);
    // It stays, however long nobody is there, although the home region...
    world.quiet_until(DUE + EMPTY_FOR);
    world.put(0, &[]);
    // ...has nobody either from now on.
    world.quiet_until(DUE + 3 * EMPTY_FOR);

    // Merges by the distances are not affected: somebody comes within the merge
    // distance of the chunk players enter in, in the stripe's chunks.
    world.put(1, &[((2, 0), 1)]);
    let wanted = world.now + LOOK;
    world.quiet_until(wanted + 1000);
    assert_eq!(world.look(), [merge_of(0, 1)]);
    assert!(!world.cluster.coordinator.merges[&RegionId(1)].absorption);
}

#[test]
fn whether_a_region_is_pinned_follows_the_last_reading_that_succeeded() {
    let mut world = World::new(&[4], &["a"]);
    assert!(world.kept(0).pinned && world.kept(1).pinned);
    // A reading that fails changes nothing; one that succeeds does.
    world.list.regions[1].pinned.clear();
    world.cluster.unlisted(world.now);
    assert!(world.kept(1).pinned);
    world.read();
    assert!(world.kept(0).pinned && !world.kept(1).pinned);
    world.quiet_until(DUE - LOOK);
    assert_eq!(world.look(), [merge_of(0, 1)]);
}

#[test]
fn a_region_that_had_a_player_a_second_ago_is_no_survivor() {
    let mut world = World::unpinned(&[4], &["a"]);
    world.put(0, &[((0, 0), 1)]);
    world.quiet_until(DUE - LOOK);
    // The last player of the home region leaves as the other region is due. Its
    // first report without them is at `DUE`; nothing is begun on one look.
    world.put(0, &[]);
    world.quiet_until(DUE + 1000);
    assert_eq!(world.look(), [merge_of(0, 1)]);
}

#[test]
fn several_regions_without_players_are_absorbed_side_by_side_each_by_a_survivor_of_its_own() {
    // K24. Five are due at one tick, and the home region has players.
    let mut world = World::unpinned(&[4, 8, 12, 16, 20], &["a"]);
    world.put(0, &[((0, 0), 1)]);
    world.quiet_until(DUE - LOOK);
    // The highest goes into the lowest and the next into the next. The one in the
    // middle has no survivor that is not in something begun at this tick.
    assert_eq!(world.look(), [merge_of(2, 4), merge_of(1, 5)]);
    world.quiet_until(DUE + 2000);
    // When a survivor has been reported without players for more than a second
    // after its absorption has ended, it absorbs again.
    world.merge_ends(2, 4);
    world.quiet_until(DUE + 2000 + LOOK + 1000);
    assert_eq!(world.look(), [merge_of(2, 3)]);
}

#[test]
fn no_more_than_four_absorptions_are_under_way_at_a_time() {
    let boundaries: Vec<i32> = (1..=11).map(|region| 4 * region).collect();
    let mut world = World::unpinned(&boundaries, &["a"]);
    world.put(0, &[((0, 0), 1)]);
    world.quiet_until(DUE - LOOK);
    let first = [
        merge_of(4, 8),
        merge_of(3, 9),
        merge_of(2, 10),
        merge_of(1, 11),
    ];
    assert_eq!(world.look(), first);
    // Region 7 has region 5 to go into, and waits for one of the four to end.
    assert!(world.look().is_empty());
    world.merge_ends(1, 11);
    assert_eq!(world.look(), [merge_of(5, 7)]);
}

/// The home region and the regions 1 and 2 have nobody, and region 2 has just been
/// absorbed by the home region, at [`DUE`]. Region 1 waits: its one survivor was in
/// that absorption. Nothing has been reported since.
fn after_an_absorption() -> World {
    let mut world = World::unpinned(&[4, 8], &["a"]);
    world.quiet_until(DUE - LOOK);
    assert_eq!(world.look(), [merge_of(0, 2)]);
    world.merge_ends(0, 2);
    assert!(world.kept(0).after_absorption);
    world
}

#[test]
fn a_survivor_absorbs_again_a_second_after_it_was_reported_without_players_and_not_a_rest_after() {
    let mut world = after_an_absorption();
    // Its first report after the end is at `DUE + 250`.
    world.quiet_until(DUE + LOOK + 1000);
    assert_eq!(world.look(), [merge_of(0, 1)]);

    // Also while it rests for another reason: its rest is not looked at.
    let mut world = after_an_absorption();
    world.rests_from_now(0);
    world.quiet_until(DUE + LOOK + 1000);
    assert_eq!(world.look(), [merge_of(0, 1)]);
    assert!(world.alone_until(0) > Some(world.now));
}

#[test]
fn an_absorption_that_ends_well_leaves_its_survivor_free_for_a_merge_by_the_distances() {
    let mut world = after_an_absorption();
    // Somebody is in region 1, far away, and walks up to the chunk players enter
    // in. The merge is begun when it has stood, without a rest of the home region.
    world.put(1, &[((2, 0), 1)]);
    world.quiet_until(DUE + LOOK + 1000);
    assert_eq!(world.look(), [merge_of(0, 1)]);
    assert!(!world.cluster.coordinator.merges[&RegionId(1)].absorption);
    assert_eq!(world.alone_until(0), Some(LEASE + REST));
    // Neither its counters nor its turn are touched by an absorption.
    assert!(!world.kept(0).split_last && world.kept(0).failures == 0);
}

#[test]
fn a_survivor_rests_from_its_first_report_after_an_absorption_if_that_has_a_player() {
    // K13: somebody came into one of the two regions as the absorption began, and
    // has stood still for it. However late that report comes: the worker restores
    // a region meanwhile and reports nothing for five seconds.
    let mut world = after_an_absorption();
    world.silent.insert(0);
    world.put(1, &[((2, 0), 1)]);
    world.quiet_until(DUE + 5000);
    world.silent.clear();
    world.put(0, &[((0, 1), 1)]);
    assert!(world.look().is_empty());
    let reported = world.now;
    assert_eq!(world.alone_until(0), Some(reported + REST));
    assert!(!world.kept(0).after_absorption);
    // Nothing by the distances stands them still again within ten seconds.
    world.quiet_until(reported + REST - LOOK);
    assert_eq!(world.look(), [merge_of(0, 1)]);
}

#[test]
fn a_player_in_the_second_report_after_an_absorption_begins_no_rest() {
    let mut world = after_an_absorption();
    world.look();
    assert!(!world.kept(0).after_absorption);
    world.put(0, &[((0, 1), 1)]);
    world.put(1, &[((2, 0), 1)]);
    world.look();
    assert_eq!(world.alone_until(0), Some(LEASE + REST));
    let wanted = world.now;
    world.quiet_until(wanted + 1000);
    assert_eq!(world.look(), [merge_of(0, 1)]);
}

#[test]
fn an_absorption_that_comes_to_nothing_leaves_the_region_alone_and_its_survivor_as_it_was() {
    let mut world = World::unpinned(&[4, 8], &["a"]);
    world.quiet_until(DUE - LOOK);
    assert_eq!(world.look(), [merge_of(0, 2)]);
    let ended = world.merge_fails(0, 2, Off::Unreadable);
    let why = Err(Undone::Off(Off::Unreadable));
    assert_eq!(ended.reshaped, [merged(None, 0, 2, why)]);
    // The absorbed region as after any merge that comes to nothing. The survivor
    // is neither left alone nor has a failure counted: it is the survivor of other
    // regions as well, and nobody stood still on its side.
    assert_eq!(world.alone_until(2), Some(DUE + LONG));
    assert_eq!(world.kept(2).failures, 1);
    assert_eq!(world.alone_until(0), Some(LEASE + REST));
    assert_eq!(world.kept(0).failures, 0);
    assert!(world.kept(0).after_absorption);
    // So the next region is absorbed by it a second after its next report.
    world.quiet_until(DUE + LOOK + 1000);
    assert_eq!(world.look(), [merge_of(0, 1)]);
    world.merge_ends(0, 1);
    // The region that failed was given an owner anew, and has been without players
    // since its first report after that, at `DUE + 250`.
    world.quiet_until(DUE + LOOK + EMPTY_FOR - LOOK);
    assert_eq!(world.look(), [merge_of(0, 2)]);
}

/// The world of section 11's F36, at 40 500: the home region has nobody and has
/// just been the survivor of an absorption, of the highest region, which ended at
/// 40 250. The region before the highest has been without players since 11 500 and
/// is due at 41 500. Reports are handed in 100 ms after each tick from then on: the
/// home region's first after the absorption was taken at 40 350, and with it that
/// of the region `arrives`, which has a player two chunks from where players enter.
/// So the merge of that region with the home region is wanted from 40 500.
fn somebody_arrives_as_an_empty_region_is_due(boundaries: &[i32], arrives: u32) -> World {
    let mut world = World::begin(boundaries, &["a"]);
    let last = u32::try_from(boundaries.len()).unwrap();
    for info in &mut world.list.regions {
        // The region between the home region and the others stays a stripe.
        if info.region.0 == 0 || info.region.0 + 2 >= last {
            info.pinned.clear();
        }
    }
    world.list.regions[arrives as usize].pinned.clear();
    world.read();
    world.put(arrives, &[((100, 0), 1)]);
    world.put(last - 1, &[((200, 0), 1)]);
    world.quiet_until(11_250);
    world.put(last - 1, &[]);
    world.quiet_until(DUE - LOOK);
    assert_eq!(world.look(), [merge_of(0, last)]);
    world.merge_ends(0, last);
    world.put(arrives, &[((2, 0), 1)]);
    assert!(world.look_after_reports().is_empty());
    assert_eq!(world.now, 40_500);
    assert_eq!(world.kept(0).empty_since, Some(world.cluster.at(40_350)));
    world
}

#[test]
fn a_region_that_a_merge_is_wanted_of_is_no_survivor() {
    // F36. Regions: the home region, the one that arrives, the one that is due,
    // and the one that was absorbed first.
    let mut world = somebody_arrives_as_an_empty_region_is_due(&[4, 8, 12], 1);
    // At 41 500 the home region has been without players for more than a second,
    // and the merge has been wanted for exactly a second, which is not long enough
    // to have stood. The region that is due is not absorbed by the home region
    // then, nor at any tick before, and has no other survivor.
    while world.now < 41_500 {
        assert!(world.look_after_reports().is_empty(), "at {}", world.now);
    }
    assert_eq!(world.look_after_reports(), [merge_of(0, 1)]);
    assert!(!world.cluster.coordinator.merges[&RegionId(1)].absorption);
}

#[test]
fn a_region_without_players_goes_into_the_next_survivor_while_a_merge_is_wanted_of_the_first() {
    // F36 with a second candidate: a stripe without players, which has a lower id.
    let mut world = somebody_arrives_as_an_empty_region_is_due(&[4, 8, 12, 16], 2);
    while world.now < 41_250 {
        assert!(world.look_after_reports().is_empty(), "at {}", world.now);
    }
    assert_eq!(world.look_after_reports(), [merge_of(1, 3)]);
    assert_eq!(world.now, 41_500);
    assert_eq!(world.look_after_reports(), [merge_of(0, 2)]);
}

// Evening out: section 6.

#[test]
fn of_a_worker_s_regions_the_one_with_the_fewest_players_that_does_not_rest_is_moved_at_once() {
    let mut world = World::new(&[4, 8, 12], &["a"]);
    world.put(0, &[((0, 0), 3)]);
    world.put(1, &[((100, 0), 1)]);
    world.put(2, &[((200, 0), 2)]);
    world.put(3, &[((300, 0), 1)]);
    world.look();
    // Of the two with one player, the one with the higher id rests.
    world.rests_from_now(3);
    world.cluster.register(world.now, "b", "b:25601", &[]);
    assert!(world.look().is_empty());
    assert_eq!(world.said.releases, [order("a", 1, E + 2)]);
    // One release at a time, as before.
    assert!(world.look().is_empty());
    assert!(world.said.releases.is_empty());
    world.cluster.released(world.now, "a", 1, E + 2);
    assert_eq!(world.held(1).0, "b");
    // Three against one: of the two that do not rest, the one with two players.
    assert!(world.look().is_empty());
    assert_eq!(world.said.releases, [order("a", 2, E + 3)]);
    let moved = world.now;
    world.cluster.released(moved, "a", 2, E + 3);
    world.quiet_until(moved + 2 * REST);
}

#[test]
fn of_regions_with_as_many_players_the_highest_is_moved_and_one_never_reported_comes_last() {
    let mut world = World::begin(&[4, 8, 12], &["a"]);
    world.silent.insert(3);
    world.put(0, &[((0, 0), 1)]);
    world.put(1, &[((100, 0), 1)]);
    world.put(2, &[((200, 0), 1)]);
    world.quiet_until(READY);
    // Nothing is merged or split while a region has never been reported; evening
    // out goes by owners, and is not held back by that.
    world.cluster.register(world.now, "b", "b:25601", &[]);
    world.look();
    assert_eq!(world.said.releases, [order("a", 2, E + 3)]);
    world.cluster.released(world.now, "a", 2, E + 3);
    world.look();
    assert_eq!(world.said.releases, [order("a", 1, E + 2)]);
}

#[test]
fn a_region_that_a_merge_or_a_split_is_wanted_of_is_not_moved_to_even_out() {
    let mut world = World::new(&[4, 8, 12], &["a"]);
    world.put(0, &[((0, 0), 5)]);
    world.put(1, &[((100, 0), 1)]);
    world.put(2, &[((102, 0), 1)]);
    world.put(3, &[((300, 0), 2), ((310, 0), 1)]);
    // Wanted from this look, and not stood yet.
    world.look();
    world.cluster.register(world.now, "b", "b:25601", &[]);
    assert!(world.look().is_empty());
    assert_eq!(world.said.releases, [order("a", 0, E + 1)]);

    // If something is wanted of every region of that worker, nothing is evened
    // out at that tick.
    let mut world = World::new(&[4, 8, 12], &["a"]);
    world.put(0, &[((0, 0), 5), ((10, 0), 1)]);
    world.put(1, &[((100, 0), 1)]);
    world.put(2, &[((102, 0), 1)]);
    world.put(3, &[((300, 0), 2), ((310, 0), 1)]);
    world.look();
    world.cluster.register(world.now, "b", "b:25601", &[]);
    assert!(world.look().is_empty());
    assert!(world.said.releases.is_empty());
}

#[test]
fn nothing_is_evened_out_while_a_merge_or_a_split_is_under_way() {
    let mut world = World::new(&[4, 8, 12], &["a"]);
    world.look();
    world.cluster.merge(world.now, 0, 1, Some(7)).unwrap();
    world.cluster.register(world.now, "b", "b:25601", &[]);
    world.quiet_until(READY + 3000);
    world.merge_ends(0, 1);
    // The survivor rests; another region of that worker is moved at once, and not
    // a lease after the merge has ended.
    assert!(world.look().is_empty());
    assert_eq!(world.said.releases, [order("a", 3, E + 4)]);
}

#[test]
fn a_part_is_moved_when_it_has_rested_and_not_a_lease_after_the_split() {
    // Section 11, E2: with a rest that is shorter than the lease, the two can be
    // told apart.
    let short = Policy {
        rest: Duration::from_secs(4),
        ..small()
    };
    let mut world = World::begin_by(short, &[4], &["a", "b"]);
    world.quiet_until(LEASE + 4000);
    // Somebody merges the two stripes: one worker has the one region there is.
    world.cluster.merge(world.now, 0, 1, Some(7)).unwrap();
    world.merge_ends(0, 1);
    world.put(0, &[((0, 0), 2), ((10, 0), 1)]);
    let (begun, what) = world.until_begun(LEASE + 8000);
    assert_eq!((begun, what), (LEASE + 8000, vec![split_of(0)]));
    world.split_ends(0, Ok(2));
    world.put(0, &[((0, 0), 2)]);
    world.put(2, &[((10, 0), 1)]);
    let part = world.held(2).1;
    assert_eq!(
        (world.held(0).0.as_str(), world.held(2).0.as_str()),
        ("a", "a")
    );
    // The worker that made the part has two regions and the other none. Both rest
    // for four seconds; then the part, which has fewer players, is moved.
    world.quiet_until(begun + 4000 - LOOK);
    assert!(world.look().is_empty());
    assert_eq!(world.said.releases, [order("a", 2, part.epoch)]);
}

// Orders of events: section 9, where no test above has them.

#[test]
fn a_region_that_is_to_be_split_is_not_merged_with_whoever_is_near_the_group_that_goes() {
    // K2 (a). Region 1 has a group at home and one far off, and region 2's player
    // is near the far one only.
    let mut world = World::new(&[4, 8], &["a"]);
    world.put(1, &[((100, 0), 2), ((110, 0), 1)]);
    world.put(2, &[((112, 0), 1)]);
    world.quiet_until(READY + 1250);
    assert!(world.cluster.coordinator.noted.merges.is_empty());
    assert_eq!(world.look(), [split_of(1)]);
    world.split_ends(1, Ok(3));
    world.put(1, &[((100, 0), 2)]);
    world.put(3, &[((110, 0), 1)]);
    // The part and that region merge when the part has rested: three sets of
    // players stand still once, once and twice.
    let rested = world.now + REST;
    world.quiet_until(rested - LOOK);
    assert_eq!(world.look(), [merge_of(2, 3)]);
}

#[test]
fn a_region_whose_groups_another_region_joins_is_merged_with_it_and_not_split() {
    // K2 (b), with two players of the other region, as the distances 2 and 5 take.
    let mut world = World::new(&[4, 8], &["a"]);
    world.put(1, &[((100, 0), 2), ((109, 0), 1)]);
    world.put(2, &[((102, 0), 1), ((107, 0), 1)]);
    world.quiet_until(READY + 1250);
    assert!(world.prepared().is_empty());
    assert_eq!(world.look(), [merge_of(1, 2)]);
}

#[test]
fn players_who_cross_the_band_between_the_distances_are_stood_still_once_in_a_rest() {
    // K4.
    let mut world = World::new(&[4, 8], &["a"]);
    world.put(1, &[((100, 0), 1)]);
    world.put(2, &[((102, 0), 1)]);
    world.quiet_until(READY + 1250);
    assert_eq!(world.look(), [merge_of(1, 2)]);
    world.merge_ends(1, 2);
    // One region. They are four apart, at which nothing is wanted in one region
    // or in two, and then six.
    world.put(1, &[((100, 0), 1), ((104, 0), 1)]);
    world.quiet_until(world.now + 2 * REST);
    world.put(1, &[((100, 0), 1), ((106, 0), 1)]);
    world.quiet_until(world.now + 1250);
    assert_eq!(world.look(), [split_of(1)]);
    world.split_ends(1, Ok(3));
    // Two regions, four apart, and then two: each a rest after the one before.
    world.put(1, &[((100, 0), 1)]);
    world.put(3, &[((104, 0), 1)]);
    let rested = world.now + REST;
    world.quiet_until(rested + REST);
    world.put(3, &[((102, 0), 1)]);
    world.quiet_until(rested + REST + 1250);
    assert_eq!(world.look(), [merge_of(1, 3)]);
    world.merge_ends(1, 3);
    world.put(1, &[((100, 0), 1), ((106, 0), 1)]);
    let rested = world.now + REST;
    world.quiet_until(rested - LOOK);
    assert_eq!(world.look(), [split_of(1)]);
}

#[test]
fn when_the_survivor_s_worker_dies_both_regions_are_left_alone_for_long() {
    // K7. Region 1 is `b`'s and survives; `a` is asked to release region 2.
    let mut world = two_regions_near_each_other(&["a", "b"]);
    world.quiet_until(READY + 1250);
    assert_eq!(world.look(), [merge_of(1, 2)]);
    world.dead.insert("b".to_owned());
    world.cluster.disconnected(world.now, "b");
    while world.said.reshaped.is_empty() {
        world.look();
    }
    let why = Err(Undone::Disowned(RegionId(1)));
    assert_eq!(world.said.reshaped, [merged(None, 1, 2, why)]);
    let ended = world.now;
    // Both are given owners and rest, and are left alone for `LONG`. Their
    // sightings stay meanwhile.
    assert_eq!(
        (world.held(1).0.as_str(), world.held(2).0.as_str()),
        ("a", "a")
    );
    assert_eq!(world.alone_until(1), Some(ended + LONG));
    assert_eq!(world.alone_until(2), Some(ended + LONG));
    assert_eq!(world.sighted(1).unwrap(), [((100, 0), 2)]);
    world.quiet_until(ended + LONG - LOOK);
    assert_eq!(world.look(), [merge_of(1, 2)]);
}

#[test]
fn what_is_asked_by_hand_is_undone_when_the_distances_say_otherwise() {
    // K17. A group is split off by hand within the merge distance of the others.
    let mut world = World::new(&[4], &["a"]);
    world.put(0, &[((0, 0), 2), ((2, 0), 1)]);
    world.look();
    let chunks = around(&[(2, 0)]);
    world.cluster.split(world.now, 0, &chunks, Some(7)).unwrap();
    world.split_ends(0, Ok(2));
    world.put(0, &[((0, 0), 2)]);
    world.put(2, &[((2, 0), 1)]);
    // It is merged back when both have rested.
    let rested = world.now + REST;
    world.quiet_until(rested - LOOK);
    assert_eq!(world.look(), [merge_of(0, 2)]);
    world.merge_ends(0, 2);

    // And two regions merged by hand whose players are far apart are split again.
    world.put(0, &[((0, 0), 3)]);
    world.put(1, &[((100, 0), 1)]);
    world.quiet_until(world.now + 2 * REST);
    world.cluster.merge(world.now, 0, 1, Some(7)).unwrap();
    world.merge_ends(0, 1);
    let rested = world.now + REST;
    world.quiet_until(rested - LOOK);
    assert_eq!(world.look(), [split_of(0)]);
    assert_eq!(world.split_off().unwrap().1, around(&[(100, 0)]));
}

#[test]
fn a_region_that_the_list_adds_takes_part_in_nothing_and_holds_everything_back_until_reported() {
    // K19 and K23. The list shows a region nobody knew: a part whose worker died
    // before it said so.
    let mut world = two_regions_near_each_other(&["a"]);
    world.list.regions.push(living(3, 0));
    world.list.next = RegionId(4);
    world.read();
    // It is assigned at once, rests from then, and has no sighting.
    assert_eq!(world.held(3).0, "a");
    assert_eq!(world.alone_until(3), Some(READY + REST));
    assert!(world.sighted(3).is_none());
    // The worker restores it for five seconds and says nothing of it meanwhile.
    // The merge of the two others has stood long since and is not begun.
    world.silent.insert(3);
    world.quiet_until(READY + 5000);
    world.silent.clear();
    world.put(3, &[((300, 0), 2), ((310, 0), 1)]);
    assert_eq!(world.look(), [merge_of(1, 2)]);
    // The region itself is split when it has rested.
    world.merge_ends(1, 2);
    world.quiet_until(READY + REST - LOOK);
    assert_eq!(world.look(), [split_of(3)]);
}

#[test]
fn a_part_that_a_reading_shows_before_its_worker_says_so_is_left_out_and_then_its_worker_s() {
    // K19.
    let mut world = World::new(&[4], &["a"]);
    world.put(0, &[((0, 0), 2), ((10, 0), 1)]);
    world.quiet_until(READY + 1250);
    assert_eq!(world.look(), [split_of(0)]);
    let as_epoch = world.cluster.coordinator.splits[&RegionId(0)].as_epoch;
    // The store has made the part, and the timer's reading lands before the
    // worker's word.
    world.list.regions.push(living(2, as_epoch));
    world.list.next = RegionId(3);
    world.read();
    assert!(!world.cluster.coordinator.regions.contains_key(&RegionId(2)));
    world.quiet_until(READY + 2500);
    world
        .cluster
        .split_ended(world.now, "a", 0, as_epoch, Ok(2));
    world.read();
    assert_eq!(world.held(2).1.epoch, as_epoch);
    assert_eq!(world.sighted(2).unwrap(), [((10, 0), 1)]);
}

#[test]
fn a_merge_that_the_list_shows_done_before_the_worker_says_so_ends_there_and_the_survivor_rests() {
    // K20.
    let mut world = two_regions_near_each_other(&["a"]);
    world.quiet_until(READY + 1250);
    assert_eq!(world.look(), [merge_of(1, 2)]);
    let epoch = world.held(2).1.epoch;
    world.cluster.released(world.now, "a", 2, epoch);
    world.list.regions.retain(|info| info.region.0 != 2);
    world.list.absorbed.push((RegionId(2), RegionId(1)));
    world.now += LOOK;
    let ended = world.read();
    assert_eq!(ended.reshaped, [merged(None, 1, 2, Ok(1))]);
    assert_eq!(world.alone_until(1), Some(world.now + REST));
    assert_eq!(world.sighted(1).unwrap(), [((100, 0), 2), ((102, 0), 1)]);
    // The worker's word finds no merge noted, and has the list read once more.
    let said = world.cluster.absorb_ended(world.now, "a", 1, 2, Ok(()));
    assert_eq!(said, reads());
    world.read();
    assert_eq!(world.alone_until(1), Some(world.now + REST));
}

#[test]
fn nothing_is_begun_and_no_region_is_told_to_prepare_when_no_epoch_is_left() {
    let mut world = World::new(&[4, 8], &["a"]);
    world.put(0, &[((0, 0), 2), ((10, 0), 1)]);
    world.put(1, &[((100, 0), 2)]);
    world.put(2, &[((102, 0), 1)]);
    world.cluster.coordinator.last_epoch = u64::MAX;
    while world.now < READY + 5000 {
        assert!(world.look().is_empty());
        assert!(world.said.orders.is_empty() && world.said.releases.is_empty());
    }
}

// The order inside one tick, and what the log says of it: sections 5.3 and 10.

/// The thread whose events [`Lines`] keeps, and the lines it has of them so far.
static CAUGHT: std::sync::Mutex<Option<(std::thread::ThreadId, Vec<String>)>> =
    std::sync::Mutex::new(None);

/// A subscriber that keeps the events of one thread, each as the line the log has
/// of it without the time and the level: the module, the message, and the fields in
/// their order.
///
/// It is the default of the whole process and not of the thread that listens. A
/// line is written only if somebody was interested in it where it was first
/// reached, which is looked at again only when a subscriber comes or goes; with a
/// subscriber of one thread, a line that the thread of another test reaches first
/// is switched off for good, and this test would miss it now and then. So every
/// thread asks this subscriber at every event, and it says no to all but one.
struct Lines;

impl Lines {
    /// Whether the events of this thread are kept at the moment.
    fn listens() -> bool {
        // A test that failed while it held this is none of the other threads' concern.
        let Ok(caught) = CAUGHT.lock() else {
            return false;
        };
        let thread = caught.as_ref().map(|(thread, _)| *thread);
        thread == Some(std::thread::current().id())
    }
}

/// The message and the fields of one event.
#[derive(Default)]
struct Line {
    message: String,
    fields: String,
}

impl tracing::field::Visit for Line {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            write!(self.fields, " {}={value:?}", field.name()).unwrap();
        }
    }
}

impl tracing::Subscriber for Lines {
    fn register_callsite(
        &self,
        _: &'static tracing::Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        // Asked at every event, as the answer depends on the thread.
        tracing::subscriber::Interest::sometimes()
    }

    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        Self::listens()
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        if !Self::listens() {
            return;
        }
        let mut line = Line::default();
        event.record(&mut line);
        let target = event.metadata().target();
        let line = format!("{target}: {}{}", line.message, line.fields);
        if let Ok(mut caught) = CAUGHT.lock()
            && let Some((_, lines)) = caught.as_mut()
        {
            lines.push(line);
        }
    }

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}

/// Regions 1 and 5 are each to be split, regions 2 and 3 are near each other, and
/// region 4 and the home region have nobody; all of it is due at one tick, and a
/// worker that runs nothing has registered a moment before. Returns the world after
/// the look of that tick, and what the look began.
fn everything_is_due_at_one_tick() -> (World, Vec<Asked>) {
    let mut world = World::unpinned(&[4, 8, 12, 16, 20], &["a"]);
    world.put(2, &[((200, 0), 2)]);
    world.put(3, &[((203, 0), 1)]);
    world.quiet_until(DUE - 1500);
    world.put(1, &[((100, 0), 2), ((110, 0), 1)]);
    world.put(5, &[((500, 0), 2), ((510, 0), 1), ((520, 0), 1)]);
    world.put(3, &[((202, 0), 1)]);
    world.quiet_until(DUE - LOOK);
    world.cluster.register(world.now, "b", "b:25601", &[]);
    let begun = world.look();
    (world, begun)
}

#[test]
fn a_tick_begins_a_split_then_the_merges_then_the_absorptions_and_evens_out_nothing_then() {
    let (world, begun) = everything_is_due_at_one_tick();
    assert_eq!(begun, [merge_of(2, 3), merge_of(0, 4), split_of(1)]);
    // One split in the world: of two whose groups have gone since the same tick
    // the lower region. Then the merge, then the absorption; each merge has its
    // survivor prepare.
    let told: Vec<&Order> = world.said.orders.iter().map(|told| &told.order).collect();
    assert!(matches!(told[0], Order::SplitOff { region, .. } if region.0 == 1));
    assert_eq!(*told[1], prepare("a", 2, E + 3).order);
    assert_eq!(*told[2], prepare("a", 0, E + 1).order);
    assert_eq!(told.len(), 3);
    // Evening out runs after all of it, and begins nothing while a merge or a
    // split is under way: the only regions to be released are those to absorb,
    // although one worker runs six regions and the other none.
    assert_eq!(
        world.said.releases,
        [order("a", 3, E + 4), order("a", 4, E + 5)]
    );
}

/// What the log has of `scenario`: every event of it as the line it is written as,
/// without the time and the level. The tests that call it listen one at a time, as
/// [`CAUGHT`] keeps the lines of one thread, and no other test of this crate listens
/// to the log.
fn logged(scenario: impl FnOnce()) -> Vec<String> {
    static LISTENING: std::sync::Once = std::sync::Once::new();
    static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LISTENING.call_once(|| {
        tracing::subscriber::set_global_default(Lines)
            .expect("no other test of this crate listens to the log");
    });
    // A test that failed while it listened has had its turn.
    let _turn = TURN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let this = std::thread::current().id();
    *CAUGHT.lock().unwrap() = Some((this, Vec::new()));
    scenario();
    let caught = CAUGHT.lock().unwrap().take();
    caught.expect("nobody else takes the lines").1
}

#[test]
fn the_log_has_a_line_for_each_thing_the_coordinator_begins_by_itself() {
    // Section 10: the end-to-end tests count from these lines, by their messages,
    // and read the fields in this order.
    let lines = logged(|| {
        everything_is_due_at_one_tick();
    });
    let begun: Vec<&str> = lines
        .iter()
        .map(String::as_str)
        .filter(|line| line.contains("begun by"))
        .collect();
    assert_eq!(
        begun,
        [
            "clustine_coordinator::state: a split is begun by itself \
             region=1 part=6 groups=1 chunks=25",
            "clustine_coordinator::state: a merge is begun by the distances \
             survivor=2 absorbed=3 gap=2",
            "clustine_coordinator::state: an absorption is begun by itself \
             survivor=0 absorbed=4",
        ]
    );
    // The lines that a merge and a split write for whoever asked are written for
    // these as well.
    let count = |what: &str| lines.iter().filter(|line| line.contains(what)).count();
    assert_eq!(count("a region is to absorb another"), 2);
    assert_eq!(count("a region is to be split"), 1);
    assert_eq!(count("a region is moved to even regions out"), 0);

    // A split that names two groups and finds nobody, and a release to even out.
    let lines = logged(|| {
        let mut world = World::new(&[4, 8], &["a"]);
        world.put(1, &[((100, 0), 3), ((110, 0), 1), ((120, 0), 1)]);
        world.quiet_until(READY + 1250);
        assert_eq!(world.look(), [split_of(1)]);
        world.split_ends(1, Err(Off::Nobody));
        world.cluster.register(world.now, "b", "b:25601", &[]);
        world.look();
    });
    let has = |line: &str| lines.iter().any(|said| said == line);
    assert!(has(
        "clustine_coordinator::state: a split is begun by itself \
         region=1 part=3 groups=2 chunks=50"
    ));
    assert!(has(
        "clustine_coordinator::state: a region is moved to even regions out \
         region=2 from=a to=b"
    ));
    assert!(has(&format!(
        "clustine_coordinator::state: a worker says what came of a split \
         worker=\"a\" region=1 as_epoch={} outcome=Err(Nobody)",
        E + 4
    )));
}

// N14 of `docs/adr/0017-the-end-of-the-stripes.md`, and its scenario Q7.
#[test]
fn the_log_says_once_that_a_world_that_is_reshaped_by_itself_has_pinned_regions() {
    const LINE: &str = "clustine_coordinator::state: the world has regions that are pinned \
                        to an area: a region that is split off here cannot grow. Start the \
                        coordinator with --reshape by-hand to keep pinned regions as they are";
    let said = |lines: &[String]| lines.iter().filter(|line| *line == LINE).count();

    // A world of stripes: the first reading shows them pinned, and the list is read
    // every lease from then on, and once more here.
    let lines = logged(|| {
        let mut world = World::new(&[4], &["a"]);
        world.quiet_until(READY + 3 * LEASE);
        world.read();
    });
    assert_eq!(said(&lines), 1, "{lines:#?}");

    // A world without pinned regions: never. If a later reading shows one, then;
    // and not again for the readings after it, whichever regions they show pinned.
    let lines = logged(|| {
        let mut cluster = following(&[4]);
        cluster.listed(0, &stripes(2));
        cluster.listed(1, &stripes(2));
    });
    assert_eq!(said(&lines), 0, "{lines:#?}");
    let lines = logged(|| {
        let mut cluster = following(&[4]);
        cluster.listed(0, &stripes(2));
        let mut list = stripes(2);
        list.regions[1] = stripe(1);
        cluster.listed(1, &list);
        cluster.listed(2, &list);
        list.regions[0] = stripe(0);
        cluster.listed(3, &list);
        cluster.listed(4, &stripes(2));
        cluster.listed(5, &list);
    });
    assert_eq!(said(&lines), 1, "{lines:#?}");

    // A coordinator that reshapes by hand keeps pinned regions as they are, and has
    // nothing to say of them.
    let lines = logged(|| {
        let mut cluster = Cluster::new(&[4]);
        let list = RegionList {
            regions: vec![stripe(0), stripe(1)],
            ..stripes(2)
        };
        cluster.listed(0, &list);
    });
    assert_eq!(said(&lines), 0, "{lines:#?}");
}

#[test]
fn nothing_is_wanted_of_a_region_that_stands_still() {
    // K8. The regions wait for the store: their ticks do not go up, and their
    // worker says the same of them at every look.
    let mut world = two_regions_near_each_other(&["a"]);
    world.look();
    while world.now < READY + 6000 {
        world.now += LOOK;
        world.report(0);
        assert!(world.decide().is_empty(), "at {}", world.now);
    }
    assert!(!world.fresh(1, world.now) && !world.fresh(2, world.now));
    let merges = &world.cluster.coordinator.noted.merges;
    assert!(merges.values().all(|waited| waited.since.is_none()));
    // They tick on, and the merge has to stand anew.
    world.quiet_until(READY + 6000 + 1250);
    assert_eq!(world.look(), [merge_of(1, 2)]);
}

#[test]
fn a_split_whose_region_changes_hands_leaves_the_region_alone_and_was_its_turn() {
    // K18. The owner dies between the look and the order.
    let mut world = World::new(&[4], &["a", "b"]);
    world.put(1, &[((100, 0), 2), ((110, 0), 1)]);
    world.quiet_until(READY + 1250);
    assert_eq!(world.look(), [split_of(1)]);
    assert_eq!(world.held(1).0, "b");
    world.dead.insert("b".to_owned());
    world.cluster.disconnected(world.now, "b");
    while world.said.reshaped.is_empty() {
        world.look();
    }
    let why = Err(Undone::Disowned(RegionId(1)));
    assert_eq!(world.said.reshaped, [was_split(None, 1, why)]);
    let ended = world.now;
    assert_eq!(world.alone_until(1), Some(ended + LONG));
    assert!(world.kept(1).split_last);
    // A region that is given an owner keeps its count of failures: what failed
    // need not have been the owner's doing.
    assert_eq!(world.held(1).0, "a");
    assert_eq!(world.kept(1).failures, 1);
    world.quiet_until(ended + LONG - LOOK);
    assert_eq!(world.look(), [split_of(1)]);
}

/// How far two chunks are apart, as the record counts it.
fn apart(one: (i32, i32), other: (i32, i32)) -> i32 {
    (one.0 - other.0).abs().max((one.1 - other.1).abs())
}

/// Whether `chunks` hold together by steps of at most `step`.
fn joined(chunks: &[(i32, i32)], step: i32) -> bool {
    let mut reached = vec![false; chunks.len()];
    let mut next: Vec<usize> = chunks.first().map(|_| 0).into_iter().collect();
    while let Some(one) = next.pop() {
        if std::mem::replace(&mut reached[one], true) {
            continue;
        }
        let near = |other: &usize| apart(chunks[one], chunks[*other]) <= step;
        next.extend((0..chunks.len()).filter(near));
    }
    reached.into_iter().all(|reached| reached)
}

/// A player of the walk below: the region they are of, the chunk they are in, and
/// the place they walk to.
type Walker = (u32, (i32, i32), (i32, i32));

/// Players walk between a handful of places, join and leave, all at random; workers
/// do what they are told a few looks later. This is no substitute for the runs that
/// are written from the record. It shows that the state machine and [`Cluster`]'s
/// checks hold over long runs of ordinary play, that nothing is begun with a region
/// that is left alone, and that when everybody stands still it ends where the
/// record says: who is near each other is in one region, who is far apart is not,
/// and of the regions without players one is left at most.
#[test]
fn players_who_walk_at_random_are_followed_and_it_ends_when_they_stand_still() {
    const PLACES: [(i32, i32); 7] = [
        (0, 0),
        (2, 1),
        (9, 0),
        (0, 12),
        (-14, -14),
        (11, 2),
        (30, 0),
    ];
    let (mut merges, mut absorptions, mut splits, mut moves) = (0, 0, 0, 0);
    for seed in 1..=10_u64 {
        let mut random = Generator(0x9e37_79b9_7f4a_7c15 ^ seed);
        let mut world = World::unpinned(&[40], &["a", "b"]);
        let mut players: Vec<Walker> = Vec::new();
        // What workers are at, and at which look each is done.
        let mut doing: Vec<(u64, Asked)> = Vec::new();
        for look in 0..2400_u64 {
            let walking = look < 1400;
            if walking {
                if players.len() < 9 && random.once_in(12) {
                    players.push((0, (0, 0), (0, 0)));
                }
                if players.len() > 2 && random.once_in(160) {
                    let left = random.below(players.len() as u64);
                    players.remove(usize::try_from(left).unwrap());
                }
                for (_, at, to) in &mut players {
                    if random.once_in(40) {
                        *to = PLACES[usize::try_from(random.below(7)).unwrap()];
                    }
                    // A chunk in three looks along each axis, as somebody who flies.
                    if random.once_in(3) {
                        at.0 += (to.0 - at.0).signum();
                        at.1 += (to.1 - at.1).signum();
                    }
                }
            }

            // The workers end what is due: a merge puts the players of the one
            // region into the other, and a split takes who stands in a chunk named.
            let (due, later): (Vec<_>, Vec<_>) = doing.iter().partition(|(at, _)| *at <= look);
            doing = later;
            for (_, asked) in due {
                match asked {
                    Asked::Merge { survivor, absorbed } => {
                        world.merge_ends(survivor.0, absorbed.0);
                        for player in &mut players {
                            if player.0 == absorbed.0 {
                                player.0 = survivor.0;
                            }
                        }
                    }
                    Asked::Split { region } => {
                        let named = world.cluster.coordinator.splits[&region].chunks.clone();
                        let part = world.list.next.0;
                        let mut went = false;
                        for player in &mut players {
                            let chunk = ChunkPos::new(player.1.0, player.1.1);
                            if player.0 == region.0 && named.contains(&chunk) {
                                player.0 = part;
                                went = true;
                            }
                        }
                        let outcome = if went { Ok(part) } else { Err(Off::Nobody) };
                        world.split_ends(region.0, outcome);
                    }
                }
            }
            let regions: Vec<u32> = world
                .list
                .regions
                .iter()
                .map(|info| info.region.0)
                .collect();
            for region in &regions {
                let of_region = players.iter().filter(|player| player.0 == *region);
                let crowds: Where = of_region.map(|player| (player.1, 1)).collect();
                world.put(*region, &crowds);
            }

            let alone = |world: &World, region: RegionId| {
                let until = world.alone_until(region.0);
                assert!(until.is_none_or(|until| until <= world.now), "{region}");
            };
            let before = world.now + LOOK;
            let rested: Vec<(u32, bool)> = regions
                .iter()
                .map(|region| {
                    let until = world.alone_until(*region);
                    (*region, until.is_none_or(|until| until <= before))
                })
                .collect();
            let rested = |region: RegionId| rested.contains(&(region.0, true));
            let begun = world.look();
            for asked in &begun {
                match asked {
                    Asked::Merge { survivor, absorbed } => {
                        assert!(rested(*absorbed), "seed {seed} at {}: {asked:?}", world.now);
                        if world.cluster.coordinator.merges[absorbed].absorption {
                            absorptions += 1;
                        } else {
                            assert!(rested(*survivor), "seed {seed}: {asked:?}");
                            merges += 1;
                        }
                    }
                    Asked::Split { region } => {
                        assert!(rested(*region), "seed {seed} at {}: {asked:?}", world.now);
                        splits += 1;
                    }
                }
                doing.push((look + 1 + random.below(4), *asked));
            }
            // A release that is no merge's is one to even out, and is answered.
            for release in world.said.releases.clone() {
                if world
                    .cluster
                    .coordinator
                    .releases
                    .contains_key(&release.region)
                {
                    assert!(rested(release.region), "seed {seed}: {release:?}");
                    alone(&world, release.region);
                    let (now, region) = (world.now, release.region.0);
                    world
                        .cluster
                        .released(now, &release.worker, region, release.epoch);
                    moves += 1;
                }
            }

            let coordinator = &world.cluster.coordinator;
            let under_way = coordinator.under_way();
            assert!(under_way.len() <= 4 && coordinator.splits.len() <= 1);
            let known = |region: &RegionId| coordinator.regions.contains_key(region);
            let noted = &coordinator.noted;
            assert!(
                noted
                    .merges
                    .keys()
                    .all(|(lower, higher)| known(lower) && known(higher))
            );
            assert!(noted.going.keys().all(known));
            assert!(noted.merges.len() <= coordinator.regions.len().pow(2));
            if !walking && look >= 2300 {
                assert!(begun.is_empty(), "seed {seed} at look {look}: {begun:?}");
            }
        }

        // Everybody has stood still for four minutes. Nothing is under way.
        assert!(doing.is_empty() && world.cluster.coordinator.under_way().is_empty());
        let regions: Vec<u32> = world
            .list
            .regions
            .iter()
            .map(|info| info.region.0)
            .collect();
        for (index, one) in players.iter().enumerate() {
            // Who is within the merge distance of each other is in one region, and
            // so is who is that near to where players enter with the home region.
            for other in &players[index + 1..] {
                let near = apart(one.1, other.1) <= 2;
                assert!(!near || one.0 == other.0, "seed {seed}: {one:?} {other:?}");
            }
            assert!(
                apart(one.1, (0, 0)) > 2 || one.0 == 0,
                "seed {seed}: {one:?}"
            );
        }
        let mut without = 0;
        for region in &regions {
            let of_region = players.iter().filter(|player| player.0 == *region);
            let mut chunks: Vec<(i32, i32)> = of_region.map(|player| player.1).collect();
            without += u32::from(chunks.is_empty() && *region != 0);
            // The players of each region hold together within the split distance,
            // in the home region also with where players enter.
            if *region == 0 {
                chunks.push((0, 0));
            }
            assert!(
                joined(&chunks, 5),
                "seed {seed}: region {region} {chunks:?}"
            );
        }
        let at_home = players.iter().any(|player| player.0 == 0);
        assert!(without <= u32::from(at_home), "seed {seed}: {regions:?}");
    }
    // The runs are worth something only if all of it happened in them.
    let counts = [merges, absorptions, splits, moves];
    assert!(counts.iter().all(|count| *count >= 10), "{counts:?}");
}
