//! Comparisons with the official server of a villager's trades (`docs/groundwork/n10-plan.md`,
//! section 14; `docs/groundwork/n10-brief.md`, sections 5.1 to 5.6): the merchant's
//! window packet by packet through a trade, a shift-click that repeats it, a choice of a
//! trade and a close, and the offers the next opening shows after the villager has
//! restocked at its job site.
//!
//! **The cleric.** The plan has a villager summoned with fixed `Offers`, `VillagerData`
//! and `LastRestock`. Clustine has no command, and no other way to make a mob from a tag
//! (a spawn egg's `entity_data` is not read, `region/spawn_eggs.rs`), so the scene is
//! made by what a creative player can do on both servers (`common/n10_village.rs`): a
//! villager by its egg in a pit, a brewing stand placed beside it, which makes it a
//! cleric at level 1 (`AssignProfessionFromJobSite`). A cleric's first trade set is
//! two trades of two, neither of which draws anything (`trade_set/cleric/level_1`:
//! an emerald for two redstone, 12 uses, 1 experience; 32 rotten flesh for an emerald,
//! 16 uses, 2 experience; both with a multiplier of 0.05), so its offers are fixed but
//! for their order, which `addOffersFromTradeSet` draws (from the world's sequence on
//! the official server, from the villager's own stream on Clustine: decision 5). So the
//! offers of every `merchant_offers` packet are compared sorted, and the bot picks the
//! redstone trade by what it is, not by its place.
//!
//! **The steps.** Both servers start at 2500 ticks into a day, in the villager's working
//! hours. The bot holds five emeralds in its first hotbar slot and nothing else. Then:
//! it opens the window (`open_screen`, the window's contents, `merchant_offers`); picks
//! the redstone trade (`select_trade`, which moves the five emeralds into the first
//! payment slot and shows the result); takes the result, guessing that its cursor holds
//! two redstone and the result slot is empty (one trade); puts the redstone into the
//! inventory's first slot; shift-clicks the result, guessing nothing, which trades until
//! the emeralds run out (four more); picks the rotten flesh trade, which puts nothing
//! into the payment slots, there being no flesh; and closes the window. Every packet of
//! the window each step brings (`open_screen`, the contents but the inventory's own,
//! slots, the cursor, closes and `merchant_offers`) is compared in order, the state ids
//! among them. A step ends when the server has handled what was sent and the tick after
//! it, since the offers come again the tick after a trade (the `TRADE` event's
//! `updateSpecialPrices`).
//!
//! **The restock.** After the close the bot waits for the cleric's work sound, which
//! `WorkAtPoi.start` plays before it restocks a villager that has traded (finding 22:
//! 300 ticks after its last check and a level draw, so when is not compared), then opens
//! the window again: the offers it is sent are compared, their uses back at 0 and their
//! demand updated (`5 − (12 − 5)` for the redstone trade). `LastRestock` cannot be set
//! on Clustine, so its timing is not compared beyond this: a villager whose offers were
//! used restocks at its first work at the site.
//!
//! Left out, each because it is drawn: the experience orb a trade makes (`3 +
//! nextInt(4)`), the villager's sounds (`villager_yes` and its pitch), where it looks.
//!
//! They need Java, the server jar downloaded by `cargo datagen`, and your agreement to
//! the Minecraft EULA, so they are ignored by default. Run them on a quiet machine:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test merchant_oracle -- --ignored official_server --nocapture --test-threads 1
//! ```

mod common;
#[path = "common/n10_village.rs"]
#[allow(dead_code)]
mod n10_village;
#[path = "common/n9_scene.rs"]
#[allow(dead_code)]
mod n9_scene;

use std::time::Duration;

use anyhow::{Context, Result};
use clustine_data::villagers::professions;
use clustine_data::{entity_types, items, sounds};
use clustine_protocol::item::ItemStack;
use clustine_protocol::packets::play::{
    ClientboundPlay, HashedStack, MerchantOffers, click, face, inventory,
};

use n9_scene::{PATIENCE, Scene};
use n10_village::{Pit, hatch, interact, on_both_at, select_trade, until_profession, until_seen};

/// The time of day both servers start at: in a villager's working hours (2000 to 9000).
const DAY: u64 = 2_500;

/// The hotbar slot the emeralds are in, and the empty one the bot holds.
const EMERALDS: i16 = 0;
const EMPTY: i16 = 8;

/// The emeralds the bot brings: five trades of the redstone trade, which leaves the
/// villager at 5 experience, short of the 10 of its next level.
const BROUGHT: i32 = 5;

/// The merchant window's first slot of the player's inventory (after the two payments
/// and the result).
const INVENTORY_FIRST: i16 = 3;
const RESULT: i16 = 2;

/// How long the cleric may take to work at its site after the close: the next 300-tick
/// check, then coin flips each tick.
const WORK_PATIENCE: Duration = Duration::from_secs(60);

/// The window packets of one run, each step's apart.
#[derive(Debug, Default, PartialEq)]
struct Ran {
    steps: Vec<(&'static str, Vec<ClientboundPlay>)>,
}

/// Whether `packet` is one of the merchant's window, as `containers_oracle.rs` keeps
/// them, with the offers.
fn of_the_window(packet: &ClientboundPlay) -> bool {
    match packet {
        ClientboundPlay::OpenScreen(_)
        | ClientboundPlay::ContainerSetSlot(_)
        | ClientboundPlay::SetCursorItem(_)
        | ClientboundPlay::ClientboundContainerClose(_)
        | ClientboundPlay::MerchantOffers(_) => true,
        ClientboundPlay::SetContainerContent(content) => {
            content.window_id != inventory::PLAYER_WINDOW
        }
        _ => false,
    }
}

/// The offers in an order of their own, which the trade set's draw does not decide.
fn sorted(mut offers: MerchantOffers) -> MerchantOffers {
    offers.offers.sort_by_key(|offer| format!("{offer:?}"));
    offers
}

/// The window's packets from `from` on, the offers sorted.
fn window_since(scene: &Scene, from: usize) -> Vec<ClientboundPlay> {
    scene
        .of(from, |packet| of_the_window(packet).then(|| packet.clone()))
        .into_iter()
        .map(|(_, packet)| match packet {
            ClientboundPlay::MerchantOffers(offers) => {
                ClientboundPlay::MerchantOffers(sorted(offers))
            }
            packet => packet,
        })
        .collect()
}

/// The latest offers sent from `from` on, as they were sent.
fn latest_offers(scene: &Scene, from: usize) -> Option<MerchantOffers> {
    scene
        .of(from, |packet| match packet {
            ClientboundPlay::MerchantOffers(offers) => Some(offers.clone()),
            _ => None,
        })
        .pop()
        .map(|(_, offers)| offers)
}

/// The place among `offers` of the trade whose result is `item`.
fn index_of(offers: &MerchantOffers, item: i32) -> Result<i32> {
    let at = offers
        .offers
        .iter()
        .position(|offer| {
            offer
                .result
                .as_ref()
                .is_some_and(|stack| stack.item == item)
        })
        .with_context(|| format!("no offer of item {item} in {offers:?}"))?;
    Ok(i32::try_from(at)?)
}

fn hashed(item: i32, count: i32) -> HashedStack {
    HashedStack {
        item,
        count,
        added: Vec::new(),
        removed: Vec::new(),
    }
}

/// Ends a step: what the window was sent from `from` on, once the server has handled
/// what the bot sent and the tick after it has been sent.
async fn step(scene: &mut Scene, ran: &mut Ran, name: &'static str, from: usize) -> Result<()> {
    scene.round_trip().await?;
    ran.steps.push((name, window_since(scene, from)));
    Ok(())
}

/// Opens the villager's window and waits for its offers.
async fn open(scene: &mut Scene, villager: i32) -> Result<usize> {
    let from = scene.mark();
    scene.window = None;
    interact(scene, villager).await?;
    scene
        .until(PATIENCE, "the merchant's window", |scene| {
            scene
                .window
                .as_ref()
                .is_some_and(|window| !window.slots.is_empty())
        })
        .await?;
    until_seen(scene, from, PATIENCE, "the merchant's offers", |seen| {
        matches!(seen.packet, Some(ClientboundPlay::MerchantOffers(_)))
    })
    .await?;
    Ok(from)
}

async fn trade(address: &str) -> Result<Ran> {
    let mut scene = Scene::join(address, "Trader").await?;
    scene.empty_hotbar().await?;
    let pit = Pit::beside(&scene, 2);
    pit.dig(&mut scene).await?;
    let villager = hatch(
        &mut scene,
        items::VILLAGER_SPAWN_EGG,
        entity_types::VILLAGER,
        pit.floor(0),
    )
    .await?;
    let from = scene.mark();
    scene
        .place(items::BREWING_STAND, pit.floor(1), face::TOP, pit.feet(1))
        .await?;
    until_profession(&mut scene, from, villager, professions::CLERIC).await?;

    // Five emeralds and an empty hand, nothing else.
    scene.empty_hotbar().await?;
    scene
        .put(EMERALDS, ItemStack::new(items::EMERALD, BROUGHT))
        .await?;
    scene.hold(EMPTY).await?;
    scene.round_trip().await?;

    let mut ran = Ran::default();
    let from = open(&mut scene, villager).await?;
    let offers = latest_offers(&scene, from).context("the offers")?;
    let redstone = index_of(&offers, items::REDSTONE)?;
    let flesh = index_of(&offers, items::EMERALD)?;
    step(&mut scene, &mut ran, "open the window", from).await?;

    let from = scene.mark();
    select_trade(&mut scene, redstone).await?;
    step(&mut scene, &mut ran, "pick the redstone trade", from).await?;

    // The result taken: the client guesses its cursor holds it and the slot is empty.
    let from = scene.mark();
    scene
        .click(
            RESULT,
            0,
            click::PICKUP,
            vec![(RESULT, None)],
            Some(hashed(items::REDSTONE, 2)),
        )
        .await?;
    step(&mut scene, &mut ran, "take the result", from).await?;

    let from = scene.mark();
    scene
        .click(
            INVENTORY_FIRST,
            0,
            click::PICKUP,
            vec![(INVENTORY_FIRST, Some(hashed(items::REDSTONE, 2)))],
            None,
        )
        .await?;
    step(&mut scene, &mut ran, "put the redstone down", from).await?;

    // Guessing nothing: the servers say what each repeated trade changed.
    let from = scene.mark();
    scene
        .click(RESULT, 0, click::QUICK_MOVE, Vec::new(), None)
        .await?;
    step(&mut scene, &mut ran, "shift-click the result", from).await?;

    let from = scene.mark();
    select_trade(&mut scene, flesh).await?;
    step(&mut scene, &mut ran, "pick the rotten flesh trade", from).await?;

    let from = scene.mark();
    scene.close_window().await?;
    step(&mut scene, &mut ran, "close the window", from).await?;

    // The next work at the site restocks the used trade.
    let from = scene.mark();
    until_seen(
        &mut scene,
        from,
        WORK_PATIENCE,
        "the cleric's work sound",
        |seen| match &seen.packet {
            Some(ClientboundPlay::Sound(sound)) => {
                sound.sound == sounds::ENTITY_VILLAGER_WORK_CLERIC
            }
            Some(ClientboundPlay::SoundEntity(sound)) => {
                sound.sound == sounds::ENTITY_VILLAGER_WORK_CLERIC
            }
            _ => false,
        },
    )
    .await?;
    scene.round_trip().await?;
    let from = open(&mut scene, villager).await?;
    step(
        &mut scene,
        &mut ran,
        "open the window after the restock",
        from,
    )
    .await?;
    let from = scene.mark();
    scene.close_window().await?;
    step(&mut scene, &mut ran, "close it again", from).await?;
    Ok(ran)
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_clerics_window_trades_and_restocks_as_on_the_official_server() {
    let (theirs, ours) = on_both_at(DAY, trade).await;
    let theirs = theirs.unwrap_or_else(|error| panic!("the official server: {error:#}"));
    let ours = ours.unwrap_or_else(|error| panic!("Clustine: {error:#}"));
    let mut differences = Vec::new();
    for ((name, their), (_, our)) in theirs.steps.iter().zip(&ours.steps) {
        println!("{name}:\n  official: {their:?}\n  Clustine: {our:?}");
        if their != our {
            differences.push(*name);
        }
    }
    assert_eq!(theirs.steps.len(), ours.steps.len(), "the steps run");
    assert!(
        differences.is_empty(),
        "these steps differ, printed above: {differences:?}"
    );
}
