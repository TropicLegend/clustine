//! `Goal`, `WrappedGoal` and `GoalSelector` (the brief's 2.2): goals tried in the order
//! they were added, priorities deciding only who may take a flag from whom, the full
//! selection in the ticks `Mob.serverAiStep` asks for it and the running goals' ticks
//! between.
//!
//! A type adds its goals in its `registerGoals` order ([`super::Setup`]); each goal's
//! state is the part of its slot (`mind_ids::FIRST_GOAL` plus the slot's index, its
//! target goals numbered after its goals), read before the selectors run and written
//! after, and the selector's running set and locks are its own part. A goal's code is
//! its type's, its state the record's, so that a goal added later starts empty and one
//! no longer added is dropped.

// Until a type registers goals (N7e on), only the tests make them.
#![allow(dead_code, reason = "the goals the types of N7e on register")]

use clustine_world::Entry;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{Ai, Selector};

/// The flags a goal holds (`Goal.Flag`), a bit each by ordinal.
pub(crate) mod flags {
    pub(crate) const MOVE: u8 = 1;
    pub(crate) const LOOK: u8 = 1 << 1;
    pub(crate) const JUMP: u8 = 1 << 2;
    pub(crate) const TARGET: u8 = 1 << 3;
}

/// A goal: its code, with the arguments its type built it with; its state is
/// [`Goal::State`], which the record keeps.
pub(crate) trait Goal: 'static {
    /// What it keeps between ticks: the fields the game's goal changes.
    type State: Serialize + DeserializeOwned + Default + PartialEq;
    /// The version of [`Goal::State`]'s encoding.
    const VERSION: u8 = 1;

    /// The flags it takes.
    fn flags(&self) -> u8;

    /// What building it does to its mob (`FloatGoal`'s constructor lets the navigation
    /// float): nothing for most.
    fn on_register(&self, config: &mut super::Config) {
        let _ = config;
    }

    /// `requiresUpdateEveryTick`.
    fn every_tick(&self) -> bool {
        false
    }

    /// `isInterruptable`.
    fn interruptable(&self, state: &Self::State) -> bool {
        let _ = state;
        true
    }

    /// Whether it is a `PanicGoal`, which `PathfinderMob.isPanicking` looks for.
    fn panics(&self) -> bool {
        false
    }

    fn can_use(&self, state: &mut Self::State, ai: &mut Ai<'_, '_>) -> bool;

    /// `canContinueToUse`, by default `canUse`.
    fn can_continue(&self, state: &mut Self::State, ai: &mut Ai<'_, '_>) -> bool {
        self.can_use(state, ai)
    }

    fn start(&self, state: &mut Self::State, ai: &mut Ai<'_, '_>) {
        let _ = (state, ai);
    }

    fn stop(&self, state: &mut Self::State, ai: &mut Ai<'_, '_>) {
        let _ = (state, ai);
    }

    fn tick(&self, state: &mut Self::State, ai: &mut Ai<'_, '_>) {
        let _ = (state, ai);
    }

    /// `adjustedTickDelay(ticks)`: as given for a goal ticked every tick, else the
    /// ceiling of half, the goal being ticked every other tick.
    fn adjusted_tick_delay(&self, ticks: i32) -> i32 {
        if self.every_tick() {
            ticks
        } else {
            super::maths::reduced_tick_delay(ticks)
        }
    }
}

/// A goal with its state, as the selector holds it.
pub(crate) trait Slot {
    fn flags(&self) -> u8;
    fn every_tick(&self) -> bool;
    fn interruptable(&self) -> bool;
    fn panics(&self) -> bool;
    fn can_use(&mut self, ai: &mut Ai<'_, '_>) -> bool;
    fn can_continue(&mut self, ai: &mut Ai<'_, '_>) -> bool;
    fn start(&mut self, ai: &mut Ai<'_, '_>);
    fn stop(&mut self, ai: &mut Ai<'_, '_>);
    fn tick(&mut self, ai: &mut Ai<'_, '_>);
    /// Its state from the record's entry, fresh where there is none or it cannot be
    /// read (a goal whose encoding changed starts again, as the game's goals start at
    /// every load).
    fn load(&mut self, entry: Option<&Entry>);
    /// Its entry under `id`, or none where its state is as a fresh one is.
    fn save(&self, id: u16) -> Option<Entry>;
}

/// A goal and its state.
pub(crate) struct Held<G: Goal> {
    pub(crate) goal: G,
    pub(crate) state: G::State,
}

impl<G: Goal> Slot for Held<G> {
    fn flags(&self) -> u8 {
        self.goal.flags()
    }

    fn every_tick(&self) -> bool {
        self.goal.every_tick()
    }

    fn interruptable(&self) -> bool {
        self.goal.interruptable(&self.state)
    }

    fn panics(&self) -> bool {
        self.goal.panics()
    }

    fn can_use(&mut self, ai: &mut Ai<'_, '_>) -> bool {
        self.goal.can_use(&mut self.state, ai)
    }

    fn can_continue(&mut self, ai: &mut Ai<'_, '_>) -> bool {
        self.goal.can_continue(&mut self.state, ai)
    }

    fn start(&mut self, ai: &mut Ai<'_, '_>) {
        self.goal.start(&mut self.state, ai);
    }

    fn stop(&mut self, ai: &mut Ai<'_, '_>) {
        self.goal.stop(&mut self.state, ai);
    }

    fn tick(&mut self, ai: &mut Ai<'_, '_>) {
        self.goal.tick(&mut self.state, ai);
    }

    fn load(&mut self, entry: Option<&Entry>) {
        self.state = entry
            .filter(|entry| entry.version == G::VERSION)
            .and_then(|entry| postcard::from_bytes(&entry.bytes).ok())
            .unwrap_or_default();
    }

    fn save(&self, id: u16) -> Option<Entry> {
        if self.state == G::State::default() {
            return None;
        }
        Some(Entry {
            id,
            version: G::VERSION,
            bytes: postcard::to_allocvec(&self.state)
                .expect("a goal's state is encoded into memory"),
        })
    }
}

/// A goal as its type added it: its priority and the goal with its state.
pub(crate) struct Registered {
    pub(crate) priority: i32,
    pub(crate) slot: Box<dyn Slot>,
}

/// One of a mob's two selectors in its tick: its goals in the order added and its
/// state.
pub(crate) struct Goals {
    pub(crate) slots: Vec<Registered>,
    pub(crate) state: Selector,
}

/// `GoalSelector.NO_GOAL`'s priority: a flag nobody holds is taken by any goal of a
/// lower number.
const NO_GOAL: i32 = i32::MAX;

/// The flags by ordinal.
const ORDINALS: [u8; 4] = [flags::MOVE, flags::LOOK, flags::JUMP, flags::TARGET];

impl Goals {
    fn running(&self, index: usize) -> bool {
        self.state.running.binary_search(&(index as u16)).is_ok()
    }

    fn set_running(&mut self, index: usize, running: bool) {
        let key = index as u16;
        match (self.state.running.binary_search(&key), running) {
            (Err(at), true) => self.state.running.insert(at, key),
            (Ok(at), false) => {
                self.state.running.remove(at);
            }
            _ => {}
        }
    }

    /// `WrappedGoal.start`: only a goal not running.
    fn start(&mut self, index: usize, ai: &mut Ai<'_, '_>) {
        if self.running(index) {
            return;
        }
        self.set_running(index, true);
        self.slots[index].slot.start(ai);
    }

    /// `WrappedGoal.stop`: only a goal running.
    fn stop(&mut self, index: usize, ai: &mut Ai<'_, '_>) {
        if !self.running(index) {
            return;
        }
        self.set_running(index, false);
        self.slots[index].slot.stop(ai);
    }

    /// `lockedFlags.getOrDefault(flag, NO_GOAL).canBeReplacedBy(goal)`.
    fn may_take(&self, flag: usize, index: usize) -> bool {
        let priority = self.slots[index].priority;
        match self.state.locks[flag] {
            None => priority < NO_GOAL,
            Some(holder) => {
                let holder = &self.slots[usize::from(holder)];
                holder.slot.interruptable() && priority < holder.priority
            }
        }
    }

    /// `GoalSelector.tick()`: the clean-up, the update, and every running goal's tick.
    pub(crate) fn tick(&mut self, ai: &mut Ai<'_, '_>) {
        let disabled = self.state.disabled;
        for index in 0..self.slots.len() {
            if self.running(index)
                && (self.slots[index].slot.flags() & disabled != 0
                    || !self.slots[index].slot.can_continue(ai))
            {
                self.stop(index, ai);
            }
        }
        for flag in 0..ORDINALS.len() {
            if let Some(holder) = self.state.locks[flag]
                && !self.running(usize::from(holder))
            {
                self.state.locks[flag] = None;
            }
        }
        for index in 0..self.slots.len() {
            if self.running(index) {
                continue;
            }
            let goal_flags = self.slots[index].slot.flags();
            if goal_flags & disabled != 0 {
                continue;
            }
            let free = (0..ORDINALS.len())
                .filter(|&flag| goal_flags & ORDINALS[flag] != 0)
                .all(|flag| self.may_take(flag, index));
            if !free {
                continue;
            }
            if !self.slots[index].slot.can_use(ai) {
                continue;
            }
            for (flag, bit) in ORDINALS.into_iter().enumerate() {
                if goal_flags & bit == 0 {
                    continue;
                }
                if let Some(holder) = self.state.locks[flag] {
                    self.stop(usize::from(holder), ai);
                }
                self.state.locks[flag] = Some(index as u16);
            }
            self.start(index, ai);
        }
        self.tick_running(true, ai);
    }

    /// `tickRunningGoals(force)`: in a half tick only those that tick every tick.
    pub(crate) fn tick_running(&mut self, force: bool, ai: &mut Ai<'_, '_>) {
        for index in 0..self.slots.len() {
            if self.running(index) && (force || self.slots[index].slot.every_tick()) {
                self.slots[index].slot.tick(ai);
            }
        }
    }

    /// Whether a running goal is a panic (`PathfinderMob.isPanicking` without a brain).
    pub(crate) fn panicking(&self) -> bool {
        (0..self.slots.len()).any(|index| self.running(index) && self.slots[index].slot.panics())
    }

    /// Each goal's state from the record's slots, `first` being its first goal's slot.
    pub(crate) fn load(&mut self, entries: &[Entry], first: u16) {
        for (index, registered) in self.slots.iter_mut().enumerate() {
            let id = first + index as u16;
            let entry = entries
                .binary_search_by_key(&id, |entry| entry.id)
                .ok()
                .map(|at| &entries[at]);
            registered.slot.load(entry);
        }
        // A goal no longer added runs no longer.
        let count = self.slots.len() as u16;
        self.state.running.retain(|index| *index < count);
        for lock in &mut self.state.locks {
            if lock.is_some_and(|holder| holder >= count) {
                *lock = None;
            }
        }
    }

    /// The goals' entries, ascending, `first` being its first goal's slot.
    pub(crate) fn save(&self, first: u16, entries: &mut Vec<Entry>) {
        for (index, registered) in self.slots.iter().enumerate() {
            entries.extend(registered.slot.save(first + index as u16));
        }
    }
}
