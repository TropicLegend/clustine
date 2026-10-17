//! Tests of the store's lends (step L2 of `docs/adr/0027-lending-at-a-line.md`): a lend
//! asked of its lender, granted behind the lender's saves, refused, called off, kept
//! one way in a stretch, and ended by the borrower's return.
//!
//! The world is two stripes that meet at x = 0: region 0 west of it, region 1 east of
//! it with the home chunk at the origin. Region 1 borrows from region 0.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use clustine_world::{Chunk, ChunkPos};

use super::*;
use crate::disk::MemoryDisk;
use crate::regions::store_on;
use crate::table::Division;
use crate::tests::generator;

const LENDER: RegionId = RegionId(0);
const BORROWER: RegionId = RegionId(1);
/// A chunk of the lender beside the borrower's land, and one further west.
const AT_LINE: ChunkPos = ChunkPos::new(-1, 3);
const FURTHER: ChunkPos = ChunkPos::new(-2, 3);

fn stripes() -> Division {
    Division::side_by_side(ChunkPos::new(0, 0), &[0]).unwrap()
}

fn opened(store: &Store, region: RegionId, epoch: u64) -> (StoreHandle, Restored) {
    store.open_region(RegionHello { region, epoch }).unwrap()
}

/// The next answer that has to do with lends or grants, passing over what the stream
/// carries besides and the answers to commits.
fn lend_reply(handle: &StoreHandle) -> StoreReply {
    for _ in 0..30_000 {
        match handle.try_reply() {
            Some(
                StoreReply::Committed { .. }
                | StoreReply::Generating { .. }
                | StoreReply::Mark { .. }
                | StoreReply::Effects { .. }
                | StoreReply::Flushed
                | StoreReply::CopyBase { .. }
                | StoreReply::CopyUpdate { .. }
                | StoreReply::CopyUsed { .. }
                | StoreReply::Dead { .. },
            ) => {}
            Some(reply) => return reply,
            None => thread::sleep(Duration::from_millis(1)),
        }
    }
    panic!("the store did not answer");
}

/// Whether `handle` has no answer to do with lends waiting once everything it asked for
/// before is done.
fn nothing_more(handle: &StoreHandle) -> Vec<StoreReply> {
    handle.request(StoreRequest::Flush);
    let mut more = Vec::new();
    loop {
        match lend_reply_or_flushed(handle) {
            None => return more,
            Some(reply) => more.push(reply),
        }
    }
}

fn lend_reply_or_flushed(handle: &StoreHandle) -> Option<StoreReply> {
    for _ in 0..30_000 {
        match handle.try_reply() {
            Some(StoreReply::Flushed) => return None,
            Some(
                StoreReply::Committed { .. }
                | StoreReply::Generating { .. }
                | StoreReply::Mark { .. }
                | StoreReply::Effects { .. }
                | StoreReply::CopyBase { .. }
                | StoreReply::CopyUpdate { .. }
                | StoreReply::CopyUsed { .. }
                | StoreReply::Dead { .. },
            ) => {}
            Some(reply) => return Some(reply),
            None => thread::sleep(Duration::from_millis(1)),
        }
    }
    panic!("the store did not answer");
}

fn ask(handle: &StoreHandle, chunks: &[ChunkPos]) {
    handle.request(StoreRequest::Lend {
        chunks: chunks.to_vec(),
    });
}

/// The lender's ask, taken apart.
fn asked(lender: &StoreHandle) -> (u64, Vec<ChunkPos>, RegionId) {
    match lend_reply(lender) {
        StoreReply::LendAsked { lend, chunks, to } => (lend, chunks, to),
        other => panic!("expected a lend asked for, got {other:?}"),
    }
}

fn save(handle: &StoreHandle, position: ChunkPos, tick: u64, chunk: Chunk) {
    handle.request(StoreRequest::Save {
        position,
        tick,
        chunk,
    });
}

/// Lends `AT_LINE` from a lender that saved `saved` in it first, and returns the lend's
/// number once both are told.
fn lent(lender: &StoreHandle, borrower: &StoreHandle, saved: Chunk) -> u64 {
    ask(borrower, &[AT_LINE]);
    let (lend, chunks, to) = asked(lender);
    assert_eq!((chunks.as_slice(), to), ([AT_LINE].as_slice(), BORROWER));
    save(lender, AT_LINE, 1, saved);
    lender.request(StoreRequest::Lending {
        lend,
        chunks: vec![AT_LINE],
    });
    assert_eq!(
        lend_reply(borrower),
        StoreReply::Claimed {
            granted: vec![AT_LINE],
            foreign: Vec::new(),
        }
    );
    assert_eq!(
        lend_reply(lender),
        StoreReply::Lent {
            chunks: vec![AT_LINE],
            to: BORROWER,
        }
    );
    lend
}

fn marked() -> Chunk {
    let mut chunk = generator().generate(AT_LINE);
    chunk.set(1, 100, 1, clustine_data::blocks::GLASS);
    chunk
}

#[test]
fn a_lend_is_granted_behind_the_lenders_save_and_both_are_told() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &stripes()).unwrap();
    let (lender, _) = opened(&store, LENDER, 1);
    let (borrower, _) = opened(&store, BORROWER, 1);
    lent(&lender, &borrower, marked());

    // The borrower loads what the lender saved, and the lender holds it no longer.
    borrower.request(StoreRequest::Load { position: AT_LINE });
    match lend_reply(&borrower) {
        StoreReply::Loaded { position, chunk } => {
            assert_eq!(position, AT_LINE);
            assert_eq!(chunk, marked());
        }
        other => panic!("expected the lent chunk, got {other:?}"),
    }
    lender.request(StoreRequest::Load { position: AT_LINE });
    assert_eq!(
        lend_reply(&lender),
        StoreReply::NotHeld {
            position: AT_LINE,
            holder: Some(BORROWER),
        }
    );
}

#[test]
fn a_lend_and_what_is_lent_away_come_with_a_hello() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &stripes()).unwrap();
    let (lender, _) = opened(&store, LENDER, 1);
    let (borrower, _) = opened(&store, BORROWER, 1);
    lent(&lender, &borrower, marked());
    drop((lender, borrower));
    let (_, restored) = opened(&store, BORROWER, 2);
    assert_eq!(restored.lends.lent, [AT_LINE]);
    assert!(restored.lends.lent_away.is_empty());
    assert!(restored.held.iter().any(|(chunk, _)| *chunk == AT_LINE));
    let (_, restored) = opened(&store, LENDER, 2);
    assert!(restored.lends.lent.is_empty());
    assert_eq!(restored.lends.lent_away, [(AT_LINE, BORROWER)]);
    drop(store);

    // And after a start, from the log.
    let store = store_on(&disk, &stripes()).unwrap();
    let (_, restored) = opened(&store, LENDER, 3);
    assert_eq!(restored.lends.lent_away, [(AT_LINE, BORROWER)]);
}

#[test]
fn the_lender_saves_and_loads_a_chunk_under_lend_until_it_gives_it_back() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &stripes()).unwrap();
    let (lender, _) = opened(&store, LENDER, 1);
    let (borrower, _) = opened(&store, BORROWER, 1);
    ask(&borrower, &[AT_LINE]);
    let (lend, ..) = asked(&lender);
    save(&lender, AT_LINE, 1, marked());
    lender.request(StoreRequest::Load { position: AT_LINE });
    match lend_reply(&lender) {
        StoreReply::Loaded { chunk, .. } => assert_eq!(chunk, marked()),
        other => panic!("expected the chunk loaded, got {other:?}"),
    }
    lender.request(StoreRequest::Claim {
        chunks: vec![AT_LINE],
    });
    assert_eq!(
        lend_reply(&lender),
        StoreReply::Claimed {
            granted: vec![AT_LINE],
            foreign: Vec::new(),
        }
    );
    // Nobody else is lent it meanwhile, and the borrower's own ask again is passed over.
    ask(&borrower, &[AT_LINE]);
    assert_eq!(nothing_more(&borrower), []);
    lender.request(StoreRequest::Lending {
        lend,
        chunks: vec![AT_LINE],
    });
    assert!(matches!(lend_reply(&borrower), StoreReply::Claimed { .. }));
}

#[test]
fn a_chunk_that_cannot_be_lent_is_answered_so_at_once() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &stripes()).unwrap();
    let (lender, _) = opened(&store, LENDER, 1);
    let (borrower, _) = opened(&store, BORROWER, 1);
    // Not beside the borrower's land, its own, and the home chunk.
    let own = ChunkPos::new(2, 3);
    ask(&borrower, &[FURTHER, own, ChunkPos::new(0, 0)]);
    assert_eq!(
        lend_reply(&borrower),
        StoreReply::NotLent {
            chunks: vec![FURTHER, own, ChunkPos::new(0, 0)],
            again: false,
        }
    );
    // A chunk it holds by a grant is answered as a claim would be, and the lender is
    // asked nothing.
    lent(&lender, &borrower, marked());
    ask(&borrower, &[AT_LINE]);
    assert_eq!(
        lend_reply(&borrower),
        StoreReply::Claimed {
            granted: vec![AT_LINE],
            foreign: Vec::new(),
        }
    );
    assert_eq!(nothing_more(&lender), []);
}

#[test]
fn the_lenders_refusal_reaches_the_borrower_as_it_was_given() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &stripes()).unwrap();
    let (lender, _) = opened(&store, LENDER, 1);
    let (borrower, _) = opened(&store, BORROWER, 1);
    for again in [true, false] {
        ask(&borrower, &[AT_LINE]);
        let (lend, ..) = asked(&lender);
        lender.request(StoreRequest::LendRefused {
            lend,
            chunks: vec![AT_LINE],
            again,
        });
        assert_eq!(
            lend_reply(&borrower),
            StoreReply::NotLent {
                chunks: vec![AT_LINE],
                again,
            }
        );
    }
}

#[test]
fn an_answer_for_a_lend_called_off_does_not_answer_a_later_one() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &stripes()).unwrap();
    let (lender, _) = opened(&store, LENDER, 1);
    let (borrower, _) = opened(&store, BORROWER, 1);
    ask(&borrower, &[AT_LINE]);
    let (first, ..) = asked(&lender);
    // The borrower's hello calls the lend off, and both are told.
    let (borrower, _) = opened(&store, BORROWER, 2);
    assert_eq!(
        lend_reply(&lender),
        StoreReply::LendCalledOff {
            lend: first,
            chunks: vec![AT_LINE],
        }
    );
    ask(&borrower, &[AT_LINE]);
    let (second, ..) = asked(&lender);
    assert_ne!(first, second);
    // A wait for the first does not end the second, and giving back for the first is
    // answered as called off and lends nothing.
    lender.request(StoreRequest::LendRefused {
        lend: first,
        chunks: vec![AT_LINE],
        again: true,
    });
    lender.request(StoreRequest::Lending {
        lend: first,
        chunks: vec![AT_LINE],
    });
    assert_eq!(
        lend_reply(&lender),
        StoreReply::LendCalledOff {
            lend: first,
            chunks: vec![AT_LINE],
        }
    );
    assert_eq!(nothing_more(&borrower), []);
    lender.request(StoreRequest::Lending {
        lend: second,
        chunks: vec![AT_LINE],
    });
    assert!(matches!(lend_reply(&borrower), StoreReply::Claimed { .. }));
}

#[test]
fn lends_go_one_way_in_a_stretch_of_the_line() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &stripes()).unwrap();
    let (west, _) = opened(&store, LENDER, 1);
    let (east, _) = opened(&store, BORROWER, 1);
    lent(&west, &east, marked());
    // The east's chunk across the line in the same stretch is not lent to the west,
    // and one in the next stretch is asked for.
    let same = ChunkPos::new(0, 7);
    let next = ChunkPos::new(0, 8);
    ask(&west, &[same, next]);
    assert_eq!(
        lend_reply(&west),
        StoreReply::NotLent {
            chunks: vec![same],
            again: false,
        }
    );
    let (_, chunks, to) = asked(&east);
    assert_eq!((chunks.as_slice(), to), ([next].as_slice(), LENDER));
    // While that is under way, the east waits for one of the west in that stretch.
    ask(&east, &[ChunkPos::new(-1, 9)]);
    assert_eq!(
        lend_reply(&east),
        StoreReply::NotLent {
            chunks: vec![ChunkPos::new(-1, 9)],
            again: true,
        }
    );
}

#[test]
fn a_lent_chunk_given_back_is_the_lenders_again_and_it_is_told() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &stripes()).unwrap();
    let (lender, _) = opened(&store, LENDER, 1);
    let (borrower, _) = opened(&store, BORROWER, 1);
    lent(&lender, &borrower, marked());
    borrower.request(StoreRequest::Return {
        chunks: vec![AT_LINE],
    });
    assert_eq!(
        lend_reply(&lender),
        StoreReply::PinnedBack {
            chunks: vec![AT_LINE],
        }
    );
    lender.request(StoreRequest::Load { position: AT_LINE });
    assert!(matches!(lend_reply(&lender), StoreReply::Loaded { .. }));
}

#[test]
fn effects_the_lender_had_not_taken_go_to_the_borrower_with_the_lend() {
    use clustine_world::{BlockPos, Effect, Effected};
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &stripes()).unwrap();
    let (lender, _) = opened(&store, LENDER, 1);
    let (borrower, _) = opened(&store, BORROWER, 1);
    // The lender uses the chunk, and the borrower does something in it, which is filed
    // for the lender and not taken.
    lender.request(StoreRequest::Load { position: AT_LINE });
    assert!(matches!(lend_reply(&lender), StoreReply::Loaded { .. }));
    let effect = Effect {
        at: BlockPos::new(AT_LINE.x * 16, 64, AT_LINE.z * 16),
        what: Effected::SetIf {
            expected: clustine_data::blocks::AIR,
            state: clustine_data::blocks::STONE,
            flags: 3,
        },
    };
    borrower.request(StoreRequest::Commit {
        tick: 1,
        changes: Vec::new(),
        state: b"tick 1".to_vec(),
        stays: Vec::new(),
        ticks: Vec::new(),
        marks_taken: Vec::new(),
        seams: Vec::new(),
        effects: vec![effect],
        taken: Vec::new(),
        events: Vec::new(),
    });
    lent(&lender, &borrower, marked());
    // Behind the grant, on the borrower's stream before a mark.
    let mut sent = Vec::new();
    loop {
        match borrower.replies.recv().unwrap() {
            StoreReply::Effects { effects } => sent.extend(effects),
            StoreReply::Mark { .. } if !sent.is_empty() => break,
            _ => {}
        }
    }
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].effect, effect);
}
