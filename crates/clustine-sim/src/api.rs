//! What goes into a region's tick and what comes out of it.
//!
//! These types cross the boundary between the worker and the edge, so they have to stay
//! serialisable and free of anything specific to the Minecraft protocol.

use clustine_data::BlockState;
use clustine_world::{BlockPos, Chunk, ChunkPos, EdgeId, EntityId, PlayerId, RegionId, Vec3};
use serde::{Deserialize, Serialize};

use crate::state::StateDelta;

/// Where an entity is and how it is oriented.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pose {
    /// The position of the feet.
    pub position: Vec3,
    /// Horizontal rotation in degrees; 0 looks towards positive z.
    pub yaw: f32,
    /// Vertical rotation in degrees; negative looks up.
    pub pitch: f32,
    pub on_ground: bool,
}

impl Pose {
    /// Standing at `position`, looking straight ahead towards positive z.
    pub const fn at(position: Vec3) -> Self {
        Self {
            position,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
        }
    }
}

/// The number of slots in a player's hotbar.
pub const HOTBAR_SLOTS: usize = 9;

/// A number of items of one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemStack {
    /// Id in the item registry.
    pub item: i32,
    pub count: i32,
}

/// A side of a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Face {
    /// Towards negative y.
    Bottom,
    /// Towards positive y.
    Top,
    /// Towards negative z.
    North,
    /// Towards positive z.
    South,
    /// Towards negative x.
    West,
    /// Towards positive x.
    East,
}

impl Face {
    /// The block that touches `position` on this side.
    pub fn neighbour(self, position: BlockPos) -> BlockPos {
        match self {
            Self::Bottom => position.offset(0, -1, 0),
            Self::Top => position.offset(0, 1, 0),
            Self::North => position.offset(0, 0, -1),
            Self::South => position.offset(0, 0, 1),
            Self::West => position.offset(-1, 0, 0),
            Self::East => position.offset(1, 0, 0),
        }
    }
}

/// A player entering the region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerJoin {
    pub player: PlayerId,
    pub name: String,
}

/// A player as one region hands them to another: everything a region knows about them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerTransfer {
    /// The player keeps their entity, so that nobody watching sees them vanish.
    pub entity_id: EntityId,
    pub name: String,
    pub pose: Pose,
    pub hotbar: [Option<ItemStack>; HOTBAR_SLOTS],
    pub selected_slot: u8,
    /// The number of the last input the region applied; see [`TickInputs::inputs`].
    pub last_input: u64,
}

/// A player entering or leaving the region. Each but `Discard` names the edge it came
/// from, which is not on the wire: the runner knows which edge a link belongs to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlayerChange {
    /// A player enters the world through the edge, and is that edge's from then on.
    ///
    /// A join begins a new stay whatever the region has: a stay is a player's time in
    /// the world from one join to the leave that ends it, and has one entity for all of
    /// it. If the region has the player already, under this edge or another, they have
    /// connected anew: the entity they had is reported removed and they enter the world
    /// afresh. A join through an edge the region does not know is ignored. See
    /// `docs/adr/0014-merging-and-splitting.md`, section 2.1.
    Join(EdgeId, PlayerJoin),
    /// A player's connection through the edge has ended. It removes the player only if
    /// they are that edge's: what another edge says is about an earlier connection,
    /// which must not end the current one.
    ///
    /// The entity names the stay that has ended: the player is removed only if they
    /// have that entity, as a leave that comes late must not end a later stay. With
    /// none, the leave is for the player whatever their entity.
    Leave(EdgeId, PlayerId, Option<EntityId>),
    /// A player comes in from another region, as that region let them go with
    /// [`Durable::Departed`], and is the edge's from then on.
    ///
    /// Of two stays of a player the one with the higher entity id is the later, so an
    /// arrival takes the place of a player the region has with a lower entity id: the
    /// entity that was there is reported removed where it stood. A player the region
    /// has with a higher entity id or the same stays as they are, and the entity that
    /// was on its way is reported removed if it is another one; so is the entity of an
    /// arrival through an edge the region does not know, as nobody could be told about
    /// the player.
    ///
    /// A player who arrives in a chunk the region believes another to hold is not taken
    /// in: the arrival goes on to that region with a [`Durable::NotMine`]. Not so in
    /// the areas the region is pinned to, where it drops the belief instead. Whatever
    /// else the region knows of the chunk, it takes the player in, and claims the chunk
    /// if it knows nothing of it.
    Arrive(EdgeId, PlayerId, PlayerTransfer),
    /// An entity that another region let go will not arrive anywhere, because its player
    /// left in the meantime. It is reported as removed to those watching `chunk`, where
    /// it was seen last.
    Discard { entity: EntityId, chunk: ChunkPos },
}

/// What kind of thing an entity is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EntityKind {
    Player { player: PlayerId, name: String },
}

/// Everything needed to show an entity to someone who has not seen it before.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityState {
    pub entity: EntityId,
    pub kind: EntityKind,
    pub pose: Pose,
}

impl EntityState {
    /// The chunk the entity is in.
    pub fn chunk(&self) -> ChunkPos {
        ChunkPos::containing(self.pose.position.x, self.pose.position.z)
    }
}

/// Something a player did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlayerInput {
    /// The player moved, turned, or left or touched the ground. Parts that did not
    /// change are absent.
    Move {
        position: Option<Vec3>,
        /// Yaw and pitch in degrees.
        rotation: Option<(f32, f32)>,
        on_ground: bool,
    },
    /// The player broke a block. `sequence` numbers the changes the player's client has
    /// already shown on its own; see [`PlayerEvent::Acknowledged`].
    Dig { position: BlockPos, sequence: i32 },
    /// The player used the item in their hand on `face` of the block at `position`,
    /// which for a block item places it against that face. `sequence` is as for `Dig`.
    UseItemOn {
        position: BlockPos,
        face: Face,
        sequence: i32,
    },
    /// The player selected another hotbar slot, from 0 to 8.
    SelectSlot { slot: u8 },
    /// A creative-mode player put a stack into a hotbar slot, or emptied it.
    SetHotbarSlot { slot: u8, stack: Option<ItemStack> },
}

/// What is left to do of something a player did to blocks that the region the player is
/// in does not have. It goes from region to region, each doing the part that concerns
/// its own blocks, until it has been dealt with.
///
/// A region's players can reach a few blocks beyond where the region ends. What they do
/// there is for the region that has those blocks to decide, as it alone knows them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteAction {
    /// Who did it.
    pub player: PlayerId,
    /// The number the player's client gave the action. Nobody acknowledges it to the
    /// player before the action has been dealt with; see [`Durable::RemoteDone`].
    pub sequence: i32,
    pub step: RemoteStep,
}

/// The next step of a [`RemoteAction`]. The player's region has found the player to be
/// within reach and, for placing, to hold the block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RemoteStep {
    /// Break the block at `position`.
    Break { position: BlockPos },
    /// Place `block` at `target`, provided there is a block at `against` to place it
    /// against. `placer` is where the player's feet are, who must not be built into.
    PlaceAgainst {
        against: BlockPos,
        target: BlockPos,
        block: BlockState,
        placer: Vec3,
    },
    /// Place `block` at `target`, provided the spot is free. The block it is placed
    /// against has been found to be there.
    Place {
        target: BlockPos,
        block: BlockState,
        placer: Vec3,
    },
}

impl RemoteStep {
    /// The block this step is about: the region that has it is the one to take the step.
    pub fn concerns(&self) -> BlockPos {
        match self {
            Self::Break { position } => *position,
            Self::PlaceAgainst { against, .. } => *against,
            Self::Place { target, .. } => *target,
        }
    }
}

/// Something a region tells an edge that must reach it even if the region's owner dies
/// right after: an entry of the region's outbox for that edge. It stays in the outbox, and
/// is sent again on every new link, until the edge has confirmed it. See
/// `docs/adr/0008-durable-regions-and-resuming.md`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
///
/// `Departed`, `Refused`, a `Remote` made of a player's own action and a `NotMine` for
/// an arrival go to the outbox of the edge the player belongs to or arrived through;
/// `RemoteDone`, and a `Remote` or a `NotMine` that answers a remote action, go to the
/// outbox of the edge the action came from.
pub enum Durable {
    /// The player has stepped into a chunk the region believes `to` to hold, and is no
    /// longer in the region. Whoever routes the player passes this on to `to` as
    /// [`PlayerChange::Arrive`], together with every input numbered above
    /// [`PlayerTransfer::last_input`]. Nothing says that the entity is gone: it lives on
    /// in the region it walked into. Passed on to an edge as [`PlayerEvent::Departed`].
    Departed {
        player: PlayerId,
        transfer: PlayerTransfer,
        /// The region the world store said holds the chunk the player stands in.
        to: RegionId,
    },
    /// The player could not enter the world, because the region has no entity id left
    /// for them. Passed on to an edge as [`PlayerEvent::Refused`].
    Refused { player: PlayerId },
    /// What is left of an action concerns a chunk this region does not hold: the one
    /// with the block [`RemoteStep::concerns`] names. The player's own action that is
    /// passed on is not among the acknowledged ones of the tick.
    Remote {
        action: RemoteAction,
        /// The region this one believes to hold that chunk. `None` if it has no answer
        /// of the world store about the chunk: the action is then for the region that
        /// serves the edge the chunk, which the edge knows at first hand. See
        /// `docs/adr/0012-the-tick-on-chunks.md`, section 2.3.
        to: Option<RegionId>,
    },
    /// A remote action has been dealt with, whether or not it changed anything. The
    /// player can now be told so, as with [`PlayerEvent::Acknowledged`]; what it changed
    /// has been reported among the tick's events.
    RemoteDone { player: PlayerId, sequence: i32 },
    /// An arrival or a remote action reached this region for a chunk it believes
    /// `holder` to hold, and goes there. For an arrival the player is on their way as
    /// after a `Departed`. A region that does not know who holds the chunk never says
    /// this: it takes an arrival in, and passes an action on as a `Remote` without a
    /// region. Nor does a region say it of a chunk of its own pinned areas, where it
    /// drops the belief and does the same. See `docs/adr/0012-the-tick-on-chunks.md`,
    /// sections 2.2 and 2.4, and `docs/adr/0014-merging-and-splitting.md`, section 2.2.
    NotMine { what: Misdirected, holder: RegionId },
    /// This region has absorbed `region`, and what follows in the outbox, as far as
    /// `numbers` reaches, is what that region had in its outbox for the edge, under
    /// new numbers. It names no players: the presence answers that follow every
    /// welcome say whom the region has. See
    /// `docs/adr/0014-merging-and-splitting.md`, section 2.3. No region makes this yet.
    Absorbed {
        region: RegionId,
        /// The `since` a welcome of the absorbed region would have said to the edge:
        /// its word for the numbering the two shared. 0 if the absorbed region did not
        /// know the edge, or knew it with a lower start than this region does: it has
        /// then forgotten whatever the edge kept for it.
        since: u64,
        /// The number of the last message of the edge that the absorbed region
        /// applied; 0 likewise.
        applied: u64,
        /// The numbers that the entries behind this one had in the absorbed region's
        /// outbox, in ascending order; none likewise. The edge passes over those it
        /// had seen there already.
        numbers: Vec<u64>,
    },
    /// A part of this region has become the region `region`, and the stays named are
    /// in it from now on: those of the edge's players who went, each with their
    /// entity, in ascending order. Which chunks went is said on the link, anew on
    /// every link. See `docs/adr/0014-merging-and-splitting.md`, section 2.4. No region
    /// makes this yet.
    SplitOff {
        region: RegionId,
        players: Vec<(PlayerId, EntityId)>,
    },
}

/// What a [`Durable::NotMine`] sends on its way again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Misdirected {
    /// A player who was let go to this region, as in [`PlayerChange::Arrive`].
    Arrival {
        player: PlayerId,
        transfer: PlayerTransfer,
    },
    /// A remote action that was passed on to this region.
    Remote(RemoteAction),
}

/// Something the runner tells the region about an edge. See
/// `docs/adr/0008-durable-regions-and-resuming.md`, section 2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EdgeEvent {
    /// The edge is there with this start. If the region knows the edge with a lower
    /// start, the edge is reset: its players are removed and reported so, the entity of
    /// every [`Durable::Departed`] in its outbox, and of every [`Durable::NotMine`] for
    /// an arrival, is reported removed too, its outbox is dropped, and `applied` and
    /// `sent` start from 0 again. An edge the region does not know is noted with nothing
    /// applied or sent. The same start changes nothing, and so does a lower one, which
    /// the runner never passes on.
    Started { edge: EdgeId, start: u64 },
    /// The edge has the outbox entries up to `number`, which are dropped.
    Confirmed { edge: EdgeId, number: u64 },
    /// The edge has been away too long. Its players and the entities of the players on
    /// their way in its outbox are removed as for a reset, and the region forgets the
    /// edge.
    Gone { edge: EdgeId },
}

/// Everything that happened since the previous tick.
///
/// A tick applies all of `edges`, then `applied`, then all of `player_changes`, then all
/// of `remote_actions`, and then all of `inputs`. What players did
/// and what became of them arrives as one sequence, though, and its order is lost when
/// it is sorted into the two. [`TickInputs::change`] and [`TickInputs::input`] sort it
/// so that nothing a player did before they came to the region is applied after it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TickInputs {
    /// What became of edges, in the order it happened. Applied before anything else.
    pub edges: Vec<EdgeEvent>,
    /// For edges whose messages are among these inputs: the number of the last of them.
    /// The region notes it as that edge's [`EdgeState::applied`]; an edge it does not
    /// know is passed over.
    ///
    /// [`EdgeState::applied`]: crate::EdgeState::applied
    pub applied: Vec<(EdgeId, u64)>,
    /// Players entering and leaving, in the order it happened. The order matters: a
    /// player can leave and come back, or join and leave, within one tick.
    pub player_changes: Vec<PlayerChange>,
    /// What players did, in the order it arrived. The inputs of a player are numbered in
    /// ascending order. An input whose number is not above that of the player's last
    /// applied input is ignored, so that inputs can be sent again to the region a player
    /// has moved to without any being applied twice.
    ///
    /// Each names the edge it came through. Only the edge a player belongs to acts for
    /// them: an input through another edge is ignored.
    ///
    /// Each names, between the player and the number, the entity of the stay it is of:
    /// an edge numbers a player's inputs from 1 with every connection, so the number
    /// alone does not say which of two stays an input belongs to. An input for a stay
    /// the region does not have is ignored. See
    /// `docs/adr/0014-merging-and-splitting.md`, section 2.1.
    ///
    /// Only what a player did after the last join or arrival of theirs in
    /// `player_changes` belongs here; see [`TickInputs::change`].
    pub inputs: Vec<(EdgeId, PlayerId, EntityId, u64, PlayerInput)>,
    /// What players of other regions did to blocks of this one, in the order it arrived,
    /// each with the edge that passed it on. It is applied after `player_changes` and
    /// before `inputs`, and answered one for one with a [`Durable::RemoteDone`], a
    /// [`Durable::Remote`] or a [`Durable::NotMine`] for that edge. An action through an
    /// edge the region does not know is ignored, as there is nobody to answer.
    pub remote_actions: Vec<(EdgeId, RemoteAction)>,
    /// Subscriptions of links to chunks that began, each with its kind. The region
    /// counts them on every chunk, whatever it knows of it. A chunk the region holds is
    /// loaded while it has tickets.
    pub tickets_added: Vec<(ChunkPos, Ticket)>,
    /// Subscriptions that ended; one entry releases one ticket of its kind. Additions
    /// are counted first, so that a ticket released and taken again within one tick
    /// keeps the chunk loaded.
    pub tickets_removed: Vec<(ChunkPos, Ticket)>,
    /// Chunks that storage delivered in answer to earlier [`TickOutput::chunk_requests`].
    pub chunks_loaded: Vec<(ChunkPos, Chunk)>,
    /// Chunks the world store has granted the region in answer to earlier
    /// [`TickOutput::claims`]: the region holds them from this tick on. See
    /// `docs/adr/0012-the-tick-on-chunks.md`, section 1.3.
    pub granted: Vec<ChunkPos>,
    /// Chunks the region claimed that another region holds, with that region. The
    /// region believes it for as long as it wants the chunk. Ignored for a chunk the
    /// region holds: it does not unlearn that.
    pub foreign: Vec<(ChunkPos, RegionId)>,
    /// Chunks of which the region is to forget that it believes the region named to
    /// hold them, and to claim again if it wants them. A belief that has changed since
    /// is left alone: it is newer than the doubt.
    pub unbelieve: Vec<(ChunkPos, RegionId)>,
}

/// What a link's subscription to a chunk is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ticket {
    /// For a viewer whose player is this region's. It makes the region claim the chunk
    /// if it knows nothing of it, and go on knowing who holds it otherwise.
    Viewer,
    /// For a viewer whose player is another region's. It makes the region claim the
    /// chunk only in an area the region is pinned to, where a claim takes nothing from
    /// anyone. A chunk the region holds it keeps from being given back, as any ticket
    /// does.
    Guest,
}

impl TickInputs {
    /// Adds what became of a player, which came after everything added so far.
    ///
    /// For a join or an arrival, what that player did before, through any edge and as
    /// far as it waits here for the coming tick, is dropped. The player was not there
    /// then, so the region would have ignored it. Applied after the arrival instead, it
    /// would be taken for what the player did since, and an input sent to the region
    /// while the player was away would be applied ahead of earlier ones that are sent
    /// again with the arrival, which then count as applied already and are lost.
    ///
    /// A leave drops nothing. Whether it applies only the tick can tell: one for a stay
    /// the region does not have changes nothing, and must not take away what the stay
    /// that is there did. Nor is anything lost by keeping it all. A tick applies
    /// changes before inputs, so what a player did before a leave that takes effect
    /// finds nobody; and if they are back within the tick, the join or the arrival
    /// that brings them drops it.
    pub fn change(&mut self, change: PlayerChange) {
        match &change {
            PlayerChange::Join(_, PlayerJoin { player, .. })
            | PlayerChange::Arrive(_, player, _) => {
                self.inputs.retain(|(_, actor, ..)| actor != player);
            }
            PlayerChange::Leave(..) | PlayerChange::Discard { .. } => {}
        }
        self.player_changes.push(change);
    }

    /// Adds something a player did while they had `entity`, as passed on by `edge`,
    /// which came after everything added so far.
    pub fn input(
        &mut self,
        edge: EdgeId,
        player: PlayerId,
        entity: EntityId,
        number: u64,
        input: PlayerInput,
    ) {
        self.inputs.push((edge, player, entity, number, input));
    }
}

/// Something that concerns a single player.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlayerEvent {
    /// The player has entered the world.
    Spawned {
        entity_id: EntityId,
        position: Vec3,
        hotbar: [Option<ItemStack>; HOTBAR_SLOTS],
        selected_slot: u8,
    },
    /// Everything the player did with a sequence number up to `sequence` has been
    /// handled, whether it took effect or not. The client then stops showing its own
    /// guess of the outcome and shows what the region reported.
    Acknowledged { sequence: i32 },
    /// The player has been let go to another region. A region does not emit this
    /// itself: it makes a [`Durable::Departed`], which a worker passes on as this.
    Departed(PlayerTransfer),
    /// The player could not enter the world. A region does not emit this itself: it
    /// makes a [`Durable::Refused`], which a worker passes on as this.
    Refused,
}

/// Something that happened in the region and concerns everyone who can see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RegionEvent {
    /// An entity has come into existence.
    EntitySpawned(EntityState),
    /// An entity has ceased to exist. `chunk` is where it was last.
    EntityRemoved { entity: EntityId, chunk: ChunkPos },
    /// A block has changed.
    BlockChanged {
        position: BlockPos,
        state: BlockState,
    },
    /// An entity has a new pose. At most one per entity and tick.
    EntityMoved {
        entity: EntityId,
        pose: Pose,
        /// The chunk the entity was in before, which differs from the chunk of the new
        /// position when it crossed a chunk border.
        previous_chunk: ChunkPos,
    },
}

impl RegionEvent {
    /// The chunks from which the event can be observed.
    pub fn chunks(&self) -> [ChunkPos; 2] {
        match self {
            Self::EntitySpawned(state) => [state.chunk(); 2],
            Self::EntityRemoved { chunk, .. } => [*chunk; 2],
            Self::BlockChanged { position, .. } => [position.chunk(); 2],
            Self::EntityMoved {
                pose,
                previous_chunk,
                ..
            } => [
                ChunkPos::containing(pose.position.x, pose.position.z),
                *previous_chunk,
            ],
        }
    }
}

/// What a tick resulted in.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TickOutput {
    /// The number of this tick; the first tick of a region is 1.
    pub tick: u64,
    /// [`PlayerEvent::Spawned`], in the order the players joined, and then
    /// [`PlayerEvent::Acknowledged`], in the order of the players. What else concerns a
    /// single player is among `durable`.
    pub player_events: Vec<(PlayerId, PlayerEvent)>,
    pub events: Vec<RegionEvent>,
    /// Chunks that have to be fetched from storage and passed in through
    /// [`TickInputs::chunks_loaded`].
    pub chunk_requests: Vec<ChunkPos>,
    /// The outbox entries made in this tick, each with the edge whose outbox it is in
    /// and its number there. An edge's entries are numbered on from its
    /// [`EdgeState::sent`].
    ///
    /// They are in the order they were made: refusals and the arrivals that are another
    /// region's, in the order of [`TickInputs::player_changes`]; the answers to
    /// [`TickInputs::remote_actions`], one for one and in their order; what players did
    /// to blocks of chunks the region does not hold, in the order of
    /// [`TickInputs::inputs`]; and the players who were let go, in the order of the
    /// players.
    ///
    /// [`EdgeState::sent`]: crate::EdgeState::sent
    pub durable: Vec<(EdgeId, u64, Durable)>,
    /// Chunks the region asks the world store to grant it, in ascending order: those it
    /// wants and knows nothing of. Each is answered once, through
    /// [`TickInputs::granted`] or [`TickInputs::foreign`], and is not claimed again
    /// before.
    pub claims: Vec<ChunkPos>,
    /// Chunks the region gives back to the world store, in ascending order: nothing has
    /// used them for [`RegionConfig::return_after`] ticks. None of them is loaded, and
    /// every change the region made to them is in a save it asked for before.
    ///
    /// [`RegionConfig::return_after`]: crate::RegionConfig::return_after
    pub returns: Vec<ChunkPos>,
    /// Everything that changed in the region's state in this tick.
    pub delta: StateDelta,
}
