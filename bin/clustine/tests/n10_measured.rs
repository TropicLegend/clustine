//! What is measured for N10 (`docs/groundwork/n10-plan.md`, section 15: "a village of 30
//! villagers at dusk before they have beds (every bed claimed by a path search), and a
//! raid's three waves, in memory and on the real disk, as N7 measured mobs"):
//!
//! - **A village of 30 at dusk**: two regions pinned with a line at block x = 64, and a
//!   village built across it from what a creative player can do: 30 villagers from their
//!   egg, then a bell and 30 beds, which the villagers claim one by one, each by a path
//!   search (`AcquirePoi`). The clock is set to 9000 beforehand, so that the beds are
//!   placed in the afternoon and the villagers go to them at dusk (12000) and lie down.
//!   Two stretches are measured: from the first bed to 11800, the claims; and from 11800
//!   for 90 seconds, the walk to bed and the lying down.
//! - **A raid's three waves**: the same village in the morning, where the bot drinks an
//!   ominous bottle and, once the raid begins, kills every raider it is shown with a
//!   sword, wave after wave, until the third wave is dead (three waves on easy, the
//!   world's difficulty) and the raid has had its celebration.
//!
//! Printed for each: how much the world's files grew, what the world store answered its
//! regions by kind (where the processes are started with `CLUSTINE_STREAM_TALLY` set),
//! and every report of ticks that took longer than a tick has. What each asserts is only
//! what must hold: no tick longer than the 5 s the tests of regions allow a pause
//! (`common::wandering::LONGEST_PAUSE`), and that the scene happened (the villagers lay
//! down; three waves came and died). The plan acts on a tick over 50 ms on the real disk.
//!
//! Where the world lies decides what is measured: `TMPDIR` on memory (`/dev/shm/…`) for
//! the logic, on the real disk for the disk, which on the owner's machine has a cache in
//! memory in front of it (CLAUDE.md, "Checking work"). Say which a number is from.
//!
//! ```text
//! CLUSTINE_STREAM_TALLY=1 cargo test --release -p clustine --test n10_measured -- \
//!     --ignored --nocapture --test-threads 1
//! ```

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, Instant};

use clustine_botswarm::Bot;
use clustine_data::{entity_types, items};
use clustine_format::TableFile;
use clustine_protocol::item::ItemStack;
use clustine_protocol::packets::play::{face, pose};

use common::processes::Cluster;
use common::view_area;
use common::wandering::{LONGEST_PAUSE, number};

const PATIENCE: Duration = Duration::from_secs(60);

/// What a worker says of ticks that took too long.
const SLOW: &str = "ticks took longer than the 50 ms a tick has";

/// What the world store says of its tally.
const TALLY: &str = "the store answered: ";

/// The flat world's grass, which players stand on.
const GRASS: i32 = -61;

/// Where a player's feet are on the grass.
const FEET: f64 = -60.0;

/// How fast the bot moves, in blocks a tick: a brisk walk, as in `herds.rs`.
const BRISK: f64 = 0.6;

/// How many villagers and beds the village has.
const VILLAGERS: usize = 30;

/// The village's middle: on the line between the two regions.
const MIDDLE: (f64, f64) = (64.5, 0.5);

/// The kinds of raider a raid on easy brings.
const RAIDERS: [i32; 5] = [
    entity_types::PILLAGER,
    entity_types::VINDICATOR,
    entity_types::RAVAGER,
    entity_types::WITCH,
    entity_types::EVOKER,
];

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

/// The entities of the kinds `kinds` the bot is shown, by id.
fn shown_of(bot: &Bot, kinds: &[i32]) -> Vec<i32> {
    bot.entities
        .values()
        .filter(|entity| kinds.contains(&entity.kind))
        .map(|entity| entity.entity_id)
        .collect()
}

/// How many villagers the bot is shown lying in a bed, by the pose their data says: a
/// sleeper is put 0.6875 above the floor (`LivingEntity.setPosToBed`) and falls onto the
/// mattress at 0.5625 a tick later, the height of one standing on a bed.
fn lying_down(bot: &Bot) -> usize {
    bot.entities
        .values()
        .filter(|entity| {
            entity.kind == entity_types::VILLAGER && entity.pose == Some(pose::SLEEPING)
        })
        .count()
}

/// Starts a cluster of two workers on two regions pinned at chunk x = 4 in `directory`,
/// on a world whose overworld clock has been set to `day` ticks into its second day.
/// Returns the cluster and the game time the clock was set at, from which the time of
/// day is told ([`day_time`]).
async fn started_at(directory: &Path, day: i64) -> (Cluster, i64) {
    let mut cluster = Cluster::new(directory, 2, "4").await;
    cluster.start().await;
    // Joined once, so that the world is made and its table written.
    let mut bot = joined(&cluster, "Clockmaker").await;
    bot.close().await.unwrap();
    cluster.terminate().await;
    let path = cluster.world.join("regions").join("table");
    let mut table = TableFile::decode(&std::fs::read(&path).unwrap()).unwrap();
    let wanted = 24_000 + u64::try_from(day).expect("a time of day");
    assert!(
        table.clock[1] < wanted,
        "the new world's clock {:?} is already past {day} of its second day",
        table.clock
    );
    table.clock[1] = wanted;
    let game_time = i64::try_from(table.clock[0]).expect("a game time");
    std::fs::write(&path, table.encode()).unwrap();
    cluster.start().await;
    (cluster, game_time)
}

/// The time of day the bot was told last: the clock was set to `day` at the game time
/// `set_at`, and moves with the game time, which every time packet says.
fn day_time(bot: &Bot, day: i64, set_at: i64) -> i64 {
    let latest = bot.times.last().map_or(set_at, |time| time.game_time);
    (day + latest - set_at).rem_euclid(24_000)
}

/// A bot that has joined `cluster` and been shown the chunks about the chunk players
/// enter in.
async fn joined(cluster: &Cluster, name: &str) -> Bot {
    let mut bot = Bot::join(&cluster.edge.0, name).await.unwrap();
    let around = view_area((0, 0), cluster.view_distance).len();
    bot.wait_for_chunks(around, PATIENCE).await.unwrap();
    bot
}

/// Moves the bot at the grass's height to `(x, z)`, looking along `look`, and waits
/// until it is shown the grass there, so that what it clicks there can be judged.
async fn go_to(bot: &mut Bot, (x, z): (f64, f64), look: (f32, f32)) {
    bot.fly_to((x, FEET, z), look, BRISK).await.unwrap();
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

/// Whether the bot is shown a block other than air on the grass at `(x, z)`.
fn placed(bot: &Bot, (x, z): (i32, i32)) -> bool {
    bot.block_at(x, GRASS + 1, z)
        .is_ok_and(|state| state.is_some_and(|state| state != 0))
}

/// Uses the held item on the grass at `(x, z)` and waits until the bot is shown a block
/// on it, trying again while something stands in the way.
async fn put(bot: &mut Bot, (x, z): (i32, i32)) {
    for _ in 0..20 {
        bot.use_item_on(x, GRASS, z, face::TOP).await.unwrap();
        let shown = bot
            .wait_until(Duration::from_secs(2), |bot| placed(bot, (x, z)))
            .await;
        if shown.is_ok() {
            return;
        }
    }
    panic!("nothing was placed at {x}, {z}");
}

/// The 30 cells beds' feet are placed on: six columns three blocks apart across the
/// line, five rows four apart, each bed's head one block south of its foot.
fn bed_cells() -> Vec<(i32, i32)> {
    let mut cells = Vec::new();
    for row in 0..5 {
        for column in 0..6 {
            cells.push((55 + 3 * column, -10 + 4 * row));
        }
    }
    cells
}

/// Builds the village: 30 villagers from their egg on the grass south of where the beds
/// will be, then the bell, then the 30 beds, the bot standing north of each looking
/// south, so that its head lies south of its foot. Says how long each part took, and
/// returns when the first bed was placed.
async fn build_village(bot: &mut Bot) -> Instant {
    bot.set_flying(true).await.unwrap();
    // Laid west and east of the line, 15 on each side.
    let laying = Instant::now();
    hold(bot, items::VILLAGER_SPAWN_EGG, 1).await;
    for (side, x) in [(0, 59.5), (1, 69.5)] {
        go_to(bot, (x, 14.5), (0.0, 60.0)).await;
        let (bx, bz) = (x.floor() as i32, 14);
        let cells: Vec<(i32, i32)> = (0..25)
            .map(|spot| (bx + spot % 5 - 2, bz + spot / 5 - 2))
            .collect();
        for laid in 0..VILLAGERS / 2 {
            let before = shown_of(bot, &[entity_types::VILLAGER]).len();
            let (cx, cz) = cells[laid % cells.len()];
            bot.use_item_on(cx, GRASS, cz, face::TOP).await.unwrap();
            bot.wait_until(PATIENCE, |bot| {
                shown_of(bot, &[entity_types::VILLAGER]).len() > before
            })
            .await
            .unwrap_or_else(|error| panic!("{error}: villager {laid} of side {side} did not come"));
        }
    }
    println!(
        "  {VILLAGERS} villagers laid in {:.1} s",
        laying.elapsed().as_secs_f64()
    );

    let building = Instant::now();
    hold(bot, items::BELL, 1).await;
    go_to(bot, (MIDDLE.0, -12.5), (0.0, 60.0)).await;
    put(bot, (64, -11)).await;
    hold(bot, items::WHITE_BED, 1).await;
    let mut first = None;
    for (x, z) in bed_cells() {
        go_to(bot, (f64::from(x) + 0.5, f64::from(z) - 1.5), (0.0, 60.0)).await;
        put(bot, (x, z)).await;
        first.get_or_insert_with(Instant::now);
    }
    println!(
        "  the bell and {VILLAGERS} beds placed in {:.1} s",
        building.elapsed().as_secs_f64()
    );
    first.expect("a bed was placed")
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

/// Says the reports of slow ticks every worker made since `marks`, and the longest tick
/// they name, and fails if it was longer than a pause may be.
fn say_and_bound_the_ticks(cluster: &Cluster, marks: &BTreeMap<String, usize>) {
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

/// Says how much the world grew since `bytes`, and what the world store's tally said in
/// its log since `marks`, by kind: the count and the bytes.
fn say_the_growth(cluster: &Cluster, marks: &BTreeMap<String, usize>, bytes: u64) {
    println!(
        "  the world in {} grew by {} KiB",
        cluster.world.display(),
        bytes_under(&cluster.world).saturating_sub(bytes) / 1024
    );
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
    if kinds.is_empty() {
        println!("  the store kept no tally: start the test with CLUSTINE_STREAM_TALLY=1");
    }
    let total: u64 = kinds.values().map(|(_, bytes)| bytes).sum();
    for (kind, (answers, bytes)) in &kinds {
        println!(
            "  {kind}: {answers} answers, {} KiB, {:.1} % of the bytes",
            bytes / 1024,
            *bytes as f64 * 100.0 / total.max(1) as f64
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement; run it by name with --release --ignored --nocapture"]
async fn measured_a_village_of_30_that_claims_its_beds_and_goes_to_them_at_dusk() {
    let directory = tempfile::Builder::new()
        .prefix("n10-village")
        .tempdir()
        .unwrap();
    const DAY: i64 = 9000;
    let (mut cluster, set_at) = started_at(directory.path(), DAY).await;
    let mut bot = joined(&cluster, "Elder").await;
    bot.set_flying(true).await.unwrap();
    go_to(&mut bot, (MIDDLE.0, 14.5), (0.0, 60.0)).await;

    println!("a village of {VILLAGERS} across a pinned line, built from 9000:");
    let (marks_before, bytes_before) = (marks(&cluster), bytes_under(&cluster.world));
    let first_bed = build_village(&mut bot).await;
    // The bot looks on from the bell, in view of every bed.
    go_to(&mut bot, (MIDDLE.0, -13.5), (0.0, 30.0)).await;
    let claims = bot
        .wait_until(Duration::from_secs(300), |bot| {
            day_time(bot, DAY, set_at) >= 11_800
        })
        .await;
    claims.unwrap_or_else(|error| panic!("{error}: the clock did not reach 11800"));
    println!(
        "from the first villager to 11800 (the beds claimed), the last {:.0} s of it after \
         the first bed:",
        first_bed.elapsed().as_secs_f64()
    );
    say_the_growth(&cluster, &marks_before, bytes_before);
    say_and_bound_the_ticks(&cluster, &marks_before);

    let (marks_dusk, bytes_dusk) = (marks(&cluster), bytes_under(&cluster.world));
    let dusk = Instant::now();
    let mut most_lying = 0;
    while dusk.elapsed() < Duration::from_secs(90) {
        bot.idle(Duration::from_secs(1)).await.unwrap();
        most_lying = most_lying.max(lying_down(&bot));
    }
    println!(
        "from 11800 for 90 s (to {}), the walk to bed: at most {most_lying} of {} villagers \
         shown lying down",
        day_time(&bot, DAY, set_at),
        shown_of(&bot, &[entity_types::VILLAGER]).len()
    );
    say_the_growth(&cluster, &marks_dusk, bytes_dusk);
    say_and_bound_the_ticks(&cluster, &marks_dusk);
    assert!(
        most_lying >= VILLAGERS / 2,
        "only {most_lying} of {VILLAGERS} villagers lay down at dusk"
    );
    cluster.terminate().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement; run it by name with --release --ignored --nocapture"]
async fn measured_a_raid_of_three_waves_on_a_village_of_30() {
    let directory = tempfile::Builder::new()
        .prefix("n10-raid")
        .tempdir()
        .unwrap();
    const DAY: i64 = 1000;
    let (mut cluster, _) = started_at(directory.path(), DAY).await;
    let mut bot = joined(&cluster, "Hero").await;
    bot.set_flying(true).await.unwrap();
    go_to(&mut bot, (MIDDLE.0, 14.5), (0.0, 60.0)).await;
    println!("a raid on a village of {VILLAGERS} across a pinned line:");
    build_village(&mut bot).await;
    // The beds' claims settle first, so that what is measured is the raid's.
    bot.idle(Duration::from_secs(30)).await.unwrap();

    go_to(&mut bot, MIDDLE, (0.0, 0.0)).await;
    let (marks_raid, bytes_raid) = (marks(&cluster), bytes_under(&cluster.world));
    let raid = Instant::now();
    hold(&mut bot, items::OMINOUS_BOTTLE, 1).await;
    bot.use_item(0.0, 0.0).await.unwrap();
    // The drink's 32 ticks, the raid omen's 600 and the countdown's 300, and some.
    bot.wait_until(Duration::from_secs(90), |bot| {
        !shown_of(bot, &RAIDERS).is_empty()
    })
    .await
    .unwrap_or_else(|error| panic!("{error}: no raider came"));
    println!(
        "  the first raider came {:.1} s after the drink",
        raid.elapsed().as_secs_f64()
    );
    hold(&mut bot, items::NETHERITE_SWORD, 1).await;

    // Every raider shown is hunted, the nearest first: the bot flies to above its head
    // and hits it at a sword's full strength (its attack speed of 1.6 a second). A wave
    // is the raiders that come while none of the one before are left.
    let mut seen: BTreeSet<i32> = BTreeSet::new();
    let mut waves: Vec<usize> = Vec::new();
    let mut alive_before = 0;
    let mut since_last = Instant::now();
    let fight = Instant::now();
    loop {
        if fight.elapsed() >= Duration::from_secs(900) {
            let left: Vec<String> = shown_of(&bot, &RAIDERS)
                .iter()
                .filter_map(|id| bot.entities.get(id))
                .map(|entity| {
                    format!(
                        "{} at {:.1?}",
                        clustine_data::ENTITY_TYPES[entity.kind as usize],
                        entity.position
                    )
                })
                .collect();
            panic!(
                "the raid was not over in 15 minutes: waves {waves:?}, the bot at {:.1?}, \
                 left {left:?}",
                bot.location
            );
        }
        let alive = shown_of(&bot, &RAIDERS);
        let new: Vec<i32> = alive
            .iter()
            .copied()
            .filter(|id| !seen.contains(id))
            .collect();
        if !new.is_empty() {
            if alive_before == 0 || waves.is_empty() {
                waves.push(0);
            }
            *waves.last_mut().expect("a wave") += new.len();
            seen.extend(new);
        }
        alive_before = alive.len();
        if alive.is_empty() {
            // A wave's countdown is 300 ticks; after the third, the celebration's 600.
            if waves.len() >= 3 && since_last.elapsed() > Duration::from_secs(40) {
                break;
            }
            bot.idle(Duration::from_millis(250)).await.unwrap();
            continue;
        }
        since_last = Instant::now();
        let at = |bot: &Bot, id: i32| bot.entities.get(&id).map(|entity| entity.position);
        let (bx, by, bz) = bot.location;
        let nearest = alive
            .iter()
            .copied()
            .filter_map(|id| Some((id, at(&bot, id)?)))
            .min_by(|(_, a), (_, b)| {
                let da = (a.0 - bx).powi(2) + (a.1 - by).powi(2) + (a.2 - bz).powi(2);
                let db = (b.0 - bx).powi(2) + (b.1 - by).powi(2) + (b.2 - bz).powi(2);
                da.total_cmp(&db)
            });
        let Some((id, (x, y, z))) = nearest else {
            bot.idle(Duration::from_millis(250)).await.unwrap();
            continue;
        };
        let above = if bot.entities[&id].kind == entity_types::RAVAGER {
            2.4
        } else {
            2.0
        };
        bot.fly_to((x, y + above, z), (0.0, 90.0), 1.0)
            .await
            .unwrap();
        bot.attack(id).await.unwrap();
        bot.idle(Duration::from_millis(650)).await.unwrap();
    }
    println!(
        "  {} waves of {:?} raiders, the raid over {:.0} s after the drink",
        waves.len(),
        waves,
        raid.elapsed().as_secs_f64()
    );
    say_the_growth(&cluster, &marks_raid, bytes_raid);
    say_and_bound_the_ticks(&cluster, &marks_raid);
    assert_eq!(waves.len(), 3, "a raid on easy has three waves: {waves:?}");
    cluster.terminate().await;
}
