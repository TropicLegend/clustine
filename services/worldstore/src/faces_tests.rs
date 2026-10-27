//! Tests of the faces the store keeps and streams (`docs/groundwork/n7-faces.md`, step
//! 2), in the world with a gap (`regions::gap`): region 0 west of chunk x = 0, region 1
//! from chunk x = 16 on, and the home region 2 at the origin.

use std::thread;
use std::time::Duration;

use clustine_rpc::{FaceChange, StayNote, WorldClock};
use clustine_world::{ChunkPos, EntityId};

use super::*;
use crate::disk::MemoryDisk;
use crate::regions::{gap, store_on};
use crate::stays::{commit, entering, has, place, player};
use crate::tests::{hello, open};

const WEST: u32 = 0;
const EAST: u32 = 1;
const HOME: u32 = 2;

/// A chunk of the western region's land, and one of the gap.
const WESTERN: ChunkPos = ChunkPos::new(-3, 0);

fn three() -> (Store, StoreHandle, StoreHandle, StoreHandle) {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let home = open(&store, hello(HOME, 1));
    let west = open(&store, hello(WEST, 1));
    let east = open(&store, hello(EAST, 1));
    (store, home, west, east)
}

/// Asks for a flush and returns everything the handle was answered before it.
fn answers(handle: &StoreHandle) -> Vec<StoreReply> {
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

/// The face changes among `replies`, in order: on the stream, and in a copy's base.
fn faces(replies: &[StoreReply]) -> Vec<FaceChange> {
    replies
        .iter()
        .filter_map(|reply| match reply {
            StoreReply::Faces { changes } => Some(changes.clone()),
            StoreReply::CopyBase { faces, .. } => Some(faces.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

/// Whether `change` is a face (not a removal or a base) of the stay `entity`.
fn is_face_of(change: &FaceChange, entity: u64) -> bool {
    matches!(change, FaceChange::Face { entity: of, .. } if *of == EntityId(entity))
}

fn is_removal_of(change: &FaceChange, entity: u64) -> bool {
    matches!(change, FaceChange::Removed { entity: of, .. } if *of == EntityId(entity))
}

/// Asks for a copy of `chunk` and waits for its base and for the answer to the flush
/// behind the ask, which come in either order: the base is made on the thread for
/// chunks.
fn copy(handle: &StoreHandle, chunk: ChunkPos) -> StoreReply {
    handle.request(StoreRequest::Copies {
        wanted: vec![chunk],
        gone: Vec::new(),
    });
    handle.request(StoreRequest::Flush);
    let (mut base, mut flushed) = (None, false);
    for _ in 0..30_000 {
        match handle.try_reply() {
            Some(reply @ StoreReply::CopyBase { .. }) => base = Some(reply),
            Some(StoreReply::Flushed) => flushed = true,
            Some(_) => {}
            None => thread::sleep(Duration::from_millis(1)),
        }
        if flushed && let Some(base) = base.take() {
            return base;
        }
    }
    panic!("no base came");
}

/// In the western region's land, at x = -40.
fn in_the_west() -> clustine_rpc::Place {
    place(-40.0, 8.0)
}

/// In the gap, in the chunk at (5, 5).
fn in_the_gap() -> clustine_rpc::Place {
    place(5.0 * 16.0 + 3.5, 5.0 * 16.0 + 9.0)
}

#[test]
fn a_face_goes_to_who_holds_and_who_copies_its_chunk_and_not_to_its_writer() {
    let (_store, home, west, east) = three();
    let p = player(7);
    commit(&home, 1, vec![entering(p, 5)]);
    answers(&home);
    // Nobody stands there yet.
    let based = faces(&[copy(&east, WESTERN)]);
    assert!(based.is_empty(), "{based:?}");
    answers(&west);

    commit(&home, 2, vec![has(p, 5, 0, in_the_west())]);
    let told = faces(&answers(&home));
    assert!(told.is_empty(), "the writer is told nothing: {told:?}");
    for (name, handle) in [("west", &west), ("east", &east)] {
        let all = answers(handle);
        let told = faces(&all);
        assert!(
            matches!(&told[..], [face] if is_face_of(face, 5)),
            "{name}: {all:?}"
        );
    }

    // Into the gap, which nobody holds or copies: the face reaches nobody, and those
    // of the old chunk are told it is gone from there.
    commit(&home, 3, vec![has(p, 5, 0, in_the_gap())]);
    assert!(faces(&answers(&home)).is_empty());
    for handle in [&west, &east] {
        let told = faces(&answers(handle));
        assert!(
            matches!(&told[..], [removal] if is_removal_of(removal, 5)),
            "{told:?}"
        );
    }
}

#[test]
fn a_later_stay_removes_the_face_of_the_one_before() {
    let (_store, home, west, _east) = three();
    let p = player(7);
    commit(&home, 1, vec![entering(p, 5)]);
    commit(&home, 2, vec![has(p, 5, 0, in_the_west())]);
    answers(&home);
    assert!(
        faces(&answers(&west))
            .iter()
            .any(|face| is_face_of(face, 5))
    );
    commit(&home, 3, vec![entering(p, 9)]);
    answers(&home);
    let told = faces(&answers(&west));
    assert!(
        matches!(&told[..], [removal] if is_removal_of(removal, 5)),
        "{told:?}"
    );
}

#[test]
fn a_hello_is_given_the_faces_of_the_chunks_its_region_holds() {
    let (store, home, west, _east) = three();
    let p = player(7);
    commit(&home, 1, vec![entering(p, 5)]);
    commit(&home, 2, vec![has(p, 5, 0, in_the_west())]);
    answers(&home);
    drop(west);
    let (_west, restored) = store.open_region(hello(WEST, 2)).unwrap();
    assert!(
        matches!(&restored.faces[..], [face] if is_face_of(face, 5)),
        "{:?}",
        restored.faces
    );
    let (_east, restored) = store.open_region(hello(EAST, 2)).unwrap();
    assert!(restored.faces.is_empty(), "{:?}", restored.faces);
}

#[test]
fn the_clock_is_published_with_the_world_s_players() {
    let (_store, home, west, _east) = three();
    let p = player(7);
    commit(&home, 1, vec![entering(p, 5)]);
    commit(&home, 2, vec![has(p, 5, 0, in_the_west())]);
    answers(&home);
    answers(&west);
    home.request(StoreRequest::Commit {
        tick: 3,
        changes: Vec::new(),
        state: b"delta 3".to_vec(),
        stays: Vec::<StayNote>::new(),
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
            game_time: 40,
            day: 40,
        },
        claims: Vec::new(),
        raids: Vec::new(),
    });
    answers(&home);
    let published = answers(&west).into_iter().find_map(|reply| match reply {
        StoreReply::Clock { clock, players } => Some((clock, players)),
        _ => None,
    });
    let (clock, players) = published.expect("the clock was published");
    assert_eq!(clock.game_time, 40);
    assert!(
        matches!(&players[..], [one] if one.player == p && one.entity == EntityId(5)),
        "{players:?}"
    );
}

#[test]
fn a_copy_s_base_carries_the_faces_of_its_chunk_as_they_are() {
    let (_store, home, _west, east) = three();
    let p = player(7);
    commit(&home, 1, vec![entering(p, 5)]);
    commit(&home, 2, vec![has(p, 5, 0, in_the_west())]);
    answers(&home);
    let based = faces(&[copy(&east, WESTERN)]);
    assert!(
        matches!(&based[..], [face] if is_face_of(face, 5)),
        "{based:?}"
    );
}

fn gone(player: clustine_world::PlayerId, entity: u64, hops: u32) -> StayNote {
    StayNote::Gone {
        player,
        entity: EntityId(entity),
        hops,
    }
}

fn let_go(player: clustine_world::PlayerId, entity: u64, hops: u32) -> StayNote {
    let mut note = has(player, entity, hops, in_the_west());
    if let StayNote::Has { let_go, .. } = &mut note {
        *let_go = true;
    }
    note
}

/// Whether the store still has the player's face: it is in the world's players.
fn in_the_world(store_west: &StoreHandle, home: &StoreHandle, tick: u64) -> bool {
    home.request(StoreRequest::Commit {
        tick,
        changes: Vec::new(),
        state: format!("delta {tick}").into_bytes(),
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
            game_time: tick * 100,
            day: tick * 100,
        },
        claims: Vec::new(),
        raids: Vec::new(),
    });
    answers(home);
    let published = answers(store_west)
        .into_iter()
        .find_map(|reply| match reply {
            StoreReply::Clock { players, .. } => Some(players),
            _ => None,
        })
        .expect("the clock was published");
    !published.is_empty()
}

#[test]
fn a_gone_counts_from_the_writer_with_its_hops_and_from_others_only_with_more() {
    let (_store, home, west, east) = three();
    let p = player(7);
    commit(&home, 1, vec![entering(p, 5)]);
    commit(&west, 1, vec![has(p, 5, 2, in_the_west())]);
    answers(&west);
    // Another region with as many hand-overs: an out-of-date copy's word.
    commit(&east, 1, vec![gone(p, 5, 2)]);
    answers(&east);
    assert!(in_the_world(&west, &home, 2));
    // The writer with fewer: an earlier hold of the stay.
    commit(&west, 2, vec![gone(p, 5, 1)]);
    assert!(in_the_world(&west, &home, 3));
    // An earlier stay.
    commit(&west, 3, vec![gone(p, 4, 9)]);
    assert!(in_the_world(&west, &home, 4));
    // Another with more: the stay went on from the writer.
    commit(&east, 2, vec![gone(p, 5, 3)]);
    answers(&east);
    assert!(!in_the_world(&west, &home, 5));
    // A later note of the living stay brings the player back.
    commit(&west, 4, vec![has(p, 5, 4, in_the_west())]);
    assert!(in_the_world(&west, &home, 6));
    commit(&west, 5, vec![gone(p, 5, 4)]);
    assert!(!in_the_world(&west, &home, 7));
}

#[test]
fn a_player_who_leaves_while_handed_over_leaves_the_world() {
    // The sender let them go; the edge tells the region they were heading for, which
    // never held them (the review's R1).
    let (_store, home, west, east) = three();
    let p = player(7);
    commit(&home, 1, vec![entering(p, 5)]);
    commit(&west, 1, vec![let_go(p, 5, 1)]);
    answers(&west);
    assert!(in_the_world(&west, &home, 2));
    commit(&east, 1, vec![gone(p, 5, 1)]);
    answers(&east);
    assert!(!in_the_world(&west, &home, 3));
}

#[test]
fn a_region_s_first_commit_after_a_hello_marks_offline_whom_it_wrote_and_does_not_name() {
    let (store, home, west, _east) = three();
    let (p, q, r) = (player(7), player(8), player(9));
    commit(
        &home,
        1,
        vec![entering(p, 5), entering(q, 6), entering(r, 7)],
    );
    commit(
        &west,
        1,
        vec![
            has(p, 5, 1, in_the_west()),
            has(q, 6, 1, in_the_west()),
            let_go(r, 7, 1),
        ],
    );
    answers(&west);
    drop(west);
    let (west, _) = store.open_region(hello(WEST, 2)).unwrap();
    // Its first tick names p only: q is gone with the crash, r is between regions.
    commit(&west, 2, vec![has(p, 5, 1, in_the_west())]);
    answers(&west);
    home.request(StoreRequest::Commit {
        tick: 2,
        changes: Vec::new(),
        state: b"delta 2".to_vec(),
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
            game_time: 200,
            day: 200,
        },
        claims: Vec::new(),
        raids: Vec::new(),
    });
    answers(&home);
    let players = answers(&west)
        .into_iter()
        .find_map(|reply| match reply {
            StoreReply::Clock { players, .. } => Some(players),
            _ => None,
        })
        .expect("the clock was published");
    let left: Vec<_> = players.iter().map(|one| one.player).collect();
    assert_eq!(left, [p, r]);
    // Later commits mark nobody.
    commit(&west, 3, vec![has(r, 7, 2, in_the_west())]);
    answers(&west);
}
