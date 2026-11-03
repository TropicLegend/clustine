//! Tests of the player's own inventory screen, window 0, in the region (V3,
//! `docs/groundwork/v-plan.md`, section 4.7; `inventory_menu.rs`): its clicks run on the
//! inventory and the grid and are answered as serial 0, a click while a chest is open is
//! passed over, its close puts the grid and the cursor back, and its grid goes with the
//! player across a hand-over and into their place.
//!
//! The regions are those of the tests of R3a (`redstone.rs`): player 1 at the spawn, with
//! the chunks around chunk (0, 0) loaded.

use clustine_data::items;

use super::redstone::{state, ticking_with};
use super::*;
use crate::api::{ClickKind, MenuKind};
use crate::state::Crafting;

const CHEST: BlockPos = BlockPos::new(3, -60, 3);

fn stack(item: i32, count: i32) -> Option<ItemStack> {
    Some(ItemStack::new(item, count))
}

fn input(number: u64, input: PlayerInput) -> TickInputs {
    moves(vec![with_number(number, numbered(1, input))])
}

fn click_window_0(number: u64, slot: i16, kind: ClickKind) -> TickInputs {
    input(
        number,
        PlayerInput::ContainerClick {
            serial: 0,
            slot,
            button: 0,
            kind,
        },
    )
}

/// Window 0's reports to player 1 in `output`: its changes and its wholes.
fn window_0(output: &TickOutput) -> Vec<PlayerEvent> {
    output
        .player_events
        .iter()
        .filter(|(id, _)| *id == player(1))
        .map(|(_, event)| event.clone())
        .filter(|event| {
            matches!(
                event,
                PlayerEvent::MenuChanged { serial: 0, .. }
                    | PlayerEvent::MenuOpened { serial: 0, .. }
            )
        })
        .collect()
}

/// A region whose player 1, in survival, holds `inventory` and nothing else.
fn holding(inventory: &[(usize, Option<ItemStack>)]) -> Region {
    let mut region = ticking_with(&[(
        CHEST,
        state("minecraft:chest[facing=north,type=single,waterlogged=false]"),
    )]);
    let own = region.players.get_mut(&player(1)).expect("joined");
    own.mode = clustine_world::PlayerMode::new(clustine_world::GameMode::Survival);
    own.inventory = Inventory::default();
    for (slot, stack) in inventory {
        own.inventory.set(*slot, stack.clone());
    }
    // A tick for the region to take the inventory as it is now as reported.
    region.tick(&TickInputs::default());
    region
}

#[test]
fn a_click_on_window_0_moves_a_stack_and_is_answered_as_serial_0() {
    let mut region = holding(&[(0, stack(items::STONE, 64))]);
    // The hotbar's first slot is window 0's 36: picked up onto the cursor.
    let output = region.tick(&click_window_0(5, 36, ClickKind::Pickup));
    assert_eq!(
        window_0(&output),
        [PlayerEvent::MenuChanged {
            serial: 0,
            applied: 5,
            slots: vec![(36, None)],
            carried: Some(stack(items::STONE, 64)),
        }]
    );
    // Put down in the main inventory's first slot, window 0's 9.
    let output = region.tick(&click_window_0(6, 9, ClickKind::Pickup));
    assert_eq!(
        window_0(&output),
        [PlayerEvent::MenuChanged {
            serial: 0,
            applied: 6,
            slots: vec![(9, stack(items::STONE, 64))],
            carried: Some(None),
        }]
    );
    let state = region.player_state(player(1)).expect("in");
    assert_eq!(state.inventory.0[9], stack(items::STONE, 64));
    assert_eq!(state.inventory.0[0], None);
}

#[test]
fn a_click_that_changes_nothing_is_answered_with_nothing() {
    let mut region = holding(&[]);
    let output = region.tick(&click_window_0(5, 20, ClickKind::Pickup));
    assert_eq!(
        window_0(&output),
        [PlayerEvent::MenuChanged {
            serial: 0,
            applied: 5,
            slots: Vec::new(),
            carried: None,
        }]
    );
}

#[test]
fn a_helmet_put_on_by_the_screen_plays_its_equip_sound_and_is_shown_to_others() {
    let mut region = holding(&[(0, stack(items::IRON_HELMET, 1))]);
    let output = region.tick(&click_window_0(5, 36, ClickKind::QuickMove));
    let state = region.player_state(player(1)).expect("in");
    assert_eq!(state.inventory.0[39], stack(items::IRON_HELMET, 1));
    let sounds: Vec<i32> = output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound { sound, .. } => Some(*sound),
            _ => None,
        })
        .collect();
    assert_eq!(sounds, [clustine_data::sounds::ITEM_ARMOR_EQUIP_IRON]);
    assert!(
        output.events.iter().any(|event| matches!(
            event,
            RegionEvent::EquipmentChanged { slots, .. }
                if slots.iter().any(|(slot, stack)| *slot == 5 && stack.is_some())
        )),
        "{:?}",
        output.events
    );
}

#[test]
fn a_click_on_window_0_while_a_chest_is_open_is_passed_over_without_an_answer() {
    let mut region = holding(&[(0, stack(items::STONE, 64))]);
    region.tick(&input(
        4,
        PlayerInput::UseItemOn {
            position: CHEST,
            face: Face::Top,
            cursor: [0.5; 3],
            sneaking: false,
            sequence: 1,
        },
    ));
    assert!(region.player_state(player(1)).expect("in").menu.is_some());
    let output = region.tick(&click_window_0(5, 36, ClickKind::Pickup));
    assert_eq!(window_0(&output), []);
    let state = region.player_state(player(1)).expect("in");
    assert_eq!(state.inventory.0[0], stack(items::STONE, 64));
    assert_eq!(state.crafting.carried, None);
}

#[test]
fn a_close_of_window_0_puts_the_cursor_then_the_grid_back_and_reports_the_grid_emptied() {
    let mut region = holding(&[]);
    region.players.get_mut(&player(1)).expect("in").crafting = Crafting {
        grid: [stack(items::OAK_LOG, 1), None, None, stack(items::STONE, 3)],
        carried: stack(items::DIRT, 5),
        ..Crafting::default()
    };
    region.tick(&TickInputs::default());
    let output = region.tick(&input(5, PlayerInput::CloseContainer { serial: 0 }));
    let state = region.player_state(player(1)).expect("in");
    assert!(state.crafting.is_empty());
    // `placeItemBackInInventory`: the first free slot of the hotbar first, in the order
    // the game puts them back, the cursor first.
    assert_eq!(state.inventory.0[0], stack(items::DIRT, 5));
    assert_eq!(state.inventory.0[1], stack(items::OAK_LOG, 1));
    assert_eq!(state.inventory.0[2], stack(items::STONE, 3));
    let [PlayerEvent::MenuChanged { slots, carried, .. }] = &window_0(&output)[..] else {
        panic!("{:?}", output.player_events);
    };
    assert_eq!(*carried, Some(None));
    for emptied in [1, 4] {
        assert!(slots.contains(&(emptied, None)), "{slots:?}");
    }
    for filled in [36, 37, 38] {
        assert!(slots.iter().any(|(slot, _)| *slot == filled), "{slots:?}");
    }
}

#[test]
fn a_close_of_window_0_with_a_full_inventory_throws_the_grid() {
    let full: Vec<(usize, Option<ItemStack>)> = (0..36)
        .map(|slot| (slot, stack(items::STONE, 64)))
        .collect();
    let mut region = holding(&full);
    region.players.get_mut(&player(1)).expect("in").crafting = Crafting {
        grid: [stack(items::OAK_LOG, 2), None, None, None],
        ..Crafting::default()
    };
    let output = region.tick(&input(5, PlayerInput::CloseContainer { serial: 0 }));
    assert!(
        region
            .player_state(player(1))
            .expect("in")
            .crafting
            .is_empty()
    );
    let thrown = output
        .events
        .iter()
        .filter(|event| matches!(event, RegionEvent::EntitySpawned(_)))
        .count();
    assert_eq!(thrown, 1, "{:?}", output.events);
}

#[test]
fn a_close_with_a_chest_open_closes_the_chest_and_keeps_the_grid() {
    let mut region = holding(&[]);
    region.players.get_mut(&player(1)).expect("in").crafting = Crafting {
        grid: [stack(items::OAK_LOG, 1), None, None, None],
        ..Crafting::default()
    };
    region.tick(&input(
        4,
        PlayerInput::UseItemOn {
            position: CHEST,
            face: Face::Top,
            cursor: [0.5; 3],
            sneaking: false,
            sequence: 1,
        },
    ));
    // The edge names a close by what it shows, the chest; one of window 0 that crossed
    // the chest's opening is passed over.
    region.tick(&input(5, PlayerInput::CloseContainer { serial: 0 }));
    let state = region.player_state(player(1)).expect("in");
    assert!(state.menu.is_some());
    assert_eq!(state.crafting.grid[0], stack(items::OAK_LOG, 1));
    region.tick(&input(6, PlayerInput::CloseContainer { serial: 4 }));
    let state = region.player_state(player(1)).expect("in");
    assert!(state.menu.is_none());
    assert_eq!(state.crafting.grid[0], stack(items::OAK_LOG, 1));
}

#[test]
fn the_grid_is_in_the_place_the_store_keeps_but_a_drag_is_not() {
    let mut region = holding(&[]);
    let own = region.players.get_mut(&player(1)).expect("in");
    own.crafting = Crafting {
        grid: [None, stack(items::OAK_LOG, 1), None, None],
        carried: stack(items::DIRT, 1),
        drag: Some(crate::state::Drag {
            kind: crate::state::DragKind::Split,
            slots: vec![9],
        }),
        ..Crafting::default()
    };
    let place = own.place(region.tick, region.config.dimension);
    assert_eq!(place.crafting.grid[1], stack(items::OAK_LOG, 1));
    assert_eq!(place.crafting.carried, stack(items::DIRT, 1));
    assert_eq!(place.crafting.drag, None);
    let transfer = region
        .players
        .remove(&player(1))
        .expect("in")
        .into_transfer(0, Dimension::Overworld);
    assert!(
        transfer.crafting.drag.is_some(),
        "a hand-over keeps the drag"
    );
}

#[test]
fn a_dead_players_click_on_window_0_is_answered_with_window_0_whole() {
    let mut region = holding(&[(0, stack(items::STONE, 1))]);
    region.players.get_mut(&player(1)).expect("in").dead = true;
    let output = region.tick(&click_window_0(5, 36, ClickKind::Pickup));
    let events = window_0(&output);
    let [
        PlayerEvent::MenuOpened {
            serial: 0,
            kind: MenuKind::Inventory,
            slots,
            ..
        },
    ] = &events[..]
    else {
        panic!("{events:?}");
    };
    assert_eq!(slots.len(), 46);
    assert_eq!(slots[36], stack(items::STONE, 1));
}
