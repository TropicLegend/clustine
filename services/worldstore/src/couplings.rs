//! Lockstep between regions whose border ticks (`docs/adr/0025-effects-between-regions.md`,
//! section 8): who needs coupling, the handshake, offsets to border time, partners'
//! progress, the conversion of effects, the end of couplings and the regions given up.
//! The commit thread's alone, which hands in the time, so that this is a function of
//! what it is told; what it has to say goes on the regions' streams, in the order made.
//!
//! A region's progress is the border time it is durable through: its last tick
//! committed, or the tick before the one it joined a coupling at if that is later
//! (until it joined it ran uncoupled, and nothing it did crosses coupled), plus its
//! offset.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use clustine_format::{CoupledRegion, CouplingsFile, Handshake};
use clustine_region::RegionId;
use clustine_rpc::{CopyUse, Coupling, StoreReply, Uncoupling};
use clustine_world::{ChunkPos, Due, Effect, Effected};

use crate::effects::Uses;

/// How long a coupling that neither region needs is kept, so that a view's edge going
/// back and forth does not make it flap.
pub(crate) const COUPLING_GRACE: Duration = Duration::from_secs(1);

/// How long a region may hold its partners: not answering a handshake, or not moving
/// while nothing holds it.
pub(crate) const COUPLING_PATIENCE: Duration = Duration::from_millis(250);

/// A region whose border time advances by less than this many ticks a second over
/// [`SLOW_FOR`] while a partner waits on it is given up; one given up is coupled again
/// once it has committed this fast over as long.
pub(crate) const SLOW_PACE: u64 = 15;
pub(crate) const SLOW_FOR: Duration = Duration::from_secs(5);

type Pair = (RegionId, RegionId);

/// The two regions in order, the lower first.
fn pair(a: RegionId, b: RegionId) -> Pair {
    if a < b { (a, b) } else { (b, a) }
}

/// The other region of a pair.
fn other(pair: Pair, region: RegionId) -> RegionId {
    if pair.0 == region { pair.1 } else { pair.0 }
}

struct Coupled {
    /// The ticks each region of the pair joined at, the lower's first: its effects from
    /// then on are converted as coupled. 0 after a start of the store, after which a
    /// region's commits are all at or above it.
    joined: [u64; 2],
    /// Since when neither region has needed the coupling.
    unneeded_since: Option<Duration>,
}

struct Shake {
    /// The tick each region named, the lower's first, once it has.
    joins: [Option<u64>; 2],
    /// When "couple with" went out.
    began: Duration,
}

/// How a region coupled to others keeps up.
#[derive(Default)]
struct Pace {
    /// Since when nothing has held it: every partner is durable through its border
    /// time.
    free_since: Option<Duration>,
    /// Since when a partner has waited on it, and its border time then.
    waited_on: Option<(Duration, i64)>,
}

/// How a region given up earns its couplings back: since when it has been measured,
/// and its tick then.
struct Earning {
    since: Duration,
    from: u64,
}

#[derive(Default)]
pub(crate) struct Couplings {
    offsets: BTreeMap<RegionId, i64>,
    coupled: BTreeMap<Pair, Coupled>,
    shakes: BTreeMap<Pair, Shake>,
    given_up: BTreeMap<RegionId, Earning>,
    /// Regions preparing for a merge or a split, or released for a move: none is
    /// coupled until it is through.
    reshaping: BTreeSet<RegionId>,
    /// Of each region, the copies it has beside a chunk that would tick with them.
    beside: BTreeMap<RegionId, BTreeSet<ChunkPos>>,
    /// Of each region, the tick it is durable through (see the module's text).
    ticks: BTreeMap<RegionId, u64>,
    pace: BTreeMap<RegionId, Pace>,
    /// Whether anything the file keeps has changed since it was last written.
    changed: bool,
}

/// What a region is opened with.
pub(crate) struct OfRegion {
    pub(crate) offset: Option<i64>,
    pub(crate) couplings: Vec<Coupling>,
    pub(crate) handshakes: Vec<RegionId>,
    pub(crate) held_tick: Option<u64>,
    pub(crate) given_up: bool,
}

impl Couplings {
    /// As the file has them, after a start of the store. `ticks` says of each region the
    /// highest tick the store has of it.
    pub(crate) fn from_file(
        file: CouplingsFile,
        ticks: BTreeMap<RegionId, u64>,
        now: Duration,
    ) -> Self {
        let mut couplings = Couplings {
            ticks,
            ..Couplings::default()
        };
        for region in file.regions {
            let id = RegionId(region.region);
            if let Some(offset) = region.offset {
                couplings.offsets.insert(id, offset);
            }
            if region.given_up {
                let from = couplings.tick(id);
                couplings.given_up.insert(id, Earning { since: now, from });
            }
        }
        for (low, high) in file.couplings {
            let coupled = Coupled {
                joined: [0, 0],
                unneeded_since: None,
            };
            couplings
                .coupled
                .insert((RegionId(low), RegionId(high)), coupled);
        }
        for shake in file.handshakes {
            let joins = [shake.low_joins, shake.high_joins];
            for (region, joins) in [(shake.low, joins[0]), (shake.high, joins[1])] {
                if let Some(joins) = joins {
                    couplings.joined_floor(RegionId(region), joins);
                }
            }
            couplings.shakes.insert(
                (RegionId(shake.low), RegionId(shake.high)),
                Shake { joins, began: now },
            );
        }
        couplings
    }

    /// What the file is to keep.
    pub(crate) fn file(&self) -> CouplingsFile {
        let mut regions: BTreeSet<RegionId> = self.offsets.keys().copied().collect();
        regions.extend(self.given_up.keys());
        CouplingsFile {
            regions: regions
                .into_iter()
                .map(|region| CoupledRegion {
                    region: region.0,
                    offset: self.offsets.get(&region).copied(),
                    given_up: self.given_up.contains_key(&region),
                })
                .collect(),
            couplings: self.coupled.keys().map(|(a, b)| (a.0, b.0)).collect(),
            handshakes: self
                .shakes
                .iter()
                .map(|((low, high), shake)| Handshake {
                    low: low.0,
                    high: high.0,
                    low_joins: shake.joins[0],
                    high_joins: shake.joins[1],
                })
                .collect(),
        }
    }

    /// Whether the file is to be written, which it then is.
    pub(crate) fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// Whether anything is under way that the time can change.
    pub(crate) fn has_timers(&self) -> bool {
        !self.coupled.is_empty() || !self.shakes.is_empty() || !self.given_up.is_empty()
    }

    fn tick(&self, region: RegionId) -> u64 {
        self.ticks.get(&region).copied().unwrap_or(0)
    }

    fn joined_floor(&mut self, region: RegionId, joins: u64) {
        let tick = self.ticks.entry(region).or_insert(0);
        *tick = (*tick).max(joins.saturating_sub(1));
    }

    /// The border time `region` is durable through, if it is coupled.
    fn through(&self, region: RegionId) -> Option<i64> {
        let offset = self.offsets.get(&region)?;
        Some(self.tick(region) as i64 + offset)
    }

    fn partners(&self, region: RegionId) -> Vec<RegionId> {
        self.coupled
            .keys()
            .filter(|(a, b)| *a == region || *b == region)
            .map(|pair| other(*pair, region))
            .collect()
    }

    pub(crate) fn are_coupled(&self, a: RegionId, b: RegionId) -> bool {
        self.coupled.contains_key(&pair(a, b))
    }

    /// Who uses a chunk, as `copier` is told.
    pub(crate) fn copy_use(&self, copier: RegionId, user: Option<RegionId>) -> CopyUse {
        match user {
            None => CopyUse::Nobody,
            Some(user) if self.are_coupled(copier, user) => CopyUse::Coupled(user),
            Some(user) => CopyUse::Uncoupled(user),
        }
    }

    /// When an effect `maker` made in its tick `tick` into land `holder` uses is due
    /// (section 8, "Exact delays", and "Found while building"): in the holder's ticks
    /// if the two are coupled and the maker had joined by then, and otherwise counted
    /// from its taking.
    pub(crate) fn due(&self, maker: RegionId, holder: RegionId, tick: u64, effect: &Effect) -> Due {
        let Some(coupled) = self.coupled.get(&pair(maker, holder)) else {
            return Due::FromTaking;
        };
        let side = usize::from(maker > holder);
        let (Some(of_maker), Some(of_holder)) =
            (self.offsets.get(&maker), self.offsets.get(&holder))
        else {
            return Due::FromTaking;
        };
        if tick < coupled.joined[side] {
            return Due::FromTaking;
        }
        let delay = match effect.what {
            Effected::Tick { delay, .. } => i64::from(delay),
            _ => 0,
        };
        let due = tick as i64 + of_maker - of_holder + delay;
        Due::At {
            tick: u64::try_from(due).unwrap_or(0),
        }
    }

    /// What `region` is opened with.
    pub(crate) fn of_region(&self, region: RegionId) -> OfRegion {
        let couplings = self
            .partners(region)
            .into_iter()
            .map(|partner| Coupling {
                partner,
                through: self.through(partner).map(|through| through.max(0) as u64),
            })
            .collect();
        let shakes: Vec<(RegionId, Option<u64>)> = self
            .shakes
            .iter()
            .filter(|((a, b), _)| *a == region || *b == region)
            .map(|(pair, shake)| {
                let side = usize::from(pair.1 == region);
                (other(*pair, region), shake.joins[side])
            })
            .collect();
        OfRegion {
            offset: self.offsets.get(&region).copied(),
            couplings,
            handshakes: shakes.iter().map(|(partner, _)| *partner).collect(),
            held_tick: shakes.iter().find_map(|(_, joins)| *joins),
            given_up: self.given_up.contains_key(&region),
        }
    }

    /// A region was opened: it is measured afresh if it is given up, and says again
    /// which of its copies are beside chunks that tick.
    pub(crate) fn opened(&mut self, region: RegionId, now: Duration) {
        self.beside.remove(&region);
        let from = self.tick(region);
        if let Some(earning) = self.given_up.get_mut(&region) {
            *earning = Earning { since: now, from };
        }
        self.pace.remove(&region);
    }

    /// `region`'s copies in `begins` are beside a chunk that would tick with them, and
    /// those in `ends` no longer are.
    pub(crate) fn beside(&mut self, region: RegionId, begins: &[ChunkPos], ends: &[ChunkPos]) {
        let beside = self.beside.entry(region).or_default();
        for chunk in ends {
            beside.remove(chunk);
        }
        beside.extend(begins);
        if beside.is_empty() {
            self.beside.remove(&region);
        }
    }

    /// The pairs of regions that need coupling: one has a copy beside a chunk that
    /// would tick of a chunk the other uses.
    fn needed(&self, uses: &Uses) -> BTreeSet<Pair> {
        let mut needed = BTreeSet::new();
        for (region, chunks) in &self.beside {
            for chunk in chunks {
                if let Some(user) = uses.user(*chunk)
                    && user != *region
                {
                    needed.insert(pair(*region, user));
                }
            }
        }
        needed
    }

    /// `region`, preparing for a merge or a split or released for a move, ends its
    /// couplings, and is not coupled again until [`Couplings::reshaped`].
    pub(crate) fn reshaping(&mut self, region: RegionId, said: &mut Said) {
        self.end_all(region, Uncoupling::Reshaping { region }, said);
        self.reshaping.insert(region);
    }

    /// `region` is through with a merge or a split, and may be coupled again.
    pub(crate) fn reshaped(&mut self, region: RegionId) {
        self.reshaping.remove(&region);
    }

    /// `region` is no more: it was absorbed.
    pub(crate) fn gone(&mut self, region: RegionId, said: &mut Said) {
        self.end_all(region, Uncoupling::Reshaping { region }, said);
        self.reshaping.remove(&region);
        self.beside.remove(&region);
        self.ticks.remove(&region);
        if self.given_up.remove(&region).is_some() {
            self.changed = true;
        }
    }

    /// Ends every coupling and handshake of `region`, telling both sides; its offset
    /// goes, and that of each partner left with no coupling.
    pub(crate) fn end_all(&mut self, region: RegionId, why: Uncoupling, said: &mut Said) {
        for partner in self.partners(region) {
            self.coupled.remove(&pair(region, partner));
            said.to(region, StoreReply::Uncoupled { partner, why });
            said.to(
                partner,
                StoreReply::Uncoupled {
                    partner: region,
                    why,
                },
            );
            self.changed = true;
        }
        let shakes: Vec<Pair> = self
            .shakes
            .keys()
            .filter(|(a, b)| *a == region || *b == region)
            .copied()
            .collect();
        for shake in shakes {
            self.call_off(shake, said);
        }
        self.forget_lone_offsets(said);
    }

    fn call_off(&mut self, shake: Pair, said: &mut Said) {
        if self.shakes.remove(&shake).is_some() {
            said.to(shake.0, StoreReply::HandshakeOff { partner: shake.1 });
            said.to(shake.1, StoreReply::HandshakeOff { partner: shake.0 });
            self.changed = true;
        }
    }

    /// A region with no coupling has no border time: its offset is forgotten, and its
    /// next coupling gives it one anew.
    fn forget_lone_offsets(&mut self, _said: &mut Said) {
        let lone: Vec<RegionId> = self
            .offsets
            .keys()
            .filter(|region| self.partners(**region).is_empty())
            .copied()
            .collect();
        for region in lone {
            self.offsets.remove(&region);
            self.pace.remove(&region);
            self.changed = true;
        }
    }

    /// `region` names `tick`, its next not yet run, to the handshake with `partner`.
    /// Made durable with the file at the end of the group.
    pub(crate) fn joins_at(&mut self, region: RegionId, partner: RegionId, tick: u64) {
        let key = pair(region, partner);
        if let Some(shake) = self.shakes.get_mut(&key) {
            shake.joins[usize::from(key.1 == region)] = Some(tick);
            self.changed = true;
        }
    }

    /// `region` has committed its tick `tick`, durably.
    pub(crate) fn committed(&mut self, region: RegionId, tick: u64) {
        let ticks = self.ticks.entry(region).or_insert(0);
        *ticks = (*ticks).max(tick);
    }

    /// The members of `region`'s group of coupled regions.
    fn group_of(&self, region: RegionId) -> BTreeSet<RegionId> {
        let mut group = BTreeSet::from([region]);
        let mut next = vec![region];
        while let Some(at) = next.pop() {
            for partner in self.partners(at) {
                if group.insert(partner) {
                    next.push(partner);
                }
            }
        }
        group
    }

    /// Couples the two regions of a handshake whose ticks both named are durable
    /// (section 8, step 3).
    fn couple(&mut self, key: Pair, joins: [u64; 2], said: &mut Said) {
        let (low, high) = key;
        for (region, tick) in [(low, joins[0]), (high, joins[1])] {
            self.joined_floor(region, tick);
        }
        match (
            self.offsets.get(&low).copied(),
            self.offsets.get(&high).copied(),
        ) {
            (None, None) => {
                self.offsets.insert(low, 0);
                self.offsets.insert(high, joins[0] as i64 - joins[1] as i64);
            }
            (Some(of_low), None) => {
                self.offsets
                    .insert(high, joins[0] as i64 + of_low - joins[1] as i64);
            }
            (None, Some(of_high)) => {
                self.offsets
                    .insert(low, joins[1] as i64 + of_high - joins[0] as i64);
            }
            (Some(of_low), Some(of_high)) => {
                let low_group = self.group_of(low);
                if !low_group.contains(&high) {
                    // The smaller group shifts by one constant, so that both named
                    // ticks have the same border time.
                    let high_group = self.group_of(high);
                    let (shifted, delta) = if low_group.len() <= high_group.len() {
                        let delta = (joins[1] as i64 + of_high) - (joins[0] as i64 + of_low);
                        (low_group, delta)
                    } else {
                        let delta = (joins[0] as i64 + of_low) - (joins[1] as i64 + of_high);
                        (high_group, delta)
                    };
                    for region in shifted {
                        let offset = self.offsets.get_mut(&region).expect("a group's offsets");
                        *offset += delta;
                        said.to(region, StoreReply::Shifted { offset: *offset });
                    }
                }
            }
        }
        self.coupled.insert(
            key,
            Coupled {
                joined: joins,
                unneeded_since: None,
            },
        );
        self.changed = true;
        for (region, partner) in [(low, high), (high, low)] {
            let offset = self.offsets[&region];
            said.to(region, StoreReply::Coupled { partner, offset });
            let through = self.through(partner).expect("coupled");
            said.to(
                region,
                StoreReply::Progress {
                    partner,
                    through: through.max(0) as u64,
                },
            );
        }
    }

    /// What the end of a durable group brings: the progress of the regions that
    /// committed in it (`committed`, each region with its highest tick), the couplings
    /// of handshakes both sides have answered, handshakes for pairs that need them,
    /// couplings that ended, and the regions given up. `may_couple` says of a region
    /// whether it has an owner.
    pub(crate) fn turn(
        &mut self,
        now: Duration,
        committed: &BTreeMap<RegionId, u64>,
        uses: &Uses,
        may_couple: impl Fn(RegionId) -> bool,
        said: &mut Said,
    ) {
        // Progress, behind the effects of the same commits on the partners' streams.
        for (region, tick) in committed {
            let before = self.tick(*region);
            self.committed(*region, *tick);
            if self.tick(*region) > before {
                self.moved(*region);
            }
            let Some(through) = self.through(*region) else {
                continue;
            };
            for partner in self.partners(*region) {
                said.to(
                    partner,
                    StoreReply::Progress {
                        partner: *region,
                        through: through.max(0) as u64,
                    },
                );
            }
        }
        // Handshakes that both sides have answered; the file with their ticks is
        // written before anything said here goes out.
        let answered: Vec<(Pair, [u64; 2])> = self
            .shakes
            .iter()
            .filter_map(|(key, shake)| Some((*key, [shake.joins[0]?, shake.joins[1]?])))
            .collect();
        for (key, joins) in answered {
            self.shakes.remove(&key);
            self.couple(key, joins, said);
        }
        // Handshakes for the pairs that need coupling, and the grace of those that do
        // not.
        let needed = self.needed(uses);
        let can = |region: RegionId, this: &Self| {
            may_couple(region)
                && !this.reshaping.contains(&region)
                && !this.given_up.contains_key(&region)
        };
        for key in &needed {
            if let Some(coupled) = self.coupled.get_mut(key) {
                coupled.unneeded_since = None;
                continue;
            }
            if self.shakes.contains_key(key) || !can(key.0, self) || !can(key.1, self) {
                continue;
            }
            self.shakes.insert(
                *key,
                Shake {
                    joins: [None, None],
                    began: now,
                },
            );
            self.changed = true;
            said.to(key.0, StoreReply::CoupleWith { partner: key.1 });
            said.to(key.1, StoreReply::CoupleWith { partner: key.0 });
        }
        let mut ended = Vec::new();
        for (key, coupled) in &mut self.coupled {
            if needed.contains(key) {
                continue;
            }
            let since = *coupled.unneeded_since.get_or_insert(now);
            if now.saturating_sub(since) >= COUPLING_GRACE {
                ended.push(*key);
            }
        }
        for key in ended {
            self.coupled.remove(&key);
            self.changed = true;
            let why = Uncoupling::Unneeded;
            said.to(
                key.0,
                StoreReply::Uncoupled {
                    partner: key.1,
                    why,
                },
            );
            said.to(
                key.1,
                StoreReply::Uncoupled {
                    partner: key.0,
                    why,
                },
            );
        }
        self.forget_lone_offsets(said);
        self.patience(now, said);
    }

    /// Gives up the regions that hold their partners: a handshake not answered within
    /// [`COUPLING_PATIENCE`], a region that does not move for as long when nothing
    /// holds it, or one slower than [`SLOW_PACE`] for [`SLOW_FOR`] while a partner
    /// waits on it; and couples again, by the handshake, those given up that have
    /// earned it.
    fn patience(&mut self, now: Duration, said: &mut Said) {
        let mut giving_up = BTreeSet::new();
        for (key, shake) in &self.shakes {
            if now.saturating_sub(shake.began) < COUPLING_PATIENCE {
                continue;
            }
            for (side, region) in [key.0, key.1].into_iter().enumerate() {
                if shake.joins[side].is_none() {
                    giving_up.insert(region);
                }
            }
        }
        let coupled: Vec<RegionId> = self.offsets.keys().copied().collect();
        for region in coupled {
            let Some(through) = self.through(region) else {
                continue;
            };
            let partners: Vec<i64> = self
                .partners(region)
                .into_iter()
                .filter_map(|partner| self.through(partner))
                .collect();
            let free = partners.iter().all(|partner| *partner >= through);
            let waited_on = partners.iter().any(|partner| *partner > through);
            let pace = self.pace.entry(region).or_default();
            // Measured from the group in which it stopped waiting, and afresh whenever
            // it moves (`committed` clears it).
            match (free, pace.free_since) {
                (true, None) => pace.free_since = Some(now),
                (true, Some(since)) if now.saturating_sub(since) >= COUPLING_PATIENCE => {
                    giving_up.insert(region);
                }
                (false, _) => pace.free_since = None,
                _ => {}
            }
            match (waited_on, pace.waited_on) {
                (false, _) => pace.waited_on = None,
                (true, None) => pace.waited_on = Some((now, through)),
                (true, Some((since, from))) if now.saturating_sub(since) >= SLOW_FOR => {
                    let ticks = (through - from).max(0) as u64;
                    if ticks < SLOW_PACE * SLOW_FOR.as_secs() {
                        giving_up.insert(region);
                    } else {
                        pace.waited_on = Some((now, through));
                    }
                }
                _ => {}
            }
        }
        for region in giving_up {
            self.end_all(region, Uncoupling::GivenUp { region }, said);
            let from = self.tick(region);
            self.given_up.insert(region, Earning { since: now, from });
            self.changed = true;
        }
        let earned: Vec<RegionId> = self
            .given_up
            .iter()
            .filter(|(_, earning)| now.saturating_sub(earning.since) >= SLOW_FOR)
            .map(|(region, _)| *region)
            .collect();
        for region in earned {
            let tick = self.tick(region);
            let earning = self.given_up.get_mut(&region).expect("given up");
            if tick.saturating_sub(earning.from) >= SLOW_PACE * SLOW_FOR.as_secs() {
                self.given_up.remove(&region);
                self.changed = true;
            } else {
                *earning = Earning {
                    since: now,
                    from: tick,
                };
            }
        }
    }

    /// A region that moved is not stalled: it is measured afresh.
    pub(crate) fn moved(&mut self, region: RegionId) {
        if let Some(pace) = self.pace.get_mut(&region) {
            pace.free_since = None;
        }
    }
}

/// What is to go on the regions' streams, in the order said.
#[derive(Default)]
pub(crate) struct Said(pub(crate) Vec<(RegionId, StoreReply)>);

impl Said {
    fn to(&mut self, region: RegionId, reply: StoreReply) {
        self.0.push((region, reply));
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use clustine_world::{BlockPos, Ticked};

    use super::*;

    const A: RegionId = RegionId(1);
    const B: RegionId = RegionId(2);
    const C: RegionId = RegionId(3);

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    /// A copies a chunk B uses, beside a chunk that would tick.
    fn needing(
        couplings: &mut Couplings,
        uses: &mut Uses,
        copier: RegionId,
        user: RegionId,
        x: i32,
    ) {
        let chunk = ChunkPos::new(x, 0);
        uses.begin(user, chunk);
        couplings.beside(copier, &[chunk], &[]);
    }

    fn turn(
        couplings: &mut Couplings,
        now: Duration,
        committed: &[(RegionId, u64)],
        uses: &Uses,
    ) -> Vec<(RegionId, StoreReply)> {
        let mut said = Said::default();
        let committed = committed.iter().copied().collect();
        couplings.turn(now, &committed, uses, |_| true, &mut said);
        said.0
    }

    fn couple(
        couplings: &mut Couplings,
        uses: &Uses,
        a: (RegionId, u64),
        b: (RegionId, u64),
        now: Duration,
    ) -> Vec<(RegionId, StoreReply)> {
        couplings.joins_at(a.0, b.0, a.1);
        couplings.joins_at(b.0, a.0, b.1);
        turn(couplings, now, &[], uses)
    }

    #[test]
    fn a_handshake_gives_both_named_ticks_one_border_time() {
        let (mut couplings, mut uses) = (Couplings::default(), Uses::default());
        needing(&mut couplings, &mut uses, A, B, 0);
        assert_eq!(
            turn(&mut couplings, ms(0), &[], &uses),
            [
                (A, StoreReply::CoupleWith { partner: B }),
                (B, StoreReply::CoupleWith { partner: A }),
            ]
        );
        // Asked once, however often the need is looked at.
        assert_eq!(turn(&mut couplings, ms(10), &[], &uses), []);
        let said = couple(&mut couplings, &uses, (A, 100), (B, 40), ms(20));
        assert_eq!(
            said,
            [
                (
                    A,
                    StoreReply::Coupled {
                        partner: B,
                        offset: 0
                    }
                ),
                (
                    A,
                    StoreReply::Progress {
                        partner: B,
                        through: 99
                    }
                ),
                (
                    B,
                    StoreReply::Coupled {
                        partner: A,
                        offset: 60
                    }
                ),
                (
                    B,
                    StoreReply::Progress {
                        partner: A,
                        through: 99
                    }
                ),
            ]
        );
        assert!(couplings.take_changed());
        // A tick B schedules at its tick 50 with delay 5 is due at A's 115.
        let tick = Effect {
            at: BlockPos::new(0, 0, 0),
            what: Effected::Tick {
                of: Ticked::Block(blocks::SAND.0),
                delay: 5,
                priority: 0,
            },
        };
        assert_eq!(couplings.due(B, A, 50, &tick), Due::At { tick: 115 });
        // An update made in B's 50 counts from A's 110.
        let update = Effect {
            at: BlockPos::new(0, 0, 0),
            what: Effected::SetIf {
                expected: blocks::AIR,
                state: blocks::STONE,
                flags: 3,
            },
        };
        assert_eq!(couplings.due(B, A, 50, &update), Due::At { tick: 110 });
        // From before B joined, it is late as uncoupled.
        assert_eq!(couplings.due(B, A, 39, &update), Due::FromTaking);
        assert_eq!(couplings.due(B, C, 50, &update), Due::FromTaking);
        assert_eq!(couplings.copy_use(A, Some(B)), CopyUse::Coupled(B));
        assert_eq!(couplings.copy_use(A, Some(C)), CopyUse::Uncoupled(C));
    }

    #[test]
    fn progress_is_said_to_partners_in_border_time() {
        let (mut couplings, mut uses) = (Couplings::default(), Uses::default());
        needing(&mut couplings, &mut uses, A, B, 0);
        turn(&mut couplings, ms(0), &[], &uses);
        couple(&mut couplings, &uses, (A, 100), (B, 40), ms(1));
        assert_eq!(
            turn(&mut couplings, ms(2), &[(B, 40), (C, 7)], &uses),
            [(
                A,
                StoreReply::Progress {
                    partner: B,
                    through: 100
                }
            )]
        );
    }

    #[test]
    fn joining_two_groups_shifts_the_smaller_and_a_ring_shifts_nothing() {
        let (mut couplings, mut uses) = (Couplings::default(), Uses::default());
        let d = RegionId(4);
        // A-B and C-D, then B-C: two groups of two; the lower's shifts.
        needing(&mut couplings, &mut uses, A, B, 0);
        needing(&mut couplings, &mut uses, C, d, 1);
        turn(&mut couplings, ms(0), &[], &uses);
        couple(&mut couplings, &uses, (A, 10), (B, 10), ms(1));
        couple(&mut couplings, &uses, (C, 50), (d, 50), ms(1));
        needing(&mut couplings, &mut uses, B, C, 2);
        turn(&mut couplings, ms(2), &[], &uses);
        let said = couple(&mut couplings, &uses, (B, 20), (C, 60), ms(3));
        // B's border time at 20 is 20; C's at 60 is 60: A and B shift by 40.
        assert!(said.contains(&(A, StoreReply::Shifted { offset: 40 })));
        assert!(said.contains(&(B, StoreReply::Shifted { offset: 40 })));
        assert!(said.contains(&(
            B,
            StoreReply::Coupled {
                partner: C,
                offset: 40
            }
        )));
        assert!(said.contains(&(
            C,
            StoreReply::Coupled {
                partner: B,
                offset: 0
            }
        )));
        // D-A closes a ring: offsets stay.
        needing(&mut couplings, &mut uses, d, A, 3);
        turn(&mut couplings, ms(4), &[], &uses);
        let said = couple(&mut couplings, &uses, (d, 70), (A, 25), ms(5));
        assert!(
            !said
                .iter()
                .any(|(_, reply)| matches!(reply, StoreReply::Shifted { .. }))
        );
        assert!(said.contains(&(
            A,
            StoreReply::Coupled {
                partner: d,
                offset: 40
            }
        )));
    }

    #[test]
    fn a_coupling_nobody_needs_ends_after_its_grace_and_its_offsets_go() {
        let (mut couplings, mut uses) = (Couplings::default(), Uses::default());
        needing(&mut couplings, &mut uses, A, B, 0);
        turn(&mut couplings, ms(0), &[], &uses);
        couple(&mut couplings, &uses, (A, 10), (B, 10), ms(1));
        couplings.beside(A, &[], &[ChunkPos::new(0, 0)]);
        // Both keep ticking, so nobody is stalled meanwhile.
        let mut tick = 10;
        let mut at = 2;
        while at < 1_000 {
            assert_eq!(
                turn(&mut couplings, ms(at), &[(A, tick), (B, tick)], &uses)
                    .iter()
                    .filter(|(_, reply)| matches!(reply, StoreReply::Uncoupled { .. }))
                    .count(),
                0,
                "at {at}"
            );
            tick += 1;
            at += 50;
        }
        let said = turn(&mut couplings, ms(1_002), &[], &uses);
        let why = Uncoupling::Unneeded;
        assert!(said.contains(&(A, StoreReply::Uncoupled { partner: B, why })));
        assert!(said.contains(&(B, StoreReply::Uncoupled { partner: A, why })));
        assert_eq!(couplings.of_region(A).offset, None);
    }

    #[test]
    fn a_side_that_does_not_answer_within_patience_is_given_up_and_earns_it_back() {
        let (mut couplings, mut uses) = (Couplings::default(), Uses::default());
        needing(&mut couplings, &mut uses, A, B, 0);
        turn(&mut couplings, ms(0), &[], &uses);
        couplings.joins_at(A, B, 10);
        assert_eq!(turn(&mut couplings, ms(249), &[], &uses), []);
        let said = turn(&mut couplings, ms(250), &[], &uses);
        assert_eq!(
            said,
            [
                (A, StoreReply::HandshakeOff { partner: B }),
                (B, StoreReply::HandshakeOff { partner: A }),
            ]
        );
        assert!(couplings.of_region(B).given_up);
        assert!(!couplings.of_region(A).given_up);
        // Not asked again while given up; slow, it stays so; at pace, it is asked again.
        assert_eq!(turn(&mut couplings, ms(5_250), &[(B, 50)], &uses), []);
        assert!(couplings.of_region(B).given_up);
        turn(&mut couplings, ms(10_250), &[(B, 150)], &uses);
        assert!(!couplings.of_region(B).given_up);
        let said = turn(&mut couplings, ms(10_251), &[], &uses);
        assert!(said.contains(&(B, StoreReply::CoupleWith { partner: A })));
    }

    #[test]
    fn a_region_that_stops_is_given_up_and_one_that_waits_is_not() {
        let (mut couplings, mut uses) = (Couplings::default(), Uses::default());
        needing(&mut couplings, &mut uses, A, B, 0);
        turn(&mut couplings, ms(0), &[], &uses);
        couple(&mut couplings, &uses, (A, 10), (B, 10), ms(1));
        // A runs 10 and stops; B, durable through 9 and then 10, waits for nothing,
        // then waits for A: B is behind... A goes on to 11, B stops at 10.
        turn(&mut couplings, ms(2), &[(A, 10), (B, 10)], &uses);
        turn(&mut couplings, ms(50), &[(A, 11)], &uses);
        // A waits on B, which is free and does not move.
        assert_eq!(turn(&mut couplings, ms(200), &[], &uses), []);
        let said = turn(&mut couplings, ms(300), &[], &uses);
        let why = Uncoupling::GivenUp { region: B };
        assert!(
            said.contains(&(A, StoreReply::Uncoupled { partner: B, why })),
            "{said:?}"
        );
        assert!(couplings.of_region(B).given_up);
        assert!(!couplings.of_region(A).given_up);
    }

    #[test]
    fn a_region_slower_than_the_pace_for_five_seconds_while_waited_on_is_given_up() {
        let (mut couplings, mut uses) = (Couplings::default(), Uses::default());
        needing(&mut couplings, &mut uses, A, B, 0);
        turn(&mut couplings, ms(0), &[], &uses);
        couple(&mut couplings, &uses, (A, 10), (B, 10), ms(1));
        // B moves every 100 ms (10 a second), A always one ahead and waiting on it.
        let mut given_up_at = None;
        for step in 0..60 {
            let tick = 10 + step;
            let now = ms(2 + step * 100);
            let said = turn(&mut couplings, now, &[(A, tick + 1), (B, tick)], &uses);
            if said
                .iter()
                .any(|(_, reply)| matches!(reply, StoreReply::Uncoupled { .. }))
            {
                given_up_at = Some(now);
                break;
            }
        }
        let at = given_up_at.expect("B was given up");
        assert!(at >= SLOW_FOR && at < SLOW_FOR + ms(200), "{at:?}");
        assert!(couplings.of_region(B).given_up);
    }

    #[test]
    fn the_file_keeps_offsets_couplings_handshakes_and_who_is_given_up() {
        let (mut couplings, mut uses) = (Couplings::default(), Uses::default());
        needing(&mut couplings, &mut uses, A, B, 0);
        needing(&mut couplings, &mut uses, B, C, 1);
        turn(&mut couplings, ms(0), &[], &uses);
        couplings.joins_at(A, B, 10);
        couplings.joins_at(B, A, 12);
        couplings.joins_at(B, C, 12);
        turn(&mut couplings, ms(1), &[], &uses);
        let file = couplings.file();
        // A start knows each region's last tick from the log.
        let ticks = BTreeMap::from([(A, 9)]);
        let again = Couplings::from_file(file.clone(), ticks, ms(0));
        assert_eq!(again.file(), file);
        let of_b = again.of_region(B);
        assert_eq!(of_b.offset, Some(-2));
        assert_eq!(of_b.handshakes, [C]);
        assert_eq!(of_b.held_tick, Some(12));
        assert_eq!(
            of_b.couplings,
            [Coupling {
                partner: A,
                through: Some(9)
            }]
        );
    }
}
