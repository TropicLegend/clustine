//! Tests of the portal index as the store keeps it (`docs/groundwork/p-plan.md`,
//! decision 8 and sections 3.3 and 15.5): kept from every commit's block changes, by
//! dimension; answered to a region's question once what it committed before is durable;
//! written with the table file, so that the log's compaction does not lose it; and
//! found again, the same, after a crash at any point.

use clustine_data::{BlockState, blocks};
use clustine_rpc::{PortalQuery, QueryId};
use clustine_world::BlockPos;

use super::*;
use crate::disk::MemoryDisk;
use crate::regions::{SURVIVALS, store_on, table_file};
use crate::tests::{any_reply, committed, division, hello, open, other_homes};

/// The eastern region of [`division`], which holds the home chunk (0, 0).
const EAST: u32 = 1;

/// Two blocks of a portal in chunk (0, 0), which the eastern region holds in the
/// overworld and the Nether's home in the Nether.
const LOWER: BlockPos = BlockPos::new(4, -58, 4);
const UPPER: BlockPos = BlockPos::new(4, -57, 4);

fn commit(handle: &StoreHandle, tick: u64, changes: &[(BlockPos, BlockState)]) {
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
        claims: Vec::new(),
        entity_changes: Vec::new(),
        arrivals: Vec::new(),
        arrivals_taken: Vec::new(),
        clock: Default::default(),
        raids: Vec::new(),
    });
}

/// The portal blocks within 16 blocks of [`LOWER`] that the store says the region of
/// `handle` has, in its dimension: the answer to one question, waited for.
fn portals(handle: &StoreHandle, tick: u64) -> Vec<BlockPos> {
    let id = QueryId { tick, index: 0 };
    handle.request(StoreRequest::Portals {
        queries: vec![PortalQuery {
            id,
            centre: LOWER,
            radius: 16,
        }],
    });
    loop {
        if let StoreReply::Portals { answers } = any_reply(handle) {
            assert_eq!(answers.len(), 1);
            assert_eq!(answers[0].id, id);
            return answers[0].portals.clone();
        }
    }
}

#[test]
fn the_portal_index_is_kept_by_dimension_written_with_the_table_and_found_after_a_crash() {
    let nether_home = other_homes(&division())[0];
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let east = open(&store, hello(EAST, 1));
    let nether = open(&store, hello(nether_home, 1));

    commit(
        &east,
        1,
        &[
            (LOWER, blocks::NETHER_PORTAL),
            (UPPER, blocks::NETHER_PORTAL),
        ],
    );
    commit(&nether, 1, &[(LOWER, blocks::NETHER_PORTAL)]);
    committed(&east, 1);
    committed(&nether, 1);
    assert_eq!(portals(&east, 1), [LOWER, UPPER]);
    assert_eq!(portals(&nether, 1), [LOWER]);

    // A block put in a portal block's place takes it out; a question asked behind the
    // commit is answered as of it.
    commit(&east, 2, &[(UPPER, blocks::AIR)]);
    committed(&east, 2);
    assert_eq!(portals(&east, 2), [LOWER]);

    // Both checkpoints free the log: the table file takes its place, with the index.
    east.request(StoreRequest::Checkpoint {
        tick: 2,
        state: b"east".to_vec(),
    });
    nether.request(StoreRequest::Checkpoint {
        tick: 1,
        state: b"nether".to_vec(),
    });
    east.flush();
    nether.flush();
    // A segment being written is never let go: a later commit and its checkpoint, as
    // the table's own tests do, free the first.
    commit(&east, 3, &[]);
    committed(&east, 3);
    east.request(StoreRequest::Checkpoint {
        tick: 3,
        state: b"east".to_vec(),
    });
    east.flush();
    let file = table_file(&disk);
    assert_eq!(
        file.portals,
        [(Dimension::Overworld, LOWER), (Dimension::Nether, LOWER)]
    );
    drop((east, nether, store));

    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let case = format!("{survival:?}");
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &division()).unwrap();
        let east = open(&store, hello(EAST, epoch));
        let nether = open(&store, hello(nether_home, epoch));
        assert_eq!(portals(&east, 4), [LOWER], "{case}");
        assert_eq!(portals(&nether, 2), [LOWER], "{case}");
    }
}

/// A store that never wrote its table file since the commits finds the index again by
/// replaying them, whatever survived of the last one.
#[test]
fn the_portal_index_is_found_again_from_the_log_alone() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let east = open(&store, hello(EAST, 1));
    commit(&east, 1, &[(LOWER, blocks::NETHER_PORTAL)]);
    committed(&east, 1);
    commit(&east, 2, &[(UPPER, blocks::NETHER_PORTAL)]);
    committed(&east, 2);
    east.flush();
    assert!(table_file(&disk).portals.is_empty());
    drop((east, store));

    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &division()).unwrap();
        let east = open(&store, hello(EAST, epoch));
        assert_eq!(portals(&east, 3), [LOWER, UPPER], "{survival:?}");
    }
}
