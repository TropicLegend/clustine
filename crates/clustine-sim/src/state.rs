//! What a region is, apart from its chunks, and how it changes from tick to tick.
//!
//! A region can be rebuilt from its chunks and a [`RegionState`]; the state after a tick
//! is the state before it with the tick's [`StateDelta`] applied. Both are serialisable,
//! so that a worker can keep them in the world store and another worker can carry on
//! with the region. See `docs/adr/0008-durable-regions-and-resuming.md`, section 1.

use std::collections::BTreeMap;

use clustine_world::{EdgeId, EntityId, EntityIds, PlayerId};
use serde::{Deserialize, Serialize};

use crate::api::{Durable, HOTBAR_SLOTS, ItemStack, Pose};

/// Everything a region knows apart from its chunks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegionState {
    /// The number of the last tick; 0 before the first.
    pub tick: u64,
    /// The region's block of entity ids, issued once and the region's for life.
    pub entity_ids: EntityIds,
    /// The next entity id to give out. Once it is past the block, joining players are
    /// refused.
    pub next_entity_id: EntityId,
    pub players: BTreeMap<PlayerId, PlayerState>,
    /// The edges the region knows.
    pub edges: BTreeMap<EdgeId, EdgeState>,
}

/// A player in the region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerState {
    pub entity_id: EntityId,
    pub name: String,
    pub pose: Pose,
    pub hotbar: [Option<ItemStack>; HOTBAR_SLOTS],
    /// The hotbar slot whose item the player holds.
    pub selected_slot: u8,
    /// The number of the last input that was applied.
    pub last_input: u64,
    /// The highest sequence number of the player's own actions on blocks of this region
    /// that has been handled, if any. Actions passed on to another region are not
    /// covered: they are handled when a [`Durable::RemoteDone`] says so.
    pub handled: Option<i32>,
    /// The edge the player belongs to: the one they joined or arrived through.
    pub edge: EdgeId,
}

/// What a region keeps for an edge.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EdgeState {
    /// Which start of the edge the region knows.
    pub start: u64,
    /// The number of the last message of that edge that was applied.
    pub applied: u64,
    /// The number of the last outbox entry made; the next one is numbered one higher.
    pub sent: u64,
    /// The entries that the edge has not confirmed yet, by number.
    pub outbox: BTreeMap<u64, Durable>,
}

impl RegionState {
    /// The state of a region that has never run, with `entity_ids` as its block.
    pub fn new(entity_ids: EntityIds) -> Self {
        Self {
            tick: 0,
            entity_ids,
            next_entity_id: entity_ids.first,
            players: BTreeMap::new(),
            edges: BTreeMap::new(),
        }
    }

    /// Makes this the state after the tick that `delta` came from, given that it is the
    /// state before that tick.
    pub fn apply(&mut self, delta: &StateDelta) {
        self.tick = delta.tick;
        if let Some(next) = delta.next_entity_id {
            self.next_entity_id = next;
        }
        for (id, player) in &delta.players {
            match player {
                Some(player) => {
                    self.players.insert(*id, player.clone());
                }
                None => {
                    self.players.remove(id);
                }
            }
        }
        for (id, change) in &delta.edges {
            let Some(change) = change else {
                self.edges.remove(id);
                continue;
            };
            let edge = self.edges.entry(*id).or_default();
            edge.start = change.start;
            edge.applied = change.applied;
            edge.sent = change.sent;
            if change.cleared {
                edge.outbox.clear();
            }
            edge.confirm(change.confirmed);
            edge.outbox.extend(
                change
                    .added
                    .iter()
                    .map(|(number, entry)| (*number, entry.clone())),
            );
        }
    }
}

impl EdgeState {
    /// Drops the outbox entries up to `number`. Returns whether there were any.
    pub(crate) fn confirm(&mut self, number: u64) -> bool {
        let kept = match number.checked_add(1) {
            Some(first_kept) => self.outbox.split_off(&first_kept),
            None => BTreeMap::new(),
        };
        let dropped = !self.outbox.is_empty();
        self.outbox = kept;
        dropped
    }
}

/// Everything that changed in a [`RegionState`] in one tick, such that
/// `state_before.apply(&delta) == state_after`. It names only what changed: the players
/// and edges that did, and of an outbox the entries added and dropped.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StateDelta {
    /// The number of the tick.
    pub tick: u64,
    /// The next entity id to give out, if it changed.
    pub next_entity_id: Option<EntityId>,
    /// Each player that changed, in the order of the players, as they are after the tick:
    /// `None` for one who is no longer in the region.
    pub players: Vec<(PlayerId, Option<PlayerState>)>,
    /// Each edge that changed, in the order of the edges: `None` for one the region has
    /// forgotten.
    pub edges: Vec<(EdgeId, Option<EdgeDelta>)>,
}

impl StateDelta {
    /// Whether nothing changed in the tick but its number.
    pub fn changes_only_the_tick(&self) -> bool {
        self.next_entity_id.is_none() && self.players.is_empty() && self.edges.is_empty()
    }
}

/// What changed of an edge the region knows after the tick. Applied in this order: the
/// numbers are set, the outbox is cleared if `cleared`, the entries up to `confirmed` are
/// dropped, and `added` is added.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeDelta {
    pub start: u64,
    pub applied: u64,
    pub sent: u64,
    /// Whether the outbox the edge had before the tick was dropped as a whole, as when
    /// the edge was reset, or forgotten and then started again.
    pub cleared: bool,
    /// The outbox entries up to this number were dropped; 0 if none were.
    pub confirmed: u64,
    /// The entries made in the tick, in the order of their numbers.
    pub added: Vec<(u64, Durable)>,
}
