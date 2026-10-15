//! The spawn as the world store keeps it, tested from the T8 plan
//! (`docs/groundwork/t8-plan.md`, T8c and section 5) by someone who had not read the
//! code: a new world's `meta` is written once with the spawn, a world keeps it whatever
//! version of the generator continues it, the home chunk is the spawn's, every region
//! is told it when opened, and a seeded world made before T8 is refused unless its
//! players entered where the game's rule puts the spawn.

use std::fs;
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;

use clustine_region::RegionId;
use clustine_rpc::{ChunkBox, RegionHello, RegionList, Restored, StoreRequest};
use clustine_world::generation::GeneratorSettings;
use clustine_world::{BlockPos, Chunk, ChunkArea, ChunkGenerator, ChunkPos};
use clustine_worldgen::{FlatGenerator, TerrainGenerator};

use crate::chunks::FileChunks;
use crate::disk::{Disk, Fault, MemoryDisk};
use crate::regions::SURVIVALS;
use crate::tests::reply;
use crate::{Pins, Store, StoreError, StoreHandle};

/// A spawn well away from the origin, in neither of its axes' first chunk, so that a
/// spawn taken from anywhere else would show.
const AWAY: BlockPos = BlockPos::new(40, -60, -300);

/// The flat world with its spawn at `spawn`.
fn flat_at(spawn: BlockPos) -> Arc<dyn ChunkGenerator> {
    Arc::new(FlatGenerator::classic().with_spawn(spawn))
}

/// The hello of a region's first owner.
fn first(region: RegionId) -> RegionHello {
    RegionHello { region, epoch: 1 }
}

fn meta(directory: &Path) -> String {
    fs::read_to_string(directory.join("meta")).unwrap()
}

fn spawn_line(spawn: BlockPos) -> String {
    format!("spawn={},{},{}", spawn.x, spawn.y, spawn.z)
}

/// The lines of `meta` that hold a spawn.
fn spawn_lines(meta: &str) -> Vec<&str> {
    meta.lines()
        .filter(|line| line.starts_with("spawn="))
        .collect()
}

/// `meta` as a world made before T8 had it: without its spawn.
fn before_t8(directory: &Path) -> String {
    let earlier: String = meta(directory)
        .lines()
        .filter(|line| !line.starts_with("spawn="))
        .map(|line| format!("{line}\n"))
        .collect();
    fs::write(directory.join("meta"), &earlier).unwrap();
    earlier
}

/// Whether a region is pinned to or holds the chunk, by what the list says of it.
fn lists_chunk(list: &RegionList, region: RegionId, chunk: ChunkPos) -> bool {
    let info = list
        .regions
        .iter()
        .find(|info| info.region == region)
        .expect("the region is listed");
    let in_bounds = info.bounds.is_some_and(|bounds| {
        (bounds.min.x..=bounds.max.x).contains(&chunk.x)
            && (bounds.min.z..=bounds.max.z).contains(&chunk.z)
    });
    in_bounds || info.pinned.iter().any(|area| area.contains(chunk))
}

/// Holds the store to the home region being the one, and the only one, that has the
/// spawn's chunk, and returns the list and what the home region is opened with.
fn home_holds_the_spawn(store: &Store, spawn: BlockPos) -> (RegionList, Restored) {
    let list = store.regions().unwrap();
    assert_eq!(list.spawn, spawn);
    let chunk = spawn.chunk();
    for info in &list.regions {
        assert_eq!(
            lists_chunk(&list, info.region, chunk),
            info.region == list.home,
            "{:?} and the home chunk {chunk:?}, with {:?} home: {list:?}",
            info.region,
            list.home
        );
    }
    // Above any epoch the region was opened with before, so that the world can be
    // looked at again after its regions had owners.
    let epoch = list
        .regions
        .iter()
        .find(|info| info.region == list.home)
        .expect("the home region is listed")
        .epoch;
    let hello = RegionHello {
        region: list.home,
        epoch: epoch + 1,
    };
    let (handle, restored) = store.open_region(hello).unwrap();
    drop(handle);
    assert_eq!(restored.spawn, spawn);
    (list, restored)
}

/// T8c: "A crash before that leaves no `meta`, so the world is new again, and the
/// search, being a function of the seed, gives the same spawn." Wherever the making of
/// a new world stops, and whatever a crash then keeps, there is no `meta`, or one with
/// the spawn; and the world opened again after it has the same spawn.
#[test]
fn a_new_world_killed_at_any_step_of_its_making_has_no_meta_or_one_with_the_spawn() {
    let generator = flat_at(AWAY);
    let mut seen_none = false;
    let mut seen_some = false;
    let mut made = false;
    for stop in 1..10_000 {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let disk = Arc::new(MemoryDisk::failing(Fault::Stop(stop)));
        let shared: Arc<dyn Disk> = disk.clone();
        // The whole of the making: `meta`, and then the store's table and log.
        let whole = crate::local::prepare(&shared, root, generator.as_ref()).and_then(|meta| {
            assert_eq!(meta.spawn, AWAY, "stopped at {stop}");
            let spawn = meta.spawn;
            let chunks = FileChunks::new(Arc::clone(&shared), root).with_meta(meta);
            let store = crate::begin(
                Arc::clone(&shared),
                root,
                Box::new(chunks),
                Arc::clone(&generator),
                Pins::none(),
                spawn,
            )?;
            store.regions()
        });
        for survival in SURVIVALS {
            let left = disk.crashed(survival);
            let found = left.read(&root.join("meta")).unwrap();
            let Some(found) = found else {
                seen_none = true;
                continue;
            };
            seen_some = true;
            let found = String::from_utf8(found).unwrap();
            assert_eq!(
                spawn_lines(&found),
                [spawn_line(AWAY)],
                "stopped at {stop}, {survival:?}: {found}"
            );
            // The world as the crash left its `meta` opens with the spawn it holds.
            let again = tempfile::tempdir().unwrap();
            fs::write(again.path().join("meta"), &found).unwrap();
            let store = Store::local_divided(again.path(), flat_at(AWAY), Pins::none())
                .unwrap_or_else(|error| panic!("stopped at {stop}, {survival:?}: {error}"));
            assert_eq!(store.regions().unwrap().spawn, AWAY);
        }
        if let Ok(list) = whole {
            assert_eq!(list.spawn, AWAY);
            made = true;
            break;
        }
    }
    assert!(made, "the world was never made");
    assert!(
        seen_none && seen_some,
        "the kills did not reach both sides of writing meta"
    );
}

/// The flat world, said to be made by `version` of its generator, whose search puts the
/// spawn at `spawn`.
struct Versioned {
    version: u16,
    spawn: BlockPos,
}

impl ChunkGenerator for Versioned {
    fn generate(&self, position: ChunkPos) -> Chunk {
        let mut chunk = FlatGenerator::classic().generate(position);
        chunk.mark_generated(self.version, false);
        chunk
    }

    fn settings(&self) -> GeneratorSettings {
        GeneratorSettings {
            identity: FlatGenerator::classic().settings().identity,
            version: self.version,
        }
    }

    fn spawn(&self) -> BlockPos {
        self.spawn
    }
}

/// T8c: "Raising `generator-version` keeps the line", and "a world keeps it from then
/// on, whatever version of the generator continues it".
#[test]
fn the_spawn_survives_a_newer_generator_that_would_put_it_elsewhere() {
    let directory = tempfile::tempdir().unwrap();
    let elsewhere = BlockPos::new(-500, -60, 77);
    let older = Arc::new(Versioned {
        version: 1,
        spawn: AWAY,
    });
    let store = Store::local_divided(directory.path(), older, Pins::whole_world()).unwrap();
    assert_eq!(store.regions().unwrap().spawn, AWAY);
    drop(store);

    let newer = || {
        Arc::new(Versioned {
            version: 2,
            spawn: elsewhere,
        })
    };
    let store = Store::local_divided(directory.path(), newer(), Pins::whole_world()).unwrap();
    let (_, restored) = home_holds_the_spawn(&store, AWAY);
    assert_eq!(restored.spawn, AWAY);
    // A chunk of the newer version is stored, which raises the version `meta` names.
    let list = store.regions().unwrap();
    let (handle, _) = store
        .open_region(RegionHello {
            region: list.home,
            epoch: 10,
        })
        .unwrap();
    let position = ChunkPos::new(-30, 12);
    handle.request(StoreRequest::Load { position });
    let _ = reply(&handle);
    drop(handle);
    store.close().unwrap();
    drop(store);
    let text = meta(directory.path());
    assert!(text.contains("\ngenerator-version=2\n"), "{text}");
    assert_eq!(spawn_lines(&text), [spawn_line(AWAY)], "{text}");

    // And the world opened again by the newer generator still has it.
    let store = Store::local_divided(directory.path(), newer(), Pins::whole_world()).unwrap();
    home_holds_the_spawn(&store, AWAY);
    drop(store);
    let store = Store::local_divided(directory.path(), newer(), Pins::none()).unwrap();
    home_holds_the_spawn(&store, AWAY);
}

/// T8c: "The store derives the home chunk itself from `meta`'s spawn"; with no pins
/// the home region is one pinned to nothing that is granted the home chunk
/// (ADR-0017).
#[test]
fn with_no_pins_the_home_region_is_granted_the_spawns_chunk() {
    let spawn = BlockPos::new(-200, -60, 37);
    let chunk = ChunkPos::new(-13, 2);
    assert_eq!(spawn.chunk(), chunk);
    let directory = tempfile::tempdir().unwrap();
    let local = Store::local_divided(directory.path(), flat_at(spawn), Pins::none()).unwrap();
    let memory = Store::memory_divided(flat_at(spawn), Pins::none()).unwrap();
    for store in [memory, local] {
        let (list, restored) = home_holds_the_spawn(&store, spawn);
        assert_eq!(list.regions.len(), 1, "{list:?}");
        let home = &list.regions[0];
        assert!(home.pinned.is_empty(), "{list:?}");
        assert_eq!(
            home.bounds,
            Some(ChunkBox {
                min: chunk,
                max: chunk
            })
        );
        assert!(restored.pinned.is_empty());
        assert_eq!(restored.held, [(chunk, 0)]);
    }
}

/// T8c: "With `--pin`, the home region is whichever pinned area holds the spawn's
/// chunk": west of the cuts, between them, and east of every cut.
#[test]
fn with_pins_the_home_region_is_the_pinned_one_that_holds_the_spawns_chunk() {
    let cuts = [-2, 4];
    for (spawn, home) in [
        (BlockPos::new(-80, -60, 3), RegionId(0)),
        (BlockPos::new(-32, -60, -900), RegionId(1)),
        (BlockPos::new(20, -60, 5), RegionId(1)),
        (BlockPos::new(63, -60, 5), RegionId(1)),
        (BlockPos::new(64, -60, 5), RegionId(2)),
        // East of every cut, at chunk 9.
        (BlockPos::new(150, -60, -20), RegionId(2)),
    ] {
        let pins = || Pins::side_by_side(&cuts).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let local = Store::local_divided(directory.path(), flat_at(spawn), pins()).unwrap();
        let memory = Store::memory_divided(flat_at(spawn), pins()).unwrap();
        for store in [memory, local] {
            let (list, restored) = home_holds_the_spawn(&store, spawn);
            assert_eq!(list.home, home, "{spawn:?}");
            // No region is made for the home chunk beside the pinned ones.
            assert_eq!(list.regions.len(), 3, "{spawn:?}: {list:?}");
            assert!(
                restored
                    .pinned
                    .iter()
                    .any(|area| area.contains(spawn.chunk())),
                "{spawn:?}: {restored:?}"
            );
            // Held by being pinned, not by a grant.
            assert!(restored.held.is_empty(), "{spawn:?}: {restored:?}");
        }
        // Started again, the world has the same home.
        let store = Store::local_divided(directory.path(), flat_at(AWAY), pins()).unwrap();
        let (list, _) = home_holds_the_spawn(&store, spawn);
        assert_eq!(list.home, home, "{spawn:?}");
    }
}

/// The spawn east of every cut, as the brief has it: cuts at -2 and 4, the spawn at
/// chunk 9. The last pinned region, which reaches east without end, is home.
#[test]
fn a_spawn_east_of_every_cut_is_in_the_last_pinned_region() {
    let spawn = BlockPos::new(9 * 16 + 3, -60, 40);
    assert_eq!(spawn.chunk(), ChunkPos::new(9, 2));
    let store =
        Store::memory_divided(flat_at(spawn), Pins::side_by_side(&[-2, 4]).unwrap()).unwrap();
    let (list, restored) = home_holds_the_spawn(&store, spawn);
    assert_eq!(list.home, RegionId(2));
    assert_eq!(
        restored.pinned,
        [ChunkArea {
            min_x: Some(4),
            max_x: None
        }]
    );
}

/// ADR-0017: where no pinned area holds the home chunk, a region made after the pinned
/// ones is granted it.
#[test]
fn a_spawn_in_a_gap_between_pinned_areas_is_granted_to_a_region_of_its_own() {
    let spawn = BlockPos::new(5 * 16 + 15, -60, -3 * 16);
    let chunk = ChunkPos::new(5, -3);
    assert_eq!(spawn.chunk(), chunk);
    let pins = Pins {
        areas: vec![
            ChunkArea {
                min_x: None,
                max_x: Some(0),
            },
            ChunkArea {
                min_x: Some(16),
                max_x: None,
            },
        ],
    };
    let store = Store::memory_divided(flat_at(spawn), pins).unwrap();
    let (list, restored) = home_holds_the_spawn(&store, spawn);
    assert_eq!(list.home, RegionId(2));
    assert_eq!(list.regions.len(), 3);
    assert!(restored.pinned.is_empty());
    assert_eq!(restored.held, [(chunk, 0)]);
}

/// T8c: "The store tells each region the spawn when it opens it (in `StoreWelcome`)":
/// every region, the home one and the others, in the store's process and over TCP; and
/// the list says it too.
#[test]
fn every_region_is_told_the_spawn_when_opened_in_memory_and_over_tcp() {
    let directory = tempfile::tempdir().unwrap();
    let pins = || Pins::side_by_side(&[-2, 4]).unwrap();
    let local = Store::local_divided(directory.path(), flat_at(AWAY), pins()).unwrap();
    let memory = Store::memory_divided(flat_at(AWAY), pins()).unwrap();
    for store in [memory, local] {
        let list = store.regions().unwrap();
        assert_eq!(list.spawn, AWAY);
        let regions: Vec<RegionId> = list.regions.iter().map(|info| info.region).collect();
        assert_eq!(regions.len(), 3);
        for region in &regions {
            let (handle, restored) = store.open_region(first(*region)).unwrap();
            assert_eq!(restored.spawn, AWAY, "{region:?}");
            drop(handle);
        }

        let server = crate::serve(store, TcpListener::bind("127.0.0.1:0").unwrap()).unwrap();
        let address = server.local_addr().to_string();
        assert_eq!(crate::regions(&address).unwrap().spawn, AWAY);
        for region in &regions {
            let hello = RegionHello {
                region: *region,
                epoch: 2,
            };
            let (handle, restored) = StoreHandle::connect(&address, hello).unwrap();
            assert_eq!(restored.spawn, AWAY, "{region:?} over TCP");
            drop(handle);
        }
        server.stop();
    }
}

/// The same over TCP for a world with no pins, whose one region is home.
#[test]
fn the_home_region_of_a_world_without_pins_is_told_the_spawn_over_tcp() {
    let store = Store::memory_divided(flat_at(AWAY), Pins::none()).unwrap();
    let server = crate::serve(store, TcpListener::bind("127.0.0.1:0").unwrap()).unwrap();
    let address = server.local_addr().to_string();
    let list = crate::regions(&address).unwrap();
    assert_eq!(list.spawn, AWAY);
    let (handle, restored) = StoreHandle::connect(&address, first(list.home)).unwrap();
    assert_eq!(restored.spawn, AWAY);
    assert_eq!(restored.held, [(AWAY.chunk(), 0)]);
    drop(handle);
    server.stop();
}

/// T8c: "The flat world keeps 0.5, -60, 0.5, which is what the game's rule gives
/// there": its spawn is the block 0, -60, 0 and its home chunk 0, 0.
#[test]
fn the_flat_world_keeps_its_spawn_at_the_origin() {
    let spawn = BlockPos::new(0, -60, 0);
    let flat = || -> Arc<dyn ChunkGenerator> { Arc::new(FlatGenerator::classic()) };
    assert_eq!(flat().spawn(), spawn);
    assert_eq!(flat().origin(), ChunkPos::new(0, 0));

    let directory = tempfile::tempdir().unwrap();
    let local = Store::local_divided(directory.path(), flat(), Pins::none()).unwrap();
    for store in [Store::memory_divided(flat(), Pins::none()).unwrap(), local] {
        let (list, restored) = home_holds_the_spawn(&store, spawn);
        assert_eq!(list.spawn.chunk(), ChunkPos::new(0, 0));
        assert_eq!(restored.held, [(ChunkPos::new(0, 0), 0)]);
    }
    assert_eq!(spawn_lines(&meta(directory.path())), ["spawn=0,-60,0"]);

    // The world of one region the single process has always had.
    let store = Store::memory(flat());
    let (list, restored) = home_holds_the_spawn(&store, spawn);
    assert_eq!(list.home, RegionId(0));
    assert_eq!(restored.pinned, [ChunkArea::EVERYWHERE]);
}

/// Question 3: a seeded world made before T8 is refused, as its players entered on the
/// column at the origin and the game puts its spawn elsewhere; and nothing in it is
/// changed. Seed 13579 puts it at -96, 66, 32.
#[test]
fn a_seeded_world_from_before_t8_whose_spawn_would_move_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    let spawn = BlockPos::new(-96, 66, 32);
    let generator = || Arc::new(TerrainGenerator::new(13579));
    let store = Store::local_divided(directory.path(), generator(), Pins::none()).unwrap();
    assert_eq!(store.regions().unwrap().spawn, spawn);
    drop(store);
    let earlier = before_t8(directory.path());

    let Err(error) = Store::local_divided(directory.path(), generator(), Pins::none()) else {
        panic!("a world made before T8 whose spawn would move was opened");
    };
    match &error {
        StoreError::SpawnWouldMove {
            spawn: said,
            directory: named,
        } => {
            assert_eq!(*said, spawn);
            assert_eq!(named, directory.path());
        }
        other => panic!("refused otherwise: {other}"),
    }
    let said = error.to_string();
    assert!(
        said.contains(&directory.path().display().to_string()),
        "{said}"
    );
    assert_eq!(meta(directory.path()), earlier);
    // Refused again, the same: nothing was left half done.
    assert!(matches!(
        Store::local_divided(directory.path(), generator(), Pins::none()),
        Err(StoreError::SpawnWouldMove { .. })
    ));
    assert_eq!(meta(directory.path()), earlier);
}

/// Question 3, the other side: a seeded world made before T8 whose spawn the game puts
/// on the column at block 0, 0, where its players entered, gets the line and opens.
/// Seed 3 puts it at 0, 71, 0.
#[test]
fn a_seeded_world_from_before_t8_whose_spawn_is_at_the_origin_opens_and_keeps_it() {
    let directory = tempfile::tempdir().unwrap();
    let spawn = BlockPos::new(0, 71, 0);
    let generator = || Arc::new(TerrainGenerator::new(3));
    drop(Store::local_divided(directory.path(), generator(), Pins::none()).unwrap());
    let earlier = before_t8(directory.path());

    let store = Store::local_divided(directory.path(), generator(), Pins::none()).unwrap();
    home_holds_the_spawn(&store, spawn);
    drop(store);
    let text = meta(directory.path());
    assert_eq!(spawn_lines(&text), [spawn_line(spawn)], "{text}");
    // Everything else it had is kept.
    for line in earlier.lines() {
        assert!(text.lines().any(|kept| kept == line), "{line} went: {text}");
    }
    let store = Store::local_divided(directory.path(), generator(), Pins::none()).unwrap();
    home_holds_the_spawn(&store, spawn);
}

/// Question 3 goes by the column, not the chunk: a spawn one block off the column at
/// the origin, in the same chunk or the one beside it, is a move.
#[test]
fn a_world_from_before_t8_whose_spawn_is_one_block_off_the_origin_column_is_refused() {
    for off in [
        BlockPos::new(1, -60, 0),
        BlockPos::new(0, -60, 1),
        BlockPos::new(0, -60, -1),
        BlockPos::new(-1, -60, 0),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let classic = Arc::new(FlatGenerator::classic());
        drop(Store::local_divided(directory.path(), classic, Pins::none()).unwrap());
        let earlier = before_t8(directory.path());
        let opened = Store::local_divided(directory.path(), flat_at(off), Pins::none());
        assert!(
            matches!(opened, Err(StoreError::SpawnWouldMove { spawn, .. }) if spawn == off),
            "{off:?}"
        );
        assert_eq!(meta(directory.path()), earlier);
    }
}
