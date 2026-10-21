//! Tests of the units players leave the world on, from the design and not from the
//! code: `docs/groundwork/n7h-logout-on-a-mount.md`, sections 4 and 13. Each member of a
//! unit is in exactly one of a chunk, a filed arrival or a record: a note's unit is kept
//! with the player's record where it counts and sent back to its root's chunk where it
//! does not, the next stay is told of it with the holder of that chunk, and a taking
//! clears it, before the notes of its commit and again in a replay.

use std::sync::Arc;

use clustine_rpc::{StayNote, StoreReply, Unit};
use clustine_world::{Arrival, ChunkPos, Entity, EntityId, PlayerId};

use crate::Store;
use crate::disk::MemoryDisk;
use crate::entities_tests::item;
use crate::regions::store_on;
use crate::stays::{answers, commit, entering, has, place, player};
use crate::tests::{division, hello, open};

/// The regions of [`division`]: the eastern one holds the home chunk.
const WEST: u32 = 0;
const HOME: u32 = 1;

/// A unit in the west: a vehicle of key 40 at x = -10, and its passenger of key 41.
fn unit() -> Unit {
    Unit(vec![item(40, -10.5, 0.5), item(41, -10.5, 0.5)])
}

/// The chunk of the unit's root, which the western region holds.
const ROOT_CHUNK: ChunkPos = ChunkPos::new(-1, 0);

/// The `Has` of `entity` in the west with `unit`.
fn has_with(p: PlayerId, entity: u64, hops: u32, unit: Unit) -> StayNote {
    let StayNote::Has {
        player,
        entity,
        hops,
        place,
        face,
        let_go,
        ..
    } = has(p, entity, hops, place(-10.0, 0.0))
    else {
        unreachable!("a has note")
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

/// The unit of `p`'s record, as its members' keys.
fn kept_unit(store: &Store, p: PlayerId) -> Option<Vec<u64>> {
    let (_, records) = store.players();
    let record = records
        .into_iter()
        .find(|record| record.player == p.0.as_u128())?;
    record
        .unit
        .map(|unit| unit.0.iter().map(|member| member.key.0).collect())
}

/// The `Enter` among `replies`: its unit's keys and its holder.
fn entered(replies: &[StoreReply]) -> Option<(Option<Vec<u64>>, Option<u32>)> {
    replies.iter().find_map(|reply| match reply {
        StoreReply::Enter { unit, holder, .. } => Some((
            unit.as_ref()
                .map(|unit| unit.0.iter().map(|member| member.key.0).collect()),
            holder.map(|holder| holder.0),
        )),
        _ => None,
    })
}

/// The arrivals among `replies`, as each one's chunk and key.
fn arrived(replies: &[StoreReply]) -> Vec<(ChunkPos, u64)> {
    replies
        .iter()
        .filter_map(|reply| match reply {
            StoreReply::Arrivals { arrivals } => Some(arrivals.clone()),
            _ => None,
        })
        .flatten()
        .map(|arrival: Arrival| (arrival.chunk, arrival.entity.key.0))
        .collect()
}

fn keys(entities: &[Entity]) -> Vec<u64> {
    entities.iter().map(|entity| entity.key.0).collect()
}

/// A store with player 7's stay 5 given out by the home region.
fn given() -> (
    Arc<MemoryDisk>,
    Store,
    crate::StoreHandle,
    crate::StoreHandle,
) {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let home = open(&store, hello(HOME, 1));
    let west = open(&store, hello(WEST, 1));
    commit(&home, 1, vec![entering(player(7), 5)]);
    answers(&home);
    answers(&west);
    (disk, store, home, west)
}

/// The table's first row: a note of the living stay keeps its unit with the record, no
/// arrival is filed, and the next stay is told of it with the holder of the root's
/// chunk, not of the place's.
#[test]
fn a_unit_in_a_note_of_the_living_stay_is_kept_and_given_to_the_next_stay() {
    let (_disk, store, home, west) = given();
    let p = player(7);
    commit(&west, 1, vec![has_with(p, 5, 1, unit())]);
    assert!(arrived(&answers(&west)).is_empty(), "nothing sent back");
    assert_eq!(kept_unit(&store, p), Some(vec![40, 41]));

    commit(&home, 2, vec![entering(p, 9)]);
    let told = answers(&home);
    assert_eq!(entered(&told), Some((Some(vec![40, 41]), Some(WEST))));
    // Kept until a taking: the stay that enters is told of it again.
    assert_eq!(kept_unit(&store, p), Some(vec![40, 41]));
    commit(&home, 3, vec![entering(p, 9)]);
    assert_eq!(
        entered(&answers(&home)),
        Some((Some(vec![40, 41]), Some(WEST)))
    );
}

/// The table's last row: a note of a dead stay (the next stay's `Entering` came first,
/// as in a quick reconnect) sends its unit back to its root's chunk, every member an
/// arrival there for its holder, and the record keeps none.
#[test]
fn a_unit_in_a_note_of_a_dead_stay_is_sent_back_to_its_chunk() {
    let (_disk, store, home, west) = given();
    let p = player(7);
    commit(&home, 2, vec![entering(p, 9)]);
    assert_eq!(entered(&answers(&home)), Some((None, None)));
    commit(&west, 1, vec![has_with(p, 5, 1, unit())]);
    let told = answers(&west);
    assert_eq!(arrived(&told), [(ROOT_CHUNK, 40), (ROOT_CHUNK, 41)]);
    assert_eq!(kept_unit(&store, p), None);
}

/// A record that keeps a unit takes no second one: the second note's goes back, and the
/// first stays.
#[test]
fn a_unit_for_a_record_that_keeps_one_is_sent_back() {
    let (_disk, store, _home, west) = given();
    let p = player(7);
    commit(&west, 1, vec![has_with(p, 5, 1, unit())]);
    answers(&west);
    let other = Unit(vec![item(50, -12.5, 0.5)]);
    commit(&west, 2, vec![has_with(p, 5, 1, other)]);
    assert_eq!(arrived(&answers(&west)), [(ROOT_CHUNK, 50)]);
    assert_eq!(kept_unit(&store, p), Some(vec![40, 41]));
}

/// Two notes of one player in one commit are tried in their order: the first keeps its
/// unit, the second, finding the record's, sends its own back.
#[test]
fn two_units_of_one_player_in_one_commit_are_tried_in_their_order() {
    let (_disk, store, _home, west) = given();
    let p = player(7);
    let other = Unit(vec![item(50, -12.5, 0.5)]);
    commit(
        &west,
        1,
        vec![has_with(p, 5, 1, unit()), has_with(p, 5, 1, other)],
    );
    assert_eq!(arrived(&answers(&west)), [(ROOT_CHUNK, 50)]);
    assert_eq!(kept_unit(&store, p), Some(vec![40, 41]));
}

/// A taking of the record's unit clears it; one of another root changes nothing.
#[test]
fn a_taking_clears_the_records_unit_and_one_of_another_root_does_nothing() {
    let (_disk, store, home, west) = given();
    let p = player(7);
    commit(&west, 1, vec![has_with(p, 5, 1, unit())]);
    answers(&west);
    commit(&home, 2, vec![took(p, 50)]);
    answers(&home);
    assert_eq!(kept_unit(&store, p), Some(vec![40, 41]));
    commit(&home, 3, vec![took(p, 40)]);
    assert!(
        !answers(&home)
            .iter()
            .any(|reply| matches!(reply, StoreReply::Enter { .. } | StoreReply::Dead { .. })),
        "a taking is answered with nothing"
    );
    assert_eq!(kept_unit(&store, p), None);
}

/// A commit's takings are taken before its notes, whatever order the commit has them in
/// (section 13 item 3): a home region that took the unit and names the stay as entering
/// in one commit is not told of the unit again, which it has put back already.
#[test]
fn a_commits_takings_come_before_its_notes() {
    let (_disk, store, home, west) = given();
    let p = player(7);
    commit(&west, 1, vec![has_with(p, 5, 1, unit())]);
    answers(&west);
    commit(&home, 2, vec![entering(p, 9), took(p, 40)]);
    // Told of no unit, and so of the holder of the place's chunk, the west's.
    assert_eq!(entered(&answers(&home)), Some((None, Some(WEST))));
    assert_eq!(kept_unit(&store, p), None);
}

/// A unit kept and a taking are in the log: a store started again from it has the unit
/// while it is kept, and none once a commit whose only stay entry is the taking is
/// replayed (row 12).
#[test]
fn a_restarted_store_replays_a_kept_unit_and_a_commit_with_only_a_taking() {
    let (disk, store, home, west) = given();
    let p = player(7);
    commit(&west, 1, vec![has_with(p, 5, 1, unit())]);
    answers(&west);
    drop((home, west));
    drop(store);
    let store = store_on(&disk, &division()).unwrap();
    assert_eq!(kept_unit(&store, p), Some(vec![40, 41]));

    let home = open(&store, hello(HOME, 2));
    commit(&home, 2, vec![took(p, 40)]);
    answers(&home);
    drop(home);
    drop(store);
    let store = store_on(&disk, &division()).unwrap();
    assert_eq!(kept_unit(&store, p), None);
}

/// What the units' members are, so that a test reads its keys from the unit itself.
#[test]
fn the_units_members_are_its_root_and_its_passenger() {
    assert_eq!(keys(&unit().0), [40, 41]);
    assert_eq!(unit().root().chunk(), ROOT_CHUNK);
}
