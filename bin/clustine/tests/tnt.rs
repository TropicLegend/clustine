//! TNT end to end (step N6c): a block of TNT lit with flint and steel burns as primed TNT
//! that everyone is shown, explodes, and everyone near is sent the explosion and shown the
//! hole it made.

mod common;

use std::time::Duration;

use clustine_botswarm::Bot;
use clustine_data::{blocks, entity_types, items};
use clustine_protocol::packets::play::face;

use common::{VIEW_DISTANCE, start, view_area};

const PATIENCE: Duration = Duration::from_secs(30);

/// Where the TNT stands: on the flat world's grass, a few blocks from the spawn.
const TNT: (i32, i32, i32) = (4, -60, 4);

async fn join(address: &str, name: &str) -> Bot {
    let mut bot = Bot::join(address, name).await.unwrap();
    let count = view_area((0, 0), VIEW_DISTANCE).len();
    bot.wait_for_chunks(count, PATIENCE).await.unwrap();
    bot
}

#[tokio::test]
async fn tnt_lit_with_flint_and_steel_burns_explodes_and_leaves_a_hole() {
    let (_server, address) = start().await;
    let mut lighter = join(&address, "Lighter").await;
    let mut watcher = join(&address, "Watcher").await;
    let (x, y, z) = TNT;

    lighter
        .take_from_creative_inventory(0, items::TNT)
        .await
        .unwrap();
    lighter.select_slot(0).await.unwrap();
    lighter.use_item_on(x, y - 1, z, face::TOP).await.unwrap();
    lighter
        .wait_until(PATIENCE, |bot| {
            bot.block_at(x, y, z).unwrap() == Some(i32::from(blocks::TNT.0))
        })
        .await
        .unwrap();

    lighter
        .take_from_creative_inventory(1, items::FLINT_AND_STEEL)
        .await
        .unwrap();
    lighter.select_slot(1).await.unwrap();
    lighter.use_item_on(x, y, z, face::TOP).await.unwrap();

    // The block goes and primed TNT burns in its place, for the watcher too.
    watcher
        .wait_until(PATIENCE, |bot| {
            bot.entities
                .values()
                .any(|entity| entity.kind == entity_types::TNT)
        })
        .await
        .unwrap_or_else(|error| panic!("{error}: the watcher was never shown primed TNT"));
    // The game spawns it before it sets the block to air, in the same tick.
    watcher
        .wait_until(PATIENCE, |bot| {
            bot.block_at(x, y, z).unwrap() == Some(i32::from(blocks::AIR.0))
        })
        .await
        .unwrap_or_else(|error| panic!("{error}: the TNT block was never taken away"));

    // Eighty ticks later it explodes, and the grass under it goes.
    for bot in [&mut lighter, &mut watcher] {
        bot.wait_until(PATIENCE, |bot| !bot.explosions.is_empty())
            .await
            .unwrap_or_else(|error| panic!("{error}: no explosion was shown"));
        let explosion = &bot.explosions[0];
        assert_eq!(explosion.radius, 4.0);
        let [cx, cy, cz] = explosion.centre;
        assert!(
            (cx - 4.5).abs() < 0.5 && (cz - 4.5).abs() < 0.5,
            "{cx} {cy} {cz}"
        );
        bot.wait_until(PATIENCE, |bot| {
            bot.block_at(x, y - 1, z).unwrap() == Some(i32::from(blocks::AIR.0))
        })
        .await
        .unwrap_or_else(|error| panic!("{error}: the grass under the TNT is still there"));
        assert!(
            bot.entities
                .values()
                .all(|entity| entity.kind != entity_types::TNT),
            "the primed TNT is gone"
        );
    }
}
