//! The model alone, with the test in the coordinator's place: a script says what "the
//! coordinator" begins at which look, and the model's one worker does it. So a property
//! can be shown to fail, by a run in which it is broken on purpose, whatever the
//! coordinator there is does; and a run played as the record asks shows that a property
//! does not fail what is right. Each of the six faults that step C4.6 is to catch has
//! its run here, named in the comment of its test.

use std::collections::{BTreeMap, BTreeSet};

use clustine_coordinator::{Asked, Changes, Order, ReshapeOrder, named};
use clustine_region::RegionId;
use clustine_world::ChunkPos;

use super::judge::{Begun, Judge, Kind, Property};
use super::model::{
    Dice, EMPTY_FOR, FRESH, HOME, Known, LIST_EVERY, ORIGIN, PACE, Player, REST, Stale, World,
    crowds, policy,
};

/// A merge or a split that the script has begun, and when and how it is to end.
#[derive(Debug, Clone)]
struct Play {
    asked: Asked,
    chunks: Vec<ChunkPos>,
    due: u64,
    well: bool,
}

/// The model, the judge, and as much of a cluster as the properties look at: one worker
/// that owns every region, reports each at every step and does what the script begins.
pub struct Stage {
    pub world: World,
    pub judge: Judge,
    owners: BTreeMap<RegionId, (String, u64)>,
    epochs: u64,
    plays: Vec<Play>,
    /// The regions that are being released to even out, and when each has its new owner.
    moves: Vec<(RegionId, u64)>,
    /// The regions of which no report is given.
    pub silent: BTreeSet<RegionId>,
    /// The regions whose report of the next step is a step old.
    old: BTreeSet<RegionId>,
    /// Whether the list is handed in, once in `LIST_EVERY`.
    pub reads: bool,
    listed: Option<u64>,
}

impl Stage {
    /// A world as [`World::new`] makes it, at step 0, with nothing reported yet.
    pub fn new(regions: u32, pinned: bool, players: &[(u32, i32, i32)]) -> Self {
        let world = World::new(regions, pinned, players);
        let owners = world
            .regions
            .keys()
            .map(|region| (*region, ("w".to_owned(), 1)))
            .collect();
        Self {
            world,
            judge: Judge::default(),
            owners,
            epochs: 1,
            plays: Vec::new(),
            moves: Vec::new(),
            silent: BTreeSet::new(),
            old: BTreeSet::new(),
            reads: true,
            listed: None,
        }
    }

    pub fn step(&self) -> u64 {
        self.world.step
    }

    fn known(&self) -> Known {
        Known {
            routes: self.owners.clone(),
            waiting: Vec::new(),
            under_way: self.plays.iter().map(|play| play.asked).collect(),
        }
    }

    /// A step, in the order section 11 fixes: the players do what `game` says; the
    /// worker says what came of what; a reading of the list is handed in if one is due;
    /// the reports are given; and then comes the look, at which [`Stage::begin`] says
    /// what the coordinator begins.
    pub fn turn(&mut self, game: impl FnOnce(&mut World)) {
        self.world.advance();
        let step = self.world.step;
        game(&mut self.world);

        let (due, later) = std::mem::take(&mut self.plays)
            .into_iter()
            .partition(|play| play.due <= step);
        self.plays = later;
        for play in due {
            self.end(play);
        }
        let (moved, later) = std::mem::take(&mut self.moves)
            .into_iter()
            .partition(|(_, due)| *due <= step);
        self.moves = later;
        for (region, _) in moved {
            self.epochs += 1;
            self.owners.insert(region, ("v".to_owned(), self.epochs));
        }

        if self.reads && self.listed.is_none_or(|listed| step >= listed + LIST_EVERY) {
            let before = self.known();
            self.judge
                .listed(step, self.world.regions.keys().copied(), &before);
            self.listed = Some(step);
        }
        self.judge.routes(step, &self.owners);

        for land in self.world.regions.values_mut() {
            land.tick += 1;
        }
        self.world.remember();
        let known = self.known();
        for (region, land) in &self.world.regions {
            if self.silent.contains(region) {
                continue;
            }
            let (worker, epoch) = &self.owners[region];
            let members = self.world.report(*region, self.old.contains(region));
            self.judge.given(
                step, &known, true, worker, *region, *epoch, land.tick, members,
            );
        }
        self.old.clear();
        self.judge.look(step, &known);
        self.judge.under_way(step, &known.under_way);
        self.world.check();
    }

    /// Steps in which the players stand.
    pub fn turns(&mut self, steps: u64) {
        for _ in 0..steps {
            self.turn(|_| {});
        }
    }

    /// The first step, and as many more as the regions rest for having been given
    /// their owners.
    pub fn rested(&mut self) {
        self.turns(REST + 1);
    }

    /// The player walks to a chunk, a chunk in two seconds.
    pub fn walk(&mut self, player: Player, goal: ChunkPos) {
        while self.world.players[&player].at != goal {
            self.turn(|world| world.walk_towards(player, goal));
        }
    }

    /// A step in which the player is handed to another region, with one report that is
    /// a step old (K10).
    pub fn hand_over(&mut self, player: Player, to: RegionId, stale: Stale) {
        let from = self.world.players[&player].region;
        self.old.insert(match stale {
            Stale::Both => from,
            Stale::Neither => to,
        });
        self.turn(|world| world.hand(player, to));
    }

    /// The coordinator begins this at the look of this step, as far as the judge can
    /// tell. The worker has done it `lasts` steps later, or has not if it is not to end
    /// well.
    pub fn begin(&mut self, what: Begun, lasts: u64, well: bool) -> Kind {
        let before = self.known();
        let kind = self.judge.begun(&self.world, &before, &what, 0);
        let due = self.world.step + lasts;
        match what {
            Begun::Merge { survivor, absorbed } => self.plays.push(Play {
                asked: Asked::Merge { survivor, absorbed },
                chunks: Vec::new(),
                due,
                well,
            }),
            Begun::Split { region, chunks } => self.plays.push(Play {
                asked: Asked::Split { region },
                chunks,
                due,
                well,
            }),
            Begun::EvenOut { region } => self.moves.push((region, due)),
        }
        let under_way = self.known().under_way;
        self.judge.under_way(self.world.step, &under_way);
        kind
    }

    fn end(&mut self, play: Play) {
        let step = self.world.step;
        let well = match play.asked {
            Asked::Merge { survivor, absorbed } => {
                if play.well {
                    self.world.merge(survivor, absorbed);
                    self.owners.remove(&absorbed);
                }
                play.well
            }
            Asked::Split { region } => {
                let made = play
                    .well
                    .then(|| self.world.split(region, &play.chunks).ok())
                    .flatten();
                if let Some(part) = made {
                    self.epochs += 1;
                    self.owners.insert(part, ("w".to_owned(), self.epochs));
                    self.judge.told_part(part);
                }
                made.is_some()
            }
        };
        self.judge.ended(step, play.asked, well);
    }

    /// The properties that were broken so far.
    pub fn broken(&self) -> BTreeSet<Property> {
        self.judge
            .breaches
            .iter()
            .map(|breach| breach.property)
            .collect()
    }

    /// Fails unless exactly these properties were broken. What was broken is said, to
    /// be read with `--nocapture`.
    #[track_caller]
    pub fn assert_broken(&self, properties: &[Property]) {
        for breach in &self.judge.breaches {
            println!(
                "{:?} at step {}: {}",
                breach.property, breach.step, breach.what
            );
        }
        let expected: BTreeSet<Property> = properties.iter().copied().collect();
        assert_eq!(
            self.broken(),
            expected,
            "the breaches: {:#?}",
            self.judge.breaches
        );
    }

    fn seen(&self, what: &str) -> u32 {
        self.judge.seen.get(what).copied().unwrap_or(0)
    }
}

fn chunk(x: i32, z: i32) -> ChunkPos {
    ChunkPos::new(x, z)
}

fn region(id: u32) -> RegionId {
    RegionId(id)
}

/// The chunks a split names for groups at these chunks (section 4.3).
fn around(groups: &[&[(i32, i32)]]) -> Vec<ChunkPos> {
    let groups: Vec<Vec<ChunkPos>> = groups
        .iter()
        .map(|group| group.iter().map(|(x, z)| chunk(*x, *z)).collect())
        .collect();
    let groups: Vec<&[ChunkPos]> = groups.iter().map(|group| group.as_slice()).collect();
    named(&policy(), &groups)
}

fn merge(survivor: u32, absorbed: u32) -> Begun {
    Begun::Merge {
        survivor: region(survivor),
        absorbed: region(absorbed),
    }
}

fn split(of: u32, groups: &[&[(i32, i32)]]) -> Begun {
    Begun::Split {
        region: region(of),
        chunks: around(groups),
    }
}

fn even_out(of: u32) -> Begun {
    Begun::EvenOut { region: region(of) }
}

// ---------------------------------------------------------------------------------
// The model's own invariants.
// ---------------------------------------------------------------------------------

/// Whatever is done to the model, every player is in exactly one region, the list has
/// what the model made, and a report is true: the crowds of who is in the region at that
/// step, or of who was a step before if it is a step old. Merges, splits, hand-overs,
/// joins and departures are thrown at it without any sense; what the properties make of
/// that is not looked at.
#[test]
fn the_model_keeps_every_player_in_one_region_its_list_true_and_its_reports_true() {
    for seed in 1..=20 {
        let mut dice = Dice(seed);
        let mut stage = Stage::new(3, seed % 2 == 0, &[(0, 0, 0), (1, 20, 0), (2, -20, 4)]);
        let mut goals: BTreeMap<Player, ChunkPos> = BTreeMap::new();
        for _ in 0..400 {
            let players: Vec<Player> = stage.world.players.keys().copied().collect();
            let regions: Vec<RegionId> = stage.world.regions.keys().copied().collect();
            let busy: Vec<RegionId> = stage
                .known()
                .under_way
                .iter()
                .flat_map(super::model::regions_of)
                .collect();
            let free: Vec<RegionId> = regions
                .iter()
                .filter(|region| !busy.contains(region))
                .copied()
                .collect();
            // What the coordinator of this run begins, by the dice.
            match dice.below(12) {
                0 => {
                    let (one, other) = (dice.pick(&free), dice.pick(&free));
                    if let (Some(survivor), Some(absorbed)) = (one, other)
                        && survivor != absorbed
                        && absorbed != HOME
                    {
                        let lasts = dice.below(5);
                        stage.begin(Begun::Merge { survivor, absorbed }, lasts, dice.chance(800));
                    }
                }
                1 => {
                    if let Some(of) = dice.pick(&free)
                        && let Some((_, at)) = dice.pick(&stage.world.members(of))
                    {
                        let chunks = named(&policy(), &[&[at]]);
                        let lasts = dice.below(5);
                        stage.begin(Begun::Split { region: of, chunks }, lasts, dice.chance(800));
                    }
                }
                _ => {}
            }
            // A report that is a step old, of a region or two.
            let mut old = BTreeSet::new();
            for of in &regions {
                if dice.chance(150) {
                    old.insert(*of);
                }
            }
            stage.old = old.clone();
            let before = stage.world.clone();
            stage.turn(|world| {
                for player in &players {
                    let goal = *goals
                        .entry(*player)
                        .or_insert_with(|| chunk(dice.around(30), dice.around(30)));
                    if world.players[player].at == goal {
                        goals.remove(player);
                    } else if dice.chance(700) {
                        world.walk_towards(*player, goal);
                    }
                }
                if dice.chance(30) && world.players.len() < 12 {
                    world.join();
                }
                if dice.chance(20)
                    && let Some(player) = dice.pick(&players)
                {
                    world.leave(player);
                }
                if dice.chance(40)
                    && let Some(player) = dice.pick(&players)
                    && world.players.contains_key(&player)
                    && let Some(to) = dice.pick(&regions)
                    && world.players[&player].region != to
                {
                    world.hand(player, to);
                }
            });
            let step = stage.step();

            // Nobody moved further than a chunk, and nobody sooner than two seconds
            // after their last move.
            for (player, walker) in &stage.world.players {
                if let Some(was) = before.players.get(player) {
                    assert!(super::model::apart(was.at, walker.at) <= 1, "seed {seed}");
                    if was.at != walker.at {
                        assert!(
                            was.moved.is_none_or(|moved| step >= moved + PACE),
                            "seed {seed}"
                        );
                    }
                }
            }
            // The reports of the step are true.
            let look = stage.judge.look_at(step).expect("the look of this step");
            let mut reported = 0;
            for (of, given) in &look.reports {
                let a_step_old = old.contains(of) && !stage.world.reshaped.contains(of);
                let truly = if a_step_old {
                    stage.world.members_at(step - 1, *of)
                } else {
                    stage.world.members(*of)
                };
                assert_eq!(
                    crowds(&given.members),
                    crowds(&truly),
                    "seed {seed}, step {step}: the report of {of}"
                );
                if !a_step_old {
                    reported += given.members.len();
                }
            }
            if old.is_empty() {
                assert_eq!(
                    reported,
                    stage.world.players.len(),
                    "seed {seed}, step {step}"
                );
            }
            assert_eq!(
                look.reports.keys().collect::<Vec<_>>(),
                stage.world.regions.keys().collect::<Vec<_>>(),
                "seed {seed}, step {step}: a report of every region that lives"
            );
            // And the list is of the regions there are.
            let list = stage.world.list();
            assert_eq!(list.home, HOME);
            assert_eq!(
                list.regions
                    .iter()
                    .map(|info| info.region)
                    .collect::<Vec<_>>(),
                stage.world.regions.keys().copied().collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn a_split_of_the_model_takes_who_stands_in_the_chunks_named_and_a_merge_gives_them_back() {
    let mut world = World::new(2, true, &[(0, 0, 0), (0, 20, 0), (0, 21, 1), (1, 40, 0)]);
    // Nobody stands in the chunks named: nothing is made, and no id is used.
    assert!(world.split(HOME, &around(&[&[(-20, 0)]])).is_err());
    assert_eq!(world.list().next, region(2));
    let part = world
        .split(HOME, &around(&[&[(20, 0)]]))
        .expect("two players stand there");
    assert_eq!(part, region(2));
    assert_eq!(world.members(part), [(1, chunk(20, 0)), (2, chunk(21, 1))]);
    assert_eq!(world.members(HOME), [(0, ORIGIN)]);
    assert!(!world.regions[&part].pinned(), "a part is never pinned");
    world.check();
    // A survivor is pinned if either of the two was.
    world.merge(part, region(1));
    assert!(world.regions[&part].pinned());
    assert_eq!(world.list().absorbed, [(region(1), part)]);
    assert_eq!(world.members(part).len(), 3);
    world.check();
}

// ---------------------------------------------------------------------------------
// Runs played as the record asks: no property fails them.
// ---------------------------------------------------------------------------------

/// K4, across the whole band and back: a merge when two regions have been within the
/// merge distance for more than a second, a split when the players have been further
/// apart than the split distance for more than a second and the region has rested, and a
/// merge again.
#[test]
fn a_merge_a_split_and_a_merge_each_after_it_has_stood_and_its_regions_have_rested_break_nothing() {
    let mut stage = Stage::new(2, false, &[(0, 0, 0), (1, 8, 0)]);
    stage.rested();
    stage.walk(1, chunk(6, 0));
    stage.turns(FRESH + 1);
    assert_eq!(stage.begin(merge(0, 1), 3, true), Kind::Distances);
    stage.turns(3);
    assert_eq!(stage.world.players[&1].region, HOME);

    stage.walk(1, chunk(13, 0));
    stage.turns(FRESH + 1);
    assert_eq!(stage.begin(split(0, &[&[(13, 0)]]), 2, true), Kind::Split);
    stage.turns(2);
    assert_eq!(stage.world.players[&1].region, region(2));

    stage.walk(1, chunk(6, 0));
    stage.turns(FRESH + 1);
    stage.begin(merge(0, 2), 3, true);
    stage.turns(3);
    assert_eq!(stage.world.players[&1].region, HOME);

    stage.assert_broken(&[]);
    assert_eq!(stage.seen("merges by the distances that R4 (e) checked"), 2);
    assert_eq!(stage.seen("splits that R4 (e) checked"), 1);
}

/// K22 as a coordinator is to play it: the group that has stood goes, and the group
/// that is there since this look stays.
#[test]
fn a_split_that_names_only_the_group_that_has_stood_breaks_nothing() {
    let mut stage = k22();
    stage.begin(split(0, &[&[(20, 0)]]), 2, true);
    stage.turns(2);
    stage.assert_broken(&[]);
    assert_eq!(stage.seen("splits that R4 (e) checked"), 1);
    // The players in the north are the home region's still.
    assert_eq!(stage.world.players[&2].region, HOME);
    assert_eq!(stage.world.players[&1].region, region(2));
}

/// An empty region that is absorbed when it has been without players for three rests,
/// by a home region without players; and by the lower of two when the home region has
/// somebody.
#[test]
fn an_absorption_of_a_region_that_was_empty_for_three_rests_breaks_nothing() {
    let mut stage = Stage::new(3, false, &[(1, 30, 0)]);
    stage.turns(60);
    stage.turn(|world| world.leave(0));
    stage.turns(EMPTY_FOR + 1);
    assert_eq!(stage.begin(merge(0, 2), 2, true), Kind::Absorption);
    stage.turns(2 + FRESH + 2);
    // The survivor rests for nobody: the next goes into it a second and a half later.
    assert_eq!(stage.begin(merge(0, 1), 2, true), Kind::Absorption);
    stage.turns(2);
    stage.assert_broken(&[]);
    assert_eq!(stage.seen("absorptions begun at a plain look"), 2);

    let mut stage = Stage::new(3, false, &[(0, 0, 0)]);
    stage.turns(EMPTY_FOR + 2);
    assert_eq!(stage.begin(merge(1, 2), 2, true), Kind::Absorption);
    stage.turns(2);
    stage.assert_broken(&[]);
}

// ---------------------------------------------------------------------------------
// Runs in which a property is broken on purpose.
// ---------------------------------------------------------------------------------

/// The fault "no rest" of step C4.6: a group is split off, and the part is merged with
/// the region that stands near it as soon as that merge has stood, a second later.
#[test]
fn a_merge_within_a_rest_of_the_split_that_made_its_region_is_caught_by_r1_and_by_r2s_count() {
    let mut stage = Stage::new(2, false, &[(0, 0, 0), (0, 13, 0), (1, 18, 0)]);
    stage.rested();
    stage.begin(split(0, &[&[(13, 0)]]), 2, true);
    stage.turns(2);
    assert_eq!(stage.world.players[&1].region, region(2));
    stage.assert_broken(&[]);
    // The merge of the part with its neighbour has stood: R4 (e) has nothing to say.
    stage.turns(FRESH + 1);
    stage.begin(merge(1, 2), 3, true);
    stage.assert_broken(&[Property::R1, Property::R2]);
    assert_eq!(stage.seen("merges by the distances that R4 (e) checked"), 1);
}

#[test]
fn a_release_to_even_out_within_a_rest_of_a_region_being_given_its_owner_is_caught_by_r1() {
    let mut stage = Stage::new(2, false, &[(0, 0, 0), (1, 30, 0)]);
    stage.turns(REST);
    stage.begin(even_out(1), 2, true);
    stage.assert_broken(&[Property::R1]);

    // A step later the rest is over.
    let mut stage = Stage::new(2, false, &[(0, 0, 0), (1, 30, 0)]);
    stage.rested();
    stage.begin(even_out(1), 2, true);
    stage.assert_broken(&[]);
    // And the region rests from when its new owner has it.
    stage.turns(2 + REST - 1);
    stage.begin(even_out(1), 2, true);
    stage.assert_broken(&[Property::R1]);
}

/// A region is made the survivor of an absorption at any time, and the end of an
/// absorption is no end that a rest counts from; but what the survivor owed before, it
/// owes still, and it rests if the first report after the end has somebody in it.
#[test]
fn an_absorption_neither_ends_a_rest_of_its_survivor_nor_begins_one_unless_somebody_came() {
    // Two regions merge; their players leave; the survivor takes in an empty region
    // while it rests, which breaks nothing.
    let after_a_merge = || {
        let mut stage = Stage::new(4, false, &[(1, 100, 0), (2, 104, 0)]);
        stage.turns(EMPTY_FOR);
        stage.begin(merge(1, 2), 2, true);
        stage.turns(2);
        stage.turn(|world| {
            world.leave(0);
            world.leave(1);
        });
        stage.turns(FRESH + 1);
        assert_eq!(stage.begin(merge(1, 3), 2, true), Kind::Absorption);
        stage.turns(2);
        stage.assert_broken(&[]);
        stage
    };
    // Something by itself with the survivor, within a rest of the merge by the
    // distances that it survived before: the rest is still owed.
    let mut stage = after_a_merge();
    stage.turns(2);
    stage.begin(even_out(1), 2, true);
    stage.assert_broken(&[Property::R1]);
    // When that rest is over, nothing is owed for the absorption.
    let mut stage = after_a_merge();
    stage.turns(REST);
    stage.begin(even_out(1), 2, true);
    stage.assert_broken(&[]);

    // Somebody comes into the survivor as the absorption ends, and is in the first
    // report after it: the survivor rests from that report.
    let absorbed = |comes_after: u64| {
        let mut stage = Stage::new(3, false, &[(2, 50, 0)]);
        stage.turns(EMPTY_FOR + REST);
        assert_eq!(stage.begin(merge(0, 1), 2, true), Kind::Absorption);
        stage.turns(1 + comes_after);
        stage.turn(|world| world.hand(0, HOME));
        stage.turns(REST - 1 - comes_after);
        stage.begin(even_out(0), 2, true);
        stage
    };
    absorbed(0).assert_broken(&[Property::R1]);
    // In the second report after the end, they begin no rest.
    absorbed(1).assert_broken(&[]);
}

#[test]
fn a_player_who_was_handed_over_is_left_out_of_r2s_count_for_a_rest() {
    let mut stage = Stage::new(3, false, &[(1, 100, 0), (1, 113, 0), (2, 200, 0)]);
    stage.rested();
    stage.turns(FRESH + 1);
    stage.begin(split(1, &[&[(113, 0)]]), 2, true);
    stage.turns(2);
    // Player 0 stood still for the split, is handed to region 2, and that is moved.
    stage.hand_over(0, region(2), Stale::Both);
    stage.turns(2);
    stage.begin(even_out(2), 2, true);
    stage.assert_broken(&[]);

    // Without the hand-over, a second thing with the player's region is counted.
    let mut stage = Stage::new(3, false, &[(1, 100, 0), (1, 113, 0), (2, 200, 0)]);
    stage.rested();
    stage.turns(FRESH + 1);
    stage.begin(split(1, &[&[(113, 0)]]), 2, true);
    stage.turns(5);
    stage.begin(even_out(1), 2, true);
    stage.assert_broken(&[Property::R1, Property::R2]);
}

/// Two regions whose players are within the merge distance of each other, far from the
/// origin, and a third without anybody.
fn two_near() -> Stage {
    Stage::new(4, false, &[(1, 30, 0), (2, 34, 0)])
}

#[test]
fn a_merge_of_a_region_of_which_no_report_was_taken_for_a_second_is_caught_by_r3() {
    let mut stage = two_near();
    stage.rested();
    stage.silent.insert(region(2));
    stage.turns(FRESH);
    stage.begin(merge(1, 2), 3, true);
    stage.assert_broken(&[]);

    let mut stage = two_near();
    stage.rested();
    stage.silent.insert(region(2));
    stage.turns(FRESH + 1);
    stage.begin(merge(1, 2), 3, true);
    stage.assert_broken(&[Property::R3]);
}

/// The fault "a survivor that is not home" of step C4.6, if the merge is begun.
#[test]
fn a_merge_that_names_the_home_region_as_the_one_to_absorb_is_caught_by_r3() {
    let mut stage = Stage::new(2, false, &[(0, 0, 0), (1, 4, 0), (1, 5, 0)]);
    stage.rested();
    stage.turns(FRESH + 1);
    stage.begin(merge(1, 0), 3, false);
    stage.assert_broken(&[Property::R3]);
}

/// The fault "a pinned region absorbed for being empty" of step C4.6.
#[test]
fn an_absorption_of_a_pinned_region_is_caught_by_r3_and_by_r4b() {
    let mut stage = Stage::new(2, true, &[]);
    stage.turns(EMPTY_FOR + 2);
    assert_eq!(stage.begin(merge(0, 1), 2, true), Kind::Absorption);
    stage.assert_broken(&[Property::R3, Property::R4b]);

    // A pinned region is merged by the distances like any other.
    let mut stage = Stage::new(2, true, &[(0, 0, 0), (1, 5, 0)]);
    stage.rested();
    stage.turns(FRESH + 1);
    assert_eq!(stage.begin(merge(0, 1), 2, true), Kind::Distances);
    stage.assert_broken(&[]);
}

#[test]
fn a_fifth_merge_under_way_is_caught_by_r3() {
    let players: Vec<(u32, i32, i32)> = (1..=10)
        .map(|id| (id, 100 * ((id as i32 + 1) / 2) + 4 * (id as i32 % 2), 0))
        .collect();
    let mut stage = Stage::new(11, false, &players);
    stage.rested();
    stage.turns(FRESH + 1);
    for pair in 1..=4 {
        stage.begin(merge(2 * pair - 1, 2 * pair), 4, true);
    }
    stage.assert_broken(&[]);
    stage.begin(merge(9, 10), 4, true);
    stage.assert_broken(&[Property::R3]);
}

#[test]
fn a_split_begun_while_another_is_under_way_is_caught_by_r3() {
    let mut stage = Stage::new(
        3,
        false,
        &[(1, 100, 0), (1, 120, 0), (2, 200, 0), (2, 220, 0)],
    );
    stage.rested();
    stage.turns(FRESH + 1);
    stage.begin(split(1, &[&[(120, 0)]]), 4, true);
    stage.turns(1);
    stage.begin(split(2, &[&[(220, 0)]]), 4, true);
    stage.assert_broken(&[Property::R3]);
}

/// The fault "the list never read on the timer" of step C4.6, if what holds back for
/// the age of the last reading went with the timer.
#[test]
fn what_is_begun_more_than_two_readings_after_the_last_or_before_the_first_is_caught_by_r3() {
    let begun_after = |steps: u64| {
        let mut stage = two_near();
        stage.rested();
        // The list was handed in at the steps 1, 21 and 41, and is no more.
        stage.reads = false;
        stage.turns(steps);
        stage.begin(merge(1, 2), 3, true);
        stage.broken()
    };
    assert_eq!(begun_after(2 * LIST_EVERY), BTreeSet::new());
    assert_eq!(
        begun_after(2 * LIST_EVERY + 1),
        BTreeSet::from([Property::R3])
    );

    let mut stage = two_near();
    stage.reads = false;
    stage.rested();
    stage.turns(FRESH + 1);
    stage.begin(merge(1, 2), 3, true);
    stage.assert_broken(&[Property::R3]);
}

#[test]
fn what_is_begun_while_a_region_of_the_list_was_never_reported_is_caught_by_r3() {
    let mut stage = two_near();
    stage.silent.insert(region(3));
    stage.rested();
    stage.turns(FRESH + 1);
    stage.begin(merge(1, 2), 3, true);
    stage.assert_broken(&[Property::R3]);

    // A part of which the coordinator was told is no such region: a sighting was made
    // for it when the split ended (section 2.4).
    let mut stage = Stage::new(
        4,
        false,
        &[(1, 30, 0), (2, 34, 0), (3, 100, 0), (3, 120, 0)],
    );
    stage.rested();
    stage.turns(FRESH + 1);
    stage.begin(split(3, &[&[(120, 0)]]), 1, true);
    stage.silent.insert(region(4));
    stage.turns(LIST_EVERY + 1);
    assert!(stage.world.regions.contains_key(&region(4)));
    stage.begin(merge(1, 2), 3, true);
    stage.assert_broken(&[]);
}

#[test]
fn a_merge_of_regions_whose_players_were_never_near_each_other_is_caught_by_r4a() {
    let mut stage = Stage::new(3, false, &[(1, 30, 0), (2, 37, 0)]);
    stage.rested();
    stage.turns(FRESH + 1);
    stage.begin(merge(1, 2), 3, true);
    stage.assert_broken(&[Property::R4a, Property::R4e]);

    // A player who was near two seconds ago and has walked off is enough for R4 (a):
    // the reports the merge stood on are up to a second old, and then some.
    let mut stage = Stage::new(3, false, &[(1, 30, 0), (2, 36, 0)]);
    stage.rested();
    stage.turns(FRESH + 1);
    stage.turn(|world| world.walk(1, chunk(37, 0)));
    stage.turns(2 * FRESH - 1);
    stage.begin(merge(1, 2), 3, true);
    assert!(!stage.broken().contains(&Property::R4a));
    stage.turn(|_| {});
    stage.begin(merge(1, 2), 3, true);
    assert!(stage.broken().contains(&Property::R4a));
}

#[test]
fn an_absorption_into_the_higher_region_or_of_a_region_that_had_a_player_is_caught_by_r4b() {
    // The survivor is neither the home region nor the lower of the two.
    let mut stage = Stage::new(3, false, &[]);
    stage.turns(EMPTY_FOR + 2);
    assert_eq!(stage.begin(merge(2, 1), 2, true), Kind::Absorption);
    stage.assert_broken(&[Property::R4b]);

    // The absorbed region had a player in a report less than `EMPTY_FOR` ago.
    let left_before = |steps: u64| {
        let mut stage = Stage::new(2, false, &[(1, 30, 0)]);
        stage.turns(60);
        stage.turn(|world| world.leave(0));
        stage.turns(steps);
        assert_eq!(stage.begin(merge(0, 1), 2, true), Kind::Absorption);
        stage.broken()
    };
    // The last report with the player in it is of the step before they left.
    assert_eq!(left_before(EMPTY_FOR - 1), BTreeSet::from([Property::R4b]));
    assert_eq!(left_before(EMPTY_FOR), BTreeSet::new());

    // The survivor has not been without players for more than a second: its first
    // report without the player is of the step at which they left.
    let left_before = |steps: u64| {
        let mut stage = Stage::new(2, false, &[(0, 0, 0)]);
        stage.turns(EMPTY_FOR + 2);
        stage.turn(|world| world.leave(0));
        stage.turns(steps);
        assert_eq!(stage.begin(merge(0, 1), 2, true), Kind::Absorption);
        stage.broken()
    };
    assert_eq!(left_before(FRESH), BTreeSet::from([Property::R4b]));
    assert_eq!(left_before(FRESH + 1), BTreeSet::new());
}

/// Section 4.4: a region that a merge is wanted of is no survivor. Somebody of another
/// region stands within the merge distance of the origin, and the home region, which
/// is without players, takes in an empty region all the same.
#[test]
fn an_absorption_into_a_home_region_that_a_merge_is_wanted_of_is_caught_by_r4b() {
    let mut stage = Stage::new(3, false, &[(1, 5, 0)]);
    stage.turns(EMPTY_FOR + 2);
    assert_eq!(stage.begin(merge(0, 2), 2, true), Kind::Absorption);
    stage.assert_broken(&[Property::R4b]);
}

/// K2 (b) and D15 with the distances 6 and 12: two sets of players of region 1, 22
/// apart, and two players of region 2 between them, each within 6 of one set and 10 from
/// each other. Region 1 is whole, and a split of it is wrong.
fn joined_by_another() -> Stage {
    let mut stage = Stage::new(
        3,
        false,
        &[(1, 100, 0), (1, 122, 0), (2, 106, 0), (2, 116, 0)],
    );
    stage.rested();
    stage.turns(FRESH + 1);
    stage
}

#[test]
fn a_split_of_a_region_whose_groups_players_of_another_region_join_is_caught_by_r4c() {
    let mut stage = joined_by_another();
    stage.begin(split(1, &[&[(122, 0)]]), 2, true);
    stage.assert_broken(&[Property::R4c, Property::R4e]);

    // Where the test does not know what the coordinator has heard of the other region,
    // it checks what follows from the region's own report, and that is not broken: this
    // split is caught at plain looks only.
    let mut stage = joined_by_another();
    stage.silent.insert(region(2));
    stage.turn(|_| {});
    stage.begin(split(1, &[&[(122, 0)]]), 2, true);
    stage.assert_broken(&[]);
    assert_eq!(stage.seen("splits that R4 (e) could not check"), 1);

    // And there a split that takes a player within the split distance of one who stays
    // is caught still.
    let mut stage = Stage::new(3, false, &[(1, 100, 0), (1, 110, 0), (2, 200, 0)]);
    stage.rested();
    stage.silent.insert(region(2));
    stage.turns(2);
    stage.begin(split(1, &[&[(110, 0)]]), 2, true);
    stage.assert_broken(&[Property::R4c]);
}

#[test]
fn a_split_of_the_home_region_that_takes_who_is_near_the_origin_is_caught_by_r4c() {
    let mut stage = Stage::new(1, false, &[(0, 10, 0)]);
    stage.rested();
    stage.turns(FRESH + 1);
    stage.begin(split(0, &[&[(10, 0)]]), 2, true);
    stage.assert_broken(&[Property::R4c, Property::R4e]);
}

#[test]
fn a_split_that_parts_two_players_whom_a_merge_had_joined_while_all_stand_is_caught_by_r4d() {
    let mut stage = Stage::new(3, false, &[(1, 100, 0), (2, 106, 0)]);
    stage.rested();
    stage.judge.stand_still(stage.step());
    stage.turns(FRESH + 1);
    stage.begin(merge(1, 2), 2, true);
    stage.turns(2 + REST);
    stage.assert_broken(&[]);
    stage.begin(split(1, &[&[(106, 0)]]), 2, true);
    assert!(
        stage.broken().contains(&Property::R4d),
        "{:?}",
        stage.broken()
    );

    // What R4 (d) leaves out: a region with a group that is to go takes in a region
    // that stands near the players who stay, and the split that follows parts that
    // region's players from the group. They were never joined.
    let mut stage = Stage::new(
        3,
        false,
        &[(1, 100, 0), (1, 120, 0), (2, 96, 0), (2, 97, 0)],
    );
    stage.rested();
    stage.judge.stand_still(stage.step());
    stage.turns(FRESH + 1);
    stage.begin(merge(2, 1), 2, true);
    stage.turns(2 + REST);
    stage.begin(split(2, &[&[(120, 0)]]), 2, true);
    stage.turns(2);
    stage.assert_broken(&[]);
}

/// The fault "no standing" of step C4.6, on the one stale report of K10. Player 1 of
/// region 1 is handed to region 2, and for one report is in both: region 1's is a step
/// old. A place of each region in one chunk; a look later no player of the one is
/// within the merge distance of a player of the other.
fn k10() -> Stage {
    let mut stage = Stage::new(3, false, &[(1, 30, 0), (1, 40, 0), (2, 48, 0)]);
    stage.rested();
    stage.hand_over(1, region(2), Stale::Both);
    stage
}

#[test]
fn a_merge_begun_on_the_one_stale_report_of_a_hand_over_is_caught_by_r4e_and_not_by_r4a() {
    let mut stage = k10();
    stage.begin(merge(1, 2), 3, true);
    stage.assert_broken(&[Property::R4e]);
    assert_eq!(stage.seen("merges by the distances that R4 (e) checked"), 1);
}

#[test]
fn a_coordinator_that_waits_out_the_stale_report_of_a_hand_over_breaks_nothing() {
    let mut stage = k10();
    stage.turns(FRESH + 2);
    // A look later nothing is wanted of the two, and nothing is begun. When the player
    // walks up to the region they left, the merge is wanted for good.
    stage.walk(1, chunk(36, 0));
    stage.turns(FRESH + 1);
    stage.begin(merge(1, 2), 3, true);
    stage.assert_broken(&[]);
    assert_eq!(stage.seen("merges by the distances that R4 (e) checked"), 1);
}

/// K22 up to the look at which the home region's rest ends. The home region has a
/// player at the origin, a group far east that has stood since the start, players far
/// north (player 2) and a player between those and the origin (player 3), who is handed
/// to region 1 at this look and is in neither report.
fn k22() -> Stage {
    let mut stage = Stage::new(
        2,
        false,
        &[(0, 0, 0), (0, 20, 0), (0, 0, -20), (0, 0, -10), (1, -40, 0)],
    );
    stage.rested();
    stage.hand_over(3, region(1), Stale::Neither);
    stage
}

/// The fault "a group that goes on its first look" of step C4.6.
#[test]
fn a_group_named_at_the_look_it_appears_with_one_that_has_stood_is_caught_by_r4e_and_not_by_r4c() {
    let mut stage = k22();
    stage.begin(split(0, &[&[(20, 0)], &[(0, -20)]]), 2, true);
    stage.assert_broken(&[Property::R4e]);
    assert_eq!(stage.seen("splits that R4 (e) checked"), 1);
}

#[test]
fn a_split_that_names_chunks_which_are_not_those_around_groups_that_are_to_go_is_caught_by_r4e() {
    // More than the margin around the group.
    let mut stage = Stage::new(1, false, &[(0, 0, 0), (0, 20, 0)]);
    stage.rested();
    stage.turns(FRESH + 1);
    let mut chunks = around(&[&[(20, 0)]]);
    chunks.push(chunk(24, 0));
    stage.begin(
        Begun::Split {
            region: HOME,
            chunks,
        },
        2,
        true,
    );
    stage.assert_broken(&[Property::R4e]);

    // A group that moves a chunk in two seconds keeps its time (section 5.3).
    let mut stage = Stage::new(1, false, &[(0, 0, 0), (0, 20, 0)]);
    stage.rested();
    stage.walk(1, chunk(22, 0));
    stage.begin(split(0, &[&[(22, 0)]]), 2, true);
    stage.assert_broken(&[]);
}

/// The R5 of a stage: the players stand from now, what is left is counted, the script
/// plays the coordinator until the deadline, and the end is to be reached then.
fn settle(stage: &mut Stage, runs: &[u32]) {
    stage.judge.stand_still(stage.step());
    stage.judge.settle(&stage.world, runs, None);
}

fn at_the_deadline(stage: &mut Stage) {
    let deadline = stage.judge.settling.as_ref().expect("settled").deadline();
    stage.turns(deadline - stage.step());
    let known = stage.known();
    stage
        .judge
        .end(&stage.world, &known, !stage.moves.is_empty());
}

/// The faults "the list never read on the timer" and "a survivor that is not home" of
/// step C4.6, where the coordinator begins nothing for it: two regions that are to be
/// merged stay two.
#[test]
fn a_coordinator_that_never_begins_what_is_left_to_do_is_caught_by_r5() {
    let mut stage = Stage::new(2, false, &[(0, 0, 0), (1, 5, 0), (1, 5, 1)]);
    stage.rested();
    settle(&mut stage, &[2]);
    let settling = stage.judge.settling.clone().expect("settled");
    assert_eq!((settling.n, settling.e, settling.k), (1, 0, 0));
    at_the_deadline(&mut stage);
    stage.assert_broken(&[Property::R5]);

    // The same when it is begun: the end is reached.
    let mut stage = Stage::new(2, false, &[(0, 0, 0), (1, 5, 0), (1, 5, 1)]);
    stage.rested();
    settle(&mut stage, &[2]);
    stage.turns(FRESH + 1);
    stage.begin(merge(0, 1), 3, true);
    at_the_deadline(&mut stage);
    stage.assert_broken(&[]);
}

#[test]
fn what_is_left_to_do_is_counted_by_groups_regions_and_clusters() {
    // The home region with a group that is to go, and two regions that are to merge
    // with each other; an empty region that is pinned and one that is not.
    let mut stage = Stage::new(3, true, &[(0, 0, 0), (0, 30, 0), (1, 100, 0), (2, 105, 0)]);
    stage.rested();
    stage.begin(split(0, &[&[(30, 0)]]), 1, true);
    stage.turns(1 + REST);
    stage.turn(|world| world.hand(1, HOME));
    let part = region(3);
    assert!(stage.world.members(part).is_empty());
    assert!(!stage.world.regions[&part].pinned());
    stage.judge.breaches.clear();
    settle(&mut stage, &[4, 0, 2]);
    let settling = stage.judge.settling.clone().expect("settled");
    // One group of the home region beyond its first, and one pair to be joined; the
    // part is without players. Two regions have to change workers for 4, 0 and 2 to
    // become 2, 2 and 2.
    assert_eq!((settling.n, settling.e, settling.k), (2, 1, 2));
}

#[test]
fn more_merges_and_splits_than_were_left_to_do_or_one_that_comes_to_nothing_is_caught_by_r5() {
    // Nothing is left to do, and a merge is begun.
    let mut stage = Stage::new(3, false, &[(1, 30, 0), (2, 37, 0)]);
    stage.rested();
    settle(&mut stage, &[3]);
    stage.turns(FRESH + 1);
    stage.begin(merge(1, 2), 3, true);
    assert!(stage.broken().contains(&Property::R5));

    // One is left to do, is begun, and comes to nothing.
    let mut stage = Stage::new(3, false, &[(1, 30, 0), (2, 34, 0)]);
    stage.rested();
    settle(&mut stage, &[3]);
    stage.turns(FRESH + 1);
    stage.begin(merge(1, 2), 3, false);
    stage.assert_broken(&[]);
    stage.turns(3);
    stage.assert_broken(&[Property::R5]);
}

/// A build that evens out for ever fails R5 at its release number `k + n + e + 1`.
#[test]
fn a_coordinator_that_evens_out_for_ever_is_caught_by_r5() {
    let mut stage = Stage::new(3, false, &[(0, 0, 0), (1, 30, 0), (2, 60, 0)]);
    stage.rested();
    settle(&mut stage, &[3, 0]);
    let settling = stage.judge.settling.clone().expect("settled");
    assert_eq!((settling.n, settling.e, settling.k), (0, 0, 1));
    stage.begin(even_out(2), 2, true);
    stage.turns(2 + REST);
    stage.assert_broken(&[]);
    stage.begin(even_out(2), 2, true);
    stage.assert_broken(&[Property::R5]);
}

#[test]
fn a_region_without_players_that_is_left_where_it_should_have_been_absorbed_is_caught_by_r5() {
    // The home region has nobody: no region without players is left but a pinned one.
    let mut stage = Stage::new(2, false, &[]);
    stage.rested();
    settle(&mut stage, &[2]);
    assert_eq!(stage.judge.settling.as_ref().expect("settled").e, 1);
    at_the_deadline(&mut stage);
    stage.assert_broken(&[Property::R5]);

    let mut stage = Stage::new(2, true, &[]);
    stage.rested();
    settle(&mut stage, &[2]);
    at_the_deadline(&mut stage);
    stage.assert_broken(&[]);

    // The home region has somebody: the lowest region without players stays, and no
    // other.
    let mut stage = Stage::new(3, false, &[(0, 0, 0)]);
    stage.rested();
    settle(&mut stage, &[3]);
    stage.turns(EMPTY_FOR);
    stage.begin(merge(1, 2), 2, true);
    at_the_deadline(&mut stage);
    stage.assert_broken(&[]);
}

#[test]
fn a_coordinator_that_says_something_about_a_merge_or_a_split_by_hand_only_is_caught_by_r7() {
    let mut judge = Judge::default();
    judge.begins_nothing(1, &Changes::default(), &[]);
    let read = Changes {
        read: true,
        routing: true,
        workers: vec!["w".to_owned()],
        ..Changes::default()
    };
    judge.begins_nothing(2, &read, &[]);
    assert!(!judge.breached(Property::R7));

    let prepare = Changes {
        orders: vec![ReshapeOrder {
            worker: "w".to_owned(),
            order: Order::Prepare {
                region: HOME,
                epoch: 1,
            },
        }],
        ..Changes::default()
    };
    judge.begins_nothing(3, &prepare, &[]);
    assert!(judge.breached(Property::R7));

    let mut judge = Judge::default();
    judge.begins_nothing(1, &Changes::default(), &[Asked::Split { region: HOME }]);
    assert!(judge.breached(Property::R7));
}

#[test]
fn a_call_that_says_something_after_the_end_was_to_be_reached_is_caught_by_r5() {
    let mut judge = Judge::default();
    let read = Changes {
        read: true,
        ..Changes::default()
    };
    judge.hushed(1, "a tick", &read);
    assert!(judge.breaches.is_empty());
    let routing = Changes {
        routing: true,
        ..Changes::default()
    };
    judge.hushed(2, "a tick", &routing);
    assert!(judge.breached(Property::R5));
}

#[test]
fn something_that_ends_without_having_been_seen_to_begin_is_noted() {
    let mut stage = two_near();
    stage.rested();
    stage.judge.ended(
        stage.step(),
        Asked::Merge {
            survivor: region(1),
            absorbed: region(2),
        },
        true,
    );
    stage.assert_broken(&[Property::Call]);
}
