//! Tests of what the store keeps of the players, written from the record and not from
//! the code: `docs/adr/0020-one-stay-per-player.md`, sections 3, 5, 7 and 8, and the
//! row for the store under "Tests". `stays_kill.rs` has the store killed at every
//! point.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use clustine_format::{LogRecord, LoggedPlace, LoggedStay, PlayerRecord, PlayersFile, read_log};
use clustine_rpc::{ItemStack, Place, Pose, SplitPart, StayNote};
use clustine_world::{ChunkPos, EntityId, EntityIds, PlayerId, Vec3};
use tracing::Level;

use super::*;
use crate::disk::{Fault, MemoryDisk, OsDisk, Survival, replace};
use crate::players::{Players, Taken, Undo, Why};
use crate::regions::{SURVIVALS, checkpoint, claim, gap, give_back, put, store_on, table_file};
use crate::rest::held_twice;
use crate::tests::{HELD, Switched, division, generator, hello, open, switched_for};
use crate::unpinned::logged;

/// The regions of [`division`]: the eastern one holds the home chunk.
const WEST: u32 = 0;
const HOME: u32 = 1;

/// The regions of [`gap`]: two pinned ones, and the home region between them.
const GAP_WEST: u32 = 0;
const GAP_EAST: u32 = 1;
const GAP_HOME: u32 = 2;

/// A chunk in the gap of [`gap`], which nobody holds until it is claimed, and another.
const FREE: ChunkPos = ChunkPos::new(5, 5);
const OTHER: ChunkPos = ChunkPos::new(6, 5);

pub(crate) fn player(number: u128) -> PlayerId {
    // The id from its thirty-two hexadecimal digits: the store has no other use for
    // the crate that makes one from a number.
    let id = format!("{number:032x}").parse();
    PlayerId(id.expect("thirty-two hexadecimal digits are an id"))
}

/// A place at `(x, z)`, which tells it from every other place of these tests, with
/// something in every field.
pub(crate) fn place(x: f64, z: f64) -> Place {
    let mut hotbar = [None; 9];
    hotbar[0] = Some(ItemStack { item: 1, count: 64 });
    hotbar[8] = Some(ItemStack {
        item: 40,
        count: x as i32,
    });
    Place {
        pose: Pose {
            position: Vec3::new(x, 70.5, z),
            yaw: 135.25,
            pitch: -12.5,
            on_ground: z >= 0.0,
        },
        flying: z < 0.0,
        hotbar,
        selected_slot: 3,
    }
}

pub(crate) fn entering(player: PlayerId, entity: i32) -> StayNote {
    StayNote::Entering {
        player,
        entity: EntityId(entity),
    }
}

pub(crate) fn has(player: PlayerId, entity: i32, hops: u32, place: Place) -> StayNote {
    StayNote::Has {
        player,
        entity: EntityId(entity),
        hops,
        place,
    }
}

/// Commits `tick` with these notes and nothing else that tells it from another.
pub(crate) fn commit(handle: &StoreHandle, tick: u64, stays: Vec<StayNote>) {
    handle.request(StoreRequest::Commit {
        tick,
        changes: Vec::new(),
        state: format!("delta {tick}").into_bytes(),
        stays,
    });
}

/// Asks for a flush and returns what the handle was answered before it, in order. A
/// handle that is lost has no more answers, and what it had is returned.
pub(crate) fn answers(handle: &StoreHandle) -> Vec<StoreReply> {
    handle.request(StoreRequest::Flush);
    let mut answers = Vec::new();
    while let Ok(reply) = handle.replies.recv() {
        if reply == StoreReply::Flushed {
            break;
        }
        answers.push(reply);
    }
    answers
}

fn committed(tick: u64) -> StoreReply {
    StoreReply::Committed { tick }
}

fn enter(player: PlayerId, entity: i32, place: Option<Place>, holder: Option<u32>) -> StoreReply {
    StoreReply::Enter {
        player,
        entity: EntityId(entity),
        place,
        holder: holder.map(RegionId),
    }
}

fn dead(stays: &[(PlayerId, i32, u32)]) -> StoreReply {
    let stays = stays
        .iter()
        .map(|(player, stay, hops)| (*player, EntityId(*stay), *hops));
    StoreReply::Dead {
        stays: stays.collect(),
    }
}

/// The place as the file keeps it, worked out here and not by the store.
pub(crate) fn on_disk(place: &Place) -> LoggedPlace {
    let mut hotbar = [None; 9];
    for (slot, stack) in place.hotbar.iter().enumerate() {
        hotbar[slot] = stack.map(|stack| (stack.item, stack.count));
    }
    LoggedPlace {
        position: [
            place.pose.position.x,
            place.pose.position.y,
            place.pose.position.z,
        ],
        yaw: place.pose.yaw,
        pitch: place.pose.pitch,
        on_ground: place.pose.on_ground,
        flying: place.flying,
        selected_slot: place.selected_slot,
        hotbar,
    }
}

/// The note as the log keeps it, worked out here and not by the store.
pub(crate) fn note_on_disk(note: &StayNote) -> LoggedStay {
    match note {
        StayNote::Entering { player, entity } => LoggedStay::Entering {
            player: player.0.as_u128(),
            entity: entity.0,
        },
        StayNote::Has {
            player,
            entity,
            hops,
            place,
        } => LoggedStay::Has {
            player: player.0.as_u128(),
            entity: entity.0,
            hops: *hops,
            place: on_disk(place),
        },
    }
}

/// The record of `player` as the store is to have it.
pub(crate) fn kept(player: PlayerId, stay: i32, hops: u32, place: Option<Place>) -> PlayerRecord {
    PlayerRecord {
        player: player.0.as_u128(),
        stay,
        hops,
        place: place.as_ref().map(on_disk),
    }
}

/// What the table of section 3 makes of notes, written down a second time: the
/// records and the highest stay given, as a store that took these notes in this order
/// has them.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Model {
    pub(crate) issued: i32,
    pub(crate) records: BTreeMap<u128, (i32, u32, Option<LoggedPlace>)>,
}

impl Model {
    pub(crate) fn of(file: PlayersFile) -> Self {
        let records = file
            .records
            .into_iter()
            .map(|record| (record.player, (record.stay, record.hops, record.place)));
        Self {
            issued: file.issued,
            records: records.collect(),
        }
    }

    /// Takes a note of a commit of the home region, or of another.
    pub(crate) fn note(&mut self, home: bool, note: &LoggedStay) {
        match *note {
            LoggedStay::Entering { player, entity } => {
                let floor = self.records.get(&player).map_or(0, |record| record.0);
                if home && entity > floor {
                    let place = self.records.get(&player).and_then(|record| record.2);
                    self.records.insert(player, (entity, 0, place));
                    self.issued = self.issued.max(entity);
                }
            }
            LoggedStay::Has {
                player,
                entity,
                hops,
                place,
            } => {
                if let Some(record) = self.records.get_mut(&player)
                    && record.0 == entity
                    && hops >= record.1
                {
                    *record = (entity, hops, Some(place));
                }
            }
        }
    }

    /// As [`Store::players`] says it.
    pub(crate) fn kept(&self) -> (i32, Vec<PlayerRecord>) {
        let records = self
            .records
            .iter()
            .map(|(player, (stay, hops, place))| PlayerRecord {
                player: *player,
                stay: *stay,
                hops: *hops,
                place: *place,
            });
        (self.issued, records.collect())
    }
}

/// The segments of the log in the world on `disk`.
pub(crate) fn segments(disk: &dyn Disk) -> Vec<u64> {
    let names = disk.list(Path::new("/world/log")).unwrap();
    let mut segments: Vec<u64> = names
        .iter()
        .map(|name| name.strip_suffix(".wal").unwrap().parse().unwrap())
        .collect();
    segments.sort_unstable();
    segments
}

/// The players' file of the world on `disk`, if it has one.
pub(crate) fn players_file(disk: &dyn Disk) -> Option<PlayersFile> {
    let bytes = disk.read(Path::new("/world/regions/players")).unwrap();
    bytes.map(|bytes| PlayersFile::decode(&bytes).unwrap())
}

/// The records of the log in the world on `disk`, in its order.
fn records(disk: &dyn Disk) -> Vec<LogRecord> {
    let read = |segment: u64| {
        let path = format!("/world/log/{segment:020}.wal");
        let bytes = disk.read(Path::new(&path)).unwrap().unwrap();
        read_log(&bytes).unwrap().0
    };
    segments(disk).into_iter().flat_map(read).collect()
}

/// A store in memory for the two regions of [`division`], both opened: the home
/// region and the western one.
fn two() -> (Arc<MemoryDisk>, Store, StoreHandle, StoreHandle) {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let home = open(&store, hello(HOME, 1));
    let west = open(&store, hello(WEST, 1));
    (disk, store, home, west)
}

// The table of section 3, a test a row.

/// Row 2. The floor is raised, the place stays, the home region is answered behind
/// its commit, and every region that has an owner is told what is dead by it.
#[test]
fn an_entering_note_above_the_floor_raises_it_and_is_answered_with_the_place() {
    let (_disk, store, home, west) = two();
    let p = player(7);
    commit(&home, 1, vec![entering(p, 5)]);
    assert_eq!(
        answers(&home),
        [committed(1), enter(p, 5, None, None), dead(&[(p, 5, 0)])]
    );
    assert_eq!(answers(&west), [dead(&[(p, 5, 0)])]);
    assert_eq!(store.players(), (5, vec![kept(p, 5, 0, None)]));

    // The stay is written with hand-overs behind it and a place in the west.
    commit(&home, 2, vec![has(p, 5, 0, place(3.5, 8.0))]);
    commit(&west, 1, vec![has(p, 5, 2, place(-20.0, 8.0))]);
    assert_eq!(answers(&home), [committed(2)]);
    assert_eq!(answers(&west), [committed(1)]);
    assert_eq!(
        store.players(),
        (5, vec![kept(p, 5, 2, Some(place(-20.0, 8.0)))])
    );

    // The next stay begins with no hand-overs and at the place the last one had; the
    // answer names who holds the chunk of that place.
    commit(&home, 3, vec![entering(p, 9)]);
    let entered = enter(p, 9, Some(place(-20.0, 8.0)), Some(WEST));
    assert_eq!(answers(&home), [committed(3), entered, dead(&[(p, 9, 0)])]);
    assert_eq!(answers(&west), [dead(&[(p, 9, 0)])]);
    assert_eq!(
        store.players(),
        (9, vec![kept(p, 9, 0, Some(place(-20.0, 8.0)))])
    );
}

/// Row 3: a stay named again, as the home region does in its first tick after a
/// restore, is answered again and nobody else is told anything.
#[test]
fn an_entering_note_of_the_floor_is_answered_again_and_changes_nothing() {
    let (_disk, store, home, west) = two();
    let p = player(7);
    commit(&home, 1, vec![entering(p, 5)]);
    commit(&home, 2, vec![has(p, 5, 1, place(4.0, 4.0))]);
    answers(&home);
    answers(&west);
    let before = store.players();

    commit(&home, 3, vec![entering(p, 5)]);
    assert_eq!(
        answers(&home),
        [committed(3), enter(p, 5, Some(place(4.0, 4.0)), Some(HOME))]
    );
    assert_eq!(answers(&west), []);
    // Also not the hand-overs, which a floor that is raised begins anew.
    assert_eq!(store.players(), before);
    assert_eq!(before, (5, vec![kept(p, 5, 1, Some(place(4.0, 4.0)))]));
}

/// Row 4.
#[test]
fn an_entering_note_below_the_floor_is_answered_dead() {
    let (_disk, store, home, west) = two();
    let p = player(7);
    commit(&home, 1, vec![entering(p, 9)]);
    commit(&home, 2, vec![has(p, 9, 3, place(4.0, 4.0))]);
    answers(&home);
    answers(&west);
    let before = store.players();

    commit(&home, 3, vec![entering(p, 8)]);
    // With what the record has, below which every stay is dead.
    assert_eq!(answers(&home), [committed(3), dead(&[(p, 9, 3)])]);
    assert_eq!(answers(&west), []);
    assert_eq!(store.players(), before);
}

/// Row 1: only the home region gives stays out.
#[test]
fn an_entering_note_of_a_region_that_is_not_home_is_dropped_and_answered_dead() {
    let (_disk, store, home, west) = two();
    let (p, q) = (player(7), player(8));
    commit(&home, 1, vec![entering(p, 5)]);
    answers(&home);
    answers(&west);
    let before = store.players();

    // Above the floor, of the floor, and of a player the store knows nothing of.
    commit(
        &west,
        1,
        vec![entering(p, 9), entering(p, 5), entering(q, 3)],
    );
    assert_eq!(
        answers(&west),
        [committed(1), dead(&[(p, 5, 0), (p, 5, 0), (q, 0, 0)])]
    );
    assert_eq!(answers(&home), []);
    // No floor is raised, no record is made, and no id counts as given.
    assert_eq!(store.players(), before);
    assert_eq!(before, (5, vec![kept(p, 5, 0, None)]));
}

/// Row 5.
#[test]
fn a_has_note_of_the_living_stay_with_as_many_hops_or_more_is_kept() {
    let (_disk, store, home, west) = two();
    let p = player(7);
    commit(&home, 1, vec![entering(p, 5)]);
    answers(&home);
    answers(&west);

    commit(&home, 2, vec![has(p, 5, 0, place(1.0, 1.0))]);
    assert_eq!(answers(&home), [committed(2)]);
    assert_eq!(
        store.players(),
        (5, vec![kept(p, 5, 0, Some(place(1.0, 1.0)))])
    );
    // As many: the later note counts.
    commit(&home, 3, vec![has(p, 5, 0, place(2.0, 1.0))]);
    assert_eq!(answers(&home), [committed(3)]);
    assert_eq!(
        store.players(),
        (5, vec![kept(p, 5, 0, Some(place(2.0, 1.0)))])
    );
    // More, from another region.
    commit(&west, 1, vec![has(p, 5, 4, place(-3.0, -1.0))]);
    assert_eq!(answers(&west), [committed(1)]);
    assert_eq!(answers(&home), []);
    assert_eq!(
        store.players(),
        (5, vec![kept(p, 5, 4, Some(place(-3.0, -1.0)))])
    );
}

/// Row 6: the floor drops a record that comes late.
#[test]
fn a_has_note_of_an_earlier_stay_is_dropped_and_answered_dead() {
    let (_disk, store, home, west) = two();
    let p = player(7);
    commit(&home, 1, vec![entering(p, 5)]);
    commit(&home, 2, vec![has(p, 5, 1, place(-9.0, 2.0))]);
    commit(&home, 3, vec![entering(p, 6)]);
    commit(&home, 4, vec![has(p, 6, 0, place(1.0, 2.0))]);
    answers(&home);
    answers(&west);
    let before = store.players();

    // The region the earlier stay went to writes it, with more hand-overs than the
    // record has of any stay.
    commit(&west, 1, vec![has(p, 5, 7, place(-30.0, 2.0))]);
    assert_eq!(answers(&west), [committed(1), dead(&[(p, 6, 0)])]);
    assert_eq!(answers(&home), []);
    assert_eq!(store.players(), before);
    assert_eq!(before, (6, vec![kept(p, 6, 0, Some(place(1.0, 2.0)))]));
}

/// Row 7. That it is said in the log as an error is asserted in
/// `what_the_record_calls_an_error_is_said_in_the_log_and_nothing_else_is`.
#[test]
fn a_has_note_of_the_living_stay_with_fewer_hops_is_dropped_and_answered_dead() {
    let (_disk, store, home, west) = two();
    let p = player(7);
    commit(&home, 1, vec![entering(p, 5)]);
    commit(&home, 2, vec![has(p, 5, 2, place(-9.0, 2.0))]);
    answers(&home);
    answers(&west);
    let before = store.players();

    commit(&west, 1, vec![has(p, 5, 1, place(-30.0, 2.0))]);
    assert_eq!(answers(&west), [committed(1), dead(&[(p, 5, 2)])]);
    assert_eq!(answers(&home), []);
    assert_eq!(store.players(), before);
}

/// Row 8: it costs a place and not a player.
#[test]
fn a_has_note_of_a_stay_above_the_floor_is_dropped_and_not_answered() {
    let (_disk, store, home, west) = two();
    let (p, q) = (player(7), player(8));
    commit(&home, 1, vec![entering(p, 5)]);
    answers(&home);
    answers(&west);
    let before = store.players();

    // Of a player with a record, and of one without.
    let notes = vec![has(p, 6, 0, place(1.0, 1.0)), has(q, 3, 0, place(1.0, 1.0))];
    commit(&west, 1, notes.clone());
    commit(&home, 2, notes);
    assert_eq!(answers(&west), [committed(1)]);
    assert_eq!(answers(&home), [committed(2)]);
    assert_eq!(store.players(), before);
    assert_eq!(before, (5, vec![kept(p, 5, 0, None)]));
}

/// The three rows of the table that say "logs an error" do, once each, and the other
/// five say nothing at all. The store's own thread says it where no test can listen,
/// so the notes are taken here as that thread takes them.
#[test]
fn what_the_record_calls_an_error_is_said_in_the_log_and_nothing_else_is() {
    let p = player(7);
    let region = RegionId(3);
    // A player with the floor 5 and two hand-overs.
    let known = || {
        let mut players = Players::default();
        let mut undo = Undo::default();
        for note in [entering(p, 5), has(p, 5, 2, place(1.0, 1.0))] {
            players.take(region, true, &note_on_disk(&note), &mut undo);
        }
        players
    };
    let said = |home: bool, note: StayNote| {
        let mut players = known();
        let note = note_on_disk(&note);
        let (taken, lines) = logged(|| players.take(region, home, &note, &mut Undo::default()));
        (taken, lines)
    };
    let quiet = [
        (true, entering(p, 6)),
        (true, entering(p, 5)),
        (true, entering(p, 4)),
        (true, has(p, 5, 2, place(2.0, 2.0))),
        (false, has(p, 5, 3, place(2.0, 2.0))),
        (false, has(p, 4, 9, place(2.0, 2.0))),
    ];
    for (home, note) in quiet {
        let (_, lines) = said(home, note.clone());
        assert!(lines.is_empty(), "{note:?}: {lines:?}");
    }

    let errors = [
        (
            false,
            entering(p, 6),
            "a region that is not the home region named a stay as entering",
            Taken::Dead {
                stay: 5,
                hops: 2,
                why: Why::NotHome,
            },
        ),
        (
            false,
            has(p, 5, 1, place(2.0, 2.0)),
            "a region named the living stay of a player with fewer hand-overs",
            Taken::Dead {
                stay: 5,
                hops: 2,
                why: Why::Copy,
            },
        ),
        (
            true,
            has(p, 6, 0, place(2.0, 2.0)),
            "a region named a stay the home region never said it gave out",
            Taken::Ahead,
        ),
    ];
    for (home, note, begins, expected) in errors {
        let (taken, lines) = said(home, note.clone());
        assert_eq!(taken, expected, "{note:?}");
        assert_eq!(lines.len(), 1, "{lines:?}");
        let (level, line) = &lines[0];
        assert_eq!(*level, Level::ERROR, "{line}");
        assert!(line.starts_with(begins), "{line}");
        // The line names the region, the player and the stay.
        for field in [
            " region=3",
            " player=7",
            &format!(" entity={}", entity_of(&note)),
        ] {
            assert!(line.contains(field), "{line}");
        }
    }
}

fn entity_of(note: &StayNote) -> i32 {
    let (StayNote::Entering { entity, .. } | StayNote::Has { entity, .. }) = note;
    entity.0
}

/// Every order in which `items` can come.
fn orders<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
    if items.len() <= 1 {
        return vec![items.to_vec()];
    }
    let mut all = Vec::new();
    for first in 0..items.len() {
        let mut rest = items.to_vec();
        let item = rest.remove(first);
        for mut order in orders(&rest) {
            order.insert(0, item.clone());
            all.push(order);
        }
    }
    all
}

/// "So a later place is never overwritten by an earlier one, whatever order commits of
/// different regions arrive in": four notes of one stay, by two regions in turn, in
/// each of the twenty-four orders.
#[test]
fn the_commits_of_two_regions_in_every_order_leave_the_place_of_the_highest_hops() {
    let p = player(7);
    let notes: Vec<(u32, u32)> = vec![(HOME, 0), (WEST, 1), (HOME, 2), (WEST, 3)];
    let all = orders(&notes);
    assert_eq!(all.len(), 24);
    for order in all {
        let (disk, store, home, west) = two();
        commit(&home, 1, vec![entering(p, 5)]);
        answers(&home);
        answers(&west);
        let mut ticks = [1, 0];
        let mut highest = None;
        for (region, hops) in &order {
            let handle = if *region == HOME { &home } else { &west };
            let tick = &mut ticks[(*region == WEST) as usize];
            *tick += 1;
            commit(
                handle,
                *tick,
                vec![has(p, 5, *hops, place(f64::from(*hops), 0.0))],
            );
            // A note with fewer hand-overs than one before it is a copy, and dead.
            let expected = match highest {
                Some(highest) if *hops < highest => {
                    vec![committed(*tick), dead(&[(p, 5, highest)])]
                }
                _ => vec![committed(*tick)],
            };
            assert_eq!(answers(handle), expected, "{order:?}");
            highest = highest.max(Some(*hops));
        }
        let expected = (5, vec![kept(p, 5, 3, Some(place(3.0, 0.0)))]);
        assert_eq!(store.players(), expected, "{order:?}");
        // And so says a store that starts on what is on disk.
        let left = Arc::new(disk.crashed(Survival::Nothing));
        let again = store_on(&left, &division()).unwrap();
        assert_eq!(again.players(), expected, "{order:?}");
    }
}

// The answers.

/// `holder` is who holds the chunk of the place as the table is when the note is
/// taken: a pinned region, a region a chunk was granted to, or nobody.
#[test]
fn enter_names_who_holds_the_chunk_of_the_place_when_the_note_is_taken() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let home = open(&store, hello(GAP_HOME, 1));
    let east = open(&store, hello(GAP_EAST, 1));
    let p = player(7);
    // In the chunk at (5, 5), which is in the gap.
    let free = place(5.0 * 16.0 + 3.5, 5.0 * 16.0 + 9.0);
    commit(&home, 1, vec![entering(p, 5)]);
    commit(&home, 2, vec![has(p, 5, 0, free.clone())]);
    commit(&home, 3, vec![entering(p, 6)]);
    let replies = answers(&home);
    assert!(replies.contains(&enter(p, 5, None, None)), "{replies:?}");
    // Nobody holds it.
    let entered = enter(p, 6, Some(free.clone()), None);
    assert!(replies.contains(&entered), "{replies:?}");
    answers(&east);

    // The eastern region claims it; a stay named again is answered with the holder
    // of now.
    assert_eq!(claim(&east, &[FREE]).0, [FREE]);
    commit(&home, 4, vec![entering(p, 6)]);
    let entered = enter(p, 6, Some(free.clone()), Some(GAP_EAST));
    assert_eq!(answers(&home), [committed(4), entered]);

    // The home region itself, which holds the home chunk and is pinned to nothing;
    // and the pinned regions, which hold what nobody was granted of their areas.
    let places = [(3.0, GAP_HOME), (-40.0, GAP_WEST), (500.0, GAP_EAST)];
    for (tick, (x, holder)) in (5..).zip(places) {
        commit(
            &home,
            tick,
            vec![has(p, 6, 0, place(x, 3.0)), entering(p, 6)],
        );
        let entered = enter(p, 6, Some(place(x, 3.0)), Some(holder));
        assert_eq!(answers(&home), [committed(tick), entered], "{x}");
    }
}

/// "The store answers at once, always": whatever the region of the earlier stay does.
/// It has no owner; its owner never reads an answer; its owner is lost; it is taken
/// over. The login waits for the group of its own commit and for nothing else.
#[test]
fn an_entering_note_is_answered_at_once_whatever_the_other_regions_do() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let home = open(&store, hello(HOME, 1));
    let p = player(7);

    // The western region has never been opened.
    commit(&home, 1, vec![entering(p, 5)]);
    assert_eq!(
        answers(&home),
        [committed(1), enter(p, 5, None, None), dead(&[(p, 5, 0)])]
    );

    // It is open, has the earlier stay, and its owner reads nothing and commits
    // nothing: it hangs.
    let west = open(&store, hello(WEST, 1));
    commit(&west, 1, vec![has(p, 5, 1, place(-8.0, 1.0))]);
    assert_eq!(answers(&west), [committed(1)]);
    commit(&home, 2, vec![entering(p, 6)]);
    let entered = enter(p, 6, Some(place(-8.0, 1.0)), Some(WEST));
    assert_eq!(answers(&home), [committed(2), entered, dead(&[(p, 6, 0)])]);

    // It is being taken over: the handle of the owner before is lost, and the new
    // owner has asked for nothing yet.
    let taken_over = open(&store, hello(WEST, 2));
    assert!(west.is_lost());
    commit(&home, 3, vec![entering(p, 7)]);
    let entered = enter(p, 7, Some(place(-8.0, 1.0)), Some(WEST));
    assert_eq!(answers(&home), [committed(3), entered, dead(&[(p, 7, 0)])]);
    // What waited for the western region is there when its owner looks: the new
    // owner was told of the floor raised since it opened the region, and of no other.
    assert_eq!(answers(&taken_over), [dead(&[(p, 7, 0)])]);

    // Its owner is gone and nobody has it.
    drop(taken_over);
    commit(&home, 4, vec![entering(p, 8)]);
    let entered = enter(p, 8, Some(place(-8.0, 1.0)), Some(WEST));
    assert_eq!(answers(&home), [committed(4), entered, dead(&[(p, 8, 0)])]);
}

/// Means A of section 5: every region that has an owner when the floor is durable,
/// and no other. Two floors of one group are said together.
#[test]
fn dead_is_said_to_every_region_that_has_an_owner() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let home = open(&store, hello(GAP_HOME, 1));
    let west = open(&store, hello(GAP_WEST, 1));
    let (p, q) = (player(7), player(8));

    commit(&home, 1, vec![entering(p, 5), entering(q, 6)]);
    let floors = dead(&[(p, 5, 0), (q, 6, 0)]);
    assert_eq!(
        answers(&home),
        [
            committed(1),
            enter(p, 5, None, None),
            enter(q, 6, None, None),
            floors.clone()
        ]
    );
    assert_eq!(answers(&west), [floors]);

    // A region that is opened afterwards is told nothing of what was before: it
    // names its stays, and is answered.
    let east = open(&store, hello(GAP_EAST, 1));
    assert_eq!(answers(&east), []);
    commit(&home, 2, vec![entering(p, 9)]);
    answers(&home);
    for handle in [&west, &east] {
        assert_eq!(answers(handle), [dead(&[(p, 9, 0)])]);
    }
}

/// Means B: a region names every stay it has in its first tick after a restore, and
/// is told which are dead; the home region's entering stay is answered again. Also
/// by a store that was started anew in between, which tells nobody anything itself.
#[test]
fn a_region_that_names_its_stays_after_a_restore_is_told_which_are_dead() {
    let (disk, store, home, west) = two();
    let (p, q, r) = (player(7), player(8), player(9));
    commit(
        &home,
        1,
        vec![entering(p, 5), entering(q, 6), entering(r, 7)],
    );
    commit(
        &home,
        2,
        vec![
            has(p, 5, 1, place(-8.0, 1.0)),
            has(q, 6, 1, place(-9.0, 1.0)),
        ],
    );
    // The western region has both. Then both join again; one of the new stays is
    // handed on to the west once more.
    commit(
        &west,
        1,
        vec![
            has(p, 5, 1, place(-8.0, 1.0)),
            has(q, 6, 1, place(-9.0, 1.0)),
        ],
    );
    commit(&home, 3, vec![entering(p, 10), entering(q, 11)]);
    commit(&home, 4, vec![has(q, 11, 2, place(-9.0, 2.0))]);
    answers(&home);
    answers(&west);
    drop((home, west));

    let named = vec![
        // Dead: a later stay was given.
        has(p, 5, 1, place(-8.0, 1.0)),
        has(q, 6, 1, place(-9.0, 1.0)),
        // The living stay with as many hand-overs as the record has.
        has(q, 11, 2, place(-9.0, 3.0)),
    ];
    let told = [committed(2), dead(&[(p, 10, 0), (q, 11, 2)])];
    let west = open(&store, hello(WEST, 2));
    commit(&west, 2, named.clone());
    assert_eq!(answers(&west), told);
    // The home region holds one stay as entering still, and one that is no more.
    let home = open(&store, hello(HOME, 2));
    commit(
        &home,
        5,
        vec![entering(p, 10), entering(r, 7), entering(q, 6)],
    );
    let entered = [
        committed(5),
        enter(p, 10, Some(place(-8.0, 1.0)), Some(WEST)),
        enter(r, 7, None, None),
        dead(&[(q, 11, 2)]),
    ];
    assert_eq!(answers(&home), entered);
    // Naming raised no floor, so nobody else heard of it.
    assert_eq!(answers(&west), []);

    // The same of a store that starts on what is on disk.
    for survival in SURVIVALS {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &division()).unwrap();
        let west = open(&store, hello(WEST, 3));
        // Nobody is told anything by a start.
        assert_eq!(answers(&west), [], "{survival:?}");
        commit(&west, 3, named.clone());
        let told = [committed(3), dead(&[(p, 10, 0), (q, 11, 2)])];
        assert_eq!(answers(&west), told, "{survival:?}");
        let home = open(&store, hello(HOME, 3));
        commit(
            &home,
            6,
            vec![entering(p, 10), entering(r, 7), entering(q, 6)],
        );
        let mut again = entered.clone();
        again[0] = committed(6);
        assert_eq!(answers(&home), again, "{survival:?}");
    }
}

/// Whoever runs a region may be in another process: the notes, both answers and
/// `issued` cross the connection as they are.
#[test]
fn notes_and_their_answers_cross_a_connection() {
    let store = Store::memory_divided(generator(), division()).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let server = serve(store.clone(), listener).unwrap();
    let address = server.local_addr().to_string();
    let (home, restored) = StoreHandle::connect(&address, hello(HOME, 1)).unwrap();
    assert_eq!(restored.issued, EntityId(0));
    let (west, _) = StoreHandle::connect(&address, hello(WEST, 1)).unwrap();
    let p = player(7);

    commit(&home, 1, vec![entering(p, 5)]);
    assert_eq!(
        answers(&home),
        [committed(1), enter(p, 5, None, None), dead(&[(p, 5, 0)])]
    );
    assert_eq!(answers(&west), [dead(&[(p, 5, 0)])]);
    commit(&west, 1, vec![has(p, 5, 1, place(-8.5, -3.25))]);
    assert_eq!(answers(&west), [committed(1)]);
    commit(&home, 2, vec![entering(p, 6)]);
    let entered = enter(p, 6, Some(place(-8.5, -3.25)), Some(WEST));
    assert_eq!(answers(&home), [committed(2), entered, dead(&[(p, 6, 0)])]);
    assert_eq!(
        store.players(),
        (6, vec![kept(p, 6, 0, Some(place(-8.5, -3.25)))])
    );

    drop(home);
    let (_, restored) = StoreHandle::connect(&address, hello(HOME, 2)).unwrap();
    assert_eq!(restored.issued, EntityId(6));
    server.stop();
}

// A note is applied only for a commit that is taken.

#[test]
fn a_commit_of_an_owner_that_was_replaced_leaves_no_note() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let first = open(&store, hello(HOME, 1));
    let p = player(7);
    commit(&first, 1, vec![entering(p, 5)]);
    answers(&first);
    let second = open(&store, hello(HOME, 2));
    let before = (5, vec![kept(p, 5, 0, None)]);

    commit(
        &first,
        2,
        vec![
            entering(p, 9),
            entering(player(8), 10),
            has(p, 5, 0, place(1.0, 1.0)),
        ],
    );
    assert_eq!(answers(&first), []);
    // The owner there is hears nothing of it either.
    assert_eq!(answers(&second), []);
    assert_eq!(store.players(), before);
    // Nor is it in the log, for a store that starts to find.
    let with_notes =
        |record: &LogRecord| matches!(record, LogRecord::Commit { stays, .. } if !stays.is_empty());
    assert_eq!(
        records(disk.as_ref())
            .iter()
            .filter(|record| with_notes(record))
            .count(),
        1
    );
    for survival in SURVIVALS {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &division()).unwrap();
        assert_eq!(store.players(), before, "{survival:?}");
    }
}

/// Section 8, step 3: a commit with a tick above what a later opening of its region
/// was restored up to is no part of the region's history, and neither are its notes.
/// A running store writes no such log; a crash that undid a cut does, so it is written
/// by hand here.
#[test]
fn a_commit_that_an_opening_passes_over_leaves_no_note() {
    let (p, q) = (player(7), player(8));
    let noted = |region: u32, tick: u64, epoch: u64, stays: &[StayNote]| LogRecord::Commit {
        region,
        tick,
        epoch,
        changes: Vec::new(),
        state: format!("delta {tick}").into_bytes(),
        stays: stays.iter().map(note_on_disk).collect(),
    };
    let world = |restored: u64| {
        let disk = Arc::new(MemoryDisk::default());
        // A world with its table and nothing else.
        drop(store_on(&disk, &division()).unwrap());
        let log: Vec<LogRecord> = vec![
            noted(HOME, 1, 1, &[entering(p, 5)]),
            noted(HOME, 2, 1, &[entering(q, 6), has(p, 5, 0, place(1.0, 1.0))]),
            noted(WEST, 2, 1, &[has(p, 5, 1, place(-2.0, 1.0))]),
            LogRecord::Opened {
                region: HOME,
                epoch: 2,
                restored,
            },
            noted(HOME, 2, 2, &[has(p, 5, 0, place(3.0, 1.0))]),
        ];
        let bytes: Vec<u8> = log.iter().flat_map(LogRecord::encode).collect();
        put(&disk, "/world/log/00000000000000000001.wal", &bytes);
        disk
    };

    // Restored up to tick 1: the home region's first commit of tick 2 is passed over,
    // with the stay it gave out and the place it wrote. The western region's commit of
    // the same tick is not, and neither is what the home region committed afterwards,
    // which names fewer hand-overs than the west did and is dropped for that.
    let store = store_on(&world(1), &division()).unwrap();
    assert_eq!(
        store.players(),
        (5, vec![kept(p, 5, 1, Some(place(-2.0, 1.0)))])
    );
    // Restored up to tick 2: nothing is passed over.
    let store = store_on(&world(2), &division()).unwrap();
    let expected = vec![kept(p, 5, 1, Some(place(-2.0, 1.0))), kept(q, 6, 0, None)];
    assert_eq!(store.players(), (6, expected));
}

// Rule P, and when the file is written.

/// The test the record names (section 8, rule P), with its assertion that no segment
/// is left. Nothing changes the table here, so the table file's `from` stays where a
/// new world has it, below the players': the line for the table in `load` does not
/// stand in for the one for the players.
#[test]
fn a_store_that_is_stopped_with_no_segment_left_and_started_twice_has_every_note_of_the_session_between()
 {
    let (p, q, r) = (player(1), player(2), player(3));
    let disk = Arc::new(MemoryDisk::default());
    {
        let store = store_on(&disk, &gap()).unwrap();
        let home = open(&store, hello(GAP_HOME, 1));
        let west = open(&store, hello(GAP_WEST, 1));
        commit(&home, 1, vec![entering(p, 5), entering(q, 6)]);
        commit(
            &home,
            2,
            vec![
                has(p, 5, 0, place(1.0, 1.0)),
                has(q, 6, 1, place(-20.0, 1.0)),
            ],
        );
        commit(&west, 1, vec![has(q, 6, 1, place(-21.0, 1.0))]);
        answers(&home);
        answers(&west);
        assert_eq!(segments(disk.as_ref()), [1]);
        assert_eq!(players_file(disk.as_ref()), None);
        assert_eq!(table_file(&disk).from, 1);

        // A runner that is stopped checkpoints and flushes.
        for (handle, tick) in [(&home, 2), (&west, 1)] {
            handle.request(StoreRequest::Checkpoint {
                tick,
                state: b"state".to_vec(),
            });
            handle.flush();
        }
        drop((home, west));
        store.flush().unwrap();
    }
    assert_eq!(segments(disk.as_ref()), Vec::<u64>::new());
    let first = (
        6,
        vec![
            kept(p, 5, 0, Some(place(1.0, 1.0))),
            kept(q, 6, 1, Some(place(-21.0, 1.0))),
        ],
    );
    let file = players_file(disk.as_ref()).expect("the file is what let the segment go");
    assert_eq!((file.issued, file.records.clone()), first);
    assert_eq!(file.from, 2);
    assert_eq!(table_file(&disk).from, 1);

    // The session between, started on nothing but files. The store was stopped and
    // not killed, so the segments it removed are gone: what was on its disk is there.
    let second = Arc::new(disk.crashed(Survival::Everything));
    assert_eq!(segments(second.as_ref()), Vec::<u64>::new());
    {
        let store = store_on(&second, &gap()).unwrap();
        assert_eq!(store.players(), first);
        let home = open(&store, hello(GAP_HOME, 2));
        let west = open(&store, hello(GAP_WEST, 2));
        commit(&home, 3, vec![entering(p, 9)]);
        commit(
            &home,
            4,
            vec![has(p, 9, 0, place(2.0, 2.0)), entering(r, 10)],
        );
        commit(&west, 2, vec![has(q, 6, 2, place(-22.0, 1.0))]);
        answers(&home);
        answers(&west);
        // No segment is numbered below the file's `from`.
        let written = segments(second.as_ref());
        assert!(!written.is_empty());
        assert!(
            written.iter().all(|segment| *segment >= file.from),
            "{written:?}"
        );
    }

    // Killed, and started again: the records are those of the last notes, and the
    // floors and the highest stay are the session's.
    let session = (
        10,
        vec![
            kept(p, 9, 0, Some(place(2.0, 2.0))),
            kept(q, 6, 2, Some(place(-22.0, 1.0))),
            kept(r, 10, 0, None),
        ],
    );
    for survival in SURVIVALS {
        let third = Arc::new(second.crashed(survival));
        let store = store_on(&third, &gap()).unwrap();
        assert_eq!(store.players(), session, "{survival:?}");
        let (home, restored) = store.open_region(hello(GAP_HOME, 3)).unwrap();
        assert_eq!(restored.issued, EntityId(10), "{survival:?}");
        commit(&home, 5, vec![entering(p, 5), entering(r, 10)]);
        assert_eq!(
            answers(&home),
            [committed(5), enter(r, 10, None, None), dead(&[(p, 9, 0)])],
            "{survival:?}"
        );
    }
}

/// The players' file is looked at after the table file and not before (second
/// review, finding 10): a first segment that is kept for the table and for the
/// players alike is freed of the table first, and then goes. Looked at before, it
/// would be found kept for more than the players, and would stay after a clean stop.
#[test]
fn a_first_segment_kept_for_the_table_and_for_the_players_goes_at_a_clean_stop() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let home = open(&store, hello(GAP_HOME, 1));
    let west = open(&store, hello(GAP_WEST, 1));
    let p = player(7);
    assert_eq!(claim(&west, &[FREE]).0, [FREE]);
    commit(&home, 1, vec![entering(p, 5)]);
    commit(&west, 1, vec![has(p, 5, 1, place(-3.0, 0.0))]);
    answers(&home);
    answers(&west);
    for (handle, tick) in [(&home, 1), (&west, 1)] {
        handle.request(StoreRequest::Checkpoint {
            tick,
            state: b"state".to_vec(),
        });
        handle.flush();
    }
    assert_eq!(segments(disk.as_ref()), Vec::<u64>::new());
    assert_eq!(table_file(&disk).from, 2);
    let file = players_file(disk.as_ref()).unwrap();
    assert_eq!(file.from, 2);
    assert_eq!(file.records, [kept(p, 5, 1, Some(place(-3.0, 0.0)))]);
}

/// The file is written only where it lets a segment go, and what it then stands for
/// is not kept in the log; the notes written afterwards are, until the file is
/// written next.
#[test]
fn the_players_file_is_written_when_only_the_notes_keep_the_first_segment() {
    let (disk, store, home, west) = two();
    let p = player(7);
    commit(&home, 1, vec![entering(p, 5)]);
    commit(&west, 1, vec![has(p, 5, 1, place(-3.0, 0.0))]);
    answers(&home);
    answers(&west);

    // The home region's checkpoint does not free the segment: the western region has
    // a commit in it. No file is written.
    home.request(StoreRequest::Checkpoint {
        tick: 1,
        state: b"home".to_vec(),
    });
    home.flush();
    assert_eq!(segments(disk.as_ref()), [1]);
    assert_eq!(players_file(disk.as_ref()), None);
    commit(&home, 2, vec![has(p, 5, 2, place(4.0, 0.0))]);
    answers(&home);
    assert_eq!(segments(disk.as_ref()), [1, 2]);

    // The western region's does: the first segment is kept for its notes alone. The
    // file has what every note so far made of the records, also those of the second
    // segment, which stays for the home region's commit.
    west.request(StoreRequest::Checkpoint {
        tick: 1,
        state: b"west".to_vec(),
    });
    west.flush();
    let file = players_file(disk.as_ref()).unwrap();
    assert_eq!(file.from, 3);
    assert_eq!(file.issued, 5);
    assert_eq!(file.records, [kept(p, 5, 2, Some(place(4.0, 0.0)))]);
    assert_eq!(segments(disk.as_ref()), [2]);

    // A note behind the file is in a segment the file names, and counts at a start
    // on top of what the file has; the second segment's notes are not taken twice.
    commit(&home, 3, vec![entering(p, 6)]);
    answers(&home);
    assert_eq!(segments(disk.as_ref()), [2, 3]);
    for survival in SURVIVALS {
        let left = Arc::new(disk.crashed(survival));
        let again = store_on(&left, &division()).unwrap();
        let expected = (6, vec![kept(p, 6, 0, Some(place(4.0, 0.0)))]);
        assert_eq!(again.players(), expected, "{survival:?}");
    }
    assert_eq!(store.players().0, 6);
}

// Entity ids never go back.

/// What the runner is to make of the store's block, of the next id of the state it
/// restored and of `issued` (section 7, step 3), written down here to hold the store
/// to what that rule needs of it.
fn next_id(block: EntityIds, state_next: i32, issued: EntityId) -> i32 {
    let inside = |id: i32| block.first.0 <= id && id < block.end.0;
    let next = if inside(state_next) {
        state_next
    } else {
        block.first.0
    };
    if inside(issued.0) {
        next.max(issued.0 + 1)
    } else {
        next
    }
}

fn block(index: u32) -> EntityIds {
    EntityIds::block(index).unwrap()
}

/// Section 7, step 1, with `end <= issued + 1` and not `end <= issued`: a block whose
/// last id is the highest stay has no id left. And step 2: the opener is told
/// `issued`. Between the new block and the next checkpoint every restore names the
/// same block and the same highest stay, so that the runner's rule gives the same
/// next id each time.
#[test]
fn a_block_whose_last_id_is_the_highest_stay_is_given_anew_and_is_the_same_at_every_restore() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let (p, q) = (player(7), player(8));
    let last = block(0).end.0 - 1;

    let (home, restored) = store.open_region(hello(HOME, 1)).unwrap();
    assert_eq!(
        (restored.entity_ids, restored.issued),
        (block(0), EntityId(0))
    );
    // The last id but one: the block has one id above it, and is kept.
    commit(&home, 1, vec![entering(p, last - 1)]);
    answers(&home);
    let (home, restored) = store.open_region(hello(HOME, 2)).unwrap();
    assert_eq!(
        (restored.entity_ids, restored.issued),
        (block(0), EntityId(last - 1))
    );
    // The last id, and a checkpoint, whose state names the old block from now on.
    commit(&home, 2, vec![entering(q, last)]);
    home.request(StoreRequest::Checkpoint {
        tick: 2,
        state: b"a state that names block 0".to_vec(),
    });
    home.flush();

    // The home region is given the next block, which no region file names.
    let (home, restored) = store.open_region(hello(HOME, 3)).unwrap();
    assert_eq!(
        (restored.entity_ids, restored.issued),
        (block(1), EntityId(last))
    );
    let state_next = block(0).end.0;
    assert_eq!(
        next_id(restored.entity_ids, state_next, restored.issued),
        block(1).first.0
    );
    // A join in the new block, in a delta: the state on disk still names the old one.
    commit(&home, 3, vec![entering(p, block(1).first.0)]);
    answers(&home);
    drop(home);
    let issued = EntityId(block(1).first.0);
    let same = |restored: &Restored, case: &str| {
        assert_eq!(
            (restored.entity_ids, restored.issued),
            (block(1), issued),
            "{case}"
        );
        assert_eq!(
            restored.state.as_ref().map(|state| state.tick),
            Some(2),
            "{case}"
        );
        assert_eq!(restored.deltas.len(), 1, "{case}");
        // Whether the runner goes by the state's next id, which is outside the block,
        // or by the delta's, which is the one after the join.
        for state_next in [5, block(1).first.0 + 1] {
            let next = next_id(restored.entity_ids, state_next, restored.issued);
            assert_eq!(next, block(1).first.0 + 1, "{case}");
        }
    };
    for epoch in 4..7 {
        let (_, restored) = store.open_region(hello(HOME, epoch)).unwrap();
        same(&restored, &format!("epoch {epoch}"));
    }
    for survival in SURVIVALS {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &division()).unwrap();
        for epoch in 7..9 {
            let (_, restored) = store.open_region(hello(HOME, epoch)).unwrap();
            same(&restored, &format!("{survival:?}, epoch {epoch}"));
        }
        // No other region is given the new block: it is in the home region's file.
        let (_, west) = store.open_region(hello(WEST, 1)).unwrap();
        assert_eq!(west.entity_ids, block(2), "{survival:?}");
        assert_eq!(west.issued, issued, "{survival:?}");
    }
}

/// Only the home region gives stays out, so only its block has to lie above them: a
/// pinned region keeps a block below the highest stay.
#[test]
fn a_region_that_is_not_home_keeps_its_block_whatever_was_given_out() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let (_, west) = store.open_region(hello(WEST, 1)).unwrap();
    let (home, restored) = store.open_region(hello(HOME, 1)).unwrap();
    assert_eq!((west.entity_ids, restored.entity_ids), (block(0), block(1)));
    commit(&home, 1, vec![entering(player(7), block(1).first.0 + 4)]);
    answers(&home);
    for epoch in 2..4 {
        let (_, west) = store.open_region(hello(WEST, epoch)).unwrap();
        assert_eq!(west.entity_ids, block(0));
        assert_eq!(west.issued, EntityId(block(1).first.0 + 4));
        let (_, home) = store.open_region(hello(HOME, epoch)).unwrap();
        assert_eq!(home.entity_ids, block(1));
    }
}

/// A world that is started with another division has another home region, whose
/// block can be a lower one. The records are kept through the making over, and the
/// home region's next stay is above every floor.
#[test]
fn a_world_started_with_another_division_gives_the_home_region_ids_above_every_floor() {
    let far = ChunkPos::new(20, 0);
    let stripes = Division::side_by_side(far, &[0, 16]).unwrap();
    let disk = Arc::new(MemoryDisk::default());
    let (p, q) = (player(7), player(8));
    let kept_place = place(20.0 * 16.0 + 2.0, 4.0);
    let given = {
        let store = store_on(&disk, &stripes).unwrap();
        // Opened from west to east: the home region, the third, has the third block.
        let _others = [open(&store, hello(0, 1)), open(&store, hello(1, 1))];
        let (home, restored) = store.open_region(hello(2, 1)).unwrap();
        assert_eq!(restored.entity_ids, block(2));
        let given = block(2).first.0 + 5;
        commit(&home, 1, vec![entering(p, given - 1), entering(q, given)]);
        commit(&home, 2, vec![has(p, given - 1, 0, kept_place.clone())]);
        answers(&home);
        store.flush().unwrap();
        given
    };
    let records = vec![
        kept(p, given - 1, 0, Some(kept_place.clone())),
        kept(q, given, 0, None),
    ];

    for survival in SURVIVALS {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &Division::open(far)).unwrap();
        assert_eq!(store.players(), (given, records.clone()), "{survival:?}");
        // The one region there is now is region 0, whose file names the first block.
        let (home, restored) = store.open_region(hello(0, 2)).unwrap();
        assert_eq!(restored.issued, EntityId(given), "{survival:?}");
        assert_eq!(restored.entity_ids, block(3), "{survival:?}");
        assert!(restored.entity_ids.first.0 > given, "{survival:?}");
        // The player joins again in place, with a higher id.
        let next = next_id(restored.entity_ids, 0, restored.issued);
        commit(&home, 1, vec![entering(p, next)]);
        let entered = enter(p, next, Some(kept_place.clone()), Some(0));
        assert_eq!(
            answers(&home),
            [committed(1), entered, dead(&[(p, next, 0)])],
            "{survival:?}"
        );
    }
}

// A group that fails.

/// The records as they are before a group fails in these tests: one player, a place.
fn before(p: PlayerId) -> (i32, Vec<PlayerRecord>) {
    (5, vec![kept(p, 5, 0, Some(place(1.0, 1.0)))])
}

/// The notes of a group that is to fail: they would raise a floor, write a place for
/// the new stay, and make a record for a second player.
fn doomed(p: PlayerId, q: PlayerId) -> Vec<StayNote> {
    vec![
        entering(p, 9),
        has(p, 9, 0, place(2.0, 2.0)),
        entering(q, 10),
    ]
}

/// Opens the home region and the western one of a world with a gap, and has the home
/// region commit what makes the records of [`before`].
fn opened_before(store: &Store, p: PlayerId) -> (StoreHandle, StoreHandle) {
    let home = open(store, hello(GAP_HOME, 1));
    let west = open(store, hello(GAP_WEST, 1));
    // Before any floor is raised, so that the answer to the claim is the next one.
    assert_eq!(claim(&west, &[FREE]).0, [FREE]);
    commit(&home, 1, vec![entering(p, 5)]);
    commit(&home, 2, vec![has(p, 5, 0, place(1.0, 1.0))]);
    answers(&home);
    answers(&west);
    assert_eq!(store.players(), before(p));
    (home, west)
}

/// After a group with the notes of [`doomed`] has failed: the records are as they
/// were before it, with no record for the second player and nothing added to the ids
/// given out, and the notes are in no log. Then the same ids are given again and
/// taken, and the paths into `fail_log` that come with an empty group (a hello and a
/// split that cannot be written) undo nothing of that: nothing is undone twice.
fn undone_once(store: &Store, disk: &Switched, p: PlayerId, q: PlayerId, case: &str) {
    assert_eq!(store.players(), before(p), "{case}");
    // The hello is answered once what was cut off the log is durably gone. Until
    // then a crash can bring the group back, never answered, as a store that starts
    // would count it; from then on none can.
    let (home, restored) = store.open_region(hello(GAP_HOME, 2)).unwrap();
    assert_eq!(restored.issued, EntityId(5), "{case}");
    for survival in SURVIVALS {
        let left = Arc::new(disk.disk.crashed(survival));
        let again = store_on(&left, &gap()).unwrap();
        assert_eq!(again.players(), before(p), "{case}, {survival:?}");
    }
    let tick = restored.deltas.last().map_or(0, |delta| delta.tick);
    assert_eq!(tick, 3, "{case}");
    // The id is given again, and the answer has the place of before the group.
    commit(&home, tick + 1, vec![entering(p, 9), entering(q, 10)]);
    assert_eq!(
        answers(&home),
        [
            committed(tick + 1),
            enter(p, 9, Some(place(1.0, 1.0)), Some(GAP_HOME)),
            enter(q, 10, None, None),
            dead(&[(p, 9, 0), (q, 10, 0)]),
        ],
        "{case}"
    );
    commit(&home, tick + 2, vec![has(q, 10, 0, place(3.0, 3.0))]);
    assert_eq!(answers(&home), [committed(tick + 2)], "{case}");
    let after = (
        10,
        vec![
            kept(p, 9, 0, Some(place(1.0, 1.0))),
            kept(q, 10, 0, Some(place(3.0, 3.0))),
        ],
    );
    assert_eq!(store.players(), after, "{case}");

    // A hello whose record cannot be written.
    let fails: fn(&LogRecord) -> bool = |record| matches!(record, LogRecord::Opened { .. });
    *disk.failing_records.lock().unwrap() = Some(fails);
    assert!(store.open_region(hello(GAP_WEST, 9)).is_err(), "{case}");
    *disk.failing_records.lock().unwrap() = None;
    assert!(home.is_lost(), "{case}");
    assert_eq!(store.players(), after, "{case}");

    // A split that cannot be written, of a region that has nothing uncheckpointed.
    let home = open(store, hello(GAP_HOME, 3));
    assert_eq!(claim(&home, &[OTHER]).0, [OTHER]);
    checkpoint(&home, tick + 3);
    let fails: fn(&LogRecord) -> bool = |record| matches!(record, LogRecord::Split { .. });
    *disk.failing_records.lock().unwrap() = Some(fails);
    let next = store.regions().unwrap().next;
    home.request(StoreRequest::SplitCommit {
        tick: tick + 4,
        state: b"rest".to_vec(),
        part: SplitPart {
            chunks: vec![OTHER],
            state: b"part".to_vec(),
        },
        as_epoch: 1,
        region: next,
    });
    assert_eq!(answers(&home), [], "{case}");
    assert!(home.is_lost(), "{case}");
    *disk.failing_records.lock().unwrap() = None;
    assert_eq!(store.players(), after, "{case}");
    for survival in SURVIVALS {
        let left = Arc::new(disk.disk.crashed(survival));
        let again = store_on(&left, &gap()).unwrap();
        assert_eq!(again.players(), after, "{case}, {survival:?}");
    }
}

/// Holds the sync of a commit of `home` without notes, so that what is asked for
/// until [`let_go`] is one group behind it.
fn hold(disk: &Switched, home: &StoreHandle, tick: u64) {
    disk.holding_syncs.store(true, Ordering::SeqCst);
    commit(home, tick, Vec::new());
    disk.held.wait();
}

fn let_go(disk: &Switched) {
    disk.held.wait();
}

/// The path of the sync at the end of a group.
#[test]
fn a_group_whose_sync_fails_takes_its_notes_its_new_records_and_issued_back() {
    let (store, disk) = switched_for(&gap());
    let (p, q) = (player(7), player(8));
    let (home, west) = opened_before(&store, p);

    hold(&disk, &home, 3);
    commit(&home, 4, doomed(p, q));
    // Decided for the syncs that begin from now on: the one that is held goes through.
    disk.failing_syncs.store(true, Ordering::SeqCst);
    let_go(&disk);
    // Nothing of the group was answered: not the commit, not `Enter`, and no region
    // was told that anything is dead.
    assert_eq!(answers(&home), [committed(3)]);
    assert_eq!(answers(&west), []);
    assert!(home.is_lost() && west.is_lost());
    disk.failing_syncs.store(false, Ordering::SeqCst);
    undone_once(&store, &disk, p, q, "sync");
}

/// The path of a commit that cannot be written, behind a commit with notes in the
/// same group.
#[test]
fn a_group_in_which_a_commit_cannot_be_written_takes_its_notes_back() {
    let (store, disk) = switched_for(&gap());
    let (p, q) = (player(7), player(8));
    let (home, west) = opened_before(&store, p);

    let fails: fn(&LogRecord) -> bool =
        |record| matches!(record, LogRecord::Commit { tick: 99, .. });
    *disk.failing_records.lock().unwrap() = Some(fails);
    hold(&disk, &home, 3);
    commit(&home, 4, doomed(p, q));
    commit(&west, 99, Vec::new());
    let_go(&disk);
    assert_eq!(answers(&home), [committed(3)]);
    assert_eq!(answers(&west), []);
    assert!(home.is_lost() && west.is_lost());
    *disk.failing_records.lock().unwrap() = None;
    undone_once(&store, &disk, p, q, "commit");
}

/// The path of a grant that cannot be written.
#[test]
fn a_group_in_which_a_grant_cannot_be_written_takes_its_notes_back() {
    let (store, disk) = switched_for(&gap());
    let (p, q) = (player(7), player(8));
    let (home, west) = opened_before(&store, p);

    let fails: fn(&LogRecord) -> bool = |record| matches!(record, LogRecord::Granted { .. });
    *disk.failing_records.lock().unwrap() = Some(fails);
    hold(&disk, &home, 3);
    commit(&home, 4, doomed(p, q));
    west.request(StoreRequest::Claim {
        chunks: vec![ChunkPos::new(7, 7)],
    });
    let_go(&disk);
    assert_eq!(answers(&home), [committed(3)]);
    assert_eq!(answers(&west), []);
    assert!(home.is_lost() && west.is_lost());
    *disk.failing_records.lock().unwrap() = None;
    undone_once(&store, &disk, p, q, "grant");
}

/// The path of a return that cannot be written. The return comes back from the
/// thread for chunks, which is held until the commit with the notes waits for the
/// commit thread, and is known to have sent it when it is held a second time.
#[test]
fn a_group_in_which_a_return_cannot_be_written_takes_its_notes_back() {
    let (store, disk, generating) = held_twice(&gap());
    let (p, q) = (player(7), player(8));
    let (home, west) = opened_before(&store, p);
    let east = open(&store, hello(GAP_EAST, 1));
    assert!(gap().pinned[GAP_EAST as usize].contains(HELD));
    // The opening is durable by itself, so that the sync held below is the commit's:
    // a hello is answered before the sync of its record.
    assert_eq!(answers(&east), []);

    let fails: fn(&LogRecord) -> bool = |record| matches!(record, LogRecord::Returned { .. });
    *disk.failing_records.lock().unwrap() = Some(fails);
    // The thread for chunks is busy; behind what keeps it are the return and what
    // will keep it once more.
    east.request(StoreRequest::Load { position: HELD });
    generating.wait();
    give_back(&west, &[FREE]);
    east.request(StoreRequest::Load { position: HELD });
    hold(&disk, &home, 3);
    commit(&home, 4, doomed(p, q));
    // The thread for chunks goes on, says that the return's saves are durable, and
    // is held again: the commit thread has that word waiting behind the commit.
    generating.wait();
    generating.wait();
    let_go(&disk);
    assert_eq!(answers(&home), [committed(3)]);
    assert!(home.is_lost() && west.is_lost());
    generating.wait();
    *disk.failing_records.lock().unwrap() = None;
    // The chunk is the western region's still, as before the group.
    let (_, restored) = store.open_region(hello(GAP_WEST, 2)).unwrap();
    assert_eq!(restored.held.len(), 1);
    undone_once(&store, &disk, p, q, "return");
}

// A world that is made over.

/// A world of [`division`] with records of which some are in the players' file and
/// some only in the log, and what the records are.
pub(crate) fn lived_in() -> (Arc<MemoryDisk>, (i32, Vec<PlayerRecord>)) {
    let (disk, store, home, west) = two();
    let (p, q, r) = (player(1), player(2), player(3));
    commit(&home, 1, vec![entering(p, 5), entering(q, 6)]);
    commit(
        &home,
        2,
        vec![
            has(p, 5, 0, place(1.0, 1.0)),
            has(q, 6, 1, place(-4.0, 1.0)),
        ],
    );
    commit(&west, 1, vec![has(q, 6, 1, place(-5.0, 1.0))]);
    for (handle, tick) in [(&home, 2), (&west, 1)] {
        handle.request(StoreRequest::Checkpoint {
            tick,
            state: b"state".to_vec(),
        });
        handle.flush();
    }
    assert!(players_file(disk.as_ref()).is_some());
    // Behind the file, and not checkpointed.
    commit(&home, 3, vec![entering(p, 7), entering(r, 8)]);
    commit(&home, 4, vec![has(r, 8, 0, place(9.0, 9.0))]);
    commit(&west, 2, vec![has(q, 6, 2, place(-6.0, 1.0))]);
    answers(&home);
    answers(&west);
    let records = store.players();
    let expected = vec![
        kept(p, 7, 0, Some(place(1.0, 1.0))),
        kept(q, 6, 2, Some(place(-6.0, 1.0))),
        kept(r, 8, 0, Some(place(9.0, 9.0))),
    ];
    assert_eq!(records, (8, expected));
    drop((home, west));
    store.flush().unwrap();
    (disk, records)
}

/// Section 8, step 5: making a world over writes that every region was restored with
/// nothing, by which the start after it would forget every note since the file there
/// is. So the file is written first, and if it cannot be, the start fails with that
/// error and nothing is made over.
#[test]
fn the_players_file_that_cannot_be_written_before_a_make_over_fails_the_start() {
    let (world, records) = lived_in();
    let other = gap();
    let before = (
        segments(world.as_ref()),
        players_file(world.as_ref()),
        table_file(&world),
    );
    // The file is the first thing such a start changes on disk: it is written under
    // another name, synced, renamed, and its directory is synced.
    for n in 1..=4 {
        for fault in [Fault::Fail(n), Fault::Stop(n)] {
            let disk = Arc::new(world.crashed(Survival::Everything).with(fault));
            let failed = store_on(&disk, &other);
            assert!(matches!(failed, Err(StoreError::Io(_))), "{fault:?}");
            // Nothing was made over: the log and the table are as they were.
            assert_eq!(segments(disk.as_ref()), before.0, "{fault:?}");
            assert_eq!(table_file(&disk), before.2, "{fault:?}");
            for survival in SURVIVALS {
                let case = format!("{fault:?}, {survival:?}");
                let left = Arc::new(disk.crashed(survival));
                // The old file is the one there is until the new one has its name.
                if n < 4 {
                    assert_eq!(players_file(left.as_ref()), before.1, "{case}");
                }
                // A start for the division the world has loses nothing, and neither
                // does one that makes it over after all.
                let as_it_was = Arc::new(left.crashed(Survival::Everything));
                let store = store_on(&as_it_was, &division()).unwrap();
                assert_eq!(store.players(), records, "{case}");
                let store = store_on(&left, &other).unwrap();
                assert_eq!(store.players(), records, "{case}");
            }
        }
    }
    // Without a fault the file is written with everything, and names a segment that
    // no note was written to.
    let disk = Arc::new(world.crashed(Survival::Everything));
    let store = store_on(&disk, &other).unwrap();
    assert_eq!(store.players(), records);
    let file = players_file(disk.as_ref()).unwrap();
    assert_eq!((file.issued, file.records), records);
    assert!(before.0.iter().all(|segment| *segment < file.from));
}

// What the file costs.

/// The measurement step R1.1 asks for ("Risks", and "What is measured"): the players'
/// file with 10,000 records, written whole on the commit thread, during which no
/// region's commit is taken. Prints how long writing it takes on the disk the test
/// runs on, by itself and as a store does it at a checkpoint.
///
/// `cargo test -p clustine-worldstore --release -- --ignored --nocapture ten_thousand`
#[test]
#[ignore = "a measurement, which prints what it found"]
fn the_players_file_with_ten_thousand_records_is_written_in_this_time() {
    const PLAYERS: u128 = 10_000;
    // With something in every slot of the hotbar, which is the longest a record gets.
    let full = |x: f64| {
        let mut place = place(x, 3.0);
        place.hotbar = [Some(ItemStack { item: 9, count: 64 }); 9];
        place
    };
    let noted = |from: u128, to: u128| -> Vec<Vec<StayNote>> {
        let players: Vec<u128> = (from..to).collect();
        let batches = players.chunks(500).map(|batch| {
            let entering = batch
                .iter()
                .map(|number| entering(player(*number), *number as i32 + 1));
            let has = batch
                .iter()
                .map(|number| has(player(*number), *number as i32 + 1, 0, full(*number as f64)));
            entering.chain(has).collect()
        });
        batches.collect()
    };
    let middle = |mut times: Vec<Duration>| {
        times.sort();
        (times[0], times[times.len() / 2], times[times.len() - 1])
    };

    // By itself: what `write_players` does, on a real file system.
    let mut players = Players::default();
    for note in noted(0, PLAYERS).iter().flatten() {
        players.apply(&note_on_disk(note), true, None);
    }
    let directory = tempfile::tempdir().unwrap();
    let disk = OsDisk::default();
    let path = directory.path().join("players");
    let mut encoding = Vec::new();
    let mut writing = Vec::new();
    let mut bytes = 0;
    for _ in 0..20 {
        let started = Instant::now();
        let file = players.file(7).encode();
        encoding.push(started.elapsed());
        bytes = file.len();
        let started = Instant::now();
        replace(&disk, &path, &file).unwrap();
        disk.sync_directory(directory.path()).unwrap();
        writing.push(started.elapsed());
    }
    println!("the players' file with {PLAYERS} records is {bytes} bytes");
    println!(
        "encoding it, of 20 times (least, middle, most): {:?}",
        middle(encoding)
    );
    println!(
        "writing it, syncing it, renaming it and syncing the directory: {:?}",
        middle(writing)
    );

    // As a store does it: a commit with a note, a checkpoint and a flush, which puts
    // a state file in place, closes the segment and writes the players' file to let
    // the segment go. Once with no records but one, once with ten thousand.
    for count in [1, PLAYERS] {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::local(directory.path(), generator()).unwrap();
        let home = open(&store, hello(0, 1));
        let mut tick = 0;
        for notes in noted(0, count) {
            tick += 1;
            commit(&home, tick, notes);
        }
        answers(&home);
        let mut rounds = Vec::new();
        for _ in 0..20 {
            tick += 1;
            let started = Instant::now();
            commit(&home, tick, vec![has(player(0), 1, 0, full(tick as f64))]);
            home.request(StoreRequest::Checkpoint {
                tick,
                state: b"state".to_vec(),
            });
            home.flush();
            rounds.push(started.elapsed());
        }
        let file = fs_players(directory.path());
        assert_eq!(file.records.len() as u128, count);
        assert_eq!(file.records[0].place, Some(on_disk(&full(tick as f64))));
        println!(
            "a commit, a checkpoint and a flush with {count} records, of 20 times (least, middle, most): {:?}",
            middle(rounds)
        );
    }
}

/// The players' file of a world in a directory of the local file system.
fn fs_players(root: &Path) -> PlayersFile {
    let bytes = std::fs::read(root.join("regions/players")).unwrap();
    PlayersFile::decode(&bytes).unwrap()
}
