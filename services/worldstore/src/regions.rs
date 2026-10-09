//! Tests of the regions the store keeps: the division it is started with, the table of
//! regions and who holds a chunk. See `docs/adr/0011-the-world-store-and-regions.md`.

use std::path::Path;
use std::sync::Barrier;

use clustine_data::{BlockState, blocks};
use clustine_format::{LogRecord, RegionFile, StateFile, TableFile};
use clustine_rpc::{ChunkBox, Decline, RegionInfo, RegionList, SplitPart, TickState};
use clustine_world::{BlockPos, Chunk, ChunkArea, ChunkPos, EntityIds};

use super::*;
use crate::disk::{Fault, MemoryDisk, Survival, replace};
use crate::tests::{division, edited, generator, hello, load, log, open, reply, save};

const ROOT: &str = "/world";

const ORIGIN: ChunkPos = ChunkPos::new(0, 0);

/// The chunk west of the origin, which the western of two stripes holds.
const WEST: ChunkPos = ChunkPos::new(-1, 0);

/// Every way a crash can leave what was not durable.
pub(crate) const SURVIVALS: [Survival; 4] = [
    Survival::Nothing,
    Survival::Torn,
    Survival::Everything,
    Survival::Untruncated,
];

/// A store on `disk` for a world divided as `division` says, with its chunks in files
/// there as well.
pub(crate) fn store_on(disk: &Arc<MemoryDisk>, division: &Division) -> Result<Store, StoreError> {
    let root = Path::new(ROOT);
    let chunks = FileChunks::new(disk.clone(), root);
    start(disk.clone(), root, Box::new(chunks), generator(), division)
}

/// A division with a gap: one area west of x = 0, one from x = 16 on, and the home
/// chunk at the origin. Regions 0 and 1 are pinned, and region 2 is home and holds the
/// home chunk and nothing else; the rest of the gap is nobody's.
pub(crate) fn gap() -> Division {
    Division {
        home: ORIGIN,
        pinned: vec![
            ChunkArea {
                min_x: None,
                max_x: Some(0),
            },
            ChunkArea {
                min_x: Some(16),
                max_x: None,
            },
        ],
    }
}

/// The hello for a region of a world divided as `division` says. A hello names a
/// region and an epoch and nothing of the division, which the tests go on handing in
/// so that each says which world its hello is for.
pub(crate) fn hello_of(_division: &Division, region: u32, epoch: u64) -> RegionHello {
    RegionHello {
        region: RegionId(region),
        epoch,
    }
}

/// Regions pinned side by side as stripes with these boundaries, with the home chunk
/// at the origin.
fn stripes(boundaries: &[i32]) -> Division {
    Division::side_by_side(ORIGIN, boundaries).unwrap()
}

/// The table file of the world on `disk`.
pub(crate) fn table_file(disk: &MemoryDisk) -> TableFile {
    let bytes = disk.read(Path::new("/world/regions/table")).unwrap();
    TableFile::decode(&bytes.expect("the world has a table")).unwrap()
}

/// The list without the epochs of the regions, which depend on who opened them.
pub(crate) fn listed(list: &RegionList) -> RegionList {
    let mut list = list.clone();
    for info in &mut list.regions {
        info.epoch = 0;
    }
    list
}

#[test]
fn a_new_world_has_the_regions_of_its_division() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let home = ChunkBox {
        min: ORIGIN,
        max: ORIGIN,
    };
    let expected = RegionList {
        home: RegionId(2),
        regions: vec![
            RegionInfo {
                region: RegionId(0),
                epoch: 0,
                bounds: None,
                pinned: gap().pinned[..1].to_vec(),
            },
            RegionInfo {
                region: RegionId(1),
                epoch: 0,
                bounds: None,
                pinned: gap().pinned[1..].to_vec(),
            },
            RegionInfo {
                region: RegionId(2),
                epoch: 0,
                bounds: Some(home),
                pinned: Vec::new(),
            },
        ],
        absorbed: Vec::new(),
        next: RegionId(3),
    };
    assert_eq!(store.regions().unwrap(), expected);

    // The table is on disk for good before the store has said anything.
    for survival in SURVIVALS {
        let file = table_file(&disk.crashed(survival));
        assert_eq!((file.from, file.next_region), (1, 3), "{survival:?}");
        assert_eq!((file.home_chunk, file.home_region), (ORIGIN, 2));
        assert_eq!(file.division, gap().pinned);
        assert_eq!(file.regions.len(), 3);
        assert_eq!(file.regions[2].grants, [(ORIGIN, 0)]);
    }

    // Each region is opened with what it is pinned to, and with a block of entity ids
    // of its own; the list has the epochs.
    let mut blocks = Vec::new();
    for region in 0..3 {
        let (_, restored) = store
            .open_region(hello_of(&gap(), region, u64::from(region) + 5))
            .unwrap();
        assert_eq!(restored.pinned, expected.regions[region as usize].pinned);
        assert_eq!((restored.state, restored.deltas), (None, Vec::new()));
        assert!(!blocks.contains(&restored.entity_ids));
        assert!((0..3).any(|index| EntityIds::block(index) == Some(restored.entity_ids)));
        blocks.push(restored.entity_ids);
    }
    let epochs: Vec<u64> = store
        .regions()
        .unwrap()
        .regions
        .iter()
        .map(|info| info.epoch)
        .collect();
    assert_eq!(epochs, [5, 6, 7]);
}

/// A hello makes no region: the regions are those the table has.
#[test]
fn a_region_the_world_does_not_have_is_refused_before_anything_is_written() {
    let directory = tempfile::tempdir().unwrap();
    for store in crate::tests::stores(directory.path()) {
        for region in [2, 9, u32::MAX] {
            let refused = store.open_region(hello(region, 1));
            assert!(
                matches!(
                    refused,
                    Err(StoreError::UnknownRegion { region: unknown }) if unknown == RegionId(region)
                ),
                "{:?}",
                refused.err()
            );
        }
        assert_eq!(store.regions().unwrap().regions.len(), 2);
    }
    assert!(!directory.path().join("regions/2.region").exists());
    assert!(!directory.path().join("regions/9.region").exists());

    // Over a connection it is refused in words.
    let store = crate::tests::memory();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let server = serve(store, listener).unwrap();
    let refused = StoreHandle::connect(&server.local_addr().to_string(), hello(2, 1));
    let reason = StoreError::UnknownRegion {
        region: RegionId(2),
    };
    assert!(
        matches!(&refused, Err(StoreError::Refused(given)) if *given == reason.to_string()),
        "{:?}",
        refused.err()
    );
}

#[test]
fn a_division_whose_areas_overlap_starts_no_store() {
    let overlapping = Division {
        pinned: vec![ChunkArea::EVERYWHERE, gap().pinned[0]],
        ..gap()
    };
    let disk = Arc::new(MemoryDisk::default());
    for started in [
        Store::memory_divided(generator(), overlapping.clone()),
        store_on(&disk, &overlapping),
    ] {
        assert!(matches!(
            started,
            Err(StoreError::Division {
                first: RegionId(0),
                second: RegionId(1)
            })
        ));
    }
    // And nothing was written for it.
    assert_eq!(disk.operations(), 0);
}

/// Scenario 1 of ADR-0011: a region loads and saves a chunk of its stripe without
/// claiming; the other gets `NotHeld` for both, and the chunk is unchanged.
#[test]
fn only_the_holder_loads_and_saves_a_chunk() {
    let directory = tempfile::tempdir().unwrap();
    for store in crate::tests::stores(directory.path()) {
        let west = open(&store, hello(0, 1));
        let east = open(&store, hello(1, 1));
        save(&east, ORIGIN, &edited());
        assert_eq!(load(&east, ORIGIN), edited());

        let not_held = StoreReply::NotHeld {
            position: ORIGIN,
            holder: Some(RegionId(1)),
        };
        west.request(StoreRequest::Load { position: ORIGIN });
        assert_eq!(reply(&west), not_held);
        save(&west, ORIGIN, &generator().generate(ORIGIN));
        assert_eq!(reply(&west), not_held);
        // Nor does it wait behind a commit, as a save that is done would.
        log(&west, 1, &[(-3, 100, 4, blocks::STONE)]);
        save(&west, ORIGIN, &generator().generate(ORIGIN));
        west.request(StoreRequest::Flush);
        assert_eq!(reply(&west), not_held);
        assert_eq!(reply(&west), StoreReply::Flushed);
        assert!(!west.is_lost());
        assert_eq!(load(&east, ORIGIN), edited());

        // The other way round, with the holder named as well.
        east.request(StoreRequest::Load { position: WEST });
        let not_held = StoreReply::NotHeld {
            position: WEST,
            holder: Some(RegionId(0)),
        };
        assert_eq!(reply(&east), not_held);
        assert_eq!(load(&west, WEST), generator().generate(WEST));
    }

    // A chunk nobody holds is loaded and saved by nobody, and has no holder to name.
    let store = Store::memory_divided(generator(), gap()).unwrap();
    let free = ChunkPos::new(5, 5);
    for region in 0..3 {
        let handle = open(&store, hello_of(&gap(), region, 1));
        handle.request(StoreRequest::Load { position: free });
        let not_held = StoreReply::NotHeld {
            position: free,
            holder: None,
        };
        assert_eq!(reply(&handle), not_held);
        // The home chunk is the home region's alone.
        handle.request(StoreRequest::Load { position: ORIGIN });
        let answer = reply(&handle);
        if region == 2 {
            assert!(matches!(answer, StoreReply::Loaded { .. }));
        } else {
            let not_held = StoreReply::NotHeld {
                position: ORIGIN,
                holder: Some(RegionId(2)),
            };
            assert_eq!(answer, not_held);
        }
    }
}

/// A world of two stripes in which the western region has a commit in the log and the
/// eastern one a saved chunk, a state and a commit after it.
fn lived_in(disk: &Arc<MemoryDisk>) {
    let store = store_on(disk, &division()).unwrap();
    let west = open(&store, hello(0, 3));
    let east = open(&store, hello(1, 4));
    log(&west, 1, &[(-3, -61, 4, blocks::AIR)]);
    log(&east, 1, &[(3, -61, 4, blocks::AIR)]);
    let mut saved = generator().generate(ORIGIN);
    saved.set(3, -61, 4, blocks::AIR);
    save(&east, ORIGIN, &saved);
    east.request(StoreRequest::Checkpoint {
        tick: 1,
        state: b"east".to_vec(),
    });
    log(&east, 2, &[(3, 100, 4, blocks::GLASS)]);
    west.flush();
    east.flush();
}

/// The chunk west of the origin as the western region of [`lived_in`] left it.
fn dug() -> Chunk {
    let mut dug = generator().generate(WEST);
    dug.set(13, -61, 4, blocks::AIR);
    dug
}

/// What each of the two regions of [`lived_in`] is restored with.
fn as_lived_in(store: &Store, epoch: u64, case: &str) {
    let (west, restored) = store.open_region(hello(0, epoch)).unwrap();
    assert_eq!(restored.state, None, "{case}");
    assert_eq!(restored.deltas.len(), 1, "{case}");
    assert_eq!(load(&west, WEST), dug(), "{case}");
    let (east, restored) = store.open_region(hello(1, epoch)).unwrap();
    let state = TickState {
        tick: 1,
        state: b"east".to_vec(),
    };
    assert_eq!(restored.state, Some(state), "{case}");
    let ticks: Vec<u64> = restored.deltas.iter().map(|delta| delta.tick).collect();
    assert_eq!(ticks, [2], "{case}");
    assert_eq!(load(&east, ORIGIN), edited(), "{case}");
    west.flush();
    east.flush();
}

/// The world of [`lived_in`] made over for `told`: its regions are those of `told`
/// and are restored with nothing, and what was built is in the chunks as their holders
/// load them. `used` is the next region id of the table the world had, 0 if it had
/// none: the ids below it are not given out again, so the list's next id is above
/// them also where `told` has fewer regions.
fn as_made_over(store: &Store, told: &Division, used: u32, epoch: u64, case: &str) {
    let expected = table::Table::made_from(told, used, 1).list(|_| 0);
    let list = store.regions().unwrap();
    assert_eq!(listed(&list), listed(&expected), "{case}");
    let mut loaded = Vec::new();
    for info in &list.regions {
        let (handle, restored) = store
            .open_region(hello_of(told, info.region.0, epoch))
            .unwrap_or_else(|error| panic!("{case}: {error}"));
        assert_eq!((restored.state, restored.deltas), (None, Vec::new()));
        for position in [WEST, ORIGIN] {
            handle.request(StoreRequest::Load { position });
            match reply(&handle) {
                StoreReply::Loaded { position, chunk } => loaded.push((position, chunk)),
                StoreReply::NotHeld { .. } => {}
                other => panic!("{case}: {other:?}"),
            }
        }
        handle.flush();
    }
    assert_eq!(loaded, [(WEST, dug()), (ORIGIN, edited())], "{case}");
}

/// Starts a store for `told` on what a crash leaves of `world`, killed at every change
/// and sync of that start and with everything a crash can keep of it; starts one on
/// what is left, which has to be as `check` says, and one more on what that one left,
/// which has to be the same. `check` is given the epoch to open regions with.
fn started_at_every_kill_point(
    world: &MemoryDisk,
    told: &Division,
    check: impl Fn(&Store, u64, &str),
) {
    let whole = Survival::Everything;
    let operations = {
        let disk = Arc::new(world.crashed(whole));
        store_on(&disk, told).unwrap();
        disk.operations()
    };
    for n in 1..=operations + 1 {
        for fault in [Fault::Stop(n), Fault::Fail(n), Fault::Fails(n, 2)] {
            let disk = Arc::new(world.crashed(whole).with(fault));
            // It fails or not; either way it has done what it has.
            drop(store_on(&disk, told));
            for survival in SURVIVALS {
                let case = format!("{fault:?}, {survival:?}");
                let left = Arc::new(disk.crashed(survival));
                let store = store_on(&left, told).unwrap_or_else(|error| panic!("{case}: {error}"));
                check(&store, 100, &case);
                assert_eq!(left.read(Path::new("/world/layout")).unwrap(), None);
                let again = Arc::new(left.crashed(Survival::Nothing));
                let store =
                    store_on(&again, told).unwrap_or_else(|error| panic!("{case}: {error}"));
                check(&store, 101, &format!("{case}, again"));
            }
        }
    }
}

#[test]
fn a_store_started_with_the_same_division_finds_the_world_as_it_was() {
    let disk = Arc::new(MemoryDisk::default());
    lived_in(&disk);
    let table = disk.read(Path::new("/world/regions/table")).unwrap();
    for (epoch, survival) in (10..).zip(SURVIVALS) {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &division()).unwrap();
        as_lived_in(&store, epoch, &format!("{survival:?}"));
        // The table is not written again.
        assert_eq!(left.read(Path::new("/world/regions/table")).unwrap(), table);
        let list = store.regions().unwrap();
        let epochs: Vec<u64> = list.regions.iter().map(|info| info.epoch).collect();
        assert_eq!(epochs, [epoch, epoch]);
    }
    // Killed at any point, such a start leaves the world as it was as well.
    started_at_every_kill_point(&disk, &division(), as_lived_in);
}

/// Other areas, or the same areas and another home chunk: the regions of the old
/// division mean nothing to those of the new one, but what was built belongs to the
/// world. Also when the store dies at any point of making the world over.
#[test]
fn a_store_started_with_another_division_makes_the_world_over() {
    let disk = Arc::new(MemoryDisk::default());
    lived_in(&disk);
    let moved_home = Division {
        home: ChunkPos::new(-5, 0),
        ..division()
    };
    for told in [
        stripes(&[]),
        stripes(&[0, 16]),
        stripes(&[-8]),
        moved_home,
        gap_at_the_east(),
    ] {
        // The world that was lived in has a table, with two regions.
        started_at_every_kill_point(&disk, &told, |store, epoch, case| {
            as_made_over(store, &told, 2, epoch, case);
        });
    }
}

/// A division whose home region is not pinned: one area west of x = 4, and the home
/// chunk east of it in nobody's area.
fn gap_at_the_east() -> Division {
    Division {
        home: ChunkPos::new(9, 9),
        pinned: vec![ChunkArea {
            min_x: None,
            max_x: Some(4),
        }],
    }
}

/// The ids of the stripes are used again by the stripes of another division, with the
/// epochs their region files have and the entity ids they were issued; and no block of
/// entity ids is issued twice, whatever has become of the region that has one.
#[test]
fn epochs_and_entity_ids_outlive_a_change_of_the_division() {
    let disk = Arc::new(MemoryDisk::default());
    let opened = |told: &Division, region: u32, epoch: u64| {
        let store = store_on(&disk, told).unwrap();
        let opened = store.open_region(hello_of(told, region, epoch));
        opened.map(|(handle, restored)| {
            handle.flush();
            restored.entity_ids
        })
    };
    let two = stripes(&[0]);
    let west = opened(&two, 0, 3).unwrap();
    let east = opened(&two, 1, 4).unwrap();
    assert_ne!(west, east);

    // One region: it is region 0 as the western stripe was.
    let one = stripes(&[]);
    assert!(matches!(
        opened(&one, 0, 2),
        Err(StoreError::EpochRefused { seen: 3, .. })
    ));
    assert_eq!(opened(&one, 0, 3).unwrap(), west);
    assert!(matches!(
        opened(&one, 1, 9),
        Err(StoreError::UnknownRegion { .. })
    ));

    // Three: the third is new, and its block is not the one the eastern stripe has,
    // whose file was kept although the region was gone for a while.
    let three = stripes(&[0, 16]);
    let third = opened(&three, 2, 1).unwrap();
    assert!(third != west && third != east);
    assert_eq!(opened(&three, 1, 4).unwrap(), east);
    assert!(matches!(
        opened(&three, 1, 3),
        Err(StoreError::EpochRefused { seen: 4, .. })
    ));
    // And back to two, after which a region of yet another division gets a fourth.
    assert_eq!(opened(&two, 0, 5).unwrap(), west);
    let home = opened(&gap(), 2, 1).unwrap();
    assert_eq!(home, third);
    let four = stripes(&[0, 16, 32]);
    let fourth = opened(&four, 3, 1).unwrap();
    assert!(![west, east, third].contains(&fourth));
}

/// Writes `contents` to the file at `path` of `disk`, for good.
pub(crate) fn put(disk: &MemoryDisk, path: &str, contents: &[u8]) {
    let path = Path::new(path);
    replace(disk, path, contents).unwrap();
    disk.sync_directory(path.parent().unwrap()).unwrap();
}

/// A world as a store from before there was a table left it, which no store writes any
/// more: two stripes divided at x = 0 that have what the regions of [`lived_in`] have,
/// the file `layout`, and no table.
pub(crate) fn world_of_today() -> Arc<MemoryDisk> {
    let disk = Arc::new(MemoryDisk::default());
    let file = |epoch, block| {
        let entity_ids = EntityIds::block(block).unwrap();
        RegionFile { epoch, entity_ids }.encode()
    };
    put(&disk, "/world/regions/0.region", &file(3, 0));
    put(&disk, "/world/regions/1.region", &file(4, 1));
    let state = StateFile {
        tick: 1,
        state: b"east".to_vec(),
    };
    put(&disk, "/world/regions/1.state", &state.encode());
    let commit = |region, tick, epoch, x, y, block| LogRecord::Commit {
        region,
        tick,
        epoch,
        changes: vec![(BlockPos::new(x, y, 4), block)],
        state: crate::tests::delta(tick),
    };
    let opened = |region, epoch| LogRecord::Opened {
        region,
        epoch,
        restored: 0,
    };
    let records = [
        opened(0, 3),
        opened(1, 4),
        commit(0, 1, 3, -3, -61, blocks::AIR),
        commit(1, 1, 4, 3, -61, blocks::AIR),
        commit(1, 2, 4, 3, 100, blocks::GLASS),
    ];
    let log: Vec<u8> = records.iter().flat_map(LogRecord::encode).collect();
    put(&disk, "/world/log/00000000000000000001.wal", &log);
    // The chunk the eastern region saved before its checkpoint, which covers its
    // first commit.
    let mut chunks = FileChunks::new(disk.clone(), Path::new(ROOT));
    let mut saved = generator().generate(ORIGIN);
    saved.set(3, -61, 4, blocks::AIR);
    chunks.save(ORIGIN, 1, &saved).unwrap();
    chunks.sync().unwrap();
    // What such a store wrote there for stripes divided at x = 0. Nothing reads it.
    put(&disk, "/world/layout", b"9c191507aacacf62\n");
    disk
}

/// A world from before there was a table is made over, whether it is started with the
/// stripes it had, with others, or with a division that is none: nothing says any
/// more how it was divided (`docs/adr/0017-the-end-of-the-stripes.md`, section 2.2).
#[test]
fn a_world_of_today_is_made_over_whatever_it_is_started_with() {
    let world = world_of_today();
    for told in [division(), stripes(&[]), stripes(&[0, 16]), gap()] {
        // A world of today has no table.
        started_at_every_kill_point(&world, &told, |store, epoch, case| {
            as_made_over(store, &told, 0, epoch, case);
        });
    }
}

/// A layout file that a store left because it died between making the table durable
/// and removing the file goes at the next start, and changes nothing.
#[test]
fn a_layout_file_beside_a_table_is_removed() {
    let disk = Arc::new(MemoryDisk::default());
    lived_in(&disk);
    // By itself the file would make a world without a table over.
    put(&disk, "/world/layout", b"4d25767f9dce13f5\n");
    let left = Arc::new(disk.crashed(Survival::Nothing));
    let store = store_on(&left, &division()).unwrap();
    as_lived_in(&store, 10, "a layout file beside a table");
    for survival in SURVIVALS {
        let read = left.crashed(survival).read(Path::new("/world/layout"));
        assert_eq!(read.unwrap(), None, "{survival:?}");
    }
}

#[test]
fn a_table_that_cannot_be_read_starts_no_store() {
    let disk = Arc::new(MemoryDisk::default());
    lived_in(&disk);
    let path = Path::new("/world/regions/table");
    let mut table = disk.read(path).unwrap().unwrap();
    table[12] ^= 0x01;
    let damaged = Arc::new(disk.crashed(Survival::Everything));
    put(&damaged, "/world/regions/table", &table);
    assert!(matches!(
        store_on(&damaged, &division()),
        Err(StoreError::Damaged { path: at, .. }) if at == path
    ));

    // One that can be read and says what cannot be: the home region is none.
    let mut file = table_file(&disk);
    file.home_region = 7;
    let impossible = Arc::new(disk.crashed(Survival::Everything));
    put(&impossible, "/world/regions/table", &file.encode());
    assert!(matches!(
        store_on(&impossible, &division()),
        Err(StoreError::Table(_))
    ));
}

/// A chunk in the gap of [`gap`], which nobody holds until it is claimed.
const FREE: ChunkPos = ChunkPos::new(5, 5);

/// Claims `chunks` and waits for the answer: those granted, and those of other regions.
pub(crate) fn claim(
    handle: &StoreHandle,
    chunks: &[ChunkPos],
) -> (Vec<ChunkPos>, Vec<(ChunkPos, RegionId)>) {
    handle.request(StoreRequest::Claim {
        chunks: chunks.to_vec(),
    });
    match reply(handle) {
        StoreReply::Claimed { granted, foreign } => (granted, foreign),
        other => panic!("expected the answer to a claim, got {other:?}"),
    }
}

pub(crate) fn give_back(handle: &StoreHandle, chunks: &[ChunkPos]) {
    handle.request(StoreRequest::Return {
        chunks: chunks.to_vec(),
    });
}

/// Opens a region of the world with a gap, and returns the handle with what the region
/// was granted.
fn opened(store: &Store, region: u32, epoch: u64) -> (StoreHandle, Vec<(ChunkPos, u64)>) {
    let (handle, restored) = store.open_region(hello_of(&gap(), region, epoch)).unwrap();
    (handle, restored.held)
}

/// What each region of the world with a gap that is left on `disk` was granted, as a
/// store that starts on it says when the regions are opened with `epoch`.
fn held_after(disk: &MemoryDisk, survival: Survival, epoch: u64) -> Vec<Vec<(ChunkPos, u64)>> {
    let left = Arc::new(disk.crashed(survival));
    let store = store_on(&left, &gap()).unwrap();
    let regions = store.regions().unwrap().regions;
    regions
        .iter()
        .map(|info| opened(&store, info.region.0, epoch).1)
        .collect()
}

/// The chunk at `position` with a block set that the flat world does not have there.
fn built(position: ChunkPos, state: BlockState) -> Chunk {
    let mut chunk = generator().generate(position);
    chunk.set(7, 100, 7, state);
    chunk
}

/// The block [`built`] sets, in the chunk at `position`.
fn block_of(position: ChunkPos) -> (i32, i32, i32) {
    (position.x * 16 + 7, 100, position.z * 16 + 7)
}

/// Keeps chunks as `chunks` does, but a save of one of the chunks in `gated` waits
/// until the test lets it go on, keeping the thread for chunks busy.
struct Gated<C> {
    chunks: C,
    gated: Vec<ChunkPos>,
    barrier: Arc<Barrier>,
}

impl<C: Chunks> Chunks for Gated<C> {
    fn load(&mut self, position: ChunkPos) -> Result<Option<Chunk>, StoreError> {
        self.chunks.load(position)
    }

    fn save(&mut self, position: ChunkPos, tick: u64, chunk: &Chunk) -> Result<(), StoreError> {
        if self.gated.contains(&position) {
            // Once to say that the save has started, once to be let go on.
            self.barrier.wait();
            self.barrier.wait();
        }
        self.chunks.save(position, tick, chunk)
    }

    fn sync(&mut self) -> Result<(), StoreError> {
        self.chunks.sync()
    }
}

/// A store for the world with a gap on `disk`, whose saves of the chunks in `gated`
/// wait for the barrier.
fn gated(disk: &Arc<MemoryDisk>, gated: &[ChunkPos]) -> (Store, Arc<Barrier>) {
    let barrier = Arc::new(Barrier::new(2));
    let root = Path::new(ROOT);
    let chunks = Gated {
        chunks: FileChunks::new(disk.clone(), root),
        gated: gated.to_vec(),
        barrier: Arc::clone(&barrier),
    };
    let store = start(disk.clone(), root, Box::new(chunks), generator(), &gap()).unwrap();
    (store, barrier)
}

/// The chunk of the eastern pinned region whose save holds the thread for chunks.
const BUSY: ChunkPos = ChunkPos::new(1000, 1000);

/// Scenario 2 of ADR-0011: a claim of a chunk in the region's own stripe is granted,
/// writes nothing and adds nothing to what the region is restored with as granted; one
/// in the other's stripe is `foreign` with that region.
#[test]
fn a_pinned_region_is_answered_from_the_table_without_a_record() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let west = open(&store, hello(0, 1));
    let east = open(&store, hello(1, 1));
    west.flush();
    east.flush();
    let before = disk.operations();
    // Each chunk once, in the order of the request, however often it is named.
    let far = ChunkPos::new(40, -40);
    assert_eq!(
        claim(&east, &[ORIGIN, WEST, far, ORIGIN, WEST]),
        (vec![ORIGIN, far], vec![(WEST, RegionId(0))])
    );
    assert_eq!(
        claim(&west, &[ORIGIN, WEST]),
        (vec![WEST], vec![(ORIGIN, RegionId(1))])
    );
    assert_eq!(disk.operations(), before);
    // Giving back what was never granted does nothing either.
    give_back(&east, &[ORIGIN, WEST]);
    east.flush();
    assert_eq!(disk.operations(), before);
    assert_eq!(load(&east, ORIGIN), generator().generate(ORIGIN));
    drop((west, east));
    for region in 0..2 {
        let (_, restored) = store.open_region(hello(region, 2)).unwrap();
        assert_eq!(restored.held, []);
    }
    let list = store.regions().unwrap();
    assert!(list.regions.iter().all(|info| info.bounds.is_none()));
}

/// Scenario 3: a chunk nobody holds is granted once; the second region to ask gets
/// `foreign`, also when both ask in one group; and a store that starts again has the
/// chunk for the first and not for the second.
#[test]
fn a_free_chunk_is_granted_to_the_first_region_that_claims_it() {
    let (store, disk) = crate::tests::switched_for(&gap());
    let (first, held) = opened(&store, 0, 1);
    let (second, _) = opened(&store, 1, 1);
    assert_eq!(held, []);
    assert_eq!(claim(&first, &[FREE]), (vec![FREE], Vec::new()));
    assert_eq!(
        claim(&second, &[FREE]),
        (Vec::new(), vec![(FREE, RegionId(0))])
    );
    // Claimed again by its holder, it is among those granted, and nothing changes.
    assert_eq!(claim(&first, &[FREE]), (vec![FREE], Vec::new()));
    assert_eq!(load(&first, FREE), generator().generate(FREE));
    second.request(StoreRequest::Load { position: FREE });
    let not_held = StoreReply::NotHeld {
        position: FREE,
        holder: Some(RegionId(0)),
    };
    assert_eq!(reply(&second), not_held);

    // Two claims of one chunk in one group, which the held sync of a commit makes of
    // them: the grant is not durable when the second claim is looked at.
    let other = ChunkPos::new(6, 5);
    disk.holding_syncs.store(true, Ordering::SeqCst);
    log(&first, 1, &[]);
    disk.held.wait();
    second.request(StoreRequest::Claim {
        chunks: vec![other],
    });
    first.request(StoreRequest::Claim {
        chunks: vec![other, FREE],
    });
    disk.held.wait();
    let granted = StoreReply::Claimed {
        granted: vec![other],
        foreign: Vec::new(),
    };
    assert_eq!(reply(&second), granted);
    let foreign = StoreReply::Claimed {
        granted: vec![FREE],
        foreign: vec![(other, RegionId(1))],
    };
    assert_eq!(reply(&first), foreign);

    // The list has the box around what each was granted.
    let list = store.regions().unwrap();
    let bounds: Vec<_> = list.regions.iter().map(|info| info.bounds).collect();
    let around = |chunk| {
        Some(ChunkBox {
            min: chunk,
            max: chunk,
        })
    };
    assert_eq!(bounds, [around(FREE), around(other), around(ORIGIN)]);
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        // The grant of the first was made before it had committed anything; that of
        // the second as well.
        let held = held_after(&disk.disk, survival, epoch);
        assert_eq!(
            held,
            [vec![(FREE, 0)], vec![(other, 0)], vec![(ORIGIN, 0)]],
            "{survival:?}"
        );
    }
}

/// Scenario 4: a claim is answered only after the commits asked for before it, and is
/// in the log whenever it was answered.
#[test]
fn a_claim_is_answered_behind_the_commits_before_it_and_is_durable_by_then() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (region, _) = opened(&store, 0, 1);
    for tick in 1..=3 {
        log(&region, tick, &[]);
    }
    region.request(StoreRequest::Claim { chunks: vec![FREE] });
    log(&region, 4, &[]);
    let mut answers = Vec::new();
    for _ in 0..5 {
        answers.push(crate::tests::any_reply(&region));
    }
    let claimed = StoreReply::Claimed {
        granted: vec![FREE],
        foreign: Vec::new(),
    };
    let committed = |tick| StoreReply::Committed { tick };
    assert_eq!(
        answers,
        [
            committed(1),
            committed(2),
            committed(3),
            claimed,
            committed(4)
        ]
    );
    // A crash that keeps nothing unsynced has the grant, with the tick of the last
    // commit before it.
    let held = held_after(&disk, Survival::Nothing, 2);
    assert_eq!(held[0], [(FREE, 3)]);
}

/// Scenario 5: the tick of a grant is that of the last commit the store has of the
/// region, and a change the region makes to the chunk in the tick after it is in the
/// chunk when the region is opened once more.
#[test]
fn a_grant_has_the_tick_of_the_last_commit_and_what_follows_is_replayed() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (first, _) = opened(&store, 0, 1);
    for tick in 1..=3 {
        log(&first, tick, &[]);
    }
    first.flush();
    drop(first);

    // Restored up to tick 3, whatever tick the region itself is in by now.
    let (second, restored) = store.open_region(hello_of(&gap(), 0, 2)).unwrap();
    assert_eq!(restored.tick(), 3);
    assert_eq!(claim(&second, &[FREE]), (vec![FREE], Vec::new()));
    let (x, y, z) = block_of(FREE);
    log(&second, 4, &[(x, y, z, blocks::GLASS)]);
    second.flush();
    drop(second);

    let (third, restored) = store.open_region(hello_of(&gap(), 0, 3)).unwrap();
    assert_eq!(restored.held, [(FREE, 3)]);
    assert_eq!(load(&third, FREE), built(FREE, blocks::GLASS));
    // And the same after the store has started again.
    third.flush();
    let left = Arc::new(disk.crashed(Survival::Nothing));
    let store = store_on(&left, &gap()).unwrap();
    let (fourth, held) = opened(&store, 0, 4);
    assert_eq!(held, [(FREE, 3)]);
    assert_eq!(load(&fourth, FREE), built(FREE, blocks::GLASS));
}

/// What a region committed for a chunk it does not hold is logged and never put into a
/// stored chunk.
#[test]
fn changes_to_chunks_a_region_does_not_hold_are_not_replayed() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let west = open(&store, hello(0, 1));
    let east = open(&store, hello(1, 1));
    // The western region changes a block of its own and one of the eastern stripe.
    log(
        &west,
        1,
        &[(-3, -61, 4, blocks::AIR), (3, 100, 4, blocks::GLASS)],
    );
    west.flush();
    drop(west);
    let (west, restored) = store.open_region(hello(0, 2)).unwrap();
    assert_eq!(restored.deltas.len(), 1);
    assert_eq!(load(&west, WEST), dug());
    assert_eq!(load(&east, ORIGIN), generator().generate(ORIGIN));
    // Nor when the world is made over, with whoever holds the chunk then.
    west.flush();
    east.flush();
    let left = Arc::new(disk.crashed(Survival::Nothing));
    let single = stripes(&[]);
    let store = store_on(&left, &single).unwrap();
    let whole = open(&store, hello_of(&single, 0, 3));
    assert_eq!(load(&whole, WEST), dug());
    assert_eq!(load(&whole, ORIGIN), generator().generate(ORIGIN));
}

/// Scenario 6, built over: a region changes a block of a chunk, saves the chunk and
/// returns it, without a checkpoint; another claims the chunk, sets the same block
/// otherwise, saves, checkpoints and returns; the first claims the chunk again and is
/// then opened anew. The block is as the second left it.
#[test]
fn what_a_later_holder_built_is_not_undone_by_an_earlier_holders_log() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (first, _) = opened(&store, 0, 1);
    let (second, _) = opened(&store, 1, 1);
    let (x, y, z) = block_of(FREE);

    assert_eq!(claim(&first, &[FREE]).0, [FREE]);
    log(&first, 1, &[(x, y, z, blocks::STONE)]);
    first.request(StoreRequest::Save {
        position: FREE,
        tick: 1,
        chunk: built(FREE, blocks::STONE),
    });
    give_back(&first, &[FREE]);
    first.flush();

    assert_eq!(claim(&second, &[FREE]).0, [FREE]);
    assert_eq!(load(&second, FREE), built(FREE, blocks::STONE));
    log(&second, 1, &[(x, y, z, blocks::GLASS)]);
    second.request(StoreRequest::Save {
        position: FREE,
        tick: 1,
        chunk: built(FREE, blocks::GLASS),
    });
    second.request(StoreRequest::Checkpoint {
        tick: 1,
        state: b"second".to_vec(),
    });
    give_back(&second, &[FREE]);
    second.flush();

    assert_eq!(claim(&first, &[FREE]).0, [FREE]);
    first.flush();
    drop(first);
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &gap()).unwrap();
        let (first, restored) = store.open_region(hello_of(&gap(), 0, epoch)).unwrap();
        // The commit is still what the region is restored with, and the grant is of
        // its tick, so that its change is not put into the chunk again.
        assert_eq!(restored.deltas.len(), 1, "{survival:?}");
        assert_eq!(restored.held, [(FREE, 1)], "{survival:?}");
        assert_eq!(
            load(&first, FREE),
            built(FREE, blocks::GLASS),
            "{survival:?}"
        );
    }
}

/// Scenario 7: a chunk that is returned and not saved again loses nothing that was
/// saved before; a claim that arrives before the return is through is `foreign`, one
/// after it granted; and a flush behind the return is answered only once a crash would
/// keep the return.
#[test]
fn a_chunk_is_its_regions_until_its_return_is_through() {
    let disk = Arc::new(MemoryDisk::default());
    let (store, barrier) = gated(&disk, &[BUSY]);
    let (first, _) = opened(&store, 0, 1);
    let (busy, _) = opened(&store, 1, 1);
    let (second, _) = opened(&store, 2, 1);
    assert_eq!(claim(&first, &[FREE]).0, [FREE]);
    save(&first, FREE, &built(FREE, blocks::STONE));
    first.flush();

    // The thread for chunks is busy; the return waits behind what it is busy with.
    save(&busy, BUSY, &edited());
    barrier.wait();
    give_back(&first, &[FREE]);
    first.request(StoreRequest::Flush);
    assert_eq!(
        claim(&second, &[FREE]),
        (Vec::new(), vec![(FREE, RegionId(0))])
    );
    // The claim was looked at after the flush was asked for, which is not answered.
    assert_eq!(first.try_reply(), None);
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let held = held_after(&disk, survival, epoch);
        assert_eq!(held[0], [(FREE, 0)], "{survival:?}");
    }

    barrier.wait();
    assert_eq!(reply(&first), StoreReply::Flushed);
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let held = held_after(&disk, survival, epoch);
        assert_eq!(held[0], [], "{survival:?}");
    }
    first.request(StoreRequest::Load { position: FREE });
    let not_held = StoreReply::NotHeld {
        position: FREE,
        holder: None,
    };
    assert_eq!(reply(&first), not_held);
    assert_eq!(claim(&second, &[FREE]), (vec![FREE], Vec::new()));
    assert_eq!(load(&second, FREE), built(FREE, blocks::STONE));
    assert_eq!(
        claim(&first, &[FREE]),
        (Vec::new(), vec![(FREE, RegionId(2))])
    );
}

/// Scenario 8: a region that claims a chunk again while its return of it is under way
/// keeps it, with the tick it had.
#[test]
fn a_chunk_claimed_again_while_it_is_being_returned_is_kept() {
    let disk = Arc::new(MemoryDisk::default());
    let (store, barrier) = gated(&disk, &[BUSY]);
    let (first, _) = opened(&store, 0, 1);
    let (busy, _) = opened(&store, 1, 1);
    let (second, _) = opened(&store, 2, 1);
    log(&first, 1, &[]);
    assert_eq!(claim(&first, &[FREE]).0, [FREE]);
    log(&first, 2, &[]);

    save(&busy, BUSY, &edited());
    barrier.wait();
    give_back(&first, &[FREE]);
    assert_eq!(claim(&first, &[FREE]), (vec![FREE], Vec::new()));
    barrier.wait();
    first.flush();
    assert_eq!(
        claim(&second, &[FREE]),
        (Vec::new(), vec![(FREE, RegionId(0))])
    );
    assert_eq!(load(&first, FREE), generator().generate(FREE));
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let held = held_after(&disk, survival, epoch);
        assert_eq!(held[0], [(FREE, 1)], "{survival:?}");
    }
}

/// Scenario 9: a return that was called off frees nothing later. With the thread for
/// chunks held, a region returns a chunk, claims it again, commits a change to it,
/// saves it and returns it again. When the thread has got as far as the first return,
/// the chunk is the region's, and the change is in what it loads after a crash there.
/// When it has got to the end, the chunk is free and the stored chunk has the change.
#[test]
fn a_return_that_was_called_off_frees_nothing_later() {
    let disk = Arc::new(MemoryDisk::default());
    let (store, barrier) = gated(&disk, &[BUSY, FREE]);
    let (first, _) = opened(&store, 0, 1);
    let (busy, _) = opened(&store, 1, 1);
    let (second, _) = opened(&store, 2, 1);
    let (x, y, z) = block_of(FREE);
    log(&first, 1, &[]);
    assert_eq!(claim(&first, &[FREE]).0, [FREE]);

    save(&busy, BUSY, &edited());
    barrier.wait();
    give_back(&first, &[FREE]);
    assert_eq!(claim(&first, &[FREE]), (vec![FREE], Vec::new()));
    log(&first, 2, &[(x, y, z, blocks::GLASS)]);
    first.request(StoreRequest::Save {
        position: FREE,
        tick: 2,
        chunk: built(FREE, blocks::GLASS),
    });
    give_back(&first, &[FREE]);
    first.request(StoreRequest::Flush);

    // On to the save of the chunk, which is behind the first return and waits.
    barrier.wait();
    barrier.wait();
    // Once the list is here, the commit thread has heard of the first return.
    let list = store.regions().unwrap();
    assert!(list.regions[0].bounds.is_some());
    assert_eq!(
        claim(&second, &[FREE]),
        (Vec::new(), vec![(FREE, RegionId(0))])
    );
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &gap()).unwrap();
        let (again, held) = opened(&store, 0, epoch);
        assert_eq!(held, [(FREE, 1)], "{survival:?}");
        assert_eq!(
            load(&again, FREE),
            built(FREE, blocks::GLASS),
            "{survival:?}"
        );
    }

    // On to the end.
    barrier.wait();
    assert_eq!(reply(&first), StoreReply::Flushed);
    assert_eq!(store.regions().unwrap().regions[0].bounds, None);
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &gap()).unwrap();
        let (_, held) = opened(&store, 0, epoch);
        assert_eq!(held, [], "{survival:?}");
        let (home, _) = opened(&store, 2, epoch);
        assert_eq!(claim(&home, &[FREE]).0, [FREE], "{survival:?}");
        assert_eq!(
            load(&home, FREE),
            built(FREE, blocks::GLASS),
            "{survival:?}"
        );
    }
}

/// The home chunk is not returned, and neither is a chunk more than once by one
/// return, nor one the region holds by being pinned.
#[test]
fn the_home_chunk_and_chunks_that_were_not_granted_are_not_returned() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (home, held) = opened(&store, 2, 1);
    assert_eq!(held, [(ORIGIN, 0)]);
    assert_eq!(claim(&home, &[FREE]).0, [FREE]);
    give_back(&home, &[ORIGIN, FREE, FREE, WEST, ChunkPos::new(9, 9)]);
    home.flush();
    assert_eq!(load(&home, ORIGIN), generator().generate(ORIGIN));
    let held = held_after(&disk, Survival::Nothing, 2);
    assert_eq!(held, [vec![], vec![], vec![(ORIGIN, 0)]]);
    // A pinned region holds a chunk of its area whatever it returns.
    let (west, _) = opened(&store, 0, 3);
    give_back(&west, &[WEST]);
    west.flush();
    assert_eq!(load(&west, WEST), generator().generate(WEST));
}

/// Appends `records` to the world on `disk` as a segment of the log with `number`.
fn segment(disk: &MemoryDisk, number: u64, records: &[LogRecord]) {
    let log: Vec<u8> = records.iter().flat_map(LogRecord::encode).collect();
    put(disk, &format!("/world/log/{number:020}.wal"), &log);
}

/// Scenario 10: a `Returned` in the log for a chunk the region has no grant of does
/// not keep the store from starting, and changes nothing. A `Granted` that does not
/// fit the table does, and so does any record of regions in a world without a table.
#[test]
fn records_of_the_log_that_do_not_fit_the_table() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (first, _) = opened(&store, 0, 1);
    assert_eq!(claim(&first, &[FREE]).0, [FREE]);
    first.flush();
    let world = disk.crashed(Survival::Nothing);
    let expected = held_after(&world, Survival::Nothing, 2);

    let returned = |region, chunk| LogRecord::Returned {
        region,
        chunks: vec![chunk],
    };
    let left = world.crashed(Survival::Nothing);
    // Of another region than the one that holds it, of a chunk nobody was granted, of
    // one held by being pinned, and of a region there is none of.
    let stray = [
        returned(1, FREE),
        returned(0, ChunkPos::new(6, 6)),
        returned(0, WEST),
        returned(9, FREE),
    ];
    segment(&left, 7, &stray);
    assert_eq!(held_after(&left, Survival::Nothing, 2), expected);

    let granted = |region, chunk| LogRecord::Granted {
        region,
        tick: 1,
        chunks: vec![chunk],
    };
    for misfit in [
        granted(1, FREE),
        granted(0, FREE),
        granted(9, ChunkPos::new(6, 6)),
    ] {
        let left = Arc::new(world.crashed(Survival::Nothing));
        segment(&left, 7, std::slice::from_ref(&misfit));
        let started = store_on(&left, &gap());
        assert!(matches!(started, Err(StoreError::Table(_))), "{misfit:?}");
    }
    // In a segment the table file stands for, a record is not read for the table:
    // neither this grant nor the one the store wrote, which the file made here does
    // not have.
    let left = Arc::new(world.crashed(Survival::Nothing));
    let mut file = table_file(&left);
    file.from = 8;
    put(&left, "/world/regions/table", &file.encode());
    segment(&left, 7, &[granted(1, FREE)]);
    assert_eq!(
        held_after(&left, Survival::Nothing, 2),
        [vec![], vec![], vec![(ORIGIN, 0)]]
    );

    // A world from before there was a table has no such records.
    let old = world_of_today();
    segment(&old, 2, &[granted(0, FREE)]);
    assert!(matches!(
        store_on(&old, &division()),
        Err(StoreError::Table(_))
    ));
}

/// The segments of the log in the world on `disk`.
fn segments(disk: &MemoryDisk) -> Vec<u64> {
    let names = disk.list(Path::new("/world/log")).unwrap();
    names
        .iter()
        .map(|name| name.strip_suffix(".wal").unwrap().parse().unwrap())
        .collect()
}

/// Scenario 11: after checkpoints of every region, the log has no segment below the
/// table file's `from`, and a store started on that world has the same list and the
/// same grants.
#[test]
fn the_table_file_is_written_when_that_frees_the_log() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (first, _) = opened(&store, 0, 1);
    let (second, _) = opened(&store, 1, 1);
    let other = ChunkPos::new(6, 5);
    log(&first, 1, &[]);
    assert_eq!(claim(&first, &[FREE, other]).0, [FREE, other]);
    log(&second, 1, &[]);
    give_back(&first, &[other]);
    first.flush();
    assert_eq!(claim(&second, &[other]).0, [other]);
    second.flush();
    assert_eq!(segments(&disk), [1]);
    assert_eq!(table_file(&disk).from, 1);

    // The first region's checkpoint does not free the segment: the second has a commit
    // in it. The table stays as it is.
    first.request(StoreRequest::Checkpoint {
        tick: 1,
        state: b"first".to_vec(),
    });
    first.flush();
    assert_eq!(segments(&disk), [1]);
    assert_eq!(table_file(&disk).from, 1);
    log(&first, 2, &[]);
    first.flush();
    assert_eq!(segments(&disk), [1, 2]);

    // The second's does: no lane needs the first segment any more, and the table file
    // takes its place.
    second.request(StoreRequest::Checkpoint {
        tick: 1,
        state: b"second".to_vec(),
    });
    second.flush();
    let file = table_file(&disk);
    assert_eq!(file.from, 3);
    assert_eq!(file.regions[0].grants, [(FREE, 1)]);
    assert_eq!(file.regions[1].grants, [(other, 1)]);
    // The segment with the first region's later commit stays for that commit.
    assert_eq!(segments(&disk), [2]);
    first.request(StoreRequest::Checkpoint {
        tick: 2,
        state: b"first".to_vec(),
    });
    first.flush();
    assert_eq!(segments(&disk), Vec::<u64>::new());

    let list = store.regions().unwrap();
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        // A segment that was removed can be there again after a crash; what it says
        // of the table is in the file, and is not read.
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &gap()).unwrap();
        assert_eq!(listed(&store.regions().unwrap()), listed(&list));
        let held = held_after(&disk, survival, epoch);
        assert_eq!(
            held,
            [vec![(FREE, 1)], vec![(other, 1)], vec![(ORIGIN, 0)]],
            "{survival:?}"
        );
    }

    // What changes the table afterwards is in a segment the file names, and counts.
    assert_eq!(claim(&second, &[ChunkPos::new(7, 5)]).0.len(), 1);
    second.flush();
    assert_eq!(segments(&disk), [3]);
    let held = held_after(&disk, Survival::Nothing, 9);
    assert_eq!(held[1].len(), 2);
}

/// F4 of ADR-0011: a claim in a group that failed, then the same chunk claimed by
/// another region and answered, then a crash that loses truncations it was not made to
/// write out: the store starts, and the chunk is the second region's.
#[test]
fn a_grant_of_a_failed_group_does_not_come_back() {
    let (store, disk) = crate::tests::switched_for(&gap());
    let (first, _) = opened(&store, 0, 1);
    let (second, _) = opened(&store, 1, 1);
    log(&first, 1, &[]);
    crate::tests::committed(&first, 1);

    disk.failing_syncs.store(true, Ordering::SeqCst);
    first.request(StoreRequest::Claim { chunks: vec![FREE] });
    // The claim is never answered: the answers end, without one, when the handle is
    // lost.
    assert_eq!(first.replies.iter().count(), 0);
    // The handles are lost one after the other; this returns once the second is.
    second.flush();
    assert!(first.is_lost() && second.is_lost());
    // As long as the log is not cut back for good, a crash can still bring the grant
    // back, and nobody is served.
    assert_eq!(
        held_after(&disk.disk, Survival::Untruncated, 2)[0],
        [(FREE, 1)]
    );
    assert!(matches!(store.regions(), Err(StoreError::Io(_))));
    disk.failing_syncs.store(false, Ordering::SeqCst);

    // The grant is taken back in memory as well: the chunk is free for the second.
    let (second, _) = opened(&store, 1, 2);
    assert_eq!(claim(&second, &[FREE]), (vec![FREE], Vec::new()));
    for (epoch, survival) in (3..).zip(SURVIVALS) {
        let held = held_after(&disk.disk, survival, epoch);
        assert_eq!(held[0], [], "{survival:?}");
        assert_eq!(held[1], [(FREE, 0)], "{survival:?}");
    }
}

/// A return whose record is in a group that failed is undone as well: the chunk is the
/// region's again, with the tick it had, as it is for a store that starts.
#[test]
fn a_return_of_a_failed_group_is_given_back() {
    let (store, disk) = crate::tests::switched_for(&gap());
    let (first, _) = opened(&store, 0, 1);
    log(&first, 1, &[]);
    assert_eq!(claim(&first, &[FREE]).0, [FREE]);

    disk.failing_syncs.store(true, Ordering::SeqCst);
    give_back(&first, &[FREE]);
    first.flush();
    assert!(first.is_lost());
    disk.failing_syncs.store(false, Ordering::SeqCst);

    let (second, _) = opened(&store, 1, 1);
    assert_eq!(
        claim(&second, &[FREE]),
        (Vec::new(), vec![(FREE, RegionId(0))])
    );
    let (first, held) = opened(&store, 0, 2);
    assert_eq!(held, [(FREE, 1)]);
    assert_eq!(load(&first, FREE), generator().generate(FREE));
    for (epoch, survival) in (3..).zip(SURVIVALS) {
        let held = held_after(&disk.disk, survival, epoch);
        assert_eq!(held[0], [(FREE, 1)], "{survival:?}");
    }
}

/// A claim or a return of more chunks than a record takes is several records, and all
/// of them or none count.
#[test]
fn a_claim_of_very_many_chunks_takes_several_records() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (first, _) = opened(&store, 0, 1);
    // A hundred thousand chunks of the gap, which is sixteen chunks wide.
    let many: Vec<ChunkPos> = (0..100_000)
        .map(|index| ChunkPos::new(index % 16, 100 + index / 16))
        .collect();
    assert_eq!(claim(&first, &many), (many.clone(), Vec::new()));
    first.flush();
    let log = disk.read(Path::new("/world/log/00000000000000000001.wal"));
    let (records, _) = clustine_format::read_log(&log.unwrap().unwrap()).unwrap();
    let granted: Vec<usize> = records
        .iter()
        .filter_map(|record| match record {
            LogRecord::Granted { chunks, .. } => Some(chunks.len()),
            _ => None,
        })
        .collect();
    assert_eq!(granted, [65_536, 100_000 - 65_536]);
    assert_eq!(held_after(&disk, Survival::Nothing, 2)[0].len(), 100_000);

    give_back(&first, &many);
    first.flush();
    assert_eq!(held_after(&disk, Survival::Nothing, 3)[0], []);
}

/// What a region was granted crosses a connection with the rest of what it is restored
/// with.
#[test]
fn what_a_region_was_granted_is_restored_over_a_connection() {
    let store = Store::memory_divided(generator(), gap()).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let server = serve(store.clone(), listener).unwrap();
    let address = server.local_addr().to_string();
    let (remote, restored) = StoreHandle::connect(&address, hello_of(&gap(), 0, 1)).unwrap();
    assert_eq!(
        (restored.held, restored.pinned),
        (Vec::new(), gap().pinned[..1].to_vec())
    );
    // Requests and answers of regions that hold chunks cross it as well.
    let chunks: Vec<ChunkPos> = (0..3000).map(|z| ChunkPos::new(3, z)).collect();
    log(&remote, 1, &[]);
    assert_eq!(claim(&remote, &chunks), (chunks.clone(), Vec::new()));
    give_back(&remote, &chunks[..1]);
    remote.flush();
    drop(remote);

    let expected: Vec<(ChunkPos, u64)> = chunks[1..].iter().map(|chunk| (*chunk, 1)).collect();
    let (remote, restored) = StoreHandle::connect(&address, hello_of(&gap(), 0, 2)).unwrap();
    assert_eq!(restored.held, expected);
    assert_eq!(restored.tick(), 1);
    drop(remote);
    let (_, locally) = store.open_region(hello_of(&gap(), 0, 3)).unwrap();
    assert_eq!(locally.held, expected);
    // The home region's one chunk, without a state or a delta before it.
    let (_, home) = StoreHandle::connect(&address, hello_of(&gap(), 2, 1)).unwrap();
    assert_eq!(home.held, [(ORIGIN, 0)]);
}

/// The whole state of a region as a test makes it up.
pub(crate) fn whole(name: &str, tick: u64) -> Vec<u8> {
    format!("{name} {tick}").into_bytes()
}

/// Has the region of `handle` absorb `absorbed`, and waits for the answer.
pub(crate) fn absorb(
    handle: &StoreHandle,
    absorbed: u32,
    absorbed_epoch: u64,
    tick: u64,
) -> StoreReply {
    handle.request(StoreRequest::AbsorbCommit {
        absorbed: RegionId(absorbed),
        absorbed_epoch,
        tick,
        state: whole("merged", tick),
    });
    reply(handle)
}

/// Splits `chunks` off the region of `handle` as the region `part`, and waits for the
/// answer. The store makes the region only under the next id, which a test knows from
/// the regions its world began with and the splits it has made.
pub(crate) fn split(
    handle: &StoreHandle,
    tick: u64,
    chunks: &[ChunkPos],
    as_epoch: u64,
    part: u32,
) -> StoreReply {
    handle.request(StoreRequest::SplitCommit {
        tick,
        state: whole("rest", tick),
        part: SplitPart {
            chunks: chunks.to_vec(),
            state: whole("part", tick),
        },
        as_epoch,
        region: RegionId(part),
    });
    reply(handle)
}

fn declined(reason: Decline) -> StoreReply {
    StoreReply::Declined { reason }
}

/// Commits `tick` and makes a checkpoint of it, so that no commit of the region is
/// behind its checkpoint.
pub(crate) fn checkpoint(handle: &StoreHandle, tick: u64) {
    log(handle, tick, &[]);
    handle.request(StoreRequest::Checkpoint {
        tick,
        state: whole("state", tick),
    });
    handle.flush();
}

/// What a region of the world with a gap is restored with by a store that starts on
/// what a crash leaves of `disk`.
fn restored_after(disk: &MemoryDisk, survival: Survival, region: u32, epoch: u64) -> Restored {
    let left = Arc::new(disk.crashed(survival));
    let store = store_on(&left, &gap()).unwrap();
    store
        .open_region(hello_of(&gap(), region, epoch))
        .unwrap()
        .1
}

pub(crate) fn state_of(restored: &Restored) -> Option<(u64, Vec<u8>)> {
    let state = restored.state.as_ref();
    state.map(|state| (state.tick, state.state.clone()))
}

/// Scenario 12: a merge is declined, each time with its reason and with nothing
/// changed.
#[test]
fn a_merge_that_may_not_be_is_declined_with_its_reason() {
    let disk = Arc::new(MemoryDisk::default());
    let (store, barrier) = gated(&disk, &[ORIGIN]);
    let (survivor, _) = opened(&store, 0, 1);
    let (home, _) = opened(&store, 2, 1);
    let before = store.regions().unwrap();

    // Itself, a region there is none of, and the home region.
    assert_eq!(absorb(&survivor, 0, 1, 5), declined(Decline::NoSuchRegion));
    assert_eq!(absorb(&survivor, 9, 1, 5), declined(Decline::NoSuchRegion));
    assert_eq!(absorb(&survivor, 2, 1, 5), declined(Decline::Home));
    // A region that nobody has open, or that is open with another epoch than named.
    let nobody = Decline::NotOpened { epoch: None };
    assert_eq!(absorb(&survivor, 1, 5, 5), declined(nobody));
    let (other, _) = opened(&store, 1, 5);
    let another = Decline::NotOpened { epoch: Some(5) };
    assert_eq!(absorb(&survivor, 1, 4, 5), declined(another));
    assert_eq!(absorb(&survivor, 1, 6, 5), declined(another));

    // A commit behind the checkpoint, of the survivor or of the region to absorb.
    log(&survivor, 1, &[]);
    let uncheckpointed = |region| Decline::Uncheckpointed {
        region: RegionId(region),
    };
    assert_eq!(absorb(&survivor, 1, 5, 5), declined(uncheckpointed(0)));
    checkpoint(&survivor, 2);
    log(&other, 1, &[]);
    assert_eq!(absorb(&survivor, 1, 5, 5), declined(uncheckpointed(1)));
    checkpoint(&other, 2);

    // A tick that is not above one the survivor's session named in a commit.
    for tick in [0, 1, 2] {
        let named = Decline::Tick { named: 2 };
        assert_eq!(absorb(&survivor, 1, 5, tick), declined(named));
    }
    // Nor above one it named in a checkpoint that is still with the thread for chunks.
    save(&home, ORIGIN, &edited());
    barrier.wait();
    survivor.request(StoreRequest::Checkpoint {
        tick: 7,
        state: whole("state", 7),
    });
    for tick in [3, 7] {
        let named = Decline::Tick { named: 7 };
        assert_eq!(absorb(&survivor, 1, 5, tick), declined(named));
    }
    barrier.wait();
    survivor.flush();

    // A state that no record of the log holds.
    survivor.request(StoreRequest::AbsorbCommit {
        absorbed: RegionId(1),
        absorbed_epoch: 5,
        tick: 8,
        state: vec![0; clustine_format::MAX_RECORD_LENGTH],
    });
    assert_eq!(reply(&survivor), declined(Decline::TooLarge));

    // Nothing has changed, and the handles are as they were.
    let mut after = store.regions().unwrap();
    after.regions[1].epoch = 0;
    assert_eq!(after, before);
    assert!(!survivor.is_lost() && !other.is_lost());
    // With every reason gone, it is done.
    let done = StoreReply::Absorbed {
        absorbed: RegionId(1),
        chunks: Vec::new(),
        pinned: gap().pinned[1..].to_vec(),
    };
    assert_eq!(absorb(&survivor, 1, 5, 8), done);
}

/// Scenario 13: after a merge the survivor is restored with the state and the tick of
/// the request and no deltas, holds what the other was granted from the merge's tick,
/// and is pinned to both areas; the absorbed region's handle is lost and its hello is
/// refused; the list has it among those absorbed.
#[test]
fn a_merge_gives_the_survivor_all_the_absorbed_region_had() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (survivor, _) = opened(&store, 0, 1);
    let (other, _) = opened(&store, 1, 3);
    let east = ChunkPos::new(20, 0);
    assert_eq!(claim(&other, &[FREE]).0, [FREE]);
    save(&other, FREE, &built(FREE, blocks::STONE));
    save(&other, east, &built(east, blocks::GLASS));
    checkpoint(&other, 4);
    checkpoint(&survivor, 6);

    let done = StoreReply::Absorbed {
        absorbed: RegionId(1),
        chunks: vec![FREE],
        pinned: gap().pinned[1..].to_vec(),
    };
    assert_eq!(absorb(&survivor, 1, 3, 10), done);
    // The other region is none any more: its handle is lost, and nobody opens it.
    other.flush();
    assert!(other.is_lost() && !survivor.is_lost());
    for epoch in [3, 4, 100] {
        let refused = store.open_region(hello_of(&gap(), 1, epoch));
        assert!(
            matches!(
                refused,
                Err(StoreError::Absorbed {
                    region: RegionId(1),
                    into: RegionId(0)
                })
            ),
            "{:?}",
            refused.err()
        );
    }
    let list = store.regions().unwrap();
    assert_eq!(list.absorbed, [(RegionId(1), RegionId(0))]);
    let regions: Vec<RegionId> = list.regions.iter().map(|info| info.region).collect();
    assert_eq!(regions, [RegionId(0), RegionId(2)]);
    assert_eq!(list.regions[0].pinned, gap().pinned);
    // The survivor loads and saves chunks of both areas, and what the other built.
    assert_eq!(load(&survivor, FREE), built(FREE, blocks::STONE));
    assert_eq!(load(&survivor, east), built(east, blocks::GLASS));
    save(&survivor, east, &built(east, blocks::STONE));
    assert_eq!(load(&survivor, east), built(east, blocks::STONE));
    assert_eq!(load(&survivor, WEST), generator().generate(WEST));
    survivor.flush();

    // Over a connection the refusal says where the region went.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let server = serve(store.clone(), listener).unwrap();
    let refused = StoreHandle::connect(&server.local_addr().to_string(), hello_of(&gap(), 1, 9));
    assert!(matches!(
        refused,
        Err(StoreError::Absorbed {
            region: RegionId(1),
            into: RegionId(0)
        })
    ));

    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &gap()).unwrap();
        assert_eq!(
            listed(&store.regions().unwrap()),
            listed(&list),
            "{survival:?}"
        );
        let (handle, restored) = store.open_region(hello_of(&gap(), 0, epoch)).unwrap();
        assert_eq!(state_of(&restored), Some((10, whole("merged", 10))));
        assert_eq!(restored.deltas, [], "{survival:?}");
        assert_eq!(restored.held, [(FREE, 10)], "{survival:?}");
        assert_eq!(restored.pinned, gap().pinned, "{survival:?}");
        // What the absorbed region saved before its last checkpoint is there.
        assert_eq!(
            load(&handle, FREE),
            built(FREE, blocks::STONE),
            "{survival:?}"
        );
        assert!(matches!(
            store.open_region(hello_of(&gap(), 1, 100)),
            Err(StoreError::Absorbed { .. })
        ));
        // The absorbed region's state is gone; its region file stays for the block of
        // entity ids it names, which no other region is issued.
        assert_eq!(
            left.read(Path::new("/world/regions/1.state")).unwrap(),
            None
        );
        assert!(left.exists(Path::new("/world/regions/1.region")).unwrap());
    }
}

/// Scenarios 14 and 15: commits of the survivor after a merge are restored as deltas on
/// the merged state, and a checkpoint with the merge's own tick is not put in place; a
/// checkpoint after them is, and leaves no record of the merge needed.
#[test]
fn a_region_goes_on_from_the_state_of_its_merge() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (survivor, _) = opened(&store, 0, 1);
    let (other, _) = opened(&store, 1, 1);
    assert_eq!(claim(&other, &[FREE]).0, [FREE]);
    other.flush();
    assert!(matches!(
        absorb(&survivor, 1, 1, 10),
        StoreReply::Absorbed { .. }
    ));

    // With the merge's own tick, and with one below it: neither takes the place of
    // the merged state.
    for tick in [10, 9] {
        survivor.request(StoreRequest::Checkpoint {
            tick,
            state: whole("not this", tick),
        });
    }
    survivor.flush();
    // Nor does the checkpoint of another region let the record of the merge go, which
    // is all the state the survivor has.
    let (home, _) = opened(&store, 2, 1);
    checkpoint(&home, 1);
    assert_eq!(segments(&disk), [1]);
    let restored = restored_after(&disk, Survival::Nothing, 0, 2);
    assert_eq!(state_of(&restored), Some((10, whole("merged", 10))));
    assert_eq!(
        disk.read(Path::new("/world/regions/0.state")).unwrap(),
        None
    );

    let (x, y, z) = block_of(FREE);
    log(&survivor, 11, &[(x, y, z, blocks::GLASS)]);
    log(&survivor, 12, &[]);
    survivor.flush();
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &gap()).unwrap();
        let (handle, restored) = store.open_region(hello_of(&gap(), 0, epoch)).unwrap();
        assert_eq!(state_of(&restored), Some((10, whole("merged", 10))));
        let ticks: Vec<u64> = restored.deltas.iter().map(|delta| delta.tick).collect();
        assert_eq!(ticks, [11, 12], "{survival:?}");
        // What it changed in a chunk that came with the merge is replayed: the chunk
        // is held from the merge's tick.
        assert_eq!(
            load(&handle, FREE),
            built(FREE, blocks::GLASS),
            "{survival:?}"
        );
    }

    // A checkpoint above the merge is the state from then on, and the segment of the
    // merge goes with the table file written.
    save(&survivor, FREE, &built(FREE, blocks::GLASS));
    survivor.request(StoreRequest::Checkpoint {
        tick: 12,
        state: whole("state", 12),
    });
    survivor.flush();
    assert_eq!(segments(&disk), Vec::<u64>::new());
    let file = table_file(&disk);
    assert_eq!(file.absorbed, [(1, 0)]);
    assert_eq!(file.regions[0].grants, [(FREE, 10)]);
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let restored = restored_after(&disk, survival, 0, epoch);
        assert_eq!(state_of(&restored), Some((12, whole("state", 12))));
        assert_eq!(restored.deltas, [], "{survival:?}");
        assert_eq!(restored.held, [(FREE, 10)], "{survival:?}");
        assert_eq!(restored.pinned, gap().pinned, "{survival:?}");
    }
}

/// Scenario 16: a split is declined, each time with its reason and nothing changed.
#[test]
fn a_split_that_may_not_be_is_declined_with_its_reason() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (region, _) = opened(&store, 0, 1);
    let (home, _) = opened(&store, 2, 1);
    assert_eq!(claim(&region, &[FREE]).0, [FREE]);
    let before = store.regions().unwrap();
    let part = [WEST, FREE];

    // A commit behind the checkpoint.
    log(&region, 1, &[]);
    let uncheckpointed = Decline::Uncheckpointed {
        region: RegionId(0),
    };
    assert_eq!(split(&region, 5, &part, 1, 3), declined(uncheckpointed));
    checkpoint(&region, 2);
    // A tick that is not above one the session named.
    for tick in [1, 2] {
        let named = Decline::Tick { named: 2 };
        assert_eq!(split(&region, tick, &part, 1, 3), declined(named));
    }
    // No chunks, and an epoch nobody can say hello with.
    assert_eq!(split(&region, 5, &[], 1, 3), declined(Decline::Malformed));
    assert_eq!(split(&region, 5, &part, 0, 3), declined(Decline::Malformed));
    // A chunk the region does not hold: another region's, and nobody's.
    for chunk in [ORIGIN, ChunkPos::new(20, 0), ChunkPos::new(6, 6)] {
        let not_held = Decline::NotHeld { chunk };
        assert_eq!(split(&region, 5, &[WEST, chunk], 1, 3), declined(not_held));
    }
    // The home chunk never leaves the home region.
    assert_eq!(claim(&home, &[ChunkPos::new(1, 0)]).0.len(), 1);
    let with_home = [ChunkPos::new(1, 0), ORIGIN];
    assert_eq!(split(&home, 5, &with_home, 1, 3), declined(Decline::Home));
    // A state that no record of the log holds.
    region.request(StoreRequest::SplitCommit {
        tick: 5,
        state: Vec::new(),
        part: SplitPart {
            chunks: part.to_vec(),
            state: vec![0; clustine_format::MAX_RECORD_LENGTH],
        },
        as_epoch: 1,
        region: RegionId(3),
    });
    assert_eq!(reply(&region), declined(Decline::TooLarge));

    let mut after = store.regions().unwrap();
    after.regions[2].bounds = before.regions[2].bounds;
    assert_eq!(after, before);
    assert!(!region.is_lost() && !home.is_lost());
    assert_eq!(table_file(&disk).next_region, 3);
    let done = StoreReply::Split {
        region: RegionId(3),
    };
    assert_eq!(split(&region, 5, &part, 1, 3), done);
}

/// Scenario 17: after a split the new region has an id above every id there was, is in
/// the list with its epoch, not pinned, and holds the part's chunks from the split's
/// tick; a hello for it with that epoch is restored with the part's state, and one
/// with a lower epoch is refused; the old region has its state of the request, and
/// does not hold the part's chunks any more.
#[test]
fn a_split_makes_a_region_of_the_part() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (old, _) = opened(&store, 0, 1);
    assert_eq!(claim(&old, &[FREE]).0, [FREE]);
    let inside = ChunkPos::new(-5, 3);
    save(&old, inside, &built(inside, blocks::STONE));
    save(&old, FREE, &built(FREE, blocks::GLASS));
    checkpoint(&old, 2);

    // A chunk the region was granted and one it holds by being pinned, each once.
    let done = StoreReply::Split {
        region: RegionId(3),
    };
    assert_eq!(split(&old, 7, &[FREE, inside, FREE], 4, 3), done);
    let list = store.regions().unwrap();
    let made = RegionInfo {
        region: RegionId(3),
        epoch: 4,
        bounds: Some(ChunkBox {
            min: ChunkPos::new(-5, 3),
            max: ChunkPos::new(5, 5),
        }),
        pinned: Vec::new(),
    };
    assert_eq!(list.regions[3], made);
    assert_eq!(list.regions[0].bounds, None);
    for position in [inside, FREE] {
        old.request(StoreRequest::Load { position });
        let not_held = StoreReply::NotHeld {
            position,
            holder: Some(RegionId(3)),
        };
        assert_eq!(reply(&old), not_held);
    }
    assert_eq!(
        load(&old, ChunkPos::new(-5, 4)),
        generator().generate(ChunkPos::new(-5, 4))
    );
    old.flush();

    let check = |store: &Store, epoch: u64, case: &str| {
        assert_eq!(listed(&store.regions().unwrap()), listed(&list), "{case}");
        let refused = store.open_region(hello_of(&gap(), 3, 3));
        assert!(
            matches!(
                refused,
                Err(StoreError::EpochRefused {
                    region: RegionId(3),
                    offered: 3,
                    seen: 4..
                })
            ),
            "{case}: {:?}",
            refused.err()
        );
        let (part, restored) = store.open_region(hello_of(&gap(), 3, epoch)).unwrap();
        assert_eq!(state_of(&restored), Some((7, whole("part", 7))), "{case}");
        assert_eq!(restored.deltas, [], "{case}");
        assert_eq!(restored.held, [(inside, 7), (FREE, 7)], "{case}");
        assert_eq!(restored.pinned, [], "{case}");
        let none = EntityIds {
            first: clustine_world::EntityId(0),
            end: clustine_world::EntityId(0),
        };
        assert_eq!(restored.entity_ids, none, "{case}");
        assert_eq!(load(&part, inside), built(inside, blocks::STONE), "{case}");
        assert_eq!(load(&part, FREE), built(FREE, blocks::GLASS), "{case}");
        part.flush();
    };
    // The worker that made it says hello with the epoch it named.
    check(&store, 4, "at once");
    let (_, restored) = store.open_region(hello_of(&gap(), 0, 2)).unwrap();
    assert_eq!(state_of(&restored), Some((7, whole("rest", 7))));
    assert_eq!((restored.deltas, restored.held), (Vec::new(), Vec::new()));
    for (epoch, survival) in (5..).zip(SURVIVALS) {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &gap()).unwrap();
        check(&store, epoch, &format!("{survival:?}"));
        let (_, restored) = store.open_region(hello_of(&gap(), 0, epoch)).unwrap();
        assert_eq!(state_of(&restored), Some((7, whole("rest", 7))));
        assert_eq!(restored.pinned, gap().pinned[..1], "{survival:?}");
    }
}

/// Scenario 15 for a split: a checkpoint with the split's own tick, of the old region
/// or of the part, is not put in place, and a later one is.
#[test]
fn both_regions_go_on_from_the_states_of_their_split() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (old, _) = opened(&store, 0, 1);
    assert!(matches!(
        split(&old, 7, &[WEST], 1, 3),
        StoreReply::Split { .. }
    ));
    let (part, _) = opened(&store, 3, 1);
    for handle in [&old, &part] {
        handle.request(StoreRequest::Checkpoint {
            tick: 7,
            state: whole("not this", 7),
        });
        log(handle, 8, &[]);
        handle.flush();
    }
    for (region, name) in [(0, "rest"), (3, "part")] {
        let restored = restored_after(&disk, Survival::Nothing, region, 2);
        assert_eq!(state_of(&restored), Some((7, whole(name, 7))));
        assert_eq!(restored.deltas.len(), 1);
    }
    // The record of the split is needed until both have a later state of their own.
    old.request(StoreRequest::Checkpoint {
        tick: 8,
        state: whole("state", 8),
    });
    old.flush();
    assert_eq!(segments(&disk).len(), 1);
    let restored = restored_after(&disk, Survival::Nothing, 3, 2);
    assert_eq!(state_of(&restored), Some((7, whole("part", 7))));
    part.request(StoreRequest::Checkpoint {
        tick: 8,
        state: whole("state", 8),
    });
    part.flush();
    assert_eq!(segments(&disk), Vec::<u64>::new());
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        for region in [0, 3] {
            let restored = restored_after(&disk, survival, region, epoch);
            assert_eq!(state_of(&restored), Some((8, whole("state", 8))));
            assert_eq!(restored.deltas, [], "{survival:?}");
        }
        let held = held_after(&disk, survival, epoch);
        assert_eq!(held[3], [(WEST, 7)], "{survival:?}");
    }
}

/// Scenario 18: the hello for the part of a split is answered while the thread for
/// chunks is busy, and so is any hello with nothing to put into the stored chunks; one
/// with a block change to replay is answered only when the thread is free.
#[test]
fn a_hello_with_nothing_to_replay_does_not_wait_for_the_thread_for_chunks() {
    let disk = Arc::new(MemoryDisk::default());
    let (store, barrier) = gated(&disk, &[BUSY]);
    let (old, _) = opened(&store, 0, 1);
    let (busy, _) = opened(&store, 1, 1);
    save(&busy, BUSY, &edited());
    barrier.wait();

    // None of these would return if it waited.
    assert!(matches!(
        split(&old, 7, &[WEST], 1, 3),
        StoreReply::Split { .. }
    ));
    let (part, held) = opened(&store, 3, 1);
    assert_eq!(held, [(WEST, 7)]);
    let (home, _) = opened(&store, 2, 1);
    // Commits without a block change, and one with a change to a chunk the region
    // does not hold any more, are nothing to replay either.
    log(&old, 8, &[]);
    log(&old, 9, &[(-3, -61, 4, blocks::AIR)]);
    crate::tests::committed(&old, 9);
    let (old, _) = opened(&store, 0, 2);

    // A change to a chunk the region holds is put into the chunk first.
    let (x, y, z) = block_of(ChunkPos::new(-9, 0));
    log(&old, 10, &[(x, y, z, blocks::GLASS)]);
    crate::tests::committed(&old, 10);
    let answered = crate::tests::open_later(&store, hello_of(&gap(), 0, 3));
    // The list is asked for behind the hello, which is therefore dealt with by now.
    store.regions().unwrap();
    assert!(answered.try_recv().is_err());
    barrier.wait();
    let (_, restored) = answered.recv().unwrap().unwrap();
    assert_eq!(restored.deltas.len(), 3);
    part.flush();
    home.flush();
}

/// Scenario 19: a split of a chunk that is being returned. The split is answered; when
/// the return comes through, the chunk is the part's, and the store starts again on
/// what is left.
#[test]
fn a_chunk_that_is_being_returned_goes_with_the_part_it_is_split_off_in() {
    let disk = Arc::new(MemoryDisk::default());
    let (store, barrier) = gated(&disk, &[BUSY]);
    let (old, _) = opened(&store, 0, 1);
    let (busy, _) = opened(&store, 1, 1);
    let (home, _) = opened(&store, 2, 1);
    let other = ChunkPos::new(6, 5);
    assert_eq!(claim(&old, &[FREE, other]).0, [FREE, other]);
    old.flush();

    save(&busy, BUSY, &edited());
    barrier.wait();
    give_back(&old, &[FREE, other]);
    let done = StoreReply::Split {
        region: RegionId(3),
    };
    assert_eq!(split(&old, 5, &[FREE], 1, 3), done);
    barrier.wait();
    old.flush();

    // The return has freed the chunk that stayed, and not the one that went, of which
    // the log says nothing: it left the return when it was split off.
    let log = disk.read(Path::new("/world/log/00000000000000000001.wal"));
    let (records, _) = clustine_format::read_log(&log.unwrap().unwrap()).unwrap();
    let returned: Vec<&LogRecord> = records
        .iter()
        .filter(|record| matches!(record, LogRecord::Returned { .. }))
        .collect();
    let expected = LogRecord::Returned {
        region: 0,
        chunks: vec![other],
    };
    assert_eq!(returned, [&expected]);
    assert_eq!(
        claim(&home, &[FREE, other]),
        (vec![other], vec![(FREE, RegionId(3))])
    );
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let held = held_after(&disk, survival, epoch);
        assert_eq!(held[0], [], "{survival:?}");
        assert_eq!(held[3], [(FREE, 5)], "{survival:?}");
    }
}

/// Scenario 20: a part split off a pinned region and returned by the part is the pinned
/// region's again; one absorbed by the pinned region is among what it was granted.
/// Scenario 21: ids are not used again, also after a merge and a restart.
#[test]
fn what_was_split_off_a_pinned_region_comes_back_by_a_return_or_a_merge() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (pinned, _) = opened(&store, 0, 1);
    let (first, second) = (ChunkPos::new(-5, 0), ChunkPos::new(-5, 1));
    let done = StoreReply::Split {
        region: RegionId(3),
    };
    assert_eq!(split(&pinned, 5, &[first, second], 1, 3), done);
    let (part, held) = opened(&store, 3, 1);
    assert_eq!(held, [(first, 5), (second, 5)]);

    give_back(&part, &[first]);
    part.flush();
    assert_eq!(load(&pinned, first), generator().generate(first));
    pinned.request(StoreRequest::Load { position: second });
    assert!(matches!(reply(&pinned), StoreReply::NotHeld { .. }));

    let merged = StoreReply::Absorbed {
        absorbed: RegionId(3),
        chunks: vec![second],
        pinned: Vec::new(),
    };
    assert_eq!(absorb(&pinned, 3, 1, 6), merged);
    assert_eq!(load(&pinned, second), generator().generate(second));
    pinned.flush();
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &gap()).unwrap();
        let (pinned, held) = opened(&store, 0, epoch);
        // By being pinned again, without a grant; and by the merge, from its tick.
        assert_eq!(held, [(second, 6)], "{survival:?}");
        assert_eq!(load(&pinned, first), generator().generate(first));
        // The region that is gone has left no files, and its id is not used again.
        let files = left.list(Path::new("/world/regions")).unwrap();
        assert!(
            !files.iter().any(|name| name.starts_with("3.")),
            "{files:?}"
        );
        let again = StoreReply::Split {
            region: RegionId(4),
        };
        assert_eq!(split(&pinned, 9, &[first], 1, 4), again, "{survival:?}");
    }
}

/// Scenario 22: a checkpoint of the absorbed region that is still under way when the
/// merge is written is not put in place.
#[test]
fn a_checkpoint_of_a_region_that_is_absorbed_meanwhile_is_not_put_in_place() {
    let disk = Arc::new(MemoryDisk::default());
    let (store, barrier) = gated(&disk, &[ORIGIN]);
    let (survivor, _) = opened(&store, 0, 1);
    let (other, _) = opened(&store, 1, 1);
    let (home, _) = opened(&store, 2, 1);

    save(&home, ORIGIN, &edited());
    barrier.wait();
    other.request(StoreRequest::Checkpoint {
        tick: 4,
        state: whole("late", 4),
    });
    assert!(matches!(
        absorb(&survivor, 1, 1, 5),
        StoreReply::Absorbed { .. }
    ));
    barrier.wait();
    home.flush();
    survivor.flush();
    let files = disk.list(Path::new("/world/regions")).unwrap();
    assert!(
        !files.iter().any(|name| name.starts_with("1.state")),
        "{files:?}"
    );
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &gap()).unwrap();
        let files = left.list(Path::new("/world/regions")).unwrap();
        assert!(
            !files.iter().any(|name| name.starts_with("1.state")),
            "{files:?}"
        );
        let (_, restored) = store.open_region(hello_of(&gap(), 0, epoch)).unwrap();
        assert_eq!(state_of(&restored), Some((5, whole("merged", 5))));
    }
}

/// The list of regions is read by another process as the store's own process has it:
/// a connection that asks for it is sent the list and closed.
#[test]
fn the_list_of_regions_is_read_over_a_connection() {
    let directory = tempfile::tempdir().unwrap();
    let stores = [
        Store::memory_divided(generator(), gap()).unwrap(),
        Store::local_divided(directory.path(), generator(), gap()).unwrap(),
    ];
    for store in stores {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let server = serve(store.clone(), listener).unwrap();
        let address = server.local_addr().to_string();
        // A world that has just been made, and one in which regions were granted
        // chunks, split and merged.
        assert_eq!(regions(&address).unwrap(), store.regions().unwrap());
        let (first, _) = opened(&store, 0, 4);
        let (second, _) = opened(&store, 1, 2);
        assert_eq!(claim(&first, &[FREE]).0, [FREE]);
        let split_off = StoreReply::Split {
            region: RegionId(3),
        };
        assert_eq!(split(&first, 5, &[FREE, WEST], 7, 3), split_off);
        assert!(matches!(
            absorb(&first, 1, 2, 6),
            StoreReply::Absorbed { .. }
        ));
        second.flush();
        let list = regions(&address).unwrap();
        assert_eq!(list, store.regions().unwrap());
        assert_eq!(list.home, RegionId(2));
        assert_eq!(list.absorbed, [(RegionId(1), RegionId(0))]);
        let epochs: Vec<(RegionId, u64)> = list
            .regions
            .iter()
            .map(|info| (info.region, info.epoch))
            .collect();
        assert_eq!(
            epochs,
            [(RegionId(0), 4), (RegionId(2), 0), (RegionId(3), 7)]
        );

        // On a connection of the test's own: the list, and then nothing more.
        let mut connection = std::net::TcpStream::connect(&address).unwrap();
        let wire_write = clustine_rpc::wire::blocking::write::<clustine_rpc::StoreHello>;
        wire_write(&mut connection, &clustine_rpc::StoreHello::Regions).unwrap();
        let sent: Option<RegionList> = clustine_rpc::wire::blocking::read(&mut connection).unwrap();
        assert_eq!(sent, Some(list));
        let more: Option<RegionList> = clustine_rpc::wire::blocking::read(&mut connection).unwrap();
        assert_eq!(more, None);
        // A region is opened over a connection as before, beside it.
        let (remote, restored) = StoreHandle::connect(&address, hello_of(&gap(), 3, 7)).unwrap();
        assert_eq!(restored.held, [(WEST, 5), (FREE, 5)]);
        assert_eq!(load(&remote, FREE), generator().generate(FREE));
    }

    // Where no store listens, there is no list.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    drop(listener);
    assert!(matches!(regions(&address), Err(StoreError::Io(_))));
}

/// The row "Split" of section 4.3: a store that dies once the record of a split is
/// durable and before the new region's file is has split all the same, and the next
/// start writes the file with the epoch the record has, so that nobody with a lower
/// one opens the region.
#[test]
fn the_file_of_a_region_a_split_made_is_written_at_the_next_start() {
    let world = {
        let disk = Arc::new(MemoryDisk::default());
        let store = store_on(&disk, &gap()).unwrap();
        let (old, _) = opened(&store, 0, 1);
        old.flush();
        disk.crashed(Survival::Everything)
    };
    // A store on that world with the region opened, and nothing left to be synced.
    let prepared = |disk: &Arc<MemoryDisk>| {
        let store = store_on(disk, &gap()).unwrap();
        let (old, _) = opened(&store, 0, 2);
        old.flush();
        (store, old)
    };
    let before = {
        let disk = Arc::new(world.crashed(Survival::Everything));
        let _prepared = prepared(&disk);
        disk.operations()
    };
    let done = StoreReply::Split {
        region: RegionId(3),
    };
    // The split appends its record and syncs it, and then writes the region file,
    // syncs it, renames it and syncs the directory: the disk stops at each of the four.
    for step in 3..=6 {
        let fault = Fault::Stop(before + step);
        let disk = Arc::new(world.crashed(Survival::Everything).with(fault));
        let (store, old) = prepared(&disk);
        // Answered although the file could not be written: the record is durable.
        assert_eq!(split(&old, 5, &[WEST], 7, 3), done, "step {step}");
        drop((old, store));
        for survival in SURVIVALS {
            let case = format!("step {step}, {survival:?}");
            let left = Arc::new(disk.crashed(survival));
            let store = store_on(&left, &gap()).unwrap();
            let list = store.regions().unwrap();
            assert_eq!(list.regions[3].epoch, 7, "{case}");
            let refused = store.open_region(hello_of(&gap(), 3, 6));
            assert!(
                matches!(refused, Err(StoreError::EpochRefused { seen: 7, .. })),
                "{case}: {:?}",
                refused.err()
            );
            // The file is there for good by the time the store has started.
            let durable = left.crashed(Survival::Nothing);
            let file = durable.read(Path::new("/world/regions/3.region")).unwrap();
            let file = RegionFile::decode(&file.expect(&case)).unwrap();
            assert_eq!(file.epoch, 7, "{case}");
            let (_, held) = opened(&store, 3, 7);
            assert_eq!(held, [(WEST, 5)], "{case}");
        }
    }
}

// What merging and splitting ask of the store beyond the above:
// `docs/adr/0014-merging-and-splitting.md`, section 9, and T1 to T3 of its section 10.

/// Areas by their ends, in an order of their own: the records do not say in which
/// order a region that absorbed another has its areas.
type Ends = Vec<(Option<i32>, Option<i32>)>;

fn in_order(areas: &[ChunkArea]) -> Ends {
    let mut areas: Vec<_> = areas.iter().map(|area| (area.min_x, area.max_x)).collect();
    areas.sort();
    areas
}

/// The answer to a merge, taken apart: the region that was absorbed, the grants that
/// moved, and the areas that came with it, in an order of their own.
fn absorbed_with(reply: StoreReply) -> (u32, Vec<ChunkPos>, Ends) {
    match reply {
        StoreReply::Absorbed {
            absorbed,
            chunks,
            pinned,
        } => (absorbed.0, chunks, in_order(&pinned)),
        other => panic!("expected the answer to a merge, got {other:?}"),
    }
}

/// T1: the answer to a merge names the areas the absorbed region was pinned to, and
/// none for one that was pinned to nothing; its chunks are the grants that moved, which
/// are none for a region that held everything by being pinned.
#[test]
fn the_answer_to_a_merge_names_the_areas_the_absorbed_region_was_pinned_to() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (west, east) = (gap().pinned[0], gap().pinned[1]);
    let (survivor, _) = opened(&store, 0, 1);
    let (other, _) = opened(&store, 1, 1);
    let (home, _) = opened(&store, 2, 1);

    // The eastern region holds a chunk of its area, has asked the store about it and
    // has built in it: all by being pinned, so it was granted nothing.
    let inside = ChunkPos::new(20, 0);
    assert_eq!(claim(&other, &[inside]), (vec![inside], Vec::new()));
    save(&other, inside, &built(inside, blocks::GLASS));
    checkpoint(&other, 2);
    checkpoint(&survivor, 2);
    let merged = absorbed_with(absorb(&survivor, 1, 1, 5));
    assert_eq!(merged, (1, Vec::new(), in_order(&[east])));
    // The chunk is the survivor's through the area that came to it: a claim is
    // answered from the table, and the chunk is as the other region left it.
    assert_eq!(claim(&survivor, &[inside]), (vec![inside], Vec::new()));
    assert_eq!(load(&survivor, inside), built(inside, blocks::GLASS));
    let list = store.regions().unwrap();
    assert_eq!(in_order(&list.regions[0].pinned), in_order(&[west, east]));
    assert_eq!(list.regions[0].bounds, None);

    // A part is pinned to nothing, whether its chunks were granted to the region it
    // was split off or held by that region's being pinned: no areas, and its chunks.
    assert_eq!(claim(&survivor, &[FREE]), (vec![FREE], Vec::new()));
    checkpoint(&survivor, 6);
    let part = [FREE, WEST, inside];
    let done = StoreReply::Split {
        region: RegionId(3),
    };
    assert_eq!(split(&survivor, 8, &part, 1, 3), done);
    let (opened_part, held) = opened(&store, 3, 1);
    assert_eq!(held, [(WEST, 8), (FREE, 8), (inside, 8)]);
    let merged = absorbed_with(absorb(&survivor, 3, 1, 9));
    assert_eq!(merged, (3, vec![WEST, FREE, inside], Vec::new()));
    opened_part.flush();
    assert!(opened_part.is_lost());

    // A region that has come to several areas hands on all of them, with the grants
    // it has: here to the home region, which is pinned to nothing itself.
    checkpoint(&home, 2);
    let merged = absorbed_with(absorb(&home, 0, 1, 5));
    assert_eq!(
        merged,
        (0, vec![WEST, FREE, inside], in_order(&[west, east]))
    );
    survivor.flush();
    assert!(survivor.is_lost() && !home.is_lost());
    let list = store.regions().unwrap();
    assert_eq!(list.regions.len(), 1);
    assert_eq!(in_order(&list.regions[0].pinned), in_order(&[west, east]));
    // What it was told is what it is restored with after a crash.
    home.flush();
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let restored = restored_after(&disk, survival, 2, epoch);
        assert_eq!(
            in_order(&restored.pinned),
            in_order(&[west, east]),
            "{survival:?}"
        );
        let chunks: Vec<ChunkPos> = restored.held.iter().map(|(chunk, _)| *chunk).collect();
        assert_eq!(chunks, [WEST, ORIGIN, FREE, inside], "{survival:?}");
    }
}

/// How many records of the log in the world on `disk` say that a region was split.
fn splits_in_the_log(disk: &MemoryDisk) -> usize {
    let records = segments(disk).into_iter().flat_map(|number| {
        let path = format!("/world/log/{number:020}.wal");
        let bytes = disk.read(Path::new(&path)).unwrap().unwrap();
        clustine_format::read_log(&bytes).unwrap().0
    });
    records
        .filter(|record| matches!(record, LogRecord::Split { .. }))
        .count()
}

/// T2: a split that names the next region id is done and answered with it. One that
/// names another is declined with the next id and changes nothing, and the same split
/// with that id and the same tick is then done.
#[test]
fn a_split_makes_the_region_it_names_only_under_the_next_id() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (region, _) = opened(&store, 0, 1);
    assert_eq!(claim(&region, &[FREE]).0, [FREE]);
    checkpoint(&region, 2);
    let before = store.regions().unwrap();
    assert_eq!(before.next, RegionId(3));
    let part = [WEST, FREE];

    // An id above the next one, the region's own, another living region's, and the
    // highest there is: each is told the next id, and nothing has changed.
    let not_next = declined(Decline::NotNext { next: RegionId(3) });
    for named in [4, 0, 2, u32::MAX] {
        assert_eq!(split(&region, 5, &part, 7, named), not_next, "{named}");
    }
    assert_eq!(store.regions().unwrap(), before);
    assert!(!region.is_lost());
    assert_eq!(splits_in_the_log(&disk), 0);
    assert_eq!(table_file(&disk).next_region, 3);
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let left = Arc::new(disk.crashed(survival));
        let store = store_on(&left, &gap()).unwrap();
        assert_eq!(
            listed(&store.regions().unwrap()),
            listed(&before),
            "{survival:?}"
        );
        assert_eq!(opened(&store, 0, epoch).1, [(FREE, 0)], "{survival:?}");
    }

    // The same split with the id it was told, and the tick it named before.
    let done = StoreReply::Split {
        region: RegionId(3),
    };
    assert_eq!(split(&region, 5, &part, 7, 3), done);
    let list = store.regions().unwrap();
    assert_eq!(list.next, RegionId(4));
    let made = &list.regions[3];
    assert_eq!((made.region, made.epoch), (RegionId(3), 7));
    assert_eq!(opened(&store, 3, 7).1, [(WEST, 5), (FREE, 5)]);

    // The id that was the next one is not any more.
    let other = ChunkPos::new(-5, 3);
    let not_next = declined(Decline::NotNext { next: RegionId(4) });
    assert_eq!(split(&region, 6, &[other], 1, 3), not_next);
    assert_eq!(splits_in_the_log(&disk), 1);
    let done = StoreReply::Split {
        region: RegionId(4),
    };
    assert_eq!(split(&region, 6, &[other], 1, 4), done);
    assert_eq!(store.regions().unwrap().next, RegionId(5));
}

/// T2, its last part: the id is looked at last. A split that names another id than the
/// next and is wrong in another way as well is declined for that other reason, so that
/// whoever is told the next id knows that nothing else stands in the way.
#[test]
fn a_split_that_names_another_id_is_declined_for_any_other_reason_first() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    let (region, _) = opened(&store, 0, 1);
    let (home, _) = opened(&store, 2, 1);
    assert_eq!(claim(&region, &[FREE]).0, [FREE]);
    let before = store.regions().unwrap();
    let part = [WEST, FREE];
    // Not the next id, which is 3.
    let wrong = 8;

    // A commit behind the checkpoint.
    log(&region, 1, &[]);
    let uncheckpointed = Decline::Uncheckpointed {
        region: RegionId(0),
    };
    assert_eq!(split(&region, 5, &part, 1, wrong), declined(uncheckpointed));
    checkpoint(&region, 2);
    // A tick that is not above one the session named.
    let named = Decline::Tick { named: 2 };
    assert_eq!(split(&region, 2, &part, 1, wrong), declined(named));
    // No chunks, and an epoch nobody can say hello with.
    let malformed = declined(Decline::Malformed);
    assert_eq!(split(&region, 5, &[], 1, wrong), malformed);
    assert_eq!(split(&region, 5, &part, 0, wrong), malformed);
    // A chunk the region does not hold.
    let nobodys = ChunkPos::new(6, 6);
    let not_held = Decline::NotHeld { chunk: nobodys };
    assert_eq!(
        split(&region, 5, &[WEST, nobodys], 1, wrong),
        declined(not_held)
    );
    // The home chunk.
    assert_eq!(
        split(&home, 5, &[ORIGIN], 1, wrong),
        declined(Decline::Home)
    );
    // A state that no record of the log holds.
    region.request(StoreRequest::SplitCommit {
        tick: 5,
        state: Vec::new(),
        part: SplitPart {
            chunks: part.to_vec(),
            state: vec![0; clustine_format::MAX_RECORD_LENGTH],
        },
        as_epoch: 1,
        region: RegionId(wrong),
    });
    assert_eq!(reply(&region), declined(Decline::TooLarge));

    // With every other reason gone, the id is what is left, and then nothing is.
    let not_next = declined(Decline::NotNext { next: RegionId(3) });
    assert_eq!(split(&region, 5, &part, 1, wrong), not_next);
    assert_eq!(store.regions().unwrap(), before);
    assert_eq!(splits_in_the_log(&disk), 0);
    let done = StoreReply::Split {
        region: RegionId(3),
    };
    assert_eq!(split(&region, 5, &part, 1, 3), done);
}

/// T3: the next region id of the list is above every living and every absorbed region,
/// goes up by one with each split and with nothing else, and is the same after the
/// store is started again.
#[test]
fn the_list_says_the_next_region_id_which_only_a_split_raises() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &gap()).unwrap();
    // The list's next id is above every region it has, living or absorbed, and a
    // store that starts on what a crash leaves of the world says the same list.
    let next_is = |next: u32, case: &str| {
        let list = store.regions().unwrap();
        assert_eq!(list.next, RegionId(next), "{case}");
        let living = list.regions.iter().map(|info| info.region);
        let pairs = list.absorbed.iter();
        let gone = pairs.flat_map(|(absorbed, into)| [*absorbed, *into]);
        for region in living.chain(gone) {
            assert!(region.0 < next, "{case}: {region} is not below {next}");
        }
        for survival in SURVIVALS {
            let left = Arc::new(disk.crashed(survival));
            let again = store_on(&left, &gap()).unwrap().regions().unwrap();
            assert_eq!(listed(&again), listed(&list), "{case}, {survival:?}");
        }
    };
    next_is(3, "a new world");

    let (west, _) = opened(&store, 0, 1);
    let (east, _) = opened(&store, 1, 1);
    let chunks = [ChunkPos::new(-5, 0), ChunkPos::new(-5, 1), WEST];
    let done = |part: u32| StoreReply::Split {
        region: RegionId(part),
    };
    // Opening regions, claims, commits and checkpoints give out no id.
    assert_eq!(claim(&west, &[FREE]).0, [FREE]);
    checkpoint(&west, 2);
    checkpoint(&east, 2);
    next_is(3, "regions that were opened and have committed");

    // A split that is declined gives out none either; one that is done, one.
    let nobodys = ChunkPos::new(6, 6);
    let not_held = Decline::NotHeld { chunk: nobodys };
    assert_eq!(split(&west, 5, &[nobodys], 1, 3), declined(not_held));
    next_is(3, "a split that was declined");
    assert_eq!(split(&west, 5, &chunks[..1], 1, 3), done(3));
    next_is(4, "one split");

    // A merge gives out none, and the id of an absorbed region is not the next one:
    // neither a region of the division's nor the one that was made last.
    assert!(matches!(
        absorb(&west, 1, 1, 6),
        StoreReply::Absorbed { .. }
    ));
    west.flush();
    next_is(4, "a merge");
    assert_eq!(split(&west, 7, &chunks[1..2], 1, 4), done(4));
    next_is(5, "a second split");
    let (part, _) = opened(&store, 4, 1);
    assert!(matches!(
        absorb(&west, 4, 1, 8),
        StoreReply::Absorbed { .. }
    ));
    west.flush();
    next_is(5, "the region that was made last absorbed");

    // The store that is started again makes the next region under that very id.
    drop((west, east, part));
    let left = Arc::new(disk.crashed(Survival::Nothing));
    let again = store_on(&left, &gap()).unwrap();
    assert_eq!(again.regions().unwrap().next, RegionId(5));
    let (west, _) = opened(&again, 0, 2);
    let not_next = declined(Decline::NotNext { next: RegionId(5) });
    assert_eq!(split(&west, 9, &chunks[2..], 1, 4), not_next);
    assert_eq!(split(&west, 9, &chunks[2..], 1, 5), done(5));
    assert_eq!(again.regions().unwrap().next, RegionId(6));
}
