//! Tests of a player's menus in the region (step N6b, `docs/groundwork/n6-plan.md`,
//! decision 2 and sections 6 to 10): a click on a container opens its menu, named by the
//! number of the input that clicked; clicks are run on the container and the inventory
//! and answered every time; a close puts the carried stack back and the container stops
//! counting the player; and a restored region reports the menu whole once its chest is
//! loaded.
//!
//! The regions are those of the tests of R3a (`redstone.rs`): player 1 at the spawn,
//! (0.5, -60, 0.5), with the chunks around chunk (0, 0) loaded.

use clustine_data::items;

use super::redstone::{state, ticking_with};
use super::*;
use crate::api::{ClickKind, MenuKind};

const CHEST: BlockPos = BlockPos::new(3, -60, 3);

fn chest() -> BlockState {
    state("minecraft:chest[facing=north,type=single,waterlogged=false]")
}

fn stack(item: i32, count: i32) -> Option<ItemStack> {
    Some(ItemStack::new(item, count))
}

/// Player 1's input `input` numbered `number`.
fn input(number: u64, input: PlayerInput) -> TickInputs {
    moves(vec![with_number(number, numbered(1, input))])
}

fn use_chest(number: u64) -> TickInputs {
    input(
        number,
        PlayerInput::UseItemOn {
            position: CHEST,
            face: Face::Top,
            cursor: [0.5; 3],
            sneaking: false,
            sequence: 1,
        },
    )
}

fn click(number: u64, serial: u64, slot: i16) -> TickInputs {
    input(
        number,
        PlayerInput::ContainerClick {
            serial,
            slot,
            button: 0,
            kind: ClickKind::Pickup,
        },
    )
}

/// The menu events player 1 was told of in `output`.
fn menu_events(output: &TickOutput) -> Vec<PlayerEvent> {
    output
        .player_events
        .iter()
        .filter(|(id, _)| *id == player(1))
        .map(|(_, event)| event.clone())
        .filter(|event| {
            matches!(
                event,
                PlayerEvent::MenuOpened { .. }
                    | PlayerEvent::MenuChanged { .. }
                    | PlayerEvent::MenuClosed { .. }
                    | PlayerEvent::NoMenu { .. }
            )
        })
        .collect()
}

/// A region with a chest holding 64 stone in its first slot.
fn with_a_chest() -> Region {
    let mut region = ticking_with(&[(CHEST, chest())]);
    let mut container = region.container_at(CHEST).expect("a chest");
    container.slots[0] = stack(items::STONE, 64);
    region.set_container(CHEST, container, &mut TickOutput::default());
    region
}

#[test]
fn a_click_on_a_chest_opens_its_menu_named_by_the_number_of_the_input() {
    let mut region = with_a_chest();
    let output = region.tick(&use_chest(41));
    let events = menu_events(&output);
    let [
        PlayerEvent::MenuOpened {
            serial,
            applied,
            kind,
            title,
            owner: None,
            slots,
            carried,
        },
    ] = &events[..]
    else {
        panic!("{events:?}");
    };
    assert_eq!((*serial, *applied, *kind), (41, 41, MenuKind::Rows3));
    assert_eq!(title, "container.chest");
    assert_eq!(slots.len(), 63);
    assert_eq!(slots[0], stack(items::STONE, 64));
    assert_eq!(*carried, None);
    assert_eq!(region.openers(CHEST), 1);
    // Nothing changed since, so the next tick says nothing.
    assert_eq!(menu_events(&idle(&mut region)), []);
}

#[test]
fn a_stack_taken_from_the_chest_and_put_in_the_inventory_is_answered_click_by_click() {
    let mut region = with_a_chest();
    region.tick(&use_chest(41));
    let taken = region.tick(&click(42, 41, 0));
    assert_eq!(
        menu_events(&taken),
        [PlayerEvent::MenuChanged {
            serial: 41,
            applied: 42,
            slots: vec![(0, None)],
            carried: Some(stack(items::STONE, 64)),
        }]
    );
    // The main inventory's slot 22 comes after the chest's 27, at 27 + 13.
    let before = region.player_state(player(1)).expect("joined");
    assert_eq!(before.inventory.0[22], None);
    let put = region.tick(&click(43, 41, 40));
    assert_eq!(
        menu_events(&put),
        [PlayerEvent::MenuChanged {
            serial: 41,
            applied: 43,
            slots: vec![(40, stack(items::STONE, 64))],
            carried: Some(None),
        }]
    );
    assert_eq!(region.container_at(CHEST).expect("a chest").slots[0], None);
    let state = region.player_state(player(1)).expect("joined");
    assert_eq!(state.inventory.0[22], stack(items::STONE, 64));
}

#[test]
fn a_click_that_changes_nothing_is_answered_all_the_same() {
    let mut region = with_a_chest();
    region.tick(&use_chest(41));
    // An empty slot clicked with nothing carried.
    let output = region.tick(&click(42, 41, 5));
    assert_eq!(
        menu_events(&output),
        [PlayerEvent::MenuChanged {
            serial: 41,
            applied: 42,
            slots: Vec::new(),
            carried: None,
        }]
    );
}

#[test]
fn a_click_of_another_menu_is_answered_with_the_open_one_whole_or_its_close() {
    let mut region = with_a_chest();
    let stale = region.tick(&click(5, 3, 0));
    assert_eq!(
        menu_events(&stale),
        [PlayerEvent::MenuClosed {
            serial: 3,
            applied: 5
        }],
        "no menu is open"
    );
    region.tick(&use_chest(41));
    let other = region.tick(&click(42, 40, 0));
    assert!(
        matches!(
            &menu_events(&other)[..],
            [PlayerEvent::MenuOpened { serial: 41, .. }]
        ),
        "{:?}",
        menu_events(&other)
    );
    assert_eq!(
        region.container_at(CHEST).expect("a chest").slots[0],
        stack(items::STONE, 64),
        "the click ran on nothing"
    );
}

#[test]
fn a_close_puts_the_carried_stack_back_and_the_chest_counts_nobody() {
    let mut region = with_a_chest();
    region.tick(&use_chest(41));
    region.tick(&click(42, 41, 0));
    let closed = region.tick(&input(43, PlayerInput::CloseContainer { serial: 41 }));
    let events: Vec<PlayerEvent> = closed
        .player_events
        .iter()
        .filter(|(id, _)| *id == player(1))
        .map(|(_, event)| event.clone())
        .collect();
    assert!(
        matches!(
            &events[..],
            [
                PlayerEvent::MenuClosed {
                    serial: 41,
                    applied: 43
                },
                PlayerEvent::InventorySlot { slot: 0, .. },
                ..
            ]
        ),
        "{events:?}"
    );
    assert_eq!(region.openers(CHEST), 0);
    let state = region.player_state(player(1)).expect("joined");
    assert_eq!(state.inventory.0[0], stack(items::STONE, 64));
    assert_eq!(state.menu, None);
}

#[test]
fn a_resync_reports_the_menu_whole_or_the_inventory_where_none_is_open() {
    let mut region = with_a_chest();
    let none = region.tick(&input(5, PlayerInput::MenuResync));
    assert!(matches!(
        &menu_events(&none)[..],
        [PlayerEvent::NoMenu { applied: 5, .. }]
    ));
    region.tick(&use_chest(41));
    let whole = region.tick(&input(42, PlayerInput::MenuResync));
    assert!(matches!(
        &menu_events(&whole)[..],
        [PlayerEvent::MenuOpened {
            serial: 41,
            applied: 42,
            ..
        }]
    ));
}

#[test]
fn a_restored_region_reports_the_menu_whole_once_its_chest_is_loaded() {
    let mut region = with_a_chest();
    region.tick(&use_chest(41));
    idle(&mut region);
    let around: Vec<ChunkPos> = (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect();
    let chunks: Vec<(ChunkPos, Chunk)> = around
        .iter()
        .map(|position| (*position, region.saved_chunk(*position).expect("loaded")))
        .collect();
    let mut restored = Region::restore(
        region.config.clone(),
        region.state(),
        Holdings {
            pinned: vec![ChunkArea::EVERYWHERE],
            held: around.clone(),
            lent: Vec::new(),
            lent_away: Vec::new(),
        },
    );
    // Held and not loaded yet: the menu waits, neither reported nor closed.
    assert_eq!(menu_events(&idle(&mut restored)), []);
    assert!(
        restored
            .player_state(player(1))
            .expect("restored")
            .menu
            .is_some()
    );
    let loaded = restored.tick(&TickInputs {
        chunks_loaded: chunks,
        ..TickInputs::default()
    });
    let events = menu_events(&loaded);
    assert!(
        matches!(
            &events[..],
            [PlayerEvent::MenuOpened { serial: 41, slots, .. }] if slots[0] == stack(items::STONE, 64)
        ),
        "{events:?}"
    );
    assert_eq!(restored.openers(CHEST), 1);
}

/// A player who walks into another region's land with a menu open has it closed in that
/// tick, before they are let go: the carried stack goes back into the inventory their
/// transfer carries, and no menu goes with them (section 6 item 2, section 8 item 7).
#[test]
fn a_hand_over_closes_the_menu_and_the_carried_stack_travels_in_the_inventory() {
    let mut region = region_in(EAST);
    region.tick(&changes(vec![arrive(5, &transfer(5, 40, 2.5, 0))]));
    let arrived = region.players.get_mut(&player(5)).expect("arrived");
    arrived.menu = Some(crate::state::Menu {
        serial: 1,
        of: crate::state::MenuOf::Ender {
            at: BlockPos::new(2, -60, 0),
        },
        kind: MenuKind::Rows3,
        carried: stack(items::DIAMOND, 3),
        drag: None,
        trade: None,
    });
    let step = as_entity(40, walk(5, -0.5, 0.5));
    let output = region.tick(&moves(vec![with_number(1, step)]));
    let closed = output.player_events.iter().any(|(id, event)| {
        *id == player(5) && matches!(event, PlayerEvent::MenuClosed { serial: 1, .. })
    });
    assert!(closed, "{:?}", output.player_events);
    let carried: Vec<_> = output
        .durable
        .iter()
        .filter_map(|(_, _, entry)| match entry {
            Durable::Departed { transfer, .. } => Some(
                transfer
                    .inventory
                    .0
                    .iter()
                    .flatten()
                    .filter(|stack| stack.item == items::DIAMOND)
                    .map(|stack| stack.count)
                    .sum::<i32>(),
            ),
            _ => None,
        })
        .collect();
    assert_eq!(carried, [3]);
}
