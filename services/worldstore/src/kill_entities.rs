//! The store killed at every point of commits that change block entities' data among
//! their blocks and the block events a chunk keeps (`docs/adr/0026-redstone.md`,
//! sections 4 and 6): what a crash leaves is opened by a new store, and the chunk it
//! restores must be the restored history's changes applied in their order, whatever of
//! them the stored chunk had already, with the events the last of them left.

use std::path::Path;

use clustine_data::blocks;
use clustine_world::{BlockPos, Change, Chunk, ChunkPos, InChunk, KeptEvent};

use super::*;
use crate::disk::{Fault, MemoryDisk, Survival};
use crate::tests::{division, generator, hello};

const ORIGIN: ChunkPos = ChunkPos::new(0, 0);

/// A chest of its own each tick.
fn chest(tick: u64) -> BlockPos {
    BlockPos::new(tick as i32, 100, 3)
}

/// The block whose kind changes across the checkpoint: a chest at tick 2, a barrel at
/// tick 5, and at tick 6 a barrel set over the barrel, which the game makes a fresh
/// block entity for.
const CHANGING: BlockPos = BlockPos::new(9, 100, 9);

const TICKS: u64 = 6;
/// The checkpoint is of this tick; the chunk is saved as of [`SAVED_AS_OF`], newer, so
/// that the replay runs over a chunk that has some of it already.
const CHECKPOINT: u64 = 3;
const SAVED_AS_OF: u64 = 5;

fn data(what: &str, tick: u64) -> Vec<u8> {
    format!("{what} {tick}").into_bytes()
}

/// What the commit of `tick` changes, in its order.
fn changes(tick: u64) -> Vec<Change> {
    let mut changes = vec![
        Change::Block(chest(tick), blocks::CHEST),
        Change::BlockEntity(chest(tick), data("chest", tick)),
    ];
    match tick {
        2 => changes.extend([
            Change::Block(CHANGING, blocks::CHEST),
            Change::BlockEntity(CHANGING, data("changing chest", tick)),
        ]),
        5 => changes.extend([
            Change::Block(CHANGING, blocks::BARREL),
            Change::BlockEntity(CHANGING, data("changing barrel", tick)),
        ]),
        6 => changes.extend([
            Change::Block(CHANGING, blocks::BARREL),
            Change::BlockEntity(CHANGING, Vec::new()),
            Change::BlockEntity(CHANGING, data("fresh barrel", tick)),
        ]),
        _ => {}
    }
    changes
}

/// The block events the chunk keeps after the commit of `tick`, where that changed
/// them: one queued at tick 2, a second behind it at tick 5, and none from tick 6 on.
fn events(tick: u64) -> Option<Vec<KeptEvent>> {
    let event = |data| KeptEvent {
        at: InChunk::new(2, 100, 3),
        block: 1200,
        kind: 0,
        data,
    };
    match tick {
        2 => Some(vec![event(1)]),
        5 => Some(vec![event(1), event(2)]),
        6 => Some(Vec::new()),
        _ => None,
    }
}

/// The chunk at the origin with the changes of `ticks` applied in order.
fn chunk_after(ticks: impl IntoIterator<Item = u64>) -> Chunk {
    let mut chunk = generator().generate(ORIGIN);
    for tick in ticks {
        for change in changes(tick) {
            chunk.apply(&change).expect("small data");
        }
        if let Some(events) = events(tick) {
            chunk.set_events(events);
        }
    }
    chunk
}

fn store_on(disk: Arc<MemoryDisk>) -> Result<Store, StoreError> {
    let root = Path::new("/world");
    let chunks = FileChunks::new(disk.clone(), root);
    start(disk, root, Box::new(chunks), generator(), &division())
}

fn delta(tick: u64) -> Vec<u8> {
    format!("delta {tick}").into_bytes()
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

/// Commits every tick, saves the chunk as of [`SAVED_AS_OF`] once that is committed,
/// and asks for a checkpoint of [`CHECKPOINT`]; returns the commits confirmed.
fn scenario(store: &Store) -> Vec<u64> {
    let Some((handle, _)) = (0..3).find_map(|_| store.open_region(hello(1, 1)).ok()) else {
        return Vec::new();
    };
    for tick in 1..=TICKS {
        handle.request(StoreRequest::Commit {
            tick,
            changes: changes(tick),
            state: delta(tick),
            stays: Vec::new(),
            ticks: Vec::new(),
            marks_taken: Vec::new(),
            seams: Vec::new(),
            effects: Vec::new(),
            taken: Vec::new(),
            events: events(tick)
                .map(|events| vec![(ORIGIN, events)])
                .unwrap_or_default(),
            entity_changes: Vec::new(),
            arrivals: Vec::new(),
            arrivals_taken: Vec::new(),
        });
        if tick == SAVED_AS_OF {
            handle.request(StoreRequest::Save {
                position: ORIGIN,
                tick,
                chunk: chunk_after(1..=tick),
                parts: clustine_rpc::Parts::ALL,
            });
            handle.request(StoreRequest::Checkpoint {
                tick: CHECKPOINT,
                state: b"state 3".to_vec(),
            });
        }
    }
    settle(&handle).0
}

fn the_restored_chunk_is_the_history_applied_in_order(fault: Fault, survival: Survival) {
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
    // The ticks the region is restored to: a checkpoint, and commits after it, one a
    // tick and none missing.
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
            "{case}: commit {tick} was confirmed, restored to {last}"
        );
    }
    owner.request(StoreRequest::Load { position: ORIGIN });
    let (_, replies) = settle(&owner);
    assert_eq!(
        replies,
        [StoreReply::Loaded {
            position: ORIGIN,
            chunk: chunk_after(1..=last)
        }],
        "{case}: restored to {last}"
    );
}

/// How many changes and syncs the scenario makes when nothing goes wrong, with some to
/// spare for the ways the threads can interleave.
fn operations() -> u64 {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(Arc::clone(&disk)).unwrap();
    assert_eq!(scenario(&store), (1..=TICKS).collect::<Vec<_>>());
    disk.operations() + 10
}

const SURVIVALS: [Survival; 4] = [
    Survival::Nothing,
    Survival::Torn,
    Survival::Everything,
    Survival::Untruncated,
];

#[test]
fn block_entities_are_restored_in_their_place_among_the_blocks_without_a_fault() {
    for survival in SURVIVALS {
        the_restored_chunk_is_the_history_applied_in_order(Fault::Fail(u64::MAX), survival);
    }
}

#[test]
fn block_entities_are_restored_in_their_place_after_a_stop_at_any_point() {
    for n in 1..=operations() {
        for survival in SURVIVALS {
            the_restored_chunk_is_the_history_applied_in_order(Fault::Stop(n), survival);
        }
    }
}

#[test]
fn block_entities_are_restored_in_their_place_after_one_failure_at_any_point() {
    for n in 1..=operations() {
        for survival in SURVIVALS {
            the_restored_chunk_is_the_history_applied_in_order(Fault::Fail(n), survival);
        }
    }
}
