//! Comparisons with the official server of survival's damage, hunger and death (V4): the
//! same script of survival bots is run on both servers, and what each bot was told is
//! compared. States are compared, not packets: the health told after a hurt, the food and
//! saturation told after a run, the death's message, and the item stacks a watcher is
//! shown lying where a bot died.
//!
//! The script, each step a bot of its own that joins at the spawn, is made a survival
//! player (the official server's console's `gamemode survival`; Clustine's world is a
//! survival one, as its newcomers start with nothing as the official server's do), and
//! leaves when done, so that a mob of one step goes after no bot of another. A watcher,
//! creative, which no mob goes after, is shown what lies about:
//!
//! 1. Falls from 4, 5, 10 and 23 blocks: the bot rises a block a tick in the air and falls
//!    a block a tick to the ground (`Entity.checkFallDamage` adds each move's drop, the
//!    landing's too, and `LivingEntity.calculateFallDamage` takes `floor(distance + 1e-6 −
//!    3)`); the health told after it (19, 18, 13, and a death at 23, with
//!    `death.fell.accident.generic`, `CombatTracker.getFallMessage`), and the stacks the
//!    faller of 23 blocks carried, which lie where it died (`Inventory.dropAll`).
//! 2. A sprint of jumps: a hundred and twenty sprinting jumps of half a block up and a
//!    block on, back and forth (`ServerPlayer.jumpFromGround`'s 0.2 and
//!    `checkMovementStatistics`' 0.1 a metre sprinting on the ground, half a metre a
//!    landing), and the food and saturation told once the server has handled the moves
//!    and five round trips more (`FoodData.tick @11-75` takes 4 of exhaustion a tick):
//!    the game's rule would leave 18 and 0.
//! 3. Lava for a second: a lava bucket emptied six blocks east of the spawn, which the
//!    bot steps into for twenty ticks: the first health told (`Entity.lavaHurt`, 4); the
//!    bucket filled again from the source after.
//! 4. A zombie's hit on easy in leather and in iron (`Mob.doHurtTarget`, 3 scaled to 2.5,
//!    then the armour, `CombatRules.getDamageAfterAbsorb`): the armour put on by clicks
//!    of window 0, the zombie from its egg four blocks west; the health told after its
//!    first hit, and whether the bot was sent its own `SetEntityMotion` (`markHurt`,
//!    `ServerEntity.sendChanges`).
//! 5. A death in the void with items: the bot goes down in the air, nine blocks a tick, to
//!    the void, which kills it (`death.attack.outOfWorld`); what the watcher is shown
//!    lying after (the items fall out of the world, `ItemEntity`'s `checkBelowWorld`).
//!
//! Drowning is not compared: it needs water above the eyes, which a bot on the flat world
//! cannot make without digging a pit, and digging in survival is another test's.
//!
//! The comparisons need Java, the server jar downloaded by `cargo datagen`, and your
//! agreement to the Minecraft EULA, so they are ignored by default. Run them on a quiet
//! machine:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test survival_oracle -- --ignored --nocapture --test-threads 1
//! ```

mod common;

use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use clustine::Config;
use clustine_botswarm::{Bot, Oracle};
use clustine_data::{blocks, entity_types, items};
use clustine_protocol::nbt::Nbt;
use clustine_protocol::packets::play::{ClientboundPlay, EntityDataValue, click, face};
use clustine_sim::api::Order;
use clustine_world::GameMode;

use common::{config, start_with};

const PATIENCE: Duration = Duration::from_secs(30);

/// Below the flat world's lowest y, -64, less the void's 64, by some way.
const IN_THE_VOID: f64 = -150.0;

/// `ItemEntity.DATA_ITEM`: after `Entity`'s eight.
const ITEM_DATA: u8 = 8;

/// What each item is called in the official server's `give`.
const NAMES: [(i32, &str); 13] = [
    (items::DIRT, "dirt"),
    (items::IRON_SWORD, "iron_sword"),
    (items::LAVA_BUCKET, "lava_bucket"),
    (items::ZOMBIE_SPAWN_EGG, "zombie_spawn_egg"),
    (items::LEATHER_HELMET, "leather_helmet"),
    (items::LEATHER_CHESTPLATE, "leather_chestplate"),
    (items::LEATHER_LEGGINGS, "leather_leggings"),
    (items::LEATHER_BOOTS, "leather_boots"),
    (items::IRON_HELMET, "iron_helmet"),
    (items::IRON_CHESTPLATE, "iron_chestplate"),
    (items::IRON_LEGGINGS, "iron_leggings"),
    (items::IRON_BOOTS, "iron_boots"),
    (items::COBBLESTONE, "cobblestone"),
];

/// The server a run plays on, and how its operator gives and sets a game mode.
enum Host<'a> {
    Official(&'a Oracle),
    Clustine(&'a clustine::Server),
}

impl Host<'_> {
    async fn give(&self, name: &str, item: i32, count: i32) -> Result<()> {
        match self {
            Self::Official(oracle) => {
                let (_, item_name) = NAMES
                    .iter()
                    .find(|(id, _)| *id == item)
                    .with_context(|| format!("no name for item {item}"))?;
                oracle
                    .command(
                        &format!("give {name} minecraft:{item_name} {count}"),
                        &["Gave"],
                    )
                    .await?;
            }
            Self::Clustine(server) => {
                server.order(name, Order::Give { item, count }).await?;
            }
        }
        Ok(())
    }

    /// Makes the watcher a creative player, whom no mob goes after: Clustine's world is a
    /// survival one, whose newcomers start with nothing as the official server's do; the
    /// official server's world is creative already.
    async fn creative(&self, name: &str) -> Result<()> {
        if let Self::Clustine(server) = self {
            server
                .order(name, Order::GameMode(GameMode::Creative))
                .await?;
        }
        Ok(())
    }

    async fn survival(&self, name: &str) -> Result<()> {
        match self {
            Self::Official(oracle) => {
                oracle
                    .command(&format!("gamemode survival {name}"), &["game mode"])
                    .await?;
            }
            Self::Clustine(server) => {
                server
                    .order(name, Order::GameMode(GameMode::Survival))
                    .await?;
            }
        }
        Ok(())
    }
}

/// What a run was told, step by step.
#[derive(Debug, Default, PartialEq)]
struct Ran {
    /// Each fall's height, the health told after it, and its death's message if it
    /// killed.
    falls: Vec<(i32, f32, Option<String>)>,
    /// The stacks lying where the faller of 23 blocks died, as (item, count), sorted.
    fall_drops: Vec<(i32, i32)>,
    /// The food and saturation told after the sprint of jumps.
    sprint: Option<(i32, f32)>,
    /// The first health told in lava.
    lava: Option<f32>,
    /// For leather and for iron: the health told after the zombie's first hit, and
    /// whether the bot was sent its own motion.
    zombie: Vec<(String, f32, bool)>,
    /// The void's death message and what lies where the bot went down after it.
    void_death: Option<String>,
    void_drops: Vec<(i32, i32)>,
}

/// Waits until the server has handled everything the bot sent before: a use of the
/// ground beside it with the empty hand, which changes nothing and is acknowledged.
async fn round_trip(bot: &mut Bot) -> Result<()> {
    let (x, z) = (bot.location.0.floor() as i32, bot.location.2.floor() as i32);
    let sequence = bot.use_item_on(x + 1, -61, z - 1, face::TOP).await?;
    bot.wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= sequence)
        .await
        .context("no acknowledgement")?;
    Ok(())
}

/// A bot `name` joins, waits for its chunks, is made a survival player and holds the
/// hotbar's slot 0, which gives fill first.
async fn join(address: &str, host: &Host<'_>, name: &str) -> Result<Bot> {
    let mut bot = Bot::join(address, name).await?;
    bot.record_packets();
    bot.wait_for_chunks(9, PATIENCE).await?;
    host.survival(name).await?;
    bot.select_slot(0).await?;
    round_trip(&mut bot).await?;
    Ok(bot)
}

/// Gives `name` `count` of `item`, of which it has none, and waits until window 0 holds
/// them: in the hotbar's first empty slot (`Inventory.add`), window 0's 36 where the bot
/// holds nothing else.
async fn given(host: &Host<'_>, bot: &mut Bot, name: &str, item: i32, count: i32) -> Result<()> {
    host.give(name, item, count).await?;
    bot.wait_until(PATIENCE, |bot| {
        let held: i32 = bot
            .inventory_window
            .slots
            .iter()
            .flatten()
            .filter(|stack| stack.item == item)
            .map(|stack| stack.count)
            .sum();
        held == count
    })
    .await
    .with_context(|| {
        format!(
            "{count} of {item} were not given: window 0 holds {:?}",
            bot.inventory_window.slots
        )
    })?;
    Ok(())
}

/// The key of a death's message.
fn message_key(message: &Nbt) -> Option<String> {
    let Nbt::Compound(fields) = message else {
        return None;
    };
    fields
        .iter()
        .find_map(|(name, value)| match (name.as_str(), value) {
            ("translate", Nbt::String(key)) => Some(key.clone()),
            _ => None,
        })
}

/// Waits until the bot is told a health below 20 or its death: the first health below 20
/// it was told, from what it recorded (later ones depend on how long the bot took to
/// look: regeneration and more hurts).
async fn hurt(bot: &mut Bot) -> Result<f32> {
    bot.wait_until(PATIENCE, |bot| {
        !bot.deaths.is_empty() || bot.health.as_ref().is_some_and(|told| told.health < 20.0)
    })
    .await
    .context("no hurt was told")?;
    bot.packets
        .as_deref()
        .unwrap_or_default()
        .iter()
        .find_map(|(_, packet)| match packet {
            ClientboundPlay::SetHealth(told) if told.health < 20.0 => Some(told.health),
            _ => None,
        })
        .context("the bot recorded no hurt")
}

/// The item stacks the watcher shows now within three blocks of `x` and `z`, by what its
/// recorded packets said of each, sorted.
fn lying_near(watcher: &Bot, x: f64, z: f64) -> Vec<(i32, i32)> {
    let near: BTreeSet<i32> = watcher
        .entities
        .values()
        .filter(|entity| {
            entity.kind == entity_types::ITEM
                && (entity.position.0 - x).abs() < 3.0
                && (entity.position.2 - z).abs() < 3.0
        })
        .map(|entity| entity.entity_id)
        .collect();
    let mut stacks = Vec::new();
    for id in near {
        let stack = watcher
            .packets
            .as_deref()
            .unwrap_or_default()
            .iter()
            .rev()
            .find_map(|(_, packet)| match packet {
                ClientboundPlay::SetEntityData(data) if data.entity_id == id => data
                    .values
                    .iter()
                    .find_map(|(index, value)| match (index, value) {
                        (&ITEM_DATA, EntityDataValue::ItemStack(Some(stack))) => {
                            Some((stack.item, stack.count))
                        }
                        _ => None,
                    }),
                _ => None,
            });
        if let Some(stack) = stack {
            stacks.push(stack);
        }
    }
    stacks.sort_unstable();
    stacks
}

/// Five round trips of the watcher: what the server did meanwhile has been shown.
async fn settle(watcher: &mut Bot) -> Result<()> {
    for _ in 0..5 {
        round_trip(watcher).await?;
    }
    Ok(())
}

/// Step 1: a fall of `height` blocks by the bot `name`, carrying `carried`, eight blocks
/// south of the spawn: what it drops lies out of the way of the bots that come after.
async fn fall(
    address: &str,
    host: &Host<'_>,
    watcher: &mut Bot,
    ran: &mut Ran,
    name: &str,
    height: i32,
    carried: &[(i32, i32)],
) -> Result<()> {
    let mut bot = join(address, host, name).await?;
    for (item, count) in carried {
        given(host, &mut bot, name, *item, *count).await?;
    }
    let (x, y, spawn_z) = bot.location;
    let z = spawn_z + 8.0;
    bot.move_in_tick(x, y, z, true).await?;
    for up in 1..=height {
        bot.move_in_tick(x, y + f64::from(up), z, false).await?;
    }
    for down in (0..height).rev() {
        bot.move_in_tick(x, y + f64::from(down), z, down == 0)
            .await?;
    }
    let health = hurt(&mut bot).await?;
    let death = bot
        .deaths
        .first()
        .and_then(|death| message_key(&death.message));
    ran.falls.push((height, health, death));
    if !carried.is_empty() {
        let wanted = carried.len();
        watcher
            .wait_until(PATIENCE, |watcher| {
                lying_near(watcher, x, z).len() >= wanted
            })
            .await
            .context("the watcher was shown no drops")?;
        settle(watcher).await?;
        ran.fall_drops = lying_near(watcher, x, z);
    }
    bot.close().await?;
    Ok(())
}

/// Step 2: a hundred and twenty sprinting jumps, back and forth west of the spawn: 0.25
/// of exhaustion each, 30 in all, which take the 5 of saturation (told as it reaches 0)
/// and two points of food (each told).
async fn sprint(address: &str, host: &Host<'_>, ran: &mut Ran) -> Result<()> {
    let mut bot = join(address, host, "Runner").await?;
    bot.sprint(true).await?;
    let (x, y, z) = bot.location;
    let (mut at, mut west) = (x, true);
    for _ in 0..120 {
        let step = if west { -0.5 } else { 0.5 };
        at += step;
        bot.move_in_tick(at, y + 0.42, z, false).await?;
        at += step;
        bot.move_in_tick(at, y, z, true).await?;
        if (at - x).abs() >= 3.0 {
            west = !west;
        }
    }
    for _ in 0..5 {
        round_trip(&mut bot).await?;
    }
    ran.sprint = Some(
        bot.health
            .as_ref()
            .map_or((20, 5.0), |told| (told.food, told.saturation)),
    );
    bot.close().await?;
    Ok(())
}

/// Step 3: a second in lava six blocks east of the spawn, far enough that its flow keeps
/// off the spawn and the zombies of step 4, which come from the west.
async fn lava(address: &str, host: &Host<'_>, ran: &mut Ran) -> Result<()> {
    let mut bot = join(address, host, "Bather").await?;
    given(host, &mut bot, "Bather", items::LAVA_BUCKET, 1).await?;
    let (spawn_x, y, z) = bot.location;
    let x = spawn_x + 4.0;
    bot.move_in_tick(x, y, z, true).await?;
    let (pool_x, pool_z) = (x.floor() as i32 + 2, z.floor() as i32);
    // A bucket is used by its ray along the look (`BucketItem.use`), aimed at the top of
    // the floor two blocks east: yaw 0 looks south, -90 east; a pitch above 0 down.
    let (dx, dy, dz) = (
        f64::from(pool_x) + 0.5 - x,
        -60.0 - (y + 1.62),
        f64::from(pool_z) + 0.5 - z,
    );
    let yaw = (dz.atan2(dx).to_degrees() - 90.0) as f32;
    let pitch = (-dy).atan2(dx.hypot(dz)).to_degrees() as f32;
    bot.use_item(yaw, pitch).await?;
    bot.wait_until(PATIENCE, |bot| {
        bot.block_at(pool_x, -60, pool_z)
            .unwrap()
            .is_some_and(|state| {
                clustine_data::BlockState(state as u16).block_id() == blocks::LAVA.block_id()
            })
    })
    .await
    .context("the lava was not placed")?;
    let (in_x, in_z) = (f64::from(pool_x) + 0.5, f64::from(pool_z) + 0.5);
    for _ in 0..20 {
        bot.move_in_tick(in_x, y, in_z, true).await?;
    }
    ran.lava = Some(hurt(&mut bot).await?);
    // Back where it emptied the bucket, it fills it again from the source, aimed as
    // before, so that no lava is left to flow in the way of the steps after.
    bot.move_in_tick(x, y, z, true).await?;
    bot.use_item(yaw, pitch).await?;
    bot.wait_until(PATIENCE, |bot| {
        bot.block_at(pool_x, -60, pool_z)
            .unwrap()
            .is_some_and(|state| state == 0)
    })
    .await
    .context("the lava was not taken back")?;
    bot.move_in_tick(spawn_x, y, z, true).await?;
    bot.close().await?;
    Ok(())
}

/// Step 4: a zombie's hit on the bot `name` wearing `pieces`.
async fn zombie(
    address: &str,
    host: &Host<'_>,
    ran: &mut Ran,
    name: &str,
    pieces: [i32; 4],
) -> Result<()> {
    let mut bot = join(address, host, name).await?;
    // Each piece taken up from the hotbar's first slot and put down in its armour slot of
    // window 0 (`InventoryMenu.<init>`: 5 the head to 8 the feet), as a vanilla client
    // guesses the two clicks.
    for (piece, worn_at) in pieces.into_iter().zip(5_i16..) {
        given(host, &mut bot, name, piece, 1).await?;
        let one = Some(clustine_protocol::item::ItemStack::new(piece, 1));
        bot.click_inventory(36, 0, click::PICKUP, vec![(36, None)], one.clone())
            .await?;
        bot.click_inventory(worn_at, 0, click::PICKUP, vec![(worn_at, one)], None)
            .await?;
        round_trip(&mut bot).await?;
        bot.wait_until(PATIENCE, |bot| {
            bot.inventory_window.slots[usize::try_from(worn_at).expect("a slot")]
                .as_ref()
                .is_some_and(|stack| stack.item == piece)
        })
        .await
        .with_context(|| format!("{piece} was not put on"))?;
    }
    let worn: Vec<Option<i32>> = (5..9)
        .map(|slot| {
            bot.inventory_window.slots[slot]
                .as_ref()
                .map(|stack| stack.item)
        })
        .collect();
    ensure!(worn.iter().all(Option::is_some), "worn: {worn:?}");
    given(host, &mut bot, name, items::ZOMBIE_SPAWN_EGG, 1).await?;
    let (x, z) = (bot.location.0.floor() as i32, bot.location.2.floor() as i32);
    bot.use_item_on(x - 4, -61, z, face::TOP).await?;
    bot.wait_until(PATIENCE, |bot| {
        bot.entities
            .values()
            .any(|entity| entity.kind == entity_types::ZOMBIE)
    })
    .await
    .with_context(|| {
        format!(
            "no zombie came of the egg: {:?}",
            bot.inventory_window.slots[36]
        )
    })?;
    let health = match hurt(&mut bot).await {
        Ok(health) => health,
        Err(error) => {
            let zombies: Vec<_> = bot
                .entities
                .values()
                .filter(|entity| entity.kind == entity_types::ZOMBIE)
                .collect();
            anyhow::bail!(
                "{error:#}: the bot at {:?}, the zombies {zombies:?}, damage events {:?}",
                bot.location,
                bot.damage_events
            );
        }
    };
    let own = bot.info.login.entity_id;
    let knocked = bot
        .packets
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|(_, packet)| {
            matches!(packet, ClientboundPlay::SetEntityMotion(motion) if motion.entity_id == own)
        });
    ran.zombie.push((name.to_owned(), health, knocked));
    bot.close().await?;
    Ok(())
}

/// Step 5: down into the void, in the air, carrying dirt.
async fn void(address: &str, host: &Host<'_>, watcher: &mut Bot, ran: &mut Ran) -> Result<()> {
    let mut bot = join(address, host, "Diver").await?;
    given(host, &mut bot, "Diver", items::DIRT, 16).await?;
    let (x, y, z) = bot.location;
    let mut at = y;
    while at > IN_THE_VOID {
        at = (at - 9.0).max(IN_THE_VOID);
        bot.move_in_tick(x, at, z, false).await?;
    }
    bot.wait_until(PATIENCE, |bot| !bot.deaths.is_empty())
        .await
        .context("the void did not kill")?;
    ran.void_death = bot
        .deaths
        .first()
        .and_then(|death| message_key(&death.message));
    settle(watcher).await?;
    ran.void_drops = lying_near(watcher, x, z);
    bot.close().await?;
    Ok(())
}

async fn run(address: &str, host: Host<'_>) -> Result<Ran> {
    let mut watcher = Bot::join(address, "Watcher").await?;
    watcher.wait_for_chunks(9, PATIENCE).await?;
    host.creative("Watcher").await?;
    watcher.record_packets();
    round_trip(&mut watcher).await?;
    let mut ran = Ran::default();
    for (name, height) in [("Fall4", 4), ("Fall5", 5), ("Fall10", 10)] {
        fall(address, &host, &mut watcher, &mut ran, name, height, &[]).await?;
    }
    fall(
        address,
        &host,
        &mut watcher,
        &mut ran,
        "Fall23",
        23,
        &[(items::DIRT, 16), (items::IRON_SWORD, 1)],
    )
    .await?;
    sprint(address, &host, &mut ran).await?;
    lava(address, &host, &mut ran).await?;
    zombie(
        address,
        &host,
        &mut ran,
        "Leather",
        [
            items::LEATHER_HELMET,
            items::LEATHER_CHESTPLATE,
            items::LEATHER_LEGGINGS,
            items::LEATHER_BOOTS,
        ],
    )
    .await?;
    zombie(
        address,
        &host,
        &mut ran,
        "Iron",
        [
            items::IRON_HELMET,
            items::IRON_CHESTPLATE,
            items::IRON_LEGGINGS,
            items::IRON_BOOTS,
        ],
    )
    .await?;
    void(address, &host, &mut watcher, &mut ran).await?;
    watcher.close().await?;
    Ok(ran)
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn survival_runs_as_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let theirs = run(oracle.address(), Host::Official(&oracle)).await;
    drop(oracle);
    // A survival world, whose newcomers start with nothing, as the official server's;
    // the watcher is made creative.
    let survival = Config {
        game_mode: GameMode::Survival,
        force_game_mode: true,
        ..config()
    };
    let (server, address) = start_with(survival).await;
    let ours = run(&address, Host::Clustine(&server)).await;
    server.stop().await;
    let theirs = theirs.unwrap_or_else(|error| panic!("the official server: {error:#}"));
    let ours = ours.unwrap_or_else(|error| panic!("Clustine: {error:#}"));
    println!("official: {theirs:#?}\nClustine: {ours:#?}");
    // What the game's rules give, which both are to show.
    assert_eq!(
        theirs.falls.iter().map(|fall| fall.1).collect::<Vec<_>>(),
        [19.0, 18.0, 13.0, 0.0],
        "the official server's falls"
    );
    assert_eq!(theirs, ours);
}
