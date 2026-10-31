//! End-to-end tests of N9's part A at a pinned line and across a player's leave, written
//! from the plan (`docs/groundwork/n9-plan.md`, sections 5.4, 8.3 and 14) by someone who
//! did not write the code, with real clients: an effect that goes on across a pinned
//! line, through the edge, the store and the next region, and ends when its duration is
//! over; a splash beside the line that gives its effect to a player of the region
//! across it, through the store; and an effect that comes back at a rejoin with the
//! ticks it had left.
//!
//! The tests at a pinned line divide their worlds themselves at chunk x = 0 and 4 (the
//! world of `CLUSTINE_TEST_PINS=0,4`), so that the line at block x = 0 is beside the
//! spawn whatever the environment says.

mod common;

use std::time::Duration;

use clustine::Config;
use clustine_botswarm::Bot;
use clustine_data::items;
use clustine_protocol::components::{
    EffectDetails, EffectInstance, PotionContents, WireComponent, WireComponents, mob_effect,
};
use clustine_protocol::item::ItemStack;
use clustine_protocol::packets::play::ClientboundPlay;
use tokio::time::Instant;

use common::{VIEW_DISTANCE, config, start_with, view_area};

const PATIENCE: Duration = Duration::from_secs(30);

/// The block under the feet that a round trip breaks: bedrock, which nothing breaks.
const BEDROCK: i32 = -64;

/// A world in `directory` divided at chunk x = 0 and 4, whose regions stay as they are:
/// the line at block x = 0 is beside the spawn.
fn pinned(directory: &std::path::Path) -> Config {
    Config {
        world: Some(directory.to_owned()),
        pins: vec![0, 4],
        follow: None,
        ..config()
    }
}

fn speed() -> i32 {
    let at = mob_effect::NAMES
        .iter()
        .position(|name| *name == "minecraft:speed")
        .expect("speed in its registry");
    i32::try_from(at).expect("a small registry")
}

/// A potion of `item` whose contents are speed for `duration` ticks alone.
fn speed_potion(item: i32, duration: i32) -> ItemStack {
    ItemStack {
        item,
        count: 1,
        components: WireComponents {
            added: vec![WireComponent::PotionContents(PotionContents {
                potion: None,
                custom_colour: None,
                custom_effects: vec![EffectInstance {
                    effect: speed(),
                    details: EffectDetails {
                        amplifier: 0,
                        duration,
                        ambient: false,
                        visible: true,
                        show_icon: true,
                        hidden: None,
                    },
                }],
                custom_name: None,
            })],
            removed: Vec::new(),
        },
    }
}

/// Joins as `name`, keeps every packet from then on, and waits for the chunks about the
/// spawn.
async fn join(address: &str, name: &str) -> Bot {
    let mut bot = Bot::join(address, name).await.expect("the bot joins");
    bot.record_packets();
    let count = view_area((0, 0), VIEW_DISTANCE).len();
    bot.wait_for_chunks(count, PATIENCE)
        .await
        .expect("the chunks about the spawn come");
    bot
}

/// A round trip: the server has handled everything the bot sent before.
async fn round_trip(bot: &mut Bot) {
    let (x, z) = (bot.location.0.floor() as i32, bot.location.2.floor() as i32);
    let last = bot.dig(x, BEDROCK, z).await.expect("sent");
    bot.wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= last)
        .await
        .unwrap_or_else(|error| panic!("{error:#}: {}", bot.report()));
}

/// Walks the bot to `x`, `z` and waits until the server has its last position.
async fn stand(bot: &mut Bot, x: f64, z: f64) {
    bot.walk_to(x, z, 0.5).await.expect("walked");
    round_trip(bot).await;
}

/// The speed the bot was told of for its own entity: each `(when, duration)` it was
/// added or changed, and when it was removed.
fn own_speed(bot: &Bot) -> (Vec<(Instant, i32)>, Vec<Instant>) {
    let me = bot.info.login.entity_id;
    let mut added = Vec::new();
    let mut removed = Vec::new();
    for (at, packet) in bot.packets.iter().flatten() {
        match packet {
            ClientboundPlay::UpdateMobEffect(update)
                if update.entity_id == me && update.effect == speed() =>
            {
                added.push((*at, update.duration));
            }
            ClientboundPlay::RemoveMobEffect(remove)
                if remove.entity_id == me && remove.effect == speed() =>
            {
                removed.push(*at);
            }
            _ => {}
        }
    }
    (added, removed)
}

/// The game time the server last told the client, which it tells every 20 ticks: a
/// measure of ticks that a slow machine does not stretch, to within those 20.
fn game_time(bot: &Bot) -> i64 {
    bot.times.last().map_or(0, |time| time.game_time)
}

/// Drinks `potion` from the first hotbar slot, and waits until its speed is told.
async fn drink(bot: &mut Bot, potion: ItemStack) -> (Instant, i32) {
    bot.put_in_hotbar(0, potion).await.expect("sent");
    bot.select_slot(0).await.expect("sent");
    round_trip(bot).await;
    bot.use_item(0.0, 0.0).await.expect("sent");
    bot.wait_until(PATIENCE, |bot| !own_speed(bot).0.is_empty())
        .await
        .unwrap_or_else(|error| panic!("{error:#}: the drink's speed is told"));
    own_speed(bot).0[0]
}

/// Sections 5.4 and decision 8 of 18: an effect goes with its player across a pinned
/// line, carried with the ticks it has left, and ends when its duration is over, told
/// once; a hand-over the client does not see takes nothing from it and adds nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_effect_goes_on_across_a_pinned_line_and_ends_when_its_duration_is_over() {
    let directory = tempfile::tempdir().expect("a directory");
    let (server, address) = start_with(pinned(directory.path())).await;
    let mut bot = join(&address, "Drinker").await;
    stand(&mut bot, 2.5, 0.5).await;
    let (_, duration) = drink(&mut bot, speed_potion(items::POTION, 200)).await;
    let told = game_time(&bot);
    assert_eq!(duration, 200);
    // Across the line into the western region, and on.
    stand(&mut bot, -2.5, 0.5).await;
    stand(&mut bot, -4.5, 3.5).await;
    bot.wait_until(PATIENCE, |bot| !own_speed(bot).1.is_empty())
        .await
        .unwrap_or_else(|error| panic!("{error:#}: the speed ends"));
    let (_, removed) = own_speed(&bot);
    assert_eq!(removed.len(), 1, "told once that it ended");
    let lasted = game_time(&bot) - told;
    // Its 200 ticks, by the server's clock as the client was last told it either side,
    // which is told every 20 ticks.
    assert!(
        (180..=220).contains(&lasted),
        "ended {lasted} ticks of the server's clock after it was told"
    );
    server.stop().await;
}

/// Sections 4.4 and 8.3: a splash potion that breaks beside the line gives its effect to
/// a player standing across it, whose stay is in the other region: the thrower's region
/// sends it addressed to them, and the store brings it to their region.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_splash_beside_a_pinned_line_gives_its_effect_to_a_player_across_it() {
    let directory = tempfile::tempdir().expect("a directory");
    let (server, address) = start_with(pinned(directory.path())).await;
    let mut across = join(&address, "Across").await;
    stand(&mut across, -1.0, 0.5).await;
    let mut thrower = join(&address, "Thrower").await;
    stand(&mut thrower, 1.0, 0.5).await;
    thrower
        .put_in_hotbar(0, speed_potion(items::SPLASH_POTION, 600))
        .await
        .expect("sent");
    thrower.select_slot(0).await.expect("sent");
    round_trip(&mut thrower).await;
    // At the thrower's own feet.
    thrower.use_item(0.0, 90.0).await.expect("sent");
    thrower
        .wait_until(PATIENCE, |bot| !own_speed(bot).0.is_empty())
        .await
        .unwrap_or_else(|error| panic!("{error:#}: the thrower is splashed"));
    across
        .wait_until(PATIENCE, |bot| !own_speed(bot).0.is_empty())
        .await
        .unwrap_or_else(|error| panic!("{error:#}: the player across the line is splashed"));
    let (added, _) = own_speed(&across);
    let duration = added[0].1;
    assert!(
        duration > 20 && duration < 600,
        "scaled by the distance: {duration}"
    );
    server.stop().await;
}

/// Section 5.4: "an effect does not run down while its player is away". A player who
/// drank a potion and stood for three seconds, their client saying where it is once a
/// second as a client does, then left, is told the effect again at their rejoin with
/// the ticks it had left at the leave, and it ends after them.
///
/// The region notes a stay's place only in a tick that changes the player otherwise, so
/// the place kept at the leave is as old as the client's last word: up to the second a
/// standing client's position reminders leave between them (the finding of
/// `n9_player_effects.rs`, bounded so); the test allows that second.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_effect_comes_back_at_a_rejoin_with_the_ticks_it_had_left() {
    let directory = tempfile::tempdir().expect("a directory");
    let world = Config {
        world: Some(directory.path().to_owned()),
        ..config()
    };
    let (server, address) = start_with(world).await;
    let mut bot = join(&address, "Leaver").await;
    let _ = drink(&mut bot, speed_potion(items::POTION, 600)).await;
    let told = game_time(&bot);
    for _ in 0..3 {
        bot.idle(Duration::from_secs(1)).await.expect("idled");
        round_trip(&mut bot).await;
    }
    bot.idle(Duration::from_millis(700)).await.expect("idled");
    let left = game_time(&bot);
    bot.close().await.expect("left");
    drop(bot);
    let mut back = join(&address, "Leaver").await;
    back.wait_until(PATIENCE, |bot| !own_speed(bot).0.is_empty())
        .await
        .unwrap_or_else(|error| panic!("{error:#}: the speed is told at the join"));
    let (added, _) = own_speed(&back);
    let had_left = 600 - (left - told);
    let duration = i64::from(added[0].1);
    // The second of the client's last word, and the 20 ticks between the times the
    // server tells, on either side.
    assert!(
        (had_left - 25..=had_left + 45).contains(&duration),
        "told {duration} ticks at the join, with {had_left} left at the leave"
    );
    server.stop().await;
}
