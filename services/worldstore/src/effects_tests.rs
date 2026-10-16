//! Tests of what the store does with effects (step E3a of
//! `docs/adr/0025-effects-between-regions.md`, sections 2 and 5): use, filing, the
//! stream, takings, droppings, the resend after a hello, a merge or a split, writing
//! forward, the undo of a group cut off, and a store that dies anywhere.
//!
//! The world is the one with a gap (`regions::gap`): region 0 west of x = 0, region 1
//! from chunk x = 16 on, and the home region 2 at the origin.

use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

use clustine_data::blocks;
use clustine_format::{LogRecord, read_log};
use clustine_world::{BlockPos, ChunkPos, Due, Effect, EffectId, Effected, FiledEffect};

use super::*;
use crate::disk::{Fault, MemoryDisk, Survival};
use crate::regions::{SURVIVALS, absorb, claim, gap, give_back, split, store_on};
use crate::tests::{load, switched_for};

/// A chunk of region 1 that it loads, one it does not, and one of the gap.
const USED: ChunkPos = ChunkPos::new(16, 0);
const UNUSED: ChunkPos = ChunkPos::new(17, 0);
const FREE: ChunkPos = ChunkPos::new(5, 5);

/// The `n`th effect of a test into `chunk`: each at a block of its own, so that a test
/// tells them apart by where they are.
fn effect_in(chunk: ChunkPos, n: i32) -> Effect {
    Effect {
        at: BlockPos::new(chunk.x * 16 + n, 64, chunk.z * 16),
        what: Effected::SetIf {
            expected: blocks::AIR,
            state: blocks::STONE,
            flags: 3,
        },
    }
}

fn commit(handle: &StoreHandle, tick: u64, effects: &[Effect], taken: &[EffectId]) {
    handle.request(StoreRequest::Commit {
        tick,
        changes: Vec::new(),
        state: format!("tick {tick}").into_bytes(),
        stays: Vec::new(),
        ticks: Vec::new(),
        marks_taken: Vec::new(),
        seams: Vec::new(),
        effects: effects.to_vec(),
        taken: taken.to_vec(),
    });
}

/// Everything `handle` is told up to the answer to a flush, or until it is lost, and
/// whether the flush was answered.
fn everything_told(handle: &StoreHandle) -> (Vec<StoreReply>, bool) {
    handle.request(StoreRequest::Flush);
    let mut told = Vec::new();
    while let Ok(reply) = handle.replies.recv() {
        if reply == StoreReply::Flushed {
            return (told, true);
        }
        told.push(reply);
    }
    (told, false)
}

/// What `handle` is told up to the answer to a flush, or until it is lost: the commits
/// answered, and the effects on its stream, which every mark has to follow.
fn flushed(handle: &StoreHandle) -> (Vec<u64>, Vec<FiledEffect>) {
    let mut committed = Vec::new();
    let mut effects = Vec::new();
    let mut unmarked = false;
    let (replies, answered) = everything_told(handle);
    for reply in replies {
        match reply {
            StoreReply::Committed { tick } => committed.push(tick),
            StoreReply::Effects { effects: more } => {
                assert!(!more.is_empty(), "a stream carries no empty list");
                unmarked = true;
                effects.extend(more);
            }
            StoreReply::Mark { through } => {
                assert!(
                    effects.iter().all(|filed| filed.id.sequence <= through),
                    "a mark through {through} comes before effects it covers: {effects:?}"
                );
                unmarked = false;
            }
            _ => {}
        }
    }
    // A handle lost in between is cut off wherever it is.
    assert!(!answered || !unmarked, "effects without a mark behind them");
    (committed, effects)
}

/// Where the effects are.
fn places(effects: &[FiledEffect]) -> Vec<BlockPos> {
    effects.iter().map(|filed| filed.effect.at).collect()
}

fn ids(effects: &[FiledEffect]) -> Vec<EffectId> {
    effects.iter().map(|filed| filed.id).collect()
}

/// The records of the log on `disk`, segment by segment.
fn logged(disk: &MemoryDisk) -> Vec<LogRecord> {
    disk.paths()
        .iter()
        .filter(|path| path.extension().is_some_and(|extension| extension == "wal"))
        .flat_map(|path| read_log(&disk.read(path).unwrap().unwrap()).unwrap().0)
        .collect()
}

fn hello(region: u32, epoch: u64) -> RegionHello {
    RegionHello {
        region: RegionId(region),
        epoch,
    }
}

fn opened(store: &Store, region: u32, epoch: u64) -> (StoreHandle, Restored) {
    store.open_region(hello(region, epoch)).unwrap()
}

#[test]
fn an_effect_goes_on_the_stream_of_the_region_that_uses_its_chunk_and_no_other() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (maker, _) = opened(&store, 0, 1);
    let (user, _) = opened(&store, 1, 1);
    let (home, _) = opened(&store, 2, 1);
    load(&user, USED);

    let made = [effect_in(USED, 0), effect_in(UNUSED, 0), effect_in(USED, 1)];
    commit(&maker, 1, &made, &[]);
    let (committed, own) = flushed(&maker);
    assert_eq!(committed, [1]);
    assert_eq!(own, []);
    let (_, streamed) = flushed(&user);
    assert_eq!(places(&streamed), [made[0].at, made[2].at]);
    assert!(streamed.iter().all(|filed| filed.due == Due::FromTaking));
    // Numbered by the commit's record and their place in it, which has only those
    // filed: the one into a chunk nobody uses is not written at all.
    let sequence = streamed[0].id.sequence;
    assert_eq!(
        ids(&streamed),
        [
            EffectId { sequence, index: 0 },
            EffectId { sequence, index: 1 }
        ]
    );
    assert_eq!(flushed(&home).1, []);
    let written: Vec<_> = logged(&disk)
        .into_iter()
        .filter_map(|record| match record {
            LogRecord::Commit { effects, .. } if !effects.is_empty() => Some(effects),
            _ => None,
        })
        .collect();
    assert_eq!(
        written,
        [vec![(made[0], Due::FromTaking), (made[2], Due::FromTaking)]]
    );
}

#[test]
fn an_effect_taken_is_not_sent_again_and_one_not_taken_is_after_a_hello() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (maker, _) = opened(&store, 0, 1);
    let (user, _) = opened(&store, 1, 1);
    load(&user, USED);
    commit(
        &maker,
        1,
        &[effect_in(USED, 0), effect_in(USED, 1), effect_in(USED, 2)],
        &[],
    );
    flushed(&maker);
    let (_, streamed) = flushed(&user);
    assert_eq!(streamed.len(), 3);

    // Taken: the first, and a number nobody filed, which takes nothing.
    let nobodys = EffectId {
        sequence: 1,
        index: 7,
    };
    commit(&user, 1, &[], &[streamed[0].id, nobodys]);
    assert_eq!(flushed(&user), (vec![1], Vec::new()));
    let taken: Vec<_> = logged(&disk)
        .into_iter()
        .filter_map(|record| match record {
            LogRecord::Commit { taken, .. } if !taken.is_empty() => Some(taken),
            _ => None,
        })
        .collect();
    assert_eq!(taken, [vec![streamed[0].id]]);

    let (again, restored) = opened(&store, 1, 2);
    assert_eq!(restored.in_use, [USED]);
    assert_eq!(flushed(&again).1, streamed[1..]);
}

#[test]
fn effects_takings_and_droppings_outlive_the_store() {
    let disk = Arc::new(MemoryDisk::default());
    {
        let store = store_on(&disk, &gap()).unwrap();
        let (maker, _) = opened(&store, 0, 1);
        let (user, _) = opened(&store, 1, 1);
        load(&user, USED);
        load(&user, UNUSED);
        commit(
            &maker,
            1,
            &[effect_in(USED, 0), effect_in(UNUSED, 0), effect_in(USED, 1)],
            &[],
        );
        flushed(&maker);
        let (_, streamed) = flushed(&user);
        commit(&user, 1, &[], &[streamed[0].id]);
        user.request(StoreRequest::Dropped {
            chunks: vec![UNUSED],
        });
        flushed(&user);
    }
    let left = Arc::new(disk.crashed(Survival::Nothing));
    let store = store_on(&left, &gap()).unwrap();
    let (user, restored) = opened(&store, 1, 2);
    // A start puts in use the chunks of the effects that are left, whose regions hold
    // them by being pinned.
    assert_eq!(restored.in_use, [USED]);
    assert_eq!(places(&flushed(&user).1), [effect_in(USED, 1).at]);
}

#[test]
fn a_chunk_returned_or_dropped_has_what_is_filed_for_it_dropped_and_nothing_filed_after() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (maker, _) = opened(&store, 0, 1);
    let (user, _) = opened(&store, 1, 1);
    // A claim begins the use of the chunks it grants.
    assert_eq!(claim(&user, &[FREE]).0, [FREE]);
    load(&user, USED);
    commit(&maker, 1, &[effect_in(FREE, 0), effect_in(USED, 0)], &[]);
    flushed(&maker);
    assert_eq!(flushed(&user).1.len(), 2);

    give_back(&user, &[FREE]);
    user.request(StoreRequest::Dropped { chunks: vec![USED] });
    flushed(&user);
    commit(&maker, 2, &[effect_in(FREE, 1), effect_in(USED, 1)], &[]);
    flushed(&maker);
    assert_eq!(flushed(&user).1, []);
    let dropped: Vec<_> = logged(&disk)
        .into_iter()
        .filter_map(|record| match record {
            LogRecord::EffectsDropped { ids } => Some(ids.len()),
            _ => None,
        })
        .collect();
    assert_eq!(dropped, [1, 1]);
    let (_, restored) = opened(&store, 1, 2);
    assert_eq!(restored.in_use, []);
}

#[test]
fn a_merge_moves_the_use_and_the_effects_to_the_survivor() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (survivor, _) = opened(&store, 0, 1);
    let (absorbed, _) = opened(&store, 1, 1);
    let (maker, _) = opened(&store, 2, 1);
    load(&absorbed, USED);
    commit(&maker, 1, &[effect_in(USED, 0)], &[]);
    flushed(&maker);
    flushed(&absorbed);

    match absorb(&survivor, 1, 1, 5) {
        StoreReply::Absorbed { in_use, .. } => assert_eq!(in_use, [USED]),
        other => panic!("expected the merge, got {other:?}"),
    }
    assert_eq!(places(&flushed(&survivor).1), [effect_in(USED, 0).at]);
    commit(&maker, 2, &[effect_in(USED, 1)], &[]);
    flushed(&maker);
    assert_eq!(places(&flushed(&survivor).1), [effect_in(USED, 1).at]);
}

#[test]
fn a_split_moves_the_use_of_the_parts_chunks_to_the_part() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (maker, _) = opened(&store, 0, 1);
    let (user, _) = opened(&store, 1, 1);
    load(&user, USED);
    load(&user, UNUSED);
    commit(&maker, 1, &[effect_in(USED, 0), effect_in(UNUSED, 0)], &[]);
    flushed(&maker);
    flushed(&user);

    assert_eq!(
        split(&user, 5, &[UNUSED], 1, 3),
        StoreReply::Split {
            region: RegionId(3)
        }
    );
    let (part, restored) = opened(&store, 3, 1);
    assert_eq!(restored.in_use, [UNUSED]);
    assert_eq!(places(&flushed(&part).1), [effect_in(UNUSED, 0).at]);
    commit(&maker, 2, &[effect_in(USED, 1), effect_in(UNUSED, 1)], &[]);
    flushed(&maker);
    assert_eq!(places(&flushed(&user).1), [effect_in(USED, 1).at]);
    assert_eq!(places(&flushed(&part).1), [effect_in(UNUSED, 1).at]);
}

#[test]
fn effects_in_a_segment_collection_removes_are_written_forward_first() {
    let disk = Arc::new(MemoryDisk::default());
    {
        let store = store_on(&disk, &gap()).unwrap();
        let (maker, _) = opened(&store, 0, 1);
        let (user, _) = opened(&store, 1, 1);
        load(&user, USED);
        commit(&maker, 1, &[effect_in(USED, 0), effect_in(USED, 1)], &[]);
        flushed(&maker);
        let (_, streamed) = flushed(&user);
        commit(&user, 1, &[], &[streamed[0].id]);
        flushed(&user);
        // Checkpoints that cover every commit let the segment of the effects go.
        maker.request(StoreRequest::Checkpoint {
            tick: 1,
            state: b"maker 1".to_vec(),
        });
        user.request(StoreRequest::Checkpoint {
            tick: 1,
            state: b"user 1".to_vec(),
        });
        flushed(&maker);
        flushed(&user);
        let records = logged(&disk);
        assert!(
            !records
                .iter()
                .any(|record| matches!(record, LogRecord::Commit { .. })),
            "{records:?}"
        );
        let forward: Vec<_> = records
            .into_iter()
            .filter_map(|record| match record {
                LogRecord::EffectsForward { effects } => Some(effects),
                _ => None,
            })
            .collect();
        assert_eq!(forward, [streamed[1..].to_vec()]);
    }
    let left = Arc::new(disk.crashed(Survival::Nothing));
    let store = store_on(&left, &gap()).unwrap();
    let (user, _) = opened(&store, 1, 2);
    let (_, resent) = flushed(&user);
    assert_eq!(places(&resent), [effect_in(USED, 1).at]);
    // Keeping its number, by which the region takes it.
    commit(&user, 2, &[], &ids(&resent));
    flushed(&user);
    let (again, _) = opened(&store, 1, 3);
    assert_eq!(flushed(&again).1, []);
}

#[test]
fn a_group_cut_off_takes_its_effects_takings_and_droppings_with_it() {
    let (store, disk) = switched_for(&gap());
    let (maker, _) = opened(&store, 0, 1);
    let (user, _) = opened(&store, 1, 1);
    load(&user, USED);
    load(&user, UNUSED);
    commit(&maker, 1, &[effect_in(USED, 0), effect_in(UNUSED, 0)], &[]);
    flushed(&maker);
    let (_, streamed) = flushed(&user);
    assert_eq!(streamed.len(), 2);

    disk.failing_syncs.store(true, Ordering::SeqCst);
    commit(&user, 1, &[], &[streamed[0].id]);
    user.request(StoreRequest::Dropped {
        chunks: vec![UNUSED],
    });
    commit(&maker, 2, &[effect_in(USED, 1)], &[]);
    user.request(StoreRequest::Flush);
    while user.replies.recv().is_ok() {}
    assert!(user.is_lost());
    disk.failing_syncs.store(false, Ordering::SeqCst);

    let (again, restored) = opened(&store, 1, 1);
    // Use is not in the log: the chunk dropped in the group cut off stays out of use,
    // and its effect stays filed until the region drops the chunk again.
    assert_eq!(restored.in_use, [USED]);
    let (_, resent) = flushed(&again);
    assert_eq!(resent, streamed[..1]);
}

/// What the scenario of the store that dies was told.
#[derive(Debug, Default)]
struct Told {
    /// Whether the user's load of its chunk was answered, by which its use began.
    loaded: bool,
    /// The places of the effects of each commit of the maker that was answered.
    confirmed: BTreeSet<BlockPos>,
    /// The places of the effects the user named as taken in a commit it sent.
    taken: BTreeSet<BlockPos>,
}

/// The maker commits effects into the user's chunk, which the user takes in part; both
/// make checkpoints that let the first segment go, which writes forward what is left.
fn scenario(store: &Store) -> Told {
    let mut told = Told::default();
    let open = |region| (0..3).find_map(|_| store.open_region(hello(region, 1)).ok());
    let (Some((maker, _)), Some((user, _))) = (open(0), open(1)) else {
        return told;
    };
    // Its use begins when the load is handled, which its answer shows.
    user.request(StoreRequest::Load { position: USED });
    told.loaded = everything_told(&user)
        .0
        .iter()
        .any(|reply| matches!(reply, StoreReply::Loaded { .. }));
    let mut answered = |handle: &StoreHandle, effects: &[(u64, Vec<Effect>)]| {
        let (committed, streamed) = flushed(handle);
        for (tick, made) in effects {
            if committed.contains(tick) {
                told.confirmed.extend(made.iter().map(|effect| effect.at));
            }
        }
        streamed
    };

    let first = vec![effect_in(USED, 0), effect_in(USED, 1), effect_in(USED, 2)];
    commit(&maker, 1, &first, &[]);
    answered(&maker, &[(1, first)]);
    let streamed = answered(&user, &[]);
    if let Some(filed) = streamed.first() {
        commit(&user, 1, &[], &[filed.id]);
        told.taken.insert(filed.effect.at);
    }
    let second = vec![effect_in(USED, 3)];
    commit(&maker, 2, &second, &[]);
    user.request(StoreRequest::Checkpoint {
        tick: 1,
        state: b"user 1".to_vec(),
    });
    maker.request(StoreRequest::Checkpoint {
        tick: 2,
        state: b"maker 2".to_vec(),
    });
    answered(&user, &[]);
    answered(&maker, &[(2, second)]);
    let third = vec![effect_in(USED, 4)];
    commit(&maker, 3, &third, &[]);
    answered(&maker, &[(3, third)]);
    told
}

/// The effects a store that died anywhere in the scenario hands the user: every effect
/// of a commit that was answered and not named as taken, and nothing else, once each.
#[test]
fn effects_survive_a_store_that_dies_anywhere() {
    let operations = {
        let disk = Arc::new(MemoryDisk::default());
        let store = store_on(&disk, &gap()).unwrap();
        let told = scenario(&store);
        assert_eq!(told.confirmed.len(), 5);
        disk.operations()
    };
    let made: BTreeSet<BlockPos> = (0..5).map(|n| effect_in(USED, n).at).collect();
    for n in 1..=operations + 1 {
        for fault in [Fault::Stop(n), Fault::Fail(n)] {
            let disk = Arc::new(MemoryDisk::failing(fault));
            let told = match store_on(&disk, &gap()) {
                Ok(store) => scenario(&store),
                Err(_) => Told::default(),
            };
            for survival in SURVIVALS {
                let case = format!("{fault:?}, {survival:?}");
                let left = Arc::new(disk.crashed(survival));
                let store =
                    store_on(&left, &gap()).unwrap_or_else(|error| panic!("{case}: {error}"));
                let (user, _) = store
                    .open_region(hello(1, 2))
                    .unwrap_or_else(|error| panic!("{case}: {error}"));
                let (_, resent) = flushed(&user);
                let places: Vec<BlockPos> = places(&resent);
                let unique: BTreeSet<BlockPos> = places.iter().copied().collect();
                assert_eq!(unique.len(), places.len(), "{case}: twice in {resent:?}");
                assert!(unique.is_subset(&made), "{case}: {resent:?}");
                let ids: Vec<EffectId> = resent.iter().map(|filed| filed.id).collect();
                assert!(ids.is_sorted(), "{case}: out of order: {resent:?}");
                if told.loaded {
                    for place in told.confirmed.difference(&told.taken) {
                        assert!(
                            unique.contains(place),
                            "{case}: {place:?} was confirmed and not taken, and is gone: {resent:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn a_store_started_drops_effects_whose_chunks_nobody_holds() {
    // An effect into the free chunk while it was claimed, filed, and the chunk
    // returned without the effect being dropped: a store from before this rule would
    // not leave that, but a world made over for another division does.
    let disk = Arc::new(MemoryDisk::default());
    {
        let store = store_on(&disk, &gap()).unwrap();
        let (maker, _) = opened(&store, 0, 1);
        let (user, _) = opened(&store, 1, 1);
        claim(&user, &[FREE]);
        commit(&maker, 1, &[effect_in(FREE, 0)], &[]);
        flushed(&maker);
        assert_eq!(flushed(&user).1.len(), 1);
    }
    // The world opened with region 1's area, and so the free chunk, given to nobody,
    // and the grant gone with the old table.
    let left = Arc::new(disk.crashed(Survival::Nothing));
    let told = Division {
        home: ChunkPos::new(0, 0),
        pinned: gap().pinned[..1].to_vec(),
    };
    let store = store_on(&left, &told).unwrap();
    let dropped = logged(&left)
        .into_iter()
        .any(|record| matches!(record, LogRecord::EffectsDropped { ids } if ids.len() == 1));
    assert!(dropped);
    drop(store);
    let again = Arc::new(left.crashed(Survival::Nothing));
    let store = store_on(&again, &told).unwrap();
    let (home, restored) = store.open_region(hello(1, 1)).unwrap();
    // Region 1 is the home region of that division, and holds the home chunk only.
    assert_eq!(restored.in_use, [ChunkPos::new(0, 0)]);
    assert_eq!(flushed(&home).1, []);
}
