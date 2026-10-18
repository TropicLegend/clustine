//! What phase E costs (`docs/adr/0025-effects-between-regions.md`, step E7): two
//! regions side by side on a world store on the real disk, each ticked by a worker of
//! its own in real time, with a viewer either far from the line between them or at it.
//! At the line the rims of the views tick on both sides, the store couples the two, and
//! a river runs across: a row of water sources along the eastern edge of region 0,
//! whose ticks fall in the middle of what is measured.
//!
//! Printed for each case: the ticks a second of each region, the gaps between its
//! ticks, what the store asked of the disk, and the time the process spent on the
//! processors. Run it by name in a release build, with `TMPDIR` on the disk to be
//! measured:
//!
//! ```text
//! cargo test --release -p clustine-worker --test border_costs -- --ignored --nocapture
//! ```

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use clustine_data::blocks;
use clustine_region::RegionId;
use clustine_rpc::link::{self, EdgeEnd};
use clustine_rpc::{
    EdgeMessage, EdgeToWorker, RegionHello, StoreReply, StoreRequest, WorkerToEdge,
};
use clustine_sim::api::Inventory;
use clustine_worker::{RegionRunner, RegionSettings, RegionStatus, Worker};
use clustine_world::{BlockPos, ChunkPos, EdgeId, InChunk, ScheduledTick, Ticked};
use clustine_worldgen::FlatGenerator;
use clustine_worldstore::{DiskFigures, Pins, Store, StoreHandle, disk_figures};

/// Where players would enter the world: in the home chunk, at the origin.
const SPAWN_BLOCK: BlockPos = BlockPos::new(13, -60, 8);

/// `clustine_blocks::fluid::WATER`: still water, as the game numbers fluids.
const WATER: u8 = 2;

/// The tick the river's sources first tick at: after the warming up, so that the water
/// crosses while the pace is measured.
const RIVER_AT: i32 = 100;

/// The tick both regions reach before the measuring begins: their chunks are loaded and
/// a coupling has been made by then.
const WARMED_UP: u64 = 60;

/// How long the pace is measured for.
const MEASURED_FOR: Duration = Duration::from_secs(10);

/// How many windows the pair at the line is measured for; `CLUSTINE_MEASURED_WINDOWS`
/// for fewer or more.
static MEASURED_WINDOWS: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| {
    std::env::var("CLUSTINE_MEASURED_WINDOWS").map_or(28, |windows| {
        windows
            .parse()
            .expect("CLUSTINE_MEASURED_WINDOWS is a number")
    })
});

/// What a sync of the owner's network storage takes without the cache in front of it
/// (`docs/groundwork/disk-syncs-measured.md`).
const UNCACHED_SYNC: Duration = Duration::from_millis(22);

fn settings() -> RegionSettings {
    RegionSettings {
        starting_inventory: Inventory::default(),
        return_after: 1_000,
        lowest_y: -64,
        rings: true,
    }
}

fn hello(region: u32, epoch: u64) -> RegionHello {
    RegionHello {
        region: RegionId(region),
        epoch,
    }
}

/// Waits for the store's answer to a load, passing over the rest.
fn loaded(handle: &StoreHandle, position: ChunkPos) -> clustine_world::Chunk {
    let asked = Instant::now();
    while asked.elapsed() < Duration::from_secs(60) {
        match handle.try_reply() {
            Some(StoreReply::Loaded {
                position: at,
                chunk,
            }) if at == position => return chunk,
            Some(_) => {}
            None => thread::sleep(Duration::from_millis(1)),
        }
    }
    panic!("the store did not hand out {position:?}");
}

/// The world in `root`, with a source on every block of the eastern edge of region 0
/// for three chunks along the line.
fn world(root: &Path, slowed: Option<Duration>) -> Store {
    let generator = Arc::new(FlatGenerator::classic().with_spawn(SPAWN_BLOCK));
    let pins = Pins::side_by_side(&[1]).expect("one cut ascends");
    let store = match slowed {
        Some(extra) => Store::local_divided_slowed(root, generator, pins, extra),
        None => Store::local_divided(root, generator, pins),
    }
    .expect("the world is made");
    let (handle, _) = store.open_region(hello(0, 1)).expect("a new region opens");
    for z in -1..=1 {
        let position = ChunkPos::new(0, z);
        handle.request(StoreRequest::Load { position });
        let mut chunk = loaded(&handle, position);
        let mut ticks = Vec::new();
        for along in 0..16 {
            chunk.set(15, -60, along, blocks::WATER);
            ticks.push(ScheduledTick {
                at: InChunk::new(15, -60, along),
                of: Ticked::Fluid(WATER),
                delay: RIVER_AT,
                priority: 0,
            });
        }
        chunk.set_ticks(ticks);
        handle.request(StoreRequest::Save {
            position,
            tick: 0,
            chunk,
            parts: clustine_rpc::Parts::ALL,
        });
    }
    handle.flush();
    store
}

/// A worker for `region`, with its status and a link whose edge views `chunks`.
fn viewed(
    store: &Store,
    region: u32,
    epoch: u64,
    chunks: Vec<ChunkPos>,
) -> (Worker, Arc<RegionStatus>, EdgeEnd) {
    let (handle, restored) = store
        .open_region(hello(region, epoch))
        .expect("the region opens");
    let runner = RegionRunner::restore(settings(), handle, restored.clone())
        .expect("what the store has is readable")
        .with_id(RegionId(region))
        .with_borders(&restored);
    let (edge, worker) = link::in_process::<EdgeMessage, WorkerToEdge>(8192);
    runner.links().attach(worker);
    edge.try_send(EdgeMessage::unnumbered(EdgeToWorker::Hello {
        edge: EdgeId(10 + u64::from(region)),
        start: 1,
        since: 0,
        seen: 0,
        players: Vec::new(),
        chunks,
        guests: Vec::new(),
    }))
    .expect("the link has room");
    let status = runner.status();
    (Worker::spawn(runner), status, edge)
}

fn columns(xs: std::ops::RangeInclusive<i32>) -> Vec<ChunkPos> {
    xs.flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// The time this process has spent on the processors, from `/proc/self/stat`, in
/// clock ticks of a hundredth of a second.
fn processor_time() -> u64 {
    let stat = std::fs::read_to_string("/proc/self/stat").expect("Linux says");
    // The name in brackets may hold spaces; the fields after it do not.
    let after = &stat[stat.rfind(')').expect("the name ends") + 2..];
    let fields: Vec<&str> = after.split(' ').collect();
    // utime and stime, the 14th and 15th fields counted from the process id.
    fields[11].parse::<u64>().expect("a number") + fields[12].parse::<u64>().expect("a number")
}

/// Ticks of one region, as seen by sampling its status.
#[derive(Default)]
struct Seen {
    first: u64,
    last: u64,
    changed: Option<Instant>,
    gaps: Vec<Duration>,
}

impl Seen {
    fn sample(&mut self, tick: u64, now: Instant) {
        if let Some(changed) = self.changed {
            if tick != self.last {
                self.gaps.push(now - changed);
                self.changed = Some(now);
            }
        } else {
            self.first = tick;
            self.changed = Some(now);
        }
        self.last = tick;
    }

    fn report(&mut self, name: &str) -> String {
        self.gaps.sort();
        let at = |share: f64| {
            let index = ((self.gaps.len() as f64 - 1.0) * share).round() as usize;
            self.gaps
                .get(index)
                .map_or(0.0, |gap| gap.as_secs_f64() * 1000.0)
        };
        format!(
            "{name} {:.1} ticks/s, gaps p50 {:.0} p99 {:.0} max {:.0} ms",
            (self.last - self.first) as f64 / MEASURED_FOR.as_secs_f64(),
            at(0.5),
            at(0.99),
            at(1.0),
        )
    }
}

fn measure(slowed: Option<Duration>, at_the_line: bool, windows: usize) {
    let directory = tempfile::tempdir().expect("a directory for the world");
    let store = world(directory.path(), slowed);
    let (west_columns, east_columns) = if at_the_line {
        (columns(-1..=0), columns(1..=2))
    } else {
        // The rims, a chunk beyond the views, stay four chunks from the line.
        (columns(-6..=-5), columns(6..=7))
    };
    let case = format!(
        "{}, {}",
        match slowed {
            Some(extra) => format!("syncs {} ms slower", extra.as_millis()),
            None => "the disk as it is".to_owned(),
        },
        if at_the_line {
            "viewers at the line"
        } else {
            "viewers apart"
        }
    );
    let (west, west_status, mut west_edge) = viewed(&store, 0, 2, west_columns.clone());
    let (east, east_status, mut east_edge) = viewed(&store, 1, 1, east_columns.clone());
    let tick = |status: &RegionStatus| status.tick.load(Ordering::Relaxed);
    let drain = |edges: [&mut EdgeEnd; 2]| {
        for edge in edges {
            while let Ok(Some(_)) = edge.try_recv() {}
        }
    };
    let began = Instant::now();
    while tick(&west_status) < WARMED_UP || tick(&east_status) < WARMED_UP {
        assert!(
            began.elapsed() < Duration::from_secs(120),
            "the regions did not warm up"
        );
        drain([&mut west_edge, &mut east_edge]);
        thread::sleep(Duration::from_millis(1));
    }
    let mut couplings = 0;
    for window in 0..windows {
        let (mut west_seen, mut east_seen) = (Seen::default(), Seen::default());
        let disk_before = disk_figures();
        let processor_before = processor_time();
        let measuring = Instant::now();
        while measuring.elapsed() < MEASURED_FOR {
            let now = Instant::now();
            west_seen.sample(tick(&west_status), now);
            east_seen.sample(tick(&east_status), now);
            drain([&mut west_edge, &mut east_edge]);
            thread::sleep(Duration::from_micros(500));
        }
        let disk = since(disk_before, disk_figures());
        let processor = processor_time() - processor_before;
        // Read before the stops, which end the coupling.
        couplings = std::fs::metadata(directory.path().join("regions/couplings"))
            .map_or(0, |file| file.len());
        let seconds = MEASURED_FOR.as_secs_f64();
        println!(
            "MEASURED: {case}, window {window}: {}; {}; the disk {:.0} appends/s \
             ({:.0} kB/s), {:.0} syncs/s; the process {:.0}% of a processor",
            west_seen.report("west"),
            east_seen.report("east"),
            disk.appends as f64 / seconds,
            disk.appended_bytes as f64 / seconds / 1000.0,
            (disk.file_syncs + disk.directory_syncs) as f64 / seconds,
            processor as f64 / seconds,
        );
    }
    let ticks = (tick(&west_status), tick(&east_status));
    // A stop stores what has changed and does not checkpoint, so what the regions
    // committed since they were opened is in the log for the store to read again at a
    // start and hand to the regions when they are opened.
    west.stop();
    east.stop();
    drop(store);
    let started = Instant::now();
    let generator = Arc::new(FlatGenerator::classic().with_spawn(SPAWN_BLOCK));
    let pins = Pins::side_by_side(&[1]).expect("one cut ascends");
    let store = match slowed {
        Some(extra) => Store::local_divided_slowed(directory.path(), generator, pins, extra),
        None => Store::local_divided(directory.path(), generator, pins),
    }
    .expect("the world is there");
    let start = started.elapsed();
    let mut opens = Vec::new();
    for (region, epoch) in [(0, 3), (1, 2)] {
        let opened = Instant::now();
        let (handle, restored) = store
            .open_region(hello(region, epoch))
            .expect("the region opens");
        let runner = RegionRunner::restore(settings(), handle, restored)
            .expect("what the store has is readable");
        opens.push(format!("{:.3}", opened.elapsed().as_secs_f64()));
        drop(runner);
    }
    println!(
        "MEASURED: {case}, after ticks {ticks:?}: couplings on record {couplings} bytes; \
         the store started again in {:.3} s; the regions were opened and restored in {} s",
        start.as_secs_f64(),
        opens.join(" and ")
    );
}

fn since(before: DiskFigures, after: DiskFigures) -> DiskFigures {
    DiskFigures {
        appends: after.appends - before.appends,
        appended_bytes: after.appended_bytes - before.appended_bytes,
        files_written: after.files_written - before.files_written,
        written_bytes: after.written_bytes - before.written_bytes,
        file_syncs: after.file_syncs - before.file_syncs,
        directory_syncs: after.directory_syncs - before.directory_syncs,
    }
}

/// A coupled pair's pace and the store's load along a busy border, against the same
/// regions with their viewers apart, on the disk as it is and with its syncs slowed to
/// what the owner's storage takes without its cache. The pair at the line runs for
/// almost as long as a region goes between two checkpoints, so that what a restore then
/// reads again is the most it can be.
#[test]
#[ignore = "a measurement; run it by name with --release --ignored --nocapture"]
fn what_a_coupled_pair_and_a_busy_border_cost() {
    for slowed in [None, Some(UNCACHED_SYNC)] {
        measure(slowed, false, 2);
        measure(slowed, true, *MEASURED_WINDOWS);
    }
}
