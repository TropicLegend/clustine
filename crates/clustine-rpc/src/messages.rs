//! The messages services exchange.

use clustine_data::BlockState;
use clustine_region::{Layout, RegionId, RoutingTable};
use clustine_sim::api::{
    Durable, EntityState, HOTBAR_SLOTS, ItemStack, PlayerEvent, PlayerInput, PlayerJoin,
    PlayerTransfer, Pose, RegionEvent, RemoteAction,
};
use clustine_world::{BlockPos, Chunk, ChunkPos, EdgeId, EntityId, EntityIds, PlayerId, Vec3};
use serde::{Deserialize, Serialize};

/// A message of an edge to the worker that owns a region, with its number.
///
/// What changes the region (joining, leaving, arriving, discarding, inputs and remote
/// actions) is numbered from 1 per edge and region, so that it can be sent again after a
/// link was lost without any of it being applied twice; see
/// `docs/adr/0008-durable-regions-and-resuming.md`. The rest has no number.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeMessage {
    pub number: Option<u64>,
    pub body: EdgeToWorker,
}

impl EdgeMessage {
    /// A message that is not numbered.
    pub fn unnumbered(body: EdgeToWorker) -> Self {
        Self { number: None, body }
    }
}

/// What an edge tells the worker that owns a region. See
/// `docs/adr/0005-edge-worker-interface.md`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EdgeToWorker {
    /// The first message on a link: which edge this is and what it knows of the region,
    /// so that the region can tell it what it has missed. Answered with
    /// [`WorkerToEdge::Welcome`].
    Hello {
        edge: EdgeId,
        /// Which start of the edge this is. Each start of an edge has a higher number
        /// than the one before.
        start: u64,
        /// The number of the last outbox entry the edge has got from this region; 0 if
        /// none.
        seen: u64,
        /// The players the edge believes to be in this region. Each is answered with
        /// [`WorkerToEdge::Presence`].
        players: Vec<PlayerId>,
        /// Every chunk of this region that the edge shows or has asked for. The link is
        /// subscribed to them.
        chunks: Vec<ChunkPos>,
    },
    /// The edge has handled the outbox entries up to this number; see
    /// [`WorkerToEdge::Outbox`].
    Confirm { number: u64 },
    /// A player has logged in and wants to enter the world.
    PlayerJoin(PlayerJoin),
    /// A player's connection has ended.
    PlayerLeave { player: PlayerId },
    /// A player has walked in from another region, which let them go with
    /// [`PlayerEvent::Departed`].
    PlayerArrive {
        player: PlayerId,
        transfer: PlayerTransfer,
    },
    /// The entity of a player who left while being handed over will not arrive. The
    /// region reports it as removed to those watching `chunk`, where it was seen last.
    Discard { entity: EntityId, chunk: ChunkPos },
    /// Something a player did. An edge numbers the inputs of a player in ascending
    /// order; see `TickInputs::inputs`.
    Input {
        player: PlayerId,
        number: u64,
        input: PlayerInput,
    },
    /// What is left to do of something a player of another region did to blocks of this
    /// one; see [`RemoteAction`]. Answered with [`WorkerToEdge::RemoteDone`] or with
    /// [`WorkerToEdge::Remote`] if yet another region has to take a step.
    Remote(RemoteAction),
    /// The edge wants a snapshot of these chunks, followed by every later change to
    /// them. A subscription also keeps the chunk loaded.
    Subscribe { chunks: Vec<ChunkPos> },
    /// The edge no longer needs these chunks.
    Unsubscribe { chunks: Vec<ChunkPos> },
}

impl EdgeToWorker {
    /// Whether a message of this kind is numbered; see [`EdgeMessage`].
    pub fn is_numbered(&self) -> bool {
        match self {
            Self::PlayerJoin(_)
            | Self::PlayerLeave { .. }
            | Self::PlayerArrive { .. }
            | Self::Discard { .. }
            | Self::Input { .. }
            | Self::Remote(_) => true,
            Self::Hello { .. }
            | Self::Confirm { .. }
            | Self::Subscribe { .. }
            | Self::Unsubscribe { .. } => false,
        }
    }
}

/// What a worker tells an edge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WorkerToEdge {
    /// The state of a subscribed chunk after `tick`, with the entities that are in it.
    /// Sent once per subscription; later changes follow as [`WorkerToEdge::TickDelta`].
    ChunkSnapshot {
        position: ChunkPos,
        tick: u64,
        chunk: Chunk,
        entities: Vec<EntityState>,
    },
    /// What happened during `tick` in the chunks the edge is subscribed to.
    TickDelta { tick: u64, events: Vec<RegionEvent> },
    /// Something that concerns a single player on this edge.
    ToPlayer {
        player: PlayerId,
        event: PlayerEvent,
    },
    /// Something a player did concerns blocks of another region. The edge passes it on
    /// to the region that has the block [`RemoteStep::concerns`] names.
    ///
    /// [`RemoteStep::concerns`]: clustine_sim::api::RemoteStep::concerns
    Remote(RemoteAction),
    /// A [`RemoteAction`] that reached this region has been dealt with. What it changed
    /// was reported before, in the [`WorkerToEdge::TickDelta`] of the same tick, so the
    /// player can now be told that their action with this sequence number was handled.
    RemoteDone { player: PlayerId, sequence: i32 },
    /// The answer to [`EdgeToWorker::Hello`], before anything else on the link.
    Welcome(Welcome),
    /// An entry of the region's outbox for this edge, with its number. It is sent again
    /// on every new link until the edge has confirmed it with [`EdgeToWorker::Confirm`].
    Outbox { number: u64, entry: Durable },
    /// Whether a player named in [`EdgeToWorker::Hello`] is in the region.
    Presence { player: PlayerId, answer: Presence },
    /// How far the edge's messages have been applied and made durable: up to `applied`.
    /// `inputs` has, for each of the edge's players whose last applied input changed,
    /// the number of that input. What the edge keeps to send again it trims on this.
    Progress {
        applied: u64,
        inputs: Vec<(PlayerId, u64)>,
    },
}

/// What a region answers an edge that has said hello.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Welcome {
    /// The region knew the edge with this start and carries on where it was.
    Resumed,
    /// The region did not know the edge with this start: it is new, or the region has
    /// forgotten it. What the edge believed to be in the region is not there.
    Unknown,
    /// The region knows a later start of this edge, so this one has been replaced. The
    /// link is closed.
    Superseded,
}

/// What a region says about a player an edge believes to be in it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Presence {
    /// The player is in the region, as of a tick that is durable.
    Present {
        entity: EntityId,
        pose: Pose,
        hotbar: [Option<ItemStack>; HOTBAR_SLOTS],
        selected_slot: u8,
        /// The number of the last input applied.
        last_input: u64,
        /// The highest sequence number of the player's own actions on blocks of this
        /// region that has been handled, if any.
        handled: Option<i32>,
    },
    Absent,
}

/// What a worker asks of the world store through the handle of a region it has opened.
/// Commits and checkpoints are done in the order they are asked for, and so are saves
/// and loads of one chunk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StoreRequest {
    /// Load the chunk, generating it if it has never been stored. Answered with
    /// [`StoreReply::Loaded`], or [`StoreReply::Unreadable`]. A load that follows a save
    /// of the same chunk finds what was saved.
    Load { position: ChunkPos },
    /// Store the chunk as it is after `tick`. It is written once every commit asked for
    /// before it is on disk, and dropped if one of them failed. Not answered.
    Save {
        position: ChunkPos,
        tick: u64,
        chunk: Chunk,
    },
    /// Record the block changes of `tick` and what else changed in the region's state,
    /// so that neither is lost if the process dies before the chunks they are in have
    /// been saved. `state` is the region's own record of its changes, which the store
    /// keeps as it is. Answered with [`StoreReply::Committed`] once it is on disk, and
    /// not at all if it could not be put there: the handle is lost then.
    Commit {
        tick: u64,
        changes: Vec<(BlockPos, BlockState)>,
        state: Vec<u8>,
    },
    /// `state` is the region's whole state after `tick`, and every change committed up
    /// to `tick` is contained in a chunk saved before this request. Once those saves are
    /// on disk, the store keeps `state` in place of the commits up to `tick`; later ones
    /// are kept. Not answered.
    Checkpoint { tick: u64, state: Vec<u8> },
    /// Answer with [`StoreReply::Flushed`] once everything requested before is done.
    Flush,
}

/// What the world store answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StoreReply {
    Loaded {
        position: ChunkPos,
        chunk: Chunk,
    },
    /// The stored chunk cannot be read. It is not generated instead, which would look
    /// fine at first and then overwrite what players built once it is saved.
    Unreadable {
        position: ChunkPos,
    },
    /// The commit of `tick` is on disk, and so is every one before it.
    Committed {
        tick: u64,
    },
    Flushed,
}

/// What the world store has of a region, as it hands it to the owner that opens it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Restored {
    /// The entity ids the store issued to the region when it was first opened. They are
    /// the region's for good.
    pub entity_ids: EntityIds,
    /// The region's whole state as of its last checkpoint, if it has had one.
    pub state: Option<TickState>,
    /// The state of each commit after that checkpoint, in the order of their ticks.
    /// The block changes of those commits are in the stored chunks already.
    pub deltas: Vec<TickState>,
}

impl Restored {
    /// The tick the region is restored up to: that of the last delta, or of the state if
    /// there are none, or 0 for a region that has never committed anything.
    pub fn tick(&self) -> u64 {
        self.deltas
            .last()
            .or(self.state.as_ref())
            .map_or(0, |state| state.tick)
    }
}

/// What a region handed the store as its state, or as the change of its state, after a
/// tick. The store does not look into it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TickState {
    pub tick: u64,
    pub state: Vec<u8>,
}

/// The world store's answer to a [`RegionHello`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoreWelcome {
    /// The store lets the owner have the region. What the region is restored with
    /// follows in [`RestoredPart`]s, up to one that is the last: a [`Restored`] can be
    /// larger than a message may be. After the last part the connection carries
    /// [`StoreRequest`]s and [`StoreReply`]s.
    Accepted {
        /// [`Restored::entity_ids`].
        entity_ids: EntityIds,
    },
    /// The region has been opened with epoch `seen`, which is higher than the one in the
    /// hello: whoever said hello has been replaced. The connection is closed.
    EpochRefused { seen: u64 },
    /// The connection is closed, for the reason given.
    Refused { reason: String },
}

/// Some of the state and the deltas of a [`Restored`], as they follow a
/// [`StoreWelcome::Accepted`]. The pieces of all parts, in the order they are sent, are
/// the state if there is one and then the deltas in the order of their ticks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoredPart {
    pub pieces: Vec<RestoredPiece>,
    /// Nothing follows this part: the region is restored with what has been sent.
    pub last: bool,
}

/// The state or a delta of a [`Restored`], or as much of one as its part had room for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoredPiece {
    pub of: RestoredItem,
    /// [`TickState::tick`] of what this is a piece of.
    pub tick: u64,
    /// The bytes of [`TickState::state`] that come after those of the pieces before.
    pub bytes: Vec<u8>,
    /// Whether these are the last of its bytes. If not, the next piece, which is the
    /// first of the next part, goes on with the same state or delta.
    pub complete: bool,
}

/// What a [`RestoredPiece`] is a piece of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RestoredItem {
    /// [`Restored::state`].
    State,
    /// One of [`Restored::deltas`].
    Delta,
}

/// What a service says first on a connection to a worker or to the world store: which
/// region the connection is about and who the service takes its owner to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionHello {
    pub region: RegionId,
    /// The epoch of the region's owner; see [`Assignment::epoch`].
    pub epoch: u64,
    /// [`Layout::fingerprint`] of the layout the region is part of.
    pub layout: u64,
}

/// The answer to a [`RegionHello`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegionWelcome {
    /// The connection now carries the messages of the region.
    Accepted,
    /// The connection is closed, for the reason given.
    Refused { reason: String },
}

/// A region a worker has been given to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assignment {
    pub region: RegionId,
    /// Counts the owners the region has had. An owner with a lower epoch than another
    /// has been replaced.
    pub epoch: u64,
    /// The entity ids the region may hand out under this assignment.
    pub entity_ids: EntityIds,
}

/// What a worker or an edge tells the coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToCoordinator {
    /// A worker offers to run regions. Answered with [`FromCoordinator::Assigned`] or
    /// [`FromCoordinator::Refused`].
    RegisterWorker {
        /// Identifies the worker across restarts and reconnections.
        name: String,
        /// Host and port at which edges reach the worker.
        address: String,
        /// What the worker is running already, which is the case when it registers
        /// again after losing its connection, with the fingerprint of the layout those
        /// regions belong to.
        holding: Vec<Assignment>,
        layout: Option<u64>,
    },
    /// The worker is still there, and vouches for the regions it names. A worker that is
    /// silent for too long loses its regions, and so does a region it holds and does not
    /// vouch for.
    Heartbeat { regions: Vec<(RegionId, Vouch)> },
    /// The world store refused to let the worker open `region`, because it has seen an
    /// owner with epoch `seen`, higher than the worker's. The coordinator issues epochs
    /// above it from then on.
    EpochRefused { region: RegionId, seen: u64 },
    /// An edge wants the routing table, now and whenever it changes. Answered with
    /// [`FromCoordinator::Routing`].
    WatchRouting,
}

/// Why a worker vouches for a region it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Vouch {
    /// A commit of the region has been confirmed within the lease.
    Committed,
    /// The region waits for the world store to answer.
    WaitingForStore,
}

/// What the coordinator tells a worker or an edge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FromCoordinator {
    /// To a worker: how the world is divided and which regions the worker is to run.
    /// Sent again whenever that changes.
    Assigned {
        layout: Layout,
        /// Where players enter the world.
        spawn: Vec3,
        assignments: Vec<Assignment>,
    },
    /// To a worker: it cannot take part, for the reason given.
    Refused { reason: String },
    /// To an edge: the current routing table.
    Routing(RoutingTable),
}
