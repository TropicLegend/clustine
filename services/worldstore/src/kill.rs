//! The store killed at every point: a scenario of commits, saves, checkpoints and the
//! opening of a region by a new owner, run on a simulated disk that fails, or stops for
//! good, at the `n`th change or sync, for every `n`. What a crash leaves of that disk is
//! then opened by a new store, and what it restores the region with is checked against
//! what was confirmed.

use std::collections::BTreeSet;
use std::path::Path;

use clustine_data::{BlockState, blocks};
use clustine_world::{BlockPos, Chunk, ChunkPos};

use super::*;
use crate::disk::{Fault, MemoryDisk, Survival};
use crate::tests::{generator, hello};

/// Who committed something: the first owner of the region, or the one that took it
/// over from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Owner {
    First,
    Second,
}

impl Owner {
    fn letter(self) -> char {
        match self {
            Owner::First => 'a',
            Owner::Second => 'b',
        }
    }

    /// What the owner sets the block of a tick to.
    fn block(self) -> BlockState {
        match self {
            Owner::First => blocks::STONE,
            Owner::Second => blocks::GLASS,
        }
    }
}

/// The block a commit of `tick` by `owner` changes: one per tick and owner, all in the
/// chunk at the origin. A block of the first owner is never changed by the second, so
/// that a change that got into a stored chunk without being committed stays in sight.
fn block(owner: Owner, tick: u64) -> BlockPos {
    let z = match owner {
        Owner::First => 3,
        Owner::Second => 5,
    };
    BlockPos::new(tick as i32, 100, z)
}

/// The block of a neighbouring region's commits.
fn neighbours_block(tick: u64) -> BlockPos {
    BlockPos::new(-(tick as i32), 100, 3)
}

const ORIGIN: ChunkPos = ChunkPos::new(0, 0);

fn delta(owner: Owner, tick: u64) -> Vec<u8> {
    format!("delta {} {tick}", owner.letter()).into_bytes()
}

fn whole(owner: Owner, tick: u64) -> Vec<u8> {
    format!("state {} {tick}", owner.letter()).into_bytes()
}

/// A store on `disk`, with its chunks in files there as well.
fn store_on(disk: Arc<MemoryDisk>) -> Result<Store, StoreError> {
    let root = Path::new("/world");
    let chunks = FileChunks::new(disk.clone(), root);
    start(disk, root, Box::new(chunks), generator())
}

fn commit(handle: &StoreHandle, tick: u64, position: BlockPos, state: BlockState, delta: Vec<u8>) {
    handle.request(StoreRequest::Commit {
        tick,
        changes: vec![(position, state)],
        state: delta,
    });
}

/// Asks for a flush and collects the answers until it is answered or the handle is
/// lost. Every commit that is answered counts as confirmed, also if the handle is lost
/// right after: whoever holds it was told.
fn settle(handle: &StoreHandle) -> (Vec<u64>, Vec<StoreReply>) {
    handle.request(StoreRequest::Flush);
    let mut committed = Vec::new();
    let mut others = Vec::new();
    while let Ok(reply) = handle.replies.recv() {
        match reply {
            StoreReply::Committed { tick } => committed.push(tick),
            StoreReply::Flushed => break,
            other => others.push(other),
        }
    }
    (committed, others)
}

/// What the scenario did and was told.
#[derive(Debug, Default)]
struct Told {
    /// The commits that were answered, by whom.
    confirmed: BTreeSet<(Owner, u64)>,
    /// The checkpoints asked for.
    checkpoints: Vec<(Owner, u64)>,
    /// What the second owner was restored with, if it got the region.
    second: Option<Restored>,
    /// The commits of the neighbouring region that were answered.
    neighbour: Vec<u64>,
}

/// Opens a region, trying again a few times: a fault may come in the middle of it.
fn opened(store: &Store, hello: RegionHello) -> Option<(StoreHandle, Restored)> {
    (0..3).find_map(|_| store.open_region(hello).ok())
}

/// The scenario. The first owner commits, saves and makes a checkpoint, and is replaced
/// by a second owner while a neighbouring region commits too; the second commits on
/// from where it was restored, saves and makes a checkpoint, and the first tries to
/// commit after it was replaced.
fn scenario(store: &Store) -> Told {
    let mut told = Told::default();
    let Some((neighbour, _)) = opened(store, hello(0, 1)) else {
        return told;
    };
    let Some((first, _)) = opened(store, hello(1, 1)) else {
        return told;
    };

    for tick in 1..=3 {
        commit(
            &first,
            tick,
            block(Owner::First, tick),
            blocks::STONE,
            delta(Owner::First, tick),
        );
        commit(
            &neighbour,
            tick,
            neighbours_block(tick),
            blocks::STONE,
            b"neighbour".to_vec(),
        );
    }
    // The chunk as of tick 3.
    let mut chunk = generator().generate(ORIGIN);
    for tick in 1..=3 {
        let (x, z) = block(Owner::First, tick).in_chunk();
        chunk.set(x, 100, z, blocks::STONE);
    }
    first.request(StoreRequest::Save {
        position: ORIGIN,
        tick: 3,
        chunk,
    });
    commit(
        &first,
        4,
        block(Owner::First, 4),
        blocks::STONE,
        delta(Owner::First, 4),
    );
    first.request(StoreRequest::Checkpoint {
        tick: 3,
        state: whole(Owner::First, 3),
    });
    told.checkpoints.push((Owner::First, 3));
    commit(
        &first,
        5,
        block(Owner::First, 5),
        blocks::STONE,
        delta(Owner::First, 5),
    );
    let (confirmed, _) = settle(&first);
    told.confirmed
        .extend(confirmed.into_iter().map(|tick| (Owner::First, tick)));
    told.neighbour.extend(settle(&neighbour).0);

    if let Some((second, restored)) = opened(store, hello(1, 2)) {
        // Too late: the region is not the first owner's any more.
        commit(
            &first,
            6,
            block(Owner::First, 6),
            blocks::STONE,
            delta(Owner::First, 6),
        );
        let from = restored
            .deltas
            .last()
            .map(|delta| delta.tick)
            .or(restored.state.as_ref().map(|state| state.tick))
            .unwrap_or(0);
        told.second = Some(restored);
        for tick in from + 1..=from + 2 {
            commit(
                &second,
                tick,
                block(Owner::Second, tick),
                blocks::GLASS,
                delta(Owner::Second, tick),
            );
        }
        // The second owner saves the chunk as it has it after its commits.
        second.request(StoreRequest::Load { position: ORIGIN });
        let (confirmed, replies) = settle(&second);
        told.confirmed
            .extend(confirmed.into_iter().map(|tick| (Owner::Second, tick)));
        if let Some(StoreReply::Loaded { mut chunk, .. }) = replies.into_iter().next() {
            for tick in from + 1..=from + 2 {
                let (x, z) = block(Owner::Second, tick).in_chunk();
                chunk.set(x, 100, z, blocks::GLASS);
            }
            second.request(StoreRequest::Save {
                position: ORIGIN,
                tick: from + 2,
                chunk,
            });
            second.request(StoreRequest::Checkpoint {
                tick: from + 1,
                state: whole(Owner::Second, from + 1),
            });
            told.checkpoints.push((Owner::Second, from + 1));
        }
        commit(
            &second,
            from + 3,
            block(Owner::Second, from + 3),
            blocks::GLASS,
            delta(Owner::Second, from + 3),
        );
        let (confirmed, _) = settle(&second);
        told.confirmed
            .extend(confirmed.into_iter().map(|tick| (Owner::Second, tick)));
    }
    let (confirmed, _) = settle(&first);
    told.confirmed
        .extend(confirmed.into_iter().map(|tick| (Owner::First, tick)));
    told
}

/// Who wrote what a region restored with `restored` has for each tick, given that the
/// second owner, if any, was restored up to `second_from`. Panics if the restored
/// region is not one history: a state from a checkpoint that was asked for, and after
/// it a commit for each tick, by one owner, as the first or the second had them.
fn history(restored: &Restored, told: &Told, second_from: Option<u64>) -> Vec<(u64, Owner)> {
    let mut history = Vec::new();
    let mut next = 1;
    if let Some(state) = &restored.state {
        let made = told
            .checkpoints
            .iter()
            .find(|(owner, tick)| *tick == state.tick && whole(*owner, *tick) == state.state);
        let &(owner, tick) = made.unwrap_or_else(|| panic!("a state nobody asked for: {state:?}"));
        for earlier in 1..=tick {
            let by = match (owner, second_from) {
                (Owner::Second, Some(from)) if earlier > from => Owner::Second,
                _ => Owner::First,
            };
            history.push((earlier, by));
        }
        next = tick + 1;
    }
    for restored_delta in &restored.deltas {
        assert_eq!(
            restored_delta.tick, next,
            "a tick is missing or twice: {restored:?}"
        );
        let by = [Owner::First, Owner::Second]
            .into_iter()
            .find(|owner| delta(*owner, restored_delta.tick) == restored_delta.state)
            .unwrap_or_else(|| panic!("a delta nobody committed: {restored_delta:?}"));
        history.push((restored_delta.tick, by));
        next += 1;
    }
    // The second owner goes on from where it was restored; what the first had beyond
    // that is not part of the history once the second has written anything.
    if let Some(from) = second_from
        && history.iter().any(|(_, by)| *by == Owner::Second)
    {
        assert!(
            history
                .iter()
                .all(|(tick, by)| *by == Owner::Second || *tick <= from),
            "the history mixes the owners: {history:?}"
        );
    }
    history
}

/// The chunk at the origin as a region with `history` has it.
fn expected_chunk(history: &[(u64, Owner)]) -> Chunk {
    let mut chunk = generator().generate(ORIGIN);
    for (tick, by) in history {
        let (x, z) = block(*by, *tick).in_chunk();
        chunk.set(x, 100, z, by.block());
    }
    chunk
}

fn the_restored_world_holds_everything_confirmed(fault: Fault, survival: Survival) {
    let disk = Arc::new(MemoryDisk::failing(fault));
    let told = match store_on(Arc::clone(&disk)) {
        Ok(store) => scenario(&store),
        Err(_) => Told::default(),
    };
    let case = format!("{fault:?}, {survival:?}");

    // The second owner was restored with everything the first was ever told was
    // committed: also what it was told after the second had opened the region, which
    // can only be what came before the hello.
    let second_from = told.second.as_ref().map(|restored| {
        let history = history(restored, &told, None);
        for (owner, tick) in &told.confirmed {
            if *owner == Owner::First {
                assert!(
                    history.contains(&(*tick, Owner::First)),
                    "{case}: commit {tick} was confirmed, the second owner was restored without it"
                );
            }
        }
        assert!(!told.confirmed.contains(&(Owner::First, 6)), "{case}");
        history.last().map_or(0, |(tick, _)| *tick)
    });

    let crashed = Arc::new(disk.crashed(survival));
    let store = store_on(crashed).unwrap_or_else(|error| panic!("{case}: {error}"));
    let (owner, restored) = store
        .open_region(hello(1, 3))
        .unwrap_or_else(|error| panic!("{case}: {error}"));
    let history = history(&restored, &told, second_from);
    for (by, tick) in &told.confirmed {
        assert!(
            history.contains(&(*tick, *by)),
            "{case}: commit {tick} of {by:?} was confirmed and is not in {history:?}"
        );
    }
    // What the stored chunks have agrees with the restored history: every change in
    // it, and nothing beyond it.
    owner.request(StoreRequest::Load { position: ORIGIN });
    let (_, replies) = settle(&owner);
    assert_eq!(
        replies,
        [StoreReply::Loaded {
            position: ORIGIN,
            chunk: expected_chunk(&history)
        }],
        "{case}: {history:?}"
    );

    let (neighbour, restored) = store
        .open_region(hello(0, 2))
        .unwrap_or_else(|error| panic!("{case}: {error}"));
    let ticks: Vec<u64> = restored.deltas.iter().map(|delta| delta.tick).collect();
    assert!(
        ticks.iter().copied().eq(1..=ticks.len() as u64),
        "{case}: {ticks:?}"
    );
    assert!(
        told.neighbour.iter().all(|tick| ticks.contains(tick)),
        "{case}"
    );
    neighbour.request(StoreRequest::Load {
        position: ChunkPos::new(-1, 0),
    });
    let (_, replies) = settle(&neighbour);
    let Some(StoreReply::Loaded { chunk, .. }) = replies.first() else {
        panic!("{case}: {replies:?}");
    };
    for tick in 1..=3 {
        let (x, z) = neighbours_block(tick).in_chunk();
        let expected = if ticks.contains(&tick) {
            blocks::STONE
        } else {
            blocks::AIR
        };
        assert_eq!(chunk.get(x, 100, z), Some(expected), "{case}: tick {tick}");
    }
}

/// How many changes and syncs the scenario makes when nothing goes wrong, with some to
/// spare for the ways the threads can interleave.
fn operations() -> u64 {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(Arc::clone(&disk)).unwrap();
    let told = scenario(&store);
    assert_eq!(told.confirmed.len(), 5 + 3, "{told:?}");
    disk.operations() + 10
}

#[test]
fn nothing_goes_wrong_without_a_fault() {
    for survival in [Survival::Nothing, Survival::Torn, Survival::Everything] {
        the_restored_world_holds_everything_confirmed(Fault::Fail(u64::MAX), survival);
    }
}

#[test]
fn a_store_that_stops_at_any_point_restores_everything_confirmed() {
    for n in 1..=operations() {
        for survival in [Survival::Nothing, Survival::Torn, Survival::Everything] {
            the_restored_world_holds_everything_confirmed(Fault::Stop(n), survival);
        }
    }
}

#[test]
fn a_store_that_fails_once_at_any_point_restores_everything_confirmed() {
    for n in 1..=operations() {
        for survival in [Survival::Nothing, Survival::Torn, Survival::Everything] {
            the_restored_world_holds_everything_confirmed(Fault::Fail(n), survival);
        }
    }
}
