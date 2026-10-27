//! Comparisons with the official server of a raid's start and its bar
//! (`docs/groundwork/n10-plan.md`, section 14; `docs/groundwork/n10-brief.md`, findings 26
//! to 29 and section 7): from an omen drunk in a village to the first wave.
//!
//! **The village.** No command starts a raid, and Clustine has none of the plan's
//! (`/effect`, `/setblock`), so the scene is made by what a creative player can do on
//! both servers (`common/n10_village.rs`): a villager by its egg in a pit and a lectern
//! beside it, which it claims as its job site. A claimed point of `#village` is a
//! village (`PoiManager.sectionsToVillage`, occupied points only), and the raid's centre
//! the mean of the occupied ones within 64: the lectern. The bot, hovering above the
//! pit, drinks an ominous bottle (bad omen 0, 120000 ticks), which in the village becomes
//! raid omen for 600 ticks, at whose end the raid begins (finding 28). Both servers start
//! at 2500 ticks into a day; a raid does not mind the time.
//!
//! **What is compared exactly**, since nothing in it draws:
//!
//! - the bot's effect packets from the drink to the bar: the bad omen given, the raid
//!   omen given, the bad omen taken away, each with its amplifier, duration and flags;
//! - every packet of the bar from its add to the first raider's spawn and the tick
//!   after: its add (the name, `event.minecraft.raid`, the progress, the colour, the
//!   notched overlay and the flags), and each progress and name after it, in order (the
//!   300-tick countdown fills it by `(300 − cooldown) / 300`; the raiders' health
//!   then sets it); the bar's UUID is drawn (two `nextLong`s of the raid's random) and
//!   only held to be one bar on each server;
//! - the horn's sound packets: their sound, source, volume and pitch (their place is 13
//!   blocks towards the wave's spawn, which is drawn, and their seed is drawn);
//! - by a client's clock: the ticks from the raid omen to the bar (the omen's 600, held
//!   within [`APART`] of each other) and from the bar to the first raider (the 300 of the
//!   countdown, likewise).
//!
//! Left out, each because the raid's random draws it: where the wave spawns, which
//! raiders and how many (on easy a vindicator's and a pillager's bonus is `nextInt(2)`
//! each), and everything after the first wave, which a fight decides.
//!
//! The raid's rules are still being built (part R1) beside this comparison, which was
//! written from the specification: it may fail against Clustine until they are in.
//!
//! They need Java, the server jar downloaded by `cargo datagen`, and your agreement to
//! the Minecraft EULA, so they are ignored by default. Run them on a quiet machine:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test raids_oracle -- --ignored official_server --nocapture --test-threads 1
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
use clustine_protocol::packets::play::{BossOperation, ClientboundPlay, face};
use tokio::time::Instant;
use uuid::Uuid;

use n9_scene::{Scene, ticks};
use n10_village::{Pit, hatch, on_both_at, until_profession, until_seen};

/// The time of day both servers start at.
const DAY: u64 = 2_500;

/// How long the omen may take to become a raid: the drink (40 ticks), the raid omen's
/// 600, with room.
const OMEN_PATIENCE: Duration = Duration::from_secs(60);

/// How long the countdown may take to bring the first raider: its 300 ticks, with room
/// for the spawn's searches.
const WAVE_PATIENCE: Duration = Duration::from_secs(45);

/// How many ticks of a client's clock the two servers' spans may differ by.
const APART: i64 = 3;

/// The raiders a wave may bring on easy.
const RAIDERS: [i32; 5] = [
    entity_types::PILLAGER,
    entity_types::VINDICATOR,
    entity_types::RAVAGER,
    entity_types::EVOKER,
    entity_types::WITCH,
];

/// An effect packet the bot was sent: given (the effect, its amplifier, duration and
/// flags) or taken away.
#[derive(Debug, Clone, PartialEq)]
enum Effect {
    Given(i32, i32, i32, u8),
    Taken(i32),
}

#[derive(Debug)]
struct Raided {
    effects: Vec<Effect>,
    /// The bar's operations, from its add to the tick after the first raider.
    bar: Vec<BossOperation>,
    /// The UUIDs the bar's packets carried.
    bars: Vec<Uuid>,
    /// The horn's sound, source, volume and pitch, each time it was heard.
    horns: Vec<(i32, i32, f32, f32)>,
    /// The ticks from the raid omen to the bar, and from the bar to the first raider.
    omen_to_bar: i64,
    bar_to_wave: i64,
}

async fn raid(address: &str) -> Result<Raided> {
    let mut scene = Scene::join(address, "Omen").await?;
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
        .place(items::LECTERN, pit.floor(1), face::TOP, pit.feet(1))
        .await?;
    until_profession(&mut scene, from, villager, professions::LIBRARIAN).await?;
    scene.round_trip().await?;

    // Drunk where the bot hovers, above the pit and so in the village.
    let from = scene.mark();
    scene.take(ItemStack::new(items::OMINOUS_BOTTLE, 1)).await?;
    scene.round_trip().await?;
    scene.use_item(scene.look.0, scene.look.1).await?;
    let me = scene.me;
    let added = until_seen(&mut scene, from, OMEN_PATIENCE, "the raid's bar", |seen| {
        matches!(&seen.packet, Some(ClientboundPlay::BossEvent(event))
            if matches!(event.operation, BossOperation::Add { .. }))
    })
    .await?;
    let first = until_seen(
        &mut scene,
        from,
        WAVE_PATIENCE,
        "the first raider",
        |seen| {
            matches!(&seen.packet, Some(ClientboundPlay::SpawnEntity(spawn))
            if RAIDERS.contains(&spawn.kind))
        },
    )
    .await?;
    scene.round_trip().await?;

    let effects: Vec<(Instant, Effect)> = scene.of(from, |packet| match packet {
        ClientboundPlay::UpdateMobEffect(effect) if effect.entity_id == me => Some(Effect::Given(
            effect.effect,
            effect.amplifier,
            effect.duration,
            effect.flags,
        )),
        ClientboundPlay::RemoveMobEffect(effect) if effect.entity_id == me => {
            Some(Effect::Taken(effect.effect))
        }
        _ => None,
    });
    // The raid omen is the effect given last before the bar, and lasts 600 ticks.
    let omen = effects
        .iter()
        .filter(|(at, effect)| {
            *at <= added && matches!(effect, Effect::Given(_, _, duration, _) if *duration == 600)
        })
        .map(|(at, _)| *at)
        .next_back()
        .context("no raid omen of 600 ticks before the bar")?;
    let bar_events = scene.of(from, |packet| match packet {
        ClientboundPlay::BossEvent(event) => Some(event.clone()),
        _ => None,
    });
    let mut bars: Vec<Uuid> = bar_events.iter().map(|(_, event)| event.id).collect();
    bars.dedup();
    let horns = scene
        .of(from, |packet| match packet {
            ClientboundPlay::Sound(sound) if sound.sound == sounds::EVENT_RAID_HORN => {
                Some((sound.sound, sound.source, sound.volume, sound.pitch))
            }
            _ => None,
        })
        .into_iter()
        .map(|(_, horn)| horn)
        .collect();
    Ok(Raided {
        effects: effects
            .into_iter()
            .filter(|(at, _)| *at <= added)
            .map(|(_, effect)| effect)
            .collect(),
        bar: bar_events
            .into_iter()
            .map(|(_, event)| event.operation)
            .collect(),
        bars,
        horns,
        omen_to_bar: ticks(omen, added),
        bar_to_wave: ticks(added, first),
    })
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_raids_bar_counts_down_to_its_first_wave_as_on_the_official_server() {
    let (theirs, ours) = on_both_at(DAY, raid).await;
    let theirs = theirs.unwrap_or_else(|error| panic!("the official server: {error:#}"));
    let ours = ours.unwrap_or_else(|error| panic!("Clustine: {error:#}"));
    println!("official {theirs:?}\nClustine {ours:?}");
    for (name, raided) in [("the official server", &theirs), ("Clustine", &ours)] {
        assert_eq!(raided.bars.len(), 1, "{name}: one bar, {:?}", raided.bars);
    }
    let mut differ = Vec::new();
    if theirs.effects != ours.effects {
        differ.push("the effects");
    }
    if theirs.bar != ours.bar {
        differ.push("the bar's packets");
    }
    if theirs.horns != ours.horns {
        differ.push("the horns");
    }
    if (theirs.omen_to_bar - ours.omen_to_bar).abs() > APART {
        differ.push("the ticks from the omen to the bar");
    }
    if (theirs.bar_to_wave - ours.bar_to_wave).abs() > APART {
        differ.push("the ticks from the bar to the first raider");
    }
    assert!(differ.is_empty(), "these differ, printed above: {differ:?}");
}
