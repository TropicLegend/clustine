//! Tests of entities other than players in the store, written from
//! `docs/adr/0028-entities.md` (sections 2 and 3) and `docs/groundwork/n2-plan.md`
//! (sections 4 to 6 and 11): a commit's entity changes are replayed into their chunks
//! after its blocks, under the rules of who held the chunk that blocks follow; a chunk's
//! entities are kept in a file of their own beside its blocks, saved by parts, and
//! removed when the last is gone; they go with their chunks through a merge, a split, a
//! lend and its return, and a change of division; and copies are based on them and
//! kept up to date with them. The sim makes no entity changes yet, so the tests make
//! them by hand.

use std::path::Path;
use std::thread;
use std::time::Duration;

use clustine_data::{BlockState, blocks};
use clustine_format::EntitiesFile;
use clustine_rpc::Parts;
use clustine_world::{
    BlockPos, Change, Chunk, ChunkPos, Entity, EntityChange, EntityData, EntityField, EntityId,
    ItemStack,
};

use super::*;
use crate::chunks::FileChunks;
use crate::disk::{MemoryDisk, Survival};
use crate::regions::{
    SURVIVALS, absorb, checkpoint, claim, gap, hello_of, split, state_of, store_on, whole,
};
use crate::tests::{committed, division, generator, hello, load, open};

const ROOT: &str = "/world";

// The world of most of these tests is that of [`division`]: region 0 west of x = 0,
// region 1 from x = 0 on, with the home chunk at the origin.

const WEST: u32 = 0;
const EAST: u32 = 1;

/// Chunks of the eastern region.
const A: ChunkPos = ChunkPos::new(0, 0);
const B: ChunkPos = ChunkPos::new(1, 0);
const C: ChunkPos = ChunkPos::new(2, 0);

/// A chunk of the western region.
const W: ChunkPos = ChunkPos::new(-1, 0);

/// An item of key `key` lying at `x`, `z` in the world.
pub(crate) fn item(key: u64, x: f64, z: f64) -> Entity {
    Entity {
        key: EntityId(key),
        kind: 71,
        uuid: uuid::Uuid::from_u128((u128::from(key) << 64) | 0xabc),
        position: [x, -60.0, z],
        motion: [0.0, -0.04, 0.0],
        yaw: 12.5,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 3,
        fresh_at: 0,
        data: EntityData::Item {
            stack: ItemStack::new(1, 5),
            pick_up_delay: 40,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
        fall_distance: 0.0,
        fire_ticks: -1,
        riding: Default::default(),
    }
}

/// `entity` with `fields` set.
fn set(entity: &Entity, fields: &[EntityField]) -> Entity {
    let mut entity = entity.clone();
    for field in fields {
        field.set(&mut entity);
    }
    entity
}

/// Commits `tick` with the block changes and the entity changes given.
fn commit(
    handle: &StoreHandle,
    tick: u64,
    changes: &[(BlockPos, BlockState)],
    entity_changes: Vec<(ChunkPos, EntityChange)>,
) {
    handle.request(StoreRequest::Commit {
        tick,
        changes: changes.iter().copied().map(Change::from).collect(),
        state: format!("delta {tick}").into_bytes(),
        stays: Vec::new(),
        ticks: Vec::new(),
        marks_taken: Vec::new(),
        seams: Vec::new(),
        effects: Vec::new(),
        taken: Vec::new(),
        events: Vec::new(),
        entity_changes,
        arrivals: Vec::new(),
        arrivals_taken: Vec::new(),
        clock: Default::default(),
    });
}

fn save_parts(handle: &StoreHandle, position: ChunkPos, tick: u64, chunk: &Chunk, parts: Parts) {
    handle.request(StoreRequest::Save {
        position,
        tick,
        chunk: chunk.clone(),
        parts,
    });
}

/// The chunk the flat world has at `position`, with `blocks` set and `entities` in it,
/// and no longer as generated: a commit of entities clears that as one of blocks does.
fn expected(position: ChunkPos, blocks: &[(BlockPos, BlockState)], entities: Vec<Entity>) -> Chunk {
    let mut chunk = generator().generate(position);
    for (at, state) in blocks {
        let (x, z) = at.in_chunk();
        chunk.set(x, at.y, z, *state);
    }
    chunk.set_entities(entities);
    // The generated mark is a statement about the blocks: an entity's change leaves it
    // (`docs/groundwork/n7l-plan.md`, section 9 item 4).
    chunk
}

fn files(disk: &Arc<MemoryDisk>) -> FileChunks {
    FileChunks::new(disk.clone(), Path::new(ROOT))
}

/// The entities durable on `disk` in the file of the chunk at `position`, if it has one.
fn entities_file(disk: &MemoryDisk, position: ChunkPos) -> Option<Vec<Entity>> {
    let left = Arc::new(disk.crashed(Survival::Nothing));
    let path = files(&left).entities_path(position);
    let bytes = left.read(&path).unwrap()?;
    let file = EntitiesFile::decode(&bytes).unwrap();
    assert_eq!(file.position, position);
    Some(file.entities)
}

/// The chunk durable on `disk` at `position`, as a store would load it.
fn stored(disk: &MemoryDisk, position: ChunkPos) -> Option<Chunk> {
    let left = Arc::new(disk.crashed(Survival::Nothing));
    files(&left).load(position).unwrap()
}

/// Sand in `A`, which the first commit of [`three_commits`] places.
fn sand_in_a() -> (BlockPos, BlockState) {
    (BlockPos::new(3, -60, 4), blocks::SAND)
}

fn first() -> Entity {
    item((1 << 32) | 1, 3.5, 4.5)
}

fn second() -> Entity {
    item((1 << 32) | 2, 20.5, 4.5)
}

/// Where the second goes in the second commit: into `C`, as a move between two chunks
/// of a region is written, gone from one and whole in the other.
fn second_in_c() -> Entity {
    let mut moved = second();
    moved.position = [36.25, -60.0, 4.5];
    moved.hops = 0;
    moved
}

fn first_moved() -> [EntityField; 2] {
    [
        EntityField::Position([5.5, -59.5, 4.5]),
        EntityField::Rotation {
            yaw: -90.0,
            pitch: 1.0,
        },
    ]
}

fn first_aged() -> [EntityField; 3] {
    [
        EntityField::Lived(9),
        EntityField::OnGround(false),
        EntityField::Data(EntityData::Item {
            stack: ItemStack::new(1, 4),
            pick_up_delay: 0,
            thrower: Some(uuid::Uuid::from_u128(77)),
            health: 5,
            age_offset: 0,
        }),
    ]
}

/// Three commits of the eastern region, none of them saved into a chunk:
///
/// 1. sand in `A`, the first item whole in `A` and the second in `B`;
/// 2. the first moved within `A`, the second gone from `B` and whole in `C`;
/// 3. the first aged, and a field set of a key `A` does not have, which is passed over.
fn three_commits(east: &StoreHandle) {
    commit(
        east,
        1,
        &[sand_in_a()],
        vec![
            (A, EntityChange::Whole(first())),
            (B, EntityChange::Whole(second())),
        ],
    );
    commit(
        east,
        2,
        &[],
        vec![
            (A, EntityChange::Set(first().key, first_moved().to_vec())),
            (B, EntityChange::Gone(second().key)),
            (C, EntityChange::Whole(second_in_c())),
        ],
    );
    commit(
        east,
        3,
        &[],
        vec![
            (A, EntityChange::Set(first().key, first_aged().to_vec())),
            (
                A,
                EntityChange::Set(EntityId(99), vec![EntityField::Lived(1)]),
            ),
        ],
    );
    committed(east, 3);
}

fn first_at_the_end() -> Entity {
    set(&set(&first(), &first_moved()), &first_aged())
}

/// What each chunk is restored with after [`three_commits`].
fn as_committed(handle: &StoreHandle, case: &str) {
    assert_eq!(
        load(handle, A),
        expected(A, &[sand_in_a()], vec![first_at_the_end()]),
        "{case}"
    );
    assert_eq!(load(handle, B), expected(B, &[], Vec::new()), "{case}");
    assert_eq!(
        load(handle, C),
        expected(C, &[], vec![second_in_c()]),
        "{case}"
    );
}

/// A restore puts each commit's entity changes into their chunks after its blocks, in
/// order: a whole entity is added, a field set is set, one gone is removed, and a
/// change of a key the chunk does not have is passed over. So it is on the same store,
/// and on a store started on what any crash leaves of it.
#[test]
fn a_restore_puts_each_commits_entity_changes_into_their_chunks_in_order() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let east = open(&store, hello(EAST, 1));
    three_commits(&east);
    east.flush();

    let (again, restored) = store.open_region(hello(EAST, 2)).unwrap();
    assert_eq!(restored.deltas.len(), 3);
    as_committed(&again, "the same store");
    again.flush();
    drop((again, east, store));

    for (epoch, survival) in (3..).zip(SURVIVALS) {
        let case = format!("{survival:?}");
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &division()).unwrap();
        let (handle, _) = store.open_region(hello(EAST, epoch)).unwrap();
        as_committed(&handle, &case);
        handle.flush();
    }
}

/// The chunks a restore changes are stored as a save stores them: their entities in a
/// file of their own beside the blocks, named for the chunk, and none for a chunk whose
/// entities are all gone.
#[test]
fn chunks_restored_with_entities_have_them_in_a_file_beside_their_blocks() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let east = open(&store, hello(EAST, 1));
    three_commits(&east);
    east.flush();
    let (again, _) = store.open_region(hello(EAST, 2)).unwrap();
    again.flush();
    drop((again, east, store));

    let path = files(&disk).entities_path(C);
    assert_eq!(
        path,
        Path::new("/world/chunks/overworld/0.0/2.0.entities"),
        "beside {:?}",
        files(&disk).chunk_path(C)
    );
    assert_eq!(entities_file(&disk, A), Some(vec![first_at_the_end()]));
    assert_eq!(entities_file(&disk, B), None);
    assert_eq!(entities_file(&disk, C), Some(vec![second_in_c()]));
    let stored_c = stored(&disk, C).expect("the restore stored C");
    assert_eq!(stored_c, expected(C, &[], vec![second_in_c()]));
    // Still as generated: the mark is the blocks', and a blocks file beside its
    // entities means taken (`docs/groundwork/n7l-plan.md`, sections 2.4 and 9).
    assert_ne!(stored_c.generated(), 0);
}

/// What a region committed of the entities of a chunk it does not hold is not put into
/// the chunk, as its block changes are not.
#[test]
fn entity_changes_committed_for_a_chunk_the_region_does_not_hold_are_not_replayed() {
    let store = store_on(&Arc::new(MemoryDisk::default()), &division()).unwrap();
    let west = open(&store, hello(WEST, 1));
    let east = open(&store, hello(EAST, 1));
    let in_w = item(7, -8.5, 3.0);
    commit(
        &west,
        1,
        &[],
        vec![
            (W, EntityChange::Whole(in_w.clone())),
            (B, EntityChange::Whole(second())),
        ],
    );
    committed(&west, 1);
    west.flush();
    let (again, _) = store.open_region(hello(WEST, 2)).unwrap();
    assert_eq!(load(&again, W), expected(W, &[], vec![in_w]));
    assert_eq!(load(&east, B), generator().generate(B));
}

/// A save of the blocks alone leaves the entities' file as it was, and one of the
/// entities alone leaves the blocks' file (the plan, section 5).
#[test]
fn a_save_of_one_part_leaves_the_other_file_as_it_was() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let east = open(&store, hello(EAST, 1));
    let stone = (BlockPos::new(1, 100, 1), blocks::STONE);
    let glass = (BlockPos::new(1, 100, 1), blocks::GLASS);
    let other = item(5, 9.0, 9.0);
    save_parts(
        &east,
        A,
        1,
        &expected(A, &[stone], vec![first()]),
        Parts::ALL,
    );
    east.flush();
    assert_eq!(stored(&disk, A), Some(expected(A, &[stone], vec![first()])));

    save_parts(
        &east,
        A,
        2,
        &expected(A, &[glass], vec![first(), other.clone()]),
        Parts::BLOCKS,
    );
    east.flush();
    assert_eq!(stored(&disk, A), Some(expected(A, &[glass], vec![first()])));

    save_parts(
        &east,
        A,
        3,
        &expected(A, &[stone], vec![other.clone()]),
        Parts::ENTITIES,
    );
    east.flush();
    assert_eq!(
        stored(&disk, A),
        Some(expected(A, &[glass], vec![other.clone()]))
    );
    // As a load through the store has it, from the files.
    let left = Arc::new(disk.crashed(Survival::Nothing));
    let store = store_on(&left, &division()).unwrap();
    let again = open(&store, hello(EAST, 2));
    assert_eq!(load(&again, A), expected(A, &[glass], vec![other]));
}

/// A second save of a chunk while the first is pending writes the parts of both, from
/// the later chunk (the plan, section 11): a save of the blocks followed by one of the
/// entities writes both, and so does the other way round.
#[test]
fn a_pending_save_of_blocks_then_of_entities_writes_both_from_the_later_chunk() {
    for (one, other) in [
        (Parts::BLOCKS, Parts::ENTITIES),
        (Parts::ENTITIES, Parts::BLOCKS),
    ] {
        let case = format!("{one:?} then {other:?}");
        let disk = Arc::new(MemoryDisk::default());
        let mut chunks = files(&disk);
        let earlier = expected(
            A,
            &[(BlockPos::new(1, 100, 1), blocks::STONE)],
            vec![first()],
        );
        let later = expected(
            A,
            &[(BlockPos::new(2, 100, 1), blocks::GLASS)],
            vec![set(&first(), &first_moved())],
        );
        // What is stored before: the earlier chunk, whole.
        chunks.save(A, 1, 1, &earlier).unwrap();
        chunks.sync().unwrap();
        let newer = expected(A, &[], vec![item(9, 1.0, 1.0)]);
        chunks.save_parts(A, 2, 1, &newer, one).unwrap();
        chunks.save_parts(A, 3, 1, &later, other).unwrap();
        assert_eq!(chunks.load(A).unwrap().as_ref(), Some(&later), "{case}");
        chunks.sync().unwrap();
        assert_eq!(stored(&disk, A), Some(later), "{case}");
    }
}

/// The entities' file of a chunk whose last entity is gone is removed by its save, and
/// the entity does not come back: not after a checkpoint has cut its commits from the
/// log, and not after a crash that leaves whatever it leaves.
#[test]
fn removing_a_chunks_last_entity_removes_its_file_and_the_entity_does_not_come_back() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let east = open(&store, hello(EAST, 1));
    commit(
        &east,
        1,
        &[sand_in_a()],
        vec![(A, EntityChange::Whole(first()))],
    );
    let with = expected(A, &[sand_in_a()], vec![first()]);
    save_parts(&east, A, 1, &with, Parts::ALL);
    east.request(StoreRequest::Checkpoint {
        tick: 1,
        state: b"state 1".to_vec(),
    });
    east.flush();
    assert_eq!(entities_file(&disk, A), Some(vec![first()]));

    commit(&east, 2, &[], vec![(A, EntityChange::Gone(first().key))]);
    let without = expected(A, &[sand_in_a()], Vec::new());
    save_parts(&east, A, 2, &without, Parts::ENTITIES);
    east.request(StoreRequest::Checkpoint {
        tick: 2,
        state: b"state 2".to_vec(),
    });
    east.flush();
    let path = files(&disk).entities_path(A);
    assert!(!disk.exists(&path).unwrap());
    assert!(!disk.crashed(Survival::Nothing).exists(&path).unwrap());
    assert_eq!(load(&east, A), without);
    drop((east, store));

    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &division()).unwrap();
        let (handle, restored) = store.open_region(hello(EAST, epoch)).unwrap();
        assert_eq!(restored.deltas, [], "{survival:?}");
        assert_eq!(load(&handle, A), without, "{survival:?}");
        handle.flush();
    }

    // Gone in a commit that no save follows: the restore replays it, and removes the
    // file the save before left.
    let store = store_on(&disk, &division()).unwrap();
    let (east, _) = store.open_region(hello(EAST, 10)).unwrap();
    let again = item(3, 7.0, 7.0);
    commit(&east, 3, &[], vec![(A, EntityChange::Whole(again.clone()))]);
    save_parts(
        &east,
        A,
        3,
        &expected(A, &[sand_in_a()], vec![again.clone()]),
        Parts::ENTITIES,
    );
    commit(&east, 4, &[], vec![(A, EntityChange::Gone(again.key))]);
    committed(&east, 4);
    east.flush();
    assert_eq!(entities_file(&disk, A), Some(vec![again]));
    let (restored, _) = store.open_region(hello(EAST, 11)).unwrap();
    restored.flush();
    assert_eq!(entities_file(&disk, A), None);
    assert_eq!(load(&restored, A), without);
}

/// When the world is made over for another division at start-up, the entity changes
/// the regions of the old one committed go into their chunks, as their blocks do; the
/// regions of the new one are restored with nothing and find them in the chunks.
#[test]
fn a_world_made_over_for_another_division_has_the_entities_its_regions_committed() {
    let disk = Arc::new(MemoryDisk::default());
    let in_w = item(7, -8.5, 3.0);
    {
        let store = store_on(&disk, &division()).unwrap();
        let west = open(&store, hello(WEST, 1));
        let east = open(&store, hello(EAST, 1));
        commit(&west, 1, &[], vec![(W, EntityChange::Whole(in_w.clone()))]);
        three_commits(&east);
        committed(&west, 1);
        west.flush();
        east.flush();
    }
    let single = Division::side_by_side(A, &[]).unwrap();
    for survival in SURVIVALS {
        let case = format!("{survival:?}");
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &single).unwrap();
        let (whole, restored) = store.open_region(hello(0, 5)).unwrap();
        assert_eq!(
            (restored.state, restored.deltas),
            (None, Vec::new()),
            "{case}"
        );
        assert_eq!(
            load(&whole, W),
            expected(W, &[], vec![in_w.clone()]),
            "{case}"
        );
        as_committed(&whole, &case);
        whole.flush();
        drop((whole, store));
        let again = Arc::new(left.crashed(Survival::Nothing));
        let store = store_on(&again, &single).unwrap();
        let (whole, _) = store.open_region(hello(0, 6)).unwrap();
        as_committed(&whole, &format!("{case}, again"));
        whole.flush();
    }
}

// Merges, splits and lends, in the world with a gap (`regions::gap`): region 0 west of
// x = 0, region 1 from chunk x = 16 on, the home region 2 at the origin, and between
// them chunks nobody holds until they are claimed.

const FREE: ChunkPos = ChunkPos::new(5, 5);

fn in_free() -> Entity {
    item((2 << 32) | 1, 85.5, 90.25)
}

/// The chunk `FREE` as a region that committed [`in_free`] into it has it.
fn free_with_its_item() -> Chunk {
    expected(FREE, &[], vec![in_free()])
}

/// A merge takes the absorbed region's entities with its chunks: the survivor loads
/// them, and so does a store started on what any crash leaves.
#[test]
fn a_merge_takes_the_absorbed_regions_entities_with_its_chunks() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (survivor, _) = store.open_region(hello_of(&gap(), 0, 1)).unwrap();
    let (other, _) = store.open_region(hello_of(&gap(), 1, 3)).unwrap();
    assert_eq!(claim(&other, &[FREE]).0, [FREE]);
    commit(&other, 1, &[], vec![(FREE, EntityChange::Whole(in_free()))]);
    save_parts(&other, FREE, 1, &free_with_its_item(), Parts::ALL);
    checkpoint(&other, 4);
    checkpoint(&survivor, 6);
    assert!(matches!(
        absorb(&survivor, 1, 3, 10),
        StoreReply::Absorbed { .. }
    ));
    assert_eq!(load(&survivor, FREE), free_with_its_item());
    survivor.flush();
    drop((survivor, other, store));

    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &gap()).unwrap();
        let (handle, restored) = store.open_region(hello_of(&gap(), 0, epoch)).unwrap();
        assert_eq!(state_of(&restored), Some((10, whole("merged", 10))));
        assert_eq!(load(&handle, FREE), free_with_its_item(), "{survival:?}");
        handle.flush();
    }
}

/// A split gives the part the entities of its chunks.
#[test]
fn a_split_gives_the_part_the_entities_of_its_chunks() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (old, _) = store.open_region(hello_of(&gap(), 0, 1)).unwrap();
    assert_eq!(claim(&old, &[FREE]).0, [FREE]);
    commit(&old, 1, &[], vec![(FREE, EntityChange::Whole(in_free()))]);
    save_parts(&old, FREE, 1, &free_with_its_item(), Parts::ALL);
    checkpoint(&old, 2);
    assert_eq!(
        split(&old, 7, &[FREE], 4, 3),
        StoreReply::Split {
            region: RegionId(3)
        }
    );
    old.flush();
    let (part, _) = store.open_region(hello_of(&gap(), 3, 4)).unwrap();
    assert_eq!(load(&part, FREE), free_with_its_item());
    part.flush();
    drop((part, old, store));

    for (epoch, survival) in (5..).zip(SURVIVALS) {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &gap()).unwrap();
        let (part, _) = store.open_region(hello_of(&gap(), 3, epoch)).unwrap();
        assert_eq!(load(&part, FREE), free_with_its_item(), "{survival:?}");
        part.flush();
    }
}

/// The next answer to `handle` that `wanted` says is the one, passing over the others.
fn until(handle: &StoreHandle, wanted: impl Fn(&StoreReply) -> bool) -> StoreReply {
    for _ in 0..30_000 {
        match handle.try_reply() {
            Some(reply) if wanted(&reply) => return reply,
            Some(_) => {}
            None => thread::sleep(Duration::from_millis(1)),
        }
    }
    panic!("the store did not answer");
}

fn loaded_by(handle: &StoreHandle, position: ChunkPos) -> Chunk {
    handle.request(StoreRequest::Load { position });
    match until(handle, |reply| matches!(reply, StoreReply::Loaded { .. })) {
        StoreReply::Loaded { chunk, .. } => chunk,
        _ => unreachable!(),
    }
}

/// A lend lends a chunk's entities with it, and the return takes back what the
/// borrower made of them: its last save, of the entities alone, is what the lender
/// loads, also after a crash (`docs/adr/0028-entities.md`, section 2).
#[test]
fn a_lend_lends_a_chunks_entities_and_its_return_takes_them_back_as_the_borrower_left_them() {
    let stripes = Division::side_by_side(ChunkPos::new(0, 0), &[0]).unwrap();
    let at_line = ChunkPos::new(-1, 3);
    let lying = item((4 << 32) | 1, -3.5, 50.5);
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &stripes).unwrap();
    let lender = open(&store, hello(0, 1));
    let borrower = open(&store, hello(1, 1));

    borrower.request(StoreRequest::Lend {
        chunks: vec![at_line],
    });
    let StoreReply::LendAsked { lend, .. } = until(&lender, |reply| {
        matches!(reply, StoreReply::LendAsked { .. })
    }) else {
        unreachable!()
    };
    let lent = expected(at_line, &[], vec![lying.clone()]);
    save_parts(&lender, at_line, 1, &lent, Parts::ALL);
    lender.request(StoreRequest::Lending {
        lend,
        chunks: vec![at_line],
    });
    until(&borrower, |reply| {
        matches!(reply, StoreReply::Claimed { .. })
    });
    until(&lender, |reply| matches!(reply, StoreReply::Lent { .. }));
    assert_eq!(loaded_by(&borrower, at_line), lent);

    let moved = [EntityField::Position([-2.5, -61.0, 51.5])];
    let left_behind = expected(at_line, &[], vec![set(&lying, &moved)]);
    save_parts(&borrower, at_line, 9, &left_behind, Parts::ENTITIES);
    borrower.request(StoreRequest::Return {
        chunks: vec![at_line],
        unreadable: Vec::new(),
    });
    until(&lender, |reply| {
        matches!(reply, StoreReply::PinnedBack { .. })
    });
    assert_eq!(loaded_by(&lender, at_line), left_behind);
    lender.flush();
    drop((lender, borrower, store));

    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &stripes).unwrap();
        let lender = open(&store, hello(0, epoch));
        assert_eq!(loaded_by(&lender, at_line), left_behind, "{survival:?}");
    }
}

// Copies, in the world with a gap: region 1 copies a chunk of region 0.

const COPIED: ChunkPos = ChunkPos::new(-3, 0);

fn copies(handle: &StoreHandle, wanted: &[ChunkPos]) {
    handle.request(StoreRequest::Copies {
        wanted: wanted.to_vec(),
        gone: Vec::new(),
    });
}

/// The copies' answers on the stream of `handle` before the answer to a flush.
fn streamed_before_flush(handle: &StoreHandle) -> Vec<StoreReply> {
    handle.request(StoreRequest::Flush);
    let mut streamed = Vec::new();
    loop {
        match until(handle, |reply| {
            matches!(
                reply,
                StoreReply::Flushed | StoreReply::CopyBase { .. } | StoreReply::CopyUpdate { .. }
            )
        }) {
            StoreReply::Flushed => return streamed,
            other => streamed.push(other),
        }
    }
}

/// A copy's base has the entities its holder committed, and its updates carry the
/// entity changes of later commits in their order: a commit of entities alone is a
/// link of the chain, which the next update names as the one before, so that a gap
/// in them shows (`docs/adr/0028-entities.md`, section 3).
#[test]
fn a_copy_is_based_on_its_chunks_entities_and_its_updates_carry_their_changes() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let holder = store.open_region(hello_of(&gap(), 0, 1)).unwrap().0;
    let copier = store.open_region(hello_of(&gap(), 1, 1)).unwrap().0;
    let lying = item(6, -40.5, 2.5);
    commit(
        &holder,
        1,
        &[],
        vec![(COPIED, EntityChange::Whole(lying.clone()))],
    );
    streamed_before_flush(&holder);

    copies(&copier, &[COPIED]);
    let StoreReply::CopyBase {
        sequence: based,
        chunk,
        ..
    } = until(&copier, |reply| {
        matches!(reply, StoreReply::CopyBase { .. })
    })
    else {
        unreachable!()
    };
    assert_eq!(chunk, Some(expected(COPIED, &[], vec![lying.clone()])));

    let moved = EntityChange::Set(lying.key, vec![EntityField::Position([-41.5, -60.0, 2.5])]);
    commit(&holder, 2, &[], vec![(COPIED, moved.clone())]);
    let glass = (BlockPos::new(-47, 100, 1), blocks::GLASS);
    let gone = EntityChange::Gone(lying.key);
    // A change elsewhere is not carried.
    let elsewhere = (
        ChunkPos::new(-4, 0),
        EntityChange::Whole(item(8, -60.0, 1.0)),
    );
    commit(
        &holder,
        3,
        &[glass],
        vec![elsewhere, (COPIED, gone.clone())],
    );
    streamed_before_flush(&holder);
    let streamed = streamed_before_flush(&copier);
    let [
        StoreReply::CopyUpdate {
            sequence: second,
            previous: before_second,
            changes: second_changes,
            entity_changes: second_entities,
            ..
        },
        StoreReply::CopyUpdate {
            previous: before_third,
            changes: third_changes,
            entity_changes: third_entities,
            ..
        },
    ] = &streamed[..]
    else {
        panic!("expected two updates, got {streamed:?}");
    };
    assert_eq!(*before_second, based);
    assert_eq!(second_changes, &[]);
    assert_eq!(second_entities, &[moved]);
    assert_eq!(before_third, second);
    assert_eq!(third_changes, &[Change::from(glass)]);
    assert_eq!(third_entities, &[gone]);

    // A copier that comes now is based on the chunk as the commits left it.
    let late = store.open_region(hello_of(&gap(), 2, 1)).unwrap().0;
    copies(&late, &[COPIED]);
    let StoreReply::CopyBase { chunk, .. } =
        until(&late, |reply| matches!(reply, StoreReply::CopyBase { .. }))
    else {
        unreachable!()
    };
    assert_eq!(chunk, Some(expected(COPIED, &[glass], Vec::new())));
}
