//! Tests of a world that is one home region pinned to nothing, of regions pinned side
//! by side, and of the worlds from before them, which a store makes over. See sections
//! 2.1 and 2.2 of `docs/adr/0017-the-end-of-the-stripes.md`.

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use clustine_data::{BlockState, blocks};
use clustine_format::{LogRecord, TableFile};
use clustine_rpc::{ChunkBox, Decline, RegionInfo, RegionList};
use clustine_world::{Chunk, ChunkPos, EntityId, EntityIds};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Dispatch, Event, Level, Metadata, Subscriber};

use super::*;
use crate::disk::{Fault, MemoryDisk, Survival};
use crate::regions::{
    SURVIVALS, absorb, claim, give_back, hello_of, listed, put, split, state_of, store_on,
    table_file, whole, world_of_today,
};
use crate::tests::{generator, load, log, open, reply, save};

/// Where players enter the worlds of these tests: not at the origin, so that nothing
/// here rests on its being there.
pub(crate) const HOME: ChunkPos = ChunkPos::new(2, -3);

const ORIGIN: ChunkPos = ChunkPos::new(0, 0);

/// A world that is one home region, pinned to nothing.
pub(crate) fn open_world() -> Division {
    Division::open(HOME)
}

/// Regions pinned side by side, cut at these chunk x coordinates.
fn pinned_at(cuts: &[i32]) -> Division {
    Division::side_by_side(HOME, cuts).unwrap()
}

/// The stripes of a layout with these boundaries, as a store was told until now.
fn stripes(boundaries: &[i32]) -> Division {
    Division::stripes(HOME, &Layout::new(boundaries.to_vec()).unwrap())
}

/// The block of entity ids of a region that has none.
pub(crate) const NO_ENTITY_IDS: EntityIds = EntityIds {
    first: EntityId(0),
    end: EntityId(0),
};

/// The smallest box around `chunks`.
fn around(chunks: &[ChunkPos]) -> Option<ChunkBox> {
    let xs = || chunks.iter().map(|chunk| chunk.x);
    let zs = || chunks.iter().map(|chunk| chunk.z);
    Some(ChunkBox {
        min: ChunkPos::new(xs().min()?, zs().min()?),
        max: ChunkPos::new(xs().max()?, zs().max()?),
    })
}

/// The list of a world that is one home region which holds the home chunk at `home`
/// and nothing else, with `next` as the next region id and without its epoch.
fn alone(home: ChunkPos, next: u32) -> RegionList {
    RegionList {
        home: RegionId(0),
        regions: vec![RegionInfo {
            region: RegionId(0),
            epoch: 0,
            bounds: around(&[home]),
            pinned: Vec::new(),
        }],
        absorbed: Vec::new(),
        next: RegionId(next),
    }
}

/// What a test has built: the blocks it set, chunk by chunk and in the order it set
/// them, each where it is in its chunk.
#[derive(Debug, Clone, Default)]
pub(crate) struct Built(BTreeMap<ChunkPos, Vec<(usize, i32, usize, BlockState)>>);

impl Built {
    /// Notes that the block at `(x, y, z)` of the chunk at `position` is set to
    /// `state`, and returns the change as a commit names it.
    pub(crate) fn set(
        &mut self,
        position: ChunkPos,
        (x, y, z): (usize, i32, usize),
        state: BlockState,
    ) -> (i32, i32, i32, BlockState) {
        self.0.entry(position).or_default().push((x, y, z, state));
        let block = |chunk: i32, within: usize| chunk * 16 + within as i32;
        (block(position.x, x), y, block(position.z, z), state)
    }

    /// The chunk at `position` with everything that was built in it.
    pub(crate) fn chunk(&self, position: ChunkPos) -> Chunk {
        let mut chunk = generator().generate(position);
        for (x, y, z, state) in self.0.get(&position).into_iter().flatten() {
            chunk.set(*x, *y, *z, *state);
        }
        chunk
    }

    /// The chunks something was built in, in ascending order.
    pub(crate) fn positions(&self) -> Vec<ChunkPos> {
        self.0.keys().copied().collect()
    }
}

// The lines of the store's log.

/// What the store says in its log when it has made a world over, at the level of a
/// note.
pub(crate) const MADE_OVER: &str =
    "the world was divided otherwise before; what its regions had is in the stored chunks now";

/// What it says with it, at the level of a warning.
pub(crate) const BEGIN_ANEW: &str = "the regions of this world begin anew: whoever is in it has to join again. Stop the workers and the edges of a cluster before its world store is started with other pins";

/// Listens to the log and keeps each line with its level: the message, and behind it
/// the fields the line was given, as `name=value` with a space before each.
#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<(Level, String)>>>);

#[derive(Default)]
struct Line {
    message: String,
    fields: String,
}

impl Visit for Line {
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.fields
                .push_str(&format!(" {}={value:?}", field.name()));
        }
    }
}

impl Subscriber for Lines {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut line = Line::default();
        event.record(&mut line);
        let lines = &mut self.0.lock().expect("nothing panics that holds the lines");
        lines.push((*event.metadata().level(), line.message + &line.fields));
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}

/// Runs `what`, and returns what it returns with the lines that the thread it ran on
/// wrote to the log meanwhile, each with its level.
///
/// A store reads its world on the thread that starts it, so what a start says of the
/// world it found is among the lines. What the store's own threads say later is not.
pub(crate) fn logged<T>(what: impl FnOnce() -> T) -> (T, Vec<(Level, String)>) {
    // Whether anybody listens to a line is remembered where the line is written, when
    // the first thread gets there. While the process has a single listener, only that
    // thread's own is asked, and the tests run side by side, most of them with nobody
    // listening. Once there are two, every listener there is is asked; so one is kept
    // for good here, which no thread ever writes to.
    static KEPT: OnceLock<Dispatch> = OnceLock::new();
    KEPT.get_or_init(|| Dispatch::new(Lines::default()));
    let lines = Lines::default();
    let returned = tracing::subscriber::with_default(lines.clone(), what);
    let lines = lines.0.lock().expect("nothing panics that holds the lines");
    (returned, lines.clone())
}

/// Whether `lines` say that a world was made over: both lines of section 2.2 of
/// ADR-0017, once each and each at its level, or neither of them.
pub(crate) fn says_made_over(lines: &[(Level, String)], case: &str) -> bool {
    let said = |level: Level, text: &str| {
        let same = |(at, line): &&(Level, String)| *at == level && line == text;
        lines.iter().filter(same).count()
    };
    let counted = (said(Level::INFO, MADE_OVER), said(Level::WARN, BEGIN_ANEW));
    assert!(counted == (0, 0) || counted == (1, 1), "{case}: {lines:?}");
    // And at no other level.
    let either = |(_, line): &&(Level, String)| line == MADE_OVER || line == BEGIN_ANEW;
    assert_eq!(
        lines.iter().filter(either).count(),
        counted.0 + counted.1,
        "{case}: {lines:?}"
    );
    counted == (1, 1)
}

/// Starts a store for `told` on `disk`, which has to say in its log that it made the
/// world over, or to say nothing of it, as `made_over` says.
fn started(disk: &Arc<MemoryDisk>, told: &Division, made_over: bool, case: &str) -> Store {
    let (store, lines) = logged(|| store_on(disk, told));
    let store = store.unwrap_or_else(|error| panic!("{case}: {error}"));
    assert_eq!(says_made_over(&lines, case), made_over, "{case}: {lines:?}");
    store
}

/// Whether the world on `disk` has a table that was made from `told`: whether a store
/// that is started with `told` finds it as it is.
fn is_of(disk: &MemoryDisk, told: &Division) -> bool {
    let table = disk.read(Path::new("/world/regions/table")).unwrap();
    table.is_some_and(|bytes| {
        let file = TableFile::decode(&bytes).unwrap();
        file.division == told.pinned && file.home_chunk == told.home
    })
}

/// Starts a store for `told` on what a crash leaves of `world`, killed at every change
/// and sync of that start and with everything a crash can keep of it; starts one on
/// what is left, which has to be as `check` says, and one more on what that one left,
/// which has to be the same. `check` is given the disk its store runs on and the epoch
/// to open regions with. This is how `regions.rs` kills a start.
///
/// Besides, each of these starts says in its log that it has made the world over
/// exactly if the table it found was not that of `told`, and the one more never does.
fn started_at_every_kill_point(
    world: &MemoryDisk,
    told: &Division,
    check: impl Fn(&Store, &MemoryDisk, u64, &str),
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
            // It fails or not; either way it has done what it has, and has not said
            // half of what there is to say.
            let (_, lines) = logged(|| drop(store_on(&disk, told)));
            says_made_over(&lines, &format!("{fault:?}"));
            for survival in SURVIVALS {
                let case = format!("{fault:?}, {survival:?}");
                let left = Arc::new(disk.crashed(survival));
                let store = started(&left, told, !is_of(&left, told), &case);
                check(&store, &left, 100, &case);
                assert_eq!(left.read(Path::new("/world/layout")).unwrap(), None);
                let again = Arc::new(left.crashed(Survival::Nothing));
                let case = format!("{case}, again");
                let store = started(&again, told, false, &case);
                check(&store, &again, 101, &case);
            }
        }
    }
}

/// Opens every region of the list of `store` with `epoch`, as a region of `told`, and
/// returns the list without its epochs and each region's handle with what the region
/// is restored with.
pub(crate) fn opened(
    store: &Store,
    told: &Division,
    epoch: u64,
    case: &str,
) -> (RegionList, Vec<(StoreHandle, Restored)>) {
    let list = store
        .regions()
        .unwrap_or_else(|error| panic!("{case}: {error}"));
    let with_epoch = |info: &RegionInfo| {
        store
            .open_region(hello_of(told, info.region.0, epoch))
            .unwrap_or_else(|error| panic!("{case}: region {}: {error}", info.region))
    };
    let regions = list.regions.iter().map(with_epoch).collect();
    (listed(&list), regions)
}

/// Waits for the next answer to `handle`. The checks that are made at every point a
/// store can be killed at ask thousands of times, and do not look again and again
/// for what is not there yet, as [`reply`] does.
pub(crate) fn answer(handle: &StoreHandle, case: &str) -> StoreReply {
    let answered = handle.replies.recv();
    answered.unwrap_or_else(|_| panic!("{case}: the handle is lost"))
}

/// Every chunk something was built in is as `built` has it, as the one region that
/// holds it loads it. A chunk that is nobody's is claimed by the home region to be
/// looked at, and given back, so that the world is left as it was found.
fn as_built(
    list: &RegionList,
    told: &Division,
    regions: &[(StoreHandle, Restored)],
    built: &Built,
    case: &str,
) {
    let home = list
        .regions
        .iter()
        .position(|info| info.region == list.home);
    let (home, restored) = &regions[home.expect("the home region is in the list")];
    home.request(StoreRequest::Claim {
        chunks: built.positions(),
    });
    let StoreReply::Claimed { granted, .. } = answer(home, case) else {
        panic!("{case}: a claim is answered as one");
    };
    for position in built.positions() {
        let mut loaded = Vec::new();
        for (handle, _) in regions {
            handle.request(StoreRequest::Load { position });
            match answer(handle, case) {
                StoreReply::Loaded { chunk, .. } => loaded.push(chunk),
                StoreReply::NotHeld { .. } => {}
                other => panic!("{case}: {other:?}"),
            }
        }
        assert_eq!(
            loaded.len(),
            1,
            "{case}: {position:?} is loaded by one region"
        );
        assert!(
            loaded[0] == built.chunk(position),
            "{case}: {position:?} is not as it was built"
        );
    }
    // What it was granted only now; not what it holds anyway.
    let own = |chunk: &ChunkPos| {
        *chunk == told.home
            || restored.pinned.iter().any(|area| area.contains(*chunk))
            || restored.held.iter().any(|(held, _)| held == chunk)
    };
    let back: Vec<ChunkPos> = granted.into_iter().filter(|chunk| !own(chunk)).collect();
    give_back(home, &back);
    for (handle, _) in regions {
        handle.flush();
    }
}

/// The world on the disk of `store` is made over for `told`: its regions are those of
/// `told` and are restored with nothing, and everything of `built` is in the chunks as
/// their holders load them. `used` is the next region id of the table the world had,
/// 0 if it had none. Returns what each region was restored with.
fn as_made_over(
    store: &Store,
    told: &Division,
    used: u32,
    built: &Built,
    epoch: u64,
    case: &str,
) -> Vec<Restored> {
    let expected = table::Table::made_from(told, used, 1).list(|_| 0);
    let (list, regions) = opened(store, told, epoch, case);
    assert_eq!(list, expected, "{case}");
    for (info, (_, restored)) in list.regions.iter().zip(&regions) {
        assert_eq!(
            (&restored.state, &restored.deltas),
            (&None, &Vec::new()),
            "{case}"
        );
        assert_eq!(restored.pinned, info.pinned, "{case}");
        // The home region of a world without pins holds the home chunk from the
        // start, and nobody holds anything else that is not pinned.
        let held = match info.pinned.is_empty() {
            true => vec![(told.home, 0)],
            false => Vec::new(),
        };
        assert_eq!(restored.held, held, "{case}");
    }
    as_built(&list, told, &regions, built, case);
    regions.into_iter().map(|(_, restored)| restored).collect()
}

/// The names of the files the world on `disk` has of its regions.
fn region_files(disk: &MemoryDisk) -> Vec<String> {
    disk.list(Path::new("/world/regions")).unwrap()
}

// Section 2.1: a world without pins.

/// T1: the world a store makes when it is told no pins, in memory, in a directory and
/// on the disk of these tests.
#[test]
fn a_new_world_without_pins_is_one_home_region_that_holds_the_home_chunk() {
    let directory = tempfile::tempdir().unwrap();
    let disk = Arc::new(MemoryDisk::default());
    let told = open_world();
    let stores = [
        Store::memory_divided(generator(), told.clone()).unwrap(),
        Store::local_divided(directory.path(), generator(), told.clone()).unwrap(),
        store_on(&disk, &told).unwrap(),
    ];
    for store in stores {
        assert_eq!(store.regions().unwrap(), alone(HOME, 1));
        // A hello makes no region, also where there is one only.
        for region in [1, 2] {
            let refused = store.open_region(hello_of(&told, region, 1));
            assert!(
                matches!(refused, Err(StoreError::UnknownRegion { .. })),
                "{:?}",
                refused.err()
            );
        }
        let (_, restored) = store.open_region(hello_of(&told, 0, 5)).unwrap();
        // It is issued entity ids as the home region, though it is pinned to nothing.
        assert_eq!(restored.entity_ids, EntityIds::block(0).unwrap());
        assert_eq!((restored.state, restored.deltas), (None, Vec::new()));
        assert_eq!(restored.held, [(HOME, 0)]);
        assert_eq!(restored.pinned, []);
        assert_eq!(store.regions().unwrap().regions[0].epoch, 5);
        assert_eq!(store.regions().unwrap().next, RegionId(1));
        // A hello is held to no layout: whatever fingerprint it names, it is a region
        // and an epoch.
        let named = RegionHello {
            layout: Layout::new(vec![4]).unwrap().fingerprint(),
            ..hello_of(&told, 0, 5)
        };
        let (home, restored) = store.open_region(named).unwrap();
        assert_eq!(restored.held, [(HOME, 0)]);

        // The home chunk is its own, and every other chunk is nobody's.
        assert_eq!(load(&home, HOME), generator().generate(HOME));
        for position in [ORIGIN, ChunkPos::new(3, -3), ChunkPos::new(-40, 90)] {
            home.request(StoreRequest::Load { position });
            let nobodys = StoreReply::NotHeld {
                position,
                holder: None,
            };
            assert_eq!(reply(&home), nobodys);
        }
        home.flush();
    }

    // The table has no area, and is on disk for good before the store has said
    // anything; opening the region does not write it again.
    let in_the_directory = std::fs::read(directory.path().join("regions/table")).unwrap();
    let tables = SURVIVALS
        .map(|survival| table_file(&disk.crashed(survival)))
        .into_iter()
        .chain([TableFile::decode(&in_the_directory).unwrap()]);
    for file in tables {
        assert_eq!((file.from, file.next_region), (1, 1));
        assert_eq!((file.home_chunk, file.home_region), (HOME, 0));
        assert_eq!(file.division, []);
        assert_eq!(file.regions.len(), 1);
        assert_eq!((file.regions[0].id, &file.regions[0].pinned), (0, &vec![]));
        assert_eq!(file.regions[0].grants, [(HOME, 0)]);
        assert_eq!(file.absorbed, []);
    }
}

/// What the home region of the world without pins on what a crash leaves of `disk` was
/// granted, as a store that starts there says when the region is opened with `epoch`.
fn held_after(disk: &MemoryDisk, survival: Survival, epoch: u64) -> Vec<(ChunkPos, u64)> {
    let left = Arc::new(disk.crashed(survival));
    let store = store_on(&left, &open_world()).unwrap();
    let (_, restored) = store
        .open_region(hello_of(&open_world(), 0, epoch))
        .unwrap();
    restored.held
}

/// T2: every chunk is free in such a world, and is the home region's for the asking;
/// what it gives back is free again, but for the home chunk.
#[test]
fn the_home_region_is_granted_what_it_claims_and_never_gives_the_home_chunk_back() {
    let disk = Arc::new(MemoryDisk::default());
    let told = open_world();
    let store = store_on(&disk, &told).unwrap();
    let home = open(&store, hello_of(&told, 0, 1));

    // Twenty chunks, and the home chunk with them, which is its own already.
    let twenty: Vec<ChunkPos> = (0..20)
        .map(|n| ChunkPos::new(HOME.x - 2 + n % 5, HOME.z + 1 + n / 5))
        .collect();
    let asked: Vec<ChunkPos> = twenty.iter().copied().chain([HOME]).collect();
    assert_eq!(claim(&home, &asked), (asked.clone(), Vec::new()));
    let mut held: Vec<(ChunkPos, u64)> = asked.iter().map(|chunk| (*chunk, 0)).collect();
    held.sort();
    // Answered, so it is on disk for good.
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        assert_eq!(held_after(&disk, survival, epoch), held, "{survival:?}");
    }
    let records = disk.read(Path::new("/world/log/00000000000000000001.wal"));
    let (records, _) = clustine_format::read_log(&records.unwrap().unwrap()).unwrap();
    let granted: Vec<&LogRecord> = records
        .iter()
        .filter(|record| matches!(record, LogRecord::Granted { .. }))
        .collect();
    let expected = LogRecord::Granted {
        region: 0,
        tick: 0,
        chunks: twenty.clone(),
    };
    assert_eq!(granted, [&expected]);
    let list = store.regions().unwrap();
    assert_eq!(list.regions[0].bounds, around(&asked));

    // A chunk it has built in and saved, given back with eighteen more and with the
    // home chunk.
    let mut built = Built::default();
    let (first, kept) = (twenty[0], twenty[19]);
    log(
        &home,
        1,
        &[
            built.set(first, (7, 100, 7), blocks::GLASS),
            built.set(HOME, (1, 100, 1), blocks::STONE),
        ],
    );
    save(&home, first, &built.chunk(first));
    save(&home, HOME, &built.chunk(HOME));
    let back: Vec<ChunkPos> = twenty[..19].iter().copied().chain([HOME]).collect();
    give_back(&home, &back);
    home.flush();
    for position in &twenty[..19] {
        home.request(StoreRequest::Load {
            position: *position,
        });
        let nobodys = StoreReply::NotHeld {
            position: *position,
            holder: None,
        };
        assert_eq!(reply(&home), nobodys);
    }
    assert_eq!(load(&home, HOME), built.chunk(HOME));
    assert_eq!(load(&home, kept), generator().generate(kept));
    assert_eq!(
        store.regions().unwrap().regions[0].bounds,
        around(&[HOME, kept])
    );
    home.flush();
    let mut held = vec![(HOME, 0), (kept, 0)];
    held.sort();
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        assert_eq!(held_after(&disk, survival, epoch), held, "{survival:?}");
    }

    // Claimed again, the chunk is as it was given back, and the region's from the
    // tick of its last commit on.
    assert_eq!(claim(&home, &[first]).0, [first]);
    assert_eq!(load(&home, first), built.chunk(first));
    home.flush();
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let held = held_after(&disk, survival, epoch);
        assert!(held.contains(&(first, 1)), "{survival:?}: {held:?}");
    }
}

/// The life of a world without pins, as far as the store sees it: the home region
/// claims, commits, saves, gives back and makes a checkpoint; a part is split off it,
/// grows by a claim of its own, commits and makes a checkpoint; and the home region
/// absorbs it again. Every answer is looked at. Returns what was built.
fn life(store: &Store) -> Built {
    let told = open_world();
    let (home, restored) = store.open_region(hello_of(&told, 0, 1)).unwrap();
    assert_eq!(restored.entity_ids, EntityIds::block(0).unwrap());
    let free = [NEAR, EAST, EAST_TOO, LEFT];
    assert_eq!(claim(&home, &free).0, free);
    let mut built = Built::default();
    log(
        &home,
        1,
        &[
            built.set(HOME, (1, 100, 1), blocks::STONE),
            built.set(EAST, (2, 100, 2), blocks::GLASS),
            built.set(LEFT, (3, -61, 4), blocks::AIR),
        ],
    );
    for position in [HOME, EAST, LEFT] {
        save(&home, position, &built.chunk(position));
    }
    give_back(&home, &[LEFT]);

    // A split wants a checkpoint that covers every commit, and the home chunk never
    // leaves the home region.
    let declined = |reason| StoreReply::Declined { reason };
    let uncovered = Decline::Uncheckpointed {
        region: RegionId(0),
    };
    assert_eq!(split(&home, 2, &[EAST], 4, 1), declined(uncovered));
    home.request(StoreRequest::Checkpoint {
        tick: 1,
        state: whole("home", 1),
    });
    home.flush();
    assert_eq!(
        split(&home, 2, &[EAST, HOME], 4, 1),
        declined(Decline::Home)
    );
    // The first part of such a world is region 1.
    let made = StoreReply::Split {
        region: RegionId(1),
    };
    assert_eq!(split(&home, 2, &[EAST, EAST_TOO], 4, 1), made);
    let list = store.regions().unwrap();
    assert_eq!((list.home, list.next), (RegionId(0), RegionId(2)));
    assert_eq!(list.regions[0].bounds, around(&[HOME, NEAR]));
    let part = RegionInfo {
        region: RegionId(1),
        epoch: 4,
        bounds: around(&[EAST, EAST_TOO]),
        pinned: Vec::new(),
    };
    assert_eq!(list.regions[1], part);

    // The part has no entity ids, and is pinned to nothing either.
    let (part, restored) = store.open_region(hello_of(&told, 1, 4)).unwrap();
    assert_eq!(restored.entity_ids, NO_ENTITY_IDS);
    assert_eq!(state_of(&restored), Some((2, whole("part", 2))));
    assert_eq!(restored.held, [(EAST, 2), (EAST_TOO, 2)]);
    assert_eq!(restored.pinned, []);
    assert_eq!(load(&part, EAST), built.chunk(EAST));
    // It grows as the home region does: what is free is granted to whoever asks
    // first, and what the other holds is the other's.
    assert_eq!(
        claim(&part, &[GROWN, LEFT, NEAR, HOME]),
        (
            vec![GROWN, LEFT],
            vec![(NEAR, RegionId(0)), (HOME, RegionId(0))]
        )
    );
    assert_eq!(
        claim(&home, &[EAST, LEFT]),
        (Vec::new(), vec![(EAST, RegionId(1)), (LEFT, RegionId(1))])
    );
    assert_eq!(load(&part, LEFT), built.chunk(LEFT));
    log(
        &part,
        3,
        &[
            built.set(EAST, (4, 100, 4), blocks::STONE),
            built.set(GROWN, (5, 100, 5), blocks::GLASS),
        ],
    );
    log(&home, 3, &[built.set(NEAR, (6, 100, 6), blocks::GLASS)]);

    // The home region is never absorbed, and a merge wants checkpoints as well.
    assert_eq!(absorb(&part, 0, 1, 4), declined(Decline::Home));
    let uncovered = Decline::Uncheckpointed {
        region: RegionId(0),
    };
    assert_eq!(absorb(&home, 1, 4, 4), declined(uncovered));
    for (handle, name, chunks) in [
        (&part, "part", [EAST, GROWN]),
        (&home, "home", [NEAR, HOME]),
    ] {
        for position in chunks {
            save(handle, position, &built.chunk(position));
        }
        handle.request(StoreRequest::Checkpoint {
            tick: 3,
            state: whole(name, 3),
        });
        handle.flush();
    }
    let merged = StoreReply::Absorbed {
        absorbed: RegionId(1),
        chunks: vec![LEFT, GROWN, EAST, EAST_TOO],
        pinned: Vec::new(),
    };
    assert_eq!(absorb(&home, 1, 4, 4), merged);
    assert!(part.is_lost());
    for position in [EAST, GROWN, LEFT] {
        assert_eq!(load(&home, position), built.chunk(position));
    }
    home.flush();
    built
}

/// The world [`life`] leaves, as a store says that is started on it: the home region
/// alone again, with everything the part held and with the state of the merge.
fn as_after_a_life(store: &Store, built: &Built, epoch: u64, case: &str) {
    let told = open_world();
    let (list, regions) = opened(store, &told, epoch, case);
    let expected = RegionList {
        home: RegionId(0),
        regions: vec![RegionInfo {
            region: RegionId(0),
            epoch: 0,
            bounds: around(&[HOME, NEAR, EAST, EAST_TOO, LEFT, GROWN]),
            pinned: Vec::new(),
        }],
        absorbed: vec![(RegionId(1), RegionId(0))],
        next: RegionId(2),
    };
    assert_eq!(list, expected, "{case}");
    let restored = &regions[0].1;
    assert_eq!(restored.entity_ids, EntityIds::block(0).unwrap(), "{case}");
    assert_eq!(state_of(restored), Some((4, whole("merged", 4))), "{case}");
    assert_eq!(restored.deltas, [], "{case}");
    // Its own from when it was granted them, and the part's from the merge on.
    let held = [
        (LEFT, 4),
        (GROWN, 4),
        (HOME, 0),
        (NEAR, 0),
        (EAST, 4),
        (EAST_TOO, 4),
    ];
    assert_eq!(restored.held, held, "{case}");
    assert_eq!(restored.pinned, [], "{case}");
    let gone = store.open_region(hello_of(&told, 1, epoch));
    assert!(
        matches!(
            gone,
            Err(StoreError::Absorbed {
                into: RegionId(0),
                ..
            })
        ),
        "{case}: {:?}",
        gone.err()
    );
    as_built(&list, &told, &regions, built, case);
}

#[test]
fn a_world_without_pins_lives_through_claims_a_split_and_a_merge() {
    // In memory.
    let store = Store::memory_divided(generator(), open_world()).unwrap();
    let built = life(&store);
    as_after_a_life(&store, &built, 2, "in memory");

    // In a directory, where the next store finds it.
    let directory = tempfile::tempdir().unwrap();
    let local = || Store::local_divided(directory.path(), generator(), open_world()).unwrap();
    let built = life(&local());
    as_after_a_life(&local(), &built, 2, "in a directory");
    as_after_a_life(&local(), &built, 3, "in a directory, again");
    let files: Vec<String> = std::fs::read_dir(directory.path().join("regions"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        !files.iter().any(|name| name.starts_with("1.")),
        "{files:?}"
    );

    // And on a disk that crashes, with a store killed at every point of its start.
    let disk = Arc::new(MemoryDisk::default());
    let built = life(&store_on(&disk, &open_world()).unwrap());
    for (epoch, survival) in (2..).zip(SURVIVALS) {
        let left = Arc::new(disk.crashed(survival));
        let store = started(&left, &open_world(), false, &format!("{survival:?}"));
        as_after_a_life(&store, &built, epoch, &format!("{survival:?}"));
        // The part's id is not used again.
        let home = open(&store, hello_of(&open_world(), 0, epoch + 1));
        let made = StoreReply::Split {
            region: RegionId(2),
        };
        assert_eq!(split(&home, 9, &[EAST], 1, 2), made, "{survival:?}");
    }
    started_at_every_kill_point(&disk, &open_world(), |store, left, epoch, case| {
        as_after_a_life(store, &built, epoch, case);
        let files = region_files(left);
        assert!(
            !files.iter().any(|name| name.starts_with("1.")),
            "{files:?}"
        );
    });
}

/// Another process reads the list of such a world and opens its regions as those of
/// any other.
#[test]
fn a_world_without_pins_is_served_over_a_connection() {
    let told = open_world();
    let store = Store::memory_divided(generator(), told.clone()).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let server = serve(store.clone(), listener).unwrap();
    let address = server.local_addr().to_string();
    assert_eq!(regions(&address).unwrap(), alone(HOME, 1));

    let (home, restored) = StoreHandle::connect(&address, hello_of(&told, 0, 1)).unwrap();
    assert_eq!(restored.entity_ids, EntityIds::block(0).unwrap());
    assert_eq!(
        (restored.held, restored.pinned),
        (vec![(HOME, 0)], Vec::new())
    );
    assert_eq!(claim(&home, &[NEAR, EAST]).0, [NEAR, EAST]);
    assert_eq!(load(&home, EAST), generator().generate(EAST));
    crate::regions::checkpoint(&home, 1);
    let made = StoreReply::Split {
        region: RegionId(1),
    };
    assert_eq!(split(&home, 2, &[EAST], 3, 1), made);
    let list = regions(&address).unwrap();
    assert_eq!(list, store.regions().unwrap());
    assert_eq!((list.home, list.next), (RegionId(0), RegionId(2)));
    let (_, restored) = StoreHandle::connect(&address, hello_of(&told, 1, 3)).unwrap();
    assert_eq!(restored.entity_ids, NO_ENTITY_IDS);
    assert_eq!(
        (restored.held, restored.pinned),
        (vec![(EAST, 2)], Vec::new())
    );
    let refused = StoreHandle::connect(&address, hello_of(&told, 2, 1));
    let reason = StoreError::UnknownRegion {
        region: RegionId(2),
    };
    assert!(
        matches!(&refused, Err(StoreError::Refused(given)) if *given == reason.to_string()),
        "{:?}",
        refused.err()
    );
}

// Section 2.2: worlds from before.

/// The chunks of [`without_pins_lived_in`]: one beside the home chunk, two east of
/// x = 4, which go with the part, one that is given back, and one the part claims.
const NEAR: ChunkPos = ChunkPos::new(3, -3);
const EAST: ChunkPos = ChunkPos::new(4, -3);
const EAST_TOO: ChunkPos = ChunkPos::new(5, -3);
const LEFT: ChunkPos = ChunkPos::new(-1, 0);
const GROWN: ChunkPos = ChunkPos::new(1, 7);

/// A world without pins that was lived in: the home region has a checkpoint, was
/// split, and has a commit in the log after that; the part that was split off it has
/// claimed a chunk, has a state file of its own and a commit after it; and a chunk
/// that something was built in was given back, and is nobody's. Returns what was
/// built.
fn without_pins_lived_in(disk: &Arc<MemoryDisk>) -> Built {
    let told = open_world();
    let store = store_on(disk, &told).unwrap();
    let home = open(&store, hello_of(&told, 0, 3));
    let free = [NEAR, EAST, EAST_TOO, LEFT];
    assert_eq!(claim(&home, &free).0, free);
    let mut built = Built::default();
    log(
        &home,
        1,
        &[
            built.set(HOME, (1, 100, 1), blocks::STONE),
            built.set(NEAR, (3, -61, 4), blocks::AIR),
            built.set(LEFT, (2, 100, 2), blocks::GLASS),
            built.set(EAST, (0, 100, 0), blocks::STONE),
        ],
    );
    for position in [HOME, NEAR, LEFT, EAST] {
        save(&home, position, &built.chunk(position));
    }
    give_back(&home, &[LEFT]);
    home.request(StoreRequest::Checkpoint {
        tick: 1,
        state: whole("home", 1),
    });
    home.flush();
    let made = StoreReply::Split {
        region: RegionId(1),
    };
    assert_eq!(split(&home, 2, &[EAST, EAST_TOO], 5, 1), made);

    let part = open(&store, hello_of(&told, 1, 5));
    assert_eq!(claim(&part, &[GROWN]).0, [GROWN]);
    log(
        &part,
        3,
        &[
            built.set(EAST, (4, 100, 4), blocks::GLASS),
            built.set(GROWN, (5, 100, 5), blocks::STONE),
        ],
    );
    save(&part, EAST, &built.chunk(EAST));
    save(&part, GROWN, &built.chunk(GROWN));
    part.request(StoreRequest::Checkpoint {
        tick: 3,
        state: whole("part", 3),
    });
    // What follows is in the log alone, of either region.
    log(
        &part,
        4,
        &[
            built.set(EAST_TOO, (6, 100, 6), blocks::GLASS),
            built.set(GROWN, (7, -61, 7), blocks::AIR),
        ],
    );
    log(
        &home,
        3,
        &[
            built.set(NEAR, (3, 100, 4), blocks::GLASS),
            built.set(HOME, (2, -61, 2), blocks::AIR),
        ],
    );
    home.flush();
    part.flush();
    built
}

/// The world of [`without_pins_lived_in`] as a store finds it that is told no pins
/// and the same home chunk.
fn as_without_pins_lived_in(store: &Store, built: &Built, epoch: u64, case: &str) {
    let told = open_world();
    let (list, regions) = opened(store, &told, epoch, case);
    let region = |region: u32, held: &[ChunkPos]| RegionInfo {
        region: RegionId(region),
        epoch: 0,
        bounds: around(held),
        pinned: Vec::new(),
    };
    let expected = RegionList {
        home: RegionId(0),
        regions: vec![
            region(0, &[HOME, NEAR]),
            region(1, &[EAST, EAST_TOO, GROWN]),
        ],
        absorbed: Vec::new(),
        next: RegionId(2),
    };
    assert_eq!(list, expected, "{case}");
    let ticks = |restored: &Restored| -> Vec<u64> {
        restored.deltas.iter().map(|delta| delta.tick).collect()
    };
    let (home, part) = (&regions[0].1, &regions[1].1);
    assert_eq!(home.entity_ids, EntityIds::block(0).unwrap(), "{case}");
    assert_eq!(state_of(home), Some((2, whole("rest", 2))), "{case}");
    assert_eq!(ticks(home), [3], "{case}");
    assert_eq!(home.held, [(HOME, 0), (NEAR, 0)], "{case}");
    assert_eq!(part.entity_ids, NO_ENTITY_IDS, "{case}");
    assert_eq!(state_of(part), Some((3, whole("part", 3))), "{case}");
    assert_eq!(ticks(part), [4], "{case}");
    assert_eq!(part.held, [(GROWN, 2), (EAST, 2), (EAST_TOO, 2)], "{case}");
    assert_eq!((&home.pinned, &part.pinned), (&vec![], &vec![]), "{case}");
    as_built(&list, &told, &regions, built, case);
}

/// The first row of the table of section 2.2, for a world without pins.
#[test]
fn a_world_without_pins_started_without_pins_is_found_as_it_was() {
    let disk = Arc::new(MemoryDisk::default());
    let built = without_pins_lived_in(&disk);
    let table = disk.read(Path::new("/world/regions/table")).unwrap();
    started_at_every_kill_point(&disk, &open_world(), |store, left, epoch, case| {
        // The table is not written again.
        let found = left.read(Path::new("/world/regions/table")).unwrap();
        assert_eq!(found, table, "{case}");
        as_without_pins_lived_in(store, &built, epoch, case);
    });
}

/// The chunks of [`side_by_side_lived_in`] with a cut at `cut`: one of the western
/// region, one of the eastern, and two more of the eastern that go with its part.
fn of_the_west(cut: i32) -> ChunkPos {
    ChunkPos::new(cut - 2, 0)
}

fn of_the_east(cut: i32) -> ChunkPos {
    ChunkPos::new(cut + 5, 1)
}

fn of_the_part(cut: i32) -> [ChunkPos; 2] {
    [ChunkPos::new(cut + 6, 1), ChunkPos::new(cut + 6, 2)]
}

/// A world of two regions side by side that was lived in, made by a store that is
/// started with `told`, whose regions are cut at `cut`: the western region has a
/// commit in the log; the eastern one a saved chunk, a checkpoint, a part split off
/// it, and a commit after that; and the part holds two chunks, has built in both, and
/// has a state file of its own and a commit after it. Returns what was built.
fn side_by_side_lived_in(disk: &Arc<MemoryDisk>, told: &Division, cut: i32) -> Built {
    let store = store_on(disk, told).unwrap();
    let west = open(&store, hello_of(told, 0, 3));
    let east = open(&store, hello_of(told, 1, 4));
    let (western, eastern, [first, second]) =
        (of_the_west(cut), of_the_east(cut), of_the_part(cut));
    let mut built = Built::default();
    log(&west, 1, &[built.set(western, (13, -61, 4), blocks::AIR)]);
    log(
        &east,
        1,
        &[
            built.set(eastern, (3, -61, 4), blocks::AIR),
            built.set(first, (1, 100, 1), blocks::STONE),
        ],
    );
    save(&east, eastern, &built.chunk(eastern));
    save(&east, first, &built.chunk(first));
    east.request(StoreRequest::Checkpoint {
        tick: 1,
        state: whole("east", 1),
    });
    east.flush();
    let made = StoreReply::Split {
        region: RegionId(2),
    };
    assert_eq!(split(&east, 2, &[first, second], 6, 2), made);

    let part = open(&store, hello_of(told, 2, 6));
    log(&part, 3, &[built.set(first, (4, 100, 4), blocks::GLASS)]);
    save(&part, first, &built.chunk(first));
    part.request(StoreRequest::Checkpoint {
        tick: 3,
        state: whole("part", 3),
    });
    // What follows is in the log alone.
    log(
        &part,
        4,
        &[
            built.set(second, (5, 100, 5), blocks::GLASS),
            built.set(first, (6, 100, 6), blocks::STONE),
        ],
    );
    log(&east, 3, &[built.set(eastern, (3, 100, 4), blocks::GLASS)]);
    west.flush();
    east.flush();
    part.flush();
    built
}

/// The world of [`side_by_side_lived_in`] as a store finds it that is started with
/// the same areas and the same home chunk.
fn as_side_by_side_lived_in(
    store: &Store,
    told: &Division,
    cut: i32,
    built: &Built,
    epoch: u64,
    case: &str,
) {
    let (list, regions) = opened(store, told, epoch, case);
    let mut expected = table::Table::made_from(told, 3, 1).list(|_| 0);
    expected.regions.push(RegionInfo {
        region: RegionId(2),
        epoch: 0,
        bounds: around(&of_the_part(cut)),
        pinned: Vec::new(),
    });
    assert_eq!(list, expected, "{case}");
    let ticks = |restored: &Restored| -> Vec<u64> {
        restored.deltas.iter().map(|delta| delta.tick).collect()
    };
    let [west, east, part] = [0, 1, 2].map(|region: usize| &regions[region].1);
    assert_eq!((state_of(west), ticks(west)), (None, vec![1]), "{case}");
    assert_eq!(state_of(east), Some((2, whole("rest", 2))), "{case}");
    assert_eq!(ticks(east), [3], "{case}");
    assert_eq!(state_of(part), Some((3, whole("part", 3))), "{case}");
    assert_eq!(ticks(part), [4], "{case}");
    assert_eq!(
        part.held,
        of_the_part(cut).map(|chunk| (chunk, 2)),
        "{case}"
    );
    // The stripes keep the entity ids they were issued, and the part has none.
    let blocks = [0, 1].map(|block| EntityIds::block(block).unwrap());
    assert_eq!([west.entity_ids, east.entity_ids], blocks, "{case}");
    assert_eq!(part.entity_ids, NO_ENTITY_IDS, "{case}");
    as_built(&list, told, &regions, built, case);
}

/// T7: a world that was served with `--boundaries 4` and is started with `--pin 4` is
/// found as it was, regions and states and all; and the other way round, for as long
/// as there are both.
#[test]
fn a_world_of_stripes_started_with_its_boundaries_as_pins_is_found_as_it_was() {
    for cut in [4, 0] {
        let disk = Arc::new(MemoryDisk::default());
        let built = side_by_side_lived_in(&disk, &stripes(&[cut]), cut);
        let table = disk.read(Path::new("/world/regions/table")).unwrap();
        let pins = pinned_at(&[cut]);
        started_at_every_kill_point(&disk, &pins, |store, left, epoch, case| {
            // The table is not written again.
            let found = left.read(Path::new("/world/regions/table")).unwrap();
            assert_eq!(found, table, "{case}");
            as_side_by_side_lived_in(store, &pins, cut, &built, epoch, case);
        });
    }

    let disk = Arc::new(MemoryDisk::default());
    let built = side_by_side_lived_in(&disk, &pinned_at(&[4]), 4);
    let table = disk.read(Path::new("/world/regions/table")).unwrap();
    let left = Arc::new(disk.crashed(Survival::Nothing));
    let store = started(&left, &stripes(&[4]), false, "pins, then stripes");
    assert_eq!(left.read(Path::new("/world/regions/table")).unwrap(), table);
    as_side_by_side_lived_in(&store, &stripes(&[4]), 4, &built, 10, "pins, then stripes");
}

/// Starts a store for `told` on what is durable of `world`, which has to make the
/// world over and to say so, after which the world is as `check` says; and one more on
/// what that one left, which has to find the same and to say nothing. With `killed`,
/// the same of a store that dies at any point of making the world over.
fn made_over(
    world: &MemoryDisk,
    told: &Division,
    killed: bool,
    check: impl Fn(&Store, &MemoryDisk, u64, &str),
) {
    let left = Arc::new(world.crashed(Survival::Nothing));
    let store = started(&left, told, true, "at once");
    check(&store, &left, 100, "at once");
    assert_eq!(left.read(Path::new("/world/layout")).unwrap(), None);
    drop(store);
    let again = Arc::new(left.crashed(Survival::Nothing));
    let store = started(&again, told, false, "again");
    check(&store, &again, 101, "again");
    if killed {
        started_at_every_kill_point(world, told, check);
    }
}

/// T3 and T9: a world of two stripes that is started without pins is one home region
/// afterwards, region 0, with everything that was built in the stored chunks; the
/// stripes' states, logs and grants are gone, and the part with them.
#[test]
fn a_world_of_stripes_started_without_pins_is_made_over_into_one_home_region() {
    // With the home chunk in the western stripe, and in the eastern one, whose id
    // the home region then does not keep.
    for cut in [4, 0] {
        let disk = Arc::new(MemoryDisk::default());
        let built = side_by_side_lived_in(&disk, &stripes(&[cut]), cut);
        let told = open_world();

        let left = Arc::new(disk.crashed(Survival::Nothing));
        let store = started(&left, &told, true, "at once");
        // The list is that of a new world, but for the ids that were used before.
        assert_eq!(listed(&store.regions().unwrap()), alone(HOME, 3));
        // The regions that are no more are refused as regions that never were.
        for region in [1, 2] {
            let refused = store.open_region(hello_of(&told, region, 100));
            assert!(
                matches!(refused, Err(StoreError::UnknownRegion { .. })),
                "{:?}",
                refused.err()
            );
        }
        // Region 0 has the epoch the western stripe was last opened with: a hello
        // with a lower one is refused, and one with that epoch restores a region
        // without a state.
        assert_eq!(store.regions().unwrap().regions[0].epoch, 3);
        assert!(matches!(
            store.open_region(hello_of(&told, 0, 2)),
            Err(StoreError::EpochRefused { seen: 3, .. })
        ));
        let (_, restored) = store.open_region(hello_of(&told, 0, 3)).unwrap();
        assert_eq!((restored.state, restored.deltas), (None, Vec::new()));
        assert_eq!(restored.held, [(HOME, 0)]);

        made_over(&disk, &told, cut == 0, |store, left, epoch, case| {
            let regions = as_made_over(store, &told, 3, &built, epoch, case);
            // And the entity ids it was issued.
            let issued = EntityIds::block(0).unwrap();
            assert_eq!(regions[0].entity_ids, issued, "{case}");
            // No state is left, and of the regions that are gone only the file of the
            // one that has entity ids, which are not to be issued again.
            let files = region_files(left);
            assert_eq!(files, ["0.region", "1.region", "table"], "{case}");
        });
    }
}

/// T4: the other way round. The part that was region 1 is gone as the stripes were,
/// and region 1 is the eastern of two pinned regions.
#[test]
fn a_world_without_pins_started_with_pins_is_made_over_into_pinned_regions() {
    let disk = Arc::new(MemoryDisk::default());
    let built = without_pins_lived_in(&disk);
    let two = pinned_at(&[4]);
    for told in [two.clone(), pinned_at(&[]), pinned_at(&[-8, 2, 5])] {
        let pinned = told.pinned.len() as u32;
        let left = Arc::new(disk.crashed(Survival::Nothing));
        let store = started(&left, &told, true, "at once");
        let list = store.regions().unwrap();
        let home = told.pinned.iter().position(|area| area.contains(HOME));
        assert_eq!(list.home, RegionId(home.unwrap() as u32));
        assert_eq!(list.next, RegionId(pinned.max(2)));
        assert_eq!(list.absorbed, []);

        made_over(&disk, &told, told == two, |store, left, epoch, case| {
            let regions = as_made_over(store, &told, 2, &built, epoch, case);
            // Region 0 keeps the entity ids it was issued as the home region, and
            // every other region is issued a block of its own, also the one whose
            // id was the part's, which had none.
            let mut blocks: Vec<EntityIds> =
                regions.iter().map(|region| region.entity_ids).collect();
            assert_eq!(blocks[0], EntityIds::block(0).unwrap(), "{case}");
            blocks.sort_by_key(|block| block.first.0);
            blocks.dedup();
            assert_eq!(blocks.len(), regions.len(), "{case}");
            assert!(!blocks.contains(&NO_ENTITY_IDS), "{case}");
            let files = region_files(left);
            assert!(
                !files.iter().any(|name| name.ends_with(".state")),
                "{files:?}"
            );
        });
    }

    // A store from before this step, which is told the stripes of a layout, makes such
    // a world over into its stripes in the same way: that is the way back.
    let told = stripes(&[4]);
    made_over(&disk, &told, false, |store, _, epoch, case| {
        as_made_over(store, &told, 2, &built, epoch, case);
    });

    // A world whose part was absorbed again: nothing is remembered as absorbed, and
    // the id that was is that of a region again.
    let disk = Arc::new(MemoryDisk::default());
    let built = life(&store_on(&disk, &open_world()).unwrap());
    made_over(&disk, &two, false, |store, _, epoch, case| {
        as_made_over(store, &two, 2, &built, epoch, case);
        assert_eq!(store.regions().unwrap().absorbed, [], "{case}");
    });
}

/// Another home chunk is another world as well: the second row of the table of
/// section 2.2 for a world that has no pins before or after.
#[test]
fn a_world_without_pins_started_with_another_home_chunk_is_made_over() {
    let disk = Arc::new(MemoryDisk::default());
    let built = without_pins_lived_in(&disk);
    // A chunk that nobody held, one of the part's, and one that the part had claimed.
    for home in [ORIGIN, EAST, GROWN] {
        let told = Division::open(home);
        made_over(&disk, &told, home == ORIGIN, |store, left, epoch, case| {
            if epoch == 100 {
                assert_eq!(listed(&store.regions().unwrap()), alone(home, 2), "{case}");
            }
            let regions = as_made_over(store, &told, 2, &built, epoch, case);
            let issued = EntityIds::block(0).unwrap();
            assert_eq!(regions[0].entity_ids, issued, "{case}");
            assert_eq!(region_files(left), ["0.region", "table"], "{case}");
        });
    }
}

/// What the regions of [`world_of_today`] built: the western one dug west of the
/// origin, and the eastern one dug and built at the origin.
fn built_today() -> Built {
    let mut built = Built::default();
    built.set(ChunkPos::new(-1, 0), (13, -61, 4), blocks::AIR);
    built.set(ORIGIN, (3, -61, 4), blocks::AIR);
    built.set(ORIGIN, (3, 100, 4), blocks::GLASS);
    built
}

/// T5: a world from before there was a table is made over by a store that is told no
/// fingerprint, also with the boundaries it had as pins, and the file `layout` is gone
/// when the table is durable.
#[test]
fn a_world_from_before_there_was_a_table_is_made_over_whatever_its_layout_was() {
    let world = world_of_today();
    let built = built_today();
    let same_cut = Division::side_by_side(ORIGIN, &[0]).unwrap();
    let killed = [open_world(), same_cut.clone()];
    for told in [
        open_world(),
        same_cut,
        Division::open(ORIGIN),
        pinned_at(&[4]),
    ] {
        // Such a world has no table whose next region id could count.
        let dies = killed.contains(&told);
        made_over(&world, &told, dies, |store, left, epoch, case| {
            let regions = as_made_over(store, &told, 0, &built, epoch, case);
            // The ids of the stripes are those of the new regions, with their epochs
            // and entity ids.
            let issued = EntityIds::block(0).unwrap();
            assert_eq!(regions[0].entity_ids, issued, "{case}");
            let files = region_files(left);
            assert!(
                !files.iter().any(|name| name.ends_with(".state")),
                "{files:?}"
            );
        });
    }
}

/// The file says nothing that such a store reads. Told a fingerprint, as a store of
/// stripes still is, it holds the file to being one, as before.
#[test]
fn a_layout_file_is_not_read_by_a_store_that_is_told_no_fingerprint() {
    let built = built_today();
    for says in [&b"anything at all\n"[..], b"", b"00000000000000zz"] {
        let world = world_of_today();
        put(&world, "/world/layout", says);
        assert!(matches!(
            store_on(&Arc::new(world.crashed(Survival::Nothing)), &stripes(&[0])),
            Err(StoreError::MalformedMeta(_))
        ));
        for told in [open_world(), pinned_at(&[0])] {
            let left = Arc::new(world.crashed(Survival::Nothing));
            let store = started(&left, &told, true, "at once");
            as_made_over(&store, &told, 0, &built, 10, "at once");
            assert_eq!(left.read(Path::new("/world/layout")).unwrap(), None);
        }
    }
    let world = world_of_today();
    put(&world, "/world/layout", b"anything at all\n");
    let told = open_world();
    started_at_every_kill_point(&world, &told, |store, _, epoch, case| {
        as_made_over(store, &told, 0, &built, epoch, case);
    });

    // Beside a table the file means nothing either, and goes.
    let disk = Arc::new(MemoryDisk::default());
    let built = without_pins_lived_in(&disk);
    put(&disk, "/world/layout", b"anything at all\n");
    let left = Arc::new(disk.crashed(Survival::Nothing));
    let store = started(&left, &told, false, "beside a table");
    as_without_pins_lived_in(&store, &built, 10, "beside a table");
    assert_eq!(left.read(Path::new("/world/layout")).unwrap(), None);
}

/// A world that cannot be opened says so and is not touched: nothing is put into the
/// stored chunks for a table of another division, and no table is written, before
/// everything the regions have was read.
#[test]
fn a_world_that_cannot_be_read_is_not_made_over() {
    let disk = Arc::new(MemoryDisk::default());
    side_by_side_lived_in(&disk, &stripes(&[4]), 4);
    let damaged = |name: &str| {
        let path = Path::new("/world").join(name);
        let mut bytes = disk.read(&path).unwrap().expect(name);
        bytes[12] ^= 0x01;
        let damaged = disk.crashed(Survival::Everything);
        put(&damaged, path.to_str().unwrap(), &bytes);
        // Counted from here on.
        (Arc::new(damaged.with(Fault::Fail(u64::MAX))), path)
    };
    for name in ["regions/table", "regions/1.region", "regions/2.state"] {
        for told in [open_world(), pinned_at(&[0]), stripes(&[4])] {
            let (left, path) = damaged(name);
            let (refused, lines) = logged(|| store_on(&left, &told));
            assert!(
                matches!(&refused, Err(StoreError::Damaged { path: at, .. }) if *at == path),
                "{name}: {:?}",
                refused.err()
            );
            assert!(!says_made_over(&lines, name));
            assert_eq!(left.operations(), 0, "{name}");
        }
    }
    // A table that can be read and says what cannot be.
    let mut file = table_file(&disk);
    file.home_region = 7;
    let impossible = disk.crashed(Survival::Everything);
    put(&impossible, "/world/regions/table", &file.encode());
    let left = Arc::new(impossible.with(Fault::Fail(u64::MAX)));
    assert!(matches!(
        store_on(&left, &open_world()),
        Err(StoreError::Table(_))
    ));
    assert_eq!(left.operations(), 0);
}

/// T9, where there is nothing to put into the chunks: the store says what it says of
/// a world it makes over whenever its regions begin anew, since that is what whoever
/// runs one of them has to know.
#[test]
fn a_world_whose_regions_had_nothing_says_that_they_begin_anew_all_the_same() {
    // A world of stripes that nobody ever opened a region of.
    let disk = Arc::new(MemoryDisk::default());
    drop(started(&disk, &stripes(&[4]), false, "a new world"));
    let left = Arc::new(disk.crashed(Survival::Nothing));
    let store = started(&left, &open_world(), true, "never opened");
    assert_eq!(store.regions().unwrap(), alone(HOME, 2));

    // One whose regions were opened and did nothing. Their owners are those who find
    // their regions gone.
    let disk = Arc::new(MemoryDisk::default());
    let store = started(&disk, &open_world(), false, "a new world");
    open(&store, hello_of(&open_world(), 0, 7)).flush();
    let left = Arc::new(disk.crashed(Survival::Nothing));
    let store = started(&left, &pinned_at(&[4]), true, "opened and no more");
    assert_eq!(store.regions().unwrap().regions[0].epoch, 7);
    // And nothing is said by the start after it, or by one with the same areas told
    // as stripes.
    for told in [pinned_at(&[4]), stripes(&[4])] {
        let again = Arc::new(left.crashed(Survival::Nothing));
        drop(started(&again, &told, false, "again"));
    }
}

/// The same in a directory of the file system, where the next store finds what the
/// one before left.
#[test]
fn a_world_in_a_directory_is_made_over_for_other_pins() {
    let directory = tempfile::tempdir().unwrap();
    let start = |told: &Division, made_over: bool, case: &str| {
        let (store, lines) =
            logged(|| Store::local_divided(directory.path(), generator(), told.clone()));
        assert_eq!(says_made_over(&lines, case), made_over, "{case}");
        store.unwrap_or_else(|error| panic!("{case}: {error}"))
    };
    let mut built = Built::default();
    let (western, eastern) = (ChunkPos::new(-6, 1), ChunkPos::new(9, 1));
    {
        let told = pinned_at(&[4]);
        let store = start(&told, false, "a new world");
        let west = open(&store, hello_of(&told, 0, 3));
        let east = open(&store, hello_of(&told, 1, 4));
        log(&west, 1, &[built.set(western, (1, 100, 1), blocks::GLASS)]);
        log(&east, 1, &[built.set(eastern, (2, -61, 2), blocks::AIR)]);
        save(&east, eastern, &built.chunk(eastern));
        east.request(StoreRequest::Checkpoint {
            tick: 1,
            state: whole("east", 1),
        });
        log(&east, 2, &[built.set(eastern, (3, 100, 3), blocks::STONE)]);
        west.flush();
        east.flush();
    }
    // The same pins: as it was.
    {
        let told = pinned_at(&[4]);
        let store = start(&told, false, "the same pins");
        let (_, restored) = store.open_region(hello_of(&told, 1, 5)).unwrap();
        assert_eq!(state_of(&restored), Some((1, whole("east", 1))));
    }
    // No pins, and then pins again: made over each time, and found as it is after.
    for (told, epoch) in [(open_world(), 10), (pinned_at(&[4]), 12)] {
        let store = start(&told, true, "other pins");
        as_made_over(&store, &told, 2, &built, epoch, "other pins");
        drop(store);
        let store = start(&told, false, "the same pins again");
        as_made_over(&store, &told, 2, &built, epoch + 1, "the same pins again");
        let state = directory.path().join("regions/1.state");
        assert!(!state.exists());
    }
}
