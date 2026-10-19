//! Tests of a region runner with the items its players throw, written from
//! `docs/adr/0028-entities.md` (sections 1 to 3 and 6) and `docs/groundwork/n2-plan.md`
//! (sections 5, 6 and 11) and the public API alone, by someone who did not write the
//! code: a thrown item is in the tick's commit, and so is there, once, after a crash
//! at any step around that commit and the saves after it, and after the store is
//! started again; a chunk whose only change is an entity is saved without its blocks
//! being written again; and a region's keys are never given twice, across crashes and
//! the store's restarts.
//!
//! A test plays the edge: it attaches a link to a runner, says hello, numbers its
//! messages and reads what comes back, stepping the runner until a message or a state
//! is there. A crash is the region opened at the same store with a higher epoch while
//! the old runner is still there, never stepped again.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use clustine_data::items;
use clustine_format::{ChunkFile, EntitiesFile};
use clustine_region::RegionId;
use clustine_rpc::link::{self, EdgeEnd};
use clustine_rpc::{EdgeMessage, EdgeToWorker, RegionHello, Restored, WorkerToEdge};
use clustine_sim::api::{
    EntityKind, EntityState, HOTBAR_SLOTS, Inventory, ItemStack, PlayerInput, RegionEvent,
};
use clustine_sim::{PlayerEvent, PlayerJoin};
use clustine_worker::{RegionRunner, RegionSettings, Reshape, Reshaped, absorbable};
use clustine_world::{BlockPos, ChunkPos, EdgeId, EntityData, EntityId, PlayerId, Vec3};
use clustine_worldgen::FlatGenerator;
use clustine_worldstore::{Store, StoreHandle};
use uuid::Uuid;

/// The world is one region, pinned to all of it.
const REGION: RegionId = RegionId(0);

/// Players enter in the middle of chunk (0, 0), on the classic flat world's grass.
const SPAWN_BLOCK: BlockPos = BlockPos::new(8, -60, 8);
const HOME: ChunkPos = ChunkPos::new(0, 0);
const FEET: f64 = -60.0;

/// What a player holds when they first enter: 64 stone in slot 0, which is selected.
const HELD: ItemStack = ItemStack {
    item: items::STONE,
    count: 64,
};

/// How often a wait steps the runner before it gives up, a millisecond apart.
const STEPS: usize = 8000;

const CAPACITY: usize = 8192;

/// The game's standing eye height, and how far below the eyes an item leaves.
const EYES: f32 = 1.62;
const BELOW_THE_EYES: f64 = 0.300_000_011_920_928_96;

fn settings() -> RegionSettings {
    let mut hotbar = [None; HOTBAR_SLOTS];
    hotbar[0] = Some(HELD);
    RegionSettings {
        starting_inventory: Inventory::with_hotbar(hotbar),
        return_after: 0,
        lowest_y: -64,
        rings: true,
    }
}

fn player(n: u128) -> PlayerId {
    PlayerId(Uuid::from_u128(n))
}

/// The chunks a viewer at the spawn is shown: the 5×5 around it, so that the spawn's
/// chunk and the ring around it are loaded and it ticks.
fn view() -> Vec<ChunkPos> {
    (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// A world on disk, and the epochs its region has been opened with.
struct World {
    store: Option<Store>,
    epoch: u64,
    directory: tempfile::TempDir,
    /// The interval of checkpoints the runners are given, in ticks, if any.
    checkpoints: Option<u64>,
}

impl World {
    fn new(checkpoints: Option<u64>) -> Self {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let store = Self::start(directory.path());
        Self {
            store: Some(store),
            epoch: 0,
            directory,
            checkpoints,
        }
    }

    fn start(root: &Path) -> Store {
        let generator = Arc::new(FlatGenerator::classic().with_spawn(SPAWN_BLOCK));
        Store::local(root, generator).expect("the world in the directory opens")
    }

    /// Stops the store and starts it again on the same directory. Whoever ran the
    /// region has lost it.
    fn restart_store(&mut self) {
        let store = self.store.take().expect("the store runs");
        store.close().expect("the store closes");
        drop(store);
        self.store = Some(Self::start(self.directory.path()));
    }

    /// Opens the region as its next owner, with what the store answered.
    fn open(&mut self) -> (RegionRunner, Restored) {
        self.open_region(REGION)
    }

    fn next_epoch(&mut self) -> u64 {
        self.epoch += 1;
        self.epoch
    }

    fn open_with(&self, region: RegionId, epoch: u64) -> (StoreHandle, Restored) {
        let store = self.store.as_ref().expect("the store runs");
        store
            .open_region(RegionHello { region, epoch })
            .expect("a higher epoch opens the region")
    }

    /// Opens `region` as its next owner, with what the store answered.
    fn open_region(&mut self, region: RegionId) -> (RegionRunner, Restored) {
        let epoch = self.next_epoch();
        let (handle, restored) = self.open_with(region, epoch);
        let runner = RegionRunner::restore(settings(), handle, restored.clone())
            .expect("what the store has is readable")
            .with_id(region);
        let runner = match self.checkpoints {
            Some(ticks) => runner.with_checkpoint_interval(ticks),
            None => runner,
        };
        (runner, restored)
    }

    fn chunk_file(&self, position: ChunkPos) -> PathBuf {
        self.directory
            .path()
            .join("chunks/overworld")
            .join(format!("{}.{}", position.x >> 5, position.z >> 5))
            .join(format!("{}.{}.chunk", position.x, position.z))
    }

    fn entities_file(&self, position: ChunkPos) -> PathBuf {
        self.chunk_file(position)
            .with_file_name(format!("{}.{}.entities", position.x, position.z))
    }
}

/// The edge's end of a link, with everything read from it so far.
struct Link {
    end: EdgeEnd,
    log: Vec<WorkerToEdge>,
    next: u64,
}

impl Link {
    /// A link of the edge `edge`, which the region has never heard of, that says hello
    /// and asks for the spawn's view.
    fn hello(runner: &RegionRunner, edge: EdgeId) -> Self {
        Self::hello_seeing(runner, edge, view())
    }

    fn hello_seeing(runner: &RegionRunner, edge: EdgeId, chunks: Vec<ChunkPos>) -> Self {
        let (end, worker) = link::in_process::<EdgeMessage, WorkerToEdge>(CAPACITY);
        runner.links().attach(worker);
        let link = Self {
            end,
            log: Vec::new(),
            next: 1,
        };
        link.send(EdgeMessage::unnumbered(EdgeToWorker::Hello {
            edge,
            start: 1,
            since: 0,
            seen: 0,
            players: Vec::new(),
            chunks,
            guests: Vec::new(),
        }));
        link
    }

    fn send(&self, message: EdgeMessage) {
        self.end
            .try_send(message)
            .expect("the link is open and has room");
    }

    fn numbered(&mut self, body: EdgeToWorker) {
        let number = self.next;
        self.next += 1;
        self.send(EdgeMessage {
            number: Some(number),
            body,
        });
    }

    fn drain(&mut self) {
        while let Ok(Some(message)) = self.end.try_recv() {
            self.log.push(message);
        }
    }

    /// The last snapshot of `position` there is, with its tick and its entities.
    fn snapshot(&self, position: ChunkPos) -> Option<(u64, &Vec<EntityState>)> {
        self.log.iter().rev().find_map(|message| match message {
            WorkerToEdge::ChunkSnapshot {
                position: at,
                tick,
                entities,
                ..
            } if *at == position => Some((*tick, entities)),
            _ => None,
        })
    }

    fn spawned(&self, id: PlayerId) -> Option<EntityId> {
        self.log.iter().find_map(|message| match message {
            WorkerToEdge::ToPlayer {
                player,
                event: PlayerEvent::Spawned { entity_id, .. },
            } if *player == id => Some(*entity_id),
            _ => None,
        })
    }

    /// Every entity other than a player the link was told was spawned, with the tick
    /// and the events of that tick before it.
    fn items_spawned(&self) -> Vec<(u64, EntityState, Vec<RegionEvent>)> {
        let mut found = Vec::new();
        for message in &self.log {
            if let WorkerToEdge::TickDelta { tick, events } = message {
                for (index, event) in events.iter().enumerate() {
                    if let RegionEvent::EntitySpawned(state) = event
                        && matches!(state.kind, EntityKind::Other { .. })
                    {
                        found.push((*tick, state.clone(), events[..index].to_vec()));
                    }
                }
            }
        }
        found
    }
}

/// Steps the runner until `done`, reading the link after every step.
fn run_until(
    runner: &mut RegionRunner,
    link: &mut Link,
    what: &str,
    mut done: impl FnMut(&RegionRunner, &Link) -> bool,
) {
    for _ in 0..STEPS {
        runner.step();
        link.drain();
        if done(runner, link) {
            return;
        }
        // The store answers on threads of its own; this gives them the processor.
        thread::sleep(Duration::from_millis(1));
    }
    panic!(
        "never happened: {what}; the region is at tick {}",
        runner.region().tick_number()
    );
}

/// Steps the runner `ticks` ticks on.
fn run_ticks(runner: &mut RegionRunner, link: &mut Link, ticks: u64) {
    let until = runner.region().tick_number() + ticks;
    run_until(runner, link, "ticks passing", |runner, _| {
        runner.region().tick_number() >= until
    });
}

/// Says hello, waits for the spawn's chunk and has player 1 join; returns the link and
/// the player's entity.
fn with_a_player(runner: &mut RegionRunner, edge: EdgeId) -> (Link, EntityId) {
    let mut link = Link::hello(runner, edge);
    run_until(runner, &mut link, "the spawn's chunk", |_, link| {
        link.snapshot(HOME).is_some()
    });
    let joined = player(1);
    link.numbered(EdgeToWorker::PlayerJoin(PlayerJoin {
        player: joined,
        name: "Thrower".to_owned(),
        attempt: 7001,
    }));
    run_until(runner, &mut link, "the player entering", |_, link| {
        link.spawned(joined).is_some()
    });
    let entity = link.spawned(joined).expect("just waited for it");
    (link, entity)
}

/// `input` numbered as the link numbers it, which an input carries twice.
fn send_input(link: &mut Link, entity: EntityId, what: PlayerInput) {
    let number = link.next;
    link.next += 1;
    link.send(EdgeMessage {
        number: Some(number),
        body: EdgeToWorker::Input {
            player: player(1),
            entity,
            number,
            input: what,
        },
    });
}

fn throw(link: &mut Link, entity: EntityId, all: bool) {
    send_input(
        link,
        entity,
        PlayerInput::Drop {
            all,
            crouching: false,
        },
    );
}

/// The items of the chunk at the spawn as the region has it.
fn items_in(runner: &RegionRunner) -> Vec<clustine_world::Entity> {
    runner
        .region()
        .chunk(HOME)
        .map(|chunk| chunk.entities().to_vec())
        .unwrap_or_default()
}

fn stack_of(data: &EntityData) -> ItemStack {
    let EntityData::Item { stack, .. } = data else {
        panic!("{data:?} is not an item");
    };
    *stack
}

/// What player 1 holds in slot 0, as the region has it.
fn held(runner: &RegionRunner) -> Option<ItemStack> {
    let state = runner.region().state();
    state.players.get(&player(1)).and_then(|p| p.inventory.0[0])
}

/// After a restore: the region is shown to a new edge and loads the spawn's chunk.
fn restored_view(runner: &mut RegionRunner, edge: EdgeId) -> Link {
    let mut link = Link::hello(runner, edge);
    run_until(runner, &mut link, "the spawn's chunk", |runner, link| {
        link.snapshot(HOME).is_some() && runner.region().chunk(HOME).is_some()
    });
    link
}

/// The plan, section 11: an item thrown by Q is at the player's x and z and 0.3 below
/// their eyes, with one of the held stack, a pick-up delay of 40 and the player as its
/// thrower; the region's next key is its key; the thrower's arm swings for others in
/// the same tick before it appears. The commit has it: a crash after the edge was shown
/// it leaves it there, once, under its key and UUID, with one more change of holder
/// (the restore's load, ADR-0028 section 6), where its moves left it, its counters as of
/// its record (its moves commit no counters), and the stack it left kept whole between the item
/// and the hand. The restored region's keys are a fresh block (section 1), and an item
/// thrown after the restore is given one of them.
#[test]
fn a_thrown_item_is_committed_and_is_there_once_after_a_crash_with_its_counters_as_of_its_record() {
    let mut world = World::new(None);
    let (mut runner, first_restored) = world.open();
    let (mut link, entity) = with_a_player(&mut runner, EdgeId(1));
    let key_before = runner.region().state().next_entity_id;
    throw(&mut link, entity, false);
    run_until(&mut runner, &mut link, "an item", |_, link| {
        !link.items_spawned().is_empty()
    });
    let [(thrown_at, state, before)] = &link.items_spawned()[..] else {
        panic!("one item: {:?}", link.items_spawned());
    };
    let (thrown_at, state) = (*thrown_at, state.clone());
    assert_eq!(state.entity, key_before, "the region's next key");
    assert!(first_restored.entity_ids.contains(state.entity));
    assert!(
        before.iter().any(|event| matches!(
            event,
            RegionEvent::ArmSwung { entity: swung, .. } if *swung == entity
        )),
        "the thrower's arm swings before the item appears: {before:?}"
    );
    let spawn = Vec3::new(8.5, FEET, 8.5);
    assert_eq!(
        state.pose.position,
        Vec3::new(spawn.x, FEET + f64::from(EYES) - BELOW_THE_EYES, spawn.z)
    );
    let EntityKind::Other {
        kind,
        uuid,
        hops,
        data,
        motion,
        ..
    } = state.kind.clone()
    else {
        panic!("an item");
    };
    assert_eq!(i32::from(kind), clustine_data::entity_types::ITEM);
    assert_eq!(hops, 0);
    assert!(motion != [0.0; 3]);
    assert_eq!(
        data,
        EntityData::Item {
            stack: ItemStack {
                item: items::STONE,
                count: 1
            },
            pick_up_delay: 40,
            thrower: Some(player(1).0),
            health: 5,
            age_offset: 0,
        }
    );
    assert_eq!(
        held(&runner),
        Some(ItemStack {
            item: items::STONE,
            count: 63
        })
    );
    // It flies, lands and comes to rest, committing its moves (step N4b); then the
    // worker is lost.
    run_ticks(&mut runner, &mut link, 30);
    let lost_at = runner.region().tick_number();
    let rested = runner
        .region()
        .entity(state.entity)
        .expect("the item lies in the region")
        .pose
        .position;
    let _lost = runner;

    let (mut runner, restored) = world.open();
    assert!(
        restored.entity_ids.first > first_restored.entity_ids.first,
        "a fresh block at the hello: {:?} after {:?}",
        restored.entity_ids,
        first_restored.entity_ids
    );
    assert!(!restored.entity_ids.contains(state.entity));
    assert_eq!(
        runner.region().state().next_entity_id,
        restored.entity_ids.first
    );
    let resumed_at = runner.region().tick_number();
    let mut link = restored_view(&mut runner, EdgeId(2));
    let (shown_at, shown) = link.snapshot(HOME).expect("waited for it");
    let shown: Vec<&EntityState> = shown
        .iter()
        .filter(|shown| matches!(shown.kind, EntityKind::Other { .. }))
        .collect();
    let [again] = shown[..] else {
        panic!("one item in the snapshot: {shown:?}");
    };
    assert_eq!(again.entity, state.entity);
    assert_eq!(again.pose.position, rested, "where its last move left it");
    let EntityKind::Other {
        uuid: uuid_again,
        hops: hops_again,
        data: data_again,
        ..
    } = &again.kind
    else {
        unreachable!()
    };
    assert_eq!(*uuid_again, uuid);
    assert_eq!(*hops_again, 1, "the restore's load raises its hops");
    assert_eq!(stack_of(data_again), stack_of(&data));
    let EntityData::Item {
        pick_up_delay,
        thrower,
        ..
    } = data_again
    else {
        panic!("not an item");
    };
    assert_eq!(*thrower, Some(player(1).0));
    // Its counters went on from its record, at the throw: not from where the lost
    // worker had them, 30 ticks and more on.
    let since_restore = shown_at - resumed_at;
    assert!(
        u64::from(*pick_up_delay) + since_restore + 1 >= 40,
        "a delay of {pick_up_delay} {since_restore} ticks after the restore, where the \
         lost worker was {} ticks past the throw",
        lost_at - thrown_at
    );
    let [kept] = &items_in(&runner)[..] else {
        panic!("one item in the chunk: {:?}", items_in(&runner));
    };
    let EntityData::Item { pick_up_delay, .. } = &kept.data else {
        panic!("{kept:?} is not an item");
    };
    assert_eq!(u32::from(*pick_up_delay) + kept.lived, 40, "{kept:?}");
    // The hand and the item hold the 64 between them.
    assert_eq!(
        held(&runner),
        Some(ItemStack {
            item: items::STONE,
            count: 63
        })
    );

    // A throw after the restore takes a key of the fresh block.
    let (mut joined, entity) = {
        let joined = player(1);
        link.numbered(EdgeToWorker::PlayerJoin(PlayerJoin {
            player: joined,
            name: "Thrower".to_owned(),
            attempt: 7002,
        }));
        run_until(
            &mut runner,
            &mut link,
            "the player entering again",
            |_, link| link.spawned(joined).is_some(),
        );
        let entity = link.spawned(joined).expect("just waited for it");
        (link, entity)
    };
    assert!(
        restored.entity_ids.contains(entity),
        "a new stay, a new key"
    );
    throw(&mut joined, entity, false);
    run_until(&mut runner, &mut joined, "a second item", |_, link| {
        !link.items_spawned().is_empty()
    });
    let second = joined.items_spawned()[0].1.entity;
    assert!(restored.entity_ids.contains(second));
    assert_ne!(second, state.entity);
    assert_eq!(items_in(&runner).len(), 2);
}

/// A crash at every step from the throw being sent until well after it: the restored
/// region has the item once or not at all, and if the edge was shown it, once; the
/// stack is whole between the hand and the item either way (never two, never none).
/// With checkpoints every few ticks, so that the crashes also fall around saves of the
/// chunk and checkpoints.
#[test]
fn a_crash_at_any_step_around_a_throw_leaves_the_item_once_or_not_at_all_and_the_stack_whole() {
    for checkpoints in [None, Some(3)] {
        // How many crashes came before the edge was shown the item, and how many after.
        let mut outcomes = [0; 2];
        for steps in 0..24 {
            let mut world = World::new(checkpoints);
            let (mut runner, _) = world.open();
            let (mut link, entity) = with_a_player(&mut runner, EdgeId(1));
            throw(&mut link, entity, true);
            for _ in 0..steps {
                runner.step();
                link.drain();
                thread::sleep(Duration::from_millis(1));
            }
            let shown = !link.items_spawned().is_empty();
            let _lost = runner;

            let (mut runner, _) = world.open();
            let in_hand = held(&runner).map_or(0, |stack| stack.count);
            restored_view(&mut runner, EdgeId(2));
            let found = items_in(&runner);
            let case = format!(
                "a crash {steps} steps after the throw, checkpoints {checkpoints:?}: \
                 {in_hand} in hand, {found:?} lying, shown: {shown}"
            );
            assert!(found.len() <= 1, "{case}");
            if shown {
                assert_eq!(found.len(), 1, "{case}");
            }
            let lying: i32 = found.iter().map(|item| stack_of(&item.data).count).sum();
            assert_eq!(in_hand + lying, HELD.count, "{case}");
            outcomes[usize::from(shown)] += 1;
        }
        // The crashes fell both before and after the item was shown, or the test
        // tried less than it says.
        assert!(outcomes[0] > 0 && outcomes[1] > 0, "{outcomes:?}");
    }
}

/// The plan, section 5 and 11: once a chunk's blocks are saved, a chunk whose only
/// change since is an entity is saved without its blocks file being written again;
/// its entities are in a file beside it, as of the save's tick. The store is then
/// stopped and started (a restart, the region's runner lost), and the item is there
/// once.
#[test]
fn a_chunk_whose_only_change_is_an_item_is_saved_without_writing_its_blocks_again() {
    let mut world = World::new(Some(5));
    let (mut runner, _) = world.open();
    let (mut link, entity) = with_a_player(&mut runner, EdgeId(1));
    // A block broken in the chunk, so that it is no longer as generated and its blocks
    // are saved.
    send_input(
        &mut link,
        entity,
        PlayerInput::Dig {
            position: BlockPos::new(8, -61, 10),
            sequence: 1,
        },
    );
    let blocks = world.chunk_file(HOME);
    run_until(
        &mut runner,
        &mut link,
        "the chunk's blocks saved",
        |_, _| {
            std::fs::read(&blocks)
                .ok()
                .and_then(|bytes| ChunkFile::decode(&bytes).ok())
                .is_some_and(|(file, _)| file.chunk.generated() == 0)
        },
    );
    // Let whatever the dig left to save settle into the files.
    run_ticks(&mut runner, &mut link, 30);
    let before = std::fs::read(&blocks).expect("the blocks were saved");

    throw(&mut link, entity, false);
    run_until(&mut runner, &mut link, "an item", |_, link| {
        !link.items_spawned().is_empty()
    });
    let key = link.items_spawned()[0].1.entity;
    let thrown_at = link.items_spawned()[0].0;
    let entities = world.entities_file(HOME);
    run_until(
        &mut runner,
        &mut link,
        "the chunk's entities saved",
        |_, _| entities.exists(),
    );
    run_ticks(&mut runner, &mut link, 30);
    let after = std::fs::read(&blocks).expect("the blocks are there");
    assert!(
        before == after,
        "the blocks file was written again for a change of entities only"
    );
    let file = EntitiesFile::decode(&std::fs::read(&entities).expect("just found it"))
        .expect("an entities file");
    assert_eq!(file.position, HOME);
    let [saved] = &file.entities[..] else {
        panic!("one entity in the file: {:?}", file.entities);
    };
    assert_eq!(saved.key, key);
    // As of the tick it was saved at: it has aged from its throw to then, and its
    // delay with it.
    let EntityData::Item { pick_up_delay, .. } = &saved.data else {
        panic!("{saved:?} is not an item");
    };
    assert_eq!(u32::from(*pick_up_delay) + saved.lived, 40, "{saved:?}");
    assert!(
        u64::from(saved.lived) <= file.tick - thrown_at + 1,
        "aged {} by tick {} of a throw at {thrown_at}",
        saved.lived,
        file.tick
    );

    let _lost = runner;
    world.restart_store();
    let (mut runner, _) = world.open();
    restored_view(&mut runner, EdgeId(2));
    let found = items_in(&runner);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].key, key);
}

/// The plan, section 11: an entity change to a chunk that is still as generated
/// clears that and saves its blocks too, so that the chunk is not made again over its
/// entities. A checkpoint after the throw has the log no longer hold it; the region is
/// lost, the store started again, and the item is there.
#[test]
fn an_item_thrown_into_a_chunk_as_generated_has_its_blocks_saved_too_and_outlasts_a_checkpoint() {
    let mut world = World::new(Some(5));
    let (mut runner, _) = world.open();
    let (mut link, entity) = with_a_player(&mut runner, EdgeId(1));
    throw(&mut link, entity, false);
    run_until(&mut runner, &mut link, "an item", |_, link| {
        !link.items_spawned().is_empty()
    });
    let (thrown_at, key) = {
        let (tick, state, _) = &link.items_spawned()[0];
        (*tick, state.entity)
    };
    let (blocks, entities) = (world.chunk_file(HOME), world.entities_file(HOME));
    run_until(&mut runner, &mut link, "both files saved", |_, _| {
        entities.exists()
            && std::fs::read(&blocks)
                .ok()
                .and_then(|bytes| ChunkFile::decode(&bytes).ok())
                .is_some_and(|(file, _)| file.chunk.generated() == 0)
    });
    // Checkpoints go on; the restore below shows one was made after the throw.
    run_ticks(&mut runner, &mut link, 30);
    let _lost = runner;
    world.restart_store();
    let (mut runner, restored) = world.open();
    let checkpoint = restored.state.as_ref().map_or(0, |state| state.tick);
    assert!(
        checkpoint >= thrown_at,
        "a checkpoint at {checkpoint}, before the throw at {thrown_at}: the log alone \
         would have it"
    );
    restored_view(&mut runner, EdgeId(2));
    let found = items_in(&runner);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].key, key);
}

/// ADR-0028, section 1: every hello is given a fresh block of keys, above every block
/// given before, after crashes and after the store's restarts; a restored region gives
/// out keys from its fresh block, whatever key it had reached.
#[test]
fn keys_are_never_given_twice_across_hellos_and_restarts_of_the_store() {
    let mut world = World::new(None);
    let mut blocks = Vec::new();
    let mut keys = Vec::new();
    for round in 0..4 {
        let (mut runner, restored) = world.open();
        assert_eq!(
            runner.region().state().next_entity_id,
            restored.entity_ids.first,
            "round {round}"
        );
        blocks.push(restored.entity_ids);
        let (mut link, entity) = with_a_player(&mut runner, EdgeId(10 + round));
        keys.push(entity);
        throw(&mut link, entity, false);
        run_until(&mut runner, &mut link, "an item", |_, link| {
            !link.items_spawned().is_empty()
        });
        keys.push(link.items_spawned()[0].1.entity);
        let _lost = runner;
        if round % 2 == 1 {
            world.restart_store();
        }
    }
    for pair in blocks.windows(2) {
        assert!(
            pair[1].first >= pair[0].end,
            "a block given again or overlapping: {blocks:?}"
        );
    }
    let mut sorted = keys.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), keys.len(), "a key given twice: {keys:?}");
    for key in &keys {
        assert!(
            key.0 & 0xFFFF_FFFF != 0 || key.0 >> 32 != 0,
            "key 0: {keys:?}"
        );
    }
}

/// Where player 1 throws in the test of a split: the middle of chunk (-3, 0), far
/// enough from the spawn for a part of its own.
const AWAY: ChunkPos = ChunkPos::new(-3, 0);

/// The 5×5 around [`AWAY`] and the spawn's: what the edge of that test sees.
fn wide_view() -> Vec<ChunkPos> {
    (-5..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// The items `runner` has in [`AWAY`], if it has the chunk.
fn items_away(runner: &RegionRunner) -> Option<Vec<clustine_world::Entity>> {
    runner
        .region()
        .chunk(AWAY)
        .map(|chunk| chunk.entities().to_vec())
}

/// Steps `runner` until `outcome` has an answer.
fn reshaped(
    runner: &mut RegionRunner,
    link: &mut Link,
    outcome: &std::sync::mpsc::Receiver<Reshaped>,
) -> Reshaped {
    for _ in 0..STEPS {
        runner.step();
        link.drain();
        if let Ok(reshaped) = outcome.try_recv() {
            return reshaped;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("no outcome of the reshape");
}

fn ask(runner: &mut RegionRunner, reshape: Reshape) -> std::sync::mpsc::Receiver<Reshaped> {
    let (said, heard) = std::sync::mpsc::channel();
    runner.reshape(
        reshape,
        Box::new(move |reshaped| {
            let _ = said.send(reshaped);
        }),
    );
    heard
}

/// ADR-0028, sections 2 and 6, and the plan, section 11: a split gives the part the
/// item of the chunk it takes, with one more change of holder (a split's warm chunk),
/// and the region split no longer has it; the part, crashed and restored, has it once;
/// a merge of the part back into the region takes it with its chunk; and the survivor,
/// crashed and restored, has it once.
#[test]
fn an_item_goes_with_its_chunk_through_a_split_and_a_merge_and_is_there_once_after_each_crash() {
    let mut world = World::new(Some(5));
    let (mut runner, _) = world.open();
    let mut link = Link::hello_seeing(&runner, EdgeId(1), wide_view());
    run_until(&mut runner, &mut link, "the chunks in view", |runner, _| {
        runner.region().chunk(AWAY).is_some() && runner.region().chunk(HOME).is_some()
    });
    let mut entities = Vec::new();
    for (number, name) in [(1, "Thrower"), (2, "Stayer")] {
        let id = player(number);
        link.numbered(EdgeToWorker::PlayerJoin(PlayerJoin {
            player: id,
            name: name.to_owned(),
            attempt: 7000 + number as u64,
        }));
        run_until(&mut runner, &mut link, "a player entering", |_, link| {
            link.spawned(id).is_some()
        });
        entities.push(link.spawned(id).expect("just waited for it"));
    }
    let away_x = f64::from(AWAY.x) * 16.0 + 8.5;
    send_input(
        &mut link,
        entities[0],
        PlayerInput::Move {
            position: Some(Vec3::new(away_x, FEET, 8.5)),
            rotation: None,
            on_ground: true,
        },
    );
    run_until(&mut runner, &mut link, "the walk", |runner, _| {
        runner
            .region()
            .player(player(1))
            .is_some_and(|(_, pose)| pose.position.x == away_x)
    });
    throw(&mut link, entities[0], false);
    run_until(&mut runner, &mut link, "an item", |_, link| {
        !link.items_spawned().is_empty()
    });
    let key = link.items_spawned()[0].1.entity;
    assert_eq!(items_away(&runner).map(|items| items.len()), Some(1));

    // The split.
    let as_epoch = world.next_epoch();
    let next = world
        .store
        .as_ref()
        .expect("runs")
        .regions()
        .expect("a list")
        .next;
    let outcome = ask(
        &mut runner,
        Reshape::SplitOff {
            chunks: vec![AWAY],
            as_epoch,
            part: next,
        },
    );
    let Reshaped::Split {
        region: part_region,
        as_epoch: part_epoch,
        part,
    } = reshaped(&mut runner, &mut link, &outcome)
    else {
        panic!("no split");
    };
    let (handle, restored) = world.open_with(part_region, part_epoch);
    let mut part_runner = RegionRunner::of_part(part, handle)
        .with_borders(&restored)
        .with_id(part_region);
    let mut part_link = Link::hello_seeing(&part_runner, EdgeId(2), wide_view());
    run_until(
        &mut part_runner,
        &mut part_link,
        "the part's chunk",
        |runner, _| runner.region().chunk(AWAY).is_some(),
    );
    let items = items_away(&part_runner).expect("just waited for it");
    let [item] = &items[..] else {
        panic!("one item in the part: {items:?}");
    };
    assert_eq!(
        (item.key, item.hops),
        (key, 1),
        "a split's warm chunk raises hops"
    );
    assert!(
        items_away(&runner).is_none_or(|items| items.is_empty()),
        "the region split keeps it too: {:?}",
        items_away(&runner)
    );
    run_ticks(&mut part_runner, &mut part_link, 20);

    // The part's worker is lost.
    let _lost = part_runner;
    let (mut part_runner, _) = world.open_region(part_region);
    let mut part_link = Link::hello_seeing(&part_runner, EdgeId(3), wide_view());
    run_until(
        &mut part_runner,
        &mut part_link,
        "the part's chunk again",
        |runner, _| runner.region().chunk(AWAY).is_some(),
    );
    let items = items_away(&part_runner).expect("just waited for it");
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].key, key);

    // The merge of the part back into the region.
    part_runner.begin_release();
    run_until(
        &mut part_runner,
        &mut part_link,
        "the part released",
        |runner, _| runner.ended().is_some(),
    );
    let absorbed_epoch = world.next_epoch();
    let (absorbed_handle, absorbed_restored) = world.open_with(part_region, absorbed_epoch);
    let state = absorbable(&absorbed_handle, absorbed_restored).expect("readable");
    let outcome = ask(
        &mut runner,
        Reshape::Absorb {
            absorbed: part_region,
            absorbed_epoch,
            state,
        },
    );
    let merged = reshaped(&mut runner, &mut link, &outcome);
    assert_eq!(
        merged,
        Reshaped::Absorbed {
            absorbed: part_region
        }
    );
    drop(absorbed_handle);
    let mut survivor_link = Link::hello_seeing(&runner, EdgeId(4), wide_view());
    run_until(
        &mut runner,
        &mut survivor_link,
        "the survivor's chunk",
        |runner, _| runner.region().chunk(AWAY).is_some(),
    );
    let items = items_away(&runner).expect("just waited for it");
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].key, key);
    run_ticks(&mut runner, &mut survivor_link, 20);

    // The survivor's worker is lost.
    let _lost = runner;
    let (mut runner, _) = world.open();
    let mut link = Link::hello_seeing(&runner, EdgeId(5), wide_view());
    run_until(
        &mut runner,
        &mut link,
        "the chunk after the restore",
        |runner, _| runner.region().chunk(AWAY).is_some(),
    );
    let items = items_away(&runner).expect("just waited for it");
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].key, key);
}
