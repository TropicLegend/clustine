//! Window 0, the player's own inventory screen (V3a), with real clients, written from
//! the game's classes by someone who did not write the code: a survival player moves a
//! stack within their inventory by window-0 clicks and the server agrees
//! (`ServerGamePacketListenerImpl.handleContainerClick`, `InventoryMenu`), a stack left
//! in the 2×2 grid comes back with them (Clustine's named difference: the game drops it
//! at the feet), a survival player puts on a helmet by a shift-click
//! (`InventoryMenu.quickMoveStack` @135-200), hears its equip sound and another player
//! sees it (`LivingEntity.onEquipItem`, the equipment update), and a stack carried on
//! the cursor across a region line is still carried and put down on the other side (the
//! game never closes the inventory menu).
//!
//! The crossing needs the pinned layout (`CLUSTINE_TEST_PINS=0,4`, whose line at block
//! x = 0 runs beside the spawn); in a world whose regions follow their players it
//! returns at once.

mod common;

use std::time::Duration;

use clustine::Config;
use clustine_botswarm::{Bot, Heard};
use clustine_data::{items, sounds};
use clustine_protocol::item::ItemStack;
use clustine_protocol::packets::play::{ClientboundPlay, click};
use clustine_sim::api::Order;
use clustine_world::GameMode;

use common::{VIEW_DISTANCE, config, start_with, view_area};

const PATIENCE: Duration = Duration::from_secs(60);

/// Window 0's slots (`InventoryMenu.<init>`).
const GRID_FIRST: usize = 1;
const HEAD: usize = 5;
const MAIN_FIRST: usize = 9;
const HOTBAR_FIRST: usize = 36;

/// The head's id in an equipment update (`EquipmentSlot.HEAD`'s ordinal).
const EQUIPMENT_HEAD: u8 = 5;

/// A world whose new players are survival ones.
fn survival() -> Config {
    Config {
        game_mode: GameMode::Survival,
        force_game_mode: true,
        ..config()
    }
}

fn pinned() -> bool {
    !std::env::var("CLUSTINE_TEST_PINS")
        .unwrap_or_default()
        .is_empty()
}

/// Joins, waits for the chunks around the spawn point and for window 0 whole.
async fn join(address: &str, name: &str) -> Bot {
    let mut bot = Bot::join(address, name).await.unwrap();
    let count = view_area((0, 0), VIEW_DISTANCE).len();
    bot.wait_for_chunks(count, PATIENCE).await.unwrap();
    bot.wait_until(PATIENCE, |bot| bot.inventory_window.state_id >= 1)
        .await
        .unwrap();
    bot
}

fn slot(bot: &Bot, index: usize) -> Option<(i32, i32)> {
    bot.inventory_window.slots[index]
        .as_ref()
        .map(|stack| (stack.item, stack.count))
}

/// Has the operator give `bot`, called `name`, `count` of `item`, and waits for it to
/// show in window 0: the slot it went to (a survival newcomer starts empty, and the
/// game's `Inventory.add` fills the hotbar first).
async fn given(
    server: &clustine::Server,
    bot: &mut Bot,
    name: &str,
    item: i32,
    count: i32,
) -> usize {
    server
        .order(name, Order::Give { item, count })
        .await
        .unwrap();
    let holds = |bot: &Bot| (0..46).find(|at| slot(bot, *at) == Some((item, count)));
    bot.wait_until(PATIENCE, |bot| holds(bot).is_some())
        .await
        .unwrap_or_else(|error| panic!("{error}: {item} was not given"));
    holds(bot).expect("given")
}

/// Picks up the stack of window 0's slot `from` and puts it down on `to`, which is
/// empty, saying what a vanilla client guesses of each.
async fn move_stack(bot: &mut Bot, from: usize, to: usize) {
    let stack = bot.inventory_window.slots[from]
        .clone()
        .expect("a stack to move");
    bot.click_inventory(
        from as i16,
        0,
        click::PICKUP,
        vec![(from as i16, None)],
        Some(stack.clone()),
    )
    .await
    .unwrap();
    bot.click_inventory(
        to as i16,
        0,
        click::PICKUP,
        vec![(to as i16, Some(stack))],
        None,
    )
    .await
    .unwrap();
}

/// Has `bot` stand until the server has kept everything, leave, and join again.
async fn rejoin(address: &str, name: &str, mut bot: Bot) -> Bot {
    bot.stand_until_kept(PATIENCE).await.unwrap();
    bot.close().await.unwrap();
    drop(bot);
    join(address, name).await
}

/// A survival player given stone and cobblestone picks up the stone of the hotbar's
/// first slot and puts it down in the main inventory's first, and puts the cobblestone
/// of the hotbar's second into the grid's first slot: the server takes both (`handleContainerClick` with window 0, no
/// other menu open). When the player comes back, window 0 is sent whole with the state
/// id 1 and nothing carried (`initInventoryMenu`), the stone where it was put, and the
/// grid's cobblestone is told after it (kept in the player's place: Clustine's named
/// difference).
#[tokio::test]
async fn a_survival_player_moves_stacks_by_window_0_clicks_and_finds_them_so_on_coming_back() {
    let (server, address) = start_with(survival()).await;
    let mut bot = join(&address, "Mover").await;
    assert_eq!(
        given(&server, &mut bot, "Mover", items::STONE, 64).await,
        HOTBAR_FIRST
    );
    assert_eq!(
        given(&server, &mut bot, "Mover", items::COBBLESTONE, 3).await,
        HOTBAR_FIRST + 1
    );
    let (stone, cobblestone) = ((items::STONE, 64), (items::COBBLESTONE, 3));
    assert_eq!(slot(&bot, MAIN_FIRST), None);
    move_stack(&mut bot, HOTBAR_FIRST, MAIN_FIRST).await;
    move_stack(&mut bot, HOTBAR_FIRST + 1, GRID_FIRST).await;

    let mut bot = rejoin(&address, "Mover", bot).await;
    let whole = bot.inventory_window.clone();
    assert_eq!(slot(&bot, MAIN_FIRST), Some(stone), "{whole:?}");
    assert_eq!(slot(&bot, HOTBAR_FIRST), None, "{whole:?}");
    bot.wait_until(PATIENCE, |bot| slot(bot, GRID_FIRST) == Some(cobblestone))
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error}: the grid's first slot is {:?}",
                bot.inventory_window.slots[GRID_FIRST]
            )
        });
    assert_eq!(slot(&bot, HOTBAR_FIRST + 1), None);
    assert_eq!(bot.cursor, None);
    server.stop().await;
}

/// A survival player is given an iron helmet, which goes to the hotbar, and shift-clicks
/// it onto the head (`quickMoveStack`
/// @135-200). They hear its equip sound (`onEquipItem` plays it with no player left
/// out), and another player is shown the helmet on their head.
#[tokio::test]
async fn a_survival_player_puts_on_a_helmet_by_a_shift_click_and_another_sees_it() {
    let (server, address) = start_with(survival()).await;
    let mut watcher = join(&address, "Watcher").await;
    let mut wearer = join(&address, "Wearer").await;
    // A stone first, so that the helmet goes into the hotbar's second slot.
    given(&server, &mut wearer, "Wearer", items::STONE, 1).await;
    let at = given(&server, &mut wearer, "Wearer", items::IRON_HELMET, 1).await;
    assert_eq!(at, HOTBAR_FIRST + 1);
    watcher
        .wait_until(PATIENCE, |bot| bot.seen_player("Wearer").is_some())
        .await
        .unwrap();
    let wearer_id = watcher.seen_player("Wearer").expect("seen").entity_id;
    watcher.record_packets();
    wearer.record_heard();
    let helmet = ItemStack::new(items::IRON_HELMET, 1);
    wearer
        .click_inventory(
            at as i16,
            0,
            click::QUICK_MOVE,
            vec![(at as i16, None), (HEAD as i16, Some(helmet))],
            None,
        )
        .await
        .unwrap();
    watcher
        .wait_until(PATIENCE, |bot| {
            bot.packets.iter().flatten().any(|(_, packet)| {
                matches!(packet, ClientboundPlay::SetEquipment(equipment)
                    if equipment.entity_id == wearer_id
                        && equipment.slots.iter().any(|(slot, stack)| *slot == EQUIPMENT_HEAD
                            && stack.as_ref().is_some_and(|stack| stack.item == items::IRON_HELMET)))
            })
        })
        .await
        .unwrap_or_else(|error| panic!("{error}: the watcher was not shown the helmet"));
    wearer
        .wait_until(PATIENCE, |bot| {
            bot.heard.iter().flatten().any(|(_, heard)| {
                matches!(heard, Heard::Sound { sound, .. } if *sound == sounds::ITEM_ARMOR_EQUIP_IRON)
            })
        })
        .await
        .unwrap_or_else(|error| panic!("{error}: the wearer did not hear the helmet go on"));
    // The server agrees: nothing it sent says otherwise.
    assert_eq!(slot(&wearer, HEAD), Some((items::IRON_HELMET, 1)));
    assert_eq!(slot(&wearer, at), None);
    server.stop().await;
}

/// The game never closes the inventory menu: a survival player given stone picks it up
/// from the hotbar's first slot, walks across the region line at block x = 0 carrying it,
/// and puts it down in the main inventory's first slot on the other side. Coming back,
/// they find it there and nothing carried.
#[tokio::test]
async fn a_stack_carried_across_a_region_line_is_put_down_on_the_other_side() {
    if !pinned() {
        // Only a pinned line is sure to run beside the spawn.
        return;
    }
    let (server, address) = start_with(survival()).await;
    let mut bot = join(&address, "Carrier").await;
    assert_eq!(
        given(&server, &mut bot, "Carrier", items::STONE, 64).await,
        HOTBAR_FIRST
    );
    let stone = ItemStack::new(items::STONE, 64);
    bot.click_inventory(
        HOTBAR_FIRST as i16,
        0,
        click::PICKUP,
        vec![(HOTBAR_FIRST as i16, None)],
        Some(stone.clone()),
    )
    .await
    .unwrap();
    bot.stand_until_kept(PATIENCE).await.unwrap();
    assert!(bot.location.0 > 0.0, "the bot starts east of the line");
    let z = bot.location.2;
    bot.walk_to(-2.5, z, 0.2).await.unwrap();
    bot.stand_until_kept(PATIENCE).await.unwrap();
    assert_eq!(
        bot.cursor.as_ref().map(|stack| (stack.item, stack.count)),
        Some((stone.item, stone.count)),
        "still carried, as far as the server has said"
    );
    bot.click_inventory(
        MAIN_FIRST as i16,
        0,
        click::PICKUP,
        vec![(MAIN_FIRST as i16, Some(stone.clone()))],
        None,
    )
    .await
    .unwrap();
    let bot = rejoin(&address, "Carrier", bot).await;
    assert!(
        bot.location.0 < 0.0,
        "back west of the line: {:?}",
        bot.location
    );
    assert_eq!(slot(&bot, MAIN_FIRST), Some((stone.item, stone.count)));
    assert_eq!(slot(&bot, HOTBAR_FIRST), None);
    assert_eq!(bot.cursor, None);
    server.stop().await;
}
