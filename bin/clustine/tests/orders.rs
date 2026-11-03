//! An operator's orders seen by the players (V1, step three), written from the
//! specification by someone who did not write the code: the design
//! `v1-orders-design.md` with its review, and the game's `ServerPlayer.setGameMode`,
//! `ServerPlayerGameMode.changeGameModeForPlayer` and `GiveCommand.giveItem`.
//!
//! Two bots stand near each other in a creative world; the order `gamemode <first>
//! survival` reaches the first through the server's coordinator, as `clustine gamemode`
//! does. Their client is told what `setGameMode` sends, in the game's order (the review,
//! item 3): the abilities, the list's `UPDATE_GAME_MODE`, `GameEvent` 3 with the mode's
//! id, the abilities again; the second's list is told the first is in survival. Then
//! `give <first> stone 5` puts five stone in the first's inventory.

mod common;

use std::time::Duration;

use clustine_botswarm::Bot;
use clustine_data::items;
use clustine_protocol::packets::play::{ClientboundPlay, abilities, game_event, player_info};
use clustine_sim::api::Order;
use clustine_world::GameMode;
use uuid::Uuid;

use common::start;

const PATIENCE: Duration = Duration::from_secs(30);

/// Survival's id, as `GameEvent` and the player list carry it.
const SURVIVAL: i32 = 0;

/// What of the packets the bot recorded tells of a change of game mode, in order.
#[derive(Debug, Clone, PartialEq)]
enum Told {
    /// `ClientboundPlayerAbilities`, by its flags.
    Abilities(u8),
    /// A list entry's `UPDATE_GAME_MODE`: whose, and the mode's id.
    ListedMode(Uuid, i32),
    /// `ClientboundGameEvent` `CHANGE_GAME_MODE`, by its value.
    ModeEvent(f32),
}

fn told(bot: &Bot) -> Vec<Told> {
    let packets = bot.packets.as_deref().unwrap_or_default();
    packets
        .iter()
        .flat_map(|(_, packet)| match packet {
            ClientboundPlay::PlayerAbilities(packet) => vec![Told::Abilities(packet.flags)],
            ClientboundPlay::PlayerInfoUpdate(packet)
                if packet.actions & player_info::UPDATE_GAME_MODE != 0 =>
            {
                (packet.entries.iter())
                    .filter_map(|entry| Some(Told::ListedMode(entry.uuid, entry.game_mode?)))
                    .collect()
            }
            ClientboundPlay::GameEvent(packet) if packet.event == game_event::CHANGE_GAME_MODE => {
                vec![Told::ModeEvent(packet.value)]
            }
            _ => Vec::new(),
        })
        .collect()
}

/// The stone in the bot's hotbar, all stacks together.
fn stone_in_hotbar(bot: &Bot) -> i32 {
    (bot.hotbar.iter().flatten())
        .filter(|stack| stack.item == items::STONE)
        .map(|stack| stack.count)
        .sum()
}

#[tokio::test]
async fn an_operator_sets_a_creative_flyer_to_survival_and_gives_them_stone() {
    let (server, address) = start().await;
    let mut first = Bot::join(&address, "Ordered").await.unwrap();
    first.wait_until(PATIENCE, Bot::is_loaded).await.unwrap();
    let mut second = Bot::join(&address, "Onlooker").await.unwrap();
    second.wait_until(PATIENCE, Bot::is_loaded).await.unwrap();
    let first_uuid = first.info.profile.uuid;
    // Near each other: the second is shown the first.
    second
        .wait_until(PATIENCE, |bot| {
            bot.entities
                .values()
                .any(|entity| entity.uuid == first_uuid)
        })
        .await
        .unwrap();
    // The first flies, kept by their region before the order.
    first.set_flying(true).await.unwrap();
    first.stand_until_kept(PATIENCE).await.unwrap();
    let stone_before = stone_in_hotbar(&first);
    first.record_packets();
    second.record_packets();

    // Names are matched as the game's `getPlayerByName` does, ignoring case.
    let edges = server
        .order("ordered", Order::GameMode(GameMode::Survival))
        .await
        .unwrap();
    assert!(edges >= 1, "the order reached no edge");

    first
        .wait_until(PATIENCE, |bot| {
            told(bot).contains(&Told::ModeEvent(SURVIVAL as f32))
                && matches!(told(bot).last(), Some(Told::Abilities(_)))
        })
        .await
        .unwrap();
    let to_first = told(&first);
    let start = to_first
        .iter()
        .position(|told| matches!(told, Told::Abilities(_)))
        .expect("the abilities came first");
    assert_eq!(
        to_first[start..],
        [
            Told::Abilities(0),
            Told::ListedMode(first_uuid, SURVIVAL),
            Told::ModeEvent(SURVIVAL as f32),
            Told::Abilities(0),
        ],
        "setGameMode's packets, in the game's order: {to_first:?}"
    );
    let flags = first.abilities.last().expect("told").flags;
    assert_eq!(flags & (abilities::FLYING | abilities::INSTANT_BREAK), 0);
    assert!(!first.flying, "a survival player does not fly");

    second
        .wait_until(PATIENCE, |bot| {
            told(bot).contains(&Told::ListedMode(first_uuid, SURVIVAL))
        })
        .await
        .unwrap();
    assert!(
        !told(&second)
            .iter()
            .any(|told| matches!(told, Told::ModeEvent(_) | Told::Abilities(_))),
        "the onlooker's own mode is unchanged: {:?}",
        told(&second)
    );

    server
        .order(
            "Ordered",
            Order::Give {
                item: items::STONE,
                count: 5,
            },
        )
        .await
        .unwrap();
    first
        .wait_until(PATIENCE, |bot| stone_in_hotbar(bot) == stone_before + 5)
        .await
        .unwrap();
    server.stop().await;
}
