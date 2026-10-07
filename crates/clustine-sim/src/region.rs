//! The state of a region and its tick.

use std::collections::{BTreeMap, BTreeSet};

use clustine_world::{Chunk, ChunkPos, EntityId, PlayerId, Vec3};

use crate::api::{
    EntityKind, EntityState, PlayerChange, PlayerEvent, PlayerInput, Pose, RegionEvent, TickInputs,
    TickOutput,
};

/// What a region is created with.
#[derive(Debug, Clone, PartialEq)]
pub struct RegionConfig {
    /// Where players enter the world.
    pub spawn: Vec3,
    /// The first entity id the region hands out. Ids must be unique within a world, so
    /// each region gets its own block of them. Clients reject id 0.
    pub first_entity_id: EntityId,
}

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
            next_entity_id: config.first_entity_id,
            config,
            tick: 0,
            chunks: BTreeMap::new(),
            tickets: BTreeMap::new(),
            requested: BTreeSet::new(),
            players: BTreeMap::new(),
        }
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
                    self.next_entity_id = EntityId(entity_id.0 + 1);
                    let player = Player {
                        entity_id,
                        name: join.name.clone(),
                        pose: Pose::at(self.config.spawn),
                        moved_from: None,
                    };
                    output.player_events.push((
                        join.player,
                        PlayerEvent::Spawned {
                            entity_id,
                            position: player.pose.position,
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
                            chunk: player.entity_state(*id).chunk(),
                        });
                    }
                }
            }
        }

        for (player, input) in &inputs.inputs {
            // Input can arrive for a player who has just left.
            if let Some(player) = self.players.get_mut(player) {
                player.apply(input);
            }
        }
        for player in self.players.values_mut() {
            if let Some(previous_chunk) = player.moved_from.take() {
                output.events.push(RegionEvent::EntityMoved {
                    entity: player.entity_id,
                    pose: player.pose,
                    previous_chunk,
                });
            }
        }

        output
    }

    fn update_chunks(&mut self, inputs: &TickInputs, output: &mut TickOutput) {
        // Additions come first so that a ticket released and taken again within one tick
        // keeps the chunk loaded.
        for position in &inputs.tickets_added {
            *self.tickets.entry(*position).or_default() += 1;
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

    fn apply(&mut self, input: &PlayerInput) {
        match input {
            PlayerInput::Move {
                position,
                rotation,
                on_ground,
            } => {
                // Input that is not a number is dropped as a whole: nothing sensible can
                // be done with it and it must never enter the state.
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
                    pose.yaw = *yaw;
                    pose.pitch = *pitch;
                }
                pose.on_ground = *on_ground;

                if pose != self.pose {
                    let current = self.pose.position;
                    self.moved_from
                        .get_or_insert(ChunkPos::containing(current.x, current.z));
                    self.pose = pose;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use clustine_world::Biome;

    use super::*;
    use crate::api::PlayerJoin;

    const SPAWN: Vec3 = Vec3::new(0.5, -60.0, 0.5);

    fn region() -> Region {
        Region::new(RegionConfig {
            spawn: SPAWN,
            first_entity_id: EntityId(1),
        })
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

    fn walk(number: u128, x: f64, z: f64) -> (PlayerId, PlayerInput) {
        let input = PlayerInput::Move {
            position: Some(Vec3::new(x, -60.0, z)),
            rotation: None,
            on_ground: true,
        };
        (player(number), input)
    }

    fn moves(inputs: Vec<(PlayerId, PlayerInput)>) -> TickInputs {
        TickInputs {
            inputs,
            ..TickInputs::default()
        }
    }

    /// A region that the given players have joined.
    fn joined(numbers: &[u128]) -> Region {
        let mut region = region();
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
        let output = region.tick(&moves(vec![(player(1), turn.clone())]));
        let (_, pose) = region.player(player(1)).unwrap();
        assert_eq!(pose.position, SPAWN);
        assert_eq!((pose.yaw, pose.pitch, pose.on_ground), (90.0, -30.0, true));
        assert_eq!(output.events.len(), 1);

        // Repeating the same input changes nothing, so nothing is reported.
        let output = region.tick(&moves(vec![(player(1), turn)]));
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
        region.tick(&moves(vec![(player(1), spin)]));
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
                match random(6) {
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
                        .push((ChunkPos::new(random(4) as i32, 0), chunk())),
                    _ => inputs.inputs.push(walk(
                        number,
                        random(2000) as f64 / 10.0 - 100.0,
                        random(2000) as f64 / 10.0 - 100.0,
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
        assert!(
            outputs
                .iter()
                .any(|output| !output.chunk_requests.is_empty())
        );
        assert_eq!(region.tick_number(), 300);
    }
}
