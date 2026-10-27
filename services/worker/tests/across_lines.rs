//! Tests of the worker's part in carrying effects between regions, written from
//! `docs/adr/0025-effects-between-regions.md` (sections 1, 3, 5 and 8 and "Found while
//! building") and the public API alone, by someone who did not write the code (step E6).
//!
//! The world is cut into two stripes at chunk x = 1: region 0 holds the chunks west of
//! it, region 1 the rest. A test runs one or both regions with runners it steps itself,
//! or plays region 1 through a handle of its own, as `chunks.rs` does. An edge of each
//! runner subscribes to the chunks along the line for a viewer, so that the chunks there
//! tick with their neighbours' copies. Where a test compares with one region, it runs
//! the same land in a world that is not divided.
//!
//! Every wait steps until a state or a message is there; a fixed pause is only that of a
//! polling loop.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use clustine_data::{BlockState, blocks};
use clustine_region::RegionId;
use clustine_rpc::link::{self, EdgeEnd};
use clustine_rpc::{
    CopyUse, EdgeMessage, EdgeToWorker, RegionHello, StoreReply, StoreRequest, WorkerToEdge,
};
use clustine_sim::api::Inventory;
use clustine_worker::{RegionRunner, RegionSettings};
use clustine_world::{
    BlockPos, Chunk, ChunkGenerator, ChunkPos, EdgeId, InChunk, ScheduledTick, Ticked,
};
use clustine_worldgen::FlatGenerator;
use clustine_worldstore::{Pins, Store, StoreHandle};

/// Where the world is cut: region 0 holds the chunks with x below this.
const CUT: i32 = 1;
/// The spawn, in the home chunk at the origin, which region 0 holds.
const SPAWN_BLOCK: BlockPos = BlockPos::new(3, -60, 3);
/// The flat world's grass is at y = -61; what the tests build stands on it.
const ON_GROUND: i32 = -60;
/// The chunk west of the line and the one east of it.
const WEST: ChunkPos = ChunkPos::new(0, 0);
const EAST: ChunkPos = ChunkPos::new(1, 0);
/// How often a wait steps before it gives up: a few times longer than any test needs.
const STEPS: usize = 20_000;

fn generator() -> Arc<FlatGenerator> {
    Arc::new(FlatGenerator::classic().with_spawn(SPAWN_BLOCK))
}

fn settings() -> RegionSettings {
    // These tests count and follow the entities they make: no mob comes of itself.
    clustine_worker::set_spawn_mobs(false);
    RegionSettings {
        starting_inventory: Inventory::default(),
        return_after: 0,
        lowest_y: -64,
        rings: false,
    }
}

fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a block state"))
}

fn at(x: i32, z: i32) -> BlockPos {
    BlockPos::new(x, ON_GROUND, z)
}

/// The store's answers that belong to the stream rather than answer a request.
fn of_the_stream(reply: &StoreReply) -> bool {
    matches!(
        reply,
        StoreReply::Effects { .. }
            | StoreReply::CopyBase { .. }
            | StoreReply::CopyUpdate { .. }
            | StoreReply::CopyUsed { .. }
            | StoreReply::CoupleWith { .. }
            | StoreReply::Coupled { .. }
            | StoreReply::HandshakeOff { .. }
            | StoreReply::Shifted { .. }
            | StoreReply::Uncoupled { .. }
            | StoreReply::Progress { .. }
            | StoreReply::Mark { .. }
    )
}

/// A tick that [`land`] schedules: where, of what, and its delay.
type Started = (BlockPos, Ticked, i32);

/// The land of the scenario of ordinary play, as blocks set on the flat world, with the
/// ticks that set it going: a channel of water that crosses the line and comes back, sand
/// under which a wall torch across the line hangs, and a row of fences beside the line
/// that sand across it falls beside.
///
/// Nothing moves before the three hundredth tick in which its chunk ticks: by then the
/// test has seen the two regions coupled, and what crosses is coupled. Before that a chunk along the
/// line may tick beside a copy of land nobody uses yet, and what crosses then is late
/// (section 1), as it may be.
fn land() -> (Vec<(BlockPos, BlockState)>, Vec<Started>) {
    let mut set = Vec::new();
    let mut ticks = Vec::new();
    // The channel: from a source at (14, 12) east over the line to x = 16, two blocks
    // south, and back west to (14, 14), six blocks from the source. Everything else around it is stone.
    let channel = [(15, 12), (16, 12), (16, 13), (16, 14), (15, 14), (14, 14)];
    for x in 12..=17 {
        for z in 11..=15 {
            if (x, z) == (14, 12) || channel.contains(&(x, z)) {
                continue;
            }
            set.push((at(x, z), blocks::STONE));
        }
    }
    set.push((at(14, 12), blocks::WATER));
    let water = blocks::WATER
        .props()
        .fluid()
        .expect("water has a fluid")
        .fluid;
    ticks.push((at(14, 12), Ticked::Fluid(water), 300));

    let sand = Ticked::Block(blocks::SAND.block_id().expect("sand is a block") as u16);
    // Sand west of the line, one block above the ground, and a torch on its east face,
    // across the line.
    set.push((BlockPos::new(15, ON_GROUND + 1, 4), blocks::SAND));
    ticks.push((BlockPos::new(15, ON_GROUND + 1, 4), sand, 310));
    set.push((
        BlockPos::new(16, ON_GROUND + 1, 4),
        state("minecraft:wall_torch[facing=east]"),
    ));

    // A row of fences west of the line, joined to each other, and sand east of the line
    // above the ground, which falls beside them.
    set.push((at(15, 0), state("minecraft:oak_fence[south=true]")));
    set.push((
        at(15, 1),
        state("minecraft:oak_fence[north=true,south=true]"),
    ));
    set.push((at(15, 2), state("minecraft:oak_fence[north=true]")));
    for z in 0..=2 {
        set.push((BlockPos::new(16, ON_GROUND + 1, z), blocks::SAND));
        ticks.push((BlockPos::new(16, ON_GROUND + 1, z), sand, 320));
    }
    (set, ticks)
}

/// The chunk at `position` with what [`land`] sets in it.
fn built(position: ChunkPos) -> Chunk {
    let (set, ticks) = land();
    let mut chunk = generator().generate(position);
    for (block, state) in set {
        if block.chunk() == position {
            let (x, z) = block.in_chunk();
            chunk.set(x, block.y, z, state);
        }
    }
    let ticks: Vec<ScheduledTick> = ticks
        .into_iter()
        .filter(|(block, _, _)| block.chunk() == position)
        .map(|(block, of, delay)| {
            let (x, z) = block.in_chunk();
            ScheduledTick {
                at: InChunk::new(x, block.y, z),
                of,
                delay,
                priority: 0,
            }
        })
        .collect();
    chunk.set_ticks(ticks);
    chunk
}

/// Stores `chunks` as `region` of `store`, by an owner that goes again at once.
fn seed(store: &Store, region: u32, chunks: &[ChunkPos]) {
    let (handle, _) = store
        .open_region(RegionHello {
            region: RegionId(region),
            epoch: 1,
        })
        .expect("nobody has opened the region");
    for &position in chunks {
        handle.request(StoreRequest::Save {
            position,
            tick: 0,
            chunk: built(position),
            parts: clustine_rpc::Parts::ALL,
        });
    }
    handle.flush();
}

/// A runner of `region`, opened as its second owner, with an edge that subscribes to
/// `chunks` for a viewer.
struct Side {
    runner: RegionRunner,
    edge: EdgeEnd,
    /// For each block watched, the tick it first changed in and what it is now.
    changed: BTreeMap<BlockPos, (u64, BlockState)>,
    watched: Vec<BlockPos>,
}

impl Side {
    fn open(store: &Store, region: u32, epoch: u64, chunks: Vec<ChunkPos>) -> Self {
        let (handle, restored) = store
            .open_region(RegionHello {
                region: RegionId(region),
                epoch,
            })
            .expect("a higher epoch opens the region");
        let runner = RegionRunner::restore(settings(), handle, restored)
            .expect("what the store has is readable")
            .with_id(RegionId(region));
        let (edge, worker) = link::in_process::<EdgeMessage, WorkerToEdge>(8192);
        runner.links().attach(worker);
        let side = Self {
            runner,
            edge,
            changed: BTreeMap::new(),
            watched: Vec::new(),
        };
        side.say(EdgeToWorker::Hello {
            edge: EdgeId(10 + u64::from(region)),
            start: 1,
            since: 0,
            seen: 0,
            players: Vec::new(),
            chunks,
            guests: Vec::new(),
        });
        side
    }

    fn say(&self, body: EdgeToWorker) {
        self.edge
            .try_send(EdgeMessage::unnumbered(body))
            .expect("the link is open and has room");
    }

    fn tick(&self) -> u64 {
        self.runner.region().tick_number()
    }

    fn block(&self, block: BlockPos) -> Option<BlockState> {
        let (x, z) = block.in_chunk();
        self.runner
            .region()
            .chunk(block.chunk())?
            .get(x, block.y, z)
    }

    /// Steps once, reads the edge's link, and notes what changed of the blocks watched.
    fn step(&mut self) {
        let before = self.tick();
        self.runner.step();
        while let Ok(Some(_)) = self.edge.try_recv() {}
        let now = self.tick();
        assert!(now <= before + 1, "a step ran more than one tick");
        if now == before {
            return;
        }
        for &block in &self.watched {
            let Some(state) = self.block(block) else {
                continue;
            };
            match self.changed.get_mut(&block) {
                None => {
                    self.changed.insert(block, (u64::MAX, state));
                }
                Some((first, was)) if *was != state => {
                    if *first == u64::MAX {
                        *first = now;
                    }
                    *was = state;
                }
                Some(_) => {}
            }
        }
    }

    /// The tick in which `block` first changed from what it was when it was first seen.
    fn first_change(&self, block: BlockPos) -> Option<u64> {
        self.changed
            .get(&block)
            .map(|(first, _)| *first)
            .filter(|first| *first != u64::MAX)
    }
}

/// Steps every side until `done`.
fn run_until(sides: &mut [&mut Side], what: &str, done: impl Fn(&[&mut Side]) -> bool) {
    for _ in 0..STEPS {
        for side in sides.iter_mut() {
            side.step();
        }
        if done(sides) {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    let ticks: Vec<u64> = sides.iter().map(|side| side.tick()).collect();
    panic!("never happened: {what}; the regions are at ticks {ticks:?}");
}

/// The blocks the scenario of ordinary play watches: every block it builds on and
/// beside, from the ground to two above.
fn watched() -> Vec<BlockPos> {
    let mut blocks = Vec::new();
    for x in 12..=17 {
        for z in -1..=15 {
            for y in ON_GROUND..=ON_GROUND + 2 {
                blocks.push(BlockPos::new(x, y, z));
            }
        }
    }
    blocks
}

/// The chunks a viewer of each side subscribes to: the line's chunks and one more on
/// each side of them, so that the two chunks along the line tick with their copies.
fn west_view() -> Vec<ChunkPos> {
    (-1..=0)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

fn east_view() -> Vec<ChunkPos> {
    (1..=2)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// The scenario in one region: what the watched blocks come to and when each first
/// changed, after `ticks` ticks.
fn in_one_region(ticks: u64) -> (BTreeMap<BlockPos, BlockState>, BTreeMap<BlockPos, u64>) {
    let store = Store::memory_divided(generator(), Pins::whole_world()).expect("one area");
    seed(&store, 0, &[WEST, EAST]);
    let mut view = west_view();
    view.extend(east_view());
    let mut one = Side::open(&store, 0, 2, view);
    one.watched = watched();
    run_until(&mut [&mut one], "the region running its ticks", |sides| {
        sides[0].tick() >= ticks
    });
    let settled = watched()
        .into_iter()
        .map(|block| (block, one.block(block).expect("loaded")))
        .collect();
    let first = watched()
        .into_iter()
        .filter_map(|block| one.first_change(block).map(|tick| (block, tick)))
        .collect();
    (settled, first)
}

/// Steps both regions until they are seen to tick in lockstep: the western one, stepped
/// alone for a while, runs at most two ticks. Returns its tick then.
fn wait_coupled(west: &mut Side, east: &mut Side) -> u64 {
    for _ in 0..200 {
        for _ in 0..5 {
            west.step();
            east.step();
            thread::sleep(Duration::from_millis(1));
        }
        let from = west.tick();
        for _ in 0..25 {
            west.step();
            thread::sleep(Duration::from_millis(1));
        }
        if west.tick() <= from + 2 {
            return west.tick();
        }
    }
    panic!("the two regions were never seen in lockstep");
}

/// The divided world's two runners, with the land built on both sides of the line.
fn two_regions() -> (Store, Side, Side) {
    let store = Store::memory_divided(generator(), Pins::side_by_side(&[CUT]).expect("a cut"))
        .expect("stripes do not overlap");
    seed(&store, 0, &[WEST]);
    seed(&store, 1, &[EAST]);
    let mut west = Side::open(&store, 0, 2, west_view());
    let mut east = Side::open(&store, 1, 2, east_view());
    west.watched = watched()
        .into_iter()
        .filter(|block| block.chunk().x < CUT)
        .collect();
    east.watched = watched()
        .into_iter()
        .filter(|block| block.chunk().x >= CUT)
        .collect();
    (store, west, east)
}

fn block_of(west: &Side, east: &Side, block: BlockPos) -> Option<BlockState> {
    if block.chunk().x < CUT {
        west.block(block)
    } else {
        east.block(block)
    }
}

/// The scenario of ordinary play in one region and in two: what the watched blocks come
/// to in one region and when each first changed there, and the two regions' runners,
/// stepped until their land has come to the same and forty ticks more.
struct Across {
    first: BTreeMap<BlockPos, u64>,
    west: Side,
    east: Side,
}

fn played_across() -> Across {
    let (settled, first) = in_one_region(450);
    let (_store, mut west, mut east) = two_regions();
    let coupled_at = wait_coupled(&mut west, &mut east);
    run_until(
        &mut [&mut west, &mut east],
        "the land across the line coming to what it comes to in one region",
        |sides| {
            settled
                .iter()
                .all(|(block, state)| block_of(sides[0], sides[1], *block) == Some(*state))
        },
    );
    // And it stays so: nothing goes on changing that has settled in one region.
    let (from_west, from_east) = (west.tick(), east.tick());
    run_until(&mut [&mut west, &mut east], "forty more ticks", |sides| {
        sides[0].tick() >= from_west + 40 && sides[1].tick() >= from_east + 40
    });
    let differ: Vec<_> = settled
        .iter()
        .filter(|(block, state)| block_of(&west, &east, **block) != Some(**state))
        .map(|(block, state)| (*block, *state, block_of(&west, &east, *block)))
        .collect();
    assert_eq!(
        differ,
        [],
        "blocks that came to otherwise than in one region"
    );
    assert!(
        west.changed
            .values()
            .all(|(first, _)| *first == u64::MAX || *first > coupled_at),
        "something moved before the regions were seen coupled at {coupled_at}"
    );
    Across { first, west, east }
}

/// The tick in which `block` first changed on `side`.
fn ticks_of(side: &Side, block: BlockPos) -> u64 {
    side.first_change(block)
        .unwrap_or_else(|| panic!("{block:?} never changed"))
}

/// The block before the line that the water comes to first, the one it comes back to,
/// the one after that, and one east of the line that is not where it crosses.
const OUT: BlockPos = BlockPos::new(15, ON_GROUND, 12);
const BACK: BlockPos = BlockPos::new(15, ON_GROUND, 14);
const BEYOND: BlockPos = BlockPos::new(14, ON_GROUND, 14);
const OVER: BlockPos = BlockPos::new(16, ON_GROUND, 13);

impl Across {
    /// The western region's tick of the same border time as the eastern region's
    /// `tick`, as the water's pace across the line, which is that of one region,
    /// shows it.
    fn east_to_west(&self, tick: u64) -> i64 {
        tick as i64 - (ticks_of(&self.east, OVER) as i64 - ticks_of(&self.west, OUT) as i64)
            + (self.first[&OVER] as i64 - self.first[&OUT] as i64)
    }
}

/// Section 1: "While the two regions' border ticks they are coupled: a scheduled tick
/// that crosses runs on the tick it would run on in one region, so water ... cross[es] a
/// line at [its] own pace"; section 4: the spread, the update and the set across the line
/// are done by the holder; E4: "the ticks the holder schedules while it does an effect
/// count their delays from that tick". Water crosses the line and comes back; sand falls
/// from under a torch across the line; sand falls beside a row of fences across the
/// line. What the blocks come to is what they come to in one region, and the water comes
/// back, after crossing twice, at the pace of one region.
#[test]
fn water_a_torch_and_fences_across_a_line_end_as_in_one_region_at_its_pace() {
    let played = played_across();
    let (west, first) = (&played.west, &played.first);
    assert_eq!(
        ticks_of(west, BEYOND) - ticks_of(west, OUT),
        first[&BEYOND] - first[&OUT],
        "the water came back at another pace than in one region"
    );
}

/// Section 1: "an update (`neighborChanged`, `updateShape`), which the game runs within
/// the same tick, runs exactly one tick late"; section 8: "a coupled effect is taken at
/// the border time after its making whenever it is ready then". The torch across the
/// line from the sand under it goes a tick after it would in one region, the fence
/// across the line from the sand that lands beside it answers a tick after it would, and
/// the water's spread over the line shows a tick late both ways.
#[test]
fn an_update_across_a_coupled_line_runs_exactly_one_tick_late_both_ways() {
    let played = played_across();
    let (west, east, first) = (&played.west, &played.east, &played.first);
    let sand = BlockPos::new(15, ON_GROUND + 1, 4);
    let torch = BlockPos::new(16, ON_GROUND + 1, 4);
    assert_eq!(
        played.east_to_west(ticks_of(east, torch)) - ticks_of(west, sand) as i64,
        first[&torch] as i64 - first[&sand] as i64 + 1,
        "the torch did not go a tick after it would in one region"
    );
    let landed = at(16, 1);
    let fence = at(15, 1);
    assert_eq!(
        ticks_of(west, fence) as i64 - played.east_to_west(ticks_of(east, landed)),
        first[&fence] as i64 - first[&landed] as i64 + 1,
        "the fence did not answer a tick after it would in one region"
    );
    let crossing = at(16, 12);
    assert_eq!(
        played.east_to_west(ticks_of(east, crossing)) - ticks_of(west, OUT) as i64,
        first[&crossing] as i64 - first[&OUT] as i64 + 1,
        "the water's crossing to the east did not show a tick late"
    );
    assert_eq!(
        ticks_of(west, BACK) - ticks_of(west, OUT),
        first[&BACK] - first[&OUT] + 1,
        "the water's crossing back did not show a tick late"
    );
}

// Lockstep, with the eastern region played by the test.

/// The eastern region as the test plays it: a handle and what came on its stream.
struct Played {
    handle: StoreHandle,
    stream: Vec<StoreReply>,
    read: usize,
    committed: Vec<u64>,
    next: u64,
}

impl Played {
    fn open(store: &Store, region: u32, epoch: u64) -> Self {
        let (handle, restored) = store
            .open_region(RegionHello {
                region: RegionId(region),
                epoch,
            })
            .expect("the region opens");
        Self {
            handle,
            stream: Vec::new(),
            read: 0,
            committed: Vec::new(),
            next: restored.tick() + 1,
        }
    }

    fn pump(&mut self) {
        while let Some(reply) = self.handle.try_reply() {
            match reply {
                StoreReply::Committed { tick } => self.committed.push(tick),
                reply if of_the_stream(&reply) => self.stream.push(reply),
                _ => {}
            }
        }
    }

    fn load(&mut self, position: ChunkPos) {
        self.handle.request(StoreRequest::Load { position });
    }

    fn commit(&mut self) -> u64 {
        let tick = self.next;
        self.next += 1;
        self.handle.request(StoreRequest::Commit {
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
            arrivals: Vec::new(),
            arrivals_taken: Vec::new(),
            clock: Default::default(),
            claims: Vec::new(),
        });
        tick
    }

    /// Whether a message past those waited for before holds `wanted`; it is waited past.
    fn take(&mut self, wanted: impl Fn(&StoreReply) -> bool) -> Option<StoreReply> {
        self.pump();
        let found = self.stream[self.read..].iter().position(wanted)?;
        self.read += found + 1;
        Some(self.stream[self.read - 1].clone())
    }

    /// The highest progress of `partner` said on the stream.
    fn progress_of(&mut self, partner: RegionId) -> Option<u64> {
        self.pump();
        self.stream
            .iter()
            .filter_map(|reply| match reply {
                StoreReply::Progress {
                    partner: of,
                    through,
                } if *of == partner => Some(*through),
                _ => None,
            })
            .max()
    }
}

/// A message of the stream without the chunk a base carries.
fn brief(reply: &StoreReply) -> String {
    match reply {
        StoreReply::CopyBase {
            position,
            sequence,
            last_change,
            chunk,
            used,
            ..
        } => format!(
            "CopyBase {position:?} {sequence} {last_change} {} {used:?}",
            if chunk.is_some() { "chunk" } else { "none" }
        ),
        other => format!("{other:?}"),
    }
}

/// Steps `side` until `done` holds of it and the played region.
fn step_until(
    side: &mut Side,
    played: &mut Played,
    what: &str,
    done: impl Fn(&Side, &mut Played) -> bool,
) {
    for _ in 0..STEPS {
        side.step();
        if done(side, played) {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    let heard: Vec<String> = played.stream.iter().map(brief).collect();
    panic!(
        "never happened: {what}; the region is at tick {}, the played one heard {heard:?}",
        side.tick(),
    );
}

/// The western region runs with a viewer along the line; the eastern one, played by the
/// test, uses the chunk across the line. The western region copies it beside a chunk
/// that would tick, so the store has the two shake hands. Returns the runner, the played
/// region and the runner's tick that it holds.
fn handshake_begun(store: &Store) -> (Side, Played) {
    let mut east = Played::open(store, 1, 1);
    east.load(EAST);
    for _ in 0..3 {
        east.commit();
    }
    let mut west = Side::open(store, 0, 1, west_view());
    step_until(
        &mut west,
        &mut east,
        "the store asking for a handshake",
        |_, east| {
            east.take(|r| {
                matches!(
                    r,
                    StoreReply::CoupleWith {
                        partner: RegionId(0)
                    }
                )
            })
            .is_some()
        },
    );
    (west, east)
}

/// Section 8, the handshake, step 2: "Each runner, taking it, names its next tick not yet
/// run, commits a record 'joins at tick n', and does not run tick n until the handshake
/// ends"; and "The rule": "A coupled region runs its tick at border time t only once its
/// stream has said that each partner is durable through border time t - 1."
#[test]
fn a_coupled_region_runs_no_tick_before_its_partner_is_durable_through_the_one_before() {
    let store = Store::memory_divided(generator(), Pins::side_by_side(&[CUT]).expect("a cut"))
        .expect("stripes do not overlap");
    let (mut west, mut east) = handshake_begun(&store);
    // The runner holds its next tick while the played region has not joined.
    for _ in 0..5 {
        west.step();
        thread::sleep(Duration::from_millis(1));
    }
    let held = west.tick();
    for _ in 0..20 {
        west.step();
        thread::sleep(Duration::from_millis(1));
        assert_eq!(
            west.tick(),
            held,
            "a tick ran in the middle of the handshake"
        );
    }
    let named = east.next;
    east.handle.request(StoreRequest::JoinsAt {
        partner: RegionId(0),
        tick: named,
    });
    let mut offset = None;
    step_until(&mut west, &mut east, "the coupling", |_, east| {
        east.pump();
        east.stream
            .iter()
            .any(|r| matches!(r, StoreReply::Coupled { .. }))
    });
    for reply in &east.stream {
        if let StoreReply::Coupled {
            partner,
            offset: told,
        } = reply
        {
            assert_eq!(*partner, RegionId(0));
            offset = Some(*told);
        }
    }
    let offset = offset.expect("coupled");
    let border = |tick: u64| u64::try_from(tick as i64 + offset).expect("not below 0");
    // The runner runs the tick it named, whose border time is that of the played
    // region's named tick, and no further until the played region has committed that.
    step_until(
        &mut west,
        &mut east,
        "the runner's named tick",
        |west, east| {
            west.tick() == held + 1 && east.progress_of(RegionId(0)) == Some(border(named))
        },
    );
    for round in 0..5 {
        for _ in 0..20 {
            west.step();
            thread::sleep(Duration::from_millis(1));
            assert_eq!(
                west.tick(),
                held + 1 + round,
                "the runner ran ahead of its partner in round {round}"
            );
            if let Some(through) = east.progress_of(RegionId(0)) {
                assert!(
                    through <= border(named + round),
                    "durable through {through}, a partner through {}",
                    border(named + round) - 1
                );
            }
        }
        east.commit();
        step_until(
            &mut west,
            &mut east,
            "the runner's next tick",
            |west, east| {
                west.tick() == held + 2 + round
                    && east.progress_of(RegionId(0)) == Some(border(named + round + 1))
            },
        );
    }
    assert!(
        !east
            .stream
            .iter()
            .any(|r| matches!(r, StoreReply::Uncoupled { .. })),
        "{:?}",
        east.stream
    );
}

/// Section 8, "Never stuck": a partner that stops "holds its partners back for up to
/// `COUPLING_PATIENCE`" (Consequences), "The store gives up a stalled region ... the
/// regions that waited for it tick on"; and "The store also ends a region's couplings at
/// once when its handle is closed".
#[test]
fn a_partner_that_stops_or_goes_does_not_hold_the_region_for_good() {
    for goes in [false, true] {
        let store = Store::memory_divided(generator(), Pins::side_by_side(&[CUT]).expect("a cut"))
            .expect("stripes do not overlap");
        let (mut west, mut east) = handshake_begun(&store);
        let named = east.next;
        east.handle.request(StoreRequest::JoinsAt {
            partner: RegionId(0),
            tick: named,
        });
        step_until(&mut west, &mut east, "the coupling", |_, east| {
            east.pump();
            east.stream
                .iter()
                .any(|r| matches!(r, StoreReply::Coupled { .. }))
        });
        let held = west.tick();
        if goes {
            drop(east);
        }
        for _ in 0..STEPS {
            west.step();
            if west.tick() >= held + 20 {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            west.tick() >= held + 20,
            "the region was held at tick {} (the partner {})",
            west.tick(),
            if goes { "went" } else { "stopped" }
        );
    }
}

/// Section 8, the handshake, step 4: "The store calls a handshake off ... when one side
/// has not made its 'joins at' durable within `COUPLING_PATIENCE`"; the held tick then
/// runs.
#[test]
fn a_handshake_the_partner_never_answers_does_not_hold_the_region_for_good() {
    let store = Store::memory_divided(generator(), Pins::side_by_side(&[CUT]).expect("a cut"))
        .expect("stripes do not overlap");
    let (mut west, mut east) = handshake_begun(&store);
    let held = west.tick();
    step_until(&mut west, &mut east, "the runner ticking on", |west, _| {
        west.tick() >= held + 20
    });
    assert!(
        east.take(|r| matches!(
            r,
            StoreReply::HandshakeOff {
                partner: RegionId(0)
            }
        ))
        .is_some(),
        "{:?}",
        east.stream
    );
}

// `Dropped`, as the eastern region copying a chunk of the western one hears of it.

/// What the played region is told of the use of `chunk`, in order.
fn uses_told(played: &mut Played, chunk: ChunkPos) -> Vec<CopyUse> {
    played.pump();
    played
        .stream
        .iter()
        .filter_map(|reply| match reply {
            StoreReply::CopyBase { position, used, .. } if *position == chunk => Some(*used),
            StoreReply::CopyUsed { position, used } if *position == chunk => Some(*used),
            _ => None,
        })
        .collect()
}

/// A chunk of the western region away from the home chunk, which is never let go.
const OWN: ChunkPos = ChunkPos::new(-3, 0);

/// Section 5: use ends "at a new request `Dropped`, which the runner sends whenever it
/// stops having a held chunk loaded or warm, for whatever reason (its tickets went ...)";
/// E5: "of every held chunk it stops having loaded that it does not return in the same
/// tick and does not hold warm".
#[test]
fn a_chunk_the_region_lets_go_of_is_said_dropped() {
    let store = Store::memory_divided(generator(), Pins::side_by_side(&[CUT]).expect("a cut"))
        .expect("stripes do not overlap");
    let mut east = Played::open(&store, 1, 1);
    east.handle.request(StoreRequest::Copies {
        wanted: vec![OWN],
        gone: Vec::new(),
    });
    let mut west = Side::open(&store, 0, 1, vec![OWN]);
    step_until(
        &mut west,
        &mut east,
        "the use of the chunk beginning",
        |_, east| uses_told(east, OWN).last() == Some(&CopyUse::Uncoupled(RegionId(0))),
    );
    west.say(EdgeToWorker::Unsubscribe {
        ask: 1,
        chunks: vec![OWN],
    });
    step_until(
        &mut west,
        &mut east,
        "the region letting go of the chunk",
        |west, _| west.runner.region().chunk(OWN).is_none(),
    );
    step_until(
        &mut west,
        &mut east,
        "the use of the chunk ending",
        |_, east| uses_told(east, OWN).last() == Some(&CopyUse::Nobody),
    );
}

/// E3a: "A load answered `Unreadable` ends no use in the store ... The runner says
/// `Dropped` of a chunk whose load was answered so".
#[test]
fn a_chunk_whose_load_is_unreadable_is_said_dropped() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let store = Store::local_divided(
        directory.path(),
        generator(),
        Pins::side_by_side(&[CUT]).expect("a cut"),
    )
    .expect("a new world");
    {
        let (handle, _) = store
            .open_region(RegionHello {
                region: RegionId(0),
                epoch: 1,
            })
            .expect("the region opens");
        handle.request(StoreRequest::Save {
            position: OWN,
            tick: 0,
            chunk: generator().generate(OWN),
            parts: clustine_rpc::Parts::ALL,
        });
        handle.flush();
    }
    let file = directory.path().join("chunks/overworld/-1.0/-3.0.chunk");
    assert!(file.exists(), "the chunk is stored at {file:?}");
    std::fs::write(&file, b"not a chunk").expect("the file is written");
    let mut east = Played::open(&store, 1, 1);
    east.handle.request(StoreRequest::Copies {
        wanted: vec![OWN],
        gone: Vec::new(),
    });
    let mut west = Side::open(&store, 0, 2, vec![OWN]);
    step_until(
        &mut west,
        &mut east,
        "the use beginning and ending",
        |_, east| {
            let told = uses_told(east, OWN);
            told.contains(&CopyUse::Uncoupled(RegionId(0))) && told.last() == Some(&CopyUse::Nobody)
        },
    );
}

/// Section 5, "Use timers": "For every chunk listed (by ... `Restored`) that the runner
/// neither has loaded nor holds warm in memory, it keeps a use timer, and sends `Dropped`
/// for the chunk if it has not loaded it within `WARM_FOR` ticks of the tick it began
/// from" (`WARM_FOR` is 30 seconds, 600 ticks); one it loads in time stays in use.
#[test]
fn a_chunk_in_use_after_a_crash_is_said_dropped_only_if_not_loaded_within_warm_for() {
    let store = Store::memory_divided(generator(), Pins::side_by_side(&[CUT]).expect("a cut"))
        .expect("stripes do not overlap");
    let kept = ChunkPos::new(-3, 5);
    {
        let (handle, _) = store
            .open_region(RegionHello {
                region: RegionId(0),
                epoch: 1,
            })
            .expect("the region opens");
        handle.request(StoreRequest::Load { position: OWN });
        handle.request(StoreRequest::Load { position: kept });
        handle.flush();
    }
    let mut east = Played::open(&store, 1, 1);
    east.handle.request(StoreRequest::Copies {
        wanted: vec![OWN, kept],
        gone: Vec::new(),
    });
    // The crashed owner's successor: nobody asks for the one chunk, and a viewer for
    // the other after a while.
    let mut west = Side::open(&store, 0, 2, Vec::new());
    let from = west.tick();
    step_until(&mut west, &mut east, "both copies based", |_, east| {
        !uses_told(east, OWN).is_empty() && !uses_told(east, kept).is_empty()
    });
    assert_eq!(uses_told(&mut east, OWN), [CopyUse::Uncoupled(RegionId(0))]);
    step_until(&mut west, &mut east, "a few ticks", |west, _| {
        west.tick() >= from + 20
    });
    west.say(EdgeToWorker::Subscribe {
        ask: 1,
        chunks: vec![kept],
    });
    step_until(
        &mut west,
        &mut east,
        "the use of the chunk not asked for ending",
        |_, east| uses_told(east, OWN).last() == Some(&CopyUse::Nobody),
    );
    assert!(
        west.tick() >= from + 600,
        "dropped after {} ticks",
        west.tick() - from
    );
    step_until(&mut west, &mut east, "some ticks more", |west, _| {
        west.tick() >= from + 700
    });
    store.flush().expect("the store is well");
    assert_eq!(
        uses_told(&mut east, kept),
        [CopyUse::Uncoupled(RegionId(0))],
        "the chunk loaded in time stays in use"
    );
}
