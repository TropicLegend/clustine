//! Comparisons with the official server of crafting (V3c): the same script of clicks of a
//! survival player is run on both servers, and after each click the window's slots, the
//! cursor and the inventory are compared, once each server has handled the click and
//! the tick after it. States are compared, not packets: Clustine sends a changed result
//! once per click, where the game may send it more than once (`slotChangedCraftingGrid`
//! runs at every change of the grid inside a click).
//!
//! The script, each click guessed as a vanilla client guesses it (the grid's stacks
//! taken from, and the result slot empty after a take, since the client works out no
//! result):
//!
//! 1. Window 0 (`InventoryMenu`): a log into the 2×2 grid, its four planks taken by a
//!    click (`ResultSlot.onTake`), then two logs and the result shift-clicked, which the
//!    game repeats while the grid makes the same item (`AbstractContainerMenu.doClick`
//!    @698-733) into the stack of planks (`InventoryMenu.quickMoveStack` @45-75).
//! 2. A crafting table placed and used (`CraftingTableBlock.useWithoutItem`, a
//!    `CraftingMenu` of 46 slots): three planks over two sticks, the wooden pickaxe taken
//!    by a click.
//! 3. A cake of three milk buckets, two sugar, an egg and three wheat, given with the
//!    table shut and laid out once it is open again, taken by a click: each milk bucket leaves its bucket in its slot
//!    (`ResultSlot.onTake`, the recipe's remainders, `Item.getCraftingRemainder`).
//! 4. A shift-click of the planks from the inventory into the table's grid
//!    (`CraftingMenu.quickMoveStack` @87-106: `moveItemStackTo(stack, 1, 10, false)`, into
//!    the first empty grid slot).
//! 5. The close, which puts the grid back into the inventory (`CraftingMenu.removed`,
//!    `clearContainer`, `Inventory.placeItemBackInInventory`).
//!
//! The recipes sent at the join (`update_recipes`) are compared too: the property sets
//! (a map, so sorted) and the stonecutter's recipes in the order sent, which is the order
//! of the stonecutter's buttons.
//!
//! The player is a survival one on both: the official server's world is creative, so its
//! console sets the mode (`gamemode survival`) and gives (`give`); Clustine's world is
//! survival and the operator's orders give. They need Java, the server jar downloaded by
//! `cargo datagen`, and your agreement to the Minecraft EULA, so they are ignored by
//! default. Run them on a quiet machine:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test crafting_oracle -- --ignored --nocapture --test-threads 1
//! ```

mod common;

use std::time::Duration;

use anyhow::{Context, Result, ensure};
use clustine::Config;
use clustine_botswarm::{Bot, Oracle};
use clustine_data::{blocks, items};
use clustine_protocol::item::ItemStack;
use clustine_protocol::packets::play::{UpdateRecipes, click, face};
use clustine_sim::api::Order;
use clustine_world::GameMode;

use common::{config, start_with};

const PATIENCE: Duration = Duration::from_secs(30);

/// The bot's name on both servers.
const NAME: &str = "Crafter";

/// The hotbar slot the bot holds, kept empty: a round trip's use of the ground with it
/// places nothing. Nothing the script does puts anything there (gives fill the hotbar
/// from its first slot; the only shift-click of a result in window 0 merges into a
/// stack).
const HELD: i16 = 8;

/// Window 0's slots (`InventoryMenu.<init>`).
const MAIN_FIRST: usize = 9;
const HOTBAR_FIRST: usize = 36;
/// The table's (`CraftingMenu.<init>`).
const TABLE_MAIN_FIRST: usize = 10;
const TABLE_HOTBAR_FIRST: usize = 37;

/// What each item is called in the official server's `give`.
const NAMES: [(i32, &str); 8] = [
    (items::OAK_LOG, "oak_log"),
    (items::CRAFTING_TABLE, "crafting_table"),
    (items::STICK, "stick"),
    (items::MILK_BUCKET, "milk_bucket"),
    (items::SUGAR, "sugar"),
    (items::EGG, "egg"),
    (items::WHEAT, "wheat"),
    (items::OAK_PLANKS, "oak_planks"),
];

/// The server a run plays on, and how it gives.
enum Giver<'a> {
    Official(&'a Oracle),
    Clustine(&'a clustine::Server),
}

impl Giver<'_> {
    async fn give(&self, item: i32, count: i32) -> Result<()> {
        match self {
            Self::Official(oracle) => {
                let (_, name) = NAMES
                    .iter()
                    .find(|(id, _)| *id == item)
                    .with_context(|| format!("no name for item {item}"))?;
                oracle
                    .command(&format!("give {NAME} minecraft:{name} {count}"), &["Gave"])
                    .await?;
            }
            Self::Clustine(server) => {
                server.order(NAME, Order::Give { item, count }).await?;
            }
        }
        Ok(())
    }
}

/// What a client shows after a step: the window shown and its slots, or window 0's when
/// none is (a bot's window 0 is not kept up to date while another window is shown, as a
/// vanilla client's is through the inventory both share), and the cursor.
#[derive(Debug, PartialEq)]
struct Shown {
    window: Option<(i32, Vec<Option<ItemStack>>)>,
    window_0: Option<Vec<Option<ItemStack>>>,
    cursor: Option<ItemStack>,
}

fn shown(bot: &Bot) -> Shown {
    match &bot.menu {
        Some(menu) => Shown {
            window: Some((menu.menu, menu.slots.clone())),
            window_0: None,
            cursor: bot.cursor.clone(),
        },
        None => Shown {
            window: None,
            window_0: Some(bot.inventory_window.slots.clone()),
            cursor: bot.cursor.clone(),
        },
    }
}

/// A run: the recipes sent at the join, then what was shown after each step.
#[derive(Debug, Default)]
struct Ran {
    recipes: Option<UpdateRecipes>,
    steps: Vec<(String, Shown)>,
}

fn stack(item: i32, count: i32) -> Option<ItemStack> {
    Some(ItemStack::new(item, count))
}

/// The slot of the window shown (window 0 where none is) that the inventory's slot
/// `index` (the hotbar first, then the main inventory) is.
fn window_slot(bot: &Bot, index: usize) -> usize {
    let (main, hotbar) = if bot.menu.is_some() {
        (TABLE_MAIN_FIRST, TABLE_HOTBAR_FIRST)
    } else {
        (MAIN_FIRST, HOTBAR_FIRST)
    };
    if index < 9 {
        hotbar + index
    } else {
        main + index - 9
    }
}

/// The inventory's 36 slots as the window shown says (window 0 where none is).
fn inventory(bot: &Bot) -> Vec<Option<ItemStack>> {
    let slots = match &bot.menu {
        Some(menu) => &menu.slots,
        None => &bot.inventory_window.slots,
    };
    (0..36)
        .map(|index| slots.get(window_slot(bot, index)).cloned().flatten())
        .collect()
}

/// The window slots of the inventory that hold `item`, in the inventory's order.
fn holding(bot: &Bot, item: i32) -> Vec<usize> {
    inventory(bot)
        .iter()
        .enumerate()
        .filter(|(_, slot)| slot.as_ref().is_some_and(|stack| stack.item == item))
        .map(|(index, _)| window_slot(bot, index))
        .collect()
}

fn count_of(bot: &Bot, item: i32) -> i32 {
    inventory(bot)
        .iter()
        .flatten()
        .filter(|stack| stack.item == item)
        .map(|stack| stack.count)
        .sum()
}

/// Waits until the server has handled everything the bot sent before: a use of the
/// ground beside it with the empty hand, which changes nothing and is acknowledged.
async fn round_trip(bot: &mut Bot) -> Result<()> {
    let (x, z) = (bot.location.0.floor() as i32, bot.location.2.floor() as i32);
    let sequence = bot.use_item_on(x + 1, -61, z - 1, face::TOP).await?;
    bot.wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= sequence)
        .await
        .context("no acknowledgement")
}

/// Ends a step: what the bot shows once the server has handled what it sent, and the
/// tick after it (two round trips, each acknowledged at the end of a tick).
async fn step(bot: &mut Bot, ran: &mut Ran, name: impl Into<String>) -> Result<()> {
    round_trip(bot).await?;
    round_trip(bot).await?;
    ran.steps.push((name.into(), shown(bot)));
    Ok(())
}

/// Clicks the slot `slot` of the window shown, or window 0's where none is, saying what
/// a vanilla client guesses; then ends the step.
async fn click(
    bot: &mut Bot,
    ran: &mut Ran,
    name: impl Into<String>,
    (slot, button, input): (usize, i8, i32),
    changed: Vec<(usize, Option<ItemStack>)>,
    carried: Option<ItemStack>,
) -> Result<()> {
    let changed = changed
        .into_iter()
        .map(|(at, stack)| (at as i16, stack))
        .collect();
    if bot.menu.is_some() {
        bot.click_window(slot as i16, button, input, changed, carried)
            .await?;
    } else {
        bot.click_inventory(slot as i16, button, input, changed, carried)
            .await?;
    }
    step(bot, ran, name).await
}

/// Picks up the whole stack of `slot` with a left click.
async fn pick_up(bot: &mut Bot, ran: &mut Ran, slot: usize) -> Result<()> {
    let stack = inventory_or_window(bot, slot);
    click(
        bot,
        ran,
        format!("pick up slot {slot}"),
        (slot, 0, click::PICKUP),
        vec![(slot, None)],
        stack,
    )
    .await
}

/// Puts the whole cursor down on the empty `slot` with a left click.
async fn put_down(bot: &mut Bot, ran: &mut Ran, slot: usize) -> Result<()> {
    let carried = bot.cursor.clone();
    click(
        bot,
        ran,
        format!("put the cursor down on slot {slot}"),
        (slot, 0, click::PICKUP),
        vec![(slot, carried)],
        None,
    )
    .await
}

/// Puts one of the cursor down on the empty `slot` with a right click.
async fn put_one(bot: &mut Bot, ran: &mut Ran, slot: usize) -> Result<()> {
    let carried = bot.cursor.clone().context("nothing carried")?;
    let one = stack(carried.item, 1);
    let left = (carried.count > 1).then(|| ItemStack {
        count: carried.count - 1,
        ..carried
    });
    click(
        bot,
        ran,
        format!("put one down on slot {slot}"),
        (slot, 1, click::PICKUP),
        vec![(slot, one)],
        left,
    )
    .await
}

/// What the slot `slot` of the window shown (window 0 where none is) holds.
fn inventory_or_window(bot: &Bot, slot: usize) -> Option<ItemStack> {
    match &bot.menu {
        Some(menu) => menu.slots.get(slot).cloned().flatten(),
        None => bot.inventory_window.slots.get(slot).cloned().flatten(),
    }
}

/// Gives `count` of `item` and waits until the inventory shows them; then ends the step.
async fn give(
    bot: &mut Bot,
    giver: &Giver<'_>,
    ran: &mut Ran,
    item: i32,
    count: i32,
) -> Result<()> {
    let before = count_of(bot, item);
    giver.give(item, count).await?;
    bot.wait_until(PATIENCE, |bot| count_of(bot, item) >= before + count)
        .await
        .with_context(|| format!("{count} of item {item} were not given"))?;
    step(bot, ran, format!("given {count} of item {item}")).await
}

/// Uses the table at `(x, y, z)` and waits for its window; then ends the step.
async fn open_table(
    bot: &mut Bot,
    ran: &mut Ran,
    (x, y, z): (i32, i32, i32),
    name: &str,
) -> Result<()> {
    bot.use_item_on(x, y, z, face::TOP).await?;
    bot.wait_until(PATIENCE, |bot| {
        bot.menu.as_ref().is_some_and(|menu| !menu.slots.is_empty())
    })
    .await
    .context("the table did not open")?;
    step(bot, ran, name).await
}

/// Closes the table's window; then ends the step. A vanilla client's window 0 shows the
/// inventory the table's window did (the two share it), which the bot's is made to show
/// before the server's answer comes.
async fn close_table(bot: &mut Bot, ran: &mut Ran, name: &str) -> Result<()> {
    let table_window = bot.menu.clone().context("the table's window")?;
    for at in 0..36 {
        bot.inventory_window.slots[MAIN_FIRST + at] =
            table_window.slots[TABLE_MAIN_FIRST + at].clone();
    }
    bot.close_window().await?;
    step(bot, ran, name).await
}

async fn run(address: &str, giver: Giver<'_>) -> Result<Ran> {
    let mut bot = Bot::join(address, NAME).await?;
    bot.wait_for_chunks(9, PATIENCE).await?;
    if let Giver::Official(oracle) = giver {
        oracle
            .command(&format!("gamemode survival {NAME}"), &["game mode"])
            .await?;
    }
    bot.select_slot(HELD).await?;
    round_trip(&mut bot).await?;
    let mut ran = Ran {
        recipes: bot.recipes.clone(),
        ..Ran::default()
    };
    let (log, planks) = (items::OAK_LOG, items::OAK_PLANKS);

    // 1. Window 0: a log into the grid, its planks taken by a click.
    give(&mut bot, &giver, &mut ran, log, 3).await?;
    let logs = *holding(&bot, log).first().context("no logs")?;
    pick_up(&mut bot, &mut ran, logs).await?;
    put_one(&mut bot, &mut ran, 1).await?;
    put_down(&mut bot, &mut ran, logs).await?;
    click(
        &mut bot,
        &mut ran,
        "take the planks by a click",
        (0, 0, click::PICKUP),
        vec![(0, None), (1, None)],
        stack(planks, 4),
    )
    .await?;
    put_down(&mut bot, &mut ran, MAIN_FIRST).await?;
    // Two logs in, the result shift-clicked: the client guesses one craft.
    pick_up(&mut bot, &mut ran, logs).await?;
    put_down(&mut bot, &mut ran, 1).await?;
    click(
        &mut bot,
        &mut ran,
        "shift-click the planks",
        (0, 0, click::QUICK_MOVE),
        vec![
            (0, None),
            (1, stack(log, 1)),
            (MAIN_FIRST, stack(planks, 8)),
        ],
        None,
    )
    .await?;

    // 2. The table placed and used: three planks over two sticks.
    give(&mut bot, &giver, &mut ran, items::CRAFTING_TABLE, 1).await?;
    let table = *holding(&bot, items::CRAFTING_TABLE)
        .first()
        .context("no table")?;
    let table_hotbar = i16::try_from(table - HOTBAR_FIRST).context("the table in the hotbar")?;
    let (x, z) = (
        bot.location.0.floor() as i32 + 2,
        bot.location.2.floor() as i32,
    );
    bot.select_slot(table_hotbar).await?;
    bot.use_item_on(x, -61, z, face::TOP).await?;
    bot.wait_until(PATIENCE, |bot| {
        bot.block_at(x, -60, z).unwrap().is_some_and(|state| {
            clustine_data::BlockState(state as u16).block_id() == blocks::CRAFTING_TABLE.block_id()
        })
    })
    .await
    .context("the table was not placed")?;
    bot.select_slot(HELD).await?;
    step(&mut bot, &mut ran, "place the table").await?;
    give(&mut bot, &giver, &mut ran, items::STICK, 2).await?;
    open_table(&mut bot, &mut ran, (x, -60, z), "open the table").await?;

    let planks_at = *holding(&bot, planks).first().context("no planks")?;
    pick_up(&mut bot, &mut ran, planks_at).await?;
    for at in [1, 2, 3] {
        put_one(&mut bot, &mut ran, at).await?;
    }
    put_down(&mut bot, &mut ran, planks_at).await?;
    let sticks = *holding(&bot, items::STICK).first().context("no sticks")?;
    pick_up(&mut bot, &mut ran, sticks).await?;
    for at in [5, 8] {
        put_one(&mut bot, &mut ran, at).await?;
    }
    click(
        &mut bot,
        &mut ran,
        "take the pickaxe by a click",
        (0, 0, click::PICKUP),
        vec![
            (0, None),
            (1, None),
            (2, None),
            (3, None),
            (5, None),
            (8, None),
        ],
        stack(items::WOODEN_PICKAXE, 1),
    )
    .await?;
    put_down(&mut bot, &mut ran, sticks).await?;

    // 3. A cake, its milk buckets leaving their buckets. What it is made of is given
    // with the table shut: a give while it is open is told by the game in the table's
    // window and may be told in window 0, which a vanilla client shows alike (the two
    // share the inventory) and a bot does not.
    close_table(&mut bot, &mut ran, "close the empty table").await?;
    give(&mut bot, &giver, &mut ran, items::MILK_BUCKET, 3).await?;
    give(&mut bot, &giver, &mut ran, items::SUGAR, 2).await?;
    give(&mut bot, &giver, &mut ran, items::EGG, 1).await?;
    give(&mut bot, &giver, &mut ran, items::WHEAT, 3).await?;
    open_table(&mut bot, &mut ran, (x, -60, z), "open the table again").await?;
    let milk = holding(&bot, items::MILK_BUCKET);
    ensure!(milk.len() == 3, "three milk buckets: {milk:?}");
    for (from, to) in milk.into_iter().zip([1, 2, 3]) {
        pick_up(&mut bot, &mut ran, from).await?;
        put_down(&mut bot, &mut ran, to).await?;
    }
    let sugar = *holding(&bot, items::SUGAR).first().context("no sugar")?;
    pick_up(&mut bot, &mut ran, sugar).await?;
    put_one(&mut bot, &mut ran, 4).await?;
    put_one(&mut bot, &mut ran, 6).await?;
    let egg = *holding(&bot, items::EGG).first().context("no egg")?;
    pick_up(&mut bot, &mut ran, egg).await?;
    put_down(&mut bot, &mut ran, 5).await?;
    let wheat = *holding(&bot, items::WHEAT).first().context("no wheat")?;
    pick_up(&mut bot, &mut ran, wheat).await?;
    for at in [7, 8, 9] {
        put_one(&mut bot, &mut ran, at).await?;
    }
    // The client works out the remainders as the game's default does
    // (`CraftingRecipe.defaultCraftingReminder`): a bucket for each milk bucket.
    let mut changed = vec![(0, None)];
    changed.extend((1..=3).map(|at| (at, stack(items::BUCKET, 1))));
    changed.extend((4..=9).map(|at| (at, None)));
    click(
        &mut bot,
        &mut ran,
        "take the cake by a click",
        (0, 0, click::PICKUP),
        changed,
        stack(items::CAKE, 1),
    )
    .await?;
    let empty = inventory(&bot)
        .iter()
        .enumerate()
        .skip(9)
        .find(|(_, slot)| slot.is_none())
        .map(|(index, _)| window_slot(&bot, index))
        .context("no empty slot in the main inventory")?;
    put_down(&mut bot, &mut ran, empty).await?;

    // 4. The planks shift-clicked into the grid's first empty slot.
    let planks_at = *holding(&bot, planks).first().context("no planks")?;
    let moved = inventory_or_window(&bot, planks_at);
    click(
        &mut bot,
        &mut ran,
        "shift-click the planks into the grid",
        (planks_at, 0, click::QUICK_MOVE),
        vec![(planks_at, None), (4, moved)],
        None,
    )
    .await?;

    // 5. The close, which gives the buckets and the planks back.
    close_table(&mut bot, &mut ran, "close the table").await?;
    bot.close().await?;
    Ok(ran)
}

/// `update_recipes`'s property sets sorted (a map, whose order is a hash map's), each
/// set's items sorted.
fn property_sets(recipes: &UpdateRecipes) -> Vec<(String, Vec<i32>)> {
    let mut sets: Vec<_> = recipes
        .property_sets
        .iter()
        .map(|set| {
            let mut items = set.items.clone();
            items.sort_unstable();
            (set.key.clone(), items)
        })
        .collect();
    sets.sort();
    sets
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn crafting_runs_as_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let theirs = run(oracle.address(), Giver::Official(&oracle)).await;
    drop(oracle);
    let survival = Config {
        game_mode: GameMode::Survival,
        force_game_mode: true,
        ..config()
    };
    let (server, address) = start_with(survival).await;
    let ours = run(&address, Giver::Clustine(&server)).await;
    server.stop().await;
    let theirs = theirs.unwrap_or_else(|error| panic!("the official server: {error:#}"));
    let ours = ours.unwrap_or_else(|error| panic!("Clustine: {error:#}"));

    let mut differences = Vec::new();
    match (&theirs.recipes, &ours.recipes) {
        (Some(their), Some(our)) => {
            if property_sets(their) != property_sets(our) {
                println!(
                    "property sets:\n  official: {:?}\n  Clustine: {:?}",
                    property_sets(their),
                    property_sets(our)
                );
                differences.push("the recipes' property sets".to_owned());
            }
            if their.stonecutter != our.stonecutter {
                println!(
                    "stonecutter recipes:\n  official: {:?}\n  Clustine: {:?}",
                    their.stonecutter, our.stonecutter
                );
                differences.push("the stonecutter's recipes, in order".to_owned());
            }
        }
        (their, our) => differences.push(format!(
            "update_recipes sent: official {}, Clustine {}",
            their.is_some(),
            our.is_some()
        )),
    }
    for ((name, their), (_, our)) in theirs.steps.iter().zip(&ours.steps) {
        println!("{name}:\n  official: {their:?}\n  Clustine: {our:?}");
        if their != our {
            differences.push(name.clone());
        }
    }
    assert_eq!(theirs.steps.len(), ours.steps.len(), "the steps run");
    assert!(
        differences.is_empty(),
        "these differ, printed above: {differences:?}"
    );
}
