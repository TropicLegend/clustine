//! Crafting with real clients (V3c), written from the game's classes by someone who did
//! not write the code: a survival player crafts in window 0's 2×2 grid and at a crafting
//! table, taking each result by a click or a shift-click, and the server and the player
//! agree on the inventory after each click and when the player comes back.
//!
//! What the tests rest on, in the game's classes (`reference/v3-javap`):
//!
//! - `ResultSlot.onTake`: a take consumes one of each grid stack and the result is
//!   worked out again from what is left (`slotChangedCraftingGrid`, which sends the
//!   result slot to the client, which computes none).
//! - `AbstractContainerMenu.doClick` @698-733: a shift-click repeats `quickMoveStack`
//!   while it moves something and the slot then holds the same item.
//! - `InventoryMenu.quickMoveStack` @45-75: the result goes to slots 9 to 44, last first
//!   (`moveItemStackTo(stack, 9, 45, true)`), merging into a stack of the same item
//!   before an empty slot.
//! - `CraftingMenu`: a window of 46 slots (0 the result, 1-9 the grid row by row, 10-36
//!   the main inventory, 37-45 the hotbar), `removed` @0-21 then
//!   `AbstractContainerMenu.clearContainer` @0-29, which puts each grid stack back by
//!   `Inventory.placeItemBackInInventory` (a slot holding the same item with room, else
//!   the first empty slot of 0..36).
//! - `CraftingTableBlock.useWithoutItem`: a use opens `minecraft:crafting` (menu type 12)
//!   titled `container.crafting`.
//!
//! Every click guesses what a vanilla client guesses: the grid's stacks taken from, and
//! the result slot empty after a take, since the client works out no result. A step
//! ends when the server has handled the click (an acknowledged use of the ground with
//! an empty hand) and the bot's slots and cursor are those the game's would be: where the
//! guess was wrong, only what the server sent makes them so.

mod common;

use std::time::Duration;

use clustine::Config;
use clustine_botswarm::Bot;
use clustine_data::{blocks, entity_types, items};
use clustine_protocol::item::ItemStack;
use clustine_protocol::packets::play::{ClientboundPlay, click, face, pose};
use clustine_protocol::text::Text;
use clustine_sim::api::Order;
use clustine_world::GameMode;

use common::{VIEW_DISTANCE, config, start_with, view_area};

const PATIENCE: Duration = Duration::from_secs(60);

/// The hotbar slot the bots hold, kept empty: a round trip's use of the ground with it
/// places nothing. Window 0's slot 40 and the table's 41.
const HELD: i16 = 4;

/// Window 0's slots (`InventoryMenu.<init>`).
const RESULT: usize = 0;
const MAIN_FIRST: usize = 9;
const HOTBAR_FIRST: usize = 36;
/// The crafting table's hotbar (`CraftingMenu.<init>`: the inventory after the result
/// and the nine grid slots).
const TABLE_HOTBAR_FIRST: usize = 37;
const TABLE_MAIN_FIRST: usize = 10;

/// `minecraft:crafting` in the menu type registry.
const CRAFTING_MENU: i32 = 12;

type Stack = Option<(i32, i32)>;

/// A world whose new players are survival ones.
fn survival() -> Config {
    Config {
        game_mode: GameMode::Survival,
        force_game_mode: true,
        ..config()
    }
}

/// Joins, waits for the chunks around the spawn point and for window 0 whole, and holds
/// the empty hotbar slot [`HELD`].
async fn join(address: &str, name: &str) -> Bot {
    let mut bot = Bot::join(address, name).await.unwrap();
    let count = view_area((0, 0), VIEW_DISTANCE).len();
    bot.wait_for_chunks(count, PATIENCE).await.unwrap();
    bot.wait_until(PATIENCE, |bot| bot.inventory_window.state_id >= 1)
        .await
        .unwrap();
    bot.select_slot(HELD).await.unwrap();
    bot
}

fn stack(item: i32, count: i32) -> Stack {
    Some((item, count))
}

fn item_stack(stack: Stack) -> Option<ItemStack> {
    stack.map(|(item, count)| ItemStack::new(item, count))
}

fn seen(slots: &[Option<ItemStack>]) -> Vec<Stack> {
    slots
        .iter()
        .map(|slot| slot.as_ref().map(|stack| (stack.item, stack.count)))
        .collect()
}

fn cursor(bot: &Bot) -> Stack {
    bot.cursor.as_ref().map(|stack| (stack.item, stack.count))
}

/// `count` slots, those of `holding` holding their stacks and the rest empty.
fn slots(count: usize, holding: &[(usize, i32, i32)]) -> Vec<Stack> {
    let mut slots = vec![None; count];
    for (at, item, count) in holding {
        slots[*at] = stack(*item, *count);
    }
    slots
}

/// Waits until the server has handled everything the bot sent before: a use of the
/// ground beside it with the empty hand, which changes nothing and is acknowledged.
async fn round_trip(bot: &mut Bot) {
    let (x, z) = (bot.location.0.floor() as i32, bot.location.2.floor() as i32);
    let sequence = bot.use_item_on(x + 1, -61, z - 1, face::TOP).await.unwrap();
    bot.wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= sequence)
        .await
        .unwrap_or_else(|error| panic!("{error:#}: no acknowledgement"));
}

/// Once the server has handled what the bot sent, waits until window 0 holds `holding`
/// and nothing else, and the cursor `carried`.
async fn agree(bot: &mut Bot, what: &str, holding: &[(usize, i32, i32)], carried: Stack) {
    round_trip(bot).await;
    let want = slots(46, holding);
    bot.wait_until(PATIENCE, |bot| {
        seen(&bot.inventory_window.slots) == want && cursor(bot) == carried
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "{error}: after {what}, window 0 is {:?} and the cursor {:?}; the game's would be \
             {want:?} and {carried:?}",
            seen(&bot.inventory_window.slots),
            cursor(bot)
        )
    });
}

/// As [`agree`], of the crafting table's window.
async fn agree_table(bot: &mut Bot, what: &str, holding: &[(usize, i32, i32)], carried: Stack) {
    round_trip(bot).await;
    let want = slots(46, holding);
    let shown = |bot: &Bot| bot.menu.as_ref().map(|menu| seen(&menu.slots));
    bot.wait_until(PATIENCE, |bot| {
        shown(bot).as_ref() == Some(&want) && cursor(bot) == carried
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "{error}: after {what}, the table's window is {:?} and the cursor {:?}; the \
             game's would be {want:?} and {carried:?}",
            shown(bot),
            cursor(bot)
        )
    });
}

/// Clicks window 0's slot `slot`, guessing as a vanilla client does.
async fn click_inventory(
    bot: &mut Bot,
    slot: usize,
    button: i8,
    input: i32,
    changed: &[(usize, Stack)],
    carried: Stack,
) {
    let changed = changed
        .iter()
        .map(|(at, stack)| (*at as i16, item_stack(*stack)))
        .collect();
    bot.click_inventory(slot as i16, button, input, changed, item_stack(carried))
        .await
        .unwrap();
}

/// Clicks the table's slot `slot`, guessing as a vanilla client does; then waits until
/// the server has handled it.
async fn click_table(
    bot: &mut Bot,
    slot: usize,
    button: i8,
    input: i32,
    changed: &[(usize, Stack)],
    carried: Stack,
) {
    let changed = changed
        .iter()
        .map(|(at, stack)| (*at as i16, item_stack(*stack)))
        .collect();
    bot.click_window(slot as i16, button, input, changed, item_stack(carried))
        .await
        .unwrap();
    round_trip(bot).await;
}

/// Has the operator give `name` `count` of `item`, and waits for window 0's slot `at`
/// to hold it (a survival newcomer starts empty, and the game's `Inventory.add` fills the
/// hotbar first).
async fn given(
    server: &clustine::Server,
    bot: &mut Bot,
    name: &str,
    item: i32,
    count: i32,
    at: usize,
) {
    server
        .order(name, Order::Give { item, count })
        .await
        .unwrap();
    bot.wait_until(PATIENCE, |bot| {
        seen(&bot.inventory_window.slots)[at] == stack(item, count)
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "{error}: {count} of {item} were not given into slot {at}: {:?}",
            seen(&bot.inventory_window.slots)
        )
    });
}

/// Places the block held in the hotbar slot `slot` on the ground two blocks east of the
/// bot, waits for `watcher` to be shown it, and holds [`HELD`] again. Returns where it is.
async fn place(
    bot: &mut Bot,
    slot: i16,
    block: clustine_data::BlockState,
    watcher: &mut Bot,
) -> (i32, i32, i32) {
    let (x, z) = (
        bot.location.0.floor() as i32 + 2,
        bot.location.2.floor() as i32,
    );
    bot.select_slot(slot).await.unwrap();
    bot.use_item_on(x, -61, z, face::TOP).await.unwrap();
    let is_it = |bot: &Bot| {
        bot.block_at(x, -60, z).unwrap().is_some_and(|state| {
            clustine_data::BlockState(state as u16).block_id() == block.block_id()
        })
    };
    bot.wait_until(PATIENCE, is_it)
        .await
        .unwrap_or_else(|error| panic!("{error}: the block was not placed"));
    watcher
        .wait_until(PATIENCE, is_it)
        .await
        .unwrap_or_else(|error| panic!("{error}: the watcher was not shown the block"));
    bot.select_slot(HELD).await.unwrap();
    (x, -60, z)
}

/// Joins a watcher and then the crafter, and waits until the watcher sees the crafter.
async fn watched(address: &str, crafter: &str) -> (Bot, Bot) {
    let mut watcher = join(address, "Watcher").await;
    let bot = join(address, crafter).await;
    watcher
        .wait_until(PATIENCE, |watcher| watcher.seen_player(crafter).is_some())
        .await
        .unwrap();
    (watcher, bot)
}

/// Has `bot`, called `name`, sneak or stand up, and waits until `watcher` is shown its
/// pose so: a mark in what the watcher is sent, which comes in order with what the
/// crafter's clicks may show. Returns how much the watcher had been sent by then.
async fn mark(bot: &mut Bot, name: &str, watcher: &mut Bot, sneaking: bool) -> usize {
    let pose = if sneaking {
        pose::CROUCHING
    } else {
        pose::STANDING
    };
    bot.sneak(sneaking).await.unwrap();
    watcher
        .wait_until(PATIENCE, |watcher| {
            watcher
                .seen_player(name)
                .is_some_and(|seen| seen.pose == Some(pose))
        })
        .await
        .unwrap_or_else(|error| panic!("{error}: the watcher was not shown the pose {pose}"));
    watcher.packets.as_ref().map_or(0, Vec::len)
}

/// Nothing a craft does shows to another player between two marks: an item thrown (a
/// craft's remainder or rest with no room is dropped by `player.drop(stack, false,
/// PREDICTED)`, and nothing here lacks room) or an arm swung (no click swings;
/// `handleContainerClick` calls no `swing`). The use of a block swings
/// (`handleUseItemOn` @440-471 for a `SUCCESS`), which is why the watch is between marks
/// and not around the placing and the opening of the table.
fn nothing_odd(watcher: &Bot, from: usize, to: usize) {
    let odd: Vec<_> = watcher.packets.as_deref().unwrap_or_default()[from..to]
        .iter()
        .filter(|(_, packet)| match packet {
            ClientboundPlay::SpawnEntity(spawned) => spawned.kind == entity_types::ITEM,
            ClientboundPlay::SwingAnimation(_) => true,
            _ => false,
        })
        .map(|(_, packet)| packet)
        .collect();
    assert!(odd.is_empty(), "the watcher was sent {odd:?}");
}

/// Has `bot` stand until the server has kept everything, leave, and join again.
async fn rejoin(address: &str, name: &str, mut bot: Bot) -> Bot {
    bot.stand_until_kept(PATIENCE).await.unwrap();
    bot.close().await.unwrap();
    drop(bot);
    join(address, name).await
}

/// A survival player given three oak logs puts one into the 2×2 grid, is shown four
/// planks as the result and takes them by a click; puts the two other logs in and
/// shift-clicks the result, which crafts twice (the repeat of `doClick`) into the stack
/// of planks; makes four sticks of two planks one over the other (one plank alone shows
/// an oak button) and takes them by a click; and makes a crafting table of four planks,
/// which a shift-click moves to the hotbar's last slot. They place the table, which a
/// player beside them is shown; that player was sent nothing of the crafting (no item
/// thrown, no swing), which the crafter's sneaking before it and standing up after it
/// mark. When the crafter comes back, window 0 is sent whole with what
/// is left: six planks and four sticks.
#[tokio::test]
async fn a_survival_player_crafts_planks_sticks_and_a_table_in_the_inventory_grid() {
    let (server, address) = start_with(survival()).await;
    let (mut watcher, mut bot) = watched(&address, "Crafter").await;
    let (log, planks, stick, table, button) = (
        items::OAK_LOG,
        items::OAK_PLANKS,
        items::STICK,
        items::CRAFTING_TABLE,
        items::OAK_BUTTON,
    );
    given(&server, &mut bot, "Crafter", log, 3, HOTBAR_FIRST).await;
    let logs = HOTBAR_FIRST;
    watcher.record_packets();
    let from = mark(&mut bot, "Crafter", &mut watcher, true).await;

    // One log into the grid's first slot by a right click: four planks are shown.
    click_inventory(
        &mut bot,
        logs,
        0,
        click::PICKUP,
        &[(logs, None)],
        stack(log, 3),
    )
    .await;
    agree(&mut bot, "picking up the logs", &[], stack(log, 3)).await;
    click_inventory(
        &mut bot,
        1,
        1,
        click::PICKUP,
        &[(1, stack(log, 1))],
        stack(log, 2),
    )
    .await;
    agree(
        &mut bot,
        "putting one log into the grid",
        &[(1, log, 1), (RESULT, planks, 4)],
        stack(log, 2),
    )
    .await;
    click_inventory(
        &mut bot,
        logs,
        0,
        click::PICKUP,
        &[(logs, stack(log, 2))],
        None,
    )
    .await;
    agree(
        &mut bot,
        "putting the other logs back",
        &[(logs, log, 2), (1, log, 1), (RESULT, planks, 4)],
        None,
    )
    .await;

    // Taken by a click: the grid is empty, and so is the result worked out again.
    click_inventory(
        &mut bot,
        RESULT,
        0,
        click::PICKUP,
        &[(RESULT, None), (1, None)],
        stack(planks, 4),
    )
    .await;
    agree(
        &mut bot,
        "taking the planks",
        &[(logs, log, 2)],
        stack(planks, 4),
    )
    .await;
    click_inventory(
        &mut bot,
        MAIN_FIRST,
        0,
        click::PICKUP,
        &[(MAIN_FIRST, stack(planks, 4))],
        None,
    )
    .await;
    agree(
        &mut bot,
        "putting the planks down",
        &[(logs, log, 2), (MAIN_FIRST, planks, 4)],
        None,
    )
    .await;

    // Two logs in, and the result shift-clicked: the client guesses one craft, the server
    // crafts twice into the stack of planks.
    click_inventory(
        &mut bot,
        logs,
        0,
        click::PICKUP,
        &[(logs, None)],
        stack(log, 2),
    )
    .await;
    click_inventory(&mut bot, 1, 0, click::PICKUP, &[(1, stack(log, 2))], None).await;
    agree(
        &mut bot,
        "putting two logs into the grid",
        &[(1, log, 2), (MAIN_FIRST, planks, 4), (RESULT, planks, 4)],
        None,
    )
    .await;
    click_inventory(
        &mut bot,
        RESULT,
        0,
        click::QUICK_MOVE,
        &[
            (RESULT, None),
            (1, stack(log, 1)),
            (MAIN_FIRST, stack(planks, 8)),
        ],
        None,
    )
    .await;
    agree(
        &mut bot,
        "shift-clicking the planks",
        &[(MAIN_FIRST, planks, 12)],
        None,
    )
    .await;

    // Sticks: a plank in the grid's first slot (an oak button), one under it (sticks).
    click_inventory(
        &mut bot,
        MAIN_FIRST,
        0,
        click::PICKUP,
        &[(MAIN_FIRST, None)],
        stack(planks, 12),
    )
    .await;
    click_inventory(
        &mut bot,
        1,
        1,
        click::PICKUP,
        &[(1, stack(planks, 1))],
        stack(planks, 11),
    )
    .await;
    agree(
        &mut bot,
        "putting one plank into the grid",
        &[(1, planks, 1), (RESULT, button, 1)],
        stack(planks, 11),
    )
    .await;
    click_inventory(
        &mut bot,
        3,
        1,
        click::PICKUP,
        &[(3, stack(planks, 1))],
        stack(planks, 10),
    )
    .await;
    agree(
        &mut bot,
        "putting a plank under it",
        &[(1, planks, 1), (3, planks, 1), (RESULT, stick, 4)],
        stack(planks, 10),
    )
    .await;
    click_inventory(
        &mut bot,
        MAIN_FIRST,
        0,
        click::PICKUP,
        &[(MAIN_FIRST, stack(planks, 10))],
        None,
    )
    .await;
    click_inventory(
        &mut bot,
        RESULT,
        0,
        click::PICKUP,
        &[(RESULT, None), (1, None), (3, None)],
        stack(stick, 4),
    )
    .await;
    agree(
        &mut bot,
        "taking the sticks",
        &[(MAIN_FIRST, planks, 10)],
        stack(stick, 4),
    )
    .await;
    click_inventory(
        &mut bot,
        MAIN_FIRST + 1,
        0,
        click::PICKUP,
        &[(MAIN_FIRST + 1, stack(stick, 4))],
        None,
    )
    .await;

    // A crafting table: a plank in each of the grid's four slots.
    click_inventory(
        &mut bot,
        MAIN_FIRST,
        0,
        click::PICKUP,
        &[(MAIN_FIRST, None)],
        stack(planks, 10),
    )
    .await;
    for (at, left) in [(1, 9), (2, 8), (3, 7), (4, 6)] {
        click_inventory(
            &mut bot,
            at,
            1,
            click::PICKUP,
            &[(at, stack(planks, 1))],
            stack(planks, left),
        )
        .await;
        round_trip(&mut bot).await;
    }
    click_inventory(
        &mut bot,
        MAIN_FIRST,
        0,
        click::PICKUP,
        &[(MAIN_FIRST, stack(planks, 6))],
        None,
    )
    .await;
    agree(
        &mut bot,
        "filling the grid with planks",
        &[
            (1, planks, 1),
            (2, planks, 1),
            (3, planks, 1),
            (4, planks, 1),
            (RESULT, table, 1),
            (MAIN_FIRST, planks, 6),
            (MAIN_FIRST + 1, stick, 4),
        ],
        None,
    )
    .await;
    // Shift-clicked: to the last empty slot of 9..45, the hotbar's last.
    click_inventory(
        &mut bot,
        RESULT,
        0,
        click::QUICK_MOVE,
        &[
            (RESULT, None),
            (1, None),
            (2, None),
            (3, None),
            (4, None),
            (44, stack(table, 1)),
        ],
        None,
    )
    .await;
    agree(
        &mut bot,
        "shift-clicking the table",
        &[
            (MAIN_FIRST, planks, 6),
            (MAIN_FIRST + 1, stick, 4),
            (44, table, 1),
        ],
        None,
    )
    .await;

    let to = mark(&mut bot, "Crafter", &mut watcher, false).await;
    nothing_odd(&watcher, from, to);
    place(&mut bot, 8, blocks::CRAFTING_TABLE, &mut watcher).await;
    agree(
        &mut bot,
        "placing the table",
        &[(MAIN_FIRST, planks, 6), (MAIN_FIRST + 1, stick, 4)],
        None,
    )
    .await;

    let bot = rejoin(&address, "Crafter", bot).await;
    assert_eq!(
        seen(&bot.inventory_window.slots),
        slots(46, &[(MAIN_FIRST, planks, 6), (MAIN_FIRST + 1, stick, 4)]),
        "window 0 as it comes whole"
    );
    assert_eq!(bot.cursor, None);
    server.stop().await;
}

/// A survival player given a crafting table, four planks and two sticks places the table
/// and uses it: a window of `minecraft:crafting` titled `container.crafting` with 46
/// slots. Three planks along the top row over two sticks down the middle make a wooden
/// pickaxe, which a click takes, emptying the grid; a plank put back into the grid shows
/// an oak button. The close puts the plank back into the inventory's first empty slot
/// (`placeItemBackInInventory`: no stack of planks is left), and the window-0 view the
/// player has of the inventory (the client's slots are those of the inventory the table's
/// window showed) is brought up to date. A player beside them was sent nothing of it (no
/// item thrown, no swing). When the crafter comes back, window 0 is sent whole: the
/// pickaxe and the plank.
#[tokio::test]
async fn a_survival_player_crafts_a_pickaxe_at_a_table_and_its_close_gives_the_grid_back() {
    let (server, address) = start_with(survival()).await;
    let (mut watcher, mut bot) = watched(&address, "Smith").await;
    let (planks, stick, pickaxe, button) = (
        items::OAK_PLANKS,
        items::STICK,
        items::WOODEN_PICKAXE,
        items::OAK_BUTTON,
    );
    given(
        &server,
        &mut bot,
        "Smith",
        items::CRAFTING_TABLE,
        1,
        HOTBAR_FIRST,
    )
    .await;
    given(&server, &mut bot, "Smith", planks, 4, HOTBAR_FIRST + 1).await;
    given(&server, &mut bot, "Smith", stick, 2, HOTBAR_FIRST + 2).await;
    watcher.record_packets();
    let (x, y, z) = place(&mut bot, 0, blocks::CRAFTING_TABLE, &mut watcher).await;
    agree(
        &mut bot,
        "placing the table",
        &[(HOTBAR_FIRST + 1, planks, 4), (HOTBAR_FIRST + 2, stick, 2)],
        None,
    )
    .await;

    bot.use_item_on(x, y, z, face::TOP).await.unwrap();
    bot.wait_until(PATIENCE, |bot| {
        bot.menu.as_ref().is_some_and(|menu| !menu.slots.is_empty())
    })
    .await
    .unwrap_or_else(|error| panic!("{error}: the table did not open"));
    let menu = bot.menu.clone().expect("shown");
    assert_eq!(menu.menu, CRAFTING_MENU);
    assert_eq!(menu.title, Text::translatable("container.crafting"));
    assert_eq!(menu.slots.len(), 46);
    let (planks_at, sticks_at) = (TABLE_HOTBAR_FIRST + 1, TABLE_HOTBAR_FIRST + 2);
    agree_table(
        &mut bot,
        "opening the table",
        &[(planks_at, planks, 4), (sticks_at, stick, 2)],
        None,
    )
    .await;
    let from = mark(&mut bot, "Smith", &mut watcher, true).await;

    // Three planks along the top row, the fourth put back.
    click_table(
        &mut bot,
        planks_at,
        0,
        click::PICKUP,
        &[(planks_at, None)],
        stack(planks, 4),
    )
    .await;
    for (at, left) in [(1, 3), (2, 2), (3, 1)] {
        click_table(
            &mut bot,
            at,
            1,
            click::PICKUP,
            &[(at, stack(planks, 1))],
            stack(planks, left),
        )
        .await;
    }
    click_table(
        &mut bot,
        planks_at,
        0,
        click::PICKUP,
        &[(planks_at, stack(planks, 1))],
        None,
    )
    .await;
    // Two sticks down the middle.
    click_table(
        &mut bot,
        sticks_at,
        0,
        click::PICKUP,
        &[(sticks_at, None)],
        stack(stick, 2),
    )
    .await;
    click_table(
        &mut bot,
        5,
        1,
        click::PICKUP,
        &[(5, stack(stick, 1))],
        stack(stick, 1),
    )
    .await;
    click_table(&mut bot, 8, 1, click::PICKUP, &[(8, stack(stick, 1))], None).await;
    agree_table(
        &mut bot,
        "laying out a pickaxe",
        &[
            (1, planks, 1),
            (2, planks, 1),
            (3, planks, 1),
            (5, stick, 1),
            (8, stick, 1),
            (planks_at, planks, 1),
            (0, pickaxe, 1),
        ],
        None,
    )
    .await;

    click_table(
        &mut bot,
        0,
        0,
        click::PICKUP,
        &[
            (0, None),
            (1, None),
            (2, None),
            (3, None),
            (5, None),
            (8, None),
        ],
        stack(pickaxe, 1),
    )
    .await;
    agree_table(
        &mut bot,
        "taking the pickaxe",
        &[(planks_at, planks, 1)],
        stack(pickaxe, 1),
    )
    .await;
    let first = TABLE_HOTBAR_FIRST;
    click_table(
        &mut bot,
        first,
        0,
        click::PICKUP,
        &[(first, stack(pickaxe, 1))],
        None,
    )
    .await;
    click_table(
        &mut bot,
        planks_at,
        0,
        click::PICKUP,
        &[(planks_at, None)],
        stack(planks, 1),
    )
    .await;
    click_table(
        &mut bot,
        1,
        0,
        click::PICKUP,
        &[(1, stack(planks, 1))],
        None,
    )
    .await;
    agree_table(
        &mut bot,
        "putting a plank into the grid",
        &[(first, pickaxe, 1), (1, planks, 1), (0, button, 1)],
        None,
    )
    .await;

    // A vanilla client's window 0 shows the same inventory the table's window did.
    let shown = bot.menu.clone().expect("shown");
    for at in 0..36 {
        bot.inventory_window.slots[MAIN_FIRST + at] = shown.slots[TABLE_MAIN_FIRST + at].clone();
    }
    bot.close_window().await.unwrap();
    agree(
        &mut bot,
        "closing the table",
        &[(HOTBAR_FIRST, pickaxe, 1), (HOTBAR_FIRST + 1, planks, 1)],
        None,
    )
    .await;
    let to = mark(&mut bot, "Smith", &mut watcher, false).await;
    nothing_odd(&watcher, from, to);

    let bot = rejoin(&address, "Smith", bot).await;
    assert_eq!(
        seen(&bot.inventory_window.slots),
        slots(
            46,
            &[(HOTBAR_FIRST, pickaxe, 1), (HOTBAR_FIRST + 1, planks, 1)]
        ),
        "window 0 as it comes whole"
    );
    assert_eq!(bot.cursor, None);
    server.stop().await;
}

/// The recipes' property sets and the stonecutter's recipes are sent at a join
/// (`PlayerList.placeNewPlayer` sends `ClientboundUpdateRecipesPacket` of the server's
/// `recipeAccess().getSynchronizedItemProperties()` and `getSynchronizedStonecutterRecipes()`):
/// the furnace's inputs hold an oak log, and the stonecutter cuts stone into stone bricks.
#[tokio::test]
async fn the_recipes_are_sent_at_a_join() {
    let (server, address) = start_with(survival()).await;
    let bot = join(&address, "Reader").await;
    let recipes = bot.recipes.clone().expect("update_recipes was sent");
    let furnace = recipes
        .property_sets
        .iter()
        .find(|set| set.key == "minecraft:furnace_input")
        .unwrap_or_else(|| panic!("no furnace inputs: {:?}", recipes.property_sets));
    assert!(furnace.items.contains(&items::OAK_LOG));
    assert!(
        recipes.stonecutter.iter().any(|recipe| {
            let takes_stone = match &recipe.input {
                clustine_protocol::packets::play::HolderSet::Items(listed) => {
                    listed.contains(&items::STONE)
                }
                clustine_protocol::packets::play::HolderSet::Tag(_) => false,
            };
            takes_stone
                && recipe.display
                    == clustine_protocol::packets::play::SlotDisplay::ItemStack {
                        item: items::STONE_BRICKS,
                        count: 1,
                    }
        }),
        "no stonecutting of stone into stone bricks"
    );
    server.stop().await;
}
