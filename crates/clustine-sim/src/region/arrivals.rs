//! Entities crossing a line (`docs/adr/0028-entities.md`, section 5;
//! `docs/groundwork/n3-plan.md`, step N3d with its sections 5 and 6): an entity whose
//! move ends in a chunk the region does not have loaded is sent, the region's chunk
//! committing it `Gone` and the tick handing it on whole as an arrival, with the `hops`
//! it had; the arrivals the runner hands in are taken after the effects and before the
//! entities tick, with `hops` one higher; and a chunk an arrival waits for is loaded
//! with an arrival ticket.
//!
//! Where a move ends decides: in a chunk the region ticks the entity goes on there; in
//! one of its own loaded and not ticking it moves in and stands still, as the game's
//! entities do in a chunk that does not tick; in one of its own loaded and full the
//! guard of the physics holds it back (`View::may_hold`); anywhere else (another
//! region's, lent away or being lent, the region's own not loaded, asked for, unknown)
//! it is sent, and the store gives it to whoever holds the chunk. Named differences: a
//! falling block that lands in the tick it crosses lands a tick later, in the region it
//! went to; what the game keeps of an entity in memory only (the block it stands on, a
//! cobweb's hold) is worked out again where it is taken, as after a load.

use std::collections::BTreeSet;

use clustine_world::{Arrival, ChunkPos, Entity, EntityChange, RegionId};

use super::Region;
use super::entities::state_of;
use crate::api::{RegionEvent, TickOutput};

/// How long an arrival ticket keeps a chunk loaded after its last arrival is taken and
/// none waits for it (ADR-0028, section 5: "and then 100 ticks").
const ARRIVAL_TICKET_FOR: u64 = 100;

/// How long a crossing counts as entities working at its line (the plan, section 5,
/// G8, as circuits' changes count, ADR-0027 section 7).
const LINE_CROSSED_FOR: u64 = 600;

/// How far from where an entity was sent the chunks about it count as at the line.
const AT_THE_LINE: f64 = 4.0;

impl Region {
    /// Sends `entity`, as the tick left it, out of the chunk `from`: its chunk commits it
    /// `Gone`, the tick hands it on whole to the chunk it is in with the `hops` it had,
    /// the edges are told it is handed on where it is, and what the region kept of it
    /// beyond its record is dropped.
    pub(super) fn send_entity(&mut self, from: ChunkPos, entity: Entity, output: &mut TickOutput) {
        let key = entity.key;
        let gone = EntityChange::Gone(key);
        if let Some(chunk) = self.chunks.get_mut(&from) {
            chunk.apply_entity(&gone);
        }
        output.entity_changes.push((from, gone));
        self.entity_chunks.remove(&key);
        self.entity_memory.remove(&key);
        self.entity_draws.remove(&key);
        self.piston_deltas.remove(&key);
        output.events.push(RegionEvent::HandedOn {
            entity: key,
            chunk: from,
            hops: entity.hops,
            position: entity.position,
            motion: entity.motion,
        });
        self.crossed_at(&entity);
        output.arrivals.push((entity.chunk(), entity));
    }

    /// Hands on an entity that is made in a chunk the region does not have loaded and
    /// ticking (a throw there): the tick hands it on whole as it was made, and the edges
    /// hear of it from the region that takes it.
    pub(super) fn send_made(&mut self, entity: Entity, output: &mut TickOutput) {
        self.crossed_at(&entity);
        output.arrivals.push((entity.chunk(), entity));
    }

    /// Notes the chunks about where `entity` was sent, by the region it went to.
    fn crossed_at(&mut self, entity: &Entity) {
        let to = self.land.knowledge(entity.chunk()).holder();
        let [x, _, z] = entity.position;
        let low = ChunkPos::containing(x - AT_THE_LINE, z - AT_THE_LINE);
        let high = ChunkPos::containing(x + AT_THE_LINE, z + AT_THE_LINE);
        for chunk_x in low.x..=high.x {
            for chunk_z in low.z..=high.z {
                self.lines_crossed
                    .insert((ChunkPos::new(chunk_x, chunk_z), to), self.tick);
            }
        }
    }

    /// Takes the arrivals the runner hands in whose chunks are loaded, in their order,
    /// after the effects and before the entities' phase, so that each ticks in this tick
    /// where its chunk ticks: whole, with `hops` one higher, into its chunk however full
    /// (an arrival cannot be refused), and reported spawned before the tick's moves. One
    /// of a chunk not loaded is left for the runner to hand in again. One whose key the
    /// region has already is a malformed stream (the plan, section 6, item 9): the region
    /// keeps its own entity and counts the arrival taken, so that the stream heals and a
    /// restore does not stop at it again; the record says to log it, which the runner
    /// may do from what it handed in, as the sim does no logging.
    pub(super) fn take_arrivals(&mut self, arrivals: &[Arrival], output: &mut TickOutput) {
        for arrival in arrivals {
            let chunk = arrival.chunk;
            if !self.chunks.contains_key(&chunk) {
                continue;
            }
            output.arrivals_taken.push(arrival.id);
            if let Some(until) = self.arrival_tickets.get_mut(&chunk) {
                *until = self.tick + ARRIVAL_TICKET_FOR;
            }
            let key = arrival.entity.key;
            if self.entity_chunks.contains_key(&key) {
                continue;
            }
            let entity = Entity {
                hops: arrival.entity.hops.saturating_add(1),
                ..arrival.entity.clone()
            };
            output
                .events
                .push(RegionEvent::EntitySpawned(state_of(&entity)));
            self.entity_chunks.insert(key, chunk);
            self.entity_memory.remove(&key);
            let whole = EntityChange::Whole(entity);
            if let Some(loaded) = self.chunks.get_mut(&chunk) {
                loaded.apply_entity(&whole);
            }
            output.entity_changes.push((chunk, whole));
        }
        output.arrivals_taken.sort_unstable();
        output.arrivals_taken.dedup();
    }

    /// The chunks arrivals wait for, as the runner names them at the start of a tick:
    /// each is ticketed until a hundred ticks on, which every tick it is named again
    /// moves on, and so does each taking into it.
    pub(super) fn take_arrivals_waiting(&mut self, waiting: &[ChunkPos]) {
        for position in waiting {
            // No arrival ticket loads a chunk being lent (the record, section 5); the
            // runner leaves those out, and the region makes sure.
            if self.is_lending(*position) {
                continue;
            }
            self.arrival_tickets
                .insert(*position, self.tick + ARRIVAL_TICKET_FOR);
        }
    }

    /// Whether the chunk at `position` is loaded for an arrival.
    pub(super) fn arrival_ticketed(&self, position: ChunkPos) -> bool {
        self.arrival_tickets.contains_key(&position)
    }

    /// The chunks loaded for arrivals, which a tick loads as it loads ticketed ones.
    pub(super) fn arrival_ticket_chunks(&self) -> Vec<ChunkPos> {
        self.arrival_tickets.keys().copied().collect()
    }

    /// The last thing of a tick about arrivals, before the region claims and gives
    /// back: the tickets that end let their chunks go where nothing else keeps them
    /// loaded, saved as they are (through [`Region::letting_go`]), and a chunk that
    /// nothing else uses and that the region does not keep is given back at once, not
    /// after the time a chunk is otherwise kept unused, so that a chunk held for an
    /// arrival alone does not keep another region's players out of it (ADR-0028,
    /// section 5). The crossings older than they count for are forgotten.
    pub(super) fn settle_arrivals(&mut self, output: &mut TickOutput) {
        let tick = self.tick;
        self.lines_crossed
            .retain(|_, crossed| tick.saturating_sub(*crossed) < LINE_CROSSED_FOR);
        // A chunk let go of for a lend is loaded for nobody's arrival.
        let lending = &self.land.lending;
        self.arrival_tickets
            .retain(|position, _| !lending.contains_key(position));
        let ended: Vec<ChunkPos> = self
            .arrival_tickets
            .iter()
            .filter(|(_, until)| **until < tick)
            .map(|(position, _)| *position)
            .collect();
        if ended.is_empty() {
            return;
        }
        let standing: BTreeSet<ChunkPos> =
            self.players.values().map(|player| player.chunk()).collect();
        for position in ended {
            self.arrival_tickets.remove(&position);
            let loaded_otherwise = self.tickets.contains_key(&position)
                || position == self.land.home
                || self.is_lent(position)
                || self.lend_ticketed(position);
            if loaded_otherwise {
                continue;
            }
            if self.chunks.contains_key(&position) {
                self.letting_go.insert(position);
            }
            self.requested.remove(&position);
            if self.land.holds(position)
                && !self.land.keeps(position)
                && !standing.contains(&position)
            {
                self.land.known.remove(&position);
                output.returns.push(position);
            }
        }
    }

    /// Where entities were sent across a line in the last half a minute, for merging
    /// regions that follow players (the plan, section 5, G8): the chunks within four
    /// blocks of where each was sent, each with the region it went to where the region
    /// knows it, ascending. Reported by the worker beside circuits at work (step N3f).
    pub fn entities_at_line(&self) -> Vec<(ChunkPos, Option<RegionId>)> {
        self.lines_crossed.keys().copied().collect()
    }
}
