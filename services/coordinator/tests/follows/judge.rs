//! The properties R1 to R5 and R7 of section 11 of
//! `docs/adr/0016-when-to-merge-and-split.md`, each as a check of what a coordinator
//! begins by itself against what the model did, where its players truly were and what
//! it reported. R6 compares two runs, and is where the runs are played.
//!
//! The judge is told what happens and notes every breach of a property; it never
//! fails by itself, so that a run in which a property is broken on purpose can ask
//! which one was. It knows nothing of how a coordinator works: whoever drives it says
//! what was begun, what ended, which reports were given and what the routing table has.

use std::collections::{BTreeMap, BTreeSet};

use clustine_coordinator::{Asked, Changes, Sighted, Wanted, decide, named};
use clustine_region::RegionId;
use clustine_rpc::Crowds;
use clustine_world::ChunkPos;

use super::model::{
    AT_ONCE, DELAY, EMPTY_FOR, FRESH, HOME, Known, LIST_EVERY, Members, ORIGIN, Player, REST,
    World, apart, clusters, crowds, joined, policy, regions_of,
};

/// What section 11 asks of a coordinator that decides by itself, and one thing more:
/// `Call` is section 5's "nothing is decided in any other call" than `tick`, and that
/// whatever ends was seen to begin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Property {
    R1,
    R2,
    R3,
    R4a,
    R4b,
    R4c,
    R4d,
    R4e,
    R5,
    R7,
    Call,
}

/// A property did not hold.
#[derive(Debug, Clone)]
pub struct Breach {
    pub property: Property,
    pub step: u64,
    pub what: String,
}

/// What a coordinator begins by itself at a look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Begun {
    Merge {
        survivor: RegionId,
        absorbed: RegionId,
    },
    Split {
        region: RegionId,
        chunks: Vec<ChunkPos>,
    },
    /// A release to even regions out.
    EvenOut { region: RegionId },
}

impl std::fmt::Display for Begun {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let at = |chunk: Option<&ChunkPos>| chunk.map_or((0, 0), |chunk| (chunk.x, chunk.z));
        match self {
            Self::Merge { survivor, absorbed } => {
                write!(
                    formatter,
                    "a merge of region {absorbed} into region {survivor}"
                )
            }
            Self::Split { region, chunks } => write!(
                formatter,
                "a split of region {region} that names {} chunks, from {:?} to {:?},",
                chunks.len(),
                at(chunks.first()),
                at(chunks.last())
            ),
            Self::EvenOut { region } => {
                write!(formatter, "a release of region {region} to even out")
            }
        }
    }
}

/// What a thing begun is to the properties. A merge is an absorption if the last
/// reports taken of both its regions were without players, and by the distances
/// otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Distances,
    Absorption,
    Split,
    EvenOut,
}

/// A report of a region as the model gave it, and whether the coordinator took it by
/// the rules of section 2.3.
#[derive(Debug, Clone)]
pub struct Given {
    pub members: Members,
    pub taken: bool,
}

/// A look: the call of `tick` of a step, with the reports the model gave in that step.
#[derive(Debug, Clone)]
pub struct Look {
    /// Whether the model gave a report of every region the coordinator knows and every
    /// one of them was taken: the coordinator's sightings are then those reports, and
    /// all of them are fresh.
    pub plain: bool,
    pub reports: BTreeMap<RegionId, Given>,
    /// What `decide` wants by those reports, if the look is plain.
    pub wanted: Vec<Wanted>,
}

/// The last report the coordinator took of a region.
#[derive(Debug, Clone)]
struct Taken {
    step: u64,
    worker: String,
    epoch: u64,
    tick: u64,
    players: bool,
}

/// A merge or a split that the test saw begin and has not seen end.
#[derive(Debug, Clone)]
struct Doing {
    asked: Asked,
    kind: Kind,
    begun: u64,
}

/// A merge begun while the players stand still, for R4 (d): which of the players of its
/// two regions were joined then by steps of at most the split distance through each
/// other, or through the origin if one of the two was the home region.
#[derive(Debug, Clone)]
struct Joining {
    asked: Asked,
    set: BTreeMap<Player, usize>,
    /// Whether it has ended, and whether it ended well.
    ended: Option<bool>,
}

/// What R5 counted at the step `s'`, and what has been begun since.
#[derive(Debug, Clone)]
pub struct Settling {
    pub at: u64,
    /// The later of `s'` and the latest `alone_until` of any region then.
    pub q: u64,
    /// The merges by the distances and the splits that are left to do.
    pub n: u32,
    /// The regions without players that are neither the home region nor pinned.
    pub e: u32,
    /// How many regions have to change workers for no worker to have two more than
    /// another.
    pub k: u32,
    pub reshapes: u32,
    pub absorptions: u32,
    pub moves: u32,
    pub empty_moves: u32,
}

impl Settling {
    /// `rest + D + 2 s`: what every merge, split and move waits for at most.
    const X: u64 = REST + DELAY + 8;

    /// By when the end is reached, given the releases to even out begun so far.
    pub fn deadline(&self) -> u64 {
        self.q
            + u64::from(self.n + self.moves + 1) * Self::X
            + u64::from(self.e + self.empty_moves) * (EMPTY_FOR + Self::X)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Judge {
    pub breaches: Vec<Breach>,
    /// How often each thing worth telling happened.
    pub seen: BTreeMap<&'static str, u32>,

    // What this coordinator was told. A coordinator made anew knows none of it.
    taken: BTreeMap<RegionId, Taken>,
    /// When a reading of the list was last handed in with `listed`.
    listed: Option<u64>,
    /// The regions that were living in a list handed in while no split was under way.
    shown: BTreeSet<RegionId>,
    /// The regions of which it was told that a split had made them.
    parts: BTreeSet<RegionId>,
    /// Who owned what, with which epoch, after the last call.
    routes: BTreeMap<RegionId, (String, u64)>,
    doing: Vec<Doing>,

    // R1.
    /// The latest end, for each region, that a rest counts from.
    ended: BTreeMap<RegionId, u64>,
    /// The survivors of absorptions that have ended and of which no report has been
    /// taken since.
    absorbed_into: BTreeSet<RegionId>,

    // R2.
    /// When each player's region was last in something that is counted for them.
    stopped: BTreeMap<Player, u64>,

    // R4.
    open: BTreeMap<RegionId, Given>,
    looks: BTreeMap<u64, Look>,
    /// The last report the model gave of each region.
    last: BTreeMap<RegionId, Members>,
    /// From which step on the players stand still, nobody joins or leaves and no fault
    /// happens, if they do.
    still: Option<u64>,
    joinings: Vec<Joining>,

    // R5.
    pub settling: Option<Settling>,
}

impl Judge {
    pub fn breach(&mut self, property: Property, step: u64, what: String) {
        self.breaches.push(Breach {
            property,
            step,
            what,
        });
    }

    pub fn breached(&self, property: Property) -> bool {
        self.breaches
            .iter()
            .any(|breach| breach.property == property)
    }

    fn count(&mut self, what: &'static str) {
        *self.seen.entry(what).or_default() += 1;
    }

    /// The coordinator is another from now on, which was told nothing of all this.
    /// When a player last stood still is kept: a new coordinator begins nothing with a
    /// region before its owner has reported it and it has rested (K6).
    pub fn anew(&mut self) {
        self.taken.clear();
        self.listed = None;
        self.shown.clear();
        self.parts.clear();
        self.routes.clear();
        self.doing.clear();
        self.ended.clear();
        self.absorbed_into.clear();
    }

    /// From this step on the players stand still, nobody joins or leaves and no fault
    /// happens.
    pub fn stand_still(&mut self, step: u64) {
        self.still = Some(step);
    }

    /// A reading of the list is handed in with `listed`.
    pub fn listed(&mut self, step: u64, living: impl Iterator<Item = RegionId>, before: &Known) {
        self.listed = Some(step);
        if !before.splitting() {
            self.shown.extend(living);
        }
    }

    /// The coordinator is told that a split has made this region.
    pub fn told_part(&mut self, part: RegionId) {
        self.parts.insert(part);
    }

    /// The routing table after a call: a region that has an owner or an epoch which the
    /// coordinator did not have for it rests from now (section 5.4).
    pub fn routes(&mut self, step: u64, routes: &BTreeMap<RegionId, (String, u64)>) {
        for (region, route) in routes {
            if self.routes.get(region) != Some(route) {
                self.rests_from(*region, step);
            }
        }
        self.routes = routes.clone();
    }

    fn rests_from(&mut self, region: RegionId, step: u64) {
        let end = self.ended.entry(region).or_insert(step);
        *end = (*end).max(step);
    }

    /// A worker gives a report of a region. `heard` is whether the coordinator knows the
    /// worker, and `known` what it has before the call. The report is taken (section
    /// 2.3) if the worker owns the region with that epoch, the region is in nothing
    /// under way, and the tick is above that of the last report taken of it from this
    /// owner with this epoch.
    #[allow(clippy::too_many_arguments)]
    pub fn given(
        &mut self,
        step: u64,
        known: &Known,
        heard: bool,
        worker: &str,
        region: RegionId,
        epoch: u64,
        tick: u64,
        members: Members,
    ) {
        let owned = known
            .routes
            .get(&region)
            .is_some_and(|(owner, with)| owner == worker && *with == epoch);
        let newer = self
            .taken
            .get(&region)
            .is_none_or(|last| last.worker != worker || last.epoch != epoch || last.tick < tick);
        let taken = heard && owned && !known.reserved(region) && newer;
        if taken {
            let players = !members.is_empty();
            // The first report taken of a survivor after an absorption decides whether
            // it rests: from that report, if somebody is in it (section 5.5).
            if self.absorbed_into.remove(&region) && players {
                self.rests_from(region, step);
                self.count("survivors that rest for who came as they absorbed");
            }
            self.taken.insert(
                region,
                Taken {
                    step,
                    worker: worker.to_owned(),
                    epoch,
                    tick,
                    players,
                },
            );
        }
        self.last.insert(region, members.clone());
        let given = Given { members, taken };
        // Of two reports of a region in one step the one that was taken is kept.
        if taken || !self.open.contains_key(&region) {
            self.open.insert(region, given);
        }
    }

    /// The look of a step: every report of the step has been given, and `tick` is
    /// called next.
    pub fn look(&mut self, step: u64, known: &Known) {
        let reports = std::mem::take(&mut self.open);
        let plain = known.waiting.is_empty()
            && !known.routes.is_empty()
            && known
                .routes
                .keys()
                .all(|region| reports.get(region).is_some_and(|given| given.taken));
        let wanted = if plain {
            let said: Vec<(RegionId, Crowds)> = known
                .routes
                .keys()
                .map(|region| (*region, crowds(&reports[region].members)))
                .collect();
            let sighted: Vec<Sighted<'_>> = said
                .iter()
                .map(|(region, crowds)| Sighted {
                    region: *region,
                    fresh: true,
                    crowds,
                })
                .collect();
            decide(&policy(), ORIGIN, HOME, &sighted)
        } else {
            Vec::new()
        };
        if plain {
            self.count("plain looks");
        }
        self.count("looks");
        self.looks.insert(
            step,
            Look {
                plain,
                reports,
                wanted,
            },
        );
    }

    /// Whether the list was handed in no more than two `LIST_EVERY` before a step.
    pub fn listed_lately(&self, step: u64) -> bool {
        self.listed
            .is_some_and(|listed| step <= listed + 2 * LIST_EVERY)
    }

    /// Whether a rest has passed since the last end that a rest counts from for the
    /// region (R1).
    pub fn rested(&self, region: RegionId, step: u64) -> bool {
        self.ended.get(&region).is_none_or(|end| step >= end + REST)
    }

    /// The look of a step, once it has been.
    pub fn look_at(&self, step: u64) -> Option<&Look> {
        self.looks.get(&step)
    }

    /// The coordinator has begun this by itself at the look of the world's step. `before`
    /// is what it had before the call, and `splits` how many splits it has begun in the
    /// same call before this.
    pub fn begun(&mut self, world: &World, before: &Known, what: &Begun, splits: usize) -> Kind {
        let step = world.step;
        let (kind, regions) = match what {
            Begun::Merge { survivor, absorbed } => {
                let empty =
                    |region: &RegionId| self.taken.get(region).is_some_and(|taken| !taken.players);
                let kind = if empty(survivor) && empty(absorbed) {
                    Kind::Absorption
                } else {
                    Kind::Distances
                };
                (kind, vec![*survivor, *absorbed])
            }
            Begun::Split { region, .. } => (Kind::Split, vec![*region]),
            Begun::EvenOut { region } => (Kind::EvenOut, vec![*region]),
        };
        self.count(match kind {
            Kind::Distances => "merges by the distances begun",
            Kind::Absorption => "absorptions begun",
            Kind::Split => "splits begun",
            Kind::EvenOut => "releases to even out begun",
        });

        // R1. A region can be made the survivor of an absorption at any time.
        let held_to_a_rest: &[RegionId] = match kind {
            Kind::Absorption => &regions[1..],
            _ => &regions,
        };
        for region in held_to_a_rest {
            if let Some(end) = self.ended.get(region).copied()
                && step < end + REST
            {
                self.breach(
                    Property::R1,
                    step,
                    format!(
                        "{what} is begun {} steps after something that involved region {region} \
                         ended at step {end}, and a rest is {REST}",
                        step - end
                    ),
                );
            }
        }

        // R2. A player whom the model handed over is left out for a rest.
        for (player, walker) in &world.players {
            if !regions.contains(&walker.region) {
                continue;
            }
            if world
                .handed
                .get(player)
                .is_some_and(|handed| step < handed + REST)
            {
                continue;
            }
            if let Some(last) = self.stopped.insert(*player, step)
                && step < last + REST
            {
                self.breach(
                    Property::R2,
                    step,
                    format!(
                        "{what} is the second thing begun with a region of player {player} \
                         in {} steps: the one before was begun at step {last}",
                        step - last
                    ),
                );
            }
        }

        match (kind, what) {
            (Kind::EvenOut, _) => {}
            (Kind::Distances, Begun::Merge { survivor, absorbed }) => {
                self.known_enough(world, before, kind, what, &regions, splits);
                self.near(world, what, *survivor, *absorbed);
                self.merge_stood(step, what, *survivor, *absorbed);
            }
            (Kind::Absorption, Begun::Merge { survivor, absorbed }) => {
                self.known_enough(world, before, kind, what, &regions, splits);
                self.empty(world, what, *survivor, *absorbed);
            }
            (_, Begun::Split { region, chunks }) => {
                self.known_enough(world, before, kind, what, &regions, splits);
                self.far(world, what, *region, chunks);
                self.split_stood(step, what, *region, chunks);
                self.not_flapping(world, what, *region, chunks);
            }
            (_, Begun::Merge { .. } | Begun::EvenOut { .. }) => {
                unreachable!("the kind follows from what was begun")
            }
        }

        // R5: what is begun from `s'` on is counted against what was left to do then.
        if let Some(settling) = &mut self.settling
            && step >= settling.at
        {
            let over = match kind {
                Kind::Distances | Kind::Split => {
                    settling.reshapes += 1;
                    (settling.reshapes > settling.n).then(|| {
                        format!(
                            "merge by the distances or split number {} since step {}, where \
                             {} were left to do",
                            settling.reshapes, settling.at, settling.n
                        )
                    })
                }
                Kind::Absorption => {
                    settling.absorptions += 1;
                    (settling.absorptions > settling.e).then(|| {
                        format!(
                            "absorption number {} since step {}, where {} regions were \
                             without players",
                            settling.absorptions, settling.at, settling.e
                        )
                    })
                }
                Kind::EvenOut => {
                    settling.moves += 1;
                    if world.members(regions[0]).is_empty() {
                        settling.empty_moves += 1;
                    }
                    let most = settling.k + settling.n + settling.e;
                    (settling.moves > most).then(|| {
                        format!(
                            "release to even out number {} since step {}, where k + n + e \
                             is {} + {} + {}",
                            settling.moves, settling.at, settling.k, settling.n, settling.e
                        )
                    })
                }
            };
            if let Some(over) = over {
                self.breach(Property::R5, step, format!("{what} is {over}"));
            }
        }

        let asked = match what {
            Begun::Merge { survivor, absorbed } => Some(Asked::Merge {
                survivor: *survivor,
                absorbed: *absorbed,
            }),
            Begun::Split { region, .. } => Some(Asked::Split { region: *region }),
            Begun::EvenOut { .. } => None,
        };
        if let Some(asked) = asked {
            self.doing.push(Doing {
                asked,
                kind,
                begun: step,
            });
            // R4 (d): who a merge joins, if the players stand still by now.
            if let Asked::Merge { survivor, absorbed } = asked
                && self.still.is_some_and(|still| step > still)
            {
                self.count("merges begun while the players stand");
                let mut members = world.members(survivor);
                members.extend(world.members(absorbed));
                let mut points: Vec<ChunkPos> = members.iter().map(|(_, at)| *at).collect();
                if regions.contains(&HOME) {
                    points.push(ORIGIN);
                }
                let sets = joined(&points, policy().split_distance);
                self.joinings.push(Joining {
                    asked,
                    set: members
                        .iter()
                        .zip(&sets)
                        .map(|((player, _), set)| (*player, *set))
                        .collect(),
                    ended: None,
                });
            }
        }
        kind
    }

    /// R3, nothing on what is not known.
    fn known_enough(
        &mut self,
        world: &World,
        before: &Known,
        kind: Kind,
        what: &Begun,
        regions: &[RegionId],
        splits: usize,
    ) {
        let step = world.step;
        for region in regions {
            let taken = self.taken.get(region).map(|taken| taken.step);
            if taken.is_none_or(|taken| step > taken + FRESH) {
                self.breach(
                    Property::R3,
                    step,
                    format!(
                        "{what} is begun, and the last report taken of region {region} was at step \
                         {taken:?}: none within a second"
                    ),
                );
            }
        }
        if let Begun::Merge { absorbed, .. } = what {
            if *absorbed == HOME {
                self.breach(
                    Property::R3,
                    step,
                    format!("{what} names the home region as the one to absorb"),
                );
            }
            if kind == Kind::Absorption
                && world
                    .regions
                    .get(absorbed)
                    .is_some_and(|land| land.pinned())
            {
                self.breach(
                    Property::R3,
                    step,
                    format!("{what} absorbs a pinned region for being empty"),
                );
            }
        }
        if kind == Kind::Split && (before.splitting() || splits > 0) {
            self.breach(
                Property::R3,
                step,
                format!(
                    "{what} is begun while another split is under way: {:?}",
                    before.under_way
                ),
            );
        }
        match self.listed {
            None => self.breach(
                Property::R3,
                step,
                format!("{what} is begun before the list was ever handed in"),
            ),
            Some(listed) if step > listed + 2 * LIST_EVERY => self.breach(
                Property::R3,
                step,
                format!(
                    "{what} is begun {} steps after the list was last handed in, at step \
                     {listed}; two `LIST_EVERY` are {}",
                    step - listed,
                    2 * LIST_EVERY
                ),
            ),
            Some(_) => {}
        }
        let unreported: Vec<RegionId> = self
            .shown
            .iter()
            .filter(|region| {
                world.regions.contains_key(region)
                    && !self.taken.contains_key(region)
                    && !self.parts.contains(region)
            })
            .copied()
            .collect();
        if !unreported.is_empty() {
            self.breach(
                Property::R3,
                step,
                format!(
                    "{what} is begun while no report was ever taken of {unreported:?}, \
                     which the list has shown and which live"
                ),
            );
        }
    }

    /// R4 (a): a player of each region of a merge by the distances, at most the merge
    /// distance apart, each at a step of the last two seconds.
    fn near(&mut self, world: &World, what: &Begun, one: RegionId, other: RegionId) {
        let step = world.step;
        let since = step.saturating_sub(2 * FRESH);
        let places = |region: RegionId| -> Vec<ChunkPos> {
            let mut places: Vec<ChunkPos> = (since..=step)
                .flat_map(|at| world.members_at(at, region))
                .map(|(_, at)| at)
                .collect();
            // The origin stands for a player of the home region at every step.
            if region == HOME {
                places.push(ORIGIN);
            }
            places
        };
        let (ones, others) = (places(one), places(other));
        let reach = u64::from(policy().merge_distance);
        let near = ones
            .iter()
            .any(|one| others.iter().any(|other| apart(*one, *other) <= reach));
        if !near {
            self.breach(
                Property::R4a,
                step,
                format!(
                    "{what} is begun, and in the last two seconds no player of the one \
                     region was within {reach} chunks of a player of the other: {ones:?} \
                     and {others:?}"
                ),
            );
        }
    }

    /// R4 (b): what holds of an absorption.
    fn empty(&mut self, world: &World, what: &Begun, survivor: RegionId, absorbed: RegionId) {
        let step = world.step;
        if absorbed == HOME
            || world
                .regions
                .get(&absorbed)
                .is_some_and(|land| land.pinned())
        {
            self.breach(
                Property::R4b,
                step,
                format!("{what} absorbs the home region or a pinned one for being empty"),
            );
        }
        if survivor != HOME && survivor > absorbed {
            self.breach(
                Property::R4b,
                step,
                format!("{what}: the survivor is neither the home region nor the lower"),
            );
        }
        let reported_with_players = |region: RegionId, steps: u64| {
            self.looks
                .range(step.saturating_sub(steps)..=step)
                .find(|(_, look)| {
                    look.reports
                        .get(&region)
                        .is_some_and(|given| !given.members.is_empty())
                })
                .map(|(at, _)| *at)
        };
        // The absorbed region has been without players for `EMPTY_FOR` or longer, and
        // the survivor for more than a second (section 4.4): its run of reports without
        // players began five looks ago or earlier, so the report of that look, if one
        // was given, is without players too.
        let in_absorbed = reported_with_players(absorbed, EMPTY_FOR);
        let in_survivor = reported_with_players(survivor, FRESH + 1);
        if let Some(at) = in_absorbed {
            self.breach(
                Property::R4b,
                step,
                format!(
                    "{what}: the report of step {at} had a player in the absorbed region, \
                     which is within `EMPTY_FOR`"
                ),
            );
        }
        if let Some(at) = in_survivor {
            self.breach(
                Property::R4b,
                step,
                format!(
                    "{what}: the report of step {at} had a player in the survivor, which \
                     has not been without players for more than a second"
                ),
            );
        }
        let wanted = self.looks.get(&step).filter(|look| look.plain).map(|look| {
            look.wanted
                .iter()
                .filter(|wanted| {
                    matches!(wanted, Wanted::Merge { survivor: one, absorbed: other, .. }
                        if *one == survivor || *other == survivor)
                })
                .cloned()
                .collect::<Vec<Wanted>>()
        });
        match wanted {
            Some(wanted) if !wanted.is_empty() => self.breach(
                Property::R4b,
                step,
                format!("{what}: by the reports of this look {wanted:?} is wanted of the survivor"),
            ),
            Some(_) => self.count("absorptions begun at a plain look"),
            None => {}
        }
    }

    /// Who of the region, by the last report the model gave of it, is still of it, with
    /// where that report has them and whether they now stand in a chunk named.
    fn parted(
        &self,
        world: &World,
        region: RegionId,
        chunks: &[ChunkPos],
    ) -> (Vec<ChunkPos>, Vec<ChunkPos>) {
        let (mut go, mut stay) = (Vec::new(), Vec::new());
        for (player, reported) in self.last.get(&region).into_iter().flatten() {
            let Some(walker) = world.players.get(player) else {
                continue;
            };
            if walker.region != region {
                continue;
            }
            if chunks.contains(&walker.at) {
                go.push(*reported);
            } else {
                stay.push(*reported);
            }
        }
        (go, stay)
    }

    /// R4 (c): those a split takes were, by the region's last report, in another cluster
    /// than those it leaves.
    fn far(&mut self, world: &World, what: &Begun, region: RegionId, chunks: &[ChunkPos]) {
        let step = world.step;
        let (go, stay) = self.parted(world, region, chunks);
        let Some(look) = self.looks.get(&step).filter(|look| look.plain) else {
            // The test does not know what the coordinator had heard of the others, and
            // checks what follows from the region's own report.
            let reach = u64::from(policy().split_distance);
            let mut stay = stay;
            if region == HOME {
                stay.push(ORIGIN);
            }
            let near = go.iter().find_map(|go| {
                stay.iter()
                    .find(|stay| apart(*go, **stay) <= reach)
                    .map(|stay| (*go, *stay))
            });
            if let Some((go, stay)) = near {
                self.breach(
                    Property::R4c,
                    step,
                    format!(
                        "{what} takes a player whom the region's last report has at {go:?}, \
                         within {reach} chunks of {stay:?}, where somebody stays"
                    ),
                );
            }
            return;
        };
        let mut places: Vec<(RegionId, ChunkPos)> = vec![(HOME, ORIGIN)];
        for (of, given) in &look.reports {
            places.extend(given.members.iter().map(|(_, at)| (*of, *at)));
        }
        let cluster = clusters(&places);
        // The cluster of a place of the region. Every chunk of its last report is one:
        // at a plain look that report is of this step.
        let cluster_of = |at: ChunkPos| {
            places
                .iter()
                .position(|place| *place == (region, at))
                .map(|place| cluster[place])
        };
        let mut stay = stay;
        if region == HOME {
            stay.push(ORIGIN);
        }
        let joined = go.iter().find_map(|go| {
            stay.iter()
                .find(|stay| cluster_of(*go).is_some() && cluster_of(*go) == cluster_of(**stay))
                .map(|stay| (*go, *stay))
        });
        if let Some((go, stay)) = joined {
            self.breach(
                Property::R4c,
                step,
                format!(
                    "{what} takes a player whom the reports of this look have at {go:?}, in \
                     one cluster with {stay:?}, where somebody stays"
                ),
            );
        }
    }

    /// R4 (d), no flapping: a split begun while the players stand still does not part
    /// two players whom a merge begun since they stand had put into one region, if they
    /// were joined when that merge was begun.
    fn not_flapping(&mut self, world: &World, what: &Begun, region: RegionId, chunks: &[ChunkPos]) {
        let step = world.step;
        if self.still.is_none_or(|still| step <= still) {
            return;
        }
        self.count("splits begun while the players stand");
        let members = world.members(region);
        let (go, stay): (Members, Members) =
            members.into_iter().partition(|(_, at)| chunks.contains(at));
        let mut parted = None;
        for joining in &self.joinings {
            if joining.ended != Some(true) {
                continue;
            }
            for (goes, _) in &go {
                for (stays, _) in &stay {
                    if let (Some(one), Some(other)) =
                        (joining.set.get(goes), joining.set.get(stays))
                        && one == other
                    {
                        parted = Some((*goes, *stays, joining.asked));
                    }
                }
            }
        }
        if let Some((goes, stays, merge)) = parted {
            self.breach(
                Property::R4d,
                step,
                format!(
                    "{what} parts the players {goes} and {stays}, whom {merge:?} had put \
                     into one region when they were joined by steps of the split distance"
                ),
            );
        }
    }

    /// The looks of the second before a step and that step's, if all of them are plain:
    /// those R4 (e) names. And before them the look one earlier, if that is plain as
    /// well: what has stood has been wanted for more than a second (section 5.3), which
    /// with looks a quarter of a second apart is at six looks, so where the test knows
    /// the sightings of the sixth, it holds what is begun to that one too.
    fn plain_looks(&self, step: u64) -> Option<Vec<&Look>> {
        let plain = |at: u64| self.looks.get(&at).filter(|look| look.plain);
        let named: Option<Vec<&Look>> = (step.checked_sub(FRESH)?..=step).map(plain).collect();
        let mut looks = named?;
        if let Some(earlier) = step.checked_sub(FRESH + 1).and_then(plain) {
            looks.insert(0, earlier);
        }
        Some(looks)
    }

    /// R4 (e) for a merge by the distances: nothing on one look, by the reports.
    fn merge_stood(&mut self, step: u64, what: &Begun, one: RegionId, other: RegionId) {
        let Some(looks) = self.plain_looks(step) else {
            self.count("merges by the distances that R4 (e) could not check");
            return;
        };
        let unwanted = looks.iter().position(|look| {
            !look.wanted.iter().any(|wanted| {
                matches!(wanted, Wanted::Merge { survivor, absorbed, .. }
                    if (*survivor, *absorbed) == (one, other)
                        || (*survivor, *absorbed) == (other, one))
            })
        });
        // The step of the first of those looks at which it was not wanted.
        let unwanted = unwanted.map(|at| step + 1 + at as u64 - looks.len() as u64);
        self.count("merges by the distances that R4 (e) checked");
        if let Some(at) = unwanted {
            self.breach(
                Property::R4e,
                step,
                format!(
                    "{what} is begun, and by the reports of the look at step {at} no merge \
                     of the two was wanted: it has not stood for more than a second"
                ),
            );
        }
    }

    /// The groups that are to go of a region by the reports of a look.
    fn groups(look: &Look, region: RegionId) -> &[Vec<ChunkPos>] {
        look.wanted
            .iter()
            .find_map(|wanted| match wanted {
                Wanted::Split {
                    region: of, groups, ..
                } if *of == region => Some(groups.as_slice()),
                _ => None,
            })
            .unwrap_or(&[])
    }

    /// R4 (e) for a split: the chunks named are those of groups that were to go at
    /// every look of the second before, each continuing the one before it.
    fn split_stood(&mut self, step: u64, what: &Begun, region: RegionId, chunks: &[ChunkPos]) {
        let Some(looks) = self.plain_looks(step) else {
            self.count("splits that R4 (e) could not check");
            return;
        };
        let wrong = Self::has_not_stood(&looks, step, region, chunks);
        self.count("splits that R4 (e) checked");
        if let Some(wrong) = wrong {
            self.breach(Property::R4e, step, format!("{what} {wrong}"));
        }
    }

    /// What is wrong with a split by the reports of the looks of the second before it
    /// and of its own, which is the last of them.
    fn has_not_stood(
        looks: &[&Look],
        step: u64,
        region: RegionId,
        chunks: &[ChunkPos],
    ) -> Option<String> {
        let policy = policy();
        let margin = u64::from(policy.margin());
        let (now, earlier) = looks.split_last().expect("five looks");
        let stood: Vec<&Vec<ChunkPos>> = Self::groups(now, region)
            .iter()
            .filter(|group| group.iter().all(|chunk| chunks.contains(chunk)))
            .collect();
        if stood.is_empty() {
            return Some(format!(
                "names the chunks of no group that is to go by the reports of its look, \
                 which are {:?}",
                Self::groups(now, region)
            ));
        }
        let of_those: Vec<&[ChunkPos]> = stood.iter().map(|group| group.as_slice()).collect();
        let expected = named(&policy, &of_those);
        if expected != chunks {
            return Some(format!(
                "names other chunks than those around the groups {stood:?} whose chunks are \
                 among them: {expected:?}"
            ));
        }
        // Whether a group continues another in the sense of section 5.3.
        let continues = |later: &Vec<ChunkPos>, earlier: &Vec<ChunkPos>| {
            later
                .iter()
                .all(|chunk| earlier.iter().any(|other| apart(*chunk, *other) <= margin))
        };
        for group in stood {
            let mut later: Vec<&Vec<ChunkPos>> = vec![group];
            for (back, look) in earlier.iter().rev().enumerate() {
                let before: Vec<&Vec<ChunkPos>> = Self::groups(look, region)
                    .iter()
                    .filter(|earlier| later.iter().any(|later| continues(later, earlier)))
                    .collect();
                if before.is_empty() {
                    return Some(format!(
                        "names the group {group:?}, which by the reports of the look at step \
                         {} continues no group that was to go: it has not stood for a second",
                        step - 1 - back as u64
                    ));
                }
                later = before;
            }
        }
        None
    }

    /// A merge or a split has ended, well or not.
    pub fn ended(&mut self, step: u64, asked: Asked, well: bool) {
        let Some(at) = self.doing.iter().position(|doing| doing.asked == asked) else {
            self.breach(
                Property::Call,
                step,
                format!("{asked:?} has ended, and the test did not see it begin"),
            );
            return;
        };
        let doing = self.doing.remove(at);
        self.count(match (doing.kind, well) {
            (Kind::Distances, true) => "merges by the distances made",
            (Kind::Distances, false) => "merges by the distances that came to nothing",
            (Kind::Absorption, true) => "absorptions made",
            (Kind::Absorption, false) => "absorptions that came to nothing",
            (_, true) => "splits made",
            (_, false) => "splits that came to nothing",
        });
        match (doing.kind, asked) {
            // The end of an absorption is not, for its survivor, an end that a rest
            // counts from, unless the first report taken of it afterwards has a player.
            (Kind::Absorption, Asked::Merge { survivor, absorbed }) => {
                self.absorbed_into.insert(survivor);
                self.rests_from(absorbed, step);
            }
            (_, asked) => {
                for region in regions_of(&asked) {
                    self.rests_from(region, step);
                }
            }
        }
        if let Some(joining) = self
            .joinings
            .iter_mut()
            .find(|joining| joining.asked == asked && joining.ended.is_none())
        {
            joining.ended = Some(well);
        }
        if let Some(settling) = &self.settling
            && doing.begun >= settling.at
            && !well
        {
            self.breach(
                Property::R5,
                step,
                format!(
                    "{asked:?}, begun at step {} when everything had settled, came to nothing",
                    doing.begun
                ),
            );
        }
    }

    /// What is under way after a call: never more than `AT_ONCE` (R3).
    pub fn under_way(&mut self, step: u64, under_way: &[Asked]) {
        if under_way.len() > AT_ONCE {
            self.breach(
                Property::R3,
                step,
                format!("more than {AT_ONCE} are under way: {under_way:?}"),
            );
        }
    }

    /// R7: a coordinator that decides nothing by itself begins nothing. It says nothing
    /// to a worker about a merge or a split, none ends, and none is under way.
    pub fn begins_nothing(&mut self, step: u64, changes: &Changes, under_way: &[Asked]) {
        if !changes.orders.is_empty() || !changes.reshaped.is_empty() || !under_way.is_empty() {
            self.breach(
                Property::R7,
                step,
                format!(
                    "a coordinator that decides nothing by itself says {:?}, ends {:?} and \
                     has {under_way:?} under way",
                    changes.orders, changes.reshaped
                ),
            );
        }
    }

    /// R5, at the step `s'`: what is left to do is counted, by the model's regions and
    /// the true positions. `runs` is how many regions each worker runs, and `alone` the
    /// latest `alone_until` of any region, as a step.
    pub fn settle(&mut self, world: &World, runs: &[u32], alone: Option<u64>) {
        let mut places: Vec<(RegionId, ChunkPos)> = vec![(HOME, ORIGIN)];
        places.extend(
            world
                .players
                .values()
                .map(|walker| (walker.region, walker.at)),
        );
        let cluster = clusters(&places);
        // A group is a region's places in one cluster.
        let groups: BTreeSet<(RegionId, usize)> = places
            .iter()
            .zip(&cluster)
            .map(|((region, _), cluster)| (*region, *cluster))
            .collect();
        let with_a_group: BTreeSet<RegionId> = groups.iter().map(|(region, _)| *region).collect();
        let all: BTreeSet<usize> = cluster.iter().copied().collect();
        let (g, r, c) = (groups.len(), with_a_group.len(), all.len());
        let n = (g - r) + (g - c);
        let e = world
            .regions
            .iter()
            .filter(|(region, land)| {
                **region != HOME && !land.pinned() && world.members(**region).is_empty()
            })
            .count();
        let k = if runs.is_empty() {
            0
        } else {
            let (regions, workers) = (runs.iter().sum::<u32>(), runs.len() as u32);
            let (most, fewest) = (regions.div_ceil(workers), regions / workers);
            let above: u32 = runs.iter().map(|has| has.saturating_sub(most)).sum();
            let below: u32 = runs.iter().map(|has| fewest.saturating_sub(*has)).sum();
            above.max(below)
        };
        self.settling = Some(Settling {
            at: world.step,
            q: world.step.max(alone.unwrap_or(0)),
            n: n as u32,
            e: e as u32,
            k,
            reshapes: 0,
            absorptions: 0,
            moves: 0,
            empty_moves: 0,
        });
    }

    /// R5, at its deadline: the end is reached. `releasing` is whether a region is being
    /// released.
    pub fn end(&mut self, world: &World, known: &Known, releasing: bool) {
        let step = world.step;
        let policy = policy();
        let mut wrong: Vec<String> = Vec::new();
        let near = u64::from(policy.merge_distance);
        for (player, walker) in &world.players {
            if apart(walker.at, ORIGIN) <= near && walker.region != HOME {
                wrong.push(format!(
                    "player {player} at {:?} is within {near} of the origin and in {}",
                    walker.at, walker.region
                ));
            }
            for (other, with) in world.players.range(player + 1..) {
                if apart(walker.at, with.at) <= near && walker.region != with.region {
                    wrong.push(format!(
                        "the players {player} at {:?} and {other} at {:?} are within {near} \
                         of each other, in {} and {}",
                        walker.at, with.at, walker.region, with.region
                    ));
                }
            }
        }
        let empty: Vec<RegionId> = world
            .regions
            .keys()
            .filter(|region| world.members(**region).is_empty())
            .copied()
            .collect();
        for (region, land) in &world.regions {
            let mut points: Vec<ChunkPos> =
                world.members(*region).iter().map(|(_, at)| *at).collect();
            if points.is_empty() {
                let lowest = empty.first() == Some(region);
                let may_stay =
                    *region == HOME || land.pinned() || (!empty.contains(&HOME) && lowest);
                if !may_stay {
                    wrong.push(format!(
                        "region {region} is without players and still there; those without are \
                         {empty:?}"
                    ));
                }
                continue;
            }
            if *region == HOME {
                points.push(ORIGIN);
            }
            let sets = joined(&points, policy.split_distance);
            if sets.iter().any(|set| *set != sets[0]) {
                wrong.push(format!(
                    "the players of region {region} are not joined by steps of the split distance: \
                     {points:?}"
                ));
            }
        }
        if !known.under_way.is_empty() {
            wrong.push(format!("{:?} are under way", known.under_way));
        }
        if releasing {
            wrong.push("a region is being released".to_owned());
        }
        let settling = self.settling.clone().expect("the run has settled");
        // What R5 counted and what was begun since, to see how near its bounds come.
        for (what, times) in [
            (
                "R5: merges by the distances and splits left to do (n)",
                settling.n,
            ),
            (
                "R5: merges by the distances and splits begun since",
                settling.reshapes,
            ),
            ("R5: regions without players to absorb (e)", settling.e),
            ("R5: absorptions begun since", settling.absorptions),
            ("R5: regions to change workers (k)", settling.k),
            ("R5: releases to even out begun since (m)", settling.moves),
        ] {
            *self.seen.entry(what).or_default() += times;
        }
        for wrong in wrong {
            self.breach(
                Property::R5,
                step,
                format!("the end is not reached by its time ({settling:?}): {wrong}"),
            );
        }
    }

    /// R5, after its deadline: no call of the coordinator says anything to anybody, but
    /// to read the list.
    pub fn hushed(&mut self, step: u64, what: &str, changes: &Changes) {
        let quiet = Changes {
            read: changes.read,
            ..Changes::default()
        };
        if *changes != quiet {
            self.breach(
                Property::R5,
                step,
                format!("after the end was to be reached, {what} says {changes:?}"),
            );
        }
    }
}
