//! Comparisons with the official server of N7k's golems, villagers and monsters
//! (`docs/groundwork/n7k-plan.md`, sections 6 and 10): a mob of each of the 21 types
//! made by its spawn egg; a zombie in the open by day catching fire; a creeper lit by
//! flint and steel exploding; and a snowball thrown at a villager. A builder does each
//! beside the spawn of a flat world and records what it is sent.
//!
//! **A summoned mob.** Each type's egg is used on the grass at its own spot about the
//! builder, and the spawn packet, the data sent with it, the attributes and what the
//! mob wears are compared between the servers, with what depends on draws the servers
//! make differently left out, each named below:
//!
//! - the yaw, drawn on both (`EntityType.spawn`): only that the head was sent facing it,
//!   as the egg turns both, and the pitch, 0;
//! - the left hand, drawn one time in twenty (`Mob.finalizeSpawn`): its bit of the mob's
//!   flags (index 15);
//! - for the zombie, the husk, the drowned and the zombie villager, the baby flag (one
//!   time in twenty, index 16), what they wear and hold (a zombie's sword, spear or
//!   shovel one time in a hundred; a drowned's trident, rod and shell), the zombie
//!   villager's profession (drawn from the registry; its type, by the biome, is
//!   compared), the reinforcements' base (drawn) and every attribute's modifiers (the
//!   knockback and follow range bonuses and a leader's health are drawn);
//! - for the pillager, the vindicator and the evoker, the head (a patrol leader's
//!   banner one time in seventeen);
//! - for every other type the modifiers' amounts (the follow range's random bonus),
//!   whose ids and operations are compared.
//!
//! A spider is ridden by a skeleton one time in a hundred; the skeleton is another
//! entity and is not compared. Everything else is compared exactly: the type, the
//! place the egg put the mob, relative to the clicked block, to the bit, the velocity,
//! the rest of the data (a villager's type by the biome, profession and level; the
//! snow golem's pumpkin; the illagers' celebrating), the attributes' bases, and what
//! the rest wear (the skeletons' bows, the pillager's crossbow, the vindicator's axe).
//!
//! **A zombie by day.** The flat world's sky is open and both servers start a new
//! world in the morning, when the game's `monsters_burn` holds and the sky is light
//! enough (the sky darkens by nothing from tick 0); the zombie draws `(1.0 - 0.4) * 2 /
//! 30` a tick to catch fire, so both are held to its fire flag within fifteen seconds
//! (missing it is one chance in some 10^8). The helmeted zombie of the plan's 6.3 is
//! left out: no egg's zombie wears one but by a draw, and neither server gives one a
//! helmet otherwise but by a command Clustine has not.
//!
//! **A creeper lit.** Its ignited flag (index 18), the primed sound, and its explosion
//! of radius 3 thirty ticks later: the ticks from the flag to the explosion are held
//! within one of each other's, which is what a client's clock can tell.
//!
//! **A snowball.** Thrown from the builder's eyes at a villager three blocks away: its
//! spawn (with its owner, the builder, as its data), its data, the throw's sound, its
//! entity event 3 and its removal at the hit, and the villager's damage event (caused
//! by the builder through the snowball) and the knockback's velocity, upward by the
//! same within a hundredth and outward by the same within five hundredths (the throw's
//! direction is drawn, a little).
//!
//! They need Java, the server jar downloaded by `cargo datagen`, and your agreement to
//! the Minecraft EULA, so they are ignored by default. Run them on a quiet machine:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test monsters_oracle -- --ignored --nocapture --test-threads 1
//! ```

mod common;

use std::time::Duration;

use anyhow::{Context, Result};
use clustine_botswarm::{Bot, Oracle};
use clustine_data::{attributes, entity_types, items, sounds};
use clustine_protocol::item::ItemStack;
use clustine_protocol::packets::play::{ClientboundPlay, EntityDataValue, face};
use tokio::time::Instant;

use common::shown::{FirstShown, first_shown};
use common::start;

const PATIENCE: Duration = Duration::from_secs(30);

/// The height a player stands at on both servers' flat worlds: on the grass at -61.
const GROUND: i32 = -61;

/// What of a type's making depends on draws the two servers make differently.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Drawn {
    /// Nothing beyond the yaw, the left hand and the modifiers' amounts.
    Little,
    /// The zombie's kin: the baby flag, what they wear, the reinforcements, the
    /// modifiers, a zombie villager's profession.
    ZombieKin,
    /// A raider: the head a patrol leader's banner is put on.
    Raider,
}

/// A type made by its egg.
struct Summoned {
    egg: i32,
    kind: i32,
    drawn: Drawn,
}

const fn summoned(egg: i32, kind: i32, drawn: Drawn) -> Summoned {
    Summoned { egg, kind, drawn }
}

/// N7k's 21 types.
const SUMMONED: [Summoned; 21] = [
    summoned(
        items::IRON_GOLEM_SPAWN_EGG,
        entity_types::IRON_GOLEM,
        Drawn::Little,
    ),
    summoned(
        items::SNOW_GOLEM_SPAWN_EGG,
        entity_types::SNOW_GOLEM,
        Drawn::Little,
    ),
    summoned(
        items::VILLAGER_SPAWN_EGG,
        entity_types::VILLAGER,
        Drawn::Little,
    ),
    summoned(
        items::WANDERING_TRADER_SPAWN_EGG,
        entity_types::WANDERING_TRADER,
        Drawn::Little,
    ),
    summoned(
        items::ZOMBIE_SPAWN_EGG,
        entity_types::ZOMBIE,
        Drawn::ZombieKin,
    ),
    summoned(items::HUSK_SPAWN_EGG, entity_types::HUSK, Drawn::ZombieKin),
    summoned(
        items::DROWNED_SPAWN_EGG,
        entity_types::DROWNED,
        Drawn::ZombieKin,
    ),
    summoned(
        items::ZOMBIE_VILLAGER_SPAWN_EGG,
        entity_types::ZOMBIE_VILLAGER,
        Drawn::ZombieKin,
    ),
    summoned(
        items::SKELETON_SPAWN_EGG,
        entity_types::SKELETON,
        Drawn::Little,
    ),
    summoned(items::STRAY_SPAWN_EGG, entity_types::STRAY, Drawn::Little),
    summoned(items::BOGGED_SPAWN_EGG, entity_types::BOGGED, Drawn::Little),
    summoned(
        items::PARCHED_SPAWN_EGG,
        entity_types::PARCHED,
        Drawn::Little,
    ),
    summoned(
        items::CREEPER_SPAWN_EGG,
        entity_types::CREEPER,
        Drawn::Little,
    ),
    summoned(items::SPIDER_SPAWN_EGG, entity_types::SPIDER, Drawn::Little),
    summoned(
        items::CAVE_SPIDER_SPAWN_EGG,
        entity_types::CAVE_SPIDER,
        Drawn::Little,
    ),
    summoned(items::WITCH_SPAWN_EGG, entity_types::WITCH, Drawn::Little),
    summoned(
        items::PILLAGER_SPAWN_EGG,
        entity_types::PILLAGER,
        Drawn::Raider,
    ),
    summoned(
        items::VINDICATOR_SPAWN_EGG,
        entity_types::VINDICATOR,
        Drawn::Raider,
    ),
    summoned(items::EVOKER_SPAWN_EGG, entity_types::EVOKER, Drawn::Raider),
    summoned(
        items::GUARDIAN_SPAWN_EGG,
        entity_types::GUARDIAN,
        Drawn::Little,
    ),
    summoned(
        items::ELDER_GUARDIAN_SPAWN_EGG,
        entity_types::ELDER_GUARDIAN,
        Drawn::Little,
    ),
];

/// The spots about the builder the eggs are used on, one a type, each within reach.
const SPOTS: [(i32, i32); 21] = [
    (-4, -4),
    (-2, -4),
    (0, -4),
    (2, -4),
    (4, -4),
    (-4, -2),
    (-2, -2),
    (0, -2),
    (2, -2),
    (4, -2),
    (-4, 2),
    (-2, 2),
    (0, 2),
    (2, 2),
    (4, 2),
    (-4, 4),
    (-2, 4),
    (0, 4),
    (2, 4),
    (4, 4),
    (4, 0),
];

/// The mob's flags' bit of the left hand (`Mob.DATA_MOB_FLAGS_ID`, index 15).
const LEFT_HANDED: u8 = 0x02;

/// The head, by `EquipmentSlot`'s ordinal.
const HEAD: u8 = 5;

/// A builder in `address`'s world with the chunks around it, recording what it is sent.
async fn builder(address: &str) -> Result<Bot> {
    let mut bot = Bot::join(address, "Builder").await?;
    bot.wait_until(PATIENCE, |bot| {
        let x = (bot.location.0.floor() as i32).div_euclid(16);
        let z = (bot.location.2.floor() as i32).div_euclid(16);
        (-1..=1).all(|dx| (-1..=1).all(|dz| bot.chunks.contains_key(&(x + dx, z + dz))))
    })
    .await?;
    bot.record_packets();
    Ok(bot)
}

/// The column the bot stands in.
fn feet(bot: &Bot) -> (i32, i32) {
    (bot.location.0.floor() as i32, bot.location.2.floor() as i32)
}

/// The entities of `kind` the bot is shown.
fn of_kind(bot: &Bot, kind: i32) -> Vec<i32> {
    bot.entities
        .values()
        .filter(|entity| entity.kind == kind)
        .map(|entity| entity.entity_id)
        .collect()
}

/// Uses the egg `egg` on the grass at `(x, z)` and waits until the bot is shown a new
/// entity of `kind`, whose id it returns.
async fn egg(bot: &mut Bot, egg: i32, kind: i32, (x, z): (i32, i32)) -> Result<i32> {
    let before = of_kind(bot, kind);
    bot.put_in_hotbar(
        0,
        ItemStack {
            item: egg,
            count: 1,
        },
    )
    .await?;
    bot.select_slot(0).await?;
    bot.use_item_on(x, GROUND, z, face::TOP).await?;
    bot.wait_until(PATIENCE, |bot| {
        of_kind(bot, kind).iter().any(|id| !before.contains(id))
    })
    .await
    .with_context(|| format!("a new {}", clustine_data::ENTITY_TYPES[kind as usize]))?;
    of_kind(bot, kind)
        .into_iter()
        .find(|id| !before.contains(id))
        .context("a new mob")
}

/// What the bot has recorded so far.
fn recorded(bot: &Bot) -> &[(Instant, ClientboundPlay)] {
    bot.packets.as_deref().unwrap_or_default()
}

/// The entity `id` as the bot was first shown it.
fn first(bot: &Bot, id: i32) -> Result<FirstShown> {
    first_shown(recorded(bot))
        .into_iter()
        .find(|entity| entity.entity_id == id)
        .with_context(|| format!("the spawn of entity {id}"))
}

/// An attribute as compared: its id, its base where it is not drawn, and its
/// modifiers' ids and operations where their presence is not drawn.
type Attribute = (i32, Option<f64>, Vec<(String, i32)>);

/// What is compared of a mob made by an egg, with what depends on draws taken out.
#[derive(Debug, Clone, PartialEq)]
struct Comparable {
    kind: i32,
    /// Its place less the clicked block's corner.
    offset: (f64, f64, f64),
    head_on_yaw: bool,
    pitch: u8,
    velocity: [f64; 3],
    values: Vec<(u8, EntityDataValue)>,
    attributes: Vec<Attribute>,
    equipment: Vec<(u8, ItemStack)>,
}

fn comparable(mob: &FirstShown, clicked: (i32, i32), drawn: Drawn) -> Comparable {
    let kin = drawn == Drawn::ZombieKin;
    let mut values: Vec<(u8, EntityDataValue)> = mob
        .values
        .iter()
        .filter(|(index, _)| !(kin && *index == 16))
        .filter_map(|(index, value)| match (index, value) {
            (15, EntityDataValue::Byte(flags)) => {
                let flags = flags & !LEFT_HANDED;
                // Without the left hand the flags may be the default, which is not sent.
                (flags != 0).then_some((15, EntityDataValue::Byte(flags)))
            }
            (_, EntityDataValue::VillagerData { kind, level, .. })
                if mob.kind == entity_types::ZOMBIE_VILLAGER =>
            {
                Some((
                    *index,
                    EntityDataValue::VillagerData {
                        kind: *kind,
                        profession: -1,
                        level: *level,
                    },
                ))
            }
            _ => Some((*index, value.clone())),
        })
        .collect();
    values.sort_by_key(|(index, _)| *index);
    let mut attributes: Vec<Attribute> = mob
        .attributes
        .iter()
        .map(|attribute| {
            let base = (!(kin && attribute.attribute == attributes::SPAWN_REINFORCEMENTS))
                .then_some(attribute.base);
            let mut modifiers: Vec<(String, i32)> = if kin {
                Vec::new()
            } else {
                attribute
                    .modifiers
                    .iter()
                    .map(|(id, _, operation)| (id.clone(), *operation))
                    .collect()
            };
            modifiers.sort();
            (attribute.attribute, base, modifiers)
        })
        .collect();
    attributes.sort_by_key(|(id, _, _)| *id);
    let mut equipment: Vec<(u8, ItemStack)> = if kin {
        Vec::new()
    } else {
        mob.equipment
            .iter()
            .filter(|(slot, _)| !(drawn == Drawn::Raider && *slot == HEAD))
            .filter_map(|(slot, stack)| stack.map(|stack| (*slot, stack)))
            .collect()
    };
    equipment.sort_by_key(|(slot, _)| *slot);
    Comparable {
        kind: mob.kind,
        offset: (
            mob.position.0 - f64::from(clicked.0),
            mob.position.1 - f64::from(GROUND),
            mob.position.2 - f64::from(clicked.1),
        ),
        head_on_yaw: mob.head_yaw == mob.yaw,
        pitch: mob.pitch,
        velocity: mob.velocity,
        values,
        attributes,
        equipment,
    }
}

/// Every type made by its egg on the server at `address`, as compared.
async fn summon_all(address: &str) -> Result<Vec<Comparable>> {
    let mut bot = builder(address).await?;
    let (x, z) = feet(&bot);
    let mut made = Vec::new();
    for (summoned, (dx, dz)) in SUMMONED.iter().zip(SPOTS) {
        let clicked = (x + dx, z + dz);
        let id = egg(&mut bot, summoned.egg, summoned.kind, clicked).await?;
        made.push((id, clicked, summoned.drawn));
    }
    // What a server sends with a spawn comes with it, attributes perhaps a tick later.
    bot.idle(Duration::from_secs(1)).await?;
    made.into_iter()
        .map(|(id, clicked, drawn)| Ok(comparable(&first(&bot, id)?, clicked, drawn)))
        .collect()
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn each_type_made_by_its_egg_is_shown_like_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let theirs = summon_all(oracle.address()).await.unwrap();
    drop(oracle);
    let (server, address) = start().await;
    let ours = summon_all(&address).await.unwrap();
    server.stop().await;
    let mut differ = Vec::new();
    for (theirs, ours) in theirs.iter().zip(&ours) {
        let name = clustine_data::ENTITY_TYPES[theirs.kind as usize];
        println!("{name}:\n  official {theirs:?}\n  Clustine {ours:?}");
        assert!(theirs.head_on_yaw, "{name}: the official server's head");
        if theirs != ours {
            differ.push(name);
        }
    }
    assert!(differ.is_empty(), "these differ, printed above: {differ:?}");
}

/// The shared flags' bit of fire (`Entity.FLAG_ONFIRE`, index 0).
fn on_fire(values: &[(u8, EntityDataValue)]) -> bool {
    values.iter().any(|(index, value)| {
        *index == 0 && matches!(value, EntityDataValue::Byte(flags) if flags & 0x01 != 0)
    })
}

/// How long after its spawn a zombie made by an egg in the open is shown on fire, on
/// the server at `address`.
async fn catches_fire(address: &str) -> Result<Duration> {
    let mut bot = builder(address).await?;
    let (x, z) = feet(&bot);
    let zombie = egg(
        &mut bot,
        items::ZOMBIE_SPAWN_EGG,
        entity_types::ZOMBIE,
        (x + 2, z),
    )
    .await?;
    let burning = |bot: &Bot| {
        recorded(bot).iter().find_map(|(at, packet)| match packet {
            ClientboundPlay::SetEntityData(data)
                if data.entity_id == zombie && on_fire(&data.values) =>
            {
                Some(*at)
            }
            _ => None,
        })
    };
    bot.wait_until(Duration::from_secs(15), |bot| burning(bot).is_some())
        .await
        .context("the zombie's fire")?;
    let made = first(&bot, zombie)?.came;
    let burnt = burning(&bot).context("the zombie's fire")?;
    Ok(burnt.duration_since(made))
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_zombie_in_the_open_by_day_catches_fire_like_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let theirs = catches_fire(oracle.address()).await;
    drop(oracle);
    let (server, address) = start().await;
    let ours = catches_fire(&address).await;
    server.stop().await;
    println!("on fire after: official {theirs:?}, Clustine {ours:?}");
    theirs.expect("the official server's zombie");
    ours.expect("Clustine's zombie");
}

/// What a creeper lit by flint and steel did, by ticks from its ignited flag.
#[derive(Debug)]
struct Lit {
    primed: bool,
    exploded_after: u128,
    radius: f32,
    removed: bool,
}

/// The ticks, rounded, from `from` to `to`.
fn ticks(from: Instant, to: Instant) -> u128 {
    (to.duration_since(from).as_millis() + 25) / 50
}

async fn light_a_creeper(address: &str) -> Result<Lit> {
    let mut bot = builder(address).await?;
    let (x, z) = feet(&bot);
    let creeper = egg(
        &mut bot,
        items::CREEPER_SPAWN_EGG,
        entity_types::CREEPER,
        (x + 2, z),
    )
    .await?;
    bot.put_in_hotbar(
        1,
        ItemStack {
            item: items::FLINT_AND_STEEL,
            count: 1,
        },
    )
    .await?;
    bot.select_slot(1).await?;
    bot.interact(creeper, [0.0, 0.85, 0.0]).await?;
    let exploded = |bot: &Bot| {
        recorded(bot).iter().find_map(|(at, packet)| match packet {
            ClientboundPlay::Explode(explosion) => Some((*at, explosion.radius)),
            _ => None,
        })
    };
    bot.wait_until(Duration::from_secs(5), |bot| exploded(bot).is_some())
        .await
        .context("the explosion")?;
    bot.idle(Duration::from_millis(500)).await?;
    let packets = recorded(&bot);
    let ignited = packets
        .iter()
        .find_map(|(at, packet)| match packet {
            ClientboundPlay::SetEntityData(data)
                if data.entity_id == creeper
                    && data.values.iter().any(|(index, value)| {
                        *index == 18 && *value == EntityDataValue::Boolean(true)
                    }) =>
            {
                Some(*at)
            }
            _ => None,
        })
        .context("the creeper's ignited flag")?;
    let (blew, radius) = exploded(&bot).context("the explosion")?;
    let primed = packets.iter().any(|(_, packet)| {
        matches!(packet, ClientboundPlay::Sound(sound) if sound.sound == sounds::ENTITY_CREEPER_PRIMED)
    });
    let removed = packets.iter().any(|(_, packet)| {
        matches!(packet, ClientboundPlay::RemoveEntities(removed) if removed.entity_ids.contains(&creeper))
    });
    Ok(Lit {
        primed,
        exploded_after: ticks(ignited, blew),
        radius,
        removed,
    })
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_creeper_lit_by_flint_and_steel_explodes_like_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let theirs = light_a_creeper(oracle.address()).await.unwrap();
    drop(oracle);
    let (server, address) = start().await;
    let ours = light_a_creeper(&address).await.unwrap();
    server.stop().await;
    println!("official {theirs:?}\nClustine {ours:?}");
    for (name, lit) in [("the official server", &theirs), ("Clustine", &ours)] {
        assert!(lit.primed, "{name}: the primed sound");
        assert!(lit.removed, "{name}: the creeper gone");
        assert_eq!(lit.radius, 3.0, "{name}: the radius");
        assert!(
            (28..=33).contains(&lit.exploded_after),
            "{name}: exploded {} ticks after it was lit",
            lit.exploded_after
        );
    }
    assert!(
        theirs.exploded_after.abs_diff(ours.exploded_after) <= 1,
        "the official server's creeper exploded {} ticks after it was lit, Clustine's {}",
        theirs.exploded_after,
        ours.exploded_after
    );
}

/// What a snowball thrown at a villager was shown doing.
#[derive(Debug)]
struct Thrown {
    /// The snowball's spawn data less the thrower's id: 0 where it names the thrower.
    owner_off_by: i32,
    data: Vec<(u8, EntityDataValue)>,
    throw_heard: bool,
    /// Whether its entity event 3 came, and before its removal.
    event_then_removed: bool,
    /// Whether the villager's damage event named the thrower as its cause and the
    /// snowball as its direct cause.
    damaged_by_the_throw: bool,
    /// The villager's velocity after the hit: upward, and outward.
    knocked: Option<(f64, f64)>,
}

async fn throw_a_snowball(address: &str) -> Result<Thrown> {
    let mut bot = builder(address).await?;
    let me = bot.info.login.entity_id;
    let (x, z) = feet(&bot);
    let villager = egg(
        &mut bot,
        items::VILLAGER_SPAWN_EGG,
        entity_types::VILLAGER,
        (x + 3, z),
    )
    .await?;
    bot.put_in_hotbar(
        1,
        ItemStack {
            item: items::SNOWBALL,
            count: 16,
        },
    )
    .await?;
    bot.select_slot(1).await?;
    // From the eyes at the middle of the bot's column to the villager's middle, three
    // blocks east: east is a yaw of -90; down by the drop from the eyes (1.62) to its
    // middle (0.975).
    let pitch = (1.62_f64 - 0.975).atan2(3.0).to_degrees() as f32;
    bot.use_item(-90.0, pitch).await?;
    let gone = |bot: &Bot| {
        let snowball = recorded(bot).iter().find_map(|(_, packet)| match packet {
            ClientboundPlay::SpawnEntity(spawn) if spawn.kind == entity_types::SNOWBALL => {
                Some(spawn.entity_id)
            }
            _ => None,
        });
        snowball.is_some_and(|snowball| {
            recorded(bot).iter().any(|(_, packet)| {
                matches!(packet, ClientboundPlay::RemoveEntities(removed) if removed.entity_ids.contains(&snowball))
            })
        })
    };
    bot.wait_until(Duration::from_secs(5), gone)
        .await
        .context("the snowball's removal")?;
    bot.idle(Duration::from_millis(500)).await?;
    let packets = recorded(&bot);
    let snowball = first_shown(packets)
        .into_iter()
        .find(|entity| entity.kind == entity_types::SNOWBALL)
        .context("the snowball")?;
    let id = snowball.entity_id;
    let throw_heard = packets.iter().any(|(_, packet)| {
        matches!(packet, ClientboundPlay::Sound(sound) if sound.sound == sounds::ENTITY_SNOWBALL_THROW)
    });
    let event = packets.iter().position(|(_, packet)| {
        matches!(packet, ClientboundPlay::EntityEvent(event) if event.entity_id == id && event.event == 3)
    });
    let removal = packets.iter().position(|(_, packet)| {
        matches!(packet, ClientboundPlay::RemoveEntities(removed) if removed.entity_ids.contains(&id))
    });
    let damage = packets.iter().position(|(_, packet)| {
        matches!(packet, ClientboundPlay::DamageEvent(damage)
            if damage.entity_id == villager && damage.cause == Some(me) && damage.direct == Some(id))
    });
    let knocked = damage.and_then(|damage| {
        packets[damage..]
            .iter()
            .find_map(|(_, packet)| match packet {
                ClientboundPlay::SetEntityMotion(motion) if motion.entity_id == villager => {
                    let [x, y, z] = motion.motion;
                    Some((y, x.hypot(z)))
                }
                _ => None,
            })
    });
    Ok(Thrown {
        owner_off_by: snowball.spawn_data - me,
        data: snowball.values,
        throw_heard,
        event_then_removed: matches!((event, removal), (Some(event), Some(removal)) if event < removal),
        damaged_by_the_throw: damage.is_some(),
        knocked,
    })
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_snowball_thrown_at_a_villager_is_shown_like_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let theirs = throw_a_snowball(oracle.address()).await.unwrap();
    drop(oracle);
    let (server, address) = start().await;
    let ours = throw_a_snowball(&address).await.unwrap();
    server.stop().await;
    println!("official {theirs:?}\nClustine {ours:?}");
    for (name, thrown) in [("the official server", &theirs), ("Clustine", &ours)] {
        assert_eq!(thrown.owner_off_by, 0, "{name}: the snowball's owner");
        assert!(thrown.throw_heard, "{name}: the throw's sound");
        assert!(
            thrown.event_then_removed,
            "{name}: the hit's event 3, then the removal"
        );
        assert!(
            thrown.damaged_by_the_throw,
            "{name}: the villager's damage event (a miss is drawn, rarely: run it again)"
        );
    }
    assert_eq!(ours.data, theirs.data, "the snowball's data");
    let (Some(theirs_knocked), Some(ours_knocked)) = (theirs.knocked, ours.knocked) else {
        panic!("a knockback's velocity: official {theirs:?}, Clustine {ours:?}");
    };
    assert!(
        (theirs_knocked.0 - ours_knocked.0).abs() < 0.01
            && (theirs_knocked.1 - ours_knocked.1).abs() < 0.05,
        "the knockback: official {theirs_knocked:?}, Clustine {ours_knocked:?}"
    );
}

// N8's fourth comparison (`docs/groundwork/n8-plan.md`, section 9): a zombie at noon
// under glass, which lets the sky's light down whole, burns; one under leaves, which
// take one from it, does not (burning needs the sky seen, light 15 at the eyes).
//
// Each zombie is in a pit three blocks deep, dug beside the builder, roofed at the
// grass's height with glass or oak leaves placed against the side of the grass beside
// it, and made by its egg on the bedrock: it cannot climb out, and only the roof lets
// light in. The official server's clock is set to noon and paused; Clustine, which has
// no commands, is started on a world whose clock was written into the world store's
// table file at noon, which the region takes as the later of it and its own (see
// `detector_oracle.rs`). The zombie under glass is to be on fire within fifteen seconds
// (it draws `(1.0 - 0.4) * 2 / 30` a tick), and the one under leaves never, while the
// builder watches until five seconds after the first burnt. If it fails on Clustine
// alone, look at the light under the roof first (`region/light.rs`, leaves'
// dampening), then at the burning (`burning.rs`).

use clustine::Config;
use clustine_format::TableFile;
use common::{config, start_with};

/// The day time of noon, and the game time Clustine's noon world is given.
const NOON: u64 = 6_000;
const NOON_GAME_TIME: u64 = 100_000;

/// Starts Clustine on a new world in `directory` whose clock is at noon.
async fn start_clustine_at_noon(directory: &std::path::Path) -> Result<clustine::Server> {
    let on_disk = || Config {
        world: Some(directory.to_owned()),
        ..config()
    };
    let (server, _) = start_with(on_disk()).await;
    server.stop().await;
    let path = directory.join("regions").join("table");
    let mut table = TableFile::decode(&std::fs::read(&path)?).context("the table file")?;
    anyhow::ensure!(
        table.clock[0] < NOON_GAME_TIME && table.clock[1] < 24_000 + NOON,
        "the new world's clock {:?} is already past noon of its second day",
        table.clock
    );
    table.clock = [NOON_GAME_TIME, 24_000 + NOON];
    std::fs::write(&path, table.encode())?;
    Ok(start_with(on_disk()).await.0)
}

/// A pit three deep at `(x, z)` roofed with `roof`, placed against the side of the grass
/// a step towards the builder, and a zombie made by its egg at its bottom, whose id is
/// returned.
async fn zombie_in_a_pit(bot: &mut Bot, (x, z): (i32, i32), roof: i32, east: bool) -> Result<i32> {
    for depth in 0..3 {
        let sequence = bot.dig(x, GROUND - depth, z).await?;
        bot.wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= sequence)
            .await?;
    }
    bot.put_in_hotbar(
        2,
        ItemStack {
            item: roof,
            count: 1,
        },
    )
    .await?;
    bot.select_slot(2).await?;
    // The grass a step towards the builder, and its face towards the pit.
    let (beside, side, cursor) = if east {
        (x - 1, face::EAST, [1.0, 0.5, 0.5])
    } else {
        (x + 1, face::WEST, [0.0, 0.5, 0.5])
    };
    let sequence = bot.click(beside, GROUND, z, side, cursor).await?;
    bot.wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= sequence)
        .await?;
    bot.wait_until(PATIENCE, |bot| {
        bot.block_at(x, GROUND, z)
            .ok()
            .flatten()
            .and_then(|id| u16::try_from(id).ok())
            .map(clustine_data::BlockState)
            .and_then(|state| state.block())
            .is_some_and(|block| block.name != "minecraft:air")
    })
    .await
    .context("the pit's roof")?;
    let before = of_kind(bot, entity_types::ZOMBIE);
    bot.put_in_hotbar(
        0,
        ItemStack {
            item: items::ZOMBIE_SPAWN_EGG,
            count: 1,
        },
    )
    .await?;
    bot.select_slot(0).await?;
    bot.use_item_on(x, GROUND - 3, z, face::TOP).await?;
    bot.wait_until(PATIENCE, |bot| {
        of_kind(bot, entity_types::ZOMBIE)
            .iter()
            .any(|id| !before.contains(id))
    })
    .await
    .context("a new zombie in the pit")?;
    of_kind(bot, entity_types::ZOMBIE)
        .into_iter()
        .find(|id| !before.contains(id))
        .context("a new zombie")
}

/// When each zombie was first shown on fire, if it was: the one under glass and the one
/// under leaves, from its spawn.
#[derive(Debug)]
struct Burnt {
    under_glass: Option<Duration>,
    under_leaves: Option<Duration>,
}

async fn zombies_under_roofs(address: &str) -> Result<Burnt> {
    let mut bot = builder(address).await?;
    let (x, z) = feet(&bot);
    let glass = zombie_in_a_pit(&mut bot, (x + 2, z), items::GLASS, true).await?;
    let leaves = zombie_in_a_pit(&mut bot, (x - 2, z), items::OAK_LEAVES, false).await?;
    let burnt = |bot: &Bot, zombie: i32| {
        recorded(bot).iter().find_map(|(at, packet)| match packet {
            ClientboundPlay::SetEntityData(data)
                if data.entity_id == zombie && on_fire(&data.values) =>
            {
                Some(*at)
            }
            _ => None,
        })
    };
    // Not catching fire in time is what is compared, so the wait's end is no error.
    let _ = bot
        .wait_until(Duration::from_secs(15), |bot| burnt(bot, glass).is_some())
        .await;
    bot.idle(Duration::from_secs(5)).await?;
    let since = |zombie: i32| -> Result<Option<Duration>> {
        let made = first(&bot, zombie)?.came;
        Ok(burnt(&bot, zombie).map(|at| at.duration_since(made)))
    };
    Ok(Burnt {
        under_glass: since(glass)?,
        under_leaves: since(leaves)?,
    })
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_zombie_at_noon_burns_under_glass_and_not_under_leaves_like_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    oracle
        .command(
            &format!("time set {NOON}"),
            &["tick(s)", "Set the time", "already set"],
        )
        .await
        .unwrap();
    oracle
        .command("time pause", &["Paused clock", "already paused"])
        .await
        .unwrap();
    let theirs = zombies_under_roofs(oracle.address()).await;
    drop(oracle);
    let world = tempfile::tempdir().unwrap();
    let server = start_clustine_at_noon(world.path()).await.unwrap();
    let address = server.address().to_string();
    let ours = zombies_under_roofs(&address).await;
    server.stop().await;
    println!("official {theirs:?}\nClustine {ours:?}");
    for (name, burnt) in [("the official server", theirs), ("Clustine", ours)] {
        let burnt = burnt.unwrap_or_else(|error| panic!("{name}: {error:#}"));
        assert!(
            burnt.under_glass.is_some(),
            "{name}: the zombie under glass did not catch fire within fifteen seconds at noon"
        );
        assert!(
            burnt.under_leaves.is_none(),
            "{name}: the zombie under leaves caught fire {:?} after its spawn",
            burnt.under_leaves
        );
    }
}
