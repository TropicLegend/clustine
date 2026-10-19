//! The store killed at every point of commits and saves of entities
//! (`docs/adr/0028-entities.md`, section 3: a crash loses no tick of any entity): a
//! scenario of commits with entity changes, saves of both parts and of the entities
//! alone, the removal of a chunk's entities' file and checkpoints, run on a simulated
//! disk that fails, or stops for good, at the `n`th change or sync, for every `n`. What
//! a crash leaves is then opened by a new store, and the chunk it loads is held to the
//! history the region is restored with: every confirmed tick's entity changes, and
//! nothing beyond.

use std::path::Path;

use clustine_data::blocks;
use clustine_rpc::Parts;
use clustine_world::{BlockPos, Change, Chunk, ChunkPos, Entity, EntityChange, EntityField};

use super::*;
use crate::disk::{Fault, MemoryDisk, Survival};
use crate::entities_tests::item;
use crate::regions::SURVIVALS;
use crate::tests::{division, generator, hello};

const EAST: u32 = 1;
const A: ChunkPos = ChunkPos::new(0, 0);

/// The block the first tick sets.
const SAND: BlockPos = BlockPos::new(3, -60, 4);

/// The last tick of the scenario.
const LAST: u64 = 6;

fn first() -> Entity {
    item(1, 3.5, 4.5)
}

fn second() -> Entity {
    item(2, 9.5, 12.5)
}

fn third() -> Entity {
    item(3, 15.75, 0.25)
}

/// What each tick of the scenario commits: a block and an entity in a chunk as
/// generated, changes of fields, a whole entity, entities gone until none is left,
/// and one more after that.
fn entity_changes(tick: u64) -> Vec<EntityChange> {
    let moved = EntityField::Position([4.5, -59.0, 5.5]);
    match tick {
        1 => vec![EntityChange::Whole(first())],
        2 => vec![
            EntityChange::Set(first().key, vec![moved]),
            EntityChange::Whole(second()),
        ],
        3 => vec![EntityChange::Gone(first().key)],
        4 => vec![EntityChange::Set(
            second().key,
            vec![EntityField::Lived(40)],
        )],
        5 => vec![EntityChange::Gone(second().key)],
        6 => vec![EntityChange::Whole(third())],
        _ => unreachable!(),
    }
}

/// The chunk `A` as the commits up to `tick` leave it.
fn chunk_after(tick: u64) -> Chunk {
    let mut chunk = generator().generate(A);
    for done in 1..=tick {
        if done == 1 {
            let (x, z) = SAND.in_chunk();
            chunk.set(x, SAND.y, z, blocks::SAND);
        }
        for change in entity_changes(done) {
            chunk.apply_entity(&change);
        }
    }
    chunk
}

fn delta(tick: u64) -> Vec<u8> {
    format!("delta {tick}").into_bytes()
}

fn whole(tick: u64) -> Vec<u8> {
    format!("state {tick}").into_bytes()
}

fn store_on(disk: Arc<MemoryDisk>) -> Result<Store, StoreError> {
    let root = Path::new("/world");
    let chunks = FileChunks::new(disk.clone(), root);
    start(disk, root, Box::new(chunks), generator(), &division())
}

fn commit(handle: &StoreHandle, tick: u64) {
    let changes = if tick == 1 {
        vec![Change::Block(SAND, blocks::SAND)]
    } else {
        Vec::new()
    };
    handle.request(StoreRequest::Commit {
        tick,
        changes,
        state: delta(tick),
        stays: Vec::new(),
        ticks: Vec::new(),
        marks_taken: Vec::new(),
        seams: Vec::new(),
        effects: Vec::new(),
        taken: Vec::new(),
        events: Vec::new(),
        entity_changes: entity_changes(tick)
            .into_iter()
            .map(|change| (A, change))
            .collect(),
        arrivals: Vec::new(),
        arrivals_taken: Vec::new(),
        clock: Default::default(),
    });
}

fn save(handle: &StoreHandle, tick: u64, parts: Parts) {
    handle.request(StoreRequest::Save {
        position: A,
        tick,
        chunk: chunk_after(tick),
        parts,
    });
}

/// The commits answered before the flush is.
fn settle(handle: &StoreHandle) -> Vec<u64> {
    handle.request(StoreRequest::Flush);
    let mut committed = Vec::new();
    while let Ok(reply) = handle.replies.recv() {
        match reply {
            StoreReply::Committed { tick } => committed.push(tick),
            StoreReply::Flushed => break,
            _ => {}
        }
    }
    committed
}

/// The scenario, as a worker would ask it: the first save of the chunk writes both its
/// files in one round, a later one its entities alone, and the one after the last
/// entity has gone removes their file. Returns the commits confirmed.
fn scenario(store: &Store) -> Vec<u64> {
    let Some((handle, _)) = (0..3).find_map(|_| store.open_region(hello(EAST, 1)).ok()) else {
        return Vec::new();
    };
    commit(&handle, 1);
    save(&handle, 1, Parts::ALL);
    commit(&handle, 2);
    commit(&handle, 3);
    save(&handle, 3, Parts::ENTITIES);
    handle.request(StoreRequest::Checkpoint {
        tick: 3,
        state: whole(3),
    });
    commit(&handle, 4);
    commit(&handle, 5);
    save(&handle, 5, Parts::ENTITIES);
    handle.request(StoreRequest::Checkpoint {
        tick: 5,
        state: whole(5),
    });
    commit(&handle, LAST);
    settle(&handle)
}

/// The last tick of the history a region is restored with, which is one: a state of a
/// checkpoint asked for, and after it a commit for each tick.
fn restored_through(restored: &Restored, case: &str) -> u64 {
    let mut next = 1;
    if let Some(state) = &restored.state {
        assert_eq!(state.state, whole(state.tick), "{case}");
        next = state.tick + 1;
    }
    for restored_delta in &restored.deltas {
        assert_eq!(restored_delta.tick, next, "{case}: a tick missing or twice");
        assert_eq!(restored_delta.state, delta(next), "{case}");
        next += 1;
    }
    next - 1
}

fn every_confirmed_entity_change_survives(fault: Fault, survival: Survival) {
    let disk = Arc::new(MemoryDisk::failing(fault));
    let confirmed = match store_on(Arc::clone(&disk)) {
        Ok(store) => scenario(&store),
        Err(_) => Vec::new(),
    };
    let case = format!("{fault:?}, {survival:?}");
    let crashed = Arc::new(disk.crashed(survival));
    let store = store_on(crashed).unwrap_or_else(|error| panic!("{case}: {error}"));
    let (handle, restored) = store
        .open_region(hello(EAST, 2))
        .unwrap_or_else(|error| panic!("{case}: {error}"));
    let through = restored_through(&restored, &case);
    assert!(
        confirmed.iter().all(|tick| *tick <= through),
        "{case}: {confirmed:?} were confirmed, the region is restored through {through}"
    );
    handle.request(StoreRequest::Load { position: A });
    let chunk = loop {
        match handle.replies.recv().unwrap() {
            StoreReply::Loaded { chunk, .. } => break chunk,
            StoreReply::Generating { .. } => {}
            other => panic!("{case}: {other:?}"),
        }
    };
    assert_eq!(chunk, chunk_after(through), "{case}: through {through}");
}

/// How many changes and syncs the scenario makes when nothing goes wrong, with some to
/// spare for the ways the threads can interleave.
fn operations() -> u64 {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(Arc::clone(&disk)).unwrap();
    assert_eq!(scenario(&store), (1..=LAST).collect::<Vec<_>>());
    disk.operations() + 10
}

#[test]
fn every_entity_change_survives_without_a_fault() {
    for survival in SURVIVALS {
        every_confirmed_entity_change_survives(Fault::Fail(u64::MAX), survival);
    }
}

#[test]
fn a_store_that_stops_at_any_point_of_commits_and_saves_of_entities_keeps_every_confirmed_change() {
    for n in 1..=operations() {
        for survival in SURVIVALS {
            every_confirmed_entity_change_survives(Fault::Stop(n), survival);
        }
    }
}

#[test]
fn a_store_that_fails_once_at_any_point_of_commits_and_saves_of_entities_keeps_every_confirmed_change()
 {
    for n in 1..=operations() {
        for survival in SURVIVALS {
            every_confirmed_entity_change_survives(Fault::Fail(n), survival);
        }
    }
}
