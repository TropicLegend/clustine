//! What is measured for N6h (`docs/groundwork/n6-plan.md`, section 3): a crater of 64 TNT.
//! A bot builds a cube of 4 × 4 × 4 TNT beside the spawn of a flat world and lights its
//! top; the 64 go off in a chain. Printed: how long from the first explosion to the
//! last, how many the bot was shown, how large the world's log grew while they went
//! off, and every report of ticks that took longer than a tick has, with the longest.
//!
//! Where the world lies decides what is measured: `TMPDIR` on memory (`/dev/shm/…`) for
//! the logic, on the real disk for the disk, which on the owner's machine has a cache in
//! memory in front of it (CLAUDE.md, "Checking work"). Say which a number is from.
//!
//! ```text
//! cargo test --release -p clustine --test craters -- --ignored --nocapture
//! ```
//!
//! Measured on 2026-10-19, beside a full check and two builds: 64 explosions over 1.61
//! s, the world's files 309 KiB larger, and no tick over 50 ms, with the world in memory
//! and twice on the real disk with its cache. The disk without the cache was not
//! measured: the cache is the machine's and cannot be turned off from here.

mod common;

use std::time::{Duration, Instant};

use clustine_botswarm::Bot;
use clustine_data::{blocks, items};
use clustine_protocol::packets::play::face;

use common::{VIEW_DISTANCE, free_address, spawn_server_with_log, view_area};

const PATIENCE: Duration = Duration::from_secs(60);

/// What the worker says of ticks that took too long.
const SLOW: &str = "ticks took longer than the 50 ms a tick has";

/// The cube's corner nearest the origin, on the flat world's grass, and its side.
const CORNER: (i32, i32, i32) = (2, -60, 2);
const SIDE: i32 = 4;

/// The bytes of every file under `directory`.
fn bytes_under(directory: &std::path::Path) -> u64 {
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

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement; run it by name with --release --ignored --nocapture"]
async fn measured_a_crater_of_64_tnt() {
    let directory = tempfile::Builder::new().prefix("crater").tempdir().unwrap();
    let world = directory.path().join("world");
    let log = directory.path().join("log");
    let address = free_address().await;
    let mut server = spawn_server_with_log(&address, &world, &[], &log).await;

    let mut bot = Bot::join(&address, "Demolisher").await.unwrap();
    let count = view_area((0, 0), VIEW_DISTANCE).len();
    bot.wait_for_chunks(count, PATIENCE).await.unwrap();
    // Beside the cube's middle, within reach of every block of it.
    let (cx, cy, cz) = CORNER;
    bot.walk_to(f64::from(cx) + 2.0, f64::from(cz) - 1.5, 0.2)
        .await
        .unwrap();
    bot.put_in_hotbar(
        0,
        clustine_protocol::item::ItemStack {
            item: items::TNT,
            count: 64,
        },
    )
    .await
    .unwrap();
    bot.select_slot(0).await.unwrap();
    for y in 0..SIDE {
        for x in 0..SIDE {
            for z in 0..SIDE {
                let (px, py, pz) = (cx + x, cy + y, cz + z);
                bot.use_item_on(px, py - 1, pz, face::TOP).await.unwrap();
                bot.wait_until(PATIENCE, |bot| {
                    bot.block_at(px, py, pz).unwrap() == Some(i32::from(blocks::TNT.0))
                })
                .await
                .unwrap_or_else(|error| panic!("{error}: no TNT at {px}, {py}, {pz}"));
            }
        }
    }
    let before = bytes_under(&world);

    bot.take_from_creative_inventory(1, items::FLINT_AND_STEEL)
        .await
        .unwrap();
    bot.select_slot(1).await.unwrap();
    let top = cy + SIDE - 1;
    bot.use_item_on(cx, top, cz, face::TOP).await.unwrap();
    bot.wait_until(PATIENCE, |bot| !bot.explosions.is_empty())
        .await
        .unwrap();
    let first = Instant::now();
    // The chain is over when no explosion has come for two seconds, as the shortest
    // fuse a chained TNT gets is half a second and the longest a second and a half.
    let mut seen = bot.explosions.len();
    let mut last = Instant::now();
    while last.elapsed() < Duration::from_secs(2) {
        bot.idle(Duration::from_millis(200)).await.unwrap();
        if bot.explosions.len() > seen {
            seen = bot.explosions.len();
            last = Instant::now();
        }
    }
    let span = last.duration_since(first);
    let after = bytes_under(&world);
    drop(bot);
    server.kill().await.unwrap();

    let said = std::fs::read_to_string(&log).unwrap();
    let slow: Vec<&str> = said.lines().filter(|line| line.contains(SLOW)).collect();
    println!(
        "world in {}: {seen} explosions shown over {:.2} s; the world's files grew by {} KiB",
        directory.path().display(),
        span.as_secs_f64(),
        after.saturating_sub(before) / 1024
    );
    for line in &slow {
        println!("{line}");
    }
    println!("{} reports of slow ticks", slow.len());
}
