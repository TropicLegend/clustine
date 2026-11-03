//! Survival's damage, hunger and death with real clients (V4), written from
//! `docs/groundwork/v-plan.md`, sections 2.2 to 2.5, and the game's classes by someone who
//! did not write the code: survival bots fall from a pillar they built, sprint and jump
//! until they hunger, die in the void and respawn, and fight.
//!
//! What the tests rest on, in the game's classes:
//!
//! - `LivingEntity.calculateFallDamage @0-35`: `floor(distance + 1e-6 − 3)`; the server
//!   counts the distance from the client's moves (`ServerPlayer.doCheckFallDamage`), and
//!   tells the client its health (`ServerPlayer.doTick @146-264`, `SetHealth`).
//! - `ServerPlayer.jumpFromGround @19, @29`: a sprinting jump costs 0.2 of exhaustion, and
//!   `checkMovementStatistics` 0.1 a metre sprinting on the ground; `FoodData.tick @11-75`
//!   takes 4 of exhaustion off a point of saturation, then of food.
//! - `LivingEntity.onBelowWorld`: the void kills; `ServerPlayer.die` sends
//!   `PlayerCombatKill` with the message, and `Player.dropEquipment` throws the inventory
//!   (`Inventory.dropAll`), which a watcher is shown; `ServerPlayer.restoreFrom` gives the
//!   new body full health and a new `FoodData` (20, 5).
//! - `Player.attack`: a fist hits another player for 1 (`player_attack`), and
//!   `causeExtraKnockback @124-166` sends the hit player its own `SetEntityMotion`.

mod common;

use std::time::Duration;

use anyhow::{Context, Result};
use clustine::Config;
use clustine_botswarm::{Bot, Connection};
use clustine_data::{blocks, entity_types, items};
use clustine_protocol::packets::play::{
    ClientCommand, ClientboundPlay, Respawn, SetHealth, client_command, face,
};
use clustine_sim::api::Order;
use clustine_world::GameMode;
use tokio::time::{Instant, timeout};

use common::{VIEW_DISTANCE, config, start_with, view_area};

const PATIENCE: Duration = Duration::from_secs(60);

/// Far below the flat world's lowest y, -64, less the void's 64.
const IN_THE_VOID: f64 = -200.0;

/// A world whose new players are survival ones.
fn survival() -> Config {
    Config {
        game_mode: GameMode::Survival,
        force_game_mode: true,
        ..config()
    }
}

/// Joins and waits for the chunks around the spawn point and for window 0 whole; the bot
/// said it has loaded (`PlayerLoaded`) once its chunk came, as a vanilla client does, and
/// a use of the ground acknowledged after it says the server has taken that: from then on
/// the player can be hurt (`ServerPlayer.isInvulnerableTo`).
async fn join(address: &str, name: &str) -> Bot {
    let mut bot = Bot::join(address, name).await.unwrap();
    let count = view_area((0, 0), VIEW_DISTANCE).len();
    bot.wait_for_chunks(count, PATIENCE).await.unwrap();
    bot.wait_until(PATIENCE, |bot| bot.inventory_window.state_id >= 1)
        .await
        .unwrap();
    round_trip(&mut bot).await;
    bot
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

/// The health, food and saturation the bot was told last, or a new client's own (20, 20
/// and 5) where it was told none.
fn health(bot: &Bot) -> SetHealth {
    bot.health.unwrap_or(SetHealth {
        health: 20.0,
        food: 20,
        saturation: 5.0,
    })
}

/// Has the operator give `name` `count` of `item`, and waits until its hotbar slot 0
/// (window 0's 36) holds it: a survival newcomer starts with nothing.
async fn given(server: &clustine::Server, bot: &mut Bot, name: &str, item: i32, count: i32) {
    server
        .order(name, Order::Give { item, count })
        .await
        .unwrap();
    bot.wait_until(PATIENCE, |bot| {
        bot.inventory_window.slots[36]
            .as_ref()
            .is_some_and(|stack| stack.item == item && stack.count == count)
    })
    .await
    .unwrap_or_else(|error| panic!("{error}: {count} of {item} were not given"));
}

/// `LivingEntity.calculateFallDamage`: a bot builds a pillar of five cobblestone beside
/// it, climbs it, steps off and falls a block a tick to the ground: five blocks, 2 of
/// health, which it is told; a fall costs no food (`fall`'s exhaustion is 0).
#[tokio::test]
async fn a_bot_that_jumps_off_a_pillar_it_built_is_told_less_health() {
    let (server, address) = start_with(survival()).await;
    let mut bot = join(&address, "Climber").await;
    given(&server, &mut bot, "Climber", items::COBBLESTONE, 5).await;
    bot.select_slot(0).await.unwrap();
    let (x, z) = (bot.location.0.floor() as i32, bot.location.2.floor() as i32);
    let pillar = x + 1;
    for height in 0..5 {
        bot.use_item_on(pillar, -61 + height, z, face::TOP)
            .await
            .unwrap();
        bot.wait_until(PATIENCE, |bot| {
            bot.block_at(pillar, -60 + height, z)
                .unwrap()
                .is_some_and(|state| {
                    clustine_data::BlockState(state as u16).block_id()
                        == blocks::COBBLESTONE.block_id()
                })
        })
        .await
        .unwrap_or_else(|error| panic!("{error}: block {height} of the pillar"));
    }
    // On top of the pillar, its top at y = -56, then off it to the east and down a block a
    // tick to the floor, whose top is at -60.
    let top = -55.0;
    let (on_x, off_x, middle_z) = (
        f64::from(pillar) + 0.5,
        f64::from(pillar) + 1.5,
        f64::from(z) + 0.5,
    );
    bot.move_in_tick(on_x, top, middle_z, true).await.unwrap();
    bot.move_in_tick(off_x, top, middle_z, false).await.unwrap();
    for down in 1..=5 {
        let y = top - f64::from(down);
        bot.move_in_tick(off_x, y, middle_z, down == 5)
            .await
            .unwrap();
    }
    bot.wait_until(PATIENCE, |bot| {
        bot.health.as_ref().is_some_and(|told| told.health < 20.0)
    })
    .await
    .unwrap_or_else(|error| panic!("{error}: the fall was not told: {:?}", bot.health));
    let told = health(&bot);
    assert_eq!(told.health, 18.0, "{told:?}");
    assert_eq!(told.food, 20, "{told:?}");
    server.stop().await;
}

/// `FoodData.tick @11-75` with `jumpFromGround`'s 0.2 a sprinting jump and 0.1 a metre
/// sprinting on the ground: a bot that sprints and jumps back and forth and eats nothing
/// uses up its 5 of saturation (told when it reaches 0, `doTick @146-264`) and then loses
/// food, which it is told.
#[tokio::test]
async fn a_bot_that_sprints_and_does_not_eat_loses_food() {
    let (server, address) = start_with(survival()).await;
    let mut bot = join(&address, "Runner").await;
    bot.sprint(true).await.unwrap();
    let (x, y, z) = bot.location;
    let mut east = true;
    let mut at = x;
    let deadline = Instant::now() + PATIENCE;
    while health(&bot).food == 20 {
        assert!(
            Instant::now() < deadline,
            "no food was lost: {:?}",
            bot.health
        );
        // A jump: off the ground and rising, half a block on; then the landing, half a
        // block more on the ground.
        let step = if east { 0.5 } else { -0.5 };
        at += step;
        bot.move_in_tick(at, y + 0.42, z, false).await.unwrap();
        at += step;
        bot.move_in_tick(at, y, z, true).await.unwrap();
        if (at - x).abs() >= 3.0 {
            east = !east;
        }
    }
    let told = health(&bot);
    assert_eq!(told.food, 19, "{told:?}");
    assert_eq!(told.health, 20.0, "{told:?}");
    server.stop().await;
}

/// What the client was sent once it asked for its respawn: the `Respawn`, and after it
/// a health of more than nothing.
#[derive(Default)]
struct AfterTheAsk {
    respawn: Option<Respawn>,
    health: Option<SetHealth>,
}

async fn read_the_respawn(connection: &mut Connection) -> Result<AfterTheAsk> {
    let deadline = Instant::now() + PATIENCE;
    let mut seen = AfterTheAsk::default();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let frame = timeout(left, connection.read_frame())
            .await
            .context("the respawn did not come in time")??;
        match ClientboundPlay::decode(&frame)? {
            ClientboundPlay::Respawn(respawn) => {
                seen.respawn.get_or_insert(respawn);
            }
            ClientboundPlay::SetHealth(health) if seen.respawn.is_some() && health.health > 0.0 => {
                seen.health.get_or_insert(health);
            }
            _ => {}
        }
        if seen.health.is_some() {
            return Ok(seen);
        }
    }
}

/// `LivingEntity.onBelowWorld` and `ServerPlayer.die`: a bot holding dirt goes into the
/// void, is hurt until it dies and is shown its death (`death.attack.outOfWorld`); its
/// dirt is thrown where it died (`Inventory.dropAll`), which a watcher at the spawn is
/// shown (the items then fall out of the world, as the game's do); it asks for its
/// respawn and is told full health and a new `FoodData`, 20 and 5.
#[tokio::test]
async fn a_bot_that_dies_in_the_void_drops_its_items_and_respawns_with_full_health_and_food() {
    let (server, address) = start_with(survival()).await;
    let mut watcher = join(&address, "Watcher").await;
    let mut bot = join(&address, "Faller").await;
    given(&server, &mut bot, "Faller", items::DIRT, 16).await;
    let (x, _, z) = bot.location;
    // In the air, falling: a move onto the ground would be a fall of 140 blocks
    // (`Entity.checkFallDamage` adds the move's drop and then lands), which kills first.
    bot.move_in_tick(x, IN_THE_VOID, z, false).await.unwrap();
    bot.wait_until(PATIENCE, |bot| !bot.deaths.is_empty())
        .await
        .unwrap_or_else(|error| panic!("{error}: the void did not kill: {}", bot.report()));
    assert_eq!(health(&bot).health, 0.0);
    let message = format!("{:?}", bot.deaths[0].message);
    assert!(message.contains("death.attack.outOfWorld"), "{message}");
    let item = |entity: &clustine_botswarm::SeenEntity| {
        entity.kind == entity_types::ITEM
            && (entity.position.0 - x).abs() < 2.0
            && (entity.position.2 - z).abs() < 2.0
    };
    watcher
        .wait_until(PATIENCE, |watcher| {
            watcher.entities.values().any(item) || watcher.vanished.iter().any(item)
        })
        .await
        .unwrap_or_else(|error| panic!("{error}: the watcher was shown no dropped item"));

    let mut connection = bot.into_connection();
    connection
        .write(&ClientCommand {
            action: client_command::PERFORM_RESPAWN,
        })
        .await
        .unwrap();
    let seen = read_the_respawn(&mut connection).await.unwrap();
    assert_eq!(
        seen.respawn.expect("a Respawn").data_to_keep,
        0,
        "a death's respawn keeps nothing"
    );
    let told = seen.health.expect("a health");
    assert_eq!(
        (told.health, told.food, told.saturation),
        (20.0, 20, 5.0),
        "{told:?}"
    );
    drop(connection);
    server.stop().await;
}

/// `Player.attack`: one survival bot punches another, who is told 1 less health and its
/// own motion at once (`causeExtraKnockback`'s `SetEntityMotion` of its own entity).
#[tokio::test]
async fn two_bots_fight_and_the_one_hit_is_told_its_health_and_its_knock() {
    let (server, address) = start_with(survival()).await;
    let mut hit = join(&address, "Hit").await;
    hit.record_packets();
    let mut hitter = join(&address, "Hitter").await;
    let (x, _, z) = hitter.location;
    hitter.step_to(x + 2.0, z).await.unwrap();
    let target = {
        hitter
            .wait_until(PATIENCE, |bot| bot.seen_player("Hit").is_some())
            .await
            .unwrap();
        hitter.seen_player("Hit").expect("seen").entity_id
    };
    hitter.attack(target).await.unwrap();
    hit.wait_until(PATIENCE, |bot| {
        bot.health.as_ref().is_some_and(|told| told.health < 20.0)
    })
    .await
    .unwrap_or_else(|error| panic!("{error}: the hit was not told: {:?}", hit.health));
    assert_eq!(health(&hit).health, 19.0);
    let own = hit.info.login.entity_id;
    let knocked = hit.packets.as_deref().unwrap_or_default().iter().any(|(_, packet)| {
        matches!(packet, ClientboundPlay::SetEntityMotion(motion) if motion.entity_id == own)
    });
    assert!(knocked, "the hit player is told its own motion");
    server.stop().await;
}
