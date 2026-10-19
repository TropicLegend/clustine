//! Comparisons with the official server of a chest's window (step N6b,
//! `docs/groundwork/n6-plan.md`, section 6 item 11): what a client is sent when it opens
//! a chest, when it clicks and guesses right, when it guesses wrong, when it shift-clicks
//! and when it closes the window. The bots share Clustine's codec, so these packets are
//! only really checked here.
//!
//! These tests need Java, the server jar downloaded by `cargo datagen`, and your
//! agreement to the Minecraft EULA, so they are ignored by default. Run them with:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test containers_oracle -- --ignored --nocapture --test-threads 1
//! ```
//!
//! The same bot does the same things on both servers, and every window packet each sent
//! it is compared in order, the state ids among them.

mod common;

use std::time::Duration;

use clustine_botswarm::{Bot, Oracle};
use clustine_data::{blocks, items};
use clustine_protocol::item::ItemStack;
use clustine_protocol::packets::play::{ClientboundPlay, click, face};

use common::start;

const PATIENCE: Duration = Duration::from_secs(30);

/// The hotbar slot the bot keeps its stack in, and the one it keeps empty to open the
/// chest with.
const STACK_SLOT: i16 = 1;
const EMPTY_SLOT: i16 = 8;

/// The menu slot of the hotbar's slot `slot` in a chest's window: after the chest's 27 and
/// the main inventory's 27.
fn hotbar_in_chest(slot: i16) -> i16 {
    54 + slot
}

/// The window packets of one run, each step's apart.
#[derive(Debug, Default, PartialEq)]
struct Ran {
    steps: Vec<(&'static str, Vec<ClientboundPlay>)>,
}

/// Waits until the server has handled everything the bot sent before: a click with an
/// empty hand on the ground beside it, which changes nothing and is acknowledged.
async fn round_trip(bot: &mut Bot) {
    let (x, z) = (bot.location.0.floor() as i32, bot.location.2.floor() as i32);
    let sequence = bot.use_item_on(x + 1, -61, z - 1, face::TOP).await.unwrap();
    bot.wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= sequence)
        .await
        .unwrap_or_else(|error| panic!("{error:#}: no acknowledgement"));
}

/// What the bot is sent from now on, until the server has handled what it sent.
async fn step(bot: &mut Bot, name: &'static str, ran: &mut Ran) {
    round_trip(bot).await;
    let sent = std::mem::take(&mut bot.menu_log);
    ran.steps.push((name, sent));
}

async fn run(address: &str) -> Ran {
    let mut bot = Bot::join(address, "Clicker").await.unwrap();
    bot.wait_for_chunks(9, PATIENCE).await.unwrap();
    let (x, z) = (
        bot.location.0.floor() as i32 + 2,
        bot.location.2.floor() as i32,
    );
    let mut ran = Ran::default();

    bot.take_from_creative_inventory(0, items::CHEST)
        .await
        .unwrap();
    bot.select_slot(0).await.unwrap();
    bot.use_item_on(x, -61, z, face::TOP).await.unwrap();
    bot.wait_until(PATIENCE, |bot| {
        bot.block_at(x, -60, z).unwrap().is_some_and(|state| {
            clustine_data::BlockState(state as u16).block_id() == blocks::CHEST.block_id()
        })
    })
    .await
    .unwrap();
    let stone = ItemStack {
        item: items::STONE,
        count: 16,
    };
    bot.put_in_hotbar(STACK_SLOT, stone).await.unwrap();
    // Clustine gives a new player a hotbar of blocks and the official server none: the
    // rest of it emptied, the two are alike.
    for slot in (2..=8).chain([0]) {
        bot.empty_hotbar_slot(slot).await.unwrap();
    }
    bot.select_slot(EMPTY_SLOT).await.unwrap();
    round_trip(&mut bot).await;
    bot.menu_log.clear();

    bot.use_item_on(x, -60, z, face::TOP).await.unwrap();
    bot.wait_until(PATIENCE, |bot| bot.menu.is_some())
        .await
        .unwrap();
    step(&mut bot, "open the chest", &mut ran).await;

    // The stack picked up from the hotbar, guessed right: nothing is sent back.
    let at = hotbar_in_chest(STACK_SLOT);
    bot.click_window(at, 0, click::PICKUP, vec![(at, None)], Some(stone))
        .await
        .unwrap();
    step(&mut bot, "pick up, guessed right", &mut ran).await;

    // Put down in the chest's first slot, guessed wrong: the client says it kept one.
    let kept = ItemStack { count: 1, ..stone };
    let put = ItemStack { count: 15, ..stone };
    bot.click_window(0, 0, click::PICKUP, vec![(0, Some(put))], Some(kept))
        .await
        .unwrap();
    step(&mut bot, "put down, guessed wrong", &mut ran).await;

    // Shift-clicked back, the client guessing only that the slot empties.
    bot.click_window(0, 0, click::QUICK_MOVE, vec![(0, None)], None)
        .await
        .unwrap();
    step(&mut bot, "shift-click, guessed in part", &mut ran).await;

    bot.close_window().await.unwrap();
    step(&mut bot, "close", &mut ran).await;
    ran
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_chests_window_is_sent_as_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let (server, address) = start().await;
    let theirs = run(oracle.address()).await;
    let ours = run(&address).await;
    server.stop().await;
    drop(oracle);

    let mut differences = Vec::new();
    for ((name, their), (_, our)) in theirs.steps.iter().zip(&ours.steps) {
        println!("{name}:\n  official: {their:?}\n  Clustine: {our:?}");
        if their != our {
            differences.push(format!(
                "{name}: the official server sent {their:?}, Clustine {our:?}"
            ));
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}
