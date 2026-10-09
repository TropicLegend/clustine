//! The single process: the cluster's parts in one process, joined by channels
//! (`docs/adr/0017-the-end-of-the-stripes.md`, section 6).
//!
//! These are the scenarios P1 to P6 and P8 to P10 of that record's section 9.5, each
//! marked with its name, written from the record by someone who had not read
//! `Server::start`, `stop` and `take_over`, the worker's loop or the edge's
//! link-keeper. P7 is with the worker's loop (`src/cluster/worker.rs`), and so is
//! what P9 says of a store whose answers are held back, which a `Server` gives a
//! test no way to do. A few tests are of what sections 1, 2.2, 6.5 and 8 and N7 say of
//! the single process and no scenario has; each says so above it.
//!
//! Every test whose bots build keeps what they were acknowledged, and has somebody
//! who joins afterwards find each block so.
//!
//! **Where the server does something else than the record says, the test is kept and
//! ignored**, with the sequence, what the record says and what happened above it:
//! `cargo test -p clustine --test single -- --ignored` runs those.
//!
//! P5 is fifty lives of a server, which take about six minutes;
//! `CLUSTINE_SINGLE_LIVES` sets another number. How long a start and a stop took is
//! printed, as `single: a start took …`.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clustine::{Config, Server};
use clustine_botswarm::Bot;
use clustine_coordinator::Policy;
use clustine_data::blocks;
use clustine_protocol::packets::play::face;
use clustine_region::{Layout, RegionId};
use clustine_rpc::{ChunkBox, RegionHello, RegionInfo, RegionList, StoreRequest};
use clustine_world::ChunkPos;
use clustine_worldgen::FlatGenerator;
use clustine_worldstore::{Division, Store};

use common::{VIEW_DISTANCE, config, free_address, spawn_server_with_log, view_area};

const PATIENCE: Duration = Duration::from_secs(30);

/// How often a state that is waited for is looked at.
const LOOK: Duration = Duration::from_millis(20);

const AIR: Option<i32> = Some(blocks::AIR.0 as i32);
const STONE: Option<i32> = Some(blocks::STONE.0 as i32);

/// The region that holds the chunk players enter the world in, in every world here.
const HOME: RegionId = RegionId(0);

/// The chunk players enter the world in.
const HOME_CHUNK: ChunkPos = ChunkPos::new(0, 0);

/// The height of the block players stand on, and the height they stand at.
const FLOOR: i32 = -61;
const GROUND: f64 = -60.0;

/// The distances the worlds that follow their players are merged and split by: those
/// of the end-to-end tests of ADR-0016, which bots cover in seconds, with the rest of
/// one second that P4 and P5 ask for.
const FOLLOW: Policy = Policy {
    merge_distance: 3,
    split_distance: 5,
    rest: Duration::from_secs(1),
};

/// Where somebody who walks away from the spawn point is split off under [`FOLLOW`]:
/// in the chunk (6, 0), more than the split distance from the chunk players enter
/// in. And from where somebody else sees that chunk without being split off.
const ROVER: (f64, f64) = (104.5, 0.5);
const ROVER_CHUNK: ChunkPos = ChunkPos::new(6, 0);
const TOWARDS_ROVER: (f64, f64) = (56.5, 0.5);

/// Whether this run of the tests is the one that repeats the end-to-end tests on a
/// world divided into regions, which `CLUSTINE_TEST_PINS` asks for. These tests
/// say themselves how their worlds are divided, so they run once, in the run without
/// it.
fn a_repetition() -> bool {
    std::env::var_os("CLUSTINE_TEST_PINS").is_some()
}

/// A server whose world is one home region, kept in `world` if there is one.
fn open(world: Option<&Path>) -> Config {
    Config {
        world: world.map(Path::to_owned),
        pins: Vec::new(),
        follow: None,
        ..config()
    }
}

/// Starts a server, and says how long that took.
async fn start(config: Config) -> (Server, String) {
    let asked = Instant::now();
    let server = match Server::start(config).await {
        Ok(server) => server,
        Err(error) => panic!("the server did not start: {error:#}"),
    };
    println!("single: a start took {:?}", asked.elapsed());
    let address = server.address().to_string();
    (server, address)
}

/// Stops a server, and says how long that took.
async fn stop(server: Server) -> Duration {
    let asked = Instant::now();
    server.stop().await;
    let took = asked.elapsed();
    println!("single: a stop took {took:?}");
    took
}

/// The shortest, the middle and the longest of `times`, for a line of what a test
/// prints.
fn spread(times: &mut [Duration]) -> String {
    times.sort_unstable();
    let middle = times.get(times.len() / 2);
    format!(
        "between {:?} and {:?}, {middle:?} in the middle",
        times.first(),
        times.last()
    )
}

/// The world store's list of regions, as the server gives it.
fn list(server: &Server) -> RegionList {
    server
        .regions()
        .unwrap_or_else(|error| panic!("the list of regions cannot be read: {error:#}"))
}

/// The regions that live, in the order of their ids.
fn living(server: &Server) -> Vec<RegionId> {
    let regions = list(server).regions;
    regions.iter().map(|info| info.region).collect()
}

/// What the list says of `region`, which lives.
fn info(list: &RegionList, region: RegionId) -> &RegionInfo {
    let found = list.regions.iter().find(|info| info.region == region);
    found.unwrap_or_else(|| panic!("region {region} does not live: {list:?}"))
}

/// The highest epoch `region` was opened with.
fn epoch(server: &Server, region: RegionId) -> u64 {
    info(&list(server), region).epoch
}

/// Whether `chunk` is in the box of what a region was granted.
fn holds(bounds: Option<ChunkBox>, chunk: ChunkPos) -> bool {
    bounds.is_some_and(|bounds| {
        (bounds.min.x..=bounds.max.x).contains(&chunk.x)
            && (bounds.min.z..=bounds.max.z).contains(&chunk.z)
    })
}

/// Waits until `state` holds, looking every [`LOOK`], while `bots` stay connected.
async fn until(what: &str, bots: &mut [&mut Bot], mut state: impl FnMut() -> bool) {
    let waiting = Instant::now();
    while !state() {
        assert!(
            waiting.elapsed() <= PATIENCE,
            "{PATIENCE:?} passed before {what}"
        );
        if bots.is_empty() {
            tokio::time::sleep(LOOK).await;
        }
        for bot in bots.iter_mut() {
            if let Err(error) = bot.idle(LOOK).await {
                panic!("a bot was disconnected before {what}: {error:#}");
            }
        }
    }
}

/// Waits until `bot` holds exactly the chunks around `center`.
async fn settle(bot: &mut Bot, center: (i32, i32)) {
    let mut expected = view_area(center, VIEW_DISTANCE);
    expected.sort_unstable();
    bot.wait_until(PATIENCE, |bot| {
        bot.center == Some(center) && bot.chunks.keys().copied().eq(expected.iter().copied())
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "{error}: around {:?} with {} chunks, not around {center:?} with {}",
            bot.center,
            bot.chunks.len(),
            expected.len()
        )
    });
}

/// Joins and waits for the chunks around the spawn point.
async fn join(address: &str, name: &str) -> Bot {
    let mut bot = match Bot::join(address, name).await {
        Ok(bot) => bot,
        Err(error) => panic!("{name} could not join: {error:#}"),
    };
    settle(&mut bot, (0, 0)).await;
    bot
}

/// Waits until the server has handled everything `bot` did up to `sequence`.
async fn acknowledged(bot: &mut Bot, sequence: i32) {
    bot.wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= sequence)
        .await
        .unwrap_or_else(|error| panic!("{error}: sequence {sequence} was not acknowledged"));
}

/// Waits until `bot` sees the player called `name` at the given x and z.
async fn sees_at(bot: &mut Bot, name: &str, x: f64, z: f64) {
    bot.wait_until(PATIENCE, |bot| {
        bot.seen_player(name)
            .is_some_and(|entity| entity.position == (x, GROUND, z))
    })
    .await
    .unwrap_or_else(|error| panic!("{error}: {name} is seen as {:?}", bot.seen_player(name)));
}

/// Fails unless every acknowledgement `bot` has had came once: no number twice, and
/// none behind a higher one.
fn acknowledged_once(bot: &Bot) {
    let numbers: Vec<i32> = bot.acknowledgements.iter().map(|(n, _)| *n).collect();
    assert!(
        numbers.windows(2).all(|pair| pair[0] < pair[1]),
        "an action was acknowledged twice, or behind a later one: {numbers:?}"
    );
}

/// What the bots of a test were acknowledged: every block one of them changed, as the
/// server had shown it when it acknowledged the action. Whoever joins later has to
/// find each of them so.
#[derive(Default)]
struct Told(BTreeMap<(i32, i32, i32), Option<i32>>);

impl Told {
    /// `bot` places the stone it holds on top of the block at `on`, and is
    /// acknowledged. A client shows what the server said from then on, so the stone
    /// has to have been shown by then.
    async fn places(&mut self, bot: &mut Bot, on: (i32, i32, i32)) {
        let (x, y, z) = on;
        let sequence = bot.use_item_on(x, y, z, face::TOP).await.unwrap();
        acknowledged(bot, sequence).await;
        self.note(bot, (x, y + 1, z), STONE);
    }

    /// `bot` breaks the block at `at`, and is acknowledged.
    async fn digs(&mut self, bot: &mut Bot, at: (i32, i32, i32)) {
        let sequence = bot.dig(at.0, at.1, at.2).await.unwrap();
        acknowledged(bot, sequence).await;
        self.note(bot, at, AIR);
    }

    /// Keeps that the block at `at` is `state`, which `bot` has to have been shown.
    fn note(&mut self, bot: &Bot, at: (i32, i32, i32), state: Option<i32>) {
        assert_eq!(
            bot.block_at(at.0, at.1, at.2).unwrap(),
            state,
            "the block at {at:?} when what was done to it was acknowledged"
        );
        self.0.insert(at, state);
    }

    /// Fails unless every block in a chunk `bot` holds is as it was acknowledged.
    /// Returns those blocks.
    fn looked_at_by(&self, bot: &Bot) -> BTreeSet<(i32, i32, i32)> {
        let mut looked_at = BTreeSet::new();
        for (at, state) in &self.0 {
            let Some(found) = bot.block_at(at.0, at.1, at.2).unwrap() else {
                continue;
            };
            assert_eq!(Some(found), *state, "the block at {at:?}");
            looked_at.insert(*at);
        }
        looked_at
    }

    /// Waits until `bot`, who was there while others built, has been shown every
    /// block in a chunk it holds as it was acknowledged to whoever built it.
    async fn is_shown_to(&self, bot: &mut Bot) {
        for (at, state) in &self.0 {
            let (x, y, z) = *at;
            let shown = |bot: &Bot| {
                let found = bot.block_at(x, y, z).unwrap();
                found.is_none() || found == *state
            };
            bot.wait_until(PATIENCE, shown)
                .await
                .unwrap_or_else(|error| {
                    panic!(
                        "{error}: the block at {at:?} is {:?}, not {state:?}",
                        bot.block_at(x, y, z)
                    )
                });
        }
    }

    /// Somebody called `name` joins, and walks to each of `stands` in turn. They have
    /// to find every block as it was acknowledged, and to come by every one of them.
    async fn audit(&self, address: &str, name: &str, stands: &[(f64, f64)]) -> Bot {
        let mut auditor = join(address, name).await;
        let mut looked_at = self.looked_at_by(&auditor);
        for (x, z) in stands {
            auditor.walk_to(*x, *z, 1.0).await.unwrap();
            let chunk = ChunkPos::containing(*x, *z);
            settle(&mut auditor, (chunk.x, chunk.z)).await;
            looked_at.extend(self.looked_at_by(&auditor));
        }
        assert_eq!(
            looked_at.len(),
            self.0.len(),
            "the auditor came by every block that was acknowledged"
        );
        auditor
    }
}

/// Where a bot builds in its `turn`th turn, relative to the block column it stands in:
/// one of the 48 columns around it, and how many blocks are there already.
fn plot(turn: usize) -> (i32, i32, i32) {
    let columns: Vec<(i32, i32)> = (-3..=3)
        .flat_map(|x| (-3..=3).map(move |z| (x, z)))
        .filter(|column| *column != (0, 0))
        .collect();
    let (x, z) = columns[turn % columns.len()];
    (x, z, (turn / columns.len()) as i32)
}

/// The block that a bot standing in the block column `stands` builds on in its
/// `turn`th turn.
fn plot_at(stands: (i32, i32), turn: usize) -> (i32, i32, i32) {
    let (x, z, height) = plot(turn);
    (stands.0 + x, FLOOR + height, stands.1 + z)
}

/// Every file of the world in `directory`, with what is in it.
fn files(directory: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut found = BTreeMap::new();
    let mut directories = vec![directory.to_owned()];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                directories.push(path);
            } else {
                let bytes = std::fs::read(&path).unwrap();
                found.insert(path, bytes);
            }
        }
    }
    found
}

/// Whether anything accepts connections at `address`.
async fn listens(address: &str) -> bool {
    tokio::net::TcpStream::connect(address).await.is_ok()
}

// P1.
//
/// A new world without pins is the list of the record's T1 as soon as `start` has
/// returned, but for the epoch of its one region, which runs by then; and whoever
/// joins is sent the chunks around the spawn point. In memory and on a disk.
#[tokio::test]
async fn a_new_world_is_one_home_region_that_runs_as_soon_as_the_start_has_returned() {
    if a_repetition() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    for world in [None, Some(directory.path().join("world"))] {
        let (server, address) = start(open(world.as_deref())).await;

        // Nothing is waited for between the start and this.
        let list = list(&server);
        assert_eq!(list.home, HOME, "{list:?}");
        assert_eq!(list.next, RegionId(1), "{list:?}");
        assert!(list.absorbed.is_empty(), "{list:?}");
        let [home] = list.regions.as_slice() else {
            panic!("a new world is one region: {list:?}");
        };
        assert_eq!(home.region, HOME);
        assert!(home.pinned.is_empty(), "{list:?}");
        let alone = ChunkBox {
            min: HOME_CHUNK,
            max: HOME_CHUNK,
        };
        assert_eq!(home.bounds, Some(alone), "{list:?}");
        assert!(home.epoch > 0, "the home region runs: {list:?}");

        // `join` waits for exactly the chunks of `view_area` around the spawn chunk.
        let mut bot = join(&address, "Alice").await;
        assert_eq!(bot.stats.teleports_confirmed, 1);
        let mut told = Told::default();
        told.places(&mut bot, (2, FLOOR, 1)).await;
        told.digs(&mut bot, (-3, FLOOR, -2)).await;
        told.audit(&address, "Auditor", &[]).await;
        stop(server).await;
    }
}

// P1.
//
/// The file that has the home region's state is no state file any more. The record
/// says of such a world that it says so and is not touched (section 2.2), and of the
/// server that its start returns an error that names the region and that nothing
/// listens. This is the reading of "cannot be read" in which the file itself is
/// damaged; the test after this one is the other.
#[tokio::test]
async fn a_world_whose_state_file_of_the_home_region_is_damaged_does_not_start_and_is_not_touched()
{
    if a_repetition() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let world = directory.path().join("world");
    let mut told = Told::default();

    // A life in which something is built, so that the region has a state to keep.
    let (server, address) = start(open(Some(&world))).await;
    let mut builder = join(&address, "Builder").await;
    told.places(&mut builder, (2, FLOOR, 1)).await;
    stop(server).await;
    drop(builder);

    // A runner that is stopped checkpoints before it lets go (section 6.4).
    let state = world.join("regions").join("0.state");
    let kept = std::fs::read(&state)
        .unwrap_or_else(|error| panic!("the home region has no state file after a stop: {error}"));
    std::fs::write(&state, b"this is no state of a region").unwrap();
    let before = files(&world);

    let address = free_address().await;
    let on_disk = |world: &Path| Config {
        bind: address.parse().unwrap(),
        ..open(Some(world))
    };
    let error = match Server::start(on_disk(&world)).await {
        Ok(_) => panic!("a world whose state file is damaged was started"),
        Err(error) => format!("{error:#}"),
    };
    println!("the start said: {error}");
    assert!(
        error.contains("region 0") || error.contains("0.state"),
        "the error names the region: {error}"
    );
    assert!(!listens(&address).await, "nothing listens after that start");
    assert!(files(&world) == before, "the world was not touched");

    // A second start, at the same address, on a directory that is in order works.
    let other = directory.path().join("other");
    let (server, address) = start(on_disk(&other)).await;
    let mut visitor = join(&address, "Visitor").await;
    Told::default().places(&mut visitor, (2, FLOOR, 1)).await;
    stop(server).await;
    drop(visitor);

    // And so does one on the world itself once the file is what it was: the start
    // that failed changed nothing.
    std::fs::write(&state, kept).unwrap();
    let (server, address) = start(on_disk(&world)).await;
    told.audit(&address, "Auditor", &[]).await;
    stop(server).await;
}

/// Makes a world in `world` whose home region cannot be restored: the store has a
/// state of it that the worker cannot read. The file is in order; what the region is
/// said to have written into it is of this build by its first two bytes and nothing
/// a state can be read from behind.
fn make_a_world_that_cannot_be_restored(world: &Path) {
    let generator = Arc::new(FlatGenerator::classic());
    let store = Store::local_divided(world, generator, Division::open(HOME_CHUNK)).unwrap();
    let hello = RegionHello {
        region: HOME,
        epoch: 1,
        layout: Layout::single().fingerprint(),
    };
    let (handle, _) = store.open_region(hello).unwrap();
    let unreadable = vec![0, clustine_worker::STATE_FORMAT, 0xff, 0xff, 0xff];
    handle.request(StoreRequest::Checkpoint {
        tick: 4,
        state: unreadable,
    });
    handle.flush();
    drop(handle);
    store.flush().unwrap();
    assert!(
        world.join("regions").join("0.state").exists(),
        "the store kept the checkpoint"
    );
}

// P1.
//
/// The store has a state of the home region that the worker cannot read. Section 6.1:
/// the worker's loop ends with an error, `start` stops what it has started and
/// returns an error, and nothing listens. A second start on a directory that is in
/// order works, at the same address. What the error says is the test after this one.
#[tokio::test]
async fn a_world_whose_home_region_cannot_be_restored_does_not_start_and_nothing_listens() {
    if a_repetition() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let world = directory.path().join("world");
    make_a_world_that_cannot_be_restored(&world);

    let address = free_address().await;
    let on_disk = |world: &Path| Config {
        bind: address.parse().unwrap(),
        ..open(Some(world))
    };
    for _ in 0..5 {
        let started = Server::start(on_disk(&world)).await;
        assert!(started.is_err(), "the world was started");
        assert!(!listens(&address).await, "nothing listens after that start");
    }

    let other = directory.path().join("other");
    let (server, address) = start(on_disk(&other)).await;
    let mut told = Told::default();
    let mut visitor = join(&address, "Visitor").await;
    told.places(&mut visitor, (2, FLOOR, 1)).await;
    told.audit(&address, "Auditor", &[]).await;
    stop(server).await;

    // And the world that cannot be restored still does not start: the starts that
    // failed did not make it one that can.
    assert!(Server::start(on_disk(&world)).await.is_err());
    assert!(!listens(&address).await);
}

// P1.
//
// A finding. The sequence: a world whose home region has a state the worker cannot
// read (the world of the test above) is started, forty times. What the record says:
// section 6.1, point 4, "if the loop's task ends first, `start` returns with the
// loop's error", and below it, "`start` then stops what it has started and returns
// that error (`restoring region 0`, with the store's reason), so a world that cannot
// be restored does not start and says why"; P1, "`start` returns an error that names
// the region". What happened: about half of the starts (19 to 23 of 40 in four runs)
// return `the worker of this process has ended` and nothing else, with no region and
// no reason; the others return `restoring region 0: the stored state of the region
// as of tick 4 cannot be read: …`. Which of the two a start returns differs from one
// start to the next on the same world. Every one of them is an error, and nothing
// listens after any (the test above), so what is lost is the word of what is wrong
// with the world, which is what whoever started the server has to go by.
//
// The record's "Found while building", step C5.3, says that the loop lets go of its
// watch of what it serves when it ends and that "a closed watch is a worker that
// serves nothing any more, which is what `start` and `take_over` watch for beside
// the loop's end". That would be two ways for `start` to learn of the same end, of
// which only one has the error; `Server::start` was not read to see. If it is so,
// `take_over` has the same two ways (section 6.4, point 6: "or with the loop's
// error, if the loop's task ends first"), which no test here can reach.
//
/// Every start on such a world returns the error of the worker's loop: it names the
/// region and says why (`restoring region 0`, with the reason).
#[tokio::test]
async fn every_start_on_a_world_whose_home_region_cannot_be_restored_says_which_region_and_why() {
    if a_repetition() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let world = directory.path().join("world");
    make_a_world_that_cannot_be_restored(&world);

    const STARTS: usize = 40;
    let mut said_otherwise: BTreeMap<String, usize> = BTreeMap::new();
    for _ in 0..STARTS {
        let error = match Server::start(open(Some(&world))).await {
            Ok(_) => panic!("a world whose home region cannot be restored was started"),
            Err(error) => format!("{error:#}"),
        };
        if !(error.contains("restoring region 0") && error.contains("cannot be read")) {
            *said_otherwise.entry(error).or_default() += 1;
        }
    }
    assert!(
        said_otherwise.is_empty(),
        "{} of {STARTS} starts did not say which region cannot be restored, and why; what \
         they said, and how often: {said_otherwise:?}",
        said_otherwise.values().sum::<usize>()
    );
}

// P2.
//
/// With one pin and nothing that reshapes, the world is two pinned regions, both
/// running when `start` has returned. Somebody walks across the boundary at block
/// x = 16 and back, building on both sides, and is one entity to whoever watches.
#[tokio::test]
async fn a_pinned_world_is_two_regions_and_who_walks_across_and_back_is_one_entity_to_a_watcher() {
    if a_repetition() {
        return;
    }
    for serialise_link in [false, true] {
        let (server, address) = start(Config {
            pins: vec![1],
            serialise_link,
            ..open(None)
        })
        .await;

        let list = list(&server);
        let regions: Vec<RegionId> = list.regions.iter().map(|info| info.region).collect();
        assert_eq!(regions, [RegionId(0), RegionId(1)], "{list:?}");
        assert_eq!((list.home, list.next), (HOME, RegionId(2)), "{list:?}");
        assert!(list.absorbed.is_empty(), "{list:?}");
        for info in &list.regions {
            assert!(!info.pinned.is_empty(), "both are pinned: {list:?}");
            assert!(info.epoch > 0, "both run: {list:?}");
        }

        let mut watcher = join(&address, "Watcher").await;
        let mut walker = join(&address, "Walker").await;
        sees_at(&mut watcher, "Walker", 0.5, 0.5).await;
        let entity = walker.info.login.entity_id;
        let mut told = Told::default();

        told.places(&mut walker, (2, FLOOR, 2)).await;
        walker.walk_to(40.5, 0.5, 0.5).await.unwrap();
        settle(&mut walker, (2, 0)).await;
        sees_at(&mut watcher, "Walker", 40.5, 0.5).await;
        told.places(&mut walker, (42, FLOOR, 2)).await;
        told.digs(&mut walker, (41, FLOOR, 1)).await;
        walker.walk_to(0.5, 0.5, 0.5).await.unwrap();
        settle(&mut walker, (0, 0)).await;
        sees_at(&mut watcher, "Walker", 0.5, 0.5).await;
        told.places(&mut walker, (-2, FLOOR, 2)).await;

        // The watcher was shown one entity once, and never told that it had gone.
        // (The bot fails if an entity it shows is spawned again.)
        assert_eq!(watcher.stats.entities_spawned, 1);
        assert_eq!(watcher.stats.entities_removed, 0);
        let seen: Vec<i32> = watcher.entities.keys().copied().collect();
        assert_eq!(seen, [entity]);
        // And the walker is who they were, and was never put anywhere.
        assert_eq!(walker.info.login.entity_id, entity);
        assert_eq!(walker.stats.teleports_confirmed, 1);
        assert_eq!(walker.stats.entities_spawned, 1);
        assert_eq!(walker.stats.entities_removed, 0);

        // Nothing follows anybody here: the regions are what they were.
        assert_eq!(living(&server), [RegionId(0), RegionId(1)]);
        told.audit(&address, "Auditor", &[(40.5, 0.5)]).await;
        stop(server).await;
    }
}

// P3.
//
/// A home region that is pinned to nothing is taken over under somebody who builds
/// in it: the list has a higher epoch for it, the player stays who and where they
/// were and goes on building, and what they were acknowledged is there, for whoever
/// joins and for the next server on that directory.
#[tokio::test]
async fn a_region_that_is_taken_over_has_a_higher_epoch_and_its_player_stays_and_keeps_what_was_built()
 {
    if a_repetition() {
        return;
    }
    for serialise_link in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let on_disk = || Config {
            serialise_link,
            ..open(Some(directory.path()))
        };
        let (mut server, address) = start(on_disk()).await;
        let mut builder = join(&address, "Builder").await;
        let entity = builder.info.login.entity_id;
        let mut told = Told::default();
        told.places(&mut builder, (2, FLOOR, 1)).await;

        let mut last = epoch(&server, HOME);
        for turn in 0..3 {
            server.take_over(HOME).await.unwrap();
            let now = epoch(&server, HOME);
            assert!(now > last, "epoch {now} after epoch {last}");
            last = now;
            told.places(&mut builder, plot_at((0, 0), turn)).await;
            told.digs(&mut builder, (3, FLOOR, turn as i32 - 1)).await;
        }
        assert_eq!(builder.info.login.entity_id, entity);
        assert_eq!(builder.stats.teleports_confirmed, 1);
        acknowledged_once(&builder);
        assert_eq!(living(&server), [HOME]);

        told.audit(&address, "Auditor", &[]).await;
        stop(server).await;
        drop(builder);

        let (server, address) = start(on_disk()).await;
        told.audit(&address, "Auditor", &[]).await;
        stop(server).await;
    }
}

// P3.
//
/// `take_over` is called right after `start` and again at once, with nothing waited
/// for in between: both return, each with a higher epoch. In a world of one home
/// region and in one of two pinned regions, of each region.
#[tokio::test]
async fn a_region_is_taken_over_right_after_the_start_and_again_at_once() {
    if a_repetition() {
        return;
    }
    for pins in [vec![], vec![1]] {
        let (mut server, address) = start(Config { pins, ..open(None) }).await;
        for region in living(&server) {
            let started = epoch(&server, region);
            server.take_over(region).await.unwrap();
            let once = epoch(&server, region);
            server.take_over(region).await.unwrap();
            let twice = epoch(&server, region);
            assert!(
                started < once && once < twice,
                "region {region} was run with the epochs {started}, {once} and {twice}"
            );
        }

        // And the world is served.
        let mut builder = join(&address, "Builder").await;
        let mut told = Told::default();
        told.places(&mut builder, (2, FLOOR, 1)).await;
        builder.walk_to(40.5, 0.5, 0.5).await.unwrap();
        settle(&mut builder, (2, 0)).await;
        told.places(&mut builder, (42, FLOOR, 2)).await;
        assert_eq!(builder.stats.teleports_confirmed, 1);
        told.audit(&address, "Auditor", &[(40.5, 0.5)]).await;
        stop(server).await;
    }
}

// P3.
//
/// `take_over` of a region the world does not have is an error that says so
/// (section 6.4, point 1), and the server goes on serving.
#[tokio::test]
async fn taking_over_a_region_the_world_does_not_have_is_an_error() {
    if a_repetition() {
        return;
    }
    let (mut server, address) = start(open(None)).await;
    let mut builder = join(&address, "Builder").await;
    let mut told = Told::default();
    told.places(&mut builder, (2, FLOOR, 1)).await;

    let before = list(&server);
    for region in [RegionId(1), RegionId(7)] {
        let error = match server.take_over(region).await {
            Ok(()) => panic!("region {region}, which the world does not have, was taken over"),
            Err(error) => format!("{error:#}"),
        };
        assert!(error.contains("the world has no region"), "{error}");
    }
    assert_eq!(list(&server), before, "nothing came of it");

    told.places(&mut builder, (3, FLOOR, 1)).await;
    assert_eq!(builder.stats.teleports_confirmed, 1);
    told.audit(&address, "Auditor", &[]).await;
    stop(server).await;
}

// P3, of a region that is no stripe: a part.
//
/// Somebody walks away and is split off. Their part is taken over as soon as the list
/// has it, when it may still be being opened at the store (section 6.4, point 1: the
/// takeover waits until it runs), and again when they have built in it; then the
/// home region under whoever stayed. Each time the list has a higher epoch, nobody
/// is disconnected or put anywhere, and what both were acknowledged is there, also
/// for the next server on that directory.
#[tokio::test]
async fn a_part_is_taken_over_as_soon_as_the_list_has_it_and_its_player_stays_and_keeps_what_was_built()
 {
    if a_repetition() {
        return;
    }
    for serialise_link in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let (mut server, address) = start(Config {
            follow: Some(FOLLOW),
            serialise_link,
            ..open(Some(directory.path()))
        })
        .await;
        let stands = (ROVER.0.floor() as i32, ROVER.1.floor() as i32);
        let mut told = Told::default();
        let mut stays = join(&address, "Stays").await;
        let mut rover = join(&address, "Rover").await;
        told.places(&mut stays, plot_at((0, 0), 0)).await;
        rover.walk_to(ROVER.0, ROVER.1, 2.0).await.unwrap();
        {
            let mut both = [&mut stays, &mut rover];
            let has_the_part = || living(&server).len() == 2;
            until("the second bot was split off", &mut both, has_the_part).await;
        }
        let split = list(&server);
        let part = split.regions[1].region;

        // At once: nothing is waited for between the list and this.
        let mut last = info(&split, part).epoch;
        for turn in 0..3 {
            server.take_over(part).await.unwrap();
            let now = epoch(&server, part);
            assert!(now > last, "epoch {now} after epoch {last} of the part");
            last = now;
            settle(&mut rover, (ROVER_CHUNK.x, ROVER_CHUNK.z)).await;
            told.places(&mut rover, plot_at(stands, turn)).await;
        }
        let home = epoch(&server, HOME);
        server.take_over(HOME).await.unwrap();
        assert!(epoch(&server, HOME) > home);
        told.places(&mut stays, plot_at((0, 0), 1)).await;
        told.places(&mut rover, plot_at(stands, 3)).await;

        for bot in [&stays, &rover] {
            assert_eq!(bot.stats.teleports_confirmed, 1);
            acknowledged_once(bot);
        }
        // They are six chunks apart: nothing is merged, and nothing split again.
        assert_eq!(living(&server), [HOME, part]);
        told.audit(&address, "Auditor", &[TOWARDS_ROVER]).await;
        stop(server).await;
        drop((stays, rover));

        let (server, address) = start(open(Some(directory.path()))).await;
        told.audit(&address, "Auditor", &[TOWARDS_ROVER]).await;
        stop(server).await;
    }
}

// N7 of section 7, with the means of P3: the home region has no owner for a moment.
//
/// Somebody joins while the home region changes hands. The join is kept and sent when
/// the edge is linked to the region again: they are let in at the spawn point, are
/// put there once, and build; and those who were there before stay.
#[tokio::test]
async fn somebody_who_joins_while_the_home_region_changes_hands_is_let_in() {
    if a_repetition() {
        return;
    }
    for serialise_link in [false, true] {
        let (mut server, address) = start(Config {
            serialise_link,
            ..open(None)
        })
        .await;
        let mut told = Told::default();
        let mut bots: Vec<Bot> = Vec::new();
        for turn in 0..6 {
            let name = format!("Joins{turn}");
            let (taken, joined) = tokio::join!(server.take_over(HOME), Bot::join(&address, &name));
            taken.unwrap();
            let mut bot = match joined {
                Ok(bot) => bot,
                Err(error) => panic!("{name} could not join: {error:#}"),
            };
            settle(&mut bot, (0, 0)).await;
            told.places(&mut bot, plot_at((0, 0), turn)).await;
            bots.push(bot);
            // Those who came before are still there, and are told of the newcomer.
            let players = bots.len();
            for bot in &mut bots {
                bot.wait_until(PATIENCE, |bot| bot.player_list.len() == players)
                    .await
                    .unwrap();
            }
        }
        for bot in &mut bots {
            // Everybody sees the five others, each of whom was shown once: the bot
            // fails if an entity it shows is spawned again.
            bot.wait_until(PATIENCE, |bot| bot.entities.len() == 5)
                .await
                .unwrap_or_else(|error| panic!("{error}: {:?}", bot.entities));
            assert_eq!(bot.stats.teleports_confirmed, 1);
            assert_eq!(bot.stats.entities_removed, 0);
            told.is_shown_to(bot).await;
        }
        told.audit(&address, "Auditor", &[]).await;
        stop(server).await;
    }
}

// Section 1, point 6, and section 2.2, of the single process: no scenario of section
// 9.5 has it, and the store's (T3, T4, T7) are of the store alone.
//
/// A world that was served with a pin is started without one: it is made over into
/// one home region that holds the home chunk, and what was built in it stays. Started
/// with the pin again, it is made over into two pinned regions, and what was built
/// stays. Started so once more, it is found as it was.
#[tokio::test]
async fn a_world_that_was_divided_otherwise_is_made_over_and_keeps_what_was_built() {
    if a_repetition() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let on_disk = |pins: &[i32]| Config {
        pins: pins.to_vec(),
        ..open(Some(directory.path()))
    };
    let mut told = Told::default();
    let far = [(40.5, 0.5)];

    // Two pinned regions, with something built in each.
    let (server, address) = start(on_disk(&[1])).await;
    let mut builder = join(&address, "Builder").await;
    told.places(&mut builder, plot_at((0, 0), 0)).await;
    builder.walk_to(40.5, 0.5, 1.0).await.unwrap();
    settle(&mut builder, (2, 0)).await;
    told.places(&mut builder, plot_at((40, 0), 0)).await;
    let pinned = list(&server);
    stop(server).await;
    drop(builder);

    // Without the pin: one home region, pinned to nothing, holding the home chunk,
    // as soon as `start` has returned. The id of its next region is not below the
    // one the world had.
    let (server, address) = start(on_disk(&[])).await;
    let open_now = list(&server);
    let [home] = open_now.regions.as_slice() else {
        panic!("a world that is made over without pins is one region: {open_now:?}");
    };
    assert_eq!((open_now.home, home.region), (HOME, HOME), "{open_now:?}");
    assert!(home.pinned.is_empty(), "{open_now:?}");
    let alone = ChunkBox {
        min: HOME_CHUNK,
        max: HOME_CHUNK,
    };
    assert_eq!(home.bounds, Some(alone), "{open_now:?}");
    assert!(open_now.absorbed.is_empty(), "{open_now:?}");
    assert!(
        open_now.next >= pinned.next,
        "{open_now:?} after {pinned:?}"
    );
    assert!(home.epoch > info(&pinned, HOME).epoch, "{open_now:?}");
    let mut builder = told.audit(&address, "Builder", &far).await;
    told.places(&mut builder, plot_at((40, 0), 1)).await;
    builder.walk_to(0.5, 0.5, 1.0).await.unwrap();
    settle(&mut builder, (0, 0)).await;
    told.places(&mut builder, plot_at((0, 0), 1)).await;
    stop(server).await;
    drop(builder);

    // With the pin again: two pinned regions, both running.
    let (server, address) = start(on_disk(&[1])).await;
    let again = list(&server);
    let regions: Vec<RegionId> = again.regions.iter().map(|info| info.region).collect();
    assert_eq!(regions, [RegionId(0), RegionId(1)], "{again:?}");
    assert_eq!(again.home, HOME, "{again:?}");
    for info in &again.regions {
        assert!(!info.pinned.is_empty() && info.epoch > 0, "{again:?}");
    }
    let mut builder = told.audit(&address, "Builder", &far).await;
    told.places(&mut builder, plot_at((40, 0), 2)).await;
    stop(server).await;
    drop(builder);

    // And once more, the same: found as it was, with every region opened anew.
    let (server, address) = start(on_disk(&[1])).await;
    let found = list(&server);
    assert_eq!(found.regions.len(), 2, "{found:?}");
    assert_eq!((found.next, &found.absorbed), (again.next, &again.absorbed));
    for (now, before) in found.regions.iter().zip(&again.regions) {
        assert_eq!((now.region, &now.pinned), (before.region, &before.pinned));
        assert!(now.epoch > before.epoch, "{found:?} after {again:?}");
    }
    told.audit(&address, "Auditor", &far).await;
    stop(server).await;
}

/// Where the three who are split off in P4 stand: more than the split distance from
/// the chunk players enter in and from each other. And the block each builds on.
const APART: [(f64, f64); 3] = [(104.5, 0.5), (-104.5, 0.5), (0.5, 104.5)];
const BUILT_APART: [(i32, i32, i32); 3] = [(106, FLOOR, 2), (-106, FLOOR, 2), (2, FLOOR, 106)];

/// From where somebody who joins sees what the three of [`APART`] built without
/// going further from the chunk players enter in than the split distance.
const TOWARDS: [(f64, f64); 3] = [(56.5, 0.5), (-56.5, 0.5), (0.5, 56.5)];

// P4.
//
/// A world is stopped while it has three parts, each with somebody in it. The server
/// that is started on its disk, with a rest of one second, returns from `start` with
/// all four regions running; nobody is in the parts any more, and within twenty
/// seconds the list has the home region alone and the parts as absorbed. What was
/// built in the parts is then the home region's to show.
#[tokio::test]
async fn a_world_that_was_stopped_with_three_parts_starts_with_all_four_regions_and_absorbs_the_parts()
 {
    if a_repetition() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let following = || Config {
        follow: Some(FOLLOW),
        ..open(Some(directory.path()))
    };
    let mut told = Told::default();

    // The life in which the parts are made: three walk away, one after the other,
    // and each is split off when they have stood.
    let (server, address) = start(following()).await;
    let mut bots = Vec::new();
    for name in ["East", "West", "South"] {
        bots.push(join(&address, name).await);
    }
    for (walker, (x, z)) in APART.iter().enumerate() {
        bots[walker].walk_to(*x, *z, 2.0).await.unwrap();
        let mut all: Vec<&mut Bot> = bots.iter_mut().collect();
        let what = format!("a part was split off for the player at {x}, {z}");
        until(&what, &mut all, || living(&server).len() == walker + 2).await;
    }
    for (bot, on) in bots.iter_mut().zip(BUILT_APART) {
        told.places(bot, on).await;
        assert_eq!(bot.stats.teleports_confirmed, 1);
    }
    let stopped_with = list(&server);
    assert_eq!(stopped_with.regions.len(), 4, "{stopped_with:?}");
    let parts: Vec<RegionId> = stopped_with.regions[1..]
        .iter()
        .map(|info| info.region)
        .collect();
    stop(server).await;
    drop(bots);

    // The next life. Every region of the world runs when `start` has returned: each
    // has been opened with a higher epoch than it was last run with.
    let (mut server, address) = start(following()).await;
    let started = Instant::now();
    let started_with = list(&server);
    assert_eq!(living(&server).len(), 4, "{started_with:?}");
    for before in &stopped_with.regions {
        let now = info(&started_with, before.region);
        assert!(
            now.epoch > before.epoch,
            "region {} runs: it had epoch {} and has {}",
            before.region,
            before.epoch,
            now.epoch
        );
    }

    // Twenty seconds are the scenario's.
    let within = Duration::from_secs(20);
    while living(&server) != [HOME] {
        assert!(
            started.elapsed() <= within,
            "the parts were not absorbed within {within:?}: {:?}",
            list(&server)
        );
        tokio::time::sleep(LOOK).await;
    }
    println!(
        "single: the parts were absorbed {:?} after the start",
        started.elapsed()
    );
    let absorbed: BTreeSet<RegionId> = list(&server)
        .absorbed
        .iter()
        .map(|(absorbed, _)| *absorbed)
        .collect();
    assert_eq!(
        absorbed,
        parts.iter().copied().collect(),
        "{:?}",
        list(&server)
    );
    // A region that was absorbed is one the world does not have (section 6.4).
    for part in parts {
        let error = match server.take_over(part).await {
            Ok(()) => panic!("region {part}, which was absorbed, was taken over"),
            Err(error) => format!("{error:#}"),
        };
        assert!(error.contains("the world has no region"), "{error}");
    }

    told.audit(&address, "Auditor", &TOWARDS).await;
    assert_eq!(living(&server), [HOME]);
    stop(server).await;
}

/// How many lives the servers of P5 have: fifty, as the scenario has it, or what
/// `CLUSTINE_SINGLE_LIVES` says.
fn lives() -> usize {
    match std::env::var("CLUSTINE_SINGLE_LIVES") {
        Ok(lives) => lives.parse().expect("CLUSTINE_SINGLE_LIVES is a number"),
        Err(_) => 50,
    }
}

// P5.
//
/// A server is stopped and started again on the same directory, fifty times in a row,
/// each start right after the stop before it has returned. Somebody builds in every
/// life, and after the last every block that was acknowledged is there. Nothing
/// reshapes here; the test after this one is the scenario's second half.
#[tokio::test]
async fn fifty_lives_on_one_directory_each_begun_as_the_last_has_stopped_keep_every_block() {
    if a_repetition() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let mut told = Told::default();
    let mut last = 0;
    let (mut starts, mut stops) = (Vec::new(), Vec::new());
    for life in 0..lives() {
        // Nothing is between a stop and the next start.
        let asked = Instant::now();
        let started = Server::start(open(Some(directory.path()))).await;
        starts.push(asked.elapsed());
        let server = match started {
            Ok(server) => server,
            Err(error) => panic!("life {life} did not start: {error:#}"),
        };
        let now = epoch(&server, HOME);
        assert!(now > last, "life {life} runs the home region");
        last = now;
        let address = server.address().to_string();
        let mut builder = join(&address, "Builder").await;
        // What the lives before left is what this one begins with.
        told.looked_at_by(&builder);
        told.places(&mut builder, plot_at((0, 0), life)).await;
        // While the builder is still there.
        let asked = Instant::now();
        server.stop().await;
        stops.push(asked.elapsed());
    }
    println!(
        "single: in lives that follow each other at once a start took {} and a stop {}",
        spread(&mut starts),
        spread(&mut stops)
    );

    let (server, address) = start(open(Some(directory.path()))).await;
    told.audit(&address, "Auditor", &[]).await;
    stop(server).await;
}

/// Whether the life numbered `life` is one of those that are stopped while the
/// second bot's split is begun: every fifth, ten of fifty.
fn is_stopped_in_the_split(life: usize) -> bool {
    life % 5 == 2
}

/// Fails unless `list` is of a world none of whose regions is half there: every id
/// the store has given out is a region that lives or one that was absorbed, once;
/// and every region that lives has been opened, as `start` has returned.
fn is_whole(list: &RegionList, life: usize) {
    let mut ids: Vec<u32> = list.regions.iter().map(|info| info.region.0).collect();
    ids.extend(list.absorbed.iter().map(|(absorbed, _)| absorbed.0));
    ids.sort_unstable();
    let given_out: Vec<u32> = (0..list.next.0).collect();
    assert_eq!(ids, given_out, "life {life}: {list:?}");
    assert_eq!(list.home, HOME, "life {life}: {list:?}");
    for info in &list.regions {
        assert!(info.epoch > 0, "life {life}: every region runs: {list:?}");
    }
}

// P5.
//
/// Fifty lives on one directory, each begun as the last has stopped, with a rest of
/// one second. In every life one bot builds at the spawn point and a second walks
/// beyond the split distance and is split off, so that a stop meets parts and
/// readings of the list. Every fifth life is stopped when the second bot has been
/// out there for a second and the list does not have its part yet, which is when
/// its split is begun. Every start works whichever way that split went; the list of
/// the next life has the part, whole, or has not; and after the last life every
/// block that was acknowledged is there.
///
/// One thing is in here that the scenario does not say, and that "split off in every
/// life" needs: each life waits, before anybody joins, until the part of the life
/// before has been absorbed. A part without players is absorbed by the home region
/// only while that has no players either (ADR-0016, section 4.4). Left alone it
/// would keep its land, as nothing is given back sooner than thirty seconds after a
/// restore (section 3.2), and the second bot of the next life would walk into that
/// land and be handed over to it where it was to be split off.
#[tokio::test]
async fn fifty_lives_in_which_somebody_is_split_off_and_some_are_stopped_in_the_split_keep_every_block()
 {
    if a_repetition() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let following = || Config {
        follow: Some(FOLLOW),
        ..open(Some(directory.path()))
    };
    let stands = (ROVER.0.floor() as i32, ROVER.1.floor() as i32);
    let mut told = Told::default();
    // What the life before left: its list when it was stopped, and whether that had
    // the part of that life.
    let mut left: Option<(RegionList, bool)> = None;
    let (mut in_the_split, mut before_the_part, mut made, mut not_made) = (0, 0, 0, 0);
    // How long the second bot had been out there when the list had its part, in the
    // lives that waited for that.
    let mut took: Vec<Duration> = Vec::new();
    let (mut starts, mut stops) = (Vec::new(), Vec::new());

    for life in 0..lives() {
        // Nothing is between a stop and the next start.
        let asked = Instant::now();
        let started = Server::start(following()).await;
        starts.push(asked.elapsed());
        let server = match started {
            Ok(server) => server,
            Err(error) => panic!("life {life} did not start: {error:#}"),
        };
        let address = server.address().to_string();
        let found = list(&server);
        is_whole(&found, life);
        if let Some((left, had_its_part)) = &left {
            // Nothing that lived has gone but by being absorbed.
            for before in &left.regions {
                let lives = found.regions.iter().any(|now| now.region == before.region);
                let absorbed = found
                    .absorbed
                    .iter()
                    .any(|(gone, _)| *gone == before.region);
                assert!(lives || absorbed, "life {life}: {found:?} after {left:?}");
            }
            // The part of the life before is there or is not, and never half of it.
            let part = match had_its_part {
                true => RegionId(left.next.0 - 1),
                false => left.next,
            };
            let has_it = found.regions.iter().any(|now| now.region == part);
            if *had_its_part {
                assert!(has_it, "life {life}: {found:?} after {left:?}");
                assert_eq!(
                    found.next, left.next,
                    "life {life}: {found:?} after {left:?}"
                );
            } else if has_it {
                made += 1;
                assert_eq!(found.next.0, part.0 + 1, "life {life}: {found:?}");
            } else {
                not_made += 1;
                assert_eq!(
                    found.next, left.next,
                    "life {life}: {found:?} after {left:?}"
                );
            }
            if has_it {
                assert!(
                    holds(info(&found, part).bounds, ROVER_CHUNK),
                    "life {life}: the part has the chunk it was split off for: {found:?}"
                );
                assert!(
                    !holds(info(&found, HOME).bounds, ROVER_CHUNK),
                    "life {life}: the home region has no land where the part is: {found:?}"
                );
            }
        }

        // See above: the part of the life before goes into the home region first.
        until("the part of the life before was absorbed", &mut [], || {
            living(&server) == [HOME]
        })
        .await;

        let mut builder = join(&address, "Builder").await;
        let mut rover = join(&address, "Rover").await;
        told.looked_at_by(&builder);
        told.places(&mut builder, plot_at((0, 0), life)).await;
        rover.walk_to(ROVER.0, ROVER.1, 4.0).await.unwrap();
        let out_there = Instant::now();

        let has_the_part = || living(&server).len() == 2;
        if is_stopped_in_the_split(life) {
            // "When the second bot has been beyond the split distance for a second
            // and `regions()` does not have its part yet." **The second is time
            // that passes, and it is the scenario's.** The ten stops are spread from
            // there to when the lives before had the part, so that they fall on
            // different moments of the split.
            in_the_split += 1;
            let second = Duration::from_secs(1);
            took.sort_unstable();
            let usual = took.get(took.len() / 2).copied().unwrap_or(second);
            let spread = usual.saturating_sub(second) * (life as u32 / 5 % 10) / 10;
            while out_there.elapsed() < second + spread && !has_the_part() {
                rover.idle(LOOK).await.unwrap();
                builder.idle(LOOK).await.unwrap();
            }
            if !has_the_part() {
                before_the_part += 1;
            }
        } else {
            let mut both = [&mut builder, &mut rover];
            until("the second bot was split off", &mut both, has_the_part).await;
            took.push(out_there.elapsed());
            settle(&mut rover, (ROVER_CHUNK.x, ROVER_CHUNK.z)).await;
            told.looked_at_by(&rover);
            told.places(&mut rover, plot_at(stands, life)).await;
            assert_eq!(rover.stats.teleports_confirmed, 1);
        }
        let leaves = list(&server);
        let had_its_part = leaves.regions.len() == 2;
        // While both are still there.
        let asked = Instant::now();
        server.stop().await;
        stops.push(asked.elapsed());
        left = Some((leaves, had_its_part));
    }
    println!(
        "single: in lives with a part, of which some were stopped in the split, a start took \
         {} and a stop {}",
        spread(&mut starts),
        spread(&mut stops)
    );
    took.sort_unstable();
    println!(
        "single: {in_the_split} lives were stopped when the second bot had been out there for \
         a second or a little more, {before_the_part} of them before the list had its part; \
         the next life found the part of {made} of those and none of {not_made}. In the other \
         lives the list had the part between {:?} and {:?} after the bot was out there",
        took.first(),
        took.last()
    );

    // Whatever the last life left, a server that reshapes nothing shows what was
    // built, at the spawn point and where the second bot stood.
    let (server, address) = start(open(Some(directory.path()))).await;
    is_whole(&list(&server), lives());
    told.audit(&address, "Auditor", &[TOWARDS_ROVER]).await;
    stop(server).await;
}

// P6.
//
/// A server is stopped while players are connected: `stop` returns, and not after the
/// twenty seconds a worker that leaves waits to be relieved. The players find their
/// connections closed, and the next server on that directory has what they were
/// acknowledged. In a world of one home region and in one of two pinned regions with
/// somebody in each.
#[tokio::test]
async fn a_server_is_stopped_while_players_are_connected_and_does_not_wait_to_be_relieved() {
    if a_repetition() {
        return;
    }
    for pins in [vec![], vec![1]] {
        let directory = tempfile::tempdir().unwrap();
        let on_disk = || Config {
            pins: pins.clone(),
            ..open(Some(directory.path()))
        };
        let (server, address) = start(on_disk()).await;
        let mut stays = join(&address, "Stays").await;
        let mut walks = join(&address, "Walks").await;
        let mut told = Told::default();
        told.places(&mut stays, (2, FLOOR, 1)).await;
        walks.walk_to(40.5, 0.5, 0.5).await.unwrap();
        settle(&mut walks, (2, 0)).await;
        told.places(&mut walks, (42, FLOOR, 2)).await;

        let took = stop(server).await;
        // Half of what a worker that leaves waits for another to take its regions.
        assert!(took < Duration::from_secs(10), "the stop took {took:?}");
        for bot in [&mut stays, &mut walks] {
            let closed = bot.idle(PATIENCE).await;
            assert!(closed.is_err(), "the connection of a player was not closed");
        }

        let (server, address) = start(on_disk()).await;
        told.audit(&address, "Auditor", &[(40.5, 0.5)]).await;
        stop(server).await;
    }
}

/// The single process as a process of its own, with its log in a file.
struct Process {
    child: tokio::process::Child,
    address: String,
    log: PathBuf,
    /// Where its world and its log are, for as long as it is there.
    #[allow(dead_code)] // Held, never looked at.
    directory: tempfile::TempDir,
}

impl Process {
    /// Starts `clustine` with the arguments `more` besides where it listens and
    /// where its world is, and waits until it accepts connections.
    async fn start(more: &[&str]) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let address = free_address().await;
        let log = directory.path().join("log");
        let world = directory.path().join("world");
        let child = spawn_server_with_log(&address, &world, more, &log).await;
        Self {
            child,
            address,
            log,
            directory,
        }
    }

    /// What it has logged.
    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// The lines of its log that have `words`.
    fn lines_with(&self, words: &str) -> Vec<String> {
        let log = self.log();
        let lines = log.lines().filter(|line| line.contains(words));
        lines.map(str::to_owned).collect()
    }

    /// Waits until it has logged a line with `words`, and returns the first.
    async fn says(&self, words: &str) -> String {
        let waiting = Instant::now();
        loop {
            if let Some(line) = self.lines_with(words).into_iter().next() {
                return line;
            }
            assert!(
                waiting.elapsed() <= PATIENCE,
                "the process did not log `{words}`:\n{}",
                self.log()
            );
            tokio::time::sleep(LOOK).await;
        }
    }

    /// Sends it a signal by name, as `kill` does.
    async fn signal(&self, signal: &str) {
        let pid = self.child.id().expect("the process runs").to_string();
        let sent = tokio::process::Command::new("kill")
            .args([signal, &pid])
            .status()
            .await;
        assert!(sent.unwrap().success(), "kill {signal} {pid}");
    }
}

const BY_ITSELF: &str = "reshaping by itself: regions merge and split by where their players are";
const BY_HAND: &str = "reshaping by hand: regions merge and split when somebody asks";

/// What a coordinator that decides by itself logs when its first list shows a pinned
/// region (N14 of the record).
const PINNED: &str = "the world has regions that are pinned to an area: a region that is split \
                      off here cannot grow. Start the coordinator with --reshape by-hand to keep \
                      pinned regions as they are";

// P8, as far as it holds before `by-itself` is what a server does unless told: the
// refusal of `--boundaries`, the default and section 3.5's line are the later steps'.
//
/// `clustine --reshape by-itself` logs that it reshapes by itself, with the distances
/// that follow from the view distance it has when it is told none, 22 and 30.
/// `clustine --reshape by-hand` logs that it reshapes by hand, and so does a server
/// that is told nothing, at this step.
#[tokio::test]
async fn the_single_process_says_in_its_log_how_it_reshapes() {
    if a_repetition() {
        return;
    }
    let by_itself = Process::start(&["--reshape", "by-itself"]).await;
    let line = by_itself.says(BY_ITSELF).await;
    for field in ["merge_distance=22", "split_distance=30"] {
        let fields: Vec<&str> = line.split_whitespace().collect();
        assert!(fields.contains(&field), "{field} is not in: {line}");
    }
    assert!(
        by_itself.lines_with(BY_HAND).is_empty(),
        "{}",
        by_itself.log()
    );
    assert_eq!(by_itself.lines_with(BY_ITSELF).len(), 1);

    for flags in [&["--reshape", "by-hand"][..], &[]] {
        let by_hand = Process::start(flags).await;
        by_hand.says(BY_HAND).await;
        assert!(
            by_hand.lines_with(BY_ITSELF).is_empty(),
            "{}",
            by_hand.log()
        );
        assert_eq!(by_hand.lines_with(BY_HAND).len(), 1);
    }
}

// P8.
//
/// `clustine --pin 4 --reshape by-itself` logs N14's line when it has read its list:
/// once, as a warning. A server that reshapes such a world by hand does not log it,
/// and neither does one that reshapes a world without pins by itself.
#[tokio::test]
async fn a_single_process_that_reshapes_pinned_regions_by_itself_says_so_once_it_has_read_its_list()
{
    if a_repetition() {
        return;
    }
    let view = VIEW_DISTANCE.to_string();
    let pinned = [
        "--view-distance",
        &view,
        "--pin",
        "4",
        "--reshape",
        "by-itself",
    ];
    let pinned = Process::start(&pinned).await;
    let line = pinned.says(PINNED).await;
    assert!(line.contains(" WARN "), "it is a warning: {line}");
    // Somebody plays, which takes the list to have been read, and regions to have
    // been reported, many times over.
    let mut told = Told::default();
    let mut builder = join(&pinned.address, "Builder").await;
    told.places(&mut builder, (2, FLOOR, 1)).await;
    told.audit(&pinned.address, "Auditor", &[]).await;
    assert_eq!(pinned.lines_with(PINNED).len(), 1, "{}", pinned.log());

    let others: [&[&str]; 2] = [
        &[
            "--view-distance",
            &view,
            "--pin",
            "4",
            "--reshape",
            "by-hand",
        ],
        &["--view-distance", &view, "--reshape", "by-itself"],
    ];
    for flags in others {
        let other = Process::start(flags).await;
        // Players are let in when the routing table is whole, which it is by a list.
        let mut told = Told::default();
        let mut builder = join(&other.address, "Builder").await;
        told.places(&mut builder, (2, FLOOR, 1)).await;
        told.audit(&other.address, "Auditor", &[]).await;
        assert!(
            other.lines_with("pinned to an area").is_empty(),
            "{}",
            other.log()
        );
    }
}

// Section 8 of the record, of the single process: P8 does not have it, and nothing
// else of section 9.5 does.
//
/// `--pin` is refused unless its coordinates ascend without repetition, with exit
/// code 2 and the sentence of section 8, and before anything is made of a world.
#[tokio::test]
async fn the_single_process_refuses_pins_that_do_not_ascend() {
    if a_repetition() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    for pins in ["4,4", "5,4", "0,-1"] {
        let address = free_address().await;
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_clustine"));
        command
            .args(["--bind", &address, "--world"])
            .arg(directory.path().join("refused"))
            .arg(format!("--pin={pins}"))
            .env("NO_COLOR", "1")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        let Ok(output) = tokio::time::timeout(PATIENCE, command.output()).await else {
            panic!("clustine --pin={pins} did not end");
        };
        let output = output.unwrap();
        let complained = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "--pin={pins}: {complained}");
        let sentence = "--pin takes chunk x coordinates in ascending order without repetitions";
        assert!(complained.contains(sentence), "--pin={pins}: {complained}");
    }
    assert!(!directory.path().join("refused").exists());
}

/// A single process that is started with `pins`, which pin four regions side by side
/// with cuts at the chunk x coordinates -2, 0 and 5: somebody walks through all four
/// and builds in each, and whoever joins afterwards finds it.
async fn four_regions_are_pinned_at_negative_and_positive_cuts(pins: &[&str]) {
    let view = VIEW_DISTANCE.to_string();
    let mut flags = vec!["--view-distance", &view];
    flags.extend(pins);
    let process = Process::start(&flags).await;
    let mut told = Told::default();
    let mut builder = join(&process.address, "Builder").await;
    told.places(&mut builder, (2, FLOOR, 1)).await;
    for (x, on) in [(-24.5, -26), (-56.5, -58), (88.5, 90)] {
        builder.walk_to(x, 0.5, 1.0).await.unwrap();
        let chunk = ChunkPos::containing(x, 0.5);
        settle(&mut builder, (chunk.x, chunk.z)).await;
        told.places(&mut builder, (on, FLOOR, 2)).await;
    }
    assert_eq!(builder.stats.teleports_confirmed, 1);
    let stands = [(-24.5, 0.5), (-56.5, 0.5), (88.5, 0.5)];
    told.audit(&process.address, "Auditor", &stands).await;
    // Four regions were given out, and no other.
    let assigned = process.lines_with("a region was assigned");
    for region in 0..4 {
        let field = format!("region={region}");
        let of_it = assigned
            .iter()
            .filter(|line| line.split_whitespace().any(|word| word == field));
        assert_eq!(of_it.count(), 1, "{field}:\n{}", assigned.join("\n"));
    }
    assert_eq!(assigned.len(), 4, "{}", assigned.join("\n"));
}

// Sections 2.1 and 8.
//
/// Negative coordinates are allowed in `--pin` (section 2.1). Written with an equals
/// sign, they are taken.
#[tokio::test]
async fn the_single_process_takes_negative_pins_that_are_written_with_an_equals_sign() {
    if a_repetition() {
        return;
    }
    four_regions_are_pinned_at_negative_and_positive_cuts(&["--pin=-2,0,5"]).await;
}

// Sections 2.1 and 8, and T6 of section 9.3, which writes it so.
//
// A finding. The sequence: `clustine --pin -2,0,5`, the flag and its coordinates as
// two arguments, which is how section 8 writes the flag (`[--pin X[,X…]]`) and how
// T6 writes this very list ("`--pin 4`, `--pin 0,4`, `--pin -2,0,5` start a store
// with two, three and four pinned regions"). What the record says: section 2.1,
// "chunk x coordinates, ascending, no two alike, negative ones allowed", and section
// 8, "`--pin` is section 2.1's, for the store and for the single process". What
// happened: the process ends with exit code 2 and `error: unexpected argument '-2'
// found`, as the command line takes a value that begins with a minus for a flag. So
// does `clustine worldstore --pin -2,0,5`, which is T6's own case, and so do
// `--boundaries -2,0,5` on all three commands. `--pin=-2,0,5` is taken (the test
// above). A list whose first coordinate is negative is the only one this meets.
#[tokio::test]
async fn the_single_process_takes_negative_pins_as_the_record_writes_them() {
    if a_repetition() {
        return;
    }
    four_regions_are_pinned_at_negative_and_positive_cuts(&["--pin", "-2,0,5"]).await;
}

// Section 6.5: what the single process prints is what its parts print, in one log,
// in the words they have in a cluster. No scenario of section 9.5 has it.
//
/// A single process that reshapes by itself, at distances that fit its view
/// distance: somebody walks away and is split off, comes back far enough to be
/// merged by the distances, walks away again, and then everybody leaves, so that
/// the part is absorbed. The log has every line that section 6.5 lists.
#[tokio::test]
async fn the_single_process_logs_what_the_parts_of_a_cluster_log() {
    if a_repetition() {
        return;
    }
    // Regions are merged before their players see each other's land: 9 is twice
    // the view distance and 3 (section 3.5).
    let view = VIEW_DISTANCE.to_string();
    let flags = [
        "--view-distance",
        &view,
        "--reshape",
        "by-itself",
        "--merge-distance",
        "9",
        "--split-distance",
        "11",
        "--rest-seconds",
        "1",
    ];
    let process = Process::start(&flags).await;
    let said = |words: &str| process.lines_with(words).len();
    let mut told = Told::default();
    let mut stays = join(&process.address, "Stays").await;
    let mut rover = join(&process.address, "Rover").await;
    told.places(&mut stays, plot_at((0, 0), 0)).await;
    for line in [
        "listening",
        "a region was assigned",
        "the routing table changed",
        "given a region",
        "running a region",
        "linked to a region",
    ] {
        assert!(
            said(line) > 0,
            "the log has no `{line}`:\n{}",
            process.log()
        );
    }

    // Twelve chunks out: split off.
    let (out, near) = ((200.5, 0.5), (152.5, 0.5));
    rover.walk_to(out.0, out.1, 2.0).await.unwrap();
    for line in [
        "a split is begun by itself",
        "a worker says what came of a split",
        "the split has ended; opening the new region",
        "a region stood still for a merge or a split",
    ] {
        let what = format!("the process logged `{line}`");
        until(&what, &mut [&mut stays, &mut rover], || said(line) > 0).await;
    }
    settle(&mut rover, (12, 0)).await;
    told.places(&mut rover, plot_at((200, 0), 0)).await;

    // Back to nine chunks out, where neither sees the other's land: merged.
    rover.walk_to(near.0, near.1, 2.0).await.unwrap();
    for line in [
        "a merge is begun by the distances",
        "a merge has ended",
        "the merge has ended",
    ] {
        let what = format!("the process logged `{line}`");
        until(&what, &mut [&mut stays, &mut rover], || said(line) > 0).await;
    }
    settle(&mut rover, (9, 0)).await;
    told.places(&mut rover, plot_at((152, 0), 0)).await;

    // Out again, split off again, and then nobody is left anywhere.
    rover.walk_to(out.0, out.1, 2.0).await.unwrap();
    let what = "the process logged a second split";
    let twice = || said("a worker says what came of a split") > 1;
    until(what, &mut [&mut stays, &mut rover], twice).await;
    settle(&mut rover, (12, 0)).await;
    told.places(&mut rover, plot_at((200, 0), 1)).await;
    for bot in [&stays, &rover] {
        assert_eq!(bot.stats.teleports_confirmed, 1);
        acknowledged_once(bot);
    }
    let merges = said("a merge has ended");
    drop((stays, rover));
    process.says("an absorption is begun by itself").await;
    until("the absorption had ended", &mut [], || {
        said("a merge has ended") > merges
    })
    .await;

    // The auditor goes no further than the split distance allows.
    told.audit(&process.address, "Auditor", &[(168.5, 0.5)])
        .await;
    for gave_up in [
        "the lease of a worker ran out",
        "a region was taken from its owner",
    ] {
        assert_eq!(said(gave_up), 0, "{}", process.log());
    }
}

// P9, as far as a `Server` lets a test go: it has no way to hold back what its store
// answers, so which ticks the old runner had applied and not had confirmed when it
// was fenced is as it falls. What this shows is what the record says `takeover.rs`
// shows then, on a home region that is pinned to nothing: a takeover loses and
// doubles nothing, whichever order it met. The scenario itself, with the answers
// held back, is with the worker's loop (`src/cluster/worker.rs`).
//
/// The home region is taken over, again and again, in the middle of what a player
/// does to blocks, over a link that serialises. Every action is of a pair whose
/// second undoes or covers the first, so that one that was done a second time, or
/// behind its successor, shows: a stone that is placed and then broken, and a block
/// of the floor that is broken and then filled with stone. Every action is
/// acknowledged once, every block ends as the pairs leave it, for the player, for
/// whoever joins, and for the next server on that directory.
#[tokio::test]
async fn nothing_is_lost_or_done_twice_when_the_home_region_is_taken_over_in_the_middle_of_what_a_player_does()
 {
    if a_repetition() {
        return;
    }
    for serialise_link in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let on_disk = || Config {
            serialise_link,
            ..open(Some(directory.path()))
        };
        let (mut server, address) = start(on_disk()).await;
        let mut digger = join(&address, "Digger").await;
        let entity = digger.info.login.entity_id;
        let began = epoch(&server, HOME);

        // Five rounds of seven columns, all within reach; the takeover falls behind
        // another of the four actions on a column in each.
        let mut expected = Told::default();
        let mut last = 0;
        for round in 0..5 {
            for step in 0..7 {
                let (x, z) = (step - 3, round - 2);
                if (x, z) == (0, 0) {
                    // Where the digger stands.
                    continue;
                }
                let actions = [
                    (true, (x, FLOOR, z)),
                    (false, (x, FLOOR + 1, z)),
                    (false, (x, FLOOR, z)),
                    (true, (x, FLOOR - 1, z)),
                ];
                for (number, (places, at)) in actions.into_iter().enumerate() {
                    last = if places {
                        digger.use_item_on(at.0, at.1, at.2, face::TOP).await
                    } else {
                        digger.dig(at.0, at.1, at.2).await
                    }
                    .unwrap();
                    if step == 2 && number == round as usize % 4 {
                        server.take_over(HOME).await.unwrap();
                    }
                }
                expected.0.insert((x, FLOOR + 1, z), AIR);
                expected.0.insert((x, FLOOR, z), STONE);
            }
        }
        acknowledged(&mut digger, last).await;
        for (at, state) in &expected.0 {
            let (x, y, z) = *at;
            digger
                .wait_until(PATIENCE, |bot| bot.block_at(x, y, z).unwrap() == *state)
                .await
                .unwrap_or_else(|error| {
                    panic!(
                        "{error}: the block at {at:?} is {:?}, not {state:?}",
                        digger.block_at(x, y, z)
                    )
                });
        }
        acknowledged_once(&digger);
        assert_eq!(digger.acknowledged_sequence, last);
        assert_eq!(digger.info.login.entity_id, entity);
        assert_eq!(digger.stats.teleports_confirmed, 1);
        assert!(
            epoch(&server, HOME) >= began + 5,
            "five takeovers, five epochs"
        );

        // What the world store has is what the player was told.
        expected.audit(&address, "Auditor", &[]).await;
        stop(server).await;
        drop(digger);
        let (server, address) = start(on_disk()).await;
        expected.audit(&address, "Auditor", &[]).await;
        stop(server).await;
    }
}

/// How long the process of P10 is stopped where it is. **This is time that passes,
/// and it is what the scenario is**: more than two leases of five seconds, and less
/// than the fifteen seconds after which the edge's next keep-alive would be due.
const HELD_UP: Duration = Duration::from_secs(12);

/// How long the bots of P10 go on after the process has woken before its log is
/// read, where nothing shows that a coordinator has looked at its leases: two of
/// the ticks of a coordinator that reshapes by hand, a lease's quarter each, and a
/// little. A coordinator that gives a silent worker up does so at its first tick,
/// and nothing can be waited for that says it did not.
const AFTER_WAKING: Duration = Duration::from_secs(3);

/// Fails if the log of `process` has what a coordinator says that gives a worker up
/// for silence, or has region 0 assigned a second time.
fn kept_its_regions(process: &Process) {
    let log = process.log();
    for gave_up in [
        "the lease of a worker ran out",
        "a region was taken from its owner",
    ] {
        assert!(
            !log.contains(gave_up),
            "the process logged `{gave_up}`:\n{log}"
        );
    }
    let assigned = log
        .lines()
        .filter(|line| line.contains("a region was assigned"))
        .filter(|line| line.split_whitespace().any(|field| field == "region=0"))
        .count();
    assert_eq!(assigned, 1, "region 0 was assigned once:\n{log}");
}

/// Stops `process` where it is for [`HELD_UP`], and wakes it.
async fn hold_up(process: &Process) {
    process.signal("-STOP").await;
    tokio::time::sleep(HELD_UP).await;
    process.signal("-CONT").await;
}

// P10.
//
/// The single process as a process of its own, with somebody connected and playing,
/// is stopped where it is for twelve seconds and woken. Its log has nothing of a
/// worker that was given up, and region 0 assigned once; the player is not
/// disconnected and has a new action acknowledged. As a server is started when it
/// is told nothing of reshaping.
#[tokio::test]
async fn a_process_that_was_held_up_for_twelve_seconds_keeps_its_region_and_its_player() {
    if a_repetition() {
        return;
    }
    let view = VIEW_DISTANCE.to_string();
    let process = Process::start(&["--view-distance", &view]).await;
    let mut told = Told::default();
    let mut player = join(&process.address, "Player").await;
    let entity = player.info.login.entity_id;
    told.places(&mut player, plot_at((0, 0), 0)).await;
    told.digs(&mut player, (3, FLOOR, 3)).await;

    hold_up(&process).await;

    told.places(&mut player, plot_at((0, 0), 1)).await;
    if let Err(error) = player.idle(AFTER_WAKING).await {
        panic!("the player was disconnected after the process woke: {error:#}");
    }
    told.places(&mut player, plot_at((0, 0), 2)).await;
    kept_its_regions(&process);
    assert_eq!(player.info.login.entity_id, entity);
    assert_eq!(player.stats.teleports_confirmed, 1);
    acknowledged_once(&player);
    told.audit(&process.address, "Auditor", &[]).await;
    kept_its_regions(&process);
}

// P10.
//
/// The same of a process that reshapes by itself, which looks at its regions four
/// times a second and, had it given its worker up, would begin nothing with that
/// worker's regions for half a minute. Here there is something to wait for: when
/// the process has woken, a second player walks beyond the split distance, and the
/// log has a split begun for them. By then the coordinator has looked many times.
#[tokio::test]
async fn a_process_that_reshapes_by_itself_and_was_held_up_keeps_its_region_and_splits_as_before() {
    if a_repetition() {
        return;
    }
    let view = VIEW_DISTANCE.to_string();
    let flags = [
        "--view-distance",
        &view,
        "--reshape",
        "by-itself",
        "--merge-distance",
        "3",
        "--split-distance",
        "5",
        "--rest-seconds",
        "1",
    ];
    let process = Process::start(&flags).await;
    process.says(BY_ITSELF).await;
    let stands = (ROVER.0.floor() as i32, ROVER.1.floor() as i32);
    let mut told = Told::default();
    let mut player = join(&process.address, "Player").await;
    let mut rover = join(&process.address, "Rover").await;
    told.places(&mut player, plot_at((0, 0), 0)).await;
    told.places(&mut rover, (-2, FLOOR, -2)).await;

    hold_up(&process).await;

    told.places(&mut player, plot_at((0, 0), 1)).await;
    told.places(&mut rover, (-2, FLOOR, -3)).await;
    assert!(process.lines_with("a split is begun by itself").is_empty());
    rover.walk_to(ROVER.0, ROVER.1, 2.0).await.unwrap();
    settle(&mut rover, (ROVER_CHUNK.x, ROVER_CHUNK.z)).await;
    let mut both = [&mut player, &mut rover];
    until("a split was begun for the second player", &mut both, || {
        !process.lines_with("a split is begun by itself").is_empty()
    })
    .await;
    // And was made: the worker says what came of it, and both play on.
    until("the worker said what came of the split", &mut both, || {
        !process
            .lines_with("a worker says what came of a split")
            .is_empty()
    })
    .await;
    let came = process.lines_with("a worker says what came of a split");
    assert!(came[0].contains("outcome=Ok("), "{}", came[0]);
    told.places(&mut rover, plot_at(stands, 0)).await;
    told.places(&mut player, plot_at((0, 0), 2)).await;

    kept_its_regions(&process);
    for bot in [&player, &rover] {
        assert_eq!(bot.stats.teleports_confirmed, 1);
        acknowledged_once(bot);
    }
    told.audit(&process.address, "Auditor", &[TOWARDS_ROVER])
        .await;
    kept_its_regions(&process);
}
