//! What goes into a region's tick and what comes out of it.
//!
//! These types cross the boundary between the worker and the edge, so they have to stay
//! serialisable and free of anything specific to the Minecraft protocol.

use clustine_data::BlockState;
use clustine_world::{
    BlockPos, Change, Chunk, ChunkPos, EdgeId, Effect, EffectId, EntityId, FiledEffect, PlayerId,
    RegionId, Vec3,
};
use serde::{Deserialize, Serialize};

use crate::placement::Click;
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

pub use clustine_world::{HOTBAR_SLOTS, INVENTORY_SLOTS, Inventory, ItemStack, OFF_HAND};

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

/// Where a player is and what they hold: what the world store keeps of a player from
/// one stay to the next. See `docs/adr/0020-one-stay-per-player.md`, section 2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub pose: Pose,
    pub flying: bool,
    pub inventory: Inventory,
    pub selected_slot: u8,
}

/// What a region says to the world store about a stay, in the commit of a tick. See
/// `docs/adr/0020-one-stay-per-player.md`, section 3, and [`TickOutput::stays`] for
/// which tick makes which.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StayNote {
    /// The home region has given `player` the stay `entity` and holds it as entering,
    /// or still holds it so and names it again.
    Entering { player: PlayerId, entity: EntityId },
    /// The stay is in this region after the tick, or was let go in it, with `hops`
    /// hand-overs behind it and this place.
    Has {
        player: PlayerId,
        entity: EntityId,
        hops: u32,
        place: Place,
    },
}

/// The world store's answer to a [`StayNote::Entering`]: the stay `entity` of `player`
/// may enter. `place` is where the player was last, if anywhere, and `holder` the
/// region that held the chunk of that place when the store took the note, if one did,
/// which is advice and no more. See `docs/adr/0020-one-stay-per-player.md`, section 4.
///
/// A region does not know its own id, so whoever makes this for it says `None` for a
/// place the region itself was holding: a `holder` is always another region to the
/// tick, which lets the stay go there without placing it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entered {
    pub player: PlayerId,
    pub entity: EntityId,
    pub place: Option<Place>,
    pub holder: Option<RegionId>,
}

/// A player entering the region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerJoin {
    pub player: PlayerId,
    pub name: String,
    /// Which of the edge's connections the join is of: the edge numbers them from 1
    /// with every start. The stay the join begins carries it until its first input is
    /// applied, and every answer to the join names it, so that an edge which has not
    /// been told the stay's entity can tell the stay of this connection from one of an
    /// earlier connection of the same player. See
    /// `docs/adr/0020-one-stay-per-player.md`, sections 4.1 and 4.3.
    pub attempt: u64,
}

/// A player as one region hands them to another: everything a region knows about them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerTransfer {
    /// The player keeps their entity, so that nobody watching sees them vanish.
    pub entity_id: EntityId,
    pub name: String,
    pub pose: Pose,
    pub inventory: Inventory,
    pub selected_slot: u8,
    /// The number of the last input the region applied; see [`TickInputs::inputs`].
    pub last_input: u64,
    /// How often the stay has been handed on from one region to another, this time
    /// counted: a region that lets a player go, and a split for those who go with it,
    /// each add one. Of two copies of a stay the one with more is the later. See
    /// `docs/adr/0020-one-stay-per-player.md`, section 6.
    pub hops: u32,
    /// Whether the player flies; see [`PlayerInput::SetFlying`].
    pub flying: bool,
    /// [`PlayerJoin::attempt`] of the join that began the stay, for as long as no
    /// input of the stay has been applied by any region.
    pub attempt: Option<u64>,
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
    /// have that entity, as a leave that comes late must not end a later stay.
    ///
    /// The last is the attempt of the connection that ended, which an edge names where
    /// it names no entity: the edge has not been told the entity of a player who is
    /// still entering the world. Such a leave ends only a stay of that edge that
    /// carries that attempt, one the region has or one it holds as entering. A stay
    /// carries its attempt until its first input is applied, and an edge sends no
    /// input of a stay whose entity it does not know, so the leave and its stay meet.
    /// A leave that names neither names no stay and ends none. See
    /// `docs/adr/0020-one-stay-per-player.md`, section 4.4.
    Leave(EdgeId, PlayerId, Option<EntityId>, Option<u64>),
    /// A player comes in from another region, as that region let them go with
    /// [`Durable::Departed`], and is the edge's from then on.
    ///
    /// Of two stays of a player the one with the higher entity id is the later, so an
    /// arrival takes the place of a player the region has with a lower entity id: the
    /// entity that was there is reported removed where it stood. A player the region
    /// has with a higher entity id stays as they are, and the entity that was on its
    /// way is reported removed; so is the entity of an arrival through an edge the
    /// region does not know, as nobody could be told about the player.
    ///
    /// Of two copies of one stay the one that was handed on more often is the later
    /// ([`PlayerTransfer::hops`]). An arrival of the stay the region has, as often
    /// handed on or less, changes nothing; one that was handed on more often takes the
    /// place of the copy that is there, and nothing is reported removed, as the entity
    /// lives on. See `docs/adr/0020-one-stay-per-player.md`, section 6.
    ///
    /// An arrival is also passed over if the region holds a later stay of the player
    /// as entering, and whoever loses to a later stay has their edge told with
    /// [`Durable::Ended`]: the edge of the stay that was there, or the edge an arrival
    /// that is passed over came through.
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
    /// which uses the block (a door) or places the item's block against that face.
    /// `cursor` is where on the block the click landed, each from 0 to 1; `sneaking`
    /// whether the player held the sneak key, which places a block in hand against a
    /// door rather than opening it. `sequence` is as for `Dig`. See
    /// `docs/groundwork/w6-plan.md`, section 3.
    UseItemOn {
        position: BlockPos,
        face: Face,
        cursor: [f32; 3],
        sneaking: bool,
        sequence: i32,
    },
    /// The player selected another hotbar slot, from 0 to 8.
    SelectSlot { slot: u8 },
    /// A creative-mode player put a stack into a slot of their inventory, numbered as
    /// [`Inventory`] numbers them, or emptied it.
    SetInventorySlot { slot: u8, stack: Option<ItemStack> },
    /// The player used the item in their hand without a block it uses on: a bucket or
    /// a lily pad, which find what they reach by a ray along `rotation` (yaw, pitch),
    /// the player's rotation from then on. `sneaking` and `sequence` are as for
    /// `UseItemOn`. See `docs/groundwork/w6-plan.md`, section 5.
    UseItem {
        rotation: (f32, f32),
        sneaking: bool,
        sequence: i32,
    },
    /// The player began or stopped flying. The region keeps it, hands it on with the
    /// player and tells the world store with their place; nobody else is shown it. See
    /// `docs/adr/0020-one-stay-per-player.md`, section 13.
    SetFlying { flying: bool },
    /// The player swapped what they hold in the main hand with what they hold in the
    /// off hand (the game's `SWAP_ITEM_WITH_OFFHAND`, the key F). Added at the end, so
    /// that what is stored of inputs reads as it did.
    SwapHands,
    /// A use (`UseItemOn` or `UseItem`) made with the off hand, which a client sends when
    /// the same use with the main hand did nothing. Any other input so wrapped is passed
    /// over.
    WithOffHand(Box<PlayerInput>),
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
    /// The entity of the stay they did it in. A client numbers its actions afresh with
    /// every connection, so the number alone does not say which stay an answer is for.
    /// See `docs/adr/0020-one-stay-per-player.md`, section 11.
    pub entity: EntityId,
    /// The number the player's client gave the action. Nobody acknowledges it to the
    /// player before the action has been dealt with; see [`Durable::RemoteDone`].
    pub sequence: i32,
    pub step: RemoteStep,
}

/// The next step of a [`RemoteAction`]. The player's region has found the player to be
/// within reach and, for placing, to hold the block.
///
/// The first three are what regions passed on before clicks carried what vanilla
/// decides by; a region still takes them, from a state stored by an older build, as
/// it did then. New variants are added at the end, so that what is stored reads as
/// it did (`docs/groundwork/w6-plan.md`, section 4.6).
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
    /// A click on the block at `click.position`, for the region that holds it to use
    /// the block, or to find where a block is placed and place it or pass it on.
    UseOn { click: Click },
    /// Place the item of `click` at `target`, which is not the clicked block. The
    /// clicked block was `clicked` when the region that holds it read it: the region
    /// that holds `target` places against that unless it has the clicked block itself.
    PlaceAt {
        click: Click,
        target: BlockPos,
        clicked: BlockState,
    },
    /// A click as `UseOn`'s, made with the off hand, whose item `click` holds: as the game's
    /// `ServerPlayerGameMode.useItemOn` for the off hand, the clicked block is not used
    /// without its item (`useWithoutItem` is for the main hand only); in Clustine the
    /// block is not used at all, and the item is placed, the uses of a block with an item
    /// in the off hand (a flower into a pot) being a difference to mend when they matter.
    /// Added at the end, so that what is stored of remote actions reads as it did.
    OffHandOn { click: Click },
}

impl RemoteStep {
    /// The block this step is about: the region that has it is the one to take the step.
    pub fn concerns(&self) -> BlockPos {
        match self {
            Self::Break { position } => *position,
            Self::PlaceAgainst { against, .. } => *against,
            Self::Place { target, .. } => *target,
            Self::UseOn { click } | Self::OffHandOn { click } => click.position,
            Self::PlaceAt { target, .. } => *target,
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
/// outbox of the edge the action came from. `Ended` goes to the outbox of the edge of
/// the stay it ends, or, for an arrival that is passed over, of the edge the arrival
/// came through.
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
    /// for them. `attempt` is that of the join it answers. Passed on to an edge as
    /// [`PlayerEvent::Refused`].
    Refused { player: PlayerId, attempt: u64 },
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
    /// has been reported among the tick's events. `entity` is that of the action it
    /// answers.
    RemoteDone {
        player: PlayerId,
        entity: EntityId,
        sequence: i32,
    },
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
    /// `docs/adr/0014-merging-and-splitting.md`, section 2.3.
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
    /// entity and with the attempt the stay carried at the split
    /// ([`PlayerTransfer::attempt`]), in ascending order. Which chunks went is said on
    /// the link, anew on every link. See `docs/adr/0014-merging-and-splitting.md`,
    /// section 2.4.
    SplitOff {
        region: RegionId,
        players: Vec<(PlayerId, EntityId, Option<u64>)>,
    },
    /// The stay `entity` of `player`, which was this edge's, has been ended by the
    /// server: a later stay of the player took its place. `attempt` is that of the
    /// stay's join, as long as the stay had it.
    ///
    /// Made by every removal of a stay that its own edge did not ask for: a join or
    /// an arrival over a stay that is there, an arrival that is passed over for a
    /// later stay, an entering stay that is placed or let go over a present one, and
    /// the store's word that a stay is dead. Not by a leave, nor for an edge that is
    /// gone or reset. See `docs/adr/0020-one-stay-per-player.md`, section 6.
    Ended {
        player: PlayerId,
        entity: EntityId,
        attempt: Option<u64>,
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
/// A tick applies all of `edges`, then `applied`, then `dead`, then all of
/// `player_changes`, then `entered`, then all of `remote_actions`, and then all of
/// `inputs`. What players did and what became of them arrives as one sequence, though,
/// and its order is lost when it is sorted into the two. [`TickInputs::change`] and
/// [`TickInputs::input`] sort it so that nothing a player did before they came to the
/// region is applied after it.
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
    /// Border copies of chunks of other regions' land, or of land nobody holds, as
    /// their bases: read like loaded chunks, never written, ticked or saved
    /// (`docs/adr/0025-effects-between-regions.md`, section 3). The runner hands a base
    /// in only once its chunk is used by nobody or by a region coupled to this one
    /// (section 8 of the same record). Applied with the chunks, before the region
    /// works out which chunks tick.
    pub copy_bases: Vec<(ChunkPos, Chunk)>,
    /// The changes of later commits in copied chunks, blocks and block entities' data,
    /// in the order the store sent them, applied after the bases.
    pub copy_updates: Vec<(ChunkPos, Vec<Change>)>,
    /// Copies the region no longer has: gone from its tickets' reach, withdrawn
    /// because their chunk came to be used by a region not coupled to this one, or
    /// loaded as the region's own.
    pub copies_withdrawn: Vec<ChunkPos>,
    /// Effects for chunks the region has loaded, ready by the rule of the same record's
    /// section 5, in the order of their numbers. Run where seam entries run, after the
    /// inputs and the reckoning of which chunks tick.
    pub effects: Vec<FiledEffect>,
    /// The world store's answers to the region's [`StayNote::Entering`] notes, in the
    /// order they came. An answer is taken only for the very stay the region still
    /// holds as entering for the player, and passed over otherwise. Whatever the case,
    /// a stay of the player that is in the region goes first, with a
    /// [`Durable::Ended`] to its edge. Then the stay enters by its place:
    ///
    /// - none, or one whose feet are below [`RegionConfig::lowest_y`]: the player is
    ///   placed at the spawn point, looking ahead and not flying, with the starting
    ///   inventory if there is no place and with what the place has them hold otherwise;
    /// - a place and no `holder`: the player is placed there as the place has them;
    /// - a place and a `holder`: the stay is let go to that region in this tick
    ///   without being placed, with a [`Durable::Departed`] whose transfer has one
    ///   hand-over and the attempt of the join.
    ///
    /// See `docs/adr/0020-one-stay-per-player.md`, section 4, step 7.
    ///
    /// [`RegionConfig::lowest_y`]: crate::RegionConfig::lowest_y
    pub entered: Vec<Entered>,
    /// Of each player named, a stay below `(entity, hops)` is dead: one with a lower
    /// entity id, or that stay with fewer hand-overs. Such a stay that the region has
    /// is removed, and so is one with a lower entity id that it holds as entering;
    /// its entity is reported removed if it had been shown, and its edge is told with
    /// [`Durable::Ended`]. Nothing of an entry is kept after the tick. See section 5
    /// of the same record.
    pub dead: Vec<(PlayerId, EntityId, u32)>,
    /// The world store asks the region for chunks of its pinned land, each lend under its
    /// number, in the order asked. Answered in [`TickOutput::lend_answers`] at the end of
    /// the tick (`docs/adr/0027-lending-at-a-line.md`, section 3, step 2).
    pub lends_asked: Vec<LendAsked>,
    /// Chunks the region asked to be lent that are not: asked again later where the
    /// flag says so, and not before what the region knows of them changes otherwise
    /// (the same record, section 2).
    pub not_lent: Vec<(Vec<ChunkPos>, bool)>,
    /// Lends called off, by their numbers: the region that asked keeps its ask, and the
    /// lender takes the chunks up again as its own.
    pub lends_called_off: Vec<(u64, Vec<ChunkPos>)>,
    /// Chunks of the region's pinned land that the region named holds by a lend, or by
    /// what a merge or a split made of one: believed until they are pinned back.
    pub lent: Vec<(Vec<ChunkPos>, RegionId)>,
    /// Chunks of the region's pinned land that whoever held them by a grant has given
    /// back.
    pub pinned_back: Vec<ChunkPos>,
}

/// The world store's ask of a lend of chunks of the region's pinned land to the region
/// `to` (`docs/adr/0027-lending-at-a-line.md`, section 3, step 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LendAsked {
    pub lend: u64,
    pub chunks: Vec<ChunkPos>,
    pub to: RegionId,
}

/// A lender's answer to a [`LendAsked`] for some of its chunks (the same record,
/// section 3, step 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LendAnswer {
    /// Not lent: asked again later if `again`, and not otherwise while what stands
    /// there stands.
    Refused {
        lend: u64,
        chunks: Vec<ChunkPos>,
        again: bool,
    },
    /// Let go of for the lend: the region has stopped ticking them, and drops them at
    /// the start of its next tick, so that they are to be saved as they are after this
    /// one, with their ticks, before the answer goes to the store.
    Lending { lend: u64, chunks: Vec<ChunkPos> },
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
    /// For a chunk beside a viewer's chunk that the region holds, which the chunk needs
    /// loaded to tick (`docs/adr/0024-block-behaviour-in-a-region.md`, section 2). It
    /// wants and claims the chunk as a viewer's ticket does.
    ViewerRing,
    /// The same beside a guest's chunk only: it wants and claims the chunk as a guest's
    /// ticket does.
    GuestRing,
}

impl TickInputs {
    /// Adds what became of a player, which came after everything added so far.
    ///
    /// For a join, what that player did before, through any edge and as far as it
    /// waits here for the coming tick, is dropped; for an arrival, what waits that
    /// names the entity that arrives. The stay was not there then, so the region would
    /// have ignored it. Applied after the arrival instead, it would be taken for what
    /// the player did since, and an input sent to the region while the player was away
    /// would be applied ahead of earlier ones that are sent again with the arrival,
    /// which then count as applied already and are lost.
    ///
    /// An arrival leaves what waits of another stay of the player: with several edges
    /// an earlier stay can arrive where a later one is, and must not take away what
    /// the later one did in that tick. See `docs/adr/0020-one-stay-per-player.md`,
    /// section 12.
    ///
    /// A leave drops nothing. Whether it applies only the tick can tell: one for a stay
    /// the region does not have changes nothing, and must not take away what the stay
    /// that is there did. Nor is anything lost by keeping it all. A tick applies
    /// changes before inputs, so what a player did before a leave that takes effect
    /// finds nobody; and if they are back within the tick, the join or the arrival
    /// that brings them drops it.
    pub fn change(&mut self, change: PlayerChange) {
        match &change {
            PlayerChange::Join(_, PlayerJoin { player, .. }) => {
                self.inputs.retain(|(_, actor, ..)| actor != player);
            }
            PlayerChange::Arrive(_, player, transfer) => {
                let arriving = transfer.entity_id;
                self.inputs
                    .retain(|(_, actor, entity, ..)| actor != player || *entity != arriving);
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
    /// The player has entered the world. `attempt` is that of the join it answers
    /// ([`PlayerJoin::attempt`]).
    Spawned {
        attempt: u64,
        entity_id: EntityId,
        pose: Pose,
        flying: bool,
        inventory: Inventory,
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
    /// The region changed a slot of the player's inventory of its own accord, as the
    /// game's server does where the client does not foresee it (the swap of the hands),
    /// and the client is to be told what it holds now. `slot` is numbered as
    /// [`Inventory`] numbers them.
    InventorySlot { slot: u8, stack: Option<ItemStack> },
}

/// Something that happened in the region and concerns everyone who can see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RegionEvent {
    /// An entity has come into existence.
    EntitySpawned(EntityState),
    /// An entity has ceased to exist. `chunk` is where it was last.
    EntityRemoved { entity: EntityId, chunk: ChunkPos },
    /// A block has changed. `seen` says whether it was set with the game's
    /// `UPDATE_CLIENTS`: one set without it (a block a piston moves, which clients
    /// move themselves from the piston's block event) is not sent, but a block waiting
    /// to be sent is sent as it is then, as `ChunkHolder.broadcastChanges` reads it.
    BlockChanged {
        position: BlockPos,
        state: BlockState,
        seen: bool,
    },
    /// The block entity at the position has new data, the bytes of one NBT compound as
    /// [`clustine_world::BlockEntity`] keeps them; empty where the game makes a fresh
    /// one for a block of the same kind (`docs/adr/0026-redstone.md`, section 6). It
    /// comes in its place among the tick's [`Self::BlockChanged`].
    BlockEntityChanged { position: BlockPos, data: Vec<u8> },
    /// A block broke, which clients show with its sound and bits (the game's level
    /// event 2001). The block itself changes by a [`Self::BlockChanged`] beside it.
    /// `breaker` is the player who broke it, whose client shows it already.
    BlockDestroyed {
        position: BlockPos,
        state: BlockState,
        breaker: Option<PlayerId>,
    },
    /// A block event ran (`docs/adr/0026-redstone.md`, section 4): the game's
    /// `ClientboundBlockEventPacket`, of the block whose id is `block`, with its two
    /// numbers, which clients act on (a piston's move, a note block's note).
    BlockEvent {
        position: BlockPos,
        block: u16,
        kind: i32,
        data: i32,
    },
    /// A sound the server plays at a place (`Level.playSound`), the game's sound event
    /// of id `sound` from its `source` (4 for blocks): heard by the players within 16
    /// blocks, or 16 times `volume` if that is more, but `except`, whose own client
    /// plays it (`docs/adr/0026-redstone.md`, section 8).
    Sound {
        sound: i32,
        source: i32,
        position: Vec3,
        volume: f32,
        pitch: f32,
        except: Option<PlayerId>,
    },
    /// The game's level event `kind` at a block (`Level.levelEvent`), with its `data`:
    /// what clients show of something there, a torch burning out (1502) say. Seen by
    /// the players within 64 blocks; the breaking of a block and lava's hiss have
    /// variants of their own.
    LevelEvent {
        kind: i32,
        position: BlockPos,
        data: i32,
    },
    /// The phase of block events begins here (`docs/adr/0026-redstone.md`, section
    /// 8): the game sends the tick's block changes before its block events, and those
    /// made from here on, in the block events, the entities and the block entities,
    /// with the next tick's. Said in a tick in which something comes after it. It has
    /// no place, and whoever filters events by where they are passes it to all.
    BlockEventsBegin,
    /// Lava met water, or took the place of a block, which clients show with a hiss and
    /// smoke (the game's level event 1501).
    Fizzed { position: BlockPos },
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
            Self::BlockChanged { position, .. }
            | Self::BlockEntityChanged { position, .. }
            | Self::BlockEvent { position, .. }
            | Self::LevelEvent { position, .. }
            | Self::BlockDestroyed { position, .. }
            | Self::Fizzed { position } => [position.chunk(); 2],
            Self::Sound { position, .. } => [ChunkPos::containing(position.x, position.z); 2],
            // No place: see the variant.
            Self::BlockEventsBegin => [ChunkPos::new(0, 0); 2],
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

/// The game's `SoundSource.BLOCKS`, which a block's sounds are of.
pub const SOUND_SOURCE_BLOCKS: i32 = 4;

/// The game's `SoundSource.PLAYERS`, which what a player plays as themself is of.
pub const SOUND_SOURCE_PLAYERS: i32 = 7;

/// The game's `SoundSource.RECORDS`, which a note block's notes are of.
pub const SOUND_SOURCE_RECORDS: i32 = 2;

/// What a tick resulted in.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TickOutput {
    /// The number of this tick; the first tick of a region is 1.
    pub tick: u64,
    /// [`PlayerEvent::Spawned`], in the order the players joined, or, where the world
    /// store says where a player enters, in the order of its answers
    /// ([`TickInputs::entered`]); then [`PlayerEvent::InventorySlot`], in the order of
    /// the inputs that made them; and then [`PlayerEvent::Acknowledged`], in the order
    /// of the players. What else concerns a single player is among `durable`.
    pub player_events: Vec<(PlayerId, PlayerEvent)>,
    pub events: Vec<RegionEvent>,
    /// Chunks that have to be fetched from storage and passed in through
    /// [`TickInputs::chunks_loaded`].
    pub chunk_requests: Vec<ChunkPos>,
    /// The outbox entries made in this tick, each with the edge whose outbox it is in
    /// and its number there. An edge's entries are numbered on from its
    /// [`EdgeState::sent`].
    ///
    /// They are in this order: the stays that [`TickInputs::dead`] ended; refusals,
    /// the stays that a join or an arrival ended and the arrivals that are another
    /// region's, in the order of [`TickInputs::player_changes`]; the stays that
    /// [`TickInputs::entered`] ended, in its order; the answers to
    /// [`TickInputs::remote_actions`], one for one and in their order; what players did
    /// to blocks of chunks the region does not hold, in the order of
    /// [`TickInputs::inputs`]; and those who were let go: the stays let go without
    /// being placed, in the order of [`TickInputs::entered`], and then the players, in
    /// their order.
    ///
    /// So every [`Durable::Ended`] of a tick has a lower number than every
    /// [`Durable::Departed`] of that tick in the same outbox, and a stay that is let
    /// go without being placed is numbered with the tick's other departures and not
    /// where it is decided. An edge remembers only the highest number it has seen, and
    /// the runner publishes a tick's departures last. See
    /// `docs/adr/0020-one-stay-per-player.md`, section 6.
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
    ///
    /// A lent chunk is given back once no circuit's part is near it, loaded: the region
    /// drops it at the start of its next tick, so that it is to be saved as it is after
    /// this one before the return goes to the store
    /// (`docs/adr/0027-lending-at-a-line.md`, section 4).
    pub returns: Vec<ChunkPos>,
    /// Chunks across a pinned line the region asks to be lent, ascending: for a circuit's
    /// part placed in its own pinned land within reach of them, or asked again
    /// (`docs/adr/0027-lending-at-a-line.md`, section 2).
    pub lends: Vec<ChunkPos>,
    /// The region's answers to [`TickInputs::lends_asked`], in the order asked.
    pub lend_answers: Vec<LendAnswer>,
    /// Chunks whose scheduled ticks changed in this tick, in ascending order: a tick
    /// scheduled or run there, or the ticks of a chunk that stopped ticking written
    /// into it. They are to be saved as chunks whose blocks changed are
    /// (`docs/adr/0024-block-behaviour-in-a-region.md`, section 4).
    pub ticks_changed: Vec<ChunkPos>,
    /// Chunks the region ticks with scheduled ticks pending at the end of this tick, in
    /// ascending order. Their saved forms change with every tick, since what is kept is
    /// the delay left, without anything being committed; so where a stop is near they
    /// are to be saved as unsaved chunks are
    /// (`docs/adr/0024-block-behaviour-in-a-region.md`, section 4).
    pub ticks_pending: Vec<ChunkPos>,
    /// Chunks post-processed in this tick, whose marks were taken, in ascending order
    /// (`docs/adr/0024-block-behaviour-in-a-region.md`, section 6). They are to be
    /// committed and saved as chunks whose blocks changed are.
    pub marks_taken: Vec<ChunkPos>,
    /// Chunks whose seam entries changed in this tick, in ascending order
    /// (`docs/adr/0024-block-behaviour-in-a-region.md`, section 5). They are to be
    /// committed and saved as chunks whose blocks changed are.
    pub seams_changed: Vec<ChunkPos>,
    /// Chunks whose kept block events changed in this tick, in ascending order
    /// (`docs/adr/0026-redstone.md`, section 4). They are to be committed and saved as
    /// chunks whose blocks changed are.
    pub events_changed: Vec<ChunkPos>,
    /// What the tick did in chunks the region does not have loaded and does not hold
    /// or ask for, in the order it did them
    /// (`docs/adr/0025-effects-between-regions.md`, section 4). They are to be
    /// committed with the tick's commit.
    pub effects: Vec<Effect>,
    /// The effects the tick took, ran or kept as seam entries, ascending (the same
    /// record, section 5). A tick that takes any is to be committed whatever else it
    /// changed.
    pub taken: Vec<EffectId>,
    /// Everything that changed in the region's state in this tick.
    pub delta: StateDelta,
    /// What the region says to the world store of stays in this tick, in the order of
    /// the players, a [`StayNote::Entering`] before a [`StayNote::Has`] of the same
    /// player:
    ///
    /// - `Entering` for the stay a join of this tick began, if the region still holds
    ///   it as entering at the end of the tick;
    /// - `Has` for every player who is in the region after the tick and of whom the
    ///   tick changed anything, which is the players of [`StateDelta::players`]: one
    ///   who was placed, one who arrived, one who did something;
    /// - `Has` for every player who was let go, and for every stay let go without
    ///   being placed, with the hand-overs and the place of the transfer;
    /// - in the first tick of a region, which is the first after
    ///   [`Region::restore`], after a merge or after a split: a note for every stay
    ///   the region has, changed or not.
    ///
    /// None for a player who was removed. A tick with notes is to be committed
    /// whatever else it changed. See `docs/adr/0020-one-stay-per-player.md`,
    /// section 3.
    ///
    /// [`Region::restore`]: crate::Region::restore
    pub stays: Vec<StayNote>,
}
