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

use std::time::Instant;

use super::Coordinator;

/// What a coordinator that decides by itself keeps besides what it keeps of each
/// region.
#[derive(Debug, Clone, Default)]
pub(super) struct Noted {
    /// When a reading of the list was last answered, with a list or without.
    pub(super) answered: Option<Instant>,
}

impl Coordinator {
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
}
