//! Tests of the regions the store keeps: the division it is started with, the table of
//! regions and who holds a chunk. See `docs/adr/0011-the-world-store-and-regions.md`.

use std::path::Path;

use clustine_data::blocks;
use clustine_format::{LogRecord, RegionFile, StateFile, TableFile};
use clustine_rpc::{ChunkBox, RegionInfo, RegionList, TickState};
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
        layout: None,
    }
}

/// The hello for a region of a world divided as `division` says.
pub(crate) fn hello_of(division: &Division, region: u32, epoch: u64) -> RegionHello {
    RegionHello {
        region: RegionId(region),
        epoch,
        layout: division.layout.unwrap_or(0),
    }
}

/// The stripes of a layout with these boundaries, with the home chunk at the origin.
fn stripes(boundaries: &[i32]) -> Division {
    Division::stripes(ORIGIN, &Layout::new(boundaries.to_vec()).unwrap())
}

/// The table file of the world on `disk`.
fn table_file(disk: &MemoryDisk) -> TableFile {
    let bytes = disk.read(Path::new("/world/regions/table")).unwrap();
    TableFile::decode(&bytes.expect("the world has a table")).unwrap()
}

/// The list without the epochs of the regions, which depend on who opened them.
fn listed(list: &RegionList) -> RegionList {
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
        // Another layout is said before that: it is what is wrong with the hello.
        let other = RegionHello {
            layout: Layout::single().fingerprint(),
            ..hello(2, 1)
        };
        assert!(matches!(
            store.open_region(other),
            Err(StoreError::LayoutMismatch { .. })
        ));
        assert_eq!(store.regions().unwrap().regions.len(), 2);
    }
    assert!(!directory.path().join("regions/2.region").exists());
    assert!(!directory.path().join("regions/9.region").exists());

    // Over a connection it is refused in words, as another layout is.
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
/// load them.
fn as_made_over(store: &Store, told: &Division, epoch: u64, case: &str) {
    let expected = table::Table::made_from(told, 0, 1).list(|_| 0);
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
        started_at_every_kill_point(&disk, &told, |store, epoch, case| {
            as_made_over(store, &told, epoch, case);
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
        layout: None,
    }
}

/// The ids of the stripes are used again by the stripes of another layout, with the
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
fn put(disk: &MemoryDisk, path: &str, contents: &[u8]) {
    let path = Path::new(path);
    replace(disk, path, contents).unwrap();
    disk.sync_directory(path.parent().unwrap()).unwrap();
}

/// A world as a store from before there was a table left it, which no store writes any
/// more: two stripes divided at x = 0 that have what the regions of [`lived_in`] have,
/// the file `layout`, and no table.
fn world_of_today() -> Arc<MemoryDisk> {
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
    let fingerprint = Layout::new(vec![0]).unwrap().fingerprint();
    put(
        &disk,
        "/world/layout",
        format!("{fingerprint:016x}\n").as_bytes(),
    );
    disk
}

/// A world from before there was a table, started with the layout it was last served
/// with, is opened as it is: the stripes keep their states, logs, epochs and entity
/// ids, the table is written and the layout file goes.
#[test]
fn a_world_of_today_with_the_same_layout_keeps_its_regions() {
    let world = world_of_today();
    let check = |store: &Store, epoch: u64, case: &str| {
        // The stripes have the epochs they had.
        assert!(
            matches!(
                store.open_region(hello(0, 2)),
                Err(StoreError::EpochRefused { seen: 3.., .. })
            ),
            "{case}"
        );
        let list = store.regions().unwrap();
        assert_eq!(
            listed(&list),
            listed(&table::Table::made_from(&division(), 0, 1).list(|_| 0))
        );
        let (_, east) = store.open_region(hello(1, epoch)).unwrap();
        assert_eq!(east.entity_ids, EntityIds::block(1).unwrap(), "{case}");
        as_lived_in(store, epoch, case);
    };
    let disk = Arc::new(world.crashed(Survival::Nothing));
    let store = store_on(&disk, &division()).unwrap();
    check(&store, 10, "at once");
    let file = table_file(&disk.crashed(Survival::Nothing));
    // What changes the table is in segments after those the world had.
    assert_eq!((file.from, file.next_region), (2, 2));
    assert_eq!(disk.read(Path::new("/world/layout")).unwrap(), None);
    started_at_every_kill_point(&world, &division(), check);
}

/// Started with another layout, or with a division that is none, it is made over.
#[test]
fn a_world_of_today_with_another_layout_is_made_over() {
    let world = world_of_today();
    for told in [stripes(&[]), stripes(&[0, 16]), gap()] {
        started_at_every_kill_point(&world, &told, |store, epoch, case| {
            as_made_over(store, &told, epoch, case);
        });
    }
}

/// A layout file that a store left because it died between making the table durable
/// and removing the file goes at the next start, and changes nothing.
#[test]
fn a_layout_file_beside_a_table_is_removed() {
    let disk = Arc::new(MemoryDisk::default());
    lived_in(&disk);
    // Of another layout than the world has, which would make a world without a table
    // over.
    let other = Layout::single().fingerprint();
    put(&disk, "/world/layout", format!("{other:016x}\n").as_bytes());
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
