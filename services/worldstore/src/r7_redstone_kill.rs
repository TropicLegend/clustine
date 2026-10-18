//! Step R7 of `docs/adr/0026-redstone.md`, sections 4, 5, 6 and 12, written from the
//! record by someone who did not write the store: the history of a piston and of
//! comparators over two chunks, committed tick by tick as a region commits it (blocks
//! and block entities' data in the order the tick made them, kept block events of the
//! chunks whose events changed, whole), the store killed at every point of it, and each
//! chunk restored equal to the restored history applied in order over what was stored.
//!
//! What the history has that the record calls for: a comparator's output written before
//! the comparator's own change of state in one commit, as `refreshOutputState` does; a
//! comparator replaced by one of its kind, which keeps its block entity, and removed
//! and placed again, which makes a fresh one; a moving piston made and given data in
//! one commit, its data changed half way, and replaced in a commit after a change of
//! its data, which drops its block entity; kept events that grow, move between chunks
//! and are emptied; a chunk saved as of a tick between the checkpoint and the last
//! commit, and one saved as of the checkpoint.

use std::path::Path;

use clustine_data::{BlockState, blocks};
use clustine_world::{BlockPos, Change, Chunk, ChunkPos, InChunk, KeptEvent};

use super::*;
use crate::disk::{Fault, MemoryDisk, Survival};
use crate::tests::{division, generator, hello};

const ORIGIN: ChunkPos = ChunkPos::new(0, 0);
const EAST: ChunkPos = ChunkPos::new(1, 0);

const COMPARATOR: BlockPos = BlockPos::new(5, -60, 5);
const BASE: BlockPos = BlockPos::new(2, -60, 8);
const HEAD: BlockPos = BlockPos::new(3, -60, 8);
const AHEAD: BlockPos = BlockPos::new(4, -60, 8);
const EAST_COMPARATOR: BlockPos = BlockPos::new(20, -60, 5);
const EAST_MOVING: BlockPos = BlockPos::new(21, -60, 6);

const TICKS: u64 = 8;
const CHECKPOINT: u64 = 4;
/// The origin's chunk is saved as of this tick, after the checkpoint, so that the
/// replay runs over a chunk that has part of what it replays; the eastern chunk as of
/// the checkpoint.
const ORIGIN_SAVED_AS_OF: u64 = 6;
const EAST_SAVED_AS_OF: u64 = CHECKPOINT;

fn state(text: &str) -> BlockState {
    BlockState::parse(text).expect("a state")
}

fn comparator(powered: bool, facing: &str) -> BlockState {
    state(&format!(
        "minecraft:comparator[facing={facing},mode=compare,powered={powered}]"
    ))
}

fn moving() -> BlockState {
    state("minecraft:moving_piston[facing=east,type=normal]")
}

/// A name of an NBT tag, as the network writes one: its length and its bytes.
fn name(text: &str) -> Vec<u8> {
    let mut bytes = (text.len() as u16).to_be_bytes().to_vec();
    bytes.extend_from_slice(text.as_bytes());
    bytes
}

/// A comparator's data as the game writes it: `{components:{}, OutputSignal:n}`.
fn output(signal: i32) -> Vec<u8> {
    let mut bytes = vec![10];
    bytes.push(10);
    bytes.extend(name("components"));
    bytes.push(0);
    bytes.push(3);
    bytes.extend(name("OutputSignal"));
    bytes.extend(signal.to_be_bytes());
    bytes.push(0);
    bytes
}

/// A moving piston's data, of the shape the game saves: what moves, its direction, its
/// progress and whether it extends and is the piston's own.
fn moving_data(what: &str, progress: f32, extending: bool, source: bool) -> Vec<u8> {
    let mut bytes = vec![10];
    bytes.push(10);
    bytes.extend(name("blockState"));
    bytes.push(8);
    bytes.extend(name("Name"));
    bytes.extend(name(what));
    bytes.push(0);
    bytes.push(3);
    bytes.extend(name("facing"));
    bytes.extend(5i32.to_be_bytes());
    bytes.push(5);
    bytes.extend(name("progress"));
    bytes.extend(progress.to_be_bytes());
    bytes.push(1);
    bytes.extend(name("extending"));
    bytes.push(u8::from(extending));
    bytes.push(1);
    bytes.extend(name("source"));
    bytes.push(u8::from(source));
    bytes.push(0);
    bytes
}

/// What the commit of `tick` changes, in its order, over both chunks.
fn changes(tick: u64) -> Vec<Change> {
    use Change::{Block, BlockEntity};
    match tick {
        // A comparator placed, with the empty block entity the game makes for it.
        1 => vec![
            Block(COMPARATOR, comparator(false, "west")),
            Block(EAST_COMPARATOR, comparator(false, "north")),
            BlockEntity(EAST_COMPARATOR, output(0)),
        ],
        // Its output, then its state, as `refreshOutputState` sets them.
        2 => vec![
            BlockEntity(COMPARATOR, output(3)),
            Block(COMPARATOR, comparator(true, "west")),
            BlockEntity(EAST_COMPARATOR, output(9)),
            Block(EAST_COMPARATOR, comparator(true, "north")),
        ],
        // A piston extends: its head and the block it pushes become moving pistons,
        // made and ticked once in the tick.
        3 => vec![
            Block(BASE, state("minecraft:piston[extended=true,facing=east]")),
            Block(HEAD, moving()),
            BlockEntity(HEAD, moving_data("minecraft:piston_head", 0.0, true, true)),
            Block(AHEAD, moving()),
            BlockEntity(AHEAD, moving_data("minecraft:stone", 0.0, true, false)),
            Block(EAST_MOVING, moving()),
            BlockEntity(
                EAST_MOVING,
                moving_data("minecraft:stone", 0.0, true, false),
            ),
        ],
        // Half way.
        4 => vec![
            BlockEntity(HEAD, moving_data("minecraft:piston_head", 0.5, true, true)),
            BlockEntity(AHEAD, moving_data("minecraft:stone", 0.5, true, false)),
            BlockEntity(COMPARATOR, output(7)),
            BlockEntity(
                EAST_MOVING,
                moving_data("minecraft:stone", 0.5, true, false),
            ),
        ],
        // The move finishes: the data changes once more and the block that takes the
        // moving piston's place drops its block entity.
        5 => vec![
            BlockEntity(AHEAD, moving_data("minecraft:stone", 1.0, true, false)),
            Block(
                HEAD,
                state("minecraft:piston_head[facing=east,short=false,type=normal]"),
            ),
            Block(AHEAD, blocks::STONE),
            BlockEntity(
                EAST_MOVING,
                moving_data("minecraft:stone", 1.0, true, false),
            ),
            Block(EAST_MOVING, blocks::STONE),
        ],
        // A comparator broken and placed again in one tick: a fresh block entity, and
        // one of the same kind set over the eastern one, which keeps its data.
        6 => vec![
            Block(COMPARATOR, blocks::AIR),
            Block(COMPARATOR, comparator(false, "south")),
            Block(EAST_COMPARATOR, comparator(false, "north")),
        ],
        // A new output for each, the eastern one written fresh where the game makes a
        // fresh block entity for a block of its kind.
        7 => vec![
            BlockEntity(COMPARATOR, output(12)),
            Block(COMPARATOR, comparator(true, "south")),
            BlockEntity(EAST_COMPARATOR, Vec::new()),
            BlockEntity(EAST_COMPARATOR, output(1)),
        ],
        // The piston retracts: its base becomes a moving piston of its own, the head
        // goes.
        8 => vec![
            Block(BASE, moving()),
            BlockEntity(BASE, moving_data("minecraft:piston", 0.0, false, true)),
            Block(HEAD, blocks::AIR),
        ],
        _ => Vec::new(),
    }
}

fn event(x: usize, z: usize, block: u16, kind: i32, data: i32) -> KeptEvent {
    KeptEvent {
        at: InChunk::new(x, -60, z),
        block,
        kind,
        data,
    }
}

/// The kept events of the chunks whose events changed in `tick`, whole.
fn events(tick: u64) -> Vec<(ChunkPos, Vec<KeptEvent>)> {
    let note = event(6, 3, 108, 0, 0);
    let bell = event(7, 3, 900, 1, 2);
    let piston = event(2, 8, 138, 1, 5);
    match tick {
        2 => vec![(EAST, vec![note])],
        4 => vec![(EAST, vec![note, bell])],
        5 => vec![(ORIGIN, vec![piston]), (EAST, vec![bell])],
        7 => vec![(ORIGIN, vec![piston, note]), (EAST, Vec::new())],
        8 => vec![(ORIGIN, vec![note])],
        _ => Vec::new(),
    }
}

/// The chunk at `position` with the changes of `ticks` in it applied in order, and the
/// events the last of them that changed its events left.
fn chunk_after(position: ChunkPos, ticks: impl IntoIterator<Item = u64>) -> Chunk {
    let mut chunk = generator().generate(position);
    for tick in ticks {
        for change in changes(tick) {
            if change.position().chunk() == position {
                chunk.apply(&change).expect("data a chunk takes");
            }
        }
        for (at, events) in events(tick) {
            if at == position {
                chunk.set_events(events);
            }
        }
    }
    chunk
}

fn store_on(disk: Arc<MemoryDisk>) -> Result<Store, StoreError> {
    let root = Path::new("/world");
    let chunks = FileChunks::new(disk.clone(), root);
    start(disk, root, Box::new(chunks), generator(), &division())
}

/// Collects the answers until a flush is answered or the handle is lost; returns the
/// commits confirmed and the other answers.
fn settle(handle: &StoreHandle) -> (Vec<u64>, Vec<StoreReply>) {
    handle.request(StoreRequest::Flush);
    let mut committed = Vec::new();
    let mut others = Vec::new();
    while let Ok(reply) = handle.replies.recv() {
        match reply {
            StoreReply::Committed { tick } => committed.push(tick),
            StoreReply::Flushed => break,
            StoreReply::Generating { .. } => {}
            other => others.push(other),
        }
    }
    (committed, others)
}

/// Commits every tick; after the commit of [`ORIGIN_SAVED_AS_OF`], saves both chunks
/// and asks for a checkpoint of [`CHECKPOINT`]. Returns the commits confirmed.
fn scenario(store: &Store) -> Vec<u64> {
    let Some((handle, _)) = (0..3).find_map(|_| store.open_region(hello(1, 1)).ok()) else {
        return Vec::new();
    };
    for tick in 1..=TICKS {
        handle.request(StoreRequest::Commit {
            tick,
            changes: changes(tick),
            state: format!("delta {tick}").into_bytes(),
            stays: Vec::new(),
            ticks: Vec::new(),
            marks_taken: Vec::new(),
            seams: Vec::new(),
            effects: Vec::new(),
            taken: Vec::new(),
            events: events(tick),
            entity_changes: Vec::new(),
            arrivals: Vec::new(),
            arrivals_taken: Vec::new(),
        });
        if tick == ORIGIN_SAVED_AS_OF {
            handle.request(StoreRequest::Save {
                position: ORIGIN,
                tick,
                chunk: chunk_after(ORIGIN, 1..=tick),
                parts: clustine_rpc::Parts::ALL,
            });
            handle.request(StoreRequest::Save {
                position: EAST,
                tick: EAST_SAVED_AS_OF,
                chunk: chunk_after(EAST, 1..=EAST_SAVED_AS_OF),
                parts: clustine_rpc::Parts::ALL,
            });
            handle.request(StoreRequest::Checkpoint {
                tick: CHECKPOINT,
                state: b"state 4".to_vec(),
            });
        }
    }
    settle(&handle).0
}

fn each_chunk_is_its_restored_history_applied_in_order(fault: Fault, survival: Survival) {
    let disk = Arc::new(MemoryDisk::failing(fault));
    let confirmed = match store_on(Arc::clone(&disk)) {
        Ok(store) => scenario(&store),
        Err(_) => Vec::new(),
    };
    let case = format!("{fault:?}, {survival:?}");
    let crashed = Arc::new(disk.crashed(survival));
    let store = store_on(crashed).unwrap_or_else(|error| panic!("{case}: {error}"));
    let (owner, restored) = store
        .open_region(hello(1, 2))
        .unwrap_or_else(|error| panic!("{case}: {error}"));
    let from = restored.state.as_ref().map_or(0, |state| state.tick);
    let ticks: Vec<u64> = restored.deltas.iter().map(|delta| delta.tick).collect();
    assert!(
        ticks
            .iter()
            .copied()
            .eq(from + 1..=from + ticks.len() as u64),
        "{case}: from {from}, {ticks:?}"
    );
    let last = ticks.last().copied().unwrap_or(from);
    for tick in &confirmed {
        assert!(
            *tick <= last,
            "{case}: {tick} confirmed, restored to {last}"
        );
    }
    for position in [ORIGIN, EAST] {
        owner.request(StoreRequest::Load { position });
        let (_, replies) = settle(&owner);
        assert_eq!(
            replies,
            [StoreReply::Loaded {
                position,
                chunk: chunk_after(position, 1..=last)
            }],
            "{case}: {position:?}, restored to {last}"
        );
    }
}

/// How many changes and syncs the scenario makes when nothing goes wrong, with some to
/// spare for the ways the threads can interleave.
fn operations() -> u64 {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(Arc::clone(&disk)).expect("a store in memory");
    assert_eq!(scenario(&store), (1..=TICKS).collect::<Vec<_>>());
    disk.operations() + 10
}

const SURVIVALS: [Survival; 4] = [
    Survival::Nothing,
    Survival::Torn,
    Survival::Everything,
    Survival::Untruncated,
];

/// The history itself is one the chunk can take: every block entity's data lands on a
/// block of a kind that has one, in its place in the order.
#[test]
fn the_history_applies_in_order_and_ends_as_the_record_says() {
    let origin = chunk_after(ORIGIN, 1..=TICKS);
    let east = chunk_after(EAST, 1..=TICKS);
    let data = |chunk: &Chunk, pos: BlockPos| {
        let (x, z) = pos.in_chunk();
        chunk
            .block_entities()
            .get(&InChunk::new(x, pos.y, z))
            .map(|entity| entity.data.clone())
    };
    assert_eq!(data(&origin, COMPARATOR), Some(output(12)));
    assert_eq!(data(&origin, AHEAD), None, "stone keeps no block entity");
    assert_eq!(
        data(&origin, BASE),
        Some(moving_data("minecraft:piston", 0.0, false, true))
    );
    assert_eq!(data(&east, EAST_COMPARATOR), Some(output(1)));
    assert_eq!(data(&east, EAST_MOVING), None);
    assert_eq!(origin.events().len(), 1);
    assert_eq!(east.events(), []);
    // A comparator set over one of its kind keeps its data: after tick 6 the eastern
    // comparator still has the output of tick 2.
    assert_eq!(
        data(&chunk_after(EAST, 1..=6), EAST_COMPARATOR),
        Some(output(9))
    );
    // One broken and placed again has a fresh block entity.
    assert_eq!(
        data(&chunk_after(ORIGIN, 1..=6), COMPARATOR).as_deref(),
        Some(&[][..])
    );
}

#[test]
fn a_history_of_pistons_and_comparators_is_restored_in_order_without_a_fault() {
    for survival in SURVIVALS {
        each_chunk_is_its_restored_history_applied_in_order(Fault::Fail(u64::MAX), survival);
    }
}

#[test]
fn a_history_of_pistons_and_comparators_is_restored_in_order_after_a_stop_at_any_point() {
    for n in 1..=operations() {
        for survival in SURVIVALS {
            each_chunk_is_its_restored_history_applied_in_order(Fault::Stop(n), survival);
        }
    }
}

#[test]
fn a_history_of_pistons_and_comparators_is_restored_in_order_after_a_failure_at_any_point() {
    for n in 1..=operations() {
        for survival in SURVIVALS {
            each_chunk_is_its_restored_history_applied_in_order(Fault::Fail(n), survival);
        }
    }
}
