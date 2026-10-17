//! Redstone torches, repeaters, comparators, observers and target blocks, as the 26.3
//! jar's classes have them (step R4 of `docs/adr/0026-redstone.md`); what they read
//! of the blocks is `clustine_blocks::diode`'s, pure functions of the region's view.
//!
//! - **A torch** (`RedstoneTorchBlock`, `RedstoneWallTorchBlock`) told of a neighbour
//!   whose block's power disagrees with its light schedules a tick of 2, unless one is
//!   drawn to run in this tick; the tick puts it out or lights it, with every update,
//!   and as it is set, as it is placed and as it goes it tells the neighbours of its
//!   six neighbours (`notifyNeighbors`). **Burnout**: each time a torch is put out, its
//!   position and the region's tick go on a list, from whose front every torch's tick
//!   takes what is more than 60 ticks old; the eighth time within those 60 ticks it
//!   smokes and hisses (level event 1502, which the client plays) and schedules a tick
//!   in 160, and while the list holds eight of it, it is not lit again. The list is
//!   the region's in memory, as the game's is the level's (a `WeakHashMap` keyed by
//!   the level): not durable, and empty in a region restored, merged or split, which
//!   the record allows (section 12).
//! - **A diode** (`DiodeBlock`) told of a neighbour schedules its tick when what it
//!   should be differs from what it is, with the game's priorities
//!   (`checkTickOnNeighbor`): a repeater's −3 where the block in front is a diode
//!   that does not face back into it, −2 while it is powered, −1 otherwise, after its
//!   delay; a comparator's −1 or 0, two ticks on. A locked repeater does nothing. Its
//!   tick sets it with `UPDATE_CLIENTS` only, and it tells the block in front and that
//!   block's neighbours but itself as it is set, placed or taken away
//!   (`updateNeighborsInFront`). A diode that its block below can no longer hold goes
//!   and tells the neighbours of its neighbours.
//! - **A repeater** clicked takes its next delay (no sound, as in the jar); placed
//!   beside power, it schedules a tick of 1 (`setPlacedBy`).
//! - **A comparator** clicked changes its mode, heard by all but the player
//!   (`ComparatorBlock.useWithoutItem`), and works its output out again
//!   (`refreshOutputState`); its output lives in its block entity
//!   (`block_entities.rs`).
//! - **An observer** whose watched side changes starts a pulse of two ticks
//!   (`updateShape` in `clustine_blocks::diode`), powered and then unpowered, each
//!   time telling the block behind it and that block's neighbours.
//! - **A target block** gives the power an arrow gave it, to every side, and lets it
//!   fall back to 0 at its tick; without arrows, which wait for entities, it is never
//!   powered, and it conducts as any full block.

use clustine_blocks::data::{flag, is_class, relative, same_block, set, set_flag, tick_id};
use clustine_blocks::diode;
use clustine_data::{BlockClass, BlockState, Face, sounds};
use clustine_world::{BlockPos, PlayerId, Ticked};

use super::Region;
use super::redstone::play;
use super::updates::{RegionView, UPDATE_CLIENTS, UPDATE_KNOWN_SHAPE, UPDATE_LIMIT, fluid_block};
use crate::api::{RegionEvent, TickOutput};

/// `RedstoneTorchBlock.TOGGLE_DELAY`.
const TORCH_DELAY: i32 = 2;
/// `RedstoneTorchBlock.RECENT_TOGGLE_TIMER`: how long a torch's going out is kept.
const RECENT_TOGGLE_TIMER: u64 = 60;
/// `RedstoneTorchBlock.MAX_RECENT_TOGGLES`.
const MAX_RECENT_TOGGLES: usize = 8;
/// `RedstoneTorchBlock.RESTART_DELAY`: how long a burnt-out torch waits.
const RESTART_DELAY: i32 = 160;
/// `LevelEvent.REDSTONE_TORCH_BURNOUT`: smoke and a hiss.
pub(crate) const REDSTONE_TORCH_BURNOUT: i32 = 1502;
/// How long an observer's pulse lasts, and how long it waits to start one.
const OBSERVER_DELAY: i32 = 2;

/// The times torches went out (`RedstoneTorchBlock.RECENT_TOGGLES`): each torch's
/// position and the region's tick, oldest first.
pub(crate) type TorchToggles = Vec<(BlockPos, u64)>;

impl Region {
    /// `neighborChanged` of a torch or a diode at `pos`, whose state is `state`.
    /// Returns whether it is one.
    pub(super) fn diode_neighbour_changed(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) -> bool {
        if diode::is_torch(state) {
            // `RedstoneTorchBlock.neighborChanged`.
            let powered = diode::torch_powered(&mut RegionView(self), state, pos);
            let of = Ticked::Block(tick_id(state));
            if flag(state, "lit") == powered && !self.will_tick_this_tick(pos, of) {
                self.schedule_tick(pos, of, TORCH_DELAY, diode::NORMAL);
            }
            return true;
        }
        if !is_class(state, BlockClass::DiodeBlock) {
            return false;
        }
        if clustine_blocks::can_survive(state, pos, &mut RegionView(self)) {
            self.check_tick_on_neighbour(pos, state);
            return true;
        }
        // `DiodeBlock.neighborChanged` of a diode that cannot stay: it is removed
        // (`removeBlock`, with every update), and the neighbours of its neighbours are
        // told, in the order of `Direction.values()`.
        self.set_block(pos, fluid_block(state), output);
        for towards in Face::ALL {
            self.update_neighbours_at(relative(pos, towards), state, output);
        }
        true
    }

    /// `DiodeBlock.checkTickOnNeighbor` and `ComparatorBlock.checkTickOnNeighbor`.
    fn check_tick_on_neighbour(&mut self, pos: BlockPos, state: BlockState) {
        let of = Ticked::Block(tick_id(state));
        let mut view = RegionView(self);
        if diode::is_comparator(state) {
            if view.0.will_tick_this_tick(pos, of) {
                return;
            }
            let output = diode::comparator_output(&mut view, state, pos);
            let kept = view.0.comparator_output(pos);
            let on = diode::should_turn_on(&mut view, state, pos);
            if output != kept || flag(state, "powered") != on {
                let priority = diode::neighbour_tick_priority(&mut view, state, pos);
                self.schedule_tick(pos, of, diode::delay(state), priority);
            }
            return;
        }
        if diode::locked(&mut view, state, pos) {
            return;
        }
        let on = diode::should_turn_on(&mut view, state, pos);
        if flag(state, "powered") != on && !view.0.will_tick_this_tick(pos, of) {
            let priority = diode::neighbour_tick_priority(&mut view, state, pos);
            self.schedule_tick(pos, of, diode::delay(state), priority);
        }
    }

    /// A block's own `tick` for the blocks of this step. Returns whether `state` is one
    /// of them.
    pub(super) fn diode_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) -> bool {
        if diode::is_torch(state) {
            self.torch_tick(pos, state, output);
            return true;
        }
        if diode::is_comparator(state) {
            self.refresh_output_state(pos, state, output);
            return true;
        }
        if diode::is_repeater(state) {
            self.repeater_tick(pos, state, output);
            return true;
        }
        if is_class(state, BlockClass::ObserverBlock) {
            // `ObserverBlock.tick`: powered, and two ticks on unpowered, each telling
            // what is behind it of the state it had.
            let of = Ticked::Block(tick_id(state));
            if flag(state, "powered") {
                let unpowered = set_flag(state, "powered", false);
                self.set_block_updated(pos, unpowered, UPDATE_CLIENTS, UPDATE_LIMIT, output);
            } else {
                let powered = set_flag(state, "powered", true);
                self.set_block_updated(pos, powered, UPDATE_CLIENTS, UPDATE_LIMIT, output);
                self.schedule_tick(pos, of, OBSERVER_DELAY, diode::NORMAL);
            }
            self.update_neighbours_in_front(pos, state, output);
            return true;
        }
        if is_class(state, BlockClass::TargetBlock) {
            // `TargetBlock.tick`: an arrow's power falls back to 0.
            if clustine_blocks::data::number(state, "power").unwrap_or(0) != 0 {
                self.set_block(pos, set(state, "power", "0"), output);
            }
            return true;
        }
        false
    }

    /// `RedstoneTorchBlock.tick`.
    fn torch_tick(&mut self, pos: BlockPos, state: BlockState, output: &mut TickOutput) {
        let powered = diode::torch_powered(&mut RegionView(self), state, pos);
        let now = self.tick;
        while self
            .torch_toggles
            .first()
            .is_some_and(|(_, when)| now.saturating_sub(*when) > RECENT_TOGGLE_TIMER)
        {
            self.torch_toggles.remove(0);
        }
        if flag(state, "lit") {
            if !powered {
                return;
            }
            self.set_block(pos, set_flag(state, "lit", false), output);
            if self.toggled_too_frequently(pos, true) {
                output.events.push(RegionEvent::LevelEvent {
                    kind: REDSTONE_TORCH_BURNOUT,
                    position: pos,
                    data: 0,
                });
                // The block there now, whatever the updates made of it; a tick already
                // pending there, as one a torch clock schedules, keeps its place.
                if let Some(block) = self.own_block(pos).and_then(|now| now.block_id()) {
                    let of = Ticked::Block(block as u16);
                    self.schedule_tick(pos, of, RESTART_DELAY, diode::NORMAL);
                }
            }
        } else if !powered && !self.toggled_too_frequently(pos, false) {
            self.set_block(pos, set_flag(state, "lit", true), output);
        }
    }

    /// `RedstoneTorchBlock.isToggledTooFrequently`: with `add`, the torch at `pos`
    /// goes on the list first; then whether the list holds it eight times.
    fn toggled_too_frequently(&mut self, pos: BlockPos, add: bool) -> bool {
        if add {
            self.torch_toggles.push((pos, self.tick));
        }
        let times = self.torch_toggles.iter().filter(|(at, _)| *at == pos);
        times.count() >= MAX_RECENT_TOGGLES
    }

    /// The burnout list, for the tests.
    #[cfg(test)]
    pub(crate) fn torch_toggles(&self) -> &TorchToggles {
        &self.torch_toggles
    }

    /// `DiodeBlock.tick` of a repeater: a locked one stays as it is; an unpowered one
    /// is powered, and is let go again after its delay if its input has gone; a powered
    /// one whose input has gone is let go.
    fn repeater_tick(&mut self, pos: BlockPos, state: BlockState, output: &mut TickOutput) {
        let mut view = RegionView(self);
        if diode::locked(&mut view, state, pos) {
            return;
        }
        let on = diode::should_turn_on(&mut view, state, pos);
        let powered = flag(state, "powered");
        if powered && !on {
            let off = set_flag(state, "powered", false);
            self.set_block_updated(pos, off, UPDATE_CLIENTS, UPDATE_LIMIT, output);
        } else if !powered {
            let lit = set_flag(state, "powered", true);
            self.set_block_updated(pos, lit, UPDATE_CLIENTS, UPDATE_LIMIT, output);
            if !on {
                let of = Ticked::Block(tick_id(state));
                self.schedule_tick(pos, of, diode::delay(state), diode::VERY_HIGH);
            }
        }
    }

    /// `ComparatorBlock.refreshOutputState`: the output worked out again and kept in
    /// the block entity; where it changed, or in compare mode, the comparator is
    /// powered by whether it should be, set with `UPDATE_CLIENTS`, and tells the block
    /// in front.
    fn refresh_output_state(&mut self, pos: BlockPos, state: BlockState, output: &mut TickOutput) {
        let signal = diode::comparator_output(&mut RegionView(self), state, pos);
        let kept = self.set_comparator_output(pos, signal, output);
        if kept == signal && diode::subtracts(state) {
            return;
        }
        let on = diode::should_turn_on(&mut RegionView(self), state, pos);
        let powered = flag(state, "powered");
        if powered != on {
            let changed = set_flag(state, "powered", on);
            self.set_block_updated(pos, changed, UPDATE_CLIENTS, UPDATE_LIMIT, output);
        }
        self.update_neighbours_in_front(pos, state, output);
    }

    /// `DiodeBlock.updateNeighborsInFront` and `ObserverBlock.updateNeighborsInFront`:
    /// the block in front of a diode, behind an observer, is told that `state` changed,
    /// and then that block's neighbours but the diode or observer itself.
    fn update_neighbours_in_front(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        let front = diode::front(state);
        let at = relative(pos, front);
        self.neighbour_changed_at(at, front.opposite(), state, false, output);
        self.update_neighbours_at_except(at, state, front.opposite(), output);
    }

    /// `RedstoneTorchBlock.notifyNeighbors`: the neighbours of each of the torch's six
    /// neighbours, in the order of `Direction.values()`.
    fn notify_torch_neighbours(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        for towards in Face::ALL {
            self.update_neighbours_at(relative(pos, towards), state, output);
        }
    }

    /// `onPlace` of the blocks of this step, for `state` set at `pos` in place of
    /// `old`: a torch tells the neighbours of its neighbours and a diode the block in
    /// front, whenever their state changes; an observer or a target block placed
    /// powered with no tick of its own pending is set unpowered.
    pub(super) fn diode_on_place(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        old: BlockState,
        output: &mut TickOutput,
    ) {
        if diode::is_torch(state) {
            self.notify_torch_neighbours(pos, state, output);
            return;
        }
        if is_class(state, BlockClass::DiodeBlock) {
            self.update_neighbours_in_front(pos, state, output);
            return;
        }
        if same_block(state, old) {
            return;
        }
        let of = Ticked::Block(tick_id(state));
        let flags = UPDATE_CLIENTS | UPDATE_KNOWN_SHAPE;
        if is_class(state, BlockClass::ObserverBlock) {
            if flag(state, "powered") && !self.has_scheduled_tick(pos, of) {
                let unpowered = set_flag(state, "powered", false);
                self.set_block_updated(pos, unpowered, flags, UPDATE_LIMIT, output);
                self.update_neighbours_in_front(pos, unpowered, output);
            }
        } else if is_class(state, BlockClass::TargetBlock) {
            let power = clustine_blocks::data::number(state, "power").unwrap_or(0);
            if power > 0 && !self.has_scheduled_tick(pos, of) {
                self.set_block_updated(pos, set(state, "power", "0"), flags, UPDATE_LIMIT, output);
            }
        }
    }

    /// `affectNeighborsAfterRemoval` of the blocks of this step, for `old` gone from
    /// `pos`: an observer that was powered with its tick pending tells what is behind
    /// it, even when a piston moved it, as the jar's does; a torch tells the
    /// neighbours of its neighbours and a diode the block in front, unless a piston
    /// moved them.
    pub(super) fn diode_after_removal(
        &mut self,
        pos: BlockPos,
        old: BlockState,
        moved_by_piston: bool,
        output: &mut TickOutput,
    ) {
        if is_class(old, BlockClass::ObserverBlock) {
            let of = Ticked::Block(tick_id(old));
            if flag(old, "powered") && self.has_scheduled_tick(pos, of) {
                let unpowered = set_flag(old, "powered", false);
                self.update_neighbours_in_front(pos, unpowered, output);
            }
            return;
        }
        if moved_by_piston {
            return;
        }
        if diode::is_torch(old) {
            self.notify_torch_neighbours(pos, old, output);
        } else if is_class(old, BlockClass::DiodeBlock) {
            self.update_neighbours_in_front(pos, old, output);
        }
    }

    /// A click on a repeater or a comparator (`useWithoutItem`) by `player`. Returns
    /// whether `state` is one.
    pub(super) fn diode_used(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        player: PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        if diode::is_repeater(state) {
            // `RepeaterBlock.useWithoutItem`: the next delay, with every update and no
            // sound.
            let delay = clustine_blocks::data::number(state, "delay").unwrap_or(1);
            let next = delay % 4 + 1;
            self.set_block(pos, set(state, "delay", &next.to_string()), output);
            return true;
        }
        if diode::is_comparator(state) {
            // `ComparatorBlock.useWithoutItem`: the other mode, its click played by the
            // player's own client, then set with `UPDATE_CLIENTS` and its output
            // worked out again.
            let subtract = !diode::subtracts(state);
            let mode = if subtract { "subtract" } else { "compare" };
            let changed = set(state, "mode", mode);
            let pitch = if subtract { 0.55 } else { 0.5 };
            play(
                pos,
                sounds::BLOCK_COMPARATOR_CLICK,
                0.3,
                pitch,
                Some(player),
                output,
            );
            self.set_block_updated(pos, changed, UPDATE_CLIENTS, UPDATE_LIMIT, output);
            if self.own_block(pos).is_some_and(diode::is_comparator) {
                self.refresh_output_state(pos, changed, output);
            }
            return true;
        }
        false
    }

    /// `DiodeBlock.setPlacedBy`: a repeater or a comparator placed where it should be
    /// on schedules a tick of 1.
    pub(super) fn diode_placed_by_player(&mut self, pos: BlockPos, state: BlockState) {
        if !is_class(state, BlockClass::DiodeBlock) {
            return;
        }
        if diode::should_turn_on(&mut RegionView(self), state, pos) {
            let of = Ticked::Block(tick_id(state));
            self.schedule_tick(pos, of, 1, diode::NORMAL);
        }
    }
}
