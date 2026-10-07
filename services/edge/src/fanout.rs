//! Fan-out: turning what the worker publishes into packets for each player.
//!
//! One task per worker link owns everything the players of this edge share: a replica of
//! the chunks they can see and, per player, what they have been sent. Connections talk
//! to it through [`Command`]s and receive encoded packets through a bounded queue. The
//! task never waits for a client: a client whose queue is full is dropped.

use std::collections::{BTreeMap, BTreeSet};

use bytes::Bytes;
use clustine_data::synced_registry;
use clustine_protocol::nbt::Nbt;
use clustine_protocol::packets::play::{
    ChunkBatchFinished, ChunkBatchStart, Disconnect, GameEvent, Login, PlayerAbilities,
    SetCenterChunk, SynchronizePlayerPosition, game_event, game_mode,
};
use clustine_protocol::packets::{self, Packet};
use clustine_rpc::link::EdgeEnd;
use clustine_rpc::{EdgeToWorker, WorkerToEdge};
use clustine_sim::api::{PlayerEvent, PlayerJoin};
use clustine_world::{Chunk, ChunkPos, EntityId, PlayerId, Vec3};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::encode::chunk_packet;
use crate::login::Profile;

const OVERWORLD: &str = "minecraft:overworld";

/// Player ability flags of creative mode: invulnerable, may fly, breaks blocks instantly.
const CREATIVE_ABILITIES: u8 = 0x01 | 0x04 | 0x08;

/// Chunks in the first batch a client is sent, as in vanilla.
const INITIAL_BATCH_SIZE: usize = 9;
/// The largest batch, whatever the client asks for, as in vanilla.
const MAX_BATCH_SIZE: usize = 64;

/// Identifies one connection, so that a late message about a connection that is gone
/// cannot affect a newer connection of the same player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SessionId(pub(crate) u64);

/// What a connection tells the fan-out task.
#[derive(Debug)]
pub(crate) enum Command {
    /// A player has finished configuration. Encoded packets for them go to `outbound`;
    /// when it is closed, the connection has to end.
    Join {
        session: SessionId,
        profile: Profile,
        /// The view distance the client asked for, if it said.
        requested_view_distance: Option<i32>,
        outbound: mpsc::Sender<Bytes>,
    },
    /// The connection has ended.
    Leave {
        session: SessionId,
        player: PlayerId,
    },
    /// The client has processed a chunk batch and wants this many chunks per tick.
    ChunkBatchReceived {
        session: SessionId,
        player: PlayerId,
        chunks_per_tick: f32,
    },
}

/// Settings of the fan-out task.
#[derive(Debug, Clone)]
pub(crate) struct FanoutConfig {
    pub(crate) max_players: u32,
    /// The largest view distance granted to a client, in chunks.
    pub(crate) view_distance: i32,
}

/// A chunk at least one player of this edge can see.
struct ReplicaChunk {
    /// How many players have the chunk in view.
    viewers: u32,
    /// The chunk's packet, once the worker has sent the chunk.
    packet: Option<Bytes>,
}

/// What one player has been sent and is waiting for.
struct PlayerView {
    session: SessionId,
    name: String,
    outbound: mpsc::Sender<Bytes>,
    view_distance: i32,
    /// The chunks in view. Empty until the worker has placed the player.
    wanted: BTreeSet<ChunkPos>,
    center: ChunkPos,
    /// Chunks in view that are in the replica but have not been sent.
    pending: BTreeSet<ChunkPos>,
    /// Whether a batch has been sent that the client has not confirmed.
    batch_outstanding: bool,
    batch_size: usize,
}

pub(crate) struct Fanout {
    config: FanoutConfig,
    link: EdgeEnd,
    commands: mpsc::Receiver<Command>,
    players: BTreeMap<PlayerId, PlayerView>,
    replica: BTreeMap<ChunkPos, ReplicaChunk>,
}

impl Fanout {
    pub(crate) fn new(
        config: FanoutConfig,
        link: EdgeEnd,
        commands: mpsc::Receiver<Command>,
    ) -> Self {
        Self {
            config,
            link,
            commands,
            players: BTreeMap::new(),
            replica: BTreeMap::new(),
        }
    }

    /// Serves until the worker or the edge is gone. Every player is disconnected when
    /// this returns, because their queues are dropped.
    pub(crate) async fn run(mut self) {
        loop {
            let alive = tokio::select! {
                command = self.commands.recv() => match command {
                    Some(command) => self.handle_command(command).await,
                    None => false,
                },
                message = self.link.recv() => match message {
                    Some(message) => {
                        self.handle_worker(message).await;
                        true
                    }
                    None => {
                        warn!("the worker is gone");
                        false
                    }
                },
            };
            if !alive {
                return;
            }
        }
    }

    /// Returns false if the worker can no longer be reached.
    async fn handle_command(&mut self, command: Command) -> bool {
        match command {
            Command::Join {
                session,
                profile,
                requested_view_distance,
                outbound,
            } => {
                let player = PlayerId(profile.uuid);
                if self.players.contains_key(&player) {
                    refuse(&outbound, "You are already connected to this server.");
                    return true;
                }
                let view_distance = requested_view_distance
                    .unwrap_or(self.config.view_distance)
                    .clamp(2, self.config.view_distance);
                self.players.insert(
                    player,
                    PlayerView {
                        session,
                        name: profile.name.clone(),
                        outbound,
                        view_distance,
                        wanted: BTreeSet::new(),
                        center: ChunkPos::new(0, 0),
                        pending: BTreeSet::new(),
                        batch_outstanding: false,
                        batch_size: INITIAL_BATCH_SIZE,
                    },
                );
                let join = PlayerJoin {
                    player,
                    name: profile.name,
                };
                self.send_to_worker(EdgeToWorker::PlayerJoin(join)).await
            }
            Command::Leave { session, player } => {
                if self.session_matches(player, session) {
                    return self.remove_player(player).await;
                }
                true
            }
            Command::ChunkBatchReceived {
                session,
                player,
                chunks_per_tick,
            } => {
                if self.session_matches(player, session) {
                    let view = self.players.get_mut(&player).expect("session matched");
                    view.batch_outstanding = false;
                    // Also rejects NaN, which compares false with everything.
                    if chunks_per_tick >= 1.0 {
                        view.batch_size = (chunks_per_tick as usize).min(MAX_BATCH_SIZE);
                    } else {
                        view.batch_size = 1;
                    }
                    return self.send_chunks(player).await;
                }
                true
            }
        }
    }

    async fn handle_worker(&mut self, message: WorkerToEdge) {
        match message {
            WorkerToEdge::ToPlayer {
                player,
                event:
                    PlayerEvent::Spawned {
                        entity_id,
                        position,
                    },
            } => self.spawn_player(player, entity_id, position).await,
            WorkerToEdge::ChunkSnapshot {
                position, chunk, ..
            } => self.store_chunk(position, &chunk).await,
        }
    }

    /// Puts a player the worker has placed into the world: the packets that start the
    /// play state, then the chunks around them.
    async fn spawn_player(&mut self, player: PlayerId, entity_id: EntityId, position: Vec3) {
        let Some(view) = self.players.get_mut(&player) else {
            // The player left before the worker answered.
            return;
        };
        let center = ChunkPos::containing(position.x, position.z);
        let entered = [
            encoded(&login_packet(entity_id, &self.config)),
            encoded(&PlayerAbilities {
                flags: CREATIVE_ABILITIES,
                flying_speed: 0.05,
                field_of_view_modifier: 0.1,
            }),
            encoded(&SynchronizePlayerPosition {
                teleport_id: 1,
                x: position.x,
                y: position.y,
                z: position.z,
                velocity_x: 0.0,
                velocity_y: 0.0,
                velocity_z: 0.0,
                yaw: 0.0,
                pitch: 0.0,
                relative_flags: 0,
            }),
            encoded(&GameEvent {
                event: game_event::START_WAITING_FOR_LEVEL_CHUNKS,
                value: 0.0,
            }),
            encoded(&SetCenterChunk {
                chunk_x: center.x,
                chunk_z: center.z,
            }),
        ];
        info!(name = %view.name, entity_id = entity_id.0, "player joined");

        view.center = center;
        view.wanted = square(center, view.view_distance);
        let mut subscribe = Vec::new();
        for position in &view.wanted {
            let chunk = self.replica.entry(*position).or_insert_with(|| {
                subscribe.push(*position);
                ReplicaChunk {
                    viewers: 0,
                    packet: None,
                }
            });
            chunk.viewers += 1;
            if chunk.packet.is_some() {
                view.pending.insert(*position);
            }
        }

        if !self.send_to_player(player, entered).await {
            return;
        }
        if !subscribe.is_empty() {
            self.send_to_worker(EdgeToWorker::Subscribe { chunks: subscribe })
                .await;
        }
        self.send_chunks(player).await;
    }

    /// Takes a chunk the worker sent into the replica and offers it to its viewers.
    async fn store_chunk(&mut self, position: ChunkPos, chunk: &Chunk) {
        let Some(entry) = self.replica.get_mut(&position) else {
            // Nobody has the chunk in view any more.
            return;
        };
        entry.packet = Some(encoded(&chunk_packet(position, chunk)));

        let viewers: Vec<_> = self
            .players
            .iter_mut()
            .filter(|(_, view)| view.wanted.contains(&position))
            .map(|(player, view)| {
                view.pending.insert(position);
                *player
            })
            .collect();
        for player in viewers {
            self.send_chunks(player).await;
        }
    }

    /// Sends the next batch of pending chunks, nearest first, unless the client still
    /// has to confirm the previous batch. Returns false if the worker is unreachable.
    async fn send_chunks(&mut self, player: PlayerId) -> bool {
        let Some(view) = self.players.get_mut(&player) else {
            return true;
        };
        if view.batch_outstanding || view.pending.is_empty() {
            return true;
        }
        let center = view.center;
        let mut batch: Vec<_> = view.pending.iter().copied().collect();
        batch.sort_by_key(|position| {
            let (dx, dz) = (
                i64::from(position.x - center.x),
                i64::from(position.z - center.z),
            );
            dx * dx + dz * dz
        });
        batch.truncate(view.batch_size);

        let mut packets = vec![encoded(&ChunkBatchStart)];
        for position in &batch {
            view.pending.remove(position);
            let packet = self
                .replica
                .get(position)
                .and_then(|chunk| chunk.packet.clone());
            packets.push(packet.expect("pending chunks are in the replica"));
        }
        packets.push(encoded(&ChunkBatchFinished {
            batch_size: batch.len() as i32,
        }));
        view.batch_outstanding = true;
        self.send_to_player(player, packets).await
    }

    /// Queues packets for a player. A player whose queue is full or closed is removed.
    /// Returns false if the player was removed.
    async fn send_to_player(
        &mut self,
        player: PlayerId,
        packets: impl IntoIterator<Item = Bytes>,
    ) -> bool {
        let Some(view) = self.players.get(&player) else {
            return false;
        };
        for packet in packets {
            if let Err(error) = view.outbound.try_send(packet) {
                debug!(name = %view.name, %error, "dropping a player that does not keep up");
                self.remove_player(player).await;
                return false;
            }
        }
        true
    }

    /// Forgets a player and releases what was held for them. Dropping their queue ends
    /// their connection. Returns false if the worker can no longer be reached.
    async fn remove_player(&mut self, player: PlayerId) -> bool {
        let Some(view) = self.players.remove(&player) else {
            return true;
        };
        info!(name = %view.name, "player left");
        let mut unsubscribe = Vec::new();
        for position in &view.wanted {
            let chunk = self
                .replica
                .get_mut(position)
                .expect("viewed chunks are in the replica");
            chunk.viewers -= 1;
            if chunk.viewers == 0 {
                self.replica.remove(position);
                unsubscribe.push(*position);
            }
        }
        if !unsubscribe.is_empty()
            && !self
                .send_to_worker(EdgeToWorker::Unsubscribe {
                    chunks: unsubscribe,
                })
                .await
        {
            return false;
        }
        self.send_to_worker(EdgeToWorker::PlayerLeave { player })
            .await
    }

    fn session_matches(&self, player: PlayerId, session: SessionId) -> bool {
        self.players
            .get(&player)
            .is_some_and(|view| view.session == session)
    }

    /// Returns false if the worker can no longer be reached.
    async fn send_to_worker(&mut self, message: EdgeToWorker) -> bool {
        self.link.send(message).await.is_ok()
    }
}

fn encoded<P: Packet>(packet: &P) -> Bytes {
    packets::encode(packet).into()
}

/// Tells a client why it cannot join. Its connection ends when `outbound` is dropped.
fn refuse(outbound: &mpsc::Sender<Bytes>, reason: &str) {
    let reason = Nbt::String(reason.to_owned());
    let _ = outbound.try_send(encoded(&Disconnect { reason }));
}

/// The chunks within `radius` chunks of `center` in both directions.
fn square(center: ChunkPos, radius: i32) -> BTreeSet<ChunkPos> {
    (-radius..=radius)
        .flat_map(|dx| {
            (-radius..=radius).map(move |dz| ChunkPos::new(center.x + dx, center.z + dz))
        })
        .collect()
}

fn login_packet(entity_id: EntityId, config: &FanoutConfig) -> Login {
    let dimension_type = synced_registry("minecraft:dimension_type")
        .and_then(|registry| registry.id_of(OVERWORLD))
        .expect("the overworld is a vanilla dimension type");
    Login {
        entity_id: entity_id.0,
        hardcore: false,
        dimension_names: vec![OVERWORLD.to_owned()],
        max_players: config.max_players as i32,
        view_distance: config.view_distance,
        simulation_distance: config.view_distance,
        reduced_debug_info: false,
        enable_respawn_screen: true,
        limited_crafting: false,
        dimension_type,
        dimension_name: OVERWORLD.to_owned(),
        hashed_seed: 0,
        game_mode: game_mode::CREATIVE,
        previous_game_mode: None,
        is_debug: false,
        is_flat: true,
        death_location: None,
        portal_cooldown: 0,
        sea_level: -63,
        online_mode: false,
        enforces_secure_chat: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_covers_the_view_distance_in_both_directions() {
        let chunks = square(ChunkPos::new(10, -3), 2);
        assert_eq!(chunks.len(), 25);
        assert!(chunks.contains(&ChunkPos::new(8, -5)));
        assert!(chunks.contains(&ChunkPos::new(12, -1)));
        assert!(!chunks.contains(&ChunkPos::new(13, -3)));
    }
}
