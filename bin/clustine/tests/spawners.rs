//! Monster spawners end to end (`region/spawners.rs`): a bot places a spawner, sets it
//! to zombies with an egg, closes it in a dark room of stone and stands by it; it is
//! shown the spawner's new mob, then zombies appearing in the room with the spawner's
//! smoke and the block event that starts its delay over. And the comparison with the
//! official server of a spawner's first spawns (ignored, as every comparison: it needs
//! Java, the jar and agreement to the Minecraft EULA).
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test spawners -- --ignored --nocapture --test-threads 1
//! ```

mod common;

use std::collections::BTreeSet;
use std::time::Duration;

use clustine_botswarm::{Bot, Heard, Oracle};
use clustine_data::{entity_types, items};
use clustine_protocol::nbt::Nbt;
use clustine_protocol::packets::play::face;

use common::start;

const PATIENCE: Duration = Duration::from_secs(60);

/// The flat world's grass, and the height of the feet of who stands on it.
const GRASS: i32 = -61;
const FEET: i32 = -60;

/// The spawner, away from where players enter (a world pinned at chunk 0 has a line
/// there) and within one region of a world pinned at chunks 0 and 4.
const SPAWNER: (i32, i32, i32) = (24, FEET, 0);

/// `LevelEvent.PARTICLES_MOBBLOCK_SPAWN` and the spawner's block event.
const SPAWN_SMOKE: i32 = 2004;
const EVENT_SPAWN: u8 = 1;

/// What a run showed of the spawner.
#[derive(Debug)]
struct Ran {
    /// The type the spawner's block entity update named, after the egg.
    shown_type: Option<String>,
    /// The types of the entities that appeared in the room by the first spawns.
    kinds: BTreeSet<i32>,
    /// How many zombies appeared in the room by then.
    zombies: usize,
    /// How many times the spawner's smoke was shown by then.
    smokes: usize,
    /// The spawner's block events by then: their action, their parameter and block.
    block_events: Vec<(u8, u8, i32)>,
}

fn in_room(position: (f64, f64, f64)) -> bool {
    let (x, _, z) = position;
    (f64::from(SPAWNER.0 - 1)..f64::from(SPAWNER.0 + 2)).contains(&x)
        && (f64::from(SPAWNER.2 - 1)..f64::from(SPAWNER.2 + 2)).contains(&z)
}

/// The `id` of the spawn data's entity of a spawner's update tag.
fn shown_type(data: &Option<Nbt>) -> Option<String> {
    let entity = data.as_ref()?.get("SpawnData")?.get("entity")?;
    match entity.get("id")? {
        Nbt::String(id) => Some(id.clone()),
        _ => None,
    }
}

/// Places the held block against the side `side` of the block at `at`, and waits until
/// the bot is shown it at `placed`.
async fn place(bot: &mut Bot, at: (i32, i32, i32), side: u8, placed: (i32, i32, i32)) {
    bot.use_item_on(at.0, at.1, at.2, side).await.unwrap();
    bot.wait_until(PATIENCE, |bot| {
        bot.block_at(placed.0, placed.1, placed.2)
            .is_ok_and(|state| state.is_some_and(|state| state != 0))
    })
    .await
    .unwrap_or_else(|error| panic!("{error}: nothing was placed at {placed:?}"));
}

async fn run(address: &str) -> Ran {
    let mut bot = Bot::join(address, "Keeper").await.unwrap();
    bot.wait_for_chunks(9, PATIENCE).await.unwrap();
    // West of the room's wall, within reach of all of it.
    let (sx, y, sz) = SPAWNER;
    bot.walk_to(f64::from(sx - 3) + 0.5, f64::from(sz) + 0.5, 0.6)
        .await
        .unwrap();
    bot.wait_for_chunks(9, PATIENCE).await.unwrap();
    bot.heard = Some(Vec::new());
    // The spawner, set to zombies by an egg: its clients are shown its new mob.
    bot.take_from_creative_inventory(0, items::SPAWNER)
        .await
        .unwrap();
    bot.select_slot(0).await.unwrap();
    place(&mut bot, (sx, GRASS, sz), face::TOP, SPAWNER).await;
    bot.take_from_creative_inventory(1, items::ZOMBIE_SPAWN_EGG)
        .await
        .unwrap();
    bot.select_slot(1).await.unwrap();
    bot.use_item_on(sx, y, sz, face::TOP).await.unwrap();
    bot.wait_until(PATIENCE, |bot| {
        bot.block_entity_updates.iter().any(|update| {
            (update.position.x, update.position.y, update.position.z) == SPAWNER
                && shown_type(&update.data).is_some()
        })
    })
    .await
    .unwrap_or_else(|error| panic!("{error}: the spawner's new mob was not shown"));
    let shown = bot
        .block_entity_updates
        .iter()
        .rev()
        .find(|update| (update.position.x, update.position.y, update.position.z) == SPAWNER)
        .and_then(|update| shown_type(&update.data));
    // A room of stone about it, three blocks wide and two high inside, and its roof:
    // dark inside, where a zombie may spawn.
    bot.take_from_creative_inventory(2, items::STONE)
        .await
        .unwrap();
    bot.select_slot(2).await.unwrap();
    let ring: Vec<(i32, i32)> = (sx - 2..=sx + 2)
        .flat_map(|x| (sz - 2..=sz + 2).map(move |z| (x, z)))
        .filter(|(x, z)| (x - sx).abs() == 2 || (z - sz).abs() == 2)
        .collect();
    for level in [GRASS, FEET, FEET + 1] {
        for &(x, z) in &ring {
            place(&mut bot, (x, level, z), face::TOP, (x, level + 1, z)).await;
        }
    }
    for x in sx - 1..=sx + 1 {
        for z in sz - 1..=sz + 1 {
            place(&mut bot, (x - 1, FEET + 2, z), face::EAST, (x, FEET + 2, z)).await;
        }
    }
    let before: BTreeSet<i32> = bot.entities.keys().copied().collect();
    let new_in_room = |bot: &Bot| {
        bot.entities
            .values()
            .filter(|entity| !before.contains(&entity.entity_id) && in_room(entity.position))
            .map(|entity| entity.kind)
            .collect::<Vec<_>>()
    };
    bot.wait_until(PATIENCE, |bot| {
        new_in_room(bot).contains(&entity_types::ZOMBIE)
    })
    .await
    .unwrap_or_else(|error| panic!("{error}: no zombie appeared by the spawner"));
    // The rest of the first spawns, all in the tick of the first, and the block event
    // that comes with the next tick's.
    bot.idle(Duration::from_millis(500)).await.unwrap();
    let kinds: Vec<i32> = new_in_room(&bot);
    let heard = bot.heard.take().unwrap_or_default();
    let smokes = heard
        .iter()
        .filter(|(_, heard)| {
            matches!(heard, Heard::LevelEvent { event, at, data: 0 }
                if *event == SPAWN_SMOKE && *at == SPAWNER)
        })
        .count();
    let block_events = heard
        .iter()
        .filter_map(|(_, heard)| match heard {
            Heard::BlockEvent {
                at,
                action,
                param,
                block,
            } if *at == SPAWNER => Some((*action, *param, *block)),
            _ => None,
        })
        .collect();
    Ran {
        shown_type: shown,
        zombies: kinds
            .iter()
            .filter(|kind| **kind == entity_types::ZOMBIE)
            .count(),
        kinds: kinds.into_iter().collect(),
        smokes,
        block_events,
    }
}

/// The spawner block's id in the block registry.
fn spawner_block() -> i32 {
    let id = clustine_data::blocks::SPAWNER
        .block_id()
        .expect("the spawner is a block");
    i32::try_from(id).unwrap()
}

#[tokio::test]
async fn a_bot_by_a_zombie_spawner_in_the_dark_sees_zombies_appear() {
    let (_server, address) = start().await;
    let ran = run(&address).await;
    println!("{ran:?}");
    assert_eq!(
        ran.shown_type.as_deref(),
        Some("minecraft:zombie"),
        "{ran:?}"
    );
    assert!((1..=4).contains(&ran.zombies), "{ran:?}");
    // A smoke for each mob made, and the block event that starts the delay over.
    assert_eq!(ran.smokes, ran.zombies, "{ran:?}");
    assert!(
        ran.block_events
            .contains(&(EVENT_SPAWN, 0, spawner_block())),
        "{ran:?}"
    );
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_spawner_s_first_spawns_are_shown_as_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let (server, address) = start().await;
    let theirs = run(oracle.address()).await;
    let ours = run(&address).await;
    server.stop().await;
    drop(oracle);
    println!("official: {theirs:?}\nClustine: {ours:?}");
    assert_eq!(theirs.shown_type, ours.shown_type);
    // What spawns is drawn: the same kinds, a zombie first, each with its smoke, as
    // many as four, and the same block event after them.
    assert!(theirs.kinds.contains(&entity_types::ZOMBIE));
    assert!(ours.kinds.contains(&entity_types::ZOMBIE));
    for ran in [&theirs, &ours] {
        assert!((1..=4).contains(&ran.zombies), "{ran:?}");
        assert_eq!(ran.smokes, ran.zombies, "{ran:?}");
    }
    let spawn_events =
        |ran: &Ran| -> BTreeSet<(u8, u8, i32)> { ran.block_events.iter().copied().collect() };
    assert_eq!(spawn_events(&theirs), spawn_events(&ours));
}
