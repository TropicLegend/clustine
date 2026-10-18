//! End-to-end tests of step N4's falling blocks and drops, written from
//! `docs/groundwork/n4-plan.md` (section 1, what a player meets) and the porting brief
//! (sections 6 to 9) by someone who did not write the code: sand a player undermines
//! falls as an entity both players are shown, and lands as a block, shown at once and
//! as a transient block; a creative player's break drops nothing; a torch whose
//! support goes drops a torch item that both are shown falling and lying on the
//! ground, and that is there once, where it lay, after the server is stopped and
//! started.
//!
//! With `CLUSTINE_TEST_PINS` set, the world is divided as the other end-to-end tests
//! divide it, and the spawn, near which this happens, is at a pinned line.

mod common;

use std::time::Duration;

use clustine::Config;
use clustine_botswarm::{Bot, SeenEntity};
use clustine_data::{blocks, entity_types, items};
use clustine_protocol::packet_ids::play::clientbound;
use clustine_protocol::packets::play::face;

use common::{VIEW_DISTANCE, config, start_with, view_area};

const PATIENCE: Duration = Duration::from_secs(30);

/// The top of the flat world's grass, where things come to lie.
const FLOOR: f64 = -60.0;

async fn join(address: &str, name: &str) -> Bot {
    let mut bot = Bot::join(address, name).await.unwrap();
    let count = view_area((0, 0), VIEW_DISTANCE).len();
    bot.wait_for_chunks(count, PATIENCE).await.unwrap();
    bot
}

fn state(block: clustine_data::BlockState) -> Option<i32> {
    Some(i32::from(block.0))
}

async fn sees_block(bot: &mut Bot, (x, y, z): (i32, i32, i32), block: Option<i32>) {
    bot.wait_until(PATIENCE, |bot| bot.block_at(x, y, z).unwrap() == block)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error}: the block at {:?} is {:?}, not {block:?}",
                (x, y, z),
                bot.block_at(x, y, z)
            )
        });
}

/// A round trip: the server has handled everything the bot sent before, and the bot
/// has read what came before the answer.
async fn round_trip(bot: &mut Bot) {
    let last = bot.dig(8, -64, 8).await.unwrap();
    bot.wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= last)
        .await
        .unwrap();
}

fn received(bot: &Bot, id: i32) -> u32 {
    let name = clientbound::NAMES[usize::try_from(id).unwrap()];
    bot.stats.received.get(name).copied().unwrap_or(0)
}

fn items_of(bot: &Bot) -> Vec<&SeenEntity> {
    bot.entities
        .values()
        .filter(|entity| entity.kind == entity_types::ITEM)
        .collect()
}

/// Places a stone on the grass at `x`, `z` and `block`, taken from the creative
/// inventory into slot 6, on that; waits until `watcher` sees both.
async fn build(
    builder: &mut Bot,
    watcher: &mut Bot,
    (x, z): (i32, i32),
    item: i32,
    block: clustine_data::BlockState,
) {
    builder.select_slot(0).await.unwrap();
    builder.use_item_on(x, -61, z, face::TOP).await.unwrap();
    sees_block(watcher, (x, -60, z), state(blocks::STONE)).await;
    builder.take_from_creative_inventory(6, item).await.unwrap();
    builder.select_slot(6).await.unwrap();
    builder.use_item_on(x, -60, z, face::TOP).await.unwrap();
    sees_block(watcher, (x, -59, z), state(block)).await;
    sees_block(builder, (x, -59, z), state(block)).await;
}

/// The brief, sections 6, 7 and 9: sand whose support a creative player breaks falls
/// as an entity, which both players are shown appear and go, and lands on the grass as
/// a block, which both are shown, with a transient block of it; the break drops
/// nothing, nor does the sand, which landed.
#[tokio::test]
async fn sand_that_is_undermined_falls_as_an_entity_and_lands_as_a_block_for_everyone() {
    let (server, address) = start_with(config()).await;
    let mut builder = join(&address, "Builder").await;
    let mut watcher = join(&address, "Watcher").await;
    build(
        &mut builder,
        &mut watcher,
        (3, 1),
        items::SAND,
        blocks::SAND,
    )
    .await;
    round_trip(&mut builder).await;
    round_trip(&mut watcher).await;
    let before: Vec<(u32, u32, u32)> = [&builder, &watcher]
        .iter()
        .map(|bot| {
            (
                bot.stats.entities_spawned,
                bot.stats.entities_removed,
                received(bot, clientbound::ADD_TRANSIENT_BLOCK),
            )
        })
        .collect();

    builder.dig(3, -60, 1).await.unwrap();
    for bot in [&mut builder, &mut watcher] {
        sees_block(bot, (3, -59, 1), state(blocks::AIR)).await;
        sees_block(bot, (3, -60, 1), state(blocks::SAND)).await;
        bot.wait_until(PATIENCE, |bot| {
            received(bot, clientbound::ADD_TRANSIENT_BLOCK) > 0
                && bot
                    .entities
                    .values()
                    .all(|entity| entity.kind != entity_types::FALLING_BLOCK)
        })
        .await
        .unwrap();
        round_trip(bot).await;
    }
    for (bot, (spawned, removed, transient)) in [&builder, &watcher].into_iter().zip(before) {
        assert_eq!(
            bot.stats.entities_spawned,
            spawned + 1,
            "one entity appeared, the falling sand: {}",
            bot.report()
        );
        assert_eq!(bot.stats.entities_removed, removed + 1, "and went");
        assert_eq!(
            received(bot, clientbound::ADD_TRANSIENT_BLOCK),
            transient + 1
        );
        assert!(
            items_of(bot).is_empty(),
            "nothing dropped: {}",
            bot.report()
        );
    }
    let latecomer = join(&address, "Latecomer").await;
    assert_eq!(latecomer.block_at(3, -60, 1).unwrap(), state(blocks::SAND));
    assert_eq!(latecomer.block_at(3, -59, 1).unwrap(), state(blocks::AIR));
    server.stop().await;
}

/// Waits until `bot` shows exactly one item, on the ground at the floor, where it was
/// through 45 round trips, each a tick of the region at least: more than two of the
/// edge's updates of an item, which come every 20 ticks (the brief, section 9), so
/// that it has stopped sliding. Returns it.
async fn lying_item(bot: &mut Bot) -> SeenEntity {
    let lying = |bot: &Bot| {
        let shown = items_of(bot);
        (shown.len() == 1 && shown[0].on_ground && (shown[0].position.1 - FLOOR).abs() < 1e-6)
            .then(|| shown[0].position)
    };
    loop {
        bot.wait_until(PATIENCE, |bot| lying(bot).is_some())
            .await
            .unwrap_or_else(|error| panic!("{error}: {}", bot.report()));
        let before = lying(bot);
        for _ in 0..45 {
            round_trip(bot).await;
        }
        if lying(bot) == before {
            return items_of(bot)[0].clone();
        }
    }
}

fn on_disk(directory: &std::path::Path) -> Config {
    Config {
        world: Some(directory.to_owned()),
        ..config()
    }
}

/// The brief, sections 8.1 and 8.4, and the plan, section 1: a torch whose support a
/// creative player breaks breaks too and drops a torch item, popped out within a
/// quarter of a block of where it stood, which both players are shown fall onto the
/// grass and lie there, the same for both; the creative break drops nothing more. The
/// server is stopped and started, and the item is there once, where it lay.
#[tokio::test]
async fn a_torch_whose_support_is_broken_drops_an_item_everyone_sees_land_and_it_outlasts_a_restart()
 {
    let directory = tempfile::tempdir().unwrap();
    let (server, address) = start_with(on_disk(directory.path())).await;
    let mut builder = join(&address, "Builder").await;
    let mut watcher = join(&address, "Watcher").await;
    build(
        &mut builder,
        &mut watcher,
        (3, 3),
        items::TORCH,
        blocks::TORCH,
    )
    .await;
    builder.dig(3, -60, 3).await.unwrap();
    let mut seen = Vec::new();
    for bot in [&mut builder, &mut watcher] {
        sees_block(bot, (3, -59, 3), state(blocks::AIR)).await;
        let item = lying_item(bot).await;
        let (x, _, z) = item.position;
        // Popped out within a quarter of a block of the torch's middle, it flies at
        // under 0.1 a tick sideways for the dozen ticks or so of its hop up and fall
        // (0.2 up, 0.04 down a tick), and slides a little on the grass.
        let throw = 1.6;
        assert!(
            (3.25 - throw..3.75 + throw).contains(&x) && (3.25 - throw..3.75 + throw).contains(&z),
            "where the torch stood, give or take its throw: {item:?}"
        );
        seen.push(item);
    }
    assert_eq!(seen[0].uuid, seen[1].uuid);
    let off = |a: (f64, f64, f64), b: (f64, f64, f64)| (a.0 - b.0).abs().max((a.2 - b.2).abs());
    assert!(
        off(seen[0].position, seen[1].position) < 1.0 / 256.0,
        "{seen:?}"
    );
    for bot in [&mut builder, &mut watcher] {
        round_trip(bot).await;
        assert_eq!(
            items_of(bot).len(),
            1,
            "one drop, the torch's: {}",
            bot.report()
        );
    }
    let lay = seen[0].clone();
    drop(builder);
    drop(watcher);
    server.stop().await;

    let (server, address) = start_with(on_disk(directory.path())).await;
    let mut again = join(&address, "Watcher").await;
    // Shown as it lies; whether a client shown a lying item in a chunk is told it is
    // on the ground before the region next speaks is the edge's test of its own.
    again
        .wait_until(PATIENCE, |bot| items_of(bot).len() == 1)
        .await
        .unwrap_or_else(|error| panic!("{error}: {}", again.report()));
    let item = items_of(&again)[0].clone();
    assert_eq!(item.uuid, lay.uuid);
    assert!(
        off(item.position, lay.position) < 1.0 / 256.0,
        "{item:?} after {lay:?}"
    );
    round_trip(&mut again).await;
    assert_eq!(items_of(&again).len(), 1);
    server.stop().await;
}
