//! Comparison 11 of N9 with the official server (`docs/groundwork/n9-plan.md`, section
//! 11): natural spawning, by distribution.
//!
//! Both servers serve the classic flat world of the same seed (decision 3 of section
//! 18: the slime chunks are the seed's), grant and simulate the same view distance,
//! and are given no command. One creative player stands in the open at the world's
//! spawn for a whole day, 24 000 ticks, from the morning a new world begins in; every
//! mob it is shown is counted once, when first added, by its type and by whether it
//! stands on the grass, and the most monsters shown at once are counted at night. The counts of each
//! type are held to each other within their noise (`counts_agree`), and so are the
//! peaks of monsters. A peak passes the monsters' cap of 70 about a player (the cap
//! scales with the chunks spawning reaches, 289 at a view distance of 8) by what one
//! tick's groups add, since the game checks the cap once a tick before it spawns: the
//! official server's were 74 and 75 (2026-10-29).
//!
//! Clustine's flat world has the seed 0 (N9h) and spawns by the game's rules (S10). It
//! takes some forty minutes, a day on each server; run it alone. The test checks that
//! the two logins name the same seed (their hashed seeds), so a wrong [`SEED`] fails at
//! once rather than after the day.
//!
//! The official server is started by `common/n9_oracle.rs`, since the harness's oracle
//! serves no flat world with a seed and turns spawning off by a command.
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test spawning_oracle -- --ignored official_server --nocapture
//! ```

mod common;
#[path = "common/n9_oracle.rs"]
mod n9_oracle;
#[path = "common/n9_scene.rs"]
#[allow(dead_code)]
mod n9_scene;

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use anyhow::{Context, Result};
use clustine_data::entity_types;
use clustine_protocol::packets::play::ClientboundPlay;
use tokio::time::Instant;

use n9_oracle::FlatSeeded;
use n9_scene::Scene;

/// The flat world's seed on both servers: the one Clustine gives a flat world it is told
/// none for.
const SEED: i64 = 0;

/// The view and simulation distance of both: the game's 289 spawning chunks about a
/// player at 8, which the monsters' cap of 70 is reckoned for.
const VIEW_DISTANCE: u32 = 8;

/// A day of the game, in ticks.
const DAY: i64 = 24_000;

/// The longest a day may take: a server at half the game's pace still finishes. Clustine's
/// debug build ticks a full cap of monsters at about 16 a second.
const DAY_AT_MOST: Duration = Duration::from_secs(2 * 24_000 / 20);

/// The monsters of a flat plains world, by type: what the cap of 70 counts.
const MONSTERS: [i32; 9] = [
    entity_types::ZOMBIE,
    entity_types::ZOMBIE_VILLAGER,
    entity_types::SKELETON,
    entity_types::CREEPER,
    entity_types::SPIDER,
    entity_types::ENDERMAN,
    entity_types::WITCH,
    entity_types::SLIME,
    entity_types::HUSK,
];

/// What a day showed.
#[derive(Debug, Default)]
struct Day {
    hashed_seed: i64,
    /// Mobs added, by type, and of those how many stood on the grass (y = -60).
    added: BTreeMap<i32, usize>,
    on_the_grass: BTreeMap<i32, usize>,
    /// The most monsters shown at once.
    peak_monsters: usize,
    /// The game time the day began and ended at, as the server said it.
    game_time: (i64, i64),
}

/// Whether the kind is a mob a spawner makes, rather than a projectile, an item or a
/// player.
fn counted(kind: i32) -> bool {
    ![
        entity_types::PLAYER,
        entity_types::ITEM,
        entity_types::EXPERIENCE_ORB,
        entity_types::ARROW,
        entity_types::SPLASH_POTION,
    ]
    .contains(&kind)
}

async fn a_day(address: &str) -> Result<Day> {
    let mut scene = Scene::join(address, "Stander").await?;
    let mut day = Day {
        hashed_seed: scene.hashed_seed,
        ..Day::default()
    };
    // The shown monsters, counted as they come and go.
    let mut shown: BTreeSet<i32> = scene
        .known
        .iter()
        .filter(|(_, kind)| MONSTERS.contains(kind))
        .map(|(id, _)| *id)
        .collect();
    // Each entity counted once, when it is first shown: one that walks out of view and
    // back is added again, and the two servers' views differ in that (the edge shows an
    // entity as far as its chunk is in view, the game as far as its type's tracking
    // range), which is not spawning.
    let mut counted_ids: BTreeSet<i32> = BTreeSet::new();
    // A day of the server's own clock, not of the wall's: a server that ticks slower
    // than 20 a second would otherwise be watched for less of a night (a run that
    // stopped at 19 800 found two thirds of the game's monsters).
    let began = scene
        .times
        .last()
        .map(|time| time.game_time)
        .context("the join's time")?;
    let mut now = began;
    let given_up = Instant::now() + DAY_AT_MOST;
    while now < began + DAY {
        anyhow::ensure!(
            Instant::now() < given_up,
            "the server's day had only reached {} of {DAY} ticks after {DAY_AT_MOST:?}",
            now - began
        );
        scene.idle(Duration::from_secs(10)).await?;
        for seen in &scene.log {
            if let Some(ClientboundPlay::SetTime(time)) = &seen.packet {
                now = now.max(time.game_time);
            }
        }
        for seen in &scene.log {
            match &seen.packet {
                Some(ClientboundPlay::SpawnEntity(spawn)) if counted(spawn.kind) => {
                    if counted_ids.insert(spawn.entity_id) {
                        *day.added.entry(spawn.kind).or_default() += 1;
                        if (spawn.y - -60.0).abs() < 1e-6 {
                            *day.on_the_grass.entry(spawn.kind).or_default() += 1;
                        }
                    }
                    if MONSTERS.contains(&spawn.kind) {
                        shown.insert(spawn.entity_id);
                        day.peak_monsters = day.peak_monsters.max(shown.len());
                    }
                }
                Some(ClientboundPlay::RemoveEntities(removed)) => {
                    for id in &removed.entity_ids {
                        shown.remove(id);
                    }
                }
                _ => {}
            }
        }
        // What was read is not needed again; a day's packets would fill memory.
        scene.log.clear();
    }
    day.game_time = (began, now);
    Ok(day)
}

/// Whether two counts of one type agree within their noise: their difference within
/// three deviations of the difference of two Poisson counts (`√(a + b)`), and two
/// besides for the smallest counts.
fn counts_agree(theirs: usize, ours: usize) -> bool {
    let (a, b) = (theirs as f64, ours as f64);
    (a - b).abs() <= 3.0 * (a + b).sqrt() + 2.0
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_day_of_spawning_is_like_on_the_official_server() {
    let oracle = FlatSeeded::start(SEED, VIEW_DISTANCE)
        .await
        .expect("the official server starts");
    let mut config = common::config();
    // This comparison is of natural spawning: the game rule on.
    clustine_worker::set_spawn_mobs(true);
    config.view_distance = VIEW_DISTANCE as i32;
    let (server, address) = common::start_with(config).await;
    // The seeds first, so that a wrong one fails before the day.
    let theirs_seed = Scene::join(oracle.address(), "Seed")
        .await
        .unwrap()
        .hashed_seed;
    let ours_seed = Scene::join(&address, "Seed").await.unwrap().hashed_seed;
    assert_eq!(
        ours_seed, theirs_seed,
        "the two worlds' seeds differ: set SEED to Clustine's flat world's"
    );
    server.stop().await;
    // Each day on a world no player has been in.
    let theirs = a_day(oracle.address())
        .await
        .expect("the official server's day");
    drop(oracle);
    let mut config = common::config();
    // This comparison is of natural spawning: the game rule on.
    clustine_worker::set_spawn_mobs(true);
    config.view_distance = VIEW_DISTANCE as i32;
    let (server, address) = common::start_with(config).await;
    let ours = a_day(&address).await.expect("Clustine's day");
    server.stop().await;
    println!("official {theirs:#?}\nClustine {ours:#?}");
    assert_eq!(ours.hashed_seed, theirs.hashed_seed, "the days' seeds");

    let kinds: BTreeSet<i32> = theirs
        .added
        .keys()
        .chain(ours.added.keys())
        .copied()
        .collect();
    let mut differ = Vec::new();
    for kind in kinds {
        let (a, b) = (
            theirs.added.get(&kind).copied().unwrap_or(0),
            ours.added.get(&kind).copied().unwrap_or(0),
        );
        let (ga, gb) = (
            theirs.on_the_grass.get(&kind).copied().unwrap_or(0),
            ours.on_the_grass.get(&kind).copied().unwrap_or(0),
        );
        if !counts_agree(a, b) || !counts_agree(ga, gb) {
            differ.push(format!(
                "{}: {a} ({ga} on the grass) against {b} ({gb})",
                clustine_data::ENTITY_TYPES[kind as usize]
            ));
        }
    }
    assert!(differ.is_empty(), "these differ: {differ:#?}");
    assert!(
        counts_agree(theirs.peak_monsters, ours.peak_monsters),
        "the most monsters at once: {} against {}",
        theirs.peak_monsters,
        ours.peak_monsters
    );
}

#[test]
fn counts_agree_within_their_noise() {
    assert!(counts_agree(100, 120));
    assert!(!counts_agree(100, 160));
    assert!(counts_agree(0, 2));
    // Ten against none is within three deviations of ten; twenty is not.
    assert!(counts_agree(0, 10));
    assert!(!counts_agree(0, 20));
}
