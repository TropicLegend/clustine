//! Tests of a region runner with items and falling blocks that move, written from
//! `docs/groundwork/n4-plan.md` (sections 2 and 7: D1, what commits) and
//! `docs/adr/0028-entities.md` (section 3) and the public API alone, by someone who did
//! not write the code: an item at rest commits nothing, but its age, its delay and its
//! motion reach the store at the checkpoints; its age survives the region's stop and
//! start, and it despawns when that age reaches the game's 6000; and sand that falls is
//! in the commits as it falls, so that a crash at any step of its fall leaves exactly
//! one sand, as a block or falling, which then lands.
//!
//! A test plays the edge: it attaches a link to a runner, says hello, numbers its
//! messages and reads what comes back, stepping the runner until a message or a state
//! is there. A crash is the region opened at the same store with a higher epoch while
//! the old runner is still there, never stepped again; a stop is the runner's release.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use clustine_data::{blocks, entity_types, items};
use clustine_format::EntitiesFile;
use clustine_region::RegionId;
use clustine_rpc::link::{self, EdgeEnd};
use clustine_rpc::{EdgeMessage, EdgeToWorker, RegionHello, Restored, WorkerToEdge};
use clustine_sim::api::{
    EntityKind, EntityState, Face, Inventory, ItemStack, PlayerInput, RegionEvent,
};
use clustine_sim::{PlayerEvent, PlayerJoin};
use clustine_worker::{Ended, RegionRunner, RegionSettings};
use clustine_world::{BlockPos, ChunkPos, EdgeId, Entity, EntityData, EntityId, PlayerId};
use clustine_worldgen::FlatGenerator;
use clustine_worldstore::{Store, StoreHandle};
use uuid::Uuid;

const REGION: RegionId = RegionId(0);
const SPAWN_BLOCK: BlockPos = BlockPos::new(8, -60, 8);
const HOME: ChunkPos = ChunkPos::new(0, 0);

const STEPS: usize = 20_000;
const CAPACITY: usize = 8192;

/// The game's `LIFETIME` of an item.
const LIFETIME: u32 = 6000;

fn settings() -> RegionSettings {
    let mut hotbar = std::array::from_fn(|_| None);
    hotbar[0] = Some(ItemStack::new(items::STONE, 64));
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

fn view() -> Vec<ChunkPos> {
    (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

struct World {
    store: Option<Store>,
    epoch: u64,
    directory: tempfile::TempDir,
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

    fn open(&mut self) -> (RegionRunner, Restored) {
        self.epoch += 1;
        let store = self.store.as_ref().expect("the store runs");
        let (handle, restored): (StoreHandle, Restored) = store
            .open_region(RegionHello {
                region: REGION,
                epoch: self.epoch,
            })
            .expect("a higher epoch opens the region");
        let runner = RegionRunner::restore(settings(), handle, restored.clone())
            .expect("what the store has is readable")
            .with_id(REGION);
        let runner = match self.checkpoints {
            Some(ticks) => runner.with_checkpoint_interval(ticks),
            None => runner,
        };
        (runner, restored)
    }

    fn entities_file(&self, position: ChunkPos) -> PathBuf {
        self.directory
            .path()
            .join("chunks/overworld")
            .join(format!("{}.{}", position.x >> 5, position.z >> 5))
            .join(format!("{}.{}.entities", position.x, position.z))
    }

    /// The entities file of `position` as it is on disk, if there is one.
    fn saved(&self, position: ChunkPos) -> Option<EntitiesFile> {
        let bytes = std::fs::read(self.entities_file(position)).ok()?;
        EntitiesFile::decode(&bytes).ok()
    }
}

struct Link {
    end: EdgeEnd,
    log: Vec<WorkerToEdge>,
    next: u64,
}

impl Link {
    fn hello(runner: &RegionRunner, edge: EdgeId) -> Self {
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
            chunks: view(),
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

    fn snapshot(&self, position: ChunkPos) -> bool {
        self.log.iter().any(|message| {
            matches!(message, WorkerToEdge::ChunkSnapshot { position: at, .. } if *at == position)
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

    /// Every entity other than a player the link was told was spawned, with the tick.
    fn others_spawned(&self) -> Vec<(u64, EntityState)> {
        let mut found = Vec::new();
        for message in &self.log {
            if let WorkerToEdge::TickDelta { tick, events } = message {
                for event in events {
                    if let RegionEvent::EntitySpawned(state) = event
                        && matches!(state.kind, EntityKind::Other { .. })
                    {
                        found.push((*tick, state.clone()));
                    }
                }
            }
        }
        found
    }

    fn gone(&self, key: EntityId) -> Option<u64> {
        self.log.iter().find_map(|message| {
            match message {
            WorkerToEdge::TickDelta { tick, events }
                if events.iter().any(|event| {
                    matches!(event, RegionEvent::EntityGone { entity, .. } if *entity == key)
                }) =>
            {
                Some(*tick)
            }
            _ => None,
        }
        })
    }
}

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
        thread::sleep(Duration::from_millis(1));
    }
    panic!(
        "never happened: {what}; the region is at tick {}",
        runner.region().tick_number()
    );
}

fn run_ticks(runner: &mut RegionRunner, link: &mut Link, ticks: u64) {
    let until = runner.region().tick_number() + ticks;
    run_until(runner, link, "ticks passing", |runner, _| {
        runner.region().tick_number() >= until
    });
}

fn with_a_player(runner: &mut RegionRunner, edge: EdgeId) -> (Link, EntityId) {
    let mut link = Link::hello(runner, edge);
    run_until(runner, &mut link, "the spawn's chunk", |_, link| {
        link.snapshot(HOME)
    });
    link.numbered(EdgeToWorker::PlayerJoin(PlayerJoin {
        player: player(1),
        name: "Thrower".to_owned(),
        attempt: 7001,
    }));
    run_until(runner, &mut link, "the player entering", |_, link| {
        link.spawned(player(1)).is_some()
    });
    let entity = link.spawned(player(1)).expect("just waited for it");
    (link, entity)
}

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

/// The record of `key` in the spawn's chunk, as the region has it.
fn record(runner: &RegionRunner, key: EntityId) -> Option<Entity> {
    let chunk = runner.region().chunk(HOME)?;
    chunk
        .entities()
        .iter()
        .find(|entity| entity.key == key)
        .cloned()
}

/// Throws one stone, looking ahead (towards +z); returns the item's key and the tick
/// it was thrown in, in which it ticked for the first time.
fn throw_one(runner: &mut RegionRunner, link: &mut Link, entity: EntityId) -> (EntityId, u64) {
    send_input(
        link,
        entity,
        PlayerInput::Drop {
            all: false,
            crouching: false,
        },
    );
    run_until(runner, link, "an item", |_, link| {
        !link.others_spawned().is_empty()
    });
    let (tick, state) = link.others_spawned()[0].clone();
    (state.entity, tick)
}

/// Steps until the item `key` has lain still on the ground for 45 ticks.
fn until_at_rest(runner: &mut RegionRunner, link: &mut Link, key: EntityId) -> u64 {
    let mut still_since = None;
    let mut last = None;
    run_until(runner, link, "the item at rest", |runner, _| {
        let Some(now) = record(runner, key) else {
            return false;
        };
        let tick = runner.region().tick_number();
        if !now.on_ground || last != Some(now.position) {
            still_since = Some(tick);
            last = Some(now.position);
        }
        still_since.is_some_and(|since| tick >= since + 45)
    });
    runner.region().tick_number()
}

/// The age and the delay left of an item thrown in tick `thrown`, as of the end of
/// tick `tick`: it ticked in its throw's tick, from 40.
fn counters_at(thrown: u64, tick: u64) -> (u32, u16) {
    let age = u32::try_from(tick - thrown + 1).expect("small");
    (
        age,
        u16::try_from(40_u32.saturating_sub(age)).expect("small"),
    )
}

/// The motion along y of an item at rest on the ground as of the end of tick `tick`:
/// gravity's 0.04 added up over the ticks since its last fourth-tick move, which cut it
/// to nothing (`(tickCount + getId()) % 4`, `tickCount` the region's tick).
fn resting_fall(tick: u64, id: i32) -> f64 {
    let mut moved_on = tick;
    while (moved_on as i32).wrapping_add(id) % 4 != 0 {
        moved_on -= 1;
    }
    let mut fall = 0.0_f64;
    for _ in moved_on..tick {
        fall += -0.04;
    }
    fall
}

/// The plan, section 7 (D1): an item at rest commits nothing, yet the store's file of
/// its chunk, written at the checkpoints, has its age, delay and motion as of the
/// file's tick: the age counted from its throw, the delay counted down, and the motion
/// gravity has added since its last fourth-tick move.
#[test]
fn a_resting_item_s_age_delay_and_motion_reach_the_store_at_checkpoints_without_a_commit() {
    let mut world = World::new(Some(5));
    let (mut runner, _) = world.open();
    let (mut link, entity) = with_a_player(&mut runner, EdgeId(1));
    let (key, thrown) = throw_one(&mut runner, &mut link, entity);
    let rested = until_at_rest(&mut runner, &mut link, key);
    let id = key.game_id();
    let mut files = 0;
    let mut last_tick = 0;
    run_until(
        &mut runner,
        &mut link,
        "files of the item at rest",
        |_, _| {
            let Some(file) = world.saved(HOME) else {
                return false;
            };
            if file.tick <= rested + 5 || file.tick == last_tick {
                return false;
            }
            last_tick = file.tick;
            let [saved] = &file.entities[..] else {
                panic!("one entity: {:?}", file.entities);
            };
            assert_eq!(saved.key, key);
            let (age, delay) = counters_at(thrown, file.tick);
            assert_eq!(
                saved.lived, age,
                "the file of tick {}: {saved:?}",
                file.tick
            );
            let EntityData::Item { pick_up_delay, .. } = saved.data else {
                panic!("{saved:?}");
            };
            assert_eq!(pick_up_delay, delay, "the file of tick {}", file.tick);
            assert_eq!(
                saved.motion[1].to_bits(),
                resting_fall(file.tick, id).to_bits(),
                "the file of tick {}: {saved:?}",
                file.tick
            );
            assert!(saved.on_ground);
            files += 1;
            files >= 4
        },
    );
}

/// The brief, 3.1, and the plan, section 7 (D1): an item's age survives its region's
/// stop and start, counted on from where the stop left it, and it despawns in the tick
/// it reaches 6000, never seen older.
#[test]
fn an_item_s_age_survives_a_stop_and_start_and_it_despawns_at_6000() {
    let mut world = World::new(Some(20));
    let (mut runner, _) = world.open();
    let (mut link, entity) = with_a_player(&mut runner, EdgeId(1));
    let (key, thrown) = throw_one(&mut runner, &mut link, entity);
    until_at_rest(&mut runner, &mut link, key);
    run_ticks(&mut runner, &mut link, 2000);
    runner.begin_release();
    run_until(&mut runner, &mut link, "the release", |runner, _| {
        runner.ended().is_some()
    });
    assert_eq!(runner.ended(), Some(Ended::Released));
    let stopped_at = runner.region().tick_number();
    let at_stop = record(&runner, key).expect("lies there");
    assert_eq!(at_stop.lived, counters_at(thrown, stopped_at).0);
    let file = world.saved(HOME).expect("saved at the stop");
    assert_eq!(
        file.tick, stopped_at,
        "the stop saves the chunk as it leaves it"
    );
    let saved = file
        .entities
        .iter()
        .find(|saved| saved.key == key)
        .expect("in the file");
    assert_eq!(saved.lived, at_stop.lived);
    drop(runner);

    let (mut runner, _) = world.open();
    let mut link = Link::hello(&runner, EdgeId(2));
    run_until(&mut runner, &mut link, "the chunk again", |runner, _| {
        record(runner, key).is_some()
    });
    // Taken in as the store had it, and aged on from there one a tick.
    let taken_at = runner.region().tick_number();
    let again = record(&runner, key).expect("just waited for it");
    assert!(again.lived >= at_stop.lived, "{again:?} after {at_stop:?}");
    assert!(
        u64::from(again.lived - at_stop.lived) <= taken_at - stopped_at,
        "it aged {} in {} ticks",
        again.lived - at_stop.lived,
        taken_at - stopped_at
    );
    let mut oldest = again.lived;
    run_until(
        &mut runner,
        &mut link,
        "the item despawning",
        |runner, link| match record(runner, key) {
            Some(now) => {
                assert!(now.lived < LIFETIME, "{now:?}");
                oldest = now.lived;
                false
            }
            None => link.gone(key).is_some(),
        },
    );
    assert_eq!(
        oldest,
        LIFETIME - 1,
        "it went in the tick it would have been 6000"
    );
}

/// The sand of the test below, as the region has it: sand blocks at the two places it
/// can be, and falling sand.
fn sand(runner: &RegionRunner) -> (usize, usize) {
    let region = runner.region();
    let chunk = region.chunk(HOME).expect("loaded");
    let blocks = [-59, -60]
        .iter()
        .filter(|y| chunk.get(10, **y, 8) == Some(blocks::SAND))
        .count();
    let falling = chunk
        .entities()
        .iter()
        .filter(|entity| i32::from(entity.kind) == entity_types::FALLING_BLOCK)
        .count();
    (blocks, falling)
}

/// ADR-0028, section 3, and the plan, section 5 ("survives every interruption"): sand
/// standing on stone that a player breaks falls as an entity whose moves are in the
/// commits; a crash at every step around its fall leaves, once restored, exactly one
/// sand, a block or falling, never both and never none, and the restored region lands
/// a falling one.
#[test]
fn a_crash_at_any_step_of_a_fall_leaves_one_sand_which_then_lands() {
    let mut outcomes = [0; 3];
    for steps in 0..30 {
        let mut world = World::new(Some(4));
        let (mut runner, _) = world.open();
        let (mut link, entity) = with_a_player(&mut runner, EdgeId(1));
        // Stone on the grass at (10, -60, 8), sand on it.
        send_input(
            &mut link,
            entity,
            PlayerInput::UseItemOn {
                position: BlockPos::new(10, -61, 8),
                face: Face::Top,
                cursor: [0.5, 1.0, 0.5],
                sneaking: false,
                sequence: 1,
            },
        );
        send_input(
            &mut link,
            entity,
            PlayerInput::SetInventorySlot {
                slot: 1,
                stack: Some(ItemStack::new(items::SAND, 64)),
            },
        );
        send_input(&mut link, entity, PlayerInput::SelectSlot { slot: 1 });
        send_input(
            &mut link,
            entity,
            PlayerInput::UseItemOn {
                position: BlockPos::new(10, -60, 8),
                face: Face::Top,
                cursor: [0.5, 1.0, 0.5],
                sneaking: false,
                sequence: 2,
            },
        );
        run_until(&mut runner, &mut link, "the sand placed", |runner, _| {
            runner
                .region()
                .chunk(HOME)
                .is_some_and(|chunk| chunk.get(10, -59, 8) == Some(blocks::SAND))
        });
        // Let its placing settle into the store before the break.
        run_ticks(&mut runner, &mut link, 12);
        send_input(
            &mut link,
            entity,
            PlayerInput::Dig {
                position: BlockPos::new(10, -60, 8),
                sequence: 3,
            },
        );
        for _ in 0..steps {
            runner.step();
            link.drain();
            thread::sleep(Duration::from_millis(1));
        }
        let (_, falling_then) = sand(&runner);
        let _lost = runner;

        let (mut runner, _) = world.open();
        let mut link = Link::hello(&runner, EdgeId(2));
        run_until(&mut runner, &mut link, "the chunk again", |runner, _| {
            runner.region().chunk(HOME).is_some()
        });
        let (blocks_now, falling_now) = sand(&runner);
        let case = format!(
            "a crash {steps} steps after the break, {falling_then} falling then: \
             {blocks_now} blocks and {falling_now} falling after the restore"
        );
        assert_eq!(blocks_now + falling_now, 1, "{case}");
        let landed = runner
            .region()
            .chunk(HOME)
            .is_some_and(|chunk| chunk.get(10, -60, 8) == Some(blocks::SAND));
        let outcome = if falling_now == 1 {
            1
        } else if landed {
            2
        } else {
            0
        };
        outcomes[outcome] += 1;
        // Whatever was falling lands.
        run_ticks(&mut runner, &mut link, 30);
        let (blocks_after, falling_after) = sand(&runner);
        assert_eq!((blocks_after, falling_after), (1, 0), "{case}, 30 ticks on");
    }
    // The crashes fell before the break, while it fell and after it landed, or the test
    // tried less than it says.
    assert!(outcomes.iter().all(|count| *count > 0), "{outcomes:?}");
}
