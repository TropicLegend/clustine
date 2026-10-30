//! Tests of the store's part in a player leaving the world on a mount, written from
//! `docs/groundwork/n7h-logout-on-a-mount.md` (sections 4, 8 and 10, as sections 12 to
//! 14 amend them), ADR-0020's table of notes (section 3) and ADR-0028's arrivals (section
//! 5) by someone who did not write the code.
//!
//! What the design promises, and every test here checks after each of its steps: "every
//! durable state has the unit in exactly one of: a chunk, a filed arrival, a record". It
//! is read from a store started again on what the disk has, never from the running one.
//!
//! The world of [`division`]: the western region 0 west of x = 0, the home region 1 east
//! of it with the home chunk at the origin. A player's stay 5 was given by the home
//! region and let go to the west, where they rode a boat with a pig behind them in chunk
//! (-1, 0); the boat and the pig are the unit, its members having been handed on three
//! times.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use clustine_format::{LogRecord, LoggedStay, read_log};
use clustine_rpc::{Parts, Place, StayNote, StoreReply, StoreRequest, Unit};
use clustine_world::{Arrival, ChunkPos, Entity, EntityChange, EntityId, PlayerId, Seat};

use crate::disk::{Disk, MemoryDisk, Survival};
use crate::entities_tests::item;
use crate::regions::{SURVIVALS, gap, put, store_on};
use crate::stays::{entering, has, note_on_disk, place, player, players_file, segments};
use crate::tests::{Switched, division, generator, hello, open, switched_for};
use crate::{Division, Store, StoreHandle};

/// The regions of [`division`].
const WEST: u32 = 0;
const HOME: u32 = 1;

/// The regions of [`gap`]: two pinned ones, and the home region, which holds the origin
/// alone; the land between them is nobody's.
const GAP_WEST: u32 = 0;
const GAP_HOME: u32 = 2;

/// The chunk the unit stands in, the western region's.
const ROOT_CHUNK: ChunkPos = ChunkPos::new(-1, 0);
/// The home chunk, the home region's.
const HOME_CHUNK: ChunkPos = ChunkPos::new(0, 0);
/// In the world of [`gap`], a chunk nobody holds.
const FREE: ChunkPos = ChunkPos::new(5, 5);

/// The stay the player left on, and the one the home region gives them next.
const STAY: u64 = 5;
const NEXT: u64 = 9;
/// The hand-overs behind the leaving stay: from the home region to the west.
const STAY_HOPS: u32 = 1;
/// The hand-overs behind each member of the unit when it left.
const HOPS: u32 = 3;

/// The members of the unit: the boat and the pig behind the player.
const BOAT: u64 = 40;
const PIG: u64 = 41;
/// The members of a second unit, which the record cannot take while it has the first.
const OTHER_BOAT: u64 = 50;
const OTHER_PIG: u64 = 51;

/// A member of a unit at `x`, `z`, with [`HOPS`].
fn member(key: u64, x: f64, z: f64) -> Entity {
    let mut entity = item(key, x, z);
    entity.hops = HOPS;
    entity
}

/// A unit of `root` and `pig` at `x`, `z`, its seats naming the player's stay [`STAY`]
/// first, as the leave left them ("the unit's seat for the player stays as it is:
/// seated, naming the leaving stay's key"), and the pig behind.
fn unit_of(root: u64, pig: u64, x: f64, z: f64) -> Unit {
    let mut boat = member(root, x, z);
    boat.riding.passengers = vec![
        Seat {
            key: EntityId(STAY),
            player: true,
            pending: None,
        },
        Seat {
            key: EntityId(pig),
            player: false,
            pending: None,
        },
    ];
    let mut animal = member(pig, x, z);
    animal.riding.vehicle = Some(EntityId(root));
    Unit(vec![boat, animal])
}

/// The unit in the west's chunk (-1, 0).
fn the_unit() -> Unit {
    unit_of(BOAT, PIG, -10.5, 0.5)
}

/// Another unit in the same chunk.
fn other_unit() -> Unit {
    unit_of(OTHER_BOAT, OTHER_PIG, -12.5, 0.5)
}

fn keys_of(unit: &Unit) -> Vec<u64> {
    unit.0.iter().map(|member| member.key.0).collect()
}

/// The `Has` of `stay` with `hops` at `place`, carrying `unit`.
fn has_unit(p: PlayerId, stay: u64, hops: u32, place: Place, unit: Unit) -> StayNote {
    let StayNote::Has {
        player,
        entity,
        hops,
        place,
        face,
        let_go,
        ..
    } = has(p, stay, hops, place)
    else {
        unreachable!("`has` makes a `Has`")
    };
    StayNote::Has {
        player,
        entity,
        hops,
        place,
        face,
        let_go,
        unit: Some(unit),
    }
}

fn took(p: PlayerId, root: u64) -> StayNote {
    StayNote::Took {
        player: p,
        root: EntityId(root),
    }
}

/// The home region's commit that takes `unit` out of `p`'s record and puts it back as
/// arrivals into its root's chunk, the player's seat naming the stay [`NEXT`]
/// (section 5, steps 1 to 3, and section 14 item 3), with `stays` before the taking.
fn take_back(handle: &StoreHandle, tick: u64, p: PlayerId, unit: &Unit, mut stays: Vec<StayNote>) {
    let mut members = unit.0.clone();
    members[0].riding.passengers[0].key = EntityId(NEXT);
    let chunk = unit.root().chunk();
    stays.push(took(p, unit.root().key.0));
    let arrivals = members.into_iter().map(|member| (chunk, member)).collect();
    commit(handle, tick, stays, Vec::new(), arrivals);
}

/// Commits `tick` with these notes, entity changes and arrivals sent, and nothing else
/// that tells it from another.
fn commit(
    handle: &StoreHandle,
    tick: u64,
    stays: Vec<StayNote>,
    entity_changes: Vec<(ChunkPos, EntityChange)>,
    arrivals: Vec<(ChunkPos, Entity)>,
) {
    handle.request(StoreRequest::Commit {
        travellers: Vec::new(),
        tick,
        changes: Vec::new(),
        state: format!("delta {tick}").into_bytes(),
        stays,
        ticks: Vec::new(),
        marks_taken: Vec::new(),
        seams: Vec::new(),
        effects: Vec::new(),
        taken: Vec::new(),
        events: Vec::new(),
        entity_changes,
        arrivals,
        arrivals_taken: Vec::new(),
        clock: Default::default(),
        claims: Vec::new(),
        raids: Vec::new(),
    });
}

/// Commits `tick` with these notes and nothing else.
fn noted(handle: &StoreHandle, tick: u64, stays: Vec<StayNote>) {
    commit(handle, tick, stays, Vec::new(), Vec::new());
}

/// The unit comes to be in its root's chunk, every member whole there.
fn put_in_world(handle: &StoreHandle, tick: u64, unit: &Unit) {
    let chunk = unit.root().chunk();
    let whole = unit
        .0
        .iter()
        .map(|member| (chunk, EntityChange::Whole(member.clone())))
        .collect();
    commit(handle, tick, Vec::new(), whole, Vec::new());
}

/// The commit of the leave as the design has the sim write it (section 3): every member
/// `Gone` from the root's chunk, the removal note carrying the unit, then `Gone`.
fn leave(handle: &StoreHandle, tick: u64, p: PlayerId, stay: u64, hops: u32, unit: &Unit) {
    let chunk = unit.root().chunk();
    let gone = unit
        .0
        .iter()
        .map(|member| (chunk, EntityChange::Gone(member.key)))
        .collect();
    let stays = vec![
        has_unit(p, stay, hops, place(-9.0, 1.0), unit.clone()),
        StayNote::Gone {
            player: p,
            entity: EntityId(stay),
            hops,
        },
    ];
    commit(handle, tick, stays, gone, Vec::new());
}

/// Everything `handle` is told until a flush asked for now is answered, but for the
/// faces, the marks, the answers to commits and that chunks are being made.
fn told(handle: &StoreHandle) -> Vec<StoreReply> {
    handle.request(StoreRequest::Flush);
    let mut told = Vec::new();
    while let Ok(reply) = handle.replies.recv() {
        match reply {
            StoreReply::Flushed => break,
            StoreReply::Faces { .. }
            | StoreReply::Mark { .. }
            | StoreReply::Committed { .. }
            | StoreReply::Generating { .. } => {}
            reply => told.push(reply),
        }
    }
    told
}

fn arrivals_in(told: &[StoreReply]) -> Vec<Arrival> {
    told.iter()
        .filter_map(|reply| match reply {
            StoreReply::Arrivals { arrivals } => Some(arrivals.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

/// The arrivals among `told`, as each one's chunk, key and hops.
fn arrived(told: &[StoreReply]) -> Vec<(ChunkPos, u64, u32)> {
    arrivals_in(told)
        .iter()
        .map(|arrival| (arrival.chunk, arrival.entity.key.0, arrival.entity.hops))
        .collect()
}

/// The `Enter`s among `told`: each one's stay, its unit's keys and its holder.
fn enters(told: &[StoreReply]) -> Vec<(u64, Option<Vec<u64>>, Option<u32>)> {
    told.iter()
        .filter_map(|reply| match reply {
            StoreReply::Enter {
                entity,
                unit,
                holder,
                ..
            } => Some((
                entity.0,
                unit.as_ref().map(keys_of),
                holder.map(|holder| holder.0),
            )),
            _ => None,
        })
        .collect()
}

/// Whether `told` says the stay of `p` with `stay` is dead.
fn said_dead(told: &[StoreReply], p: PlayerId, stay: u64) -> bool {
    told.iter().any(|reply| match reply {
        StoreReply::Dead { stays } => stays
            .iter()
            .any(|(who, entity, _)| *who == p && entity.0 >= stay),
        _ => false,
    })
}

/// The unit the running store keeps with `p`'s record, as its members' keys.
fn kept_unit(store: &Store, p: PlayerId) -> Option<Vec<u64>> {
    let (_, records) = store.players();
    records
        .into_iter()
        .find(|record| record.player == p.0.as_u128())
        .and_then(|record| record.unit)
        .map(|unit| unit.0.iter().map(|member| member.key.0).collect())
}

/// The chunk at `position` as the region of `handle` loads it, passing over whatever
/// else it is told first; `None` if it does not hold it.
fn loaded(handle: &StoreHandle, position: ChunkPos) -> Option<Vec<Entity>> {
    handle.request(StoreRequest::Load { position });
    while let Ok(reply) = handle.replies.recv() {
        match reply {
            StoreReply::Loaded {
                position: at,
                chunk,
            } if at == position => return Some(chunk.entities().to_vec()),
            StoreReply::NotHeld { position: at, .. } | StoreReply::Unreadable { position: at }
                if at == position =>
            {
                return None;
            }
            _ => {}
        }
    }
    None
}

/// Where every entity of `keys` is in a store started on what `disk` has: in which
/// chunk, filed as an arrival for which region, or in which player's record. The regions
/// named are opened, with an epoch above any of the tests', and each loads the chunks
/// named for it; each is sent the arrivals filed for it at its hello.
fn whereabouts(
    disk: &MemoryDisk,
    division: &Division,
    regions: &[(u32, &[ChunkPos])],
    keys: &[u64],
) -> BTreeMap<u64, Vec<String>> {
    let left = Arc::new(disk.crashed(Survival::Nothing));
    let store = store_on(&left, division).expect("the store starts again");
    let mut places: BTreeMap<u64, Vec<String>> =
        keys.iter().map(|key| (*key, Vec::new())).collect();
    let mut note = |key: u64, place: String| {
        if let Some(places) = places.get_mut(&key) {
            places.push(place);
        }
    };
    for (region, chunks) in regions {
        let (handle, _) = store
            .open_region(hello(*region, 1_000))
            .expect("the region opens");
        for arrival in arrivals_in(&told(&handle)) {
            note(
                arrival.entity.key.0,
                format!("filed for {region} into {:?}", arrival.chunk),
            );
        }
        for chunk in *chunks {
            for entity in loaded(&handle, *chunk).unwrap_or_default() {
                note(entity.key.0, format!("in {chunk:?}"));
            }
        }
    }
    for record in store.players().1 {
        for member in record.unit.iter().flat_map(|unit| unit.0.iter()) {
            note(member.key.0, format!("with player {}", record.player));
        }
    }
    places
}

/// The design's invariant for the world of [`division`]: each member of `keys` is in
/// exactly one place, and it is `expected` for each.
fn in_one_place(disk: &MemoryDisk, keys: &[u64], expected: &str, case: &str) {
    let regions: [(u32, &[ChunkPos]); 2] = [
        (WEST, &[ROOT_CHUNK]),
        (HOME, &[HOME_CHUNK, ChunkPos::new(1, 0)]),
    ];
    let places = whereabouts(disk, &division(), &regions, keys);
    for (key, places) in places {
        assert_eq!(
            places.len(),
            1,
            "{case}: member {key} is in exactly one place: {places:?}"
        );
        assert!(
            places[0].starts_with(expected),
            "{case}: member {key} is {} and not {expected}",
            places[0]
        );
    }
}

/// A store in memory for the regions of [`division`], both opened, with player 7's stay
/// [`STAY`] given out by the home region, let go to the west, and the unit standing in
/// the west's chunk.
fn given() -> (Arc<MemoryDisk>, Store, StoreHandle, StoreHandle) {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let home = open(&store, hello(HOME, 1));
    let west = open(&store, hello(WEST, 1));
    let p = player(7);
    noted(&home, 1, vec![entering(p, STAY)]);
    put_in_world(&west, 1, &the_unit());
    noted(&west, 2, vec![has(p, STAY, STAY_HOPS, place(-10.0, 0.0))]);
    told(&home);
    told(&west);
    in_one_place(&disk, &[BOAT, PIG], "in", "before the leave");
    (disk, store, home, west)
}

// ------------------------------------------------------------------------------------
// Section 4's table, a row a test.

/// The first row: a note that counts (`Written`), for a record with no unit, keeps the
/// unit in the note, and on apply the record takes it. Nothing is filed.
#[test]
fn a_unit_in_a_note_that_counts_goes_into_a_record_that_has_none() {
    let (disk, store, home, west) = given();
    let p = player(7);
    leave(&west, 3, p, STAY, STAY_HOPS, &the_unit());
    let answers = told(&west);
    assert!(arrived(&answers).is_empty(), "nothing filed: {answers:?}");
    assert!(arrived(&told(&home)).is_empty());
    assert_eq!(kept_unit(&store, p), Some(vec![BOAT, PIG]));
    in_one_place(&disk, &[BOAT, PIG], "with player", "a note that counts");
}

/// The second row: a note that counts while the record has a unit already (which cannot
/// be) sends its own unit back, and the record keeps the one it has.
#[test]
fn a_unit_in_a_note_that_counts_for_a_record_that_has_one_is_sent_back() {
    let (disk, store, _home, west) = given();
    let p = player(7);
    leave(&west, 3, p, STAY, STAY_HOPS, &the_unit());
    told(&west);
    put_in_world(&west, 4, &other_unit());
    leave(&west, 5, p, STAY, STAY_HOPS + 1, &other_unit());
    let answers = told(&west);
    assert_eq!(
        arrived(&answers),
        [
            (ROOT_CHUNK, OTHER_BOAT, HOPS),
            (ROOT_CHUNK, OTHER_PIG, HOPS)
        ],
        "sent back to its root's chunk, for its holder, at the hops it left with"
    );
    assert_eq!(kept_unit(&store, p), Some(vec![BOAT, PIG]));
    in_one_place(&disk, &[BOAT, PIG], "with player", "the first unit");
    in_one_place(&disk, &[OTHER_BOAT, OTHER_PIG], "filed for 0", "the second");
}

/// The last row, `Dead`: the next stay's `Entering` was taken first (row 2 of section 8,
/// a quick reconnect), so the leave's note is of an earlier stay. Its unit goes back to
/// its root's chunk as arrivals, it is answered `Dead` as ADR-0020 has it, and nobody is
/// told to enter.
#[test]
fn a_unit_in_a_note_of_an_earlier_stay_is_sent_back_to_its_roots_chunk() {
    let (disk, store, home, west) = given();
    let p = player(7);
    noted(&home, 2, vec![entering(p, NEXT)]);
    assert_eq!(enters(&told(&home)), [(NEXT, None, Some(WEST))]);
    leave(&west, 3, p, STAY, STAY_HOPS, &the_unit());
    let answers = told(&west);
    assert_eq!(
        arrived(&answers),
        [(ROOT_CHUNK, BOAT, HOPS), (ROOT_CHUNK, PIG, HOPS)]
    );
    assert!(said_dead(&answers, p, NEXT), "{answers:?}");
    assert!(enters(&answers).is_empty(), "sending back answers nobody");
    assert!(
        enters(&told(&home)).is_empty(),
        "sending back answers nobody"
    );
    assert_eq!(kept_unit(&store, p), None);
    in_one_place(&disk, &[BOAT, PIG], "filed for 0", "a dead note");
}

/// The last row, a copy: a note of the living stay with fewer hand-overs than the record
/// has sends its unit back.
#[test]
fn a_unit_in_a_copy_of_the_living_stay_is_sent_back() {
    let (disk, store, home, west) = given();
    let p = player(7);
    noted(&home, 2, vec![has(p, STAY, STAY_HOPS + 2, place(3.0, 3.0))]);
    told(&home);
    leave(&west, 3, p, STAY, STAY_HOPS, &the_unit());
    assert_eq!(
        arrived(&told(&west)),
        [(ROOT_CHUNK, BOAT, HOPS), (ROOT_CHUNK, PIG, HOPS)]
    );
    assert_eq!(kept_unit(&store, p), None);
    in_one_place(&disk, &[BOAT, PIG], "filed for 0", "a copy");
}

/// The last row, `Ahead`: a note of a stay above the floor, which the store drops (and
/// does not answer `Dead`), sends its unit back all the same.
#[test]
fn a_unit_in_a_note_of_a_stay_above_the_floor_is_sent_back() {
    let (disk, store, _home, west) = given();
    let p = player(7);
    leave(&west, 3, p, NEXT + 10, 0, &the_unit());
    let answers = told(&west);
    assert_eq!(
        arrived(&answers),
        [(ROOT_CHUNK, BOAT, HOPS), (ROOT_CHUNK, PIG, HOPS)]
    );
    assert!(!said_dead(&answers, p, 0), "{answers:?}");
    assert_eq!(kept_unit(&store, p), None);
    in_one_place(&disk, &[BOAT, PIG], "filed for 0", "a note ahead");
}

// ------------------------------------------------------------------------------------
// The trial, by `load`'s semantics.

/// Two notes of one player in one commit are tried in their order against the records as
/// each leaves them. The home region's own commit: an `Entering` that raises the floor,
/// then a note of the stay below it, whose unit goes back; and the other way round, the
/// note first, whose unit the record takes and the `Enter` then carries (row 1a).
#[test]
fn two_notes_of_one_player_in_one_commit_are_tried_in_their_order() {
    let (disk, store, home, west) = given();
    let p = player(7);
    // The unit is in the home region's hands for these commits: as if the player had
    // ridden it back into the home region's land, where the west had let them go.
    noted(&west, 3, vec![has(p, STAY, STAY_HOPS + 1, place(9.0, 1.0))]);
    told(&west);
    let unit = unit_of(BOAT + 100, PIG + 100, 9.5, 1.5);
    put_in_world(&home, 2, &unit);
    told(&home);
    let gone = |unit: &Unit| {
        unit.0
            .iter()
            .map(|member| (HOME_CHUNK, EntityChange::Gone(member.key)))
            .collect::<Vec<_>>()
    };

    // The note after the `Entering`: of a dead stay by then.
    commit(
        &home,
        3,
        vec![
            entering(p, NEXT),
            has_unit(p, STAY, STAY_HOPS + 1, place(9.0, 1.0), unit.clone()),
        ],
        gone(&unit),
        Vec::new(),
    );
    let answers = told(&home);
    assert_eq!(
        arrived(&answers),
        [
            (HOME_CHUNK, BOAT + 100, HOPS),
            (HOME_CHUNK, PIG + 100, HOPS)
        ]
    );
    assert_eq!(enters(&answers), [(NEXT, None, Some(HOME))]);
    assert_eq!(kept_unit(&store, p), None);

    // The note before the `Entering`: of the living stay then.
    let q = player(8);
    noted(&home, 4, vec![entering(q, NEXT + 1)]);
    told(&home);
    let unit = unit_of(BOAT + 200, PIG + 200, 10.5, 2.5);
    put_in_world(&home, 5, &unit);
    told(&home);
    commit(
        &home,
        6,
        vec![
            has_unit(q, NEXT + 1, 0, place(10.0, 2.0), unit.clone()),
            entering(q, NEXT + 2),
        ],
        gone(&unit),
        Vec::new(),
    );
    let answers = told(&home);
    assert!(arrived(&answers).is_empty(), "{answers:?}");
    assert_eq!(
        enters(&answers),
        [(NEXT + 2, Some(vec![BOAT + 200, PIG + 200]), Some(HOME))]
    );
    assert_eq!(kept_unit(&store, q), Some(vec![BOAT + 200, PIG + 200]));
    drop(west);
    in_one_place(&disk, &[BOAT + 100, PIG + 100], "filed for 1", "after");
    in_one_place(&disk, &[BOAT + 200, PIG + 200], "with player", "before");
}

/// Holds the sync of a commit of `handle` without notes, so that what is asked for until
/// [`go_on`] is one group behind it.
fn hold(disk: &Switched, handle: &StoreHandle, tick: u64) {
    disk.holding_syncs.store(true, Ordering::SeqCst);
    noted(handle, tick, Vec::new());
    disk.held.wait();
}

fn go_on(disk: &Switched) {
    disk.held.wait();
}

/// The trial runs "against the records as the group's earlier commits leave them": an
/// `Entering` of the home region and a leave of the west in one group, the `Entering`
/// first, send the unit back; two leaves of the west in one group keep the first unit and
/// send the second back.
#[test]
fn the_trial_reads_the_records_as_the_groups_earlier_commits_leave_them() {
    let (store, disk) = switched_for(&division());
    let home = open(&store, hello(HOME, 1));
    let west = open(&store, hello(WEST, 1));
    let p = player(7);
    noted(&home, 1, vec![entering(p, STAY)]);
    put_in_world(&west, 1, &the_unit());
    put_in_world(&west, 2, &other_unit());
    noted(&west, 3, vec![has(p, STAY, STAY_HOPS, place(-10.0, 0.0))]);
    told(&home);
    told(&west);

    hold(&disk, &home, 2);
    noted(&home, 3, vec![entering(p, NEXT)]);
    leave(&west, 4, p, STAY, STAY_HOPS, &the_unit());
    go_on(&disk);
    let answers = told(&west);
    assert_eq!(
        arrived(&answers),
        [(ROOT_CHUNK, BOAT, HOPS), (ROOT_CHUNK, PIG, HOPS)],
        "the floor the group's earlier commit raised kills the note"
    );
    assert_eq!(enters(&told(&home)), [(NEXT, None, Some(WEST))]);
    assert_eq!(kept_unit(&store, p), None);

    // A second player, whose two leaves (as a region restored on top of its first would
    // say them) come in one group.
    let q = player(8);
    noted(&home, 4, vec![entering(q, NEXT + 1)]);
    noted(&west, 5, vec![has(q, NEXT + 1, 1, place(-11.0, 0.0))]);
    told(&home);
    told(&west);
    let q_unit = |root, pig| {
        let mut unit = unit_of(root, pig, -13.5, 0.5);
        unit.0[0].riding.passengers[0].key = EntityId(NEXT + 1);
        unit
    };
    put_in_world(&west, 6, &q_unit(60, 61));
    put_in_world(&west, 7, &q_unit(70, 71));
    told(&west);
    hold(&disk, &home, 5);
    leave(&west, 8, q, NEXT + 1, 1, &q_unit(60, 61));
    leave(&west, 9, q, NEXT + 1, 2, &q_unit(70, 71));
    go_on(&disk);
    assert_eq!(
        arrived(&told(&west)),
        [(ROOT_CHUNK, 70, HOPS), (ROOT_CHUNK, 71, HOPS)]
    );
    assert_eq!(kept_unit(&store, q), Some(vec![60, 61]));
    told(&home);
    in_one_place(&disk.disk, &[BOAT, PIG, 70, 71], "filed for 0", "sent back");
    in_one_place(&disk.disk, &[60, 61], "with player", "kept");
    in_one_place(&disk.disk, &[OTHER_BOAT, OTHER_PIG], "in", "never moved");
}

// ------------------------------------------------------------------------------------
// The arrival a unit is sent back as.

/// "The unit leaves the note and becomes one of the commit's arrivals, indexed after the
/// region's own, addressed to the root's chunk, at `hops` h": a commit that sends an
/// entity of its own across a line and a dead note's unit has the region's arrival first
/// and the unit's members after it, root first, under one sequence.
#[test]
fn a_unit_sent_back_is_one_of_the_commits_arrivals_after_the_regions_own() {
    let (disk, _store, home, west) = given();
    let p = player(7);
    noted(&home, 2, vec![entering(p, NEXT)]);
    told(&home);
    let own = item(90, -1.5, 0.5);
    put_in_world(&west, 3, &Unit(vec![own.clone()]));
    told(&west);
    let unit = the_unit();
    let mut gone: Vec<(ChunkPos, EntityChange)> = unit
        .0
        .iter()
        .map(|member| (ROOT_CHUNK, EntityChange::Gone(member.key)))
        .collect();
    gone.push((ROOT_CHUNK, EntityChange::Gone(own.key)));
    let mut sent = own.clone();
    sent.position = [1.5, -60.0, 0.5];
    commit(
        &west,
        4,
        vec![has_unit(p, STAY, STAY_HOPS, place(-9.0, 1.0), unit)],
        gone,
        vec![(ChunkPos::new(0, 0), sent)],
    );
    let mut all = arrivals_in(&told(&west));
    all.extend(arrivals_in(&told(&home)));
    all.sort_by_key(|arrival| arrival.id);
    let order: Vec<(u64, u64)> = all
        .iter()
        .map(|arrival| (arrival.entity.key.0, u64::from(arrival.id.index)))
        .collect();
    assert_eq!(order, [(90, 0), (BOAT, 1), (PIG, 2)], "{all:?}");
    assert!(
        all.iter()
            .all(|arrival| arrival.id.sequence == all[0].id.sequence),
        "one commit's arrivals: {all:?}"
    );
    in_one_place(&disk, &[BOAT, PIG], "filed for 0", "sent back");
    in_one_place(&disk, &[90], "filed for 1", "the region's own");
}

/// The arrival is "filed by `Table::holder` (normally the sender, which takes it back)":
/// the home region's own dead note, of a unit in its own land, files the members for the
/// home region, and the west, which keeps a unit of its own meanwhile, is sent nothing.
#[test]
fn a_unit_sent_back_is_filed_for_the_holder_of_its_roots_chunk() {
    let (disk, _store, home, west) = given();
    let p = player(7);
    noted(&home, 2, vec![entering(p, NEXT)]);
    told(&home);
    // The note's unit stands in the home region's land: its rider had crossed back.
    let (boat, pig) = (BOAT + 100, PIG + 100);
    let unit = unit_of(boat, pig, 3.5, 0.5);
    put_in_world(&home, 3, &unit);
    told(&home);
    let gone = unit
        .0
        .iter()
        .map(|member| (HOME_CHUNK, EntityChange::Gone(member.key)))
        .collect();
    commit(
        &home,
        4,
        vec![has_unit(p, STAY, STAY_HOPS, place(3.0, 1.0), unit)],
        gone,
        Vec::new(),
    );
    assert!(arrived(&told(&west)).is_empty());
    assert_eq!(
        arrived(&told(&home)),
        [(HOME_CHUNK, boat, HOPS), (HOME_CHUNK, pig, HOPS)]
    );
    in_one_place(&disk, &[boat, pig], "filed for 1", "for the holder");
    in_one_place(&disk, &[BOAT, PIG], "in", "the west's unit, untouched");
}

/// "Land nobody holds granted to the sender in the group, its `Granted` record ahead of
/// the commit's since the trial comes before `nobodys`": a unit sent back into a chunk
/// nobody holds grants that chunk to the region whose note it was, which is told of the
/// grant before the arrivals, and the log has the grant before the commit.
#[test]
fn a_unit_sent_back_into_land_nobody_holds_grants_it_to_the_sender_ahead_of_the_commit() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let home = open(&store, hello(GAP_HOME, 1));
    let west = open(&store, hello(GAP_WEST, 1));
    let p = player(7);
    noted(&home, 1, vec![entering(p, STAY)]);
    noted(&home, 2, vec![entering(p, NEXT)]);
    told(&home);
    let before = logged(&disk).len();
    // The unit's root was in land nobody holds when the region let it out of the world:
    // a chunk it had and gave back, say. Its note is of a dead stay.
    let unit = unit_of(BOAT, PIG, 88.5, 88.5);
    assert_eq!(unit.root().chunk(), FREE);
    noted(&home, 3, vec![has_unit(p, STAY, 0, place(1.0, 1.0), unit)]);
    let answers = told(&home);
    let claimed = answers
        .iter()
        .position(
            |reply| matches!(reply, StoreReply::Claimed { granted, .. } if granted == &[FREE]),
        )
        .unwrap_or_else(|| panic!("the sender is granted the chunk: {answers:?}"));
    let arrivals = answers
        .iter()
        .position(|reply| matches!(reply, StoreReply::Arrivals { .. }))
        .expect("the arrivals are the sender's");
    assert!(claimed < arrivals, "{answers:?}");
    assert_eq!(arrived(&answers), [(FREE, BOAT, HOPS), (FREE, PIG, HOPS)]);
    assert!(arrived(&told(&west)).is_empty());

    let records = logged(&disk);
    let new = &records[before..];
    let granted = new
        .iter()
        .position(|record| matches!(record, LogRecord::Granted { chunks, .. } if chunks == &[FREE]))
        .unwrap_or_else(|| panic!("a grant of the chunk: {new:?}"));
    let committed = new
        .iter()
        .position(|record| matches!(record, LogRecord::Commit { tick: 3, .. }))
        .expect("the commit");
    assert!(granted < committed, "{new:?}");
    drop((home, west));
    let regions: [(u32, &[ChunkPos]); 1] = [(GAP_HOME, &[])];
    let places = whereabouts(&disk, &gap(), &regions, &[BOAT, PIG]);
    for (key, places) in places {
        assert_eq!(
            places,
            [format!("filed for {GAP_HOME} into {FREE:?}")],
            "{key}"
        );
    }
    drop(store);
}

/// The records of the log on `disk`, segment by segment.
fn logged(disk: &MemoryDisk) -> Vec<LogRecord> {
    let mut paths: Vec<_> = disk
        .paths()
        .into_iter()
        .filter(|path| path.extension().is_some_and(|extension| extension == "wal"))
        .collect();
    paths.sort();
    paths
        .iter()
        .flat_map(|path| read_log(&disk.read(path).unwrap().unwrap()).unwrap().0)
        .collect()
}

// ------------------------------------------------------------------------------------
// Takings.

/// "A taking `(P, K)` clears `record.unit` of P if its root is K, with no stay test",
/// whatever note of a dead stay its commit carries; "a taking of another key changes
/// nothing", not even one of a member that is not the root.
#[test]
fn a_taking_clears_the_unit_beside_a_dead_note_and_one_of_another_key_changes_nothing() {
    let (disk, store, home, west) = given();
    let p = player(7);
    leave(&west, 3, p, STAY, STAY_HOPS, &the_unit());
    told(&west);
    noted(&home, 2, vec![entering(p, NEXT)]);
    assert_eq!(
        enters(&told(&home)),
        [(NEXT, Some(vec![BOAT, PIG]), Some(WEST))]
    );

    noted(&home, 3, vec![took(p, PIG)]);
    told(&home);
    noted(&home, 4, vec![took(p, OTHER_BOAT)]);
    told(&home);
    assert_eq!(kept_unit(&store, p), Some(vec![BOAT, PIG]));
    in_one_place(&disk, &[BOAT, PIG], "with player", "another key");

    // The home region puts the unit back as an arrival in the commit of its taking.
    take_back(
        &home,
        5,
        p,
        &the_unit(),
        vec![has(p, STAY, 0, place(4.0, 4.0))],
    );
    let answers = told(&home);
    assert!(
        said_dead(&answers, p, NEXT),
        "the note is dead: {answers:?}"
    );
    assert_eq!(kept_unit(&store, p), None);
    assert_eq!(
        arrived(&told(&west)),
        [(ROOT_CHUNK, BOAT, HOPS), (ROOT_CHUNK, PIG, HOPS)]
    );
    in_one_place(&disk, &[BOAT, PIG], "filed for 0", "taken");
}

// ------------------------------------------------------------------------------------
// `Enter` and `Again`.

/// "`Enter` gains `unit`, the record's (`Raised` and `Again` alike), and `holder` is the
/// holder of the root's chunk when there is a unit": the place is in the home region's
/// land and the unit in the west's, and the answer names the west, at the raise, at
/// every naming of the stay again, and after the home region is opened anew.
#[test]
fn enter_and_again_carry_the_unit_and_name_the_holder_of_the_roots_chunk() {
    let (disk, store, home, west) = given();
    let p = player(7);
    let unit = the_unit();
    let chunk = unit.root().chunk();
    let gone = unit
        .0
        .iter()
        .map(|member| (chunk, EntityChange::Gone(member.key)))
        .collect();
    // They got off east of the line, the boat west of it (row 17).
    commit(
        &west,
        3,
        vec![has_unit(p, STAY, STAY_HOPS, place(2.0, 1.0), unit)],
        gone,
        Vec::new(),
    );
    told(&west);
    let unit_keys = Some(vec![BOAT, PIG]);

    noted(&home, 2, vec![entering(p, NEXT)]);
    let raised = told(&home);
    assert_eq!(enters(&raised), [(NEXT, unit_keys.clone(), Some(WEST))]);
    let place_told = raised.iter().find_map(|reply| match reply {
        StoreReply::Enter { place, .. } => place.clone(),
        _ => None,
    });
    assert_eq!(place_told, Some(place(2.0, 1.0)), "the dismount pose");

    noted(&home, 3, vec![entering(p, NEXT)]);
    assert_eq!(
        enters(&told(&home)),
        [(NEXT, unit_keys.clone(), Some(WEST))]
    );
    drop(home);
    let home = open(&store, hello(HOME, 2));
    noted(&home, 4, vec![entering(p, NEXT)]);
    assert_eq!(enters(&told(&home)), [(NEXT, unit_keys, Some(WEST))]);
    in_one_place(&disk, &[BOAT, PIG], "with player", "told, not taken");

    // A store started again says the same.
    drop((home, west));
    drop(store);
    let store = store_on(&disk, &division()).unwrap();
    let home = open(&store, hello(HOME, 3));
    noted(&home, 5, vec![entering(p, NEXT)]);
    assert_eq!(
        enters(&told(&home)),
        [(NEXT, Some(vec![BOAT, PIG]), Some(WEST))]
    );
}

// ------------------------------------------------------------------------------------
// Takings count as notes.

/// "The commit arm's early return": a commit whose only stay entry is a taking is taken
/// as one with notes, logged with it, and its taking applied.
#[test]
fn a_commit_whose_only_stay_entry_is_a_taking_is_logged_and_clears_the_unit() {
    let (disk, store, home, west) = given();
    let p = player(7);
    leave(&west, 3, p, STAY, STAY_HOPS, &the_unit());
    told(&west);
    take_back(&home, 2, p, &the_unit(), Vec::new());
    told(&home);
    assert_eq!(kept_unit(&store, p), None);
    in_one_place(&disk, &[BOAT, PIG], "filed for 0", "taken");
    let logged_taking = logged(&disk).iter().any(|record| {
        matches!(record, LogRecord::Commit { region, tick: 2, stays, .. }
            if *region == HOME && stays == &[note_on_disk(&took(p, BOAT))])
    });
    assert!(logged_taking, "the taking is in the log");
    let _ = (&home, &west);
}

/// Row 12: "the store restarts with a segment whose only stay entry is a taking: the
/// taking is replayed; U not doubled", whatever a crash kept of the disk.
#[test]
fn a_restart_replays_a_commit_whose_only_stay_entry_is_a_taking() {
    let (disk, store, home, west) = given();
    let p = player(7);
    leave(&west, 3, p, STAY, STAY_HOPS, &the_unit());
    told(&west);
    for survival in SURVIVALS {
        let again = store_on(&Arc::new(disk.crashed(survival)), &division()).unwrap();
        assert_eq!(kept_unit(&again, p), Some(vec![BOAT, PIG]), "{survival:?}");
    }
    take_back(&home, 2, p, &the_unit(), Vec::new());
    told(&home);
    for survival in SURVIVALS {
        let again = store_on(&Arc::new(disk.crashed(survival)), &division()).unwrap();
        assert_eq!(kept_unit(&again, p), None, "{survival:?}");
    }
    assert_eq!(kept_unit(&store, p), None);
    in_one_place(&disk, &[BOAT, PIG], "filed for 0", "taken");
    let _ = (&home, &west);
}

/// "`Opened`'s `retain` of what a region's commits above `restored` said": a taking in a
/// commit that a later opening of its region passes over is no part of the region's
/// history, and the record keeps the unit; restored up to that commit, it is taken. A
/// running store writes no such log; a crash that undid a cut does, so it is written by
/// hand.
#[test]
fn an_opening_that_passes_over_a_taking_leaves_the_unit_in_the_record() {
    let p = player(7);
    let record = |region: u32, tick: u64, epoch: u64, stays: Vec<LoggedStay>| LogRecord::Commit {
        travellers: Vec::new(),
        region,
        tick,
        epoch,
        changes: Vec::new(),
        state: format!("delta {tick}").into_bytes(),
        stays,
        ticks: Vec::new(),
        marks_taken: Vec::new(),
        seams: Vec::new(),
        effects: Vec::new(),
        taken: Vec::new(),
        events: Vec::new(),
        entity_changes: Vec::new(),
        arrivals: Vec::new(),
        arrivals_taken: Vec::new(),
        claims: Vec::new(),
        raids: Vec::new(),
    };
    let world = |restored: u64| {
        let disk = Arc::new(MemoryDisk::default());
        drop(store_on(&disk, &division()).unwrap());
        let log = [
            record(HOME, 1, 1, vec![note_on_disk(&entering(p, STAY))]),
            record(
                WEST,
                1,
                1,
                vec![note_on_disk(&has_unit(
                    p,
                    STAY,
                    STAY_HOPS,
                    place(-9.0, 1.0),
                    the_unit(),
                ))],
            ),
            record(HOME, 2, 1, vec![note_on_disk(&took(p, BOAT))]),
            LogRecord::Opened {
                region: HOME,
                epoch: 2,
                restored,
            },
        ];
        let bytes: Vec<u8> = log.iter().flat_map(LogRecord::encode).collect();
        put(&disk, "/world/log/00000000000000000001.wal", &bytes);
        disk
    };
    let store = store_on(&world(1), &division()).unwrap();
    assert_eq!(kept_unit(&store, p), Some(vec![BOAT, PIG]), "passed over");
    let store = store_on(&world(2), &division()).unwrap();
    assert_eq!(kept_unit(&store, p), None, "a part of the history");
}

/// "Segment keeping (`trim_for_the_players`, rule P)": once every region has
/// checkpointed past the commit of a taking, its segment may go only with the players'
/// file written after it. A store started again after the checkpoints, whatever a crash
/// kept, has no unit with the record.
#[test]
fn a_segment_whose_only_stay_entry_is_a_taking_is_not_let_go_before_the_players_file_has_it() {
    let (disk, store, home, west) = given();
    let p = player(7);
    leave(&west, 3, p, STAY, STAY_HOPS, &the_unit());
    told(&west);
    for (handle, tick) in [(&home, 1), (&west, 3)] {
        handle.request(StoreRequest::Checkpoint {
            tick,
            state: b"state".to_vec(),
        });
        handle.flush();
    }
    // The unit is in the players' file by now, or in a segment kept for it.
    let in_file = players_file(&*disk).is_some_and(|file| {
        file.records
            .iter()
            .any(|record| record.player == p.0.as_u128() && record.unit.is_some())
    });
    assert!(
        in_file || !segments(&*disk).is_empty(),
        "the unit is somewhere on the disk"
    );

    take_back(&home, 2, p, &the_unit(), Vec::new());
    told(&home);
    // The west takes the unit in, one hop higher, so that no arrival filed keeps the
    // taking's segment; then both regions checkpoint past every commit.
    let arrivals = arrivals_in(&told(&west));
    let members: Vec<Entity> = arrivals
        .iter()
        .map(|arrival| {
            let mut member = arrival.entity.clone();
            member.hops += 1;
            member
        })
        .collect();
    let whole = members
        .iter()
        .map(|member| (ROOT_CHUNK, EntityChange::Whole(member.clone())))
        .collect();
    west.request(StoreRequest::Commit {
        travellers: Vec::new(),
        tick: 4,
        changes: Vec::new(),
        state: b"delta 4".to_vec(),
        stays: Vec::new(),
        ticks: Vec::new(),
        marks_taken: Vec::new(),
        seams: Vec::new(),
        effects: Vec::new(),
        taken: Vec::new(),
        events: Vec::new(),
        entity_changes: whole,
        arrivals: Vec::new(),
        arrivals_taken: arrivals.iter().map(|arrival| arrival.id).collect(),
        clock: Default::default(),
        claims: Vec::new(),
        raids: Vec::new(),
    });
    // As a region does before it checkpoints, the west saves the chunk it changed.
    let mut chunk = generator().generate(ROOT_CHUNK);
    chunk.set_entities(members);
    chunk.clear_generated();
    west.request(StoreRequest::Save {
        position: ROOT_CHUNK,
        tick: 4,
        chunk,
        parts: Parts::ALL,
    });
    told(&west);
    for (handle, tick) in [(&home, 2), (&west, 4)] {
        handle.request(StoreRequest::Checkpoint {
            tick,
            state: b"later".to_vec(),
        });
        handle.flush();
    }
    // Either the taking's segment is kept, or the players' file was written after it.
    let unit_in_file = players_file(&*disk).is_some_and(|file| {
        file.records
            .iter()
            .any(|record| record.player == p.0.as_u128() && record.unit.is_some())
    });
    assert!(
        !segments(&*disk).is_empty() || !unit_in_file,
        "the log was let go with the taking, and the file still has the unit"
    );
    for survival in SURVIVALS {
        let again = store_on(&Arc::new(disk.crashed(survival)), &division()).unwrap();
        assert_eq!(kept_unit(&again, p), None, "{survival:?}");
    }
    assert_eq!(kept_unit(&store, p), None);
    in_one_place(&disk, &[BOAT, PIG], "in", "taken in");
}

/// What a store reckons of the players and their units, and what it has filed: the
/// running store's, after a flush, and that of a store started again on its disk after
/// any crash, are the same ("replay equals the running store").
#[test]
fn a_store_started_again_has_the_units_the_running_store_had() {
    let (disk, store, home, west) = given();
    let (p, q) = (player(7), player(8));
    // P's unit is kept; Q's leave comes after Q's next stay and is sent back; P's second
    // note's unit is sent back for the one kept; a taking of another root does nothing.
    noted(&home, 2, vec![entering(q, NEXT + 1)]);
    noted(&west, 3, vec![has(q, NEXT + 1, 1, place(-12.0, 3.0))]);
    told(&home);
    told(&west);
    leave(&west, 4, p, STAY, STAY_HOPS, &the_unit());
    noted(&home, 3, vec![entering(q, NEXT + 2), took(q, BOAT)]);
    let mut q_unit = other_unit();
    q_unit.0[0].riding.passengers[0].key = EntityId(NEXT + 1);
    put_in_world(&west, 5, &q_unit);
    leave(&west, 6, q, NEXT + 1, 1, &q_unit);
    let extra = unit_of(80, 81, -14.5, 0.5);
    put_in_world(&west, 7, &extra);
    leave(&west, 8, p, STAY, STAY_HOPS + 1, &extra);
    told(&home);
    let filed = arrived(&told(&west));
    let running = store.players();
    assert_eq!(kept_unit(&store, p), Some(vec![BOAT, PIG]));
    assert_eq!(kept_unit(&store, q), None);
    assert_eq!(
        filed,
        [
            (ROOT_CHUNK, OTHER_BOAT, HOPS),
            (ROOT_CHUNK, OTHER_PIG, HOPS),
            (ROOT_CHUNK, 80, HOPS),
            (ROOT_CHUNK, 81, HOPS)
        ]
    );
    for survival in SURVIVALS {
        let again = store_on(&Arc::new(disk.crashed(survival)), &division()).unwrap();
        assert_eq!(again.players(), running, "{survival:?}");
        let (handle, _) = again.open_region(hello(WEST, 500)).unwrap();
        assert_eq!(arrived(&told(&handle)), filed, "{survival:?}");
    }
    in_one_place(&disk, &[BOAT, PIG], "with player", "kept");
    in_one_place(
        &disk,
        &[OTHER_BOAT, OTHER_PIG, 80, 81],
        "filed for 0",
        "sent",
    );
}

// ------------------------------------------------------------------------------------
// A group that fails.

/// Row 14: "a group with any of these fails: `Undo` takes back the record's unit, the
/// taking and the arrival together". In one group that cannot be made durable: a
/// taking of the unit the record keeps, a second player's leave whose unit the record
/// would take, and a third's dead note whose unit would be filed. Afterwards the records
/// are as before, nothing is filed, and every member is where the durable commits put
/// it, in the running store and after any crash.
#[test]
fn a_group_that_fails_takes_back_the_units_the_takings_and_the_arrivals() {
    let (store, disk) = switched_for(&division());
    let home = open(&store, hello(HOME, 1));
    let west = open(&store, hello(WEST, 1));
    let (p, q, r) = (player(7), player(8), player(9));
    noted(
        &home,
        1,
        vec![entering(p, STAY), entering(q, 20), entering(r, 30)],
    );
    noted(&home, 2, vec![entering(r, 31)]);
    put_in_world(&west, 1, &the_unit());
    noted(
        &west,
        2,
        vec![
            has(p, STAY, 1, place(-10.0, 0.0)),
            has(q, 20, 1, place(-11.0, 0.0)),
        ],
    );
    leave(&west, 3, p, STAY, 1, &the_unit());
    let q_unit = other_unit();
    put_in_world(&west, 4, &q_unit);
    let r_unit = unit_of(80, 81, -14.5, 0.5);
    put_in_world(&west, 5, &r_unit);
    told(&home);
    told(&west);
    let before = store.players();
    assert_eq!(kept_unit(&store, p), Some(vec![BOAT, PIG]));

    hold(&disk, &home, 3);
    take_back(&home, 4, p, &the_unit(), Vec::new());
    leave(&west, 6, q, 20, 1, &q_unit);
    leave(&west, 7, r, 30, 1, &r_unit);
    disk.failing_syncs.store(true, Ordering::SeqCst);
    go_on(&disk);
    assert!(
        arrived(&told(&west)).is_empty(),
        "nothing filed was answered"
    );
    assert!(arrived(&told(&home)).is_empty());
    assert!(home.is_lost() && west.is_lost());
    disk.failing_syncs.store(false, Ordering::SeqCst);

    assert_eq!(store.players(), before, "the records as they were");
    assert_eq!(kept_unit(&store, p), Some(vec![BOAT, PIG]));
    assert_eq!(kept_unit(&store, q), None);
    let (west, _) = store.open_region(hello(WEST, 2)).unwrap();
    assert!(
        arrived(&told(&west)).is_empty(),
        "no arrival of the failed group is filed"
    );
    drop(west);
    for survival in SURVIVALS {
        let again = store_on(&Arc::new(disk.disk.crashed(survival)), &division()).unwrap();
        assert_eq!(again.players(), before, "{survival:?}");
    }
    in_one_place(&disk.disk, &[BOAT, PIG], "with player", "kept");
    in_one_place(
        &disk.disk,
        &[OTHER_BOAT, OTHER_PIG, 80, 81],
        "in",
        "never left",
    );
}
