//! The players a portal sent to this region who wait to be placed
//! (`docs/groundwork/p-plan.md`, sections 16.3, 16.7 and 16.8): kept apart from the
//! region's players, so that nothing that iterates over those sees them, and met by
//! every rule that ends or answers a stay as an entering stay is. A waiting stay has an
//! entity nobody here was shown; its edge holds its client between the dimensions until
//! the placing says where it is.

use super::*;
use clustine_world::{EffectId, Traveller};

use crate::state::{WaitingState, WaitingTraveller};

impl Region {
    /// Takes in a player a portal sent here, whose arrival the region has judged as it
    /// judges every arrival: they wait to be placed by their travel.
    pub(super) fn take_waiting_stay(
        &mut self,
        edge: EdgeId,
        id: PlayerId,
        transfer: &PlayerTransfer,
    ) {
        let waiting = WaitingState {
            transfer: transfer.clone(),
            edge,
            kept: Vec::new(),
            asked: None,
            answer: None,
            since: self.tick,
            until_asked: false,
            forwards: 0,
        };
        self.waiting.insert(id, waiting);
        self.journal.waiting.insert(id);
    }

    /// The entity and hand-overs of the stay the region holds waiting for `id`, if it
    /// holds one.
    pub(super) fn waiting_stay(&self, id: PlayerId) -> Option<(EntityId, u32)> {
        (self.waiting.get(&id)).map(|waiting| (waiting.transfer.entity_id, waiting.transfer.hops))
    }

    /// Drops a waiting stay without a word to anyone: a later copy of the same stay
    /// takes its place.
    pub(super) fn forget_waiting(&mut self, id: PlayerId) {
        if self.waiting.remove(&id).is_some() {
            self.journal.waiting.insert(id);
        }
    }

    /// Drops the waiting stay of a player, which a later stay replaces, and tells its
    /// edge so as [`Region::end_player`] does. Nobody was shown its entity here, and the
    /// region it came from reported it removed.
    pub(super) fn end_waiting(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some(waiting) = self.waiting.remove(&id) else {
            return;
        };
        self.journal.waiting.insert(id);
        let ended = Durable::Ended {
            player: id,
            entity: waiting.transfer.entity_id,
            attempt: waiting.transfer.attempt,
        };
        self.send(waiting.edge, ended, output);
    }

    /// A leave of `edge` that names this waiting stay, by its entity or by the attempt
    /// of the join behind it, ends it: the store keeps the last `Has`, which has the
    /// travel, and the player is placed by it at their next join.
    pub(super) fn leave_waiting(
        &mut self,
        edge: EdgeId,
        id: PlayerId,
        entity: Option<EntityId>,
        attempt: Option<u64>,
    ) {
        let ended = |waiting: &WaitingState| {
            waiting.edge == edge
                && match entity {
                    Some(entity) => waiting.transfer.entity_id == entity,
                    None => attempt.is_some() && waiting.transfer.attempt == attempt,
                }
        };
        if let Some(waiting) = self.waiting.remove(&id) {
            if ended(&waiting) {
                // Its place as it was left, with the travel, then the end of the stay
                // (`docs/groundwork/p-plan.md`, section 16.8, A1).
                let note = has_of(id, &waiting.transfer);
                self.waiting_left.push((id, note));
                self.gone
                    .push((id, waiting.transfer.entity_id, waiting.transfer.hops));
                self.journal.waiting.insert(id);
            } else {
                self.waiting.insert(id, waiting);
            }
        }
    }

    /// The waiting stays of an edge that is gone or reset are no more.
    pub(super) fn drop_waiting_of(&mut self, edge: EdgeId) {
        let ended: Vec<_> = (self.waiting.iter())
            .filter(|(_, waiting)| waiting.edge == edge)
            .map(|(player, _)| *player)
            .collect();
        for id in ended {
            if let Some(waiting) = self.waiting.remove(&id) {
                self.gone
                    .push((id, waiting.transfer.entity_id, waiting.transfer.hops));
                self.journal.waiting.insert(id);
            }
        }
    }

    /// An input for a player who is not in the world here: if it is of a waiting stay
    /// of this edge and new, it is counted applied, and kept to be applied at the
    /// placing where it needs no place (what the edge passes on during the change).
    /// Whether the input was of a waiting stay.
    pub(super) fn waiting_input(
        &mut self,
        edge: EdgeId,
        id: PlayerId,
        entity: EntityId,
        number: u64,
        input: &PlayerInput,
    ) -> bool {
        let Some(waiting) = self.waiting.get_mut(&id) else {
            return false;
        };
        if waiting.edge != edge || waiting.transfer.entity_id != entity {
            return true;
        }
        if number <= waiting.transfer.last_input {
            return true;
        }
        waiting.transfer.last_input = number;
        waiting.transfer.attempt = None;
        self.journal.waiting.insert(id);
        // A stay in the credits or dead takes only the ask for its respawn
        // (`respawning.rs`).
        if self.asked_input(id, input) {
            return true;
        }
        let Some(waiting) = self.waiting.get_mut(&id) else {
            return true;
        };
        if input.has_no_place() {
            waiting.kept.push(input.clone());
        }
        self.journal.waiting.insert(id);
        true
    }

    /// The `Has` of each waiting stay that began or changed in this tick, or of every
    /// one in the region's first tick: in the travel's dimension, at its target, with
    /// the travel and no face of a body anyone saw (the plan, section 16.8).
    pub(super) fn waiting_notes(&self, first: bool) -> Vec<(PlayerId, StayNote)> {
        (self.waiting.iter())
            .filter(|(id, _)| first || self.journal.waiting.contains(id))
            .map(|(id, waiting)| (*id, has_of(*id, &waiting.transfer)))
            .chain(self.waiting_left.iter().cloned())
            .collect()
    }

    /// The chunks the waiting stays and travellers are at, which the region holds for
    /// them as it does a chunk a player stands in: it gives back no chunk one of them
    /// waits in. A stay in the credits or dead waits at its respawn's target, perhaps in
    /// another dimension, which is no chunk of this region's (`respawning.rs`).
    pub(super) fn waiting_chunks(&self) -> Vec<ChunkPos> {
        let dimension = self.config.dimension;
        let stays = (self.waiting.values())
            .filter(move |waiting| waiting.transfer.dimension == dimension)
            .map(|waiting| {
                let position = waiting.transfer.pose.position;
                ChunkPos::containing(position.x, position.z)
            });
        let travellers =
            (self.waiting_travellers.iter()).map(|waiting| waiting.traveller.chunk.position);
        stays.chain(travellers).collect()
    }

    /// Takes the travellers the stream hands in, each in the tick it comes: from this
    /// tick's commit on it is the region's state and the store files it no more. One the
    /// region holds waiting already, which the stream can hand in again before the
    /// commit that took it is durable, is counted taken and passed over
    /// (`docs/groundwork/p-plan.md`, sections 16.7 and 16.8).
    pub(super) fn take_travellers(
        &mut self,
        travellers: &[(EffectId, Traveller)],
        output: &mut TickOutput,
    ) {
        for (id, traveller) in travellers {
            output.arrivals_taken.push(*id);
            if self.waiting_travellers.iter().any(|held| held.id == *id) {
                continue;
            }
            self.waiting_travellers.push(WaitingTraveller {
                id: *id,
                traveller: traveller.clone(),
                asked: None,
                answer: None,
                since: self.tick,
            });
            self.journal.waiting_travellers = true;
        }
    }
}

/// The waiting stays as a restore takes them: the index's answers are not durable, and
/// a store asked before the restore answers nobody now, so a stay without its answer
/// asks again (`docs/groundwork/p-plan.md`, section 15.4).
pub(super) fn asking_again(
    mut waiting: BTreeMap<PlayerId, WaitingState>,
) -> BTreeMap<PlayerId, WaitingState> {
    for stay in waiting.values_mut() {
        if stay.answer.is_none() {
            stay.asked = None;
        }
    }
    waiting
}

/// The waiting travellers as a restore takes them, as [`asking_again`].
pub(super) fn travellers_asking_again(
    mut travellers: Vec<WaitingTraveller>,
) -> Vec<WaitingTraveller> {
    for traveller in &mut travellers {
        if traveller.answer.is_none() {
            traveller.asked = None;
        }
    }
    travellers
}

/// The `Has` of a waiting stay: in the travel's dimension, at its target, with the
/// travel and no face of a body anyone saw.
fn has_of(id: PlayerId, transfer: &PlayerTransfer) -> StayNote {
    let place = waiting_place(transfer);
    StayNote::Has {
        unit: None,
        player: id,
        entity: transfer.entity_id,
        hops: transfer.hops,
        face: PlayerFace::standing(&place),
        place,
        let_go: false,
    }
}

/// Where a waiting stay is, as the world store keeps it: the travel's target in its
/// dimension, with the travel.
pub(super) fn waiting_place(transfer: &PlayerTransfer) -> Place {
    Place {
        health: transfer.health,
        dead: transfer.dead,
        pose: transfer.pose,
        flying: transfer.flying,
        inventory: transfer.inventory.clone(),
        selected_slot: transfer.selected_slot,
        ender: transfer.ender.clone(),
        experience: transfer.experience,
        respawn: transfer.respawn,
        effects: transfer.effects.clone(),
        time_since_rest: transfer.time_since_rest,
        raid_omen_position: transfer.raid_omen_position,
        dimension: transfer.dimension,
        travel: transfer.travel,
        seen_credits: transfer.seen_credits,
        portal_cooldown: transfer.portal.cooldown,
    }
}
