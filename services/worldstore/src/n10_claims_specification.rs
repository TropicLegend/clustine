//! The store's part of points of interest, tested from the specification by someone who
//! did not write it: `docs/groundwork/n10-plan.md` (decisions 1 and 2, sections 3.2,
//! 3.5 and 3.6), `docs/world-format.md` (format 17: a chunk's claims, commit kind 19,
//! effect kind 12), `docs/adr/0022-what-a-chunk-carries.md` and
//! `docs/adr/0025-effects-between-regions.md`, their sections "Changed in N10".
//!
//! Claims are a chunk's part: a commit carries each changed chunk's claims whole, a
//! replay puts them in after the commit's blocks, a chunk with claims is no longer as
//! generated, and copies are updated by a commit that changes only claims, in the chain.
//! A release into land another region holds is an effect its holder is streamed. The
//! margin: each region's reports, and what each is published once a game second.

use clustine_data::{BlockState, blocks};
use clustine_rpc::{PoiSeen, PublishedVillage, VillageReport, WorldClock};
use clustine_world::poi::{Claim, PoiType};
use clustine_world::{BlockPos, Chunk, ChunkPos, Effect, Effected, FiledEffect, InChunk};

use super::*;
use crate::disk::{MemoryDisk, Survival};
use crate::regions::{SURVIVALS, gap, store_on};
use crate::tests::{committed, division, generator, load};
use crate::village::Village;

fn hello(region: u32, epoch: u64) -> RegionHello {
    RegionHello {
        region: RegionId(region),
        epoch,
    }
}

fn opened(store: &Store, region: u32, epoch: u64) -> StoreHandle {
    store.open_region(hello(region, epoch)).unwrap().0
}

/// What a commit of these tests carries.
#[derive(Default)]
struct Commit {
    changes: Vec<(BlockPos, BlockState)>,
    effects: Vec<Effect>,
    claims: Vec<(ChunkPos, Vec<Claim>)>,
    game_time: u64,
}

fn commit(handle: &StoreHandle, tick: u64, what: Commit) {
    handle.request(StoreRequest::Commit {
        tick,
        changes: what
            .changes
            .into_iter()
            .map(clustine_world::Change::from)
            .collect(),
        state: format!("delta {tick}").into_bytes(),
        stays: Vec::new(),
        ticks: Vec::new(),
        marks_taken: Vec::new(),
        seams: Vec::new(),
        effects: what.effects,
        taken: Vec::new(),
        events: Vec::new(),
        claims: what.claims,
        entity_changes: Vec::new(),
        arrivals: Vec::new(),
        arrivals_taken: Vec::new(),
        clock: WorldClock {
            game_time: what.game_time,
            day: what.game_time,
        },
    });
}

/// Everything `handle` is told up to the answer to a flush.
fn told_before_flush(handle: &StoreHandle) -> Vec<StoreReply> {
    handle.request(StoreRequest::Flush);
    let mut told = Vec::new();
    while let Ok(reply) = handle.replies.recv() {
        if reply == StoreReply::Flushed {
            return told;
        }
        told.push(reply);
    }
    panic!("the handle was lost: {told:?}");
}

fn in_chunk(pos: BlockPos) -> InChunk {
    let (x, z) = pos.in_chunk();
    InChunk::new(x, pos.y, z)
}

fn claim(pos: BlockPos, kind: PoiType, free: u8) -> Claim {
    Claim {
        at: in_chunk(pos),
        kind,
        free,
    }
}

fn bed(part: &str) -> BlockState {
    blocks::LIME_BED
        .with_property("part", part)
        .expect("a bed has a part")
}

// Claims in commits and their replay (plan, section 3.2; format 17, kind 19).

/// A chunk of the eastern region of [`division`] that nothing has stored yet.
const EAST: u32 = 1;
const A: ChunkPos = ChunkPos::new(2, 1);
const B: ChunkPos = ChunkPos::new(3, 1);
const HEAD: BlockPos = BlockPos::new(40, -60, 20);
const FOOT: BlockPos = BlockPos::new(40, -60, 21);
const BELL: BlockPos = BlockPos::new(50, -60, 20);

#[test]
fn a_bed_placed_and_claimed_in_one_tick_keeps_its_claim_through_a_replay_and_a_crash() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let east = opened(&store, EAST, 1);
    // Placed and claimed in one tick, in a chunk the store has never stored.
    commit(
        &east,
        1,
        Commit {
            changes: vec![(HEAD, bed("head")), (FOOT, bed("foot"))],
            claims: vec![(A, vec![claim(HEAD, PoiType::Home, 0)])],
            ..Commit::default()
        },
    );
    committed(&east, 1);
    east.flush();

    let again = opened(&store, EAST, 2);
    let a = load(&again, A);
    assert_eq!(a.claims(), [claim(HEAD, PoiType::Home, 0)]);
    assert_eq!(a.free_tickets(in_chunk(HEAD)), Some(0));
    assert_eq!(a.generated(), 0, "a chunk with claims is not as generated");
    again.flush();
    drop((again, east, store));

    for (epoch, survival) in (3..).zip(SURVIVALS) {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &division()).unwrap();
        let handle = opened(&store, EAST, epoch);
        assert_eq!(
            load(&handle, A).claims(),
            [claim(HEAD, PoiType::Home, 0)],
            "{survival:?}"
        );
        handle.flush();
    }
}

/// Kind 19 carries "per chunk whose claims changed … the chunk's claims whole (here a
/// chunk's list may be empty, when its last ticket was given back)": a chunk not named
/// keeps its claims, one named with none has none, the last commit naming it wins.
#[test]
fn each_commit_s_claims_replace_those_of_the_chunks_it_names_and_no_others() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let east = opened(&store, EAST, 1);
    let lectern = BlockPos::new(52, -60, 22);
    commit(
        &east,
        1,
        Commit {
            changes: vec![
                (HEAD, bed("head")),
                (FOOT, bed("foot")),
                (BELL, blocks::BELL),
                (lectern, blocks::LECTERN),
            ],
            claims: vec![
                (A, vec![claim(HEAD, PoiType::Home, 0)]),
                (B, vec![claim(BELL, PoiType::Meeting, 30)]),
            ],
            ..Commit::default()
        },
    );
    // The bed's ticket given back; the bell's chunk not named.
    commit(
        &east,
        2,
        Commit {
            claims: vec![(A, Vec::new())],
            ..Commit::default()
        },
    );
    // Two more of the bell's and the lectern's, whole.
    commit(
        &east,
        3,
        Commit {
            claims: vec![(
                B,
                vec![
                    claim(BELL, PoiType::Meeting, 28),
                    claim(lectern, PoiType::Librarian, 0),
                ],
            )],
            ..Commit::default()
        },
    );
    committed(&east, 3);
    east.flush();
    let again = opened(&store, EAST, 2);
    let a = load(&again, A);
    let b = load(&again, B);
    assert!(a.claims().is_empty());
    assert_eq!(a.free_tickets(in_chunk(HEAD)), Some(1));
    assert_eq!(b.free_tickets(in_chunk(BELL)), Some(28));
    assert_eq!(b.free_tickets(in_chunk(lectern)), Some(0));
    again.flush();
}

/// A chunk saved with its claims is loaded with them, by a store started again too.
#[test]
fn a_chunk_saved_with_claims_is_loaded_with_them_after_the_store_starts_again() {
    let disk = Arc::new(MemoryDisk::default());
    let mut chunk = generator().generate(A);
    let (x, z) = HEAD.in_chunk();
    chunk.set(x, HEAD.y, z, bed("head"));
    chunk.set(x, FOOT.y, z + 1, bed("foot"));
    assert!(chunk.acquire_ticket(in_chunk(HEAD)));
    {
        let store = store_on(&disk, &division()).unwrap();
        let east = opened(&store, EAST, 1);
        east.request(StoreRequest::Save {
            position: A,
            tick: 1,
            chunk: chunk.clone(),
            parts: clustine_rpc::Parts::BLOCKS,
        });
        assert_eq!(load(&east, A).claims(), chunk.claims());
        east.flush();
    }
    let left = Arc::new(disk.crashed(Survival::Everything));
    let store = store_on(&left, &division()).unwrap();
    let east = opened(&store, EAST, 2);
    let loaded = load(&east, A);
    assert_eq!(loaded.claims(), [claim(HEAD, PoiType::Home, 0)]);
    assert_eq!(loaded, chunk);
    east.flush();
}

/// A commit whose claims name a bed its own changes broke: the claim does not fit and
/// is dropped, and the chunk is kept with the commit's blocks.
#[test]
fn a_claim_on_a_block_its_commit_broke_is_dropped_and_the_chunk_kept() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let east = opened(&store, EAST, 1);
    commit(
        &east,
        1,
        Commit {
            changes: vec![(HEAD, bed("head")), (FOOT, bed("foot"))],
            claims: vec![(A, vec![claim(HEAD, PoiType::Home, 0)])],
            ..Commit::default()
        },
    );
    // Broken and a lectern placed where the head was, with the old claim named.
    commit(
        &east,
        2,
        Commit {
            changes: vec![(FOOT, blocks::AIR), (HEAD, blocks::LECTERN)],
            claims: vec![(A, vec![claim(HEAD, PoiType::Home, 0)])],
            ..Commit::default()
        },
    );
    committed(&east, 2);
    east.flush();
    let again = opened(&store, EAST, 2);
    let a = load(&again, A);
    let (x, z) = HEAD.in_chunk();
    assert_eq!(a.get(x, HEAD.y, z), Some(blocks::LECTERN));
    assert!(a.claims().is_empty());
    assert_eq!(a.free_tickets(in_chunk(HEAD)), Some(1));
    again.flush();
}

// Copies (plan, section 3.2; ADR-0025, "Changed in N10").

/// A chunk of region 0 of the world with a gap, which region 1 copies.
const WEST: ChunkPos = ChunkPos::new(-3, 0);

fn west_block(n: i32) -> BlockPos {
    BlockPos::new(WEST.x * 16 + n, 100, WEST.z * 16 + 1)
}

/// What is on `handle`'s stream of copies up to the answer to a flush.
fn copy_replies(handle: &StoreHandle) -> Vec<StoreReply> {
    told_before_flush(handle)
        .into_iter()
        .filter(|reply| {
            matches!(
                reply,
                StoreReply::CopyBase { .. } | StoreReply::CopyUpdate { .. }
            )
        })
        .collect()
}

/// The base of a copy comes on the stream when the thread for chunks has made it,
/// which a flush does not wait for: so this waits for it.
fn base(handle: &StoreHandle) -> (u64, Option<Chunk>) {
    loop {
        match handle.replies.recv().expect("the handle is kept") {
            StoreReply::CopyBase {
                sequence, chunk, ..
            } => return (sequence, chunk),
            StoreReply::CopyUpdate { .. } => panic!("an update before the base"),
            _ => {}
        }
    }
}

#[test]
fn a_copy_is_based_with_its_claims_and_updated_in_its_chain_by_commits_of_claims_alone() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let holder = opened(&store, 0, 1);
    let copier = opened(&store, 1, 1);
    let lectern = west_block(2);
    let bell = west_block(4);
    commit(
        &holder,
        1,
        Commit {
            changes: vec![(lectern, blocks::LECTERN), (bell, blocks::BELL)],
            claims: vec![(WEST, vec![claim(lectern, PoiType::Librarian, 0)])],
            ..Commit::default()
        },
    );
    told_before_flush(&holder);
    copier.request(StoreRequest::Copies {
        wanted: vec![WEST],
        gone: Vec::new(),
    });
    let (based, chunk) = base(&copier);
    let chunk = chunk.expect("the chunk is there");
    assert_eq!(chunk.claims(), [claim(lectern, PoiType::Librarian, 0)]);

    // Tick 2: a claim of the bell alone. Tick 3: a block alone. Tick 4: claims alone.
    commit(
        &holder,
        2,
        Commit {
            claims: vec![(
                WEST,
                vec![
                    claim(lectern, PoiType::Librarian, 0),
                    claim(bell, PoiType::Meeting, 31),
                ],
            )],
            ..Commit::default()
        },
    );
    commit(
        &holder,
        3,
        Commit {
            changes: vec![(west_block(9), blocks::STONE)],
            ..Commit::default()
        },
    );
    commit(
        &holder,
        4,
        Commit {
            claims: vec![(WEST, vec![claim(bell, PoiType::Meeting, 31)])],
            ..Commit::default()
        },
    );
    told_before_flush(&holder);
    let updates = copy_replies(&copier);
    let [
        StoreReply::CopyUpdate {
            position: first_at,
            sequence: first,
            previous: before_first,
            changes: first_changes,
            claims: first_claims,
            ..
        },
        StoreReply::CopyUpdate {
            sequence: second,
            previous: before_second,
            changes: second_changes,
            claims: second_claims,
            ..
        },
        StoreReply::CopyUpdate {
            previous: before_third,
            changes: third_changes,
            claims: third_claims,
            ..
        },
    ] = &updates[..]
    else {
        panic!("expected three updates, got {updates:?}");
    };
    assert_eq!(*first_at, WEST);
    assert_eq!(before_first, &based);
    assert!(first_changes.is_empty());
    assert_eq!(
        first_claims.as_deref(),
        Some(
            &[
                claim(lectern, PoiType::Librarian, 0),
                claim(bell, PoiType::Meeting, 31)
            ][..]
        )
    );
    assert_eq!(before_second, first);
    assert_eq!(second_changes, &[(west_block(9), blocks::STONE).into()]);
    assert_eq!(*second_claims, None, "the claims did not change");
    assert_eq!(before_third, second);
    assert!(third_changes.is_empty());
    assert_eq!(
        third_claims.as_deref(),
        Some(&[claim(bell, PoiType::Meeting, 31)][..])
    );
}

/// A commit's claims of a chunk nobody copies are no update to anyone.
#[test]
fn claims_of_a_chunk_nobody_copies_are_streamed_to_nobody() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let holder = opened(&store, 0, 1);
    let other = opened(&store, 1, 1);
    let lectern = west_block(2);
    commit(
        &holder,
        1,
        Commit {
            changes: vec![(lectern, blocks::LECTERN)],
            claims: vec![(WEST, vec![claim(lectern, PoiType::Librarian, 0)])],
            ..Commit::default()
        },
    );
    told_before_flush(&holder);
    assert_eq!(copy_replies(&other), []);
}

// Releases across a line (decision 2; section 3.6; effect kind 12).

/// A chunk of region 1 that it loads, and one it does not.
const USED: ChunkPos = ChunkPos::new(16, 0);
const UNUSED: ChunkPos = ChunkPos::new(17, 0);

fn release_in(chunk: ChunkPos, kind: PoiType) -> Effect {
    Effect {
        at: BlockPos::new(chunk.x * 16 + 3, -60, chunk.z * 16 + 3),
        what: Effected::Release { kind },
    }
}

fn effects_told(handle: &StoreHandle) -> Vec<FiledEffect> {
    told_before_flush(handle)
        .into_iter()
        .flat_map(|reply| match reply {
            StoreReply::Effects { effects } => effects,
            _ => Vec::new(),
        })
        .collect()
}

#[test]
fn a_release_into_a_chunk_its_holder_has_loaded_is_streamed_to_it_with_its_type() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let maker = opened(&store, 0, 1);
    let holder = opened(&store, 1, 1);
    load(&holder, USED);
    let release = release_in(USED, PoiType::Meeting);
    commit(
        &maker,
        1,
        Commit {
            effects: vec![release.clone()],
            ..Commit::default()
        },
    );
    told_before_flush(&maker);
    let streamed = effects_told(&holder);
    assert_eq!(streamed.len(), 1);
    assert_eq!(streamed[0].effect, release);

    // And it outlives the store, untaken.
    drop((maker, holder, store));
    let left = Arc::new(disk.crashed(Survival::Nothing));
    let store = store_on(&left, &gap()).unwrap();
    let holder = opened(&store, 1, 2);
    let streamed = effects_told(&holder);
    assert_eq!(
        streamed
            .iter()
            .map(|filed| &filed.effect)
            .collect::<Vec<_>>(),
        [&release]
    );
}

/// Section 3.6: "A release effect to land nobody holds is filed by the store and
/// applied when the chunk is next held (the effects' existing filing)"; decision 2:
/// "filed by the store for land nobody holds as every effect is". The existing filing
/// (ADR-0025, section 2) drops an effect into a chunk nobody uses.
#[test]
#[ignore = "finding: the plan says a release into land nobody holds is filed and applied when the chunk is next held; the store drops an effect into a chunk nobody uses (ADR-0025, section 2), so the ticket is never given back"]
fn a_release_into_land_its_holder_has_not_loaded_is_delivered_when_it_loads_it() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let maker = opened(&store, 0, 1);
    let holder = opened(&store, 1, 1);
    let release = release_in(UNUSED, PoiType::Home);
    commit(
        &maker,
        1,
        Commit {
            effects: vec![release.clone()],
            ..Commit::default()
        },
    );
    told_before_flush(&maker);
    assert_eq!(effects_told(&holder), []);
    load(&holder, UNUSED);
    let streamed = effects_told(&holder);
    assert_eq!(
        streamed
            .iter()
            .map(|filed| &filed.effect)
            .collect::<Vec<_>>(),
        [&release]
    );
}

// The margin's records in the store (section 3.5), by `Village` itself.

const W: RegionId = RegionId(0);
const E: RegionId = RegionId(1);

fn occupied_bed(x: i32, z: i32, occupied: bool) -> PoiSeen {
    PoiSeen {
        at: BlockPos::new(x * 16 + 8, 64, z * 16 + 8),
        kind: PoiType::Home,
        occupied,
    }
}

/// A report of `chunks`, each with one bed (occupied as said) or none.
fn report(full: bool, chunks: &[(i32, i32, Option<bool>)], let_go: &[(i32, i32)]) -> VillageReport {
    VillageReport {
        full,
        pois: chunks
            .iter()
            .map(|&(x, z, bed)| {
                (
                    ChunkPos::new(x, z),
                    bed.map(|occupied| vec![occupied_bed(x, z, occupied)])
                        .unwrap_or_default(),
                )
            })
            .collect(),
        let_go: let_go.iter().map(|&(x, z)| ChunkPos::new(x, z)).collect(),
    }
}

fn nothing_copied(_: ChunkPos) -> bool {
    false
}

fn chunks_of(published: &PublishedVillage) -> Vec<ChunkPos> {
    published
        .pois
        .iter()
        .map(|(position, _)| *position)
        .collect()
}

#[test]
fn nothing_is_published_to_a_region_before_its_full_report() {
    let mut village = Village::default();
    village.report(E, 1, report(true, &[(1, 0, Some(true))], &[]));
    assert_eq!(village.publication(W, 1, nothing_copied), None);
    // A partial report before any full one is no land.
    village.report(W, 1, report(false, &[(0, 0, None)], &[]));
    assert_eq!(village.publication(W, 1, nothing_copied), None);
    village.report(W, 1, report(true, &[(0, 0, None)], &[]));
    let first = village.publication(W, 1, nothing_copied).unwrap();
    assert!(first.whole);
    assert_eq!(chunks_of(&first), [ChunkPos::new(1, 0)]);
}

#[test]
fn a_region_is_published_the_chunks_within_five_of_its_land_and_no_farther() {
    let mut village = Village::default();
    village.report(W, 1, report(true, &[(0, 0, None)], &[]));
    let mut others = Vec::new();
    for (x, z) in [
        (5, 0),
        (6, 0),
        (-5, 5),
        (5, -6),
        (0, -5),
        (-6, 1),
        (5, 5),
        (3, -4),
    ] {
        others.push((x, z, Some(true)));
    }
    village.report(E, 1, report(true, &others, &[]));
    let published = village.publication(W, 1, nothing_copied).unwrap();
    let mut expected: Vec<ChunkPos> = [(5, 0), (-5, 5), (0, -5), (5, 5), (3, -4)]
        .into_iter()
        .map(|(x, z)| ChunkPos::new(x, z))
        .collect();
    expected.sort_unstable();
    let mut got = chunks_of(&published);
    got.sort_unstable();
    assert_eq!(got, expected);
}

#[test]
fn a_region_is_never_published_its_own_land_nor_the_chunks_it_copies() {
    let mut village = Village::default();
    village.report(W, 1, report(true, &[(0, 0, Some(true)), (1, 0, None)], &[]));
    village.report(
        E,
        1,
        report(true, &[(2, 0, Some(true)), (3, 0, Some(false))], &[]),
    );
    let copied = |position: ChunkPos| position == ChunkPos::new(2, 0);
    let published = village.publication(W, 1, copied).unwrap();
    assert_eq!(chunks_of(&published), [ChunkPos::new(3, 0)]);
    assert_eq!(
        published.pois[0].1,
        [occupied_bed(3, 0, false)],
        "a chunk's records whole, occupied or not"
    );
    // The east is published the west's bed, and not its own.
    let east = village.publication(E, 1, nothing_copied).unwrap();
    assert_eq!(chunks_of(&east), [ChunkPos::new(0, 0)]);
}

#[test]
fn after_the_first_publication_only_changes_are_published_and_nothing_when_none() {
    let mut village = Village::default();
    village.report(W, 1, report(true, &[(0, 0, None)], &[]));
    village.report(E, 1, report(true, &[(2, 0, Some(true)), (3, 0, None)], &[]));
    assert!(village.publication(W, 1, nothing_copied).unwrap().whole);
    assert_eq!(village.publication(W, 1, nothing_copied), None);

    // The bed freed, a bell placed in chunk 3.
    village.report(
        E,
        1,
        VillageReport {
            full: false,
            pois: vec![
                (ChunkPos::new(2, 0), vec![occupied_bed(2, 0, false)]),
                (
                    ChunkPos::new(3, 0),
                    vec![PoiSeen {
                        at: BlockPos::new(50, 64, 3),
                        kind: PoiType::Meeting,
                        occupied: false,
                    }],
                ),
            ],
            let_go: Vec::new(),
        },
    );
    let delta = village.publication(W, 1, nothing_copied).unwrap();
    assert!(!delta.whole);
    assert_eq!(
        chunks_of(&delta),
        [ChunkPos::new(2, 0), ChunkPos::new(3, 0)]
    );
    assert!(delta.dropped.is_empty());
    // The same report again changes nothing.
    village.report(E, 1, report(false, &[(2, 0, Some(false))], &[]));
    assert_eq!(village.publication(W, 1, nothing_copied), None);
}

#[test]
fn the_ring_follows_the_land_and_the_copies_as_they_change() {
    let mut village = Village::default();
    village.report(W, 1, report(true, &[(0, 0, None)], &[]));
    village.report(
        E,
        1,
        report(true, &[(6, 0, Some(true)), (2, 0, Some(true))], &[]),
    );
    let first = village.publication(W, 1, nothing_copied).unwrap();
    assert_eq!(chunks_of(&first), [ChunkPos::new(2, 0)]);

    // The west's land grows to chunk 1: chunk 6 comes within five.
    village.report(W, 1, report(false, &[(1, 0, None)], &[]));
    let grown = village.publication(W, 1, nothing_copied).unwrap();
    assert!(!grown.whole);
    assert_eq!(chunks_of(&grown), [ChunkPos::new(6, 0)]);

    // It copies chunk 2 now: dropped from its margin.
    let copied = |position: ChunkPos| position == ChunkPos::new(2, 0);
    let copying = village.publication(W, 1, copied).unwrap();
    assert_eq!(copying.dropped, [ChunkPos::new(2, 0)]);
    assert!(copying.pois.is_empty());
    // And no longer: published again.
    let again = village.publication(W, 1, nothing_copied).unwrap();
    assert_eq!(chunks_of(&again), [ChunkPos::new(2, 0)]);

    // It lets chunk 1 go: chunk 6 is beyond five again.
    village.report(W, 1, report(false, &[], &[(1, 0)]));
    let shrunk = village.publication(W, 1, nothing_copied).unwrap();
    assert_eq!(shrunk.dropped, [ChunkPos::new(6, 0)]);
}

#[test]
fn a_chunk_that_changes_hands_is_published_by_its_new_holder_s_word() {
    let mut village = Village::default();
    village.report(W, 1, report(true, &[(0, 0, None)], &[]));
    village.report(E, 1, report(true, &[(2, 0, Some(true))], &[]));
    village.report(RegionId(2), 1, report(true, &[(-9, 0, None)], &[]));
    village.publication(W, 1, nothing_copied).unwrap();

    // The east lets chunk 2 go: its records are kept meanwhile.
    village.report(E, 1, report(false, &[], &[(2, 0)]));
    assert_eq!(village.publication(W, 1, nothing_copied), None);
    // A third region takes it and says the bed is free.
    village.report(RegionId(2), 1, report(false, &[(2, 0, Some(false))], &[]));
    let delta = village.publication(W, 1, nothing_copied).unwrap();
    assert_eq!(
        delta.pois,
        [(ChunkPos::new(2, 0), vec![occupied_bed(2, 0, false)])]
    );
    // The west takes it in turn: its own land, dropped from its margin.
    village.report(W, 1, report(false, &[(2, 0, Some(false))], &[]));
    let own = village.publication(W, 1, nothing_copied).unwrap();
    assert_eq!(own.dropped, [ChunkPos::new(2, 0)]);
}

#[test]
fn an_opening_makes_the_next_publication_whole_and_an_old_owner_s_word_is_let_go() {
    let mut village = Village::default();
    village.report(W, 1, report(true, &[(0, 0, None)], &[]));
    village.report(E, 1, report(true, &[(2, 0, Some(true))], &[]));
    village.publication(W, 1, nothing_copied).unwrap();
    assert_eq!(village.publication(W, 1, nothing_copied), None);

    // A take-over: the east's new owner, session 2, reports whole; the old owner's
    // late partial report says the bed was freed, and is not taken.
    village.report(E, 2, report(true, &[(2, 0, Some(true))], &[]));
    village.report(E, 1, report(false, &[(2, 0, Some(false))], &[]));
    assert_eq!(village.publication(W, 1, nothing_copied), None);
    let east = village.publication(E, 2, nothing_copied).unwrap();
    assert!(east.whole, "whole to a new owner");
    // Nothing for the east's old session.
    assert_eq!(village.publication(E, 1, nothing_copied), None);

    // A merge or split within the west's session: its full report, then whole again.
    village.report(W, 1, report(true, &[(0, 0, None)], &[]));
    let whole = village.publication(W, 1, nothing_copied).unwrap();
    assert!(whole.whole);
    assert_eq!(
        whole.pois,
        [(ChunkPos::new(2, 0), vec![occupied_bed(2, 0, true)])]
    );
}

#[test]
fn a_store_started_again_knows_only_what_regions_reported_since() {
    let mut village = Village::default();
    village.report(W, 1, report(true, &[(0, 0, None)], &[]));
    // The east has not reported to this store: nothing of its land is known.
    let first = village.publication(W, 1, nothing_copied).unwrap();
    assert!(first.whole);
    assert!(first.pois.is_empty());
    village.report(E, 1, report(true, &[(2, 0, Some(true))], &[]));
    let next = village.publication(W, 1, nothing_copied).unwrap();
    assert_eq!(chunks_of(&next), [ChunkPos::new(2, 0)]);
}

// The margin through the store's requests and answers: once a game second, with the
// clock.

/// Region 0 of the world with a gap holds chunks west of x = 0, the home region 2 the
/// chunk at the origin.
#[test]
fn reports_are_published_behind_the_clock_to_the_regions_within_five_chunks() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let west = opened(&store, 0, 1);
    let home = opened(&store, 2, 1);
    west.request(StoreRequest::Village(report(
        true,
        &[(-1, 0, None), (-2, 0, None)],
        &[],
    )));
    home.request(StoreRequest::Village(report(
        true,
        &[(0, 0, Some(true))],
        &[],
    )));
    let villages = |handle: &StoreHandle| -> Vec<PublishedVillage> {
        told_before_flush(handle)
            .into_iter()
            .filter_map(|reply| match reply {
                StoreReply::Village(published) => Some(published),
                _ => None,
            })
            .collect()
    };
    // Nothing until the clock has moved a second on.
    told_before_flush(&home);
    let mut seen: Vec<PublishedVillage> = villages(&west);
    commit(
        &home,
        1,
        Commit {
            game_time: 1_000,
            ..Commit::default()
        },
    );
    committed(&home, 1);
    told_before_flush(&home);
    seen.extend(villages(&west));
    let whole: Vec<&PublishedVillage> = seen.iter().filter(|p| !p.pois.is_empty()).collect();
    assert_eq!(whole.len(), 1, "{seen:?}");
    assert!(whole[0].whole);
    assert_eq!(
        whole[0].pois,
        [(ChunkPos::new(0, 0), vec![occupied_bed(0, 0, true)])]
    );

    // The bed freed: a change, behind the next second's clock.
    home.request(StoreRequest::Village(report(
        false,
        &[(0, 0, Some(false))],
        &[],
    )));
    commit(
        &home,
        2,
        Commit {
            game_time: 1_020,
            ..Commit::default()
        },
    );
    committed(&home, 2);
    told_before_flush(&home);
    let later = villages(&west);
    assert_eq!(later.len(), 1, "{later:?}");
    assert!(!later[0].whole);
    assert_eq!(
        later[0].pois,
        [(ChunkPos::new(0, 0), vec![occupied_bed(0, 0, false)])]
    );

    // Nothing changed: nothing published behind the next clock.
    commit(
        &home,
        3,
        Commit {
            game_time: 1_040,
            ..Commit::default()
        },
    );
    committed(&home, 3);
    told_before_flush(&home);
    assert_eq!(villages(&west), []);
}
