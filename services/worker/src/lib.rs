//! Worker service: ticks the regions it owns.
//!
//! A [`RegionRunner`] connects a region to the outside: it turns the messages of the
//! edges and the answers of the world store into tick inputs, runs the tick, and
//! publishes what resulted. The simulation itself never waits for anything.
//!
//! Edges come and go. Each has a link of its own, with its own subscriptions and its own
//! players, and a link that ends or does not keep up is dropped without the region
//! missing a tick.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::mem;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use clustine_rpc::link::WorkerEnd;
use clustine_rpc::{EdgeToWorker, StoreReply, StoreRequest, WorkerToEdge};
use clustine_sim::api::RegionEvent;
use clustine_sim::{PlayerChange, PlayerEvent, Region, RemoteOutcome, TickInputs};
use clustine_world::{ChunkPos, PlayerId};
use clustine_worldstore::StoreHandle;
use tracing::{error, info, warn};

/// The length of a tick: 20 ticks per second.
pub const TICK: Duration = Duration::from_millis(50);

/// Ticks between two checkpoints unless set otherwise: five minutes.
pub const DEFAULT_CHECKPOINT_INTERVAL: u64 = 5 * 60 * 20;

/// A runner that has fallen further behind than this many ticks skips them instead of
/// trying to catch up.
const MAX_CATCH_UP_TICKS: u32 = 10;

/// Counters of a region that others may read while its runner runs.
#[derive(Debug, Default)]
pub struct RegionStatus {
    /// The number of the last tick that ran.
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
    /// reached, or it has given the region to another owner. The region goes on
    /// ticking, but nothing it changes is kept, so whoever runs it should stop it.
    pub store_lost: AtomicBool,
}

/// Attaches links to a [`RegionRunner`] while it runs, from any thread.
#[derive(Debug, Clone)]
pub struct Links {
    attached: Sender<WorkerEnd>,
}

impl Links {
    /// Hands `link` to the runner, which serves it from its next tick on. If the runner
    /// is gone, the link is closed instead, which its other end notices.
    pub fn attach(&self, link: WorkerEnd) {
        // Nobody is left to serve the link, and dropping it is what closes it.
        let _ = self.attached.send(link);
    }
}

/// Names a link for as long as its runner exists. Links are numbered in the order they
/// were attached, which is also the order they are served in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct LinkId(u64);

/// A link to an edge and what the runner keeps for it.
struct EdgeLink {
    end: WorkerEnd,
    /// Chunks the edge is subscribed to.
    subscriptions: BTreeSet<ChunkPos>,
    /// Subscribed chunks the edge has not been sent a snapshot of yet.
    awaiting_snapshot: BTreeSet<ChunkPos>,
}

impl EdgeLink {
    /// What the edge is to hear of `events`: what happened in chunks it subscribed to.
    fn visible(&self, events: &[RegionEvent], region: &Region) -> Vec<RegionEvent> {
        let mut visible = Vec::new();
        for event in events {
            let [current, previous] = event.chunks();
            let visible_now = self.subscriptions.contains(&current);
            let visible_before = self.subscriptions.contains(&previous);
            match event {
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

/// Runs one region for the edges that are linked to it.
pub struct RegionRunner {
    region: Region,
    store: StoreHandle,
    /// The links to edges, in the order they were attached.
    links: BTreeMap<LinkId, EdgeLink>,
    /// The number the next link gets.
    next_link: u64,
    /// Links that have been attached but not been taken up yet.
    attached: Receiver<WorkerEnd>,
    /// Kept to make [`Links`] of.
    attach: Sender<WorkerEnd>,
    /// The link each player belongs to: the one they joined or arrived through. What
    /// concerns a single player goes to that link, and only that link acts for them.
    players: BTreeMap<PlayerId, LinkId>,
    /// What the coming tick will be given.
    inputs: TickInputs,
    /// The link each of the remote actions among those inputs came through, in the same
    /// order. What becomes of an action is told to that link.
    remote_from: Vec<LinkId>,
    /// Loaded chunks that have changed since they were loaded or last stored.
    unsaved: BTreeSet<ChunkPos>,
    /// How many ticks pass between two checkpoints.
    checkpoint_interval: u64,
    status: Arc<RegionStatus>,
}

impl RegionRunner {
    /// A runner with `link` as its first link.
    pub fn new(region: Region, link: WorkerEnd, store: StoreHandle) -> Self {
        let mut runner = Self::without_links(region, store);
        runner.take_up(link);
        runner
    }

    /// A runner that ticks without any link until one is attached through
    /// [`RegionRunner::links`].
    pub fn without_links(region: Region, store: StoreHandle) -> Self {
        let (attach, attached) = mpsc::channel();
        Self {
            region,
            store,
            links: BTreeMap::new(),
            next_link: 0,
            attached,
            attach,
            players: BTreeMap::new(),
            inputs: TickInputs::default(),
            remote_from: Vec::new(),
            unsaved: BTreeSet::new(),
            checkpoint_interval: DEFAULT_CHECKPOINT_INTERVAL,
            status: Arc::default(),
        }
    }

    /// A handle through which further links are attached while the runner runs.
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
    /// chunks that stay loaded are only in the write-ahead log, which grows meanwhile.
    pub fn with_checkpoint_interval(mut self, ticks: u64) -> Self {
        self.checkpoint_interval = ticks.max(1);
        self
    }

    pub fn region(&self) -> &Region {
        &self.region
    }

    /// Runs one tick with everything that has arrived since the last one. A link that
    /// has ended or does not keep up is dropped on the way; the region ticks on with the
    /// links that remain, or with none.
    pub fn step(&mut self) {
        // The links known so far come first, and only then the ones attached since. An
        // edge that connects again has ended its old link before, so its players leave
        // through the old link before they join through the new one, and stay.
        let known: Vec<_> = self.links.keys().copied().collect();
        for id in known {
            self.drain(id);
        }
        while let Ok(end) = self.attached.try_recv() {
            let id = self.take_up(end);
            self.drain(id);
        }
        while let Some(StoreReply::Loaded { position, chunk }) = self.store.try_reply() {
            self.inputs.chunks_loaded.push((position, chunk));
        }

        // What is collected from here on is for the tick after this one: the players and
        // the chunks of a link that is lost while it is told what this tick did.
        let inputs = mem::take(&mut self.inputs);
        let remote_from = mem::take(&mut self.remote_from);
        let output = self.region.tick(&inputs);

        for position in output.chunk_requests {
            self.store.request(StoreRequest::Load { position });
        }
        // Logged before anyone is told, so that what players are shown is on its way to
        // disk already.
        let changes: Vec<_> = output
            .events
            .iter()
            .filter_map(|event| match event {
                RegionEvent::BlockChanged { position, state } => Some((*position, *state)),
                _ => None,
            })
            .collect();
        if !changes.is_empty() {
            self.unsaved
                .extend(changes.iter().map(|(position, _)| position.chunk()));
            self.store.request(StoreRequest::Log {
                tick: output.tick,
                changes,
            });
        }
        if output.tick % self.checkpoint_interval == 0 {
            self.checkpoint();
        }

        // An edge only hears about what happens in chunks it subscribed to.
        let deltas: Vec<_> = self
            .links
            .iter()
            .map(|(id, link)| (*id, link.visible(&output.events, &self.region)))
            .collect();
        for (id, events) in deltas {
            if !events.is_empty() {
                let delta = WorkerToEdge::TickDelta {
                    tick: output.tick,
                    events,
                };
                self.publish(id, delta);
            }
        }

        // What players did to blocks of other regions goes to their edge to be passed
        // on, while it is still known which link they belong to: a player can be let go
        // in the same tick.
        for action in output.remote_requests {
            if let Some(id) = self.players.get(&action.player).copied() {
                self.publish(id, WorkerToEdge::Remote(action));
            }
        }

        // After the events, so that a player is told that their action was handled only
        // once they have been told what it did. That goes for players of other regions
        // too, whose edge asked for what they did to be done here.
        for (outcome, id) in output.remote_outcomes.into_iter().zip(remote_from) {
            let message = match outcome {
                RemoteOutcome::Done { player, sequence } => {
                    WorkerToEdge::RemoteDone { player, sequence }
                }
                RemoteOutcome::Next(action) => WorkerToEdge::Remote(action),
            };
            self.publish(id, message);
        }
        for (player, event) in output.player_events {
            self.tell(output.tick, player, event);
        }

        // Snapshots come last and show the state after this tick, so they include what
        // the events above already said. The edge has to cope with hearing it twice.
        let remaining: Vec<_> = self.links.keys().copied().collect();
        for id in remaining {
            self.send_snapshots(id, output.tick);
        }

        if self.store.is_lost() && !self.status.store_lost.swap(true, Ordering::Relaxed) {
            error!("the world store is lost; nothing that changes from now on is kept");
        }
        self.status.tick.store(output.tick, Ordering::Relaxed);
        let players = self.region.player_count() as u64;
        self.status.players.store(players, Ordering::Relaxed);
        let chunks = self.region.loaded_chunk_count() as u64;
        self.status.chunks.store(chunks, Ordering::Relaxed);
    }

    /// Hands the chunk at `position` to the store if it has unsaved changes.
    fn save(&mut self, position: ChunkPos) {
        if self.unsaved.remove(&position)
            && let Some(chunk) = self.region.chunk(position)
        {
            self.store.request(StoreRequest::Save {
                position,
                tick: self.region.tick_number(),
                chunk: chunk.clone(),
            });
        }
    }

    /// Hands every chunk with unsaved changes to the store and lets it empty the log,
    /// which from then on only has to cover what happens next.
    fn checkpoint(&mut self) {
        for position in self.unsaved.clone() {
            self.save(position);
        }
        self.store.request(StoreRequest::Checkpoint);
    }

    /// Ticks 20 times per second until `stop` is set, then stores what has not been
    /// stored yet.
    pub fn run(&mut self, stop: &AtomicBool) {
        let mut deadline = Instant::now() + TICK;
        while !stop.load(Ordering::Relaxed) {
            self.step();
            let now = Instant::now();
            if let Some(early) = deadline.checked_duration_since(now) {
                thread::sleep(early);
            } else if now.duration_since(deadline) > TICK * MAX_CATCH_UP_TICKS {
                deadline = now;
            }
            deadline += TICK;
        }
        self.checkpoint();
        self.store.flush();
    }

    /// Makes `end` a link of the runner.
    fn take_up(&mut self, end: WorkerEnd) -> LinkId {
        let id = LinkId(self.next_link);
        self.next_link += 1;
        self.links.insert(
            id,
            EdgeLink {
                end,
                subscriptions: BTreeSet::new(),
                awaiting_snapshot: BTreeSet::new(),
            },
        );
        info!(link = id.0, "an edge attached a link");
        id
    }

    /// Turns everything a link has received into inputs of the coming tick.
    fn drain(&mut self, id: LinkId) {
        // Taken out meanwhile, so that the links that are left are the other ones.
        let Some(mut link) = self.links.remove(&id) else {
            return;
        };
        loop {
            match link.end.try_recv() {
                Ok(Some(message)) => self.accept(id, &mut link, message),
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

    /// Handles a message of the link `id`, which is not among `self.links` meanwhile.
    fn accept(&mut self, id: LinkId, link: &mut EdgeLink, message: EdgeToWorker) {
        match message {
            EdgeToWorker::PlayerJoin(join) => {
                // A player who joins through another link than the one they belong to
                // has connected anew, and the link they had may not have noticed yet
                // that they are gone. The new connection replaces the old one.
                let previous = self.players.insert(join.player, id);
                if previous.is_some_and(|previous| previous != id) {
                    self.leave(join.player);
                }
                self.inputs.change(PlayerChange::Join(join));
            }
            EdgeToWorker::PlayerLeave { player } => {
                // What another link has to say is about a connection the player had
                // before, which must not end the one they have now.
                if self.players.get(&player) == Some(&id) {
                    self.players.remove(&player);
                    self.leave(player);
                }
            }
            EdgeToWorker::PlayerArrive { player, transfer } => {
                // The region keeps a player who is here already and gives up the entity
                // that was on its way, so such a player stays with the link they have.
                if let Entry::Vacant(entry) = self.players.entry(player) {
                    entry.insert(id);
                    self.status.arrivals.fetch_add(1, Ordering::Relaxed);
                    info!(
                        name = %transfer.name,
                        entity_id = transfer.entity_id.0,
                        "player arrived from another region"
                    );
                }
                self.inputs.change(PlayerChange::Arrive(player, transfer));
            }
            EdgeToWorker::Remote(action) => {
                self.inputs.remote_actions.push(action);
                self.remote_from.push(id);
            }
            EdgeToWorker::Discard { entity, chunk } => {
                self.inputs.change(PlayerChange::Discard { entity, chunk });
            }
            EdgeToWorker::Input {
                player,
                number,
                input,
            } => {
                // As with leaving: only the link a player belongs to acts for them.
                if self.players.get(&player) == Some(&id) {
                    self.inputs.input(player, number, input);
                }
            }
            EdgeToWorker::Subscribe { chunks } => {
                let area = self.region.area();
                for position in chunks {
                    // Chunks elsewhere are another region's to show.
                    if area.contains(position) && link.subscriptions.insert(position) {
                        self.inputs.tickets_added.push(position);
                        link.awaiting_snapshot.insert(position);
                    }
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
        }
    }

    /// Queues that `player` leaves the region. See [`TickInputs::change`] for what
    /// becomes of what they did earlier in this step.
    fn leave(&mut self, player: PlayerId) {
        self.inputs.change(PlayerChange::Leave(player));
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
    /// `self.links`: its players leave and its chunks are no longer needed.
    fn let_go(&mut self, id: LinkId, link: EdgeLink) {
        let players: Vec<_> = self
            .players
            .iter()
            .filter(|(_, owner)| **owner == id)
            .map(|(player, _)| *player)
            .collect();
        for player in players {
            self.players.remove(&player);
            self.leave(player);
        }
        for position in link.subscriptions {
            self.release(position);
        }
    }

    /// Sends `message` to a link without ever waiting for it. An edge that cannot keep
    /// up loses its link rather than slowing the region down. Returns whether the
    /// message is on its way.
    fn publish(&mut self, id: LinkId, message: WorkerToEdge) -> bool {
        let Some(link) = self.links.get(&id) else {
            return false;
        };
        match link.end.try_send(message) {
            Ok(()) => true,
            Err(error) => {
                warn!(link = id.0, %error, "giving up on a link to an edge");
                if let Some(link) = self.links.remove(&id) {
                    self.let_go(id, link);
                }
                false
            }
        }
    }

    /// Passes on what concerns a single player to the link they belong to.
    fn tell(&mut self, tick: u64, player: PlayerId, event: PlayerEvent) {
        let departed = match &event {
            PlayerEvent::Departed(transfer) => {
                self.status.departures.fetch_add(1, Ordering::Relaxed);
                info!(
                    name = %transfer.name,
                    entity_id = transfer.entity_id.0,
                    "player departed to another region"
                );
                let position = transfer.pose.position;
                Some(RegionEvent::EntityRemoved {
                    entity: transfer.entity_id,
                    chunk: ChunkPos::containing(position.x, position.z),
                })
            }
            _ => None,
        };
        let last = matches!(event, PlayerEvent::Departed(_) | PlayerEvent::Refused);

        // A player without a link had one that was lost earlier in this step.
        let delivered = match self.players.get(&player).copied() {
            Some(id) => self.publish(id, WorkerToEdge::ToPlayer { player, event }),
            None => false,
        };
        // Forgotten only now, so that the event still found its way. A refused player
        // who is in the region nonetheless arrived within the same tick, and is kept.
        if last && self.region.player(player).is_none() {
            self.players.remove(&player);
        }

        if !delivered && let Some(removed) = departed {
            // Nobody will pass the player on to the region they walked into. A departing
            // entity is not reported as removed, because it lives on over there, so this
            // one would stay on the screens of those who saw it leave. It left for where
            // no link of this region is subscribed, so all of them are told.
            warn!(
                player = %player.0,
                "a departed player cannot be passed on; removing their entity"
            );
            let remaining: Vec<_> = self.links.keys().copied().collect();
            for id in remaining {
                let delta = WorkerToEdge::TickDelta {
                    tick,
                    events: vec![removed.clone()],
                };
                self.publish(id, delta);
            }
        }
    }

    /// Sends a link the snapshots it is waiting for of chunks that are loaded by now.
    fn send_snapshots(&mut self, id: LinkId, tick: u64) {
        let Some(link) = self.links.get_mut(&id) else {
            return;
        };
        let ready: Vec<_> = link
            .awaiting_snapshot
            .iter()
            .filter_map(|position| Some((*position, self.region.chunk(*position)?.clone())))
            .collect();
        for (position, _) in &ready {
            link.awaiting_snapshot.remove(position);
        }
        for (position, chunk) in ready {
            let entities = self
                .region
                .entities()
                .filter(|entity| entity.chunk() == position)
                .collect();
            let snapshot = WorkerToEdge::ChunkSnapshot {
                position,
                tick,
                chunk,
                entities,
            };
            if !self.publish(id, snapshot) {
                return;
            }
        }
    }
}

/// A region running on its own thread.
pub struct Worker {
    thread: JoinHandle<()>,
    stop: Arc<AtomicBool>,
}

impl Worker {
    /// Starts ticking `runner` on a new thread.
    pub fn spawn(runner: RegionRunner) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread = thread::Builder::new()
            .name("region".to_owned())
            .spawn({
                let stop = Arc::clone(&stop);
                let mut runner = runner;
                move || {
                    runner.run(&stop);
                    info!("region stopped");
                }
            })
            .expect("spawning the region thread");
        Self { thread, stop }
    }

    /// Stops ticking and waits for the thread to finish its current tick.
    pub fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        // A panic in the region thread has already been reported by the panic hook.
        let _ = self.thread.join();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use clustine_rpc::link::{self, EdgeEnd};
    use clustine_sim::api::{
        EntityKind, EntityState, HOTBAR_SLOTS, PlayerInput, PlayerJoin, Pose, RegionEvent,
    };
    use clustine_sim::{PlayerTransfer, RegionConfig, RemoteAction, RemoteStep};
    use clustine_world::{BlockPos, ChunkArea, EntityId, EntityIds, Vec3};
    use clustine_worldgen::FlatGenerator;
    use tokio::time::timeout;
    use uuid::Uuid;

    use super::*;

    /// The two kinds of link: a direct one, and one that serialises every message the
    /// way a link between two processes does.
    const KINDS: [fn(usize) -> (EdgeEnd, WorkerEnd); 2] = [link::in_process, link::framed];

    /// The western one of two regions: it ends where the chunks with x = 1 begin.
    const WEST: ChunkArea = ChunkArea {
        min_x: None,
        max_x: Some(1),
    };

    /// Where players enter the flat world.
    const SPAWN: Vec3 = Vec3::new(0.5, -60.0, 0.5);

    fn config(area: ChunkArea) -> RegionConfig {
        RegionConfig {
            spawn: SPAWN,
            area,
            entity_ids: EntityIds::block(0).unwrap(),
            starting_hotbar: [None; HOTBAR_SLOTS],
        }
    }

    /// A runner for a region of a flat world that only lasts as long as the runner.
    fn runner_of(config: RegionConfig, link: WorkerEnd) -> RegionRunner {
        let store = clustine_worldstore::spawn(Arc::new(FlatGenerator::classic()));
        RegionRunner::new(Region::new(config), link, store)
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

    /// Steps `runner` until `done` holds, giving the store thread and links that
    /// serialise time to deliver.
    fn step_until(runner: &mut RegionRunner, mut done: impl FnMut(&RegionRunner) -> bool) {
        for _ in 0..2000 {
            runner.step();
            if done(runner) {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("the condition never became true");
    }

    /// Steps `runner` until `edge` has been sent something, and returns that.
    fn step_for(runner: &mut RegionRunner, edge: &mut EdgeEnd) -> WorkerToEdge {
        for _ in 0..2000 {
            if let Ok(Some(message)) = edge.try_recv() {
                return message;
            }
            runner.step();
            thread::sleep(Duration::from_millis(1));
        }
        panic!("the worker sent nothing");
    }

    /// Everything `edge` has been sent and has not looked at yet.
    fn received(edge: &mut EdgeEnd) -> Vec<WorkerToEdge> {
        let mut messages = Vec::new();
        while let Ok(Some(message)) = edge.try_recv() {
            messages.push(message);
        }
        messages
    }

    async fn next(edge: &mut EdgeEnd) -> WorkerToEdge {
        timeout(Duration::from_secs(10), edge.recv())
            .await
            .expect("the worker sent nothing")
            .expect("the worker closed the link")
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

    /// A stand-in edge joins and subscribes, over a direct and over a serialising link.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_joining_player_is_spawned_and_sent_the_chunks_around() {
        for (mut edge, worker_end) in [link::in_process(256), link::framed(256)] {
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
        let (mut edge, worker_end) = link::in_process(256);
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
        runner.step();
        received(&mut edge);

        // Within the subscribed chunk, then out of it: both can be seen from it.
        for x in [5.0, 20.0] {
            edge.send(walk(player(), x)).await.unwrap();
            runner.step();
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
        runner.step();
        assert_eq!(edge.try_recv(), Ok(None));
    }

    /// An entity that walks into view from somewhere the edge was not watching is
    /// introduced in full, because the edge has never heard of it.
    #[tokio::test]
    async fn an_entity_entering_the_subscribed_area_is_introduced() {
        let (mut edge, worker_end) = link::in_process(256);
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
        runner.step();
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
        runner.step();
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
        runner.step();
        assert!(matches!(
            edge.try_recv(),
            Ok(Some(WorkerToEdge::TickDelta { events, .. }))
                if matches!(events[..], [RegionEvent::EntityMoved { .. }])
        ));
        edge.send(EdgeToWorker::PlayerLeave { player: player() })
            .await
            .unwrap();
        runner.step();
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
            let store = clustine_worldstore::spawn(Arc::new(FlatGenerator::classic()));
            let region = Region::new(config(ChunkArea::EVERYWHERE));
            let runner = RegionRunner::without_links(region, store);
            let (links, status) = (runner.links(), runner.status());
            let worker = Worker::spawn(runner);
            let origin = ChunkPos::new(0, 0);

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
                    chunks: vec![origin],
                })
                .await
                .unwrap();
            let (position, entities) = snapshot(next(&mut second).await);
            assert_eq!(position, origin);
            // The player of the other edge stands there.
            assert_eq!(entities.len(), 1);
            assert!(status.tick.load(Ordering::Relaxed) > 0);
            assert_eq!(status.players.load(Ordering::Relaxed), 1);

            // A link that comes too late is closed rather than left waiting.
            worker.stop();
            let (mut late, late_end) = connect(256);
            links.attach(late_end);
            let closed = timeout(Duration::from_secs(10), late.recv()).await;
            assert_eq!(closed, Ok(None));
        }
    }

    /// Joins a player, subscribes to the chunk they stand in and waits until it is loaded.
    async fn joined(edge: &EdgeEnd, runner: &mut RegionRunner) {
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
        let (edge, worker_end) = link::in_process(256);
        let mut runner = runner(worker_end);
        let origin = ChunkPos::new(0, 0);
        joined(&edge, &mut runner).await;

        edge.send(dig(1)).await.unwrap();
        runner.step();
        let changed = runner.region().chunk(origin).unwrap().clone();
        assert_eq!(changed.get(1, -61, 0), Some(clustine_data::blocks::AIR));

        edge.send(EdgeToWorker::Unsubscribe {
            chunks: vec![origin],
        })
        .await
        .unwrap();
        runner.step();
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

    /// Stopping stores what is still loaded, so that another runner on the same world
    /// finds it.
    #[tokio::test]
    async fn stopping_stores_changed_chunks() {
        let directory = tempfile::tempdir().unwrap();
        let on_disk = |link| {
            let generator = FlatGenerator::classic();
            let region = Region::new(config(ChunkArea::EVERYWHERE));
            let store = clustine_worldstore::spawn_local(directory.path(), Arc::new(generator));
            RegionRunner::new(region, link, store.unwrap())
        };
        let origin = ChunkPos::new(0, 0);

        let (edge, worker_end) = link::in_process(256);
        let mut first = on_disk(worker_end);
        joined(&edge, &mut first).await;
        edge.send(dig(1)).await.unwrap();
        edge.send(dig(2)).await.unwrap();
        first.step();
        let changed = first.region().chunk(origin).unwrap().clone();
        // The player is still there and the chunk still loaded when the runner stops.
        first.run(&AtomicBool::new(true));
        drop((first, edge));

        let (edge, worker_end) = link::in_process(256);
        let mut second = on_disk(worker_end);
        joined(&edge, &mut second).await;
        assert_eq!(second.region().chunk(origin), Some(&changed));
        assert_eq!(changed.get(2, -61, 0), Some(clustine_data::blocks::AIR));
    }

    /// Changes to a chunk that stays loaded reach the stored world at the next
    /// checkpoint, which also empties the log.
    #[tokio::test]
    async fn checkpoints_save_loaded_chunks_and_empty_the_log() {
        let directory = tempfile::tempdir().unwrap();
        let generator = FlatGenerator::classic();
        let region = Region::new(RegionConfig {
            spawn: Vec3::new(0.5, f64::from(generator.surface_y()), 0.5),
            area: ChunkArea::EVERYWHERE,
            entity_ids: EntityIds::block(0).unwrap(),
            starting_hotbar: [None; HOTBAR_SLOTS],
        });
        let store = clustine_worldstore::spawn_local(directory.path(), Arc::new(generator));
        let (edge, worker_end) = link::in_process(256);
        let mut runner =
            RegionRunner::new(region, worker_end, store.unwrap()).with_checkpoint_interval(50);
        joined(&edge, &mut runner).await;
        let log_length = || {
            std::fs::metadata(directory.path().join("logs/0.wal"))
                .unwrap()
                .len()
        };
        let manifest = directory
            .path()
            .join("manifests/overworld/0.0/0.0.manifest");

        // Just after a checkpoint, so that the next one is 50 ticks away.
        step_until(&mut runner, |runner| {
            runner.region().tick_number() % 50 == 1
        });
        edge.send(dig(1)).await.unwrap();
        runner.step();
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
        let (edge, worker_end) = link::in_process(256);
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
        runner.step();
        assert_eq!(runner.region().loaded_chunk_count(), 4);
        for position in kept {
            assert!(runner.region().chunk(position).is_some());
        }
    }

    #[tokio::test]
    async fn subscribing_twice_sends_one_snapshot() {
        let (mut edge, worker_end) = link::in_process(256);
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
        runner.step();

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
        runner.step();
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

    #[tokio::test(flavor = "multi_thread")]
    async fn a_link_whose_edge_is_gone_is_dropped_and_the_runner_goes_on() {
        for connect in KINDS {
            let (edge, worker_end) = connect(256);
            let mut runner = runner(worker_end);
            joined(&edge, &mut runner).await;
            assert_eq!(runner.region().player_count(), 1);

            // Found out before the tick, so the tick is already one without the edge:
            // its player has left and its chunk is no longer needed.
            drop(edge);
            step_until(&mut runner, |runner| runner.links.is_empty());
            assert_eq!(runner.region().player_count(), 0);
            assert_eq!(runner.region().loaded_chunk_count(), 0);
            assert!(runner.players.is_empty());

            let ticks = runner.region().tick_number();
            runner.step();
            assert_eq!(runner.region().tick_number(), ticks + 1);
        }
    }

    #[tokio::test]
    async fn an_edge_that_does_not_keep_up_loses_its_link_and_the_runner_goes_on() {
        // Room for a single message, and nobody reads it.
        let (mut edge, worker_end) = link::in_process(1);
        let mut runner = runner(worker_end);
        edge.try_send(join(player(), "Notch")).unwrap();
        runner.step();
        edge.try_send(EdgeToWorker::Subscribe { chunks: square(1) })
            .unwrap();

        // What the player was told fills the link, so the chunks cannot be sent.
        step_until(&mut runner, |runner| runner.links.is_empty());
        assert_eq!(runner.region().player_count(), 1);
        assert!(runner.region().loaded_chunk_count() > 0);

        // The player leaves and the chunks are let go with the next tick, and the
        // region ticks on.
        runner.step();
        assert_eq!(runner.region().player_count(), 0);
        assert_eq!(runner.region().loaded_chunk_count(), 0);
        let ticks = runner.region().tick_number();
        runner.step();
        assert_eq!(runner.region().tick_number(), ticks + 1);

        // The edge finds its link closed after what did fit.
        assert!(matches!(
            edge.recv().await,
            Some(WorkerToEdge::ToPlayer { .. })
        ));
        assert_eq!(edge.recv().await, None);
    }

    /// When a link turns out to be of no use while it is told what a tick did, that tick
    /// has run. What follows from losing the link happens in the next one.
    #[tokio::test]
    async fn a_link_lost_while_publishing_has_its_players_removed_in_the_next_tick() {
        // Room for the snapshots of two chunks, which are never read.
        let (slow, slow_end) = link::in_process(2);
        let (mut watcher, watcher_end) = link::in_process(256);
        let mut runner = runner(slow_end);
        runner.links().attach(watcher_end);
        let (origin, beside) = (ChunkPos::new(0, 0), ChunkPos::new(0, 1));

        slow.send(EdgeToWorker::Subscribe {
            chunks: vec![origin, beside],
        })
        .await
        .unwrap();
        watcher
            .send(EdgeToWorker::Subscribe {
                chunks: vec![origin],
            })
            .await
            .unwrap();
        step_until(&mut runner, |runner| {
            runner.region().loaded_chunk_count() == 2
        });
        assert_eq!(snapshot(step_for(&mut runner, &mut watcher)).0, origin);
        assert_eq!(runner.links.len(), 2);

        // The slow edge cannot be told about its own player, who has joined by then.
        slow.send(join(player(), "Notch")).await.unwrap();
        runner.step();
        assert_eq!(runner.links.len(), 1);
        assert_eq!(runner.region().player_count(), 1);
        assert_eq!(runner.region().loaded_chunk_count(), 2);

        // A tick later the player is gone, and so is the chunk that only the slow edge
        // had asked for.
        runner.step();
        assert_eq!(runner.region().player_count(), 0);
        assert_eq!(runner.region().loaded_chunk_count(), 1);
        assert!(runner.region().chunk(origin).is_some());

        let seen = received(&mut watcher);
        let [
            WorkerToEdge::TickDelta {
                tick: came,
                events: first,
            },
            WorkerToEdge::TickDelta {
                tick: went,
                events: second,
            },
        ] = &seen[..]
        else {
            panic!("expected two deltas, got {seen:?}");
        };
        assert!(matches!(first[..], [RegionEvent::EntitySpawned(_)]));
        let removed = RegionEvent::EntityRemoved {
            entity: EntityId(1),
            chunk: origin,
        };
        assert_eq!(*second, [removed]);
        assert_eq!(*went, came + 1);
    }

    /// An edge that loses its link connects again and brings its players back. When the
    /// region sees all of that between two ticks, the players have to end up in it.
    #[tokio::test]
    async fn a_player_whose_edge_reconnects_within_one_tick_is_in_the_region_afterwards() {
        let (old, old_end) = link::in_process(256);
        let mut runner = runner(old_end);
        joined(&old, &mut runner).await;
        assert_eq!(
            runner.region().player(player()),
            Some((EntityId(1), Pose::at(SPAWN)))
        );

        // The last the old link carried is something the player did.
        old.send(walk(player(), 3.0)).await.unwrap();
        drop(old);
        let (mut new, new_end) = link::in_process(256);
        runner.links().attach(new_end);
        new.send(join(player(), "Notch")).await.unwrap();
        new.send(EdgeToWorker::Subscribe {
            chunks: vec![ChunkPos::new(0, 0)],
        })
        .await
        .unwrap();
        runner.step();

        // The player left through the old link first and then joined through the new
        // one, which makes them a new entity that has not done anything yet.
        assert_eq!(runner.links.len(), 1);
        assert_eq!(
            runner.region().player(player()),
            Some((EntityId(2), Pose::at(SPAWN)))
        );
        // The chunk was let go of and asked for within the same tick, so it stayed.
        assert_eq!(runner.region().loaded_chunk_count(), 1);
        let spawned = received(&mut new).into_iter().find_map(|message| {
            let WorkerToEdge::ToPlayer { event, .. } = message else {
                return None;
            };
            Some(event)
        });
        assert!(matches!(
            spawned,
            Some(PlayerEvent::Spawned {
                entity_id: EntityId(2),
                ..
            })
        ));
    }

    /// The region can also learn of a player's new connection before it learns that the
    /// old one is gone, when the link they had lingers or is served later.
    #[tokio::test]
    async fn a_player_who_joins_through_another_link_is_taken_over_by_it() {
        let (first, first_end) = link::in_process(256);
        let (mut second, second_end) = link::in_process(256);
        let mut runner = runner(first_end);
        runner.links().attach(second_end);
        let entity = |runner: &RegionRunner| Some(runner.region().player(player())?.0);

        first.send(join(player(), "Notch")).await.unwrap();
        runner.step();
        assert_eq!(entity(&runner), Some(EntityId(1)));

        // The new entity does not take the step that came through the first link.
        first.send(walk(player(), 3.0)).await.unwrap();
        second.send(join(player(), "Notch")).await.unwrap();
        runner.step();
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
        runner.step();
        assert_eq!(
            runner.region().player(player()),
            Some((EntityId(2), Pose::at(SPAWN)))
        );

        // The other way round: they join through the link that is served first, and
        // the link they belong to ends within the same tick.
        first.send(join(player(), "Notch")).await.unwrap();
        drop(second);
        runner.step();
        assert_eq!(runner.links.len(), 1);
        assert_eq!(entity(&runner), Some(EntityId(3)));
    }

    #[tokio::test]
    async fn only_the_link_a_player_belongs_to_acts_for_them() {
        let (owner, owner_end) = link::in_process(256);
        let (stranger, stranger_end) = link::in_process(256);
        let mut runner = runner(owner_end);
        runner.links().attach(stranger_end);
        let leave = || EdgeToWorker::PlayerLeave { player: player() };

        owner.send(join(player(), "Notch")).await.unwrap();
        runner.step();
        stranger.send(walk(player(), 5.0)).await.unwrap();
        stranger.send(leave()).await.unwrap();
        runner.step();
        assert_eq!(
            runner.region().player(player()),
            Some((EntityId(1), Pose::at(SPAWN)))
        );

        owner.send(leave()).await.unwrap();
        runner.step();
        assert_eq!(runner.region().player_count(), 0);
        assert!(runner.players.is_empty());
    }

    /// The region handles what players did only after all of them have joined or left.
    /// A player who is back within the tick must not begin with what they did before
    /// they left, least of all with the numbers of those inputs.
    #[tokio::test]
    async fn a_player_who_is_back_within_the_tick_starts_afresh() {
        let (edge, worker_end) = link::in_process(256);
        let mut runner = runner(worker_end);
        edge.send(join(player(), "Notch")).await.unwrap();
        runner.step();

        edge.send(walk_as(player(), 900, 3.0)).await.unwrap();
        edge.send(EdgeToWorker::PlayerLeave { player: player() })
            .await
            .unwrap();
        edge.send(join(player(), "Notch")).await.unwrap();
        runner.step();
        assert_eq!(
            runner.region().player(player()),
            Some((EntityId(2), Pose::at(SPAWN)))
        );

        // Their new connection numbers what they do from the start again.
        edge.send(walk_as(player(), 1, 4.0)).await.unwrap();
        runner.step();
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
            let departed = WorkerToEdge::ToPlayer {
                player: player(),
                event: PlayerEvent::Departed(leaving),
            };
            assert_eq!(message, departed);
            assert_eq!(runner.region().player_count(), 0);
            assert_eq!(status.departures.load(Ordering::Relaxed), 1);
            // The region is done with the player, and nobody else was told anything.
            assert!(runner.players.is_empty());
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
                WorkerToEdge::Remote(request)
            );
            // Nobody is told that it was handled, and the other edge hears nothing.
            runner.step();
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
                WorkerToEdge::RemoteDone {
                    player: other_player(),
                    sequence: 3,
                }
            );
            // The first edge watches the chunk too and is told of the change, but not
            // that anything is done: it did not ask.
            assert_eq!(events(step_for(&mut runner, &mut edge)), changed);
            runner.step();
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
                WorkerToEdge::Remote(RemoteAction {
                    player: other_player(),
                    sequence: 4,
                    step: RemoteStep::Place {
                        target,
                        block: stone,
                        placer,
                    },
                })
            );
            runner.step();
            assert_eq!(received(&mut edge), []);
            assert_eq!(received(&mut other), []);
        }
    }

    /// A transfer can still be on its way when the player has long connected anew and
    /// joined. The region keeps the player it has.
    #[tokio::test]
    async fn an_arrival_does_not_take_a_player_from_the_link_they_belong_to() {
        let (first, first_end) = link::in_process(256);
        let (second, second_end) = link::in_process(256);
        let mut runner = runner(first_end);
        runner.links().attach(second_end);
        let status = runner.status();
        let leave = || EdgeToWorker::PlayerLeave { player: player() };

        first.send(join(player(), "Notch")).await.unwrap();
        runner.step();
        second
            .send(EdgeToWorker::PlayerArrive {
                player: player(),
                transfer: transfer(EntityIds::block(3).unwrap().first),
            })
            .await
            .unwrap();
        runner.step();
        assert_eq!(
            runner.region().player(player()),
            Some((EntityId(1), Pose::at(SPAWN)))
        );
        assert_eq!(status.arrivals.load(Ordering::Relaxed), 0);

        // The player is still the first link's to take out of the region.
        second.send(leave()).await.unwrap();
        runner.step();
        assert_eq!(runner.region().player_count(), 1);
        first.send(leave()).await.unwrap();
        runner.step();
        assert_eq!(runner.region().player_count(), 0);
    }

    /// A player steps out of the region, but the link that would pass them on is lost.
    /// Nothing will come of their entity, so everyone else is told that it is gone.
    #[tokio::test]
    async fn a_departure_that_cannot_be_delivered_removes_the_entity_for_the_other_links() {
        let (origin, beside) = (ChunkPos::new(0, 0), ChunkPos::new(0, 1));
        // The link is lost either on the departure itself or on what is sent before it.
        for subscribed in [false, true] {
            // Room for two messages, which are never read.
            let (slow, slow_end) = link::in_process(2);
            let (mut watcher, watcher_end) = link::in_process(256);
            let mut runner = runner_of(config(WEST), slow_end);
            runner.links().attach(watcher_end);

            if subscribed {
                // Two snapshots fill the link. The player joins and steps out within
                // one tick, of which the slow edge cannot even be sent the events.
                slow.send(EdgeToWorker::Subscribe {
                    chunks: vec![origin, beside],
                })
                .await
                .unwrap();
                step_until(&mut runner, |runner| {
                    runner.region().loaded_chunk_count() == 2
                });
                slow.send(join(player(), "Notch")).await.unwrap();
            } else {
                // That the player has joined and that their digging was handled fills
                // the link, so that their departure does not fit.
                slow.send(join(player(), "Notch")).await.unwrap();
                runner.step();
                slow.send(dig(1)).await.unwrap();
                runner.step();
            }
            assert_eq!(runner.links.len(), 2);
            slow.send(walk(player(), 20.0)).await.unwrap();
            runner.step();
            assert_eq!(runner.links.len(), 1);
            assert_eq!(runner.region().player_count(), 0);
            assert!(runner.players.is_empty());

            // The watcher is not subscribed to anything, least of all to the chunk the
            // player walked into, which is not this region's.
            let removed = RegionEvent::EntityRemoved {
                entity: EntityId(1),
                chunk: ChunkPos::new(1, 0),
            };
            let delta = WorkerToEdge::TickDelta {
                tick: runner.region().tick_number(),
                events: vec![removed],
            };
            assert_eq!(received(&mut watcher), [delta]);

            // That the lost link's player leaves in the next tick changes nothing more.
            runner.step();
            assert!(received(&mut watcher).is_empty());
        }
    }

    #[tokio::test]
    async fn a_refused_player_is_told_through_their_link_and_forgotten() {
        let (mut edge, worker_end) = link::in_process(256);
        // A region with a single entity id to give out.
        let entity_ids = EntityIds {
            first: EntityId(1),
            end: EntityId(2),
        };
        let config = RegionConfig {
            entity_ids,
            ..config(ChunkArea::EVERYWHERE)
        };
        let mut runner = runner_of(config, worker_end);

        edge.send(join(player(), "Notch")).await.unwrap();
        edge.send(join(other_player(), "Jeb")).await.unwrap();
        runner.step();
        let told = received(&mut edge);
        let refused = WorkerToEdge::ToPlayer {
            player: other_player(),
            event: PlayerEvent::Refused,
        };
        assert!(
            matches!(&told[..], [WorkerToEdge::ToPlayer { .. }, last] if *last == refused),
            "{told:?}"
        );
        assert_eq!(runner.players.keys().collect::<Vec<_>>(), [&player()]);
    }

    #[tokio::test]
    async fn the_status_follows_the_region() {
        let (edge, worker_end) = link::in_process(256);
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
            runner.step();
            assert_eq!((population(), traffic()), ((2, 1), (1, 0)));
        }

        // Both step out of the region; one of them comes back.
        edge.send(walk(player(), 20.0)).await.unwrap();
        edge.send(walk_as(other_player(), 8, 20.0)).await.unwrap();
        runner.step();
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
        runner.step();
        assert_eq!((population(), traffic()), ((1, 0), (2, 2)));
        assert_eq!(read(&status.tick), runner.region().tick_number());
    }

    #[test]
    fn ticks_keep_their_pace() {
        let (_edge, worker_end) = link::in_process(256);
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
}
