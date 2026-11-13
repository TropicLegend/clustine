//! Worker service: ticks the regions it owns.
//!
//! A [`RegionRunner`] connects a region to the outside: it turns the messages of the
//! edges and the answers of the world store into tick inputs, runs the tick, hands what
//! changed to the store and, once the store has it on disk, publishes what resulted. The
//! simulation itself never waits for anything.
//!
//! A region is restored from what the world store has of it, and nothing a tick produced
//! is shown to anyone before the store has confirmed that tick. So whatever an edge was
//! told can be told again by another runner that carries on with the region. See
//! `docs/adr/0008-durable-regions-and-resuming.md`, section 4.
//!
//! Which chunks a region simulates is the world store's to say. The runner passes what
//! a tick claims and gives back on to the store, and the store's answers into the ticks
//! that follow. A link subscribes to chunks, for viewers of the region's own players or
//! as a guest for viewers of others', and is told of each either the chunk, if the
//! region holds it, or that it is elsewhere. See `docs/adr/0012-the-tick-on-chunks.md`,
//! section 4.
//!
//! Where a player enters the world, and which of two stays of a player is the later, is
//! the world store's to say as well. The runner hands the store what a tick says of
//! stays with the tick's commit, and passes the store's answers into the ticks that
//! follow: that a stay may enter and where, and that a stay is dead. See
//! `docs/adr/0020-one-stay-per-player.md`, sections 3 to 5.
//!
//! Edges come and go. Each has one link at a time, which begins with a hello. Players
//! belong to edges, not to links: a link that ends or does not keep up is dropped without
//! the region missing a tick, and its edge's players stay until the edge is back, has
//! started anew or has been away for too long.
//!
//! A region is given to another worker by **releasing** it, which is a crash that the
//! runner prepares: it brings the store up to date, lets go of the region and closes its
//! links, so that the next owner restores it from a state file alone. See
//! `docs/adr/0009-moving-a-region.md`, section 1, and [`RegionRunner::begin_release`].
//!
//! A region **absorbs** another, or a part of it is **split off** as a region of its
//! own, in one tick in which nothing else happens ([`RegionRunner::reshape`]). The
//! runner brings the store up to date as for a release, works the tick out aside and
//! hands it to the store, and takes it only when the store has it on disk. Then the
//! region and the runner begin anew, as after a restore but without leaving the
//! worker: every link is closed, and the edges link again and are told what happened
//! among the entries of a welcome. See `docs/adr/0014-merging-and-splitting.md`,
//! section 3.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::mem;
use std::ops::Bound;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use clustine_features::{Dimension, FeatureSettings};
use clustine_rpc::link::WorkerEnd;
use clustine_rpc::{
    Crowds, Decline, EdgeMessage, EdgeToWorker, Off, Parts, Presence, Restored, SplitPart,
    StoreReply, StoreRequest, Welcome, WorkerToEdge,
};
use clustine_sim::api::{Entered, FaceChange, FaceSeq, Inventory, PlayerInput, RegionEvent};
use clustine_sim::{
    Durable, EdgeEvent, Features, Holdings, Knowledge, LendAnswer, LendAsked, Misdirected, NoSplit,
    Part, PlayerChange, PlayerEvent, Region, RegionConfig, RegionState, RemoteStep, Sides,
    Splitting, StateDelta, TickInputs, Ticket,
};
use clustine_world::{
    BlockPos, Change, Chunk, ChunkPos, EdgeId, EntityId, PlayerId, RegionId, Vec3,
};
use clustine_worldstore::StoreHandle;
use tracing::{error, info, warn};

use crate::border::{Asks, Border};

mod border;

/// The length of a tick: 20 ticks per second.
pub const TICK: Duration = Duration::from_millis(50);

/// Ticks between two checkpoints unless set otherwise: five minutes.
pub const DEFAULT_CHECKPOINT_INTERVAL: u64 = 5 * 60 * 20;

/// Ticks an edge may be without a link before the region forgets it, unless set
/// otherwise: 30 seconds.
pub const DEFAULT_GONE_AFTER: u64 = 30 * 20;

/// Ticks a chunk a region holds outside its pinned areas may be without use before the
/// region gives it back, as the processes set it (`RegionConfig::return_after`): 30
/// seconds, which is the time after which an edge that stays away is gone. A region
/// that is restored has no tickets until its edges have said hello, and must not give
/// its chunks back in the meantime.
pub const DEFAULT_RETURN_AFTER: u64 = 30 * 20;

/// How many ticks a region may be ahead of what the world store has confirmed. At that
/// bound it waits.
pub const MAX_TICKS_AHEAD: usize = 8;

/// A runner that has fallen further behind than this many ticks skips them instead of
/// trying to catch up.
const MAX_CATCH_UP_TICKS: u32 = 10;

/// How often a runner that waits for its next tick looks whether the store has confirmed
/// a commit, so that what a tick did reaches players a write later and not a tick later.
const COMMIT_POLL: Duration = Duration::from_millis(1);

/// How long what the store is handed for a merge or a split may be: the merged state,
/// or the two states of a split and its chunks, counted at 16 bytes each. A request to
/// a store in another process is at most 16 MiB, and one that is longer loses the
/// handle, which is no answer to give a large region; so the runner sends nothing
/// longer than half of that, and says [`Off::TooLarge`] instead.
pub const MAX_RESHAPE_BYTES: usize = 8 * 1024 * 1024;

/// Ticks a runner keeps a chunk warm after the merge or the split it has it from: the
/// time after which an edge that stays away is gone. A chunk nobody has asked for by
/// then is not coming back to a screen soon.
const WARM_FOR: u64 = DEFAULT_GONE_AFTER;

/// How many chunks may have unsaved changes for a region that was told to prepare to
/// stop for a merge or a split without another checkpoint first. The checkpoint that is
/// made while the region stands still then saves that many chunks at most, which the
/// store writes in as many waits as it writes one, each a little longer: where a sync
/// took 20 ms, a round of eight files took 77 ms and a round of two 40
/// (`docs/groundwork/disk-syncs-measured.md`, section 2), and a checkpoint made
/// beforehand instead is six such waits. It is a number of chunks and not of ticks
/// since the last checkpoint, because the chunks are what players stand still for: a
/// crowd changes more of them in a second than a few players do in a minute.
const FEW_UNSAVED: usize = 8;

/// For how many ticks after a region was told to prepare its checkpoint counts as
/// made a moment ago, however many chunks have changed since. Where a crowd changes
/// two dozen chunks in every tick, "few chunks have changed" never holds, and a
/// checkpoint made while ticking leaves as many unsaved as it found: measured with a
/// hundred players on a disk of 22 ms a sync, a merge then made two such checkpoints
/// in turn, 130 and 190 ms, with the players of the absorbed region standing still
/// for both, and stopped with 25 chunks unsaved all the same. What has changed
/// within these ticks is a round of chunk files in the checkpoint made at the stop.
const FRESH_FOR: u64 = 40;

/// Counters of a region that others may read while its runner runs.
#[derive(Debug, Default)]
pub struct RegionStatus {
    /// The number of the last tick that ran. A region whose commits go unanswered stops
    /// ticking within [`MAX_TICKS_AHEAD`] ticks, so a number that goes up shows that the
    /// store confirms what the region does.
    pub tick: AtomicU64,
    /// How many players were in the region after that tick.
    pub players: AtomicU64,
    /// How many chunks were loaded after that tick.
    pub chunks: AtomicU64,
    /// How many chunks the region held after that tick by the world store's word: what
    /// the store told it it holds when it was opened or has granted it since, and it
    /// has not given back. A chunk of an area the region is pinned to counts from the
    /// store's answer to the region's claim of it, and the store does not name it when
    /// the region is opened: the count of a pinned region begins anew with every owner
    /// and rises as its edges ask for chunks.
    pub held: AtomicU64,
    /// How many players have come in from other regions.
    pub arrivals: AtomicU64,
    /// How many players have been let go to other regions.
    pub departures: AtomicU64,
    /// Whether the world store no longer does what the region asks of it: it cannot be
    /// reached, or it has given the region to another owner. The runner has stopped for
    /// good then; the region is to be opened again and restored.
    pub store_lost: AtomicBool,
    /// How the runner ended, once it has; read through [`RegionStatus::ended`].
    ended: AtomicU8,
    /// Where the players were after that tick; read through [`RegionStatus::crowds`].
    crowds: Mutex<Crowds>,
    /// Where a circuit worked in the last second; read through
    /// [`RegionStatus::redstone`].
    redstone: Mutex<Vec<(ChunkPos, RegionId)>>,
}

impl RegionStatus {
    /// The chunks with players in them after the last tick that ran, each with how
    /// many, in ascending order.
    pub fn crowds(&self) -> Crowds {
        // Whoever held the lock only ever replaced the whole list.
        let crowds = self.crowds.lock().unwrap_or_else(PoisonError::into_inner);
        crowds.clone()
    }

    /// The chunks in which a circuit's part changed in the last second, as a
    /// [`clustine_rpc::PlayersOf`] reports them (`docs/adr/0027-lending-at-a-line.md`,
    /// section 7).
    pub fn redstone(&self) -> Vec<(ChunkPos, RegionId)> {
        let redstone = self.redstone.lock().unwrap_or_else(PoisonError::into_inner);
        redstone.clone()
    }

    /// How the runner ended, or `None` while it runs or is still releasing the region.
    /// By the time this says so, the runner has closed its links and, unless the store
    /// handle was lost anyway, has let go of it; whoever waits for a release needs to
    /// look at nothing else.
    pub fn ended(&self) -> Option<Ended> {
        match self.ended.load(Ordering::SeqCst) {
            1 => Some(Ended::Stopped),
            2 => Some(Ended::StoreLost),
            3 => Some(Ended::Released),
            4 => Some(Ended::Abandoned),
            _ => None,
        }
    }

    fn set_ended(&self, ended: Ended) {
        let code = match ended {
            Ended::Stopped => 1,
            Ended::StoreLost => 2,
            Ended::Released => 3,
            Ended::Abandoned => 4,
        };
        self.ended.store(code, Ordering::SeqCst);
    }
}

/// Attaches links to a [`RegionRunner`] while it runs, from any thread.
#[derive(Debug, Clone)]
pub struct Links {
    attached: Sender<WorkerEnd>,
}

impl Links {
    /// Hands `link` to the runner, which serves it from its next tick on. If the runner
    /// is gone, has ended, or has stopped ticking in order to release its region, or to
    /// merge or split it, the link is closed instead, which its other end notices. An
    /// edge whose link a merge or a split closed is let in again as soon as that is
    /// through or off.
    pub fn attach(&self, link: WorkerEnd) {
        // Nobody is left to serve the link, and dropping it is what closes it.
        let _ = self.attached.send(link);
    }
}

/// Why a runner no longer runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// It was asked to stop, and the world store has everything.
    Stopped,
    /// The world store no longer does what the region asks of it. The region has to be
    /// opened again and restored from what the store has; this runner must not be used
    /// any more.
    StoreLost,
    /// It released the region: the store has the region's whole state and every changed
    /// chunk as of its last tick, everything that tick and those before it produced was
    /// published, the region is closed at the store and the links are closed. Another
    /// owner can open the region and restores it from a state file alone.
    Released,
    /// It was asked to stop while it was releasing the region, or in the middle of a
    /// merge or a split, and let go of the region without waiting for the store any
    /// longer. Nothing was published that the store had not confirmed, so this is a
    /// crash like any other: the next owner restores what the store has, which is the
    /// region before the merge or the split, or after it.
    Abandoned,
}

/// What a runner is told to do to its region besides ticking and releasing it. See
/// `docs/adr/0014-merging-and-splitting.md`, section 3.1.
#[derive(Debug, Clone, PartialEq)]
pub enum Reshape {
    /// Checkpoint now and tick on: a merge or a split is coming, and what is saved
    /// now is not saved while players stand still for it. Changes nothing else, and
    /// has no outcome. The merge or the split that comes next makes no checkpoint of
    /// its own before the region stops, if this one is done by then and few chunks
    /// have changed since.
    Prepare,
    /// Absorb the region `absorbed`, which this worker has open with `absorbed_epoch`
    /// and whose whole state is `state` ([`absorbable`]). The handle of that region is
    /// to stay open until the outcome is there: the store declines a merge whose
    /// absorbed region has no owner with that epoch.
    Absorb {
        absorbed: RegionId,
        absorbed_epoch: u64,
        state: RegionState,
    },
    /// Split the players standing in `chunks` off as the region `part`, to be opened
    /// with `as_epoch`. If the store has another id next, the split is made with that.
    SplitOff {
        chunks: Vec<ChunkPos>,
        as_epoch: u64,
        part: RegionId,
    },
}

/// What came of a [`Reshape::Absorb`] or a [`Reshape::SplitOff`].
// One is made for a merge or a split and handed over once, so the room a part takes in
// every one of them costs nothing that a box would save.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum Reshaped {
    /// The region has absorbed `absorbed`, which is no more. Its links are closed.
    Absorbed { absorbed: RegionId },
    /// The region `region` has been split off, and is to be opened with `as_epoch` and
    /// run from `part` ([`RegionRunner::of_part`]). The links of the region that was
    /// split are closed.
    Split {
        region: RegionId,
        as_epoch: u64,
        part: Part,
    },
    /// Nothing came of it, and the region is as it was. [`Off::StoreLost`] alone
    /// leaves open whether the store has the record: the runner lost its handle, or
    /// was stopped or dropped, before it had the store's answer.
    Off { why: Off },
}

/// How far a runner is with a release, a merge or a split; see
/// [`RegionRunner::stage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// A checkpoint and a flush have been asked for: for this, or by a
    /// [`Reshape::Prepare`] before it. The region ticks and serves its links until
    /// the flush is answered.
    Preparing,
    /// The region ticks no more and takes nothing from its links, and links attached
    /// now are closed. The last checkpoint and a flush have been asked for. The ticks
    /// that ran are published as their commits are confirmed.
    Settling,
    /// Every tick that ran is published, and the flush behind the last checkpoint is
    /// waited for.
    Closing,
    /// Only for a merge or a split: the store has been handed it, and has not answered.
    Committing,
}

/// How far a runner is with releasing its region, or with merging or splitting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Nobody has asked for anything of the kind, or what was asked is done or off.
    Running,
    /// The first checkpoint and a flush have been asked for, or were by a
    /// [`Reshape::Prepare`] before. The region ticks on and serves its links until the
    /// store has answered the flush, so that the checkpoint that is taken while
    /// players stand still is a small one.
    Preparing,
    /// The region ticks no more and takes nothing from its links. The last checkpoint
    /// and a flush were asked for as it stopped. It waits for the store to confirm the
    /// commits of the ticks that have run, and publishes them.
    Settling,
    /// Every tick that ran is published. The flush behind the last checkpoint is
    /// waited for.
    Closing,
    /// The store has been handed a merge or a split, and its answer is waited for.
    Committing,
    /// The runner has ended, for whichever reason, and does nothing but close the links
    /// it is handed.
    Ended,
}

/// The call that takes the outcome of a merge or a split. It is made once, whatever
/// becomes of the runner: if this is dropped before there is an outcome, the outcome is
/// that nothing came of it, for the reason `otherwise`.
struct Done {
    call: Option<Box<dyn FnOnce(Reshaped) + Send>>,
    otherwise: Off,
}

impl Done {
    /// For a command that no runner has taken up yet. A runner that never does has
    /// ended, and is busy with nothing else any more.
    fn new(call: Box<dyn FnOnce(Reshaped) + Send>) -> Self {
        Self {
            call: Some(call),
            otherwise: Off::Busy,
        }
    }

    /// For a command that has no outcome.
    fn never() -> Self {
        Self {
            call: None,
            otherwise: Off::Busy,
        }
    }

    fn call(mut self, outcome: Reshaped) {
        if let Some(call) = self.call.take() {
            call(outcome);
        }
    }

    fn off(self, why: Off) {
        self.call(Reshaped::Off { why });
    }
}

impl Drop for Done {
    fn drop(&mut self) {
        if let Some(call) = self.call.take() {
            call(Reshaped::Off {
                why: self.otherwise,
            });
        }
    }
}

/// A [`Reshape`] on its way to a runner's thread, with the call for its outcome.
struct Command {
    reshape: Reshape,
    done: Done,
}

impl Command {
    fn new(reshape: Reshape, done: Box<dyn FnOnce(Reshaped) + Send>) -> Self {
        let done = match reshape {
            // Dropped uncalled, here and now.
            Reshape::Prepare => Done::never(),
            Reshape::Absorb { .. } | Reshape::SplitOff { .. } => Done::new(done),
        };
        Self { reshape, done }
    }
}

/// What another thread hands a runner's thread, which waits for it between two ticks.
enum Order {
    /// A command to take.
    Reshape(Box<Command>),
    /// A release has been asked for, which [`RegionRunner::release_asked`] says: this
    /// only has the thread look now and not at its next tick.
    Release,
}

/// A merge or a split that a runner is in the middle of.
struct Reshaping {
    plan: Plan,
    done: Done,
}

/// What a runner that is merging or splitting its region was told, and what it has
/// handed the store, once it has.
enum Plan {
    Absorb {
        absorbed: RegionId,
        absorbed_epoch: u64,
        /// The whole state of the region to absorb.
        other: RegionState,
        /// The merged state the store was handed, while its answer is waited for.
        state: Option<RegionState>,
    },
    Split {
        /// The chunks named, whose players go.
        named: Vec<ChunkPos>,
        as_epoch: u64,
        /// The id of the new region: the one the command named, or the one the store
        /// has said is next.
        part: RegionId,
        /// Whether the store has declined the split once for its id, and named the
        /// next. Another split can have taken the one the command named; a second
        /// time is a decline like any other.
        renamed: bool,
        /// The split the store was handed, while its answer is waited for.
        splitting: Option<Splitting>,
    },
}

/// A chunk a runner has in memory that its region holds and has not loaded, known to
/// be what the store has; see [`RegionRunner::warm`].
struct Warm {
    chunk: Chunk,
    /// The tick of the merge or the split the chunk is kept from.
    since: u64,
    /// The chunk's light as the region handed it on then, which the region adopts when
    /// the chunk comes back, its blocks unchanged since (ADR-0029, section 7).
    light: Option<clustine_sim::KeptLight>,
}

/// How long a region did not tick for a merge or a split, and what it was then. See
/// [`RegionRunner::with_standstills`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Standstill {
    /// The players the region had after its last tick before it stopped: those who
    /// stood still in it.
    pub players: u64,
    /// The chunks it held then by the world store's word, as [`RegionStatus::held`].
    pub held: u64,
    /// From when it stopped ticking until its next tick had run.
    pub milliseconds: u64,
}

/// When a region stopped ticking for a merge or a split, and what it was then: what a
/// [`Standstill`] is made of once the region has ticked on.
struct Stopped {
    at: Instant,
    players: u64,
    held: u64,
}

/// What a runner that has let go of its region has in place of a store handle. Putting
/// it there drops the handle, which is what closes the region at the store.
struct Closed;

impl RegionStore for Closed {
    fn request(&self, _: StoreRequest) {}

    fn try_reply(&self) -> Option<StoreReply> {
        None
    }

    fn is_lost(&self) -> bool {
        false
    }

    fn flush(&self) {}
}

/// Why a region could not be restored from what the world store has of it.
#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    #[error("the stored state of the region as of tick {tick} cannot be read: {error}")]
    State { tick: u64, error: postcard::Error },
    #[error("the stored change of the region's state in tick {tick} cannot be read: {error}")]
    Delta { tick: u64, error: postcard::Error },
    #[error(
        "what is stored of the region as of tick {tick} was written by a later build (format {format})"
    )]
    Format { tick: u64, format: u8 },
}

/// The number of the form in which a region's state and its changes are handed to the
/// world store. It is raised by every change to the shape of anything a `RegionState`
/// or a `StateDelta` contains: what was stored with a lower number, or before there
/// was one, cannot be read as what it is now, and postcard does not notice (it has no
/// names or kinds on the wire, so bytes of one shape can read as another).
/// `the_bytes_of_a_state_and_of_a_delta_are_as_written_down` fails when a shape
/// changes, and says so.
pub const STATE_FORMAT: u8 = 36;

/// What the store is handed for `value`, a `RegionState` or a `StateDelta`: a zero byte,
/// [`STATE_FORMAT`], and the value as postcard writes it. The zero tells it from what
/// was stored before there was a number: such bytes begin with the tick, which
/// postcard writes with a first byte of zero only for tick 0, and no state or delta of
/// tick 0 is ever stored.
fn stored<T: serde::Serialize>(value: &T) -> Vec<u8> {
    let bytes = vec![0, STATE_FORMAT];
    postcard::to_extend(value, bytes).expect("a region's state is made of what postcard can write")
}

/// What `bytes`, a state or a delta as the store has it for `tick`, are.
enum Stored<'a> {
    /// Of this build: the postcard of the value.
    Current(&'a [u8]),
    /// From before: written without a number, or with a lower one. It cannot be read.
    Before,
}

fn sort_stored(tick: u64, bytes: &[u8]) -> Result<Stored<'_>, RestoreError> {
    match bytes {
        [0, format, rest @ ..] if *format == STATE_FORMAT => Ok(Stored::Current(rest)),
        [0, format, ..] if *format > STATE_FORMAT => Err(RestoreError::Format {
            tick,
            format: *format,
        }),
        _ => Ok(Stored::Before),
    }
}

/// What a runner needs of the world store. A [`StoreHandle`] is what it is in earnest;
/// tests put something in between that holds answers back.
trait RegionStore: Send {
    fn request(&self, request: StoreRequest);
    fn try_reply(&self) -> Option<StoreReply>;
    fn is_lost(&self) -> bool;
    fn flush(&self);
}

impl RegionStore for StoreHandle {
    fn request(&self, request: StoreRequest) {
        StoreHandle::request(self, request);
    }

    fn try_reply(&self) -> Option<StoreReply> {
        StoreHandle::try_reply(self)
    }

    fn is_lost(&self) -> bool {
        StoreHandle::is_lost(self)
    }

    fn flush(&self) {
        StoreHandle::flush(self);
    }
}

/// Names a link for as long as its runner exists. Links are numbered in the order they
/// were attached, which is also the order they are served in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct LinkId(u64);

/// What a link is owed in answer to its hello, which is made from the region as it is
/// just before the next tick.
struct Resume {
    /// The number of the last outbox entry the edge has got.
    seen: u64,
    /// The players the edge believes to be in the region.
    players: Vec<PlayerId>,
    /// What the hello is answered with.
    answer: Answer,
}

/// How a hello is answered; see `docs/adr/0012-the-tick-on-chunks.md`, section 4.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// The region knows the edge with the hello's start and `since`.
    Resumed,
    /// The region knows the edge with that start since another moment than the edge
    /// says, and has taken nothing from it since: the edge never read the welcome that
    /// said so. It is told again, and sent the whole outbox.
    ToldAgain,
    /// The region does not know the edge with that start, or resets it: the tick that
    /// takes the hello makes its state.
    New,
}

/// What has become of a link's subscription to a chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Condition {
    /// It has not been answered.
    Waiting,
    /// The snapshot has been made, and no other answer follows it: the subscription
    /// stays as it is until the link ends it or ends itself, as a region holds a chunk
    /// for as long as a link is subscribed to it.
    Served,
    /// The link has been told that this region holds the chunk, which only a viewer's
    /// subscription is. Nothing of the chunk is said on it until the link asks again.
    Elsewhere(RegionId),
}

/// A link's subscription to a chunk, of which it has at most one. See
/// `docs/adr/0012-the-tick-on-chunks.md`, section 4.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Subscription {
    /// Whether it is a viewer's or a guest's, which is the kind of ticket the region
    /// counts for it.
    kind: Ticket,
    /// The number of the last subscription message of the link that named the chunk,
    /// 0 for its hello. The answer carries it, so that the edge can tell an answer to
    /// what it asked last from one to a subscription it has changed or ended since.
    ask: u64,
    condition: Condition,
    /// The part the region was told the chunk is of for this subscription, by the
    /// runner and not by the store ([`RegionRunner::of_a_part`]). The belief is the
    /// subscription's: when it ends or becomes a guest's, the belief is taken back,
    /// unless another subscription to the chunk has it too.
    taken: Option<RegionId>,
}

/// How many chunks one message of a link asked for that the region took for a part's,
/// by the part: all of them, and those that did not go with the part's split.
type Taken = BTreeMap<RegionId, (usize, usize)>;

/// Counts a chunk that [`RegionRunner::subscribe`] took for a part's.
/// How far `position` is from the nearest of the chunks `players` stand in, as the
/// square of the distance in chunks; 0 if there is nobody, so that the loads then go
/// out in the order of their positions.
fn nearest(players: &[ChunkPos], position: ChunkPos) -> i64 {
    players
        .iter()
        .map(|player| {
            let dx = i64::from(player.x) - i64::from(position.x);
            let dz = i64::from(player.z) - i64::from(position.z);
            dx * dx + dz * dz
        })
        .min()
        .unwrap_or(0)
}

fn note_taken(taken: &mut Taken, for_part: Option<(RegionId, bool)>) {
    if let Some((part, free)) = for_part {
        let (chunks, of_them_free) = taken.entry(part).or_default();
        *chunks += 1;
        *of_them_free += usize::from(free);
    }
}

/// Says what a message of the link `id` asked for that was taken for a part's.
fn say_taken(id: LinkId, taken: &Taken) {
    for (part, (chunks, free)) in taken {
        info!(
            link = id.0,
            part = part.0,
            chunks,
            free,
            "chunks asked for players who went are taken for the part's"
        );
    }
}

/// A split this runner made, kept while an edge has not heard of it; see
/// `docs/adr/0017-the-end-of-the-stripes.md`, section 3.6.2.
struct Parted {
    /// Where its line is.
    sides: Sides,
    /// The chunks that went with it.
    gone: BTreeSet<ChunkPos>,
}

/// A link to an edge and what the runner keeps for it.
struct EdgeLink {
    end: WorkerEnd,
    /// The edge, once it has said hello.
    edge: Option<EdgeId>,
    /// The number of the last numbered message taken from this link, passed on or not.
    last: Option<u64>,
    /// Whether the region did not know the edge with the start of the hello, and no
    /// numbered message of this link has been passed on yet; see
    /// [`RegionRunner::accept`].
    unknown: bool,
    /// The answer to the hello, until the next tick takes it.
    resume: Option<Resume>,
    /// The number of the last outbox entry the edge has said on this link that it has:
    /// with a hello that was answered as a resume, or with a `Confirm`. What it says so
    /// with takes the entry out of the outbox only when the coming tick has run, so
    /// the region's state can still have an entry the edge has read.
    heard: u64,
    /// The number of the last subscription message taken from this link, 0 for a hello
    /// that named chunks; `None` if there has been none. Each has to be above the one
    /// before.
    asked: Option<u64>,
    /// The chunks named in the hello whose subscriptions have not been answered yet.
    /// While there are any, what the link sends is kept in `held`.
    hold: BTreeSet<ChunkPos>,
    /// What the link sent while it was held, in the order it came: behind its hello,
    /// or behind an action on a block that waits for its chunk, which is then the
    /// first of them ([`RegionRunner::waits_for_its_chunk`]).
    held: VecDeque<EdgeMessage>,
    /// The chunks the edge is subscribed to, each with what kind of subscription it is,
    /// its number and what has become of it.
    subscriptions: BTreeMap<ChunkPos, Subscription>,
    /// The chunks whose subscriptions wait for an answer.
    waiting: BTreeSet<ChunkPos>,
    /// Whether the last tick's events the link was sent had block changes after the
    /// beginning of block events, which the edge sends with the next tick's: that tick
    /// is then sent, also with no events (`docs/adr/0026-redstone.md`, section 8).
    carried: bool,
}

impl EdgeLink {
    /// Whether the link is told what happens in the chunk at `position`: it has a
    /// subscription to it that waits or is served. Between an `Elsewhere` and the
    /// link's asking again a region says nothing of the chunk.
    fn watches(&self, position: ChunkPos) -> bool {
        let subscription = self.subscriptions.get(&position);
        subscription
            .is_some_and(|subscription| !matches!(subscription.condition, Condition::Elsewhere(_)))
    }

    /// What the edge is to hear of `events`: what happened in chunks it watches, where
    /// its own players have moved to, and that the entities among `orphaned` are gone.
    fn visible(
        &self,
        events: &[RegionEvent],
        region: &Region,
        orphaned: &BTreeSet<EntityId>,
    ) -> Vec<RegionEvent> {
        let mut visible = Vec::new();
        for event in events {
            // The beginning of block events has no place, and every edge is told.
            if *event == RegionEvent::BlockEventsBegin {
                visible.push(event.clone());
                continue;
            }
            // A container's data is never sent to clients, so the edges are not told
            // of it (`docs/groundwork/n6-plan.md`, section 6 item 12).
            if matches!(event, RegionEvent::BlockEntityChanged { seen: false, .. }) {
                continue;
            }
            // What a player near the edge's watched chunk did to a block across a line:
            // the edge's clients have the block from the region that holds it.
            if let RegionEvent::Near { chunk, event } = event {
                if self.watches(*chunk) {
                    visible.push((**event).clone());
                }
                continue;
            }
            let [current, previous] = event.chunks();
            let visible_now = self.watches(current);
            let visible_before = self.watches(previous);
            match event {
                // A player who was let go and will not be passed on was last seen in a
                // chunk this region does not hold, which nobody can subscribe to here.
                // Those who saw them leave did so from any chunk, so everyone is told.
                RegionEvent::EntityRemoved { entity, chunk }
                    if orphaned.contains(entity) && region.knowledge(*chunk) != Knowledge::Held =>
                {
                    visible.push(event.clone());
                }
                // An entity coming in from where the edge could not see it is new to
                // the edge, which needs to know what it is.
                RegionEvent::EntityMoved { entity, .. } if visible_now && !visible_before => {
                    if let Some(state) = region.entity(*entity) {
                        visible.push(RegionEvent::EntitySpawned(state));
                    }
                }
                event if visible_now || visible_before => visible.push(event.clone()),
                // Where its own player is, an edge is told wherever that is. It asks
                // for what the player sees by where the region says they are, and
                // would never ask again for someone who got ahead of what it watches:
                // in the ticks it takes the edge's wish for a chunk to come back here,
                // somebody fast enough leaves the chunks around the place they were
                // last reported in, and so does anybody while those ticks are many.
                RegionEvent::EntityMoved { entity, .. }
                    if self.edge.is_some() && region.edge_of(*entity) == self.edge =>
                {
                    visible.push(event.clone());
                }
                _ => {}
            }
        }
        visible
    }
}

/// What the runner keeps for an edge the region knows, or will know after the coming
/// tick.
struct KnownEdge {
    /// The highest start the edge has said hello with.
    start: u64,
    /// Whether the region, as it is after its last tick, knows the edge with that start.
    /// False from a hello of an edge it does not know, or with a higher start, until the
    /// tick that takes that hello has run.
    settled: bool,
    /// The number of the last numbered message of the edge that was passed on to the
    /// region: into the inputs of a tick that has run, or of the coming one.
    received: u64,
    /// The region's `applied` for the edge after its last tick.
    applied: u64,
    /// The edge's link, if it has one: the last that said hello as this edge.
    link: Option<LinkId>,
    /// The tick after which the edge was last seen without a link.
    away_since: u64,
}

/// What a tick produced for edges, held until the world store has confirmed the tick.
struct HeldTick {
    tick: u64,
    /// Whether a commit was sent for the tick. A tick that changed nothing and says
    /// nothing of any stay sends none and counts as committed as soon as the tick before
    /// it is.
    needs_commit: bool,
    /// What goes to which link, in the order it is to be published.
    outgoing: Vec<(LinkId, WorkerToEdge)>,
}

/// Runs one region for the edges that are linked to it.
pub struct RegionRunner {
    region: Region,
    /// Which region this is, if the runner was told ([`RegionRunner::with_id`]). The
    /// region itself does not know.
    id: Option<RegionId>,
    store: Box<dyn RegionStore>,
    /// The links to edges, in the order they were attached.
    links: BTreeMap<LinkId, EdgeLink>,
    /// The number the next link gets.
    next_link: u64,
    /// Links that have been attached but not been taken up yet.
    attached: Receiver<WorkerEnd>,
    /// Kept to make [`Links`] of.
    attach: Sender<WorkerEnd>,
    /// The edges the region knows, and those it will know after the coming tick.
    edges: BTreeMap<EdgeId, KnownEdge>,
    /// The number of the last applied input of each player in the region, after its last
    /// tick; what has changed of it is what edges are told as progress.
    last_inputs: BTreeMap<PlayerId, u64>,
    /// What the coming tick will be given.
    inputs: TickInputs,
    /// Whose doing each `Discard` among `inputs` is, which the inputs themselves do not
    /// say.
    discards: Vec<(EdgeId, EntityId, ChunkPos)>,
    /// The ticks that have run and are not published yet, oldest first.
    pending: VecDeque<HeldTick>,
    /// The highest tick the store has confirmed the commit of.
    committed: u64,
    /// Chunks the store could not read. Nothing waits for a snapshot of them.
    unreadable: BTreeSet<ChunkPos>,
    /// Chunks the store said it is making, until it answers their load. They hold no
    /// link: a chunk that is being made was never on anyone's screen, so nothing an
    /// edge sends again acts on it but by `waits_for_its_chunk`, which still waits.
    /// See `docs/adr/0021-generation-is-a-function.md`, section 7, "The hold of a
    /// link".
    generating: BTreeSet<ChunkPos>,
    /// The splits this runner made of which some edge has not heard yet, by the part's
    /// id. A runner that is made anew has none: its region is restored from the
    /// store's record, which does not have where a split's line was.
    parted: BTreeMap<RegionId, Parted>,
    /// How many loads of each chunk the store has been asked for and has not answered.
    /// The store answers the loads of one chunk in the order they were asked, so an
    /// answer that leaves some is to a request the region has dropped since; see
    /// [`RegionRunner::answers_the_latest_load`].
    loads: BTreeMap<ChunkPos, u32>,
    /// [`Region::crowds`] after the last tick, to tell when it changes.
    crowds: Crowds,
    /// Loaded chunks that have changed since they were loaded or last stored, with the
    /// parts of them that changed: their blocks (with everything else a chunk keeps)
    /// and their entities, which the store keeps apart (`docs/adr/0028-entities.md`,
    /// section 2).
    unsaved: BTreeMap<ChunkPos, Parts>,
    /// Loaded chunks the runner has saved since it loaded them. A chunk's first save
    /// writes every part, so that its files are of one save whatever the store had of
    /// it: a note of the generator's that a restart lost, say, which would leave the
    /// entities of a save of them alone without the blocks they are read beside
    /// (`docs/groundwork/n2-plan.md`, section 5). Kept to the chunks loaded at each
    /// checkpoint.
    saved_once: BTreeSet<ChunkPos>,
    /// The chunks the region has asked the store to lend it and has had no answer for
    /// yet: subscriptions to them wait, so that the edge is not sent back and forth
    /// while a grant is on its way (`docs/adr/0027-lending-at-a-line.md`, section 5).
    lends_in_flight: BTreeSet<ChunkPos>,
    /// Loaded chunks that ticked with ticks pending in the last tick and have not been
    /// saved since ([`TickOutput::ticks_pending`]). What is kept of a tick is the delay
    /// left, so their saved forms age with every tick without a commit; they are saved
    /// as unsaved chunks are only where a stop is near, since a checkpoint while the
    /// region ticks would write them all again each time
    /// (`docs/adr/0024-block-behaviour-in-a-region.md`, section 4).
    ///
    /// [`TickOutput::ticks_pending`]: clustine_sim::api::TickOutput::ticks_pending
    ageing: BTreeSet<ChunkPos>,
    /// Loaded chunks whose entities changed without a commit since they were last
    /// saved: counters, a resting item's motion. Saved at every checkpoint and at a stop,
    /// so that what the store has of them is never older than the last checkpoint
    /// (`docs/groundwork/n4-plan.md`, section 7).
    entities_ageing: BTreeSet<ChunkPos>,
    /// How many ticks pass between two checkpoints.
    checkpoint_interval: u64,
    /// How many ticks an edge may be without a link before it is gone.
    gone_after: u64,
    /// Chunks the runner has in memory that the region holds and has not loaded, each
    /// known to be what the store has: every chunk the region had loaded at its last
    /// merge or split, and for a region that a split made, every chunk of it that the
    /// split region had loaded. When a tick asks storage for one, the runner hands it
    /// to the next tick instead of asking the store, and forgets it; so a resume after
    /// a merge or a split does not read back from the store what was in memory a
    /// moment before. That is safe because only the holder saves a chunk and nothing
    /// was unsaved at that tick: until the region loads the chunk, nothing can have
    /// changed what the store has of it. One is also forgotten when the region gives
    /// the chunk back, and after [`WARM_FOR`] ticks.
    warm: BTreeMap<ChunkPos, Warm>,
    /// The ring tickets handed to the region: around each chunk it holds that a link
    /// subscribes to, the neighbours no link subscribes to, which the chunk needs loaded
    /// to tick (`docs/adr/0024-block-behaviour-in-a-region.md`, section 2).
    rings: BTreeMap<ChunkPos, Ticket>,
    /// Whether the runner hands the region rings at all: off only in the tests of
    /// ADR-0012's tickets, which count exactly what a region claims and loads.
    with_rings: bool,
    /// Whether the store handle is lost, after which the runner does nothing any more.
    lost: bool,
    /// How far the runner is with releasing the region, or with a merge or a split.
    phase: Phase,
    /// The merge or the split that is under way: from the first checkpoint for it
    /// until the store has answered, or the runner has ended.
    reshaping: Option<Reshaping>,
    /// The store's answer to the merge or the split it was handed, until the step that
    /// finds it acts on it.
    answer: Option<StoreReply>,
    /// Whether a [`Reshape::Prepare`] has had the store asked for a checkpoint and a
    /// flush that no merge or split has made use of yet.
    prepared: bool,
    /// While the runner prepares a release, a merge or a split: whether a checkpoint
    /// was asked for it, and not only by a [`Reshape::Prepare`] before it. It then
    /// stops when the store has that one, however much has changed meanwhile, so that
    /// a region that changes faster than it is saved stops all the same.
    checkpointed: bool,
    /// What was handed in from another thread and has not been taken.
    commands: Receiver<Order>,
    /// Kept to give to a [`Worker`].
    command: Sender<Order>,
    /// [`MAX_RESHAPE_BYTES`], unless a test has lowered it.
    max_reshape_bytes: usize,
    /// [`FEW_UNSAVED`], unless a test has lowered it.
    few_unsaved: usize,
    /// Entity changes a test has the next tick make, beside the sim's: the sim of step
    /// N2b makes none (`docs/groundwork/n2-plan.md`, section 2).
    #[cfg(test)]
    injected_entity_changes: Vec<(ChunkPos, clustine_world::EntityChange)>,
    /// The tick at which the checkpoint of the last [`Reshape::Prepare`] was asked for.
    prepared_at: u64,
    /// [`FRESH_FOR`], unless a test has lowered it.
    fresh_for: u64,
    /// [`TICK`], unless a test has set another time between two ticks.
    pace: Duration,
    /// How the runner ended, once it has.
    ended: Option<Ended>,
    /// How many flushes the runner has asked the store for, and how many of them the
    /// store has answered. A release waits for its flushes this way rather than with
    /// [`RegionStore::flush`], which would wait for ever for a store that does not
    /// answer, and throw away the confirmations of commits on the way.
    flushes_asked: u64,
    flushes_answered: u64,
    /// Set from another thread to have [`RegionRunner::run`] release the region.
    release_asked: Arc<AtomicBool>,
    status: Arc<RegionStatus>,
    /// Whom to tell how long the region stood still for a merge or a split, if anyone;
    /// see [`RegionRunner::with_standstills`].
    standstills: Option<Box<dyn Fn(Standstill) + Send>>,
    /// Since when the region has not ticked for a merge or a split, until its next
    /// tick has run. Noted only if there is somebody to tell.
    stopped: Option<Stopped>,
    /// The ticks that took longer than a tick has, for the log.
    slow: SlowTicks,
    /// What the runner keeps of the region's borders: effects, copies, couplings and
    /// use timers (`docs/adr/0025-effects-between-regions.md`).
    border: Border,
}

/// Ticks that took longer than the [`TICK`] a tick has: the first is said in the log
/// at once, and those after it within a second are counted, with the longest, and
/// said with the next slow tick after that, so that a region that is behind does not
/// fill the log.
#[derive(Debug, Default)]
struct SlowTicks {
    /// When slow ticks were last said.
    said: Option<Instant>,
    /// How many there were since, and the longest of them.
    count: u32,
    longest: Duration,
}

impl SlowTicks {
    fn note(&mut self, region: Option<RegionId>, tick: u64, took: Duration) {
        if took <= TICK {
            return;
        }
        self.count += 1;
        self.longest = self.longest.max(took);
        let now = Instant::now();
        if self
            .said
            .is_some_and(|said| now - said < Duration::from_secs(1))
        {
            return;
        }
        warn!(
            region = ?region,
            tick,
            ticks = self.count,
            longest_ms = self.longest.as_secs_f64() * 1000.0,
            "ticks took longer than the 50 ms a tick has"
        );
        *self = Self {
            said: Some(now),
            ..Self::default()
        };
    }
}

/// What a region is restored with beside what the world store says of it: a
/// [`RegionConfig`] without the spawn, which is the world's, kept by the store and said
/// when it opens the region.
#[derive(Debug, Clone, PartialEq)]
pub struct RegionSettings {
    /// [`RegionConfig::starting_inventory`].
    pub starting_inventory: Inventory,
    /// [`RegionConfig::return_after`].
    pub return_after: u64,
    /// [`RegionConfig::lowest_y`].
    pub lowest_y: i32,
    /// Whether the runner hands its region ring tickets around what it sees, so that
    /// the rim of a view ticks (`docs/adr/0024-block-behaviour-in-a-region.md`,
    /// section 2): always, but in the tests of what a region claims and loads for
    /// viewers' and guests' tickets alone ([`RegionRunner::without_rings`]).
    pub rings: bool,
}

impl RegionSettings {
    /// The configuration of a region of the world whose spawn is the block `spawn`,
    /// whose number is `world` and whose features' seed is `seed`
    /// ([`Restored::seed`]): players enter in the middle of the spawn, random ticks are
    /// drawn from the number, and saplings grow the trees of the seed's features.
    pub fn with_spawn(self, spawn: BlockPos, world: u64, seed: Option<i64>) -> RegionConfig {
        self.of_region(Dimension::Overworld, Some(spawn), world, seed)
    }

    /// The configuration of the region the store restored as `restored`: of its
    /// dimension, with the world's spawn if it is the overworld's, and with the homes
    /// a portal sends players to (`docs/groundwork/p-plan.md`, 16.1). A runner and
    /// whatever else makes a region from what the store restored take it from here,
    /// so that a region restored and a part split from it in memory are configured
    /// alike.
    pub fn of_restored(self, restored: &Restored) -> RegionConfig {
        RegionConfig {
            homes: restored.homes.clone(),
            strongholds: restored.strongholds.clone(),
            world_spawn: restored.world_spawn,
            default_mode: restored.game_mode,
            force_mode: restored.force_game_mode,
            ..self.of_region(
                restored.dimension,
                restored.spawn,
                restored.world,
                restored.seed,
            )
        }
    }

    /// The configuration of a region of `dimension`, which has the world's spawn if it
    /// is the overworld's ([`Restored::spawn`], [`Restored::dimension`]); otherwise as
    /// [`RegionSettings::with_spawn`]. It names no homes: see
    /// [`RegionSettings::of_restored`].
    pub fn of_region(
        self,
        dimension: Dimension,
        spawn: Option<BlockPos>,
        world: u64,
        seed: Option<i64>,
    ) -> RegionConfig {
        RegionConfig {
            dimension,
            // The store's to say ([`Restored::homes`], [`Restored::strongholds`]), which
            // the caller puts in.
            homes: Vec::new(),
            strongholds: Vec::new(),
            // The overworld's spawn where this is one of its regions; the store's to
            // say otherwise ([`Restored::world_spawn`]), which the caller puts in.
            world_spawn: spawn.unwrap_or(BlockPos::new(0, 0, 0)),
            spawn: spawn.map(Vec3::bottom_centre_of),
            starting_inventory: self.starting_inventory,
            // The world's, the store's to say ([`Restored::game_mode`]), which the
            // caller puts in.
            default_mode: clustine_world::GameMode::Creative,
            force_mode: false,
            return_after: self.return_after,
            // Each dimension's own bottom (`docs/groundwork/p-plan.md`, 17.5): the
            // overworld's as the settings say.
            lowest_y: match dimension {
                Dimension::Overworld => self.lowest_y,
                other => other.rules().min_y,
            },
            seed: world,
            // A flat world has no seed of its own yet: 0, which an official server
            // given `level-seed=0` shares.
            game_seed: seed.unwrap_or(0),
            // The dimensions' noise settings' (the plan, 17.1); a flat overworld's is
            // its own.
            sea_level: match dimension {
                Dimension::Overworld if seed.is_some() => 63,
                Dimension::Overworld => -63,
                Dimension::Nether => 32,
                Dimension::End => 0,
            },
            spawn_mobs: spawn_mobs(),
            // A server's budget for lighting what it loads (ADR-0029), and the checks the
            // end-to-end tests ask for.
            light: clustine_sim::LightSettings {
                checks: light_checks(),
                ..clustine_sim::LightSettings::SERVER
            },
            features: features_of(dimension, seed),
            random_tick_speed: random_tick_speed(),
        }
    }
}

/// Whether the regions this process opens spawn mobs (the game rule `spawn_mobs`), unless
/// the environment says ([`set_spawn_mobs`]).
static SPAWN_MOBS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Has the regions this process opens from now on spawn mobs or not (the game rule
/// `spawn_mobs`), where `CLUSTINE_SPAWN_MOBS` does not say otherwise: for the end-to-end
/// tests that count entities and run a server inside their own process.
pub fn set_spawn_mobs(on: bool) {
    SPAWN_MOBS.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Whether the regions spawn mobs: as `CLUSTINE_SPAWN_MOBS` says (`0` or `false` for
/// not), else as [`set_spawn_mobs`] last said, else yes, as a game has it.
fn spawn_mobs() -> bool {
    match std::env::var("CLUSTINE_SPAWN_MOBS").ok().as_deref() {
        Some("0" | "false") => false,
        Some("1" | "true") => true,
        _ => SPAWN_MOBS.load(std::sync::atomic::Ordering::Relaxed),
    }
}

/// The game rule `random_tick_speed` of the regions this process opens, unless the
/// environment says ([`set_random_tick_speed`]).
static RANDOM_TICK_SPEED: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(clustine_sim::RANDOM_TICK_SPEED);

/// Has the regions this process opens from now on draw this many positions of each
/// section a tick (the game rule `random_tick_speed`), where `CLUSTINE_RANDOM_TICK_SPEED`
/// does not say otherwise: for the comparisons with an official server, which turn it
/// off on both servers so that nothing grows while they are read.
pub fn set_random_tick_speed(speed: u32) {
    RANDOM_TICK_SPEED.store(speed, std::sync::atomic::Ordering::Relaxed);
}

/// The regions' random tick speed: as `CLUSTINE_RANDOM_TICK_SPEED` says, else as
/// [`set_random_tick_speed`] last said, else the game's 3.
fn random_tick_speed() -> u32 {
    std::env::var("CLUSTINE_RANDOM_TICK_SPEED")
        .ok()
        .and_then(|speed| speed.parse().ok())
        .unwrap_or_else(|| RANDOM_TICK_SPEED.load(std::sync::atomic::Ordering::Relaxed))
}

/// How many chunks a step the regions of this process check their light in, unless the
/// environment says ([`check_light_in_turn`]); 0 for none, as on a server.
static LIGHT_CHECKS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Has the regions this process opens from now on check this many lit chunks a step
/// against the whole-chunk function (`docs/groundwork/n8-plan.md`, section 4.7), where
/// `CLUSTINE_LIGHT_ORACLE` does not say otherwise: for the end-to-end tests that run a
/// server inside their own process, whose environment they cannot safely change.
pub fn check_light_in_turn(chunks: u32) {
    LIGHT_CHECKS.store(chunks, std::sync::atomic::Ordering::Relaxed);
}

/// How much of their light the regions check after each step: as many chunks a step as
/// `CLUSTINE_LIGHT_ORACLE` names (the end-to-end tests' harness sets it for the processes
/// it starts), else as [`check_light_in_turn`] last said, else none. A value that is no
/// number is taken as unset, since a server is not to stop for its tests' setting.
fn light_checks() -> clustine_sim::LightChecks {
    let count = std::env::var("CLUSTINE_LIGHT_ORACLE")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or_else(|| LIGHT_CHECKS.load(std::sync::atomic::Ordering::Relaxed));
    match count {
        0 => clustine_sim::LightChecks::Off,
        count => clustine_sim::LightChecks::InTurn(count),
    }
}

/// The features of the world a process serves, by dimension and seed: built for the
/// first region of a dimension it opens and shared by the rest, since reading every
/// feature of the game's data takes some milliseconds and a worker serves one world.
/// Tests that open worlds of other seeds in one process keep each seed's, so that two
/// running at once do not take each other's away.
static FEATURES: Mutex<BTreeMap<(Dimension, i64), Arc<FeatureSettings>>> =
    Mutex::new(BTreeMap::new());

/// The features of `dimension` of `seed`, or of the flat world's seed 0 without one:
/// in a flat world too a sapling grows into the game's tree and bone meal on grass
/// places the plains' plants, as a game's superflat world has them.
///
/// They are without the blocks of the structure templates, which a worker does not
/// read from the game's jar: the trees saplings grow and what bone meal places need
/// none, and a region places no other feature.
fn features_of(dimension: Dimension, seed: Option<i64>) -> Features {
    let seed = seed.unwrap_or(0);
    // Built while the lock is held, so that regions opened at once wait for the one
    // build rather than each making their own.
    let mut kept = FEATURES.lock().unwrap_or_else(PoisonError::into_inner);
    let features = kept.entry((dimension, seed)).or_insert_with(|| {
        Arc::new(
            FeatureSettings::new(seed, dimension)
                .expect("the features' data is built into the server and read whole by its tests"),
        )
    });
    Features(Some(Arc::clone(features)))
}

impl RegionRunner {
    /// A runner for the region as the world store has it: `restored` is what the store
    /// returned when the region was opened, and `store` the handle it came with. The
    /// region carries on after the last tick the store has; one that was never opened
    /// before starts with the entity ids the store issued to it. Whatever its state
    /// says, it gives out ids of the block the store names and none that the store
    /// was ever told a stay was given. It has no links until some are attached through
    /// [`RegionRunner::links`], and does not know which region it is until it is told
    /// ([`RegionRunner::with_id`]).
    pub fn restore(
        settings: RegionSettings,
        store: StoreHandle,
        mut restored: Restored,
    ) -> Result<Self, RestoreError> {
        let holdings = holdings(&restored);
        let rings = settings.rings;
        let config = settings.of_restored(&restored);
        let faces = mem::take(&mut restored.faces);
        let borders = Restored {
            deltas: Vec::new(),
            state: None,
            in_use: mem::take(&mut restored.in_use),
            couplings: mem::take(&mut restored.couplings),
            handshakes: mem::take(&mut restored.handshakes),
            held: Vec::new(),
            pinned: Vec::new(),
            lends: mem::take(&mut restored.lends),
            faces: Vec::new(),
            homes: Vec::new(),
            strongholds: Vec::new(),
            ..restored
        };
        let state = restored_state(restored)?;
        let mut runner =
            Self::with_store(Region::restore(config, state, holdings), Box::new(store))
                .with_borders(&borders);
        runner.with_rings = rings;
        runner.inputs.faces = faces;
        Ok(runner)
    }

    /// A runner that carries on with `region`, of which `store` has everything up to its
    /// last tick.
    fn with_store(region: Region, store: Box<dyn RegionStore>) -> Self {
        let (attach, attached) = mpsc::channel();
        let (command, commands) = mpsc::channel();
        let state = region.state();
        let mut runner = Self {
            region,
            id: None,
            store,
            links: BTreeMap::new(),
            next_link: 0,
            attached,
            attach,
            edges: known_edges(&state),
            last_inputs: last_inputs(&state),
            inputs: TickInputs::default(),
            discards: Vec::new(),
            pending: VecDeque::new(),
            committed: state.tick,
            unreadable: BTreeSet::new(),
            generating: BTreeSet::new(),
            parted: BTreeMap::new(),
            loads: BTreeMap::new(),
            crowds: Crowds::new(),
            unsaved: BTreeMap::new(),
            saved_once: BTreeSet::new(),
            lends_in_flight: BTreeSet::new(),
            ageing: BTreeSet::new(),
            entities_ageing: BTreeSet::new(),
            checkpoint_interval: DEFAULT_CHECKPOINT_INTERVAL,
            gone_after: DEFAULT_GONE_AFTER,
            warm: BTreeMap::new(),
            rings: BTreeMap::new(),
            with_rings: true,
            lost: false,
            phase: Phase::Running,
            reshaping: None,
            answer: None,
            prepared: false,
            checkpointed: false,
            commands,
            command,
            max_reshape_bytes: MAX_RESHAPE_BYTES,
            few_unsaved: FEW_UNSAVED,
            #[cfg(test)]
            injected_entity_changes: Vec::new(),
            prepared_at: 0,
            fresh_for: FRESH_FOR,
            pace: TICK,
            ended: None,
            flushes_asked: 0,
            flushes_answered: 0,
            release_asked: Arc::new(AtomicBool::new(false)),
            status: Arc::new(RegionStatus::default()),
            standstills: None,
            stopped: None,
            slow: SlowTicks::default(),
            border: Border::default(),
        };
        runner.show_status();
        runner
    }

    /// The runner with what the store said of the region's borders when it was opened:
    /// its block of entity keys, its couplings, and the chunks it uses, each timed until the region loads it
    /// (`docs/adr/0025-effects-between-regions.md`, sections 5 and 8). [`RegionRunner::
    /// restore`] does this itself; the runner of a split's part is given it here.
    pub fn with_borders(mut self, restored: &Restored) -> Self {
        // The part was made in memory with no keys; its hello gave it a block.
        self.region.take_keys(restored.entity_ids);
        let warm = self.warm.keys().copied().collect::<Vec<_>>();
        self.border = Border::opened(
            restored.offset,
            &restored.couplings,
            &restored.handshakes,
            restored.held_tick,
            restored.given_up,
        );
        let tick = self.region.tick_number();
        self.border.time(restored.in_use.iter().copied(), tick);
        // Warm chunks are timed as well: they count as loaded until they go.
        self.border.time(warm, tick);
        self
    }

    /// A runner for the region a split has made, of which `store` is the handle its
    /// worker opened it with: one as [`RegionRunner::restore`] makes of what the store
    /// has of the region, but that the chunks of the part that the split region had
    /// loaded are kept warm, so that the part's first links are served from memory.
    /// What the store said when the region was opened is not needed: the part is what
    /// the store's record has. See `docs/adr/0014-merging-and-splitting.md`, section
    /// 3.5.
    pub fn of_part(part: Part, store: StoreHandle) -> Self {
        Self::of_part_with(part, Box::new(store))
    }

    fn of_part_with(part: Part, store: Box<dyn RegionStore>) -> Self {
        let mut runner = Self::with_store(part.region, store);
        let since = runner.region.tick_number();
        let warm = |(position, chunk)| {
            (
                position,
                Warm {
                    chunk,
                    since,
                    light: None,
                },
            )
        };
        runner.warm = part.chunks.into_iter().map(warm).collect();
        let warm: Vec<ChunkPos> = runner.warm.keys().copied().collect();
        runner.border.time(warm, since);
        runner
    }

    /// A handle through which links are attached, also while the runner runs.
    pub fn links(&self) -> Links {
        Links {
            attached: self.attach.clone(),
        }
    }

    /// Counters that are kept up to date while the runner runs.
    pub fn status(&self) -> Arc<RegionStatus> {
        Arc::clone(&self.status)
    }

    /// Says which region the runner runs. The home region's runner has to be told:
    /// the world store answers a stay that is entering with the region that held the
    /// player's last place, and the region is to place the player itself where that is
    /// the region itself. A region does not know its own id, so the runner says
    /// "nobody" to it in place of this id ([`Entered::holder`]).
    ///
    /// A runner that was not told passes on whichever region the store names. For its
    /// own that means that the stay is let go to this very region without being
    /// placed, and comes back through its edge as an arrival. The store names a
    /// region so only to the home region. See
    /// `docs/adr/0020-one-stay-per-player.md`, section 4, step 7.
    pub fn with_id(mut self, region: RegionId) -> Self {
        self.id = Some(region);
        self
    }

    /// A runner that hands its region no ring tickets: for the tests of what a region
    /// claims and loads for viewers' and guests' tickets alone (ADR-0012), which rings
    /// would add to.
    pub fn without_rings(mut self) -> Self {
        self.with_rings = false;
        self
    }

    /// Sets how many ticks pass between two checkpoints. Until a checkpoint, changes to
    /// chunks that stay loaded are only in the write-ahead log, which grows meanwhile,
    /// and so does what a region is restored from.
    pub fn with_checkpoint_interval(mut self, ticks: u64) -> Self {
        self.checkpoint_interval = ticks.max(1);
        self
    }

    /// Sets how many ticks an edge may be without a link before the region forgets it
    /// and removes its players.
    pub fn with_gone_after(mut self, ticks: u64) -> Self {
        self.gone_after = ticks;
        self
    }

    /// Has `tell` called, on the runner's thread, each time the region has ticked on
    /// after it had stopped ticking for a merge or a split, whether that was made or
    /// came to nothing. `tell` must not wait.
    ///
    /// Only the runner knows the two moments: when the store has the first checkpoint
    /// and the region stops, and when its next tick has run. That is how long the
    /// region did not tick, and a little less than a player waits, whose edge has to
    /// find the region again besides. Nothing is told of a release, after which the
    /// region never ticks on, nor by a runner that ends in the middle. See
    /// `docs/adr/0017-the-end-of-the-stripes.md`, section 5.6.
    pub fn with_standstills(mut self, tell: Box<dyn Fn(Standstill) + Send>) -> Self {
        self.standstills = Some(tell);
        self
    }

    /// Notes that the region stops ticking, and what it is as it does, if that is for
    /// a merge or a split and somebody is told how long it stood still.
    fn note_the_stop(&mut self) {
        // A stop that is noted already stays: no tick has run since, so the region
        // has stood still from then.
        if self.standstills.is_some() && self.reshaping.is_some() && self.stopped.is_none() {
            self.stopped = Some(Stopped {
                at: Instant::now(),
                players: self.region.player_count() as u64,
                held: self.region.held_chunk_count() as u64,
            });
        }
    }

    /// Tells how long the region stood still, if the tick that has just run is its
    /// first since it stopped for a merge or a split.
    fn tell_the_standstill(&mut self) {
        let Some(stopped) = self.stopped.take() else {
            return;
        };
        if let Some(tell) = &self.standstills {
            let milliseconds = stopped.at.elapsed().as_millis();
            tell(Standstill {
                players: stopped.players,
                held: stopped.held,
                milliseconds: u64::try_from(milliseconds).unwrap_or(u64::MAX),
            });
        }
    }

    pub fn region(&self) -> &Region {
        &self.region
    }

    /// Whether the store handle is lost and the runner has stopped for good.
    pub fn store_is_lost(&self) -> bool {
        self.lost
    }

    /// Takes what has arrived since the last step, publishes the ticks the store has
    /// confirmed and runs one tick, unless the region is [`MAX_TICKS_AHEAD`] ticks ahead
    /// of what is confirmed. A link that has ended or does not keep up is dropped on the
    /// way; the region ticks on with the links that remain, or with none.
    ///
    /// Once the store handle is lost, this does nothing but close links.
    ///
    /// After [`RegionRunner::begin_release`], and after [`RegionRunner::reshape`] with
    /// a merge or a split, it does the same until the store has answered the flush
    /// behind the first checkpoint, and from then on carries the release, the merge or
    /// the split on instead, a step at a time and without ever waiting: no tick runs,
    /// and nothing is taken from links. Each such step gets at most one stage further
    /// ([`RegionRunner::stage`]), so whoever steps a runner by hand sees every stage.
    /// Once the runner has ended, a step closes the links attached since and does
    /// nothing else.
    pub fn step(&mut self) {
        if self.lost {
            self.give_up();
            return;
        }
        match self.phase {
            Phase::Running => {}
            Phase::Preparing => {
                // Looked at before anything is taken from a link, so that whatever a
                // link is relieved of is also given to a tick.
                if !self.take_replies() || self.stop_if_prepared() {
                    return;
                }
            }
            Phase::Settling | Phase::Closing | Phase::Committing => {
                self.carry_on();
                return;
            }
            Phase::Ended => {
                self.refuse_links();
                return;
            }
        }
        // The links known so far come first, and only then the ones attached since, so
        // that a link an edge has ended is seen to have ended before its next one says
        // hello.
        let known: Vec<_> = self.links.keys().copied().collect();
        for id in known {
            self.drain(id);
        }
        while let Ok(end) = self.attached.try_recv() {
            let id = self.take_up(end);
            self.drain(id);
        }
        if !self.take_replies() {
            return;
        }
        self.publish_committed();
        if self.pending.len() < MAX_TICKS_AHEAD && !self.held_by_partners() {
            let started = Instant::now();
            self.tick();
            let tick = self.region.tick_number();
            self.slow.note(self.id, tick, started.elapsed());
            self.tell_the_standstill();
            // A tick that changed nothing has nothing to wait for.
            self.publish_committed();
        }
    }

    /// Whether the next tick waits for the region's partners, or for a handshake
    /// (`docs/adr/0025-effects-between-regions.md`, section 8).
    fn held_by_partners(&self) -> bool {
        !self.border.may_tick(self.region.tick_number() + 1)
    }

    /// Stops the region's ticking for the release, the merge or the split it prepares,
    /// if the store has what that waits for; returns whether it did. The store's
    /// answers have been taken.
    ///
    /// The store has answered every flush: the one behind the checkpoint asked for
    /// this, or the one behind the checkpoint of a [`Reshape::Prepare`] before it. In
    /// the second case more than [`FEW_UNSAVED`] chunks can have changed since, and
    /// then a checkpoint is asked for now and waited for as the region ticks, once.
    ///
    /// As it stops, the region asks for its last checkpoint and a flush. The commits of
    /// the ticks that ran need not be confirmed first: the store holds what a handle
    /// asks behind commits of that handle that are not durable until they are, and
    /// drops it with the handle if they never are. The checkpoint is of the region's
    /// last tick, no tick runs from here on, and it is asked behind everything that
    /// tick asked; so when the flush behind it is answered, the store keeps the state
    /// in place of every commit, has answered every claim and load, has every return
    /// through and every changed chunk saved, whatever was or was not checkpointed
    /// before the stop. That alone is what a merge or a split is worked out on
    /// ([`RegionRunner::commit`]) and what a release ends on.
    ///
    /// This is also called between two ticks ([`RegionRunner::run`]). The region is
    /// then stopped as the next step would have stopped it: that step looks here
    /// before it takes anything from a link, so no tick is given anything else for it.
    fn stop_if_prepared(&mut self) -> bool {
        if self.flushes_answered != self.flushes_asked {
            return false;
        }
        if !self.checkpointed && !self.little_since_prepared() {
            info!(
                tick = self.region.tick_number(),
                unsaved = self.unsaved.len(),
                "much has changed since the region was told to prepare; bringing the store \
                 up to date once more"
            );
            self.checkpoint();
            self.ask_for_flush();
            self.checkpointed = true;
            return false;
        }
        info!(
            tick = self.region.tick_number(),
            reshaping = self.reshaping.is_some(),
            unsaved = self.unsaved.len(),
            // Saved at the stop too, though nothing changed in them but their ticks'
            // delays: how many says what a stop's checkpoint costs (B8).
            ageing = self
                .ageing
                .iter()
                .filter(|position| !self.unsaved.contains_key(position))
                .count(),
            "the store has the first checkpoint of a release, a merge or a split; \
             the region stops ticking"
        );
        self.note_the_stop();
        self.checkpoint_at_stop();
        self.ask_for_flush();
        self.phase = Phase::Settling;
        true
    }

    /// Whether the checkpoint of the last order to prepare has left little for the one
    /// made at the stop: few chunks have changed since, or it was asked for a moment
    /// ago, so that what has changed is what changes in that moment whatever is
    /// saved beforehand.
    fn little_since_prepared(&self) -> bool {
        let since = self.region.tick_number().saturating_sub(self.prepared_at);
        let unsaved = self.unsaved.len()
            + self
                .ageing
                .iter()
                .filter(|position| !self.unsaved.contains_key(position))
                .count();
        unsaved <= self.few_unsaved || since <= self.fresh_for
    }

    /// Whether the region has stopped ticking for a release, a merge or a split, and
    /// the runner has not ended.
    fn stands_still(&self) -> bool {
        matches!(
            self.phase,
            Phase::Settling | Phase::Closing | Phase::Committing
        )
    }

    /// Takes the store's answers. Returns false if the store handle is lost, which
    /// stops the runner.
    fn take_replies(&mut self) -> bool {
        while let Some(reply) = self.store.try_reply() {
            match reply {
                // Travellers and the portal index's answers go to the coming tick as
                // they come (`docs/groundwork/p-plan.md`, section 15.4): the region keeps
                // what it needs loaded for them itself.
                StoreReply::Travellers { travellers } => {
                    self.inputs.travellers.extend(travellers);
                }
                StoreReply::Portals { answers } => {
                    self.inputs.portal_answers.extend(answers);
                }
                StoreReply::GatewayExits { answers } => {
                    self.inputs.gateway_exits.extend(answers);
                }
                // Lends (`docs/adr/0027-lending-at-a-line.md`): into the coming tick, as
                // a claim's answer goes. An ask answered in any way is no longer under way.
                StoreReply::LendAsked { lend, chunks, to } => {
                    self.inputs.lends_asked.push(LendAsked { lend, chunks, to });
                }
                StoreReply::NotLent { chunks, again } => {
                    for position in &chunks {
                        self.lends_in_flight.remove(position);
                    }
                    self.inputs.not_lent.push((chunks, again));
                }
                StoreReply::LendCalledOff { lend, chunks } => {
                    for position in &chunks {
                        self.lends_in_flight.remove(position);
                    }
                    self.inputs.lends_called_off.push((lend, chunks));
                }
                // What the lender was sent for them and has not run is the borrower's
                // now, which the store sends it (the same record, section 3, step 4).
                StoreReply::Lent { chunks, to } => {
                    let lent = chunks.iter().copied().collect();
                    self.border.drop_effects_of(&lent);
                    self.border.drop_arrivals_of(&lent);
                    self.inputs.lent.push((chunks, to));
                }
                StoreReply::PinnedBack { chunks } => self.inputs.pinned_back.extend(chunks),
                StoreReply::Loaded { position, chunk } => {
                    self.generating.remove(&position);
                    if self.answers_the_latest_load(position) {
                        self.inputs.chunks_loaded.push((position, chunk));
                    }
                }
                // The region goes on waiting for the chunk, which leaves a hole in the
                // world rather than a chunk that would overwrite what was built there.
                // Nothing else waits for it.
                StoreReply::Unreadable { position } => {
                    error!(?position, "a chunk cannot be read from the world store");
                    self.generating.remove(&position);
                    if self.answers_the_latest_load(position) {
                        // Its use ends: the store does not end it by itself ("Found
                        // while building", E3a, of ADR-0025).
                        self.store.request(StoreRequest::Dropped {
                            chunks: vec![position],
                        });
                        self.border.drop_effects_of(&BTreeSet::from([position]));
                        self.unreadable.insert(position);
                        for link in self.links.values_mut() {
                            link.hold.remove(&position);
                        }
                        self.release_held();
                    }
                }
                // A chunk that is being made holds no link from here until its
                // load is answered: the links it held are let go of, and a later
                // hello does not hold on it.
                StoreReply::Generating { position } => {
                    self.generating.insert(position);
                    for link in self.links.values_mut() {
                        link.hold.remove(&position);
                    }
                    self.release_held();
                }
                // A load the region no longer wants, since it gave the chunk back: it
                // is counted as that load's answer, so that the answer to a later
                // load of the chunk is not taken for this one's.
                StoreReply::Withdrawn { position } => {
                    self.generating.remove(&position);
                    self.answers_the_latest_load(position);
                }
                // The store's answers to the claims of earlier ticks, in the order of
                // those claims. They go into the coming tick whenever that runs: a chunk
                // the region has asked for is asked until it is answered, so an answer
                // that was dropped would leave whoever waits for the chunk waiting for
                // good. Only a runner that never ticks again lets them lie.
                StoreReply::Claimed { granted, foreign } => {
                    for position in &granted {
                        self.lends_in_flight.remove(position);
                    }
                    self.inputs.granted.extend(granted);
                    self.inputs.foreign.extend(foreign);
                }
                // A region loads and saves only what it holds, and holds only what the
                // store has granted it and it has not given back. So the region and the
                // store disagree about what the region holds, and nothing the region
                // goes on to do can be trusted: it stops as if the store were lost, and
                // learns what it holds when it is opened again.
                StoreReply::NotHeld { position, holder } => {
                    error!(
                        ?position,
                        ?holder,
                        "the world store does not take the region for the holder of a chunk \
                         it loads or saves"
                    );
                    self.give_up();
                    return false;
                }
                StoreReply::Committed { tick } => {
                    self.committed = self.committed.max(tick);
                    self.border.committed_through(self.committed);
                }
                StoreReply::Flushed => self.flushes_answered += 1,
                // The store's answer to a stay the region named as entering, behind the
                // answer to the commit that named it. Like a claim's answer it goes
                // into the coming tick whenever that runs. A merge or a split that is
                // taken first drops it with everything else that waits; the tick after
                // it names every stay again, and the store answers again. See
                // `docs/adr/0020-one-stay-per-player.md`, section 4, step 6, and
                // section 9.
                StoreReply::Enter {
                    player,
                    entity,
                    place,
                    holder,
                    unit,
                } => {
                    // The region does not know which region it is: for a place it
                    // held itself, it is told that nobody did, and places the player.
                    let holder = holder.filter(|holder| Some(*holder) != self.id);
                    self.inputs.entered.push(Entered {
                        player,
                        entity,
                        place,
                        holder,
                        unit,
                    });
                }
                // Stays that a later one has replaced: in answer to what the region
                // said of them, or, at any time, because the home region has given a
                // player a new one (section 5 of the same record). The coming tick
                // removes what the region has of them, and keeps nothing of the word.
                StoreReply::Dead { stays } => self.inputs.dead.extend(stays),
                StoreReply::Clock { clock, players } => {
                    self.inputs.clock = Some(self.inputs.clock.unwrap_or(clock).later(clock));
                    self.inputs.world_players = Some(players);
                }
                // The other regions' counts, which come behind the clock.
                StoreReply::Counts(counts) => self.inputs.counts = Some(counts),
                // The margin's publications are changes on changes: folded in order,
                // never one put in place of another (`docs/groundwork/n10-plan.md`,
                // section 3.5).
                StoreReply::Village(village) => match &mut self.inputs.village {
                    Some(folded) => folded.then(village),
                    None => self.inputs.village = Some(village),
                },
                // The trader's chance: the store's latest, which the region weighs
                // against its own.
                StoreReply::TraderChance { firing, chance } => {
                    self.inputs.trader_chance = Some((firing, chance));
                }
                // Faces go to the next tick as they come, whatever the borders hold
                // back: the region keeps them all and shows those it holds or copies
                // (`docs/groundwork/n7-faces.md`).
                StoreReply::Faces { changes } => self.inputs.faces.extend(changes),
                // The answer to the merge or the split the store was handed. It is
                // acted on by the step that finds it, not here: taking it makes the
                // runner begin anew, and this is also called between two ticks.
                reply @ (StoreReply::Absorbed { .. }
                | StoreReply::Split { .. }
                | StoreReply::Declined { .. }) => {
                    if self.phase == Phase::Committing && self.answer.is_none() {
                        self.answer = Some(reply);
                    } else {
                        error!(?reply, "the world store answered what was not asked");
                    }
                }
                // The region's stream: effects, copies and couplings
                // (`docs/adr/0025-effects-between-regions.md`), kept until a tick can
                // use them. A handshake is answered at once, with the next tick.
                reply @ (StoreReply::Effects { .. }
                | StoreReply::Arrivals { .. }
                | StoreReply::CopyBase { .. }
                | StoreReply::CopyUpdate { .. }
                | StoreReply::CopyUsed { .. }
                | StoreReply::CoupleWith { .. }
                | StoreReply::Coupled { .. }
                | StoreReply::HandshakeOff { .. }
                | StoreReply::Shifted { .. }
                | StoreReply::Uncoupled { .. }
                | StoreReply::Progress { .. }
                | StoreReply::Mark { .. }) => {
                    // A copy's faces go to the next tick at once, whenever the borders
                    // hand the base itself in: the region keeps them and shows them once
                    // it copies the chunk, and drops what it kept there from before.
                    let mut reply = reply;
                    if let StoreReply::CopyBase {
                        position,
                        sequence,
                        faces,
                        ..
                    } = &mut reply
                    {
                        self.inputs.faces.push(FaceChange::Base {
                            chunks: Some(vec![*position]),
                            seq: FaceSeq(*sequence, u32::MAX),
                        });
                        self.inputs.faces.append(faces);
                    }
                    let mut asks = Asks::default();
                    let next = self.region.tick_number() + 1;
                    self.border.take(reply, next, &mut asks);
                    if !asks.rebase.is_empty() {
                        self.store.request(StoreRequest::Copies {
                            wanted: asks.rebase,
                            gone: Vec::new(),
                        });
                    }
                    for (partner, tick) in asks.joins {
                        self.store.request(StoreRequest::JoinsAt { partner, tick });
                    }
                }
            }
        }
        // Looked at after the answers: a handle that is lost has none, so nothing
        // confirmed is published by an owner the store has let go of.
        if self.store.is_lost() {
            self.give_up();
            return false;
        }
        true
    }

    /// Notes that the store has answered a load of the chunk at `position`. Returns
    /// whether it is the answer to the last load the region asked for.
    ///
    /// The region takes what storage delivers for a chunk it has asked for, whichever
    /// request the delivery answers. It can ask for a chunk, drop the request with the
    /// chunk's last ticket, give the chunk back, be granted it again and ask again;
    /// another region can have held and changed the chunk in between, and what was
    /// read for the first request is then no longer the chunk. Only the answer to the
    /// latest request is passed on. (An answer that comes before the region has asked
    /// again finds no request there, and the region drops it by itself.)
    fn answers_the_latest_load(&mut self, position: ChunkPos) -> bool {
        match self.loads.get_mut(&position) {
            Some(asked) if *asked > 1 => {
                *asked -= 1;
                false
            }
            // An answer nobody asked for cannot be: the store answers each load once.
            Some(_) | None => {
                self.loads.remove(&position);
                true
            }
        }
    }

    /// Stops for good, because the store handle is lost, or because the store has said
    /// that the region is not what the runner takes it for: what the region did since
    /// its last confirmed commit may never have reached the disk, so nobody is told of
    /// it, and carrying on from memory would build on what a restored region does not
    /// have.
    fn give_up(&mut self) {
        if !self.lost {
            error!("the world store is lost; the region stops and has to be restored");
            self.lost = true;
            self.status.store_lost.store(true, Ordering::Relaxed);
        }
        // Dropping a link closes it, which is how edges learn to look for the region
        // again.
        self.links.clear();
        self.pending.clear();
        self.refuse_links();
        // A release that was under way skips what needs the store, which is all that
        // was left of it.
        self.phase = Phase::Ended;
        if self.ended.is_none() {
            self.ended = Some(Ended::StoreLost);
            self.status.set_ended(Ended::StoreLost);
        }
        // Whether a merge or a split that the store was handed has happened is for
        // the store to say when the region is opened again.
        self.answer = None;
        if let Some(reshaping) = self.reshaping.take() {
            reshaping.done.off(Off::StoreLost);
        }
    }

    /// Closes the links that have been attached and not been taken up.
    fn refuse_links(&mut self) {
        // Dropping a link closes it.
        while self.attached.try_recv().is_ok() {}
    }

    /// Begins to release the region, as section 1 of
    /// `docs/adr/0009-moving-a-region.md` has it for an owner that runs it. This asks the
    /// store for an ordinary checkpoint and returns at once; [`RegionRunner::step`]
    /// does the rest, and [`RegionRunner::ended`] says when it is done:
    ///
    /// 1. The region goes on ticking and serving its links until the store has the
    ///    checkpoint, which can be minutes of changed chunks.
    /// 2. Then it ticks no more, takes nothing more from its links and closes links that
    ///    are attached from now on. It asks for a checkpoint once more, which is what
    ///    changed since a moment ago, and publishes what the ticks that ran produced,
    ///    in order, as the store confirms the commit of each.
    /// 3. It waits until the store has that checkpoint, closes the region at the store
    ///    and then its links, and has ended as [`Ended::Released`].
    ///
    /// If the store handle is lost on the way, the runner publishes nothing more, closes
    /// its links and has ended as [`Ended::StoreLost`]. A store that neither answers nor
    /// closes keeps the release where it is; nothing here waits for it, so whoever steps
    /// the runner decides how long to go on, and drops the runner to give up.
    ///
    /// Asking again, or asking a runner that has ended, changes nothing. Neither does
    /// asking one that is in the middle of a merge or a split: a release waits for
    /// that to end, by being asked for again, as [`RegionRunner::run`] does before
    /// every step.
    pub fn begin_release(&mut self) {
        if self.phase != Phase::Running {
            return;
        }
        if self.lost || self.store.is_lost() {
            self.give_up();
            return;
        }
        info!(
            tick = self.region.tick_number(),
            unsaved = self.unsaved.len(),
            "releasing the region"
        );
        self.checkpoint();
        self.ask_for_flush();
        self.checkpointed = true;
        self.phase = Phase::Preparing;
        self.store.request(StoreRequest::EndCouplings);
    }

    /// How the runner ended, or `None` if it has not. One that has ended ticks no more,
    /// whatever is asked of it, and closes every link it is handed.
    pub fn ended(&self) -> Option<Ended> {
        self.ended
    }

    /// Asks the store to say when it has done everything asked of it so far.
    fn ask_for_flush(&mut self) {
        self.flushes_asked += 1;
        self.store.request(StoreRequest::Flush);
    }

    /// Does to the region what `reshape` says, as section 3 of
    /// `docs/adr/0014-merging-and-splitting.md` has it, for whoever steps the runner by
    /// hand; [`Worker::reshape`] is the same for a runner on a thread of its own. This
    /// returns at once, and [`RegionRunner::step`] does the rest.
    ///
    /// [`Reshape::Prepare`] asks the store for an ordinary checkpoint and to say when
    /// it has it, and nothing else, and that only of a runner that neither releases
    /// nor reshapes its region. One that has been told so before, and whose checkpoint
    /// for it is still being made or has been followed by changes to no more than a
    /// few chunks ([`FEW_UNSAVED`]), asks for nothing: the order comes more than once
    /// before a merge. `done` is never called for it.
    ///
    /// A merge and a split go through the stages of a release
    /// ([`RegionRunner::stage`]) and one more:
    ///
    /// 1. The region goes on ticking and serving its links until the store has a
    ///    checkpoint of it. That is the checkpoint of a [`Reshape::Prepare`] if there
    ///    was one since the last merge or split: the runner then asks for none of its
    ///    own, unless more than a few chunks have changed since by the time the store
    ///    has it, and so stops at once where the store is prepared.
    /// 2. Then it ticks no more, takes nothing more from its links and closes links
    ///    that are attached from now on. It checkpoints once more. The ticks that ran
    ///    are published as the store confirms them.
    /// 3. When the store has that checkpoint, it has answered everything asked of it
    ///    before: no commit of the region is left that a checkpoint does not cover,
    ///    every claim is answered, and nothing is being loaded or unsaved.
    /// 4. The merge or the split is worked out, as the tick after the region's last,
    ///    which changes nothing, and handed to the store. If none comes of it (nobody
    ///    stands in the chunks named, or it is too large to hand over), the region
    ///    ticks on.
    /// 5. When the store has answered that it is on disk, the region takes that tick,
    ///    and the runner begins anew as for a region just restored: **every link is
    ///    closed**, what links sent and no tick took is dropped (their edges send it
    ///    again), and the chunks that were loaded are kept warm, so that the links the
    ///    edges make next are served without the store. If the store declines, the
    ///    region ticks on from its last tick with its links, as if nothing had been
    ///    asked; what they sent while it stood still is taken by its next tick.
    ///
    /// `done` is called once, with the outcome, by the step that has it. A runner that
    /// is not just running (it releases its region, is in the middle of another merge
    /// or split, or has ended) calls it at once with [`Off::Busy`]. If the store
    /// handle is lost on the way, or the runner is stopped or dropped before the
    /// store has answered, the outcome is [`Off::StoreLost`], and whether the merge or
    /// the split happened is for the store's list of regions to say.
    pub fn reshape(&mut self, reshape: Reshape, done: Box<dyn FnOnce(Reshaped) + Send>) {
        self.take_command(Command::new(reshape, done));
    }

    fn take_command(&mut self, command: Command) {
        let Command { reshape, mut done } = command;
        if self.phase != Phase::Running {
            done.off(Off::Busy);
            return;
        }
        let plan = match reshape {
            Reshape::Prepare => {
                // The order comes twice before a merge, with the order to release
                // the other region and with the order to absorb it. A checkpoint that
                // is still being made would only keep a second one waiting behind it,
                // and one that few chunks have changed since has left little to save.
                let outstanding = self.flushes_answered != self.flushes_asked;
                if self.prepared && (outstanding || self.little_since_prepared()) {
                    return;
                }
                // What a lost handle does not take, the next step finds out.
                self.checkpoint();
                self.ask_for_flush();
                self.prepared = true;
                self.prepared_at = self.region.tick_number();
                return;
            }
            Reshape::Absorb {
                absorbed,
                absorbed_epoch,
                state,
            } => Plan::Absorb {
                absorbed,
                absorbed_epoch,
                other: state,
                state: None,
            },
            Reshape::SplitOff {
                chunks,
                as_epoch,
                part,
            } => Plan::Split {
                named: chunks,
                as_epoch,
                part,
                renamed: false,
                splitting: None,
            },
        };
        // From here on, what becomes of it without an outcome is the store's to say.
        done.otherwise = Off::StoreLost;
        self.reshaping = Some(Reshaping { plan, done });
        if self.lost || self.store.is_lost() {
            self.give_up();
            return;
        }
        // The checkpoint of an order to prepare is the one the region ticks until the
        // store has (`docs/adr/0014-merging-and-splitting.md`, section 3.1, step 1),
        // once: the next merge or split is told to prepare again.
        self.checkpointed = !mem::take(&mut self.prepared);
        if self.checkpointed {
            info!(
                tick = self.region.tick_number(),
                unsaved = self.unsaved.len(),
                "bringing the store up to date for a merge or a split"
            );
            self.checkpoint();
            self.ask_for_flush();
        } else {
            info!(
                tick = self.region.tick_number(),
                unsaved = self.unsaved.len(),
                "the store was told to prepare for a merge or a split"
            );
        }
        self.phase = Phase::Preparing;
        // Its partners go on without it from here (section 8).
        self.store.request(StoreRequest::EndCouplings);
    }

    /// Where the runner is with a release, a merge or a split; `None` while it only
    /// runs, and once it has ended. Each [`RegionRunner::step`] gets at most one stage
    /// further, so whoever steps a runner by hand can stop at the first step of each.
    pub fn stage(&self) -> Option<Stage> {
        match self.phase {
            Phase::Running | Phase::Ended => None,
            Phase::Preparing => Some(Stage::Preparing),
            Phase::Settling => Some(Stage::Settling),
            Phase::Closing => Some(Stage::Closing),
            Phase::Committing => Some(Stage::Committing),
        }
    }

    /// Does what can be done of a release, a merge or a split whose region has stopped
    /// ticking, without waiting for anything: at most one stage of it.
    fn carry_on(&mut self) {
        // No new links: their edges find them closed and look for the region again,
        // which they find at its next owner, or here once the merge or the split is
        // through or off. So no hello is answered between handing the store a merge or
        // a split and taking it, and one that is said after the routing table knows of
        // it is answered from after it.
        self.refuse_links();
        if !self.take_replies() {
            return;
        }
        // What the ticks that ran produced is owed to the edges as soon as the store has
        // it, as ever. The next owner is restored with all of it, and an edge that had
        // not been told would have to be told again through the resume.
        self.publish_committed();
        match self.phase {
            // The last checkpoint was asked for as the region stopped. The store keeps
            // its state in place of every commit up to the region's last tick, and
            // holds it back itself until those commits are durable.
            Phase::Settling if self.pending.is_empty() => self.phase = Phase::Closing,
            Phase::Closing if self.flushes_answered == self.flushes_asked => {
                if self.reshaping.is_none() {
                    info!(tick = self.region.tick_number(), "the region is released");
                    self.end(Ended::Released);
                } else if self.loads.is_empty() {
                    // The store answers a load before the flush asked behind it, so
                    // none is under way now. It is looked at all the same: a chunk
                    // that arrived after the tick of a merge or a split would be one
                    // the region has not asked for.
                    self.commit();
                }
            }
            Phase::Committing => {
                if let Some(answer) = self.answer.take() {
                    self.take_answer(answer);
                }
            }
            _ => {}
        }
    }

    /// Works out the merge or the split as the tick after the region's last, which
    /// changes nothing, and hands it to the store; or finds that nothing comes of it.
    ///
    /// The store has answered the flush behind the second checkpoint, and with it
    /// everything asked before: every commit is confirmed, published and covered by
    /// the checkpoint; every claim is answered, and the answers wait in the coming
    /// tick's inputs; every return is through; no load is under way and no chunk is
    /// unsaved, so every loaded chunk is, block for block, what the store has. See
    /// `docs/adr/0014-merging-and-splitting.md`, section 3.2.
    ///
    /// A split is worked out with the grants among those answers, which the store's
    /// table has as the region's like any chunk a tick was told of: those nearer to
    /// who goes are the part's (`docs/adr/0017-the-end-of-the-stripes.md`, section
    /// 3.6.1). The inputs stay where they are. If nothing comes of the split the next
    /// tick takes them, and if the store has another id for the part the split is
    /// worked out again from the same.
    fn commit(&mut self) {
        let Some(mut reshaping) = self.reshaping.take() else {
            return;
        };
        let tick = self.region.tick_number() + 1;
        let limit = self.max_reshape_bytes;
        let request = match &mut reshaping.plan {
            Plan::Absorb {
                absorbed,
                absorbed_epoch,
                other,
                state,
            } => {
                let merged = self.region.absorb(*absorbed, other);
                let bytes = stored(&merged);
                if bytes.len() > limit {
                    Err(Off::TooLarge)
                } else {
                    *state = Some(merged);
                    Ok(StoreRequest::AbsorbCommit {
                        absorbed: *absorbed,
                        absorbed_epoch: *absorbed_epoch,
                        tick,
                        state: bytes,
                    })
                }
            }
            Plan::Split {
                named,
                as_epoch,
                part,
                splitting,
                ..
            } => match self.region.split(named, *part, &self.inputs.granted) {
                Err(NoSplit::Nobody) => Err(Off::Nobody),
                Err(NoSplit::NothingStays) => Err(Off::NothingStays),
                Ok(planned) => {
                    let state = stored(&planned.state);
                    let of_part = stored(&planned.part);
                    if state.len() + of_part.len() + 16 * planned.chunks.len() > limit {
                        Err(Off::TooLarge)
                    } else {
                        let request = StoreRequest::SplitCommit {
                            tick,
                            state,
                            part: SplitPart {
                                chunks: planned.chunks.clone(),
                                state: of_part,
                            },
                            as_epoch: *as_epoch,
                            region: *part,
                        };
                        *splitting = Some(planned);
                        Ok(request)
                    }
                }
            },
        };
        match request {
            Ok(request) => {
                info!(tick, "handing the store a merge or a split");
                self.store.request(request);
                self.reshaping = Some(reshaping);
                self.phase = Phase::Committing;
            }
            Err(why) => self.tick_on(reshaping.done, why),
        }
    }

    /// Goes on as if no merge or split had been asked for, as nothing came of it.
    /// Nothing was said to anyone, no link the runner had was closed and no tick
    /// number was used: the next tick is the one after the region's last, and takes
    /// what links sent while the region stood still and what the store answered
    /// meanwhile.
    fn tick_on(&mut self, done: Done, why: Off) {
        info!(
            tick = self.region.tick_number(),
            ?why,
            "nothing comes of a merge or a split; the region ticks on"
        );
        self.phase = Phase::Running;
        done.off(why);
    }

    /// Acts on the store's answer to the merge or the split it was handed.
    fn take_answer(&mut self, answer: StoreReply) {
        let Some(Reshaping { plan, done }) = self.reshaping.take() else {
            return;
        };
        match (answer, plan) {
            (
                StoreReply::Absorbed {
                    absorbed,
                    chunks,
                    pinned,
                    in_use,
                    lends,
                },
                Plan::Absorb {
                    state: Some(state), ..
                },
            ) => {
                let (granted, delivered) = self.waiting_for_the_tick();
                // The region holds what came with the merge, and what the store had
                // granted it in answer to claims that no tick was told of.
                let held: Vec<ChunkPos> = chunks.into_iter().chain(granted).collect();
                let light = self.region.hand_over_light();
                let loaded =
                    self.region
                        .take_absorbed(state, &held, &pinned, &lends.lent, &lends.lent_away);
                self.begin_anew(loaded.into_iter().chain(delivered), light);
                // The absorbed region's chunks in use are this one's now, and timed.
                let tick = self.region.tick_number();
                self.border.time(in_use, tick);
                // The region that made those splits may be the one that was absorbed
                // or one that has grown: where their lines were says nothing any more.
                self.parted.clear();
                info!(
                    tick = self.region.tick_number(),
                    absorbed = absorbed.0,
                    "the region has absorbed another"
                );
                done.call(Reshaped::Absorbed { absorbed });
            }
            (
                StoreReply::Split { region },
                Plan::Split {
                    splitting: Some(splitting),
                    as_epoch,
                    ..
                },
            ) => {
                // Before the region takes the split, while it still says what its
                // ticks were told: of what the store had delivered, only a chunk held
                // so is kept, whichever of the two regions gets it. No claim was
                // outstanding while the store was waited for, so the grants are those
                // the split was worked out with.
                let (granted, delivered) = self.waiting_for_the_tick();
                let gone: BTreeSet<ChunkPos> = splitting.chunks.iter().copied().collect();
                let waited: BTreeSet<ChunkPos> = granted
                    .iter()
                    .filter(|position| gone.contains(*position))
                    .filter(|position| self.region.knowledge(**position) != Knowledge::Held)
                    .copied()
                    .collect();
                let players = splitting.part.players.len();
                let sides = splitting.sides.clone();
                // The part's chunks are lit anew where they go; this region's keep their
                // light (ADR-0029, section 7).
                let light = self.region.hand_over_light();
                let (loaded, mut part) = self.region.take_split(splitting, &granted);
                // What the runner has of the part's chunks beyond those that were
                // loaded goes with them: what the store had delivered for the coming
                // tick, and what was still warm from an earlier merge or split.
                let (theirs, ours): (Vec<_>, Vec<_>) = delivered
                    .into_iter()
                    .partition(|(position, _)| gone.contains(position));
                let (warm, kept): (BTreeMap<_, _>, BTreeMap<_, _>) = mem::take(&mut self.warm)
                    .into_iter()
                    .partition(|(position, _)| gone.contains(position));
                self.warm = kept;
                let mut of_part: BTreeMap<ChunkPos, Chunk> = part.chunks.into_iter().collect();
                let warm = warm
                    .into_iter()
                    .map(|(position, warm)| (position, warm.chunk));
                for (position, chunk) in theirs.into_iter().chain(warm) {
                    of_part.entry(position).or_insert(chunk);
                }
                part.chunks = of_part.into_iter().collect();
                self.begin_anew(loaded.into_iter().chain(ours), light);
                info!(
                    tick = self.region.tick_number(),
                    part = region.0,
                    players,
                    chunks = gone.len(),
                    waited = waited.len(),
                    "a part of the region has been split off"
                );
                // Until every edge that had a player go has heard of it: what such an
                // edge asks this region for, for somebody it still takes for this
                // region's, is not claimed on the part's side of the line.
                self.parted.insert(region, Parted { sides, gone });
                done.call(Reshaped::Split {
                    region,
                    as_epoch,
                    part,
                });
            }
            // The store gives out region ids, and another split can have taken the
            // one the command named. It changes nothing when it declines, so the same
            // tick can be named again; and it looks at the id last, so nothing else
            // stands in the way.
            (
                StoreReply::Declined {
                    reason: Decline::NotNext { next },
                },
                Plan::Split {
                    named,
                    as_epoch,
                    renamed: false,
                    ..
                },
            ) => {
                info!(
                    part = next.0,
                    "the store has another id for the part of a split"
                );
                let plan = Plan::Split {
                    named,
                    as_epoch,
                    part: next,
                    renamed: true,
                    splitting: None,
                };
                self.reshaping = Some(Reshaping { plan, done });
                self.commit();
            }
            (StoreReply::Declined { reason }, _) => self.tick_on(done, Off::Declined(reason)),
            // An answer to another request than was made: region and store disagree
            // about what was asked, and nothing the runner goes on to do can be
            // trusted.
            (answer, plan) => {
                error!(
                    ?answer,
                    "the world store answered a merge or a split with another answer"
                );
                self.reshaping = Some(Reshaping { plan, done });
                self.give_up();
            }
        }
    }

    /// Takes what waits for the coming tick, which a merge or a split takes the place
    /// of, and returns of it what that tick is given: the chunks the store has granted
    /// the region in answer to claims that no tick was told of, and the chunks it has
    /// delivered for requests of the region's last ticks.
    ///
    /// Everything else is dropped. What a link sent and no tick took was not applied,
    /// is not counted as received, and is sent again by its edge on its next link.
    /// What the store said is another region's is forgotten with everything else the
    /// region believed or had asked. So is what the store said of stays, that one may
    /// enter or that some are dead: the region's first tick after the merge or the
    /// split names every stay it has, and the store says both again
    /// (`docs/adr/0020-one-stay-per-player.md`, section 9). A delivered chunk is passed
    /// on only if the region holds it by what its ticks were told: then it has held
    /// the chunk since it asked for it, and the chunk is what the store has.
    fn waiting_for_the_tick(&mut self) -> (Vec<ChunkPos>, Vec<(ChunkPos, Chunk)>) {
        let inputs = mem::take(&mut self.inputs);
        self.discards.clear();
        let held = |position: ChunkPos| self.region.knowledge(position) == Knowledge::Held;
        let delivered = inputs
            .chunks_loaded
            .into_iter()
            .filter(|(position, _)| held(*position))
            .collect();
        (inputs.granted, delivered)
    }

    /// Makes the runner what it is for a region just restored, once its region has
    /// taken the tick of a merge or a split and so is what a restore makes of its new
    /// state and of what it holds. `in_memory` are the chunks the runner still has
    /// that the region held and may hold on: those that were loaded, and those the
    /// store had delivered. See `docs/adr/0014-merging-and-splitting.md`, section 3.3.
    ///
    /// From here on the runner does what it does after [`RegionRunner::restore`], but
    /// that it has the handle already and has warm chunks. Nothing is published for
    /// the tick: there is no link to hear it. The next tick is an ordinary one, and
    /// answers the hellos of the links that edges make now from the state as of this
    /// tick, with the entries the merge or the split made among the welcome's.
    fn begin_anew(
        &mut self,
        in_memory: impl Iterator<Item = (ChunkPos, Chunk)>,
        light: Vec<(ChunkPos, clustine_sim::KeptLight)>,
    ) {
        let mut light: BTreeMap<ChunkPos, clustine_sim::KeptLight> = light.into_iter().collect();
        // Dropping a link closes it, also one that was attached and not taken up. No
        // ticket is given back for one: the region has none.
        self.links.clear();
        self.refuse_links();
        // Nor has it a ring ticket: the rings are worked out afresh as the links come
        // back and subscribe, and each is handed in again. Kept, they would be taken
        // for handed in, and a region would hold no ring around players who stand still
        // (ADR-0024, section 2).
        self.rings.clear();
        // Edges count as away from this tick, as after a restore, and nothing is
        // received of any beyond what the state has applied.
        let state = self.region.state();
        self.edges = known_edges(&state);
        self.last_inputs = last_inputs(&state);
        // The store has the tick, and every one before it: nothing is pending, unsaved
        // or being loaded. What it could not read stays noted.
        self.committed = state.tick;
        for (position, chunk) in in_memory {
            let since = state.tick;
            let light = light.remove(&position);
            self.warm.entry(position).or_insert(Warm {
                chunk,
                since,
                light,
            });
        }
        let region = &self.region;
        self.warm
            .retain(|position, _| region.knowledge(*position) == Knowledge::Held);
        // What the merge or the split durably took of the arrivals is the store's to
        // forget; those kept for chunks the region holds no longer (a split's part's) are
        // the part's, which the store sends it (`docs/groundwork/n3-plan.md`, section 5,
        // G5).
        self.border.committed_through(state.tick);
        let lost: BTreeSet<ChunkPos> = self
            .border
            .arrivals_waiting(|_| true, |_| false)
            .into_iter()
            .filter(|chunk| !region.holds_for_arrivals(*chunk, &[]))
            .collect();
        self.border.drop_arrivals_of(&lost);
        // The sim has no copies now and is handed them again; its warm chunks count as
        // loaded until they go.
        self.border.began_anew(self.region.loaded_chunks());
        let warm: Vec<ChunkPos> = self.warm.keys().copied().collect();
        self.border.time(warm, state.tick);
        self.phase = Phase::Running;
        self.show_status();
    }

    /// Lets go of the region for good: of the store handle first, which closes the
    /// region at the store once everything asked of it before is done, and then of the
    /// links, so that an edge that finds its link closed finds the region free.
    fn end(&mut self, ended: Ended) {
        self.store = Box::new(Closed);
        self.links.clear();
        self.pending.clear();
        self.refuse_links();
        self.phase = Phase::Ended;
        self.ended = Some(ended);
        self.status.set_ended(ended);
        // A merge or a split that was under way is the next owner's to find done or
        // not: the store still does what it was asked before it closes the region.
        self.answer = None;
        if let Some(reshaping) = self.reshaping.take() {
            reshaping.done.off(Off::StoreLost);
        }
    }

    /// Publishes the ticks that are committed, oldest first, as far as that goes. A link
    /// that turns out to be of no use on the way gets nothing more.
    fn publish_committed(&mut self) {
        while self
            .pending
            .front()
            .is_some_and(|held| !held.needs_commit || held.tick <= self.committed)
        {
            let Some(held) = self.pending.pop_front() else {
                return;
            };
            for (id, message) in held.outgoing {
                self.publish(id, message);
            }
        }
    }

    /// Runs one tick with everything that has arrived since the last one, hands what
    /// changed to the store and makes ready what edges are to hear of it.
    fn tick(&mut self) {
        let before = self.region.tick_number();
        let away: Vec<_> = self
            .edges
            .iter()
            .filter(|(_, edge)| {
                edge.link.is_none() && before.saturating_sub(edge.away_since) >= self.gone_after
            })
            .map(|(id, _)| *id)
            .collect();
        for edge in away {
            info!(edge = edge.0, "an edge has been away for too long");
            self.edges.remove(&edge);
            self.inputs.edges.push(EdgeEvent::Gone { edge });
        }

        // Everything is made ready as of the end of the tick, to be held back as a whole.
        // The answers to hellos come first, and are made from the region as it is before
        // the tick: what earlier ticks put into an outbox was made ready for links that
        // are closed now, and is sent again here.
        let mut outgoing = Vec::new();
        let mut resumed = BTreeSet::new();
        for (id, link) in &mut self.links {
            let (Some(edge), Some(resume)) = (link.edge, link.resume.take()) else {
                continue;
            };
            resumed.insert(*id);
            // The entries that follow the welcome: after a resume those the edge has
            // not seen, and all of them for an edge that is told again since when the
            // region knows it. A state that the coming tick makes has none.
            let state = self.region.edge(edge);
            let (above, since) = match (resume.answer, state) {
                (Answer::Resumed, Some(state)) => (Some(resume.seen), state.since),
                (Answer::ToldAgain, Some(state)) => (Some(0), state.since),
                // The number of the tick that takes the hello, which is the `since`
                // of the state it makes.
                _ => (None, before + 1),
            };
            let entries: Vec<_> = match (above, state) {
                (Some(above), Some(state)) => state
                    .outbox
                    .range((Bound::Excluded(above), Bound::Unbounded))
                    .map(|(number, entry)| (*number, entry.clone()))
                    .collect(),
                _ => Vec::new(),
            };
            let count = u32::try_from(entries.len()).unwrap_or(u32::MAX);
            // Whom the region has for the edge, in the state the entries are of: one
            // answer for each player the hello names, in the hello's order, and then
            // one for every other stay of the edge, in ascending order of the
            // players. A merge and a split put stays into a region that the edge did
            // not send there, so its hello cannot name them. A state that the coming
            // tick makes, or makes anew, has nobody of the edge: whoever is still
            // there is of the edge's past and goes with that tick.
            let from_state = matches!(resume.answer, Answer::Resumed | Answer::ToldAgain);
            let present = |player: PlayerId| {
                // A player of another edge has connected anew through that one; this
                // edge's connection of theirs is of the past.
                let state = self
                    .region
                    .player_state(player)
                    .filter(|state| from_state && state.edge == edge)?;
                Some(Presence::Present {
                    riding: state.riding.map(|riding| riding.vehicle),
                    entity: state.entity_id,
                    pose: state.pose,
                    inventory: state.inventory.clone(),
                    selected_slot: state.selected_slot,
                    last_input: state.last_input,
                    handled: state.handled,
                    flying: state.flying,
                    // For as long as the stay has it: an edge that was not told the
                    // entity finds the stay of its connection by it. See
                    // `docs/adr/0020-one-stay-per-player.md`, section 4.3.
                    attempt: state.attempt,
                    last_death: state.last_death,
                    mode: state.mode,
                })
            };
            // A player the hello names whose join the region has taken and holds as
            // entering, until the store has said where they enter: the edge goes on
            // waiting. Only for a player the hello names. An edge that does not name
            // them has ended that connection, and its leave ends the stay. See
            // `docs/adr/0020-one-stay-per-player.md`, section 4.2.
            let entering = |player: PlayerId| {
                let state = self
                    .region
                    .entering_state(player)
                    .filter(|state| from_state && state.edge == edge)?;
                Some(Presence::Entering {
                    attempt: state.attempt,
                })
            };
            // A player a portal sent here who waits to be placed is entering to the
            // edge, which holds their client between the dimensions until the placing
            // (`docs/groundwork/p-plan.md`, section 16.8). The edge does not read the
            // attempt of such an answer.
            let waiting = |player: PlayerId| {
                let state = self
                    .region
                    .waiting_state(player)
                    .filter(|state| from_state && state.edge == edge)?;
                Some(Presence::Entering {
                    attempt: state.transfer.attempt.unwrap_or(0),
                })
            };
            let answer = |player: PlayerId| {
                let found =
                    (present(player).or_else(|| entering(player))).or_else(|| waiting(player));
                found.unwrap_or(Presence::Absent)
            };
            let mut answers: Vec<_> = resume
                .players
                .iter()
                .map(|player| (*player, answer(*player)))
                .collect();
            if from_state {
                // `last_inputs` has every player of the region as of its last tick.
                let named: BTreeSet<_> = resume.players.iter().copied().collect();
                let others = self.last_inputs.keys().copied();
                let unnamed = others.filter(|player| !named.contains(player));
                answers.extend(unnamed.filter_map(|player| Some((player, present(player)?))));
            }
            // The welcome says how many follow, so that the edge knows when it has
            // heard of everyone.
            let presences = u32::try_from(answers.len()).unwrap_or(u32::MAX);
            // How far the edge's messages are applied in the state that the entries
            // and the presence answers are made from. A state that the coming tick
            // makes, or makes anew, has applied none.
            let applied = match (from_state, state) {
                (true, Some(state)) => state.applied,
                _ => 0,
            };
            let welcome = match resume.answer {
                Answer::Resumed => Welcome::Resumed {
                    entries: count,
                    presences,
                    applied,
                },
                Answer::ToldAgain | Answer::New => Welcome::Unknown {
                    since,
                    entries: count,
                    presences,
                    applied,
                },
            };
            outgoing.push((*id, WorkerToEdge::Welcome(welcome)));
            for (number, entry) in entries {
                outgoing.push((*id, WorkerToEdge::Outbox { number, entry }));
            }
            for (player, answer) in answers {
                outgoing.push((*id, WorkerToEdge::Presence { player, answer }));
            }
        }

        // The entities of the players on their way in the outbox of an edge that this
        // tick resets or forgets, which it reports removed: nobody will pass those
        // players on. They are the ones the region let go, and the ones that arrived
        // for a chunk it believes another's and were sent on.
        let mut orphaned = BTreeSet::new();
        for event in &self.inputs.edges {
            let dropped = match event {
                EdgeEvent::Gone { edge } => self.region.edge(*edge),
                EdgeEvent::Started { edge, start } => {
                    self.region.edge(*edge).filter(|known| known.start < *start)
                }
                EdgeEvent::Confirmed { .. } => None,
            };
            for entry in dropped.iter().flat_map(|edge| edge.outbox.values()) {
                if let Durable::Departed { transfer, .. }
                | Durable::NotMine {
                    what: Misdirected::Arrival { transfer, .. },
                    ..
                } = entry
                {
                    orphaned.insert(transfer.entity_id);
                }
            }
        }

        // Who comes in from another region, to be counted if the region takes them in:
        // the stays it does not have. An arrival of a player it has with a lower
        // entity takes that stay's place and is a player coming in like any other;
        // one of the very stay it has changes nothing.
        let arriving: Vec<_> = self
            .inputs
            .player_changes
            .iter()
            .filter_map(|change| match change {
                PlayerChange::Arrive(_, player, transfer) => {
                    let entity = transfer.entity_id;
                    let has = self.region.player(*player);
                    has.is_none_or(|(present, _)| present != entity)
                        .then_some((*player, entity))
                }
                _ => None,
            })
            .collect();

        self.update_rings();
        // The borders: the copies wanted, asked for and given up, and what of them and of
        // the effects received the tick is given (`docs/adr/0025-effects-between-
        // regions.md`, sections 3 and 5).
        let (wanted, gone, withdrawn) = self.border.want(&self.region.border_chunks());
        if !wanted.is_empty() || !gone.is_empty() {
            self.store.request(StoreRequest::Copies { wanted, gone });
        }
        self.inputs.copies_withdrawn.extend(withdrawn);
        let region = &self.region;
        let next = region.tick_number() + 1;
        // The chunks arrivals wait for that the region holds or is about to hold, not
        // loaded: the sim tickets them, and claims them where it must
        // (`docs/groundwork/n3-plan.md`, section 5, D3).
        let granted = &self.inputs.granted;
        let unreadable = &self.unreadable;
        self.inputs.arrivals_waiting = self.border.arrivals_waiting(
            |chunk| region.holds_for_arrivals(chunk, granted) && !unreadable.contains(&chunk),
            |chunk| region.chunk(chunk).is_some(),
        );
        self.border.inputs(&mut self.inputs, next, |chunk| {
            region.chunk(chunk).is_some()
        });
        // What is collected from here on is for the tick after this one.
        let inputs = mem::take(&mut self.inputs);
        // A chunk handed in that the region does not take, since nothing wants it any
        // more, is in use at the store from its load on: it is dropped after the tick.
        let delivered: Vec<ChunkPos> = inputs
            .chunks_loaded
            .iter()
            .map(|(position, _)| *position)
            .collect();
        self.discards.clear();
        let output = self.region.tick(&inputs);
        #[cfg(test)]
        let output = {
            let mut output = output;
            let injected = mem::take(&mut self.injected_entity_changes);
            output.entity_changes.extend(injected);
            output
        };
        // The arrivals the tick took are kept as taken until its commit is durable, the
        // others kept again: before the tick's returns and lends drop what they drop
        // (`docs/groundwork/n3-plan.md`, section 6).
        self.border
            .took_arrivals(inputs.arrivals.clone(), &output.arrivals_taken, output.tick);
        for edge in self.edges.values_mut() {
            edge.settled = true;
        }
        self.forget_parted();
        for (player, entity) in arriving {
            if let Some(state) = self.region.player_state(player)
                && state.entity_id == entity
            {
                self.status.arrivals.fetch_add(1, Ordering::Relaxed);
                info!(
                    name = %state.name,
                    entity_id = entity.0,
                    "player arrived from another region"
                );
            }
        }

        // What the store is asked after a tick, in this order: the loads, the commit,
        // what the region gives back, what it claims, and the checkpoint if it is time.
        // See `docs/adr/0012-the-tick-on-chunks.md`, section 4.2.
        // Nearest the region's players first, so that the store, which makes chunks
        // for the oldest request first, makes the chunk a joining player stands in
        // and its neighbours before the rim of the view. The simulation's own order
        // is not touched.
        let players: Vec<ChunkPos> = self
            .region
            .entities()
            .map(|player| player.chunk())
            .collect();
        let mut requests = output.chunk_requests;
        requests.sort_by_key(|position| (nearest(&players, *position), *position));
        for position in requests {
            // A chunk that is warm is what the store has, and is handed to the next
            // tick as the store's delivery would be, a tick sooner.
            if let Some(warm) = self.warm.remove(&position) {
                self.inputs.chunks_loaded.push((position, warm.chunk));
                if let Some(light) = warm.light {
                    self.inputs.chunks_kept.push((position, light));
                }
                continue;
            }
            *self.loads.entry(position).or_default() += 1;
            self.border.loading(position);
            self.store.request(StoreRequest::Load { position });
        }
        // Another region can hold and change a chunk that is given back, and one that
        // nobody has asked for in all this time is not about to be shown.
        for position in &output.returns {
            self.warm.remove(position);
        }
        let tick = output.tick;
        self.warm
            .retain(|_, warm| tick.saturating_sub(warm.since) < WARM_FOR);
        // The store is told of every held chunk the region stopped having loaded or
        // warm, so that what is filed for it is dropped (section 5); a return ends the
        // use by itself.
        let warm = &self.warm;
        let dropped = self.border.dropped(
            self.region.loaded_chunks(),
            &delivered,
            &output.returns,
            |chunk| warm.contains_key(&chunk),
            tick,
            WARM_FOR,
        );
        if !dropped.is_empty() {
            self.border.drop_effects_of(&dropped);
            self.store.request(StoreRequest::Dropped {
                chunks: dropped.into_iter().collect(),
            });
        }
        // What the tick made in other regions' land, and the copies beside a chunk that
        // would tick, which is whom the region needs coupling with (section 8).
        self.border.made(output.effects);
        let region = &self.region;
        let (begins, ends) = self.border.beside(&region.viewed_chunks(), |chunk| {
            region.chunk(chunk).is_some()
        });
        if !begins.is_empty() || !ends.is_empty() {
            self.store.request(StoreRequest::Beside { begins, ends });
        }
        // Blocks and block entities' data, in the order the tick changed them
        // (`docs/adr/0026-redstone.md`, section 6).
        let changes: Vec<Change> = output
            .events
            .iter()
            .filter_map(|event| match event {
                RegionEvent::BlockChanged {
                    position, state, ..
                } => Some(Change::Block(*position, *state)),
                RegionEvent::BlockEntityChanged { position, data, .. } => {
                    Some(Change::BlockEntity(*position, data.clone()))
                }
                _ => None,
            })
            .collect();
        // A tick that says something of a stay is committed whatever else it changed:
        // the first tick of a region names every stay it has, changed or not, and the
        // store's answers to that are what removes a dead one and lets an entering one
        // in. See `docs/adr/0020-one-stay-per-player.md`, section 3.
        // The ticks of each chunk whose ticks changed go into the commit whole, as
        // the delays left at the tick's end, so that a crash loses no more of them than
        // of the blocks; a chunk the region has let go of went with its last save
        // (`docs/adr/0024-block-behaviour-in-a-region.md`, section 4).
        let ticks: Vec<_> = output
            .ticks_changed
            .iter()
            .filter(|position| self.region.chunk(**position).is_some())
            .map(|position| (*position, self.region.scheduled_ticks(*position)))
            .collect();
        self.ageing = output.ticks_pending.iter().copied().collect();
        for mob in &output.unreadable_mobs {
            warn!(
                tick = self.region.tick_number(),
                mob = mob.0,
                "a mob's record is not one this build reads; it is left as it is"
            );
        }
        let region = &self.region;
        self.entities_ageing.extend(
            output
                .entities_ageing
                .iter()
                .filter(|position| region.chunk(**position).is_some()),
        );
        // A chunk post-processed in the tick is committed as such, so that a restore
        // does not post-process it again (section 6 of the same record).
        let marks_taken: Vec<ChunkPos> = output
            .marks_taken
            .iter()
            .copied()
            .filter(|position| self.region.chunk(*position).is_some())
            .collect();
        let seams: Vec<_> = output
            .seams_changed
            .iter()
            .filter(|position| self.region.chunk(**position).is_some())
            .map(|position| (*position, self.region.seams(*position)))
            .collect();
        // The block events each chunk keeps, whole, where they changed
        // (`docs/adr/0026-redstone.md`, section 4).
        let events: Vec<_> = output
            .events_changed
            .iter()
            .filter(|position| self.region.chunk(**position).is_some())
            .map(|position| (*position, self.region.kept_events(*position)))
            .collect();
        // The claims on each chunk's points of interest, whole, where they changed
        // (`docs/groundwork/n10-plan.md`, section 3.2).
        let claims: Vec<_> = output
            .claims_changed
            .iter()
            .filter_map(|position| {
                let chunk = self.region.chunk(*position)?;
                Some((*position, chunk.claims().to_vec()))
            })
            .collect();
        // The raids each chunk keeps, whole, where they changed (section 9.1 of the same
        // plan).
        let raids: Vec<_> = output
            .raids_changed
            .iter()
            .filter_map(|position| {
                let chunk = self.region.chunk(*position)?;
                Some((*position, chunk.raids().to_vec()))
            })
            .collect();
        // What the tick did to entities other than players, in its order. The sim makes
        // none for a chunk it does not have loaded (`docs/groundwork/n2-plan.md`,
        // section 11).
        let entity_changes = output.entity_changes;
        for (position, change) in &entity_changes {
            assert!(
                self.region.chunk(*position).is_some(),
                "the tick changed an entity of the chunk {position:?}, which is not loaded: \
                 {change:?}"
            );
        }
        let unchanged = changes.is_empty()
            && entity_changes.is_empty()
            && ticks.is_empty()
            && marks_taken.is_empty()
            && seams.is_empty()
            && events.is_empty()
            && claims.is_empty()
            && raids.is_empty()
            && output.delta.changes_only_the_tick();
        // A tick that takes effects commits, so that the store drops them; one that made
        // some commits them; and a coupled region commits every tick (section 8).
        let taken = output.taken;
        let arrivals = output.arrivals;
        let arrivals_taken = output.arrivals_taken;
        let needs_commit = !(unchanged
            && output.stays.is_empty()
            && taken.is_empty()
            && arrivals.is_empty()
            && arrivals_taken.is_empty()
            && output.travellers.is_empty()
            && !self.border.has_made()
            && !self.border.commits_every_tick());
        if needs_commit {
            let blocks = changes
                .iter()
                .map(|change| change.position().chunk())
                .chain(ticks.iter().map(|(position, _)| *position))
                .chain(marks_taken.iter().copied())
                .chain(seams.iter().map(|(position, _)| *position))
                .chain(events.iter().map(|(position, _)| *position))
                .chain(claims.iter().map(|(position, _)| *position))
                .chain(raids.iter().map(|(position, _)| *position));
            for position in blocks {
                self.unsaved_part(position, Parts::BLOCKS);
            }
            // A chunk not stored yet is written whole by the store's first save of it,
            // whatever part is named (`docs/groundwork/n7l-plan.md`, section 2.4); a
            // stored one's blocks are not rewritten for an animal's step.
            for (position, _) in &entity_changes {
                self.unsaved_part(*position, Parts::ENTITIES);
            }
            self.store.request(StoreRequest::Commit {
                tick: output.tick,
                changes,
                state: stored(&output.delta),
                stays: output.stays,
                ticks,
                marks_taken,
                seams,
                effects: self.border.take_for_commit(),
                taken,
                events,
                claims,
                raids,
                entity_changes,
                arrivals,
                arrivals_taken,
                travellers: output.travellers,
                clock: output.clock,
            });
        }
        // The tick's questions to the portal index, behind its commit, which the answer
        // covers (`docs/groundwork/p-plan.md`, section 15.4). The chunks a waiting
        // traveller needs the region keeps loaded itself, as an arrival's.
        if !output.portal_queries.is_empty() {
            self.store.request(StoreRequest::Portals {
                queries: output.portal_queries,
            });
        }
        if !output.gateway_queries.is_empty() {
            self.store.request(StoreRequest::GatewayExits {
                queries: output.gateway_queries,
            });
        }
        // The counts of its mobs the tick reported, behind its commit
        // (`docs/groundwork/n9-plan.md`, section 9.2).
        if let Some(counts) = output.counts {
            self.store.request(StoreRequest::Counts(counts));
        }
        // And what it says of its land's points of interest
        // (`docs/groundwork/n10-plan.md`, section 3.5).
        if let Some(village) = output.village {
            self.store.request(StoreRequest::Village(village));
        }
        // And the trader's chance a firing of the tick left (section 9.6 of the same
        // plan).
        if let Some((firing, chance)) = output.trader_chance {
            self.store
                .request(StoreRequest::TraderChance { firing, chance });
        }
        // A chunk the region gives back is not loaded and has no unsaved change: its
        // last save went out when its last ticket was let go of, before the tick that
        // dropped it, and so before this. Neither a return nor a claim waits for the
        // commit: what the store does about either for a tick that is then lost, it
        // says when the region is opened again.
        // What the region let go of loaded, for a lend or as a lent chunk given back,
        // goes as the tick left it: saved, with its ticks, before the store hears of it,
        // and what links were served of it waits until the region knows who has it
        // (`docs/adr/0027-lending-at-a-line.md`, sections 3 to 5).
        for position in self.region.letting_go() {
            self.ageing.insert(position);
            self.save(position);
            for link in self.links.values_mut() {
                if let Some(subscription) = link.subscriptions.get_mut(&position)
                    && subscription.condition == Condition::Served
                {
                    subscription.condition = Condition::Waiting;
                    link.waiting.insert(position);
                }
            }
        }
        if !output.returns.is_empty() {
            // What arrived for them is the store's to send again where they are not
            // given back, or to their next holder.
            self.border
                .drop_arrivals_of(&output.returns.iter().copied().collect());
            // The store keeps an unreadable chunk's arrivals for its next holder rather
            // than keeping its return back for them (the plan, section 6). The chunk is
            // asked for again when it is granted anew: the file, or the generator, may
            // have changed by then.
            let unreadable = output
                .returns
                .iter()
                .copied()
                .filter(|chunk| self.unreadable.remove(chunk))
                .collect();
            self.store.request(StoreRequest::Return {
                chunks: output.returns,
                unreadable,
            });
        }
        for answer in output.lend_answers {
            let request = match answer {
                LendAnswer::Refused {
                    lend,
                    chunks,
                    again,
                } => StoreRequest::LendRefused {
                    lend,
                    chunks,
                    again,
                },
                LendAnswer::Lending { lend, chunks } => {
                    self.border
                        .drop_arrivals_of(&chunks.iter().copied().collect());
                    StoreRequest::Lending { lend, chunks }
                }
            };
            self.store.request(request);
        }
        if !output.lends.is_empty() {
            self.lends_in_flight.extend(output.lends.iter().copied());
            self.store.request(StoreRequest::Lend {
                chunks: output.lends,
            });
        }
        if !output.claims.is_empty() {
            self.store.request(StoreRequest::Claim {
                chunks: output.claims,
            });
        }
        // After the commit of the tick the saved chunks show.
        if output.tick.is_multiple_of(self.checkpoint_interval) {
            self.checkpoint();
        }

        // An edge only hears about what happens in chunks it watches, by its
        // subscriptions as they were while the tick ran.
        for (id, link) in &mut self.links {
            let mut events = link.visible(&output.events, &self.region, &orphaned);
            // The beginning of block events alone tells the edge nothing.
            if events == [RegionEvent::BlockEventsBegin] {
                events.clear();
            }
            let carries = events
                .iter()
                .skip_while(|event| **event != RegionEvent::BlockEventsBegin)
                .any(|event| matches!(event, RegionEvent::BlockChanged { .. }));
            if !events.is_empty() || link.carried {
                let delta = WorkerToEdge::TickDelta {
                    tick: output.tick,
                    events,
                };
                outgoing.push((*id, delta));
            }
            link.carried = carries;
        }
        // The clock: every second of game time, in a tick that placed a player, and at
        // once where it moved other than by its tick.
        let placed = output
            .player_events
            .iter()
            .any(|(_, event)| matches!(event, PlayerEvent::Spawned { .. }));
        if output.clock_jumped || output.clock.game_time.is_multiple_of(20) || placed {
            for id in self.links.keys() {
                outgoing.push((
                    *id,
                    WorkerToEdge::Clock {
                        tick: output.tick,
                        clock: output.clock,
                        jumped: output.clock_jumped,
                    },
                ));
            }
        }

        // What concerns a single player goes to the edge they belong to. One who was let
        // go or refused in this very tick is no longer in the region, but then the outbox
        // entry saying so names their edge. That is also how a player is told that they
        // entered who was placed where the region believes the chunk another's and let
        // go in the same tick: a tick lets at most one stay of a player go, and the
        // stay it placed is the only one the region had of them then.
        let edge_of = |player: PlayerId| {
            let present = self.region.player_state(player).map(|state| state.edge);
            present.or_else(|| {
                output
                    .durable
                    .iter()
                    .find_map(|(edge, _, entry)| match entry {
                        Durable::Departed { player: of, .. }
                        | Durable::Refused { player: of, .. }
                            if *of == player =>
                        {
                            Some(*edge)
                        }
                        _ => None,
                    })
            })
        };
        // What concerns an edge goes to the link it has, and nowhere if it has none: an
        // outbox entry stays in the outbox, and the rest the edge works out again when it
        // is back.
        let link_of = |edge: EdgeId| self.edges.get(&edge).and_then(|known| known.link);
        let to_player = |player: PlayerId, event: PlayerEvent| {
            let link = link_of(edge_of(player)?)?;
            Some((link, WorkerToEdge::ToPlayer { player, event }))
        };
        let entry_for = |edge: EdgeId, number: u64, entry: &Durable| {
            let entry = entry.clone();
            Some((link_of(edge)?, WorkerToEdge::Outbox { number, entry }))
        };

        // After the events, so that a player is told that their action was handled only
        // once they have been told what it did. Who entered the world, then what was
        // refused or concerns other regions, then what was acknowledged, then who was let
        // go: a player is told what became of their actions before they are told that
        // they are someone else's from now on. The region numbers the entries of a tick
        // in that order too, so their numbers ascend on the link, which an edge relies
        // on: it remembers only the highest it has seen.
        let departed = |entry: &Durable| matches!(entry, Durable::Departed { .. });
        let (acknowledged, entered): (Vec<_>, Vec<_>) = output
            .player_events
            .into_iter()
            .partition(|(_, event)| matches!(event, PlayerEvent::Acknowledged { .. }));
        for (player, event) in entered {
            outgoing.extend(to_player(player, event));
        }
        for (edge, number, entry) in &output.durable {
            if !departed(entry) {
                outgoing.extend(entry_for(*edge, *number, entry));
            }
        }
        for (player, event) in acknowledged {
            outgoing.extend(to_player(player, event));
        }
        for (edge, number, entry) in &output.durable {
            if let Durable::Departed { transfer, .. } = entry {
                self.status.departures.fetch_add(1, Ordering::Relaxed);
                info!(
                    name = %transfer.name,
                    entity_id = transfer.entity_id.0,
                    "player departed to another region"
                );
                outgoing.extend(entry_for(*edge, *number, entry));
            }
        }

        // The subscriptions that wait are answered by what the region knows of their
        // chunks now that the tick has run, each link's in ascending order of the
        // chunks. That covers a link that subscribed before the store's answer came and
        // one that subscribes to a chunk the region has long known not to hold in the
        // same way. Each answer carries the subscription's number as it was while the
        // tick ran. See `docs/adr/0012-the-tick-on-chunks.md`, section 4.4.
        let unsettled: BTreeSet<ChunkPos> = self
            .links
            .values()
            .flat_map(|link| link.waiting.iter().copied())
            .filter(|position| self.settles_soon(*position))
            .collect();
        for (id, link) in &mut self.links {
            let waiting: Vec<_> = link.waiting.iter().copied().collect();
            for position in waiting {
                let Some(subscription) = link.subscriptions.get_mut(&position) else {
                    continue;
                };
                // A chunk let go of for a lend, or asked to be lent, is answered once the
                // store has said who has it (`docs/adr/0027-lending-at-a-line.md`,
                // section 5).
                if self.region.lending_away(position) || self.lends_in_flight.contains(&position) {
                    continue;
                }
                let ask = subscription.ask;
                let answer = match (self.region.knowledge(position), subscription.kind) {
                    (Knowledge::Held, _) => {
                        // Held and not loaded yet: storage has not delivered it.
                        let Some(chunk) = self.region.chunk(position) else {
                            continue;
                        };
                        // New land is shown settled: a chunk that waits for its
                        // post-processing waits for a neighbour on its way, without
                        // holding what the edge sends (ADR-0024, section 6).
                        if unsettled.contains(&position) {
                            link.hold.remove(&position);
                            continue;
                        }
                        subscription.condition = Condition::Served;
                        // A snapshot shows the state after this tick, so it includes
                        // what the events above already said. The edge has to cope
                        // with hearing it twice.
                        let entities = self
                            .region
                            .entities()
                            .filter(|entity| entity.chunk() == position)
                            .collect();
                        WorkerToEdge::ChunkSnapshot {
                            position,
                            ask,
                            tick: output.tick,
                            // Without the ticks and marks, which are not an edge's.
                            chunk: chunk.for_viewers(),
                            entities,
                        }
                    }
                    // The store has not answered the claim yet. A chunk with a viewer's
                    // ticket that the region knows nothing of cannot be: the ticket has
                    // the region ask at the end of the tick that counts it.
                    (Knowledge::Asked, _) | (Knowledge::Unknown, Ticket::Viewer) => continue,
                    // The subscription and its ticket stay: they are why the region
                    // goes on knowing who holds the chunk, lets a player go to the
                    // holder in the tick they step into it, and names the holder when
                    // its players act there.
                    (Knowledge::Foreign(region), Ticket::Viewer) => {
                        subscription.condition = Condition::Elsewhere(region);
                        WorkerToEdge::Elsewhere {
                            chunk: position,
                            ask,
                            region,
                        }
                    }
                    // A guest is served only what the region holds. Its subscription
                    // ends here, and its ticket is taken back with the coming tick.
                    (_, Ticket::ViewerRing | Ticket::GuestRing) => {
                        unreachable!("a link subscribes as a viewer or a guest")
                    }
                    (Knowledge::Foreign(_) | Knowledge::Unknown, Ticket::Guest) => {
                        link.subscriptions.remove(&position);
                        self.inputs.tickets_removed.push((position, Ticket::Guest));
                        WorkerToEdge::NotMine {
                            chunk: position,
                            ask,
                        }
                    }
                };
                link.waiting.remove(&position);
                // What the edge sent behind its hello may act on the chunk from here
                // on: the region knows whose it is.
                link.hold.remove(&position);
                outgoing.push((*id, answer));
            }
        }

        outgoing.extend(self.progress(&output.delta, &resumed));
        self.pending.push_back(HeldTick {
            tick: output.tick,
            needs_commit,
            outgoing,
        });
        // What links sent behind a hello acts on chunks whose holder the region knows
        // now, and which are loaded if it is the holder itself; and an action that
        // waited for its chunk finds it there, or another region's.
        self.release_held();

        self.show_status();
    }

    /// Whether the chunk at `position` still has marks and a neighbour on its way that
    /// would let it be post-processed: one this region wants (a link subscribes to it,
    /// or it is in a ring), has not loaded, and holds and is loading or generating, or
    /// is claiming, or is about to claim as a viewer's. A neighbour that is another
    /// region's, or that cannot be read, lets the chunk be shown as it is, to be
    /// settled later (`docs/adr/0024-block-behaviour-in-a-region.md`, section 6).
    fn settles_soon(&self, position: ChunkPos) -> bool {
        let Some(chunk) = self.region.chunk(position) else {
            return false;
        };
        if chunk.marks().is_empty() {
            return false;
        }
        let wanted = |neighbour: &ChunkPos| {
            self.rings.contains_key(neighbour)
                || self
                    .links
                    .values()
                    .any(|link| link.subscriptions.contains_key(neighbour))
        };
        // Only a viewer's ticket claims a chunk the region knows nothing of wherever it
        // is; a guest's claims only in a pinned area, and may never.
        let as_viewer = |neighbour: &ChunkPos| {
            self.rings.get(neighbour) == Some(&Ticket::ViewerRing)
                || self.links.values().any(|link| {
                    link.subscriptions
                        .get(neighbour)
                        .is_some_and(|subscription| subscription.kind == Ticket::Viewer)
                })
        };
        let coming = |neighbour: &ChunkPos| match self.region.knowledge(*neighbour) {
            // A warm copy is handed to the tick after the one that asks for it.
            Knowledge::Held => {
                self.loads.contains_key(neighbour)
                    || self.generating.contains(neighbour)
                    || self.warm.contains_key(neighbour)
            }
            Knowledge::Asked => true,
            Knowledge::Unknown => as_viewer(neighbour),
            Knowledge::Foreign(_) => false,
        };
        (-1..=1)
            .flat_map(|dx| (-1..=1).map(move |dz| ChunkPos::new(position.x + dx, position.z + dz)))
            .filter(|neighbour| *neighbour != position)
            .any(|neighbour| {
                self.region.chunk(neighbour).is_none()
                    && !self.unreadable.contains(&neighbour)
                    && wanted(&neighbour)
                    && coming(&neighbour)
            })
    }

    /// Brings what others may read of the region up to date with its last tick.
    fn show_status(&mut self) {
        let tick = self.region.tick_number();
        self.status.tick.store(tick, Ordering::Relaxed);
        let players = self.region.player_count() as u64;
        self.status.players.store(players, Ordering::Relaxed);
        let chunks = self.region.loaded_chunk_count() as u64;
        self.status.chunks.store(chunks, Ordering::Relaxed);
        let held = self.region.held_chunk_count() as u64;
        self.status.held.store(held, Ordering::Relaxed);
        let redstone = self.redstone_at_work();
        let shown = self.status.redstone.lock();
        *shown.unwrap_or_else(PoisonError::into_inner) = redstone;
        let crowds = self.region.crowds();
        if crowds != self.crowds {
            let shown = self.status.crowds.lock();
            *shown.unwrap_or_else(PoisonError::into_inner) = crowds.clone();
            self.crowds = crowds;
        }
    }

    /// Where a circuit worked in the last second, for the coordinator: the chunks beside
    /// another region's chunk the region copies first, each with that region, then the
    /// others with the region itself, at most [`clustine_rpc::REDSTONE_REPORTED`]
    /// (`docs/adr/0027-lending-at-a-line.md`, section 7).
    fn redstone_at_work(&self) -> Vec<(ChunkPos, RegionId)> {
        let Some(own) = self.id else {
            return Vec::new();
        };
        let mut beside = Vec::new();
        let mut others = Vec::new();
        // Entities working at a line count as circuits do: what the region sent across
        // one in the last 600 ticks, where it went if the region knows, and otherwise
        // whoever uses the chunks about it (`docs/adr/0028-entities.md`, section 7;
        // `docs/groundwork/n3-plan.md`, step N3f). The cap of what is reported is shared.
        let mut circuits = self.region.circuit_changes();
        for (position, went) in self.region.entities_at_line() {
            match went {
                Some(to) if to != own => beside.push((position, to)),
                _ => circuits.push(position),
            }
        }
        circuits.sort_unstable();
        circuits.dedup();
        for position in circuits {
            let users: BTreeSet<RegionId> = (-1..=1)
                .flat_map(|dx| (-1..=1).map(move |dz| (dx, dz)))
                .filter_map(|(dx, dz)| {
                    let near = ChunkPos::new(position.x + dx, position.z + dz);
                    self.border.copy_user(near)
                })
                .filter(|user| *user != own)
                .collect();
            if users.is_empty() {
                others.push((position, own));
            }
            beside.extend(users.into_iter().map(|user| (position, user)));
        }
        beside
            .into_iter()
            .chain(others)
            .take(clustine_rpc::REDSTONE_REPORTED)
            .collect()
    }

    /// Makes ready what each edge with a link is told of how far the region has got with
    /// what the edge sent, after the tick `delta` is of: those whose `applied` changed,
    /// those with a player whose last applied input changed, and those whose link is
    /// among `resumed`. The last so that an edge that is back hears how far the region
    /// is even when nothing it sends again is new to the region.
    fn progress(
        &mut self,
        delta: &StateDelta,
        resumed: &BTreeSet<LinkId>,
    ) -> Vec<(LinkId, WorkerToEdge)> {
        let mut changed = BTreeSet::new();
        for (id, edge) in &delta.edges {
            if let (Some(edge), Some(known)) = (edge, self.edges.get_mut(id))
                && known.applied != edge.applied
            {
                known.applied = edge.applied;
                changed.insert(*id);
            }
        }
        let mut inputs: BTreeMap<EdgeId, Vec<(PlayerId, u64)>> = BTreeMap::new();
        for (id, player) in &delta.players {
            let Some(player) = player else {
                self.last_inputs.remove(id);
                continue;
            };
            let previous = self.last_inputs.insert(*id, player.last_input);
            if previous.unwrap_or(0) != player.last_input {
                let of_edge = inputs.entry(player.edge).or_default();
                of_edge.push((*id, player.last_input));
            }
        }
        let mut progress = Vec::new();
        for (id, known) in &self.edges {
            let Some(link) = known.link else {
                continue;
            };
            let inputs = inputs.remove(id).unwrap_or_default();
            if changed.contains(id) || !inputs.is_empty() || resumed.contains(&link) {
                let applied = known.applied;
                progress.push((link, WorkerToEdge::Progress { applied, inputs }));
            }
        }
        progress
    }

    /// Notes that `parts` of the loaded chunk at `position` changed.
    fn unsaved_part(&mut self, position: ChunkPos, parts: Parts) {
        let unsaved = self.unsaved.entry(position).or_insert(Parts::NONE);
        *unsaved = unsaved.with(parts);
    }

    /// Hands the chunk at `position` to the store if it has unsaved changes or ticks
    /// that aged since it was saved, with the parts of it that changed.
    fn save(&mut self, position: ChunkPos) {
        self.save_parts(position, Parts::NONE);
    }

    /// Does what [`RegionRunner::save`] does, with `also` among the parts written if it
    /// is saved at all.
    fn save_parts(&mut self, position: ChunkPos, also: Parts) {
        // Ticks are kept with the blocks.
        let ageing = self.ageing.remove(&position).then_some(Parts::BLOCKS);
        let entities = self
            .entities_ageing
            .remove(&position)
            .then_some(Parts::ENTITIES);
        let Some(mut parts) = self.unsaved.remove(&position).or(ageing).or(entities) else {
            return;
        };
        parts = parts.with(also);
        for more in [ageing, entities].into_iter().flatten() {
            parts = parts.with(more);
        }
        if let Some(chunk) = self.region.saved_chunk(position) {
            // A runner's first save of a chunk is whole: what the store has of it may be
            // a note of the generator's, or nothing (`docs/groundwork/n2-plan.md`,
            // section 5). After it only what changed, so that an animal stepping in an
            // unedited chunk does not rewrite its blocks at every checkpoint; the
            // generated mark stays a statement about the blocks
            // (`docs/groundwork/n7l-plan.md`, section 9 item 5).
            if self.saved_once.insert(position) {
                parts = Parts::ALL;
            }
            // The commit of the region's last tick has been sent, which the store has
            // on disk before it writes the chunk. The chunk goes with its ticks.
            self.store.request(StoreRequest::Save {
                position,
                tick: self.region.tick_number(),
                chunk,
                parts,
            });
        }
    }

    /// Hands every chunk with unsaved changes and the region's whole state to the store,
    /// which keeps them in place of the commits up to here.
    fn checkpoint(&mut self) {
        let mut unsaved: BTreeSet<ChunkPos> = self.unsaved.keys().copied().collect();
        unsaved.extend(self.entities_ageing.iter().copied());
        for position in unsaved {
            self.save(position);
        }
        let region = &self.region;
        self.saved_once
            .retain(|position| region.chunk(*position).is_some());
        self.request_checkpoint();
    }

    fn request_checkpoint(&mut self) {
        self.store.request(StoreRequest::Checkpoint {
            tick: self.region.tick_number(),
            state: stored(&self.region.state()),
        });
    }

    /// The checkpoint at which the region stops ticking: the chunks with ageing ticks
    /// are saved too, so that the store has every tick's delay left as the region
    /// leaves it, which a merge or a split hands on and a resume starts from. Each is
    /// saved whole, both its files written as the region leaves it.
    fn checkpoint_at_stop(&mut self) {
        let mut stopping: BTreeSet<ChunkPos> = self.unsaved.keys().copied().collect();
        stopping.extend(self.ageing.iter().copied());
        stopping.extend(self.entities_ageing.iter().copied());
        for position in stopping {
            self.save_parts(position, Parts::ALL);
        }
        self.request_checkpoint();
    }

    /// Ticks 20 times per second until `stop` is set, then stores what has not been
    /// stored yet. Returns early, and for good, if the store handle is lost, or once the
    /// region is released, which [`Worker::begin_release`] asks for.
    ///
    /// A release is carried on by looking at the store's answers every millisecond, and
    /// never by waiting for one; so are a merge and a split, which are taken from
    /// what [`Worker::reshape`] handed in, as the wish for a release is looked at.
    /// Neither waits for a tick: between two ticks the thread waits for what is
    /// handed in and takes it at once, and while it prepares one of the three it looks
    /// every millisecond whether the store has the checkpoint, and stops the region
    /// then. A tick runs when it is due all the same, and is given what it would have
    /// been given: what is taken up between two ticks asks the store for a checkpoint
    /// of the region as its last tick left it, and takes nothing from a link. If
    /// `stop` is set while one of them is under way, the region is let go of as it
    /// is, without anything more being asked of the store or waited for: this ends as
    /// [`Ended::Abandoned`].
    pub fn run(&mut self, stop: &AtomicBool) -> Ended {
        let mut deadline = Instant::now() + self.pace;
        loop {
            if let Some(ended) = self.ended {
                return ended;
            }
            if stop.load(Ordering::Relaxed) {
                break;
            }
            self.take_orders();
            let before = self.region.tick_number();
            self.step();
            // A tick held for the region's partners is tried again as their progress
            // arrives, and keeps its deadline: it does not lose its slot (section 8).
            // Only the slot's own tick is waited for: once it has run, the next is
            // usually held too, and running it as soon as the partners allow would
            // tick the pair faster than the game's pace wherever commits are slow.
            while self.ended.is_none()
                && self.region.tick_number() == before
                && !stop.load(Ordering::Relaxed)
                && matches!(self.phase, Phase::Running | Phase::Preparing)
                && self.pending.len() < MAX_TICKS_AHEAD
                && self.held_by_partners()
            {
                thread::sleep(COMMIT_POLL);
                self.take_orders();
                self.step();
            }
            if self.stands_still() {
                // No tick is due meanwhile; all there is to do is to take the store's
                // answers. A merge or a split that is through or off goes on ticking
                // from here, late by as long as it took.
                thread::sleep(COMMIT_POLL);
                continue;
            }
            // Between two ticks the runner publishes what the store confirms
            // meanwhile, and takes what it is told to do.
            loop {
                if let Some(ended) = self.ended {
                    return ended;
                }
                // Stopped between two ticks. The tick that was due next is the one
                // that runs when the region ticks on.
                if self.stands_still() {
                    break;
                }
                let now = Instant::now();
                let Some(early) = deadline.checked_duration_since(now) else {
                    if now.duration_since(deadline) > self.pace * MAX_CATCH_UP_TICKS {
                        deadline = now;
                    }
                    break;
                };
                // Whether an answer of the store is waited for: to a commit, or to
                // the flush behind the checkpoint that a stop waits for.
                let awaited = !self.pending.is_empty() || self.phase == Phase::Preparing;
                let wait = if awaited {
                    early.min(COMMIT_POLL)
                } else {
                    early
                };
                match self.commands.recv_timeout(wait) {
                    Ok(order) => self.take_order(order),
                    Err(RecvTimeoutError::Timeout) => {}
                    // Cannot be, as the runner keeps an end to hand things in by.
                    Err(RecvTimeoutError::Disconnected) => thread::sleep(wait),
                }
                self.take_orders();
                // The store's answers are taken as a step takes them before it takes
                // anything from a link: while the region prepares to stop, and, as
                // ever, while a tick waits to be published.
                if (awaited || self.phase == Phase::Preparing) && self.take_replies() {
                    self.publish_committed();
                    if self.phase == Phase::Preparing {
                        self.stop_if_prepared();
                    }
                }
            }
            if self.stands_still() {
                continue;
            }
            deadline += self.pace;
        }
        if self.phase != Phase::Running {
            // Whoever stops a release, a merge or a split has waited long enough for
            // the store. What the store was asked for it still does if it can, before
            // it closes the region: a merge or a split that was handed to it is found
            // done or not by whoever opens the region next.
            warn!(
                tick = self.region.tick_number(),
                "stopped in the middle of a release, a merge or a split; letting go of the \
                 region as it is"
            );
            self.end(Ended::Abandoned);
            return Ended::Abandoned;
        }
        self.checkpoint_at_stop();
        self.store.flush();
        if self.store.is_lost() {
            self.give_up();
            return Ended::StoreLost;
        }
        self.end(Ended::Stopped);
        Ended::Stopped
    }

    /// Takes what other threads have handed in: the wish for a release, and every
    /// command.
    fn take_orders(&mut self) {
        if self.release_asked.load(Ordering::Relaxed) {
            self.begin_release();
        }
        while let Ok(order) = self.commands.try_recv() {
            self.take_order(order);
        }
    }

    fn take_order(&mut self, order: Order) {
        match order {
            Order::Reshape(command) => self.take_command(*command),
            // Looked at by whoever took this, as before every step.
            Order::Release => {}
        }
    }

    /// Makes `end` a link of the runner.
    fn take_up(&mut self, end: WorkerEnd) -> LinkId {
        let id = LinkId(self.next_link);
        self.next_link += 1;
        self.links.insert(
            id,
            EdgeLink {
                end,
                edge: None,
                last: None,
                unknown: false,
                resume: None,
                heard: 0,
                asked: None,
                hold: BTreeSet::new(),
                held: VecDeque::new(),
                subscriptions: BTreeMap::new(),
                waiting: BTreeSet::new(),
                carried: false,
            },
        );
        info!(link = id.0, "an edge attached a link");
        id
    }

    /// Takes everything a link has received: into the inputs of the coming tick, or
    /// aside while the link is held. A link is held while a chunk of its hello is
    /// unanswered, and while the first message of its queue is an action on a block
    /// that waits for its chunk ([`RegionRunner::waits_for_its_chunk`]); behind what
    /// waits, everything waits, in the order it came.
    fn drain(&mut self, id: LinkId) {
        // Taken out meanwhile, so that the links that are left are the other ones.
        let Some(mut link) = self.links.remove(&id) else {
            return;
        };
        loop {
            match link.end.try_recv() {
                Ok(Some(message)) => {
                    let held = !(link.hold.is_empty() && link.held.is_empty())
                        || self.waits_for_its_chunk(&link, &message);
                    if held {
                        link.held.push_back(message);
                    } else if !self.accept(id, &mut link, message) {
                        self.let_go(id, link);
                        return;
                    }
                }
                Ok(None) => {
                    self.links.insert(id, link);
                    return;
                }
                Err(_) => {
                    info!(link = id.0, "an edge closed its link");
                    self.let_go(id, link);
                    return;
                }
            }
        }
    }

    /// Passes on what links sent while they were held, for those that no longer are:
    /// each link's messages in the order they came, up to the first that has to wait.
    fn release_held(&mut self) {
        let free: Vec<_> = self
            .links
            .iter()
            .filter(|(_, link)| link.hold.is_empty() && !link.held.is_empty())
            .map(|(id, _)| *id)
            .collect();
        for id in free {
            let Some(mut link) = self.links.remove(&id) else {
                continue;
            };
            let mut useful = true;
            // A hello among them can hold the link anew, with the chunks it names.
            while useful
                && link.hold.is_empty()
                && let Some(message) = link.held.pop_front()
            {
                if self.waits_for_its_chunk(&link, &message) {
                    link.held.push_front(message);
                    break;
                }
                useful = self.accept(id, &mut link, message);
            }
            if useful {
                self.links.insert(id, link);
            } else {
                self.let_go(id, link);
            }
        }
    }

    /// Whether `message` of `link` is an action on a block that has to wait: the link
    /// has a subscription that waits for a chunk the action is about, and the region
    /// is itself about to serve that chunk. That is a chunk it holds and has not
    /// loaded, or one of an area it is pinned to of which the store has told it
    /// nothing yet. Judged before the chunk is there, the action would be
    /// acknowledged without effect, or passed on without a region for a chunk that
    /// turns out to be this region's own. See
    /// `docs/adr/0014-merging-and-splitting.md`, section 3.6.
    ///
    /// A chunk the region borrows waits so too, whether or not the link waits for it:
    /// one whose lend it has asked for and had no answer of, one granted it by a lend
    /// in the coming tick or lent to it, and not loaded yet. The region keeps a lent
    /// chunk loaded from the grant on, so the wait is for the store's answer and then
    /// for the load. Judged by what the region knew before the grant, the action would
    /// be taken in the tick that takes the grant first, find the chunk held and not
    /// there, and be acknowledged without effect; the wait begins at the ask, since a
    /// grant can come in the step that takes the action, or while taken actions wait
    /// for a tick (`docs/adr/0027-lending-at-a-line.md`, step L6).
    ///
    /// Nothing else waits so. A chunk the region has asked for outside its pinned
    /// areas is, if a client can click it, another region's: the click behind one's
    /// back right after a hand-over is judged at once and goes on without a region
    /// named. A chunk the store cannot read is waited for without end, and holds
    /// nothing.
    fn waits_for_its_chunk(&self, link: &EdgeLink, message: &EdgeMessage) -> bool {
        let chunk = |position: BlockPos| position.chunk();
        let about = match &message.body {
            EdgeToWorker::Input { input, .. } => match off_hand_or_not(input) {
                PlayerInput::Dig { position, .. } | PlayerInput::Destroy { position, .. } => {
                    vec![chunk(*position)]
                }
                // The block that was clicked, and the spot beside the clicked face,
                // where a block is placed.
                PlayerInput::UseItemOn { position, face, .. } => {
                    vec![chunk(*position), chunk(face.neighbour(*position))]
                }
                // Until step W6e a bucket only has the client's guess handled, which
                // needs no chunk.
                PlayerInput::Move { .. }
                | PlayerInput::PerformRespawn
                | PlayerInput::UseItem { .. }
                | PlayerInput::SelectSlot { .. }
                | PlayerInput::SetInventorySlot { .. }
                | PlayerInput::SwapHands
                | PlayerInput::WithOffHand(_)
                | PlayerInput::SetFlying { .. }
                | PlayerInput::Drop { .. }
                | PlayerInput::Throw { .. }
                | PlayerInput::Keys(_)
                | PlayerInput::Interact { .. }
                | PlayerInput::Attack { .. }
                | PlayerInput::MoveVehicle(_)
                | PlayerInput::PaddleBoat { .. }
                | PlayerInput::PickItemFromEntity { .. }
                | PlayerInput::Order(_)
                | PlayerInput::ReleaseUse
                | PlayerInput::SetCraftingSlot { .. } => return false,
                // A click and a close are about the chunks of the menu's container: held
                // while one is on its way, so that a click after a merge waits for the
                // chest rather than being refused (`docs/groundwork/n6-plan.md`, section 8
                // item 6, bounded as section 9 item 3 has it, below).
                PlayerInput::ContainerClick { .. }
                | PlayerInput::CloseContainer { .. }
                | PlayerInput::SelectTrade { .. } => match &message.body {
                    EdgeToWorker::Input { player, .. } => self.region.menu_chunks(*player),
                    _ => return false,
                },
                PlayerInput::OpenVehicleInventory
                | PlayerInput::MenuResync
                | PlayerInput::Punch
                | PlayerInput::LeaveBed
                | PlayerInput::RidingJump { .. }
                | PlayerInput::Appearance(_)
                | PlayerInput::StartFallFlying
                | PlayerInput::Loaded => return false,
            },
            EdgeToWorker::Remote(action) => match &action.step {
                RemoteStep::Break { position }
                | RemoteStep::BreakWith { position, .. }
                | RemoteStep::Attack { position } => vec![chunk(*position)],
                RemoteStep::PlaceAgainst {
                    against, target, ..
                } => vec![chunk(*against), chunk(*target)],
                RemoteStep::Place { target, .. } => vec![chunk(*target)],
                // As for a click of the region's own player.
                RemoteStep::UseOn { click } | RemoteStep::OffHandOn { click } => {
                    let beside = click.face.neighbour(click.position);
                    vec![chunk(click.position), chunk(beside)]
                }
                RemoteStep::PlaceAt { target, .. } => vec![chunk(*target)],
            },
            // A move, an arrival, a join and a leave are about no chunk for this.
            _ => return false,
        };
        about.into_iter().any(|position| {
            let borrowed = (self.lends_in_flight.contains(&position)
                || self.region.borrows(position, &self.inputs.granted))
                && self.region.chunk(position).is_none();
            if borrowed && !self.unreadable.contains(&position) {
                return true;
            }
            link.waiting.contains(&position)
                && !self.unreadable.contains(&position)
                && match self.region.knowledge(position) {
                    Knowledge::Held => self.region.chunk(position).is_none(),
                    Knowledge::Unknown | Knowledge::Asked => self.region.pins(position),
                    Knowledge::Foreign(_) => false,
                }
        })
    }

    /// Handles a message of the link `id`, which is not among `self.links` meanwhile.
    /// Returns false if the link is of no use any more: what it sent is out of order, or
    /// its edge has been replaced.
    ///
    /// A numbered message is passed on to the region once: one whose number is not above
    /// what the region has received of the edge is dropped, as the edge sends again what
    /// it has not heard to be applied, and the next number is taken. Anything else is a
    /// gap, which ends the link. An edge the region did not know when it said hello is
    /// the exception until its first message is taken: it may be an edge the region has
    /// forgotten, which sends what it kept under numbers that mean nothing any more
    /// before it has read that it is unknown, and numbers from 1 again after. Ending the
    /// link on those would end it before the edge has read the answer, and the edge
    /// would never learn to start again. So they are dropped until number 1 comes.
    fn accept(&mut self, id: LinkId, link: &mut EdgeLink, message: EdgeMessage) -> bool {
        let EdgeMessage { number, body } = message;
        if number.is_some() != body.is_numbered() {
            warn!(
                link = id.0,
                ?number,
                "an edge sent a number where none belongs, or none where one does; closing its link"
            );
            return false;
        }
        match body {
            EdgeToWorker::Hello {
                edge,
                start,
                since,
                seen,
                players,
                chunks,
                guests,
            } => {
                let resume = Resume {
                    seen,
                    players,
                    answer: Answer::New,
                };
                return self.hello(id, link, edge, (start, since), resume, (chunks, guests));
            }
            EdgeToWorker::Confirm { number } => {
                link.heard = link.heard.max(number);
                // Without a hello there is no telling whose outbox is meant.
                if let Some(edge) = link.edge {
                    self.inputs
                        .edges
                        .push(EdgeEvent::Confirmed { edge, number });
                }
            }
            EdgeToWorker::Subscribe { ask, chunks } => {
                if !Self::takes_ask(id, link, ask) {
                    return false;
                }
                let mut taken = Taken::new();
                for position in chunks {
                    let for_part = self.subscribe(link, position, Ticket::Viewer, ask);
                    note_taken(&mut taken, for_part);
                }
                say_taken(id, &taken);
            }
            EdgeToWorker::SubscribeAsGuest { ask, chunks } => {
                if !Self::takes_ask(id, link, ask) {
                    return false;
                }
                for position in chunks {
                    self.subscribe(link, position, Ticket::Guest, ask);
                }
            }
            EdgeToWorker::Unsubscribe { ask, chunks } => {
                if !Self::takes_ask(id, link, ask) {
                    return false;
                }
                for position in chunks {
                    if let Some(subscription) = link.subscriptions.remove(&position) {
                        link.waiting.remove(&position);
                        self.release(position, subscription.kind);
                        if let Some(part) = subscription.taken {
                            self.take_back(position, part);
                        }
                    }
                }
            }
            numbered => {
                // What changes the region is some edge's doing, and the region has to know
                // whose: an edge says hello before anything else. One that does not is
                // treated as one whose messages are out of order.
                let (Some(edge), Some(number)) = (link.edge, number) else {
                    warn!(
                        link = id.0,
                        "an edge sent a numbered message before saying hello; closing its link"
                    );
                    return false;
                };
                // A link that has said hello is its edge's link, and such an edge is
                // not forgotten.
                let Some(known) = self.edges.get_mut(&edge) else {
                    return false;
                };
                let next = known.received + 1;
                if link.unknown {
                    if number != next {
                        return true;
                    }
                    link.unknown = false;
                } else {
                    // On one link each number follows the one before. The first may be
                    // one the region has already, but not one beyond the next.
                    let in_order = match link.last {
                        Some(last) => number == last + 1,
                        None => number <= next,
                    };
                    if !in_order {
                        warn!(
                            link = id.0,
                            number,
                            last = ?link.last,
                            received = known.received,
                            "an edge sent a message out of order; closing its link"
                        );
                        return false;
                    }
                }
                link.last = Some(number);
                if number < next {
                    return true;
                }
                known.received = number;
                match self.inputs.applied.iter_mut().find(|(of, _)| *of == edge) {
                    Some((_, applied)) => *applied = number,
                    None => self.inputs.applied.push((edge, number)),
                }
                self.accept_numbered(edge, numbered);
            }
        }
        true
    }

    /// Notes the number of a subscription message of the link `id`. Returns false if it
    /// is not above that of the one before, which ends the link as a numbered message
    /// out of order does: answers carry these numbers, and an edge tells by them
    /// whether an answer is about a subscription as it last asked for it.
    fn takes_ask(id: LinkId, link: &mut EdgeLink, ask: u64) -> bool {
        // The first is above 0, which is the number a hello's chunks have.
        if ask <= link.asked.unwrap_or(0) {
            warn!(
                link = id.0,
                ask,
                last = ?link.asked,
                "an edge numbered a subscription message out of order; closing its link"
            );
            return false;
        }
        link.asked = Some(ask);
        true
    }

    /// Handles the hello of the link `id`, which is not among `self.links` meanwhile.
    /// Returns false if the link is of no use: its edge has been replaced by a later
    /// start, it has said hello before, or it names chunks after the link has sent a
    /// subscription message.
    fn hello(
        &mut self,
        id: LinkId,
        link: &mut EdgeLink,
        edge: EdgeId,
        (start, since): (u64, u64),
        mut resume: Resume,
        (chunks, guests): (Vec<ChunkPos>, Vec<ChunkPos>),
    ) -> bool {
        // A link is one edge's for as long as it lasts, and what a hello sets off happens
        // once per link.
        if link.edge.is_some() {
            warn!(link = id.0, "an edge said hello twice; closing its link");
            return false;
        }
        // The chunks of a hello are the subscription message with the number 0, which
        // nothing comes before.
        if !(chunks.is_empty() && guests.is_empty()) {
            if link.asked.is_some() {
                warn!(
                    link = id.0,
                    "an edge named chunks in a hello after it had subscribed; closing its link"
                );
                return false;
            }
            link.asked = Some(0);
        }
        if self
            .edges
            .get(&edge)
            .is_some_and(|known| known.start > start)
        {
            info!(
                link = id.0,
                edge = edge.0,
                start,
                "an edge said hello that a later start has replaced; closing its link"
            );
            // At once, and not with a tick: nothing of the region is in it. If it does
            // not fit, the edge finds its link closed, which says nearly as much.
            let _ = link
                .end
                .try_send(WorkerToEdge::Welcome(Welcome::Superseded));
            return false;
        }

        // What the region has to tell the edge goes to this link from now on. A link the
        // edge had before is one it has given up, whether or not that has been noticed.
        let others: Vec<_> = self
            .links
            .iter()
            .filter(|(_, other)| other.edge == Some(edge))
            .map(|(other, _)| *other)
            .collect();
        for other in others {
            if let Some(replaced) = self.links.remove(&other) {
                info!(link = other.0, "an edge said hello on another link");
                self.let_go(other, replaced);
            }
        }

        let tick = self.region.tick_number();
        let known = match self.edges.get_mut(&edge) {
            Some(known) if known.start == start => known,
            Some(known) => {
                // The coming tick resets the edge. Nothing the earlier start sent may be
                // applied after that, and the new start numbers from 1.
                known.start = start;
                known.settled = false;
                known.received = 0;
                self.forget_inputs_of(edge);
                self.edges.get_mut(&edge).expect("the edge was just found")
            }
            None => self.edges.entry(edge).or_insert(KnownEdge {
                start,
                settled: false,
                received: 0,
                applied: 0,
                link: None,
                away_since: tick,
            }),
        };
        known.link = Some(id);
        // Since when the region, as it is before the coming tick, knows the edge with
        // this start, if it does.
        let has = self.region.edge(edge).map(|state| state.since);
        resume.answer = match has.filter(|_| known.settled) {
            Some(has) if has == since => Answer::Resumed,
            // The edge never read the welcome that told it since when: nothing it has
            // sent was numbered for the state the region has, and none of it was taken.
            Some(_) if known.received == 0 => Answer::ToldAgain,
            // The edge has lost its `since` after the region took messages from it.
            // Nothing it kept can be trusted to fit what the region has, and the link
            // would drop its messages numbered from 1 as ones it has had, without a
            // word. So the edge is reset as for a higher start.
            Some(_) => {
                warn!(
                    link = id.0,
                    edge = edge.0,
                    start,
                    "an edge no longer knows since when the region knows it; resetting it"
                );
                known.settled = false;
                known.received = 0;
                self.forget_inputs_of(edge);
                self.inputs.edges.push(EdgeEvent::Gone { edge });
                Answer::New
            }
            None => Answer::New,
        };
        info!(
            link = id.0,
            edge = edge.0,
            start,
            answer = ?resume.answer,
            "an edge said hello"
        );
        self.inputs.edges.push(EdgeEvent::Started { edge, start });
        // After anything but a resume the hello's `seen` is a number of a numbering the
        // region does not share, and confirms nothing.
        if resume.answer == Answer::Resumed {
            self.inputs.edges.push(EdgeEvent::Confirmed {
                edge,
                number: resume.seen,
            });
            link.heard = resume.seen;
        }
        link.edge = Some(edge);
        link.unknown = resume.answer != Answer::Resumed;
        link.resume = Some(resume);
        // The subscriptions the link begins with: a viewer's for each of `chunks` and a
        // guest's for each of `guests`. A chunk in both is a viewer's.
        let mut taken = Taken::new();
        for (positions, kind) in [(chunks, Ticket::Viewer), (guests, Ticket::Guest)] {
            for position in positions {
                if link.subscriptions.contains_key(&position) {
                    continue;
                }
                let for_part = self.subscribe(link, position, kind, 0);
                note_taken(&mut taken, for_part);
                // What the edge sends next acts on these chunks, so it waits until each
                // of them is answered: with a snapshot, which is made when the chunk is
                // loaded, or with the word that the region does not hold it. So nothing
                // an edge sends again reaches a tick before the region knows, of every
                // chunk its players can reach, who holds it.
                if !self.unreadable.contains(&position) && !self.generating.contains(&position) {
                    link.hold.insert(position);
                }
            }
        }
        say_taken(id, &taken);
        true
    }

    /// Takes a subscription message of `link` with the number `ask` for the chunk at
    /// `position`: `Subscribe` if `kind` is a viewer's, `SubscribeAsGuest` if a guest's.
    /// See `docs/adr/0012-the-tick-on-chunks.md`, section 4.3, for every case.
    ///
    /// A ticket is counted for the subscription whatever the region knows of the chunk.
    /// One that changes its kind stays in the condition it is in, so that a served one
    /// costs no second snapshot and loses no event.
    ///
    /// Returns the part the chunk is taken to be of, and whether it is one that did
    /// not go with that part's split, if this is a new viewer's subscription of an
    /// edge that has not heard of a split this runner made
    /// ([`RegionRunner::of_a_part`]).
    fn subscribe(
        &mut self,
        link: &mut EdgeLink,
        position: ChunkPos,
        kind: Ticket,
        ask: u64,
    ) -> Option<(RegionId, bool)> {
        let Some(subscription) = link.subscriptions.get_mut(&position) else {
            // As if the store had said so: the coming tick believes it, claims nothing
            // for the chunk while the ticket lasts, and answers that the part holds it.
            let for_part = match kind {
                Ticket::Viewer => self.of_a_part(link, position),
                Ticket::Guest => None,
                Ticket::ViewerRing | Ticket::GuestRing => {
                    unreachable!("a link subscribes as a viewer or a guest")
                }
            };
            let subscription = Subscription {
                kind,
                ask,
                condition: Condition::Waiting,
                taken: for_part.map(|(part, _)| part),
            };
            link.subscriptions.insert(position, subscription);
            link.waiting.insert(position);
            self.inputs.tickets_added.push((position, kind));
            if let Some((part, _)) = for_part {
                // A belief of the same that a subscription which has just ended was
                // to take back with the coming tick is this one's from now on.
                self.inputs
                    .unbelieve
                    .retain(|doubt| *doubt != (position, part));
                self.inputs.foreign.push((position, part));
            }
            return for_part;
        };
        subscription.ask = ask;
        if let Condition::Elsewhere(region) = subscription.condition {
            // The edge asks again. As a viewer it has the region ask the store again,
            // unless the region has heard otherwise since: a belief that has changed is
            // left alone, and is what the link is answered with. As a guest it is
            // answered by what the region knows.
            subscription.condition = Condition::Waiting;
            link.waiting.insert(position);
            if kind == Ticket::Viewer {
                self.inputs.unbelieve.push((position, region));
                // The edge doubts it, so it is no longer this subscription's to keep.
                subscription.taken = None;
            }
        }
        let mut took = None;
        if subscription.kind != kind {
            // The new ticket is counted before the old one is released, so the chunk
            // stays loaded.
            self.inputs.tickets_added.push((position, kind));
            self.inputs
                .tickets_removed
                .push((position, subscription.kind));
            subscription.kind = kind;
            // A guest is told what the region knows, not what the runner took.
            took = subscription.taken.take();
        }
        if let Some(part) = took {
            self.take_back(position, part);
        }
        None
    }

    /// Takes back the belief that the chunk at `position` is `part`'s, which the
    /// runner put in for a subscription that has ended or become a guest's, unless
    /// a subscription of another link has it too. That subscription's link must not
    /// be among `self.links`, or the subscription no longer be marked.
    ///
    /// A belief that is not the store's lasts as long as what it was put in for, and
    /// no longer: left to the tickets alone, it would outlive its link whenever
    /// another ticket is counted for the chunk in the tick that takes the first back,
    /// as when an edge links again within a tick.
    fn take_back(&mut self, position: ChunkPos, part: RegionId) {
        let has = |link: &EdgeLink| {
            let subscription = link.subscriptions.get(&position);
            subscription.is_some_and(|subscription| subscription.taken == Some(part))
        };
        if !self.links.values().any(has) {
            self.inputs.unbelieve.push((position, part));
        }
    }

    /// The part this region takes the chunk at `position` to be of, when the edge of
    /// `link` asks for it as a viewer for the first time: the lowest part of a split
    /// this runner made that the edge has not heard of, on whose side of the line the
    /// chunk is, if the region knows nothing of the chunk. With it, whether the chunk
    /// is one that did not go with the split.
    ///
    /// An edge that has not read that its players were split off names all they see
    /// as this region's to show them. What went with the split the store would call
    /// the part's a tick or two later. What nobody holds yet would be granted to this
    /// region, a row of chunks ahead of those who went, and they would be handed back
    /// to it when they reach it and split off again. So the region claims neither; the
    /// part does, when the edge has read the entry and asks it. See
    /// `docs/adr/0017-the-end-of-the-stripes.md`, section 3.6.2, also for what follows
    /// where the chunk is a third region's or somebody who stayed sees it as well.
    fn of_a_part(&self, link: &EdgeLink, position: ChunkPos) -> Option<(RegionId, bool)> {
        if self.parted.is_empty() {
            return None;
        }
        // A chunk the region already believes a part's can be one this runner took for
        // it, for a subscription that ends with the coming tick: the belief is then
        // kept for this one.
        let believed = match self.region.knowledge(position) {
            Knowledge::Unknown => None,
            Knowledge::Foreign(part) => Some(part),
            Knowledge::Held | Knowledge::Asked => return None,
        };
        // The state is that of the last tick, so it still has an entry the edge has
        // said on this link that it has: the number tells.
        let state = self.region.edge(link.edge?)?;
        let unheard = (Bound::Excluded(link.heard), Bound::Unbounded);
        let parts: BTreeSet<RegionId> = state
            .outbox
            .range(unheard)
            .filter_map(|(_, entry)| match entry {
                Durable::SplitOff { region, .. } => Some(*region),
                _ => None,
            })
            .collect();
        let of = parts.into_iter().find_map(|part| {
            let parted = self.parted.get(&part)?;
            let went = parted.gone.contains(&position);
            let goes = went || (!self.region.pins(position) && parted.sides.goes(position));
            goes.then_some((part, !went))
        });
        of.filter(|(part, _)| believed.is_none_or(|believed| believed == *part))
    }

    /// Forgets the splits every edge has heard of: those no outbox names any more,
    /// each edge that had a player go having confirmed the entry, been forgotten or
    /// started anew.
    fn forget_parted(&mut self) {
        if self.parted.is_empty() {
            return;
        }
        let named: BTreeSet<RegionId> = self
            .edges
            .keys()
            .filter_map(|edge| self.region.edge(*edge))
            .flat_map(|state| state.outbox.values())
            .filter_map(|entry| match entry {
                Durable::SplitOff { region, .. } => Some(*region),
                _ => None,
            })
            .collect();
        self.parted.retain(|part, _| named.contains(part));
    }

    /// Drops from the coming tick's inputs everything that came from `edge`.
    fn forget_inputs_of(&mut self, edge: EdgeId) {
        let mut discards: Vec<_> = self
            .discards
            .iter()
            .filter(|(from, ..)| *from == edge)
            .map(|(_, entity, chunk)| (*entity, *chunk))
            .collect();
        self.discards.retain(|(from, ..)| *from != edge);
        self.inputs.player_changes.retain(|change| match change {
            PlayerChange::Join(from, _)
            | PlayerChange::Leave(from, ..)
            | PlayerChange::Arrive(from, ..) => *from != edge,
            PlayerChange::Discard { entity, chunk, .. } => {
                let found = discards.iter().position(|of| *of == (*entity, *chunk));
                found.map(|index| discards.swap_remove(index)).is_none()
            }
        });
        self.inputs.inputs.retain(|(from, ..)| *from != edge);
        self.inputs.remote_actions.retain(|(from, _)| *from != edge);
        self.inputs.applied.retain(|(of, _)| *of != edge);
    }

    /// Passes a numbered message on to the region, as one that came from `edge`.
    fn accept_numbered(&mut self, edge: EdgeId, body: EdgeToWorker) {
        match body {
            EdgeToWorker::PlayerJoin(join) => {
                self.inputs.change(PlayerChange::Join(edge, join));
            }
            EdgeToWorker::PlayerLeave {
                player,
                entity,
                attempt,
            } => {
                self.inputs
                    .change(PlayerChange::Leave(edge, player, entity, attempt));
            }
            EdgeToWorker::PlayerArrive { player, transfer } => {
                self.inputs
                    .change(PlayerChange::Arrive(edge, player, transfer));
            }
            EdgeToWorker::Remote(action) => {
                self.inputs.remote_actions.push((edge, action));
            }
            EdgeToWorker::Discard {
                player,
                entity,
                hops,
                chunk,
            } => {
                self.discards.push((edge, entity, chunk));
                self.inputs.change(PlayerChange::Discard {
                    player,
                    entity,
                    hops,
                    chunk,
                });
            }
            EdgeToWorker::Input {
                player,
                entity,
                number,
                input,
            } => {
                // The region ignores what comes through another edge than the player's.
                self.inputs.input(edge, player, entity, number, input);
            }
            // Not numbered; `accept` handles them.
            EdgeToWorker::Hello { .. }
            | EdgeToWorker::Confirm { .. }
            | EdgeToWorker::Subscribe { .. }
            | EdgeToWorker::Unsubscribe { .. }
            | EdgeToWorker::SubscribeAsGuest { .. } => {}
        }
    }

    /// Gives back the ticket of the kind `kind` of a link that no longer needs the
    /// chunk at `position`. That link must not be among `self.links`, or no longer be
    /// subscribed.
    fn release(&mut self, position: ChunkPos, kind: Ticket) {
        let needs = |link: &EdgeLink| link.subscriptions.contains_key(&position);
        if !self.links.values().any(needs) && !self.rings.contains_key(&position) {
            // The region drops the chunk at the start of the coming tick, before
            // anything else can change it, so this is its final state. It is also what
            // the store has of the chunk when the region gives it back, which it can
            // only after that tick.
            self.save(position);
        }
        self.inputs.tickets_removed.push((position, kind));
    }

    /// Works the rings out again from the links' subscriptions and what the region
    /// holds, and hands the difference to the region as tickets: a new kind is added
    /// before the old is let go, and a ring that goes is let go through
    /// [`Self::release`], which saves the chunk where no ticket is left on it.
    fn update_rings(&mut self) {
        if !self.with_rings {
            return;
        }
        let mut subscribed: BTreeMap<ChunkPos, Ticket> = BTreeMap::new();
        for link in self.links.values() {
            for (position, subscription) in &link.subscriptions {
                let kind = subscribed.entry(*position).or_insert(subscription.kind);
                if subscription.kind == Ticket::Viewer {
                    *kind = Ticket::Viewer;
                }
            }
        }
        let mut rings: BTreeMap<ChunkPos, Ticket> = BTreeMap::new();
        for (position, kind) in &subscribed {
            if self.region.knowledge(*position) != Knowledge::Held {
                continue;
            }
            let ring = if *kind == Ticket::Viewer {
                Ticket::ViewerRing
            } else {
                Ticket::GuestRing
            };
            for dx in -1..=1 {
                for dz in -1..=1 {
                    let beside = ChunkPos::new(position.x + dx, position.z + dz);
                    if subscribed.contains_key(&beside) {
                        continue;
                    }
                    let entry = rings.entry(beside).or_insert(ring);
                    if ring == Ticket::ViewerRing {
                        *entry = Ticket::ViewerRing;
                    }
                }
            }
        }
        let before = mem::replace(&mut self.rings, rings);
        for (position, kind) in &self.rings {
            if before.get(position) != Some(kind) {
                self.inputs.tickets_added.push((*position, *kind));
            }
        }
        for (position, kind) in before {
            if self.rings.get(&position) != Some(&kind) {
                self.release(position, kind);
            }
        }
    }

    /// Forgets a link that is of no use any more and has been taken out of
    /// `self.links`: its chunks are no longer needed, and what it sent while it was held
    /// is dropped, which its edge sends again. Its edge's players stay: the edge comes
    /// back for them, or is gone after a while.
    fn let_go(&mut self, id: LinkId, link: EdgeLink) {
        if let Some(known) = link.edge.and_then(|edge| self.edges.get_mut(&edge))
            && known.link == Some(id)
        {
            known.link = None;
            known.away_since = self.region.tick_number();
        }
        for (position, subscription) in link.subscriptions {
            self.release(position, subscription.kind);
            if let Some(part) = subscription.taken {
                self.take_back(position, part);
            }
        }
    }

    /// Sends `message` to a link without ever waiting for it. An edge that cannot keep
    /// up loses its link rather than slowing the region down.
    fn publish(&mut self, id: LinkId, message: WorkerToEdge) {
        let Some(link) = self.links.get(&id) else {
            return;
        };
        if let Err(error) = link.end.try_send(message) {
            warn!(link = id.0, %error, "giving up on a link to an edge");
            if let Some(link) = self.links.remove(&id) {
                self.let_go(id, link);
            }
        }
    }
}

/// What a runner keeps for the edges that a region's state knows, when it begins with
/// that state: after a restore, and after a merge or a split. Edges count as away from
/// the state's tick, however long they were before: they could not have had a link to
/// a region that was not running, and a merge or a split closes every link. Nothing is
/// received of an edge beyond what the state has applied.
fn known_edges(state: &RegionState) -> BTreeMap<EdgeId, KnownEdge> {
    let mut edges = BTreeMap::new();
    for (id, edge) in &state.edges {
        let known = KnownEdge {
            start: edge.start,
            settled: true,
            received: edge.applied,
            applied: edge.applied,
            link: None,
            away_since: state.tick,
        };
        edges.insert(*id, known);
    }
    edges
}

/// The number of the last applied input of each player of `state`.
fn last_inputs(state: &RegionState) -> BTreeMap<PlayerId, u64> {
    let players = state.players.iter();
    players
        .map(|(id, player)| (*id, player.last_input))
        .collect()
}

/// The whole state of a region that is to be absorbed, as [`Reshape::Absorb`] takes
/// it: what the store has of the region, `restored` being what it returned when the
/// region was opened for that and `handle` the handle it came with.
///
/// The store declines a merge while a commit of either region is not covered by a
/// checkpoint. A released region has none. One whose owner lost the store on its way
/// out has, and for that one this asks the store for a checkpoint of the state and
/// waits until the store has it. The store has put the block changes of those commits
/// into the chunks before it answered the hello, so the checkpoint needs no save.
///
/// This waits for the store for as long as the store neither answers nor closes the
/// handle. A handle that is lost on the way shows in the merge being declined.
pub fn absorbable(handle: &StoreHandle, restored: Restored) -> Result<RegionState, RestoreError> {
    let tick = restored.tick();
    let uncovered = !restored.deltas.is_empty();
    let state = restored_state(restored)?;
    if uncovered {
        handle.request(StoreRequest::Checkpoint {
            tick,
            state: stored(&state),
        });
        handle.flush();
    }
    Ok(state)
}

/// What the store says of the region's chunks when it opens the region: the chunks it
/// has granted the region, without the ticks they are held from, which only the store
/// needs, and the areas the region is pinned to.
fn holdings(restored: &Restored) -> Holdings {
    Holdings {
        held: restored.held.iter().map(|(chunk, _)| *chunk).collect(),
        pinned: restored.pinned.clone(),
        lent: restored.lends.lent.clone(),
        lent_away: restored.lends.lent_away.clone(),
    }
}

/// The state of a region as the store has it: its last checkpoint, or that of a region
/// that has never run, with every commit since applied.
///
/// What was stored by a build with another form of state cannot be read. Everything up
/// to the last such item is dropped: the region is as one that has never run, at the
/// tick of that item, and what was stored behind it is applied to that. Players and
/// edges do not outlive a server that is replaced by another build, and the chunks have
/// every block. The same comes out every time the region is opened, until its next
/// checkpoint replaces what was dropped.
///
/// The block of entity keys is the one the store names, whatever the state says, and
/// the next key its first: the store gives a fresh block at every hello, so that no key
/// is given twice in the life of a world, also where a durable tick that gave keys out
/// was lost with its machine, or a state was dropped for another build
/// (`docs/adr/0028-entities.md`, section 1).
fn restored_state(restored: Restored) -> Result<RegionState, RestoreError> {
    let fresh = |tick| {
        let mut state = RegionState::new(restored.entity_ids);
        state.tick = tick;
        state
    };
    let mut state = match &restored.state {
        // That of a region that never ran, whatever its bytes are: a runner that is
        // stopped before its first tick stores one, and one of an earlier build begins
        // with the zero of its tick, which is not the zero in front of a format number.
        Some(stored) if stored.tick == 0 => fresh(0),
        Some(stored) => match sort_stored(stored.tick, &stored.state)? {
            Stored::Current(bytes) => {
                postcard::from_bytes(bytes).map_err(|error| RestoreError::State {
                    tick: stored.tick,
                    error,
                })?
            }
            Stored::Before => {
                warn!(
                    tick = stored.tick,
                    "the stored state of the region is of an earlier build; starting without it"
                );
                fresh(stored.tick)
            }
        },
        None => fresh(0),
    };
    for stored in &restored.deltas {
        match sort_stored(stored.tick, &stored.state)? {
            Stored::Current(bytes) => {
                let delta: StateDelta =
                    postcard::from_bytes(bytes).map_err(|error| RestoreError::Delta {
                        tick: stored.tick,
                        error,
                    })?;
                state.apply(&delta);
            }
            Stored::Before => {
                warn!(
                    tick = stored.tick,
                    "a stored change of the region's state is of an earlier build; starting \
                     without what came before it"
                );
                state = fresh(stored.tick);
            }
        }
    }
    state.entity_ids = restored.entity_ids;
    state.next_entity_id = restored.entity_ids.first;
    // The world's clock only moves forward: the later of the state's and the store's.
    state.clock = state.clock.later(restored.clock);
    Ok(state)
}

/// The use within a use with the off hand, which is about the same chunks; any other
/// input as it is.
fn off_hand_or_not(input: &PlayerInput) -> &PlayerInput {
    match input {
        PlayerInput::WithOffHand(used) => used,
        input => input,
    }
}

/// A region running on its own thread.
pub struct Worker {
    thread: JoinHandle<Ended>,
    stop: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
    commands: Sender<Order>,
}

impl Worker {
    /// Starts ticking `runner` on a new thread. The thread ends by itself if the store
    /// handle is lost, which [`RegionStatus::store_lost`] shows.
    pub fn spawn(runner: RegionRunner) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let release = Arc::clone(&runner.release_asked);
        let commands = runner.command.clone();
        let thread = thread::Builder::new()
            .name("region".to_owned())
            .spawn({
                let stop = Arc::clone(&stop);
                let mut runner = runner;
                move || {
                    let ended = runner.run(&stop);
                    info!(?ended, "region stopped");
                    ended
                }
            })
            .expect("spawning the region thread");
        Self {
            thread,
            stop,
            release,
            commands,
        }
    }

    /// Hands `reshape` to the region's thread, which takes it as soon as it is not in
    /// the middle of a step and without waiting for its next tick, and returns at
    /// once; see [`RegionRunner::reshape`] for what becomes of it.
    ///
    /// `done` is called with the outcome, once, and never for [`Reshape::Prepare`]: on
    /// the region's thread, so it must not wait for anything; a closure that sends
    /// into a channel is what it is meant to be. If the thread has ended, or ends
    /// before it has taken the command, the outcome is that nothing came of it, and
    /// `done` is called on whichever thread finds that out.
    pub fn reshape(&self, reshape: Reshape, done: Box<dyn FnOnce(Reshaped) + Send>) {
        // A thread that has ended takes nothing more. The command that comes back is
        // dropped, which calls `done`.
        let _ = self
            .commands
            .send(Order::Reshape(Box::new(Command::new(reshape, done))));
    }

    /// Asks the region thread to release the region, as [`RegionRunner::begin_release`]
    /// describes, and returns at once. The thread ends by itself when the release is
    /// done or the store handle is lost; [`RegionStatus::ended`] and
    /// [`Worker::is_finished`] show that, and [`Worker::release`] or [`Worker::stop`]
    /// return how it ended.
    ///
    /// The release waits for the store for as long as the store neither answers nor
    /// closes. [`Worker::stop`] ends that wait.
    pub fn begin_release(&self) {
        self.release.store(true, Ordering::Relaxed);
        // So that the thread looks now, and not when its next tick is due. One that
        // has ended takes nothing more.
        let _ = self.commands.send(Order::Release);
    }

    /// Releases the region and waits until that is done: [`Worker::begin_release`], and
    /// then for the thread to end. Returns [`Ended::Released`] if the region is another
    /// owner's to open now, restored from a state file alone, and [`Ended::StoreLost`]
    /// if the store handle was lost before or on the way; either way the links are
    /// closed, the store handle is dropped and the thread is gone.
    ///
    /// This blocks for as long as the release takes, which is without a limit if the
    /// store neither answers nor closes. Whoever has a limit asks with
    /// [`Worker::begin_release`], watches [`RegionStatus::ended`] and calls
    /// [`Worker::stop`] when the time is up.
    pub fn release(self) -> Ended {
        self.begin_release();
        // As in `stop`.
        self.thread.join().unwrap_or(Ended::StoreLost)
    }

    /// Whether the region thread has ended, by itself or because it was asked to.
    pub fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }

    /// Stops ticking, waits for the thread to finish its current tick and to store what
    /// has changed, and says how the runner ended. Its links are closed by then.
    ///
    /// A worker that is releasing its region, or is in the middle of a merge or a
    /// split, stores nothing more and does not wait for the store: it lets go of the
    /// region as it is, which is [`Ended::Abandoned`]. One that has ended already says
    /// how.
    pub fn stop(self) -> Ended {
        self.stop.store(true, Ordering::Relaxed);
        // A panic in the region thread has already been reported by the panic hook, and
        // what such a region had not committed is as lost as with a lost store.
        self.thread.join().unwrap_or(Ended::StoreLost)
    }
}

#[cfg(test)]
mod tests;
