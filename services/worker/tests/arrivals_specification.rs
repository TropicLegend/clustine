//! Tests of the worker's part in entities crossing a line, written from
//! `docs/adr/0028-entities.md` (sections 5 to 7 and its review) and
//! `docs/groundwork/n3-plan.md` (all of it, its sections 5 and 6 over the earlier ones)
//! and the public API alone, by someone who did not write the code.
//!
//! The world is cut into two stripes at chunk x = 1: region 0 holds the chunks west of
//! it, the spawn's among them, and region 1 the rest. A player stands at the spawn, two
//! blocks west of the line, facing east, and throws a stone over it. An edge of each
//! runner subscribes to the chunks on its side for a viewer, so that the chunks along
//! the line tick with their neighbours' copies.
//!
//! A crash of a region is the region opened at the same store with a higher epoch while
//! the old runner is still there, never stepped again. A crash of the store is the store
//! left where it rests with everything asked of it and never asked anything again, its
//! runners with it, and another store started on the same directory. After a crash both
//! regions are opened anew and run until what was thrown has come to rest; the stone is
//! then in exactly one place, by its key, and never in two on the way.
//!
//! Where a test needs an arrival sent at a moment of its choosing, it plays the sender
//! through a store handle of its own, which commits a tick that sends a stone and
//! nothing else; the worlds of those tests are also the gap (land nobody holds between
//! two pinned regions and the home region) and three stripes (a lend at the first line).
//!
//! Every wait steps until a state or a message is there; a fixed pause is only that of a
//! polling loop.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use clustine_data::{BlockState, items};
use clustine_format::EntitiesFile;
use clustine_region::RegionId;
use clustine_rpc::link::{self, EdgeEnd};
use clustine_rpc::{
    EdgeMessage, EdgeToWorker, RegionHello, Restored, StoreReply, StoreRequest, WorkerToEdge,
};
use clustine_sim::api::{EntityKind, Inventory, ItemStack, PlayerInput, RegionEvent};
use clustine_sim::{Knowledge, RemoteAction, RemoteStep};
use clustine_sim::{PlayerEvent, PlayerJoin};
use clustine_worker::{RegionRunner, RegionSettings, Reshape, Reshaped, absorbable};
use clustine_world::{
    Arrival, BlockPos, ChunkArea, ChunkGenerator, ChunkPos, EdgeId, Entity, EntityData, EntityId,
    PlayerId, Vec3,
};
use clustine_worldgen::FlatGenerator;
use clustine_worldstore::{Pins, Store, StoreHandle};
use uuid::Uuid;

/// Where the world is cut: region 0 holds the chunks with x below this.
const CUT: i32 = 1;
const WEST: RegionId = RegionId(0);
const EAST: RegionId = RegionId(1);
/// The spawn, in the home chunk at the origin, two blocks west of the line.
const SPAWN_BLOCK: BlockPos = BlockPos::new(14, -60, 8);
const HOME: ChunkPos = ChunkPos::new(0, 0);
const OVER: ChunkPos = ChunkPos::new(1, 0);

const STEPS: usize = 20_000;
const CAPACITY: usize = 8192;

fn generator() -> Arc<FlatGenerator> {
    Arc::new(FlatGenerator::classic().with_spawn(SPAWN_BLOCK))
}

fn settings() -> RegionSettings {
    // These tests count and follow the entities they make: no mob comes of itself.
    clustine_worker::set_spawn_mobs(false);
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

/// The chunks a viewer of each region subscribes to: its own, two deep from the line
/// and two either way along it.
fn view_of(region: RegionId) -> Vec<ChunkPos> {
    let xs = if region == WEST { -2..=0 } else { 1..=3 };
    xs.flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// A world on disk, cut as `pins` says, and the epochs its regions have been opened with.
struct World {
    store: Option<Store>,
    epochs: [u64; 3],
    directory: tempfile::TempDir,
    pins: Pins,
}

impl World {
    fn new(pins: Pins) -> Self {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let store = Self::start(directory.path(), &pins);
        Self {
            store: Some(store),
            epochs: [0; 3],
            directory,
            pins,
        }
    }

    fn cut() -> Self {
        Self::new(Pins::side_by_side(&[CUT]).expect("a cut"))
    }

    fn start(root: &Path, pins: &Pins) -> Store {
        Store::local_divided(root, generator(), pins.clone())
            .expect("the world in the directory opens")
    }

    fn store(&self) -> &Store {
        self.store.as_ref().expect("the store runs")
    }

    /// The store stops where it is: it has done what it was asked, and is never asked
    /// anything again, nor are its threads waited for; whoever ran a region of it has
    /// lost it. Another store starts on the same directory.
    fn crash_store(&mut self, sides: Vec<Side>) {
        let store = self.store.take().expect("the store runs");
        store.flush().expect("the store is well");
        // Kept from being dropped, so that nothing they let go of reaches the store.
        std::mem::forget(sides);
        std::mem::forget(store);
        self.store = Some(Self::start(self.directory.path(), &self.pins));
    }

    fn open_handle(&mut self, region: RegionId) -> (StoreHandle, Restored) {
        self.epochs[region.0 as usize] += 1;
        let epoch = self.epochs[region.0 as usize];
        self.store()
            .open_region(RegionHello { region, epoch })
            .expect("a higher epoch opens the region")
    }

    /// Opens `region` as its next owner, with a viewer of its side.
    fn open(&mut self, region: RegionId) -> Side {
        let (handle, restored) = self.open_handle(region);
        let runner = RegionRunner::restore(settings(), handle, restored)
            .expect("what the store has is readable")
            .with_id(region)
            .with_checkpoint_interval(40);
        Side::new(runner, region, view_of(region))
    }
}

/// A runner and the edge's end of its link, with what came over it.
struct Side {
    runner: RegionRunner,
    region: RegionId,
    end: EdgeEnd,
    log: Vec<WorkerToEdge>,
    next: u64,
}

impl Side {
    fn new(runner: RegionRunner, region: RegionId, chunks: Vec<ChunkPos>) -> Self {
        let (end, worker) = link::in_process::<EdgeMessage, WorkerToEdge>(CAPACITY);
        runner.links().attach(worker);
        let side = Self {
            runner,
            region,
            end,
            log: Vec::new(),
            next: 1,
        };
        side.send(EdgeMessage::unnumbered(EdgeToWorker::Hello {
            edge: EdgeId(10 + u64::from(region.0)),
            start: 1,
            since: 0,
            seen: 0,
            players: Vec::new(),
            chunks,
            guests: Vec::new(),
        }));
        side
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

    fn input(&mut self, entity: EntityId, input: PlayerInput) {
        let number = self.next;
        self.next += 1;
        self.send(EdgeMessage {
            number: Some(number),
            body: EdgeToWorker::Input {
                player: player(1),
                entity,
                number,
                input,
            },
        });
    }

    fn tick(&self) -> u64 {
        self.runner.region().tick_number()
    }

    fn step(&mut self) {
        self.runner.step();
        while let Ok(Some(message)) = self.end.try_recv() {
            self.log.push(message);
        }
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

    /// The events of the ticks told over the link, with their ticks.
    fn events(&self) -> impl Iterator<Item = (u64, &RegionEvent)> {
        self.log.iter().flat_map(|message| match message {
            WorkerToEdge::TickDelta { tick, events } => {
                events.iter().map(move |event| (*tick, event)).collect()
            }
            _ => Vec::new(),
        })
    }

    /// The first entity other than a player told spawned over the link.
    fn thrown(&self) -> Option<EntityId> {
        // The item thrown, not a mob the world spawned meanwhile.
        self.events().find_map(|(_, event)| match event {
            RegionEvent::EntitySpawned(state)
                if matches!(
                    state.kind,
                    EntityKind::Other { kind, .. } if i32::from(kind) == clustine_data::entity_types::ITEM
                ) =>
            {
                Some(state.entity)
            }
            _ => None,
        })
    }

    /// The records of `key` in the region's loaded chunks.
    fn records(&self, key: EntityId) -> Vec<Entity> {
        let region = self.runner.region();
        region
            .loaded_chunks()
            .into_iter()
            .filter_map(|position| region.chunk(position))
            .flat_map(|chunk| chunk.entities().iter().filter(|e| e.key == key).cloned())
            .collect()
    }

    fn loaded(&self, position: ChunkPos) -> bool {
        self.runner.region().chunk(position).is_some()
    }
}

/// Steps every side once, with a pause of the polling loop.
fn step_all(sides: &mut [&mut Side]) {
    for side in sides.iter_mut() {
        side.step();
    }
    thread::sleep(Duration::from_millis(1));
}

fn run_until(sides: &mut [&mut Side], what: &str, mut done: impl FnMut(&[&mut Side]) -> bool) {
    for _ in 0..STEPS {
        step_all(sides);
        if done(sides) {
            return;
        }
    }
    let ticks: Vec<u64> = sides.iter().map(|side| side.tick()).collect();
    panic!("never happened: {what}; the regions are at ticks {ticks:?}");
}

/// Joins player 1 at the spawn in the western region, facing east; returns their id.
fn join_facing_east(west: &mut Side, east: &mut Side) -> EntityId {
    run_until(&mut [west, east], "the spawn's chunk loaded", |sides| {
        sides[0].loaded(HOME) && sides[1].loaded(OVER)
    });
    west.numbered(EdgeToWorker::PlayerJoin(PlayerJoin {
        player: player(1),
        name: "Thrower".to_owned(),
        attempt: 7001,
    }));
    run_until(&mut [west, east], "the player entering", |sides| {
        sides[0].spawned(player(1)).is_some()
    });
    let entity = west.spawned(player(1)).expect("just waited for it");
    west.input(
        entity,
        PlayerInput::Move {
            position: None,
            rotation: Some((-90.0, 0.0)),
            on_ground: true,
        },
    );
    run_until(&mut [west, east], "the turn taken", |_| true);
    entity
}

fn throw(west: &mut Side, east: &mut Side, entity: EntityId) -> EntityId {
    west.input(
        entity,
        PlayerInput::Drop {
            all: false,
            crouching: false,
        },
    );
    run_until(&mut [west, east], "a stone thrown", |sides| {
        sides[0].thrown().is_some()
    });
    let key = west.thrown().expect("just waited for it");
    // It stays where it was thrown until the chunks about it tick, which they do once
    // the copies of the land over the line are there.
    let from = west.records(key)[0].position;
    run_until(&mut [west, east], "the stone flying", |sides| {
        sides[0]
            .records(key)
            .first()
            .is_none_or(|record| record.position != from)
    });
    key
}

/// Every record of `key` in the two regions, with the region that has it.
fn everywhere(sides: &[&mut Side], key: EntityId) -> Vec<(RegionId, Entity)> {
    sides
        .iter()
        .flat_map(|side| {
            side.records(key)
                .into_iter()
                .map(|record| (side.region, record))
        })
        .collect()
}

/// Steps the two sides until `key` has lain still on the ground in one of them for 40
/// ticks, never seeing it in two places or twice; returns where it lies.
fn until_at_rest_once(sides: &mut [&mut Side], key: EntityId, case: &str) -> (RegionId, Entity) {
    let mut still: Option<(RegionId, [f64; 3], u64)> = None;
    let mut found = None;
    run_until(sides, &format!("the stone at rest, {case}"), |sides| {
        let now = everywhere(sides, key);
        assert!(
            now.len() <= 1,
            "{case}: the stone is in two places: {now:?}"
        );
        let Some((region, record)) = now.into_iter().next() else {
            still = None;
            return false;
        };
        let tick = sides
            .iter()
            .find(|side| side.region == region)
            .expect("one of them")
            .tick();
        match still {
            Some((was, at, since))
                if was == region && at == record.position && record.on_ground =>
            {
                if tick >= since + 40 {
                    found = Some((region, record));
                    return true;
                }
            }
            _ => still = Some((region, record.position, tick)),
        }
        false
    });
    found.expect("just waited for it")
}

/// Section 5: "An entity whose position comes, in a tick, into a chunk its region does
/// not have loaded (another region's ...) is sent"; "The taking commit has the entity
/// `Whole` with its `hops` one higher". A stone thrown over the line comes to rest in the
/// eastern region, once, with one more hop than it was thrown with, and the western
/// region has it no longer.
#[test]
fn a_stone_thrown_over_a_pinned_line_lies_once_in_the_far_region_one_hop_on() {
    let mut world = World::cut();
    let mut west = world.open(WEST);
    let mut east = world.open(EAST);
    let entity = join_facing_east(&mut west, &mut east);
    let key = throw(&mut west, &mut east, entity);
    let thrown_hops = west.records(key).first().map(|record| record.hops);
    let (region, record) =
        until_at_rest_once(&mut [&mut west, &mut east], key, "thrown over the line");
    assert_eq!(region, EAST, "it lies at {:?}", record.position);
    assert_eq!(Some(record.hops), thrown_hops.map(|hops| hops + 1));
    let sent: BTreeSet<u32> = west
        .events()
        .filter_map(|(_, event)| match event {
            RegionEvent::HandedOn { entity, hops, .. } if *entity == key => Some(*hops),
            _ => None,
        })
        .collect();
    assert_eq!(
        sent,
        BTreeSet::from([record.hops - 1]),
        "the west handed it on once"
    );
}

/// What crashes in [`crash_around_a_crossing`].
#[derive(Debug, Clone, Copy)]
enum Crash {
    Sender,
    Holder,
    Both,
    Store,
}

/// Where the stone was when the crash came: in the sender, on its way, or taken.
fn when(west: &Side, east: &Side, key: EntityId) -> usize {
    if !west.records(key).is_empty() {
        0
    } else if east.records(key).is_empty() {
        1
    } else {
        2
    }
}

/// Throws a stone over the line, lets the two regions run `steps` steps, crashes what
/// `crash` says, opens what crashed anew and runs until the stone is at rest: it is
/// there once, and never was twice. Returns where the stone was at the crash.
fn crash_around_a_crossing(crash: Crash, steps: usize) -> usize {
    let mut world = World::cut();
    let mut west = world.open(WEST);
    let mut east = world.open(EAST);
    let entity = join_facing_east(&mut west, &mut east);
    let key = throw(&mut west, &mut east, entity);
    for _ in 0..steps {
        step_all(&mut [&mut west, &mut east]);
        let now = everywhere(&[&mut west, &mut east], key);
        assert!(now.len() <= 1, "in two places before the crash: {now:?}");
    }
    let then = when(&west, &east, key);
    let case = format!("a crash of {crash:?} {steps} steps after the throw (stage {then})");
    let mut lost = Vec::new();
    let (mut west, mut east) = match crash {
        Crash::Sender => {
            lost.push(west);
            (world.open(WEST), east)
        }
        Crash::Holder => {
            lost.push(east);
            (west, world.open(EAST))
        }
        Crash::Both => {
            lost.push(west);
            lost.push(east);
            (world.open(WEST), world.open(EAST))
        }
        Crash::Store => {
            world.crash_store(vec![west, east]);
            (world.open(WEST), world.open(EAST))
        }
    };
    let (region, record) = until_at_rest_once(&mut [&mut west, &mut east], key, &case);
    assert_eq!(region, EAST, "{case}: it lies at {:?}", record.position);
    // And stays so, once.
    let from = east.tick();
    run_until(&mut [&mut west, &mut east], "sixty ticks more", |sides| {
        let now = everywhere(sides, key);
        assert_eq!(now.len(), 1, "{case}: {now:?}");
        sides[1].tick() >= from + 60
    });
    drop(lost);
    then
}

/// Runs [`crash_around_a_crossing`] at every step from the throw until well after the
/// taking, and says that the crashes fell before the sending, while the stone was on its
/// way and after it was taken.
fn crash_at_every_step(crash: Crash) {
    let mut stages = [0; 3];
    for steps in 0..16 {
        stages[crash_around_a_crossing(crash, steps)] += 1;
    }
    eprintln!("the crashes of {crash:?} fell at stages {stages:?}");
    assert!(
        stages[0] > 0 && stages[2] > 0,
        "the crashes of {crash:?} fell at stages {stages:?}"
    );
}

/// Section 5: "Commit and arrival are one group, so the entity is in exactly one place in
/// every durable state"; section 6: "A sender restored after a crash that lost the
/// sending loads the entity again with `hops` one higher". The sender crashed at every
/// step around a crossing, before its commit is durable and after: the stone is there
/// once.
#[test]
fn a_crash_of_the_sender_at_any_step_of_a_crossing_leaves_the_stone_once() {
    crash_at_every_step(Crash::Sender);
}

/// Section 5: "A holder that cannot take it yet ... restarting: the arrival stays filed
/// and is sent again at the holder's hello"; "Taken once". The holder crashed at every
/// step around a crossing, before the arrival reaches it, before its taking is durable
/// and after: the stone is there once.
#[test]
fn a_crash_of_the_holder_at_any_step_of_a_crossing_leaves_the_stone_once() {
    crash_at_every_step(Crash::Holder);
}

/// Both regions crashed together at every step around a crossing: the stone is there
/// once.
#[test]
fn a_crash_of_both_regions_at_any_step_of_a_crossing_leaves_the_stone_once() {
    crash_at_every_step(Crash::Both);
}

/// The plan, section 2 (N3b, "The start"): "commits' arrivals filed from the log,
/// `arrivals_taken` and `ArrivalsDropped` remove". The store stopped and started again
/// at every step around a crossing, with the regions it ran: the stone is there once.
#[test]
fn a_crash_of_the_store_at_any_step_of_a_crossing_leaves_the_stone_once() {
    crash_at_every_step(Crash::Store);
}

/// The tick of the western region at which the comparison's stone is thrown, long after
/// the regions are coupled.
const THROWN_AT: u64 = 200;

/// Throws a stone in the world `pins` cuts, at [`THROWN_AT`], and follows it until it
/// lies on the ground: each record seen, with the tick of border time it is of (the
/// western region's tick, and the eastern one's moved by the distance between the two
/// as the lockstep shows it) and the region that had it.
fn followed(pins: Pins) -> Vec<(u64, RegionId, Entity)> {
    let one = pins == Pins::whole_world();
    let mut world = World::new(pins);
    let mut west = world.open(WEST);
    let mut east = None;
    if one {
        let mut view = view_of(WEST);
        view.extend(view_of(EAST));
        west = Side::new(west.runner, WEST, view);
    } else {
        east = Some(world.open(EAST));
    }
    let step = |west: &mut Side, east: &mut Option<Side>| {
        west.step();
        if let Some(east) = east.as_mut() {
            east.step();
        }
        thread::sleep(Duration::from_millis(1));
    };
    for _ in 0..STEPS {
        if west.loaded(HOME) {
            break;
        }
        step(&mut west, &mut east);
    }
    west.numbered(EdgeToWorker::PlayerJoin(PlayerJoin {
        player: player(1),
        name: "Thrower".to_owned(),
        attempt: 7001,
    }));
    for _ in 0..STEPS {
        if west.spawned(player(1)).is_some() {
            break;
        }
        step(&mut west, &mut east);
    }
    let entity = west.spawned(player(1)).expect("the player entered");
    west.input(
        entity,
        PlayerInput::Move {
            position: None,
            rotation: Some((-90.0, 0.0)),
            on_ground: true,
        },
    );
    while west.tick() < THROWN_AT {
        step(&mut west, &mut east);
    }
    assert_eq!(west.tick(), THROWN_AT, "the throw is late");
    west.input(
        entity,
        PlayerInput::Drop {
            all: false,
            crouching: false,
        },
    );
    let mut seen = Vec::new();
    let mut from_east = Vec::new();
    let mut distances: std::collections::BTreeMap<i64, usize> = Default::default();
    let mut last = (west.tick(), east.as_ref().map_or(0, Side::tick));
    let mut key = None;
    for _ in 0..STEPS {
        step(&mut west, &mut east);
        key = key.or_else(|| west.thrown());
        let Some(key) = key else {
            continue;
        };
        if west.tick() != last.0 {
            for record in west.records(key) {
                seen.push((west.tick(), WEST, record));
            }
        }
        if let Some(east) = &east {
            if east.tick() != last.1 {
                for record in east.records(key) {
                    from_east.push((east.tick(), record));
                }
            }
            if west.tick() == last.0 + 1 && east.tick() == last.1 + 1 {
                *distances
                    .entry(west.tick() as i64 - east.tick() as i64)
                    .or_insert(0) += 1;
            }
            last.1 = east.tick();
        }
        last.0 = west.tick();
        let landed = |record: &Entity| record.on_ground;
        if seen.last().is_some_and(|(_, _, record)| landed(record))
            || from_east.last().is_some_and(|(_, record)| landed(record))
        {
            // The distance between the two regions' ticks that the lockstep shows most.
            let distance = distances
                .iter()
                .max_by_key(|(_, count)| **count)
                .map_or(0, |(distance, _)| *distance);
            for (tick, record) in from_east {
                let border = u64::try_from(tick as i64 + distance).expect("after zero");
                seen.push((border, EAST, record));
            }
            return seen;
        }
    }
    panic!("the stone never came down: {seen:?}");
}

/// Section 5: "While coupled ..., the holder runs border time t + 1 only once the sender
/// is durable through t, and takes the arrival in it: no tick of the entity is lost";
/// "it ticks in that tick"; the plan, section 5 (D4): "a coupled holder does not take a
/// crossing in the tick it already ran, which would tick the entity twice in it". A stone
/// thrown over a coupled line is where the same stone thrown in a world of one region is,
/// in every tick of border time from its throw until it lands: not ticked twice in the
/// tick it crossed, nor left out of it.
#[test]
fn a_stone_over_a_coupled_line_ticks_once_in_every_tick_as_in_one_region() {
    let alone = followed(Pins::whole_world());
    let across = followed(Pins::side_by_side(&[CUT]).expect("a cut"));
    let in_one: std::collections::BTreeMap<u64, [f64; 3]> = alone
        .iter()
        .map(|(tick, _, record)| (*tick, record.position))
        .collect();
    let mut by_tick: std::collections::BTreeMap<u64, Vec<RegionId>> = Default::default();
    for (tick, region, record) in &across {
        by_tick.entry(*tick).or_default().push(*region);
        assert_eq!(
            in_one.get(tick),
            Some(&record.position),
            "at border tick {tick} in region {region:?}: {record:?}; in one region: {:?}",
            in_one.range(tick - 1..=tick + 1).collect::<Vec<_>>()
        );
    }
    assert!(
        by_tick.values().all(|regions| regions.len() == 1),
        "seen in two regions in one tick: {by_tick:?}"
    );
    let regions: Vec<RegionId> = across.iter().map(|(_, region, _)| *region).collect();
    assert!(
        regions.first() == Some(&WEST) && regions.last() == Some(&EAST),
        "it did not cross: {regions:?}"
    );
    // The tick it crossed in is the one tick neither has it at its end: the sender's.
    let ticks: Vec<u64> = by_tick.keys().copied().collect();
    let gaps: Vec<u64> = ticks
        .windows(2)
        .filter(|pair| pair[1] != pair[0] + 1)
        .map(|pair| pair[0] + 1)
        .collect();
    assert!(gaps.len() <= 1, "missing in the ticks after {gaps:?}");
}

/// A region the test plays through a handle of its own, with no runner: it sends
/// what it is told to and keeps what the store answers.
struct Played {
    handle: StoreHandle,
    restored: Restored,
    replies: Vec<StoreReply>,
    next: u64,
}

impl Played {
    fn open(world: &mut World, region: RegionId) -> Self {
        let (handle, restored) = world.open_handle(region);
        let next = restored.tick() + 1;
        Self {
            handle,
            restored,
            replies: Vec::new(),
            next,
        }
    }

    fn pump(&mut self) {
        while let Some(reply) = self.handle.try_reply() {
            self.replies.push(reply);
        }
    }

    /// Commits a tick that sends `arrivals` and nothing else, and waits until it is
    /// durable.
    fn send(&mut self, arrivals: Vec<(ChunkPos, Entity)>) -> u64 {
        let tick = self.next;
        self.next += 1;
        self.handle.request(StoreRequest::Commit {
            travellers: Vec::new(),
            tick,
            changes: Vec::new(),
            state: Vec::new(),
            stays: Vec::new(),
            ticks: Vec::new(),
            marks_taken: Vec::new(),
            seams: Vec::new(),
            effects: Vec::new(),
            taken: Vec::new(),
            events: Vec::new(),
            entity_changes: Vec::new(),
            arrivals,
            arrivals_taken: Vec::new(),
            clock: Default::default(),
            claims: Vec::new(),
            raids: Vec::new(),
        });
        self.wait(
            "the commit",
            |reply| matches!(reply, StoreReply::Committed { tick: of } if *of == tick),
        );
        tick
    }

    fn wait(&mut self, what: &str, wanted: impl Fn(&StoreReply) -> bool) {
        for _ in 0..STEPS {
            self.pump();
            if self.replies.iter().any(&wanted) {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("never answered: {what}; answered {:?}", self.replies);
    }

    /// Every arrival the store sent this handle, by key.
    fn arrivals_of(&mut self, key: EntityId) -> Vec<Arrival> {
        self.pump();
        self.replies
            .iter()
            .flat_map(|reply| match reply {
                StoreReply::Arrivals { arrivals } => arrivals.clone(),
                _ => Vec::new(),
            })
            .filter(|arrival| arrival.entity.key == key)
            .collect()
    }
}

/// A stone lying still on the ground at `position`, with `hops` changes of holder behind
/// it, which nobody may pick up for a long time.
fn stone(key: EntityId, position: [f64; 3], hops: u32) -> Entity {
    Entity {
        portal: Default::default(),
        key,
        kind: clustine_data::entity_types::ITEM as u16,
        uuid: Uuid::from_u128(u128::from(key.0) + 99),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops,
        lived: 1,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: EntityData::Item {
            stack: ItemStack::new(items::STONE, 1),
            pick_up_delay: 30_000,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
    }
}

/// The middle of the ground of `chunk`, where a stone lies.
fn ground_of(chunk: ChunkPos) -> [f64; 3] {
    [
        f64::from(chunk.x * 16) + 8.5,
        -60.0,
        f64::from(chunk.z * 16) + 8.5,
    ]
}

/// The entities file of `position` on disk, if there is one.
fn saved(world: &World, position: ChunkPos) -> Option<EntitiesFile> {
    let path = world
        .directory
        .path()
        .join("chunks/overworld")
        .join(format!("{}.{}", position.x >> 5, position.z >> 5))
        .join(format!("{}.{}.entities", position.x, position.z));
    let bytes = std::fs::read(path).ok()?;
    EntitiesFile::decode(&bytes).ok()
}

/// A chunk of the eastern region's pinned land far beyond its viewers.
const FAR: ChunkPos = ChunkPos::new(6, 0);

/// Section 5, "Held and not loaded": "A chunk an entity enters can be held by a region
/// that does not use it (a pinned region's land beyond its viewers). The arrival goes on
/// the holder's stream all the same, and the holder loads the chunk for it with an
/// arrival ticket ..., which lasts until the chunk is loaded and the arrival taken ...,
/// and then 100 ticks; the chunk goes with its next save when the ticket ends"; the plan,
/// section 6 (5): "The runner holds its pinned land". A stone sent into the eastern
/// region's land beyond its viewers is taken there with one more hop; the chunk is let go
/// no sooner than 100 ticks after, with the stone in its file; and the store sends it no
/// more.
#[test]
fn an_arrival_into_pinned_land_beyond_the_viewers_is_taken_and_its_chunk_let_go_after_100_ticks() {
    let mut world = World::cut();
    let mut east = world.open(EAST);
    run_until(&mut [&mut east], "the viewer's chunks", |sides| {
        sides[0].loaded(OVER)
    });
    let mut sender = Played::open(&mut world, WEST);
    let key = sender.restored.entity_ids.first;
    sender.send(vec![(FAR, stone(key, ground_of(FAR), 3))]);
    run_until(&mut [&mut east], "the stone taken", |sides| {
        !sides[0].records(key).is_empty()
    });
    let taken_at = east.tick();
    let records = east.records(key);
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].hops, 4, "taken with one more hop");
    assert_eq!(records[0].chunk(), FAR);
    run_until(&mut [&mut east], "the chunk let go", |sides| {
        assert!(sides[0].records(key).len() <= 1);
        !sides[0].loaded(FAR)
    });
    let let_go = east.tick() - taken_at;
    assert!(
        (100..=400).contains(&let_go),
        "let go {let_go} ticks after the taking"
    );
    world.store().flush().expect("the store is well");
    let file = saved(&world, FAR).expect("the chunk was saved");
    let in_file = file
        .entities
        .iter()
        .filter(|entity| entity.key == key)
        .count();
    assert_eq!(in_file, 1, "{:?}", file.entities);
    // Taken: nothing of it is sent again at the next hello.
    let _crashed = east;
    let mut again = Played::open(&mut world, EAST);
    again.handle.request(StoreRequest::Flush);
    again.wait("the flush", |reply| matches!(reply, StoreReply::Flushed));
    assert_eq!(again.arrivals_of(key), []);
}

/// The world of the gap: region 0 is pinned to the chunks west of x = 0 and region 1 to
/// those from x = 16 on; region 2 is home and holds the home chunk alone, and the chunks
/// in between are held by nobody until a region claims them.
fn gap() -> World {
    World::new(Pins {
        areas: vec![
            ChunkArea {
                min_x: None,
                max_x: Some(0),
            },
            ChunkArea {
                min_x: Some(16),
                max_x: None,
            },
        ],
    })
}

/// A chunk of the gap that nobody holds.
const FREE: ChunkPos = ChunkPos::new(5, 0);

/// Opens `region` of `world` as its next owner, giving back what it does not use after
/// `return_after` ticks, with a viewer of `view`.
fn open_returning_after(
    world: &mut World,
    region: RegionId,
    return_after: u64,
    view: Vec<ChunkPos>,
) -> Side {
    let (handle, restored) = world.open_handle(region);
    let runner = RegionRunner::restore(
        RegionSettings {
            return_after,
            ..settings()
        },
        handle,
        restored,
    )
    .expect("what the store has is readable")
    .with_id(region)
    .with_checkpoint_interval(40);
    Side::new(runner, region, view)
}

/// Section 5: "into land held by no region ..., the store grants the chunk to the sending
/// region in the arrival's group ...; the sender loads the chunk with an arrival ticket
/// and takes its own entity in. A chunk held only for an arrival is given back as soon as
/// its arrival ticket ends and the arrival is taken, not after `return_after`". A stone
/// sent by region 0 into the gap is granted to it: the store says so ahead of the
/// arrival. Region 0's runner, opened after, takes it with one more hop and gives the
/// chunk back about 100 ticks later, long before its `return_after` of 6000 ticks, with
/// the stone in the chunk's file and nothing of it filed.
#[test]
fn a_stone_sent_into_land_nobody_holds_is_granted_to_its_sender_taken_and_given_back_when_its_ticket_ends()
 {
    let mut world = gap();
    let mut sender = Played::open(&mut world, RegionId(0));
    let key = sender.restored.entity_ids.first;
    sender.send(vec![(FREE, stone(key, ground_of(FREE), 0))]);
    sender.wait("the arrival on the stream", |reply| {
        matches!(reply, StoreReply::Arrivals { .. })
    });
    let granted = sender.replies.iter().position(
        |reply| matches!(reply, StoreReply::Claimed { granted, .. } if granted.contains(&FREE)),
    );
    let arrived = sender
        .replies
        .iter()
        .position(|reply| matches!(reply, StoreReply::Arrivals { .. }));
    assert!(
        granted.is_some() && granted < arrived,
        "the grant is not ahead of the arrival: {:?}",
        sender.replies
    );
    let mut region =
        open_returning_after(&mut world, RegionId(0), 6000, vec![ChunkPos::new(-2, 0)]);
    drop(sender);
    run_until(&mut [&mut region], "the stone taken", |sides| {
        !sides[0].records(key).is_empty()
    });
    let taken_at = region.tick();
    let records = region.records(key);
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].hops, 1);
    run_until(&mut [&mut region], "the chunk given back", |sides| {
        assert!(sides[0].records(key).len() <= 1);
        sides[0].runner.region().knowledge(FREE) != Knowledge::Held
    });
    let given_back = region.tick() - taken_at;
    assert!(
        (100..=1000).contains(&given_back),
        "given back {given_back} ticks after the taking"
    );
    world.store().flush().expect("the store is well");
    let file = saved(&world, FREE).expect("the chunk was saved");
    assert_eq!(
        file.entities
            .iter()
            .filter(|entity| entity.key == key)
            .count(),
        1,
        "{:?}",
        file.entities
    );
    let regions = world.store().regions().expect("the list");
    assert!(
        format!("{regions:?}").contains("RegionId(0)"),
        "the region is there"
    );
    let _crashed = region;
    let mut again = Played::open(&mut world, RegionId(0));
    again.handle.request(StoreRequest::Flush);
    again.wait("the flush", |reply| matches!(reply, StoreReply::Flushed));
    assert_eq!(again.arrivals_of(key), [], "the stone is filed still");
}

/// Section 5: "A chunk that cannot be read (`Unreadable`) holds no ticket: the worker
/// keeps its arrival, a `Return` of it is allowed whatever arrivals are filed, and they
/// stay filed for whoever is granted the chunk next"; "any grant of a chunk with arrivals
/// filed (a claim's, a lend's) puts them on the new holder's stream"; the plan, section 6
/// (3). A stone granted with its chunk to region 0, whose stored chunk cannot be read, is
/// not taken; the runner gives the chunk back, and the home region's claim of it is
/// granted with the stone on its stream.
#[test]
fn an_arrival_into_an_unreadable_chunk_stays_filed_through_its_return_for_the_next_holder() {
    let mut world = gap();
    // The chunk stored, and spoilt.
    {
        let (handle, _) = world.open_handle(RegionId(2));
        handle.request(StoreRequest::Claim { chunks: vec![FREE] });
        handle.request(StoreRequest::Save {
            position: FREE,
            tick: 0,
            chunk: generator().generate(FREE),
            parts: clustine_rpc::Parts::ALL,
        });
        handle.request(StoreRequest::Return {
            chunks: vec![FREE],
            unreadable: Vec::new(),
        });
        handle.flush();
    }
    let file = world
        .directory
        .path()
        .join(format!("chunks/overworld/0.0/{}.{}.chunk", FREE.x, FREE.z));
    assert!(file.exists(), "the chunk is stored at {file:?}");
    std::fs::write(&file, b"not a chunk").expect("the file is written");

    let mut sender = Played::open(&mut world, RegionId(0));
    let key = sender.restored.entity_ids.first;
    sender.send(vec![(FREE, stone(key, ground_of(FREE), 0))]);
    drop(sender);
    let mut region = open_returning_after(&mut world, RegionId(0), 0, vec![ChunkPos::new(-2, 0)]);
    run_until(&mut [&mut region], "the chunk given back", |sides| {
        assert!(sides[0].records(key).is_empty(), "taken from nothing");
        sides[0].tick() > 50 && sides[0].runner.region().knowledge(FREE) != Knowledge::Held
    });
    // Given back for good: it is not granted to the region again.
    let from = region.tick();
    run_until(&mut [&mut region], "a few hundred ticks", |sides| {
        assert_ne!(
            sides[0].runner.region().knowledge(FREE),
            Knowledge::Held,
            "the chunk was granted again at tick {}",
            sides[0].tick()
        );
        sides[0].tick() >= from + 300
    });
    world.store().flush().expect("the store is well");
    let mut next = Played::open(&mut world, RegionId(2));
    next.handle
        .request(StoreRequest::Claim { chunks: vec![FREE] });
    next.wait(
        "the claim's answer",
        |reply| matches!(reply, StoreReply::Claimed { granted, .. } if granted.contains(&FREE)),
    );
    next.wait("the stone on the stream", |reply| {
        matches!(reply, StoreReply::Arrivals { arrivals } if arrivals.iter().any(|a| a.entity.key == key))
    });
    assert_eq!(next.arrivals_of(key).len(), 1);
}

/// The plan, section 6 (9): "A key the holder has already ... the holder keeps its own
/// entity, counts the arrival as taken and logs an error, so that the stream heals and a
/// restore does not halt on it again"; ADR-0028, section 5: "A key the holder has already
/// is a malformed stream". A stone thrown over the line lies in the eastern region; an
/// arrival of another entity with its key, sent after, leaves the stone as it was, the
/// region running, and nothing filed for its next hello.
#[test]
fn an_arrival_of_a_key_the_holder_has_already_is_counted_taken_and_the_holder_keeps_its_own() {
    let mut world = World::cut();
    let mut west = world.open(WEST);
    let mut east = world.open(EAST);
    let entity = join_facing_east(&mut west, &mut east);
    let key = throw(&mut west, &mut east, entity);
    let (_, lying) = until_at_rest_once(&mut [&mut west, &mut east], key, "thrown");
    let _crashed = west;
    let mut sender = Played::open(&mut world, WEST);
    let elsewhere = ground_of(ChunkPos::new(2, 1));
    sender.send(vec![(ChunkPos::new(2, 1), stone(key, elsewhere, 7))]);
    let from = east.tick();
    run_until(&mut [&mut east], "a hundred ticks", |sides| {
        assert!(!sides[0].runner.store_is_lost(), "the region gave up");
        assert!(sides[0].runner.ended().is_none(), "the region ended");
        sides[0].tick() >= from + 100
    });
    let records = east.records(key);
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].position, lying.position, "its own stone moved");
    assert_eq!(records[0].hops, lying.hops);
    let _crashed = east;
    let mut again = Played::open(&mut world, EAST);
    again.handle.request(StoreRequest::Flush);
    again.wait("the flush", |reply| matches!(reply, StoreReply::Flushed));
    assert_eq!(again.arrivals_of(key), [], "the arrival is filed still");
}

/// Section 5: "A holder that cannot take it yet ...: the arrival stays filed and is sent
/// again ... with the holding at a merge ...: every arrival whose `Table::holder` the
/// merge ... changed, pinned areas the survivor took over among them, goes on its new
/// holder's stream"; "Arrivals kept in memory are not part of a region's state: an
/// absorbed region's come to the absorber by the store's resend, behind `Absorbed`". A
/// stone thrown over the line while the eastern region is not running is filed for it;
/// the western region absorbs it, loads the chunk the stone went to, which it holds now,
/// and takes the stone once, with one more hop.
#[test]
fn an_arrival_filed_for_a_region_that_is_absorbed_is_taken_once_by_the_survivor() {
    let mut world = World::cut();
    let mut west = world.open(WEST);
    run_until(&mut [&mut west], "the spawn's chunk loaded", |sides| {
        sides[0].loaded(HOME)
    });
    west.numbered(EdgeToWorker::PlayerJoin(PlayerJoin {
        player: player(1),
        name: "Thrower".to_owned(),
        attempt: 7001,
    }));
    run_until(&mut [&mut west], "the player entering", |sides| {
        sides[0].spawned(player(1)).is_some()
    });
    let entity = west.spawned(player(1)).expect("just waited for it");
    west.input(
        entity,
        PlayerInput::Move {
            position: None,
            rotation: Some((-90.0, 0.0)),
            on_ground: true,
        },
    );
    west.input(
        entity,
        PlayerInput::Drop {
            all: false,
            crouching: false,
        },
    );
    run_until(&mut [&mut west], "a stone thrown", |sides| {
        sides[0].thrown().is_some()
    });
    let key = west.thrown().expect("just waited for it");
    let thrown_hops = west.records(key)[0].hops;
    run_until(&mut [&mut west], "the stone sent over the line", |sides| {
        sides[0].records(key).is_empty()
    });
    world.store().flush().expect("the store is well");

    let (handle, restored) = world.open_handle(EAST);
    let epoch = world.epochs[EAST.0 as usize];
    let state = absorbable(&handle, restored).expect("what the store has is readable");
    let (said, heard) = mpsc::channel();
    west.runner.reshape(
        Reshape::Absorb {
            absorbed: EAST,
            absorbed_epoch: epoch,
            state,
        },
        Box::new(move |reshaped| {
            let _ = said.send(reshaped);
        }),
    );
    let mut outcome = None;
    run_until(&mut [&mut west], "the merge", |_| {
        outcome = outcome.take().or_else(|| heard.try_recv().ok());
        outcome.is_some()
    });
    assert_eq!(outcome, Some(Reshaped::Absorbed { absorbed: EAST }));
    drop(handle);
    // The merge closed the edge's link; a viewer of the land over the line comes again.
    let mut west = Side::new(west.runner, WEST, view_of(EAST));
    run_until(&mut [&mut west], "the stone taken", |sides| {
        !sides[0].records(key).is_empty()
    });
    let from = west.tick();
    run_until(&mut [&mut west], "a hundred ticks", |sides| {
        assert_eq!(sides[0].records(key).len(), 1, "taken more than once");
        sides[0].tick() >= from + 100
    });
    assert_eq!(west.records(key)[0].hops, thrown_hops + 1);
}

/// Gives region 0 of the gap a stone in [`FREE`] and lets its runner take it; the
/// runner, which gives back what it does not use at once, gives the chunk back once
/// the ticket ends. A stone of another key is sent into the chunk by the home region
/// `after` steps from the step in which the runner let go of it.
fn sent_into_a_chunk_being_given_back(after: usize) -> &'static str {
    let mut world = gap();
    let mut first = Played::open(&mut world, RegionId(0));
    let a = first.restored.entity_ids.first;
    first.send(vec![(FREE, stone(a, ground_of(FREE), 0))]);
    drop(first);
    let mut home = Played::open(&mut world, RegionId(2));
    let b = home.restored.entity_ids.first;
    let mut region = open_returning_after(&mut world, RegionId(0), 0, vec![ChunkPos::new(-2, 0)]);
    run_until(&mut [&mut region], "the first stone taken", |sides| {
        !sides[0].records(a).is_empty()
    });
    run_until(&mut [&mut region], "the chunk let go of", |sides| {
        sides[0].runner.region().knowledge(FREE) != Knowledge::Held
    });
    for _ in 0..after {
        step_all(&mut [&mut region]);
    }
    let mut position = ground_of(FREE);
    position[0] += 2.0;
    home.send(vec![(FREE, stone(b, position, 0))]);
    let case = format!("the second stone sent {after} steps after the return");
    // The runner holds the chunk again for it, takes it and gives the chunk back again,
    // or it was given back before and the stone is the home region's to take.
    let mut took = false;
    let mut quiet_since = region.tick();
    run_until(&mut [&mut region], &case, |sides| {
        let side = &sides[0];
        let now = side.records(b).len();
        assert!(now <= 1, "{case}: taken twice");
        took |= now == 1;
        if side.runner.region().knowledge(FREE) == Knowledge::Held {
            quiet_since = side.tick();
        }
        side.tick() >= quiet_since + 300
    });
    world.store().flush().expect("the store is well");
    let _crashed = region;
    drop(home);
    let mut again = Played::open(&mut world, RegionId(2));
    again.handle.request(StoreRequest::Flush);
    again.wait("the flush", |reply| matches!(reply, StoreReply::Flushed));
    let filed = again.arrivals_of(b).len();
    let file = saved(&world, FREE).expect("the chunk was saved");
    let count = |key| file.entities.iter().filter(|e| e.key == key).count();
    assert_eq!(count(a), 1, "{case}: the first stone in the chunk's file");
    if took {
        assert_eq!(filed, 0, "{case}: taken and filed for the home region");
        assert_eq!(count(b), 1, "{case}: the second stone in the chunk's file");
        "taken by the region that gave the chunk back"
    } else {
        assert_eq!(filed, 1, "{case}: lost");
        assert_eq!(count(b), 0, "{case}");
        "filed for the home region"
    }
}

/// Section 5: "a `Return` of a chunk with arrivals filed, where the release would leave
/// no region holding it ...: the store leaves the chunk out of `returning`, answers
/// `Claimed { granted }` for it, and sends its arrivals again ... for those filed while
/// the chunk is being returned, which call the return off as a claim does"; the plan,
/// section 5 (G3). A stone sent into a chunk of the gap at every step around its holder's
/// return of it is in exactly one place: taken by that holder, which holds the chunk
/// again for it, or, once the chunk is free, granted with it to its sender.
#[test]
fn an_arrival_into_a_chunk_being_given_back_calls_the_return_off_or_is_granted_to_its_sender() {
    let mut outcomes = BTreeSet::new();
    for after in 0..8 {
        outcomes.insert(sent_into_a_chunk_being_given_back(after));
    }
    eprintln!("{outcomes:?}");
    assert!(
        outcomes.contains("taken by the region that gave the chunk back"),
        "no arrival came while the chunk was being given back: {outcomes:?}"
    );
}

/// Wire on the grass of the west's chunk at the origin, a block from the line, which has
/// the chunks across the line lent to the west (`docs/adr/0027-lending-at-a-line.md`).
const WIRE: BlockPos = BlockPos::new(15, -60, 1);

fn wire() -> BlockState {
    BlockState::parse("minecraft:redstone_wire[east=none,north=none,power=0,south=none,west=none]")
        .expect("a state")
}

/// What a player of another region does to a block of the west, through its edge.
fn remote(side: &mut Side, step: RemoteStep) {
    let number = side.next;
    side.next += 1;
    side.send(EdgeMessage {
        number: Some(number),
        body: EdgeToWorker::Remote(RemoteAction {
            player: PlayerId(Uuid::from_u128(7)),
            entity: EntityId(70),
            sequence: number as i32,
            step,
        }),
    });
}

/// Three stripes, cut at 1 and 4: the west lends the chunks across the line from the
/// middle region when wire is placed by it, and the region of the far east sends a stone
/// into the chunk across `after` steps from the placing. Returns whether the chunk was
/// lent when the stone was sent.
fn sent_into_a_chunk_being_lent(after: usize) -> bool {
    let mut world = World::new(Pins::side_by_side(&[1, 4]).expect("the cuts ascend"));
    let columns = |xs: std::ops::RangeInclusive<i32>| -> Vec<ChunkPos> {
        xs.flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
            .collect()
    };
    let mut west = open_returning_after(&mut world, WEST, 1000, columns(-1..=0));
    let mut east = open_returning_after(&mut world, EAST, 1000, columns(1..=2));
    run_until(
        &mut [&mut west, &mut east],
        "the land on both sides",
        |sides| {
            sides[0].runner.region().knowledge(OVER) == Knowledge::Foreign(EAST)
                && sides[1].loaded(OVER)
                && sides[0].loaded(WIRE.chunk())
        },
    );
    let mut far = Played::open(&mut world, RegionId(2));
    let key = far.restored.entity_ids.first;
    remote(
        &mut west,
        RemoteStep::Place {
            target: WIRE,
            block: wire(),
            placer: Vec3::new(10.5, -60.0, 1.5),
        },
    );
    for _ in 0..after {
        step_all(&mut [&mut west, &mut east]);
    }
    let lent = west.runner.region().knowledge(OVER) == Knowledge::Held;
    far.send(vec![(OVER, stone(key, [20.5, -60.0, 8.5], 2))]);
    let case = format!("a stone sent {after} steps after the wire was placed");
    let once = |sides: &[&mut Side]| {
        let now = everywhere(sides, key);
        assert!(now.len() <= 1, "{case}: in two places: {now:?}");
        now.len() == 1
    };
    run_until(
        &mut [&mut west, &mut east],
        &format!("{case}: lent"),
        |sides| {
            once(sides)
                && sides[0].loaded(OVER)
                && sides[0].runner.region().knowledge(OVER) == Knowledge::Held
        },
    );
    let (region, _) = everywhere(&[&mut west, &mut east], key)[0].clone();
    assert_eq!(
        region, WEST,
        "{case}: the borrower has the chunk and the stone"
    );
    // The wire broken, the chunk goes back, with the stone in it.
    remote(&mut west, RemoteStep::Break { position: WIRE });
    run_until(
        &mut [&mut west, &mut east],
        &format!("{case}: given back"),
        |sides| {
            let once = once(sides);
            once && !sides[0].loaded(OVER)
                && sides[1].loaded(OVER)
                && sides[1].runner.region().knowledge(OVER) == Knowledge::Held
                && !sides[1].records(key).is_empty()
        },
    );
    lent
}

/// Section 5: "After `Lent`, the lender drops the arrivals it was sent for the chunk, as it
/// drops effects ..., and the store files them for the borrower with the grant"; "a lent
/// chunk given back ... passes its filed arrivals to the holder the release leaves";
/// "What a worker keeps ... drops them for a chunk it sends in ... a `Lending`". A stone
/// sent into a chunk at every step around its lend is in one place throughout: with the
/// borrower once the chunk is lent, and with the lender once it is given back.
#[test]
fn an_arrival_into_a_chunk_around_its_lend_is_in_one_place_with_the_borrower_and_back() {
    let mut seen = BTreeSet::new();
    for after in [0, 1, 2, 4, 8, 16, 32, 64, 128, 256] {
        seen.insert(sent_into_a_chunk_being_lent(after));
    }
    assert!(seen.contains(&true) && seen.contains(&false), "{seen:?}");
}
