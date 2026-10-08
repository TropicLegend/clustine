//! The state of a region and its tick.

use std::collections::{BTreeMap, BTreeSet};
use std::mem;

use clustine_data::{BlockState, ITEMS, blocks};
use clustine_world::{
    BlockPos, Chunk, ChunkArea, ChunkPos, EdgeId, EntityId, EntityIds, PlayerId, Vec3,
};

use crate::api::{
    Durable, EdgeEvent, EntityKind, EntityState, HOTBAR_SLOTS, ItemStack, PlayerChange,
    PlayerEvent, PlayerInput, PlayerTransfer, Pose, RegionEvent, RemoteAction, RemoteStep,
    TickInputs, TickOutput,
};
use crate::state::{EdgeDelta, EdgeState, PlayerState, RegionState, StateDelta};

/// What a region is created with, and restored with: what it is given rather than what
/// it has come to know. Its block of entity ids is part of its [`RegionState`].
#[derive(Debug, Clone, PartialEq)]
pub struct RegionConfig {
    /// Where players enter the world.
    pub spawn: Vec3,
    /// The part of the world the region simulates. Chunks outside it are never loaded,
    /// and a player who steps out of it is let go; see [`Durable::Departed`].
    pub area: ChunkArea,
    /// What players have in their hotbar when they enter the world.
    pub starting_hotbar: [Option<ItemStack>; HOTBAR_SLOTS],
}

/// How far above a player's feet their eyes are.
const EYE_HEIGHT: f64 = 1.62;

/// How far from their eyes a creative-mode player can reach a block, with the block of
/// tolerance vanilla allows for the client being slightly ahead of the server.
const BLOCK_REACH: f64 = 6.0;

/// The width and the height of a standing player.
const PLAYER_WIDTH: f64 = 0.6;
const PLAYER_HEIGHT: f64 = 1.8;

/// Positions beyond these are pulled back, as in vanilla.
const MAX_HORIZONTAL_COORDINATE: f64 = 3.0e7;
const MAX_VERTICAL_COORDINATE: f64 = 2.0e7;

#[derive(Debug, Clone, PartialEq)]
struct Player {
    entity_id: EntityId,
    name: String,
    pose: Pose,
    /// The chunk the player was in at the end of the previous tick, if the pose has
    /// changed since then. Always `None` between ticks.
    moved_from: Option<ChunkPos>,
    /// The highest sequence number handled in this tick, to be acknowledged. Always
    /// `None` between ticks.
    handled_sequence: Option<i32>,
    hotbar: [Option<ItemStack>; HOTBAR_SLOTS],
    /// The hotbar slot whose item the player holds.
    selected_slot: u8,
    /// The number of the last input that was applied.
    last_input: u64,
    /// The highest sequence number handled in any tick; see [`PlayerState::handled`].
    handled: Option<i32>,
    /// The edge the player belongs to.
    edge: EdgeId,
}

/// What changed of an edge's outbox within the tick, beyond what the edge's state after
/// it shows.
#[derive(Debug, Clone, PartialEq, Default)]
struct EdgeJournal {
    cleared: bool,
    confirmed: u64,
    added: Vec<(u64, Durable)>,
}

/// What has changed in the current tick, from which its [`StateDelta`] is made. Empty
/// between ticks.
#[derive(Debug, Clone, PartialEq, Default)]
struct Journal {
    next_entity_id: bool,
    players: BTreeSet<PlayerId>,
    edges: BTreeMap<EdgeId, EdgeJournal>,
}

/// A part of the world that is simulated as one unit.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    config: RegionConfig,
    tick: u64,
    entity_ids: EntityIds,
    next_entity_id: EntityId,
    chunks: BTreeMap<ChunkPos, Chunk>,
    /// How many tickets each needed chunk has.
    tickets: BTreeMap<ChunkPos, u32>,
    /// Needed chunks that storage has been asked for but has not delivered.
    requested: BTreeSet<ChunkPos>,
    players: BTreeMap<PlayerId, Player>,
    edges: BTreeMap<EdgeId, EdgeState>,
    journal: Journal,
}

impl Region {
    /// A region that has never run, which gives its players the entity ids of
    /// `entity_ids`. The same as restoring [`RegionState::new`].
    pub fn new(config: RegionConfig, entity_ids: EntityIds) -> Self {
        Self::restore(config, RegionState::new(entity_ids))
    }

    /// The region whose state is `state`, without any chunk loaded or needed. Chunks come
    /// in as for any region: through tickets and what storage delivers. Ticks go on from
    /// [`RegionState::tick`].
    pub fn restore(config: RegionConfig, state: RegionState) -> Self {
        let players = state
            .players
            .into_iter()
            .map(|(id, player)| (id, Player::from_state(player)))
            .collect();
        Self {
            config,
            tick: state.tick,
            entity_ids: state.entity_ids,
            next_entity_id: state.next_entity_id,
            chunks: BTreeMap::new(),
            tickets: BTreeMap::new(),
            requested: BTreeSet::new(),
            players,
            edges: state.edges,
            journal: Journal::default(),
        }
    }

    /// Everything the region knows apart from its chunks, as of the last tick.
    pub fn state(&self) -> RegionState {
        RegionState {
            tick: self.tick,
            entity_ids: self.entity_ids,
            next_entity_id: self.next_entity_id,
            players: self
                .players
                .iter()
                .map(|(id, player)| (*id, player.to_state()))
                .collect(),
            edges: self.edges.clone(),
        }
    }

    /// The part of the world the region simulates.
    pub fn area(&self) -> ChunkArea {
        self.config.area
    }

    /// The number of ticks the region has run.
    pub fn tick_number(&self) -> u64 {
        self.tick
    }

    /// The chunk at `position`, if it is loaded.
    pub fn chunk(&self, position: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&position)
    }

    /// The number of loaded chunks.
    pub fn loaded_chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// The number of players in the region.
    pub fn player_count(&self) -> usize {
        self.players.len()
    }

    /// The entity id and pose of `player`, if they are in the region.
    pub fn player(&self, player: PlayerId) -> Option<(EntityId, Pose)> {
        let player = self.players.get(&player)?;
        Some((player.entity_id, player.pose))
    }

    /// Everything the region knows of `player`, if they are in the region: among it the
    /// edge they belong to, which is whom the region tells what concerns them.
    pub fn player_state(&self, player: PlayerId) -> Option<PlayerState> {
        self.players.get(&player).map(Player::to_state)
    }

    /// What the region keeps for `edge`, if it knows the edge.
    pub fn edge(&self, edge: EdgeId) -> Option<&EdgeState> {
        self.edges.get(&edge)
    }

    /// All entities, in a fixed order.
    pub fn entities(&self) -> impl Iterator<Item = EntityState> + '_ {
        self.players
            .iter()
            .map(|(id, player)| player.entity_state(*id))
    }

    /// The entity with the given id, if it exists.
    pub fn entity(&self, entity: EntityId) -> Option<EntityState> {
        self.entities().find(|state| state.entity == entity)
    }

    /// Advances the region by one tick.
    pub fn tick(&mut self, inputs: &TickInputs) -> TickOutput {
        self.tick += 1;
        let mut output = TickOutput {
            tick: self.tick,
            ..TickOutput::default()
        };
        self.update_chunks(inputs, &mut output);

        for event in &inputs.edges {
            self.apply_edge_event(event, &mut output);
        }
        for (id, applied) in &inputs.applied {
            if let Some(edge) = self.edges.get_mut(id)
                && edge.applied != *applied
            {
                edge.applied = *applied;
                self.journal.edges.entry(*id).or_default();
            }
        }

        for change in &inputs.player_changes {
            match change {
                PlayerChange::Join(edge, join) => {
                    if !self.edges.contains_key(edge) {
                        // Nobody could be told anything about the player.
                        continue;
                    }
                    if let Some(present) = self.players.get(&join.player) {
                        // The edge admits each player once; a second join is its mistake.
                        if present.edge == *edge {
                            continue;
                        }
                        // The player has connected anew through another edge, which the
                        // edge they had may not have noticed yet. The new connection
                        // replaces the old one.
                        self.remove_player(join.player, &mut output);
                    }
                    let entity_id = self.next_entity_id;
                    if !self.entity_ids.contains(entity_id) {
                        let refused = Durable::Refused {
                            player: join.player,
                        };
                        self.send(*edge, refused, &mut output);
                        continue;
                    }
                    self.next_entity_id = EntityId(entity_id.0 + 1);
                    self.journal.next_entity_id = true;
                    let player = Player {
                        entity_id,
                        name: join.name.clone(),
                        pose: Pose::at(self.config.spawn),
                        moved_from: None,
                        handled_sequence: None,
                        hotbar: self.config.starting_hotbar,
                        selected_slot: 0,
                        last_input: 0,
                        handled: None,
                        edge: *edge,
                    };
                    output.player_events.push((
                        join.player,
                        PlayerEvent::Spawned {
                            entity_id,
                            position: player.pose.position,
                            hotbar: player.hotbar,
                            selected_slot: player.selected_slot,
                        },
                    ));
                    output
                        .events
                        .push(RegionEvent::EntitySpawned(player.entity_state(join.player)));
                    self.players.insert(join.player, player);
                    self.journal.players.insert(join.player);
                }
                PlayerChange::Leave(edge, id) => {
                    // Whatever their entity: a player who quit while entering the world
                    // has none yet that the edge knows of.
                    if self
                        .players
                        .get(id)
                        .is_some_and(|player| player.edge == *edge)
                    {
                        self.remove_player(*id, &mut output);
                    }
                }
                PlayerChange::Arrive(edge, id, transfer) => {
                    let present = self.players.get(id);
                    if present.is_some() || !self.edges.contains_key(edge) {
                        // The player is here already, or nobody could be told about
                        // them. If that is with another entity, or not at all, the one
                        // that was on its way has nowhere to go.
                        if present.is_none_or(|present| present.entity_id != transfer.entity_id) {
                            let position = transfer.pose.position;
                            output.events.push(RegionEvent::EntityRemoved {
                                entity: transfer.entity_id,
                                chunk: ChunkPos::containing(position.x, position.z),
                            });
                        }
                        continue;
                    }
                    let player = Player {
                        entity_id: transfer.entity_id,
                        name: transfer.name.clone(),
                        pose: transfer.pose,
                        moved_from: None,
                        handled_sequence: None,
                        hotbar: transfer.hotbar,
                        selected_slot: transfer.selected_slot,
                        last_input: transfer.last_input,
                        handled: None,
                        edge: *edge,
                    };
                    // Those watching already show the entity if they saw it cross over;
                    // to them this is nothing new.
                    output
                        .events
                        .push(RegionEvent::EntitySpawned(player.entity_state(*id)));
                    self.players.insert(*id, player);
                    self.journal.players.insert(*id);
                }
                PlayerChange::Discard { entity, chunk } => {
                    output.events.push(RegionEvent::EntityRemoved {
                        entity: *entity,
                        chunk: *chunk,
                    });
                }
            }
        }

        for (edge, action) in &inputs.remote_actions {
            // Nobody would hear what became of it.
            if !self.edges.contains_key(edge) {
                continue;
            }
            let answer = self.apply_remote(action, &mut output);
            self.send(*edge, answer, &mut output);
        }
        for (edge, id, number, input) in &inputs.inputs {
            self.apply_input(*edge, *id, *number, input, &mut output);
        }
        let area = self.config.area;
        let departing: Vec<_> = self
            .players
            .iter()
            .filter(|(_, player)| !area.contains(player.chunk()))
            .map(|(id, _)| *id)
            .collect();
        for (id, player) in &mut self.players {
            if let Some(previous_chunk) = player.moved_from.take() {
                output.events.push(RegionEvent::EntityMoved {
                    entity: player.entity_id,
                    pose: player.pose,
                    previous_chunk,
                });
            }
            if let Some(sequence) = player.handled_sequence.take() {
                output
                    .player_events
                    .push((*id, PlayerEvent::Acknowledged { sequence }));
            }
        }
        // Last, so that a player is told what became of their actions before they are
        // told that they are someone else's from now on. Nothing says that the entity
        // is gone: it lives on in the region it walked into.
        for id in departing {
            if let Some(player) = self.players.remove(&id) {
                self.journal.players.insert(id);
                let edge = player.edge;
                let departed = Durable::Departed {
                    player: id,
                    transfer: player.into_transfer(),
                };
                self.send(edge, departed, &mut output);
            }
        }

        output.delta = self.take_delta();
        output
    }

    /// Applies what became of an edge.
    fn apply_edge_event(&mut self, event: &EdgeEvent, output: &mut TickOutput) {
        match *event {
            EdgeEvent::Started { edge: id, start } => match self.edges.get(&id) {
                None => {
                    self.edges.insert(
                        id,
                        EdgeState {
                            start,
                            since: self.tick,
                            ..EdgeState::default()
                        },
                    );
                    // The edge may have been forgotten earlier in this tick, outbox and
                    // all.
                    self.journal.edges.entry(id).or_default().cleared = true;
                }
                Some(known) if known.start < start => {
                    self.drop_edge(id, output);
                    self.edges.insert(
                        id,
                        EdgeState {
                            start,
                            since: self.tick,
                            ..EdgeState::default()
                        },
                    );
                }
                // The same start is the edge on another link, and a lower one never
                // reaches the tick: the runner refuses it.
                Some(_) => {}
            },
            EdgeEvent::Confirmed { edge: id, number } => {
                if let Some(edge) = self.edges.get_mut(&id)
                    && edge.confirm(number)
                {
                    let journal = self.journal.edges.entry(id).or_default();
                    journal.confirmed = journal.confirmed.max(number);
                }
            }
            EdgeEvent::Gone { edge: id } => {
                if self.edges.contains_key(&id) {
                    self.drop_edge(id, output);
                    self.edges.remove(&id);
                }
            }
        }
    }

    /// Removes the players of an edge the region knows and empties its outbox, reporting
    /// every entity of it as removed: the players' and those of the departures in the
    /// outbox, which nobody will pass on now. The edge is left with nothing applied or
    /// sent.
    fn drop_edge(&mut self, id: EdgeId, output: &mut TickOutput) {
        let players: Vec<_> = self
            .players
            .iter()
            .filter(|(_, player)| player.edge == id)
            .map(|(player, _)| *player)
            .collect();
        let mut reported = BTreeSet::new();
        for player in players {
            if let Some(entity) = self.players.get(&player).map(|player| player.entity_id) {
                reported.insert(entity);
            }
            self.remove_player(player, output);
        }
        // A departed player can have come back since, through this edge or another, and
        // keeps their entity when they do. Such an entity is either reported removed
        // above already or alive in the region still, and must not be reported again.
        let alive: BTreeSet<_> = self
            .players
            .values()
            .map(|player| player.entity_id)
            .collect();
        let Some(edge) = self.edges.get_mut(&id) else {
            return;
        };
        for entry in mem::take(&mut edge.outbox).into_values() {
            if let Durable::Departed { transfer, .. } = entry
                && !alive.contains(&transfer.entity_id)
                && reported.insert(transfer.entity_id)
            {
                let position = transfer.pose.position;
                output.events.push(RegionEvent::EntityRemoved {
                    entity: transfer.entity_id,
                    chunk: ChunkPos::containing(position.x, position.z),
                });
            }
        }
        edge.applied = 0;
        edge.sent = 0;
        self.journal.edges.entry(id).or_default().cleared = true;
    }

    /// Takes a player out of the region and reports their entity as removed.
    fn remove_player(&mut self, id: PlayerId, output: &mut TickOutput) {
        if let Some(player) = self.players.remove(&id) {
            self.journal.players.insert(id);
            output.events.push(RegionEvent::EntityRemoved {
                entity: player.entity_id,
                chunk: player.chunk(),
            });
        }
    }

    /// Puts `entry` into the outbox of `edge` under the next number, and reports it.
    fn send(&mut self, id: EdgeId, entry: Durable, output: &mut TickOutput) {
        let edge = self
            .edges
            .get_mut(&id)
            .expect("only players of a known edge and actions through one make entries");
        edge.sent += 1;
        let number = edge.sent;
        edge.outbox.insert(number, entry.clone());
        let journal = self.journal.edges.entry(id).or_default();
        journal.added.push((number, entry.clone()));
        output.durable.push((id, number, entry));
    }

    /// What has changed in this tick, which leaves the journal empty for the next one.
    fn take_delta(&mut self) -> StateDelta {
        let journal = mem::take(&mut self.journal);
        StateDelta {
            tick: self.tick,
            next_entity_id: journal.next_entity_id.then_some(self.next_entity_id),
            players: journal
                .players
                .into_iter()
                .map(|id| (id, self.players.get(&id).map(Player::to_state)))
                .collect(),
            edges: journal
                .edges
                .into_iter()
                .map(|(id, journal)| {
                    let delta = self.edges.get(&id).map(|edge| EdgeDelta {
                        start: edge.start,
                        since: edge.since,
                        applied: edge.applied,
                        sent: edge.sent,
                        cleared: journal.cleared,
                        confirmed: journal.confirmed,
                        added: journal.added,
                    });
                    (id, delta)
                })
                .collect(),
        }
    }

    fn apply_input(
        &mut self,
        edge: EdgeId,
        id: PlayerId,
        number: u64,
        input: &PlayerInput,
        output: &mut TickOutput,
    ) {
        // Input can arrive for a player who has just left.
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        // Only the edge a player belongs to acts for them. What another one passes on is
        // from a connection the player had before.
        if player.edge != edge {
            return;
        }
        // Applied before: it was sent again in case the region the player came from had
        // not got to it.
        if number <= player.last_input {
            return;
        }
        // The player has stepped out and is let go at the end of the tick. What they did
        // after that step is for the region they are in now to judge, which is sent
        // everything this region has not counted as applied.
        if !self.config.area.contains(player.chunk()) {
            return;
        }
        player.last_input = number;
        self.journal.players.insert(id);
        match input {
            PlayerInput::Move {
                position,
                rotation,
                on_ground,
            } => player.move_to(*position, *rotation, *on_ground),
            PlayerInput::SelectSlot { slot } => {
                if usize::from(*slot) < HOTBAR_SLOTS {
                    player.selected_slot = *slot;
                }
            }
            PlayerInput::SetHotbarSlot { slot, stack } => {
                // Only stacks of items that exist are kept.
                let known = stack.is_none_or(|stack| {
                    stack.count > 0
                        && usize::try_from(stack.item).is_ok_and(|item| item < ITEMS.len())
                });
                if let Some(held) = player.hotbar.get_mut(usize::from(*slot))
                    && known
                {
                    *held = *stack;
                }
            }
            PlayerInput::Dig { position, sequence } => {
                let (position, sequence) = (*position, *sequence);
                if player.can_reach(position) && !self.config.area.contains(position.chunk()) {
                    // Within reach, but another region's block: that region decides,
                    // and the player hears that it was handled when it has been.
                    let action = RemoteAction {
                        player: id,
                        sequence,
                        step: RemoteStep::Break { position },
                    };
                    self.send(edge, Durable::Remote(action), output);
                    return;
                }
                // Acknowledged whatever comes of it, so the client stops guessing.
                player.acknowledge(sequence);
                if player.can_reach(position) {
                    self.break_block(position, output);
                }
            }
            PlayerInput::UseItemOn {
                position,
                face,
                sequence,
            } => {
                let (against, sequence) = (*position, *sequence);
                let target = face.neighbour(against);
                let placer = player.pose.position;
                // In creative mode placing does not use the item up.
                let held = player.hotbar[usize::from(player.selected_slot)]
                    .and_then(|stack| ITEMS.get(usize::try_from(stack.item).ok()?)?.block);
                let Some(block) = held.filter(|_| player.can_reach(against)) else {
                    player.acknowledge(sequence);
                    return;
                };
                // Placed against a block, into a free spot nobody stands in. Each of
                // the two is for the region that has it to see to.
                let area = self.config.area;
                let step = if !area.contains(against.chunk()) {
                    Some(RemoteStep::PlaceAgainst {
                        against,
                        target,
                        block,
                        placer,
                    })
                } else if !self.is_block(against) {
                    None
                } else if !area.contains(target.chunk()) {
                    Some(RemoteStep::Place {
                        target,
                        block,
                        placer,
                    })
                } else {
                    self.place_block(target, block, None, output);
                    None
                };
                match step {
                    Some(step) => {
                        let action = RemoteAction {
                            player: id,
                            sequence,
                            step,
                        };
                        self.send(edge, Durable::Remote(action), output);
                    }
                    None => {
                        if let Some(player) = self.players.get_mut(&id) {
                            player.acknowledge(sequence);
                        }
                    }
                }
            }
        }
    }

    /// Takes the next step of something a player of another region did to blocks of
    /// this one. Returns the answer for the edge that passed it on: that it is done, or
    /// what is left of it for a third region.
    fn apply_remote(&mut self, action: &RemoteAction, output: &mut TickOutput) -> Durable {
        let done = Durable::RemoteDone {
            player: action.player,
            sequence: action.sequence,
        };
        // A step about a block this region does not have changes nothing: the block is
        // not loaded here. It is not passed on either, so that regions which disagree
        // about who has what cannot pass an action back and forth for ever.
        match action.step {
            RemoteStep::Break { position } => {
                self.break_block(position, output);
                done
            }
            RemoteStep::PlaceAgainst {
                against,
                target,
                block,
                placer,
            } => {
                if !self.is_block(against) {
                    done
                } else if self.config.area.contains(target.chunk()) {
                    self.place_block(target, block, Some(placer), output);
                    done
                } else {
                    Durable::Remote(RemoteAction {
                        step: RemoteStep::Place {
                            target,
                            block,
                            placer,
                        },
                        ..action.clone()
                    })
                }
            }
            RemoteStep::Place {
                target,
                block,
                placer,
            } => {
                self.place_block(target, block, Some(placer), output);
                done
            }
        }
    }

    /// Whether there is a block at `position` that another can be placed against.
    fn is_block(&self, position: BlockPos) -> bool {
        self.block(position)
            .is_some_and(|state| state != blocks::AIR)
    }

    /// Breaks the block at `position`, if there is one.
    fn break_block(&mut self, position: BlockPos, output: &mut TickOutput) {
        if self.is_block(position) {
            self.set_block(position, blocks::AIR, output);
        }
    }

    /// Places `block` at `target` if the spot is free and nobody stands in it: no player
    /// of this region, and not the one who places it from another region with their
    /// feet at `placer`.
    fn place_block(
        &mut self,
        target: BlockPos,
        block: BlockState,
        placer: Option<Vec3>,
        output: &mut TickOutput,
    ) {
        if self.block(target) == Some(blocks::AIR)
            && !self
                .players
                .values()
                .any(|player| occupies(player.pose.position, target))
            && !placer.is_some_and(|feet| occupies(feet, target))
        {
            self.set_block(target, block, output);
        }
    }

    /// The block at `position`, if its chunk is loaded and it is within the world.
    fn block(&self, position: BlockPos) -> Option<BlockState> {
        let (x, z) = position.in_chunk();
        self.chunks.get(&position.chunk())?.get(x, position.y, z)
    }

    /// Changes a block of a loaded chunk and reports it.
    fn set_block(&mut self, position: BlockPos, state: BlockState, output: &mut TickOutput) {
        let (x, z) = position.in_chunk();
        if let Some(chunk) = self.chunks.get_mut(&position.chunk())
            && chunk.set(x, position.y, z, state).is_some()
        {
            output
                .events
                .push(RegionEvent::BlockChanged { position, state });
        }
    }

    fn update_chunks(&mut self, inputs: &TickInputs, output: &mut TickOutput) {
        // Additions come first so that a ticket released and taken again within one tick
        // keeps the chunk loaded.
        for position in &inputs.tickets_added {
            // Chunks elsewhere are another region's to load.
            if self.config.area.contains(*position) {
                *self.tickets.entry(*position).or_default() += 1;
            }
        }
        for position in &inputs.tickets_removed {
            if let Some(count) = self.tickets.get_mut(position) {
                *count -= 1;
                if *count == 0 {
                    self.tickets.remove(position);
                    self.chunks.remove(position);
                    self.requested.remove(position);
                }
            }
        }
        for (position, chunk) in &inputs.chunks_loaded {
            // A chunk can arrive after everyone stopped needing it.
            if self.requested.remove(position) && self.tickets.contains_key(position) {
                self.chunks.insert(*position, chunk.clone());
            }
        }
        for position in self.tickets.keys() {
            if !self.chunks.contains_key(position) && self.requested.insert(*position) {
                output.chunk_requests.push(*position);
            }
        }
    }
}

/// Whether the body of a player whose feet are at `feet` overlaps the block at `position`.
fn occupies(feet: Vec3, position: BlockPos) -> bool {
    let half = PLAYER_WIDTH / 2.0;
    let overlaps =
        |low: f64, high: f64, block: i32| low < f64::from(block) + 1.0 && high > f64::from(block);
    overlaps(feet.x - half, feet.x + half, position.x)
        && overlaps(feet.y, feet.y + PLAYER_HEIGHT, position.y)
        && overlaps(feet.z - half, feet.z + half, position.z)
}

impl Player {
    /// The chunk the player stands in.
    fn chunk(&self) -> ChunkPos {
        ChunkPos::containing(self.pose.position.x, self.pose.position.z)
    }

    /// The player as of the end of a tick.
    fn to_state(&self) -> PlayerState {
        PlayerState {
            entity_id: self.entity_id,
            name: self.name.clone(),
            pose: self.pose,
            hotbar: self.hotbar,
            selected_slot: self.selected_slot,
            last_input: self.last_input,
            handled: self.handled,
            edge: self.edge,
        }
    }

    fn from_state(state: PlayerState) -> Self {
        Self {
            entity_id: state.entity_id,
            name: state.name,
            pose: state.pose,
            moved_from: None,
            handled_sequence: None,
            hotbar: state.hotbar,
            selected_slot: state.selected_slot,
            last_input: state.last_input,
            handled: state.handled,
            edge: state.edge,
        }
    }

    /// What another region needs to carry on with the player.
    fn into_transfer(self) -> PlayerTransfer {
        PlayerTransfer {
            entity_id: self.entity_id,
            name: self.name,
            pose: self.pose,
            hotbar: self.hotbar,
            selected_slot: self.selected_slot,
            last_input: self.last_input,
        }
    }

    fn entity_state(&self, id: PlayerId) -> EntityState {
        EntityState {
            entity: self.entity_id,
            kind: EntityKind::Player {
                player: id,
                name: self.name.clone(),
            },
            pose: self.pose,
        }
    }

    /// Notes that everything up to `sequence` has been handled.
    fn acknowledge(&mut self, sequence: i32) {
        self.handled_sequence = self.handled_sequence.max(Some(sequence));
        self.handled = self.handled.max(Some(sequence));
    }

    /// Whether the player's eyes are close enough to the block at `position` to work on it.
    fn can_reach(&self, position: BlockPos) -> bool {
        let eye = [
            self.pose.position.x,
            self.pose.position.y + EYE_HEIGHT,
            self.pose.position.z,
        ];
        let block = [position.x, position.y, position.z];
        // Distance from the eye to the nearest point of the block.
        let squared: f64 = eye
            .into_iter()
            .zip(block)
            .map(|(eye, block)| {
                let low = f64::from(block);
                let nearest = eye.clamp(low, low + 1.0);
                (eye - nearest).powi(2)
            })
            .sum();
        squared <= BLOCK_REACH * BLOCK_REACH
    }

    fn move_to(&mut self, position: Option<Vec3>, rotation: Option<(f32, f32)>, on_ground: bool) {
        // Input that is not a number is dropped as a whole: nothing sensible can be done
        // with it and it must never enter the state.
        let numbers = position
            .iter()
            .flat_map(|position| [position.x, position.y, position.z])
            .chain(
                rotation
                    .iter()
                    .flat_map(|(yaw, pitch)| [f64::from(*yaw), f64::from(*pitch)]),
            );
        if !numbers.into_iter().all(f64::is_finite) {
            return;
        }

        let mut pose = self.pose;
        if let Some(position) = position {
            pose.position = Vec3::new(
                position
                    .x
                    .clamp(-MAX_HORIZONTAL_COORDINATE, MAX_HORIZONTAL_COORDINATE),
                position
                    .y
                    .clamp(-MAX_VERTICAL_COORDINATE, MAX_VERTICAL_COORDINATE),
                position
                    .z
                    .clamp(-MAX_HORIZONTAL_COORDINATE, MAX_HORIZONTAL_COORDINATE),
            );
        }
        if let Some((yaw, pitch)) = rotation {
            pose.yaw = yaw;
            pose.pitch = pitch;
        }
        pose.on_ground = on_ground;

        if pose != self.pose {
            let current = self.pose.position;
            self.moved_from
                .get_or_insert(ChunkPos::containing(current.x, current.z));
            self.pose = pose;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use clustine_world::Biome;

    use super::*;
    use clustine_data::items;

    use crate::api::{Face, PlayerJoin};

    const SPAWN: Vec3 = Vec3::new(0.5, -60.0, 0.5);

    const STONE: ItemStack = ItemStack {
        item: items::STONE,
        count: 1,
    };
    const DIRT: ItemStack = ItemStack {
        item: items::DIRT,
        count: 1,
    };
    /// An item that is not a block.
    const STICK: ItemStack = ItemStack {
        item: items::STICK,
        count: 1,
    };

    /// Stone, dirt and a stick in the first three slots.
    fn hotbar() -> [Option<ItemStack>; HOTBAR_SLOTS] {
        let mut hotbar = [None; HOTBAR_SLOTS];
        hotbar[0] = Some(STONE);
        hotbar[1] = Some(DIRT);
        hotbar[2] = Some(STICK);
        hotbar
    }

    /// What a region for `area` is created with.
    fn config(area: ChunkArea) -> RegionConfig {
        RegionConfig {
            spawn: SPAWN,
            area,
            starting_hotbar: hotbar(),
        }
    }

    /// The edge the players of these tests come through.
    const EDGE: EdgeId = EdgeId(0xED6E);

    /// The edge that passes on what players of other regions do to blocks of the
    /// region, so that the answers can be told from what the region's own players ask
    /// of others.
    const REMOTE: EdgeId = EdgeId(0x0E6E);

    /// A region that has never run, gives out `entity_ids` and knows both edges.
    fn fresh(config: RegionConfig, entity_ids: EntityIds) -> Region {
        let mut state = RegionState::new(entity_ids);
        for edge in [EDGE, REMOTE] {
            let known = EdgeState {
                start: 1,
                ..EdgeState::default()
            };
            state.edges.insert(edge, known);
        }
        Region::restore(config, state)
    }

    /// A fresh region for `area` with the first block of entity ids.
    fn region_in(area: ChunkArea) -> Region {
        fresh(config(area), EntityIds::block(0).unwrap())
    }

    fn region() -> Region {
        region_in(ChunkArea::EVERYWHERE)
    }

    /// An input as the edge passes it on: numbered in the order the inputs are made.
    type Input = (EdgeId, PlayerId, u64, PlayerInput);

    fn numbered(player: PlayerId, input: PlayerInput) -> Input {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        (EDGE, player, NEXT.fetch_add(1, Ordering::Relaxed), input)
    }

    /// `input` with a number of the test's choosing in place of the next one.
    fn with_number(number: u64, (edge, player, _, input): Input) -> Input {
        (edge, player, number, input)
    }

    /// What the players are told as a worker tells them, which is as a region told them
    /// before it had an outbox: whether they entered the world or were refused, then
    /// what was acknowledged, then who was let go.
    fn told(output: &TickOutput) -> Vec<(PlayerId, PlayerEvent)> {
        let TickOutput {
            player_events: events,
            ..
        } = output;
        let spawned = events
            .iter()
            .position(|(_, event)| matches!(event, PlayerEvent::Acknowledged { .. }))
            .unwrap_or(events.len());
        let refused = output
            .durable
            .iter()
            .filter_map(|(_, _, entry)| match entry {
                Durable::Refused { player } => Some((*player, PlayerEvent::Refused)),
                _ => None,
            });
        let departed = output
            .durable
            .iter()
            .filter_map(|(_, _, entry)| match entry {
                Durable::Departed { player, transfer } => {
                    Some((*player, PlayerEvent::Departed(transfer.clone())))
                }
                _ => None,
            });
        let mut told = events[..spawned].to_vec();
        told.extend(refused);
        told.extend_from_slice(&events[spawned..]);
        told.extend(departed);
        told
    }

    /// What the region's own players asked of other regions: what the region sent to
    /// `EDGE` to pass on.
    fn requests(output: &TickOutput) -> Vec<RemoteAction> {
        let entries = output.durable.iter().filter(|(edge, ..)| *edge == EDGE);
        entries
            .filter_map(|(_, _, entry)| match entry {
                Durable::Remote(action) => Some(action.clone()),
                _ => None,
            })
            .collect()
    }

    /// What became of a remote action, as the region answered it.
    #[derive(Debug, Clone, PartialEq)]
    enum RemoteOutcome {
        Done { player: PlayerId, sequence: i32 },
        Next(RemoteAction),
    }

    /// The answers to what `REMOTE` passed on, in order.
    fn outcomes(output: &TickOutput) -> Vec<RemoteOutcome> {
        let entries = output.durable.iter().filter(|(edge, ..)| *edge == REMOTE);
        entries
            .map(|(_, _, entry)| match entry {
                Durable::RemoteDone { player, sequence } => RemoteOutcome::Done {
                    player: *player,
                    sequence: *sequence,
                },
                Durable::Remote(action) => RemoteOutcome::Next(action.clone()),
                other => panic!("{other:?} is no answer to a remote action"),
            })
            .collect()
    }

    fn player(number: u128) -> PlayerId {
        PlayerId(uuid::Uuid::from_u128(number))
    }

    fn join(number: u128) -> PlayerChange {
        PlayerChange::Join(
            EDGE,
            PlayerJoin {
                player: player(number),
                name: format!("Player{number}"),
            },
        )
    }

    fn leave(number: u128) -> PlayerChange {
        PlayerChange::Leave(EDGE, player(number))
    }

    fn changes(player_changes: Vec<PlayerChange>) -> TickInputs {
        TickInputs {
            player_changes,
            ..TickInputs::default()
        }
    }

    fn walk(number: u128, x: f64, z: f64) -> Input {
        let input = PlayerInput::Move {
            position: Some(Vec3::new(x, -60.0, z)),
            rotation: None,
            on_ground: true,
        };
        numbered(player(number), input)
    }

    fn moves(inputs: Vec<Input>) -> TickInputs {
        TickInputs {
            inputs,
            ..TickInputs::default()
        }
    }

    /// A region that the given players have joined.
    fn joined(numbers: &[u128]) -> Region {
        joined_in(ChunkArea::EVERYWHERE, numbers)
    }

    /// A region for `area` that the given players have joined.
    fn joined_in(area: ChunkArea, numbers: &[u128]) -> Region {
        let mut region = region_in(area);
        region.tick(&changes(
            numbers.iter().map(|number| join(*number)).collect(),
        ));
        region
    }

    fn state(number: u128, entity: i32, position: Vec3) -> EntityState {
        EntityState {
            entity: EntityId(entity),
            kind: EntityKind::Player {
                player: player(number),
                name: format!("Player{number}"),
            },
            pose: Pose::at(position),
        }
    }

    fn chunk() -> Chunk {
        let overworld = clustine_data::DIMENSION_TYPES
            .iter()
            .find(|dimension| dimension.name == "minecraft:overworld")
            .unwrap();
        Chunk::empty(overworld, Biome(0))
    }

    fn tickets(added: Vec<ChunkPos>, removed: Vec<ChunkPos>) -> TickInputs {
        TickInputs {
            tickets_added: added,
            tickets_removed: removed,
            ..TickInputs::default()
        }
    }

    fn loaded(position: ChunkPos) -> TickInputs {
        TickInputs {
            chunks_loaded: vec![(position, chunk())],
            ..TickInputs::default()
        }
    }

    #[test]
    fn ticks_are_numbered_from_one() {
        let mut region = region();
        assert_eq!(region.tick(&TickInputs::default()).tick, 1);
        assert_eq!(region.tick(&TickInputs::default()).tick, 2);
    }

    #[test]
    fn joining_players_spawn_with_distinct_entity_ids() {
        let mut region = region();
        let output = region.tick(&changes(vec![join(1), join(2)]));
        let spawned = |entity| PlayerEvent::Spawned {
            entity_id: EntityId(entity),
            position: SPAWN,
            hotbar: hotbar(),
            selected_slot: 0,
        };
        assert_eq!(
            told(&output),
            [(player(1), spawned(1)), (player(2), spawned(2))]
        );
        assert_eq!(
            output.events,
            [
                RegionEvent::EntitySpawned(state(1, 1, SPAWN)),
                RegionEvent::EntitySpawned(state(2, 2, SPAWN)),
            ]
        );
        assert_eq!(region.player_count(), 2);
        assert_eq!(
            region.player(player(2)),
            Some((EntityId(2), Pose::at(SPAWN)))
        );
        assert_eq!(region.player(player(3)), None);
    }

    #[test]
    fn entities_can_be_listed_and_looked_up() {
        let region = joined(&[1, 2]);
        assert_eq!(
            region.entities().collect::<Vec<_>>(),
            [state(1, 1, SPAWN), state(2, 2, SPAWN)]
        );
        assert_eq!(region.entity(EntityId(2)), Some(state(2, 2, SPAWN)));
        assert_eq!(region.entity(EntityId(3)), None);
        assert_eq!(state(1, 1, SPAWN).chunk(), ChunkPos::new(0, 0));
    }

    #[test]
    fn leaving_removes_the_entity_from_where_it_was() {
        let mut region = joined(&[1]);
        region.tick(&moves(vec![walk(1, 40.0, -1.0)]));
        let output = region.tick(&changes(vec![leave(1)]));
        assert_eq!(
            output.events,
            [RegionEvent::EntityRemoved {
                entity: EntityId(1),
                chunk: ChunkPos::new(2, -1),
            }]
        );
        assert_eq!(region.player_count(), 0);
        // Leaving twice, or without having joined, changes nothing.
        assert!(
            region
                .tick(&changes(vec![leave(1), leave(9)]))
                .events
                .is_empty()
        );
    }

    #[test]
    fn leaving_and_coming_back_within_one_tick_gives_a_new_entity() {
        let mut region = joined(&[1]);
        let output = region.tick(&changes(vec![leave(1), join(1)]));
        assert_eq!(
            output.events,
            [
                RegionEvent::EntityRemoved {
                    entity: EntityId(1),
                    chunk: ChunkPos::new(0, 0),
                },
                RegionEvent::EntitySpawned(state(1, 2, SPAWN)),
            ]
        );
        assert_eq!(region.player(player(1)).unwrap().0, EntityId(2));
    }

    /// The order of changes within a tick decides the outcome: this is the reverse of
    /// the test above and must leave nobody behind.
    #[test]
    fn joining_and_leaving_within_one_tick_leaves_nobody() {
        let mut region = region();
        let output = region.tick(&changes(vec![join(1), leave(1)]));
        assert_eq!(region.player_count(), 0);
        assert_eq!(output.events.len(), 2);

        // The player can join again later.
        let output = region.tick(&changes(vec![join(1)]));
        assert_eq!(told(&output).len(), 1);
    }

    #[test]
    fn a_second_join_of_the_same_player_is_ignored() {
        let mut region = joined(&[1]);
        let output = region.tick(&changes(vec![join(1)]));
        assert!(told(&output).is_empty());
        assert!(output.events.is_empty());
        assert_eq!(region.player_count(), 1);
    }

    #[test]
    fn ticketed_chunks_are_requested_once_and_loaded_when_they_arrive() {
        let mut region = region();
        let position = ChunkPos::new(2, -3);

        let output = region.tick(&tickets(vec![position], vec![]));
        assert_eq!(output.chunk_requests, [position]);
        assert!(region.chunk(position).is_none());

        // Not requested again while the answer is outstanding.
        let output = region.tick(&TickInputs::default());
        assert!(output.chunk_requests.is_empty());

        region.tick(&loaded(position));
        assert_eq!(region.chunk(position), Some(&chunk()));
        let output = region.tick(&TickInputs::default());
        assert!(output.chunk_requests.is_empty());
    }

    #[test]
    fn a_chunk_stays_loaded_until_its_last_ticket_is_released() {
        let mut region = region();
        let position = ChunkPos::new(0, 0);
        region.tick(&tickets(vec![position, position], vec![]));
        region.tick(&loaded(position));

        region.tick(&tickets(vec![], vec![position]));
        assert_eq!(region.loaded_chunk_count(), 1);
        region.tick(&tickets(vec![], vec![position]));
        assert_eq!(region.loaded_chunk_count(), 0);
    }

    #[test]
    fn a_ticket_moved_within_one_tick_keeps_the_chunk() {
        let mut region = region();
        let position = ChunkPos::new(0, 0);
        region.tick(&tickets(vec![position], vec![]));
        region.tick(&loaded(position));

        let output = region.tick(&tickets(vec![position], vec![position]));
        assert!(output.chunk_requests.is_empty());
        assert_eq!(region.loaded_chunk_count(), 1);
    }

    #[test]
    fn a_chunk_that_arrives_after_its_ticket_was_released_is_dropped() {
        let mut region = region();
        let position = ChunkPos::new(0, 0);
        region.tick(&tickets(vec![position], vec![]));
        region.tick(&tickets(vec![], vec![position]));
        region.tick(&loaded(position));
        assert_eq!(region.loaded_chunk_count(), 0);

        // Needing it again asks storage again.
        let output = region.tick(&tickets(vec![position], vec![]));
        assert_eq!(output.chunk_requests, [position]);
    }

    #[test]
    fn unrequested_chunks_are_ignored() {
        let mut region = region();
        region.tick(&loaded(ChunkPos::new(9, 9)));
        assert_eq!(region.loaded_chunk_count(), 0);
    }

    #[test]
    fn moves_within_a_tick_are_reported_once_with_the_final_pose() {
        let mut region = joined(&[1]);
        let output = region.tick(&moves(vec![
            walk(1, 1.0, 0.5),
            walk(1, 2.0, 0.5),
            walk(1, 17.0, 0.5),
        ]));
        let pose = Pose {
            position: Vec3::new(17.0, -60.0, 0.5),
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
        };
        assert_eq!(
            output.events,
            [RegionEvent::EntityMoved {
                entity: EntityId(1),
                pose,
                previous_chunk: ChunkPos::new(0, 0),
            }]
        );
        assert_eq!(
            output.events[0].chunks(),
            [ChunkPos::new(1, 0), ChunkPos::new(0, 0)]
        );
        assert_eq!(region.player(player(1)), Some((EntityId(1), pose)));

        // Nothing is reported for a tick without movement.
        assert!(region.tick(&TickInputs::default()).events.is_empty());
    }

    #[test]
    fn rotation_and_ground_contact_change_independently_of_the_position() {
        let mut region = joined(&[1]);
        let turn = PlayerInput::Move {
            position: None,
            rotation: Some((90.0, -30.0)),
            on_ground: true,
        };
        let output = region.tick(&moves(vec![numbered(player(1), turn.clone())]));
        let (_, pose) = region.player(player(1)).unwrap();
        assert_eq!(pose.position, SPAWN);
        assert_eq!((pose.yaw, pose.pitch, pose.on_ground), (90.0, -30.0, true));
        assert_eq!(output.events.len(), 1);

        // Repeating the same input changes nothing, so nothing is reported.
        let output = region.tick(&moves(vec![numbered(player(1), turn)]));
        assert!(output.events.is_empty());
    }

    #[test]
    fn moves_are_reported_in_a_fixed_order() {
        let mut region = joined(&[1, 2]);
        let output = region.tick(&moves(vec![walk(2, 5.0, 5.0), walk(1, 3.0, 3.0)]));
        let entities: Vec<_> = output
            .events
            .iter()
            .map(|event| match event {
                RegionEvent::EntityMoved { entity, .. } => *entity,
                other => panic!("unexpected event {other:?}"),
            })
            .collect();
        assert_eq!(entities, [EntityId(1), EntityId(2)]);
    }

    #[test]
    fn positions_outside_the_world_are_pulled_back() {
        let mut region = joined(&[1]);
        region.tick(&moves(vec![walk(1, 1e9, -1e9)]));
        let (_, pose) = region.player(player(1)).unwrap();
        assert_eq!(pose.position, Vec3::new(3.0e7, -60.0, -3.0e7));
    }

    #[test]
    fn input_that_is_not_a_number_is_dropped() {
        let mut region = joined(&[1]);
        let before = region.player(player(1));
        for bad in [f64::NAN, f64::INFINITY] {
            let output = region.tick(&moves(vec![walk(1, bad, 0.0), walk(1, 0.0, bad)]));
            assert!(output.events.is_empty());
        }
        let spin = PlayerInput::Move {
            position: None,
            rotation: Some((f32::NAN, 0.0)),
            on_ground: false,
        };
        region.tick(&moves(vec![numbered(player(1), spin)]));
        assert_eq!(region.player(player(1)), before);
    }

    #[test]
    fn input_for_an_absent_player_is_ignored() {
        let mut region = joined(&[1]);
        let output = region.tick(&TickInputs {
            player_changes: vec![leave(1)],
            inputs: vec![walk(1, 9.0, 9.0), walk(7, 1.0, 1.0)],
            ..TickInputs::default()
        });
        assert_eq!(
            output.events.len(),
            1,
            "only the removal: {:?}",
            output.events
        );
    }

    fn dig(number: u128, x: i32, y: i32, z: i32, sequence: i32) -> Input {
        let input = PlayerInput::Dig {
            position: BlockPos::new(x, y, z),
            sequence,
        };
        numbered(player(number), input)
    }

    /// A chunk with a floor of stone right below where players stand.
    fn floor() -> Chunk {
        let mut chunk = chunk();
        for z in 0..16 {
            for x in 0..16 {
                chunk.set(x, -61, z, blocks::STONE);
            }
        }
        chunk
    }

    /// A region with the given players standing on a stone floor in chunk (0, 0).
    fn on_floor(numbers: &[u128]) -> Region {
        on_floor_in(ChunkArea::EVERYWHERE, numbers)
    }

    /// The same in a region for `area`, which has to contain that chunk.
    fn on_floor_in(area: ChunkArea, numbers: &[u128]) -> Region {
        let mut region = joined_in(area, numbers);
        let origin = ChunkPos::new(0, 0);
        region.tick(&tickets(vec![origin], vec![]));
        region.tick(&TickInputs {
            chunks_loaded: vec![(origin, floor())],
            ..TickInputs::default()
        });
        region
    }

    fn acknowledged(number: u128, sequence: i32) -> (PlayerId, PlayerEvent) {
        (player(number), PlayerEvent::Acknowledged { sequence })
    }

    #[test]
    fn digging_removes_the_block_and_is_acknowledged() {
        let mut region = on_floor(&[1]);
        let output = region.tick(&moves(vec![dig(1, 2, -61, 3, 7)]));
        assert_eq!(
            output.events,
            [RegionEvent::BlockChanged {
                position: BlockPos::new(2, -61, 3),
                state: blocks::AIR,
            }]
        );
        assert_eq!(output.events[0].chunks(), [ChunkPos::new(0, 0); 2]);
        assert_eq!(told(&output), [acknowledged(1, 7)]);

        let chunk = region.chunk(ChunkPos::new(0, 0)).unwrap();
        assert_eq!(chunk.get(2, -61, 3), Some(blocks::AIR));
        assert_eq!(chunk.get(3, -61, 3), Some(blocks::STONE));
    }

    #[test]
    fn digging_where_nothing_can_be_broken_is_only_acknowledged() {
        let mut region = on_floor(&[1]);
        let attempts = [
            // Air.
            dig(1, 2, -60, 3, 1),
            // Too far away: 12 blocks along the floor.
            dig(1, 12, -61, 0, 2),
            // Below and above the world.
            dig(1, 0, -65, 0, 3),
            dig(1, 0, 320, 0, 4),
            // In a chunk that is not loaded.
            dig(1, -1, -61, 0, 5),
        ];
        for attempt in attempts {
            let PlayerInput::Dig { sequence, .. } = attempt.3 else {
                unreachable!();
            };
            let output = region.tick(&moves(vec![attempt]));
            assert!(output.events.is_empty(), "sequence {sequence}");
            assert_eq!(told(&output), [acknowledged(1, sequence)]);
        }
        assert_eq!(region.chunk(ChunkPos::new(0, 0)), Some(&floor()));
    }

    #[test]
    fn reach_is_measured_from_the_eyes_to_the_nearest_point_of_the_block() {
        let mut region = on_floor(&[1]);
        // The eyes are at x = 0.5. The block at x = 6 begins 5.5 blocks away
        // horizontally and about 1.6 below: just within 6. The one at x = 7 is not.
        let output = region.tick(&moves(vec![dig(1, 7, -61, 0, 1), dig(1, 6, -61, 0, 2)]));
        assert_eq!(
            output.events,
            [RegionEvent::BlockChanged {
                position: BlockPos::new(6, -61, 0),
                state: blocks::AIR,
            }]
        );
    }

    #[test]
    fn one_acknowledgement_per_tick_covers_the_highest_sequence() {
        let mut region = on_floor(&[1, 2]);
        let output = region.tick(&moves(vec![
            dig(1, 0, -61, 0, 5),
            dig(2, 1, -61, 0, 3),
            dig(1, 0, -61, 1, 6),
            dig(1, 0, -61, 2, 4),
        ]));
        assert_eq!(output.events.len(), 4);
        assert_eq!(told(&output), [acknowledged(1, 6), acknowledged(2, 3)]);
        // Nothing is acknowledged in a tick without such input.
        assert!(region.tick(&TickInputs::default()).player_events.is_empty());
    }

    #[test]
    fn a_block_cannot_be_broken_twice() {
        let mut region = on_floor(&[1, 2]);
        let output = region.tick(&moves(vec![dig(1, 0, -61, 0, 1), dig(2, 0, -61, 0, 1)]));
        assert_eq!(output.events.len(), 1);
        assert_eq!(told(&output).len(), 2);
    }

    fn place(number: u128, x: i32, y: i32, z: i32, face: Face) -> Input {
        let input = PlayerInput::UseItemOn {
            position: BlockPos::new(x, y, z),
            face,
            sequence: 1,
        };
        numbered(player(number), input)
    }

    fn select(number: u128, slot: u8) -> Input {
        numbered(player(number), PlayerInput::SelectSlot { slot })
    }

    fn changed(x: i32, y: i32, z: i32, state: BlockState) -> RegionEvent {
        RegionEvent::BlockChanged {
            position: BlockPos::new(x, y, z),
            state,
        }
    }

    #[test]
    fn using_a_block_item_on_a_block_places_it_against_the_clicked_face() {
        let mut region = on_floor(&[1]);
        // On top of the floor, two blocks from the player.
        let output = region.tick(&moves(vec![place(1, 2, -61, 0, Face::Top)]));
        assert_eq!(output.events, [changed(2, -60, 0, blocks::STONE)]);
        assert_eq!(told(&output), [acknowledged(1, 1)]);

        // Against the side of the block just placed, with the next hotbar slot.
        let output = region.tick(&moves(vec![select(1, 1), place(1, 2, -60, 0, Face::South)]));
        assert_eq!(output.events, [changed(2, -60, 1, blocks::DIRT)]);

        let chunk = region.chunk(ChunkPos::new(0, 0)).unwrap();
        assert_eq!(chunk.get(2, -60, 0), Some(blocks::STONE));
        assert_eq!(chunk.get(2, -60, 1), Some(blocks::DIRT));
    }

    #[test]
    fn faces_point_to_the_six_neighbours() {
        let origin = BlockPos::new(0, 0, 0);
        let neighbours = [
            (Face::Bottom, (0, -1, 0)),
            (Face::Top, (0, 1, 0)),
            (Face::North, (0, 0, -1)),
            (Face::South, (0, 0, 1)),
            (Face::West, (-1, 0, 0)),
            (Face::East, (1, 0, 0)),
        ];
        for (face, (x, y, z)) in neighbours {
            assert_eq!(face.neighbour(origin), BlockPos::new(x, y, z), "{face:?}");
        }
    }

    #[test]
    fn placing_where_it_is_not_possible_is_only_acknowledged() {
        let mut region = on_floor(&[1, 2]);
        region.tick(&moves(vec![walk(2, 3.5, 3.5)]));
        let attempts = [
            // Where the player themselves stands, and where the other player stands.
            place(1, 0, -61, 0, Face::Top),
            place(1, 3, -61, 3, Face::Top),
            // Against thin air.
            place(1, 2, -55, 0, Face::Top),
            // Into a spot that is taken: below the floor is air, but the floor is not.
            place(1, 2, -62, 0, Face::Top),
            // Out of reach.
            place(1, 12, -61, 0, Face::Top),
            // Above the top of the world.
            place(1, 0, 319, 0, Face::Top),
        ];
        for (index, attempt) in attempts.into_iter().enumerate() {
            let output = region.tick(&moves(vec![attempt]));
            assert!(
                output.events.is_empty(),
                "attempt {index}: {:?}",
                output.events
            );
            assert_eq!(told(&output), [acknowledged(1, 1)], "attempt {index}");
        }
    }

    #[test]
    fn only_block_items_can_be_placed() {
        let mut region = on_floor(&[1]);
        // A stick, then an empty slot.
        for slot in [2, 5] {
            let output = region.tick(&moves(vec![
                select(1, slot),
                place(1, 2, -61, 0, Face::Top),
            ]));
            assert!(output.events.is_empty(), "slot {slot}");
            assert_eq!(told(&output), [acknowledged(1, 1)]);
        }
        // Selecting a slot that does not exist keeps the selection.
        let output = region.tick(&moves(vec![
            select(1, 0),
            select(1, 9),
            place(1, 2, -61, 0, Face::Top),
        ]));
        assert_eq!(output.events, [changed(2, -60, 0, blocks::STONE)]);
    }

    #[test]
    fn creative_players_fill_their_own_hotbar() {
        let mut region = on_floor(&[1]);
        let set = |slot, stack| numbered(player(1), PlayerInput::SetHotbarSlot { slot, stack });
        let glass = ItemStack {
            item: items::GLASS,
            count: 1,
        };
        let output = region.tick(&moves(vec![
            set(4, Some(glass)),
            select(1, 4),
            place(1, 2, -61, 0, Face::Top),
        ]));
        assert_eq!(output.events, [changed(2, -60, 0, blocks::GLASS)]);

        // Emptying the slot again leaves nothing to place.
        let output = region.tick(&moves(vec![set(4, None), place(1, 3, -61, 0, Face::Top)]));
        assert!(output.events.is_empty());

        // Slots that do not exist and items that do not exist are ignored.
        let nonsense = ItemStack {
            item: i32::MAX,
            count: 1,
        };
        let output = region.tick(&moves(vec![
            set(9, Some(glass)),
            set(4, Some(nonsense)),
            place(1, 3, -61, 0, Face::Top),
        ]));
        assert!(output.events.is_empty());
    }

    #[test]
    fn a_placed_block_can_be_broken_again() {
        let mut region = on_floor(&[1]);
        region.tick(&moves(vec![place(1, 2, -61, 0, Face::Top)]));
        let output = region.tick(&moves(vec![dig(1, 2, -60, 0, 2)]));
        assert_eq!(output.events, [changed(2, -60, 0, blocks::AIR)]);
        assert_eq!(region.chunk(ChunkPos::new(0, 0)), Some(&floor()));
    }

    /// Chunks 0 and 1: the blocks with x from 0 up to, but not including, 32.
    const MIDDLE: ChunkArea = ChunkArea {
        min_x: Some(0),
        max_x: Some(2),
    };
    /// The two parts of a world divided at the block with x = 0, which like the spawn
    /// point belongs to the eastern one.
    const WEST: ChunkArea = ChunkArea {
        min_x: None,
        max_x: Some(0),
    };
    const EAST: ChunkArea = ChunkArea {
        min_x: Some(0),
        max_x: None,
    };

    const GLASS: ItemStack = ItemStack {
        item: items::GLASS,
        count: 1,
    };

    fn set_slot(number: u128, slot: u8, stack: Option<ItemStack>) -> Input {
        numbered(player(number), PlayerInput::SetHotbarSlot { slot, stack })
    }

    /// What a region hands over when it lets a player go at `x` after their input
    /// `last_input`. Nothing about them is as it is for a player who has just joined.
    fn transfer(number: u128, entity: i32, x: f64, last_input: u64) -> PlayerTransfer {
        let mut hotbar = hotbar();
        hotbar[0] = None;
        hotbar[6] = Some(GLASS);
        PlayerTransfer {
            entity_id: EntityId(entity),
            name: format!("Player{number}"),
            pose: Pose {
                position: Vec3::new(x, -60.0, 0.5),
                yaw: 135.0,
                pitch: -20.0,
                on_ground: true,
            },
            hotbar,
            selected_slot: 6,
            last_input,
        }
    }

    fn arrive(number: u128, transfer: &PlayerTransfer) -> PlayerChange {
        PlayerChange::Arrive(EDGE, player(number), transfer.clone())
    }

    fn departed(number: u128, transfer: PlayerTransfer) -> (PlayerId, PlayerEvent) {
        (player(number), PlayerEvent::Departed(transfer))
    }

    fn is_removal(event: &RegionEvent) -> bool {
        matches!(event, RegionEvent::EntityRemoved { .. })
    }

    #[test]
    fn chunks_outside_the_area_are_neither_requested_nor_loaded() {
        let mut region = region_in(MIDDLE);
        // Right beyond either end, the eastern one not being part of the area, and far off.
        let outside = [
            ChunkPos::new(-1, 0),
            ChunkPos::new(2, 0),
            ChunkPos::new(40, -7),
        ];
        let inside = [ChunkPos::new(0, 5), ChunkPos::new(1, -7)];
        let all = [outside.as_slice(), &inside].concat();

        let output = region.tick(&tickets(all.clone(), vec![]));
        assert_eq!(output.chunk_requests, inside);

        // Storage is not asked later either, and a chunk nobody asked for is not taken.
        let output = region.tick(&TickInputs {
            chunks_loaded: all.iter().map(|position| (*position, chunk())).collect(),
            ..TickInputs::default()
        });
        assert!(output.chunk_requests.is_empty());
        for position in outside {
            assert_eq!(region.chunk(position), None, "{position:?}");
        }
        for position in inside {
            assert_eq!(region.chunk(position), Some(&chunk()), "{position:?}");
        }
        assert_eq!(region.loaded_chunk_count(), 2);

        // Tickets that were never counted can be released and taken again without effect.
        let output = region.tick(&tickets(outside.to_vec(), outside.to_vec()));
        assert!(output.chunk_requests.is_empty());
        assert_eq!(region.loaded_chunk_count(), 2);
        // Inside, a chunk still goes with its last ticket.
        region.tick(&tickets(vec![], inside.to_vec()));
        assert_eq!(region.loaded_chunk_count(), 0);
    }

    #[test]
    fn an_input_numbered_no_higher_than_the_last_applied_one_is_ignored() {
        let mut region = on_floor(&[1, 2]);
        let x = |region: &Region, number| region.player(player(number)).unwrap().1.position.x;

        // The same number twice.
        region.tick(&moves(vec![
            with_number(5, walk(1, 1.5, 0.5)),
            with_number(5, walk(1, 2.5, 0.5)),
        ]));
        assert_eq!(x(&region, 1), 1.5);

        // A lower number arriving later, be it in the same tick or in a later one.
        region.tick(&moves(vec![
            with_number(7, walk(1, 3.5, 0.5)),
            with_number(6, walk(1, 4.5, 0.5)),
        ]));
        assert_eq!(x(&region, 1), 3.5);
        let output = region.tick(&moves(vec![
            with_number(4, walk(1, 5.5, 0.5)),
            with_number(7, walk(1, 6.5, 0.5)),
            // Nothing at all comes of such an input: no block breaks, and the player is
            // not told that it was handled.
            with_number(7, dig(1, 2, -61, 0, 9)),
        ]));
        assert!(output.events.is_empty(), "{:?}", output.events);
        assert!(told(&output).is_empty());
        assert_eq!(x(&region, 1), 3.5);
        assert_eq!(region.chunk(ChunkPos::new(0, 0)), Some(&floor()));

        // The next higher number is applied, and each player's inputs are numbered on
        // their own.
        region.tick(&moves(vec![
            with_number(8, walk(1, 8.5, 0.5)),
            with_number(1, walk(2, 7.5, 0.5)),
        ]));
        assert_eq!((x(&region, 1), x(&region, 2)), (8.5, 7.5));
    }

    #[test]
    fn a_player_who_steps_out_of_the_area_is_let_go_with_all_they_carry() {
        let mut region = on_floor_in(MIDDLE, &[1]);
        region.tick(&moves(vec![with_number(1, set_slot(1, 4, Some(GLASS)))]));

        let output = region.tick(&moves(vec![
            with_number(2, dig(1, 2, -61, 3, 7)),
            with_number(3, select(1, 4)),
            // Chunk 2 is the first one east of the area.
            with_number(4, walk(1, 32.5, 0.5)),
            // What follows the step is for the region the player is in now to judge.
            with_number(5, select(1, 1)),
            with_number(6, dig(1, 3, -61, 3, 8)),
            with_number(7, walk(1, 1.5, 0.5)),
        ]));
        let pose = Pose {
            position: Vec3::new(32.5, -60.0, 0.5),
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
        };
        // Those watching see the step, and nothing tells them that the entity is gone.
        assert_eq!(
            output.events,
            [
                changed(2, -61, 3, blocks::AIR),
                RegionEvent::EntityMoved {
                    entity: EntityId(1),
                    pose,
                    previous_chunk: ChunkPos::new(0, 0),
                },
            ]
        );
        let mut carried = hotbar();
        carried[4] = Some(GLASS);
        assert_eq!(
            told(&output),
            [
                acknowledged(1, 7),
                departed(
                    1,
                    PlayerTransfer {
                        entity_id: EntityId(1),
                        name: "Player1".to_owned(),
                        pose,
                        hotbar: carried,
                        selected_slot: 4,
                        last_input: 4,
                    }
                ),
            ]
        );
        assert_eq!(region.player_count(), 0);
        assert_eq!(region.player(player(1)), None);
        assert_eq!(region.entity(EntityId(1)), None);

        // The region has nothing to do with the player any more, not even when they leave.
        let output = region.tick(&TickInputs {
            player_changes: vec![leave(1)],
            inputs: vec![with_number(8, walk(1, 1.5, 0.5))],
            ..TickInputs::default()
        });
        assert!(output.events.is_empty(), "{:?}", output.events);
        assert!(told(&output).is_empty());
    }

    #[test]
    fn a_player_is_let_go_exactly_where_the_area_ends() {
        let steps = [
            (MIDDLE, 0.0, true),
            (MIDDLE, -0.1, false),
            (MIDDLE, 31.9, true),
            (MIDDLE, 32.0, false),
            (EAST, 2.9e7, true),
            (EAST, -0.1, false),
        ];
        for (area, x, stays) in steps {
            let mut region = joined_in(area, &[1]);
            // Along z an area has no end.
            let output = region.tick(&moves(vec![walk(1, x, -1.0e6)]));
            assert_eq!(region.player_count(), usize::from(stays), "x = {x}");
            assert_eq!(told(&output).len(), usize::from(!stays), "x = {x}");
        }
    }

    #[test]
    fn a_player_who_leaves_in_the_tick_they_step_out_in_is_simply_gone() {
        let mut region = joined_in(MIDDLE, &[1]);
        // Changes are applied before inputs, so there is nobody left to step out.
        let output = region.tick(&TickInputs {
            player_changes: vec![leave(1)],
            inputs: vec![walk(1, 40.5, 0.5)],
            ..TickInputs::default()
        });
        assert_eq!(
            output.events,
            [RegionEvent::EntityRemoved {
                entity: EntityId(1),
                chunk: ChunkPos::new(0, 0),
            }]
        );
        assert!(told(&output).is_empty());
        assert_eq!(region.player_count(), 0);
    }

    #[test]
    fn an_arriving_player_carries_on_as_they_were_handed_over() {
        let mut region = fresh(
            config(MIDDLE),
            EntityIds {
                first: EntityId(1),
                end: EntityId(3),
            },
        );
        let transfer = transfer(1, 77, 20.5, 40);
        let output = region.tick(&changes(vec![arrive(1, &transfer)]));
        let arrived = EntityState {
            pose: transfer.pose,
            ..state(1, 77, SPAWN)
        };
        assert_eq!(output.events, [RegionEvent::EntitySpawned(arrived.clone())]);
        // The player is in the world already and is not told that they entered it.
        assert!(told(&output).is_empty());
        assert_eq!(
            region.player(player(1)),
            Some((EntityId(77), transfer.pose))
        );
        assert_eq!(region.entity(EntityId(77)), Some(arrived));

        // The entity id came with the player: both of the region's own are still to be had.
        let output = region.tick(&changes(vec![join(2), join(3)]));
        assert_eq!(
            output.events,
            [
                RegionEvent::EntitySpawned(state(2, 1, SPAWN)),
                RegionEvent::EntitySpawned(state(3, 2, SPAWN)),
            ]
        );

        // What the region the player came from had applied is not applied again.
        let output = region.tick(&moves(vec![
            with_number(39, walk(1, 3.5, 0.5)),
            with_number(40, select(1, 2)),
        ]));
        assert!(output.events.is_empty(), "{:?}", output.events);
        // What comes after that is. As it is a step out of the area, it shows that the
        // region kept everything else as it was handed over.
        let output = region.tick(&moves(vec![with_number(41, walk(1, 33.5, 0.5))]));
        let handed_on = PlayerTransfer {
            pose: Pose {
                position: Vec3::new(33.5, -60.0, 0.5),
                ..transfer.pose
            },
            last_input: 41,
            ..transfer
        };
        assert_eq!(told(&output), [departed(1, handed_on)]);
    }

    #[test]
    fn a_player_who_is_already_there_does_not_arrive_again() {
        let mut region = joined_in(EAST, &[1]);
        let mut untouched = region.clone();

        // With the entity the player has here, the handover is one that came twice.
        let output = region.tick(&changes(vec![arrive(1, &transfer(1, 1, 20.5, 40))]));
        assert!(output.events.is_empty(), "{:?}", output.events);
        assert!(told(&output).is_empty());
        untouched.tick(&TickInputs::default());
        assert_eq!(region, untouched);

        // With another entity, that one was on its way and has nowhere to go now. It is
        // removed where it was seen last, be that inside the area or outside.
        for (x, chunk) in [(40.5, 2), (-40.5, -3)] {
            let output = region.tick(&changes(vec![arrive(1, &transfer(1, 50, x, 40))]));
            assert_eq!(
                output.events,
                [RegionEvent::EntityRemoved {
                    entity: EntityId(50),
                    chunk: ChunkPos::new(chunk, 0),
                }]
            );
            assert!(told(&output).is_empty());
            untouched.tick(&TickInputs::default());
            assert_eq!(region, untouched);
        }
    }

    #[test]
    fn a_player_who_joins_outside_the_area_is_let_go_at_once() {
        // The spawn point lies in chunk 0, which this region does not have.
        let mut region = region_in(ChunkArea {
            min_x: Some(1),
            max_x: None,
        });
        let output = region.tick(&changes(vec![join(1)]));
        let spawned = PlayerEvent::Spawned {
            entity_id: EntityId(1),
            position: SPAWN,
            hotbar: hotbar(),
            selected_slot: 0,
        };
        let transfer = PlayerTransfer {
            entity_id: EntityId(1),
            name: "Player1".to_owned(),
            pose: Pose::at(SPAWN),
            hotbar: hotbar(),
            selected_slot: 0,
            last_input: 0,
        };
        assert_eq!(told(&output), [(player(1), spawned), departed(1, transfer)]);
        assert!(!output.events.iter().any(is_removal));
        assert_eq!(region.player_count(), 0);
    }

    #[test]
    fn a_player_who_arrives_outside_the_area_is_passed_on_unchanged() {
        let mut region = region_in(MIDDLE);
        let transfer = transfer(1, 77, -8.5, 40);
        let output = region.tick(&changes(vec![arrive(1, &transfer)]));
        assert_eq!(told(&output), [departed(1, transfer.clone())]);
        assert!(!output.events.iter().any(is_removal));
        assert_eq!(region.player_count(), 0);

        // Not even an input that would bring them into the area keeps them here.
        let output = region.tick(&TickInputs {
            player_changes: vec![arrive(1, &transfer)],
            inputs: vec![with_number(41, walk(1, 8.5, 0.5))],
            ..TickInputs::default()
        });
        assert_eq!(told(&output), [departed(1, transfer)]);
        assert_eq!(region.player_count(), 0);
    }

    #[test]
    fn a_discarded_entity_is_reported_as_removed_and_nobody_is_touched() {
        let mut region = joined_in(MIDDLE, &[1]);
        let mut untouched = region.clone();
        // The second has the id of an entity that is here, and its chunk is not in the
        // area: neither matters.
        let discarded = [
            (EntityId(77), ChunkPos::new(1, -4)),
            (EntityId(1), ChunkPos::new(-3, 9)),
        ];
        let output = region.tick(&changes(
            discarded
                .iter()
                .map(|(entity, chunk)| PlayerChange::Discard {
                    entity: *entity,
                    chunk: *chunk,
                })
                .collect(),
        ));
        assert_eq!(
            output.events,
            discarded.map(|(entity, chunk)| RegionEvent::EntityRemoved { entity, chunk })
        );
        assert!(told(&output).is_empty());
        untouched.tick(&TickInputs::default());
        assert_eq!(region, untouched);
    }

    #[test]
    fn a_region_that_has_used_up_its_entity_ids_refuses_players() {
        let mut region = fresh(
            config(ChunkArea::EVERYWHERE),
            EntityIds {
                first: EntityId(7),
                end: EntityId(9),
            },
        );
        let output = region.tick(&changes(vec![join(1), join(2), join(3)]));
        assert_eq!(
            output.events,
            [
                RegionEvent::EntitySpawned(state(1, 7, SPAWN)),
                RegionEvent::EntitySpawned(state(2, 8, SPAWN)),
            ]
        );
        assert_eq!(told(&output).len(), 3);
        assert_eq!(told(&output)[2], (player(3), PlayerEvent::Refused));
        assert_eq!(region.player_count(), 2);
        assert_eq!(region.player(player(3)), None);

        // Trying again does not help, and what a refused player does is ignored.
        let output = region.tick(&TickInputs {
            player_changes: vec![join(3)],
            inputs: vec![walk(3, 1.5, 0.5)],
            ..TickInputs::default()
        });
        assert_eq!(told(&output), [(player(3), PlayerEvent::Refused)]);
        assert!(output.events.is_empty(), "{:?}", output.events);
        assert_eq!(region.entities().count(), 2);
    }

    /// The chunks that meet at the line between `WEST` and `EAST` where the players of
    /// the tests below are.
    const WEST_CHUNK: ChunkPos = ChunkPos::new(-1, 0);
    const EAST_CHUNK: ChunkPos = ChunkPos::new(0, 0);

    /// Where the feet of a player are who stands east of the line, a block and a half
    /// from it and so with no part of them across it.
    const FEET: Vec3 = Vec3::new(1.5, -60.0, 2.5);

    const FACES: [Face; 6] = [
        Face::Bottom,
        Face::Top,
        Face::North,
        Face::South,
        Face::West,
        Face::East,
    ];

    /// Has `region` load a chunk with a floor at `position`, which its area has to contain.
    fn lay_floor(region: &mut Region, position: ChunkPos) {
        region.tick(&tickets(vec![position], vec![]));
        region.tick(&TickInputs {
            chunks_loaded: vec![(position, floor())],
            ..TickInputs::default()
        });
    }

    /// The region east of the line with player 1 standing on its floor at `FEET`, stone
    /// in hand.
    fn east_with_player() -> Region {
        let mut region = on_floor_in(EAST, &[1]);
        region.tick(&moves(vec![walk(1, FEET.x, FEET.z)]));
        region
    }

    /// The region west of the line with a floor in the chunk at the line, which is as
    /// far as the eastern one's players can reach, and nobody in it.
    fn west_with_floor() -> Region {
        let mut region = fresh(config(WEST), EntityIds::block(1).unwrap());
        lay_floor(&mut region, WEST_CHUNK);
        region
    }

    /// `input`, which has to dig or place, with a sequence number of the test's choosing.
    fn sequenced(chosen: i32, (edge, player, number, mut input): Input) -> Input {
        match &mut input {
            PlayerInput::Dig { sequence, .. } | PlayerInput::UseItemOn { sequence, .. } => {
                *sequence = chosen;
            }
            other => panic!("{other:?} has no sequence number"),
        }
        (edge, player, number, input)
    }

    fn remote(number: u128, sequence: i32, step: RemoteStep) -> RemoteAction {
        RemoteAction {
            player: player(number),
            sequence,
            step,
        }
    }

    fn remotely(remote_actions: Vec<RemoteAction>) -> TickInputs {
        TickInputs {
            remote_actions: from_remote(remote_actions),
            ..TickInputs::default()
        }
    }

    /// `actions` as `REMOTE` passes them on.
    fn from_remote(actions: Vec<RemoteAction>) -> Vec<(EdgeId, RemoteAction)> {
        actions.into_iter().map(|action| (REMOTE, action)).collect()
    }

    fn done(number: u128, sequence: i32) -> RemoteOutcome {
        RemoteOutcome::Done {
            player: player(number),
            sequence,
        }
    }

    fn break_at(x: i32, y: i32, z: i32) -> RemoteStep {
        RemoteStep::Break {
            position: BlockPos::new(x, y, z),
        }
    }

    /// The step that places `block` against `face` of the block at `x`, `y` and `z`.
    fn place_against(
        x: i32,
        y: i32,
        z: i32,
        face: Face,
        block: BlockState,
        placer: Vec3,
    ) -> RemoteStep {
        let against = BlockPos::new(x, y, z);
        RemoteStep::PlaceAgainst {
            against,
            target: face.neighbour(against),
            block,
            placer,
        }
    }

    fn place_at(x: i32, y: i32, z: i32, block: BlockState, placer: Vec3) -> RemoteStep {
        RemoteStep::Place {
            target: BlockPos::new(x, y, z),
            block,
            placer,
        }
    }

    fn is_block_change(event: &RegionEvent) -> bool {
        matches!(event, RegionEvent::BlockChanged { .. })
    }

    #[test]
    fn blocks_outside_the_area_are_dug_and_placed_by_the_region_that_has_them() {
        // A player near the line works on blocks beyond it. The chunk there has been
        // asked for and has been offered, with a floor like the one the player stands on.
        let attempt = |area: ChunkArea| {
            let mut region = on_floor_in(area, &[1]);
            region.tick(&tickets(vec![WEST_CHUNK], vec![]));
            region.tick(&TickInputs {
                chunks_loaded: vec![(WEST_CHUNK, floor())],
                inputs: vec![walk(1, FEET.x, FEET.z)],
                ..TickInputs::default()
            });
            let outputs = [
                // Break the floor right beyond the line.
                dig(1, -1, -61, 2, 1),
                // Put a block on the floor one block further out.
                sequenced(2, place(1, -2, -61, 2, Face::Top)),
                // Fill the hole again, against the side of a block that is in the area.
                sequenced(3, place(1, 0, -61, 2, Face::West)),
            ]
            .map(|input| region.tick(&moves(vec![input])));
            (region, outputs)
        };
        let changes = [
            changed(-1, -61, 2, blocks::AIR),
            changed(-2, -60, 2, blocks::STONE),
            changed(-1, -61, 2, blocks::STONE),
        ];

        // Where one region has the whole world, it does all of this itself.
        let (whole, outputs) = attempt(ChunkArea::EVERYWHERE);
        for (output, sequence) in outputs.iter().zip(1..) {
            assert_eq!(told(output), [acknowledged(1, sequence)]);
            assert!(requests(output).is_empty());
        }
        assert_eq!(
            outputs.map(|output| output.events),
            changes.clone().map(|change| [change])
        );

        // The region east of the line changes nothing and tells the player nothing. It
        // asks for each of the three to be done where the blocks are.
        let (east, outputs) = attempt(EAST);
        let asked = [
            remote(1, 1, break_at(-1, -61, 2)),
            remote(
                1,
                2,
                place_against(-2, -61, 2, Face::Top, blocks::STONE, FEET),
            ),
            remote(1, 3, place_at(-1, -61, 2, blocks::STONE, FEET)),
        ];
        for output in &outputs {
            assert!(output.events.is_empty(), "{:?}", output.events);
            assert!(told(output).is_empty(), "tick {}", output.tick);
        }
        assert_eq!(
            outputs.map(|output| requests(&output)),
            asked.clone().map(|request| [request])
        );
        assert_eq!(east.chunk(WEST_CHUNK), None);
        assert_eq!(east.chunk(EAST_CHUNK), Some(&floor()));

        // The region west of the line does to its blocks what the single region did to
        // them, and says so.
        let mut west = west_with_floor();
        for ((request, change), sequence) in asked.into_iter().zip(changes).zip(1..) {
            let output = west.tick(&remotely(vec![request]));
            assert_eq!(output.events, [change]);
            assert_eq!(outcomes(&output), [done(1, sequence)]);
        }
        assert_eq!(west.chunk(WEST_CHUNK), whole.chunk(WEST_CHUNK));
    }

    #[test]
    fn digging_beyond_the_area_within_reach_is_asked_of_the_region_that_has_the_block() {
        let mut region = east_with_player();
        let beyond = [
            // Right beyond the line.
            (-1, -61),
            // As far beyond it as the player can reach: the block begins 5.5 blocks from
            // their eyes horizontally and about 1.6 below.
            (-5, -61),
            // Thin air, which this region cannot know.
            (-1, -60),
        ];
        for (sequence, (x, y)) in (1..).zip(beyond) {
            let output = region.tick(&moves(vec![dig(1, x, y, 2, sequence)]));
            assert_eq!(requests(&output), [remote(1, sequence, break_at(x, y, 2))]);
            assert!(output.events.is_empty(), "{:?}", output.events);
            // The player is not told that it was handled: it has not been.
            assert!(told(&output).is_empty(), "sequence {sequence}");
        }
        // Not later either, as far as this region is concerned.
        let output = region.tick(&TickInputs::default());
        assert!(told(&output).is_empty());
        assert!(requests(&output).is_empty());
        // The input counts as applied all the same: sent again, as it is to the region
        // a player walks into, it asks for nothing a second time.
        let again = dig(1, -1, -61, 2, 4);
        let output = region.tick(&moves(vec![again.clone()]));
        assert_eq!(requests(&output).len(), 1);
        let output = region.tick(&moves(vec![again]));
        assert!(requests(&output).is_empty());
        assert!(told(&output).is_empty());

        // One block further out is out of reach, which this region can tell.
        let output = region.tick(&moves(vec![dig(1, -6, -61, 2, 5)]));
        assert!(requests(&output).is_empty());
        assert!(output.events.is_empty(), "{:?}", output.events);
        assert_eq!(told(&output), [acknowledged(1, 5)]);

        // And a block of its own it breaks as ever, at the line as anywhere.
        let output = region.tick(&moves(vec![dig(1, 0, -61, 2, 6)]));
        assert!(requests(&output).is_empty());
        assert_eq!(output.events, [changed(0, -61, 2, blocks::AIR)]);
        assert_eq!(told(&output), [acknowledged(1, 6)]);
        assert_eq!(region.loaded_chunk_count(), 1);
    }

    #[test]
    fn placing_against_a_block_beyond_the_area_is_asked_of_the_region_that_has_it() {
        let mut region = east_with_player();
        let clicks = [
            // On the floor beyond the line, where the spot is the other region's too.
            ((-2, -61, 2), Face::Top),
            // Against the side of that floor into a spot of this region. That it is
            // taken does not matter before the other region has found the block.
            ((-1, -61, 2), Face::East),
            // Against thin air, which this region cannot know.
            ((-1, -60, 2), Face::West),
            // As far out as the player can reach: what counts is the block they click,
            // not the spot beyond it.
            ((-5, -61, 2), Face::West),
        ];
        for (sequence, ((x, y, z), face)) in (1..).zip(clicks) {
            let click = sequenced(sequence, place(1, x, y, z, face));
            let output = region.tick(&moves(vec![click]));
            let step = place_against(x, y, z, face, blocks::STONE, FEET);
            assert_eq!(requests(&output), [remote(1, sequence, step)]);
            assert!(output.events.is_empty(), "{:?}", output.events);
            assert!(told(&output).is_empty(), "sequence {sequence}");
        }

        // The block is the one the player holds and the placer is where they stand when
        // they place it.
        let output = region.tick(&moves(vec![
            select(1, 1),
            walk(1, 2.5, 3.5),
            sequenced(5, place(1, -2, -61, 2, Face::North)),
        ]));
        let feet = Vec3::new(2.5, -60.0, 3.5);
        let step = place_against(-2, -61, 2, Face::North, blocks::DIRT, feet);
        assert_eq!(requests(&output), [remote(1, 5, step)]);
        assert!(told(&output).is_empty());
        assert_eq!(region.chunk(EAST_CHUNK), Some(&floor()));
    }

    #[test]
    fn placing_into_a_spot_beyond_the_area_is_asked_of_the_region_that_has_the_spot() {
        let mut region = east_with_player();
        // Against the western side of the floor at the line.
        let click = sequenced(1, place(1, 0, -61, 2, Face::West));
        let output = region.tick(&moves(vec![click]));
        let step = place_at(-1, -61, 2, blocks::STONE, FEET);
        assert_eq!(requests(&output), [remote(1, 1, step)]);
        assert!(output.events.is_empty(), "{:?}", output.events);
        assert!(told(&output).is_empty());

        // Against thin air there is nothing to ask for: this region knows that no block
        // is there.
        let click = sequenced(2, place(1, 0, -60, 2, Face::West));
        let output = region.tick(&moves(vec![click]));
        assert!(requests(&output).is_empty());
        assert!(output.events.is_empty(), "{:?}", output.events);
        assert_eq!(told(&output), [acknowledged(1, 2)]);

        // Nor against a chunk that is not loaded. The player walks to the end of the
        // loaded one and clicks the floor of the next.
        let feet = Vec3::new(1.5, -60.0, 14.5);
        let output = region.tick(&moves(vec![
            walk(1, feet.x, feet.z),
            sequenced(3, place(1, 0, -61, 16, Face::West)),
        ]));
        assert!(requests(&output).is_empty());
        assert_eq!(told(&output), [acknowledged(1, 3)]);
        // One block back there is a floor to place against.
        let click = sequenced(4, place(1, 0, -61, 15, Face::West));
        let output = region.tick(&moves(vec![click]));
        let step = place_at(-1, -61, 15, blocks::STONE, feet);
        assert_eq!(requests(&output), [remote(1, 4, step)]);
        assert!(told(&output).is_empty());
        assert_eq!(region.chunk(EAST_CHUNK), Some(&floor()));
    }

    #[test]
    fn placing_without_a_block_or_out_of_reach_is_acknowledged_whoever_has_the_blocks() {
        let mut region = east_with_player();
        let clicks = [
            // The block clicked and the spot are both beyond the line.
            ((-2, -61, 2), Face::Top),
            // The block clicked is.
            ((-1, -61, 2), Face::East),
            // The spot is.
            ((0, -61, 2), Face::West),
            // Neither is.
            ((2, -61, 2), Face::Top),
        ];
        let attempt = |region: &mut Region, hand: &str| {
            for (sequence, ((x, y, z), face)) in (1..).zip(clicks) {
                let click = sequenced(sequence, place(1, x, y, z, face));
                let output = region.tick(&moves(vec![click]));
                assert!(requests(&output).is_empty(), "{hand}, {sequence}");
                assert!(output.events.is_empty(), "{hand}: {:?}", output.events);
                assert_eq!(told(&output), [acknowledged(1, sequence)], "{hand}");
            }
        };

        // A stick, then an empty slot.
        region.tick(&moves(vec![select(1, 2)]));
        attempt(&mut region, "a stick");
        region.tick(&moves(vec![select(1, 5)]));
        attempt(&mut region, "nothing");
        // Stone, but from eight blocks further east.
        region.tick(&moves(vec![select(1, 0), walk(1, 9.5, 2.5)]));
        attempt(&mut region, "too far");
        assert_eq!(region.chunk(EAST_CHUNK), Some(&floor()));

        // From where the player stood before, the last of these places a block.
        let output = region.tick(&moves(vec![
            walk(1, FEET.x, FEET.z),
            place(1, 2, -61, 2, Face::Top),
        ]));
        assert!(output.events.contains(&changed(2, -60, 2, blocks::STONE)));
    }

    #[test]
    fn what_is_asked_of_another_region_is_not_acknowledged_by_this_one() {
        let mut region = east_with_player();
        // A later action that this region handles itself is acknowledged with its own
        // number. That number is above the one that waits, as numbers go; whoever passes
        // it on to the player has to see to that.
        let output = region.tick(&moves(vec![dig(1, -1, -61, 2, 1), dig(1, 2, -61, 2, 2)]));
        assert_eq!(requests(&output), [remote(1, 1, break_at(-1, -61, 2))]);
        assert_eq!(told(&output), [acknowledged(1, 2)]);

        // An earlier one is acknowledged with its number and no higher one.
        let output = region.tick(&moves(vec![
            dig(1, 2, -61, 3, 3),
            dig(1, -1, -61, 3, 4),
            sequenced(5, place(1, -2, -61, 3, Face::Top)),
            sequenced(6, place(1, 0, -61, 3, Face::West)),
        ]));
        let asked: Vec<_> = requests(&output)
            .iter()
            .map(|action| action.sequence)
            .collect();
        assert_eq!(asked, [4, 5, 6]);
        assert_eq!(told(&output), [acknowledged(1, 3)]);
        assert!(region.tick(&TickInputs::default()).player_events.is_empty());
    }

    #[test]
    fn remote_requests_are_in_the_order_of_the_inputs_that_caused_them() {
        let mut region = on_floor_in(EAST, &[1, 2]);
        let other = Vec3::new(2.5, -60.0, 4.5);
        region.tick(&moves(vec![
            walk(1, FEET.x, FEET.z),
            walk(2, other.x, other.z),
        ]));
        // Neither by player nor by kind: as they were made.
        let output = region.tick(&moves(vec![
            dig(2, -1, -61, 4, 1),
            sequenced(1, place(1, -2, -61, 2, Face::Top)),
            // This one the region handles itself.
            dig(1, 3, -61, 2, 2),
            sequenced(2, place(2, 0, -61, 4, Face::West)),
            dig(1, -1, -61, 2, 3),
        ]));
        assert_eq!(
            requests(&output),
            [
                remote(2, 1, break_at(-1, -61, 4)),
                remote(
                    1,
                    1,
                    place_against(-2, -61, 2, Face::Top, blocks::STONE, FEET)
                ),
                remote(2, 2, place_at(-1, -61, 4, blocks::STONE, other)),
                remote(1, 3, break_at(-1, -61, 2)),
            ]
        );
        assert_eq!(output.events, [changed(3, -61, 2, blocks::AIR)]);
        assert_eq!(told(&output), [acknowledged(1, 2)]);
    }

    #[test]
    fn a_remote_break_removes_the_block_and_is_reported_as_done() {
        let mut region = west_with_floor();
        let output = region.tick(&remotely(vec![remote(1, 7, break_at(-2, -61, 3))]));
        assert_eq!(output.events, [changed(-2, -61, 3, blocks::AIR)]);
        assert_eq!(outcomes(&output), [done(1, 7)]);
        let chunk = region.chunk(WEST_CHUNK).unwrap();
        assert_eq!(chunk.get(14, -61, 3), Some(blocks::AIR));
        assert_eq!(chunk.get(13, -61, 3), Some(blocks::STONE));

        let nothing = [
            // The hole, and the air above the floor.
            break_at(-2, -61, 3),
            break_at(-3, -60, 3),
            // Below and above the world.
            break_at(-2, -65, 3),
            break_at(-2, 320, 3),
            // In a chunk that is not loaded.
            break_at(-20, -61, 3),
            // In another region, whose blocks this one does not have.
            break_at(0, -61, 3),
        ];
        let before = region.chunk(WEST_CHUNK).cloned();
        for (sequence, step) in (8..).zip(nothing) {
            let output = region.tick(&remotely(vec![remote(1, sequence, step.clone())]));
            assert!(output.events.is_empty(), "{step:?}: {:?}", output.events);
            // Done all the same.
            assert_eq!(outcomes(&output), [done(1, sequence)], "{step:?}");
        }
        assert_eq!(region.chunk(WEST_CHUNK), before.as_ref());
        assert_eq!(region.loaded_chunk_count(), 1);
    }

    #[test]
    fn a_remote_placement_needs_a_block_of_this_region_to_place_against() {
        let mut region = west_with_floor();
        // On the floor: there is a block to place against, and the one that is placed is
        // the one that was asked for.
        let step = place_against(-2, -61, 2, Face::Top, blocks::DIRT, FEET);
        let output = region.tick(&remotely(vec![remote(1, 1, step)]));
        assert_eq!(output.events, [changed(-2, -60, 2, blocks::DIRT)]);
        assert_eq!(outcomes(&output), [done(1, 1)]);

        // Each of these would go into a free spot of this region, if only this region
        // had a block to place it against. For the last two the spot is a hole in the
        // floor, at the western end of the loaded chunk and at the line.
        region.tick(&remotely(vec![
            remote(1, 2, break_at(-16, -61, 2)),
            remote(1, 3, break_at(-1, -61, 2)),
        ]));
        let nothing = [
            // Thin air.
            place_against(-3, -59, 2, Face::Bottom, blocks::STONE, FEET),
            // Above the world.
            place_against(-3, 320, 2, Face::Bottom, blocks::STONE, FEET),
            // Where the floor of the next chunk would be, which is not loaded.
            place_against(-17, -61, 2, Face::East, blocks::STONE, FEET),
            // Where the floor of the next region is: that is not for this one to say.
            place_against(0, -61, 2, Face::West, blocks::STONE, FEET),
        ];
        let before = region.chunk(WEST_CHUNK).cloned();
        for (sequence, step) in (4..).zip(nothing) {
            let output = region.tick(&remotely(vec![remote(1, sequence, step.clone())]));
            assert!(output.events.is_empty(), "{step:?}: {:?}", output.events);
            assert_eq!(outcomes(&output), [done(1, sequence)], "{step:?}");
        }
        assert_eq!(region.chunk(WEST_CHUNK), before.as_ref());
    }

    /// The two steps that place stone on the floor at `x` and `z` for a player whose
    /// feet are at `placer`: against the floor, and with the floor found by another
    /// region already.
    fn onto_floor(x: i32, z: i32, placer: Vec3) -> [RemoteStep; 2] {
        [
            place_against(x, -61, z, Face::Top, blocks::STONE, placer),
            place_at(x, -60, z, blocks::STONE, placer),
        ]
    }

    #[test]
    fn a_remote_placement_goes_into_a_free_spot_only() {
        let region = west_with_floor();
        for step in onto_floor(-2, 2, FEET) {
            let mut region = region.clone();
            let output = region.tick(&remotely(vec![remote(1, 1, step.clone())]));
            assert_eq!(output.events, [changed(-2, -60, 2, blocks::STONE)]);
            assert_eq!(outcomes(&output), [done(1, 1)]);

            // A second block does not go where the first is, and does not replace it.
            let again = match step {
                RemoteStep::Place { target, placer, .. } => RemoteStep::Place {
                    target,
                    block: blocks::GLASS,
                    placer,
                },
                _ => place_against(-2, -61, 2, Face::Top, blocks::GLASS, FEET),
            };
            let output = region.tick(&remotely(vec![remote(1, 2, again)]));
            assert!(output.events.is_empty(), "{:?}", output.events);
            assert_eq!(outcomes(&output), [done(1, 2)]);
            let chunk = region.chunk(WEST_CHUNK).unwrap();
            assert_eq!(chunk.get(14, -60, 2), Some(blocks::STONE));
        }
        // Nor does it go into the floor, or anywhere outside the world.
        let taken = [
            place_at(-2, -61, 2, blocks::GLASS, FEET),
            place_against(-2, -61, 2, Face::South, blocks::GLASS, FEET),
            place_at(-2, 320, 2, blocks::GLASS, FEET),
            place_at(-2, -65, 2, blocks::GLASS, FEET),
        ];
        let mut region = region;
        for (sequence, step) in (1..).zip(taken) {
            let output = region.tick(&remotely(vec![remote(1, sequence, step.clone())]));
            assert!(output.events.is_empty(), "{step:?}: {:?}", output.events);
            assert_eq!(outcomes(&output), [done(1, sequence)], "{step:?}");
        }
        assert_eq!(region.chunk(WEST_CHUNK), Some(&floor()));
    }

    #[test]
    fn a_remote_placement_does_not_build_into_a_player_of_the_region() {
        let mut region = west_with_floor();
        // Their feet are at x = -3.5 and z = 0.5, on the floor.
        region.tick(&changes(vec![arrive(2, &transfer(2, 77, -3.5, 0))]));
        for (sequence, step) in (1..).zip(onto_floor(-4, 0, FEET)) {
            let output = region.tick(&remotely(vec![remote(1, sequence, step)]));
            assert!(output.events.is_empty(), "{:?}", output.events);
            assert_eq!(outcomes(&output), [done(1, sequence)]);
        }
        // Not at the height of their head either, but above it.
        let output = region.tick(&remotely(vec![
            remote(1, 3, place_at(-4, -59, 0, blocks::STONE, FEET)),
            remote(1, 4, place_at(-4, -58, 0, blocks::STONE, FEET)),
        ]));
        assert_eq!(output.events, [changed(-4, -58, 0, blocks::STONE)]);
        assert_eq!(outcomes(&output), [done(1, 3), done(1, 4)]);

        // When the player has gone, the spot is free.
        region.tick(&changes(vec![leave(2)]));
        let [step, _] = onto_floor(-4, 0, FEET);
        let output = region.tick(&remotely(vec![remote(1, 5, step)]));
        assert_eq!(output.events, [changed(-4, -60, 0, blocks::STONE)]);
    }

    #[test]
    fn a_remote_placement_does_not_build_into_the_one_who_places() {
        let region = west_with_floor();
        // The spot is the block above the floor right at the line, which a player east
        // of the line reaches into from 0.3 blocks away, being 0.6 wide and 1.8 high.
        let (x, z) = (-1, 2);
        let placers = [
            // Well clear of the line, as in the other tests.
            (FEET, true),
            // With a shoulder across the line, and just not.
            (Vec3::new(0.2, -60.0, 2.5), false),
            (Vec3::new(0.4, -60.0, 2.5), true),
            // The same along the line, from the block to the south.
            (Vec3::new(0.2, -60.0, 3.2), false),
            (Vec3::new(0.2, -60.0, 3.4), true),
            // From below: with the head in the spot, and just under it.
            (Vec3::new(0.2, -61.7, 2.5), false),
            (Vec3::new(0.2, -61.9, 2.5), true),
            // From above: with the feet in the spot, and just over it.
            (Vec3::new(0.2, -59.1, 2.5), false),
            (Vec3::new(0.2, -58.9, 2.5), true),
        ];
        for (placer, free) in placers {
            for step in onto_floor(x, z, placer) {
                let mut region = region.clone();
                let output = region.tick(&remotely(vec![remote(1, 1, step.clone())]));
                let expected = if free {
                    vec![changed(x, -60, z, blocks::STONE)]
                } else {
                    vec![]
                };
                assert_eq!(output.events, expected, "{step:?}");
                // Done either way.
                assert_eq!(outcomes(&output), [done(1, 1)], "{step:?}");
            }
        }
    }

    #[test]
    fn a_remote_placement_against_a_block_of_this_region_into_another_is_passed_on() {
        let mut region = west_with_floor();
        // Against the eastern side of the floor at the line, with a placer whom the
        // next region has to hear of as well.
        let placer = Vec3::new(0.2, -60.0, 2.5);
        let step = place_against(-1, -61, 2, Face::East, blocks::DIRT, placer);
        let output = region.tick(&remotely(vec![remote(3, 41, step)]));
        // This region found the block to place against. The spot is the next one's.
        let next = remote(3, 41, place_at(0, -61, 2, blocks::DIRT, placer));
        assert_eq!(outcomes(&output), [RemoteOutcome::Next(next)]);
        assert!(output.events.is_empty(), "{:?}", output.events);
        assert!(requests(&output).is_empty());
        assert_eq!(region.chunk(WEST_CHUNK), Some(&floor()));

        // Without a block to place against there is nothing to pass on.
        let step = place_against(-1, -60, 2, Face::East, blocks::DIRT, placer);
        let output = region.tick(&remotely(vec![remote(3, 42, step)]));
        assert_eq!(outcomes(&output), [done(3, 42)]);
    }

    /// Two regions that disagree about who has a block must not pass an action back and
    /// forth for ever, so the last step is never passed on.
    #[test]
    fn a_remote_placement_into_a_spot_of_another_region_is_dropped_and_not_passed_on() {
        let mut region = west_with_floor();
        let output = region.tick(&remotely(vec![
            // The air above the floor east of the line, and that floor.
            remote(1, 1, place_at(0, -60, 2, blocks::STONE, FEET)),
            remote(1, 2, place_at(0, -61, 2, blocks::STONE, FEET)),
            // Neither the block to place against nor the spot is this region's.
            remote(
                1,
                3,
                place_against(1, -61, 2, Face::West, blocks::STONE, FEET),
            ),
        ]));
        assert_eq!(outcomes(&output), [done(1, 1), done(1, 2), done(1, 3)]);
        assert!(output.events.is_empty(), "{:?}", output.events);
        assert!(requests(&output).is_empty());
        assert_eq!(region.chunk(WEST_CHUNK), Some(&floor()));
        assert_eq!(region.loaded_chunk_count(), 1);
    }

    #[test]
    fn remote_actions_are_applied_in_order_and_answered_one_for_one() {
        let mut region = west_with_floor();
        let output = region.tick(&remotely(vec![
            // Placed and broken again.
            remote(1, 4, place_at(-2, -60, 2, blocks::STONE, FEET)),
            remote(2, 9, break_at(-2, -60, 2)),
            // Broken, though there is nothing yet, and placed.
            remote(2, 10, break_at(-3, -60, 2)),
            remote(1, 5, place_at(-3, -60, 2, blocks::DIRT, FEET)),
            // Placed against the block that has just been placed.
            remote(
                3,
                1,
                place_against(-3, -60, 2, Face::Top, blocks::GLASS, FEET),
            ),
            // For another region to finish.
            remote(
                1,
                6,
                place_against(-1, -61, 2, Face::East, blocks::STONE, FEET),
            ),
            // Changes nothing.
            remote(2, 11, break_at(-5, -60, 2)),
        ]));
        assert_eq!(
            output.events,
            [
                changed(-2, -60, 2, blocks::STONE),
                changed(-2, -60, 2, blocks::AIR),
                changed(-3, -60, 2, blocks::DIRT),
                changed(-3, -59, 2, blocks::GLASS),
            ]
        );
        assert_eq!(
            outcomes(&output),
            [
                done(1, 4),
                done(2, 9),
                done(2, 10),
                done(1, 5),
                done(3, 1),
                RemoteOutcome::Next(remote(1, 6, place_at(0, -61, 2, blocks::STONE, FEET))),
                done(2, 11),
            ]
        );
        // A tick without remote actions has no outcomes.
        let output = region.tick(&TickInputs::default());
        assert!(outcomes(&output).is_empty());
    }

    #[test]
    fn remote_actions_are_applied_after_player_changes_and_before_inputs() {
        let mut region = west_with_floor();
        // A player who arrives in this tick is in the way already. They stand at
        // x = -3.5 and z = 0.5 and hold glass.
        let output = region.tick(&TickInputs {
            player_changes: vec![arrive(2, &transfer(2, 77, -3.5, 0))],
            remote_actions: vec![(
                REMOTE,
                remote(1, 1, place_at(-4, -60, 0, blocks::STONE, FEET)),
            )],
            ..TickInputs::default()
        });
        assert!(!output.events.iter().any(is_block_change));
        assert_eq!(outcomes(&output), [done(1, 1)]);

        // What the region's own players do in the tick comes after. The player breaks
        // the block that is placed in it,
        let output = region.tick(&TickInputs {
            remote_actions: vec![(
                REMOTE,
                remote(1, 2, place_at(-3, -60, 2, blocks::STONE, FEET)),
            )],
            inputs: vec![dig(2, -3, -60, 2, 1)],
            ..TickInputs::default()
        });
        assert_eq!(
            output.events,
            [
                changed(-3, -60, 2, blocks::STONE),
                changed(-3, -60, 2, blocks::AIR),
            ]
        );
        // finds nothing left to place a block against,
        let output = region.tick(&TickInputs {
            remote_actions: vec![(REMOTE, remote(1, 3, break_at(-3, -61, 2)))],
            inputs: vec![place(2, -3, -61, 2, Face::Top)],
            ..TickInputs::default()
        });
        assert_eq!(output.events, [changed(-3, -61, 2, blocks::AIR)]);
        // and is not yet in the way where they walk to.
        let output = region.tick(&TickInputs {
            remote_actions: vec![(
                REMOTE,
                remote(1, 4, place_at(-6, -60, 4, blocks::STONE, FEET)),
            )],
            inputs: vec![walk(2, -5.5, 4.5)],
            ..TickInputs::default()
        });
        assert!(output.events.contains(&changed(-6, -60, 4, blocks::STONE)));

        // A player who leaves in this tick is in the way no longer.
        let output = region.tick(&TickInputs {
            player_changes: vec![leave(2)],
            remote_actions: vec![(
                REMOTE,
                remote(1, 5, place_at(-6, -59, 4, blocks::STONE, FEET)),
            )],
            ..TickInputs::default()
        });
        assert!(output.events.contains(&changed(-6, -59, 4, blocks::STONE)));
    }

    #[test]
    fn the_player_of_a_remote_action_is_told_nothing_by_the_region() {
        let mut region = west_with_floor();
        // As a rule the player is not in this region.
        let output = region.tick(&remotely(vec![
            remote(1, 7, break_at(-2, -61, 2)),
            remote(1, 8, place_at(-2, -60, 2, blocks::STONE, FEET)),
            remote(
                1,
                9,
                place_against(-1, -61, 2, Face::East, blocks::STONE, FEET),
            ),
        ]));
        assert_eq!(output.events.len(), 2);
        assert!(told(&output).is_empty());

        // They may have walked over while the action was on its way. It is reported as
        // done all the same, and to them only what they do here is acknowledged.
        let output = region.tick(&TickInputs {
            player_changes: vec![arrive(1, &transfer(1, 77, -3.5, 0))],
            remote_actions: vec![(REMOTE, remote(1, 10, break_at(-2, -61, 3)))],
            ..TickInputs::default()
        });
        assert_eq!(outcomes(&output), [done(1, 10)]);
        assert!(told(&output).is_empty());
        let output = region.tick(&TickInputs {
            remote_actions: vec![(REMOTE, remote(1, 12, break_at(-2, -61, 4)))],
            inputs: vec![dig(1, -3, -61, 1, 11)],
            ..TickInputs::default()
        });
        assert_eq!(output.events.len(), 2);
        assert_eq!(outcomes(&output), [done(1, 12)]);
        assert_eq!(told(&output), [acknowledged(1, 11)]);
    }

    /// The entity of a player in `region` and what a handover has to preserve of them.
    fn carried(
        region: &Region,
        id: PlayerId,
    ) -> Option<(EntityId, Pose, [Option<ItemStack>; HOTBAR_SLOTS], u8)> {
        let player = region.players.get(&id)?;
        Some((
            player.entity_id,
            player.pose,
            player.hotbar,
            player.selected_slot,
        ))
    }

    /// What the edge keeps of a player to route them between two regions: it sends their
    /// inputs to the region it believes them to be in, and when that region lets them go
    /// it passes them on to the other one, with every input the first did not apply.
    struct Route {
        /// The region the edge believes the player to be in: 0 lies west of the line.
        region: usize,
        /// How many inputs the player has made.
        made: u64,
        /// Whether the player last walked to the east of the line.
        east: bool,
        /// The inputs sent that no region has reported as applied.
        kept: Vec<Input>,
        /// What a region let the player go with, and the step in which the edge gets
        /// round to passing them on.
        departed: Option<(u64, PlayerTransfer)>,
    }

    impl Route {
        /// Passes the player on if a region has let them go and it is time. Returns the
        /// number of inputs that were sent again.
        fn pass_on(&mut self, id: PlayerId, step: u64, waiting: &mut [TickInputs; 2]) -> usize {
            let Some((_, transfer)) = self.departed.take_if(|(due, _)| *due <= step) else {
                return 0;
            };
            self.region = 1 - self.region;
            self.kept
                .retain(|(_, _, number, _)| *number > transfer.last_input);
            let inputs = &mut waiting[self.region];
            // In the order the region gets them: whatever was sent to it while the
            // player was away came before.
            inputs.change(PlayerChange::Arrive(EDGE, id, transfer));
            inputs.inputs.extend(self.kept.iter().cloned());
            self.kept.len()
        }

        /// Whether one region has let the player go and the other has not taken them in.
        fn under_way(&self, id: PlayerId, waiting: &[TickInputs; 2]) -> bool {
            let arriving = |change: &PlayerChange| match change {
                PlayerChange::Arrive(_, player, _) => Some(*player),
                _ => None,
            };
            self.departed.is_some()
                || waiting
                    .iter()
                    .flat_map(|inputs| &inputs.player_changes)
                    .any(|change| arriving(change) == Some(id))
        }
    }

    /// Walks three players back and forth across the line between two regions, with a
    /// router that does what the edge will do, and compares what becomes of them with a
    /// single region that has the whole world and gets the same inputs in the same order.
    ///
    /// The regions tick in turns. The edge hears of a player being let go right after
    /// the tick or up to two steps later, so that the region the player walked into
    /// ticks without them, and what the player does in the meantime is sent to the
    /// region that no longer has them. The edge passes a player on at the end of a step,
    /// after the tick; `at_any_time` lets it do so before the tick as well, when the
    /// inputs of the step have been sent.
    fn two_regions_match_one(at_any_time: bool) {
        // The players do something for so many steps and are then given time to come to
        // rest in the region their last input took them to.
        const STEPS: u64 = 3000;
        const SETTLING: u64 = 100;
        let numbers = [1, 2, 3];

        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut random = |bound: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % bound
        };

        let mut reference = joined(&numbers);
        // Everyone starts east of the line, where the spawn point is.
        let mut regions = [
            fresh(config(WEST), EntityIds::block(1).unwrap()),
            joined_in(EAST, &numbers),
        ];
        // What the edge has sent to each region since that region's last tick.
        let mut waiting = [TickInputs::default(), TickInputs::default()];
        let mut routes: BTreeMap<PlayerId, Route> = numbers
            .iter()
            .map(|number| {
                let route = Route {
                    region: 1,
                    made: 0,
                    east: true,
                    kept: Vec::new(),
                    departed: None,
                };
                (player(*number), route)
            })
            .collect();
        let (mut compared, mut handovers, mut sent_again, mut ticked_without) = (0, 0, 0, 0);

        for step in 0..STEPS + SETTLING {
            if step < STEPS {
                let mut made = TickInputs::default();
                for _ in 0..random(6) {
                    let id = player(u128::from(1 + random(3)));
                    let route = routes.get_mut(&id).unwrap();
                    let input = match random(8) {
                        0 => PlayerInput::SelectSlot {
                            slot: random(10) as u8,
                        },
                        1 => {
                            // Counts make it unlikely that two stacks are alike.
                            let stack = ItemStack {
                                count: 1 + random(64) as i32,
                                ..[STONE, DIRT, GLASS][random(3) as usize]
                            };
                            PlayerInput::SetHotbarSlot {
                                slot: random(10) as u8,
                                stack: (random(4) != 0).then_some(stack),
                            }
                        }
                        2 => PlayerInput::Move {
                            position: None,
                            rotation: Some((random(360) as f32, random(180) as f32 - 90.0)),
                            on_ground: random(2) == 0,
                        },
                        _ => {
                            // Up to 12 blocks from the line, now and then on its other side.
                            route.east ^= random(4) == 0;
                            let distance = random(120) as f64 / 10.0;
                            let x = if route.east {
                                distance
                            } else {
                                -0.1 - distance
                            };
                            PlayerInput::Move {
                                position: Some(Vec3::new(x, -60.0, random(80) as f64 / 10.0)),
                                rotation: None,
                                on_ground: true,
                            }
                        }
                    };
                    route.made += 1;
                    let input = (EDGE, id, route.made, input);
                    made.inputs.push(input.clone());
                    waiting[route.region].inputs.push(input.clone());
                    route.kept.push(input);
                }
                reference.tick(&made);
            }

            if at_any_time {
                for (id, route) in &mut routes {
                    sent_again += route.pass_on(*id, step, &mut waiting);
                }
            }
            let ticking = (step % 2) as usize;
            ticked_without += routes
                .values()
                .filter(|route| route.departed.is_some() && route.region != ticking)
                .count();
            let output = regions[ticking].tick(&std::mem::take(&mut waiting[ticking]));
            assert!(!output.events.iter().any(is_removal), "step {step}");
            for (id, event) in told(&output) {
                // Nobody joins or digs, so nothing else is to be expected.
                let PlayerEvent::Departed(transfer) = event else {
                    panic!("unexpected event {event:?} in step {step}");
                };
                let route = routes.get_mut(&id).unwrap();
                assert_eq!(route.departed, None, "step {step}");
                route.departed = Some((step + random(3), transfer));
                handovers += 1;
            }
            for (id, route) in &mut routes {
                sent_again += route.pass_on(*id, step, &mut waiting);
            }

            // A player is in one region, or in none while they are being passed on. Once
            // that region has ticked with everything the player did, it has them as the
            // single region has, which is never behind.
            for (id, route) in &routes {
                let holding = regions
                    .iter()
                    .filter(|region| region.player(*id).is_some())
                    .count();
                let under_way = route.under_way(*id, &waiting);
                assert_eq!(holding, usize::from(!under_way), "step {step}");
                let behind = waiting[route.region]
                    .inputs
                    .iter()
                    .any(|(_, player, ..)| player == id);
                if !under_way && !behind {
                    let here = carried(&regions[route.region], *id);
                    assert!(here.is_some(), "step {step}");
                    assert_eq!(here, carried(&reference, *id), "step {step}");
                    compared += 1;
                }
            }
        }

        // By now everything has arrived: the players have come to rest where their last
        // input took them, and are what the single region made of them.
        assert_eq!(waiting, [TickInputs::default(), TickInputs::default()]);
        for (id, route) in &routes {
            assert_eq!(route.departed, None);
            let here = carried(&regions[route.region], *id);
            assert!(here.is_some());
            assert_eq!(here, carried(&reference, *id));
        }
        // The run did something worth comparing.
        assert!(compared > 3000, "{compared} comparisons");
        assert!(handovers > 500, "{handovers} handovers");
        assert!(sent_again > 500, "{sent_again} inputs sent again");
        assert!(ticked_without > 100, "{ticked_without} ticks without");
    }

    #[test]
    fn two_regions_and_a_router_treat_players_as_one_region_does() {
        two_regions_match_one(false);
    }

    /// The harder schedule: the test below shows on its own what it adds.
    #[test]
    fn two_regions_and_a_router_that_acts_at_any_time_treat_players_as_one_region_does() {
        two_regions_match_one(true);
    }

    /// The shortest run in which a handover would lose an input if what a region is sent
    /// were sorted into changes and inputs without regard to its order.
    ///
    /// A region applies the changes of a tick before its inputs, so a player who arrives
    /// is there for inputs that were sent to the region ahead of them: those the edge
    /// sent while it had not heard that the region had let the player go. If the player
    /// went away and came back between two ticks of the region, such an input would be
    /// applied before the earlier ones that the edge sends again, and those would then
    /// count as applied already. [`TickInputs::change`] drops it instead.
    #[test]
    fn inputs_waiting_where_a_player_returns_to_do_not_overtake_those_sent_again() {
        let made = [
            with_number(1, walk(1, -0.5, 0.5)),
            with_number(2, walk(1, 0.5, 0.5)),
            with_number(3, set_slot(1, 4, Some(GLASS))),
            with_number(4, select(1, 4)),
        ];
        let mut reference = joined(&[1]);
        reference.tick(&moves(made.to_vec()));

        let mut west = fresh(config(WEST), EntityIds::block(1).unwrap());
        let mut east = joined_in(EAST, &[1]);
        let handed_over = |output: TickOutput| match told(&output).as_slice() {
            [(_, PlayerEvent::Departed(transfer))] => transfer.clone(),
            other => panic!("unexpected events {other:?}"),
        };

        // The first three inputs reach the eastern region in one tick. It lets the player
        // go at the first and leaves the other two to the western one.
        let first = handed_over(east.tick(&moves(made[..3].to_vec())));
        assert_eq!(first.last_input, 1);
        // The edge has not heard of that when the fourth input comes and sends it east
        // as well. Then it hears, and passes the player on with all that the eastern
        // region did not apply. In the west the player turns round at once.
        let mut for_west = TickInputs::default();
        for_west.change(arrive(1, &first));
        for (edge, player, number, input) in made[1..].iter().cloned() {
            for_west.input(edge, player, number, input);
        }
        let second = handed_over(west.tick(&for_west));
        assert_eq!(second.last_input, 2);
        // The eastern region has not ticked in the meantime, so the fourth input is
        // still waiting there when the third and the fourth are sent to it again.
        let mut for_east = TickInputs::default();
        let (edge_4, player_4, number_4, input_4) = made[3].clone();
        for_east.input(edge_4, player_4, number_4, input_4);
        for_east.change(arrive(1, &second));
        for (edge, player, number, input) in made[2..].iter().cloned() {
            for_east.input(edge, player, number, input);
        }
        assert_eq!(for_east.inputs, made[2..]);
        east.tick(&for_east);
        assert_eq!(carried(&east, player(1)), carried(&reference, player(1)));
    }

    /// A world divided at the line between `WEST` and `EAST` into two regions, with a
    /// router that does what the edge does, beside a single region that has all of it
    /// and is given the same inputs.
    ///
    /// The router sends each input to the region it believes the player to be in and
    /// hands players over as in `two_regions_match_one`. What a region asks of another
    /// or passes on, the router gives to the region that has the block concerned for
    /// its next tick, so that an action takes a tick longer for each region it goes to.
    struct Divided {
        whole: Region,
        /// West of the line and east of it.
        regions: [Region; 2],
        /// What the router has sent to each region since that region's last tick.
        waiting: [TickInputs; 2],
        routes: BTreeMap<PlayerId, Route>,
        step: u64,
        /// How many steps the router takes to hear that a region has let a player go.
        lag: u64,
        handovers: usize,
        /// The sequence numbers the players gave to what they did to blocks.
        made: Vec<(PlayerId, i32)>,
        /// Those that the region the player was in acknowledged, and those that
        /// another region reported as done.
        acknowledged: Vec<(PlayerId, i32)>,
        done: Vec<(PlayerId, i32)>,
        /// What a player's region asked of the other one, and what a region passed on.
        asked: Vec<RemoteAction>,
        passed_on: Vec<RemoteAction>,
        /// The changes of blocks in the single region and in the two, and how many of
        /// the latter came of what a region was given by the other one.
        whole_changes: Vec<RegionEvent>,
        changes: Vec<RegionEvent>,
        changed_for_others: usize,
    }

    impl Divided {
        /// Both worlds with a floor on either side of the line and the given players at
        /// the spawn point, which is east of it.
        fn new(numbers: &[u128]) -> Self {
            let mut whole = on_floor(numbers);
            lay_floor(&mut whole, WEST_CHUNK);
            let routes = numbers
                .iter()
                .map(|number| {
                    let route = Route {
                        region: 1,
                        made: 0,
                        east: true,
                        kept: Vec::new(),
                        departed: None,
                    };
                    (player(*number), route)
                })
                .collect();
            Self {
                whole,
                regions: [west_with_floor(), on_floor_in(EAST, numbers)],
                waiting: [TickInputs::default(), TickInputs::default()],
                routes,
                step: 0,
                lag: 0,
                handovers: 0,
                made: Vec::new(),
                acknowledged: Vec::new(),
                done: Vec::new(),
                asked: Vec::new(),
                passed_on: Vec::new(),
                whole_changes: Vec::new(),
                changes: Vec::new(),
                changed_for_others: 0,
            }
        }

        /// The index of the region that has the block at `position`.
        fn region_of(&self, position: BlockPos) -> usize {
            let has = |region: &Region| region.area().contains(position.chunk());
            self.regions.iter().position(has).unwrap()
        }

        /// The block at `position` in the divided world.
        fn block(&self, position: BlockPos) -> Option<BlockState> {
            self.regions[self.region_of(position)].block(position)
        }

        /// One step: the players make `made`, in this order, and every region ticks
        /// once. The router numbers the inputs itself.
        fn step(&mut self, made: Vec<Input>) {
            let step = self.step;
            let mut whole = TickInputs::default();
            for (_, id, _, input) in made {
                if let PlayerInput::Dig { sequence, .. } | PlayerInput::UseItemOn { sequence, .. } =
                    &input
                {
                    self.made.push((id, *sequence));
                }
                let route = self.routes.get_mut(&id).unwrap();
                route.made += 1;
                let input = (EDGE, id, route.made, input);
                whole.inputs.push(input.clone());
                self.waiting[route.region].inputs.push(input.clone());
                route.kept.push(input);
            }
            let output = self.whole.tick(&whole);
            // Having all the blocks, the single region asks nobody for anything.
            assert!(requests(&output).is_empty(), "step {step}");
            let changes = output.events.into_iter().filter(is_block_change);
            self.whole_changes.extend(changes);

            // Both regions tick with what they were sent before this step, so that what
            // one of them leaves to the other is taken up in the other's next tick.
            let given = std::mem::take(&mut self.waiting);
            let outputs = [0, 1].map(|index| self.regions[index].tick(&given[index]));
            for (from, (output, given)) in outputs.into_iter().zip(given).enumerate() {
                assert_eq!(
                    outcomes(&output).len(),
                    given.remote_actions.len(),
                    "step {step}"
                );
                let changes: Vec<_> = output
                    .events
                    .iter()
                    .filter(|event| is_block_change(event))
                    .cloned()
                    .collect();
                if given.inputs.is_empty() {
                    self.changed_for_others += changes.len();
                }
                self.changes.extend(changes);
                for (id, event) in told(&output) {
                    match event {
                        PlayerEvent::Acknowledged { sequence } => {
                            self.acknowledged.push((id, sequence));
                        }
                        PlayerEvent::Departed(transfer) => {
                            let route = self.routes.get_mut(&id).unwrap();
                            assert_eq!(route.departed, None, "step {step}");
                            route.departed = Some((step + self.lag, transfer));
                            self.handovers += 1;
                        }
                        other => panic!("unexpected event {other:?} in step {step}"),
                    }
                }
                // In the order the region got to them: what it was given by another
                // region comes before what its own players did.
                let mut onward = Vec::new();
                for outcome in outcomes(&output) {
                    match outcome {
                        RemoteOutcome::Done { player, sequence } => {
                            self.done.push((player, sequence));
                        }
                        RemoteOutcome::Next(action) => {
                            self.passed_on.push(action.clone());
                            onward.push(action);
                        }
                    }
                }
                self.asked.extend(requests(&output).iter().cloned());
                onward.extend(requests(&output));
                for action in onward {
                    let to = self.region_of(action.step.concerns());
                    // What concerns its own blocks a region has to do itself.
                    assert_ne!(to, from, "step {step}: {action:?}");
                    self.waiting[to].remote_actions.push((REMOTE, action));
                }
            }
            for (id, route) in &mut self.routes {
                route.pass_on(*id, step, &mut self.waiting);
            }
            self.step += 1;
        }

        /// Whether nothing is on its way between the regions: no player and no action.
        fn at_rest(&self) -> bool {
            self.waiting == [TickInputs::default(), TickInputs::default()]
                && self.routes.values().all(|route| route.departed.is_none())
        }

        /// Lets steps pass until nothing is on its way, so that everything that was
        /// done so far has finished.
        fn settle(&mut self) {
            // A player is taken in a step after the router has heard that they were let
            // go, which takes `lag` steps. An action goes to the other region and back
            // at most, a step each. Whatever takes longer goes round in circles.
            for _ in 0..8 {
                if self.at_rest() {
                    return;
                }
                self.step(vec![]);
            }
            panic!("not at rest in step {}: {:?}", self.step, self.waiting);
        }

        /// A step in which the players make `made`, with time for it to finish, after
        /// which the two regions have to agree with the single one.
        fn act(&mut self, made: Vec<Input>) {
            self.step(made);
            self.settle();
            self.assert_agreement();
        }

        /// The blocks that the region that has them does not have as the single region
        /// has them, with what is there in the single region and in the other.
        fn differences(&self) -> Vec<(BlockPos, Option<BlockState>, Option<BlockState>)> {
            let mut differences = Vec::new();
            for (region, position) in self.regions.iter().zip([WEST_CHUNK, EAST_CHUNK]) {
                if region.chunk(position) == self.whole.chunk(position) {
                    continue;
                }
                let bottom = chunk().min_y();
                for y in bottom..bottom + chunk().height() as i32 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let block = BlockPos::new(position.x * 16 + x, y, position.z * 16 + z);
                            let (whole, part) = (self.whole.block(block), region.block(block));
                            if whole != part {
                                differences.push((block, whole, part));
                            }
                        }
                    }
                }
            }
            differences
        }

        /// Checks that each player is in one of the two regions as they are in the
        /// single region. That holds at rest only.
        fn assert_same_players(&self) {
            let step = self.step;
            for (id, route) in &self.routes {
                let here = carried(&self.regions[route.region], *id);
                assert!(here.is_some(), "step {step}");
                assert_eq!(here, carried(&self.whole, *id), "step {step}");
                let other = &self.regions[1 - route.region];
                assert_eq!(other.player(*id), None, "step {step}");
            }
        }

        /// Checks that the two regions together have the blocks of the single region,
        /// and the players as well. That holds at rest only.
        fn assert_agreement(&self) {
            let differences = self.differences();
            assert!(
                differences.is_empty(),
                "step {}: {differences:?}",
                self.step
            );
            self.assert_same_players();
        }

        /// Checks that everything a player did to a block has been handled exactly
        /// once: its number was either acknowledged by the region the player was in or
        /// reported as done by the region that took the last step.
        ///
        /// An acknowledgement covers every number up to its own. So this holds only
        /// where a player's region acknowledges one action at a time, and not if it
        /// acknowledged a number it has asked the other region about.
        fn assert_each_handled_once(&self) {
            let mut handled = [self.acknowledged.as_slice(), &self.done].concat();
            handled.sort_unstable();
            let mut made = self.made.clone();
            made.sort_unstable();
            assert_eq!(handled, made);
        }
    }

    /// The sequence numbers of what was handled, in the order it was.
    fn numbers(handled: &[(PlayerId, i32)]) -> Vec<i32> {
        handled.iter().map(|(_, sequence)| *sequence).collect()
    }

    /// How many of `actions` break a block, place one against a block that is yet to be
    /// found, and place one into a spot.
    fn kinds<'a>(actions: impl IntoIterator<Item = &'a RemoteAction>) -> [usize; 3] {
        let mut kinds = [0; 3];
        for action in actions {
            let kind = match action.step {
                RemoteStep::Break { .. } => 0,
                RemoteStep::PlaceAgainst { .. } => 1,
                RemoteStep::Place { .. } => 2,
            };
            kinds[kind] += 1;
        }
        kinds
    }

    /// One of the six faces for a number below eight. The two that look across the line
    /// come up twice as often as the others.
    fn across_more_often(number: u64) -> Face {
        FACES[[0, 1, 2, 3, 4, 4, 5, 5][number as usize]]
    }

    #[test]
    fn two_regions_change_blocks_across_the_line_as_one_region_does() {
        let mut world = Divided::new(&[1]);
        let script = [
            // The player stands east of the line, a block and a half from it, and breaks
            // the floor on their own side and beyond the line.
            walk(1, FEET.x, FEET.z),
            dig(1, 2, -61, 2, 1),
            dig(1, -1, -61, 2, 2),
            // They place a block on their own side,
            sequenced(3, place(1, 2, -61, 3, Face::Top)),
            // from their own side into the hole beyond the line,
            sequenced(4, place(1, 0, -61, 2, Face::West)),
            // on the floor beyond the line,
            sequenced(5, place(1, -2, -61, 2, Face::Top)),
            // and from beyond the line into a hole on their own side, for which the
            // action goes there and comes back.
            dig(1, 0, -61, 4, 6),
            sequenced(7, place(1, -1, -61, 4, Face::East)),
            // What comes to nothing: against thin air beyond the line, into a spot
            // that is taken there, and against thin air on their own side.
            sequenced(8, place(1, -2, -59, 2, Face::Top)),
            sequenced(9, place(1, -2, -61, 2, Face::Top)),
            sequenced(10, place(1, 0, -60, 2, Face::West)),
            // Dirt against the block they placed beyond the line, which they then break.
            select(1, 1),
            sequenced(11, place(1, -2, -60, 2, Face::South)),
            dig(1, -2, -60, 2, 12),
            // With a stick in hand, and from too far away.
            select(1, 2),
            sequenced(13, place(1, -3, -61, 2, Face::Top)),
            dig(1, -9, -61, 2, 14),
            // Then they walk across the line and do the like from the west.
            select(1, 0),
            walk(1, -1.5, 6.5),
            dig(1, -3, -61, 6, 15),
            dig(1, 0, -61, 6, 16),
            sequenced(17, place(1, -3, -61, 7, Face::Top)),
            sequenced(18, place(1, -1, -61, 6, Face::East)),
            sequenced(19, place(1, 1, -61, 6, Face::Top)),
            dig(1, -1, -61, 8, 20),
            sequenced(21, place(1, 0, -61, 8, Face::West)),
        ];
        for input in script {
            world.act(vec![input]);
        }

        world.assert_each_handled_once();
        // The player's region handled what was about its own blocks only, and what it
        // could tell to be in vain. The rest was for the other region to finish, or,
        // for the two placements that went there and back, for itself.
        assert_eq!(
            numbers(&world.acknowledged),
            [1, 3, 6, 10, 13, 14, 15, 17, 20]
        );
        assert_eq!(
            numbers(&world.done),
            [2, 4, 5, 7, 8, 9, 11, 12, 16, 18, 19, 21]
        );
        assert_eq!(kinds(&world.asked), [3, 7, 2]);
        let passed_on: Vec<_> = world
            .passed_on
            .iter()
            .map(|action| action.sequence)
            .collect();
        assert_eq!(passed_on, [7, 21]);
        assert_eq!(world.handovers, 1);
        // Block for block the same happened in the same order.
        assert_eq!(world.changes, world.whole_changes);
        assert_eq!(world.changes.len(), 16);
        assert_eq!(world.changed_for_others, 10);
    }

    /// The same for a player who does at random what can be done near the line: digs,
    /// places against any face with a block in hand or without, changes what they hold
    /// and moves about, now and then across the line or to where they stand astride it.
    ///
    /// They do one thing at a time, and the next when all that came of it has finished.
    /// Without that the two worlds differ, which is a known limit: see
    /// `a_placement_by_way_of_another_region_is_overtaken_by_a_dig_of_its_spot`.
    #[test]
    fn two_regions_change_blocks_as_one_region_does_whatever_a_player_does() {
        const ROUNDS: usize = 3000;
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut random = |bound: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % bound
        };

        let mut world = Divided::new(&[1]);
        let mut sequence = 0;
        for _ in 0..ROUNDS {
            // A block within four blocks of the line, more often than not right at it:
            // of the floor for the most part, else right below it or in the two layers
            // above. One row in seven lies north of the loaded chunks. A player clicks
            // what they see, so it is tried a few times to hit a block that is there.
            let mut position = BlockPos::new(0, 0, 0);
            for _ in 0..4 {
                position = BlockPos::new(
                    [-4, -3, -2, -1, -1, 0, 0, 1, 2, 3][random(10) as usize],
                    [-62, -61, -61, -61, -60, -60, -59][random(7) as usize],
                    random(7) as i32 - 1,
                );
                if world.whole.is_block(position) {
                    break;
                }
            }
            let input = match random(10) {
                // Stone or dirt in hand, at times a stick or nothing.
                0 => PlayerInput::SelectSlot {
                    slot: [0, 0, 0, 1, 1, 1, 2, 3][random(8) as usize],
                },
                // Up to three blocks from the line on either side, on the floor or in
                // the air above it.
                1 | 2 => PlayerInput::Move {
                    position: Some(Vec3::new(
                        random(61) as f64 / 10.0 - 3.0,
                        [-60.0, -60.0, -60.0, -59.5, -58.8][random(5) as usize],
                        1.0 + random(40) as f64 / 10.0,
                    )),
                    rotation: None,
                    on_ground: true,
                },
                3 | 4 => {
                    sequence += 1;
                    PlayerInput::Dig { position, sequence }
                }
                // More is placed than dug, or nothing would be left to work on.
                _ => {
                    sequence += 1;
                    PlayerInput::UseItemOn {
                        position,
                        face: across_more_often(random(8)),
                        sequence,
                    }
                }
            };
            // The router hears that the player was let go at once or up to two steps
            // later.
            world.lag = random(3);
            world.act(vec![numbered(player(1), input)]);
        }

        world.assert_each_handled_once();
        assert_eq!(world.changes, world.whole_changes);
        // The run did something worth comparing: blocks changed on the player's side
        // and on the other, by actions of every kind and by such as went there and back,
        // and the player crossed the line.
        let [breaks, against, into] = kinds(&world.asked);
        assert!(breaks > 100, "{breaks} breaks");
        assert!(against > 200, "{against} placements against");
        assert!(into > 10, "{into} placements into");
        let passed_on = world.passed_on.len();
        assert!(passed_on > 10, "{passed_on} passed on");
        assert!(world.changes.len() > 500, "{}", world.changes.len());
        let for_others = world.changed_for_others;
        assert!(for_others > 200, "{for_others} changes for others");
        let acknowledged = world.acknowledged.len();
        assert!(acknowledged > 500, "{acknowledged} acknowledged");
        assert!(world.handovers > 200, "{} handovers", world.handovers);
    }

    /// This documents a limit and not what is wanted: whoever lifts it has to turn this
    /// test round.
    ///
    /// What a player does to blocks of another region takes effect a tick later than in
    /// a single region, and two ticks later if it comes back to the player's own. What
    /// they do right afterwards to the same block can therefore take effect first. The
    /// smallest case is a block placed against one beyond the line into a spot on the
    /// player's own side and broken again at once: divided, the world keeps the block.
    #[test]
    fn a_placement_by_way_of_another_region_is_overtaken_by_a_dig_of_its_spot() {
        let mut world = Divided::new(&[1]);
        let spot = BlockPos::new(0, -61, 2);
        // A hole in the floor on the player's side, right at the line.
        world.act(vec![walk(1, FEET.x, FEET.z), dig(1, 0, -61, 2, 1)]);

        // The player fills it against the side of the floor beyond the line and breaks
        // the block again.
        world.step(vec![
            sequenced(2, place(1, -1, -61, 2, Face::East)),
            dig(1, 0, -61, 2, 3),
        ]);
        // In the single region that is a block placed and broken.
        assert_eq!(
            world.whole_changes[1..],
            [
                changed(0, -61, 2, blocks::STONE),
                changed(0, -61, 2, blocks::AIR),
            ]
        );
        assert_eq!(world.whole.block(spot), Some(blocks::AIR));

        // Divided, the dig comes first and finds nothing to break. The placement is with
        // the western region for a tick, which finds the block to place against,
        assert_eq!(world.block(spot), Some(blocks::AIR));
        assert_eq!(numbers(&world.acknowledged), [1, 3]);
        world.step(vec![]);
        assert_eq!(world.block(spot), Some(blocks::AIR));
        assert!(world.done.is_empty());
        // and comes back to the eastern one, which finds the spot free.
        world.step(vec![]);
        assert_eq!(world.block(spot), Some(blocks::STONE));
        assert_eq!(numbers(&world.done), [2]);
        assert!(world.at_rest());
        world.assert_each_handled_once();
    }

    /// However quickly players act, nothing they do to blocks is lost or handled twice,
    /// though the blocks are then not always those of a single region, as the test
    /// above shows. Two players dig, place and walk back and forth across the line, up
    /// to three times in a tick between them, and the router is slow to hear of it.
    #[test]
    fn nothing_done_to_blocks_is_lost_or_handled_twice_however_quickly_players_act() {
        const STEPS: usize = 3000;
        let mut state = 0xD1B5_4A32_D192_ED03u64;
        let mut random = |bound: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % bound
        };

        let mut world = Divided::new(&[1, 2]);
        let mut sequences = [0; 2];
        for _ in 0..STEPS {
            let mut made = Vec::new();
            for _ in 0..random(4) {
                let index = random(2) as usize;
                // A block within four blocks of the line that is likely to be there.
                let mut position = BlockPos::new(0, 0, 0);
                for _ in 0..4 {
                    position = BlockPos::new(
                        random(8) as i32 - 4,
                        [-61, -61, -60, -59][random(4) as usize],
                        random(6) as i32,
                    );
                    if world.whole.is_block(position) {
                        break;
                    }
                }
                let input = match random(8) {
                    0 => PlayerInput::SelectSlot {
                        slot: random(3) as u8,
                    },
                    // Up to three blocks from the line, on either side of it.
                    1..=3 => PlayerInput::Move {
                        position: Some(Vec3::new(
                            random(61) as f64 / 10.0 - 3.0,
                            -60.0,
                            1.0 + random(40) as f64 / 10.0,
                        )),
                        rotation: None,
                        on_ground: true,
                    },
                    4 => {
                        sequences[index] += 1;
                        PlayerInput::Dig {
                            position,
                            sequence: sequences[index],
                        }
                    }
                    _ => {
                        sequences[index] += 1;
                        PlayerInput::UseItemOn {
                            position,
                            face: across_more_often(random(8)),
                            sequence: sequences[index],
                        }
                    }
                };
                made.push(numbered(player(1 + index as u128), input));
            }
            world.lag = random(3);
            world.step(made);
        }
        // A player may have crossed the line several times since the router last got
        // round to passing them on, so it takes a while for everything to come to rest.
        for _ in 0..100 {
            if !world.at_rest() {
                world.step(vec![]);
            }
        }
        assert!(world.at_rest());
        world.assert_same_players();

        // What a player's region asked of the other one it asked once, and it has been
        // reported as done once.
        let mut asked: Vec<_> = world
            .asked
            .iter()
            .map(|action| (action.player, action.sequence))
            .collect();
        asked.sort_unstable();
        assert!(asked.windows(2).all(|pair| pair[0] < pair[1]));
        let mut done = world.done.clone();
        done.sort_unstable();
        assert_eq!(done, asked);
        // Everything else the region the player was in handled itself. It acknowledged
        // nothing but that, each number once and in order, and the last of it.
        for id in [player(1), player(2)] {
            let of_player = |handled: &[(PlayerId, i32)]| -> Vec<i32> {
                let own = handled.iter().filter(|(player, _)| *player == id);
                own.map(|(_, sequence)| *sequence).collect()
            };
            let mut handled_there = of_player(&world.made);
            handled_there.retain(|sequence| asked.binary_search(&(id, *sequence)).is_err());
            let acknowledged = of_player(&world.acknowledged);
            assert!(acknowledged.windows(2).all(|pair| pair[0] < pair[1]));
            for sequence in &acknowledged {
                assert!(handled_there.contains(sequence), "{sequence}");
            }
            assert_eq!(acknowledged.last(), handled_there.last());
            // The run was quick enough for some acknowledgements to cover more than
            // one action.
            let (covering, covered) = (acknowledged.len(), handled_there.len());
            assert!(covering > 300, "{covering} acknowledgements");
            assert!(covering < covered - 50, "{covering} for {covered}");
        }
        // And it did something worth checking otherwise.
        assert!(asked.len() > 500, "{} asked for", asked.len());
        let passed_on = world.passed_on.len();
        assert!(passed_on > 10, "{passed_on} passed on");
        assert!(world.handovers > 500, "{} handovers", world.handovers);
    }

    /// Two players, one on either side of the line, each of whom works for the most part
    /// on the blocks of the other's side, both in the same tick.
    #[test]
    fn two_regions_serve_players_on_either_side_of_the_line_as_one_region_does() {
        const ROUNDS: i32 = 1500;
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut random = |bound: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % bound
        };

        let mut world = Divided::new(&[1, 2]);
        // Player 1 stays east of the line and player 2 goes west of it. Each clicks
        // blocks of a strip of their own, which reaches from the second block on their
        // side of the line to the third beyond it and is two blocks wide. What they
        // place goes one block further at most, and the strips are three blocks apart:
        // the two never work on the same block, so it does not matter who is first.
        // Neither stands in the other's strip.
        world.act(vec![walk(1, 2.5, 1.5), walk(2, -2.5, 6.5)]);
        for sequence in 1..=ROUNDS {
            let mut made = Vec::new();
            // A player, their strip from west to east with the blocks at the line twice,
            // and its northern row.
            let strips = [
                (1, [-3, -2, -1, -1, 0, 0, 1], 1),
                (2, [-2, -1, -1, 0, 0, 1, 2], 6),
            ];
            for (number, strip, north) in strips {
                // A player clicks what they see, so it is tried a few times to hit a
                // block that is there.
                let mut position = BlockPos::new(0, 0, 0);
                for _ in 0..4 {
                    position = BlockPos::new(
                        strip[random(7) as usize],
                        [-62, -61, -61, -61, -60, -60, -59][random(7) as usize],
                        north + random(2) as i32,
                    );
                    if world.whole.is_block(position) {
                        break;
                    }
                }
                let input = match random(8) {
                    // Stone or dirt in hand, at times a stick.
                    0 => PlayerInput::SelectSlot {
                        slot: [0, 0, 1, 1, 2][random(5) as usize],
                    },
                    1 => PlayerInput::Dig { position, sequence },
                    // Far more is placed than dug, much of it in vain: a strip is
                    // small, and once it is empty there is nothing left to click.
                    _ => PlayerInput::UseItemOn {
                        position,
                        face: across_more_often(random(8)),
                        sequence,
                    },
                };
                made.push(numbered(player(number), input));
            }
            world.act(made);
        }

        world.assert_each_handled_once();
        // The same blocks changed in the same way. Those of one strip changed in the
        // same order too, or the worlds would not have agreed after each round.
        let sorted = |changes: &[RegionEvent]| {
            let mut changes: Vec<_> = changes
                .iter()
                .map(|event| match event {
                    RegionEvent::BlockChanged { position, state } => (*position, *state),
                    other => panic!("unexpected event {other:?}"),
                })
                .collect();
            changes.sort_unstable();
            changes
        };
        assert_eq!(sorted(&world.changes), sorted(&world.whole_changes));
        // The run did something worth comparing: each player had the other's region
        // do things of every kind, and some of them came back.
        for number in [1, 2] {
            let own = |action: &&RemoteAction| action.player == player(number);
            let [breaks, against, into] = kinds(world.asked.iter().filter(own));
            let passed_on = world.passed_on.iter().filter(own).count();
            assert!(breaks > 50, "{breaks} breaks of player {number}");
            assert!(against > 200, "{against} placements against");
            assert!(into > 30, "{into} placements into");
            assert!(passed_on > 30, "{passed_on} passed on");
        }
        assert!(world.changes.len() > 500, "{}", world.changes.len());
        let for_others = world.changed_for_others;
        assert!(for_others > 300, "{for_others} changes for others");
        assert_eq!(world.handovers, 1);
    }

    #[test]
    fn nobody_is_built_into_across_the_line_whom_the_region_of_the_spot_knows_of() {
        let mut world = Divided::new(&[1, 2]);
        let (shoulder, other) = (BlockPos::new(-1, -60, 2), BlockPos::new(-3, -60, 4));
        // Player 1 stands east of the line with a shoulder across it. They cannot place
        // a block into themselves there, as the western region is told where they
        // stand, nor into player 2, who is in the western region.
        world.act(vec![walk(1, 0.2, 2.5), walk(2, -2.5, 4.5)]);
        let attempts = |first: i32| {
            [
                sequenced(first, place(1, -1, -61, 2, Face::Top)),
                sequenced(first + 1, place(1, -3, -61, 4, Face::Top)),
            ]
        };
        for attempt in attempts(1) {
            world.act(vec![attempt]);
        }
        assert_eq!(world.block(shoulder), Some(blocks::AIR));
        assert_eq!(world.block(other), Some(blocks::AIR));
        assert_eq!(numbers(&world.done), [1, 2]);
        assert!(world.changes.is_empty(), "{:?}", world.changes);

        // Nothing else was in the way: when both have stepped aside, the blocks are
        // placed.
        world.act(vec![walk(1, FEET.x, FEET.z), walk(2, -4.5, 6.5)]);
        for attempt in attempts(3) {
            world.act(vec![attempt]);
        }
        assert_eq!(world.block(shoulder), Some(blocks::STONE));
        assert_eq!(world.block(other), Some(blocks::STONE));

        // Nor can player 1 place a block into themselves on their own side by way of
        // the western region: against the block at the line into the spot east of it.
        let own = BlockPos::new(0, -60, 2);
        world.act(vec![walk(1, 0.5, 2.5)]);
        world.act(vec![sequenced(5, place(1, -1, -60, 2, Face::East))]);
        assert_eq!(world.block(own), Some(blocks::AIR));
        world.act(vec![walk(1, FEET.x, FEET.z)]);
        world.act(vec![sequenced(6, place(1, -1, -60, 2, Face::East))]);
        assert_eq!(world.block(own), Some(blocks::STONE));

        assert_eq!(numbers(&world.done), [1, 2, 3, 4, 5, 6]);
        assert_eq!(world.passed_on.len(), 2);
        assert_eq!(world.changes, world.whole_changes);
        assert_eq!(world.changes.len(), 3);
    }

    /// Where the one who places a block stood when they did it goes with the action, so
    /// that it still counts when the action comes back to a region they have left.
    #[test]
    fn a_placement_that_comes_back_does_not_build_into_where_the_placer_stood() {
        let mut world = Divided::new(&[1]);
        let spot = BlockPos::new(0, -60, 2);
        // A block on the floor beyond the line, and the player beside it on their own
        // side of the line.
        world.act(vec![walk(1, FEET.x, FEET.z)]);
        world.act(vec![sequenced(1, place(1, -1, -61, 2, Face::Top))]);
        world.act(vec![walk(1, 0.5, 2.5)]);

        // They place a block against it into the spot they stand in, and in the next
        // tick walk round it and across the line.
        world.step(vec![sequenced(2, place(1, -1, -60, 2, Face::East))]);
        world.step(vec![walk(1, -0.5, 4.5)]);
        world.settle();
        // The eastern region had nobody left to stand in the spot when the placement
        // came back to it.
        assert_eq!(numbers(&world.done), [1, 2]);
        assert_eq!(world.regions[1].player_count(), 0);
        assert_eq!(world.block(spot), Some(blocks::AIR));
        world.assert_agreement();

        // With nobody near the spot, the same click places the block.
        world.act(vec![sequenced(3, place(1, -1, -60, 2, Face::East))]);
        assert_eq!(world.block(spot), Some(blocks::STONE));
        world.assert_each_handled_once();
    }

    /// This documents a limit and not what is wanted: whoever lifts it has to turn this
    /// test round.
    ///
    /// A region knows its own players and, of a player who places a block from another
    /// region, where they stand. It does not know who else stands in another region
    /// close enough to the line to reach across it with a part of their body, and
    /// places blocks into them.
    #[test]
    fn a_player_astride_the_line_is_built_into_by_the_region_they_are_not_in() {
        let mut world = Divided::new(&[1, 2, 3]);
        // Player 1 stands east of the line with a shoulder across it. Player 2 is west
        // of the line and player 3 east of it, both clear of it.
        world.act(vec![
            walk(1, 0.2, 2.5),
            walk(2, -2.5, 2.5),
            walk(3, 2.5, 5.5),
        ]);

        // Player 2 places a block in their own region where that shoulder is.
        let spot = BlockPos::new(-1, -60, 2);
        world.step(vec![place(2, -1, -61, 2, Face::Top)]);
        world.settle();
        assert_eq!(world.whole.block(spot), Some(blocks::AIR));
        assert_eq!(world.block(spot), Some(blocks::STONE));

        // Player 3 does the like from the region player 1 is in, further along the line:
        // the western region is told where the one who places stands, and of nobody else.
        let spot = BlockPos::new(-1, -60, 5);
        world.step(vec![walk(1, 0.2, 5.5)]);
        world.step(vec![place(3, -1, -61, 5, Face::Top)]);
        world.settle();
        assert_eq!(world.whole.block(spot), Some(blocks::AIR));
        assert_eq!(world.block(spot), Some(blocks::STONE));

        assert!(world.whole_changes.is_empty());
        assert_eq!(world.changes.len(), 2);
    }

    /// The same inputs always lead to the same region and the same outputs: a recorded
    /// run can be replayed.
    #[test]
    fn a_recorded_run_replays_identically() {
        // A fixed pseudo-random sequence; any generator with a fixed seed would do.
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut random = |bound: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % bound
        };

        let mut recorded = Vec::new();
        for _ in 0..300 {
            let mut inputs = TickInputs::default();
            for _ in 0..random(4) {
                let number = u128::from(random(6));
                match random(10) {
                    0 => inputs.player_changes.push(join(number)),
                    1 => inputs.player_changes.push(leave(number)),
                    2 => inputs
                        .tickets_added
                        .push(ChunkPos::new(random(4) as i32, 0)),
                    3 => inputs
                        .tickets_removed
                        .push(ChunkPos::new(random(4) as i32, 0)),
                    4 => inputs
                        .chunks_loaded
                        .push((ChunkPos::new(random(4) as i32, 0), floor())),
                    5 => inputs.inputs.push(dig(
                        number,
                        random(24) as i32,
                        -61,
                        random(8) as i32,
                        random(1000) as i32,
                    )),
                    6 => inputs.inputs.push(place(
                        number,
                        random(24) as i32,
                        -61,
                        random(8) as i32,
                        Face::Top,
                    )),
                    7 => inputs.inputs.push(select(number, random(4) as u8)),
                    // Players stay close together so that digging is often in reach.
                    _ => inputs.inputs.push(walk(
                        number,
                        random(200) as f64 / 10.0,
                        random(80) as f64 / 10.0,
                    )),
                }
            }
            recorded.push(inputs);
        }

        let replay = || {
            let mut region = region();
            let outputs: Vec<_> = recorded.iter().map(|inputs| region.tick(inputs)).collect();
            (region, outputs)
        };
        let (region, outputs) = replay();
        assert_eq!(replay(), (region.clone(), outputs.clone()));
        // The run did something worth comparing.
        let moved = |event: &RegionEvent| matches!(event, RegionEvent::EntityMoved { .. });
        assert!(outputs.iter().any(|output| output.events.iter().any(moved)));
        let broken = |event: &RegionEvent| matches!(event, RegionEvent::BlockChanged { .. });
        assert!(
            outputs
                .iter()
                .any(|output| output.events.iter().any(broken))
        );
        assert!(
            outputs
                .iter()
                .any(|output| !output.chunk_requests.is_empty())
        );
        assert_eq!(region.tick_number(), 300);
    }

    /// The same holds for a region that has a part of the world only, with players
    /// arriving from its neighbours and departing to them.
    #[test]
    fn a_recorded_run_of_a_bounded_region_replays_identically() {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut random = |bound: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % bound
        };
        // Inputs are numbered here and not by `numbered`, whose numbers depend on the
        // tests that run at the same time: a handover decides by them what is applied.
        let mut made = 0u64;

        let mut recorded = Vec::new();
        for _ in 0..600 {
            let mut inputs = TickInputs::default();
            for _ in 0..random(4) {
                let number = u128::from(random(6));
                made += 1;
                // Chunks -1 to 3, of which 0 and 1 are in the area.
                let position = ChunkPos::new(random(5) as i32 - 1, 0);
                // Mostly in the area, which reaches from 0 to 32.
                let x = random(400) as f64 / 10.0 - 4.0;
                match random(14) {
                    0 => inputs.player_changes.push(join(number)),
                    1 => inputs.player_changes.push(leave(number)),
                    2 => {
                        // Handed over with a few of the latest inputs still to be applied.
                        let entity = 1000 + random(4) as i32;
                        let last_input = made.saturating_sub(random(4));
                        let transfer = transfer(number, entity, x, last_input);
                        inputs.player_changes.push(arrive(number, &transfer));
                    }
                    3 => inputs.player_changes.push(PlayerChange::Discard {
                        entity: EntityId(2000 + random(4) as i32),
                        chunk: position,
                    }),
                    4 => inputs.tickets_added.push(position),
                    5 => inputs.tickets_removed.push(position),
                    6 => inputs.chunks_loaded.push((position, floor())),
                    7 => {
                        let block = dig(
                            number,
                            random(40) as i32 - 4,
                            -61,
                            random(8) as i32,
                            random(1000) as i32,
                        );
                        inputs.inputs.push(with_number(made, block));
                    }
                    8 => {
                        let block = place(
                            number,
                            random(40) as i32 - 4,
                            -61,
                            random(8) as i32,
                            Face::Top,
                        );
                        inputs.inputs.push(with_number(made, block));
                    }
                    9 => {
                        let slot = select(number, random(9) as u8);
                        inputs.inputs.push(with_number(made, slot));
                    }
                    10 => {
                        let stack = [None, Some(STONE), Some(GLASS)][random(3) as usize];
                        let slot = set_slot(number, random(9) as u8, stack);
                        inputs.inputs.push(with_number(made, slot));
                    }
                    _ => {
                        let step = walk(number, x, random(80) as f64 / 10.0);
                        inputs.inputs.push(with_number(made, step));
                    }
                }
            }
            recorded.push(inputs);
        }

        let replay = || {
            let mut region = region_in(MIDDLE);
            let outputs: Vec<_> = recorded.iter().map(|inputs| region.tick(inputs)).collect();
            (region, outputs)
        };
        let (region, outputs) = replay();
        assert_eq!(replay(), (region.clone(), outputs.clone()));

        // The run did something worth comparing: players who joined and players who
        // arrived were let go, the latter after moving about, and entities were discarded.
        let let_go = |arrived: bool| {
            outputs.iter().flat_map(told).any(|(_, event)| {
                matches!(event, PlayerEvent::Departed(transfer)
                        if (transfer.entity_id.0 >= 1000) == arrived && transfer.last_input > 0)
            })
        };
        assert!(let_go(false));
        assert!(let_go(true));
        let happened = |wanted: fn(&RegionEvent) -> bool| {
            outputs
                .iter()
                .any(|output| output.events.iter().any(wanted))
        };
        assert!(happened(|event| {
            matches!(event, RegionEvent::EntityMoved { entity, .. } if entity.0 >= 1000)
        }));
        assert!(happened(|event| {
            matches!(event, RegionEvent::EntityRemoved { entity, .. } if entity.0 >= 2000)
        }));
        assert!(happened(|event| {
            matches!(event, RegionEvent::BlockChanged { .. })
        }));
        assert!(
            outputs
                .iter()
                .any(|output| !output.chunk_requests.is_empty())
        );
        assert_eq!(region.tick_number(), 600);
    }

    /// And for a region whose players work on blocks at either end of its area and
    /// beyond, and which is given what players of other regions do to blocks there.
    #[test]
    fn a_recorded_run_with_remote_actions_replays_identically() {
        let mut state = 0xD1B5_4A32_D192_ED03u64;
        let mut random = |bound: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % bound
        };
        // Numbered here for the reason given in the test above.
        let mut made = 0u64;

        let mut recorded = Vec::new();
        for _ in 0..1500 {
            let mut inputs = TickInputs::default();
            for _ in 0..random(4) {
                let number = u128::from(random(4));
                made += 1;
                // Chunks -1 to 2, of which 0 and 1 are in the area.
                let position = ChunkPos::new(random(4) as i32 - 1, 0);
                // A block within two blocks of either end of the area, of the floor or
                // at times right above it, and a player who stands within two blocks
                // of it.
                let x = [-2, -1, 0, 1, 30, 31, 32, 33][random(8) as usize];
                let (y, z) = ([-61, -61, -61, -60][random(4) as usize], random(4) as i32);
                let feet = Vec3::new(
                    f64::from(x) + random(40) as f64 / 10.0 - 2.0,
                    -60.0,
                    random(40) as f64 / 10.0,
                );
                let face = across_more_often(random(8));
                let block = [blocks::STONE, blocks::DIRT, blocks::GLASS][random(3) as usize];
                let sequence = random(1000) as i32;
                match random(20) {
                    0 => inputs.player_changes.push(join(number)),
                    1 => inputs.tickets_added.push(position),
                    2 => inputs.chunks_loaded.push((position, floor())),
                    3..=5 => {
                        let step = walk(number, feet.x, feet.z);
                        inputs.inputs.push(with_number(made, step));
                    }
                    6 | 7 => {
                        let block = dig(number, x, y, z, sequence);
                        inputs.inputs.push(with_number(made, block));
                    }
                    8..=11 => {
                        let block = sequenced(sequence, place(number, x, y, z, face));
                        inputs.inputs.push(with_number(made, block));
                    }
                    12 => {
                        // Stone or dirt in hand, at times a stick.
                        let slot = select(number, [0, 0, 1, 2][random(4) as usize]);
                        inputs.inputs.push(with_number(made, slot));
                    }
                    13 | 14 => {
                        let action = remote(number, sequence, break_at(x, y, z));
                        inputs.remote_actions.push((REMOTE, action));
                    }
                    15..=17 => {
                        let step = place_against(x, y, z, face, block, feet);
                        inputs
                            .remote_actions
                            .push((REMOTE, remote(number, sequence, step)));
                    }
                    _ => {
                        let step = place_at(x, y, z, block, feet);
                        inputs
                            .remote_actions
                            .push((REMOTE, remote(number, sequence, step)));
                    }
                }
            }
            recorded.push(inputs);
        }

        let replay = || {
            let mut region = region_in(MIDDLE);
            let outputs: Vec<_> = recorded.iter().map(|inputs| region.tick(inputs)).collect();
            (region, outputs)
        };
        let (region, outputs) = replay();
        assert_eq!(replay(), (region.clone(), outputs.clone()));

        // The run did something worth comparing. The region asked for actions of every
        // kind to be done elsewhere,
        let asked: Vec<_> = outputs.iter().flat_map(requests).collect();
        let [breaks, against, into] = kinds(&asked);
        assert!(breaks > 0 && against > 0 && into > 0);
        // answered what it was asked for, some of which it passed on,
        let outcomes: Vec<_> = outputs.iter().flat_map(outcomes).collect();
        let given: usize = recorded
            .iter()
            .map(|inputs| inputs.remote_actions.len())
            .sum();
        assert_eq!(outcomes.len(), given);
        let passed_on = outcomes
            .iter()
            .filter(|outcome| matches!(outcome, RemoteOutcome::Next(_)))
            .count();
        assert!(passed_on > 0);
        // and changed blocks in ticks in which none of its own players did anything.
        let for_others = recorded
            .iter()
            .zip(&outputs)
            .filter(|(inputs, _)| inputs.inputs.is_empty())
            .flat_map(|(_, output)| &output.events)
            .filter(|event| is_block_change(event))
            .count();
        assert!(for_others > 0);
        assert_eq!(region.tick_number(), 1500);
    }
}
