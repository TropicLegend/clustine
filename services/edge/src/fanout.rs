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
use clustine_data::{BlockState, Swing, entity_types, synced_registry};
use clustine_protocol::codec::Position;
use clustine_protocol::item as protocol;
use clustine_protocol::item::ItemStack as WireStack;
use clustine_protocol::nbt::Nbt;
use clustine_protocol::packets::play::{
    AcknowledgeBlockChange, AddTransientBlock, Animate, AttributeSnapshot, BlockDestruction,
    BlockEvent, BlockUpdate, ChangeDifficulty, ChunkBatchFinished, ChunkBatchStart,
    ClientboundMoveVehicle, ClockState, Cooldown, DamageEvent, DeathLocation, Disconnect,
    EntityDataValue, EntityEvent, GameEvent, LevelEvent, Login, PlayerAbilities, PlayerCombatKill,
    PlayerInfoEntry, PlayerInfoRemove, PlayerInfoUpdate, PositionPath, RemoveEntities,
    RemoveMobEffect, Respawn, SectionBlocksUpdate, SetCenterChunk, SetDefaultSpawnPosition,
    SetEntityData, SetEntityLink, SetEquipment, SetExperience, SetHeadRotation, SetHealth,
    SetHeldSlot, SetPassengers, SetPlayerInventory, SetTime, Sound, SpawnEntity, SpawnInfo,
    SwingAnimation, SyncEntityPosition, SynchronizePlayerPosition, SystemChat, TakeItemEntity,
    UnloadChunk, UpdateAttributes, UpdateMobEffect, abilities, angle, entity_data, game_event,
    level_event, player_info, pose, relative,
};
use clustine_protocol::packets::{self, Packet};
use clustine_protocol::text::Text;
use clustine_region::RegionId;
use clustine_rpc::link;
use clustine_rpc::{EdgeMessage, EdgeToWorker, Presence, Welcome, WorkerToEdge};
use clustine_sim::api::{
    Appearance, AttributeShown, Durable, EntityKind, EntityState, Inventory, ItemStack,
    Misdirected, PlayerEvent, PlayerInput, PlayerJoin, PlayerTransfer, Pose, RegionEvent,
    RemoteAction, Respawned, Tracked, WorldClock,
};
use clustine_world::{
    BlockPos, Chunk, ChunkPos, Dimension, EntityData, EntityId, InChunk, LevelChunk, Light,
    PlayerId, SectionLight, Vec3, light_tells_apart, sky_light,
};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::{Instant, MissedTickBehavior};
use tracing::{debug, error, info, warn};

use crate::encode::chunk_packet;
use crate::figures::{self, FIGURES};
use crate::light::{FromThread, LightJob, LightOutcome, Lighting};
use crate::login::Profile;
use crate::tracking::{Pairing, Seen, Tracker};
use crate::{EdgeIdentity, RegionLink, Relink, Routing, Stopped};

const OVERWORLD: &str = "minecraft:overworld";

/// The ability flags a client is told for a player in `mode` who flies or not
/// (`Abilities.write`, as `GameType.updatePlayerAbilities` sets them).
fn ability_flags(mode: clustine_world::GameMode, flying: bool) -> u8 {
    let can = mode.abilities();
    let mut flags = 0;
    if can.invulnerable {
        flags |= abilities::INVULNERABLE;
    }
    if mode.flies_from(flying) {
        flags |= abilities::FLYING;
    }
    if can.may_fly {
        flags |= abilities::MAY_FLY;
    }
    if can.instabuild {
        flags |= abilities::INSTANT_BREAK;
    }
    flags
}

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

/// What a player did to an entity their client named.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum OnEntity {
    /// `attack`.
    Attack,
    /// `interact`: where on the entity, the packet's own sneak flag, and the hand.
    Interact {
        location: [f64; 3],
        sneaking: bool,
        off_hand: bool,
    },
    /// `pick_item_from_entity`: the middle click that picks the entity's item.
    Pick,
}

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
        /// How the client asked the others to draw its player, as it said in the
        /// configuration (the game's defaults where it said nothing).
        appearance: Appearance,
        outbound: mpsc::Sender<Bytes>,
        /// Shared with the connection; see [`NO_TELEPORT`].
        awaiting_teleport: Arc<AtomicI32>,
    },
    /// The connection has ended.
    Leave {
        session: SessionId,
        player: PlayerId,
    },
    /// The edge is to stop: every player leaves as though their connection had ended,
    /// and `done` is told once every region has applied and made durable all that the
    /// edge sent it, those leaves among it (`PlayerList.removeAll`;
    /// `docs/groundwork/n7h-logout-on-a-mount.md`, section 13 item 1).
    Drain {
        done: tokio::sync::oneshot::Sender<()>,
    },
    /// The client has processed a chunk batch and wants this many chunks per tick.
    ChunkBatchReceived {
        session: SessionId,
        player: PlayerId,
        chunks_per_tick: f32,
    },
    /// The client asked for another view distance in play.
    ViewDistance {
        session: SessionId,
        player: PlayerId,
        requested: i32,
    },
    /// The player did something that the region they are in has to know.
    Input {
        session: SessionId,
        player: PlayerId,
        input: PlayerInput,
    },
    /// The player hit or used the entity their client knows by `id`.
    OnEntity {
        session: SessionId,
        player: PlayerId,
        id: i32,
        what: OnEntity,
    },
    /// The player clicked in, or closed, the window of a menu
    /// (`docs/groundwork/n6-plan.md`, sections 7 to 9).
    Menu {
        session: SessionId,
        player: PlayerId,
        what: MenuCommand,
    },
    /// The player did something to blocks that no region needs to hear of, and their
    /// client waits to be told that it was handled.
    Handled {
        session: SessionId,
        player: PlayerId,
        sequence: i32,
    },
}

/// What a player did in a menu's window.
#[derive(Debug)]
pub(crate) enum MenuCommand {
    Click(clustine_protocol::packets::play::ContainerClick),
    /// `container_close`, whose window id the game does not read
    /// (`handleContainerClose`).
    Close,
    /// `select_trade`: the trade the player picked in the window shown.
    SelectTrade {
        item: i32,
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
    /// What a client is told of the world when it enters: see
    /// [`crate::EdgeConfig::login`].
    pub(crate) login: crate::LoginWorld,
    /// When a chunk whose light is not right gets its job.
    pub(crate) light: LightSettings,
    /// Every wire id the edge gives, with its key, for tests to read what a client is
    /// shown as keys.
    #[cfg(test)]
    pub(crate) wire_ledger: WireLedger,
}

/// See [`FanoutConfig::wire_ledger`]. Wire ids are never given twice, so it maps them.
#[cfg(test)]
pub(crate) type WireLedger = Arc<std::sync::Mutex<BTreeMap<i32, EntityId>>>;

/// When a chunk whose light is not right gets its job
/// (`docs/adr/0023-light-at-the-edge.md`, section 4). Settings and not constants:
/// the edge's tests set the hold and the rest to nothing, so that everything a turn
/// marks is lit before the turn ends.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LightSettings {
    /// For how long a chunk that cannot be sent is held for a chunk beside it that
    /// is wanted and has not come: lit with all its neighbours it costs one job,
    /// lit before them one for each that comes.
    pub(crate) latest: Duration,
    /// For how long after a job a chunk that can be sent, a little old, gets no
    /// other: a place where blocks never stop changing costs so many jobs a second
    /// and no more.
    pub(crate) rest: Duration,
    /// After how long a chunk nobody waits for goes before the nearer ones nobody
    /// waits for.
    pub(crate) overdue: Duration,
    /// For how long a thread may have a job before the job is given up as failed
    /// and the thread written off.
    pub(crate) job_patience: Duration,
    /// For measuring only (`docs/adr/0023-light-at-the-edge.md`, step L7): in how
    /// many tenths of the world a chunk is taken as a changed one comes from the
    /// store, without light and not as generated, so that its light, and its
    /// neighbours', is the edge's to make. Nothing but 0 outside a measurement.
    pub(crate) as_changed: u8,
}

impl LightSettings {
    /// What the server runs with.
    pub(crate) const SERVER: Self = Self {
        latest: Duration::from_secs(1),
        rest: Duration::from_millis(250),
        overdue: Duration::from_secs(5),
        job_patience: Duration::from_secs(5),
        as_changed: 0,
    };

    /// Nothing is held and nothing rests, for whoever must have sent all it will
    /// by the end of a turn.
    #[cfg(test)]
    pub(crate) const AT_ONCE: Self = Self {
        latest: Duration::ZERO,
        rest: Duration::ZERO,
        overdue: Duration::from_secs(5),
        job_patience: Duration::from_secs(1_000_000),
        as_changed: 0,
    };

    /// Whether the chunk at `position` is one [`LightSettings::as_changed`] takes as
    /// changed: the same chunks at every edge and in every run, spread over the
    /// world without a pattern a view would line up with.
    pub(crate) fn takes_as_changed(&self, position: ChunkPos) -> bool {
        let mut mixed = (position.x as u32).wrapping_mul(0x9E37_79B1)
            ^ (position.z as u32).wrapping_mul(0x85EB_CA77);
        mixed ^= mixed >> 15;
        mixed = mixed.wrapping_mul(0x2C1B_3C6D);
        mixed ^= mixed >> 13;
        mixed % 10 < u32::from(self.as_changed)
    }
}

/// How often at most the fan-out task wakes for light alone: to move the chunks
/// whose hold or rest has ended, and to give up jobs.
const LIGHT_TICK: Duration = Duration::from_millis(50);

/// Which of the sets a marked entry is in, and under what key, so that it is taken
/// out or moved in a logarithm's time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Queued {
    /// It cannot be sent and someone waits for it. `rank` is the squared distance
    /// to the centre of the nearest waiter, `by`; `since` the moment it was put in,
    /// which a new rank keeps, so that of two equally near the older goes first.
    Waiting { rank: i64, since: u64, by: PlayerId },
    /// It can be sent, a little old, or nobody waits for it. The rank is of when
    /// it was put in and is not kept up; `marked` is since when its light is not
    /// right, by which the overdue are found.
    Unawaited {
        rank: i64,
        since: u64,
        marked: Instant,
    },
    /// Held or resting until then.
    Later { until: Instant },
}

/// The marked entries that have no job under way, each in one of three sets
/// (`docs/adr/0023-light-at-the-edge.md`, section 4). Nothing is scanned to find
/// the next job: a turn costs what it touched.
#[derive(Default)]
struct LightQueues {
    waiting: BTreeSet<(i64, u64, LevelChunk)>,
    unawaited: BTreeSet<(i64, u64, LevelChunk)>,
    /// The entries of `unawaited` again, by since when their light is not right.
    oldest: BTreeSet<(Instant, u64, LevelChunk)>,
    later: BTreeSet<(Instant, LevelChunk)>,
}

impl LightQueues {
    /// Takes the entry at `position` out of the set it is in.
    fn take_out(&mut self, position: LevelChunk, entry: &mut ReplicaChunk) {
        match entry.queued.take() {
            Some(Queued::Waiting { rank, since, .. }) => {
                self.waiting.remove(&(rank, since, position));
            }
            Some(Queued::Unawaited {
                rank,
                since,
                marked,
            }) => {
                self.unawaited.remove(&(rank, since, position));
                self.oldest.remove(&(marked, since, position));
            }
            Some(Queued::Later { until }) => {
                self.later.remove(&(until, position));
            }
            None => {}
        }
    }

    /// Puts the entry at `position`, which is in no set, into the one `queued` says.
    fn put_in(&mut self, position: LevelChunk, entry: &mut ReplicaChunk, queued: Queued) {
        match queued {
            Queued::Waiting { rank, since, .. } => {
                self.waiting.insert((rank, since, position));
            }
            Queued::Unawaited {
                rank,
                since,
                marked,
            } => {
                self.unawaited.insert((rank, since, position));
                self.oldest.insert((marked, since, position));
            }
            Queued::Later { until } => {
                self.later.insert((until, position));
            }
        }
        entry.queued = Some(queued);
    }

    /// The entry that gets the next job: the first of those someone waits for,
    /// always; otherwise the oldest of the others if it is overdue; otherwise the
    /// nearest of them. Nothing nobody waits for ever comes before a chunk
    /// someone does.
    fn next(&self, now: Instant, overdue: Duration) -> Option<LevelChunk> {
        if let Some((_, _, position)) = self.waiting.first() {
            return Some(*position);
        }
        if let Some((marked, _, position)) = self.oldest.first()
            && now.saturating_duration_since(*marked) > overdue
        {
            return Some(*position);
        }
        self.unawaited.first().map(|(_, _, position)| *position)
    }

    fn len(&self) -> usize {
        self.waiting.len() + self.unawaited.len() + self.later.len()
    }
}

/// The squared distance between two chunks, in chunks.
fn apart(one: ChunkPos, other: ChunkPos) -> i64 {
    let (dx, dz) = (i64::from(one.x - other.x), i64::from(one.z - other.z));
    dx * dx + dz * dz
}

/// A chunk at least one player of this edge can see.
struct ReplicaChunk {
    /// How many players have the chunk in view.
    viewers: u32,
    /// The chunk, once the worker has sent it, kept up to date with later changes. It
    /// is without light: the edge takes the light out when the chunk comes and keeps
    /// it beside (`docs/adr/0023-light-at-the-edge.md`, section 1). Shared, so that
    /// whoever lights a chunk can be handed it and its neighbours without a copy; a
    /// block change copies the chunk only while someone else holds it.
    chunk: Option<Arc<Chunk>>,
    /// The light a client is sent with the chunk. One copy: what the chunk came with,
    /// while the test for carried light passes, and otherwise light the edge made.
    light: Option<Arc<Light>>,
    /// Where `light` is from.
    kind: LightKind,
    /// The generator's version whose blocks, as light sees them, the chunk has; none
    /// once a block that light sees has changed, or for a chunk no generator vouches
    /// for. A neighbour's carried light is good beside this chunk only if it was
    /// made beside this version's.
    as_made: Option<u16>,
    /// The moment at which the chunks light is made from last changed: this chunk
    /// came or was replaced, or a neighbour did.
    shaped: u64,
    /// The moment of the last block change that light sees, in the chunk or in a
    /// neighbour within its reach.
    changed: u64,
    /// The moment `light` is as recent as: of the job that made it or failed, or of
    /// the test that passed.
    lit: u64,
    /// The moment of the job under way for the chunk, if any.
    job: Option<u64>,
    /// Those who have the chunk and are owed its light were not sent the entry's
    /// latest: a result was withheld, or a job failed. The next result that goes to
    /// them names every section.
    owed: bool,
    /// The next result goes to everyone who has the chunk, wherever in their view it
    /// is, because one of them cannot have its light by themselves: a chunk has come
    /// or been replaced beside this one since they were sent its light, or someone
    /// was sent the chunk with light older than its blocks, whose client never saw
    /// the block change and so never lit it.
    reshaped: bool,
    /// Since when the light is not right for the chunks and blocks as they are, if
    /// it is not and no job has been made for that yet. A block change in an entry
    /// that is marked already does not set it anew.
    marked_since: Option<Instant>,
    /// Since when the chunk cannot be sent, if it cannot: the hold is counted from
    /// here.
    unsendable_since: Option<Instant>,
    /// When the last job for the chunk was dispatched: the rest is counted from
    /// here.
    dispatched: Option<Instant>,
    /// The next job does not wait for the rest: a result was withheld from those
    /// who have the chunk, and they are owed the next. Once in a rest, so `exempted`
    /// is when that was last granted.
    exempt: bool,
    exempted: Option<Instant>,
    /// Which set of [`LightQueues`] the entry is in.
    queued: Option<Queued>,
    /// The chunk's packet. It is encoded when first needed, so that many players can
    /// be sent the same bytes, and discarded when the chunk changes and when nobody
    /// waits for the chunk any more: it is as large as the chunk and its light
    /// together, and for players who are apart it is never read a second time
    /// (`docs/adr/0023-light-at-the-edge.md`, sections 1 and 9).
    packet: Option<Bytes>,
    /// The players who have the chunk pending.
    waiters: BTreeSet<PlayerId>,
    /// The region whose snapshot the replica has. It stays when that region's link
    /// ends, and goes when the edge has no subscription there any more or the region
    /// says that it does not hold the chunk.
    served_by: Option<RegionId>,
    /// When the chunk came, until it is first sent: how long that took is one of
    /// the edge's figures (`crate::figures`).
    came: Option<std::time::Instant>,
}

/// Where an entry's light is from (`docs/adr/0023-light-at-the-edge.md`, section 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LightKind {
    /// The light the chunk came with, which passed the test beside its neighbours.
    Carried,
    /// Made by the edge from the chunk and the neighbours it had.
    Computed,
    /// Not right, and counted: the last job for the chunk failed. It is the light the
    /// entry had, or the sky light reckoned by columns.
    Fallback,
}

impl ReplicaChunk {
    /// An entry for a chunk that has not come.
    fn awaited() -> Self {
        Self {
            viewers: 0,
            chunk: None,
            light: None,
            kind: LightKind::Carried,
            as_made: None,
            shaped: 0,
            changed: 0,
            lit: 0,
            job: None,
            owed: false,
            reshaped: false,
            marked_since: None,
            unsendable_since: None,
            dispatched: None,
            exempt: false,
            exempted: None,
            queued: None,
            packet: None,
            waiters: BTreeSet::new(),
            served_by: None,
            came: None,
        }
    }

    /// Whether the chunk may be sent: it is there with light that is at least as
    /// recent as the chunks it was made from. No chunk is ever sent otherwise.
    fn sendable(&self) -> bool {
        self.chunk.is_some() && self.light.is_some() && self.lit >= self.shaped
    }

    /// Whether the light is right for the blocks as they are: sendable, and no block
    /// that light sees has changed since. An entry that is not is marked, and gets
    /// a job.
    fn current(&self) -> bool {
        self.sendable() && self.lit >= self.changed
    }

    /// The packet that sends the chunk, located at `position`, if the chunk is there.
    fn packet(&mut self, position: ChunkPos) -> Option<Bytes> {
        if self.packet.is_none() {
            let chunk = self.chunk.as_ref()?;
            let began = std::time::Instant::now();
            self.packet = Some(encoded(&chunk_packet(
                position,
                chunk,
                self.light.as_deref(),
            )));
            figures::made_packet(began);
        }
        self.packet.clone()
    }

    /// `player` waits for the chunk no more: they were sent it, or their view has
    /// moved away or gone. The packet goes with the last of them.
    fn waits_no_more(&mut self, player: PlayerId) {
        self.waiters.remove(&player);
        if self.waiters.is_empty() {
            self.packet = None;
        }
    }
}

/// What one player has been sent and is waiting for.
struct PlayerView {
    session: SessionId,
    name: String,
    outbound: mpsc::Sender<Bytes>,
    awaiting_teleport: Arc<AtomicI32>,
    /// The id of the last teleport the client was sent: the login's 1, then one more for
    /// each getting on or off a vehicle, which the game does as teleports.
    teleports: i32,
    /// The player's entity, once a region has placed the player.
    entity: Option<EntityId>,
    /// The region the player is in, as far as this task has been told. The region itself
    /// may already have let the player go, with the message saying so still on its way.
    region: RegionId,
    /// The dimension the client shows: what every chunk of the view is of. It changes
    /// with the first half of a change of dimension (`docs/groundwork/p-plan.md`,
    /// section 4.2).
    dimension: Dimension,
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
    /// The menu the client shows, and its clicks on their way (`crate::menus`).
    menus: crate::menus::Menus,
    /// The region whose clocks the client was last told whole, which a region it is
    /// handed to then tells it whole again (`docs/groundwork/n7-plan.md`, section 8.4).
    told_clocks: Option<RegionId>,
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
    /// Chunks in view the client has as they were sent before and is to be sent
    /// anew: they are among the pending, and the client goes on showing what it has,
    /// and is told of blocks that change in it, until the new chunk goes out with
    /// an unload in front of it. So what a client holds of a chunk is always what
    /// one packet and the updates after it said
    /// (`docs/adr/0023-light-at-the-edge.md`, section 6).
    replace: BTreeSet<ChunkPos>,
    /// Entities the client has been shown, with the player each of them is, or none for
    /// an entity other than a player.
    visible: BTreeMap<EntityId, Option<PlayerId>>,
    /// Players the client has in its player list, each with the id of the game mode it
    /// was last told of them. A client only shows the entity of a player it has in that
    /// list.
    listed: BTreeMap<PlayerId, u8>,
    /// Whether a batch has been sent that the client has not confirmed.
    batch_outstanding: bool,
    /// The change of dimension under way: the client has been sent `Respawn`, and
    /// waits, its moves held back, until the destination says it placed the player
    /// (`docs/groundwork/p-plan.md`, section 4.2).
    travelling: Option<Travelling>,
    /// A respawn the client is owed (`docs/groundwork/p-plan.md`, section 19.7): from a
    /// death, the credits, or a departure out of the End by its portal, until the
    /// placing that sends the `Respawn`. Meanwhile what the player's regions tell them
    /// is held here, and told after the `Respawn`, which would otherwise drop it.
    respawn_owed: Option<Vec<PlayerEvent>>,
    batch_size: usize,
    /// Where the player last died, which every `SpawnInfo` made for them says
    /// (`docs/groundwork/p-plan.md`, section 19.9): from the stay that entered, and
    /// from each death since.
    last_death: Option<(Dimension, BlockPos)>,
    /// Their game mode, which their abilities, every `SpawnInfo` made for them and the
    /// player list say (`docs/groundwork/v-plan.md`, section 4.1): from the stay that
    /// entered, and from every hand-over since.
    mode: clustine_world::PlayerMode,
}

/// A change of dimension under way: since when, and how the player goes, which the
/// end of it shows (the portal's sound).
#[derive(Debug, Clone, Copy)]
struct Travelling {
    since: Instant,
    way: Option<clustine_world::Way>,
    /// The look the departing stay had, from which the placing's look is told as a
    /// turn: the game's portals place a player with their look relative.
    look: (f32, f32),
}

/// The teleport a travelling client is said to await: no teleport has this id, so
/// that its moves are held back until the teleport that places it
/// (`docs/groundwork/p-plan.md`, section 4.2, R N3).
const TRAVELLING: i32 = i32::MIN;

/// `degrees` brought into [-180, 180) (`Mth.wrapDegrees`).
fn wrap_degrees(degrees: f32) -> f32 {
    let wrapped = degrees % 360.0;
    if wrapped >= 180.0 {
        wrapped - 360.0
    } else if wrapped < -180.0 {
        wrapped + 360.0
    } else {
        wrapped
    }
}

/// A player's name as `Player.getDisplayName` makes it, with no team: its click suggests
/// a message to them, its hover shows them, and a shift-click inserts their name.
/// How a death message names who killed the player (`Entity.getDisplayName`): by their
/// name where they have one (a player's, or a custom name), else by their type's
/// (`entity.minecraft.zombie`). Named: the game's name carries a hover of the entity and,
/// for a player, the click of `display_name`; and a death message's weapon (`.item`)
/// cannot arise, since no stack carries a custom name yet.
fn killer_name(killer: &clustine_sim::api::Killer) -> Nbt {
    if let Some(name) = &killer.name {
        return Nbt::Compound(vec![("text".to_owned(), Nbt::String(name.clone()))]);
    }
    let kind = clustine_data::ENTITY_TYPES
        .get(usize::from(killer.kind))
        .copied()
        .unwrap_or("minecraft:pig");
    let key = format!("entity.{}", kind.replacen(':', ".", 1));
    Nbt::Compound(vec![("translate".to_owned(), Nbt::String(key))])
}

fn display_name(name: &str, player: PlayerId) -> Nbt {
    let uuid = player.0.as_u128();
    // `UUIDUtil.uuidToIntArray`: the most significant 32 bits first.
    let words = (0..4)
        .map(|at| (uuid >> (96 - 32 * at)) as u32 as i32)
        .collect();
    Nbt::Compound(vec![
        (
            "click_event".to_owned(),
            Nbt::Compound(vec![
                (
                    "action".to_owned(),
                    Nbt::String("suggest_command".to_owned()),
                ),
                ("command".to_owned(), Nbt::String(format!("/tell {name} "))),
            ]),
        ),
        ("insertion".to_owned(), Nbt::String(name.to_owned())),
        ("text".to_owned(), Nbt::String(name.to_owned())),
        (
            "hover_event".to_owned(),
            Nbt::Compound(vec![
                ("name".to_owned(), Nbt::String(name.to_owned())),
                ("action".to_owned(), Nbt::String("show_entity".to_owned())),
                ("id".to_owned(), Nbt::String("minecraft:player".to_owned())),
                ("uuid".to_owned(), Nbt::IntArray(words)),
            ]),
        ),
    ])
}

/// The ticks a player waits after a portal before another takes them
/// (`Player.getDimensionChangingDelay`), which the `Respawn` of their travel tells.
const PLAYER_PORTAL_COOLDOWN: i32 = 10;

/// Whether `input` has nothing to do with where the player is, so that it is taken
/// across a change of dimension: a held slot, a creative slot, a swap of hands and
/// flying (the plan, section 4.2, R D11). Moves and what is aimed at blocks or
/// entities are of the old level.
fn has_no_place(input: &PlayerInput) -> bool {
    input.has_no_place()
}

/// `input` as a player in `mode` may make it, or none where the mode refuses it: a
/// creative slot or throw only with `instabuild` (`handleSetCreativeModeSlot @16`), and
/// flying only with `mayfly` (`handlePlayerAbilities @13-44`), where a client that says
/// it flies is not cut off but does not fly. The region asks again, since the mode may
/// change between this look and its tick (`docs/groundwork/v-plan.md`, section 4.1).
/// The connection's throttler of creative throws (`play.rs`) has counted a refused
/// throw already, where the game asks the mode first: only a client that sends
/// creative slots in survival, which a real one does not, is the slower for it.
fn allowed(mode: clustine_world::GameMode, input: PlayerInput) -> Option<PlayerInput> {
    let abilities = mode.abilities();
    match input {
        PlayerInput::SetInventorySlot { .. }
        | PlayerInput::SetCraftingSlot { .. }
        | PlayerInput::Throw { .. }
            if !abilities.instabuild =>
        {
            None
        }
        PlayerInput::SetFlying { flying } => Some(PlayerInput::SetFlying {
            flying: flying && abilities.may_fly,
        }),
        input => Some(input),
    }
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
    /// The dimension whose chunks the region holds, for its whole life: what every
    /// `ChunkPos` it sends and every one of `subscriptions` is of
    /// (`docs/groundwork/p-plan.md`, section 15.1).
    dimension: Dimension,
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

/// The blocks a tick changed, by chunk and section (its y in sections), each with its
/// last state.
type Changed = BTreeMap<(LevelChunk, i32), BTreeMap<BlockPos, BlockState>>;

/// How far a level event is seen, as the game's `ServerLevel.levelEvent` sends it.
const LEVEL_EVENTS_WITHIN: f64 = 64.0;
/// How far a block event is seen, as the game's `ServerLevel.runBlockEvents` sends it.
const BLOCK_EVENTS_WITHIN: f64 = 64.0;
/// How far a block's cracks are seen (`ServerLevel.destroyBlockProgress`'s 1024, the
/// square of 32).
const DESTRUCTION_WITHIN: f64 = 32.0;

/// The middle of the block at `position`.
fn block_centre(position: BlockPos) -> (f64, f64, f64) {
    (
        f64::from(position.x) + 0.5,
        f64::from(position.y) + 0.5,
        f64::from(position.z) + 0.5,
    )
}

pub(crate) struct Fanout {
    config: FanoutConfig,
    /// The region players enter the world in.
    spawn_region: RegionId,
    /// The world's spawn, which every client that enters is told.
    world_spawn: BlockPos,
    /// Who this edge is to the regions.
    identity: EdgeIdentity,
    /// What is kept for each region, by region id.
    regions: BTreeMap<RegionId, RegionPort>,
    /// Whom to tell once a drain is through: every region has confirmed all it was sent.
    draining: Option<tokio::sync::oneshot::Sender<()>>,
    /// Each region's clock as it last said, from which its players are told the time.
    clocks: BTreeMap<RegionId, WorldClock>,
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
    /// The latest stay of each player that the edge has been told of, by a region or
    /// as its own. Entities are numbered by the world store in the order stays begin,
    /// so the greater entity is the later stay.
    highest: BTreeMap<PlayerId, EntityId>,
    /// The latest stay of each player that this edge ended itself, by a leave or a
    /// second login, until a later stay of theirs is noted. Its region may still
    /// report it, as come (an arrival it takes in only now) or as moved, after the
    /// edge has hidden it, and none of that may show it again.
    ended: BTreeMap<PlayerId, EntityId>,
    /// The chunks the edge's players see, of every dimension.
    replica: BTreeMap<LevelChunk, ReplicaChunk>,
    /// A counter for the whole edge, raised by one every time it is read: what came
    /// later has the greater moment, whatever was removed from the replica and made
    /// anew in between.
    moment: u64,
    /// The entries whose light is not right for the chunks as they are: those that
    /// cannot be sent, and those that can and are a little old. Each has a job under
    /// way or gets one when a slot is free.
    marked: BTreeSet<LevelChunk>,
    /// Those of them that have no job under way, in the order they get one.
    queues: LightQueues,
    /// The jobs a thread has said it took, with the thread and when: a job is
    /// given up so long after that, and not after its dispatch, so that one that
    /// waits behind others is never given up.
    started: BTreeMap<u64, (usize, Instant)>,
    /// When the task last woke for light alone.
    light_ticked: Instant,
    /// How many jobs have failed, how many of them since that was last said in
    /// the log, and when that was: an error line a second at most.
    failed_jobs: u64,
    failed_unsaid: u64,
    failures_said: Option<Instant>,
    /// The jobs under way, by their moment, with the chunk each is for. One that its
    /// entry has forgotten stays here until it is answered, which bounds how many
    /// there are.
    jobs: BTreeMap<u64, LevelChunk>,
    /// Where jobs go: threads of the edge's own, or, in the edge's tests, the turn
    /// that dispatches them.
    lighting: Lighting,
    /// What the threads say. Taken by [`Fanout::run`].
    lit: Option<mpsc::UnboundedReceiver<FromThread>>,
    /// The entities in the chunks of the replica.
    entities: BTreeMap<EntityId, Shown>,
    /// Who rides what: each vehicle's passengers in order, by the vehicle's word, and
    /// the other way round (`docs/groundwork/n5-plan.md`).
    passengers: BTreeMap<EntityId, Vec<EntityId>>,
    vehicle_of: BTreeMap<EntityId, EntityId>,
    /// What each of this edge's players rides by their own region's word: what it said
    /// last of getting on and off, or what the hand-over that brought them said. It
    /// decides whether they are shown in a vehicle's seat, over the vehicle's word
    /// (`docs/groundwork/n5-plan.md`, section 8 item 4).
    rides_word: BTreeMap<EntityId, RidesWord>,
    /// Who holds each leashed entity's leash, by the leashee's region's word
    /// (`set_entity_link`, `docs/groundwork/n5-brief.md`, section 7.5).
    leashes: BTreeMap<EntityId, EntityId>,
    /// The ids this edge's clients know entities by.
    wire: WireIds,
    /// The regions that are no more, each with the region it went into, as far as this
    /// edge has acted on it. Never a chain: a value is not itself a key. Written by
    /// [`Fanout::retire`] and by nothing else; see
    /// `docs/adr/0015-the-edge-through-merges-and-splits.md`, section 1.
    stands_for: BTreeMap<RegionId, RegionId>,
    /// The block changes a region made after the beginning of its block events, by
    /// region, which go with its next tick's changes, as the game broadcasts them
    /// (`docs/adr/0026-redstone.md`, section 8).
    carried: BTreeMap<RegionId, Changed>,
    /// Where the seeds of the sounds this edge sends come from: any number will do,
    /// and none of the regions' rules draws it.
    sound_seeds: u64,
    /// The tick of each region's latest `TickDelta`, by the region it stands for.
    region_ticks: BTreeMap<RegionId, u64>,
    /// The region and tick of the report being handled, and whether the updates of
    /// that tick's entities have run: where a tracker made now begins.
    reporting: Option<Reporting>,
    /// The entities other than players that died, were picked up, despawned or merged,
    /// with their `hops`, kept so that a late report of an old holder does not bring
    /// them back (`docs/adr/0028-entities.md`, section 6).
    gone: BTreeMap<EntityId, Gone>,
}

/// The report the edge is handling: of `region`'s `tick`, before or after the updates
/// of that tick's entities ran (the game's `sendChanges`, which comes after the tick's
/// block changes are broadcast and before its entities tick).
#[derive(Debug, Clone, Copy)]
struct Reporting {
    region: RegionId,
    tick: u64,
    updated: bool,
}

/// An entity other than a player that is gone: the `hops` of its removal, and for each
/// region the edge has heard from since, the tick of the first report it heard of it
/// after the removal. It is forgotten once every region the edge hears from has
/// reported [`GONE_FOR`] ticks after that.
#[derive(Debug, Clone, Default)]
struct Gone {
    hops: u32,
    heard: BTreeMap<RegionId, u64>,
}

/// How many of a region's ticks a removal is kept after its first report following it.
const GONE_FOR: u64 = 600;

/// The ids by which this edge's clients know entities. An entity's key is unique for
/// the life of the world and does not fit the protocol's `i32`, so each edge gives the
/// entities it shows ids of its own, in order from 1 (`docs/adr/0028-entities.md`,
/// section 1). A key keeps its id while the edge shows its entity to anyone or a
/// connected player of the edge is that entity, whose own id the login told their
/// client. Ids are not given again for the life of the edge: a client may still name an
/// id it was told is removed, and until something a client sends names entities that is
/// harmless, but no id is reused before that is designed.
#[derive(Debug, Default)]
struct WireIds {
    of: BTreeMap<EntityId, i32>,
    /// The other way round, for what a client says of an entity.
    by_id: BTreeMap<i32, EntityId>,
    /// The ids of the parts of an entity of several parts, each with the entity and the
    /// part's number (`docs/groundwork/p-plan.md`, section 19.5 item 10).
    parts: BTreeMap<i32, (EntityId, u8)>,
    last: i32,
    #[cfg(test)]
    ledger: WireLedger,
}

impl WireIds {
    /// The id clients know the entity `key` by, given now if it has none.
    fn of(&mut self, key: EntityId) -> i32 {
        if let Some(id) = self.of.get(&key) {
            return *id;
        }
        self.last = self
            .last
            .checked_add(1)
            .expect("an edge shows fewer than 2^31 entities in its life");
        self.of.insert(key, self.last);
        self.by_id.insert(self.last, key);
        #[cfg(test)]
        self.ledger
            .lock()
            .expect("nothing panics holding the ledger")
            .insert(self.last, key);
        self.last
    }

    /// The id of the entity `key` of `parts` parts, given now with the ids of its parts
    /// right after it if it has none: the game numbers a dragon's parts from its own id
    /// on (`EnderDragon.setId`), and the client finds them so.
    fn run(&mut self, key: EntityId, parts: u8) -> i32 {
        if let Some(id) = self.of.get(&key) {
            return *id;
        }
        let id = self.of(key);
        for part in 0..parts {
            self.last = self
                .last
                .checked_add(1)
                .expect("an edge shows fewer than 2^31 entities in its life");
            self.parts.insert(self.last, (key, part));
        }
        id
    }

    fn forget(&mut self, key: EntityId) {
        if let Some(id) = self.of.remove(&key) {
            self.by_id.remove(&id);
            self.parts.retain(|_, (whose, _)| *whose != key);
        }
    }

    /// The entity clients know by `id` now, if any: one the edge no longer shows is no
    /// longer known by it, and what a client says of it is passed over, as the game
    /// passes over a use of an entity it does not find.
    fn key_of(&self, id: i32) -> Option<EntityId> {
        self.by_id.get(&id).copied()
    }

    /// The entity clients know by `id`, with the part `id` names of it where it is one
    /// of its parts' ids.
    fn part_of(&self, id: i32) -> Option<(EntityId, Option<u8>)> {
        match self.parts.get(&id) {
            Some((key, part)) => Some((*key, Some(*part))),
            None => self.key_of(id).map(|key| (key, None)),
        }
    }
}

/// The parts of an ender dragon, whose ids follow its own (`EnderDragon.subEntities`).
const DRAGON_PARTS: u8 = 8;

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
    /// The dimension `from` holds, which the entity is in: shown only to the views of
    /// that dimension (`docs/groundwork/p-plan.md`, section 4.1).
    dimension: Dimension,
    /// What the edge sends of it as it moves, for an entity other than a player.
    tracker: Option<Tracker>,
    /// For an entity other than a player that its region sent across a line, the changes
    /// of holder it had then: it is shown where it was sent until the region it went to
    /// reports it with more (`docs/adr/0028-entities.md`, section 6).
    handed_on: Option<u32>,
    /// What clients were last sent of a mob's attributes and equipment, which a client
    /// that comes to see it is sent with its spawn (`sendPairingData`).
    living: LivingShown,
}

/// A mob's attributes, by their ids, and what it wears, by slot, as last sent; and the
/// bed a player sleeps in.
#[derive(Debug, Clone, Default, PartialEq)]
struct LivingShown {
    attributes: BTreeMap<i32, AttributeSnapshot>,
    equipment: BTreeMap<u8, WireStack>,
    sleeping: Option<Position>,
    /// A player's values its region reports apart (`RegionEvent::PlayerData`: the
    /// living flags of a use), by index.
    data: BTreeMap<u8, EntityDataValue>,
}

impl Fanout {
    pub(crate) fn new(
        config: FanoutConfig,
        routing: Routing,
        commands: mpsc::Receiver<Command>,
        (lighting, lit): (Lighting, mpsc::UnboundedReceiver<FromThread>),
    ) -> Self {
        let spawn_region = routing.home;
        let wire = WireIds {
            #[cfg(test)]
            ledger: Arc::clone(&config.wire_ledger),
            ..WireIds::default()
        };
        // The messages of all regions are read from one queue, each link's in the order
        // they were sent.
        let (queue, messages) = mpsc::channel(REGION_QUEUE_CAPACITY);
        Self {
            config,
            spawn_region,
            world_spawn: routing.spawn,
            identity: routing.identity,
            regions: BTreeMap::new(),
            draining: None,
            clocks: BTreeMap::new(),
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
            highest: BTreeMap::new(),
            ended: BTreeMap::new(),
            replica: BTreeMap::new(),
            moment: 0,
            marked: BTreeSet::new(),
            queues: LightQueues::default(),
            started: BTreeMap::new(),
            light_ticked: Instant::now(),
            failed_jobs: 0,
            failed_unsaid: 0,
            failures_said: None,
            jobs: BTreeMap::new(),
            lighting,
            lit: Some(lit),
            entities: BTreeMap::new(),
            passengers: BTreeMap::new(),
            vehicle_of: BTreeMap::new(),
            rides_word: BTreeMap::new(),
            leashes: BTreeMap::new(),
            wire,
            stands_for: BTreeMap::new(),
            carried: BTreeMap::new(),
            sound_seeds: 0,
            region_ticks: BTreeMap::new(),
            reporting: None,
            gone: BTreeMap::new(),
        }
    }

    /// Serves until the edge has to stop. Every player is disconnected when this
    /// returns, because their queues are dropped. A region whose link ends is waited
    /// for: what its players do is kept and sent when there is a link to it again.
    pub(crate) async fn run(mut self) -> Stopped {
        let mut messages = self.messages.take().expect("the fan-out task runs once");
        let mut lit = self.lit.take().expect("the fan-out task runs once");
        for link in std::mem::take(&mut self.first_links) {
            self.take_link(link).await;
        }
        let mut patience = tokio::time::interval(PATIENCE_CHECK);
        patience.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            // Only while something is held, rests or is with a thread: an edge
            // where light is at rest has no timer for it.
            let wake = self.light_wake();
            // A turn is timed from when what it is for came, not from when the task
            // began to wait for it.
            let began;
            tokio::select! {
                command = self.commands.recv() => {
                    began = std::time::Instant::now();
                    match command {
                        Some(command) => self.handle_command(command).await,
                        None => return Stopped::Abandoned,
                    }
                },
                message = messages.recv() => {
                    began = std::time::Instant::now();
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
                Some(relink) = self.relinks.recv() => {
                    began = std::time::Instant::now();
                    match relink {
                        Relink::Link(link) => self.take_link(link).await,
                        Relink::Absorbed(pairs) => self.take_pairs(pairs).await,
                        Relink::Order { player, order } => self.take_order(&player, order).await,
                    }
                },
                // The task holds the other end's sender in `lighting`, so this never
                // ends.
                Some(said) = lit.recv() => {
                    began = std::time::Instant::now();
                    self.take_light(said).await;
                }
                _ = tokio::time::sleep_until(wake.unwrap_or_else(Instant::now)), if wake.is_some() => {
                    began = std::time::Instant::now();
                    self.light_tick().await;
                }
                _ = patience.tick() => {
                    began = std::time::Instant::now();
                    self.drop_the_overdue().await;
                    self.ask_again_where_due().await;
                }
            }
            // At the end of the turn, so that all the block changes of one tick, which
            // come in one message, are in the replica before a job is made from it.
            self.light_the_marked(&mut lit).await;
            figures::turn(began);
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
        // Who waits for a chunk is who has it pending, and a packet is kept only for
        // someone who waits (`docs/adr/0023-light-at-the-edge.md`, sections 1 and 4).
        for (position, entry) in &self.replica {
            let pending = |(player, view): (&PlayerId, &PlayerView)| {
                view.has_pending(*position).then_some(*player)
            };
            let waiting: BTreeSet<PlayerId> = self.players.iter().filter_map(pending).collect();
            assert_eq!(entry.waiters, waiting, "the waiters of {position:?}");
            assert!(
                entry.packet.is_none() || !entry.waiters.is_empty(),
                "{position:?} keeps a packet nobody waits for"
            );
            // An entry whose light is not right is marked, and has a job or is to
            // get one; and a kept packet is of light that may be sent.
            if entry.chunk.is_some() && !entry.current() {
                assert!(self.marked.contains(position), "{position:?} is not marked");
            }
            assert!(
                entry.packet.is_none() || entry.sendable(),
                "{position:?} keeps a packet of light that may not be sent"
            );
        }
        for position in &self.marked {
            let entry = self.replica.get(position);
            assert!(
                entry.is_some_and(|entry| entry.chunk.is_some()),
                "{position:?} is marked and has no chunk"
            );
        }
        // A marked entry has a job under way or is in one of the three sets, in the
        // one it belongs in, and nothing else is in any.
        let mut queued = 0;
        for (position, entry) in &self.replica {
            let idle = self.marked.contains(position) && entry.job.is_none();
            assert_eq!(
                entry.queued.is_some(),
                idle,
                "{position:?} {:?}",
                entry.queued
            );
            let Some(place) = entry.queued else {
                continue;
            };
            queued += 1;
            let waited_for = !entry.sendable() && !entry.waiters.is_empty();
            match place {
                Queued::Waiting { rank, since, by } => {
                    assert!(waited_for, "{position:?} is among the waited-for");
                    assert!(self.queues.waiting.contains(&(rank, since, *position)));
                    let near =
                        |waiter: &PlayerId| apart(position.position, self.players[waiter].center);
                    let nearest = entry.waiters.iter().map(near).min();
                    assert_eq!(Some(rank), nearest, "the rank of {position:?}");
                    assert_eq!(near(&by), rank, "{position:?} is ranked by {by:?}");
                }
                Queued::Unawaited {
                    rank,
                    since,
                    marked,
                } => {
                    assert!(!waited_for, "{position:?} is waited for");
                    assert!(self.queues.unawaited.contains(&(rank, since, *position)));
                    assert!(self.queues.oldest.contains(&(marked, since, *position)));
                }
                Queued::Later { until } => {
                    assert!(self.queues.later.contains(&(until, *position)));
                }
            }
        }
        assert_eq!(
            self.queues.len(),
            queued,
            "the sets of light hold what is queued"
        );
        assert_eq!(self.queues.oldest.len(), self.queues.unawaited.len());
        for basis in self.started.keys() {
            assert!(
                self.jobs.contains_key(basis),
                "job {basis} is no longer under way"
            );
        }
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
                    .get(&LevelChunk::new(port.dimension, *chunk))
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
        let RegionLink {
            region,
            dimension,
            epoch,
            end,
        } = link;
        if self.stands_for.contains_key(&region) {
            debug!(%region, epoch, "not taking a link to a region that is no more");
            return;
        }
        let port = self.regions.entry(region).or_default();
        // A region's dimension is its for life; the link says it as the routing table
        // has it.
        port.dimension = dimension;
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
            Welcome::Resumed { .. } => {
                // Player events are not sent again on a link that resumes, so what the
                // region said of menus on the link that ended may be lost (section 7
                // item 7).
                self.resync_menus(from).await;
            }
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
                // A travelling player as a joining one: the destination places them,
                // or they are put out, and placed by their stay when they come back
                // (`docs/groundwork/p-plan.md`, section 4.2, R N3).
                let travelling = view.travelling.map(|travelling| travelling.since);
                let since = [view.joining_since, waiting, travelling]
                    .into_iter()
                    .flatten()
                    .min();
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
                appearance,
                outbound,
                awaiting_teleport,
            } => {
                let player = PlayerId(profile.uuid);
                if let Some(earlier) = self.players.get(&player) {
                    // A second login puts the first out, as on the official server:
                    // the earlier connection is told why and its stay is left, by a
                    // leave that is in front of the join on the link to the region
                    // (`docs/adr/0020-one-stay-per-player.md`, section 4, step 1).
                    put_out(&earlier.outbound);
                    self.remove_player(player).await;
                }
                let view_distance =
                    self.granted_view_distance(requested_view_distance.unwrap_or(i32::MAX));
                self.players.insert(
                    player,
                    PlayerView {
                        travelling: None,
                        respawn_owed: None,
                        last_death: None,
                        mode: Default::default(),
                        session,
                        name: profile.name.clone(),
                        outbound,
                        awaiting_teleport,
                        teleports: 0,
                        entity: None,
                        region: self.spawn_region,
                        passed_on: 0,
                        joining_since: Some(Instant::now()),
                        inputs_sent: 0,
                        kept_inputs: VecDeque::new(),
                        menus: Default::default(),
                        told_clocks: None,
                        handled: None,
                        under_way: BTreeSet::new(),
                        acknowledged: None,
                        view_distance,
                        // The overworld's until a region places the player: a stay in
                        // another dimension is placed by its dimension's home, which
                        // sends it there (`docs/groundwork/p-plan.md`, section 4.2).
                        dimension: Dimension::Overworld,
                        wanted: BTreeSet::new(),
                        center: ChunkPos::new(0, 0),
                        pending: BTreeSet::new(),
                        sent: BTreeSet::new(),
                        replace: BTreeSet::new(),
                        visible: BTreeMap::new(),
                        listed: BTreeMap::new(),
                        batch_outstanding: false,
                        batch_size: INITIAL_BATCH_SIZE,
                    },
                );
                let join = PlayerJoin {
                    player,
                    name: profile.name,
                    attempt: session.0,
                    appearance,
                };
                self.send_to_region(self.spawn_region, EdgeToWorker::PlayerJoin(join))
                    .await;
            }
            Command::OnEntity {
                session,
                player,
                id,
                what,
            } => {
                let Some((entity, part)) = self.wire.part_of(id) else {
                    return;
                };
                let input = match what {
                    OnEntity::Attack => PlayerInput::Attack { part, entity },
                    OnEntity::Pick => PlayerInput::PickItemFromEntity { entity },
                    OnEntity::Interact {
                        location,
                        sneaking,
                        off_hand,
                    } => {
                        let used = PlayerInput::Interact {
                            entity,
                            location,
                            sneaking,
                        };
                        if off_hand {
                            PlayerInput::WithOffHand(Box::new(used))
                        } else {
                            used
                        }
                    }
                };
                self.player_input(session, player, input).await;
            }
            Command::Input {
                session,
                player,
                input,
            } => self.player_input(session, player, input).await,
            Command::Menu {
                session,
                player,
                what,
            } => {
                if self.session_matches(player, session) {
                    let view = self.players.get_mut(&player).expect("session matched");
                    // The number `player_input` gives the input it is passed next.
                    let number = view.inputs_sent + 1;
                    let (input, packets) = match what {
                        MenuCommand::Click(packet) => {
                            (view.menus.click(packet, number), Vec::new())
                        }
                        MenuCommand::Close => {
                            let (input, packets) = view.menus.close(number);
                            (Some(input), packets)
                        }
                        MenuCommand::SelectTrade { item } => {
                            (view.menus.select_trade(item), Vec::new())
                        }
                    };
                    if let Some(input) = input {
                        self.player_input(session, player, input).await;
                    }
                    if !packets.is_empty() {
                        self.send_to_player(player, packets).await;
                    }
                }
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
            Command::Drain { done } => {
                let players: Vec<PlayerId> = self.players.keys().copied().collect();
                for player in players {
                    self.remove_player(player).await;
                }
                self.draining = Some(done);
                self.check_drained();
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
            Command::ViewDistance {
                session,
                player,
                requested,
            } => {
                if self.session_matches(player, session) {
                    self.change_view_distance(player, requested).await;
                }
            }
        }
    }

    /// The view distance a client that asks for `requested` is granted: what it asks
    /// for, within what the server grants anyone, as on the official server.
    fn granted_view_distance(&self, requested: i32) -> i32 {
        requested.clamp(2, self.config.view_distance)
    }

    /// Grows or shrinks a player's view to what their client now asks for, as a move
    /// does: the chunks that came into view are asked for and sent, and those that left
    /// it are forgotten. A player who is not in the world yet is only noted; their view
    /// is laid out when they enter.
    async fn change_view_distance(&mut self, player: PlayerId, requested: i32) {
        let granted = self.granted_view_distance(requested);
        let Some(view) = self.players.get_mut(&player) else {
            return;
        };
        if view.view_distance == granted {
            return;
        }
        view.view_distance = granted;
        if view.wanted.is_empty() {
            return;
        }
        let center = view.center;
        let packets = self.reshape_view(player, center).await;
        if self.send_to_player(player, packets).await {
            self.send_chunks(player).await;
        }
    }

    /// Handles what the region `from` sent over its current link. Returns why the edge
    /// has to stop, if it has to.
    async fn handle_region(&mut self, from: RegionId, message: WorkerToEdge) -> Option<Stopped> {
        let message = self.held_while_respawning(message)?;
        match message {
            WorkerToEdge::Elsewhere { chunk, ask, region } => {
                let holder = self.living(region);
                self.elsewhere(from, chunk, ask, holder).await;
            }
            WorkerToEdge::NotMine { chunk, ask } => self.not_mine(from, chunk, ask).await,
            WorkerToEdge::Clock { clock, jumped, .. } => {
                self.clocks.insert(from, clock);
                // Not a client between its `Respawn` and its placing, which the game
                // sends in one tick: its placing tells it the new level's time whole.
                let players: Vec<PlayerId> = self
                    .players
                    .iter()
                    .filter(|(_, view)| {
                        view.region == from && view.entity.is_some() && view.travelling.is_none()
                    })
                    .map(|(player, _)| *player)
                    .collect();
                for player in players {
                    self.tell_time(player, from, jumped).await;
                }
            }
            WorkerToEdge::Welcome(welcome) => return self.welcomed(from, welcome).await,
            WorkerToEdge::ToPlayer {
                player,
                event:
                    PlayerEvent::Spawned {
                        attempt,
                        entity_id,
                        pose,
                        flying,
                        inventory,
                        selected_slot,
                        riding,
                        last_death,
                        mode,
                    },
            } => {
                // Only the connection whose join this answers enters by it. An answer
                // to an earlier join of the player, read by a later connection, would
                // give that one an entity its region has ended since
                // (`docs/adr/0020-one-stay-per-player.md`, section 4.1).
                let theirs = self
                    .players
                    .get(&player)
                    .is_some_and(|view| view.entity.is_none() && view.session.0 == attempt);
                if theirs {
                    let entering = Entering {
                        entity: entity_id,
                        pose,
                        flying,
                        inventory: inventory.clone(),
                        selected_slot,
                        riding,
                        last_death,
                        mode,
                    };
                    self.spawn_player(player, entering).await;
                }
            }
            WorkerToEdge::ToPlayer {
                player,
                event: PlayerEvent::Acknowledged { sequence },
            } => self.handled(player, sequence).await,
            WorkerToEdge::ToPlayer {
                player,
                event: PlayerEvent::InventorySlot { slot, stack },
            } => self.inventory_slot(player, slot, stack).await,
            WorkerToEdge::ToPlayer {
                player,
                event: PlayerEvent::BlockShown { position, state },
            } => {
                // A block of another dimension than the one the client shows now is
                // not shown (`docs/groundwork/p-plan.md`, section 4.1).
                let dimension = self.dimension_of(from);
                let shows = self
                    .players
                    .get(&player)
                    .is_some_and(|view| view.dimension == dimension);
                if shows {
                    let packet = self.shown_block(dimension, position, state);
                    self.send_to_player(player, [packet]).await;
                }
            }
            WorkerToEdge::ToPlayer {
                player,
                event: PlayerEvent::HeldSlot { slot },
            } => self.held_slot(player, slot).await,
            WorkerToEdge::ToPlayer {
                player,
                event:
                    PlayerEvent::GlobalLevelEvent {
                        event,
                        position,
                        data,
                    },
            } => {
                let packet = encoded(&LevelEvent {
                    event,
                    position: Position {
                        x: position.x,
                        y: position.y,
                        z: position.z,
                    },
                    data,
                    global: true,
                });
                self.send_to_player(player, [packet]).await;
            }
            WorkerToEdge::ToPlayer {
                player,
                event:
                    PlayerEvent::Health {
                        health,
                        food,
                        saturation,
                    },
            } => {
                let packet = encoded(&SetHealth {
                    health,
                    food,
                    saturation,
                });
                self.send_to_player(player, [packet]).await;
            }
            // A knock the player's region gave them (`markHurt`, or another player's hit):
            // their client takes its motion only from the server's `SetEntityMotion` of
            // its own entity (`ServerEntity.sendChanges @960-993`).
            WorkerToEdge::ToPlayer {
                player,
                event: PlayerEvent::Knocked { motion },
            } => {
                if let Some(entity) = self.players.get(&player).and_then(|view| view.entity) {
                    let packet = encoded(&clustine_protocol::packets::play::SetEntityMotion {
                        entity_id: self.wire.of(entity),
                        motion,
                    });
                    self.send_to_player(player, [packet]).await;
                }
            }
            WorkerToEdge::ToPlayer {
                player,
                event: PlayerEvent::OwnData(data),
            } => self.show_own_data(player, data).await,
            WorkerToEdge::ToPlayer {
                player,
                event: PlayerEvent::ArrowHitPlayer,
            } => {
                let packet = encoded(&GameEvent {
                    event: game_event::PLAY_ARROW_HIT_SOUND,
                    value: 0.0,
                });
                self.send_to_player(player, [packet]).await;
            }
            WorkerToEdge::ToPlayer {
                player,
                event: PlayerEvent::HurtAnimation { yaw },
            } => {
                if let Some(entity) = self.players.get(&player).and_then(|view| view.entity) {
                    let packet = encoded(&clustine_protocol::packets::play::HurtAnimation {
                        entity_id: self.wire.of(entity),
                        yaw,
                    });
                    self.send_to_player(player, [packet]).await;
                }
            }
            WorkerToEdge::ToPlayer {
                player,
                event: PlayerEvent::Message { key, args, overlay },
            } => {
                let packet = encoded(&SystemChat::translated(&key, &args, overlay));
                self.send_to_player(player, [packet]).await;
            }
            WorkerToEdge::ToPlayer {
                player,
                event: PlayerEvent::Teleported { pose },
            } => self.rides(player, pose).await,
            // A merchant's window and a raid's bar (`merchant.rs`, `boss_bars.rs`).
            WorkerToEdge::ToPlayer {
                player,
                event:
                    PlayerEvent::MerchantOffers {
                        serial,
                        offers,
                        level,
                        xp,
                        show_progress,
                        can_restock,
                    },
            } => {
                let packet = self.players.get(&player).and_then(|view| {
                    crate::merchant::offers(
                        &view.menus,
                        serial,
                        &offers,
                        level,
                        xp,
                        show_progress,
                        can_restock,
                    )
                });
                if let Some(packet) = packet {
                    self.send_to_player(player, [packet]).await;
                }
            }
            WorkerToEdge::ToPlayer {
                player,
                event: PlayerEvent::BossBar { id, change },
            } => {
                if let Some(packet) = crate::boss_bars::boss_bar(id, change) {
                    self.send_to_player(player, [packet]).await;
                }
            }
            WorkerToEdge::ToPlayer {
                player,
                event:
                    event @ (PlayerEvent::EffectAdded { .. }
                    | PlayerEvent::EffectRemoved { .. }
                    | PlayerEvent::Attributes(_)),
            } => self.own_effects(player, event).await,
            WorkerToEdge::ToPlayer {
                player,
                event: PlayerEvent::GameModeChanged { mode, flying },
            } => self.game_mode_changed(player, mode, flying).await,
            WorkerToEdge::ToPlayer {
                player,
                event: PlayerEvent::UseCompleted,
            } => {
                // Entity event 9 of their own entity, to them alone.
                if let Some(entity) = self.players.get(&player).and_then(|view| view.entity) {
                    let packet = encoded(&EntityEvent {
                        entity_id: self.wire.of(entity),
                        event: 9,
                    });
                    self.send_to_player(player, [packet]).await;
                }
            }
            WorkerToEdge::ToPlayer {
                player,
                event: PlayerEvent::Cooldown { group, ticks },
            } => {
                let packet = encoded(&Cooldown { group, ticks });
                self.send_to_player(player, [packet]).await;
            }
            WorkerToEdge::ToPlayer {
                player,
                event: PlayerEvent::Experience(experience),
            } => {
                let packet = encoded(&SetExperience {
                    progress: experience.progress,
                    level: experience.level,
                    total: experience.total,
                });
                self.send_to_player(player, [packet]).await;
            }
            WorkerToEdge::ToPlayer {
                player,
                event:
                    event @ (PlayerEvent::MenuOpened { .. }
                    | PlayerEvent::MenuChanged { .. }
                    | PlayerEvent::MenuClosed { .. }
                    | PlayerEvent::NoMenu { .. }),
            } => self.menu_event(from, player, event).await,
            // Shown from the steps of N5 that make them (`docs/groundwork/n5-plan.md`).
            WorkerToEdge::ToPlayer {
                player,
                event: PlayerEvent::Rides { vehicle, pose },
            } => {
                self.rides(player, pose).await;
                if let Some(rider) = self.players.get(&player).and_then(|view| view.entity) {
                    self.take_rides_word(rider, vehicle).await;
                }
            }
            WorkerToEdge::ToPlayer {
                player,
                event:
                    PlayerEvent::VehicleCorrected {
                        position,
                        yaw,
                        pitch,
                    },
            } => self.vehicle_corrected(player, position, yaw, pitch).await,
            WorkerToEdge::ToPlayer {
                player,
                event: PlayerEvent::Kicked { reason },
            } => self.kick(player, reason).await,
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
                tick,
            } => {
                if self.served(from, position, ask) {
                    // A snapshot is the chunk after its tick: an entity first seen in it
                    // is updated from the next tick on.
                    let region = self.living(from);
                    self.reporting = Some(Reporting {
                        region,
                        tick,
                        updated: true,
                    });
                    self.take_snapshot(from, position, chunk, entities).await;
                    self.reporting = None;
                }
            }
            WorkerToEdge::TickDelta { tick, events } => {
                // The changes of blocks of one tick go out together, those of one
                // section in one packet, as the game's `ChunkHolder.broadcastChanges`
                // sends them: a tick can change thousands (the leaves of a forest set
                // right as it is loaded), which one packet each would not fit in a
                // client's queue. The game broadcasts them before its block events, and
                // what changes from those on with the next tick's: a region's changes
                // after its beginning of block events wait for its next tick, behind
                // which this tick's own come (`docs/adr/0026-redstone.md`, section 8).
                let mut changed = self.carried.remove(&from).unwrap_or_default();
                let mut later = Changed::new();
                let mut begun = false;
                // The entities other than players are updated for the ticks the region
                // said nothing in, and for this one where the game does it: after the
                // block changes are broadcast, before the entities tick
                // (`docs/groundwork/n4-plan.md`, section 7).
                let region = self.living(from);
                self.heard_tick(region, tick);
                self.update_entities(region, tick, true).await;
                self.reporting = Some(Reporting {
                    region,
                    tick,
                    updated: false,
                });
                for event in events {
                    match event {
                        RegionEvent::BlockChanged {
                            position,
                            state,
                            seen,
                        } => {
                            let dimension = self.dimension_of(from);
                            if self.change_block(dimension, position, state) {
                                let into = if begun { &mut later } else { &mut changed };
                                let chunk = LevelChunk::new(dimension, position.chunk());
                                let section = (chunk, position.y.div_euclid(16));
                                if seen {
                                    into.entry(section).or_default().insert(position, state);
                                } else if let Some(waiting) = into
                                    .get_mut(&section)
                                    .and_then(|blocks| blocks.get_mut(&position))
                                {
                                    // Set without `UPDATE_CLIENTS`, it is not sent for
                                    // itself; but a block waiting to be sent goes as it
                                    // is when it goes, as the game reads it then.
                                    *waiting = state;
                                }
                            }
                        }
                        RegionEvent::BlockEventsBegin => {
                            if !begun {
                                begun = true;
                                self.show_changes(std::mem::take(&mut changed)).await;
                                self.update_entities(region, tick, false).await;
                                self.reporting = Some(Reporting {
                                    region,
                                    tick,
                                    updated: true,
                                });
                            }
                        }
                        event => self.handle_event(from, event).await,
                    }
                }
                self.show_changes(changed).await;
                if !begun {
                    self.update_entities(region, tick, false).await;
                }
                self.reporting = None;
                if !later.is_empty() {
                    self.carried.insert(from, later);
                }
            }
            // A region says these through its outbox, so that they reach the edge even
            // if the region's owner dies right after.
            message @ (WorkerToEdge::Remote(_)
            | WorkerToEdge::RemoteDone { .. }
            | WorkerToEdge::ToPlayer {
                event: PlayerEvent::Departed(_) | PlayerEvent::Refused | PlayerEvent::Placed { .. },
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
            Durable::Placed {
                player,
                entity,
                dimension,
                pose,
                flying,
                respawn,
                gateway,
            } => {
                if self.has_stay(player, entity) {
                    self.placed(player, dimension, pose, flying, respawn, gateway)
                        .await;
                }
            }
            Durable::Died {
                player,
                entity,
                message,
                at,
            } => {
                if self.has_stay(player, entity) {
                    if let Some(view) = self.players.get_mut(&player) {
                        view.last_death = Some(at);
                    }
                    self.died(player, entity, &message).await;
                    self.owe_respawn(player);
                }
            }
            Durable::WonGame { player, entity } => {
                if self.has_stay(player, entity) {
                    self.owe_respawn(player);
                    let packet = encoded(&GameEvent {
                        event: game_event::WIN_GAME,
                        value: 0.0,
                    });
                    self.send_to_player(player, [packet]).await;
                }
            }
            Durable::Refused { player, attempt } => {
                // A player who has an entity is in the world through another way than
                // the join that was refused, and stays; and so does a connection whose
                // join is another than the one refused.
                let waiting = self
                    .players
                    .get(&player)
                    .filter(|view| view.entity.is_none() && view.session.0 == attempt);
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
                let serves = (self.replica.get(&self.level_of(from, chunk)))
                    .and_then(|entry| entry.served_by);
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
                    self.discard(holder, player, &transfer, chunk).await;
                    self.remove_player(player).await;
                } else {
                    self.hand_over(player, from, holder, transfer, back).await;
                }
            }
            // A client numbers its actions afresh with every connection, so the word
            // of an earlier stay's action must not end the wait for a later one's
            // with the same number (`docs/adr/0020-one-stay-per-player.md`, section 11).
            Durable::RemoteDone {
                player,
                entity,
                sequence,
                shown,
            } => {
                if self.has_stay(player, entity) {
                    // The blocks of a click first, as the game sends them before it
                    // acknowledges the click (`handleUseItemOn`); none of another
                    // dimension than the client shows now, whose answer is taken all
                    // the same (`docs/groundwork/p-plan.md`, section 4.1, R N6).
                    let dimension = self.dimension_of(from);
                    let shows =
                        (self.players.get(&player)).is_some_and(|view| view.dimension == dimension);
                    let packets: Vec<_> = shown
                        .iter()
                        .filter(|_| shows)
                        .map(|(position, state)| self.shown_block(dimension, *position, *state))
                        .collect();
                    if !packets.is_empty() {
                        self.send_to_player(player, packets).await;
                    }
                    self.arrived(player, sequence).await;
                }
            }
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
            // A later stay of the player has taken this one's place. If it is the stay
            // of the connection the edge has, that connection is told why and ends;
            // otherwise the entry is about one the edge has ended itself
            // (`docs/adr/0020-one-stay-per-player.md`, section 6).
            Durable::Ended {
                player,
                entity,
                attempt,
            } => {
                let theirs = self.players.get(&player).filter(|view| match view.entity {
                    Some(shown) => shown == entity,
                    None => attempt == Some(view.session.0),
                });
                if let Some(view) = theirs {
                    put_out(&view.outbound);
                    self.remove_player(player).await;
                }
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
        // Only for the stay that acted: what an earlier stay of the player did is
        // nobody's to hear of any more.
        let acting = self
            .players
            .get_mut(&player)
            .filter(|view| view.entity == Some(action.entity));
        let Some(view) = acting else {
            // Nobody is left to be told how it ended.
            return;
        };
        // A step that answers no action holds no acknowledgement back, and nobody
        // answers it.
        if sequence != RemoteAction::UNANSWERED {
            view.under_way.insert(sequence);
        }
        let Some(to) = to.filter(|to| *to != from || back) else {
            debug!(%from, ?to, "an action on blocks has nowhere to go and ends here");
            self.arrived(player, sequence).await;
            return;
        };
        let chunk = action.step.concerns().chunk();
        let watched = self
            .replica
            .get(&self.level_of(to, chunk))
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
        // The region holds the stay of a join as entering and waits for the store's
        // word. The connection of that join goes on waiting, where "absent" would put
        // it out; for anyone else there is nothing to do
        // (`docs/adr/0020-one-stay-per-player.md`, section 4.2).
        if matches!(answer, Presence::Entering { .. }) {
            return;
        }
        let Presence::Present {
            entity,
            pose,
            inventory,
            selected_slot,
            last_input,
            handled,
            flying,
            attempt,
            riding,
            last_death,
            mode,
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
        // The stay of this connection's join, which the edge has not been told the
        // entity of: it carries the join's attempt until its first input. Wherever
        // it is by now (a split or a merge can have moved it), it is the view's
        // (section 4.3, path 1).
        let of_this_join = self
            .players
            .get(&player)
            .is_some_and(|view| view.entity.is_none() && attempt == Some(view.session.0));
        match stay {
            Some((None, _)) if of_this_join => {
                if let Some(view) = self.players.get_mut(&player) {
                    view.region = from;
                }
                let entering = Entering {
                    entity,
                    pose,
                    flying,
                    inventory: inventory.clone(),
                    selected_slot,
                    riding,
                    last_death,
                    mode,
                };
                self.spawn_player(player, entering).await;
            }
            // The stay the edge has, where it has it. A riding word lost with the link
            // is mended (`docs/groundwork/n7h-logout-on-a-mount.md`, section 6); one the
            // edge has had since is not unsaid by the durable tick's.
            Some((Some(shown), region)) if shown == entity && region == from => {
                if riding.is_some() {
                    self.take_rides_word(entity, riding).await;
                }
            }
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
            // A player who is entering the world, and a stay that is not of their
            // join. If the join is still among what is to be sent to the region again,
            // the region had not applied it when it made this answer, which is then
            // about who they were before: a stay the join will end, and nothing is
            // done. Otherwise it is a stay the edge does not have, as below.
            Some((None, region))
                if region == from && {
                    let port = &self.regions.entry(from).or_default();
                    port.kept.iter().any(|(_, body)| {
                        matches!(body, EdgeToWorker::PlayerJoin(join) if join.player == player)
                    })
                } =>
            {
                return;
            }
            // A stay the edge does not have: of a player it has given up, or has as
            // another entity. It is the region's stay that ends, by a leave that names
            // it, unless one is on its way already.
            _ => {
                let port = &self.regions.entry(from).or_default();
                let leaving = port.kept.iter().any(|(_, body)| {
                    matches!(
                        body,
                        EdgeToWorker::PlayerLeave { player: left, entity: named, .. }
                            if *left == player && *named == Some(entity)
                    )
                });
                if !leaving {
                    debug!(%from, entity = entity.0, "a region has a stay this edge does not; ending it");
                    let leave = EdgeToWorker::PlayerLeave {
                        player,
                        entity: Some(entity),
                        attempt: None,
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

    /// The dimension of `region`, as its link said: a region's never changes, and a
    /// merge is of two regions of one dimension. The overworld's for a region the edge
    /// has had no link to.
    fn dimension_of(&self, region: RegionId) -> Dimension {
        self.regions
            .get(&self.living(region))
            .map_or(Dimension::Overworld, |port| port.dimension)
    }

    /// The chunk `chunk` of `region`'s dimension: what every chunk a region names is of.
    fn level_of(&self, region: RegionId, chunk: ChunkPos) -> LevelChunk {
        LevelChunk::new(self.dimension_of(region), chunk)
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
        // It sends no next tick: what it changed after its last block events goes now.
        if let Some(carried) = self.carried.remove(&region) {
            self.show_changes(carried).await;
        }
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
                .get(&self.level_of(into, chunk))
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
        players: Vec<(PlayerId, EntityId, Option<u64>)>,
    ) {
        info!(%from, %part, players = players.len(), "a region has been split");
        if part == from {
            // The part has gone back into the region since.
            return;
        }
        for (player, entity, attempt) in players {
            // The stay of a join the edge has not been told the entity of went with
            // the part: the view is the part's from now on, and nothing else, as it
            // sees nothing yet. The part's hello names the player, and its answer
            // enters them (`docs/adr/0020-one-stay-per-player.md`, section 4.3, path 2).
            if let Some(view) = self.players.get_mut(&player)
                && view.entity.is_none()
                && view.region == from
                && attempt == Some(view.session.0)
            {
                view.region = part;
                continue;
            }
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
        let dimension = self.dimension_of(to);
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
            shown.dimension = dimension;
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
        // What the region the stay moved to says of the menu may have come before the
        // edge knew it stood for the player's, and been passed over (section 9 item 3).
        // Named: a report of window 0 passed over so is not asked for again unless a
        // menu is shown (the V3 review's finding 9 asks for it always); the slot is put
        // right by its next change or the next whole, and this needs a change of window
        // 0 in the very ticks of a merge or a split.
        let shown = self
            .players
            .get(&player)
            .filter(|view| view.menus.serial().is_some())
            .map(|view| view.session);
        if let Some(session) = shown {
            self.player_input(session, player, PlayerInput::MenuResync)
                .await;
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
    /// Tells whoever drains the edge that it is through, once no region has anything of
    /// the edge's left to apply and make durable.
    fn check_drained(&mut self) {
        if self.regions.values().all(|port| port.kept.is_empty())
            && let Some(done) = self.draining.take()
        {
            let _ = done.send(());
        }
    }

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
        self.check_drained();
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

    /// Tells the client of `player` what a slot of its inventory holds now that its
    /// region changed it. Only a client that has entered the world is told: one that
    /// has not is sent the whole inventory when it enters, as the region has it then.
    async fn inventory_slot(&mut self, player: PlayerId, slot: u8, stack: Option<ItemStack>) {
        if let Some(view) = self.players.get_mut(&player) {
            view.menus.inventory_slot(usize::from(slot), stack.clone());
        }
        let entered = self
            .players
            .get(&player)
            .is_some_and(|view| view.entity.is_some());
        if entered {
            let packet = inventory_slot_packet(slot, stack);
            self.send_to_player(player, [packet]).await;
        }
    }

    /// Sends an explosion to every player of this edge within 64 blocks of its centre,
    /// whatever region they are in, each with the push `knockback` names for them or
    /// none (`ServerLevel.explode`; `docs/groundwork/n6-brief.md`, finding 2.9).
    async fn show_explosion(
        &mut self,
        centre: [f64; 3],
        radius: f32,
        blocks: i32,
        knockback: &[(EntityId, [f64; 3])],
        (large, kind): (bool, clustine_sim::api::ExplosionKind),
        sound: bool,
    ) {
        use clustine_protocol::packets::play::{EXPLOSION_BLOCK_PARTICLES, Explode, particle};
        use clustine_sim::api::ExplosionKind;
        // `ServerLevel.explode`'s particles, sound and block particles, by the kind.
        let (small_particle, large_particle, shown_sound, block_particles) = match kind {
            ExplosionKind::Blast => (
                particle::EXPLOSION,
                particle::EXPLOSION_EMITTER,
                clustine_data::sounds::ENTITY_GENERIC_EXPLODE,
                &EXPLOSION_BLOCK_PARTICLES[..],
            ),
            ExplosionKind::Wind => (
                particle::GUST_EMITTER_SMALL,
                particle::GUST_EMITTER_LARGE,
                clustine_data::sounds::ENTITY_BREEZE_WIND_BURST,
                &[][..],
            ),
        };
        let mut outgoing = Vec::new();
        for (player, view) in &self.players {
            let Some(entity) = view.entity else {
                continue;
            };
            let Some(shown) = self.entities.get(&entity) else {
                continue;
            };
            let at = shown.state.pose.position;
            let distance = (at.x - centre[0]).powi(2)
                + (at.y - centre[1]).powi(2)
                + (at.z - centre[2]).powi(2);
            if distance >= 4096.0 {
                continue;
            }
            let push = knockback
                .iter()
                .find(|(hit, _)| *hit == entity)
                .map(|(_, push)| *push);
            let packet = Explode {
                centre,
                radius,
                block_count: blocks,
                knockback: push,
                particle: if large {
                    large_particle
                } else {
                    small_particle
                },
                sound: shown_sound,
                block_particles: block_particles.to_vec(),
                play_sound: sound,
            };
            outgoing.push((*player, encoded(&packet)));
        }
        for (player, packet) in outgoing {
            self.send_to_player(player, [packet]).await;
        }
    }

    /// Shows a vibration a sculk sensor took up in `dimension` to every player of this
    /// edge in that dimension whose block's centre is within 32 blocks of where it
    /// starts, whatever region they are in (`ServerLevel.sendParticles`;
    /// `docs/groundwork/n6-brief.md`, section 9.6).
    async fn show_vibration(
        &mut self,
        dimension: Dimension,
        from: [f64; 3],
        to: BlockPos,
        ticks: i32,
    ) {
        use clustine_protocol::packets::play::{LevelParticles, Particle};
        let packet = encoded(&LevelParticles {
            particle: Particle::Vibration {
                to: Position {
                    x: to.x,
                    y: to.y,
                    z: to.z,
                },
                arrival: ticks,
            },
            override_limiter: false,
            always_show: false,
            position: from,
            spread: [0.0; 3],
            max_speed: [0.0; 3],
            count: 1,
            randomization: 0,
        });
        self.show_particles(dimension, from, packet).await;
    }

    /// Sends `packet`, a `level_particles` at `position` in `dimension`, to every player
    /// of this edge in that dimension whose block's centre is within 32 blocks of it
    /// (`ServerLevel.sendParticles` for a player, `closerToCenterThan`), whatever region
    /// they are in.
    async fn show_particles(&mut self, dimension: Dimension, position: [f64; 3], packet: Bytes) {
        let near: Vec<PlayerId> = self
            .players
            .iter()
            .filter(|(_, view)| view.dimension == dimension)
            .filter_map(|(player, view)| {
                let at = self.entities.get(&view.entity?)?.state.pose.position;
                let centre = [at.x.floor() + 0.5, at.y.floor() + 0.5, at.z.floor() + 0.5];
                let distance: f64 = (0..3)
                    .map(|axis| (centre[axis] - position[axis]).powi(2))
                    .sum();
                (distance < 32.0 * 32.0).then_some(*player)
            })
            .collect();
        for player in near {
            self.send_to_player(player, [packet.clone()]).await;
        }
    }

    /// Shows the client of `player` what the region `from` says of their menu
    /// (`crate::menus`). Only a region that stands for the player's own speaks of it: a
    /// region that let them go speaks of a menu that is closed (section 9 item 3).
    async fn menu_event(&mut self, from: RegionId, player: PlayerId, event: PlayerEvent) {
        let living = self.living(from);
        // A horse's menu names the horse, by the id its client knows it by.
        let mount = match &event {
            PlayerEvent::MenuOpened {
                kind: clustine_sim::api::MenuKind::Mount { entity, .. },
                ..
            } => Some(self.wire.of(*entity)),
            _ => None,
        };
        let Some(view) = self.players.get_mut(&player) else {
            return;
        };
        if view.entity.is_none()
            || self
                .stands_for
                .get(&view.region)
                .copied()
                .unwrap_or(view.region)
                != living
        {
            return;
        }
        let packets = match event {
            PlayerEvent::MenuOpened {
                serial,
                applied,
                kind,
                title,
                owner,
                slots,
                carried,
            } => view.menus.opened_with(
                serial,
                applied,
                kind,
                &crate::menus::menu_title(&title, owner),
                slots,
                carried,
                mount,
            ),
            PlayerEvent::MenuChanged {
                serial,
                applied,
                slots,
                carried,
            } => view.menus.changed(serial, applied, &slots, carried),
            PlayerEvent::MenuClosed { serial, applied } => view.menus.closed(serial, applied),
            // Any menu still shown closed; window 0 whole follows from the region.
            PlayerEvent::NoMenu { applied, .. } => match view.menus.none(applied) {
                Some(packets) => packets,
                None => return,
            },
            _ => return,
        };
        self.send_to_player(player, packets).await;
    }

    /// Asks the region of each player in `region` whose client shows a menu for it whole
    /// (`PlayerInput::MenuResync`), and window 0 whole with it: what it said of them may
    /// have been lost. A player who shows none is not asked (named: the V3 review's
    /// finding 9 asks for window 0 always; a report of it lost on a link that ended is
    /// put right by the slot's next change or the next whole).
    async fn resync_menus(&mut self, region: RegionId) {
        let players: Vec<(SessionId, PlayerId)> = self
            .players
            .iter()
            .filter(|(_, view)| {
                view.entity.is_some()
                    && view.menus.serial().is_some()
                    && self.living(view.region) == region
            })
            .map(|(player, view)| (view.session, *player))
            .collect();
        for (session, player) in players {
            self.player_input(session, player, PlayerInput::MenuResync)
                .await;
        }
    }

    /// Tells the client of `player` that the region selected the hotbar slot `slot`
    /// (`ClientboundSetHeldSlotPacket`), once it has entered.
    /// What changed of the player's own effects and the attributes they touch, sent to
    /// their client about their own entity (`ServerPlayer.onEffectAdded`,
    /// `onEffectUpdated`, `onEffectsRemoved`; `docs/groundwork/n9-plan.md`, section 5.4).
    async fn own_effects(&mut self, player: PlayerId, event: PlayerEvent) {
        let Some(entity) = self.players.get(&player).and_then(|view| view.entity) else {
            return;
        };
        let entity_id = self.wire.of(entity);
        let effect_id = |name: &str| {
            clustine_protocol::components::mob_effect::NAMES
                .iter()
                .position(|known| *known == name)
                .and_then(|at| i32::try_from(at).ok())
        };
        let packet = match event {
            PlayerEvent::EffectAdded { instance, blend } => {
                use clustine_protocol::packets::play::effect_flags;
                let Some(effect) = effect_id(&instance.effect) else {
                    return;
                };
                let mut flags = 0;
                for (set, flag) in [
                    (instance.ambient, effect_flags::AMBIENT),
                    (instance.visible, effect_flags::VISIBLE),
                    (instance.show_icon, effect_flags::SHOW_ICON),
                    (blend, effect_flags::BLEND),
                ] {
                    if set {
                        flags |= flag;
                    }
                }
                encoded(&UpdateMobEffect {
                    entity_id,
                    effect,
                    amplifier: instance.amplifier,
                    duration: instance.duration,
                    flags,
                })
            }
            PlayerEvent::EffectRemoved { effect } => {
                let Some(effect) = effect_id(&effect) else {
                    return;
                };
                encoded(&RemoveMobEffect { entity_id, effect })
            }
            PlayerEvent::Attributes(attributes) => encoded(&UpdateAttributes {
                entity_id,
                attributes: attributes.into_iter().map(attribute_snapshot).collect(),
            }),
            _ => return,
        };
        self.send_to_player(player, [packet]).await;
    }

    /// An operator changed the game mode of the edge's `player`: kept for what the edge
    /// refuses of their client, and told as `ServerPlayer.setGameMode` tells it (n5
    /// `ServerPlayerGameMode.changeGameModeForPlayer` @57-84, `setGameMode` @37-84): the
    /// abilities, the list's update to everyone who lists them, the game event, and the
    /// abilities again.
    async fn game_mode_changed(
        &mut self,
        player: PlayerId,
        mode: clustine_world::PlayerMode,
        flying: bool,
    ) {
        let Some(view) = self.players.get_mut(&player) else {
            return;
        };
        view.mode = mode;
        let abilities = encoded(&PlayerAbilities {
            flags: ability_flags(mode.current, flying),
            flying_speed: 0.05,
            field_of_view_modifier: 0.1,
        });
        self.send_to_player(player, [abilities.clone()]).await;
        self.tell_listers(player, mode.current.id()).await;
        let event = encoded(&GameEvent {
            event: game_event::CHANGE_GAME_MODE,
            value: f32::from(mode.current.id()),
        });
        self.send_to_player(player, [event, abilities]).await;
    }

    /// Another edge's `player`, whose entity is `entity`, is now in the game mode of id
    /// `mode`: what this edge shows of the entity says it, and those of its players who
    /// list them are told. The edge's own players' changes come as their own event.
    async fn other_game_mode(&mut self, entity: EntityId, player: PlayerId, mode: u8) {
        if self.players.contains_key(&player) {
            return;
        }
        if let Some(shown) = self.entities.get_mut(&entity)
            && let EntityKind::Player { game_mode, .. } = &mut shown.state.kind
        {
            *game_mode = mode;
        }
        self.tell_listers(player, mode).await;
    }

    /// Tells each of the edge's players who lists `player` with another mode than the
    /// one of id `mode` (`UPDATE_GAME_MODE`).
    async fn tell_listers(&mut self, player: PlayerId, mode: u8) {
        let listers: Vec<PlayerId> = self
            .players
            .iter_mut()
            .filter_map(|(other, view)| {
                let told = view.listed.get_mut(&player)?;
                (*told != mode).then(|| {
                    *told = mode;
                    *other
                })
            })
            .collect();
        let packet = encoded(&game_mode_update(player, mode));
        for other in listers {
            self.send_to_player(other, [packet.clone()]).await;
        }
    }

    async fn held_slot(&mut self, player: PlayerId, slot: u8) {
        let entered = self
            .players
            .get(&player)
            .is_some_and(|view| view.entity.is_some());
        if entered {
            let packet = encoded(&SetHeldSlot { slot: slot.into() });
            self.send_to_player(player, [packet]).await;
        }
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
        // What the player rides as their old region let them go is their word until
        // their new one says otherwise.
        // A transfer that names nothing says nothing of getting off, which the player's
        // region says with `Rides` itself.
        if let Some(riding) = transfer.riding {
            self.take_rides_word(transfer.entity_id, Some(riding.vehicle))
                .await;
        }
        let position = transfer.pose.position;
        let chunk = ChunkPos::containing(position.x, position.z);
        // The stay of this connection's join, let go before the edge was told its
        // entity: by the home region without being placed, as the place it has is in
        // another region's land, or placed and let go before the word of the placing
        // was read (`docs/adr/0020-one-stay-per-player.md`, section 4.3, path 3).
        let entering = self.players.get(&player).is_some_and(|view| {
            view.entity.is_none() && view.region == from && transfer.attempt == Some(view.session.0)
        });
        if entering && (to != from || back) {
            self.enter_by_arrival(player, to, transfer).await;
            return;
        }
        // The player's connection can have ended while the message was on its way, and
        // they can even be back already, as a new entity somewhere else.
        let current = self
            .players
            .get_mut(&player)
            .filter(|view| view.entity == Some(transfer.entity_id));
        let Some(view) = current else {
            self.discard(to, player, &transfer, chunk).await;
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
            self.discard(to, player, &transfer, chunk).await;
            self.remove_player(player).await;
            return;
        }

        // The mode goes with the stay, as the region had it when it let it go.
        view.mode = transfer.mode;
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
        // A hand-over always closes the menu, and the close the old region said may
        // have been lost on a link that resumed (section 7 item 7).
        let closed = view.menus.departed();
        // A transfer of another dimension than the client shows is a change of
        // dimension: what triggers it is the difference, so that a departure turned
        // back from a `NotMine` in the same dimension is none (R D10).
        // A respawn's departure (after a death or the credits, or out of the End by its
        // portal) owes the client its `Respawn`, which goes with the placing, whatever
        // the dimension (`docs/groundwork/p-plan.md`, section 19.7).
        let respawn_way = transfer.travel.is_some_and(|travel| match travel.way {
            clustine_world::Way::Respawn | clustine_world::Way::Credits => true,
            clustine_world::Way::EndPortal => {
                view.dimension == Dimension::End && transfer.dimension != Dimension::End
            }
            _ => false,
        });
        if respawn_way {
            view.respawn_owed.get_or_insert_with(Vec::new);
        }
        let respawning = view.respawn_owed.is_some();
        let changes_level = transfer.dimension != view.dimension || respawning;
        let level = transfer.dimension;
        let through_a_portal = transfer.travel.is_some();
        let again: Vec<_> = view
            .kept_inputs
            .iter()
            .filter(|(_, _, input)| !changes_level || has_no_place(input))
            .map(|(number, _, input)| (*number, input.clone()))
            .collect();
        debug!(name = %view.name, %from, %to, inputs = again.len(), changes_level, "handing a player over");

        // What the player sees is asked of their region from now on. On the link to
        // `to` this is before the arrival, so that one claim of that region covers the
        // chunk the player arrives in and their view. A player who arrives where the
        // word of their leaving comes from is asked for there already: the region
        // that let them go has gone into it, or it into that region. One who changes
        // dimension lets the old level go whole, and is shown nothing of the new one
        // before the destination has placed them (`docs/groundwork/p-plan.md`, section
        // 4.2, steps 1 to 5).
        if changes_level {
            self.leave_level(player, from).await;
            if let Some(view) = self.players.get_mut(&player) {
                view.dimension = transfer.dimension;
                view.told_clocks = None;
                view.travelling = Some(Travelling {
                    since: Instant::now(),
                    way: transfer.travel.map(|travel| travel.way),
                    look: (transfer.pose.yaw, transfer.pose.pitch),
                });
                view.awaiting_teleport.store(TRAVELLING, Ordering::Relaxed);
            }
            // The new view is asked of `to`; what the client is to be sent of it waits
            // for the placing.
            let _ = self.shift_view(player, chunk).await;
            self.flush_asking().await;
        } else if to != from {
            // A gateway's traveller, within one level: its moves are held until it is
            // placed, and its look is told then as the turn from the one it left with
            // (`docs/groundwork/p-plan.md`, section 19.8, the review's sixth point).
            if let Some(travel) = transfer.travel
                && matches!(travel.way, clustine_world::Way::Gateway { .. })
                && let Some(view) = self.players.get_mut(&player)
            {
                view.travelling = Some(Travelling {
                    since: Instant::now(),
                    way: Some(travel.way),
                    look: (transfer.pose.yaw, transfer.pose.pitch),
                });
                view.awaiting_teleport.store(TRAVELLING, Ordering::Relaxed);
            }
            let Some(view) = self.players.get(&player) else {
                return;
            };
            let seen: Vec<ChunkPos> = view.wanted.iter().copied().collect();
            for chunk in &seen {
                self.unwant(from, *chunk);
            }
            for chunk in &seen {
                self.want(to, *chunk);
            }
            self.flush_asking().await;
        }

        self.send_to_player(player, closed).await;
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
        if changes_level && !respawning {
            // Step 7: only now the client, after the region has been told. It drops its
            // level and its entities, keeping its own entity's data (`Respawn`, keep 3).
            // A portal sets the traveller's cooldown before the `Respawn` is made
            // (`Entity.setPortalCooldown`, a player's `getDimensionChangingDelay` of 10).
            let (last_death, mode) = self
                .players
                .get(&player)
                .map_or((None, Default::default()), |view| {
                    (view.last_death, view.mode)
                });
            let mut spawn = spawn_info(&self.config, level, last_death, mode);
            if through_a_portal {
                spawn.portal_cooldown = PLAYER_PORTAL_COOLDOWN;
            }
            let respawn = [
                encoded(&Respawn {
                    spawn,
                    data_to_keep: Respawn::KEEP_ALL_DATA,
                }),
                encoded(&ChangeDifficulty {
                    difficulty: clustine_world::Difficulty::WORLD.id() as u8,
                    locked: false,
                }),
            ];
            self.send_to_player(player, respawn).await;
            return;
        }
        // Normally the view has followed the move already; this covers a player who was
        // let go without having been seen to move, such as one who joined right there.
        self.move_view(player, chunk).await;
    }

    /// Keeps what a region tells a player who is owed a respawn, to be told after it
    /// (section 19.7), and passes everything else on. A join's entering and a kick are
    /// never held.
    fn held_while_respawning(&mut self, message: WorkerToEdge) -> Option<WorkerToEdge> {
        let WorkerToEdge::ToPlayer { player, event } = message else {
            return Some(message);
        };
        if matches!(
            event,
            PlayerEvent::Spawned { .. } | PlayerEvent::Kicked { .. }
        ) {
            return Some(WorkerToEdge::ToPlayer { player, event });
        }
        match self
            .players
            .get_mut(&player)
            .and_then(|view| view.respawn_owed.as_mut())
        {
            Some(held) => {
                held.push(event);
                None
            }
            None => Some(WorkerToEdge::ToPlayer { player, event }),
        }
    }

    /// From now on the player is owed a respawn (section 19.7).
    fn owe_respawn(&mut self, player: PlayerId) {
        if let Some(view) = self.players.get_mut(&player) {
            view.respawn_owed.get_or_insert_with(Vec::new);
        }
    }

    /// Shows the player whose stay is `entity` their death: the death message, the game's
    /// sentence `message` with their name, and the respawn screen it opens
    /// (`PlayerCombatKill`; `docs/groundwork/p-plan.md`, sections 19.5 and 19.6).
    async fn died(
        &mut self,
        player: PlayerId,
        entity: EntityId,
        message: &clustine_sim::api::DeathMessage,
    ) {
        let Some(view) = self.players.get(&player) else {
            return;
        };
        let name = view.name.clone();
        let wire = self.wire.of(entity);
        let packet = encoded(&PlayerCombatKill {
            player_id: wire,
            // In the order the official server writes them.
            message: Nbt::Compound(vec![
                (
                    "with".to_owned(),
                    Nbt::List(
                        std::iter::once(display_name(&name, player))
                            .chain(message.killer.as_ref().map(killer_name))
                            .collect(),
                    ),
                ),
                ("translate".to_owned(), Nbt::String(message.key.clone())),
            ]),
        });
        // `ServerPlayer.die` sends the death, and then `broadcastEntityEvent` the death's
        // event 3, which the player is shown after it: the region's event 3 comes before
        // its outbox, so the edge shows it to the dying player here
        // (`show_entity_event`).
        let fell = encoded(&EntityEvent {
            entity_id: wire,
            event: 3,
        });
        self.send_to_player(player, [packet, fell]).await;
    }

    /// The second half of a change of dimension, or a travel within one (a gateway): the
    /// destination has placed the player at `pose`. The client is sent, in the order
    /// the game sends them (`docs/groundwork/p-brief.md`, section 3.1, items 8 to 17):
    /// where they are, as a teleport it must accept, which lets its moves through
    /// again; what they may do; the level's information, ending with the wait for its
    /// chunks; the portal's sound. Then its view and its chunks. The inventory is the
    /// destination's, asked of it whole.
    async fn placed(
        &mut self,
        player: PlayerId,
        dimension: Dimension,
        pose: Pose,
        flying: bool,
        respawn: Option<Respawned>,
        gateway: bool,
    ) {
        if let Some(respawn) = respawn {
            Box::pin(self.respawned(player, dimension, pose, flying, respawn)).await;
            return;
        }
        let Some(view) = self.players.get_mut(&player) else {
            return;
        };
        if view.dimension != dimension {
            // A placing in another dimension than the client was last sent to: the
            // edge did not see the departure that went there, which a transfer of
            // another dimension always is.
            error!(name = %view.name, ?dimension, now = ?view.dimension, "a player was placed in another dimension than their client shows");
            return;
        }
        let travelling = view.travelling.take();
        view.teleports = view.teleports.wrapping_add(1).max(1);
        let teleport_id = view.teleports;
        view.awaiting_teleport.store(teleport_id, Ordering::Relaxed);
        let flags = ability_flags(view.mode.current, flying);
        // A travel's look is told as the turn from the look the stay departed with, so
        // that the client keeps its own look turned as the portal turns it, and its
        // velocity turned with it (`Relative.DELTA` with `ROTATE_DELTA`, and
        // `Relative.ROTATION`, as a portal's `TeleportTransition` has them).
        // The End's portal faces the traveller west whatever they looked at, and keeps
        // their pitch (`DELTA ∪ {X_ROT}`; `docs/groundwork/p-brief.md`, section 2.4).
        let (yaw, pitch, rotation) = match &travelling {
            Some(Travelling {
                way: Some(clustine_world::Way::EndPortal),
                look: (_, pitch),
                ..
            }) => (
                pose.yaw,
                pose.pitch - pitch,
                relative::X_ROT | relative::ROTATE_DELTA,
            ),
            Some(travelling) => {
                let (yaw, pitch) = travelling.look;
                (
                    wrap_degrees(pose.yaw - yaw),
                    pose.pitch - pitch,
                    relative::ROTATION | relative::ROTATE_DELTA,
                )
            }
            // A gateway's traveller placed by the region they are in, with no hand-over
            // to tell their look by: the gateway keeps the client's own, a turn of
            // nought (`EndGatewayBlock.getPortalDestination`).
            None if gateway => (0.0, 0.0, relative::ROTATION | relative::ROTATE_DELTA),
            None => (pose.yaw, pose.pitch, 0),
        };
        let placing = [
            // The game keeps the velocity the client has (`Relative.DELTA`).
            encoded(&SynchronizePlayerPosition {
                teleport_id,
                x: pose.position.x,
                y: pose.position.y,
                z: pose.position.z,
                velocity_x: 0.0,
                velocity_y: 0.0,
                velocity_z: 0.0,
                yaw,
                pitch,
                relative_flags: relative::DELTA_X
                    | relative::DELTA_Y
                    | relative::DELTA_Z
                    | rotation,
            }),
            encoded(&PlayerAbilities {
                flags,
                flying_speed: 0.05,
                field_of_view_modifier: 0.1,
            }),
        ];
        let region = view.region;
        if !self.send_to_player(player, placing).await {
            return;
        }
        let mut packets = Vec::new();
        // A gateway changes no level: no level's information and no wait for chunks.
        let gateway = travelling.as_ref().is_some_and(|travelling| {
            matches!(travelling.way, Some(clustine_world::Way::Gateway { .. }))
        });
        if travelling.is_some() && !gateway {
            // The level's information (`PlayerList.sendLevelInfo`): its clocks, where a
            // compass points, and the wait for its chunks.
            self.tell_time(player, region, true).await;
            packets.extend([
                encoded(&SetDefaultSpawnPosition {
                    dimension: OVERWORLD.to_owned(),
                    position: Position {
                        x: self.world_spawn.x,
                        y: self.world_spawn.y,
                        z: self.world_spawn.z,
                    },
                    yaw: 0.0,
                    pitch: 0.0,
                }),
                encoded(&GameEvent {
                    event: game_event::START_WAITING_FOR_LEVEL_CHUNKS,
                    value: 0.0,
                }),
            ]);
        }
        // The post-transition's portal sound: a Nether portal's, and the End's into the
        // End (`TeleportTransition.PLAY_PORTAL_SOUND`).
        let portal = travelling
            .and_then(|travelling| travelling.way)
            .is_some_and(|way| match way {
                clustine_world::Way::NetherPortal { .. } => true,
                clustine_world::Way::EndPortal => dimension == Dimension::End,
                _ => false,
            });
        if portal {
            packets.push(encoded(&LevelEvent {
                event: level_event::PORTAL_TRAVEL,
                position: Position { x: 0, y: 0, z: 0 },
                data: 0,
                global: false,
            }));
        }
        if !self.send_to_player(player, packets).await {
            return;
        }
        // Its inventory as the destination has it: the answer to a resync is the
        // inventory whole (`PlayerEvent::NoMenu`).
        self.player_input_unchecked(player, PlayerInput::MenuResync)
            .await;
        let center = ChunkPos::containing(pose.position.x, pose.position.z);
        self.show_level(player, center).await;
    }

    /// The placing of a respawn (`docs/groundwork/p-plan.md`, section 19.7;
    /// `PlayerList.respawn`): the client is sent its abilities, the missing block's
    /// event where one is owed, `Respawn` into the level it is placed in with the keep,
    /// and the difficulty; where it is, absolutely, still; the level's information
    /// ending with the wait for its chunks; the anchor's sound where a charge was
    /// spent; then what its regions told it meanwhile, its inventory, its view and its
    /// chunks. A respawn placed by the region the player died in comes with no
    /// hand-over, so the old level is let go here.
    async fn respawned(
        &mut self,
        player: PlayerId,
        dimension: Dimension,
        pose: Pose,
        flying: bool,
        respawn: Respawned,
    ) {
        let Some(view) = self.players.get_mut(&player) else {
            return;
        };
        let region = view.region;
        if view.travelling.take().is_none() {
            self.leave_level(player, region).await;
        }
        let Some(view) = self.players.get_mut(&player) else {
            return;
        };
        view.dimension = dimension;
        view.told_clocks = None;
        // After a death (no data kept) the client's player is new, and so is its
        // window 0 (the V3 review's finding 4).
        if respawn.keep == 0 {
            view.menus.respawned_after_death();
        }
        let held = view.respawn_owed.take().unwrap_or_default();
        let last_death = view.last_death;
        let mode = view.mode;
        view.teleports = view.teleports.wrapping_add(1).max(1);
        let teleport_id = view.teleports;
        view.awaiting_teleport.store(teleport_id, Ordering::Relaxed);
        let flags = ability_flags(view.mode.current, flying);
        let mut packets = vec![encoded(&PlayerAbilities {
            flags,
            flying_speed: 0.05,
            field_of_view_modifier: 0.1,
        })];
        if respawn.no_respawn_block {
            packets.push(encoded(&GameEvent {
                event: game_event::NO_RESPAWN_BLOCK_AVAILABLE,
                value: 0.0,
            }));
        }
        // `PlayerList.respawn`'s order: the `Respawn`, the place (993), the default
        // spawn (1001), the difficulty (1011), then the level's information.
        packets.extend([
            encoded(&Respawn {
                spawn: spawn_info(&self.config, dimension, last_death, mode),
                data_to_keep: respawn.keep,
            }),
            // `ServerPlayer.teleport(DDDFF)`: absolute, and still.
            encoded(&SynchronizePlayerPosition {
                teleport_id,
                x: pose.position.x,
                y: pose.position.y,
                z: pose.position.z,
                velocity_x: 0.0,
                velocity_y: 0.0,
                velocity_z: 0.0,
                yaw: pose.yaw,
                pitch: pose.pitch,
                relative_flags: 0,
            }),
            encoded(&SetDefaultSpawnPosition {
                dimension: OVERWORLD.to_owned(),
                position: Position {
                    x: self.world_spawn.x,
                    y: self.world_spawn.y,
                    z: self.world_spawn.z,
                },
                yaw: 0.0,
                pitch: 0.0,
            }),
            encoded(&ChangeDifficulty {
                difficulty: clustine_world::Difficulty::WORLD.id() as u8,
                locked: false,
            }),
        ]);
        if !self.send_to_player(player, packets).await {
            return;
        }
        self.tell_time(player, region, true).await;
        let mut packets = vec![encoded(&GameEvent {
            event: game_event::START_WAITING_FOR_LEVEL_CHUNKS,
            value: 0.0,
        })];
        if let Some(anchor) = respawn.anchor_sound {
            // At the block's corner, to the player alone, of the blocks' source.
            self.sound_seeds = self.sound_seeds.wrapping_add(1);
            packets.push(encoded(&Sound {
                sound: clustine_data::sounds::BLOCK_RESPAWN_ANCHOR_DEPLETE,
                source: clustine_protocol::packets::play::SOUND_SOURCE_BLOCKS,
                x: anchor.x * 8,
                y: anchor.y * 8,
                z: anchor.z * 8,
                volume: 1.0,
                pitch: 1.0,
                seed: self.sound_seeds as i64,
            }));
        }
        if !self.send_to_player(player, packets).await {
            return;
        }
        for event in held {
            let message = WorkerToEdge::ToPlayer { player, event };
            Box::pin(self.handle_region(region, message)).await;
        }
        self.player_input_unchecked(player, PlayerInput::MenuResync)
            .await;
        let center = ChunkPos::containing(pose.position.x, pose.position.z);
        self.show_level(player, center).await;
    }

    /// Shows a client the level it was placed in: its view centred on `center`, the
    /// entities in it and the chunks, as a join's view is.
    async fn show_level(&mut self, player: PlayerId, center: ChunkPos) {
        let Some(view) = self.players.get(&player) else {
            return;
        };
        if view.center != center || view.wanted.is_empty() {
            self.move_view(player, center).await;
        }
        let Some(view) = self.players.get_mut(&player) else {
            return;
        };
        let mut packets = vec![encoded(&SetCenterChunk {
            chunk_x: view.center.x,
            chunk_z: view.center.z,
        })];
        for shown in self.entities.values() {
            packets.extend(view.update_visibility(player, shown, false, &mut self.wire));
        }
        for (vehicle, riders) in &effective_passengers(&self.passengers, &self.rides_word) {
            let shows = |key: &EntityId| view.visible.contains_key(key);
            if (shows(vehicle) || riders.iter().any(shows))
                && let Some(packet) = passengers_packet(view, *vehicle, riders, &mut self.wire)
            {
                packets.push(packet);
            }
        }
        if self.send_to_player(player, packets).await {
            self.send_chunks(player).await;
        }
    }

    /// Lets the level the client shows go whole, as at a change of dimension: the
    /// client drops its chunks and its entities with the `Respawn` that follows, so
    /// nothing is unloaded or removed one by one. The view's chunks are counted out of
    /// the replica and let go at `from`, the region the player was of.
    async fn leave_level(&mut self, player: PlayerId, from: RegionId) {
        let Some(view) = self.players.get_mut(&player) else {
            return;
        };
        let dimension = view.dimension;
        let wanted = std::mem::take(&mut view.wanted);
        let pending = std::mem::take(&mut view.pending);
        view.sent.clear();
        view.replace.clear();
        view.batch_outstanding = false;
        let visible: Vec<EntityId> = std::mem::take(&mut view.visible).into_keys().collect();
        for position in &pending {
            if let Some(chunk) = self.replica.get_mut(&LevelChunk::new(dimension, *position)) {
                chunk.waits_no_more(player);
            }
        }
        for position in &wanted {
            let at = LevelChunk::new(dimension, *position);
            let chunk = self
                .replica
                .get_mut(&at)
                .expect("viewed chunks are in the replica");
            chunk.viewers -= 1;
            if chunk.viewers == 0 {
                self.queues.take_out(at, chunk);
                self.replica.remove(&at);
                self.marked.remove(&at);
            }
        }
        for position in &pending {
            let at = LevelChunk::new(dimension, *position);
            if self.replica.contains_key(&at) {
                self.requeue(at);
            }
        }
        for position in &wanted {
            self.unwant(from, *position);
        }
        for entity in visible {
            self.release_wire(entity);
        }
        self.forget_unwatched_entities();
        self.flush_asking().await;
    }

    /// Gives up an entity that a region let go and that has nowhere to go: its player
    /// left meanwhile, or cannot be placed.
    ///
    /// The old region did not report the entity as removed, because it lived on. The
    /// region it was heading for, `to`, is told to report it, for every edge whose
    /// players saw it cross. This edge takes it off its own screens here and now: it
    /// takes a removal only from the region that showed it the entity last, which is
    /// the old one, and it may not be asking `to` for that chunk at all.
    async fn discard(
        &mut self,
        to: RegionId,
        player: PlayerId,
        transfer: &PlayerTransfer,
        chunk: ChunkPos,
    ) {
        let entity = transfer.entity_id;
        let discard = EdgeToWorker::Discard {
            player,
            entity,
            hops: transfer.hops,
            chunk,
        };
        self.send_to_region(to, discard).await;
        self.remove_entity(entity).await;
    }

    /// Handles something that happened in the region `from`.
    async fn handle_event(&mut self, from: RegionId, event: RegionEvent) {
        // What the region says is of its dimension.
        let dimension = self.dimension_of(from);
        match event {
            RegionEvent::PlayerGameMode {
                entity,
                player,
                mode,
                ..
            } => self.other_game_mode(entity, player, mode).await,
            RegionEvent::EntitySpawned(state) => self.upsert_entity(from, state, true).await,
            // An entity that came into a chunk the edge watches from one it was shown at
            // another tick, or not at all: shown as from a snapshot. One the edge shows
            // already is moved by its own report, as the game moves it.
            RegionEvent::EntityEntered(state) => {
                if !self.entities.contains_key(&state.entity) {
                    self.upsert_entity(from, state, false).await;
                }
            }
            RegionEvent::EntityRemoved { entity, .. } => {
                if self.is_of(entity, from) {
                    self.remove_entity(entity).await;
                }
            }
            RegionEvent::BlockChanged {
                position,
                state,
                seen,
            } => {
                let dimension = self.dimension_of(from);
                if self.change_block(dimension, position, state) && seen {
                    let chunk = LevelChunk::new(dimension, position.chunk());
                    let section = (chunk, position.y.div_euclid(16));
                    let changed = Changed::from([(section, BTreeMap::from([(position, state)]))]);
                    self.show_changes(changed).await;
                }
            }
            RegionEvent::BlockEntityChanged {
                position,
                data,
                seen,
            } => {
                let dimension = self.dimension_of(from);
                self.change_block_entity(dimension, position, data);
                if seen {
                    self.show_block_entity(dimension, position).await;
                }
            }
            RegionEvent::BlockEvent {
                position,
                block,
                kind,
                data,
            } => {
                let packet = encoded(&BlockEvent {
                    position: Position {
                        x: position.x,
                        y: position.y,
                        z: position.z,
                    },
                    // The game writes both as bytes.
                    action: kind as u8,
                    param: data as u8,
                    block: i32::from(block),
                });
                let at = block_centre(position);
                let chunk = LevelChunk::new(dimension, position.chunk());
                self.send_near(chunk, at, BLOCK_EVENTS_WITHIN, None, packet)
                    .await;
            }
            RegionEvent::Sound {
                sound,
                source,
                position,
                volume,
                pitch,
                except,
            } => {
                self.sound_seeds = self.sound_seeds.wrapping_add(0x9E37_79B9_7F4A_7C15);
                let packet = encoded(&Sound {
                    sound,
                    source,
                    x: (position.x * 8.0) as i32,
                    y: (position.y * 8.0) as i32,
                    z: (position.z * 8.0) as i32,
                    volume,
                    pitch,
                    seed: self.sound_seeds as i64,
                });
                // `ServerLevel.playSeededSound`: within 16 blocks, or 16 times the volume.
                let range = 16.0 * f64::from(volume.max(1.0));
                let at = (position.x, position.y, position.z);
                let chunk =
                    LevelChunk::new(dimension, ChunkPos::containing(position.x, position.z));
                self.send_near(chunk, at, range, except, packet).await;
            }
            RegionEvent::LevelEvent {
                kind,
                position,
                data,
            } => {
                self.show_near(dimension, position, kind, data, None).await;
            }
            // A region's changes after it are taken where its tick's are.
            RegionEvent::BlockEventsBegin => {}
            // The worker unwraps it; should one come, it is its event.
            RegionEvent::Near { event, .. } => Box::pin(self.handle_event(from, *event)).await,
            RegionEvent::BlockDestroyed {
                position,
                state,
                breaker,
            } => {
                let data = state.0.into();
                self.show_near(
                    dimension,
                    position,
                    level_event::BLOCK_BROKEN,
                    data,
                    breaker,
                )
                .await;
            }
            RegionEvent::Fizzed { position } => {
                self.show_near(dimension, position, level_event::LAVA_FIZZ, 0, None)
                    .await;
            }
            RegionEvent::BlockDestruction {
                breaker,
                position,
                stage,
            } => {
                self.show_destruction(dimension, breaker, position, stage)
                    .await;
            }
            RegionEvent::ArmSwung {
                entity,
                off_hand,
                swing,
                own,
                ..
            } => self.swing_arm(entity, off_hand, swing, own).await,
            RegionEvent::EntityTracked(tracked) => self.track_entity(from, tracked).await,
            RegionEvent::EntityGone { entity, hops, .. } => {
                if self.was_gone(entity, hops) || !self.takes_word_of(from, entity, hops) {
                    return;
                }
                self.remove_entity(entity).await;
                self.gone.insert(
                    entity,
                    Gone {
                        hops,
                        heard: BTreeMap::new(),
                    },
                );
            }
            RegionEvent::ItemTaken {
                item,
                collector,
                count,
                ..
            } => self.show_taken(from, item, collector, count).await,
            // Shown as the plan of N3 says from step N3e on.
            RegionEvent::HandedOn {
                entity,
                hops,
                position,
                motion,
                ..
            } => self.hand_on(from, entity, hops, position, motion).await,
            // Whom a vehicle carries counts from the region whose word of the vehicle
            // counts, as its moves do: across a line the region it went to says it
            // with one change of holder more (`docs/groundwork/n5-plan.md`, section 8
            // item 4).
            RegionEvent::Passengers {
                vehicle,
                hops,
                passengers,
                ..
            } => {
                if self.takes_word_of(from, vehicle, hops) {
                    self.show_passengers(vehicle, passengers).await;
                }
            }
            RegionEvent::EntityEvent { entity, event, .. } => {
                if self.is_of(entity, from) {
                    self.show_entity_event(entity, event).await;
                }
            }
            RegionEvent::Leashed { entity, holder, .. } => self.show_leash(entity, holder).await,
            // Shown from the steps of N5 that make them.
            RegionEvent::DataChanged { .. } | RegionEvent::EntitySound { .. } => {}
            // A mob's living packets (`docs/groundwork/n7-brief.md`, section 9.1), from
            // the region whose word of it counts.
            RegionEvent::DamageEvent {
                entity,
                damage_type,
                cause,
                direct,
                position,
                ..
            } => {
                if self.is_of(entity, from) {
                    let packet = DamageEvent {
                        entity_id: self.wire.of(entity),
                        damage_type,
                        cause: cause.map(|cause| self.wire.of(cause)),
                        direct: direct.map(|direct| self.wire.of(direct)),
                        position,
                    };
                    let packet = encoded(&packet);
                    self.show_to_watchers(entity, packet.clone()).await;
                    // `broadcastDamageEvent` goes to the hurt player too, whose client
                    // plays the hurt and shakes its camera from it.
                    let own = self
                        .players
                        .iter()
                        .find(|(_, view)| view.entity == Some(entity))
                        .map(|(player, _)| *player);
                    if let Some(player) = own {
                        self.send_to_player(player, [packet]).await;
                    }
                }
            }
            // A player lying down or getting up, shown to those who see them and to
            // themselves, whose client takes its in-bed screen from it.
            RegionEvent::Sleeping { entity, head, .. } => {
                if self.is_of(entity, from) {
                    self.show_sleeping(entity, head).await;
                }
            }
            RegionEvent::PlayerData { entity, values, .. } => {
                if self.is_of(entity, from) {
                    self.show_player_data(entity, values).await;
                }
            }
            RegionEvent::AttributesChanged {
                entity, attributes, ..
            } => {
                if self.is_of(entity, from) {
                    self.show_attributes(entity, attributes).await;
                }
            }
            RegionEvent::EquipmentChanged { entity, slots, .. } => {
                if self.is_of(entity, from) {
                    self.show_equipment(entity, slots).await;
                }
            }
            // A critical hit's sparks on the target, seen by those who see the attacker
            // and by the attacker (`Player.crit`, `ServerChunkCache.sendToTrackingPlayersAndSelf`).
            RegionEvent::Animated {
                target,
                attacker,
                action,
                ..
            } => {
                let packet = encoded(&Animate {
                    entity_id: self.wire.of(target),
                    action,
                });
                self.show_to_watchers(attacker, packet.clone()).await;
                let own = self
                    .players
                    .iter()
                    .find(|(_, view)| view.entity == Some(attacker))
                    .map(|(player, _)| *player);
                if let Some(player) = own {
                    self.send_to_player(player, [packet]).await;
                }
            }
            RegionEvent::Explosion {
                centre,
                radius,
                blocks,
                knockback,
                large,
                sound,
                kind,
            } => {
                self.show_explosion(centre, radius, blocks, &knockback, (large, kind), sound)
                    .await;
            }
            RegionEvent::TransientBlock { position, state } => {
                self.show_transient(dimension, position, state).await;
            }
            RegionEvent::Vibration { from, to, ticks } => {
                self.show_vibration(dimension, from, to, ticks).await;
            }
            RegionEvent::Particles {
                particle,
                position,
                spread,
                speed,
                count,
                alternative,
            } => {
                use clustine_protocol::packets::play::{LevelParticles, Particle};
                let packet = encoded(&LevelParticles {
                    particle: Particle::Simple { id: particle },
                    override_limiter: false,
                    always_show: false,
                    position,
                    spread,
                    max_speed: [speed; 3],
                    count,
                    randomization: i32::from(alternative),
                });
                self.show_particles(dimension, position, packet).await;
            }
            RegionEvent::EntityMoved { entity, pose, .. } => {
                if !self.is_of(entity, from) {
                    return;
                }
                // A move of an entity the edge has not been told about yet is covered by
                // the snapshot of the chunk it is in, which is still on its way.
                if let Some(shown) = self.entities.get_mut(&entity) {
                    shown.state.pose = pose;
                    // An entity other than a player is moved on the wire by its tracker.
                    let tracked = match (&mut shown.tracker, &shown.state.kind) {
                        (Some(tracker), EntityKind::Other { kind, motion, .. }) => {
                            tracker.moved(Seen {
                                kind: *kind,
                                position: [pose.position.x, pose.position.y, pose.position.z],
                                motion: *motion,
                                yaw: pose.yaw,
                                pitch: pose.pitch,
                                on_ground: pose.on_ground,
                            });
                            true
                        }
                        _ => false,
                    };
                    self.refresh_entity(entity, !tracked).await;
                }
                // The view of a player follows where the worker says the player is.
                if let Some(player) = self.entity_owners.get(&entity).copied() {
                    let center = ChunkPos::containing(pose.position.x, pose.position.z);
                    self.move_view(player, center).await;
                }
            }
        }
    }

    /// Passes an input of the player on to the region they are in, numbered, and keeps it
    /// until the region has applied it.
    async fn player_input(&mut self, session: SessionId, player: PlayerId, input: PlayerInput) {
        if !self.session_matches(player, session) {
            return;
        }
        // What a travelling client does that has to do with where it is, is of the
        // level it has left: dropped as its moves are (`docs/groundwork/p-plan.md`,
        // section 4.2, "Inputs").
        let travelling = self
            .players
            .get(&player)
            .is_some_and(|view| view.travelling.is_some());
        if travelling && !has_no_place(&input) {
            return;
        }
        let Some(input) = self
            .players
            .get(&player)
            .and_then(|view| allowed(view.mode.current, input))
        else {
            return;
        };
        // A creative slot is what the client shows of window 0 (`setRemoteSlot`), so
        // that the region's report of it sends nothing back.
        let shown = match &input {
            PlayerInput::SetInventorySlot { slot, stack } => {
                Inventory::to_container(usize::from(*slot)).map(|at| (at, stack))
            }
            // The grid's slots are window 0's 1 to 4.
            PlayerInput::SetCraftingSlot { slot, stack } => Some((i16::from(*slot) + 1, stack)),
            _ => None,
        };
        if let Some((at, stack)) = shown
            && let Some(view) = self.players.get_mut(&player)
        {
            let at = usize::try_from(at).expect("window 0's slots are not negative");
            view.menus.creative_slot(at, stack.clone());
        }
        self.player_input_unchecked(player, input).await;
    }

    /// An operator's order for the player called `name` (the game's `getPlayerByName`,
    /// regardless of case): passed into their inputs if this edge holds them, past the
    /// refusals of what a client may send (no client sent it).
    async fn take_order(&mut self, name: &str, order: clustine_sim::api::Order) {
        let found = self
            .players
            .iter()
            .find(|(_, view)| view.name.eq_ignore_ascii_case(name))
            .map(|(id, _)| *id);
        let Some(player) = found else {
            debug!(%name, "an operator's order for a player this edge does not hold");
            return;
        };
        info!(%name, ?order, "passing on an operator's order");
        self.player_input_unchecked(player, PlayerInput::Order(order))
            .await;
    }

    /// Passes `input` on to the player's region as theirs, numbered and kept as every
    /// input is: what the client sent, or what the edge asks for them.
    async fn player_input_unchecked(&mut self, player: PlayerId, input: PlayerInput) {
        let Some(view) = self.players.get_mut(&player) else {
            return;
        };
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

    /// Whether what the region `from` says of `entity` counts: the entity is one that
    /// region introduced last, or one the edge does not show at all.
    fn is_of(&self, entity: EntityId, from: RegionId) -> bool {
        self.entities
            .get(&entity)
            .is_none_or(|shown| shown.from == from)
    }

    /// Takes in an entity the region `from` described, which may be new or already
    /// known. The entity is that region's from now on. `spawned` where the region
    /// reported it as come into existence: made just now unless it came over a line.
    async fn upsert_entity(&mut self, from: RegionId, state: EntityState, spawned: bool) {
        let dimension = self.dimension_of(from);
        if !self
            .replica
            .contains_key(&LevelChunk::new(dimension, state.chunk()))
        {
            // Nobody watches where it is; this can be a snapshot that arrives late. An
            // entity shown here that its new holder took in out there has left the
            // viewers' sight, though, and is taken off them as a move out would be.
            if let EntityKind::Other { hops, .. } = &state.kind
                && self.entities.contains_key(&state.entity)
                && !self.was_gone(state.entity, *hops)
                && self.takes_word_of(from, state.entity, *hops)
                && let Some(shown) = self.entities.get_mut(&state.entity)
            {
                let entity = state.entity;
                shown.state = state;
                shown.from = from;
                shown.dimension = dimension;
                shown.handed_on = None;
                self.refresh_entity(entity, true).await;
            }
            return;
        }
        let entity = state.entity;
        let pose = state.pose;
        // Of one player only the latest stay the edge has been told of is shown. What
        // two regions say of an earlier and a later stay reaches the edge in no order,
        // and a client must never be shown two entities of one player
        // (`docs/adr/0020-one-stay-per-player.md`, section 6, rule 6). Any other
        // entity is shown by its key from the region whose word has the most changes
        // of holder behind it (`docs/adr/0028-entities.md`, section 6).
        let player = match &state.kind {
            EntityKind::Player { player, .. } => *player,
            EntityKind::Other {
                hops,
                kind,
                motion,
                data,
                lived,
                ..
            } => {
                if self.was_gone(entity, *hops) || !self.takes_word_of(from, entity, *hops) {
                    return;
                }
                // More changes of holder than its removal had: not what was removed.
                self.gone.remove(&entity);
                let seen = Seen {
                    kind: *kind,
                    position: [pose.position.x, pose.position.y, pose.position.z],
                    motion: *motion,
                    yaw: pose.yaw,
                    pitch: pose.pitch,
                    on_ground: pose.on_ground,
                };
                let start = self.tracker_start(from);
                let known = self.entities.get(&entity);
                let taken_over =
                    known.is_some_and(|known| self.living(known.from) != self.living(from));
                let tracker = match known.and_then(|known| known.tracker.clone()) {
                    Some(mut tracker) => {
                        // The tick it is updated next is the new holder's next.
                        if taken_over {
                            tracker.ran = start;
                        }
                        tracker.moved(seen);
                        tracker
                    }
                    None => {
                        // The game numbers a dragon's eight parts after its own id
                        // (`EnderDragon.setId`, `recreateFromPacket`): they are given
                        // with it the first time it is met.
                        if i32::from(*kind) == entity_types::ENDER_DRAGON {
                            self.wire.run(entity, DRAGON_PARTS);
                        }
                        let mut tracker =
                            Tracker::new(seen, data, start, &mut |key| self.wire.of(key));
                        // A cloud met after it was made is as old as it has lived.
                        tracker.lived(*lived, data);
                        if spawned && *hops == 0 {
                            tracker.made();
                        }
                        tracker
                    }
                };
                let riding = match &state.kind {
                    EntityKind::Other { riding, .. } => riding.clone(),
                    EntityKind::Player { .. } => clustine_world::Riding::default(),
                };
                // What it was shown with goes on with it, wherever it was reported from;
                // one the edge meets for the first time (made now, loaded after a
                // restart, a copy beyond a line) is shown wearing what its record says it
                // wears, which no event of a change will tell (`sendPairingData`).
                let living = match self.entities.get(&entity) {
                    Some(known) => known.living.clone(),
                    None => LivingShown {
                        equipment: clustine_sim::shown::mob_equipment(data)
                            .into_iter()
                            .map(|(slot, stack)| (slot, protocol_stack(&stack)))
                            .collect(),
                        attributes: match &state.kind {
                            EntityKind::Other { kind, .. } => {
                                clustine_sim::shown::mob_attributes(*kind, data)
                                    .into_iter()
                                    .map(|shown| {
                                        let snapshot = attribute_snapshot(shown);
                                        (snapshot.attribute, snapshot)
                                    })
                                    .collect()
                            }
                            EntityKind::Player { .. } => BTreeMap::new(),
                        },
                        ..LivingShown::default()
                    },
                };
                let shown = Shown {
                    state,
                    from,
                    dimension: self.dimension_of(from),
                    tracker: Some(tracker),
                    handed_on: None,
                    living,
                };
                self.entities.insert(entity, shown);
                // Before it is shown, so that a client shown it is told who rides it and
                // who holds it, as a vehicle's and a leashee's pairing tells it.
                self.note_riding(entity, &riding);
                self.refresh_entity(entity, false).await;
                return;
            }
        };
        if self
            .highest
            .get(&player)
            .is_some_and(|highest| entity < *highest)
            || self
                .ended
                .get(&player)
                .is_some_and(|ended| entity <= *ended)
        {
            return;
        }
        for earlier in self.note_stay(player, entity) {
            self.remove_entity(earlier).await;
        }
        // The values of the player's data that are not the defaults, which the region
        // reports with them wherever they are reported from: what a client that comes to
        // see them is shown (`sendPairingData`), whatever the edge was told before.
        let mut living = LivingShown::default();
        if let EntityKind::Player { data, .. } = &state.kind {
            for (index, value) in data.clone() {
                if let Some(value) =
                    crate::tracking::data_value(value, &mut |key| self.wire.of(key))
                {
                    living.data.insert(index, value);
                }
            }
        }
        let shown = Shown {
            state,
            from,
            dimension: self.dimension_of(from),
            tracker: None,
            handed_on: None,
            living,
        };
        let moved = self
            .entities
            .insert(entity, shown)
            .is_some_and(|known| known.state.pose != pose);
        self.refresh_entity(entity, moved).await;
    }

    /// The tick of `from` whose update an entity it introduces now is past: that of the
    /// report being handled if its entities' updates ran already, and the one before
    /// if not, so that one thrown before the tick's update is updated in it.
    fn tracker_start(&self, from: RegionId) -> u64 {
        let region = self.living(from);
        match self.reporting {
            Some(reporting) if reporting.region == region => {
                if reporting.updated {
                    reporting.tick
                } else {
                    reporting.tick.saturating_sub(1)
                }
            }
            _ => self.region_ticks.get(&region).copied().unwrap_or(0),
        }
    }

    /// Whether `entity` is gone with at least `hops` changes of holder behind its
    /// removal, so that a report with as many is of what was removed.
    fn was_gone(&self, entity: EntityId, hops: u32) -> bool {
        self.gone.get(&entity).is_some_and(|gone| hops <= gone.hops)
    }

    /// Notes that `region` has reported `tick`, and forgets the removals that every
    /// region the edge hears from has reported [`GONE_FOR`] ticks after.
    fn heard_tick(&mut self, region: RegionId, tick: u64) {
        self.region_ticks.insert(region, tick);
        if self.gone.is_empty() {
            return;
        }
        let linked: BTreeSet<RegionId> = self
            .regions
            .iter()
            .filter(|(_, port)| port.link.is_some())
            .map(|(id, _)| self.living(*id))
            .collect();
        let ticks = &self.region_ticks;
        self.gone.retain(|_, gone| {
            gone.heard.entry(region).or_insert(tick);
            !linked.iter().all(|linked| {
                let first = gone.heard.get(linked);
                let latest = ticks.get(linked);
                matches!((first, latest), (Some(first), Some(latest)) if *latest >= first + GONE_FOR)
            })
        });
    }

    /// Runs the updates of the entities shown from `region`: with `catching_up`, of the
    /// ticks before `tick` the region said nothing in, and otherwise of `tick` itself;
    /// each sent to everyone shown the entity.
    async fn update_entities(&mut self, region: RegionId, tick: u64, catching_up: bool) {
        let keys: Vec<EntityId> = self
            .entities
            .iter()
            .filter(|(_, shown)| shown.tracker.is_some() && self.living(shown.from) == region)
            .map(|(key, _)| *key)
            .collect();
        let mut outgoing: Vec<(PlayerId, Bytes)> = Vec::new();
        for key in keys {
            let viewers: Vec<PlayerId> = self
                .players
                .iter()
                .filter(|(_, view)| view.visible.contains_key(&key))
                .map(|(player, _)| *player)
                .collect();
            // Only a packet that names the entity needs its id.
            let id = if viewers.is_empty() {
                0
            } else {
                self.wire.of(key)
            };
            let Some(shown) = self.entities.get_mut(&key) else {
                continue;
            };
            let EntityKind::Other { data, .. } = &shown.state.kind else {
                continue;
            };
            let Some(tracker) = shown.tracker.as_mut() else {
                continue;
            };
            let sent = if catching_up {
                // A region restored from its checkpoint reports ticks again that the
                // tracker ran: it goes on from them.
                if tracker.ran >= tick {
                    tracker.ran = tick.saturating_sub(1);
                    continue;
                }
                let missed = tick - (tracker.ran + 1);
                if missed == 0 {
                    continue;
                }
                tracker.ran = tick - 1;
                tracker.catch_up(id, data, missed, &mut |key| self.wire.of(key))
            } else {
                if tracker.ran >= tick {
                    continue;
                }
                tracker.ran = tick;
                tracker.send_changes(id, data, &mut |key| self.wire.of(key))
            };
            for packet in sent {
                let packet = packet.encoded();
                outgoing.extend(viewers.iter().map(|player| (*player, packet.clone())));
            }
        }
        for (player, packet) in outgoing {
            self.send_to_player(player, [packet]).await;
        }
    }

    /// Takes the region's report of an entity's tick (`docs/groundwork/n4-plan.md`,
    /// section 7): what it shows of it now, for its tracker to send at its updates, and
    /// for its viewers to change where it moved into or out of their view. One the edge
    /// does not show is covered by the snapshot of its chunk, which is on its way.
    async fn track_entity(&mut self, from: RegionId, tracked: Tracked) {
        let dimension = self.dimension_of(from);
        let entity = tracked.entity;
        if self.was_gone(entity, tracked.hops) || !self.takes_word_of(from, entity, tracked.hops) {
            return;
        }
        let start = self.tracker_start(from);
        let Some(known) = self.entities.get(&entity) else {
            return;
        };
        let taken_over = self.living(known.from) != self.living(from);
        let Some(shown) = self.entities.get_mut(&entity) else {
            return;
        };
        let EntityKind::Other {
            motion, hops, data, ..
        } = &mut shown.state.kind
        else {
            return;
        };
        if let Some(tracker) = shown.tracker.as_mut() {
            tracker.report(&tracked, data);
            if taken_over {
                tracker.ran = start;
            }
        }
        *motion = tracked.motion;
        *hops = tracked.hops;
        if let Some(changed) = tracked.data {
            *data = changed;
        }
        let [x, y, z] = tracked.position;
        shown.state.pose = Pose {
            position: clustine_world::Vec3::new(x, y, z),
            yaw: tracked.yaw,
            pitch: tracked.pitch,
            on_ground: tracked.on_ground,
        };
        // A report with more changes of holder than it was handed on with is of where it
        // went: it is handed on no longer.
        if shown.handed_on.is_some_and(|handed| tracked.hops > handed) {
            shown.handed_on = None;
        }
        shown.from = from;
        shown.dimension = dimension;
        self.refresh_entity(entity, false).await;
    }

    /// Shows everyone who is shown the item `item` that `collector` picked up `count`
    /// of it (`ClientboundTakeItemEntityPacket`, which the game broadcasts to the item's
    /// trackers); its removal follows if it went whole.
    async fn show_taken(
        &mut self,
        from: RegionId,
        item: EntityId,
        collector: EntityId,
        count: i32,
    ) {
        if !self.entities.contains_key(&item) || !self.is_of(item, from) {
            return;
        }
        let watching: Vec<PlayerId> = self
            .players
            .iter()
            .filter(|(_, view)| view.visible.contains_key(&item))
            .map(|(player, _)| *player)
            .collect();
        if watching.is_empty() {
            return;
        }
        let packet = encoded(&TakeItemEntity {
            item_id: self.wire.of(item),
            collector_id: self.wire.of(collector),
            count,
        });
        self.release_wire(collector);
        for player in watching {
            self.send_to_player(player, [packet.clone()]).await;
        }
    }

    /// Shows the clients that have the chunk the block a falling block landed as, at
    /// once and then for a moment, as the game's `FallingBlockEntity` does: its
    /// `BlockUpdate`, then `AddTransientBlock`. The change of the block itself goes with
    /// the tick's other changes as well.
    async fn show_transient(&mut self, dimension: Dimension, position: BlockPos, state: u16) {
        let chunk = position.chunk();
        let at = Position {
            x: position.x,
            y: position.y,
            z: position.z,
        };
        let packets = [
            encoded(&BlockUpdate {
                position: at,
                state: i32::from(state),
            }),
            encoded(&AddTransientBlock {
                position: at,
                state: i32::from(state),
            }),
        ];
        let viewers: Vec<PlayerId> = self
            .players
            .iter()
            .filter(|(_, view)| {
                view.entity.is_some() && view.holds(LevelChunk::new(dimension, chunk))
            })
            .map(|(player, _)| *player)
            .collect();
        for player in viewers {
            self.send_to_player(player, packets.clone()).await;
        }
    }

    /// Whether a report of the region `from` about the entity `entity`, other than a
    /// player, with `hops` changes of holder behind it counts: it has more than the edge
    /// shows, or as many and is from the region the edge shows the entity from, or one
    /// that stands for it (`docs/adr/0028-entities.md`, section 6). One the edge does
    /// not show counts.
    fn takes_word_of(&self, from: RegionId, entity: EntityId, hops: u32) -> bool {
        let Some(shown) = self.entities.get(&entity) else {
            return true;
        };
        let shown_hops = match &shown.state.kind {
            EntityKind::Other { hops, .. } => *hops,
            // A key is one entity's for the life of the world.
            EntityKind::Player { .. } => return false,
        };
        hops > shown_hops || (hops == shown_hops && self.living(from) == self.living(shown.from))
    }

    /// The region `from` sent `entity` across a line with `hops` changes of holder behind
    /// it, to `position` with `motion`: it goes on being shown there, as continuing what
    /// was shown, until the region it went to reports it with more; one the edge does not
    /// show is not introduced (`docs/adr/0028-entities.md`, section 6).
    async fn hand_on(
        &mut self,
        from: RegionId,
        entity: EntityId,
        hops: u32,
        position: [f64; 3],
        motion: [f64; 3],
    ) {
        if self.was_gone(entity, hops) || !self.takes_word_of(from, entity, hops) {
            return;
        }
        let Some(shown) = self.entities.get_mut(&entity) else {
            return;
        };
        shown.handed_on = Some(hops);
        let [x, y, z] = position;
        shown.state.pose.position = Vec3::new(x, y, z);
        if let EntityKind::Other {
            motion: shown_motion,
            kind,
            ..
        } = &mut shown.state.kind
        {
            *shown_motion = motion;
            let seen = Seen {
                kind: *kind,
                position,
                motion,
                yaw: shown.state.pose.yaw,
                pitch: shown.state.pose.pitch,
                on_ground: shown.state.pose.on_ground,
            };
            if let Some(tracker) = shown.tracker.as_mut() {
                tracker.moved(seen);
            }
        }
        // Moved on the clients, or taken off those whose view it left: an entity handed
        // into land the edge does not watch is removed through the same path.
        self.refresh_entity(entity, true).await;
    }

    /// Shows the arm of the player `entity` swinging to everyone shown the player, and to
    /// the player too where `own` (`LivingEntity.swing`'s `sendToTrackingPlayersAndSelf`).
    async fn swing_arm(&mut self, entity: EntityId, off_hand: bool, swing: Swing, own: bool) {
        let watching: Vec<PlayerId> = self
            .players
            .iter()
            .filter(|(_, view)| {
                view.visible.contains_key(&entity) || (own && view.entity == Some(entity))
            })
            .map(|(player, _)| *player)
            .collect();
        if watching.is_empty() {
            return;
        }
        let packet = encoded(&SwingAnimation {
            entity_id: self.wire.of(entity),
            hand: i32::from(off_hand),
            kind: i32::from(swing.kind),
            duration: i32::from(swing.duration),
        });
        for player in watching {
            self.send_to_player(player, [packet.clone()]).await;
        }
    }

    /// Notes `entity` as a stay of `player` the edge has been told of. Returns the
    /// entities of earlier stays of the player that the edge still shows, which have
    /// to go first.
    fn note_stay(&mut self, player: PlayerId, entity: EntityId) -> Vec<EntityId> {
        let highest = self.highest.entry(player).or_insert(entity);
        if entity <= *highest {
            return Vec::new();
        }
        *highest = entity;
        // A later stay is passed over by `highest` alone from now on.
        if self.ended.get(&player).is_some_and(|ended| *ended < entity) {
            self.ended.remove(&player);
        }
        let of_player = |shown: &Shown| matches!(&shown.state.kind, EntityKind::Player { player: of, .. } if *of == player);
        let earlier = self.entities.iter();
        let earlier = earlier.filter(|(known, shown)| **known < entity && of_player(shown));
        earlier.map(|(known, _)| *known).collect()
    }

    /// Whether the edge has `player` in the world as `entity`.
    fn has_stay(&self, player: PlayerId, entity: EntityId) -> bool {
        self.players
            .get(&player)
            .is_some_and(|view| view.entity == Some(entity))
    }

    /// Brings every player's client up to date about one entity: shows it to those who
    /// have it in view, moves it if `moved`, and hides it from those who lost sight of it.
    async fn refresh_entity(&mut self, entity: EntityId, moved: bool) {
        let Some(shown) = self.entities.get(&entity).cloned() else {
            return;
        };
        let state = &shown.state;
        // A rider is placed by each client on its vehicle: it is sent no position, only
        // which way it looks (`ServerEntity`'s passengers).
        let rides = match self.rides_word.get(&entity) {
            Some(word) => word.vehicle,
            None => self.vehicle_of.get(&entity).copied(),
        };
        let lists = effective_passengers(&self.passengers, &self.rides_word);
        let mut outgoing = Vec::new();
        for (player, view) in &mut self.players {
            let newly = !view.visible.contains_key(&entity);
            let mut packets =
                view.update_visibility(*player, &shown, moved && rides.is_none(), &mut self.wire);
            let visible = view.visible.contains_key(&entity);
            if moved && rides.is_some() && visible && !newly {
                // `ServerEntity` sends a passenger its body's rotation and its head's.
                let id = self.wire.of(entity);
                packets.push(encoded(&clustine_protocol::packets::play::MoveEntityRot {
                    entity_id: id,
                    yaw: clustine_protocol::packets::play::pack_degrees(state.pose.yaw),
                    pitch: clustine_protocol::packets::play::pack_degrees(state.pose.pitch),
                    on_ground: state.pose.on_ground,
                }));
                packets.push(head_rotation(state, id));
            }
            // Who rides it, and what it rides, go with it to a client newly shown it,
            // and then who holds its leash (`ServerEntity.sendPairingData`).
            if newly && visible {
                let mut vehicles = vec![entity];
                vehicles.extend(rides);
                for vehicle in vehicles {
                    if let Some(riders) = lists.get(&vehicle)
                        && let Some(packet) =
                            passengers_packet(view, vehicle, riders, &mut self.wire)
                    {
                        packets.push(packet);
                    }
                }
                if let Some(holder) = self.leashes.get(&entity) {
                    packets.push(link_packet(entity, Some(*holder), &mut self.wire));
                }
            }
            if !packets.is_empty() {
                outgoing.push((*player, packets));
            }
        }
        if !self
            .replica
            .contains_key(&self.level_of(shown.from, state.chunk()))
        {
            // It went where this edge does not watch.
            self.entities.remove(&entity);
            self.release_wire(entity);
        }
        for (player, packets) in outgoing {
            self.send_to_player(player, packets).await;
        }
    }

    /// Takes in who rides the vehicle `vehicle` now, by the vehicle's word, and tells each
    /// client that shows it or rides it (`ClientboundSetPassengersPacket`, which the
    /// game sends the vehicle's trackers on any change of its list).
    async fn show_passengers(&mut self, vehicle: EntityId, passengers: Vec<EntityId>) {
        let shown_before = self.carried(vehicle);
        if let Some(before) = self.passengers.remove(&vehicle) {
            for rider in before {
                if self.vehicle_of.get(&rider) == Some(&vehicle) {
                    self.vehicle_of.remove(&rider);
                }
            }
        }
        for rider in &passengers {
            self.vehicle_of.insert(*rider, vehicle);
            // The vehicle names them under their word: its later word counts.
            if let Some(word) = self.rides_word.get_mut(rider)
                && word.vehicle == Some(vehicle)
            {
                word.named = true;
            }
        }
        if !passengers.is_empty() {
            self.passengers.insert(vehicle, passengers);
        }
        // The game tells a vehicle's trackers only of a change of its list: a vehicle
        // that names a rider their own word put in its seat already changes nothing.
        if self.carried(vehicle) != shown_before {
            self.tell_passengers(vehicle).await;
        }
    }

    /// Whom the vehicle `vehicle` carries as clients are shown it, by its word and its
    /// riders' own.
    fn carried(&self, vehicle: EntityId) -> Vec<EntityId> {
        effective_passengers(&self.passengers, &self.rides_word)
            .remove(&vehicle)
            .unwrap_or_default()
    }

    /// Tells each client that shows the vehicle `vehicle`, or rides it, whom it carries
    /// now, by its word and its riders' own.
    async fn tell_passengers(&mut self, vehicle: EntityId) {
        let riders = self.carried(vehicle);
        let mut outgoing = Vec::new();
        for (player, view) in &self.players {
            if let Some(packet) = passengers_packet(view, vehicle, &riders, &mut self.wire) {
                outgoing.push((*player, packet));
            }
        }
        for (player, packet) in outgoing {
            self.send_to_player(player, [packet]).await;
        }
    }

    /// Takes the word of the region of the player whose entity is `rider` that they ride
    /// `vehicle` now, or nothing, and tells the clients of the vehicles it concerns.
    async fn take_rides_word(&mut self, rider: EntityId, vehicle: Option<EntityId>) {
        let before = self.rides_word.get(&rider).and_then(|word| word.vehicle);
        let mut concerned: Vec<EntityId> = before.into_iter().chain(vehicle).collect();
        concerned.extend(self.vehicle_of.get(&rider).copied());
        concerned.sort();
        concerned.dedup();
        let shown_before: Vec<Vec<EntityId>> = concerned
            .iter()
            .map(|vehicle| self.carried(*vehicle))
            .collect();
        self.rides_word.insert(
            rider,
            RidesWord {
                vehicle,
                named: false,
            },
        );
        // Only a vehicle whose list the word changes is told of it, as the game tells a
        // change of the list alone: the word of a rider the vehicle names already, as
        // when their region says they got on in the tick the vehicle says so, is no news.
        for (vehicle, shown_before) in concerned.into_iter().zip(shown_before) {
            if self.carried(vehicle) != shown_before {
                self.tell_passengers(vehicle).await;
            }
        }
    }

    /// Takes in, from a report of the whole entity (a spawn, a snapshot), whom it carries
    /// and who holds its leash, which a client newly shown it is then told. A leash tied
    /// to a fence is held by the knot there, which the edge finds among what it shows.
    fn note_riding(&mut self, entity: EntityId, riding: &clustine_world::Riding) {
        let passengers = riding.shown_passengers();
        if let Some(before) = self.passengers.remove(&entity) {
            for rider in before {
                if self.vehicle_of.get(&rider) == Some(&entity) {
                    self.vehicle_of.remove(&rider);
                }
            }
        }
        for rider in &passengers {
            self.vehicle_of.insert(*rider, entity);
        }
        if !passengers.is_empty() {
            self.passengers.insert(entity, passengers);
        }
        let holder = riding
            .leash
            .as_deref()
            .and_then(|leash| match leash.holder {
                clustine_world::LeashHolder::Entity(key) => Some(key),
                clustine_world::LeashHolder::Fence(fence) => {
                    self.entities
                        .iter()
                        .find_map(|(key, shown)| match &shown.state.kind {
                            EntityKind::Other {
                                data: EntityData::LeashKnot { attached },
                                ..
                            } if attached.block == fence => Some(*key),
                            _ => None,
                        })
                }
            });
        match holder {
            Some(holder) => self.leashes.insert(entity, holder),
            None => self.leashes.remove(&entity),
        };
    }

    /// Takes in who holds the leash of `entity` now, by its region's word, and tells
    /// each client that shows it (`ClientboundSetEntityLinkPacket`, which the game sends
    /// the leashee's trackers on every change). A client looks the holder up by its id
    /// whenever it draws the leash, so one it is not shown yet is named all the same.
    async fn show_leash(&mut self, entity: EntityId, holder: Option<EntityId>) {
        match holder {
            Some(holder) => self.leashes.insert(entity, holder),
            None => self.leashes.remove(&entity),
        };
        let watching: Vec<PlayerId> = self
            .players
            .iter()
            .filter(|(_, view)| view.visible.contains_key(&entity))
            .map(|(player, _)| *player)
            .collect();
        if watching.is_empty() {
            return;
        }
        let packet = link_packet(entity, holder, &mut self.wire);
        for player in watching {
            self.send_to_player(player, [packet.clone()]).await;
        }
    }

    /// Tells every client that shows `entity` of the event `event`
    /// (`ClientboundEntityEventPacket`, which the game broadcasts to the entity's
    /// trackers).
    /// Sends `packet` to every player shown `entity`.
    async fn show_to_watchers(&mut self, entity: EntityId, packet: Bytes) {
        let watching: Vec<PlayerId> = self
            .players
            .iter()
            .filter(|(_, view)| view.visible.contains_key(&entity))
            .map(|(player, _)| *player)
            .collect();
        for player in watching {
            self.send_to_player(player, [packet.clone()]).await;
        }
    }

    /// A mob's attributes that changed or that it made: remembered for those who come
    /// to see it, and sent to those who see it.
    async fn show_attributes(&mut self, entity: EntityId, attributes: Vec<AttributeShown>) {
        let snapshots: Vec<AttributeSnapshot> =
            attributes.into_iter().map(attribute_snapshot).collect();
        if let Some(shown) = self.entities.get_mut(&entity) {
            for snapshot in &snapshots {
                shown
                    .living
                    .attributes
                    .insert(snapshot.attribute, snapshot.clone());
            }
        }
        let packet = encoded(&UpdateAttributes {
            entity_id: self.wire.of(entity),
            attributes: snapshots,
        });
        self.show_to_watchers(entity, packet).await;
    }

    /// A player lay down in the bed whose head is `head`, or got up: remembered for those
    /// who come to see them, and sent to those who see them and to themselves.
    async fn show_sleeping(&mut self, entity: EntityId, head: Option<BlockPos>) {
        let head = head.map(|head| Position {
            x: head.x,
            y: head.y,
            z: head.z,
        });
        if let Some(shown) = self.entities.get_mut(&entity) {
            shown.living.sleeping = head;
            // The pose and the bed are of the player's data, which a report of the
            // player carries too: kept as the sleep sets them, the standing pose and no
            // bed as the defaults they are.
            let data = &mut shown.living.data;
            data.remove(&entity_data::SLEEPING_POSITION);
            match head {
                Some(_) => {
                    data.insert(entity_data::POSE, EntityDataValue::Pose(pose::SLEEPING));
                }
                None => {
                    data.remove(&entity_data::POSE);
                }
            }
        }
        let packet = encoded(&sleeping_data(self.wire.of(entity), head));
        self.show_to_watchers(entity, packet.clone()).await;
        let own = self
            .players
            .iter()
            .find(|(_, view)| view.entity == Some(entity))
            .map(|(player, _)| *player);
        if let Some(player) = own {
            self.send_to_player(player, [packet]).await;
        }
    }

    /// Values a player's entity shows changed (the living flags of a use): remembered
    /// for those who come to see the player, and sent to those who see them now, but
    /// not to the player, whose own client foresees them.
    async fn show_player_data(
        &mut self,
        entity: EntityId,
        values: Vec<(u8, clustine_sim::shown::DataShown)>,
    ) {
        let values: Vec<(u8, EntityDataValue)> = values
            .into_iter()
            .filter_map(|(index, value)| {
                Some((
                    index,
                    crate::tracking::data_value(value, &mut |key| self.wire.of(key))?,
                ))
            })
            .collect();
        if let Some(shown) = self.entities.get_mut(&entity) {
            for (index, value) in &values {
                // A pose, a skin or a hand back at the default is no longer sent to
                // whoever comes to see the player (`getNonDefaultValues`).
                if is_default_of_player(*index, value) {
                    shown.living.data.remove(index);
                } else {
                    shown.living.data.insert(*index, value.clone());
                }
            }
        }
        let packet = encoded(&SetEntityData {
            entity_id: self.wire.of(entity),
            values,
        });
        self.show_to_watchers(entity, packet).await;
    }

    /// What the player's own entity data says that only the server sets (V4): their
    /// air, absorption, burning, ticks frozen and arrows, sent to their own client
    /// (`ServerEntity.sendDirtyEntityData` sends to the player too). The region reports
    /// it when any of it changes and whole at an arrival, so all five go each time
    /// (named: the game sends only the fields that changed; the client ends the same).
    /// The burning flag goes in with the shared flags the player is shown with.
    async fn show_own_data(&mut self, player: PlayerId, data: clustine_sim::api::OwnData) {
        let Some(entity) = self.players.get(&player).and_then(|view| view.entity) else {
            return;
        };
        let shown = self
            .entities
            .get(&entity)
            .and_then(|shown| shown.living.data.get(&entity_data::SHARED_FLAGS))
            .and_then(|value| match value {
                EntityDataValue::Byte(flags) => Some(*flags),
                _ => None,
            })
            .unwrap_or(0);
        let flags = if data.on_fire {
            shown | entity_data::ON_FIRE
        } else {
            shown & !entity_data::ON_FIRE
        };
        let packet = encoded(&SetEntityData {
            entity_id: self.wire.of(entity),
            values: vec![
                (entity_data::SHARED_FLAGS, EntityDataValue::Byte(flags)),
                (entity_data::AIR_SUPPLY, EntityDataValue::Int(data.air)),
                (entity_data::TICKS_FROZEN, EntityDataValue::Int(data.frozen)),
                (entity_data::ARROW_COUNT, EntityDataValue::Int(data.arrows)),
                (
                    entity_data::PLAYER_ABSORPTION,
                    EntityDataValue::Float(data.absorption),
                ),
            ],
        });
        self.send_to_player(player, [packet]).await;
    }

    /// What a mob wears and holds changed: remembered, and sent to those who see it.
    async fn show_equipment(&mut self, entity: EntityId, slots: Vec<(u8, Option<ItemStack>)>) {
        let slots: Vec<(u8, Option<WireStack>)> = slots
            .into_iter()
            .map(|(slot, stack)| {
                let stack = stack.as_ref().map(protocol_stack);
                (slot, stack)
            })
            .collect();
        if let Some(shown) = self.entities.get_mut(&entity) {
            for (slot, stack) in &slots {
                match stack {
                    Some(stack) => shown.living.equipment.insert(*slot, stack.clone()),
                    None => shown.living.equipment.remove(slot),
                };
            }
        }
        let packet = encoded(&SetEquipment {
            entity_id: self.wire.of(entity),
            slots,
        });
        self.show_to_watchers(entity, packet).await;
    }

    async fn show_entity_event(&mut self, entity: EntityId, event: u8) {
        // `broadcastEntityEvent` tells the entity itself too; but a player's own death
        // (event 3) they are shown after the death itself (`died`).
        let watching: Vec<PlayerId> = self
            .players
            .iter()
            .filter(|(_, view)| {
                view.visible.contains_key(&entity) || (view.entity == Some(entity) && event != 3)
            })
            .map(|(player, _)| *player)
            .collect();
        if watching.is_empty() {
            return;
        }
        let packet = encoded(&EntityEvent {
            entity_id: self.wire.of(entity),
            event: event as i8,
        });
        for player in watching {
            self.send_to_player(player, [packet.clone()]).await;
        }
    }

    /// The player got on or off a vehicle, which the game does as a teleport: their
    /// client is told where they are now (`player_position`), with its look left as it
    /// is (`Relative.ROTATION`, by nothing), and its moves are passed over until it
    /// confirms it. The look it confirms with is then the player's (`play.rs`).
    async fn rides(&mut self, player: PlayerId, pose: Pose) {
        let Some(view) = self.players.get_mut(&player) else {
            return;
        };
        view.teleports += 1;
        let teleport_id = view.teleports;
        view.awaiting_teleport.store(teleport_id, Ordering::Relaxed);
        let packet = encoded(&SynchronizePlayerPosition {
            teleport_id,
            x: pose.position.x,
            y: pose.position.y,
            z: pose.position.z,
            velocity_x: 0.0,
            velocity_y: 0.0,
            velocity_z: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            relative_flags: relative::ROTATION,
        });
        self.send_to_player(player, [packet]).await;
    }

    /// Puts the vehicle the player drives back where their region has it, as the game
    /// answers a move it refused (the clientbound `move_vehicle`).
    async fn vehicle_corrected(
        &mut self,
        player: PlayerId,
        position: clustine_world::Vec3,
        yaw: f32,
        pitch: f32,
    ) {
        let packet = encoded(&ClientboundMoveVehicle {
            x: position.x,
            y: position.y,
            z: position.z,
            yaw,
            pitch,
        });
        self.send_to_player(player, [packet]).await;
    }

    /// Puts the player out for `reason`, a sentence of the game's that their client
    /// shows in its own language (`multiplayer.disconnect.flying`), as the region asked;
    /// their region is told they left.
    async fn kick(&mut self, player: PlayerId, reason: String) {
        let Some(view) = self.players.get(&player) else {
            return;
        };
        let _ = view
            .outbound
            .try_send(encoded(&Disconnect::from(Text::Translatable(reason))));
        self.remove_player(player).await;
    }

    /// Forgets an entity that no longer exists and hides it from everyone who saw it.
    async fn remove_entity(&mut self, entity: EntityId) {
        self.entities.remove(&entity);
        self.leashes.remove(&entity);
        self.rides_word.remove(&entity);
        // A vehicle that is gone carries nobody, and a rider that is gone rides nothing.
        if let Some(riders) = self.passengers.remove(&entity) {
            for rider in riders {
                if self.vehicle_of.get(&rider) == Some(&entity) {
                    self.vehicle_of.remove(&rider);
                }
            }
        }
        if let Some(vehicle) = self.vehicle_of.remove(&entity)
            && let Some(riders) = self.passengers.get_mut(&vehicle)
        {
            riders.retain(|rider| *rider != entity);
        }
        let viewers: Vec<_> = self
            .players
            .iter_mut()
            .filter_map(|(player, view)| view.visible.remove(&entity).map(|_| *player))
            .collect();
        // Only a packet that names the entity needs its id: one nobody was shown is
        // not given one now.
        for player in viewers {
            let packet = encoded(&RemoveEntities {
                entity_ids: vec![self.wire.of(entity)],
            });
            self.send_to_player(player, [packet]).await;
        }
        self.release_wire(entity);
    }

    /// Lets the wire id of `entity` go if nothing needs it any more: nobody is shown the
    /// entity, the edge does not keep it, and no connected player of the edge is it.
    fn release_wire(&mut self, entity: EntityId) {
        let needed = self.entities.contains_key(&entity)
            || self
                .players
                .values()
                .any(|view| view.entity == Some(entity) || view.visible.contains_key(&entity));
        if !needed {
            self.wire.forget(entity);
        }
    }

    /// Forgets the entities in chunks the replica no longer has, which nobody sees any
    /// more, and lets their wire ids go.
    fn forget_unwatched_entities(&mut self) {
        let unwatched: Vec<_> = self
            .entities
            .iter()
            .filter(|(_, shown)| {
                let chunk = self.level_of(shown.from, shown.state.chunk());
                !self.replica.contains_key(&chunk)
            })
            .map(|(entity, _)| *entity)
            .collect();
        for entity in unwatched {
            self.entities.remove(&entity);
            self.release_wire(entity);
        }
    }

    /// Puts a player a region has placed into the world: the packets that start the
    /// play state, then the chunks around them.
    async fn spawn_player(&mut self, player: PlayerId, entering: Entering) {
        if !self.note_entered(player, entering.entity).await {
            return;
        }
        let position = entering.pose.position;
        let (entity, riding) = (entering.entity, entering.riding);
        // The level is the placing region's: a stay kept in another dimension than the
        // overworld is placed by a region of it (`docs/groundwork/p-plan.md`, section 4.2).
        let Some(region) = self.players.get(&player).map(|view| view.region) else {
            return;
        };
        let dimension = self.dimension_of(region);
        if !self.tell_entered(player, dimension, entering).await {
            return;
        }
        // A player put back on their unit rides it from the first: the word is taken
        // after their login and before their view, so that the unit is introduced to
        // them and to everyone with them in its seats
        // (`docs/groundwork/n7h-logout-on-a-mount.md`, section 6).
        if riding.is_some() {
            self.take_rides_word(entity, riding).await;
        }
        let center = ChunkPos::containing(position.x, position.z);
        self.move_view(player, center).await;
        let region = self.players.get(&player).map(|view| view.region);
        if let Some(region) = region {
            self.tell_time(player, region, true).await;
        }
    }

    /// Tells the player the time by the clock `region` last said, if it said one: the
    /// clocks whole where `whole`, or where the player was not told them by this region
    /// yet (a join, a hand-over), and the game time alone otherwise, as the game sends
    /// it every 20 ticks (`MinecraftServer.forceGameTimeSynchronization`).
    async fn tell_time(&mut self, player: PlayerId, region: RegionId, whole: bool) {
        let Some(clock) = self.clocks.get(&region).copied() else {
            return;
        };
        let Some(view) = self.players.get_mut(&player) else {
            return;
        };
        let whole = whole || view.told_clocks != Some(region);
        let clocks = if whole {
            view.told_clocks = Some(region);
            // Every clock of the registry, as the official server sends them: the
            // overworld's, which sleep moves, and the End's, which nothing moves and so
            // counts the ticks as the game time does (**inferred**: Clustine has no End
            // yet, and the official server's two agree in a world nobody slept in).
            let clock_of = |name: &str| {
                synced_registry("minecraft:world_clock")
                    .and_then(|registry| registry.id_of(name))
                    .expect("the world's clocks are the overworld's and the End's")
            };
            [
                (clock_of("minecraft:overworld"), clock.day),
                (clock_of("minecraft:the_end"), clock.game_time),
            ]
            .into_iter()
            .map(|(of, ticks)| ClockState {
                clock: of,
                total_ticks: ticks as i64,
                partial_tick: 0.0,
                rate: 1.0,
            })
            .collect()
        } else {
            Vec::new()
        };
        let packet = encoded(&SetTime {
            game_time: clock.game_time as i64,
            clocks,
        });
        self.send_to_player(player, [packet]).await;
    }

    /// Puts a player into the world whose stay a region let go before the edge was
    /// told its entity. The order is that of any hand-over: what the edge notes, what
    /// it asks of the region the stay goes to, the arrival, and only then anything to
    /// the client. A client whose queue is full is removed by the first packet that
    /// does not fit, and that removal sends the stay's leave: were the packets first,
    /// the leave would be in front of the arrival, the region would apply it to nobody
    /// and then take the stay in, with nobody behind it.
    async fn enter_by_arrival(&mut self, player: PlayerId, to: RegionId, transfer: PlayerTransfer) {
        // A stay a portal sent on that no region has placed yet: the region it goes to
        // places it when its wait is over, and says so with `Spawned`, which puts the
        // client into the world there, in its dimension. Until then the client is not
        // in the world, and the join's patience runs on (`docs/groundwork/p-plan.md`,
        // section 16.8).
        if transfer.travel.is_some() {
            if let Some(view) = self.players.get_mut(&player) {
                view.region = to;
            }
            self.send_to_region(to, EdgeToWorker::PlayerArrive { player, transfer })
                .await;
            return;
        }
        let entity = transfer.entity_id;
        let position = transfer.pose.position;
        let entering = Entering {
            riding: transfer.riding.map(|riding| riding.vehicle),
            entity,
            pose: transfer.pose,
            flying: transfer.flying,
            inventory: transfer.inventory.clone(),
            selected_slot: transfer.selected_slot,
            last_death: transfer.last_death,
            mode: transfer.mode,
        };
        // The view's level is the stay's before the view is shifted, which keys every
        // chunk it asks for by it: a join placed in the End by its home has the
        // overworld's until here, and no link to the End need be up yet to say so.
        let dimension = transfer.dimension;
        if let Some(view) = self.players.get_mut(&player) {
            view.region = to;
            view.dimension = dimension;
        }
        if !self.note_entered(player, entity).await {
            return;
        }
        let riding = entering.riding;
        let center = ChunkPos::containing(position.x, position.z);
        let view_packets = self.shift_view(player, center).await;
        self.send_to_region(to, EdgeToWorker::PlayerArrive { player, transfer })
            .await;
        if !self.tell_entered(player, dimension, entering).await {
            return;
        }
        // One put back on the unit they left on rides it from the first, as
        // `spawn_player` has it.
        if riding.is_some() {
            self.take_rides_word(entity, riding).await;
        }
        if let Some(packets) = view_packets
            && self.send_to_player(player, packets).await
        {
            self.send_chunks(player).await;
        }
    }

    /// Notes that `player` is in the world as `entity`. False if they are not there
    /// to be entered: they left before the region answered, or have been entered,
    /// as when the word comes once as it happened and once on resuming.
    async fn note_entered(&mut self, player: PlayerId, entity: EntityId) -> bool {
        let Some(view) = self.players.get_mut(&player) else {
            return false;
        };
        if view.entity.is_some() {
            return false;
        }
        view.entity = Some(entity);
        view.joining_since = None;
        self.entity_owners.insert(entity, player);
        self.config.online.fetch_add(1, Ordering::Relaxed);
        info!(name = %view.name, entity_id = entity.0, "player joined");
        // What a region still shows of an earlier stay of the player goes first.
        for earlier in self.note_stay(player, entity) {
            self.remove_entity(earlier).await;
        }
        true
    }

    /// Sends a player who has just been noted as in the world what starts the play
    /// state: the login, what they may do and whether they fly, where they are and
    /// how they look, their inventory, and the player list. False if the player was
    /// removed on the way.
    async fn tell_entered(
        &mut self,
        player: PlayerId,
        dimension: Dimension,
        entering: Entering,
    ) -> bool {
        let Entering {
            entity,
            pose,
            flying,
            inventory,
            selected_slot,
            last_death,
            mode,
            ..
        } = entering;
        let Some(view) = self.players.get_mut(&player) else {
            return false;
        };
        // Window 0 whole, its grid and cursor empty until the region reports them
        // (`initInventoryMenu`: state id 1), and the hotbar slot held.
        let inventory = [
            view.menus.entered(&inventory),
            encoded(&SetHeldSlot {
                slot: selected_slot.into(),
            }),
        ];
        view.dimension = dimension;
        view.last_death = last_death;
        view.mode = mode;
        // Placing the player is the first teleport the client has to confirm.
        let teleport_id = 1;
        view.teleports = teleport_id;
        view.awaiting_teleport.store(teleport_id, Ordering::Relaxed);
        let flags = ability_flags(mode.current, flying);
        let entered = [
            encoded(&login_packet(
                self.wire.of(entity),
                &self.config,
                dimension,
                last_death,
                mode,
            )),
            // The world's difficulty, unlocked as a new dedicated server's is; a client's
            // change of it is not taken (`docs/groundwork/n7k-plan.md`, section 10).
            encoded(&ChangeDifficulty {
                difficulty: clustine_world::Difficulty::WORLD.id() as u8,
                locked: false,
            }),
            encoded(&PlayerAbilities {
                flags,
                flying_speed: 0.05,
                field_of_view_modifier: 0.1,
            }),
            // `PlayerList.placeNewPlayer` @407-424: after the held slot (which the
            // window-0 packets carry here), the recipes the client needs (V3c).
            update_recipes_packet(),
            encoded(&SynchronizePlayerPosition {
                teleport_id,
                x: pose.position.x,
                y: pose.position.y,
                z: pose.position.z,
                velocity_x: 0.0,
                velocity_y: 0.0,
                velocity_z: 0.0,
                yaw: pose.yaw,
                pitch: pose.pitch,
                relative_flags: 0,
            }),
            // As the official server's level information, after the position and
            // before the wait for chunks: where a compass points.
            encoded(&SetDefaultSpawnPosition {
                dimension: OVERWORLD.to_owned(),
                position: Position {
                    x: self.world_spawn.x,
                    y: self.world_spawn.y,
                    z: self.world_spawn.z,
                },
                yaw: 0.0,
                pitch: 0.0,
            }),
            encoded(&GameEvent {
                event: game_event::START_WAITING_FOR_LEVEL_CHUNKS,
                value: 0.0,
            }),
        ];
        if !self.send_to_player(player, entered).await
            || !self.send_to_player(player, inventory).await
        {
            return false;
        }

        // Everyone in the world is in everyone's player list, including their own.
        let in_world: Vec<_> = self
            .players
            .iter()
            .filter(|(_, view)| view.entity.is_some())
            .map(|(id, view)| (*id, view.name.clone(), view.mode.current.id()))
            .collect();
        let newcomer = in_world
            .iter()
            .find(|(id, ..)| *id == player)
            .cloned()
            .expect("the player was just placed");
        for (other, ..) in &in_world {
            let additions: &[(PlayerId, String, u8)] = if *other == player {
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
                .filter(|(id, _, mode)| view.listed.insert(*id, *mode).is_none())
                .cloned()
                .collect();
            if !unlisted.is_empty() {
                let packet = encoded(&player_list_additions(&unlisted));
                self.send_to_player(*other, [packet]).await;
            }
        }
        self.players.contains_key(&player)
    }

    /// Centres a player's view on `center`: tells the client, starts sending the chunks
    /// that came into view and makes the client forget those that left it.
    async fn move_view(&mut self, player: PlayerId, center: ChunkPos) {
        if let Some(packets) = self.shift_view(player, center).await
            && self.send_to_player(player, packets).await
        {
            self.send_chunks(player).await;
        }
    }

    /// The half of [`Fanout::move_view`] that the regions hear of: the view is centred
    /// on `center` and the chunks that came into it are asked of the player's region.
    /// Returns what the client is to be sent for it, which the caller sends; `None` if
    /// nothing changed.
    async fn shift_view(&mut self, player: PlayerId, center: ChunkPos) -> Option<Vec<Bytes>> {
        let view = self.players.get(&player)?;
        if view.center == center && !view.wanted.is_empty() {
            return None;
        }
        Some(self.reshape_view(player, center).await)
    }

    /// Lays out a player's view around `center` at their view distance, whether or not
    /// it changed: the chunks that came into it are asked of the player's region, and
    /// those that left it are let go. Returns what the client is to be sent for it,
    /// which the caller sends.
    async fn reshape_view(&mut self, player: PlayerId, center: ChunkPos) -> Vec<Bytes> {
        let view = self
            .players
            .get_mut(&player)
            .expect("a view is reshaped only for a player the edge has");
        let wanted = view_area(center, view.view_distance);
        let mut packets = vec![encoded(&SetCenterChunk {
            chunk_x: center.x,
            chunk_z: center.z,
        })];

        let region = view.region;
        let dimension = view.dimension;
        let mut left = Vec::new();
        for position in view.wanted.difference(&wanted) {
            left.push(*position);
            let at = LevelChunk::new(dimension, *position);
            let waited = view.pending.remove(position);
            // What the client has: sent, or sent before and waiting to be replaced.
            let replaced = view.replace.remove(position);
            if view.sent.remove(position) || replaced {
                packets.push(encoded(&UnloadChunk {
                    chunk_x: position.x,
                    chunk_z: position.z,
                }));
            }
            let chunk = self
                .replica
                .get_mut(&at)
                .expect("viewed chunks are in the replica");
            if waited {
                chunk.waits_no_more(player);
            }
            chunk.viewers -= 1;
            if chunk.viewers == 0 {
                self.queues.take_out(at, chunk);
                self.replica.remove(&at);
                self.marked.remove(&at);
            }
        }
        let mut entered = Vec::new();
        for position in wanted.difference(&view.wanted) {
            entered.push(*position);
            let chunk = self
                .replica
                .entry(LevelChunk::new(dimension, *position))
                .or_insert_with(ReplicaChunk::awaited);
            chunk.viewers += 1;
            if chunk.chunk.is_some() {
                view.pending.insert(*position);
                chunk.waiters.insert(player);
            }
        }
        view.center = center;
        view.wanted = wanted;
        // Entities in chunks that came into view appear, those left behind disappear.
        let shown_before: BTreeSet<EntityId> = view.visible.keys().copied().collect();
        for shown in self.entities.values() {
            packets.extend(view.update_visibility(player, shown, false, &mut self.wire));
        }
        // And who rides those that appeared, or what those that appeared ride.
        for (vehicle, riders) in &effective_passengers(&self.passengers, &self.rides_word) {
            let appeared =
                |key: &EntityId| view.visible.contains_key(key) && !shown_before.contains(key);
            if (appeared(vehicle) || riders.iter().any(appeared))
                && let Some(packet) = passengers_packet(view, *vehicle, riders, &mut self.wire)
            {
                packets.push(packet);
            }
        }
        // And who holds the leashes of those that appeared.
        for (leashee, holder) in &self.leashes {
            if view.visible.contains_key(leashee) && !shown_before.contains(leashee) {
                packets.push(link_packet(*leashee, Some(*holder), &mut self.wire));
            }
        }
        // Who waits for what has changed, and how near they are: that moves entries
        // between the sets of light and sends or dispatches nothing.
        let pending: Vec<ChunkPos> = view.pending.iter().copied().collect();
        for position in &left {
            self.requeue(LevelChunk::new(dimension, *position));
        }
        for position in pending {
            self.rerank(LevelChunk::new(dimension, position), player);
        }
        self.forget_unwatched_entities();

        for chunk in left {
            self.unwant(region, chunk);
        }
        for chunk in entered {
            self.want(region, chunk);
        }
        self.flush_asking().await;
        packets
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
        let at = self.level_of(from, position);
        let Some(entry) = self.replica.get_mut(&at) else {
            // Nobody has the chunk in view any more.
            return;
        };
        let shown_before = entry.chunk.is_some();
        // By what a client is shown, and not by everything a chunk carries: one that
        // differs from the replica's only in what is not shown (whether it is as
        // generated, its light as carried) is not sent to its viewers again, or every
        // resume, hand-over, merge and split would send such chunks whole
        // (`docs/adr/0022-what-a-chunk-carries.md`, section 2).
        let same = entry
            .chunk
            .as_ref()
            .is_some_and(|known| known.shows_the_same(&chunk));
        if same {
            // Its viewers have it, and light follows from blocks, which are the same:
            // the entry keeps the chunk and the light it has, whichever region the
            // snapshot is from.
        } else {
            self.arrive(at, chunk);
            let viewers: Vec<_> = self
                .players
                .iter_mut()
                .filter(|(_, view)| {
                    view.dimension == at.dimension && view.wanted.contains(&position)
                })
                .map(|(player, view)| {
                    // A client that has the chunk keeps it until the new one goes
                    // out, behind an unload. One that is waiting to have it replaced
                    // already waits on.
                    if view.sent.remove(&position) {
                        view.replace.insert(position);
                    }
                    view.pending.insert(position);
                    *player
                })
                .collect();
            if let Some(entry) = self.replica.get_mut(&at) {
                entry.waiters.extend(viewers.iter().copied());
            }
            self.requeue(at);
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
                        // What the region handed on is no longer its to show or take off.
                        && shown.handed_on.is_none()
                        && !self.entity_owners.contains_key(id)
                        && entities.iter().all(|present| present.entity != **id)
                })
                .map(|(id, _)| *id)
                .collect();
            for entity in stale {
                self.remove_entity(entity).await;
            }
            // What another region handed on into this chunk and the snapshot does not
            // have was lost on its way (a sender's tick lost after the edge heard of it):
            // taken off the clients, its key kept at the hops it was handed on with, so
            // that a late word of it does not bring it back. A sender's own snapshot of
            // its chunk does not take off what it handed on (`docs/adr/0028-entities.md`,
            // section 6; `docs/groundwork/n3-plan.md`, section 5, G9).
            let lost: Vec<(EntityId, u32)> = self
                .entities
                .iter()
                .filter_map(|(id, shown)| {
                    let hops = shown.handed_on?;
                    (shown.state.chunk() == position
                        && shown.dimension == at.dimension
                        && shown.from != from
                        && entities.iter().all(|present| present.entity != *id))
                    .then_some((*id, hops))
                })
                .collect();
            for (entity, hops) in lost {
                self.remove_entity(entity).await;
                self.gone.insert(
                    entity,
                    Gone {
                        hops,
                        heard: BTreeMap::new(),
                    },
                );
            }
        }
        for state in entities {
            self.upsert_entity(from, state, false).await;
        }
    }

    /// The eight chunks around `position`: north-west, north, north-east, west, east,
    /// south-west, south, south-east, which is the order the light rules have them in.
    fn around(position: LevelChunk) -> [LevelChunk; 8] {
        let at = |dx, dz| {
            let around = ChunkPos::new(position.position.x + dx, position.position.z + dz);
            LevelChunk::new(position.dimension, around)
        };
        [
            at(-1, -1),
            at(0, -1),
            at(1, -1),
            at(-1, 0),
            at(1, 0),
            at(-1, 1),
            at(0, 1),
            at(1, 1),
        ]
    }

    /// The next moment.
    fn next_moment(&mut self) -> u64 {
        self.moment += 1;
        self.moment
    }

    /// The test for carried light (`docs/adr/0023-light-at-the-edge.md`, section 7):
    /// `light`, which the chunk at `position` came with, was made by rules the edge
    /// takes, beside chunks of one generator's version, and every chunk around it
    /// that the replica has is still, as light sees it, as that version made it. A
    /// neighbour that is not there passes: when it comes, this is asked again.
    fn carried_light_holds(&self, position: LevelChunk, light: &Light) -> bool {
        let taken = light.made_by == clustine_light::VERSION || light.made_by == Light::IMPORTED;
        let beside = |around: &LevelChunk| match self.replica.get(around) {
            Some(entry) if entry.chunk.is_some() => entry.as_made == Some(light.beside),
            _ => true,
        };
        taken && light.beside != 0 && Self::around(position).iter().all(beside)
    }

    /// A chunk comes into an entry, or takes the place of the one that was there:
    /// its light is taken out and kept if it passes the test, and the chunks around
    /// it are asked again whether theirs still holds beside this one. What does not
    /// hold may not be sent, and is marked for a job.
    fn arrive(&mut self, position: LevelChunk, mut chunk: Chunk) {
        let mut carried = chunk.take_light();
        if self.config.light.takes_as_changed(position.position) {
            carried = None;
            chunk.mark_generated(0, false);
        }
        let now = Instant::now();
        // What the chunk is as made by, as light sees it: its light says, whoever's
        // rules made it, since nothing light sees has changed since; without light,
        // the chunk says, as long as it is as generated.
        let as_made = match &carried {
            Some(light) if light.beside != 0 => Some(light.beside),
            _ => (chunk.generated() != 0).then_some(chunk.generated()),
        };
        let shaped = self.next_moment();
        let entry = self
            .replica
            .get_mut(&position)
            .expect("the caller found it");
        let could_be_sent = entry.sendable();
        entry.chunk = Some(Arc::new(chunk));
        entry.packet = None;
        entry.came = Some(std::time::Instant::now());
        // A job under way was for the chunk of before; its result is dropped.
        entry.job = None;
        entry.light = None;
        // Whoever had the chunk is sent the new one whole.
        entry.owed = false;
        entry.reshaped = false;
        entry.exempt = false;
        entry.as_made = as_made;
        entry.shaped = shaped;
        let holds = carried
            .as_ref()
            .is_some_and(|light| self.carried_light_holds(position, light));
        if holds {
            let entry = self.replica.get_mut(&position).expect("found above");
            entry.light = carried.map(Arc::new);
            entry.kind = LightKind::Carried;
            entry.lit = shaped;
            entry.marked_since = None;
            entry.unsendable_since = None;
            self.marked.remove(&position);
        } else {
            // A chunk that takes the place of one that could not be sent either does
            // not begin its hold again: replaced as often as that, it would never
            // end while a neighbour stays away.
            let entry = self.replica.get_mut(&position).expect("found above");
            entry.marked_since.get_or_insert(now);
            if could_be_sent {
                entry.unsendable_since = Some(now);
            }
            entry.unsendable_since.get_or_insert(now);
            self.marked.insert(position);
        }
        for beside in Self::around(position) {
            let keeps = self.replica.get(&beside).and_then(|entry| {
                entry.chunk.as_ref()?;
                // Carried light that was right stays right if the test still passes
                // beside the new chunk. Light the edge made, or could not make, was
                // made without this chunk: a new neighbour is a new input.
                let light = entry.light.as_deref()?;
                let kept = entry.kind == LightKind::Carried
                    && entry.current()
                    && entry.job.is_none()
                    && self.carried_light_holds(beside, light);
                Some(kept)
            });
            let Some(keeps) = keeps.or_else(|| {
                let entry = self.replica.get(&beside)?;
                entry.chunk.as_ref().map(|_| false)
            }) else {
                continue;
            };
            let moment = self.next_moment();
            let entry = self.replica.get_mut(&beside).expect("found above");
            let could_be_sent = entry.sendable();
            entry.shaped = moment;
            if keeps {
                entry.lit = moment;
            } else {
                entry.reshaped = true;
                // Its light may not be sent as it is, and so not a packet made of it.
                entry.packet = None;
                if could_be_sent {
                    entry.unsendable_since = Some(now);
                }
                entry.unsendable_since.get_or_insert(now);
                entry.marked_since.get_or_insert(now);
                self.marked.insert(beside);
            }
        }
        // The chunk goes where it now belongs, and so do those around it: one that
        // was held for this chunk is held for it no longer.
        self.requeue(position);
        for beside in Self::around(position) {
            self.requeue(beside);
        }
    }

    /// Where the marked entry at `position`, which has no job under way, belongs
    /// now (`docs/adr/0023-light-at-the-edge.md`, section 4). The moment it is
    /// put in is left for [`Fanout::requeue`] to say.
    fn place_of(&self, position: LevelChunk, entry: &ReplicaChunk, now: Instant) -> Queued {
        let settings = self.config.light;
        if !entry.sendable() {
            // Held while a chunk beside it is wanted and has not come, for as long
            // as the hold allows: lit now, it would be lit again when that comes.
            let until = entry
                .unsendable_since
                .map_or(now, |since| since + settings.latest);
            let awaited = |at: &LevelChunk| {
                self.replica
                    .get(at)
                    .is_some_and(|beside| beside.chunk.is_none())
            };
            if until > now && Self::around(position).iter().any(awaited) {
                return Queued::Later { until };
            }
            let nearest = |waiter: &PlayerId| {
                let view = self.players.get(waiter)?;
                Some((apart(position.position, view.center), *waiter))
            };
            if let Some((rank, by)) = entry.waiters.iter().filter_map(nearest).min() {
                return Queued::Waiting { rank, since: 0, by };
            }
        } else if !entry.exempt
            && let Some(until) = entry.dispatched.map(|at| at + settings.rest)
            && until > now
        {
            // It can be sent, a little old, and had a job a moment ago. A place
            // where water flows is never still, and is never held for anything.
            return Queued::Later { until };
        }
        let viewers = self.players.values();
        let viewers = viewers.filter(|view| {
            view.dimension == position.dimension && view.wanted.contains(&position.position)
        });
        let nearest = viewers
            .map(|view| apart(position.position, view.center))
            .min();
        Queued::Unawaited {
            rank: nearest.unwrap_or(0),
            since: 0,
            marked: entry.marked_since.unwrap_or(now),
        }
    }

    /// Puts the entry at `position` into the set it belongs in now, or takes it out
    /// of them if it is not marked or has a job under way. Called wherever what
    /// that turns on has changed; it changes nothing where nothing has.
    fn requeue(&mut self, position: LevelChunk) {
        let now = Instant::now();
        let Some(entry) = self.replica.get(&position) else {
            return;
        };
        let was = entry.queued;
        let idle = self.marked.contains(&position) && entry.job.is_none();
        let place = idle.then(|| self.place_of(position, entry, now));
        let place = match (place, was) {
            // Among equally near chunks the one that has waited longer goes first,
            // so a new rank keeps the moment.
            (Some(Queued::Waiting { rank, by, .. }), Some(Queued::Waiting { since, .. })) => {
                Some(Queued::Waiting { rank, since, by })
            }
            (Some(Queued::Waiting { rank, by, .. }), _) => {
                let since = self.next_moment();
                Some(Queued::Waiting { rank, since, by })
            }
            // The rank of one nobody waits for is not kept up.
            (Some(Queued::Unawaited { .. }), Some(Queued::Unawaited { .. })) => was,
            (Some(Queued::Unawaited { rank, marked, .. }), _) => {
                let since = self.next_moment();
                Some(Queued::Unawaited {
                    rank,
                    since,
                    marked,
                })
            }
            (place, _) => place,
        };
        if place == was {
            return;
        }
        let entry = self.replica.get_mut(&position).expect("found above");
        self.queues.take_out(position, entry);
        if let Some(place) = place {
            self.queues.put_in(position, entry, place);
        }
    }

    /// `mover`'s view has a new centre: the rank of a chunk they wait for is the
    /// distance to its nearest waiter as the views now stand. It is found anew over
    /// all who wait only if the mover was the one it was ranked by; otherwise it is
    /// the lesser of what it was and the mover's distance now.
    fn rerank(&mut self, position: LevelChunk, mover: PlayerId) {
        let Some(view) = self.players.get(&mover) else {
            return;
        };
        let mine = apart(position.position, view.center);
        let Some(entry) = self.replica.get_mut(&position) else {
            return;
        };
        match entry.queued {
            Some(Queued::Waiting { rank, since, by }) if by != mover => {
                if mine < rank {
                    self.queues.take_out(position, entry);
                    let nearer = Queued::Waiting {
                        rank: mine,
                        since,
                        by: mover,
                    };
                    self.queues.put_in(position, entry, nearer);
                }
            }
            _ => self.requeue(position),
        }
    }

    /// When the task is next to wake for light alone, if at all: when the first
    /// hold or rest ends, or the first job is to be given up, and never sooner
    /// than a tick after it last did.
    fn light_wake(&self) -> Option<Instant> {
        let later = self.queues.later.first().map(|(until, _)| *until);
        let patience = self.config.light.job_patience;
        let given_up = self.started.values().map(|(_, at)| *at + patience).min();
        let due = [later, given_up].into_iter().flatten().min()?;
        Some(due.max(self.light_ticked + LIGHT_TICK))
    }

    /// Moves the entries whose hold or rest has ended, and gives up the jobs a
    /// thread has had for too long: such a job has failed, and its thread is
    /// counted on no longer.
    async fn light_tick(&mut self) {
        let now = Instant::now();
        self.light_ticked = now;
        let ended = self.queues.later.iter();
        let ended = ended.take_while(|(until, _)| *until <= now);
        let ended: Vec<LevelChunk> = ended.map(|(_, position)| *position).collect();
        for position in ended {
            self.requeue(position);
        }
        let patience = self.config.light.job_patience;
        let late = |(basis, (thread, at)): (&u64, &(usize, Instant))| {
            (now.saturating_duration_since(*at) >= patience).then_some((*basis, *thread))
        };
        let late: Vec<(u64, usize)> = self.started.iter().filter_map(late).collect();
        for (basis, thread) in late {
            self.started.remove(&basis);
            let left = self.lighting.write_off(thread);
            error!(
                thread,
                left, "A thread that lights chunks has not answered and is counted on no longer"
            );
            let Some(position) = self.jobs.get(&basis).copied() else {
                continue;
            };
            let outcome = LightOutcome::Failed {
                why: "the job was not answered".to_owned(),
            };
            let failed = FromThread::Done {
                basis,
                position: position.position,
                outcome,
            };
            self.take_light(failed).await;
        }
    }

    /// Counts a failed job, and says so where an operator reads it, with how many
    /// have failed and how many chunks have light that is not right for it now: a
    /// line a second at most, whatever fails how often.
    fn say_failure(&mut self, position: LevelChunk, why: &str) {
        self.failed_jobs += 1;
        self.failed_unsaid += 1;
        let now = Instant::now();
        let said = |at: Instant| now.saturating_duration_since(at) < Duration::from_secs(1);
        if self.failures_said.is_some_and(said) {
            return;
        }
        let fallback = |entry: &&ReplicaChunk| entry.kind == LightKind::Fallback;
        error!(
            ?position,
            why,
            since_last_said = self.failed_unsaid,
            failed = self.failed_jobs,
            chunks_with_fallback_light = self.replica.values().filter(fallback).count(),
            "A chunk's light could not be made"
        );
        self.failed_unsaid = 0;
        self.failures_said = Some(now);
    }

    /// Makes jobs for marked entries while a slot is free, in the order
    /// [`LightQueues::next`] gives, and takes what the threads have said meanwhile.
    /// In the edge's tests a job is done where it is dispatched and nothing is held
    /// or rests, so everything a turn marks is lit before the turn ends.
    async fn light_the_marked(&mut self, lit: &mut mpsc::UnboundedReceiver<FromThread>) {
        loop {
            let mut moved = false;
            while self.jobs.len() < self.lighting.in_flight() {
                let overdue = self.config.light.overdue;
                let Some(position) = self.queues.next(Instant::now(), overdue) else {
                    break;
                };
                moved = true;
                self.dispatch(position).await;
            }
            while let Ok(said) = lit.try_recv() {
                moved = true;
                self.take_light(said).await;
            }
            if !moved {
                return;
            }
        }
    }

    /// Hands the threads a job for the entry at `position`: the chunk and the eight
    /// around it as they are now, which nobody changes while the job has them.
    async fn dispatch(&mut self, position: LevelChunk) {
        let basis = self.next_moment();
        let chunk_at = |at: LevelChunk| self.replica.get(&at).and_then(|entry| entry.chunk.clone());
        let [nw, n, ne, w, e, sw, south, se] = Self::around(position).map(chunk_at);
        let entry = self.replica.get_mut(&position).expect("a marked entry");
        entry.job = Some(basis);
        entry.dispatched = Some(Instant::now());
        // The job answers for everything up to here. What marks the entry under it
        // is new, and waits for the rest again.
        entry.marked_since = None;
        entry.exempt = false;
        self.queues.take_out(position, entry);
        let job = LightJob {
            basis,
            position: position.position,
            nine: [nw, n, ne, w, entry.chunk.clone(), e, sw, south, se],
            // The Nether has no sky, and its sections no sky light.
            sky: position.dimension.rules().has_skylight,
            old: entry.light.clone(),
            // Those who are owed the light hold one from before the entry's last:
            // the sections that differ from the entry's would not be all of theirs.
            whole: entry.owed,
        };
        self.jobs.insert(basis, position);
        figures::count(&FIGURES.jobs);
        if self.lighting.send(job).is_err() {
            // No thread is left to take it: it fails here, in this turn, and the
            // chunk gets what light there is.
            let outcome = LightOutcome::Failed {
                why: "no thread is left to light chunks".to_owned(),
            };
            let failed = FromThread::Done {
                basis,
                position: position.position,
                outcome,
            };
            self.take_light(failed).await;
        }
    }

    /// Takes what a thread says. A result is taken if its job is still the entry's:
    /// one for a chunk that was replaced, has left, or left and came again is dropped.
    async fn take_light(&mut self, said: FromThread) {
        let FromThread::Done { basis, outcome, .. } = said else {
            // From here the job's time runs: a thread has it.
            if let FromThread::Started { basis, thread } = said
                && self.jobs.contains_key(&basis)
            {
                self.started.insert(basis, (thread, Instant::now()));
            }
            return;
        };
        self.started.remove(&basis);
        // The job's chunk, of the dimension it was dispatched in: the thread names the
        // position alone.
        let Some(position) = self.jobs.remove(&basis) else {
            return;
        };
        let Some(entry) = self.replica.get_mut(&position) else {
            return;
        };
        if entry.job != Some(basis) {
            return;
        }
        entry.job = None;
        entry.lit = basis;
        entry.packet = None;
        figures::count(&FIGURES.taken);
        // A block that light sees changed under the job, in the chunk or within reach
        // of it: the block update went at once and the client lit the change itself.
        // This light, made from the blocks of before, would put that out again, so
        // those who have the chunk are not sent it and get the next job's, whole.
        let withheld = entry.changed > basis;
        let mut failed = None;
        let update = match outcome {
            LightOutcome::Lit { light, update } => {
                entry.light = Some(light);
                entry.kind = LightKind::Computed;
                update.filter(|_| !withheld)
            }
            LightOutcome::Failed { why } => {
                // The chunk is shown with the light it had, or lit by columns as
                // chunks were before the edge made light.
                figures::count(&FIGURES.failed);
                failed = Some(why);
                entry.kind = LightKind::Fallback;
                if entry.light.is_none() {
                    let chunk = entry.chunk.as_deref().expect("a job is for a chunk");
                    entry.light = Some(Arc::new(reckoned_light(chunk)));
                }
                None
            }
        };
        let now = Instant::now();
        if entry.sendable() {
            entry.unsendable_since = None;
        }
        if entry.current() {
            entry.marked_since = None;
            self.marked.remove(&position);
        } else {
            entry.marked_since.get_or_insert(now);
            // Those who have the chunk were kept waiting for this light and get
            // the next: that job does not rest first, once in a rest.
            let rest = self.config.light.rest;
            let lately = |at: Instant| now.saturating_duration_since(at) < rest;
            if withheld && !entry.exempted.is_some_and(lately) {
                entry.exempt = true;
                entry.exempted = Some(now);
            }
        }
        self.requeue(position);
        if let Some(why) = failed {
            self.say_failure(position, &why);
        }
        // To whom the light goes, of those who have the chunk
        // (`docs/adr/0023-light-at-the-edge.md`, "Found while building"): to everyone,
        // if a chunk came or was replaced beside it, which no client knows of by
        // itself; otherwise to those at the rim of whose view the chunk is, who lack
        // a chunk beside it. Whoever has all the chunks around a change lights it as
        // the official client always has, and the official server sends them nothing.
        let entry = &self.replica[&position];
        let everyone = entry.reshaped;
        let lacks_one_beside =
            |view: &PlayerView| !Self::around(position).iter().all(|at| view.holds(*at));
        let owed_it = |(player, view): (&PlayerId, &PlayerView)| {
            let has = view.was_sent(position);
            (has && (everyone || lacks_one_beside(view))).then_some(*player)
        };
        let holders: Vec<PlayerId> = self.players.iter().filter_map(owed_it).collect();
        let entry = self.replica.get_mut(&position).expect("found above");
        let sent = entry.kind == LightKind::Computed && !withheld;
        if sent {
            entry.owed = false;
            // Unless a chunk came beside it under the job as well: the next job's
            // result goes to everyone too.
            entry.reshaped = entry.shaped > basis;
        } else if !holders.is_empty() || entry.kind == LightKind::Fallback {
            // Also after a job that failed while nobody had the chunk: whoever is
            // sent it with the light of its heights is to have every section of the
            // next.
            entry.owed = true;
        }
        let waiters: Vec<PlayerId> = if entry.sendable() {
            entry.waiters.iter().copied().collect()
        } else {
            Vec::new()
        };
        // Everything is as it is to be before anyone is sent anything, and whoever is
        // sent something is looked up again by what sends it: a player whose queue is
        // full is removed in the middle of this, and the entry with them if they were
        // its last viewer.
        if let Some(update) = update {
            for player in holders {
                self.send_to_player(player, [update.clone()]).await;
            }
        }
        for player in waiters {
            self.send_chunks(player).await;
        }
    }

    /// Applies a block change to the replica and tells everyone who has the chunk.
    /// Keeps a block entity's new data in the replica, so that the chunk's packet
    /// carries it from now on (`docs/adr/0026-redstone.md`, section 6). Whether a
    /// client is told of it at once is the game's to say, block by block.
    fn change_block_entity(&mut self, dimension: Dimension, position: BlockPos, data: Vec<u8>) {
        let Some(entry) = self
            .replica
            .get_mut(&LevelChunk::new(dimension, position.chunk()))
        else {
            return;
        };
        let Some(chunk) = &mut entry.chunk else {
            // The chunk itself is still on its way and will include the change.
            return;
        };
        let (x, z) = position.in_chunk();
        let at = InChunk::new(x, position.y, z);
        // Data the chunk refuses was refused to the region's own chunk too.
        if Arc::make_mut(chunk).set_block_entity_data(at, data).is_ok() {
            entry.packet = None;
        }
    }

    /// Sends the players who have the chunk of `position` the block entity there as it
    /// is now, where its type is one whose changes are sent (`encode::UPDATED`): the
    /// game's `sendBlockUpdated` of a spawner whose next spawn was set.
    async fn show_block_entity(&mut self, dimension: Dimension, position: BlockPos) {
        let chunk = LevelChunk::new(dimension, position.chunk());
        let (x, z) = position.in_chunk();
        let packet = self
            .replica
            .get(&chunk)
            .and_then(|entry| entry.chunk.as_ref())
            .and_then(|kept| {
                kept.block_entities()
                    .get(&InChunk::new(x, position.y, z))
                    .and_then(|entity| crate::encode::block_entity_update(position, entity))
            });
        let Some(packet) = packet else {
            return;
        };
        let packet = encoded(&packet);
        let viewers: Vec<_> = self
            .players
            .iter()
            .filter(|(_, view)| view.holds(chunk))
            .map(|(player, _)| *player)
            .collect();
        for player in viewers {
            self.send_to_player(player, [packet.clone()]).await;
        }
    }

    /// The update that shows a player the block at `position` after their use of an
    /// item on a block, which the region read as `state` right after the use. The game
    /// sends it while it handles the use, before the changes of the tick go out at its
    /// end; the edge hears it after them, and a later action of the same tick may have
    /// changed the block again. So it is shown as the edge has it, which is what every
    /// client of the chunk is told, and as the region read it only where the edge does
    /// not have the chunk.
    ///
    /// But a block whose change waits to go out with the next tick's (one made after the
    /// tick's block events began: an entity's, as a mob's press of the plate it was let
    /// out onto by the egg of this very click) is shown as the region read it: the game
    /// sends this update before its entities tick, and the change after it, a tick later
    /// (`ServerGamePacketListenerImpl.handleUseItemOn`, then `ChunkHolder.broadcastChanges`
    /// in the next tick). Shown as the edge has it, the client would be told the change
    /// twice and a tick early.
    fn shown_block(&self, dimension: Dimension, position: BlockPos, state: BlockState) -> Bytes {
        let (x, z) = position.in_chunk();
        let chunk = LevelChunk::new(dimension, position.chunk());
        let section = (chunk, position.y.div_euclid(16));
        let waiting = self.carried.values().any(|changed| {
            changed
                .get(&section)
                .is_some_and(|blocks| blocks.contains_key(&position))
        });
        let state = if waiting {
            state
        } else {
            self.replica
                .get(&chunk)
                .and_then(|entry| entry.chunk.as_ref())
                .and_then(|chunk| chunk.get(x, position.y, z))
                .unwrap_or(state)
        };
        encoded(&BlockUpdate {
            position: Position {
                x: position.x,
                y: position.y,
                z: position.z,
            },
            state: i32::from(state.0),
        })
    }

    fn change_block(
        &mut self,
        dimension: Dimension,
        position: BlockPos,
        state: BlockState,
    ) -> bool {
        let chunk_position = LevelChunk::new(dimension, position.chunk());
        let Some(entry) = self.replica.get_mut(&chunk_position) else {
            return false;
        };
        let Some(chunk) = &mut entry.chunk else {
            // The chunk itself is still on its way and will include the change.
            return false;
        };
        let (x, z) = position.in_chunk();
        // A change that puts the state that is there changes nothing: two regions can
        // say the same change. The client is told all the same, as ever.
        let before = chunk.get(x, position.y, z);
        // Whether light sees the change, and if so whether it reaches all around:
        // by making the first block of a section or taking the last.
        let mut seen = None;
        if let Some(before) = before.filter(|before| *before != state) {
            let section = (position.y - chunk.min_y()) as usize / 16;
            let was_empty = chunk.sections()[section].non_air_count() == 0;
            Arc::make_mut(chunk).set(x, position.y, z, state);
            let is_empty = chunk.sections()[section].non_air_count() == 0;
            entry.packet = None;
            if light_tells_apart(before, state) {
                entry.as_made = None;
                seen = Some(was_empty != is_empty);
            }
        }
        if let Some(everywhere) = seen {
            // The light of the chunk and of those the change reaches was made for
            // other blocks. It may still be sent, a little old; each gets a job.
            let moment = self.next_moment();
            let now = Instant::now();
            let reached = clustine_light::reach(x, z);
            let around = Self::around(chunk_position);
            let reached = (0..8).filter(|index| everywhere || reached[*index]);
            for at in reached.map(|index| around[index]).chain([chunk_position]) {
                if let Some(entry) = self.replica.get_mut(&at)
                    && entry.chunk.is_some()
                {
                    entry.changed = moment;
                    entry.marked_since.get_or_insert(now);
                    self.marked.insert(at);
                    figures::count(&FIGURES.relit);
                    self.requeue(at);
                }
            }
        }

        true
    }

    /// Tells the players who have been sent a chunk of the changes `changed` made to its
    /// blocks: a single change of a section as a block update, several as one update of
    /// the section. Players who have not been sent the chunk yet get it with the changes
    /// in it.
    async fn show_changes(&mut self, changed: Changed) {
        for ((chunk, section), blocks) in changed {
            let chunk_position = chunk.position;
            let packet = if let [(position, state)] = blocks.iter().collect::<Vec<_>>()[..] {
                encoded(&BlockUpdate {
                    position: Position {
                        x: position.x,
                        y: position.y,
                        z: position.z,
                    },
                    state: state.0.into(),
                })
            } else {
                let at = |coordinate: i32| coordinate.rem_euclid(16) as u8;
                encoded(&SectionBlocksUpdate {
                    section: (chunk_position.x, section, chunk_position.z),
                    blocks: blocks
                        .iter()
                        .map(|(position, state)| {
                            let place = (at(position.x), at(position.y), at(position.z));
                            (place, i32::from(state.0))
                        })
                        .collect(),
                })
            };
            let viewers: Vec<_> = self
                .players
                .iter()
                // Also one whose client still shows the chunk of before.
                .filter(|(_, view)| view.holds(chunk))
                .map(|(player, _)| *player)
                .collect();
            for player in viewers {
                self.send_to_player(player, [packet.clone()]).await;
            }
        }
    }

    /// Shows the level event `event` at `position` (a block breaking, with its sound and
    /// bits; lava's hiss) to the players near it who have its chunk, but `except`, the
    /// one who broke the block, whose client shows it already: the game sends a level
    /// event to players within 64 blocks (`PlayerList.broadcast`).
    async fn show_near(
        &mut self,
        dimension: Dimension,
        position: BlockPos,
        event: i32,
        data: i32,
        except: Option<PlayerId>,
    ) {
        let packet = encoded(&LevelEvent {
            event,
            position: Position {
                x: position.x,
                y: position.y,
                z: position.z,
            },
            data,
            global: false,
        });
        let at = block_centre(position);
        let chunk = LevelChunk::new(dimension, position.chunk());
        self.send_near(chunk, at, LEVEL_EVENTS_WITHIN, except, packet)
            .await;
    }

    /// `ServerLevel.destroyBlockProgress` (n5 @0-144): the breaker's cracks in the block
    /// at `position`, the stage written as a byte, to the players whose feet are less
    /// than 32 blocks from the block's corner, but the breaker. Named: as the edge's other
    /// events of blocks, only to those who have the block's chunk, where the game sends
    /// to every player of the level that near.
    async fn show_destruction(
        &mut self,
        dimension: Dimension,
        breaker: EntityId,
        position: BlockPos,
        stage: i32,
    ) {
        let packet = encoded(&BlockDestruction {
            entity_id: self.wire.of(breaker),
            position: Position {
                x: position.x,
                y: position.y,
                z: position.z,
            },
            // `writeByte`: the low eight bits.
            progress: stage as u8,
        });
        let corner = (
            f64::from(position.x),
            f64::from(position.y),
            f64::from(position.z),
        );
        let chunk = LevelChunk::new(dimension, position.chunk());
        let except = self
            .players
            .iter()
            .find(|(_, view)| view.entity == Some(breaker))
            .map(|(player, _)| *player);
        self.send_near(chunk, corner, DESTRUCTION_WITHIN, except, packet)
            .await;
    }

    /// Sends `packet` to the players who have the chunk `chunk` and stand within
    /// `range` of `at`, but `except`.
    async fn send_near(
        &mut self,
        chunk: LevelChunk,
        at: (f64, f64, f64),
        range: f64,
        except: Option<PlayerId>,
        packet: Bytes,
    ) {
        let (x, y, z) = at;
        let near: Vec<_> = self
            .players
            .iter()
            .filter(|(player, view)| {
                Some(**player) != except
                    && view.holds(chunk)
                    && view
                        .entity
                        .and_then(|entity| self.entities.get(&entity))
                        .is_some_and(|shown| {
                            let at = shown.state.pose.position;
                            let (dx, dy, dz) = (at.x - x, at.y - y, at.z - z);
                            dx * dx + dy * dy + dz * dz < range * range
                        })
            })
            .map(|(player, _)| *player)
            .collect();
        for player in near {
            self.send_to_player(player, [packet.clone()]).await;
        }
    }

    /// Sends the next batch of pending chunks, nearest first, unless the client still
    /// has to confirm the previous batch. Returns false if the player was removed.
    async fn send_chunks(&mut self, player: PlayerId) -> bool {
        let Some(view) = self.players.get_mut(&player) else {
            return true;
        };
        // A travelling client is sent nothing of its new level before it is placed
        // there (`docs/groundwork/p-plan.md`, section 4.2).
        if view.batch_outstanding || view.pending.is_empty() || view.travelling.is_some() {
            return true;
        }
        let center = view.center;
        let dimension = view.dimension;
        // Only chunks that may be sent: those with light as recent as the chunks it
        // was made from. The others wait for their light, and nothing is sent for a
        // player who has none to be sent: no start and no end of a batch.
        let sendable = |position: &&ChunkPos| {
            let entry = self.replica.get(&LevelChunk::new(dimension, **position));
            entry.is_some_and(ReplicaChunk::sendable)
        };
        let mut batch: Vec<_> = view.pending.iter().filter(sendable).copied().collect();
        if batch.is_empty() {
            return true;
        }
        batch.sort_by_key(|position| {
            let (dx, dz) = (
                i64::from(position.x - center.x),
                i64::from(position.z - center.z),
            );
            dx * dx + dz * dz
        });
        batch.truncate(view.batch_size);

        // A chunk the client has from before is unloaded immediately before the batch
        // that carries it anew.
        let mut packets: Vec<_> = batch
            .iter()
            .filter(|position| view.replace.remove(position))
            .map(|position| {
                encoded(&UnloadChunk {
                    chunk_x: position.x,
                    chunk_z: position.z,
                })
            })
            .collect();
        packets.push(encoded(&ChunkBatchStart));
        for position in &batch {
            view.pending.remove(position);
            view.sent.insert(*position);
            let chunk = self.replica.get_mut(&LevelChunk::new(dimension, *position));
            let chunk = chunk.expect("pending chunks are in the replica");
            let packet = chunk.packet(*position);
            packets.push(packet.expect("a pending chunk has come"));
            if let Some(came) = chunk.came.take() {
                figures::sent_after(came);
            }
            chunk.waits_no_more(player);
            // Sent with light that is older than its blocks: this client was not told
            // of the block that changed, and so has not lit it. The light of the job
            // that is owed to the entry goes to them wherever the chunk is in their
            // view.
            if !chunk.current() {
                chunk.reshaped = true;
            }
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
                figures::count(&FIGURES.dropped);
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
    /// region of its dimension.
    fn unwant(&mut self, region: RegionId, chunk: ChunkPos) {
        let at = self.level_of(region, chunk);
        self.changed = true;
        let watched = self.replica.get(&at).is_some_and(|entry| entry.viewers > 0);
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
            // Only the regions of the chunk's own dimension: a region of another
            // dimension that has a chunk of the same position has another chunk.
            let everywhere: Vec<_> = self
                .regions
                .iter()
                .filter(|(_, port)| port.subscriptions.contains_key(&chunk))
                .map(|(region, _)| *region)
                .filter(|region| self.level_of(*region, chunk) == at)
                .collect();
            for region in everywhere {
                self.end(region, chunk);
            }
        }
    }

    /// Ends the edge's subscription to `chunk` at `region`.
    fn end(&mut self, region: RegionId, chunk: ChunkPos) {
        let at = self.level_of(region, chunk);
        let port = self.regions.entry(region).or_default();
        if port.subscriptions.remove(&chunk).is_some() {
            self.asking
                .push((region, Asking::Unsubscribe, chunk, false));
        }
        if let Some(entry) = self.replica.get_mut(&at)
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
        let at = self.level_of(region, chunk);
        let port = self.regions.entry(region).or_default();
        let Some(subscription) = port.subscriptions.get_mut(&chunk) else {
            return false;
        };
        if ask < subscription.begun {
            return false;
        }
        subscription.condition = Condition::Served;
        subscription.again_due = false;
        if let Some(entry) = self.replica.get_mut(&at) {
            entry.served_by = Some(region);
        }
        true
    }

    /// The region `from` says that `holder` holds `chunk`, in answer to a viewer's
    /// subscription: the edge asks there as a guest, unless it is subscribed there
    /// already. The viewer's subscription stays, as it is why `from` goes on knowing
    /// who holds the chunk.
    async fn elsewhere(&mut self, from: RegionId, chunk: ChunkPos, ask: u64, holder: RegionId) {
        let at = self.level_of(from, chunk);
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
        if let Some(entry) = self.replica.get_mut(&at)
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
        let at = self.level_of(from, chunk);
        self.changed = true;
        let port = self.regions.entry(from).or_default();
        let current = port.subscriptions.get(&chunk).is_some_and(|subscription| {
            subscription.kind == Kind::Guest && ask == subscription.ask
        });
        if !current {
            return;
        }
        port.subscriptions.remove(&chunk);
        if let Some(entry) = self.replica.get_mut(&at)
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
                entity_ids: vec![self.wire.of(entity)],
            });
            for other in self.players.values_mut() {
                if other.visible.remove(&entity).is_some() {
                    // See below for why this is not `send_to_player`.
                    let _ = other.outbound.try_send(packet.clone());
                }
            }
            self.release_wire(entity);
            let ended = self.ended.entry(player).or_insert(entity);
            *ended = (*ended).max(entity);
        }
        // The entry in the player list is this edge's to remove. A client that shows
        // another entity of the player keeps it: that is a later stay, through another
        // edge, and nothing would list the player again until that entity left the
        // client's view and came back (`docs/adr/0020-one-stay-per-player.md`,
        // section 6).
        let listing: Vec<_> = self
            .players
            .iter_mut()
            .filter(|(_, view)| !view.visible.values().any(|shown| *shown == Some(player)))
            .filter_map(|(other, view)| view.listed.remove(&player).map(|_| *other))
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
        // Done before anything is awaited: this is entered from inside every send to
        // a player whose queue is full, in the middle of whatever was being fanned
        // out (`docs/adr/0023-light-at-the-edge.md`, section 4, rule R1).
        for position in &view.pending {
            if let Some(chunk) = self.replica.get_mut(&view.level(*position)) {
                chunk.waits_no_more(player);
            }
        }
        for position in &view.wanted {
            let at = view.level(*position);
            let chunk = self
                .replica
                .get_mut(&at)
                .expect("viewed chunks are in the replica");
            chunk.viewers -= 1;
            if chunk.viewers == 0 {
                self.queues.take_out(at, chunk);
                self.replica.remove(&at);
                self.marked.remove(&at);
            }
        }
        // Those that keep viewers and have lost a waiter may belong in another set.
        for position in &view.pending {
            self.requeue(view.level(*position));
        }
        self.forget_unwatched_entities();
        for position in &view.wanted {
            self.unwant(view.region, *position);
        }
        self.flush_asking().await;
        // If that region has just let the player go, it ignores this, and the message
        // saying so, which is on its way, makes `hand_over` clean up. The leave names
        // the entity the player had, if the edge was told of one, and otherwise the
        // attempt of the connection, which is what the stay of its join carries
        // (`docs/adr/0020-one-stay-per-player.md`, section 4.4).
        let leave = EdgeToWorker::PlayerLeave {
            player,
            entity: view.entity,
            attempt: view.entity.is_none().then_some(view.session.0),
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
    /// `chunk` of the dimension the client shows.
    fn level(&self, chunk: ChunkPos) -> LevelChunk {
        LevelChunk::new(self.dimension, chunk)
    }

    /// Whether the client was sent `at`, and has not been told to replace it.
    fn was_sent(&self, at: LevelChunk) -> bool {
        at.dimension == self.dimension && self.sent.contains(&at.position)
    }

    /// Whether the client shows `at`: sent, or sent before and waiting to be replaced.
    fn holds(&self, at: LevelChunk) -> bool {
        at.dimension == self.dimension
            && (self.sent.contains(&at.position) || self.replace.contains(&at.position))
    }

    /// Whether the client is to be sent `at`, which it waits for.
    fn has_pending(&self, at: LevelChunk) -> bool {
        at.dimension == self.dimension && self.pending.contains(&at.position)
    }

    /// Works out what the client has to be told about `state` and records it: the
    /// packets that show the entity if it came into view, move it if `moved`, or hide it
    /// if it left the view. `me` is the player whose view this is. A player is never
    /// shown their own entity; the client creates that itself. Nor are they shown an
    /// entity they were before: one who leaves while being handed over and joins again
    /// at once is a new entity, while the old one may still arrive in the region it
    /// was on its way to and be removed there a moment later. A client that is shown
    /// a player with its own UUID has two of itself.
    fn update_visibility(
        &mut self,
        me: PlayerId,
        shown: &Shown,
        moved: bool,
        wire: &mut WireIds,
    ) -> Vec<Bytes> {
        let state = &shown.state;
        let living = &shown.living;
        let pairing = shown.tracker.as_ref().map(Tracker::pairing);
        let in_world = self.entity.is_some();
        let own = self.entity == Some(state.entity)
            || matches!(&state.kind, EntityKind::Player { player: of, .. } if *of == me);
        let in_view = in_world
            && !own
            && self.travelling.is_none()
            && shown.dimension == self.dimension
            && self.wanted.contains(&state.chunk());
        let shown = self.visible.contains_key(&state.entity);
        match (in_view, shown) {
            (true, false) => {
                let mut packets = Vec::new();
                let EntityKind::Player {
                    player,
                    name,
                    game_mode,
                    ..
                } = &state.kind
                else {
                    self.visible.insert(state.entity, None);
                    // The owner's id, given now if it has none, so that an owner shown
                    // after its projectile is the one it names (`n9-plan.md`, section
                    // 10).
                    let owner = owner_of(state).map(|owner| wire.of(owner));
                    let id = wire.of(state.entity);
                    let mut packets = spawn_packets(
                        state,
                        id,
                        pairing.as_ref(),
                        owner.unwrap_or(0),
                        &mut |key| wire.of(key),
                    );
                    packets.extend(living_packets(living, wire.of(state.entity)));
                    return packets;
                };
                // A client takes a player to be one entity. Should word of an entity
                // the player was before still be around, it goes first.
                let outdated: Vec<_> = self
                    .visible
                    .iter()
                    .filter(|(_, shown)| **shown == Some(*player))
                    .map(|(entity, _)| wire.of(*entity))
                    .collect();
                if !outdated.is_empty() {
                    self.visible.retain(|_, shown| *shown != Some(*player));
                    packets.push(encoded(&RemoveEntities {
                        entity_ids: outdated,
                    }));
                }
                self.visible.insert(state.entity, Some(*player));
                match self.listed.insert(*player, *game_mode) {
                    None => {
                        let addition = [(*player, name.clone(), *game_mode)];
                        packets.push(encoded(&player_list_additions(&addition)));
                    }
                    // Listed before with another mode, which changed while the client
                    // did not see them.
                    Some(told) if told != *game_mode => {
                        packets.push(encoded(&game_mode_update(*player, *game_mode)));
                    }
                    Some(_) => {}
                }
                let id = wire.of(state.entity);
                packets.extend(spawn_packets(state, id, None, 0, &mut |key| wire.of(key)));
                // What they hold and wear, their data (shared flags, effect particles)
                // and their sleep, as the others were shown them.
                packets.extend(living_packets(living, id));
                packets
            }
            (true, true) if moved => move_packets(state, wire.of(state.entity)),
            (false, true) => {
                self.visible.remove(&state.entity);
                vec![encoded(&RemoveEntities {
                    entity_ids: vec![wire.of(state.entity)],
                })]
            }
            _ => Vec::new(),
        }
    }
}

/// `ClientboundUpdateRecipesPacket` as the game sends it at a join: the recipe property
/// sets and the stonecutter's selectable recipes (`RecipeManager.finalizeRecipeLoading`),
/// the same for every player, so made once.
fn update_recipes_packet() -> Bytes {
    static PACKET: std::sync::OnceLock<Bytes> = std::sync::OnceLock::new();
    PACKET
        .get_or_init(|| {
            use clustine_protocol::packets::play::{
                HolderSet, PropertySet, SlotDisplay, StonecutterRecipe, UpdateRecipes,
            };
            let property_sets = clustine_data::recipes::property_sets()
                .into_iter()
                .map(|(key, items)| PropertySet {
                    key: key.to_owned(),
                    items,
                })
                .collect();
            let stonecutter = clustine_data::recipes::stonecutter_entries()
                .into_iter()
                .map(|entry| StonecutterRecipe {
                    input: match entry.input.tag {
                        Some(tag) => HolderSet::Tag(tag.to_owned()),
                        None => HolderSet::Items(
                            entry
                                .input
                                .items
                                .iter()
                                .map(|item| i32::from(*item))
                                .collect(),
                        ),
                    },
                    display: SlotDisplay::ItemStack {
                        item: i32::from(entry.item),
                        count: i32::from(entry.count),
                    },
                })
                .collect();
            encoded(&UpdateRecipes {
                property_sets,
                stonecutter,
            })
        })
        .clone()
}

/// The packet that tells a client what one slot of its inventory holds now, after the
/// region changed it of its own accord (`PlayerEvent::InventorySlot`): the game's
/// `ClientboundSetPlayerInventoryPacket`, which names the slot as the inventory does.
fn inventory_slot_packet(slot: u8, stack: Option<ItemStack>) -> Bytes {
    encoded(&SetPlayerInventory {
        slot: i32::from(slot),
        stack: stack.as_ref().map(protocol_stack),
    })
}

/// A stack as the protocol writes it (without its components for now).
fn protocol_stack(stack: &ItemStack) -> protocol::ItemStack {
    crate::encode::wire_stack(stack)
}

/// Adds players to a client's player list, each with the id of their game mode.
fn player_list_additions(players: &[(PlayerId, String, u8)]) -> PlayerInfoUpdate {
    PlayerInfoUpdate {
        actions: player_info::ADD_PLAYER
            | player_info::UPDATE_GAME_MODE
            | player_info::UPDATE_LISTED,
        entries: players
            .iter()
            .map(|(player, name, mode)| PlayerInfoEntry {
                uuid: player.0,
                profile: Some((name.clone(), Vec::new())),
                game_mode: Some(i32::from(*mode)),
                listed: Some(true),
                ..PlayerInfoEntry::default()
            })
            .collect(),
    }
}

/// The player list's word that `player` is in the game mode of id `mode`
/// (`UPDATE_GAME_MODE`).
fn game_mode_update(player: PlayerId, mode: u8) -> PlayerInfoUpdate {
    PlayerInfoUpdate {
        actions: player_info::UPDATE_GAME_MODE,
        entries: vec![PlayerInfoEntry {
            uuid: player.0,
            game_mode: Some(i32::from(mode)),
            ..PlayerInfoEntry::default()
        }],
    }
}

/// The packets that make an entity appear on a client: for a player the entity and
/// where its head looks; for any other the entity, where its tracker sent it last and
/// with the motion and angles sent last, its head at 0 and, for a falling block, its
/// block as the spawn's data; and then the values of its data that are not the
/// defaults, if any (the game's `ServerEntity.sendPairingData`).
/// Whether `value` is the default of a player's data at `index` that the edge keeps
/// apart from the others: the standing pose, no skin's parts, the right hand
/// (`Entity.defineSynchedData`, `Avatar.defineSynchedData` 5–27).
fn is_default_of_player(index: u8, value: &EntityDataValue) -> bool {
    match (index, value) {
        (entity_data::POSE, EntityDataValue::Pose(ordinal)) => *ordinal == pose::STANDING,
        (entity_data::PLAYER_SKIN_PARTS, EntityDataValue::Byte(parts)) => *parts == 0,
        (entity_data::PLAYER_MAIN_HAND, EntityDataValue::Registered { id, .. }) => *id == 1,
        _ => false,
    }
}

/// The pose and sleeping position of a player lying in the bed whose head is `head`, or
/// standing again (`None`).
fn sleeping_data(id: i32, head: Option<Position>) -> SetEntityData {
    let pose = if head.is_some() {
        pose::SLEEPING
    } else {
        pose::STANDING
    };
    SetEntityData {
        entity_id: id,
        values: vec![
            (entity_data::POSE, EntityDataValue::Pose(pose)),
            (
                entity_data::SLEEPING_POSITION,
                EntityDataValue::OptionalBlockPos(head),
            ),
        ],
    }
}

/// A living entity's pairing beyond its spawn and data: its attributes, a sleeper's bed,
/// and what it wears.
fn living_packets(living: &LivingShown, id: i32) -> Vec<Bytes> {
    let mut packets = Vec::new();
    if !living.attributes.is_empty() {
        packets.push(encoded(&UpdateAttributes {
            entity_id: id,
            attributes: living.attributes.values().cloned().collect(),
        }));
    }
    if let Some(head) = living.sleeping {
        packets.push(encoded(&sleeping_data(id, Some(head))));
    }
    // A sleeper's pose and bed went with the sleep's data, above.
    let values: Vec<(u8, EntityDataValue)> = living
        .data
        .iter()
        .filter(|(index, _)| {
            living.sleeping.is_none()
                || ![entity_data::POSE, entity_data::SLEEPING_POSITION].contains(index)
        })
        .map(|(index, value)| (*index, value.clone()))
        .collect();
    if !values.is_empty() {
        packets.push(encoded(&SetEntityData {
            entity_id: id,
            values,
        }));
    }
    if !living.equipment.is_empty() {
        packets.push(encoded(&SetEquipment {
            entity_id: id,
            slots: living
                .equipment
                .iter()
                .map(|(slot, stack)| (*slot, Some(stack.clone())))
                .collect(),
        }));
    }
    packets
}

/// An attribute as a client is sent it (`UpdateAttributes`).
fn attribute_snapshot(shown: AttributeShown) -> AttributeSnapshot {
    AttributeSnapshot {
        attribute: i32::from(shown.attribute),
        base: shown.base,
        modifiers: shown
            .modifiers
            .into_iter()
            .map(|(id, amount, operation)| (id, amount, i32::from(operation)))
            .collect(),
    }
}

/// The owner of a projectile `state` shows, if it has one: a thrown item's, an arrow's
/// or a trident's, the llama's spit's, a rocket's, a fireball's or a shulker's bullet's
/// (each a `Projectile`, whose add packet carries its owner). A cloud and the fangs are not projectiles.
fn owner_of(state: &EntityState) -> Option<EntityId> {
    match &state.kind {
        EntityKind::Other { data, .. } => match data {
            EntityData::Thrown { owner, .. } => *owner,
            EntityData::Arrow(arrow) => arrow.owner,
            EntityData::Spit(spit) => spit.owner,
            EntityData::Firework(rocket) => rocket.owner,
            EntityData::Fireball(fireball) => fireball.owner.map(|owner| owner.key),
            EntityData::ShulkerBullet(bullet) => bullet.owner.map(|owner| owner.key),
            _ => None,
        },
        _ => None,
    }
}

/// The packets that show `state` to a client, under the wire id `id`; `owner` is the
/// wire id of a projectile's owner, which its spawn carries
/// (`Projectile.getAddEntityPacket`), 0 for none or one the clients are not shown.
fn spawn_packets(
    state: &EntityState,
    id: i32,
    pairing: Option<&Pairing>,
    owner: i32,
    ids: crate::tracking::WireIds<'_>,
) -> Vec<Bytes> {
    let position = state.pose.position;
    if let (
        EntityKind::Other {
            kind, uuid, data, ..
        },
        Some(pairing),
    ) = (&state.kind, pairing)
    {
        let [mut x, mut y, mut z] = pairing.position;
        let block = match data {
            EntityData::FallingBlock { state, .. } => i32::from(*state),
            // A frame's and a painting's spawn carries its block's corner and its
            // direction's 3D value (`ClientboundAddEntityPacket(entity, direction, pos)`),
            // from which the client works out its box and centre.
            EntityData::ItemFrame { attached, .. } | EntityData::Painting { attached, .. } => {
                [x, y, z] = [
                    f64::from(attached.block.x),
                    f64::from(attached.block.y),
                    f64::from(attached.block.z),
                ];
                i32::from(attached.direction)
            }
            // A knot's is its fence's corner and no data (`ClientboundAddEntityPacket(this,
            // 0, getPos())`); the client puts it at the block's middle itself.
            EntityData::LeashKnot { attached } => {
                [x, y, z] = [
                    f64::from(attached.block.x),
                    f64::from(attached.block.y),
                    f64::from(attached.block.z),
                ];
                0
            }
            EntityData::Thrown { .. }
            | EntityData::Arrow(_)
            | EntityData::Spit(_)
            | EntityData::Firework(_)
            | EntityData::Fireball(_)
            | EntityData::ShulkerBullet(_) => owner,
            // An end crystal's is nothing (`new ClientboundAddEntityPacket(this, entity)`
            // of `Entity.getAddEntityPacket`); its beam and base come in its data.
            EntityData::EndCrystal(_) => 0,
            // What N5's other kinds send here comes with the steps that bring them.
            _ => 0,
        };
        // A living entity's head is sent as it turns (`getYHeadRot`): an armour stand's
        // always faces its yaw, and a mob's is where it was sent last.
        let head_yaw = match data {
            EntityData::ArmorStand { .. } => pairing.yaw,
            _ => pairing.head_yaw,
        };
        let mut packets = vec![encoded(&SpawnEntity {
            entity_id: id,
            uuid: *uuid,
            kind: i32::from(*kind),
            x,
            y,
            z,
            velocity: pairing.motion,
            pitch: pairing.pitch,
            yaw: pairing.yaw,
            head_yaw,
            data: block,
        })];
        if !pairing.values.is_empty() {
            packets.push(encoded(&SetEntityData {
                entity_id: id,
                values: pairing.values.clone(),
            }));
        }
        // A living entity's pairing goes on with what it wears, every slot that is not
        // empty (`sendPairingData`). Its attributes (`UpdateAttributes`) are left out: a
        // stand's are the defaults a client gives it, which no stack Clustine has
        // changes.
        if !pairing.equipment.is_empty() {
            packets.push(encoded(&SetEquipment {
                entity_id: id,
                slots: pairing
                    .equipment
                    .iter()
                    .map(|(slot, stack)| (*slot, Some(stack.clone())))
                    .collect(),
            }));
        }
        return packets;
    }
    match &state.kind {
        EntityKind::Player { player, .. } => vec![
            encoded(&SpawnEntity {
                entity_id: id,
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
            head_rotation(state, id),
        ],
        EntityKind::Other {
            kind,
            uuid,
            motion,
            data,
            ..
        } => vec![
            encoded(&SpawnEntity {
                entity_id: id,
                uuid: *uuid,
                kind: i32::from(*kind),
                x: position.x,
                y: position.y,
                z: position.z,
                velocity: *motion,
                pitch: 0,
                yaw: angle(state.pose.yaw),
                head_yaw: 0,
                data: 0,
            }),
            encoded(&entity_data_packet(id, *kind, data, ids)),
        ],
    }
}

/// The data of an entity other than a player that a client is told with its spawn:
/// for an item, its stack (`ItemEntity.DATA_ITEM`); for an armour stand, the values of
/// its data that are not the defaults.
fn entity_data_packet(
    id: i32,
    kind: u16,
    data: &EntityData,
    ids: crate::tracking::WireIds<'_>,
) -> SetEntityData {
    match data {
        // An end crystal's beam and base where they are not the defaults.
        EntityData::ArmorStand { .. } | EntityData::Mob { .. } | EntityData::EndCrystal(_) => {
            SetEntityData {
                entity_id: id,
                values: crate::tracking::living_values(kind, data, ids)
                    .into_iter()
                    .filter(|(_, _, default)| !default)
                    .map(|(index, value, _)| (index, value))
                    .collect(),
            }
        }
        EntityData::Item { stack, .. } | EntityData::Thrown { stack, .. } => SetEntityData {
            entity_id: id,
            values: vec![(
                entity_data::ITEM_STACK,
                EntityDataValue::ItemStack(Some(protocol_stack(&stack.clone()))),
            )],
        },
        // `EyeOfEnder.DATA_ITEM_STACK`, always sent, as the tracker's pairing has it.
        EntityData::EyeOfEnder(eye) => SetEntityData {
            entity_id: id,
            values: vec![(
                entity_data::ITEM_STACK,
                EntityDataValue::ItemStack(Some(protocol_stack(&eye.item))),
            )],
        },
        // `Fireball.DATA_ITEM_STACK`, sent where it is not the fire charge it starts
        // with.
        EntityData::Fireball(fireball) => SetEntityData {
            entity_id: id,
            values: (fireball.item.item != clustine_data::items::FIRE_CHARGE)
                .then(|| {
                    (
                        entity_data::ITEM_STACK,
                        EntityDataValue::ItemStack(Some(protocol_stack(&fireball.item))),
                    )
                })
                .into_iter()
                .collect(),
        },
        EntityData::Minecart { .. } => SetEntityData {
            entity_id: id,
            values: crate::tracking::minecart_values(data),
        },
        EntityData::Boat { .. } => SetEntityData {
            entity_id: id,
            values: crate::tracking::boat_values(data),
        },
        // A falling block's start, its index 8, comes with step N4d; N5's other kinds'
        // data with the steps that bring them.
        _ => SetEntityData {
            entity_id: id,
            values: Vec::new(),
        },
    }
}

/// The packets that put an entity a client already shows where it is now: for a player
/// with where its head looks.
fn move_packets(state: &EntityState, id: i32) -> Vec<Bytes> {
    let position = state.pose.position;
    let mut packets = vec![encoded(&SyncEntityPosition {
        entity_id: id,
        path: PositionPath::Linear {
            x: position.x,
            y: position.y,
            z: position.z,
        },
        yaw: state.pose.yaw,
        pitch: state.pose.pitch,
        on_ground: state.pose.on_ground,
    })];
    if matches!(state.kind, EntityKind::Player { .. }) {
        packets.push(head_rotation(state, id));
    }
    packets
}

/// `set_passengers` of `vehicle` for the client of `view`, if it shows the vehicle or is
/// among its riders: the riders it shows, in order, its own player among them.
fn passengers_packet(
    view: &PlayerView,
    vehicle: EntityId,
    riders: &[EntityId],
    wire: &mut WireIds,
) -> Option<Bytes> {
    let own = |rider: &EntityId| view.entity == Some(*rider);
    if !view.visible.contains_key(&vehicle) && !riders.iter().any(own) {
        return None;
    }
    let passengers = riders
        .iter()
        .filter(|rider| own(rider) || view.visible.contains_key(rider))
        .map(|rider| wire.of(*rider))
        .collect();
    Some(encoded(&SetPassengers {
        vehicle_id: wire.of(vehicle),
        passengers,
    }))
}

/// A player's own region's word of what they ride, and whether the vehicle has named
/// them since: a word adds a player to a vehicle's seats only until it does, so that a
/// vehicle's own later word of their getting off is not undone by a word gone stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RidesWord {
    vehicle: Option<EntityId>,
    named: bool,
}

/// Whom each vehicle carries as clients are to be shown it: those the vehicle names, but
/// a player whose own region says they ride something else or nothing, and then the
/// players whose own region says they ride it and the vehicle does not name yet.
fn effective_passengers(
    passengers: &BTreeMap<EntityId, Vec<EntityId>>,
    words: &BTreeMap<EntityId, RidesWord>,
) -> BTreeMap<EntityId, Vec<EntityId>> {
    let mut lists: BTreeMap<EntityId, Vec<EntityId>> = passengers
        .iter()
        .map(|(vehicle, riders)| {
            let kept = riders
                .iter()
                .copied()
                .filter(|rider| {
                    words
                        .get(rider)
                        .is_none_or(|word| word.vehicle == Some(*vehicle))
                })
                .collect();
            (*vehicle, kept)
        })
        .collect();
    for (rider, word) in words {
        if let (Some(vehicle), false) = (word.vehicle, word.named) {
            let list = lists.entry(vehicle).or_default();
            if !list.contains(rider) {
                list.push(*rider);
            }
        }
    }
    lists
}

/// `set_entity_link` of the leashee `entity` to `holder`, 0 for none.
fn link_packet(entity: EntityId, holder: Option<EntityId>, wire: &mut WireIds) -> Bytes {
    encoded(&SetEntityLink {
        leashed_id: wire.of(entity),
        holder_id: holder.map_or(0, |holder| wire.of(holder)),
    })
}

/// The head of a player looks where the player looks; the position packets only turn
/// the body.
fn head_rotation(state: &EntityState, id: i32) -> Bytes {
    encoded(&SetHeadRotation {
        entity_id: id,
        head_yaw: angle(state.pose.yaw),
    })
}

fn encoded<P: Packet>(packet: &P) -> Bytes {
    packets::encode(packet).into()
}

/// The sentence of the game for a connection that a later login of the same player
/// has put out; each client shows it in its own language. The official server sends
/// this very component, which the comparisons hold it to.
const DUPLICATE_LOGIN: &str = "multiplayer.disconnect.duplicate_login";

/// Tells a client that a later login of its player has taken its place. Its
/// connection ends when `outbound` is dropped.
fn put_out(outbound: &mpsc::Sender<Bytes>) {
    let reason = Text::Translatable(DUPLICATE_LOGIN.to_owned());
    let _ = outbound.try_send(encoded(&Disconnect::from(reason)));
}

/// What a player is put into the world with.
struct Entering {
    entity: EntityId,
    pose: Pose,
    flying: bool,
    /// What they carry, which window 0 is sent whole with, and the hotbar slot they hold.
    inventory: Inventory,
    selected_slot: u8,
    /// What they ride, whose word is taken before their client is told anything.
    riding: Option<EntityId>,
    /// Where they last died, for the login (`docs/groundwork/p-plan.md`, section 19.9).
    last_death: Option<(Dimension, BlockPos)>,
    /// Their game mode (`docs/groundwork/v-plan.md`, section 4.1).
    mode: clustine_world::PlayerMode,
}

/// The light a chunk is shown with where none could be made for it: the sky light
/// reckoned by columns, with block light dark wherever sky light is said, as every
/// chunk was lit before the edge made light. It says of itself that no rules made it.
fn reckoned_light(chunk: &Chunk) -> Light {
    let sky = sky_light(chunk);
    let block = sky.iter().map(|section| match section {
        SectionLight::Absent => SectionLight::Absent,
        _ => SectionLight::Dark,
    });
    Light {
        made_by: 0,
        beside: 0,
        block: block.collect(),
        sky,
    }
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

/// The login of a player who enters the world in `dimension`: every dimension the
/// world has, and the one they are in.
fn login_packet(
    entity_id: i32,
    config: &FanoutConfig,
    dimension: Dimension,
    last_death: Option<(Dimension, BlockPos)>,
    mode: clustine_world::PlayerMode,
) -> Login {
    Login {
        entity_id,
        hardcore: false,
        dimension_names: Dimension::ALL
            .iter()
            .map(|dimension| dimension.type_name().to_owned())
            .collect(),
        max_players: config.max_players as i32,
        view_distance: config.view_distance,
        simulation_distance: config.view_distance,
        reduced_debug_info: false,
        enable_respawn_screen: true,
        limited_crafting: false,
        spawn: spawn_info(config, dimension, last_death, mode),
        online_mode: false,
        enforces_secure_chat: false,
    }
}

/// What a client is told of the level of `dimension` it enters, at its login and at a
/// change of dimension (`CommonPlayerSpawnInfo`; `docs/groundwork/p-brief.md`, section
/// 3.1).
fn spawn_info(
    config: &FanoutConfig,
    dimension: Dimension,
    last_death: Option<(Dimension, BlockPos)>,
    mode: clustine_world::PlayerMode,
) -> SpawnInfo {
    let dimension_type = synced_registry("minecraft:dimension_type")
        .and_then(|registry| registry.id_of(dimension.type_name()))
        .expect("the three dimensions are vanilla dimension types");
    SpawnInfo {
        dimension_type,
        dimension_name: dimension.type_name().to_owned(),
        // The client blurs the borders of biomes with it, as the server's surface did.
        hashed_seed: config.login.hashed_seed,
        game_mode: i32::from(mode.current.id()),
        previous_game_mode: mode.previous.map(|previous| i32::from(previous.id())),
        is_debug: false,
        // The world's, in every dimension, as the game's `ServerLevel.isFlat` is: the
        // flat world's Nether and End are those of seed 0
        // (`docs/groundwork/p-plan.md`, decision 9) and still said to be flat.
        is_flat: config.login.flat,
        death_location: last_death.map(|(level, block)| DeathLocation {
            dimension: level.type_name().to_owned(),
            position: Position {
                x: block.x,
                y: block.y,
                z: block.z,
            },
        }),
        portal_cooldown: 0,
        sea_level: match dimension {
            Dimension::Overworld => config.login.sea_level,
            // The noise settings' `sea_level` of the Nether and of the End.
            Dimension::Nether => 32,
            Dimension::End => 0,
        },
    }
}

#[cfg(test)]
#[path = "fanout/scenarios/mod.rs"]
mod scenarios;
#[cfg(test)]
#[path = "fanout/tests.rs"]
mod tests;
