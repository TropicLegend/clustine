//! Tests of how the fan-out routes a player's menus (`docs/groundwork/n6-plan.md`,
//! sections 7 to 9): a menu is shown only as the player's own region says, a click goes
//! to that region by the menu's serial, a hand-over closes the window, and a resumed
//! link or a moved stay has the region asked for the menu again. What the window shows
//! after each click is `crate::menus`'s, tested there.

use super::*;
use clustine_protocol::packets::play::{ContainerClick, click};
use clustine_sim::api::{ClickKind, MenuKind};

fn opened(serial: u64) -> PlayerEvent {
    opened_as_of(serial, serial)
}

fn opened_as_of(serial: u64, applied: u64) -> PlayerEvent {
    let mut slots = vec![None; MenuKind::Rows3.slots()];
    slots[0] = Some(ItemStack { item: 1, count: 64 });
    PlayerEvent::MenuOpened {
        serial,
        applied,
        kind: MenuKind::Rows3,
        title: "container.chest".to_owned(),
        slots,
        carried: None,
    }
}

fn to(player: PlayerId, event: PlayerEvent) -> WorkerToEdge {
    WorkerToEdge::ToPlayer { player, event }
}

fn inputs(said: &[EdgeMessage]) -> Vec<PlayerInput> {
    said.iter()
        .filter_map(|message| match &message.body {
            EdgeToWorker::Input { input, .. } => Some(input.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_menu_the_players_region_opens_is_shown_and_its_clicks_go_there_by_its_serial() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    edge.tell(WEST, to(player(1), opened(3)));
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    assert!(
        matches!(
            &client.menus[..],
            [
                ClientboundPlay::OpenScreen(_),
                ClientboundPlay::SetContainerContent(_)
            ]
        ),
        "{:?}",
        client.menus
    );
    let click = ContainerClick {
        window_id: 1,
        state_id: 1,
        slot: 0,
        button: 0,
        input: click::PICKUP,
        changed: vec![(0, None)],
        carried: None,
    };
    edge.menu(&client, MenuCommand::Click(click)).await;
    let (_, body) = edge.next_numbered(WEST).await;
    assert!(
        matches!(
            body,
            EdgeToWorker::Input {
                input: PlayerInput::ContainerClick {
                    serial: 3,
                    slot: 0,
                    kind: ClickKind::Pickup,
                    ..
                },
                ..
            }
        ),
        "{body:?}"
    );
    edge.end().await;
}

#[tokio::test]
async fn a_menu_another_region_speaks_of_is_not_shown() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    edge.tell(EAST, to(player(1), opened(3)));
    edge.settle(EAST).await;
    edge.sync(&mut client).await;
    assert_eq!(client.menus, []);
    // Nor is a click on the window that was never shown passed on.
    let click = ContainerClick {
        window_id: 1,
        state_id: 0,
        slot: 0,
        button: 0,
        input: click::PICKUP,
        changed: Vec::new(),
        carried: None,
    };
    edge.menu(&client, MenuCommand::Click(click)).await;
    edge.quiet().await;
    edge.end().await;
}

#[tokio::test]
async fn a_hand_over_closes_the_window_the_client_shows() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    edge.tell(WEST, to(player(1), opened(3)));
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    edge.hand(player(1), EntityId(5), WEST, EAST, EASTERN).await;
    edge.sync(&mut client).await;
    assert!(
        matches!(
            client.menus.last(),
            Some(ClientboundPlay::ClientboundContainerClose(close)) if close.window_id == 1
        ),
        "{:?}",
        client.menus
    );
    edge.end().await;
}

#[tokio::test]
async fn a_close_by_the_client_goes_to_the_region_and_a_late_report_opens_nothing() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    edge.tell(WEST, to(player(1), opened(3)));
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    edge.menu(&client, MenuCommand::Close { window: 1 }).await;
    let (number, body) = edge.next_numbered(WEST).await;
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
    // The region reports the menu whole as of before the close.
    let shown = client.menus.len();
    edge.tell(WEST, to(player(1), opened_as_of(3, number - 1)));
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    assert_eq!(client.menus.len(), shown, "{:?}", &client.menus[shown..]);
    edge.end().await;
}

#[tokio::test]
async fn a_resumed_link_has_the_players_region_asked_for_the_menu_their_client_shows() {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    edge.tell(WEST, to(player(1), opened(3)));
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    edge.relink(WEST).await;
    edge.resume(WEST);
    edge.tell(WEST, present(player(1), EntityId(5)));
    let said = edge.said(WEST).await;
    assert!(inputs(&said).contains(&PlayerInput::MenuResync), "{said:?}");
    edge.end().await;
}
