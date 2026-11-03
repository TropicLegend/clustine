//! Tests of crafting in the region (V3c, `docs/groundwork/v-plan.md`, section 4.7): the
//! 2×2 grid of window 0 and the crafting table's 3×3 make what their recipes make; the
//! table opens as a menu whose grid goes back into the inventory when it closes, by the
//! player's close, the table's breaking, a hand-over or a change of level; a creative
//! player's grid slot is set by the client; a spectator's click is answered whole.
//!
//! The regions are those of the tests of R3a (`redstone.rs`): player 1 at the spawn,
//! (0.5, -60, 0.5), with the chunks around chunk (0, 0) loaded.

use clustine_data::{blocks, items};
use clustine_world::EntityData;

use super::redstone::{state, ticking_with};
use super::*;
use crate::api::{ClickKind, MenuKind};

const TABLE: BlockPos = BlockPos::new(3, -60, 3);

fn stack(item: i32, count: i32) -> Option<ItemStack> {
    Some(ItemStack::new(item, count))
}

/// Player 1's input `input` numbered `number`.
fn input(number: u64, input: PlayerInput) -> TickInputs {
    moves(vec![with_number(number, numbered(1, input))])
}

fn use_table(number: u64) -> TickInputs {
    input(
        number,
        PlayerInput::UseItemOn {
            position: TABLE,
            face: Face::Top,
            cursor: [0.5; 3],
            sneaking: false,
            sequence: 1,
        },
    )
}

fn click(number: u64, serial: u64, slot: i16, button: i8, kind: ClickKind) -> TickInputs {
    input(
        number,
        PlayerInput::ContainerClick {
            serial,
            slot,
            button,
            kind,
        },
    )
}

/// A region with a crafting table at `TABLE` whose player 1, in `mode`, holds
/// `inventory` and nothing else.
fn holding(mode: clustine_world::GameMode, inventory: &[(usize, Option<ItemStack>)]) -> Region {
    let mut region = ticking_with(&[(TABLE, state("minecraft:crafting_table"))]);
    let own = region.players.get_mut(&player(1)).expect("joined");
    own.mode = clustine_world::PlayerMode::new(mode);
    own.inventory = Inventory::default();
    for (slot, stack) in inventory {
        own.inventory.set(*slot, stack.clone());
    }
    region.tick(&TickInputs::default());
    region
}

fn survival(inventory: &[(usize, Option<ItemStack>)]) -> Region {
    holding(clustine_world::GameMode::Survival, inventory)
}

/// What player 1 holds of `item`, in the inventory, counted.
fn held(region: &Region, item: i32) -> i32 {
    region
        .player_state(player(1))
        .expect("in")
        .inventory
        .0
        .iter()
        .flatten()
        .filter(|stack| stack.item == item)
        .map(|stack| stack.count)
        .sum()
}

/// The table's grid as the open menu keeps it (index 0 the result).
fn table_grid(region: &Region) -> [Option<ItemStack>; 10] {
    let menu = region
        .player_state(player(1))
        .expect("in")
        .menu
        .expect("a menu open");
    *menu.grid.expect("a crafting table's grid")
}

#[test]
fn a_use_of_a_crafting_table_opens_the_crafting_menu() {
    let mut region = survival(&[(0, stack(items::OAK_PLANKS, 3))]);
    let output = region.tick(&use_table(41));
    let opened: Vec<&PlayerEvent> = output
        .player_events
        .iter()
        .filter(|(id, event)| {
            *id == player(1) && matches!(event, PlayerEvent::MenuOpened { serial: 41, .. })
        })
        .map(|(_, event)| event)
        .collect();
    let [
        PlayerEvent::MenuOpened {
            kind,
            title,
            owner,
            slots,
            carried,
            ..
        },
    ] = &opened[..]
    else {
        panic!("{:?}", output.player_events);
    };
    // `CraftingTableBlock.getMenuProvider`: `container.crafting`; ten slots of the
    // table's own (the result and the 3×3 grid), then the inventory's 36.
    assert_eq!(*kind, MenuKind::Crafting);
    assert_eq!(title, "container.crafting");
    assert_eq!(*owner, None);
    assert_eq!(slots.len(), 46);
    assert!(slots[..10].iter().all(Option::is_none));
    assert_eq!(slots[37], stack(items::OAK_PLANKS, 3));
    assert_eq!(*carried, None);
}

#[test]
fn a_close_of_the_table_puts_the_cursor_then_the_grid_back() {
    let mut region = survival(&[]);
    region.tick(&use_table(41));
    let menu = region
        .players
        .get_mut(&player(1))
        .expect("in")
        .menu
        .as_mut()
        .expect("open");
    menu.carried = stack(items::DIRT, 5);
    let grid = menu.grid.as_mut().expect("a grid");
    grid[1] = stack(items::OAK_PLANKS, 2);
    grid[9] = stack(items::STICK, 1);
    region.tick(&input(42, PlayerInput::CloseContainer { serial: 41 }));
    let state = region.player_state(player(1)).expect("in");
    assert_eq!(state.menu, None);
    // `CraftingMenu.removed`: the cursor first (`AbstractContainerMenu.removed`), then
    // `clearContainer` over the grid in order, each by `placeItemBackInInventory`.
    assert_eq!(state.inventory.0[0], stack(items::DIRT, 5));
    assert_eq!(state.inventory.0[1], stack(items::OAK_PLANKS, 2));
    assert_eq!(state.inventory.0[2], stack(items::STICK, 1));
}

#[test]
fn a_broken_table_closes_its_menu_and_gives_the_grid_back() {
    let mut region = survival(&[]);
    region.tick(&use_table(41));
    let menu = region
        .players
        .get_mut(&player(1))
        .expect("in")
        .menu
        .as_mut()
        .expect("open");
    menu.grid.as_mut().expect("a grid")[5] = stack(items::OAK_LOG, 4);
    let mut output = TickOutput::default();
    region.set_block(TABLE, blocks::AIR, &mut output);
    // `stillValid` (`AbstractContainerMenu.stillValid` over `CRAFTING_TABLE`) fails at
    // the next tick, and the menu is closed as by the player.
    let output = region.tick(&TickInputs::default());
    assert!(
        output.player_events.iter().any(|(id, event)| {
            *id == player(1) && matches!(event, PlayerEvent::MenuClosed { serial: 41, .. })
        }),
        "{:?}",
        output.player_events
    );
    assert_eq!(held(&region, items::OAK_LOG), 4);
}

/// A player who walks into another region's land with the table open has it closed in
/// that tick, as any menu (`menus.rs`): the grid goes back into the inventory their
/// transfer carries.
#[test]
fn a_hand_over_closes_the_table_and_the_grid_travels_in_the_inventory() {
    let mut region = region_in(EAST);
    region.tick(&changes(vec![arrive(5, &transfer(5, 40, 2.5, 0))]));
    let arrived = region.players.get_mut(&player(5)).expect("arrived");
    let mut grid: Box<[Option<ItemStack>; 10]> = Box::default();
    grid[1] = stack(items::OAK_PLANKS, 3);
    arrived.menu = Some(crate::state::Menu {
        serial: 1,
        of: crate::state::MenuOf::CraftingTable {
            at: BlockPos::new(2, -60, 0),
        },
        kind: MenuKind::Crafting,
        carried: None,
        drag: None,
        trade: None,
        grid: Some(grid),
    });
    let step = as_entity(40, walk(5, -0.5, 0.5));
    let output = region.tick(&moves(vec![with_number(1, step)]));
    let carried: Vec<i32> = output
        .durable
        .iter()
        .filter_map(|(_, _, entry)| match entry {
            Durable::Departed { transfer, .. } => Some(
                transfer
                    .inventory
                    .0
                    .iter()
                    .flatten()
                    .filter(|stack| stack.item == items::OAK_PLANKS)
                    .map(|stack| stack.count)
                    .sum(),
            ),
            _ => None,
        })
        .collect();
    assert_eq!(carried, [3]);
}

/// `ServerGamePacketListenerImpl.handleSetCreativeModeSlot` takes slots 1-4 of window 0
/// too: a creative player's grid is set by the client, and the result is computed from
/// it (`InventoryMenu.slotsChanged` → `slotChangedCraftingGrid`).
#[test]
fn a_creative_players_grid_slot_is_set_and_the_result_computed() {
    let mut region = holding(clustine_world::GameMode::Creative, &[]);
    region.tick(&input(
        5,
        PlayerInput::SetCraftingSlot {
            slot: 0,
            stack: stack(items::OAK_LOG, 1),
        },
    ));
    let crafting = region.player_state(player(1)).expect("in").crafting;
    assert_eq!(crafting.grid[0], stack(items::OAK_LOG, 1));
    assert_eq!(crafting.result, stack(items::OAK_PLANKS, 4));
}

/// `ServerGamePacketListenerImpl.handleContainerClick` @…: a spectator's click changes
/// nothing, and the client is sent the menu whole to undo what it predicted.
#[test]
fn a_spectators_click_on_the_table_is_answered_with_the_menu_whole() {
    let mut region = survival(&[(0, stack(items::OAK_LOG, 1))]);
    region.tick(&use_table(41));
    region.players.get_mut(&player(1)).expect("in").mode =
        clustine_world::PlayerMode::new(clustine_world::GameMode::Spectator);
    let output = region.tick(&click(42, 41, 37, 0, ClickKind::Pickup));
    assert!(
        output.player_events.iter().any(|(id, event)| {
            *id == player(1)
                && matches!(event, PlayerEvent::MenuOpened { serial: 41, slots, .. } if slots.len() == 46)
        }),
        "{:?}",
        output.player_events
    );
    assert_eq!(held(&region, items::OAK_LOG), 1);
}

/// Window 0's 2×2 grid: a log put in makes four planks in the result, and taking them
/// consumes the log (`ResultSlot.onTake`).
#[test]
fn a_log_in_the_inventory_grid_crafts_four_planks() {
    let mut region = survival(&[(0, stack(items::OAK_LOG, 2))]);
    // The hotbar's first slot is window 0's 36; one log put in the grid's first slot.
    region.tick(&click(5, 0, 36, 0, ClickKind::Pickup));
    region.tick(&click(6, 0, 1, 1, ClickKind::Pickup));
    let crafting = region.player_state(player(1)).expect("in").crafting;
    assert_eq!(crafting.grid[0], stack(items::OAK_LOG, 1));
    assert_eq!(crafting.result, stack(items::OAK_PLANKS, 4));
    // The other log back to the hotbar, then the result taken by a shift-click.
    region.tick(&click(7, 0, 36, 0, ClickKind::Pickup));
    region.tick(&click(8, 0, 0, 0, ClickKind::QuickMove));
    assert_eq!(held(&region, items::OAK_PLANKS), 4);
    assert_eq!(held(&region, items::OAK_LOG), 1);
    let crafting = region.player_state(player(1)).expect("in").crafting;
    assert!(crafting.is_empty(), "{crafting:?}");
}

/// The table's 3×3 grid: three planks over two sticks make a wooden pickaxe, taken by a
/// shift-click into the inventory.
#[test]
fn planks_and_sticks_at_the_table_craft_a_pickaxe() {
    let mut region = survival(&[
        (0, stack(items::OAK_PLANKS, 3)),
        (1, stack(items::STICK, 2)),
    ]);
    region.tick(&use_table(41));
    // The hotbar is the table's 37-45; one of each put down by a right-click.
    region.tick(&click(42, 41, 37, 0, ClickKind::Pickup));
    for (number, slot) in [(43, 1), (44, 2), (45, 3)] {
        region.tick(&click(number, 41, slot, 1, ClickKind::Pickup));
    }
    region.tick(&click(46, 41, 38, 0, ClickKind::Pickup));
    for (number, slot) in [(47, 5), (48, 8)] {
        region.tick(&click(number, 41, slot, 1, ClickKind::Pickup));
    }
    assert_eq!(table_grid(&region)[0], stack(items::WOODEN_PICKAXE, 1));
    region.tick(&click(49, 41, 0, 0, ClickKind::QuickMove));
    assert_eq!(held(&region, items::WOODEN_PICKAXE), 1);
    assert!(table_grid(&region).iter().all(Option::is_none));
}

/// The rest of a shift-click on the result that has no room is dropped as a craft drops
/// (`player.drop(stack, false, PREDICTED)`): no thrower kept and no swing, where a
/// click's throw keeps both.
#[test]
fn a_crafts_rest_without_room_is_dropped_with_no_thrower_and_no_swing() {
    let mut inventory: Vec<(usize, Option<ItemStack>)> = (0..36)
        .map(|slot| (slot, stack(items::STONE, 64)))
        .collect();
    inventory[9] = (9, stack(items::OAK_PLANKS, 62));
    let mut region = survival(&inventory);
    let own = region.players.get_mut(&player(1)).expect("in");
    own.crafting.grid[0] = stack(items::OAK_LOG, 1);
    own.crafting.result = stack(items::OAK_PLANKS, 4);
    region.tick(&TickInputs::default());
    let output = region.tick(&click(5, 0, 0, 0, ClickKind::QuickMove));
    assert_eq!(held(&region, items::OAK_PLANKS), 64);
    assert_eq!(held(&region, items::OAK_LOG), 0);
    let dropped: Vec<_> = super::super::tnt::tests::records(&region)
        .into_iter()
        .filter_map(|entity| match entity.data {
            EntityData::Item { stack, thrower, .. } if stack.item == items::OAK_PLANKS => {
                Some((stack.count, thrower))
            }
            _ => None,
        })
        .collect();
    assert_eq!(dropped, [(2, None)]);
    assert!(
        !output
            .events
            .iter()
            .any(|event| matches!(event, RegionEvent::ArmSwung { .. })),
        "{:?}",
        output.events
    );
}
