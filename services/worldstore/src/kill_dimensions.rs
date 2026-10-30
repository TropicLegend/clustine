//! The store killed at every point of a world with regions of two dimensions
//! (`docs/groundwork/p-plan.md`, sections 3.3 and 15.4): an overworld region and a
//! Nether region change, save and claim chunks at the same positions, and the overworld
//! sends travellers into the Nether, one into land the Nether's region holds and one
//! into land nobody holds; the Nether's region takes the first, and both regions
//! checkpoint, which has the log write the second forward. What a crash at any point
//! leaves is opened by a new store, and each region is restored with its own chunks and
//! its own grants, and the Nether's with every traveller filed and not taken, once.
//!
//! The regions of the Nether and the End are their homes, which are made with the
//! world; the test makes the table before the store starts, so that the faults fall on
//! what the store does after.

use std::path::Path;

use clustine_data::blocks;
use clustine_format::{LogRecord, TableFile, read_log};
use clustine_world::{
    BlockPos, Change, Chunk, ChunkPos, Dimension, EffectId, LevelChunk, Travel, Traveller, Way,
};

use super::*;
use crate::disk::{Fault, MemoryDisk, Survival, replace};
use crate::entities_tests::item;
use crate::table::Table;
use crate::tests::{generator, hello, other_homes};

const ROOT: &str = "/world";
const ORIGIN: ChunkPos = ChunkPos::new(0, 0);
/// Claimed by both regions, each in its dimension.
const CLAIMED: ChunkPos = ChunkPos::new(5, 5);
/// Where the second traveller goes, in the Nether, which nobody holds before it.
const NOBODYS: ChunkPos = ChunkPos::new(9, 9);

/// The overworld's home region, which holds the overworld's chunk at the origin.
const OVERWORLD: RegionId = RegionId(0);
/// The Nether's home, which holds the Nether's chunk at the origin: the Nether's home
/// chunk.
const NETHER: RegionId = RegionId(1);
/// The End's home, which holds the End's chunk at the origin and nothing else.
const END: RegionId = RegionId(2);

/// The chunks are saved, and the regions checkpoint, as of the last tick, so that the
/// segment of the commits that sent the travellers can go.
const TICKS: u64 = 4;
const SAVED_AS_OF: u64 = TICKS;
const CHECKPOINT: u64 = TICKS;

/// The world of one home region in the overworld and the homes of the Nether and of
/// the End, with its table in place, on a disk that gets `fault` from then on.
fn planted(fault: Option<Fault>) -> Arc<MemoryDisk> {
    let disk = MemoryDisk::default();
    let division = Division::open(ORIGIN);
    assert_eq!(other_homes(&division), [NETHER.0, END.0]);
    let file = Table::made_from(&division, 0, 1).file(1);
    plant(&disk, &file);
    Arc::new(match fault {
        Some(fault) => disk.with(fault),
        None => disk,
    })
}

/// Puts `file` in place as the world's table, durably.
fn plant(disk: &MemoryDisk, file: &TableFile) {
    let regions = Path::new(ROOT).join("regions");
    disk.create_dir_all(&regions).unwrap();
    replace(disk, &regions.join("table"), &file.encode()).unwrap();
    disk.sync_directory(&regions).unwrap();
}

fn store_on(disk: Arc<MemoryDisk>) -> Result<Store, StoreError> {
    let root = Path::new(ROOT);
    let chunks = FileChunks::new(disk.clone(), root);
    start(
        disk,
        root,
        Box::new(chunks),
        generator(),
        &Division::open(ORIGIN),
    )
}

/// The block of the commit of `tick`, at the same place in either dimension.
fn block(tick: u64) -> BlockPos {
    BlockPos::new(tick as i32, 100, 3)
}

/// What a region of `dimension` sets its block of each tick to.
fn state_of(dimension: Dimension) -> clustine_data::BlockState {
    match dimension {
        Dimension::Overworld => blocks::STONE,
        _ => blocks::GLASS,
    }
}

/// The chunk at the origin of `dimension` with the changes of `ticks` applied.
fn chunk_after(dimension: Dimension, ticks: impl IntoIterator<Item = u64>) -> Chunk {
    let mut chunk = Generators::from(generator()).of(dimension).generate(ORIGIN);
    for tick in ticks {
        chunk
            .apply(&Change::Block(block(tick), state_of(dimension)))
            .expect("a block");
    }
    chunk
}

/// A traveller into the Nether's chunk `chunk`, an item of key `key`.
fn traveller(key: u64, chunk: ChunkPos) -> Traveller {
    let target = BlockPos::new(chunk.x * 16 + 8, 70, chunk.z * 16 + 8);
    let mut entity = item(key, f64::from(target.x), f64::from(target.z));
    entity.position[1] = 70.0;
    Traveller {
        travel: Travel {
            to: Dimension::Nether,
            target,
            way: Way::NetherPortal {
                axis: None,
                relative: Default::default(),
            },
        },
        chunk: LevelChunk::new(Dimension::Nether, chunk),
        entities: vec![entity],
    }
}

fn delta(region: RegionId, tick: u64) -> Vec<u8> {
    format!("delta {region} {tick}").into_bytes()
}

/// A commit of `tick` with `changes`, the travellers sent and the arrivals taken.
fn commit(
    tick: u64,
    region: RegionId,
    changes: Vec<Change>,
    travellers: Vec<Traveller>,
    arrivals_taken: Vec<EffectId>,
) -> StoreRequest {
    StoreRequest::Commit {
        tick,
        changes,
        state: delta(region, tick),
        stays: Vec::new(),
        ticks: Vec::new(),
        marks_taken: Vec::new(),
        seams: Vec::new(),
        effects: Vec::new(),
        taken: Vec::new(),
        events: Vec::new(),
        claims: Vec::new(),
        raids: Vec::new(),
        entity_changes: Vec::new(),
        arrivals: Vec::new(),
        arrivals_taken,
        travellers,
        clock: Default::default(),
    }
}

/// Collects the answers until a flush is answered or the handle is lost; returns the
/// commits confirmed and the other answers.
fn settle(handle: &StoreHandle) -> (Vec<u64>, Vec<StoreReply>) {
    handle.request(StoreRequest::Flush);
    let mut committed = Vec::new();
    let mut others = Vec::new();
    while let Ok(reply) = handle.replies.recv() {
        match reply {
            StoreReply::Committed { tick } => committed.push(tick),
            StoreReply::Flushed => break,
            StoreReply::Generating { .. } | StoreReply::Mark { .. } => {}
            other => others.push(other),
        }
    }
    (committed, others)
}

/// The travellers among `replies`, in their order.
fn travellers_in(replies: &[StoreReply]) -> Vec<(EffectId, Traveller)> {
    replies
        .iter()
        .filter_map(|reply| match reply {
            StoreReply::Travellers { travellers } => Some(travellers.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

/// Whether `replies` grant `chunk`.
fn granted(replies: &[StoreReply], chunk: ChunkPos) -> bool {
    replies.iter().any(
        |reply| matches!(reply, StoreReply::Claimed { granted, .. } if granted.contains(&chunk)),
    )
}

/// What the scenario saw confirmed.
#[derive(Debug, Default)]
struct Seen {
    overworld: Vec<u64>,
    nether: Vec<u64>,
    /// Whether each region was granted [`CLAIMED`].
    claimed: [bool; 2],
    /// Whether the Nether's region took the first traveller, in its commit of tick 1.
    took: bool,
}

/// The two regions change and save the chunk at the origin of their dimensions and
/// claim [`CLAIMED`]; the overworld sends a traveller into the Nether's land at tick 1
/// and one into land nobody holds at tick 2; the Nether's region takes the first in
/// its tick 1; both checkpoint.
fn scenario(store: &Store) -> Seen {
    let mut seen = Seen::default();
    let open = |region: RegionId| (0..3).find_map(|_| store.open_region(hello(region.0, 1)).ok());
    let (Some((overworld, _)), Some((nether, _))) = (open(OVERWORLD), open(NETHER)) else {
        return seen;
    };
    let claim = StoreRequest::Claim {
        chunks: vec![CLAIMED],
    };
    overworld.request(claim.clone());
    nether.request(claim);
    for tick in 1..=TICKS {
        let changes = vec![Change::Block(block(tick), state_of(Dimension::Overworld))];
        let travellers = match tick {
            1 => vec![traveller(901, ORIGIN)],
            2 => vec![traveller(902, NOBODYS)],
            _ => Vec::new(),
        };
        overworld.request(commit(tick, OVERWORLD, changes, travellers, Vec::new()));
    }
    let (confirmed, replies) = settle(&overworld);
    seen.overworld = confirmed;
    seen.claimed[0] = granted(&replies, CLAIMED);
    assert!(travellers_in(&replies).is_empty(), "the overworld's");

    // The Nether's region has the travellers on its stream by now.
    let (_, replies) = settle(&nether);
    seen.claimed[1] = granted(&replies, CLAIMED);
    let first = travellers_in(&replies)
        .into_iter()
        .find(|(_, traveller)| traveller.chunk.position == ORIGIN);
    seen.took = first.is_some();
    for tick in 1..=TICKS {
        let changes = vec![Change::Block(block(tick), state_of(Dimension::Nether))];
        let taken = match (tick, &first) {
            (1, Some((id, _))) => vec![*id],
            _ => Vec::new(),
        };
        nether.request(commit(tick, NETHER, changes, Vec::new(), taken));
    }
    for (handle, dimension) in [
        (&overworld, Dimension::Overworld),
        (&nether, Dimension::Nether),
    ] {
        handle.request(StoreRequest::Save {
            position: ORIGIN,
            tick: SAVED_AS_OF,
            chunk: chunk_after(dimension, 1..=SAVED_AS_OF),
            parts: clustine_rpc::Parts::ALL,
        });
        handle.request(StoreRequest::Checkpoint {
            tick: CHECKPOINT,
            state: b"state".to_vec(),
        });
    }
    seen.nether = settle(&nether).0;
    settle(&overworld);
    seen
}

/// The region's restored ticks, which have to be a checkpoint and the commits after it,
/// one a tick, among them every one confirmed. Returns the last.
fn restored_to(case: &str, restored: &Restored, confirmed: &[u64]) -> u64 {
    let from = restored.state.as_ref().map_or(0, |state| state.tick);
    let ticks: Vec<u64> = restored.deltas.iter().map(|delta| delta.tick).collect();
    assert!(
        ticks
            .iter()
            .copied()
            .eq(from + 1..=from + ticks.len() as u64),
        "{case}: from {from}, {ticks:?}"
    );
    let last = ticks.last().copied().unwrap_or(from);
    for tick in confirmed {
        assert!(
            *tick <= last,
            "{case}: {tick} was confirmed, restored to {last}"
        );
    }
    last
}

fn each_region_is_restored_with_its_own_chunks_grants_and_travellers(
    fault: Fault,
    survival: Survival,
) {
    let disk = planted(Some(fault));
    let seen = match store_on(Arc::clone(&disk)) {
        Ok(store) => scenario(&store),
        Err(_) => Seen::default(),
    };
    let case = format!("{fault:?}, {survival:?}");
    let crashed = Arc::new(disk.crashed(survival));
    let store = store_on(crashed).unwrap_or_else(|error| panic!("{case}: {error}"));

    let (overworld, restored) = store
        .open_region(hello(OVERWORLD.0, 2))
        .unwrap_or_else(|error| panic!("{case}: {error}"));
    assert_eq!(restored.dimension, Dimension::Overworld, "{case}");
    assert!(restored.spawn.is_some(), "{case}");
    let overworld_last = restored_to(&case, &restored, &seen.overworld);
    let held_by_overworld = restored.held.clone();
    let (nether, restored) = store
        .open_region(hello(NETHER.0, 2))
        .unwrap_or_else(|error| panic!("{case}: {error}"));
    assert_eq!(restored.dimension, Dimension::Nether, "{case}");
    assert_eq!(restored.spawn, None, "{case}: the spawn is the overworld's");
    let nether_last = restored_to(&case, &restored, &seen.nether);
    let held_by_nether = restored.held.clone();

    // Each region's grant of the same position, in its own dimension.
    for (claimed, held) in seen
        .claimed
        .iter()
        .zip([&held_by_overworld, &held_by_nether])
    {
        if *claimed {
            assert!(
                held.iter().any(|(chunk, _)| *chunk == CLAIMED),
                "{case}: {held:?}"
            );
        }
    }
    // The land the second traveller went to is the Nether region's with it.
    if overworld_last >= 2 {
        assert!(
            held_by_nether.iter().any(|(chunk, _)| *chunk == NOBODYS),
            "{case}: {held_by_nether:?}"
        );
    }
    assert!(
        !held_by_overworld.iter().any(|(chunk, _)| *chunk == NOBODYS),
        "{case}"
    );

    // The chunk at the origin of each dimension is its own region's history.
    overworld.request(StoreRequest::Load { position: ORIGIN });
    nether.request(StoreRequest::Load { position: ORIGIN });
    let (_, overworld_replies) = settle(&overworld);
    let (_, nether_replies) = settle(&nether);
    assert_eq!(
        overworld_replies,
        [StoreReply::Loaded {
            position: ORIGIN,
            chunk: chunk_after(Dimension::Overworld, 1..=overworld_last)
        }],
        "{case}: restored to {overworld_last}"
    );
    // The travellers not taken are on the Nether's stream at its hello, each once;
    // nothing of them reaches the overworld's.
    let travellers = travellers_in(&nether_replies);
    let loaded: Vec<&StoreReply> = nether_replies
        .iter()
        .filter(|reply| matches!(reply, StoreReply::Loaded { .. }))
        .collect();
    assert_eq!(
        loaded,
        [&StoreReply::Loaded {
            position: ORIGIN,
            chunk: chunk_after(Dimension::Nether, 1..=nether_last)
        }],
        "{case}: restored to {nether_last}"
    );
    let first_filed = overworld_last >= 1;
    let first_taken = seen.took && nether_last >= 1;
    let mut expected = Vec::new();
    if first_filed && !first_taken {
        expected.push(traveller(901, ORIGIN));
    }
    if overworld_last >= 2 {
        expected.push(traveller(902, NOBODYS));
    }
    let got: Vec<Traveller> = travellers
        .into_iter()
        .map(|(_, traveller)| traveller)
        .collect();
    assert_eq!(got, expected, "{case}: {seen:?}");
}

/// How many changes and syncs the scenario makes when nothing goes wrong, with some to
/// spare for the ways the threads can interleave.
fn operations() -> u64 {
    let disk = planted(None);
    let operations_before = disk.operations();
    let store = store_on(Arc::clone(&disk)).unwrap();
    let seen = scenario(&store);
    assert_eq!(seen.overworld, (1..=TICKS).collect::<Vec<_>>());
    assert_eq!(seen.nether, (1..=TICKS).collect::<Vec<_>>());
    assert!(seen.took && seen.claimed == [true, true], "{seen:?}");
    disk.operations() - operations_before + 10
}

const SURVIVALS: [Survival; 4] = [
    Survival::Nothing,
    Survival::Torn,
    Survival::Everything,
    Survival::Untruncated,
];

#[test]
fn regions_of_two_dimensions_are_restored_each_with_its_own_without_a_fault() {
    for survival in SURVIVALS {
        each_region_is_restored_with_its_own_chunks_grants_and_travellers(
            Fault::Fail(u64::MAX),
            survival,
        );
    }
}

#[test]
fn regions_of_two_dimensions_are_restored_each_with_its_own_after_a_stop_at_any_point() {
    for n in 1..=operations() {
        for survival in SURVIVALS {
            each_region_is_restored_with_its_own_chunks_grants_and_travellers(
                Fault::Stop(n),
                survival,
            );
        }
    }
}

#[test]
fn regions_of_two_dimensions_are_restored_each_with_its_own_after_one_failure_at_any_point() {
    for n in 1..=operations() {
        for survival in SURVIVALS {
            each_region_is_restored_with_its_own_chunks_grants_and_travellers(
                Fault::Fail(n),
                survival,
            );
        }
    }
}

/// The records of the log on `disk`, in the order of its segments.
fn records(disk: &MemoryDisk) -> Vec<LogRecord> {
    let mut segments: Vec<PathBuf> = disk
        .paths()
        .into_iter()
        .filter(|path| path.extension().is_some_and(|extension| extension == "wal"))
        .collect();
    segments.sort();
    segments
        .iter()
        .flat_map(|path| {
            let bytes = disk.read(path).unwrap().unwrap_or_default();
            read_log(&bytes).unwrap().0
        })
        .collect()
}

#[test]
fn a_traveller_not_taken_is_written_forward_when_its_segment_goes_and_the_chunks_lie_apart() {
    let disk = planted(None);
    let store = store_on(Arc::clone(&disk)).unwrap();
    let seen = scenario(&store);
    assert!(seen.took);
    store.close().unwrap();
    // The segment of the commit that sent the travellers went at the checkpoints, and
    // the one not taken lives in a record of its own.
    let records = records(&disk);
    let forward: Vec<&Traveller> = records
        .iter()
        .filter_map(|record| match record {
            LogRecord::TravellersForward { travellers } => Some(travellers),
            _ => None,
        })
        .flatten()
        .map(|(_, traveller)| traveller)
        .collect();
    assert_eq!(forward, [&traveller(902, NOBODYS)]);
    assert!(
        !records.iter().any(|record| matches!(
            record,
            LogRecord::Commit { travellers, .. } if !travellers.is_empty()
        )),
        "the commits that sent them are gone"
    );
    // Each dimension keeps its chunks in a directory of its own.
    let files = FileChunks::new(disk.clone(), Path::new(ROOT));
    for dimension in [Dimension::Overworld, Dimension::Nether] {
        let path = files.chunk_path(LevelChunk::new(dimension, ORIGIN));
        let bytes = disk.read(&path).unwrap().expect("the chunk is stored");
        let (file, _) = clustine_format::ChunkFile::decode(&bytes).unwrap();
        assert_eq!(file.chunk, chunk_after(dimension, 1..=SAVED_AS_OF));
    }
}

/// A traveller into land nobody holds goes to the home of its dimension, which is
/// granted the land (`docs/groundwork/p-plan.md`, section 3.3, R D12). Before the
/// homes were made with the world, the End had none that held its home chunk, and
/// the traveller waited, filed, for whoever claimed its chunk; a world has no such
/// End any more, and its table is refused (`Table::read`).
#[test]
fn a_traveller_into_land_nobody_holds_goes_to_the_home_of_its_dimension() {
    let disk = planted(None);
    let store = store_on(Arc::clone(&disk)).unwrap();
    let (overworld, _) = store.open_region(hello(OVERWORLD.0, 1)).unwrap();
    let (end, _) = store.open_region(hello(END.0, 1)).unwrap();
    // Nobody holds the End's chunk the traveller goes to: the End's home holds the
    // End's chunk at the origin alone.
    let far = ChunkPos::new(3, -3);
    let mut sent = traveller(77, far);
    sent.travel.to = Dimension::End;
    sent.chunk = LevelChunk::new(Dimension::End, far);
    overworld.request(commit(
        1,
        OVERWORLD,
        Vec::new(),
        vec![sent.clone()],
        Vec::new(),
    ));
    assert_eq!(settle(&overworld).0, [1]);
    // The End's home is granted the chunk, and has the traveller behind the grant.
    let (_, replies) = settle(&end);
    assert!(granted(&replies, far), "{replies:?}");
    let travellers = travellers_in(&replies);
    assert_eq!(travellers.len(), 1);
    assert_eq!(travellers[0].1, sent);
    // And again at a hello, until a commit of its takes it.
    drop(end);
    let (end, _) = store.open_region(hello(END.0, 2)).unwrap();
    let (_, replies) = settle(&end);
    assert_eq!(travellers_in(&replies), travellers);
    end.request(commit(
        1,
        END,
        Vec::new(),
        Vec::new(),
        vec![travellers[0].0],
    ));
    assert_eq!(settle(&end).0, [1]);
    drop(end);
    let (end, _) = store.open_region(hello(END.0, 3)).unwrap();
    assert_eq!(travellers_in(&settle(&end).1), []);
}

#[test]
fn a_change_of_division_folds_each_region_s_changes_into_its_dimension_and_never_a_traveller() {
    let disk = planted(None);
    let store = store_on(Arc::clone(&disk)).unwrap();
    let (overworld, _) = store.open_region(hello(OVERWORLD.0, 1)).unwrap();
    let (nether, _) = store.open_region(hello(NETHER.0, 1)).unwrap();
    let sent = traveller(55, ORIGIN);
    let at = |dimension| vec![Change::Block(block(1), state_of(dimension))];
    let changes = at(Dimension::Overworld);
    overworld.request(commit(
        1,
        OVERWORLD,
        changes,
        vec![sent.clone()],
        Vec::new(),
    ));
    assert_eq!(settle(&overworld).0, [1]);
    nether.request(commit(
        1,
        NETHER,
        at(Dimension::Nether),
        Vec::new(),
        Vec::new(),
    ));
    let (committed, replies) = settle(&nether);
    assert_eq!((committed, travellers_in(&replies).len()), (vec![1], 1));
    drop((overworld, nether));
    store.close().unwrap();
    drop(store);

    // Started with pins, the world is made over: what the regions committed goes into
    // the chunks of their dimensions, and their states go. The traveller is not put into
    // the Nether's chunk: it is written forward.
    let root = Path::new(ROOT);
    let pinned = Division::side_by_side(ORIGIN, &[0]).unwrap();
    let chunks = FileChunks::new(disk.clone(), root);
    let store = start(disk.clone(), root, Box::new(chunks), generator(), &pinned).unwrap();
    store.close().unwrap();
    drop(store);
    let files = FileChunks::new(disk.clone(), root);
    for dimension in [Dimension::Overworld, Dimension::Nether] {
        let path = files.chunk_path(LevelChunk::new(dimension, ORIGIN));
        let bytes = disk.read(&path).unwrap().expect("the chunk is stored");
        let (file, _) = clustine_format::ChunkFile::decode(&bytes).unwrap();
        assert_eq!(file.chunk, chunk_after(dimension, [1]), "{dimension:?}");
    }
    let nether_origin = LevelChunk::new(Dimension::Nether, ORIGIN);
    let stored = disk.read(&files.entities_path(nether_origin)).unwrap();
    assert_eq!(stored, None, "no entity of the traveller is in a chunk");
    let forward = records(&disk).into_iter().any(|record| {
        matches!(record, LogRecord::TravellersForward { travellers }
            if travellers.iter().any(|(_, traveller)| *traveller == sent))
    });
    assert!(forward, "the traveller is written forward");

    // The Nether's home of the world made over holds the chunk again, and is handed
    // the traveller at its hello.
    let file =
        TableFile::decode(&disk.read(&root.join("regions/table")).unwrap().unwrap()).unwrap();
    let [id, _] = other_homes(&pinned);
    assert_eq!(file.nether_home, id);
    let chunks = FileChunks::new(disk.clone(), root);
    let store = start(disk.clone(), root, Box::new(chunks), generator(), &pinned).unwrap();
    let (nether, restored) = store.open_region(hello(id, 1)).unwrap();
    assert_eq!(restored.held, [(ORIGIN, 0)]);
    let travellers = travellers_in(&settle(&nether).1);
    assert_eq!(travellers.len(), 1);
    assert_eq!(travellers[0].1, traveller(55, ORIGIN));
}
