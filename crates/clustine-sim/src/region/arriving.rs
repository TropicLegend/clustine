//! The travel's destination side (step P9 of `docs/groundwork/p-plan.md`, sections
//! 16.3, 16.4, 16.7 and 16.8): the wait of each traveller the region holds, and its
//! placing, as `PortalForcer` and `NetherPortalBlock.createDimensionTransition` place
//! one in the destination's level.
//!
//! - **The wait.** Each asks the store's portal index once, for the square the game
//!   searches about its target, and keeps the chunks it needs loaded: the 3 × 3 chunks
//!   about the target, which `createPortal`'s spiral reads, and those about the
//!   candidate it would choose. It is placed when the index has answered and those
//!   chunks are loaded or copied, or are another region's, which nothing waits for; or,
//!   for loading only, after [`LOADING_LIMIT`] ticks, a chunk still missing taken on the
//!   index's word or, for the spiral, as unsuitable. Nothing is placed without the
//!   index's answer.
//! - **The choice.** The index's portals and those the region made itself since it
//!   asked, each kept where its block is still a portal with an axis, or taken on the
//!   index's word where the region cannot read it; then the game's: the nearest, then
//!   the lowest. Two placed in one tick see each other's portal.
//! - **The portal made** where none is found, in the overworld or the Nether: its
//!   blocks set in the region's chunks with the game's flags, and filed as `SetIf`
//!   effects where another region holds the chunk.
//! - **The pose**: the destination's rectangle and the place the traveller had in the
//!   one it left, its yaw and velocity turned where the axes differ, and the nearest
//!   place free of blocks for anything of at most 4 × 4.
//!
//! Named differences: a portal the game cannot make (no room under the top of the
//! level) leaves the game's traveller where it was; here it has left its source, and is
//! put at the target, freed of blocks. A 21-wide portal reaching beyond the chunks
//! about its candidate is measured as far as the region reads it. A portal lit by a
//! player in this region after the question, before its commit is in an answer, is not
//! seen.

use clustine_blocks::data::is_block;
use clustine_blocks::portal::axis_of;
use clustine_blocks::portal_forcer::{
    self, DEFAULT_BORDER, FoundRectangle, NETHER_PORTAL_RADIUS, OVERWORLD_PORTAL_RADIUS,
};
use clustine_data::{BlockState, blocks};
use clustine_world::{
    BlockPos, ChunkPos, Dimension, Effect, EffectId, Effected, Entity, EntityChange, PlayerId,
    PortalAxis, Travel, Way,
};

use super::Region;
use super::entities::state_of;
use super::updates::{UPDATE_ALL, UPDATE_CLIENTS, UPDATE_KNOWN_SHAPE, UPDATE_LIMIT};
use crate::api::{Durable, PortalAnswer, PortalQuery, QueryId, RegionEvent, TickOutput};

/// How long a traveller waits for the chunks it needs before it is placed with what
/// the region has (the plan, section 16.3).
pub(super) const LOADING_LIMIT: u64 = 200;

/// How long a portal the region made is a candidate of its own, before the index's
/// answers have it.
const MADE_PORTAL_KEPT: u64 = 400;

/// How long the chunks about a placed traveller stay loaded: the game's `PORTAL`
/// ticket, 300 ticks (`Entity.placePortalTicket`).
const PORTAL_TICKET_FOR: u64 = 300;

/// A player's size standing, which the game places them by.
const PLAYER_SIZE: (f32, f32) = (0.6, 1.8);

/// `createPortalBlocks`' flags, as the portal's own blocks are set
/// (`PortalForcer.createPortal`): the clients are told, no neighbour's shape asked.
const PORTAL_FLAGS: u32 = UPDATE_CLIENTS | UPDATE_KNOWN_SHAPE;

/// Who waits: a player's stay or a traveller of the stream, by its number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Waiter {
    Stay(PlayerId),
    Traveller(EffectId),
}

/// The radius the game searches for a portal in, about the target: 16 into the
/// Nether, 128 out of it (p-brief 2.2, item 3).
fn search_radius(to: Dimension) -> i32 {
    if to == Dimension::Nether {
        NETHER_PORTAL_RADIUS
    } else {
        OVERWORLD_PORTAL_RADIUS
    }
}

/// The chunks about a block that a search or a rectangle there reads: the 3 × 3 about
/// its chunk, which reach at least 16 blocks from it.
fn chunks_about(at: BlockPos) -> Vec<ChunkPos> {
    let centre = at.chunk();
    let mut chunks = Vec::with_capacity(9);
    for dz in -1..=1 {
        for dx in -1..=1 {
            chunks.push(ChunkPos::new(centre.x + dx, centre.z + dz));
        }
    }
    chunks
}

impl Region {
    /// The waits of the tick, after the travellers and arrivals are taken: the store's
    /// answers taken, the questions asked, and each traveller that is ready placed, in
    /// the order they came.
    pub(super) fn tick_waits(&mut self, answers: &[PortalAnswer], output: &mut TickOutput) {
        let tick = self.tick;
        self.made_portals
            .retain(|(made, _)| tick.saturating_sub(*made) <= MADE_PORTAL_KEPT);
        for answer in answers {
            self.take_answer(answer);
        }
        if self.waiting.is_empty() && self.waiting_travellers.is_empty() {
            return;
        }
        self.ask(output);
        let mut order: Vec<(u64, Waiter)> = (self.waiting.iter())
            .map(|(id, waiting)| (waiting.since, Waiter::Stay(*id)))
            .chain(
                (self.waiting_travellers.iter())
                    .map(|waiting| (waiting.since, Waiter::Traveller(waiting.id))),
            )
            .collect();
        order.sort();
        for (since, waiter) in order {
            let Some((travel, answer)) = self.wait_of(waiter) else {
                continue;
            };
            let Some(answer) = answer else {
                continue;
            };
            let candidate = self.choose(&travel, &answer);
            let mut needed = chunks_about(travel.target);
            if let Some(candidate) = candidate {
                needed.extend(chunks_about(candidate));
            }
            let missing: Vec<ChunkPos> = (needed.into_iter())
                .filter(|chunk| !self.settled_for_travel(*chunk))
                .collect();
            if !missing.is_empty() && tick.saturating_sub(since) < LOADING_LIMIT {
                for chunk in missing {
                    self.ticket_for_travel(chunk);
                }
                continue;
            }
            match waiter {
                Waiter::Stay(id) => self.place_stay(id, &travel, candidate, output),
                Waiter::Traveller(id) => self.place_traveller(id, &travel, candidate, output),
            }
        }
    }

    /// The travel and the index's answer of a waiter, if it is waiting still.
    fn wait_of(&self, waiter: Waiter) -> Option<(Travel, Option<Vec<BlockPos>>)> {
        match waiter {
            Waiter::Stay(id) => {
                let waiting = self.waiting.get(&id)?;
                Some((waiting.transfer.travel?, waiting.answer.clone()))
            }
            Waiter::Traveller(id) => {
                let waiting = (self.waiting_travellers.iter()).find(|waiting| waiting.id == id)?;
                Some((waiting.traveller.travel, waiting.answer.clone()))
            }
        }
    }

    /// An answer of the store's index, to the question of a waiter: an answer to a
    /// question nobody here asked (one before a take-over) is passed over.
    fn take_answer(&mut self, answer: &PortalAnswer) {
        for (id, waiting) in &mut self.waiting {
            if waiting.asked == Some(answer.id) && waiting.answer.is_none() {
                waiting.answer = Some(answer.portals.clone());
                self.journal.waiting.insert(*id);
            }
        }
        for waiting in &mut self.waiting_travellers {
            if waiting.asked == Some(answer.id) && waiting.answer.is_none() {
                waiting.answer = Some(answer.portals.clone());
                self.journal.waiting_travellers = true;
            }
        }
    }

    /// Asks the index once for each waiter that has not asked: the square the game
    /// searches, about the target, in the region's own dimension.
    fn ask(&mut self, output: &mut TickOutput) {
        let tick = self.tick;
        let mut index = 0;
        let mut next = |travel: &Travel| {
            let id = QueryId { tick, index };
            index += 1;
            let query = PortalQuery {
                id,
                centre: travel.target,
                radius: search_radius(travel.to),
            };
            (id, query)
        };
        for (id, waiting) in &mut self.waiting {
            if waiting.asked.is_some() {
                continue;
            }
            let Some(travel) = waiting.transfer.travel else {
                continue;
            };
            let (asked, query) = next(&travel);
            waiting.asked = Some(asked);
            output.portal_queries.push(query);
            self.journal.waiting.insert(*id);
        }
        for waiting in &mut self.waiting_travellers {
            if waiting.asked.is_some() {
                continue;
            }
            let (asked, query) = next(&waiting.traveller.travel);
            waiting.asked = Some(asked);
            output.portal_queries.push(query);
            self.journal.waiting_travellers = true;
        }
    }

    /// Whether a chunk a traveller needs is as it will be: loaded or copied, or known
    /// to be another region's, which the region does not wait for.
    fn settled_for_travel(&self, chunk: ChunkPos) -> bool {
        self.readable_chunk(chunk).is_some() || self.land.knowledge(chunk).holder().is_some()
    }

    /// Keeps a chunk a traveller needs loaded, as an arrival's is, which claims it where
    /// it is nobody's.
    fn ticket_for_travel(&mut self, chunk: ChunkPos) {
        if self.is_lending(chunk) {
            return;
        }
        let until = self.tick + super::arrivals::ARRIVAL_TICKET_FOR;
        let ticket = self.arrival_tickets.entry(chunk).or_insert(until);
        *ticket = (*ticket).max(until);
    }

    /// `PortalForcer.findClosestPortalPosition` of what the region knows: the index's
    /// portals and those it made since, each kept where the region reads its block as a
    /// portal still, or on the index's word where it cannot read it.
    fn choose(&self, travel: &Travel, answer: &[BlockPos]) -> Option<BlockPos> {
        let made = self.made_portals.iter().map(|(_, at)| *at);
        let mut candidates: Vec<BlockPos> = answer.iter().copied().chain(made).collect();
        candidates.sort();
        candidates.dedup();
        candidates.retain(|at| match self.readable_chunk(at.chunk()) {
            Some(_) => self
                .block(*at)
                .is_some_and(|state| is_block(state, blocks::NETHER_PORTAL)),
            None => true,
        });
        portal_forcer::nearest_portal(
            &candidates,
            travel.target,
            search_radius(travel.to),
            DEFAULT_BORDER,
        )
    }

    /// The portal a traveller comes out of: the one chosen, measured, or else one made
    /// as the game makes it, its blocks set; with its axis. None where none can be made.
    fn exit_portal(
        &mut self,
        travel: &Travel,
        candidate: Option<BlockPos>,
        output: &mut TickOutput,
    ) -> Option<(FoundRectangle, PortalAxis)> {
        if let Some(found) = candidate {
            let axis = self
                .block(found)
                .filter(|state| is_block(*state, blocks::NETHER_PORTAL))
                .map_or(PortalAxis::X, axis_of);
            let read = |at: BlockPos| self.block(at);
            return Some((portal_forcer::largest_rectangle(found, axis, &read), axis));
        }
        let source_axis = match travel.way {
            Way::NetherPortal { axis, .. } => axis.unwrap_or(PortalAxis::X),
            _ => PortalAxis::X,
        };
        let rules = self.config.dimension.rules();
        let made = {
            let read = |at: BlockPos| self.block(at);
            let heights = |x: i32, z: i32| self.motion_blocking_height(x, z);
            portal_forcer::make_portal(
                travel.target,
                source_axis,
                &read,
                &heights,
                rules.min_y,
                rules.logical_height as i32,
                DEFAULT_BORDER,
            )
        };
        let (changes, rectangle, axis) = made?;
        for (at, state) in changes {
            let portal = is_block(state, blocks::NETHER_PORTAL);
            let flags = if portal { PORTAL_FLAGS } else { UPDATE_ALL };
            self.set_for_travel(at, state, flags, output);
            if portal {
                self.made_portals.push((self.tick, at));
            }
        }
        Some((rectangle, axis))
    }

    /// Sets a block of a portal made: in the region's own chunk with its updates, or as
    /// an effect for the region that holds the chunk, if the block is still what the
    /// region read in its copy.
    fn set_for_travel(
        &mut self,
        at: BlockPos,
        state: BlockState,
        flags: u32,
        output: &mut TickOutput,
    ) {
        if self.own_block(at).is_some() {
            self.set_block_updated(at, state, flags, UPDATE_LIMIT, output);
            return;
        }
        let Some(expected) = self.block(at) else {
            return;
        };
        if expected != state && self.sends_to(at.chunk()) {
            self.made.push(Effect {
                at,
                what: Effected::SetIf {
                    expected,
                    state,
                    flags,
                },
            });
        }
    }

    /// The column's `MOTION_BLOCKING` height where the region reads its chunk.
    fn motion_blocking_height(&self, x: i32, z: i32) -> Option<i32> {
        let chunk = self.readable_chunk(ChunkPos::containing(f64::from(x), f64::from(z)))?;
        let column = ((z & 15) << 4 | (x & 15)) as usize;
        Some(chunk.min_y() + i32::from(chunk.heights().motion_blocking[column]))
    }

    /// Where a traveller of `size` comes out, by its travel, with the velocity and yaw
    /// it had: the exit portal's place for it, freed of blocks; or, where no portal can
    /// be made, the target's bottom centre freed of blocks.
    fn exit_place(
        &mut self,
        travel: &Travel,
        candidate: Option<BlockPos>,
        size: (f32, f32),
        velocity: [f64; 3],
        yaw: f32,
        output: &mut TickOutput,
    ) -> ([f64; 3], [f64; 3], f32) {
        let (relative, source_axis) = match travel.way {
            Way::NetherPortal { axis, relative } => {
                (relative.map(|value| value.0), axis.unwrap_or(PortalAxis::X))
            }
            _ => ([0.5, 0.0, 0.0], PortalAxis::X),
        };
        let exit = self.exit_portal(travel, candidate, output);
        let (position, velocity, yaw) = match exit {
            Some((rectangle, axis)) => portal_forcer::exit_pose(
                &rectangle,
                axis,
                relative,
                source_axis,
                size,
                velocity,
                yaw,
            ),
            None => {
                let target = travel.target;
                let at = [
                    f64::from(target.x) + 0.5,
                    f64::from(target.y),
                    f64::from(target.z) + 0.5,
                ];
                (at, velocity, yaw)
            }
        };
        let read = |at: BlockPos| self.block(at);
        let position = portal_forcer::free_position(position, size, &read, DEFAULT_BORDER);
        // The game's portal ticket, about where the traveller comes out.
        let at = BlockPos::new(
            position[0].floor() as i32,
            position[1].floor() as i32,
            position[2].floor() as i32,
        );
        for chunk in chunks_about(at) {
            if !self.is_lending(chunk) {
                let until = self.tick + PORTAL_TICKET_FOR;
                let ticket = self.arrival_tickets.entry(chunk).or_insert(until);
                *ticket = (*ticket).max(until);
            }
        }
        (position, velocity, yaw)
    }

    /// Places a waiting player: into the world at the exit, their travel done, the
    /// inputs they kept applied in order. A player the edge had in the world is told
    /// `Placed`, on which it finishes the change it began at their departure; one it
    /// never had (a join whose place had a travel) enters as a join does, `Spawned`
    /// (the plan, section 16.7). Their effects are told again, as the game resends them.
    fn place_stay(
        &mut self,
        id: PlayerId,
        travel: &Travel,
        candidate: Option<BlockPos>,
        output: &mut TickOutput,
    ) {
        let Some(waiting) = self.waiting.get(&id) else {
            return;
        };
        let pose = waiting.transfer.pose;
        let (position, _, yaw) =
            self.exit_place(travel, candidate, PLAYER_SIZE, [0.0; 3], pose.yaw, output);
        let Some(waiting) = self.waiting.remove(&id) else {
            return;
        };
        self.journal.waiting.insert(id);
        let mut transfer = waiting.transfer;
        transfer.travel = None;
        transfer.pose.position.x = position[0];
        transfer.pose.position.y = position[1];
        transfer.pose.position.z = position[2];
        transfer.pose.yaw = yaw;
        let mut player = self.player_of(waiting.edge, &transfer);
        player.travel = None;
        // A stay that never had an input applied carries its join's attempt: its edge
        // was never told an entity for it, so it was never in the world. A stay a
        // portal sent on lost its attempt at the firing (`travel.rs`).
        match transfer.attempt {
            Some(attempt) => self.enter(id, player, attempt, output),
            None => {
                let placed = Durable::Placed {
                    player: id,
                    entity: player.entity_id,
                    dimension: self.config.dimension,
                    pose: player.pose,
                    flying: player.flying,
                };
                output
                    .events
                    .push(RegionEvent::EntitySpawned(player.entity_state(id)));
                output.events.extend(player.equipment_shown());
                let shows_flags = !player.effects.active.is_empty();
                self.players.insert(id, player);
                self.journal.players.insert(id);
                self.send(waiting.edge, placed, output);
                self.tell_effects(id, output);
                if shows_flags {
                    self.show_shared_flags(id, output);
                }
            }
        }
        for input in &waiting.kept {
            self.act(id, input, output);
        }
    }

    /// Places a waiting traveller of the stream: each of its unit put into the world at
    /// the exit, the riders on their seats, as an arrival is taken; or sent on whole to
    /// the region that holds the chunk where it comes out.
    fn place_traveller(
        &mut self,
        id: EffectId,
        travel: &Travel,
        candidate: Option<BlockPos>,
        output: &mut TickOutput,
    ) {
        let Some(index) = (self.waiting_travellers.iter()).position(|waiting| waiting.id == id)
        else {
            return;
        };
        let Some(root) = self.waiting_travellers[index].traveller.entities.first() else {
            self.waiting_travellers.remove(index);
            self.journal.waiting_travellers = true;
            return;
        };
        let size = self.entity_box(root).size();
        let size = (size[0] as f32, size[1] as f32);
        let (motion, yaw) = (root.motion, root.yaw);
        let (position, motion, yaw) = self.exit_place(travel, candidate, size, motion, yaw, output);
        let waiting = self.waiting_travellers.remove(index);
        self.journal.waiting_travellers = true;
        let chunk = ChunkPos::containing(position[0], position[2]);
        let mut entities = waiting.traveller.entities;
        if let Some(root) = entities.first_mut() {
            root.motion = motion;
            root.yaw = yaw;
        }
        for entity in &mut entities {
            entity.position = position;
        }
        if !self.chunks.contains_key(&chunk) {
            // Another region's, or not loaded: sent on as an entity made there is.
            for entity in entities {
                output.arrivals.push((chunk, entity));
            }
            return;
        }
        for entity in entities {
            self.put_travelled(chunk, entity, output);
        }
    }

    /// Puts an entity a portal sent here into the world in `chunk`, as an arrival is
    /// taken: its hand-over counted at the firing.
    fn put_travelled(&mut self, chunk: ChunkPos, entity: Entity, output: &mut TickOutput) {
        let key = entity.key;
        if self.entity_chunks.contains_key(&key) {
            return;
        }
        output
            .events
            .push(RegionEvent::EntitySpawned(state_of(&entity)));
        self.entity_chunks.insert(key, chunk);
        self.entity_memory.remove(&key);
        if entity.riding.leash.is_some() {
            self.note_leash_found(key);
        }
        let whole = EntityChange::Whole(entity);
        if let Some(loaded) = self.chunks.get_mut(&chunk) {
            loaded.apply_entity(&whole);
        }
        output.entity_changes.push((chunk, whole));
    }
}
