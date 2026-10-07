//! Fan-out: turning what the regions publish into packets for each player.
//!
//! One task owns everything the players of this edge share: a replica of the chunks they
//! can see and, per player, what they have been sent and which region they are in.
//! Connections talk to it through [`Command`]s and receive encoded packets through a
//! bounded queue. The task never waits for a client: a client whose queue is full is
//! dropped.
//!
//! The world is divided into regions, each reached through a link of its own. What a
//! player sees comes from whichever regions the chunks in view belong to; what a player
//! does goes to the region the player is in. When a region lets a player go because they
//! walked out of it, this task hands them to the region they walked into.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

use bytes::Bytes;
use clustine_data::{BlockState, entity_types, synced_registry};
use clustine_protocol::codec::Position;
use clustine_protocol::item as protocol;
use clustine_protocol::nbt::Nbt;
use clustine_protocol::packets::play::{
    AcknowledgeBlockChange, BlockUpdate, ChunkBatchFinished, ChunkBatchStart, Disconnect,
    GameEvent, Login, PlayerAbilities, PlayerInfoEntry, PlayerInfoRemove, PlayerInfoUpdate,
    PositionPath, RemoveEntities, SetCenterChunk, SetContainerContent, SetHeadRotation,
    SetHeldSlot, SpawnEntity, SyncEntityPosition, SynchronizePlayerPosition, UnloadChunk, angle,
    game_event, game_mode, inventory, player_info,
};
use clustine_protocol::packets::{self, Packet};
use clustine_region::{Layout, RegionId};
use clustine_rpc::link;
use clustine_rpc::{EdgeToWorker, WorkerToEdge};
use clustine_sim::api::{
    EntityKind, EntityState, HOTBAR_SLOTS, ItemStack, PlayerEvent, PlayerInput, PlayerJoin,
    PlayerTransfer, RegionEvent,
};
use clustine_world::{BlockPos, Chunk, ChunkPos, EntityId, PlayerId, Vec3};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tracing::{debug, error, info, warn};

use crate::Routing;
use crate::encode::chunk_packet;
use crate::login::Profile;

const OVERWORLD: &str = "minecraft:overworld";

/// Player ability flags of creative mode: invulnerable, may fly, breaks blocks instantly.
const CREATIVE_ABILITIES: u8 = 0x01 | 0x04 | 0x08;

/// Messages from the regions that may wait for the fan-out task.
const REGION_QUEUE_CAPACITY: usize = 1024;

/// How many of a player's latest inputs are kept in case they have to be sent again to
/// the region the player walks into. That region needs the ones the old region had not
/// got to when it let the player go, which is what arrived within a tick or so: a
/// handful. A client sends some twenty to forty inputs per second.
const KEPT_INPUTS: usize = 128;

/// Chunks in the first batch a client is sent, as in vanilla.
const INITIAL_BATCH_SIZE: usize = 9;
/// The largest batch, whatever the client asks for, as in vanilla.
const MAX_BATCH_SIZE: usize = 64;

/// The value of a player's `awaiting_teleport` when no teleport is unconfirmed.
///
/// When the server moves a player, the client keeps sending positions from before the
/// move until it has processed it. The fan-out task therefore stores the id of the
/// teleport it sends, and the connection ignores movement until the client has confirmed
/// that id.
pub(crate) const NO_TELEPORT: i32 = 0;

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
        /// Shared with the connection; see [`NO_TELEPORT`].
        awaiting_teleport: Arc<AtomicI32>,
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
    /// The player did something that the region they are in has to know.
    Input {
        session: SessionId,
        player: PlayerId,
        input: PlayerInput,
    },
}

/// Settings of the fan-out task.
#[derive(Debug, Clone)]
pub(crate) struct FanoutConfig {
    pub(crate) max_players: u32,
    /// The largest view distance granted to a client, in chunks.
    pub(crate) view_distance: i32,
    /// Where the number of players in the world is published.
    pub(crate) online: Arc<AtomicU32>,
}

/// A chunk at least one player of this edge can see.
struct ReplicaChunk {
    /// How many players have the chunk in view.
    viewers: u32,
    /// The chunk, once the worker has sent it, kept up to date with later changes.
    chunk: Option<Chunk>,
    /// The chunk's packet. It is encoded when first needed and discarded when the chunk
    /// changes, so that many players can be sent the same bytes.
    packet: Option<Bytes>,
}

impl ReplicaChunk {
    /// The packet that sends the chunk, located at `position`, if the chunk is there.
    fn packet(&mut self, position: ChunkPos) -> Option<Bytes> {
        if self.packet.is_none() {
            let chunk = self.chunk.as_ref()?;
            self.packet = Some(encoded(&chunk_packet(position, chunk)));
        }
        self.packet.clone()
    }
}

/// What one player has been sent and is waiting for.
struct PlayerView {
    session: SessionId,
    name: String,
    outbound: mpsc::Sender<Bytes>,
    awaiting_teleport: Arc<AtomicI32>,
    /// The player's entity, once a region has placed the player.
    entity: Option<EntityId>,
    /// The region the player is in, as far as this task has been told. The region itself
    /// may already have let the player go, with the message saying so still on its way.
    region: RegionId,
    /// How many inputs of the player have been passed on. Inputs are numbered from 1.
    inputs_sent: u64,
    /// The latest inputs with their numbers; see [`KEPT_INPUTS`].
    kept_inputs: VecDeque<(u64, PlayerInput)>,
    view_distance: i32,
    /// The chunks in view. Empty until the worker has placed the player.
    wanted: BTreeSet<ChunkPos>,
    center: ChunkPos,
    /// Chunks in view that are in the replica but have not been sent.
    pending: BTreeSet<ChunkPos>,
    /// Chunks in view the client has been sent.
    sent: BTreeSet<ChunkPos>,
    /// Entities the client has been shown, with the player each of them is.
    visible: BTreeMap<EntityId, PlayerId>,
    /// Players the client has in its player list. A client only shows the entity of a
    /// player it has in that list.
    listed: BTreeSet<PlayerId>,
    /// Whether a batch has been sent that the client has not confirmed.
    batch_outstanding: bool,
    batch_size: usize,
}

pub(crate) struct Fanout {
    config: FanoutConfig,
    layout: Layout,
    /// The region players enter the world in.
    spawn_region: RegionId,
    /// Where to send what is meant for each region, by region id.
    regions: Vec<link::Sender<EdgeToWorker>>,
    /// Where each region's messages arrive, until [`Fanout::run`] starts reading them.
    receivers: Vec<link::Receiver<WorkerToEdge>>,
    commands: mpsc::Receiver<Command>,
    players: BTreeMap<PlayerId, PlayerView>,
    /// Which player each player entity of this edge belongs to.
    entity_owners: BTreeMap<EntityId, PlayerId>,
    replica: BTreeMap<ChunkPos, ReplicaChunk>,
    /// The entities in the chunks of the replica.
    entities: BTreeMap<EntityId, EntityState>,
}

impl Fanout {
    /// `routing` must have one link per region of its layout.
    pub(crate) fn new(
        config: FanoutConfig,
        routing: Routing,
        commands: mpsc::Receiver<Command>,
    ) -> Self {
        assert_eq!(
            routing.links.len(),
            routing.layout.region_count(),
            "one link per region"
        );
        let spawn = routing.spawn;
        let spawn_region = routing
            .layout
            .region_of(ChunkPos::containing(spawn.x, spawn.z));
        let (regions, receivers) = routing.links.into_iter().map(link::End::split).unzip();
        Self {
            config,
            layout: routing.layout,
            spawn_region,
            regions,
            receivers,
            commands,
            players: BTreeMap::new(),
            entity_owners: BTreeMap::new(),
            replica: BTreeMap::new(),
            entities: BTreeMap::new(),
        }
    }

    /// Serves until a region or the edge is gone. Every player is disconnected when this
    /// returns, because their queues are dropped.
    pub(crate) async fn run(mut self) {
        // The messages of all regions are read from one queue, each region's in the
        // order it sent them. `None` says that a region's link has ended. The tasks
        // feeding the queue stop with this function, which owns them.
        let (queue, mut messages) = mpsc::channel(REGION_QUEUE_CAPACITY);
        let mut readers = JoinSet::new();
        for (index, mut receiver) in std::mem::take(&mut self.receivers).into_iter().enumerate() {
            let region = RegionId(index as u32);
            let queue = queue.clone();
            readers.spawn(async move {
                while let Some(message) = receiver.recv().await {
                    if queue.send((region, Some(message))).await.is_err() {
                        return;
                    }
                }
                let _ = queue.send((region, None)).await;
            });
        }
        drop(queue);

        loop {
            let alive = tokio::select! {
                command = self.commands.recv() => match command {
                    Some(command) => self.handle_command(command).await,
                    None => false,
                },
                message = messages.recv() => match message {
                    Some((region, Some(message))) => self.handle_region(region, message).await,
                    Some((region, None)) => {
                        // Without one of its regions the world has a hole; there is no
                        // carrying on until someone runs that region again.
                        warn!(%region, "a region is gone");
                        false
                    }
                    None => false,
                },
            };
            if !alive {
                return;
            }
        }
    }

    /// Returns false if a region can no longer be reached.
    async fn handle_command(&mut self, command: Command) -> bool {
        match command {
            Command::Join {
                session,
                profile,
                requested_view_distance,
                outbound,
                awaiting_teleport,
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
                        awaiting_teleport,
                        entity: None,
                        region: self.spawn_region,
                        inputs_sent: 0,
                        kept_inputs: VecDeque::new(),
                        view_distance,
                        wanted: BTreeSet::new(),
                        center: ChunkPos::new(0, 0),
                        pending: BTreeSet::new(),
                        sent: BTreeSet::new(),
                        visible: BTreeMap::new(),
                        listed: BTreeSet::new(),
                        batch_outstanding: false,
                        batch_size: INITIAL_BATCH_SIZE,
                    },
                );
                let join = PlayerJoin {
                    player,
                    name: profile.name,
                };
                self.send_to_region(self.spawn_region, EdgeToWorker::PlayerJoin(join))
                    .await
            }
            Command::Input {
                session,
                player,
                input,
            } => {
                if !self.session_matches(player, session) {
                    return true;
                }
                let view = self.players.get_mut(&player).expect("session matched");
                view.inputs_sent += 1;
                let number = view.inputs_sent;
                if view.kept_inputs.len() == KEPT_INPUTS {
                    view.kept_inputs.pop_front();
                }
                view.kept_inputs.push_back((number, input.clone()));
                let region = view.region;
                let message = EdgeToWorker::Input {
                    player,
                    number,
                    input,
                };
                self.send_to_region(region, message).await
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

    /// Handles what the region `from` sent. Returns false if a region can no longer be
    /// reached.
    async fn handle_region(&mut self, from: RegionId, message: WorkerToEdge) -> bool {
        match message {
            WorkerToEdge::ToPlayer {
                player,
                event:
                    PlayerEvent::Spawned {
                        entity_id,
                        position,
                        hotbar,
                        selected_slot,
                    },
            } => {
                let inventory = inventory_packets(&hotbar, selected_slot);
                self.spawn_player(player, entity_id, position, inventory)
                    .await;
            }
            WorkerToEdge::ToPlayer {
                player,
                event: PlayerEvent::Acknowledged { sequence },
            } => {
                let packet = encoded(&AcknowledgeBlockChange { sequence });
                self.send_to_player(player, [packet]).await;
            }
            WorkerToEdge::ToPlayer {
                player,
                event: PlayerEvent::Refused,
            } => {
                if let Some(view) = self.players.get(&player) {
                    refuse(&view.outbound, "The world cannot take another player.");
                }
                self.remove_player(player).await;
            }
            WorkerToEdge::ToPlayer {
                player,
                event: PlayerEvent::Departed(transfer),
            } => return self.hand_over(player, from, transfer).await,
            WorkerToEdge::ChunkSnapshot {
                position,
                chunk,
                entities,
                ..
            } => {
                self.store_chunk(position, chunk).await;
                for state in entities {
                    self.upsert_entity(state).await;
                }
            }
            WorkerToEdge::TickDelta { events, .. } => {
                for event in events {
                    self.handle_event(event).await;
                }
            }
        }
        true
    }

    /// Passes a player whom the region `from` has let go on to the region they walked
    /// into, together with what they have done since. Returns false if a region can no
    /// longer be reached.
    ///
    /// Nothing else is handled while this runs, so no input of the player can go to the
    /// old region after the ones sent again here have been picked, or to the new region
    /// before them.
    async fn hand_over(
        &mut self,
        player: PlayerId,
        from: RegionId,
        transfer: PlayerTransfer,
    ) -> bool {
        let position = transfer.pose.position;
        let chunk = ChunkPos::containing(position.x, position.z);
        let to = self.layout.region_of(chunk);
        // The region did not report the entity as removed, because it lives on. Should
        // it turn out to have nowhere to go, the region it was heading for is told to
        // report it, or it would stay on the screens of those who saw it cross.
        let discard = EdgeToWorker::Discard {
            entity: transfer.entity_id,
            chunk,
        };

        // The player's connection can have ended while the message was on its way, and
        // they can even be back already, as a new entity somewhere else.
        let current = self
            .players
            .get_mut(&player)
            .filter(|view| view.entity == Some(transfer.entity_id));
        let Some(view) = current else {
            return self.send_to_region(to, discard).await;
        };
        if to == from {
            // The region and this edge disagree about where the region ends. Sending
            // the player back would have them bounce between the two forever.
            error!(name = %view.name, %from, "a region let go of a player who is inside it");
            refuse(&view.outbound, "The server lost track of where you are.");
            return self.send_to_region(to, discard).await && self.remove_player(player).await;
        }

        // The old region ignored everything it was sent after letting the player go.
        while view
            .kept_inputs
            .front()
            .is_some_and(|(number, _)| *number <= transfer.last_input)
        {
            view.kept_inputs.pop_front();
        }
        let missed = view.inputs_sent.saturating_sub(transfer.last_input);
        if view.kept_inputs.len() as u64 != missed {
            // The region was so far behind that some of what it missed is no longer
            // kept. Carrying on would leave the player with, say, another item in hand
            // than the server believes.
            warn!(name = %view.name, missed, "too many inputs to send again");
            refuse(&view.outbound, "The server fell too far behind.");
            return self.send_to_region(to, discard).await && self.remove_player(player).await;
        }
        view.region = to;
        let again: Vec<_> = view.kept_inputs.iter().cloned().collect();
        debug!(name = %view.name, %from, %to, inputs = again.len(), "handing a player over");

        if !self
            .send_to_region(to, EdgeToWorker::PlayerArrive { player, transfer })
            .await
        {
            return false;
        }
        for (number, input) in again {
            let message = EdgeToWorker::Input {
                player,
                number,
                input,
            };
            if !self.send_to_region(to, message).await {
                return false;
            }
        }
        // Normally the view has followed the move already; this covers a player who was
        // let go without having been seen to move, such as one who joined right there.
        self.move_view(player, chunk).await;
        true
    }

    async fn handle_event(&mut self, event: RegionEvent) {
        match event {
            RegionEvent::EntitySpawned(state) => self.upsert_entity(state).await,
            RegionEvent::EntityRemoved { entity, .. } => self.remove_entity(entity).await,
            RegionEvent::BlockChanged { position, state } => {
                self.change_block(position, state).await;
            }
            RegionEvent::EntityMoved { entity, pose, .. } => {
                // A move of an entity the edge has not been told about yet is covered by
                // the snapshot of the chunk it is in, which is still on its way.
                if let Some(state) = self.entities.get_mut(&entity) {
                    state.pose = pose;
                    self.refresh_entity(entity, true).await;
                }
                // The view of a player follows where the worker says the player is.
                if let Some(player) = self.entity_owners.get(&entity).copied() {
                    let center = ChunkPos::containing(pose.position.x, pose.position.z);
                    self.move_view(player, center).await;
                }
            }
        }
    }

    /// Takes in an entity the worker described, which may be new or already known.
    async fn upsert_entity(&mut self, state: EntityState) {
        if !self.replica.contains_key(&state.chunk()) {
            // Nobody watches where it is; this can be a snapshot that arrives late.
            return;
        }
        let entity = state.entity;
        let moved = self
            .entities
            .insert(entity, state.clone())
            .is_some_and(|known| known.pose != state.pose);
        self.refresh_entity(entity, moved).await;
    }

    /// Brings every player's client up to date about one entity: shows it to those who
    /// have it in view, moves it if `moved`, and hides it from those who lost sight of it.
    async fn refresh_entity(&mut self, entity: EntityId, moved: bool) {
        let Some(state) = self.entities.get(&entity).cloned() else {
            return;
        };
        let mut outgoing = Vec::new();
        for (player, view) in &mut self.players {
            let packets = view.update_visibility(&state, moved);
            if !packets.is_empty() {
                outgoing.push((*player, packets));
            }
        }
        if !self.replica.contains_key(&state.chunk()) {
            // It went where this edge does not watch.
            self.entities.remove(&entity);
        }
        for (player, packets) in outgoing {
            self.send_to_player(player, packets).await;
        }
    }

    /// Forgets an entity that no longer exists and hides it from everyone who saw it.
    async fn remove_entity(&mut self, entity: EntityId) {
        self.entities.remove(&entity);
        let viewers: Vec<_> = self
            .players
            .iter_mut()
            .filter_map(|(player, view)| view.visible.remove(&entity).map(|_| *player))
            .collect();
        for player in viewers {
            let packet = encoded(&RemoveEntities {
                entity_ids: vec![entity.0],
            });
            self.send_to_player(player, [packet]).await;
        }
    }

    /// Puts a player the worker has placed into the world: the packets that start the
    /// play state, then the chunks around them.
    async fn spawn_player(
        &mut self,
        player: PlayerId,
        entity_id: EntityId,
        position: Vec3,
        inventory: [Bytes; 2],
    ) {
        let Some(view) = self.players.get_mut(&player) else {
            // The player left before the worker answered.
            return;
        };
        view.entity = Some(entity_id);
        self.entity_owners.insert(entity_id, player);
        self.config.online.fetch_add(1, Ordering::Relaxed);
        info!(name = %view.name, entity_id = entity_id.0, "player joined");

        // Placing the player is the first teleport the client has to confirm.
        let teleport_id = 1;
        view.awaiting_teleport.store(teleport_id, Ordering::Relaxed);
        let entered = [
            encoded(&login_packet(entity_id, &self.config)),
            encoded(&PlayerAbilities {
                flags: CREATIVE_ABILITIES,
                flying_speed: 0.05,
                field_of_view_modifier: 0.1,
            }),
            encoded(&SynchronizePlayerPosition {
                teleport_id,
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
        ];
        if !self.send_to_player(player, entered).await
            || !self.send_to_player(player, inventory).await
        {
            return;
        }

        // Everyone in the world is in everyone's player list, including their own.
        let in_world: Vec<_> = self
            .players
            .iter()
            .filter(|(_, view)| view.entity.is_some())
            .map(|(id, view)| (*id, view.name.clone()))
            .collect();
        let newcomer = in_world
            .iter()
            .find(|(id, _)| *id == player)
            .cloned()
            .expect("the player was just placed");
        for (other, _) in &in_world {
            let additions: &[(PlayerId, String)] = if *other == player {
                &in_world
            } else {
                std::slice::from_ref(&newcomer)
            };
            let Some(view) = self.players.get_mut(other) else {
                continue;
            };
            // Someone may already have been told, with the newcomer's entity.
            let unlisted: Vec<_> = additions
                .iter()
                .filter(|(id, _)| view.listed.insert(*id))
                .cloned()
                .collect();
            if !unlisted.is_empty() {
                let packet = encoded(&player_list_additions(&unlisted));
                self.send_to_player(*other, [packet]).await;
            }
        }

        let center = ChunkPos::containing(position.x, position.z);
        self.move_view(player, center).await;
    }

    /// Centres a player's view on `center`: tells the client, starts sending the chunks
    /// that came into view and makes the client forget those that left it.
    async fn move_view(&mut self, player: PlayerId, center: ChunkPos) {
        let Some(view) = self.players.get_mut(&player) else {
            return;
        };
        if view.center == center && !view.wanted.is_empty() {
            return;
        }
        let wanted = view_area(center, view.view_distance);
        let mut packets = vec![encoded(&SetCenterChunk {
            chunk_x: center.x,
            chunk_z: center.z,
        })];

        let mut unsubscribe = Vec::new();
        for position in view.wanted.difference(&wanted) {
            view.pending.remove(position);
            if view.sent.remove(position) {
                packets.push(encoded(&UnloadChunk {
                    chunk_x: position.x,
                    chunk_z: position.z,
                }));
            }
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
        let mut subscribe = Vec::new();
        for position in wanted.difference(&view.wanted) {
            let chunk = self.replica.entry(*position).or_insert_with(|| {
                subscribe.push(*position);
                ReplicaChunk {
                    viewers: 0,
                    chunk: None,
                    packet: None,
                }
            });
            chunk.viewers += 1;
            if chunk.chunk.is_some() {
                view.pending.insert(*position);
            }
        }
        view.center = center;
        view.wanted = wanted;
        // Entities in chunks that came into view appear, those left behind disappear.
        for state in self.entities.values() {
            packets.extend(view.update_visibility(state, false));
        }
        let replica = &self.replica;
        self.entities
            .retain(|_, state| replica.contains_key(&state.chunk()));

        self.subscribe(subscribe).await;
        self.unsubscribe(unsubscribe).await;
        if self.send_to_player(player, packets).await {
            self.send_chunks(player).await;
        }
    }

    /// Takes a chunk the worker sent into the replica and offers it to its viewers.
    async fn store_chunk(&mut self, position: ChunkPos, chunk: Chunk) {
        let Some(entry) = self.replica.get_mut(&position) else {
            // Nobody has the chunk in view any more.
            return;
        };
        entry.chunk = Some(chunk);
        entry.packet = None;

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

    /// Applies a block change to the replica and tells everyone who has the chunk.
    async fn change_block(&mut self, position: BlockPos, state: BlockState) {
        let chunk_position = position.chunk();
        let Some(entry) = self.replica.get_mut(&chunk_position) else {
            return;
        };
        let Some(chunk) = &mut entry.chunk else {
            // The chunk itself is still on its way and will include the change.
            return;
        };
        let (x, z) = position.in_chunk();
        chunk.set(x, position.y, z, state);
        entry.packet = None;

        let packet = encoded(&BlockUpdate {
            position: Position {
                x: position.x,
                y: position.y,
                z: position.z,
            },
            state: state.0.into(),
        });
        // Players who have not been sent the chunk yet get it with the change in it.
        let viewers: Vec<_> = self
            .players
            .iter()
            .filter(|(_, view)| view.sent.contains(&chunk_position))
            .map(|(player, _)| *player)
            .collect();
        for player in viewers {
            self.send_to_player(player, [packet.clone()]).await;
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
            view.sent.insert(*position);
            let packet = self
                .replica
                .get_mut(position)
                .and_then(|chunk| chunk.packet(*position));
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

    /// Asks the regions the chunks belong to for them and for what happens in them.
    /// Returns false if a region can no longer be reached, which ends the task shortly
    /// after anyway.
    async fn subscribe(&mut self, chunks: Vec<ChunkPos>) -> bool {
        let mut reached = true;
        for (region, chunks) in self.by_region(chunks) {
            reached &= self
                .send_to_region(region, EdgeToWorker::Subscribe { chunks })
                .await;
        }
        reached
    }

    /// Tells the regions the chunks belong to that they are no longer needed here.
    async fn unsubscribe(&mut self, chunks: Vec<ChunkPos>) -> bool {
        let mut reached = true;
        for (region, chunks) in self.by_region(chunks) {
            reached &= self
                .send_to_region(region, EdgeToWorker::Unsubscribe { chunks })
                .await;
        }
        reached
    }

    /// Sorts chunks by the region they belong to.
    fn by_region(&self, chunks: Vec<ChunkPos>) -> BTreeMap<RegionId, Vec<ChunkPos>> {
        let mut sorted: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for chunk in chunks {
            sorted
                .entry(self.layout.region_of(chunk))
                .or_default()
                .push(chunk);
        }
        sorted
    }

    /// Forgets a player and releases what was held for them. Dropping their queue ends
    /// their connection. Returns false if a region can no longer be reached.
    async fn remove_player(&mut self, player: PlayerId) -> bool {
        let Some(view) = self.players.remove(&player) else {
            return true;
        };
        info!(name = %view.name, "player left");
        if let Some(entity) = view.entity {
            self.entity_owners.remove(&entity);
            self.config.online.fetch_sub(1, Ordering::Relaxed);
            // The region the player is in reports the entity gone as well, but only
            // once it has heard, and if the player is being handed over just now, only
            // after a detour. By then the player may be back as a new entity, and a
            // client must never be shown two entities of one player. So the entity is
            // hidden here and now; the region's report then finds nothing left to do.
            self.entities.remove(&entity);
            let packet = encoded(&RemoveEntities {
                entity_ids: vec![entity.0],
            });
            for other in self.players.values_mut() {
                if other.visible.remove(&entity).is_some() {
                    // See below for why this is not `send_to_player`.
                    let _ = other.outbound.try_send(packet.clone());
                }
            }
        }
        // The entry in the player list is this edge's to remove.
        let listing: Vec<_> = self
            .players
            .iter_mut()
            .filter_map(|(other, view)| view.listed.remove(&player).then_some(*other))
            .collect();
        for other in listing {
            let packet = encoded(&PlayerInfoRemove {
                players: vec![player.0],
            });
            // A viewer that cannot be reached is removed by this very function, which
            // cannot be awaited from itself; its turn comes with its next packet.
            if let Some(other) = self.players.get(&other) {
                let _ = other.outbound.try_send(packet);
            }
        }
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
        let replica = &self.replica;
        self.entities
            .retain(|_, state| replica.contains_key(&state.chunk()));
        if !self.unsubscribe(unsubscribe).await {
            return false;
        }
        // If that region has just let the player go, it ignores this, and the message
        // saying so, which is on its way, makes `hand_over` clean up.
        self.send_to_region(view.region, EdgeToWorker::PlayerLeave { player })
            .await
    }

    fn session_matches(&self, player: PlayerId, session: SessionId) -> bool {
        self.players
            .get(&player)
            .is_some_and(|view| view.session == session)
    }

    /// Returns false if the region can no longer be reached.
    async fn send_to_region(&mut self, region: RegionId, message: EdgeToWorker) -> bool {
        match self.regions.get(region.0 as usize) {
            Some(link) => link.send(message).await.is_ok(),
            None => false,
        }
    }
}

impl PlayerView {
    /// Works out what the client has to be told about `state` and records it: the
    /// packets that show the entity if it came into view, move it if `moved`, or hide it
    /// if it left the view. A player is never shown their own entity; the client
    /// creates that itself.
    fn update_visibility(&mut self, state: &EntityState, moved: bool) -> Vec<Bytes> {
        let in_world = self.entity.is_some();
        let own = self.entity == Some(state.entity);
        let in_view = in_world && !own && self.wanted.contains(&state.chunk());
        let shown = self.visible.contains_key(&state.entity);
        match (in_view, shown) {
            (true, false) => {
                let mut packets = Vec::new();
                let EntityKind::Player { player, name } = &state.kind;
                // A client takes a player to be one entity. Should word of an entity
                // the player was before still be around, it goes first.
                let outdated: Vec<_> = self
                    .visible
                    .iter()
                    .filter(|(_, shown)| *shown == player)
                    .map(|(entity, _)| entity.0)
                    .collect();
                if !outdated.is_empty() {
                    self.visible.retain(|_, shown| shown != player);
                    packets.push(encoded(&RemoveEntities {
                        entity_ids: outdated,
                    }));
                }
                self.visible.insert(state.entity, *player);
                if self.listed.insert(*player) {
                    let addition = [(*player, name.clone())];
                    packets.push(encoded(&player_list_additions(&addition)));
                }
                packets.extend(spawn_packets(state));
                packets
            }
            (true, true) if moved => move_packets(state).into(),
            (false, true) => {
                self.visible.remove(&state.entity);
                vec![encoded(&RemoveEntities {
                    entity_ids: vec![state.entity.0],
                })]
            }
            _ => Vec::new(),
        }
    }
}

/// The packets that tell a client what its player carries: the nine hotbar slots, all
/// other slots empty, and which hotbar slot is selected.
fn inventory_packets(hotbar: &[Option<ItemStack>; HOTBAR_SLOTS], selected_slot: u8) -> [Bytes; 2] {
    let mut slots = vec![None; inventory::SLOT_COUNT];
    for (slot, stack) in hotbar.iter().enumerate() {
        slots[inventory::HOTBAR_START + slot] = stack.map(|stack| protocol::ItemStack {
            item: stack.item,
            count: stack.count,
        });
    }
    [
        encoded(&SetContainerContent {
            window_id: inventory::PLAYER_WINDOW,
            state_id: 0,
            slots,
            carried: None,
        }),
        encoded(&SetHeldSlot {
            slot: selected_slot.into(),
        }),
    ]
}

/// Adds players to a client's player list.
fn player_list_additions(players: &[(PlayerId, String)]) -> PlayerInfoUpdate {
    PlayerInfoUpdate {
        actions: player_info::ADD_PLAYER
            | player_info::UPDATE_GAME_MODE
            | player_info::UPDATE_LISTED,
        entries: players
            .iter()
            .map(|(player, name)| PlayerInfoEntry {
                uuid: player.0,
                profile: Some((name.clone(), Vec::new())),
                game_mode: Some(game_mode::CREATIVE),
                listed: Some(true),
                ..PlayerInfoEntry::default()
            })
            .collect(),
    }
}

/// The packets that make an entity appear on a client.
fn spawn_packets(state: &EntityState) -> [Bytes; 2] {
    let EntityKind::Player { player, .. } = &state.kind;
    let position = state.pose.position;
    [
        encoded(&SpawnEntity {
            entity_id: state.entity.0,
            uuid: player.0,
            kind: entity_types::PLAYER,
            x: position.x,
            y: position.y,
            z: position.z,
            velocity: [0.0; 3],
            pitch: angle(state.pose.pitch),
            yaw: angle(state.pose.yaw),
            head_yaw: angle(state.pose.yaw),
            data: 0,
        }),
        head_rotation(state),
    ]
}

/// The packets that put an entity a client already shows where it is now.
fn move_packets(state: &EntityState) -> [Bytes; 2] {
    let position = state.pose.position;
    [
        encoded(&SyncEntityPosition {
            entity_id: state.entity.0,
            path: PositionPath::Linear {
                x: position.x,
                y: position.y,
                z: position.z,
            },
            yaw: state.pose.yaw,
            pitch: state.pose.pitch,
            on_ground: state.pose.on_ground,
        }),
        head_rotation(state),
    ]
}

/// The head of a player looks where the player looks; the position packets only turn
/// the body.
fn head_rotation(state: &EntityState) -> Bytes {
    encoded(&SetHeadRotation {
        entity_id: state.entity.0,
        head_yaw: angle(state.pose.yaw),
    })
}

fn encoded<P: Packet>(packet: &P) -> Bytes {
    packets::encode(packet).into()
}

/// Tells a client why it cannot join. Its connection ends when `outbound` is dropped.
fn refuse(outbound: &mpsc::Sender<Bytes>, reason: &str) {
    let reason = Nbt::String(reason.to_owned());
    let _ = outbound.try_send(encoded(&Disconnect { reason }));
}

/// The chunks a client with the given view distance is sent when it is in `center`.
///
/// This is the area the official server uses: a circle of the view distance around a
/// 5×5 block of chunks, so slightly more than the view distance in every direction.
fn view_area(center: ChunkPos, view_distance: i32) -> BTreeSet<ChunkPos> {
    // Chunks this close to the centre always count as distance 0.
    const BUFFER: i64 = 2;
    let reach = view_distance + BUFFER as i32;
    let limit = i64::from(view_distance) * i64::from(view_distance);
    let mut area = BTreeSet::new();
    for dx in -reach..=reach {
        for dz in -reach..=reach {
            let x = (i64::from(dx).abs() - BUFFER).max(0);
            let z = (i64::from(dz).abs() - BUFFER).max(0);
            if x * x + z * z < limit {
                area.insert(ChunkPos::new(center.x + dx, center.z + dz));
            }
        }
    }
    area
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
    fn view_area_is_a_rounded_square() {
        // The official server sends 329 chunks at view distance 8.
        let area = view_area(ChunkPos::new(10, -3), 8);
        assert_eq!(area.len(), 329);
        // Straight ahead it reaches one chunk beyond the view distance.
        assert!(area.contains(&ChunkPos::new(19, -3)));
        assert!(!area.contains(&ChunkPos::new(20, -3)));
        // The corners are cut.
        assert!(!area.contains(&ChunkPos::new(19, 6)));
        assert!(area.contains(&ChunkPos::new(17, 4)));
    }

    #[test]
    fn small_view_areas_are_full_squares() {
        assert_eq!(view_area(ChunkPos::new(0, 0), 2).len(), 49);
        assert_eq!(view_area(ChunkPos::new(0, 0), 3).len(), 81);
    }
}
