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

/// Asks for a copy of `chunk` and waits for its base.
fn copy(handle: &StoreHandle, chunk: ChunkPos) -> StoreReply {
    handle.request(StoreRequest::Copies {
        wanted: vec![chunk],
        gone: Vec::new(),
    });
    handle.request(StoreRequest::Flush);
    for _ in 0..30_000 {
        match handle.try_reply() {
            Some(reply @ StoreReply::CopyBase { .. }) => return reply,
            Some(_) => {}
            None => thread::sleep(Duration::from_millis(1)),
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
    for handle in [&west, &east] {
        let told = faces(&answers(handle));
        assert!(
            matches!(&told[..], [face] if is_face_of(face, 5)),
            "{told:?}"
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
