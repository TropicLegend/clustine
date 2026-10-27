//! What a bell's ring does to the living near it (`BellBlockEntity.updateEntities`,
//! `makeRaidersGlow`, the resonance; `docs/groundwork/n10-dumps.md`, part B, correction 1;
//! `docs/groundwork/n10-plan.md`, decision 17). Part V3 fills it; the contract step N10c
//! put the calls where the ring's block event runs and where the bell's block entity
//! ticks, and gave the region the memory of each ringing bell.

use std::collections::BTreeMap;

use clustine_world::{BlockPos, EntityId};

use super::Region;
use crate::api::TickOutput;

/// What the region remembers of a bell that rang: the living it listed and when
/// (`nearbyEntities`, `lastRingTimestamp`). In memory only, as the game's is unsaved.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Ring {
    pub(crate) entities: Vec<EntityId>,
    pub(crate) listed_at: i64,
}

/// Every bell's [`Ring`], by its place.
pub(crate) type Rings = BTreeMap<BlockPos, Ring>;

impl Region {
    /// The ring's block event ran at the bell `pos` (`triggerEvent`'s `updateEntities`):
    /// the living in its box grown by 48 listed (again only after 60 ticks), and those
    /// within 32 of its centre given `heard_bell_time`.
    pub(super) fn bell_rang(&mut self, pos: BlockPos, output: &mut TickOutput) {
        let _ = (pos, output);
    }

    /// The bell's block entity at `pos` ticked: on its fifth tick of shaking a raider in
    /// its list within 32 starts the resonance; on the forty-fifth the raiders within 48
    /// glow.
    pub(super) fn bell_ticked(&mut self, pos: BlockPos, output: &mut TickOutput) {
        let _ = (pos, output);
    }
}
