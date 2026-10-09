//! The world of the generated runs, as section 11 of
//! `docs/adr/0016-when-to-merge-and-split.md` describes it: players that are points on
//! the grid of chunks, regions that are sets of players, and the store's list, which is
//! what the model made. Nothing here knows of a coordinator.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use clustine_coordinator::{Asked, Coordinator, Policy};
use clustine_region::RegionId;
use clustine_rpc::{Crowds, Off, RegionInfo, RegionList};
use clustine_world::{ChunkArea, ChunkPos};

/// A step of a run, and the time between two looks of the coordinator.
pub const LOOK: Duration = Coordinator::LOOK;

/// The lease of the coordinators of these runs: longer than the longest delay of a
/// worker, as the record asks.
pub const LEASE: Duration = Duration::from_secs(5);

/// The times of the record, in steps of a run. `FRESH` is a second, the rest ten, a
/// region is absorbed when it has been without players for three rests, and the list
/// is read once a lease.
pub const FRESH: u64 = 4;
pub const REST: u64 = 40;
pub const EMPTY_FOR: u64 = 3 * REST;
pub const LIST_EVERY: u64 = 20;

/// How many merges and splits are under way at one time, at most.
pub const AT_ONCE: usize = 4;

/// The longest a worker of the model takes to do what it is told, in steps.
pub const DELAY: u64 = 4;

/// How many steps a player takes over a chunk: one chunk in two seconds.
pub const PACE: u64 = 8;

/// The chunk players enter the world in, and the region that holds it.
pub const ORIGIN: ChunkPos = ChunkPos::new(0, 0);
pub const HOME: RegionId = RegionId(0);

/// The distances of the generated runs: 6 and 12, so the margin is 3.
pub fn policy() -> Policy {
    Policy {
        merge_distance: 6,
        split_distance: 12,
        rest: Duration::from_secs(10),
    }
}

/// How many steps a time is, which has to be a whole number of them.
pub fn steps(time: Duration) -> u64 {
    let (time, look) = (time.as_millis(), LOOK.as_millis());
    assert_eq!(time % look, 0, "{time} ms is not a whole number of steps");
    u64::try_from(time / look).expect("a time of a test")
}

/// Numbers from a seed (SplitMix64), the same on every machine.
#[derive(Debug, Clone)]
pub struct Dice(pub u64);

impl Dice {
    pub fn roll(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut mixed = self.0;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        mixed ^ (mixed >> 31)
    }

    pub fn below(&mut self, bound: u64) -> u64 {
        self.roll() % bound
    }

    /// True in `per_thousand` of a thousand times.
    pub fn chance(&mut self, per_thousand: u64) -> bool {
        self.below(1_000) < per_thousand
    }

    /// A number from `-reach` to `reach`.
    pub fn around(&mut self, reach: i32) -> i32 {
        let width = u64::try_from(2 * reach + 1).expect("a reach is not negative");
        i32::try_from(self.below(width)).expect("a small number") - reach
    }

    pub fn pick<T: Clone>(&mut self, from: &[T]) -> Option<T> {
        if from.is_empty() {
            None
        } else {
            Some(from[self.below(from.len() as u64) as usize].clone())
        }
    }
}

/// How far two chunks are apart: in chunks along the longer of the two axes (section 3).
pub fn apart(one: ChunkPos, other: ChunkPos) -> u64 {
    let along = |one: i32, other: i32| (i64::from(one) - i64::from(other)).unsigned_abs();
    along(one.x, other.x).max(along(one.z, other.z))
}

/// The connected sets of `count` things of which `linked` says which two hold together:
/// for each thing the lowest thing of its set.
fn connected(count: usize, linked: impl Fn(usize, usize) -> bool) -> Vec<usize> {
    let mut set: Vec<usize> = (0..count).collect();
    for first in 0..count {
        if set[first] != first {
            continue;
        }
        let mut reached = vec![first];
        while let Some(one) = reached.pop() {
            // A thing above `first` that is still its own set is in no set yet.
            for (other, of) in set.iter_mut().enumerate().skip(first + 1) {
                if *of == other && linked(one, other) {
                    *of = first;
                    reached.push(other);
                }
            }
        }
    }
    set
}

/// The clusters of section 4.1 for these places, all of them taken as known: two places
/// are linked if they are of one region and at most the split distance apart, or of two
/// regions and at most the merge distance apart. For each place the lowest place of its
/// cluster. Whoever calls this adds the home region's place at the origin.
pub fn clusters(places: &[(RegionId, ChunkPos)]) -> Vec<usize> {
    let policy = policy();
    connected(places.len(), |one, other| {
        let reach = if places[one].0 == places[other].0 {
            policy.split_distance
        } else {
            policy.merge_distance
        };
        apart(places[one].1, places[other].1) <= u64::from(reach)
    })
}

/// The sets of points that are joined by steps of at most `reach`, whoever they are of.
pub fn joined(points: &[ChunkPos], reach: u32) -> Vec<usize> {
    connected(points.len(), |one, other| {
        apart(points[one], points[other]) <= u64::from(reach)
    })
}

/// A player of the model.
pub type Player = u32;

/// Which report is a step old when a player is handed from one region to another (K10):
/// that of the region they left, so that they are in both sightings for one report, or
/// that of the region they came to, so that they are in neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stale {
    Both,
    Neither,
}

/// Where a player is, in which region, and when they last moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Walker {
    pub region: RegionId,
    pub at: ChunkPos,
    /// The step of their last move, if they have moved: the next is `PACE` later at
    /// the earliest.
    pub moved: Option<u64>,
}

/// A living region of the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Land {
    /// The areas it is pinned to. A part has none, and a survivor those of both.
    pub areas: Vec<ChunkArea>,
    /// Its tick, which goes up by one with every step it is run in. A report has it,
    /// so the first report of a region has the tick 1.
    pub tick: u64,
}

impl Land {
    pub fn pinned(&self) -> bool {
        !self.areas.is_empty()
    }
}

/// Who is in a region and where: a report of it as the test keeps it, with the players
/// named, which no worker says.
pub type Members = Vec<(Player, ChunkPos)>;

/// What a worker says of those members: the chunks with players in them, each with how
/// many, ascending.
pub fn crowds(members: &Members) -> Crowds {
    let mut counted: BTreeMap<ChunkPos, u32> = BTreeMap::new();
    for (_, at) in members {
        *counted.entry(*at).or_default() += 1;
    }
    counted.into_iter().collect()
}

/// The regions of a merge or a split.
pub fn regions_of(asked: &Asked) -> Vec<RegionId> {
    match asked {
        Asked::Merge { survivor, absorbed } => vec![*survivor, *absorbed],
        Asked::Split { region } => vec![*region],
    }
}

/// What the coordinator knows, as far as the test can see it from outside: who owns
/// which region with which epoch (the routing table), which regions have no owner, and
/// what is under way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Known {
    pub routes: BTreeMap<RegionId, (String, u64)>,
    pub waiting: Vec<RegionId>,
    pub under_way: Vec<Asked>,
}

impl Known {
    pub fn reserved(&self, region: RegionId) -> bool {
        self.under_way
            .iter()
            .any(|asked| regions_of(asked).contains(&region))
    }

    pub fn splitting(&self) -> bool {
        self.under_way
            .iter()
            .any(|asked| matches!(asked, Asked::Split { .. }))
    }
}

/// The players, the regions and the store's list.
#[derive(Debug, Clone)]
pub struct World {
    /// The step the run is at. Everything in a step happens at one time of the clock.
    pub step: u64,
    pub players: BTreeMap<Player, Walker>,
    /// How many players there have ever been: the next one's name.
    entered: Player,
    pub regions: BTreeMap<RegionId, Land>,
    pub absorbed: Vec<(RegionId, RegionId)>,
    pub next: u32,
    /// Who was where at every step so far, as it was when the reports of that step were
    /// given: `past[step]`.
    past: Vec<BTreeMap<Player, (RegionId, ChunkPos)>>,
    /// When each player was last handed from one region to another.
    pub handed: BTreeMap<Player, u64>,
    /// The regions a merge or a split has changed in this step: the report of such a
    /// region is of after it, never a step old (section 2.2).
    pub reshaped: BTreeSet<RegionId>,
}

impl World {
    /// A world of `regions` regions, the first of which is the home region, with
    /// players at the chunks given, each in the region given. If `pinned`, every region
    /// but the home region is pinned to an area.
    pub fn new(regions: u32, pinned: bool, players: &[(u32, i32, i32)]) -> Self {
        let mut world = Self {
            step: 0,
            players: BTreeMap::new(),
            entered: 0,
            regions: (0..regions)
                .map(|id| {
                    let areas = if pinned && id != HOME.0 {
                        // Which area does not matter to anything here: the coordinator
                        // keeps whether there is one.
                        let west = i32::try_from(id).expect("a small number") * 1_000;
                        vec![ChunkArea {
                            min_x: Some(west),
                            max_x: Some(west + 1_000),
                        }]
                    } else {
                        Vec::new()
                    };
                    (RegionId(id), Land { areas, tick: 0 })
                })
                .collect(),
            absorbed: Vec::new(),
            next: regions,
            past: Vec::new(),
            handed: BTreeMap::new(),
            reshaped: BTreeSet::new(),
        };
        for (region, x, z) in players {
            world.enter(RegionId(*region), ChunkPos::new(*x, *z));
        }
        world.remember();
        world
    }

    /// A player is in the world from now on, in that region.
    pub fn enter(&mut self, region: RegionId, at: ChunkPos) -> Player {
        assert!(
            self.regions.contains_key(&region),
            "region {region} does not live"
        );
        let player = self.entered;
        self.entered += 1;
        self.players.insert(
            player,
            Walker {
                region,
                at,
                moved: None,
            },
        );
        player
    }

    /// A player joins: at the origin, so in the home region.
    pub fn join(&mut self) -> Player {
        self.enter(HOME, ORIGIN)
    }

    pub fn leave(&mut self, player: Player) {
        self.players.remove(&player).expect("a player of the world");
    }

    /// Whether the player may move at this step: a chunk in two seconds at most.
    pub fn may_walk(&self, player: Player) -> bool {
        self.players[&player]
            .moved
            .is_none_or(|moved| self.step >= moved + PACE)
    }

    /// The player moves to a chunk next to theirs.
    pub fn walk(&mut self, player: Player, to: ChunkPos) {
        assert!(self.may_walk(player), "player {player} moves too fast");
        let step = self.step;
        let walker = self
            .players
            .get_mut(&player)
            .expect("a player of the world");
        assert!(apart(walker.at, to) <= 1, "player {player} jumps to {to:?}");
        if walker.at != to {
            walker.at = to;
            walker.moved = Some(step);
        }
    }

    /// The player takes a step towards a chunk, if they may move and are not there.
    pub fn walk_towards(&mut self, player: Player, goal: ChunkPos) {
        let at = self.players[&player].at;
        if at != goal && self.may_walk(player) {
            let next = ChunkPos::new(
                at.x + (goal.x - at.x).signum(),
                at.z + (goal.z - at.z).signum(),
            );
            self.walk(player, next);
        }
    }

    /// The player is handed from the region they are in to another.
    pub fn hand(&mut self, player: Player, to: RegionId) {
        assert!(self.regions.contains_key(&to), "region {to} does not live");
        let walker = self
            .players
            .get_mut(&player)
            .expect("a player of the world");
        assert_ne!(walker.region, to, "player {player} is handed to their own");
        walker.region = to;
        self.handed.insert(player, self.step);
    }

    /// A merge as a worker does it: the absorbed region's players are the survivor's,
    /// which is pinned to what either was pinned to.
    pub fn merge(&mut self, survivor: RegionId, absorbed: RegionId) {
        assert_ne!(
            absorbed, HOME,
            "the store never has the home region absorbed"
        );
        assert_ne!(survivor, absorbed);
        assert!(self.regions.contains_key(&survivor));
        let gone = self.regions.remove(&absorbed).expect("a living region");
        self.regions
            .get_mut(&survivor)
            .expect("a living region")
            .areas
            .extend(gone.areas);
        for walker in self.players.values_mut() {
            if walker.region == absorbed {
                walker.region = survivor;
            }
        }
        self.absorbed.push((absorbed, survivor));
        self.reshaped.insert(survivor);
    }

    /// A split as a worker does it: whoever of the region stands in a chunk named, by
    /// where they truly are at this moment, is of a new region with the store's next
    /// id; or nobody does.
    pub fn split(&mut self, region: RegionId, named: &[ChunkPos]) -> Result<RegionId, Off> {
        assert!(
            self.regions.contains_key(&region),
            "region {region} does not live"
        );
        let part = RegionId(self.next);
        let mut anybody = false;
        for walker in self.players.values_mut() {
            if walker.region == region && named.contains(&walker.at) {
                walker.region = part;
                anybody = true;
            }
        }
        if !anybody {
            return Err(Off::Nobody);
        }
        self.next += 1;
        self.regions.insert(
            part,
            Land {
                areas: Vec::new(),
                tick: 0,
            },
        );
        self.reshaped.extend([region, part]);
        Ok(part)
    }

    /// Who is in the region now, and where.
    pub fn members(&self, region: RegionId) -> Members {
        self.players
            .iter()
            .filter(|(_, walker)| walker.region == region)
            .map(|(player, walker)| (*player, walker.at))
            .collect()
    }

    /// Who was in the region at an earlier step, and where.
    pub fn members_at(&self, step: u64, region: RegionId) -> Members {
        self.past[usize::try_from(step).expect("a step of a run")]
            .iter()
            .filter(|(_, (of, _))| *of == region)
            .map(|(player, (_, at))| (*player, *at))
            .collect()
    }

    /// The report of a region at this step, with the players named: who is in it now,
    /// or, if the report is a step old, who was a step ago. A region that a merge or a
    /// split has changed in this step is never reported a step old.
    pub fn report(&self, region: RegionId, a_step_old: bool) -> Members {
        if a_step_old && !self.reshaped.contains(&region) && self.step > 0 {
            self.members_at(self.step - 1, region)
        } else {
            self.members(region)
        }
    }

    /// The store's list: what the model made.
    pub fn list(&self) -> RegionList {
        RegionList {
            home: HOME,
            regions: self
                .regions
                .iter()
                .map(|(region, land)| RegionInfo {
                    region: *region,
                    epoch: 0,
                    bounds: None,
                    pinned: land.areas.clone(),
                })
                .collect(),
            absorbed: self.absorbed.clone(),
            next: RegionId(self.next),
        }
    }

    /// A step begins.
    pub fn advance(&mut self) {
        self.step += 1;
        self.reshaped.clear();
    }

    /// Who is where is kept for this step: called once in every step, when everything
    /// that changes it has happened and before the reports are given.
    pub fn remember(&mut self) {
        assert_eq!(self.past.len() as u64, self.step, "one memory for a step");
        self.past.push(
            self.players
                .iter()
                .map(|(player, walker)| (*player, (walker.region, walker.at)))
                .collect(),
        );
    }

    /// What holds of the model whatever happens to it: every player is in exactly one
    /// region, which lives; the list has every region there ever was as living or as
    /// absorbed and none as both; and the home region lives.
    pub fn check(&self) {
        for (player, walker) in &self.players {
            assert!(
                self.regions.contains_key(&walker.region),
                "player {player} is in region {}, which does not live",
                walker.region
            );
        }
        let list = self.list();
        assert!(self.regions.contains_key(&list.home), "home does not live");
        let living: BTreeSet<RegionId> = list.regions.iter().map(|info| info.region).collect();
        let gone: BTreeSet<RegionId> = list.absorbed.iter().map(|(gone, _)| *gone).collect();
        assert_eq!(living.len(), list.regions.len(), "a region is listed twice");
        assert_eq!(
            gone.len(),
            list.absorbed.len(),
            "a region was absorbed twice"
        );
        assert!(living.is_disjoint(&gone), "a region lives and was absorbed");
        for id in 0..list.next.0 {
            let region = RegionId(id);
            assert!(
                living.contains(&region) || gone.contains(&region),
                "region {region} is below the next id and neither lives nor was absorbed"
            );
        }
        assert!(living.iter().chain(&gone).all(|region| *region < list.next));
        for (gone, into) in &list.absorbed {
            assert!(
                *into < list.next && gone != into,
                "region {gone} went into region {into}"
            );
            assert!(
                self.members(*gone).is_empty(),
                "region {gone} has players still"
            );
        }
        // Every player is in the members of one region, and in the crowds of that one.
        let mut counted = 0;
        for region in self.regions.keys() {
            let members = self.members(*region);
            let said: u32 = crowds(&members).iter().map(|(_, players)| players).sum();
            assert_eq!(said as usize, members.len());
            counted += members.len();
        }
        assert_eq!(counted, self.players.len());
    }
}
