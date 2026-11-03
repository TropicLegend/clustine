//! Tests of how the fan-out serves window 0, the player's own inventory screen (V3a),
//! written from the game's classes by someone who did not write the code: a click on
//! it goes to the player's region as the serial 0 while no other window is shown
//! (`ServerGamePacketListenerImpl.handleContainerClick` @19-36), the answer sends only
//! what the client's hashes do not show (`AbstractContainerMenu.broadcastChanges`,
//! `RemoteSlot.matches`), a click with a stale state id gets the window whole
//! (`handleContainerClick` @180-193, `broadcastFullState`), a chest's close shows window
//! 0 again with only the slots that differ (`doCloseContainer` → `transferState`, the
//! next tick's `broadcastChanges`), a join sends it whole with the state id 1 and nothing
//! carried (`initInventoryMenu`), and a close with no other window shown is window 0's
//! (`handleContainerClose` reads no window id).

use super::*;
use clustine_protocol::item::ItemStack as WireStack;
use clustine_protocol::packets::play::{ContainerClick, HashedStack, click};
use clustine_sim::api::{ClickKind, MenuKind};

/// The stone of these tests' inventories.
const STONE: i32 = 1;
/// An iron helmet's id, as far as these tests care: an item the client shows.
const HELMET: i32 = 900;

fn stone(count: i32) -> ItemStack {
    ItemStack::new(STONE, count)
}

/// What a vanilla client says it shows of a stack after a click: the item and the
/// count of a stack with no components.
fn hashed(item: i32, count: i32) -> Option<HashedStack> {
    Some(HashedStack {
        item,
        count,
        added: Vec::new(),
        removed: Vec::new(),
    })
}

fn to(player: PlayerId, event: PlayerEvent) -> WorkerToEdge {
    WorkerToEdge::ToPlayer { player, event }
}

/// A change of window 0 as the region reports it.
fn window_0_changed(
    applied: u64,
    slots: Vec<(u16, Option<ItemStack>)>,
    carried: Option<Option<ItemStack>>,
) -> PlayerEvent {
    PlayerEvent::MenuChanged {
        serial: 0,
        applied,
        slots,
        carried,
    }
}

/// A chest opened by the input `serial`, showing `hotbar` in the hotbar's first slot.
fn chest_opened(serial: u64, hotbar: Option<ItemStack>) -> PlayerEvent {
    let mut slots = vec![None; MenuKind::Rows3.slots()];
    slots[54] = hotbar;
    PlayerEvent::MenuOpened {
        serial,
        applied: serial,
        kind: MenuKind::Rows3,
        title: "container.chest".to_owned(),
        owner: None,
        slots,
        carried: None,
    }
}

/// Player 1 joins and the western region places them as entity 5 holding `inventory`.
async fn joined_holding(edge: &mut Harness, inventory: Inventory) -> Client {
    let mut client = edge.join(player(1)).await;
    let (_, join) = edge.next_numbered(WEST).await;
    assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
    let attempt = edge.attempts.get(&player(1)).copied().expect("joined");
    edge.placed.borrow_mut().insert(player(1), EntityId(5));
    edge.tell(
        WEST,
        to(
            player(1),
            PlayerEvent::Spawned {
                last_death: None,
                mode: Default::default(),
                riding: None,
                attempt,
                entity_id: EntityId(5),
                pose: Pose::at(SPAWN),
                flying: false,
                inventory,
                selected_slot: 0,
            },
        ),
    );
    edge.settle(WEST).await;
    edge.quiet().await;
    edge.sync(&mut client).await;
    client
}

/// Player 1 with 64 stone in the hotbar's first slot, window 0's 36.
async fn holding_stone(edge: &mut Harness) -> Client {
    let mut inventory = Inventory::default();
    inventory.set(0, Some(stone(64)));
    joined_holding(edge, inventory).await
}

/// The client's click on window 0, with the state id `state_id`, saying it shows
/// `changed` and carries `carried` after it.
fn click_0(
    state_id: i32,
    slot: i16,
    changed: Vec<(i16, Option<HashedStack>)>,
    carried: Option<HashedStack>,
) -> MenuCommand {
    MenuCommand::Click(ContainerClick {
        window_id: 0,
        state_id,
        slot,
        button: 0,
        input: click::PICKUP,
        changed,
        carried,
    })
}

/// The packets of window 0 among the client's menus' from `from` on: its slots, and
/// the cursor's.
fn window_0_packets(client: &Client, from: usize) -> Vec<ClientboundPlay> {
    client.menus[from..]
        .iter()
        .filter(|packet| match packet {
            ClientboundPlay::ContainerSetSlot(slot) => slot.window_id == 0,
            ClientboundPlay::SetCursorItem(_) => true,
            _ => false,
        })
        .cloned()
        .collect()
}

fn wire_item(stack: &Option<WireStack>) -> Option<(i32, i32)> {
    stack.as_ref().map(|stack| (stack.item, stack.count))
}

/// `PlayerList.placeNewPlayer` → `initInventoryMenu`: a new connection's window 0 is
/// sent whole, its state id 1 (`incrementStateId` from 0), its grid and result empty,
/// nothing carried, the inventory where the screen shows it.
#[tokio::test]
async fn a_joins_window_0_is_sent_whole_with_the_state_id_1_and_nothing_carried() {
    let mut edge = Harness::start().await;
    let mut inventory = Inventory::default();
    inventory.set(0, Some(stone(64)));
    inventory.set(39, Some(ItemStack::new(HELMET, 1)));
    let client = joined_holding(&mut edge, inventory).await;
    assert_eq!(client.inventories.len(), 1, "{:?}", client.inventories);
    let whole = &client.inventories[0];
    assert_eq!((whole.window_id, whole.state_id), (0, 1));
    assert_eq!(whole.carried, None);
    assert_eq!(whole.slots.len(), 46);
    assert!(whole.slots[..5].iter().all(Option::is_none));
    assert_eq!(wire_item(&whole.slots[36]), Some((STONE, 64)));
    assert_eq!(wire_item(&whole.slots[5]), Some((HELMET, 1)));
    edge.end().await;
}

/// `handleContainerClick` @19-36: with no other menu open the player's menu is window 0,
/// and its click goes to the region as the serial 0, its slot, button and kind as the
/// client sent them.
#[tokio::test]
async fn a_click_on_window_0_goes_to_the_region_as_the_serial_0() {
    let mut edge = Harness::start().await;
    let client = holding_stone(&mut edge).await;
    edge.menu(&client, click_0(1, 36, vec![(36, None)], hashed(STONE, 64)))
        .await;
    let (_, body) = edge.next_numbered(WEST).await;
    assert!(
        matches!(
            body,
            EdgeToWorker::Input {
                input: PlayerInput::ContainerClick {
                    serial: 0,
                    slot: 36,
                    button: 0,
                    kind: ClickKind::Pickup,
                },
                ..
            }
        ),
        "{body:?}"
    );
    edge.end().await;
}

/// `broadcastChanges` after the click: the client said it shows slot 36 empty and the
/// stone carried, which is what the region answers; nothing is sent, neither the slot
/// nor the cursor, nor the window whole.
#[tokio::test]
async fn an_answer_the_clients_hashes_already_show_sends_nothing() {
    let mut edge = Harness::start().await;
    let mut client = holding_stone(&mut edge).await;
    let menus = client.menus.len();
    edge.menu(&client, click_0(1, 36, vec![(36, None)], hashed(STONE, 64)))
        .await;
    let (number, _) = edge.next_numbered(WEST).await;
    edge.tell(
        WEST,
        to(
            player(1),
            window_0_changed(number, vec![(36, None)], Some(Some(stone(64)))),
        ),
    );
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    assert_eq!(window_0_packets(&client, menus), []);
    assert_eq!(client.inventories.len(), 1, "only the join's whole");
    edge.end().await;
}

/// `broadcastChanges` sends what the client does not show: a client that guessed the
/// stone stayed in slot 36 and that it carries nothing is sent the slot emptied and the
/// cursor, each once.
#[tokio::test]
async fn an_answer_the_clients_hashes_do_not_show_sends_the_slot_and_the_cursor() {
    let mut edge = Harness::start().await;
    let mut client = holding_stone(&mut edge).await;
    let menus = client.menus.len();
    edge.menu(&client, click_0(1, 36, vec![(36, hashed(STONE, 64))], None))
        .await;
    let (number, _) = edge.next_numbered(WEST).await;
    edge.tell(
        WEST,
        to(
            player(1),
            window_0_changed(number, vec![(36, None)], Some(Some(stone(64)))),
        ),
    );
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    let sent = window_0_packets(&client, menus);
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(
        matches!(&sent[0], ClientboundPlay::ContainerSetSlot(slot)
            if slot.slot == 36 && slot.stack.is_none()),
        "{sent:?}"
    );
    assert!(
        matches!(&sent[1], ClientboundPlay::SetCursorItem(cursor)
            if wire_item(&cursor.stack) == Some((STONE, 64))),
        "{sent:?}"
    );
    assert_eq!(client.inventories.len(), 1, "no whole");
    edge.end().await;
}

/// `handleContainerClick` @180-193: a click whose state id is not the window's is
/// followed by the window whole (`broadcastFullState`), with the next state id, whatever
/// the client's hashes show.
#[tokio::test]
async fn a_click_with_a_stale_state_id_gets_window_0_whole() {
    let mut edge = Harness::start().await;
    let mut client = holding_stone(&mut edge).await;
    edge.menu(&client, click_0(0, 36, vec![(36, None)], hashed(STONE, 64)))
        .await;
    let (number, _) = edge.next_numbered(WEST).await;
    edge.tell(
        WEST,
        to(
            player(1),
            window_0_changed(number, vec![(36, None)], Some(Some(stone(64)))),
        ),
    );
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    assert_eq!(client.inventories.len(), 2, "{:?}", client.inventories);
    let whole = &client.inventories[1];
    assert_eq!(whole.window_id, 0);
    assert_eq!(whole.state_id, 2, "the next after the join's 1");
    assert_eq!(whole.slots[36], None);
    assert_eq!(wire_item(&whole.carried), Some((STONE, 64)));
    edge.end().await;
}

/// The region's report of window 0 that no click asked for (a stack given, armour put
/// on by a dispenser) is sent as the slots it changed, while no other window is shown.
#[tokio::test]
async fn a_change_of_window_0_the_region_reports_of_itself_is_sent_as_its_slot() {
    let mut edge = Harness::start().await;
    let mut client = holding_stone(&mut edge).await;
    let menus = client.menus.len();
    edge.tell(
        WEST,
        to(
            player(1),
            window_0_changed(0, vec![(5, Some(ItemStack::new(HELMET, 1)))], None),
        ),
    );
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    let sent = window_0_packets(&client, menus);
    assert!(
        matches!(&sent[..], [ClientboundPlay::ContainerSetSlot(slot)]
            if slot.window_id == 0 && slot.slot == 5
                && wire_item(&slot.stack) == Some((HELMET, 1))),
        "{sent:?}"
    );
    edge.end().await;
}

/// `handleContainerClick` @19-36: while a chest is shown the player's menu is the
/// chest's, and a click naming window 0 is passed over: the region hears of it nothing,
/// and the next thing it hears is the client's close of the chest.
#[tokio::test]
async fn a_click_on_window_0_while_a_chest_is_shown_is_not_passed_on() {
    let mut edge = Harness::start().await;
    let mut client = holding_stone(&mut edge).await;
    edge.tell(WEST, to(player(1), chest_opened(3, Some(stone(64)))));
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    edge.menu(&client, click_0(1, 36, vec![(36, None)], hashed(STONE, 64)))
        .await;
    edge.menu(&client, MenuCommand::Close).await;
    let (_, body) = edge.next_numbered(WEST).await;
    assert!(
        matches!(
            body,
            EdgeToWorker::Input {
                input: PlayerInput::CloseContainer { serial: 3 },
                ..
            }
        ),
        "the close of the chest, and no click before it: {body:?}"
    );
    edge.end().await;
}

/// While a chest is shown nothing of window 0 is sent, though the region reports it; when
/// the chest closes, window 0 is the client's again without a whole: the 36 slots both
/// show carry over what the client shows (`transferState`), and only a slot that
/// differs is sent (`broadcastChanges` of the next tick): here the helmet the chest
/// does not show, and not the hotbar slot the client emptied in the chest.
#[tokio::test]
async fn a_chests_close_sends_only_the_slots_of_window_0_that_differ() {
    let mut edge = Harness::start().await;
    let mut client = holding_stone(&mut edge).await;
    edge.tell(WEST, to(player(1), chest_opened(3, Some(stone(64)))));
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    // The client shift-clicks the stone from the hotbar into the chest, and says so.
    edge.menu(
        &client,
        MenuCommand::Click(ContainerClick {
            window_id: 1,
            state_id: 1,
            slot: 54,
            button: 0,
            input: click::QUICK_MOVE,
            changed: vec![(0, hashed(STONE, 64)), (54, None)],
            carried: None,
        }),
    )
    .await;
    let (number, _) = edge.next_numbered(WEST).await;
    // The region answers the chest's click and reports window 0, with a helmet put on
    // in the same tick.
    edge.tell(
        WEST,
        to(
            player(1),
            PlayerEvent::MenuChanged {
                serial: 3,
                applied: number,
                slots: vec![(0, Some(stone(64))), (54, None)],
                carried: None,
            },
        ),
    );
    edge.tell(
        WEST,
        to(
            player(1),
            window_0_changed(
                number,
                vec![(5, Some(ItemStack::new(HELMET, 1))), (36, None)],
                None,
            ),
        ),
    );
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    let shown = client.menus.len();
    assert_eq!(
        window_0_packets(&client, 0)
            .into_iter()
            .filter(|packet| matches!(packet, ClientboundPlay::ContainerSetSlot(_)))
            .count(),
        0,
        "nothing of window 0 while the chest is shown: {:?}",
        client.menus
    );
    edge.menu(&client, MenuCommand::Close).await;
    let (_, body) = edge.next_numbered(WEST).await;
    assert!(
        matches!(
            body,
            EdgeToWorker::Input {
                input: PlayerInput::CloseContainer { serial: 3 },
                ..
            }
        ),
        "{body:?}"
    );
    edge.sync(&mut client).await;
    let sent: Vec<&ClientboundPlay> = client.menus[shown..].iter().collect();
    assert!(
        matches!(&sent[..], [ClientboundPlay::ContainerSetSlot(slot)]
            if slot.window_id == 0 && slot.slot == 5
                && wire_item(&slot.stack) == Some((HELMET, 1))),
        "only the helmet: {sent:?}"
    );
    assert_eq!(client.inventories.len(), 1, "no whole");
    edge.end().await;
}

/// `handleContainerClose` reads no window id: with no other window shown it closes
/// window 0 (`InventoryMenu.removed`), and goes to the region as the serial 0. The
/// client is sent nothing for it at once; the region's report empties its grid.
#[tokio::test]
async fn a_close_with_no_window_shown_goes_to_the_region_as_the_serial_0() {
    let mut edge = Harness::start().await;
    let mut client = holding_stone(&mut edge).await;
    let menus = client.menus.len();
    edge.menu(&client, MenuCommand::Close).await;
    let (_, body) = edge.next_numbered(WEST).await;
    assert!(
        matches!(
            body,
            EdgeToWorker::Input {
                input: PlayerInput::CloseContainer { serial: 0 },
                ..
            }
        ),
        "{body:?}"
    );
    edge.sync(&mut client).await;
    assert_eq!(client.menus[menus..], [], "nothing is closed on the client");
    edge.end().await;
}
