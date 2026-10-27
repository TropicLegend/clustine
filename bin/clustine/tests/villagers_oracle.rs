//! Comparisons with the official server of a villager's day and of a zombie villager's
//! cure (`docs/groundwork/n10-plan.md`, section 14; `docs/groundwork/n10-brief.md`,
//! sections 3 and 6): a profession taken at a lectern placed beside an unemployed
//! villager, and its first work at the lectern when its working hours begin; a villager
//! lying down in its bed when its hours of rest begin; and a zombie villager's cure from
//! the golden apple to the villager it becomes.
//!
//! Each scene is made by what a creative player can do on both servers, its clock set
//! before it begins (`common/n10_village.rs`), since Clustine has no commands: the plan's
//! `/time set` in the middle of a scene and its zombie villager with a fixed
//! `ConversionTime` cannot be made on Clustine.
//!
//! **What a client can tell of a time.** A villager's activity follows its schedule
//! (`VILLAGER_DEFAULT`: work from 2000, rest from 12000) as its brain reads it, and a
//! client sees the activity only by what the villager then does: the work sound when
//! `WorkAtPoi` starts (after a level `nextInt(2) == 0`, drawn each tick it is tried:
//! finding 22), the sleeping pose when `SleepInBed` starts, a step or two from the bed.
//! So each switch is held to a window after its hour on both servers, [`WINDOW`] ticks
//! of a client's clock, and to [`APART`] ticks of each other's; the day time of each is
//! printed.
//!
//! **What is compared exactly.** The villager's entity events and its villager data as
//! they were sent, in order, from the job site's or the bed's placing to the profession
//! or the sleep (`AssignProfessionFromJobSite`: event 14 and the profession; a claimed
//! bed's event 14, `AcquirePoi`); the sleeping position, relative to the pit. Of the
//! cure: the zombie's events from the apple on (16, the client's cure sound), its
//! converting flag, the order of the end's packets (level event 1027, the zombie's
//! removal, the villager's add), the level event's place relative to the zombie, and
//! the villager's type and level. Left out, each because it is drawn: the cure's length
//! (`3600 + nextInt(2401)`; held to that range on each server instead), the zombie
//! villager's profession (drawn when it is made; the villager is held to keep its own
//! zombie's on each server), and when a point of interest is claimed (`AcquirePoi`'s
//! waits and retries).
//!
//! They need Java, the server jar downloaded by `cargo datagen`, and your agreement to
//! the Minecraft EULA, so they are ignored by default. Run them on a quiet machine:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test villagers_oracle -- --ignored official_server --nocapture \
//!     --test-threads 1 --no-fail-fast
//! ```

mod common;
#[path = "common/n10_village.rs"]
#[allow(dead_code)]
mod n10_village;
#[path = "common/n9_scene.rs"]
#[allow(dead_code)]
mod n9_scene;

use std::time::Duration;

use anyhow::{Context, Result, bail};
use clustine_data::villagers::{professions, types};
use clustine_data::{entity_types, items, sounds};
use clustine_protocol::components::{PotionContents, WireComponent, WireComponents, potion};
use clustine_protocol::item::ItemStack;
use clustine_protocol::packets::play::{ClientboundPlay, EntityDataValue, face, pose};

use n9_scene::{PATIENCE, Scene, ticks};
use n10_village::{
    Pit, data_of, day_time, events_of, hatch, interact, on_both_at, until_profession, until_seen,
    villager_data,
};

/// How many ticks of a client's clock after its hour a switch may be seen: the brain's
/// schedule, the behaviour's start (the work's coin flips) and the client's clock.
const WINDOW: i64 = 80;

/// How many ticks apart the two servers' switches may be seen.
const APART: i64 = 40;

/// When the working hours and the hours of rest begin (`VILLAGER_DEFAULT`).
const WORK: i64 = 2_000;
const REST: i64 = 12_000;

/// The duration of a tick.
const TICK: Duration = Duration::from_millis(50);

/// How long to wait for a switch at `hour` from the time of day `now`: until the hour,
/// and a minute more.
fn until_after(now: i64, hour: i64) -> Duration {
    let ticks = u32::try_from((hour - now).max(0)).unwrap_or(0);
    TICK * ticks + Duration::from_secs(60)
}

/// Whether `seen` is the work sound `sound`, at a place or at an entity.
fn is_sound(seen: &n9_scene::Seen, sound: i32) -> bool {
    match &seen.packet {
        Some(ClientboundPlay::Sound(heard)) => heard.sound == sound,
        Some(ClientboundPlay::SoundEntity(heard)) => heard.sound == sound,
        _ => false,
    }
}

/// The villager data `villager` was sent from `from` on, in order.
fn villager_data_since(scene: &Scene, from: usize, villager: i32) -> Vec<(i32, i32, i32)> {
    scene
        .since(from)
        .iter()
        .filter_map(|seen| villager_data(&data_of(seen, villager)?))
        .collect()
}

/// Checks that each server saw the switch at `hour` within [`WINDOW`] ticks after it,
/// and the two within [`APART`] of each other.
fn hold_to_the_hour(what: &str, hour: i64, theirs: i64, ours: i64) {
    println!("{what}: official at {theirs}, Clustine at {ours} (the hour {hour})");
    for (name, seen) in [("the official server", theirs), ("Clustine", ours)] {
        assert!(
            (hour..hour + WINDOW).contains(&seen),
            "{name}: {what} at {seen}, not within {WINDOW} ticks after {hour}"
        );
    }
    assert!(
        (theirs - ours).abs() <= APART,
        "{what}: {theirs} on the official server and {ours} on Clustine"
    );
}

// --- A profession at a lectern, and the work it begins at 2000 ---------------------

/// When the scene begins: early enough that the villager has its profession before its
/// working hours.
const MORNING: u64 = 200;

#[derive(Debug)]
struct Employed {
    /// The villager's entity events and data from the lectern's placing to the
    /// profession, in order.
    events: Vec<i8>,
    data: Vec<(i32, i32, i32)>,
    /// The times of day the profession was shown and the work sound heard.
    employed_at: i64,
    worked_at: i64,
}

async fn employ(address: &str) -> Result<Employed> {
    let mut scene = Scene::join(address, "Librarian").await?;
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
    let employed = until_profession(&mut scene, from, villager, professions::LIBRARIAN).await?;
    let employed_at = day_time(&scene, employed)?;
    let events = events_of(&scene, from, villager);
    let data = villager_data_since(&scene, from, villager);
    if employed_at >= WORK {
        bail!("the villager was a librarian only at {employed_at}, after its working hours began");
    }
    let worked = until_seen(
        &mut scene,
        from,
        until_after(employed_at, WORK),
        "the librarian's work sound",
        |seen| is_sound(seen, sounds::ENTITY_VILLAGER_WORK_LIBRARIAN),
    )
    .await?;
    Ok(Employed {
        events,
        data,
        employed_at,
        worked_at: day_time(&scene, worked)?,
    })
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_librarian_is_made_at_a_lectern_and_works_from_two_thousand_as_on_the_official_server() {
    let (theirs, ours) = on_both_at(MORNING, employ).await;
    let theirs = theirs.unwrap_or_else(|error| panic!("the official server: {error:#}"));
    let ours = ours.unwrap_or_else(|error| panic!("Clustine: {error:#}"));
    println!("official {theirs:?}\nClustine {ours:?}");
    println!(
        "a librarian at {} on the official server, at {} on Clustine",
        theirs.employed_at, ours.employed_at
    );
    assert_eq!(
        theirs.data.last(),
        Some(&(types::PLAINS, professions::LIBRARIAN, 1)),
        "the official server's librarian"
    );
    assert_eq!(
        (&theirs.events, &theirs.data),
        (&ours.events, &ours.data),
        "the events and the villager data from the lectern to the profession"
    );
    hold_to_the_hour(
        "the first work sound",
        WORK,
        theirs.worked_at,
        ours.worked_at,
    );
}

// --- A bed, and the rest that begins at 12000 --------------------------------------

/// When the scene begins: long enough before the hours of rest for the villager to
/// claim the bed.
const EVENING: u64 = 11_000;

#[derive(Debug)]
struct Rested {
    /// The villager's entity events from its hatching to its sleep, in order.
    events: Vec<i8>,
    /// Where it sleeps, relative to the pit's first cell.
    sleeping_at: Option<(i32, i32, i32)>,
    /// The time of day it was shown lying down.
    slept_at: i64,
}

async fn rest(address: &str) -> Result<Rested> {
    let mut scene = Scene::join(address, "Sleeper").await?;
    scene.empty_hotbar().await?;
    // The villager's cell, then the bed's foot and its head to the south.
    let pit = Pit::beside(&scene, 3);
    pit.dig(&mut scene).await?;
    // A bed's head goes the way its placer faces.
    scene.turn(0.0, 60.0).await?;
    scene
        .place(items::RED_BED, pit.floor(1), face::TOP, pit.feet(1))
        .await?;
    let from = scene.mark();
    let villager = hatch(
        &mut scene,
        items::VILLAGER_SPAWN_EGG,
        entity_types::VILLAGER,
        pit.floor(0),
    )
    .await?;
    let now = day_time(&scene, scene.log.last().context("a packet")?.at)?;
    let lying = |values: &[(u8, EntityDataValue)]| {
        values
            .iter()
            .any(|(_, value)| *value == EntityDataValue::Pose(pose::SLEEPING))
    };
    let slept = until_seen(
        &mut scene,
        from,
        until_after(now, REST),
        "the villager lying down",
        |seen| data_of(seen, villager).is_some_and(|values| lying(&values)),
    )
    .await?;
    let corner = pit.feet(0);
    let sleeping_at = scene
        .since(from)
        .iter()
        .filter_map(|seen| data_of(seen, villager))
        .flatten()
        .find_map(|(_, value)| match value {
            EntityDataValue::OptionalBlockPos(Some(at)) => {
                Some((at.x - corner.0, at.y - corner.1, at.z - corner.2))
            }
            _ => None,
        });
    Ok(Rested {
        events: events_of(&scene, from, villager),
        sleeping_at,
        slept_at: day_time(&scene, slept)?,
    })
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_villager_lies_down_in_its_bed_at_twelve_thousand_as_on_the_official_server() {
    let (theirs, ours) = on_both_at(EVENING, rest).await;
    let theirs = theirs.unwrap_or_else(|error| panic!("the official server: {error:#}"));
    let ours = ours.unwrap_or_else(|error| panic!("Clustine: {error:#}"));
    println!("official {theirs:?}\nClustine {ours:?}");
    // The head, two cells south of the villager's.
    assert_eq!(
        theirs.sleeping_at,
        Some((0, 0, 2)),
        "where the official server's villager sleeps"
    );
    assert_eq!(
        (&theirs.events, theirs.sleeping_at),
        (&ours.events, ours.sleeping_at),
        "the events and the sleeping position"
    );
    hold_to_the_hour("lying down", REST, theirs.slept_at, ours.slept_at);
}

// --- A zombie villager's cure --------------------------------------------------------

/// When the scene begins: at nightfall, so that the zombie villager does not burn in
/// the five minutes its cure may take.
const NIGHT: u64 = 13_000;

/// The index of a zombie villager's converting flag (`DATA_CONVERTING_ID`).
const CONVERTING: u8 = 19;

/// The shortest and longest cure, in ticks (`3600 + nextInt(2401)`, with no iron bars
/// or beds about to speed it up).
const SHORTEST_CURE: i64 = 3_600;
const LONGEST_CURE: i64 = 6_000;

/// How long the cure may take, with room.
const CURE_PATIENCE: Duration = Duration::from_secs(LONGEST_CURE as u64 / 20 + 60);

#[derive(Debug)]
struct Cured {
    /// The zombie's entity events and its converting flag as sent, from the apple on.
    events: Vec<i8>,
    converting: Vec<bool>,
    /// The cure's ticks, from the zombie's event 16 to its removal.
    lasted: i64,
    /// The end's packets in the order they came.
    end: Vec<&'static str>,
    /// The level event 1027: where, relative to the zombie's block, and its data.
    level_event: Option<((i32, i32, i32), i32)>,
    /// How far the villager was put from where the zombie was last shown.
    moved: f64,
    /// The villager's data, and whether it is its zombie's with the level kept.
    data: Option<(i32, i32, i32)>,
    kept: bool,
}

fn splash_of_weakness() -> ItemStack {
    let weakness = potion::NAMES
        .iter()
        .position(|name| *name == "minecraft:weakness")
        .expect("weakness among the potions");
    ItemStack {
        item: items::SPLASH_POTION,
        count: 1,
        components: WireComponents {
            added: vec![WireComponent::PotionContents(PotionContents {
                potion: Some(i32::try_from(weakness).expect("a registry is small")),
                ..PotionContents::default()
            })],
            removed: Vec::new(),
        },
    }
}

async fn cure(address: &str) -> Result<Cured> {
    let mut scene = Scene::join(address, "Curer").await?;
    scene.empty_hotbar().await?;
    let pit = Pit::beside(&scene, 1);
    pit.dig(&mut scene).await?;
    let zombie = hatch(
        &mut scene,
        items::ZOMBIE_VILLAGER_SPAWN_EGG,
        entity_types::ZOMBIE_VILLAGER,
        pit.floor(0),
    )
    .await?;

    // Weakness, thrown straight down onto it from above the pit.
    let from = scene.mark();
    scene.take(splash_of_weakness()).await?;
    scene.use_item(0.0, 90.0).await?;
    let thrown = scene
        .wait_added(from, entity_types::SPLASH_POTION, 1, PATIENCE)
        .await?;
    let potion = thrown.first().context("the potion")?.1.entity_id;
    until_seen(&mut scene, from, PATIENCE, "the potion's splash", |seen| {
        matches!(&seen.packet, Some(ClientboundPlay::RemoveEntities(removed))
            if removed.entity_ids.contains(&potion))
    })
    .await?;
    scene.round_trip().await?;

    let from = scene.mark();
    scene.take(ItemStack::new(items::GOLDEN_APPLE, 1)).await?;
    interact(&mut scene, zombie).await?;
    let begun = until_seen(&mut scene, from, PATIENCE, "the cure begun", |seen| {
        matches!(&seen.packet, Some(ClientboundPlay::EntityEvent(event))
            if event.entity_id == zombie && event.event == 16)
    })
    .await?;
    let ended = until_seen(&mut scene, from, CURE_PATIENCE, "the cure's end", |seen| {
        matches!(&seen.packet, Some(ClientboundPlay::RemoveEntities(removed))
            if removed.entity_ids.contains(&zombie))
    })
    .await?;
    let made = scene
        .wait_added(from, entity_types::VILLAGER, 1, PATIENCE)
        .await?;
    let (_, villager) = made.into_iter().next().context("the villager")?;
    scene.round_trip().await?;

    let track = scene.track(zombie);
    let last = track.places.last().context("the zombie's place")?.1;
    let block = (
        last[0].floor() as i32,
        last[1].floor() as i32,
        last[2].floor() as i32,
    );
    let mut end = Vec::new();
    let mut level_event = None;
    for seen in scene.since(from) {
        match &seen.packet {
            Some(ClientboundPlay::LevelEvent(event)) if event.event == 1027 => {
                end.push("level event 1027");
                let at = event.position;
                level_event = Some(((at.x - block.0, at.y - block.1, at.z - block.2), event.data));
            }
            Some(ClientboundPlay::RemoveEntities(removed))
                if removed.entity_ids.contains(&zombie) =>
            {
                end.push("the zombie removed");
            }
            Some(ClientboundPlay::SpawnEntity(spawn)) if spawn.entity_id == villager.entity_id => {
                end.push("the villager added");
            }
            _ => {}
        }
    }
    let zombie_data = scene
        .log
        .iter()
        .filter_map(|seen| villager_data(&data_of(seen, zombie)?))
        .next_back()
        .unwrap_or((types::PLAINS, professions::NONE, 1));
    let data = scene
        .since(from)
        .iter()
        .filter_map(|seen| villager_data(&data_of(seen, villager.entity_id)?))
        .next_back();
    let converting = scene
        .since(from)
        .iter()
        .filter_map(|seen| data_of(seen, zombie))
        .flatten()
        .filter_map(|(index, value)| match value {
            EntityDataValue::Boolean(flag) if index == CONVERTING => Some(flag),
            _ => None,
        })
        .collect();
    let moved = ((villager.x - last[0]).powi(2)
        + (villager.y - last[1]).powi(2)
        + (villager.z - last[2]).powi(2))
    .sqrt();
    Ok(Cured {
        events: events_of(&scene, from, zombie),
        converting,
        lasted: ticks(begun, ended),
        end,
        level_event,
        moved,
        data,
        kept: data == Some(zombie_data),
    })
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_zombie_villagers_cure_ends_in_its_villager_as_on_the_official_server() {
    let (theirs, ours) = on_both_at(NIGHT, cure).await;
    let theirs = theirs.unwrap_or_else(|error| panic!("the official server: {error:#}"));
    let ours = ours.unwrap_or_else(|error| panic!("Clustine: {error:#}"));
    println!("official {theirs:?}\nClustine {ours:?}");
    for (name, cured) in [("the official server", &theirs), ("Clustine", &ours)] {
        // A client's clock, a tick either way over the whole cure.
        assert!(
            (SHORTEST_CURE - 40..=LONGEST_CURE + 40).contains(&cured.lasted),
            "{name}: a cure of {} ticks",
            cured.lasted
        );
        assert!(
            cured.moved < 0.01,
            "{name}: the villager put {} from its zombie",
            cured.moved
        );
        assert!(
            cured.kept,
            "{name}: the villager's data {:?} is not its zombie's",
            cured.data
        );
    }
    assert_eq!(
        (
            &theirs.events,
            &theirs.converting,
            &theirs.end,
            theirs.level_event
        ),
        (&ours.events, &ours.converting, &ours.end, ours.level_event),
        "the cure's events, flag, end and level event"
    );
    assert_eq!(
        theirs.data.map(|(kind, _, level)| (kind, level)),
        ours.data.map(|(kind, _, level)| (kind, level)),
        "the villager's type and level"
    );
}
