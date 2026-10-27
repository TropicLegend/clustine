//! Containers end to end (step N6b): a chest placed by one player opens for them, a stack
//! moved into it by clicks stays there, and another player who opens it is shown it.

mod common;

use std::time::Duration;

use clustine_botswarm::Bot;
use clustine_data::{blocks, items};
use clustine_protocol::item::ItemStack;
use clustine_protocol::packets::play::{click, face, menu};
use clustine_protocol::text::Text;

use common::{VIEW_DISTANCE, start, view_area};

const PATIENCE: Duration = Duration::from_secs(30);

/// Where the chest stands: on the flat world's grass, beside the spawn.
const CHEST: (i32, i32, i32) = (2, -60, 1);

async fn join(address: &str, name: &str) -> Bot {
    let mut bot = Bot::join(address, name).await.unwrap();
    let count = view_area((0, 0), VIEW_DISTANCE).len();
    bot.wait_for_chunks(count, PATIENCE).await.unwrap();
    bot
}

/// `bot` clicks the chest with an empty hand and waits for its window.
async fn open(bot: &mut Bot) {
    bot.empty_hotbar_slot(8).await.unwrap();
    bot.select_slot(8).await.unwrap();
    let (x, y, z) = CHEST;
    bot.use_item_on(x, y, z, face::TOP).await.unwrap();
    bot.wait_until(PATIENCE, |bot| {
        bot.menu.as_ref().is_some_and(|menu| menu.slots.len() == 63)
    })
    .await
    .unwrap_or_else(|error| panic!("{error}: the chest's window never came: {:?}", bot.menu));
}

#[tokio::test]
async fn a_stack_put_into_a_chest_is_there_for_whoever_opens_it_next() {
    let (_server, address) = start().await;
    let mut builder = join(&address, "Builder").await;

    builder
        .take_from_creative_inventory(0, items::CHEST)
        .await
        .unwrap();
    builder.select_slot(0).await.unwrap();
    let (x, y, z) = CHEST;
    builder.use_item_on(x, y - 1, z, face::TOP).await.unwrap();
    builder
        .wait_until(PATIENCE, |bot| {
            bot.block_at(x, y, z).unwrap().is_some_and(|state| {
                clustine_data::BlockState(state as u16).block_id() == blocks::CHEST.block_id()
            })
        })
        .await
        .unwrap();

    open(&mut builder).await;
    let shown = builder.menu.clone().expect("open");
    assert_eq!(shown.menu, menu::GENERIC_9X3);
    assert_eq!(shown.title, Text::translatable("container.chest"));
    assert!(shown.slots[..27].iter().all(Option::is_none), "{shown:?}");

    // Three stone from the hotbar's second slot (after the chest's 27 and the main
    // inventory's 27, at 55) into the chest's first slot, as a client clicks them.
    builder
        .take_from_creative_inventory(1, items::STONE)
        .await
        .unwrap();
    let stone = ItemStack::new(items::STONE, 1);
    builder
        .click_window(55, 0, click::PICKUP, vec![(55, None)], Some(stone.clone()))
        .await
        .unwrap();
    builder
        .click_window(0, 0, click::PICKUP, vec![(0, Some(stone.clone()))], None)
        .await
        .unwrap();
    builder.close_window().await.unwrap();

    let mut visitor = join(&address, "Visitor").await;
    open(&mut visitor).await;
    let seen = visitor.menu.clone().expect("open");
    assert_eq!(seen.slots[0], Some(stone), "{seen:?}");
    assert_eq!(visitor.cursor, None);
}
