//! Tests of the edge's side of menus, from the game's `ContainerSynchronizer` and
//! `handleContainerClick` (`docs/groundwork/n6-brief.md`, sections 3.15 to 3.18) and the
//! plan's sections 7 to 9.

use super::*;
use clustine_protocol::packets::play::ClientboundPlay;

fn stack(item: i32, count: i32) -> Option<ItemStack> {
    Some(ItemStack::new(item, count))
}

fn hashed(item: i32, count: i32) -> Option<HashedStack> {
    Some(HashedStack {
        item,
        count,
        added: Vec::new(),
        removed: Vec::new(),
    })
}

fn read(packets: &[Bytes]) -> Vec<ClientboundPlay> {
    packets
        .iter()
        .map(|bytes| ClientboundPlay::decode(bytes).expect("a packet the edge writes"))
        .collect()
}

/// A chest's 27 slots and the inventory's 36, the first slot holding a stack.
fn chest() -> Vec<Option<ItemStack>> {
    let mut slots = vec![None; MenuKind::Rows3.slots()];
    slots[0] = stack(1, 64);
    slots
}

fn opened(menus: &mut Menus, serial: u64) -> Vec<ClientboundPlay> {
    read(&menus.opened(
        serial,
        serial,
        MenuKind::Rows3,
        "container.chest",
        chest(),
        None,
    ))
}

fn click_at(
    window: i32,
    state_id: i32,
    slot: i16,
    changed: Vec<(i16, Option<HashedStack>)>,
    carried: Option<HashedStack>,
) -> ContainerClick {
    ContainerClick {
        window_id: window,
        state_id,
        slot,
        button: 0,
        input: click::PICKUP,
        changed,
        carried,
    }
}

#[test]
fn a_menu_opens_with_a_new_window_shown_whole() {
    let mut menus = Menus::default();
    let packets = opened(&mut menus, 7);
    let [
        ClientboundPlay::OpenScreen(screen),
        ClientboundPlay::SetContainerContent(content),
    ] = &packets[..]
    else {
        panic!("{packets:?}");
    };
    assert_eq!((screen.window_id, screen.menu), (1, menu::GENERIC_9X3));
    assert_eq!(
        Text::from_nbt(screen.title.clone()),
        Text::translatable("container.chest")
    );
    assert_eq!((content.window_id, content.state_id), (1, 1));
    assert_eq!(content.slots.len(), 63);
    assert_eq!(content.slots[0], Some(WireStack { item: 1, count: 64 }));
    assert_eq!(menus.serial(), Some(7));
}

#[test]
fn another_menu_opened_closes_the_window_shown_first() {
    let mut menus = Menus::default();
    opened(&mut menus, 7);
    let packets = opened(&mut menus, 9);
    assert!(matches!(
        &packets[..],
        [
            ClientboundPlay::ClientboundContainerClose(ClientboundContainerClose { window_id: 1 }),
            ClientboundPlay::OpenScreen(OpenScreen { window_id: 2, .. }),
            ClientboundPlay::SetContainerContent(_),
        ]
    ));
}

#[test]
fn the_window_id_runs_from_1_to_100_and_round_again() {
    let mut menus = Menus::default();
    for serial in 1..=100 {
        opened(&mut menus, serial);
    }
    let packets = opened(&mut menus, 101);
    assert!(matches!(
        packets.get(1),
        Some(ClientboundPlay::OpenScreen(OpenScreen { window_id: 1, .. }))
    ));
}

#[test]
fn a_click_the_client_guessed_right_is_answered_with_nothing() {
    let mut menus = Menus::default();
    opened(&mut menus, 7);
    // The client takes the stack in slot 0 and says so.
    let input = menus
        .click(click_at(1, 1, 0, vec![(0, None)], hashed(1, 64)), 8)
        .expect("a click of the window shown");
    assert_eq!(
        input,
        PlayerInput::ContainerClick {
            serial: 7,
            slot: 0,
            button: 0,
            kind: ClickKind::Pickup,
        }
    );
    let packets = menus.changed(7, 8, &[(0, None)], Some(stack(1, 64)));
    assert_eq!(read(&packets), []);
}

#[test]
fn a_click_that_changed_nothing_corrects_what_the_client_guessed() {
    // Two players at one chest: the other took the stack a tick before this one's click,
    // which the region passed over. Its empty answer still has the edge compare.
    let mut menus = Menus::default();
    opened(&mut menus, 7);
    menus.changed(7, 7, &[(0, None)], None);
    menus.click(click_at(1, 2, 0, vec![(0, None)], hashed(1, 64)), 8);
    let packets = read(&menus.changed(7, 8, &[], None));
    assert_eq!(
        packets,
        [ClientboundPlay::SetCursorItem(SetCursorItem {
            stack: None
        })]
    );
}

#[test]
fn a_slot_the_client_shows_otherwise_is_sent_with_the_next_state_id() {
    let mut menus = Menus::default();
    opened(&mut menus, 7);
    // The client says it put the stack into slot 5; the region says slot 5 is full of
    // something else.
    menus.click(click_at(1, 1, 5, vec![(5, hashed(1, 64))], None), 8);
    let packets = read(&menus.changed(7, 8, &[(5, stack(2, 3))], None));
    assert_eq!(
        packets,
        [ClientboundPlay::ContainerSetSlot(ContainerSetSlot {
            window_id: 1,
            state_id: 2,
            slot: 5,
            stack: Some(WireStack { item: 2, count: 3 }),
        })]
    );
}

#[test]
fn a_click_with_a_stale_state_id_has_the_window_sent_whole() {
    let mut menus = Menus::default();
    opened(&mut menus, 7);
    menus.click(click_at(1, 0, 0, vec![(0, None)], hashed(1, 64)), 8);
    let packets = read(&menus.changed(7, 8, &[(0, None)], Some(stack(1, 64))));
    assert!(matches!(
        &packets[..],
        [ClientboundPlay::SetContainerContent(SetContainerContent {
            state_id: 2,
            ..
        })]
    ));
}

#[test]
fn a_click_waits_for_the_answer_that_covers_it() {
    let mut menus = Menus::default();
    opened(&mut menus, 7);
    menus.click(click_at(1, 1, 0, vec![(0, None)], hashed(1, 64)), 9);
    // An answer up to input 8 is not about the click: its hashes stay held, and the
    // client is told nothing of what it guessed.
    assert_eq!(read(&menus.changed(7, 8, &[], None)), []);
    assert_eq!(
        read(&menus.changed(7, 9, &[(0, None)], Some(stack(1, 64)))),
        []
    );
}

#[test]
fn a_click_of_a_window_not_shown_is_refused() {
    let mut menus = Menus::default();
    assert_eq!(menus.click(click_at(1, 0, 0, Vec::new(), None), 3), None);
    opened(&mut menus, 7);
    assert_eq!(menus.click(click_at(2, 1, 0, Vec::new(), None), 8), None);
    assert_eq!(menus.close(2, 8), None);
}

#[test]
fn an_answer_of_another_menu_changes_nothing() {
    let mut menus = Menus::default();
    opened(&mut menus, 7);
    assert_eq!(read(&menus.changed(6, 9, &[(0, None)], None)), []);
    assert_eq!(read(&menus.closed(6, 9)), []);
    assert_eq!(menus.serial(), Some(7));
}

#[test]
fn a_menu_the_client_closed_is_not_opened_again_by_an_older_report() {
    let mut menus = Menus::default();
    opened(&mut menus, 7);
    assert_eq!(
        menus.close(1, 10),
        Some(PlayerInput::CloseContainer { serial: 7 })
    );
    // A region restored to before the close reports the menu whole.
    assert_eq!(
        read(&menus.opened(7, 9, MenuKind::Rows3, "container.chest", chest(), None)),
        []
    );
    assert_eq!(menus.serial(), None);
    // What the region reports after the close counts.
    assert_eq!(opened(&mut menus, 11).len(), 2);
}

#[test]
fn a_whole_report_of_the_menu_shown_is_sent_as_its_contents() {
    let mut menus = Menus::default();
    opened(&mut menus, 7);
    let packets = read(&menus.opened(7, 12, MenuKind::Rows3, "container.chest", chest(), None));
    assert!(matches!(
        &packets[..],
        [ClientboundPlay::SetContainerContent(SetContainerContent {
            window_id: 1,
            ..
        })]
    ));
}

#[test]
fn the_region_closing_the_menu_closes_the_window() {
    let mut menus = Menus::default();
    opened(&mut menus, 7);
    let packets = read(&menus.closed(7, 7));
    assert_eq!(
        packets,
        [ClientboundPlay::ClientboundContainerClose(
            ClientboundContainerClose { window_id: 1 }
        )]
    );
    assert_eq!(read(&menus.closed(7, 7)), [], "a close is said once");
}

#[test]
fn a_hand_over_closes_the_window_and_nothing_else() {
    let mut menus = Menus::default();
    assert_eq!(read(&menus.departed()), []);
    opened(&mut menus, 7);
    assert_eq!(read(&menus.departed()).len(), 1);
    assert_eq!(menus.serial(), None);
}

#[test]
fn an_inventory_slot_the_region_changed_is_the_menus_too() {
    let mut menus = Menus::default();
    opened(&mut menus, 7);
    // The hotbar's first slot is shown after the chest's 27 and the main inventory's 27.
    menus.inventory_slot(0, stack(3, 1));
    menus.click(click_at(1, 1, 54, vec![(54, hashed(3, 1))], None), 8);
    assert_eq!(read(&menus.changed(7, 8, &[], None)), []);
    assert_eq!(menu_slot_of(MenuKind::Hopper, 9), Some(5));
    assert_eq!(menu_slot_of(MenuKind::Rows6, 8), Some(89));
    assert_eq!(menu_slot_of(MenuKind::Rows3, 36), None);
}

#[test]
fn a_kind_of_click_the_game_does_not_have_is_a_pickup() {
    assert_eq!(click_kind(click::PICKUP_ALL), ClickKind::PickupAll);
    assert_eq!(click_kind(7), ClickKind::Pickup);
    assert_eq!(click_kind(-1), ClickKind::Pickup);
}

#[test]
fn a_horse_s_menu_opens_by_mount_screen_open_naming_the_horse_then_its_window_whole() {
    let mut menus = Menus::default();
    let kind = MenuKind::Mount {
        columns: 5,
        entity: clustine_world::EntityId(40),
    };
    let slots = vec![None; kind.slots()];
    let packets = read(&menus.opened_with(3, 3, kind, "container.chest", slots, None, Some(17)));
    assert!(matches!(
        &packets[..],
        [
            ClientboundPlay::MountScreenOpen(MountScreenOpen {
                window_id: 1,
                columns: 5,
                entity_id: 17,
            }),
            ClientboundPlay::SetContainerContent(SetContainerContent { window_id: 1, .. }),
        ]
    ));
    assert_eq!(kind.slots(), 2 + 15 + 36);
}
