//! Two regions side by side on one world store, each run by a runner with a viewer at
//! the line between them (`docs/adr/0025-effects-between-regions.md`): the runners take
//! copies of each other's border chunks, the store couples them, and water crosses the
//! line at its own pace.
//!
//! The world is cut at chunk x = 1: region 0 holds the chunks west of it and the home
//! chunk, region 1 those from it on. A water source is stored at the eastern edge of
//! region 0's chunk at the origin, with its tick scheduled.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use clustine_data::blocks;
use clustine_region::RegionId;
use clustine_rpc::link::{self, EdgeEnd};
use clustine_rpc::{
    EdgeMessage, EdgeToWorker, RegionHello, StoreReply, StoreRequest, WorkerToEdge,
};
use clustine_sim::api::HOTBAR_SLOTS;
use clustine_worker::{RegionRunner, RegionSettings};
use clustine_world::{BlockPos, ChunkPos, EdgeId, InChunk, ScheduledTick, Ticked};
use clustine_worldgen::FlatGenerator;
use clustine_worldstore::{Pins, Store, StoreHandle};

/// Where players would enter the world: in the home chunk, at the origin.
const SPAWN_BLOCK: BlockPos = BlockPos::new(13, -60, 8);

/// The water source, on the grass at the eastern edge of region 0, and the two blocks
/// east of it, across the line.
const SOURCE: BlockPos = BlockPos::new(15, -60, 1);
const ACROSS: BlockPos = BlockPos::new(16, -60, 1);
const BEYOND: BlockPos = BlockPos::new(17, -60, 1);

/// `clustine_blocks::fluid::WATER`: still water, as the game numbers fluids.
const WATER: u8 = 2;

/// How often the test steps both runners before it gives up.
const STEPS: usize = 20_000;

fn settings() -> RegionSettings {
    RegionSettings {
        starting_hotbar: [None; HOTBAR_SLOTS],
        return_after: 1_000,
        lowest_y: -64,
        // The rims of the views tick, and are claimed: that is how each region learns
        // that the chunks across the line are the other's.
        rings: true,
    }
}

fn hello(region: u32, epoch: u64) -> RegionHello {
    RegionHello {
        region: RegionId(region),
        epoch,
    }
}

/// Waits for the store's answer to a load, passing over the rest.
fn loaded(handle: &StoreHandle, position: ChunkPos) -> clustine_world::Chunk {
    for _ in 0..30_000 {
        match handle.try_reply() {
            Some(StoreReply::Loaded {
                position: at,
                chunk,
            }) if at == position => return chunk,
            Some(_) => {}
            None => thread::sleep(Duration::from_millis(1)),
        }
    }
    panic!("the store did not hand out {position:?}");
}

/// The world, with the water source stored and its tick due a little after the chunk
/// first ticks.
fn world() -> Store {
    let generator = Arc::new(FlatGenerator::classic().with_spawn(SPAWN_BLOCK));
    let pins = Pins::side_by_side(&[1]).expect("one cut ascends");
    let store = Store::memory_divided(generator, pins).expect("the areas do not overlap");
    let (handle, _) = store.open_region(hello(0, 1)).expect("a new region opens");
    let origin = ChunkPos::new(0, 0);
    handle.request(StoreRequest::Load { position: origin });
    let mut chunk = loaded(&handle, origin);
    let (x, z) = SOURCE.in_chunk();
    chunk.set(x, SOURCE.y, z, blocks::WATER);
    chunk.set_ticks(vec![ScheduledTick {
        at: InChunk::new(x, SOURCE.y, z),
        of: Ticked::Fluid(WATER),
        delay: 10,
        priority: 0,
    }]);
    handle.request(StoreRequest::Save {
        position: origin,
        tick: 0,
        chunk,
    });
    handle.flush();
    store
}

/// A runner for `region`, with a link whose edge views `chunks`.
fn viewed(
    store: &Store,
    region: u32,
    epoch: u64,
    chunks: Vec<ChunkPos>,
) -> (RegionRunner, EdgeEnd) {
    let (handle, restored) = store
        .open_region(hello(region, epoch))
        .expect("the region opens");
    let runner = RegionRunner::restore(settings(), handle, restored)
        .expect("what the store has is readable")
        .with_id(RegionId(region));
    let (edge, worker) = link::in_process::<EdgeMessage, WorkerToEdge>(8192);
    runner.links().attach(worker);
    edge.try_send(EdgeMessage::unnumbered(EdgeToWorker::Hello {
        edge: EdgeId(10 + u64::from(region)),
        start: 1,
        since: 0,
        seen: 0,
        players: Vec::new(),
        chunks,
        guests: Vec::new(),
    }))
    .expect("the link has room");
    (runner, edge)
}

fn columns(xs: std::ops::RangeInclusive<i32>) -> Vec<ChunkPos> {
    xs.flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// The block at `at` in what `runner`'s region has loaded, if it has its chunk.
fn block(runner: &RegionRunner, at: BlockPos) -> Option<clustine_data::BlockState> {
    let (x, z) = at.in_chunk();
    runner.region().chunk(at.chunk())?.get(x, at.y, z)
}

/// Coupled, water crosses the line on the tick it would in one region: the block
/// across the line is set a tick late, as an update is, and its own tick counts from
/// the tick the spread was made in. So the water reaches the block beyond four ticks
/// after the block across; in one region, and uncoupled, it takes five.
#[test]
fn water_crosses_a_coupled_line_at_its_own_pace() {
    let store = world();
    let (mut west, mut west_edge) = viewed(&store, 0, 2, columns(-1..=0));
    let (mut east, mut east_edge) = viewed(&store, 1, 1, columns(1..=2));
    let mut across = None;
    let mut beyond = None;
    for _ in 0..STEPS {
        west.step();
        east.step();
        for edge in [&mut west_edge, &mut east_edge] {
            while let Ok(Some(_)) = edge.try_recv() {}
        }
        let tick = east.region().tick_number();
        let water = |at| block(&east, at).is_some_and(|state| state != blocks::AIR);
        if across.is_none() && water(ACROSS) {
            across = Some(tick);
        }
        if beyond.is_none() && water(BEYOND) {
            beyond = Some(tick);
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    let (Some(across), Some(beyond)) = (across, beyond) else {
        panic!("the water did not cross: across {across:?}, beyond {beyond:?}");
    };
    assert_eq!(beyond - across, 4, "across at {across}, beyond at {beyond}");
    // The west did its side as in one region.
    assert!(block(&west, SOURCE).is_some_and(|state| state == blocks::WATER));
}
