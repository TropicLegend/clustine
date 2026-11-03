//! What a player's game mode does to what they use (`docs/groundwork/v-plan.md`,
//! sections 4.1 and 4.6, step V1): a survival player's placed block is taken from their
//! stack, and their client is told the slot, as the game's `BlockItem.place`
//! (`ItemStack.consume`) and `broadcastChanges` do; a creative player's is not.

mod common;

use std::time::Duration;

use clustine::Config;
use clustine_botswarm::Bot;
use clustine_data::{blocks, items};
use clustine_protocol::item::ItemStack;
use clustine_protocol::packets::play::face;
use clustine_world::GameMode;

use common::{VIEW_DISTANCE, config, start_with, view_area};

const PATIENCE: Duration = Duration::from_secs(30);

const STONE: Option<i32> = Some(blocks::STONE.0 as i32);

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

fn stones(count: i32) -> Option<ItemStack> {
    Some(ItemStack::new(items::STONE, count))
}

/// A creative player places stone from a stack of ten, which stays ten; forced into
/// survival when the world starts again, they place from the same stack, which goes
/// down to nine, and their client is told so.
#[tokio::test]
async fn a_survival_player_s_placed_block_comes_off_the_stack_and_a_creative_player_s_does_not() {
    let directory = tempfile::tempdir().unwrap();
    let on_disk = |mode: GameMode, forced: bool| Config {
        world: Some(directory.path().to_owned()),
        game_mode: mode,
        force_game_mode: forced,
        ..config()
    };

    let (server, address) = start_with(on_disk(GameMode::Creative, false)).await;
    let mut builder = join(&address, "Builder").await;
    builder
        .wait_until(PATIENCE, |bot| bot.hotbar[0].is_some())
        .await
        .unwrap();
    builder
        .put_in_hotbar(0, ItemStack::new(items::STONE, 10))
        .await
        .unwrap();
    builder.select_slot(0).await.unwrap();
    // On top of the ground, two blocks from where players stand.
    let sequence = builder.use_item_on(2, -61, 1, face::TOP).await.unwrap();
    sees_block(&mut builder, (2, -60, 1), STONE).await;
    builder
        .wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= sequence)
        .await
        .unwrap();
    // A creative player keeps the stack, which the server tells them of again only when
    // they come back: ten, as before the placing.
    builder.close().await.unwrap();
    drop(builder);
    let mut builder = join(&address, "Builder").await;
    builder
        .wait_until(PATIENCE, |bot| bot.hotbar[0] == stones(10))
        .await
        .unwrap_or_else(|error| panic!("{error}: the hotbar is {:?}", builder.hotbar));
    builder.close().await.unwrap();
    drop(builder);
    server.stop().await;

    let (server, address) = start_with(on_disk(GameMode::Survival, true)).await;
    let mut builder = join(&address, "Builder").await;
    assert_eq!(
        builder.info.login.spawn.game_mode,
        clustine_protocol::packets::play::game_mode::SURVIVAL
    );
    builder
        .wait_until(PATIENCE, |bot| bot.hotbar[0] == stones(10))
        .await
        .unwrap();
    builder.select_slot(0).await.unwrap();
    builder.use_item_on(3, -61, 1, face::TOP).await.unwrap();
    sees_block(&mut builder, (3, -60, 1), STONE).await;
    builder
        .wait_until(PATIENCE, |bot| bot.hotbar[0] == stones(9))
        .await
        .unwrap_or_else(|error| panic!("{error}: the hotbar is {:?}", builder.hotbar));
    server.stop().await;
}
