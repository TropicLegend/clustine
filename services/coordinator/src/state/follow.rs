//! What a coordinator that merges and splits regions by itself keeps, and what it does
//! with it at a tick. The rules are those of
//! `docs/adr/0016-when-to-merge-and-split.md`, and the sections named here are that
//! record's.
//!
//! Like the rest of the state machine, nothing here reads a clock or does I/O, and
//! every collection is ordered: the same calls lead to the same decisions. A
//! coordinator that decides nothing by itself (`CoordinatorConfig::follow` is `None`)
//! keeps nothing of any of this, and every function here that notes something returns
//! at once for it.
//!
//! Nothing that is kept grows without bound. What is kept of a region ([`Kept`]) is
//! part of the region and goes with it. What is kept besides ([`Noted`]) names only
//! regions the coordinator knows: a merge that is wanted is forgotten when it has not
//! been wanted for a second with both its regions heard from, or when a reading of the
//! list takes one of its regions away, and the groups that are to go are those of the
//! last tick and no others.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use clustine_rpc::{Crowds, Decline, Off, PlayersOf, RegionList};
use clustine_world::{ChunkPos, RegionId};
use tracing::{info, warn};

use super::{Coordinator, LOG, Order, ReshapeOrder, ReshapeRefusal, Split, Undone};
use crate::policy::{self, Policy, Sighted, Wanted};

/// How old a sighting may be and still be fresh; and what is wanted has to be wanted
/// for longer than this before it is begun (section 3).
pub(super) const FRESH: Duration = Duration::from_secs(1);

/// How many merges and splits are under way at one time, whoever asked for them
/// (section 3).
pub(super) const AT_ONCE: usize = 4;

/// How many rests a region is without players before it is absorbed (`EMPTY_FOR`),
/// and how many it is left alone for after an attempt that failed (`LONG`). Both go
/// with the rest (section 8).
const RESTS: u32 = 3;

/// How often `LONG` doubles at most: a region whose attempts keep failing is left
/// alone for 8 times `LONG` and no longer (section 5.5).
const DOUBLINGS: u32 = 3;

/// The answer "not yet" to a split is a failure the third time in a row (section 5.5).
const NOT_YET: u32 = 3;

/// What was taken last of where the players of a region are (section 2.3).
#[derive(Debug, Clone)]
pub(super) struct Sighting {
    /// The owner and the epoch it was of.
    pub(super) owner: String,
    pub(super) epoch: u64,
    /// The tick of the region it was said with.
    pub(super) tick: u64,
    /// The chunks with players in them, each once and with how many, ascending. A
    /// chunk without players is left out.
    pub(super) crowds: Crowds,
    /// When it was taken; nothing once a merge or a split of the region has begun
    /// since, and for a sighting that was made for a part. Such a sighting is not
    /// fresh until a report replaces it.
    pub(super) taken: Option<Instant>,
}

/// What a coordinator that decides by itself keeps of a region (section 2.3). It is
/// part of the region, and goes when a reading of the list takes the region away.
#[derive(Debug, Clone, Default)]
pub(super) struct Kept {
    pub(super) sighting: Option<Sighting>,
    /// The owner and the epoch the region had when this was last looked at
    /// ([`Coordinator::note_owners`]): what tells an owner or an epoch that the
    /// coordinator did not have for the region from one that it had.
    pub(super) held: Option<(String, u64)>,
    /// The time of the first of the unbroken run of taken reports without players.
    pub(super) empty_since: Option<Instant>,
    /// Before when the coordinator begins nothing with the region by itself: this is
    /// what "rests" and "is left alone" mean.
    pub(super) alone_until: Option<Instant>,
    /// How many attempts with the region have failed since the last one that ended
    /// well.
    pub(super) failures: u32,
    /// How many splits of the region were answered "not yet" since the last attempt
    /// that ended well, or since such answers were last counted as a failure.
    pub(super) not_yet: u32,
    /// Whether the last merge or split the region was in, an absorption and a merge
    /// that came to nothing aside, was a split of it (section 5.3, "Turns").
    pub(super) split_last: bool,
    /// When its owner was last told to prepare it for a split (section 5.6).
    pub(super) prepared: Option<Instant>,
    /// Whether an absorption it was the survivor of has ended, well or not, and no
    /// report of it has been taken since (section 5.5).
    pub(super) after_absorption: bool,
    /// Whether the last reading of the list that succeeded has it pinned to an area.
    pub(super) pinned: bool,
}

/// Since when a merge of two regions has been wanted (section 5.3).
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Waited {
    /// The first tick of the unbroken run of ticks at which it has been wanted;
    /// nothing if it was not wanted at the last tick.
    pub(super) since: Option<Instant>,
    /// Since when it has waited, which decides the order in which merges are begun
    /// and nothing else. It outlasts ticks at which the merge is not wanted.
    pub(super) waiting: Option<Instant>,
    /// The first tick of the unbroken run of ticks at which it was not wanted and
    /// the sightings of both its regions were fresh.
    pub(super) missed: Option<Instant>,
}

/// A group of a region's players that is to go, as of the last tick (section 5.3).
#[derive(Debug, Clone)]
pub(super) struct Going {
    /// The chunks its players are in, ascending.
    pub(super) chunks: Vec<ChunkPos>,
    /// The first tick of the unbroken run of ticks at which a group that this one
    /// continues was to go.
    pub(super) since: Instant,
}

/// What a coordinator that decides by itself keeps besides what it keeps of each
/// region.
#[derive(Debug, Clone, Default)]
pub(super) struct Noted {
    /// The merges that are wanted or keep a place in the order, each under its two
    /// regions, the lower first: a merge is kept by its regions, whichever of them
    /// would survive.
    pub(super) merges: BTreeMap<(RegionId, RegionId), Waited>,
    /// The groups that were to go at the last tick, by their region. A region of
    /// which no split was wanted then has none.
    pub(super) going: BTreeMap<RegionId, Vec<Going>>,
    /// When a reading of the list last succeeded.
    pub(super) listed: Option<Instant>,
    /// When a reading of the list was last answered, with a list or without.
    pub(super) answered: Option<Instant>,
}

/// The time `long` after `now`. A time that cannot be told is as late a one as can
/// be: a rest that somebody set to more than the clock can count must not stop the
/// coordinator.
fn after(now: Instant, long: Duration) -> Instant {
    let mut long = long;
    loop {
        if let Some(then) = now.checked_add(long) {
            return then;
        }
        long /= 2;
    }
}

/// Leaves a region alone until `until`, if it is not left alone for longer already.
fn leave_alone(kept: &mut Kept, until: Instant) {
    kept.alone_until = Some(kept.alone_until.map_or(until, |had| had.max(until)));
}

/// Crowds by chunk: each chunk once, ascending, with the players of every entry that
/// names it, and no chunk without players.
fn by_chunk(crowds: &Crowds) -> Crowds {
    let mut kept: BTreeMap<ChunkPos, u32> = BTreeMap::new();
    for &(chunk, players) in crowds {
        if players > 0 {
            let there = kept.entry(chunk).or_insert(0);
            *there = there.saturating_add(players);
        }
    }
    kept.into_iter().collect()
}

/// The two regions of a merge as it is kept: the lower first.
fn pair(one: RegionId, other: RegionId) -> (RegionId, RegionId) {
    (one.min(other), one.max(other))
}

/// Whether a group with `chunks` continues one that had the chunks `was`: every chunk
/// of it is at most the margin from a chunk of that one (section 5.3).
fn continues(chunks: &[ChunkPos], was: &[ChunkPos], margin: u32) -> bool {
    let near = |chunk: &ChunkPos| {
        was.iter()
            .any(|other| policy::distance(*chunk, *other) <= u64::from(margin))
    };
    chunks.iter().all(near)
}

/// Whether more than [`FRESH`] has passed since `since`: what is wanted has stood
/// then, and what has not been wanted is no longer waited for.
fn longer_than_fresh(now: Instant, since: Instant) -> bool {
    now.saturating_duration_since(since) > FRESH
}

impl Coordinator {
    /// Takes or passes over each entry of what the worker `name` says of its regions'
    /// players (section 2.3).
    pub(super) fn take_players(&mut self, now: Instant, name: &str, regions: &[PlayersOf]) {
        let Some(policy) = self.config.follow else {
            return;
        };
        for said in regions {
            // Only the region's owner knows where its players are, and what it says
            // while the region is in a merge or a split may be of before or of after.
            if !self.holds(said.region, name, said.epoch) || self.reserved(said.region) {
                continue;
            }
            let state = self.regions.get_mut(&said.region);
            let kept = &mut state.expect("a region that has an owner is known").kept;
            // A region that says nothing new is not heard anew: one that stands
            // still repeats its tick, and its sighting is to stop being fresh.
            let nothing_new = kept.sighting.as_ref().is_some_and(|sighting| {
                sighting.owner == name && sighting.epoch == said.epoch && sighting.tick >= said.tick
            });
            if nothing_new {
                continue;
            }
            let crowds = by_chunk(&said.crowds);
            if crowds.is_empty() {
                kept.empty_since.get_or_insert(now);
            } else {
                kept.empty_since = None;
            }
            // The first report after an absorption decides whether the survivor
            // rests: somebody came as it began, and has stood still for it (section
            // 5.5).
            if std::mem::take(&mut kept.after_absorption) && !crowds.is_empty() {
                leave_alone(kept, after(now, policy.rest));
            }
            kept.sighting = Some(Sighting {
                owner: name.to_owned(),
                epoch: said.epoch,
                tick: said.tick,
                crowds,
                taken: Some(now),
            });
        }
    }

    /// Whether the sighting of `region` is fresh at `now` (section 2.3): it is of the
    /// region's present owner and epoch, was taken no more than [`FRESH`] ago, and
    /// the region is not part of a merge or a split under way.
    pub(super) fn fresh(&self, region: RegionId, now: Instant) -> bool {
        let state = self.regions.get(&region);
        let Some(sighting) = state.and_then(|state| state.kept.sighting.as_ref()) else {
            return false;
        };
        sighting.taken.is_some_and(|taken| {
            now.saturating_duration_since(taken) <= FRESH
                && self.holds(region, &sighting.owner, sighting.epoch)
                && !self.reserved(region)
        })
    }

    /// Notes of every region whether its owner or its epoch is not what it was when
    /// this was last called (sections 2.4 and 5.4). A region that was given an owner
    /// or an epoch the coordinator did not have for it rests from `now`; for such a
    /// region, and for one that lost its owner, the time it has been without players
    /// and its last `Prepare` are forgotten, as both were of the owner it had.
    /// Nothing has happened to a region whose owner registers again with the epoch it
    /// had, and nothing is noted of it.
    ///
    /// Called where owners can have changed and before anything goes by a rest: at
    /// the end of every call that can change an owner, and in a tick once regions
    /// have been taken and given away.
    pub(super) fn note_owners(&mut self, now: Instant) {
        let Some(policy) = self.config.follow else {
            return;
        };
        for state in self.regions.values_mut() {
            let same = match (&state.kept.held, &state.owner) {
                (Some((name, epoch)), Some(owner)) => {
                    *name == owner.worker && *epoch == state.epoch
                }
                (None, None) => true,
                _ => false,
            };
            if same {
                continue;
            }
            let kept = &mut state.kept;
            kept.empty_since = None;
            kept.prepared = None;
            kept.held = state
                .owner
                .as_ref()
                .map(|owner| (owner.worker.clone(), state.epoch));
            if kept.held.is_some() {
                leave_alone(kept, after(now, policy.rest));
            }
        }
    }

    /// A merge or a split of `regions` begins, whoever asked for it (section 2.4):
    /// their sightings stay and are not fresh until a report is taken after the end,
    /// and the time each has been without players and its last `Prepare` are
    /// forgotten.
    pub(super) fn note_begun(&mut self, regions: &[RegionId]) {
        if self.config.follow.is_none() {
            return;
        }
        for region in regions {
            let Some(state) = self.regions.get_mut(region) else {
                continue;
            };
            if let Some(sighting) = &mut state.kept.sighting {
                sighting.taken = None;
            }
            state.kept.empty_since = None;
            state.kept.prepared = None;
        }
    }

    /// An attempt with `region` has failed (section 5.5): it is left alone for
    /// `LONG`, twice as long for every failure it has had in a row before this one,
    /// 8 times `LONG` at most, and has one failure more. Nothing, if the region is
    /// no more.
    fn note_failed(&mut self, now: Instant, region: RegionId) {
        let Some(policy) = self.config.follow else {
            return;
        };
        let Some(state) = self.regions.get_mut(&region) else {
            return;
        };
        let kept = &mut state.kept;
        let long = policy
            .rest
            .saturating_mul(RESTS)
            .saturating_mul(1 << kept.failures.min(DOUBLINGS));
        leave_alone(kept, after(now, long));
        kept.failures = kept.failures.saturating_add(1);
    }

    /// The merge of `absorbed` into `survivor` has ended at `now`, well or not,
    /// whoever asked for it (section 5.5).
    pub(super) fn note_merge_ended(
        &mut self,
        now: Instant,
        survivor: RegionId,
        absorbed: RegionId,
        absorption: bool,
        well: bool,
    ) {
        let Some(policy) = self.config.follow else {
            return;
        };
        if absorption {
            // Nobody stood still on the survivor's side, so it neither rests nor
            // has a failure counted; but its next report says whether somebody came
            // as the absorption began.
            if let Some(state) = self.regions.get_mut(&survivor) {
                state.kept.after_absorption = true;
            }
            if !well {
                self.note_failed(now, absorbed);
            }
            return;
        }
        if !well {
            self.note_failed(now, survivor);
            self.note_failed(now, absorbed);
            return;
        }
        if let Some(state) = self.regions.get_mut(&survivor) {
            let kept = &mut state.kept;
            leave_alone(kept, after(now, policy.rest));
            kept.failures = 0;
            kept.not_yet = 0;
            kept.split_last = false;
        }
    }

    /// The split of `region` that was noted as `split` has ended at `now` with
    /// `outcome`, whoever asked for it (sections 2.4 and 5.5). However it ended, it
    /// was the region's turn at splitting (section 5.3).
    pub(super) fn note_split_ended(
        &mut self,
        now: Instant,
        region: RegionId,
        split: &Split,
        outcome: &Result<RegionId, Undone>,
    ) {
        let Some(policy) = self.config.follow else {
            return;
        };
        if let Ok(part) = outcome {
            self.move_crowds(region, *part, split);
        }
        let Some(state) = self.regions.get_mut(&region) else {
            return;
        };
        let kept = &mut state.kept;
        kept.split_last = true;
        let not_yet = |why: &Undone| {
            matches!(
                why,
                Undone::Off(
                    Off::Nobody
                        | Off::NothingStays
                        | Off::Busy
                        | Off::NotRunning
                        | Off::Declined(Decline::NotNext { .. })
                )
            )
        };
        let failed = match outcome {
            Ok(_) => {
                leave_alone(kept, after(now, policy.rest));
                kept.failures = 0;
                kept.not_yet = 0;
                false
            }
            // The players had moved on or left, or the region was in the middle of
            // something: it has stopped for a few ticks, and rests as after a split
            // that was made. The third such answer in a row is a failure, and the
            // count of such answers begins anew.
            Err(why) if not_yet(why) => {
                leave_alone(kept, after(now, policy.rest));
                kept.not_yet += 1;
                let third = kept.not_yet >= NOT_YET;
                if third {
                    kept.not_yet = 0;
                }
                third
            }
            Err(_) => true,
        };
        if failed {
            self.note_failed(now, region);
        }
    }

    /// The crowds in the chunks that `split` named go from the sighting of `region`
    /// to a sighting of `part`, which is not fresh (section 2.4): the players who
    /// stood there are the part's now, as far as anybody has said.
    fn move_crowds(&mut self, region: RegionId, part: RegionId, split: &Split) {
        // A part that the coordinator does not take for a region has no sighting,
        // and what was last heard of those players stays where it was heard.
        if part == region || !self.regions.contains_key(&part) {
            return;
        }
        let named: BTreeSet<ChunkPos> = split.chunks.iter().copied().collect();
        let state = self.regions.get_mut(&region);
        // Section 2.4 says nothing of a region that was split without ever having
        // been heard of, which only a split by hand can be. No sighting is made for
        // its part then: nobody has said a word of who is there.
        let Some(sighting) = state.and_then(|state| state.kept.sighting.as_mut()) else {
            return;
        };
        let (gone, stay): (Crowds, Crowds) = sighting
            .crowds
            .iter()
            .partition(|(chunk, _)| named.contains(chunk));
        sighting.crowds = stay;
        let kept = &mut self
            .regions
            .get_mut(&part)
            .expect("the part was found above")
            .kept;
        match &mut kept.sighting {
            // Section 2.4 has the part without a sighting. If it has one all the
            // same, the players are counted to it and it is not fresh until the
            // part's worker says where they are: ignorance holds back.
            Some(sighting) => {
                let mut crowds = std::mem::take(&mut sighting.crowds);
                crowds.extend(gone);
                sighting.crowds = by_chunk(&crowds);
                sighting.taken = None;
            }
            // Of the part's owner and epoch, which are the worker that split it off
            // and the epoch it was told. A tick of 0, so that whatever the worker
            // says of the part is newer.
            None => {
                kept.sighting = Some(Sighting {
                    owner: split.owner.clone(),
                    epoch: split.as_epoch,
                    tick: 0,
                    crowds: gone,
                    taken: None,
                });
            }
        }
    }

    /// A reading of the list has taken `gone` away, of which `kept` was kept; `into`
    /// is the living region it was absorbed by, by the pairs of that reading, if it
    /// was absorbed and there is one (sections 2.4 and 5.3).
    pub(super) fn note_gone(&mut self, gone: RegionId, kept: Kept, into: Option<RegionId>) {
        if self.config.follow.is_none() {
            return;
        }
        // Its players are in the region that took them, if anybody has said a word
        // of that one. No sighting is made for a region that has none: it would
        // count as heard of without a worker's word.
        let survivor = into.and_then(|into| self.regions.get_mut(&into));
        let sighting = survivor.and_then(|state| state.kept.sighting.as_mut());
        if let (Some(sighting), Some(was)) = (sighting, kept.sighting) {
            let mut crowds = std::mem::take(&mut sighting.crowds);
            crowds.extend(was.crowds);
            sighting.crowds = by_chunk(&crowds);
        }

        self.noted.going.remove(&gone);
        let with_it: Vec<(RegionId, RegionId)> = self
            .noted
            .merges
            .keys()
            .filter(|(lower, higher)| *lower == gone || *higher == gone)
            .copied()
            .collect();
        for key in with_it {
            let waited = self.noted.merges.remove(&key);
            let waited = waited.expect("the merge was found a moment ago");
            let other = if key.0 == gone { key.1 } else { key.0 };
            // What waited for a merge with it waits for a merge with the region it
            // went into, unless that is the merge's other region: a merge of a
            // region with itself is forgotten.
            let Some(into) = into.filter(|into| *into != other) else {
                continue;
            };
            let Some(waiting) = waited.waiting else {
                continue;
            };
            let moved = self.noted.merges.entry(pair(other, into)).or_default();
            moved.waiting = Some(moved.waiting.map_or(waiting, |had| had.min(waiting)));
        }
    }

    /// A reading of the list has succeeded at `now` and has been applied (section 7):
    /// when that was is noted, and of every region whether the list has it pinned to
    /// an area. Nothing is kept of a region the coordinator does not know.
    pub(super) fn note_listed(&mut self, now: Instant, list: &RegionList) {
        if self.config.follow.is_none() {
            return;
        }
        let listed = self.noted.listed;
        self.noted.listed = Some(listed.map_or(now, |had| had.max(now)));
        let pinned: BTreeSet<RegionId> = list
            .regions
            .iter()
            .filter(|info| !info.pinned.is_empty())
            .map(|info| info.region)
            .collect();
        for (region, state) in &mut self.regions {
            // A region the reading does not show, which is a part, is not.
            state.kept.pinned = pinned.contains(region);
        }
        let regions = &self.regions;
        self.noted.merges.retain(|(lower, higher), _| {
            regions.contains_key(lower) && regions.contains_key(higher)
        });
        self.noted
            .going
            .retain(|region, _| regions.contains_key(region));
    }

    /// A reading of the list has been answered at `now`, with a list or without: the
    /// next one is asked for a `LIST_EVERY` from this answer, not at every tick of a
    /// store that is away (section 7).
    pub(super) fn note_answered(&mut self, now: Instant) {
        if self.config.follow.is_none() {
            return;
        }
        let answered = self.noted.answered;
        self.noted.answered = Some(answered.map_or(now, |had| had.max(now)));
    }

    /// Whether the list is to be read at `now` because no reading has been answered
    /// for a `LIST_EVERY`, which is one lease, or none ever (section 7). Only a
    /// coordinator that decides by itself reads the list by the time, and whoever
    /// asks by this waits for the answer before asking again.
    pub(super) fn list_is_due(&self, now: Instant) -> bool {
        let lease = self.config.lease;
        let due = |answered: Instant| now.saturating_duration_since(answered) >= lease;
        self.config.follow.is_some() && self.noted.answered.is_none_or(due)
    }

    /// Whether nothing but its rest keeps the coordinator from beginning something
    /// with `region` by itself at `now` (section 5.2): it has an owner that has a
    /// connection, is not leaving and is not at fault, it is not part of a merge or a
    /// split under way, and it is not being released.
    fn free_but_for_its_rest(&self, region: RegionId, now: Instant) -> bool {
        let state = self.regions.get(&region);
        let owner = state.and_then(|state| state.owner.as_ref());
        let worker = owner.and_then(|owner| self.workers.get(&owner.worker));
        worker.is_some_and(|worker| {
            worker.connected && !worker.leaving && !self.at_fault(worker, now)
        }) && !self.reserved(region)
            && !self.releases.contains_key(&region)
    }

    /// How long from `now` the region is still left alone.
    fn rest_left(&self, region: RegionId, now: Instant) -> Duration {
        let state = self.regions.get(&region);
        let until = state.and_then(|state| state.kept.alone_until);
        until.map_or(Duration::ZERO, |until| until.saturating_duration_since(now))
    }

    /// Whether `region` is free at `now` (section 5.2).
    fn free(&self, region: RegionId, now: Instant) -> bool {
        self.free_but_for_its_rest(region, now) && self.rest_left(region, now).is_zero()
    }

    /// Whether `region` is free at `now` or will be within [`FRESH`], if nothing
    /// else happens to it (section 5.2).
    fn nearly_free(&self, region: RegionId, now: Instant) -> bool {
        self.free_but_for_its_rest(region, now) && self.rest_left(region, now) <= FRESH
    }

    /// Since when `region` has been without players, if its sighting is fresh at
    /// `now` and has none.
    fn without_players_since(&self, region: RegionId, now: Instant) -> Option<Instant> {
        let kept = &self.regions.get(&region)?.kept;
        let empty = kept.sighting.as_ref()?.crowds.is_empty();
        (empty && self.fresh(region, now))
            .then_some(kept.empty_since)
            .flatten()
    }

    /// What has to hold of the world for anything to be begun by itself (section
    /// 5.1), but for how many merges and splits are under way and whether an epoch
    /// is left: the grace period is over, the last reading of the list that
    /// succeeded is no more than two `LIST_EVERY` old, and every region the
    /// coordinator knows has a sighting.
    fn the_world_is_known(&self, now: Instant) -> bool {
        let lease = self.config.lease;
        let grace = now.saturating_duration_since(self.started) < lease;
        let read = self
            .noted
            .listed
            .is_some_and(|listed| now.saturating_duration_since(listed) <= lease.saturating_mul(2));
        let sighted = |state: &super::Region| state.kept.sighting.is_some();
        !grace && read && self.regions.values().all(sighted)
    }

    /// Whether one more merge or split may be begun: fewer than [`AT_ONCE`] are
    /// under way, and an epoch is left to issue (section 5.1).
    fn one_more_may_begin(&self) -> bool {
        self.merges.len() + self.splits.len() < AT_ONCE && self.last_epoch < u64::MAX
    }

    /// Whether the merge of the two regions `key` has stood at `now`.
    fn merge_has_stood(&self, key: (RegionId, RegionId), now: Instant) -> bool {
        let since = self.noted.merges.get(&key).and_then(|waited| waited.since);
        since.is_some_and(|since| longer_than_fresh(now, since))
    }

    /// What a tick of a coordinator that decides by itself does between settling
    /// and evening out (section 5): works out what is wanted, notes since when, and
    /// begins what may be begun.
    ///
    /// Nothing is wanted before the list has said which region is home.
    pub(super) fn decide_and_begin(&mut self, now: Instant) {
        let (Some(policy), Some(home)) = (self.config.follow, self.home) else {
            return;
        };
        let spawn = self.config.spawn;
        let enter = ChunkPos::containing(spawn.x, spawn.z);
        let fresh: BTreeSet<RegionId> = self
            .regions
            .keys()
            .filter(|region| self.fresh(**region, now))
            .copied()
            .collect();
        let wanted = {
            let sighted: Vec<Sighted<'_>> = self
                .regions
                .iter()
                .filter_map(|(region, state)| {
                    let sighting = state.kept.sighting.as_ref()?;
                    Some(Sighted {
                        region: *region,
                        fresh: fresh.contains(region),
                        crowds: &sighting.crowds,
                    })
                })
                .collect();
            policy::decide(&policy, enter, home, &sighted)
        };
        // Whether or not anything may be begun: what is wanted stands while its
        // regions rest, and while the world is not known.
        self.note_wanted(now, &policy, &wanted, &fresh);

        if self.the_world_is_known(now) {
            self.begin_a_split(now, &policy, &wanted);
            self.begin_merges(now, &wanted);
            self.begin_absorptions(now, &policy, home, &wanted);
            self.say_prepare(now);
        }
        // A `Prepare` that no split followed is forgotten when it is more than a
        // rest old and no split is wanted, so that a group that parts and comes
        // back has it said once in a rest at most (section 5.6).
        let going = &self.noted.going;
        for (region, state) in &mut self.regions {
            let old =
                |prepared: &mut Instant| now.saturating_duration_since(*prepared) > policy.rest;
            if !going.contains_key(region) {
                state.kept.prepared.take_if(old);
            }
        }
    }

    /// Notes since when each thing that is wanted at `now` has been wanted without
    /// a break, and what of the tick before is not wanted any more (section 5.3,
    /// "Standing" and "Waiting"). `fresh` are the regions whose sighting is fresh.
    fn note_wanted(
        &mut self,
        now: Instant,
        policy: &Policy,
        wanted: &[Wanted],
        fresh: &BTreeSet<RegionId>,
    ) {
        let margin = policy.margin();
        let mut merges = BTreeSet::new();
        let mut going = BTreeMap::new();
        for want in wanted {
            match want {
                Wanted::Merge {
                    survivor, absorbed, ..
                } => {
                    merges.insert(pair(*survivor, *absorbed));
                }
                Wanted::Split { region, groups, .. } => {
                    let were = self.noted.going.get(region);
                    let were = || were.into_iter().flatten();
                    let groups: Vec<Going> = groups
                        .iter()
                        .map(|chunks| {
                            // A group continues one group at most where twice the
                            // margin is less than the split distance. With
                            // distances that nobody checked it could continue
                            // several, and then has the time of the latest: when
                            // in doubt the time begins later.
                            let continued = were()
                                .filter(|was| continues(chunks, &was.chunks, margin))
                                .map(|was| was.since)
                                .max();
                            Going {
                                chunks: chunks.clone(),
                                since: continued.unwrap_or(now),
                            }
                        })
                        .collect();
                    going.insert(*region, groups);
                }
            }
        }
        // A tick at which no split is wanted of a region forgets all its groups.
        self.noted.going = going;

        for key in &merges {
            let waited = self.noted.merges.entry(*key).or_default();
            waited.since.get_or_insert(now);
            waited.waiting.get_or_insert(now);
            waited.missed = None;
        }
        self.noted.merges.retain(|key, waited| {
            if merges.contains(key) {
                return true;
            }
            waited.since = None;
            // While one of its regions is in something else, or silent, the merge
            // keeps its place: it is not wanted because nothing is known, not
            // because the players have parted. A region without a sighting counts
            // as one whose sighting is not fresh.
            if !(fresh.contains(&key.0) && fresh.contains(&key.1)) {
                waited.missed = None;
                return true;
            }
            // It loses its place as slowly as a thing is believed: one look that
            // misleads does not cost it.
            let missed = *waited.missed.get_or_insert(now);
            !longer_than_fresh(now, missed)
        });
    }

    /// Step 1 of the order (section 5.3): one split at most, of the region whose
    /// group has stood longest.
    fn begin_a_split(&mut self, now: Instant, policy: &Policy, wanted: &[Wanted]) {
        // One split at a time in the whole world, whoever asked for it; and none
        // while a reading of the list is asked for or owed, as that reading may add
        // a region that a split under way would have it leave out.
        if !self.one_more_may_begin() || !self.splits.is_empty() || self.reading || self.owed {
            return;
        }
        let mut first: Option<(Instant, RegionId)> = None;
        for (region, groups) in &self.noted.going {
            let stood = groups
                .iter()
                .map(|group| group.since)
                .filter(|since| longer_than_fresh(now, *since))
                .min();
            let Some(since) = stood else {
                continue;
            };
            if !self.free(*region, now) || self.passed_over(*region, now, wanted) {
                continue;
            }
            if first.is_none_or(|first| (since, *region) < first) {
                first = Some((since, *region));
            }
        }
        let Some((_, region)) = first else {
            return;
        };
        // The groups that have stood, and no others: a group that is there for one
        // look does not go on that look.
        let groups: Vec<&[ChunkPos]> = self.noted.going[&region]
            .iter()
            .filter(|group| longer_than_fresh(now, group.since))
            .map(|group| group.chunks.as_slice())
            .collect();
        let named = policy::named(policy, &groups);
        let groups = groups.len();
        match self.begin_split(now, region, &named, None) {
            Ok(()) => {
                let part = self.next.expect("a split names the next id of the list");
                let chunks = named.len();
                info!(target: LOG, %region, %part, groups, chunks, "a split is begun by itself");
            }
            Err(refusal) => self.refused(now, &[region], &refusal),
        }
    }

    /// Whether `region` is passed over for a split at `now` because it is the turn
    /// of a merge (section 5.3, "Turns"): it was split last, and a merge of it has
    /// stood whose two regions are free.
    fn passed_over(&self, region: RegionId, now: Instant, wanted: &[Wanted]) -> bool {
        let split_last = self.regions.get(&region);
        if !split_last.is_some_and(|state| state.kept.split_last) {
            return false;
        }
        wanted.iter().any(|want| {
            let Wanted::Merge {
                survivor, absorbed, ..
            } = want
            else {
                return false;
            };
            (*survivor == region || *absorbed == region)
                && self.merge_has_stood(pair(*survivor, *absorbed), now)
                && self.free(*survivor, now)
                && self.free(*absorbed, now)
        })
    }

    /// Step 2 of the order (section 5.3): the merges that have stood, the one that
    /// has waited longest first.
    fn begin_merges(&mut self, now: Instant, wanted: &[Wanted]) {
        let mut stood: Vec<_> = wanted
            .iter()
            .filter_map(|want| {
                let Wanted::Merge {
                    survivor,
                    absorbed,
                    gap,
                    ..
                } = want
                else {
                    return None;
                };
                let key = pair(*survivor, *absorbed);
                let waited = self.noted.merges.get(&key)?;
                let since = waited
                    .since
                    .filter(|since| longer_than_fresh(now, *since))?;
                // Of several that have waited since the same tick the one with
                // the smaller gap, then by their regions.
                let waiting = waited.waiting.unwrap_or(since);
                Some((waiting, *gap, key, *survivor, *absorbed))
            })
            .collect();
        stood.sort_unstable();
        for (_, gap, _, survivor, absorbed) in stood {
            if !self.one_more_may_begin() {
                break;
            }
            // A region that something was begun with at this tick is reserved, and
            // so not free.
            if !(self.free(survivor, now) && self.free(absorbed, now)) {
                continue;
            }
            match self.begin_merge(now, survivor, absorbed, None, false) {
                Ok(()) => {
                    info!(target: LOG, %survivor, %absorbed, gap, "a merge is begun by the distances");
                }
                Err(refusal) => self.refused(now, &[survivor, absorbed], &refusal),
            }
        }
    }

    /// Step 3 of the order (section 5.3): the regions without players that are to
    /// be absorbed, the highest first, each by its survivor as of that moment
    /// (section 4.4).
    fn begin_absorptions(
        &mut self,
        now: Instant,
        policy: &Policy,
        home: RegionId,
        wanted: &[Wanted],
    ) {
        // A region that a merge is wanted of is no survivor, whether that merge has
        // stood or not: it is about to have players.
        let mut merging = BTreeSet::new();
        for want in wanted {
            if let Wanted::Merge {
                survivor, absorbed, ..
            } = want
            {
                merging.extend([*survivor, *absorbed]);
            }
        }
        let empty_for = policy.rest.saturating_mul(RESTS);
        let regions: Vec<RegionId> = self.regions.keys().rev().copied().collect();
        for &region in &regions {
            if !self.one_more_may_begin() {
                break;
            }
            // Never the home region, and never a region that is pinned to an area:
            // it holds its areas whether anybody is there or not.
            let pinned = self.regions[&region].kept.pinned;
            if region == home || pinned || !self.free(region, now) {
                continue;
            }
            let empty = self.without_players_since(region, now);
            if !empty.is_some_and(|since| now.saturating_duration_since(since) >= empty_for) {
                continue;
            }
            // The home region, then the regions with a lower id, the lowest first.
            // A survivor's rest is not looked at: nobody stands still for this.
            let lower = regions
                .iter()
                .rev()
                .copied()
                .filter(|other| *other != home && *other < region);
            let survivor = [home].into_iter().chain(lower).find(|other| {
                let empty = self.without_players_since(*other, now);
                empty.is_some_and(|since| longer_than_fresh(now, since))
                    && self.free_but_for_its_rest(*other, now)
                    && !merging.contains(other)
            });
            let Some(survivor) = survivor else {
                continue;
            };
            match self.begin_merge(now, survivor, region, None, true) {
                Ok(()) => {
                    let absorbed = region;
                    info!(target: LOG, %survivor, %absorbed, "an absorption is begun by itself");
                }
                Err(refusal) => self.refused(now, &[survivor, region], &refusal),
            }
        }
    }

    /// Step 4 of the order (sections 5.3 and 5.6): the owner of every region of
    /// which a split is wanted, that is free or will be within a second, and that
    /// has not been told since it last mattered, is told to prepare it, so that the
    /// split's own checkpoint is short.
    fn say_prepare(&mut self, now: Instant) {
        // The number under way does not come into it; that an epoch is left does.
        if self.last_epoch == u64::MAX {
            return;
        }
        let regions: Vec<RegionId> = self.noted.going.keys().copied().collect();
        for region in regions {
            if !self.nearly_free(region, now) {
                continue;
            }
            let Some((worker, epoch)) = self.owner_of(region) else {
                continue;
            };
            let state = self.regions.get_mut(&region);
            let kept = &mut state.expect("a region that has an owner is known").kept;
            if kept.prepared.is_some() {
                continue;
            }
            kept.prepared = Some(now);
            info!(%region, %worker, epoch, "a region is to prepare for being split");
            let order = Order::Prepare { region, epoch };
            self.pending.reshapes.push(ReshapeOrder { worker, order });
        }
    }

    /// A merge or a split that the coordinator began by itself was refused, which
    /// the rules that chose it are written not to let happen (section 5.2): it is
    /// logged as a fault of this code, and the regions are left alone for a rest.
    fn refused(&mut self, now: Instant, regions: &[RegionId], refusal: &ReshapeRefusal) {
        let Some(policy) = self.config.follow else {
            return;
        };
        warn!(
            ?regions,
            %refusal,
            "what the coordinator began by itself was refused, which is a fault of its rules"
        );
        for region in regions {
            if let Some(state) = self.regions.get_mut(region) {
                leave_alone(&mut state.kept, after(now, policy.rest));
            }
        }
    }

    /// Whether a merge or a split was wanted of `region` at the last tick that
    /// worked out what is wanted, whether it has stood or not.
    fn wanted_of(&self, region: RegionId) -> bool {
        let merges = self.noted.merges.iter();
        let mut wanted = merges.filter(|(_, waited)| waited.since.is_some());
        self.noted.going.contains_key(&region)
            || wanted.any(|((lower, higher), _)| *lower == region || *higher == region)
    }

    /// The region of the worker `heavy` that is released to even regions out at
    /// `now`, with its epoch, when the coordinator decides by itself (section 6): of
    /// those that are not at rest and that no merge and no split is wanted of at
    /// this tick, the one with the fewest players by its sighting, fresh or not, and
    /// of several such the one with the highest id. A region without a sighting
    /// counts as having more than any. `None` if none is left, and nothing is evened
    /// out at this tick then.
    ///
    /// A region that a merge is about to stand still for would otherwise be moved
    /// and merged a rest later, two stops where one does, and one that is about to
    /// be split would be moved with players who are about to leave it.
    pub(super) fn region_to_even_out(&self, heavy: &str, now: Instant) -> Option<(RegionId, u64)> {
        let players = |kept: &Kept| {
            let sighting = kept.sighting.as_ref()?;
            let counts = sighting
                .crowds
                .iter()
                .map(|(_, players)| u64::from(*players));
            Some(counts.fold(0_u64, u64::saturating_add))
        };
        self.regions
            .iter()
            .filter(|(_, state)| {
                let owner = state.owner.as_ref();
                owner.is_some_and(|owner| owner.worker == heavy)
            })
            .filter(|(region, _)| !self.wanted_of(**region))
            .filter(|(region, _)| self.rest_left(**region, now).is_zero())
            .min_by_key(|(region, state)| {
                let players = players(&state.kept);
                (players.is_none(), players, std::cmp::Reverse(**region))
            })
            .map(|(region, state)| (*region, state.epoch))
    }
}
