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
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use clustine_rpc::link::WorkerEnd;
use clustine_rpc::{
    Crowds, Decline, EdgeMessage, EdgeToWorker, Off, Presence, Restored, SplitPart, StoreReply,
    StoreRequest, Welcome, WorkerToEdge,
};
use clustine_sim::api::{PlayerInput, RegionEvent};
use clustine_sim::{
    Durable, EdgeEvent, Holdings, Knowledge, Misdirected, NoSplit, Part, PlayerChange, PlayerEvent,
    Region, RegionConfig, RegionState, RemoteStep, Splitting, StateDelta, TickInputs, Ticket,
};
use clustine_world::{BlockPos, Chunk, ChunkPos, EdgeId, EntityId, PlayerId, RegionId};
use clustine_worldstore::StoreHandle;
use tracing::{error, info, warn};

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
}

impl RegionStatus {
    /// The chunks with players in them after the last tick that ran, each with how
    /// many, in ascending order.
    pub fn crowds(&self) -> Crowds {
        // Whoever held the lock only ever replaced the whole list.
        let crowds = self.crowds.lock().unwrap_or_else(PoisonError::into_inner);
        crowds.clone()
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
    /// has no outcome.
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
    /// A checkpoint and a flush have been asked for. The region ticks and serves its
    /// links until the flush is answered.
    Preparing,
    /// The region ticks no more and takes nothing from its links, and links attached
    /// now are closed. The ticks that ran are published as their commits are confirmed.
    Settling,
    /// A second checkpoint and a flush have been asked for.
    Closing,
    /// Only for a merge or a split: the store has been handed it, and has not answered.
    Committing,
}

/// How far a runner is with releasing its region, or with merging or splitting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Nobody has asked for anything of the kind, or what was asked is done or off.
    Running,
    /// The first checkpoint and a flush have been asked for. The region ticks on and
    /// serves its links until the store has answered the flush, so that the checkpoint
    /// that is taken while players stand still is a small one.
    Preparing,
    /// The region ticks no more and takes nothing from its links. It waits for the store
    /// to confirm the commits of the ticks that have run, and publishes them.
    Settling,
    /// The last checkpoint and a flush have been asked for.
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
pub const STATE_FORMAT: u8 = 3;

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

    /// What the edge is to hear of `events`: what happened in chunks it watches, and
    /// that the entities among `orphaned` are gone.
    fn visible(
        &self,
        events: &[RegionEvent],
        region: &Region,
        orphaned: &BTreeSet<EntityId>,
    ) -> Vec<RegionEvent> {
        let mut visible = Vec::new();
        for event in events {
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
    /// Whether a commit was sent for the tick. A tick that changed nothing sends none and
    /// counts as committed as soon as the tick before it is.
    needs_commit: bool,
    /// What goes to which link, in the order it is to be published.
    outgoing: Vec<(LinkId, WorkerToEdge)>,
}

/// Runs one region for the edges that are linked to it.
pub struct RegionRunner {
    region: Region,
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
    /// How many loads of each chunk the store has been asked for and has not answered.
    /// The store answers the loads of one chunk in the order they were asked, so an
    /// answer that leaves some is to a request the region has dropped since; see
    /// [`RegionRunner::answers_the_latest_load`].
    loads: BTreeMap<ChunkPos, u32>,
    /// [`Region::crowds`] after the last tick, to tell when it changes.
    crowds: Crowds,
    /// Loaded chunks that have changed since they were loaded or last stored.
    unsaved: BTreeSet<ChunkPos>,
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
    /// Commands that were handed in from another thread and have not been taken.
    commands: Receiver<Command>,
    /// Kept to give to a [`Worker`].
    command: Sender<Command>,
    /// [`MAX_RESHAPE_BYTES`], unless a test has lowered it.
    max_reshape_bytes: usize,
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
}

impl RegionRunner {
    /// A runner for the region as the world store has it: `restored` is what the store
    /// returned when the region was opened, and `store` the handle it came with. The
    /// region carries on after the last tick the store has; one that was never opened
    /// before starts with the entity ids the store issued to it. It has no links until
    /// some are attached through [`RegionRunner::links`].
    pub fn restore(
        config: RegionConfig,
        store: StoreHandle,
        restored: Restored,
    ) -> Result<Self, RestoreError> {
        let holdings = holdings(&restored);
        let state = restored_state(restored)?;
        Ok(Self::with_store(
            Region::restore(config, state, holdings),
            Box::new(store),
        ))
    }

    /// A runner that carries on with `region`, of which `store` has everything up to its
    /// last tick.
    fn with_store(region: Region, store: Box<dyn RegionStore>) -> Self {
        let (attach, attached) = mpsc::channel();
        let (command, commands) = mpsc::channel();
        let state = region.state();
        let mut runner = Self {
            region,
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
            loads: BTreeMap::new(),
            crowds: Crowds::new(),
            unsaved: BTreeSet::new(),
            checkpoint_interval: DEFAULT_CHECKPOINT_INTERVAL,
            gone_after: DEFAULT_GONE_AFTER,
            warm: BTreeMap::new(),
            lost: false,
            phase: Phase::Running,
            reshaping: None,
            answer: None,
            commands,
            command,
            max_reshape_bytes: MAX_RESHAPE_BYTES,
            ended: None,
            flushes_asked: 0,
            flushes_answered: 0,
            release_asked: Arc::new(AtomicBool::new(false)),
            status: Arc::new(RegionStatus::default()),
        };
        runner.show_status();
        runner
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
        let warm = |(position, chunk)| (position, Warm { chunk, since });
        runner.warm = part.chunks.into_iter().map(warm).collect();
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
                if !self.take_replies() {
                    return;
                }
                if self.flushes_answered == self.flushes_asked {
                    info!(
                        tick = self.region.tick_number(),
                        reshaping = self.reshaping.is_some(),
                        "the store has the first checkpoint of a release, a merge or a split; \
                         the region stops ticking"
                    );
                    self.phase = Phase::Settling;
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
        if self.pending.len() < MAX_TICKS_AHEAD {
            self.tick();
            // A tick that changed nothing has nothing to wait for.
            self.publish_committed();
        }
    }

    /// Takes the store's answers. Returns false if the store handle is lost, which
    /// stops the runner.
    fn take_replies(&mut self) -> bool {
        while let Some(reply) = self.store.try_reply() {
            match reply {
                StoreReply::Loaded { position, chunk } => {
                    if self.answers_the_latest_load(position) {
                        self.inputs.chunks_loaded.push((position, chunk));
                    }
                }
                // The region goes on waiting for the chunk, which leaves a hole in the
                // world rather than a chunk that would overwrite what was built there.
                // Nothing else waits for it.
                StoreReply::Unreadable { position } => {
                    error!(?position, "a chunk cannot be read from the world store");
                    if self.answers_the_latest_load(position) {
                        self.unreadable.insert(position);
                        for link in self.links.values_mut() {
                            link.hold.remove(&position);
                        }
                        self.release_held();
                    }
                }
                // The store's answers to the claims of earlier ticks, in the order of
                // those claims. They go into the coming tick whenever that runs: a chunk
                // the region has asked for is asked until it is answered, so an answer
                // that was dropped would leave whoever waits for the chunk waiting for
                // good. Only a runner that never ticks again lets them lie.
                StoreReply::Claimed { granted, foreign } => {
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
                StoreReply::Committed { tick } => self.committed = self.committed.max(tick),
                StoreReply::Flushed => self.flushes_answered += 1,
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
    ///    are attached from now on. It waits until the store has confirmed the commit of
    ///    every tick that ran, and publishes what those ticks produced, in order.
    /// 3. It checkpoints once more, which is what changed since a moment ago, waits
    ///    until the store has that, closes the region at the store and then its links,
    ///    and has ended as [`Ended::Released`].
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
        self.phase = Phase::Preparing;
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
    /// [`Reshape::Prepare`] asks the store for an ordinary checkpoint and nothing else,
    /// and that only of a runner that neither releases nor reshapes its region. `done`
    /// is never called for it.
    ///
    /// A merge and a split go through the stages of a release
    /// ([`RegionRunner::stage`]) and one more:
    ///
    /// 1. The region goes on ticking and serving its links until the store has a
    ///    checkpoint of it.
    /// 2. Then it ticks no more, takes nothing more from its links and closes links
    ///    that are attached from now on. The ticks that ran are published as the store
    ///    confirms them.
    /// 3. It checkpoints once more. When the store has that, it has answered everything
    ///    asked of it before: no commit of the region is left that a checkpoint does
    ///    not cover, every claim is answered, and nothing is being loaded or unsaved.
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
                // What a lost handle does not take, the next step finds out.
                self.checkpoint();
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
        info!(
            tick = self.region.tick_number(),
            unsaved = self.unsaved.len(),
            "bringing the store up to date for a merge or a split"
        );
        self.checkpoint();
        self.ask_for_flush();
        self.phase = Phase::Preparing;
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
            Phase::Settling if self.pending.is_empty() => {
                // Only now: the store keeps this state in place of every commit up to
                // the region's last tick, so those commits have to be confirmed first.
                self.checkpoint();
                self.ask_for_flush();
                self.phase = Phase::Closing;
            }
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
            } => match self.region.split(named, *part) {
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
                },
                Plan::Absorb {
                    state: Some(state), ..
                },
            ) => {
                let (granted, delivered) = self.waiting_for_the_tick();
                // The region holds what came with the merge, and what the store had
                // granted it in answer to claims that no tick was told of.
                let held: Vec<ChunkPos> = chunks.into_iter().chain(granted).collect();
                let loaded = self.region.take_absorbed(state, &held, &pinned);
                self.begin_anew(loaded.into_iter().chain(delivered));
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
                let (granted, delivered) = self.waiting_for_the_tick();
                let gone: BTreeSet<ChunkPos> = splitting.chunks.iter().copied().collect();
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
                self.begin_anew(loaded.into_iter().chain(ours));
                info!(
                    tick = self.region.tick_number(),
                    part = region.0,
                    "a part of the region has been split off"
                );
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
    /// region believed or had asked. A delivered chunk is passed on only if the region
    /// holds it by what its ticks were told: then it has held the chunk since it asked
    /// for it, and the chunk is what the store has.
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
    fn begin_anew(&mut self, in_memory: impl Iterator<Item = (ChunkPos, Chunk)>) {
        // Dropping a link closes it, also one that was attached and not taken up. No
        // ticket is given back for one: the region has none.
        self.links.clear();
        self.refuse_links();
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
            self.warm.entry(position).or_insert(Warm { chunk, since });
        }
        let region = &self.region;
        self.warm
            .retain(|position, _| region.knowledge(*position) == Knowledge::Held);
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
                    entity: state.entity_id,
                    pose: state.pose,
                    hotbar: state.hotbar,
                    selected_slot: state.selected_slot,
                    last_input: state.last_input,
                    handled: state.handled,
                })
            };
            let mut answers: Vec<_> = resume
                .players
                .iter()
                .map(|player| (*player, present(*player).unwrap_or(Presence::Absent)))
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

        // What is collected from here on is for the tick after this one.
        let inputs = mem::take(&mut self.inputs);
        self.discards.clear();
        let output = self.region.tick(&inputs);
        for edge in self.edges.values_mut() {
            edge.settled = true;
        }
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
        for position in output.chunk_requests {
            // A chunk that is warm is what the store has, and is handed to the next
            // tick as the store's delivery would be, a tick sooner.
            if let Some(warm) = self.warm.remove(&position) {
                self.inputs.chunks_loaded.push((position, warm.chunk));
                continue;
            }
            *self.loads.entry(position).or_default() += 1;
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
        let changes: Vec<_> = output
            .events
            .iter()
            .filter_map(|event| match event {
                RegionEvent::BlockChanged { position, state } => Some((*position, *state)),
                _ => None,
            })
            .collect();
        let needs_commit = !(changes.is_empty() && output.delta.changes_only_the_tick());
        if needs_commit {
            self.unsaved
                .extend(changes.iter().map(|(position, _)| position.chunk()));
            self.store.request(StoreRequest::Commit {
                tick: output.tick,
                changes,
                state: stored(&output.delta),
            });
        }
        // A chunk the region gives back is not loaded and has no unsaved change: its
        // last save went out when its last ticket was let go of, before the tick that
        // dropped it, and so before this. Neither a return nor a claim waits for the
        // commit: what the store does about either for a tick that is then lost, it
        // says when the region is opened again.
        if !output.returns.is_empty() {
            self.store.request(StoreRequest::Return {
                chunks: output.returns,
            });
        }
        if !output.claims.is_empty() {
            self.store.request(StoreRequest::Claim {
                chunks: output.claims,
            });
        }
        // After the commit of the tick the saved chunks show.
        if output.tick % self.checkpoint_interval == 0 {
            self.checkpoint();
        }

        // An edge only hears about what happens in chunks it watches, by its
        // subscriptions as they were while the tick ran.
        for (id, link) in &self.links {
            let events = link.visible(&output.events, &self.region, &orphaned);
            if !events.is_empty() {
                let delta = WorkerToEdge::TickDelta {
                    tick: output.tick,
                    events,
                };
                outgoing.push((*id, delta));
            }
        }

        // What concerns a single player goes to the edge they belong to. One who was let
        // go or refused in this very tick is no longer in the region, but then the outbox
        // entry saying so names their edge.
        let edge_of = |player: PlayerId| {
            let present = self.region.player_state(player).map(|state| state.edge);
            present.or_else(|| {
                output
                    .durable
                    .iter()
                    .find_map(|(edge, _, entry)| match entry {
                        Durable::Departed { player: of, .. } | Durable::Refused { player: of }
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
        for (id, link) in &mut self.links {
            let waiting: Vec<_> = link.waiting.iter().copied().collect();
            for position in waiting {
                let Some(subscription) = link.subscriptions.get_mut(&position) else {
                    continue;
                };
                let ask = subscription.ask;
                let answer = match (self.region.knowledge(position), subscription.kind) {
                    (Knowledge::Held, _) => {
                        // Held and not loaded yet: storage has not delivered it.
                        let Some(chunk) = self.region.chunk(position) else {
                            continue;
                        };
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
                            chunk: chunk.clone(),
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
        let crowds = self.region.crowds();
        if crowds != self.crowds {
            let shown = self.status.crowds.lock();
            *shown.unwrap_or_else(PoisonError::into_inner) = crowds.clone();
            self.crowds = crowds;
        }
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

    /// Hands the chunk at `position` to the store if it has unsaved changes.
    fn save(&mut self, position: ChunkPos) {
        if self.unsaved.remove(&position)
            && let Some(chunk) = self.region.chunk(position)
        {
            // The commit of the region's last tick has been sent, which the store has
            // on disk before it writes the chunk.
            self.store.request(StoreRequest::Save {
                position,
                tick: self.region.tick_number(),
                chunk: chunk.clone(),
            });
        }
    }

    /// Hands every chunk with unsaved changes and the region's whole state to the store,
    /// which keeps them in place of the commits up to here.
    fn checkpoint(&mut self) {
        for position in self.unsaved.clone() {
            self.save(position);
        }
        self.store.request(StoreRequest::Checkpoint {
            tick: self.region.tick_number(),
            state: stored(&self.region.state()),
        });
    }

    /// Ticks 20 times per second until `stop` is set, then stores what has not been
    /// stored yet. Returns early, and for good, if the store handle is lost, or once the
    /// region is released, which [`Worker::begin_release`] asks for.
    ///
    /// A release is carried on by looking at the store's answers every millisecond, and
    /// never by waiting for one; so are a merge and a split, which are taken before a
    /// step from what [`Worker::reshape`] handed in, as the wish for a release is
    /// looked at. If `stop` is set while one of them is under way, the region is let
    /// go of as it is, without anything more being asked of the store or waited for:
    /// this ends as [`Ended::Abandoned`].
    pub fn run(&mut self, stop: &AtomicBool) -> Ended {
        let mut deadline = Instant::now() + TICK;
        loop {
            if let Some(ended) = self.ended {
                return ended;
            }
            if stop.load(Ordering::Relaxed) {
                break;
            }
            if self.release_asked.load(Ordering::Relaxed) {
                self.begin_release();
            }
            while let Ok(command) = self.commands.try_recv() {
                self.take_command(command);
            }
            self.step();
            if matches!(
                self.phase,
                Phase::Settling | Phase::Closing | Phase::Committing
            ) {
                // No tick is due meanwhile; all there is to do is to take the store's
                // answers. A merge or a split that is through or off goes on ticking
                // from here, late by as long as it took.
                thread::sleep(COMMIT_POLL);
                continue;
            }
            // Between two ticks the runner publishes what the store confirms meanwhile.
            loop {
                if let Some(ended) = self.ended {
                    return ended;
                }
                let now = Instant::now();
                let Some(early) = deadline.checked_duration_since(now) else {
                    if now.duration_since(deadline) > TICK * MAX_CATCH_UP_TICKS {
                        deadline = now;
                    }
                    break;
                };
                if self.pending.is_empty() {
                    thread::sleep(early);
                    break;
                }
                thread::sleep(early.min(COMMIT_POLL));
                if self.take_replies() {
                    self.publish_committed();
                }
            }
            deadline += TICK;
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
        self.checkpoint();
        self.store.flush();
        if self.store.is_lost() {
            self.give_up();
            return Ended::StoreLost;
        }
        self.end(Ended::Stopped);
        Ended::Stopped
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
                asked: None,
                hold: BTreeSet::new(),
                held: VecDeque::new(),
                subscriptions: BTreeMap::new(),
                waiting: BTreeSet::new(),
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
    /// Nothing else waits so. A chunk the region has asked for outside its pinned
    /// areas is, if a client can click it, another region's: the click behind one's
    /// back right after a hand-over is judged at once and goes on without a region
    /// named. A chunk the store cannot read is waited for without end, and holds
    /// nothing.
    fn waits_for_its_chunk(&self, link: &EdgeLink, message: &EdgeMessage) -> bool {
        let chunk = |position: BlockPos| position.chunk();
        let about = match &message.body {
            EdgeToWorker::Input { input, .. } => match input {
                PlayerInput::Dig { position, .. } => vec![chunk(*position)],
                // The block that was clicked, and the spot beside the clicked face,
                // where a block is placed.
                PlayerInput::UseItemOn { position, face, .. } => {
                    vec![chunk(*position), chunk(face.neighbour(*position))]
                }
                PlayerInput::Move { .. }
                | PlayerInput::SelectSlot { .. }
                | PlayerInput::SetHotbarSlot { .. } => return false,
            },
            EdgeToWorker::Remote(action) => match &action.step {
                RemoteStep::Break { position } => vec![chunk(*position)],
                RemoteStep::PlaceAgainst {
                    against, target, ..
                } => vec![chunk(*against), chunk(*target)],
                RemoteStep::Place { target, .. } => vec![chunk(*target)],
            },
            // A move, an arrival, a join and a leave are about no chunk for this.
            _ => return false,
        };
        about.into_iter().any(|position| {
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
                for position in chunks {
                    self.subscribe(link, position, Ticket::Viewer, ask);
                }
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
        }
        link.edge = Some(edge);
        link.unknown = resume.answer != Answer::Resumed;
        link.resume = Some(resume);
        // The subscriptions the link begins with: a viewer's for each of `chunks` and a
        // guest's for each of `guests`. A chunk in both is a viewer's.
        for (positions, kind) in [(chunks, Ticket::Viewer), (guests, Ticket::Guest)] {
            for position in positions {
                if link.subscriptions.contains_key(&position) {
                    continue;
                }
                self.subscribe(link, position, kind, 0);
                // What the edge sends next acts on these chunks, so it waits until each
                // of them is answered: with a snapshot, which is made when the chunk is
                // loaded, or with the word that the region does not hold it. So nothing
                // an edge sends again reaches a tick before the region knows, of every
                // chunk its players can reach, who holds it.
                if !self.unreadable.contains(&position) {
                    link.hold.insert(position);
                }
            }
        }
        true
    }

    /// Takes a subscription message of `link` with the number `ask` for the chunk at
    /// `position`: `Subscribe` if `kind` is a viewer's, `SubscribeAsGuest` if a guest's.
    /// See `docs/adr/0012-the-tick-on-chunks.md`, section 4.3, for every case.
    ///
    /// A ticket is counted for the subscription whatever the region knows of the chunk.
    /// One that changes its kind stays in the condition it is in, so that a served one
    /// costs no second snapshot and loses no event.
    fn subscribe(&mut self, link: &mut EdgeLink, position: ChunkPos, kind: Ticket, ask: u64) {
        let Some(subscription) = link.subscriptions.get_mut(&position) else {
            let subscription = Subscription {
                kind,
                ask,
                condition: Condition::Waiting,
            };
            link.subscriptions.insert(position, subscription);
            link.waiting.insert(position);
            self.inputs.tickets_added.push((position, kind));
            return;
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
            }
        }
        if subscription.kind != kind {
            // The new ticket is counted before the old one is released, so the chunk
            // stays loaded.
            self.inputs.tickets_added.push((position, kind));
            self.inputs
                .tickets_removed
                .push((position, subscription.kind));
            subscription.kind = kind;
        }
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
            PlayerChange::Discard { entity, chunk } => {
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
            EdgeToWorker::PlayerLeave { player, entity } => {
                self.inputs
                    .change(PlayerChange::Leave(edge, player, entity));
            }
            EdgeToWorker::PlayerArrive { player, transfer } => {
                self.inputs
                    .change(PlayerChange::Arrive(edge, player, transfer));
            }
            EdgeToWorker::Remote(action) => {
                self.inputs.remote_actions.push((edge, action));
            }
            EdgeToWorker::Discard { entity, chunk } => {
                self.discards.push((edge, entity, chunk));
                self.inputs.change(PlayerChange::Discard { entity, chunk });
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
        if !self.links.values().any(needs) {
            // The region drops the chunk at the start of the coming tick, before
            // anything else can change it, so this is its final state. It is also what
            // the store has of the chunk when the region gives it back, which it can
            // only after that tick.
            self.save(position);
        }
        self.inputs.tickets_removed.push((position, kind));
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
    Ok(state)
}

/// A region running on its own thread.
pub struct Worker {
    thread: JoinHandle<Ended>,
    stop: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
    commands: Sender<Command>,
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

    /// Hands `reshape` to the region's thread, which takes it before its next step,
    /// and returns at once; see [`RegionRunner::reshape`] for what becomes of it.
    ///
    /// `done` is called with the outcome, once, and never for [`Reshape::Prepare`]: on
    /// the region's thread, so it must not wait for anything; a closure that sends
    /// into a channel is what it is meant to be. If the thread has ended, or ends
    /// before it has taken the command, the outcome is that nothing came of it, and
    /// `done` is called on whichever thread finds that out.
    pub fn reshape(&self, reshape: Reshape, done: Box<dyn FnOnce(Reshaped) + Send>) {
        // A thread that has ended takes nothing more. The command that comes back is
        // dropped, which calls `done`.
        let _ = self.commands.send(Command::new(reshape, done));
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
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use clustine_region::{Layout, RegionId};
    use clustine_rpc::RegionHello;
    use clustine_rpc::link::{self, EdgeEnd, LinkError};
    use clustine_sim::api::{
        EntityKind, EntityState, HOTBAR_SLOTS, PlayerInput, PlayerJoin, Pose, RegionEvent,
    };
    use clustine_sim::{PlayerTransfer, RemoteAction};
    use clustine_world::{ChunkArea, EntityIds, Vec3};
    use clustine_worldgen::FlatGenerator;
    use clustine_worldstore::{Division, Store, StoreHandle};
    use tokio::time::timeout;
    use uuid::Uuid;

    use super::*;

    /// The two kinds of link: a direct one, and one that serialises every message the
    /// way a link between two processes does.
    const KINDS: [fn(usize) -> (TestEdge, WorkerEnd); 2] = [in_process, framed];

    /// A welcome as [`TestEdge`] hands it on: without its numbers, which most tests
    /// are not about. [`TestEdge::welcomed`] has it as it was said.
    const UNKNOWN: WorkerToEdge = WorkerToEdge::Welcome(Welcome::Unknown {
        since: 0,
        entries: 0,
        presences: 0,
        applied: 0,
    });
    const RESUMED: WorkerToEdge = WorkerToEdge::Welcome(Welcome::Resumed {
        entries: 0,
        presences: 0,
        applied: 0,
    });

    /// An edge's end of a link that numbers what it sends, as an edge does.
    struct TestEdge {
        end: EdgeEnd,
        /// Which edge this is, and which start of it.
        edge: EdgeId,
        start: u64,
        /// Since when the region knows this edge, as its last welcome said; 0 for an
        /// edge that has read none. Said in its hellos.
        since: u64,
        /// The number of the last numbered message sent.
        sent: AtomicU64,
        /// The number of the last subscription message sent on this link, which are
        /// counted apart and per link.
        asked: AtomicU64,
        /// What the worker said about resuming and progress, which `recv` and `try_recv`
        /// set aside: most tests are about what else a region says.
        aside: Vec<WorkerToEdge>,
        /// The welcome read on this link, as it was said.
        welcomed: Option<Welcome>,
    }

    impl TestEdge {
        /// An edge of its own that has said hello, with nothing to resume.
        fn new(end: EdgeEnd) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let name = format!("test-edge-{}", NEXT.fetch_add(1, Ordering::Relaxed));
            let edge = Self::silent(end, EdgeId::from_name(&name), 1);
            // Before anything else, as an edge does; there is room for it on a new link.
            edge.try_send(edge.hello(0, &[], &[])).unwrap();
            edge
        }

        /// The start `start` of `edge` on a new link, on which nothing has been said yet.
        fn silent(end: EdgeEnd, edge: EdgeId, start: u64) -> Self {
            Self {
                end,
                edge,
                start,
                since: 0,
                sent: AtomicU64::new(0),
                asked: AtomicU64::new(0),
                aside: Vec::new(),
                welcomed: None,
            }
        }

        /// The same start of the same edge on another link, numbering on from where it
        /// was. It has not said hello there yet. It has read every welcome the region
        /// `runner` runs has made for it, and so knows since when the region knows it.
        fn again(&self, end: EdgeEnd, runner: &RegionRunner) -> Self {
            let mut again = Self::silent(end, self.edge, self.start);
            again.since = runner
                .region()
                .edge(self.edge)
                .map_or(0, |state| state.since);
            again
                .sent
                .store(self.sent.load(Ordering::Relaxed), Ordering::Relaxed);
            again
        }

        /// What this edge says first on a link, with `chunks` as those it asks for as
        /// a viewer and none as a guest.
        fn hello(&self, seen: u64, players: &[PlayerId], chunks: &[ChunkPos]) -> EdgeToWorker {
            EdgeToWorker::Hello {
                edge: self.edge,
                start: self.start,
                since: self.since,
                seen,
                players: players.to_vec(),
                chunks: chunks.to_vec(),
                guests: Vec::new(),
            }
        }

        /// `body` with the number this edge gives it: the next of those that change
        /// the region, or, for a subscription message, the next of the link's askings,
        /// whatever number it was made with.
        fn numbered(&self, body: EdgeToWorker) -> EdgeMessage {
            let number = body
                .is_numbered()
                .then(|| self.sent.fetch_add(1, Ordering::Relaxed) + 1);
            let next = || self.asked.fetch_add(1, Ordering::Relaxed) + 1;
            let body = match body {
                EdgeToWorker::Subscribe { chunks, .. } => {
                    let ask = next();
                    EdgeToWorker::Subscribe { ask, chunks }
                }
                EdgeToWorker::SubscribeAsGuest { chunks, .. } => {
                    let ask = next();
                    EdgeToWorker::SubscribeAsGuest { ask, chunks }
                }
                EdgeToWorker::Unsubscribe { chunks, .. } => {
                    let ask = next();
                    EdgeToWorker::Unsubscribe { ask, chunks }
                }
                other => other,
            };
            EdgeMessage { number, body }
        }

        /// The number of the last subscription message this edge sent on its link.
        fn asked(&self) -> u64 {
            self.asked.load(Ordering::Relaxed)
        }

        async fn send(&self, body: EdgeToWorker) -> Result<(), LinkError> {
            self.end.send(self.numbered(body)).await
        }

        fn try_send(&self, body: EdgeToWorker) -> Result<(), LinkError> {
            self.end.try_send(self.numbered(body))
        }

        /// Sends a message as it is, numbered or not.
        async fn send_as_is(&self, message: EdgeMessage) -> Result<(), LinkError> {
            self.end.send(message).await
        }

        /// Sends `body` under `number`, whatever this edge has sent before.
        async fn send_as(&self, number: u64, body: EdgeToWorker) {
            let number = Some(number);
            self.send_as_is(EdgeMessage { number, body }).await.unwrap();
        }

        /// Whether `message` is set aside by `recv` and `try_recv`.
        fn about_resuming(message: &WorkerToEdge) -> bool {
            matches!(
                message,
                WorkerToEdge::Welcome(_)
                    | WorkerToEdge::Presence { .. }
                    | WorkerToEdge::Progress { .. }
            )
        }

        /// Takes note of a welcome as it was said, and hands every message on with a
        /// welcome's numbers taken out.
        fn plain(&mut self, message: WorkerToEdge) -> WorkerToEdge {
            match message {
                WorkerToEdge::Welcome(welcome) => {
                    self.welcomed = Some(welcome);
                    match welcome {
                        Welcome::Resumed { .. } => RESUMED,
                        Welcome::Unknown { .. } => UNKNOWN,
                        Welcome::Superseded => WorkerToEdge::Welcome(Welcome::Superseded),
                    }
                }
                other => other,
            }
        }

        async fn recv(&mut self) -> Option<WorkerToEdge> {
            loop {
                let message = self.end.recv().await?;
                let message = self.plain(message);
                if !Self::about_resuming(&message) {
                    return Some(message);
                }
                self.aside.push(message);
            }
        }

        fn try_recv(&mut self) -> Result<Option<WorkerToEdge>, LinkError> {
            loop {
                match self.end.try_recv()?.map(|message| self.plain(message)) {
                    Some(message) if Self::about_resuming(&message) => self.aside.push(message),
                    other => return Ok(other),
                }
            }
        }

        /// Everything the edge has been sent and has not looked at yet, with nothing set
        /// aside, in the order it came.
        fn everything(&mut self) -> Vec<WorkerToEdge> {
            let mut messages = Vec::new();
            while let Ok(Some(message)) = self.end.try_recv() {
                let message = self.plain(message);
                messages.push(message);
            }
            messages
        }
    }

    /// A direct link.
    fn in_process(capacity: usize) -> (TestEdge, WorkerEnd) {
        let (edge, worker) = link::in_process(capacity);
        (TestEdge::new(edge), worker)
    }

    /// A link that serialises every message, as one between two processes does.
    fn framed(capacity: usize) -> (TestEdge, WorkerEnd) {
        let (edge, worker) = link::framed(capacity);
        (TestEdge::new(edge), worker)
    }

    /// A store's handle whose answers to commits can be held back, and which can be
    /// lost on demand. It notes what the runner asks for.
    struct Gate {
        inner: StoreHandle,
        control: Arc<GateControl>,
    }

    #[derive(Default)]
    struct GateControl {
        holding: AtomicBool,
        /// Whether the answers to flushes are held back, which is apart from those to
        /// commits: a release waits for the one while the other goes on.
        holding_flushes: AtomicBool,
        /// Whether the answers to claims are held back, and whether those to loads are.
        holding_claims: AtomicBool,
        holding_loads: AtomicBool,
        /// Whether the answer to a merge or a split is held back.
        holding_reshapes: AtomicBool,
        /// Everything the runner asked for, in that order.
        asked: Mutex<Vec<Asked>>,
        lost: AtomicBool,
        /// The answers held back, in the order the store gave them.
        kept: Mutex<VecDeque<StoreReply>>,
        /// The ticks of the commits asked for, in that order.
        commits: Mutex<Vec<u64>>,
        /// How many flushes were asked for.
        flushes: AtomicU64,
    }

    impl GateControl {
        fn hold(&self) {
            self.holding.store(true, Ordering::SeqCst);
        }

        fn release(&self) {
            self.holding.store(false, Ordering::SeqCst);
        }

        fn hold_flushes(&self) {
            self.holding_flushes.store(true, Ordering::SeqCst);
        }

        fn release_flushes(&self) {
            self.holding_flushes.store(false, Ordering::SeqCst);
        }

        fn hold_claims(&self) {
            self.holding_claims.store(true, Ordering::SeqCst);
        }

        fn release_claims(&self) {
            self.holding_claims.store(false, Ordering::SeqCst);
        }

        fn hold_loads(&self) {
            self.holding_loads.store(true, Ordering::SeqCst);
        }

        fn release_loads(&self) {
            self.holding_loads.store(false, Ordering::SeqCst);
        }

        fn hold_reshapes(&self) {
            self.holding_reshapes.store(true, Ordering::SeqCst);
        }

        /// Loses the handle, as the store does when it gives the region to another
        /// owner or cannot be reached.
        fn lose(&self) {
            self.lost.store(true, Ordering::SeqCst);
        }

        /// Whether `reply` is to be held back as things are.
        fn holds(&self, reply: &StoreReply) -> bool {
            match reply {
                StoreReply::Committed { .. } => self.holding.load(Ordering::SeqCst),
                StoreReply::Flushed => self.holding_flushes.load(Ordering::SeqCst),
                StoreReply::Claimed { .. } => self.holding_claims.load(Ordering::SeqCst),
                StoreReply::Loaded { .. } => self.holding_loads.load(Ordering::SeqCst),
                StoreReply::Absorbed { .. }
                | StoreReply::Split { .. }
                | StoreReply::Declined { .. } => self.holding_reshapes.load(Ordering::SeqCst),
                _ => false,
            }
        }

        /// How many of the answers held back `counts`.
        fn kept_of(&self, counts: impl Fn(&StoreReply) -> bool) -> usize {
            let kept = self.kept.lock().unwrap();
            kept.iter().filter(|reply| counts(reply)).count()
        }

        /// How many answers to claims are held back.
        fn kept_claims(&self) -> usize {
            self.kept_of(|reply| matches!(reply, StoreReply::Claimed { .. }))
        }

        /// How many answers to loads are held back.
        fn kept_loads(&self) -> usize {
            self.kept_of(|reply| matches!(reply, StoreReply::Loaded { .. }))
        }

        /// How many answers to a merge or a split are held back.
        fn kept_reshapes(&self) -> usize {
            self.kept_of(|reply| {
                matches!(
                    reply,
                    StoreReply::Absorbed { .. }
                        | StoreReply::Split { .. }
                        | StoreReply::Declined { .. }
                )
            })
        }

        /// What the runner has asked for since this was last called, in that order.
        fn asked(&self) -> Vec<Asked> {
            mem::take(&mut self.asked.lock().unwrap())
        }

        /// How many answers to commits are held back.
        fn kept(&self) -> usize {
            let kept = self.kept.lock().unwrap();
            let commits = kept
                .iter()
                .filter(|reply| matches!(reply, StoreReply::Committed { .. }));
            commits.count()
        }

        /// How many answers to flushes are held back.
        fn kept_flushes(&self) -> usize {
            let kept = self.kept.lock().unwrap();
            let flushes = kept.iter().filter(|reply| **reply == StoreReply::Flushed);
            flushes.count()
        }

        fn commits(&self) -> Vec<u64> {
            self.commits.lock().unwrap().clone()
        }

        fn flushes(&self) -> u64 {
            self.flushes.load(Ordering::SeqCst)
        }
    }

    /// Something a runner asked the store for, without what it carried.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Asked {
        Load(ChunkPos),
        Save(ChunkPos),
        Commit(u64),
        Return(Vec<ChunkPos>),
        Claim(Vec<ChunkPos>),
        Checkpoint(u64),
        Flush,
        /// A merge, with its tick.
        Absorb(u64),
        /// A split, with its tick and the id it names for the new region.
        Split(u64, RegionId),
    }

    impl Asked {
        fn of(request: &StoreRequest) -> Self {
            match request {
                StoreRequest::Load { position } => Self::Load(*position),
                StoreRequest::Save { position, .. } => Self::Save(*position),
                StoreRequest::Commit { tick, .. } => Self::Commit(*tick),
                StoreRequest::Return { chunks } => Self::Return(chunks.clone()),
                StoreRequest::Claim { chunks } => Self::Claim(chunks.clone()),
                StoreRequest::Checkpoint { tick, .. } => Self::Checkpoint(*tick),
                StoreRequest::Flush => Self::Flush,
                StoreRequest::AbsorbCommit { tick, .. } => Self::Absorb(*tick),
                StoreRequest::SplitCommit { tick, region, .. } => Self::Split(*tick, *region),
            }
        }
    }

    impl RegionStore for Gate {
        fn request(&self, request: StoreRequest) {
            self.control.asked.lock().unwrap().push(Asked::of(&request));
            if let StoreRequest::Commit { tick, .. } = &request {
                self.control.commits.lock().unwrap().push(*tick);
            }
            if request == StoreRequest::Flush {
                self.control.flushes.fetch_add(1, Ordering::SeqCst);
            }
            self.inner.request(request);
        }

        fn try_reply(&self) -> Option<StoreReply> {
            if RegionStore::is_lost(self) {
                return None;
            }
            let mut kept = self.control.kept.lock().unwrap();
            let free = kept.iter().position(|reply| !self.control.holds(reply));
            if let Some(reply) = free.and_then(|index| kept.remove(index)) {
                return Some(reply);
            }
            loop {
                let reply = self.inner.try_reply()?;
                if !self.control.holds(&reply) {
                    return Some(reply);
                }
                kept.push_back(reply);
            }
        }

        fn is_lost(&self) -> bool {
            self.control.lost.load(Ordering::SeqCst) || self.inner.is_lost()
        }

        fn flush(&self) {
            self.inner.flush();
        }
    }

    /// Where players enter the flat world.
    const SPAWN: Vec3 = Vec3::new(0.5, -60.0, 0.5);

    const ORIGIN: ChunkPos = ChunkPos::new(0, 0);

    /// The eastern one of the two regions of [`Divided::stripes`], which begins where
    /// the chunks with x = 1 do.
    const EAST: RegionId = RegionId(1);

    /// What a region is made with. It knows of a chunk only what the store tells it,
    /// and gives one back that nothing has used for `return_after` ticks.
    fn config(return_after: u64) -> RegionConfig {
        RegionConfig {
            spawn: SPAWN,
            starting_hotbar: [None; HOTBAR_SLOTS],
            return_after,
        }
    }

    /// A store of a flat world that is one region and only lasts as long as the store.
    fn memory() -> Store {
        Store::memory(Arc::new(FlatGenerator::classic()))
    }

    /// A store of a flat world that is one region, kept in `directory`.
    fn on_disk(directory: &std::path::Path) -> Store {
        Store::local(directory, Arc::new(FlatGenerator::classic())).unwrap()
    }

    /// The hello of the owner of the one region of [`memory`] and [`on_disk`].
    fn owner() -> RegionHello {
        RegionHello {
            region: RegionId(0),
            epoch: 1,
            layout: Layout::single().fingerprint(),
        }
    }

    /// A runner for the region as `store` has it, without links.
    fn opened(store: &Store, config: RegionConfig) -> RegionRunner {
        let (handle, restored) = store.open_region(owner()).unwrap();
        RegionRunner::restore(config, handle, restored).unwrap()
    }

    /// A runner as [`opened`] makes it, with a gate before its store.
    fn gated(store: &Store, config: RegionConfig) -> (RegionRunner, Arc<GateControl>) {
        gated_as(store, owner(), config)
    }

    /// A runner for the region that `hello` names, with a gate before its store.
    fn gated_as(
        store: &Store,
        hello: RegionHello,
        config: RegionConfig,
    ) -> (RegionRunner, Arc<GateControl>) {
        let (inner, restored) = store.open_region(hello).unwrap();
        let control = Arc::new(GateControl::default());

        let gate = Gate {
            inner,
            control: Arc::clone(&control),
        };
        let holdings = holdings(&restored);
        let region = Region::restore(config, restored_state(restored).unwrap(), holdings);

        (RegionRunner::with_store(region, Box::new(gate)), control)
    }

    /// A runner for the one region of a flat world that only lasts as long as the
    /// runner, with `link` as its first link.
    fn runner(link: WorkerEnd) -> RegionRunner {
        let runner = opened(&memory(), config(0));
        runner.links().attach(link);
        runner
    }

    /// A runner for the western one of the two regions of [`Divided::stripes`], in a
    /// world that only lasts as long as the runner, with `link` as its first link.
    fn west(link: WorkerEnd) -> RegionRunner {
        let runner = Divided::stripes().runner(RegionId(0), 1);
        runner.links().attach(link);
        runner
    }

    /// Has `edge` ask the region of `runner` for [`BESIDE`] as a viewer and waits for
    /// the answer, as an edge does whose player can see across the line. From then on
    /// the region knows that the chunk is [`EAST`]'s: a player who steps into it is let
    /// go in the tick of the step, and what one does to a block of it is passed on with
    /// the region named. Without it the region would learn that only when it has asked
    /// the store because a player stands there, a tick or two after the step.
    fn look_east(runner: &mut RegionRunner, edge: &mut TestEdge) {
        edge.try_send(asking_for(vec![BESIDE])).unwrap();
        let elsewhere = WorkerToEdge::Elsewhere {
            chunk: BESIDE,
            ask: edge.asked(),
            region: EAST,
        };
        assert_eq!(step_for(runner, edge), elsewhere);
    }

    fn player() -> PlayerId {
        PlayerId(Uuid::from_u128(1))
    }

    fn other_player() -> PlayerId {
        PlayerId(Uuid::from_u128(2))
    }

    fn third_player() -> PlayerId {
        PlayerId(Uuid::from_u128(3))
    }

    fn join(player: PlayerId, name: &str) -> EdgeToWorker {
        EdgeToWorker::PlayerJoin(PlayerJoin {
            player,
            name: name.to_owned(),
        })
    }

    /// A player as another region hands them over, standing in the chunk at the origin.
    fn transfer(entity_id: EntityId) -> PlayerTransfer {
        PlayerTransfer {
            entity_id,
            name: "Jeb".to_owned(),
            pose: Pose::at(Vec3::new(10.5, -60.0, 0.5)),
            hotbar: [None; HOTBAR_SLOTS],
            selected_slot: 4,
            last_input: 7,
        }
    }

    /// Numbers inputs the way an edge does: in the order they are made.
    fn next_number() -> u64 {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    /// The entity a player has in most of these tests, which their inputs name: the
    /// players join a region that gives out the first block of entity ids, in the
    /// order of [`player`], [`other_player`] and [`third_player`]. A test whose player
    /// has another entity says so with [`of_entity`].
    fn usual_entity(player: PlayerId) -> EntityId {
        EntityId(i32::try_from(player.0.as_u128()).expect("a player of these tests"))
    }

    /// `input`, which has to be an input, as of the stay with `entity`.
    fn of_entity(entity: EntityId, input: EdgeToWorker) -> EdgeToWorker {
        match input {
            EdgeToWorker::Input {
                player,
                number,
                input,
                ..
            } => EdgeToWorker::Input {
                player,
                entity,
                number,
                input,
            },
            other => panic!("{other:?} is no input"),
        }
    }

    /// A step along the x axis as the input with the given number.
    fn walk_as(player: PlayerId, number: u64, x: f64) -> EdgeToWorker {
        EdgeToWorker::Input {
            player,
            entity: usual_entity(player),
            number,
            input: PlayerInput::Move {
                position: Some(Vec3::new(x, -60.0, 0.5)),
                rotation: None,
                on_ground: true,
            },
        }
    }

    fn walk(player: PlayerId, x: f64) -> EdgeToWorker {
        walk_as(player, next_number(), x)
    }

    fn dig_by(player: PlayerId, x: i32, sequence: i32) -> EdgeToWorker {
        EdgeToWorker::Input {
            player,
            entity: usual_entity(player),
            number: next_number(),
            input: PlayerInput::Dig {
                position: BlockPos::new(x, -61, 0),
                sequence,
            },
        }
    }

    fn dig(x: i32) -> EdgeToWorker {
        dig_by(player(), x, 1)
    }

    /// A viewer's subscription to `chunks`. The [`TestEdge`] that sends it numbers it.
    fn asking_for(chunks: Vec<ChunkPos>) -> EdgeToWorker {
        EdgeToWorker::Subscribe { ask: 0, chunks }
    }

    /// A guest's subscription to `chunks`, numbered as [`asking_for`] is.
    fn asking_as_guest_for(chunks: Vec<ChunkPos>) -> EdgeToWorker {
        EdgeToWorker::SubscribeAsGuest { ask: 0, chunks }
    }

    /// The end of the subscriptions to `chunks`, numbered as [`asking_for`] is.
    fn done_with(chunks: Vec<ChunkPos>) -> EdgeToWorker {
        EdgeToWorker::Unsubscribe { ask: 0, chunks }
    }

    fn square(radius: i32) -> Vec<ChunkPos> {
        (-radius..=radius)
            .flat_map(|x| (-radius..=radius).map(move |z| ChunkPos::new(x, z)))
            .collect()
    }

    /// Where `player` stands along the x axis, if they are in the region.
    fn x_of(runner: &RegionRunner, player: PlayerId) -> Option<f64> {
        Some(runner.region().player(player)?.1.position.x)
    }

    /// Waits until the store has confirmed every tick that has run, and publishes them.
    /// No tick runs meanwhile.
    fn settle(runner: &mut RegionRunner) {
        for _ in 0..20_000 {
            if !runner.take_replies() {
                return;
            }
            runner.publish_committed();
            if runner.pending.is_empty() {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("the store did not confirm what the region committed");
    }

    /// Runs one tick with what has arrived, and publishes it once the store has it.
    fn step(runner: &mut RegionRunner) {
        runner.step();
        settle(runner);
    }

    /// Steps `runner` until `done` holds, giving the store thread and links that
    /// serialise time to deliver.
    fn step_until(runner: &mut RegionRunner, mut done: impl FnMut(&RegionRunner) -> bool) {
        for _ in 0..2000 {
            step(runner);
            if done(runner) {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("the condition never became true");
    }

    /// Steps `runner` until `edge` has been sent something, and returns that.
    fn step_for(runner: &mut RegionRunner, edge: &mut TestEdge) -> WorkerToEdge {
        for _ in 0..2000 {
            if let Ok(Some(message)) = edge.try_recv() {
                return message;
            }
            step(runner);
            thread::sleep(Duration::from_millis(1));
        }
        panic!("the worker sent nothing");
    }

    /// Everything `edge` has been sent and has not looked at yet, but for what is said
    /// about resuming and progress.
    fn received(edge: &mut TestEdge) -> Vec<WorkerToEdge> {
        let mut messages = Vec::new();
        while let Ok(Some(message)) = edge.try_recv() {
            messages.push(message);
        }
        messages
    }

    async fn next(edge: &mut TestEdge) -> WorkerToEdge {
        timeout(Duration::from_secs(10), edge.recv())
            .await
            .expect("the worker sent nothing")
            .expect("the worker closed the link")
    }

    /// Whether the worker has closed the link of `edge`, after whatever it sent before.
    async fn closed(edge: &mut TestEdge) -> bool {
        let ended = async { while edge.recv().await.is_some() {} };
        timeout(Duration::from_secs(10), ended).await.is_ok()
    }

    /// The events of the delta that `message` has to be.
    fn events(message: WorkerToEdge) -> Vec<RegionEvent> {
        match message {
            WorkerToEdge::TickDelta { events, .. } => events,
            other => panic!("expected a delta, got {other:?}"),
        }
    }

    /// The chunk that `message` has to be a snapshot of, and the entities in it.
    fn snapshot(message: WorkerToEdge) -> (ChunkPos, Vec<EntityState>) {
        match message {
            WorkerToEdge::ChunkSnapshot {
                position, entities, ..
            } => (position, entities),
            other => panic!("expected a snapshot, got {other:?}"),
        }
    }

    /// The outbox of `edge` as the region has it: the number of the last entry made and
    /// the numbers of those not confirmed.
    fn outbox(runner: &RegionRunner, edge: &TestEdge) -> (u64, Vec<u64>) {
        let state = runner.region().edge(edge.edge).unwrap();
        (state.sent, state.outbox.keys().copied().collect())
    }

    /// A stand-in edge joins and subscribes, over a direct and over a serialising link.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_joining_player_is_spawned_and_sent_the_chunks_around() {
        for (mut edge, worker_end) in [in_process(256), framed(256)] {
            let worker = Worker::spawn(runner(worker_end));

            edge.send(join(player(), "Notch")).await.unwrap();
            edge.send(asking_for(square(1))).await.unwrap();

            let mut spawned = None;
            let mut snapshots = BTreeSet::new();
            let mut entities = Vec::new();
            while spawned.is_none() || snapshots.len() < 9 {
                match next(&mut edge).await {
                    WorkerToEdge::ToPlayer { player: to, event } => {
                        assert_eq!(to, player());
                        spawned = Some(event);
                    }
                    WorkerToEdge::ChunkSnapshot {
                        position,
                        chunk,
                        entities: in_chunk,
                        ..
                    } => {
                        assert_eq!(chunk.surface_heights(), [4; 256]);
                        assert!(snapshots.insert(position), "{position:?} sent twice");
                        // Entities come with the chunk they are in.
                        assert!(in_chunk.iter().all(|entity| entity.chunk() == position));
                        entities.extend(in_chunk);
                    }
                    // Whether the join is also reported as an event depends on whether
                    // the subscription arrived within the same tick.
                    WorkerToEdge::TickDelta { events, .. } => {
                        assert!(matches!(events[..], [RegionEvent::EntitySpawned(_)]));
                    }
                    other => panic!("unexpected {other:?}"),
                }
            }
            assert_eq!(
                spawned,
                Some(PlayerEvent::Spawned {
                    entity_id: EntityId(1),
                    position: SPAWN,
                    hotbar: [None; HOTBAR_SLOTS],
                    selected_slot: 0,
                })
            );
            assert_eq!(snapshots, square(1).into_iter().collect());
            // The player's own entity is in the snapshot of the chunk it stands in.
            assert_eq!(entities.len(), 1);
            assert_eq!(entities[0].entity, EntityId(1));

            worker.stop();
        }
    }

    #[tokio::test]
    async fn movement_is_published_only_for_subscribed_chunks() {
        let (mut edge, worker_end) = in_process(256);
        let mut runner = runner(worker_end);

        edge.send(join(player(), "Notch")).await.unwrap();
        edge.send(asking_for(vec![ChunkPos::new(0, 0)]))
            .await
            .unwrap();
        step_until(&mut runner, |runner| {
            runner.region().loaded_chunk_count() == 1
        });
        step(&mut runner);
        received(&mut edge);

        // Within the subscribed chunk, then out of it: both can be seen from it.
        for x in [5.0, 20.0] {
            edge.send(walk(player(), x)).await.unwrap();
            step(&mut runner);
            let Ok(Some(WorkerToEdge::TickDelta { events, .. })) = edge.try_recv() else {
                panic!("expected a delta for the move to x = {x}");
            };
            assert!(matches!(
                events[..],
                [RegionEvent::EntityMoved { pose, .. }] if pose.position.x == x
            ));
        }

        // From one chunk nobody subscribed to into another: nobody is told.
        edge.send(walk(player(), 40.0)).await.unwrap();
        step(&mut runner);
        assert_eq!(edge.try_recv(), Ok(None));
    }

    /// An entity that walks into view from somewhere the edge was not watching is
    /// introduced in full, because the edge has never heard of it.
    #[tokio::test]
    async fn an_entity_entering_the_subscribed_area_is_introduced() {
        let (mut edge, worker_end) = in_process(256);
        let mut runner = runner(worker_end);

        // The edge watches a chunk far from where the player enters the world.
        edge.send(join(player(), "Notch")).await.unwrap();
        edge.send(asking_for(vec![ChunkPos::new(5, 0)]))
            .await
            .unwrap();
        step_until(&mut runner, |runner| {
            runner.region().loaded_chunk_count() == 1
        });
        step(&mut runner);
        let seen = received(&mut edge);
        assert!(
            seen.iter().all(|message| match message {
                WorkerToEdge::ChunkSnapshot { entities, .. } => entities.is_empty(),
                WorkerToEdge::ToPlayer { .. } => true,
                _ => false,
            }),
            "{seen:?}"
        );

        edge.send(walk(player(), 85.0)).await.unwrap();
        step(&mut runner);
        let Ok(Some(WorkerToEdge::TickDelta { events, .. })) = edge.try_recv() else {
            panic!("expected a delta");
        };
        let [RegionEvent::EntitySpawned(state)] = &events[..] else {
            panic!("expected the entity to be introduced, got {events:?}");
        };
        assert_eq!(state.entity, EntityId(1));
        assert_eq!(state.pose.position.x, 85.0);
        assert!(matches!(&state.kind, EntityKind::Player { name, .. } if name == "Notch"));

        // Leaving the watched chunk again is an ordinary move, and the player leaving
        // the game out there is none of the edge's business.
        edge.send(walk(player(), 120.0)).await.unwrap();
        step(&mut runner);
        assert!(matches!(
            edge.try_recv(),
            Ok(Some(WorkerToEdge::TickDelta { events, .. }))
                if matches!(events[..], [RegionEvent::EntityMoved { .. }])
        ));
        edge.send(EdgeToWorker::PlayerLeave {
            player: player(),
            entity: None,
        })
        .await
        .unwrap();
        step(&mut runner);
        assert_eq!(edge.try_recv(), Ok(None));
    }

    /// Two edges watch different parts of the region. Neither hears what happens in the
    /// other's part, and neither is sent the other's chunk.
    #[tokio::test(flavor = "multi_thread")]
    async fn each_link_hears_only_of_the_chunks_it_subscribed_to() {
        for connect in KINDS {
            let (mut near, near_end) = connect(256);
            let (mut far, far_end) = connect(256);
            let mut runner = runner(near_end);
            runner.links().attach(far_end);
            let (origin, distant) = (ChunkPos::new(0, 0), ChunkPos::new(5, 0));

            near.send(asking_for(vec![origin])).await.unwrap();
            far.send(asking_for(vec![distant])).await.unwrap();
            assert_eq!(snapshot(step_for(&mut runner, &mut near)).0, origin);
            assert_eq!(snapshot(step_for(&mut runner, &mut far)).0, distant);

            // A player enters the world at the origin, which only one edge watches.
            near.send(join(player(), "Notch")).await.unwrap();
            let seen = events(step_for(&mut runner, &mut near));
            assert!(matches!(seen[..], [RegionEvent::EntitySpawned(_)]));
            assert!(matches!(
                step_for(&mut runner, &mut near),
                WorkerToEdge::ToPlayer { .. }
            ));

            // They walk over to what the other edge watches. That it is told who they
            // are, and before anything else, shows that it had not heard of them.
            near.send(walk(player(), 85.0)).await.unwrap();
            let seen = events(step_for(&mut runner, &mut near));
            assert!(matches!(seen[..], [RegionEvent::EntityMoved { .. }]));
            let seen = events(step_for(&mut runner, &mut far));
            let [RegionEvent::EntitySpawned(state)] = &seen[..] else {
                panic!("expected the entity to be introduced, got {seen:?}");
            };
            assert_eq!(state.pose.position.x, 85.0);

            // A step over there is for the far edge alone. The near edge is told next
            // when they come back, and then as of someone it does not know.
            near.send(walk(player(), 86.0)).await.unwrap();
            let seen = events(step_for(&mut runner, &mut far));
            assert!(matches!(seen[..], [RegionEvent::EntityMoved { .. }]));
            near.send(walk(player(), 5.0)).await.unwrap();
            let seen = events(step_for(&mut runner, &mut near));
            assert!(matches!(seen[..], [RegionEvent::EntitySpawned(_)]));
            let seen = events(step_for(&mut runner, &mut far));
            assert!(matches!(seen[..], [RegionEvent::EntityMoved { .. }]));
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn what_concerns_a_player_goes_to_their_link_only() {
        for connect in KINDS {
            let (mut first, first_end) = connect(256);
            let (mut second, second_end) = connect(256);
            let mut runner = runner(first_end);
            runner.links().attach(second_end);

            // One after the other, however long a link takes over a message: what the
            // players do names the entities they are given, which are given out in the
            // order the joins are taken.
            first.send(join(player(), "Notch")).await.unwrap();
            step_until(&mut runner, |runner| runner.region().player_count() == 1);
            second.send(join(other_player(), "Jeb")).await.unwrap();
            step_until(&mut runner, |runner| runner.region().player_count() == 2);
            // The region claims the chunk its players stand in. A block of a chunk it
            // does not know to hold yet it would pass on, to whoever serves the edge
            // the chunk, in place of acknowledging it.
            step_until(&mut runner, |runner| {
                runner.region().knowledge(ORIGIN) == Knowledge::Held
            });
            first.send(dig_by(player(), 1, 3)).await.unwrap();
            second.send(dig_by(other_player(), 2, 7)).await.unwrap();

            // Neither edge is subscribed to anything, so this is all there is on a link.
            let expected = [(&mut first, player(), 3), (&mut second, other_player(), 7)];
            for (edge, player, sequence) in expected {
                let message = step_for(&mut runner, edge);
                assert!(
                    matches!(
                        message,
                        WorkerToEdge::ToPlayer { player: to, event: PlayerEvent::Spawned { .. } }
                            if to == player
                    ),
                    "{message:?}"
                );
                let event = PlayerEvent::Acknowledged { sequence };
                assert_eq!(
                    step_for(&mut runner, edge),
                    WorkerToEdge::ToPlayer { player, event }
                );
            }
        }
    }

    /// Edges connect whenever they like, also long after the region has started.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_link_attached_while_the_runner_runs_is_served() {
        for connect in KINDS {
            let runner = opened(&memory(), config(0));
            let (links, status) = (runner.links(), runner.status());
            let worker = Worker::spawn(runner);

            let (mut first, first_end) = connect(256);
            links.attach(first_end);
            first.send(join(player(), "Notch")).await.unwrap();
            assert!(matches!(
                next(&mut first).await,
                WorkerToEdge::ToPlayer { .. }
            ));

            // From another thread, as whatever accepts connections attaches them.
            let (mut second, second_end) = connect(256);
            let attach = {
                let links = links.clone();
                move || links.attach(second_end)
            };
            thread::spawn(attach).join().unwrap();
            second.send(asking_for(vec![ORIGIN])).await.unwrap();
            let (position, entities) = snapshot(next(&mut second).await);
            assert_eq!(position, ORIGIN);
            // The player of the other edge stands there.
            assert_eq!(entities.len(), 1);
            assert!(status.tick.load(Ordering::Relaxed) > 0);
            assert_eq!(status.players.load(Ordering::Relaxed), 1);

            // A link that comes too late is closed rather than left waiting.
            assert_eq!(worker.stop(), Ended::Stopped);
            let (mut late, late_end) = connect(256);
            links.attach(late_end);
            assert!(closed(&mut late).await);
        }
    }

    /// Joins a player, subscribes to the chunk they stand in and waits until it is loaded.
    async fn joined(edge: &TestEdge, runner: &mut RegionRunner) {
        edge.send(join(player(), "Notch")).await.unwrap();
        edge.send(asking_for(vec![ChunkPos::new(0, 0)]))
            .await
            .unwrap();
        step_until(runner, |runner| runner.region().loaded_chunk_count() == 1);
    }

    /// A changed chunk that nobody needs any more is stored, and comes back changed.
    #[tokio::test]
    async fn changes_survive_a_chunk_being_unloaded() {
        let (edge, worker_end) = in_process(256);
        let mut runner = runner(worker_end);
        let origin = ChunkPos::new(0, 0);
        joined(&edge, &mut runner).await;

        edge.send(dig(1)).await.unwrap();
        step(&mut runner);
        let changed = runner.region().chunk(origin).unwrap().clone();
        assert_eq!(changed.get(1, -61, 0), Some(clustine_data::blocks::AIR));

        edge.send(done_with(vec![origin])).await.unwrap();
        step(&mut runner);
        assert_eq!(runner.region().loaded_chunk_count(), 0);

        edge.send(asking_for(vec![origin])).await.unwrap();
        step_until(&mut runner, |runner| {
            runner.region().loaded_chunk_count() == 1
        });
        assert_eq!(runner.region().chunk(origin), Some(&changed));
    }

    /// A chunk that two edges watch is needed until both have let go of it. Only then is
    /// it stored, with everything that has happened to it by then.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_chunk_is_kept_until_its_last_subscriber_lets_go_and_is_saved_then() {
        for connect in KINDS {
            let (first, first_end) = connect(256);
            let (mut second, second_end) = connect(256);
            let mut runner = runner(first_end);
            runner.links().attach(second_end);
            let origin = ChunkPos::new(0, 0);
            let subscribe = || asking_for(vec![origin]);
            let unsubscribe = || done_with(vec![origin]);
            let dug = |runner: &RegionRunner, x: usize| {
                let chunk = runner.region().chunk(origin);
                chunk.and_then(|chunk| chunk.get(x, -61, 0)) == Some(clustine_data::blocks::AIR)
            };

            joined(&first, &mut runner).await;
            second.send(subscribe()).await.unwrap();
            assert_eq!(snapshot(step_for(&mut runner, &mut second)).0, origin);

            first.send(dig(1)).await.unwrap();
            step_until(&mut runner, |runner| dug(runner, 1));
            first.send(unsubscribe()).await.unwrap();
            step_until(&mut runner, |runner| {
                runner.links[&LinkId(0)].subscriptions.is_empty()
            });
            assert_eq!(runner.region().loaded_chunk_count(), 1);

            // The player is still there and changes the chunk once more.
            first.send(dig(2)).await.unwrap();
            step_until(&mut runner, |runner| dug(runner, 2));
            second.send(unsubscribe()).await.unwrap();
            step_until(&mut runner, |runner| {
                runner.region().loaded_chunk_count() == 0
            });

            second.send(subscribe()).await.unwrap();
            step_until(&mut runner, |runner| {
                runner.region().loaded_chunk_count() == 1
            });
            assert!(dug(&runner, 1) && dug(&runner, 2));
        }
    }

    /// Stopping stores what is still loaded and the region's state, so that another
    /// runner on the same world carries on with both.
    #[tokio::test]
    async fn stopping_stores_changed_chunks_and_the_state() {
        let directory = tempfile::tempdir().unwrap();
        let (edge, worker_end) = in_process(256);
        let mut first = opened(&on_disk(directory.path()), config(0));
        first.links().attach(worker_end);
        joined(&edge, &mut first).await;
        edge.send(dig(1)).await.unwrap();
        edge.send(dig(2)).await.unwrap();
        step(&mut first);
        let changed = first.region().chunk(ORIGIN).unwrap().clone();
        // The player is still there and the chunk still loaded when the runner stops.
        assert_eq!(first.run(&AtomicBool::new(true)), Ended::Stopped);
        let state = first.region().state();
        drop((first, edge));

        let (edge, worker_end) = in_process(256);
        let mut second = opened(&on_disk(directory.path()), config(0));
        assert_eq!(second.region().state(), state);
        second.links().attach(worker_end);
        edge.send(asking_for(vec![ORIGIN])).await.unwrap();
        step_until(&mut second, |runner| {
            runner.region().loaded_chunk_count() == 1
        });
        assert_eq!(second.region().chunk(ORIGIN), Some(&changed));
        assert_eq!(changed.get(2, -61, 0), Some(clustine_data::blocks::AIR));
    }

    /// Changes to a chunk that stays loaded reach the stored world at the next
    /// checkpoint, which also empties the log.
    #[tokio::test]
    async fn checkpoints_save_loaded_chunks_and_empty_the_log() {
        let directory = tempfile::tempdir().unwrap();
        let generator = FlatGenerator::classic();
        let config = RegionConfig {
            spawn: Vec3::new(0.5, f64::from(generator.surface_y()), 0.5),
            ..config(0)
        };
        let (edge, worker_end) = in_process(256);
        let mut runner = opened(&on_disk(directory.path()), config).with_checkpoint_interval(50);
        runner.links().attach(worker_end);
        joined(&edge, &mut runner).await;
        // The log is in segments, which a checkpoint removes once it covers them.
        let log_length = || -> u64 {
            std::fs::read_dir(directory.path().join("log"))
                .unwrap()
                .map(|segment| segment.unwrap().metadata().unwrap().len())
                .sum()
        };
        let manifest = directory
            .path()
            .join("manifests/overworld/0.0/0.0.manifest");

        // Just after a checkpoint, so that the next one is 50 ticks away.
        step_until(&mut runner, |runner| {
            runner.region().tick_number() % 50 == 1
        });
        edge.send(dig(1)).await.unwrap();
        step(&mut runner);
        runner.store.flush();
        assert!(log_length() > 0, "the change was not logged");
        assert!(
            !manifest.exists(),
            "the chunk was saved before the checkpoint"
        );

        step_until(&mut runner, |runner| {
            runner.region().tick_number() % 50 == 0
        });
        runner.store.flush();
        assert!(manifest.exists(), "the checkpoint did not save the chunk");
        assert_eq!(log_length(), 0);
    }

    #[tokio::test]
    async fn unsubscribed_chunks_are_unloaded() {
        let (edge, worker_end) = in_process(256);
        let mut runner = runner(worker_end);

        edge.send(asking_for(square(1))).await.unwrap();
        step_until(&mut runner, |runner| {
            runner.region().loaded_chunk_count() == 9
        });

        let mut kept = square(1);
        let dropped = kept.split_off(4);
        edge.send(done_with(dropped)).await.unwrap();
        step(&mut runner);
        assert_eq!(runner.region().loaded_chunk_count(), 4);
        for position in kept {
            assert!(runner.region().chunk(position).is_some());
        }
    }

    #[tokio::test]
    async fn subscribing_twice_sends_one_snapshot() {
        let (mut edge, worker_end) = in_process(256);
        let mut runner = runner(worker_end);
        let position = ChunkPos::new(0, 0);

        for _ in 0..2 {
            edge.send(asking_for(vec![position])).await.unwrap();
        }
        step_until(&mut runner, |runner| {
            runner.region().loaded_chunk_count() == 1
        });
        step(&mut runner);

        assert!(matches!(
            edge.try_recv(),
            Ok(Some(WorkerToEdge::ChunkSnapshot { .. }))
        ));
        assert_eq!(edge.try_recv(), Ok(None));

        // One unsubscribe ends the subscription, whatever the number of subscribes.
        edge.send(done_with(vec![position])).await.unwrap();
        step(&mut runner);
        assert_eq!(runner.region().loaded_chunk_count(), 0);
    }

    /// A region shows the chunks it holds. Of a chunk another region holds it says who
    /// that is, loads nothing, and tells the edge nothing more: what an edge wants to
    /// see of it, it has to ask the region there for.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_subscription_to_a_chunk_of_another_region_is_answered_elsewhere_and_told_no_events()
    {
        for connect in KINDS {
            let (mut edge, worker_end) = connect(256);
            let mut runner = west(worker_end);
            let (inside, outside, far) = (
                ChunkPos::new(0, 0),
                ChunkPos::new(1, 0),
                ChunkPos::new(7, -3),
            );

            edge.send(asking_for(vec![outside, inside, far]))
                .await
                .unwrap();
            // The region asks the store about all three. Whose the two chunks beyond the
            // line are it says in the tick the answer is in, in the order of the chunks.
            // Its own chunk has to be loaded first.
            let elsewhere = |chunk| WorkerToEdge::Elsewhere {
                chunk,
                ask: 1,
                region: EAST,
            };
            assert_eq!(step_for(&mut runner, &mut edge), elsewhere(outside));
            assert_eq!(step_for(&mut runner, &mut edge), elsewhere(far));
            let answer = step_for(&mut runner, &mut edge);
            assert!(
                matches!(answer, WorkerToEdge::ChunkSnapshot { position, ask: 1, .. } if position == inside),
                "{answer:?}"
            );
            let link = &runner.links[&LinkId(0)];
            assert!(link.waiting.is_empty());
            let conditions: Vec<_> = link.subscriptions.iter().collect();
            let told = |condition| Subscription {
                kind: Ticket::Viewer,
                ask: 1,
                condition,
            };
            assert_eq!(
                conditions,
                [
                    (&inside, &told(Condition::Served)),
                    (&outside, &told(Condition::Elsewhere(EAST))),
                    (&far, &told(Condition::Elsewhere(EAST))),
                ]
            );
            assert_eq!(runner.region().loaded_chunk_count(), 1);

            // Nor is the edge told what happens out there: the next it hears is about
            // the chunk it does get.
            for (entity, chunk) in [(EntityId(8), outside), (EntityId(9), inside)] {
                edge.send(EdgeToWorker::Discard { entity, chunk })
                    .await
                    .unwrap();
            }
            let removed = RegionEvent::EntityRemoved {
                entity: EntityId(9),
                chunk: inside,
            };
            assert_eq!(events(step_for(&mut runner, &mut edge)), [removed]);

            // Asked again, the region says it again, with the number of that asking.
            edge.send(asking_for(vec![outside])).await.unwrap();
            let again = WorkerToEdge::Elsewhere {
                chunk: outside,
                ask: 2,
                region: EAST,
            };
            assert_eq!(step_for(&mut runner, &mut edge), again);

            // Letting go of it changes nothing of what is loaded.
            edge.send(done_with(vec![outside])).await.unwrap();
            edge.send(EdgeToWorker::Discard {
                entity: EntityId(10),
                chunk: inside,
            })
            .await
            .unwrap();
            step_for(&mut runner, &mut edge);
            assert_eq!(runner.region().loaded_chunk_count(), 1);
            let link = &runner.links[&LinkId(0)];
            assert!(!link.subscriptions.contains_key(&outside));
        }
    }

    /// Numbers are what lets an edge send messages again without any being applied
    /// twice. An edge whose numbers on one link have a gap or go back, or are where none
    /// belong, has lost track, and so has one that says hello twice. The runner stops
    /// listening to it; its players stay, for the edge to come back to.
    #[tokio::test]
    async fn a_link_whose_messages_are_out_of_order_is_closed() {
        let subscription = asking_for(vec![ORIGIN]);
        let wrong = |edge: &TestEdge| {
            [
                // The join was number 1.
                EdgeMessage {
                    number: Some(3),
                    body: walk_as(player(), 1, 5.0),
                },
                EdgeMessage {
                    number: Some(1),
                    body: walk_as(player(), 1, 5.0),
                },
                EdgeMessage {
                    number: Some(2),
                    body: subscription.clone(),
                },
                EdgeMessage::unnumbered(walk_as(player(), 1, 5.0)),
                EdgeMessage::unnumbered(edge.hello(0, &[], &[])),
            ]
        };
        for case in 0..5 {
            let (mut edge, worker_end) = in_process(256);
            let mut runner = runner(worker_end);
            joined(&edge, &mut runner).await;
            assert_eq!(runner.links.len(), 1);

            let wrong = wrong(&edge).into_iter().nth(case).unwrap();
            edge.send_as_is(wrong).await.unwrap();
            step_until(&mut runner, |runner| runner.links.is_empty());
            step(&mut runner);
            assert_eq!(runner.region().player_count(), 1, "case {case}");
            assert_eq!(x_of(&runner, player()), Some(SPAWN.x), "case {case}");
            // The edge finds its link closed after what it was told before.
            assert!(closed(&mut edge).await);
        }
    }

    /// The region has to know whose doing a change is, so an edge says hello before
    /// anything numbered. One that does not is out of order.
    #[tokio::test]
    async fn a_link_that_sends_something_numbered_before_saying_hello_is_closed() {
        let (edge, worker_end) = link::in_process(256);
        let mut runner = runner(worker_end);
        // Unnumbered messages concern nobody's doing and are taken.
        let subscribe = EdgeMessage {
            number: None,
            body: EdgeToWorker::Subscribe {
                ask: 1,
                chunks: vec![ChunkPos::new(0, 0)],
            },
        };

        edge.send(subscribe).await.unwrap();
        step(&mut runner);
        assert_eq!(runner.links.len(), 1);

        let join = EdgeMessage {
            number: Some(1),
            body: join(player(), "Notch"),
        };
        edge.send(join).await.unwrap();
        step_until(&mut runner, |runner| runner.links.is_empty());
        step(&mut runner);
        assert_eq!(runner.region().player_count(), 0);
        assert_eq!(runner.region().loaded_chunk_count(), 0);
    }

    /// An outbox entry stays in the outbox until the edge confirms it, however often
    /// it was sent.
    #[tokio::test]
    async fn an_outbox_entry_stays_until_the_edge_confirms_it() {
        let (mut edge, worker_end) = in_process(256);
        let mut runner = west(worker_end);
        look_east(&mut runner, &mut edge);
        edge.send(join(player(), "Notch")).await.unwrap();
        edge.send(walk(player(), 14.5)).await.unwrap();
        step(&mut runner);
        assert_eq!(outbox(&runner, &edge), (0, vec![]));
        received(&mut edge);

        // A block beyond the region, and a step beyond it.
        edge.send(dig(16)).await.unwrap();
        edge.send(walk(player(), 20.0)).await.unwrap();
        step(&mut runner);
        assert_eq!(outbox(&runner, &edge), (2, vec![1, 2]));
        assert!(matches!(
            received(&mut edge)[..],
            [
                WorkerToEdge::Outbox {
                    number: 1,
                    entry: Durable::Remote { .. },
                },
                WorkerToEdge::Outbox {
                    number: 2,
                    entry: Durable::Departed { .. },
                },
            ]
        ));
        // Nobody but the edge confirms anything.
        step(&mut runner);
        assert_eq!(outbox(&runner, &edge), (2, vec![1, 2]));

        edge.send(EdgeToWorker::Confirm { number: 1 })
            .await
            .unwrap();
        step(&mut runner);
        assert_eq!(outbox(&runner, &edge), (2, vec![2]));
        edge.send(EdgeToWorker::Confirm { number: 2 })
            .await
            .unwrap();
        step(&mut runner);
        assert_eq!(outbox(&runner, &edge), (2, vec![]));
        // How far the edge's messages are applied is noted too, and nothing was sent
        // again meanwhile.
        assert_eq!(runner.region().edge(edge.edge).unwrap().applied, 4);
        assert_eq!(received(&mut edge), []);
    }

    /// A link that ends takes its subscriptions with it and nothing else: the players
    /// are the edge's, which is expected back.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_link_that_ends_leaves_its_players_in_the_region() {
        for connect in KINDS {
            let (edge, worker_end) = connect(256);
            let mut runner = runner(worker_end);
            joined(&edge, &mut runner).await;
            assert_eq!(runner.region().player_count(), 1);
            let known = edge.edge;

            // Found out before the tick, so the tick is already one without the link:
            // its chunk is no longer needed.
            drop(edge);
            step_until(&mut runner, |runner| runner.links.is_empty());
            assert_eq!(runner.region().player_count(), 1);
            assert_eq!(runner.region().loaded_chunk_count(), 0);
            assert_eq!(runner.edges[&known].link, None);

            let ticks = runner.region().tick_number();
            step(&mut runner);
            assert_eq!(runner.region().tick_number(), ticks + 1);
            assert_eq!(runner.region().player_count(), 1);
        }
    }

    #[tokio::test]
    async fn an_edge_that_does_not_keep_up_loses_its_link_and_the_runner_goes_on() {
        // Room for the answer to the hello, which is two messages, and nobody reads.
        let (mut edge, worker_end) = in_process(2);
        let mut runner = runner(worker_end);
        step(&mut runner);
        assert_eq!(runner.links.len(), 1);
        edge.try_send(join(player(), "Notch")).unwrap();
        edge.try_send(asking_for(square(1))).unwrap();

        // The tick has run when it turns out that the player cannot be told that they
        // entered the world. They stay; the chunks are let go with the next tick.
        step(&mut runner);
        assert!(runner.links.is_empty());
        assert_eq!(runner.region().player_count(), 1);
        step_until(&mut runner, |runner| {
            runner.region().loaded_chunk_count() == 0
        });
        let ticks = runner.region().tick_number();
        step(&mut runner);
        assert_eq!(runner.region().tick_number(), ticks + 1);

        // The edge finds its link closed after what did fit.
        assert!(matches!(
            edge.everything()[..],
            [UNKNOWN, WorkerToEdge::Progress { .. }]
        ));
        assert!(closed(&mut edge).await);
    }

    /// An edge that loses its link connects again and finds its players as they were,
    /// with what the old link still carried applied.
    #[tokio::test]
    async fn an_edge_that_is_back_on_a_new_link_has_its_players_still() {
        let (old, old_end) = in_process(256);
        let mut runner = runner(old_end);
        joined(&old, &mut runner).await;
        let Some((entity, _)) = runner.region().player(player()) else {
            panic!("the player has joined");
        };

        // The last the old link carried is something the player did.
        old.send(walk(player(), 3.0)).await.unwrap();
        let (edge_end, new_end) = link::in_process(256);
        let mut new = old.again(edge_end, &runner);
        drop(old);
        runner.links().attach(new_end);
        new.send(new.hello(0, &[player()], &[ORIGIN]))
            .await
            .unwrap();
        step(&mut runner);

        assert_eq!(runner.links.len(), 1);
        assert_eq!(runner.region().player_count(), 1);
        let walked = Pose {
            position: Vec3::new(3.0, -60.0, 0.5),
            on_ground: true,
            ..Pose::at(SPAWN)
        };
        assert_eq!(runner.region().player(player()), Some((entity, walked)));
        // The answer is of the region as it was before the tick that took the step.
        let told = new.everything();
        assert_eq!(told[0], RESUMED);
        let WorkerToEdge::Presence {
            answer:
                Presence::Present {
                    entity: present,
                    pose,
                    ..
                },
            ..
        } = &told[1]
        else {
            panic!("expected the player to be present, got {told:?}");
        };
        assert_eq!((*present, *pose), (entity, Pose::at(SPAWN)));
        // The chunk was let go of and asked for within the same tick, so it stayed.
        assert_eq!(runner.region().loaded_chunk_count(), 1);
    }

    /// The region can also learn of a player's new connection before it learns that the
    /// old one is gone, when the link they had lingers or is served later.
    #[tokio::test]
    async fn a_player_who_joins_through_another_link_is_taken_over_by_it() {
        let (first, first_end) = in_process(256);
        let (mut second, second_end) = in_process(256);
        let mut runner = runner(first_end);
        runner.links().attach(second_end);
        let entity = |runner: &RegionRunner| Some(runner.region().player(player())?.0);

        first.send(join(player(), "Notch")).await.unwrap();
        step(&mut runner);
        assert_eq!(entity(&runner), Some(EntityId(1)));

        // The new entity does not take the step that came through the first link.
        first.send(walk(player(), 3.0)).await.unwrap();
        second.send(join(player(), "Notch")).await.unwrap();
        step(&mut runner);
        assert_eq!(
            runner.region().player(player()),
            Some((EntityId(2), Pose::at(SPAWN)))
        );
        assert_eq!(runner.region().player_count(), 1);
        assert!(matches!(
            received(&mut second)[..],
            [WorkerToEdge::ToPlayer {
                event: PlayerEvent::Spawned {
                    entity_id: EntityId(2),
                    ..
                },
                ..
            }]
        ));

        // What the link they had still says about them no longer counts.
        first.send(walk(player(), 5.0)).await.unwrap();
        first
            .send(EdgeToWorker::PlayerLeave {
                player: player(),
                entity: None,
            })
            .await
            .unwrap();
        step(&mut runner);
        assert_eq!(
            runner.region().player(player()),
            Some((EntityId(2), Pose::at(SPAWN)))
        );

        // The other way round: they join through the link that is served first, and
        // the link they belong to ends within the same tick.
        first.send(join(player(), "Notch")).await.unwrap();
        drop(second);
        step(&mut runner);
        assert_eq!(runner.links.len(), 1);
        assert_eq!(entity(&runner), Some(EntityId(3)));
    }

    #[tokio::test]
    async fn only_the_link_a_player_belongs_to_acts_for_them() {
        let (owner, owner_end) = in_process(256);
        let (stranger, stranger_end) = in_process(256);
        let mut runner = runner(owner_end);
        runner.links().attach(stranger_end);
        let leave = || EdgeToWorker::PlayerLeave {
            player: player(),
            entity: None,
        };

        owner.send(join(player(), "Notch")).await.unwrap();
        step(&mut runner);
        stranger.send(walk(player(), 5.0)).await.unwrap();
        stranger.send(leave()).await.unwrap();
        step(&mut runner);
        assert_eq!(
            runner.region().player(player()),
            Some((EntityId(1), Pose::at(SPAWN)))
        );

        owner.send(leave()).await.unwrap();
        step(&mut runner);
        assert_eq!(runner.region().player_count(), 0);
    }

    /// The region handles what players did only after all of them have joined or left.
    /// A player who is back within the tick must not begin with what they did before
    /// they left, least of all with the numbers of those inputs.
    #[tokio::test]
    async fn a_player_who_is_back_within_the_tick_starts_afresh() {
        let (edge, worker_end) = in_process(256);
        let mut runner = runner(worker_end);
        edge.send(join(player(), "Notch")).await.unwrap();
        step(&mut runner);

        edge.send(walk_as(player(), 900, 3.0)).await.unwrap();
        edge.send(EdgeToWorker::PlayerLeave {
            player: player(),
            entity: None,
        })
        .await
        .unwrap();
        edge.send(join(player(), "Notch")).await.unwrap();
        step(&mut runner);
        assert_eq!(
            runner.region().player(player()),
            Some((EntityId(2), Pose::at(SPAWN)))
        );

        // Their new connection numbers what they do from the start again, and what
        // they do is of the stay that began with the second join.
        let anew = of_entity(EntityId(2), walk_as(player(), 1, 4.0));
        edge.send(anew).await.unwrap();
        step(&mut runner);
        let (_, pose) = runner.region().player(player()).unwrap();
        assert_eq!(pose.position.x, 4.0);
    }

    /// The eastern neighbour lets a player go, who arrives here with the entity they
    /// had there, and walks back.
    #[tokio::test(flavor = "multi_thread")]
    async fn players_arrive_with_their_entity_and_depart_through_their_link() {
        for connect in KINDS {
            let (mut edge, worker_end) = connect(256);
            let (mut bystander, bystander_end) = connect(256);
            let mut runner = west(worker_end);
            runner.links().attach(bystander_end);
            look_east(&mut runner, &mut edge);
            let status = runner.status();
            // An id that only another region can have given out.
            let entity = EntityIds::block(3).unwrap().first;
            let arriving = transfer(entity);

            edge.send(EdgeToWorker::PlayerArrive {
                player: player(),
                transfer: arriving.clone(),
            })
            .await
            .unwrap();
            step_until(&mut runner, |runner| runner.region().player_count() == 1);
            assert_eq!(
                runner.region().player(player()),
                Some((entity, arriving.pose))
            );
            assert_eq!(status.arrivals.load(Ordering::Relaxed), 1);

            // The input the neighbour applied last is sent again and changes nothing;
            // the one after it takes the player back east.
            let walk_to = |number, x| of_entity(entity, walk_as(player(), number, x));
            edge.send(walk_to(7, 12.0)).await.unwrap();
            edge.send(walk_to(8, 20.0)).await.unwrap();
            let message = step_for(&mut runner, &mut edge);
            let leaving = PlayerTransfer {
                pose: Pose {
                    position: Vec3::new(20.0, -60.0, 0.5),
                    on_ground: true,
                    ..arriving.pose
                },
                last_input: 8,
                ..arriving
            };
            let departed = WorkerToEdge::Outbox {
                number: 1,
                entry: Durable::Departed {
                    player: player(),
                    transfer: leaving,
                    to: EAST,
                },
            };
            assert_eq!(message, departed);
            assert_eq!(runner.region().player_count(), 0);
            assert_eq!(status.departures.load(Ordering::Relaxed), 1);
            // The region is done with the player, and nobody else was told anything.
            assert_eq!(bystander.try_recv(), Ok(None));
        }
    }

    /// What a player does to a block of another region goes to the player's edge to be
    /// passed on. What reaches this region that way is answered to the link it came
    /// through, after the tick's changes: done, or with what is left for a third region.
    #[tokio::test(flavor = "multi_thread")]
    async fn actions_on_blocks_of_other_regions_go_through_the_edges() {
        for connect in KINDS {
            let (mut edge, worker_end) = connect(256);
            let (mut other, other_end) = connect(256);
            let mut runner = west(worker_end);
            runner.links().attach(other_end);
            look_east(&mut runner, &mut edge);
            let origin = ChunkPos::new(0, 0);
            let subscribe = || asking_for(vec![origin]);

            // A player of this region, close to where it ends at x = 16, breaks a block
            // beyond that.
            edge.send(join(player(), "Notch")).await.unwrap();
            edge.send(subscribe()).await.unwrap();
            other.send(subscribe()).await.unwrap();
            step_until(&mut runner, |runner| {
                runner.region().chunk(origin).is_some()
            });
            edge.send(walk(player(), 14.5)).await.unwrap();
            step_until(&mut runner, |runner| {
                runner
                    .region()
                    .player(player())
                    .is_some_and(|(_, pose)| pose.position.x == 14.5)
            });
            // Both edges watch the chunk and so are told of that step. Once they have
            // been, everything sent before it has arrived as well, however long a link
            // takes over it, and what follows is all there is to come.
            for link in [&mut edge, &mut other] {
                loop {
                    let WorkerToEdge::TickDelta { events, .. } = step_for(&mut runner, link) else {
                        continue;
                    };
                    let there = |event: &RegionEvent| matches!(event, RegionEvent::EntityMoved { pose, .. } if pose.position.x == 14.5);
                    if events.iter().any(there) {
                        break;
                    }
                }
            }
            edge.send(dig_by(player(), 16, 7)).await.unwrap();
            let request = RemoteAction {
                player: player(),
                sequence: 7,
                step: RemoteStep::Break {
                    position: BlockPos::new(16, -61, 0),
                },
            };
            assert_eq!(
                step_for(&mut runner, &mut edge),
                WorkerToEdge::Outbox {
                    number: 1,
                    entry: Durable::Remote {
                        action: request,
                        to: Some(EAST),
                    },
                }
            );
            // Nobody is told that it was handled, and the other edge hears nothing.
            step(&mut runner);
            assert_eq!(received(&mut edge), []);
            assert_eq!(received(&mut other), []);

            // The other edge passes on what a player of another region did to a block
            // of this one. It hears what that changed, then that it is done.
            let block = BlockPos::new(15, -61, 0);
            other
                .send(EdgeToWorker::Remote(RemoteAction {
                    player: other_player(),
                    sequence: 3,
                    step: RemoteStep::Break { position: block },
                }))
                .await
                .unwrap();
            let changed = [RegionEvent::BlockChanged {
                position: block,
                state: clustine_data::blocks::AIR,
            }];
            assert_eq!(events(step_for(&mut runner, &mut other)), changed);
            assert_eq!(
                step_for(&mut runner, &mut other),
                WorkerToEdge::Outbox {
                    number: 1,
                    entry: Durable::RemoteDone {
                        player: other_player(),
                        sequence: 3,
                    },
                }
            );
            // The first edge watches the chunk too and is told of the change, but not
            // that anything is done: it did not ask.
            assert_eq!(events(step_for(&mut runner, &mut edge)), changed);
            step(&mut runner);
            assert_eq!(received(&mut edge), []);

            // A block to be placed beyond this region against one of this region's is
            // found to have something to be placed against, and passed on.
            let stone = clustine_data::blocks::STONE;
            let against = BlockPos::new(15, -62, 0);
            let target = BlockPos::new(16, -62, 0);
            let placer = Vec3::new(17.5, -60.0, 0.5);
            other
                .send(EdgeToWorker::Remote(RemoteAction {
                    player: other_player(),
                    sequence: 4,
                    step: RemoteStep::PlaceAgainst {
                        against,
                        target,
                        block: stone,
                        placer,
                    },
                }))
                .await
                .unwrap();
            assert_eq!(
                step_for(&mut runner, &mut other),
                WorkerToEdge::Outbox {
                    number: 2,
                    entry: Durable::Remote {
                        action: RemoteAction {
                            player: other_player(),
                            sequence: 4,
                            step: RemoteStep::Place {
                                target,
                                block: stone,
                                placer,
                            },
                        },
                        to: Some(EAST),
                    },
                }
            );
            step(&mut runner);
            assert_eq!(received(&mut edge), []);
            assert_eq!(received(&mut other), []);
        }
    }

    /// A transfer can still be on its way when the player has long connected anew and
    /// joined. The region keeps the player it has: theirs is the later stay, which is
    /// the one with the higher entity id.
    #[tokio::test]
    async fn an_arrival_does_not_take_a_player_from_the_link_they_belong_to() {
        let (first, first_end) = in_process(256);
        let (second, second_end) = in_process(256);
        let mut runner = runner(first_end);
        runner.links().attach(second_end);
        let status = runner.status();
        let leave = || EdgeToWorker::PlayerLeave {
            player: player(),
            entity: None,
        };

        // The stay that is still on its way had the first entity the region gave out,
        // and the one that began since has the second.
        first.send(join(player(), "Notch")).await.unwrap();
        first.send(leave()).await.unwrap();
        first.send(join(player(), "Notch")).await.unwrap();
        step(&mut runner);
        second
            .send(EdgeToWorker::PlayerArrive {
                player: player(),
                transfer: transfer(EntityId(1)),
            })
            .await
            .unwrap();
        step(&mut runner);
        assert_eq!(
            runner.region().player(player()),
            Some((EntityId(2), Pose::at(SPAWN)))
        );
        assert_eq!(status.arrivals.load(Ordering::Relaxed), 0);

        // The player is still the first link's to take out of the region.
        second.send(leave()).await.unwrap();
        step(&mut runner);
        assert_eq!(runner.region().player_count(), 1);
        first.send(leave()).await.unwrap();
        step(&mut runner);
        assert_eq!(runner.region().player_count(), 0);
    }

    /// A player steps out of the region, but their edge has no link and never comes
    /// back for them. When the region forgets the edge, everyone is told that the
    /// entity is gone: it was seen to leave, and nobody will pass it on.
    #[tokio::test]
    async fn an_edge_that_stays_away_is_gone_with_its_players_and_departures() {
        let (mut edge, edge_end) = in_process(256);
        let (mut watcher, watcher_end) = in_process(256);
        let mut runner = west(edge_end).with_gone_after(5);
        runner.links().attach(watcher_end);
        look_east(&mut runner, &mut edge);
        edge.send(join(player(), "Notch")).await.unwrap();
        edge.send(join(other_player(), "Jeb")).await.unwrap();
        watcher.send(asking_for(vec![ORIGIN])).await.unwrap();
        step_until(&mut runner, |runner| {
            runner.region().loaded_chunk_count() == 1 && runner.region().player_count() == 2
        });
        step(&mut runner);
        received(&mut watcher);
        let entity = |runner: &RegionRunner, player| runner.region().player(player).unwrap().0;
        let (leaving, staying) = (entity(&runner, player()), entity(&runner, other_player()));
        let known = edge.edge;

        // The last the link carries is the step out of the region.
        edge.send(walk(player(), 20.0)).await.unwrap();
        drop(edge);
        let away_since = runner.region().tick_number();
        step(&mut runner);
        assert!(runner.edges[&known].link.is_none());
        assert_eq!(runner.region().edge(known).unwrap().outbox.len(), 1);
        assert_eq!(runner.region().player_count(), 1);
        // The entity lives on as far as anyone knows.
        let seen = received(&mut watcher);
        let [WorkerToEdge::TickDelta { events, .. }] = &seen[..] else {
            panic!("expected a delta, got {seen:?}");
        };
        assert!(matches!(events[..], [RegionEvent::EntityMoved { .. }]));

        // Five ticks without a link pass, and the sixth forgets the edge.
        while runner.region().tick_number() < away_since + 5 {
            step(&mut runner);
            assert_eq!(runner.region().player_count(), 1);
            assert_eq!(received(&mut watcher), []);
        }
        step(&mut runner);
        assert_eq!(runner.region().tick_number(), away_since + 6);
        assert_eq!(runner.region().player_count(), 0);
        assert!(runner.region().edge(known).is_none());
        assert!(!runner.edges.contains_key(&known));
        let seen = received(&mut watcher);
        let [WorkerToEdge::TickDelta { events, .. }] = &seen[..] else {
            panic!("expected a delta, got {seen:?}");
        };
        let mut removed: Vec<_> = events
            .iter()
            .map(|event| match event {
                RegionEvent::EntityRemoved { entity, chunk } => (*entity, *chunk),
                other => panic!("expected a removal, got {other:?}"),
            })
            .collect();
        removed.sort();
        let mut expected = vec![(leaving, ChunkPos::new(1, 0)), (staying, ORIGIN)];
        expected.sort();
        assert_eq!(removed, expected);

        // An edge with a link is never gone, however little it says.
        let ticks = runner.region().tick_number();
        while runner.region().tick_number() < ticks + 10 {
            step(&mut runner);
        }
        assert!(runner.region().edge(watcher.edge).is_some());
    }

    #[tokio::test]
    async fn a_refused_player_is_told_through_the_outbox_of_their_edge() {
        let (mut edge, worker_end) = in_process(256);
        // A region with a single entity id to give out.
        let entity_ids = EntityIds {
            first: EntityId(1),
            end: EntityId(2),
        };
        let store = clustine_worldstore::spawn(Arc::new(FlatGenerator::classic()));
        let region = Region::new(config(0), entity_ids, Holdings::default());
        let mut runner = RegionRunner::with_store(region, Box::new(store));
        runner.links().attach(worker_end);

        edge.send(join(player(), "Notch")).await.unwrap();
        edge.send(join(other_player(), "Jeb")).await.unwrap();
        step(&mut runner);
        let told = received(&mut edge);
        let refused = WorkerToEdge::Outbox {
            number: 1,
            entry: Durable::Refused {
                player: other_player(),
            },
        };
        assert!(
            matches!(&told[..], [WorkerToEdge::ToPlayer { .. }, last] if *last == refused),
            "{told:?}"
        );
    }

    #[tokio::test]
    async fn the_status_follows_the_region() {
        let (mut edge, worker_end) = in_process(256);
        let mut runner = west(worker_end);
        let status = runner.status();
        let read = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        let population = || (read(&status.players), read(&status.chunks));
        let traffic = || (read(&status.arrivals), read(&status.departures));
        assert_eq!(
            (read(&status.tick), population(), traffic()),
            (0, (0, 0), (0, 0))
        );
        // Knowing whose a chunk is does not make it the region's.
        look_east(&mut runner, &mut edge);
        assert_eq!((population(), read(&status.held)), ((0, 0), 0));

        // Joining is not arriving.
        joined(&edge, &mut runner).await;
        assert_eq!(read(&status.tick), runner.region().tick_number());
        assert_eq!((population(), traffic()), ((1, 1), (0, 0)));

        // Neither is arriving a second time.
        for _ in 0..2 {
            edge.send(EdgeToWorker::PlayerArrive {
                player: other_player(),
                transfer: transfer(EntityIds::block(3).unwrap().first),
            })
            .await
            .unwrap();
            step(&mut runner);
            assert_eq!((population(), traffic()), ((2, 1), (1, 0)));
        }

        // Both step out of the region; one of them comes back.
        edge.send(walk(player(), 20.0)).await.unwrap();
        let arrived = EntityIds::block(3).unwrap().first;
        let out = of_entity(arrived, walk_as(other_player(), 8, 20.0));
        edge.send(out).await.unwrap();
        step(&mut runner);
        assert_eq!((population(), traffic()), ((0, 1), (1, 2)));
        edge.send(EdgeToWorker::PlayerArrive {
            player: other_player(),
            transfer: transfer(EntityIds::block(3).unwrap().first),
        })
        .await
        .unwrap();
        edge.send(done_with(vec![ChunkPos::new(0, 0)]))
            .await
            .unwrap();
        step(&mut runner);
        assert_eq!((population(), traffic()), ((1, 0), (2, 2)));
        assert_eq!(read(&status.tick), runner.region().tick_number());
        // The store has granted the region the one chunk its players stood in and its
        // edge looked at. It is of the region's stripe, so it stays the region's when
        // nothing is loaded any more.
        assert_eq!(read(&status.held), 1);
        assert_eq!(status.crowds(), [(ChunkPos::new(0, 0), 1)]);
    }

    #[test]
    fn ticks_keep_their_pace() {
        let (_edge, worker_end) = in_process(256);
        let mut runner = runner(worker_end);
        let stop = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| runner.run(&stop));
            thread::sleep(TICK * 10 + TICK / 2);
            stop.store(true, Ordering::Relaxed);
        });
        // Eleven ticks fit into ten and a half tick lengths; a loaded machine may run
        // fewer, but a runner that does not pace itself would run thousands.
        let ticks = runner.region().tick_number();
        assert!((5..=12).contains(&ticks), "{ticks} ticks");
    }

    /// Waits until the store has answered `count` commits that the gate holds back.
    fn wait_for_kept(runner: &mut RegionRunner, gate: &GateControl, count: usize) {
        for _ in 0..20_000 {
            assert!(runner.take_replies());
            if gate.kept() >= count {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("the store did not answer the commits");
    }

    /// What a tick did is shown to nobody before the store has it, and the region does
    /// not run far ahead of what the store has.
    #[tokio::test]
    async fn nothing_of_a_tick_is_published_before_its_commit_is_confirmed() {
        let (mut edge, worker_end) = in_process(256);
        let (mut runner, gate) = gated(&memory(), config(0));
        runner.links().attach(worker_end);
        gate.hold();

        // Each tick has something of the player's to apply, so each is to be committed.
        edge.send(join(player(), "Notch")).await.unwrap();
        for tick in 1..=20 {
            runner.step();
            let ran = tick.min(MAX_TICKS_AHEAD as u64);
            assert_eq!(runner.region().tick_number(), ran);
            assert_eq!(runner.status().tick.load(Ordering::Relaxed), ran);
            edge.send(walk(player(), tick as f64)).await.unwrap();
        }
        // The store has every one of them on disk, and still nobody is told anything.
        wait_for_kept(&mut runner, &gate, MAX_TICKS_AHEAD);
        runner.step();
        assert_eq!(gate.commits(), (1..=8).collect::<Vec<_>>());
        assert_eq!(runner.region().tick_number(), 8);
        assert_eq!(edge.everything(), []);
        // Not even the player's position, which is seven steps along.
        assert_eq!(x_of(&runner, player()), Some(7.0));

        // Once the commits are answered, everything arrives in the order it happened,
        // and the region goes on with what has come in meanwhile.
        gate.release();
        step(&mut runner);
        assert_eq!(runner.region().tick_number(), 9);
        assert_eq!(x_of(&runner, player()), Some(20.0));
        let told = edge.everything();
        assert_eq!(told[0], UNKNOWN);
        assert!(matches!(
            told[1],
            WorkerToEdge::ToPlayer {
                event: PlayerEvent::Spawned { .. },
                ..
            }
        ));
        let applied: Vec<_> = told[2..]
            .iter()
            .map(|message| match message {
                WorkerToEdge::Progress { applied, .. } => *applied,
                other => panic!("expected progress, got {other:?}"),
            })
            .collect();
        // The join and seven steps in a tick each, then the other thirteen in one.
        assert_eq!(applied, [1, 2, 3, 4, 5, 6, 7, 8, 21]);
    }

    /// A tick in which nothing changed but its number is not worth a commit. It counts
    /// as committed once the tick before it is, and not earlier.
    #[tokio::test]
    async fn a_tick_that_changes_nothing_sends_no_commit_and_is_published() {
        let (mut edge, worker_end) = in_process(256);
        let (mut runner, gate) = gated(&memory(), config(0));
        runner.links().attach(worker_end);

        // The hello makes the edge known, which is a change. Loading a chunk and showing
        // it are none: chunks are not part of the region's state.
        edge.send(asking_for(vec![ORIGIN])).await.unwrap();
        let (position, _) = snapshot(step_for(&mut runner, &mut edge));
        assert_eq!(position, ORIGIN);
        assert!(runner.region().tick_number() > 1);
        assert_eq!(gate.commits(), [1]);

        // Behind a tick that is not confirmed, ticks that changed nothing wait as well,
        // and count towards how far ahead the region may be.
        gate.hold();
        edge.send(join(player(), "Notch")).await.unwrap();
        let before = runner.region().tick_number();
        runner.step();
        edge.send(done_with(vec![ORIGIN])).await.unwrap();
        for _ in 0..20 {
            runner.step();
        }
        wait_for_kept(&mut runner, &gate, 1);
        assert_eq!(runner.region().tick_number(), before + 8);
        assert_eq!(gate.commits(), [1, before + 1]);
        assert_eq!(edge.everything(), []);

        gate.release();
        step(&mut runner);
        let told = edge.everything();
        assert!(
            matches!(
                told[..],
                [
                    WorkerToEdge::TickDelta { .. },
                    WorkerToEdge::ToPlayer { .. },
                    WorkerToEdge::Progress { applied: 1, .. },
                ]
            ),
            "{told:?}"
        );
        assert_eq!(gate.commits(), [1, before + 1]);
    }

    /// The owner of a region dies after the store has a tick and before any edge has
    /// heard of it. Whoever carries on with the region tells the edge what it missed:
    /// what is in the outbox, and where its players are as of what is on disk.
    #[tokio::test]
    async fn a_runner_restored_after_a_commit_that_was_never_published_tells_it_on_hello() {
        let world = Divided::stripes();
        let (mut edge, worker_end) = in_process(256);
        let (mut runner, gate) = world.gated(RegionId(0), config(0));
        runner.links().attach(worker_end);
        look_east(&mut runner, &mut edge);
        edge.send(join(player(), "Notch")).await.unwrap();
        edge.send(join(other_player(), "Jeb")).await.unwrap();
        step(&mut runner);
        let entity = runner.region().player(other_player()).unwrap().0;
        edge.everything();

        // One steps out of the region and the other aside, in a tick that the store
        // gets and the edge does not.
        gate.hold();
        edge.send(walk(player(), 20.0)).await.unwrap();
        let aside = walk(other_player(), 5.0);
        let EdgeToWorker::Input { number: input, .. } = aside else {
            unreachable!("a step is an input");
        };
        edge.send(aside).await.unwrap();
        runner.step();
        wait_for_kept(&mut runner, &gate, 1);
        assert_eq!(edge.everything(), []);
        let tick = runner.region().tick_number();
        let committed = runner.region().state();
        drop(runner);
        assert!(closed(&mut edge).await);

        let mut restored = world.runner(RegionId(0), 1);
        assert_eq!(restored.region().state(), committed);
        let (edge_end, worker_end) = link::in_process(256);
        let mut again = edge.again(edge_end, &restored);
        restored.links().attach(worker_end);
        again
            .send(again.hello(0, &[player(), other_player()], &[]))
            .await
            .unwrap();
        step(&mut restored);
        assert_eq!(restored.region().tick_number(), tick + 1);

        let told = again.everything();
        let [
            RESUMED,
            WorkerToEdge::Outbox {
                number: 1,
                entry: Durable::Departed { player: gone, .. },
            },
            WorkerToEdge::Presence {
                player: first,
                answer: Presence::Absent,
            },
            WorkerToEdge::Presence {
                player: second,
                answer,
            },
            WorkerToEdge::Progress { applied: 4, inputs },
        ] = &told[..]
        else {
            panic!("unexpected {told:?}");
        };
        assert_eq!(
            (*gone, *first, *second),
            (player(), player(), other_player())
        );
        assert_eq!(*inputs, []);
        let pose = Pose {
            position: Vec3::new(5.0, -60.0, 0.5),
            on_ground: true,
            ..Pose::at(SPAWN)
        };
        assert_eq!(
            *answer,
            Presence::Present {
                entity,
                pose,
                hotbar: [None; HOTBAR_SLOTS],
                selected_slot: 0,
                last_input: input,
                handled: None,
            }
        );
    }

    /// A region comes back from disk as it was committed: its players and whose they
    /// are, what it owes each edge and how far it has got with what each edge sent. It
    /// takes the edge's messages up from there.
    #[tokio::test]
    async fn a_restored_region_carries_on_where_the_store_has_it() {
        let directory = tempfile::tempdir().unwrap();
        let (mut edge, worker_end) = in_process(256);
        let mut first = Divided::stripes_in(directory.path()).runner(RegionId(0), 1);
        first.links().attach(worker_end);
        look_east(&mut first, &mut edge);
        edge.send(join(player(), "Notch")).await.unwrap();
        edge.send(walk(player(), 14.5)).await.unwrap();
        step(&mut first);
        // A block beyond the region, which leaves an entry in the outbox.
        edge.send(dig(16)).await.unwrap();
        step(&mut first);
        let state = first.region().state();
        assert_eq!(state.players[&player()].edge, edge.edge);
        assert_eq!(state.edges[&edge.edge].applied, 3);
        assert_eq!(state.edges[&edge.edge].outbox.len(), 1);
        // It ticks on without anything changing, and is gone without a word: neither a
        // checkpoint nor a goodbye.
        step(&mut first);
        step(&mut first);
        assert_eq!(first.region().tick_number(), state.tick + 2);
        drop(first);

        let mut second = Divided::stripes_in(directory.path()).runner(RegionId(0), 1);
        // Ticks go on from the last the store has.
        assert_eq!(second.region().state(), state);
        assert_eq!(second.status().tick.load(Ordering::Relaxed), state.tick);
        assert_eq!(second.status().players.load(Ordering::Relaxed), 1);
        step(&mut second);
        assert_eq!(second.region().tick_number(), state.tick + 1);

        // The edge is back and sends again what it has not heard to be applied. What the
        // region has is dropped, and the next is taken.
        let (edge_end, worker_end) = link::in_process(256);
        let again = edge.again(edge_end, &second);
        second.links().attach(worker_end);
        again.send(again.hello(0, &[], &[])).await.unwrap();
        again.send_as(3, walk(player(), 3.0)).await;
        step(&mut second);
        assert_eq!(second.links.len(), 1);
        assert_eq!(x_of(&second, player()), Some(14.5));
        again.send_as(4, walk(player(), 13.0)).await;
        step(&mut second);
        assert_eq!(x_of(&second, player()), Some(13.0));
        assert_eq!(second.region().edge(edge.edge).unwrap().applied, 4);

        // A message that leaves a gap ends the link.
        again.send_as(6, walk(player(), 12.0)).await;
        step(&mut second);
        assert!(second.links.is_empty());
        assert_eq!(x_of(&second, player()), Some(13.0));

        // And so does a first message beyond the next one the region expects.
        let (edge_end, worker_end) = link::in_process(256);
        let ahead = edge.again(edge_end, &second);
        second.links().attach(worker_end);
        ahead.send(ahead.hello(0, &[], &[])).await.unwrap();
        ahead.send_as(6, walk(player(), 12.0)).await;
        step(&mut second);
        assert!(second.links.is_empty());
        assert_eq!(second.region().edge(edge.edge).unwrap().applied, 4);
    }

    /// A start of an edge that a later one has replaced is told so at once, and its
    /// link is closed.
    #[tokio::test]
    async fn an_edge_with_a_lower_start_than_the_region_knows_is_superseded() {
        // Whether the region has run a tick with the later start or not.
        for stepped in [true, false] {
            let id = EdgeId::from_name("superseded");
            let (edge_end, worker_end) = link::in_process(256);
            let mut later = TestEdge::silent(edge_end, id, 5);
            let mut runner = runner(worker_end);
            later.send(later.hello(0, &[], &[])).await.unwrap();
            later.send(join(player(), "Notch")).await.unwrap();
            if stepped {
                step(&mut runner);
            }

            let (edge_end, worker_end) = link::in_process(256);
            let mut earlier = TestEdge::silent(edge_end, id, 3);
            runner.links().attach(worker_end);
            earlier.send(earlier.hello(0, &[], &[])).await.unwrap();
            earlier.send(join(other_player(), "Jeb")).await.unwrap();
            step(&mut runner);

            let superseded = WorkerToEdge::Welcome(Welcome::Superseded);
            assert_eq!(earlier.everything(), [superseded]);
            assert!(closed(&mut earlier).await);
            // The later start is untouched by it.
            assert_eq!(runner.links.len(), 1);
            assert_eq!(runner.region().edge(id).unwrap().start, 5);
            assert_eq!(runner.region().player_count(), 1);
            assert!(runner.region().player(player()).is_some());
            assert_eq!(later.everything()[0], UNKNOWN);
        }
    }

    /// An edge the region knows says hello on a new link. It is told that the region
    /// carries on, then what it has not seen of its outbox, then where each player it
    /// asks about is, before anything the tick itself has to say. Its old link is closed.
    #[tokio::test]
    async fn an_edge_that_resumes_is_told_what_it_missed_before_anything_else() {
        let (mut edge, edge_end) = in_process(256);
        let (stranger, stranger_end) = in_process(256);
        let mut runner = west(edge_end);
        runner.links().attach(stranger_end);
        look_east(&mut runner, &mut edge);
        edge.send(join(player(), "Notch")).await.unwrap();
        edge.send(join(third_player(), "Dinnerbone")).await.unwrap();
        stranger.send(join(other_player(), "Jeb")).await.unwrap();
        edge.send(walk(player(), 14.5)).await.unwrap();
        step(&mut runner);
        // Two entries: a block beyond the region, then a step beyond it.
        edge.send(dig(16)).await.unwrap();
        step(&mut runner);
        edge.send(walk(player(), 20.0)).await.unwrap();
        step(&mut runner);
        assert_eq!(outbox(&runner, &edge), (2, vec![1, 2]));
        let entity = runner.region().player(third_player()).unwrap().0;

        // The edge has seen the first entry. With the hello comes something its
        // remaining player did.
        let (edge_end, worker_end) = link::in_process(256);
        let mut again = edge.again(edge_end, &runner);
        runner.links().attach(worker_end);
        let asked = [player(), third_player(), other_player()];
        again.send(again.hello(1, &asked, &[])).await.unwrap();
        let step_aside = of_entity(entity, walk(third_player(), 2.0));
        let EdgeToWorker::Input { number: input, .. } = step_aside else {
            unreachable!("a step is an input");
        };
        again.send(step_aside).await.unwrap();
        step(&mut runner);

        let present = Presence::Present {
            entity,
            // As of before the tick that took the step.
            pose: Pose::at(SPAWN),
            hotbar: [None; HOTBAR_SLOTS],
            selected_slot: 0,
            last_input: 0,
            handled: None,
        };
        let told = again.everything();
        let [
            RESUMED,
            WorkerToEdge::Outbox {
                number: 2,
                entry: Durable::Departed { .. },
            },
            WorkerToEdge::Presence {
                player: first,
                answer: Presence::Absent,
            },
            WorkerToEdge::Presence {
                player: second,
                answer,
            },
            // Of another edge, so not this one's to ask about.
            WorkerToEdge::Presence {
                player: third,
                answer: Presence::Absent,
            },
            WorkerToEdge::Progress { applied: 6, inputs },
        ] = &told[..]
        else {
            panic!("unexpected {told:?}");
        };
        assert_eq!([*first, *second, *third], asked);
        assert_eq!(*answer, present);
        assert_eq!(*inputs, [(third_player(), input)]);
        // The hello confirmed the entry it had seen.
        assert_eq!(outbox(&runner, &again), (2, vec![2]));
        // Nothing went to the link the edge had before, which is closed.
        received(&mut edge);
        assert!(closed(&mut edge).await);
        assert_eq!(runner.links.len(), 2);
    }

    /// An edge that has started anew is unknown to the region, which removes what the
    /// earlier start left behind: its players, and what it had sent that was still
    /// waiting for a tick.
    #[tokio::test]
    async fn an_edge_with_a_higher_start_is_unknown_and_what_its_earlier_start_left_is_dropped() {
        let id = EdgeId::from_name("restarted");
        let (edge_end, worker_end) = link::in_process(256);
        let mut old = TestEdge::silent(edge_end, id, 1);
        let (mut watcher, watcher_end) = in_process(256);
        let mut runner = runner(worker_end);
        runner.links().attach(watcher_end);
        old.send(old.hello(0, &[], &[])).await.unwrap();
        old.send(join(player(), "Notch")).await.unwrap();
        watcher.send(asking_for(vec![ORIGIN])).await.unwrap();
        step_until(&mut runner, |runner| {
            runner.region().loaded_chunk_count() == 1
        });
        step(&mut runner);
        received(&mut watcher);
        let entity = runner.region().player(player()).unwrap().0;

        // The earlier start sends more, and the later one says hello before a tick has
        // taken any of it.
        old.send(join(other_player(), "Jeb")).await.unwrap();
        old.send(walk(player(), 3.0)).await.unwrap();
        old.send(EdgeToWorker::Discard {
            entity: EntityId(77),
            chunk: ORIGIN,
        })
        .await
        .unwrap();
        let (edge_end, worker_end) = link::in_process(256);
        let mut new = TestEdge::silent(edge_end, id, 2);
        runner.links().attach(worker_end);
        new.send(new.hello(4, &[player()], &[])).await.unwrap();
        step(&mut runner);

        assert_eq!(runner.region().player_count(), 0);
        let state = runner.region().edge(id).unwrap();
        assert_eq!((state.start, state.applied, state.sent), (2, 0, 0));
        assert_eq!(
            new.everything(),
            [
                UNKNOWN,
                WorkerToEdge::Presence {
                    player: player(),
                    answer: Presence::Absent,
                },
                WorkerToEdge::Progress {
                    applied: 0,
                    inputs: vec![],
                },
            ]
        );
        // Those watching see the earlier start's player go, and nothing else.
        let removed = RegionEvent::EntityRemoved {
            entity,
            chunk: ORIGIN,
        };
        let seen = received(&mut watcher);
        let [WorkerToEdge::TickDelta { events, .. }] = &seen[..] else {
            panic!("expected a delta, got {seen:?}");
        };
        assert_eq!(*events, [removed]);
        assert!(closed(&mut old).await);

        // The later start numbers from 1.
        new.send(join(player(), "Notch")).await.unwrap();
        step(&mut runner);
        assert_eq!(runner.region().player_count(), 1);
        assert_eq!(runner.region().edge(id).unwrap().applied, 1);
    }

    /// An edge the region has forgotten sends what it kept under numbers that mean
    /// nothing to the region any more, before it has read that it is unknown, and
    /// numbers from 1 again once it has. The link outlasts that, or the edge would
    /// never read the answer.
    #[tokio::test]
    async fn an_edge_the_region_has_forgotten_is_unknown_and_starts_its_numbers_again() {
        let (mut edge, worker_end) = in_process(256);
        let mut runner = runner(worker_end);
        assert_eq!(edge.sent.load(Ordering::Relaxed), 0);
        // As if it had sent thirty messages to a region that no longer knows of them.
        edge.send_as(31, join(player(), "Notch")).await;
        edge.send_as(32, walk(player(), 3.0)).await;
        step(&mut runner);
        assert_eq!(runner.links.len(), 1);
        assert_eq!(runner.region().player_count(), 0);
        assert_eq!(
            edge.everything(),
            [
                UNKNOWN,
                WorkerToEdge::Progress {
                    applied: 0,
                    inputs: vec![],
                },
            ]
        );

        edge.send_as(1, join(other_player(), "Jeb")).await;
        step(&mut runner);
        assert_eq!(runner.region().player_count(), 1);
        assert_eq!(runner.region().edge(edge.edge).unwrap().applied, 1);
        // From here on its numbers have to follow each other again.
        edge.send_as(3, walk(other_player(), 3.0)).await;
        step(&mut runner);
        assert!(runner.links.is_empty());
    }

    /// A resume holds the line: what an edge sends behind a hello that names chunks
    /// acts on those chunks, so it waits until they are loaded and shown.
    #[tokio::test]
    async fn what_follows_a_hello_with_chunks_waits_for_their_snapshots() {
        let (edge_end, worker_end) = link::in_process(256);
        let mut edge = TestEdge::silent(edge_end, EdgeId::from_name("holding"), 1);
        let mut runner = runner(worker_end);
        edge.send(edge.hello(0, &[], &[ORIGIN])).await.unwrap();
        edge.send(join(player(), "Notch")).await.unwrap();
        edge.send(dig(1)).await.unwrap();
        // An ordinary subscription holds nothing, but it waits its turn behind the rest.
        edge.send(asking_for(vec![ChunkPos::new(0, 1)]))
            .await
            .unwrap();

        // The chunk has been asked for by the first tick and cannot be there before the
        // second.
        step(&mut runner);
        assert_eq!(runner.region().loaded_chunk_count(), 0);
        assert_eq!(runner.links[&LinkId(0)].held.len(), 3);
        let mut loaded_without_the_player = false;
        step_until(&mut runner, |runner| {
            let loaded = runner.region().loaded_chunk_count() >= 1;
            loaded_without_the_player |= loaded && runner.region().player_count() == 0;
            runner.region().player_count() == 1
        });
        assert!(loaded_without_the_player);
        // The player dug into a chunk that was there.
        let chunk = runner.region().chunk(ORIGIN).unwrap();
        assert_eq!(chunk.get(1, -61, 0), Some(clustine_data::blocks::AIR));
        assert_eq!(runner.region().edge(edge.edge).unwrap().applied, 2);
        assert!(runner.links[&LinkId(0)].held.is_empty());

        // On the wire, the snapshot comes before anything of the tick that took what
        // was held.
        let told = edge.everything();
        let place = |wanted: fn(&WorkerToEdge) -> bool| told.iter().position(wanted).unwrap();
        let shown = place(|message| matches!(message, WorkerToEdge::ChunkSnapshot { .. }));
        let entered = place(|message| matches!(message, WorkerToEdge::ToPlayer { .. }));
        assert!(shown < entered, "{told:?}");
        assert_eq!(told[0], UNKNOWN);
    }

    /// What a link sent while it was held does not count as received. If the link ends
    /// before the hold does, the edge sends it again on its next link, where it is
    /// applied once.
    #[tokio::test]
    async fn what_was_held_on_a_link_that_ended_is_sent_again_and_applied_once() {
        let id = EdgeId::from_name("held and lost");
        let (edge_end, worker_end) = link::in_process(256);
        let first = TestEdge::silent(edge_end, id, 1);
        let mut runner = runner(worker_end);
        first.send(first.hello(0, &[], &[ORIGIN])).await.unwrap();
        first.send(join(player(), "Notch")).await.unwrap();
        // The chunk has been asked for by the first tick and cannot be there before the
        // second, so the join is held when the link ends.
        step(&mut runner);
        assert_eq!(runner.links[&LinkId(0)].held.len(), 1);
        let (edge_end, worker_end) = link::in_process(256);
        let second = first.again(edge_end, &runner);
        drop(first);
        step(&mut runner);
        assert!(runner.links.is_empty());
        assert_eq!(runner.edges[&id].received, 0);

        runner.links().attach(worker_end);
        second.send(second.hello(0, &[], &[ORIGIN])).await.unwrap();
        second.send_as(1, join(player(), "Notch")).await;
        second.send_as(2, walk(player(), 3.0)).await;
        step_until(&mut runner, |runner| runner.region().player_count() == 1);
        step(&mut runner);
        assert_eq!(x_of(&runner, player()), Some(3.0));
        let state = runner.region().state();
        assert_eq!(state.edges[&id].applied, 2);
        // One player has entered the world, once.
        assert_eq!(state.next_entity_id.0, state.entity_ids.first.0 + 1);
    }

    /// Within what a tick has to say to an edge, the order is fixed: what happened,
    /// who entered the world, what the outbox got that is not a departure, what was
    /// acknowledged, who was let go, the chunks, and how far the region has got. The
    /// numbers of outbox entries ascend.
    #[tokio::test]
    async fn what_a_tick_tells_an_edge_is_in_a_fixed_order() {
        let (mut edge, edge_end) = in_process(256);
        let (other, other_end) = in_process(256);
        let mut runner = west(edge_end);
        runner.links().attach(other_end);
        look_east(&mut runner, &mut edge);
        let beside = ChunkPos::new(0, 1);
        edge.send(join(player(), "Notch")).await.unwrap();
        edge.send(join(other_player(), "Jeb")).await.unwrap();
        edge.send(walk(player(), 14.5)).await.unwrap();
        edge.send(asking_for(vec![ORIGIN])).await.unwrap();
        // Another edge has the chunk beside loaded, so that this one is shown it in the
        // very tick it asks for it.
        other.send(asking_for(vec![beside])).await.unwrap();
        step_until(&mut runner, |runner| {
            runner.region().loaded_chunk_count() == 2
        });
        step(&mut runner);
        edge.everything();

        // All in one tick: a third player joins; the first breaks a block beyond the
        // region; the second breaks one of the region's and steps out of it.
        edge.send(join(third_player(), "Dinnerbone")).await.unwrap();
        edge.send(dig_by(player(), 16, 5)).await.unwrap();
        edge.send(dig_by(other_player(), 1, 6)).await.unwrap();
        edge.send(walk(other_player(), 20.0)).await.unwrap();
        edge.send(asking_for(vec![beside])).await.unwrap();
        step(&mut runner);

        let told = edge.everything();
        let [
            WorkerToEdge::TickDelta { .. },
            WorkerToEdge::ToPlayer {
                player: entered,
                event: PlayerEvent::Spawned { .. },
            },
            WorkerToEdge::Outbox {
                number: 1,
                entry: Durable::Remote { .. },
            },
            WorkerToEdge::ToPlayer {
                player: acknowledged,
                event: PlayerEvent::Acknowledged { sequence: 6 },
            },
            WorkerToEdge::Outbox {
                number: 2,
                entry: Durable::Departed { player: gone, .. },
            },
            WorkerToEdge::ChunkSnapshot { position, .. },
            WorkerToEdge::Progress { applied: 7, inputs },
        ] = &told[..]
        else {
            panic!("unexpected {told:?}");
        };
        assert_eq!(
            (*entered, *acknowledged, *gone),
            (third_player(), other_player(), other_player())
        );
        assert_eq!(*position, beside);
        // The player who was let go is no longer the region's to report on.
        assert!(matches!(inputs[..], [(of, _)] if of == player()));
    }

    /// An edge hears how far the region has got with what it sent, once that is on
    /// disk, and nothing while nothing of its own changes.
    #[tokio::test]
    async fn progress_follows_what_an_edge_sent_and_nothing_else() {
        let (mut edge, edge_end) = in_process(256);
        let (mut other, other_end) = in_process(256);
        let mut runner = runner(edge_end);
        runner.links().attach(other_end);
        let progress = |applied, inputs| WorkerToEdge::Progress { applied, inputs };
        let unknown = UNKNOWN;
        step(&mut runner);
        // An edge that has said hello hears where the region is, even if nowhere.
        assert_eq!(edge.everything(), [unknown.clone(), progress(0, vec![])]);
        assert_eq!(other.everything(), [unknown, progress(0, vec![])]);

        edge.send(join(player(), "Notch")).await.unwrap();
        step(&mut runner);
        let told = edge.everything();
        assert_eq!(told.last(), Some(&progress(1, vec![])));

        let aside = walk(player(), 3.0);
        let EdgeToWorker::Input { number, .. } = aside else {
            unreachable!("a step is an input");
        };
        edge.send(aside).await.unwrap();
        step(&mut runner);
        assert_eq!(edge.everything(), [progress(2, vec![(player(), number)])]);

        // An input the region has already is dropped by the region, but the message that
        // carried it counts.
        edge.send(walk_as(player(), number, 9.0)).await.unwrap();
        step(&mut runner);
        assert_eq!(x_of(&runner, player()), Some(3.0));
        assert_eq!(edge.everything(), [progress(3, vec![])]);

        step(&mut runner);
        assert_eq!(edge.everything(), []);
        assert_eq!(other.everything(), []);
    }

    /// A region whose store handle is lost stops for good: it may have been given to
    /// another owner, and what it did last may never have reached the disk.
    #[tokio::test]
    async fn a_region_whose_store_handle_is_lost_stops_and_publishes_nothing_held() {
        let store = memory();
        let (mut edge, worker_end) = in_process(256);
        let (mut runner, gate) = gated(&store, config(0));
        let (links, status) = (runner.links(), runner.status());
        links.attach(worker_end);
        step(&mut runner);
        edge.everything();

        // A tick the store has, and the edge has not heard of yet.
        gate.hold();
        edge.send(join(player(), "Notch")).await.unwrap();
        runner.step();
        wait_for_kept(&mut runner, &gate, 1);
        assert_eq!(runner.pending.len(), 1);
        assert!(!status.store_lost.load(Ordering::Relaxed));

        // Someone opens the region again, which takes it from this runner.
        let (_handle, restored) = store.open_region(owner()).unwrap();
        assert_eq!(restored.tick(), runner.region().tick_number());
        for _ in 0..20_000 {
            runner.step();
            if runner.store_is_lost() {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(runner.store_is_lost());
        assert!(status.store_lost.load(Ordering::Relaxed));
        assert!(runner.links.is_empty());

        // That the commit was confirmed after all changes nothing.
        gate.release();
        let ticks = runner.region().tick_number();
        runner.step();
        assert_eq!(runner.region().tick_number(), ticks);
        assert_eq!(runner.run(&AtomicBool::new(false)), Ended::StoreLost);
        assert_eq!(edge.everything(), []);
        assert!(closed(&mut edge).await);

        // A link that comes now is closed too.
        let (mut late, late_end) = in_process(256);
        links.attach(late_end);
        runner.step();
        assert!(closed(&mut late).await);
    }

    /// What the store has of a region is what the region wrote. If it cannot be read,
    /// the region is not started from something else instead.
    #[test]
    fn a_region_whose_stored_state_cannot_be_read_is_not_restored() {
        let restored = |state, deltas| Restored {
            held: Vec::new(),
            pinned: Vec::new(),
            entity_ids: EntityIds::block(0).unwrap(),
            state,
            deltas,
        };
        // Of this build by its first two bytes, and nothing postcard can read behind.
        let unreadable = |tick| clustine_rpc::TickState {
            tick,
            state: vec![0, STATE_FORMAT, 0xff, 0xff, 0xff],
        };
        assert!(matches!(
            restored_state(restored(Some(unreadable(4)), vec![])),
            Err(RestoreError::State { tick: 4, .. })
        ));
        assert!(matches!(
            restored_state(restored(None, vec![unreadable(7)])),
            Err(RestoreError::Delta { tick: 7, .. })
        ));
        let state = restored_state(restored(None, vec![])).unwrap();
        assert_eq!(state, RegionState::new(EntityIds::block(0).unwrap()));
    }

    /// An edge and a region share a numbering only since the welcome that the edge has
    /// read. An edge that says hello again without having read it is told the same
    /// once more, and one that says what it was told is resumed.
    #[tokio::test]
    async fn an_edge_that_never_read_its_welcome_is_told_again_since_when_it_is_known() {
        let (mut edge, worker_end) = in_process(256);
        let mut runner = runner(worker_end);
        step(&mut runner);
        edge.everything();
        let Some(Welcome::Unknown {
            since,
            entries: 0,
            presences: 0,
            applied: 0,
        }) = edge.welcomed
        else {
            panic!("{:?}", edge.welcomed);
        };
        assert_eq!(since, runner.region().tick_number());
        assert_eq!(runner.region().edge(edge.edge).unwrap().since, since);

        // Another link of the same start that knows of no welcome.
        let (edge_end, worker_end) = link::in_process(256);
        let mut unread = TestEdge::silent(edge_end, edge.edge, edge.start);
        runner.links().attach(worker_end);
        unread.send(unread.hello(0, &[], &[])).await.unwrap();
        step(&mut runner);
        step(&mut runner);
        unread.everything();
        let again = Welcome::Unknown {
            since,
            entries: 0,
            presences: 0,
            applied: 0,
        };
        assert_eq!(unread.welcomed, Some(again));
        // What it sends from 1 is taken.
        unread.send(join(player(), "Notch")).await.unwrap();
        step(&mut runner);
        assert!(runner.region().player(player()).is_some());

        // One that says since when is resumed, and its player is there.
        let (edge_end, worker_end) = link::in_process(256);
        let mut read = unread.again(edge_end, &runner);
        assert_eq!(read.since, since);
        runner.links().attach(worker_end);
        read.send(read.hello(0, &[player()], &[])).await.unwrap();
        step(&mut runner);
        read.everything();
        // The one presence answer is for the player its hello named, and the one
        // message the region has applied is the join.
        let resumed = Welcome::Resumed {
            entries: 0,
            presences: 1,
            applied: 1,
        };
        assert_eq!(read.welcomed, Some(resumed));
        assert!(runner.region().player(player()).is_some());
    }

    /// An edge that says another `since` than the region has for it, after the region
    /// took messages from it, has lost track. Nothing it kept can be trusted to fit,
    /// so the region resets it as for a higher start: its players go, and it is known
    /// anew from that tick.
    #[tokio::test]
    async fn an_edge_that_lost_its_since_after_the_region_took_its_messages_is_reset() {
        let (mut edge, worker_end) = in_process(256);
        let mut runner = runner(worker_end);
        edge.send(join(player(), "Notch")).await.unwrap();
        step(&mut runner);
        edge.everything();
        let Some(Welcome::Unknown { since, .. }) = edge.welcomed else {
            panic!("{:?}", edge.welcomed);
        };
        let (entity, _) = runner.region().player(player()).unwrap();
        let before = runner.region().edge(edge.edge).unwrap().applied;

        let (edge_end, worker_end) = link::in_process(256);
        let mut lost = TestEdge::silent(edge_end, edge.edge, edge.start);
        runner.links().attach(worker_end);
        lost.send(lost.hello(0, &[player()], &[ORIGIN]))
            .await
            .unwrap();
        step(&mut runner);
        step_until(&mut runner, |runner| {
            runner.links.values().all(|link| link.hold.is_empty())
        });
        let told = lost.everything();
        // The state the reset makes has applied nothing, whatever the one before had.
        assert_eq!(before, 1);
        let Some(Welcome::Unknown {
            since: anew,
            entries: 0,
            presences: 1,
            applied: 0,
        }) = lost.welcomed
        else {
            panic!("{:?}", lost.welcomed);
        };
        assert!(anew > since, "{anew} {since}");
        assert_eq!(runner.region().edge(edge.edge).unwrap().since, anew);
        // The player is gone, and is said to be absent.
        assert!(runner.region().player(player()).is_none());
        assert!(told.contains(&WorkerToEdge::Presence {
            player: player(),
            answer: Presence::Absent,
        }));
        let removed = told.iter().any(|message| {
            matches!(message, WorkerToEdge::TickDelta { events, .. }
                if events.iter().any(|event| matches!(event,
                    RegionEvent::EntityRemoved { entity: gone, .. } if *gone == entity)))
        });
        assert!(removed, "{told:?}");
        // What the edge numbers from 1 is taken.
        lost.send(join(other_player(), "Alex")).await.unwrap();
        step(&mut runner);
        assert!(runner.region().player(other_player()).is_some());
        assert_eq!(runner.region().edge(edge.edge).unwrap().applied, 1);
    }

    /// A state and a delta that have something of everything a state is made of.
    fn a_state_and_a_delta() -> (RegionState, StateDelta) {
        let ids = EntityIds::block(0).unwrap();
        let mut region = Region::new(config(0), ids, Holdings::default());
        let edge = EdgeId(7);
        let join = PlayerChange::Join(
            edge,
            PlayerJoin {
                player: player(),
                name: "Steve".to_owned(),
            },
        );
        // The edge looks at two chunks of the neighbour, and the store says whose they
        // are before anyone does anything about them.
        let neighbours = [BESIDE, ChunkPos::new(2, 0)];
        let output = region.tick(&TickInputs {
            edges: vec![EdgeEvent::Started { edge, start: 3 }],
            tickets_added: neighbours.map(|chunk| (chunk, Ticket::Viewer)).to_vec(),
            ..TickInputs::default()
        });
        assert_eq!(output.claims, neighbours);
        let mut inputs = TickInputs {
            applied: vec![(edge, 2)],
            foreign: neighbours.map(|chunk| (chunk, EAST)).to_vec(),
            ..TickInputs::default()
        };
        inputs.change(join);
        // One who stays, so that the state has a player.
        inputs.change(PlayerChange::Join(
            edge,
            PlayerJoin {
                player: other_player(),
                name: "Alex".to_owned(),
            },
        ));
        // One who arrives for a chunk of the neighbour and is sent on to it, which is
        // an outbox entry: a `NotMine`.
        let beyond = Vec3::new(40.5, -60.0, 0.5);
        let arriving = PlayerTransfer {
            entity_id: EntityId(77),
            name: "Notch".to_owned(),
            pose: Pose::at(beyond),
            hotbar: [None; HOTBAR_SLOTS],
            selected_slot: 0,
            last_input: 5,
        };
        inputs.change(PlayerChange::Arrive(edge, third_player(), arriving));
        // A step into a chunk of the neighbour: the player is let go, which is a
        // `Departed`.
        let walk = |x| PlayerInput::Move {
            position: Some(Vec3::new(x, -60.0, 0.5)),
            rotation: None,
            on_ground: true,
        };
        // The two who join in this tick get the first two ids of the block.
        let (first, second) = (ids.first, EntityId(ids.first.0 + 1));
        inputs.input(edge, player(), first, 1, walk(beyond.x));
        // The one who stays walks up to the neighbour and breaks a block of it, which
        // is passed on: a `Remote`.
        inputs.input(edge, other_player(), second, 1, walk(14.5));
        let dig = PlayerInput::Dig {
            position: BlockPos::new(16, -61, 0),
            sequence: 9,
        };
        inputs.input(edge, other_player(), second, 2, dig);
        let output = region.tick(&inputs);
        // The three entries whose shapes say where something goes.
        let kinds = output.durable.iter().map(|(_, _, entry)| match entry {
            Durable::NotMine { .. } => "not mine",
            Durable::Remote { .. } => "remote",
            Durable::Departed { .. } => "departed",
            _ => "another",
        });
        let kinds: Vec<_> = kinds.collect();
        assert_eq!(kinds, ["not mine", "remote", "departed"]);

        // The two entries of a merge and of a split, which no tick makes yet
        // (ADR-0014, section 9): they are put behind the others by hand, into the
        // state's outbox and among what the delta adds, so that the delta still
        // turns the state before the tick into this one.
        let (mut state, mut delta) = (region.state(), output.delta);
        let absorbed = Durable::Absorbed {
            region: EAST,
            since: 4,
            applied: 6,
            numbers: vec![1, 3],
        };
        let split_off = Durable::SplitOff {
            region: RegionId(9),
            players: vec![(third_player(), EntityId(77))],
        };
        let known = state
            .edges
            .get_mut(&edge)
            .expect("the state knows the edge");
        let change = delta.edges.iter_mut().find(|(id, _)| *id == edge);
        let change = change.and_then(|(_, change)| change.as_mut());
        let change = change.expect("the edge changed in the tick");
        for entry in [absorbed, split_off] {
            known.sent += 1;
            known.outbox.insert(known.sent, entry.clone());
            change.added.push((known.sent, entry));
        }
        change.sent = known.sent;
        (state, delta)
    }

    /// What is stored of a region is read back by its first two bytes. Postcard writes
    /// neither names nor kinds, so bytes of one shape can read as another, and a build
    /// that changes the shape of a state has to say so with [`STATE_FORMAT`]. This test
    /// has the bytes of a state and of a delta written out. **If it fails, a shape has
    /// changed: raise `STATE_FORMAT` and write the new bytes down here.**
    #[test]
    fn the_bytes_of_a_state_and_of_a_delta_are_as_written_down() {
        let (state, delta) = a_state_and_a_delta();
        let hex =
            |bytes: Vec<u8>| -> String { bytes.iter().map(|byte| format!("{byte:02x}")).collect() };
        assert_eq!(STATE_FORMAT, 3);
        assert_eq!(
            hex(stored(&state)),
            concat!(
                "0003020280808001060110000000000000000000000000000000020404416c65780000000000002d",
                "400000000000004ec0000000000000e03f0000000000000000010000000000000000000002000701",
                "07030102050501040010000000000000000000000000000000039a01054e6f746368000000000040",
                "44400000000000004ec0000000000000e03f00000000000000000000000000000000000000050102",
                "02100000000000000000000000000000000212002079000101030010000000000000000000000000",
                "000000010205537465766500000000004044400000000000004ec0000000000000e03f0000000000",
                "00000001000000000000000000000101040501040602010305060901100000000000000000000000",
                "00000000039a01",
            )
        );
        assert_eq!(
            hex(stored(&delta)),
            concat!(
                "00030201060210000000000000000000000000000000010010000000000000000000000000000000",
                "02010404416c65780000000000002d400000000000004ec0000000000000e03f0000000000000000",
                "01000000000000000000000200070107010301020500000501040010000000000000000000000000",
                "000000039a01054e6f74636800000000004044400000000000004ec0000000000000e03f00000000",
                "00000000000000000000000000000005010202100000000000000000000000000000000212002079",
                "00010103001000000000000000000000000000000001020553746576650000000000404440000000",
                "0000004ec0000000000000e03f000000000000000001000000000000000000000101040501040602",
                "01030506090110000000000000000000000000000000039a01",
            )
        );
    }

    /// What an earlier build stored cannot be read. It is dropped up to the last such
    /// item: the region is as one that never ran, at that item's tick, and what this
    /// build stored behind it is applied.
    #[test]
    fn what_an_earlier_build_stored_of_a_region_is_dropped_and_the_rest_applied() {
        let ids = EntityIds::block(0).unwrap();
        let restored = |state, deltas| Restored {
            held: Vec::new(),
            pinned: Vec::new(),
            entity_ids: ids,
            state,
            deltas,
        };
        let (state, delta) = a_state_and_a_delta();
        let item = |tick, state| clustine_rpc::TickState { tick, state };
        // As builds before the number wrote them: the postcard alone, which begins
        // with the tick.
        let bare_state = postcard::to_stdvec(&state).unwrap();
        let bare_delta = postcard::to_stdvec(&delta).unwrap();
        let fresh = |tick| {
            let mut fresh = RegionState::new(ids);
            fresh.tick = tick;
            fresh
        };

        // A state from before, alone and with a delta from before behind it.
        let alone = restored(Some(item(2, bare_state.clone())), vec![]);
        assert_eq!(restored_state(alone).unwrap(), fresh(2));
        let both = restored(
            Some(item(2, bare_state.clone())),
            vec![item(3, bare_delta.clone())],
        );
        assert_eq!(restored_state(both).unwrap(), fresh(3));

        // A delta of this build behind one from before is applied to what is left.
        let later = StateDelta {
            tick: 4,
            ..StateDelta::default()
        };
        let mixed = restored(
            Some(item(2, stored(&state))),
            vec![item(3, bare_delta), item(4, stored(&later))],
        );
        assert_eq!(restored_state(mixed).unwrap(), fresh(4));

        // What this build stored is read as it is.
        let whole = restored(Some(item(2, stored(&state))), vec![]);
        assert_eq!(restored_state(whole).unwrap(), state);

        // A lower number is from before as well; a higher one is a later build's.
        let mut lower = stored(&state);
        lower[1] = STATE_FORMAT - 1;
        let lower = restored(Some(item(2, lower)), vec![]);
        assert_eq!(restored_state(lower).unwrap(), fresh(2));
        let mut higher = stored(&state);
        higher[1] = STATE_FORMAT + 1;
        let higher = restored(Some(item(2, higher)), vec![]);
        assert!(matches!(
            restored_state(higher),
            Err(RestoreError::Format { tick: 2, format }) if format == STATE_FORMAT + 1
        ));
    }

    /// Opens the one region of `store` as the owner that comes after the one of
    /// [`owner`], as the worker does that a released region is given to.
    fn opened_next(store: &Store) -> (StoreHandle, Restored) {
        let hello = RegionHello {
            epoch: 2,
            ..owner()
        };
        store.open_region(hello).unwrap()
    }

    /// Steps a runner that is releasing its region until it has ended, and says how.
    fn released(runner: &mut RegionRunner) -> Ended {
        for _ in 0..20_000 {
            runner.step();
            if let Some(ended) = runner.ended() {
                return ended;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("the release did not end");
    }

    /// Steps a runner that is releasing its region until it has stopped ticking.
    fn step_until_it_ticks_no_more(runner: &mut RegionRunner) {
        for _ in 0..20_000 {
            runner.step();
            if runner.phase == Phase::Settling {
                return;
            }
            assert_eq!(runner.phase, Phase::Preparing);
            thread::sleep(Duration::from_millis(1));
        }
        panic!("the store did not answer the flush behind the first checkpoint");
    }

    /// Waits until the store has answered a flush that the gate holds back.
    fn wait_for_kept_flush(runner: &mut RegionRunner, gate: &GateControl) {
        for _ in 0..20_000 {
            assert!(runner.take_replies());
            if gate.kept_flushes() >= 1 {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("the store did not answer the flush");
    }

    /// The x coordinates of the moves among `messages`, in the order they were told.
    fn moves(messages: &[WorkerToEdge]) -> Vec<f64> {
        let mut moves = Vec::new();
        for message in messages {
            if let WorkerToEdge::TickDelta { events, .. } = message {
                for event in events {
                    if let RegionEvent::EntityMoved { pose, .. } = event {
                        moves.push(pose.position.x);
                    }
                }
            }
        }
        moves
    }

    /// The ticks that `messages` tell of, in the order they were told.
    fn ticks(messages: &[WorkerToEdge]) -> Vec<u64> {
        let tick = |message: &WorkerToEdge| match message {
            WorkerToEdge::TickDelta { tick, .. } => Some(*tick),
            _ => None,
        };
        messages.iter().filter_map(tick).collect()
    }

    /// A released region is in the store as it was after its last tick: the next owner
    /// gets a state file and no commits to apply to it, and every chunk as it was.
    #[tokio::test]
    async fn a_released_region_is_restored_from_a_state_file_alone() {
        for on_disk_too in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let world = match on_disk_too {
                true => Divided::stripes_in(directory.path()),
                false => Divided::stripes(),
            };
            let (mut edge, worker_end) = in_process(256);
            let mut first = world.runner(RegionId(0), 1);
            let status = first.status();
            first.links().attach(worker_end);
            look_east(&mut first, &mut edge);
            joined(&edge, &mut first).await;
            edge.send(join(other_player(), "Jeb")).await.unwrap();
            edge.send(dig(1)).await.unwrap();
            edge.send(walk(player(), 14.5)).await.unwrap();
            step(&mut first);
            // A block beyond the region, which leaves an entry in the outbox.
            edge.send(dig(16)).await.unwrap();
            step(&mut first);
            assert_eq!(status.ended(), None);

            // Something more happens while the release is under way, and whether the
            // region still takes it or not, the store ends up with the region as it is.
            first.begin_release();
            edge.send(dig(2)).await.unwrap();
            assert_eq!(released(&mut first), Ended::Released);
            assert_eq!(status.ended(), Some(Ended::Released));
            assert!(!status.store_lost.load(Ordering::Relaxed));
            assert!(!first.store_is_lost());
            assert!(first.links.is_empty());
            let state = first.region().state();
            let chunk = first.region().chunk(ORIGIN).unwrap().clone();
            assert_eq!(chunk.get(1, -61, 0), Some(clustine_data::blocks::AIR));
            assert_eq!(state.players.len(), 2);
            assert_eq!(state.edges[&edge.edge].outbox.len(), 1);

            // The edge was told of every tick up to the last before its link closed.
            let told = edge.everything();
            let applied = told.iter().rev().find_map(|message| match message {
                WorkerToEdge::Progress { applied, .. } => Some(*applied),
                _ => None,
            });
            assert_eq!(applied, Some(state.edges[&edge.edge].applied));
            assert!(closed(&mut edge).await);

            // The old runner is still there, which a restore must not depend on.
            let (handle, restored) = world.open(RegionId(0), 2);
            assert_eq!(restored.deltas, []);
            assert_eq!(
                restored.state.as_ref().map(|stored| stored.tick),
                Some(state.tick)
            );
            let mut second = RegionRunner::restore(config(0), handle, restored).unwrap();
            assert_eq!(second.region().state(), state);
            let (again, worker_end) = in_process(256);
            second.links().attach(worker_end);
            again.send(asking_for(vec![ORIGIN])).await.unwrap();
            step_until(&mut second, |runner| {
                runner.region().loaded_chunk_count() == 1
            });
            assert_eq!(second.region().chunk(ORIGIN), Some(&chunk));

            // Released for good: no tick, whatever is asked of the runner.
            first.begin_release();
            first.step();
            assert_eq!(first.region().state(), state);
            assert_eq!(first.run(&AtomicBool::new(false)), Ended::Released);
        }
    }

    /// What a release publishes last is followed at once by the link being closed. An
    /// edge gets all of it all the same, also over a link that serialises, where a
    /// message is on its way for a while.
    #[tokio::test(flavor = "multi_thread")]
    async fn what_a_release_publishes_reaches_the_edge_before_its_link_is_closed() {
        for connect in KINDS {
            let store = memory();
            let (mut edge, worker_end) = connect(256);
            let (mut runner, gate) = gated(&store, config(0));
            runner.links().attach(worker_end);
            joined(&edge, &mut runner).await;

            // The step is taken in a tick whose commit is confirmed only once the region
            // has stopped ticking, so that it is the release that publishes it.
            gate.hold();
            edge.send(walk(player(), 3.0)).await.unwrap();
            for _ in 0..20_000 {
                runner.step();
                if x_of(&runner, player()) == Some(3.0) {
                    break;
                }
                thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(x_of(&runner, player()), Some(3.0));
            runner.begin_release();
            step_until_it_ticks_no_more(&mut runner);
            gate.release();
            assert_eq!(released(&mut runner), Ended::Released);

            let mut told = Vec::new();
            let everything = async {
                while let Some(message) = edge.end.recv().await {
                    told.push(message);
                }
            };
            timeout(Duration::from_secs(10), everything)
                .await
                .expect("the link was not closed");
            assert_eq!(moves(&told).last(), Some(&3.0));
            let applied = runner.region().edge(edge.edge).unwrap().applied;
            assert!(
                matches!(told.last(), Some(WorkerToEdge::Progress { applied: told, .. }) if *told == applied),
                "{told:?}"
            );
        }
    }

    /// The first checkpoint of a release can be minutes of changed chunks, and players
    /// do not stand still for it: the region ticks on and serves its links until the
    /// store has it. From then on it takes nothing more, and what an edge sends is the
    /// next owner's to apply when the edge sends it again.
    #[tokio::test]
    async fn a_region_ticks_on_during_the_first_checkpoint_of_a_release_and_not_after() {
        let store = memory();
        let (mut edge, worker_end) = in_process(256);
        let (mut runner, gate) = gated(&store, config(0));
        runner.links().attach(worker_end);
        joined(&edge, &mut runner).await;
        step(&mut runner);
        edge.everything();

        gate.hold_flushes();
        runner.begin_release();
        assert_eq!(runner.ended(), None);
        // The store has done all of it, which the runner does not get to know.
        wait_for_kept_flush(&mut runner, &gate);
        for x in [3.0, 4.0] {
            let before = runner.region().tick_number();
            edge.send(walk(player(), x)).await.unwrap();
            step(&mut runner);
            assert_eq!(runner.region().tick_number(), before + 1);
            assert_eq!(x_of(&runner, player()), Some(x));
            let told = edge.everything();
            assert_eq!(moves(&told), [x]);
            let applied = edge.sent.load(Ordering::Relaxed);
            assert!(
                matches!(told.last(), Some(WorkerToEdge::Progress { applied: told, .. }) if *told == applied),
                "{told:?}"
            );
        }
        // A link that comes meanwhile is served as well.
        let (mut other, other_end) = in_process(256);
        runner.links().attach(other_end);
        step(&mut runner);
        assert_eq!(other.everything()[0], UNKNOWN);
        assert_eq!(runner.phase, Phase::Preparing);

        // The flush is answered, and the step that finds it so takes nothing from a
        // link any more.
        let late = walk(player(), 5.0);
        edge.send(late.clone()).await.unwrap();
        let number = edge.sent.load(Ordering::Relaxed);
        gate.release_flushes();
        let ticks = runner.region().tick_number();
        assert_eq!(released(&mut runner), Ended::Released);
        assert_eq!(runner.region().tick_number(), ticks);
        assert_eq!(x_of(&runner, player()), Some(4.0));
        assert_eq!(runner.region().edge(edge.edge).unwrap().applied, number - 1);
        assert_eq!(edge.everything(), []);
        assert!(closed(&mut edge).await);
        assert!(closed(&mut other).await);
        let state = runner.region().state();

        // The edge has kept what it was not told to be applied, and sends it again
        // under the number it had.
        let (handle, restored) = opened_next(&store);
        assert_eq!(restored.deltas, []);
        let mut next = RegionRunner::restore(config(0), handle, restored).unwrap();
        assert_eq!(next.region().state(), state);
        let (edge_end, worker_end) = link::in_process(256);
        let mut again = edge.again(edge_end, &runner);
        next.links().attach(worker_end);
        again
            .send(again.hello(0, &[player()], &[ORIGIN]))
            .await
            .unwrap();
        again.send_as(number, late).await;
        step_until(&mut next, |runner| x_of(runner, player()) == Some(5.0));
        step(&mut next);
        assert_eq!(next.region().edge(edge.edge).unwrap().applied, number);
        assert_eq!(again.everything()[0], RESUMED);
    }

    /// Ticks that ran before the region stopped ticking are owed to the edges once the
    /// store has them, and not before. A release waits for that, and closes the links
    /// only after it has published them, oldest first.
    #[tokio::test]
    async fn a_release_publishes_the_ticks_that_ran_once_they_are_confirmed_and_in_order() {
        let store = memory();
        let (mut edge, worker_end) = in_process(256);
        let (mut runner, gate) = gated(&store, config(0));
        runner.links().attach(worker_end);
        joined(&edge, &mut runner).await;
        step(&mut runner);
        edge.everything();

        // Two ticks before the release begins and one during its first checkpoint, none
        // of which the runner hears to be on disk.
        gate.hold();
        gate.hold_flushes();
        let first = runner.region().tick_number() + 1;
        for x in [3.0, 4.0] {
            edge.send(walk(player(), x)).await.unwrap();
            runner.step();
        }
        runner.begin_release();
        edge.send(walk(player(), 5.0)).await.unwrap();
        runner.step();
        assert_eq!(runner.region().tick_number(), first + 2);
        assert_eq!(gate.commits().last(), Some(&(first + 2)));
        gate.release_flushes();
        step_until_it_ticks_no_more(&mut runner);
        let last = runner.region().tick_number();

        // The store has all three, and for as long as the runner is not told, nobody
        // else is, and the release goes no further.
        wait_for_kept(&mut runner, &gate, 3);
        for _ in 0..20 {
            runner.step();
        }
        assert_eq!(runner.phase, Phase::Settling);
        assert_eq!(runner.ended(), None);
        assert_eq!(runner.region().tick_number(), last);
        assert_eq!(runner.links.len(), 1);
        assert_eq!(edge.everything(), []);

        gate.release();
        assert_eq!(released(&mut runner), Ended::Released);
        let told = edge.everything();
        assert_eq!(moves(&told), [3.0, 4.0, 5.0]);
        assert_eq!(ticks(&told), [first, first + 1, first + 2]);
        let number = edge.sent.load(Ordering::Relaxed);
        let applied: Vec<_> = told
            .iter()
            .filter_map(|message| match message {
                WorkerToEdge::Progress { applied, .. } => Some(*applied),
                _ => None,
            })
            .collect();
        assert_eq!(applied, [number - 2, number - 1, number]);
        assert!(closed(&mut edge).await);

        let (_handle, restored) = opened_next(&store);
        assert_eq!(restored.deltas, []);
        let state = restored_state(restored).unwrap();
        assert_eq!(state, runner.region().state());
        assert_eq!(state.players[&player()].pose.position.x, 5.0);
    }

    /// A release whose store handle is lost has nothing left to do that it could do:
    /// the store has what it confirmed, and the rest was never shown. It shows nothing
    /// more, closes its links and ends, whenever that happens.
    #[tokio::test]
    async fn a_release_whose_store_handle_is_lost_ends_as_lost_and_publishes_nothing_more() {
        for case in [
            "before it begins",
            "during the first checkpoint",
            "while it waits for commits",
            "during the last checkpoint",
        ] {
            let store = memory();
            let (mut edge, worker_end) = in_process(256);
            let (mut runner, gate) = gated(&store, config(0));
            let (links, status) = (runner.links(), runner.status());
            links.attach(worker_end);
            joined(&edge, &mut runner).await;
            step(&mut runner);
            edge.everything();

            // A tick the store has, and the edge has not heard of.
            gate.hold();
            edge.send(walk(player(), 3.0)).await.unwrap();
            runner.step();
            wait_for_kept(&mut runner, &gate, 1);
            match case {
                "before it begins" => {
                    gate.lose();
                    runner.begin_release();
                    // Known at once, without a step.
                    assert_eq!(runner.ended(), Some(Ended::StoreLost));
                }
                "during the first checkpoint" => {
                    gate.hold_flushes();
                    runner.begin_release();
                    runner.step();
                    assert_eq!(runner.phase, Phase::Preparing);
                    gate.lose();
                }
                "while it waits for commits" => {
                    runner.begin_release();
                    step_until_it_ticks_no_more(&mut runner);
                    runner.step();
                    assert_eq!(runner.phase, Phase::Settling);
                    gate.lose();
                }
                _ => {
                    // The tick is confirmed and published; only the last flush is not
                    // answered.
                    runner.begin_release();
                    step_until_it_ticks_no_more(&mut runner);
                    gate.hold_flushes();
                    gate.release();
                    for _ in 0..20_000 {
                        runner.step();
                        if runner.phase == Phase::Closing {
                            break;
                        }
                        thread::sleep(Duration::from_millis(1));
                    }
                    assert_eq!(runner.phase, Phase::Closing);
                    wait_for_kept_flush(&mut runner, &gate);
                    assert_eq!(moves(&edge.everything()), [3.0], "{case}");
                    gate.lose();
                }
            }
            assert_eq!(released(&mut runner), Ended::StoreLost, "{case}");
            assert_eq!(runner.ended(), Some(Ended::StoreLost), "{case}");
            assert_eq!(status.ended(), Some(Ended::StoreLost), "{case}");
            assert!(status.store_lost.load(Ordering::Relaxed), "{case}");
            assert!(runner.store_is_lost(), "{case}");
            assert!(runner.links.is_empty(), "{case}");

            // That the store answers after all changes nothing, and the runner returns
            // by itself rather than wait for anything.
            gate.release();
            gate.release_flushes();
            let ticks = runner.region().tick_number();
            runner.step();
            assert_eq!(runner.run(&AtomicBool::new(false)), Ended::StoreLost);
            assert_eq!(runner.region().tick_number(), ticks, "{case}");
            assert_eq!(edge.everything(), [], "{case}");
            assert!(closed(&mut edge).await, "{case}");

            let (mut late, late_end) = in_process(256);
            links.attach(late_end);
            runner.step();
            assert_eq!(late.everything(), [], "{case}");
            assert!(closed(&mut late).await, "{case}");
        }
    }

    /// Once a region has stopped ticking it takes no new links: there is nothing it
    /// could tell them. Their edges find them closed and look for the region again.
    #[tokio::test]
    async fn a_link_attached_once_a_released_region_ticks_no_more_is_closed_and_not_served() {
        let store = memory();
        let (mut edge, worker_end) = in_process(256);
        let (mut runner, gate) = gated(&store, config(0));
        let links = runner.links();
        links.attach(worker_end);
        joined(&edge, &mut runner).await;
        step(&mut runner);
        edge.everything();

        // The release waits for a commit, so that it lasts.
        gate.hold();
        edge.send(walk(player(), 3.0)).await.unwrap();
        runner.step();
        runner.begin_release();
        step_until_it_ticks_no_more(&mut runner);

        let (mut during, during_end) = in_process(256);
        links.attach(during_end);
        during.send(asking_for(vec![ORIGIN])).await.unwrap();
        runner.step();
        assert_eq!(runner.phase, Phase::Settling);
        assert_eq!(during.everything(), []);
        assert!(closed(&mut during).await);
        // The link it had stays for what it is still owed.
        assert_eq!(runner.links.len(), 1);
        assert!(runner.region().edge(during.edge).is_none());

        gate.release();
        assert_eq!(released(&mut runner), Ended::Released);
        assert_eq!(moves(&edge.everything()), [3.0]);
        assert!(closed(&mut edge).await);

        let (mut after, after_end) = in_process(256);
        links.attach(after_end);
        runner.step();
        assert_eq!(after.everything(), []);
        assert!(closed(&mut after).await);
    }

    /// The checkpoints of the interval go on while a release is under way, also in the
    /// very tick the release begins after. The store ends up with the region as it is
    /// all the same.
    #[tokio::test]
    async fn checkpoints_of_the_interval_that_fall_into_a_release_do_no_harm() {
        for held in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let store = on_disk(directory.path());
            let (edge, worker_end) = in_process(256);
            let (runner, gate) = gated(&store, config(0));
            // A checkpoint with every tick.
            let mut runner = runner.with_checkpoint_interval(1);
            runner.links().attach(worker_end);
            joined(&edge, &mut runner).await;
            edge.send(dig(1)).await.unwrap();
            step(&mut runner);

            if held {
                gate.hold_flushes();
            }
            runner.begin_release();
            // Held, the region takes all of these; otherwise as many as it gets to.
            for x in 2..6 {
                edge.send(dig(x)).await.unwrap();
                step(&mut runner);
            }
            if held {
                assert_eq!(runner.phase, Phase::Preparing);
                let chunk = runner.region().chunk(ORIGIN).unwrap();
                assert_eq!(chunk.get(5, -61, 0), Some(clustine_data::blocks::AIR));
                gate.release_flushes();
            }
            assert_eq!(released(&mut runner), Ended::Released);
            let state = runner.region().state();
            let chunk = runner.region().chunk(ORIGIN).unwrap().clone();

            let (handle, restored) = opened_next(&store);
            assert_eq!(restored.deltas, []);
            let config = config(0);
            let mut next = RegionRunner::restore(config, handle, restored).unwrap();
            assert_eq!(next.region().state(), state);
            let (again, worker_end) = in_process(256);
            next.links().attach(worker_end);
            again.send(asking_for(vec![ORIGIN])).await.unwrap();
            step_until(&mut next, |runner| {
                runner.region().loaded_chunk_count() == 1
            });
            assert_eq!(next.region().chunk(ORIGIN), Some(&chunk));
        }
    }

    /// Waits until the region thread of `worker` has ended by itself.
    fn wait_until_finished(worker: &Worker) {
        for _ in 0..20_000 {
            if worker.is_finished() {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("the region thread did not end");
    }

    /// A region on a thread of its own is released by asking its worker, which says how
    /// that went once the thread is gone.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_worker_releases_its_region_and_its_thread_ends() {
        // Waiting for the release, and asking for it and looking later.
        for blocking in [true, false] {
            let store = memory();
            let runner = opened(&store, config(0));
            let (links, status) = (runner.links(), runner.status());
            let worker = Worker::spawn(runner);
            let (mut edge, worker_end) = in_process(256);
            links.attach(worker_end);
            edge.send(join(player(), "Notch")).await.unwrap();
            edge.send(asking_for(vec![ORIGIN])).await.unwrap();
            // A player acts on a chunk they have been shown.
            while !matches!(next(&mut edge).await, WorkerToEdge::ChunkSnapshot { .. }) {}
            edge.send(dig_by(player(), 1, 9)).await.unwrap();
            let acknowledged = PlayerEvent::Acknowledged { sequence: 9 };
            loop {
                match next(&mut edge).await {
                    WorkerToEdge::ToPlayer { event, .. } if event == acknowledged => break,
                    _ => {}
                }
            }
            assert_eq!(status.ended(), None);
            assert!(!worker.is_finished());

            let ended = if blocking {
                worker.release()
            } else {
                worker.begin_release();
                wait_until_finished(&worker);
                assert_eq!(status.ended(), Some(Ended::Released));
                // There is nothing left to stop, and stopping says how it ended.
                worker.stop()
            };
            assert_eq!(ended, Ended::Released);
            assert_eq!(status.ended(), Some(Ended::Released));
            assert!(!status.store_lost.load(Ordering::Relaxed));
            assert!(closed(&mut edge).await);
            let (mut late, late_end) = in_process(256);
            links.attach(late_end);
            assert!(closed(&mut late).await);

            let (handle, restored) = opened_next(&store);
            assert_eq!(restored.deltas, []);
            assert!(restored.state.is_some());
            let config = config(0);
            let mut next = RegionRunner::restore(config, handle, restored).unwrap();
            assert_eq!(next.region().player_count(), 1);
            assert_eq!(
                next.region().tick_number(),
                status.tick.load(Ordering::Relaxed)
            );
            let (again, worker_end) = in_process(256);
            next.links().attach(worker_end);
            again.send(asking_for(vec![ORIGIN])).await.unwrap();
            step_until(&mut next, |runner| {
                runner.region().loaded_chunk_count() == 1
            });
            let chunk = next.region().chunk(ORIGIN).unwrap();
            assert_eq!(chunk.get(1, -61, 0), Some(clustine_data::blocks::AIR));
        }
    }

    /// A release waits for the store for as long as the store neither answers nor
    /// closes. Whoever cannot wait any longer stops the worker, which lets go of the
    /// region at once and as it is; that is a crash, which the next owner recovers
    /// from. A worker that is not releasing anything stops as it always has.
    #[tokio::test(flavor = "multi_thread")]
    async fn stopping_a_worker_ends_a_release_that_waits_for_the_store() {
        let store = memory();
        let (runner, gate) = gated(&store, config(0));
        let (links, status) = (runner.links(), runner.status());
        let worker = Worker::spawn(runner);
        let (mut edge, worker_end) = in_process(256);
        links.attach(worker_end);
        edge.send(join(player(), "Notch")).await.unwrap();
        while !matches!(next(&mut edge).await, WorkerToEdge::ToPlayer { .. }) {}

        // The answer to the first flush never comes.
        gate.hold_flushes();
        worker.begin_release();
        for _ in 0..20_000 {
            if gate.flushes() >= 1 {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(gate.flushes(), 1);
        assert_eq!(status.ended(), None);
        assert!(!worker.is_finished());

        assert_eq!(worker.stop(), Ended::Abandoned);
        assert_eq!(status.ended(), Some(Ended::Abandoned));
        assert!(!status.store_lost.load(Ordering::Relaxed));
        assert!(closed(&mut edge).await);
        // Nothing more was asked of the store.
        assert_eq!(gate.flushes(), 1);

        // The player was shown to be in the world, so the next owner has them.
        let (_handle, restored) = opened_next(&store);
        let state = restored_state(restored).unwrap();
        assert!(state.players.contains_key(&player()));

        let runner = opened(&memory(), config(0));
        let status = runner.status();
        let worker = Worker::spawn(runner);
        assert_eq!(worker.stop(), Ended::Stopped);
        assert_eq!(status.ended(), Some(Ended::Stopped));
    }

    // -----------------------------------------------------------------------------------
    // A region on the chunks the world store grants it: section 4 of
    // `docs/adr/0012-the-tick-on-chunks.md`.
    // -----------------------------------------------------------------------------------

    /// The chunk east of the one players enter the world in: the first of the eastern
    /// stripe in [`Divided::stripes`], and nobody's in [`Divided::gap`].
    const BESIDE: ChunkPos = ChunkPos::new(1, 0);

    /// The home region of [`Divided::gap`].
    const HOME: RegionId = RegionId(2);

    /// A world that is divided, in memory unless said otherwise, of which a test runs
    /// one region and plays another through a handle of its own.
    struct Divided {
        store: Store,
        division: Division,
    }

    impl Divided {
        fn new(division: Division) -> Self {
            let generator = Arc::new(FlatGenerator::classic());
            let store = Store::memory_divided(generator, division.clone()).unwrap();
            Self { store, division }
        }

        /// The division of [`Divided::stripes`].
        fn line_at_one() -> Division {
            Division::stripes(ORIGIN, &Layout::new(vec![1]).unwrap())
        }

        /// Two stripes with the line at x = 1: region 0 west of it, which has the chunk
        /// players enter the world in, and [`EAST`].
        fn stripes() -> Self {
            Self::new(Self::line_at_one())
        }

        /// The world of [`Divided::stripes`] as it is kept in `directory`.
        fn stripes_in(directory: &std::path::Path) -> Self {
            let generator = Arc::new(FlatGenerator::classic());
            let division = Self::line_at_one();
            let store = Store::local_divided(directory, generator, division.clone()).unwrap();
            Self { store, division }
        }

        /// The division with a gap of ADR-0011: region 0 is pinned to the chunks west of
        /// x = 0 and [`EAST`] to those from x = 16 on. [`HOME`] is pinned to nothing
        /// and holds the chunk at the origin, where players enter the world; the other
        /// chunks in between are nobody's.
        fn gap() -> Self {
            let west = ChunkArea {
                min_x: None,
                max_x: Some(0),
            };
            let east = ChunkArea {
                min_x: Some(16),
                max_x: None,
            };
            Self::new(Division {
                home: ORIGIN,
                pinned: vec![west, east],
                layout: None,
            })
        }

        fn hello(&self, region: RegionId, epoch: u64) -> RegionHello {
            RegionHello {
                region,
                epoch,
                layout: self.division.layout.unwrap_or(0),
            }
        }

        /// Opens `region` as its owner with `epoch`, for a test that plays the region
        /// itself.
        fn open(&self, region: RegionId, epoch: u64) -> (StoreHandle, Restored) {
            self.store.open_region(self.hello(region, epoch)).unwrap()
        }

        /// A runner for `region` as the store has it, opened with `epoch`, that gives a
        /// chunk back as soon as nothing uses it.
        fn runner(&self, region: RegionId, epoch: u64) -> RegionRunner {
            let (handle, restored) = self.open(region, epoch);
            RegionRunner::restore(config(0), handle, restored).unwrap()
        }

        /// A runner for `region` as its first owner, with a gate before its store.
        fn gated(
            &self,
            region: RegionId,
            config: RegionConfig,
        ) -> (RegionRunner, Arc<GateControl>) {
            gated_as(&self.store, self.hello(region, 1), config)
        }
    }

    /// Asks the store through `handle` to grant `chunks`, and returns what it answers:
    /// those it grants, and those another region holds.
    fn claim(
        handle: &StoreHandle,
        chunks: &[ChunkPos],
    ) -> (Vec<ChunkPos>, Vec<(ChunkPos, RegionId)>) {
        handle.request(StoreRequest::Claim {
            chunks: chunks.to_vec(),
        });
        for _ in 0..20_000 {
            match handle.try_reply() {
                Some(StoreReply::Claimed { granted, foreign }) => return (granted, foreign),
                Some(other) => panic!("the store answered a claim with {other:?}"),
                None => thread::sleep(Duration::from_millis(1)),
            }
        }
        panic!("the store did not answer the claim");
    }

    /// Claims `chunk` through `handle` until the store grants it, stepping `runner` in
    /// between: a chunk that a region gives back is free once the store has got to the
    /// return, which nothing tells anyone.
    fn claim_until_granted(runner: &mut RegionRunner, handle: &StoreHandle, chunk: ChunkPos) {
        for _ in 0..2000 {
            if claim(handle, &[chunk]).0 == [chunk] {
                return;
            }
            step(runner);
        }
        panic!("the chunk was never given back");
    }

    /// What an answer to a subscription says, without the content of a snapshot.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum Told {
        Snapshot,
        Elsewhere(RegionId),
        NotMine,
    }

    /// The chunk `message` answers a subscription to, the number of the asking it
    /// answers and what it says, if it is such an answer.
    fn told(message: &WorkerToEdge) -> Option<(ChunkPos, u64, Told)> {
        match message {
            WorkerToEdge::ChunkSnapshot { position, ask, .. } => {
                Some((*position, *ask, Told::Snapshot))
            }
            WorkerToEdge::Elsewhere { chunk, ask, region } => {
                Some((*chunk, *ask, Told::Elsewhere(*region)))
            }
            WorkerToEdge::NotMine { chunk, ask } => Some((*chunk, *ask, Told::NotMine)),
            _ => None,
        }
    }

    /// Steps `runner` until `edge` has been given `count` answers to subscriptions, and
    /// returns them in ascending order of their chunks. What else it is told meanwhile
    /// is passed over.
    fn answers(
        runner: &mut RegionRunner,
        edge: &mut TestEdge,
        count: usize,
    ) -> Vec<(ChunkPos, u64, Told)> {
        let mut answers = Vec::new();
        for _ in 0..2000 {
            answers.extend(received(edge).iter().filter_map(told));
            if answers.len() >= count {
                answers.sort();
                return answers;
            }
            step(runner);
            thread::sleep(Duration::from_millis(1));
        }
        panic!("of {count} answers only these came: {answers:?}");
    }

    /// Subscribes `edge` to a chunk in the column `x` that nobody else needs, which has
    /// to be one the region is granted, and steps `runner` until its snapshot is there.
    /// Returns what the edge was told meanwhile, but for that snapshot.
    ///
    /// The store answers claims in the order they were made and loads in the order they
    /// were asked, and a tick is published whole and after those before it. So whatever
    /// the askings before this one lead to has been told by then, and a test can say
    /// that something did not come.
    fn told_until_marked(
        runner: &mut RegionRunner,
        edge: &mut TestEdge,
        x: i32,
    ) -> Vec<WorkerToEdge> {
        static NEXT: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(1000);
        let mark = ChunkPos::new(x, NEXT.fetch_add(1, Ordering::Relaxed));
        edge.try_send(asking_for(vec![mark])).unwrap();
        let mut before = Vec::new();
        for _ in 0..2000 {
            let batch = received(edge);
            let marked = batch
                .iter()
                .any(|message| told(message).is_some_and(|told| told.0 == mark));
            before.extend(
                batch
                    .into_iter()
                    .filter(|message| told(message).is_none_or(|told| told.0 != mark)),
            );
            if marked {
                edge.try_send(done_with(vec![mark])).unwrap();
                return before;
            }
            step(runner);
            thread::sleep(Duration::from_millis(1));
        }
        panic!("the marking snapshot never came");
    }

    /// Waits until the gate holds back `count` answers of the kind that `kept` counts.
    fn wait_for_kept_answers(
        runner: &mut RegionRunner,
        gate: &GateControl,
        kept: fn(&GateControl) -> usize,
        count: usize,
    ) {
        for _ in 0..20_000 {
            assert!(runner.take_replies());
            if kept(gate) >= count {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("the store did not answer");
    }

    /// A viewer's subscription is answered with the chunk if the region holds it, which
    /// it finds out by asking the store, and else with who does. Of a chunk that is
    /// elsewhere nothing more is said until the edge asks again.
    #[tokio::test]
    async fn a_viewer_is_sent_what_the_region_holds_and_told_who_holds_the_rest() {
        let world = Divided::stripes();
        let (mut edge, worker_end) = in_process(256);
        let mut runner = world.runner(RegionId(0), 1);
        runner.links().attach(worker_end);
        assert_eq!(runner.region().knowledge(ORIGIN), Knowledge::Unknown);

        edge.send(asking_for(vec![ORIGIN, BESIDE])).await.unwrap();
        assert_eq!(
            answers(&mut runner, &mut edge, 2),
            [
                (ORIGIN, 1, Told::Snapshot),
                (BESIDE, 1, Told::Elsewhere(EAST))
            ]
        );
        assert_eq!(runner.region().knowledge(ORIGIN), Knowledge::Held);
        assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Foreign(EAST));
        assert_eq!(runner.region().loaded_chunk_count(), 1);

        // What happens in the chunk that is elsewhere is not told.
        for (entity, chunk) in [(EntityId(8), BESIDE), (EntityId(9), ORIGIN)] {
            edge.send(EdgeToWorker::Discard { entity, chunk })
                .await
                .unwrap();
        }
        let removed = RegionEvent::EntityRemoved {
            entity: EntityId(9),
            chunk: ORIGIN,
        };
        assert_eq!(events(step_for(&mut runner, &mut edge)), [removed]);

        // Asked again, the region asks the store again, and answers with the number of
        // that asking.
        edge.send(asking_for(vec![BESIDE])).await.unwrap();
        assert_eq!(
            answers(&mut runner, &mut edge, 1),
            [(BESIDE, 2, Told::Elsewhere(EAST))]
        );
        assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Foreign(EAST));
    }

    /// A guest is served what the region holds, and what it may take without taking it
    /// from anyone: a chunk of an area it is pinned to. Of any other chunk it is told
    /// that it is not the region's, and the region does not ask for it.
    #[tokio::test]
    async fn a_guest_is_sent_what_the_region_holds_and_told_that_the_rest_is_not_its() {
        let world = Divided::stripes();
        let (mut edge, worker_end) = in_process(256);
        let mut runner = world.runner(RegionId(0), 1);
        runner.links().attach(worker_end);
        let own = ChunkPos::new(-1, 0);

        edge.send(asking_as_guest_for(vec![own, BESIDE]))
            .await
            .unwrap();
        assert_eq!(
            answers(&mut runner, &mut edge, 2),
            [(own, 1, Told::Snapshot), (BESIDE, 1, Told::NotMine)]
        );
        // The subscription that was told so is over.
        assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Unknown);
        let subscribed: Vec<_> = runner.links[&LinkId(0)].subscriptions.keys().collect();
        assert_eq!(subscribed, [&own]);

        // On open land a guest is no reason to take a chunk: it stays free.
        let world = Divided::gap();
        let (mut edge, worker_end) = in_process(256);
        let mut runner = world.runner(HOME, 1);
        runner.links().attach(worker_end);
        edge.send(asking_as_guest_for(vec![BESIDE])).await.unwrap();
        assert_eq!(
            answers(&mut runner, &mut edge, 1),
            [(BESIDE, 1, Told::NotMine)]
        );
        let (neighbour, _) = world.open(EAST, 1);
        assert_eq!(claim(&neighbour, &[BESIDE]), (vec![BESIDE], vec![]));
    }

    /// A subscription that was told elsewhere, then made a guest's and told that the
    /// chunk is not the region's, is over. Asking again begins anew.
    #[tokio::test]
    async fn a_subscription_that_ended_with_not_mine_begins_anew_when_it_is_asked_for_again() {
        let world = Divided::stripes();
        let (mut edge, worker_end) = in_process(256);
        let mut runner = world.runner(RegionId(0), 1);
        runner.links().attach(worker_end);

        for (message, told) in [
            (asking_for(vec![BESIDE]), Told::Elsewhere(EAST)),
            (asking_as_guest_for(vec![BESIDE]), Told::NotMine),
            (asking_for(vec![BESIDE]), Told::Elsewhere(EAST)),
        ] {
            edge.send(message).await.unwrap();
            let ask = edge.asked();
            assert_eq!(answers(&mut runner, &mut edge, 1), [(BESIDE, ask, told)]);
        }
    }

    /// A chunk outside the pinned areas is the region's from a viewer's asking for it
    /// until no link is subscribed to it any more, whatever kind the subscription is
    /// by then. Changing the kind of a subscription that is served costs no answer
    /// and loses no event.
    #[tokio::test]
    async fn a_chunk_stays_with_its_region_while_a_link_is_subscribed_to_it_as_viewer_or_as_guest()
    {
        let world = Divided::gap();
        let (mut edge, worker_end) = in_process(256);
        let mut runner = world.runner(HOME, 1);
        runner.links().attach(worker_end);
        let status = runner.status();
        let (neighbour, _) = world.open(EAST, 1);
        let taken = (vec![], vec![(BESIDE, HOME)]);

        edge.send(asking_for(vec![BESIDE])).await.unwrap();
        assert_eq!(
            answers(&mut runner, &mut edge, 1),
            [(BESIDE, 1, Told::Snapshot)]
        );
        // The home chunk and this one.
        assert_eq!(status.held.load(Ordering::Relaxed), 2);
        assert_eq!(claim(&neighbour, &[BESIDE]), taken);

        // As a guest's, and as a viewer's again.
        for (entity, message) in [
            (EntityId(8), asking_as_guest_for(vec![BESIDE])),
            (EntityId(9), asking_for(vec![BESIDE])),
        ] {
            edge.send(message).await.unwrap();
            let discard = EdgeToWorker::Discard {
                entity,
                chunk: BESIDE,
            };
            edge.send(discard).await.unwrap();
            let told: Vec<_> = told_until_marked(&mut runner, &mut edge, 15)
                .into_iter()
                .map(events)
                .collect();
            let removed = RegionEvent::EntityRemoved {
                entity,
                chunk: BESIDE,
            };
            assert_eq!(told, [vec![removed]]);
            assert_eq!(claim(&neighbour, &[BESIDE]), taken);
        }

        // Once the link lets go of it, nothing uses the chunk, and it is given back.
        edge.send(done_with(vec![BESIDE])).await.unwrap();
        claim_until_granted(&mut runner, &neighbour, BESIDE);
        assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Unknown);
    }

    /// A link that ends takes its tickets with it, so what the region held for it
    /// alone is given back.
    #[tokio::test]
    async fn the_chunks_of_a_link_that_ended_are_given_back() {
        let world = Divided::gap();
        let (mut edge, worker_end) = in_process(256);
        let mut runner = world.runner(HOME, 1);
        runner.links().attach(worker_end);
        let (neighbour, _) = world.open(EAST, 1);
        edge.send(asking_for(vec![BESIDE])).await.unwrap();
        assert_eq!(
            answers(&mut runner, &mut edge, 1),
            [(BESIDE, 1, Told::Snapshot)]
        );

        drop(edge);
        claim_until_granted(&mut runner, &neighbour, BESIDE);
        // The chunk players enter in is never given back.
        assert_eq!(runner.region().knowledge(ORIGIN), Knowledge::Held);
    }

    /// A region that was told who holds a chunk does not learn by itself that the
    /// chunk has become free. The edge's asking again has it ask the store again.
    #[tokio::test]
    async fn asking_again_finds_a_chunk_that_its_holder_has_given_back() {
        let world = Divided::gap();
        let (mut edge, worker_end) = in_process(256);
        let mut runner = world.runner(HOME, 1);
        runner.links().attach(worker_end);
        let (neighbour, _) = world.open(EAST, 1);
        assert_eq!(claim(&neighbour, &[BESIDE]), (vec![BESIDE], vec![]));

        edge.send(asking_for(vec![BESIDE])).await.unwrap();
        assert_eq!(
            answers(&mut runner, &mut edge, 1),
            [(BESIDE, 1, Told::Elsewhere(EAST))]
        );

        // The neighbour gives the chunk back, and the store has got to it.
        neighbour.request(StoreRequest::Return {
            chunks: vec![BESIDE],
        });
        neighbour.flush();
        let told = told_until_marked(&mut runner, &mut edge, 15);
        assert!(told.is_empty(), "{told:?}");
        assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Foreign(EAST));

        edge.send(asking_for(vec![BESIDE])).await.unwrap();
        let ask = edge.asked();
        assert_eq!(
            answers(&mut runner, &mut edge, 1),
            [(BESIDE, ask, Told::Snapshot)]
        );
    }

    /// Several askings for one chunk before the region gets to answer share one
    /// answer, which carries the number of the last and is of the kind the
    /// subscription has by then.
    #[tokio::test]
    async fn an_answer_carries_the_number_of_the_last_asking() {
        let world = Divided::stripes();
        let (mut edge, worker_end) = in_process(256);
        let mut runner = world.runner(RegionId(0), 1);
        runner.links().attach(worker_end);
        let (again, turned) = (ChunkPos::new(-1, 0), ChunkPos::new(-2, 0));

        // All of it before the region ticks once.
        for message in [
            // Ended and made again.
            asking_for(vec![again]),
            done_with(vec![again]),
            asking_for(vec![again]),
            // A viewer's turned into a guest's: no `Elsewhere` comes for it.
            asking_for(vec![BESIDE]),
            asking_as_guest_for(vec![BESIDE]),
            // A guest's turned into a viewer's.
            asking_as_guest_for(vec![turned]),
            asking_for(vec![turned]),
        ] {
            edge.send(message).await.unwrap();
        }
        let mut told: Vec<_> = told_until_marked(&mut runner, &mut edge, 0)
            .iter()
            .filter_map(told)
            .collect();
        told.sort();
        assert_eq!(
            told,
            [
                (turned, 7, Told::Snapshot),
                (again, 3, Told::Snapshot),
                (BESIDE, 5, Told::NotMine)
            ]
        );
        let link = &runner.links[&LinkId(0)];
        assert_eq!(link.subscriptions[&turned].kind, Ticket::Viewer);
        assert!(!link.subscriptions.contains_key(&BESIDE));
    }

    /// Answers carry the numbers of subscription messages, so an edge that numbers
    /// them out of order has lost track, as with the numbers of what changes the
    /// region. A hello's chunks are the message with the number 0.
    #[tokio::test]
    async fn a_link_whose_subscription_messages_are_out_of_order_is_closed() {
        let hello = |edge: &TestEdge, chunks: &[ChunkPos], guests: &[ChunkPos]| {
            let EdgeToWorker::Hello {
                edge,
                start,
                since,
                seen,
                players,
                ..
            } = edge.hello(0, &[], &[])
            else {
                unreachable!("a hello is a hello");
            };
            EdgeToWorker::Hello {
                edge,
                start,
                since,
                seen,
                players,
                chunks: chunks.to_vec(),
                guests: guests.to_vec(),
            }
        };
        let subscribe = |ask| EdgeToWorker::Subscribe {
            ask,
            chunks: vec![ORIGIN],
        };
        let as_guest = |ask| EdgeToWorker::SubscribeAsGuest {
            ask,
            chunks: vec![ORIGIN],
        };
        let unsubscribe = |ask| EdgeToWorker::Unsubscribe {
            ask,
            chunks: vec![ORIGIN],
        };
        let wrong = |edge: &TestEdge| {
            [
                vec![subscribe(0)],
                vec![subscribe(2), unsubscribe(2)],
                vec![subscribe(2), as_guest(1)],
                vec![unsubscribe(1), subscribe(1)],
                vec![subscribe(1), hello(edge, &[ORIGIN], &[])],
                vec![unsubscribe(1), hello(edge, &[], &[ORIGIN])],
                vec![hello(edge, &[ORIGIN], &[]), subscribe(0)],
            ]
        };
        for case in 0..7 {
            let (end, worker_end) = link::in_process(256);
            let mut edge = TestEdge::silent(end, EdgeId::from_name("out-of-order"), 1);
            let mut runner = runner(worker_end);
            for message in wrong(&edge).into_iter().nth(case).unwrap() {
                let message = EdgeMessage::unnumbered(message);
                edge.send_as_is(message).await.unwrap();
            }
            step_until(&mut runner, |runner| runner.links.is_empty());
            assert!(closed(&mut edge).await, "case {case}");
        }

        // Numbers that only ascend are in order, with gaps or without, and a hello that
        // names no chunk can come behind a subscription.
        let (end, worker_end) = link::in_process(256);
        let mut edge = TestEdge::silent(end, EdgeId::from_name("in-order"), 1);
        let mut runner = runner(worker_end);
        for message in [
            subscribe(1),
            hello(&edge, &[], &[]),
            as_guest(5),
            unsubscribe(6),
        ] {
            let message = EdgeMessage::unnumbered(message);
            edge.send_as_is(message).await.unwrap();
        }
        step(&mut runner);
        assert!(edge.everything().contains(&UNKNOWN));
        assert_eq!(runner.links[&LinkId(0)].asked, Some(6));
    }

    /// What an edge sends behind its hello is applied only when the region knows of
    /// every chunk the hello names whose it is: each has been answered, a viewer's and
    /// a guest's alike, with the number 0.
    #[tokio::test]
    async fn a_hello_holds_its_link_until_each_of_its_chunks_is_answered() {
        let world = Divided::stripes();
        let (end, worker_end) = link::in_process(256);
        let mut edge = TestEdge::silent(end, EdgeId::from_name("held"), 1);
        let mut runner = world.runner(RegionId(0), 1);
        runner.links().attach(worker_end);
        let (own, far) = (ChunkPos::new(-1, 0), ChunkPos::new(2, 0));

        let hello = EdgeToWorker::Hello {
            edge: edge.edge,
            start: edge.start,
            since: 0,
            seen: 0,
            players: Vec::new(),
            chunks: vec![ORIGIN, BESIDE],
            // One of its own, one of the other stripe, and one that is a viewer's too.
            guests: vec![own, far, ORIGIN],
        };
        edge.send(hello).await.unwrap();
        edge.send(join(player(), "Notch")).await.unwrap();

        let all = BTreeSet::from([
            (ORIGIN, 0, Told::Snapshot),
            (BESIDE, 0, Told::Elsewhere(EAST)),
            (own, 0, Told::Snapshot),
            (far, 0, Told::NotMine),
        ]);
        let mut answered = BTreeSet::new();
        let mut once = 0;
        step_until(&mut runner, |runner| {
            for answer in edge.everything().iter().filter_map(told) {
                answered.insert(answer);
                once += 1;
            }
            let joined = runner.region().player_count() == 1;
            assert!(!joined || answered == all, "{answered:?}");
            joined
        });
        assert_eq!(once, 4, "each is answered once");
        // A chunk in both lists is a viewer's.
        let link = &runner.links[&LinkId(0)];
        assert_eq!(link.subscriptions[&ORIGIN].kind, Ticket::Viewer);
        assert_eq!(link.subscriptions[&own].kind, Ticket::Guest);
    }

    /// A player who steps into a chunk the region knows nothing of stays the region's
    /// until the store has said whose the chunk is. If it is another region's they are
    /// let go in the tick of that answer, before the edge is told of the chunk.
    #[tokio::test]
    async fn a_player_in_a_chunk_nobody_asked_about_is_let_go_when_the_store_has_answered() {
        let world = Divided::stripes();
        let (mut edge, worker_end) = in_process(256);
        let mut runner = world.runner(RegionId(0), 1);
        runner.links().attach(worker_end);
        joined(&edge, &mut runner).await;
        edge.everything();

        // The step and the edge's asking for the chunk it leads into come together.
        edge.send(walk(player(), 16.5)).await.unwrap();
        edge.send(asking_for(vec![BESIDE])).await.unwrap();
        let ask = edge.asked();
        runner.step();
        assert_eq!(x_of(&runner, player()), Some(16.5));
        assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Asked);

        step_until(&mut runner, |runner| runner.region().player_count() == 0);
        let log = edge.everything();
        let departed = log.iter().position(|message| {
            matches!(
                message,
                WorkerToEdge::Outbox {
                    entry: Durable::Departed { to: EAST, transfer, .. },
                    ..
                } if transfer.pose.position.x == 16.5
            )
        });
        let elsewhere = WorkerToEdge::Elsewhere {
            chunk: BESIDE,
            ask,
            region: EAST,
        };
        let elsewhere = log.iter().position(|message| *message == elsewhere);
        assert!(departed.is_some() && departed < elsewhere, "{log:?}");
    }

    /// What a player does to a block of a chunk the region has no answer about is
    /// passed on without a region named, which leaves it to the edge to know who serves
    /// it the chunk; once the store has said who holds the chunk, with that region.
    #[tokio::test]
    async fn a_block_of_a_chunk_is_passed_on_with_its_holder_only_once_the_store_has_named_it() {
        let world = Divided::stripes();
        let (mut edge, worker_end) = in_process(256);
        let mut runner = world.runner(RegionId(0), 1);
        runner.links().attach(worker_end);
        joined(&edge, &mut runner).await;
        edge.send(walk(player(), 14.5)).await.unwrap();
        step(&mut runner);
        edge.everything();

        let passed_on = |message: WorkerToEdge| match message {
            WorkerToEdge::Outbox {
                entry: Durable::Remote { action, to },
                ..
            } => (action.sequence, to),
            other => panic!("expected an action passed on, got {other:?}"),
        };
        edge.send(dig_by(player(), 16, 1)).await.unwrap();
        assert_eq!(passed_on(step_for(&mut runner, &mut edge)), (1, None));
        // A click is no reason to ask whose the chunk is.
        assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Unknown);

        edge.send(asking_for(vec![BESIDE])).await.unwrap();
        let ask = edge.asked();
        assert_eq!(
            answers(&mut runner, &mut edge, 1),
            [(BESIDE, ask, Told::Elsewhere(EAST))]
        );
        edge.send(dig_by(player(), 16, 2)).await.unwrap();
        assert_eq!(passed_on(step_for(&mut runner, &mut edge)), (2, Some(EAST)));
    }

    /// A player who arrives for a chunk the region believes another's is sent on, and
    /// is on their way as one the region let go: if the edge that was to pass them on
    /// starts anew, or stays away until the region forgets it, every link is told that
    /// the entity is gone.
    #[tokio::test]
    async fn an_arrival_that_was_sent_on_and_then_dropped_is_reported_removed_to_every_link() {
        for stays_away in [false, true] {
            let world = Divided::stripes();
            let (mut edge, worker_end) = in_process(256);
            let (mut watcher, watcher_end) = in_process(256);
            let mut runner = world.runner(RegionId(0), 1).with_gone_after(20);
            runner.links().attach(worker_end);
            runner.links().attach(watcher_end);

            edge.send(asking_for(vec![BESIDE])).await.unwrap();
            assert_eq!(
                answers(&mut runner, &mut edge, 1),
                [(BESIDE, 1, Told::Elsewhere(EAST))]
            );
            let arriving = PlayerTransfer {
                pose: Pose::at(Vec3::new(16.5, -60.0, 0.5)),
                ..transfer(EntityId(77))
            };
            edge.send(EdgeToWorker::PlayerArrive {
                player: other_player(),
                transfer: arriving.clone(),
            })
            .await
            .unwrap();
            let sent_on = WorkerToEdge::Outbox {
                number: 1,
                entry: Durable::NotMine {
                    what: Misdirected::Arrival {
                        player: other_player(),
                        transfer: arriving,
                    },
                    holder: EAST,
                },
            };
            assert_eq!(step_for(&mut runner, &mut edge), sent_on);
            assert_eq!(runner.region().player_count(), 0);

            // The edge does not pass the player on. The watcher is subscribed to
            // nothing and hears all the same that the entity is gone.
            let (end, anew_end) = link::in_process(256);
            let anew = TestEdge::silent(end, edge.edge, edge.start + 1);
            if stays_away {
                drop(edge);
            } else {
                anew.send(anew.hello(0, &[], &[])).await.unwrap();
                runner.links().attach(anew_end);
            }
            let removed = RegionEvent::EntityRemoved {
                entity: EntityId(77),
                chunk: BESIDE,
            };
            assert_eq!(events(step_for(&mut runner, &mut watcher)), [removed]);
            assert_eq!(runner.region().edge(anew.edge).is_none(), stays_away);
        }
    }

    /// The status has how many chunks the store has granted the region and where its
    /// players are, for the coordinator to merge and split regions by.
    #[tokio::test]
    async fn the_status_counts_the_chunks_the_region_was_granted_and_says_where_its_players_are() {
        let world = Divided::gap();
        let (edge, worker_end) = in_process(256);
        let mut runner = world.runner(HOME, 1);
        runner.links().attach(worker_end);
        let status = runner.status();
        // The home chunk was granted to the home region when the world was made.
        assert_eq!(status.held.load(Ordering::Relaxed), 1);
        assert!(status.crowds().is_empty());

        joined(&edge, &mut runner).await;
        assert_eq!(status.crowds(), [(ORIGIN, 1)]);

        // A second player, and the first walks into a chunk that is nobody's: the
        // region takes it.
        edge.send(join(other_player(), "Jeb")).await.unwrap();
        edge.send(walk(player(), 16.5)).await.unwrap();
        step_until(&mut runner, |runner| {
            runner.region().knowledge(BESIDE) == Knowledge::Held
        });
        assert_eq!(status.crowds(), [(ORIGIN, 1), (BESIDE, 1)]);
        assert_eq!(status.held.load(Ordering::Relaxed), 2);
        assert_eq!(runner.region().player_count(), 2);
    }

    /// What a region holds is the store's to say when the region is opened: the next
    /// owner holds what the owner before was granted, and loads it without asking.
    #[tokio::test]
    async fn the_next_owner_of_a_region_holds_what_the_store_had_granted_it() {
        let world = Divided::gap();
        let (mut edge, worker_end) = in_process(256);
        let mut runner = world.runner(HOME, 1);
        runner.links().attach(worker_end);
        edge.send(asking_for(vec![BESIDE])).await.unwrap();
        assert_eq!(
            answers(&mut runner, &mut edge, 1),
            [(BESIDE, 1, Told::Snapshot)]
        );

        // The next owner keeps a chunk for long, so that what it holds can be looked at.
        let hello = world.hello(HOME, 2);
        let (mut next, gate) = gated_as(&world.store, hello, config(1000));
        drop(runner);
        assert_eq!(next.region().knowledge(ORIGIN), Knowledge::Held);
        assert_eq!(next.region().knowledge(BESIDE), Knowledge::Held);
        assert_eq!(
            next.region().knowledge(ChunkPos::new(2, 0)),
            Knowledge::Unknown
        );
        assert_eq!(next.status().held.load(Ordering::Relaxed), 2);

        let (mut edge, worker_end) = in_process(256);
        next.links().attach(worker_end);
        edge.send(asking_for(vec![BESIDE])).await.unwrap();
        assert_eq!(
            answers(&mut next, &mut edge, 1),
            [(BESIDE, 1, Told::Snapshot)]
        );
        let asked = gate.asked();
        assert!(asked.contains(&Asked::Load(BESIDE)), "{asked:?}");
        assert!(
            asked.iter().all(|asked| !matches!(asked, Asked::Claim(_))),
            "{asked:?}"
        );
    }

    /// A chunk is given back only when nothing has used it for as many ticks as the
    /// region is made with, so that a region whose edge is away for a moment does not
    /// shed everything it holds.
    #[tokio::test]
    async fn a_chunk_is_given_back_only_after_nothing_has_used_it_for_a_while() {
        let world = Divided::gap();
        let (mut edge, worker_end) = in_process(256);
        let (mut runner, gate) = world.gated(HOME, config(30));
        runner.links().attach(worker_end);
        edge.send(asking_for(vec![BESIDE])).await.unwrap();
        assert_eq!(
            answers(&mut runner, &mut edge, 1),
            [(BESIDE, 1, Told::Snapshot)]
        );
        gate.asked();

        // The tick that takes the ticket back is the first in which nothing uses the
        // chunk.
        edge.send(done_with(vec![BESIDE])).await.unwrap();
        step(&mut runner);
        let unused_from = runner.region().tick_number();
        let returned = Asked::Return(vec![BESIDE]);
        step_until(&mut runner, |_| gate.asked().contains(&returned));
        assert_eq!(runner.region().tick_number(), unused_from + 30);
        assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Unknown);
    }

    /// What the store is asked after a tick is in a fixed order, and a chunk that is
    /// given back has been saved before, with every change the region made to it.
    #[tokio::test]
    async fn after_a_tick_the_store_is_asked_in_a_fixed_order_and_a_chunk_is_saved_before_it_is_returned()
     {
        let world = Divided::gap();
        let (mut edge, worker_end) = in_process(256);
        let (mut runner, gate) = world.gated(HOME, config(0));
        runner.links().attach(worker_end);
        let (loaded, claimed) = (ChunkPos::new(2, 0), ChunkPos::new(3, 0));

        // A player changes a block of a chunk the region was granted for the link.
        joined(&edge, &mut runner).await;
        edge.send(asking_for(vec![BESIDE])).await.unwrap();
        edge.send(walk(player(), 14.5)).await.unwrap();
        answers(&mut runner, &mut edge, 2);
        edge.send(dig(16)).await.unwrap();
        step(&mut runner);
        let changed = runner.region().chunk(BESIDE).unwrap();
        assert_eq!(changed.get(0, -61, 0), Some(clustine_data::blocks::AIR));

        // The store's answer to a claim of another chunk is on its way.
        gate.hold_claims();
        edge.send(asking_for(vec![loaded])).await.unwrap();
        step(&mut runner);
        wait_for_kept_answers(&mut runner, &gate, GateControl::kept_claims, 1);
        gate.asked();

        // One tick with everything: the answer comes, so the chunk is asked of
        // storage; a player joins, which is a commit; the changed chunk is let go of
        // and given back; a third chunk is asked for; and it is time for a checkpoint.
        runner.checkpoint_interval = 1;
        edge.send(join(other_player(), "Jeb")).await.unwrap();
        edge.send(done_with(vec![BESIDE])).await.unwrap();
        edge.send(asking_for(vec![claimed])).await.unwrap();
        gate.release_claims();
        runner.step();
        let tick = runner.region().tick_number();
        assert_eq!(
            gate.asked(),
            [
                // Before the tick that drops the chunk.
                Asked::Save(BESIDE),
                Asked::Load(loaded),
                Asked::Commit(tick),
                Asked::Return(vec![BESIDE]),
                Asked::Claim(vec![claimed]),
                Asked::Checkpoint(tick),
            ]
        );
    }

    /// A region loads and saves only what it holds. If the store says otherwise, the
    /// two disagree, and the runner stops as it does when the store is lost.
    #[tokio::test]
    async fn a_runner_that_is_told_it_does_not_hold_a_chunk_it_loads_gives_up() {
        let world = Divided::stripes();
        let (mut edge, worker_end) = in_process(256);
        // The region is told on opening that it was granted a chunk of the other stripe,
        // of which the store knows nothing: the test makes the disagreement up, as
        // nothing a region and a store do brings one about.
        let (handle, mut restored) = world.open(RegionId(0), 1);
        restored.held.push((BESIDE, 0));
        let mut runner = RegionRunner::restore(config(0), handle, restored).unwrap();
        runner.links().attach(worker_end);
        let status = runner.status();

        edge.send(asking_for(vec![BESIDE])).await.unwrap();
        step_until(&mut runner, |runner| runner.store_is_lost());
        assert_eq!(runner.ended(), Some(Ended::StoreLost));
        assert!(status.store_lost.load(Ordering::Relaxed));
        assert_eq!(status.ended(), Some(Ended::StoreLost));
        assert!(closed(&mut edge).await);
        let before = runner.region().tick_number();
        runner.step();
        assert_eq!(runner.region().tick_number(), before);
    }

    /// A chunk the region has asked for is asked until the store answers. A runner that
    /// is releasing its region and still ticks takes the answer into its next tick
    /// like any other; what it was granted is the next owner's.
    #[tokio::test]
    async fn an_answer_to_a_claim_that_comes_while_a_release_still_ticks_is_taken_into_the_next_tick()
     {
        let world = Divided::gap();
        let (mut edge, worker_end) = in_process(256);
        let (mut runner, gate) = world.gated(HOME, config(0));
        runner.links().attach(worker_end);

        gate.hold_claims();
        edge.send(asking_for(vec![BESIDE])).await.unwrap();
        step(&mut runner);
        assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Asked);
        wait_for_kept_answers(&mut runner, &gate, GateControl::kept_claims, 1);

        // The release waits for the store to have its first checkpoint, and ticks on.
        gate.hold_flushes();
        runner.begin_release();
        gate.release_claims();
        runner.step();
        assert_eq!(runner.phase, Phase::Preparing);
        assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Held);
        assert_eq!(
            answers(&mut runner, &mut edge, 1),
            [(BESIDE, 1, Told::Snapshot)]
        );
        assert_eq!(runner.phase, Phase::Preparing);

        gate.release_flushes();
        assert_eq!(released(&mut runner), Ended::Released);
        let (_handle, restored) = world.open(HOME, 2);
        let held: Vec<_> = restored.held.iter().map(|(chunk, _)| *chunk).collect();
        assert_eq!(held, [ORIGIN, BESIDE]);
    }

    /// The region asks storage for a chunk, drops the request with the chunk's last
    /// ticket and gives the chunk back; another region changes it; the region is
    /// granted it again and asks for it again. What storage read for the first request
    /// is not the chunk any more, and is not taken for the answer to the second.
    #[tokio::test]
    async fn what_storage_read_for_a_request_the_region_dropped_is_not_taken_for_a_later_one() {
        let world = Divided::gap();
        let (mut edge, worker_end) = in_process(256);
        let (mut runner, gate) = world.gated(HOME, config(0));
        runner.links().attach(worker_end);
        let (neighbour, _) = world.open(EAST, 1);
        let held = |runner: &RegionRunner| runner.region().knowledge(BESIDE) == Knowledge::Held;

        // The first request, whose answer is on its way for a long time.
        gate.hold_loads();
        edge.send(asking_for(vec![BESIDE])).await.unwrap();
        step_until(&mut runner, held);
        wait_for_kept_answers(&mut runner, &gate, GateControl::kept_loads, 1);
        edge.send(done_with(vec![BESIDE])).await.unwrap();

        // The neighbour takes the chunk when it is free, changes it and gives it back.
        claim_until_granted(&mut runner, &neighbour, BESIDE);
        neighbour.request(StoreRequest::Load { position: BESIDE });
        let mut chunk = loop {
            match neighbour.try_reply() {
                Some(StoreReply::Loaded { chunk, .. }) => break chunk,
                Some(other) => panic!("the store answered a load with {other:?}"),
                None => thread::sleep(Duration::from_millis(1)),
            }
        };
        let air = clustine_data::blocks::AIR;
        assert_ne!(chunk.get(5, -61, 5), Some(air));
        chunk.set(5, -61, 5, air);
        neighbour.request(StoreRequest::Save {
            position: BESIDE,
            tick: 0,
            chunk,
        });
        neighbour.request(StoreRequest::Return {
            chunks: vec![BESIDE],
        });
        neighbour.flush();

        // The second request. Both answers come, the older one first.
        edge.send(asking_for(vec![BESIDE])).await.unwrap();
        step_until(&mut runner, held);
        wait_for_kept_answers(&mut runner, &gate, GateControl::kept_loads, 2);
        gate.release_loads();
        let snapshot = step_for(&mut runner, &mut edge);
        let WorkerToEdge::ChunkSnapshot {
            position, chunk, ..
        } = snapshot
        else {
            panic!("expected a snapshot, got {snapshot:?}");
        };
        assert_eq!(position, BESIDE);
        assert_eq!(chunk.get(5, -61, 5), Some(air));
        assert_eq!(runner.region().chunk(BESIDE), Some(&chunk));
    }

    /// That a chunk is elsewhere is part of what a tick produced, like everything else
    /// a link is told: it is not on the link before the commit of that tick and of
    /// those before it is confirmed, and a runner that has lost the region by then
    /// does not say it at all.
    #[tokio::test]
    async fn what_a_link_is_told_of_a_chunk_waits_for_the_commits_like_everything_else() {
        for confirmed in [true, false] {
            let world = Divided::stripes();
            let (mut edge, worker_end) = in_process(256);
            let (mut runner, gate) = world.gated(RegionId(0), config(0));
            runner.links().attach(worker_end);
            step(&mut runner);
            edge.everything();

            // A tick with a commit, in which the region asks about the chunk.
            gate.hold();
            edge.send(join(player(), "Notch")).await.unwrap();
            edge.send(asking_for(vec![BESIDE])).await.unwrap();
            runner.step();
            // The store's answer is there, and the tick that takes it has a commit of
            // its own.
            for _ in 0..20_000 {
                assert!(runner.take_replies());
                if !runner.inputs.foreign.is_empty() {
                    break;
                }
                thread::sleep(Duration::from_millis(1));
            }
            edge.send(join(other_player(), "Jeb")).await.unwrap();
            runner.step();
            assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Foreign(EAST));
            wait_for_kept(&mut runner, &gate, 2);
            let early = edge.everything();
            assert!(early.is_empty(), "{early:?}");

            let entered = |log: &[WorkerToEdge], who: PlayerId| {
                log.iter().position(|message| {
                    matches!(message, WorkerToEdge::ToPlayer { player, .. } if *player == who)
                })
            };
            let elsewhere = WorkerToEdge::Elsewhere {
                chunk: BESIDE,
                ask: 1,
                region: EAST,
            };
            if confirmed {
                gate.release();
                step(&mut runner);
                let log = edge.everything();
                let told = log.iter().position(|message| *message == elsewhere);
                let progress = log
                    .iter()
                    .rposition(|message| matches!(message, WorkerToEdge::Progress { .. }));
                // Behind everything of the tick before, and in its own tick behind who
                // entered the world and before the progress.
                let order = [
                    entered(&log, player()),
                    entered(&log, other_player()),
                    told,
                    progress,
                ];
                assert!(order.iter().all(Option::is_some), "{log:?}");
                assert!(order.is_sorted(), "{order:?} in {log:?}");
            } else {
                // Another owner takes the region.
                let _next = world.open(RegionId(0), 2);
                step_until(&mut runner, |runner| runner.store_is_lost());
                let mut log = Vec::new();
                while let Some(message) = edge.end.recv().await {
                    log.push(message);
                }
                assert!(log.is_empty(), "{log:?}");
            }
        }
    }

    /// What a region changed in a chunk is in the store when it gives the chunk back:
    /// whoever is granted the chunk next loads it as the region left it.
    #[tokio::test]
    async fn a_chunk_that_is_given_back_is_loaded_by_the_next_holder_as_the_region_left_it() {
        let world = Divided::gap();
        let (mut edge, worker_end) = in_process(256);
        let mut runner = world.runner(HOME, 1);
        runner.links().attach(worker_end);
        let (neighbour, _) = world.open(EAST, 1);
        let air = clustine_data::blocks::AIR;

        joined(&edge, &mut runner).await;
        edge.send(asking_for(vec![BESIDE])).await.unwrap();
        edge.send(walk(player(), 14.5)).await.unwrap();
        answers(&mut runner, &mut edge, 2);
        edge.send(dig(16)).await.unwrap();
        step(&mut runner);
        let changed = runner.region().chunk(BESIDE).unwrap().clone();
        assert_eq!(changed.get(0, -61, 0), Some(air));

        edge.send(done_with(vec![BESIDE])).await.unwrap();
        claim_until_granted(&mut runner, &neighbour, BESIDE);
        neighbour.request(StoreRequest::Load { position: BESIDE });
        let loaded = loop {
            match neighbour.try_reply() {
                Some(StoreReply::Loaded { chunk, .. }) => break chunk,
                Some(other) => panic!("the store answered a load with {other:?}"),
                None => thread::sleep(Duration::from_millis(1)),
            }
        };
        assert_eq!(loaded, changed);
    }

    /// A chunk has one ticket for each link that is subscribed to it, of the kind of
    /// that link's subscription: it stays loaded and the region's until the last of
    /// them has let go.
    #[tokio::test]
    async fn a_chunk_that_two_links_are_subscribed_to_stays_until_both_have_let_go() {
        let world = Divided::gap();
        let (mut viewer, viewer_end) = in_process(256);
        let (mut guest, guest_end) = in_process(256);
        let mut runner = world.runner(HOME, 1);
        runner.links().attach(viewer_end);
        runner.links().attach(guest_end);
        let (neighbour, _) = world.open(EAST, 1);

        viewer.send(asking_for(vec![BESIDE])).await.unwrap();
        assert_eq!(
            answers(&mut runner, &mut viewer, 1),
            [(BESIDE, 1, Told::Snapshot)]
        );
        // The guest is served what the region holds for the viewer.
        guest.send(asking_as_guest_for(vec![BESIDE])).await.unwrap();
        assert_eq!(
            answers(&mut runner, &mut guest, 1),
            [(BESIDE, 1, Told::Snapshot)]
        );

        // The viewer lets go. The guest's subscription keeps the chunk, and goes on
        // being told what happens in it.
        viewer.send(done_with(vec![BESIDE])).await.unwrap();
        let discard = EdgeToWorker::Discard {
            entity: EntityId(9),
            chunk: BESIDE,
        };
        guest.send(discard).await.unwrap();
        let told: Vec<_> = told_until_marked(&mut runner, &mut guest, 15)
            .into_iter()
            .map(events)
            .collect();
        let removed = RegionEvent::EntityRemoved {
            entity: EntityId(9),
            chunk: BESIDE,
        };
        assert_eq!(told, [vec![removed]]);
        assert!(runner.region().chunk(BESIDE).is_some());
        assert_eq!(claim(&neighbour, &[BESIDE]), (vec![], vec![(BESIDE, HOME)]));

        guest.send(done_with(vec![BESIDE])).await.unwrap();
        claim_until_granted(&mut runner, &neighbour, BESIDE);
        assert!(runner.region().chunk(BESIDE).is_none());
    }

    // -----------------------------------------------------------------------------------
    // What waits for its chunk, and whom a region says it has: sections 3.6 and 3.7 of
    // `docs/adr/0014-merging-and-splitting.md`.
    // -----------------------------------------------------------------------------------

    /// The first chunk of the western stripe west of the one players enter the world
    /// in, which a player who stands there can reach into.
    const WEST_OF_HOME: ChunkPos = ChunkPos::new(-1, 0);

    /// Whether the block that [`dig`] breaks with `x` is broken, if its chunk is loaded.
    fn dug(runner: &RegionRunner, x: i32) -> Option<bool> {
        let position = BlockPos::new(x, -61, 0);
        let (in_x, in_z) = position.in_chunk();
        let chunk = runner.region().chunk(position.chunk())?;
        Some(chunk.get(in_x, position.y, in_z) == Some(clustine_data::blocks::AIR))
    }

    /// Reads what `edge` is sent up to the word of a block that changed, however long
    /// its link takes over it, and returns whether the snapshot of `chunk` came before.
    async fn shown_before_it_changed(edge: &mut TestEdge, chunk: ChunkPos) -> bool {
        let mut shown = false;
        loop {
            match next(edge).await {
                WorkerToEdge::ChunkSnapshot { position, .. } => shown |= position == chunk,
                WorkerToEdge::TickDelta { events, .. } => {
                    let changed =
                        |event: &RegionEvent| matches!(event, RegionEvent::BlockChanged { .. });
                    if events.iter().any(changed) {
                        return shown;
                    }
                }
                _ => {}
            }
        }
    }

    /// A chunk of the region's own stripe that nothing has asked about is the region's
    /// to serve, though it does not know so yet. A dig that comes right behind the
    /// subscription to it is judged when the chunk is there, and not passed on without
    /// a region as for a chunk nobody is known to hold; what the link sent behind the
    /// dig waits with it.
    #[tokio::test]
    async fn a_dig_behind_a_subscription_waits_for_a_chunk_of_the_regions_own_area() {
        // Over a direct link, on which all three are there when the runner next looks.
        let (mut edge, worker_end) = in_process(256);
        let mut runner = west(worker_end);
        joined(&edge, &mut runner).await;
        settle(&mut runner);
        edge.everything();
        assert_eq!(runner.region().knowledge(WEST_OF_HOME), Knowledge::Unknown);
        let applied = runner.region().edge(edge.edge).unwrap().applied;

        edge.send(asking_for(vec![WEST_OF_HOME])).await.unwrap();
        edge.send(dig(-1)).await.unwrap();
        edge.send(walk(player(), 3.0)).await.unwrap();
        step_until(&mut runner, |runner| {
            runner.links[&LinkId(0)].held.len() == 2
        });
        // Neither is taken until the chunk is claimed, read and shown.
        let mut waited = 0;
        step_until(&mut runner, |runner| {
            let loaded = runner.region().chunk(WEST_OF_HOME).is_some();
            if !loaded {
                waited += 1;
                assert_eq!(runner.links[&LinkId(0)].held.len(), 2);
                assert_eq!(runner.region().edge(edge.edge).unwrap().applied, applied);
                assert_eq!(x_of(runner, player()), Some(SPAWN.x));
            }
            loaded
        });
        assert!(waited > 0);
        step_until(&mut runner, |runner| x_of(runner, player()) == Some(3.0));
        assert_eq!(dug(&runner, -1), Some(true));
        assert_eq!(
            runner.region().edge(edge.edge).unwrap().applied,
            applied + 2
        );
        assert!(runner.links[&LinkId(0)].held.is_empty());
        // Nothing was passed on to anyone.
        assert_eq!(outbox(&runner, &edge), (0, vec![]));

        // The edge is shown the chunk before it is told of the block.
        settle(&mut runner);
        assert!(shown_before_it_changed(&mut edge, WEST_OF_HOME).await);
    }

    /// The same for a chunk the region holds and has not loaded: one it was granted
    /// for a viewer who has let go of it since.
    #[tokio::test]
    async fn a_dig_behind_a_subscription_waits_for_a_chunk_the_region_holds_and_has_not_loaded() {
        let world = Divided::gap();
        let (mut edge, worker_end) = in_process(256);
        let (mut runner, _) = world.gated(HOME, config(40));
        runner.links().attach(worker_end);
        joined(&edge, &mut runner).await;
        edge.everything();
        edge.send(walk(player(), 14.5)).await.unwrap();
        edge.send(asking_for(vec![BESIDE])).await.unwrap();
        assert_eq!(
            answers(&mut runner, &mut edge, 1),
            [(BESIDE, edge.asked(), Told::Snapshot)]
        );
        edge.send(done_with(vec![BESIDE])).await.unwrap();
        step_until(&mut runner, |runner| {
            runner.region().chunk(BESIDE).is_none()
        });
        assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Held);
        settle(&mut runner);
        edge.everything();
        let applied = runner.region().edge(edge.edge).unwrap().applied;

        edge.send(asking_for(vec![BESIDE])).await.unwrap();
        edge.send(dig(16)).await.unwrap();
        edge.send(walk(player(), 13.5)).await.unwrap();
        step(&mut runner);
        assert_eq!(runner.links[&LinkId(0)].held.len(), 2);
        assert_eq!(runner.region().edge(edge.edge).unwrap().applied, applied);
        step_until(&mut runner, |runner| x_of(runner, player()) == Some(13.5));
        assert_eq!(dug(&runner, 16), Some(true));
        assert_eq!(outbox(&runner, &edge), (0, vec![]));
        settle(&mut runner);
        assert!(shown_before_it_changed(&mut edge, BESIDE).await);
    }

    /// The hold is narrow. A chunk of another region's stripe is not this region's to
    /// serve, whatever it knows of it: a dig into it is judged in the tick that takes
    /// it and goes on without a region named, and a move behind it is applied in that
    /// tick. So is a move behind a subscription that waits, and a dig into a chunk the
    /// link has not asked for.
    #[tokio::test]
    async fn what_a_region_is_not_about_to_serve_holds_nothing() {
        let (mut edge, worker_end) = in_process(256);
        let mut runner = west(worker_end);
        joined(&edge, &mut runner).await;
        edge.send(walk(player(), 14.5)).await.unwrap();
        step(&mut runner);
        settle(&mut runner);
        edge.everything();

        // Another region's chunk, asked for and not yet answered.
        edge.send(asking_for(vec![BESIDE])).await.unwrap();
        edge.send(dig_by(player(), 16, 3)).await.unwrap();
        edge.send(walk(player(), 13.5)).await.unwrap();
        step(&mut runner);
        assert!(runner.links[&LinkId(0)].held.is_empty());
        assert_eq!(x_of(&runner, player()), Some(13.5));
        let state = runner.region().edge(edge.edge).unwrap();
        let entries: Vec<_> = state.outbox.values().collect();
        assert!(
            matches!(entries[..], [Durable::Remote { to: None, .. }]),
            "{entries:?}"
        );

        // The region's own chunk: a move behind the subscription that waits for it,
        // and on another link a dig into it without a subscription.
        let (other, other_end) = in_process(256);
        runner.links().attach(other_end);
        other.send(join(other_player(), "Jeb")).await.unwrap();
        step(&mut runner);
        edge.send(asking_for(vec![WEST_OF_HOME])).await.unwrap();
        edge.send(walk(player(), 3.0)).await.unwrap();
        other
            .send(of_entity(EntityId(2), dig_by(other_player(), -1, 1)))
            .await
            .unwrap();
        step(&mut runner);
        assert!(runner.region().chunk(WEST_OF_HOME).is_none());
        assert_eq!(x_of(&runner, player()), Some(3.0));
        let applied = |edge: &TestEdge| runner.region().edge(edge.edge).unwrap().applied;
        assert_eq!(applied(&other), other.sent.load(Ordering::Relaxed));
        assert_eq!(applied(&edge), edge.sent.load(Ordering::Relaxed));
        assert!(runner.links.values().all(|link| link.held.is_empty()));
    }

    /// What a player of another region does to a block waits like a player's own
    /// action: a remote action behind a guest's subscription to a chunk of the
    /// region's own area is taken when the chunk is there, and done.
    #[tokio::test]
    async fn a_remote_action_behind_a_guests_subscription_waits_for_the_chunk() {
        let (edge, worker_end) = in_process(256);
        let mut runner = west(worker_end);
        let action = RemoteAction {
            player: player(),
            sequence: 4,
            step: RemoteStep::Break {
                position: BlockPos::new(-1, -61, 0),
            },
        };

        edge.send(asking_as_guest_for(vec![WEST_OF_HOME]))
            .await
            .unwrap();
        edge.send(EdgeToWorker::Remote(action)).await.unwrap();
        step(&mut runner);
        assert_eq!(runner.links[&LinkId(0)].held.len(), 1);
        assert_eq!(runner.region().edge(edge.edge).unwrap().applied, 0);

        step_until(&mut runner, |runner| {
            runner.region().edge(edge.edge).unwrap().applied == 1
        });
        assert_eq!(dug(&runner, -1), Some(true));
        let state = runner.region().edge(edge.edge).unwrap();
        let entries: Vec<_> = state.outbox.values().collect();
        let done = Durable::RemoteDone {
            player: player(),
            sequence: 4,
        };
        assert_eq!(entries, [&done]);
    }

    /// A chunk the store cannot read is waited for without end, and holds nothing: a
    /// dig into it is taken at once.
    #[tokio::test]
    async fn a_chunk_that_cannot_be_read_holds_no_dig() {
        let (edge, worker_end) = in_process(256);
        let mut runner = west(worker_end);
        joined(&edge, &mut runner).await;
        // As the store's word that it cannot read the chunk leaves it.
        runner.unreadable.insert(WEST_OF_HOME);

        edge.send(asking_for(vec![WEST_OF_HOME])).await.unwrap();
        edge.send(dig(-1)).await.unwrap();
        step(&mut runner);
        assert!(runner.links[&LinkId(0)].held.is_empty());
        let applied = runner.region().edge(edge.edge).unwrap().applied;
        assert_eq!(applied, edge.sent.load(Ordering::Relaxed));
    }

    /// The presence answers among `told`: the players in the order they were answered
    /// for, each with their entity if they are present.
    fn presences(told: &[WorkerToEdge]) -> Vec<(PlayerId, Option<EntityId>)> {
        let answer = |message: &WorkerToEdge| match message {
            WorkerToEdge::Presence { player, answer } => {
                let entity = match answer {
                    Presence::Present { entity, .. } => Some(*entity),
                    Presence::Absent => None,
                };
                Some((*player, entity))
            }
            _ => None,
        };
        told.iter().filter_map(answer).collect()
    }

    /// A region says whom it has for an edge at every hello, named or not: an answer
    /// for each player of the hello, in the hello's order, and then one for every
    /// other stay of the edge, in ascending order. The welcome counts them.
    #[tokio::test]
    async fn a_hello_is_answered_with_every_stay_the_region_has_for_the_edge() {
        let (first, first_end) = in_process(256);
        let (bystander, bystander_end) = in_process(256);
        let mut runner = runner(first_end);
        runner.links().attach(bystander_end);
        // In this order, so that the entities do not ascend with the players.
        first.send(join(other_player(), "Jeb")).await.unwrap();
        first.send(join(player(), "Notch")).await.unwrap();
        // Somebody else's player is nobody's business but that edge's.
        bystander
            .send(join(third_player(), "Dinnerbone"))
            .await
            .unwrap();
        step(&mut runner);
        let applied = first.sent.load(Ordering::Relaxed);

        // A hello that names neither.
        let (end, worker_end) = link::in_process(256);
        let mut again = first.again(end, &runner);
        runner.links().attach(worker_end);
        again.send(again.hello(0, &[], &[])).await.unwrap();
        step(&mut runner);
        let told = again.everything();
        let welcome = Welcome::Resumed {
            entries: 0,
            presences: 2,
            applied,
        };
        assert_eq!(again.welcomed, Some(welcome));
        assert_eq!(
            presences(&told),
            [
                (player(), Some(EntityId(2))),
                (other_player(), Some(EntityId(1)))
            ]
        );
        assert!(
            matches!(
                told[1..3],
                [WorkerToEdge::Presence { .. }, WorkerToEdge::Presence { .. }]
            ),
            "{told:?}"
        );

        // One that names one of them, and somebody who is not there, or not this
        // edge's.
        for stranger in [PlayerId(Uuid::from_u128(9)), third_player()] {
            let (end, worker_end) = link::in_process(256);
            let mut again = first.again(end, &runner);
            runner.links().attach(worker_end);
            let named = [other_player(), stranger];
            again.send(again.hello(0, &named, &[])).await.unwrap();
            step(&mut runner);
            let told = again.everything();
            let welcome = Welcome::Resumed {
                entries: 0,
                presences: 3,
                applied,
            };
            assert_eq!(again.welcomed, Some(welcome));
            assert_eq!(
                presences(&told),
                [
                    (other_player(), Some(EntityId(1))),
                    (stranger, None),
                    (player(), Some(EntityId(2)))
                ]
            );
        }

        // An edge the region does not know is answered for the names of its hello, and
        // a start that resets the edge likewise: whoever is there goes with that tick.
        let (end, worker_end) = link::in_process(256);
        let mut later = TestEdge::silent(end, first.edge, first.start + 1);
        runner.links().attach(worker_end);
        later.send(later.hello(0, &[player()], &[])).await.unwrap();
        step(&mut runner);
        let told = later.everything();
        let welcome = Welcome::Unknown {
            since: runner.region().tick_number(),
            entries: 0,
            presences: 1,
            applied: 0,
        };
        assert_eq!(later.welcomed, Some(welcome));
        assert_eq!(presences(&told), [(player(), None)]);
        assert_eq!(runner.region().player_count(), 1);
    }

    /// A leave that names the entity a presence answer showed ends that stay, and one
    /// that names another entity ends nothing.
    #[tokio::test]
    async fn a_leave_ends_the_stay_a_presence_answer_showed_and_no_other() {
        let (first, first_end) = in_process(256);
        let mut runner = runner(first_end);
        first.send(join(player(), "Notch")).await.unwrap();
        step(&mut runner);

        let (end, worker_end) = link::in_process(256);
        let mut again = first.again(end, &runner);
        runner.links().attach(worker_end);
        again.send(again.hello(0, &[], &[])).await.unwrap();
        step(&mut runner);
        let shown = presences(&again.everything());
        let [(shown_player, Some(entity))] = shown[..] else {
            panic!("{shown:?}");
        };
        assert_eq!(shown_player, player());

        let leave = |entity| EdgeToWorker::PlayerLeave {
            player: player(),
            entity: Some(entity),
        };
        again.send(leave(EntityId(entity.0 + 1))).await.unwrap();
        step(&mut runner);
        assert_eq!(runner.region().player_count(), 1);
        again.send(leave(entity)).await.unwrap();
        step(&mut runner);
        assert_eq!(runner.region().player_count(), 0);
    }

    /// An arrival that takes the place of an earlier stay of its player is a player
    /// who came in from another region, and counted as one; the arrival of the very
    /// stay the region has is not.
    #[tokio::test]
    async fn an_arrival_that_replaces_an_earlier_stay_is_counted_and_the_same_stay_is_not() {
        let (edge, worker_end) = in_process(256);
        let mut runner = runner(worker_end);
        let status = runner.status();
        edge.send(join(player(), "Notch")).await.unwrap();
        step(&mut runner);
        assert_eq!(runner.region().player(player()).unwrap().0, EntityId(1));

        let arrive = |entity| EdgeToWorker::PlayerArrive {
            player: player(),
            transfer: transfer(EntityId(entity)),
        };
        edge.send(arrive(50)).await.unwrap();
        step(&mut runner);
        assert_eq!(runner.region().player(player()).unwrap().0, EntityId(50));
        assert_eq!(status.arrivals.load(Ordering::Relaxed), 1);

        edge.send(arrive(50)).await.unwrap();
        step(&mut runner);
        assert_eq!(status.arrivals.load(Ordering::Relaxed), 1);
    }

    // -----------------------------------------------------------------------------------
    // Merging and splitting: sections 3.1 to 3.5 and 3.8 of
    // `docs/adr/0014-merging-and-splitting.md`, and the builder's own tests of its
    // section 10, which need the gate before the store.
    // -----------------------------------------------------------------------------------

    /// The chunk two to the west of the one players enter the world in.
    const FAR_WEST: ChunkPos = ChunkPos::new(-2, 0);

    /// A chunk of the eastern stripe, away from the line.
    const FAR_EAST: ChunkPos = ChunkPos::new(2, 0);

    /// A call for the outcome of a merge or a split, and where the outcome shows.
    fn outcome() -> (Box<dyn FnOnce(Reshaped) + Send>, Receiver<Reshaped>) {
        let (said, outcome) = mpsc::channel();
        let done = move |reshaped| {
            // Whoever asked may have stopped looking.
            let _ = said.send(reshaped);
        };
        (Box::new(done), outcome)
    }

    /// Steps `runner` until the merge or the split it was told to make has an outcome.
    fn reshaped(runner: &mut RegionRunner, outcome: &Receiver<Reshaped>) -> Reshaped {
        for _ in 0..20_000 {
            if let Ok(outcome) = outcome.try_recv() {
                return outcome;
            }
            runner.step();
            thread::sleep(Duration::from_millis(1));
        }
        panic!("the merge or the split had no outcome");
    }

    /// Steps `runner` until it is at `stage` of what it was told to do.
    fn step_to(runner: &mut RegionRunner, stage: Stage) {
        for _ in 0..20_000 {
            if runner.stage() == Some(stage) {
                return;
            }
            runner.step();
            thread::sleep(Duration::from_millis(1));
        }
        panic!("the runner never came to {stage:?}");
    }

    /// What the runner asked of the store since this was last looked at, without the
    /// commits of its ticks.
    fn asked_beside_commits(gate: &GateControl) -> Vec<Asked> {
        let asked = gate.asked().into_iter();
        asked
            .filter(|asked| !matches!(asked, Asked::Commit(_)))
            .collect()
    }

    /// A gate before `inner`, for a runner that is not made by opening a region.
    fn gate_before(inner: StoreHandle) -> (Box<dyn RegionStore>, Arc<GateControl>) {
        let control = Arc::new(GateControl::default());
        let gate = Gate {
            inner,
            control: Arc::clone(&control),
        };
        (Box::new(gate), control)
    }

    /// The eastern stripe as a test has it that plays the worker that is told to
    /// absorb it: opened with the epoch 2, read, and kept open.
    struct East {
        /// Kept open until the merge has an outcome: the store declines a merge whose
        /// absorbed region has no owner with the epoch named.
        handle: StoreHandle,
        state: RegionState,
        /// An edge that only this region knew, which has [`third_player`] here.
        stranger: EdgeId,
    }

    impl East {
        fn absorb(&self) -> Reshape {
            Reshape::Absorb {
                absorbed: EAST,
                absorbed_epoch: 2,
                state: self.state.clone(),
            }
        }
    }

    /// Someone who arrives in [`FAR_EAST`] with `entity`.
    fn arriving_in_the_east(player: PlayerId, entity: i32) -> EdgeToWorker {
        EdgeToWorker::PlayerArrive {
            player,
            transfer: PlayerTransfer {
                pose: Pose::at(Vec3::new(40.5, -60.0, 0.5)),
                ..transfer(EntityId(entity))
            },
        }
    }

    /// Runs the eastern stripe of `world` with a runner of its own, releases it and
    /// opens it again as whoever absorbs it does. While it ran, [`other_player`] of
    /// `edge` arrived there, and something they did to a block west of the line was
    /// passed on, which left an entry in the region's outbox for the edge that nobody
    /// has confirmed; and [`third_player`] arrived through an edge that no other
    /// region knows.
    async fn released_east(world: &Divided, edge: &TestEdge) -> East {
        let mut east = world.runner(EAST, 1);
        let (end, worker_end) = link::in_process(256);
        let there = TestEdge::silent(end, edge.edge, edge.start);
        east.links().attach(worker_end);
        let (stranger, stranger_end) = in_process(256);
        east.links().attach(stranger_end);

        there.send(there.hello(0, &[], &[])).await.unwrap();
        there
            .send(arriving_in_the_east(other_player(), 40))
            .await
            .unwrap();
        let beyond = RemoteAction {
            player: other_player(),
            sequence: 3,
            step: RemoteStep::Break {
                position: BlockPos::new(3, -61, 0),
            },
        };
        there.send(EdgeToWorker::Remote(beyond)).await.unwrap();
        stranger
            .send(arriving_in_the_east(third_player(), 41))
            .await
            .unwrap();
        step_until(&mut east, |runner| {
            runner.region().knowledge(FAR_EAST) == Knowledge::Held
        });
        assert_eq!(east.region().player_count(), 2);
        assert_eq!(outbox(&east, &there), (1, vec![1]));
        east.begin_release();
        assert_eq!(released(&mut east), Ended::Released);

        let (handle, restored) = world.open(EAST, 2);
        let state = absorbable(&handle, restored).unwrap();
        assert_eq!(state, east.region().state());
        East {
            handle,
            state,
            stranger: stranger.edge,
        }
    }

    /// The eastern stripe of `world` as it is when it has never run, opened and read
    /// as whoever absorbs it does.
    fn untouched_east(world: &Divided) -> East {
        let (handle, restored) = world.open(EAST, 2);
        let state = absorbable(&handle, restored).unwrap();
        assert_eq!(state.tick, 0);
        East {
            handle,
            state,
            stranger: EdgeId::from_name("nobody"),
        }
    }

    /// A link of the edge of `before` that says hello to `runner` as an edge does that
    /// had a link to the region before a merge or a split closed it: it names
    /// `players` and `chunks`, and has seen nothing of the outbox.
    async fn linked_again(
        runner: &RegionRunner,
        before: &TestEdge,
        players: &[PlayerId],
        chunks: &[ChunkPos],
    ) -> TestEdge {
        let (end, worker_end) = link::in_process(256);
        let again = before.again(end, runner);
        runner.links().attach(worker_end);
        again.send(again.hello(0, players, chunks)).await.unwrap();
        again
    }

    /// The entries of the outbox among `told`, with their numbers.
    fn entries(told: &[WorkerToEdge]) -> Vec<(u64, Durable)> {
        let entry = |message: &WorkerToEdge| match message {
            WorkerToEdge::Outbox { number, entry } => Some((*number, entry.clone())),
            _ => None,
        };
        told.iter().filter_map(entry).collect()
    }

    /// The chunks `told` has snapshots of, in the order they came.
    fn shown(told: &[WorkerToEdge]) -> Vec<ChunkPos> {
        let position = |message: &WorkerToEdge| match message {
            WorkerToEdge::ChunkSnapshot { position, .. } => Some(*position),
            _ => None,
        };
        told.iter().filter_map(position).collect()
    }

    /// A merge is one tick of the survivor in which nothing else happens, and it is
    /// taken when the store has it. Then the survivor has the players and the entries
    /// of both regions and is pinned to both areas, has closed its links without a
    /// word, and begins as a restored region does: it believes nothing of any chunk,
    /// and answers the next hello with what the merge made among the welcome's
    /// entries and with every stay it has for the edge. The chunks it had loaded are
    /// served again without the store, and the resume is three ticks.
    #[tokio::test]
    async fn a_region_that_absorbs_another_closes_its_links_and_begins_anew_with_both_states() {
        for on_disk_too in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let world = match on_disk_too {
                true => Divided::stripes_in(directory.path()),
                false => Divided::stripes(),
            };
            let (mut edge, worker_end) = in_process(256);
            let (mut west, gate) = world.gated(RegionId(0), config(0));
            west.links().attach(worker_end);
            look_east(&mut west, &mut edge);
            joined(&edge, &mut west).await;
            let east = released_east(&world, &edge).await;
            step(&mut west);
            edge.everything();
            gate.asked();
            let before = west.region().state();
            let chunk = west.region().chunk(ORIGIN).unwrap().clone();
            assert_eq!(west.region().knowledge(BESIDE), Knowledge::Foreign(EAST));

            let (done, outcome) = outcome();
            west.reshape(east.absorb(), done);
            assert_eq!(west.stage(), Some(Stage::Preparing));
            let absorbed = Reshaped::Absorbed { absorbed: EAST };
            assert_eq!(reshaped(&mut west, &outcome), absorbed);
            assert_eq!((west.stage(), west.ended()), (None, None));
            drop(east.handle);

            // What it cost at the store: a checkpoint while the region ticks, one when
            // it has stopped, and the merge as the tick after its last.
            let merged = west.region().state();
            let tick = merged.tick;
            assert_eq!(
                asked_beside_commits(&gate),
                [
                    Asked::Checkpoint(before.tick),
                    Asked::Flush,
                    Asked::Checkpoint(tick - 1),
                    Asked::Flush,
                    Asked::Absorb(tick),
                ]
            );
            assert_eq!(west.status().tick.load(Ordering::Relaxed), tick);
            assert_eq!(west.status().players.load(Ordering::Relaxed), 3);
            assert_eq!(west.status().chunks.load(Ordering::Relaxed), 0);

            // Both regions' players, and the store's list has one region.
            let players: Vec<_> = merged.players.keys().copied().collect();
            assert_eq!(players, [player(), other_player(), third_player()]);
            let list = world.store.regions().unwrap();
            assert_eq!(list.absorbed, [(EAST, RegionId(0))]);
            assert_eq!(list.regions.len(), 1);
            assert_eq!(list.regions[0].pinned.len(), 2);

            // The links are closed, and nothing was said on them of the merge.
            assert_eq!(edge.everything(), []);
            assert!(closed(&mut edge).await);
            assert!(west.links.is_empty());
            // As after a restore: nothing is loaded, nothing believed, and the areas
            // that came with the merge are the region's.
            assert_eq!(west.region().loaded_chunk_count(), 0);
            assert_eq!(west.region().knowledge(BESIDE), Knowledge::Unknown);
            assert!(west.region().pins(BESIDE));
            assert_eq!(west.region().knowledge(ORIGIN), Knowledge::Held);
            assert_eq!(west.committed, tick);
            assert!(west.pending.is_empty() && west.unsaved.is_empty() && west.loads.is_empty());

            // The edge is back, names what it had here, and sends what its player did
            // meanwhile behind its hello. Three ticks: the hello and the tickets, the
            // chunks from memory and their snapshots, and what was held.
            let mut again = linked_again(&west, &edge, &[player()], &[ORIGIN]).await;
            again.send(walk(player(), 3.0)).await.unwrap();
            for _ in 0..3 {
                west.step();
            }
            assert_eq!(west.region().tick_number(), tick + 3);
            assert_eq!(x_of(&west, player()), Some(3.0));
            // The store is asked for no chunk the region had loaded. What it is asked
            // is whose the chunk is that the absorbed region's players stand in: of
            // an area that came with the merge the store names no chunks, and the
            // region claims each as it is wanted.
            assert_eq!(asked_beside_commits(&gate), [Asked::Claim(vec![FAR_EAST])]);
            settle(&mut west);

            let told = again.everything();
            let welcome = Welcome::Resumed {
                entries: 2,
                presences: 2,
                applied: before.edges[&edge.edge].applied,
            };
            assert_eq!(again.welcomed, Some(welcome));
            let theirs = &east.state.edges[&edge.edge];
            let entry = Durable::Absorbed {
                region: EAST,
                since: theirs.since,
                applied: theirs.applied,
                numbers: vec![1],
            };
            assert_eq!(theirs.applied, 2);
            assert_eq!(entries(&told), [(1, entry), (2, theirs.outbox[&1].clone())]);
            assert_eq!(
                presences(&told),
                [
                    (player(), Some(EntityId(1))),
                    (other_player(), Some(EntityId(40)))
                ]
            );
            let as_it_was = |message: &WorkerToEdge| matches!(message, WorkerToEdge::ChunkSnapshot { chunk: shown, .. } if *shown == chunk);
            assert_eq!(shown(&told), [ORIGIN]);
            assert!(told.iter().any(as_it_was));

            // An edge that only the absorbed region knew is told since when this one
            // knows it, which is the tick of the merge, and whom it has here.
            let (end, worker_end) = link::in_process(256);
            let mut stranger = TestEdge::silent(end, east.stranger, 1);
            west.links().attach(worker_end);
            stranger.send(stranger.hello(0, &[], &[])).await.unwrap();
            step(&mut west);
            let told = stranger.everything();
            let welcome = Welcome::Unknown {
                since: tick,
                entries: 1,
                presences: 1,
                applied: 0,
            };
            assert_eq!(stranger.welcomed, Some(welcome));
            let theirs = &east.state.edges[&east.stranger];
            let entry = Durable::Absorbed {
                region: EAST,
                since: theirs.since,
                applied: 1,
                numbers: vec![],
            };
            assert_eq!(entries(&told), [(1, entry)]);
            assert_eq!(presences(&told), [(third_player(), Some(EntityId(41)))]);

            // Whoever opens the region next finds the merged state as of that tick.
            settle(&mut west);
            let now = west.region().state();
            let (_handle, restored) = world.open(RegionId(0), 2);
            let stored = restored.state.clone().unwrap();
            assert_eq!(stored.tick, tick);
            let as_of_the_merge = Restored {
                deltas: Vec::new(),
                ..restored.clone()
            };
            assert_eq!(restored_state(as_of_the_merge).unwrap(), merged);
            assert_eq!(restored.pinned.len(), 2);
            // A tick that changed nothing has no commit, and is not counted by the store.
            let mut stored = restored_state(restored).unwrap();
            assert!(tick < stored.tick && stored.tick <= now.tick);
            stored.tick = now.tick;
            assert_eq!(stored, now);
        }
    }

    /// The chunks a region has loaded at a merge are kept in memory, and handed to the
    /// tick that asks storage for them. One that the region gives back is forgotten:
    /// another region can hold and change it, so when the region is granted it again
    /// it asks the store.
    #[tokio::test]
    async fn a_warm_chunk_is_served_from_memory_and_forgotten_when_the_region_gives_it_back() {
        let world = Divided::gap();
        let (mut edge, worker_end) = in_process(256);
        let (mut runner, gate) = world.gated(HOME, config(0));
        runner.links().attach(worker_end);
        joined(&edge, &mut runner).await;
        edge.everything();
        edge.send(asking_for(vec![BESIDE])).await.unwrap();
        assert_eq!(
            answers(&mut runner, &mut edge, 1),
            [(BESIDE, edge.asked(), Told::Snapshot)]
        );
        let east = untouched_east(&world);

        let (done, outcome) = outcome();
        runner.reshape(east.absorb(), done);
        let absorbed = Reshaped::Absorbed { absorbed: EAST };
        assert_eq!(reshaped(&mut runner, &outcome), absorbed);
        let warm: Vec<_> = runner.warm.keys().copied().collect();
        assert_eq!(warm, [ORIGIN, BESIDE]);
        assert!(runner.region().pins(ChunkPos::new(16, 0)));
        gate.asked();

        // The edge is back for the home chunk alone. Nothing uses the other, which
        // the region gives back, as nothing keeps it.
        let mut again = linked_again(&runner, &edge, &[player()], &[ORIGIN]).await;
        assert_eq!(
            answers(&mut runner, &mut again, 1),
            [(ORIGIN, 0, Told::Snapshot)]
        );
        step_until(&mut runner, |runner| {
            runner.region().knowledge(BESIDE) == Knowledge::Unknown
        });
        assert!(runner.warm.is_empty());
        let loads = |asked: Vec<Asked>| -> Vec<Asked> {
            let loads = asked.into_iter();
            loads
                .filter(|asked| matches!(asked, Asked::Load(_)))
                .collect()
        };
        assert_eq!(loads(gate.asked()), []);

        again.send(asking_for(vec![BESIDE])).await.unwrap();
        assert_eq!(
            answers(&mut runner, &mut again, 1),
            [(BESIDE, again.asked(), Told::Snapshot)]
        );
        assert_eq!(loads(gate.asked()), [Asked::Load(BESIDE)]);
    }

    /// When the store has answered the flush behind the second checkpoint, it has
    /// answered every load asked before. The runner looks all the same, and hands the
    /// store no merge while a chunk is on its way to it; the chunk that then comes is
    /// kept like those that were loaded.
    #[tokio::test]
    async fn a_merge_is_not_handed_to_the_store_while_a_chunk_is_being_loaded() {
        let world = Divided::stripes();
        let (edge, worker_end) = in_process(256);
        let (mut west, gate) = world.gated(RegionId(0), config(0));
        west.links().attach(worker_end);
        joined(&edge, &mut west).await;
        let east = untouched_east(&world);

        gate.hold_loads();
        edge.send(asking_for(vec![WEST_OF_HOME])).await.unwrap();
        step_until(&mut west, |runner| runner.loads.contains_key(&WEST_OF_HOME));
        wait_for_kept_answers(&mut west, &gate, GateControl::kept_loads, 1);
        gate.asked();

        let (done, outcome) = outcome();
        west.reshape(east.absorb(), done);
        step_to(&mut west, Stage::Closing);
        step_until(&mut west, |runner| {
            runner.flushes_answered == runner.flushes_asked
        });
        for _ in 0..20 {
            west.step();
        }
        assert_eq!(west.stage(), Some(Stage::Closing));
        assert!(outcome.try_recv().is_err());
        let asked = gate.asked();
        assert!(
            !asked.iter().any(|asked| matches!(asked, Asked::Absorb(_))),
            "{asked:?}"
        );

        gate.release_loads();
        let absorbed = Reshaped::Absorbed { absorbed: EAST };
        assert_eq!(reshaped(&mut west, &outcome), absorbed);
        let warm: Vec<_> = west.warm.keys().copied().collect();
        assert_eq!(warm, [WEST_OF_HOME, ORIGIN]);
        gate.asked();

        // Both are shown to the edge that is back without the store being asked.
        let chunks = [ORIGIN, WEST_OF_HOME];
        let mut again = linked_again(&west, &edge, &[player()], &chunks).await;
        assert_eq!(
            answers(&mut west, &mut again, 2),
            [
                (WEST_OF_HOME, 0, Told::Snapshot),
                (ORIGIN, 0, Told::Snapshot)
            ]
        );
        assert_eq!(asked_beside_commits(&gate), []);
    }

    /// A runner that is stopped while the store has the merge and has not answered
    /// lets go of the region as it is. It has said nothing of the merge to anyone, and
    /// says of its outcome that the store knows: here the store did it, and whoever
    /// opens the region next finds it merged.
    #[tokio::test]
    async fn a_runner_stopped_while_the_store_has_a_merge_is_abandoned_and_has_said_nothing() {
        let world = Divided::stripes();
        let (mut edge, worker_end) = in_process(256);
        let (mut west, gate) = world.gated(RegionId(0), config(0));
        west.links().attach(worker_end);
        joined(&edge, &mut west).await;
        let east = released_east(&world, &edge).await;
        step(&mut west);
        edge.everything();

        gate.hold_reshapes();
        let (done, outcome) = outcome();
        west.reshape(east.absorb(), done);
        step_to(&mut west, Stage::Committing);
        let tick = west.region().tick_number();
        wait_for_kept_answers(&mut west, &gate, GateControl::kept_reshapes, 1);
        for _ in 0..5 {
            west.step();
        }
        assert_eq!(west.stage(), Some(Stage::Committing));
        assert!(outcome.try_recv().is_err());

        assert_eq!(west.run(&AtomicBool::new(true)), Ended::Abandoned);
        assert_eq!(west.ended(), Some(Ended::Abandoned));
        let off = Reshaped::Off {
            why: Off::StoreLost,
        };
        assert_eq!(outcome.try_recv(), Ok(off));
        assert_eq!(west.region().tick_number(), tick);
        assert_eq!(edge.everything(), []);
        assert!(closed(&mut edge).await);

        let list = world.store.regions().unwrap();
        assert_eq!(list.absorbed, [(EAST, RegionId(0))]);
        let (_handle, restored) = world.open(RegionId(0), 2);
        assert_eq!(restored.deltas, []);
        let state = restored_state(restored).unwrap();
        assert_eq!(state.tick, tick + 1);
        assert_eq!(state.players.len(), 3);
    }

    /// A region that has stopped ticking for a merge, with two things taken from links
    /// that no tick has taken: a step of [`player`] that came when the region was as
    /// far ahead of the store as it may be, and the join of [`other_player`], which
    /// waited behind the hello of a second edge until the region's last tick had shown
    /// that edge its chunk.
    struct Stalled {
        west: RegionRunner,
        gate: Arc<GateControl>,
        edge: TestEdge,
        second: TestEdge,
        /// The number of the step, and the step.
        late: (u64, EdgeToWorker),
        /// The region it was told to absorb, which is kept open.
        east: East,
        outcome: Receiver<Reshaped>,
        /// The number of the region's last tick.
        tick: u64,
    }

    /// Tells the western stripe of `world` to absorb the eastern one, as opened with
    /// `absorbed_epoch`, which is 2 if the store is to do it, and stops it so.
    async fn stalled(world: &Divided, absorbed_epoch: u64) -> Stalled {
        let (edge, worker_end) = in_process(256);
        let (mut west, gate) = world.gated(RegionId(0), config(0));
        west.links().attach(worker_end);
        joined(&edge, &mut west).await;
        step(&mut west);
        let east = untouched_east(world);
        let reshape = Reshape::Absorb {
            absorbed: EAST,
            absorbed_epoch,
            state: east.state.clone(),
        };

        // Eight ticks that the store does not confirm, the last of which takes the
        // second edge's hello and, as the chunk is loaded, answers it at once.
        gate.hold();
        gate.hold_flushes();
        for x in 1..MAX_TICKS_AHEAD {
            edge.send(walk(player(), x as f64)).await.unwrap();
            west.step();
        }
        let (end, worker_end) = link::in_process(256);
        let second = TestEdge::silent(end, EdgeId::from_name("second"), 1);
        west.links().attach(worker_end);
        second.send(second.hello(0, &[], &[ORIGIN])).await.unwrap();
        second.send(join(other_player(), "Jeb")).await.unwrap();
        edge.send(walk(player(), 8.0)).await.unwrap();
        west.step();
        assert_eq!(west.pending.len(), MAX_TICKS_AHEAD);
        let tick = west.region().tick_number();
        assert!(west.links.values().all(|link| link.held.is_empty()));
        assert_eq!(west.edges[&second.edge].received, 1);
        assert_eq!(west.region().edge(second.edge).unwrap().applied, 0);

        let (done, outcome) = outcome();
        west.reshape(reshape, done);
        let late = walk(player(), 20.0);
        edge.send(late.clone()).await.unwrap();
        let number = edge.sent.load(Ordering::Relaxed);
        // This step takes it from the link, and does not tick.
        west.step();
        assert_eq!(west.region().tick_number(), tick);
        assert_eq!(west.edges[&edge.edge].received, number);
        assert_eq!(west.region().edge(edge.edge).unwrap().applied, number - 1);

        wait_for_kept_flush(&mut west, &gate);
        gate.release_flushes();
        step_to(&mut west, Stage::Settling);
        assert_eq!(west.region().tick_number(), tick);
        gate.release();
        Stalled {
            west,
            gate,
            edge,
            second,
            late: (number, late),
            east,
            outcome,
            tick,
        }
    }

    /// What a link sent and no tick took when the region stopped is not in the state
    /// the merge is made of. With the tick of the merge it is dropped and not counted
    /// as received, so that its edge, which was not told that it was applied, sends
    /// it again on its next link, where it is applied once.
    #[tokio::test]
    async fn what_no_tick_took_before_a_merge_is_not_counted_and_is_applied_when_sent_again() {
        let world = Divided::stripes();
        let mut stalled = stalled(&world, 2).await;
        let west = &mut stalled.west;
        let absorbed = Reshaped::Absorbed { absorbed: EAST };
        assert_eq!(reshaped(west, &stalled.outcome), absorbed);
        assert_eq!(west.region().tick_number(), stalled.tick + 1);
        // The absorbed region is no more, and its handle with it.
        assert!(stalled.east.handle.is_lost());
        let (number, late) = stalled.late;

        assert_eq!(x_of(west, player()), Some(8.0));
        assert_eq!(west.region().player_count(), 1);
        let known = &west.edges[&stalled.edge.edge];
        assert_eq!((known.received, known.applied), (number - 1, number - 1));
        assert_eq!(west.edges[&stalled.second.edge].received, 0);
        assert_eq!(west.inputs, TickInputs::default());

        // The welcome says how far the region had got, and each edge sends again what
        // is beyond that.
        let mut again = linked_again(west, &stalled.edge, &[player()], &[ORIGIN]).await;
        again.send_as(number, late).await;
        let mut second = linked_again(west, &stalled.second, &[], &[ORIGIN]).await;
        second.send_as(1, join(other_player(), "Jeb")).await;
        step_until(west, |runner| {
            x_of(runner, player()) == Some(20.0) && runner.region().player_count() == 2
        });
        settle(west);
        assert_eq!(west.region().edge(again.edge).unwrap().applied, number);
        assert_eq!(west.region().edge(second.edge).unwrap().applied, 1);
        again.everything();
        second.everything();
        let applied = |welcome: Option<Welcome>| match welcome {
            Some(Welcome::Resumed { applied, .. }) => applied,
            other => panic!("welcomed {other:?}"),
        };
        assert_eq!(applied(again.welcomed), number - 1);
        assert_eq!(applied(second.welcomed), 0);
    }

    /// If the store declines the merge, what a link sent while the region stood still
    /// is where it was, and the region's next tick takes it. That tick is the one a
    /// merge would have been: no tick number was used, and no link was closed or told
    /// anything.
    #[tokio::test]
    async fn after_a_merge_that_is_declined_the_region_ticks_on_with_what_its_links_sent() {
        let world = Divided::stripes();
        // Not the epoch the region to absorb is open with.
        let mut stalled = stalled(&world, 3).await;
        let west = &mut stalled.west;
        let declined = Decline::NotOpened { epoch: Some(2) };
        let off = Reshaped::Off {
            why: Off::Declined(declined),
        };
        assert_eq!(reshaped(west, &stalled.outcome), off);
        assert_eq!((west.stage(), west.ended()), (None, None));
        assert_eq!(west.region().tick_number(), stalled.tick);
        assert_eq!(west.links.len(), 2);
        assert_eq!(world.store.regions().unwrap().absorbed, []);
        assert!(!stalled.east.handle.is_lost());

        step(west);
        assert_eq!(west.region().tick_number(), stalled.tick + 1);
        assert_eq!(x_of(west, player()), Some(20.0));
        assert_eq!(west.region().player_count(), 2);
        assert_eq!(stalled.gate.commits().last(), Some(&(stalled.tick + 1)));
        // What the edges were told is what ticks tell: no entry of a merge.
        let told = stalled.edge.everything();
        assert_eq!(entries(&told), []);
        assert_eq!(moves(&told).last(), Some(&20.0));
        assert!(stalled.second.everything().contains(&UNKNOWN));
    }

    /// Two players of one edge in the western stripe: [`player`] where players enter
    /// the world and [`other_player`] in [`FAR_WEST`]. The edge is subscribed to the
    /// chunks they stand in and to the one between, and all three are loaded.
    async fn two_players_apart(runner: &mut RegionRunner, edge: &mut TestEdge) {
        edge.send(join(player(), "Notch")).await.unwrap();
        edge.send(join(other_player(), "Jeb")).await.unwrap();
        edge.send(asking_for(vec![ORIGIN, WEST_OF_HOME, FAR_WEST]))
            .await
            .unwrap();
        edge.send(walk(other_player(), -20.5)).await.unwrap();
        step_until(runner, |runner| {
            runner.region().loaded_chunk_count() == 3 && x_of(runner, other_player()) == Some(-20.5)
        });
        step(runner);
        edge.everything();
    }

    /// A split of `chunks`, with the id the store of two stripes gives out next.
    fn split_off(chunks: &[ChunkPos]) -> Reshape {
        Reshape::SplitOff {
            chunks: chunks.to_vec(),
            as_epoch: 5,
            part: RegionId(2),
        }
    }

    /// A split is one tick as well. The players standing in the chunks named go, with
    /// the chunks nearer to them than to anyone who stays; the region tells their edge
    /// so in its outbox, closes its links, and begins anew without the part. The part
    /// is a region as any worker would restore it from the store's record, and a
    /// runner made of it serves its chunks from memory.
    #[tokio::test]
    async fn a_split_makes_a_region_of_the_players_in_the_chunks_named_and_both_begin_anew() {
        for on_disk_too in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let world = match on_disk_too {
                true => Divided::stripes_in(directory.path()),
                false => Divided::stripes(),
            };
            let (mut edge, worker_end) = in_process(256);
            let (mut west, gate) = world.gated(RegionId(0), config(0));
            west.links().attach(worker_end);
            two_players_apart(&mut west, &mut edge).await;
            gate.asked();
            let before = west.region().state();
            let far = west.region().chunk(FAR_WEST).unwrap().clone();

            let (done, outcome) = outcome();
            west.reshape(split_off(&[FAR_WEST]), done);
            let Reshaped::Split {
                region: new,
                as_epoch,
                part,
            } = reshaped(&mut west, &outcome)
            else {
                panic!("no split");
            };
            assert_eq!((new, as_epoch), (RegionId(2), 5));
            let tick = west.region().tick_number();
            assert_eq!(
                asked_beside_commits(&gate),
                [
                    Asked::Checkpoint(before.tick),
                    Asked::Flush,
                    Asked::Checkpoint(tick - 1),
                    Asked::Flush,
                    Asked::Split(tick, new),
                ]
            );

            // The part: the player who stood there, the chunk, as it was.
            assert_eq!(part.chunks, [(FAR_WEST, far)]);
            assert_eq!(part.region.tick_number(), tick);
            assert_eq!(part.region.player_count(), 1);
            assert_eq!(part.region.player(other_player()).unwrap().0, EntityId(2));
            assert_eq!(part.region.knowledge(FAR_WEST), Knowledge::Held);
            assert_eq!(part.region.knowledge(WEST_OF_HOME), Knowledge::Unknown);
            // The region: without them, and knowing nothing of the chunk.
            assert_eq!(west.region().player_count(), 1);
            assert_eq!(west.region().knowledge(FAR_WEST), Knowledge::Unknown);
            assert_eq!(west.region().knowledge(WEST_OF_HOME), Knowledge::Held);
            assert_eq!(west.region().loaded_chunk_count(), 0);
            let warm: Vec<_> = west.warm.keys().copied().collect();
            assert_eq!(warm, [WEST_OF_HOME, ORIGIN]);
            assert_eq!(west.status().players.load(Ordering::Relaxed), 1);
            assert_eq!(edge.everything(), []);
            assert!(closed(&mut edge).await);

            // The store has the new region, to be opened with the epoch named.
            let list = world.store.regions().unwrap();
            let listed: Vec<_> = list
                .regions
                .iter()
                .map(|info| (info.region, info.epoch))
                .collect();
            assert_eq!(listed, [(RegionId(0), 1), (EAST, 0), (new, 5)]);
            let (handle, restored) = world.store.open_region(world.hello(new, 5)).unwrap();
            let holdings = holdings(&restored);
            let state = restored_state(restored).unwrap();
            assert_eq!(Region::restore(config(0), state, holdings), part.region);

            // The edge is back at the region that was split, with what it had there.
            let both = [player(), other_player()];
            let chunks = [ORIGIN, WEST_OF_HOME, FAR_WEST];
            let mut again = linked_again(&west, &edge, &both, &chunks).await;
            again.send(walk(player(), 3.0)).await.unwrap();
            assert_eq!(
                answers(&mut west, &mut again, 3),
                [
                    (FAR_WEST, 0, Told::Elsewhere(new)),
                    (WEST_OF_HOME, 0, Told::Snapshot),
                    (ORIGIN, 0, Told::Snapshot),
                ]
            );
            step_until(&mut west, |runner| x_of(runner, player()) == Some(3.0));
            let welcome = Welcome::Resumed {
                entries: 1,
                presences: 2,
                applied: before.edges[&edge.edge].applied,
            };
            assert_eq!(again.welcomed, Some(welcome));
            let went = Durable::SplitOff {
                region: new,
                players: vec![(other_player(), EntityId(2))],
            };
            let state = west.region().edge(edge.edge).unwrap();
            assert_eq!(state.outbox.values().collect::<Vec<_>>(), [&went]);
            assert_eq!(
                presences(&again.aside),
                [(player(), Some(EntityId(1))), (other_player(), None)]
            );
            let loads =
                |asked: Vec<Asked>| asked.iter().any(|asked| matches!(asked, Asked::Load(_)));
            assert!(!loads(gate.asked()));

            // And at the part, which it has never said anything to: it is told since
            // when the part knows it and whom the part has, and shown the chunk without
            // the store being asked for it. Three ticks again.
            let (store, gate) = gate_before(handle);
            let mut part = RegionRunner::of_part_with(part, store);
            let (end, worker_end) = link::in_process(256);
            let mut there = TestEdge::silent(end, edge.edge, edge.start);
            part.links().attach(worker_end);
            there.send(there.hello(0, &[], &[FAR_WEST])).await.unwrap();
            there.send(walk(other_player(), -22.5)).await.unwrap();
            for _ in 0..3 {
                part.step();
            }
            assert_eq!(x_of(&part, other_player()), Some(-22.5));
            settle(&mut part);
            let told = there.everything();
            let welcome = Welcome::Unknown {
                since: tick,
                entries: 0,
                presences: 1,
                applied: 0,
            };
            assert_eq!(there.welcomed, Some(welcome));
            assert_eq!(presences(&told), [(other_player(), Some(EntityId(2)))]);
            assert_eq!(shown(&told), [FAR_WEST]);
            assert_eq!(asked_beside_commits(&gate), []);
        }
    }

    /// The store gives out the ids of regions, and another split can have taken the
    /// one a command names. The runner makes the split again with the id the store
    /// says is next, and what it tells the edge names the region the store has.
    #[tokio::test]
    async fn a_split_that_names_another_id_than_the_next_is_made_with_the_next() {
        let world = Divided::stripes();
        let (mut edge, worker_end) = in_process(256);
        let (mut west, gate) = world.gated(RegionId(0), config(0));
        west.links().attach(worker_end);
        two_players_apart(&mut west, &mut edge).await;
        gate.asked();

        let (done, outcome) = outcome();
        let reshape = Reshape::SplitOff {
            chunks: vec![FAR_WEST],
            as_epoch: 5,
            part: RegionId(9),
        };
        west.reshape(reshape, done);
        let Reshaped::Split { region, part, .. } = reshaped(&mut west, &outcome) else {
            panic!("no split");
        };
        assert_eq!(region, RegionId(2));
        assert_eq!(part.region.player_count(), 1);
        let tick = west.region().tick_number();
        let asked = gate.asked();
        let splits: Vec<_> = asked
            .iter()
            .filter(|asked| matches!(asked, Asked::Split(..)))
            .collect();
        assert_eq!(
            splits,
            [
                &Asked::Split(tick, RegionId(9)),
                &Asked::Split(tick, RegionId(2))
            ]
        );
        let state = west.region().edge(edge.edge).unwrap();
        let went = Durable::SplitOff {
            region: RegionId(2),
            players: vec![(other_player(), EntityId(2))],
        };
        assert_eq!(state.outbox.values().collect::<Vec<_>>(), [&went]);
        let list = world.store.regions().unwrap();
        assert_eq!(list.regions.len(), 3);
        assert_eq!(list.next, RegionId(3));
    }

    /// Nothing comes of a split that nobody stands in the chunks of, nor of a merge or
    /// a split that is too large to hand to the store. The store is not asked, the
    /// region ticks on from its last tick, and its link stays and is told nothing.
    #[tokio::test]
    async fn a_split_of_nobody_and_one_that_is_too_large_leave_the_region_as_it_was() {
        let world = Divided::stripes();
        let (mut edge, worker_end) = in_process(256);
        let (mut west, gate) = world.gated(RegionId(0), config(0));
        west.links().attach(worker_end);
        two_players_apart(&mut west, &mut edge).await;
        let east = untouched_east(&world);
        let list = world.store.regions().unwrap();

        let attempts = [
            (
                split_off(&[WEST_OF_HOME, BESIDE]),
                MAX_RESHAPE_BYTES,
                Off::Nobody,
            ),
            // The home chunk never leaves the region that is joined.
            (split_off(&[ORIGIN]), MAX_RESHAPE_BYTES, Off::Nobody),
            (split_off(&[FAR_WEST]), 64, Off::TooLarge),
            (east.absorb(), 64, Off::TooLarge),
        ];
        let mut x = 3.0;
        for (reshape, limit, why) in attempts {
            gate.asked();
            west.max_reshape_bytes = limit;
            let (done, outcome) = outcome();
            west.reshape(reshape, done);
            step_to(&mut west, Stage::Settling);
            let tick = west.region().tick_number();
            // Sent while the region stands still.
            x += 1.0;
            edge.send(walk(player(), x)).await.unwrap();
            assert_eq!(reshaped(&mut west, &outcome), Reshaped::Off { why });
            assert_eq!((west.stage(), west.ended()), (None, None));
            let asked = gate.asked();
            assert!(
                !asked
                    .iter()
                    .any(|asked| matches!(asked, Asked::Absorb(_) | Asked::Split(..))),
                "{asked:?}"
            );
            assert_eq!(west.region().tick_number(), tick);
            assert_eq!(west.links.len(), 1);
            assert_eq!(edge.everything(), []);

            step(&mut west);
            assert_eq!(west.region().tick_number(), tick + 1);
            assert_eq!(x_of(&west, player()), Some(x));
            assert_eq!(moves(&edge.everything()), [x]);
        }
        assert_eq!(world.store.regions().unwrap(), list);
    }

    /// One thing at a time. A runner that is in the middle of a merge says of a second
    /// one that it is busy, at once, and so does one that is releasing its region or
    /// has ended. A release that is asked for during a merge waits for it, and the
    /// next owner is restored with the merged state. A checkpoint to prepare a merge
    /// is made only by a runner that does nothing else, and has no outcome.
    #[tokio::test]
    async fn a_runner_does_one_thing_at_a_time_and_a_release_waits_for_a_merge() {
        let world = Divided::stripes();
        let (edge, worker_end) = in_process(256);
        let (mut west, gate) = world.gated(RegionId(0), config(0));
        west.links().attach(worker_end);
        joined(&edge, &mut west).await;
        let east = untouched_east(&world);
        let busy = Reshaped::Off { why: Off::Busy };
        let never = || -> Box<dyn FnOnce(Reshaped) + Send> {
            Box::new(|outcome: Reshaped| panic!("a checkpoint has an outcome: {outcome:?}"))
        };

        gate.asked();
        west.reshape(Reshape::Prepare, never());
        let tick = west.region().tick_number();
        assert_eq!(asked_beside_commits(&gate), [Asked::Checkpoint(tick)]);
        assert_eq!(west.stage(), None);

        let (done, outcome) = outcome();
        west.reshape(east.absorb(), done);
        let (second_done, second) = self::outcome();
        west.reshape(split_off(&[ORIGIN]), second_done);
        assert_eq!(second.try_recv(), Ok(busy.clone()));
        gate.asked();
        west.reshape(Reshape::Prepare, never());
        assert_eq!(asked_beside_commits(&gate), []);
        west.begin_release();
        assert_eq!(west.stage(), Some(Stage::Preparing));

        let absorbed = Reshaped::Absorbed { absorbed: EAST };
        assert_eq!(reshaped(&mut west, &outcome), absorbed);
        assert_eq!((west.stage(), west.ended()), (None, None));
        let merged = west.region().state();

        // Asked again, as `run` does before every step.
        west.begin_release();
        assert_eq!(west.stage(), Some(Stage::Preparing));
        let (done, during_release) = self::outcome();
        west.reshape(split_off(&[ORIGIN]), done);
        assert_eq!(during_release.try_recv(), Ok(busy.clone()));
        assert_eq!(released(&mut west), Ended::Released);
        let (done, after) = self::outcome();
        west.reshape(split_off(&[ORIGIN]), done);
        assert_eq!(after.try_recv(), Ok(busy));

        // The ticks that ran while the release was prepared changed nothing but their
        // number.
        let (_handle, restored) = world.open(RegionId(0), 2);
        assert_eq!(restored.deltas, []);
        let mut state = restored_state(restored).unwrap();
        assert!(state.tick >= merged.tick);
        state.tick = merged.tick;
        assert_eq!(state, merged);
    }

    /// A runner whose store handle is lost in the middle of a merge stops for good, as
    /// it does at any time, and says of the merge that the store knows what came of
    /// it. So does one that is dropped.
    #[tokio::test]
    async fn a_merge_whose_runner_loses_the_store_or_is_dropped_leaves_the_outcome_to_the_store() {
        let lost = Reshaped::Off {
            why: Off::StoreLost,
        };
        for stage in [Stage::Preparing, Stage::Settling, Stage::Closing] {
            let world = Divided::stripes();
            let (mut edge, worker_end) = in_process(256);
            let (mut west, gate) = world.gated(RegionId(0), config(0));
            west.links().attach(worker_end);
            joined(&edge, &mut west).await;
            let east = untouched_east(&world);

            let (done, outcome) = outcome();
            west.reshape(east.absorb(), done);
            step_to(&mut west, stage);
            gate.lose();
            west.step();
            assert_eq!(outcome.try_recv(), Ok(lost.clone()));
            assert_eq!(west.ended(), Some(Ended::StoreLost));
            assert_eq!(west.stage(), None);
            assert!(closed(&mut edge).await);
            assert_eq!(world.store.regions().unwrap().absorbed, []);

            let again = world.hello(RegionId(0), 2);
            let (mut west, _) = gated_as(&world.store, again, config(0));
            let (done, outcome) = self::outcome();
            west.reshape(east.absorb(), done);
            step_to(&mut west, stage);
            drop(west);
            assert_eq!(outcome.try_recv(), Ok(lost.clone()));
        }
    }

    /// Chunks that nobody has asked for 600 ticks after the merge or the split they
    /// are kept from are forgotten.
    #[tokio::test]
    async fn warm_chunks_are_forgotten_when_nobody_has_asked_for_them_for_600_ticks() {
        let world = Divided::stripes();
        let (edge, worker_end) = in_process(256);
        let (mut west, _) = world.gated(RegionId(0), config(0));
        west.links().attach(worker_end);
        joined(&edge, &mut west).await;
        let east = untouched_east(&world);
        let (done, outcome) = outcome();
        west.reshape(east.absorb(), done);
        let absorbed = Reshaped::Absorbed { absorbed: EAST };
        assert_eq!(reshaped(&mut west, &outcome), absorbed);
        let tick = west.region().tick_number();
        assert_eq!(WARM_FOR, 600);

        step_until(&mut west, |runner| {
            runner.region().tick_number() == tick + WARM_FOR - 1
        });
        let warm: Vec<_> = west.warm.keys().copied().collect();
        assert_eq!(warm, [ORIGIN]);
        step(&mut west);
        assert_eq!(west.region().tick_number(), tick + WARM_FOR);
        assert!(west.warm.is_empty());
        // The chunk is still the region's, and comes from the store when it is asked for.
        assert_eq!(west.region().knowledge(ORIGIN), Knowledge::Held);
    }

    /// A worker takes a merge on the thread of its region and calls with the outcome
    /// from there. One whose thread has ended calls with the word that nothing came of
    /// it.
    #[tokio::test]
    async fn a_worker_reshapes_its_region_on_its_thread_and_says_so_once() {
        let world = Divided::stripes();
        let (edge, worker_end) = in_process(256);
        let west = world.runner(RegionId(0), 1);
        west.links().attach(worker_end);
        let status = west.status();
        let east = untouched_east(&world);
        let worker = Worker::spawn(west);
        edge.send(join(player(), "Notch")).await.unwrap();

        let never =
            Box::new(|outcome: Reshaped| panic!("a checkpoint has an outcome: {outcome:?}"));
        worker.reshape(Reshape::Prepare, never);
        let (done, outcome) = outcome();
        worker.reshape(east.absorb(), done);
        let absorbed = Reshaped::Absorbed { absorbed: EAST };
        let patience = Duration::from_secs(10);
        assert_eq!(outcome.recv_timeout(patience), Ok(absorbed));
        assert_eq!(world.store.regions().unwrap().regions.len(), 1);
        // It ticks on.
        let tick = status.tick.load(Ordering::Relaxed);
        for _ in 0..20_000 {
            if status.tick.load(Ordering::Relaxed) > tick {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(status.tick.load(Ordering::Relaxed) > tick);

        worker.begin_release();
        wait_until_finished(&worker);
        let (done, outcome) = self::outcome();
        worker.reshape(split_off(&[ORIGIN]), done);
        let busy = Reshaped::Off { why: Off::Busy };
        assert_eq!(outcome.recv_timeout(patience), Ok(busy));
        assert_eq!(worker.stop(), Ended::Released);
    }

    /// A region whose owner died instead of releasing it has commits that no checkpoint
    /// covers, and the store declines to have it absorbed as it is. Whoever is told to
    /// absorb it reads its state with [`absorbable`], which has the store keep that
    /// state in place of those commits; then the merge is made.
    #[tokio::test]
    async fn a_region_whose_owner_died_is_checkpointed_before_it_is_absorbed() {
        let world = Divided::stripes();
        let mut east = world.runner(EAST, 1);
        let (there, worker_end) = in_process(256);
        east.links().attach(worker_end);
        there
            .send(arriving_in_the_east(other_player(), 40))
            .await
            .unwrap();
        step_until(&mut east, |runner| runner.region().player_count() == 1);
        settle(&mut east);
        let players = east.region().state().players;
        drop(east);

        let (handle, restored) = world.open(EAST, 2);
        assert!(!restored.deltas.is_empty());
        let (mut west, _) = world.gated(RegionId(0), config(0));
        let absorb = |state: RegionState| Reshape::Absorb {
            absorbed: EAST,
            absorbed_epoch: 2,
            state,
        };

        // As it was read, without a word to the store.
        let unchecked = restored_state(restored.clone()).unwrap();
        let (done, outcome) = outcome();
        west.reshape(absorb(unchecked), done);
        let declined = Decline::Uncheckpointed { region: EAST };
        let off = Reshaped::Off {
            why: Off::Declined(declined),
        };
        assert_eq!(reshaped(&mut west, &outcome), off);

        let state = absorbable(&handle, restored).unwrap();
        assert_eq!(state.players, players);
        let (done, outcome) = self::outcome();
        west.reshape(absorb(state), done);
        let absorbed = Reshaped::Absorbed { absorbed: EAST };
        assert_eq!(reshaped(&mut west, &outcome), absorbed);
        assert_eq!(west.region().state().players, players);
    }

    /// What the store delivers for the coming tick is kept at a merge only if the
    /// region held the chunk by what its ticks were told. A chunk it has given back
    /// since it asked for it, and is granted again by an answer that no tick has
    /// taken, can have been another region's in between: what was read for the first
    /// request is not kept, and the chunk is asked of the store when it is wanted.
    #[tokio::test]
    async fn a_chunk_read_before_the_region_gave_it_back_is_not_kept_when_it_is_granted_again() {
        let world = Divided::gap();
        let (edge, worker_end) = in_process(256);
        let (mut runner, gate) = world.gated(HOME, config(0));
        runner.links().attach(worker_end);
        joined(&edge, &mut runner).await;

        // The chunk is granted and asked of the store, whose answer does not come.
        gate.hold_loads();
        edge.send(asking_for(vec![BESIDE])).await.unwrap();
        step_until(&mut runner, |runner| runner.loads.contains_key(&BESIDE));
        wait_for_kept_answers(&mut runner, &gate, GateControl::kept_loads, 1);
        // The link lets go, and the region gives the chunk back; then the link asks
        // again, and the store's word that the chunk is the region's does not come.
        edge.send(done_with(vec![BESIDE])).await.unwrap();
        step_until(&mut runner, |runner| {
            runner.region().knowledge(BESIDE) == Knowledge::Unknown
        });
        gate.hold_claims();
        edge.send(asking_for(vec![BESIDE])).await.unwrap();
        step_until(&mut runner, |runner| {
            runner.region().knowledge(BESIDE) == Knowledge::Asked
        });
        wait_for_kept_answers(&mut runner, &gate, GateControl::kept_claims, 1);

        let east = untouched_east(&world);
        let (done, outcome) = outcome();
        runner.reshape(east.absorb(), done);
        step_to(&mut runner, Stage::Closing);
        gate.release_loads();
        gate.release_claims();
        let absorbed = Reshaped::Absorbed { absorbed: EAST };
        assert_eq!(reshaped(&mut runner, &outcome), absorbed);
        // Granted by the answer that waited, and not kept from the answer before it.
        assert_eq!(runner.region().knowledge(BESIDE), Knowledge::Held);
        let warm: Vec<_> = runner.warm.keys().copied().collect();
        assert_eq!(warm, [ORIGIN]);
        gate.asked();

        let chunks = [ORIGIN, BESIDE];
        let mut again = linked_again(&runner, &edge, &[player()], &chunks).await;
        assert_eq!(
            answers(&mut runner, &mut again, 2),
            [(ORIGIN, 0, Told::Snapshot), (BESIDE, 0, Told::Snapshot)]
        );
        let asked = gate.asked();
        let loads: Vec<_> = asked
            .iter()
            .filter(|asked| matches!(asked, Asked::Load(_)))
            .collect();
        assert_eq!(loads, [&Asked::Load(BESIDE)]);
    }

    /// A chunk that is still warm when its region is split goes where the split puts
    /// it: a part that is split again before any edge has asked it for anything hands
    /// the second part its chunks, and the runner of that one serves them from memory.
    #[tokio::test]
    async fn a_chunk_that_is_still_warm_goes_with_the_part_when_its_region_is_split_again() {
        let world = Divided::stripes();
        let (mut edge, worker_end) = in_process(256);
        let (mut west, _) = world.gated(RegionId(0), config(0));
        west.links().attach(worker_end);
        two_players_apart(&mut west, &mut edge).await;
        let further = ChunkPos::new(-4, 0);
        edge.send(join(third_player(), "Dinnerbone")).await.unwrap();
        edge.send(asking_for(vec![further])).await.unwrap();
        edge.send(walk(third_player(), -60.5)).await.unwrap();
        step_until(&mut west, |runner| {
            runner.region().chunk(further).is_some() && x_of(runner, third_player()) == Some(-60.5)
        });

        // Two of the three players go, each with the chunk they stand in.
        let (done, outcome) = outcome();
        west.reshape(split_off(&[FAR_WEST, further]), done);
        let Reshaped::Split { region, part, .. } = reshaped(&mut west, &outcome) else {
            panic!("no split");
        };
        let positions = |part: &Part| -> Vec<ChunkPos> {
            let chunks = part.chunks.iter();
            chunks.map(|(position, _)| *position).collect()
        };
        assert_eq!(positions(&part), [further, FAR_WEST]);
        let (handle, _) = world.store.open_region(world.hello(region, 5)).unwrap();
        let mut first = RegionRunner::of_part(part, handle);

        let (done, outcome) = self::outcome();
        let again = Reshape::SplitOff {
            chunks: vec![further],
            as_epoch: 6,
            part: RegionId(3),
        };
        first.reshape(again, done);
        let Reshaped::Split { region, part, .. } = reshaped(&mut first, &outcome) else {
            panic!("no second split");
        };
        assert_eq!(region, RegionId(3));
        assert_eq!(positions(&part), [further]);
        assert_eq!(part.region.player(third_player()).unwrap().0, EntityId(3));
        let warm: Vec<_> = first.warm.keys().copied().collect();
        assert_eq!(warm, [FAR_WEST]);
        assert_eq!(first.region().player_count(), 1);

        let (handle, _) = world.store.open_region(world.hello(region, 6)).unwrap();
        let (store, gate) = gate_before(handle);
        let mut second = RegionRunner::of_part_with(part, store);
        let (end, worker_end) = link::in_process(256);
        let mut there = TestEdge::silent(end, edge.edge, edge.start);
        second.links().attach(worker_end);
        there
            .send(there.hello(0, &[third_player()], &[further]))
            .await
            .unwrap();
        assert_eq!(
            answers(&mut second, &mut there, 1),
            [(further, 0, Told::Snapshot)]
        );
        assert_eq!(asked_beside_commits(&gate), []);
        assert_eq!(
            presences(&there.aside),
            [(third_player(), Some(EntityId(3)))]
        );
    }
}
