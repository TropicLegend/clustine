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
use clustine_region::RegionId;
use clustine_rpc::link;
use clustine_rpc::{EdgeMessage, EdgeToWorker, Presence, Welcome, WorkerToEdge};
use clustine_sim::api::{
    Durable, EntityKind, EntityState, HOTBAR_SLOTS, ItemStack, Misdirected, PlayerEvent,
    PlayerInput, PlayerJoin, PlayerTransfer, RegionEvent, RemoteAction,
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
    /// The region whose snapshot the replica has. It stays when that region's link
    /// ends, and goes when the edge has no subscription there any more or the region
    /// says that it does not hold the chunk.
    served_by: Option<RegionId>,
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
    /// How often a region has sent the player on because it believes another to hold
    /// the chunk they stand in, since a region last applied something of theirs.
    passed_on: u32,
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
    /// messages numbered from 1, and would close the link over the gap. Nor is it sent
    /// before the outbox entries that the welcome announced have been handled: one of
    /// them can change what is kept (`docs/adr/0013-the-edge-without-a-layout.md`,
    /// section 6).
    welcomed: bool,
    /// How many of the outbox entries the welcome announced are still to come; `None`
    /// until the welcome has been read.
    announced: Option<u32>,
    /// The number of the last subscription message sent over this link. A region takes
    /// them only in ascending order; see `docs/adr/0012-the-tick-on-chunks.md`,
    /// section 4.3.
    asked: u64,
}

/// What the edge keeps for a region, with or without a link to it; see
/// `docs/adr/0008-durable-regions-and-resuming.md`. A region gets one when the edge
/// first has to do with it: when a link to it comes, or something is to be sent to it
/// (`docs/adr/0013-the-edge-without-a-layout.md`, section 1).
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
    /// The `since` of the last welcome read from the region, 0 if none: the region's
    /// word for the numbering the two share, which the edge says in every hello. The
    /// region resumes only if it is the one it has; see
    /// `docs/adr/0012-the-tick-on-chunks.md`, section 5.3.
    since: u64,
    /// The edge's subscriptions at the region, by chunk.
    subscriptions: BTreeMap<ChunkPos, Subscription>,
}

/// What a region takes a subscription of this edge for; see
/// `docs/adr/0013-the-edge-without-a-layout.md`, section 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A viewer whose player is the region's sees the chunk: the region takes the chunk
    /// if nobody holds it, and keeps it.
    Viewer,
    /// Only viewers of other regions' players see it: the region serves the chunk if it
    /// holds it.
    Guest,
}

/// How the region has answered a subscription on the current link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Condition {
    /// Not yet.
    Waiting,
    /// With a snapshot: every later event of the chunk comes from this region.
    Served,
    /// The region named holds the chunk.
    Elsewhere(RegionId),
}

/// The edge's subscription to a chunk at a region.
#[derive(Debug, Clone)]
struct Subscription {
    kind: Kind,
    /// The number of the edge's last message on the current link that named the chunk;
    /// 0 for one the hello named, and while the region has no link.
    ask: u64,
    /// The number of the message that made the subscription on the current link, or
    /// asked the region again; 0 for one the hello named. A snapshot numbered from here
    /// on is this subscription's, whatever message has changed its kind since.
    begun: u64,
    condition: Condition,
    /// How many viewers whose player the edge believes to be this region's see the
    /// chunk. The subscription is a viewer's exactly while this is above 0.
    viewers: u32,
    /// When the region was last asked again after a guest's region said it does not
    /// hold the chunk, and whether another asking is due.
    asked_again: Option<Instant>,
    again_due: bool,
}

impl Subscription {
    fn new(kind: Kind, viewers: u32) -> Self {
        Self {
            kind,
            ask: 0,
            begun: 0,
            condition: Condition::Waiting,
            viewers,
            asked_again: None,
            again_due: false,
        }
    }

    /// As it is on a link that has just begun: named by the hello, with no answer.
    fn begin_anew(&mut self) {
        self.ask = 0;
        self.begun = 0;
        self.condition = Condition::Waiting;
        self.again_due = false;
    }
}

/// A subscription message that is still to be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Asking {
    Unsubscribe,
    AsGuest,
    Subscribe,
}

/// How long the edge waits before it asks a region again for a chunk that the region
/// named as its holder says it does not hold: two regions that each name the other
/// must not keep the edge busy.
const ASK_AGAIN_EVERY: Duration = Duration::from_secs(1);

/// What reaches the fan-out task from a link to a region: a message, or `None` when the
/// link has ended.
type FromLink = (RegionId, u64, Option<WorkerToEdge>);

pub(crate) struct Fanout {
    config: FanoutConfig,
    /// The region players enter the world in.
    spawn_region: RegionId,
    /// Who this edge is to the regions.
    identity: EdgeIdentity,
    /// What is kept for each region, by region id.
    regions: BTreeMap<RegionId, RegionPort>,
    /// The links to start with, until [`Fanout::run`] takes them up.
    first_links: Vec<RegionLink>,
    /// Where new links to regions arrive.
    relinks: mpsc::Receiver<RegionLink>,
    /// Where it is said that a link has ended, for whoever makes the links.
    lost: mpsc::UnboundedSender<(RegionId, u64)>,
    /// The number the next link gets.
    next_link: u64,
    /// Subscription messages that the turn under way has made and not sent yet, in the
    /// order they were made: the region, what to say, the chunk, and whether it makes
    /// the subscription or asks again. See [`Fanout::flush_asking`].
    asking: Vec<(RegionId, Asking, ChunkPos, bool)>,
    /// Whether a subscription, or who sees what, has changed since the statements about
    /// subscriptions were last checked, and when that was. They are checked at the end
    /// of a turn that changed something: in the edge's own tests every time, and in
    /// other builds with debug assertions ten times a second at most, as going through
    /// every subscription takes longer than an edge with many viewers has between two
    /// messages. A resume alone is hundreds of answers.
    changed: bool,
    checked: Instant,
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
    pub(crate) fn new(
        config: FanoutConfig,
        routing: Routing,
        commands: mpsc::Receiver<Command>,
    ) -> Self {
        let spawn_region = routing.home;
        // The messages of all regions are read from one queue, each link's in the order
        // they were sent.
        let (queue, messages) = mpsc::channel(REGION_QUEUE_CAPACITY);
        Self {
            config,
            spawn_region,
            identity: routing.identity,
            regions: BTreeMap::new(),
            first_links: routing.links,
            relinks: routing.relinks,
            lost: routing.lost,
            next_link: 0,
            asking: Vec::new(),
            changed: false,
            checked: Instant::now(),
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
                    let current = self.regions.entry(region).or_default()
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
                _ = patience.tick() => {
                    self.drop_the_overdue().await;
                    self.ask_again_where_due().await;
                }
            }
            let due = cfg!(test) || self.checked.elapsed() >= Duration::from_millis(100);
            if self.changed && due && cfg!(debug_assertions) {
                self.changed = false;
                self.checked = Instant::now();
                self.check_subscriptions();
            }
        }
    }

    /// Holds the subscriptions to the three statements of
    /// `docs/adr/0013-the-edge-without-a-layout.md`, section 1, which are to hold at
    /// the end of every turn. Only in builds with debug assertions, which every test
    /// run is.
    fn check_subscriptions(&self) {
        assert!(self.asking.is_empty(), "a turn ended with messages unsent");
        // V: a subscription's viewers are the players of its region that see the chunk,
        // and it is a viewer's exactly while there are any.
        let mut seen: BTreeMap<(RegionId, ChunkPos), u32> = BTreeMap::new();
        for view in self.players.values() {
            for chunk in &view.wanted {
                *seen.entry((view.region, *chunk)).or_default() += 1;
            }
        }
        for (region, port) in &self.regions {
            for (chunk, subscription) in &port.subscriptions {
                let viewers = seen.remove(&(*region, *chunk)).unwrap_or(0);
                assert_eq!(subscription.viewers, viewers, "{region} {chunk:?}");
                assert_eq!(
                    subscription.kind == Kind::Viewer,
                    viewers > 0,
                    "{region} {chunk:?} {subscription:?}"
                );
                // G: a guest's subscription only for a chunk that someone sees.
                let watched = self
                    .replica
                    .get(chunk)
                    .is_some_and(|entry| entry.viewers > 0);
                assert!(
                    subscription.kind == Kind::Viewer || watched,
                    "a guest's subscription nobody sees: {region} {chunk:?}"
                );
                // E: who was told that another region holds the chunk is subscribed
                // there, unless it is about to ask again.
                if let Condition::Elsewhere(holder) = subscription.condition {
                    let there = self.regions.get(&holder);
                    assert!(
                        subscription.again_due
                            || there.is_some_and(|port| port.subscriptions.contains_key(chunk)),
                        "{region} was told that {holder} holds {chunk:?}, where nothing is asked"
                    );
                }
            }
        }
        assert!(seen.is_empty(), "viewers without a subscription: {seen:?}");
    }

    /// Notes that the link to `region` has ended. Its players stay, and what they do is
    /// kept, until there is a link to the region again.
    fn lose_link(&mut self, region: RegionId) {
        self.changed = true;
        let port = self.regions.entry(region).or_default();
        if let Some(link) = port.link.take() {
            warn!(%region, epoch = link.epoch, "the link to a region ended; keeping its players");
            // Nobody may be listening, which is fine.
            let _ = self.lost.send((region, link.epoch));
        }
        // Whatever the region had answered, the next link begins with nothing: its
        // hello names every subscription, and the answers come under the number 0.
        port.subscriptions
            .values_mut()
            .for_each(Subscription::begin_anew);
    }

    /// Makes `link` the edge's link to its region and begins to resume with the region:
    /// says who this edge is and what it knows of the region. What was kept for the
    /// region is sent once the region has answered; see [`Fanout::welcomed`].
    async fn take_link(&mut self, link: RegionLink) {
        self.changed = true;
        let RegionLink { region, epoch, end } = link;
        let port = self.regions.entry(region).or_default();
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
        // the world there, and every subscription it has there, by its kind: those the
        // region said another region holds as well, as it may have been restored since
        // and asks the store again. A link that is replaced while it still stands
        // begins with nothing like one that ended.
        let players = self
            .players
            .iter()
            .filter(|(_, view)| view.region == region)
            .map(|(player, _)| *player)
            .collect();
        port.subscriptions
            .values_mut()
            .for_each(Subscription::begin_anew);
        let of_kind = |kind| {
            let named = port.subscriptions.iter();
            let named = named.filter(move |(_, subscription)| subscription.kind == kind);
            named.map(|(chunk, _)| *chunk).collect::<Vec<_>>()
        };
        let chunks = of_kind(Kind::Viewer);
        let guests = of_kind(Kind::Guest);
        let hello = EdgeToWorker::Hello {
            edge: self.identity.edge,
            start: self.identity.start,
            since: port.since,
            seen: port.seen,
            players,
            chunks,
            guests,
        };
        info!(%region, epoch, kept = port.kept.len(), "linked to a region");
        // A link that is gone already is noticed by its reader.
        let _ = sender.send(EdgeMessage::unnumbered(hello)).await;
        port.link = Some(Link {
            sender,
            epoch,
            id,
            welcomed: false,
            announced: None,
            asked: 0,
        });
    }

    /// The region `from` has said what it knows of this edge. Returns why the edge has
    /// to stop, if it has to.
    async fn welcomed(&mut self, from: RegionId, welcome: Welcome) -> Option<Stopped> {
        let port = &self.regions.entry(from).or_default();
        match welcome {
            Welcome::Superseded => {
                error!(%from, "another edge has taken this one's name; stopping");
                return Some(Stopped::Superseded);
            }
            Welcome::Resumed { .. } => {}
            Welcome::Unknown { since, .. } => {
                // To an edge that has had nothing to do with the region this is how
                // everything begins, and what it kept for the region is numbered as
                // the region expects it: from 1.
                if port.applied != 0 || port.seen != 0 {
                    self.forget_region(from).await;
                }
                // Said in every hello from now on. Kept also when the link ends before
                // anything more is read: the region tells an edge that says another
                // number the same again, as long as it has taken nothing from it.
                self.regions.entry(from).or_default().since = since;
            }
        }
        let entries = match welcome {
            Welcome::Resumed { entries } | Welcome::Unknown { entries, .. } => entries,
            Welcome::Superseded => 0,
        };
        let link = self.regions.entry(from).or_default().link.as_mut()?;
        link.announced = Some(entries);
        if entries == 0 {
            self.send_kept(from).await;
        }
        None
    }

    /// Sends the region `region` what was kept for it, now that it has said where it
    /// stands and the outbox entries its welcome announced have been handled.
    async fn send_kept(&mut self, region: RegionId) {
        let port = &mut self.regions.entry(region).or_default();
        let Some(link) = port.link.as_mut() else {
            return;
        };
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
    }

    /// Takes an entry of the outbox of the region `from`, and counts it against what the
    /// region's welcome announced: when the last of those has been handled, what was
    /// kept for the region is sent.
    async fn outbox(&mut self, from: RegionId, number: u64, entry: Durable) {
        self.handle_entry(from, number, entry).await;
        let link = self.regions.entry(from).or_default().link.as_mut();
        let Some(link) = link.filter(|link| !link.welcomed) else {
            return;
        };
        // An entry the edge had handled before counts as well: the region announced it.
        if let Some(left) = &mut link.announced {
            *left = left.saturating_sub(1);
            if *left == 0 {
                self.send_kept(from).await;
            }
        }
    }

    /// The region `region` has forgotten this edge, which stayed away from it for too
    /// long: the players the edge believed to be there are not, and what it kept for the
    /// region means nothing to it any more.
    async fn forget_region(&mut self, region: RegionId) {
        warn!(%region, "a region has forgotten this edge; dropping what was kept for it");
        let port = &mut self.regions.entry(region).or_default();
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
                        passed_on: 0,
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
            WorkerToEdge::Elsewhere { chunk, ask, region } => {
                self.elsewhere(from, chunk, ask, region).await;
            }
            WorkerToEdge::NotMine { chunk, ask } => self.not_mine(from, chunk, ask).await,
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
                ask,
                chunk,
                entities,
                ..
            } => {
                if self.served(from, position, ask) {
                    self.take_snapshot(from, position, chunk, entities).await;
                }
            }
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
    async fn handle_entry(&mut self, from: RegionId, number: u64, entry: Durable) {
        let port = &mut self.regions.entry(from).or_default();
        if number <= port.seen {
            return;
        }
        port.seen = number;
        match entry {
            Durable::Departed {
                player,
                transfer,
                to,
            } => {
                self.hand_over(player, from, to, transfer).await;
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
            Durable::Remote { action, to } => {
                // The region the entry names takes the next step. Where it names none,
                // the region did not know who holds the chunk: the one that serves
                // this edge the chunk does, as a client can only have clicked a block
                // of a chunk some region sent it.
                let chunk = action.step.concerns().chunk();
                let serves = self.replica.get(&chunk).and_then(|entry| entry.served_by);
                self.pass_on(from, to.or(serves), action).await;
            }
            // A remote action reached a region that does not hold the chunk and
            // believes another to.
            Durable::NotMine {
                what: Misdirected::Remote(action),
                holder,
            } => self.pass_on(from, Some(holder), action).await,
            // A player was let go to a region that believes another to hold the chunk
            // they stand in: they go on to that one, as if this region had let them go.
            Durable::NotMine {
                what: Misdirected::Arrival { player, transfer },
                holder,
            } => {
                let passed = self.players.get_mut(&player).map(|view| {
                    view.passed_on += 1;
                    view.passed_on
                });
                // Beliefs form no ring, so this ends after fewer steps than there are
                // regions (ADR-0012, rule 20). Should they ever, the player is not
                // sent round for ever.
                if passed.is_some_and(|passed| passed as usize > self.regions.len()) {
                    error!(%from, %holder, "a player is passed from region to region without end");
                    if let Some(view) = self.players.get(&player) {
                        refuse(&view.outbound, "The server lost track of where you are.");
                    }
                    let chunk =
                        ChunkPos::containing(transfer.pose.position.x, transfer.pose.position.z);
                    let discard = EdgeToWorker::Discard {
                        entity: transfer.entity_id,
                        chunk,
                    };
                    self.send_to_region(holder, discard).await;
                    self.remove_player(player).await;
                } else {
                    self.hand_over(player, from, holder, transfer).await;
                }
            }
            Durable::RemoteDone { player, sequence } => self.arrived(player, sequence).await,
            // Of regions that merge and split, which no region does yet (ADR-0010). It
            // is confirmed like any other, so that it does not come back.
            entry @ (Durable::Absorbed { .. } | Durable::SplitOff { .. }) => {
                error!(%from, ?entry, "a region said what this edge does not act on yet");
            }
        }
        // Only now: what the entry led to is kept for the regions it concerns, so the
        // region may forget the entry.
        self.send_to_region(from, EdgeToWorker::Confirm { number })
            .await;
    }

    /// Passes what is left of a player's action on blocks on to the region `to`, which
    /// the region `from` named or which serves this edge the chunk. With nowhere to
    /// send it, or only back where it came from, the action ends here: its player is
    /// told that it was handled, and sees the block as it is.
    async fn pass_on(&mut self, from: RegionId, to: Option<RegionId>, action: RemoteAction) {
        let (player, sequence) = (action.player, action.sequence);
        let Some(view) = self.players.get_mut(&player) else {
            // Nobody is left to be told how it ended.
            return;
        };
        view.under_way.insert(sequence);
        match to.filter(|to| *to != from) {
            Some(to) => self.send_to_region(to, EdgeToWorker::Remote(action)).await,
            None => {
                debug!(%from, ?to, "an action on blocks has nowhere to go and ends here");
                self.arrived(player, sequence).await;
            }
        }
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
        let port = &self.regions.entry(from).or_default();
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
                let port = &self.regions.entry(from).or_default();
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
        let port = &mut self.regions.entry(from).or_default();
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
            // A region has applied something of theirs: they have arrived.
            view.passed_on = 0;
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

    /// Passes a player whom the region `from` has let go on to `to`, the region it named
    /// as the one they walked into, together with what they have done since.
    ///
    /// Nothing else is handled while this runs, so no input of the player can go to the
    /// old region after the ones sent again here have been picked, or to the new region
    /// before them.
    async fn hand_over(
        &mut self,
        player: PlayerId,
        from: RegionId,
        to: RegionId,
        transfer: PlayerTransfer,
    ) {
        let position = transfer.pose.position;
        let chunk = ChunkPos::containing(position.x, position.z);
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
        if view.region != from {
            // About an earlier stay of the player in that region: they have been passed
            // on from there since.
            error!(name = %view.name, %from, now = %view.region, "a region let go of a player who is not its own");
            return;
        }
        if to == from {
            // The region named itself as the one the player walked into, which none
            // does. Sending the player back would have them bounce there forever.
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

        // What the player sees is asked of their region from now on. On the link to
        // `to` this is before the arrival, so that one claim of that region covers the
        // chunk the player arrives in and their view.
        let seen: Vec<ChunkPos> = view.wanted.iter().copied().collect();
        for chunk in &seen {
            self.unwant(from, *chunk);
        }
        for chunk in &seen {
            self.want(to, *chunk);
        }
        self.flush_asking().await;

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

        let region = view.region;
        let mut left = Vec::new();
        for position in view.wanted.difference(&wanted) {
            left.push(*position);
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
            }
        }
        let mut entered = Vec::new();
        for position in wanted.difference(&view.wanted) {
            entered.push(*position);
            let chunk = self
                .replica
                .entry(*position)
                .or_insert_with(|| ReplicaChunk {
                    viewers: 0,
                    chunk: None,
                    packet: None,
                    served_by: None,
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

        for chunk in left {
            self.unwant(region, chunk);
        }
        for chunk in entered {
            self.want(region, chunk);
        }
        self.flush_asking().await;
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
    /// removed, if it was this region that introduced it. The entity of one of this
    /// edge's own players is never removed by this: what becomes of them the region
    /// says for each of them. Nor is an entity that another region introduced: a
    /// player of one region can stand in a chunk another holds while the store says
    /// whose it is, and the holder's snapshot does not have them
    /// (`docs/adr/0012-the-tick-on-chunks.md`, rule 32).
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
                        && shown.from == from
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

    /// A viewer whose player is `region`'s has `chunk` in view from now on. The first
    /// such viewer makes the edge's subscription there a viewer's: a new one, or a
    /// guest's that changes its kind and keeps what the region has answered.
    fn want(&mut self, region: RegionId, chunk: ChunkPos) {
        self.changed = true;
        let port = self.regions.entry(region).or_default();
        match port.subscriptions.get_mut(&chunk) {
            None => {
                let subscription = Subscription::new(Kind::Viewer, 1);
                port.subscriptions.insert(chunk, subscription);
                self.asking.push((region, Asking::Subscribe, chunk, true));
            }
            Some(subscription) => {
                subscription.viewers += 1;
                if subscription.viewers == 1 {
                    subscription.kind = Kind::Viewer;
                    self.asking.push((region, Asking::Subscribe, chunk, false));
                }
            }
        }
    }

    /// A viewer whose player is `region`'s no longer has `chunk` in view, or the player
    /// is no longer that region's. The replica has counted the viewer out already if
    /// they no longer see the chunk at all.
    ///
    /// When it was the last such viewer and someone still sees the chunk, the
    /// subscription becomes a guest's, **whatever the region has answered so far**:
    /// one that serves the chunk goes on serving it, and one that waits is answered.
    /// Ending it would leave a viewer of another region, whom that region told that
    /// this one holds the chunk, looking at a chunk nobody serves. Only a subscription
    /// that was told the chunk is held elsewhere is ended: it carried nothing. When
    /// nobody sees the chunk any more, every subscription to it is ended, at every
    /// region.
    fn unwant(&mut self, region: RegionId, chunk: ChunkPos) {
        self.changed = true;
        let watched = self
            .replica
            .get(&chunk)
            .is_some_and(|entry| entry.viewers > 0);
        let port = self.regions.entry(region).or_default();
        let Some(subscription) = port.subscriptions.get_mut(&chunk) else {
            error!(%region, ?chunk, "a viewer saw a chunk that nothing was asked for");
            return;
        };
        subscription.viewers = subscription.viewers.saturating_sub(1);
        if subscription.viewers == 0 {
            if watched && !matches!(subscription.condition, Condition::Elsewhere(_)) {
                subscription.kind = Kind::Guest;
                self.asking.push((region, Asking::AsGuest, chunk, false));
            } else {
                self.end(region, chunk);
            }
        }
        if !watched {
            let everywhere: Vec<_> = self
                .regions
                .iter()
                .filter(|(_, port)| port.subscriptions.contains_key(&chunk))
                .map(|(region, _)| *region)
                .collect();
            for region in everywhere {
                self.end(region, chunk);
            }
        }
    }

    /// Ends the edge's subscription to `chunk` at `region`.
    fn end(&mut self, region: RegionId, chunk: ChunkPos) {
        let port = self.regions.entry(region).or_default();
        if port.subscriptions.remove(&chunk).is_some() {
            self.asking
                .push((region, Asking::Unsubscribe, chunk, false));
        }
        if let Some(entry) = self.replica.get_mut(&chunk)
            && entry.served_by == Some(region)
        {
            entry.served_by = None;
        }
    }

    /// Sends the subscription messages the turn has made, each with the next number of
    /// its link, and notes that number with the subscriptions it names.
    ///
    /// They go in the order they were made, each run of chunks for one region and of
    /// one kind as one message. Where no chunk is named twice for a region, which is
    /// nearly always, the messages for a region are gathered by kind instead, so that a
    /// view that moves is a few messages and not one for every chunk; what is said
    /// twice of one subscription within a turn has to keep its order.
    async fn flush_asking(&mut self) {
        let mut asking = std::mem::take(&mut self.asking);
        let mut named = BTreeSet::new();
        if asking
            .iter()
            .all(|(region, _, chunk, _)| named.insert((*region, *chunk)))
        {
            // Stable: within a region and a kind the order of the chunks stays.
            asking.sort_by_key(|(region, what, ..)| (*region, *what));
        }
        let mut rest = asking.as_slice();
        while let Some((region, what, ..)) = rest.first().copied() {
            let length = rest
                .iter()
                .take_while(|(of, kind, ..)| (*of, *kind) == (region, what))
                .count();
            let (run, behind) = rest.split_at(length);
            rest = behind;
            let ask = self.next_ask(region);
            let port = self.regions.entry(region).or_default();
            for (_, _, chunk, begins) in run {
                // One that was ended later in this turn is not there any more.
                if what != Asking::Unsubscribe
                    && let Some(subscription) = port.subscriptions.get_mut(chunk)
                {
                    subscription.ask = ask;
                    if *begins {
                        subscription.begun = ask;
                    }
                }
            }
            let chunks = run.iter().map(|(_, _, chunk, _)| *chunk).collect();
            let body = match what {
                Asking::Subscribe => EdgeToWorker::Subscribe { ask, chunks },
                Asking::AsGuest => EdgeToWorker::SubscribeAsGuest { ask, chunks },
                Asking::Unsubscribe => EdgeToWorker::Unsubscribe { ask, chunks },
            };
            self.send_to_region(region, body).await;
        }
    }

    /// Whether a snapshot of `chunk` from `region` numbered `ask` is one the edge takes:
    /// it has a subscription there, and the snapshot answers that subscription, not one
    /// that was ended before it. If so, the region serves the chunk from now on.
    ///
    /// The number may be below that of the edge's last message about the chunk: a
    /// message that changes the kind of a subscription the region has made the snapshot
    /// for already is not answered, and the snapshot in flight carries the number from
    /// before.
    fn served(&mut self, region: RegionId, chunk: ChunkPos, ask: u64) -> bool {
        let port = self.regions.entry(region).or_default();
        let Some(subscription) = port.subscriptions.get_mut(&chunk) else {
            return false;
        };
        if ask < subscription.begun {
            return false;
        }
        subscription.condition = Condition::Served;
        subscription.again_due = false;
        if let Some(entry) = self.replica.get_mut(&chunk) {
            entry.served_by = Some(region);
        }
        true
    }

    /// The region `from` says that `holder` holds `chunk`, in answer to a viewer's
    /// subscription: the edge asks there as a guest, unless it is subscribed there
    /// already. The viewer's subscription stays, as it is why `from` goes on knowing
    /// who holds the chunk.
    async fn elsewhere(&mut self, from: RegionId, chunk: ChunkPos, ask: u64, holder: RegionId) {
        self.changed = true;
        let port = self.regions.entry(from).or_default();
        let Some(subscription) = port.subscriptions.get_mut(&chunk) else {
            return;
        };
        // About the subscription as it was before the edge's last message, or about a
        // guest's, which is not answered so.
        if subscription.kind != Kind::Viewer || ask != subscription.ask {
            return;
        }
        if holder == from {
            error!(%from, ?chunk, "a region said that it holds a chunk elsewhere: itself");
            return;
        }
        subscription.condition = Condition::Elsewhere(holder);
        subscription.again_due = false;
        if let Some(entry) = self.replica.get_mut(&chunk)
            && entry.served_by == Some(from)
        {
            entry.served_by = None;
        }
        let there = self.regions.entry(holder).or_default();
        if let std::collections::btree_map::Entry::Vacant(free) = there.subscriptions.entry(chunk) {
            free.insert(Subscription::new(Kind::Guest, 0));
            self.asking.push((holder, Asking::AsGuest, chunk, true));
        }
        self.flush_asking().await;
    }

    /// The region `from` says that it does not hold `chunk`, in answer to a guest's
    /// subscription, which is over with that. Whoever was told that `from` holds the
    /// chunk is asked again.
    async fn not_mine(&mut self, from: RegionId, chunk: ChunkPos, ask: u64) {
        self.changed = true;
        let port = self.regions.entry(from).or_default();
        let current = port.subscriptions.get(&chunk).is_some_and(|subscription| {
            subscription.kind == Kind::Guest && ask == subscription.ask
        });
        if !current {
            return;
        }
        port.subscriptions.remove(&chunk);
        if let Some(entry) = self.replica.get_mut(&chunk)
            && entry.served_by == Some(from)
        {
            entry.served_by = None;
        }
        let now = Instant::now();
        for (region, port) in &mut self.regions {
            let Some(subscription) = port.subscriptions.get_mut(&chunk) else {
                continue;
            };
            if subscription.kind != Kind::Viewer
                || subscription.condition != Condition::Elsewhere(from)
            {
                continue;
            }
            let lately = subscription
                .asked_again
                .is_some_and(|asked| now.duration_since(asked) < ASK_AGAIN_EVERY);
            if lately {
                // The check that the task makes every second asks then.
                subscription.again_due = true;
            } else {
                subscription.condition = Condition::Waiting;
                subscription.asked_again = Some(now);
                self.asking.push((*region, Asking::Subscribe, chunk, true));
            }
        }
        self.flush_asking().await;
    }

    /// Asks again where a region was asked again less than a second before the last
    /// answer that called for it.
    async fn ask_again_where_due(&mut self) {
        let now = Instant::now();
        for (region, port) in &mut self.regions {
            for (chunk, subscription) in &mut port.subscriptions {
                if subscription.again_due {
                    subscription.again_due = false;
                    subscription.condition = Condition::Waiting;
                    subscription.asked_again = Some(now);
                    self.asking.push((*region, Asking::Subscribe, *chunk, true));
                }
            }
        }
        self.flush_asking().await;
    }

    /// The number of the next subscription message to `region`: they are counted per
    /// link, from 1. Without a link the message is not sent, and its number means
    /// nothing: the hello of the next link names every chunk anew.
    fn next_ask(&mut self, region: RegionId) -> u64 {
        let link = self.regions.entry(region).or_default().link.as_mut();
        link.map_or(0, |link| {
            link.asked += 1;
            link.asked
        })
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
        for position in &view.wanted {
            let chunk = self
                .replica
                .get_mut(position)
                .expect("viewed chunks are in the replica");
            chunk.viewers -= 1;
            if chunk.viewers == 0 {
                self.replica.remove(position);
            }
        }
        let replica = &self.replica;
        self.entities
            .retain(|_, shown| replica.contains_key(&shown.state.chunk()));
        for position in &view.wanted {
            self.unwant(view.region, *position);
        }
        self.flush_asking().await;
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
        // A region the edge has only just heard of gets its port here: what is numbered
        // is kept for it until the routing table brings its link.
        let port = self.regions.entry(region).or_default();
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
            let (west, west_end) = link_to(WEST, 1);
            let (east, east_end) = link_to(EAST, 1);
            let spawn = Vec3::new(0.5, -60.0, 0.5);
            // Players enter the world in the west.
            let (routing, relinks) = Routing::new(WEST, spawn, IDENTITY, vec![west, east]);
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
                harness.tell(
                    region,
                    WorkerToEdge::Welcome(Welcome::Unknown {
                        since: 1,
                        entries: 0,
                    }),
                );
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

    /// A region takes the subscription messages of a link only in ascending order of
    /// their numbers, which the edge counts per link, from 1. Everything a player sees
    /// is asked of the player's region; another region is asked as a guest when the
    /// player's region names it.
    #[tokio::test]
    async fn subscription_messages_are_numbered_from_one_on_each_link() {
        /// What the edge says next to `region` about its subscriptions.
        async fn next_asked(edge: &mut Harness, region: RegionId) -> EdgeToWorker {
            loop {
                let body = edge.next(region).await.body;
                if let EdgeToWorker::Subscribe { .. }
                | EdgeToWorker::SubscribeAsGuest { .. }
                | EdgeToWorker::Unsubscribe { .. } = body
                {
                    return body;
                }
            }
        }
        // The region says that the player has walked a chunk east, to `x`.
        let walked = |x: f64| WorkerToEdge::TickDelta {
            tick: 1,
            events: vec![RegionEvent::EntityMoved {
                entity: EntityId(5),
                pose: Pose::at(Vec3::new(x, -60.0, 0.5)),
                previous_chunk: ChunkPos::containing(x - 16.0, 0.5),
            }],
        };

        // Entering the world makes the edge ask the player's region for the chunks
        // around.
        let mut edge = Harness::start().await;
        let _packets = edge.join(player(1)).await;
        // Only once the edge has taken the join can it be told where the player is.
        let (_, join) = edge.next_numbered(WEST).await;
        assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
        edge.tell(WEST, spawned(player(1), EntityId(5)));
        let asked = next_asked(&mut edge, WEST).await;
        assert!(
            matches!(asked, EdgeToWorker::Subscribe { ask: 1, .. }),
            "{asked:?}"
        );

        // A column of chunks leaves the view and another comes into it: both are said
        // to the player's region, whoever holds the chunks.
        edge.tell(WEST, walked(16.5));
        let asked = next_asked(&mut edge, WEST).await;
        assert!(
            matches!(asked, EdgeToWorker::Unsubscribe { ask: 2, .. }),
            "{asked:?}"
        );
        let asked = next_asked(&mut edge, WEST).await;
        let EdgeToWorker::Subscribe { ask: 3, chunks } = asked else {
            panic!("{asked:?}");
        };
        // The region names the eastern one as the holder of one of them: the edge asks
        // there as a guest, with that link's own count.
        let chunk = chunks[0];
        let elsewhere = WorkerToEdge::Elsewhere {
            chunk,
            ask: 3,
            region: EAST,
        };
        edge.tell(WEST, elsewhere);
        let asked = next_asked(&mut edge, EAST).await;
        assert_eq!(
            asked,
            EdgeToWorker::SubscribeAsGuest {
                ask: 1,
                chunks: vec![chunk]
            }
        );

        // On a new link the count begins anew; on the other it goes on.
        let hello = edge.relink(WEST, 2).await;
        let EdgeToWorker::Hello {
            chunks: named,
            guests,
            ..
        } = hello
        else {
            panic!("{hello:?}");
        };
        // The hello names what the player sees, the chunk held elsewhere among it.
        assert!(
            named.contains(&chunk) && guests.is_empty(),
            "{named:?} {guests:?}"
        );
        edge.tell(WEST, walked(32.5));
        let asked = next_asked(&mut edge, WEST).await;
        assert!(
            matches!(asked, EdgeToWorker::Unsubscribe { ask: 1, .. }),
            "{asked:?}"
        );
        let asked = next_asked(&mut edge, WEST).await;
        let EdgeToWorker::Subscribe { ask: 2, chunks } = asked else {
            panic!("{asked:?}");
        };
        let next = chunks[0];
        let elsewhere = WorkerToEdge::Elsewhere {
            chunk: next,
            ask: 2,
            region: EAST,
        };
        edge.tell(WEST, elsewhere);
        let asked = next_asked(&mut edge, EAST).await;
        assert_eq!(
            asked,
            EdgeToWorker::SubscribeAsGuest {
                ask: 2,
                chunks: vec![next]
            }
        );
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
            to: EAST,
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
            since,
            seen,
            players,
            chunks,
            guests,
        } = hello
        else {
            panic!("expected a hello, got {hello:?}");
        };
        // What it has seen of the region's outbox are the entries that settled it.
        assert_eq!(
            (id, start, seen),
            (IDENTITY.edge, IDENTITY.start, edge.outbox[0])
        );
        // And it says since when the region knows it, as the region's welcome told it.
        assert_eq!(since, 1);
        assert_eq!(players, [player(1)]);
        // Every chunk the region's player sees, whoever serves it: the region has not
        // said of any that another holds it. And the edge is a guest nowhere.
        let seen: Vec<_> = view_area(ChunkPos::new(0, 0), 2).into_iter().collect();
        assert_eq!(chunks, seen);
        assert!(guests.is_empty());
        assert!(connected(&mut packets));

        edge.tell(WEST, WorkerToEdge::Welcome(Welcome::Resumed { entries: 0 }));
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
        edge.tell(WEST, WorkerToEdge::Welcome(Welcome::Resumed { entries: 0 }));
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
        edge.tell(
            WEST,
            WorkerToEdge::Welcome(Welcome::Unknown {
                since: 1,
                entries: 0,
            }),
        );
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
            to: EAST,
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
        edge.tell(WEST, WorkerToEdge::Welcome(Welcome::Resumed { entries: 0 }));
        let departed = Durable::Departed {
            player: player(1),
            transfer: transfer(EntityId(5), 1),
            to: EAST,
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
        edge.tell(WEST, WorkerToEdge::Welcome(Welcome::Resumed { entries: 0 }));
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
        edge.tell(WEST, WorkerToEdge::Welcome(Welcome::Resumed { entries: 0 }));
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
        edge.tell(WEST, WorkerToEdge::Welcome(Welcome::Resumed { entries: 0 }));
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
        edge.tell(WEST, WorkerToEdge::Welcome(Welcome::Resumed { entries: 0 }));
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

    /// An empty chunk, as a region sends it in answer to a subscription numbered `ask`.
    fn snapshot(position: ChunkPos, ask: u64) -> WorkerToEdge {
        let overworld = clustine_data::DIMENSION_TYPES
            .iter()
            .find(|dimension| dimension.name == OVERWORLD)
            .expect("the overworld is a dimension");
        WorkerToEdge::ChunkSnapshot {
            position,
            ask,
            tick: 1,
            chunk: Chunk::empty(overworld, clustine_world::Biome(0)),
            entities: Vec::new(),
        }
    }

    /// What the edge says next to `region` about its subscriptions.
    async fn next_asked(edge: &mut Harness, region: RegionId) -> EdgeToWorker {
        loop {
            let body = edge.next(region).await.body;
            if let EdgeToWorker::Subscribe { .. }
            | EdgeToWorker::SubscribeAsGuest { .. }
            | EdgeToWorker::Unsubscribe { .. } = body
            {
                return body;
            }
        }
    }

    /// A chunk that someone at the spawn point sees, and still sees from the first
    /// chunk of the eastern region.
    const SHARED: ChunkPos = ChunkPos::new(2, 0);

    /// The number under which the edge asked the western region for the view of a
    /// player who has just entered the world.
    async fn first_asked(edge: &mut Harness) -> u64 {
        let EdgeToWorker::Subscribe { ask, chunks } = next_asked(edge, WEST).await else {
            panic!("the edge did not ask for the view");
        };
        assert!(chunks.contains(&SHARED), "{chunks:?}");
        ask
    }

    /// The outbox entry with which the western region lets a player go to the eastern.
    fn departing_to_east(player: PlayerId, entity: EntityId) -> Durable {
        Durable::Departed {
            player,
            transfer: transfer(entity, 0),
            to: EAST,
        }
    }

    /// A snapshot answers the subscription it was made for, whatever message has changed
    /// the subscription's kind since: a region that has made the snapshot does not
    /// answer the change, so an edge that took only an answer with its latest number
    /// would wait for ever. This is the edge as a guest at the east, and its player
    /// handed there while the snapshot is in flight.
    #[tokio::test]
    async fn a_snapshot_in_flight_is_taken_when_the_subscriptions_kind_has_changed_since() {
        let mut edge = Harness::start().await;
        let mut packets = edge.join(player(1)).await;
        let (_, join) = edge.next_numbered(WEST).await;
        assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
        edge.tell(WEST, spawned(player(1), EntityId(5)));
        let ask = first_asked(&mut edge).await;
        let chunk = SHARED;

        // The west says that the east holds the chunk; the edge asks there as a guest.
        let elsewhere = WorkerToEdge::Elsewhere {
            chunk,
            ask,
            region: EAST,
        };
        edge.tell(WEST, elsewhere);
        let asked = next_asked(&mut edge, EAST).await;
        let as_guest = EdgeToWorker::SubscribeAsGuest {
            ask: 1,
            chunks: vec![chunk],
        };
        assert_eq!(asked, as_guest);

        // The player is handed to the east before the east's snapshot is read: the
        // guest's subscription becomes a viewer's, under a later number.
        edge.say(WEST, departing_to_east(player(1), EntityId(5)));
        let asked = next_asked(&mut edge, EAST).await;
        let EdgeToWorker::Subscribe { ask: later, chunks } = asked else {
            panic!("{asked:?}");
        };
        assert!(later > 1 && chunks.contains(&chunk), "{later} {chunks:?}");

        // The snapshot made for the guest's subscription arrives, and is shown.
        while packets.try_recv().is_ok() {}
        edge.tell(EAST, snapshot(chunk, 1));
        edge.settle(EAST).await;
        assert!(packets.try_recv().is_ok(), "the chunk was not sent on");
    }

    /// Two players of two regions see one chunk, which the first one's region serves
    /// and the second one's region says is held there. When the first player goes, the
    /// subscription at their region becomes a guest's instead of ending: it is what
    /// the second player's chunk comes from.
    #[tokio::test]
    async fn a_chunk_another_regions_viewer_still_sees_stays_subscribed_as_a_guests() {
        let mut edge = Harness::start().await;
        // The first enters the world in the west and is handed to the east.
        let _first = edge.joined(player(1), EntityId(5)).await;
        edge.say(WEST, departing_to_east(player(1), EntityId(5)));
        let asked = next_asked(&mut edge, EAST).await;
        let EdgeToWorker::Subscribe { ask, chunks } = asked else {
            panic!("{asked:?}");
        };
        let chunk = SHARED;
        assert!(chunks.contains(&chunk), "{chunks:?}");
        edge.tell(EAST, snapshot(chunk, ask));
        edge.settle(EAST).await;

        // The second enters in the west and sees the same chunk; the west names the
        // east, where the edge is subscribed already and asks nothing more.
        let mut second = edge.join(player(2)).await;
        let (_, join) = edge.next_numbered(WEST).await;
        assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
        edge.tell(WEST, spawned(player(2), EntityId(6)));
        let asked = loop {
            // Past what the hand-over of the first left for the west to hear.
            match next_asked(&mut edge, WEST).await {
                EdgeToWorker::Subscribe { ask, chunks } if chunks.contains(&chunk) => break ask,
                _ => {}
            }
        };
        let elsewhere = WorkerToEdge::Elsewhere {
            chunk,
            ask: asked,
            region: EAST,
        };
        edge.tell(WEST, elsewhere);
        edge.settle(WEST).await;

        // The first leaves. The east is not told to forget the chunk.
        let leave = Command::Leave {
            session: SessionId(1),
            player: player(1),
        };
        edge.commands.send(leave).await.unwrap();
        loop {
            match next_asked(&mut edge, EAST).await {
                EdgeToWorker::SubscribeAsGuest { chunks, .. } if chunks.contains(&chunk) => break,
                EdgeToWorker::Unsubscribe { chunks, .. } => {
                    assert!(!chunks.contains(&chunk), "the chunk was given up");
                }
                _ => {}
            }
        }
        assert!(connected(&mut second));
    }

    /// A region the edge has no link to yet is kept for like any other: what is to go
    /// to it waits for its link, and is not dropped.
    #[tokio::test]
    async fn what_is_sent_to_a_region_without_a_link_yet_is_kept_for_it() {
        let mut edge = Harness::start().await;
        let _packets = edge.joined(player(1), EntityId(5)).await;
        let third = RegionId(2);
        let action = RemoteAction {
            player: player(1),
            sequence: 3,
            step: RemoteStep::Break {
                position: BlockPos::new(640, -61, 0),
            },
        };
        let entry = Durable::Remote {
            action: action.clone(),
            to: Some(third),
        };
        edge.say(WEST, entry);
        edge.settle(WEST).await;

        // The routing table brings the region: after its welcome it is sent what waited.
        let (link, mut worker) = link_to(third, 1);
        assert!(edge.relinks.replace(link).await);
        let hello = timeout(SOON, worker.recv()).await.unwrap().unwrap();
        assert!(
            matches!(hello.body, EdgeToWorker::Hello { .. }),
            "{hello:?}"
        );
        let welcome = Welcome::Unknown {
            since: 1,
            entries: 0,
        };
        worker.try_send(WorkerToEdge::Welcome(welcome)).unwrap();
        let kept = timeout(SOON, worker.recv()).await.unwrap().unwrap();
        assert_eq!(kept.number, Some(1));
        assert_eq!(kept.body, EdgeToWorker::Remote(action));
    }

    /// What was kept for a region is sent again only when the outbox entries that the
    /// region's welcome announced have been handled: one of them can change what is
    /// kept.
    #[tokio::test]
    async fn what_was_kept_is_sent_after_the_entries_the_welcome_announced() {
        let mut edge = Harness::start().await;
        let _packets = edge.joined(player(1), EntityId(5)).await;
        edge.input(player(1), step(1.5)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 2);

        let hello = edge.relink(WEST, 2).await;
        assert!(matches!(hello, EdgeToWorker::Hello { .. }), "{hello:?}");
        edge.tell(WEST, WorkerToEdge::Welcome(Welcome::Resumed { entries: 2 }));
        let done = |sequence| Durable::RemoteDone {
            player: player(u128::MAX),
            sequence,
        };
        // The first entry is confirmed, and nothing that was kept has been sent.
        let first = edge.say(WEST, done(1));
        let message = edge.next(WEST).await;
        assert_eq!(message.body, EdgeToWorker::Confirm { number: first });
        // With the second, what was kept follows its confirmation, in order.
        let second = edge.say(WEST, done(2));
        let message = edge.next(WEST).await;
        assert_eq!(message.body, EdgeToWorker::Confirm { number: second });
        let (number, join) = edge.next_numbered(WEST).await;
        assert!(
            number == 1 && matches!(join, EdgeToWorker::PlayerJoin(_)),
            "{join:?}"
        );
        assert_eq!(edge.next_numbered(WEST).await.0, 2);
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
        edge.tell(
            WEST,
            WorkerToEdge::Welcome(Welcome::Unknown {
                since: 1,
                entries: 0,
            }),
        );

        // The link of an owner with a lower epoch is dropped, which closes it.
        let (stale, mut worker) = link_to(WEST, 4);
        assert!(edge.relinks.replace(stale).await);
        assert_eq!(timeout(SOON, worker.recv()).await.unwrap(), None);
        // The link the edge has is still the one it uses.
        let _packets = edge.join(player(1)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 1);
    }

    /// What is left of a player's action on blocks goes to the region the entry names.
    /// Where it names none, the region did not know who holds the chunk, and the edge
    /// sends it to the region that serves it the chunk; with nobody serving it, the
    /// action ends at the edge.
    #[tokio::test]
    async fn an_action_on_blocks_goes_to_the_region_named_or_to_the_one_that_serves_the_chunk() {
        let mut edge = Harness::start().await;
        let mut packets = edge.join(player(1)).await;
        let (_, join) = edge.next_numbered(WEST).await;
        assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
        edge.tell(WEST, spawned(player(1), EntityId(5)));
        let ask = first_asked(&mut edge).await;
        // About a block of the chunk both regions' players can see.
        let action = |sequence| RemoteAction {
            player: player(1),
            sequence,
            step: RemoteStep::Break {
                position: BlockPos::new(40, -61, 0),
            },
        };
        let without_a_region = |sequence| Durable::Remote {
            action: action(sequence),
            to: None,
        };

        // The region names the one that takes the next step.
        let named = Durable::Remote {
            action: action(3),
            to: Some(EAST),
        };
        edge.say(WEST, named);
        let (number, body) = edge.next_numbered(EAST).await;
        assert_eq!((number, body), (1, EdgeToWorker::Remote(action(3))));

        // It names none, and no region serves the edge that chunk: the action ends
        // here, and nothing goes to the east (the next message there is numbered 2).
        edge.say(WEST, without_a_region(4));
        edge.settle(WEST).await;

        // The east serves the chunk: the west says so, the edge asks there as a guest
        // and is sent the chunk.
        let elsewhere = WorkerToEdge::Elsewhere {
            chunk: SHARED,
            ask,
            region: EAST,
        };
        edge.tell(WEST, elsewhere);
        let asked = next_asked(&mut edge, EAST).await;
        assert!(
            matches!(asked, EdgeToWorker::SubscribeAsGuest { .. }),
            "{asked:?}"
        );
        edge.tell(EAST, snapshot(SHARED, 1));
        edge.settle(EAST).await;
        edge.say(WEST, without_a_region(5));
        let (number, body) = edge.next_numbered(EAST).await;
        assert_eq!((number, body), (2, EdgeToWorker::Remote(action(5))));

        // A region that was passed the action and does not hold the chunk names who
        // does. Had that been the west itself, it would have ended at the edge.
        let not_mine = Durable::NotMine {
            what: Misdirected::Remote(action(6)),
            holder: EAST,
        };
        edge.say(WEST, not_mine);
        let (number, body) = edge.next_numbered(EAST).await;
        assert_eq!((number, body), (3, EdgeToWorker::Remote(action(6))));
        assert!(connected(&mut packets));
    }

    /// A player who is let go to a region that believes another to hold the chunk they
    /// stand in is sent on to that one, as if the region had let them go, also back to
    /// where they came from. One who is sent from region to region without end is
    /// disconnected.
    #[tokio::test]
    async fn a_player_a_region_sends_on_goes_to_the_region_it_names() {
        let mut edge = Harness::start().await;
        let mut packets = edge.joined(player(1), EntityId(5)).await;
        edge.say(WEST, departing_to_east(player(1), EntityId(5)));
        let (number, body) = edge.next_numbered(EAST).await;
        assert!(
            number == 1 && matches!(body, EdgeToWorker::PlayerArrive { .. }),
            "{body:?}"
        );
        let sends_on = |holder| Durable::NotMine {
            what: Misdirected::Arrival {
                player: player(1),
                transfer: transfer(EntityId(5), 0),
            },
            holder,
        };

        // The east sends them back; the west is sent the arrival, behind the join.
        edge.say(EAST, sends_on(WEST));
        let (number, body) = edge.next_numbered(WEST).await;
        assert!(
            number == 2 && matches!(body, EdgeToWorker::PlayerArrive { .. }),
            "{body:?}"
        );
        assert!(connected(&mut packets));

        // The two go on sending them to each other: after as many times as there are
        // regions, the edge gives up.
        edge.say(WEST, sends_on(EAST));
        let (_, body) = edge.next_numbered(EAST).await;
        assert!(
            matches!(body, EdgeToWorker::PlayerArrive { .. }),
            "{body:?}"
        );
        edge.say(EAST, sends_on(WEST));
        disconnected(&mut packets).await;
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
