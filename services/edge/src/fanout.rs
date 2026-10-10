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
use crate::{EdgeIdentity, RegionLink, Relink, Routing, Stopped};

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
    /// How many of the presence answers the welcome announced are still to come;
    /// `None` until the welcome has been read.
    presences: Option<u32>,
    /// The players this welcome itself made the region's and for whom no answer has
    /// said that they are there. They are judged when the answers are through; see
    /// `docs/adr/0015-the-edge-through-merges-and-splits.md`, section 2.2.
    brought: BTreeSet<PlayerId>,
    /// The port's `owes` as it was when the hello of this link was said. Only of these
    /// does the end of the welcome's entries say that their `Absorbed` is not coming:
    /// a hello said before the edge knew of a merge can have been answered from
    /// before it.
    owed: BTreeSet<RegionId>,
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
    /// The entries of this region's outbox that came to it with a merge: for each, by
    /// its number here, the region whose entry it was and the number it had there. An
    /// entry is taken out when it has been handled or passed over, and all of them
    /// wherever `seen` is put back to 0.
    came_with: BTreeMap<u64, (RegionId, u64)>,
    /// The regions the routing table says went into this one, of which the edge still
    /// has something and for which it has handled no `Absorbed`.
    owes: BTreeSet<RegionId>,
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
    relinks: mpsc::Receiver<Relink>,
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
    /// The regions that are no more, each with the region it went into, as far as this
    /// edge has acted on it. Never a chain: a value is not itself a key. Written by
    /// [`Fanout::retire`] and by nothing else; see
    /// `docs/adr/0015-the-edge-through-merges-and-splits.md`, section 1.
    stands_for: BTreeMap<RegionId, RegionId>,
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
            stands_for: BTreeMap::new(),
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
                Some(relink) = self.relinks.recv() => match relink {
                    Relink::Link(link) => self.take_link(link).await,
                    Relink::Absorbed(pairs) => self.take_pairs(pairs).await,
                },
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
        // S: nothing the edge keeps is under a region that is no more, or names one.
        for (absorbed, into) in &self.stands_for {
            assert!(
                !self.stands_for.contains_key(into),
                "{absorbed} stands for {into}, which is no more itself"
            );
            assert!(
                self.has_nothing_of(*absorbed),
                "something is left under {absorbed}, which is no more"
            );
            let port = self.regions.get(absorbed);
            assert!(
                port.is_none_or(|port| port.link.is_none()),
                "a link to {absorbed}, which is no more"
            );
        }
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
        if self.stands_for.contains_key(&region) {
            debug!(%region, epoch, "not taking a link to a region that is no more");
            return;
        }
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
            presences: None,
            brought: BTreeSet::new(),
            owed: port.owes.clone(),
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
        let (entries, presences, applied) = match welcome {
            Welcome::Resumed {
                entries,
                presences,
                applied,
            }
            | Welcome::Unknown {
                entries,
                presences,
                applied,
                ..
            } => (entries, presences, applied),
            Welcome::Superseded => (0, 0, 0),
        };
        debug!(%from, entries, presences, applied, "a region has welcomed this edge");
        // How far the region had applied this edge's messages in the state its entries
        // and its presence answers are made of. None of that is sent again, and what
        // is still kept tells an answer about a player who is entering the world from
        // one about who they were before: see `presence`.
        self.progress(from, applied, &[]);
        let link = self.regions.entry(from).or_default().link.as_mut()?;
        link.announced = Some(entries);
        link.presences = Some(presences);
        if entries == 0 {
            self.entries_through(from).await;
        }
        None
    }

    /// The outbox entries that the welcome of the region `from` announced have all been
    /// handled: what was kept for the region is sent, and if no presence answers are
    /// to come, the region has said whom it has.
    async fn entries_through(&mut self, from: RegionId) {
        // A region the routing table says went into this one, known when this link's
        // hello was said, whose `Absorbed` was not among the entries, here or before:
        // it had forgotten this edge, or this region has since. Nothing kept for it
        // means anything any more; what the edge had under it is this region's, and
        // the players are judged by the answers that follow.
        let port = self.regions.entry(from).or_default();
        let not_coming: Vec<RegionId> = match &port.link {
            Some(link) => link.owed.intersection(&port.owes).copied().collect(),
            None => return,
        };
        for absorbed in not_coming {
            warn!(%from, %absorbed, "an absorbed region had forgotten this edge");
            self.give_up_kept(absorbed).await;
            self.bring(absorbed, from).await;
        }
        // A merge the edge heard of while this welcome was on its way: only a hello
        // said knowing of it is answered from after it for certain.
        if !self.regions.entry(from).or_default().owes.is_empty() {
            debug!(%from, "ending a link to say hello anew after a merge");
            self.lose_link(from);
            return;
        }
        self.send_kept(from).await;
        let link = self.regions.entry(from).or_default().link.as_ref();
        if link.is_some_and(|link| link.presences == Some(0)) {
            self.presence_through(from).await;
        }
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
                self.entries_through(from).await;
            }
        }
    }

    /// The region `region` has forgotten this edge, which stayed away from it for too
    /// long: the players the edge believed to be there are not, and what it kept for the
    /// region means nothing to it any more.
    async fn forget_region(&mut self, region: RegionId) {
        warn!(%region, "a region has forgotten this edge; dropping what was kept for it");
        self.give_up_kept(region).await;
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

    /// Gives up what was kept for `region` and the numbering the two shared: the region
    /// has forgotten this edge, and numbers its entries, and expects the edge's
    /// messages numbered, from 1 again.
    async fn give_up_kept(&mut self, region: RegionId) {
        let port = &mut self.regions.entry(region).or_default();
        let kept = std::mem::take(&mut port.kept);
        port.numbered = 0;
        port.applied = 0;
        port.seen = 0;
        // They named numbers of the numbering that is over.
        port.came_with.clear();
        // What players of other regions did to blocks of this one will never be
        // answered. Their clients are told that it was handled, and see the blocks as
        // the region has them.
        for (_, body) in kept {
            if let EdgeToWorker::Remote(action) = body {
                self.arrived(action.player, action.sequence).await;
            }
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
                // An input names the stay it is of by its entity. Of a player who has
                // not been placed in the world the edge knows none, and their client,
                // which has not been put into the world, has nothing to say of what a
                // player does there.
                let Some(entity) = view.entity else {
                    debug!(
                        name = %view.name,
                        "dropping an input of a player who has not been placed in the world"
                    );
                    return;
                };
                view.inputs_sent += 1;
                let number = view.inputs_sent;
                view.kept_inputs
                    .push_back((number, Instant::now(), input.clone()));
                let region = view.region;
                let message = EdgeToWorker::Input {
                    player,
                    entity,
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
                let holder = self.living(region);
                self.elsewhere(from, chunk, ask, holder).await;
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
        // An entry that came to this region with a merge was another region's. If the
        // edge handled it as that region's, it is passed over here. If not, it is this
        // region's to say now, and can send a thing to the region it now comes from:
        // the absorbed region had let a player go to the survivor.
        let own = match port.came_with.remove(&number) {
            Some((origin, there)) => {
                let handled = self
                    .regions
                    .get(&origin)
                    .is_some_and(|port| there <= port.seen);
                if handled {
                    self.send_to_region(from, EdgeToWorker::Confirm { number })
                        .await;
                    return;
                }
                false
            }
            None => true,
        };
        match entry {
            Durable::Departed {
                player,
                transfer,
                to,
            } => {
                let back = !own || to != from;
                let to = self.living(to);
                self.hand_over(player, from, to, transfer, back).await;
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
                let back = !own || to.is_some_and(|to| to != from);
                let to = to.map(|to| self.living(to)).or(serves);
                self.pass_on(from, to, action, back).await;
            }
            // A remote action reached a region that does not hold the chunk and
            // believes another to.
            Durable::NotMine {
                what: Misdirected::Remote(action),
                holder,
            } => {
                let back = !own || holder != from;
                let holder = self.living(holder);
                self.pass_on(from, Some(holder), action, back).await;
            }
            // A player was let go to a region that believes another to hold the chunk
            // they stand in: they go on to that one, as if this region had let them go.
            Durable::NotMine {
                what: Misdirected::Arrival { player, transfer },
                holder,
            } => {
                let back = !own || holder != from;
                let holder = self.living(holder);
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
                    self.discard(holder, transfer.entity_id, chunk).await;
                    self.remove_player(player).await;
                } else {
                    self.hand_over(player, from, holder, transfer, back).await;
                }
            }
            Durable::RemoteDone { player, sequence } => self.arrived(player, sequence).await,
            Durable::Absorbed {
                region,
                since,
                applied,
                numbers,
            } => {
                self.absorbed(from, number, region, since, applied, numbers)
                    .await;
            }
            Durable::SplitOff { region, players } => {
                let part = self.living(region);
                self.split_off(from, part, players).await;
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
    /// told that it was handled, and sees the block as it is. It may go `back` where
    /// it came from if that is another region's word, which came to `from` with a
    /// merge, or if `from` named a region that has gone into it since.
    ///
    /// The region it goes to is asked for the chunk first, as a guest, if the edge is
    /// not asking it already: a region that holds the chunk and has yet to load it
    /// then judges the action only when it has (ADR-0014, rule 34), and one that does
    /// not hold it says so and passes the action on as ever.
    async fn pass_on(
        &mut self,
        from: RegionId,
        to: Option<RegionId>,
        action: RemoteAction,
        back: bool,
    ) {
        let (player, sequence) = (action.player, action.sequence);
        let Some(view) = self.players.get_mut(&player) else {
            // Nobody is left to be told how it ended.
            return;
        };
        view.under_way.insert(sequence);
        let Some(to) = to.filter(|to| *to != from || back) else {
            debug!(%from, ?to, "an action on blocks has nowhere to go and ends here");
            self.arrived(player, sequence).await;
            return;
        };
        let chunk = action.step.concerns().chunk();
        let watched = self
            .replica
            .get(&chunk)
            .is_some_and(|entry| entry.viewers > 0);
        let port = self.regions.entry(to).or_default();
        if watched && !port.subscriptions.contains_key(&chunk) {
            self.changed = true;
            port.subscriptions
                .insert(chunk, Subscription::new(Kind::Guest, 0));
            self.asking.push((to, Asking::AsGuest, chunk, true));
            self.flush_asking().await;
        }
        self.send_to_region(to, EdgeToWorker::Remote(action)).await;
    }

    /// Takes a presence answer of the region `from`, and counts it against what the
    /// region's welcome announced: when the last has been read, the region has said
    /// whom it has.
    async fn presence(&mut self, from: RegionId, player: PlayerId, answer: Presence) {
        self.take_presence(from, player, answer).await;
        let link = self.regions.entry(from).or_default().link.as_mut();
        let Some(left) = link.and_then(|link| link.presences.as_mut()) else {
            return;
        };
        // An answer beyond those announced is one too many, and ends nothing twice.
        let last = *left == 1;
        *left = left.saturating_sub(1);
        if last {
            self.presence_through(from).await;
        }
    }

    /// The region `from` has said whether it has `player`: one the edge named in its
    /// hello as one it believes to be there, or one the region has for this edge
    /// whether the hello named them or not. The entries of the region's outbox that
    /// the edge had not seen came before this, so a player the region let go has been
    /// handed on by now.
    ///
    /// A region can have a stay the edge did not send it: one that came with a merge
    /// or a split. So what it says is taken by the stay it names, the player with that
    /// entity, and by where the edge has that stay; see
    /// `docs/adr/0015-the-edge-through-merges-and-splits.md`, section 2.1.
    async fn take_presence(&mut self, from: RegionId, player: PlayerId, answer: Presence) {
        let Presence::Present {
            entity,
            pose,
            hotbar,
            selected_slot,
            last_input,
            handled,
        } = answer
        else {
            if self
                .players
                .get(&player)
                .is_some_and(|view| view.region == from)
            {
                self.absent(from, player).await;
            }
            return;
        };
        let stay = self
            .players
            .get(&player)
            .map(|view| (view.entity, view.region));
        match stay {
            // The stay the edge has, where it has it.
            Some((Some(shown), region)) if shown == entity && region == from => {}
            // The stay the edge has, under another region: it came here with a merge
            // or a split that the edge has not caught up with, and is this region's.
            Some((Some(shown), _)) if shown == entity => {
                // What the region had applied of theirs is not sent to it.
                if let Some(view) = self.players.get_mut(&player) {
                    while view
                        .kept_inputs
                        .front()
                        .is_some_and(|(number, ..)| *number <= last_input)
                    {
                        view.kept_inputs.pop_front();
                    }
                }
                self.move_stay(player, from).await;
            }
            // A player who is entering the world. If their join is still among what is
            // to be sent to the region again, the region had not applied it when it
            // made this answer, which is then about who they were before: a stay the
            // join will end. Taking it for them would put them into the world as
            // their old self, with an entity the region removes a moment later.
            Some((None, region)) if region == from => {
                let port = &self.regions.entry(from).or_default();
                let joining = port.kept.iter().any(|(_, body)| {
                    matches!(body, EdgeToWorker::PlayerJoin(join) if join.player == player)
                });
                if joining {
                    return;
                }
                // The region placed the player, and the word of it was lost with the
                // link.
                let inventory = inventory_packets(&hotbar, selected_slot);
                self.spawn_player(player, entity, pose.position, inventory)
                    .await;
            }
            // A stay the edge does not have: of a player it has given up, or has as
            // another entity. It is the region's stay that ends, by a leave that names
            // it, unless one is on its way already.
            _ => {
                let port = &self.regions.entry(from).or_default();
                let leaving = port.kept.iter().any(|(_, body)| {
                    matches!(
                        body,
                        EdgeToWorker::PlayerLeave { player: left, entity: named }
                            if *left == player && *named == Some(entity)
                    )
                });
                if !leaving {
                    debug!(%from, entity = entity.0, "a region has a stay this edge does not; ending it");
                    let leave = EdgeToWorker::PlayerLeave {
                        player,
                        entity: Some(entity),
                    };
                    self.send_to_region(from, leave).await;
                }
                return;
            }
        }
        if let Some(link) = self.regions.entry(from).or_default().link.as_mut() {
            link.brought.remove(&player);
        }
        let Some(view) = self.players.get_mut(&player) else {
            return;
        };
        while view
            .kept_inputs
            .front()
            .is_some_and(|(number, ..)| *number <= last_input)
        {
            view.kept_inputs.pop_front();
        }
        // Through the same holding back as every acknowledgement: an action that is
        // under way elsewhere holds back later ones.
        if let Some(sequence) = handled {
            self.handled(player, sequence).await;
        }
    }

    /// The region that `region` means: itself, or the one it went into if it is no
    /// more and the edge has acted on that. Every region a region names is read through
    /// this.
    fn living(&self, region: RegionId) -> RegionId {
        self.stands_for.get(&region).copied().unwrap_or(region)
    }

    /// Makes `absorbed` stand for `into` from now on. Its link, if the port still has
    /// one, is taken: the end of a link is read from the same queue as everything
    /// else, in no order with what another region says, so the edge can hear of a
    /// merge while the absorbed region's link still stands with messages unread. What
    /// was unread on it and matters comes again behind the survivor's `Absorbed`.
    fn retire(&mut self, absorbed: RegionId, into: RegionId) {
        self.changed = true;
        for region in self.stands_for.values_mut() {
            if *region == absorbed {
                *region = into;
            }
        }
        self.stands_for.insert(absorbed, into);
        let port = self.regions.entry(absorbed).or_default();
        if let Some(link) = port.link.take() {
            debug!(region = %absorbed, "dropping the link to a region that is no more");
            let _ = self.lost.send((absorbed, link.epoch));
        }
        // What was owed for it is the survivor's to say now, among the entries that
        // came with the merge.
        let owed = std::mem::take(&mut port.owes);
        let survivor = self.regions.entry(into).or_default();
        survivor.owes.remove(&absorbed);
        survivor.owes.extend(owed);
    }

    /// Whether the edge has nothing an `Absorbed` for `region` could move.
    fn has_nothing_of(&self, region: RegionId) -> bool {
        let port_empty = self
            .regions
            .get(&region)
            .is_none_or(|port| port.subscriptions.is_empty() && port.kept.is_empty());
        let named = self.regions.values().any(|port| {
            let mut conditions = port.subscriptions.values().map(|held| held.condition);
            conditions.any(|condition| condition == Condition::Elsewhere(region))
        });
        port_empty
            && !named
            && self.players.values().all(|view| view.region != region)
            && self.entities.values().all(|shown| shown.from != region)
            && (self.replica.values()).all(|entry| entry.served_by != Some(region))
    }

    /// Takes what the routing table says of the regions that were absorbed, each with
    /// the region it went into. The edge acts on a merge when the survivor tells it,
    /// with an `Absorbed` among a welcome's entries; the table only says that such a
    /// word is owed, or, of a region the edge has nothing of, that there is nothing to
    /// be told. See `docs/adr/0015-the-edge-through-merges-and-splits.md`, section 5.
    async fn take_pairs(&mut self, pairs: Vec<(RegionId, RegionId)>) {
        let pairs: BTreeMap<RegionId, RegionId> = pairs.into_iter().collect();
        for (&absorbed, &first) in &pairs {
            if self.stands_for.contains_key(&absorbed) {
                continue;
            }
            // Through merges in a row, to the region that lives; the table can be
            // behind what the edge has been told since.
            let mut into = first;
            for _ in 0..pairs.len() {
                match pairs.get(&into) {
                    Some(next) => into = *next,
                    None => break,
                }
            }
            let into = self.living(into);
            if into == absorbed || pairs.contains_key(&into) {
                error!(%absorbed, %into, "the routing table has regions that went into each other");
                continue;
            }
            if self.has_nothing_of(absorbed) {
                self.retire(absorbed, into);
                continue;
            }
            let port = self.regions.entry(into).or_default();
            port.owes.insert(absorbed);
            // A link that is through its welcome's entries was answered without the
            // word; whether before the merge or after it, only a new hello tells.
            let settled = port
                .link
                .as_ref()
                .is_some_and(|link| link.welcomed && !link.owed.contains(&absorbed));
            if settled {
                debug!(region = %into, %absorbed, "ending a link to say hello anew after a merge");
                self.lose_link(into);
            }
        }
    }

    /// The region `into` says that it has absorbed `region`, in the entry numbered
    /// `number` of its outbox: `since` is what the absorbed region knew this edge
    /// since, `applied` how far it had applied the edge's messages, and `numbers` the
    /// numbers its own entries for the edge had, which follow this one under the next
    /// numbers. See `docs/adr/0015-the-edge-through-merges-and-splits.md`, section 3.
    async fn absorbed(
        &mut self,
        into: RegionId,
        number: u64,
        region: RegionId,
        since: u64,
        applied: u64,
        numbers: Vec<u64>,
    ) {
        if region == into {
            error!(%into, "a region said that it has absorbed itself");
            return;
        }
        info!(%region, %into, "a region has been absorbed");
        // Whether the two shared a numbering: the edge and the region then mean the
        // same by `applied` and by the numbers of the entries.
        let port = self.regions.entry(region).or_default();
        let shared = since != 0 && since == port.since;
        let forgotten = !shared && (port.seen != 0 || port.applied != 0);
        if forgotten {
            // It had forgotten this edge, and removed its players of it: they are
            // judged by the answers behind these entries.
            self.give_up_kept(region).await;
        }
        // Nothing of what is kept was applied by a region that had forgotten the edge,
        // or never had anything from it.
        let applied = if shared { applied } else { 0 };

        self.bring(region, into).await;

        // What the absorbed region had not applied is the survivor's to apply, under
        // its numbers, behind what was kept for it and behind the subscriptions above.
        let port = self.regions.entry(region).or_default();
        let kept = std::mem::take(&mut port.kept);
        let came_with = std::mem::take(&mut port.came_with);
        let survivor = self.regions.entry(into).or_default();
        let mut moved = Vec::new();
        for (_, body) in kept.into_iter().filter(|(number, _)| *number > applied) {
            survivor.numbered += 1;
            survivor.kept.push_back((survivor.numbered, body.clone()));
            moved.push((survivor.numbered, body));
        }
        // The entries behind this one were the absorbed region's, or came to it with a
        // merge of its own, and are told by the numbers they had from those the edge
        // has handled.
        for (behind, there) in (number + 1..).zip(numbers) {
            let origin = came_with.get(&there).copied().unwrap_or((region, there));
            survivor.came_with.insert(behind, origin);
        }
        // While a welcome's entries are read nothing kept is sent; it all goes in
        // order when they are through. Should the link be past that, what was moved
        // goes at once.
        if let Some(link) = survivor.link.as_ref().filter(|link| link.welcomed) {
            for (number, body) in moved {
                let message = EdgeMessage {
                    number: Some(number),
                    body,
                };
                if link.sender.send(message).await.is_err() {
                    break;
                }
            }
        }
    }

    /// Makes everything the edge has under `absorbed` the region `into`'s: the region
    /// stands for it, its players are that region's and their views are asked of it,
    /// and so is what players of other regions see of it. Nothing is said to the
    /// absorbed region, which is no more. The players are judged by the presence
    /// answers of `into`'s link.
    async fn bring(&mut self, absorbed: RegionId, into: RegionId) {
        self.retire(absorbed, into);
        let players: Vec<PlayerId> = self
            .players
            .iter()
            .filter(|(_, view)| view.region == absorbed)
            .map(|(player, _)| *player)
            .collect();
        for player in &players {
            let Some(view) = self.players.get_mut(player) else {
                continue;
            };
            view.region = into;
            let seen: Vec<ChunkPos> = view.wanted.iter().copied().collect();
            for chunk in &seen {
                self.unwant(absorbed, *chunk);
            }
            for chunk in &seen {
                self.want(into, *chunk);
            }
        }
        if let Some(link) = self.regions.entry(into).or_default().link.as_mut() {
            link.brought.extend(players);
        }
        // What is left there are guests' subscriptions, for what players of other
        // regions see.
        let port = self.regions.entry(absorbed).or_default();
        let left: Vec<ChunkPos> = std::mem::take(&mut port.subscriptions)
            .into_keys()
            .collect();
        self.asking.retain(|(region, ..)| *region != absorbed);
        for chunk in left {
            let watched = self
                .replica
                .get(&chunk)
                .is_some_and(|entry| entry.viewers > 0);
            let survivor = self.regions.entry(into).or_default();
            if watched && !survivor.subscriptions.contains_key(&chunk) {
                survivor
                    .subscriptions
                    .insert(chunk, Subscription::new(Kind::Guest, 0));
                self.asking.push((into, Asking::AsGuest, chunk, true));
            }
        }
        // Whatever named the absorbed region names the survivor, which is asked for
        // those chunks now wherever the absorbed region was.
        for (region, port) in &mut self.regions {
            for subscription in port.subscriptions.values_mut() {
                if subscription.condition == Condition::Elsewhere(absorbed) {
                    subscription.condition = if *region == into {
                        Condition::Waiting
                    } else {
                        Condition::Elsewhere(into)
                    };
                }
            }
        }
        for shown in self.entities.values_mut() {
            if shown.from == absorbed {
                shown.from = into;
            }
        }
        for entry in self.replica.values_mut() {
            if entry.served_by == Some(absorbed) {
                entry.served_by = Some(into);
            }
        }
        // On the survivor's link before anything that was kept for either.
        self.flush_asking().await;
    }

    /// The region `from` says that it was split and that the stays of `players` were
    /// in `part` from then on. The word is as old as the split and read at any time
    /// after: a stay the edge has under `from` with an arrival kept for it has come
    /// back since, by way of the part, and stays.
    async fn split_off(
        &mut self,
        from: RegionId,
        part: RegionId,
        players: Vec<(PlayerId, EntityId)>,
    ) {
        info!(%from, %part, players = players.len(), "a region has been split");
        if part == from {
            // The part has gone back into the region since.
            return;
        }
        for (player, entity) in players {
            let there = self
                .players
                .get(&player)
                .is_some_and(|view| view.entity == Some(entity) && view.region == from);
            let port = self.regions.entry(from).or_default();
            let back = port.kept.iter().any(|(_, body)| {
                matches!(
                    body,
                    EdgeToWorker::PlayerArrive { player: arriving, .. } if *arriving == player
                )
            });
            if there && !back {
                self.move_stay(player, part).await;
            }
        }
    }

    /// Puts the stay of `player` under the region `to`, which has said that it has it
    /// although the edge did not send it there: what the player sees is asked of `to`
    /// from now on, and what they did that no region has reported applied is sent
    /// there. There is no arrival: the region has the player whole.
    async fn move_stay(&mut self, player: PlayerId, to: RegionId) {
        let Some(view) = self.players.get_mut(&player) else {
            return;
        };
        let (from, Some(entity)) = (view.region, view.entity) else {
            return;
        };
        if from == to {
            return;
        }
        debug!(name = %view.name, %from, %to, "a stay has moved without this edge");
        view.region = to;
        // The region that has the stay is the one whose word on its entity counts from
        // now on. It will not introduce the entity before it says where it moves: the
        // player is there already, and a tick says what moved before it sends the
        // chunks that were asked for. What the region it left says of the entity is
        // about one it no longer has.
        if let Some(shown) = self.entities.get_mut(&entity) {
            shown.from = to;
        }
        let again: Vec<_> = view
            .kept_inputs
            .iter()
            .map(|(number, _, input)| (*number, input.clone()))
            .collect();
        // On the link to `to` this is before the inputs, as at a hand-over.
        let seen: Vec<ChunkPos> = view.wanted.iter().copied().collect();
        for chunk in &seen {
            self.unwant(from, *chunk);
        }
        for chunk in &seen {
            self.want(to, *chunk);
        }
        self.flush_asking().await;
        for (number, input) in again {
            let message = EdgeToWorker::Input {
                player,
                entity,
                number,
                input,
            };
            self.send_to_region(to, message).await;
        }
    }

    /// The region `from` does not have `player`, whom the edge has under it. A join or
    /// an arrival that the region has not applied yet is among what is sent to it
    /// again, and puts the player there; otherwise they are lost.
    async fn absent(&mut self, from: RegionId, player: PlayerId) {
        let port = &self.regions.entry(from).or_default();
        let under_way = port.kept.iter().any(|(_, body)| match body {
            EdgeToWorker::PlayerJoin(join) => join.player == player,
            EdgeToWorker::PlayerArrive {
                player: arriving, ..
            } => *arriving == player,
            _ => false,
        });
        if under_way {
            return;
        }
        if let Some(view) = self.players.get(&player) {
            warn!(name = %view.name, %from, "a region does not have a player it should have");
            refuse(&view.outbound, "The server lost track of where you are.");
        }
        self.remove_player(player).await;
    }

    /// The region `from` has said whom it has for this edge. The players its welcome
    /// itself made its own, and of whom it then said nothing, are not there. Nobody
    /// else is judged: a player the hello named has had an answer of their own, and
    /// one whom another region's word put under this region meanwhile may have been
    /// sent on since.
    async fn presence_through(&mut self, from: RegionId) {
        let Some(link) = self.regions.entry(from).or_default().link.as_mut() else {
            return;
        };
        let brought = std::mem::take(&mut link.brought);
        for player in brought {
            if self
                .players
                .get(&player)
                .is_some_and(|view| view.region == from)
            {
                self.absent(from, player).await;
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
    /// as the one they walked into, together with what they have done since. They may
    /// go `back` to the region the word comes from if it is another region's word,
    /// which came to `from` with a merge, or if `from` named a region that has gone
    /// into it since; see `docs/adr/0015-the-edge-through-merges-and-splits.md`,
    /// section 4.
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
        back: bool,
    ) {
        let position = transfer.pose.position;
        let chunk = ChunkPos::containing(position.x, position.z);
        // The player's connection can have ended while the message was on its way, and
        // they can even be back already, as a new entity somewhere else.
        let current = self
            .players
            .get_mut(&player)
            .filter(|view| view.entity == Some(transfer.entity_id));
        let Some(view) = current else {
            self.discard(to, transfer.entity_id, chunk).await;
            return;
        };
        if view.region != from {
            // About an earlier stay of the player in that region: they have been passed
            // on from there since.
            error!(name = %view.name, %from, now = %view.region, "a region let go of a player who is not its own");
            return;
        }
        if to == from && !back {
            // The region named itself as the one the player walked into, which none
            // does. Sending the player back would have them bounce there forever.
            error!(name = %view.name, %from, "a region let go of a player who is inside it");
            refuse(&view.outbound, "The server lost track of where you are.");
            self.discard(to, transfer.entity_id, chunk).await;
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
        // chunk the player arrives in and their view. A player who arrives where the
        // word of their leaving comes from is asked for there already: the region
        // that let them go has gone into it, or it into that region.
        if to != from {
            let seen: Vec<ChunkPos> = view.wanted.iter().copied().collect();
            for chunk in &seen {
                self.unwant(from, *chunk);
            }
            for chunk in &seen {
                self.want(to, *chunk);
            }
            self.flush_asking().await;
        }

        // The entity of the player's view, as was looked at above: the inputs sent
        // again are of the stay that is handed over.
        let entity = transfer.entity_id;
        self.send_to_region(to, EdgeToWorker::PlayerArrive { player, transfer })
            .await;
        for (number, input) in again {
            let message = EdgeToWorker::Input {
                player,
                entity,
                number,
                input,
            };
            self.send_to_region(to, message).await;
        }
        // Normally the view has followed the move already; this covers a player who was
        // let go without having been seen to move, such as one who joined right there.
        self.move_view(player, chunk).await;
    }

    /// Gives up an entity that a region let go and that has nowhere to go: its player
    /// left meanwhile, or cannot be placed.
    ///
    /// The old region did not report the entity as removed, because it lived on. The
    /// region it was heading for, `to`, is told to report it, for every edge whose
    /// players saw it cross. This edge takes it off its own screens here and now: it
    /// takes a removal only from the region that showed it the entity last, which is
    /// the old one, and it may not be asking `to` for that chunk at all.
    async fn discard(&mut self, to: RegionId, entity: EntityId, chunk: ChunkPos) {
        self.send_to_region(to, EdgeToWorker::Discard { entity, chunk })
            .await;
        self.remove_entity(entity).await;
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
                // Said where an operator reads it: the player is disconnected for it,
                // and "player left" alone looks like their own doing.
                warn!(name = %view.name, %error, "dropping a player that does not keep up");
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
                // Another region's viewers can have been told that this region holds
                // the chunk, from a belief older than this region's own. Nothing is
                // asked here any more, so they ask their own region again.
                if watched {
                    self.ask_those_told(region, chunk);
                }
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
        match there.subscriptions.entry(chunk) {
            std::collections::btree_map::Entry::Vacant(free) => {
                free.insert(Subscription::new(Kind::Guest, 0));
                self.asking.push((holder, Asking::AsGuest, chunk, true));
            }
            // The region that is named has said itself that another holds the chunk.
            // One of the two is behind, and if they name each other nobody serves the
            // chunk, with nothing left that would have either asked again: a region
            // can hold a chunk by an area it is pinned to and go on believing the
            // region that held it for a while and gave it back. So the one that was
            // named is asked again, which has it ask the store.
            std::collections::btree_map::Entry::Occupied(held) => {
                let held = held.into_mut();
                let told =
                    held.kind == Kind::Viewer && matches!(held.condition, Condition::Elsewhere(_));
                if told && Self::ask_again(held, Instant::now()) {
                    self.asking.push((holder, Asking::Subscribe, chunk, true));
                }
            }
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
        self.ask_those_told(from, chunk);
        self.flush_asking().await;
    }

    /// Has every region ask the store again whose viewers were told that `holder`
    /// holds `chunk`, now that nothing is asked of `holder` for it: at once, or at the
    /// task's next check where that region was asked again less than a second ago.
    /// The messages are sent with the turn's others.
    fn ask_those_told(&mut self, holder: RegionId, chunk: ChunkPos) {
        let now = Instant::now();
        for (region, port) in &mut self.regions {
            let Some(subscription) = port.subscriptions.get_mut(&chunk) else {
                continue;
            };
            if subscription.kind != Kind::Viewer
                || subscription.condition != Condition::Elsewhere(holder)
            {
                continue;
            }
            if Self::ask_again(subscription, now) {
                self.asking.push((*region, Asking::Subscribe, chunk, true));
            }
        }
    }

    /// Has a viewer's subscription that was told elsewhere asked again: at once, which
    /// this says by returning true, and the caller sends the message; or, where it was
    /// asked again less than a second ago, at the check the task makes every second.
    fn ask_again(subscription: &mut Subscription, now: Instant) -> bool {
        let lately = subscription
            .asked_again
            .is_some_and(|asked| now.duration_since(asked) < ASK_AGAIN_EVERY);
        if lately {
            subscription.again_due = true;
            return false;
        }
        subscription.condition = Condition::Waiting;
        subscription.asked_again = Some(now);
        true
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
        // saying so, which is on its way, makes `hand_over` clean up. The leave names
        // the entity the player had, if the edge was told of one.
        let leave = EdgeToWorker::PlayerLeave {
            player,
            entity: view.entity,
        };
        self.send_to_region(view.region, leave).await;
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
        /// The number of the last outbox entry each region has made: the two the edge
        /// starts with, and one that a split makes.
        outbox: [u64; 3],
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
                outbox: [0; 3],
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
                        presences: 0,
                        applied: 0,
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

        /// Gives the edge its first link to the region a split made, and returns the
        /// hello it says there.
        async fn link_part(&mut self) -> EdgeToWorker {
            assert_eq!(self.regions.len(), PART.0 as usize);
            let (link, worker) = link_to(PART, 1);
            self.regions.push(worker);
            assert!(self.relinks.replace(link).await);
            self.next(PART).await.body
        }

        /// What the edge sends `region` up to its next numbered message: the
        /// messages without a number, and then that one with its number.
        async fn up_to_numbered(
            &mut self,
            region: RegionId,
        ) -> (Vec<EdgeToWorker>, (u64, EdgeToWorker)) {
            let mut before = Vec::new();
            loop {
                let EdgeMessage { number, body } = self.next(region).await;
                match number {
                    Some(number) => return (before, (number, body)),
                    None => before.push(body),
                }
            }
        }
    }

    /// The region a split of these tests makes.
    const PART: RegionId = RegionId(2);

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

        edge.tell(
            WEST,
            WorkerToEdge::Welcome(Welcome::Resumed {
                entries: 0,
                presences: 1,
                applied: 1,
            }),
        );
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
        edge.tell(
            WEST,
            WorkerToEdge::Welcome(Welcome::Resumed {
                entries: 0,
                presences: 1,
                applied: 2,
            }),
        );
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
                presences: 1,
                applied: 0,
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
        edge.tell(
            WEST,
            WorkerToEdge::Welcome(Welcome::Resumed {
                entries: 0,
                presences: 1,
                applied: 1,
            }),
        );
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
        edge.tell(
            WEST,
            WorkerToEdge::Welcome(Welcome::Resumed {
                entries: 0,
                presences: 2,
                applied: 1,
            }),
        );
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
        edge.tell(
            WEST,
            WorkerToEdge::Welcome(Welcome::Resumed {
                entries: 0,
                presences: 1,
                applied: 1,
            }),
        );
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
            (
                2,
                EdgeToWorker::PlayerLeave {
                    player: player(1),
                    entity: Some(EntityId(5)),
                }
            )
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
        edge.tell(
            WEST,
            WorkerToEdge::Welcome(Welcome::Resumed {
                entries: 0,
                presences: 1,
                applied: 1,
            }),
        );
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
            (
                2,
                EdgeToWorker::PlayerLeave {
                    player: player(1),
                    entity: Some(EntityId(5)),
                }
            )
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
        edge.tell(
            WEST,
            WorkerToEdge::Welcome(Welcome::Resumed {
                entries: 0,
                presences: 1,
                applied: 3,
            }),
        );
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
            presences: 0,
            applied: 0,
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
        edge.tell(
            WEST,
            WorkerToEdge::Welcome(Welcome::Resumed {
                entries: 2,
                presences: 1,
                applied: 0,
            }),
        );
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
                presences: 0,
                applied: 0,
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

        // The region names the one that takes the next step. The edge is not asking
        // that one for the chunk yet, and does so first, as a guest: a region that
        // holds the chunk and has yet to load it then judges the action only when it
        // has.
        let named = Durable::Remote {
            action: action(3),
            to: Some(EAST),
        };
        edge.say(WEST, named);
        let EdgeMessage { number, body } = edge.next(EAST).await;
        assert!(
            number.is_none()
                && matches!(&body, EdgeToWorker::SubscribeAsGuest { chunks, .. } if chunks == &[SHARED]),
            "{body:?}"
        );
        let (number, body) = edge.next_numbered(EAST).await;
        assert_eq!((number, body), (1, EdgeToWorker::Remote(action(3))));

        // It names none, and no region serves the edge that chunk: the action ends
        // here, and nothing goes to the east (the next message there is numbered 2).
        edge.say(WEST, without_a_region(4));
        edge.settle(WEST).await;

        // The east serves the chunk: the west says so, and the east, which the edge is
        // asking as a guest already, sends it.
        let elsewhere = WorkerToEdge::Elsewhere {
            chunk: SHARED,
            ask,
            region: EAST,
        };
        edge.tell(WEST, elsewhere);
        edge.settle(WEST).await;
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

    /// A welcome that resumes, with `presences` answers behind it and the region
    /// having applied this edge's messages up to `applied`.
    fn resumed(presences: u32, applied: u64) -> WorkerToEdge {
        WorkerToEdge::Welcome(Welcome::Resumed {
            entries: 0,
            presences,
            applied,
        })
    }

    fn says_present(player: PlayerId, entity: EntityId) -> WorkerToEdge {
        WorkerToEdge::Presence {
            player,
            answer: present(entity, 0),
        }
    }

    /// A region says every stay it has for the edge, also those the hello did not
    /// name. One the edge does not have, of a player it has given up or has as another
    /// entity, is ended by a leave that names it; the player the edge has is not
    /// touched by that.
    #[tokio::test]
    async fn a_stay_a_region_has_and_the_edge_does_not_is_ended_by_a_leave_that_names_it() {
        let mut edge = Harness::start().await;
        let mut packets = edge.joined(player(1), EntityId(5)).await;

        edge.relink(WEST, 2).await;
        edge.tell(WEST, resumed(2, 1));
        edge.tell(WEST, says_present(player(1), EntityId(5)));
        edge.tell(WEST, says_present(player(2), EntityId(9)));
        let leave = EdgeToWorker::PlayerLeave {
            player: player(2),
            entity: Some(EntityId(9)),
        };
        assert_eq!(edge.next_numbered(WEST).await, (2, leave));

        // The region has the player the edge has, as someone they were before.
        edge.relink(WEST, 3).await;
        edge.tell(WEST, resumed(1, 2));
        edge.tell(WEST, says_present(player(1), EntityId(4)));
        let leave = EdgeToWorker::PlayerLeave {
            player: player(1),
            entity: Some(EntityId(4)),
        };
        assert_eq!(edge.next_numbered(WEST).await, (3, leave));
        edge.settle(WEST).await;
        assert!(connected(&mut packets));
    }

    /// A region that says it has a stay the edge has under another region has come by
    /// it through a merge or a split. The stay is that region's from then on: it is
    /// asked for what the player sees before it is sent anything they did, there is no
    /// arrival, and what they do next goes there.
    #[tokio::test]
    async fn a_stay_the_edge_has_under_another_region_moves_to_the_one_that_says_it_has_it() {
        let mut edge = Harness::start().await;
        let mut packets = edge.joined(player(1), EntityId(5)).await;
        edge.input(player(1), step(1.5)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 2);

        let hello = edge.relink(EAST, 2).await;
        let EdgeToWorker::Hello { players, .. } = hello else {
            panic!("{hello:?}");
        };
        assert_eq!(players, []);
        let welcome = Welcome::Unknown {
            since: 7,
            entries: 0,
            presences: 1,
            applied: 0,
        };
        edge.tell(EAST, WorkerToEdge::Welcome(welcome));
        edge.tell(EAST, says_present(player(1), EntityId(5)));

        // What they see, as a viewer's, and only then what they did.
        let EdgeMessage { number, body } = edge.next(EAST).await;
        let EdgeToWorker::Subscribe { chunks, .. } = &body else {
            panic!("{body:?}");
        };
        assert!(number.is_none() && chunks.contains(&SHARED), "{body:?}");
        let (number, body) = edge.next_numbered(EAST).await;
        let again = EdgeToWorker::Input {
            player: player(1),
            entity: EntityId(5),
            number: 1,
            input: step(1.5),
        };
        assert_eq!((number, body), (1, again));
        // The region they were under is left a guest's subscription for what they
        // still see.
        let left = next_asked(&mut edge, WEST).await;
        assert!(
            matches!(&left, EdgeToWorker::SubscribeAsGuest { chunks, .. } if chunks.contains(&SHARED)),
            "{left:?}"
        );

        edge.input(player(1), step(2.5)).await;
        let (number, body) = edge.next_numbered(EAST).await;
        assert!(
            number == 2 && matches!(body, EdgeToWorker::Input { number: 2, .. }),
            "{body:?}"
        );
        assert!(connected(&mut packets));
    }

    /// A region's answer about a player who is entering the world is about the stay
    /// their join made only if the region had applied the join, which its welcome
    /// says. Otherwise it is about who they were before, and the join, sent again,
    /// places them.
    #[tokio::test]
    async fn a_player_who_is_entering_the_world_is_placed_by_an_answer_only_if_their_join_was_applied()
     {
        let mut edge = Harness::start().await;
        let mut packets = edge.join(player(1)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 1);
        edge.relink(WEST, 2).await;
        edge.tell(WEST, resumed(1, 1));
        edge.tell(WEST, says_present(player(1), EntityId(5)));
        assert!(timeout(SOON, packets.recv()).await.unwrap().is_some());
        // The join is not sent again, and what they do names the stay they were told.
        edge.settle(WEST).await;
        edge.input(player(1), step(1.5)).await;
        let (number, body) = edge.next_numbered(WEST).await;
        assert!(
            number == 2
                && matches!(
                    body,
                    EdgeToWorker::Input {
                        entity: EntityId(5),
                        ..
                    }
                ),
            "{number} {body:?}"
        );

        let mut edge = Harness::start().await;
        let mut packets = edge.join(player(1)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 1);
        edge.relink(WEST, 2).await;
        // The region has them from before, by a way that left no leave with the edge.
        edge.tell(WEST, resumed(1, 0));
        edge.tell(WEST, says_present(player(1), EntityId(4)));
        let (number, join) = edge.next_numbered(WEST).await;
        assert!(
            number == 1 && matches!(join, EdgeToWorker::PlayerJoin(_)),
            "{join:?}"
        );
        edge.settle(WEST).await;
        assert_eq!(
            packets.try_recv().err(),
            Some(mpsc::error::TryRecvError::Empty)
        );
        edge.tell(WEST, spawned(player(1), EntityId(6)));
        assert!(timeout(SOON, packets.recv()).await.unwrap().is_some());
        edge.settle(WEST).await;
        edge.input(player(1), step(1.5)).await;
        let (_, body) = edge.next_numbered(WEST).await;
        assert!(
            matches!(
                body,
                EdgeToWorker::Input {
                    entity: EntityId(6),
                    ..
                }
            ),
            "{body:?}"
        );
    }

    /// What a welcome says the region had applied is not sent to it again.
    #[tokio::test]
    async fn what_a_welcome_says_was_applied_is_not_sent_again() {
        let mut edge = Harness::start().await;
        let _packets = edge.joined(player(1), EntityId(5)).await;
        edge.input(player(1), step(1.5)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 2);

        edge.relink(WEST, 2).await;
        edge.tell(WEST, resumed(1, 1));
        edge.tell(WEST, says_present(player(1), EntityId(5)));
        let (number, body) = edge.next_numbered(WEST).await;
        assert!(
            number == 2 && matches!(body, EdgeToWorker::Input { .. }),
            "{number} {body:?}"
        );
    }

    /// A welcome that resumes with `entries` outbox entries and `presences` answers
    /// behind it, from a region that had applied this edge's messages up to `applied`.
    fn resumed_with(entries: u32, presences: u32, applied: u64) -> WorkerToEdge {
        WorkerToEdge::Welcome(Welcome::Resumed {
            entries,
            presences,
            applied,
        })
    }

    /// The east as absorbed by the region that says this: it knew the edge since the
    /// welcome of these tests' start, had applied its messages up to `applied`, and
    /// had entries for it under `numbers`, which follow.
    fn east_absorbed(applied: u64, numbers: Vec<u64>) -> Durable {
        Durable::Absorbed {
            region: EAST,
            since: 1,
            applied,
            numbers,
        }
    }

    fn applied_up_to(applied: u64) -> WorkerToEdge {
        WorkerToEdge::Progress {
            applied,
            inputs: Vec::new(),
        }
    }

    /// A player joins as entity 5, is let go to the east, which applies their arrival,
    /// and takes a step there that the east has not applied.
    async fn stepping_in_the_east(edge: &mut Harness) -> mpsc::Receiver<Bytes> {
        let packets = edge.joined(player(1), EntityId(5)).await;
        edge.say(WEST, departing_to_east(player(1), EntityId(5)));
        let (number, body) = edge.next_numbered(EAST).await;
        assert!(
            number == 1 && matches!(body, EdgeToWorker::PlayerArrive { .. }),
            "{body:?}"
        );
        edge.tell(EAST, applied_up_to(1));
        edge.settle(EAST).await;
        edge.settle(WEST).await;
        edge.input(player(1), step(65.5)).await;
        assert_eq!(edge.next_numbered(EAST).await.0, 2);
        packets
    }

    fn step_in_the_east() -> EdgeToWorker {
        EdgeToWorker::Input {
            player: player(1),
            entity: EntityId(5),
            number: 1,
            input: step(65.5),
        }
    }

    /// A region that was absorbed is the survivor's from the moment the survivor says
    /// so, among the entries of a welcome: its players are the survivor's, what they
    /// see is asked of the survivor, and what was kept for it and not applied goes to
    /// the survivor under the survivor's numbers, behind those subscriptions.
    #[tokio::test]
    async fn what_the_edge_had_at_an_absorbed_region_is_the_survivors_from_its_word_on() {
        let mut edge = Harness::start().await;
        let mut packets = stepping_in_the_east(&mut edge).await;

        let hello = edge.relink(WEST, 2).await;
        let EdgeToWorker::Hello { players, .. } = hello else {
            panic!("{hello:?}");
        };
        assert_eq!(players, []);
        edge.tell(WEST, resumed_with(1, 1, 1));
        let entry = edge.say(WEST, east_absorbed(1, Vec::new()));
        edge.tell(WEST, says_present(player(1), EntityId(5)));

        let (before, numbered) = edge.up_to_numbered(WEST).await;
        // What they see, as a viewer's; the entry confirmed; then what the east had
        // not applied, under the west's next number: its join was applied.
        let eastern = ChunkPos::new(4, 0);
        assert!(
            before.iter().any(|body| matches!(
                body,
                EdgeToWorker::Subscribe { chunks, .. } if chunks.contains(&eastern)
            )),
            "{before:?}"
        );
        assert!(
            before.contains(&EdgeToWorker::Confirm { number: entry }),
            "{before:?}"
        );
        assert_eq!(numbered, (2, step_in_the_east()));

        edge.settle(WEST).await;
        edge.input(player(1), step(66.5)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 3);
        assert!(connected(&mut packets));
    }

    /// A player the survivor's welcome brought with an absorbed region, and of whom
    /// its presence answers then say nothing, is not there: they are told so, and the
    /// leave names their stay.
    #[tokio::test]
    async fn a_player_who_came_with_an_absorbed_region_and_is_not_there_is_disconnected() {
        let mut edge = Harness::start().await;
        let mut packets = stepping_in_the_east(&mut edge).await;

        edge.relink(WEST, 2).await;
        edge.tell(WEST, resumed_with(1, 0, 1));
        edge.say(WEST, east_absorbed(1, Vec::new()));
        assert_eq!(edge.next_numbered(WEST).await, (2, step_in_the_east()));
        let leave = EdgeToWorker::PlayerLeave {
            player: player(1),
            entity: Some(EntityId(5)),
        };
        assert_eq!(edge.next_numbered(WEST).await, (3, leave));
        disconnected(&mut packets).await;
    }

    /// The entries an absorbed region had for the edge follow the survivor's word of
    /// the merge under the survivor's numbers. Those the edge had handled as the
    /// absorbed region's are passed over; the others are handled, and one that lets a
    /// player go to the survivor, which until now a region could not say of itself,
    /// is an arrival there.
    #[tokio::test]
    async fn entries_that_came_with_a_merge_are_handled_unless_they_were_before() {
        let mut edge = Harness::start().await;
        let mut packets = edge.joined(player(1), EntityId(5)).await;
        edge.say(WEST, departing_to_east(player(1), EntityId(5)));
        assert_eq!(edge.next_numbered(EAST).await.0, 1);
        edge.tell(EAST, applied_up_to(1));
        edge.settle(WEST).await;
        // The edge has seen five entries of the east.
        for _ in 0..5 {
            edge.settle(EAST).await;
        }
        assert_eq!(edge.outbox[EAST.0 as usize], 5);

        edge.relink(WEST, 2).await;
        edge.tell(WEST, resumed_with(4, 0, 1));
        edge.say(WEST, east_absorbed(1, vec![4, 5, 6]));
        // Two the edge had seen as the east's, which would send an action on to the
        // west if they were handled again; and one it had not: the east let the
        // player go back to the west.
        let action = |sequence| RemoteAction {
            player: player(1),
            sequence,
            step: RemoteStep::Break {
                position: BlockPos::new(40, -61, 0),
            },
        };
        for sequence in [3, 4] {
            let seen = Durable::Remote {
                action: action(sequence),
                to: Some(WEST),
            };
            edge.say(WEST, seen);
        }
        let back = Durable::Departed {
            player: player(1),
            transfer: transfer(EntityId(5), 0),
            to: WEST,
        };
        edge.say(WEST, back);

        let (number, body) = edge.next_numbered(WEST).await;
        assert!(
            number == 2 && matches!(body, EdgeToWorker::PlayerArrive { .. }),
            "{number} {body:?}"
        );
        // Nothing else was sent the west: what the player does next has the next
        // number. And they are there by the arrival that is on its way, whatever the
        // welcome, which announced no answers, did not say of them.
        edge.settle(WEST).await;
        edge.input(player(1), step(1.5)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 3);
        assert!(connected(&mut packets));
    }

    /// A survivor's own word from before the merge that names the absorbed region is
    /// read while that region is still itself to the edge: the player is put under it
    /// and their arrival kept for it, and the word of the merge behind it brings both
    /// to the survivor.
    #[tokio::test]
    async fn a_player_let_go_to_a_region_that_was_then_absorbed_arrives_at_the_survivor() {
        let mut edge = Harness::start().await;
        let mut packets = edge.joined(player(1), EntityId(5)).await;

        edge.relink(WEST, 2).await;
        edge.tell(WEST, resumed_with(2, 1, 1));
        edge.say(WEST, departing_to_east(player(1), EntityId(5)));
        edge.say(WEST, east_absorbed(0, Vec::new()));
        edge.tell(
            WEST,
            WorkerToEdge::Presence {
                player: player(1),
                answer: Presence::Absent,
            },
        );
        let (number, body) = edge.next_numbered(WEST).await;
        assert!(
            number == 2 && matches!(body, EdgeToWorker::PlayerArrive { .. }),
            "{number} {body:?}"
        );
        edge.settle(WEST).await;
        edge.input(player(1), step(65.5)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 3);
        assert!(connected(&mut packets));
    }

    /// The routing table says that a region the edge still has something of went into
    /// another. The edge acts on the survivor's word, not on the table's; a link to
    /// the survivor that is through its welcome was answered without that word, and
    /// the edge ends it to say a hello that is answered from after the merge.
    #[tokio::test]
    async fn a_merge_the_routing_table_tells_first_is_acted_on_when_the_survivor_tells_it() {
        let mut edge = Harness::start().await;
        let mut packets = stepping_in_the_east(&mut edge).await;

        assert!(edge.relinks.absorbed(vec![(EAST, WEST)]).await);
        let ended = timeout(SOON, edge.relinks.ended()).await.unwrap();
        assert_eq!(ended, Some((WEST, 1)));
        // Nothing was moved on the table's word: what the player does still goes east.
        edge.input(player(1), step(66.5)).await;
        assert_eq!(edge.next_numbered(EAST).await.0, 3);

        edge.relink(WEST, 2).await;
        edge.tell(WEST, resumed_with(1, 1, 1));
        edge.say(WEST, east_absorbed(1, Vec::new()));
        edge.tell(WEST, says_present(player(1), EntityId(5)));
        assert_eq!(edge.next_numbered(WEST).await, (2, step_in_the_east()));
        assert_eq!(edge.next_numbered(WEST).await.0, 3);
        edge.settle(WEST).await;
        assert!(connected(&mut packets));
    }

    /// The survivor answers a hello said after the table told of the merge without a
    /// word of it: the absorbed region had forgotten the edge, or the survivor has
    /// since. What was kept for the absorbed region is given up, and its players are
    /// the survivor's to answer for.
    #[tokio::test]
    async fn a_merge_the_survivor_never_tells_of_gives_up_what_was_kept_for_the_absorbed_region() {
        let mut edge = Harness::start().await;
        let mut packets = stepping_in_the_east(&mut edge).await;

        assert!(edge.relinks.absorbed(vec![(EAST, WEST)]).await);
        let ended = timeout(SOON, edge.relinks.ended()).await.unwrap();
        assert_eq!(ended, Some((WEST, 1)));
        edge.relink(WEST, 2).await;
        edge.tell(WEST, resumed_with(0, 0, 1));
        // Not the step the east never applied: only the leave of the player the
        // survivor did not answer for.
        let leave = EdgeToWorker::PlayerLeave {
            player: player(1),
            entity: Some(EntityId(5)),
        };
        assert_eq!(edge.next_numbered(WEST).await, (2, leave));
        disconnected(&mut packets).await;
    }

    /// A region of which the edge has nothing stands for the one it went into on the
    /// routing table's word: there is nothing a word of the survivor could move. What
    /// another region then sends there goes to the survivor.
    #[tokio::test]
    async fn a_region_the_edge_has_nothing_of_stands_for_its_survivor_on_the_tables_word() {
        let mut edge = Harness::start().await;
        let mut packets = edge.joined(player(1), EntityId(5)).await;
        assert!(edge.relinks.absorbed(vec![(EAST, WEST)]).await);
        let ended = timeout(SOON, edge.relinks.ended()).await.unwrap();
        assert_eq!(ended, Some((EAST, 1)));

        // The west's own word from before the merge: an arrival at the west itself.
        edge.say(WEST, departing_to_east(player(1), EntityId(5)));
        let (number, body) = edge.next_numbered(WEST).await;
        assert!(
            number == 2 && matches!(body, EdgeToWorker::PlayerArrive { .. }),
            "{number} {body:?}"
        );
        edge.settle(WEST).await;
        assert!(connected(&mut packets));
    }

    fn split_off(players: Vec<(PlayerId, EntityId)>) -> Durable {
        Durable::SplitOff {
            region: PART,
            players,
        }
    }

    /// A region that was split says which stays went into the part. Those the edge has
    /// under the region are the part's: the part is asked for what they see in its
    /// first hello, and sent what they did after its welcome, without an arrival. A
    /// stay the edge does not have is ended when the part says it has it.
    #[tokio::test]
    async fn the_stays_a_split_took_are_the_parts_from_the_word_of_it() {
        let mut edge = Harness::start().await;
        let mut packets = edge.joined(player(1), EntityId(5)).await;
        edge.input(player(1), step(1.5)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 2);

        edge.relink(WEST, 2).await;
        edge.tell(WEST, resumed_with(1, 1, 1));
        let gone = vec![(player(1), EntityId(5)), (player(2), EntityId(9))];
        edge.say(WEST, split_off(gone));
        // The split region no longer has them, which costs them nothing.
        edge.tell(
            WEST,
            WorkerToEdge::Presence {
                player: player(1),
                answer: Presence::Absent,
            },
        );
        edge.settle(WEST).await;
        assert!(connected(&mut packets));

        let hello = edge.link_part().await;
        let EdgeToWorker::Hello {
            players, chunks, ..
        } = hello
        else {
            panic!("{hello:?}");
        };
        assert_eq!(players, [player(1)]);
        assert!(chunks.contains(&SHARED), "{chunks:?}");
        let welcome = Welcome::Unknown {
            since: 3,
            entries: 0,
            presences: 2,
            applied: 0,
        };
        edge.tell(PART, WorkerToEdge::Welcome(welcome));
        edge.tell(PART, says_present(player(1), EntityId(5)));
        edge.tell(PART, says_present(player(2), EntityId(9)));
        let again = EdgeToWorker::Input {
            player: player(1),
            entity: EntityId(5),
            number: 1,
            input: step(1.5),
        };
        assert_eq!(edge.next_numbered(PART).await, (1, again));
        let leave = EdgeToWorker::PlayerLeave {
            player: player(2),
            entity: Some(EntityId(9)),
        };
        assert_eq!(edge.next_numbered(PART).await, (2, leave));
        assert!(connected(&mut packets));
    }

    /// The word of a split is as old as the split. The edge can have heard from the
    /// part first, moved the stay there on its answer, and seen the player walk back:
    /// the split region's word, read then, does not take them to the part again.
    #[tokio::test]
    async fn the_word_of_a_split_does_not_take_back_a_stay_that_has_returned() {
        let mut edge = Harness::start().await;
        let mut packets = edge.joined(player(1), EntityId(5)).await;

        let hello = edge.link_part().await;
        let EdgeToWorker::Hello { players, .. } = hello else {
            panic!("{hello:?}");
        };
        assert_eq!(players, []);
        let welcome = Welcome::Unknown {
            since: 3,
            entries: 0,
            presences: 1,
            applied: 0,
        };
        edge.tell(PART, WorkerToEdge::Welcome(welcome));
        edge.tell(PART, says_present(player(1), EntityId(5)));
        edge.settle(PART).await;

        // They walk back into the region that was split.
        let back = Durable::Departed {
            player: player(1),
            transfer: transfer(EntityId(5), 0),
            to: WEST,
        };
        edge.say(PART, back);
        let (number, body) = edge.next_numbered(WEST).await;
        assert!(
            number == 2 && matches!(body, EdgeToWorker::PlayerArrive { .. }),
            "{number} {body:?}"
        );

        edge.relink(WEST, 2).await;
        edge.tell(WEST, resumed_with(1, 1, 1));
        edge.say(WEST, split_off(vec![(player(1), EntityId(5))]));
        edge.tell(
            WEST,
            WorkerToEdge::Presence {
                player: player(1),
                answer: Presence::Absent,
            },
        );
        // The arrival is sent again, and what they do goes to the west.
        let (number, body) = edge.next_numbered(WEST).await;
        assert!(
            number == 2 && matches!(body, EdgeToWorker::PlayerArrive { .. }),
            "{number} {body:?}"
        );
        edge.settle(WEST).await;
        edge.input(player(1), step(1.5)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 3);
        assert!(connected(&mut packets));
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

/// The scenarios of ADR-0013, section 8, and more of their kind, written from that
/// record, from the contract of ADR-0012, section 5, and from ADR-0008 by someone who
/// had not read the code above. What a test expects is what the records say. A test
/// marked as a finding is one the code does not pass.
#[cfg(test)]
mod scenarios {
    use clustine_protocol::packets::play::ClientboundPlay;
    use clustine_rpc::link::WorkerEnd;
    use clustine_sim::api::{Pose, RemoteAction, RemoteStep};
    use tokio::task::JoinHandle;
    use tokio::time::timeout;
    use uuid::Uuid;

    use super::*;
    use crate::Relinks;

    /// How long a test waits for something that has to happen.
    const SOON: Duration = Duration::from_secs(10);

    /// The three regions of these tests. Players enter the world in the western one.
    /// Which of them holds a chunk is whatever the test has them say: the edge knows
    /// nothing else about it.
    const WEST: RegionId = RegionId(0);
    const EAST: RegionId = RegionId(1);
    const NORTH: RegionId = RegionId(2);
    const REGIONS: [RegionId; 3] = [WEST, EAST, NORTH];
    /// Regions that come and go in the tests of merges and splits: one more that is
    /// there from the start of the world, and the parts that are split off.
    const SOUTH: RegionId = RegionId(3);
    const PART: RegionId = RegionId(4);
    const SECOND_PART: RegionId = RegionId(5);
    /// Where the witness of `Harness::witnessed` is kept. No test gives it a link.
    const ASIDE: RegionId = RegionId(11);
    /// How many regions a test can play: the ids below this.
    const PORTS: usize = 12;

    const IDENTITY: EdgeIdentity = EdgeIdentity {
        edge: clustine_world::EdgeId(7),
        start: 100,
    };

    /// Where players enter the world, which is in the chunk `HOME`.
    const SPAWN: Vec3 = Vec3::new(0.5, -60.0, 0.5);
    const HOME: ChunkPos = ChunkPos::new(0, 0);
    /// The view distance of every player in these tests: a view is seven chunks wide.
    const VIEW: i32 = 2;

    /// What a region takes a subscription for.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum Role {
        Viewer,
        Guest,
    }

    /// What the edge has said on its current link to a region.
    #[derive(Debug, Default)]
    struct Heard {
        /// The number of the last subscription message.
        asked: u64,
        /// What the region is subscribed to by those messages and the hello, each with
        /// the number of the last message that named it.
        subscriptions: BTreeMap<ChunkPos, (Role, u64)>,
        /// What the edge said that no test has looked at yet, in order.
        said: VecDeque<EdgeMessage>,
        /// How many players the hello of the link named: as many presence answers as
        /// a welcome on it announces.
        named: u32,
        /// The highest number of a numbered message the edge has sent on the link.
        numbered: u64,
    }

    impl Heard {
        /// The chunks the region is subscribed to as `role`.
        fn chunks(&self, role: Role) -> BTreeSet<ChunkPos> {
            self.subscriptions
                .iter()
                .filter(|(_, (kind, _))| *kind == role)
                .map(|(chunk, _)| *chunk)
                .collect()
        }

        /// Takes a message of the edge into what the region is subscribed to. A region
        /// ends a link whose subscription messages do not ascend, so that is checked
        /// for everything any test reads.
        fn note(&mut self, region: RegionId, message: &EdgeMessage) {
            if let Some(number) = message.number {
                self.numbered = self.numbered.max(number);
            }
            let (ask, chunks, role) = match &message.body {
                EdgeToWorker::Subscribe { ask, chunks } => (*ask, chunks, Some(Role::Viewer)),
                EdgeToWorker::SubscribeAsGuest { ask, chunks } => (*ask, chunks, Some(Role::Guest)),
                EdgeToWorker::Unsubscribe { ask, chunks } => (*ask, chunks, None),
                _ => return,
            };
            assert!(
                ask > self.asked,
                "{region:?} was sent the subscription message {ask} after {}: {message:?}",
                self.asked
            );
            assert!(!chunks.is_empty(), "{region:?} was sent {message:?}");
            self.asked = ask;
            for chunk in chunks {
                match role {
                    Some(role) => self.subscriptions.insert(*chunk, (role, ask)),
                    None => self.subscriptions.remove(chunk),
                };
            }
        }
    }

    /// What a hello of the edge says.
    #[derive(Debug)]
    struct Hello {
        since: u64,
        seen: u64,
        players: Vec<PlayerId>,
        chunks: Vec<ChunkPos>,
        guests: Vec<ChunkPos>,
    }

    /// A fan-out task with three regions played by the test.
    struct Harness {
        commands: mpsc::Sender<Command>,
        relinks: Relinks,
        /// The workers' ends of the links to the regions; none where the test has
        /// given the edge no link, or has dropped it.
        links: Vec<Option<WorkerEnd>>,
        heard: Vec<Heard>,
        task: JoinHandle<Stopped>,
        sessions: u64,
        /// The number of the last outbox entry each region has made.
        outbox: [u64; PORTS],
        /// The epoch of the owner each region's last link went to.
        epochs: [u64; PORTS],
        /// The last sequence number used to find out that the edge has got to a
        /// player's commands.
        sequences: i32,
        /// The witness of `Harness::witnessed`, if the test has one.
        witness: Option<Client>,
        /// The hello the edge said last to each region on a link that a step of a
        /// script made.
        hellos: Vec<Option<Hello>>,
        /// The links the edge has said have ended, of which no test has asked.
        ended: Vec<(RegionId, u64)>,
        /// The regions whose link the edge closed while a test waited for the edge
        /// to read what the region had said on it.
        gone: BTreeSet<RegionId>,
    }

    /// A link to `region` whose owner has `epoch`, and the worker's end of it.
    fn link_to(region: RegionId, epoch: u64) -> (RegionLink, WorkerEnd) {
        let (end, worker) = link::in_process(1024);
        (RegionLink { region, epoch, end }, worker)
    }

    impl Harness {
        /// An edge linked to all three regions, each of which has been said hello to
        /// and has answered that it does not know the edge, as at the start of
        /// everything.
        async fn start() -> Self {
            Self::start_with(&REGIONS, Duration::from_secs(1_000_000)).await
        }

        /// An edge that starts with links to `linked` only.
        async fn start_with(linked: &[RegionId], region_patience: Duration) -> Self {
            let mut links = Vec::new();
            let mut ends: Vec<Option<WorkerEnd>> = (0..PORTS).map(|_| None).collect();
            for region in linked {
                let (link, end) = link_to(*region, 1);
                links.push(link);
                ends[region.0 as usize] = Some(end);
            }
            let (routing, relinks) = Routing::new(WEST, SPAWN, IDENTITY, links);
            let config = FanoutConfig {
                max_players: 20,
                view_distance: VIEW,
                online: Arc::default(),
                region_patience,
            };
            let (commands, receiver) = mpsc::channel(64);
            let task = tokio::spawn(Fanout::new(config, routing, receiver).run());
            let mut harness = Self {
                commands,
                relinks,
                links: ends,
                heard: (0..PORTS).map(|_| Heard::default()).collect(),
                task,
                sessions: 0,
                outbox: [0; PORTS],
                epochs: [1; PORTS],
                sequences: 0,
                witness: None,
                hellos: (0..PORTS).map(|_| None).collect(),
                ended: Vec::new(),
                gone: BTreeSet::new(),
            };
            for region in linked {
                let hello = harness.read(*region).await;
                assert!(
                    matches!(
                        &hello.body,
                        EdgeToWorker::Hello { seen: 0, players, chunks, guests, .. }
                            if players.is_empty() && chunks.is_empty() && guests.is_empty()
                    ),
                    "{hello:?}"
                );
                harness.tell(
                    *region,
                    WorkerToEdge::Welcome(Welcome::Unknown {
                        since: 1,
                        entries: 0,
                        presences: 0,
                        applied: 0,
                    }),
                );
            }
            harness
        }

        /// Reads what the edge sends `region` next. The edge checks what it holds
        /// about subscriptions at the end of its turns and panics if it is wrong, so
        /// a task that has ended is what a test finds here.
        async fn read(&mut self, region: RegionId) -> EdgeMessage {
            let index = region.0 as usize;
            let link = self.links[index].as_mut().expect("the region has a link");
            let message = tokio::select! {
                biased;
                ended = &mut self.task => panic!("the edge's task ended: {ended:?}"),
                message = timeout(SOON, link.recv()) => message,
            };
            let message = message.unwrap_or_else(|_| panic!("the edge sent {region:?} nothing"));
            let Some(message) = message else {
                let ended = timeout(SOON, &mut self.task).await;
                panic!("the edge closed its link to {region:?}: {ended:?}");
            };
            self.heard[index].note(region, &message);
            message
        }

        /// What the edge said to `region` next that no test has looked at.
        async fn next(&mut self, region: RegionId) -> EdgeMessage {
            match self.heard[region.0 as usize].said.pop_front() {
                Some(message) => message,
                None => self.read(region).await,
            }
        }

        /// What the edge said to `region` next that is numbered, with its number.
        async fn next_numbered(&mut self, region: RegionId) -> (u64, EdgeToWorker) {
            loop {
                let EdgeMessage { number, body } = self.next(region).await;
                if let Some(number) = number {
                    return (number, body);
                }
            }
        }

        /// What the edge said to `region` next about its subscriptions.
        async fn next_asked(&mut self, region: RegionId) -> EdgeToWorker {
            loop {
                let body = self.next(region).await.body;
                if is_about_subscriptions(&body) {
                    return body;
                }
            }
        }

        /// Says something as `region`.
        fn tell(&mut self, region: RegionId, message: WorkerToEdge) {
            let index = region.0 as usize;
            // A region that says `NotMine` to a guest has ended that subscription.
            if let WorkerToEdge::NotMine { chunk, ask } = &message {
                let subscriptions = &mut self.heard[index].subscriptions;
                if subscriptions.get(chunk) == Some(&(Role::Guest, *ask)) {
                    subscriptions.remove(chunk);
                }
            }
            let sent = self.links[index]
                .as_ref()
                .expect("the region has a link")
                .try_send(message);
            if let Err(error) = sent {
                let ended = self.task.is_finished();
                panic!("{region:?} cannot say anything: {error:?}; the edge's task ended: {ended}");
            }
        }

        /// Says `entry` as the next entry of the outbox of `region`. Returns its number.
        fn say(&mut self, region: RegionId, entry: Durable) -> u64 {
            let number = &mut self.outbox[region.0 as usize];
            *number += 1;
            let number = *number;
            self.tell(region, WorkerToEdge::Outbox { number, entry });
            number
        }

        /// Waits until the edge has handled everything `region` has said so far, by an
        /// entry about nobody, which the edge confirms like any other. What the edge
        /// said to the region meanwhile is kept for the test to look at. The edge
        /// takes what regions say, what players do and new links from different
        /// queues, in no order between them, so a test that depends on the edge having
        /// heard something makes sure of it with this.
        async fn settle(&mut self, region: RegionId) {
            let nobody = Durable::RemoteDone {
                player: player(u128::MAX),
                sequence: 0,
            };
            let number = self.say(region, nobody);
            loop {
                let message = self.read(region).await;
                if message.body == (EdgeToWorker::Confirm { number }) {
                    return;
                }
                self.heard[region.0 as usize].said.push_back(message);
            }
        }

        /// Everything the edge has said to `region` that no test has looked at, once
        /// it has handled what the region has said so far.
        async fn said(&mut self, region: RegionId) -> Vec<EdgeMessage> {
            self.settle(region).await;
            self.heard[region.0 as usize].said.drain(..).collect()
        }

        /// What the edge has said to `region` about subscriptions that no test has
        /// looked at, once it has handled what the region has said so far.
        async fn asked(&mut self, region: RegionId) -> Vec<EdgeToWorker> {
            let said = self.said(region).await;
            said.into_iter()
                .map(|message| message.body)
                .filter(is_about_subscriptions)
                .collect()
        }

        /// The number of the edge's last message to `region` that named `chunk`.
        fn ask(&self, region: RegionId, chunk: ChunkPos) -> u64 {
            let subscription = self.heard[region.0 as usize].subscriptions.get(&chunk);
            subscription
                .expect("the region is subscribed to the chunk")
                .1
        }

        /// Waits until the edge has taken every command sent to it. It handles one
        /// thing at a time, so what a region is told after this is handled after the
        /// commands.
        async fn drained(&mut self) {
            let waited = async {
                while self.commands.capacity() < self.commands.max_capacity() {
                    tokio::task::yield_now().await;
                }
            };
            timeout(SOON, waited)
                .await
                .expect("the edge takes its commands");
        }

        /// A player connects. Returns their client.
        async fn join(&mut self, player: PlayerId) -> Client {
            self.sessions += 1;
            let session = SessionId(self.sessions);
            let (outbound, packets) = mpsc::channel(4096);
            let join = Command::Join {
                session,
                profile: Profile {
                    uuid: player.0,
                    name: format!("Player{}", self.sessions),
                },
                requested_view_distance: None,
                outbound,
                awaiting_teleport: Arc::new(AtomicI32::new(NO_TELEPORT)),
            };
            self.commands.send(join).await.unwrap();
            Client::new(player, session, packets)
        }

        /// A player joins and the western region places them as `entity` where
        /// players enter the world. The edge has asked for their view when this
        /// returns, and no test has looked at that yet.
        async fn joined(&mut self, player: PlayerId, entity: EntityId) -> Client {
            let client = self.join(player).await;
            let (_, join) = self.next_numbered(WEST).await;
            assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
            self.tell(WEST, spawned(player, entity));
            self.settle(WEST).await;
            client
        }

        /// The player's connection ends.
        async fn leave(&mut self, client: &Client) {
            let leave = Command::Leave {
                session: client.session,
                player: client.player,
            };
            self.commands.send(leave).await.unwrap();
            self.drained().await;
        }

        /// Something the player did.
        async fn input(&mut self, client: &Client, input: PlayerInput) {
            let command = Command::Input {
                session: client.session,
                player: client.player,
                input,
            };
            self.commands.send(command).await.unwrap();
        }

        /// The next packet the edge sends the client; `None` once it has ended the
        /// connection.
        async fn packet(&mut self, client: &mut Client) -> Option<Bytes> {
            tokio::select! {
                biased;
                ended = &mut self.task => panic!("the edge's task ended: {ended:?}"),
                packet = timeout(SOON, client.packets.recv()) => {
                    packet.expect("the edge sent the client nothing")
                }
            }
        }

        /// Brings `client` up to date with everything the edge has sent it and will
        /// send it without anything else happening: chunks come in batches, each of
        /// which the client has to confirm before the next. The edge is asked to
        /// acknowledge an action that no region hears of, and what it sent before the
        /// acknowledgement is what it had to send.
        async fn sync(&mut self, client: &mut Client) {
            loop {
                client.drain();
                let confirmed = client.batches > 0;
                while client.batches > 0 {
                    client.batches -= 1;
                    let received = Command::ChunkBatchReceived {
                        session: client.session,
                        player: client.player,
                        chunks_per_tick: 64.0,
                    };
                    self.commands.send(received).await.unwrap();
                }
                let sequence = self.sequence();
                let handled = Command::Handled {
                    session: client.session,
                    player: client.player,
                    sequence,
                };
                self.commands.send(handled).await.unwrap();
                while client.acknowledged.last() != Some(&sequence) {
                    let packet = self.packet(client).await;
                    client.take(packet.expect("the edge ended the connection"));
                }
                if !confirmed && client.batches == 0 {
                    return;
                }
            }
        }

        /// Whether the client has `chunk`, once the edge has handled everything
        /// `region` has said so far.
        async fn shows(&mut self, client: &mut Client, region: RegionId, chunk: ChunkPos) -> bool {
            self.settle(region).await;
            self.sync(client).await;
            client.chunks.contains_key(&chunk)
        }

        /// Gives the edge a new link to `region`, in place of the one it has if it
        /// has one, and returns the hello the edge says on it.
        async fn relink(&mut self, region: RegionId) -> Hello {
            let index = region.0 as usize;
            self.epochs[index] += 1;
            let (link, worker) = link_to(region, self.epochs[index]);
            self.links[index] = Some(worker);
            self.heard[index] = Heard::default();
            assert!(self.relinks.replace(link).await);
            let hello = self.read(region).await;
            let EdgeToWorker::Hello {
                edge,
                start,
                since,
                seen,
                players,
                chunks,
                guests,
            } = hello.body
            else {
                panic!("expected a hello, got {hello:?}");
            };
            assert_eq!((edge, start), (IDENTITY.edge, IDENTITY.start));
            let heard = &mut self.heard[index];
            heard.named = players.len() as u32;
            for chunk in &guests {
                heard.subscriptions.insert(*chunk, (Role::Guest, 0));
            }
            // A chunk in both lists is a viewer's.
            for chunk in &chunks {
                heard.subscriptions.insert(*chunk, (Role::Viewer, 0));
            }
            Hello {
                since,
                seen,
                players,
                chunks,
                guests,
            }
        }

        /// The region's link ends, and the edge has found that out when this returns.
        async fn lose(&mut self, region: RegionId) {
            let index = region.0 as usize;
            drop(self.links[index].take().expect("the region has a link"));
            self.heard[index] = Heard::default();
            // Links that were replaced have ended as well, earlier.
            loop {
                let ended = timeout(SOON, self.relinks.ended())
                    .await
                    .expect("the edge notices that a link ended")
                    .expect("the edge is there");
                if ended == (region, self.epochs[index]) {
                    return;
                }
                self.ended.push(ended);
            }
        }

        /// The region answers a hello as one that knows the edge, with nothing in its
        /// outbox that the edge has not seen.
        fn resume(&mut self, region: RegionId) {
            // No region of these tests has told the edge of any progress.
            let welcome = Welcome::Resumed {
                entries: 0,
                presences: self.at(region).named,
                applied: 0,
            };
            self.tell(region, WorkerToEdge::Welcome(welcome));
        }

        /// What the edge has said on its current link to `region`, as far as it was read.
        fn at(&self, region: RegionId) -> &Heard {
            &self.heard[region.0 as usize]
        }

        /// `from` lets the player go to `to`, into `chunk`. The edge has handled that
        /// when this returns.
        async fn hand(
            &mut self,
            player: PlayerId,
            entity: EntityId,
            from: RegionId,
            to: RegionId,
            chunk: ChunkPos,
        ) {
            self.say(from, departed(player, entity, to, chunk));
            self.settle(from).await;
        }

        /// Waits until the edge has handled everything the regions have said so far,
        /// reads everything it has said to them, and has the test look at none of it.
        async fn quiet(&mut self) {
            // Twice, as what one region said can make the edge say something to a
            // region that was settled before.
            for _ in 0..2 {
                for region in self.linked() {
                    self.settle(region).await;
                }
            }
            for heard in &mut self.heard {
                heard.said.clear();
            }
        }

        /// What the edge has said to `region` that no test has looked at, as far as it
        /// is on the link now: nothing is waited for.
        fn waiting(&mut self, region: RegionId) -> Vec<EdgeMessage> {
            let index = region.0 as usize;
            let mut waiting: Vec<_> = self.heard[index].said.drain(..).collect();
            let link = self.links[index].as_mut().expect("the region has a link");
            while let Ok(Some(message)) = link.try_recv() {
                self.heard[index].note(region, &message);
                waiting.push(message);
            }
            waiting
        }

        /// Looks through what the edge has said to `region` that no test has looked
        /// at for a player who arrives. Returns the number of the arrival and the
        /// chunks the region was asked for as a viewer's before it.
        async fn arrived(&mut self, region: RegionId) -> (u64, BTreeSet<ChunkPos>) {
            let said = self.said(region).await;
            let arrival = said
                .iter()
                .position(|message| matches!(message.body, EdgeToWorker::PlayerArrive { .. }))
                .unwrap_or_else(|| panic!("nobody arrived in {region:?}: {said:?}"));
            let mut before = BTreeSet::new();
            for message in &said[..arrival] {
                if let EdgeToWorker::Subscribe { chunks, .. } = &message.body {
                    before.extend(chunks.iter().copied());
                }
            }
            let number = said[arrival].number.expect("an arrival is numbered");
            (number, before)
        }

        /// Makes sure that the edge has said nothing numbered to any region that no
        /// test has looked at.
        async fn nothing_numbered(&mut self) {
            for region in self.linked() {
                let said = numbered(&self.said(region).await);
                assert!(said.is_empty(), "{region:?} was sent {said:?}");
            }
        }

        /// The regions the test plays a link of, in ascending order.
        fn linked(&self) -> Vec<RegionId> {
            let linked = |index: &usize| self.links[*index].is_some();
            let regions = (0..PORTS).filter(linked);
            regions.map(|index| RegionId(index as u32)).collect()
        }

        /// A sequence number for an action of a player, above every one used before.
        fn sequence(&mut self) -> i32 {
            self.sequences += 1;
            self.sequences
        }

        /// Waits until the client is told that its action `sequence` was handled.
        async fn acknowledged(&mut self, client: &mut Client, sequence: i32) {
            while !client.acknowledged.contains(&sequence) {
                let packet = self.packet(client).await;
                client.take(packet.expect("the edge ended the connection"));
            }
        }

        /// Makes sure that the edge has got through everything said to it without
        /// finding its own account of subscriptions wrong.
        async fn end(mut self) {
            self.drained().await;
            // A link the edge has closed by itself is not one to settle.
            if self.witness.is_some() {
                for region in self.linked() {
                    self.handled(region).await;
                }
            }
            self.quiet().await;
            assert!(!self.task.is_finished(), "the edge's task ended");
        }
    }

    fn is_about_subscriptions(body: &EdgeToWorker) -> bool {
        matches!(
            body,
            EdgeToWorker::Subscribe { .. }
                | EdgeToWorker::SubscribeAsGuest { .. }
                | EdgeToWorker::Unsubscribe { .. }
        )
    }

    /// What a player's client has been sent.
    struct Client {
        player: PlayerId,
        session: SessionId,
        packets: mpsc::Receiver<Bytes>,
        /// The chunks the client has, each with what was at `block_of` it when it was
        /// sent last, if that is one of the few states these tests put there.
        chunks: BTreeMap<ChunkPos, Option<BlockState>>,
        /// Every chunk the client was sent, in order.
        sent: Vec<ChunkPos>,
        /// The blocks it was told have changed since their chunk was sent, each as it
        /// was told last.
        blocks: BTreeMap<BlockPos, i32>,
        /// The entities it shows.
        entities: BTreeSet<i32>,
        /// Every action it was told was handled, in order.
        acknowledged: Vec<i32>,
        /// Batches of chunks it has been sent and has not confirmed.
        batches: u32,
        /// Why the edge said it disconnects the player, if it did.
        disconnected: Option<String>,
        /// Whether the edge has ended the connection.
        closed: bool,
    }

    impl Client {
        fn new(player: PlayerId, session: SessionId, packets: mpsc::Receiver<Bytes>) -> Self {
            Self {
                player,
                session,
                packets,
                chunks: BTreeMap::new(),
                sent: Vec::new(),
                blocks: BTreeMap::new(),
                entities: BTreeSet::new(),
                acknowledged: Vec::new(),
                batches: 0,
                disconnected: None,
                closed: false,
            }
        }

        /// Takes a packet as a client would.
        fn take(&mut self, packet: Bytes) {
            let decoded = ClientboundPlay::decode(&packet).expect("the edge sends packets");
            match decoded {
                ClientboundPlay::LevelChunkWithLight(chunk) => {
                    let position = ChunkPos::new(chunk.chunk_x, chunk.chunk_z);
                    let state = STATES.into_iter().find(|state| {
                        packet == encoded(&chunk_packet(position, &chunk_with(position, *state)))
                    });
                    self.chunks.insert(position, state);
                    self.sent.push(position);
                    self.blocks.retain(|block, _| block.chunk() != position);
                }
                ClientboundPlay::UnloadChunk(chunk) => {
                    let position = ChunkPos::new(chunk.chunk_x, chunk.chunk_z);
                    self.chunks.remove(&position);
                    self.blocks.retain(|block, _| block.chunk() != position);
                }
                ClientboundPlay::BlockUpdate(update) => {
                    let Position { x, y, z } = update.position;
                    self.blocks.insert(BlockPos::new(x, y, z), update.state);
                }
                ClientboundPlay::SpawnEntity(entity) => {
                    self.entities.insert(entity.entity_id);
                }
                ClientboundPlay::RemoveEntities(removed) => {
                    for entity in removed.entity_ids {
                        self.entities.remove(&entity);
                    }
                }
                ClientboundPlay::AcknowledgeBlockChange(acknowledged) => {
                    self.acknowledged.push(acknowledged.sequence);
                }
                ClientboundPlay::ChunkBatchFinished(_) => self.batches += 1,
                ClientboundPlay::Disconnect(disconnect) => {
                    self.disconnected = Some(format!("{:?}", disconnect.reason));
                }
                _ => {}
            }
        }

        /// Takes everything the edge has sent so far.
        fn drain(&mut self) {
            loop {
                match self.packets.try_recv() {
                    Ok(packet) => self.take(packet),
                    Err(mpsc::error::TryRecvError::Empty) => return,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        self.closed = true;
                        return;
                    }
                }
            }
        }

        /// Whether the edge still serves the player: their queue is open.
        fn connected(&mut self) -> bool {
            self.drain();
            !self.closed
        }

        /// What the client shows at `block_of(chunk)`, if it has the chunk and that is
        /// one of the few states these tests put there: by the chunk as it was sent
        /// last, or by what the client was told of the block since.
        fn state(&self, chunk: ChunkPos) -> Option<BlockState> {
            let sent = self.chunks.get(&chunk)?;
            match self.blocks.get(&block_of(chunk)) {
                Some(state) => Some(BlockState(*state as u16)),
                None => *sent,
            }
        }

        /// How often the client was sent `chunk`.
        fn times_sent(&self, chunk: ChunkPos) -> usize {
            self.sent.iter().filter(|sent| **sent == chunk).count()
        }

        /// Waits until the connection is ended by the edge.
        async fn disconnected(&mut self) {
            let ended = async {
                while let Some(packet) = self.packets.recv().await {
                    self.take(packet);
                }
            };
            timeout(SOON, ended)
                .await
                .expect("the player was not disconnected");
            self.closed = true;
        }
    }

    fn player(number: u128) -> PlayerId {
        PlayerId(Uuid::from_u128(number))
    }

    /// The chunks someone in `centre` sees.
    fn view(centre: ChunkPos) -> BTreeSet<ChunkPos> {
        view_area(centre, VIEW)
    }

    /// A place to stand in `chunk`.
    fn within(chunk: ChunkPos) -> Vec3 {
        Vec3::new(
            f64::from(chunk.x) * 16.0 + 0.5,
            -60.0,
            f64::from(chunk.z) * 16.0 + 0.5,
        )
    }

    /// A block of `chunk`.
    fn block_of(chunk: ChunkPos) -> BlockPos {
        BlockPos::new(chunk.x * 16 + 3, -60, chunk.z * 16 + 5)
    }

    /// A region's word to a player that they have entered the world as `entity`.
    fn spawned(player: PlayerId, entity: EntityId) -> WorkerToEdge {
        WorkerToEdge::ToPlayer {
            player,
            event: PlayerEvent::Spawned {
                entity_id: entity,
                position: SPAWN,
                hotbar: [None; HOTBAR_SLOTS],
                selected_slot: 0,
            },
        }
    }

    /// A player as a region lets them go, standing in `chunk`.
    fn transfer(entity: EntityId, last_input: u64, chunk: ChunkPos) -> PlayerTransfer {
        PlayerTransfer {
            entity_id: entity,
            name: "Player".to_owned(),
            pose: Pose::at(within(chunk)),
            hotbar: [None; HOTBAR_SLOTS],
            selected_slot: 0,
            last_input,
        }
    }

    /// The outbox entry with which a region lets a player go to `to`, into `chunk`.
    fn departed(player: PlayerId, entity: EntityId, to: RegionId, chunk: ChunkPos) -> Durable {
        Durable::Departed {
            player,
            transfer: transfer(entity, 0, chunk),
            to,
        }
    }

    /// The outbox entry with which a region sends an arrival on to `holder`.
    fn sent_on(player: PlayerId, entity: EntityId, holder: RegionId, chunk: ChunkPos) -> Durable {
        Durable::NotMine {
            what: Misdirected::Arrival {
                player,
                transfer: transfer(entity, 0, chunk),
            },
            holder,
        }
    }

    /// A region's word that `entity` has walked from one chunk into another.
    fn walked(entity: EntityId, from: ChunkPos, to: ChunkPos) -> WorkerToEdge {
        WorkerToEdge::TickDelta {
            tick: 1,
            events: vec![RegionEvent::EntityMoved {
                entity,
                pose: Pose::at(within(to)),
                previous_chunk: from,
            }],
        }
    }

    fn empty_chunk() -> Chunk {
        let overworld = clustine_data::DIMENSION_TYPES
            .iter()
            .find(|dimension| dimension.name == OVERWORLD)
            .expect("the overworld is a dimension");
        Chunk::empty(overworld, clustine_world::Biome(0))
    }

    /// A chunk that is empty but for `state` at `block_of` it.
    fn chunk_with(position: ChunkPos, state: BlockState) -> Chunk {
        let mut chunk = empty_chunk();
        let block = block_of(position);
        let (x, z) = (block.x.rem_euclid(16), block.z.rem_euclid(16));
        chunk
            .set(x as usize, block.y, z as usize, state)
            .expect("the block is within the chunk");
        chunk
    }

    /// An empty chunk, as a region sends it in answer to a subscription numbered `ask`.
    fn snapshot(position: ChunkPos, ask: u64) -> WorkerToEdge {
        snapshot_of(position, ask, empty_chunk(), Vec::new())
    }

    fn snapshot_of(
        position: ChunkPos,
        ask: u64,
        chunk: Chunk,
        entities: Vec<EntityState>,
    ) -> WorkerToEdge {
        WorkerToEdge::ChunkSnapshot {
            position,
            ask,
            tick: 1,
            chunk,
            entities,
        }
    }

    fn elsewhere(chunk: ChunkPos, ask: u64, region: RegionId) -> WorkerToEdge {
        WorkerToEdge::Elsewhere { chunk, ask, region }
    }

    fn not_mine(chunk: ChunkPos, ask: u64) -> WorkerToEdge {
        WorkerToEdge::NotMine { chunk, ask }
    }

    /// A region's word that the block at `block_of(chunk)` has become `state`.
    fn changed(chunk: ChunkPos, state: BlockState) -> WorkerToEdge {
        WorkerToEdge::TickDelta {
            tick: 2,
            events: vec![RegionEvent::BlockChanged {
                position: block_of(chunk),
                state,
            }],
        }
    }

    /// A region's word that the player is in it, as `entity`.
    fn present(player: PlayerId, entity: EntityId) -> WorkerToEdge {
        WorkerToEdge::Presence {
            player,
            answer: Presence::Present {
                entity,
                pose: Pose::at(SPAWN),
                hotbar: [None; HOTBAR_SLOTS],
                selected_slot: 0,
                last_input: 0,
                handled: None,
            },
        }
    }

    fn ordered(chunks: impl IntoIterator<Item = ChunkPos>) -> Vec<ChunkPos> {
        let mut chunks: Vec<_> = chunks.into_iter().collect();
        chunks.sort();
        chunks
    }

    fn subscribe(ask: u64, chunks: impl IntoIterator<Item = ChunkPos>) -> EdgeToWorker {
        let chunks = ordered(chunks);
        EdgeToWorker::Subscribe { ask, chunks }
    }

    fn as_guest(ask: u64, chunks: impl IntoIterator<Item = ChunkPos>) -> EdgeToWorker {
        let chunks = ordered(chunks);
        EdgeToWorker::SubscribeAsGuest { ask, chunks }
    }

    fn unsubscribe(ask: u64, chunks: impl IntoIterator<Item = ChunkPos>) -> EdgeToWorker {
        let chunks = ordered(chunks);
        EdgeToWorker::Unsubscribe { ask, chunks }
    }

    /// What a subscription message makes of the chunks it names (nothing, if it ends
    /// their subscriptions), and those chunks.
    fn meaning(body: &EdgeToWorker) -> (Option<Role>, BTreeSet<ChunkPos>) {
        let (role, chunks) = match body {
            EdgeToWorker::Subscribe { chunks, .. } => (Some(Role::Viewer), chunks),
            EdgeToWorker::SubscribeAsGuest { chunks, .. } => (Some(Role::Guest), chunks),
            EdgeToWorker::Unsubscribe { chunks, .. } => (None, chunks),
            other => panic!("{other:?} is not about subscriptions"),
        };
        (role, chunks.iter().copied().collect())
    }

    /// What some subscription messages make of the chunks they name, without the order
    /// and the numbers they came with.
    fn meanings(asked: &[EdgeToWorker]) -> BTreeSet<(Option<Role>, BTreeSet<ChunkPos>)> {
        asked.iter().map(meaning).collect()
    }

    /// Whether a subscription message names `chunk`.
    fn names(body: &EdgeToWorker, chunk: ChunkPos) -> bool {
        is_about_subscriptions(body) && meaning(body).1.contains(&chunk)
    }

    /// The numbered messages among `said`, each with its number.
    fn numbered(said: &[EdgeMessage]) -> Vec<(u64, EdgeToWorker)> {
        said.iter()
            .filter_map(|message| Some((message.number?, message.body.clone())))
            .collect()
    }

    /// A player's step into `chunk`.
    fn step_into(chunk: ChunkPos) -> PlayerInput {
        PlayerInput::Move {
            position: Some(within(chunk)),
            rotation: None,
            on_ground: true,
        }
    }

    /// What is left of a player's breaking of the block at `block_of(chunk)`.
    fn breaking(player: PlayerId, sequence: i32, chunk: ChunkPos) -> RemoteAction {
        RemoteAction {
            player,
            sequence,
            step: RemoteStep::Break {
                position: block_of(chunk),
            },
        }
    }

    /// The outbox entry with which a region passes an action on, to a region or to
    /// whoever serves the edge the chunk.
    fn remote(action: &RemoteAction, to: Option<RegionId>) -> Durable {
        Durable::Remote {
            action: action.clone(),
            to,
        }
    }

    /// Someone who is not this edge's, standing in `chunk`.
    fn stranger(entity: EntityId, chunk: ChunkPos) -> EntityState {
        EntityState {
            entity,
            kind: EntityKind::Player {
                player: player(entity.0 as u128 + 1000),
                name: format!("Stranger{}", entity.0),
            },
            pose: Pose::at(within(chunk)),
        }
    }

    fn both(one: &BTreeSet<ChunkPos>, other: &BTreeSet<ChunkPos>) -> BTreeSet<ChunkPos> {
        one.intersection(other).copied().collect()
    }

    fn minus(one: &BTreeSet<ChunkPos>, other: &BTreeSet<ChunkPos>) -> BTreeSet<ChunkPos> {
        one.difference(other).copied().collect()
    }

    fn set(chunks: &[ChunkPos]) -> BTreeSet<ChunkPos> {
        chunks.iter().copied().collect()
    }

    /// The few states these tests put at `block_of` a chunk.
    const AIR: BlockState = BlockState(0);
    const STONE: BlockState = BlockState(1);
    const GRANITE: BlockState = BlockState(2);
    const STATES: [BlockState; 3] = [AIR, STONE, GRANITE];

    /// Where a player stands whom these tests hand to the eastern region, and one they
    /// hand to the northern.
    const EASTERN: ChunkPos = ChunkPos::new(4, 0);
    const NORTHERN: ChunkPos = ChunkPos::new(0, -4);
    /// A chunk that is seen from `HOME`, from `EASTERN` and from `NORTHERN`.
    const COMMON: ChunkPos = ChunkPos::new(2, -2);
    /// A chunk that is seen from `HOME` and from `NORTHERN`, and that someone who walks
    /// a chunk east from `HOME` sees no more.
    const FAR: ChunkPos = ChunkPos::new(-3, -1);
    /// The chunk east of `HOME`.
    const STEP_EAST: ChunkPos = ChunkPos::new(1, 0);

    /// Scenario 1. Everything a player sees is asked of the player's region, which for
    /// one who joins is the home region, in one message.
    #[tokio::test]
    async fn a_player_who_joins_has_their_view_asked_of_the_home_region_and_of_no_other() {
        let mut edge = Harness::start().await;
        let _client = edge.join(player(1)).await;
        let (number, join) = edge.next_numbered(WEST).await;
        assert!(
            number == 1 && matches!(join, EdgeToWorker::PlayerJoin(_)),
            "{join:?}"
        );
        edge.tell(WEST, spawned(player(1), EntityId(5)));
        assert_eq!(edge.asked(WEST).await, [subscribe(1, view(HOME))]);
        for region in [EAST, NORTH] {
            let said = edge.said(region).await;
            assert!(said.is_empty(), "{region:?} was sent {said:?}");
        }
        edge.end().await;
    }

    /// Scenario 2. The edge becomes a guest where a region names another, once: a
    /// third region that names the same holder finds the subscription made.
    #[tokio::test]
    async fn a_region_named_for_a_chunk_by_two_others_is_asked_as_a_guest_once() {
        let mut edge = Harness::start().await;
        let _first = edge.joined(player(1), EntityId(5)).await;
        let ask = edge.ask(WEST, COMMON);
        edge.tell(WEST, elsewhere(COMMON, ask, EAST));
        edge.settle(WEST).await;
        assert_eq!(edge.asked(EAST).await, [as_guest(1, [COMMON])]);

        // A second player becomes the northern region's and sees the chunk from there.
        let _second = edge.joined(player(2), EntityId(6)).await;
        edge.hand(player(2), EntityId(6), WEST, NORTH, NORTHERN)
            .await;
        edge.quiet().await;
        assert_eq!(edge.at(NORTH).chunks(Role::Viewer), view(NORTHERN));
        let ask = edge.ask(NORTH, COMMON);
        edge.tell(NORTH, elsewhere(COMMON, ask, EAST));
        edge.settle(NORTH).await;
        let asked = edge.asked(EAST).await;
        assert!(asked.is_empty(), "{asked:?}");
        assert_eq!(edge.at(EAST).chunks(Role::Guest), set(&[COMMON]));
        edge.end().await;
    }

    /// Scenario 3. A snapshot with the number its subscription began with is the
    /// region's word that it serves the chunk.
    #[tokio::test]
    async fn a_snapshot_with_the_number_its_subscription_began_with_is_shown() {
        let mut edge = Harness::start().await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        assert!(!edge.shows(&mut client, WEST, HOME).await);
        let ask = edge.ask(WEST, HOME);
        edge.tell(WEST, snapshot(HOME, ask));
        assert!(edge.shows(&mut client, WEST, HOME).await);
        assert_eq!(client.state(HOME), Some(AIR));
        edge.end().await;
    }

    /// Scenario 3. A viewer's subscription that waits becomes a guest's when its last
    /// viewer goes and another region's player still sees the chunk. The region had
    /// made its snapshot before it heard of that, and does not answer the change.
    #[tokio::test]
    async fn a_snapshot_is_shown_when_its_viewers_subscription_has_become_a_guests_since() {
        let mut edge = Harness::start().await;
        let first = edge.joined(player(1), EntityId(5)).await;
        let mut second = edge.joined(player(2), EntityId(6)).await;
        edge.hand(player(2), EntityId(6), WEST, NORTH, NORTHERN)
            .await;
        edge.quiet().await;
        let began = edge.ask(WEST, COMMON);

        // The first goes. The second, who is the northern region's, still sees it.
        edge.leave(&first).await;
        edge.quiet().await;
        let (role, later) = edge.at(WEST).subscriptions[&COMMON];
        assert_eq!(role, Role::Guest);
        assert!(later > began, "{later} {began}");
        assert!(!edge.shows(&mut second, WEST, COMMON).await);
        edge.tell(WEST, snapshot(COMMON, began));
        assert!(edge.shows(&mut second, WEST, COMMON).await);
        edge.end().await;
    }

    /// Scenario 3, and several changes to one subscription that the edge reads in one
    /// go: a chunk that leaves a view and comes back is an `Unsubscribe` and a
    /// `Subscribe`, and begins anew. A snapshot numbered below that is of the
    /// subscription that was ended.
    #[tokio::test]
    async fn a_chunk_that_leaves_a_view_and_comes_back_is_asked_for_anew() {
        let mut edge = Harness::start().await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        let before = edge.ask(WEST, FAR);
        edge.quiet().await;

        // Both steps are on the link before the edge reads either.
        edge.tell(WEST, walked(EntityId(5), HOME, STEP_EAST));
        edge.tell(WEST, walked(EntityId(5), STEP_EAST, HOME));
        let asked = edge.asked(WEST).await;
        let about: Vec<_> = asked.iter().filter(|body| names(body, FAR)).collect();
        let after = edge.ask(WEST, FAR);
        let [
            EdgeToWorker::Unsubscribe { ask: ended, .. },
            EdgeToWorker::Subscribe { ask: begun, .. },
        ] = about[..]
        else {
            panic!("{about:?}");
        };
        assert!(
            before < *ended && ended < begun && *begun == after,
            "{about:?}"
        );
        assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));

        edge.tell(WEST, snapshot(FAR, before));
        assert!(!edge.shows(&mut client, WEST, FAR).await);
        edge.tell(WEST, snapshot(FAR, after));
        assert!(edge.shows(&mut client, WEST, FAR).await);
        edge.end().await;
    }

    /// Scenario 3. An answer for a chunk the edge has no subscription for at that
    /// region is passed over.
    #[tokio::test]
    async fn a_snapshot_from_a_region_the_edge_has_no_subscription_at_is_not_shown() {
        let mut edge = Harness::start().await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        let ask = edge.ask(WEST, HOME);
        for number in [0, ask, ask + 1] {
            edge.tell(EAST, snapshot(HOME, number));
        }
        assert!(!edge.shows(&mut client, EAST, HOME).await);
        // The region that was asked is still listened to.
        edge.tell(WEST, snapshot(HOME, ask));
        assert!(edge.shows(&mut client, WEST, HOME).await);
        edge.end().await;
    }

    /// Scenario 4. The edge is a guest at the east and waits; the east makes its
    /// snapshot; the player is handed to the east, which makes the subscription a
    /// viewer's under a later number; then the snapshot is read, with the guest's
    /// number. The east does not answer the change, so this is the only answer there
    /// will be.
    #[tokio::test]
    async fn a_guests_snapshot_in_flight_is_shown_when_the_player_has_been_handed_there_since() {
        let mut edge = Harness::start().await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        let ask = edge.ask(WEST, COMMON);
        edge.tell(WEST, elsewhere(COMMON, ask, EAST));
        edge.settle(WEST).await;
        assert_eq!(edge.asked(EAST).await, [as_guest(1, [COMMON])]);

        edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
        edge.quiet().await;
        let (role, later) = edge.at(EAST).subscriptions[&COMMON];
        assert_eq!(role, Role::Viewer);
        assert!(later > 1, "{later}");
        assert!(!edge.shows(&mut client, EAST, COMMON).await);

        edge.tell(EAST, snapshot(COMMON, 1));
        assert!(edge.shows(&mut client, EAST, COMMON).await);
        // And nothing more is asked of anyone.
        edge.settle(WEST).await;
        for region in REGIONS {
            let asked = edge.asked(region).await;
            assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
        }
        edge.end().await;
    }

    /// The edge as a guest at the east for `COMMON`, on a link the east has just been
    /// said hello on, with its player handed to the east since: `Subscribe` 1 is out
    /// while the region still holds back everything behind the hello.
    async fn handed_into_a_hold() -> (Harness, Client) {
        let mut edge = Harness::start().await;
        let client = edge.joined(player(1), EntityId(5)).await;
        let ask = edge.ask(WEST, COMMON);
        edge.tell(WEST, elsewhere(COMMON, ask, EAST));
        edge.settle(WEST).await;
        assert_eq!(edge.asked(EAST).await, [as_guest(1, [COMMON])]);

        let hello = edge.relink(EAST).await;
        assert!(hello.chunks.is_empty(), "{hello:?}");
        assert_eq!(hello.guests, [COMMON]);
        edge.resume(EAST);
        edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
        edge.quiet().await;
        assert_eq!(edge.at(EAST).subscriptions[&COMMON], (Role::Viewer, 1));
        (edge, client)
    }

    /// Scenario 5. During a resume's hold a region answers the hello's chunks before it
    /// looks at anything behind the hello, so its answers are numbered 0 whatever the
    /// edge has sent meanwhile.
    #[tokio::test]
    async fn a_snapshot_numbered_0_is_shown_although_subscribe_1_was_sent_meanwhile() {
        let (mut edge, mut client) = handed_into_a_hold().await;
        assert!(!edge.shows(&mut client, EAST, COMMON).await);
        edge.tell(EAST, snapshot(COMMON, 0));
        assert!(edge.shows(&mut client, EAST, COMMON).await);
        edge.settle(WEST).await;
        for region in REGIONS {
            let asked = edge.asked(region).await;
            assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
        }
        edge.end().await;
    }

    /// Scenario 5, the other answer a guest gets. The east said `NotMine` to the
    /// hello's guest before it read `Subscribe` 1, which then made a viewer's
    /// subscription of its own there. The `NotMine` is about what the edge has changed
    /// since, and the answer to the viewer's subscription is still to come.
    #[tokio::test]
    async fn a_not_mine_numbered_0_is_passed_over_when_subscribe_1_was_sent_meanwhile() {
        let (mut edge, mut client) = handed_into_a_hold().await;
        edge.tell(EAST, not_mine(COMMON, 0));
        edge.settle(EAST).await;
        edge.settle(WEST).await;
        for region in REGIONS {
            let asked = edge.asked(region).await;
            assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
        }
        edge.tell(EAST, snapshot(COMMON, 1));
        assert!(edge.shows(&mut client, EAST, COMMON).await);
        edge.end().await;
    }

    /// Scenario 6: the edge is a guest at the east for `FAR`, which the west named, and
    /// its one player walks a chunk east, out of sight of it.
    async fn a_chunk_nobody_sees_any_more_is_ended_everywhere(served: bool) {
        let mut edge = Harness::start().await;
        let _client = edge.joined(player(1), EntityId(5)).await;
        let ask = edge.ask(WEST, FAR);
        edge.tell(WEST, elsewhere(FAR, ask, EAST));
        edge.settle(WEST).await;
        assert_eq!(edge.asked(EAST).await, [as_guest(1, [FAR])]);
        if served {
            edge.tell(EAST, snapshot(FAR, 1));
        }
        edge.quiet().await;

        edge.tell(WEST, walked(EntityId(5), HOME, STEP_EAST));
        edge.settle(WEST).await;
        assert_eq!(edge.asked(EAST).await, [unsubscribe(2, [FAR])]);
        let asked = edge.asked(WEST).await;
        let expected = BTreeSet::from([
            (None, minus(&view(HOME), &view(STEP_EAST))),
            (Some(Role::Viewer), minus(&view(STEP_EAST), &view(HOME))),
        ]);
        assert_eq!(meanings(&asked), expected);
        assert_eq!(asked.len(), 2, "{asked:?}");
        assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(STEP_EAST));
        assert!(edge.at(EAST).subscriptions.is_empty());
        edge.end().await;
    }

    /// Scenario 6.
    #[tokio::test]
    async fn a_chunk_that_leaves_the_last_view_is_ended_at_the_region_that_was_to_serve_it() {
        a_chunk_nobody_sees_any_more_is_ended_everywhere(false).await;
    }

    /// Scenario 6.
    #[tokio::test]
    async fn a_chunk_that_leaves_the_last_view_is_ended_at_the_region_that_served_it() {
        a_chunk_nobody_sees_any_more_is_ended_everywhere(true).await;
    }

    /// Scenario 6, the other way round: a guest's subscription is ended only when
    /// nobody sees the chunk. Here a player of a third region still does.
    #[tokio::test]
    async fn a_chunk_that_leaves_one_view_stays_subscribed_where_another_regions_player_sees_it() {
        let mut edge = Harness::start().await;
        let _first = edge.joined(player(1), EntityId(5)).await;
        let _second = edge.joined(player(2), EntityId(6)).await;
        edge.hand(player(2), EntityId(6), WEST, NORTH, NORTHERN)
            .await;
        edge.quiet().await;
        // Both regions name the east, which serves the chunk.
        edge.tell(WEST, elsewhere(FAR, edge.ask(WEST, FAR), EAST));
        edge.tell(NORTH, elsewhere(FAR, edge.ask(NORTH, FAR), EAST));
        edge.settle(WEST).await;
        edge.settle(NORTH).await;
        assert_eq!(edge.asked(EAST).await, [as_guest(1, [FAR])]);
        edge.quiet().await;

        // The first walks out of sight of it. Their region's subscription carried
        // nothing and is ended; the guest's at the east is what the second sees.
        edge.tell(WEST, walked(EntityId(5), HOME, STEP_EAST));
        edge.settle(WEST).await;
        let asked = edge.asked(EAST).await;
        assert!(asked.is_empty(), "{asked:?}");
        assert!(!edge.at(WEST).subscriptions.contains_key(&FAR));
        assert_eq!(edge.at(NORTH).subscriptions[&FAR].0, Role::Viewer);
        assert_eq!(edge.at(EAST).chunks(Role::Guest), set(&[FAR]));
        edge.end().await;
    }

    /// Scenario 7. Two players of two regions see one chunk, which the first one's
    /// region serves; the second one's region says so. When the first player goes,
    /// their region's subscriptions become guest's for what the second still sees,
    /// whatever they wait for, and are ended for the rest.
    #[tokio::test]
    async fn what_another_regions_player_still_sees_stays_subscribed_as_a_guests_and_shown() {
        let mut edge = Harness::start().await;
        let mut first = edge.joined(player(1), EntityId(5)).await;
        let mut second = edge.joined(player(2), EntityId(6)).await;
        edge.hand(player(2), EntityId(6), WEST, EAST, EASTERN).await;
        edge.quiet().await;
        edge.tell(WEST, snapshot(COMMON, edge.ask(WEST, COMMON)));
        edge.tell(EAST, elsewhere(COMMON, edge.ask(EAST, COMMON), WEST));
        edge.settle(EAST).await;
        // The edge is subscribed at the west already.
        let asked = edge.asked(WEST).await;
        assert!(asked.is_empty(), "{asked:?}");
        assert!(edge.shows(&mut first, WEST, COMMON).await);
        assert!(edge.shows(&mut second, WEST, COMMON).await);
        let sent = second.times_sent(COMMON);
        edge.quiet().await;

        edge.leave(&first).await;
        let asked = edge.asked(WEST).await;
        let still_seen = both(&view(HOME), &view(EASTERN));
        let expected = BTreeSet::from([
            (Some(Role::Guest), still_seen.clone()),
            (None, minus(&view(HOME), &view(EASTERN))),
        ]);
        assert_eq!(meanings(&asked), expected);
        assert_eq!(edge.at(WEST).chunks(Role::Guest), still_seen);
        assert!(edge.at(WEST).chunks(Role::Viewer).is_empty());
        // The east was a guest's region too, for what the second player saw when they
        // were handed over and the first went on seeing. Nobody sees that now.
        let asked = edge.asked(EAST).await;
        let unseen = BTreeSet::from([(None, minus(&view(HOME), &view(EASTERN)))]);
        assert_eq!(meanings(&asked), unseen);
        assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(EASTERN));
        assert!(edge.at(EAST).chunks(Role::Guest).is_empty());

        // The chunk stays on the second player's screen as it is, and what happens
        // in it is still shown.
        edge.sync(&mut second).await;
        assert!(second.chunks.contains_key(&COMMON));
        assert_eq!(second.times_sent(COMMON), sent);
        edge.tell(WEST, changed(COMMON, STONE));
        edge.settle(WEST).await;
        edge.sync(&mut second).await;
        assert_eq!(second.state(COMMON), Some(STONE));
        edge.end().await;
    }

    /// Scenario 8: the same while the first region has no link. Its subscriptions wait
    /// then, whatever they were, and are not ended because they wait: the hello of the
    /// next link names what the second player still sees among the guests'.
    async fn the_last_player_of_a_region_without_a_link_goes(served: bool) {
        let mut edge = Harness::start().await;
        let first = edge.joined(player(1), EntityId(5)).await;
        let mut second = edge.joined(player(2), EntityId(6)).await;
        edge.hand(player(2), EntityId(6), WEST, EAST, EASTERN).await;
        edge.quiet().await;
        edge.tell(EAST, elsewhere(COMMON, edge.ask(EAST, COMMON), WEST));
        if served {
            edge.tell(WEST, snapshot(COMMON, edge.ask(WEST, COMMON)));
        }
        edge.quiet().await;
        edge.sync(&mut second).await;
        assert_eq!(second.chunks.contains_key(&COMMON), served);

        edge.lose(WEST).await;
        edge.leave(&first).await;
        // Of what the east was asked for, nobody sees now what only the first saw.
        let asked = edge.asked(EAST).await;
        let unseen = BTreeSet::from([(None, minus(&view(HOME), &view(EASTERN)))]);
        assert_eq!(meanings(&asked), unseen);
        let hello = edge.relink(WEST).await;
        assert!(hello.players.is_empty(), "{hello:?}");
        assert!(hello.chunks.is_empty(), "{hello:?}");
        assert_eq!(set(&hello.guests), both(&view(HOME), &view(EASTERN)));
        // What was on the second player's screen has stayed there.
        edge.sync(&mut second).await;
        assert_eq!(second.chunks.contains_key(&COMMON), served);

        edge.resume(WEST);
        let chunk = chunk_with(COMMON, STONE);
        edge.tell(WEST, snapshot_of(COMMON, 0, chunk, Vec::new()));
        edge.settle(WEST).await;
        edge.sync(&mut second).await;
        assert_eq!(second.state(COMMON), Some(STONE));
        let asked = edge.asked(EAST).await;
        assert!(asked.is_empty(), "{asked:?}");
        edge.end().await;
    }

    /// Scenario 8, with a chunk the first region had not served yet.
    #[tokio::test]
    async fn a_chunk_waited_for_at_a_region_without_a_link_is_a_guests_in_its_next_hello() {
        the_last_player_of_a_region_without_a_link_goes(false).await;
    }

    /// Scenario 8, with a chunk the first region had served: the snapshot on the new
    /// link is reconciled with what is shown.
    #[tokio::test]
    async fn a_chunk_served_by_a_region_without_a_link_is_a_guests_in_its_next_hello() {
        the_last_player_of_a_region_without_a_link_goes(true).await;
    }

    /// The edge with one player in the west, who sees `COMMON`. The west has named the
    /// east for it twice and the east has said `NotMine` twice, the second time less
    /// than a second after the west was asked again: another asking is due.
    async fn asked_again_and_due() -> (Harness, Client) {
        let mut edge = Harness::start().await;
        let client = edge.joined(player(1), EntityId(5)).await;
        edge.quiet().await;
        edge.tell(WEST, elsewhere(COMMON, 1, EAST));
        edge.settle(WEST).await;
        assert_eq!(edge.asked(EAST).await, [as_guest(1, [COMMON])]);

        // The east does not hold it: the west is asked again, under a new number.
        edge.tell(EAST, not_mine(COMMON, 1));
        edge.settle(EAST).await;
        assert_eq!(edge.asked(WEST).await, [subscribe(2, [COMMON])]);
        assert!(edge.at(EAST).subscriptions.is_empty());

        // The west names the east once more, and the east says the same again.
        edge.tell(WEST, elsewhere(COMMON, 2, EAST));
        edge.settle(WEST).await;
        assert_eq!(edge.asked(EAST).await, [as_guest(2, [COMMON])]);
        edge.tell(EAST, not_mine(COMMON, 2));
        edge.settle(EAST).await;
        let asked = edge.asked(WEST).await;
        assert!(asked.is_empty(), "asked again within a second: {asked:?}");
        (edge, client)
    }

    /// Scenario 9. The clock of this test stands still until the test moves it.
    #[tokio::test(start_paused = true)]
    async fn a_guest_told_not_mine_has_the_viewers_region_asked_again_at_most_once_a_second() {
        let (mut edge, mut client) = asked_again_and_due().await;
        tokio::time::advance(Duration::from_millis(1100)).await;
        assert_eq!(edge.next_asked(WEST).await, subscribe(3, [COMMON]));
        // The asking is one like any other: its answer is taken.
        edge.tell(WEST, snapshot(COMMON, 3));
        assert!(edge.shows(&mut client, WEST, COMMON).await);
        edge.end().await;
    }

    /// Scenario 9. An asking that is due is dropped with its subscription.
    #[tokio::test(start_paused = true)]
    async fn a_subscription_that_ends_while_an_asking_is_due_is_not_asked_again() {
        let (mut edge, _client) = asked_again_and_due().await;
        // The player walks two chunks south, out of sight of the chunk.
        let (south, further) = (ChunkPos::new(0, 1), ChunkPos::new(0, 2));
        edge.tell(WEST, walked(EntityId(5), HOME, south));
        edge.tell(WEST, walked(EntityId(5), south, further));
        edge.quiet().await;
        assert!(!edge.at(WEST).subscriptions.contains_key(&COMMON));
        for _ in 0..3 {
            tokio::time::advance(Duration::from_millis(1100)).await;
            for region in REGIONS {
                let asked = edge.asked(region).await;
                assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
            }
        }
        edge.end().await;
    }

    /// Scenario 9. An asking that is due is dropped when the link is replaced: the
    /// hello asks.
    #[tokio::test(start_paused = true)]
    async fn a_hello_takes_the_place_of_an_asking_that_was_due() {
        let (mut edge, _client) = asked_again_and_due().await;
        let hello = edge.relink(WEST).await;
        assert_eq!(set(&hello.chunks), view(HOME));
        edge.resume(WEST);
        edge.tell(WEST, present(player(1), EntityId(5)));
        for _ in 0..3 {
            tokio::time::advance(Duration::from_millis(1100)).await;
            for region in REGIONS {
                let asked = edge.asked(region).await;
                assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
            }
        }
        // The answer to the hello is taken like the answer to an asking.
        edge.tell(WEST, elsewhere(COMMON, 0, EAST));
        edge.settle(WEST).await;
        assert_eq!(edge.asked(EAST).await, [as_guest(3, [COMMON])]);
        edge.end().await;
    }

    /// Scenario 10. An `Elsewhere` is taken only with the number of the edge's last
    /// message about the chunk: an older one is about an asking the edge has made
    /// again since.
    #[tokio::test]
    async fn an_elsewhere_with_an_old_number_changes_nothing() {
        let mut edge = Harness::start().await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        edge.quiet().await;
        edge.tell(WEST, elsewhere(COMMON, 1, EAST));
        edge.settle(WEST).await;
        assert_eq!(edge.asked(EAST).await, [as_guest(1, [COMMON])]);
        edge.tell(EAST, not_mine(COMMON, 1));
        edge.settle(EAST).await;
        assert_eq!(edge.asked(WEST).await, [subscribe(2, [COMMON])]);

        // Two answers to the first asking, read late.
        edge.tell(WEST, elsewhere(COMMON, 1, EAST));
        edge.tell(WEST, elsewhere(COMMON, 1, NORTH));
        edge.settle(WEST).await;
        for region in REGIONS {
            let asked = edge.asked(region).await;
            assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
        }
        // The subscription waits as before: the answer to the second asking is taken.
        edge.tell(WEST, snapshot(COMMON, 2));
        assert!(edge.shows(&mut client, WEST, COMMON).await);
        edge.end().await;
    }

    /// Scenario 10. `NotMine` answers a guest's subscription; for one the edge holds as
    /// a viewer's it changes nothing.
    #[tokio::test]
    async fn a_not_mine_for_a_viewers_subscription_changes_nothing() {
        let mut edge = Harness::start().await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        edge.quiet().await;
        edge.tell(WEST, not_mine(COMMON, 1));
        for region in REGIONS {
            let asked = edge.asked(region).await;
            assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
        }
        edge.tell(WEST, snapshot(COMMON, 1));
        assert!(edge.shows(&mut client, WEST, COMMON).await);
        edge.end().await;
    }

    /// Scenario 10. `Elsewhere` answers a viewer's subscription; for one the edge
    /// holds as a guest's it changes nothing, and the region it names is not asked.
    #[tokio::test]
    async fn an_elsewhere_for_a_guests_subscription_changes_nothing() {
        let mut edge = Harness::start().await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        edge.quiet().await;
        edge.tell(WEST, elsewhere(COMMON, 1, EAST));
        edge.settle(WEST).await;
        assert_eq!(edge.asked(EAST).await, [as_guest(1, [COMMON])]);

        edge.tell(EAST, elsewhere(COMMON, 1, NORTH));
        edge.settle(EAST).await;
        for region in REGIONS {
            let asked = edge.asked(region).await;
            assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
        }
        edge.tell(EAST, snapshot(COMMON, 1));
        assert!(edge.shows(&mut client, EAST, COMMON).await);
        edge.end().await;
    }

    /// Scenario 10. A region that names itself as the holder elsewhere has made a
    /// mistake, which is passed over.
    #[tokio::test]
    async fn an_elsewhere_that_names_the_region_that_says_it_changes_nothing() {
        let mut edge = Harness::start().await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        edge.quiet().await;
        edge.tell(WEST, elsewhere(COMMON, 1, WEST));
        for region in REGIONS {
            let asked = edge.asked(region).await;
            assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
        }
        assert_eq!(edge.at(WEST).subscriptions[&COMMON], (Role::Viewer, 1));
        edge.tell(WEST, snapshot(COMMON, 1));
        assert!(edge.shows(&mut client, WEST, COMMON).await);
        edge.end().await;
    }

    /// Scenario 10. A `NotMine` is taken only with the number of the edge's last
    /// message about the chunk. Here the east said it to the first asking as a guest;
    /// since then a player of the east has come and gone, so the east has been asked
    /// as a viewer and as a guest again, and has yet to answer that.
    #[tokio::test]
    async fn a_not_mine_with_an_old_number_does_not_end_a_guests_subscription() {
        let mut edge = Harness::start().await;
        let mut first = edge.joined(player(1), EntityId(5)).await;
        edge.quiet().await;
        edge.tell(WEST, elsewhere(COMMON, 1, EAST));
        edge.settle(WEST).await;
        assert_eq!(edge.asked(EAST).await, [as_guest(1, [COMMON])]);

        let second = edge.joined(player(2), EntityId(6)).await;
        edge.hand(player(2), EntityId(6), WEST, EAST, EASTERN).await;
        edge.quiet().await;
        assert_eq!(edge.at(EAST).subscriptions[&COMMON].0, Role::Viewer);
        edge.leave(&second).await;
        edge.quiet().await;
        let (role, last) = edge.at(EAST).subscriptions[&COMMON];
        assert_eq!(role, Role::Guest);

        edge.tell(EAST, not_mine(COMMON, 1));
        edge.settle(EAST).await;
        for region in REGIONS {
            let asked = edge.asked(region).await;
            assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
        }
        edge.tell(EAST, snapshot(COMMON, last));
        assert!(edge.shows(&mut first, EAST, COMMON).await);
        edge.end().await;
    }

    /// Scenario 11: a player in the west and one in the east. The west has named the
    /// east for `told`, which both see, and serves `lent` to the east's player as a
    /// guest. Then the west's link is replaced, after it ended or while it stands.
    async fn a_hello_names_the_subscriptions_by_kind(ended: bool) {
        let told = ChunkPos::new(3, 0);
        let lent = ChunkPos::new(5, 0);
        let mut edge = Harness::start().await;
        let mut first = edge.joined(player(1), EntityId(5)).await;
        let mut second = edge.joined(player(2), EntityId(6)).await;
        edge.hand(player(2), EntityId(6), WEST, EAST, EASTERN).await;
        edge.quiet().await;
        edge.tell(WEST, elsewhere(told, edge.ask(WEST, told), EAST));
        edge.tell(WEST, snapshot(HOME, edge.ask(WEST, HOME)));
        edge.tell(EAST, elsewhere(lent, edge.ask(EAST, lent), WEST));
        edge.settle(EAST).await;
        assert_eq!(edge.asked(WEST).await, [as_guest(2, [lent])]);
        edge.tell(WEST, snapshot(lent, 2));
        edge.quiet().await;
        assert!(edge.shows(&mut first, WEST, HOME).await);
        assert!(edge.shows(&mut second, WEST, lent).await);
        let asked = edge.asked(EAST).await;
        assert!(asked.is_empty(), "{asked:?}");

        if ended {
            edge.lose(WEST).await;
        }
        let hello = edge.relink(WEST).await;
        assert_eq!(hello.players, [player(1)]);
        // The viewer's subscriptions, the one told elsewhere among them, and the
        // guest's.
        assert_eq!(set(&hello.chunks), view(HOME));
        assert_eq!(hello.chunks.len(), view(HOME).len());
        assert_eq!(hello.guests, [lent]);
        // Nothing leaves a screen because a link was replaced.
        edge.sync(&mut first).await;
        edge.sync(&mut second).await;
        assert!(first.chunks.contains_key(&HOME));
        assert!(second.chunks.contains_key(&lent));

        // The answers to the hello are numbered 0.
        edge.resume(WEST);
        edge.tell(WEST, present(player(1), EntityId(5)));
        let chunk = chunk_with(HOME, STONE);
        edge.tell(WEST, snapshot_of(HOME, 0, chunk, Vec::new()));
        let chunk = chunk_with(lent, GRANITE);
        edge.tell(WEST, snapshot_of(lent, 0, chunk, Vec::new()));
        edge.tell(WEST, elsewhere(told, 0, EAST));
        edge.settle(WEST).await;
        edge.sync(&mut first).await;
        edge.sync(&mut second).await;
        assert_eq!(first.state(HOME), Some(STONE));
        assert_eq!(second.state(lent), Some(GRANITE));
        // The east is subscribed to what the west names it for.
        for region in REGIONS {
            let asked = edge.asked(region).await;
            assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
        }

        // The numbers of the new link begin at 1.
        edge.tell(WEST, walked(EntityId(5), HOME, STEP_EAST));
        let asked = edge.asked(WEST).await;
        let numbers: Vec<_> = asked
            .iter()
            .map(|body| match body {
                EdgeToWorker::Subscribe { ask, .. }
                | EdgeToWorker::SubscribeAsGuest { ask, .. }
                | EdgeToWorker::Unsubscribe { ask, .. } => *ask,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(numbers, [1, 2], "{asked:?}");
        edge.end().await;
    }

    /// Scenario 11.
    #[tokio::test]
    async fn the_hello_on_a_link_that_replaces_one_that_stands_names_the_subscriptions_by_kind() {
        a_hello_names_the_subscriptions_by_kind(false).await;
    }

    /// Scenario 11.
    #[tokio::test]
    async fn the_hello_on_a_link_after_one_that_ended_names_the_subscriptions_by_kind() {
        a_hello_names_the_subscriptions_by_kind(true).await;
    }

    /// An edge whose first player is the west's and whose second is the east's, the
    /// west serving both of them `COMMON`. Then the west answers a hello as one that
    /// has forgotten the edge.
    async fn forgotten_by_the_west() -> (Harness, Client, Client) {
        let mut edge = Harness::start().await;
        let mut first = edge.joined(player(1), EntityId(5)).await;
        let mut second = edge.joined(player(2), EntityId(6)).await;
        edge.hand(player(2), EntityId(6), WEST, EAST, EASTERN).await;
        edge.quiet().await;
        edge.tell(WEST, snapshot(COMMON, edge.ask(WEST, COMMON)));
        edge.tell(EAST, elsewhere(COMMON, edge.ask(EAST, COMMON), WEST));
        edge.quiet().await;
        assert!(edge.shows(&mut second, WEST, COMMON).await);
        edge.quiet().await;

        let hello = edge.relink(WEST).await;
        assert_eq!(hello.players, [player(1)]);
        assert_eq!(hello.since, 1);
        let unknown = Welcome::Unknown {
            since: 2,
            entries: 0,
            presences: 1,
            applied: 0,
        };
        edge.tell(WEST, WorkerToEdge::Welcome(unknown));
        first.disconnected().await;
        // The region's outbox begins anew with the edge it did not know.
        edge.outbox[WEST.0 as usize] = 0;
        (edge, first, second)
    }

    /// Scenario 12. The players of a region that has forgotten the edge are gone, and
    /// their chunks are given up as for anyone who goes: what a player of another
    /// region still sees stays subscribed, as a guest's.
    #[tokio::test]
    async fn a_region_that_forgot_the_edge_keeps_as_guests_what_other_regions_players_see() {
        let (mut edge, _first, mut second) = forgotten_by_the_west().await;
        let said = edge.said(WEST).await;
        let left = EdgeToWorker::PlayerLeave {
            player: player(1),
            entity: Some(EntityId(5)),
        };
        assert_eq!(numbered(&said), [(1, left)]);
        assert!(edge.at(WEST).chunks(Role::Viewer).is_empty());
        let still_seen = both(&view(HOME), &view(EASTERN));
        assert_eq!(edge.at(WEST).chunks(Role::Guest), still_seen);
        // The east loses only what nobody sees any more: its player sees what they saw.
        let asked = edge.asked(EAST).await;
        let unseen = BTreeSet::from([(None, minus(&view(HOME), &view(EASTERN)))]);
        assert_eq!(meanings(&asked), unseen);
        assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(EASTERN));
        edge.sync(&mut second).await;
        assert!(second.chunks.contains_key(&COMMON));

        // The edge says the region's new word for their numbering from now on.
        let hello = edge.relink(WEST).await;
        assert_eq!((hello.since, hello.seen), (2, edge.outbox[0]));
        assert!(hello.players.is_empty() && hello.chunks.is_empty());
        assert_eq!(set(&hello.guests), still_seen);
        edge.resume(WEST);
        edge.end().await;
    }

    /// Scenario 12, and what a region that has forgotten the edge still serves it.
    /// The replica has the west's snapshot of the chunk and the edge is still
    /// subscribed there, so the west is the region that serves the chunk, as after any
    /// link that ended. (The records do not say this of `Unknown` in so many words.)
    #[tokio::test]
    async fn a_region_that_forgot_the_edge_still_serves_the_chunks_it_served() {
        let (mut edge, _first, mut second) = forgotten_by_the_west().await;
        edge.quiet().await;
        let action = breaking(player(2), edge.sequence(), COMMON);
        edge.say(EAST, remote(&action, None));
        // Behind the leaving of the player who is gone, numbered from 1 again.
        let passed_on = EdgeToWorker::Remote(action);
        assert_eq!(edge.next_numbered(WEST).await, (2, passed_on));
        assert!(second.connected());
        edge.end().await;
    }

    /// Scenario 13. A player walks east until the next chunk is one their region says
    /// the east holds, and is let go into it. On the link to the east the viewer's
    /// subscriptions come before the arrival, so that one claim covers the chunk the
    /// player arrives in and their view. The west goes on being asked, as a guest, for
    /// what the player still sees of it, and for nothing else.
    #[tokio::test]
    async fn a_player_let_go_is_asked_for_at_the_new_region_before_they_arrive_there() {
        let mut edge = Harness::start().await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        let mut at = HOME;
        for x in 1..=3 {
            let next = ChunkPos::new(x, 0);
            edge.tell(WEST, walked(EntityId(5), at, next));
            at = next;
        }
        edge.quiet().await;
        let old = view(at);
        assert_eq!(edge.at(WEST).chunks(Role::Viewer), old);
        // Of what the player sees, the west says that the east holds the chunks from
        // x = 4 on, has served some of its own and has yet to serve the others.
        let theirs: BTreeSet<_> = old.iter().copied().filter(|chunk| chunk.x >= 4).collect();
        for chunk in &old {
            let ask = edge.ask(WEST, *chunk);
            if theirs.contains(chunk) {
                edge.tell(WEST, elsewhere(*chunk, ask, EAST));
            } else if chunk.x >= 2 {
                edge.tell(WEST, snapshot(*chunk, ask));
            }
        }
        edge.quiet().await;
        assert_eq!(edge.at(EAST).chunks(Role::Guest), theirs);

        edge.say(WEST, departed(player(1), EntityId(5), EAST, EASTERN));
        edge.settle(WEST).await;
        let said = edge.said(EAST).await;
        let arrival = said
            .iter()
            .position(|message| matches!(message.body, EdgeToWorker::PlayerArrive { .. }))
            .expect("the player is passed on");
        let mut before = BTreeSet::new();
        for message in &said[..arrival] {
            if let EdgeToWorker::Subscribe { chunks, .. } = &message.body {
                before.extend(chunks.iter().copied());
            }
        }
        assert!(before.is_superset(&old), "{before:?}");
        assert!(before.contains(&EASTERN));
        let passed_on = EdgeToWorker::PlayerArrive {
            player: player(1),
            transfer: transfer(EntityId(5), 0, EASTERN),
        };
        assert_eq!(numbered(&said), [(1, passed_on)]);
        assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(EASTERN));
        assert!(edge.at(EAST).chunks(Role::Guest).is_empty());

        let asked = edge.asked(WEST).await;
        let still_seen: BTreeSet<_> = both(&old, &view(EASTERN))
            .into_iter()
            .filter(|chunk| !theirs.contains(chunk))
            .collect();
        assert!(edge.at(WEST).chunks(Role::Viewer).is_empty());
        assert_eq!(edge.at(WEST).chunks(Role::Guest), still_seen);
        // A subscription that was told elsewhere carried nothing: it is ended, and
        // never made a guest's.
        for body in &asked {
            if let EdgeToWorker::SubscribeAsGuest { chunks, .. } = body {
                assert!(
                    chunks.iter().all(|chunk| !theirs.contains(chunk)),
                    "{body:?}"
                );
            }
        }
        assert!(client.connected());
        edge.end().await;
    }

    /// Scenario 14. The west lets its player go while it has no link, and the edge
    /// reads that among the entries of the next welcome. A player of the east sees
    /// chunks the east says the west holds. The west's subscriptions wait, as on every
    /// new link, and those the east's players see must not be ended because they wait.
    #[tokio::test]
    async fn a_hand_over_read_from_a_welcomes_entries_leaves_what_is_seen_subscribed() {
        let mut edge = Harness::start().await;
        let mut first = edge.joined(player(1), EntityId(5)).await;
        let mut second = edge.joined(player(2), EntityId(6)).await;
        edge.hand(player(2), EntityId(6), WEST, EAST, EASTERN).await;
        edge.quiet().await;
        edge.tell(EAST, elsewhere(COMMON, edge.ask(EAST, COMMON), WEST));
        edge.quiet().await;

        edge.lose(WEST).await;
        let hello = edge.relink(WEST).await;
        assert_eq!(hello.players, [player(1)]);
        assert_eq!(set(&hello.chunks), view(HOME));
        assert!(hello.guests.is_empty(), "{hello:?}");
        edge.tell(
            WEST,
            WorkerToEdge::Welcome(Welcome::Resumed {
                entries: 1,
                presences: 1,
                applied: 0,
            }),
        );
        edge.say(WEST, departed(player(1), EntityId(5), EAST, EASTERN));
        edge.tell(
            WEST,
            WorkerToEdge::Presence {
                player: player(1),
                answer: Presence::Absent,
            },
        );
        edge.quiet().await;
        assert!(first.connected());

        // Everything anyone sees is asked of their region, and the west is still
        // asked for what it holds of it.
        let seen = view(EASTERN);
        assert_eq!(edge.at(EAST).chunks(Role::Viewer), seen);
        assert!(edge.at(EAST).chunks(Role::Guest).is_empty());
        assert!(edge.at(WEST).chunks(Role::Viewer).is_empty());
        assert_eq!(edge.at(WEST).chunks(Role::Guest), both(&view(HOME), &seen));
        // Its answer to the hello is shown to both.
        edge.tell(WEST, snapshot(COMMON, 0));
        assert!(edge.shows(&mut first, WEST, COMMON).await);
        assert!(edge.shows(&mut second, WEST, COMMON).await);
        edge.end().await;
    }

    /// A hand-over into a region that has no link: nothing is sent into nothing. The
    /// hello of its next link names the player and their view, and the arrival that
    /// was kept follows the welcome.
    #[tokio::test]
    async fn a_player_let_go_to_a_region_without_a_link_is_asked_for_in_its_next_hello() {
        let mut edge = Harness::start().await;
        let mut first = edge.joined(player(1), EntityId(5)).await;
        let _second = edge.joined(player(2), EntityId(6)).await;
        edge.quiet().await;
        edge.lose(EAST).await;
        edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
        // The second player sees all that the first saw from where they were.
        let asked = edge.asked(WEST).await;
        assert!(asked.is_empty(), "{asked:?}");
        assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));

        // The east is the first player's region now. It was asked for what they saw
        // when they were let go; what they see no more and the second still does it
        // is asked for as a guest, by the letter of section 2.
        let hello = edge.relink(EAST).await;
        assert_eq!(hello.players, [player(1)]);
        assert_eq!(set(&hello.chunks), view(EASTERN));
        assert_eq!(set(&hello.guests), minus(&view(HOME), &view(EASTERN)));
        edge.resume(EAST);
        // The east does not have the player yet, who is on their way to it.
        edge.tell(
            EAST,
            WorkerToEdge::Presence {
                player: player(1),
                answer: Presence::Absent,
            },
        );
        let arrival = EdgeToWorker::PlayerArrive {
            player: player(1),
            transfer: transfer(EntityId(5), 0, EASTERN),
        };
        assert_eq!(edge.next_numbered(EAST).await, (1, arrival));
        edge.tell(EAST, snapshot(EASTERN, 0));
        assert!(edge.shows(&mut first, EAST, EASTERN).await);
        assert!(first.connected());
        edge.end().await;
    }

    /// Scenario 15. `NotMine` for an arrival is a `Departed` to the holder it names:
    /// the view is asked of the holder before the arrival, the region that said it
    /// stays a guest's for what the player sees, and the holder can be the region the
    /// player came from.
    #[tokio::test]
    async fn a_player_a_region_sends_on_goes_to_the_holder_it_names_with_their_view() {
        let mut edge = Harness::start().await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
        edge.quiet().await;

        edge.say(EAST, sent_on(player(1), EntityId(5), NORTH, EASTERN));
        edge.settle(EAST).await;
        let (number, before) = edge.arrived(NORTH).await;
        assert_eq!(number, 1);
        assert!(before.is_superset(&view(EASTERN)), "{before:?}");
        assert_eq!(edge.at(NORTH).chunks(Role::Viewer), view(EASTERN));
        edge.quiet().await;
        // The east waits for all of it, and the player still sees it.
        assert!(edge.at(EAST).chunks(Role::Viewer).is_empty());
        assert_eq!(edge.at(EAST).chunks(Role::Guest), view(EASTERN));

        // The north sends them back to where they came from two steps ago.
        edge.say(NORTH, sent_on(player(1), EntityId(5), WEST, EASTERN));
        edge.settle(NORTH).await;
        let (number, before) = edge.arrived(WEST).await;
        // Behind the join.
        assert_eq!(number, 2);
        assert!(before.is_superset(&view(EASTERN)), "{before:?}");
        edge.quiet().await;
        assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(EASTERN));
        assert!(edge.at(NORTH).chunks(Role::Viewer).is_empty());
        assert!(client.connected());
        // What the player does goes to the west from now on.
        edge.input(&client, step_into(EASTERN)).await;
        let (number, body) = edge.next_numbered(WEST).await;
        assert!(
            number == 3 && matches!(body, EdgeToWorker::Input { .. }),
            "{body:?}"
        );
        edge.end().await;
    }

    /// Scenario 15. A player is passed on by `NotMine` at most as often as the edge
    /// has heard of regions. (Read as: that many times they are passed on, and the
    /// next `NotMine` disconnects them.)
    #[tokio::test]
    async fn a_player_sent_on_more_often_than_there_are_regions_is_disconnected() {
        let mut edge = Harness::start().await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
        edge.quiet().await;
        for (from, to) in [(EAST, NORTH), (NORTH, WEST), (WEST, EAST)] {
            edge.say(from, sent_on(player(1), EntityId(5), to, EASTERN));
            edge.settle(from).await;
            edge.arrived(to).await;
            assert!(client.connected(), "after {from:?} sent them on");
        }
        edge.say(EAST, sent_on(player(1), EntityId(5), NORTH, EASTERN));
        client.disconnected().await;
        let reason = client.disconnected.clone().unwrap_or_default();
        assert!(
            reason.contains("The server lost track of where you are."),
            "{reason}"
        );
        edge.quiet().await;
        for region in REGIONS {
            let left = &edge.at(region).subscriptions;
            assert!(left.is_empty(), "{region:?} is still asked for {left:?}");
        }
        edge.end().await;
    }

    /// Scenario 15. The count begins anew when a region confirms an input of the
    /// player: they have been somewhere since.
    #[tokio::test]
    async fn a_player_whose_input_a_region_confirmed_can_be_sent_on_as_often_again() {
        let mut edge = Harness::start().await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        edge.input(&client, step_into(HOME)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 2);
        edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
        edge.quiet().await;
        for (from, to) in [(EAST, NORTH), (NORTH, WEST), (WEST, EAST)] {
            edge.say(from, sent_on(player(1), EntityId(5), to, EASTERN));
            edge.settle(from).await;
            edge.arrived(to).await;
        }
        // The east takes them in this time and applies what they did.
        edge.tell(
            EAST,
            WorkerToEdge::Progress {
                applied: 4,
                inputs: vec![(player(1), 1)],
            },
        );
        edge.quiet().await;
        let mut gone = transfer(EntityId(5), 1, EASTERN);
        gone.pose = Pose::at(within(EASTERN));
        for (from, to) in [(EAST, NORTH), (NORTH, WEST), (WEST, EAST)] {
            let entry = Durable::NotMine {
                what: Misdirected::Arrival {
                    player: player(1),
                    transfer: gone.clone(),
                },
                holder: to,
            };
            edge.say(from, entry);
            edge.settle(from).await;
            edge.arrived(to).await;
            assert!(client.connected(), "after {from:?} sent them on");
        }
        edge.end().await;
    }

    /// Scenario 15, and section 4, step 1: a region that names itself as where a
    /// player goes has made a mistake, and the player is disconnected.
    #[tokio::test]
    async fn a_player_let_go_to_the_region_that_says_so_is_disconnected() {
        let mut edge = Harness::start().await;
        let mut first = edge.joined(player(1), EntityId(5)).await;
        let mut second = edge.joined(player(2), EntityId(6)).await;
        edge.hand(player(2), EntityId(6), WEST, EAST, EASTERN).await;
        edge.quiet().await;
        edge.say(WEST, departed(player(1), EntityId(5), WEST, STEP_EAST));
        first.disconnected().await;
        edge.say(EAST, sent_on(player(2), EntityId(6), EAST, EASTERN));
        second.disconnected().await;
        edge.end().await;
    }

    /// Scenario 16. The edge no longer has the player a region lets go: the entity
    /// that is on its way is discarded where it was sent, and nothing else is done.
    #[tokio::test]
    async fn a_player_who_left_before_their_region_let_them_go_is_discarded_where_they_were_sent() {
        let mut edge = Harness::start().await;
        let client = edge.joined(player(1), EntityId(5)).await;
        edge.leave(&client).await;
        edge.quiet().await;
        edge.say(WEST, departed(player(1), EntityId(5), EAST, EASTERN));
        edge.settle(WEST).await;
        let discard = EdgeToWorker::Discard {
            entity: EntityId(5),
            chunk: EASTERN,
        };
        let said = edge.said(EAST).await;
        assert_eq!(
            said,
            [EdgeMessage {
                number: Some(1),
                body: discard
            }]
        );
        edge.quiet().await;
        for region in REGIONS {
            let left = &edge.at(region).subscriptions;
            assert!(left.is_empty(), "{region:?} is asked for {left:?}");
        }
        edge.end().await;
    }

    /// Scenario 16: a player leaves and joins again, and then their region lets go who
    /// they were before. The one who joined again is the home region's, whether or not
    /// that region has placed them yet.
    async fn a_region_lets_go_who_a_player_was_before(placed: bool) {
        let mut edge = Harness::start().await;
        let before = edge.joined(player(1), EntityId(5)).await;
        edge.leave(&before).await;
        let mut again = edge.join(player(1)).await;
        edge.drained().await;
        if placed {
            edge.tell(WEST, spawned(player(1), EntityId(6)));
        }
        edge.quiet().await;

        edge.say(WEST, departed(player(1), EntityId(5), EAST, EASTERN));
        edge.settle(WEST).await;
        let discard = EdgeToWorker::Discard {
            entity: EntityId(5),
            chunk: EASTERN,
        };
        let said = edge.said(EAST).await;
        assert_eq!(
            said,
            [EdgeMessage {
                number: Some(1),
                body: discard
            }]
        );
        if !placed {
            edge.tell(WEST, spawned(player(1), EntityId(6)));
        }
        edge.quiet().await;
        assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));
        assert!(edge.at(EAST).subscriptions.is_empty());
        edge.input(&again, step_into(HOME)).await;
        let (_, body) = edge.next_numbered(WEST).await;
        assert!(
            matches!(&body, EdgeToWorker::Input { player: who, .. } if *who == player(1)),
            "{body:?}"
        );
        assert!(again.connected());
        edge.nothing_numbered().await;
        edge.end().await;
    }

    /// Scenario 16.
    #[tokio::test]
    async fn a_departure_of_who_a_player_was_before_joining_again_is_discarded() {
        a_region_lets_go_who_a_player_was_before(false).await;
    }

    /// Scenario 16.
    #[tokio::test]
    async fn a_departure_of_who_a_player_was_before_being_placed_again_is_discarded() {
        a_region_lets_go_who_a_player_was_before(true).await;
    }

    /// Scenario 16: a player is handed to the east and leaves, or leaves and joins
    /// again, and then the east sends on who they were to the north.
    async fn a_region_sends_on_who_a_player_was_before(again: bool) {
        let mut edge = Harness::start().await;
        let before = edge.joined(player(1), EntityId(5)).await;
        edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
        edge.quiet().await;
        edge.leave(&before).await;
        let mut now = None;
        if again {
            now = Some(edge.join(player(1)).await);
            edge.drained().await;
            edge.tell(WEST, spawned(player(1), EntityId(6)));
        }
        edge.quiet().await;

        edge.say(EAST, sent_on(player(1), EntityId(5), NORTH, EASTERN));
        edge.settle(EAST).await;
        let discard = EdgeToWorker::Discard {
            entity: EntityId(5),
            chunk: EASTERN,
        };
        let said = edge.said(NORTH).await;
        assert_eq!(
            said,
            [EdgeMessage {
                number: Some(1),
                body: discard
            }]
        );
        edge.quiet().await;
        assert!(edge.at(EAST).subscriptions.is_empty());
        assert!(edge.at(NORTH).subscriptions.is_empty());
        match &mut now {
            Some(client) => {
                assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));
                edge.input(client, step_into(HOME)).await;
                let (_, body) = edge.next_numbered(WEST).await;
                assert!(matches!(body, EdgeToWorker::Input { .. }), "{body:?}");
                assert!(client.connected());
                edge.nothing_numbered().await;
            }
            None => assert!(edge.at(WEST).subscriptions.is_empty()),
        }
        edge.end().await;
    }

    /// Scenario 16.
    #[tokio::test]
    async fn an_arrival_sent_on_for_a_player_who_left_is_discarded_at_the_holder() {
        a_region_sends_on_who_a_player_was_before(false).await;
    }

    /// Scenario 16.
    #[tokio::test]
    async fn an_arrival_sent_on_for_who_a_player_was_before_joining_again_is_discarded() {
        a_region_sends_on_who_a_player_was_before(true).await;
    }

    /// Scenario 16, where the region alone does not tell who is meant. A player is let
    /// go to the east and leaves; they join again, are placed anew and are let go to
    /// the east once more. Then the east's word on the arrival of who they were
    /// before is read: it names their region, and is not about them.
    #[tokio::test]
    async fn an_arrival_sent_on_for_who_a_player_was_before_does_not_move_who_they_are_now() {
        let mut edge = Harness::start().await;
        let before = edge.joined(player(1), EntityId(5)).await;
        edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
        edge.quiet().await;
        edge.leave(&before).await;
        edge.quiet().await;
        let mut now = edge.joined(player(1), EntityId(6)).await;
        edge.hand(player(1), EntityId(6), WEST, EAST, EASTERN).await;
        edge.quiet().await;
        assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(EASTERN));

        edge.say(EAST, sent_on(player(1), EntityId(5), NORTH, EASTERN));
        edge.settle(EAST).await;
        let discard = EdgeToWorker::Discard {
            entity: EntityId(5),
            chunk: EASTERN,
        };
        let said = edge.said(NORTH).await;
        assert_eq!(
            said,
            [EdgeMessage {
                number: Some(1),
                body: discard
            }]
        );
        edge.quiet().await;
        assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(EASTERN));
        assert!(edge.at(NORTH).subscriptions.is_empty());
        edge.input(&now, step_into(EASTERN)).await;
        let (_, body) = edge.next_numbered(EAST).await;
        assert!(matches!(body, EdgeToWorker::Input { .. }), "{body:?}");
        assert!(now.connected());
        edge.end().await;
    }

    /// Section 4, step 2. A player is handed east and back. A second word of the east
    /// that it lets them go is about their stay there, which is over.
    #[tokio::test]
    async fn an_entry_about_an_earlier_stay_of_a_player_is_passed_over() {
        let mut edge = Harness::start().await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
        edge.hand(player(1), EntityId(5), EAST, WEST, HOME).await;
        edge.quiet().await;
        assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));

        edge.say(EAST, departed(player(1), EntityId(5), NORTH, NORTHERN));
        edge.settle(EAST).await;
        let said = edge.said(NORTH).await;
        assert!(said.is_empty(), "{said:?}");
        edge.quiet().await;
        assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));
        edge.input(&client, step_into(HOME)).await;
        let (_, body) = edge.next_numbered(WEST).await;
        assert!(matches!(body, EdgeToWorker::Input { .. }), "{body:?}");
        assert!(client.connected());
        edge.end().await;
    }

    /// A player is handed east and back without anything being answered in between.
    /// By the letter of section 2 the east is left a guest's region for what the
    /// player still sees, whatever it waits for, until it says `NotMine`; then nothing
    /// is asked again, as the west was not told elsewhere.
    #[tokio::test]
    async fn a_player_handed_over_and_back_leaves_the_other_region_a_guests_until_it_says_not_mine()
    {
        let mut edge = Harness::start().await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
        edge.hand(player(1), EntityId(5), EAST, WEST, HOME).await;
        edge.quiet().await;
        assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));
        assert!(edge.at(WEST).chunks(Role::Guest).is_empty());
        let still_seen = both(&view(HOME), &view(EASTERN));
        assert!(edge.at(EAST).chunks(Role::Viewer).is_empty());
        assert_eq!(edge.at(EAST).chunks(Role::Guest), still_seen);

        for chunk in &still_seen {
            edge.tell(EAST, not_mine(*chunk, edge.ask(EAST, *chunk)));
        }
        edge.settle(EAST).await;
        for region in REGIONS {
            let asked = edge.asked(region).await;
            assert!(asked.is_empty(), "{region:?} was asked {asked:?}");
        }
        edge.tell(WEST, snapshot(COMMON, edge.ask(WEST, COMMON)));
        assert!(edge.shows(&mut client, WEST, COMMON).await);
        edge.end().await;
    }

    /// Two players who see the same chunks are handed east one after the other and
    /// back again. After each step every region is asked as a viewer for exactly what
    /// its players see, and as a guest for what section 2 leaves it.
    #[tokio::test]
    async fn two_players_handed_over_and_back_leave_each_region_what_its_players_see() {
        let mut edge = Harness::start().await;
        let _first = edge.joined(player(1), EntityId(5)).await;
        let _second = edge.joined(player(2), EntityId(6)).await;
        let (home, east) = (view(HOME), view(EASTERN));
        let nothing = BTreeSet::new();
        // What each region is asked for as a viewer and as a guest: the west, then
        // the east.
        let steps = [
            // The first goes east. Of what they saw, the second still sees what the
            // first sees no more: the east waits for that as a guest.
            (1, EntityId(5), WEST, EAST, EASTERN),
            (2, EntityId(6), WEST, EAST, EASTERN),
            (1, EntityId(5), EAST, WEST, HOME),
            (2, EntityId(6), EAST, WEST, HOME),
        ];
        let expected = [
            [&home, &nothing, &east, &minus(&home, &east)],
            [&nothing, &both(&home, &east), &east, &nothing],
            [&home, &minus(&east, &home), &east, &nothing],
            [&home, &nothing, &nothing, &both(&home, &east)],
        ];
        for ((who, entity, from, to, chunk), expected) in steps.into_iter().zip(expected) {
            edge.hand(player(who), entity, from, to, chunk).await;
            edge.quiet().await;
            let found = [
                edge.at(WEST).chunks(Role::Viewer),
                edge.at(WEST).chunks(Role::Guest),
                edge.at(EAST).chunks(Role::Viewer),
                edge.at(EAST).chunks(Role::Guest),
            ];
            for (index, (found, expected)) in found.iter().zip(expected).enumerate() {
                assert_eq!(
                    found, expected,
                    "after player {who} went to {to:?}: {index}"
                );
            }
        }
        edge.end().await;
    }

    /// Scenario 17. What is left of an action goes to the region its entry names;
    /// where it names none, to the region that serves the edge the chunk, also while
    /// that region has no link: it is kept for it then.
    #[tokio::test]
    async fn an_action_goes_to_the_region_named_or_to_the_one_that_serves_the_chunk() {
        let mut edge = Harness::start().await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        edge.quiet().await;
        let first = breaking(player(1), edge.sequence(), COMMON);
        edge.say(WEST, remote(&first, Some(NORTH)));
        let passed_on = EdgeToWorker::Remote(first.clone());
        assert_eq!(edge.next_numbered(NORTH).await, (1, passed_on));

        // The east serves the chunk.
        edge.tell(WEST, elsewhere(COMMON, 1, EAST));
        edge.settle(WEST).await;
        edge.tell(EAST, snapshot(COMMON, 1));
        edge.quiet().await;
        let second = breaking(player(1), edge.sequence(), COMMON);
        edge.say(WEST, remote(&second, None));
        let passed_on = EdgeToWorker::Remote(second.clone());
        assert_eq!(edge.next_numbered(EAST).await, (1, passed_on.clone()));

        // It still does for this purpose when its link has ended.
        edge.lose(EAST).await;
        let third = breaking(player(1), edge.sequence(), COMMON);
        edge.say(WEST, remote(&third, None));
        edge.settle(WEST).await;
        edge.nothing_numbered().await;
        let hello = edge.relink(EAST).await;
        assert_eq!(hello.guests, [COMMON]);
        edge.resume(EAST);
        // What the east has not reported applied is sent again, and then what waited.
        assert_eq!(edge.next_numbered(EAST).await, (1, passed_on));
        let passed_on = EdgeToWorker::Remote(third.clone());
        assert_eq!(edge.next_numbered(EAST).await, (2, passed_on));
        // None of them has ended, so none is acknowledged to the player.
        client.drain();
        for action in [first, second, third] {
            assert!(!client.acknowledged.contains(&action.sequence));
        }
        edge.end().await;
    }

    /// Scenario 17. An action whose entry names no region ends at the edge when no
    /// region serves the chunk or only the region the entry came from does, and so
    /// does one that a region passes on to itself: the player is told it was handled
    /// and sees the block as it is.
    #[tokio::test]
    async fn an_action_nobody_else_can_take_is_acknowledged_to_the_player() {
        let mut edge = Harness::start().await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        edge.quiet().await;
        // Nobody serves the chunk.
        let action = breaking(player(1), edge.sequence(), COMMON);
        edge.say(WEST, remote(&action, None));
        edge.acknowledged(&mut client, action.sequence).await;
        // The region the entry comes from serves it.
        edge.tell(WEST, snapshot(COMMON, 1));
        let action = breaking(player(1), edge.sequence(), COMMON);
        edge.say(WEST, remote(&action, None));
        edge.acknowledged(&mut client, action.sequence).await;
        // The entry names the region it comes from.
        let action = breaking(player(1), edge.sequence(), COMMON);
        edge.say(WEST, remote(&action, Some(WEST)));
        edge.acknowledged(&mut client, action.sequence).await;
        let action = breaking(player(1), edge.sequence(), COMMON);
        let to_itself = Durable::NotMine {
            what: Misdirected::Remote(action.clone()),
            holder: WEST,
        };
        edge.say(WEST, to_itself);
        edge.acknowledged(&mut client, action.sequence).await;
        edge.nothing_numbered().await;
        edge.end().await;
    }

    /// Scenario 17. A region that was passed an action and believes another to hold
    /// the chunk names it, and the action goes there.
    #[tokio::test]
    async fn an_action_a_region_says_is_not_its_own_goes_to_the_holder_it_names() {
        let mut edge = Harness::start().await;
        let _client = edge.joined(player(1), EntityId(5)).await;
        edge.quiet().await;
        let action = breaking(player(1), edge.sequence(), COMMON);
        let not_mine = Durable::NotMine {
            what: Misdirected::Remote(action.clone()),
            holder: EAST,
        };
        edge.say(NORTH, not_mine);
        let passed_on = EdgeToWorker::Remote(action);
        assert_eq!(edge.next_numbered(EAST).await, (1, passed_on));
        edge.end().await;
    }

    /// Scenario 17. The edge keeps for a region it has heard of and has no link to
    /// yet: an action that is to go there, and a guest's subscription it was named
    /// for, which the hello of its first link names.
    #[tokio::test]
    async fn a_region_the_edge_has_no_link_to_yet_is_kept_for() {
        let patience = Duration::from_secs(1_000_000);
        let mut edge = Harness::start_with(&[WEST, EAST], patience).await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        edge.quiet().await;
        let action = breaking(player(1), edge.sequence(), COMMON);
        edge.say(WEST, remote(&action, Some(NORTH)));
        edge.tell(WEST, elsewhere(COMMON, 1, NORTH));
        edge.quiet().await;

        edge.epochs[NORTH.0 as usize] = 0;
        let hello = edge.relink(NORTH).await;
        assert_eq!((hello.since, hello.seen), (0, 0));
        assert!(hello.players.is_empty() && hello.chunks.is_empty());
        assert_eq!(hello.guests, [COMMON]);
        let unknown = Welcome::Unknown {
            since: 1,
            entries: 0,
            presences: 0,
            applied: 0,
        };
        edge.tell(NORTH, WorkerToEdge::Welcome(unknown));
        let passed_on = EdgeToWorker::Remote(action.clone());
        assert_eq!(edge.next_numbered(NORTH).await, (1, passed_on));
        let done = Durable::RemoteDone {
            player: player(1),
            sequence: action.sequence,
        };
        edge.say(NORTH, done);
        edge.tell(NORTH, snapshot(COMMON, 0));
        assert!(edge.shows(&mut client, NORTH, COMMON).await);
        edge.end().await;
    }

    /// A player is let go to a region the edge has heard of and has no link to yet.
    /// The edge keeps their arrival and their view for it, as for a region whose link
    /// ended, and the hello of its first link names both.
    #[tokio::test]
    async fn a_player_let_go_to_a_region_without_a_link_yet_is_named_in_its_first_hello() {
        let patience = Duration::from_secs(1_000_000);
        let mut edge = Harness::start_with(&[WEST, EAST], patience).await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        edge.hand(player(1), EntityId(5), WEST, NORTH, NORTHERN)
            .await;
        edge.quiet().await;
        // The west is asked as a guest for what the player still sees, which is all
        // it waits for.
        assert!(edge.at(WEST).chunks(Role::Viewer).is_empty());
        assert_eq!(
            edge.at(WEST).chunks(Role::Guest),
            both(&view(HOME), &view(NORTHERN))
        );

        edge.epochs[NORTH.0 as usize] = 0;
        let hello = edge.relink(NORTH).await;
        assert_eq!(hello.players, [player(1)]);
        assert_eq!(set(&hello.chunks), view(NORTHERN));
        assert!(hello.guests.is_empty(), "{hello:?}");
        let unknown = Welcome::Unknown {
            since: 1,
            entries: 0,
            presences: 1,
            applied: 0,
        };
        edge.tell(NORTH, WorkerToEdge::Welcome(unknown));
        let arrival = EdgeToWorker::PlayerArrive {
            player: player(1),
            transfer: transfer(EntityId(5), 0, NORTHERN),
        };
        assert_eq!(edge.next_numbered(NORTH).await, (1, arrival));
        edge.tell(NORTH, snapshot(NORTHERN, 0));
        assert!(edge.shows(&mut client, NORTH, NORTHERN).await);
        assert!(client.connected());
        edge.end().await;
    }

    /// Scenario 17, rule 26. An entry without a region for an action of a player the
    /// edge no longer has is dropped, also when a region serves the chunk.
    #[tokio::test]
    async fn an_action_without_a_region_of_a_player_who_left_is_dropped() {
        let mut edge = Harness::start().await;
        let first = edge.joined(player(1), EntityId(5)).await;
        let _second = edge.joined(player(2), EntityId(6)).await;
        edge.quiet().await;
        edge.tell(WEST, elsewhere(COMMON, 1, EAST));
        edge.settle(WEST).await;
        edge.tell(EAST, snapshot(COMMON, 1));
        edge.quiet().await;

        edge.leave(&first).await;
        edge.quiet().await;
        let action = breaking(player(1), edge.sequence(), COMMON);
        edge.say(WEST, remote(&action, None));
        edge.settle(WEST).await;
        edge.nothing_numbered().await;
        edge.end().await;
    }

    /// Which region serves a chunk, after `Elsewhere`: a region that says `Elsewhere`
    /// for a chunk it had sent a snapshot of (which step C3 makes possible) serves it
    /// no more, and the region it names does once its snapshot is taken.
    #[tokio::test]
    async fn a_region_that_says_elsewhere_for_a_chunk_it_served_serves_it_no_more() {
        let mut edge = Harness::start().await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        edge.quiet().await;
        edge.tell(WEST, snapshot(COMMON, 1));
        edge.tell(WEST, elsewhere(COMMON, 1, EAST));
        edge.settle(WEST).await;
        assert_eq!(edge.asked(EAST).await, [as_guest(1, [COMMON])]);

        // A third region passes an action on without knowing who holds the chunk.
        let action = breaking(player(1), edge.sequence(), COMMON);
        edge.say(NORTH, remote(&action, None));
        edge.acknowledged(&mut client, action.sequence).await;
        edge.nothing_numbered().await;
        // The chunk stays on the screen meanwhile, and the next snapshot is
        // reconciled with it.
        edge.sync(&mut client).await;
        assert_eq!(client.state(COMMON), Some(AIR));

        let chunk = chunk_with(COMMON, STONE);
        edge.tell(EAST, snapshot_of(COMMON, 1, chunk, Vec::new()));
        edge.settle(EAST).await;
        edge.sync(&mut client).await;
        assert_eq!(client.state(COMMON), Some(STONE));
        let action = breaking(player(1), edge.sequence(), COMMON);
        edge.say(NORTH, remote(&action, None));
        let passed_on = EdgeToWorker::Remote(action);
        assert_eq!(edge.next_numbered(EAST).await, (1, passed_on));
        edge.end().await;
    }

    /// Which region serves a chunk, after `NotMine`: a guest's region that says it for
    /// a chunk it had sent a snapshot of serves it no more.
    #[tokio::test]
    async fn a_region_that_says_not_mine_for_a_chunk_it_served_serves_it_no_more() {
        let mut edge = Harness::start().await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        edge.quiet().await;
        edge.tell(WEST, elsewhere(COMMON, 1, EAST));
        edge.settle(WEST).await;
        assert_eq!(edge.asked(EAST).await, [as_guest(1, [COMMON])]);
        edge.tell(EAST, snapshot(COMMON, 1));
        assert!(edge.shows(&mut client, EAST, COMMON).await);

        edge.tell(EAST, not_mine(COMMON, 1));
        edge.settle(EAST).await;
        assert_eq!(edge.asked(WEST).await, [subscribe(2, [COMMON])]);
        let action = breaking(player(1), edge.sequence(), COMMON);
        edge.say(WEST, remote(&action, None));
        edge.acknowledged(&mut client, action.sequence).await;
        edge.nothing_numbered().await;
        // The chunk stays on the screen while the west is asked again.
        edge.sync(&mut client).await;
        assert!(client.chunks.contains_key(&COMMON));
        edge.end().await;
    }

    /// Which region serves a chunk, after its subscription ended: a chunk that nobody
    /// saw for a moment is served by nobody until a snapshot of it is taken again.
    #[tokio::test]
    async fn a_chunk_that_was_out_of_sight_is_served_by_nobody_until_its_next_snapshot() {
        let mut edge = Harness::start().await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        edge.quiet().await;
        edge.tell(WEST, snapshot(FAR, 1));
        assert!(edge.shows(&mut client, WEST, FAR).await);
        edge.tell(WEST, walked(EntityId(5), HOME, STEP_EAST));
        edge.tell(WEST, walked(EntityId(5), STEP_EAST, HOME));
        edge.quiet().await;

        let action = breaking(player(1), edge.sequence(), FAR);
        edge.say(NORTH, remote(&action, None));
        edge.acknowledged(&mut client, action.sequence).await;
        edge.nothing_numbered().await;

        edge.tell(WEST, snapshot(FAR, edge.ask(WEST, FAR)));
        edge.settle(WEST).await;
        let action = breaking(player(1), edge.sequence(), FAR);
        edge.say(NORTH, remote(&action, None));
        // Behind the join.
        let passed_on = EdgeToWorker::Remote(action);
        assert_eq!(edge.next_numbered(WEST).await, (2, passed_on));
        edge.end().await;
    }

    /// Scenario 18. What was kept for a region goes out when as many `Outbox` messages
    /// as its welcome announced have come, also when one of them is an entry the edge
    /// has handled before, and not earlier.
    #[tokio::test]
    async fn what_was_kept_is_sent_after_as_many_entries_as_the_welcome_announced() {
        let mut edge = Harness::start().await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        edge.input(&client, step_into(HOME)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 2);
        edge.quiet().await;
        let seen = edge.outbox[0];

        let hello = edge.relink(WEST).await;
        assert_eq!(hello.seen, seen);
        edge.tell(
            WEST,
            WorkerToEdge::Welcome(Welcome::Resumed {
                entries: 2,
                presences: 1,
                applied: 0,
            }),
        );
        // The first of the two is one the edge has handled.
        let nobody = Durable::RemoteDone {
            player: player(u128::MAX),
            sequence: 0,
        };
        let again = WorkerToEdge::Outbox {
            number: seen,
            entry: nobody,
        };
        edge.tell(WEST, again);
        // Something behind it that the edge passes on to the player shows that the
        // edge has read it. Nothing that was kept has been sent by then.
        let sequence = edge.sequence();
        let acknowledged = WorkerToEdge::ToPlayer {
            player: player(1),
            event: PlayerEvent::Acknowledged { sequence },
        };
        edge.tell(WEST, acknowledged);
        edge.acknowledged(&mut client, sequence).await;
        let sent = numbered(&edge.waiting(WEST));
        assert!(sent.is_empty(), "{sent:?}");

        // With the second entry, everything that was kept follows in order.
        edge.settle(WEST).await;
        let sent = numbered(&edge.waiting(WEST));
        assert!(
            matches!(
                &sent[..],
                [
                    (1, EdgeToWorker::PlayerJoin(_)),
                    (2, EdgeToWorker::Input { .. })
                ]
            ),
            "{sent:?}"
        );
        edge.end().await;
    }

    /// Scenario 18. An entry among those a welcome announces can change what is kept:
    /// here the player is let go, so what they did and the west has not applied goes
    /// to the east behind their arrival. The west is sent what was kept for it when
    /// the entry has been handled.
    #[tokio::test]
    async fn a_hand_over_among_a_welcomes_entries_is_done_before_what_was_kept_is_sent() {
        let mut edge = Harness::start().await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        edge.input(&client, step_into(HOME)).await;
        assert_eq!(edge.next_numbered(WEST).await.0, 2);
        edge.quiet().await;

        let hello = edge.relink(WEST).await;
        assert_eq!(hello.players, [player(1)]);
        edge.tell(
            WEST,
            WorkerToEdge::Welcome(Welcome::Resumed {
                entries: 1,
                presences: 1,
                applied: 0,
            }),
        );
        edge.say(WEST, departed(player(1), EntityId(5), EAST, EASTERN));
        edge.tell(
            WEST,
            WorkerToEdge::Presence {
                player: player(1),
                answer: Presence::Absent,
            },
        );
        edge.settle(WEST).await;
        let sent = numbered(&edge.said(EAST).await);
        assert!(
            matches!(
                &sent[..],
                [
                    (1, EdgeToWorker::PlayerArrive { .. }),
                    (2, EdgeToWorker::Input { number: 1, .. })
                ]
            ),
            "{sent:?}"
        );
        let sent = numbered(&edge.said(WEST).await);
        assert!(
            matches!(
                &sent[..],
                [
                    (1, EdgeToWorker::PlayerJoin(_)),
                    (2, EdgeToWorker::Input { .. })
                ]
            ),
            "{sent:?}"
        );
        assert!(client.connected());
        edge.end().await;
    }

    /// Scenario 19. A snapshot that is reconciled with a chunk the edge shows removes
    /// the entities its own region introduced and that it lacks, and leaves one that
    /// another region introduced: a player of that region who stands in a chunk this
    /// one holds. It brings the blocks that differ.
    #[tokio::test]
    async fn a_reconciled_snapshot_removes_what_its_region_introduced_and_leaves_the_rest() {
        // A chunk next to `COMMON` that the player sees as well.
        let beside = ChunkPos::new(3, -2);
        let mut edge = Harness::start().await;
        let mut client = edge.joined(player(1), EntityId(5)).await;
        edge.quiet().await;
        // The west serves `COMMON`, with someone in it.
        let there = vec![stranger(EntityId(70), COMMON)];
        edge.tell(WEST, snapshot_of(COMMON, 1, empty_chunk(), there));
        // The east serves the chunk beside it, with someone who then walks over.
        edge.tell(WEST, elsewhere(beside, 1, EAST));
        edge.settle(WEST).await;
        assert_eq!(edge.asked(EAST).await, [as_guest(1, [beside])]);
        let there = vec![stranger(EntityId(80), beside)];
        edge.tell(EAST, snapshot_of(beside, 1, empty_chunk(), there));
        let walked_over = RegionEvent::EntityMoved {
            entity: EntityId(80),
            pose: Pose::at(within(COMMON)),
            previous_chunk: beside,
        };
        edge.tell(
            EAST,
            WorkerToEdge::TickDelta {
                tick: 2,
                events: vec![walked_over],
            },
        );
        edge.settle(EAST).await;
        edge.sync(&mut client).await;
        assert_eq!(client.entities, BTreeSet::from([70, 80]));

        // The west's next snapshot of its chunk has neither of them.
        let chunk = chunk_with(COMMON, STONE);
        edge.tell(WEST, snapshot_of(COMMON, 1, chunk, Vec::new()));
        edge.settle(WEST).await;
        edge.sync(&mut client).await;
        assert_eq!(client.entities, BTreeSet::from([80]));
        assert_eq!(client.state(COMMON), Some(STONE));
        edge.end().await;
    }

    /// Scenario 19, rule 32. A snapshot never removes the entity of one of the edge's
    /// own players.
    #[tokio::test]
    async fn a_reconciled_snapshot_does_not_remove_the_edges_own_players() {
        let mut edge = Harness::start().await;
        let mut first = edge.joined(player(1), EntityId(5)).await;
        let mut second = edge.joined(player(2), EntityId(6)).await;
        edge.quiet().await;
        let own = |number: u128, entity| EntityState {
            entity,
            kind: EntityKind::Player {
                player: player(number),
                name: format!("Player{number}"),
            },
            pose: Pose::at(SPAWN),
        };
        let there = vec![own(1, EntityId(5)), own(2, EntityId(6))];
        edge.tell(WEST, snapshot_of(HOME, 1, empty_chunk(), there));
        edge.settle(WEST).await;
        edge.sync(&mut first).await;
        edge.sync(&mut second).await;
        assert!(first.entities.contains(&6), "{:?}", first.entities);
        assert!(second.entities.contains(&5), "{:?}", second.entities);

        edge.tell(WEST, snapshot(HOME, 1));
        edge.settle(WEST).await;
        edge.sync(&mut first).await;
        edge.sync(&mut second).await;
        assert!(first.entities.contains(&6), "{:?}", first.entities);
        assert!(second.entities.contains(&5), "{:?}", second.entities);
        edge.end().await;
    }

    /// Statement E, where section 2 as first written broke it. Two regions' players
    /// see a chunk a third holds. The north names the east for it, as it heard at a
    /// time when that was so; the east is asked for it already, for its own player,
    /// and names the west. Then the east's player goes. Its subscription was told
    /// elsewhere, so it is ended as one that carried nothing. But it was what the
    /// north's subscription pointed at: left as it is, the north's would be told
    /// elsewhere with a region where nothing is asked, and nothing would ever have
    /// the north asked again. Statement E says that there is a subscription at the
    /// east, or an asking due; section 2 now has the north asked again.
    #[tokio::test]
    async fn statement_e_holds_when_a_subscription_another_points_at_is_ended() {
        let mut edge = Harness::start().await;
        let first = edge.joined(player(1), EntityId(5)).await;
        let mut second = edge.joined(player(2), EntityId(6)).await;
        edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
        edge.hand(player(2), EntityId(6), WEST, NORTH, NORTHERN)
            .await;
        edge.quiet().await;
        // The west holds the chunk and is asked for it as a guest, as both players
        // saw it when the west let them go.
        assert_eq!(edge.at(WEST).subscriptions[&COMMON].0, Role::Guest);
        edge.tell(WEST, snapshot(COMMON, edge.ask(WEST, COMMON)));
        edge.tell(EAST, elsewhere(COMMON, edge.ask(EAST, COMMON), WEST));
        edge.settle(EAST).await;
        edge.tell(NORTH, elsewhere(COMMON, edge.ask(NORTH, COMMON), EAST));
        edge.quiet().await;
        assert_eq!(edge.at(EAST).subscriptions[&COMMON].0, Role::Viewer);
        assert!(edge.shows(&mut second, NORTH, COMMON).await);

        edge.leave(&first).await;
        edge.quiet().await;
        let at_the_east = edge.at(EAST).subscriptions.contains_key(&COMMON);
        let asked_again = edge.ask(NORTH, COMMON) > 1;
        assert!(
            at_the_east || asked_again,
            "the north named the east for the chunk, where nothing is asked now"
        );
        edge.end().await;
    }

    /// A player is let go to the east, which has no link, and leaves. When the east
    /// has a link again it takes them in, as the arrival comes first, and lets them
    /// go to the west, as what they did last took them there; only then it hears that
    /// they left, which it passes over. The edge has the west discard the entity. The
    /// west reports it removed to those watching the chunk, and it has to go from
    /// their screens although it was the east that showed it last: ADR-0008 says of an
    /// entity that departed that one "that never arrived anywhere is" taken off the
    /// screens, and ADR-0006 has the region it was heading for report it removed.
    #[tokio::test]
    async fn a_discarded_entity_another_region_showed_last_is_taken_off_the_screens() {
        let near = ChunkPos::new(3, 0);
        let mut edge = Harness::start().await;
        // Someone who stands a chunk east of where players enter the world, and sees
        // a chunk of the west and one the east serves.
        let mut watcher = edge.joined(player(2), EntityId(6)).await;
        edge.tell(WEST, walked(EntityId(6), HOME, STEP_EAST));
        edge.quiet().await;
        edge.tell(WEST, snapshot(near, edge.ask(WEST, near)));
        edge.tell(WEST, elsewhere(EASTERN, edge.ask(WEST, EASTERN), EAST));
        edge.settle(WEST).await;
        edge.settle(EAST).await;
        edge.tell(EAST, snapshot(EASTERN, edge.ask(EAST, EASTERN)));
        edge.quiet().await;

        let leaver = edge.joined(player(1), EntityId(5)).await;
        edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
        edge.leave(&leaver).await;
        edge.quiet().await;

        // The east takes the arrival in, and reports the entity as any that appears.
        let arrived = EntityState {
            entity: EntityId(5),
            kind: EntityKind::Player {
                player: player(1),
                name: "Player".to_owned(),
            },
            pose: Pose::at(within(EASTERN)),
        };
        edge.tell(
            EAST,
            WorkerToEdge::TickDelta {
                tick: 2,
                events: vec![RegionEvent::EntitySpawned(arrived)],
            },
        );
        // They step west and are let go there.
        edge.tell(EAST, walked(EntityId(5), EASTERN, near));
        edge.say(EAST, departed(player(1), EntityId(5), WEST, near));
        edge.settle(EAST).await;
        let said = numbered(&edge.said(WEST).await);
        let discard = EdgeToWorker::Discard {
            entity: EntityId(5),
            chunk: near,
        };
        assert_eq!(
            said.last().map(|(_, body)| body),
            Some(&discard),
            "{said:?}"
        );

        let removed = RegionEvent::EntityRemoved {
            entity: EntityId(5),
            chunk: near,
        };
        edge.tell(
            WEST,
            WorkerToEdge::TickDelta {
                tick: 3,
                events: vec![removed],
            },
        );
        edge.settle(WEST).await;
        edge.sync(&mut watcher).await;
        assert!(
            !watcher.entities.contains(&5),
            "the entity of a player who left is still shown"
        );
        edge.end().await;
    }

    // Scenario 20: generated runs.
    //
    // A run is a sequence of small steps, each chosen by a generator of numbers that
    // starts from the run's seed: a player joins, walks a chunk or leaves; a region
    // looks at some of what the edge has sent it and makes its answers; one message
    // of a region is read by the edge; a link ends or is replaced; a region gives a
    // chunk back, comes to believe something that is no longer so, or forgets the
    // edge; time passes. The regions are played by a model of what a region may send
    // by section 5 of ADR-0012, with a world store that says who holds a chunk: the
    // west, the east and the north are pinned to their parts of the world, and the
    // chunks between them are held by whoever was granted them first. What a region
    // has made reaches the edge later, in the order of its link, so answers come for
    // subscriptions the edge has changed since.
    //
    // Beside the edge runs what ADR-0013 says an edge holds: the players with their
    // views, and a subscription per region and chunk, changed by `want` and `unwant`
    // and by the answers as sections 2 to 6 have it. It takes only the numbers from
    // what the edge sends. After every step:
    //
    // - the subscriptions the edge's messages make at each region it has a link to
    //   are the ones this account has, of the same kind, and a hello names exactly
    //   the account's subscriptions of a region without a link;
    // - statements V, G and E hold for the account, so that a region is asked as a
    //   viewer exactly for what its players see and for nothing nobody sees;
    // - the subscription messages of a link ascend, and so do its numbered messages
    //   and each player's inputs, without a gap;
    // - a client was sent a chunk only if a snapshot of it was taken, has every chunk
    //   it sees of which one was taken, and shows it as that snapshot and what the
    //   region said since have it.
    //
    // Every now and then, and at the end, everything comes to rest. Then everybody is
    // in the region the edge has them in, at the place their last step took them to,
    // and is shown every chunk they see as its holder has it, and the others who
    // stand there.
    //
    // Scenario 32 of ADR-0015 is the same with regions that merge and split
    // (`Run::reshaping`). A region absorbs another, or the players standing in one of
    // its chunks are split off with what is around them; both close the region's
    // links. The edge's process hands the edge what the routing table says of
    // absorbed regions at moments of the run's choosing. A region answers a hello as
    // ADR-0014, section 3.7, has it, and beside the edge runs what ADR-0015 says it
    // holds: which region stands for which, which entries came with a merge, whom a
    // welcome brought. What the edge keeps for a region a run cannot see. Where a
    // rule turns on it (a join or an arrival that is kept, a region of which the edge
    // has nothing), the run goes by the numbered messages the edge was seen to send
    // and by what the regions said they had applied.

    /// A generator of numbers that gives the same sequence for the same seed.
    struct Dice(u64);

    impl Dice {
        fn roll(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut mixed = self.0;
            mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            mixed ^ (mixed >> 31)
        }

        /// A number below `limit`.
        fn below(&mut self, limit: usize) -> usize {
            (self.roll() % limit as u64) as usize
        }

        fn chance(&mut self, percent: u64) -> bool {
            self.roll() % 100 < percent
        }
    }

    /// The region a chunk is pinned to in a generated run. The chunks between the
    /// pinned areas are held by whoever was granted them first, and given back.
    fn pinned(chunk: ChunkPos) -> Option<RegionId> {
        match chunk {
            ChunkPos { x, .. } if x <= 0 => Some(WEST),
            ChunkPos { x, z } if x >= 4 && z >= 0 => Some(EAST),
            ChunkPos { x, .. } if x >= 4 => Some(NORTH),
            _ => None,
        }
    }

    fn chunk_of(position: Vec3) -> ChunkPos {
        ChunkPos::containing(position.x, position.z)
    }

    /// What has become of a subscription: what the region has made of it, or what the
    /// edge has read of that.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Answer {
        Waiting,
        Served,
        Told(RegionId),
    }

    /// A subscription as a region has it for its link.
    #[derive(Debug)]
    struct Ticket {
        role: Role,
        ask: u64,
        answer: Answer,
    }

    /// A player in a region.
    #[derive(Debug)]
    struct Resident {
        entity: EntityId,
        chunk: ChunkPos,
        last_input: u64,
    }

    /// Something a region has made for its link and not sent yet.
    #[derive(Debug)]
    enum Made {
        Said(WorkerToEdge),
        /// An entry of its outbox, with its number.
        Entry(u64, Durable),
        /// The answer to a hello, which goes out in one piece: the welcome, the
        /// entries it announces and the presence answers.
        Welcome {
            resumed: bool,
            since: u64,
            entries: Vec<(u64, Durable)>,
            presences: Vec<WorkerToEdge>,
            /// The number of the edge's last numbered message the region had taken
            /// when it answered the hello.
            applied: u64,
        },
    }

    /// A region as a generated run plays it.
    #[derive(Debug, Default)]
    struct Played {
        /// What the edge has sent on the link that the region has not looked at.
        inbox: VecDeque<EdgeMessage>,
        /// What the region has made for the link and the edge has not been sent.
        out: VecDeque<Made>,
        tickets: BTreeMap<ChunkPos, Ticket>,
        /// The chunks its link's hello named that it has not answered yet. Until it
        /// has, it looks at nothing the edge has sent behind the hello: what the edge
        /// sends again is to act on chunks that are there, and whoever stands in a
        /// chunk is in its snapshot before anything else is said of them.
        held: BTreeSet<ChunkPos>,
        /// The number of the last subscription message it looked at.
        asked: u64,
        /// Who the store said holds chunks the region does not hold. It does not
        /// learn by itself that this has changed.
        believes: BTreeMap<ChunkPos, RegionId>,
        residents: BTreeMap<PlayerId, Resident>,
        /// The entries of its outbox that the edge has not confirmed, each with its
        /// number, and the number of the last entry it made.
        outbox: VecDeque<(u64, Durable)>,
        sent: u64,
        /// Whether the region is there: one of the three the world began with, or a
        /// part that was split off, and not absorbed since.
        alive: bool,
        /// Whether it knows the edge. One that has forgotten it makes a state for it
        /// when the edge says hello again.
        knows: bool,
        /// The number of the edge's last numbered message it took.
        received: u64,
        /// Its word for the numbering it shares with the edge.
        since: u64,
        entities: i32,
        tick: u64,
    }

    /// What one tick of a region makes for its link, in the order it is sent.
    #[derive(Default)]
    struct Ticked {
        events: Vec<RegionEvent>,
        spawned: Vec<WorkerToEdge>,
        entries: Vec<Durable>,
        departures: Vec<Durable>,
        inputs: Vec<(PlayerId, u64)>,
    }

    /// How long after a subscription was asked again the next asking comes at once.
    const ASK_AGAIN_AFTER: Duration = Duration::from_secs(1);

    /// A subscription as the edge has to hold it by ADR-0013.
    #[derive(Debug)]
    struct Kept {
        role: Role,
        answer: Answer,
        ask: u64,
        begun: u64,
        /// How many viewers of this region's players see the chunk.
        viewers: u32,
        /// The region it was told elsewhere with has said `NotMine`, and it is to be
        /// asked again with the edge's next messages.
        at_once: bool,
        /// The same, when it was asked again less than a second before: the asking
        /// is due at one of the edge's checks.
        due: bool,
        asked_again: Option<Instant>,
    }

    impl Kept {
        fn new(role: Role, viewers: u32) -> Self {
            Self {
                role,
                answer: Answer::Waiting,
                ask: 0,
                begun: 0,
                viewers,
                at_once: false,
                due: false,
                asked_again: None,
            }
        }
    }

    /// A player as the edge has to have them.
    struct Person {
        client: Client,
        region: RegionId,
        entity: Option<EntityId>,
        centre: ChunkPos,
        wanted: BTreeSet<ChunkPos>,
        /// Where the last step they made takes them, and how many steps they made.
        target: ChunkPos,
        inputs: u64,
        /// Where they are walking to.
        goal: ChunkPos,
        /// How many of the chunks their client was sent have been looked at.
        looked_at: usize,
        /// Whether their join is among what the edge keeps for the home region, and
        /// its number once the edge has been seen to send it.
        join: Option<Option<u64>>,
    }

    /// What the edge has to hold about its link to a region while it resumes on it
    /// (ADR-0015, section 1).
    #[derive(Debug, Default)]
    struct Resume {
        /// How many of the entries and of the presence answers that the welcome
        /// announced are still to come; nothing before the welcome.
        entries: Option<u32>,
        presences: Option<u32>,
        /// The players this welcome made the region's, of whom no `Present` has come.
        brought: BTreeSet<PlayerId>,
        /// The regions whose `Absorbed` the region owed when the hello was said.
        owed: BTreeSet<RegionId>,
    }

    /// Every region a test can play, in ascending order.
    fn ports() -> impl Iterator<Item = RegionId> {
        (0..PORTS).map(|index| RegionId(index as u32))
    }

    struct Run {
        seed: u64,
        dice: Dice,
        edge: Harness,
        log: Vec<String>,
        // The world as it is.
        granted: BTreeMap<ChunkPos, RegionId>,
        /// Chunks a region is giving back: the store names it for them until it has
        /// freed them.
        returning: BTreeSet<ChunkPos>,
        content: BTreeMap<ChunkPos, BlockState>,
        played: Vec<Played>,
        /// The entities that a region has let go and no region has taken in or
        /// discarded, each with the chunk it is to arrive in and the region it was
        /// let go to.
        on_the_way: BTreeMap<EntityId, (ChunkPos, RegionId)>,
        // What the edge has to hold.
        people: BTreeMap<PlayerId, Person>,
        kept: Vec<BTreeMap<ChunkPos, Kept>>,
        /// What the edge's messages on each current link make of the region's
        /// subscriptions: the kind, the number of the last message that named the
        /// chunk, and the number the subscription began with.
        heard: Vec<BTreeMap<ChunkPos, (Role, u64, u64)>>,
        /// Subscriptions the edge asked for again in the step that is being read.
        again: BTreeSet<(usize, ChunkPos)>,
        /// The chunks the replica has a snapshot of, as that snapshot and what the
        /// region said since leave them.
        replica: BTreeMap<ChunkPos, BlockState>,
        served_by: BTreeMap<ChunkPos, RegionId>,
        /// Chunks of the replica of which the records do not say what they are like
        /// until the next snapshot: a block changed in them when the edge did not
        /// hold their region to serve them.
        unsure: BTreeSet<ChunkPos>,
        /// The `since` of the last welcome the edge read from each region.
        since: [u64; PORTS],
        /// Whether regions merge and split in this run.
        reshaping: bool,
        /// The regions that were absorbed, each with the region it went into, as the
        /// routing table has them, and how many parts were split off.
        absorbed: Vec<(RegionId, RegionId)>,
        parts: usize,
        /// The last `since` a region has given out.
        sinces: u64,
        // What the edge has to hold through merges and splits (ADR-0015).
        /// The regions that are no more, as far as the edge has acted on it.
        stands_for: BTreeMap<RegionId, RegionId>,
        /// The entries of each region's outbox that came to it with a merge.
        came_with: Vec<BTreeMap<u64, (RegionId, u64)>>,
        /// The regions the table says went into each region, of which the edge has
        /// something and has handled no `Absorbed`.
        owes: Vec<BTreeSet<RegionId>>,
        /// The number of the last outbox entry the edge has seen of each region.
        seen: [u64; PORTS],
        /// How far each region has told the edge that it applied its messages, and
        /// the highest number the edge was seen to send each region.
        applied: [u64; PORTS],
        numbered: [u64; PORTS],
        /// The regions for which the edge has made a numbered message that it could
        /// not send yet. With `numbered` and `applied` this says whether anything is
        /// kept for a region, which the run cannot see for one without a link.
        unsent: [bool; PORTS],
        /// The arrivals among what the edge keeps: the region each is for, the player,
        /// and its number once the edge has been seen to send it.
        arrivals: Vec<(RegionId, PlayerId, Option<u64>)>,
        resumes: Vec<Option<Resume>>,
        /// The links the edge has to end by itself, over what the table says.
        to_end: BTreeSet<RegionId>,
        /// Clients of players the edge had to disconnect.
        dropped: Vec<Client>,
        joined: u128,
        /// How often a run came to the cases it is there for.
        tally: BTreeMap<&'static str, u64>,
    }

    impl Drop for Run {
        fn drop(&mut self) {
            if std::thread::panicking() {
                // The whole account for whoever asks for it; it is long.
                let whole = std::env::var_os("CLUSTINE_WHOLE_RUN").is_some();
                let from = if whole {
                    0
                } else {
                    self.log.len().saturating_sub(60)
                };
                eprintln!(
                    "seed {}, {} steps; the last of them:\n{}",
                    self.seed,
                    self.log.len(),
                    self.log[from..].join("\n")
                );
            }
        }
    }

    /// A message of a region in few words, for the account of a run.
    fn brief(message: &WorkerToEdge) -> String {
        match message {
            WorkerToEdge::ChunkSnapshot {
                position,
                ask,
                chunk,
                entities,
                ..
            } => {
                let state = chunk.get(3, -60, 5);
                let entities: Vec<_> = entities.iter().map(|state| state.entity.0).collect();
                format!(
                    "ChunkSnapshot ({}, {}) ask {ask} {state:?} {entities:?}",
                    position.x, position.z
                )
            }
            other => format!("{other:?}"),
        }
    }

    impl Run {
        async fn new(seed: u64, reshaping: bool) -> Self {
            // The witness lets a run wait for the edge between the entries of a
            // welcome, where `settle` cannot be used. The edge has read the regions'
            // first welcomes when the run begins.
            let edge = Harness::witnessed().await;
            let mut played = Vec::new();
            let mut since = [0; PORTS];
            let mut resumes = Vec::new();
            for (index, since) in since.iter_mut().enumerate() {
                let there = index < REGIONS.len();
                *since = u64::from(there);
                played.push(Played {
                    since: *since,
                    entities: 1000 * (index as i32 + 1),
                    // What the witness's arrangement had the region say and take.
                    sent: edge.outbox[index],
                    received: edge.heard[index].numbered,
                    alive: there,
                    knows: there,
                    ..Played::default()
                });
                resumes.push(there.then(|| Resume {
                    entries: Some(0),
                    presences: Some(0),
                    ..Resume::default()
                }));
            }
            let seen = edge.outbox;
            let mut applied = [0; PORTS];
            for (index, applied) in applied.iter_mut().enumerate() {
                *applied = edge.heard[index].numbered;
            }
            Self {
                seed,
                dice: Dice(seed),
                edge,
                log: Vec::new(),
                granted: BTreeMap::new(),
                returning: BTreeSet::new(),
                content: BTreeMap::new(),
                played,
                on_the_way: BTreeMap::new(),
                people: BTreeMap::new(),
                kept: (0..PORTS).map(|_| BTreeMap::new()).collect(),
                heard: (0..PORTS).map(|_| BTreeMap::new()).collect(),
                again: BTreeSet::new(),
                replica: BTreeMap::new(),
                served_by: BTreeMap::new(),
                unsure: BTreeSet::new(),
                since,
                reshaping,
                absorbed: Vec::new(),
                parts: 0,
                sinces: 1,
                stands_for: BTreeMap::new(),
                came_with: (0..PORTS).map(|_| BTreeMap::new()).collect(),
                owes: (0..PORTS).map(|_| BTreeSet::new()).collect(),
                seen,
                applied,
                numbered: applied,
                unsent: [false; PORTS],
                arrivals: Vec::new(),
                resumes,
                to_end: BTreeSet::new(),
                dropped: Vec::new(),
                joined: 0,
                tally: BTreeMap::new(),
            }
        }

        fn count(&mut self, what: &'static str) {
            *self.tally.entry(what).or_default() += 1;
        }

        fn fail(&self, what: String) -> ! {
            panic!("seed {}, step {}: {what}", self.seed, self.log.len());
        }

        fn linked(&self, region: RegionId) -> bool {
            self.edge.links[region.0 as usize].is_some()
        }

        // The world.

        /// The regions that are there, in ascending order.
        fn living(&self) -> Vec<RegionId> {
            let alive = |region: &RegionId| self.played[region.0 as usize].alive;
            ports().filter(alive).collect()
        }

        /// The region that `region` is by now, if it was absorbed.
        fn now(&self, region: RegionId) -> RegionId {
            let mut region = region;
            while let Some((_, into)) = self.absorbed.iter().find(|(gone, _)| *gone == region) {
                region = *into;
            }
            region
        }

        /// What was granted comes first: a part holds chunks of the areas that the
        /// region it was split off is pinned to, until it gives them back.
        fn holder(&self, chunk: ChunkPos) -> Option<RegionId> {
            let granted = self.granted.get(&chunk).copied();
            granted.or_else(|| pinned(chunk).map(|region| self.now(region)))
        }

        fn state(&self, chunk: ChunkPos) -> BlockState {
            self.content.get(&chunk).copied().unwrap_or(AIR)
        }

        /// What `region` finds when it needs `chunk`: nothing if it holds the chunk,
        /// which it does from now on if nobody held it, or the region that does. With
        /// `trusting` it goes by what it believes, if it believes something, and
        /// asks the store only otherwise.
        fn needs(&mut self, region: RegionId, chunk: ChunkPos, trusting: bool) -> Option<RegionId> {
            let found = self.holder(chunk);
            let played = &mut self.played[region.0 as usize];
            if found == Some(region) {
                // A claim that comes before the chunk is free keeps it.
                self.returning.remove(&chunk);
                played.believes.remove(&chunk);
                return None;
            }
            if trusting && let Some(believed) = played.believes.get(&chunk).copied() {
                if found != Some(believed) {
                    self.count("a region goes by a belief that is no longer so");
                }
                return Some(believed);
            }
            match found {
                Some(holder) => {
                    played.believes.insert(chunk, holder);
                    Some(holder)
                }
                None => {
                    self.granted.insert(chunk, region);
                    played.believes.remove(&chunk);
                    None
                }
            }
        }

        /// One tick of a region: it looks at what the edge has sent it, at all of it
        /// or at the first of it, answers the subscriptions that wait, all or some,
        /// and what it makes waits to be sent in the order of ADR-0012, section 5.2.
        fn tick(&mut self, region: RegionId, thorough: bool) {
            let index = region.0 as usize;
            let take = if !self.played[index].held.is_empty() {
                0
            } else if thorough || self.dice.chance(70) {
                usize::MAX
            } else {
                self.dice.below(4)
            };
            let answering = if thorough {
                100
            } else {
                [0, 50, 100, 100][self.dice.below(4)]
            };
            self.played[index].tick += 1;
            let mut ticked = Ticked::default();
            let mut taken = 0;
            while taken < take {
                let Some(message) = self.played[index].inbox.pop_front() else {
                    break;
                };
                taken += 1;
                self.look_at(region, message, &mut ticked);
            }

            // An entity has one new pose in a tick at most: where it is after the tick,
            // with the chunk it was in before.
            let mut events: Vec<RegionEvent> = Vec::new();
            for event in std::mem::take(&mut ticked.events) {
                if let RegionEvent::EntityMoved { entity, pose, .. } = &event {
                    let earlier = events.iter_mut().find_map(|earlier| match earlier {
                        RegionEvent::EntityMoved {
                            entity: moved,
                            pose,
                            ..
                        } if moved == entity => Some(pose),
                        _ => None,
                    });
                    if let Some(earlier) = earlier {
                        *earlier = *pose;
                        continue;
                    }
                }
                events.push(event);
            }
            ticked.events = events;

            // The events of the chunks the link is subscribed to, if the region has
            // not said that another holds them.
            let tickets = &self.played[index].tickets;
            ticked.events.retain(|event| {
                event.chunks().iter().any(|chunk| {
                    tickets
                        .get(chunk)
                        .is_some_and(|ticket| !matches!(ticket.answer, Answer::Told(_)))
                })
            });
            let tick = self.played[index].tick;
            let mut made = Vec::new();
            if !ticked.events.is_empty() {
                let events = std::mem::take(&mut ticked.events);
                made.push(Made::Said(WorkerToEdge::TickDelta { tick, events }));
            }
            made.extend(ticked.spawned.into_iter().map(Made::Said));
            // An entry has its number from when it is made, and stays in the outbox
            // until the edge has confirmed it.
            let played = &mut self.played[index];
            for entry in ticked.entries.into_iter().chain(ticked.departures) {
                played.sent += 1;
                played.outbox.push_back((played.sent, entry.clone()));
                made.push(Made::Entry(played.sent, entry));
            }

            let waiting: Vec<_> = self.played[index]
                .tickets
                .iter()
                .filter(|(_, ticket)| ticket.answer == Answer::Waiting)
                .map(|(chunk, ticket)| (*chunk, ticket.role, ticket.ask))
                .collect();
            for (chunk, role, ask) in waiting {
                if !self.dice.chance(answering) {
                    continue;
                }
                self.played[index].held.remove(&chunk);
                let another = match role {
                    Role::Viewer => self.needs(region, chunk, true),
                    // A guest makes a region claim nothing outside its pinned area.
                    Role::Guest => {
                        let holds = self.holder(chunk) == Some(region);
                        (!holds || self.returning.contains(&chunk)).then_some(region)
                    }
                };
                let answer = match (role, another) {
                    (_, None) => {
                        let entities = self.played[index]
                            .residents
                            .iter()
                            .filter(|(_, resident)| resident.chunk == chunk)
                            .map(|(player, resident)| EntityState {
                                entity: resident.entity,
                                kind: EntityKind::Player {
                                    player: *player,
                                    name: "Player".to_owned(),
                                },
                                pose: Pose::at(within(chunk)),
                            })
                            .collect();
                        let state = self.state(chunk);
                        let ticket = self.played[index]
                            .tickets
                            .get_mut(&chunk)
                            .expect("the ticket was there");
                        ticket.answer = Answer::Served;
                        WorkerToEdge::ChunkSnapshot {
                            position: chunk,
                            ask,
                            tick,
                            chunk: chunk_with(chunk, state),
                            entities,
                        }
                    }
                    (Role::Viewer, Some(holder)) => {
                        let ticket = self.played[index]
                            .tickets
                            .get_mut(&chunk)
                            .expect("the ticket was there");
                        ticket.answer = Answer::Told(holder);
                        elsewhere(chunk, ask, holder)
                    }
                    (Role::Guest, Some(_)) => {
                        self.played[index].tickets.remove(&chunk);
                        if self.returning.contains(&chunk) {
                            self.count("a guest is told not mine for a chunk that is given back");
                        }
                        not_mine(chunk, ask)
                    }
                };
                made.push(Made::Said(answer));
            }
            if taken > 0 {
                let progress = WorkerToEdge::Progress {
                    applied: self.played[index].received,
                    inputs: ticked.inputs,
                };
                made.push(Made::Said(progress));
            }
            self.log.push(format!(
                "{region:?} looks at {taken} messages and makes {}",
                made.len()
            ));
            self.played[index].out.extend(made);
        }

        /// A region looks at one message of the edge.
        fn look_at(&mut self, region: RegionId, message: EdgeMessage, ticked: &mut Ticked) {
            let index = region.0 as usize;
            if let Some(number) = message.number {
                let received = self.played[index].received;
                if number <= received {
                    // Sent again on a new link.
                    return;
                }
                if number != received + 1 {
                    self.fail(format!(
                        "{region:?} was sent the numbered message {number} after {received}"
                    ));
                }
                self.played[index].received = number;
            }
            match message.body {
                EdgeToWorker::Subscribe { ask, chunks } => {
                    self.asked(region, ask);
                    let played = &mut self.played[index];
                    for chunk in chunks {
                        match played.tickets.get_mut(&chunk) {
                            Some(ticket) => {
                                ticket.role = Role::Viewer;
                                ticket.ask = ask;
                                // Asked again, the region asks the store again.
                                if matches!(ticket.answer, Answer::Told(_)) {
                                    ticket.answer = Answer::Waiting;
                                    played.believes.remove(&chunk);
                                }
                            }
                            None => {
                                let ticket = Ticket {
                                    role: Role::Viewer,
                                    ask,
                                    answer: Answer::Waiting,
                                };
                                played.tickets.insert(chunk, ticket);
                            }
                        }
                    }
                }
                EdgeToWorker::SubscribeAsGuest { ask, chunks } => {
                    self.asked(region, ask);
                    let played = &mut self.played[index];
                    for chunk in chunks {
                        match played.tickets.get_mut(&chunk) {
                            Some(ticket) => {
                                ticket.role = Role::Guest;
                                ticket.ask = ask;
                                if matches!(ticket.answer, Answer::Told(_)) {
                                    ticket.answer = Answer::Waiting;
                                }
                            }
                            None => {
                                let ticket = Ticket {
                                    role: Role::Guest,
                                    ask,
                                    answer: Answer::Waiting,
                                };
                                played.tickets.insert(chunk, ticket);
                            }
                        }
                    }
                }
                EdgeToWorker::Unsubscribe { ask, chunks } => {
                    self.asked(region, ask);
                    for chunk in chunks {
                        self.played[index].tickets.remove(&chunk);
                        // What it believed of the chunk it may keep for another need.
                        if self.dice.chance(20) {
                            self.played[index].believes.remove(&chunk);
                        }
                    }
                }
                EdgeToWorker::PlayerJoin(join) => {
                    let played = &mut self.played[index];
                    // A join begins a new stay whatever the region has (ADR-0014,
                    // section 2.1).
                    if let Some(before) = played.residents.remove(&join.player) {
                        ticked.events.push(RegionEvent::EntityRemoved {
                            entity: before.entity,
                            chunk: before.chunk,
                        });
                    }
                    played.entities += 1;
                    let entity = EntityId(played.entities);
                    let resident = Resident {
                        entity,
                        chunk: HOME,
                        last_input: 0,
                    };
                    played.residents.insert(join.player, resident);
                    ticked.spawned.push(spawned(join.player, entity));
                    ticked.events.push(RegionEvent::EntitySpawned(EntityState {
                        entity,
                        kind: EntityKind::Player {
                            player: join.player,
                            name: join.name,
                        },
                        pose: Pose::at(SPAWN),
                    }));
                }
                EdgeToWorker::PlayerLeave { player, entity } => {
                    // A leave ends the stay it names, and no other.
                    let residents = &mut self.played[index].residents;
                    let named = residents.get(&player).is_some_and(|resident| {
                        entity.is_none_or(|entity| resident.entity == entity)
                    });
                    if named && let Some(resident) = residents.remove(&player) {
                        ticked.events.push(RegionEvent::EntityRemoved {
                            entity: resident.entity,
                            chunk: resident.chunk,
                        });
                    }
                }
                EdgeToWorker::PlayerArrive { player, transfer } => {
                    let chunk = chunk_of(transfer.pose.position);
                    self.on_the_way.remove(&transfer.entity_id);
                    // One claim covers the chunk a player arrives in and their view:
                    // the region has been asked for the chunk as a viewer, unless the
                    // player has left or gone on since.
                    let theirs = self.people.get(&player).is_some_and(|person| {
                        person.region == region && person.entity == Some(transfer.entity_id)
                    });
                    let asked = self.played[index]
                        .tickets
                        .get(&chunk)
                        .is_some_and(|ticket| ticket.role == Role::Viewer);
                    if theirs && !asked {
                        self.fail(format!(
                            "{player:?} arrives in {region:?} in {chunk:?}, which it was not \
                             asked for as a viewer before"
                        ));
                    }
                    // Of two stays of a player the later one stays, which is the one
                    // with the higher entity.
                    let residents = &mut self.played[index].residents;
                    let there = residents.get(&player);
                    if let Some((entity, stood)) = there.map(|there| (there.entity, there.chunk)) {
                        if entity >= transfer.entity_id {
                            if entity != transfer.entity_id {
                                ticked.events.push(RegionEvent::EntityRemoved {
                                    entity: transfer.entity_id,
                                    chunk,
                                });
                            }
                            return;
                        }
                        ticked.events.push(RegionEvent::EntityRemoved {
                            entity,
                            chunk: stood,
                        });
                        residents.remove(&player);
                    }
                    match self.needs(region, chunk, false) {
                        None => {
                            let resident = Resident {
                                entity: transfer.entity_id,
                                chunk,
                                last_input: transfer.last_input,
                            };
                            self.played[index].residents.insert(player, resident);
                            ticked.events.push(RegionEvent::EntitySpawned(EntityState {
                                entity: transfer.entity_id,
                                kind: EntityKind::Player {
                                    player,
                                    name: transfer.name,
                                },
                                pose: transfer.pose,
                            }));
                        }
                        Some(holder) => {
                            self.count("an arrival that is sent on");
                            self.on_the_way.insert(transfer.entity_id, (chunk, holder));
                            ticked.entries.push(Durable::NotMine {
                                what: Misdirected::Arrival { player, transfer },
                                holder,
                            });
                        }
                    }
                }
                EdgeToWorker::Discard { entity, chunk } => {
                    self.on_the_way.remove(&entity);
                    ticked
                        .events
                        .push(RegionEvent::EntityRemoved { entity, chunk });
                }
                EdgeToWorker::Input {
                    player,
                    entity,
                    number,
                    input,
                } => {
                    let Some(resident) = self.played[index].residents.get_mut(&player) else {
                        // Not this region's: the edge sends it to the region that is
                        // theirs, or sends it again when they have arrived.
                        return;
                    };
                    // An input is of the stay it names, and passed over by a region
                    // that has another.
                    if resident.entity != entity {
                        return;
                    }
                    if number <= resident.last_input {
                        return;
                    }
                    if number != resident.last_input + 1 {
                        let last = resident.last_input;
                        self.fail(format!(
                            "{region:?} was sent the input {number} of {player:?} after {last}: \
                             what they did in between is lost"
                        ));
                    }
                    let PlayerInput::Move {
                        position: Some(position),
                        ..
                    } = input
                    else {
                        panic!("a generated run has players walk only");
                    };
                    let entity = resident.entity;
                    let previous_chunk = resident.chunk;
                    let target = chunk_of(position);
                    resident.last_input = number;
                    resident.chunk = target;
                    ticked.inputs.push((player, number));
                    ticked.events.push(RegionEvent::EntityMoved {
                        entity,
                        pose: Pose::at(position),
                        previous_chunk,
                    });
                    // A player who steps into a chunk the region believes another's
                    // is let go to that region.
                    if let Some(to) = self.needs(region, target, true) {
                        self.played[index].residents.remove(&player);
                        self.on_the_way.insert(entity, (target, to));
                        let mut transfer = transfer(entity, number, target);
                        transfer.pose = Pose::at(position);
                        ticked.departures.push(Durable::Departed {
                            player,
                            transfer,
                            to,
                        });
                    }
                }
                other => panic!("a generated run does not have the edge say {other:?}"),
            }
        }

        /// A region takes the subscription messages of a link in ascending order.
        fn asked(&mut self, region: RegionId, ask: u64) {
            let played = &mut self.played[region.0 as usize];
            if ask <= played.asked {
                let before = played.asked;
                self.fail(format!(
                    "{region:?} was sent the subscription message {ask} after {before}"
                ));
            }
            played.asked = ask;
        }

        // What the edge has to hold.

        fn seen(&self, chunk: ChunkPos) -> bool {
            self.people
                .values()
                .any(|person| person.wanted.contains(&chunk))
        }

        /// ADR-0013, section 2.
        fn want(&mut self, region: RegionId, chunk: ChunkPos) {
            let subscriptions = &mut self.kept[region.0 as usize];
            match subscriptions.get_mut(&chunk) {
                Some(kept) => {
                    // A guest's becomes a viewer's; what it waits for and the number
                    // it began with stay.
                    let was = (kept.role, kept.answer);
                    kept.role = Role::Viewer;
                    kept.viewers += 1;
                    match was {
                        (Role::Guest, Answer::Waiting) => {
                            self.count("a guest's that waits becomes a viewer's");
                        }
                        (Role::Guest, _) => self.count("a served guest's becomes a viewer's"),
                        _ => {}
                    }
                }
                None => {
                    subscriptions.insert(chunk, Kept::new(Role::Viewer, 1));
                }
            }
        }

        /// ADR-0013, section 2. Whoever stops seeing the chunk altogether has been
        /// taken out of `wanted` before.
        fn unwant(&mut self, region: RegionId, chunk: ChunkPos) {
            let seen = self.seen(chunk);
            let subscriptions = &mut self.kept[region.0 as usize];
            let Some(kept) = subscriptions.get_mut(&chunk) else {
                self.fail(format!(
                    "the account has no subscription at {region:?} for {chunk:?}"
                ));
            };
            kept.viewers -= 1;
            if kept.viewers > 0 {
                return;
            }
            let was = kept.answer;
            if matches!(kept.answer, Answer::Told(_)) {
                subscriptions.remove(&chunk);
                // Nothing is asked here any more, so whoever was told elsewhere with
                // this region asks again, as after its `NotMine`.
                if seen {
                    self.those_told_ask_again(region, chunk);
                }
            } else {
                kept.role = Role::Guest;
            }
            match (was, seen) {
                (Answer::Told(_), true) => self.count("one told elsewhere ends, others see it"),
                (Answer::Waiting, true) => self.count("a viewer's that waits becomes a guest's"),
                (Answer::Served, true) => self.count("a served viewer's becomes a guest's"),
                (_, false) => {}
            }
            if !seen {
                for subscriptions in &mut self.kept {
                    subscriptions.remove(&chunk);
                }
                self.replica.remove(&chunk);
                self.served_by.remove(&chunk);
                self.unsure.remove(&chunk);
            }
        }

        /// Nothing is asked of `holder` for `chunk` any more: the viewers' subscriptions
        /// that were told elsewhere with it are asked again, at once or, where that
        /// was done less than a second ago, at one of the edge's checks.
        fn those_told_ask_again(&mut self, holder: RegionId, chunk: ChunkPos) {
            let now = Instant::now();
            let mut asked_again = Vec::new();
            for subscriptions in &mut self.kept {
                let Some(kept) = subscriptions.get_mut(&chunk) else {
                    continue;
                };
                if kept.role != Role::Viewer || kept.answer != Answer::Told(holder) {
                    continue;
                }
                let lately = kept
                    .asked_again
                    .is_some_and(|at| now.duration_since(at) < ASK_AGAIN_AFTER);
                if lately {
                    kept.due = true;
                } else {
                    kept.at_once = true;
                }
                asked_again.push(lately);
            }
            for lately in asked_again {
                self.count(if lately {
                    "an asking again that is due"
                } else {
                    "an asking again at once"
                });
            }
        }

        /// A player's view is centred anew.
        fn recentre(&mut self, player: PlayerId, centre: ChunkPos) {
            let person = self.people.get_mut(&player).expect("the player is there");
            let region = person.region;
            let before = std::mem::replace(&mut person.wanted, view(centre));
            person.centre = centre;
            let now = view(centre);
            for chunk in before.difference(&now) {
                self.unwant(region, *chunk);
            }
            for chunk in now.difference(&before) {
                self.want(region, *chunk);
            }
        }

        /// A player leaves or is removed: they are unwanted everywhere.
        fn gone(&mut self, player: PlayerId) -> Option<Person> {
            let person = self.people.remove(&player)?;
            for chunk in &person.wanted {
                self.unwant(person.region, *chunk);
            }
            Some(person)
        }

        /// A region has placed a player the edge has not shown their entity yet.
        fn placed(&mut self, region: RegionId, player: PlayerId, entity: EntityId, at: ChunkPos) {
            let Some(person) = self.people.get_mut(&player) else {
                return;
            };
            if person.region != region || person.entity.is_some() {
                return;
            }
            person.entity = Some(entity);
            person.target = at;
            self.recentre(player, at);
        }

        /// The region a name of a region means to the edge: ADR-0015, section 1.
        fn stands(&self, region: RegionId) -> RegionId {
            self.stands_for.get(&region).copied().unwrap_or(region)
        }

        /// Whether the edge's link to `region` has had its welcome and the entries
        /// the welcome announced: what the edge makes for the region is sent then.
        fn through(&self, region: RegionId) -> bool {
            let resume = self.resumes[region.0 as usize].as_ref();
            self.linked(region) && resume.is_some_and(|resume| resume.entries == Some(0))
        }

        /// The edge makes a numbered message for `region`.
        fn made_for(&mut self, region: RegionId) {
            if !self.through(region) {
                self.unsent[region.0 as usize] = true;
            }
        }

        /// ADR-0013, section 4, and ADR-0015, section 4. `to` is the region the entry
        /// names as the edge reads it, and `back` whether the entry may send the
        /// player to the region it comes from: it came with a merge, or names a
        /// region that stands for the one it comes from.
        fn hand_over(
            &mut self,
            from: RegionId,
            player: PlayerId,
            transfer: &PlayerTransfer,
            to: RegionId,
            back: bool,
        ) {
            let entity = self.people.get(&player).map(|person| person.entity);
            if entity != Some(Some(transfer.entity_id)) {
                self.count(match entity {
                    None => "a hand-over of a player who left",
                    Some(_) => "a hand-over of who a player was before",
                });
                // The entity that is on its way is discarded where it was sent.
                self.made_for(to);
                return;
            }
            let person = &self.people[&player];
            let (theirs, wanted) = (person.region, person.wanted.clone());
            if theirs != from {
                self.count("a hand-over of an earlier stay in a region");
                return;
            }
            if to == from {
                if !back {
                    self.fail(format!("{from:?} lets {player:?} go to itself"));
                }
                self.count("an arrival at the region the entry came from");
                self.arrivals.push((from, player, None));
                self.made_for(from);
                self.recentre(player, chunk_of(transfer.pose.position));
                return;
            }
            let linked = (self.linked(from), self.linked(to));
            self.count(match linked {
                (true, true) => "a hand-over",
                (_, false) => "a hand-over into a region without a link",
                (false, true) => "a hand-over out of a region without a link",
            });
            for chunk in wanted {
                self.unwant(from, chunk);
                self.want(to, chunk);
            }
            let person = self.people.get_mut(&player).expect("the player is there");
            person.region = to;
            self.arrivals.push((to, player, None));
            self.made_for(to);
            self.recentre(player, chunk_of(transfer.pose.position));
        }

        /// A player's view moves from one region to another as at a hand-over, with
        /// no arrival: by a presence answer, an `Absorbed` or a `SplitOff`.
        fn moved(&mut self, player: PlayerId, to: RegionId) {
            let person = self.people.get_mut(&player).expect("the player is there");
            let from = std::mem::replace(&mut person.region, to);
            let wanted = person.wanted.clone();
            for chunk in &wanted {
                self.unwant(from, *chunk);
            }
            for chunk in &wanted {
                self.want(to, *chunk);
            }
        }

        /// A region says how far it has applied the edge's messages, in a `Progress`
        /// or in a welcome: what is kept for it up to there is dropped.
        fn progress(&mut self, region: RegionId, applied: u64) {
            let index = region.0 as usize;
            self.applied[index] = self.applied[index].max(applied);
            let applied = self.applied[index];
            self.arrivals
                .retain(|(to, _, number)| *to != region || number.is_none_or(|n| n > applied));
            if region == WEST {
                for person in self.people.values_mut() {
                    if let Some(Some(number)) = person.join
                        && number <= applied
                    {
                        person.join = None;
                    }
                }
            }
        }

        /// What is kept for a region is given up, and its numbering begins anew.
        fn given_up(&mut self, region: RegionId) {
            let index = region.0 as usize;
            self.arrivals.retain(|(to, ..)| *to != region);
            self.seen[index] = 0;
            self.applied[index] = 0;
            self.numbered[index] = 0;
            self.unsent[index] = false;
            self.came_with[index].clear();
        }

        /// Whether a join or an arrival of the player is among what is kept for the
        /// region.
        fn on_their_way(&self, region: RegionId, player: PlayerId) -> bool {
            let joining = region == WEST && self.people[&player].join.is_some();
            let arriving =
                |(to, who, _): &(RegionId, PlayerId, Option<u64>)| *to == region && *who == player;
            joining || self.arrivals.iter().any(arriving)
        }

        /// A player the edge has under `region` is not there by the region's word:
        /// ADR-0015, section 2.1, `Absent`. In a run nobody is disconnected for that
        /// whose stay a region has.
        fn judge(&mut self, region: RegionId, player: PlayerId) {
            if self.on_their_way(region, player) {
                self.count("a player who is absent is on their way");
                return;
            }
            let entity = self.people[&player].entity;
            let has = |played: &Played| {
                let resident = played.residents.get(&player);
                played.alive && resident.is_some_and(|resident| Some(resident.entity) == entity)
            };
            let on_the_way = entity.is_some_and(|entity| self.on_the_way.contains_key(&entity));
            if self.played.iter().any(has) || on_the_way {
                self.fail(format!(
                    "the records have the edge disconnect {player:?} as absent from {region:?}, \
                     and their stay is not lost"
                ));
            }
            self.count("a player is judged absent");
            let person = self.gone(player).expect("the player is there");
            self.made_for(region);
            self.dropped.push(person.client);
        }

        /// ADR-0015, section 1: the one place a region comes to stand for another.
        fn retire(&mut self, gone: RegionId, into: RegionId) {
            self.stands_for.insert(gone, into);
            for stands in self.stands_for.values_mut() {
                if *stands == gone {
                    *stands = into;
                }
            }
            self.owes[into.0 as usize].remove(&gone);
        }

        /// ADR-0015, section 3: `Absorbed`, the entry numbered `number` of `into`.
        fn absorbed_into(
            &mut self,
            into: RegionId,
            number: u64,
            gone: RegionId,
            since: u64,
            applied: u64,
            numbers: &[u64],
        ) {
            let (a, b) = (into.0 as usize, gone.0 as usize);
            self.count("an absorbed is handled");
            // 1. Whether the absorbed region shared a numbering with the edge.
            let shared = since != 0 && since == self.since[b];
            let mut applied = applied;
            if !shared {
                if self.seen[b] != 0 || self.applied[b] != 0 {
                    self.count("an absorbed region had forgotten the edge");
                    self.given_up(gone);
                }
                applied = 0;
            }
            // 2.
            self.retire(gone, into);
            // 3. Its players and their views.
            let theirs: Vec<_> = self
                .people
                .iter()
                .filter(|(_, person)| person.region == gone)
                .map(|(player, _)| *player)
                .collect();
            for player in theirs {
                self.count("a merge brings a player");
                self.moved(player, into);
                let resume = self.resumes[a].as_mut().expect("an entry comes on a link");
                resume.brought.insert(player);
            }
            // 4. What is left there are guest's subscriptions.
            let left = std::mem::take(&mut self.kept[b]);
            if !left.is_empty() {
                self.count("a merge brings guest's subscriptions");
            }
            for (chunk, _) in left {
                self.kept[a]
                    .entry(chunk)
                    .or_insert_with(|| Kept::new(Role::Guest, 0));
            }
            // 5. Whatever named it.
            for kept in self.kept.iter_mut().flat_map(|kept| kept.values_mut()) {
                if kept.answer == Answer::Told(gone) {
                    kept.answer = Answer::Told(into);
                }
            }
            for by in self.served_by.values_mut() {
                if *by == gone {
                    *by = into;
                }
            }
            // 6. What was kept for it above what it had applied.
            let kept = self.unsent[b] || self.numbered[b] > applied;
            self.arrivals
                .retain(|(to, _, n)| *to != gone || n.is_none_or(|n| n > applied));
            for arrival in &mut self.arrivals {
                if arrival.0 == gone {
                    *arrival = (into, arrival.1, None);
                }
            }
            if kept {
                self.unsent[a] = true;
            }
            self.unsent[b] = false;
            self.numbered[b] = 0;
            // 7. The entries behind it.
            for (place, was) in numbers.iter().enumerate() {
                let origin = self.came_with[b].get(was).copied().unwrap_or((gone, *was));
                self.came_with[a].insert(number + 1 + place as u64, origin);
            }
            self.came_with[b].clear();
        }

        /// ADR-0015, section 6: `SplitOff` from `from`, which names `part` as the
        /// edge reads it.
        fn split_off(&mut self, from: RegionId, part: RegionId, stays: &[(PlayerId, EntityId)]) {
            for (player, entity) in stays {
                let theirs = self
                    .people
                    .get(player)
                    .is_some_and(|person| person.region == from && person.entity == Some(*entity));
                let came_back =
                    |(to, who, _): &(RegionId, PlayerId, Option<u64>)| *to == from && who == player;
                if !theirs {
                    self.count("a split off for a stay the edge does not have there");
                } else if self.arrivals.iter().any(came_back) {
                    self.count("a split off for a stay that came back");
                } else {
                    self.count("a split off moves a stay");
                    self.moved(*player, part);
                    // Whatever the player did and the edge still keeps goes there.
                    self.made_for(part);
                }
            }
        }

        /// An entry of a region's outbox that the edge has not seen and acts on.
        /// `came_with` says whether it came to the region with a merge.
        fn entry(&mut self, region: RegionId, number: u64, entry: &Durable, came_with: bool) {
            match entry {
                Durable::Departed {
                    player,
                    transfer,
                    to,
                }
                | Durable::NotMine {
                    what: Misdirected::Arrival { player, transfer },
                    holder: to,
                } => {
                    let back = came_with || *to != region;
                    self.hand_over(region, *player, transfer, self.stands(*to), back);
                }
                Durable::Absorbed {
                    region: gone,
                    since,
                    applied,
                    numbers,
                } => self.absorbed_into(region, number, *gone, *since, *applied, numbers),
                Durable::SplitOff {
                    region: part,
                    players,
                } => self.split_off(region, self.stands(*part), players),
                _ => {}
            }
        }

        /// The entries a welcome announced have been handled: ADR-0015, section 5.
        fn entries_through(&mut self, region: RegionId) {
            let index = region.0 as usize;
            let resume = self.resumes[index].as_ref().expect("the region has a link");
            let silent = resume.owed.intersection(&self.owes[index]).next().copied();
            let no_answers = resume.presences == Some(0);
            // A run's survivor never forgets the edge, so it always says `Absorbed`.
            if let Some(gone) = silent {
                self.fail(format!(
                    "{region:?} has said no Absorbed for {gone:?}, which it owed the edge"
                ));
            }
            if !self.owes[index].is_empty() {
                self.count("a link is ended when its entries are through");
                self.to_end.insert(region);
                return;
            }
            // What was kept is sent.
            self.unsent[index] = false;
            if no_answers {
                self.presence_through(region);
            }
        }

        /// A player has had a `Present` of their own on the link to `region`.
        fn answered(&mut self, region: RegionId, player: PlayerId) {
            if let Some(resume) = &mut self.resumes[region.0 as usize] {
                resume.brought.remove(&player);
            }
        }

        /// The presence answers a welcome announced have come: ADR-0015, section 2.2.
        fn presence_through(&mut self, region: RegionId) {
            let resume = self.resumes[region.0 as usize].as_mut();
            let brought = std::mem::take(&mut resume.expect("the region has a link").brought);
            for player in brought {
                let theirs = |person: &Person| person.region == region;
                if self.people.get(&player).is_some_and(theirs) {
                    self.judge(region, player);
                }
            }
        }

        /// ADR-0015, section 2.
        fn welcomed(&mut self, region: RegionId, welcome: Welcome) {
            let index = region.0 as usize;
            let (unknown, entries, presences, applied) = match welcome {
                Welcome::Resumed {
                    entries,
                    presences,
                    applied,
                } => (None, entries, presences, applied),
                Welcome::Unknown {
                    since,
                    entries,
                    presences,
                    applied,
                } => (Some(since), entries, presences, applied),
                Welcome::Superseded => panic!("no region of a run says that"),
            };
            if let Some(since) = unknown {
                // The region has forgotten the edge, if the two had shared anything:
                // the players the edge believed to be there are removed.
                if self.seen[index] != 0 || self.applied[index] != 0 {
                    let theirs: Vec<_> = self
                        .people
                        .iter()
                        .filter(|(_, person)| person.region == region)
                        .map(|(player, _)| *player)
                        .collect();
                    self.count("a region has forgotten the edge");
                    // What the edge kept for the region it gives up, also the arrival
                    // of someone who has left since: nobody will hear of that entity.
                    self.on_the_way.retain(|_, (_, to)| *to != region);
                    for player in theirs {
                        self.count("a player of a region that has forgotten the edge");
                        let person = self.gone(player).expect("the player is there");
                        if let Some(entity) = person.entity {
                            self.on_the_way.remove(&entity);
                        }
                        self.dropped.push(person.client);
                    }
                    self.given_up(region);
                } else {
                    self.count("a welcome of a region the edge had nothing from");
                }
                self.since[index] = since;
            }
            self.progress(region, applied);
            let resume = self.resumes[index].as_mut().expect("the region has a link");
            resume.entries = Some(entries);
            resume.presences = Some(presences);
            if entries == 0 {
                self.entries_through(region);
            }
        }

        /// An entry of a region's outbox, with its number: ADR-0015, section 3.
        fn outbox(&mut self, region: RegionId, number: u64, entry: &Durable) {
            let index = region.0 as usize;
            if number > self.seen[index] {
                self.seen[index] = number;
                match self.came_with[index].remove(&number) {
                    Some((origin, was)) if was <= self.seen[origin.0 as usize] => {
                        self.count("an entry that came with a merge is passed over");
                    }
                    Some(_) => {
                        self.count("an entry that came with a merge is handled");
                        self.entry(region, number, entry, true);
                    }
                    None => self.entry(region, number, entry, false),
                }
            } else {
                self.count("an entry the edge had seen is passed over");
            }
            let resume = self.resumes[index].as_mut().expect("the region has a link");
            if let Some(left) = &mut resume.entries
                && *left > 0
            {
                *left -= 1;
                if *left == 0 {
                    self.entries_through(region);
                }
            }
        }

        /// A presence answer: ADR-0015, section 2.1.
        fn presence(&mut self, region: RegionId, player: PlayerId, answer: &Presence) {
            let index = region.0 as usize;
            let resume = self.resumes[index].as_mut().expect("the region has a link");
            let mut last = false;
            if let Some(left) = &mut resume.presences
                && *left > 0
            {
                *left -= 1;
                last = *left == 0;
            }
            let has = self
                .people
                .get(&player)
                .map(|person| (person.region, person.entity));
            match (answer, has) {
                (Presence::Present { entity, .. }, Some((theirs, Some(known))))
                    if theirs == region && known == *entity =>
                {
                    self.count("a present for a stay the edge has there");
                    self.answered(region, player);
                }
                (Presence::Present { entity, pose, .. }, Some((theirs, None)))
                    if theirs == region =>
                {
                    self.answered(region, player);
                    if self.people[&player].join.is_some() {
                        self.count("a present from before a player's join");
                    } else {
                        self.count("a present places a player");
                        self.placed(region, player, *entity, chunk_of(pose.position));
                    }
                }
                (Presence::Present { entity, .. }, Some((_, Some(known)))) if known == *entity => {
                    self.count("a present moves a stay");
                    self.answered(region, player);
                    self.moved(player, region);
                }
                (Presence::Present { .. }, _) => {
                    self.count("a present for a stay the edge does not have");
                }
                (Presence::Absent, Some((theirs, _))) if theirs == region => {
                    self.judge(region, player);
                }
                (Presence::Absent, _) => self.count("an absent passed over"),
            }
            if last {
                self.presence_through(region);
            }
        }

        /// Whether the edge has nothing of a region: ADR-0015, section 5.
        fn nothing_of(&self, region: RegionId) -> bool {
            let index = region.0 as usize;
            let mut subscriptions = self.kept.iter().flat_map(|kept| kept.values());
            !self.people.values().any(|person| person.region == region)
                && self.kept[index].is_empty()
                && !self.unsent[index]
                && self.numbered[index] <= self.applied[index]
                && !subscriptions.any(|kept| kept.answer == Answer::Told(region))
                && !self.served_by.values().any(|by| *by == region)
        }

        /// What the routing table says of regions that were absorbed: ADR-0015,
        /// section 5.
        fn table(&mut self, pairs: &[(RegionId, RegionId)]) {
            for (gone, into) in pairs {
                let mut survivor = *into;
                while let Some((_, further)) = pairs.iter().find(|(gone, _)| *gone == survivor) {
                    survivor = *further;
                }
                let survivor = self.stands(survivor);
                if self.stands_for.contains_key(gone) {
                    continue;
                }
                if self.nothing_of(*gone) {
                    self.count("the table retires a region the edge has nothing of");
                    self.retire(*gone, survivor);
                    continue;
                }
                let index = survivor.0 as usize;
                self.owes[index].insert(*gone);
                let resume = self.resumes[index].as_ref();
                let lacks = resume.is_some_and(|resume| !resume.owed.contains(gone));
                if self.through(survivor) && lacks {
                    self.count("the table ends a link to a survivor");
                    self.to_end.insert(survivor);
                }
            }
        }

        /// What the edge has to make of one message of a region: ADR-0013, sections 3,
        /// 4 and 6, and ADR-0015.
        fn told(&mut self, region: RegionId, message: &WorkerToEdge) {
            let index = region.0 as usize;
            match message {
                WorkerToEdge::ChunkSnapshot {
                    position,
                    ask,
                    chunk,
                    ..
                } => {
                    let Some(kept) = self.kept[index].get_mut(position) else {
                        self.count("a snapshot without a subscription");
                        return;
                    };
                    if *ask < kept.begun {
                        self.count("a snapshot below the number its subscription began with");
                        return;
                    }
                    let late = *ask < kept.ask;
                    kept.answer = Answer::Served;
                    self.count(if late {
                        "a snapshot taken under an earlier number"
                    } else {
                        "a snapshot taken"
                    });
                    let state = chunk.get(3, -60, 5).expect("the block is in the chunk");
                    self.replica.insert(*position, state);
                    self.served_by.insert(*position, region);
                    self.unsure.remove(position);
                }
                WorkerToEdge::Elsewhere {
                    chunk,
                    ask,
                    region: holder,
                } => {
                    // A name of a region that is no more means the region it went into.
                    let holder = self.stands(*holder);
                    let Some(kept) = self.kept[index].get_mut(chunk) else {
                        self.count("an elsewhere without a subscription");
                        return;
                    };
                    if kept.role != Role::Viewer || kept.ask != *ask || holder == region {
                        self.count("an elsewhere passed over");
                        return;
                    }
                    kept.answer = Answer::Told(holder);
                    kept.at_once = false;
                    kept.due = false;
                    if self.served_by.get(chunk) == Some(&region) {
                        self.served_by.remove(chunk);
                    }
                    let linked = self.linked(holder);
                    let there = &mut self.kept[holder.0 as usize];
                    if let Some(named) = there.get_mut(chunk) {
                        // A region that is named and has itself said that another
                        // holds the chunk is asked again: the two may name each other.
                        if named.role == Role::Viewer && matches!(named.answer, Answer::Told(_)) {
                            let lately = named.asked_again.is_some_and(|at| {
                                Instant::now().duration_since(at) < ASK_AGAIN_AFTER
                            });
                            if lately {
                                named.due = true;
                            } else {
                                named.at_once = true;
                            }
                        }
                        self.count("an elsewhere that names a region the edge is subscribed at");
                    } else {
                        there.insert(*chunk, Kept::new(Role::Guest, 0));
                        self.count(if linked {
                            "an elsewhere that makes a guest's"
                        } else {
                            "an elsewhere that makes a guest's at a region without a link"
                        });
                    }
                }
                WorkerToEdge::NotMine { chunk, ask } => {
                    let taken = self.kept[index]
                        .get(chunk)
                        .is_some_and(|kept| kept.role == Role::Guest && kept.ask == *ask);
                    if !taken {
                        self.count("a not mine passed over");
                        return;
                    }
                    self.count("a not mine taken");
                    // The region has ended the subscription by saying so.
                    self.kept[index].remove(chunk);
                    self.heard[index].remove(chunk);
                    if self.served_by.get(chunk) == Some(&region) {
                        self.served_by.remove(chunk);
                    }
                    self.those_told_ask_again(region, *chunk);
                }
                WorkerToEdge::TickDelta { events, .. } => {
                    for event in events {
                        match event {
                            RegionEvent::EntityMoved { entity, pose, .. } => {
                                let moved = self
                                    .people
                                    .iter()
                                    .find(|(_, person)| person.entity == Some(*entity))
                                    .map(|(player, person)| (*player, person.region));
                                let Some((player, theirs)) = moved else {
                                    continue;
                                };
                                assert_eq!(theirs, region, "a run has a player's region move them");
                                self.recentre(player, chunk_of(pose.position));
                            }
                            RegionEvent::BlockChanged { position, state } => {
                                let chunk = position.chunk();
                                if !self.replica.contains_key(&chunk) {
                                    continue;
                                }
                                let serving = self.served_by.get(&chunk) == Some(&region)
                                    && self.kept[index]
                                        .get(&chunk)
                                        .is_some_and(|kept| kept.answer == Answer::Served);
                                if serving {
                                    self.replica.insert(chunk, *state);
                                    self.count("a block changes in a chunk that is served");
                                } else {
                                    self.unsure.insert(chunk);
                                    self.count("a block changes in a chunk that is not served");
                                }
                            }
                            _ => {}
                        }
                    }
                }
                WorkerToEdge::ToPlayer {
                    player,
                    event:
                        PlayerEvent::Spawned {
                            entity_id,
                            position,
                            ..
                        },
                } => self.placed(region, *player, *entity_id, chunk_of(*position)),
                WorkerToEdge::Progress { applied, .. } => self.progress(region, *applied),
                WorkerToEdge::Presence { player, answer } => self.presence(region, *player, answer),
                WorkerToEdge::Outbox { number, entry } => self.outbox(region, *number, entry),
                WorkerToEdge::Welcome(welcome) => self.welcomed(region, *welcome),
                _ => {}
            }
        }

        /// What the edge has to make of its link to `region` ending or being
        /// replaced: ADR-0013, section 6. And what the region is left with.
        fn unlinked(&mut self, region: RegionId) {
            let index = region.0 as usize;
            for kept in self.kept[index].values_mut() {
                kept.ask = 0;
                kept.begun = 0;
                kept.answer = Answer::Waiting;
                kept.at_once = false;
                kept.due = false;
            }
            self.heard[index].clear();
            self.resumes[index] = None;
            self.to_end.remove(&region);
            let played = &mut self.played[index];
            played.inbox.clear();
            played.tickets.clear();
            played.held.clear();
            played.asked = 0;
            // Only the entries of its outbox outlive the link, and those are in the
            // outbox until the edge has confirmed them.
            played.out.clear();
        }

        // The steps.

        async fn join(&mut self) {
            if self.people.len() >= 4 {
                return;
            }
            self.joined += 1;
            let player = player(100 + self.joined);
            self.log.push(format!("player {} joins", 100 + self.joined));
            let client = self.edge.join(player).await;
            let person = Person {
                client,
                region: WEST,
                entity: None,
                centre: HOME,
                wanted: BTreeSet::new(),
                target: HOME,
                inputs: 0,
                goal: HOME,
                looked_at: 0,
                join: Some(None),
            };
            self.people.insert(player, person);
            self.made_for(WEST);
            self.sync().await;
        }

        fn someone(&mut self, placed: bool) -> Option<PlayerId> {
            let those: Vec<_> = self
                .people
                .iter()
                .filter(|(_, person)| !placed || person.entity.is_some())
                .map(|(player, _)| *player)
                .collect();
            (!those.is_empty()).then(|| those[self.dice.below(those.len())])
        }

        async fn leave(&mut self) {
            let Some(player) = self.someone(false) else {
                return;
            };
            self.log.push(format!("{player:?} leaves"));
            let person = self.gone(player).expect("the player is there");
            self.made_for(person.region);
            self.edge.leave(&person.client).await;
            self.sync().await;
        }

        /// A player steps into a chunk next to the one their last step took them to.
        /// They wait with it while the edge has yet to hear of two steps, so that a
        /// region is asked for the chunks its players stand in, as it is when players
        /// walk at the speed they can. And they wait until their client has the chunk
        /// the edge has them in, as a client does: so a region's snapshot of the chunk
        /// someone enters the world in has them.
        async fn walk(&mut self) {
            let Some(player) = self.someone(true) else {
                return;
            };
            let person = self.people.get_mut(&player).expect("the player is there");
            let behind = (person.target.x - person.centre.x)
                .abs()
                .max((person.target.z - person.centre.z).abs());
            if behind > 1 || !person.client.chunks.contains_key(&person.centre) {
                return;
            }
            // They walk towards a place somewhere in the world, and then to another.
            if person.goal == person.target {
                let x = self.dice.below(7) as i32 - 1;
                let z = self.dice.below(5) as i32 - 2;
                person.goal = ChunkPos::new(x, z);
                if person.goal == person.target {
                    return;
                }
            }
            let target = ChunkPos::new(
                person.target.x + (person.goal.x - person.target.x).signum(),
                person.target.z + (person.goal.z - person.target.z).signum(),
            );
            person.target = target;
            person.inputs += 1;
            *self.tally.entry("a step").or_default() += 1;
            self.log.push(format!(
                "{player:?} steps into {target:?}, input {}",
                person.inputs
            ));
            let region = person.region;
            self.edge.input(&person.client, step_into(target)).await;
            self.made_for(region);
            self.sync().await;
        }

        fn some_region(&mut self, linked: Option<bool>) -> Option<RegionId> {
            let those: Vec<_> = self
                .living()
                .into_iter()
                .filter(|region| linked.is_none_or(|linked| self.linked(*region) == linked))
                .collect();
            (!those.is_empty()).then(|| those[self.dice.below(those.len())])
        }

        /// The edge reads the next thing a region has made for it. A welcome is read
        /// by itself: the entries and the presence answers it announces follow as
        /// things of their own, so that what other regions say, what players do and
        /// what the table says can fall between any two of them.
        async fn deliver(&mut self, region: RegionId) {
            let index = region.0 as usize;
            if !self.linked(region) {
                return;
            }
            let Some(made) = self.played[index].out.pop_front() else {
                return;
            };
            let message = match made {
                Made::Said(message) => message,
                Made::Entry(number, entry) => WorkerToEdge::Outbox { number, entry },
                Made::Welcome {
                    resumed,
                    since,
                    entries,
                    presences,
                    applied,
                } => {
                    for (_, entry) in &entries {
                        self.count(match entry {
                            Durable::Departed { .. } | Durable::NotMine { .. } => {
                                "a hand-over among a welcome's entries"
                            }
                            Durable::Absorbed { .. } => "an absorbed among a welcome's entries",
                            Durable::SplitOff { .. } => "a split off among a welcome's entries",
                            _ => "another entry among a welcome's entries",
                        });
                    }
                    let (count, answers) = (entries.len() as u32, presences.len() as u32);
                    let out = &mut self.played[index].out;
                    for presence in presences.into_iter().rev() {
                        out.push_front(Made::Said(presence));
                    }
                    for (number, entry) in entries.into_iter().rev() {
                        out.push_front(Made::Entry(number, entry));
                    }
                    WorkerToEdge::Welcome(if resumed {
                        Welcome::Resumed {
                            entries: count,
                            presences: answers,
                            applied,
                        }
                    } else {
                        Welcome::Unknown {
                            since,
                            entries: count,
                            presences: answers,
                            applied,
                        }
                    })
                }
            };
            self.log
                .push(format!("{region:?} says {}", brief(&message)));
            self.told(region, &message);
            self.edge.tell(region, message);
            self.sync().await;
        }

        /// Whether the link of a region may end now. A step of a player that the
        /// region has taken and the edge has not heard of would be lost with the
        /// link, and the records do not say how the edge learns where the player is
        /// then.
        fn may_end(&self, region: RegionId) -> bool {
            let moved = |made: &Made| match made {
                Made::Said(WorkerToEdge::TickDelta { events, .. }) => events
                    .iter()
                    .any(|event| matches!(event, RegionEvent::EntityMoved { .. })),
                _ => false,
            };
            !self.played[region.0 as usize].out.iter().any(moved)
        }

        async fn lose(&mut self) {
            let Some(region) = self.some_region(Some(true)) else {
                return;
            };
            if !self.may_end(region) {
                return;
            }
            self.log.push(format!("the link to {region:?} ends"));
            self.count("a link ends");
            self.unlinked(region);
            self.edge.lose(region).await;
            self.sync().await;
        }

        /// The edge gets a link to a region, which takes its hello at once: the
        /// welcome, with the entries of its outbox and the presence answers, waits to
        /// be sent.
        async fn relink(&mut self, region: RegionId) {
            let index = region.0 as usize;
            if self.linked(region) {
                if !self.may_end(region) {
                    return;
                }
                self.unlinked(region);
                self.count("a link is replaced while it stands");
            }
            self.log.push(format!("a new link to {region:?}"));
            let hello = self.edge.relink(region).await;
            if !hello.chunks.is_empty() {
                self.count("a hello with viewer's subscriptions");
            }
            if !hello.guests.is_empty() {
                self.count("a hello with guest's subscriptions");
            }

            // The hello names exactly what the link before had left.
            let players: BTreeSet<_> = self
                .people
                .iter()
                .filter(|(_, person)| person.region == region)
                .map(|(player, _)| *player)
                .collect();
            let of = |role| -> BTreeSet<ChunkPos> {
                self.kept[index]
                    .iter()
                    .filter(|(_, kept)| kept.role == role)
                    .map(|(chunk, _)| *chunk)
                    .collect()
            };
            let said: BTreeSet<_> = hello.players.iter().copied().collect();
            if said != players || said.len() != hello.players.len() {
                self.fail(format!(
                    "the hello to {region:?} names the players {:?}, not {players:?}",
                    hello.players
                ));
            }
            for (role, list) in [(Role::Viewer, &hello.chunks), (Role::Guest, &hello.guests)] {
                let expected = of(role);
                if set(list) != expected || list.len() != expected.len() {
                    self.fail(format!(
                        "the hello to {region:?} names as {role:?}'s {:?} and lacks {:?}",
                        minus(&set(list), &expected),
                        minus(&expected, &set(list)),
                    ));
                }
            }
            if (hello.since, hello.seen) != (self.since[index], self.seen[index]) {
                self.fail(format!(
                    "the hello to {region:?} says since {} and seen {}, not {} and {}",
                    hello.since, hello.seen, self.since[index], self.seen[index]
                ));
            }
            for chunk in &hello.guests {
                self.heard[index].insert(*chunk, (Role::Guest, 0, 0));
            }
            for chunk in &hello.chunks {
                self.heard[index].insert(*chunk, (Role::Viewer, 0, 0));
            }
            self.resumes[index] = Some(Resume {
                owed: self.owes[index].clone(),
                ..Resume::default()
            });

            let fresh = self.sinces + 1;
            let played = &mut self.played[index];
            for (chunk, (role, ..)) in &self.heard[index] {
                let ticket = Ticket {
                    role: *role,
                    ask: 0,
                    answer: Answer::Waiting,
                };
                played.tickets.insert(*chunk, ticket);
                played.held.insert(*chunk);
            }
            // ADR-0012, section 4.5. A region that knows the edge with the `since` of
            // the hello resumes. One that does not know it, or knows it with another
            // `since` and has taken messages from it, makes a state for it anew, with
            // nobody in it. One whose state for the edge came with a merge or a split
            // has taken nothing: it says its `since`, and answers from that state.
            let resumed = played.knows && hello.since == played.since;
            if resumed {
                played.outbox.retain(|(number, _)| *number > hello.seen);
            } else if !played.knows || played.received > 0 {
                played.knows = true;
                played.since = fresh;
                played.residents.clear();
                played.outbox.clear();
                played.sent = 0;
                played.received = 0;
                self.sinces = fresh;
            }
            let entries: Vec<_> = played.outbox.iter().cloned().collect();
            let present = |player: &PlayerId, resident: &Resident| WorkerToEdge::Presence {
                player: *player,
                answer: Presence::Present {
                    entity: resident.entity,
                    pose: Pose::at(within(resident.chunk)),
                    hotbar: [None; HOTBAR_SLOTS],
                    selected_slot: 0,
                    last_input: resident.last_input,
                    handled: None,
                },
            };
            // ADR-0014, section 3.7: an answer for each player the hello named, and
            // then `Present` for every other stay the region has for the edge.
            let mut presences = Vec::new();
            for player in &hello.players {
                presences.push(match played.residents.get(player) {
                    Some(resident) => present(player, resident),
                    None => WorkerToEdge::Presence {
                        player: *player,
                        answer: Presence::Absent,
                    },
                });
            }
            for (player, resident) in &played.residents {
                if !hello.players.contains(player) {
                    presences.push(present(player, resident));
                }
            }
            let welcome = Made::Welcome {
                resumed,
                since: played.since,
                entries,
                presences,
                applied: played.received,
            };
            played.out.push_back(welcome);
            self.sync().await;
        }

        /// A region gives back a chunk outside its pinned area that it has no use for:
        /// no link is subscribed to it, nobody stands in it or is on their way there,
        /// and nothing about it is still to be sent.
        fn give_back(&mut self) {
            let about = |made: &Made, chunk: ChunkPos| match made {
                Made::Said(WorkerToEdge::ChunkSnapshot { position, .. }) => *position == chunk,
                Made::Said(WorkerToEdge::TickDelta { events, .. }) => {
                    events.iter().any(|event| event.chunks().contains(&chunk))
                }
                _ => false,
            };
            let free: Vec<_> = self
                .granted
                .iter()
                .filter(|(chunk, region)| {
                    let played = &self.played[region.0 as usize];
                    // A part keeps what it holds of the areas another region is
                    // pinned to. Given back, such a chunk is the pinned region's again
                    // and nobody tells it, so it goes on naming the part, which names
                    // it. The edge then has the one that was named ask the store again;
                    // the regions of these runs do not play a pinned region that is
                    // behind in that way, and the case is tried by itself in
                    // `two_regions_that_name_each_other_for_a_chunk_are_asked_again`.
                    !self.returning.contains(chunk)
                        && pinned(**chunk).is_none()
                        && !played.tickets.contains_key(chunk)
                        && !played
                            .inbox
                            .iter()
                            .any(|message| names(&message.body, **chunk))
                        && !played
                            .residents
                            .values()
                            .any(|resident| resident.chunk == **chunk)
                        && !self.on_the_way.values().any(|(to, _)| to == *chunk)
                        && !played.out.iter().any(|made| about(made, **chunk))
                })
                .map(|(chunk, region)| (*chunk, *region))
                .collect();
            if free.is_empty() {
                return;
            }
            let (chunk, region) = free[self.dice.below(free.len())];
            self.log.push(format!("{region:?} gives back {chunk:?}"));
            self.count("a region gives a chunk back");
            self.returning.insert(chunk);
        }

        /// The store frees a chunk that was given back.
        fn free(&mut self, all: bool) {
            let returning: Vec<_> = self.returning.iter().copied().collect();
            if returning.is_empty() {
                return;
            }
            let chosen = returning[self.dice.below(returning.len())];
            for chunk in returning {
                if all || chunk == chosen {
                    self.log.push(format!("the store frees {chunk:?}"));
                    self.returning.remove(&chunk);
                    self.granted.remove(&chunk);
                }
            }
        }

        /// A region comes to believe that another holds a chunk between the pinned
        /// areas which it does not hold itself, as it would have heard for another
        /// need at a time when that was so.
        fn mislead(&mut self) {
            let living = self.living();
            let region = living[self.dice.below(living.len())];
            let residents: Vec<_> = self.played[region.0 as usize]
                .residents
                .values()
                .map(|resident| resident.chunk)
                .collect();
            // Half the time about a chunk next to one of its players, who may step
            // into it and be let go to the wrong region.
            let chunk = if !residents.is_empty() && self.dice.chance(50) {
                let near = residents[self.dice.below(residents.len())];
                let dx = self.dice.below(3) as i32 - 1;
                let dz = self.dice.below(3) as i32 - 1;
                ChunkPos::new(near.x + dx, near.z + dz)
            } else {
                let x = 1 + self.dice.below(3) as i32;
                let z = self.dice.below(11) as i32 - 5;
                ChunkPos::new(x, z)
            };
            let holder = self.holder(chunk);
            if pinned(chunk).is_some() || holder == Some(region) {
                return;
            }
            let others: Vec<_> = living
                .into_iter()
                .filter(|other| *other != region && Some(*other) != holder)
                .collect();
            if others.is_empty() {
                return;
            }
            let other = others[self.dice.below(others.len())];
            self.log.push(format!(
                "{region:?} believes that {other:?} holds {chunk:?}"
            ));
            self.played[region.0 as usize].believes.insert(chunk, other);
        }

        /// The edge reads what a region has made up to an entry of its outbox that
        /// lets a player go, and then the region's link ends: the entry is read from
        /// the next welcome.
        async fn lose_before_a_hand_over(&mut self) {
            let lets_go = |made: &Made| {
                matches!(
                    made,
                    Made::Entry(_, Durable::Departed { .. } | Durable::NotMine { .. })
                )
            };
            let those: Vec<_> = self
                .living()
                .into_iter()
                .filter(|region| {
                    self.linked(*region) && self.played[region.0 as usize].out.iter().any(lets_go)
                })
                .collect();
            if those.is_empty() {
                return;
            }
            let region = those[self.dice.below(those.len())];
            while self.linked(region)
                && !self.played[region.0 as usize]
                    .out
                    .front()
                    .is_some_and(lets_go)
            {
                self.deliver(region).await;
            }
            if !self.linked(region) || !self.may_end(region) {
                return;
            }
            self.log.push(format!(
                "the link to {region:?} ends before a hand-over is read"
            ));
            self.count("a link ends before a hand-over is read");
            self.unlinked(region);
            self.edge.lose(region).await;
            self.sync().await;
        }

        /// A block changes in a chunk a region serves its link.
        fn change_a_block(&mut self) {
            let Some(region) = self.some_region(Some(true)) else {
                return;
            };
            let index = region.0 as usize;
            let served: Vec<_> = self.played[index]
                .tickets
                .iter()
                .filter(|(_, ticket)| ticket.answer == Answer::Served)
                .map(|(chunk, _)| *chunk)
                .collect();
            if served.is_empty() {
                return;
            }
            let chunk = served[self.dice.below(served.len())];
            let state = match self.state(chunk) {
                AIR => STONE,
                STONE => GRANITE,
                _ => AIR,
            };
            self.log
                .push(format!("{region:?} changes {chunk:?} to {state:?}"));
            self.content.insert(chunk, state);
            let played = &mut self.played[index];
            played.tick += 1;
            played.out.push_back(Made::Said(changed(chunk, state)));
        }

        /// A region that has no link forgets the edge, as after thirty seconds
        /// without one: its players of that edge are gone with what it had for the
        /// edge. Not in a run with merges and splits, in which nobody is to be
        /// disconnected.
        fn forget(&mut self) {
            if self.reshaping {
                return;
            }
            let Some(region) = self.some_region(Some(false)) else {
                return;
            };
            self.log.push(format!("{region:?} forgets the edge"));
            let played = &mut self.played[region.0 as usize];
            played.knows = false;
            played.residents.clear();
            played.received = 0;
            played.sent = 0;
            for (_, entry) in std::mem::take(&mut played.outbox) {
                let (Durable::Departed { transfer, .. }
                | Durable::NotMine {
                    what: Misdirected::Arrival { transfer, .. },
                    ..
                }) = entry
                else {
                    continue;
                };
                self.on_the_way.remove(&transfer.entity_id);
            }
        }

        /// A region stops, with its link if it has one: for a merge or a split,
        /// which close every link of a region (ADR-0014, section 3.3).
        async fn stop(&mut self, region: RegionId) {
            let linked = self.linked(region);
            self.unlinked(region);
            if linked {
                self.edge.lose(region).await;
            }
        }

        /// A region absorbs another: ADR-0014, section 2.3. The players of the
        /// absorbed region are the survivor's, of two stays of one player the later;
        /// the survivor's outbox gets `Absorbed` and behind it what the absorbed
        /// region had in its own; its chunks and its areas are the survivor's; and
        /// the survivor forgets what it believed of other regions.
        async fn merge(&mut self) {
            let living = self.living();
            let absorbable: Vec<_> = living
                .iter()
                .copied()
                .filter(|gone| *gone != WEST)
                .collect();
            if absorbable.is_empty() {
                return;
            }
            let gone = absorbable[self.dice.below(absorbable.len())];
            let others: Vec<_> = living.into_iter().filter(|into| *into != gone).collect();
            let into = others[self.dice.below(others.len())];
            if !self.may_end(gone) || !self.may_end(into) {
                return;
            }
            self.log.push(format!("{into:?} absorbs {gone:?}"));
            self.count("a merge");
            self.stop(gone).await;
            self.stop(into).await;

            let absorbed = std::mem::take(&mut self.played[gone.0 as usize]);
            let fresh = self.sinces + 1;
            let survivor = &mut self.played[into.0 as usize];
            survivor.believes.clear();
            if absorbed.knows && !survivor.knows {
                // The survivor comes by a state for the edge through the merge.
                survivor.knows = true;
                survivor.since = fresh;
                survivor.received = 0;
                survivor.sent = 0;
                survivor.outbox.clear();
                self.sinces = fresh;
            }
            if survivor.knows {
                let numbers = absorbed.outbox.iter().map(|(number, _)| *number);
                let entry = Durable::Absorbed {
                    region: gone,
                    since: if absorbed.knows { absorbed.since } else { 0 },
                    applied: if absorbed.knows { absorbed.received } else { 0 },
                    numbers: numbers.collect(),
                };
                let behind = absorbed.outbox.into_iter().map(|(_, entry)| entry);
                for entry in std::iter::once(entry).chain(behind) {
                    survivor.sent += 1;
                    survivor.outbox.push_back((survivor.sent, entry));
                }
            }
            for (player, resident) in absorbed.residents {
                let later = |there: &Resident| there.entity >= resident.entity;
                if !survivor.residents.get(&player).is_some_and(later) {
                    survivor.residents.insert(player, resident);
                }
            }
            for holder in self.granted.values_mut() {
                if *holder == gone {
                    *holder = into;
                }
            }
            for (_, to) in self.on_the_way.values_mut() {
                if *to == gone {
                    *to = into;
                }
            }
            self.absorbed.push((gone, into));
            self.sync().await;
        }

        /// A region is split: ADR-0014, section 2.4. The players standing in one chunk
        /// that the region holds go, with the chunks around it that are nearer to it
        /// than to anyone who stays, and are a new region's; the region's outbox gets
        /// `SplitOff`; and both begin as a restored region does, knowing nothing of
        /// who holds what.
        async fn split(&mut self) {
            // A region's id is never used again, and the last is the witness's.
            let part = RegionId((REGIONS.len() + self.parts) as u32);
            if part.0 as usize >= PORTS - 1 {
                return;
            }
            let mut seeds = Vec::new();
            for region in self.living() {
                if !self.may_end(region) {
                    continue;
                }
                for resident in self.played[region.0 as usize].residents.values() {
                    let chunk = resident.chunk;
                    let held =
                        self.holder(chunk) == Some(region) && !self.returning.contains(&chunk);
                    if chunk != HOME && held {
                        seeds.push((region, chunk));
                    }
                }
            }
            if seeds.is_empty() {
                return;
            }
            let (region, seed) = seeds[self.dice.below(seeds.len())];
            let index = region.0 as usize;
            let residents = &self.played[index].residents;
            let go: Vec<_> = residents
                .iter()
                .filter(|(_, resident)| resident.chunk == seed)
                .map(|(player, _)| *player)
                .collect();
            let mut stay: Vec<_> = residents
                .values()
                .filter(|resident| resident.chunk != seed)
                .map(|resident| resident.chunk)
                .collect();
            if self.holder(HOME) == Some(region) {
                stay.push(HOME);
            }
            // A region that would be left with nothing is not split. The three the
            // world began with are pinned to their areas, and always keep those.
            if stay.is_empty() && index >= REGIONS.len() {
                return;
            }
            let apart = |one: ChunkPos, other: ChunkPos| {
                (one.x - other.x).abs().max((one.z - other.z).abs())
            };
            let mut chunks = Vec::new();
            for x in seed.x - 2..=seed.x + 2 {
                for z in seed.z - 2..=seed.z + 2 {
                    let chunk = ChunkPos::new(x, z);
                    let held =
                        self.holder(chunk) == Some(region) && !self.returning.contains(&chunk);
                    let nearer = |other: &ChunkPos| apart(chunk, seed) < apart(chunk, *other);
                    if held && stay.iter().all(nearer) {
                        chunks.push(chunk);
                    }
                }
            }
            self.log.push(format!(
                "{region:?} is split: {part:?} has {go:?} and {} chunks around {seed:?}",
                chunks.len()
            ));
            self.count("a split");
            self.stop(region).await;

            self.parts += 1;
            self.sinces += 1;
            let played = &mut self.played[index];
            played.believes.clear();
            let mut residents = BTreeMap::new();
            let mut stays = Vec::new();
            for player in go {
                let resident = played.residents.remove(&player).expect("they stand there");
                stays.push((player, resident.entity));
                residents.insert(player, resident);
            }
            played.sent += 1;
            let entry = Durable::SplitOff {
                region: part,
                players: stays,
            };
            played.outbox.push_back((played.sent, entry));
            let tick = played.tick;
            self.played[part.0 as usize] = Played {
                since: self.sinces,
                residents,
                tick,
                alive: true,
                knows: true,
                ..Played::default()
            };
            for chunk in chunks {
                self.granted.insert(chunk, part);
            }
            self.sync().await;
        }

        /// The edge's process hands the edge what the routing table says of regions
        /// that were absorbed: all of it, every time.
        async fn tell_the_table(&mut self) {
            if self.absorbed.is_empty() {
                return;
            }
            let pairs = self.absorbed.clone();
            self.log.push(format!("the table says {pairs:?}"));
            self.count("the table is told");
            self.table(&pairs);
            self.edge.pairs(pairs).await;
            self.sync().await;
        }

        /// For a moment everything goes fast: the regions that have a link look at
        /// everything and answer everything, and the edge reads it all, a few times
        /// over and with no time passing. A region that is asked again and names the
        /// same region once more has its answer read within the second.
        async fn hurry(&mut self) {
            self.log.push("everything hurries".to_owned());
            for _ in 0..3 {
                for region in self.living() {
                    if !self.linked(region) {
                        continue;
                    }
                    self.tick(region, true);
                    while self.linked(region) && !self.played[region.0 as usize].out.is_empty() {
                        self.deliver(region).await;
                    }
                }
            }
        }

        async fn pass_time(&mut self) {
            self.log.push("time passes".to_owned());
            tokio::time::advance(Duration::from_millis(1100)).await;
            self.sync().await;
        }

        async fn step(&mut self) {
            // Now and then a region absorbs another or is split, or the edge is told
            // what the routing table says of that.
            if self.reshaping && self.dice.chance(4) {
                match self.dice.below(10) {
                    0..=3 => self.merge().await,
                    4..=7 => self.split().await,
                    _ => self.tell_the_table().await,
                }
                self.check().await;
                return;
            }
            match self.dice.below(100) {
                0..=3 => self.join().await,
                4..=5 => self.leave().await,
                6..=33 => self.walk().await,
                34..=55 => {
                    // A region that has something to look at or to answer, if any.
                    let busy: Vec<_> = self
                        .living()
                        .into_iter()
                        .filter(|region| {
                            let played = &self.played[region.0 as usize];
                            let waits = |ticket: &Ticket| ticket.answer == Answer::Waiting;
                            let reads = played.held.is_empty() && !played.inbox.is_empty();
                            self.linked(*region) && (reads || played.tickets.values().any(waits))
                        })
                        .collect();
                    if !busy.is_empty() {
                        let region = busy[self.dice.below(busy.len())];
                        self.tick(region, false);
                    }
                }
                56..=85 => {
                    let busy: Vec<_> = self
                        .living()
                        .into_iter()
                        .filter(|region| {
                            self.linked(*region) && !self.played[region.0 as usize].out.is_empty()
                        })
                        .collect();
                    if !busy.is_empty() {
                        let region = busy[self.dice.below(busy.len())];
                        for _ in 0..=self.dice.below(8) {
                            self.deliver(region).await;
                        }
                    }
                }
                86 => {
                    if self.dice.chance(50) {
                        self.lose().await;
                    } else {
                        self.lose_before_a_hand_over().await;
                    }
                }
                87..=89 => {
                    if let Some(region) = self.some_region(None) {
                        self.relink(region).await;
                    }
                }
                90..=93 => self.give_back(),
                94..=95 => {
                    if self.dice.chance(15) {
                        self.free(false);
                    } else {
                        self.mislead();
                    }
                }
                96 => self.change_a_block(),
                97 => self.pass_time().await,
                98 => self.hurry().await,
                // Seldom: it takes every player of the region with it.
                _ if self.dice.chance(30) => self.forget(),
                _ => self.walk().await,
            }
            self.check().await;
        }

        // Looking at what the edge did.

        /// Waits until the edge has handled everything said to it, reads everything
        /// it has sent the regions, and holds what that makes of the regions'
        /// subscriptions against what the edge has to hold. A link the edge has ended
        /// by itself has to be one that the table's word gave it reason to end.
        async fn sync(&mut self) {
            self.edge.drained().await;
            for region in self.edge.linked() {
                self.edge.handled(region).await;
            }
            let ended: Vec<_> = self.edge.gone.iter().copied().collect();
            for region in ended {
                if !self.to_end.contains(&region) {
                    self.fail(format!(
                        "the edge ended its link to {region:?}, which the records give it no \
                         reason for"
                    ));
                }
                self.log
                    .push(format!("the edge ends its link to {region:?}"));
                self.count("the edge ends a link over the table's word");
                self.edge.ended_by_the_edge(region).await;
                self.edge.heard[region.0 as usize] = Heard::default();
                self.unlinked(region);
            }
            if let Some(region) = self.to_end.iter().find(|region| self.linked(**region)) {
                self.fail(format!(
                    "the edge has not ended its link to {region:?}, which owes it an Absorbed"
                ));
            }
            // What the edge said to a region before the one whose word made it say so
            // was waited for is on the link by now.
            for region in self.edge.linked() {
                for message in self.edge.waiting(region) {
                    self.read(region, message);
                }
            }
            self.compare();
            self.check().await;
        }

        /// Reads a message the edge sent a region.
        fn read(&mut self, region: RegionId, message: EdgeMessage) {
            let index = region.0 as usize;
            if let Some(number) = message.number {
                // What the edge is seen to send says what it keeps: the run cannot
                // see that otherwise.
                self.numbered[index] = self.numbered[index].max(number);
                match &message.body {
                    EdgeToWorker::PlayerJoin(join) => {
                        if let Some(person) = self.people.get_mut(&join.player)
                            && let Some(known) = &mut person.join
                        {
                            *known = Some(known.unwrap_or(0).max(number));
                        }
                    }
                    EdgeToWorker::PlayerArrive { player, .. } => {
                        let arrivals = self.arrivals.iter_mut();
                        let mut theirs: Vec<_> = arrivals
                            .filter(|(to, who, _)| *to == region && who == player)
                            .collect();
                        // One that is sent again has its number already.
                        if !theirs.iter().any(|arrival| arrival.2 == Some(number))
                            && let Some(arrival) =
                                theirs.iter_mut().find(|arrival| arrival.2.is_none())
                        {
                            arrival.2 = Some(number);
                        }
                    }
                    _ => {}
                }
            }
            let (ask, chunks, role) = match &message.body {
                EdgeToWorker::Subscribe { ask, chunks } => (*ask, chunks, Some(Role::Viewer)),
                EdgeToWorker::SubscribeAsGuest { ask, chunks } => (*ask, chunks, Some(Role::Guest)),
                EdgeToWorker::Unsubscribe { ask, chunks } => (*ask, chunks, None),
                EdgeToWorker::Confirm { number } => {
                    // The region drops what the edge has handled, if it comes to read
                    // that: in a run with merges a confirmation is lost now and then,
                    // as with a link that ends. The entry is then in the outbox the
                    // region's survivor takes over, and the edge has seen it.
                    if !self.reshaping || self.dice.chance(70) {
                        let outbox = &mut self.played[index].outbox;
                        outbox.retain(|(entry, _)| entry > number);
                    }
                    return;
                }
                _ => {
                    self.log.push(format!("  to {region:?}: {message:?}"));
                    self.played[index].inbox.push_back(message);
                    return;
                }
            };
            let listed: Vec<_> = chunks.iter().map(|chunk| (chunk.x, chunk.z)).collect();
            self.log
                .push(format!("  to {region:?}: {role:?} {ask} {listed:?}"));
            for chunk in chunks {
                let before = self.heard[index].get(chunk).copied();
                match (role, before) {
                    (Some(role), None) => {
                        self.heard[index].insert(*chunk, (role, ask, ask));
                    }
                    // A viewer's subscription that is named again is asked again.
                    (Some(Role::Viewer), Some((Role::Viewer, ..))) => {
                        self.heard[index].insert(*chunk, (Role::Viewer, ask, ask));
                        self.again.insert((index, *chunk));
                    }
                    (Some(Role::Guest), Some((Role::Guest, ..))) => self.fail(format!(
                        "{region:?} is asked as a guest for {chunk:?}, which it is asked for as \
                         a guest already"
                    )),
                    // A change of kind: the number it began with stays.
                    (Some(role), Some((_, _, begun))) => {
                        self.heard[index].insert(*chunk, (role, ask, begun));
                    }
                    (None, Some(_)) => {
                        self.heard[index].remove(chunk);
                    }
                    (None, None) => self.fail(format!(
                        "{region:?} is sent Unsubscribe for {chunk:?}, which it is not asked for"
                    )),
                }
            }
            self.played[index].inbox.push_back(message);
        }

        /// Holds the subscriptions the edge's messages make at each region it has a
        /// link to against the ones it has to hold, and takes their numbers.
        fn compare(&mut self) {
            let now = Instant::now();
            for region in ports() {
                if !self.linked(region) {
                    continue;
                }
                let index = region.0 as usize;
                let expected: BTreeMap<_, _> = self.kept[index]
                    .iter()
                    .map(|(chunk, kept)| (*chunk, kept.role))
                    .collect();
                let found: BTreeMap<_, _> = self.heard[index]
                    .iter()
                    .map(|(chunk, heard)| (*chunk, heard.0))
                    .collect();
                if expected != found {
                    let lacking: Vec<_> = expected
                        .iter()
                        .filter(|(chunk, role)| found.get(chunk) != Some(role))
                        .collect();
                    let beyond: Vec<_> = found
                        .iter()
                        .filter(|(chunk, role)| expected.get(chunk) != Some(role))
                        .collect();
                    self.fail(format!(
                        "{region:?} is not asked for {lacking:?}, which the records have it \
                         asked for, and is asked for {beyond:?}"
                    ));
                }
                for (chunk, kept) in &mut self.kept[index] {
                    let (_, ask, begun) = self.heard[index][chunk];
                    kept.ask = ask;
                    kept.begun = begun;
                }
            }
            for (index, chunk) in std::mem::take(&mut self.again) {
                let region = RegionId(index as u32);
                let Some(kept) = self.kept[index].get_mut(&chunk) else {
                    continue;
                };
                if !kept.at_once && !kept.due {
                    self.fail(format!(
                        "{region:?} is asked again for {chunk:?} though nothing called for it"
                    ));
                }
                let was_due = kept.due;
                kept.answer = Answer::Waiting;
                kept.at_once = false;
                kept.due = false;
                kept.asked_again = Some(now);
                if was_due {
                    self.count("an asking that was due is made");
                }
            }
            for region in ports() {
                let late = self.kept[region.0 as usize]
                    .iter()
                    .find(|(_, kept)| kept.at_once);
                if let Some((chunk, _)) = late {
                    self.fail(format!(
                        "{region:?} is not asked again for {chunk:?} when the region it named \
                         has said NotMine"
                    ));
                }
            }
        }

        /// What has to hold after every step.
        async fn check(&mut self) {
            // Statement V: a viewer's subscription exactly where a player of the
            // region sees the chunk.
            for region in ports() {
                let mut seen = BTreeSet::new();
                for person in self.people.values() {
                    if person.region == region {
                        seen.extend(person.wanted.iter().copied());
                    }
                }
                let viewers: BTreeSet<_> = self.kept[region.0 as usize]
                    .iter()
                    .filter(|(_, kept)| kept.role == Role::Viewer)
                    .map(|(chunk, _)| *chunk)
                    .collect();
                if viewers != seen {
                    self.fail(format!(
                        "{region:?} has a viewer's subscription for {:?} without a viewer, and \
                         none for {:?}",
                        minus(&viewers, &seen),
                        minus(&seen, &viewers)
                    ));
                }
            }
            // Statement G, and that everything is ended by the time nobody sees a
            // chunk. Statement E.
            // Statement S: a region that stands for another has no player, no
            // subscription and is nobody's word for who holds a chunk.
            for gone in self.stands_for.keys() {
                let named = |kept: &Kept| kept.answer == Answer::Told(*gone);
                let mut subscriptions = self.kept.iter().flat_map(|kept| kept.values());
                if self.people.values().any(|person| person.region == *gone)
                    || !self.kept[gone.0 as usize].is_empty()
                    || subscriptions.any(named)
                    || self.served_by.values().any(|by| by == gone)
                    || self.stands_for.contains_key(&self.stands_for[gone])
                {
                    self.fail(format!(
                        "the records leave the edge with something of {gone:?}, which is no more"
                    ));
                }
            }
            for region in ports() {
                for (chunk, kept) in &self.kept[region.0 as usize] {
                    if !self.seen(*chunk) {
                        self.fail(format!(
                            "{region:?} is asked for {chunk:?}, which nobody sees"
                        ));
                    }
                    if let Answer::Told(holder) = kept.answer {
                        let there = self.kept[holder.0 as usize].contains_key(chunk);
                        if !there && !kept.due && !kept.at_once {
                            self.fail(format!(
                                "{region:?} named {holder:?} for {chunk:?}, which is not asked \
                                 for it"
                            ));
                        }
                    }
                }
            }

            // What the clients were sent.
            let players: Vec<_> = self.people.keys().copied().collect();
            for player in players {
                let person = self.people.get_mut(&player).expect("the player is there");
                if person.entity.is_some() {
                    self.edge.sync(&mut person.client).await;
                }
                if !person.client.connected() {
                    let reason = person.client.disconnected.clone();
                    self.fail(format!("{player:?} was disconnected: {reason:?}"));
                }
                let person = self.people.get_mut(&player).expect("the player is there");
                let sent = person.client.sent[person.looked_at..].to_vec();
                person.looked_at = person.client.sent.len();
                let person = &self.people[&player];
                for chunk in sent {
                    if !self.replica.contains_key(&chunk) {
                        self.fail(format!(
                            "{player:?} was sent {chunk:?}, of which no snapshot was taken"
                        ));
                    }
                }
                for chunk in &person.wanted {
                    let Some(state) = self.replica.get(chunk) else {
                        continue;
                    };
                    if !person.client.chunks.contains_key(chunk) {
                        self.fail(format!(
                            "{player:?} sees {chunk:?}, of which a snapshot was taken, and was \
                             not sent it"
                        ));
                    }
                    let shown = person.client.state(*chunk);
                    if !self.unsure.contains(chunk) && shown != Some(*state) {
                        self.fail(format!(
                            "{player:?} is shown {chunk:?} with {shown:?}, and the last \
                             snapshot and what followed it have {state:?}"
                        ));
                    }
                }
            }
            for client in &mut self.dropped {
                if client.connected() {
                    let who = client.player;
                    panic!("{who:?} is still connected though their region forgot the edge");
                }
            }
            self.dropped.clear();
        }

        /// Lets everything come to rest: the edge is told what the table says, every
        /// region gets a link, looks at everything and answers everything, and the
        /// edge reads everything, until nothing is left to do. Then everybody has to
        /// be where the edge believes them to be, and has to be shown everything they
        /// see as it is.
        async fn rest(&mut self) {
            self.log.push("everything comes to rest".to_owned());
            for _ in 0..80 {
                let behind = |(gone, _): &(RegionId, RegionId)| !self.stands_for.contains_key(gone);
                if self.absorbed.iter().any(behind) {
                    self.tell_the_table().await;
                }
                for region in self.living() {
                    if !self.linked(region) {
                        self.relink(region).await;
                    }
                }
                self.free(true);
                for region in self.living() {
                    self.tick(region, true);
                    while self.linked(region) && !self.played[region.0 as usize].out.is_empty() {
                        self.deliver(region).await;
                    }
                }
                self.pass_time().await;
                self.check().await;
                let busy = self.played.iter().any(|played| {
                    !played.inbox.is_empty()
                        || !played.out.is_empty()
                        || played
                            .tickets
                            .values()
                            .any(|ticket| ticket.answer == Answer::Waiting)
                });
                let due = self
                    .kept
                    .iter()
                    .flat_map(|subscriptions| subscriptions.values())
                    .any(|kept| kept.due || kept.at_once || kept.answer == Answer::Waiting);
                let unlinked = self.living().into_iter().any(|region| !self.linked(region));
                let behind = |(gone, _): &(RegionId, RegionId)| !self.stands_for.contains_key(gone);
                if !busy && !due && !unlinked && !self.absorbed.iter().any(behind) {
                    self.rested();
                    return;
                }
            }
            self.fail("the run does not come to rest".to_owned());
        }

        fn rested(&mut self) {
            if !self.on_the_way.is_empty() {
                self.fail(format!(
                    "the entities {:?} were let go and neither arrived nor were discarded",
                    self.on_the_way
                ));
            }
            // The edge has caught up with every merge.
            for (gone, _) in &self.absorbed {
                if self.stands_for.get(gone) != Some(&self.now(*gone)) {
                    self.fail(format!(
                        "{gone:?} is {:?} by now, and the records have it stand for {:?}",
                        self.now(*gone),
                        self.stands_for.get(gone)
                    ));
                }
            }
            // No stay is left in a region that the edge does not have there: every
            // region has answered a hello of the edge since.
            for region in self.living() {
                for (player, resident) in &self.played[region.0 as usize].residents {
                    let believed = self.people.get(player).is_some_and(|person| {
                        person.region == region && person.entity == Some(resident.entity)
                    });
                    if !believed {
                        self.fail(format!(
                            "{region:?} has {player:?} as {resident:?}, and the edge does not"
                        ));
                    }
                }
            }
            for (player, person) in &self.people {
                let resident = self.played[person.region.0 as usize].residents.get(player);
                let there = resident.is_some_and(|resident| {
                    resident.chunk == person.centre && resident.chunk == person.target
                });
                if !there {
                    self.fail(format!(
                        "{player:?} is {resident:?} in {:?}, and the edge has them in {:?}, \
                         where their last step was into {:?}",
                        person.region, person.centre, person.target
                    ));
                }
                // They are shown the others who stand where they see, and none of the
                // others who do not. Someone who has left can still be shown; see
                // `a_discarded_entity_another_region_showed_last_is_taken_off_the_screens`.
                for (other, there) in &self.people {
                    let Some(entity) = there.entity else {
                        continue;
                    };
                    let seen = other != player && person.wanted.contains(&there.centre);
                    if person.client.entities.contains(&entity.0) != seen {
                        self.fail(format!(
                            "{player:?} is shown the entities {:?}; {other:?} is {entity:?} and \
                             stands in {:?}, which they see: {seen}",
                            person.client.entities, there.centre
                        ));
                    }
                }
                for chunk in &person.wanted {
                    let holder = self.holder(*chunk);
                    let served = holder.is_some_and(|holder| {
                        self.kept[holder.0 as usize]
                            .get(chunk)
                            .is_some_and(|kept| kept.answer == Answer::Served)
                    });
                    let shown = person.client.state(*chunk);
                    if !served || self.unsure.contains(chunk) || shown != Some(self.state(*chunk)) {
                        self.fail(format!(
                            "{player:?} sees {chunk:?}, which {holder:?} holds with {:?}: \
                             served {served}, shown {shown:?}",
                            self.state(*chunk)
                        ));
                    }
                }
            }
        }

        async fn play(mut self, steps: usize) -> BTreeMap<&'static str, u64> {
            for step in 1..=steps {
                self.step().await;
                if step % 150 == 0 {
                    self.rest().await;
                }
            }
            self.rest().await;
            // Statement S: the edge takes no link to a region that is no more.
            for (gone, _) in self.absorbed.clone() {
                self.edge.offers_in_vain(gone).await;
            }
            self.sync().await;
            std::mem::take(&mut self.tally)
        }
    }

    /// The seeds of the generated runs a test makes: five, or those asked for with the
    /// environment variable named, as `<first seed>,<how many>`.
    fn runs_asked_for(variable: &str) -> (Option<(u64, u64)>, std::ops::Range<u64>) {
        let asked = std::env::var(variable).ok().and_then(|asked| {
            let (first, runs) = asked.split_once(',')?;
            Some((first.parse::<u64>().ok()?, runs.parse::<u64>().ok()?))
        });
        let (first, runs) = asked.unwrap_or((0, 5));
        (asked, first..first + runs)
    }

    /// Scenario 20. The clock of this test stands still until a run moves it.
    ///
    /// Other runs than the five it makes can be asked for with
    /// `CLUSTINE_EDGE_RUNS=<first seed>,<how many>`; four hundred take some minutes.
    /// A run that fails says its seed and its last steps, and all of its steps with
    /// `CLUSTINE_WHOLE_RUN` set.
    #[tokio::test(start_paused = true)]
    async fn the_edge_holds_what_the_records_say_after_every_step_of_generated_runs() {
        let (asked, seeds) = runs_asked_for("CLUSTINE_EDGE_RUNS");
        let mut tally: BTreeMap<&'static str, u64> = BTreeMap::new();
        for seed in seeds {
            for (what, times) in Run::new(seed, false).await.play(500).await {
                *tally.entry(what).or_default() += times;
            }
        }
        // Shown when the test fails, or to whoever asks for the test's output.
        println!("{tally:#?}");
        if asked.is_some() {
            return;
        }
        // The runs are worth something only if they come to the cases they are for.
        for what in [
            "a hand-over",
            "a hello with guest's subscriptions",
            "a link is replaced while it stands",
            "a not mine taken",
            "a served viewer's becomes a guest's",
            "a snapshot below the number its subscription began with",
            "a snapshot taken under an earlier number",
            "a viewer's that waits becomes a guest's",
            "an asking again at once",
            "an elsewhere passed over",
            "an elsewhere that makes a guest's",
            "one told elsewhere ends, others see it",
        ] {
            assert!(tally.contains_key(what), "no run came to {what}");
        }
    }

    /// Scenario 32 of ADR-0015: the generated runs of scenario 20 with regions that
    /// merge and split while links are lost, played by a model of ADR-0014, sections
    /// 2.1, 2.3, 2.4, 3.3 and 3.7. A merge and a split close the region's links; a
    /// welcome has the entries of the outbox, `Absorbed` with the absorbed region's
    /// entries behind it and `SplitOff` among them, says how far the region had
    /// applied, and is followed by an answer for every stay the region has; an input
    /// and a leave are for the stay they name. The edge is told what the routing
    /// table says of absorbed regions at moments of the run's choosing.
    ///
    /// Beside the edge runs what ADR-0015 says it holds, which is checked as in
    /// scenario 20, with statement S. At rest everyone is in the region whose state
    /// has their stay, no region has a stay the edge does not have there, and the
    /// edge takes no link to a region that is no more. Nobody is disconnected: no
    /// region of these runs forgets the edge, so nobody has done anything for it.
    ///
    /// Other runs than the five it makes can be asked for with
    /// `CLUSTINE_EDGE_RESHAPES=<first seed>,<how many>`.
    #[tokio::test(start_paused = true)]
    async fn the_edge_holds_what_the_records_say_while_regions_merge_and_split() {
        let (asked, seeds) = runs_asked_for("CLUSTINE_EDGE_RESHAPES");
        let mut tally: BTreeMap<&'static str, u64> = BTreeMap::new();
        for seed in seeds {
            for (what, times) in Run::new(seed, true).await.play(600).await {
                *tally.entry(what).or_default() += times;
            }
        }
        println!("{tally:#?}");
        if asked.is_some() {
            return;
        }
        for what in [
            "a merge",
            "a split",
            "a merge brings a player",
            "a present moves a stay",
            "a split off moves a stay",
            "an absorbed is handled",
            "the table is told",
        ] {
            assert!(tally.contains_key(what), "no run came to {what}");
        }
    }

    // The edge through merges and splits: ADR-0015, section 9, and ADR-0014, section 10.
    //
    // These tests were written from those two records, ADR-0013 and section 5 of
    // ADR-0012, without the edge's code. A merge and a split reach the edge only among
    // the entries of a welcome, so the regions of these tests answer hellos: a `Reply`
    // says what a region has for the edge, and the welcome, its entries and the
    // presence answers follow from it and from the hello as ADR-0014, section 3.7, has
    // it. Where two regions answer at the same time, a scenario is a script per link,
    // and it is run with the scripts' messages in every order that keeps each script's
    // own (`Orders`): the edge reads its links in no order between them.
    //
    // `settle` cannot be used between a welcome's entries, as it makes an entry of its
    // own, which the welcome did not announce and which would stand where an entry
    // that came with a merge is expected. So these tests have a witness: a player who
    // is kept aside in a region without a link and takes no part. A region says to the
    // witness that an action was handled, which the edge passes on whatever region
    // says it, and the test waits for that: the edge has then read everything the
    // region said before.

    /// The player who is the witness of `Harness::witnessed`.
    const WITNESS: u128 = 999;
    /// Where a player stands whom these tests hand to the southern region.
    const SOUTHERN: ChunkPos = ChunkPos::new(0, 4);
    /// Two chunks that are seen from `HOME` and neither from `NORTHERN` nor from
    /// `SOUTHERN`.
    const LENT: ChunkPos = ChunkPos::new(-1, 0);
    const SOUGHT: ChunkPos = ChunkPos::new(1, 0);

    /// A region's word that the player is in it as `entity`, with their inputs up to
    /// `last_input` applied.
    fn present_with(player: PlayerId, entity: EntityId, last_input: u64) -> WorkerToEdge {
        WorkerToEdge::Presence {
            player,
            answer: Presence::Present {
                entity,
                pose: Pose::at(SPAWN),
                hotbar: [None; HOTBAR_SLOTS],
                selected_slot: 0,
                last_input,
                handled: None,
            },
        }
    }

    fn absent(player: PlayerId) -> WorkerToEdge {
        WorkerToEdge::Presence {
            player,
            answer: Presence::Absent,
        }
    }

    fn progress(applied: u64, inputs: Vec<(PlayerId, u64)>) -> WorkerToEdge {
        WorkerToEdge::Progress { applied, inputs }
    }

    /// The outbox entry of a region that has absorbed `region`.
    fn absorbed(region: RegionId, since: u64, applied: u64, numbers: &[u64]) -> Durable {
        Durable::Absorbed {
            region,
            since,
            applied,
            numbers: numbers.to_vec(),
        }
    }

    /// The outbox entry of a region of which `region` was split off with `stays`.
    fn split_off(region: RegionId, stays: &[(u128, i32)]) -> Durable {
        let stay = |(who, entity): &(u128, i32)| (player(*who), EntityId(*entity));
        Durable::SplitOff {
            region,
            players: stays.iter().map(stay).collect(),
        }
    }

    /// The outbox entry that says an action of `player` was dealt with.
    fn done(player: PlayerId, sequence: i32) -> Durable {
        Durable::RemoteDone { player, sequence }
    }

    /// An input as the edge passes it on.
    fn passed_on(who: u128, entity: i32, number: u64, input: PlayerInput) -> EdgeToWorker {
        EdgeToWorker::Input {
            player: player(who),
            entity: EntityId(entity),
            number,
            input,
        }
    }

    /// The word that a player's stay has ended.
    fn left(who: u128, entity: Option<i32>) -> EdgeToWorker {
        EdgeToWorker::PlayerLeave {
            player: player(who),
            entity: entity.map(EntityId),
        }
    }

    /// The chunks that subscription messages among `said` ask for as `role` before the
    /// first numbered message.
    fn asked_before_numbered(said: &[EdgeMessage], role: Role) -> BTreeSet<ChunkPos> {
        let mut asked = BTreeSet::new();
        for message in said {
            if message.number.is_some() {
                break;
            }
            if is_about_subscriptions(&message.body) {
                let (kind, chunks) = meaning(&message.body);
                if kind == Some(role) {
                    asked.extend(chunks);
                }
            }
        }
        asked
    }

    /// The inputs of `who` among `said`: each with the number of its message, the
    /// entity it names and its own number.
    fn inputs_of(said: &[EdgeMessage], who: u128) -> Vec<(u64, EntityId, u64)> {
        let mut inputs = Vec::new();
        for message in said {
            if let EdgeToWorker::Input {
                player: actor,
                entity,
                number,
                ..
            } = &message.body
                && *actor == player(who)
            {
                let numbered = message.number.expect("an input is numbered");
                inputs.push((numbered, *entity, *number));
            }
        }
        inputs
    }

    /// The arrival of `who` among `said`, if there is one: its number and the player
    /// as they arrive.
    fn arrival_of(said: &[EdgeMessage], who: u128) -> Option<(u64, PlayerTransfer)> {
        said.iter().find_map(|message| match &message.body {
            EdgeToWorker::PlayerArrive {
                player: arriving,
                transfer,
            } if *arriving == player(who) => Some((message.number?, transfer.clone())),
            _ => None,
        })
    }

    /// What a region has for the edge when it answers a hello, from which the
    /// welcome, its entries and the presence answers follow.
    #[derive(Debug, Clone, Default)]
    struct Reply {
        /// The region's word for its numbering, if it does not share the one the
        /// hello named: it then says `Unknown`, and its outbox is numbered from 1.
        unknown: Option<u64>,
        /// The number of the edge's last message the region had applied.
        applied: u64,
        /// The entries of its outbox the edge has not confirmed, in order.
        entries: Vec<Durable>,
        /// The stays it has for the edge, each with the number of the last input
        /// applied.
        stays: Vec<(PlayerId, EntityId, u64)>,
    }

    impl Reply {
        fn resumed() -> Self {
            Self::default()
        }

        fn unknown(since: u64) -> Self {
            Self {
                unknown: Some(since),
                ..Self::default()
            }
        }

        fn applied(mut self, applied: u64) -> Self {
            self.applied = applied;
            self
        }

        fn entry(mut self, entry: Durable) -> Self {
            self.entries.push(entry);
            self
        }

        fn stay(mut self, who: u128, entity: i32, last_input: u64) -> Self {
            self.stays.push((player(who), EntityId(entity), last_input));
            self
        }

        /// The presence answers, by ADR-0014, section 3.7: one for each player the
        /// hello named, in its order, and then `Present` for every other stay, in
        /// ascending order of the players.
        fn presences(&self, named: &[PlayerId]) -> Vec<WorkerToEdge> {
            let stay = |who: &PlayerId| self.stays.iter().find(|(player, ..)| player == who);
            let mut answers = Vec::new();
            for who in named {
                answers.push(match stay(who) {
                    Some((_, entity, last_input)) => present_with(*who, *entity, *last_input),
                    None => absent(*who),
                });
            }
            let mut others: Vec<_> = self.stays.iter().collect();
            others.retain(|(who, ..)| !named.contains(who));
            others.sort();
            for (who, entity, last_input) in others {
                answers.push(present_with(*who, *entity, *last_input));
            }
            answers
        }
    }

    /// Something that happens to the edge in a scenario. A script is a sequence of
    /// these for one link, or for the routing table.
    #[derive(Debug, Clone)]
    enum Step {
        /// The edge is handed a new link to the region and says hello on it.
        Link(RegionId),
        /// The region answers the hello with its welcome. The entries and the
        /// presence answers follow as steps of their own, so that what another script
        /// says can fall between any two of them.
        Welcome(RegionId, Reply),
        /// The region says the next entry of its outbox.
        Entry(RegionId, Durable),
        /// The region says something else, which the edge has read when the step is
        /// over.
        Says(RegionId, WorkerToEdge),
        /// The edge's process tells it which regions the routing table says were
        /// absorbed.
        Pairs(Vec<(RegionId, RegionId)>),
    }

    /// How many orders of a scenario's steps are run one after the other before the
    /// rest is left to chance, and how many are then drawn.
    const EVERY_ORDER_UP_TO: usize = 600;
    const ORDERS_DRAWN: usize = 400;

    /// The orders in which the steps of a scenario's scripts are taken: every order
    /// that keeps each script's own, one per run, or, where there are too many, the
    /// first few hundred and as many again drawn by a generator of numbers. A scenario
    /// is run anew for each, as an edge cannot be put back to where it was. Scripts
    /// can grow while they run (a welcome is followed by as many presence answers as
    /// its hello named players), so the orders are found by walking: each run goes as
    /// the one before up to the last choice that has another way left, and takes that.
    struct Orders {
        /// The choices of the run that is under way, or was made last: which of how
        /// many scripts went next.
        path: Vec<(usize, usize)>,
        at: usize,
        runs: usize,
        begun: bool,
        /// Set once the orders are drawn.
        dice: Option<Dice>,
        /// The steps of the run that is under way, for whoever reads a failure.
        account: Vec<String>,
    }

    impl Orders {
        fn new() -> Self {
            Self {
                path: Vec::new(),
                at: 0,
                runs: 0,
                begun: false,
                dice: None,
                account: Vec::new(),
            }
        }

        /// Whether there is another order to run, which is then the one `choose`
        /// gives.
        fn another(&mut self) -> bool {
            self.account.clear();
            self.at = 0;
            if !self.begun {
                self.begun = true;
                return true;
            }
            self.runs += 1;
            if self.dice.is_some() {
                let more = self.runs < EVERY_ORDER_UP_TO + ORDERS_DRAWN;
                if !more {
                    println!("{} orders were run, the later ones drawn", self.runs);
                }
                return more;
            }
            while let Some((chosen, of)) = self.path.pop() {
                if chosen + 1 < of {
                    self.path.push((chosen + 1, of));
                    if self.runs >= EVERY_ORDER_UP_TO {
                        self.path.clear();
                        self.dice = Some(Dice(self.runs as u64));
                    }
                    return true;
                }
            }
            // Shown to whoever asks for the test's output.
            println!("every order was run: {}", self.runs);
            false
        }

        /// Which of `of` scripts goes next.
        fn choose(&mut self, of: usize) -> usize {
            if of == 1 {
                return 0;
            }
            if let Some(dice) = &mut self.dice {
                return dice.below(of);
            }
            if self.at == self.path.len() {
                self.path.push((0, of));
            }
            let (chosen, was) = self.path[self.at];
            assert_eq!(was, of, "a scenario went another way under the same order");
            self.at += 1;
            chosen
        }
    }

    impl Drop for Orders {
        fn drop(&mut self) {
            if std::thread::panicking() {
                eprintln!(
                    "run {} of the scenario, with its steps in this order:\n{}",
                    self.runs + 1,
                    self.account.join("\n")
                );
            }
        }
    }

    impl Harness {
        /// An edge as `start` gives it, and a witness: a player whom the west has
        /// placed and let go to a region the edge is never given a link to, far from
        /// everything, where they see nothing that anyone else sees. The west has
        /// applied their join, so nothing is kept for it.
        async fn witnessed() -> Self {
            let mut edge = Self::start().await;
            let who = player(WITNESS);
            let mut witness = edge.joined(who, EntityId(900)).await;
            let far = ChunkPos::new(100, 100);
            edge.say(WEST, departed(who, EntityId(900), ASIDE, far));
            edge.caught_up(WEST);
            edge.quiet().await;
            edge.sync(&mut witness).await;
            for region in REGIONS {
                let left = &edge.at(region).subscriptions;
                assert!(left.is_empty(), "{region:?} is still asked for {left:?}");
            }
            edge.witness = Some(witness);
            edge
        }

        /// The region says that it has applied every numbered message it was sent on
        /// its link.
        fn caught_up(&mut self, region: RegionId) {
            let applied = self.at(region).numbered;
            self.tell(region, progress(applied, Vec::new()));
        }

        /// Waits until the edge has read everything `region` has said so far, or has
        /// closed its link to the region, which is then noted in `gone`. It makes no
        /// entry, so it can be used between the entries of a welcome. What the edge
        /// said to the region meanwhile is kept for the test to look at.
        async fn handled(&mut self, region: RegionId) {
            let index = region.0 as usize;
            let sequence = self.sequence();
            let Some(link) = self.links[index].as_ref() else {
                return;
            };
            let asked = WorkerToEdge::ToPlayer {
                player: player(WITNESS),
                event: PlayerEvent::Acknowledged { sequence },
            };
            if link.try_send(asked).is_err() {
                self.links[index] = None;
                self.gone.insert(region);
                return;
            }
            loop {
                let witness = self.witness.as_mut().expect("the test has a witness");
                if witness.acknowledged.contains(&sequence) {
                    return;
                }
                let link = self.links[index].as_mut().expect("the region has a link");
                tokio::select! {
                    biased;
                    ended = &mut self.task => panic!("the edge's task ended: {ended:?}"),
                    packet = timeout(SOON, witness.packets.recv()) => {
                        let packet = packet
                            .unwrap_or_else(|_| panic!("the edge does not read what {region:?} says"))
                            .expect("the edge ended the witness's connection");
                        witness.take(packet);
                    }
                    message = link.recv() => match message {
                        Some(message) => {
                            self.heard[index].note(region, &message);
                            self.heard[index].said.push_back(message);
                        }
                        None => {
                            self.links[index] = None;
                            self.gone.insert(region);
                            return;
                        }
                    },
                }
            }
        }

        /// Says something as `region`, if its link is there and the edge has not
        /// closed it, and waits until the edge has read it. A region whose link the
        /// edge has closed says it into nothing, as a real one would.
        async fn says(&mut self, region: RegionId, message: WorkerToEdge) {
            let index = region.0 as usize;
            // A region that says `NotMine` to a guest has ended that subscription.
            if let WorkerToEdge::NotMine { chunk, ask } = &message {
                let subscriptions = &mut self.heard[index].subscriptions;
                if subscriptions.get(chunk) == Some(&(Role::Guest, *ask)) {
                    subscriptions.remove(chunk);
                }
            }
            let Some(link) = self.links[index].as_ref() else {
                return;
            };
            if link.try_send(message).is_ok() {
                self.handled(region).await;
            }
        }

        /// Everything the edge has said to `region` that no test has looked at, once
        /// it has read what the region has said so far. Unlike `said` it makes no
        /// entry.
        async fn sent(&mut self, region: RegionId) -> Vec<EdgeMessage> {
            self.handled(region).await;
            self.waiting(region)
        }

        /// Tells the edge which regions the routing table says were absorbed, and
        /// waits until it has taken that.
        async fn pairs(&mut self, pairs: Vec<(RegionId, RegionId)>) {
            assert!(self.relinks.absorbed(pairs).await, "the edge is gone");
            let sender = &self.relinks.sender;
            let taken = async {
                while sender.capacity() < sender.max_capacity() {
                    tokio::task::yield_now().await;
                }
            };
            timeout(SOON, taken)
                .await
                .expect("the edge takes what the routing table says");
        }

        /// Waits until the edge has closed its link to `region` by itself and has
        /// said so to whoever gives it links. What it said on the link before is
        /// kept for the test to look at.
        async fn ended_by_the_edge(&mut self, region: RegionId) {
            let index = region.0 as usize;
            if !self.gone.remove(&region) {
                loop {
                    let link = self.links[index].as_mut().expect("the region has a link");
                    let message = tokio::select! {
                        biased;
                        ended = &mut self.task => panic!("the edge's task ended: {ended:?}"),
                        message = timeout(SOON, link.recv()) => message,
                    };
                    let message = message
                        .unwrap_or_else(|_| panic!("the edge did not end its link to {region:?}"));
                    let Some(message) = message else {
                        break;
                    };
                    self.heard[index].note(region, &message);
                    self.heard[index].said.push_back(message);
                }
                self.links[index] = None;
            }
            let word = (region, self.epochs[index]);
            if let Some(at) = self.ended.iter().position(|ended| *ended == word) {
                self.ended.remove(at);
                return;
            }
            loop {
                let ended = timeout(SOON, self.relinks.ended())
                    .await
                    .unwrap_or_else(|_| {
                        panic!("the edge did not say that its link to {region:?} ended")
                    })
                    .expect("the edge is there");
                if ended == word {
                    return;
                }
                self.ended.push(ended);
            }
        }

        /// Hands the edge a link to a region that is no more, and makes sure that the
        /// edge drops it without a hello.
        async fn offers_in_vain(&mut self, region: RegionId) {
            let index = region.0 as usize;
            self.epochs[index] += 1;
            let (link, mut worker) = link_to(region, self.epochs[index]);
            assert!(self.relinks.replace(link).await);
            let said = tokio::select! {
                biased;
                ended = &mut self.task => panic!("the edge's task ended: {ended:?}"),
                said = timeout(SOON, worker.recv()) => said,
            };
            let said = said.unwrap_or_else(|_| {
                panic!("the edge keeps a link to {region:?}, which is no more")
            });
            assert!(
                said.is_none(),
                "the edge said {said:?} to {region:?}, which is no more"
            );
        }

        /// The edge is handed a new link to `region`; its hello is kept for `hello`.
        async fn link(&mut self, region: RegionId) {
            let hello = self.relink(region).await;
            self.hellos[region.0 as usize] = Some(hello);
        }

        /// The hello the edge said to `region` on the last link `link` made.
        fn hello(&self, region: RegionId) -> &Hello {
            let hello = self.hellos[region.0 as usize].as_ref();
            hello.expect("the edge was given a link to the region")
        }

        /// Takes one step of a script, and returns the steps that follow from it and
        /// come before the rest of its script.
        async fn take(&mut self, step: Step) -> Vec<Step> {
            match step {
                Step::Link(region) => {
                    self.link(region).await;
                    Vec::new()
                }
                Step::Welcome(region, reply) => {
                    let index = region.0 as usize;
                    let named = match &self.hellos[index] {
                        Some(hello) => hello.players.clone(),
                        None => Vec::new(),
                    };
                    let presences = reply.presences(&named);
                    let (entries, answers) = (reply.entries.len() as u32, presences.len() as u32);
                    let welcome = match reply.unknown {
                        Some(since) => {
                            // Its outbox begins anew for an edge it does not share a
                            // numbering with.
                            self.outbox[index] = 0;
                            Welcome::Unknown {
                                since,
                                entries,
                                presences: answers,
                                applied: reply.applied,
                            }
                        }
                        None => Welcome::Resumed {
                            entries,
                            presences: answers,
                            applied: reply.applied,
                        },
                    };
                    self.says(region, WorkerToEdge::Welcome(welcome)).await;
                    let entries = reply.entries.into_iter();
                    let mut follow: Vec<_> =
                        entries.map(|entry| Step::Entry(region, entry)).collect();
                    let answers = presences.into_iter();
                    follow.extend(answers.map(|answer| Step::Says(region, answer)));
                    follow
                }
                Step::Entry(region, entry) => {
                    let number = &mut self.outbox[region.0 as usize];
                    *number += 1;
                    let number = *number;
                    self.says(region, WorkerToEdge::Outbox { number, entry })
                        .await;
                    Vec::new()
                }
                Step::Says(region, message) => {
                    self.says(region, message).await;
                    Vec::new()
                }
                Step::Pairs(pairs) => {
                    self.pairs(pairs).await;
                    Vec::new()
                }
            }
        }

        /// Runs the scripts with their steps in the order `orders` gives.
        async fn play(&mut self, scripts: Vec<Vec<Step>>, orders: &mut Orders) {
            let mut scripts: Vec<VecDeque<Step>> =
                scripts.into_iter().map(VecDeque::from).collect();
            loop {
                let ready: Vec<_> = (0..scripts.len())
                    .filter(|script| !scripts[*script].is_empty())
                    .collect();
                if ready.is_empty() {
                    return;
                }
                let script = ready[orders.choose(ready.len())];
                let step = scripts[script].pop_front().expect("the script has a step");
                let mut told = format!("{step:?}");
                if told.len() > 400 {
                    told.truncate(400);
                }
                orders.account.push(told);
                for follows in self.take(step).await.into_iter().rev() {
                    scripts[script].push_front(follows);
                }
            }
        }

        /// The region answers the hello of its link: the welcome, its entries and
        /// the presence answers, one after the other.
        async fn answer(&mut self, region: RegionId, reply: Reply) {
            let script = vec![Step::Welcome(region, reply)];
            self.play(vec![script], &mut Orders::new()).await;
        }

        /// A player who has entered the world as `entity` and whom the west, unless
        /// it is to be their region, has let go to `region`, into `chunk`. The west
        /// has applied their join, and everything else is as the hand-over left it.
        async fn settler(
            &mut self,
            who: u128,
            entity: i32,
            region: RegionId,
            chunk: ChunkPos,
        ) -> Client {
            let client = self.joined(player(who), EntityId(entity)).await;
            self.caught_up(WEST);
            if region != WEST {
                self.say(WEST, departed(player(who), EntityId(entity), region, chunk));
            }
            self.quiet().await;
            client
        }

        /// The player makes a step. Returns the region the edge passes it on to, the
        /// entity it names and its number among the player's inputs. That region has
        /// to have a link.
        async fn acts(&mut self, client: &Client) -> (RegionId, EntityId, u64) {
            self.input(client, step_into(HOME)).await;
            self.drained().await;
            for region in self.linked() {
                self.handled(region).await;
            }
            // Without the links the edge has closed, which the waiting showed.
            let mut found = Vec::new();
            for region in self.linked() {
                for message in self.waiting(region) {
                    if let EdgeToWorker::Input {
                        player,
                        entity,
                        number,
                        ..
                    } = message.body
                        && player == client.player
                    {
                        found.push((number, region, entity));
                    }
                }
            }
            // What they did before can have been sent again; this is their last.
            found.sort();
            let (number, region, entity) = found
                .pop()
                .unwrap_or_else(|| panic!("no region was sent what {:?} did", client.player));
            (region, entity, number)
        }

        /// An action of `who` about `COMMON` that the west passes on to `to`, where it
        /// is under way when this returns.
        async fn under_way(&mut self, who: u128, to: RegionId) -> RemoteAction {
            let action = breaking(player(who), self.sequence(), COMMON);
            self.say(WEST, remote(&action, Some(to)));
            self.settle(WEST).await;
            action
        }

        /// The player makes `steps` steps, which the edge has taken when this
        /// returns.
        async fn walks(&mut self, client: &Client, chunk: ChunkPos, steps: usize) {
            for _ in 0..steps {
                self.input(client, step_into(chunk)).await;
            }
            self.drained().await;
        }
    }

    /// The subscription messages among `said`.
    fn subscriptions(said: Vec<EdgeMessage>) -> Vec<EdgeToWorker> {
        let bodies = said.into_iter().map(|message| message.body);
        bodies.filter(is_about_subscriptions).collect()
    }

    /// The highest number among the entries the edge has confirmed in `said`.
    fn confirmed(said: &[EdgeMessage]) -> Option<u64> {
        let confirmations = said.iter().filter_map(|message| match message.body {
            EdgeToWorker::Confirm { number } => Some(number),
            _ => None,
        });
        confirmations.max()
    }

    /// Scenario 1. A region says whom it has, also those the hello did not name. A stay
    /// the edge does not have is the region's to end, with the entity the region said.
    #[tokio::test]
    async fn a_stay_the_edge_does_not_have_is_ended_at_the_region_that_says_it_has_it() {
        let mut edge = Harness::witnessed().await;
        let mut first = edge.settler(1, 5, WEST, HOME).await;
        let applied = edge.at(WEST).numbered;
        edge.link(WEST).await;
        assert_eq!(edge.hello(WEST).players, [player(1)]);
        let reply = Reply::resumed().applied(applied);
        edge.answer(WEST, reply.stay(1, 5, 0).stay(2, 9, 0)).await;
        let said = edge.sent(WEST).await;
        assert_eq!(numbered(&said), [(applied + 1, left(2, Some(9)))]);
        let asked = subscriptions(said);
        assert!(asked.is_empty(), "{asked:?}");
        for region in [EAST, NORTH] {
            let said = edge.sent(region).await;
            assert!(said.is_empty(), "{region:?} was sent {said:?}");
        }
        assert!(first.connected());
        assert_eq!(edge.acts(&first).await, (WEST, EntityId(5), 1));
        edge.end().await;
    }

    /// Scenario 1, and the rest of case 4 of section 2.1: the edge has the player, as
    /// another entity under another region, or under another region without an entity
    /// yet. The region's stay is ended, and the edge's own is untouched.
    #[tokio::test]
    async fn a_stay_of_a_player_the_edge_has_otherwise_is_ended_and_the_edges_own_is_untouched() {
        let mut edge = Harness::witnessed().await;
        let mut second = edge.settler(2, 6, EAST, EASTERN).await;
        // The third has joined, and the west has not placed them yet.
        let mut third = edge.join(player(3)).await;
        let (_, join) = edge.next_numbered(WEST).await;
        assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
        edge.quiet().await;

        edge.link(NORTH).await;
        assert!(edge.hello(NORTH).players.is_empty());
        let reply = Reply::resumed().stay(2, 9, 0).stay(3, 4, 0);
        edge.answer(NORTH, reply).await;
        let said = edge.sent(NORTH).await;
        let expected = [(1, left(2, Some(9))), (2, left(3, Some(4)))];
        assert_eq!(numbered(&said), expected);
        let asked = subscriptions(said);
        assert!(asked.is_empty(), "{asked:?}");

        assert!(second.connected());
        assert_eq!(edge.acts(&second).await, (EAST, EntityId(6), 1));
        edge.says(WEST, spawned(player(3), EntityId(7))).await;
        assert!(third.connected());
        assert_eq!(edge.acts(&third).await, (WEST, EntityId(7), 1));
        edge.end().await;
    }

    /// Scenario 2. A region that has a player as another entity than the edge has
    /// them under that very region has a stay the edge does not have: that stay is
    /// ended, and the player is not disconnected for it.
    #[tokio::test]
    async fn a_player_a_region_has_as_another_entity_is_not_disconnected() {
        let mut edge = Harness::witnessed().await;
        let mut client = edge.settler(1, 5, WEST, HOME).await;
        let applied = edge.at(WEST).numbered;
        edge.link(WEST).await;
        assert_eq!(edge.hello(WEST).players, [player(1)]);
        let reply = Reply::resumed().applied(applied).stay(1, 77, 0);
        edge.answer(WEST, reply).await;
        let said = edge.sent(WEST).await;
        assert_eq!(numbered(&said), [(applied + 1, left(1, Some(77)))]);
        assert!(client.connected());
        assert_eq!(edge.acts(&client).await, (WEST, EntityId(5), 1));
        edge.end().await;
    }

    /// Scenario 2. A player the hello named and the region does not have is
    /// disconnected, with a leave that names their entity.
    #[tokio::test]
    async fn a_player_a_region_says_it_does_not_have_is_disconnected() {
        let mut edge = Harness::witnessed().await;
        let mut client = edge.settler(1, 5, WEST, HOME).await;
        let applied = edge.at(WEST).numbered;
        edge.link(WEST).await;
        edge.answer(WEST, Reply::resumed().applied(applied)).await;
        client.disconnected().await;
        let said = edge.sent(WEST).await;
        assert_eq!(numbered(&said), [(applied + 1, left(1, Some(5)))]);
        edge.end().await;
    }

    /// Scenarios 2 and 6. A player who is entering the world is not disconnected by
    /// `Absent` while their join is kept: the region has yet to apply it. Whether it
    /// is kept, the welcome says before anything else is done: a join at or below its
    /// `applied` is dropped, and `Absent` then finds none.
    #[tokio::test]
    async fn a_player_who_is_absent_stays_only_while_their_join_is_kept() {
        for applied_the_join in [false, true] {
            let mut edge = Harness::witnessed().await;
            let mut client = edge.join(player(1)).await;
            let (number, join) = edge.next_numbered(WEST).await;
            assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
            edge.lose(WEST).await;
            edge.link(WEST).await;
            assert_eq!(edge.hello(WEST).players, [player(1)]);
            let applied = if applied_the_join { number } else { number - 1 };
            edge.answer(WEST, Reply::resumed().applied(applied)).await;
            if applied_the_join {
                // They quit, as far as any region can tell: no entity was told.
                client.disconnected().await;
                let said = edge.sent(WEST).await;
                assert_eq!(numbered(&said), [(number + 1, left(1, None))]);
            } else {
                let sent = numbered(&edge.sent(WEST).await);
                assert!(
                    matches!(&sent[..], [(again, EdgeToWorker::PlayerJoin(_))] if *again == number),
                    "{sent:?}"
                );
                assert!(client.connected());
                edge.says(WEST, spawned(player(1), EntityId(5))).await;
                assert_eq!(edge.acts(&client).await, (WEST, EntityId(5), 1));
            }
            edge.end().await;
        }
    }

    /// Scenario 2. A player who is on their way to a region is not disconnected by its
    /// `Absent`: their arrival is among what is kept for it.
    #[tokio::test]
    async fn a_player_who_is_absent_stays_while_their_arrival_is_kept() {
        let mut edge = Harness::witnessed().await;
        let mut client = edge.settler(1, 5, EAST, EASTERN).await;
        edge.link(EAST).await;
        assert_eq!(edge.hello(EAST).players, [player(1)]);
        edge.answer(EAST, Reply::resumed()).await;
        let arrival = EdgeToWorker::PlayerArrive {
            player: player(1),
            transfer: transfer(EntityId(5), 0, EASTERN),
        };
        assert_eq!(numbered(&edge.sent(EAST).await), [(1, arrival)]);
        assert!(client.connected());
        assert_eq!(edge.acts(&client).await, (EAST, EntityId(5), 1));
        edge.end().await;
    }

    /// What scenario 3 begins with: the west's player has made three steps, of which
    /// the west has said nothing, and the west has named the east for `COMMON`. Then
    /// the east answers a hello that named nobody with a `Present` for that stay,
    /// with the first of the three steps applied. Returns what the east was sent.
    async fn a_stay_another_region_has() -> (Harness, Client, Vec<EdgeMessage>) {
        let mut edge = Harness::witnessed().await;
        let client = edge.settler(1, 5, WEST, HOME).await;
        edge.tell(WEST, elsewhere(COMMON, edge.ask(WEST, COMMON), EAST));
        edge.walks(&client, HOME, 3).await;
        edge.quiet().await;
        assert_eq!(edge.at(EAST).chunks(Role::Guest), set(&[COMMON]));

        edge.link(EAST).await;
        let hello = edge.hello(EAST);
        assert!(hello.players.is_empty() && hello.chunks.is_empty());
        assert_eq!(hello.guests, [COMMON]);
        edge.answer(EAST, Reply::resumed().stay(1, 5, 1)).await;
        let said = edge.sent(EAST).await;
        (edge, client, said)
    }

    /// Scenario 3, as far as it is about what the stay's new region is sent that is
    /// numbered: the inputs above the answer's `last_input`, and no others.
    #[tokio::test]
    async fn a_stay_another_region_says_it_has_is_sent_only_the_inputs_above_the_answers_last() {
        // The sequence: a player of the west makes three steps, kept as inputs 1 to
        // 3; the east says `Present { last_input: 1 }` for that stay in answer to a
        // hello that named nobody.
        //
        // The records: ADR-0015, section 2.1, case 3 ("the kept inputs up to
        // `last_input` are dropped; ... then every input still kept is sent to
        // `R`"), its scenario 3 ("the inputs above `last_input` go to `R`"), and
        // ADR-0014, rule 38 ("every input of it the edge keeps above the answer's
        // `last_input` is sent here").
        //
        // What happened: the east is sent the inputs 1, 2 and 3; with `last_input`
        // 3 it is sent all three as well. The region passes over what is not above
        // the stay's last (ADR-0014, section 2.1), so nothing is applied twice.
        let (edge, _client, said) = a_stay_another_region_has().await;
        let step = step_into(HOME);
        let expected = [
            (1, passed_on(1, 5, 2, step.clone())),
            (2, passed_on(1, 5, 3, step)),
        ];
        assert_eq!(numbered(&said), expected);
        edge.end().await;
    }

    /// Scenario 3. A region says `Present` for a stay the edge has under another: the
    /// stay is that region's, by a merge or a split the edge has not caught up with.
    /// Nobody arrives; the view is asked of the region before anything the player did
    /// is sent there; and the region they were under keeps, as a guest's, what it was
    /// asked for and had not said another holds.
    #[tokio::test]
    async fn a_stay_another_region_says_it_has_is_that_regions_without_an_arrival() {
        let (mut edge, mut client, said) = a_stay_another_region_has().await;
        assert!(arrival_of(&said, 1).is_none(), "{said:?}");
        assert_eq!(asked_before_numbered(&said, Role::Viewer), view(HOME));
        // Everything numbered is of that stay, in order, and has what the east had
        // not applied.
        let sent = numbered(&said);
        let steps = inputs_of(&said, 1);
        assert_eq!(steps.len(), sent.len(), "{sent:?}");
        assert!(steps.iter().all(|(_, entity, _)| *entity == EntityId(5)));
        let last: Vec<_> = steps
            .iter()
            .rev()
            .take(2)
            .map(|(_, _, step)| *step)
            .collect();
        assert_eq!(last, [3, 2], "{sent:?}");
        assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(HOME));
        assert!(edge.at(EAST).chunks(Role::Guest).is_empty());

        // ADR-0013, section 2: what was told elsewhere carried nothing and is ended;
        // the rest the player still sees, so it is a guest's.
        let asked = subscriptions(edge.sent(WEST).await);
        let rest = minus(&view(HOME), &set(&[COMMON]));
        let expected = BTreeSet::from([(None, set(&[COMMON])), (Some(Role::Guest), rest.clone())]);
        assert_eq!(meanings(&asked), expected);
        assert!(edge.at(WEST).chunks(Role::Viewer).is_empty());
        assert_eq!(edge.at(WEST).chunks(Role::Guest), rest);

        assert!(client.connected());
        assert_eq!(edge.acts(&client).await, (EAST, EntityId(5), 4));
        let said = edge.sent(EAST).await;
        assert!(said.is_empty(), "{said:?}");
        edge.end().await;
    }

    /// Scenario 4. The region's word that it placed a player was lost with a link.
    /// The welcome of the next says that the region had applied their join, so the
    /// `Present` is the stay of that join.
    #[tokio::test]
    async fn a_player_whose_join_the_region_had_applied_is_placed_by_its_presence_answer() {
        let mut edge = Harness::witnessed().await;
        let mut client = edge.join(player(1)).await;
        let (number, join) = edge.next_numbered(WEST).await;
        assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
        edge.lose(WEST).await;
        edge.link(WEST).await;
        assert_eq!(edge.hello(WEST).players, [player(1)]);
        let reply = Reply::resumed().applied(number).stay(1, 5, 0);
        edge.answer(WEST, reply).await;
        let said = edge.sent(WEST).await;
        let sent = numbered(&said);
        assert!(sent.is_empty(), "{sent:?}");
        assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));
        assert!(client.connected());
        assert_eq!(edge.acts(&client).await, (WEST, EntityId(5), 1));
        edge.end().await;
    }

    /// Scenario 4. The welcome says that the region had not applied the join when it
    /// made its answers: the `Present` is of a stay from before the join, which the
    /// join will end. Nothing is done with it; the join is sent again, and the region
    /// then places the player anew. What the client sends before that is dropped
    /// (section 7).
    #[tokio::test]
    async fn a_presence_answer_from_before_a_players_join_does_not_place_them() {
        let mut edge = Harness::witnessed().await;
        let mut client = edge.join(player(1)).await;
        let (number, join) = edge.next_numbered(WEST).await;
        assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
        edge.lose(WEST).await;
        edge.link(WEST).await;
        let reply = Reply::resumed().applied(number - 1).stay(1, 4, 0);
        edge.answer(WEST, reply).await;
        let said = edge.sent(WEST).await;
        let sent = numbered(&said);
        assert!(
            matches!(&sent[..], [(again, EdgeToWorker::PlayerJoin(_))] if *again == number),
            "{sent:?}"
        );
        let asked = subscriptions(said);
        assert!(asked.is_empty(), "{asked:?}");

        edge.walks(&client, HOME, 1).await;
        let said = edge.sent(WEST).await;
        assert!(said.is_empty(), "{said:?}");

        edge.says(WEST, spawned(player(1), EntityId(6))).await;
        edge.sent(WEST).await;
        assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));
        let (region, entity, _) = edge.acts(&client).await;
        assert_eq!((region, entity), (WEST, EntityId(6)));
        assert!(client.connected());
        edge.end().await;
    }

    /// Scenario 5, the sequence of the review's defect 2. The west is split with the
    /// player in the part. The player leaves before the edge has read that, so the
    /// leave goes to the west, which no longer has the stay and counts it as applied.
    /// The west then absorbs the part, which brings the stay back, and the player
    /// joins again. The west's next welcome has a `Present` for the old stay and no
    /// leave is kept: only the welcome's `applied`, which is below the join, tells the
    /// edge that this is not the stay of the join.
    #[tokio::test]
    async fn a_player_who_joins_again_is_not_placed_as_the_stay_a_merge_brought_back() {
        let mut edge = Harness::witnessed().await;
        let before = edge.settler(1, 5, WEST, HOME).await;
        let applied = edge.at(WEST).numbered;
        edge.link(WEST).await;
        edge.leave(&before).await;
        let split = Reply::resumed().applied(applied);
        edge.answer(WEST, split.entry(split_off(PART, &[(1, 5)])))
            .await;
        let said = edge.sent(WEST).await;
        assert_eq!(numbered(&said), [(applied + 1, left(1, Some(5)))]);

        // The merge closes the west's links. The player comes back meanwhile.
        edge.lose(WEST).await;
        let mut again = edge.join(player(1)).await;
        edge.drained().await;
        edge.link(WEST).await;
        assert_eq!(edge.hello(WEST).players, [player(1)]);
        let merged = Reply::resumed().applied(applied + 1);
        let merged = merged.entry(absorbed(PART, 40, 0, &[])).stay(1, 5, 0);
        edge.answer(WEST, merged).await;
        let said = edge.sent(WEST).await;
        let sent = numbered(&said);
        assert!(
            matches!(&sent[..], [(join, EdgeToWorker::PlayerJoin(_))] if *join == applied + 2),
            "{sent:?}"
        );
        let asked = subscriptions(said);
        assert!(asked.is_empty(), "placed as their old self: {asked:?}");

        edge.says(WEST, spawned(player(1), EntityId(8))).await;
        assert_eq!(edge.acts(&again).await, (WEST, EntityId(8), 1));
        assert!(again.connected());
        edge.end().await;
    }

    /// Scenario 6. What a welcome says was applied is dropped before what was kept is
    /// sent: without it, the join would be sent again.
    #[tokio::test]
    async fn what_a_welcome_says_was_applied_is_not_sent_again() {
        for applied_the_join in [true, false] {
            let mut edge = Harness::witnessed().await;
            let mut client = edge.joined(player(1), EntityId(5)).await;
            let join = edge.at(WEST).numbered;
            edge.walks(&client, HOME, 1).await;
            edge.quiet().await;
            edge.link(WEST).await;
            let applied = if applied_the_join { join } else { join - 1 };
            edge.answer(WEST, Reply::resumed().applied(applied).stay(1, 5, 0))
                .await;
            let sent = numbered(&edge.sent(WEST).await);
            let step = (join + 1, passed_on(1, 5, 1, step_into(HOME)));
            if applied_the_join {
                assert_eq!(sent, [step]);
            } else {
                assert_eq!(sent.len(), 2, "{sent:?}");
                assert!(matches!(&sent[0], (again, EdgeToWorker::PlayerJoin(_)) if *again == join));
                assert_eq!(sent[1], step);
            }
            assert!(client.connected());
            edge.end().await;
        }
    }

    /// What the tests of a merge begin with.
    struct Merging {
        edge: Harness,
        /// The first player, who is the north's.
        north: Client,
        /// The second player, who is the west's.
        west: Client,
        /// An action of the first player that is kept for the east.
        action: RemoteAction,
    }

    /// The east is about to absorb the north. The first player is the north's, at
    /// `NORTHERN`; the north has applied their arrival, and they have made four steps
    /// since, which are the north's messages 2 to 5. The second player is the west's,
    /// at `HOME`, and the west has named the north for `LENT`, so the edge is a guest
    /// there. The east has one message kept, an action of the first player about
    /// `SOUGHT`, which the second sees, so that it made the edge a guest there.
    async fn before_a_merge() -> Merging {
        let mut edge = Harness::witnessed().await;
        let north = edge.settler(1, 5, NORTH, NORTHERN).await;
        edge.caught_up(NORTH);
        let west = edge.settler(2, 6, WEST, HOME).await;
        edge.walks(&north, NORTHERN, 4).await;
        edge.tell(WEST, elsewhere(LENT, edge.ask(WEST, LENT), NORTH));
        let action = breaking(player(1), edge.sequence(), SOUGHT);
        edge.say(NORTH, remote(&action, Some(EAST)));
        edge.quiet().await;
        assert_eq!(edge.at(NORTH).chunks(Role::Viewer), view(NORTHERN));
        assert_eq!(edge.at(NORTH).chunks(Role::Guest), set(&[LENT]));
        assert_eq!(edge.at(NORTH).numbered, 5);
        assert_eq!(edge.at(EAST).chunks(Role::Guest), set(&[SOUGHT]));
        assert_eq!(edge.at(EAST).numbered, 1);
        Merging {
            edge,
            north,
            west,
            action,
        }
    }

    /// The east's welcome after it absorbed the north of `before_a_merge`, which had
    /// applied the edge's messages up to 3: the arrival and two steps.
    fn merged() -> Reply {
        Reply::resumed().entry(absorbed(NORTH, 1, 3, &[]))
    }

    /// What the east of `before_a_merge` is sent that is numbered when the entries of
    /// `merged` are through: what was kept for it, and behind that the north's
    /// messages 4 and 5 under the east's next numbers.
    fn kept_after_the_merge(action: &RemoteAction) -> Vec<(u64, EdgeToWorker)> {
        let step = step_into(NORTHERN);
        vec![
            (1, EdgeToWorker::Remote(action.clone())),
            (2, passed_on(1, 5, 3, step.clone())),
            (3, passed_on(1, 5, 4, step)),
        ]
    }

    /// Looks at what the east of `before_a_merge` was sent after `merged`, with a
    /// `Present` for the first player: scenario 7.
    async fn as_after_the_merge(edge: &mut Harness, north: &mut Client, action: &RemoteAction) {
        let said = edge.sent(EAST).await;
        assert_eq!(asked_before_numbered(&said, Role::Viewer), view(NORTHERN));
        assert_eq!(asked_before_numbered(&said, Role::Guest), set(&[LENT]));
        assert_eq!(numbered(&said), kept_after_the_merge(action));
        assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(NORTHERN));
        assert_eq!(edge.at(EAST).chunks(Role::Guest), set(&[LENT, SOUGHT]));
        assert!(north.connected());
        assert_eq!(edge.acts(north).await, (EAST, EntityId(5), 5));
    }

    /// Scenario 7.
    #[tokio::test]
    async fn what_the_edge_had_at_an_absorbed_region_it_has_at_the_survivor_from_the_absorbed_on() {
        let Merging {
            mut edge,
            mut north,
            mut west,
            action,
        } = before_a_merge().await;
        edge.lose(NORTH).await;
        edge.lose(EAST).await;
        edge.link(EAST).await;
        // Rule 43: the hello names nothing of the absorbed region.
        let hello = edge.hello(EAST);
        assert!(hello.players.is_empty() && hello.chunks.is_empty());
        assert_eq!(hello.guests, [SOUGHT]);
        edge.answer(EAST, merged().stay(1, 5, 2)).await;
        as_after_the_merge(&mut edge, &mut north, &action).await;

        // Statement S: the north has no link, and is given none.
        edge.offers_in_vain(NORTH).await;
        // What the north was to show the second player, the east shows them.
        edge.tell(EAST, snapshot(LENT, edge.ask(EAST, LENT)));
        assert!(edge.shows(&mut west, EAST, LENT).await);
        // And the subscription that was told elsewhere with the north is told
        // elsewhere with the east: its `NotMine` has the west asked again.
        edge.tell(EAST, not_mine(LENT, edge.ask(EAST, LENT)));
        edge.settle(EAST).await;
        let asked = edge.asked(WEST).await;
        assert!(
            matches!(&asked[..], [EdgeToWorker::Subscribe { chunks, .. }] if chunks == &[LENT]),
            "{asked:?}"
        );
        edge.end().await;
    }

    /// Scenario 8. A player the absorbed region's `Absorbed` made the survivor's, and
    /// of whom the welcome's presence has no `Present`, is absent: here the welcome
    /// announces no answers at all.
    #[tokio::test]
    async fn a_player_a_merge_brought_and_the_survivor_does_not_have_is_disconnected() {
        let Merging {
            mut edge,
            mut north,
            west: _west,
            action,
        } = before_a_merge().await;
        edge.lose(NORTH).await;
        edge.lose(EAST).await;
        edge.link(EAST).await;
        edge.answer(EAST, merged()).await;
        north.disconnected().await;
        let said = edge.sent(EAST).await;
        assert_eq!(asked_before_numbered(&said, Role::Viewer), view(NORTHERN));
        let mut expected = kept_after_the_merge(&action);
        expected.push((4, left(1, Some(5))));
        assert_eq!(numbered(&said), expected);
        // What the second player sees of it stays, as a guest's.
        assert!(edge.at(EAST).chunks(Role::Viewer).is_empty());
        let mut still_seen = both(&view(HOME), &view(NORTHERN));
        still_seen.extend([LENT, SOUGHT]);
        assert_eq!(edge.at(EAST).chunks(Role::Guest), still_seen);
        edge.end().await;
    }

    /// Scenario 8, where the welcome announces an answer, and it is for someone else.
    #[tokio::test]
    async fn a_player_a_merge_brought_is_judged_when_the_answers_announced_have_come() {
        let Merging {
            mut edge,
            mut north,
            west: _west,
            action,
        } = before_a_merge().await;
        edge.lose(NORTH).await;
        edge.lose(EAST).await;
        edge.link(EAST).await;
        edge.answer(EAST, merged().stay(8, 80, 0)).await;
        north.disconnected().await;
        let mut expected = kept_after_the_merge(&action);
        expected.push((4, left(8, Some(80))));
        expected.push((5, left(1, Some(5))));
        assert_eq!(numbered(&edge.sent(EAST).await), expected);
        edge.end().await;
    }

    /// Scenario 8. A player a merge brought whose arrival is among what was kept for
    /// the absorbed region and goes to the survivor is on their way there, and is not
    /// disconnected for want of a `Present`.
    #[tokio::test]
    async fn a_player_a_merge_brought_stays_while_their_arrival_is_kept_for_the_survivor() {
        let mut edge = Harness::witnessed().await;
        let mut client = edge.settler(1, 5, NORTH, NORTHERN).await;
        assert_eq!(edge.at(NORTH).numbered, 1);
        edge.lose(NORTH).await;
        edge.lose(EAST).await;
        edge.link(EAST).await;
        // The north had applied nothing.
        let reply = Reply::resumed().entry(absorbed(NORTH, 1, 0, &[]));
        edge.answer(EAST, reply).await;
        let said = edge.sent(EAST).await;
        assert_eq!(asked_before_numbered(&said, Role::Viewer), view(NORTHERN));
        let arrival = EdgeToWorker::PlayerArrive {
            player: player(1),
            transfer: transfer(EntityId(5), 0, NORTHERN),
        };
        assert_eq!(numbered(&said), [(1, arrival)]);
        assert!(client.connected());
        assert_eq!(edge.acts(&client).await, (EAST, EntityId(5), 1));
        edge.end().await;
    }

    /// Scenario 9. The `Absorbed` says another `since` than the edge holds for the
    /// absorbed region, of which the edge has seen entries: that region had forgotten
    /// the edge. Nothing kept for it goes to the survivor, and an action among it is
    /// told to its player as handled. Its players become the survivor's all the same
    /// and are judged by the presence, which has none of them.
    #[tokio::test]
    async fn nothing_kept_for_an_absorbed_region_that_had_forgotten_the_edge_goes_to_the_survivor()
    {
        let Merging {
            mut edge,
            mut north,
            west: _west,
            action,
        } = before_a_merge().await;
        // An action of a third player is under way to the north.
        let mut third = edge.settler(3, 7, WEST, HOME).await;
        let given_up = breaking(player(3), edge.sequence(), NORTHERN);
        edge.say(WEST, remote(&given_up, Some(NORTH)));
        edge.quiet().await;
        assert_eq!(edge.at(NORTH).numbered, 6);
        assert!(edge.outbox[NORTH.0 as usize] > 0);

        edge.lose(NORTH).await;
        edge.lose(EAST).await;
        edge.link(EAST).await;
        let reply = Reply::resumed().entry(absorbed(NORTH, 9, 3, &[]));
        edge.answer(EAST, reply).await;
        north.disconnected().await;
        let said = edge.sent(EAST).await;
        assert_eq!(asked_before_numbered(&said, Role::Guest), set(&[LENT]));
        let expected = [
            (1, EdgeToWorker::Remote(action.clone())),
            (2, left(1, Some(5))),
        ];
        assert_eq!(numbered(&said), expected);
        edge.acknowledged(&mut third, given_up.sequence).await;
        edge.end().await;
    }

    /// Scenario 10. The edge never had anything from the absorbed region: no entry
    /// seen, nothing reported applied. Then nothing it kept for it was applied,
    /// whatever the entry says, and all of it goes to the survivor.
    #[tokio::test]
    async fn all_that_was_kept_for_an_absorbed_region_the_edge_never_heard_from_goes_to_the_survivor()
     {
        let mut edge = Harness::witnessed().await;
        let mut client = edge.settler(1, 5, SOUTH, SOUTHERN).await;
        edge.walks(&client, SOUTHERN, 2).await;
        edge.lose(EAST).await;
        edge.link(EAST).await;
        let hello = edge.hello(EAST);
        assert!(hello.players.is_empty() && hello.chunks.is_empty() && hello.guests.is_empty());
        let reply = Reply::resumed().entry(absorbed(SOUTH, 4, 2, &[]));
        edge.answer(EAST, reply).await;
        let said = edge.sent(EAST).await;
        assert_eq!(asked_before_numbered(&said, Role::Viewer), view(SOUTHERN));
        let arrival = EdgeToWorker::PlayerArrive {
            player: player(1),
            transfer: transfer(EntityId(5), 0, SOUTHERN),
        };
        let step = step_into(SOUTHERN);
        let expected = [
            (1, arrival),
            (2, passed_on(1, 5, 1, step.clone())),
            (3, passed_on(1, 5, 2, step)),
        ];
        assert_eq!(numbered(&said), expected);
        assert!(client.connected());
        assert_eq!(edge.acts(&client).await, (EAST, EntityId(5), 3));
        edge.end().await;
    }

    /// What scenarios 11 and 14 begin with: three actions of the west's player are
    /// under way to the north, which has dealt with the first two and said so in its
    /// last two entries, and has dealt with the third without the edge reading of
    /// it. Then the north is gone. Returns how many entries of the north the edge has
    /// seen, and the actions' sequence numbers.
    async fn seen_of_the_north() -> (Harness, Client, u64, [i32; 3]) {
        let mut edge = Harness::witnessed().await;
        let mut client = edge.settler(2, 6, WEST, HOME).await;
        let mut sequences = [0; 3];
        for sequence in &mut sequences {
            *sequence = edge.under_way(2, NORTH).await.sequence;
        }
        edge.quiet().await;
        assert_eq!(edge.at(NORTH).numbered, 3);
        edge.say(NORTH, done(player(2), sequences[0]));
        edge.say(NORTH, done(player(2), sequences[1]));
        edge.acknowledged(&mut client, sequences[1]).await;
        let seen = edge.outbox[NORTH.0 as usize];
        edge.lose(NORTH).await;
        (edge, client, seen, sequences)
    }

    /// The north's last three entries, as they stand behind an `Absorbed`.
    fn three_entries(sequences: [i32; 3]) -> Vec<Durable> {
        sequences.map(|sequence| done(player(2), sequence)).to_vec()
    }

    /// Each of the three actions was acknowledged to the player once: the first two
    /// when the north said so itself, and the third when its entry came with the
    /// merge.
    async fn each_acknowledged_once(edge: &mut Harness, client: &mut Client, sequences: [i32; 3]) {
        edge.acknowledged(client, sequences[2]).await;
        client.drain();
        assert_eq!(client.acknowledged, sequences);
    }

    /// Scenario 11. Of the entries behind an `Absorbed`, those whose number at the
    /// absorbed region the edge had seen there are confirmed and not acted on.
    #[tokio::test]
    async fn entries_that_came_with_a_merge_are_passed_over_where_the_edge_had_seen_them() {
        let (mut edge, mut client, seen, sequences) = seen_of_the_north().await;
        edge.lose(EAST).await;
        edge.link(EAST).await;
        let first = edge.outbox[EAST.0 as usize] + 1;
        assert_eq!(edge.hello(EAST).seen, first - 1);
        let numbers = [seen - 1, seen, seen + 1];
        let mut reply = Reply::resumed().entry(absorbed(NORTH, 1, 3, &numbers));
        reply.entries.extend(three_entries(sequences));
        edge.answer(EAST, reply).await;
        let said = edge.sent(EAST).await;
        assert_eq!(confirmed(&said), Some(first + 3));
        let sent = numbered(&said);
        assert!(sent.is_empty(), "{sent:?}");
        each_acknowledged_once(&mut edge, &mut client, sequences).await;
        edge.end().await;
    }

    /// Scenario 12. An entry that was the absorbed region's and lets a player go to
    /// the survivor names the region it now comes from. That is no fault: the player
    /// arrives there, with an arrival and what they did since.
    #[tokio::test]
    async fn a_departure_to_the_survivor_that_came_with_the_merge_is_an_arrival_there() {
        let (mut edge, mut north, seen) = let_go_to_the_survivor().await;
        let reply = Reply::resumed().entry(absorbed(NORTH, 1, 3, &[seen + 1]));
        edge.answer(EAST, reply.entry(gone_to_the_survivor())).await;
        arrived_at_the_survivor(&mut edge, &mut north).await;
        edge.end().await;
    }

    /// The north has the first player, has applied their arrival and the first two of
    /// their three steps, and has said so. It then let them go to the east, which
    /// absorbed it before the edge read that. Returns how many entries of the north
    /// the edge has seen.
    async fn let_go_to_the_survivor() -> (Harness, Client, u64) {
        let mut edge = Harness::witnessed().await;
        let north = edge.settler(1, 5, NORTH, NORTHERN).await;
        edge.walks(&north, NORTHERN, 3).await;
        edge.tell(NORTH, progress(3, vec![(player(1), 2)]));
        edge.quiet().await;
        let seen = edge.outbox[NORTH.0 as usize];
        edge.lose(NORTH).await;
        edge.lose(EAST).await;
        edge.link(EAST).await;
        (edge, north, seen)
    }

    fn gone_to_the_survivor() -> Durable {
        Durable::Departed {
            player: player(1),
            transfer: transfer(EntityId(5), 2, NORTHERN),
            to: EAST,
        }
    }

    async fn arrived_at_the_survivor(edge: &mut Harness, north: &mut Client) {
        let said = edge.sent(EAST).await;
        let (number, arrived) = arrival_of(&said, 1).expect("the player arrives at the survivor");
        assert_eq!(arrived, transfer(EntityId(5), 2, NORTHERN));
        let asked = asked_before_numbered(&said, Role::Viewer);
        assert!(asked.is_superset(&view(NORTHERN)), "{asked:?}");
        // What they did and the north had not applied follows the arrival.
        let mut after = inputs_of(&said, 1);
        after.retain(|(numbered, ..)| *numbered > number);
        let after: Vec<_> = after
            .iter()
            .map(|(_, entity, step)| (*entity, *step))
            .collect();
        assert_eq!(after, [(EntityId(5), 3)]);
        let arrivals = said
            .iter()
            .filter(|message| matches!(message.body, EdgeToWorker::PlayerArrive { .. }));
        assert_eq!(arrivals.count(), 1);
        assert!(north.connected());
        assert_eq!(edge.acts(north).await, (EAST, EntityId(5), 4));
    }

    /// Scenario 12. An entry of the survivor's own that names the survivor is the
    /// fault it was, also behind entries that came with a merge.
    #[tokio::test]
    async fn a_departure_of_the_survivors_own_that_names_the_survivor_disconnects_the_player() {
        let mut edge = Harness::witnessed().await;
        let mut east = edge.settler(3, 7, EAST, EASTERN).await;
        let mut north = edge.settler(1, 5, NORTH, NORTHERN).await;
        let seen = edge.outbox[NORTH.0 as usize];
        edge.lose(NORTH).await;
        edge.lose(EAST).await;
        edge.link(EAST).await;
        assert_eq!(edge.hello(EAST).players, [player(3)]);
        let came_with = Durable::Departed {
            player: player(1),
            transfer: transfer(EntityId(5), 0, NORTHERN),
            to: EAST,
        };
        let reply = Reply::resumed()
            .entry(absorbed(NORTH, 1, 1, &[seen + 1]))
            .entry(came_with)
            .entry(departed(player(3), EntityId(7), EAST, EASTERN));
        edge.answer(EAST, reply).await;
        east.disconnected().await;
        assert!(north.connected());
        assert_eq!(edge.acts(&north).await, (EAST, EntityId(5), 1));
        edge.end().await;
    }

    /// Scenario 13. The survivor let a player go to the region it then absorbed. Read
    /// in order, the first entry puts the player under the absorbed region, where
    /// their arrival is kept as for any region without a link, and the `Absorbed`
    /// brings all of it back: the arrival reaches the survivor under its numbers.
    #[tokio::test]
    async fn a_player_let_go_to_a_region_the_survivor_then_absorbed_arrives_at_the_survivor() {
        let mut edge = Harness::witnessed().await;
        let mut client = edge.settler(1, 5, EAST, EASTERN).await;
        edge.caught_up(EAST);
        edge.quiet().await;
        edge.lose(NORTH).await;
        edge.lose(EAST).await;
        edge.link(EAST).await;
        assert_eq!(edge.hello(EAST).players, [player(1)]);
        let reply = Reply::resumed()
            .applied(1)
            .entry(departed(player(1), EntityId(5), NORTH, EASTERN))
            .entry(absorbed(NORTH, 1, 0, &[]));
        // The hello named the player, whom the east does not have: `Absent`.
        edge.answer(EAST, reply).await;
        let said = edge.sent(EAST).await;
        let arrival = EdgeToWorker::PlayerArrive {
            player: player(1),
            transfer: transfer(EntityId(5), 0, EASTERN),
        };
        assert_eq!(numbered(&said), [(2, arrival)]);
        assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(EASTERN));
        assert!(client.connected());
        assert_eq!(edge.acts(&client).await, (EAST, EntityId(5), 1));
        edge.end().await;
    }

    /// Scenario 13, with a link that ends between the two entries: after the first
    /// the player is under the absorbed region, so the next hello to the survivor
    /// names neither them nor their view as a viewer's.
    #[tokio::test]
    async fn a_player_let_go_to_a_region_that_is_absorbed_is_under_it_until_the_absorbed_is_read() {
        let mut edge = Harness::witnessed().await;
        let mut client = edge.settler(1, 5, EAST, EASTERN).await;
        edge.caught_up(EAST);
        edge.quiet().await;
        edge.lose(NORTH).await;
        edge.lose(EAST).await;
        edge.link(EAST).await;
        let let_go = Reply::resumed().applied(1);
        let let_go = let_go.entry(departed(player(1), EntityId(5), NORTH, EASTERN));
        edge.answer(EAST, let_go).await;
        let sent = numbered(&edge.sent(EAST).await);
        assert!(sent.is_empty(), "{sent:?}");
        assert!(client.connected());

        edge.lose(EAST).await;
        edge.link(EAST).await;
        let hello = edge.hello(EAST);
        assert!(hello.players.is_empty() && hello.chunks.is_empty());
        assert_eq!(set(&hello.guests), view(EASTERN));
        let merged = Reply::resumed().applied(1);
        edge.answer(EAST, merged.entry(absorbed(NORTH, 1, 0, &[])))
            .await;
        let said = edge.sent(EAST).await;
        assert_eq!(asked_before_numbered(&said, Role::Viewer), view(EASTERN));
        let arrival = EdgeToWorker::PlayerArrive {
            player: player(1),
            transfer: transfer(EntityId(5), 0, EASTERN),
        };
        assert_eq!(numbered(&said), [(2, arrival)]);
        assert!(client.connected());
        assert_eq!(edge.acts(&client).await, (EAST, EntityId(5), 1));
        edge.end().await;
    }

    /// Scenario 14, and A7 of ADR-0014. The link ends when the edge has read the
    /// `Absorbed` and none of the entries behind it. The next welcome brings those
    /// without the `Absorbed` in front, and they are still told by their numbers.
    #[tokio::test]
    async fn entries_that_came_with_a_merge_are_told_by_their_numbers_on_a_later_link() {
        let (mut edge, mut client, seen, sequences) = seen_of_the_north().await;
        edge.lose(EAST).await;
        edge.link(EAST).await;
        let welcome = Welcome::Resumed {
            entries: 4,
            presences: 0,
            applied: 0,
        };
        edge.says(EAST, WorkerToEdge::Welcome(welcome)).await;
        let numbers = [seen - 1, seen, seen + 1];
        let entry = absorbed(NORTH, 1, 3, &numbers);
        edge.take(Step::Entry(EAST, entry)).await;
        let first = edge.outbox[EAST.0 as usize];
        edge.lose(EAST).await;

        edge.link(EAST).await;
        assert_eq!(edge.hello(EAST).seen, first);
        let mut reply = Reply::resumed();
        reply.entries.extend(three_entries(sequences));
        edge.answer(EAST, reply).await;
        let said = edge.sent(EAST).await;
        assert_eq!(confirmed(&said), Some(first + 3));
        each_acknowledged_once(&mut edge, &mut client, sequences).await;
        edge.end().await;
    }

    /// Scenario 15. The survivor forgets the edge while entries that came with a merge
    /// are still to come. Its outbox is numbered anew, and an entry under a number
    /// that one of those had is the survivor's own: it is acted on, though the edge
    /// had seen, at the absorbed region, the entry that number stood for.
    #[tokio::test]
    async fn an_entry_of_a_survivor_that_forgot_the_edge_is_not_taken_for_one_that_came_with_a_merge()
     {
        let mut edge = Harness::witnessed().await;
        let mut client = edge.settler(2, 6, WEST, HOME).await;
        let seen = edge.outbox[NORTH.0 as usize];
        assert!(seen >= 3, "{seen}");
        edge.lose(NORTH).await;
        // The south is new to the edge, so its outbox begins at 1.
        edge.link(SOUTH).await;
        let welcome = Welcome::Unknown {
            since: 1,
            entries: 4,
            presences: 0,
            applied: 0,
        };
        edge.says(SOUTH, WorkerToEdge::Welcome(welcome)).await;
        // The entries behind it would be the south's 2, 3 and 4, all of which the
        // edge had seen at the north.
        let numbers = [seen - 2, seen - 1, seen];
        let entry = absorbed(NORTH, 1, 0, &numbers);
        edge.take(Step::Entry(SOUTH, entry)).await;
        edge.lose(SOUTH).await;

        // The south forgets the edge and is told of it anew. Three actions of the
        // player are then passed on to it, and it deals with them: its entries 1, 2
        // and 3.
        edge.link(SOUTH).await;
        let hello = edge.hello(SOUTH);
        assert_eq!((hello.since, hello.seen), (1, 1));
        edge.answer(SOUTH, Reply::unknown(2)).await;
        let mut sequences = [0; 3];
        for sequence in &mut sequences {
            *sequence = edge.under_way(2, SOUTH).await.sequence;
        }
        assert_eq!(edge.outbox[SOUTH.0 as usize], 0);
        for sequence in sequences {
            edge.take(Step::Entry(SOUTH, done(player(2), sequence)))
                .await;
        }
        edge.acknowledged(&mut client, sequences[2]).await;
        assert_eq!(client.acknowledged, sequences);
        // The north is no more all the same.
        edge.offers_in_vain(NORTH).await;
        edge.end().await;
    }

    /// Scenario 16, and A2 of ADR-0014. The north absorbed the south and the east then
    /// absorbed the north, with no link meanwhile. The east's welcome has `Absorbed`
    /// for the north and, among the entries behind it, `Absorbed` for the south with
    /// the south's entries behind that. Everything the edge had at either is at the
    /// east afterwards, and an entry of the south's that the edge had seen at the
    /// south is passed over.
    #[tokio::test]
    async fn two_merges_in_a_row_bring_what_the_edge_had_at_both_regions_to_the_survivor() {
        let mut edge = Harness::witnessed().await;
        edge.link(SOUTH).await;
        edge.answer(SOUTH, Reply::unknown(1)).await;
        // Two actions of a player of the west are under way to the south.
        let mut third = edge.settler(3, 7, WEST, HOME).await;
        let seen_before = edge.under_way(3, SOUTH).await.sequence;
        let new = edge.under_way(3, SOUTH).await.sequence;
        let mut first = edge.settler(1, 5, NORTH, NORTHERN).await;
        let mut second = edge.settler(2, 6, SOUTH, SOUTHERN).await;
        edge.walks(&first, NORTHERN, 1).await;
        edge.walks(&second, SOUTHERN, 1).await;
        edge.quiet().await;
        // The two actions, the arrival and the step.
        assert_eq!(edge.at(SOUTH).numbered, 4);
        assert_eq!(edge.at(NORTH).numbered, 2);
        edge.say(SOUTH, done(player(3), seen_before));
        edge.acknowledged(&mut third, seen_before).await;
        let north = edge.outbox[NORTH.0 as usize];
        let south = edge.outbox[SOUTH.0 as usize];
        for region in [SOUTH, NORTH, EAST] {
            edge.lose(region).await;
        }
        edge.link(EAST).await;
        let hello = edge.hello(EAST);
        assert!(hello.players.is_empty() && hello.chunks.is_empty());

        // Each had applied everything but the step of its player.
        let reply = Reply::resumed()
            .entry(absorbed(NORTH, 1, 1, &[north + 1, north + 2, north + 3]))
            .entry(absorbed(SOUTH, 1, 3, &[south, south + 1]))
            .entry(done(player(3), seen_before))
            .entry(done(player(3), new))
            .stay(1, 5, 0)
            .stay(2, 6, 0);
        edge.answer(EAST, reply).await;
        let said = edge.sent(EAST).await;
        let mut views = view(NORTHERN);
        views.extend(view(SOUTHERN));
        assert_eq!(asked_before_numbered(&said, Role::Viewer), views);
        let expected = [
            (1, passed_on(1, 5, 1, step_into(NORTHERN))),
            (2, passed_on(2, 6, 1, step_into(SOUTHERN))),
        ];
        assert_eq!(numbered(&said), expected);
        edge.acknowledged(&mut third, new).await;
        third.drain();
        assert_eq!(third.acknowledged, [seen_before, new]);
        assert!(first.connected() && second.connected());
        assert_eq!(edge.acts(&first).await, (EAST, EntityId(5), 2));
        assert_eq!(edge.acts(&second).await, (EAST, EntityId(6), 2));
        edge.offers_in_vain(NORTH).await;
        edge.offers_in_vain(SOUTH).await;
        edge.end().await;
    }

    /// Section 3, step 7. The edge has read the north's `Absorbed` for the south and
    /// not the south's entries behind it when the east absorbs the north. Behind the
    /// east's `Absorbed` for the north those entries keep the origin they had: they
    /// are the south's, and are told by what the edge had seen of the south. A name
    /// of the south then means the east.
    #[tokio::test]
    async fn an_entry_that_came_with_two_merges_is_told_by_the_number_it_had_first() {
        let mut edge = Harness::witnessed().await;
        edge.link(SOUTH).await;
        edge.answer(SOUTH, Reply::unknown(1)).await;
        let mut third = edge.settler(3, 7, WEST, HOME).await;
        let seen_before = edge.under_way(3, SOUTH).await.sequence;
        let new = edge.under_way(3, SOUTH).await.sequence;
        edge.say(SOUTH, done(player(3), seen_before));
        edge.acknowledged(&mut third, seen_before).await;
        let south = edge.outbox[SOUTH.0 as usize];
        edge.lose(SOUTH).await;
        edge.lose(NORTH).await;
        edge.link(NORTH).await;
        let welcome = Welcome::Resumed {
            entries: 3,
            presences: 0,
            applied: 0,
        };
        edge.says(NORTH, WorkerToEdge::Welcome(welcome)).await;
        // The south had applied both actions.
        let entry = absorbed(SOUTH, 1, 2, &[south, south + 1]);
        edge.take(Step::Entry(NORTH, entry)).await;
        let north = edge.outbox[NORTH.0 as usize];
        edge.lose(NORTH).await;

        edge.lose(EAST).await;
        edge.link(EAST).await;
        let reply = Reply::resumed()
            .entry(absorbed(NORTH, 1, 0, &[north + 1, north + 2]))
            .entry(done(player(3), seen_before))
            .entry(done(player(3), new));
        edge.answer(EAST, reply).await;
        edge.acknowledged(&mut third, new).await;
        third.drain();
        assert_eq!(third.acknowledged, [seen_before, new]);

        edge.say(WEST, departed(player(3), EntityId(7), SOUTH, SOUTHERN));
        edge.settle(WEST).await;
        let said = edge.sent(EAST).await;
        let arrival = arrival_of(&said, 3);
        assert!(arrival.is_some(), "{said:?}");
        let (region, entity, _) = edge.acts(&third).await;
        assert_eq!((region, entity), (EAST, EntityId(7)));
        edge.end().await;
    }

    /// Scenario 17. The edge reads the `Absorbed` while its link to the absorbed
    /// region still stands, with entries unread on it that are also behind the
    /// `Absorbed`. Whichever link it reads from first, each entry is acted on once:
    /// one read from the absorbed region's link is passed over behind the `Absorbed`
    /// by its number, and once the `Absorbed` is read that link is no longer the
    /// region's.
    #[tokio::test]
    async fn an_entry_on_the_absorbed_regions_link_and_behind_the_absorbed_is_acted_on_once() {
        let mut orders = Orders::new();
        while orders.another() {
            let mut edge = Harness::witnessed().await;
            let mut first = edge.settler(1, 5, NORTH, NORTHERN).await;
            let mut second = edge.settler(2, 6, WEST, HOME).await;
            let sequence = edge.under_way(2, NORTH).await.sequence;
            edge.quiet().await;
            // The north has applied the arrival and the action.
            assert_eq!(edge.at(NORTH).numbered, 2);
            edge.caught_up(NORTH);
            edge.quiet().await;
            let next = edge.outbox[NORTH.0 as usize] + 1;
            // The north's last two entries: an action of the second player was dealt
            // with, and the first player was let go to the west.
            let gone = departed(player(1), EntityId(5), WEST, HOME);
            edge.lose(EAST).await;
            let merged = Reply::resumed()
                .entry(absorbed(NORTH, 1, 2, &[next, next + 1]))
                .entry(done(player(2), sequence))
                .entry(gone.clone());
            let scripts = vec![
                vec![Step::Link(EAST), Step::Welcome(EAST, merged)],
                vec![
                    Step::Entry(NORTH, done(player(2), sequence)),
                    Step::Entry(NORTH, gone),
                ],
            ];
            edge.play(scripts, &mut orders).await;

            edge.acknowledged(&mut second, sequence).await;
            let said = edge.sent(WEST).await;
            let arrivals = said.iter().filter(|message| {
                matches!(&message.body, EdgeToWorker::PlayerArrive { player: who, .. } if *who == player(1))
            });
            assert_eq!(arrivals.count(), 1, "{said:?}");
            let said = edge.sent(EAST).await;
            assert!(arrival_of(&said, 1).is_none(), "{said:?}");
            assert!(first.connected());
            assert_eq!(edge.acts(&first).await, (WEST, EntityId(5), 1));
            second.drain();
            assert_eq!(second.acknowledged, [sequence]);
            edge.end().await;
        }
    }

    /// What scenario 18 begins with: the west's player sees `COMMON`, for which the
    /// west has named the north twice; the north said `NotMine` in between, so the
    /// west was asked again just now. Then the east absorbs the north.
    async fn told_elsewhere_with_the_absorbed() -> (Harness, Client) {
        let mut edge = Harness::witnessed().await;
        let client = edge.settler(1, 5, WEST, HOME).await;
        edge.says(WEST, elsewhere(COMMON, edge.ask(WEST, COMMON), NORTH))
            .await;
        let asked = subscriptions(edge.sent(NORTH).await);
        assert_eq!(
            meanings(&asked),
            BTreeSet::from([(Some(Role::Guest), set(&[COMMON]))])
        );
        edge.says(NORTH, not_mine(COMMON, edge.ask(NORTH, COMMON)))
            .await;
        let asked = subscriptions(edge.sent(WEST).await);
        assert_eq!(
            meanings(&asked),
            BTreeSet::from([(Some(Role::Viewer), set(&[COMMON]))])
        );
        edge.says(WEST, elsewhere(COMMON, edge.ask(WEST, COMMON), NORTH))
            .await;
        let asked = subscriptions(edge.sent(NORTH).await);
        assert_eq!(
            meanings(&asked),
            BTreeSet::from([(Some(Role::Guest), set(&[COMMON]))])
        );
        edge.lose(NORTH).await;
        edge.lose(EAST).await;
        edge.link(EAST).await;
        assert!(edge.hello(EAST).guests.is_empty());
        (edge, client)
    }

    /// Scenario 18. From the `Absorbed` on, a subscription that was told elsewhere
    /// with the absorbed region is told elsewhere with the survivor, where the edge
    /// is a guest for the chunk from then on. Nothing is asked again for it, also
    /// when it was asked again within the last second. `NotMine` from the survivor
    /// has the viewer's region asked again, here when the second is over; and a later
    /// `Elsewhere` that names the absorbed region names the survivor.
    #[tokio::test(start_paused = true)]
    async fn a_subscription_told_elsewhere_with_an_absorbed_region_is_told_so_with_the_survivor() {
        let (mut edge, mut client) = told_elsewhere_with_the_absorbed().await;
        edge.answer(EAST, Reply::resumed().entry(absorbed(NORTH, 1, 0, &[])))
            .await;
        let asked = subscriptions(edge.sent(EAST).await);
        assert_eq!(asked, [as_guest(1, [COMMON])]);
        let asked = subscriptions(edge.sent(WEST).await);
        assert!(asked.is_empty(), "{asked:?}");

        edge.says(EAST, not_mine(COMMON, 1)).await;
        let asked = subscriptions(edge.sent(WEST).await);
        assert!(asked.is_empty(), "asked again within a second: {asked:?}");
        tokio::time::advance(Duration::from_millis(1100)).await;
        let again = edge.next_asked(WEST).await;
        assert_eq!(meaning(&again), (Some(Role::Viewer), set(&[COMMON])));

        edge.says(WEST, elsewhere(COMMON, edge.ask(WEST, COMMON), NORTH))
            .await;
        let asked = subscriptions(edge.sent(EAST).await);
        assert_eq!(
            meanings(&asked),
            BTreeSet::from([(Some(Role::Guest), set(&[COMMON]))])
        );
        edge.says(EAST, snapshot(COMMON, edge.ask(EAST, COMMON)))
            .await;
        edge.sync(&mut client).await;
        assert!(client.chunks.contains_key(&COMMON));
        edge.end().await;
    }

    /// Scenario 18, when more than a second has passed since the west was asked
    /// again: the survivor's `NotMine` has it asked again at once.
    #[tokio::test(start_paused = true)]
    async fn not_mine_from_the_survivor_has_the_region_asked_again_that_named_the_absorbed() {
        let (mut edge, _client) = told_elsewhere_with_the_absorbed().await;
        tokio::time::advance(Duration::from_millis(2500)).await;
        edge.answer(EAST, Reply::resumed().entry(absorbed(NORTH, 1, 0, &[])))
            .await;
        let asked = subscriptions(edge.sent(EAST).await);
        assert_eq!(asked, [as_guest(1, [COMMON])]);
        let asked = subscriptions(edge.sent(WEST).await);
        assert!(asked.is_empty(), "{asked:?}");
        edge.says(EAST, not_mine(COMMON, 1)).await;
        let asked = subscriptions(edge.sent(WEST).await);
        assert_eq!(
            meanings(&asked),
            BTreeSet::from([(Some(Role::Viewer), set(&[COMMON]))])
        );
        edge.end().await;
    }

    /// Scenario 19: the routing table says that the east absorbed the north while the
    /// edge has a player and messages under the north and a link to the east whose
    /// entries are through. The edge ends that link, so that a hello is said which
    /// the east answers from after the merge. That welcome has no `Absorbed` for the
    /// north: the north had forgotten the edge, or the east has since.
    async fn a_survivor_that_says_no_absorbed(present: bool) {
        let Merging {
            mut edge,
            mut north,
            west: _west,
            action,
        } = before_a_merge().await;
        let mut third = edge.settler(3, 7, WEST, HOME).await;
        let given_up = breaking(player(3), edge.sequence(), NORTHERN);
        edge.say(WEST, remote(&given_up, Some(NORTH)));
        edge.quiet().await;
        edge.lose(NORTH).await;

        edge.pairs(vec![(NORTH, EAST)]).await;
        edge.ended_by_the_edge(EAST).await;
        edge.link(EAST).await;
        // Rule 43: the hello names nothing of the absorbed region.
        let hello = edge.hello(EAST);
        assert!(hello.players.is_empty() && hello.chunks.is_empty());
        assert_eq!(hello.guests, [SOUGHT]);
        let mut reply = Reply::resumed();
        if present {
            reply = reply.stay(1, 5, 4);
        }
        edge.answer(EAST, reply).await;
        if !present {
            north.disconnected().await;
        }
        // What was kept for the north is given up, and none of it is sent on.
        edge.acknowledged(&mut third, given_up.sequence).await;
        let said = edge.sent(EAST).await;
        assert_eq!(asked_before_numbered(&said, Role::Viewer), view(NORTHERN));
        assert_eq!(asked_before_numbered(&said, Role::Guest), set(&[LENT]));
        let mut expected = vec![(1, EdgeToWorker::Remote(action))];
        if present {
            assert_eq!(numbered(&said), expected);
            assert!(north.connected());
            assert_eq!(edge.acts(&north).await, (EAST, EntityId(5), 5));
        } else {
            expected.push((2, left(1, Some(5))));
            assert_eq!(numbered(&said), expected);
        }

        // The table says all of its pairs every time. Nothing is owed any more.
        edge.pairs(vec![(NORTH, EAST)]).await;
        edge.offers_in_vain(NORTH).await;
        edge.end().await;
    }

    /// Scenario 19, with a welcome that announces no presence answers.
    #[tokio::test]
    async fn a_player_of_a_region_the_table_says_was_absorbed_is_judged_by_a_welcome_without_answers()
     {
        a_survivor_that_says_no_absorbed(false).await;
    }

    /// Scenario 19, with a `Present` for the player.
    #[tokio::test]
    async fn a_player_of_a_region_the_table_says_was_absorbed_stays_if_the_survivor_has_them() {
        a_survivor_that_says_no_absorbed(true).await;
    }

    /// Scenario 20. The table's word comes before the survivor's welcome, or while it
    /// is being read, or after: the `Absorbed` is handled as without the table, and
    /// no link is ended, as the edge had none to the survivor whose entries were
    /// through and that could be from before the merge.
    #[tokio::test]
    async fn the_tables_word_around_a_welcome_that_has_the_absorbed_ends_no_link() {
        let mut orders = Orders::new();
        while orders.another() {
            let Merging {
                mut edge,
                mut north,
                west: _west,
                action,
            } = before_a_merge().await;
            edge.lose(NORTH).await;
            edge.lose(EAST).await;
            let scripts = vec![
                vec![Step::Pairs(vec![(NORTH, EAST)])],
                vec![
                    Step::Link(EAST),
                    Step::Welcome(EAST, merged().stay(1, 5, 2)),
                ],
            ];
            edge.play(scripts, &mut orders).await;
            assert!(edge.gone.is_empty(), "the edge ended {:?}", edge.gone);
            as_after_the_merge(&mut edge, &mut north, &action).await;
            edge.end().await;
        }
    }

    /// Scenario 20, with a link to the survivor from before the merge that the edge
    /// has not found ended: the table's word ends it, and the next welcome brings the
    /// `Absorbed`.
    #[tokio::test]
    async fn the_tables_word_ends_a_link_from_before_the_merge_and_the_next_brings_the_absorbed() {
        let Merging {
            mut edge,
            mut north,
            west: _west,
            action,
        } = before_a_merge().await;
        edge.lose(NORTH).await;
        edge.pairs(vec![(NORTH, EAST)]).await;
        edge.ended_by_the_edge(EAST).await;
        edge.link(EAST).await;
        edge.answer(EAST, merged().stay(1, 5, 2)).await;
        as_after_the_merge(&mut edge, &mut north, &action).await;
        edge.end().await;
    }

    /// Scenario 21. The table's word comes while a welcome of the survivor is being
    /// read that has no `Absorbed` for the north. The hello of that link was said
    /// before the edge knew that such a word was owed, so nothing is concluded from
    /// its silence: the link is ended when its entries are through, and nothing that
    /// was kept is sent on it.
    #[tokio::test]
    async fn the_tables_word_during_a_welcome_without_the_absorbed_ends_the_link_after_its_entries()
    {
        let Merging {
            mut edge,
            mut north,
            west: _west,
            action,
        } = before_a_merge().await;
        edge.lose(NORTH).await;
        edge.link(EAST).await;
        let welcome = Welcome::Resumed {
            entries: 2,
            presences: 0,
            applied: 0,
        };
        edge.says(EAST, WorkerToEdge::Welcome(welcome)).await;
        let nobody = done(player(u128::MAX), 0);
        edge.take(Step::Entry(EAST, nobody.clone())).await;
        edge.pairs(vec![(NORTH, EAST)]).await;
        edge.handled(EAST).await;
        assert!(
            edge.gone.is_empty(),
            "ended before its entries were through"
        );
        edge.take(Step::Entry(EAST, nobody)).await;
        edge.ended_by_the_edge(EAST).await;
        let said: Vec<_> = edge.heard[EAST.0 as usize].said.drain(..).collect();
        let sent = numbered(&said);
        assert!(sent.is_empty(), "sent on a link that was to end: {sent:?}");
        assert!(north.connected());

        edge.link(EAST).await;
        edge.answer(EAST, merged().stay(1, 5, 2)).await;
        as_after_the_merge(&mut edge, &mut north, &action).await;
        edge.end().await;
    }

    /// Scenario 22. Of the north the edge has nothing but a link. On the table's word
    /// it stands for the east at once: its link is taken, nothing is sent to anyone,
    /// and a player a third region lets go to the north arrives at the east.
    #[tokio::test]
    async fn a_region_the_edge_has_nothing_of_stands_for_its_survivor_on_the_tables_word() {
        let mut edge = Harness::witnessed().await;
        let mut client = edge.settler(1, 5, WEST, HOME).await;
        edge.pairs(vec![(NORTH, EAST)]).await;
        edge.ended_by_the_edge(NORTH).await;
        for region in [WEST, EAST] {
            let said = edge.sent(region).await;
            assert!(said.is_empty(), "{region:?} was sent {said:?}");
        }

        edge.say(WEST, departed(player(1), EntityId(5), NORTH, NORTHERN));
        edge.settle(WEST).await;
        let said = edge.sent(EAST).await;
        let arrival = arrival_of(&said, 1);
        assert_eq!(arrival, Some((1, transfer(EntityId(5), 0, NORTHERN))));
        // What the player saw is asked for before they arrive, as at any hand-over.
        assert_eq!(asked_before_numbered(&said, Role::Viewer), view(HOME));
        assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(NORTHERN));
        assert!(client.connected());
        assert_eq!(edge.acts(&client).await, (EAST, EntityId(5), 1));
        edge.offers_in_vain(NORTH).await;
        edge.end().await;
    }

    /// Section 1: every region id a region says is read through the stand-ins. The
    /// north stands for the east. An action that is sent on to the north, by `Remote`
    /// or by `NotMine`, goes to the east; a chunk for which the north is named is
    /// asked of the east; and what the east itself sends to the north comes back to
    /// the east (section 4): an action, and a player, who is not disconnected for it.
    #[tokio::test]
    async fn every_name_of_an_absorbed_region_means_its_survivor() {
        let mut edge = Harness::witnessed().await;
        let mut client = edge.settler(1, 5, WEST, HOME).await;
        edge.pairs(vec![(NORTH, EAST)]).await;
        edge.ended_by_the_edge(NORTH).await;

        let first = breaking(player(1), edge.sequence(), COMMON);
        edge.say(WEST, remote(&first, Some(NORTH)));
        edge.settle(WEST).await;
        let said = edge.sent(EAST).await;
        assert_eq!(numbered(&said), [(1, EdgeToWorker::Remote(first))]);
        assert_eq!(asked_before_numbered(&said, Role::Guest), set(&[COMMON]));

        let second = breaking(player(1), edge.sequence(), COMMON);
        let sent_on = Durable::NotMine {
            what: Misdirected::Remote(second.clone()),
            holder: NORTH,
        };
        edge.say(WEST, sent_on);
        edge.settle(WEST).await;
        let said = edge.sent(EAST).await;
        assert_eq!(numbered(&said), [(2, EdgeToWorker::Remote(second))]);

        edge.tell(WEST, elsewhere(FAR, edge.ask(WEST, FAR), NORTH));
        edge.settle(WEST).await;
        let asked = subscriptions(edge.sent(EAST).await);
        assert_eq!(
            meanings(&asked),
            BTreeSet::from([(Some(Role::Guest), set(&[FAR]))])
        );

        // The east names the region it absorbed, as it believed before the merge.
        let third = breaking(player(1), edge.sequence(), COMMON);
        edge.say(EAST, remote(&third, Some(NORTH)));
        edge.settle(EAST).await;
        let said = edge.sent(EAST).await;
        assert_eq!(numbered(&said), [(3, EdgeToWorker::Remote(third.clone()))]);
        client.drain();
        assert!(!client.acknowledged.contains(&third.sequence));

        edge.say(WEST, departed(player(1), EntityId(5), NORTH, EASTERN));
        edge.settle(WEST).await;
        let said = edge.sent(EAST).await;
        assert!(arrival_of(&said, 1).is_some(), "{said:?}");
        edge.say(EAST, departed(player(1), EntityId(5), NORTH, EASTERN));
        edge.settle(EAST).await;
        let said = edge.sent(EAST).await;
        assert!(arrival_of(&said, 1).is_some(), "{said:?}");
        assert!(client.connected());
        let (region, entity, _) = edge.acts(&client).await;
        assert_eq!((region, entity), (EAST, EntityId(5)));
        edge.end().await;
    }

    /// Scenario 23. The north absorbed the south, which the edge has handled; then
    /// the table says that the east absorbed the north, of which the edge has
    /// nothing. The stand-ins never make a chain: a name of the south means the east.
    #[tokio::test]
    async fn a_region_that_stood_for_one_that_is_absorbed_stands_for_the_last_survivor() {
        let mut edge = Harness::witnessed().await;
        let mut client = edge.settler(1, 5, WEST, HOME).await;
        edge.lose(NORTH).await;
        edge.link(NORTH).await;
        let merged = Reply::resumed().entry(absorbed(SOUTH, 0, 0, &[]));
        edge.answer(NORTH, merged).await;
        edge.pairs(vec![(SOUTH, NORTH), (NORTH, EAST)]).await;
        edge.ended_by_the_edge(NORTH).await;

        edge.say(WEST, departed(player(1), EntityId(5), SOUTH, SOUTHERN));
        edge.settle(WEST).await;
        let said = edge.sent(EAST).await;
        let arrival = arrival_of(&said, 1);
        assert_eq!(arrival, Some((1, transfer(EntityId(5), 0, SOUTHERN))));
        assert!(client.connected());
        assert_eq!(edge.acts(&client).await, (EAST, EntityId(5), 1));
        edge.offers_in_vain(SOUTH).await;
        edge.offers_in_vain(NORTH).await;
        edge.end().await;
    }

    /// Scenario 24. The east absorbed the north and answered a hello of the edge, and
    /// was then absorbed by the south. The edge has a player under the north and the
    /// east's welcome unread on a link. The table's word, that welcome and the south's
    /// come in every order; afterwards the player is the south's, and was never
    /// disconnected.
    #[tokio::test]
    async fn two_merges_told_by_the_table_and_two_welcomes_in_any_order_end_at_the_last_survivor() {
        let mut orders = Orders::new();
        while orders.another() {
            let mut edge = Harness::witnessed().await;
            let mut client = edge.settler(1, 5, NORTH, NORTHERN).await;
            edge.caught_up(NORTH);
            edge.walks(&client, NORTHERN, 1).await;
            edge.quiet().await;
            edge.lose(NORTH).await;
            edge.link(EAST).await;
            assert!(edge.hello(EAST).players.is_empty());
            let at_east = edge.outbox[EAST.0 as usize] + 1;
            let east = Reply::resumed()
                .entry(absorbed(NORTH, 1, 1, &[]))
                .stay(1, 5, 0);
            // The south came by its state for the edge through the merge.
            let south = Reply::unknown(7)
                .entry(absorbed(EAST, 1, 0, &[at_east]))
                .entry(absorbed(NORTH, 1, 1, &[]))
                .stay(1, 5, 0);
            let scripts = vec![
                vec![Step::Pairs(vec![(NORTH, EAST), (EAST, SOUTH)])],
                vec![Step::Welcome(EAST, east)],
                vec![Step::Link(SOUTH), Step::Welcome(SOUTH, south)],
            ];
            edge.play(scripts, &mut orders).await;
            assert!(
                !edge.gone.contains(&SOUTH),
                "the edge ended its link to the south"
            );
            assert!(client.connected());
            assert_eq!(edge.acts(&client).await, (SOUTH, EntityId(5), 2));
            edge.end().await;
        }
    }

    /// Scenario 25. The edge has handled the south's `Absorbed` for the east when a
    /// table arrives that is behind it: it says that the east absorbed the north and
    /// nothing of the south. The pair's target is read through the stand-ins: the
    /// word is owed by the south, whose link is ended for it.
    #[tokio::test]
    async fn a_pair_whose_target_the_edge_has_retired_names_the_living_region() {
        let mut edge = Harness::witnessed().await;
        let mut client = edge.settler(1, 5, NORTH, NORTHERN).await;
        edge.lose(NORTH).await;
        edge.link(SOUTH).await;
        let merged = Reply::unknown(7).entry(absorbed(EAST, 1, 0, &[]));
        edge.answer(SOUTH, merged).await;
        // The east's link stood: it is taken with the `Absorbed`.
        edge.ended_by_the_edge(EAST).await;

        edge.pairs(vec![(NORTH, EAST)]).await;
        edge.ended_by_the_edge(SOUTH).await;
        edge.link(SOUTH).await;
        assert!(edge.hello(SOUTH).players.is_empty());
        let merged = Reply::resumed()
            .entry(absorbed(NORTH, 1, 0, &[]))
            .stay(1, 5, 0);
        edge.answer(SOUTH, merged).await;
        let said = edge.sent(SOUTH).await;
        assert_eq!(asked_before_numbered(&said, Role::Viewer), view(NORTHERN));
        assert!(client.connected());
        assert_eq!(edge.acts(&client).await, (SOUTH, EntityId(5), 1));
        edge.end().await;
    }

    /// What the scenarios of a split begin with: a player of the west, at `HOME`, who
    /// has made two steps that the west has not said it applied. Returns the number
    /// of the edge's last message that the west has applied.
    async fn before_a_split() -> (Harness, Client, u64) {
        let mut edge = Harness::witnessed().await;
        let client = edge.settler(1, 5, WEST, HOME).await;
        let applied = edge.at(WEST).numbered;
        edge.walks(&client, HOME, 2).await;
        edge.quiet().await;
        edge.lose(WEST).await;
        (edge, client, applied)
    }

    /// Scenario 26. A stay that a `SplitOff` names and the edge has under the region
    /// that says it is the new region's: its view is asked of the new region in the
    /// hello of its first link, and what the player did is sent there after its
    /// welcome, numbered from 1, without an arrival. Of a stay the edge does not have
    /// nothing is said until the new region's presence shows it.
    #[tokio::test]
    async fn a_stay_that_was_split_off_is_the_new_regions_without_an_arrival() {
        let (mut edge, mut client, applied) = before_a_split().await;
        edge.link(WEST).await;
        assert_eq!(edge.hello(WEST).players, [player(1)]);
        let split = Reply::resumed().applied(applied);
        // The hello named the player, who is no longer the west's: `Absent`.
        edge.answer(WEST, split.entry(split_off(PART, &[(1, 5), (2, 6)])))
            .await;
        let said = edge.sent(WEST).await;
        let step = step_into(HOME);
        // What the west had not applied it is sent again, and answers itself.
        let expected = [
            (applied + 1, passed_on(1, 5, 1, step.clone())),
            (applied + 2, passed_on(1, 5, 2, step.clone())),
        ];
        assert_eq!(numbered(&said), expected);
        let asked = subscriptions(said);
        let guests = BTreeSet::from([(Some(Role::Guest), view(HOME))]);
        assert_eq!(meanings(&asked), guests);
        assert!(client.connected());
        for region in [EAST, NORTH] {
            let said = edge.sent(region).await;
            assert!(said.is_empty(), "{region:?} was sent {said:?}");
        }

        edge.link(PART).await;
        let hello = edge.hello(PART);
        assert_eq!((hello.since, hello.seen), (0, 0));
        assert_eq!(hello.players, [player(1)]);
        assert_eq!(set(&hello.chunks), view(HOME));
        assert!(hello.guests.is_empty(), "{hello:?}");
        let part = Reply::unknown(40).stay(1, 5, 1).stay(2, 6, 0);
        edge.answer(PART, part).await;
        let said = edge.sent(PART).await;
        let expected = [
            (1, passed_on(1, 5, 1, step.clone())),
            (2, passed_on(1, 5, 2, step)),
            (3, left(2, Some(6))),
        ];
        assert_eq!(numbered(&said), expected);
        assert!(client.connected());
        assert_eq!(edge.acts(&client).await, (PART, EntityId(5), 3));
        edge.end().await;
    }

    /// Scenario 27. The link to the new region comes before the `SplitOff` is read:
    /// its hello names nobody, its `Present` moves the stay as in scenario 3, and the
    /// `SplitOff` then names a stay the edge has under the new region already.
    #[tokio::test]
    async fn a_new_region_linked_before_the_split_off_is_read_says_whom_it_has() {
        let (mut edge, mut client, applied) = before_a_split().await;
        edge.link(PART).await;
        let hello = edge.hello(PART);
        assert!(hello.players.is_empty() && hello.chunks.is_empty() && hello.guests.is_empty());
        edge.answer(PART, Reply::unknown(40).stay(1, 5, 1)).await;
        let said = edge.sent(PART).await;
        assert!(arrival_of(&said, 1).is_none(), "{said:?}");
        assert_eq!(asked_before_numbered(&said, Role::Viewer), view(HOME));
        // What the part had not applied is sent to it, with the stay's entity. (That
        // nothing else is sent is scenario 3's to say.)
        let sent = numbered(&said);
        let steps = inputs_of(&said, 1);
        assert_eq!(steps.len(), sent.len(), "{sent:?}");
        let last = steps.last().map(|(_, entity, step)| (*entity, *step));
        assert_eq!(last, Some((EntityId(5), 2)), "{sent:?}");

        edge.link(WEST).await;
        let hello = edge.hello(WEST);
        assert!(hello.players.is_empty() && hello.chunks.is_empty());
        assert_eq!(set(&hello.guests), view(HOME));
        let split = Reply::resumed().applied(applied);
        edge.answer(WEST, split.entry(split_off(PART, &[(1, 5)])))
            .await;
        let said = edge.sent(PART).await;
        assert!(said.is_empty(), "the SplitOff changed something: {said:?}");
        let said = edge.sent(WEST).await;
        assert!(arrival_of(&said, 1).is_none(), "{said:?}");
        assert!(subscriptions(said).is_empty());
        assert!(client.connected());
        assert_eq!(edge.acts(&client).await, (PART, EntityId(5), 3));
        edge.end().await;
    }

    /// Scenarios 26 and 27 with the two links' messages in every order: the stay ends
    /// at the new region, nobody arrives anywhere, and what the new region had not
    /// applied reaches it with the stay's entity.
    #[tokio::test]
    async fn a_split_off_and_the_new_regions_presence_in_any_order_leave_the_stay_at_the_new_region()
     {
        let mut orders = Orders::new();
        while orders.another() {
            let (mut edge, mut client, applied) = before_a_split().await;
            let split = Reply::resumed().applied(applied);
            let split = split.entry(split_off(PART, &[(1, 5)]));
            let part = Reply::unknown(40).stay(1, 5, 1);
            let scripts = vec![
                vec![Step::Link(WEST), Step::Welcome(WEST, split)],
                vec![Step::Link(PART), Step::Welcome(PART, part)],
            ];
            edge.play(scripts, &mut orders).await;
            assert!(client.connected());
            let said = edge.sent(PART).await;
            assert!(arrival_of(&said, 1).is_none(), "{said:?}");
            let steps = inputs_of(&said, 1);
            let second =
                |(_, entity, step): &(u64, EntityId, u64)| (*entity, *step) == (EntityId(5), 2);
            assert!(steps.iter().any(second), "{steps:?}");
            assert_eq!(edge.at(PART).chunks(Role::Viewer), view(HOME));
            let said = edge.sent(WEST).await;
            assert!(arrival_of(&said, 1).is_none(), "{said:?}");
            assert!(edge.at(WEST).chunks(Role::Viewer).is_empty());
            assert_eq!(edge.acts(&client).await, (PART, EntityId(5), 3));
            edge.end().await;
        }
    }

    /// Scenario 28, the sequence of the review's defect 1, in every order of the two
    /// links' messages. The new region has the player and lets them go back to the
    /// region that was split. Read after that, the `SplitOff` finds the stay under
    /// the region that says it, with its arrival kept for that region, and leaves it.
    #[tokio::test]
    async fn a_stay_that_walked_back_from_the_new_region_is_not_taken_there_again_by_the_split_off()
    {
        let mut orders = Orders::new();
        while orders.another() {
            let (mut edge, mut client, applied) = before_a_split().await;
            let split = Reply::resumed().applied(applied);
            let split = split.entry(split_off(PART, &[(1, 5)]));
            let part = Reply::unknown(40).stay(1, 5, 1);
            let back = Durable::Departed {
                player: player(1),
                transfer: transfer(EntityId(5), 2, HOME),
                to: WEST,
            };
            let scripts = vec![
                vec![Step::Link(WEST), Step::Welcome(WEST, split)],
                vec![
                    Step::Link(PART),
                    Step::Welcome(PART, part),
                    Step::Entry(PART, back),
                ],
            ];
            edge.play(scripts, &mut orders).await;
            assert!(client.connected());
            let said = edge.sent(WEST).await;
            let arrivals: Vec<_> = said
                .iter()
                .filter(|message| matches!(message.body, EdgeToWorker::PlayerArrive { .. }))
                .collect();
            assert_eq!(arrivals.len(), 1, "{said:?}");
            let arrived = arrival_of(&said, 1).map(|(_, transfer)| transfer);
            assert_eq!(arrived, Some(transfer(EntityId(5), 2, HOME)));
            assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));
            assert_eq!(edge.acts(&client).await, (WEST, EntityId(5), 3));
            edge.end().await;
        }
    }

    /// Scenario 29, the review's defect 3. The west split both players into the part,
    /// and the part split the first into a second part; the edge has read neither.
    /// With the links to the west and to the part read in every order nobody is
    /// disconnected: nobody is judged absent on another region's entry. After the
    /// second part's presence the first player is there.
    #[tokio::test]
    async fn two_splits_read_in_any_order_disconnect_nobody() {
        let mut orders = Orders::new();
        while orders.another() {
            let mut edge = Harness::witnessed().await;
            let mut first = edge.settler(1, 5, WEST, HOME).await;
            let mut second = edge.settler(2, 6, WEST, HOME).await;
            let applied = edge.at(WEST).numbered;
            edge.lose(WEST).await;
            let west = Reply::resumed().applied(applied);
            let west = west.entry(split_off(PART, &[(1, 5), (2, 6)]));
            let part = Reply::unknown(40)
                .entry(split_off(SECOND_PART, &[(1, 5)]))
                .stay(2, 6, 0);
            let scripts = vec![
                vec![Step::Link(WEST), Step::Welcome(WEST, west)],
                vec![Step::Link(PART), Step::Welcome(PART, part)],
            ];
            edge.play(scripts, &mut orders).await;
            assert!(first.connected() && second.connected());

            edge.link(SECOND_PART).await;
            edge.answer(SECOND_PART, Reply::unknown(41).stay(1, 5, 0))
                .await;
            assert!(first.connected() && second.connected());
            assert_eq!(edge.acts(&first).await, (SECOND_PART, EntityId(5), 1));
            assert_eq!(edge.acts(&second).await, (PART, EntityId(6), 1));
            edge.end().await;
        }
    }

    /// Scenario 30, and A3 of ADR-0014, the review's defect 6. The west split the
    /// player into the part, which the east then absorbed; the edge had no link
    /// meanwhile. Whichever of the two welcomes is read first, and however their
    /// messages fall between each other, the stay ends at the east: a `SplitOff`
    /// that names the part, read when the part stands for the east, puts it there.
    /// What the player did since the west last applied something reaches the east.
    #[tokio::test]
    async fn a_stay_split_off_into_a_part_that_was_absorbed_ends_at_the_parts_survivor() {
        let mut orders = Orders::new();
        while orders.another() {
            let (mut edge, mut client, applied) = before_a_split().await;
            edge.lose(EAST).await;
            let split = Reply::resumed().applied(applied);
            let split = split.entry(split_off(PART, &[(1, 5)]));
            let merged = Reply::resumed()
                .entry(absorbed(PART, 40, 0, &[]))
                .stay(1, 5, 1);
            let scripts = vec![
                vec![Step::Link(WEST), Step::Welcome(WEST, split)],
                vec![Step::Link(EAST), Step::Welcome(EAST, merged)],
            ];
            edge.play(scripts, &mut orders).await;
            assert!(client.connected());
            let said = edge.sent(EAST).await;
            assert!(arrival_of(&said, 1).is_none(), "{said:?}");
            let steps = inputs_of(&said, 1);
            let second =
                |(_, entity, step): &(u64, EntityId, u64)| (*entity, *step) == (EntityId(5), 2);
            assert!(steps.iter().any(second), "{steps:?}");
            assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(HOME));
            assert_eq!(edge.acts(&client).await, (EAST, EntityId(5), 3));
            edge.offers_in_vain(PART).await;
            edge.end().await;
        }
    }

    /// Scenario 31. An action is never sent to a region the edge is not asking for
    /// its chunk: where it has no subscription there and someone sees the chunk, it
    /// becomes a guest first, on that link, so that the region holds the action until
    /// it has the chunk loaded (rule 34).
    #[tokio::test]
    async fn an_action_sent_on_to_a_region_is_asked_for_there_first() {
        let mut edge = Harness::witnessed().await;
        let mut client = edge.settler(1, 5, WEST, HOME).await;
        let first = breaking(player(1), edge.sequence(), COMMON);
        edge.say(WEST, remote(&first, Some(EAST)));
        edge.settle(WEST).await;
        let said = edge.sent(EAST).await;
        let expected = [
            EdgeMessage::unnumbered(as_guest(1, [COMMON])),
            EdgeMessage {
                number: Some(1),
                body: EdgeToWorker::Remote(first),
            },
        ];
        assert_eq!(said, expected);

        // The edge is asking for the chunk there by now.
        let second = breaking(player(1), edge.sequence(), COMMON);
        let sent_on = Durable::NotMine {
            what: Misdirected::Remote(second.clone()),
            holder: EAST,
        };
        edge.say(NORTH, sent_on);
        edge.settle(NORTH).await;
        let said = edge.sent(EAST).await;
        let expected = [EdgeMessage {
            number: Some(2),
            body: EdgeToWorker::Remote(second),
        }];
        assert_eq!(said, expected);

        // Nobody sees this chunk, so there is nothing to ask for.
        let unseen = ChunkPos::new(40, 40);
        let third = breaking(player(1), edge.sequence(), unseen);
        edge.say(WEST, remote(&third, Some(EAST)));
        edge.settle(WEST).await;
        let said = edge.sent(EAST).await;
        let expected = [EdgeMessage {
            number: Some(3),
            body: EdgeToWorker::Remote(third),
        }];
        assert_eq!(said, expected);

        // A region without a link is asked in its hello, which holds what follows.
        let fourth = breaking(player(1), edge.sequence(), FAR);
        edge.say(WEST, remote(&fourth, Some(SOUTH)));
        edge.settle(WEST).await;
        edge.link(SOUTH).await;
        assert_eq!(edge.hello(SOUTH).guests, [FAR]);
        edge.answer(SOUTH, Reply::unknown(1)).await;
        let said = edge.sent(SOUTH).await;
        assert_eq!(numbered(&said), [(1, EdgeToWorker::Remote(fourth))]);
        assert!(client.connected());
        edge.end().await;
    }

    /// A1 of ADR-0014. The hello to the survivor names only what the edge had there.
    /// The `Absorbed` and the absorbed region's unconfirmed entry are among the
    /// welcome's entries, and the presence answers have the absorbed region's stay,
    /// unnamed, behind the one the hello named.
    #[tokio::test]
    async fn a_hello_to_a_survivor_names_what_the_edge_had_there_and_the_answers_bring_the_rest() {
        let mut edge = Harness::witnessed().await;
        let mut north = edge.settler(1, 5, NORTH, NORTHERN).await;
        let mut east = edge.settler(3, 7, EAST, EASTERN).await;
        // The north deals with an action of the east's player, and the edge does not
        // read of that before the north is absorbed.
        let sequence = edge.under_way(3, NORTH).await.sequence;
        edge.walks(&north, NORTHERN, 1).await;
        edge.quiet().await;
        assert_eq!(edge.at(NORTH).numbered, 3);
        assert_eq!(edge.at(EAST).numbered, 1);
        let next = edge.outbox[NORTH.0 as usize] + 1;
        edge.lose(NORTH).await;
        edge.lose(EAST).await;
        edge.link(EAST).await;
        let hello = edge.hello(EAST);
        assert_eq!(hello.players, [player(3)]);
        assert_eq!(set(&hello.chunks), view(EASTERN));
        // Of what its player saw on the way in, the first player still sees some.
        let on_the_way = minus(&view(HOME), &view(EASTERN));
        assert_eq!(set(&hello.guests), both(&on_the_way, &view(NORTHERN)));

        // The north had applied the arrival and the action, and not the step.
        let reply = Reply::resumed()
            .applied(1)
            .entry(absorbed(NORTH, 1, 2, &[next]))
            .entry(done(player(3), sequence))
            .stay(3, 7, 0)
            .stay(1, 5, 0);
        edge.answer(EAST, reply).await;
        edge.acknowledged(&mut east, sequence).await;
        let said = edge.sent(EAST).await;
        // A `Subscribe` for each chunk the east was not asked for as a viewer already.
        let new = minus(&view(NORTHERN), &view(EASTERN));
        assert_eq!(asked_before_numbered(&said, Role::Viewer), new);
        assert_eq!(
            numbered(&said),
            [(2, passed_on(1, 5, 1, step_into(NORTHERN)))]
        );
        let mut views = view(EASTERN);
        views.extend(view(NORTHERN));
        assert_eq!(edge.at(EAST).chunks(Role::Viewer), views);
        assert!(north.connected() && east.connected());
        assert_eq!(edge.acts(&north).await, (EAST, EntityId(5), 2));
        assert_eq!(edge.acts(&east).await, (EAST, EntityId(7), 1));
        edge.end().await;
    }

    /// A4 of ADR-0014. A player leaves while their region has no link, and the region
    /// is absorbed. The survivor has the stay and says so, unnamed. The edge does not
    /// have it: every leave the survivor is sent names the stay's entity, be it the
    /// one that was kept for the absorbed region or the one the answer calls for.
    #[tokio::test]
    async fn a_stay_of_a_player_who_left_is_ended_at_the_survivor_of_their_region() {
        let mut edge = Harness::witnessed().await;
        let client = edge.settler(1, 5, NORTH, NORTHERN).await;
        edge.caught_up(NORTH);
        edge.quiet().await;
        edge.lose(NORTH).await;
        edge.leave(&client).await;
        edge.lose(EAST).await;
        edge.link(EAST).await;
        assert!(edge.hello(EAST).players.is_empty());
        let reply = Reply::resumed()
            .entry(absorbed(NORTH, 1, 1, &[]))
            .stay(1, 5, 0);
        edge.answer(EAST, reply).await;
        let said = edge.sent(EAST).await;
        let sent = numbered(&said);
        assert!(!sent.is_empty(), "the stay is left at the survivor");
        let names_the_stay = |(_, body): &(u64, EdgeToWorker)| *body == left(1, Some(5));
        assert!(sent.iter().all(names_the_stay), "{sent:?}");
        let asked = subscriptions(said);
        assert!(asked.is_empty(), "{asked:?}");
        edge.end().await;
    }

    /// A5 of ADR-0014. A player of the north walks and leaves while the north has no
    /// link, and joins again; the west, where they join, absorbs the north meanwhile.
    /// The join is sent first and begins a new stay; what was kept for the north
    /// follows under the west's numbers and names the old entity, so it moves nobody
    /// and ends nothing; and the new stay's first input is numbered 1.
    #[tokio::test]
    async fn what_an_earlier_stay_did_is_sent_to_the_survivor_under_the_entity_it_had() {
        let mut edge = Harness::witnessed().await;
        let before = edge.settler(1, 5, NORTH, NORTHERN).await;
        edge.caught_up(NORTH);
        edge.quiet().await;
        let applied = edge.at(WEST).numbered;
        edge.lose(NORTH).await;
        edge.walks(&before, NORTHERN, 2).await;
        edge.leave(&before).await;
        edge.lose(WEST).await;
        let mut again = edge.join(player(1)).await;
        edge.drained().await;
        edge.link(WEST).await;
        assert_eq!(edge.hello(WEST).players, [player(1)]);
        let reply = Reply::resumed()
            .applied(applied)
            .entry(absorbed(NORTH, 1, 1, &[]))
            .stay(1, 5, 0);
        edge.answer(WEST, reply).await;
        let said = edge.sent(WEST).await;
        let sent = numbered(&said);
        assert_eq!(sent.len(), 4, "{sent:?}");
        assert!(
            matches!(&sent[0], (join, EdgeToWorker::PlayerJoin(_)) if *join == applied + 1),
            "{sent:?}"
        );
        let step = step_into(NORTHERN);
        let expected = [
            (applied + 2, passed_on(1, 5, 1, step.clone())),
            (applied + 3, passed_on(1, 5, 2, step)),
            (applied + 4, left(1, Some(5))),
        ];
        assert_eq!(sent[1..], expected);
        let asked = subscriptions(said);
        assert!(asked.is_empty(), "placed as their old self: {asked:?}");

        edge.says(WEST, spawned(player(1), EntityId(8))).await;
        assert_eq!(edge.acts(&again).await, (WEST, EntityId(8), 1));
        assert!(again.connected());
        edge.end().await;
    }

    /// A6 of ADR-0014. The east is split with the player in the part. The player
    /// leaves, joins again and walks into the part before the edge has said hello to
    /// it. The part's presence shows the old stay; the edge has the player there with
    /// another entity, so that stay is the part's to end, behind the arrival that
    /// takes its place, and the edge's own is untouched.
    #[tokio::test]
    async fn an_earlier_stay_in_a_part_is_ended_behind_the_arrival_of_the_later_one() {
        let mut edge = Harness::witnessed().await;
        let before = edge.settler(1, 5, EAST, EASTERN).await;
        edge.caught_up(EAST);
        edge.quiet().await;
        edge.lose(EAST).await;
        edge.leave(&before).await;
        let mut again = edge.settler(1, 8, PART, EASTERN).await;

        edge.link(PART).await;
        let hello = edge.hello(PART);
        assert_eq!(hello.players, [player(1)]);
        assert_eq!(set(&hello.chunks), view(EASTERN));
        edge.answer(PART, Reply::unknown(40).stay(1, 5, 0)).await;
        let said = edge.sent(PART).await;
        let arrival = EdgeToWorker::PlayerArrive {
            player: player(1),
            transfer: transfer(EntityId(8), 0, EASTERN),
        };
        assert_eq!(numbered(&said), [(1, arrival), (2, left(1, Some(5)))]);
        assert!(again.connected());

        // The east's entry names the old stay, which the edge does not have.
        edge.link(EAST).await;
        assert!(edge.hello(EAST).players.is_empty());
        let split = Reply::resumed().applied(1);
        edge.answer(EAST, split.entry(split_off(PART, &[(1, 5)])))
            .await;
        let said = edge.sent(EAST).await;
        assert_eq!(numbered(&said), [(2, left(1, Some(5)))]);
        let said = edge.sent(PART).await;
        assert!(said.is_empty(), "{said:?}");
        assert!(again.connected());
        assert_eq!(edge.acts(&again).await, (PART, EntityId(8), 1));
        edge.end().await;
    }

    /// The history of A8, which the edge has seen nothing of: the east absorbed the
    /// north, the west was split (the part has the first player), the east was split
    /// (the second part has the second player, who had been the north's), the south
    /// absorbed the part, and the west absorbed the second part. Three regions live;
    /// the scripts are their answers to a hello. `whole` gives the order in which
    /// the three are answered one after the other, in place of `orders`.
    async fn after_three_merges_and_two_splits(orders: &mut Orders, whole: Option<[usize; 3]>) {
        let mut edge = Harness::witnessed().await;
        let mut first = edge.settler(1, 5, WEST, HOME).await;
        let mut second = edge.settler(2, 6, NORTH, NORTHERN).await;
        let mut third = edge.settler(3, 7, EAST, EASTERN).await;
        let mut fourth = edge.settler(4, 8, WEST, HOME).await;
        let applied = edge.at(WEST).numbered;
        for region in [WEST, EAST, NORTH] {
            edge.lose(region).await;
        }
        let west = Reply::resumed()
            .applied(applied)
            .entry(split_off(PART, &[(1, 5)]))
            .entry(absorbed(SECOND_PART, 50, 0, &[]))
            .stay(2, 6, 0)
            .stay(4, 8, 0);
        let east = Reply::resumed()
            .applied(1)
            .entry(absorbed(NORTH, 1, 1, &[]))
            .entry(split_off(SECOND_PART, &[(2, 6)]))
            .stay(3, 7, 0);
        // The south came by its state for the edge through its merge.
        let south = Reply::unknown(60)
            .entry(absorbed(PART, 40, 0, &[]))
            .stay(1, 5, 0);
        let scripts = vec![
            vec![Step::Link(WEST), Step::Welcome(WEST, west)],
            vec![Step::Link(EAST), Step::Welcome(EAST, east)],
            vec![Step::Link(SOUTH), Step::Welcome(SOUTH, south)],
        ];
        match whole {
            Some(order) => {
                for script in order {
                    edge.play(vec![scripts[script].clone()], orders).await;
                }
            }
            None => edge.play(scripts, orders).await,
        }
        for client in [&mut first, &mut second, &mut third, &mut fourth] {
            assert!(client.connected(), "{:?} was disconnected", client.player);
        }
        // Every stay is where the one region that says `Present` for it has it.
        let (region, entity, _) = edge.acts(&first).await;
        assert_eq!((region, entity), (SOUTH, EntityId(5)));
        let (region, entity, _) = edge.acts(&second).await;
        assert_eq!((region, entity), (WEST, EntityId(6)));
        let (region, entity, _) = edge.acts(&third).await;
        assert_eq!((region, entity), (EAST, EntityId(7)));
        let (region, entity, _) = edge.acts(&fourth).await;
        assert_eq!((region, entity), (WEST, EntityId(8)));
        for region in [NORTH, PART, SECOND_PART] {
            edge.offers_in_vain(region).await;
        }
        edge.end().await;
    }

    /// A8 of ADR-0014, with a hello to each living region in each order.
    #[tokio::test]
    async fn after_three_merges_and_two_splits_every_stay_is_where_its_region_says() {
        let orders = [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ];
        for order in orders {
            let mut told = Orders::new();
            told.account
                .push(format!("the three welcomes in the order {order:?}"));
            after_three_merges_and_two_splits(&mut told, Some(order)).await;
        }
    }

    /// A8 of ADR-0014, with the messages of the three links falling between each
    /// other (rule 50).
    #[tokio::test]
    async fn after_three_merges_and_two_splits_the_order_of_the_links_messages_does_not_matter() {
        let mut orders = Orders::new();
        while orders.another() {
            after_three_merges_and_two_splits(&mut orders, None).await;
        }
    }

    /// A9 of ADR-0014. The east is split with the player in the part, and then
    /// forgets the edge. The part has had a link of the edge that named nobody, of
    /// which the edge read nothing. The east's welcome is `Unknown` with no entries:
    /// the player is gone as far as the edge can tell. The part still has the stay,
    /// and the leave that names its entity ends it there.
    #[tokio::test]
    async fn a_stay_in_a_part_is_ended_there_when_the_region_it_was_split_off_forgot_the_edge() {
        let mut edge = Harness::witnessed().await;
        let mut client = edge.settler(1, 5, EAST, EASTERN).await;
        edge.caught_up(EAST);
        edge.quiet().await;
        edge.lose(EAST).await;
        edge.link(PART).await;
        assert!(edge.hello(PART).players.is_empty());
        edge.lose(PART).await;

        edge.link(EAST).await;
        assert_eq!(edge.hello(EAST).players, [player(1)]);
        edge.answer(EAST, Reply::unknown(2)).await;
        client.disconnected().await;
        let said = edge.sent(EAST).await;
        assert_eq!(numbered(&said), [(1, left(1, Some(5)))]);

        edge.link(PART).await;
        assert!(edge.hello(PART).players.is_empty());
        edge.answer(PART, Reply::unknown(40).stay(1, 5, 0)).await;
        let said = edge.sent(PART).await;
        assert_eq!(numbered(&said), [(1, left(1, Some(5)))]);
        edge.end().await;
    }

    /// A split and the merge that undoes it, both unread: the west's welcome has the
    /// `SplitOff` and then the `Absorbed` of the part. The stay goes to the part and
    /// comes back with what the player did, and the `Present` finds it where it is.
    #[tokio::test]
    async fn a_split_and_the_merge_that_undoes_it_leave_the_stay_where_it_was() {
        let (mut edge, mut client, applied) = before_a_split().await;
        edge.link(WEST).await;
        let reply = Reply::resumed()
            .applied(applied)
            .entry(split_off(PART, &[(1, 5)]))
            .entry(absorbed(PART, 40, 0, &[]))
            .stay(1, 5, 1);
        edge.answer(WEST, reply).await;
        let said = edge.sent(WEST).await;
        assert!(arrival_of(&said, 1).is_none(), "{said:?}");
        let steps = inputs_of(&said, 1);
        let second =
            |(_, entity, step): &(u64, EntityId, u64)| (*entity, *step) == (EntityId(5), 2);
        assert!(steps.iter().any(second), "{steps:?}");
        assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));
        assert!(edge.at(WEST).chunks(Role::Guest).is_empty());
        assert!(client.connected());
        assert_eq!(edge.acts(&client).await, (WEST, EntityId(5), 3));
        edge.offers_in_vain(PART).await;
        edge.end().await;
    }

    /// Section 8, third item. A `Present` of the west from before the split is read
    /// after the part's presence has moved the stay to the part, and moves it back.
    /// The `SplitOff` on the west's next link puts that right.
    #[tokio::test]
    async fn a_split_off_puts_right_a_stay_that_an_answer_from_before_the_split_moved_back() {
        let mut edge = Harness::witnessed().await;
        let mut client = edge.settler(1, 5, WEST, HOME).await;
        let applied = edge.at(WEST).numbered;
        // The west answers a hello, and is split before the edge has read all of it.
        edge.link(WEST).await;
        let welcome = Welcome::Resumed {
            entries: 0,
            presences: 1,
            applied,
        };
        edge.says(WEST, WorkerToEdge::Welcome(welcome)).await;
        edge.link(PART).await;
        assert!(edge.hello(PART).players.is_empty());
        edge.answer(PART, Reply::unknown(40).stay(1, 5, 0)).await;
        edge.sent(PART).await;
        assert_eq!(edge.at(PART).chunks(Role::Viewer), view(HOME));
        edge.says(WEST, present(player(1), EntityId(5))).await;
        edge.sent(WEST).await;
        assert_eq!(edge.at(WEST).chunks(Role::Viewer), view(HOME));
        assert!(client.connected());

        edge.lose(WEST).await;
        edge.link(WEST).await;
        assert_eq!(edge.hello(WEST).players, [player(1)]);
        let split = Reply::resumed().applied(applied);
        edge.answer(WEST, split.entry(split_off(PART, &[(1, 5)])))
            .await;
        assert!(client.connected());
        assert_eq!(edge.acts(&client).await, (PART, EntityId(5), 1));
        edge.sent(PART).await;
        assert_eq!(edge.at(PART).chunks(Role::Viewer), view(HOME));
        edge.end().await;
    }

    /// A link to the survivor that ends at each point of its welcome after a merge.
    /// The next welcome has the entries the edge has not seen and, as every welcome,
    /// the presence answers; what was kept is sent when a welcome's entries are
    /// through, under the same numbers on whichever link that is.
    #[tokio::test]
    async fn a_link_that_ends_at_any_point_of_a_survivors_welcome_loses_nothing() {
        for read in 1..=3 {
            let Merging {
                mut edge,
                mut north,
                west: _west,
                action,
            } = before_a_merge().await;
            edge.lose(NORTH).await;
            edge.lose(EAST).await;
            edge.link(EAST).await;
            let seen = edge.hello(EAST).seen;
            // The welcome, the `Absorbed` and the `Present`, of which the edge reads
            // the first `read` before the link ends.
            let welcome = Welcome::Resumed {
                entries: 1,
                presences: 1,
                applied: 0,
            };
            edge.says(EAST, WorkerToEdge::Welcome(welcome)).await;
            if read >= 2 {
                edge.take(Step::Entry(EAST, absorbed(NORTH, 1, 3, &[])))
                    .await;
            }
            if read >= 3 {
                edge.says(EAST, present_with(player(1), EntityId(5), 2))
                    .await;
            }
            edge.lose(EAST).await;

            edge.link(EAST).await;
            let hello = edge.hello(EAST);
            let mut again = Reply::resumed().stay(1, 5, 2);
            if read >= 2 {
                // The merge is behind the edge: the hello names what was the north's.
                assert_eq!(hello.seen, seen + 1);
                assert_eq!(hello.players, [player(1)]);
                assert_eq!(set(&hello.chunks), view(NORTHERN));
                assert_eq!(set(&hello.guests), set(&[LENT, SOUGHT]));
            } else {
                assert_eq!(hello.seen, seen);
                assert!(hello.players.is_empty() && hello.chunks.is_empty());
                edge.outbox[EAST.0 as usize] = seen;
                again = again.entry(absorbed(NORTH, 1, 3, &[]));
            }
            edge.answer(EAST, again).await;
            let said = edge.sent(EAST).await;
            assert_eq!(
                numbered(&said),
                kept_after_the_merge(&action),
                "read {read}"
            );
            assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(NORTHERN));
            assert_eq!(edge.at(EAST).chunks(Role::Guest), set(&[LENT, SOUGHT]));
            assert!(north.connected());
            assert_eq!(edge.acts(&north).await, (EAST, EntityId(5), 5));
            edge.end().await;
        }
    }

    /// Section 3, step 5. What the absorbed region showed counts as shown by the
    /// survivor, whose snapshot puts it right, and what it served the survivor
    /// serves: an action for which no region is named goes there, behind the
    /// subscription the `Absorbed` made.
    #[tokio::test]
    async fn what_an_absorbed_region_showed_and_served_is_the_survivors_to_put_right() {
        let mut edge = Harness::witnessed().await;
        let mut client = edge.settler(1, 5, WEST, HOME).await;
        let _second = edge.settler(2, 6, WEST, HOME).await;
        edge.says(WEST, elsewhere(LENT, edge.ask(WEST, LENT), NORTH))
            .await;
        edge.sent(NORTH).await;
        let there = vec![stranger(EntityId(70), LENT)];
        let chunk = chunk_with(LENT, STONE);
        edge.says(
            NORTH,
            snapshot_of(LENT, edge.ask(NORTH, LENT), chunk, there),
        )
        .await;
        edge.sync(&mut client).await;
        assert!(client.entities.contains(&70), "{:?}", client.entities);
        assert_eq!(client.state(LENT), Some(STONE));

        edge.lose(NORTH).await;
        edge.lose(EAST).await;
        edge.link(EAST).await;
        edge.answer(EAST, Reply::resumed().entry(absorbed(NORTH, 1, 0, &[])))
            .await;
        let asked = subscriptions(edge.sent(EAST).await);
        assert_eq!(asked, [as_guest(1, [LENT])]);
        // Nothing leaves a screen because a region was absorbed.
        edge.sync(&mut client).await;
        assert!(client.entities.contains(&70), "{:?}", client.entities);
        assert_eq!(client.state(LENT), Some(STONE));

        let action = breaking(player(2), edge.sequence(), LENT);
        edge.say(WEST, remote(&action, None));
        edge.settle(WEST).await;
        let said = edge.sent(EAST).await;
        assert_eq!(numbered(&said), [(1, EdgeToWorker::Remote(action))]);

        let chunk = chunk_with(LENT, GRANITE);
        edge.says(EAST, snapshot_of(LENT, 1, chunk, Vec::new()))
            .await;
        edge.sync(&mut client).await;
        assert!(!client.entities.contains(&70), "{:?}", client.entities);
        assert_eq!(client.state(LENT), Some(GRANITE));
        edge.end().await;
    }

    /// Rule 48, first item, and rule 34. An action that was kept for the absorbed
    /// region goes to the survivor behind the subscription for its chunk, on the
    /// same link, so that the survivor holds it until it has the chunk loaded.
    #[tokio::test]
    async fn an_action_kept_for_an_absorbed_region_follows_the_subscription_for_its_chunk() {
        let mut edge = Harness::witnessed().await;
        let _client = edge.settler(2, 6, WEST, HOME).await;
        let action = edge.under_way(2, NORTH).await;
        edge.quiet().await;
        assert_eq!(edge.at(NORTH).chunks(Role::Guest), set(&[COMMON]));
        edge.lose(NORTH).await;
        edge.lose(EAST).await;
        edge.link(EAST).await;
        assert!(edge.hello(EAST).guests.is_empty());
        edge.answer(EAST, Reply::resumed().entry(absorbed(NORTH, 1, 0, &[])))
            .await;
        let mut said = edge.sent(EAST).await;
        said.retain(|message| !matches!(message.body, EdgeToWorker::Confirm { .. }));
        let expected = [
            EdgeMessage::unnumbered(as_guest(1, [COMMON])),
            EdgeMessage {
                number: Some(1),
                body: EdgeToWorker::Remote(action),
            },
        ];
        assert_eq!(said, expected);
        edge.end().await;
    }

    /// Rule 41, and section 8.9 of ADR-0014. The survivor had forgotten the edge and
    /// came by a state for it through the merge: its welcome is `Unknown`, with the
    /// `Absorbed` among its entries and a `Present` for the stay that came. The edge
    /// gives up what it had at the survivor, its player there among it, and then has
    /// at the survivor what it had at the absorbed region, numbered from 1.
    #[tokio::test]
    async fn a_survivor_that_had_forgotten_the_edge_says_unknown_with_the_absorbed_behind_it() {
        let mut edge = Harness::witnessed().await;
        let mut north = edge.settler(1, 5, NORTH, NORTHERN).await;
        edge.caught_up(NORTH);
        let mut east = edge.settler(3, 7, EAST, EASTERN).await;
        edge.walks(&north, NORTHERN, 1).await;
        edge.quiet().await;
        edge.lose(NORTH).await;
        edge.lose(EAST).await;
        edge.link(EAST).await;
        assert_eq!(edge.hello(EAST).players, [player(3)]);
        let reply = Reply::unknown(9)
            .entry(absorbed(NORTH, 1, 1, &[]))
            .stay(1, 5, 0);
        edge.answer(EAST, reply).await;
        east.disconnected().await;
        let said = edge.sent(EAST).await;
        let expected = [
            (1, left(3, Some(7))),
            (2, passed_on(1, 5, 1, step_into(NORTHERN))),
        ];
        assert_eq!(numbered(&said), expected);
        assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(NORTHERN));
        assert!(north.connected());
        assert_eq!(edge.acts(&north).await, (EAST, EntityId(5), 2));

        edge.link(EAST).await;
        let hello = edge.hello(EAST);
        assert_eq!((hello.since, hello.seen), (9, 1));
        assert_eq!(hello.players, [player(1)]);
        edge.answer(EAST, Reply::resumed().applied(3).stay(1, 5, 2))
            .await;
        assert!(north.connected());
        edge.end().await;
    }

    /// Section 5. A region is owed its `Absorbed` also when all the edge has of it is
    /// a guest's subscription there and a subscription elsewhere that names it. The
    /// survivor's next welcome has none, so the edge concludes that none comes: what
    /// it was asking the absorbed region for it asks the survivor for, and the
    /// subscription that named the one names the other.
    #[tokio::test(start_paused = true)]
    async fn a_region_the_edge_only_asks_for_a_chunk_is_owed_a_word_by_its_survivor() {
        let (mut edge, mut client) = told_elsewhere_with_the_absorbed().await;
        edge.answer(EAST, Reply::resumed()).await;
        tokio::time::advance(Duration::from_millis(2500)).await;
        edge.pairs(vec![(NORTH, EAST)]).await;
        edge.ended_by_the_edge(EAST).await;
        edge.link(EAST).await;
        assert!(edge.hello(EAST).guests.is_empty());
        edge.answer(EAST, Reply::resumed()).await;
        let asked = subscriptions(edge.sent(EAST).await);
        assert_eq!(asked, [as_guest(1, [COMMON])]);
        let asked = subscriptions(edge.sent(WEST).await);
        assert!(asked.is_empty(), "{asked:?}");

        edge.says(EAST, not_mine(COMMON, 1)).await;
        let asked = subscriptions(edge.sent(WEST).await);
        let again = BTreeSet::from([(Some(Role::Viewer), set(&[COMMON]))]);
        assert_eq!(meanings(&asked), again);
        edge.says(WEST, snapshot(COMMON, edge.ask(WEST, COMMON)))
            .await;
        edge.sync(&mut client).await;
        assert!(client.chunks.contains_key(&COMMON));
        edge.offers_in_vain(NORTH).await;
        edge.end().await;
    }

    /// Section 4, last paragraph: the entity of a player who left, on its way to a
    /// region that stands for another, is discarded at the living one.
    #[tokio::test]
    async fn an_entity_on_its_way_to_an_absorbed_region_is_discarded_at_the_survivor() {
        let mut edge = Harness::witnessed().await;
        let client = edge.settler(1, 5, WEST, HOME).await;
        edge.pairs(vec![(NORTH, EAST)]).await;
        edge.ended_by_the_edge(NORTH).await;
        edge.leave(&client).await;
        edge.say(WEST, departed(player(1), EntityId(5), NORTH, NORTHERN));
        edge.settle(WEST).await;
        let said = edge.sent(EAST).await;
        let discard = EdgeToWorker::Discard {
            entity: EntityId(5),
            chunk: NORTHERN,
        };
        assert_eq!(numbered(&said), [(1, discard)]);
        edge.end().await;
    }

    /// A player's own entity as a region has it in a snapshot.
    fn own(who: u128, entity: i32, chunk: ChunkPos) -> EntityState {
        EntityState {
            entity: EntityId(entity),
            kind: EntityKind::Player {
                player: player(who),
                name: "Player".to_owned(),
            },
            pose: Pose::at(within(chunk)),
        }
    }

    /// What the two tests below begin with: the west has shown the first player's
    /// entity, in its snapshot of the chunk they stand in, which a second player
    /// sees. The stay is then the east's, by a split and a merge the edge has not
    /// caught up with: the east says `Present` for it in answer to a hello that named
    /// nobody, and the edge asks the east for the view and sends it the player's
    /// step, as case 3 of ADR-0015, section 2.1, has it.
    async fn a_shown_stay_another_region_has() -> (Harness, Client, Client) {
        let mut edge = Harness::witnessed().await;
        let client = edge.settler(1, 5, WEST, HOME).await;
        let mut second = edge.settler(2, 6, WEST, HOME).await;
        let there = vec![own(1, 5, HOME)];
        edge.says(
            WEST,
            snapshot_of(HOME, edge.ask(WEST, HOME), empty_chunk(), there),
        )
        .await;
        edge.sync(&mut second).await;
        assert!(second.entities.contains(&5), "{:?}", second.entities);
        edge.walks(&client, HOME, 1).await;
        edge.quiet().await;

        edge.link(EAST).await;
        edge.answer(EAST, Reply::resumed().stay(1, 5, 0)).await;
        edge.sent(EAST).await;
        assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(HOME));
        (edge, client, second)
    }

    /// Found by the generated runs of scenario 32. A stay comes to a region without
    /// an arrival, and the region reports the player's next step before it has shown
    /// the edge the player.
    #[tokio::test]
    async fn a_move_that_a_stays_new_region_reports_before_its_snapshot_is_taken() {
        // The sequence: as `a_shown_stay_another_region_has`. The east then applies
        // the step in the tick that takes it, as a move is held by nothing (ADR-0014,
        // section 3.6), and reports it among that tick's events, which come before
        // the tick's snapshots (ADR-0012, section 5.2, and rule 31).
        //
        // The records: the stay is the east's (ADR-0015, section 2.1, case 3;
        // ADR-0014, rule 38), "its view's subscriptions move here as at a
        // hand-over". They are silent on whose word on the player's entity counts
        // from then on. ADR-0013, section 7, takes it only from the region that last
        // introduced the entity; section 3 of ADR-0015 makes the survivor that region
        // at a merge, and its section 8 names the split only for several edges.
        // After a hand-over the new region introduces the entity when it takes the
        // arrival in; here nobody arrives. Read as: the region a stay is in is the
        // one whose word on the player's own entity counts.
        //
        // What happened: the east's `EntityMoved` is passed over. The view stays
        // centred where the west last had the player until the east's snapshot has
        // shown the entity and the player steps again. Where no region had shown the
        // entity before, the step is taken.
        let (mut edge, _first, _second) = a_shown_stay_another_region_has().await;
        edge.says(EAST, walked(EntityId(5), HOME, STEP_EAST)).await;
        edge.sent(EAST).await;
        assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(STEP_EAST));
        edge.end().await;
    }

    /// The same step, reported when the region has shown the entity: it is taken.
    /// This is what the regions of the generated runs do.
    #[tokio::test]
    async fn a_move_that_a_stays_new_region_reports_after_its_snapshot_is_taken() {
        let (mut edge, _first, _second) = a_shown_stay_another_region_has().await;
        let there = vec![own(1, 5, HOME)];
        edge.says(
            EAST,
            snapshot_of(HOME, edge.ask(EAST, HOME), empty_chunk(), there),
        )
        .await;
        edge.says(EAST, walked(EntityId(5), HOME, STEP_EAST)).await;
        edge.sent(EAST).await;
        assert_eq!(edge.at(EAST).chunks(Role::Viewer), view(STEP_EAST));
        edge.end().await;
    }

    /// Found by the generated runs of scenario 32 (seed 33, before the regions of
    /// those runs were kept from it). Two regions each have a viewer of a chunk and
    /// each name the other for it.
    #[tokio::test(start_paused = true)]
    async fn two_regions_that_name_each_other_for_a_chunk_are_asked_again() {
        // The sequence, as the run came to it: the west is pinned to where the chunk
        // lies and is split, and the chunk goes to the part. A player of the west
        // sees it: the west asks the store and says `Elsewhere` with the part. The
        // part gives the chunk back, which makes it the west's again by the store's
        // table, "and nobody tells the pinned region" (ADR-0014, section 2.2). A
        // player of the part sees the chunk: the part asks the store and says
        // `Elsewhere` with the west. Here the two regions are the west and the east.
        //
        // The records: ADR-0013, section 3, has the edge become a guest at the
        // region an `Elsewhere` names only if it has no subscription there, and ask
        // a viewer's region again only on a `NotMine` from a guest's region or when
        // the subscription it pointed at ends. Statement E holds: each subscription
        // names a region where the edge has one. ADR-0014, rule 49, ends such a ring
        // of beliefs for players and actions (a pinned region takes in whoever is
        // sent to it and asks again) and says nothing of subscriptions; by rule 13 of
        // ADR-0012 the west does not learn by itself. Read as: a chunk somebody sees
        // is served in the end, so one of the two is asked again.
        //
        // What happened: nothing is asked of either region again. The chunk is
        // served to nobody for as long as both players see it.
        let mut edge = Harness::witnessed().await;
        let _first = edge.settler(1, 5, WEST, HOME).await;
        let _second = edge.settler(2, 6, EAST, EASTERN).await;
        edge.says(WEST, elsewhere(COMMON, edge.ask(WEST, COMMON), EAST))
            .await;
        edge.sent(EAST).await;
        edge.says(EAST, elsewhere(COMMON, edge.ask(EAST, COMMON), WEST))
            .await;
        let mut asked = Vec::new();
        for _ in 0..3 {
            tokio::time::advance(Duration::from_millis(1100)).await;
            for region in [WEST, EAST] {
                asked.extend(subscriptions(edge.sent(region).await));
            }
        }
        let again = asked.iter().any(|body| names(body, COMMON));
        assert!(
            again,
            "neither region is asked again for the chunk: {asked:?}"
        );
        edge.end().await;
    }
}
