//! The world outlives the players looking at it, and the server.

mod common;

use std::time::Duration;

use clustine::Config;
use clustine_botswarm::Bot;
use clustine_data::blocks;
use clustine_protocol::packets::play::face;

use common::{VIEW_DISTANCE, config, free_address, spawn_server, start, start_with, view_area};

const PATIENCE: Duration = Duration::from_secs(30);

const AIR: Option<i32> = Some(blocks::AIR.0 as i32);
const GRASS: Option<i32> = Some(blocks::GRASS_BLOCK.0 as i32);
const STONE: Option<i32> = Some(blocks::STONE.0 as i32);

/// Joins and waits for the chunks around the spawn point.
async fn join(address: &str, name: &str) -> Bot {
    let mut bot = Bot::join(address, name).await.unwrap();
    let count = view_area((0, 0), VIEW_DISTANCE).len();
    bot.wait_for_chunks(count, PATIENCE).await.unwrap();
    bot
}

/// Breaks one block and places another, in two different chunks, and waits until the
/// server has confirmed both.
async fn build(bot: &mut Bot) {
    bot.dig(2, -61, 1).await.unwrap();
    let last = bot.use_item_on(-3, -61, -4, face::TOP).await.unwrap();
    bot.wait_until(PATIENCE, |bot| bot.acknowledged_sequence == last)
        .await
        .unwrap();
    assert_built(bot);
}

fn assert_built(bot: &Bot) {
    assert_eq!(bot.block_at(2, -61, 1).unwrap(), AIR);
    assert_eq!(bot.block_at(-3, -60, -4).unwrap(), STONE);
    // The surroundings are as generated.
    assert_eq!(bot.block_at(3, -61, 1).unwrap(), GRASS);
    assert_eq!(bot.block_at(-3, -59, -4).unwrap(), AIR);
}

/// What players built is still there after the server has been stopped and started
/// again on the same world, even if they were online when it stopped.
#[tokio::test]
async fn the_world_survives_a_restart() {
    let directory = tempfile::tempdir().unwrap();
    let on_disk = || Config {
        world: Some(directory.path().to_owned()),
        ..config()
    };

    let (server, address) = start_with(on_disk()).await;
    let mut builder = join(&address, "Builder").await;
    build(&mut builder).await;
    server.stop().await;
    drop(builder);

    let (server, address) = start_with(on_disk()).await;
    let visitor = join(&address, "Visitor").await;
    assert_built(&visitor);
    server.stop().await;

    // And once more, to see that a start without changes keeps what is stored.
    let (server, address) = start_with(on_disk()).await;
    let visitor = join(&address, "Visitor").await;
    assert_built(&visitor);
    server.stop().await;
}

/// A chunk that nobody has in view any more is stored, not forgotten: whoever comes
/// by later finds it as it was left. This holds without a world directory too.
#[tokio::test]
async fn changes_outlast_everyone_leaving() {
    let directory = tempfile::tempdir().unwrap();
    for world in [None, Some(directory.path().to_owned())] {
        let (server, address) = start_with(Config { world, ..config() }).await;

        let mut builder = join(&address, "Builder").await;
        build(&mut builder).await;
        drop(builder);

        // The server notices the departure a moment later and unloads the chunks.
        let mut visitor = None;
        for _ in 0..100 {
            match Bot::join(&address, "Builder").await {
                Ok(bot) => {
                    visitor = Some(bot);
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
        let mut visitor = visitor.expect("rejoining never succeeded");
        let count = view_area((0, 0), VIEW_DISTANCE).len();
        visitor.wait_for_chunks(count, PATIENCE).await.unwrap();
        assert_built(&visitor);

        server.stop().await;
    }
}

/// Walking far away unloads the chunks left behind; coming back loads them as they were.
#[tokio::test]
async fn a_lone_builder_finds_the_build_again() {
    let (server, address) = start().await;
    let mut builder = join(&address, "Builder").await;
    build(&mut builder).await;

    builder.walk_to(300.5, 0.5, 2.0).await.unwrap();
    builder
        .wait_until(PATIENCE, |bot| {
            bot.center == Some((18, 0)) && !bot.chunks.contains_key(&(0, 0))
        })
        .await
        .unwrap();
    builder.walk_to(0.5, 0.5, 2.0).await.unwrap();
    builder
        .wait_until(PATIENCE, |bot| {
            bot.block_at(2, -61, 1).unwrap().is_some()
                && bot.block_at(-3, -60, -4).unwrap().is_some()
        })
        .await
        .unwrap();
    assert_built(&builder);

    server.stop().await;
}

/// Killing the server process outright loses nothing players were told had happened:
/// what was not yet saved is recovered from the write-ahead log.
#[tokio::test]
async fn the_world_survives_the_server_being_killed() {
    let directory = tempfile::tempdir().unwrap();
    let world = directory.path().join("world");
    let address = free_address().await;

    let mut server = spawn_server(&address, &world, &[]).await;
    let mut builder = join(&address, "Builder").await;
    build(&mut builder).await;
    // The changes are handed to the log before the player is told; give the store's
    // thread a moment to write them.
    tokio::time::sleep(Duration::from_millis(300)).await;
    server.kill().await.unwrap();
    drop(builder);

    // The chunks were still loaded and no checkpoint was due, so nothing but the log
    // holds the changes.
    assert!(!world.join("manifests/overworld").exists());
    let logged: u64 = std::fs::read_dir(world.join("log"))
        .unwrap()
        .map(|segment| segment.unwrap().metadata().unwrap().len())
        .sum();
    assert!(logged > 0);

    let mut server = spawn_server(&address, &world, &[]).await;
    let visitor = join(&address, "Visitor").await;
    assert_built(&visitor);
    // Opening the region saved the chunks; the log keeps the changes until a checkpoint.
    assert!(world.join("manifests/overworld").exists());

    server.kill().await.unwrap();
}
