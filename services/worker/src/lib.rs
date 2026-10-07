//! Worker service: ticks the regions it owns.
//!
//! A [`RegionRunner`] connects a region to the outside: it turns the messages of an edge
//! and the answers of the world store into tick inputs, runs the tick, and publishes
//! what resulted. The simulation itself never waits for anything.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use clustine_rpc::link::WorkerEnd;
use clustine_rpc::{EdgeToWorker, StoreReply, StoreRequest, WorkerToEdge};
use clustine_sim::api::RegionEvent;
use clustine_sim::{PlayerChange, Region, TickInputs};
use clustine_world::ChunkPos;
use clustine_worldstore::StoreHandle;
use tracing::{error, info};

/// The length of a tick: 20 ticks per second.
pub const TICK: Duration = Duration::from_millis(50);

/// Ticks between two checkpoints unless set otherwise: five minutes.
pub const DEFAULT_CHECKPOINT_INTERVAL: u64 = 5 * 60 * 20;

/// A runner that has fallen further behind than this many ticks skips them instead of
/// trying to catch up.
const MAX_CATCH_UP_TICKS: u32 = 10;

/// Runs one region for one edge.
pub struct RegionRunner {
    region: Region,
    link: WorkerEnd,
    store: StoreHandle,
    /// Chunks the edge is subscribed to.
    subscriptions: BTreeSet<ChunkPos>,
    /// Subscribed chunks the edge has not been sent a snapshot of yet.
    awaiting_snapshot: BTreeSet<ChunkPos>,
    /// Loaded chunks that have changed since they were loaded or last stored.
    unsaved: BTreeSet<ChunkPos>,
    /// How many ticks pass between two checkpoints.
    checkpoint_interval: u64,
}

impl RegionRunner {
    pub fn new(region: Region, link: WorkerEnd, store: StoreHandle) -> Self {
        Self {
            region,
            link,
            store,
            subscriptions: BTreeSet::new(),
            awaiting_snapshot: BTreeSet::new(),
            unsaved: BTreeSet::new(),
            checkpoint_interval: DEFAULT_CHECKPOINT_INTERVAL,
        }
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

    /// Runs one tick with everything that has arrived since the last one. Returns false
    /// once the link to the edge no longer works, after which the runner is of no use.
    pub fn step(&mut self) -> bool {
        let mut inputs = TickInputs::default();
        loop {
            match self.link.try_recv() {
                Ok(Some(message)) => self.accept(message, &mut inputs),
                Ok(None) => break,
                Err(_) => return false,
            }
        }
        while let Some(StoreReply::Loaded { position, chunk }) = self.store.try_reply() {
            inputs.chunks_loaded.push((position, chunk));
        }

        let output = self.region.tick(&inputs);

        for position in output.chunk_requests {
            self.store.request(StoreRequest::Load { position });
        }
        // The edge only hears about what happens in chunks it subscribed to.
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

        let mut events = Vec::new();
        for event in output.events {
            let [current, previous] = event.chunks();
            let visible_now = self.subscriptions.contains(&current);
            let visible_before = self.subscriptions.contains(&previous);
            match event {
                // An entity coming in from where the edge could not see it is new to
                // the edge, which needs to know what it is.
                RegionEvent::EntityMoved { entity, .. } if visible_now && !visible_before => {
                    if let Some(state) = self.region.entity(entity) {
                        events.push(RegionEvent::EntitySpawned(state));
                    }
                }
                event if visible_now || visible_before => events.push(event),
                _ => {}
            }
        }
        if !events.is_empty() {
            let delta = WorkerToEdge::TickDelta {
                tick: output.tick,
                events,
            };
            if !self.publish(delta) {
                return false;
            }
        }

        // After the events, so that a player is told that their action was handled only
        // once they have been told what it did.
        for (player, event) in output.player_events {
            if !self.publish(WorkerToEdge::ToPlayer { player, event }) {
                return false;
            }
        }

        // Snapshots come last and show the state after this tick, so they include what
        // the events above already said. The edge has to cope with hearing it twice.
        let ready: Vec<_> = self
            .awaiting_snapshot
            .iter()
            .filter_map(|position| Some((*position, self.region.chunk(*position)?.clone())))
            .collect();
        for (position, chunk) in ready {
            self.awaiting_snapshot.remove(&position);
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
            if !self.publish(snapshot) {
                return false;
            }
        }
        true
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

    /// Ticks 20 times per second until the edge is gone or `stop` is set, then stores
    /// what has not been stored yet.
    pub fn run(&mut self, stop: &AtomicBool) {
        let mut deadline = Instant::now() + TICK;
        while !stop.load(Ordering::Relaxed) && self.step() {
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

    fn accept(&mut self, message: EdgeToWorker, inputs: &mut TickInputs) {
        match message {
            EdgeToWorker::PlayerJoin(join) => {
                inputs.player_changes.push(PlayerChange::Join(join));
            }
            EdgeToWorker::PlayerLeave { player } => {
                inputs.player_changes.push(PlayerChange::Leave(player));
            }
            EdgeToWorker::PlayerArrive { player, transfer } => {
                inputs
                    .player_changes
                    .push(PlayerChange::Arrive(player, transfer));
            }
            EdgeToWorker::Discard { entity, chunk } => {
                inputs
                    .player_changes
                    .push(PlayerChange::Discard { entity, chunk });
            }
            EdgeToWorker::Input {
                player,
                number,
                input,
            } => inputs.inputs.push((player, number, input)),
            EdgeToWorker::Subscribe { chunks } => {
                for position in chunks {
                    if self.subscriptions.insert(position) {
                        inputs.tickets_added.push(position);
                        self.awaiting_snapshot.insert(position);
                    }
                }
            }
            EdgeToWorker::Unsubscribe { chunks } => {
                for position in chunks {
                    if self.subscriptions.remove(&position) {
                        // The region drops the chunk at the start of the coming tick,
                        // before anything else can change it, so this is its final state.
                        self.save(position);
                        inputs.tickets_removed.push(position);
                        self.awaiting_snapshot.remove(&position);
                    }
                }
            }
        }
    }

    /// Sends `message` to the edge without ever waiting for it. An edge that cannot keep
    /// up loses its link rather than slowing the region down.
    fn publish(&mut self, message: WorkerToEdge) -> bool {
        match self.link.try_send(message) {
            Ok(()) => true,
            Err(error) => {
                error!(%error, "giving up on the link to the edge");
                false
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
    use std::sync::atomic::AtomicU64;
    use std::time::Duration;

    use clustine_rpc::link::{self, EdgeEnd};
    use clustine_sim::RegionConfig;
    use clustine_sim::api::{
        EntityKind, HOTBAR_SLOTS, PlayerEvent, PlayerInput, PlayerJoin, RegionEvent,
    };
    use clustine_world::{BlockPos, ChunkArea, EntityId, EntityIds, PlayerId, Vec3};
    use clustine_worldgen::FlatGenerator;
    use tokio::time::timeout;
    use uuid::Uuid;

    use super::*;

    fn runner(link: WorkerEnd) -> RegionRunner {
        let generator = FlatGenerator::classic();
        let region = Region::new(RegionConfig {
            spawn: Vec3::new(0.5, f64::from(generator.surface_y()), 0.5),
            area: ChunkArea::EVERYWHERE,
            entity_ids: EntityIds::block(0).unwrap(),
            starting_hotbar: [None; HOTBAR_SLOTS],
        });
        RegionRunner::new(
            region,
            link,
            clustine_worldstore::spawn(Arc::new(generator)),
        )
    }

    fn player() -> PlayerId {
        PlayerId(Uuid::from_u128(1))
    }

    /// Numbers inputs the way an edge does: in the order they are made.
    fn next_number() -> u64 {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    fn square(radius: i32) -> Vec<ChunkPos> {
        (-radius..=radius)
            .flat_map(|x| (-radius..=radius).map(move |z| ChunkPos::new(x, z)))
            .collect()
    }

    /// Steps `runner` until `done` holds, giving the store thread time to answer.
    fn step_until(runner: &mut RegionRunner, mut done: impl FnMut(&RegionRunner) -> bool) {
        for _ in 0..2000 {
            assert!(runner.step());
            if done(runner) {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("the condition never became true");
    }

    async fn next(edge: &mut EdgeEnd) -> WorkerToEdge {
        timeout(Duration::from_secs(10), edge.recv())
            .await
            .expect("the worker sent nothing")
            .expect("the worker closed the link")
    }

    /// A stand-in edge joins and subscribes, over a direct and over a serialising link.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_joining_player_is_spawned_and_sent_the_chunks_around() {
        for (mut edge, worker_end) in [link::in_process(256), link::framed(256)] {
            let worker = Worker::spawn(runner(worker_end));

            edge.send(EdgeToWorker::PlayerJoin(PlayerJoin {
                player: player(),
                name: "Notch".to_owned(),
            }))
            .await
            .unwrap();
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
                }
            }
            assert_eq!(
                spawned,
                Some(PlayerEvent::Spawned {
                    entity_id: EntityId(1),
                    position: Vec3::new(0.5, -60.0, 0.5),
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
        let walk_to = |x: f64| EdgeToWorker::Input {
            player: player(),
            number: next_number(),
            input: PlayerInput::Move {
                position: Some(Vec3::new(x, -60.0, 0.5)),
                rotation: None,
                on_ground: true,
            },
        };

        edge.send(EdgeToWorker::PlayerJoin(PlayerJoin {
            player: player(),
            name: "Notch".to_owned(),
        }))
        .await
        .unwrap();
        edge.send(EdgeToWorker::Subscribe {
            chunks: vec![ChunkPos::new(0, 0)],
        })
        .await
        .unwrap();
        step_until(&mut runner, |runner| {
            runner.region().loaded_chunk_count() == 1
        });
        assert!(runner.step());
        while let Ok(Some(_)) = edge.try_recv() {}

        // Within the subscribed chunk, then out of it: both can be seen from it.
        for x in [5.0, 20.0] {
            edge.send(walk_to(x)).await.unwrap();
            assert!(runner.step());
            let Ok(Some(WorkerToEdge::TickDelta { events, .. })) = edge.try_recv() else {
                panic!("expected a delta for the move to x = {x}");
            };
            assert!(matches!(
                events[..],
                [RegionEvent::EntityMoved { pose, .. }] if pose.position.x == x
            ));
        }

        // From one chunk nobody subscribed to into another: nobody is told.
        edge.send(walk_to(40.0)).await.unwrap();
        assert!(runner.step());
        assert_eq!(edge.try_recv(), Ok(None));
    }

    /// An entity that walks into view from somewhere the edge was not watching is
    /// introduced in full, because the edge has never heard of it.
    #[tokio::test]
    async fn an_entity_entering_the_subscribed_area_is_introduced() {
        let (mut edge, worker_end) = link::in_process(256);
        let mut runner = runner(worker_end);
        let walk_to = |x: f64| EdgeToWorker::Input {
            player: player(),
            number: next_number(),
            input: PlayerInput::Move {
                position: Some(Vec3::new(x, -60.0, 0.5)),
                rotation: None,
                on_ground: true,
            },
        };

        // The edge watches a chunk far from where the player enters the world.
        edge.send(EdgeToWorker::PlayerJoin(PlayerJoin {
            player: player(),
            name: "Notch".to_owned(),
        }))
        .await
        .unwrap();
        edge.send(EdgeToWorker::Subscribe {
            chunks: vec![ChunkPos::new(5, 0)],
        })
        .await
        .unwrap();
        step_until(&mut runner, |runner| {
            runner.region().loaded_chunk_count() == 1
        });
        assert!(runner.step());
        let mut seen = Vec::new();
        while let Ok(Some(message)) = edge.try_recv() {
            seen.push(message);
        }
        assert!(
            seen.iter().all(|message| match message {
                WorkerToEdge::ChunkSnapshot { entities, .. } => entities.is_empty(),
                WorkerToEdge::ToPlayer { .. } => true,
                WorkerToEdge::TickDelta { .. } => false,
            }),
            "{seen:?}"
        );

        edge.send(walk_to(85.0)).await.unwrap();
        assert!(runner.step());
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
        edge.send(walk_to(120.0)).await.unwrap();
        assert!(runner.step());
        assert!(matches!(
            edge.try_recv(),
            Ok(Some(WorkerToEdge::TickDelta { events, .. }))
                if matches!(events[..], [RegionEvent::EntityMoved { .. }])
        ));
        edge.send(EdgeToWorker::PlayerLeave { player: player() })
            .await
            .unwrap();
        assert!(runner.step());
        assert_eq!(edge.try_recv(), Ok(None));
    }

    /// Joins a player, subscribes to the chunk they stand in and waits until it is loaded.
    async fn joined(edge: &EdgeEnd, runner: &mut RegionRunner) {
        edge.send(EdgeToWorker::PlayerJoin(PlayerJoin {
            player: player(),
            name: "Notch".to_owned(),
        }))
        .await
        .unwrap();
        edge.send(EdgeToWorker::Subscribe {
            chunks: vec![ChunkPos::new(0, 0)],
        })
        .await
        .unwrap();
        step_until(runner, |runner| runner.region().loaded_chunk_count() == 1);
    }

    fn dig(x: i32) -> EdgeToWorker {
        EdgeToWorker::Input {
            player: player(),
            number: next_number(),
            input: PlayerInput::Dig {
                position: BlockPos::new(x, -61, 0),
                sequence: 1,
            },
        }
    }

    /// A changed chunk that nobody needs any more is stored, and comes back changed.
    #[tokio::test]
    async fn changes_survive_a_chunk_being_unloaded() {
        let (edge, worker_end) = link::in_process(256);
        let mut runner = runner(worker_end);
        let origin = ChunkPos::new(0, 0);
        joined(&edge, &mut runner).await;

        edge.send(dig(1)).await.unwrap();
        assert!(runner.step());
        let changed = runner.region().chunk(origin).unwrap().clone();
        assert_eq!(changed.get(1, -61, 0), Some(clustine_data::blocks::AIR));

        edge.send(EdgeToWorker::Unsubscribe {
            chunks: vec![origin],
        })
        .await
        .unwrap();
        assert!(runner.step());
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

    /// Stopping stores what is still loaded, so that another runner on the same world
    /// finds it.
    #[tokio::test]
    async fn stopping_stores_changed_chunks() {
        let directory = tempfile::tempdir().unwrap();
        let on_disk = |link| {
            let generator = FlatGenerator::classic();
            let region = Region::new(RegionConfig {
                spawn: Vec3::new(0.5, f64::from(generator.surface_y()), 0.5),
                area: ChunkArea::EVERYWHERE,
                entity_ids: EntityIds::block(0).unwrap(),
                starting_hotbar: [None; HOTBAR_SLOTS],
            });
            let store = clustine_worldstore::spawn_local(directory.path(), Arc::new(generator));
            RegionRunner::new(region, link, store.unwrap())
        };
        let origin = ChunkPos::new(0, 0);

        let (edge, worker_end) = link::in_process(256);
        let mut first = on_disk(worker_end);
        joined(&edge, &mut first).await;
        edge.send(dig(1)).await.unwrap();
        edge.send(dig(2)).await.unwrap();
        assert!(first.step());
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
            std::fs::metadata(directory.path().join("wal"))
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
        assert!(runner.step());
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
        assert!(runner.step());
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
        assert!(runner.step());

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
        assert!(runner.step());
        assert_eq!(runner.region().loaded_chunk_count(), 0);
    }

    #[tokio::test]
    async fn the_runner_stops_when_the_edge_is_gone() {
        let (edge, worker_end) = link::in_process(256);
        let mut runner = runner(worker_end);
        assert!(runner.step());
        drop(edge);
        assert!(!runner.step());
    }

    #[tokio::test]
    async fn an_edge_that_does_not_keep_up_loses_its_link() {
        // Room for a single message, and nobody reads it.
        let (edge, worker_end) = link::in_process(1);
        let mut runner = runner(worker_end);
        edge.try_send(EdgeToWorker::Subscribe { chunks: square(1) })
            .unwrap();

        let mut alive = true;
        for _ in 0..2000 {
            alive = runner.step();
            if !alive {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(!alive, "the runner kept going with a full link");
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
