//! Tests of the claims on points of interest a commit carries, as the store replays them
//! (`docs/groundwork/n10-plan.md`, section 3.2): a chunk's claims are those of the last
//! commit that listed it, put in after the commit's blocks, so that a claim on a block
//! placed in the same tick holds; a claim on a block of another type is dropped, not the
//! chunk; and a chunk with claims is no longer as generated.

use clustine_data::{BlockState, blocks};
use clustine_world::poi::{Claim, PoiType};
use clustine_world::{BlockPos, Chunk, ChunkPos, InChunk};

use super::*;
use crate::disk::MemoryDisk;
use crate::regions::{SURVIVALS, store_on};
use crate::tests::{committed, division, generator, hello, load, open};

/// The eastern region of [`division`], and a chunk of it.
const EAST: u32 = 1;
const A: ChunkPos = ChunkPos::new(1, 0);

/// Where the lectern stands in `A`.
const AT: BlockPos = BlockPos::new(20, -60, 4);

fn claim() -> Claim {
    let (x, z) = AT.in_chunk();
    Claim {
        at: InChunk::new(x, AT.y, z),
        kind: PoiType::Librarian,
        free: 0,
    }
}

fn commit(
    handle: &StoreHandle,
    tick: u64,
    changes: &[(BlockPos, BlockState)],
    claims: Vec<(ChunkPos, Vec<Claim>)>,
) {
    handle.request(StoreRequest::Commit {
        travellers: Vec::new(),
        tick,
        changes: changes
            .iter()
            .copied()
            .map(clustine_world::Change::from)
            .collect(),
        state: format!("delta {tick}").into_bytes(),
        stays: Vec::new(),
        ticks: Vec::new(),
        marks_taken: Vec::new(),
        seams: Vec::new(),
        effects: Vec::new(),
        taken: Vec::new(),
        events: Vec::new(),
        claims,
        entity_changes: Vec::new(),
        arrivals: Vec::new(),
        arrivals_taken: Vec::new(),
        clock: Default::default(),
        raids: Vec::new(),
    });
}

/// The flat world's `A` with a lectern at [`AT`], as stored before the commits.
fn with_lectern() -> Chunk {
    let mut chunk = generator().generate(A);
    let (x, z) = AT.in_chunk();
    chunk.set(x, AT.y, z, blocks::LECTERN);
    chunk
}

/// Tick 1 claims the lectern; tick 2 breaks it and places another, which a villager
/// claims in the same tick: the commit's blocks take the first claim away, and its
/// claims give the new one.
fn two_commits(east: &StoreHandle) {
    east.request(StoreRequest::Save {
        position: A,
        tick: 0,
        chunk: with_lectern(),
        parts: clustine_rpc::Parts::ALL,
    });
    commit(east, 1, &[], vec![(A, vec![claim()])]);
    commit(
        east,
        2,
        &[(AT, blocks::AIR), (AT, blocks::LECTERN)],
        vec![(A, vec![claim()])],
    );
    committed(east, 2);
}

#[test]
fn a_claim_on_a_block_placed_in_the_same_tick_is_replayed_after_the_blocks() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let east = open(&store, hello(EAST, 1));
    two_commits(&east);
    east.flush();
    let (again, _) = store.open_region(hello(EAST, 2)).unwrap();
    let a = load(&again, A);
    assert_eq!(a.claims(), [claim()]);
    assert_eq!(a.generated(), 0, "a chunk with claims is not as generated");
    again.flush();
    drop((again, east, store));

    for (epoch, survival) in (3..).zip(SURVIVALS) {
        let case = format!("{survival:?}");
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &division()).unwrap();
        let (handle, _) = store.open_region(hello(EAST, epoch)).unwrap();
        assert_eq!(load(&handle, A).claims(), [claim()], "{case}");
        handle.flush();
    }
}

/// A commit whose claims name a block its own changes made another type keeps the
/// chunk, without the claim.
#[test]
fn a_claim_on_a_block_of_another_type_is_dropped_in_a_replay_and_the_chunk_kept() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let east = open(&store, hello(EAST, 1));
    east.request(StoreRequest::Save {
        position: A,
        tick: 0,
        chunk: with_lectern(),
        parts: clustine_rpc::Parts::ALL,
    });
    commit(
        &east,
        1,
        &[(AT, blocks::STONECUTTER)],
        vec![(A, vec![claim()])],
    );
    committed(&east, 1);
    east.flush();
    let (again, _) = store.open_region(hello(EAST, 2)).unwrap();
    let a = load(&again, A);
    assert!(a.claims().is_empty());
    let (x, z) = AT.in_chunk();
    assert_eq!(a.get(x, AT.y, z), Some(blocks::STONECUTTER));
    again.flush();
}
