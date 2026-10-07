//! The state of a region and its tick.

use std::collections::{BTreeMap, BTreeSet};

use clustine_world::{Chunk, ChunkPos, EntityId, PlayerId, Vec3};

use crate::api::{PlayerEvent, TickInputs, TickOutput};

/// What a region is created with.
#[derive(Debug, Clone, PartialEq)]
pub struct RegionConfig {
    /// Where players enter the world.
    pub spawn: Vec3,
    /// The first entity id the region hands out. Ids must be unique within a world, so
    /// each region gets its own block of them. Clients reject id 0.
    pub first_entity_id: EntityId,
}

#[derive(Debug, Clone, PartialEq)]
struct Player {
    entity_id: EntityId,
    position: Vec3,
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

    /// The entity id and position of `player`, if they are in the region.
    pub fn player(&self, player: PlayerId) -> Option<(EntityId, Vec3)> {
        let player = self.players.get(&player)?;
        Some((player.entity_id, player.position))
    }

    /// Advances the region by one tick.
    pub fn tick(&mut self, inputs: &TickInputs) -> TickOutput {
        self.tick += 1;
        let mut output = TickOutput {
            tick: self.tick,
            ..TickOutput::default()
        };

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

        for player in &inputs.leaves {
            self.players.remove(player);
        }
        for join in &inputs.joins {
            if self.players.contains_key(&join.player) {
                // The edge admits each player once; a second join is its mistake.
                continue;
            }
            let entity_id = self.next_entity_id;
            self.next_entity_id = EntityId(entity_id.0 + 1);
            let position = self.config.spawn;
            self.players.insert(
                join.player,
                Player {
                    entity_id,
                    position,
                },
            );
            output.player_events.push((
                join.player,
                PlayerEvent::Spawned {
                    entity_id,
                    position,
                },
            ));
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use clustine_world::{Biome, PlayerId};

    use super::*;
    use crate::api::PlayerJoin;

    fn region() -> Region {
        Region::new(RegionConfig {
            spawn: Vec3::new(0.5, -60.0, 0.5),
            first_entity_id: EntityId(1),
        })
    }

    fn player(number: u128) -> PlayerId {
        PlayerId(uuid::Uuid::from_u128(number))
    }

    fn join(number: u128) -> PlayerJoin {
        PlayerJoin {
            player: player(number),
            name: format!("Player{number}"),
        }
    }

    fn chunk() -> Chunk {
        let overworld = clustine_data::DIMENSION_TYPES
            .iter()
            .find(|dimension| dimension.name == "minecraft:overworld")
            .unwrap();
        Chunk::empty(overworld, Biome(0))
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
        let output = region.tick(&TickInputs {
            joins: vec![join(1), join(2)],
            ..TickInputs::default()
        });
        let spawn = Vec3::new(0.5, -60.0, 0.5);
        assert_eq!(
            output.player_events,
            [
                (
                    player(1),
                    PlayerEvent::Spawned {
                        entity_id: EntityId(1),
                        position: spawn
                    }
                ),
                (
                    player(2),
                    PlayerEvent::Spawned {
                        entity_id: EntityId(2),
                        position: spawn
                    }
                ),
            ]
        );
        assert_eq!(region.player_count(), 2);
        assert_eq!(region.player(player(2)), Some((EntityId(2), spawn)));
        assert_eq!(region.player(player(3)), None);
    }

    #[test]
    fn entity_ids_are_not_reused_after_a_player_leaves() {
        let mut region = region();
        region.tick(&TickInputs {
            joins: vec![join(1)],
            ..TickInputs::default()
        });
        let output = region.tick(&TickInputs {
            leaves: vec![player(1)],
            joins: vec![join(1)],
            ..TickInputs::default()
        });
        assert!(matches!(
            output.player_events[..],
            [(
                _,
                PlayerEvent::Spawned {
                    entity_id: EntityId(2),
                    ..
                }
            )]
        ));
        assert_eq!(region.player_count(), 1);
    }

    #[test]
    fn a_second_join_of_the_same_player_is_ignored() {
        let mut region = region();
        region.tick(&TickInputs {
            joins: vec![join(1)],
            ..TickInputs::default()
        });
        let output = region.tick(&TickInputs {
            joins: vec![join(1)],
            ..TickInputs::default()
        });
        assert!(output.player_events.is_empty());
        assert_eq!(region.player_count(), 1);
    }

    #[test]
    fn ticketed_chunks_are_requested_once_and_loaded_when_they_arrive() {
        let mut region = region();
        let position = ChunkPos::new(2, -3);

        let output = region.tick(&TickInputs {
            tickets_added: vec![position],
            ..TickInputs::default()
        });
        assert_eq!(output.chunk_requests, [position]);
        assert!(region.chunk(position).is_none());

        // Not requested again while the answer is outstanding.
        assert!(
            region
                .tick(&TickInputs::default())
                .chunk_requests
                .is_empty()
        );

        region.tick(&TickInputs {
            chunks_loaded: vec![(position, chunk())],
            ..TickInputs::default()
        });
        assert_eq!(region.chunk(position), Some(&chunk()));
        assert!(
            region
                .tick(&TickInputs::default())
                .chunk_requests
                .is_empty()
        );
    }

    #[test]
    fn a_chunk_stays_loaded_until_its_last_ticket_is_released() {
        let mut region = region();
        let position = ChunkPos::new(0, 0);
        region.tick(&TickInputs {
            tickets_added: vec![position, position],
            ..TickInputs::default()
        });
        region.tick(&TickInputs {
            chunks_loaded: vec![(position, chunk())],
            ..TickInputs::default()
        });

        region.tick(&TickInputs {
            tickets_removed: vec![position],
            ..TickInputs::default()
        });
        assert_eq!(region.loaded_chunk_count(), 1);

        region.tick(&TickInputs {
            tickets_removed: vec![position],
            ..TickInputs::default()
        });
        assert_eq!(region.loaded_chunk_count(), 0);
    }

    #[test]
    fn a_ticket_moved_within_one_tick_keeps_the_chunk() {
        let mut region = region();
        let position = ChunkPos::new(0, 0);
        region.tick(&TickInputs {
            tickets_added: vec![position],
            ..TickInputs::default()
        });
        region.tick(&TickInputs {
            chunks_loaded: vec![(position, chunk())],
            ..TickInputs::default()
        });

        let output = region.tick(&TickInputs {
            tickets_added: vec![position],
            tickets_removed: vec![position],
            ..TickInputs::default()
        });
        assert!(output.chunk_requests.is_empty());
        assert_eq!(region.loaded_chunk_count(), 1);
    }

    #[test]
    fn a_chunk_that_arrives_after_its_ticket_was_released_is_dropped() {
        let mut region = region();
        let position = ChunkPos::new(0, 0);
        region.tick(&TickInputs {
            tickets_added: vec![position],
            ..TickInputs::default()
        });
        region.tick(&TickInputs {
            tickets_removed: vec![position],
            ..TickInputs::default()
        });
        region.tick(&TickInputs {
            chunks_loaded: vec![(position, chunk())],
            ..TickInputs::default()
        });
        assert_eq!(region.loaded_chunk_count(), 0);

        // Needing it again asks storage again.
        let output = region.tick(&TickInputs {
            tickets_added: vec![position],
            ..TickInputs::default()
        });
        assert_eq!(output.chunk_requests, [position]);
    }

    #[test]
    fn unrequested_chunks_are_ignored() {
        let mut region = region();
        region.tick(&TickInputs {
            chunks_loaded: vec![(ChunkPos::new(9, 9), chunk())],
            ..TickInputs::default()
        });
        assert_eq!(region.loaded_chunk_count(), 0);
    }

    #[test]
    fn equal_inputs_give_equal_regions() {
        let inputs = [
            TickInputs {
                joins: vec![join(1), join(2)],
                tickets_added: vec![ChunkPos::new(0, 0), ChunkPos::new(1, 0)],
                ..TickInputs::default()
            },
            TickInputs {
                chunks_loaded: vec![(ChunkPos::new(1, 0), chunk())],
                leaves: vec![player(1)],
                ..TickInputs::default()
            },
        ];
        let run = || {
            let mut region = region();
            let outputs: Vec<_> = inputs.iter().map(|inputs| region.tick(inputs)).collect();
            (region, outputs)
        };
        assert_eq!(run(), run());
    }
}
