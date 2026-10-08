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
//! Edges come and go. Each has one link at a time, which begins with a hello. Players
//! belong to edges, not to links: a link that ends or does not keep up is dropped without
//! the region missing a tick, and its edge's players stay until the edge is back, has
//! started anew or has been away for too long.
//!
//! A region is given to another worker by **releasing** it, which is a crash that the
//! runner prepares: it brings the store up to date, lets go of the region and closes its
//! links, so that the next owner restores it from a state file alone. See
//! `docs/adr/0009-moving-a-region.md`, section 1, and [`RegionRunner::begin_release`].

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::mem;
use std::ops::Bound;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use clustine_rpc::link::WorkerEnd;
use clustine_rpc::{
    EdgeMessage, EdgeToWorker, Presence, Restored, StoreReply, StoreRequest, Welcome, WorkerToEdge,
};
use clustine_sim::api::RegionEvent;
use clustine_sim::{
    Durable, EdgeEvent, PlayerChange, PlayerEvent, Region, RegionConfig, RegionState, StateDelta,
    TickInputs,
};
use clustine_world::{ChunkPos, EdgeId, EntityId, PlayerId};
use clustine_worldstore::StoreHandle;
use tracing::{error, info, warn};

/// The length of a tick: 20 ticks per second.
pub const TICK: Duration = Duration::from_millis(50);

/// Ticks between two checkpoints unless set otherwise: five minutes.
pub const DEFAULT_CHECKPOINT_INTERVAL: u64 = 5 * 60 * 20;

/// Ticks an edge may be without a link before the region forgets it, unless set
/// otherwise: 30 seconds.
pub const DEFAULT_GONE_AFTER: u64 = 30 * 20;

/// How many ticks a region may be ahead of what the world store has confirmed. At that
/// bound it waits.
pub const MAX_TICKS_AHEAD: usize = 8;

/// A runner that has fallen further behind than this many ticks skips them instead of
/// trying to catch up.
const MAX_CATCH_UP_TICKS: u32 = 10;

/// How often a runner that waits for its next tick looks whether the store has confirmed
/// a commit, so that what a tick did reaches players a write later and not a tick later.
const COMMIT_POLL: Duration = Duration::from_millis(1);

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
}

impl RegionStatus {
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
    /// is gone, has ended, or has stopped ticking in order to release its region, the
    /// link is closed instead, which its other end notices.
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
    /// It was asked to stop while it was releasing the region, and let go of the region
    /// without waiting for the store any longer. Nothing was published that the store had
    /// not confirmed, so this is a crash like any other: the next owner restores what
    /// the store has.
    Abandoned,
}

/// How far a runner is with releasing its region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Nobody has asked for a release.
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
    /// The runner has ended, for whichever reason, and does nothing but close the links
    /// it is handed.
    Ended,
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
    /// Whether the region knew the edge with the start of the hello.
    known: bool,
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
    /// The chunks named in the hello that the link has not been made a snapshot of yet.
    /// While there are any, what the link sends is kept in `held`.
    hold: BTreeSet<ChunkPos>,
    /// What the link sent while it was held, in the order it came.
    held: VecDeque<EdgeMessage>,
    /// Chunks the edge is subscribed to.
    subscriptions: BTreeSet<ChunkPos>,
    /// Subscribed chunks the edge has not been sent a snapshot of yet.
    awaiting_snapshot: BTreeSet<ChunkPos>,
}

impl EdgeLink {
    /// What the edge is to hear of `events`: what happened in chunks it subscribed to,
    /// and that the entities among `orphaned` are gone.
    fn visible(
        &self,
        events: &[RegionEvent],
        region: &Region,
        orphaned: &BTreeSet<EntityId>,
    ) -> Vec<RegionEvent> {
        let mut visible = Vec::new();
        for event in events {
            let [current, previous] = event.chunks();
            let visible_now = self.subscriptions.contains(&current);
            let visible_before = self.subscriptions.contains(&previous);
            match event {
                // A player who was let go and will not be passed on was last seen where
                // this region ends, in a chunk nobody can subscribe to here. Those who
                // saw them leave did so from any chunk, so everyone is told.
                RegionEvent::EntityRemoved { entity, chunk }
                    if orphaned.contains(entity) && !region.area().contains(*chunk) =>
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
    /// Loaded chunks that have changed since they were loaded or last stored.
    unsaved: BTreeSet<ChunkPos>,
    /// How many ticks pass between two checkpoints.
    checkpoint_interval: u64,
    /// How many ticks an edge may be without a link before it is gone.
    gone_after: u64,
    /// Whether the store handle is lost, after which the runner does nothing any more.
    lost: bool,
    /// How far the runner is with releasing the region.
    phase: Phase,
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
        let state = restored_state(restored)?;
        Ok(Self::with_store(
            Region::restore(config, state),
            Box::new(store),
        ))
    }

    /// A runner that carries on with `region`, of which `store` has everything up to its
    /// last tick.
    fn with_store(region: Region, store: Box<dyn RegionStore>) -> Self {
        let (attach, attached) = mpsc::channel();
        let state = region.state();
        // Edges count as away from now, however long they were before: they could not
        // have had a link to a region that was not running.
        let edges = state
            .edges
            .iter()
            .map(|(id, edge)| {
                let known = KnownEdge {
                    start: edge.start,
                    settled: true,
                    received: edge.applied,
                    applied: edge.applied,
                    link: None,
                    away_since: state.tick,
                };
                (*id, known)
            })
            .collect();
        let last_inputs = state
            .players
            .iter()
            .map(|(id, player)| (*id, player.last_input))
            .collect();
        let status = RegionStatus::default();
        status.tick.store(state.tick, Ordering::Relaxed);
        status
            .players
            .store(state.players.len() as u64, Ordering::Relaxed);
        Self {
            region,
            store,
            links: BTreeMap::new(),
            next_link: 0,
            attached,
            attach,
            edges,
            last_inputs,
            inputs: TickInputs::default(),
            discards: Vec::new(),
            pending: VecDeque::new(),
            committed: state.tick,
            unreadable: BTreeSet::new(),
            unsaved: BTreeSet::new(),
            checkpoint_interval: DEFAULT_CHECKPOINT_INTERVAL,
            gone_after: DEFAULT_GONE_AFTER,
            lost: false,
            phase: Phase::Running,
            ended: None,
            flushes_asked: 0,
            flushes_answered: 0,
            release_asked: Arc::new(AtomicBool::new(false)),
            status: Arc::new(status),
        }
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
    /// After [`RegionRunner::begin_release`] it does the same until the store has
    /// answered the flush behind the first checkpoint, and from then on carries the
    /// release on instead, a step at a time and without ever waiting: no tick runs, and
    /// nothing is taken from links. Once the runner has ended, a step closes the links
    /// attached since and does nothing else.
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
                        "the store has the first checkpoint of a release; the region stops ticking"
                    );
                    self.phase = Phase::Settling;
                    self.carry_release_on();
                    return;
                }
            }
            Phase::Settling | Phase::Closing => {
                self.carry_release_on();
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
                    self.inputs.chunks_loaded.push((position, chunk));
                }
                // The region goes on waiting for the chunk, which leaves a hole in the
                // world rather than a chunk that would overwrite what was built there.
                // Nothing else waits for it.
                StoreReply::Unreadable { position } => {
                    error!(?position, "a chunk cannot be read from the world store");
                    self.unreadable.insert(position);
                    for link in self.links.values_mut() {
                        link.hold.remove(&position);
                    }
                    self.release_held();
                }
                StoreReply::Committed { tick } => self.committed = self.committed.max(tick),
                StoreReply::Flushed => self.flushes_answered += 1,
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

    /// Stops for good, because the store handle is lost: what the region did since its
    /// last confirmed commit may never have reached the disk, so nobody is told of it,
    /// and carrying on from memory would build on what a restored region does not have.
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
    /// Asking again, or asking a runner that has ended, changes nothing.
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

    /// Does what can be done of a release whose region has stopped ticking, without
    /// waiting for anything.
    fn carry_release_on(&mut self) {
        // No new links: their edges find them closed and look for the region again,
        // which they find at its next owner.
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
                info!(tick = self.region.tick_number(), "the region is released");
                self.end(Ended::Released);
            }
            _ => {}
        }
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
            let welcome = match resume.known {
                true => Welcome::Resumed,
                false => Welcome::Unknown,
            };
            outgoing.push((*id, WorkerToEdge::Welcome(welcome)));
            if let Some(state) = self.region.edge(edge).filter(|_| resume.known) {
                let above = (Bound::Excluded(resume.seen), Bound::Unbounded);
                for (number, entry) in state.outbox.range(above) {
                    let entry = entry.clone();
                    outgoing.push((
                        *id,
                        WorkerToEdge::Outbox {
                            number: *number,
                            entry,
                        },
                    ));
                }
            }
            for player in resume.players {
                // A player of another edge has connected anew through that one; this
                // edge's connection of theirs is of the past.
                let state = self
                    .region
                    .player_state(player)
                    .filter(|state| resume.known && state.edge == edge);
                let answer = match state {
                    Some(state) => Presence::Present {
                        entity: state.entity_id,
                        pose: state.pose,
                        hotbar: state.hotbar,
                        selected_slot: state.selected_slot,
                        last_input: state.last_input,
                        handled: state.handled,
                    },
                    None => Presence::Absent,
                };
                outgoing.push((*id, WorkerToEdge::Presence { player, answer }));
            }
        }

        // The entities of the departures in the outbox of an edge that this tick resets
        // or forgets, which it reports removed: nobody will pass those players on.
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
                if let Durable::Departed { transfer, .. } = entry {
                    orphaned.insert(transfer.entity_id);
                }
            }
        }

        // Who comes in from another region, to be counted if the region takes them in.
        let arriving: BTreeMap<_, _> = self
            .inputs
            .player_changes
            .iter()
            .filter_map(|change| match change {
                PlayerChange::Arrive(_, player, transfer)
                    if self.region.player(*player).is_none() =>
                {
                    Some((*player, transfer.entity_id))
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

        for position in output.chunk_requests {
            self.store.request(StoreRequest::Load { position });
        }
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
                state: postcard::to_stdvec(&output.delta)
                    .expect("a change of state is made of what postcard can write"),
            });
        }
        // After the commit of the tick the saved chunks show.
        if output.tick % self.checkpoint_interval == 0 {
            self.checkpoint();
        }

        // An edge only hears about what happens in chunks it subscribed to.
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

        // Snapshots show the state after this tick, so they include what the events above
        // already said. The edge has to cope with hearing it twice.
        for (id, link) in &mut self.links {
            let ready: Vec<_> = link
                .awaiting_snapshot
                .iter()
                .filter_map(|position| Some((*position, self.region.chunk(*position)?.clone())))
                .collect();
            for (position, chunk) in ready {
                link.awaiting_snapshot.remove(&position);
                link.hold.remove(&position);
                let entities = self
                    .region
                    .entities()
                    .filter(|entity| entity.chunk() == position)
                    .collect();
                let snapshot = WorkerToEdge::ChunkSnapshot {
                    position,
                    tick: output.tick,
                    chunk,
                    entities,
                };
                outgoing.push((*id, snapshot));
            }
        }

        outgoing.extend(self.progress(&output.delta, &resumed));
        self.pending.push_back(HeldTick {
            tick: output.tick,
            needs_commit,
            outgoing,
        });
        // What links sent behind a hello acts on chunks that are loaded now.
        self.release_held();

        self.status.tick.store(output.tick, Ordering::Relaxed);
        let players = self.region.player_count() as u64;
        self.status.players.store(players, Ordering::Relaxed);
        let chunks = self.region.loaded_chunk_count() as u64;
        self.status.chunks.store(chunks, Ordering::Relaxed);
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
            state: postcard::to_stdvec(&self.region.state())
                .expect("a region's state is made of what postcard can write"),
        });
    }

    /// Ticks 20 times per second until `stop` is set, then stores what has not been
    /// stored yet. Returns early, and for good, if the store handle is lost, or once the
    /// region is released, which [`Worker::begin_release`] asks for.
    ///
    /// A release is carried on by looking at the store's answers every millisecond, and
    /// never by waiting for one. If `stop` is set while a release is under way, the
    /// region is let go of as it is, without anything more being asked of the store or
    /// waited for: this ends as [`Ended::Abandoned`].
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
            self.step();
            if matches!(self.phase, Phase::Settling | Phase::Closing) {
                // No tick is due any more; all that is left is the store's answers.
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
            // Whoever stops a release has waited long enough for the store. What the
            // store was asked for it still does if it can, before it closes the region.
            warn!(
                tick = self.region.tick_number(),
                "stopped in the middle of a release; letting go of the region as it is"
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
                hold: BTreeSet::new(),
                held: VecDeque::new(),
                subscriptions: BTreeSet::new(),
                awaiting_snapshot: BTreeSet::new(),
            },
        );
        info!(link = id.0, "an edge attached a link");
        id
    }

    /// Takes everything a link has received: into the inputs of the coming tick, or
    /// aside while the link is held.
    fn drain(&mut self, id: LinkId) {
        // Taken out meanwhile, so that the links that are left are the other ones.
        let Some(mut link) = self.links.remove(&id) else {
            return;
        };
        loop {
            match link.end.try_recv() {
                Ok(Some(message)) => {
                    if !link.hold.is_empty() {
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

    /// Passes on what links sent while they were held, for those that no longer are.
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
            while useful && let Some(message) = link.held.pop_front() {
                useful = self.accept(id, &mut link, message);
            }
            if useful {
                self.links.insert(id, link);
            } else {
                self.let_go(id, link);
            }
        }
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
                seen,
                players,
                chunks,
            } => {
                let resume = Resume {
                    seen,
                    players,
                    known: false,
                };
                return self.hello(id, link, edge, start, resume, chunks);
            }
            EdgeToWorker::Confirm { number } => {
                // Without a hello there is no telling whose outbox is meant.
                if let Some(edge) = link.edge {
                    self.inputs
                        .edges
                        .push(EdgeEvent::Confirmed { edge, number });
                }
            }
            EdgeToWorker::Subscribe { chunks } => {
                for position in chunks {
                    self.subscribe(link, position);
                }
            }
            EdgeToWorker::Unsubscribe { chunks } => {
                for position in chunks {
                    if link.subscriptions.remove(&position) {
                        link.awaiting_snapshot.remove(&position);
                        self.release(position);
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

    /// Handles the hello of the link `id`, which is not among `self.links` meanwhile.
    /// Returns false if the link is of no use: its edge has been replaced by a later
    /// start, or it has said hello before.
    fn hello(
        &mut self,
        id: LinkId,
        link: &mut EdgeLink,
        edge: EdgeId,
        start: u64,
        mut resume: Resume,
        chunks: Vec<ChunkPos>,
    ) -> bool {
        // A link is one edge's for as long as it lasts, and what a hello sets off happens
        // once per link.
        if link.edge.is_some() {
            warn!(link = id.0, "an edge said hello twice; closing its link");
            return false;
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
        resume.known = known.settled;
        info!(
            link = id.0,
            edge = edge.0,
            start,
            known = resume.known,
            "an edge said hello"
        );
        self.inputs.edges.push(EdgeEvent::Started { edge, start });
        self.inputs.edges.push(EdgeEvent::Confirmed {
            edge,
            number: resume.seen,
        });
        link.edge = Some(edge);
        link.unknown = !resume.known;
        link.resume = Some(resume);
        for position in chunks {
            self.subscribe(link, position);
            // What the edge sends next acts on these chunks, so it waits until they are
            // loaded, which is when their snapshots are made.
            if link.awaiting_snapshot.contains(&position) && !self.unreadable.contains(&position) {
                link.hold.insert(position);
            }
        }
        true
    }

    /// Subscribes `link` to the chunk at `position`, if that is the region's.
    fn subscribe(&mut self, link: &mut EdgeLink, position: ChunkPos) {
        // Chunks elsewhere are another region's to show.
        if self.region.area().contains(position) && link.subscriptions.insert(position) {
            self.inputs.tickets_added.push(position);
            link.awaiting_snapshot.insert(position);
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
            | PlayerChange::Leave(from, _)
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
            EdgeToWorker::PlayerLeave { player } => {
                self.inputs.change(PlayerChange::Leave(edge, player));
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
                number,
                input,
            } => {
                // The region ignores what comes through another edge than the player's.
                self.inputs.input(edge, player, number, input);
            }
            // Not numbered; `accept` handles them.
            EdgeToWorker::Hello { .. }
            | EdgeToWorker::Confirm { .. }
            | EdgeToWorker::Subscribe { .. }
            | EdgeToWorker::Unsubscribe { .. } => {}
        }
    }

    /// Gives back the ticket of a link that no longer needs the chunk at `position`.
    /// That link must not be among `self.links`, or no longer be subscribed.
    fn release(&mut self, position: ChunkPos) {
        let needs = |link: &EdgeLink| link.subscriptions.contains(&position);
        if !self.links.values().any(needs) {
            // The region drops the chunk at the start of the coming tick, before
            // anything else can change it, so this is its final state.
            self.save(position);
        }
        self.inputs.tickets_removed.push(position);
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
        for position in link.subscriptions {
            self.release(position);
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

/// The state of a region as the store has it: its last checkpoint, or that of a region
/// that has never run, with every commit since applied.
fn restored_state(restored: Restored) -> Result<RegionState, RestoreError> {
    let mut state = match &restored.state {
        Some(stored) => {
            postcard::from_bytes(&stored.state).map_err(|error| RestoreError::State {
                tick: stored.tick,
                error,
            })?
        }
        None => RegionState::new(restored.entity_ids),
    };
    for stored in &restored.deltas {
        let delta: StateDelta =
            postcard::from_bytes(&stored.state).map_err(|error| RestoreError::Delta {
                tick: stored.tick,
                error,
            })?;
        state.apply(&delta);
    }
    Ok(state)
}

/// A region running on its own thread.
pub struct Worker {
    thread: JoinHandle<Ended>,
    stop: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
}

impl Worker {
    /// Starts ticking `runner` on a new thread. The thread ends by itself if the store
    /// handle is lost, which [`RegionStatus::store_lost`] shows.
    pub fn spawn(runner: RegionRunner) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let release = Arc::clone(&runner.release_asked);
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
        }
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
    /// A worker that is releasing its region stores nothing more and does not wait for
    /// the store: it lets go of the region as it is, which is [`Ended::Abandoned`]. One
    /// that has ended already says how.
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
    use clustine_sim::{PlayerTransfer, RemoteAction, RemoteStep};
    use clustine_world::{BlockPos, ChunkArea, EntityIds, Vec3};
    use clustine_worldgen::FlatGenerator;
    use clustine_worldstore::{Store, StoreHandle};
    use tokio::time::timeout;
    use uuid::Uuid;

    use super::*;

    /// The two kinds of link: a direct one, and one that serialises every message the
    /// way a link between two processes does.
    const KINDS: [fn(usize) -> (TestEdge, WorkerEnd); 2] = [in_process, framed];

    /// An edge's end of a link that numbers what it sends, as an edge does.
    struct TestEdge {
        end: EdgeEnd,
        /// Which edge this is, and which start of it.
        edge: EdgeId,
        start: u64,
        /// The number of the last numbered message sent.
        sent: AtomicU64,
        /// What the worker said about resuming and progress, which `recv` and `try_recv`
        /// set aside: most tests are about what else a region says.
        aside: Vec<WorkerToEdge>,
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
                sent: AtomicU64::new(0),
                aside: Vec::new(),
            }
        }

        /// The same start of the same edge on another link, numbering on from where it
        /// was. It has not said hello there yet.
        fn again(&self, end: EdgeEnd) -> Self {
            let again = Self::silent(end, self.edge, self.start);
            again
                .sent
                .store(self.sent.load(Ordering::Relaxed), Ordering::Relaxed);
            again
        }

        /// What this edge says first on a link.
        fn hello(&self, seen: u64, players: &[PlayerId], chunks: &[ChunkPos]) -> EdgeToWorker {
            EdgeToWorker::Hello {
                edge: self.edge,
                start: self.start,
                seen,
                players: players.to_vec(),
                chunks: chunks.to_vec(),
            }
        }

        fn numbered(&self, body: EdgeToWorker) -> EdgeMessage {
            let number = body
                .is_numbered()
                .then(|| self.sent.fetch_add(1, Ordering::Relaxed) + 1);
            EdgeMessage { number, body }
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

        async fn recv(&mut self) -> Option<WorkerToEdge> {
            loop {
                let message = self.end.recv().await?;
                if !Self::about_resuming(&message) {
                    return Some(message);
                }
                self.aside.push(message);
            }
        }

        fn try_recv(&mut self) -> Result<Option<WorkerToEdge>, LinkError> {
            loop {
                match self.end.try_recv()? {
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
                _ => false,
            }
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

    impl RegionStore for Gate {
        fn request(&self, request: StoreRequest) {
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

    /// The western one of two regions: it ends where the chunks with x = 1 begin.
    const WEST: ChunkArea = ChunkArea {
        min_x: None,
        max_x: Some(1),
    };

    /// Where players enter the flat world.
    const SPAWN: Vec3 = Vec3::new(0.5, -60.0, 0.5);

    const ORIGIN: ChunkPos = ChunkPos::new(0, 0);

    fn config(area: ChunkArea) -> RegionConfig {
        RegionConfig {
            spawn: SPAWN,
            area,
            starting_hotbar: [None; HOTBAR_SLOTS],
        }
    }

    /// A store of a flat world that only lasts as long as the store.
    fn memory() -> Store {
        Store::memory(Arc::new(FlatGenerator::classic()))
    }

    /// A store of a flat world kept in `directory`.
    fn on_disk(directory: &std::path::Path) -> Store {
        Store::local(directory, Arc::new(FlatGenerator::classic())).unwrap()
    }

    /// The hello of the owner of the one region of a store in these tests, whatever
    /// part of the world the region takes itself to be.
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
        let (inner, restored) = store.open_region(owner()).unwrap();
        let control = Arc::new(GateControl::default());
        let gate = Gate {
            inner,
            control: Arc::clone(&control),
        };
        let region = Region::restore(config, restored_state(restored).unwrap());
        (RegionRunner::with_store(region, Box::new(gate)), control)
    }

    /// A runner for a region of a flat world that only lasts as long as the runner,
    /// with `link` as its first link.
    fn runner_of(config: RegionConfig, link: WorkerEnd) -> RegionRunner {
        let runner = opened(&memory(), config);
        runner.links().attach(link);
        runner
    }

    fn runner(link: WorkerEnd) -> RegionRunner {
        runner_of(config(ChunkArea::EVERYWHERE), link)
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

    /// A step along the x axis as the input with the given number.
    fn walk_as(player: PlayerId, number: u64, x: f64) -> EdgeToWorker {
        EdgeToWorker::Input {
            player,
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
            edge.send(EdgeToWorker::Subscribe { chunks: square(1) })
                .await
                .unwrap();

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
        edge.send(EdgeToWorker::Subscribe {
            chunks: vec![ChunkPos::new(0, 0)],
        })
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
        edge.send(EdgeToWorker::Subscribe {
            chunks: vec![ChunkPos::new(5, 0)],
        })
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
        edge.send(EdgeToWorker::PlayerLeave { player: player() })
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

            near.send(EdgeToWorker::Subscribe {
                chunks: vec![origin],
            })
            .await
            .unwrap();
            far.send(EdgeToWorker::Subscribe {
                chunks: vec![distant],
            })
            .await
            .unwrap();
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

            first.send(join(player(), "Notch")).await.unwrap();
            second.send(join(other_player(), "Jeb")).await.unwrap();
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
            let runner = opened(&memory(), config(ChunkArea::EVERYWHERE));
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
            second
                .send(EdgeToWorker::Subscribe {
                    chunks: vec![ORIGIN],
                })
                .await
                .unwrap();
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
        edge.send(EdgeToWorker::Subscribe {
            chunks: vec![ChunkPos::new(0, 0)],
        })
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

        edge.send(EdgeToWorker::Unsubscribe {
            chunks: vec![origin],
        })
        .await
        .unwrap();
        step(&mut runner);
        assert_eq!(runner.region().loaded_chunk_count(), 0);

        edge.send(EdgeToWorker::Subscribe {
            chunks: vec![origin],
        })
        .await
        .unwrap();
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
            let subscribe = || EdgeToWorker::Subscribe {
                chunks: vec![origin],
            };
            let unsubscribe = || EdgeToWorker::Unsubscribe {
                chunks: vec![origin],
            };
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
        let mut first = opened(&on_disk(directory.path()), config(ChunkArea::EVERYWHERE));
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
        let mut second = opened(&on_disk(directory.path()), config(ChunkArea::EVERYWHERE));
        assert_eq!(second.region().state(), state);
        second.links().attach(worker_end);
        edge.send(EdgeToWorker::Subscribe {
            chunks: vec![ORIGIN],
        })
        .await
        .unwrap();
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
            area: ChunkArea::EVERYWHERE,
            starting_hotbar: [None; HOTBAR_SLOTS],
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

        edge.send(EdgeToWorker::Subscribe { chunks: square(1) })
            .await
            .unwrap();
        step_until(&mut runner, |runner| {
            runner.region().loaded_chunk_count() == 9
        });

        let mut kept = square(1);
        let dropped = kept.split_off(4);
        edge.send(EdgeToWorker::Unsubscribe { chunks: dropped })
            .await
            .unwrap();
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
            edge.send(EdgeToWorker::Subscribe {
                chunks: vec![position],
            })
            .await
            .unwrap();
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
        edge.send(EdgeToWorker::Unsubscribe {
            chunks: vec![position],
        })
        .await
        .unwrap();
        step(&mut runner);
        assert_eq!(runner.region().loaded_chunk_count(), 0);
    }

    /// A region shows its own part of the world. What an edge wants to see of the rest
    /// it has to ask the regions there for.
    #[tokio::test(flavor = "multi_thread")]
    async fn subscriptions_outside_the_area_of_the_region_are_ignored() {
        for connect in KINDS {
            let (mut edge, worker_end) = connect(256);
            let mut runner = runner_of(config(WEST), worker_end);
            let (inside, outside) = (ChunkPos::new(0, 0), ChunkPos::new(1, 0));

            edge.send(EdgeToWorker::Subscribe {
                chunks: vec![outside, inside, ChunkPos::new(7, -3)],
            })
            .await
            .unwrap();
            assert_eq!(snapshot(step_for(&mut runner, &mut edge)).0, inside);
            let link = &runner.links[&LinkId(0)];
            assert_eq!(link.subscriptions, BTreeSet::from([inside]));
            assert!(link.awaiting_snapshot.is_empty());
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

            // Letting go of what was never held changes nothing.
            edge.send(EdgeToWorker::Unsubscribe {
                chunks: vec![outside],
            })
            .await
            .unwrap();
            edge.send(EdgeToWorker::Discard {
                entity: EntityId(10),
                chunk: inside,
            })
            .await
            .unwrap();
            step_for(&mut runner, &mut edge);
            assert_eq!(runner.region().loaded_chunk_count(), 1);
        }
    }

    /// Numbers are what lets an edge send messages again without any being applied
    /// twice. An edge whose numbers on one link have a gap or go back, or are where none
    /// belong, has lost track, and so has one that says hello twice. The runner stops
    /// listening to it; its players stay, for the edge to come back to.
    #[tokio::test]
    async fn a_link_whose_messages_are_out_of_order_is_closed() {
        let subscription = EdgeToWorker::Subscribe {
            chunks: vec![ORIGIN],
        };
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
        let mut runner = runner_of(config(WEST), worker_end);
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
                    entry: Durable::Remote(_),
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
        edge.try_send(EdgeToWorker::Subscribe { chunks: square(1) })
            .unwrap();

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
            [
                WorkerToEdge::Welcome(Welcome::Unknown),
                WorkerToEdge::Progress { .. }
            ]
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
        let mut new = old.again(edge_end);
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
        assert_eq!(told[0], WorkerToEdge::Welcome(Welcome::Resumed));
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
            .send(EdgeToWorker::PlayerLeave { player: player() })
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
        let leave = || EdgeToWorker::PlayerLeave { player: player() };

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
        edge.send(EdgeToWorker::PlayerLeave { player: player() })
            .await
            .unwrap();
        edge.send(join(player(), "Notch")).await.unwrap();
        step(&mut runner);
        assert_eq!(
            runner.region().player(player()),
            Some((EntityId(2), Pose::at(SPAWN)))
        );

        // Their new connection numbers what they do from the start again.
        edge.send(walk_as(player(), 1, 4.0)).await.unwrap();
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
            let mut runner = runner_of(config(WEST), worker_end);
            runner.links().attach(bystander_end);
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
            edge.send(walk_as(player(), 7, 12.0)).await.unwrap();
            edge.send(walk_as(player(), 8, 20.0)).await.unwrap();
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
            let mut runner = runner_of(config(WEST), worker_end);
            runner.links().attach(other_end);
            let origin = ChunkPos::new(0, 0);
            let subscribe = || EdgeToWorker::Subscribe {
                chunks: vec![origin],
            };

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
                    entry: Durable::Remote(request),
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
                    entry: Durable::Remote(RemoteAction {
                        player: other_player(),
                        sequence: 4,
                        step: RemoteStep::Place {
                            target,
                            block: stone,
                            placer,
                        },
                    }),
                }
            );
            step(&mut runner);
            assert_eq!(received(&mut edge), []);
            assert_eq!(received(&mut other), []);
        }
    }

    /// A transfer can still be on its way when the player has long connected anew and
    /// joined. The region keeps the player it has.
    #[tokio::test]
    async fn an_arrival_does_not_take_a_player_from_the_link_they_belong_to() {
        let (first, first_end) = in_process(256);
        let (second, second_end) = in_process(256);
        let mut runner = runner(first_end);
        runner.links().attach(second_end);
        let status = runner.status();
        let leave = || EdgeToWorker::PlayerLeave { player: player() };

        first.send(join(player(), "Notch")).await.unwrap();
        step(&mut runner);
        second
            .send(EdgeToWorker::PlayerArrive {
                player: player(),
                transfer: transfer(EntityIds::block(3).unwrap().first),
            })
            .await
            .unwrap();
        step(&mut runner);
        assert_eq!(
            runner.region().player(player()),
            Some((EntityId(1), Pose::at(SPAWN)))
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
        let (edge, edge_end) = in_process(256);
        let (mut watcher, watcher_end) = in_process(256);
        let mut runner = runner_of(config(WEST), edge_end).with_gone_after(5);
        runner.links().attach(watcher_end);
        edge.send(join(player(), "Notch")).await.unwrap();
        edge.send(join(other_player(), "Jeb")).await.unwrap();
        watcher
            .send(EdgeToWorker::Subscribe {
                chunks: vec![ORIGIN],
            })
            .await
            .unwrap();
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
        let region = Region::new(config(ChunkArea::EVERYWHERE), entity_ids);
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
        let (edge, worker_end) = in_process(256);
        let mut runner = runner_of(config(WEST), worker_end);
        let status = runner.status();
        let read = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        let population = || (read(&status.players), read(&status.chunks));
        let traffic = || (read(&status.arrivals), read(&status.departures));
        assert_eq!(
            (read(&status.tick), population(), traffic()),
            (0, (0, 0), (0, 0))
        );

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
        edge.send(walk_as(other_player(), 8, 20.0)).await.unwrap();
        step(&mut runner);
        assert_eq!((population(), traffic()), ((0, 1), (1, 2)));
        edge.send(EdgeToWorker::PlayerArrive {
            player: other_player(),
            transfer: transfer(EntityIds::block(3).unwrap().first),
        })
        .await
        .unwrap();
        edge.send(EdgeToWorker::Unsubscribe {
            chunks: vec![ChunkPos::new(0, 0)],
        })
        .await
        .unwrap();
        step(&mut runner);
        assert_eq!((population(), traffic()), ((1, 0), (2, 2)));
        assert_eq!(read(&status.tick), runner.region().tick_number());
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
        let (mut runner, gate) = gated(&memory(), config(ChunkArea::EVERYWHERE));
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
        assert_eq!(told[0], WorkerToEdge::Welcome(Welcome::Unknown));
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
        let (mut runner, gate) = gated(&memory(), config(ChunkArea::EVERYWHERE));
        runner.links().attach(worker_end);

        // The hello makes the edge known, which is a change. Loading a chunk and showing
        // it are none: chunks are not part of the region's state.
        edge.send(EdgeToWorker::Subscribe {
            chunks: vec![ORIGIN],
        })
        .await
        .unwrap();
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
        edge.send(EdgeToWorker::Unsubscribe {
            chunks: vec![ORIGIN],
        })
        .await
        .unwrap();
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
        let store = memory();
        let (mut edge, worker_end) = in_process(256);
        let (mut runner, gate) = gated(&store, config(WEST));
        runner.links().attach(worker_end);
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

        let mut restored = opened(&store, config(WEST));
        assert_eq!(restored.region().state(), committed);
        let (edge_end, worker_end) = link::in_process(256);
        let mut again = edge.again(edge_end);
        restored.links().attach(worker_end);
        again
            .send(again.hello(0, &[player(), other_player()], &[]))
            .await
            .unwrap();
        step(&mut restored);
        assert_eq!(restored.region().tick_number(), tick + 1);

        let told = again.everything();
        let [
            WorkerToEdge::Welcome(Welcome::Resumed),
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
        let (edge, worker_end) = in_process(256);
        let mut first = opened(&on_disk(directory.path()), config(WEST));
        first.links().attach(worker_end);
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

        let mut second = opened(&on_disk(directory.path()), config(WEST));
        // Ticks go on from the last the store has.
        assert_eq!(second.region().state(), state);
        assert_eq!(second.status().tick.load(Ordering::Relaxed), state.tick);
        assert_eq!(second.status().players.load(Ordering::Relaxed), 1);
        step(&mut second);
        assert_eq!(second.region().tick_number(), state.tick + 1);

        // The edge is back and sends again what it has not heard to be applied. What the
        // region has is dropped, and the next is taken.
        let (edge_end, worker_end) = link::in_process(256);
        let again = edge.again(edge_end);
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
        let ahead = edge.again(edge_end);
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
            assert_eq!(
                later.everything()[0],
                WorkerToEdge::Welcome(Welcome::Unknown)
            );
        }
    }

    /// An edge the region knows says hello on a new link. It is told that the region
    /// carries on, then what it has not seen of its outbox, then where each player it
    /// asks about is, before anything the tick itself has to say. Its old link is closed.
    #[tokio::test]
    async fn an_edge_that_resumes_is_told_what_it_missed_before_anything_else() {
        let (mut edge, edge_end) = in_process(256);
        let (stranger, stranger_end) = in_process(256);
        let mut runner = runner_of(config(WEST), edge_end);
        runner.links().attach(stranger_end);
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
        let mut again = edge.again(edge_end);
        runner.links().attach(worker_end);
        let asked = [player(), third_player(), other_player()];
        again.send(again.hello(1, &asked, &[])).await.unwrap();
        let step_aside = walk(third_player(), 2.0);
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
            WorkerToEdge::Welcome(Welcome::Resumed),
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
        watcher
            .send(EdgeToWorker::Subscribe {
                chunks: vec![ORIGIN],
            })
            .await
            .unwrap();
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
                WorkerToEdge::Welcome(Welcome::Unknown),
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
                WorkerToEdge::Welcome(Welcome::Unknown),
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
        edge.send(EdgeToWorker::Subscribe {
            chunks: vec![ChunkPos::new(0, 1)],
        })
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
        assert_eq!(told[0], WorkerToEdge::Welcome(Welcome::Unknown));
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
        let second = first.again(edge_end);
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
        let mut runner = runner_of(config(WEST), edge_end);
        runner.links().attach(other_end);
        let beside = ChunkPos::new(0, 1);
        edge.send(join(player(), "Notch")).await.unwrap();
        edge.send(join(other_player(), "Jeb")).await.unwrap();
        edge.send(walk(player(), 14.5)).await.unwrap();
        edge.send(EdgeToWorker::Subscribe {
            chunks: vec![ORIGIN],
        })
        .await
        .unwrap();
        // Another edge has the chunk beside loaded, so that this one is shown it in the
        // very tick it asks for it.
        other
            .send(EdgeToWorker::Subscribe {
                chunks: vec![beside],
            })
            .await
            .unwrap();
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
        edge.send(EdgeToWorker::Subscribe {
            chunks: vec![beside],
        })
        .await
        .unwrap();
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
                entry: Durable::Remote(_),
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
        let unknown = WorkerToEdge::Welcome(Welcome::Unknown);
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
        let (mut runner, gate) = gated(&store, config(ChunkArea::EVERYWHERE));
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
            entity_ids: EntityIds::block(0).unwrap(),
            state,
            deltas,
        };
        let unreadable = |tick| clustine_rpc::TickState {
            tick,
            state: vec![0xff; 3],
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
            let store = match on_disk_too {
                true => on_disk(directory.path()),
                false => memory(),
            };
            let (mut edge, worker_end) = in_process(256);
            let mut first = opened(&store, config(WEST));
            let status = first.status();
            first.links().attach(worker_end);
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
            let (handle, restored) = opened_next(&store);
            assert_eq!(restored.deltas, []);
            assert_eq!(
                restored.state.as_ref().map(|stored| stored.tick),
                Some(state.tick)
            );
            let mut second = RegionRunner::restore(config(WEST), handle, restored).unwrap();
            assert_eq!(second.region().state(), state);
            let (again, worker_end) = in_process(256);
            second.links().attach(worker_end);
            again
                .send(EdgeToWorker::Subscribe {
                    chunks: vec![ORIGIN],
                })
                .await
                .unwrap();
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
            let (mut runner, gate) = gated(&store, config(ChunkArea::EVERYWHERE));
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
        let (mut runner, gate) = gated(&store, config(ChunkArea::EVERYWHERE));
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
        assert_eq!(
            other.everything()[0],
            WorkerToEdge::Welcome(Welcome::Unknown)
        );
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
        let mut next =
            RegionRunner::restore(config(ChunkArea::EVERYWHERE), handle, restored).unwrap();
        assert_eq!(next.region().state(), state);
        let (edge_end, worker_end) = link::in_process(256);
        let mut again = edge.again(edge_end);
        next.links().attach(worker_end);
        again
            .send(again.hello(0, &[player()], &[ORIGIN]))
            .await
            .unwrap();
        again.send_as(number, late).await;
        step_until(&mut next, |runner| x_of(runner, player()) == Some(5.0));
        step(&mut next);
        assert_eq!(next.region().edge(edge.edge).unwrap().applied, number);
        assert_eq!(
            again.everything()[0],
            WorkerToEdge::Welcome(Welcome::Resumed)
        );
    }

    /// Ticks that ran before the region stopped ticking are owed to the edges once the
    /// store has them, and not before. A release waits for that, and closes the links
    /// only after it has published them, oldest first.
    #[tokio::test]
    async fn a_release_publishes_the_ticks_that_ran_once_they_are_confirmed_and_in_order() {
        let store = memory();
        let (mut edge, worker_end) = in_process(256);
        let (mut runner, gate) = gated(&store, config(ChunkArea::EVERYWHERE));
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
            let (mut runner, gate) = gated(&store, config(ChunkArea::EVERYWHERE));
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
        let (mut runner, gate) = gated(&store, config(ChunkArea::EVERYWHERE));
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
        during
            .send(EdgeToWorker::Subscribe {
                chunks: vec![ORIGIN],
            })
            .await
            .unwrap();
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
            let (runner, gate) = gated(&store, config(ChunkArea::EVERYWHERE));
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
            let config = config(ChunkArea::EVERYWHERE);
            let mut next = RegionRunner::restore(config, handle, restored).unwrap();
            assert_eq!(next.region().state(), state);
            let (again, worker_end) = in_process(256);
            next.links().attach(worker_end);
            again
                .send(EdgeToWorker::Subscribe {
                    chunks: vec![ORIGIN],
                })
                .await
                .unwrap();
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
            let runner = opened(&store, config(ChunkArea::EVERYWHERE));
            let (links, status) = (runner.links(), runner.status());
            let worker = Worker::spawn(runner);
            let (mut edge, worker_end) = in_process(256);
            links.attach(worker_end);
            edge.send(join(player(), "Notch")).await.unwrap();
            edge.send(EdgeToWorker::Subscribe {
                chunks: vec![ORIGIN],
            })
            .await
            .unwrap();
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
            let config = config(ChunkArea::EVERYWHERE);
            let mut next = RegionRunner::restore(config, handle, restored).unwrap();
            assert_eq!(next.region().player_count(), 1);
            assert_eq!(
                next.region().tick_number(),
                status.tick.load(Ordering::Relaxed)
            );
            let (again, worker_end) = in_process(256);
            next.links().attach(worker_end);
            again
                .send(EdgeToWorker::Subscribe {
                    chunks: vec![ORIGIN],
                })
                .await
                .unwrap();
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
        let (runner, gate) = gated(&store, config(ChunkArea::EVERYWHERE));
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

        let runner = opened(&memory(), config(ChunkArea::EVERYWHERE));
        let status = runner.status();
        let worker = Worker::spawn(runner);
        assert_eq!(worker.stop(), Ended::Stopped);
        assert_eq!(status.ended(), Some(Ended::Stopped));
    }
}
