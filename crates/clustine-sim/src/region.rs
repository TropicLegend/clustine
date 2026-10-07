//! The state of a region and its tick.

use std::collections::{BTreeMap, BTreeSet};

use clustine_data::{BlockState, ITEMS, blocks};
use clustine_world::{BlockPos, Chunk, ChunkArea, ChunkPos, EntityId, EntityIds, PlayerId, Vec3};

use crate::api::{
    EntityKind, EntityState, HOTBAR_SLOTS, ItemStack, PlayerChange, PlayerEvent, PlayerInput,
    PlayerTransfer, Pose, RegionEvent, TickInputs, TickOutput,
};

/// What a region is created with.
#[derive(Debug, Clone, PartialEq)]
pub struct RegionConfig {
    /// Where players enter the world.
    pub spawn: Vec3,
    /// The part of the world the region simulates. Chunks outside it are never loaded,
    /// and a player who steps out of it is let go; see [`PlayerEvent::Departed`].
    pub area: ChunkArea,
    /// The entity ids the region gives to players who enter the world in it. Ids must be
    /// unique within a world, so each region gets its own block of them.
    pub entity_ids: EntityIds,
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
    /// changed since then.
    moved_from: Option<ChunkPos>,
    /// The highest sequence number handled in this tick, to be acknowledged.
    handled_sequence: Option<i32>,
    hotbar: [Option<ItemStack>; HOTBAR_SLOTS],
    /// The hotbar slot whose item the player holds.
    selected_slot: u8,
    /// The number of the last input that was applied.
    last_input: u64,
}

/// A part of the world that is simulated as one unit.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    config: RegionConfig,
    tick: u64,
    next_entity_id: EntityId,
    chunks: BTreeMap<ChunkPos, Chunk>,
    /// How many tickets each needed chunk has.
    tickets: BTreeMap<ChunkPos, u32>,
    /// Needed chunks that storage has been asked for but has not delivered.
    requested: BTreeSet<ChunkPos>,
    players: BTreeMap<PlayerId, Player>,
}

impl Region {
    pub fn new(config: RegionConfig) -> Self {
        Self {
            next_entity_id: config.entity_ids.first,
            config,
            tick: 0,
            chunks: BTreeMap::new(),
            tickets: BTreeMap::new(),
            requested: BTreeSet::new(),
            players: BTreeMap::new(),
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

        for change in &inputs.player_changes {
            match change {
                PlayerChange::Join(join) => {
                    if self.players.contains_key(&join.player) {
                        // The edge admits each player once; a second join is its mistake.
                        continue;
                    }
                    let entity_id = self.next_entity_id;
                    if !self.config.entity_ids.contains(entity_id) {
                        output
                            .player_events
                            .push((join.player, PlayerEvent::Refused));
                        continue;
                    }
                    self.next_entity_id = EntityId(entity_id.0 + 1);
                    let player = Player {
                        entity_id,
                        name: join.name.clone(),
                        pose: Pose::at(self.config.spawn),
                        moved_from: None,
                        handled_sequence: None,
                        hotbar: self.config.starting_hotbar,
                        selected_slot: 0,
                        last_input: 0,
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
                }
                PlayerChange::Leave(id) => {
                    if let Some(player) = self.players.remove(id) {
                        output.events.push(RegionEvent::EntityRemoved {
                            entity: player.entity_id,
                            chunk: player.chunk(),
                        });
                    }
                }
                PlayerChange::Arrive(id, transfer) => {
                    if let Some(present) = self.players.get(id) {
                        // The player is here already. If that is with another entity,
                        // the one that was on its way has nowhere to go.
                        if present.entity_id != transfer.entity_id {
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
                    };
                    // Those watching already show the entity if they saw it cross over;
                    // to them this is nothing new.
                    output
                        .events
                        .push(RegionEvent::EntitySpawned(player.entity_state(*id)));
                    self.players.insert(*id, player);
                }
                PlayerChange::Discard { entity, chunk } => {
                    output.events.push(RegionEvent::EntityRemoved {
                        entity: *entity,
                        chunk: *chunk,
                    });
                }
            }
        }

        for (id, number, input) in &inputs.inputs {
            self.apply_input(*id, *number, input, &mut output);
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
                output
                    .player_events
                    .push((id, PlayerEvent::Departed(player.into_transfer())));
            }
        }

        output
    }

    fn apply_input(
        &mut self,
        id: PlayerId,
        number: u64,
        input: &PlayerInput,
        output: &mut TickOutput,
    ) {
        // Input can arrive for a player who has just left.
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
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
                // Acknowledged whatever comes of it, so the client stops guessing.
                player.acknowledge(*sequence);
                let reachable = player.can_reach(*position);
                if reachable
                    && self
                        .block(*position)
                        .is_some_and(|state| state != blocks::AIR)
                {
                    self.set_block(*position, blocks::AIR, output);
                }
            }
            PlayerInput::UseItemOn {
                position,
                face,
                sequence,
            } => {
                player.acknowledge(*sequence);
                let reachable = player.can_reach(*position);
                // In creative mode placing does not use the item up.
                let block = player.hotbar[usize::from(player.selected_slot)]
                    .and_then(|stack| ITEMS.get(usize::try_from(stack.item).ok()?)?.block);
                let target = face.neighbour(*position);
                if let Some(block) = block
                    && reachable
                    // Placed against a block, into a free spot nobody stands in.
                    && self.block(*position).is_some_and(|state| state != blocks::AIR)
                    && self.block(target) == Some(blocks::AIR)
                    && !self.players.values().any(|player| player.occupies(target))
                {
                    self.set_block(target, block, output);
                }
            }
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

impl Player {
    /// The chunk the player stands in.
    fn chunk(&self) -> ChunkPos {
        ChunkPos::containing(self.pose.position.x, self.pose.position.z)
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
    }

    /// Whether the player's body overlaps the block at `position`.
    fn occupies(&self, position: BlockPos) -> bool {
        let feet = self.pose.position;
        let half = PLAYER_WIDTH / 2.0;
        let overlaps = |low: f64, high: f64, block: i32| {
            low < f64::from(block) + 1.0 && high > f64::from(block)
        };
        overlaps(feet.x - half, feet.x + half, position.x)
            && overlaps(feet.y, feet.y + PLAYER_HEIGHT, position.y)
            && overlaps(feet.z - half, feet.z + half, position.z)
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
            entity_ids: EntityIds::block(0).unwrap(),
            starting_hotbar: hotbar(),
        }
    }

    fn region() -> Region {
        Region::new(config(ChunkArea::EVERYWHERE))
    }

    /// An input as the edge passes it on: numbered in the order the inputs are made.
    type Input = (PlayerId, u64, PlayerInput);

    fn numbered(player: PlayerId, input: PlayerInput) -> Input {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        (player, NEXT.fetch_add(1, Ordering::Relaxed), input)
    }

    /// `input` with a number of the test's choosing in place of the next one.
    fn with_number(number: u64, (player, _, input): Input) -> Input {
        (player, number, input)
    }

    fn player(number: u128) -> PlayerId {
        PlayerId(uuid::Uuid::from_u128(number))
    }

    fn join(number: u128) -> PlayerChange {
        PlayerChange::Join(PlayerJoin {
            player: player(number),
            name: format!("Player{number}"),
        })
    }

    fn leave(number: u128) -> PlayerChange {
        PlayerChange::Leave(player(number))
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
        let mut region = Region::new(config(area));
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
            output.player_events,
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
        assert_eq!(output.player_events.len(), 1);
    }

    #[test]
    fn a_second_join_of_the_same_player_is_ignored() {
        let mut region = joined(&[1]);
        let output = region.tick(&changes(vec![join(1)]));
        assert!(output.player_events.is_empty());
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
        assert_eq!(output.player_events, [acknowledged(1, 7)]);

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
            let PlayerInput::Dig { sequence, .. } = attempt.2 else {
                unreachable!();
            };
            let output = region.tick(&moves(vec![attempt]));
            assert!(output.events.is_empty(), "sequence {sequence}");
            assert_eq!(output.player_events, [acknowledged(1, sequence)]);
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
        assert_eq!(
            output.player_events,
            [acknowledged(1, 6), acknowledged(2, 3)]
        );
        // Nothing is acknowledged in a tick without such input.
        assert!(region.tick(&TickInputs::default()).player_events.is_empty());
    }

    #[test]
    fn a_block_cannot_be_broken_twice() {
        let mut region = on_floor(&[1, 2]);
        let output = region.tick(&moves(vec![dig(1, 0, -61, 0, 1), dig(2, 0, -61, 0, 1)]));
        assert_eq!(output.events.len(), 1);
        assert_eq!(output.player_events.len(), 2);
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
        assert_eq!(output.player_events, [acknowledged(1, 1)]);

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
            assert_eq!(
                output.player_events,
                [acknowledged(1, 1)],
                "attempt {index}"
            );
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
            assert_eq!(output.player_events, [acknowledged(1, 1)]);
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
        PlayerChange::Arrive(player(number), transfer.clone())
    }

    fn departed(number: u128, transfer: PlayerTransfer) -> (PlayerId, PlayerEvent) {
        (player(number), PlayerEvent::Departed(transfer))
    }

    fn is_removal(event: &RegionEvent) -> bool {
        matches!(event, RegionEvent::EntityRemoved { .. })
    }

    #[test]
    fn chunks_outside_the_area_are_neither_requested_nor_loaded() {
        let mut region = Region::new(config(MIDDLE));
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
        assert!(output.player_events.is_empty());
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
            output.player_events,
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
        assert!(output.player_events.is_empty());
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
            assert_eq!(output.player_events.len(), usize::from(!stays), "x = {x}");
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
        assert!(output.player_events.is_empty());
        assert_eq!(region.player_count(), 0);
    }

    #[test]
    fn an_arriving_player_carries_on_as_they_were_handed_over() {
        let mut region = Region::new(RegionConfig {
            entity_ids: EntityIds {
                first: EntityId(1),
                end: EntityId(3),
            },
            ..config(MIDDLE)
        });
        let transfer = transfer(1, 77, 20.5, 40);
        let output = region.tick(&changes(vec![arrive(1, &transfer)]));
        let arrived = EntityState {
            pose: transfer.pose,
            ..state(1, 77, SPAWN)
        };
        assert_eq!(output.events, [RegionEvent::EntitySpawned(arrived.clone())]);
        // The player is in the world already and is not told that they entered it.
        assert!(output.player_events.is_empty());
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
        assert_eq!(output.player_events, [departed(1, handed_on)]);
    }

    #[test]
    fn a_player_who_is_already_there_does_not_arrive_again() {
        let mut region = joined_in(EAST, &[1]);
        let mut untouched = region.clone();

        // With the entity the player has here, the handover is one that came twice.
        let output = region.tick(&changes(vec![arrive(1, &transfer(1, 1, 20.5, 40))]));
        assert!(output.events.is_empty(), "{:?}", output.events);
        assert!(output.player_events.is_empty());
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
            assert!(output.player_events.is_empty());
            untouched.tick(&TickInputs::default());
            assert_eq!(region, untouched);
        }
    }

    #[test]
    fn a_player_who_joins_outside_the_area_is_let_go_at_once() {
        // The spawn point lies in chunk 0, which this region does not have.
        let mut region = Region::new(config(ChunkArea {
            min_x: Some(1),
            max_x: None,
        }));
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
        assert_eq!(
            output.player_events,
            [(player(1), spawned), departed(1, transfer)]
        );
        assert!(!output.events.iter().any(is_removal));
        assert_eq!(region.player_count(), 0);
    }

    #[test]
    fn a_player_who_arrives_outside_the_area_is_passed_on_unchanged() {
        let mut region = Region::new(config(MIDDLE));
        let transfer = transfer(1, 77, -8.5, 40);
        let output = region.tick(&changes(vec![arrive(1, &transfer)]));
        assert_eq!(output.player_events, [departed(1, transfer.clone())]);
        assert!(!output.events.iter().any(is_removal));
        assert_eq!(region.player_count(), 0);

        // Not even an input that would bring them into the area keeps them here.
        let output = region.tick(&TickInputs {
            player_changes: vec![arrive(1, &transfer)],
            inputs: vec![with_number(41, walk(1, 8.5, 0.5))],
            ..TickInputs::default()
        });
        assert_eq!(output.player_events, [departed(1, transfer)]);
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
        assert!(output.player_events.is_empty());
        untouched.tick(&TickInputs::default());
        assert_eq!(region, untouched);
    }

    #[test]
    fn a_region_that_has_used_up_its_entity_ids_refuses_players() {
        let mut region = Region::new(RegionConfig {
            entity_ids: EntityIds {
                first: EntityId(7),
                end: EntityId(9),
            },
            ..config(ChunkArea::EVERYWHERE)
        });
        let output = region.tick(&changes(vec![join(1), join(2), join(3)]));
        assert_eq!(
            output.events,
            [
                RegionEvent::EntitySpawned(state(1, 7, SPAWN)),
                RegionEvent::EntitySpawned(state(2, 8, SPAWN)),
            ]
        );
        assert_eq!(output.player_events.len(), 3);
        assert_eq!(output.player_events[2], (player(3), PlayerEvent::Refused));
        assert_eq!(region.player_count(), 2);
        assert_eq!(region.player(player(3)), None);

        // Trying again does not help, and what a refused player does is ignored.
        let output = region.tick(&TickInputs {
            player_changes: vec![join(3)],
            inputs: vec![walk(3, 1.5, 0.5)],
            ..TickInputs::default()
        });
        assert_eq!(output.player_events, [(player(3), PlayerEvent::Refused)]);
        assert!(output.events.is_empty(), "{:?}", output.events);
        assert_eq!(region.entities().count(), 2);
    }

    #[test]
    fn blocks_outside_the_area_can_neither_be_dug_nor_placed() {
        let west = ChunkPos::new(-1, 0);
        // A player near the line works on blocks beyond it. The chunk there has been
        // asked for and has been offered, with a floor like the one the player stands on.
        let attempt = |area: ChunkArea| {
            let mut region = on_floor_in(area, &[1]);
            region.tick(&tickets(vec![west], vec![]));
            region.tick(&TickInputs {
                chunks_loaded: vec![(west, floor())],
                inputs: vec![walk(1, 1.5, 2.5)],
                ..TickInputs::default()
            });
            let outputs = [
                // Break the floor right beyond the line.
                dig(1, -1, -61, 2, 1),
                // Put a block on the floor one block further out.
                place(1, -2, -61, 2, Face::Top),
                // Fill the hole again, against the side of a block that is in the area.
                place(1, 0, -61, 2, Face::West),
            ]
            .map(|input| region.tick(&moves(vec![input])));
            (region, outputs)
        };

        // Where one region has the whole world, all of this works.
        let (_, outputs) = attempt(ChunkArea::EVERYWHERE);
        assert_eq!(
            outputs.map(|output| output.events),
            [
                [changed(-1, -61, 2, blocks::AIR)],
                [changed(-2, -60, 2, blocks::STONE)],
                [changed(-1, -61, 2, blocks::STONE)],
            ]
        );

        let (region, outputs) = attempt(EAST);
        for output in outputs {
            assert!(output.events.is_empty(), "{:?}", output.events);
            assert_eq!(output.player_events, [acknowledged(1, 1)]);
        }
        assert_eq!(region.chunk(west), None);
        assert_eq!(region.chunk(ChunkPos::new(0, 0)), Some(&floor()));
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
                .retain(|(_, number, _)| *number > transfer.last_input);
            let inputs = &mut waiting[self.region];
            inputs
                .player_changes
                .push(PlayerChange::Arrive(id, transfer));
            inputs.inputs.extend(self.kept.iter().cloned());
            self.kept.len()
        }

        /// Whether one region has let the player go and the other has not taken them in.
        fn under_way(&self, id: PlayerId, waiting: &[TickInputs; 2]) -> bool {
            let arriving = |change: &PlayerChange| match change {
                PlayerChange::Arrive(player, _) => Some(*player),
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
            Region::new(RegionConfig {
                entity_ids: EntityIds::block(1).unwrap(),
                ..config(WEST)
            }),
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
                    let input = (id, route.made, input);
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
            for (id, event) in output.player_events {
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
                    .any(|(player, ..)| player == id);
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

    /// Fails for the reason that the test below shows on its own.
    #[test]
    #[ignore = "an input is lost when a player returns to a region that has inputs waiting"]
    fn two_regions_and_a_router_that_acts_at_any_time_treat_players_as_one_region_does() {
        two_regions_match_one(true);
    }

    /// The shortest run in which a handover loses an input. It fails, and is ignored
    /// until it is decided where that is to be mended.
    ///
    /// A region applies the changes of a tick before its inputs, so a player who arrives
    /// is there for inputs that were sent to the region ahead of them: those the edge
    /// sent while it had not heard that the region had let the player go. If the player
    /// went away and came back between two ticks of the region, such an input is applied
    /// before the earlier ones that the edge sends again, and those then count as
    /// applied already.
    #[test]
    #[ignore = "an input is lost when a player returns to a region that has inputs waiting"]
    fn inputs_waiting_where_a_player_returns_to_do_not_overtake_those_sent_again() {
        let made = [
            with_number(1, walk(1, -0.5, 0.5)),
            with_number(2, walk(1, 0.5, 0.5)),
            with_number(3, set_slot(1, 4, Some(GLASS))),
            with_number(4, select(1, 4)),
        ];
        let mut reference = joined(&[1]);
        reference.tick(&moves(made.to_vec()));

        let mut west = Region::new(RegionConfig {
            entity_ids: EntityIds::block(1).unwrap(),
            ..config(WEST)
        });
        let mut east = joined_in(EAST, &[1]);
        let handed_over = |output: TickOutput| match output.player_events.as_slice() {
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
        let second = handed_over(west.tick(&TickInputs {
            player_changes: vec![arrive(1, &first)],
            inputs: made[1..].to_vec(),
            ..TickInputs::default()
        }));
        assert_eq!(second.last_input, 2);
        // The eastern region has not ticked in the meantime, so the fourth input is
        // still waiting there when the third and the fourth are sent to it again.
        east.tick(&TickInputs {
            player_changes: vec![arrive(1, &second)],
            inputs: [&made[3..], &made[2..]].concat(),
            ..TickInputs::default()
        });
        assert_eq!(carried(&east, player(1)), carried(&reference, player(1)));
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
            let mut region = Region::new(config(MIDDLE));
            let outputs: Vec<_> = recorded.iter().map(|inputs| region.tick(inputs)).collect();
            (region, outputs)
        };
        let (region, outputs) = replay();
        assert_eq!(replay(), (region.clone(), outputs.clone()));

        // The run did something worth comparing: players who joined and players who
        // arrived were let go, the latter after moving about, and entities were discarded.
        let let_go = |arrived: bool| {
            outputs
                .iter()
                .flat_map(|output| &output.player_events)
                .any(|(_, event)| {
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
}
