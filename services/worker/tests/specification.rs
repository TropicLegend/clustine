//! Tests of the worker's region runner, written from section 4 of
//! `docs/adr/0008-durable-regions-and-resuming.md` and the public API alone, by someone
//! who has not read how the runner does it.
//!
//! A test plays the edges: it attaches the worker's end of a link to a runner, says
//! hello, numbers its messages itself and reads what comes back. The runner is stepped by
//! the test, and the world store answers on a thread of its own, so every wait is a loop
//! that steps until a message or a state is there. Another owner, or a crash, is the same
//! region opened at the same store with a higher epoch.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::thread;
use std::time::Duration;

use clustine_data::{BlockState, blocks, items};
use clustine_region::{Layout, RegionId};
use clustine_rpc::link::{self, EdgeEnd};
use clustine_rpc::{
    EdgeMessage, EdgeToWorker, Presence, RegionHello, Restored, Welcome, WorkerToEdge,
};
use clustine_sim::api::{
    EntityKind, HOTBAR_SLOTS, ItemStack, PlayerInput, Pose, RegionEvent, RemoteAction, RemoteStep,
};
use clustine_sim::{Durable, PlayerEvent, PlayerJoin, RegionConfig, RegionState};
use clustine_worker::{Ended, RegionRunner, Worker};
use clustine_world::{BlockPos, Chunk, ChunkPos, EdgeId, EntityId, PlayerId, Vec3};
use clustine_worldgen::FlatGenerator;
use clustine_worldstore::{Store, StoreHandle};
use uuid::Uuid;

const E: EdgeId = EdgeId(11);
const F: EdgeId = EdgeId(22);

/// The region under test is the western one of two: every chunk with x below 1.
const REGION: RegionId = RegionId(0);

/// The chunk players enter the world in, the easternmost of the region.
const HOME: ChunkPos = ChunkPos::new(0, 0);

/// The classic flat world has its grass at y = -61, and players stand on it.
const GROUND: i32 = -61;
const FEET: f64 = -60.0;

/// Players enter the world three blocks from the region's eastern end, so that they can
/// reach blocks of the next region and walk into it.
const SPAWN: Vec3 = Vec3::new(13.5, FEET, 8.5);

/// A block of the next region that a player at the spawn can reach.
const BEYOND: BlockPos = BlockPos::new(16, GROUND, 8);

/// How often a wait steps the runner before it gives up. With a millisecond between
/// steps, a wait that cannot succeed fails after a few seconds.
const STEPS: usize = 4000;

/// Room on a link in each direction. A link that does not keep up is dropped, so this is
/// far more than any test leaves unread.
const CAPACITY: usize = 8192;

fn layout() -> Layout {
    Layout::new(vec![1]).expect("one boundary is a layout")
}

fn hotbar() -> [Option<ItemStack>; HOTBAR_SLOTS] {
    let mut hotbar = [None; HOTBAR_SLOTS];
    hotbar[0] = Some(ItemStack {
        item: items::STONE,
        count: 64,
    });
    hotbar
}

fn config() -> RegionConfig {
    RegionConfig {
        spawn: SPAWN,
        area: layout().area(REGION).expect("the layout has region 0"),
        starting_hotbar: hotbar(),
    }
}

fn player(n: u128) -> PlayerId {
    PlayerId(Uuid::from_u128(n))
}

/// A world store and the epochs its region has been opened with.
struct World {
    store: Store,
    epoch: u64,
    /// Kept so that the directory outlives the store.
    _directory: Option<tempfile::TempDir>,
}

impl World {
    fn memory() -> Self {
        Self {
            store: Store::memory(Arc::new(FlatGenerator::classic())),
            epoch: 0,
            _directory: None,
        }
    }

    fn local() -> Self {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let store = Store::local(directory.path(), Arc::new(FlatGenerator::classic()))
            .expect("a new world in an empty directory");
        Self {
            store,
            epoch: 0,
            _directory: Some(directory),
        }
    }

    /// Opens the region as its next owner. Whoever had it before has lost it.
    fn open_raw(&mut self) -> (StoreHandle, Restored) {
        self.epoch += 1;
        self.store
            .open_region(RegionHello {
                region: REGION,
                epoch: self.epoch,
                layout: layout().fingerprint(),
            })
            .expect("a higher epoch opens the region")
    }

    /// A runner for the region as the store has it now.
    fn open(&mut self) -> RegionRunner {
        let (handle, restored) = self.open_raw();
        RegionRunner::restore(config(), handle, restored).expect("what the store has is readable")
    }
}

/// Runs a test on a store in memory, whose commits are confirmed almost at once, and on
/// one on disk, whose commits take longer than a step, so that what a tick produced is
/// still held when the next things happen.
fn in_memory_and_on_disk(test: fn(World)) {
    test(World::memory());
    test(World::local());
}

/// The edge's end of a link, with everything read from it so far.
struct Link {
    end: EdgeEnd,
    log: Vec<WorkerToEdge>,
    closed: bool,
}

impl Link {
    fn attach(runner: &RegionRunner) -> Self {
        let (edge, worker) = link::in_process::<EdgeMessage, WorkerToEdge>(CAPACITY);
        runner.links().attach(worker);
        Self {
            end: edge,
            log: Vec::new(),
            closed: false,
        }
    }

    fn send(&self, message: EdgeMessage) {
        self.end
            .try_send(message)
            .expect("the link is open and has room");
    }

    fn plain(&self, body: EdgeToWorker) {
        self.send(EdgeMessage::unnumbered(body));
    }

    fn numbered(&self, number: u64, body: EdgeToWorker) {
        self.send(EdgeMessage {
            number: Some(number),
            body,
        });
    }

    fn hello(
        &self,
        edge: EdgeId,
        start: u64,
        seen: u64,
        players: Vec<PlayerId>,
        chunks: Vec<ChunkPos>,
    ) {
        self.plain(EdgeToWorker::Hello {
            edge,
            start,
            seen,
            players,
            chunks,
        });
    }

    /// Reads whatever has arrived, and notes whether the worker has closed the link.
    fn drain(&mut self) {
        loop {
            match self.end.try_recv() {
                Ok(Some(message)) => {
                    self.log.push(message);
                    self.check_last();
                }
                Ok(None) => break,
                Err(_) => {
                    self.closed = true;
                    break;
                }
            }
        }
    }
}

impl Link {
    /// Holds the message just read to what section 4 says of every link, whatever the
    /// test is about: the welcome is first and the only one; ticks are published one by
    /// one, so their numbers never go back; an outbox entry has a higher number than
    /// every one before it, in the resume and after; progress never goes back.
    fn check_last(&self) {
        let (last, before) = self.log.split_last().expect("a message was just read");
        let context = || brief(&self.log);
        match last {
            WorkerToEdge::Welcome(_) => {
                assert!(
                    before.is_empty(),
                    "a welcome that is not first: {}",
                    context()
                );
            }
            _ => assert!(
                matches!(self.log[0], WorkerToEdge::Welcome(_)),
                "something before the welcome: {}",
                context()
            ),
        }
        match last {
            WorkerToEdge::TickDelta { tick, .. } | WorkerToEdge::ChunkSnapshot { tick, .. } => {
                let earlier = before.iter().rev().find_map(|message| match message {
                    WorkerToEdge::TickDelta { tick, .. }
                    | WorkerToEdge::ChunkSnapshot { tick, .. } => Some(*tick),
                    _ => None,
                });
                assert!(
                    earlier.is_none_or(|earlier| earlier <= *tick),
                    "tick {tick} after tick {earlier:?}: {}",
                    context()
                );
            }
            WorkerToEdge::Outbox { number, .. } => {
                let earlier = before.iter().rev().find_map(|message| match message {
                    WorkerToEdge::Outbox { number, .. } => Some(*number),
                    _ => None,
                });
                assert!(
                    earlier.is_none_or(|earlier| earlier < *number),
                    "outbox entry {number} after entry {earlier:?}: {}",
                    context()
                );
            }
            WorkerToEdge::Progress { applied, .. } => {
                let earlier = before.iter().rev().find_map(|message| match message {
                    WorkerToEdge::Progress { applied, .. } => Some(*applied),
                    _ => None,
                });
                assert!(
                    earlier.is_none_or(|earlier| earlier <= *applied),
                    "progress {applied} after progress {earlier:?}: {}",
                    context()
                );
            }
            WorkerToEdge::Presence { .. } => {
                // The presence answers belong to the resume, which nothing of a tick
                // comes before or in between.
                assert!(
                    before.iter().all(|message| matches!(
                        message,
                        WorkerToEdge::Welcome(_)
                            | WorkerToEdge::Outbox { .. }
                            | WorkerToEdge::Presence { .. }
                    )),
                    "a presence answer after something that is not of the resume: {}",
                    context()
                );
            }
            _ => {}
        }
    }
}

/// One line per message, as a whole chunk is too much to read in a failure.
fn brief(log: &[WorkerToEdge]) -> String {
    log.iter()
        .enumerate()
        .map(|(index, message)| match message {
            WorkerToEdge::ChunkSnapshot {
                position,
                tick,
                entities,
                ..
            } => format!("{index}: snapshot of {position:?} at {tick} with {entities:?}\n"),
            other => format!("{index}: {other:?}\n"),
        })
        .collect()
}

/// Steps the runner until `done`, reading the links after every step.
fn run_until(
    runner: &mut RegionRunner,
    links: &mut [&mut Link],
    what: &str,
    mut done: impl FnMut(&RegionRunner, &[&mut Link]) -> bool,
) {
    for _ in 0..STEPS {
        runner.step();
        for link in links.iter_mut() {
            link.drain();
        }
        if done(runner, links) {
            return;
        }
        // The store answers on its own thread; this gives it the processor.
        thread::sleep(Duration::from_millis(1));
    }
    let logs: String = links
        .iter()
        .enumerate()
        .map(|(index, link)| {
            format!(
                "link {index} (closed: {}):\n{}",
                link.closed,
                brief(&link.log)
            )
        })
        .collect();
    panic!(
        "never happened: {what}; the region is at tick {}\n{logs}",
        runner.region().tick_number()
    );
}

/// Steps the runner until it has run the tick numbered `tick`.
fn run_to_tick(runner: &mut RegionRunner, links: &mut [&mut Link], tick: u64) {
    run_until(runner, links, "the region reaching a tick", |runner, _| {
        runner.region().tick_number() >= tick
    });
}

/// Waits until everything of the ticks so far has been published: `links[through]`
/// subscribes to a chunk nobody else needs, and its snapshot, which is published with a
/// tick like everything else, shows that every earlier tick is out. Returns that tick.
fn sync(runner: &mut RegionRunner, links: &mut [&mut Link], through: usize) -> u64 {
    static NEXT: AtomicI32 = AtomicI32::new(0);
    let position = ChunkPos::new(-64 - NEXT.fetch_add(1, Ordering::Relaxed), 40);
    links[through].plain(EdgeToWorker::Subscribe {
        chunks: vec![position],
    });
    run_until(
        runner,
        links,
        "the snapshot that marks a tick",
        |_, links| snapshot(&links[through].log, position).is_some(),
    );
    links[through].plain(EdgeToWorker::Unsubscribe {
        chunks: vec![position],
    });
    let (_, tick, _, _) =
        snapshot(&links[through].log, position).expect("the wait ended on this snapshot");
    tick
}

fn position_of(log: &[WorkerToEdge], found: impl Fn(&WorkerToEdge) -> bool) -> Option<usize> {
    log.iter().position(found)
}

/// The first snapshot of `position`: its place in the log, its tick and its content.
fn snapshot(
    log: &[WorkerToEdge],
    position: ChunkPos,
) -> Option<(usize, u64, &Chunk, &Vec<clustine_sim::api::EntityState>)> {
    log.iter()
        .enumerate()
        .find_map(|(index, message)| match message {
            WorkerToEdge::ChunkSnapshot {
                position: at,
                tick,
                chunk,
                entities,
            } if *at == position => Some((index, *tick, chunk, entities)),
            _ => None,
        })
}

/// Every event of every tick delta, with the place of its delta in the log and its tick.
fn events(log: &[WorkerToEdge]) -> Vec<(usize, u64, &RegionEvent)> {
    log.iter()
        .enumerate()
        .flat_map(|(index, message)| match message {
            WorkerToEdge::TickDelta { tick, events } => events
                .iter()
                .map(|event| (index, *tick, event))
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect()
}

/// Where and in which tick the link was told that `entity` is gone.
fn removal(log: &[WorkerToEdge], entity: EntityId) -> Option<(usize, u64)> {
    events(log)
        .into_iter()
        .find_map(|(index, tick, event)| match event {
            RegionEvent::EntityRemoved { entity: gone, .. } if *gone == entity => {
                Some((index, tick))
            }
            _ => None,
        })
}

/// Where the link was told that `block` changed, and to what.
fn block_change(log: &[WorkerToEdge], block: BlockPos) -> Option<(usize, BlockState)> {
    events(log)
        .into_iter()
        .find_map(|(index, _, event)| match event {
            RegionEvent::BlockChanged { position, state } if *position == block => {
                Some((index, *state))
            }
            _ => None,
        })
}

/// Where the link was told that the entity of `id` came into existence.
fn entity_spawn(log: &[WorkerToEdge], id: PlayerId) -> Option<usize> {
    events(log)
        .into_iter()
        .find_map(|(index, _, event)| match event {
            RegionEvent::EntitySpawned(state) => {
                let EntityKind::Player { player, .. } = &state.kind;
                (*player == id).then_some(index)
            }
            _ => None,
        })
}

/// The entity `id` was told they entered the world with, and where in the log.
fn spawned(log: &[WorkerToEdge], id: PlayerId) -> Option<(usize, EntityId)> {
    log.iter()
        .enumerate()
        .find_map(|(index, message)| match message {
            WorkerToEdge::ToPlayer {
                player,
                event: PlayerEvent::Spawned { entity_id, .. },
            } if *player == id => Some((index, *entity_id)),
            _ => None,
        })
}

fn acknowledged(log: &[WorkerToEdge], id: PlayerId, up_to: i32) -> Option<usize> {
    position_of(log, |message| {
        matches!(message, WorkerToEdge::ToPlayer { player, event: PlayerEvent::Acknowledged { sequence } }
            if *player == id && *sequence == up_to)
    })
}

/// The outbox entries on the link, in the order they arrived, with their place.
fn outbox(log: &[WorkerToEdge]) -> Vec<(usize, u64, &Durable)> {
    log.iter()
        .enumerate()
        .filter_map(|(index, message)| match message {
            WorkerToEdge::Outbox { number, entry } => Some((index, *number, entry)),
            _ => None,
        })
        .collect()
}

fn outbox_numbers(log: &[WorkerToEdge]) -> Vec<u64> {
    outbox(log)
        .into_iter()
        .map(|(_, number, _)| number)
        .collect()
}

/// Every `Progress` on the link, in order.
fn progress(log: &[WorkerToEdge]) -> Vec<(u64, &Vec<(PlayerId, u64)>)> {
    log.iter()
        .filter_map(|message| match message {
            WorkerToEdge::Progress { applied, inputs } => Some((*applied, inputs)),
            _ => None,
        })
        .collect()
}

fn applied(log: &[WorkerToEdge]) -> Option<u64> {
    progress(log).last().map(|(applied, _)| *applied)
}

fn presence(log: &[WorkerToEdge], id: PlayerId) -> Option<(usize, &Presence)> {
    log.iter()
        .enumerate()
        .find_map(|(index, message)| match message {
            WorkerToEdge::Presence { player, answer } if *player == id => Some((index, answer)),
            _ => None,
        })
}

fn welcome(log: &[WorkerToEdge]) -> Option<Welcome> {
    match log.first() {
        Some(WorkerToEdge::Welcome(welcome)) => Some(*welcome),
        _ => None,
    }
}

fn block_in(chunk: &Chunk, block: BlockPos) -> BlockState {
    let (x, z) = block.in_chunk();
    chunk.get(x, block.y, z).expect("the block is in the chunk")
}

fn join(id: PlayerId) -> EdgeToWorker {
    EdgeToWorker::PlayerJoin(PlayerJoin {
        player: id,
        name: format!("player-{}", id.0.as_u128()),
    })
}

fn input(id: PlayerId, number: u64, input: PlayerInput) -> EdgeToWorker {
    EdgeToWorker::Input {
        player: id,
        number,
        input,
    }
}

fn move_to(x: f64) -> PlayerInput {
    PlayerInput::Move {
        position: Some(Vec3::new(x, FEET, 8.5)),
        rotation: None,
        on_ground: true,
    }
}

fn dig(position: BlockPos, sequence: i32) -> PlayerInput {
    PlayerInput::Dig { position, sequence }
}

/// What is left of a player of another region breaking `position`, a block of this one.
fn remote_break(id: PlayerId, sequence: i32, position: BlockPos) -> EdgeToWorker {
    EdgeToWorker::Remote(RemoteAction {
        player: id,
        sequence,
        step: RemoteStep::Break { position },
    })
}

/// A new link of `edge` that has said hello, asked for the home chunk and been answered
/// in full: the welcome, the snapshot and the progress of the hello's tick are there.
fn established(runner: &mut RegionRunner, edge: EdgeId, start: u64) -> Link {
    resumed(runner, edge, start, 0, Vec::new())
}

/// As [`established`], for an edge that names what it has seen and whom it believes in
/// the region.
fn resumed(
    runner: &mut RegionRunner,
    edge: EdgeId,
    start: u64,
    seen: u64,
    players: Vec<PlayerId>,
) -> Link {
    let mut link = Link::attach(runner);
    link.hello(edge, start, seen, players, vec![HOME]);
    run_until(
        runner,
        &mut [&mut link],
        "the answer to a hello",
        |_, links| {
            links[0].closed
                || (snapshot(&links[0].log, HOME).is_some() && applied(&links[0].log).is_some())
        },
    );
    link
}

/// Sends a join with `number` and waits until the player is told they entered the world.
fn join_and_wait(
    runner: &mut RegionRunner,
    link: &mut Link,
    number: u64,
    id: PlayerId,
) -> EntityId {
    link.numbered(number, join(id));
    run_until(
        runner,
        &mut [link],
        "a player entering the world",
        |_, links| spawned(&links[0].log, id).is_some(),
    );
    spawned(&link.log, id).expect("just waited for it").1
}

/// Waits until the link has been told that its messages up to `number` are applied.
fn wait_applied(runner: &mut RegionRunner, links: &mut [&mut Link], through: usize, number: u64) {
    run_until(runner, links, "progress up to a number", |_, links| {
        applied(&links[through].log).is_some_and(|applied| applied >= number)
    });
}

// ---------------------------------------------------------------------------------------
// 1. Output commit
// ---------------------------------------------------------------------------------------

/// What an edge has been told of the region, kept the way an edge would keep it.
#[derive(Default)]
struct Witness {
    /// The entities it shows, with their last pose.
    entities: BTreeMap<EntityId, Pose>,
    blocks: BTreeMap<BlockPos, BlockState>,
    /// Its own players who entered the world and have not left it or the region since.
    own: BTreeMap<PlayerId, EntityId>,
    handled: BTreeMap<PlayerId, i32>,
    outbox: BTreeMap<u64, Durable>,
    applied: u64,
    inputs: BTreeMap<PlayerId, u64>,
    /// The last tick in which it was told that something happened.
    tick: u64,
    /// Messages of the kinds that outbox entries replaced.
    replaced_kinds: usize,
}

impl Witness {
    fn absorb(&mut self, message: &WorkerToEdge) {
        match message {
            WorkerToEdge::ChunkSnapshot { entities, .. } => {
                for entity in entities {
                    self.entities.insert(entity.entity, entity.pose);
                }
            }
            WorkerToEdge::TickDelta { tick, events } => {
                if !events.is_empty() {
                    self.tick = self.tick.max(*tick);
                }
                for event in events {
                    match event {
                        RegionEvent::EntitySpawned(state) => {
                            self.entities.insert(state.entity, state.pose);
                        }
                        RegionEvent::EntityRemoved { entity, .. } => {
                            self.entities.remove(entity);
                            self.own.retain(|_, own| own != entity);
                        }
                        RegionEvent::BlockChanged { position, state } => {
                            self.blocks.insert(*position, *state);
                        }
                        RegionEvent::EntityMoved { entity, pose, .. } => {
                            self.entities.insert(*entity, *pose);
                        }
                    }
                }
            }
            WorkerToEdge::ToPlayer { player, event } => match event {
                PlayerEvent::Spawned { entity_id, .. } => {
                    self.own.insert(*player, *entity_id);
                }
                PlayerEvent::Acknowledged { sequence } => {
                    self.handled.insert(*player, *sequence);
                }
                PlayerEvent::Departed(_) | PlayerEvent::Refused => self.replaced_kinds += 1,
            },
            WorkerToEdge::Outbox { number, entry } => {
                // An entry it has seen is ignored, as section 5 has it: it is sent again
                // on every link until it is confirmed, and what it said may be long past.
                if let Some(seen) = self.outbox.get(number) {
                    assert_eq!(seen, entry, "an entry sent again is the same entry");
                    return;
                }
                self.outbox.insert(*number, entry.clone());
                if let Durable::Departed { player, transfer } = entry {
                    // The entity lives on in the next region, which this edge is not
                    // linked to here.
                    self.own.remove(player);
                    self.entities.remove(&transfer.entity_id);
                }
            }
            WorkerToEdge::Progress { applied, inputs } => {
                self.applied = self.applied.max(*applied);
                for (player, number) in inputs {
                    self.inputs.insert(*player, *number);
                }
            }
            WorkerToEdge::Remote(_) | WorkerToEdge::RemoteDone { .. } => self.replaced_kinds += 1,
            WorkerToEdge::Welcome(_) | WorkerToEdge::Presence { .. } => {}
        }
    }

    /// The entities it shows in the region's own chunks.
    fn entities_in_region(&self) -> BTreeMap<EntityId, Pose> {
        self.entities
            .iter()
            .filter(|(_, pose)| {
                config()
                    .area
                    .contains(ChunkPos::containing(pose.position.x, pose.position.z))
            })
            .map(|(entity, pose)| (*entity, *pose))
            .collect()
    }

    /// Holds a restored region's state to what the edge was told. The edge had been told
    /// everything there was when the region was taken away, so the two are equal rather
    /// than the state merely being no older.
    fn check(&self, state: &RegionState, edge: EdgeId) {
        assert_eq!(self.replaced_kinds, 0, "outbox entries replace these");
        assert!(
            state.tick >= self.tick,
            "the edge saw tick {} but the store has the region at {}",
            self.tick,
            state.tick
        );
        let durable: BTreeMap<EntityId, Pose> = state
            .players
            .values()
            .map(|player| (player.entity_id, player.pose))
            .collect();
        assert_eq!(
            self.entities_in_region(),
            durable,
            "the entities {edge:?} shows are those of the restored region"
        );
        for (player, entity) in &self.own {
            let restored = state
                .players
                .get(player)
                .unwrap_or_else(|| panic!("{player:?} entered the world but is not restored"));
            assert_eq!(restored.entity_id, *entity);
            assert_eq!(restored.edge, edge);
            assert_eq!(
                restored.handled,
                self.handled.get(player).copied(),
                "what {player:?} was acknowledged is what was handled"
            );
            assert_eq!(
                restored.last_input,
                self.inputs.get(player).copied().unwrap_or(0),
                "the last input reported for {player:?} is the last applied"
            );
        }
        let known = state
            .edges
            .get(&edge)
            .unwrap_or_else(|| panic!("{edge:?} was welcomed but is not restored"));
        assert_eq!(
            known.applied, self.applied,
            "progress told the edge how far it is"
        );
        assert_eq!(
            known.outbox, self.outbox,
            "every entry the edge got and did not confirm is still in its outbox"
        );
    }
}

/// An edge in the long scenario: its link, its numbering and what it has been told.
struct Session {
    edge: EdgeId,
    start: u64,
    link: Link,
    /// The number of its last message.
    sent: u64,
    /// How much of the link's log the witness has taken in.
    absorbed: usize,
    witness: Witness,
    /// Whom it names in a hello: everyone it ever had in the region.
    players: Vec<PlayerId>,
}

impl Session {
    fn begin(runner: &RegionRunner, edge: EdgeId, start: u64) -> Self {
        let link = Link::attach(runner);
        link.hello(edge, start, 0, Vec::new(), vec![HOME]);
        Self {
            edge,
            start,
            link,
            sent: 0,
            absorbed: 0,
            witness: Witness::default(),
            players: Vec::new(),
        }
    }

    fn send(&mut self, body: EdgeToWorker) {
        self.sent += 1;
        self.link.numbered(self.sent, body);
    }

    fn absorb(&mut self) {
        for message in &self.link.log[self.absorbed..] {
            self.witness.absorb(message);
        }
        self.absorbed = self.link.log.len();
    }
}

/// Waits until every edge has its home chunk and has been told that everything it sent
/// is applied, which is when nothing is left for the region to tell.
fn settle(runner: &mut RegionRunner, sessions: &mut [&mut Session]) {
    let sent: Vec<u64> = sessions.iter().map(|session| session.sent).collect();
    let mut links: Vec<&mut Link> = sessions
        .iter_mut()
        .map(|session| &mut session.link)
        .collect();
    run_until(
        runner,
        &mut links,
        "every edge being told all it sent is applied",
        |_, links| {
            links.iter().zip(&sent).all(|(link, sent)| {
                snapshot(&link.log, HOME).is_some() && applied(&link.log) == Some(*sent)
            })
        },
    );
    for session in sessions.iter_mut() {
        assert!(!session.link.closed);
        session.absorb();
    }
}

/// Takes the region away from `runner` once the edges have been told everything, checks
/// that the next owner has all of it, and resumes every edge with that owner.
fn hand_on(
    world: &mut World,
    mut runner: RegionRunner,
    sessions: &mut [&mut Session],
) -> RegionRunner {
    settle(&mut runner, sessions);

    // The old owner is neither stopped nor told: it is simply no longer the owner.
    let (handle, restored) = world.open_raw();
    let mut next =
        RegionRunner::restore(config(), handle, restored).expect("what the store has is readable");
    drop(runner);
    let state = next.region().state();
    for session in sessions.iter() {
        session.witness.check(&state, session.edge);
    }

    for session in sessions.iter_mut() {
        session.link = Link::attach(&next);
        session.absorbed = 0;
        session.link.hello(
            session.edge,
            session.start,
            0,
            session.players.clone(),
            vec![HOME],
        );
    }
    {
        let mut links: Vec<&mut Link> = sessions
            .iter_mut()
            .map(|session| &mut session.link)
            .collect();
        run_until(&mut next, &mut links, "every edge resuming", |_, links| {
            links
                .iter()
                .all(|link| snapshot(&link.log, HOME).is_some() && applied(&link.log).is_some())
        });
    }

    for session in sessions.iter_mut() {
        let log = &session.link.log;
        let witness = &session.witness;
        assert_eq!(welcome(log), Some(Welcome::Resumed), "{}", brief(log));

        // With nothing seen, the whole outbox is sent again, before the presence answers.
        let resent: BTreeMap<u64, Durable> = outbox(log)
            .into_iter()
            .map(|(_, number, entry)| (number, entry.clone()))
            .collect();
        assert_eq!(
            resent, witness.outbox,
            "what the edge had got and not confirmed is sent again"
        );

        for player in &session.players {
            let (_, answer) = presence(log, *player)
                .unwrap_or_else(|| panic!("no presence answer for {player:?}"));
            match (witness.own.get(player), answer) {
                (
                    Some(entity),
                    Presence::Present {
                        entity: answered,
                        pose,
                        last_input,
                        handled,
                        ..
                    },
                ) => {
                    assert_eq!(answered, entity);
                    assert_eq!(Some(pose), witness.entities.get(entity));
                    assert_eq!(
                        *last_input,
                        witness.inputs.get(player).copied().unwrap_or(0)
                    );
                    assert_eq!(*handled, witness.handled.get(player).copied());
                }
                (None, Presence::Absent) => {}
                (believed, answer) => {
                    panic!("{player:?} was believed to be {believed:?} and is answered {answer:?}")
                }
            }
        }

        let (_, _, chunk, entities) = snapshot(log, HOME).expect("waited for it");
        for (block, state) in &witness.blocks {
            if block.chunk() == HOME {
                assert_eq!(
                    block_in(chunk, *block),
                    *state,
                    "{block:?} is in the chunk the next owner loads as the edge was told"
                );
            }
        }
        let shown: BTreeMap<EntityId, Pose> = entities
            .iter()
            .map(|entity| (entity.entity, entity.pose))
            .collect();
        let believed: BTreeMap<EntityId, Pose> = witness
            .entities_in_region()
            .into_iter()
            .filter(|(_, pose)| ChunkPos::containing(pose.position.x, pose.position.z) == HOME)
            .collect();
        assert_eq!(
            shown, believed,
            "the home chunk has the entities the edge showed"
        );
    }
    for session in sessions.iter_mut() {
        session.absorb();
    }
    next
}

/// Players join, move, dig here and beyond the region, walk out of it and leave, on two
/// edges. After every step the region is taken away and given to a new owner, who must
/// have everything the edges were told.
fn everything_an_edge_was_told_survives_the_owner(mut world: World) {
    let runner = world.open();
    let mut e = Session::begin(&runner, E, 7);
    let mut f = Session::begin(&runner, F, 3);
    let (one, two, three) = (player(1), player(2), player(3));
    let near = BlockPos::new(12, GROUND, 9);

    let mut runner = hand_on(&mut world, runner, &mut [&mut e, &mut f]);

    e.send(join(one));
    e.send(join(two));
    f.send(join(three));
    e.players = vec![one, two];
    f.players = vec![three];
    runner = hand_on(&mut world, runner, &mut [&mut e, &mut f]);
    assert_eq!(e.witness.own.len(), 2);
    assert_eq!(f.witness.own.len(), 1);
    assert_eq!(
        e.witness.entities.len(),
        3,
        "an edge shows everyone at home"
    );

    e.send(input(one, 1, move_to(12.5)));
    f.send(input(three, 1, PlayerInput::SelectSlot { slot: 3 }));
    runner = hand_on(&mut world, runner, &mut [&mut e, &mut f]);
    assert_eq!(
        e.witness.entities[&e.witness.own[&one]].position.x, 12.5,
        "the move was shown"
    );

    e.send(input(one, 2, dig(near, 1)));
    runner = hand_on(&mut world, runner, &mut [&mut e, &mut f]);
    assert_eq!(e.witness.blocks.get(&near), Some(&blocks::AIR));
    assert_eq!(f.witness.blocks.get(&near), Some(&blocks::AIR));
    assert_eq!(e.witness.handled.get(&one), Some(&1));

    // A block of the next region: an outbox entry, and nothing acknowledged.
    e.send(input(one, 3, dig(BEYOND, 2)));
    runner = hand_on(&mut world, runner, &mut [&mut e, &mut f]);
    assert_eq!(e.witness.outbox.len(), 1);
    assert!(matches!(e.witness.outbox.get(&1), Some(Durable::Remote(_))));

    // Out through the eastern end.
    e.send(input(two, 1, move_to(16.5)));
    runner = hand_on(&mut world, runner, &mut [&mut e, &mut f]);
    assert!(matches!(
        e.witness.outbox.get(&2),
        Some(Durable::Departed { player, .. }) if *player == two
    ));
    assert_eq!(e.witness.own.len(), 1);

    f.send(EdgeToWorker::PlayerLeave { player: three });
    f.send(remote_break(player(9), 5, BlockPos::new(11, GROUND, 9)));
    runner = hand_on(&mut world, runner, &mut [&mut e, &mut f]);
    assert!(f.witness.own.is_empty());
    assert_eq!(
        f.witness.outbox.get(&1),
        Some(&Durable::RemoteDone {
            player: player(9),
            sequence: 5
        })
    );

    // Player 2 comes back from the next region, as that region's departure says. The
    // edge knows the entity it passes on, and nobody tells the player they entered the
    // world: they never left it.
    let Some(Durable::Departed { transfer, .. }) = e.witness.outbox.get(&2).cloned() else {
        panic!("entry 2 is the departure");
    };
    let mut coming_back = transfer.clone();
    coming_back.pose.position.x = 15.5;
    coming_back.last_input = 4;
    e.send(EdgeToWorker::PlayerArrive {
        player: two,
        transfer: coming_back,
    });
    e.witness.own.insert(two, transfer.entity_id);
    e.witness.inputs.insert(two, 4);
    runner = hand_on(&mut world, runner, &mut [&mut e, &mut f]);
    assert_eq!(
        runner
            .region()
            .player(two)
            .map(|(entity, pose)| (entity, pose.position.x)),
        Some((transfer.entity_id, 15.5))
    );

    // What the edge sends again of the player's inputs is applied from where the other
    // region got to.
    e.send(input(two, 4, move_to(10.5)));
    e.send(input(two, 5, move_to(14.5)));
    runner = hand_on(&mut world, runner, &mut [&mut e, &mut f]);
    assert_eq!(
        runner.region().player(two).map(|(_, pose)| pose.position.x),
        Some(14.5)
    );

    // And once more with nothing in between, so that a region restored twice in a row
    // is held to the same.
    let runner = hand_on(&mut world, runner, &mut [&mut e, &mut f]);
    assert_eq!(runner.region().player_count(), 2);
}

#[test]
fn everything_an_edge_was_told_is_in_the_store_for_the_next_owner() {
    everything_an_edge_was_told_survives_the_owner(World::memory());
}

#[test]
fn everything_an_edge_was_told_is_on_disk_for_the_next_owner() {
    everything_an_edge_was_told_survives_the_owner(World::local());
}

// ---------------------------------------------------------------------------------------
// 2. A link begins with a hello
// ---------------------------------------------------------------------------------------

#[test]
fn a_numbered_message_before_a_hello_closes_the_link() {
    let mut world = World::memory();
    let mut runner = world.open();
    let mut watcher = established(&mut runner, F, 1);

    let mut hasty = Link::attach(&runner);
    hasty.numbered(1, join(player(1)));
    run_until(
        &mut runner,
        &mut [&mut hasty, &mut watcher],
        "the link being closed",
        |_, links| links[0].closed,
    );
    sync(&mut runner, &mut [&mut watcher], 0);

    assert!(hasty.log.is_empty(), "{}", brief(&hasty.log));
    assert_eq!(runner.region().player_count(), 0);
    assert!(entity_spawn(&watcher.log, player(1)).is_none());
    assert!(!watcher.closed);
}

#[test]
fn a_hello_with_a_lower_start_is_told_it_was_superseded_and_its_link_is_closed() {
    let mut world = World::memory();
    let mut runner = world.open();
    let mut current = established(&mut runner, E, 5);
    assert_eq!(welcome(&current.log), Some(Welcome::Unknown));
    let entity = join_and_wait(&mut runner, &mut current, 1, player(1));

    let mut stale = Link::attach(&runner);
    stale.hello(E, 4, 0, vec![player(1)], vec![HOME]);
    run_until(
        &mut runner,
        &mut [&mut stale, &mut current],
        "the stale link being closed",
        |_, links| links[0].closed,
    );
    assert_eq!(
        stale.log,
        vec![WorkerToEdge::Welcome(Welcome::Superseded)],
        "{}",
        brief(&stale.log)
    );

    // The edge that replaced it is not disturbed by it.
    sync(&mut runner, &mut [&mut current], 0);
    assert!(!current.closed);
    assert_eq!(
        runner.region().player(player(1)).map(|(entity, _)| entity),
        Some(entity)
    );
    assert_eq!(runner.region().edge(E).map(|edge| edge.start), Some(5));
    assert!(removal(&current.log, entity).is_none());
}

#[test]
fn a_hello_with_the_same_start_is_resumed_and_closes_the_edges_other_link() {
    let mut world = World::memory();
    let mut runner = world.open();
    let mut first = established(&mut runner, E, 5);
    let mut other_edge = established(&mut runner, F, 5);
    let entity = join_and_wait(&mut runner, &mut first, 1, player(1));

    let mut second = Link::attach(&runner);
    second.hello(E, 5, 0, vec![player(1)], vec![HOME]);
    run_until(
        &mut runner,
        &mut [&mut second, &mut first, &mut other_edge],
        "the second link resuming and the first being closed",
        |_, links| snapshot(&links[0].log, HOME).is_some() && links[1].closed,
    );
    assert_eq!(welcome(&second.log), Some(Welcome::Resumed));
    assert!(!second.closed);

    // Another edge's link is none of this edge's.
    sync(&mut runner, &mut [&mut other_edge, &mut second], 0);
    assert!(!other_edge.closed);
    assert!(!second.closed);
    assert!(
        removal(&other_edge.log, entity).is_none(),
        "the player stays"
    );
    assert!(matches!(
        presence(&second.log, player(1)),
        Some((_, Presence::Present { entity: present, .. })) if *present == entity
    ));
}

#[test]
fn a_higher_start_removes_the_old_starts_players_and_is_not_known() {
    in_memory_and_on_disk(a_higher_start_removes_the_old_starts_players_and_is_not_known_in);
}

fn a_higher_start_removes_the_old_starts_players_and_is_not_known_in(mut world: World) {
    let mut runner = world.open();
    let mut old = established(&mut runner, E, 5);
    let mut watcher = established(&mut runner, F, 1);
    let entity = join_and_wait(&mut runner, &mut old, 1, player(1));
    let theirs = join_and_wait(&mut runner, &mut watcher, 1, player(2));
    wait_applied(&mut runner, &mut [&mut old, &mut watcher], 0, 1);

    let mut new = Link::attach(&runner);
    new.hello(E, 6, 0, vec![player(1)], vec![HOME]);
    run_until(
        &mut runner,
        &mut [&mut new, &mut old, &mut watcher],
        "the new start being answered and the other edge seeing the player go",
        |_, links| {
            snapshot(&links[0].log, HOME).is_some()
                && applied(&links[0].log).is_some()
                && links[1].closed
                && removal(&links[2].log, entity).is_some()
        },
    );
    assert_eq!(welcome(&new.log), Some(Welcome::Unknown));
    assert!(matches!(
        presence(&new.log, player(1)),
        Some((_, Presence::Absent))
    ));
    assert!(outbox(&new.log).is_empty());
    assert_eq!(runner.region().player(player(1)), None);
    assert_eq!(
        runner.region().player(player(2)).map(|(entity, _)| entity),
        Some(theirs),
        "another edge's player is not touched"
    );
    assert!(removal(&watcher.log, theirs).is_none());
    assert_eq!(
        progress(&new.log).first().map(|(applied, _)| *applied),
        Some(0),
        "nothing of the new start is applied yet"
    );

    // The new start numbers from 1 again.
    let again = join_and_wait(&mut runner, &mut new, 1, player(1));
    assert_ne!(again, entity, "the player enters the world anew");
    assert!(!new.closed);
}

#[test]
fn what_the_old_start_sent_for_the_coming_tick_is_dropped_by_a_higher_start() {
    in_memory_and_on_disk(
        what_the_old_start_sent_for_the_coming_tick_is_dropped_by_a_higher_start_in,
    );
}

fn what_the_old_start_sent_for_the_coming_tick_is_dropped_by_a_higher_start_in(mut world: World) {
    let mut runner = world.open();
    let mut old = established(&mut runner, E, 5);
    let mut watcher = established(&mut runner, F, 1);
    join_and_wait(&mut runner, &mut old, 1, player(1));
    wait_applied(&mut runner, &mut [&mut old, &mut watcher], 0, 1);

    // Both are there when the runner next looks: the old start's join, on the link that
    // was attached first and is served first, and then the new start's hello.
    old.numbered(2, join(player(2)));
    let mut new = Link::attach(&runner);
    new.hello(E, 6, 0, Vec::new(), vec![HOME]);
    run_until(
        &mut runner,
        &mut [&mut new, &mut old, &mut watcher],
        "the new start being answered",
        |_, links| snapshot(&links[0].log, HOME).is_some() && applied(&links[0].log).is_some(),
    );
    sync(&mut runner, &mut [&mut new, &mut watcher, &mut old], 0);

    assert_eq!(welcome(&new.log), Some(Welcome::Unknown));
    assert_eq!(
        runner.region().player_count(),
        0,
        "the join of the old start was applied after the reset"
    );
    assert!(entity_spawn(&watcher.log, player(2)).is_none());
    assert!(spawned(&new.log, player(2)).is_none());
    assert_eq!(
        runner.region().edge(E).map(|edge| edge.applied),
        Some(0),
        "the old start's number became the new start's"
    );
    assert!(
        progress(&new.log).iter().all(|(applied, _)| *applied == 0),
        "{}",
        brief(&new.log)
    );

    // So the new start's first message is its number 1.
    join_and_wait(&mut runner, &mut new, 1, player(3));
    assert!(!new.closed);
}

// ---------------------------------------------------------------------------------------
// 3. The resume, and its order
// ---------------------------------------------------------------------------------------

/// Edge E with player 1, who has selected a slot, moved, dug at home (sequence 1) and
/// beyond the region (sequence 2, outbox entry 1), and player 2, who has walked out
/// (outbox entry 2), after which player 1 has put dirt into a slot; edge F with player 3.
/// E's eight messages are applied.
fn busy_region(runner: &mut RegionRunner) -> (Link, Link, EntityId) {
    let mut e = established(runner, E, 5);
    let mut f = established(runner, F, 5);
    let entity = join_and_wait(runner, &mut e, 1, player(1));
    join_and_wait(runner, &mut e, 2, player(2));
    join_and_wait(runner, &mut f, 1, player(3));
    e.numbered(3, input(player(1), 1, PlayerInput::SelectSlot { slot: 4 }));
    e.numbered(4, input(player(1), 2, move_to(12.5)));
    e.numbered(5, input(player(1), 3, dig(BlockPos::new(12, GROUND, 9), 1)));
    e.numbered(6, input(player(1), 4, dig(BEYOND, 2)));
    e.numbered(7, input(player(2), 1, move_to(16.5)));
    e.numbered(
        8,
        input(
            player(1),
            5,
            PlayerInput::SetHotbarSlot {
                slot: 2,
                stack: Some(ItemStack {
                    item: items::DIRT,
                    count: 5,
                }),
            },
        ),
    );
    wait_applied(runner, &mut [&mut e, &mut f], 0, 8);
    assert_eq!(outbox_numbers(&e.log), vec![1, 2], "{}", brief(&e.log));
    (e, f, entity)
}

#[test]
fn the_resume_comes_first_and_in_order() {
    in_memory_and_on_disk(the_resume_comes_first_and_in_order_in);
}

fn the_resume_comes_first_and_in_order_in(mut world: World) {
    let mut runner = world.open();
    let (mut old, mut f, entity) = busy_region(&mut runner);
    let named = vec![player(1), player(2), player(3), player(4)];

    let mut new = Link::attach(&runner);
    new.hello(E, 5, 1, named.clone(), vec![HOME]);
    run_until(
        &mut runner,
        &mut [&mut new, &mut old, &mut f],
        "the resume",
        |_, links| snapshot(&links[0].log, HOME).is_some() && applied(&links[0].log).is_some(),
    );
    let log = &new.log;

    assert_eq!(
        log[0],
        WorkerToEdge::Welcome(Welcome::Resumed),
        "{}",
        brief(log)
    );
    // Entry 1 has been seen; entry 2 is the departure of player 2.
    assert!(
        matches!(&log[1], WorkerToEdge::Outbox { number: 2, entry: Durable::Departed { player: gone, .. } } if *gone == player(2)),
        "{}",
        brief(log)
    );
    let answers = &log[2..6];
    assert!(
        answers
            .iter()
            .all(|message| matches!(message, WorkerToEdge::Presence { .. })),
        "the presence answers follow the outbox: {}",
        brief(log)
    );
    let answered: BTreeSet<PlayerId> = answers
        .iter()
        .filter_map(|message| match message {
            WorkerToEdge::Presence { player, .. } => Some(*player),
            _ => None,
        })
        .collect();
    assert_eq!(answered, named.iter().copied().collect());
    assert!(
        log[6..].iter().all(|message| !matches!(
            message,
            WorkerToEdge::Welcome(_) | WorkerToEdge::Presence { .. }
        )),
        "{}",
        brief(log)
    );
    assert_eq!(
        outbox_numbers(log),
        vec![2],
        "nothing at or below what was seen, and nothing twice: {}",
        brief(log)
    );

    // Player 1 is there as the edge was told before.
    let (_, answer) = presence(log, player(1)).expect("answered above");
    let mut changed_hotbar = hotbar();
    changed_hotbar[2] = Some(ItemStack {
        item: items::DIRT,
        count: 5,
    });
    assert_eq!(
        *answer,
        Presence::Present {
            entity,
            pose: Pose {
                position: Vec3::new(12.5, FEET, 8.5),
                yaw: 0.0,
                pitch: 0.0,
                on_ground: true,
            },
            hotbar: changed_hotbar,
            selected_slot: 4,
            last_input: 5,
            // The action beyond the region is not handled by this one.
            handled: Some(1),
        }
    );
    // Player 2 has left the region, player 3 is another edge's, player 4 was never here.
    for absent in [player(2), player(3), player(4)] {
        assert_eq!(
            presence(log, absent).map(|(_, answer)| answer),
            Some(&Presence::Absent)
        );
    }
    assert!(
        old.closed || {
            run_until(
                &mut runner,
                &mut [&mut old],
                "the old link closing",
                |_, links| links[0].closed,
            );
            true
        }
    );
}

#[test]
fn a_resume_with_nothing_seen_sends_the_whole_outbox_in_ascending_order() {
    let mut world = World::memory();
    let mut runner = world.open();
    let (_old, _f, _) = busy_region(&mut runner);

    let new = resumed(&mut runner, E, 5, 0, vec![player(2)]);
    let log = &new.log;
    assert_eq!(
        log[0],
        WorkerToEdge::Welcome(Welcome::Resumed),
        "{}",
        brief(log)
    );
    assert!(
        matches!(
            &log[1],
            WorkerToEdge::Outbox {
                number: 1,
                entry: Durable::Remote(_)
            }
        ),
        "{}",
        brief(log)
    );
    assert!(
        matches!(
            &log[2],
            WorkerToEdge::Outbox {
                number: 2,
                entry: Durable::Departed { .. }
            }
        ),
        "{}",
        brief(log)
    );
    assert_eq!(
        log[3],
        WorkerToEdge::Presence {
            player: player(2),
            answer: Presence::Absent
        }
    );
    assert_eq!(outbox_numbers(log), vec![1, 2]);
}

#[test]
fn nothing_of_a_tick_before_the_hellos_arrives_on_the_new_link() {
    in_memory_and_on_disk(nothing_of_a_tick_before_the_hellos_arrives_on_the_new_link_in);
}

fn nothing_of_a_tick_before_the_hellos_arrives_on_the_new_link_in(mut world: World) {
    let mut runner = world.open();
    let mut old = established(&mut runner, E, 5);
    join_and_wait(&mut runner, &mut old, 1, player(1));
    wait_applied(&mut runner, &mut [&mut old], 0, 1);
    let near = BlockPos::new(12, GROUND, 9);

    // One tick takes these in: a change at home with its acknowledgement, and an outbox
    // entry. Whether the store has confirmed that tick when the hello comes is open.
    old.numbered(2, input(player(1), 1, dig(near, 1)));
    old.numbered(3, input(player(1), 2, dig(BEYOND, 2)));
    runner.step();
    let before = runner.region().tick_number();

    let mut new = Link::attach(&runner);
    new.hello(E, 5, 0, vec![player(1)], vec![HOME]);
    run_until(
        &mut runner,
        &mut [&mut new, &mut old],
        "the resume",
        |_, links| snapshot(&links[0].log, HOME).is_some() && applied(&links[0].log).is_some(),
    );
    sync(&mut runner, &mut [&mut new, &mut old], 0);
    let log = &new.log;

    assert_eq!(
        log[0],
        WorkerToEdge::Welcome(Welcome::Resumed),
        "{}",
        brief(log)
    );
    assert_eq!(
        outbox_numbers(log),
        vec![1],
        "the entry is in the resume and only there: {}",
        brief(log)
    );
    assert!(matches!(log[1], WorkerToEdge::Outbox { number: 1, .. }));
    assert!(
        matches!(
            presence(log, player(1)),
            Some((
                2,
                Presence::Present {
                    last_input: 2,
                    handled: Some(1),
                    ..
                }
            ))
        ),
        "{}",
        brief(log)
    );
    assert!(
        acknowledged(log, player(1), 1).is_none(),
        "the acknowledgement was of an earlier tick: {}",
        brief(log)
    );
    assert!(
        block_change(log, near).is_none(),
        "the block changed in an earlier tick: {}",
        brief(log)
    );
    for message in log {
        match message {
            WorkerToEdge::TickDelta { tick, .. } | WorkerToEdge::ChunkSnapshot { tick, .. } => {
                assert!(
                    *tick > before,
                    "tick {tick} is not after {before}: {}",
                    brief(log)
                );
            }
            _ => {}
        }
    }
    let (_, _, chunk, _) = snapshot(log, HOME).expect("waited for it");
    assert_eq!(
        block_in(chunk, near),
        blocks::AIR,
        "the snapshot has the change"
    );
}

// ---------------------------------------------------------------------------------------
// 4. Numbered messages
// ---------------------------------------------------------------------------------------

/// On one link numbers only go up: an edge sends what it kept once per link, in order.
/// A number that goes back on the link it was sent on is the edge's mistake, like a gap,
/// and closes the link. (A number the region has received that comes on a *new* link is
/// what an edge sends again, and is dropped; see the tests below.)
#[test]
fn a_number_that_goes_back_on_its_link_closes_the_link_and_what_came_before_it_counts() {
    for repeated in [1, 2] {
        let mut world = World::memory();
        let mut runner = world.open();
        let mut e = established(&mut runner, E, 5);
        let mut watcher = established(&mut runner, F, 5);

        e.numbered(1, join(player(1)));
        e.numbered(2, join(player(2)));
        // The same number with another content shows that it is the number that decides.
        e.numbered(repeated, join(player(3)));
        run_until(
            &mut runner,
            &mut [&mut e, &mut watcher],
            "the link being closed over the number that went back",
            |_, links| links[0].closed,
        );
        run_until(
            &mut runner,
            &mut [&mut watcher],
            "the joins before it taking effect",
            |_, links| entity_spawn(&links[0].log, player(2)).is_some(),
        );
        sync(&mut runner, &mut [&mut watcher], 0);
        assert!(runner.region().player(player(1)).is_some());
        assert!(runner.region().player(player(2)).is_some());
        assert!(runner.region().player(player(3)).is_none());
    }
}

#[test]
fn a_gap_in_the_numbers_closes_the_link_and_what_came_before_it_counts() {
    let mut world = World::memory();
    let mut runner = world.open();
    let mut e = established(&mut runner, E, 5);
    let mut watcher = established(&mut runner, F, 5);

    e.numbered(1, join(player(1)));
    e.numbered(3, join(player(3)));
    run_until(
        &mut runner,
        &mut [&mut e, &mut watcher],
        "the link being closed over the gap",
        |_, links| links[0].closed,
    );
    run_until(
        &mut runner,
        &mut [&mut watcher],
        "the join before the gap taking effect",
        |_, links| entity_spawn(&links[0].log, player(1)).is_some(),
    );
    sync(&mut runner, &mut [&mut watcher], 0);
    assert!(runner.region().player(player(1)).is_some());
    assert!(runner.region().player(player(3)).is_none());
    assert!(entity_spawn(&watcher.log, player(3)).is_none());

    // The edge carries on from number 2 on its next link.
    let mut again = resumed(&mut runner, E, 5, 0, vec![player(1)]);
    assert_eq!(welcome(&again.log), Some(Welcome::Resumed));
    assert_eq!(applied(&again.log), Some(1));
    again.numbered(2, join(player(2)));
    again.numbered(3, join(player(3)));
    wait_applied(&mut runner, &mut [&mut again, &mut watcher], 0, 3);
    assert_eq!(runner.region().player_count(), 3);
}

#[test]
fn after_a_restore_what_is_sent_again_is_not_applied_twice() {
    let mut world = World::memory();
    let mut runner = world.open();
    let mut e = established(&mut runner, E, 5);
    let mut watcher = established(&mut runner, F, 5);
    let stranger = player(9);
    let near = BlockPos::new(12, GROUND, 9);
    let far = BlockPos::new(11, GROUND, 9);

    // Each of these would show if it were applied a second time: a leave removes the
    // player who came back, a join after it issues a new entity, a remote action makes
    // another outbox entry, and a dig breaks what was placed since.
    let messages = [
        join(player(1)),
        EdgeToWorker::PlayerLeave { player: player(1) },
        join(player(1)),
        remote_break(stranger, 4, far),
        input(player(1), 1, dig(near, 1)),
        input(
            player(1),
            2,
            PlayerInput::UseItemOn {
                position: near.offset(0, -1, 0),
                face: clustine_sim::api::Face::Top,
                sequence: 2,
            },
        ),
        input(player(1), 3, dig(BEYOND, 3)),
    ];
    for (index, message) in messages.iter().enumerate() {
        e.numbered(index as u64 + 1, message.clone());
        wait_applied(
            &mut runner,
            &mut [&mut e, &mut watcher],
            0,
            index as u64 + 1,
        );
    }
    sync(&mut runner, &mut [&mut e, &mut watcher], 0);
    let before = runner.region().state();
    assert_eq!(before.edges[&E].applied, 7);
    assert_eq!(before.edges[&E].sent, 2, "{}", brief(&e.log));
    assert_eq!(
        runner
            .region()
            .chunk(HOME)
            .map(|chunk| block_in(chunk, near)),
        Some(blocks::STONE),
        "the block was dug and then placed"
    );

    let mut next = world.open();
    drop(runner);
    assert_eq!(next.region().state().edges[&E], before.edges[&E]);
    let mut e = resumed(&mut next, E, 5, 0, vec![player(1)]);
    let mut watcher = resumed(&mut next, F, 5, 0, Vec::new());
    assert_eq!(welcome(&e.log), Some(Welcome::Resumed));
    assert_eq!(
        applied(&e.log),
        Some(7),
        "the hello's tick says how far the edge is"
    );
    let resent = outbox_numbers(&e.log);
    assert_eq!(resent, vec![1, 2]);

    // Everything again, as an edge does that has not been told how far the region is,
    // and then one message more, whose effect shows that all before it were looked at.
    for (index, message) in messages.iter().enumerate() {
        e.numbered(index as u64 + 1, message.clone());
    }
    e.numbered(8, join(player(2)));
    wait_applied(&mut next, &mut [&mut e, &mut watcher], 0, 8);
    sync(&mut next, &mut [&mut e, &mut watcher], 0);

    assert!(!e.closed);
    let after = next.region().state();
    assert_eq!(
        after.players[&player(1)],
        before.players[&player(1)],
        "the player is as before"
    );
    assert_eq!(after.edges[&E].sent, 2, "no outbox entry was made again");
    assert_eq!(after.edges[&E].outbox, before.edges[&E].outbox);
    assert_eq!(outbox_numbers(&e.log), vec![1, 2], "{}", brief(&e.log));
    assert_eq!(
        next.region().chunk(HOME).map(|chunk| block_in(chunk, near)),
        Some(blocks::STONE)
    );
    assert!(block_change(&watcher.log, near).is_none());
    assert!(block_change(&watcher.log, far).is_none());
    assert!(
        removal(&watcher.log, before.players[&player(1)].entity_id).is_none(),
        "{}",
        brief(&watcher.log)
    );
    assert!(entity_spawn(&watcher.log, player(1)).is_none());
    assert!(entity_spawn(&watcher.log, player(2)).is_some());
}

#[test]
fn a_number_on_a_message_that_has_none_or_none_on_one_that_has_closes_the_link() {
    let mut world = World::memory();
    let mut runner = world.open();

    let mut numbered = established(&mut runner, E, 5);
    numbered.numbered(
        1,
        EdgeToWorker::Subscribe {
            chunks: vec![ChunkPos::new(-3, 0)],
        },
    );
    run_until(
        &mut runner,
        &mut [&mut numbered],
        "the link being closed over a numbered subscription",
        |_, links| links[0].closed,
    );

    let mut bare = established(&mut runner, F, 5);
    bare.plain(join(player(1)));
    run_until(
        &mut runner,
        &mut [&mut bare],
        "the link being closed over a join without a number",
        |_, links| links[0].closed,
    );
    assert_eq!(runner.region().player_count(), 0);
}

// ---------------------------------------------------------------------------------------
// 5. A resume holds the line
// ---------------------------------------------------------------------------------------

/// A region with player 1 of edge E, given to a new owner, which has no chunk loaded.
fn restored_with_a_player(world: &mut World) -> RegionRunner {
    let mut runner = world.open();
    let mut e = established(&mut runner, E, 5);
    join_and_wait(&mut runner, &mut e, 1, player(1));
    wait_applied(&mut runner, &mut [&mut e], 0, 1);
    let next = world.open();
    assert_eq!(next.region().loaded_chunk_count(), 0);
    assert!(next.region().player(player(1)).is_some());
    next
}

#[test]
fn a_resume_holds_what_follows_until_the_hellos_chunks_are_out() {
    in_memory_and_on_disk(a_resume_holds_what_follows_until_the_hellos_chunks_are_out_in);
}

fn a_resume_holds_what_follows_until_the_hellos_chunks_are_out_in(mut world: World) {
    let mut runner = restored_with_a_player(&mut world);
    let near = BlockPos::new(12, GROUND, 9);
    let chunks = vec![HOME, ChunkPos::new(-1, 0), ChunkPos::new(-2, 0)];

    // The dig is there together with the hello, long before any chunk is. Applied to a
    // chunk that is not loaded it would change nothing.
    let mut e = Link::attach(&runner);
    e.hello(E, 5, 0, vec![player(1)], chunks.clone());
    e.numbered(2, input(player(1), 1, dig(near, 1)));
    wait_applied(&mut runner, &mut [&mut e], 0, 2);
    sync(&mut runner, &mut [&mut e], 0);
    let log = &e.log;

    assert_eq!(welcome(log), Some(Welcome::Resumed), "{}", brief(log));
    let (changed, state) = block_change(log, near)
        .unwrap_or_else(|| panic!("the dig changed nothing: {}", brief(log)));
    assert_eq!(state, blocks::AIR);
    assert_eq!(
        runner
            .region()
            .chunk(HOME)
            .map(|chunk| block_in(chunk, near)),
        Some(blocks::AIR)
    );
    for position in chunks {
        let (index, _, chunk, _) = snapshot(log, position)
            .unwrap_or_else(|| panic!("no snapshot of {position:?}: {}", brief(log)));
        assert!(
            index < changed,
            "the snapshot of {position:?} comes after the change: {}",
            brief(log)
        );
        if position == HOME {
            assert_eq!(
                block_in(chunk, near),
                blocks::GRASS_BLOCK,
                "the snapshot is of before the dig"
            );
        }
    }
    assert!(acknowledged(log, player(1), 1).is_some_and(|index| index > changed));
}

#[test]
fn an_ordinary_subscription_holds_nothing() {
    let mut world = World::memory();
    let mut runner = world.open();
    let mut e = established(&mut runner, E, 5);
    let entity = join_and_wait(&mut runner, &mut e, 1, player(1));
    let elsewhere = ChunkPos::new(-20, 3);

    // The chunk has to come from the store, which cannot have answered within the tick
    // that takes the move in.
    e.plain(EdgeToWorker::Subscribe {
        chunks: vec![elsewhere],
    });
    e.numbered(2, input(player(1), 1, move_to(12.5)));
    run_until(
        &mut runner,
        &mut [&mut e],
        "the snapshot and the move",
        |_, links| {
            snapshot(&links[0].log, elsewhere).is_some() && applied(&links[0].log) == Some(2)
        },
    );
    let moved = events(&e.log)
        .into_iter()
        .find_map(|(index, _, event)| match event {
            RegionEvent::EntityMoved { entity: who, .. } if *who == entity => Some(index),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the move was not shown: {}", brief(&e.log)));
    let (arrived, _, _, _) = snapshot(&e.log, elsewhere).expect("waited for it");
    assert!(
        moved < arrived,
        "the move waited for the snapshot: {}",
        brief(&e.log)
    );
}

#[test]
fn what_was_held_when_a_link_ended_is_applied_once_when_it_is_sent_again() {
    in_memory_and_on_disk(what_was_held_when_a_link_ended_is_applied_once_when_it_is_sent_again_in);
}

fn what_was_held_when_a_link_ended_is_applied_once_when_it_is_sent_again_in(mut world: World) {
    let mut runner = restored_with_a_player(&mut world);
    let stranger = player(9);
    let block = BlockPos::new(11, GROUND, 9);

    let mut first = Link::attach(&runner);
    first.hello(E, 5, 0, vec![player(1)], vec![HOME]);
    first.numbered(2, remote_break(stranger, 4, block));
    runner.step();
    first.drain();
    assert!(
        snapshot(&first.log, HOME).is_none(),
        "the store cannot have delivered the chunk within the step that asked for it"
    );
    drop(first);

    let mut second = resumed(&mut runner, E, 5, 0, vec![player(1)]);
    assert_eq!(welcome(&second.log), Some(Welcome::Resumed));
    sync(&mut runner, &mut [&mut second], 0);
    assert_eq!(
        applied(&second.log),
        Some(1),
        "what was held is not received: {}",
        brief(&second.log)
    );
    assert!(outbox(&second.log).is_empty(), "{}", brief(&second.log));
    assert!(block_change(&second.log, block).is_none());

    second.numbered(2, remote_break(stranger, 4, block));
    wait_applied(&mut runner, &mut [&mut second], 0, 2);
    sync(&mut runner, &mut [&mut second], 0);
    assert_eq!(
        block_change(&second.log, block).map(|(_, state)| state),
        Some(blocks::AIR)
    );
    let entries = outbox(&second.log);
    assert_eq!(entries.len(), 1, "{}", brief(&second.log));
    assert_eq!(
        (entries[0].1, entries[0].2),
        (
            1,
            &Durable::RemoteDone {
                player: stranger,
                sequence: 4
            }
        )
    );
    assert_eq!(runner.region().edge(E).map(|edge| edge.sent), Some(1));
}

// ---------------------------------------------------------------------------------------
// 6. Confirming
// ---------------------------------------------------------------------------------------

/// Edge E with player 1, who has dug three blocks of the next region: outbox entries 1,
/// 2 and 3.
fn three_entries(runner: &mut RegionRunner) -> Link {
    let mut e = established(runner, E, 5);
    join_and_wait(runner, &mut e, 1, player(1));
    for index in 0..3 {
        e.numbered(
            2 + index,
            input(
                player(1),
                1 + index,
                dig(BEYOND.offset(0, 0, index as i32), 1 + index as i32),
            ),
        );
    }
    wait_applied(runner, &mut [&mut e], 0, 4);
    assert_eq!(outbox_numbers(&e.log), vec![1, 2, 3], "{}", brief(&e.log));
    e
}

#[test]
fn confirmed_entries_are_dropped_and_not_sent_again() {
    let mut world = World::memory();
    let mut runner = world.open();
    let mut e = three_entries(&mut runner);

    e.plain(EdgeToWorker::Confirm { number: 2 });
    sync(&mut runner, &mut [&mut e], 0);

    let mut again = resumed(&mut runner, E, 5, 0, vec![player(1)]);
    assert_eq!(welcome(&again.log), Some(Welcome::Resumed));
    assert_eq!(outbox_numbers(&again.log), vec![3], "{}", brief(&again.log));

    // The next owner knows of the confirmation too.
    sync(&mut runner, &mut [&mut again], 0);
    let mut next = world.open();
    drop(runner);
    let outbox_kept: Vec<u64> = next.region().state().edges[&E]
        .outbox
        .keys()
        .copied()
        .collect();
    assert_eq!(outbox_kept, vec![3]);
    let after = resumed(&mut next, E, 5, 0, vec![player(1)]);
    assert_eq!(outbox_numbers(&after.log), vec![3], "{}", brief(&after.log));
}

#[test]
fn what_a_hello_has_seen_is_confirmed_by_it() {
    let mut world = World::memory();
    let mut runner = world.open();
    let _e = three_entries(&mut runner);

    let mut second = resumed(&mut runner, E, 5, 2, vec![player(1)]);
    assert_eq!(
        outbox_numbers(&second.log),
        vec![3],
        "{}",
        brief(&second.log)
    );
    sync(&mut runner, &mut [&mut second], 0);
    let kept: Vec<u64> = runner
        .region()
        .edge(E)
        .expect("the edge is known")
        .outbox
        .keys()
        .copied()
        .collect();
    assert_eq!(kept, vec![3]);

    let third = resumed(&mut runner, E, 5, 0, vec![player(1)]);
    assert_eq!(outbox_numbers(&third.log), vec![3], "{}", brief(&third.log));
}

// ---------------------------------------------------------------------------------------
// 7. Progress
// ---------------------------------------------------------------------------------------

#[test]
fn progress_says_how_far_an_edges_messages_are_and_each_players_last_input() {
    let mut world = World::memory();
    let mut runner = world.open();
    let mut e = established(&mut runner, E, 5);
    let mut f = established(&mut runner, F, 5);

    e.numbered(1, join(player(1)));
    e.numbered(2, join(player(2)));
    e.numbered(3, input(player(1), 1, move_to(12.5)));
    e.numbered(4, input(player(1), 2, PlayerInput::SelectSlot { slot: 2 }));
    e.numbered(5, input(player(2), 1, move_to(11.5)));
    wait_applied(&mut runner, &mut [&mut e, &mut f], 0, 5);
    sync(&mut runner, &mut [&mut e, &mut f], 0);

    let reports = progress(&e.log);
    assert!(
        reports.windows(2).all(|pair| pair[0].0 <= pair[1].0),
        "progress never goes back: {}",
        brief(&e.log)
    );
    assert_eq!(
        reports.last().map(|(applied, _)| *applied),
        Some(5),
        "and not beyond what was sent"
    );
    let mut last_inputs = BTreeMap::new();
    for (_, inputs) in &reports {
        for (player, number) in inputs.iter() {
            last_inputs.insert(*player, *number);
        }
    }
    assert_eq!(
        last_inputs,
        BTreeMap::from([(player(1), 2), (player(2), 1)]),
        "{}",
        brief(&e.log)
    );

    // The other edge sent nothing and has no players.
    for (applied, inputs) in progress(&f.log) {
        assert_eq!(applied, 0);
        assert!(inputs.is_empty(), "{}", brief(&f.log));
    }

    // What progress reports is durable.
    let next = world.open();
    let state = next.region().state();
    assert_eq!(state.edges[&E].applied, 5);
    assert_eq!(state.players[&player(1)].last_input, 2);
    assert_eq!(state.players[&player(2)].last_input, 1);
}

#[test]
fn a_progress_comes_with_the_tick_of_every_hello() {
    let mut world = World::memory();
    let mut runner = world.open();

    // `established` and `resumed` wait for a progress, so each of these also shows that
    // one comes although nothing but the hello happened.
    let mut first = established(&mut runner, E, 5);
    assert_eq!(welcome(&first.log), Some(Welcome::Unknown));
    assert_eq!(
        progress(&first.log)
            .first()
            .map(|(applied, inputs)| (*applied, inputs.is_empty())),
        Some((0, true))
    );

    join_and_wait(&mut runner, &mut first, 1, player(1));
    first.numbered(2, input(player(1), 1, move_to(12.5)));
    wait_applied(&mut runner, &mut [&mut first], 0, 2);

    let second = resumed(&mut runner, E, 5, 0, vec![player(1)]);
    assert_eq!(welcome(&second.log), Some(Welcome::Resumed));
    assert_eq!(
        progress(&second.log).first().map(|(applied, _)| *applied),
        Some(2)
    );

    let mut next = world.open();
    drop(runner);
    let third = resumed(&mut next, E, 5, 0, vec![player(1)]);
    assert_eq!(welcome(&third.log), Some(Welcome::Resumed));
    assert_eq!(
        progress(&third.log).first().map(|(applied, _)| *applied),
        Some(2)
    );
    let first_progress = position_of(&third.log, |message| {
        matches!(message, WorkerToEdge::Progress { .. })
    })
    .expect("there is one");
    let (answer, _) = presence(&third.log, player(1)).expect("the player was named");
    assert!(
        answer < first_progress,
        "the resume comes before anything else of its tick: {}",
        brief(&third.log)
    );
}

// ---------------------------------------------------------------------------------------
// 8. Order within a tick
// ---------------------------------------------------------------------------------------

#[test]
fn what_a_tick_produced_arrives_in_the_agreed_order() {
    in_memory_and_on_disk(what_a_tick_produced_arrives_in_the_agreed_order_in);
}

fn what_a_tick_produced_arrives_in_the_agreed_order_in(mut world: World) {
    let mut runner = world.open();
    let mut e = established(&mut runner, E, 5);
    join_and_wait(&mut runner, &mut e, 1, player(1));
    join_and_wait(&mut runner, &mut e, 2, player(3));
    wait_applied(&mut runner, &mut [&mut e], 0, 2);
    let start = e.log.len();
    let near = BlockPos::new(12, GROUND, 9);

    // All of these are there when the runner next looks, so one tick takes them in.
    e.numbered(3, join(player(2)));
    e.numbered(4, input(player(1), 1, dig(near, 1)));
    e.numbered(5, input(player(1), 2, dig(BEYOND, 2)));
    e.numbered(6, input(player(3), 1, move_to(16.5)));
    wait_applied(&mut runner, &mut [&mut e], 0, 6);
    sync(&mut runner, &mut [&mut e], 0);
    let log = &e.log[start..];

    let changed = block_change(log, near)
        .unwrap_or_else(|| panic!("no block change: {}", brief(log)))
        .0;
    let entity = entity_spawn(log, player(2)).expect("the new entity is an event");
    let entered = spawned(log, player(2)).expect("the player is told").0;
    let remote = position_of(log, |message| {
        matches!(
            message,
            WorkerToEdge::Outbox {
                entry: Durable::Remote(_),
                ..
            }
        )
    })
    .unwrap_or_else(|| panic!("no remote entry: {}", brief(log)));
    let acknowledgement = acknowledged(log, player(1), 1)
        .unwrap_or_else(|| panic!("no acknowledgement: {}", brief(log)));
    let departed = position_of(log, |message| {
        matches!(
            message,
            WorkerToEdge::Outbox {
                entry: Durable::Departed { .. },
                ..
            }
        )
    })
    .unwrap_or_else(|| panic!("no departure: {}", brief(log)));

    // The test is about one tick; say so if the runner spread the batch over several.
    let deltas_between = log[changed.min(entity)..=departed]
        .iter()
        .filter(|message| matches!(message, WorkerToEdge::TickDelta { .. }))
        .count();
    assert_eq!(
        deltas_between,
        1,
        "the batch was not taken in by one tick: {}",
        brief(log)
    );
    assert_eq!(changed, entity, "the events of a tick are one delta");
    assert!(
        changed < entered
            && entered < remote
            && remote < acknowledgement
            && acknowledgement < departed,
        "events {changed}, spawned {entered}, remote {remote}, acknowledged {acknowledgement}, departed {departed}: {}",
        brief(log)
    );
    let numbers = outbox_numbers(&e.log);
    assert_eq!(numbers, vec![1, 2], "outbox numbers ascend on the link");
}

// ---------------------------------------------------------------------------------------
// 9. Edges that stay away
// ---------------------------------------------------------------------------------------

const GONE_AFTER: u64 = 20;

#[test]
fn an_edge_without_a_link_for_too_long_is_forgotten_with_its_players() {
    let mut world = World::memory();
    let mut runner = world.open().with_gone_after(GONE_AFTER);
    let mut e = established(&mut runner, E, 5);
    let mut watcher = established(&mut runner, F, 5);
    let entity = join_and_wait(&mut runner, &mut e, 1, player(1));
    sync(&mut runner, &mut [&mut watcher, &mut e], 0);

    let left = runner.region().tick_number();
    drop(e);
    run_until(
        &mut runner,
        &mut [&mut watcher],
        "the other edge seeing the player removed",
        |_, links| removal(&links[0].log, entity).is_some(),
    );
    let (_, gone) = removal(&watcher.log, entity).expect("waited for it");
    // The runner notices the link's end with its next step, so the first tick without
    // the link is the one after `left`. One tick either way is not what this is about.
    assert!(
        gone + 1 >= left + GONE_AFTER,
        "left after tick {left}, gone in tick {gone}"
    );
    assert!(
        gone <= left + GONE_AFTER + 3,
        "left after tick {left}, gone only in tick {gone}"
    );
    assert_eq!(runner.region().player(player(1)), None);
    assert!(
        runner.region().edge(E).is_none(),
        "the region forgets the edge"
    );
    assert!(!watcher.closed);
    assert!(
        runner.region().edge(F).is_some(),
        "an edge with a link stays"
    );

    // Back with the same start, it is a stranger and numbers from 1.
    let mut back = resumed(&mut runner, E, 5, 0, vec![player(1)]);
    assert_eq!(welcome(&back.log), Some(Welcome::Unknown));
    assert_eq!(
        presence(&back.log, player(1)).map(|(_, answer)| answer),
        Some(&Presence::Absent)
    );
    assert_eq!(applied(&back.log), Some(0));
    let again = join_and_wait(&mut runner, &mut back, 1, player(1));
    assert_ne!(again, entity);
}

#[test]
fn an_edge_that_is_back_in_time_keeps_its_players_however_often_it_is_away() {
    let mut world = World::memory();
    let mut runner = world.open().with_gone_after(GONE_AFTER);
    let mut e = established(&mut runner, E, 5);
    let mut watcher = established(&mut runner, F, 5);
    let entity = join_and_wait(&mut runner, &mut e, 1, player(1));

    // Away for half the time, four times over: more than the limit in all, never at a
    // stretch.
    for _ in 0..4 {
        let left = runner.region().tick_number();
        drop(e);
        run_to_tick(&mut runner, &mut [&mut watcher], left + GONE_AFTER / 2);
        e = resumed(&mut runner, E, 5, 0, vec![player(1)]);
        assert_eq!(welcome(&e.log), Some(Welcome::Resumed), "{}", brief(&e.log));
        assert!(matches!(
            presence(&e.log, player(1)),
            Some((_, Presence::Present { entity: present, .. })) if *present == entity
        ));
    }
    // With a link, it can stay as long as it likes.
    let now = runner.region().tick_number();
    run_to_tick(
        &mut runner,
        &mut [&mut watcher, &mut e],
        now + 2 * GONE_AFTER,
    );
    sync(&mut runner, &mut [&mut watcher, &mut e], 0);
    assert!(removal(&watcher.log, entity).is_none());
    assert_eq!(
        runner.region().player(player(1)).map(|(entity, _)| entity),
        Some(entity)
    );
    assert!(!e.closed);
}

#[test]
fn after_a_restore_edges_count_from_the_restore() {
    let mut world = World::memory();
    let mut runner = world.open().with_gone_after(GONE_AFTER);
    let mut e = established(&mut runner, E, 5);
    let entity = join_and_wait(&mut runner, &mut e, 1, player(1));
    wait_applied(&mut runner, &mut [&mut e], 0, 1);

    // Away for most of the time under the old owner.
    let left = runner.region().tick_number();
    drop(e);
    run_to_tick(&mut runner, &mut [], left + GONE_AFTER - 5);
    assert!(runner.region().player(player(1)).is_some());

    let mut next = world.open().with_gone_after(GONE_AFTER);
    drop(runner);
    let restored = next.region().tick_number();
    let mut watcher = established(&mut next, F, 5);

    // And for most of it again under the new one: more than the limit in all.
    run_to_tick(&mut next, &mut [&mut watcher], restored + GONE_AFTER - 5);
    sync(&mut next, &mut [&mut watcher], 0);
    assert!(
        removal(&watcher.log, entity).is_none(),
        "the time under the old owner was counted: {}",
        brief(&watcher.log)
    );
    assert!(
        next.region().tick_number() < restored + GONE_AFTER - 1,
        "the check above came too late to mean anything"
    );

    // An edge that never comes back to the new owner is forgotten all the same.
    run_until(
        &mut next,
        &mut [&mut watcher],
        "the player of the edge that stayed away being removed",
        |_, links| removal(&links[0].log, entity).is_some(),
    );
    let (_, gone) = removal(&watcher.log, entity).expect("waited for it");
    assert!(
        gone + 1 >= restored + GONE_AFTER && gone <= restored + GONE_AFTER + 3,
        "restored at tick {restored}, gone in tick {gone}"
    );
    assert!(next.region().edge(E).is_none());
}

#[test]
fn after_a_restore_an_edge_that_is_back_in_time_is_resumed() {
    let mut world = World::memory();
    let mut runner = world.open().with_gone_after(GONE_AFTER);
    let mut e = established(&mut runner, E, 5);
    let entity = join_and_wait(&mut runner, &mut e, 1, player(1));
    wait_applied(&mut runner, &mut [&mut e], 0, 1);
    let left = runner.region().tick_number();
    drop(e);
    run_to_tick(&mut runner, &mut [], left + GONE_AFTER - 5);

    let mut next = world.open().with_gone_after(GONE_AFTER);
    drop(runner);
    let restored = next.region().tick_number();
    run_to_tick(&mut next, &mut [], restored + GONE_AFTER - 5);
    let back = resumed(&mut next, E, 5, 0, vec![player(1)]);
    assert_eq!(
        welcome(&back.log),
        Some(Welcome::Resumed),
        "{}",
        brief(&back.log)
    );
    assert!(matches!(
        presence(&back.log, player(1)),
        Some((_, Presence::Present { entity: present, .. })) if *present == entity
    ));
}

// ---------------------------------------------------------------------------------------
// 10. Losing the store
// ---------------------------------------------------------------------------------------

#[test]
fn a_runner_that_lost_its_store_stops_and_shows_nothing_more() {
    let mut world = World::memory();
    let mut runner = world.open();
    let status = runner.status();
    let mut e = established(&mut runner, E, 5);
    let mut watcher = established(&mut runner, F, 5);
    join_and_wait(&mut runner, &mut e, 1, player(1));
    wait_applied(&mut runner, &mut [&mut e, &mut watcher], 0, 1);
    assert!(!runner.store_is_lost());
    assert!(!status.store_lost.load(Ordering::SeqCst));

    // Another owner takes the region. The first is not told; it finds out.
    let other = world.open();
    let e_seen = e.log.len();
    let watcher_seen = watcher.log.len();
    e.numbered(2, join(player(2)));
    e.numbered(3, input(player(1), 1, move_to(12.5)));
    run_until(
        &mut runner,
        &mut [&mut e, &mut watcher],
        "the runner finding its store lost and closing its links",
        |runner, links| runner.store_is_lost() && links[0].closed && links[1].closed,
    );
    assert!(status.store_lost.load(Ordering::SeqCst));

    // Nothing of what it did after it was replaced has been shown to anyone.
    for (link, seen) in [(&e, e_seen), (&watcher, watcher_seen)] {
        let after = &link.log[seen..];
        assert!(spawned(after, player(2)).is_none(), "{}", brief(after));
        assert!(entity_spawn(after, player(2)).is_none(), "{}", brief(after));
        assert!(events(after).is_empty(), "{}", brief(after));
        assert!(
            progress(after).iter().all(|(applied, _)| *applied <= 1),
            "{}",
            brief(after)
        );
    }
    assert!(other.region().player(player(2)).is_none());
    assert_eq!(other.region().state().edges[&E].applied, 1);

    // It has stopped for good: no tick, and a new link is closed.
    let stopped_at = runner.region().tick_number();
    let mut late = Link::attach(&runner);
    late.hello(E, 5, 0, Vec::new(), vec![HOME]);
    run_until(
        &mut runner,
        &mut [&mut late],
        "a link to a stopped runner being closed",
        |_, links| links[0].closed,
    );
    assert!(late.log.is_empty(), "{}", brief(&late.log));
    for _ in 0..20 {
        runner.step();
    }
    assert_eq!(runner.region().tick_number(), stopped_at);
}

#[test]
fn a_runner_does_not_run_far_ahead_of_a_store_that_no_longer_answers() {
    let mut world = World::memory();
    let mut runner = world.open();
    let mut e = established(&mut runner, E, 5);
    join_and_wait(&mut runner, &mut e, 1, player(1));
    wait_applied(&mut runner, &mut [&mut e], 0, 1);
    sync(&mut runner, &mut [&mut e], 0);

    // From here on no commit of this runner is confirmed. Every tick is given something
    // to commit, so none counts as committed by itself.
    let confirmed = runner.region().tick_number();
    let _other = world.open();
    for index in 0..40u64 {
        // The link may be closed by now, which is the runner noticing.
        let _ = e.end.try_send(EdgeMessage {
            number: Some(2 + index),
            body: input(player(1), 1 + index, move_to(12.5 - index as f64 * 0.01)),
        });
        runner.step();
        e.drain();
    }
    assert!(
        runner.region().tick_number() <= confirmed + clustine_worker::MAX_TICKS_AHEAD as u64,
        "confirmed up to tick {confirmed}, and the region is at {}",
        runner.region().tick_number()
    );
    assert!(
        events(&e.log).iter().all(|(_, tick, _)| *tick <= confirmed),
        "{}",
        brief(&e.log)
    );
}

#[test]
fn a_worker_whose_store_is_lost_ends_by_itself() {
    let mut world = World::memory();
    let runner = world.open();
    let links = runner.links();
    let status = runner.status();
    let worker = Worker::spawn(runner);

    // A link attached while the runner runs is served.
    let (edge, end) = link::in_process::<EdgeMessage, WorkerToEdge>(CAPACITY);
    links.attach(end);
    let mut e = Link {
        end: edge,
        log: Vec::new(),
        closed: false,
    };
    e.hello(E, 5, 0, Vec::new(), vec![HOME]);
    wait_for(&mut e, "the welcome of a running worker", |link| {
        snapshot(&link.log, HOME).is_some()
    });
    assert_eq!(welcome(&e.log), Some(Welcome::Unknown));
    assert!(status.tick.load(Ordering::SeqCst) > 0);

    let _other = world.open();
    wait_for(&mut e, "the worker closing its link", |link| link.closed);
    for _ in 0..STEPS {
        if status.store_lost.load(Ordering::SeqCst) {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(status.store_lost.load(Ordering::SeqCst));
    assert_eq!(worker.stop(), Ended::StoreLost);

    // The runner is gone: a link handed over now is closed.
    let (edge, end) = link::in_process::<EdgeMessage, WorkerToEdge>(CAPACITY);
    links.attach(end);
    let mut late = Link {
        end: edge,
        log: Vec::new(),
        closed: false,
    };
    wait_for(
        &mut late,
        "a link to a runner that is gone closing",
        |link| link.closed,
    );
}

/// Reads a link of a region that runs on its own thread until `done`.
fn wait_for(link: &mut Link, what: &str, done: impl Fn(&Link) -> bool) {
    // A worker ticks every 50 ms, so this allows it far longer than a stepped runner.
    for _ in 0..10 * STEPS {
        link.drain();
        if done(link) {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("never happened: {what}\n{}", brief(&link.log));
}

#[test]
fn a_stopped_worker_has_stored_everything_and_closed_its_links() {
    let mut world = World::memory();
    let runner = world.open();
    let links = runner.links();
    let worker = Worker::spawn(runner);

    let (edge, end) = link::in_process::<EdgeMessage, WorkerToEdge>(CAPACITY);
    links.attach(end);
    let mut e = Link {
        end: edge,
        log: Vec::new(),
        closed: false,
    };
    e.hello(E, 5, 0, Vec::new(), vec![HOME]);
    e.numbered(1, join(player(1)));
    let near = BlockPos::new(12, GROUND, 9);
    wait_for(&mut e, "the player entering the world", |link| {
        spawned(&link.log, player(1)).is_some()
    });
    e.numbered(2, input(player(1), 1, dig(near, 1)));
    wait_for(&mut e, "the dig being acknowledged", |link| {
        acknowledged(&link.log, player(1), 1).is_some()
    });
    let entity = spawned(&e.log, player(1)).expect("waited for it").1;

    assert_eq!(worker.stop(), Ended::Stopped);
    e.drain();
    assert!(e.closed, "its links are closed by the time it has stopped");

    let mut next = world.open();
    assert_eq!(
        next.region().player(player(1)).map(|(entity, _)| entity),
        Some(entity)
    );
    let again = resumed(&mut next, E, 5, 0, vec![player(1)]);
    let (_, _, chunk, _) = snapshot(&again.log, HOME).expect("waited for it");
    assert_eq!(block_in(chunk, near), blocks::AIR);
}

// ---------------------------------------------------------------------------------------
// 11. A tick that changes nothing
// ---------------------------------------------------------------------------------------

#[test]
fn a_tick_that_changes_nothing_is_published_without_a_commit() {
    let mut world = World::memory();
    let mut runner = world.open();
    let mut e = established(&mut runner, E, 5);
    join_and_wait(&mut runner, &mut e, 1, player(1));
    wait_applied(&mut runner, &mut [&mut e], 0, 1);
    let busy = sync(&mut runner, &mut [&mut e], 0);

    // Nobody does anything from here on; a subscription changes nothing of the region's
    // state. Its snapshot arrives all the same, and so does the next.
    let quiet = ChunkPos::new(-7, -7);
    e.plain(EdgeToWorker::Subscribe {
        chunks: vec![quiet],
    });
    run_until(
        &mut runner,
        &mut [&mut e],
        "a snapshot while nothing happens",
        |_, links| snapshot(&links[0].log, quiet).is_some(),
    );
    let (_, first, _, _) = snapshot(&e.log, quiet).expect("waited for it");
    let second = sync(&mut runner, &mut [&mut e], 0);
    assert!(busy < first && first < second);

    // None of those ticks was committed: the store has the region as of a tick before
    // them, with everything in it.
    let (handle, restored) = world.open_raw();
    assert!(
        restored.tick() <= busy,
        "the store has tick {}, and nothing changed after {busy}",
        restored.tick()
    );
    let next = RegionRunner::restore(config(), handle, restored).expect("readable");
    assert!(next.region().player(player(1)).is_some());
}

// ---------------------------------------------------------------------------------------
// 12. Players belong to edges, and what else section 4 says
// ---------------------------------------------------------------------------------------

#[test]
fn a_link_that_ends_does_not_make_its_players_leave() {
    let mut world = World::memory();
    let mut runner = world.open();
    let mut e = established(&mut runner, E, 5);
    let mut watcher = established(&mut runner, F, 5);
    let entity = join_and_wait(&mut runner, &mut e, 1, player(1));
    wait_applied(&mut runner, &mut [&mut e, &mut watcher], 0, 1);

    drop(e);
    let now = runner.region().tick_number();
    run_to_tick(&mut runner, &mut [&mut watcher], now + 30);
    sync(&mut runner, &mut [&mut watcher], 0);
    assert!(
        removal(&watcher.log, entity).is_none(),
        "{}",
        brief(&watcher.log)
    );
    assert_eq!(
        runner.region().player(player(1)).map(|(entity, _)| entity),
        Some(entity)
    );

    // On its next link the edge acts for the player as before.
    let mut back = resumed(&mut runner, E, 5, 0, vec![player(1)]);
    assert_eq!(welcome(&back.log), Some(Welcome::Resumed));
    assert!(matches!(
        presence(&back.log, player(1)),
        Some((_, Presence::Present { entity: present, .. })) if *present == entity
    ));
    back.numbered(2, input(player(1), 1, move_to(12.5)));
    run_until(
        &mut runner,
        &mut [&mut watcher, &mut back],
        "the other edge seeing the player move",
        |_, links| {
            events(&links[0].log).iter().any(|(_, _, event)| {
                matches!(event, RegionEvent::EntityMoved { entity: who, pose, .. }
                    if *who == entity && pose.position.x == 12.5)
            })
        },
    );
}

#[test]
fn what_a_link_sent_before_it_ended_still_counts() {
    let mut world = World::memory();
    let mut runner = world.open();
    let e = established(&mut runner, E, 5);
    let mut watcher = established(&mut runner, F, 5);

    // Sent and gone before the runner looks again.
    e.numbered(1, join(player(1)));
    e.numbered(2, join(player(2)));
    drop(e);
    run_until(
        &mut runner,
        &mut [&mut watcher],
        "the joins of a link that has ended taking effect",
        |_, links| {
            entity_spawn(&links[0].log, player(1)).is_some()
                && entity_spawn(&links[0].log, player(2)).is_some()
        },
    );

    // They count as received: the edge is told so, and sending them again changes nothing.
    let mut back = resumed(&mut runner, E, 5, 0, vec![player(1), player(2)]);
    assert_eq!(welcome(&back.log), Some(Welcome::Resumed));
    assert_eq!(applied(&back.log), Some(2), "{}", brief(&back.log));
    for id in [player(1), player(2)] {
        assert!(matches!(
            presence(&back.log, id),
            Some((_, Presence::Present { .. }))
        ));
    }
    back.numbered(1, join(player(1)));
    back.numbered(2, join(player(2)));
    back.numbered(3, join(player(3)));
    wait_applied(&mut runner, &mut [&mut back, &mut watcher], 0, 3);
    assert!(!back.closed);
    assert_eq!(runner.region().player_count(), 3);
}

/// Edge E with players 1 and 2, of whom 2 has walked out without the edge confirming the
/// departure, and edge F watching the home chunk only. Returns both entities.
fn with_an_unconfirmed_departure(runner: &mut RegionRunner) -> (Link, Link, EntityId, EntityId) {
    let mut e = established(runner, E, 5);
    let mut watcher = established(runner, F, 5);
    let stays = join_and_wait(runner, &mut e, 1, player(1));
    let leaves = join_and_wait(runner, &mut e, 2, player(2));
    e.numbered(3, input(player(2), 1, move_to(16.5)));
    wait_applied(runner, &mut [&mut e, &mut watcher], 0, 3);
    assert!(
        matches!(
            outbox(&e.log).as_slice(),
            [(_, 1, Durable::Departed { .. })]
        ),
        "{}",
        brief(&e.log)
    );
    sync(runner, &mut [&mut watcher, &mut e], 0);
    assert!(
        removal(&watcher.log, leaves).is_none(),
        "nothing says that an entity that walks out is gone: {}",
        brief(&watcher.log)
    );
    (e, watcher, stays, leaves)
}

#[test]
fn a_departure_dropped_by_a_reset_is_reported_removed_to_every_link() {
    in_memory_and_on_disk(a_departure_dropped_by_a_reset_is_reported_removed_to_every_link_in);
}

fn a_departure_dropped_by_a_reset_is_reported_removed_to_every_link_in(mut world: World) {
    let mut runner = world.open();
    let (mut old, mut watcher, stays, leaves) = with_an_unconfirmed_departure(&mut runner);

    let mut new = Link::attach(&runner);
    new.hello(E, 6, 0, Vec::new(), vec![HOME]);
    run_until(
        &mut runner,
        &mut [&mut watcher, &mut new, &mut old],
        "the other edge seeing both entities removed",
        |_, links| {
            removal(&links[0].log, stays).is_some() && removal(&links[0].log, leaves).is_some()
        },
    );
    sync(&mut runner, &mut [&mut new, &mut watcher], 0);
    assert_eq!(welcome(&new.log), Some(Welcome::Unknown));
    assert!(
        removal(&new.log, leaves).is_some(),
        "every link is told, the new start's too: {}",
        brief(&new.log)
    );
    let told = events(&watcher.log)
        .into_iter()
        .filter(|(_, _, event)| matches!(event, RegionEvent::EntityRemoved { entity, .. } if *entity == leaves))
        .count();
    assert_eq!(told, 1, "no entity is reported twice");
}

#[test]
fn a_departure_dropped_with_an_edge_that_stayed_away_is_reported_removed() {
    let mut world = World::memory();
    let mut runner = world.open().with_gone_after(GONE_AFTER);
    let (e, mut watcher, stays, leaves) = with_an_unconfirmed_departure(&mut runner);

    drop(e);
    run_until(
        &mut runner,
        &mut [&mut watcher],
        "the other edge seeing both entities removed",
        |_, links| {
            removal(&links[0].log, stays).is_some() && removal(&links[0].log, leaves).is_some()
        },
    );
    assert!(runner.region().edge(E).is_none());

    // That is durable like everything else an edge was told.
    let next = world.open();
    let state = next.region().state();
    assert!(state.players.is_empty());
    assert!(!state.edges.contains_key(&E));
}

#[test]
fn a_confirmed_departure_is_not_reported_removed_when_its_edge_starts_anew() {
    let mut world = World::memory();
    let mut runner = world.open();
    let (mut old, mut watcher, stays, leaves) = with_an_unconfirmed_departure(&mut runner);

    // The edge has passed the departure on: the entity lives in the next region.
    old.plain(EdgeToWorker::Confirm { number: 1 });
    sync(&mut runner, &mut [&mut old, &mut watcher], 0);

    let mut new = Link::attach(&runner);
    new.hello(E, 6, 0, Vec::new(), vec![HOME]);
    run_until(
        &mut runner,
        &mut [&mut watcher, &mut new, &mut old],
        "the other edge seeing the remaining player removed",
        |_, links| removal(&links[0].log, stays).is_some(),
    );
    sync(&mut runner, &mut [&mut watcher, &mut new], 0);
    assert!(
        removal(&watcher.log, leaves).is_none(),
        "{}",
        brief(&watcher.log)
    );
    assert!(removal(&new.log, leaves).is_none());
}

#[test]
fn a_region_restored_for_the_first_time_is_new_and_takes_its_entity_ids_from_the_store() {
    let mut world = World::memory();
    let (handle, restored) = world.open_raw();
    let ids = restored.entity_ids;
    assert_eq!(restored.tick(), 0);
    let mut runner = RegionRunner::restore(config(), handle, restored).expect("readable");
    assert_eq!(runner.region().tick_number(), 0);
    assert_eq!(runner.region().state().entity_ids, ids);

    let mut e = established(&mut runner, E, 5);
    let entity = join_and_wait(&mut runner, &mut e, 1, player(1));
    assert!(ids.contains(entity));
    wait_applied(&mut runner, &mut [&mut e], 0, 1);

    // The next owner carries on with the same block and does not issue the id again.
    let mut next = world.open();
    drop(runner);
    assert_eq!(next.region().state().entity_ids, ids);
    let mut e = resumed(&mut next, E, 5, 0, vec![player(1)]);
    let other = join_and_wait(&mut next, &mut e, 2, player(2));
    assert_ne!(other, entity);
    assert!(ids.contains(other));
}

#[test]
fn what_a_new_link_sends_again_is_dropped_and_the_link_stays() {
    let mut world = World::memory();
    let mut runner = world.open();
    let mut e = established(&mut runner, E, 5);
    let mut watcher = established(&mut runner, F, 5);
    let one = join_and_wait(&mut runner, &mut e, 1, player(1));
    e.numbered(2, EdgeToWorker::PlayerLeave { player: player(1) });
    wait_applied(&mut runner, &mut [&mut e, &mut watcher], 0, 2);
    e.numbered(3, join(player(1)));
    wait_applied(&mut runner, &mut [&mut e, &mut watcher], 0, 3);
    let (again, _) = runner
        .region()
        .player(player(1))
        .expect("the player has joined again");
    assert_ne!(one, again);
    sync(&mut runner, &mut [&mut watcher, &mut e], 0);
    let seen = watcher.log.len();

    // The same runner, a new link: everything from 1 again, then something new.
    let mut back = resumed(&mut runner, E, 5, 0, vec![player(1)]);
    assert_eq!(applied(&back.log), Some(3));
    back.numbered(1, join(player(1)));
    back.numbered(2, EdgeToWorker::PlayerLeave { player: player(1) });
    back.numbered(3, join(player(1)));
    back.numbered(4, join(player(2)));
    wait_applied(&mut runner, &mut [&mut back, &mut watcher], 0, 4);
    sync(&mut runner, &mut [&mut watcher, &mut back], 0);

    assert!(!back.closed);
    assert_eq!(
        runner.region().player(player(1)).map(|(entity, _)| entity),
        Some(again)
    );
    let after = &watcher.log[seen..];
    assert!(removal(after, again).is_none(), "{}", brief(after));
    assert!(entity_spawn(after, player(1)).is_none(), "{}", brief(after));
    assert!(entity_spawn(after, player(2)).is_some(), "{}", brief(after));
}

#[test]
fn what_the_old_start_had_held_behind_its_resume_is_dropped_by_a_higher_start() {
    in_memory_and_on_disk(
        what_the_old_start_had_held_behind_its_resume_is_dropped_by_a_higher_start_in,
    );
}

fn what_the_old_start_had_held_behind_its_resume_is_dropped_by_a_higher_start_in(mut world: World) {
    let mut runner = restored_with_a_player(&mut world);

    // Held: the home chunk cannot be there within the step that asks the store for it.
    let mut old = Link::attach(&runner);
    old.hello(E, 5, 0, vec![player(1)], vec![HOME]);
    old.numbered(2, join(player(2)));
    runner.step();
    old.drain();
    assert!(snapshot(&old.log, HOME).is_none());

    let mut new = Link::attach(&runner);
    new.hello(E, 6, 0, Vec::new(), vec![HOME]);
    run_until(
        &mut runner,
        &mut [&mut new, &mut old],
        "the new start being answered",
        |_, links| snapshot(&links[0].log, HOME).is_some() && applied(&links[0].log).is_some(),
    );
    sync(&mut runner, &mut [&mut new, &mut old], 0);

    assert_eq!(
        welcome(&new.log),
        Some(Welcome::Unknown),
        "{}",
        brief(&new.log)
    );
    assert!(old.closed);
    assert_eq!(
        runner.region().player_count(),
        0,
        "the reset removes the old start's player, and its held join is not applied"
    );
    assert_eq!(
        runner
            .region()
            .edge(E)
            .map(|edge| (edge.start, edge.applied)),
        Some((6, 0))
    );
    assert!(
        spawned(&new.log, player(2)).is_none(),
        "{}",
        brief(&new.log)
    );
    assert!(
        entity_spawn(&new.log, player(2)).is_none(),
        "{}",
        brief(&new.log)
    );
    assert!(progress(&new.log).iter().all(|(applied, _)| *applied == 0));
}

#[test]
fn what_the_old_link_sent_as_the_new_one_said_hello_is_answered_on_the_new_link() {
    in_memory_and_on_disk(
        what_the_old_link_sent_as_the_new_one_said_hello_is_answered_on_the_new_link_in,
    );
}

fn what_the_old_link_sent_as_the_new_one_said_hello_is_answered_on_the_new_link_in(
    mut world: World,
) {
    let mut runner = world.open();
    let mut old = established(&mut runner, E, 5);
    join_and_wait(&mut runner, &mut old, 1, player(1));
    wait_applied(&mut runner, &mut [&mut old], 0, 1);

    // The join is received, as the old link is open when it comes; whatever tick takes
    // it in, its link is gone by the time that tick is published. What the region has
    // to tell the edge goes to the new link.
    old.numbered(2, join(player(2)));
    old.numbered(3, input(player(1), 1, dig(BEYOND, 1)));
    let mut new = Link::attach(&runner);
    new.hello(E, 5, 0, vec![player(1), player(2)], vec![HOME]);
    run_until(
        &mut runner,
        &mut [&mut new, &mut old],
        "the player of the old link's join being told on the new link",
        |_, links| spawned(&links[0].log, player(2)).is_some() && applied(&links[0].log) == Some(3),
    );
    sync(&mut runner, &mut [&mut new, &mut old], 0);
    let log = &new.log;
    // The entry made as the link changed arrives once, and after the resume.
    let entries = outbox(log);
    assert_eq!(entries.len(), 1, "{}", brief(log));
    assert_eq!(entries[0].1, 1);
    assert!(presence(log, player(1)).is_some_and(|(answer, _)| answer < entries[0].0));
    assert!(outbox(&old.log).is_empty(), "{}", brief(&old.log));
    assert_eq!(
        log[0],
        WorkerToEdge::Welcome(Welcome::Resumed),
        "{}",
        brief(log)
    );
    // The resume is of the state before the tick that takes the join in.
    let (answer, absent) = presence(log, player(2)).expect("the player was named");
    assert_eq!(*absent, Presence::Absent, "{}", brief(log));
    let (told, entity) = spawned(log, player(2)).expect("waited for it");
    assert!(answer < told, "{}", brief(log));
    assert_eq!(
        runner.region().player(player(2)).map(|(entity, _)| entity),
        Some(entity)
    );
    assert!(
        spawned(&old.log, player(2)).is_none(),
        "{}",
        brief(&old.log)
    );
}

#[test]
fn a_restored_region_knows_each_edges_start() {
    let mut world = World::memory();
    let mut runner = world.open();
    let mut e = established(&mut runner, E, 5);
    let entity = join_and_wait(&mut runner, &mut e, 1, player(1));
    wait_applied(&mut runner, &mut [&mut e], 0, 1);

    let mut next = world.open();
    drop(runner);
    let mut watcher = established(&mut next, F, 1);

    let mut stale = Link::attach(&next);
    stale.hello(E, 4, 0, vec![player(1)], vec![HOME]);
    run_until(
        &mut next,
        &mut [&mut stale, &mut watcher],
        "the stale start being refused",
        |_, links| links[0].closed,
    );
    assert_eq!(
        stale.log,
        vec![WorkerToEdge::Welcome(Welcome::Superseded)],
        "{}",
        brief(&stale.log)
    );
    sync(&mut next, &mut [&mut watcher], 0);
    assert!(next.region().player(player(1)).is_some());

    let newer = resumed(&mut next, E, 6, 0, vec![player(1)]);
    assert_eq!(welcome(&newer.log), Some(Welcome::Unknown));
    run_until(
        &mut next,
        &mut [&mut watcher],
        "the old start's player being removed",
        |_, links| removal(&links[0].log, entity).is_some(),
    );
    assert_eq!(next.region().player(player(1)), None);
}

#[test]
fn a_snapshot_is_of_its_tick_whatever_happened_before_it_could_be_published() {
    // On disk, so that a commit takes longer than the two steps below are apart.
    let mut world = World::local();
    let mut runner = world.open();
    let mut e = established(&mut runner, E, 5);
    let entity = join_and_wait(&mut runner, &mut e, 1, player(1));
    wait_applied(&mut runner, &mut [&mut e], 0, 1);
    sync(&mut runner, &mut [&mut e], 0);
    let near = BlockPos::new(12, GROUND, 9);

    // The tick of the subscription has something to commit, so its snapshot waits; the
    // tick after it changes the chunk and moves the player.
    let mut f = Link::attach(&runner);
    f.hello(F, 1, 0, Vec::new(), vec![HOME]);
    e.numbered(2, input(player(1), 1, PlayerInput::SelectSlot { slot: 1 }));
    runner.step();
    e.numbered(3, input(player(1), 2, dig(near, 1)));
    e.numbered(4, input(player(1), 3, move_to(12.5)));
    runner.step();
    wait_applied(&mut runner, &mut [&mut e, &mut f], 0, 4);
    sync(&mut runner, &mut [&mut f, &mut e], 0);

    let log = &f.log;
    let (index, taken, chunk, entities) = snapshot(log, HOME).expect("subscribed by the hello");
    let (changed, _) = block_change(log, near)
        .unwrap_or_else(|| panic!("the change after the snapshot is missing: {}", brief(log)));
    let (_, changed_in, _) = events(log)
        .into_iter()
        .find(|(at, _, _)| *at == changed)
        .expect("just found");
    assert!(index < changed && taken < changed_in, "{}", brief(log));
    assert_eq!(
        block_in(chunk, near),
        blocks::GRASS_BLOCK,
        "a snapshot of tick {taken} shows a change of tick {changed_in}"
    );
    let shown = entities
        .iter()
        .find(|state| state.entity == entity)
        .expect("the player is in the snapshot");
    assert_eq!(
        shown.pose.position.x, 13.5,
        "the move came after the snapshot's tick"
    );
}

#[test]
fn a_higher_start_is_told_nothing_of_what_the_old_start_was_owed() {
    in_memory_and_on_disk(a_higher_start_is_told_nothing_of_what_the_old_start_was_owed_in);
}

fn a_higher_start_is_told_nothing_of_what_the_old_start_was_owed_in(mut world: World) {
    let mut runner = world.open();
    let mut old = established(&mut runner, E, 5);
    let entity = join_and_wait(&mut runner, &mut old, 1, player(1));
    wait_applied(&mut runner, &mut [&mut old], 0, 1);

    // A tick of the old start with an outbox entry, an acknowledgement and progress,
    // which may still be held when the new start says hello.
    old.numbered(2, input(player(1), 1, dig(BlockPos::new(12, GROUND, 9), 1)));
    old.numbered(3, input(player(1), 2, dig(BEYOND, 2)));
    runner.step();

    let mut new = Link::attach(&runner);
    new.hello(E, 6, 0, vec![player(1)], vec![HOME]);
    run_until(
        &mut runner,
        &mut [&mut new, &mut old],
        "the new start being answered",
        |_, links| snapshot(&links[0].log, HOME).is_some() && applied(&links[0].log).is_some(),
    );
    sync(&mut runner, &mut [&mut new, &mut old], 0);
    let log = &new.log;

    assert_eq!(welcome(log), Some(Welcome::Unknown), "{}", brief(log));
    assert!(
        outbox(log).is_empty(),
        "the old start's outbox is dropped: {}",
        brief(log)
    );
    assert!(
        log.iter()
            .all(|message| !matches!(message, WorkerToEdge::ToPlayer { .. })),
        "{}",
        brief(log)
    );
    assert!(
        progress(log)
            .iter()
            .all(|(applied, inputs)| *applied == 0 && inputs.is_empty()),
        "{}",
        brief(log)
    );
    let (_, _, _, entities) = snapshot(log, HOME).expect("waited for it");
    assert!(
        entities.iter().all(|state| state.entity != entity),
        "the old start's player is in the new start's snapshot"
    );
    assert_eq!(runner.region().player_count(), 0);
}

#[test]
fn a_tick_with_nothing_to_commit_does_not_overtake_the_tick_before_it() {
    // On disk, so that the commit of the first tick is not confirmed before the second
    // tick has run.
    let mut world = World::local();
    let mut runner = world.open();
    let mut e = established(&mut runner, E, 5);
    let mut f = established(&mut runner, F, 5);
    join_and_wait(&mut runner, &mut e, 1, player(1));
    wait_applied(&mut runner, &mut [&mut e, &mut f], 0, 1);
    // Another edge keeps a second chunk loaded, so that a snapshot of it needs no store.
    let other = ChunkPos::new(-1, 0);
    f.plain(EdgeToWorker::Subscribe {
        chunks: vec![other],
    });
    run_until(
        &mut runner,
        &mut [&mut f, &mut e],
        "the second chunk being loaded",
        |_, links| snapshot(&links[0].log, other).is_some(),
    );
    sync(&mut runner, &mut [&mut e, &mut f], 0);
    let near = BlockPos::new(12, GROUND, 9);

    // A tick with a block change, and then one in which nothing changes but which has a
    // snapshot to publish.
    e.numbered(2, input(player(1), 1, dig(near, 1)));
    runner.step();
    e.plain(EdgeToWorker::Subscribe {
        chunks: vec![other],
    });
    runner.step();
    run_until(
        &mut runner,
        &mut [&mut e, &mut f],
        "the change and the snapshot",
        |_, links| {
            snapshot(&links[0].log, other).is_some() && block_change(&links[0].log, near).is_some()
        },
    );
    let (changed, _) = block_change(&e.log, near).expect("waited for it");
    let (arrived, _, _, _) = snapshot(&e.log, other).expect("waited for it");
    assert!(
        changed < arrived,
        "the later tick was published first: {}",
        brief(&e.log)
    );
}

#[test]
fn after_a_restore_a_departure_dropped_with_its_edge_is_still_reported_removed() {
    let mut world = World::memory();
    let mut runner = world.open();
    let (_e, _watcher, stays, leaves) = with_an_unconfirmed_departure(&mut runner);

    // The next owner has the outbox, and with it the departure nobody passed on.
    let mut next = world.open().with_gone_after(GONE_AFTER);
    drop(runner);
    let mut watcher = resumed(&mut next, F, 5, 0, Vec::new());
    assert_eq!(welcome(&watcher.log), Some(Welcome::Resumed));
    run_until(
        &mut next,
        &mut [&mut watcher],
        "the other edge seeing both entities removed",
        |_, links| {
            removal(&links[0].log, stays).is_some() && removal(&links[0].log, leaves).is_some()
        },
    );
    assert!(next.region().edge(E).is_none());
    assert!(next.region().edge(F).is_some());
}

#[test]
fn of_two_links_of_an_edge_that_say_hello_at_once_the_later_one_stays() {
    let mut world = World::memory();
    let mut runner = world.open();

    // What a connection that is half open causes: two links of one edge, both there
    // when the runner next looks.
    let mut first = Link::attach(&runner);
    first.hello(E, 5, 0, Vec::new(), vec![HOME]);
    let mut second = Link::attach(&runner);
    second.hello(E, 5, 0, Vec::new(), vec![HOME]);
    run_until(
        &mut runner,
        &mut [&mut second, &mut first],
        "the first link being closed and the second answered",
        |_, links| {
            links[1].closed
                && snapshot(&links[0].log, HOME).is_some()
                && applied(&links[0].log).is_some()
        },
    );
    assert!(!second.closed);
    assert!(welcome(&second.log).is_some(), "{}", brief(&second.log));
    assert!(
        snapshot(&first.log, HOME).is_none(),
        "what the region has to tell goes to the later link: {}",
        brief(&first.log)
    );

    // The edge is one edge: its numbers go on over the link that stayed.
    let entity = join_and_wait(&mut runner, &mut second, 1, player(1));
    wait_applied(&mut runner, &mut [&mut second], 0, 1);
    assert_eq!(runner.region().player_count(), 1);
    assert!(entity_spawn(&second.log, player(1)).is_some());
    assert_eq!(
        runner.region().player(player(1)).map(|(entity, _)| entity),
        Some(entity)
    );
}
