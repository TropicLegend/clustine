//! Breaking blocks: the change reaches everyone and the one who made it is told.

mod common;

use std::time::Duration;

use clustine::Config;
use clustine_botswarm::Bot;
use clustine_data::blocks;

use common::{VIEW_DISTANCE, config, start, start_with, view_area};

const PATIENCE: Duration = Duration::from_secs(30);

const AIR: Option<i32> = Some(blocks::AIR.0 as i32);
const GRASS: Option<i32> = Some(blocks::GRASS_BLOCK.0 as i32);
const DIRT: Option<i32> = Some(blocks::DIRT.0 as i32);

/// Joins and waits for the chunks around the spawn point.
async fn join(address: &str, name: &str) -> Bot {
    let mut bot = Bot::join(address, name).await.unwrap();
    let count = view_area((0, 0), VIEW_DISTANCE).len();
    bot.wait_for_chunks(count, PATIENCE).await.unwrap();
    bot
}

/// Waits until `bot` knows the block at `position` to be `state`.
async fn sees_block(bot: &mut Bot, position: (i32, i32, i32), state: Option<i32>) {
    let (x, y, z) = position;
    bot.wait_until(PATIENCE, |bot| bot.block_at(x, y, z).unwrap() == state)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error}: the block at {position:?} is {:?}, not {state:?}",
                bot.block_at(x, y, z)
            )
        });
}

/// Breaking a block removes it for the one who broke it, for a bystander and for
/// someone who joins later. Checked over a direct and a serialising edge/worker link.
#[tokio::test]
async fn a_broken_block_is_gone_for_everyone() {
    for serialise_link in [false, true] {
        let (server, address) = start_with(Config {
            serialise_link,
            ..config()
        })
        .await;
        let mut digger = join(&address, "Digger").await;
        let mut bystander = join(&address, "Bystander").await;

        // The top layer of the flat world, two blocks from where players stand.
        let target = (2, -61, 1);
        assert_eq!(digger.block_at(2, -61, 1).unwrap(), GRASS);
        let sequence = digger.dig(2, -61, 1).await.unwrap();

        sees_block(&mut digger, target, AIR).await;
        sees_block(&mut bystander, target, AIR).await;
        digger
            .wait_until(PATIENCE, |bot| bot.acknowledged_sequence == sequence)
            .await
            .unwrap();
        // Nothing else changed.
        assert_eq!(digger.block_at(2, -62, 1).unwrap(), DIRT);
        assert_eq!(bystander.block_at(3, -61, 1).unwrap(), GRASS);
        assert_eq!(bystander.acknowledged_sequence, 0);

        // Digging on: the layer below is now exposed.
        digger.dig(2, -62, 1).await.unwrap();
        sees_block(&mut bystander, (2, -62, 1), AIR).await;

        // Someone joining now is sent the chunk as it is, not as it was generated.
        let latecomer = join(&address, "Latecomer").await;
        assert_eq!(latecomer.block_at(2, -61, 1).unwrap(), AIR);
        assert_eq!(latecomer.block_at(2, -62, 1).unwrap(), AIR);
        assert_eq!(latecomer.block_at(2, -63, 1).unwrap(), DIRT);

        server.stop().await;
    }
}

#[tokio::test]
async fn breaking_out_of_reach_is_acknowledged_without_effect() {
    let (server, address) = start().await;
    let mut digger = join(&address, "Digger").await;
    let mut bystander = join(&address, "Bystander").await;

    // Thirty blocks away, far beyond arm's length, and then thin air.
    let far = digger.dig(30, -61, 0).await.unwrap();
    let air = digger.dig(0, -55, 0).await.unwrap();
    assert_eq!((far, air), (1, 2));
    digger
        .wait_until(PATIENCE, |bot| bot.acknowledged_sequence == air)
        .await
        .unwrap();

    bystander.idle(Duration::from_millis(200)).await.unwrap();
    for bot in [&digger, &bystander] {
        assert_eq!(bot.block_at(30, -61, 0).unwrap(), GRASS);
    }

    server.stop().await;
}

/// The change is in the world, not just on the screens of those who watched it.
#[tokio::test]
async fn a_hole_is_still_there_after_walking_away_and_back() {
    let (server, address) = start().await;
    let mut digger = join(&address, "Digger").await;
    // Someone has to stay: nothing is stored yet, so a chunk nobody has in view is
    // generated afresh the next time it is needed.
    let _resident = join(&address, "Resident").await;

    digger.dig(1, -61, 0).await.unwrap();
    sees_block(&mut digger, (1, -61, 0), AIR).await;

    digger.walk_to(200.5, 0.5, 2.0).await.unwrap();
    digger
        .wait_until(PATIENCE, |bot| !bot.chunks.contains_key(&(0, 0)))
        .await
        .unwrap();
    assert_eq!(digger.block_at(1, -61, 0).unwrap(), None);

    digger.walk_to(0.5, 0.5, 2.0).await.unwrap();
    sees_block(&mut digger, (1, -61, 0), AIR).await;

    server.stop().await;
}

#[tokio::test]
async fn many_blocks_broken_at_once_all_arrive() {
    let (server, address) = start().await;
    let mut digger = join(&address, "Digger").await;
    let mut bystander = join(&address, "Bystander").await;

    let mut last = 0;
    for x in -3..=3 {
        for z in -3..=3 {
            last = digger.dig(x, -61, z).await.unwrap();
        }
    }
    digger
        .wait_until(PATIENCE, |bot| bot.acknowledged_sequence == last)
        .await
        .unwrap();
    for x in -3..=3 {
        for z in -3..=3 {
            sees_block(&mut bystander, (x, -61, z), AIR).await;
            sees_block(&mut digger, (x, -61, z), AIR).await;
        }
    }
    assert_eq!(bystander.block_at(4, -61, 0).unwrap(), GRASS);

    server.stop().await;
}
