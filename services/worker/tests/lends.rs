//! Two pinned regions side by side on one world store, each run by a runner with a
//! viewer at the line between them (`docs/adr/0027-lending-at-a-line.md`, step L3b):
//! wire placed by the line in the west has the chunks across it lent, the east lets
//! them go and sends its viewer to the west, and the west gives them back once the wire
//! is gone.
//!
//! The world is cut at chunk x = 1: region 0 holds the chunks west of it and the home
//! chunk, region 1 those from it on.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use clustine_data::BlockState;
use clustine_region::RegionId;
use clustine_rpc::link::{self, EdgeEnd};
use clustine_rpc::{EdgeMessage, EdgeToWorker, RegionHello, WorkerToEdge};
use clustine_sim::api::HOTBAR_SLOTS;
use clustine_sim::{Knowledge, RemoteAction, RemoteStep};
use clustine_worker::{RegionRunner, RegionSettings};
use clustine_world::{BlockPos, ChunkPos, EdgeId, EntityId, PlayerId, Vec3};
use clustine_worldgen::FlatGenerator;
use clustine_worldstore::{Pins, Store};

const SPAWN_BLOCK: BlockPos = BlockPos::new(13, -60, 8);
/// Wire on the grass of the west's chunk at the origin, a block from the line.
const WIRE: BlockPos = BlockPos::new(15, -60, 1);
/// The chunk across the line from it, and the one north of that, within two blocks.
const ACROSS: ChunkPos = ChunkPos::new(1, 0);
const ACROSS_NORTH: ChunkPos = ChunkPos::new(1, -1);
const WEST: RegionId = RegionId(0);
const EAST: RegionId = RegionId(1);
const STEPS: usize = 20_000;

fn settings() -> RegionSettings {
    RegionSettings {
        starting_hotbar: [None; HOTBAR_SLOTS],
        return_after: 1_000,
        lowest_y: -64,
        rings: true,
    }
}

fn hello(region: RegionId, epoch: u64) -> RegionHello {
    RegionHello { region, epoch }
}

fn world() -> Store {
    let generator = Arc::new(FlatGenerator::classic().with_spawn(SPAWN_BLOCK));
    let pins = Pins::side_by_side(&[1]).expect("one cut ascends");
    Store::memory_divided(generator, pins).expect("the areas do not overlap")
}

/// A runner for `region`, with a link of edge `10 + region` that views `chunks`.
fn viewed(store: &Store, region: RegionId, chunks: Vec<ChunkPos>) -> (RegionRunner, EdgeEnd) {
    let (handle, restored) = store
        .open_region(hello(region, 1))
        .expect("the region opens");
    let runner = RegionRunner::restore(settings(), handle, restored)
        .expect("what the store has is readable")
        .with_id(region);
    let (edge, worker) = link::in_process::<EdgeMessage, WorkerToEdge>(8192);
    runner.links().attach(worker);
    edge.try_send(EdgeMessage::unnumbered(EdgeToWorker::Hello {
        edge: EdgeId(10 + u64::from(region.0)),
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

fn wire() -> BlockState {
    BlockState::parse("minecraft:redstone_wire[east=none,north=none,power=0,south=none,west=none]")
        .expect("a state")
}

/// What another region's player does to a block of the west, through its edge.
fn remote(number: u64, step: RemoteStep) -> EdgeMessage {
    EdgeMessage {
        number: Some(number),
        body: EdgeToWorker::Remote(RemoteAction {
            player: PlayerId(uuid::Uuid::from_u128(7)),
            entity: EntityId(70),
            sequence: number as i32,
            step,
        }),
    }
}

/// Steps both runners until `done` holds, and returns what the east's edge was told
/// on the way.
fn until(
    west: &mut RegionRunner,
    east: &mut RegionRunner,
    edges: [&mut EdgeEnd; 2],
    mut done: impl FnMut(&RegionRunner, &RegionRunner) -> bool,
) -> Vec<WorkerToEdge> {
    let [west_edge, east_edge] = edges;
    let mut told = Vec::new();
    for _ in 0..STEPS {
        west.step();
        east.step();
        while let Ok(Some(_)) = west_edge.try_recv() {}
        while let Ok(Some(message)) = east_edge.try_recv() {
            told.push(message);
        }
        if done(west, east) {
            return told;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!(
        "it did not come: the west knows {:?} of the chunk across, the east {:?}; the west \
         has {:?} loaded",
        west.region().knowledge(ACROSS),
        east.region().knowledge(ACROSS),
        west.region().loaded_chunks(),
    );
}

#[test]
fn wire_by_the_line_is_lent_the_chunks_across_and_gives_them_back_once_it_is_gone() {
    let store = world();
    let (mut west, mut west_edge) = viewed(&store, WEST, columns(-1..=0));
    let (mut east, mut east_edge) = viewed(&store, EAST, columns(1..=2));
    // Both have their land, and the west knows whose the chunks across are.
    until(
        &mut west,
        &mut east,
        [&mut west_edge, &mut east_edge],
        |west, east| {
            west.region().knowledge(ACROSS) == Knowledge::Foreign(EAST)
                && east.region().chunk(ACROSS).is_some()
                && west.region().chunk(WIRE.chunk()).is_some()
        },
    );

    let place = RemoteStep::Place {
        target: WIRE,
        block: wire(),
        placer: Vec3::new(10.5, -60.0, 1.5),
    };
    west_edge
        .try_send(remote(1, place))
        .expect("the link has room");
    let mut told = until(
        &mut west,
        &mut east,
        [&mut west_edge, &mut east_edge],
        |west, east| {
            let lent = |chunk| west.region().knowledge(chunk) == Knowledge::Held;
            let away = |chunk| east.region().knowledge(chunk) == Knowledge::Foreign(WEST);
            lent(ACROSS) && lent(ACROSS_NORTH) && away(ACROSS) && away(ACROSS_NORTH)
        },
    );
    // The east's viewer of the chunk across is sent to the west once the tick that
    // learnt of the lend is durable, and is told nothing else of the chunk on the way.
    let sent_west = |message: &WorkerToEdge| {
        matches!(
            message,
            WorkerToEdge::Elsewhere { chunk, region, .. } if *chunk == ACROSS && *region == WEST
        )
    };
    for _ in 0..STEPS {
        if told.iter().any(sent_west) {
            break;
        }
        told.extend(until(
            &mut west,
            &mut east,
            [&mut west_edge, &mut east_edge],
            |_, _| true,
        ));
    }
    assert!(told.iter().any(sent_west), "{told:?}");
    let about_across = |message: &&WorkerToEdge| match message {
        WorkerToEdge::Elsewhere { chunk, .. }
        | WorkerToEdge::NotMine { chunk, .. }
        | WorkerToEdge::ChunkSnapshot {
            position: chunk, ..
        } => *chunk == ACROSS,
        _ => false,
    };
    let across: Vec<_> = told.iter().filter(about_across).collect();
    assert_eq!(across.len(), 1, "{across:?}");
    // The west loads what it was lent, with nobody looking at it.
    until(
        &mut west,
        &mut east,
        [&mut west_edge, &mut east_edge],
        |west, _| {
            west.region().chunk(ACROSS).is_some() && west.region().chunk(ACROSS_NORTH).is_some()
        },
    );

    // The wire broken, both go back, and the east has them as its own once it claims them.
    let broken = RemoteStep::Break { position: WIRE };
    west_edge
        .try_send(remote(2, broken))
        .expect("the link has room");
    until(
        &mut west,
        &mut east,
        [&mut west_edge, &mut east_edge],
        |west, east| {
            west.region().chunk(ACROSS).is_none()
                && west.region().knowledge(ACROSS) != Knowledge::Held
                && east.region().chunk(ACROSS).is_some()
                && east.region().knowledge(ACROSS) == Knowledge::Held
        },
    );
}
