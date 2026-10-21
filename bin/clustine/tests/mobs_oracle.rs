//! Comparisons with the official server of what a client is shown of mobs (N7f,
//! `docs/groundwork/n7-plan.md`, section 7.10): a cow made by a spawn egg, a hit on it,
//! two cows fed that breed, and the time; and riding a pig (N7h). A builder does each beside the spawn of a flat
//! world and a watcher records what it is sent of the cows and orbs, and the sounds,
//! each at its milliseconds from the start of the part.
//!
//! A cow's yaw, its variant and sound variant, its wandering, the parent its calf is
//! made beside and the orb's worth are drawn at random on both servers, so the cases
//! hold both to what the rules fix whatever the draws: the packets of a spawn and their
//! order, the head facing the yaw, the health and the baby flag in the data, the
//! attributes, the hit's damage event, sounds and health, the hearts, and the calf and
//! orb about three seconds after; and the time every twenty ticks.
//!
//! They need Java, the server jar downloaded by `cargo datagen`, and your agreement to
//! the Minecraft EULA, so they are ignored by default. Run them on a quiet machine:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test mobs_oracle -- --ignored --nocapture --test-threads 1
//! ```

mod common;

use std::time::Duration;

use anyhow::{Context, Result};
use clustine_botswarm::{Bot, Connection, Oracle};
use clustine_data::{entity_types, items, sounds};
use clustine_protocol::item::ItemStack;
use clustine_protocol::packets::play::{
    ClientboundKeepAlive, ClientboundPlay, EntityDataValue, ServerboundKeepAlive, SetTime, face,
};
use tokio::time::{Instant, timeout};

use common::start;

const PATIENCE: Duration = Duration::from_secs(30);

/// The height a player stands at on both servers' flat worlds: on the grass at -61.
const GROUND: i32 = -61;

/// The movement speed's attribute: its id and a cow's base.
const MOVEMENT_SPEED: (i32, f64) = (26, 0.20000000298023224);

/// What the watcher was shown of a cow or an orb, or heard, as far as it is compared.
#[derive(Debug, Clone, PartialEq)]
enum Shown {
    /// `add_entity`: the type, and whether a cow's head was sent facing its yaw.
    Spawn {
        kind: i32,
        head_on_yaw: bool,
    },
    Data(Vec<(u8, EntityDataValue)>),
    /// `update_attributes`: each attribute's id and base.
    Attributes(Vec<(i32, f64)>),
    Event(i8),
    /// `damage_event`: whether the builder is its cause and its direct cause.
    Damage {
        by_the_builder: bool,
    },
    /// A sound at a place: its id.
    Sound(i32),
    Removed,
}

/// What the watcher was shown, by milliseconds from a part's start: the entity's wire
/// id (0 for a sound) and what.
type Seen = Vec<(u128, i32, Shown)>;

/// Reads what `watching` is sent for `how_long`, answering keep-alives, and keeps what
/// names a cow or an orb, and the sounds; `ours` are the cows and orbs seen so far, and
/// `builder` is the builder's wire id.
async fn record(
    watching: &mut Connection,
    ours: &mut Vec<i32>,
    builder: i32,
    how_long: Duration,
) -> Result<Seen> {
    let began = Instant::now();
    let deadline = began + how_long;
    let mut seen = Vec::new();
    loop {
        let Ok(frame) = timeout(
            deadline.saturating_duration_since(Instant::now()),
            watching.read_frame(),
        )
        .await
        else {
            break;
        };
        let frame = frame?;
        let ms = began.elapsed().as_millis();
        let (id, shown) = match ClientboundPlay::decode(&frame) {
            Ok(ClientboundPlay::ClientboundKeepAlive(ClientboundKeepAlive { id })) => {
                watching.write(&ServerboundKeepAlive { id }).await?;
                continue;
            }
            Ok(ClientboundPlay::SpawnEntity(spawn))
                if spawn.kind == entity_types::COW
                    || spawn.kind == entity_types::EXPERIENCE_ORB =>
            {
                ours.push(spawn.entity_id);
                let head_on_yaw = spawn.kind != entity_types::COW || spawn.head_yaw == spawn.yaw;
                let shown = Shown::Spawn {
                    kind: spawn.kind,
                    head_on_yaw,
                };
                (spawn.entity_id, shown)
            }
            Ok(ClientboundPlay::SetEntityData(data)) if ours.contains(&data.entity_id) => {
                (data.entity_id, Shown::Data(data.values))
            }
            Ok(ClientboundPlay::UpdateAttributes(attributes))
                if ours.contains(&attributes.entity_id) =>
            {
                let bases = attributes
                    .attributes
                    .iter()
                    .map(|attribute| (attribute.attribute, attribute.base))
                    .collect();
                (attributes.entity_id, Shown::Attributes(bases))
            }
            Ok(ClientboundPlay::EntityEvent(event)) if ours.contains(&event.entity_id) => {
                (event.entity_id, Shown::Event(event.event))
            }
            Ok(ClientboundPlay::DamageEvent(damage)) if ours.contains(&damage.entity_id) => {
                let by_the_builder =
                    damage.cause == Some(builder) && damage.direct == Some(builder);
                (damage.entity_id, Shown::Damage { by_the_builder })
            }
            Ok(ClientboundPlay::Sound(sound)) => (0, Shown::Sound(sound.sound)),
            Ok(ClientboundPlay::RemoveEntities(removed)) => {
                match removed.entity_ids.iter().find(|id| ours.contains(id)) {
                    Some(id) => (*id, Shown::Removed),
                    None => continue,
                }
            }
            _ => continue,
        };
        seen.push((ms, id, shown));
    }
    Ok(seen)
}

/// A builder and a watcher in `address`'s world, with the chunks around the builder.
async fn two_bots(address: &str) -> Result<(Bot, Bot)> {
    let mut builder = Bot::join(address, "Builder").await?;
    let mut watcher = Bot::join(address, "Watcher").await?;
    watcher
        .wait_until(PATIENCE, |bot| bot.seen_player("Builder").is_some())
        .await?;
    builder
        .wait_until(PATIENCE, |bot| {
            let x = (bot.location.0.floor() as i32).div_euclid(16);
            let z = (bot.location.2.floor() as i32).div_euclid(16);
            bot.seen_player("Watcher").is_some()
                && (-1..=1).all(|dx| (-1..=1).all(|dz| bot.chunks.contains_key(&(x + dx, z + dz))))
        })
        .await?;
    Ok((builder, watcher))
}

/// The cows the bot is shown, by their ids.
fn cows(bot: &Bot) -> Vec<i32> {
    bot.entities
        .values()
        .filter(|entity| entity.kind == entity_types::COW)
        .map(|entity| entity.entity_id)
        .collect()
}

/// Puts down a cow from an egg on the grass at `(x, z)` and waits until the bot is
/// shown one cow more.
async fn egg(bot: &mut Bot, x: i32, z: i32) -> Result<i32> {
    let before = cows(bot);
    let egg = ItemStack {
        item: items::COW_SPAWN_EGG,
        count: 1,
    };
    bot.put_in_hotbar(0, egg).await?;
    bot.select_slot(0).await?;
    bot.use_item_on(x, GROUND, z, face::TOP).await?;
    bot.wait_until(PATIENCE, |bot| cows(bot).len() > before.len())
        .await?;
    cows(bot)
        .into_iter()
        .find(|cow| !before.contains(cow))
        .context("a new cow")
}

/// What the watcher of a server was shown in each part.
struct Watched {
    made: Seen,
    hit: Seen,
    fed: Seen,
    /// The two cows fed, by their ids, which the watcher is shown them by too: both
    /// bots are shown the same entities, numbered from the same start.
    parents: [i32; 2],
}

/// The whole case on the server at `address`.
async fn watched(address: &str) -> Result<Watched> {
    let (mut builder, watcher) = two_bots(address).await?;
    let (x, z) = (
        builder.location.0.floor() as i32,
        builder.location.2.floor() as i32,
    );
    let builder_id = watcher
        .seen_player("Builder")
        .context("the watcher is shown the builder")?
        .entity_id;
    let mut watching = watcher.into_connection();
    let mut ours = Vec::new();
    let first = egg(&mut builder, x + 2, z).await?;
    let made = record(&mut watching, &mut ours, builder_id, Duration::from_secs(2)).await?;
    builder.attack(first).await?;
    let hit = record(&mut watching, &mut ours, builder_id, Duration::from_secs(2)).await?;
    // Two others, away from the one hit, which panics.
    let second = egg(&mut builder, x - 2, z + 2).await?;
    let third = egg(&mut builder, x - 2, z - 2).await?;
    let wheat = ItemStack {
        item: items::WHEAT,
        count: 1,
    };
    builder.put_in_hotbar(1, wheat).await?;
    builder.select_slot(1).await?;
    builder.interact(second, [0.0, 0.7, 0.0]).await?;
    builder.interact(third, [0.0, 0.7, 0.0]).await?;
    let fed = record(&mut watching, &mut ours, builder_id, Duration::from_secs(6)).await?;
    builder.idle(Duration::from_millis(10)).await?;
    Ok(Watched {
        made,
        hit,
        fed,
        parents: [second, third],
    })
}

/// Of `seen`, what was shown of `id`, with its milliseconds.
fn of(seen: &Seen, id: i32) -> Vec<(u128, Shown)> {
    seen.iter()
        .filter(|(_, entity, _)| *entity == id)
        .map(|(ms, _, shown)| (*ms, shown.clone()))
        .collect()
}

/// The first spawn in `seen` of the type `kind`: its id and when.
fn spawned(seen: &Seen, kind: i32) -> Option<(i32, u128)> {
    seen.iter().find_map(|(ms, id, shown)| match shown {
        Shown::Spawn { kind: of_kind, .. } if *of_kind == kind => Some((*id, *ms)),
        _ => None,
    })
}

/// The value at `index` of a data packet.
fn value(values: &[(u8, EntityDataValue)], index: u8) -> Option<&EntityDataValue> {
    values
        .iter()
        .find(|(at, _)| *at == index)
        .map(|(_, value)| value)
}

/// Whether `seen` heard `sound` before `before` ms.
fn heard(seen: &Seen, sound: i32, before: u128) -> bool {
    seen.iter()
        .any(|(ms, _, shown)| *ms < before && *shown == Shown::Sound(sound))
}

/// Checks what `server`'s watcher was shown of a cow made by an egg: its spawn with its
/// head on its yaw, its data with its health, the same data again by the first update,
/// and its movement speed within a second.
fn made_like_the_game(server: &str, seen: &Seen) {
    let (cow, _) = spawned(seen, entity_types::COW).unwrap_or_else(|| panic!("{server}: no cow"));
    let shown = of(seen, cow);
    assert!(
        matches!(
            shown.first(),
            Some((
                _,
                Shown::Spawn {
                    head_on_yaw: true,
                    ..
                }
            ))
        ),
        "{server}: {shown:?}"
    );
    let data: Vec<&Vec<(u8, EntityDataValue)>> = shown
        .iter()
        .filter_map(|(_, shown)| match shown {
            Shown::Data(values) => Some(values),
            _ => None,
        })
        .collect();
    assert!(
        data.len() == 2 && data[0] == data[1],
        "{server}: the data twice, alike: {shown:?}"
    );
    assert_eq!(
        value(data[0], 9),
        Some(&EntityDataValue::Float(10.0)),
        "{server}: its health"
    );
    assert!(
        shown
            .iter()
            .any(|(ms, shown)| *ms < 1000 && *shown == Shown::Attributes(vec![MOVEMENT_SPEED])),
        "{server}: its movement speed: {shown:?}"
    );
}

/// Checks what `server`'s watcher was shown of the hit, within a quarter of a second:
/// the damage by the builder, the cow's hurt sound and the strong attack's, and its
/// health down to 9.
fn hit_like_the_game(server: &str, made: &Seen, seen: &Seen) {
    let (cow, _) = spawned(made, entity_types::COW).expect("checked");
    let shown = of(seen, cow);
    let soon = |ms: &u128| *ms < 250;
    assert!(
        shown.iter().any(|(ms, shown)| soon(ms)
            && *shown
                == Shown::Damage {
                    by_the_builder: true
                }),
        "{server}: the damage: {shown:?}"
    );
    assert!(
        shown.iter().any(|(ms, shown)| soon(ms)
            && matches!(shown, Shown::Data(values)
                if value(values, 9) == Some(&EntityDataValue::Float(9.0)))),
        "{server}: its health 9: {shown:?}"
    );
    assert!(
        heard(seen, sounds::ENTITY_COW_HURT, 250)
            || heard(seen, sounds::ENTITY_COW_MOODY_HURT, 250),
        "{server}: the cow's hurt: {seen:?}"
    );
    assert!(
        heard(seen, sounds::ENTITY_PLAYER_ATTACK_STRONG, 250),
        "{server}: the strong attack: {seen:?}"
    );
}

/// Checks what `server`'s watcher was shown of the two cows fed: the hearts of each at
/// once; between two and a half and four seconds after, the hearts of one again, an orb
/// worth one to seven and a calf with its baby flag, its health and its movement speed.
fn bred_like_the_game(server: &str, watched: &Watched) {
    let seen = &watched.fed;
    for parent in watched.parents {
        assert!(
            of(seen, parent)
                .iter()
                .any(|(ms, shown)| *ms < 250 && *shown == Shown::Event(18)),
            "{server}: the hearts of {parent}: {seen:?}"
        );
    }
    let bred = 2500..4000;
    let again = watched.parents.iter().any(|parent| {
        of(seen, *parent)
            .iter()
            .any(|(ms, shown)| bred.contains(ms) && *shown == Shown::Event(18))
    });
    assert!(again, "{server}: the breeding parent's hearts: {seen:?}");
    let (orb, at) = spawned(seen, entity_types::EXPERIENCE_ORB)
        .unwrap_or_else(|| panic!("{server}: no orb: {seen:?}"));
    assert!(bred.contains(&at), "{server}: the orb at {at}");
    let worth = of(seen, orb)
        .into_iter()
        .find_map(|(_, shown)| match shown {
            Shown::Data(values) => value(&values, 8).cloned(),
            _ => None,
        });
    assert!(
        matches!(worth, Some(EntityDataValue::Int(worth)) if (1..=7).contains(&worth)),
        "{server}: the orb's worth {worth:?}"
    );
    let calf = seen.iter().find_map(|(ms, id, shown)| match shown {
        Shown::Spawn { kind, .. }
            if *kind == entity_types::COW && !watched.parents.contains(id) && bred.contains(ms) =>
        {
            Some(*id)
        }
        _ => None,
    });
    let calf = calf.unwrap_or_else(|| panic!("{server}: no calf: {seen:?}"));
    let shown = of(seen, calf);
    assert!(
        shown
            .iter()
            .any(|(_, shown)| matches!(shown, Shown::Data(values)
            if value(values, 16) == Some(&EntityDataValue::Boolean(true))
                && value(values, 9) == Some(&EntityDataValue::Float(10.0)))),
        "{server}: the calf's data: {shown:?}"
    );
    assert!(
        shown
            .iter()
            .any(|(_, shown)| *shown == Shown::Attributes(vec![MOVEMENT_SPEED])),
        "{server}: the calf's movement speed: {shown:?}"
    );
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn cows_made_hit_and_bred_are_shown_like_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let theirs = watched(oracle.address()).await.unwrap();
    drop(oracle);
    let (server, address) = start().await;
    let ours = watched(&address).await.unwrap();
    for (name, watched) in [("the official server", &theirs), ("Clustine", &ours)] {
        made_like_the_game(name, &watched.made);
        hit_like_the_game(name, &watched.made, &watched.hit);
        bred_like_the_game(name, watched);
    }
    server.stop().await;
}

/// What a newcomer is sent of the time in its first three and a half seconds.
async fn times(address: &str) -> Result<Vec<SetTime>> {
    let mut bot = Bot::join(address, "Timekeeper").await?;
    bot.idle(Duration::from_millis(3500)).await?;
    Ok(bot.times.clone())
}

/// The first time packet as a player joins, with the clocks as many as the official
/// server's, and then one every second, twenty ticks on from the one before.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn the_time_is_sent_like_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let theirs = times(oracle.address()).await.unwrap();
    drop(oracle);
    let (server, address) = start().await;
    let ours = times(&address).await.unwrap();
    println!("the official server: {theirs:?}\nClustine: {ours:?}");
    for (name, times) in [("the official server", &theirs), ("Clustine", &ours)] {
        assert!(times.len() >= 4, "{name}: {times:?}");
        for pair in times[1..].windows(2) {
            assert_eq!(
                pair[1].game_time - pair[0].game_time,
                20,
                "{name}: {times:?}"
            );
        }
    }
    // The first with every clock, the rest with none; how many came in the time depends
    // on where in its twenty ticks each joined.
    let clocks = |times: &[SetTime]| {
        let first: Vec<i32> = times[0].clocks.iter().map(|clock| clock.clock).collect();
        let rest_bare = times[1..].iter().all(|time| time.clocks.is_empty());
        (first, rest_bare)
    };
    assert_eq!(clocks(&ours), clocks(&theirs));
    server.stop().await;
}

/// What a watcher and the rider were shown of a pig ridden on one server: the
/// passengers of the pig as each `set_passengers` said, the boost's total at index 18 if
/// sent, and the rider's feet before and after getting off, with the pig's position.
#[derive(Debug)]
struct Rode {
    seated: Vec<Vec<i32>>,
    boost: Option<i32>,
    rider: i32,
    pig: (f64, f64, f64),
    off_at: (f64, f64, f64),
}

/// Reads what `watching` is sent for `how_long`, answering keep-alives, and keeps the
/// `set_passengers` of `pig` and its data's index 18.
async fn watch_pig(
    watching: &mut Connection,
    pig: i32,
    how_long: Duration,
    rode: &mut Rode,
) -> Result<()> {
    let deadline = Instant::now() + how_long;
    loop {
        let Ok(frame) = timeout(
            deadline.saturating_duration_since(Instant::now()),
            watching.read_frame(),
        )
        .await
        else {
            return Ok(());
        };
        match ClientboundPlay::decode(&frame?) {
            Ok(ClientboundPlay::ClientboundKeepAlive(ClientboundKeepAlive { id })) => {
                watching.write(&ServerboundKeepAlive { id }).await?;
            }
            Ok(ClientboundPlay::SetPassengers(packet)) if packet.vehicle_id == pig => {
                rode.seated.push(packet.passengers);
            }
            Ok(ClientboundPlay::SetEntityData(data)) if data.entity_id == pig => {
                if let Some(EntityDataValue::Int(total)) = value(&data.values, 18) {
                    rode.boost = Some(*total);
                }
            }
            _ => {}
        }
    }
}

/// The pig's case on the server at `address`: a pig from an egg beside the rider,
/// saddled, mounted with an empty hand, boosted with a carrot on a stick, and left by
/// sneaking; the watcher records it all.
async fn rode(address: &str) -> Result<Rode> {
    let (mut rider, watcher) = two_bots(address).await?;
    let (x, z) = (
        rider.location.0.floor() as i32,
        rider.location.2.floor() as i32,
    );
    let pigs = |bot: &Bot| -> Vec<i32> {
        bot.entities
            .values()
            .filter(|entity| entity.kind == entity_types::PIG)
            .map(|entity| entity.entity_id)
            .collect()
    };
    let rider_id = watcher
        .seen_player("Builder")
        .context("the watcher is shown the rider")?
        .entity_id;
    rider
        .put_in_hotbar(
            0,
            ItemStack {
                item: items::PIG_SPAWN_EGG,
                count: 1,
            },
        )
        .await?;
    rider.select_slot(0).await?;
    rider.use_item_on(x + 2, GROUND, z, face::TOP).await?;
    rider
        .wait_until(PATIENCE, |bot| pigs(bot).len() == 1)
        .await?;
    let pig = pigs(&rider)[0];
    let mut watching = watcher.into_connection();
    let mut rode = Rode {
        seated: Vec::new(),
        boost: None,
        rider: rider_id,
        pig: (0.0, 0.0, 0.0),
        off_at: (0.0, 0.0, 0.0),
    };
    rider
        .put_in_hotbar(
            1,
            ItemStack {
                item: items::SADDLE,
                count: 1,
            },
        )
        .await?;
    rider
        .put_in_hotbar(
            2,
            ItemStack {
                item: items::CARROT_ON_A_STICK,
                count: 1,
            },
        )
        .await?;
    rider.select_slot(1).await?;
    rider.interact(pig, [0.0, 0.5, 0.0]).await?;
    watch_pig(&mut watching, pig, Duration::from_millis(500), &mut rode).await?;
    // Slot 3 is empty: a hand with nothing in it mounts.
    rider.select_slot(3).await?;
    rider.interact(pig, [0.0, 0.5, 0.0]).await?;
    rider
        .wait_until(PATIENCE, |bot| bot.riding() == Some(pig))
        .await?;
    watch_pig(&mut watching, pig, Duration::from_millis(500), &mut rode).await?;
    rider.select_slot(2).await?;
    rider.use_item(0.0, 0.0).await?;
    watch_pig(&mut watching, pig, Duration::from_secs(1), &mut rode).await?;
    rider.sneak(true).await?;
    rider
        .wait_until(PATIENCE, |bot| bot.riding().is_none())
        .await?;
    rider.sneak(false).await?;
    watch_pig(&mut watching, pig, Duration::from_millis(500), &mut rode).await?;
    rider.idle(Duration::from_millis(300)).await?;
    rode.pig = rider
        .entities
        .get(&pig)
        .map(|entity| entity.position)
        .context("the rider is shown the pig")?;
    rode.off_at = rider.location;
    Ok(rode)
}

/// Riding a pig (`docs/groundwork/n7h-riding-plan.md`, section 8): the watcher is told
/// the rider sits on the pig once it is saddled and clicked with an empty hand, and that
/// nobody does after the rider sneaks; the stick's use sends the boost's total, drawn in
/// 140 to 980, at index 18; and the rider stands off the pig, on the ground, within two
/// blocks of it. Where on the ground depends on where the pig wandered, so it is held to
/// the rules, not to the other server's place.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_pig_ridden_boosted_and_left_is_shown_like_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let theirs = rode(oracle.address()).await.unwrap();
    drop(oracle);
    let (server, address) = start().await;
    let ours = rode(&address).await.unwrap();
    println!("the official server: {theirs:?}\nClustine: {ours:?}");
    for (name, rode) in [("the official server", &theirs), ("Clustine", &ours)] {
        assert_eq!(
            rode.seated,
            vec![vec![rode.rider], vec![]],
            "{name}: the pig's passengers"
        );
        let total = rode
            .boost
            .unwrap_or_else(|| panic!("{name}: no boost shown"));
        assert!((140..=980).contains(&total), "{name}: a boost of {total}");
        let (dx, dz) = (rode.off_at.0 - rode.pig.0, rode.off_at.2 - rode.pig.2);
        let apart = (dx * dx + dz * dz).sqrt();
        assert!(
            (0.5..2.0).contains(&apart),
            "{name}: off {apart} from the pig"
        );
        assert!(
            (rode.off_at.1 - f64::from(GROUND + 1)).abs() < 1e-6,
            "{name}: {rode:?}"
        );
    }
    server.stop().await;
}

/// A question for the design of a logout on a mount
/// (`docs/groundwork/n7h-logout-on-a-mount.md`, open question 1): whether the official
/// server takes a pig with its lone rider out of the world when the rider leaves, and
/// puts it back under them when they come back. Prints what a watcher and the rider are
/// shown; asserts nothing.
#[tokio::test]
#[ignore = "a probe of the official server; run it by name"]
async fn probe_a_rider_who_leaves_on_a_pig_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let address = oracle.address().to_owned();
    let (mut rider, mut watcher) = two_bots(&address).await.unwrap();
    let (x, z) = (
        rider.location.0.floor() as i32,
        rider.location.2.floor() as i32,
    );
    let pig_count = |bot: &Bot| {
        bot.entities
            .values()
            .filter(|entity| entity.kind == entity_types::PIG)
            .count()
    };
    let egg = ItemStack {
        item: items::PIG_SPAWN_EGG,
        count: 1,
    };
    rider.put_in_hotbar(0, egg).await.unwrap();
    rider.select_slot(0).await.unwrap();
    rider
        .use_item_on(x + 2, GROUND, z, face::TOP)
        .await
        .unwrap();
    rider
        .wait_until(PATIENCE, |bot| pig_count(bot) == 1)
        .await
        .unwrap();
    let pig = rider
        .entities
        .values()
        .find(|entity| entity.kind == entity_types::PIG)
        .unwrap()
        .entity_id;
    let saddle = ItemStack {
        item: items::SADDLE,
        count: 1,
    };
    rider.put_in_hotbar(1, saddle).await.unwrap();
    rider.select_slot(1).await.unwrap();
    rider.interact(pig, [0.0, 0.5, 0.0]).await.unwrap();
    rider.idle(Duration::from_millis(300)).await.unwrap();
    rider.select_slot(2).await.unwrap();
    rider.interact(pig, [0.0, 0.5, 0.0]).await.unwrap();
    let seated = rider
        .wait_until(Duration::from_secs(5), |bot| {
            bot.passengers
                .get(&pig)
                .is_some_and(|riders| riders.contains(&bot.info.login.entity_id))
        })
        .await;
    println!("seated: {seated:?}");
    rider.close().await.unwrap();
    drop(rider);
    watcher.idle(Duration::from_secs(2)).await.unwrap();
    println!(
        "while the rider is away the watcher is shown {} pig(s)",
        pig_count(&watcher)
    );
    let back = Bot::join(&address, "Builder").await.unwrap();
    let mut back = back;
    back.idle(Duration::from_secs(3)).await.unwrap();
    let me = back.info.login.entity_id;
    println!(
        "back: shown {} pig(s); passengers {:?}; own id {me}; seated on something: {}",
        pig_count(&back),
        back.passengers,
        back.passengers.values().any(|riders| riders.contains(&me))
    );
    watcher.idle(Duration::from_secs(1)).await.unwrap();
    println!("the watcher is shown {} pig(s) after", pig_count(&watcher));
}
