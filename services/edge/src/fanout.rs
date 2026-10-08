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
use std::time::Duration;

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
use clustine_rpc::{EdgeMessage, EdgeToWorker, Presence, Welcome, WorkerToEdge};
use clustine_sim::api::{
    Durable, EntityKind, EntityState, HOTBAR_SLOTS, ItemStack, PlayerEvent, PlayerInput,
    PlayerJoin, PlayerTransfer, RegionEvent,
};
use clustine_world::{BlockPos, Chunk, ChunkPos, EntityId, PlayerId, Vec3};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::{Instant, MissedTickBehavior};
use tracing::{debug, error, info, warn};

use crate::encode::chunk_packet;
use crate::login::Profile;
use crate::{EdgeIdentity, RegionLink, Routing, Stopped};

const OVERWORLD: &str = "minecraft:overworld";

/// Player ability flags of creative mode: invulnerable, may fly, breaks blocks instantly.
const CREATIVE_ABILITIES: u8 = 0x01 | 0x04 | 0x08;

/// Messages from the regions that may wait for the fan-out task.
const REGION_QUEUE_CAPACITY: usize = 1024;

/// How often the fan-out task looks for players whose region has kept them waiting for
/// too long.
const PATIENCE_CHECK: Duration = Duration::from_secs(1);

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
    /// The player did something to blocks that no region needs to hear of, and their
    /// client waits to be told that it was handled.
    Handled {
        session: SessionId,
        player: PlayerId,
        sequence: i32,
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
    /// See [`crate::EdgeConfig::region_patience`].
    pub(crate) region_patience: Duration,
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
    /// When the player asked to enter the world, for as long as no region has placed
    /// them.
    joining_since: Option<Instant>,
    /// How many inputs of the player have been passed on. Inputs are numbered from 1.
    inputs_sent: u64,
    /// The inputs no region has reported as applied and durable yet, with their numbers
    /// and when they were made. They are sent again to the region the player walks
    /// into, which needs the ones the old region had not got to when it let the player
    /// go.
    kept_inputs: VecDeque<(u64, Instant, PlayerInput)>,
    /// The highest sequence number of the player's actions on blocks that has been
    /// reported as handled.
    handled: Option<i32>,
    /// The sequence numbers of actions that concern blocks of another region than the
    /// player's and are on their way there. The client is not told that an action was
    /// handled while an earlier one is among these: it would stop showing its own guess
    /// of what the action did before the region that has the blocks has said what it
    /// really did, and the block would flicker.
    under_way: BTreeSet<i32>,
    /// The highest sequence number the client has been told was handled.
    acknowledged: Option<i32>,
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

/// The edge's link to a region.
struct Link {
    sender: link::Sender<EdgeMessage>,
    /// The epoch of the region's owner at the other end.
    epoch: u64,
    /// Which of this edge's links it is. What a region sends is tagged with the link it
    /// came over, and dropped if that is not the region's link any more.
    id: u64,
    /// Whether the region has said what it knows of this edge. Until then nothing that
    /// was kept is sent again: a region that has forgotten the edge expects its
    /// messages numbered from 1, and would close the link over the gap.
    welcomed: bool,
}

/// What the edge keeps for a region, with or without a link to it; see
/// `docs/adr/0008-durable-regions-and-resuming.md`.
#[derive(Default)]
struct RegionPort {
    link: Option<Link>,
    /// The number of the last numbered message made for the region.
    numbered: u64,
    /// The numbered messages the region has not reported as applied and durable, to be
    /// sent again on a new link.
    kept: VecDeque<(u64, EdgeToWorker)>,
    /// Up to which number the region has reported the messages applied and durable.
    applied: u64,
    /// The number of the last outbox entry of the region that was handled.
    seen: u64,
}

/// What reaches the fan-out task from a link to a region: a message, or `None` when the
/// link has ended.
type FromLink = (RegionId, u64, Option<WorkerToEdge>);

pub(crate) struct Fanout {
    config: FanoutConfig,
    layout: Layout,
    /// The region players enter the world in.
    spawn_region: RegionId,
    /// Who this edge is to the regions.
    identity: EdgeIdentity,
    /// What is kept for each region, by region id.
    regions: Vec<RegionPort>,
    /// The links to start with, until [`Fanout::run`] takes them up.
    first_links: Vec<RegionLink>,
    /// Where new links to regions arrive.
    relinks: mpsc::Receiver<RegionLink>,
    /// Where it is said that a link has ended, for whoever makes the links.
    lost: mpsc::UnboundedSender<(RegionId, u64)>,
    /// The number the next link gets.
    next_link: u64,
    /// Where the links' messages are put for this task; see [`Fanout::run`].
    queue: mpsc::Sender<FromLink>,
    /// Taken by [`Fanout::run`].
    messages: Option<mpsc::Receiver<FromLink>>,
    /// The tasks that read the links.
    readers: JoinSet<()>,
    commands: mpsc::Receiver<Command>,
    players: BTreeMap<PlayerId, PlayerView>,
    /// Which player each player entity of this edge belongs to.
    entity_owners: BTreeMap<EntityId, PlayerId>,
    replica: BTreeMap<ChunkPos, ReplicaChunk>,
    /// The entities in the chunks of the replica.
    entities: BTreeMap<EntityId, Shown>,
}

/// An entity the edge shows, and the region that last introduced it: reported it
/// spawned, or had it in a snapshot. Moves and removals of the entity are taken from
/// that region only. An entity that walks from one region into the next is introduced
/// by the next one when it arrives, and what its old region says of it after that,
/// such as that it has given up on a departure it believes nobody passed on, is about
/// an entity that is the old region's no longer.
///
/// With several edges this needs more: what two regions say reaches an edge that is not
/// the player's own in no order between them, so a late snapshot of the old region can
/// be taken for the entity's latest introduction.
#[derive(Debug, Clone)]
struct Shown {
    state: EntityState,
    from: RegionId,
}

impl Fanout {
    /// `routing` must not have a link to a region its layout does not have.
    pub(crate) fn new(
        config: FanoutConfig,
        routing: Routing,
        commands: mpsc::Receiver<Command>,
    ) -> Self {
        let regions = routing.layout.region_count();
        assert!(
            routing
                .links
                .iter()
                .all(|link| (link.region.0 as usize) < regions),
            "a link to a region the layout does not have"
        );
        let spawn = routing.spawn;
        let spawn_region = routing
            .layout
            .region_of(ChunkPos::containing(spawn.x, spawn.z));
        // The messages of all regions are read from one queue, each link's in the order
        // they were sent.
        let (queue, messages) = mpsc::channel(REGION_QUEUE_CAPACITY);
        Self {
            config,
            layout: routing.layout,
            spawn_region,
            identity: routing.identity,
            regions: (0..regions).map(|_| RegionPort::default()).collect(),
            first_links: routing.links,
            relinks: routing.relinks,
            lost: routing.lost,
            next_link: 0,
            queue,
            messages: Some(messages),
            readers: JoinSet::new(),
            commands,
            players: BTreeMap::new(),
            entity_owners: BTreeMap::new(),
            replica: BTreeMap::new(),
            entities: BTreeMap::new(),
        }
    }

    /// Serves until the edge has to stop. Every player is disconnected when this
    /// returns, because their queues are dropped. A region whose link ends is waited
    /// for: what its players do is kept and sent when there is a link to it again.
    pub(crate) async fn run(mut self) -> Stopped {
        let mut messages = self.messages.take().expect("the fan-out task runs once");
        for link in std::mem::take(&mut self.first_links) {
            self.take_link(link).await;
        }
        let mut patience = tokio::time::interval(PATIENCE_CHECK);
        patience.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                command = self.commands.recv() => match command {
                    Some(command) => self.handle_command(command).await,
                    None => return Stopped::Abandoned,
                },
                message = messages.recv() => {
                    // The task holds a sender itself, so the queue never ends.
                    let Some((region, link, message)) = message else {
                        return Stopped::Abandoned;
                    };
                    let current = self.regions[region.0 as usize]
                        .link
                        .as_ref()
                        .is_some_and(|current| current.id == link);
                    match message {
                        _ if !current => debug!(%region, "dropped what a former link delivered"),
                        Some(message) => {
                            if let Some(stopped) = self.handle_region(region, message).await {
                                return stopped;
                            }
                        }
                        None => self.lose_link(region),
                    }
                },
                // Whoever gives the edge links may go away; the links it has stay.
                Some(link) = self.relinks.recv() => self.take_link(link).await,
                _ = patience.tick() => self.drop_the_overdue().await,
            }
        }
    }

    /// Notes that the link to `region` has ended. Its players stay, and what they do is
    /// kept, until there is a link to the region again.
    fn lose_link(&mut self, region: RegionId) {
        if let Some(link) = self.regions[region.0 as usize].link.take() {
            warn!(%region, epoch = link.epoch, "the link to a region ended; keeping its players");
            // Nobody may be listening, which is fine.
            let _ = self.lost.send((region, link.epoch));
        }
    }

    /// Makes `link` the edge's link to its region and begins to resume with the region:
    /// says who this edge is and what it knows of the region. What was kept for the
    /// region is sent once the region has answered; see [`Fanout::welcomed`].
    async fn take_link(&mut self, link: RegionLink) {
        let RegionLink { region, epoch, end } = link;
        let Some(port) = self.regions.get_mut(region.0 as usize) else {
            warn!(%region, "a link to a region the layout does not have");
            return;
        };
        if port
            .link
            .as_ref()
            .is_some_and(|current| current.epoch > epoch)
        {
            debug!(%region, epoch, "not taking a link to a replaced owner");
            return;
        }
        let id = self.next_link;
        self.next_link += 1;
        let (sender, mut receiver) = end.split();
        let queue = self.queue.clone();
        self.readers.spawn(async move {
            while let Some(message) = receiver.recv().await {
                if queue.send((region, id, Some(message))).await.is_err() {
                    return;
                }
            }
            let _ = queue.send((region, id, None)).await;
        });

        // Everyone the edge believes to be in the region, also those still entering
        // the world there, and every chunk of it the edge shows or has asked for.
        let players = self
            .players
            .iter()
            .filter(|(_, view)| view.region == region)
            .map(|(player, _)| *player)
            .collect();
        let chunks = self
            .replica
            .keys()
            .filter(|position| self.layout.region_of(**position) == region)
            .copied()
            .collect();
        let hello = EdgeToWorker::Hello {
            edge: self.identity.edge,
            start: self.identity.start,
            seen: port.seen,
            players,
            chunks,
        };
        info!(%region, epoch, kept = port.kept.len(), "linked to a region");
        // A link that is gone already is noticed by its reader.
        let _ = sender.send(EdgeMessage::unnumbered(hello)).await;
        port.link = Some(Link {
            sender,
            epoch,
            id,
            welcomed: false,
        });
    }

    /// The region `from` has said what it knows of this edge. Returns why the edge has
    /// to stop, if it has to.
    async fn welcomed(&mut self, from: RegionId, welcome: Welcome) -> Option<Stopped> {
        let port = &self.regions[from.0 as usize];
        match welcome {
            Welcome::Superseded => {
                error!(%from, "another edge has taken this one's name; stopping");
                return Some(Stopped::Superseded);
            }
            Welcome::Resumed => {}
            // To an edge that has had nothing to do with the region this is how
            // everything begins.
            Welcome::Unknown if port.applied == 0 && port.seen == 0 => {}
            Welcome::Unknown => self.forget_region(from).await,
        }
        let port = &mut self.regions[from.0 as usize];
        let link = port.link.as_mut()?;
        link.welcomed = true;
        // In the order they were made, which is the order of their numbers.
        for (number, body) in &port.kept {
            let message = EdgeMessage {
                number: Some(*number),
                body: body.clone(),
            };
            if link.sender.send(message).await.is_err() {
                // Noticed by the link's reader, which makes room for the next link.
                break;
            }
        }
        None
    }

    /// The region `region` has forgotten this edge, which stayed away from it for too
    /// long: the players the edge believed to be there are not, and what it kept for the
    /// region means nothing to it any more.
    async fn forget_region(&mut self, region: RegionId) {
        warn!(%region, "a region has forgotten this edge; dropping what was kept for it");
        let port = &mut self.regions[region.0 as usize];
        let kept = std::mem::take(&mut port.kept);
        port.numbered = 0;
        port.applied = 0;
        port.seen = 0;
        // What players of other regions did to blocks of this one will never be
        // answered. Their clients are told that it was handled, and see the blocks as
        // the region has them.
        for (_, body) in kept {
            if let EdgeToWorker::Remote(action) = body {
                self.arrived(action.player, action.sequence).await;
            }
        }
        let lost: Vec<_> = self
            .players
            .iter()
            .filter(|(_, view)| view.region == region)
            .map(|(player, _)| *player)
            .collect();
        for player in lost {
            if let Some(view) = self.players.get(&player) {
                refuse(&view.outbound, "The server lost track of where you are.");
            }
            self.remove_player(player).await;
        }
    }

    /// Disconnects the players whose region has not confirmed what they did, or has not
    /// placed them, for longer than the edge is to wait.
    async fn drop_the_overdue(&mut self) {
        let patience = self.config.region_patience;
        let overdue: Vec<_> = self
            .players
            .iter()
            .filter(|(_, view)| {
                let waiting = view.kept_inputs.front().map(|(_, since, _)| *since);
                let since = [view.joining_since, waiting].into_iter().flatten().min();
                since.is_some_and(|since| since.elapsed() > patience)
            })
            .map(|(player, _)| *player)
            .collect();
        for player in overdue {
            if let Some(view) = self.players.get(&player) {
                warn!(
                    name = %view.name,
                    region = %view.region,
                    "a player's region kept them waiting too long"
                );
                refuse(&view.outbound, "The server fell too far behind.");
            }
            self.remove_player(player).await;
        }
    }

    async fn handle_command(&mut self, command: Command) {
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
                    return;
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
                        joining_since: Some(Instant::now()),
                        inputs_sent: 0,
                        kept_inputs: VecDeque::new(),
                        handled: None,
                        under_way: BTreeSet::new(),
                        acknowledged: None,
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
                    .await;
            }
            Command::Input {
                session,
                player,
                input,
            } => {
                if !self.session_matches(player, session) {
                    return;
                }
                let view = self.players.get_mut(&player).expect("session matched");
                view.inputs_sent += 1;
                let number = view.inputs_sent;
                view.kept_inputs
                    .push_back((number, Instant::now(), input.clone()));
                let region = view.region;
                let message = EdgeToWorker::Input {
                    player,
                    number,
                    input,
                };
                self.send_to_region(region, message).await;
            }
            Command::Handled {
                session,
                player,
                sequence,
            } => {
                if self.session_matches(player, session) {
                    self.handled(player, sequence).await;
                }
            }
            Command::Leave { session, player } => {
                if self.session_matches(player, session) {
                    self.remove_player(player).await;
                }
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
                    self.send_chunks(player).await;
                }
            }
        }
    }

    /// Handles what the region `from` sent over its current link. Returns why the edge
    /// has to stop, if it has to.
    async fn handle_region(&mut self, from: RegionId, message: WorkerToEdge) -> Option<Stopped> {
        match message {
            // Of regions that hold chunks (ADR-0010), which no region does yet.
            message @ (WorkerToEdge::Elsewhere { .. } | WorkerToEdge::NotMine { .. }) => {
                error!(%from, ?message, "a region said what this edge does not act on yet");
            }
            WorkerToEdge::Welcome(welcome) => return self.welcomed(from, welcome).await,
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
            } => self.handled(player, sequence).await,
            WorkerToEdge::Outbox { number, entry } => self.outbox(from, number, entry).await,
            WorkerToEdge::Presence { player, answer } => {
                self.presence(from, player, answer).await;
            }
            WorkerToEdge::Progress { applied, inputs } => self.progress(from, applied, &inputs),
            WorkerToEdge::ChunkSnapshot {
                position,
                chunk,
                entities,
                ..
            } => self.take_snapshot(from, position, chunk, entities).await,
            WorkerToEdge::TickDelta { events, .. } => {
                for event in events {
                    self.handle_event(from, event).await;
                }
            }
            // A region says these through its outbox, so that they reach the edge even
            // if the region's owner dies right after.
            message @ (WorkerToEdge::Remote(_)
            | WorkerToEdge::RemoteDone { .. }
            | WorkerToEdge::ToPlayer {
                event: PlayerEvent::Departed(_) | PlayerEvent::Refused,
                ..
            }) => {
                error!(%from, ?message, "a region said outside its outbox what belongs in it");
            }
        }
        None
    }

    /// Handles an entry of the outbox of the region `from` and confirms it. An entry
    /// that was handled before is one the region sends again because the confirmation
    /// had not reached it, and is passed over.
    async fn outbox(&mut self, from: RegionId, number: u64, entry: Durable) {
        let port = &mut self.regions[from.0 as usize];
        if number <= port.seen {
            return;
        }
        port.seen = number;
        match entry {
            Durable::Departed { player, transfer } => {
                self.hand_over(player, from, transfer).await;
            }
            Durable::Refused { player } => {
                // A player who has an entity is in the world through another way than
                // the join that was refused, and stays.
                let waiting = self
                    .players
                    .get(&player)
                    .filter(|view| view.entity.is_none());
                if let Some(view) = waiting {
                    refuse(&view.outbound, "The world cannot take another player.");
                    self.remove_player(player).await;
                }
            }
            Durable::Remote(action) => {
                // The region that has the block the next step is about takes it.
                let to = self.layout.region_of(action.step.concerns().chunk());
                let (player, sequence) = (action.player, action.sequence);
                if let Some(view) = self.players.get_mut(&player) {
                    view.under_way.insert(sequence);
                }
                if to == from {
                    // The region passed on what, by this edge's layout, is its own to
                    // do. Sending it back would have the two go round in circles.
                    error!(%from, "a region passed on an action about one of its own blocks");
                    self.arrived(player, sequence).await;
                } else {
                    self.send_to_region(to, EdgeToWorker::Remote(action)).await;
                }
            }
            Durable::RemoteDone { player, sequence } => self.arrived(player, sequence).await,
            // Of regions that hold chunks and merge and split, which no region does yet
            // (ADR-0010). It is confirmed like any other, so that it does not come back.
            entry @ (Durable::NotMine { .. }
            | Durable::Absorbed { .. }
            | Durable::SplitOff { .. }) => {
                error!(%from, ?entry, "a region said what this edge does not act on yet");
            }
        }
        // Only now: what the entry led to is kept for the regions it concerns, so the
        // region may forget the entry.
        self.send_to_region(from, EdgeToWorker::Confirm { number })
            .await;
    }

    /// The region `from` has said whether it has `player`, whom the edge named in its
    /// hello as one it believes to be there. The entries of the region's outbox that the
    /// edge had not seen came before this, so a player the region let go has been
    /// handed on by now.
    async fn presence(&mut self, from: RegionId, player: PlayerId, answer: Presence) {
        let Some(view) = self.players.get_mut(&player) else {
            return;
        };
        if view.region != from {
            return;
        }
        // The region speaks of the player as of the messages it has applied. If their
        // leaving is still among what it is to be sent again, it speaks of who they
        // were before they left: the player the edge has now joined afterwards, and is
        // placed when the region has applied the leaving and the join behind it. Taking
        // the answer for them would put them into the world as their old self, with an
        // entity that the region removes a moment later.
        let port = &self.regions[from.0 as usize];
        let left_since = port.kept.iter().any(
            |(_, body)| matches!(body, EdgeToWorker::PlayerLeave { player: left } if *left == player),
        );
        if left_since {
            return;
        }
        match answer {
            Presence::Present {
                entity,
                pose,
                hotbar,
                selected_slot,
                last_input,
                handled,
            } => {
                while view
                    .kept_inputs
                    .front()
                    .is_some_and(|(number, ..)| *number <= last_input)
                {
                    view.kept_inputs.pop_front();
                }
                match view.entity {
                    // The region placed the player, and the word of it was lost with
                    // the link.
                    None => {
                        let inventory = inventory_packets(&hotbar, selected_slot);
                        self.spawn_player(player, entity, pose.position, inventory)
                            .await;
                    }
                    Some(shown) if shown != entity => {
                        error!(name = %view.name, %from, "a region has a player as another entity");
                        refuse(&view.outbound, "The server lost track of where you are.");
                        self.remove_player(player).await;
                        return;
                    }
                    Some(_) => {}
                }
                // Through the same holding back as every acknowledgement: an action
                // that is under way elsewhere holds back later ones.
                if let Some(sequence) = handled {
                    self.handled(player, sequence).await;
                }
            }
            Presence::Absent => {
                // A join or an arrival that the region has not applied yet is among
                // what is sent to it again, and puts the player there.
                let port = &self.regions[from.0 as usize];
                let under_way = port.kept.iter().any(|(_, body)| match body {
                    EdgeToWorker::PlayerJoin(join) => join.player == player,
                    EdgeToWorker::PlayerArrive {
                        player: arriving, ..
                    } => *arriving == player,
                    _ => false,
                });
                if !under_way {
                    warn!(name = %view.name, %from, "a region does not have a player it should have");
                    refuse(&view.outbound, "The server lost track of where you are.");
                    self.remove_player(player).await;
                }
            }
        }
    }

    /// The region `from` has applied this edge's messages up to `applied` and made that
    /// durable, and with them the inputs of `inputs` up to the numbers given. None of
    /// that has to be sent again.
    fn progress(&mut self, from: RegionId, applied: u64, inputs: &[(PlayerId, u64)]) {
        let port = &mut self.regions[from.0 as usize];
        port.applied = port.applied.max(applied);
        while port
            .kept
            .front()
            .is_some_and(|(number, _)| *number <= applied)
        {
            port.kept.pop_front();
        }
        for (player, last_input) in inputs {
            let Some(view) = self.players.get_mut(player) else {
                continue;
            };
            while view
                .kept_inputs
                .front()
                .is_some_and(|(number, ..)| number <= last_input)
            {
                view.kept_inputs.pop_front();
            }
        }
    }

    /// Notes that a region has handled everything `player` did to blocks up to
    /// `sequence`, and tells the client as far as it may be told.
    async fn handled(&mut self, player: PlayerId, sequence: i32) {
        let Some(view) = self.players.get_mut(&player) else {
            return;
        };
        view.handled = view.handled.max(Some(sequence));
        self.acknowledge(player).await;
    }

    /// Notes that an action of `player` that concerned another region's blocks has been
    /// dealt with there, and tells the client as far as it may be told.
    async fn arrived(&mut self, player: PlayerId, sequence: i32) {
        let Some(view) = self.players.get_mut(&player) else {
            return;
        };
        // Otherwise it is of a connection the player had before.
        if view.under_way.remove(&sequence) {
            view.handled = view.handled.max(Some(sequence));
            self.acknowledge(player).await;
        }
    }

    /// Tells the client of `player` up to which sequence number its actions on blocks
    /// have been handled, if that is further than it has been told: up to the highest
    /// that a region has reported, but not beyond an action that is still under way.
    async fn acknowledge(&mut self, player: PlayerId) {
        let Some(view) = self.players.get_mut(&player) else {
            return;
        };
        let Some(handled) = view.handled else {
            return;
        };
        let ready = match view.under_way.first() {
            Some(first) => handled.min(first.saturating_sub(1)),
            None => handled,
        };
        if Some(ready) > view.acknowledged {
            view.acknowledged = Some(ready);
            let packet = encoded(&AcknowledgeBlockChange { sequence: ready });
            self.send_to_player(player, [packet]).await;
        }
    }

    /// Passes a player whom the region `from` has let go on to the region they walked
    /// into, together with what they have done since.
    ///
    /// Nothing else is handled while this runs, so no input of the player can go to the
    /// old region after the ones sent again here have been picked, or to the new region
    /// before them.
    async fn hand_over(&mut self, player: PlayerId, from: RegionId, transfer: PlayerTransfer) {
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
            self.send_to_region(to, discard).await;
            return;
        };
        if to == from {
            // The region and this edge disagree about where the region ends. Sending
            // the player back would have them bounce between the two forever.
            error!(name = %view.name, %from, "a region let go of a player who is inside it");
            refuse(&view.outbound, "The server lost track of where you are.");
            self.send_to_region(to, discard).await;
            self.remove_player(player).await;
            return;
        }

        // The old region ignored everything it was sent after letting the player go.
        // What it had applied by then the player takes along; the rest is still kept,
        // because nothing is dropped before a region has reported it applied.
        while view
            .kept_inputs
            .front()
            .is_some_and(|(number, ..)| *number <= transfer.last_input)
        {
            view.kept_inputs.pop_front();
        }
        view.region = to;
        let again: Vec<_> = view
            .kept_inputs
            .iter()
            .map(|(number, _, input)| (*number, input.clone()))
            .collect();
        debug!(name = %view.name, %from, %to, inputs = again.len(), "handing a player over");

        self.send_to_region(to, EdgeToWorker::PlayerArrive { player, transfer })
            .await;
        for (number, input) in again {
            let message = EdgeToWorker::Input {
                player,
                number,
                input,
            };
            self.send_to_region(to, message).await;
        }
        // Normally the view has followed the move already; this covers a player who was
        // let go without having been seen to move, such as one who joined right there.
        self.move_view(player, chunk).await;
    }

    /// Handles something that happened in the region `from`.
    async fn handle_event(&mut self, from: RegionId, event: RegionEvent) {
        match event {
            RegionEvent::EntitySpawned(state) => self.upsert_entity(from, state).await,
            RegionEvent::EntityRemoved { entity, .. } => {
                if self.is_of(entity, from) {
                    self.remove_entity(entity).await;
                }
            }
            RegionEvent::BlockChanged { position, state } => {
                self.change_block(position, state).await;
            }
            RegionEvent::EntityMoved { entity, pose, .. } => {
                if !self.is_of(entity, from) {
                    return;
                }
                // A move of an entity the edge has not been told about yet is covered by
                // the snapshot of the chunk it is in, which is still on its way.
                if let Some(shown) = self.entities.get_mut(&entity) {
                    shown.state.pose = pose;
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

    /// Whether what the region `from` says of `entity` counts: the entity is one that
    /// region introduced last, or one the edge does not show at all.
    fn is_of(&self, entity: EntityId, from: RegionId) -> bool {
        self.entities
            .get(&entity)
            .is_none_or(|shown| shown.from == from)
    }

    /// Takes in an entity the region `from` described, which may be new or already
    /// known. The entity is that region's from now on.
    async fn upsert_entity(&mut self, from: RegionId, state: EntityState) {
        if !self.replica.contains_key(&state.chunk()) {
            // Nobody watches where it is; this can be a snapshot that arrives late.
            return;
        }
        let entity = state.entity;
        let pose = state.pose;
        let moved = self
            .entities
            .insert(entity, Shown { state, from })
            .is_some_and(|known| known.state.pose != pose);
        self.refresh_entity(entity, moved).await;
    }

    /// Brings every player's client up to date about one entity: shows it to those who
    /// have it in view, moves it if `moved`, and hides it from those who lost sight of it.
    async fn refresh_entity(&mut self, entity: EntityId, moved: bool) {
        let Some(state) = self.entities.get(&entity).map(|shown| shown.state.clone()) else {
            return;
        };
        let mut outgoing = Vec::new();
        for (player, view) in &mut self.players {
            let packets = view.update_visibility(*player, &state, moved);
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
        if view.entity.is_some() {
            // Told twice: once as it happened and once on resuming with the region.
            return;
        }
        view.entity = Some(entity_id);
        view.joining_since = None;
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
        for shown in self.entities.values() {
            packets.extend(view.update_visibility(player, &shown.state, false));
        }
        let replica = &self.replica;
        self.entities
            .retain(|_, shown| replica.contains_key(&shown.state.chunk()));

        self.subscribe(subscribe).await;
        self.unsubscribe(unsubscribe).await;
        if self.send_to_player(player, packets).await {
            self.send_chunks(player).await;
        }
    }

    /// Takes in a snapshot of a chunk: the chunk as a region has it, and the entities in
    /// it.
    ///
    /// The first snapshot of a chunk is offered to its viewers. A later one comes when
    /// the edge resumes with the region, after missing what happened meanwhile, and is
    /// reconciled with what the edge shows: a chunk that differs is sent again, and an
    /// entity the edge shows in the chunk that the region does not have there is
    /// removed. The entity of one of this edge's own players is never removed by this:
    /// what becomes of them the region says for each of them.
    async fn take_snapshot(
        &mut self,
        from: RegionId,
        position: ChunkPos,
        chunk: Chunk,
        entities: Vec<EntityState>,
    ) {
        let Some(entry) = self.replica.get_mut(&position) else {
            // Nobody has the chunk in view any more.
            return;
        };
        let shown_before = entry.chunk.is_some();
        if entry.chunk.as_ref() != Some(&chunk) {
            entry.chunk = Some(chunk);
            entry.packet = None;
            let viewers: Vec<_> = self
                .players
                .iter_mut()
                .filter(|(_, view)| view.wanted.contains(&position))
                .map(|(player, view)| {
                    // A client takes a chunk it has already as the chunk's new content.
                    view.sent.remove(&position);
                    view.pending.insert(position);
                    *player
                })
                .collect();
            for player in viewers {
                self.send_chunks(player).await;
            }
        }

        if shown_before {
            let stale: Vec<_> = self
                .entities
                .iter()
                .filter(|(id, shown)| {
                    shown.state.chunk() == position
                        && !self.entity_owners.contains_key(id)
                        && entities.iter().all(|present| present.entity != **id)
                })
                .map(|(id, _)| *id)
                .collect();
            for entity in stale {
                self.remove_entity(entity).await;
            }
        }
        for state in entities {
            self.upsert_entity(from, state).await;
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
    /// has to confirm the previous batch. Returns false if the player was removed.
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

    /// Asks the regions the chunks belong to for them and for what happens in them. A
    /// region without a link is asked when there is one again: the hello names every
    /// chunk of the region that the edge shows or wants.
    async fn subscribe(&mut self, chunks: Vec<ChunkPos>) {
        for (region, chunks) in self.by_region(chunks) {
            self.send_to_region(region, EdgeToWorker::Subscribe { chunks })
                .await;
        }
    }

    /// Tells the regions the chunks belong to that they are no longer needed here.
    async fn unsubscribe(&mut self, chunks: Vec<ChunkPos>) {
        for (region, chunks) in self.by_region(chunks) {
            self.send_to_region(region, EdgeToWorker::Unsubscribe { chunks })
                .await;
        }
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
    /// their connection.
    async fn remove_player(&mut self, player: PlayerId) {
        let Some(view) = self.players.remove(&player) else {
            return;
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
            .retain(|_, shown| replica.contains_key(&shown.state.chunk()));
        self.unsubscribe(unsubscribe).await;
        // If that region has just let the player go, it ignores this, and the message
        // saying so, which is on its way, makes `hand_over` clean up.
        self.send_to_region(view.region, EdgeToWorker::PlayerLeave { player })
            .await;
    }

    fn session_matches(&self, player: PlayerId, session: SessionId) -> bool {
        self.players
            .get(&player)
            .is_some_and(|view| view.session == session)
    }

    /// Sends `body` to a region, or keeps it for when the region can be reached.
    ///
    /// What changes the region is numbered and kept until the region reports it applied
    /// and durable, so that it can be sent again on another link. The rest is only sent
    /// if there is a link: a hello says anew what the edge wants to see and what it has
    /// seen.
    async fn send_to_region(&mut self, region: RegionId, body: EdgeToWorker) {
        let Some(port) = self.regions.get_mut(region.0 as usize) else {
            return;
        };
        let number = body.is_numbered().then(|| {
            port.numbered += 1;
            port.numbered
        });
        if let Some(number) = number {
            port.kept.push_back((number, body.clone()));
        }
        let Some(link) = &port.link else {
            return;
        };
        // Until the region has answered the hello, what is kept waits: it goes out in
        // one piece, in order, once the region has said where it stands.
        if number.is_some() && !link.welcomed {
            return;
        }
        if link
            .sender
            .send(EdgeMessage { number, body })
            .await
            .is_err()
        {
            // The link's reader says so too, when it gets to it. Meanwhile nothing more
            // is sent into the void.
            self.lose_link(region);
        }
    }
}

impl PlayerView {
    /// Works out what the client has to be told about `state` and records it: the
    /// packets that show the entity if it came into view, move it if `moved`, or hide it
    /// if it left the view. `me` is the player whose view this is. A player is never
    /// shown their own entity; the client creates that itself. Nor are they shown an
    /// entity they were before: one who leaves while being handed over and joins again
    /// at once is a new entity, while the old one may still arrive in the region it
    /// was on its way to and be removed there a moment later. A client that is shown
    /// a player with its own UUID has two of itself.
    fn update_visibility(&mut self, me: PlayerId, state: &EntityState, moved: bool) -> Vec<Bytes> {
        let in_world = self.entity.is_some();
        let EntityKind::Player { player: of, .. } = &state.kind;
        let own = self.entity == Some(state.entity) || *of == me;
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
    use clustine_rpc::link::WorkerEnd;
    use clustine_sim::api::{Pose, RemoteAction, RemoteStep};
    use tokio::task::JoinHandle;
    use tokio::time::timeout;
    use uuid::Uuid;

    use super::*;
    use crate::Relinks;

    /// How long a test waits for something that has to happen.
    const SOON: Duration = Duration::from_secs(10);

    /// The world of these tests: a western region up to the chunks with x = 4, where
    /// players enter the world, and an eastern one from there.
    const WEST: RegionId = RegionId(0);
    const EAST: RegionId = RegionId(1);

    const IDENTITY: EdgeIdentity = EdgeIdentity {
        edge: clustine_world::EdgeId(7),
        start: 100,
    };

    /// A fan-out task with the regions played by the test.
    struct Harness {
        commands: mpsc::Sender<Command>,
        relinks: Relinks,
        /// The workers' ends of the links to the two regions.
        regions: Vec<WorkerEnd>,
        task: JoinHandle<Stopped>,
        sessions: u64,
        /// The number of the last outbox entry each region has made.
        outbox: [u64; 2],
    }

    /// A link to `region` whose owner has `epoch`, and the worker's end of it.
    fn link_to(region: RegionId, epoch: u64) -> (RegionLink, WorkerEnd) {
        let (end, worker) = link::in_process(1024);
        (RegionLink { region, epoch, end }, worker)
    }

    impl Harness {
        /// An edge linked to both regions, each of which has been said hello to and has
        /// answered that it does not know the edge, as at the start of everything.
        async fn start() -> Self {
            Self::start_with_patience(Duration::from_secs(3600)).await
        }

        async fn start_with_patience(region_patience: Duration) -> Self {
            let layout = Layout::new(vec![4]).unwrap();
            let (west, west_end) = link_to(WEST, 1);
            let (east, east_end) = link_to(EAST, 1);
            let spawn = Vec3::new(0.5, -60.0, 0.5);
            let (routing, relinks) = Routing::new(layout, spawn, IDENTITY, vec![west, east]);
            let config = FanoutConfig {
                max_players: 20,
                view_distance: 2,
                online: Arc::default(),
                region_patience,
            };
            let (commands, receiver) = mpsc::channel(64);
            let task = tokio::spawn(Fanout::new(config, routing, receiver).run());
            let mut harness = Self {
                commands,
                relinks,
                regions: vec![west_end, east_end],
                task,
                sessions: 0,
                outbox: [0; 2],
            };
            for region in [WEST, EAST] {
                let hello = harness.next(region).await;
                assert!(
                    matches!(
                        &hello.body,
                        EdgeToWorker::Hello { seen: 0, players, chunks, .. }
                            if players.is_empty() && chunks.is_empty()
                    ),
                    "{hello:?}"
                );
                harness.tell(region, WorkerToEdge::Welcome(Welcome::Unknown));
            }
            harness
        }

        /// What the edge sends `region` next.
        async fn next(&mut self, region: RegionId) -> EdgeMessage {
            timeout(SOON, self.regions[region.0 as usize].recv())
                .await
                .expect("the edge sent nothing")
                .expect("the edge closed the link")
        }

        /// What the edge sends `region` next that is numbered, with its number.
        async fn next_numbered(&mut self, region: RegionId) -> (u64, EdgeToWorker) {
            loop {
                let EdgeMessage { number, body } = self.next(region).await;
                if let Some(number) = number {
                    return (number, body);
                }
            }
        }

        /// Says something as `region`.
        fn tell(&self, region: RegionId, message: WorkerToEdge) {
            self.regions[region.0 as usize].try_send(message).unwrap();
        }

        /// Says `entry` as the next entry of the outbox of `region`. Returns its number.
        fn say(&mut self, region: RegionId, entry: Durable) -> u64 {
            let number = &mut self.outbox[region.0 as usize];
            *number += 1;
            let number = *number;
            self.tell(region, WorkerToEdge::Outbox { number, entry });
            number
        }

        /// Waits until the edge has handled everything `region` has said so far. The
        /// edge takes what regions say, what players do and new links from different
        /// queues, in no order between them, so a test that depends on the edge having
        /// heard something makes sure of it with this.
        async fn settle(&mut self, region: RegionId) {
            // An entry about nobody, which the edge confirms like any other.
            let nobody = Durable::RemoteDone {
                player: player(u128::MAX),
                sequence: 0,
            };
            let number = self.say(region, nobody);
            loop {
                let message = self.next(region).await;
                if message.body == (EdgeToWorker::Confirm { number }) {
                    return;
                }
            }
        }

        /// A player connects. Returns what their connection is sent.
        async fn join(&mut self, player: PlayerId) -> mpsc::Receiver<Bytes> {
            self.sessions += 1;
            let (outbound, packets) = mpsc::channel(4096);
            let join = Command::Join {
                session: SessionId(self.sessions),
                profile: Profile {
                    uuid: player.0,
                    name: format!("Player{}", self.sessions),
                },
                requested_view_distance: None,
                outbound,
                awaiting_teleport: Arc::new(AtomicI32::new(NO_TELEPORT)),
            };
            self.commands.send(join).await.unwrap();
            packets
        }

        /// A player joins and the western region places them as `entity`. Everything
        /// the edge sends the region for that has been received when this returns.
        async fn joined(&mut self, player: PlayerId, entity: EntityId) -> mpsc::Receiver<Bytes> {
            let packets = self.join(player).await;
            let (_, join) = self.next_numbered(WEST).await;
            assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
            self.tell(WEST, spawned(player, entity));
            self.settle(WEST).await;
            packets
        }

        /// Something the player with the latest session did.
        async fn input(&mut self, player: PlayerId, input: PlayerInput) {
            let command = Command::Input {
                session: SessionId(self.sessions),
                player,
                input,
            };
            self.commands.send(command).await.unwrap();
        }

        /// Replaces the edge's link to `region` and returns the hello the edge says on
        /// the new one.
        async fn relink(&mut self, region: RegionId, epoch: u64) -> EdgeToWorker {
            let (link, worker) = link_to(region, epoch);
            self.regions[region.0 as usize] = worker;
            assert!(self.relinks.replace(link).await);
            self.next(region).await.body
        }
    }

    fn player(number: u128) -> PlayerId {
        PlayerId(Uuid::from_u128(number))
    }

    fn spawned(player: PlayerId, entity: EntityId) -> WorkerToEdge {
        WorkerToEdge::ToPlayer {
            player,
            event: PlayerEvent::Spawned {
                entity_id: entity,
                position: Vec3::new(0.5, -60.0, 0.5),
                hotbar: [None; HOTBAR_SLOTS],
                selected_slot: 0,
            },
        }
    }

    fn step(x: f64) -> PlayerInput {
        PlayerInput::Move {
            position: Some(Vec3::new(x, -60.0, 0.5)),
            rotation: None,
            on_ground: true,
        }
    }

    /// The player as the western region lets them go, standing in the eastern one.
    fn transfer(entity: EntityId, last_input: u64) -> PlayerTransfer {
        PlayerTransfer {
            entity_id: entity,
            name: "Player1".to_owned(),
            pose: Pose::at(Vec3::new(64.5, -60.0, 0.5)),
            hotbar: [None; HOTBAR_SLOTS],
            selected_slot: 0,
            last_input,
        }
    }

    fn present(entity: EntityId, last_input: u64) -> Presence {
        Presence::Present {
            entity,
            pose: Pose::at(Vec3::new(0.5, -60.0, 0.5)),
            hotbar: [None; HOTBAR_SLOTS],
            selected_slot: 0,
            last_input,
            handled: None,
        }
    }

    /// Waits until the player's connection is ended by the edge.
    async fn disconnected(packets: &mut mpsc::Receiver<Bytes>) {
        let ended = async { while packets.recv().await.is_some() {} };
        timeout(SOON, ended)
            .await
            .expect("the player was not disconnected");
    }

    /// Whether the edge still serves the player: their queue is open.
    fn connected(packets: &mut mpsc::Receiver<Bytes>) -> bool {
        loop {
            match packets.try_recv() {
                Ok(_) => {}
                Err(mpsc::error::TryRecvError::Empty) => return true,
                Err(mpsc::error::TryRecvError::Disconnected) => return false,
            }
        }
    }

    #[tokio::test]
    async fn what_changes_a_region_is_numbered_from_one_per_region() {
        let mut edge = Harness::start().await;
        let _packets = edge.joined(player(1), EntityId(5)).await;

        edge.input(player(1), step(1.5)).await;
        let (number, body) = edge.next_numbered(WEST).await;
        assert_eq!(number, 2);
        assert!(
            matches!(body, EdgeToWorker::Input { number: 1, .. }),
            "{body:?}"
        );

        // The eastern region has been sent nothing numbered yet, so its first is 1.
        let departed = Durable::Departed {
            player: player(1),
            transfer: transfer(EntityId(5), 1),
        };
        edge.say(WEST, departed);
        let (number, body) = edge.next_numbered(EAST).await;
        assert_eq!(number, 1);
        assert!(
            matches!(body, EdgeToWorker::PlayerArrive { .. }),
            "{body:?}"
        );
    }

    /// The heart of resuming: the players stay, the hello says what the edge believes
    /// and has seen, and what the region has not reported applied is sent again with the
    /// numbers it had, once the region has answered.
    #[tokio::test]
    async fn a_lost_link_keeps_the_players_and_a_new_one_resumes() {
        let mut edge = Harness::start().await;
        let mut packets = edge.joined(player(1), EntityId(5)).await;
        // The join is applied and durable; two inputs follow that are not yet.
        edge.tell(
            WEST,
            WorkerToEdge::Progress {
                applied: 1,
                inputs: Vec::new(),
            },
        );
        edge.settle(WEST).await;
        edge.input(player(1), step(1.5)).await;
        edge.input(player(1), step(2.5)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 2);
        assert_eq!(edge.next_numbered(WEST).await.0, 3);

        // The worker dies. An input made meanwhile is kept like the others.
        let hello = {
            let (link, worker) = link_to(WEST, 2);
            drop(std::mem::replace(&mut edge.regions[0], worker));
            edge.input(player(1), step(3.5)).await;
            assert!(edge.relinks.replace(link).await);
            edge.next(WEST).await.body
        };
        let EdgeToWorker::Hello {
            edge: id,
            start,
            seen,
            players,
            chunks,
        } = hello
        else {
            panic!("expected a hello, got {hello:?}");
        };
        // What it has seen of the region's outbox are the entries that settled it.
        assert_eq!(
            (id, start, seen),
            (IDENTITY.edge, IDENTITY.start, edge.outbox[0])
        );
        assert_eq!(players, [player(1)]);
        // Every chunk of the region that the edge shows or wants, and none of the other
        // region's.
        let layout = Layout::new(vec![4]).unwrap();
        assert!(!chunks.is_empty());
        assert!(chunks.iter().all(|chunk| layout.region_of(*chunk) == WEST));
        assert!(connected(&mut packets));

        edge.tell(WEST, WorkerToEdge::Welcome(Welcome::Resumed));
        for expected in [2, 3, 4] {
            let (number, body) = edge.next_numbered(WEST).await;
            assert_eq!(number, expected);
            assert!(matches!(body, EdgeToWorker::Input { .. }), "{body:?}");
        }
        edge.tell(
            WEST,
            WorkerToEdge::Presence {
                player: player(1),
                answer: present(EntityId(5), 0),
            },
        );

        // What comes next carries on from there, and the player has stayed throughout.
        edge.input(player(1), step(4.5)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 5);
        assert!(connected(&mut packets));
    }

    #[tokio::test]
    async fn what_a_region_reported_applied_is_not_sent_again() {
        let mut edge = Harness::start().await;
        let _packets = edge.joined(player(1), EntityId(5)).await;
        edge.input(player(1), step(1.5)).await;
        edge.input(player(1), step(2.5)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 2);
        assert_eq!(edge.next_numbered(WEST).await.0, 3);
        let progress = WorkerToEdge::Progress {
            applied: 2,
            inputs: vec![(player(1), 1)],
        };
        edge.tell(WEST, progress);
        edge.settle(WEST).await;

        edge.input(player(1), step(3.5)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 4);
        let hello = edge.relink(WEST, 2).await;
        assert!(matches!(hello, EdgeToWorker::Hello { .. }), "{hello:?}");
        edge.tell(WEST, WorkerToEdge::Welcome(Welcome::Resumed));
        assert_eq!(edge.next_numbered(WEST).await.0, 3);
        assert_eq!(edge.next_numbered(WEST).await.0, 4);
    }

    /// A region that has forgotten the edge expects its messages numbered from 1. The
    /// edge must not send what it kept with the old numbers, which would close the
    /// link, and the players it believed to be there are not.
    #[tokio::test]
    async fn a_region_that_forgot_the_edge_gets_nothing_that_was_kept() {
        let mut edge = Harness::start().await;
        let mut packets = edge.joined(player(1), EntityId(5)).await;
        edge.tell(
            WEST,
            WorkerToEdge::Progress {
                applied: 1,
                inputs: Vec::new(),
            },
        );
        edge.settle(WEST).await;
        edge.input(player(1), step(1.5)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 2);

        let hello = edge.relink(WEST, 2).await;
        assert!(matches!(hello, EdgeToWorker::Hello { .. }), "{hello:?}");
        edge.tell(WEST, WorkerToEdge::Welcome(Welcome::Unknown));
        disconnected(&mut packets).await;

        // All the region hears is that the player is gone, numbered from 1 again.
        let (number, body) = edge.next_numbered(WEST).await;
        assert_eq!(number, 1);
        assert!(matches!(body, EdgeToWorker::PlayerLeave { .. }), "{body:?}");
        // And a later hello claims nothing of the past.
        let hello = edge.relink(WEST, 3).await;
        assert!(
            matches!(&hello, EdgeToWorker::Hello { seen: 0, players, .. } if players.is_empty()),
            "{hello:?}"
        );
    }

    /// An entry of a region's outbox is handled once, however often it is sent, and
    /// confirmed after what it led to is kept for the region it concerns.
    #[tokio::test]
    async fn an_outbox_entry_is_handled_once_and_confirmed() {
        let mut edge = Harness::start().await;
        let mut packets = edge.joined(player(1), EntityId(5)).await;
        let departed = || Durable::Departed {
            player: player(1),
            transfer: transfer(EntityId(5), 0),
        };

        let number = edge.say(WEST, departed());
        let (arrival, body) = edge.next_numbered(EAST).await;
        assert_eq!(arrival, 1);
        assert!(
            matches!(body, EdgeToWorker::PlayerArrive { .. }),
            "{body:?}"
        );
        let confirm = loop {
            let message = edge.next(WEST).await;
            if matches!(message.body, EdgeToWorker::Confirm { .. }) {
                break message;
            }
        };
        assert_eq!(
            confirm,
            EdgeMessage::unnumbered(EdgeToWorker::Confirm { number })
        );

        // Sent again, as after a lost confirmation: nobody arrives a second time. The
        // next numbered message the east gets is the player's next input.
        edge.tell(
            WEST,
            WorkerToEdge::Outbox {
                number,
                entry: departed(),
            },
        );
        edge.settle(WEST).await;
        edge.input(player(1), step(65.5)).await;
        let (number, body) = edge.next_numbered(EAST).await;
        assert_eq!(number, 2);
        assert!(matches!(body, EdgeToWorker::Input { .. }), "{body:?}");
        assert!(connected(&mut packets));
    }

    /// The order of a resume matters: the outbox comes before the presence answers, so
    /// a player the region let go while the link was down has been handed on by the
    /// time the region says that it does not have them.
    #[tokio::test]
    async fn a_departure_missed_with_the_link_hands_the_player_on_before_presence_is_judged() {
        let mut edge = Harness::start().await;
        let mut packets = edge.joined(player(1), EntityId(5)).await;
        edge.tell(
            WEST,
            WorkerToEdge::Progress {
                applied: 1,
                inputs: Vec::new(),
            },
        );
        edge.settle(WEST).await;
        edge.input(player(1), step(64.5)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 2);

        let hello = edge.relink(WEST, 2).await;
        assert!(
            matches!(&hello, EdgeToWorker::Hello { players, .. } if *players == [player(1)]),
            "{hello:?}"
        );
        edge.tell(WEST, WorkerToEdge::Welcome(Welcome::Resumed));
        let departed = Durable::Departed {
            player: player(1),
            transfer: transfer(EntityId(5), 1),
        };
        edge.say(WEST, departed);
        edge.tell(
            WEST,
            WorkerToEdge::Presence {
                player: player(1),
                answer: Presence::Absent,
            },
        );

        let (_, body) = edge.next_numbered(EAST).await;
        assert!(
            matches!(body, EdgeToWorker::PlayerArrive { .. }),
            "{body:?}"
        );
        edge.settle(WEST).await;
        assert!(connected(&mut packets));
    }

    #[tokio::test]
    async fn a_player_a_region_does_not_have_is_disconnected_unless_they_are_on_their_way_there() {
        let mut edge = Harness::start().await;
        // The first is in the region as far as the edge knows, and the region has
        // reported the join applied. The second has asked to join, which is still kept.
        let mut settled = edge.joined(player(1), EntityId(5)).await;
        edge.tell(
            WEST,
            WorkerToEdge::Progress {
                applied: 1,
                inputs: Vec::new(),
            },
        );
        edge.settle(WEST).await;
        let mut joining = edge.join(player(2)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 2);

        let hello = edge.relink(WEST, 2).await;
        assert!(
            matches!(&hello, EdgeToWorker::Hello { players, .. } if players.len() == 2),
            "{hello:?}"
        );
        edge.tell(WEST, WorkerToEdge::Welcome(Welcome::Resumed));
        for absent in [player(1), player(2)] {
            edge.tell(
                WEST,
                WorkerToEdge::Presence {
                    player: absent,
                    answer: Presence::Absent,
                },
            );
        }
        disconnected(&mut settled).await;
        edge.settle(WEST).await;
        assert!(connected(&mut joining));
    }

    /// A player the region placed while the link was down is told so on resuming, from
    /// what the region says of them.
    #[tokio::test]
    async fn a_player_placed_unnoticed_enters_the_world_on_resuming() {
        let mut edge = Harness::start().await;
        let mut packets = edge.join(player(1)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 1);

        let hello = edge.relink(WEST, 2).await;
        assert!(matches!(hello, EdgeToWorker::Hello { .. }), "{hello:?}");
        edge.tell(WEST, WorkerToEdge::Welcome(Welcome::Resumed));
        edge.tell(
            WEST,
            WorkerToEdge::Presence {
                player: player(1),
                answer: present(EntityId(5), 0),
            },
        );

        // Entering the world makes the edge ask for the chunks around.
        loop {
            let message = edge.next(WEST).await;
            if matches!(message.body, EdgeToWorker::Subscribe { .. }) {
                break;
            }
        }
        assert!(timeout(SOON, packets.recv()).await.unwrap().is_some());
        // The spawn the region may still report for them changes nothing.
        edge.tell(WEST, spawned(player(1), EntityId(5)));
        edge.settle(WEST).await;
        edge.input(player(1), step(1.5)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 2);
        assert!(connected(&mut packets));
    }

    /// A player leaves and joins again while the region has not yet applied their
    /// leaving, as when its worker has just died. What the region then says of the
    /// player is about who they were; the one who joined again is placed by the join.
    #[tokio::test]
    async fn what_a_region_says_of_a_player_who_left_and_came_back_since_is_not_taken_for_them() {
        let mut edge = Harness::start().await;
        let _old = edge.joined(player(1), EntityId(5)).await;
        let leave = Command::Leave {
            session: SessionId(edge.sessions),
            player: player(1),
        };
        edge.commands.send(leave).await.unwrap();
        let (number, left) = edge.next_numbered(WEST).await;
        assert_eq!(
            (number, left),
            (2, EdgeToWorker::PlayerLeave { player: player(1) })
        );
        let mut packets = edge.join(player(1)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 3);
        // The first join is durable; the leaving and the second join are not yet.
        edge.tell(
            WEST,
            WorkerToEdge::Progress {
                applied: 1,
                inputs: Vec::new(),
            },
        );
        edge.settle(WEST).await;

        // The region's owner is replaced by one that has applied neither, and still
        // has the player as they were.
        let hello = edge.relink(WEST, 2).await;
        assert!(matches!(hello, EdgeToWorker::Hello { .. }), "{hello:?}");
        edge.tell(WEST, WorkerToEdge::Welcome(Welcome::Resumed));
        edge.tell(
            WEST,
            WorkerToEdge::Presence {
                player: player(1),
                answer: present(EntityId(5), 40),
            },
        );
        let (number, left) = edge.next_numbered(WEST).await;
        assert_eq!(
            (number, left),
            (2, EdgeToWorker::PlayerLeave { player: player(1) })
        );
        let (number, join) = edge.next_numbered(WEST).await;
        assert!(
            number == 3 && matches!(join, EdgeToWorker::PlayerJoin(_)),
            "{join:?}"
        );
        edge.settle(WEST).await;
        // They have not been put into the world as who they were.
        assert_eq!(
            packets.try_recv().err(),
            Some(mpsc::error::TryRecvError::Empty)
        );

        // The region applies both and places them anew.
        edge.tell(WEST, spawned(player(1), EntityId(6)));
        assert!(timeout(SOON, packets.recv()).await.unwrap().is_some());
        edge.tell(
            WEST,
            WorkerToEdge::Progress {
                applied: 3,
                inputs: Vec::new(),
            },
        );
        edge.settle(WEST).await;

        // And it is as that entity that the edge knows them from then on.
        let hello = edge.relink(WEST, 3).await;
        assert!(matches!(hello, EdgeToWorker::Hello { .. }), "{hello:?}");
        edge.tell(WEST, WorkerToEdge::Welcome(Welcome::Resumed));
        edge.tell(
            WEST,
            WorkerToEdge::Presence {
                player: player(1),
                answer: present(EntityId(6), 0),
            },
        );
        edge.settle(WEST).await;
        assert!(connected(&mut packets));
    }

    #[tokio::test]
    async fn an_edge_that_has_been_superseded_stops() {
        let mut edge = Harness::start().await;
        let mut packets = edge.joined(player(1), EntityId(5)).await;
        let hello = edge.relink(EAST, 2).await;
        assert!(matches!(hello, EdgeToWorker::Hello { .. }), "{hello:?}");
        edge.tell(EAST, WorkerToEdge::Welcome(Welcome::Superseded));
        assert_eq!(
            timeout(SOON, &mut edge.task).await.unwrap().unwrap(),
            Stopped::Superseded
        );
        disconnected(&mut packets).await;
    }

    #[tokio::test]
    async fn a_link_to_a_replaced_owner_is_not_taken() {
        let mut edge = Harness::start().await;
        let hello = edge.relink(WEST, 5).await;
        assert!(matches!(hello, EdgeToWorker::Hello { .. }), "{hello:?}");
        edge.tell(WEST, WorkerToEdge::Welcome(Welcome::Unknown));

        // The link of an owner with a lower epoch is dropped, which closes it.
        let (stale, mut worker) = link_to(WEST, 4);
        assert!(edge.relinks.replace(stale).await);
        assert_eq!(timeout(SOON, worker.recv()).await.unwrap(), None);
        // The link the edge has is still the one it uses.
        let _packets = edge.join(player(1)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 1);
    }

    /// What a player of one region does to blocks of another is passed on to the region
    /// that has them.
    #[tokio::test]
    async fn a_remote_action_is_passed_on_to_the_region_that_has_the_block() {
        let mut edge = Harness::start().await;
        let _packets = edge.joined(player(1), EntityId(5)).await;
        let action = RemoteAction {
            player: player(1),
            sequence: 3,
            step: RemoteStep::Break {
                position: BlockPos::new(64, -61, 0),
            },
        };
        edge.say(WEST, Durable::Remote(action.clone()));
        let (number, body) = edge.next_numbered(EAST).await;
        assert_eq!((number, body), (1, EdgeToWorker::Remote(action)));
    }

    /// A player whose region does not confirm what they do is not kept for ever.
    #[tokio::test(start_paused = true)]
    async fn a_player_whose_region_stays_silent_is_disconnected_after_the_patience_is_over() {
        let patience = Duration::from_secs(20);
        let mut edge = Harness::start_with_patience(patience).await;
        let mut waiting = edge.joined(player(1), EntityId(5)).await;
        let mut content = edge.joined(player(2), EntityId(6)).await;
        // One of them does something the region never reports applied.
        edge.sessions = 1;
        edge.input(player(1), step(1.5)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 3);

        tokio::time::sleep(patience / 2).await;
        assert!(connected(&mut waiting));
        tokio::time::sleep(patience).await;
        disconnected(&mut waiting).await;
        // Who has nothing outstanding stays, however long the region is silent.
        assert!(connected(&mut content));
    }

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
