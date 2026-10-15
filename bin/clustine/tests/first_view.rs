//! What is measured for T8 (`docs/groundwork/t8-plan.md`, "Speed";
//! ADR-0021, section 5): whether the home region's ticks keep their 50 ms while a first
//! view is made from nothing. A server is started on a new world of a seed
//! (`CLUSTINE_FIRST_VIEW_LAND`, 13579 unless said), whose store has the chunks around
//! the spawn made as it starts (T8f), and a player joins as soon as it listens, with a
//! view distance of 8: their view is made while they wait for it, and the region ticks
//! meanwhile. Printed: how long the view took to come whole, and every report of
//! ticks that took longer than a tick has, which fails the measurement.
//!
//! ```text
//! cargo test --release -p clustine --test first_view -- --ignored --nocapture
//! ```

mod common;

use std::time::{Duration, Instant};

use clustine_botswarm::Bot;
use clustine_world::ChunkGenerator;
use clustine_worldgen::scheduler::{Options, Scheduler};
use clustine_worldgen::{FeatureStages, TerrainGenerator};

use common::{free_address, spawn_server_with_log, view_area};

/// The view distance of the measurement: what an edge grants by default.
const VIEW: i32 = 8;

/// How long the view may take before the measurement gives up on it.
const PATIENCE: Duration = Duration::from_secs(120);

/// What the worker says of ticks that took too long.
const SLOW: &str = "ticks took longer than the 50 ms a tick has";

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement; run it by name with --release --ignored --nocapture"]
async fn measured_the_ticks_of_the_home_region_while_a_first_view_is_made() {
    let land: i64 = std::env::var("CLUSTINE_FIRST_VIEW_LAND")
        .map(|land| land.parse().expect("CLUSTINE_FIRST_VIEW_LAND is a number"))
        .unwrap_or(13579);
    // Where the store puts the spawn: on the land with features, without templates, as
    // a server started without a jar of the game's has it.
    let spawn = Scheduler::new(
        FeatureStages::new(std::sync::Arc::new(TerrainGenerator::new(land))),
        Options::for_processors(2),
    )
    .spawn();
    let directory = tempfile::Builder::new()
        .prefix("first-view")
        .tempdir()
        .unwrap();
    let world = directory.path().join("world");
    let log = directory.path().join("log");
    let address = free_address().await;
    let seed = land.to_string();
    let view = VIEW.to_string();
    let mut server = spawn_server_with_log(
        &address,
        &world,
        &["--seed", &seed, "--view-distance", &view],
        &log,
    )
    .await;

    let started = Instant::now();
    let mut bot = Bot::join(&address, "Newcomer").await.unwrap();
    let area = view_area((spawn.chunk().x, spawn.chunk().z), VIEW);
    bot.wait_until(PATIENCE, |bot| {
        area.iter().all(|chunk| bot.chunks.contains_key(chunk))
    })
    .await
    .unwrap_or_else(|error| panic!("{error}: {} of {} chunks", bot.chunks.len(), area.len()));
    let whole = started.elapsed();
    // The region goes on ticking with the player in it a little longer, as it would.
    bot.idle(Duration::from_secs(2)).await.unwrap();
    drop(bot);
    server.kill().await.unwrap();

    let said = std::fs::read_to_string(&log).unwrap();
    let slow: Vec<&str> = said.lines().filter(|line| line.contains(SLOW)).collect();
    println!(
        "seed {land}: spawn {}, {}, {}; a view of {VIEW} ({} chunks) whole after {:.2} s",
        spawn.x,
        spawn.y,
        spawn.z,
        area.len(),
        whole.as_secs_f64()
    );
    for line in &slow {
        println!("{line}");
    }
    println!("{} reports of slow ticks", slow.len());
    assert!(slow.is_empty(), "ticks took longer than 50 ms");
}
