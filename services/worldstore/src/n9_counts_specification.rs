//! The world store's part of natural spawning's counts, tested from the plan
//! (`docs/groundwork/n9-plan.md`, section 9.2 and decision 7 of section 18) by someone
//! who did not write it, through the store's own requests and answers: the counts are
//! kept by region and chunk, a full report replacing a region's set; each region is
//! published, behind the clock, the totals of the other regions' chunks; a lend's
//! "nothing" arriving after its borrower's report zeroes only the lender's entry; a
//! take-over that loads other chunks leaves nothing of the old owner's counted; a merge
//! and a split leave the counts right; and after a restart nothing is published until
//! every owned region has reported whole.
//!
//! The world is the one with a gap of `regions.rs` (`gap()`): region 0 west of x = 0,
//! region 1 from chunk x = 16 on, region 2 the home region with the home chunk.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use clustine_rpc::{
    CountsReport, PublishedCounts, SPAWN_CATEGORIES, SpawnCounts, StoreReply, StoreRequest,
    WorldClock,
};
use clustine_world::ChunkPos;

use crate::StoreHandle;
use crate::disk::{MemoryDisk, Survival};
use crate::regions::{absorb, checkpoint, claim, gap, hello_of, split, store_on};
use crate::tests::{any_reply, committed};

fn monsters(count: u32) -> SpawnCounts {
    let mut counts = [0; SPAWN_CATEGORIES];
    counts[0] = count;
    counts
}

/// A report of `chunks`, each with its monsters; whole where `full`.
fn report(full: bool, chunks: &[(i32, i32, u32)]) -> StoreRequest {
    StoreRequest::Counts(CountsReport {
        full,
        chunks: chunks
            .iter()
            .map(|(x, z, count)| (ChunkPos::new(*x, *z), monsters(*count)))
            .collect(),
    })
}

/// Commits the tick `tick` of `handle`'s region with the world clock at `game_time`,
/// and waits until it is done.
fn commit_at(handle: &StoreHandle, tick: u64, game_time: u64) {
    handle.request(StoreRequest::Commit {
        travellers: Vec::new(),
        state: format!("delta {tick}").into_bytes(),
        tick,
        changes: Vec::new(),
        stays: Vec::new(),
        ticks: Vec::new(),
        marks_taken: Vec::new(),
        seams: Vec::new(),
        effects: Vec::new(),
        taken: Vec::new(),
        events: Vec::new(),
        entity_changes: Vec::new(),
        arrivals: Vec::new(),
        arrivals_taken: Vec::new(),
        clock: WorldClock {
            game_time,
            day: game_time,
        },
        claims: Vec::new(),
        raids: Vec::new(),
    });
    committed(handle, tick);
}

/// What `handle`'s region is published of the others' counts behind the clock of
/// `game_time`, or `None` where nothing comes behind that clock.
fn published_behind(handle: &StoreHandle, game_time: u64) -> Option<PublishedCounts> {
    loop {
        if let StoreReply::Clock { clock, .. } = any_reply(handle)
            && clock.game_time == game_time
        {
            break;
        }
    }
    // The counts are answered right behind the clock, in the same turn.
    for _ in 0..50 {
        match handle.try_reply() {
            Some(StoreReply::Counts(counts)) => return Some(counts),
            Some(_) => return None,
            None => thread::sleep(Duration::from_millis(2)),
        }
    }
    None
}

/// The world clock of the publications, a second apart.
struct Clock {
    game_time: u64,
    tick: u64,
}

impl Clock {
    fn new() -> Self {
        Self {
            game_time: 1_000,
            tick: 10,
        }
    }

    /// A commit of `by`'s region a second on, which has the store publish: what each of
    /// `to` is published, in order.
    fn publish(&mut self, by: &StoreHandle, to: &[&StoreHandle]) -> Vec<Option<PublishedCounts>> {
        self.game_time += 20;
        self.tick += 1;
        commit_at(by, self.tick, self.game_time);
        to.iter()
            .map(|handle| published_behind(handle, self.game_time))
            .collect()
    }
}

fn totals(published: &Option<PublishedCounts>) -> Option<u32> {
    published.as_ref().map(|counts| counts.totals[0])
}

/// Three regions of the world with a gap, each opened with `epoch`.
fn three(store: &crate::Store, epoch: u64) -> [StoreHandle; 3] {
    [0, 1, 2].map(|region| {
        store
            .open_region(hello_of(&gap(), region, epoch))
            .unwrap()
            .0
    })
}

/// Section 9.2: nothing is published until every owned region has reported whole; then
/// each region is published the other regions' totals, its own left out.
#[test]
fn each_region_is_published_the_others_totals_once_every_region_has_reported_whole() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let [west, east, home] = three(&store, 1);
    let mut clock = Clock::new();
    west.request(report(true, &[(-3, 0, 4)]));
    east.request(report(true, &[(20, 0, 5)]));
    // A partial report counts for nothing before the whole one.
    home.request(report(false, &[(0, 0, 9)]));
    assert_eq!(clock.publish(&west, &[&west, &east]), [None, None]);
    home.request(report(true, &[(0, 0, 1)]));
    let [to_west, to_east, to_home] = &clock.publish(&west, &[&west, &east, &home])[..] else {
        unreachable!("three asked");
    };
    assert_eq!(totals(to_west), Some(6));
    assert_eq!(totals(to_east), Some(5));
    assert_eq!(totals(to_home), Some(9));
}

/// Section 9.2 and decision 7 (finding 4): a lend's or a return's "nothing" for a chunk
/// that arrives after the borrower's report of it zeroes only the lender's entry. The
/// borrower's count of the chunk stands, counted once.
#[test]
fn a_lend_s_nothing_after_the_borrower_s_report_zeroes_only_the_lender_s_entry() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let [west, east, home] = three(&store, 1);
    let mut clock = Clock::new();
    // The western region holds a chunk at the line with three monsters in it.
    west.request(report(true, &[(-1, 0, 3)]));
    east.request(report(true, &[]));
    home.request(report(true, &[]));
    assert_eq!(totals(&clock.publish(&west, &[&home])[0]), Some(3));
    // It is lent to the home region, whose report of it comes first, the lender's
    // "nothing" after it.
    home.request(report(false, &[(-1, 0, 3)]));
    west.request(report(false, &[(-1, 0, 0)]));
    let [to_west, to_east, to_home] = &clock.publish(&west, &[&west, &east, &home])[..] else {
        unreachable!("three asked");
    };
    assert_eq!(totals(to_east), Some(3), "counted once, the borrower's");
    assert_eq!(totals(to_west), Some(3), "the borrower's");
    assert_eq!(totals(to_home), Some(0), "the lender has none left");
}

/// Section 9.2 and decision 7: a region taken over by a new owner counts only from the
/// new owner's whole report, whatever chunks it loads; nothing of the old owner's is
/// counted, and nothing is published while the new owner has not reported.
#[test]
fn a_take_over_that_loads_other_chunks_leaves_nothing_of_the_old_owner_s_counted() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let [west, east, home] = three(&store, 1);
    let mut clock = Clock::new();
    west.request(report(true, &[]));
    east.request(report(true, &[(20, 0, 5)]));
    home.request(report(true, &[]));
    assert_eq!(totals(&clock.publish(&west, &[&west])[0]), Some(5));
    // The eastern region is taken over.
    let (taker, _) = store.open_region(hello_of(&gap(), 1, 2)).unwrap();
    // The old owner's late word is let go.
    east.request(report(false, &[(21, 0, 9)]));
    assert_eq!(clock.publish(&west, &[&west])[0], None, "not yet reported");
    taker.request(report(true, &[(30, 0, 2)]));
    assert_eq!(totals(&clock.publish(&west, &[&west])[0]), Some(2));
}

/// Section 9.2 and decision 7: a merge drops the absorbed region's set; the others are
/// published the survivor's alone, as it reports, and nothing of the absorbed region's.
#[test]
fn a_merge_leaves_the_counts_right() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let [west, east, home] = three(&store, 1);
    let mut clock = Clock::new();
    west.request(report(true, &[(-3, 0, 4)]));
    east.request(report(true, &[(20, 0, 5)]));
    home.request(report(true, &[]));
    assert_eq!(totals(&clock.publish(&home, &[&home])[0]), Some(9));
    clock.tick += 1;
    checkpoint(&east, clock.tick);
    clock.tick += 1;
    checkpoint(&west, clock.tick);
    clock.tick += 1;
    assert!(matches!(
        absorb(&west, 1, 1, clock.tick),
        StoreReply::Absorbed { .. }
    ));
    // The survivor reports whole after the merge, its own and what it took over.
    west.request(report(true, &[(-3, 0, 4), (20, 0, 5)]));
    assert_eq!(totals(&clock.publish(&home, &[&home])[0]), Some(9));
    // A monster of the absorbed region's chunk goes: the survivor's report counts.
    west.request(report(false, &[(20, 0, 4)]));
    assert_eq!(totals(&clock.publish(&home, &[&home])[0]), Some(8));
}

/// A world of three regions whose western one has split `goes` off as region 3, which
/// has been opened; the western region reported `goes` with three monsters and another
/// chunk with four before the split.
fn split_off(goes: ChunkPos) -> (crate::Store, [StoreHandle; 4], Clock) {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let [west, east, home] = three(&store, 1);
    let mut clock = Clock::new();
    west.request(report(true, &[(-3, 0, 4), (goes.x, goes.z, 3)]));
    east.request(report(true, &[]));
    home.request(report(true, &[]));
    assert_eq!(totals(&clock.publish(&home, &[&home])[0]), Some(7));
    // What the western region was told so far, read past.
    published_behind(&west, clock.game_time);
    assert_eq!(
        claim(&west, &[ChunkPos::new(5, 5)]).0,
        [ChunkPos::new(5, 5)]
    );
    clock.tick += 1;
    checkpoint(&west, clock.tick);
    clock.tick += 1;
    let id = crate::tests::first_part(&gap());
    assert_eq!(
        split(&west, clock.tick, &[goes], 4, id),
        StoreReply::Split {
            region: clustine_region::RegionId(id)
        }
    );
    let (part, _) = store.open_region(hello_of(&gap(), id, 4)).unwrap();
    (store, [west, east, home, part], clock)
}

/// Section 9.2 and decision 7: after a split the part and the parent each report whole,
/// in their first ticks after it, and the others are published both, each chunk once.
#[test]
fn a_split_leaves_the_counts_right_once_both_have_reported() {
    let goes = ChunkPos::new(-5, 3);
    let (_store, [west, _east, home, part], mut clock) = split_off(goes);
    west.request(report(true, &[(-3, 0, 4)]));
    part.request(report(true, &[(goes.x, goes.z, 3)]));
    assert_eq!(totals(&clock.publish(&home, &[&home])[0]), Some(7));
    // And on from there by their partial reports.
    part.request(report(false, &[(goes.x, goes.z, 1)]));
    assert_eq!(totals(&clock.publish(&home, &[&home])[0]), Some(5));
}

/// Section 9.2 and decision 7 (finding 4): "a region's set is dropped when it loses its
/// owner (a take-over) or is absorbed (a merge; a split's parent likewise)", so that
/// splits cannot race. A part that reports before its parent has reported again has the
/// chunk that went with it counted once, not in its parent's old set as well.
#[test]
fn a_split_s_part_reporting_before_its_parent_has_its_chunks_counted_once() {
    let goes = ChunkPos::new(-5, 3);
    let (_store, [_west, _east, home, part], mut clock) = split_off(goes);
    part.request(report(true, &[(goes.x, goes.z, 3)]));
    let counted = totals(&clock.publish(&home, &[&home])[0]);
    assert!(
        counted.is_none() || counted == Some(3) || counted == Some(7),
        "the chunk that went is counted once at most: {counted:?}"
    );
}

/// Section 9.2 and decision 7: the counts are kept in memory only; a store started again
/// publishes none until every owned region has sent its whole report, which each sends
/// in its first tick after its hello.
#[test]
fn nothing_is_published_after_a_store_restart_until_every_region_has_reported() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let [west, east, home] = three(&store, 1);
    let mut clock = Clock::new();
    west.request(report(true, &[(-3, 0, 4)]));
    east.request(report(true, &[(20, 0, 5)]));
    home.request(report(true, &[]));
    assert_eq!(totals(&clock.publish(&home, &[&home])[0]), Some(9));
    for handle in [&west, &east, &home] {
        handle.flush();
    }
    drop((west, east, home));
    drop(store);

    let left = Arc::new(disk.crashed(Survival::Everything));
    let store = store_on(&left, &gap()).unwrap();
    let [west, east, home] = three(&store, 2);
    // Past every tick the regions committed before.
    clock.tick += 100;
    west.request(report(true, &[(-3, 0, 4)]));
    assert_eq!(clock.publish(&home, &[&home])[0], None);
    home.request(report(true, &[]));
    assert_eq!(clock.publish(&home, &[&home])[0], None);
    east.request(report(true, &[(20, 0, 5)]));
    assert_eq!(totals(&clock.publish(&home, &[&home])[0]), Some(9));
}

/// The next publication of the others' counts to `handle`'s region, whatever clock it
/// comes behind: unlike `published_behind`, this needs no commit to have moved the clock.
fn next_published(handle: &StoreHandle) -> PublishedCounts {
    loop {
        if let StoreReply::Counts(counts) = any_reply(handle) {
            return counts;
        }
    }
}

/// The publications to `handle`'s region among the answers before the one to a flush it
/// asks for now, which come behind everything it asked before.
fn published_before_a_flush(handle: &StoreHandle) -> Vec<PublishedCounts> {
    handle.request(StoreRequest::Flush);
    let mut published = Vec::new();
    loop {
        match any_reply(handle) {
            StoreReply::Flushed => return published,
            StoreReply::Counts(counts) => published.push(counts),
            _ => {}
        }
    }
}

/// Section 9.2, as the store does it since a world in which nothing changed spawned
/// nothing: a world that commits nothing never moves its clock, and its regions must
/// still be published the others' counts once every one has reported whole, or none of
/// them ever spawns.
#[test]
fn regions_that_report_whole_and_never_commit_are_each_published_the_others_totals() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let [west, east, home] = three(&store, 1);
    west.request(report(true, &[(-3, 0, 4)]));
    east.request(report(true, &[(20, 0, 5)]));
    home.request(report(true, &[(0, 0, 1)]));
    assert_eq!(next_published(&west).totals[0], 6);
    assert_eq!(next_published(&east).totals[0], 5);
    assert_eq!(next_published(&home).totals[0], 9);
}

/// Section 9.2: what has the store publish where no commit moves the clock is a whole
/// report. A partial one is kept, but publishes nothing; the next whole report, of any
/// region, publishes it with the rest.
#[test]
fn a_partial_report_where_the_clock_has_not_moved_is_published_only_with_the_next_whole_one() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let [west, east, home] = three(&store, 1);
    west.request(report(true, &[(-3, 0, 4)]));
    east.request(report(true, &[(20, 0, 5)]));
    home.request(report(true, &[(0, 0, 1)]));
    assert_eq!(next_published(&west).totals[0], 6);
    assert_eq!(next_published(&east).totals[0], 5);
    assert_eq!(next_published(&home).totals[0], 9);
    home.request(report(false, &[(0, 0, 9)]));
    assert_eq!(
        published_before_a_flush(&home),
        [],
        "a partial report publishes nothing"
    );
    east.request(report(true, &[(20, 0, 6)]));
    assert_eq!(
        next_published(&west).totals[0],
        15,
        "the home region's partial report and the eastern region's whole one, together"
    );
    assert_eq!(next_published(&east).totals[0], 13);
}
