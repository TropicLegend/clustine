//! Lending land at a pinned line (`docs/adr/0027-lending-at-a-line.md`): the asks of a
//! region that builds a circuit by its line, the lender's answers, keeping a lent chunk
//! and giving it back.
//!
//! The region asks only where a circuit's part is placed in its own pinned land within
//! reach of a chunk it does not hold (section 2). A lender decides at the end of the
//! tick it is asked in, from the world as the tick leaves it, and lets a chunk go by
//! dropping it at the start of the next, once the runner has saved it as it was
//! (section 3, step 2). A lent chunk is kept loaded, and given back the same way once
//! no circuit's part is near it (section 4).

use std::collections::BTreeSet;

use clustine_blocks::piston;
use clustine_blocks::signal::is_circuit_part;
use clustine_data::{BlockClass, BlockState};
use clustine_world::{BlockPos, ChunkPos};

use super::updates::RegionView;
use super::{Known, Region};
use crate::api::{LendAnswer, LendAsked, TickInputs, TickOutput};

/// How many of its ticks a region waits before it asks again for a lend it was told to
/// wait for, or that was called off (section 2).
pub(crate) const ASK_AGAIN_AFTER: u64 = 20;

/// How long a chunk the store said cannot be lent is not asked for again, a minute: the
/// borrower is not told when what stands in the lender's land changes, so the word
/// lapses rather than holding for good ("Found while building", L5).
const NOT_LENDABLE_FOR: u64 = 1_200;

/// How long a change of a circuit's part counts as a circuit at work, a second
/// (section 7).
const CIRCUIT_CHANGES_FOR: u64 = 20;

/// How long a lender keeps loaded what it loaded to decide a lend, if it is not asked
/// again first: five seconds (section 3, step 2).
const LEND_TICKET_FOR: u64 = 100;

/// How far a circuit's part reaches, in blocks, but a piston, which reaches as far as
/// what it moves (section 2).
const REACH: i32 = 2;

/// The eight chunks around `position`.
fn around(position: ChunkPos) -> impl Iterator<Item = ChunkPos> {
    (-1..=1)
        .flat_map(move |dx| (-1..=1).map(move |dz| (dx, dz)))
        .filter(|(dx, dz)| (*dx, *dz) != (0, 0))
        .map(move |(dx, dz)| ChunkPos::new(position.x + dx, position.z + dz))
}

/// The chunks of the blocks within `REACH` of `pos` along x and z.
fn chunks_near(pos: BlockPos, into: &mut BTreeSet<ChunkPos>) {
    for dx in [-REACH, 0, REACH] {
        for dz in [-REACH, 0, REACH] {
            into.insert(BlockPos::new(pos.x + dx, pos.y, pos.z + dz).chunk());
        }
    }
}

impl Region {
    /// The chunks a circuit's part `state` at `pos` reaches: those within two blocks,
    /// and for a piston those of what it would move and where to, slime's and honey's
    /// sideways included, as the resolver finds them now (section 2).
    fn reach(&mut self, pos: BlockPos, state: BlockState) -> BTreeSet<ChunkPos> {
        let mut reached = BTreeSet::new();
        chunks_near(pos, &mut reached);
        if piston::is_piston_base(state)
            && let Some(facing) = clustine_blocks::data::facing(state)
        {
            let heights = self.heights_at(pos);
            let head = clustine_blocks::data::relative(pos, facing);
            reached.insert(head.chunk());
            if let Some(structure) =
                piston::resolve(&mut RegionView(self), pos, facing, true, heights)
            {
                for moved in structure.to_push.iter().chain(&structure.to_destroy) {
                    reached.insert(moved.chunk());
                    let to = clustine_blocks::data::relative(*moved, structure.push_direction);
                    reached.insert(to.chunk());
                }
            }
        }
        reached
    }

    /// Whether a circuit's part in the loaded chunk `position` reaches one of `into`.
    /// Sections whose palettes hold no part are passed over at once.
    fn reaches_into(&mut self, position: ChunkPos, into: &BTreeSet<ChunkPos>) -> bool {
        let Some(chunk) = self.chunks.get(&position) else {
            return false;
        };
        let min_y = chunk.min_y();
        let mut parts = Vec::new();
        for (index, section) in chunk.sections().iter().enumerate() {
            if !section.may_hold(is_circuit_part) {
                continue;
            }
            let base_y = min_y + 16 * index as i32;
            for y in 0..16 {
                for z in 0..16 {
                    for x in 0..16 {
                        let state = section.get(x, y, z);
                        if is_circuit_part(state) {
                            let pos = BlockPos::new(
                                position.x * 16 + x as i32,
                                base_y + y as i32,
                                position.z * 16 + z as i32,
                            );
                            parts.push((pos, state));
                        }
                    }
                }
            }
        }
        parts.into_iter().any(|(pos, state)| {
            self.reach(pos, state)
                .iter()
                .any(|chunk| into.contains(chunk))
        })
    }

    /// Whether the region asks for a lend of `position`: it neither holds the chunk nor
    /// is pinned there, it was not told that the chunk cannot be lent, and is not asking
    /// already.
    fn may_ask_for(&self, position: ChunkPos) -> bool {
        !self.land.holds(position)
            && !self.land.is_pinned(position)
            && !self
                .land
                .not_lendable
                .get(&position)
                .is_some_and(|told| self.tick.saturating_sub(*told) < NOT_LENDABLE_FOR)
            && !self.lends_asked.contains_key(&position)
    }

    /// A circuit's part was placed at `pos`: where that is the region's own pinned land,
    /// the chunks within its reach that the region does not hold are asked to be lent
    /// (section 2). Nothing but a placement asks.
    pub(super) fn ask_lends_for(&mut self, pos: BlockPos, output: &mut TickOutput) {
        let Some(state) = self.own_block(pos) else {
            return;
        };
        if !is_circuit_part(state) || !self.land.is_pinned(pos.chunk()) {
            return;
        }
        for position in self.reach(pos, state) {
            if self.may_ask_for(position) {
                self.lends_asked.insert(position, self.tick);
                self.journal.lends_asked.insert(position);
                output.lends.push(position);
            }
        }
    }

    /// A circuit's part at `pos` was replaced by something else: the lent chunks near
    /// it are looked at for giving back at the end of the tick (section 4).
    pub(super) fn part_gone_near(&mut self, pos: BlockPos) {
        if self.land.lent.is_empty() {
            return;
        }
        let mut near = BTreeSet::new();
        chunks_near(pos, &mut near);
        let chunk = pos.chunk();
        near.extend(around(chunk));
        near.insert(chunk);
        for position in near {
            if self.land.lent.contains(&position) {
                self.lent_to_check.insert(position);
            }
        }
    }

    /// The first thing of a tick: what was let go of at the end of the last is dropped,
    /// as a chunk whose last ticket went is (section 3, step 2, and section 4).
    pub(super) fn drop_let_go(&mut self) {
        for position in std::mem::take(&mut self.letting_go) {
            self.chunks.remove(&position);
            self.let_go_events(position);
            self.let_go_block_entities(position);
            self.let_go_entities(position);
            self.requested.remove(&position);
        }
    }

    /// The store's answers about lends, with the other answers about land at the start
    /// of the tick. `inputs.granted` has been taken already.
    pub(super) fn take_lend_answers(&mut self, inputs: &TickInputs) {
        // A chunk asked for and granted is lent: kept loaded from now on.
        for position in &inputs.granted {
            if self.lends_asked.remove(position).is_some() {
                self.journal.lends_asked.insert(*position);
                self.land.lent.insert(*position);
                self.land.not_lendable.remove(position);
            }
        }
        for (chunks, again) in &inputs.not_lent {
            for position in chunks {
                if !*again && self.lends_asked.remove(position).is_some() {
                    self.journal.lends_asked.insert(*position);
                    self.land.not_lendable.insert(*position, self.tick);
                }
            }
        }
        for (lend, chunks) in &inputs.lends_called_off {
            // The lender takes the chunk up again as its tickets want it; whoever asked
            // keeps its ask, which it asks again in its time.
            for position in chunks {
                if self.land.lending.get(position) == Some(lend) {
                    self.land.lending.remove(position);
                }
            }
        }
        for (chunks, to) in &inputs.lent {
            for position in chunks {
                self.land.lending.remove(position);
                self.land.known.insert(*position, Known::Foreign(*to));
                self.land.lent_away.insert(*position, *to);
                self.pistons_may_retry = true;
            }
        }
        for position in &inputs.pinned_back {
            if self.land.lent_away.remove(position).is_some()
                && matches!(self.land.known.get(position), Some(Known::Foreign(_)))
            {
                self.land.known.remove(position);
            }
            // What the region knew of the chunk changed.
            self.land.not_lendable.remove(position);
        }
        for asked in &inputs.lends_asked {
            // Seen again: the chunks loaded for it are kept no longer than this tick.
            for position in &asked.chunks {
                if let Some(until) = self.lend_tickets.get_mut(position) {
                    *until = self.tick;
                }
            }
            self.lends_to_answer.push(asked.clone());
        }
        // A chunk the region holds needs no lend, however it came to hold it.
        let held: Vec<ChunkPos> = self
            .lends_asked
            .keys()
            .copied()
            .filter(|position| self.land.holds(*position))
            .collect();
        for position in held {
            self.lends_asked.remove(&position);
            self.journal.lends_asked.insert(position);
        }
    }

    /// Whether the chunk at `position` is loaded for a lend asked of the region.
    pub(super) fn lend_ticketed(&self, position: ChunkPos) -> bool {
        self.lend_tickets.contains_key(&position)
    }

    /// The chunks loaded for lends asked, which a tick loads as it loads ticketed ones.
    pub(super) fn lend_ticket_chunks(&self) -> Vec<ChunkPos> {
        self.lend_tickets.keys().copied().collect()
    }

    /// The last thing of a tick about lends, before the region claims and gives back:
    /// the lends asked of it are answered, asks are asked again, and lent chunks
    /// that nothing needs any more are given back.
    pub(super) fn settle_lends(&mut self, output: &mut TickOutput) {
        let tick = self.tick;
        self.lend_tickets.retain(|_, until| *until >= tick);
        self.circuit_changes
            .retain(|_, changed| tick.saturating_sub(*changed) < CIRCUIT_CHANGES_FOR);
        for asked in std::mem::take(&mut self.lends_to_answer) {
            self.answer_lend(&asked, output);
        }
        // An ask that was told to wait, or was called off, is asked again in its time.
        let mut again = Vec::new();
        for (position, asked) in &self.lends_asked {
            if tick.saturating_sub(*asked) >= ASK_AGAIN_AFTER {
                again.push(*position);
            }
        }
        for position in again {
            self.lends_asked.insert(position, tick);
            self.journal.lends_asked.insert(position);
            output.lends.push(position);
        }
        output.lends.sort_unstable();
        output.lends.dedup();
        self.give_back_lent(output);
    }

    /// The lender's decision of one ask, chunk by chunk (section 3, step 2).
    fn answer_lend(&mut self, asked: &LendAsked, output: &mut TickOutput) {
        let asked_chunks: BTreeSet<ChunkPos> = asked.chunks.iter().copied().collect();
        let mut wait = Vec::new();
        let mut refused = Vec::new();
        let mut lending = Vec::new();
        for position in &asked.chunks {
            let position = *position;
            // Its own land beside it, which its parts could reach the chunk from.
            let beside: Vec<ChunkPos> = around(position)
                .filter(|near| !asked_chunks.contains(near))
                .filter(|near| self.land.holds(*near) || self.land.is_pinned(*near))
                .filter(|near| !self.land.lent_away.contains_key(near))
                .filter(|near| !self.land.lending.contains_key(near))
                .collect();
            // Never lent without being seen: what is not loaded is loaded and the lend
            // waits for it.
            let unseen: Vec<ChunkPos> = std::iter::once(position)
                .chain(beside.iter().copied())
                .filter(|near| !self.chunks.contains_key(near))
                .collect();
            if !unseen.is_empty() {
                for near in unseen {
                    self.lend_tickets.insert(near, self.tick + LEND_TICKET_FOR);
                }
                wait.push(position);
                continue;
            }
            if beside
                .iter()
                .any(|near| self.reaches_into(*near, &asked_chunks))
            {
                refused.push(position);
                continue;
            }
            if self.must_keep_for_now(position) {
                wait.push(position);
                continue;
            }
            lending.push(position);
        }
        for position in &lending {
            self.land.known.remove(position);
            self.land.lending.insert(*position, asked.lend);
            self.lend_tickets.remove(position);
            self.letting_go.insert(*position);
        }
        let lend = asked.lend;
        if !wait.is_empty() {
            output.lend_answers.push(LendAnswer::Refused {
                lend,
                chunks: wait,
                again: true,
            });
        }
        if !refused.is_empty() {
            output.lend_answers.push(LendAnswer::Refused {
                lend,
                chunks: refused,
                again: false,
            });
        }
        if !lending.is_empty() {
            output.lend_answers.push(LendAnswer::Lending {
                lend,
                chunks: lending,
            });
        }
    }

    /// Whether the region keeps the loaded chunk at `position` for now: a player of its
    /// own stands in it, it is the chunk players enter the world in, block events are
    /// kept in it or a piston moves there, or storage has not answered for it.
    fn must_keep_for_now(&self, position: ChunkPos) -> bool {
        if position == self.land.home || self.requested.contains(&position) {
            return true;
        }
        if self
            .players
            .values()
            .any(|player| player.chunk() == position)
        {
            return true;
        }
        let Some(chunk) = self.chunks.get(&position) else {
            return true;
        };
        let moving = |state: BlockState| {
            clustine_blocks::data::is_class(state, BlockClass::MovingPistonBlock)
        };
        !chunk.events().is_empty()
            || chunk
                .sections()
                .iter()
                .any(|section| section.may_hold(moving) && section.states().any(moving))
    }

    /// Gives back each lent chunk looked at in which no circuit's part stands and into
    /// which none of the region's own pinned land beside reaches, once all of that is
    /// loaded (section 4).
    fn give_back_lent(&mut self, output: &mut TickOutput) {
        let looked_at: Vec<ChunkPos> = self.lent_to_check.iter().copied().collect();
        for position in looked_at {
            if !self.land.lent.contains(&position) {
                self.lent_to_check.remove(&position);
                continue;
            }
            let beside: Vec<ChunkPos> = around(position)
                .filter(|near| {
                    (self.land.is_pinned(*near) || self.land.lent.contains(near))
                        && !self.land.lent_away.contains_key(near)
                })
                .collect();
            let all_loaded = std::iter::once(position)
                .chain(beside.iter().copied())
                .all(|near| self.chunks.contains_key(&near));
            if !all_loaded {
                // Looked at again once they are.
                continue;
            }
            self.lent_to_check.remove(&position);
            let into = BTreeSet::from([position]);
            let has_part = self.chunks.get(&position).is_some_and(|chunk| {
                chunk
                    .sections()
                    .iter()
                    .any(|section| section.may_hold(is_circuit_part))
            }) && self.reaches_into(position, &into);
            if has_part || beside.iter().any(|near| self.reaches_into(*near, &into)) {
                continue;
            }
            self.land.lent.remove(&position);
            self.land.known.remove(&position);
            self.letting_go.insert(position);
            output.returns.push(position);
        }
    }

    /// Whether the chunk at `position` is the region's by a lend.
    pub(super) fn is_lent(&self, position: ChunkPos) -> bool {
        self.land.lent.contains(&position)
    }

    /// Whether the chunk at `position` is the region's by a lend, or would be by a grant
    /// of the store's that the coming tick takes: a chunk the region keeps loaded from
    /// then, so that what acts on it can wait until it is there.
    pub fn borrows(&self, position: ChunkPos, granted: &[ChunkPos]) -> bool {
        self.is_lent(position)
            || (self.lends_asked.contains_key(&position) && granted.contains(&position))
    }

    /// Whether the chunk at `position` is let go of for a lend, and not the region's
    /// to claim, load or write until the store says otherwise.
    pub(super) fn is_lending(&self, position: ChunkPos) -> bool {
        self.land.lending.contains_key(&position)
    }

    /// Whether the region trusts that the region named holds `position` until it is
    /// pinned back, whatever it wants.
    pub(super) fn is_lent_away(&self, position: ChunkPos) -> bool {
        self.land.lent_away.contains_key(&position)
    }

    /// The loaded chunks the region let go of at the end of its last tick, for a lend, as
    /// a lent chunk given back or as a chunk whose arrival ticket ended (`arrivals.rs`),
    /// ascending: still loaded, so that they can be saved as
    /// they are, and dropped at the start of the next tick (section 3, step 2, and
    /// section 4).
    pub fn letting_go(&self) -> Vec<ChunkPos> {
        self.letting_go.iter().copied().collect()
    }

    /// Whether the region has let go of the chunk at `position` for a lend and the
    /// store has not said lent or called off: whoever asks for the chunk waits.
    pub fn lending_away(&self, position: ChunkPos) -> bool {
        self.is_lending(position)
    }

    /// A circuit's part changed in the chunk at `position` in this tick.
    pub(super) fn circuit_changed(&mut self, position: ChunkPos) {
        self.circuit_changes.insert(position, self.tick);
    }

    /// The loaded chunks in which a circuit's part changed in the last second, ascending:
    /// what the runner reports to the coordinator, which merges regions that follow
    /// players at once where a circuit works at their line (section 7).
    pub fn circuit_changes(&self) -> Vec<ChunkPos> {
        self.circuit_changes
            .iter()
            .filter(|(position, tick)| {
                self.tick.saturating_sub(**tick) < CIRCUIT_CHANGES_FOR
                    && self.chunks.contains_key(position)
            })
            .map(|(position, _)| *position)
            .collect()
    }
}
