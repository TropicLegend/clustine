//! Where a newcomer enters a world with a seed, tested from the T8 plan
//! (`docs/groundwork/t8-plan.md`, T8c and T8d): on the column of the world's spawn,
//! which the game's rule puts away from the origin for seed 13579, on dry ground, and
//! there again when the world is started again.

mod common;

use std::path::Path;
use std::time::Duration;

use clustine::{Config, Land, Server};
use clustine_botswarm::Bot;
use clustine_data::BlockState;

use common::{config, view_area};

const PATIENCE: Duration = Duration::from_secs(60);

/// A world whose spawn is away from the origin: the origin's column is the sea at a
/// coast, and the game puts the spawn at block -96, 66, 32.
const SEED: i64 = 13579;

/// Where a newcomer's feet are: the middle of the bottom of the spawn block.
const ENTERED: (f64, f64, f64) = (-95.5, 66.0, 32.5);

/// The chunk of the spawn block.
const SPAWN_CHUNK: (i32, i32) = (-6, 2);

/// The smallest a server grants, so that the test makes few chunks.
const VIEW: i32 = 2;

fn on_disk(world: &Path) -> Config {
    Config {
        world: Some(world.to_owned()),
        view_distance: VIEW,
        ..config()
    }
}

async fn start(world: &Path) -> (Server, String) {
    let server = match Server::start_in(on_disk(world), Land::of_seed(SEED)).await {
        Ok(server) => server,
        Err(error) => panic!("the server did not start: {error:#}"),
    };
    let address = server.address().to_string();
    (server, address)
}

/// Joins and waits for the chunks around the spawn's.
async fn join(address: &str, name: &str) -> Bot {
    let mut bot = loop {
        match Bot::join(address, name).await {
            Ok(bot) => break bot,
            // Until the server has noticed that the one before left.
            Err(_) => tokio::task::yield_now().await,
        }
    };
    let area = view_area(SPAWN_CHUNK, VIEW);
    bot.wait_until(PATIENCE, |bot| {
        area.iter().all(|chunk| bot.chunks.contains_key(chunk))
    })
    .await
    .unwrap_or_else(|error| panic!("{error}: {name} has {} chunks", bot.chunks.len()));
    bot
}

/// The block at a position as the client was sent it.
fn block(bot: &Bot, x: i32, y: i32, z: i32) -> BlockState {
    let state = bot
        .block_at(x, y, z)
        .unwrap()
        .unwrap_or_else(|| panic!("{x}, {y}, {z} was not sent"));
    BlockState(u16::try_from(state).unwrap())
}

/// Holds the newcomer to standing on dry ground at the spawn: under their feet a
/// block without a fluid that carries them, and no fluid where their feet and head are
/// (`docs/groundwork/t8-plan.md`, section 2, the column rule).
fn on_dry_ground(bot: &Bot) {
    let (x, feet, z) = (-96, 66, 32);
    let under = block(bot, x, feet - 1, z);
    assert!(!under.is_air(), "nothing under the feet");
    assert!(!under.has_fluid(), "a fluid under the feet: {under:?}");
    for y in [feet, feet + 1] {
        let state = block(bot, x, y, z);
        assert!(!state.has_fluid(), "a fluid at {y}: {state:?}");
        assert!(!state.props().is_solid(), "a solid block at {y}: {state:?}");
    }
}

#[tokio::test]
async fn a_newcomer_to_a_seeded_world_enters_on_dry_ground_at_its_spawn_and_again_after_a_restart()
{
    let directory = tempfile::tempdir().unwrap();
    let (server, address) = start(directory.path()).await;
    let mut first = join(&address, "First").await;
    let entered = first.entered(PATIENCE).await.unwrap();
    assert_eq!(entered.location, ENTERED);
    on_dry_ground(&first);
    drop(first);
    server.stop().await;

    // The same world started again: its spawn is kept, and the next newcomer enters
    // there too.
    let (server, address) = start(directory.path()).await;
    let mut second = join(&address, "Second").await;
    let entered = second.entered(PATIENCE).await.unwrap();
    assert_eq!(entered.location, ENTERED);
    on_dry_ground(&second);
    drop(second);
    server.stop().await;
}

/// A world kept in memory only enters newcomers at the same place: the spawn is the
/// game's rule over the seed, not something a world on disk adds.
#[tokio::test]
async fn a_newcomer_to_a_seeded_world_in_memory_enters_at_its_spawn() {
    let config = Config {
        view_distance: VIEW,
        ..config()
    };
    let server = match Server::start_in(config, Land::of_seed(SEED)).await {
        Ok(server) => server,
        Err(error) => panic!("the server did not start: {error:#}"),
    };
    let address = server.address().to_string();
    let mut bot = join(&address, "Newcomer").await;
    let entered = bot.entered(PATIENCE).await.unwrap();
    assert_eq!(entered.location, ENTERED);
    on_dry_ground(&bot);
    drop(bot);
    server.stop().await;
}
