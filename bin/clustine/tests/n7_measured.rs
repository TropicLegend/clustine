//! What is measured for N7n (`docs/groundwork/n7-plan.md`, section 11.5: "measured:
//! merges and splits with animals; the round's two syncs on the real disk";
//! `docs/groundwork/n7l-plan.md`, section 7), beside N7f's herds (`herds.rs`):
//!
//! - **Merges and splits with herds**: two regions that follow their players (the
//!   stripes of `follows.rs`: a line at block x = 64, merged at 3 chunks, split beyond
//!   5), a player by each of two herds of 100 (cows, sheep, pigs, frogs, axolotls and
//!   goats, the last three with brains). The one walks to the other until their
//!   regions merge, away until they split, and back until they merge again. For each:
//!   the coordinator's time from its line that it began to its line that it ended, how
//!   long each region stood still by its worker, the longest tick about it, and that
//!   every animal is then shown once to both players.
//! - **Monsters and villagers by day**: 17 zombies, 17 skeletons and 16 villagers in a
//!   pen at a pinned line in the morning, the undead burning and the villagers
//!   panicking, for a minute: the world's growth and the longest tick, as `herds.rs`
//!   says them of its pen.
//! - **A new world's first chunks with their animals**: a player joins a new world of a
//!   seed as soon as it is served, and is shown the animals the chunks about it were
//!   made with: how long from the join to the view whole, and to every one of those
//!   animals shown. And beside it, in this process, the world store's first save of
//!   such chunks, which takes two rounds of syncs (their entities' files, then their
//!   blocks' files; `services/worldstore/src/chunks.rs`), against a save of the same
//!   chunks once stored, which takes one.
//! - **Interactions at a line merge regions**: in a world whose regions follow their
//!   players without pins, a player at the line between two regions feeds a cow on its
//!   side and then one on the other side, the two penned on either side of the line
//!   close enough to breed. The two regions merge for the interaction across the line
//!   (`entities_at_line`, `docs/groundwork/n7-plan.md`, section 9.7 item 1): how long
//!   after the feeding the coordinator began the merge and ended it, and that the cows
//!   breed once.
//!
//! What each asserts is only what must hold: no tick and no standstill longer than the
//! 5 s the tests of regions that follow their players allow a pause
//! (`common::wandering::LONGEST_PAUSE`), and nothing lost or shown twice.
//!
//! Where the world lies decides what is measured: `TMPDIR` on memory (`/dev/shm/…`) for
//! the logic, on the real disk for the disk, which on the owner's machine has a cache in
//! memory in front of it (CLAUDE.md, "Checking work"). Say which a number is from. The
//! first save in this process can be measured with every sync made slower, to stand in
//! for the disk without that cache: `CLUSTINE_SLOWER_SYNCS_MS=22`.
//!
//! ```text
//! CLUSTINE_STREAM_TALLY=1 cargo test --release -p clustine --test n7_measured -- \
//!     --ignored --nocapture --test-threads 1
//! ```
//!
//! `CLUSTINE_N7N_SEED` sets the seed of the new world (3 unless said, the seed of
//! N7l's flight). The animals are counted against the chunks this process makes of it
//! without the structure templates, as the processes started here make it: they find
//! no jar of the game's from where the tests run.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clustine_botswarm::Bot;
use clustine_data::{entity_types, items};
use clustine_protocol::item::ItemStack;
use clustine_protocol::packets::play::face;
use clustine_region::RegionId;
use clustine_rpc::{Parts, RegionHello, RegionList, StoreReply, StoreRequest};
use clustine_world::{ChunkGenerator, ChunkPos};
use clustine_worldgen::scheduler::{Options, Scheduler};
use clustine_worldgen::{FeatureStages, TerrainGenerator};
use clustine_worldstore::{Pins, Store, disk_figures};
use tokio::task::JoinHandle;
use uuid::Uuid;

use common::processes::Cluster;
use common::wandering::{Begun, LONGEST_PAUSE, absorbed_by, bounds, living, number, time_of};
use common::{following, view_area};

const PATIENCE: Duration = Duration::from_secs(60);

/// How often a state that is waited for is looked at.
const LOOK: Duration = Duration::from_millis(20);

/// What a worker says of ticks that took too long.
const SLOW: &str = "ticks took longer than the 50 ms a tick has";

/// What a worker says of how long a region did not tick for a merge or a split.
const STOOD_STILL: &str = "a region stood still for a merge or a split";

/// What the world store says of its tally.
const TALLY: &str = "the store answered: ";

/// The flat world's grass, which players stand on.
const GRASS: i32 = -61;

/// How fast the bots walk, in blocks a tick: a brisk walk, as in `herds.rs`.
const BRISK: f64 = 0.6;

/// The bytes of every file under `directory`.
fn bytes_under(directory: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                bytes_under(&path)
            } else {
                entry.metadata().map_or(0, |meta| meta.len())
            }
        })
        .sum()
}

/// How many entities of type `kind` the bot is shown within `radius` blocks of `(x, z)`.
fn shown_near(bot: &Bot, kind: i32, (x, z): (f64, f64), radius: f64) -> usize {
    bot.entities
        .values()
        .filter(|entity| {
            entity.kind == kind && (entity.position.0 - x).hypot(entity.position.2 - z) <= radius
        })
        .count()
}

/// How many entities of type `kind` the bot is shown.
fn shown(bot: &Bot, kind: i32) -> usize {
    bot.entities
        .values()
        .filter(|entity| entity.kind == kind)
        .count()
}

/// The time of day now, in seconds since 1970, as the logs have it.
fn now() -> f64 {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    now.as_secs_f64()
}

/// A bot that has joined `cluster` and been shown the chunks about the chunk players
/// enter in.
async fn joined(cluster: &Cluster, name: &str) -> Bot {
    let mut bot = Bot::join(&cluster.edge.0, name).await.unwrap();
    let around = view_area((0, 0), cluster.view_distance).len();
    bot.wait_for_chunks(around, PATIENCE).await.unwrap();
    bot
}

/// Walks the bot to `(x, z)` on the grass and waits until it is shown the grass about
/// it, so that what it clicks there can be judged.
async fn go_to(bot: &mut Bot, x: f64, z: f64) {
    bot.walk_to(x, z, BRISK).await.unwrap();
    let (bx, bz) = (x.floor() as i32, z.floor() as i32);
    bot.wait_until(PATIENCE, |bot| {
        bot.block_at(bx, GRASS, bz)
            .is_ok_and(|state| state.is_some_and(|state| state != 0))
    })
    .await
    .unwrap();
}

/// Holds `item` in the first hotbar slot.
async fn hold(bot: &mut Bot, item: i32, count: i32) {
    bot.put_in_hotbar(0, ItemStack::new(item, count))
        .await
        .unwrap();
    bot.select_slot(0).await.unwrap();
}

/// Uses the spawn egg `egg` `count` times on the grass of `cells` in turn, and waits
/// after each until the bot is shown one more animal of type `kind` within 12 blocks of
/// the first cell. Another bot may lay animals of the same type at the same time, 12
/// blocks farther away at the least.
async fn lay_on(bot: &mut Bot, egg: i32, kind: i32, count: usize, cells: &[(i32, i32)]) {
    hold(bot, egg, 1).await;
    let near = (f64::from(cells[0].0) + 0.5, f64::from(cells[0].1) + 0.5);
    let before = shown_near(bot, kind, near, 12.0);
    for laid in 0..count {
        let (x, z) = cells[laid % cells.len()];
        bot.use_item_on(x, GRASS, z, face::TOP).await.unwrap();
        bot.wait_until(PATIENCE, |bot| {
            shown_near(bot, kind, near, 12.0) > before + laid
        })
        .await
        .unwrap_or_else(|error| panic!("{error}: the animal {laid} of type {kind} did not come"));
    }
}

/// The 25 cells of grass within two blocks of where the bot stands.
fn about(bot: &Bot) -> Vec<(i32, i32)> {
    let (x, z) = (bot.location.0.floor() as i32, bot.location.2.floor() as i32);
    (0..25)
        .map(|spot| (x + spot % 5 - 2, z + spot / 5 - 2))
        .collect()
}

/// Puts a fence on the grass at `(x, z)` and waits until the bot is shown it, trying
/// again while something stands in the way.
async fn post(bot: &mut Bot, (x, z): (i32, i32)) {
    for _ in 0..20 {
        bot.use_item_on(x, GRASS, z, face::TOP).await.unwrap();
        let placed = bot
            .wait_until(Duration::from_secs(2), |bot| {
                bot.block_at(x, GRASS + 1, z)
                    .is_ok_and(|state| state.is_some_and(|state| state != 0))
            })
            .await;
        if placed.is_ok() {
            return;
        }
    }
    panic!("no fence at {x}, {z}");
}

/// Fences the square from `(x0, z0)` to `(x1, z1)` on the grass, walking about inside it,
/// as `herds.rs` does.
async fn fence(bot: &mut Bot, (x0, z0): (i32, i32), (x1, z1): (i32, i32)) {
    bot.put_in_hotbar(1, ItemStack::new(items::OAK_FENCE, 64))
        .await
        .unwrap();
    bot.select_slot(1).await.unwrap();
    let mut ring = Vec::new();
    for x in x0..=x1 {
        ring.push((x, z0));
        ring.push((x, z1));
    }
    for z in z0 + 1..z1 {
        ring.push((x0, z));
        ring.push((x1, z));
    }
    for (x, z) in ring {
        // From two blocks inside the ring, within reach.
        let inside_x = f64::from(x.clamp(x0 + 2, x1 - 2)) + 0.5;
        let inside_z = f64::from(z.clamp(z0 + 2, z1 - 2)) + 0.5;
        if (bot.location.0 - inside_x).hypot(bot.location.2 - inside_z) > 0.5 {
            go_to(bot, inside_x, inside_z).await;
        }
        post(bot, (x, z)).await;
    }
}

/// Where each log is now, to read what comes after.
fn marks(cluster: &Cluster) -> BTreeMap<String, usize> {
    cluster
        .names()
        .into_iter()
        .map(|name| {
            let length = cluster.log(&name).len();
            (name, length)
        })
        .collect()
}

/// What `name` logged since `marks`.
fn since(cluster: &Cluster, marks: &BTreeMap<String, usize>, name: &str) -> String {
    let log = cluster.log(name);
    let from = marks.get(name).copied().unwrap_or(0);
    log.get(from..).unwrap_or_default().to_owned()
}

/// The reports of slow ticks every worker made since `marks`, and the longest tick
/// they name.
fn slow_ticks(cluster: &Cluster, marks: &BTreeMap<String, usize>) -> (Vec<String>, f64) {
    let mut lines = Vec::new();
    let mut longest: f64 = 0.0;
    for name in cluster.names() {
        for line in since(cluster, marks, &name).lines() {
            if line.contains(SLOW) {
                longest = longest.max(number::<f64>(line, "longest_ms").unwrap_or(0.0));
                lines.push(format!("{name}: {line}"));
            }
        }
    }
    (lines, longest)
}

/// Says the longest tick since `marks`, and fails if it was longer than a pause may be.
fn say_and_bound_the_ticks(cluster: &Cluster, marks: &BTreeMap<String, usize>) {
    let (lines, longest) = slow_ticks(cluster, marks);
    for line in &lines {
        println!("    {line}");
    }
    if lines.is_empty() {
        println!("  no tick took longer than 50 ms");
    } else {
        println!(
            "  {} reports of slow ticks; the longest tick took {longest:.0} ms",
            lines.len()
        );
    }
    assert!(
        longest <= LONGEST_PAUSE.as_secs_f64() * 1000.0,
        "a tick took {longest:.0} ms, longer than the {LONGEST_PAUSE:?} a pause may be"
    );
}

/// What the world store's tally said in its log since `marks`, summed by kind: the
/// count and the bytes.
fn tallied(cluster: &Cluster, marks: &BTreeMap<String, usize>) -> BTreeMap<String, (u64, u64)> {
    let mut kinds: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for line in since(cluster, marks, "worldstore").lines() {
        let Some((_, said)) = line.split_once(TALLY) else {
            continue;
        };
        let said = said.split(" seconds=").next().unwrap_or_default();
        for part in said.split(", ") {
            let words: Vec<&str> = part.split_whitespace().collect();
            if let [kind, count, "in", bytes, "B"] = words[..]
                && let (Ok(count), Ok(bytes)) = (count.parse::<u64>(), bytes.parse::<u64>())
            {
                let entry = kinds.entry(kind.to_owned()).or_default();
                entry.0 += count;
                entry.1 += bytes;
            }
        }
    }
    kinds
}

/// Has a bot answer what it is sent in a task of its own until it is taken back, so
/// that another can act meanwhile.
struct Answering {
    stop: Arc<AtomicBool>,
    task: JoinHandle<Bot>,
}

impl Answering {
    fn start(mut bot: Bot) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        // Each idle is let end, so that no frame is left half read.
        let task = tokio::spawn(async move {
            while !stopping.load(Ordering::SeqCst) {
                bot.idle(Duration::from_millis(50)).await.unwrap();
            }
            bot
        });
        Self { stop, task }
    }

    async fn back(self) -> Bot {
        self.stop.store(true, Ordering::SeqCst);
        self.task.await.unwrap()
    }
}

/// Of `herd`, those the bot is not shown, and those it is shown under more than one id.
fn herd_as_shown(bot: &Bot, herd: &BTreeSet<Uuid>) -> (Vec<Uuid>, Vec<Uuid>) {
    let mut ids: BTreeMap<Uuid, usize> = BTreeMap::new();
    for entity in bot.entities.values() {
        if herd.contains(&entity.uuid) {
            *ids.entry(entity.uuid).or_default() += 1;
        }
    }
    let missing = herd.iter().filter(|uuid| !ids.contains_key(uuid)).copied();
    let twice = ids
        .iter()
        .filter(|(_, count)| **count > 1)
        .map(|(uuid, _)| *uuid);
    (missing.collect(), twice.collect())
}

/// Waits until the bot is shown every one of `herd` once, and says how long that took.
async fn shown_whole(bot: &mut Bot, herd: &BTreeSet<Uuid>, what: &str) -> Duration {
    let began = Instant::now();
    let whole = bot
        .wait_until(PATIENCE, |bot| {
            let (missing, twice) = herd_as_shown(bot, herd);
            missing.is_empty() && twice.is_empty()
        })
        .await;
    if whole.is_err() {
        let (missing, twice) = herd_as_shown(bot, herd);
        let gone: Vec<(Uuid, i32, (f64, f64, f64))> = bot
            .vanished
            .iter()
            .filter(|entity| missing.contains(&entity.uuid))
            .map(|entity| (entity.uuid, entity.kind, entity.position))
            .collect();
        panic!(
            "{what}: of {} animals, {} are not shown ({missing:?}; taken away as {gone:?}) and \
             {} are shown twice ({twice:?})",
            herd.len(),
            missing.len(),
            twice.len()
        );
    }
    began.elapsed()
}

/// The kinds of a herd of 100, each with its egg and how many, laid in the open: 90 of
/// them. Goats are penned apart, as their rams would kill the others.
const IN_THE_OPEN: [(i32, i32, usize); 5] = [
    (items::COW_SPAWN_EGG, entity_types::COW, 25),
    (items::SHEEP_SPAWN_EGG, entity_types::SHEEP, 20),
    (items::PIG_SPAWN_EGG, entity_types::PIG, 20),
    (items::FROG_SPAWN_EGG, entity_types::FROG, 15),
    // Last: an axolotl dries out on land after five minutes.
    (items::AXOLOTL_SPAWN_EGG, entity_types::AXOLOTL, 10),
];
const GOATS: usize = 10;

/// Lays a herd of 100 with its open part about `(x, z)`, a block on the grass, and its
/// goats in a pen of 3 × 3 sixteen blocks south of it (its fence from `x - 2` to
/// `x + 2` and from `z + 16` to `z + 20`), built and filled from outside. Returns the
/// herd's UUIDs.
async fn herd(bot: &mut Bot, (x, z): (i32, i32)) -> BTreeSet<Uuid> {
    let (x0, z0) = (x - 2, z + 16);
    let south = (f64::from(x) + 0.5, f64::from(z0) - 1.5);
    let north = (f64::from(x) + 0.5, f64::from(z0) + 5.5);
    let round_west = f64::from(x0) - 1.5;
    hold(bot, items::OAK_FENCE, 64).await;
    go_to(bot, f64::from(x) + 0.5, f64::from(z) + 0.5).await;
    go_to(bot, south.0, south.1).await;
    // From the south: the south side and the two rows of the sides nearest it.
    for px in x0..=x0 + 4 {
        post(bot, (px, z0)).await;
    }
    for pz in z0 + 1..=z0 + 2 {
        post(bot, (x0, pz)).await;
        post(bot, (x0 + 4, pz)).await;
    }
    // Round the west end to the north: the rest.
    go_to(bot, round_west, south.1).await;
    go_to(bot, round_west, north.1).await;
    go_to(bot, north.0, north.1).await;
    for px in x0..=x0 + 4 {
        post(bot, (px, z0 + 4)).await;
    }
    post(bot, (x0, z0 + 3)).await;
    post(bot, (x0 + 4, z0 + 3)).await;
    let inside: Vec<(i32, i32)> = (0..9)
        .map(|cell| (x - 1 + cell % 3, z0 + 1 + cell / 3))
        .collect();
    lay_on(
        bot,
        items::GOAT_SPAWN_EGG,
        entity_types::GOAT,
        GOATS,
        &inside,
    )
    .await;
    // Back to the open, round the west end.
    go_to(bot, round_west, north.1).await;
    go_to(bot, round_west, south.1).await;
    go_to(bot, f64::from(x) + 0.5, f64::from(z) + 0.5).await;
    let cells = about(bot);
    for (egg, kind, count) in IN_THE_OPEN {
        lay_on(bot, egg, kind, count, &cells).await;
    }
    let kinds: Vec<i32> = IN_THE_OPEN
        .iter()
        .map(|(_, kind, _)| *kind)
        .chain([entity_types::GOAT])
        .collect();
    let centre = (f64::from(x) + 0.5, f64::from(z) + 8.5);
    let herd: BTreeSet<Uuid> = bot
        .entities
        .values()
        .filter(|entity| {
            kinds.contains(&entity.kind)
                && (entity.position.0 - centre.0).hypot(entity.position.2 - centre.1) < 32.0
        })
        .map(|entity| entity.uuid)
        .collect();
    assert_eq!(herd.len(), 100, "the herd about {x}, {z} is 100");
    herd
}

/// A merge or a split of the measurement of herds.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Reshape {
    Merge,
    Split,
}

/// The coordinator's line that ends a reshape of this kind.
fn ends(reshape: Reshape, line: &str) -> bool {
    let ended = match reshape {
        Reshape::Merge => "a merge has ended",
        Reshape::Split => "a worker says what came of a split",
    };
    line.contains(ended) && line.contains("outcome=Ok")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement; run it by name with --release --ignored --nocapture"]
async fn measured_merges_and_splits_of_regions_that_follow_two_players_by_herds() {
    let directory = tempfile::Builder::new()
        .prefix("n7n-herds")
        .tempdir()
        .unwrap();
    // The stripes of `follows.rs`, with its coordinator.
    let mut cluster = Cluster::new(directory.path(), 2, "4").await;
    cluster.coordinator_arguments = [
        "--reshape",
        "by-itself",
        "--merge-distance",
        "3",
        "--split-distance",
        "5",
        "--rest-seconds",
        "5",
    ]
    .map(str::to_owned)
    .to_vec();
    cluster.view_distance = 8;
    cluster.start().await;
    let mut keeper = joined(&cluster, "Keeper").await;
    let mut drover = joined(&cluster, "Drover").await;

    // The drover goes to chunk 6, east of the line, while the keeper stays in chunk 0;
    // each lays a herd about where it stands, both at the same time.
    let answering = Answering::start(keeper);
    go_to(&mut drover, 104.5, -11.5).await;
    keeper = answering.back().await;
    let (west, east) = tokio::join!(herd(&mut keeper, (8, -12)), herd(&mut drover, (104, -12)));
    let laid = Instant::now();
    let animals: BTreeSet<Uuid> = west.union(&east).copied().collect();
    assert_eq!(animals.len(), 200);
    // The keeper to its lane in chunk 0, the drover to its place in chunk 6.
    let answering = Answering::start(drover);
    keeper.walk_to(8.5, 0.5, BRISK).await.unwrap();
    drover = answering.back().await;
    let answering = Answering::start(keeper);
    drover.walk_to(104.5, 0.5, BRISK).await.unwrap();
    keeper = answering.back().await;
    let list = cluster.regions().await.unwrap();
    assert_eq!(
        living(&list),
        [0, 1],
        "the stripes are as they began: {list:?}"
    );
    shown_whole(&mut keeper, &animals, "before the merges").await;
    shown_whole(&mut drover, &animals, "before the merges").await;

    // In chunk 3 the keeper is 3 from the drover and the regions merge; back in chunk 0
    // it is 6 from it and they split; in chunk 3 again they merge again.
    let steps = [
        (Reshape::Merge, 56.5, 1),
        (Reshape::Split, 8.5, 2),
        (Reshape::Merge, 56.5, 1),
    ];
    for (reshape, to, regions) in steps {
        let what = match reshape {
            Reshape::Merge => "a merge as the keeper walked to chunk 3",
            Reshape::Split => "a split as the keeper walked back to chunk 0",
        };
        let marks = marks(&cluster);
        let answering = Answering::start(drover);
        keeper.walk_to(to, 0.5, BRISK).await.unwrap();
        let walked = Instant::now();
        let deadline = walked + PATIENCE;
        let list: RegionList = loop {
            if let Ok(list) = cluster.regions().await
                && living(&list).len() == regions
            {
                break list;
            }
            assert!(Instant::now() < deadline, "{what}: the list never had it");
            keeper.idle(LOOK).await.unwrap();
        };
        let listed = walked.elapsed();
        // The coordinator's line that it ended, and the line of each worker that its
        // region ticked again.
        let (began, ended) = loop {
            let log = since(&cluster, &marks, "coordinator");
            let began = log.lines().find_map(|line| {
                Begun::read(line)
                    .filter(Begun::reshapes)
                    .and_then(|_| time_of(line).map(|at| (at, line.to_owned())))
            });
            let ended = log
                .lines()
                .find(|line| ends(reshape, line))
                .and_then(time_of);
            let stood = cluster
                .names()
                .iter()
                .any(|name| since(&cluster, &marks, name).contains(STOOD_STILL));
            if let (Some(began), Some(ended), true) = (began, ended, stood) {
                break (began, ended);
            }
            assert!(
                Instant::now() < deadline,
                "{what}: the coordinator's log never said it began and ended:\n{log}"
            );
            keeper.idle(LOOK).await.unwrap();
        };
        let after = Instant::now();
        shown_whole(&mut keeper, &animals, what).await;
        let keeper_shown = after.elapsed();
        drover = answering.back().await;
        shown_whole(&mut drover, &animals, what).await;
        let drover_shown = after.elapsed();
        println!("{what}: {list:?}");
        println!(
            "  begun by the coordinator {}; ended {:.0} ms after it; the list had it {:.2} s \
             after the walk",
            began.1.split(" INFO ").nth(1).unwrap_or(&began.1).trim(),
            (ended - began.0) * 1000.0,
            listed.as_secs_f64()
        );
        let mut longest_stood: u64 = 0;
        for name in cluster.names() {
            for line in since(&cluster, &marks, &name).lines() {
                if line.contains(STOOD_STILL) {
                    let milliseconds = number::<u64>(line, "milliseconds").unwrap_or(0);
                    longest_stood = longest_stood.max(milliseconds);
                    println!(
                        "  {name}: region {} stood still {milliseconds} ms with {} players and \
                         {} chunks",
                        number::<u32>(line, "region").unwrap_or(u32::MAX),
                        number::<u64>(line, "players").unwrap_or(0),
                        number::<u64>(line, "held").unwrap_or(0),
                    );
                }
            }
        }
        println!(
            "  every one of the 200 animals shown once to the keeper {:.2} s and to the drover \
             {:.2} s after the coordinator's lines; {:.0} s since they were laid",
            keeper_shown.as_secs_f64(),
            drover_shown.as_secs_f64(),
            laid.elapsed().as_secs_f64()
        );
        say_and_bound_the_ticks(&cluster, &marks);
        assert!(
            Duration::from_millis(longest_stood) <= LONGEST_PAUSE,
            "{what}: a region stood still for {longest_stood} ms"
        );
    }
    cluster.terminate().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement; run it by name with --release --ignored --nocapture"]
async fn measured_zombies_skeletons_and_villagers_in_a_pen_by_day() {
    let directory = tempfile::Builder::new()
        .prefix("n7n-monsters")
        .tempdir()
        .unwrap();
    let mut cluster = Cluster::new(directory.path(), 2, "4").await;
    cluster.start().await;
    let mut bot = joined(&cluster, "Warden").await;
    // The pen of `herds.rs`: 9 × 9 inside, from x = 54 to 62 west of the line at 64.
    fence(&mut bot, (53, -5), (63, 5)).await;
    go_to(&mut bot, 58.5, 0.5).await;
    let cells = about(&bot);
    let laying = Instant::now();
    for (egg, kind, count) in [
        (items::ZOMBIE_SPAWN_EGG, entity_types::ZOMBIE, 17),
        (items::SKELETON_SPAWN_EGG, entity_types::SKELETON, 17),
        (items::VILLAGER_SPAWN_EGG, entity_types::VILLAGER, 16),
    ] {
        lay_on(&mut bot, egg, kind, count, &cells).await;
    }
    let laid = laying.elapsed();
    // The warden stands outside, in view of the pen.
    go_to(&mut bot, 58.5, -8.5).await;
    let kinds = [
        ("zombies", entity_types::ZOMBIE),
        ("skeletons", entity_types::SKELETON),
        ("villagers", entity_types::VILLAGER),
        ("zombie villagers", entity_types::ZOMBIE_VILLAGER),
        ("items", entity_types::ITEM),
        ("arrows", entity_types::ARROW),
        ("orbs", entity_types::EXPERIENCE_ORB),
    ];
    let count = |bot: &Bot| {
        kinds
            .iter()
            .map(|(name, kind)| format!("{} {name}", shown(bot, *kind)))
            .collect::<Vec<String>>()
            .join(", ")
    };
    println!(
        "laid in {:.1} s; shown then: {}",
        laid.as_secs_f64(),
        count(&bot)
    );

    let (marks, bytes) = (marks(&cluster), bytes_under(&cluster.world));
    bot.idle(Duration::from_secs(60)).await.unwrap();
    println!(
        "50 zombies, skeletons and villagers in a pen at the line by day for a minute: the \
         world in {} grew by {} KiB",
        cluster.world.display(),
        bytes_under(&cluster.world).saturating_sub(bytes) / 1024
    );
    let kinds_tallied = tallied(&cluster, &marks);
    if kinds_tallied.is_empty() {
        println!("  the store kept no tally: start the test with CLUSTINE_STREAM_TALLY=1");
    }
    let total: u64 = kinds_tallied.values().map(|(_, bytes)| bytes).sum();
    for (kind, (answers, bytes)) in &kinds_tallied {
        println!(
            "  {kind}: {answers} answers, {} KiB, {:.1} % of the bytes",
            bytes / 1024,
            *bytes as f64 * 100.0 / total.max(1) as f64
        );
    }
    println!("  shown after the minute: {}", count(&bot));
    say_and_bound_the_ticks(&cluster, &marks);
    cluster.terminate().await;
}

/// The seed of the new world: `CLUSTINE_N7N_SEED`, or 3.
fn seed() -> i64 {
    std::env::var("CLUSTINE_N7N_SEED")
        .map(|seed| seed.parse().expect("CLUSTINE_N7N_SEED is a number"))
        .unwrap_or(3)
}

/// What makes the chunks of the world of `seed`, as a server of the seed without the
/// structure templates makes them.
fn land(seed: i64) -> Scheduler<FeatureStages> {
    let processors = std::thread::available_parallelism().map_or(2, usize::from);
    Scheduler::new(
        FeatureStages::new(Arc::new(TerrainGenerator::new(
            seed,
            clustine_world::Dimension::Overworld,
        ))),
        Options::for_processors(processors),
    )
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement; run it by name with --release --ignored --nocapture"]
async fn measured_how_soon_a_newcomer_to_a_new_world_is_shown_the_animals_about_it() {
    const VIEW: i32 = 8;
    let seed = seed();
    // The animals the chunks well inside the view are made with, by UUID: those within
    // two chunks of the view's edge, so that none walks out of it before it is shown.
    let (centre, made) = tokio::task::spawn_blocking(move || {
        let generator = land(seed);
        let spawn = generator.spawn();
        let centre = (spawn.x.div_euclid(16), spawn.z.div_euclid(16));
        let inner: Vec<ChunkPos> = view_area(centre, VIEW - 2)
            .into_iter()
            .map(|(x, z)| ChunkPos::new(x, z))
            .collect();
        generator.prepare(&inner);
        let mut made: BTreeMap<Uuid, u16> = BTreeMap::new();
        for position in inner {
            for entity in generator.generate(position).entities() {
                made.insert(entity.uuid, entity.kind);
            }
        }
        (centre, made)
    })
    .await
    .unwrap();
    let mut by_kind: BTreeMap<u16, usize> = BTreeMap::new();
    for kind in made.values() {
        *by_kind.entry(*kind).or_default() += 1;
    }
    println!(
        "seed {seed}: the chunks within {} of the spawn's chunk {centre:?} are made with {} \
         entities, by type {by_kind:?}",
        VIEW - 2,
        made.len()
    );
    assert!(!made.is_empty(), "the seed's spawn has animals about it");
    let animals: BTreeSet<Uuid> = made.keys().copied().collect();

    let directory = tempfile::Builder::new()
        .prefix("n7n-new-world")
        .tempdir()
        .unwrap();
    let mut cluster = Cluster::new(directory.path(), 1, "").await;
    cluster.view_distance = VIEW;
    let told = format!("--seed={seed}");
    cluster.store_arguments = vec![told.clone()];
    cluster.edge_arguments = vec![told];
    let starting = Instant::now();
    cluster.start().await;
    let served = starting.elapsed();
    let joining = Instant::now();
    let mut bot = Bot::join(&cluster.edge.0, "Newcomer").await.unwrap();
    let area = view_area(centre, VIEW);
    bot.wait_until(Duration::from_secs(120), |bot| {
        area.iter().all(|chunk| bot.chunks.contains_key(chunk))
    })
    .await
    .unwrap_or_else(|error| panic!("{error}: {} of {} chunks", bot.chunks.len(), area.len()));
    let whole = joining.elapsed();
    let first_animal = bot.entities.len();
    shown_whole(&mut bot, &animals, "the animals of the new world").await;
    let all_shown = joining.elapsed();
    println!(
        "  the cluster was served {:.2} s after it was started; a player who joined then was \
         shown the view of {VIEW} ({} chunks) whole {:.2} s after joining, with {first_animal} \
         entities shown by then, and every one of the {} animals {:.2} s after joining",
        served.as_secs_f64(),
        area.len(),
        whole.as_secs_f64(),
        animals.len(),
        all_shown.as_secs_f64()
    );
    let marks = BTreeMap::new();
    say_and_bound_the_ticks(&cluster, &marks);
    drop(bot);
    cluster.terminate().await;
}

/// Waits for the store's answers to the loads of `positions`, and returns the chunks.
fn loaded(
    handle: &clustine_worldstore::StoreHandle,
    positions: &[ChunkPos],
) -> BTreeMap<ChunkPos, clustine_world::Chunk> {
    let deadline = Instant::now() + Duration::from_secs(300);
    let mut chunks = BTreeMap::new();
    while chunks.len() < positions.len() {
        match handle.try_reply() {
            Some(StoreReply::Loaded { position, chunk }) => {
                chunks.insert(position, chunk);
            }
            Some(StoreReply::Unreadable { position } | StoreReply::Withdrawn { position }) => {
                panic!("the chunk at {position:?} was not loaded")
            }
            // The clock and the faces on the stream, and the news of a making.
            Some(_) => {}
            None => {
                assert!(Instant::now() < deadline, "the chunks were never loaded");
                assert!(!handle.is_lost(), "the handle was lost");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
    chunks
}

/// What a flush of a handle cost: how long it took, and the file syncs and the syncs of
/// directories the store asked for.
fn flushed(handle: &clustine_worldstore::StoreHandle) -> (Duration, u64, u64) {
    let before = disk_figures();
    let began = Instant::now();
    handle.flush();
    let took = began.elapsed();
    let asked = disk_figures().since(before);
    (took, asked.file_syncs, asked.directory_syncs)
}

/// The middle of `times`, and the worst.
fn middle_and_worst(times: &mut [Duration]) -> (Duration, Duration) {
    times.sort_unstable();
    (times[times.len() / 2], times[times.len() - 1])
}

#[test]
#[ignore = "a measurement; run it by name with --release --ignored --nocapture"]
fn measured_the_first_save_of_chunks_with_their_animals_and_a_save_after_it() {
    let seed = seed();
    let generator = Arc::new(land(seed));
    let spawn = generator.spawn();
    let centre = ChunkPos::new(spawn.x.div_euclid(16), spawn.z.div_euclid(16));
    let directory = tempfile::Builder::new()
        .prefix("n7n-saves")
        .tempdir()
        .unwrap();
    let slower = std::env::var("CLUSTINE_SLOWER_SYNCS_MS").ok().map(|ms| {
        ms.parse::<u64>()
            .expect("CLUSTINE_SLOWER_SYNCS_MS is a number")
    });
    let generating: Arc<dyn ChunkGenerator> = generator.clone();
    let store = match slower {
        Some(ms) => Store::local_divided_slowed(
            directory.path(),
            generating,
            Pins::whole_world(),
            Duration::from_millis(ms),
        ),
        None => Store::local(directory.path(), generating),
    }
    .unwrap();
    let hello = RegionHello {
        region: RegionId(0),
        epoch: 1,
    };
    let (handle, _) = store.open_region(hello).unwrap();
    println!(
        "seed {seed}, the world in {}{}",
        directory.path().display(),
        slower.map_or_else(String::new, |ms| format!(", every sync {ms} ms slower"))
    );

    // The spawn's area, which the store makes ahead as it starts: a first player's view
    // noted and made durable in one sync (the first of the world, which also raises its
    // `meta`).
    let spawn_area: Vec<ChunkPos> = (-10..=10)
        .flat_map(|x| (-10..=10).map(move |z| ChunkPos::new(centre.x + x, centre.z + z)))
        .collect();
    for position in &spawn_area {
        handle.request(StoreRequest::Load {
            position: *position,
        });
    }
    let chunks = loaded(&handle, &spawn_area);
    let entities: usize = chunks.values().map(|chunk| chunk.entities().len()).sum();
    let (took, file_syncs, directory_syncs) = flushed(&handle);
    println!(
        "the spawn's {} chunks, with {entities} entities, first saved in {took:?} ({file_syncs} \
         file syncs, {directory_syncs} directory syncs)",
        spawn_area.len()
    );

    // Squares of chunks beyond it, each made, then saved for the first time, then saved
    // again once stored.
    let mut next_x = centre.x + 12;
    for (side, times) in [(1, 7), (3, 5), (5, 5)] {
        let mut first = Vec::new();
        let mut again = Vec::new();
        let mut syncs = BTreeSet::new();
        let mut with_entities = 0;
        let mut entities = 0;
        for _ in 0..times {
            let square: Vec<ChunkPos> = (0..side)
                .flat_map(|x| (0..side).map(move |z| ChunkPos::new(next_x + x, centre.z + z)))
                .collect();
            next_x += side;
            for position in &square {
                handle.request(StoreRequest::Load {
                    position: *position,
                });
            }
            let chunks = loaded(&handle, &square);
            with_entities += chunks
                .values()
                .filter(|chunk| !chunk.entities().is_empty())
                .count();
            entities += chunks
                .values()
                .map(|chunk| chunk.entities().len())
                .sum::<usize>();
            let (took, file_syncs, directory_syncs) = flushed(&handle);
            first.push(took);
            syncs.insert(("first", file_syncs, directory_syncs));
            for (position, chunk) in &chunks {
                handle.request(StoreRequest::Save {
                    position: *position,
                    tick: 1,
                    chunk: chunk.clone(),
                    parts: Parts::ALL,
                });
            }
            // A save is encoded as the store takes it, and a note as the region is
            // answered: a load of the last saved chunk is answered once the saves before
            // it are taken, so that the flush measures the syncs alone, as for the notes.
            let last = *square.last().expect("a square has chunks");
            handle.request(StoreRequest::Load { position: last });
            loaded(&handle, &[last]);
            let (took, file_syncs, directory_syncs) = flushed(&handle);
            again.push(took);
            syncs.insert(("again", file_syncs, directory_syncs));
        }
        let (first_middle, first_worst) = middle_and_worst(&mut first);
        let (again_middle, again_worst) = middle_and_worst(&mut again);
        println!(
            "{times} squares of {} chunks ({with_entities} of them with entities, {entities} \
             entities in all): the first save, in two rounds, took {first_middle:?} in the \
             middle and {first_worst:?} at worst; a save of the same chunks stored, in one \
             round, {again_middle:?} and {again_worst:?}; (round, file syncs, directory \
             syncs) {syncs:?}",
            side * side
        );
    }
    drop(handle);
    store.flush().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement; run it by name with --release --ignored --nocapture"]
async fn measured_how_soon_a_feeding_across_the_line_merges_regions_that_follow_players() {
    let directory = tempfile::Builder::new()
        .prefix("n7n-feeding")
        .tempdir()
        .unwrap();
    let mut cluster = Cluster::new(directory.path(), 3, "").await;
    cluster.coordinator_arguments = following::coordinator_arguments();
    cluster.view_distance = following::VIEW;
    cluster.start().await;
    let mut feeder = joined(&cluster, "Feeder").await;
    let mut wanderer = joined(&cluster, "Wanderer").await;

    // The wanderer goes so far east that it is split off, as in `crossing.rs`.
    let answering = Answering::start(feeder);
    wanderer
        .walk_to(40.0 * 16.0 + 8.5, 0.5, following::TO_THE_LANE)
        .await
        .unwrap();
    let deadline = Instant::now() + following::SETTLES_WITHIN;
    let (list, home, away) = loop {
        if let Ok(list) = cluster.regions().await {
            let home = following::region_of(&list, (0, 0));
            let away = following::region_of(&list, (40, 0));
            if let (Some(home), Some(away)) = (home, away)
                && home != away
            {
                break (list, home, away);
            }
        }
        assert!(
            Instant::now() < deadline,
            "the wanderer was never split off"
        );
        wanderer.idle(LOOK).await.unwrap();
    };
    println!("split: {list:?}");
    let (Some(near), Some(off)) = (bounds(&list, home), bounds(&list, away)) else {
        panic!("both regions hold land: {list:?}");
    };
    assert_eq!(
        near.max.x + 1,
        off.min.x,
        "the two regions meet at a line: {list:?}"
    );
    let line_chunk = off.min.x;
    // The wanderer comes back to 16 chunks east of the line: the feeder in the chunk east
    // of the line is then within the split distance of it, and in the chunk west of the
    // line farther than the merge distance from it.
    let stay = (line_chunk + 16).min(off.max.x);
    let merge_distance = 2 * following::VIEW + 8;
    assert!(
        stay - (line_chunk - 1) > merge_distance && stay - line_chunk <= 20,
        "the wanderer can stand where nothing but the feeding makes the regions merge: {list:?}"
    );
    wanderer
        .walk_to(f64::from(stay * 16) + 8.5, 0.5, following::TO_THE_LANE)
        .await
        .unwrap();
    feeder = answering.back().await;
    let answering = Answering::start(wanderer);

    // The pen at the line `l`, in the row `z`: a cell west of it at `l - 2` and one east of
    // it at `l`, a fence between them at `l - 1`, so that the cows stay on their sides
    // and are close enough to breed (2 blocks apart, at most 2.97).
    let (l, z) = (line_chunk * 16, 4);
    let (lf, zf) = (f64::from(l), f64::from(z));
    feeder
        .walk_to(lf - 4.5, zf - 2.5, following::TO_THE_LANE)
        .await
        .unwrap();
    go_to(&mut feeder, lf - 4.5, zf - 2.5).await;
    hold(&mut feeder, items::OAK_FENCE, 64).await;
    // West of the line, from the south and then the north.
    for x in [l - 3, l - 2, l - 1] {
        go_to(&mut feeder, f64::from(x) + 0.5, zf - 2.5).await;
        post(&mut feeder, (x, z - 1)).await;
        if x != l - 2 {
            post(&mut feeder, (x, z)).await;
        }
    }
    go_to(&mut feeder, lf - 4.5, zf - 2.5).await;
    go_to(&mut feeder, lf - 4.5, zf + 2.5).await;
    for x in [l - 3, l - 2, l - 1] {
        go_to(&mut feeder, f64::from(x) + 0.5, zf + 2.5).await;
        post(&mut feeder, (x, z + 1)).await;
    }
    // East of it, across the line on foot: from the north and then the south.
    for x in [l, l + 1] {
        go_to(&mut feeder, f64::from(x) + 0.5, zf + 2.5).await;
        post(&mut feeder, (x, z + 1)).await;
    }
    go_to(&mut feeder, lf + 2.5, zf + 2.5).await;
    go_to(&mut feeder, lf + 2.5, zf - 2.5).await;
    for x in [l + 1, l] {
        go_to(&mut feeder, f64::from(x) + 0.5, zf - 2.5).await;
        post(&mut feeder, (x, z - 1)).await;
        if x == l + 1 {
            post(&mut feeder, (x, z)).await;
        }
    }
    let east_cow = (lf + 0.5, zf + 0.5);
    let west_cow = (lf - 1.5, zf + 0.5);
    lay_on(
        &mut feeder,
        items::COW_SPAWN_EGG,
        entity_types::COW,
        1,
        &[(l, z)],
    )
    .await;
    go_to(&mut feeder, lf - 1.5, zf - 2.5).await;
    lay_on(
        &mut feeder,
        items::COW_SPAWN_EGG,
        entity_types::COW,
        1,
        &[(l - 2, z)],
    )
    .await;
    go_to(&mut feeder, lf - 0.5, zf - 1.5).await;
    let cow_at = |bot: &Bot, (x, z): (f64, f64)| {
        bot.entities
            .values()
            .filter(|entity| entity.kind == entity_types::COW)
            .find(|entity| (entity.position.0 - x).hypot(entity.position.2 - z) < 1.0)
            .map(|entity| entity.entity_id)
            .expect("the cow is in its cell")
    };
    let (west_id, east_id) = (cow_at(&feeder, west_cow), cow_at(&feeder, east_cow));
    let cows = |bot: &Bot| shown_near(bot, entity_types::COW, (lf, zf), 6.0);
    assert_eq!(cows(&feeder), 2);
    let list = cluster.regions().await.unwrap();
    assert!(
        living(&list).contains(&home) && living(&list).contains(&away),
        "the two regions are as they were split while the pen was made: {list:?}"
    );

    // The west cow on the feeder's side, then the east one across the line.
    hold(&mut feeder, items::WHEAT, 1).await;
    feeder.interact(west_id, [0.0, 0.7, 0.0]).await.unwrap();
    feeder.idle(Duration::from_millis(200)).await.unwrap();
    let marks = marks(&cluster);
    let fed = now();
    let feeding = Instant::now();
    feeder.interact(east_id, [0.0, 0.7, 0.0]).await.unwrap();
    let begun_line = "a merge is begun for a circuit at the line";
    let deadline = Instant::now() + PATIENCE;
    let (begun, merged, ended) = loop {
        let log = since(&cluster, &marks, "coordinator");
        let begun = log
            .lines()
            .find(|line| line.contains(begun_line))
            .and_then(time_of);
        let ended = log
            .lines()
            .find(|line| ends(Reshape::Merge, line))
            .and_then(time_of);
        let listed = cluster.regions().await.ok().is_some_and(|list| {
            absorbed_by(&list, away) == Some(home) || absorbed_by(&list, home) == Some(away)
        });
        if let (Some(begun), Some(ended), true) = (begun, ended, listed) {
            assert!(
                !log.contains("a merge is begun by the distances"),
                "the merge is the feeding's, not the distances': {log}"
            );
            break (begun, feeding.elapsed(), ended);
        }
        assert!(
            Instant::now() < deadline,
            "the regions never merged for the feeding:\n{log}"
        );
        feeder.idle(LOOK).await.unwrap();
    };
    feeder
        .wait_until(PATIENCE, |bot| cows(bot) == 3)
        .await
        .unwrap_or_else(|error| panic!("{error}: the cows had no calf"));
    let calf = feeding.elapsed();
    // Long enough for a second calf, if anything made one: the parents' love lasted.
    feeder.idle(Duration::from_secs(10)).await.unwrap();
    println!(
        "a feeding across the line between two regions that follow their players: the \
         coordinator began the merge {:.0} ms after the feeding, ended it {:.0} ms after \
         that; the list had it {:.2} s after the feeding, and the calf was shown {:.2} s \
         after it",
        (begun - fed) * 1000.0,
        (ended - begun) * 1000.0,
        merged.as_secs_f64(),
        calf.as_secs_f64()
    );
    for name in cluster.names() {
        for line in since(&cluster, &marks, &name).lines() {
            if line.contains(STOOD_STILL) {
                println!(
                    "  {name}: region {} stood still {} ms",
                    number::<u32>(line, "region").unwrap_or(u32::MAX),
                    number::<u64>(line, "milliseconds").unwrap_or(0)
                );
            }
        }
    }
    say_and_bound_the_ticks(&cluster, &marks);
    assert_eq!(cows(&feeder), 3, "the cows bred once");
    drop(answering.back().await);
    drop(feeder);
    cluster.terminate().await;
}
