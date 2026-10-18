//! Tests of a chunk's entities in the store, written from `docs/adr/0028-entities.md`
//! (sections 2 and 3) and `docs/groundwork/n2-plan.md` (sections 4, 5 and 11) by
//! someone who did not write the code. A region commits an item, moves it, sets the
//! chunk's blocks, removes the item and throws another, and saves the chunk by parts
//! between those commits, as a worker does: blocks and entities at different ticks,
//! the entities alone, the blocks alone. After every step, a store started on what
//! any crash leaves of the disk restores the chunk as the commits so far made it: the
//! item that was removed never comes back, the other is there once, and the file of
//! the entities is there exactly while the chunk has any.

use std::path::Path;
use std::sync::Arc;

use clustine_data::{BlockState, blocks};
use clustine_rpc::Parts;
use clustine_world::{
    BlockPos, Change, Chunk, ChunkPos, Entity, EntityChange, EntityData, EntityField, EntityId,
    ItemStack,
};

use super::*;
use crate::chunks::FileChunks;
use crate::disk::{MemoryDisk, Survival};
use crate::regions::{SURVIVALS, checkpoint, store_on};
use crate::tests::{committed, division, generator, hello, load};

/// The eastern region of [`division`], which holds the chunk these tests use.
const EAST: u32 = 1;
const A: ChunkPos = ChunkPos::new(0, 0);

fn thrown(key: u64) -> Entity {
    Entity {
        key: EntityId(key),
        kind: 71,
        uuid: uuid::Uuid::from_u128(u128::from(key) << 8 | 0x5),
        position: [4.5, -58.68, 6.5],
        motion: [0.1, 0.2, -0.05],
        yaw: 33.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        age: 0,
        data: EntityData::Item {
            stack: ItemStack { item: 1, count: 1 },
            pick_up_delay: 40,
            thrower: Some(uuid::Uuid::from_u128(1)),
            health: 5,
        },
        fall_distance: 0.0,
        fire_ticks: -1,
        riding: Default::default(),
    }
}

const FIRST: u64 = (1 << 32) | 7;
const SECOND: u64 = (1 << 32) | 8;

fn moved() -> Vec<EntityField> {
    vec![
        EntityField::Position([5.25, -60.0, 6.5]),
        EntityField::OnGround(true),
        EntityField::Age(12),
    ]
}

/// What a step of the region does: a commit of `blocks` and `entities` at `tick`, and
/// then a save of the chunk as it is after that commit, of `parts`, if any.
struct Step {
    tick: u64,
    blocks: Vec<(BlockPos, BlockState)>,
    entities: Vec<EntityChange>,
    save: Option<Parts>,
    /// A checkpoint of the region's state after the step, at a tick of its own.
    checkpoint: Option<u64>,
}

fn steps() -> Vec<Step> {
    vec![
        // The item is thrown, and a block placed beside it; both parts saved.
        Step {
            tick: 1,
            blocks: vec![(BlockPos::new(2, -60, 2), blocks::STONE)],
            entities: vec![EntityChange::Whole(thrown(FIRST))],
            save: Some(Parts::ALL),
            checkpoint: None,
        },
        // It moves; the entities alone are saved.
        Step {
            tick: 2,
            blocks: Vec::new(),
            entities: vec![EntityChange::Set(EntityId(FIRST), moved())],
            save: Some(Parts::ENTITIES),
            checkpoint: None,
        },
        // A block changes; the blocks alone are saved, and the region checkpoints.
        Step {
            tick: 3,
            blocks: vec![(BlockPos::new(9, -61, 9), blocks::AIR)],
            entities: Vec::new(),
            save: Some(Parts::BLOCKS),
            checkpoint: Some(4),
        },
        // The item goes; the entities alone are saved, which removes their file.
        Step {
            tick: 5,
            blocks: Vec::new(),
            entities: vec![EntityChange::Gone(EntityId(FIRST))],
            save: Some(Parts::ENTITIES),
            checkpoint: None,
        },
        // Another is thrown and not saved: only the log has it.
        Step {
            tick: 6,
            blocks: Vec::new(),
            entities: vec![EntityChange::Whole(thrown(SECOND))],
            save: None,
            checkpoint: None,
        },
        // It is saved with a block, both parts, and the region checkpoints again.
        Step {
            tick: 7,
            blocks: vec![(BlockPos::new(3, -60, 3), blocks::DIRT)],
            entities: Vec::new(),
            save: Some(Parts::ALL),
            checkpoint: Some(8),
        },
    ]
}

/// The chunk after the first `count` steps.
fn after(count: usize) -> Chunk {
    let mut chunk = generator().generate(A);
    for step in steps().into_iter().take(count) {
        for (at, state) in &step.blocks {
            let (x, z) = at.in_chunk();
            chunk.set(x, at.y, z, *state);
        }
        for change in &step.entities {
            chunk.apply_entity(change);
        }
        chunk.clear_generated();
    }
    chunk
}

fn commit(handle: &StoreHandle, step: &Step) {
    handle.request(StoreRequest::Commit {
        tick: step.tick,
        changes: step.blocks.iter().copied().map(Change::from).collect(),
        state: format!("delta {}", step.tick).into_bytes(),
        stays: Vec::new(),
        ticks: Vec::new(),
        marks_taken: Vec::new(),
        seams: Vec::new(),
        effects: Vec::new(),
        taken: Vec::new(),
        events: Vec::new(),
        entity_changes: step
            .entities
            .iter()
            .map(|change| (A, change.clone()))
            .collect(),
        arrivals: Vec::new(),
        arrivals_taken: Vec::new(),
    });
}

/// Whether the file of A's entities is durable on `disk`.
fn has_entities_file(disk: &MemoryDisk) -> bool {
    let left = Arc::new(disk.crashed(Survival::Nothing));
    let files = FileChunks::new(left.clone(), Path::new("/world"));
    left.read(&files.entities_path(A)).unwrap().is_some()
}

/// After every step, and whatever a crash then leaves of the disk, the restored chunk
/// is the chunk as the commits so far made it; and once everything is flushed, the
/// entities' file is there exactly while the chunk has entities.
#[test]
fn a_chunk_saved_by_parts_between_its_commits_is_restored_as_committed_after_any_crash() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let (east, _) = store.open_region(hello(EAST, 1)).unwrap();
    let mut epoch = 1;
    for (done, step) in steps().iter().enumerate() {
        commit(&east, step);
        committed(&east, step.tick);
        if let Some(parts) = step.save {
            east.request(StoreRequest::Save {
                position: A,
                tick: step.tick,
                chunk: after(done + 1),
                parts,
            });
        }
        if let Some(tick) = step.checkpoint {
            checkpoint(&east, tick);
        }
        east.flush();
        let expected = after(done + 1);
        if step.save.is_some_and(|parts| parts.entities) {
            assert_eq!(
                has_entities_file(&disk),
                !expected.entities().is_empty(),
                "the entities' file after step {}",
                done + 1
            );
        }
        for survival in SURVIVALS {
            epoch += 1;
            let case = format!("a crash after step {} that keeps {survival:?}", done + 1);
            let left = Arc::new(disk.crashed(survival));
            let again = store_on(&left, &division()).unwrap();
            let (handle, _) = again.open_region(hello(EAST, epoch)).unwrap();
            let restored = load(&handle, A);
            assert_eq!(restored.entities(), expected.entities(), "{case}");
            assert_eq!(restored, expected, "{case}");
            handle.flush();
            // And once more, from what that restore stored.
            drop((handle, again));
            let again = store_on(&left, &division()).unwrap();
            epoch += 1;
            let (handle, _) = again.open_region(hello(EAST, epoch)).unwrap();
            assert_eq!(load(&handle, A), expected, "{case}, restored twice");
            handle.flush();
        }
    }
    drop((east, store));
}

/// The plan, section 5: a save of the entities alone of a chunk that is stored leaves
/// its blocks' file as it was, byte for byte, and one of the blocks alone leaves the
/// entities' file; and a save of the blocks pending when one of the entities comes
/// writes both, from the later chunk.
#[test]
fn a_save_of_one_part_leaves_the_other_file_byte_for_byte_and_pending_saves_write_both() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let (east, _) = store.open_region(hello(EAST, 1)).unwrap();
    let files = FileChunks::new(disk.clone(), Path::new("/world"));
    let read = |path: &Path| {
        let left = disk.crashed(Survival::Nothing);
        left.read(path).unwrap()
    };
    let all = steps();
    commit(&east, &all[0]);
    committed(&east, all[0].tick);
    east.request(StoreRequest::Save {
        position: A,
        tick: 1,
        chunk: after(1),
        parts: Parts::ALL,
    });
    east.flush();
    let blocks_file = read(&files.chunk_path(A)).expect("the blocks are stored");
    let entities_file = read(&files.entities_path(A)).expect("the entities are stored");

    commit(&east, &all[1]);
    committed(&east, all[1].tick);
    east.request(StoreRequest::Save {
        position: A,
        tick: 2,
        chunk: after(2),
        parts: Parts::ENTITIES,
    });
    east.flush();
    assert_eq!(read(&files.chunk_path(A)), Some(blocks_file.clone()));
    assert_ne!(read(&files.entities_path(A)), Some(entities_file));
    let entities_file = read(&files.entities_path(A)).expect("still there");

    commit(&east, &all[2]);
    committed(&east, all[2].tick);
    east.request(StoreRequest::Save {
        position: A,
        tick: 3,
        chunk: after(3),
        parts: Parts::BLOCKS,
    });
    east.flush();
    assert_eq!(read(&files.entities_path(A)), Some(entities_file));
    assert_ne!(read(&files.chunk_path(A)), Some(blocks_file));

    // Two saves while neither is written: blocks, then entities of a later chunk.
    let later = {
        let mut chunk = after(3);
        chunk.set(1, -60, 1, blocks::SAND);
        chunk.apply_entity(&EntityChange::Whole(thrown(SECOND)));
        chunk
    };
    east.request(StoreRequest::Save {
        position: A,
        tick: 3,
        chunk: after(3),
        parts: Parts::BLOCKS,
    });
    east.request(StoreRequest::Save {
        position: A,
        tick: 3,
        chunk: later.clone(),
        parts: Parts::ENTITIES,
    });
    east.flush();
    let left = Arc::new(disk.crashed(Survival::Nothing));
    let stored = FileChunks::new(left, Path::new("/world"))
        .load(A)
        .unwrap()
        .expect("stored");
    assert_eq!(stored, later, "both parts of the later chunk");
    drop((east, store));
}
