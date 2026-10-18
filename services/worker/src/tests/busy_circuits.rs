//! What a busy circuit costs the store (step R8 of `docs/adr/0026-redstone.md`, and the
//! risk "a busy circuit makes commits and copies' updates grow" of
//! `docs/groundwork/r-plan.md`): a clock of four repeaters, alone and driving a line of
//! 64 dust, run for twenty seconds of game time, with the commits it makes counted and
//! encoded as the log writes them.
//!
//! The circuits are stored in their chunks before the region opens, as a player would
//! have left them, with the clock's first pulse under way as two of its repeaters'
//! scheduled ticks; nothing a player does is needed to keep it going. Run with:
//!
//! ```text
//! cargo test --release -p clustine-worker --lib -- --ignored --nocapture busy_circuit
//! ```

use clustine_data::{BlockState, blocks};
use clustine_format::LogRecord;
use clustine_world::{InChunk, ScheduledTick, Ticked};

use super::*;

/// The chunk of the clock and of the line's start, in the western stripe, away from
/// the home chunk and from the line between the stripes.
const K: ChunkPos = ChunkPos::new(-3, 0);

/// The clock's north-western corner, and the height everything stands at: on the
/// classic flat world's grass.
const X0: i32 = -46;
const Z0: i32 = 0;
const Y: i32 = -60;

/// The length of the line in dust, and how far a signal runs in dust before a repeater
/// gives it its full strength again.
const DUST: i32 = 64;
const SEGMENT: i32 = 15;

/// Twenty seconds of game time.
const TICKS: u64 = 400;

fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a state"))
}

/// A repeater of the shortest delay whose input is from `facing`.
fn repeater(facing: &str, powered: bool) -> BlockState {
    state(&format!(
        "minecraft:repeater[delay=1,facing={facing},locked=false,powered={powered}]"
    ))
}

/// Wire joined to the sides named, at `power`.
fn wire(sides: &[&str], power: u8) -> BlockState {
    let side = |name: &str| {
        if sides.contains(&name) {
            "side"
        } else {
            "none"
        }
    };
    state(&format!(
        "minecraft:redstone_wire[east={},north={},power={power},south={},west={}]",
        side("east"),
        side("north"),
        side("south"),
        side("west"),
    ))
}

/// The blocks of the circuit, at their positions in the world. The clock is a ring of
/// three by three, wire at the corners and a repeater on each side, the signal going
/// clockwise seen from above: east along the north side, south along the east side,
/// and so on. A repeater's `facing` names the side its input comes from. The ring's
/// north repeater is on and the wire after it at full strength, so that the pulse is
/// under way. Where `line` is asked for, a repeater at the south-eastern corner takes
/// the signal south into a line of [`DUST`] dust, a repeater after every [`SEGMENT`].
fn circuit(line: bool) -> Vec<(BlockPos, BlockState)> {
    let at = |x: i32, z: i32| BlockPos::new(X0 + x, Y, Z0 + z);
    let mut blocks = vec![
        (at(0, 0), wire(&["east", "south"], 0)),
        (at(1, 0), repeater("west", true)),
        (at(2, 0), wire(&["west", "south"], 15)),
        (at(2, 1), repeater("north", false)),
        (at(1, 2), repeater("east", false)),
        (at(0, 2), wire(&["north", "east"], 0)),
        (at(0, 1), repeater("south", false)),
    ];
    if !line {
        blocks.push((at(2, 2), wire(&["north", "west"], 0)));
        return blocks;
    }
    blocks.push((at(2, 2), wire(&["north", "west", "south"], 0)));
    let mut z = 3;
    let mut laid = 0;
    while laid < DUST {
        blocks.push((at(2, z), repeater("north", false)));
        z += 1;
        for _ in 0..SEGMENT.min(DUST - laid) {
            blocks.push((at(2, z), wire(&["north", "south"], 0)));
            (z, laid) = (z + 1, laid + 1);
        }
    }
    blocks
}

/// The scheduled ticks that set the pulse going: the east repeater turns on in the
/// first tick, its input being at full strength, and the north one turns off in the
/// third, its input being off; from then each repeater's change schedules the next.
fn first_pulse() -> Vec<(BlockPos, i32)> {
    vec![
        (BlockPos::new(X0 + 2, Y, Z0 + 1), 1),
        (BlockPos::new(X0 + 1, Y, Z0), 3),
    ]
}

fn in_chunk(position: BlockPos) -> InChunk {
    InChunk::new(
        position.x.rem_euclid(16) as usize,
        position.y,
        position.z.rem_euclid(16) as usize,
    )
}

/// The chunk at `position` as the store hands it out through `handle`, made by the
/// generator if it was never stored.
fn made(handle: &StoreHandle, position: ChunkPos) -> Chunk {
    handle.request(StoreRequest::Load { position });
    for _ in 0..20_000 {
        match handle.try_reply() {
            Some(StoreReply::Loaded { chunk, .. }) => return chunk,
            Some(StoreReply::Generating { .. }) => {}
            Some(other) => panic!("the store answered a load with {other:?}"),
            None => thread::sleep(Duration::from_millis(1)),
        }
    }
    panic!("the store did not answer the load");
}

/// The world of [`Divided::stripes`] with the circuit stored in its chunks.
fn world_with(line: bool) -> Divided {
    let world = Divided::stripes();
    let (handle, _) = world.open(RegionId(0), 1);
    let blocks = circuit(line);
    let mut chunks: BTreeMap<ChunkPos, Chunk> = BTreeMap::new();
    for (position, _) in &blocks {
        let chunk = position.chunk();
        if let std::collections::btree_map::Entry::Vacant(entry) = chunks.entry(chunk) {
            entry.insert(made(&handle, chunk));
        }
    }
    for (position, block) in &blocks {
        let chunk = chunks.get_mut(&position.chunk()).expect("loaded above");
        let inside = in_chunk(*position);
        let (x, z) = (usize::from(inside.x), usize::from(inside.z));
        chunk.set(x, position.y, z, *block);
    }
    let id = blocks::REPEATER.block_id().expect("a repeater is a block");
    let id = u16::try_from(id).expect("a block id fits in 16 bits");
    let ticks: Vec<ScheduledTick> = first_pulse()
        .into_iter()
        .map(|(position, delay)| ScheduledTick {
            at: in_chunk(position),
            of: Ticked::Block(id),
            delay,
            priority: 0,
        })
        .collect();
    chunks
        .get_mut(&K)
        .expect("the clock is in its chunk")
        .set_ticks(ticks);
    for (position, chunk) in chunks {
        handle.request(StoreRequest::Save {
            position,
            tick: 0,
            chunk,
            parts: clustine_rpc::Parts::ALL,
        });
    }
    handle.flush();
    world
}

/// What a run of a circuit committed.
struct Measured {
    ticks: u64,
    commits: usize,
    changes: usize,
    /// The changes but those of a block changed again later in the same commit.
    last: usize,
    with_ticks: usize,
    with_events: usize,
    bytes: usize,
}

/// The commits of `requests` as the store logs them, each with its frame.
fn logged_bytes(request: &StoreRequest) -> usize {
    let StoreRequest::Commit {
        tick,
        changes,
        state,
        stays,
        ticks,
        marks_taken,
        seams,
        effects,
        taken,
        events,
        ..
    } = request
    else {
        panic!("{request:?} is not a commit");
    };
    // Away from the line between the stripes, the circuit has nothing to do with the
    // other region.
    assert!(
        stays.is_empty() && effects.is_empty(),
        "no player and no border"
    );
    LogRecord::Commit {
        region: 0,
        tick: *tick,
        epoch: 1,
        changes: changes.clone(),
        state: state.clone(),
        stays: Vec::new(),
        ticks: ticks.clone(),
        marks_taken: marks_taken.clone(),
        seams: seams.clone(),
        effects: Vec::new(),
        taken: taken.clone(),
        events: events.clone(),
        entity_changes: Vec::new(),
    }
    .encode()
    .len()
}

/// How many blocks a commit changes, each counted once however often it changed.
fn last_changes(request: &StoreRequest) -> usize {
    let StoreRequest::Commit { changes, .. } = request else {
        panic!("{request:?} is not a commit");
    };
    let blocks: BTreeSet<BlockPos> = changes.iter().map(|change| change.position()).collect();
    blocks.len()
}

/// Runs the circuit for [`TICKS`] once all its chunks tick, and says what it committed
/// in them.
async fn run(line: bool) -> Measured {
    let world = world_with(line);
    let (runner, gate) = gated_as(&world.store, world.hello(RegionId(0), 2), settings(0));
    let mut runner = runner.with_id(RegionId(0));
    runner.checkpoint_interval = 1_000_000;
    let (edge, worker_end) = in_process(256);
    runner.links().attach(worker_end);
    let around: Vec<ChunkPos> = (-4..=-2)
        .flat_map(|x| (-1..=6).map(move |z| ChunkPos::new(x, z)))
        .collect();
    edge.send(asking_for(around.clone())).await.unwrap();
    step_until(&mut runner, |runner| {
        around
            .iter()
            .all(|position| runner.region().chunk(*position).is_some())
    });
    let started = runner.region().tick_number();
    gate.committed.lock().unwrap().clear();
    for _ in 0..TICKS {
        step(&mut runner);
    }
    let ended = runner.region().tick_number();
    assert_eq!(ended - started, TICKS);
    let committed = gate.committed.lock().unwrap().clone();
    let of = |request: &StoreRequest| match request {
        StoreRequest::Commit {
            changes,
            ticks,
            events,
            ..
        } => (changes.len(), !ticks.is_empty(), !events.is_empty()),
        other => panic!("{other:?} is not a commit"),
    };
    Measured {
        ticks: TICKS,
        commits: committed.len(),
        changes: committed.iter().map(|request| of(request).0).sum(),
        last: committed.iter().map(last_changes).sum(),
        with_ticks: committed.iter().filter(|request| of(request).1).count(),
        with_events: committed.iter().filter(|request| of(request).2).count(),
        bytes: committed.iter().map(logged_bytes).sum(),
    }
}

/// Prints, for the clock alone and for the clock driving the line, the commits a second
/// of game time, the block changes in them, and the bytes the log is written.
#[tokio::test]
#[ignore = "a measurement, which prints what it found"]
async fn what_a_busy_circuit_commits() {
    for (name, line) in [
        ("a clock of four repeaters", false),
        ("the clock and 64 dust", true),
    ] {
        let measured = run(line).await;
        let seconds = measured.ticks as f64 / 20.0;
        let per_second = |count: usize| count as f64 / seconds;
        println!(
            "{name}: {:.1} commits a second ({} with ticks, none with events: {}), \
             {:.1} block changes a second ({:.1} of blocks not changed again in the \
             commit), {:.0} bytes a second, {:.0} bytes a commit",
            per_second(measured.commits),
            measured.with_ticks,
            measured.with_events == 0,
            per_second(measured.changes),
            per_second(measured.last),
            per_second(measured.bytes),
            measured.bytes as f64 / measured.commits.max(1) as f64,
        );
        assert!(measured.changes > 0, "{name} runs");
    }
}
