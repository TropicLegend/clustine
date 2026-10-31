//! Comparisons with the official server of the Nether's mobs (phase P, step P12,
//! `docs/groundwork/p-plan.md`, sections 18.4 to 18.6): a piglin bartering a gold
//! ingot, a strider on lava and off it, a piglin turning into a zombified piglin in the
//! overworld, and a zombified piglin and a magma cube in lava.
//!
//! These tests need Java, the server jar downloaded by `cargo datagen`, and your
//! agreement to the Minecraft EULA, so they are ignored by default. Run them with:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true tools/oracles.sh nether_mobs_oracle
//! ```
//!
//! The scenes in the Nether are built on its roof of bedrock, which is the same in every
//! Nether (`common/dimensions.rs` says why and how); the zombification is in the
//! overworld, beside where the bot joins. Every mob is hatched from its spawn egg.
//!
//! Every player is in creative mode on both servers, and no mob takes a creative player
//! as its target: a ghast's and a blaze's shooting, a piglin's attack and its anger are
//! not seen in these scenes, and are not attempted. Neither server can be told what to
//! draw, so what depends on draws is left out, each named where it is: whether a
//! hatched piglin is a baby (a scene hatches until it has an adult), what it wears and
//! holds, which of the bartering table's entries a barter gives and how many, a strider
//! hatched with a rider, a magma cube's size, the sounds' pitches and seeds, and where a
//! mob wanders. A time is told by the server's own clock: the game time of the latest
//! time packet (which both servers send each second) and the ticks since it came by the
//! client's clock, so that a server that is slow for a moment does not count against
//! it; one tick either way is still the same tick, as a packet may come a little late.

mod common;
#[path = "common/dimensions.rs"]
#[allow(dead_code)]
mod dimensions;

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use clustine_botswarm::Bot;
use clustine_data::{ITEMS, SOUND_EVENTS, blocks, entity_types, items};
use clustine_protocol::packets::play::{ClientboundPlay, EntityDataValue, face};
use tokio::time::Instant;

use common::shown::{FirstShown, first_shown};
use dimensions::{
    GRASS, PATIENCE, ROOF, around_is_held, describe, feet, on_both, onto_the_roof, place, pour,
    round_trip, take, teleports, ticks, turn, wait,
};

/// Collects what differs between the two servers' runs, by name.
#[derive(Default)]
struct Differences(Vec<String>);

impl Differences {
    fn differ(&mut self, what: &str, same: bool) {
        if !same {
            self.0.push(what.to_owned());
        }
    }

    fn assert_none(self) {
        assert!(self.0.is_empty(), "differ: {:?}", self.0);
    }
}

fn both<T: std::fmt::Debug>(theirs: Result<T>, ours: Result<T>) -> (T, T) {
    let theirs = theirs.unwrap_or_else(|error| panic!("on the official server: {error:#}"));
    let ours = ours.unwrap_or_else(|error| panic!("on Clustine: {error:#}"));
    println!("official: {theirs:#?}\nClustine: {ours:#?}");
    (theirs, ours)
}

/// Whether two counts of ticks are the same tick, as a client can tell it.
fn same_tick(theirs: Option<i64>, ours: Option<i64>) -> bool {
    matches!((theirs, ours), (Some(theirs), Some(ours)) if (theirs - ours).abs() <= 1)
}

// ---------------------------------------------------------------------------------
// What the scenes share.
// ---------------------------------------------------------------------------------

/// The shared flags' bit of fire (`Entity.FLAG_ONFIRE`, index 0).
const ON_FIRE: u8 = 0x01;

/// A living entity's health (`LivingEntity.DATA_HEALTH_ID`).
const HEALTH_INDEX: u8 = 9;

/// The mob's flags (`Mob.DATA_MOB_FLAGS_ID`), whose bit of the left hand is drawn.
const MOB_FLAGS_INDEX: u8 = 15;
const LEFT_HANDED: u8 = 0x02;

/// `AbstractPiglin.DATA_IMMUNE_TO_ZOMBIFICATION`, after `Mob`'s 15, and the piglin's
/// `DATA_BABY_ID`.
const PIGLIN_IMMUNE_INDEX: u8 = 16;
const PIGLIN_BABY_INDEX: u8 = 17;

/// `Zombie.DATA_BABY_ID`, which a zombified piglin has.
const ZOMBIE_BABY_INDEX: u8 = 16;

/// `Strider.DATA_SUFFOCATING`, after `AgeableMob`'s two (16 and 17) and the strider's
/// `DATA_BOOST_TIME` (18): the strider shown cold and shaking.
const STRIDER_SUFFOCATING_INDEX: u8 = 19;

/// `EquipmentSlot.OFFHAND`'s ordinal in `set_equipment`.
const OFF_HAND: u8 = 1;

/// What the bot has recorded since it was told to record.
fn recorded(bot: &Bot) -> &[(Instant, ClientboundPlay)] {
    bot.packets.as_deref().unwrap_or_default()
}

/// The server's tick when a packet came at `at`: the game time of the latest time
/// packet that came before it, and the ticks since that came by the client's clock.
fn server_tick(packets: &[(Instant, ClientboundPlay)], at: Instant) -> Option<i64> {
    let times = packets.iter().filter_map(|(came, packet)| match packet {
        ClientboundPlay::SetTime(time) => Some((*came, time.game_time)),
        _ => None,
    });
    let before = times.clone().take_while(|(came, _)| *came <= at).last();
    // Before the first time packet, as for a mob hatched just after a join: counted back
    // from the first one after.
    before
        .map(|(came, game_time)| game_time + ticks(came, at))
        .or_else(|| {
            times
                .clone()
                .find(|(came, _)| *came > at)
                .map(|(came, game_time)| game_time - ticks(at, came))
        })
}

/// The game time the server last told.
fn game_time(bot: &Bot) -> Option<i64> {
    bot.times.last().map(|time| time.game_time)
}

/// Waits until the server's clock has gone on by `count` ticks from now, as its time
/// packets tell it.
async fn let_ticks_pass(bot: &mut Bot, count: i64) -> Result<()> {
    wait(bot, &mut [], PATIENCE, "a time packet", |bot| {
        game_time(bot).is_some()
    })
    .await?;
    let until = game_time(bot).context("a game time")? + count;
    let what = format!("the game time {until}");
    // Time packets come each second: up to twenty ticks more than asked.
    let patience = Duration::from_millis(50 * u64::try_from(count + 40).unwrap_or(0)) + PATIENCE;
    wait(bot, &mut [], patience, &what, |bot| {
        game_time(bot).is_some_and(|now| now >= until)
    })
    .await
}

/// The ids of the entities of `kind` the bot is shown.
fn of_kind(bot: &Bot, kind: i32) -> BTreeSet<i32> {
    bot.entities
        .values()
        .filter(|entity| entity.kind == kind)
        .map(|entity| entity.entity_id)
        .collect()
}

/// Uses the egg `egg` on the top of the block `clicked` and waits until the bot is
/// shown a new entity of `kind`; returns the first made of those, by its id (a strider
/// hatched with a rider makes the rider after itself).
async fn hatch(bot: &mut Bot, egg: i32, kind: i32, clicked: (i32, i32, i32)) -> Result<i32> {
    let before = of_kind(bot, kind);
    take(bot, egg).await?;
    bot.use_item_on(clicked.0, clicked.1, clicked.2, face::TOP)
        .await?;
    let name = clustine_data::ENTITY_TYPES[usize::try_from(kind)?];
    wait(bot, &mut [], PATIENCE, &format!("a new {name}"), |bot| {
        !of_kind(bot, kind).is_subset(&before)
    })
    .await?;
    of_kind(bot, kind)
        .difference(&before)
        .next()
        .copied()
        .context("a new mob")
}

/// The entity `id` as the bot was first shown it.
fn first(bot: &Bot, id: i32) -> Result<FirstShown> {
    first_shown(recorded(bot))
        .into_iter()
        .find(|entity| entity.entity_id == id)
        .with_context(|| format!("the spawn of entity {id}"))
}

/// Every value of the data at `index` the entity `id` was sent, in order.
fn values_at(bot: &Bot, id: i32, index: u8) -> Vec<EntityDataValue> {
    recorded(bot)
        .iter()
        .filter_map(|(_, packet)| match packet {
            ClientboundPlay::SetEntityData(data) if data.entity_id == id => Some(&data.values),
            _ => None,
        })
        .flatten()
        .filter(|(at, _)| *at == index)
        .map(|(_, value)| value.clone())
        .collect()
}

/// Whether the entity `id` was ever shown on fire.
fn shown_burning(bot: &Bot, id: i32) -> bool {
    values_at(bot, id, 0)
        .iter()
        .any(|value| matches!(value, EntityDataValue::Byte(flags) if flags & ON_FIRE != 0))
}

/// Whether the entity `id` was removed.
fn removed(bot: &Bot, id: i32) -> bool {
    recorded(bot).iter().any(|(_, packet)| {
        matches!(packet, ClientboundPlay::RemoveEntities(removed) if removed.entity_ids.contains(&id))
    })
}

/// What the entity `id` wears and holds, as its `set_equipment` packets add up to: each
/// slot that holds something, with its item and count.
fn equipment(bot: &Bot, id: i32) -> BTreeMap<u8, (String, i32)> {
    let mut worn = BTreeMap::new();
    for (_, packet) in recorded(bot) {
        let ClientboundPlay::SetEquipment(set) = packet else {
            continue;
        };
        if set.entity_id != id {
            continue;
        }
        for (slot, stack) in &set.slots {
            match stack {
                Some(stack) => worn.insert(*slot, (item_name(stack.item), stack.count)),
                None => worn.remove(slot),
            };
        }
    }
    worn
}

fn item_name(item: i32) -> String {
    usize::try_from(item)
        .ok()
        .and_then(|item| ITEMS.get(item))
        .map_or_else(|| format!("item {item}"), |item| item.name.to_owned())
}

fn sound_name(sound: i32) -> String {
    usize::try_from(sound)
        .ok()
        .and_then(|sound| SOUND_EVENTS.get(sound))
        .map_or_else(|| format!("sound {sound}"), |name| (*name).to_owned())
}

/// The sounds whose names hold `family`, heard at a place no lower than `lowest` or of
/// the entity `id`, each with when it came.
fn sounds_of(bot: &Bot, family: &str, lowest: f64, id: i32) -> Vec<(Instant, String)> {
    recorded(bot)
        .iter()
        .filter_map(|(came, packet)| match packet {
            ClientboundPlay::Sound(sound) if f64::from(sound.y) / 8.0 >= lowest => {
                Some((*came, sound_name(sound.sound)))
            }
            ClientboundPlay::SoundEntity(sound) if sound.entity_id == id => {
                Some((*came, sound_name(sound.sound)))
            }
            _ => None,
        })
        .filter(|(_, name)| name.contains(family))
        .collect()
}

/// The names of `sounds` heard within a tick of `at`, by the server's clock.
fn heard_about(
    packets: &[(Instant, ClientboundPlay)],
    sounds: &[(Instant, String)],
    at: Option<i64>,
) -> BTreeSet<String> {
    let Some(at) = at else {
        return BTreeSet::new();
    };
    sounds
        .iter()
        .filter(|(came, _)| server_tick(packets, *came).is_some_and(|tick| (tick - at).abs() <= 1))
        .map(|(_, name)| name.clone())
        .collect()
}

// ---------------------------------------------------------------------------------
// 1. Bartering.
// ---------------------------------------------------------------------------------

/// The items of `gameplay/piglin_bartering` (26.3), in the file's order: what a barter
/// may give. Which one, and how many, the loot table draws.
const BARTERED: [&str; 19] = [
    "minecraft:book",
    "minecraft:iron_boots",
    "minecraft:potion",
    "minecraft:splash_potion",
    "minecraft:potion",
    "minecraft:iron_nugget",
    "minecraft:ender_pearl",
    "minecraft:dried_ghast",
    "minecraft:string",
    "minecraft:quartz",
    "minecraft:obsidian",
    "minecraft:crying_obsidian",
    "minecraft:fire_charge",
    "minecraft:leather",
    "minecraft:soul_sand",
    "minecraft:nether_brick",
    "minecraft:spectral_arrow",
    "minecraft:gravel",
    "minecraft:blackstone",
];

/// `PiglinAi.ADMIRE_DURATION`: the ticks a piglin admires a gold ingot before it
/// barters.
const ADMIRE_DURATION: i64 = 119;

/// How many piglins a scene hatches, each further along the roof, before it gives up
/// on one that is not a baby (each is one in five).
const PIGLINS_AT_MOST: usize = 6;

/// How far along the roof the scene goes from a baby: further than a piglin sees a
/// wanted item (`NearestItemSensor`, 32 blocks), so that the baby does not take the
/// ingot.
const AWAY_FROM_A_BABY: f64 = 48.0;

#[derive(Debug)]
struct Barter {
    /// How many piglins were hatched until one was an adult; shown, not compared.
    #[allow(dead_code, reason = "printed, not compared")]
    hatched: usize,
    /// Whether the piglin took the ingot (`take_item_entity`).
    picked_up: bool,
    /// The piglin's off hand each time it changed, by the item's name.
    off_hand: Vec<Option<String>>,
    /// The ticks from the piglin taking the ingot to its throwing the barter, by the
    /// server's clock.
    admired: Option<i64>,
    /// How many items the piglin threw within a second of its first.
    thrown: usize,
    /// What it threw, by name: shown, not compared, as the table draws it.
    #[allow(dead_code, reason = "printed, not compared")]
    thrown_names: Vec<String>,
    /// Whether all it threw is of the table.
    all_of_the_table: bool,
    /// The piglin's sounds heard within a tick of the taking and of the throw, by name.
    sounds_at_the_taking: BTreeSet<String>,
    sounds_at_the_throw: BTreeSet<String>,
    /// Every sound of the piglin from its hatching, by the server's tick relative to the
    /// taking; shown, not compared: its ambient sounds come when it draws them.
    #[allow(dead_code, reason = "printed, not compared")]
    sounds: Vec<(Option<i64>, String)>,
}

/// Flies the bot `dx` blocks east along the roof, at the height it is at, and waits
/// for the chunks about it.
async fn along_the_roof(bot: &mut Bot, dx: f64) -> Result<()> {
    let (x, y, z) = bot.location;
    bot.fly_to((x + dx, y, z), (-90.0, 0.0), 1.0).await?;
    wait(bot, &mut [], PATIENCE, "the chunks along the roof", |bot| {
        around_is_held(bot)
    })
    .await?;
    round_trip(bot, &mut [], teleports(bot)).await
}

/// Hatches piglins three blocks east of the bot, going on along the roof from each baby,
/// until one is an adult; returns it and how many were hatched.
async fn an_adult_piglin(bot: &mut Bot) -> Result<(i32, usize)> {
    for hatched in 1..=PIGLINS_AT_MOST {
        let (x, _, z) = feet(bot);
        let piglin = hatch(
            bot,
            items::PIGLIN_SPAWN_EGG,
            entity_types::PIGLIN,
            (x + 3, ROOF, z),
        )
        .await?;
        let baby =
            first(bot, piglin)?.value(PIGLIN_BABY_INDEX) == Some(&EntityDataValue::Boolean(true));
        if !baby {
            return Ok((piglin, hatched));
        }
        eprintln!("{}: piglin {piglin} is a baby", bot.info.profile.name);
        along_the_roof(bot, AWAY_FROM_A_BABY).await?;
    }
    anyhow::bail!("{PIGLINS_AT_MOST} piglins hatched, every one a baby")
}

async fn barter(address: &str) -> Result<Barter> {
    let mut roof = onto_the_roof(address, "Barterer").await?;
    let bot = &mut roof.bot;
    bot.record_packets();
    let (piglin, hatched) = an_adult_piglin(bot).await?;
    let hatched_at = first(bot, piglin)?.came;
    take(bot, items::GOLD_INGOT).await?;
    // Towards the piglin where it stands now, a little down, so that the ingot lands
    // between the two.
    let at = bot
        .entities
        .get(&piglin)
        .map(|entity| entity.position)
        .context("the piglin")?;
    let (dx, dz) = (at.0 - bot.location.0, at.2 - bot.location.2);
    let yaw = (-dx).atan2(dz).to_degrees() as f32;
    turn(bot, (yaw, 30.0)).await?;
    let items_before = of_kind(bot, entity_types::ITEM);
    bot.drop_held_item().await?;
    wait(bot, &mut [], PATIENCE, "the ingot thrown", |bot| {
        !of_kind(bot, entity_types::ITEM).is_subset(&items_before)
    })
    .await?;
    let ingot = of_kind(bot, entity_types::ITEM)
        .difference(&items_before)
        .next()
        .copied()
        .context("the ingot")?;
    // Up out of reach of the ingot, which a creative player would take back once its
    // delay of 40 ticks is over: the piglin's own delay is the same.
    let (x, y, z) = bot.location;
    bot.fly_to((x, y + 4.0, z), (yaw, 60.0), 1.0).await?;
    let taking = |bot: &Bot| {
        recorded(bot).iter().position(|(_, packet)| {
            matches!(packet, ClientboundPlay::TakeItemEntity(taken)
                if taken.item_id == ingot && taken.collector_id == piglin)
        })
    };
    // Its not taking the ingot is what the comparison reports.
    let _ = wait(
        bot,
        &mut [],
        PATIENCE,
        "the piglin taking the ingot",
        |bot| taking(bot).is_some(),
    )
    .await;
    let throw = |bot: &Bot| {
        let from = taking(bot)?;
        recorded(bot)[from..]
            .iter()
            .position(|(_, packet)| {
                matches!(packet, ClientboundPlay::SpawnEntity(spawn)
                    if spawn.kind == entity_types::ITEM && spawn.entity_id != ingot)
            })
            .map(|at| from + at)
    };
    if taking(bot).is_some() {
        let patience = Duration::from_millis(50 * 119 * 2) + PATIENCE;
        let _ = wait(bot, &mut [], patience, "the barter thrown", |bot| {
            throw(bot).is_some()
        })
        .await;
    }
    // A second more, for the rest of the throw and its sounds.
    let_ticks_pass(bot, 20).await?;
    let packets = recorded(bot);
    let taken_at = taking(bot).map(|at| packets[at].0);
    let thrown_at = throw(bot).map(|at| packets[at].0);
    let taken_tick = taken_at.and_then(|at| server_tick(packets, at));
    let thrown_tick = thrown_at.and_then(|at| server_tick(packets, at));
    let thrown: Vec<FirstShown> = match (thrown_at, thrown_tick) {
        (Some(from), Some(tick)) => first_shown(packets)
            .into_iter()
            .filter(|shown| {
                shown.kind == entity_types::ITEM
                    && shown.entity_id != ingot
                    && shown.came >= from
                    && server_tick(packets, shown.came).is_some_and(|at| at - tick <= 20)
            })
            .collect(),
        _ => Vec::new(),
    };
    let thrown_names: Vec<String> = thrown
        .iter()
        .map(|item| match item.value(8) {
            Some(EntityDataValue::ItemStack(Some(stack))) => item_name(stack.item),
            other => format!("{other:?}"),
        })
        .collect();
    let off_hand = packets
        .iter()
        .filter_map(|(_, packet)| match packet {
            ClientboundPlay::SetEquipment(set) if set.entity_id == piglin => Some(&set.slots),
            _ => None,
        })
        .flatten()
        .filter(|(slot, _)| *slot == OFF_HAND)
        .map(|(_, stack)| stack.as_ref().map(|stack| item_name(stack.item)))
        // An empty off hand sent with the spawn, or sent again unchanged, is no change.
        .fold(Vec::new(), |mut changes: Vec<Option<String>>, held| {
            if changes.last().unwrap_or(&None) != &held {
                changes.push(held);
            }
            changes
        });
    // The roof's top and above: a piglin of the Nether below the roof is heard there
    // too on the official server.
    let lowest = f64::from(ROOF) + 0.5;
    let sounds = sounds_of(bot, "entity.piglin.", lowest, piglin);
    let since_hatching: Vec<(Option<i64>, String)> = sounds
        .iter()
        .filter(|(came, _)| *came >= hatched_at)
        .map(|(came, name)| {
            let tick = server_tick(packets, *came);
            (
                tick.zip(taken_tick).map(|(tick, taken)| tick - taken),
                name.clone(),
            )
        })
        .collect();
    Ok(Barter {
        hatched,
        picked_up: taken_at.is_some(),
        off_hand,
        admired: taken_tick
            .zip(thrown_tick)
            .map(|(taken, thrown)| thrown - taken),
        thrown: thrown.len(),
        all_of_the_table: !thrown_names.is_empty()
            && thrown_names
                .iter()
                .all(|name| BARTERED.contains(&name.as_str())),
        thrown_names,
        sounds_at_the_taking: heard_about(packets, &sounds, taken_tick),
        sounds_at_the_throw: heard_about(packets, &sounds, thrown_tick),
        sounds: since_hatching,
    })
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_piglin_admires_a_gold_ingot_and_barters_as_on_the_official_server() {
    let (theirs, ours) = on_both(barter).await;
    let (theirs, ours) = both(theirs, ours);
    let mut differences = Differences::default();
    differences.differ("the ingot taken", theirs.picked_up == ours.picked_up);
    differences.differ("the off hand", theirs.off_hand == ours.off_hand);
    differences.differ(
        &format!(
            "the ticks admired ({:?} against {:?})",
            theirs.admired, ours.admired
        ),
        same_tick(theirs.admired, ours.admired),
    );
    differences.differ("how many items thrown", theirs.thrown == ours.thrown);
    differences.differ(
        "what was thrown is of the bartering table",
        ours.all_of_the_table,
    );
    differences.differ(
        "the sounds at the taking",
        theirs.sounds_at_the_taking == ours.sounds_at_the_taking,
    );
    differences.differ(
        "the sounds at the throw",
        theirs.sounds_at_the_throw == ours.sounds_at_the_throw,
    );
    // What the game's rules give, on the official server: if one of these fails, the
    // scene is wrong, not Clustine.
    differences.differ(
        "the official server's piglin took the ingot into its off hand and let it go",
        theirs.off_hand == [Some(item_name(items::GOLD_INGOT)), None],
    );
    differences.differ(
        "the official server's piglin admired for the game's time",
        theirs
            .admired
            .is_some_and(|admired| (admired - ADMIRE_DURATION).abs() <= 2),
    );
    differences.differ(
        "the official server's barter is of the table",
        theirs.all_of_the_table,
    );
    differences.assert_none();
}

// ---------------------------------------------------------------------------------
// The pool of lava on the roof.
// ---------------------------------------------------------------------------------

/// A pool of three by three lava sources on the roof, its floor's middle at `centre`
/// (a block of the roof), walled two high with obsidian, so that what is in it stays
/// on sources: a mob standing at the wall's foot would otherwise step over a wall one
/// high from a source's half block.
async fn lava_pool(bot: &mut Bot, centre: (i32, i32, i32)) -> Result<Vec<Option<String>>> {
    let (cx, cy, cz) = centre;
    // Low enough that the wall's farthest corner is well within reach.
    let hover = (f64::from(cx) + 0.5, f64::from(cy + 3), f64::from(cz) + 0.5);
    bot.fly_to(hover, (0.0, 90.0), 1.0).await?;
    let mut ring = Vec::new();
    for dx in -2..=2_i32 {
        for dz in -2..=2_i32 {
            if dx.abs() == 2 || dz.abs() == 2 {
                ring.push((cx + dx, cz + dz));
            }
        }
    }
    for (x, z) in &ring {
        for y in [cy + 1, cy + 2] {
            place(
                bot,
                &mut [],
                items::OBSIDIAN,
                blocks::OBSIDIAN,
                (*x, y - 1, *z),
                face::TOP,
                (*x, y, *z),
            )
            .await?;
        }
    }
    let cells: Vec<(i32, i32)> = (-1..=1)
        .flat_map(|dx| (-1..=1).map(move |dz| (cx + dx, cz + dz)))
        .collect();
    for (x, z) in &cells {
        pour(bot, &mut [], items::LAVA_BUCKET, (*x, cy, *z)).await?;
    }
    let source = |state: Option<i32>| {
        state
            .and_then(|state| u16::try_from(state).ok())
            .map(clustine_data::BlockState)
            .is_some_and(|state| {
                state.block_id() == blocks::LAVA.block_id() && state.property("level") == Some("0")
            })
    };
    let all_sources = |bot: &Bot| {
        cells
            .iter()
            .all(|(x, z)| source(bot.block_at(*x, cy + 1, *z).ok().flatten()))
    };
    // A pool that is not all sources is what the comparison reports.
    let _ = wait(
        bot,
        &mut [],
        PATIENCE,
        "the pool's nine sources",
        all_sources,
    )
    .await;
    bot.fly_to(hover, (0.0, 90.0), 1.0).await?;
    round_trip(bot, &mut [], teleports(bot)).await?;
    cells
        .iter()
        .map(|(x, z)| Ok(bot.block_at(*x, cy + 1, *z)?.map(describe)))
        .collect()
}

// ---------------------------------------------------------------------------------
// 2. A strider on lava and off it.
// ---------------------------------------------------------------------------------

#[derive(Debug)]
struct Striders {
    /// The pool's nine places as shown: all sources.
    pool: Vec<Option<String>>,
    /// The height above the roof's top a strider hatched in the pool came to rest at,
    /// and whether it was shown on the ground there.
    on_lava: Option<(f64, bool)>,
    /// The values of its suffocating flag it was sent, in order.
    on_lava_cold: Vec<EntityDataValue>,
    /// The same of a strider hatched on the bare roof.
    on_bedrock_cold: Vec<EntityDataValue>,
    /// Whether either was hatched with a rider (drawn): shown, not compared.
    #[allow(dead_code, reason = "printed, not compared")]
    riders: (bool, bool),
}

/// Whether the entity `id` has a rider, as the bot was told.
fn ridden(bot: &Bot, id: i32) -> bool {
    bot.passengers
        .get(&id)
        .is_some_and(|riders| !riders.is_empty())
}

/// Half a block above the roof's top: where a strider stands on a source of lava there.
const ON_A_SOURCE: f64 = ROOF as f64 + 1.5;

async fn striders(address: &str) -> Result<Striders> {
    let mut roof = onto_the_roof(address, "Strider").await?;
    let bot = &mut roof.bot;
    bot.record_packets();
    let (x, _, z) = roof.stand;
    let centre = (x + 5, ROOF, z);
    let pool = lava_pool(bot, centre).await?;
    let warm = hatch(bot, items::STRIDER_SPAWN_EGG, entity_types::STRIDER, centre).await?;
    let resting = |bot: &Bot| {
        bot.entities.get(&warm).is_some_and(|strider| {
            strider.on_ground && (strider.position.1 - ON_A_SOURCE).abs() < 0.01
        })
    };
    // Not coming to rest there is what the comparison reports.
    let _ = wait(
        bot,
        &mut [],
        PATIENCE,
        "the strider resting on lava",
        resting,
    )
    .await;
    let_ticks_pass(bot, 40).await?;
    let on_lava = bot
        .entities
        .get(&warm)
        .map(|strider| (strider.position.1 - f64::from(ROOF + 1), strider.on_ground));
    // On the roof's bare top, south of the pool's wall, from above it.
    let bare = (centre.0, ROOF, centre.2 + 5);
    bot.fly_to(
        (
            f64::from(bare.0) + 0.5,
            f64::from(ROOF + 3),
            f64::from(bare.2) + 0.5,
        ),
        (0.0, 90.0),
        1.0,
    )
    .await?;
    round_trip(bot, &mut [], teleports(bot)).await?;
    let cold = hatch(bot, items::STRIDER_SPAWN_EGG, entity_types::STRIDER, bare).await?;
    let shaking = |bot: &Bot| {
        values_at(bot, cold, STRIDER_SUFFOCATING_INDEX).contains(&EntityDataValue::Boolean(true))
    };
    // Not shown cold is what the comparison reports.
    let _ = wait(bot, &mut [], PATIENCE, "the strider shown cold", shaking).await;
    let_ticks_pass(bot, 40).await?;
    Ok(Striders {
        pool,
        on_lava,
        on_lava_cold: values_at(bot, warm, STRIDER_SUFFOCATING_INDEX),
        on_bedrock_cold: values_at(bot, cold, STRIDER_SUFFOCATING_INDEX),
        riders: (ridden(bot, warm), ridden(bot, cold)),
    })
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_strider_stands_on_lava_warm_and_shakes_on_bedrock_as_on_the_official_server() {
    let (theirs, ours) = on_both(striders).await;
    let (theirs, ours) = both(theirs, ours);
    let mut differences = Differences::default();
    differences.differ("the pool", theirs.pool == ours.pool);
    let height = |striders: &Striders| striders.on_lava.map(|(height, _)| height);
    differences.differ(
        &format!(
            "the height on lava ({:?} against {:?})",
            height(&theirs),
            height(&ours)
        ),
        matches!((height(&theirs), height(&ours)), (Some(theirs), Some(ours)) if (theirs - ours).abs() < 0.01),
    );
    differences.differ(
        "on the ground on lava",
        theirs.on_lava.map(|(_, ground)| ground) == ours.on_lava.map(|(_, ground)| ground),
    );
    differences.differ(
        "the suffocating flag on lava",
        theirs.on_lava_cold == ours.on_lava_cold,
    );
    differences.differ(
        "the suffocating flag on bedrock",
        theirs.on_bedrock_cold == ours.on_bedrock_cold,
    );
    // The game's rules, on the official server: if these fail, the scene is wrong.
    differences.differ(
        "the official server's strider stood half a block up on a source",
        height(&theirs).is_some_and(|height| (height - 0.5).abs() < 0.01),
    );
    differences.differ(
        "the official server's strider on lava was never shown cold",
        !theirs
            .on_lava_cold
            .contains(&EntityDataValue::Boolean(true)),
    );
    differences.differ(
        "the official server's strider on bedrock was shown cold",
        theirs
            .on_bedrock_cold
            .contains(&EntityDataValue::Boolean(true)),
    );
    differences.assert_none();
}

// ---------------------------------------------------------------------------------
// 3. Zombification in the overworld.
// ---------------------------------------------------------------------------------

#[derive(Debug)]
struct Zombification {
    /// The ticks from the piglin's spawn to the zombified piglin's, by the server's
    /// clock.
    ticks: Option<i64>,
    /// Whether the piglin was removed, and the zombified piglin made.
    piglin_removed: bool,
    zombified: bool,
    /// The piglin's immunity it was sent, in order: none, or false, while it shakes.
    immune: Vec<EntityDataValue>,
    /// The sounds of piglins heard within a tick of the change, by name.
    sounds: BTreeSet<String>,
    /// Whether the zombified piglin wears and holds what the piglin did, and is a baby
    /// where it was.
    kept_equipment: bool,
    kept_baby: bool,
    /// The zombified piglin's data at its spawn, without the left hand's bit and the
    /// baby's flag, which it has of the piglin's draws.
    values: Vec<(u8, EntityDataValue)>,
    /// What the piglin wore and held (drawn), shown, not compared.
    #[allow(dead_code, reason = "printed, not compared")]
    worn: BTreeMap<u8, (String, i32)>,
}

async fn zombification(address: &str) -> Result<Zombification> {
    let mut bot = dimensions::join(address, "Converter", &mut []).await?;
    bot.record_packets();
    let (x, _, z) = feet(&bot);
    let piglin = hatch(
        &mut bot,
        items::PIGLIN_SPAWN_EGG,
        entity_types::PIGLIN,
        (x + 3, GRASS, z),
    )
    .await?;
    let shown = first(&bot, piglin)?;
    let before = of_kind(&bot, entity_types::ZOMBIFIED_PIGLIN);
    let new_one = |bot: &Bot| {
        of_kind(bot, entity_types::ZOMBIFIED_PIGLIN)
            .difference(&before)
            .next()
            .copied()
    };
    // 300 ticks by the game's rule, fifteen seconds; not changing is what the
    // comparison reports.
    let patience = Duration::from_secs(15) + PATIENCE;
    let _ = wait(&mut bot, &mut [], patience, "the zombified piglin", |bot| {
        new_one(bot).is_some()
    })
    .await;
    let_ticks_pass(&mut bot, 20).await?;
    let packets = recorded(&bot);
    let zombified = new_one(&bot);
    let made = zombified.and_then(|id| first(&bot, id).ok());
    let spawned = server_tick(packets, shown.came);
    let changed = made
        .as_ref()
        .and_then(|made| server_tick(packets, made.came));
    let worn = equipment(&bot, piglin);
    let piglin_baby = shown.value(PIGLIN_BABY_INDEX) == Some(&EntityDataValue::Boolean(true));
    let values = made.as_ref().map_or_else(Vec::new, |made| {
        let mut values: Vec<(u8, EntityDataValue)> = made
            .values
            .iter()
            .filter(|(index, _)| *index != ZOMBIE_BABY_INDEX)
            .filter_map(|(index, value)| match (index, value) {
                (&MOB_FLAGS_INDEX, EntityDataValue::Byte(flags)) => {
                    let flags = flags & !LEFT_HANDED;
                    (flags != 0).then_some((MOB_FLAGS_INDEX, EntityDataValue::Byte(flags)))
                }
                _ => Some((*index, value.clone())),
            })
            .collect();
        values.sort_by_key(|(index, _)| *index);
        values
    });
    let lowest = f64::from(GRASS);
    let sounds = sounds_of(&bot, "piglin", lowest, piglin);
    Ok(Zombification {
        ticks: spawned.zip(changed).map(|(from, to)| to - from),
        piglin_removed: removed(&bot, piglin),
        zombified: zombified.is_some(),
        immune: values_at(&bot, piglin, PIGLIN_IMMUNE_INDEX),
        sounds: heard_about(packets, &sounds, changed),
        kept_equipment: zombified.is_some_and(|id| equipment(&bot, id) == worn),
        kept_baby: made.as_ref().is_some_and(|made| {
            (made.value(ZOMBIE_BABY_INDEX) == Some(&EntityDataValue::Boolean(true))) == piglin_baby
        }),
        values,
        worn,
    })
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_piglin_in_the_overworld_turns_into_a_zombified_piglin_as_on_the_official_server() {
    let (theirs, ours) = on_both(zombification).await;
    let (theirs, ours) = both(theirs, ours);
    let mut differences = Differences::default();
    differences.differ(
        &format!(
            "the ticks to the change ({:?} against {:?})",
            theirs.ticks, ours.ticks
        ),
        same_tick(theirs.ticks, ours.ticks),
    );
    differences.differ(
        "the piglin removed",
        theirs.piglin_removed == ours.piglin_removed,
    );
    differences.differ(
        "the zombified piglin made",
        theirs.zombified == ours.zombified,
    );
    differences.differ("the immunity sent", theirs.immune == ours.immune);
    differences.differ("the sounds of the change", theirs.sounds == ours.sounds);
    differences.differ(
        "what it wears and holds kept",
        theirs.kept_equipment == ours.kept_equipment,
    );
    differences.differ("its age kept", theirs.kept_baby == ours.kept_baby);
    differences.differ("the zombified piglin's data", theirs.values == ours.values);
    // The game's rules, on the official server.
    differences.differ(
        "the official server's piglin changed after 300 ticks",
        theirs
            .ticks
            .is_some_and(|ticks| (300..=303).contains(&ticks)),
    );
    differences.differ(
        "the official server's change sounded",
        theirs
            .sounds
            .contains("minecraft:entity.piglin.converted_to_zombified"),
    );
    differences.differ(
        "the official server's zombified piglin kept what the piglin had",
        theirs.kept_equipment && theirs.kept_baby,
    );
    differences.assert_none();
}

// ---------------------------------------------------------------------------------
// 4. Fire immunity in lava.
// ---------------------------------------------------------------------------------

/// What a mob hatched in lava was shown, from its hatching to a hundred ticks after.
#[derive(Debug, PartialEq)]
struct InLava {
    kind: String,
    burning: bool,
    removed: bool,
    hurt: usize,
    /// Whether any health it was sent differs from the first: a magma cube's is its
    /// size squared, and its size is drawn, so the values themselves are not compared.
    health_changed: bool,
}

#[derive(Debug)]
struct FireImmune {
    pool: Vec<Option<String>>,
    mobs: Vec<InLava>,
}

async fn fire_immune(address: &str) -> Result<FireImmune> {
    let mut roof = onto_the_roof(address, "Fireproof").await?;
    let bot = &mut roof.bot;
    bot.record_packets();
    let (x, _, z) = roof.stand;
    let centre = (x + 5, ROOF, z);
    let pool = lava_pool(bot, centre).await?;
    let zombified = hatch(
        bot,
        items::ZOMBIFIED_PIGLIN_SPAWN_EGG,
        entity_types::ZOMBIFIED_PIGLIN,
        (centre.0 - 1, ROOF, centre.2),
    )
    .await?;
    let cube = hatch(
        bot,
        items::MAGMA_CUBE_SPAWN_EGG,
        entity_types::MAGMA_CUBE,
        (centre.0 + 1, ROOF, centre.2),
    )
    .await?;
    let_ticks_pass(bot, 100).await?;
    let mobs = [zombified, cube]
        .into_iter()
        .map(|id| -> Result<InLava> {
            let shown = first(bot, id)?;
            let health = values_at(bot, id, HEALTH_INDEX);
            let health_changed = health.windows(2).any(|pair| pair[0] != pair[1]);
            Ok(InLava {
                kind: shown.name().to_owned(),
                burning: shown_burning(bot, id),
                removed: removed(bot, id),
                hurt: recorded(bot)
                    .iter()
                    .filter(|(_, packet)| {
                        matches!(packet, ClientboundPlay::DamageEvent(damage) if damage.entity_id == id)
                    })
                    .count(),
                health_changed,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(mobs.len() == 2, "both mobs read: {mobs:?}");
    Ok(FireImmune { pool, mobs })
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_zombified_piglin_and_a_magma_cube_live_unburnt_in_lava_as_on_the_official_server() {
    let (theirs, ours) = on_both(fire_immune).await;
    let (theirs, ours) = both(theirs, ours);
    let mut differences = Differences::default();
    differences.differ("the pool", theirs.pool == ours.pool);
    for (their, our) in theirs.mobs.iter().zip(&ours.mobs) {
        differences.differ(&format!("the {}", their.kind), their == our);
        differences.differ(
            &format!("the official server's {} unburnt and alive", their.kind),
            !their.burning && !their.removed && their.hurt == 0,
        );
    }
    differences.assert_none();
}
