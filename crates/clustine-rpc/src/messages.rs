//! The messages services exchange.

use clustine_data::BlockState;
use clustine_region::{Layout, RegionId, RoutingTable};
use clustine_sim::api::{
    Durable, EntityState, HOTBAR_SLOTS, ItemStack, PlayerEvent, PlayerInput, PlayerJoin,
    PlayerTransfer, Pose, RegionEvent, RemoteAction,
};
use clustine_world::{
    BlockPos, Chunk, ChunkArea, ChunkPos, EdgeId, EntityId, EntityIds, PlayerId, Vec3,
};
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
        /// The `since` of the last welcome the edge has read from this region
        /// ([`Welcome::Unknown`]), 0 if none: the region's word for the numbering the
        /// two share. The region resumes only with an edge that says the one it has.
        /// See `docs/adr/0012-the-tick-on-chunks.md`, section 4.5.
        since: u64,
        /// The number of the last outbox entry the edge has got from this region; 0 if
        /// none.
        seen: u64,
        /// The players the edge believes to be in this region. Each is answered with
        /// [`WorkerToEdge::Presence`].
        players: Vec<PlayerId>,
        /// The chunks that viewers of this region's players see, whoever serves them:
        /// the viewer's subscriptions the link begins with, as [`EdgeToWorker::Subscribe`]
        /// makes them, with the number 0.
        chunks: Vec<ChunkPos>,
        /// The chunks of this region that viewers of other regions' players see: the
        /// guest's subscriptions the link begins with, as
        /// [`EdgeToWorker::SubscribeAsGuest`] makes them, with the number 0. A chunk
        /// that is also among `chunks` is a viewer's.
        ///
        /// Each chunk of both lists holds back what the link sends behind the hello
        /// until its subscription is answered. See
        /// `docs/adr/0012-the-tick-on-chunks.md`, section 4.5.
        guests: Vec<ChunkPos>,
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
    /// The edge subscribes to these chunks for viewers whose players are this region's:
    /// it wants a snapshot of each, followed by every later change to it. Such a
    /// subscription is the region's reason to claim a chunk nobody holds and to keep it
    /// loaded; for a chunk another region holds it is answered with
    /// [`WorkerToEdge::Elsewhere`], and stays, as the region's reason to go on knowing
    /// who holds the chunk. Said again for a chunk that was told elsewhere, it has the
    /// region ask the world store again.
    ///
    /// `ask` numbers the subscription messages of a link (this one,
    /// [`EdgeToWorker::SubscribeAsGuest`] and [`EdgeToWorker::Unsubscribe`]) from 1,
    /// each higher than the one before; the lists of a hello count as number 0. One
    /// that is not above the one before ends the link. An answer carries the number of
    /// the last message that named its chunk. See
    /// `docs/adr/0012-the-tick-on-chunks.md`, section 4.3.
    Subscribe { ask: u64, chunks: Vec<ChunkPos> },
    /// The edge no longer needs these chunks. `ask` is as for
    /// [`EdgeToWorker::Subscribe`].
    Unsubscribe { ask: u64, chunks: Vec<ChunkPos> },
    /// Like [`EdgeToWorker::Subscribe`], for chunks that a viewer of another region's
    /// player sees: the region serves those it holds and answers
    /// [`WorkerToEdge::NotMine`] for the others. It does not claim a chunk because a
    /// guest asks for it, except in an area it is pinned to, and does not give one back
    /// while a guest is subscribed to it. Said for a chunk the link has a viewer's
    /// subscription to, it makes that a guest's, and [`EdgeToWorker::Subscribe`] makes
    /// it a viewer's again; a subscription that is served stays so, without another
    /// snapshot. `ask` is as for [`EdgeToWorker::Subscribe`].
    SubscribeAsGuest { ask: u64, chunks: Vec<ChunkPos> },
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
            | Self::Unsubscribe { .. }
            | Self::SubscribeAsGuest { .. } => false,
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
        /// Which asking this answers: the number of the last subscription message of
        /// the link that named the chunk when the tick ran, 0 for a hello. An edge
        /// passes over an answer with a lower number than its own last message about
        /// the chunk: it is about a subscription the edge has changed or ended since.
        ask: u64,
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
    /// The answer to a viewer's subscription to a chunk this region does not hold: the
    /// world store has said that `region` holds it. The edge subscribes there as a
    /// guest. The subscription stays, and nothing more of the chunk comes on it until
    /// the edge asks again with [`EdgeToWorker::Subscribe`]. `ask` is as in
    /// [`WorkerToEdge::ChunkSnapshot`]. See `docs/adr/0012-the-tick-on-chunks.md`,
    /// section 5.4.
    Elsewhere {
        chunk: ChunkPos,
        ask: u64,
        region: RegionId,
    },
    /// The answer to a guest's subscription to a chunk this region does not hold, which
    /// ends the subscription. The edge asks the viewer's region again. `ask` is as in
    /// [`WorkerToEdge::ChunkSnapshot`].
    NotMine { chunk: ChunkPos, ask: u64 },
}

/// What a region answers an edge that has said hello.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Welcome {
    /// The region knew the edge with this start and with the `since` it said, and
    /// carries on where it was. `entries` outbox entries follow, those above the
    /// hello's `seen`, before anything else.
    Resumed { entries: u32 },
    /// The region does not share a numbering with the edge: it did not know the edge
    /// with this start, or has forgotten it, or knows it since another moment than the
    /// edge said. What the edge believed to be in the region is not there, nothing of
    /// the hello's `seen` was taken, and the edge numbers its messages from 1 again.
    /// `since` is what the edge says in its hellos from now on. `entries` outbox
    /// entries follow, numbered from the region's own first.
    Unknown { since: u64, entries: u32 },
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
    /// [`StoreReply::Loaded`], or [`StoreReply::Unreadable`], or, if the region does
    /// not hold the chunk, with [`StoreReply::NotHeld`]. A load that follows a save of
    /// the same chunk finds what was saved.
    Load { position: ChunkPos },
    /// Store the chunk as it is after `tick`. It is written once every commit asked for
    /// before it is on disk, and dropped if one of them failed. Not answered, unless
    /// the region does not hold the chunk: then nothing is stored, and the answer is
    /// [`StoreReply::NotHeld`].
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
    /// Grant the region these chunks. Each is granted unless another region holds it,
    /// from the tick of the last commit the store has of the region: the region's own
    /// tick can be issued again after a restore, so the claim names none. Answered
    /// with [`StoreReply::Claimed`] once the grants are on disk, behind the answers to
    /// the commits asked for before. See ADR-0011, section 3.2.
    Claim { chunks: Vec<ChunkPos> },
    /// The region gives these chunks back. Every change it made to them is in a save
    /// asked for before this; the chunks are free once those saves are on disk. A chunk
    /// the region was not granted, and the chunk players enter the world in, are left
    /// out. A claim of a chunk that arrives before the chunk is free keeps it. Not
    /// answered; a [`StoreRequest::Flush`] asked for behind it is answered when the
    /// return is on disk. See ADR-0011, section 3.3.
    Return { chunks: Vec<ChunkPos> },
    /// The merge of ADR-0010, section 4: `state` is this region's whole state after
    /// `tick` with the region `absorbed` taken in, whose chunks and pinned areas are
    /// this region's from that tick. The same worker has `absorbed` open, with
    /// `absorbed_epoch`, and neither region has a commit that no checkpoint of it
    /// covers. `tick` has to be above every tick this session has named, in a commit or
    /// in a checkpoint, and above the tick it was restored up to. Answered with
    /// [`StoreReply::Absorbed`] once it is on disk, or with [`StoreReply::Declined`].
    /// See ADR-0011, section 3.6.
    AbsorbCommit {
        absorbed: RegionId,
        /// The epoch the worker opened `absorbed` with, which shows that it is the one
        /// that was told to absorb it and not one whose time for that has run out.
        absorbed_epoch: u64,
        tick: u64,
        state: Vec<u8>,
    },
    /// The split of ADR-0010, section 5: `state` is this region's whole state after
    /// `tick` without the part, and `part` becomes a new region, which nobody with a
    /// lower epoch than `as_epoch` can open. The region has no commit that no
    /// checkpoint of it covers, and `tick` has to be above every tick this session has
    /// named, as for a merge. Answered with [`StoreReply::Split`] once it is on disk,
    /// or with [`StoreReply::Declined`]. See ADR-0011, section 3.7.
    SplitCommit {
        tick: u64,
        state: Vec<u8>,
        part: SplitPart,
        as_epoch: u64,
    },
}

/// What a split makes a new region of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitPart {
    /// The chunks the new region holds, all of which the old one held. The chunk
    /// players enter the world in is not among them.
    pub chunks: Vec<ChunkPos>,
    /// The new region's whole state, as of the split's tick: its ticks go on from the
    /// old region's.
    pub state: Vec<u8>,
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
    /// The answer to [`StoreRequest::Claim`]: the chunks of the claim that the region
    /// holds, whether it was granted them by this claim or held them before, and those
    /// that another region holds.
    Claimed {
        granted: Vec<ChunkPos>,
        foreign: Vec<(ChunkPos, RegionId)>,
    },
    /// The merge asked for with [`StoreRequest::AbsorbCommit`] has happened. `chunks`
    /// are those the region holds through it.
    Absorbed {
        absorbed: RegionId,
        chunks: Vec<ChunkPos>,
    },
    /// The split asked for with [`StoreRequest::SplitCommit`] has happened, and
    /// `region` is the new region. It is not opened by the split: the worker says an
    /// ordinary hello for it with the epoch it named, which the store answers at once,
    /// and runs it from what it has in memory meanwhile.
    Split {
        region: RegionId,
    },
    /// A merge or a split was not done, for the reason given. Nothing has changed, and
    /// the handle is as it was.
    Declined {
        reason: Decline,
    },
    /// The region asked to load or to save a chunk it does not hold, which only the
    /// holder may. Nothing was done. `holder` is the region that holds the chunk, if
    /// one does.
    NotHeld {
        position: ChunkPos,
        holder: Option<RegionId>,
    },
}

/// Why the world store did not do a merge or a split. See ADR-0011, section 7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decline {
    /// `region` has commits that no checkpoint of it covers.
    Uncheckpointed { region: RegionId },
    /// `tick` is not above `named`, the highest tick this session has named in a commit
    /// or a checkpoint or was restored up to.
    Tick { named: u64 },
    /// The region to absorb is not a living region other than the one that asks.
    NoSuchRegion,
    /// The home region is never absorbed, and the home chunk never leaves it.
    Home,
    /// The region to absorb has no owner, or one with another epoch than named.
    NotOpened { epoch: Option<u64> },
    /// The part has a chunk the region does not hold.
    NotHeld { chunk: ChunkPos },
    /// The part has no chunks, or `as_epoch` is 0.
    Malformed,
    /// The record of it would be longer than a record of the log may be.
    TooLarge,
}

/// What the world store has of a region, as it hands it to the owner that opens it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Restored {
    /// The entity ids the store issued to the region when it was first opened. They are
    /// the region's for good. A region that was split off another has none: its block
    /// is empty, with `first` and `end` both 0.
    pub entity_ids: EntityIds,
    /// The region's whole state as of its last checkpoint, or of the merge or split
    /// it went through since, if it has had any of these.
    pub state: Option<TickState>,
    /// The state of each commit after that checkpoint, in the order of their ticks.
    /// The block changes of those commits are in the stored chunks already.
    pub deltas: Vec<TickState>,
    /// The chunks the region was granted, in ascending order, each with the tick of
    /// the region from which it holds the chunk (ADR-0011, section 2). Not among them
    /// are the chunks it holds by being pinned.
    pub held: Vec<(ChunkPos, u64)>,
    /// The areas the region is pinned to: it holds every chunk of them that no region
    /// was granted. The store does not say which those are; a region finds out about
    /// a chunk by claiming it.
    pub pinned: Vec<ChunkArea>,
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

/// What is said first on a connection to the world store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoreHello {
    /// Open a region. Answered with a [`StoreWelcome`], and if that accepts the hello,
    /// the connection is about the region from then on.
    Region(RegionHello),
    /// Send the list of regions and close. Answered with one [`RegionList`]; the
    /// connection is closed without one if the store cannot give it just now, and it
    /// is worth asking again.
    Regions,
}

/// The world store's answer to a [`StoreHello::Region`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoreWelcome {
    /// The store lets the owner have the region. What the region is restored with
    /// follows in [`RestoredPart`]s, up to one that is the last: a [`Restored`] can be
    /// larger than a message may be. After the last part the connection carries
    /// [`StoreRequest`]s and [`StoreReply`]s.
    Accepted {
        /// [`Restored::entity_ids`].
        entity_ids: EntityIds,
        /// [`Restored::pinned`].
        pinned: Vec<ChunkArea>,
    },
    /// The region has been opened with epoch `seen`, which is higher than the one in the
    /// hello: whoever said hello has been replaced. The connection is closed.
    EpochRefused { seen: u64 },
    /// The region has been absorbed by the region `into`, and is none any more. The
    /// connection is closed.
    Absorbed { into: RegionId },
    /// The connection is closed, for the reason given.
    Refused { reason: String },
}

/// Some of the state, the deltas and the grants of a [`Restored`], as they follow a
/// [`StoreWelcome::Accepted`]. The pieces of all parts, in the order they are sent, are
/// the state if there is one, then the deltas in the order of their ticks, and then
/// the grants if there are any.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoredPart {
    pub pieces: Vec<RestoredPiece>,
    /// Nothing follows this part: the region is restored with what has been sent.
    pub last: bool,
}

/// The state, a delta or the grants of a [`Restored`], or as much of one as its part
/// had room for.
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
    /// [`Restored::held`], as [`held_bytes`] makes bytes of it, with a tick of 0. Left
    /// out if the region was granted nothing.
    Held,
}

/// [`Restored::held`] as the bytes of a [`RestoredItem::Held`]: its postcard.
pub fn held_bytes(held: &[(ChunkPos, u64)]) -> Vec<u8> {
    postcard::to_stdvec(held).expect("positions and ticks are serialisable")
}

/// What [`held_bytes`] made bytes of, or `None` if `bytes` are not that.
pub fn held_from_bytes(bytes: &[u8]) -> Option<Vec<(ChunkPos, u64)>> {
    postcard::from_bytes(bytes).ok()
}

/// The regions of a world as the world store has them, for the coordinator. See
/// ADR-0010, section 6, and ADR-0011, section 5.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionList {
    /// The region that holds the chunk players enter the world in.
    pub home: RegionId,
    /// Every region that exists, in the order of their ids.
    pub regions: Vec<RegionInfo>,
    /// The regions that were absorbed, each with the region it went into, which may
    /// have been absorbed since.
    pub absorbed: Vec<(RegionId, RegionId)>,
}

/// What the coordinator learns of a region from the world store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionInfo {
    pub region: RegionId,
    /// The highest epoch the region was opened with; 0 if it never was.
    pub epoch: u64,
    /// The smallest box of chunks that has every chunk the region was granted, if it
    /// was granted any. The areas it is pinned to are not in it.
    pub bounds: Option<ChunkBox>,
    /// The areas the region is pinned to: it holds every chunk of them that no region
    /// was granted. A region comes to several by absorbing regions that are pinned.
    pub pinned: Vec<ChunkArea>,
}

/// A box of chunks, with both corners in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkBox {
    pub min: ChunkPos,
    pub max: ChunkPos,
}

/// The chunks of a region that have players in them, each with how many: what a worker
/// tells the coordinator, which merges and splits regions by it. See ADR-0010,
/// section 7.
pub type Crowds = Vec<(ChunkPos, u32)>;

/// What a service says first on a connection to a worker, and, in a
/// [`StoreHello::Region`], on one to the world store: which region the connection is
/// about and who the service takes its owner to be.
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
    /// A worker has let go of `region`, which it held with `epoch`: in answer to
    /// [`FromCoordinator::Release`], or by itself because it is about to stop. The
    /// region is closed at the world store and can be given to another worker at once.
    /// See `docs/adr/0009-moving-a-region.md`.
    Released { region: RegionId, epoch: u64 },
    /// A worker has been told to stop. It is given nothing new, and what it runs is
    /// moved to workers that wait, as far as there are any. The coordinator closes the
    /// connection once the worker owns nothing any more.
    Leaving,
    /// Whoever operates the cluster wants `region` moved: to the worker named `to`, or
    /// to any worker that waits. Answered with [`FromCoordinator::MoveRefused`], or with
    /// [`FromCoordinator::MoveBegun`] and later [`FromCoordinator::MoveDone`].
    Move {
        region: RegionId,
        to: Option<String>,
    },
    /// A worker says where the players of its regions are. Sent with heartbeats. See
    /// ADR-0010, section 7. No worker says this yet, and the coordinator ignores it,
    /// as it does the four below.
    Players { regions: Vec<(RegionId, Crowds)> },
    /// Whoever operates the cluster wants `absorbed` merged into `survivor`. Answered
    /// with [`FromCoordinator::Asked`].
    Merge {
        survivor: RegionId,
        absorbed: RegionId,
    },
    /// Whoever operates the cluster wants the players in `chunks`, and what is around
    /// them, split off `region` as a region of its own. Answered with
    /// [`FromCoordinator::Asked`].
    Split {
        region: RegionId,
        chunks: Vec<ChunkPos>,
    },
    /// A worker says what came of [`FromCoordinator::Absorb`]: whether `region` has
    /// absorbed `absorbed`. If not, both are as they were.
    AbsorbEnded {
        region: RegionId,
        absorbed: RegionId,
        done: bool,
    },
    /// A worker says what came of [`FromCoordinator::SplitOff`]: the new region, which
    /// it runs with the epoch it was told, or `None` if the region is as it was.
    SplitEnded {
        region: RegionId,
        part: Option<RegionId>,
    },
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
    /// To a worker: let go of `region`, which you hold with `epoch`, so that another
    /// worker can carry on with it. Answered with [`ToCoordinator::Released`].
    Release { region: RegionId, epoch: u64 },
    /// To whoever asked for a move: it is not done, for the reason given.
    MoveRefused { reason: String },
    /// To whoever asked for a move: the worker `from` has been asked to release the
    /// region for the worker `to`.
    MoveBegun { from: String, to: String },
    /// To whoever asked for a move: the region is the worker `to`'s now, with `epoch`.
    /// `released` says whether its old owner let go of it, or did not answer in time
    /// and was taken for dead. `to` need not be the worker the move began for.
    MoveDone {
        to: String,
        epoch: u64,
        released: bool,
    },
    /// To a worker: have `region`, which you hold with `epoch`, absorb the region
    /// `absorbed`, which nobody runs; open that one with `as_epoch`. Answered with
    /// [`ToCoordinator::AbsorbEnded`]. See ADR-0010, section 4. The coordinator does
    /// not say this yet, and a worker ignores it, as it does the next.
    Absorb {
        region: RegionId,
        epoch: u64,
        absorbed: RegionId,
        as_epoch: u64,
    },
    /// To a worker: split the players standing in `chunks` off `region`, which you
    /// hold with `epoch`, as a region of its own, and run that with `as_epoch`.
    /// Answered with [`ToCoordinator::SplitEnded`]. See ADR-0010, section 5.
    SplitOff {
        region: RegionId,
        epoch: u64,
        chunks: Vec<ChunkPos>,
        as_epoch: u64,
    },
    /// To whoever asked for a merge or a split: what came of it. `region` is the
    /// region that absorbed the other or the one that was split off.
    Asked(Result<RegionId, String>),
}
