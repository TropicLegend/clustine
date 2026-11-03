//! A survival player's dig of a block across a region line (step V2c), written from the
//! specification by someone who did not write the code: one player's break looks the
//! same to them whichever side of a line the block is on (n5
//! `ServerPlayerGameMode.destroyBlock`, `Item.mineBlock`, `Block.playerDestroy`). A
//! survival bot with an iron pickaxe digs stone on the other side of the line by the
//! spawn: it sees the block go, its pickaxe's damage go to 1 and the cobblestone the
//! stone drops. And a client that sends a negative sequence number is disconnected, as
//! the game's `ackBlockChangesUpTo` throws at one.
//!
//! The dig across the line needs the pinned layout (`CLUSTINE_TEST_PINS=0,4`, whose line
//! at block x = 0 runs beside the spawn); in a world whose regions follow their players
//! it returns at once.

mod common;

use std::time::Duration;

use clustine_botswarm::{Bot, Heard};
use clustine_data::{blocks, entity_types, items};
use clustine_protocol::codec::Position;
use clustine_protocol::components::WireComponent;
use clustine_protocol::item::ItemStack;
use clustine_protocol::packets::play::{
    ClientboundPlay, EntityDataValue, PlayerAction, face, game_event, player_action,
};
use clustine_sim::api::Order;
use clustine_world::GameMode;

use common::{VIEW_DISTANCE, start, view_area};

const PATIENCE: Duration = Duration::from_secs(60);

/// The stone the digger places and digs: on the grass, one block west of the line at
/// block x = 0, in a chunk of the region west of the spawn's.
const ACROSS: (i32, i32, i32) = (-1, -60, 0);

const STONE: Option<i32> = Some(blocks::STONE.0 as i32);
const AIR: Option<i32> = Some(0);

/// Survival's id, as `GameEvent` carries it.
const SURVIVAL: f32 = 0.0;

fn pinned() -> bool {
    !std::env::var("CLUSTINE_TEST_PINS")
        .unwrap_or_default()
        .is_empty()
}

/// Joins and waits for the chunks around the spawn point.
async fn join(address: &str, name: &str) -> Bot {
    let mut bot = Bot::join(address, name).await.unwrap();
    let count = view_area((0, 0), VIEW_DISTANCE).len();
    bot.wait_for_chunks(count, PATIENCE).await.unwrap();
    bot
}

/// Waits until `bot` knows the block at `position` to be `state`.
async fn sees_block(bot: &mut Bot, position: (i32, i32, i32), state: Option<i32>) {
    let (x, y, z) = position;
    bot.wait_until(PATIENCE, |bot| bot.block_at(x, y, z).unwrap() == state)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error}: the block at {position:?} is {:?}, not {state:?}",
                bot.block_at(x, y, z)
            )
        });
}

/// The stages of cracks at `at` that `bot` has heard, in order.
fn stages(bot: &Bot, at: (i32, i32, i32)) -> Vec<u8> {
    bot.heard
        .iter()
        .flatten()
        .filter_map(|(_, heard)| match heard {
            Heard::BlockDestruction {
                at: there, stage, ..
            } if *there == at => Some(*stage),
            _ => None,
        })
        .collect()
}

/// The hotbar slot that holds `item`, if one does.
fn slot_of(bot: &Bot, item: i32) -> Option<usize> {
    bot.hotbar
        .iter()
        .position(|stack| stack.as_ref().is_some_and(|stack| stack.item == item))
}

/// The damage of the stack in the hotbar slot `slot`, as the server last said.
fn damage(bot: &Bot, slot: usize) -> Option<i32> {
    let stack = bot.hotbar[slot].as_ref()?;
    Some(
        stack
            .components
            .added
            .iter()
            .find_map(|component| match component {
                WireComponent::Damage(damage) => Some(*damage),
                _ => None,
            })
            .unwrap_or(0),
    )
}

/// Whether the server has shown `bot` an item entity of `item` (by its data's stack),
/// or the bot holds one in its hotbar, having picked it up.
fn shown_or_holds(bot: &Bot, item: i32) -> bool {
    let packets = bot.packets.as_deref().unwrap_or_default();
    let mut items_shown = std::collections::BTreeSet::new();
    let mut shown = false;
    for (_, packet) in packets {
        match packet {
            ClientboundPlay::SpawnEntity(spawn) if spawn.kind == entity_types::ITEM => {
                items_shown.insert(spawn.entity_id);
            }
            ClientboundPlay::SetEntityData(data) if items_shown.contains(&data.entity_id) => {
                shown |= data.values.iter().any(|(index, value)| {
                    *index == 8
                        && matches!(value, EntityDataValue::ItemStack(Some(stack)) if stack.item == item)
                });
            }
            _ => {}
        }
    }
    shown || slot_of(bot, item).is_some()
}

/// Has `digger`, a creative bot by the spawn, place stone at [`ACROSS`], then become a
/// survival player by the operator's order and be given an iron pickaxe, which it holds
/// when this returns: its hotbar slot.
async fn survival_digger_by_stone_across(server: &clustine::Server, digger: &mut Bot) -> usize {
    // Stone across the line, placed while the digger is still creative.
    digger
        .put_in_hotbar(0, ItemStack::new(items::STONE, 1))
        .await
        .unwrap();
    digger.select_slot(0).await.unwrap();
    let (x, y, z) = ACROSS;
    digger.use_item_on(x, y - 1, z, face::TOP).await.unwrap();
    sees_block(digger, ACROSS, STONE).await;
    // Room in the hotbar for what is given (`Inventory.getFreeSlot` looks there first),
    // kept by the region before the orders, which come another way.
    for slot in 1..9 {
        digger.empty_hotbar_slot(slot).await.unwrap();
    }
    digger.stand_until_kept(PATIENCE).await.unwrap();

    // Survival, by the operator's order, and an iron pickaxe given.
    digger.record_packets();
    server
        .order("Digger", Order::GameMode(GameMode::Survival))
        .await
        .unwrap();
    digger
        .wait_until(PATIENCE, |bot| {
            bot.packets.iter().flatten().any(|(_, packet)| {
                matches!(packet, ClientboundPlay::GameEvent(event)
                    if event.event == game_event::CHANGE_GAME_MODE && event.value == SURVIVAL)
            })
        })
        .await
        .unwrap();
    server
        .order(
            "Digger",
            Order::Give {
                item: items::IRON_PICKAXE,
                count: 1,
            },
        )
        .await
        .unwrap();
    digger
        .wait_until(PATIENCE, |bot| slot_of(bot, items::IRON_PICKAXE).is_some())
        .await
        .unwrap();
    let slot = slot_of(digger, items::IRON_PICKAXE).expect("the pickaxe was given");
    assert_eq!(damage(digger, slot), Some(0));
    digger.select_slot(slot as i16).await.unwrap();
    slot
}

#[tokio::test]
async fn a_survival_player_digs_stone_across_the_line_and_gets_its_drop_and_its_wear() {
    if !pinned() {
        // Only a pinned line is sure to run beside the spawn.
        return;
    }
    let (server, address) = start().await;
    let mut watcher = join(&address, "Watcher").await;
    let mut digger = join(&address, "Digger").await;
    let slot = survival_digger_by_stone_across(&server, &mut digger).await;

    // The dig: START, then STOP once the dig has gone on long enough. An iron pickaxe
    // needs five ticks for stone; the server tells its time every 20 ticks, and three
    // tellings after the START are 40 ticks at least.
    let started = digger
        .destroy_step(player_action::START_DESTROY_BLOCK, ACROSS, face::TOP)
        .await
        .unwrap();
    let told = digger.times.len();
    digger
        .wait_until(PATIENCE, |bot| bot.times.len() >= told + 3)
        .await
        .unwrap();
    let stopped = digger
        .destroy_step(player_action::STOP_DESTROY_BLOCK, ACROSS, face::TOP)
        .await
        .unwrap();
    assert!(started < stopped);
    let (x, y, z) = ACROSS;
    digger
        .wait_until(PATIENCE, |bot| {
            bot.acknowledged_sequence >= stopped && bot.block_at(x, y, z).unwrap() == AIR
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error}: acknowledged {}, the block {:?}",
                digger.acknowledged_sequence,
                digger.block_at(x, y, z)
            )
        });
    // Another player sees it gone too.
    sees_block(&mut watcher, ACROSS, AIR).await;

    // The pickaxe wore by one, and the stone dropped cobblestone.
    digger
        .wait_until(PATIENCE, |bot| damage(bot, slot) == Some(1))
        .await
        .unwrap_or_else(|error| panic!("{error}: the pickaxe is {:?}", digger.hotbar[slot]));
    digger
        .wait_until(PATIENCE, |bot| shown_or_holds(bot, items::COBBLESTONE))
        .await
        .unwrap_or_else(|error| panic!("{error}: no cobblestone: {}", digger.report()));
    // Once worn, and not again.
    assert_eq!(damage(&digger, slot), Some(1));
    server.stop().await;
}

/// `ServerLevel.destroyBlockProgress` sends the cracks of a dig to every player within
/// 32 blocks but the breaker, whichever side of a line the block is on.
#[tokio::test]
async fn another_player_sees_the_cracks_of_a_dig_across_the_line() {
    if !pinned() {
        return;
    }
    let (server, address) = start().await;
    let mut watcher = join(&address, "Watcher").await;
    let mut digger = join(&address, "Digger").await;
    survival_digger_by_stone_across(&server, &mut digger).await;
    watcher.record_heard();
    digger
        .destroy_step(player_action::START_DESTROY_BLOCK, ACROSS, face::TOP)
        .await
        .unwrap();
    watcher
        .wait_until(PATIENCE, |bot| {
            stages(bot, ACROSS)
                .iter()
                .any(|stage| (7..=10).contains(stage))
        })
        .await
        .unwrap_or_else(|error| panic!("{error}: heard {:?}", stages(&watcher, ACROSS)));
    server.stop().await;
}

#[tokio::test]
async fn a_client_that_sends_a_negative_sequence_number_is_disconnected() {
    let (server, address) = start().await;
    let mut bot = Bot::join(&address, "Negative").await.unwrap();
    bot.wait_until(PATIENCE, Bot::is_loaded).await.unwrap();
    let mut connection = bot.into_connection();
    connection
        .write(&PlayerAction {
            status: player_action::START_DESTROY_BLOCK,
            position: Position { x: 2, y: -62, z: 1 },
            face: face::TOP,
            sequence: -1,
        })
        .await
        .unwrap();
    // The server reads on until it ends the connection, perhaps with a disconnect
    // packet first.
    let ended = tokio::time::timeout(PATIENCE, async {
        loop {
            if let Err(error) = connection.read_frame().await {
                return error;
            }
        }
    })
    .await
    .expect("the server ended the connection");
    let reason = format!("{ended:#}");
    assert!(
        !reason.contains("sent nothing for too long"),
        "the server kept the connection: {reason}"
    );
    server.stop().await;
}
