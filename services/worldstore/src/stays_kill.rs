//! The store killed at every point of what it does with stay notes: at every change
//! and sync between an `Entering` note and its answer, while the players' file is
//! written, and while a world is made over. What a crash leaves, in each of the ways a
//! crash can leave things, is then started on, and the records have to be those of
//! the commits that count, with nothing answered that is not on disk. See
//! `docs/adr/0020-one-stay-per-player.md`, section 8, and the store's row of "Tests".

use std::collections::BTreeMap;
use std::path::Path;

use clustine_format::{LogRecord, LoggedStay, PlayerRecord, PlayersFile, read_log};
use clustine_rpc::StayNote;
use clustine_world::{EntityId, PlayerId};

use super::*;
use crate::disk::{Fault, MemoryDisk, Survival};
use crate::regions::{SURVIVALS, gap, store_on};
use crate::stays::{
    Model, entering, has, lived_in, note_on_disk, place, player, players_file, segments,
};
use crate::tests::{division, hello};
use crate::unpinned::open_world;

/// The regions of [`division`]: the eastern one holds the home chunk.
const WEST: u32 = 0;
const HOME: u32 = 1;

/// What the scenario was told.
#[derive(Debug, Default)]
struct Told {
    /// The notes of the commits that were answered, with whether the home region
    /// committed them.
    confirmed: Vec<(bool, StayNote)>,
    /// Every `Enter` that was answered.
    entered: Vec<(PlayerId, EntityId)>,
    /// How many checkpoints were asked for and flushed.
    checkpoints: usize,
}

/// A handle, and the notes of what was committed through it, by tick.
struct Actor {
    handle: StoreHandle,
    home: bool,
    sent: BTreeMap<u64, Vec<StayNote>>,
}

impl Actor {
    /// Opens `region` with `epoch`, trying again a few times: a fault may come in the
    /// middle of it. Returns the tick the region was restored up to as well.
    fn opened(store: &Store, region: u32, epoch: u64) -> Option<(Actor, u64)> {
        let (handle, restored) =
            (0..3).find_map(|_| store.open_region(hello(region, epoch)).ok())?;
        let state = restored.state.as_ref().map(|state| state.tick);
        let tick = restored.deltas.last().map(|delta| delta.tick).or(state);
        let actor = Actor {
            handle,
            home: region == HOME,
            sent: BTreeMap::new(),
        };
        Some((actor, tick.unwrap_or(0)))
    }

    fn commit(&mut self, tick: u64, stays: Vec<StayNote>) {
        self.sent.insert(tick, stays.clone());
        self.handle.request(StoreRequest::Commit {
            tick,
            changes: Vec::new(),
            state: format!("delta {tick}").into_bytes(),
            stays,
        });
    }

    /// Asks for a flush and takes the answers until it is answered or the handle is
    /// lost. Whatever was answered counts as told, also if the handle is lost right
    /// after.
    fn settle(&mut self, told: &mut Told) {
        self.handle.request(StoreRequest::Flush);
        while let Ok(reply) = self.handle.replies.recv() {
            match reply {
                StoreReply::Committed { tick } => {
                    let notes = self.sent.remove(&tick).unwrap_or_default();
                    told.confirmed
                        .extend(notes.into_iter().map(|note| (self.home, note)));
                }
                StoreReply::Enter { player, entity, .. } => told.entered.push((player, entity)),
                StoreReply::Flushed => break,
                _ => {}
            }
        }
    }

    fn checkpoint(&mut self, tick: u64, told: &mut Told) {
        self.handle.request(StoreRequest::Checkpoint {
            tick,
            state: format!("state {tick}").into_bytes(),
        });
        self.settle(told);
        if !self.handle.is_lost() {
            told.checkpoints += 1;
        }
    }
}

/// The scenario: a login, the stay written by two regions, a second login that is
/// answered with the place, checkpoints of both regions by which the players' file is
/// written, a region that writes a dead stay, the home region taken over while its
/// owner before still commits, a third login, the file written once more, and a
/// fourth login behind it.
fn scenario(store: &Store) -> Told {
    let mut told = Told::default();
    let (p, q, r) = (player(1), player(2), player(3));
    let Some((mut home, _)) = Actor::opened(store, HOME, 1) else {
        return told;
    };
    let Some((mut west, _)) = Actor::opened(store, WEST, 1) else {
        return told;
    };
    home.commit(1, vec![entering(p, 5)]);
    home.settle(&mut told);
    home.commit(2, vec![has(p, 5, 0, place(2.0, 2.0))]);
    home.commit(3, vec![has(p, 5, 1, place(-5.0, 3.0))]);
    west.commit(1, vec![has(p, 5, 1, place(-6.0, 3.0))]);
    home.settle(&mut told);
    west.settle(&mut told);
    home.commit(4, vec![entering(p, 6), entering(q, 7)]);
    home.settle(&mut told);

    west.checkpoint(1, &mut told);
    home.checkpoint(4, &mut told);

    home.commit(
        5,
        vec![has(p, 6, 0, place(8.0, 8.0)), has(q, 7, 0, place(9.0, 9.0))],
    );
    // The stay the second login made dead, written by the region it went to.
    west.commit(2, vec![has(p, 5, 2, place(-7.0, 7.0))]);
    home.settle(&mut told);
    west.settle(&mut told);

    if let Some((mut second, from)) = Actor::opened(store, HOME, 2) {
        // Too late: the region is not the first owner's any more.
        home.commit(6, vec![entering(r, 8), has(q, 7, 3, place(0.0, 0.0))]);
        second.commit(
            from + 1,
            vec![entering(q, 9), has(p, 6, 1, place(10.0, 10.0))],
        );
        second.settle(&mut told);
        west.checkpoint(2, &mut told);
        second.checkpoint(from + 1, &mut told);
        second.commit(from + 2, vec![entering(r, 10)]);
        second.commit(from + 3, vec![has(r, 10, 0, place(11.0, 11.0))]);
        second.settle(&mut told);
    }
    home.settle(&mut told);
    told
}

/// The records as section 8 says a start has them, worked out here from what is on
/// `disk`: the players' file, and on top of it the notes of every commit in a segment
/// from the file's `from` on, in the order of the log, but for a commit with a tick
/// above what a later opening of its region was restored up to.
fn counted(disk: &MemoryDisk, home: u32) -> (i32, Vec<PlayerRecord>) {
    let file = players_file(disk).unwrap_or(PlayersFile {
        from: 0,
        issued: 0,
        records: Vec::new(),
    });
    let from = file.from;
    let mut model = Model::of(file);
    let mut noted: Vec<(u32, u64, Vec<LoggedStay>)> = Vec::new();
    for segment in segments(disk) {
        let path = format!("/world/log/{segment:020}.wal");
        let bytes = disk.read(Path::new(&path)).unwrap().unwrap();
        // What follows the records that can be read was never answered.
        for record in read_log(&bytes).unwrap().0 {
            match record {
                LogRecord::Commit {
                    region,
                    tick,
                    stays,
                    ..
                } if segment >= from => noted.push((region, tick, stays)),
                LogRecord::Opened {
                    region, restored, ..
                } => noted.retain(|(noting, tick, _)| *noting != region || *tick <= restored),
                _ => {}
            }
        }
    }
    for (region, _, stays) in noted {
        for note in &stays {
            model.note(region == home, note);
        }
    }
    model.kept()
}

/// The stay and the hand-overs the records have of `player`.
fn floor(records: &[PlayerRecord], player: PlayerId) -> (i32, u32) {
    let record = records
        .iter()
        .find(|record| record.player == player.0.as_u128());
    record.map_or((0, 0), |record| (record.stay, record.hops))
}

fn the_records_are_those_of_the_commits_that_count(fault: Fault, survival: Survival) -> Told {
    let disk = Arc::new(MemoryDisk::failing(fault));
    let told = match store_on(&disk, &division()) {
        Ok(store) => scenario(&store),
        Err(_) => Told::default(),
    };
    let case = format!("{fault:?}, {survival:?}");

    let left = Arc::new(disk.crashed(survival));
    let expected = counted(&left, HOME);
    let store = store_on(&left, &division()).unwrap_or_else(|error| panic!("{case}: {error}"));
    let (issued, records) = store.players();
    assert_eq!((issued, records.clone()), expected, "{case}");

    // Nothing was answered that is not on disk: a stay that was told it may enter is
    // the floor or below it, and counts as given.
    for (player, entity) in &told.entered {
        assert!(floor(&records, *player).0 >= entity.0, "{case}: {told:?}");
        assert!(issued >= entity.0, "{case}: {told:?}");
    }
    // And no note of a commit that was confirmed is lost: the record is that of the
    // note or of a later one.
    for (home, note) in &told.confirmed {
        assert!(*home || matches!(note, StayNote::Has { .. }), "{case}");
        match note_on_disk(note) {
            LoggedStay::Entering { player, entity } => {
                let record = floor(&records, crate::stays::player(player));
                assert!(record.0 >= entity, "{case}: {note:?}, {records:?}");
            }
            LoggedStay::Has {
                player,
                entity,
                hops,
                ..
            } => {
                // Unless the stay was never given out as far as the store knows: its
                // home region lost its handle before the note that names it as
                // entering was taken, and a note of such a stay is dropped.
                let record = floor(&records, crate::stays::player(player));
                let ahead = record.0 < entity;
                assert!(
                    ahead || record >= (entity, hops),
                    "{case}: {note:?}, {records:?}"
                );
            }
        }
    }
    // What the first owner of the home region committed after it was replaced left
    // nothing: the stay it named for the third player, and hand-overs nobody made.
    assert_ne!(floor(&records, player(3)).0, 8, "{case}");
    assert_ne!(floor(&records, player(2)), (7, 3), "{case}");

    // The home region is told the highest stay, and a start on what this one left
    // has the same records.
    let (_, restored) = store
        .open_region(hello(HOME, 9))
        .unwrap_or_else(|error| panic!("{case}: {error}"));
    assert_eq!(restored.issued, EntityId(issued), "{case}");
    store.flush().unwrap();
    let again = Arc::new(left.crashed(Survival::Nothing));
    let store = store_on(&again, &division()).unwrap_or_else(|error| panic!("{case}: {error}"));
    assert_eq!(store.players(), expected, "{case}, again");
    told
}

/// How many changes and syncs the scenario makes when nothing goes wrong, with some to
/// spare for the ways the threads can interleave; and that it does what it is for.
fn operations() -> u64 {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let told = scenario(&store);
    // Five logins were answered, and the file was written by each round of
    // checkpoints.
    assert_eq!(told.entered.len(), 5, "{told:?}");
    assert_eq!(told.checkpoints, 4, "{told:?}");
    let file = players_file(disk.as_ref()).expect("the checkpoints let segments go");
    assert!(file.from > 2, "{file:?}");
    let (p, q, r) = (player(1), player(2), player(3));
    let expected = vec![
        crate::stays::kept(p, 6, 1, Some(place(10.0, 10.0))),
        crate::stays::kept(q, 9, 0, Some(place(9.0, 9.0))),
        crate::stays::kept(r, 10, 0, Some(place(11.0, 11.0))),
    ];
    assert_eq!(store.players(), (10, expected));
    disk.operations() + 10
}

#[test]
fn nothing_goes_wrong_without_a_fault() {
    for survival in SURVIVALS {
        let told = the_records_are_those_of_the_commits_that_count(Fault::Fail(u64::MAX), survival);
        assert_eq!(told.entered.len(), 5, "{survival:?}");
    }
}

#[test]
fn a_store_that_stops_at_any_point_of_a_login_has_the_records_of_the_commits_that_count() {
    for n in 1..=operations() {
        for survival in SURVIVALS {
            the_records_are_those_of_the_commits_that_count(Fault::Stop(n), survival);
        }
    }
}

#[test]
fn a_store_that_fails_once_at_any_point_of_a_login_has_the_records_of_the_commits_that_count() {
    for n in 1..=operations() {
        for survival in SURVIVALS {
            the_records_are_those_of_the_commits_that_count(Fault::Fail(n), survival);
        }
    }
}

/// A world with records in the players' file and behind it is started with another
/// division, killed at every change and sync of that start and with everything a
/// crash can keep of it; a start on what is left, and one more on what that one
/// left, have every record, whichever division they are for.
#[test]
fn a_world_that_is_made_over_keeps_its_records_wherever_the_making_over_is_killed() {
    let (world, records) = lived_in();
    for told in [gap(), open_world()] {
        let operations = {
            let disk = Arc::new(world.crashed(Survival::Everything));
            let store = store_on(&disk, &told).unwrap();
            assert_eq!(store.players(), records);
            disk.operations()
        };
        for n in 1..=operations + 1 {
            for fault in [Fault::Stop(n), Fault::Fail(n), Fault::Fails(n, 2)] {
                let disk = Arc::new(world.crashed(Survival::Everything).with(fault));
                // It fails or not; either way it has done what it has.
                drop(store_on(&disk, &told));
                for survival in SURVIVALS {
                    let case = format!("{fault:?}, {survival:?}");
                    let left = Arc::new(disk.crashed(survival));
                    let store =
                        store_on(&left, &told).unwrap_or_else(|error| panic!("{case}: {error}"));
                    assert_eq!(store.players(), records, "{case}");
                    // The home region of the world as it is now gives out ids above
                    // every floor.
                    let home = store.regions().unwrap().home;
                    let (_, restored) = store
                        .open_region(RegionHello {
                            region: home,
                            epoch: 100,
                        })
                        .unwrap_or_else(|error| panic!("{case}: {error}"));
                    assert_eq!(restored.issued, EntityId(records.0), "{case}");
                    assert!(restored.entity_ids.end.0 > records.0 + 1, "{case}");
                    store.flush().unwrap();
                    let again = Arc::new(left.crashed(Survival::Nothing));
                    let store = store_on(&again, &told)
                        .unwrap_or_else(|error| panic!("{case}, again: {error}"));
                    assert_eq!(store.players(), records, "{case}, again");
                }
            }
        }
    }
}
