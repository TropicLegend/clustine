//! What is measured for N7f (`docs/groundwork/n7-plan.md`, section 11, and section 9.2
//! item 5): what animals cost a world divided into regions.
//!
//! - **A pen of 100**: a bot fences a pen of 9 × 9 west of a pinned line and fills it
//!   with 100 cows from spawn eggs, and the world runs for a minute with them.
//! - **20 sheep following wheat**: a bot holding wheat walks to and fro across the line
//!   with 20 sheep after it, for a minute; the faces of the players beyond a region are
//!   what that sends most of.
//! - **A flight over eggs' animals**: in a world whose regions follow the players, a bot
//!   flies east, leaving ten cows every 64 blocks, flies back and over them again.
//! - **A merge with animals**: 100 cows in the region east of the line, absorbed by the
//!   region west of it with `clustine merge`.
//! - **A crash's restore of 500 mobs**: the worker of a region with 500 cows is killed
//!   without warning, and another worker takes the region over from the world store.
//!
//! Printed for each: how much the world's files grew, what the world store answered
//! its regions by kind (from its tally, which a process keeps only where it is started
//! with `CLUSTINE_STREAM_TALLY` set: the copies' updates, the faces, the commits'
//! answers), and every report of ticks that took longer than a tick has; for the merge
//! the command's own time, and for the restore how long until the region ran again and
//! until a player was shown its 500 cows.
//!
//! Where the world lies decides what is measured: `TMPDIR` on memory (`/dev/shm/…`) for
//! the logic, on the real disk for the disk, which on the owner's machine has a cache in
//! memory in front of it (CLAUDE.md, "Checking work"). Say which a number is from.
//!
//! ```text
//! CLUSTINE_STREAM_TALLY=1 cargo test --release -p clustine --test herds -- \
//!     --ignored --nocapture --test-threads 1
//! ```
//!
//! Measured on 2026-10-20, alike in memory and on the real disk with its cache, after
//! the encoding was adjusted from the first measurement (a tick's changes of an entity
//! merged, and a mob's every-tick values kept apart; a cow of the pen then committed
//! 83 bytes a tick where it had 157), with no tick over 50 ms but where said:
//!
//! - the pen of 100: the world 13 MiB larger a minute (27 MiB before), and 11 MiB of
//!   copies' updates to the region across the line (22 MiB before), 99 % of what the
//!   store answered; faces 6 KiB;
//! - 20 sheep after wheat: 6 MiB a minute; faces 61 KiB, 1 % of the stream, the copies'
//!   updates 73 % and the chunks loaded as the line was crossed 16 %;
//! - the flight: 322 KiB, the chunks loaded 98 % of the stream, every cow there again;
//! - the merge: 19 ms by the command in memory, 36 ms on the disk, every cow there;
//! - the restore: region 0 ran again 3.5 s after the kill (a lease of 3 s), a player
//!   who joined was shown its 500 cows 3.9 s after it, and the worker that took it over
//!   had one tick of 73 to 76 ms.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use clustine_botswarm::Bot;
use clustine_data::{entity_types, items};
use clustine_protocol::item::ItemStack;
use clustine_protocol::packets::play::face;

use common::processes::{Cluster, ask};

const PATIENCE: Duration = Duration::from_secs(60);

/// What a worker says of ticks that took too long.
const SLOW: &str = "ticks took longer than the 50 ms a tick has";

/// What the world store says of its tally.
const TALLY: &str = "the store answered: ";

/// The top of the flat world's grass, which players stand on, and the block it is.
const FEET: f64 = -60.0;
const GRASS: i32 = -61;

/// The chunk x coordinate the pinned worlds are divided at: the line at block x = 64.
const PIN: &str = "4";
const LINE: f64 = 64.0;

/// How long the herds are left to themselves while measured.
const MEASURED: Duration = Duration::from_secs(60);

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

/// How many entities of type `kind` the bot is shown.
fn shown(bot: &Bot, kind: i32) -> usize {
    bot.entities
        .values()
        .filter(|entity| entity.kind == kind)
        .count()
}

/// A cluster of `workers` workers in `directory`, its world pinned at `pins` (none for
/// regions that follow players), started.
async fn cluster(directory: &Path, workers: usize, pins: &str) -> Cluster {
    let mut cluster = Cluster::new(directory, workers, pins).await;
    cluster.start().await;
    cluster
}

/// A bot that has joined `cluster` and been shown the chunks about it.
async fn joined(cluster: &Cluster, name: &str) -> Bot {
    let mut bot = Bot::join(&cluster.edge.0, name).await.unwrap();
    let around = common::view_area((0, 0), cluster.view_distance).len();
    bot.wait_for_chunks(around, PATIENCE).await.unwrap();
    bot
}

/// Walks the bot to `(x, z)` on the grass and waits until it is shown the grass about
/// it, so that what it clicks there can be judged.
async fn go_to(bot: &mut Bot, x: f64, z: f64) {
    bot.walk_to(x, z, 0.6).await.unwrap();
    let (bx, bz) = (x.floor() as i32, z.floor() as i32);
    bot.wait_until(PATIENCE, |bot| {
        bot.block_at(bx, GRASS, bz)
            .is_ok_and(|state| state.is_some_and(|state| state != 0))
    })
    .await
    .unwrap();
}

/// Uses the spawn egg `egg` `count` times on the grass about where the bot stands,
/// within two blocks of it, and waits until it is shown each animal of type `kind`.
async fn lay(bot: &mut Bot, egg: i32, kind: i32, count: usize) {
    bot.put_in_hotbar(0, ItemStack::new(egg, 1)).await.unwrap();
    bot.select_slot(0).await.unwrap();
    let (x, z) = (bot.location.0.floor() as i32, bot.location.2.floor() as i32);
    let before = shown(bot, kind);
    for laid in 0..count {
        let spot = i32::try_from(laid % 25).unwrap();
        let (dx, dz) = (spot % 5 - 2, spot / 5 - 2);
        bot.use_item_on(x + dx, GRASS, z + dz, face::TOP)
            .await
            .unwrap();
        bot.wait_until(PATIENCE, |bot| shown(bot, kind) > before + laid)
            .await
            .unwrap_or_else(|error| panic!("{error}: the animal {laid} did not come"));
    }
}

/// Fences the square from `(x0, z0)` to `(x1, z1)` on the grass, walking about inside it.
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
        bot.use_item_on(x, GRASS, z, face::TOP).await.unwrap();
        bot.wait_until(PATIENCE, |bot| {
            bot.block_at(x, GRASS + 1, z)
                .is_ok_and(|state| state.is_some_and(|state| state != 0))
        })
        .await
        .unwrap_or_else(|error| panic!("{error}: no fence at {x}, {z}"));
    }
}

/// What the world store's tally said in its log from the byte `from` on, summed by
/// kind: the count and the bytes.
fn tallied(cluster: &Cluster, from: usize) -> BTreeMap<String, (u64, u64)> {
    let log = cluster.log("worldstore");
    let mut kinds: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for line in log.get(from..).unwrap_or_default().lines() {
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

/// Prints what was measured of `what` since `marks` and `bytes_before`.
fn report(what: &str, cluster: &Cluster, marks: &BTreeMap<String, usize>, bytes_before: u64) {
    let grown = bytes_under(&cluster.world).saturating_sub(bytes_before);
    println!(
        "{what}: the world in {} grew by {} KiB",
        cluster.world.display(),
        grown / 1024
    );
    let kinds = tallied(cluster, marks.get("worldstore").copied().unwrap_or(0));
    if kinds.is_empty() {
        println!("  the store kept no tally: start the test with CLUSTINE_STREAM_TALLY=1");
    }
    let total: u64 = kinds.values().map(|(_, bytes)| bytes).sum();
    for (kind, (count, bytes)) in &kinds {
        println!(
            "  {kind}: {count} answers, {} KiB, {:.1} % of the bytes",
            bytes / 1024,
            *bytes as f64 * 100.0 / total.max(1) as f64
        );
    }
    let mut slow = 0;
    for name in cluster.names() {
        let from = marks.get(&name).copied().unwrap_or(0);
        let log = cluster.log(&name);
        for line in log.get(from..).unwrap_or_default().lines() {
            if line.contains(SLOW) {
                slow += 1;
                println!("  {name}: {line}");
            }
        }
    }
    println!("  {slow} reports of slow ticks");
}

/// Lets the bot stand for `how_long`, answering what it is sent.
async fn stand(bot: &mut Bot, how_long: Duration) {
    bot.idle(how_long).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement; run it by name with --release --ignored --nocapture"]
async fn measured_a_pen_of_100_animals_at_a_line() {
    let directory = tempfile::Builder::new().prefix("herds").tempdir().unwrap();
    let cluster = cluster(directory.path(), 2, PIN).await;
    let mut bot = joined(&cluster, "Farmer").await;
    // A pen of 9 × 9 inside, from x = 54 to 62 west of the line, and its fence.
    fence(&mut bot, (53, -5), (63, 5)).await;
    go_to(&mut bot, 58.5, 0.5).await;
    lay(&mut bot, items::COW_SPAWN_EGG, entity_types::COW, 100).await;
    // The bot stands outside, in view of the pen.
    go_to(&mut bot, 58.5, -8.5).await;

    let (marks, bytes) = (marks(&cluster), bytes_under(&cluster.world));
    stand(&mut bot, MEASURED).await;
    report("a pen of 100 cows for a minute", &cluster, &marks, bytes);
    println!(
        "  the farmer is shown {} cows",
        shown(&bot, entity_types::COW)
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement; run it by name with --release --ignored --nocapture"]
async fn measured_twenty_sheep_following_wheat_across_a_line() {
    let directory = tempfile::Builder::new().prefix("herds").tempdir().unwrap();
    let cluster = cluster(directory.path(), 2, PIN).await;
    let mut bot = joined(&cluster, "Shepherd").await;
    go_to(&mut bot, LINE - 8.5, 0.5).await;
    lay(&mut bot, items::SHEEP_SPAWN_EGG, entity_types::SHEEP, 20).await;
    bot.put_in_hotbar(1, ItemStack::new(items::WHEAT, 1))
        .await
        .unwrap();
    bot.select_slot(1).await.unwrap();

    let (marks, bytes) = (marks(&cluster), bytes_under(&cluster.world));
    let began = Instant::now();
    let mut eastwards = true;
    let mut crossings = 0;
    while began.elapsed() < MEASURED {
        // A sheep tempted walks at a quarter of a block a tick at most: the shepherd
        // keeps ahead of them by little.
        let to = if eastwards { LINE + 8.5 } else { LINE - 8.5 };
        bot.walk_to(to, 0.5, 0.15).await.unwrap();
        stand(&mut bot, Duration::from_secs(3)).await;
        eastwards = !eastwards;
        crossings += 1;
    }
    report(
        "20 sheep following wheat to and fro across the line for a minute",
        &cluster,
        &marks,
        bytes,
    );
    let east = bot
        .entities
        .values()
        .filter(|entity| entity.kind == entity_types::SHEEP && entity.position.0 > LINE)
        .count();
    println!(
        "  the shepherd crossed {crossings} times; is shown {} sheep, {east} of them east",
        shown(&bot, entity_types::SHEEP)
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement; run it by name with --release --ignored --nocapture"]
async fn measured_a_flight_over_animals_from_eggs() {
    let directory = tempfile::Builder::new().prefix("herds").tempdir().unwrap();
    let cluster = cluster(directory.path(), 2, "").await;
    let mut bot = joined(&cluster, "Flyer").await;
    let stops: Vec<f64> = (1..=10).map(|stop| f64::from(stop) * 64.0 + 0.5).collect();

    let (marks, bytes) = (marks(&cluster), bytes_under(&cluster.world));
    let began = Instant::now();
    for x in &stops {
        bot.fly_to((*x, FEET + 0.5, 0.5), (0.0, 0.0), 1.0)
            .await
            .unwrap();
        go_to(&mut bot, *x, 0.5).await;
        lay(&mut bot, items::COW_SPAWN_EGG, entity_types::COW, 10).await;
    }
    let laid = began.elapsed();
    bot.fly_to((0.5, FEET + 0.5, 0.5), (0.0, 0.0), 1.0)
        .await
        .unwrap();
    let mut seen_again = Vec::new();
    for x in &stops {
        bot.fly_to((*x, FEET + 0.5, 0.5), (0.0, 0.0), 1.0)
            .await
            .unwrap();
        let near = |bot: &Bot| {
            bot.entities
                .values()
                .filter(|entity| {
                    entity.kind == entity_types::COW && (entity.position.0 - x).abs() < 16.0
                })
                .count()
        };
        let _ = bot
            .wait_until(Duration::from_secs(10), |bot| near(bot) >= 10)
            .await;
        seen_again.push(near(&bot));
    }
    report(
        "a flight east leaving ten cows every 64 blocks, back, and over them again",
        &cluster,
        &marks,
        bytes,
    );
    println!(
        "  laid in {:.1} s; on the second flight shown at each stop {seen_again:?}",
        laid.as_secs_f64()
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement; run it by name with --release --ignored --nocapture"]
async fn measured_a_merge_with_100_animals() {
    let directory = tempfile::Builder::new().prefix("herds").tempdir().unwrap();
    let cluster = cluster(directory.path(), 2, PIN).await;
    let mut stayer = joined(&cluster, "Stayer").await;
    let mut herder = joined(&cluster, "Herder").await;
    go_to(&mut herder, LINE + 8.5, 0.5).await;
    lay(&mut herder, items::COW_SPAWN_EGG, entity_types::COW, 100).await;
    stand(&mut herder, Duration::from_secs(2)).await;

    let (marks, bytes) = (marks(&cluster), bytes_under(&cluster.world));
    // Both stand while it is made, answering what they are sent.
    let asking = ask(
        "region 0 to absorb region 1".to_owned(),
        cluster.merge_command(0, 1),
    );
    tokio::join!(
        stand(&mut stayer, Duration::from_secs(8)),
        stand(&mut herder, Duration::from_secs(8))
    );
    let asked = asking.await.unwrap();
    report("a merge of a region with 100 cows", &cluster, &marks, bytes);
    println!("  the command: {}", asked.outcome());
    println!(
        "  the herder is shown {} cows after it",
        shown(&herder, entity_types::COW)
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement; run it by name with --release --ignored --nocapture"]
async fn measured_the_restore_of_a_region_with_500_mobs_after_a_crash() {
    let directory = tempfile::Builder::new().prefix("herds").tempdir().unwrap();
    // A view of eight chunks, so that a cow pushed out of the crowd is still seen.
    let mut cluster = Cluster::new(directory.path(), 2, PIN).await;
    cluster.view_distance = 8;
    cluster.start().await;
    let mut bot = joined(&cluster, "Rancher").await;
    go_to(&mut bot, 8.5, 0.5).await;
    lay(&mut bot, items::COW_SPAWN_EGG, entity_types::COW, 500).await;
    stand(&mut bot, Duration::from_secs(2)).await;
    let before = shown(&bot, entity_types::COW);
    let herd: Vec<(uuid::Uuid, (f64, f64, f64))> = bot
        .entities
        .values()
        .filter(|entity| entity.kind == entity_types::COW)
        .map(|entity| (entity.uuid, entity.position))
        .collect();
    drop(bot);

    // The worker running region 0, the one with the spawn point.
    let owner = (0..cluster.workers.len())
        .find(|worker| {
            cluster
                .log(&common::processes::worker_name(*worker))
                .contains("running a region region=0 ")
        })
        .expect("a worker runs region 0");
    let (marks, bytes) = (marks(&cluster), bytes_under(&cluster.world));
    let killed = Instant::now();
    cluster.kill_worker(owner).await;
    let other = common::processes::worker_name(1 - owner);
    let from = marks.get(&other).copied().unwrap_or(0);
    let running = loop {
        let log = cluster.log(&other);
        if log
            .get(from..)
            .is_some_and(|said| said.contains("running a region region=0 "))
        {
            break killed.elapsed();
        }
        assert!(killed.elapsed() < PATIENCE, "region 0 was not taken over");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let mut back = Bot::join(&cluster.edge.0, "Returner").await.unwrap();
    let all = back
        .wait_until(PATIENCE, |bot| shown(bot, entity_types::COW) >= 500)
        .await;
    let shown_all = killed.elapsed();
    for (uuid, position) in &herd {
        if !back.entities.values().any(|entity| entity.uuid == *uuid) {
            println!("  missing after the restore: {uuid} last shown at {position:?}");
        }
    }
    println!(
        "  the rancher was shown {before} cows before the kill; the returner is shown {} {}",
        shown(&back, entity_types::COW),
        if all.is_ok() {
            ""
        } else {
            "after waiting a minute"
        }
    );
    report(
        "a crash of the worker of a region with 500 cows, and its restore",
        &cluster,
        &marks,
        bytes,
    );
    println!(
        "  region 0 ran again {:.2} s after the kill; a player who joined was shown its \
         500 cows {:.2} s after it (the lease is {} s)",
        running.as_secs_f64(),
        shown_all.as_secs_f64(),
        cluster.lease_seconds.unwrap_or(0)
    );
}

/// Found by the measurement of the restore: when the player of a herd leaves, its chunks
/// are let go, and when another comes they are loaded and shown again, each at its own
/// tick. An animal that went from one into the other between the two was in neither
/// snapshot, and the edge, which knew nothing of it, passed its moves over: it was
/// shown to nobody until its chunk was loaded afresh. Now the region reports an entity
/// entering a chunk whole (`RegionEvent::EntityEntered`), and the newcomer is shown the
/// whole herd. The animals that crossed in that tick were 2 of 500 in every run before.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "500 animals for a minute and more; run it by name with --release --ignored"]
async fn a_newcomer_is_shown_every_animal_of_a_herd_whose_chunks_were_let_go() {
    let directory = tempfile::Builder::new().prefix("herds").tempdir().unwrap();
    let mut cluster = Cluster::new(directory.path(), 2, PIN).await;
    cluster.view_distance = 8;
    cluster.start().await;
    let mut bot = joined(&cluster, "Rancher").await;
    go_to(&mut bot, 8.5, 0.5).await;
    lay(&mut bot, items::COW_SPAWN_EGG, entity_types::COW, 500).await;
    stand(&mut bot, Duration::from_secs(2)).await;
    let herd: Vec<uuid::Uuid> = bot
        .entities
        .values()
        .filter(|entity| entity.kind == entity_types::COW)
        .map(|entity| entity.uuid)
        .collect();
    assert_eq!(herd.len(), 500);
    drop(bot);
    // Long enough for the herd's chunks to be let go.
    tokio::time::sleep(Duration::from_secs(20)).await;
    let mut newcomer = Bot::join(&cluster.edge.0, "Newcomer").await.unwrap();
    let all = newcomer
        .wait_until(PATIENCE, |bot| shown(bot, entity_types::COW) >= 500)
        .await;
    let missing: Vec<&uuid::Uuid> = herd
        .iter()
        .filter(|uuid| {
            !newcomer
                .entities
                .values()
                .any(|entity| entity.uuid == **uuid)
        })
        .collect();
    assert!(all.is_ok() && missing.is_empty(), "not shown: {missing:?}");
}
