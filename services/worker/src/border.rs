//! What a runner keeps of the region's borders (`docs/adr/0025-effects-between-regions.md`):
//! the effects that came on its stream and wait until a tick can take them, the border
//! copies with their bases and updates, its couplings and the progress of its partners,
//! the effects its ticks made that are not sent yet, and the use timers of chunks the
//! store counts as in use and the region has not loaded.
//!
//! The store's stream comes in order. A copy is handed to the sim only once its base is
//! there and who uses its chunk is nobody or a coupled region; an effect only once its
//! chunk is loaded, every copy in the 3x3 of its chunk is based and the stream's mark has
//! reached it.

use std::collections::{BTreeMap, BTreeSet};

use clustine_region::RegionId;
use clustine_rpc::{CopyUse, Coupling, StoreReply, Uncoupling};
use clustine_sim::api::TickInputs;
use clustine_world::{
    Change, Chunk, ChunkPos, Due, Effect, EffectId, EffectKey, Effected, FiledEffect,
};

/// At most this many ready effects go to one tick, the rest to the next (section 5).
pub(crate) const MAX_EFFECTS_PER_TICK: usize = 4096;

/// At most this many effects go into one commit, so that its record stays within the
/// log's limit; the rest go into the next ("Found while building", E3a).
pub(crate) const MAX_EFFECTS_PER_COMMIT: usize = 65_536;

/// A copy the region keeps.
#[derive(Default)]
struct Copy {
    /// What the store based it on and has sent since, applied: the sequence of the base,
    /// the last change applied, and the chunk, or none where the copy is absent.
    base: Option<Based>,
    /// Updates that came before the base, to be applied above its sequence.
    early: Vec<Update>,
    /// Who uses the chunk, as the store said last.
    used: Option<CopyUse>,
    /// Whether the sim has it.
    handed: bool,
    /// Changes the sim has not had of a copy it has.
    unsent: Vec<Change>,
}

struct Based {
    sequence: u64,
    last: u64,
    chunk: Option<Chunk>,
}

struct Update {
    sequence: u64,
    previous: u64,
    changes: Vec<Change>,
}

/// What the runner asks the store for, from what came in.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Asks {
    /// Copies to be based anew, after a gap in their updates.
    pub(crate) rebase: Vec<ChunkPos>,
    /// The answers to handshakes: the partner and the tick held for it.
    pub(crate) joins: Vec<(RegionId, u64)>,
}

#[derive(Default)]
pub(crate) struct Border {
    received: BTreeMap<EffectId, FiledEffect>,
    /// The stream's last mark.
    mark: u64,
    /// What the ticks made and no commit has carried yet, deduplicated by key.
    made: Vec<Effect>,
    keyed: BTreeMap<EffectKey, usize>,
    copies: BTreeMap<ChunkPos, Copy>,
    /// The copies the store was told are beside a chunk that would tick.
    beside: BTreeSet<ChunkPos>,
    offset: Option<i64>,
    /// The partners and the border time each is durable through.
    partners: BTreeMap<RegionId, u64>,
    /// The partners of handshakes under way, and the tick held for them.
    shakes: BTreeSet<RegionId>,
    held_tick: Option<u64>,
    given_up: bool,
    /// Regions whose coupling with this one ended by a giving up, a lost owner or a
    /// reshape: copies of their chunks that the sim has stay handed in (section 8).
    lenient: BTreeSet<RegionId>,
    /// Chunks the store counts in use that the region has not loaded, each with the
    /// tick its timer began at.
    timers: BTreeMap<ChunkPos, u64>,
    /// The chunks loaded after the last tick.
    loaded: BTreeSet<ChunkPos>,
}

impl Border {
    /// As the store has the region's couplings when it is opened.
    pub(crate) fn opened(
        offset: Option<i64>,
        couplings: &[Coupling],
        handshakes: &[RegionId],
        held_tick: Option<u64>,
        given_up: bool,
    ) -> Self {
        Border {
            offset,
            partners: couplings
                .iter()
                .map(|coupling| (coupling.partner, coupling.through.unwrap_or(0)))
                .collect(),
            shakes: handshakes.iter().copied().collect(),
            held_tick: held_tick.filter(|_| !handshakes.is_empty()),
            given_up,
            ..Border::default()
        }
    }

    /// Whether the region is coupled to any region.
    pub(crate) fn coupled(&self) -> bool {
        !self.partners.is_empty()
    }

    /// Whether every tick is committed: while coupled, so that partners see the region
    /// move, and while given up and still needing coupling, so that the store sees its
    /// pace (section 8).
    pub(crate) fn commits_every_tick(&self) -> bool {
        self.coupled() || (self.given_up && !self.beside.is_empty())
    }

    /// Whether the region may run its tick `tick`: not one held for a handshake, and,
    /// coupled, only once every partner is durable through the border time before.
    pub(crate) fn may_tick(&self, tick: u64) -> bool {
        if self.held_tick.is_some_and(|held| tick >= held) {
            return false;
        }
        let Some(offset) = self.offset else {
            return true;
        };
        let border = tick as i64 + offset;
        self.partners
            .values()
            .all(|through| *through as i64 >= border - 1)
    }

    /// Takes one answer of the store's stream; returns it if it is none. `next` is the
    /// region's next tick not yet run.
    pub(crate) fn take(
        &mut self,
        reply: StoreReply,
        next: u64,
        asks: &mut Asks,
    ) -> Option<StoreReply> {
        match reply {
            StoreReply::Effects { effects } => {
                for filed in effects {
                    self.received.insert(filed.id, filed);
                }
            }
            StoreReply::Mark { through } => self.mark = self.mark.max(through),
            StoreReply::CopyBase {
                position,
                sequence,
                last_change,
                chunk,
                used,
            } => {
                let copy = self.copies.get_mut(&position)?;
                copy.base = Some(Based {
                    sequence,
                    last: last_change,
                    chunk,
                });
                copy.used = Some(used);
                // The sim takes the base anew, with what came before it applied.
                copy.handed = false;
                copy.unsent.clear();
                let early = std::mem::take(&mut copy.early);
                for update in early {
                    if update.sequence > sequence {
                        Self::apply(copy, position, update, asks);
                    }
                }
            }
            StoreReply::CopyUpdate {
                position,
                sequence,
                previous,
                changes,
            } => {
                let copy = self.copies.get_mut(&position)?;
                let update = Update {
                    sequence,
                    previous,
                    changes,
                };
                match &copy.base {
                    None => copy.early.push(update),
                    Some(base) if sequence <= base.sequence => {}
                    Some(_) => Self::apply(copy, position, update, asks),
                }
            }
            StoreReply::CopyUsed { position, used } => {
                if let Some(copy) = self.copies.get_mut(&position) {
                    copy.used = Some(used);
                }
            }
            StoreReply::CoupleWith { partner } => {
                // Coupled again after a giving up, by the store's word.
                self.given_up = false;
                self.shakes.insert(partner);
                let held = *self.held_tick.get_or_insert(next);
                asks.joins.push((partner, held));
            }
            StoreReply::Coupled { partner, offset } => {
                self.offset = Some(offset);
                self.partners.entry(partner).or_insert(0);
                self.lenient.remove(&partner);
                self.end_shake(partner);
            }
            StoreReply::HandshakeOff { partner } => self.end_shake(partner),
            StoreReply::Shifted { offset } => {
                // The partners held so far shifted with this region.
                let delta = offset - self.offset.unwrap_or(offset);
                for through in self.partners.values_mut() {
                    *through = (*through as i64 + delta).max(0) as u64;
                }
                self.offset = Some(offset);
            }
            StoreReply::Uncoupled { partner, why } => {
                self.partners.remove(&partner);
                match why {
                    Uncoupling::Unneeded => {}
                    // The store names the region given up: this one, unless the partner.
                    Uncoupling::GivenUp { region } if region != partner => {
                        self.given_up = true;
                        self.lenient.insert(partner);
                    }
                    Uncoupling::GivenUp { .. }
                    | Uncoupling::Reshaping { .. }
                    | Uncoupling::OwnerLost { .. } => {
                        self.lenient.insert(partner);
                    }
                }
                if self.partners.is_empty() {
                    self.offset = None;
                }
            }
            StoreReply::Progress { partner, through } => {
                if let Some(held) = self.partners.get_mut(&partner) {
                    *held = (*held).max(through);
                }
            }
            other => return Some(other),
        }
        None
    }

    fn end_shake(&mut self, partner: RegionId) {
        self.shakes.remove(&partner);
        if self.shakes.is_empty() {
            self.held_tick = None;
        }
    }

    /// Applies an update to a based copy, or asks for the base again at a gap.
    fn apply(copy: &mut Copy, position: ChunkPos, update: Update, asks: &mut Asks) {
        let Some(base) = &mut copy.base else {
            return;
        };
        if update.previous != base.last {
            // A gap: the copy is based anew, and the sim does without it meanwhile.
            copy.base = None;
            copy.early.clear();
            asks.rebase.push(position);
            return;
        }
        base.last = update.sequence;
        if let Some(chunk) = &mut base.chunk {
            for change in &update.changes {
                if change.position().chunk() == position {
                    // Data the copy refuses was refused to the holder's chunk too.
                    let _ = chunk.apply(change);
                }
            }
        }
        if copy.handed {
            copy.unsent.extend(update.changes);
        }
    }

    /// The copies the region wants from now on: `border` as the sim says. Returns those
    /// to ask the store for, and those it no longer wants.
    pub(crate) fn want(
        &mut self,
        border: &[ChunkPos],
    ) -> (Vec<ChunkPos>, Vec<ChunkPos>, Vec<ChunkPos>) {
        let wanted: BTreeSet<ChunkPos> = border.iter().copied().collect();
        let gone: Vec<ChunkPos> = self
            .copies
            .keys()
            .filter(|chunk| !wanted.contains(chunk))
            .copied()
            .collect();
        let mut withdrawn = Vec::new();
        for chunk in &gone {
            if self.copies.remove(chunk).is_some_and(|copy| copy.handed) {
                withdrawn.push(*chunk);
            }
        }
        let new: Vec<ChunkPos> = wanted
            .into_iter()
            .filter(|chunk| !self.copies.contains_key(chunk))
            .collect();
        for chunk in &new {
            self.copies.insert(*chunk, Copy::default());
        }
        (new, gone, withdrawn)
    }

    /// Whether the copy of `chunk` may be in the sim: based, present, and its chunk used
    /// by nobody or by a coupled region, or, once handed in, by a region whose coupling
    /// ended by a giving up, a lost owner or a reshape, or while this region is given up.
    fn handable(&self, copy: &Copy) -> bool {
        if !copy.base.as_ref().is_some_and(|base| base.chunk.is_some()) {
            return false;
        }
        match copy.used {
            None | Some(CopyUse::Nobody) => true,
            Some(CopyUse::Coupled(user) | CopyUse::Uncoupled(user)) => {
                self.partners.contains_key(&user)
                    || (copy.handed && (self.given_up || self.lenient.contains(&user)))
            }
        }
    }

    /// Fills what the coming tick is given of the borders: bases, updates and
    /// withdrawals of copies, and the ready effects, at most
    /// [`MAX_EFFECTS_PER_TICK`]. `loaded` says whether the region has a chunk loaded, and
    /// `next` is the tick the inputs are for.
    pub(crate) fn inputs(
        &mut self,
        inputs: &mut TickInputs,
        next: u64,
        loaded: impl Fn(ChunkPos) -> bool,
    ) {
        let positions: Vec<ChunkPos> = self.copies.keys().copied().collect();
        for position in positions {
            let handable = self.handable(&self.copies[&position]);
            let copy = self.copies.get_mut(&position).expect("listed above");
            match (copy.handed, handable) {
                (false, true) => {
                    let chunk = copy
                        .base
                        .as_ref()
                        .and_then(|base| base.chunk.clone())
                        .expect("a copy handed in has a chunk");
                    inputs.copy_bases.push((position, chunk));
                    copy.handed = true;
                    copy.unsent.clear();
                }
                (true, true) if !copy.unsent.is_empty() => {
                    inputs
                        .copy_updates
                        .push((position, std::mem::take(&mut copy.unsent)));
                }
                (true, false) => {
                    inputs.copies_withdrawn.push(position);
                    copy.handed = false;
                    copy.unsent.clear();
                }
                _ => {}
            }
        }
        let copies = &self.copies;
        let based = |chunk: ChunkPos| copies.get(&chunk).is_none_or(|copy| copy.base.is_some());
        let mark = self.mark;
        let ready: Vec<EffectId> = self
            .received
            .values()
            .filter(|filed| filed.id.sequence <= mark)
            // A coupled effect is done at the border time after the one it was made in,
            // also by a holder that runs behind its maker and could take it in the same
            // one: so an update across a coupled line is exactly a tick late, however
            // the two are scheduled (sections 1 and 8; found by E6). A scheduled tick is
            // due when the store says.
            .filter(|filed| match (filed.due, filed.effect.what) {
                (_, Effected::Tick { .. }) | (Due::FromTaking, _) => true,
                (Due::At { tick }, _) => next > tick,
            })
            .filter(|filed| {
                let chunk = filed.effect.at.chunk();
                loaded(chunk)
                    && (-1..=1).all(|dx| {
                        (-1..=1).all(|dz| based(ChunkPos::new(chunk.x + dx, chunk.z + dz)))
                    })
            })
            .map(|filed| filed.id)
            .take(MAX_EFFECTS_PER_TICK)
            .collect();
        for id in ready {
            if let Some(filed) = self.received.remove(&id) {
                inputs.effects.push(filed);
            }
        }
    }

    /// Effects received for a chunk the region stops using are dropped with it.
    pub(crate) fn drop_effects_of(&mut self, chunks: &BTreeSet<ChunkPos>) {
        self.received
            .retain(|_, filed| !chunks.contains(&filed.effect.at.chunk()));
    }

    /// The copies that are beside a chunk that would tick with its copies handed in, by
    /// the viewed chunks the sim has and what it has loaded. Returns those that came to
    /// be so and those that no longer are.
    pub(crate) fn beside(
        &mut self,
        viewed: &[ChunkPos],
        loaded: impl Fn(ChunkPos) -> bool,
    ) -> (Vec<ChunkPos>, Vec<ChunkPos>) {
        let mut beside = BTreeSet::new();
        for chunk in viewed {
            let around: Vec<ChunkPos> = (-1..=1)
                .flat_map(|dx| (-1..=1).map(move |dz| ChunkPos::new(chunk.x + dx, chunk.z + dz)))
                .collect();
            let would_tick = around.iter().all(|neighbour| {
                loaded(*neighbour)
                    || self.copies.get(neighbour).is_some_and(|copy| {
                        copy.base.as_ref().is_some_and(|base| base.chunk.is_some())
                    })
            });
            if would_tick {
                beside.extend(
                    around
                        .into_iter()
                        .filter(|neighbour| self.copies.contains_key(neighbour)),
                );
            }
        }
        let begins = beside.difference(&self.beside).copied().collect();
        let ends = self.beside.difference(&beside).copied().collect();
        self.beside = beside;
        (begins, ends)
    }

    /// What a tick made, kept for the next commit: one update per block, direction and
    /// kind (the latest), one scheduled tick per block and what it ticks (the earlier),
    /// one spread per block and direction (section 4).
    pub(crate) fn made(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            let Some(key) = effect.key() else {
                self.made.push(effect);
                continue;
            };
            match self.keyed.get(&key) {
                Some(at) => {
                    if !matches!(effect.what, Effected::Tick { .. }) {
                        self.made[*at] = effect;
                    }
                }
                None => {
                    self.keyed.insert(key, self.made.len());
                    self.made.push(effect);
                }
            }
        }
    }

    /// The effects for the coming commit, at most [`MAX_EFFECTS_PER_COMMIT`]; the rest
    /// wait for the next.
    pub(crate) fn take_for_commit(&mut self) -> Vec<Effect> {
        let rest = if self.made.len() > MAX_EFFECTS_PER_COMMIT {
            self.made.split_off(MAX_EFFECTS_PER_COMMIT)
        } else {
            Vec::new()
        };
        let sent = std::mem::replace(&mut self.made, rest);
        self.keyed.clear();
        for (at, effect) in self.made.iter().enumerate() {
            if let Some(key) = effect.key() {
                self.keyed.insert(key, at);
            }
        }
        sent
    }

    /// Whether effects wait for a commit.
    pub(crate) fn has_made(&self) -> bool {
        !self.made.is_empty()
    }

    /// Chunks the store counts in use that the region has not loaded, timed from `tick`.
    pub(crate) fn time(&mut self, chunks: impl IntoIterator<Item = ChunkPos>, tick: u64) {
        for chunk in chunks {
            self.timers.entry(chunk).or_insert(tick);
        }
    }

    /// A timed chunk the region asks to load needs no timer: its load begins its use.
    pub(crate) fn loading(&mut self, chunk: ChunkPos) {
        self.timers.remove(&chunk);
    }

    /// After a tick: the chunks the region stopped having loaded, those it was handed
    /// and did not take, and those whose timer ran out, which the store is told it
    /// dropped. `returned` and `warm` are not: a return ends the use by itself, and a
    /// warm chunk counts as loaded.
    pub(crate) fn dropped(
        &mut self,
        loaded: Vec<ChunkPos>,
        delivered: &[ChunkPos],
        returned: &[ChunkPos],
        warm: impl Fn(ChunkPos) -> bool,
        tick: u64,
        warm_for: u64,
    ) -> BTreeSet<ChunkPos> {
        let loaded: BTreeSet<ChunkPos> = loaded.into_iter().collect();
        let mut dropped: BTreeSet<ChunkPos> = self
            .loaded
            .iter()
            .chain(delivered)
            .filter(|chunk| !loaded.contains(chunk))
            .copied()
            .filter(|chunk| !returned.contains(chunk) && !warm(*chunk))
            .collect();
        for chunk in &loaded {
            self.timers.remove(chunk);
        }
        for chunk in returned {
            self.timers.remove(chunk);
        }
        let expired: Vec<ChunkPos> = self
            .timers
            .iter()
            .filter(|(chunk, since)| !warm(**chunk) && tick.saturating_sub(**since) >= warm_for)
            .map(|(chunk, _)| *chunk)
            .collect();
        for chunk in expired {
            self.timers.remove(&chunk);
            dropped.insert(chunk);
        }
        self.loaded = loaded;
        dropped
    }

    /// The region began anew after a merge or a split: the sim has no copies, and is
    /// handed them again; what it loads is as a restore leaves it.
    pub(crate) fn began_anew(&mut self, loaded: Vec<ChunkPos>) {
        for copy in self.copies.values_mut() {
            copy.handed = false;
            copy.unsent.clear();
        }
        self.loaded = loaded.into_iter().collect();
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use clustine_world::BlockPos;

    use super::*;

    const P: RegionId = RegionId(7);
    const C: ChunkPos = ChunkPos::new(1, 0);

    fn chunk() -> Chunk {
        use clustine_world::ChunkGenerator;
        clustine_worldgen::FlatGenerator::classic().generate(C)
    }

    fn base(sequence: u64, last: u64, used: CopyUse) -> StoreReply {
        StoreReply::CopyBase {
            position: C,
            sequence,
            last_change: last,
            chunk: Some(chunk()),
            used,
        }
    }

    fn update(sequence: u64, previous: u64) -> StoreReply {
        StoreReply::CopyUpdate {
            position: C,
            sequence,
            previous,
            changes: vec![(BlockPos::new(16, 100, 0), blocks::STONE).into()],
        }
    }

    fn take(border: &mut Border, reply: StoreReply) -> Asks {
        let mut asks = Asks::default();
        assert_eq!(border.take(reply, 50, &mut asks), None);
        asks
    }

    fn inputs(border: &mut Border) -> TickInputs {
        let mut inputs = TickInputs::default();
        border.inputs(&mut inputs, 100, |_| true);
        inputs
    }

    #[test]
    fn a_copy_is_handed_in_once_based_and_used_by_nobody_or_a_partner() {
        let mut border = Border::default();
        border.want(&[C]);
        // An update before the base waits for it, and goes in only above its sequence.
        take(&mut border, update(5, 3));
        take(&mut border, update(9, 5));
        take(&mut border, base(6, 5, CopyUse::Uncoupled(P)));
        assert!(inputs(&mut border).copy_bases.is_empty());
        take(
            &mut border,
            StoreReply::Coupled {
                partner: P,
                offset: 0,
            },
        );
        let given = inputs(&mut border);
        assert_eq!(given.copy_bases.len(), 1);
        assert_eq!(
            given.copy_bases[0].1.get(0, 100, 0),
            Some(blocks::STONE),
            "the update above the base is in it"
        );
        // Later updates follow as updates; a gap asks for the base again and withdraws.
        take(&mut border, update(12, 9));
        assert_eq!(inputs(&mut border).copy_updates.len(), 1);
        let asks = take(&mut border, update(20, 15));
        assert_eq!(asks.rebase, [C]);
        assert_eq!(inputs(&mut border).copies_withdrawn, [C]);
    }

    #[test]
    fn a_copy_used_by_a_region_not_coupled_is_withdrawn_unless_its_coupling_was_given_up() {
        let mut border = Border::default();
        border.want(&[C]);
        take(
            &mut border,
            StoreReply::Coupled {
                partner: P,
                offset: 0,
            },
        );
        take(&mut border, base(1, 1, CopyUse::Coupled(P)));
        assert_eq!(inputs(&mut border).copy_bases.len(), 1);
        let why = Uncoupling::GivenUp { region: P };
        take(&mut border, StoreReply::Uncoupled { partner: P, why });
        assert!(inputs(&mut border).copies_withdrawn.is_empty());
        assert!(!border.coupled());
        take(
            &mut border,
            StoreReply::Coupled {
                partner: P,
                offset: 0,
            },
        );
        let why = Uncoupling::Unneeded;
        take(&mut border, StoreReply::Uncoupled { partner: P, why });
        assert_eq!(inputs(&mut border).copies_withdrawn, [C]);
    }

    #[test]
    fn a_coupled_tick_waits_for_its_partners_and_a_handshake_holds_its_tick() {
        let mut border = Border::default();
        let asks = take(&mut border, StoreReply::CoupleWith { partner: P });
        assert_eq!(asks.joins, [(P, 50)]);
        assert!(border.may_tick(49));
        assert!(!border.may_tick(50));
        take(
            &mut border,
            StoreReply::Coupled {
                partner: P,
                offset: 10,
            },
        );
        take(
            &mut border,
            StoreReply::Progress {
                partner: P,
                through: 59,
            },
        );
        assert!(border.may_tick(50));
        assert!(!border.may_tick(51));
        take(&mut border, StoreReply::Shifted { offset: 20 });
        // The partner's progress shifted with the region.
        assert!(border.may_tick(50));
        assert!(!border.may_tick(51));
        assert!(border.commits_every_tick());
    }

    #[test]
    fn an_effect_is_ready_once_marked_loaded_and_its_copies_are_based() {
        let mut border = Border::default();
        border.want(&[C]);
        let filed = FiledEffect {
            id: EffectId {
                sequence: 4,
                index: 0,
            },
            effect: Effect {
                at: BlockPos::new(0, 64, 0),
                what: Effected::SetIf {
                    expected: blocks::AIR,
                    state: blocks::STONE,
                    flags: 3,
                },
            },
            due: Due::FromTaking,
        };
        take(
            &mut border,
            StoreReply::Effects {
                effects: vec![filed],
            },
        );
        assert!(inputs(&mut border).effects.is_empty());
        take(&mut border, StoreReply::Mark { through: 4 });
        assert!(
            inputs(&mut border).effects.is_empty(),
            "the copy beside it is not based"
        );
        take(&mut border, base(4, 4, CopyUse::Nobody));
        assert_eq!(inputs(&mut border).effects, [filed]);
        assert!(inputs(&mut border).effects.is_empty());
    }

    #[test]
    fn effects_made_are_kept_one_of_each_key() {
        let mut border = Border::default();
        let at = BlockPos::new(1, 2, 3);
        let tick = |delay| Effect {
            at,
            what: Effected::Tick {
                of: clustine_world::Ticked::Block(1),
                delay,
                priority: 0,
            },
        };
        let spread = |source| Effect {
            at,
            what: Effected::Spread {
                direction: 2,
                source,
            },
        };
        border.made(vec![
            tick(5),
            spread(blocks::WATER),
            tick(2),
            spread(blocks::LAVA),
        ]);
        assert_eq!(border.take_for_commit(), [tick(5), spread(blocks::LAVA)]);
        assert!(!border.has_made());
    }

    #[test]
    fn chunks_let_go_of_or_timed_out_are_dropped_and_returned_or_warm_ones_not() {
        let mut border = Border::default();
        let (a, b, c, d) = (
            ChunkPos::new(0, 0),
            ChunkPos::new(1, 0),
            ChunkPos::new(2, 0),
            ChunkPos::new(3, 0),
        );
        let e = ChunkPos::new(4, 0);
        border.dropped(vec![a, b, c], &[], &[], |_| false, 1, 10);
        border.time([d], 1);
        // `e` was handed to the tick and not taken.
        let dropped = border.dropped(vec![], &[e], &[b], |chunk| chunk == c, 2, 10);
        assert_eq!(dropped, BTreeSet::from([a, e]));
        let dropped = border.dropped(vec![], &[], &[], |_| false, 11, 10);
        assert_eq!(dropped, BTreeSet::from([d]));
    }
}
