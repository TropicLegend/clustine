//! Mobs on pressure plates end to end: a pig let out of a spawn egg onto a wooden plate,
//! and a zombie onto a stone one, each in a pen that keeps it there, press the plate,
//! and the lamp beside it lights and stays lit for as long as the mob stands there; the
//! plate's looks again, every twenty ticks, find the mob and leave it down, with no
//! click and no flicker. And the comparison with the official server of what a client is
//! sent of it, tick by tick (ignored, as every comparison: it needs Java, the jar and
//! agreement to the Minecraft EULA).
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test mobs_on_plates -- --ignored --nocapture --test-threads 1
//! ```
//!
//! What is compared is what the bot is sent of the plate and the lamp (their states) and
//! the plates' clicks, from the egg's use until three of the plate's looks again have
//! passed, each at its tick counted from the first, within a tick of the other server's.
//! The mob's own sounds and moves, which both servers draw at random, are not compared.

mod common;

use std::time::Duration;

use tokio::time::Instant;

use clustine_botswarm::{Bot, Heard, Oracle};
use clustine_data::{BlockState, entity_types, items, sounds};
use clustine_protocol::packets::play::face;

use common::start;

const PATIENCE: Duration = Duration::from_secs(30);

/// A server tick, by which arrivals are counted.
const TICK: f64 = 0.05;

/// How many of the server's tellings of the time (one every 20 ticks) the bot watches
/// for after the plate went down: more than three of the plate's looks again.
const TELLINGS: usize = 4;

/// A case: the spawn egg, the mob's type, and the plate.
#[derive(Debug, Clone, Copy)]
struct Case {
    egg: i32,
    kind: i32,
    plate: i32,
    name: &'static str,
}

const CASES: [Case; 2] = [
    Case {
        egg: items::PIG_SPAWN_EGG,
        kind: entity_types::PIG,
        plate: items::OAK_PRESSURE_PLATE,
        name: "oak_pressure_plate",
    },
    Case {
        egg: items::ZOMBIE_SPAWN_EGG,
        kind: entity_types::ZOMBIE,
        plate: items::STONE_PRESSURE_PLATE,
        name: "stone_pressure_plate",
    },
];

/// Where the pens are: clear of where players join on either server, within the chunks
/// at x 0 to 3 (one region where `CLUSTINE_TEST_PINS=0,4` divides the world), ten blocks
/// apart.
const FIRST_X: i32 = 20;
const Z: i32 = 20;
const SPACING: i32 = 10;

/// One thing the bot was sent of a pen, at its tick from the first.
#[derive(Debug, Clone, PartialEq)]
enum Thing {
    /// The plate's state, by its name.
    Plate(String),
    /// Whether the lamp is lit.
    Lamp(bool),
    /// A plate's click, by its sound's number.
    Click(i32),
}

type Timeline = Vec<(i64, Thing)>;

fn state(text: &str) -> i32 {
    let state = BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a state"));
    i32::from(state.0)
}

fn named(id: i32) -> String {
    let Some(block) = u16::try_from(id).ok().map(BlockState) else {
        return format!("the state {id}");
    };
    let name = block.block().map_or("?", |block| block.name);
    let properties: Vec<String> = block
        .properties()
        .map(|(property, value)| format!("{property}={value}"))
        .collect();
    format!("{name}[{}]", properties.join(","))
}

const CLICKS: [i32; 4] = [
    sounds::BLOCK_WOODEN_PRESSURE_PLATE_CLICK_ON,
    sounds::BLOCK_WOODEN_PRESSURE_PLATE_CLICK_OFF,
    sounds::BLOCK_STONE_PRESSURE_PLATE_CLICK_ON,
    sounds::BLOCK_STONE_PRESSURE_PLATE_CLICK_OFF,
];

/// Places `item` with a click on the `side` of the block `at`, and waits until the bot
/// is shown `into` changed.
async fn put(bot: &mut Bot, item: i32, at: (i32, i32, i32), side: u8, into: (i32, i32, i32)) {
    bot.take_from_creative_inventory(0, item).await.unwrap();
    bot.select_slot(0).await.unwrap();
    let before = bot.block_at(into.0, into.1, into.2).unwrap();
    bot.use_item_on(at.0, at.1, at.2, side).await.unwrap();
    bot.wait_until(PATIENCE, |bot| {
        bot.block_at(into.0, into.1, into.2).unwrap() != before
    })
    .await
    .unwrap_or_else(|error| panic!("{error}: nothing was placed at {into:?}"));
}

/// Builds the pen of `case` with its plate at `(x, feet, z)`, lets the mob out of its egg
/// onto it, and returns what the bot was sent of the plate and the lamp from then until
/// [`TELLINGS`] tellings of the time after the plate went down.
async fn run_case(bot: &mut Bot, case: Case, x: i32, feet: i32) -> Timeline {
    let ground = feet - 1;
    bot.fly_to(
        (f64::from(x) + 0.5, f64::from(feet), f64::from(Z) + 3.5),
        (180.0, 0.0),
        1.0,
    )
    .await
    .unwrap();
    bot.wait_until(PATIENCE, |bot| {
        bot.block_at(x, ground, Z)
            .is_ok_and(|state| state.is_some_and(|s| s != 0))
    })
    .await
    .unwrap();
    // The lamp east of the plate, stone on the other sides, and stone above all four, so
    // that the mob stays on the plate whatever its mind wants.
    let lamp = (x + 1, feet, Z);
    put(
        bot,
        items::REDSTONE_LAMP,
        (x + 1, ground, Z),
        face::TOP,
        lamp,
    )
    .await;
    for (dx, dz) in [(-1, 0), (0, -1), (0, 1)] {
        let wall = (x + dx, feet, Z + dz);
        put(bot, items::STONE, (wall.0, ground, wall.2), face::TOP, wall).await;
    }
    for (dx, dz) in [(1, 0), (-1, 0), (0, -1), (0, 1)] {
        let above = (x + dx, feet + 1, Z + dz);
        put(
            bot,
            items::STONE,
            (above.0, feet, above.2),
            face::TOP,
            above,
        )
        .await;
    }
    let plate = (x, feet, Z);
    put(bot, case.plate, (x, ground, Z), face::TOP, plate).await;
    let up = state(&format!("minecraft:{}[powered=false]", case.name));
    let down = state(&format!("minecraft:{}[powered=true]", case.name));
    assert_eq!(bot.block_at(x, feet, Z).unwrap(), Some(up));
    // A telling of the time, so that nothing the server sends of the plate's placing
    // comes after the record begins.
    let told = bot.times.len();
    bot.wait_until(PATIENCE, |bot| bot.times.len() > told)
        .await
        .unwrap();

    // The egg on the ground under the plate, which lets the mob out above it, onto the
    // plate.
    bot.record_heard();
    if let Some(heard) = bot.heard.as_mut() {
        heard.clear();
    }
    let began = Instant::now();
    bot.take_from_creative_inventory(0, case.egg).await.unwrap();
    bot.select_slot(0).await.unwrap();
    bot.use_item_on(x, ground, Z, face::TOP).await.unwrap();
    bot.wait_until(PATIENCE, |bot| {
        bot.entities.values().any(|entity| entity.kind == case.kind)
    })
    .await
    .unwrap_or_else(|error| panic!("{error}: no mob came of the egg"));
    let lit = state("minecraft:redstone_lamp[lit=true]");
    bot.wait_until(PATIENCE, |bot| {
        bot.block_at(x, feet, Z).unwrap() == Some(down)
            && bot.block_at(lamp.0, lamp.1, lamp.2).unwrap() == Some(lit)
    })
    .await
    .unwrap_or_else(|error| panic!("{error}: the plate did not go down under the mob"));
    let told = bot.times.len();
    bot.wait_until(PATIENCE * 2, |bot| bot.times.len() >= told + TELLINGS)
        .await
        .unwrap();

    let heard = bot.heard.take().unwrap_or_default();
    let mut things: Vec<(Instant, Thing)> = Vec::new();
    for (when, heard) in heard {
        if when < began {
            continue;
        }
        match heard {
            Heard::Blocks { blocks, .. } => {
                for (at, id) in blocks {
                    if at == plate {
                        things.push((when, Thing::Plate(named(id))));
                    } else if at == lamp {
                        things.push((when, Thing::Lamp(id == lit)));
                    }
                }
            }
            Heard::Sound { sound, at, .. } if CLICKS.contains(&sound) => {
                let at = (at.0 >> 3, at.1 >> 3, at.2 >> 3);
                if at == plate {
                    things.push((when, Thing::Click(sound)));
                }
            }
            _ => {}
        }
    }
    let Some(&(first, _)) = things.first() else {
        return Vec::new();
    };
    things
        .into_iter()
        .map(|(when, thing)| {
            let tick = ((when - first).as_secs_f64() / TICK).round() as i64;
            (tick, thing)
        })
        .collect()
}

/// Runs every case on the server at `address`, and returns their timelines and whether
/// each plate and lamp were still down and lit at the end.
async fn run(address: &str) -> Vec<(Timeline, bool)> {
    let mut bot = Bot::join(address, "Shepherd").await.unwrap();
    bot.wait_until(PATIENCE, Bot::is_loaded).await.unwrap();
    bot.set_flying(true).await.unwrap();
    let feet = bot.location.1.floor() as i32;
    let mut ran = Vec::new();
    for (index, case) in CASES.into_iter().enumerate() {
        let x = FIRST_X + SPACING * index as i32;
        let timeline = run_case(&mut bot, case, x, feet).await;
        let held = bot.block_at(x, feet, Z).unwrap()
            == Some(state(&format!("minecraft:{}[powered=true]", case.name)))
            && bot.block_at(x + 1, feet, Z).unwrap()
                == Some(state("minecraft:redstone_lamp[lit=true]"));
        ran.push((timeline, held));
    }
    ran
}

#[tokio::test]
async fn a_pig_and_a_zombie_keep_their_plates_down_and_the_lamps_lit_while_they_stand_there() {
    let (_server, address) = start().await;
    for ((timeline, held), case) in run(&address).await.into_iter().zip(CASES) {
        assert!(
            held,
            "{}: the plate or the lamp let go: {timeline:?}",
            case.name
        );
        let on = if case.name.starts_with("stone") {
            sounds::BLOCK_STONE_PRESSURE_PLATE_CLICK_ON
        } else {
            sounds::BLOCK_WOODEN_PRESSURE_PLATE_CLICK_ON
        };
        // As the official server sends it: in the tick of the egg's use, the plate as it
        // was (the use's update of the block above the one clicked, which
        // `handleUseItemOn` sends before the mob ticks) and the click of the mob's press;
        // in the next, the plate down and the lamp lit, with the changes of that tick
        // (`ChunkHolder.broadcastChanges`). Then nothing more of them however many times
        // the plate looked again: no click off, no plate up, nothing sent twice.
        let tick_of = |wanted: &Thing| {
            timeline
                .iter()
                .find(|(_, thing)| thing == wanted)
                .map(|(tick, _)| *tick)
        };
        let up = Thing::Plate(named(state(&format!(
            "minecraft:{}[powered=false]",
            case.name
        ))));
        let down = Thing::Plate(named(state(&format!(
            "minecraft:{}[powered=true]",
            case.name
        ))));
        let mut things: Vec<Thing> = timeline.iter().map(|(_, thing)| thing.clone()).collect();
        things.sort_by_key(|thing| format!("{thing:?}"));
        let mut expected = vec![
            up.clone(),
            Thing::Click(on),
            down.clone(),
            Thing::Lamp(true),
        ];
        expected.sort_by_key(|thing| format!("{thing:?}"));
        assert_eq!(things, expected, "{}: {timeline:?}", case.name);
        assert_eq!(tick_of(&up), Some(0), "{}: {timeline:?}", case.name);
        assert_eq!(
            tick_of(&Thing::Click(on)),
            Some(0),
            "{}: {timeline:?}",
            case.name
        );
        assert_eq!(tick_of(&down), Some(1), "{}: {timeline:?}", case.name);
        assert_eq!(
            tick_of(&Thing::Lamp(true)),
            Some(1),
            "{}: {timeline:?}",
            case.name
        );
    }
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn mobs_on_plates_are_sent_as_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let (server, address) = start().await;
    let theirs = run(oracle.address()).await;
    let ours = run(&address).await;
    server.stop().await;
    drop(oracle);
    let mut differences = Vec::new();
    for (case, ((their, their_held), (our, our_held))) in
        CASES.iter().zip(theirs.iter().zip(ours.iter()))
    {
        println!("{}:\n  official: {their:?}\n  Clustine: {our:?}", case.name);
        if their_held != our_held {
            differences.push(format!(
                "{}: held at the end on the official server {their_held}, on Clustine \
                 {our_held}",
                case.name
            ));
        }
        // Named: within a tick the order is not compared. The official server sends the
        // use's update of the plate while it handles the click, before the mob ticks and
        // clicks the plate; Clustine's edge hears the region's reading of the clicked
        // blocks after the tick's events, so the click comes first there. Both are in
        // the same tick, and the client shows the same. Each timeline is matched by its
        // things, each within a tick.
        let sorted = |timeline: &Timeline| {
            let mut sorted = timeline.clone();
            sorted.sort_by_key(|(tick, thing)| (format!("{thing:?}"), *tick));
            sorted
        };
        let (their, our) = (sorted(their), sorted(our));
        let same = their.len() == our.len()
            && their
                .iter()
                .zip(&our)
                .all(|((t_tick, t), (o_tick, o))| t == o && (t_tick - o_tick).abs() <= 1);
        if !same {
            differences.push(format!("{}: the timelines differ", case.name));
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}
