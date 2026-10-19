//! A mob's mind and its controls: the goal and target selectors, each goal's state, the
//! move, look and jump controls, sensing, and the navigation and its path (`Mob.
//! serverAiStep`; the brief's sections 2 and 3). Part N7c of the plan fills the hooks
//! and the parts' fields while no world keeps them; the brain (N7j) comes beside.

use clustine_world::Entry;
use serde::{Deserialize, Serialize};

use super::part::{self, Part, PartError, body_ids, mind_ids};
use super::tick::MobTick;

/// The move, look and jump controls' wanted states (`MoveControl`, `LookControl`,
/// `JumpControl`): a part of the body, which copies carry. N7c's.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Controls {}

impl Part for Controls {
    const ID: u16 = body_ids::CONTROLS;
    const VERSION: u8 = 1;
}

/// The path a mob follows, whole as it was made (`PathNavigation.path`): a part of the
/// body, which copies carry. N7c's.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Path {}

impl Part for Path {
    const ID: u16 = body_ids::PATH;
    const VERSION: u8 = 1;
}

/// How far along its path a mob is, and the navigation's timers. N7c's.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Progress {}

impl Part for Progress {
    const ID: u16 = body_ids::PROGRESS;
    const VERSION: u8 = 1;
}

/// A selector's state: its running goals, by slot, and the flags each holds
/// (`GoalSelector`). N7c's.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Selector {}

/// The goal selector's.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct GoalSelector(pub(crate) Selector);

impl Part for GoalSelector {
    const ID: u16 = mind_ids::GOAL_SELECTOR;
    const VERSION: u8 = 1;
}

/// The target selector's.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct TargetSelector(pub(crate) Selector);

impl Part for TargetSelector {
    const ID: u16 = mind_ids::TARGET_SELECTOR;
    const VERSION: u8 = 1;
}

/// A mob's mind: the selectors, and the goals' own parts by slot, which their goals'
/// code reads and writes (the slot's part is `mind_ids::FIRST_GOAL` plus its index).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Mind {
    pub(crate) goals: GoalSelector,
    pub(crate) targets: TargetSelector,
    /// The goals' parts, ascending by id, each in its goal's encoding.
    pub(crate) slots: Vec<Entry>,
}

impl Mind {
    pub(crate) fn decode(entries: &[Entry]) -> Result<Self, PartError> {
        Ok(Self {
            goals: part::decode(entries)?,
            targets: part::decode(entries)?,
            slots: entries
                .iter()
                .filter(|entry| entry.id >= mind_ids::FIRST_GOAL)
                .cloned()
                .collect(),
        })
    }

    /// The mind's entries, ascending by id.
    pub(crate) fn encode(&self) -> Vec<Entry> {
        let mut entries = Vec::new();
        part::put(&mut entries, &self.goals);
        part::put(&mut entries, &self.targets);
        entries.extend(self.slots.iter().cloned());
        entries
    }
}

/// `Mob.serverAiStep`: sensing cleared, the target selector, the goal selector, the
/// navigation, the type's own step (`customServerAiStep`, [`super::types`]), then the
/// move, look and jump controls, in the game's order (the brief's 2.0.12). N7c's: for
/// now only the type's own step.
pub(crate) fn server_ai_step(tick: &mut MobTick<'_>) {
    tick.mob_type.custom_server_ai_step(tick);
}

/// The body's rotation after `aiStep` (`BodyRotationControl.clientTick`, which the game
/// runs on the server too). N7c's.
pub(crate) fn rotate_body(tick: &mut MobTick<'_>) {
    let _ = tick;
}
