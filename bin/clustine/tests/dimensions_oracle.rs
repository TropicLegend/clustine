//! Comparisons with the official server of the rules that differ by dimension (phase
//! P, `docs/groundwork/p-plan.md`, sections 17.4 and 18.4; `p-brief.md`, sections 3
//! and 5): what a player is sent when a Nether portal takes them into the Nether, how
//! fast and how far lava runs there and in the overworld, a respawn anchor charged
//! and used in the overworld and in the Nether, a bed used in the Nether and water
//! poured there.
//!
//! These tests need Java, the server jar downloaded by `cargo datagen`, and your
//! agreement to the Minecraft EULA, so they are ignored by default. Run them with:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true tools/oracles.sh dimensions_oracle
//! ```
//!
//! A bot goes into the Nether as a player does on both servers, through a frame of
//! obsidian it builds and lights, and the scenes in the Nether are built on its roof of
//! bedrock, which is the same in every Nether (`common/dimensions.rs` says why and
//! how). What depends on the world's draws is not compared exactly: where the portal
//! comes out, the seed's hash, the explosions' rays and fire, the sounds' seeds and
//! drawn pitches, and a push that depends on where a player stands.

mod common;
#[path = "common/dimensions.rs"]
#[allow(dead_code)]
mod dimensions;

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use anyhow::Result;
use clustine_botswarm::Bot;
use clustine_data::{BlockState, blocks, items, sounds};
use clustine_protocol::packet_ids::play::clientbound;
use clustine_protocol::packets::play::{
    ClientboundPlay, Explode, LevelParticles, SOUND_SOURCE_BLOCKS, face,
};

use dimensions::{
    GRASS, HeardSound, PATIENCE, ROOF, block_name, click, describe, drain, empty_hand, feet, is,
    mark, on_both, onto_the_roof, place, pour, round_trip, sounds_heard, states_by_place, take,
    teleports, ticks, to_the_nether, turn, wait,
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

// ---------------------------------------------------------------------------------
// 1. The change of dimension through a Nether portal.
// ---------------------------------------------------------------------------------

/// The packets a client is sent from its change of level (`Respawn`) up to the
/// portal's sound, which ends the game's `ServerPlayer.teleport` (`p-brief.md`, 3.1),
/// with their fields that do not depend on the world's draws.
#[derive(Debug)]
struct Change {
    /// The kinds in order, without those that come of the new level's chunks and
    /// entities, of keeping alive or of the client's view's centre, which both servers
    /// send as the level is shown rather than as the player changes level.
    kinds: Vec<String>,
    /// How many chunks of the old level the client was told to forget after the
    /// `Respawn`, which the client ignores (3.1, item 5).
    forgotten: usize,
    /// The `Respawn`'s fields but the seed's hash: the dimension's type by its name,
    /// the dimension, the game mode and the one before, the debug and flat flags, the
    /// place of death, the portal's cooldown, the sea level, and what is kept.
    respawn: Option<String>,
    /// The difficulty the client was last told.
    difficulty: Option<String>,
    /// The placing's relative flags and velocity; its place and turn follow from the
    /// portal the world has.
    placing: Option<(i32, [f64; 3])>,
    /// The placing's place and turn, shown and not compared.
    placed_at: Option<[f64; 5]>,
    abilities: Vec<String>,
    /// The default spawn's dimension and turn.
    default_spawn: Vec<(String, f32, f32)>,
    game_events: Vec<(u8, f32)>,
    /// The portal's sound: its place, data and whether it is heard everywhere.
    portal_sound: Option<String>,
}

/// A packet's kind: its name in the game's list where the codec does not read it, and
/// the codec's name for it where it does.
fn kind(packet: &ClientboundPlay) -> String {
    if let ClientboundPlay::Unhandled { id } = packet {
        return usize::try_from(*id)
            .ok()
            .and_then(|id| clientbound::NAMES.get(id))
            .map_or_else(|| format!("packet {id}"), |name| (*name).to_owned());
    }
    let debug = format!("{packet:?}");
    debug
        .split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// The kinds that come of showing a level rather than of changing to it.
const NOT_OF_THE_CHANGE: &[&str] = &[
    "ClientboundKeepAlive",
    "ChunkBatchStart",
    "ChunkBatchFinished",
    "SetCenterChunk",
    "UnloadChunk",
    "SpawnEntity",
    "SyncEntityPosition",
    "SetHeadRotation",
    "RemoveEntities",
    "MoveEntityPos",
    "MoveEntityPosRot",
    "MoveEntityRot",
    "SetEntityMotion",
    "SetEntityData",
    "SetEquipment",
    "SetPassengers",
    "UpdateAttributes",
    "PlayerInfoUpdate",
    "minecraft:bundle_delimiter",
    "minecraft:chunks_biomes",
    "minecraft:forget_level_chunk",
];

/// What the official server sends in a change of dimension and Clustine sends at no
/// time, not at a join either, named: the permission level (`EntityEvent` 24 to 28)
/// and the command tree it is sent with, which a server without commands has no use
/// for; a line of its console's chat; the world border, which is the default one a
/// client makes with a level; the tick rate, which is the default 20 a second; the
/// post effects (`ServerPlayer.sendPostEffects`), which no Clustine player has.
/// Clustine sends the inventory and
/// the held slot as the destination answers for them, after the portal's sound.
const NOT_SENT_BY_CLUSTINE: &[&str] = &[
    "EntityEvent",
    "minecraft:commands",
    "SystemChat",
    "minecraft:initialize_border",
    "minecraft:ticking_state",
    "minecraft:ticking_step",
    "minecraft:post_effects",
    "SetContainerContent",
    "SetHeldSlot",
];

fn registry_name(bot: &Bot, registry: &str, id: i32) -> String {
    bot.info
        .registries
        .iter()
        .find(|(name, _)| name == registry)
        .and_then(|(_, entries)| entries.get(usize::try_from(id).ok()?))
        .cloned()
        .unwrap_or_else(|| format!("{registry} {id}"))
}

async fn change_of_dimension(address: &str) -> Result<Change> {
    let (bot, _, travel) = to_the_nether(address, "Traveller", None, &mut []).await?;
    let packets = &travel.packets;
    let start = packets
        .iter()
        .position(|(_, packet)| matches!(packet, ClientboundPlay::Respawn(_)))
        .unwrap_or(packets.len());
    let end = packets
        .iter()
        .position(|(_, packet)| {
            matches!(packet, ClientboundPlay::LevelEvent(event) if event.event == 1032)
        })
        .map_or(packets.len(), |at| at + 1)
        .max(start);
    let window = &packets[start..end];
    let mut change = Change {
        kinds: Vec::new(),
        forgotten: 0,
        respawn: None,
        difficulty: bot.difficulty.as_ref().map(|told| format!("{told:?}")),
        placing: None,
        placed_at: None,
        abilities: Vec::new(),
        default_spawn: Vec::new(),
        game_events: Vec::new(),
        portal_sound: None,
    };
    for (_, packet) in window {
        let kind = kind(packet);
        if matches!(
            packet,
            ClientboundPlay::UnloadChunk(_)
                | ClientboundPlay::Unhandled {
                    id: clientbound::FORGET_LEVEL_CHUNK
                }
        ) {
            change.forgotten += 1;
        }
        if !NOT_OF_THE_CHANGE.contains(&kind.as_str()) {
            change.kinds.push(kind);
        }
        match packet {
            ClientboundPlay::Respawn(respawn) if change.respawn.is_none() => {
                let spawn = &respawn.spawn;
                change.respawn = Some(format!(
                    "type {}, {}, mode {} (was {:?}), debug {}, flat {}, death {:?}, \
                     cooldown {}, sea {}, keep {}",
                    registry_name(&bot, "minecraft:dimension_type", spawn.dimension_type),
                    spawn.dimension_name,
                    spawn.game_mode,
                    spawn.previous_game_mode,
                    spawn.is_debug,
                    spawn.is_flat,
                    spawn.death_location,
                    spawn.portal_cooldown,
                    spawn.sea_level,
                    respawn.data_to_keep,
                ));
            }
            ClientboundPlay::SynchronizePlayerPosition(placing) if change.placing.is_none() => {
                change.placing = Some((
                    placing.relative_flags,
                    [placing.velocity_x, placing.velocity_y, placing.velocity_z],
                ));
                change.placed_at = Some([
                    placing.x,
                    placing.y,
                    placing.z,
                    f64::from(placing.yaw),
                    f64::from(placing.pitch),
                ]);
            }
            ClientboundPlay::PlayerAbilities(abilities) => {
                change.abilities.push(format!("{abilities:?}"));
            }
            ClientboundPlay::SetDefaultSpawnPosition(spawn) => {
                change
                    .default_spawn
                    .push((spawn.dimension.clone(), spawn.yaw, spawn.pitch));
            }
            ClientboundPlay::GameEvent(event) => {
                change.game_events.push((event.event, event.value));
            }
            ClientboundPlay::LevelEvent(event) if event.event == 1032 => {
                change.portal_sound = Some(format!(
                    "at {:?}, data {}, global {}",
                    event.position, event.data, event.global
                ));
            }
            _ => {}
        }
    }
    println!(
        "{}: arrived at {:?}, the portal's sound {}",
        bot.info.profile.name,
        travel.arrival,
        if travel.portal_sound {
            "came"
        } else {
            "never came"
        }
    );
    Ok(change)
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_change_of_dimension_through_a_nether_portal_is_sent_as_on_the_official_server() {
    let (theirs, ours) = on_both(change_of_dimension).await;
    let (theirs, ours) = both(theirs, ours);
    let mut differences = Differences::default();
    let sent: Vec<String> = theirs
        .kinds
        .iter()
        .filter(|kind| !NOT_SENT_BY_CLUSTINE.contains(&kind.as_str()))
        .cloned()
        .collect();
    differences.differ("the kinds and their order", sent == ours.kinds);
    differences.differ("the Respawn's fields", theirs.respawn == ours.respawn);
    differences.differ("the difficulty", theirs.difficulty == ours.difficulty);
    differences.differ("the placing's flags", theirs.placing == ours.placing);
    differences.differ("the abilities", theirs.abilities == ours.abilities);
    differences.differ(
        "the default spawn",
        theirs.default_spawn == ours.default_spawn,
    );
    differences.differ("the game events", theirs.game_events == ours.game_events);
    differences.differ(
        "the portal's sound",
        theirs.portal_sound == ours.portal_sound,
    );
    // Shown, not judged: the client ignores them (`p-brief.md`, 3.1, item 5).
    println!(
        "chunks forgotten after the Respawn: official {}, Clustine {}",
        theirs.forgotten, ours.forgotten
    );
    differences.assert_none();
}

// ---------------------------------------------------------------------------------
// 2. Lava in the Nether and in the overworld.
// ---------------------------------------------------------------------------------

/// How lava poured on a flat floor ran: each place it reached, relative to the source,
/// with the tick it first came at after the source and the states it was sent, in
/// order.
struct Spread {
    places: BTreeMap<(i32, i32, i32), (i64, Vec<String>)>,
}

/// Shown by distance from the source, which is what one reads it for; the places
/// themselves are compared one by one.
impl std::fmt::Debug for Spread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Spread")
            .field("places", &self.places.len())
            .field("rings", &self.rings())
            .finish()
    }
}

impl Spread {
    /// For each distance from the source along the floor: the earliest tick and the
    /// last states, as one line to read.
    fn rings(&self) -> BTreeMap<i32, (i64, BTreeSet<String>)> {
        let mut rings: BTreeMap<i32, (i64, BTreeSet<String>)> = BTreeMap::new();
        for ((x, _, z), (tick, states)) in &self.places {
            let ring = rings
                .entry(x.abs() + z.abs())
                .or_insert((i64::MAX, BTreeSet::new()));
            ring.0 = ring.0.min(*tick);
            if let Some(last) = states.last() {
                ring.1.insert(last.clone());
            }
        }
        rings
    }
}

/// The places lava poured at the top of `floor` reaches on a flat floor with a drop of
/// `drop_off` a block: a diamond.
fn diamond(reach: i32) -> usize {
    (1 + 2 * reach * (reach + 1)) as usize
}

/// Pours lava onto the top of `floor` and waits until as many places of the floor's
/// layer show lava as the game's spread would cover with `reach` (or, if not, until
/// the patience ends: a slower or shorter spread is what the comparison reports).
async fn pour_lava(bot: &mut Bot, floor: (i32, i32, i32), reach: i32) -> Result<Spread> {
    let origin = (floor.0, floor.1 + 1, floor.2);
    let from = mark(bot);
    pour(bot, &mut [], items::LAVA_BUCKET, floor).await?;
    let wanted = diamond(reach);
    let lava_places = |bot: &Bot| {
        states_by_place(bot, from, origin)
            .iter()
            .filter(|((_, y, _), states)| {
                *y == 0
                    && states
                        .last()
                        .is_some_and(|(_, state)| is(Some(*state), blocks::LAVA))
            })
            .count()
    };
    let _ = wait(
        bot,
        &mut [],
        Duration::from_secs(30),
        "the lava's spread",
        |bot| lava_places(bot) >= wanted,
    )
    .await;
    round_trip(bot, &mut [], teleports(bot)).await?;
    let by_place = states_by_place(bot, from, origin);
    let source = by_place
        .get(&(0, 0, 0))
        .and_then(|states| states.first())
        .map(|(when, _)| *when);
    let places = by_place
        .into_iter()
        .filter(|((_, y, _), _)| *y == 0)
        .map(|(place, states)| {
            let tick = match (source, states.first()) {
                (Some(source), Some((when, _))) => ticks(source, *when),
                _ => -1,
            };
            let states = states
                .into_iter()
                .map(|(_, state)| describe(state))
                .collect();
            (place, (tick, states))
        })
        .collect();
    Ok(Spread { places })
}

#[derive(Debug)]
struct LavaRuns {
    overworld: Spread,
    nether: Spread,
}

async fn lava_runs(address: &str) -> Result<LavaRuns> {
    let mut bot = dimensions::join(address, "Pourer", &mut []).await?;
    bot.record_heard();
    let spawn = bot.location;
    let (x, _, z) = feet(&bot);
    // Four blocks west, so that three blocks of spread stop short of the frame.
    let overworld = pour_lava(&mut bot, (x - 4, GRASS, z), 3).await?;
    bot.fly_to(spawn, (0.0, 0.0), 1.0).await?;
    let frame = dimensions::Frame::beside(&bot);
    frame.build_and_light(&mut bot, &mut []).await?;
    dimensions::enter(&mut bot, &mut [], frame).await?;
    let hole = dimensions::climb_to_the_roof(&mut bot, &mut []).await?;
    dimensions::plug(&mut bot, &mut [], hole).await?;
    let (sx, _, sz) = feet(&bot);
    let nether = pour_lava(&mut bot, (sx + 3, ROOF, sz), 7).await?;
    Ok(LavaRuns { overworld, nether })
}

fn compare_spreads(differences: &mut Differences, what: &str, theirs: &Spread, ours: &Spread) {
    println!(
        "{what}, by distance (earliest tick, last states):\n  official {:?}\n  Clustine {:?}",
        theirs.rings(),
        ours.rings()
    );
    let their_places: BTreeSet<_> = theirs.places.keys().collect();
    let our_places: BTreeSet<_> = ours.places.keys().collect();
    differences.differ(
        &format!("{what}: the places reached"),
        their_places == our_places,
    );
    for (place, (their_tick, their_states)) in &theirs.places {
        let Some((our_tick, our_states)) = ours.places.get(place) else {
            continue;
        };
        differences.differ(
            &format!("{what}: the states sent at {place:?}"),
            their_states == our_states,
        );
        // A client's clock rounds to the nearest tick, and either server's packets may
        // come a little late: one tick either way is the same tick.
        differences.differ(
            &format!("{what}: the tick at {place:?} ({their_tick} against {our_tick})"),
            (their_tick - our_tick).abs() <= 1,
        );
    }
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn lava_runs_fast_and_far_in_the_nether_and_slowly_in_the_overworld_as_on_the_official_server()
 {
    let (theirs, ours) = on_both(lava_runs).await;
    let (theirs, ours) = both(theirs, ours);
    let mut differences = Differences::default();
    compare_spreads(
        &mut differences,
        "the overworld",
        &theirs.overworld,
        &ours.overworld,
    );
    compare_spreads(&mut differences, "the Nether", &theirs.nether, &ours.nether);
    differences.assert_none();
}

// ---------------------------------------------------------------------------------
// 3. The respawn anchor.
// ---------------------------------------------------------------------------------

/// One charge of an anchor with glowstone: the anchor's state after it, and the sounds
/// heard, relative to the anchor.
#[derive(Debug, PartialEq)]
struct Charge {
    state: Option<String>,
    sounds: Vec<HeardSound>,
}

/// Places an anchor on the top of `floor` and charges it four times with glowstone.
async fn charge_anchor(bot: &mut Bot, floor: (i32, i32, i32)) -> Result<Vec<Charge>> {
    let anchor = (floor.0, floor.1 + 1, floor.2);
    place(
        bot,
        &mut [],
        items::RESPAWN_ANCHOR,
        blocks::RESPAWN_ANCHOR,
        floor,
        face::TOP,
        anchor,
    )
    .await?;
    take(bot, items::GLOWSTONE).await?;
    let mut charges = Vec::new();
    for charge in 1..=4 {
        let from = mark(bot);
        click(bot, &mut [], anchor, face::TOP).await?;
        let charged = |bot: &Bot| {
            bot.block_at(anchor.0, anchor.1, anchor.2)
                .ok()
                .flatten()
                .and_then(|state| u16::try_from(state).ok())
                .and_then(|state| BlockState(state).property("charges"))
                == Some(charge.to_string().as_str())
        };
        let what = format!("the anchor's charge {charge}");
        // A charge that never comes is what the comparison reports.
        let _ = wait(bot, &mut [], Duration::from_secs(10), &what, charged).await;
        round_trip(bot, &mut [], teleports(bot)).await?;
        charges.push(Charge {
            state: bot.block_at(anchor.0, anchor.1, anchor.2)?.map(describe),
            sounds: sounds_heard(bot, from, anchor)
                .into_iter()
                .map(|(_, sound)| sound)
                .collect(),
        });
    }
    Ok(charges)
}

/// What an explosion of a block used where it does not work showed: the packet, its
/// centre relative to the clicked block, whether the blocks used are gone, and the
/// blocks the explosion left, by name.
#[derive(Debug)]
struct Blast {
    explosion: Option<Explode>,
    centre: Option<[f64; 3]>,
    gone: bool,
    left: BTreeSet<String>,
    sounds: Vec<HeardSound>,
}

/// Clicks `at` with an empty hand and waits for the explosion and for `used` to be
/// air.
/// How far from what explodes a bed's or an anchor's blast of power 5 can change a
/// block: a ray starts with at most 1.3 times the power, 6.5, and loses 0.225 at least
/// with each step of 0.3 (`ServerExplosion.calculateExplodedPositions`), so it ends
/// within 8.7 blocks.
const BLAST_REACH: i32 = 9;

async fn blow_up(bot: &mut Bot, at: (i32, i32, i32), used: &[(i32, i32, i32)]) -> Result<Blast> {
    empty_hand(bot).await?;
    let from = mark(bot);
    let before = bot.explosions.len();
    click(bot, &mut [], at, face::TOP).await?;
    // Air, or the fire the blast may have set where the block stood.
    let gone = |bot: &Bot| {
        used.iter().all(|place| {
            let state = bot.block_at(place.0, place.1, place.2).ok().flatten();
            dimensions::is_air(state) || dimensions::is(state, blocks::FIRE)
        })
    };
    let _ = wait(
        bot,
        &mut [],
        Duration::from_secs(10),
        "the explosion",
        |bot| bot.explosions.len() > before && gone(bot),
    )
    .await;
    round_trip(bot, &mut [], teleports(bot)).await?;
    let explosion = bot.explosions.get(before).cloned();
    let centre = explosion.as_ref().map(|explosion| {
        let [x, y, z] = explosion.centre;
        [
            x - f64::from(at.0),
            y - f64::from(at.1),
            z - f64::from(at.2),
        ]
    });
    // Only what changed within the blast's reach: on the official server the bot sees
    // the land of a Nether of a random seed below the roof, where lava flows of its own
    // accord.
    let near = |(x, y, z): (i32, i32, i32)| {
        (x - at.0).abs().max((y - at.1).abs()).max((z - at.2).abs()) <= BLAST_REACH
    };
    let left = dimensions::changes(bot, from)
        .into_iter()
        .filter(|(_, position, _)| near(*position))
        .map(|(_, _, state)| block_name(state))
        .collect();
    Ok(Blast {
        explosion,
        centre,
        gone: gone(bot),
        left,
        sounds: sounds_heard(bot, from, at)
            .into_iter()
            .map(|(_, sound)| sound)
            .collect(),
    })
}

fn compare_blasts(differences: &mut Differences, what: &str, theirs: &Blast, ours: &Blast) {
    differences.differ(&format!("{what}: gone"), theirs.gone == ours.gone);
    differences.differ(
        &format!("{what}: the blocks left"),
        theirs.left == ours.left,
    );
    differences.differ(&format!("{what}: the centre"), theirs.centre == ours.centre);
    differences.differ(
        &format!("{what}: the sounds at a place"),
        theirs.sounds.len() == ours.sounds.len(),
    );
    match (&theirs.explosion, &ours.explosion) {
        (Some(their), Some(our)) => {
            differences.differ(&format!("{what}: the power"), their.radius == our.radius);
            differences.differ(
                &format!("{what}: whether there is a push"),
                their.knockback.is_some() == our.knockback.is_some(),
            );
            differences.differ(
                &format!("{what}: the particle"),
                their.particle == our.particle,
            );
            differences.differ(&format!("{what}: the sound"), their.sound == our.sound);
            differences.differ(
                &format!("{what}: the block particles"),
                their.block_particles == our.block_particles,
            );
            differences.differ(
                &format!("{what}: whether the sound plays"),
                their.play_sound == our.play_sound,
            );
            differences.differ(
                &format!("{what}: a count of positions reached"),
                their.block_count > 0 && our.block_count > 0,
            );
        }
        (None, None) => {}
        _ => differences.differ(&format!("{what}: whether it exploded"), false),
    }
}

#[derive(Debug)]
struct AnchorInTheOverworld {
    charges: Vec<Charge>,
    blast: Blast,
}

async fn anchor_in_the_overworld(address: &str) -> Result<AnchorInTheOverworld> {
    let mut bot = dimensions::join(address, "Charger", &mut []).await?;
    bot.record_heard();
    let (x, _, z) = feet(&bot);
    let floor = (x + 3, GRASS, z);
    let charges = charge_anchor(&mut bot, floor).await?;
    let anchor = (floor.0, floor.1 + 1, floor.2);
    let blast = blow_up(&mut bot, anchor, &[anchor]).await?;
    Ok(AnchorInTheOverworld { charges, blast })
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_respawn_anchor_charges_and_explodes_in_the_overworld_as_on_the_official_server() {
    let (theirs, ours) = on_both(anchor_in_the_overworld).await;
    let (theirs, ours) = both(theirs, ours);
    let mut differences = Differences::default();
    compare_charges(&mut differences, &theirs.charges, &ours.charges);
    compare_blasts(&mut differences, "the anchor", &theirs.blast, &ours.blast);
    differences.differ(
        "the anchor's explosion was of power 5",
        theirs
            .blast
            .explosion
            .as_ref()
            .is_some_and(|their| their.radius == 5.0),
    );
    differences.assert_none();
}

/// The charges compared but for the sounds' pitch, which the game draws for none of
/// them: so all of each sound.
fn compare_charges(differences: &mut Differences, theirs: &[Charge], ours: &[Charge]) {
    for (index, (their, our)) in theirs.iter().zip(ours).enumerate() {
        let charge = index + 1;
        differences.differ(
            &format!("the state after charge {charge}"),
            their.state == our.state,
        );
        differences.differ(
            &format!("the sounds of charge {charge}"),
            their.sounds == our.sounds,
        );
    }
    differences.differ("how many charges", theirs.len() == ours.len());
    differences.differ(
        "the anchor's charge sound is the game's",
        theirs.first().is_some_and(|their| {
            their
                .sounds
                .iter()
                .any(|sound| sound.sound == sounds::BLOCK_RESPAWN_ANCHOR_CHARGE)
        }),
    );
}

/// An anchor used in the Nether: its charges, then what each of two uses with an empty
/// hand sent: the messages, and the sounds relative to the anchor.
#[derive(Debug)]
struct AnchorInTheNether {
    charges: Vec<Charge>,
    first_use: (Vec<String>, Vec<HeardSound>, Option<String>),
    second_use: (Vec<String>, Vec<HeardSound>, Option<String>),
}

async fn use_empty_handed(
    bot: &mut Bot,
    anchor: (i32, i32, i32),
    wait_for_a_message: bool,
) -> Result<(Vec<String>, Vec<HeardSound>, Option<String>)> {
    empty_hand(bot).await?;
    let from = mark(bot);
    let messages = bot.messages.len();
    click(bot, &mut [], anchor, face::TOP).await?;
    if wait_for_a_message {
        // A message that never comes is what the comparison reports.
        let _ = wait(bot, &mut [], Duration::from_secs(10), "a message", |bot| {
            bot.messages.len() > messages
        })
        .await;
    }
    round_trip(bot, &mut [], teleports(bot)).await?;
    let said = bot.messages[messages..]
        .iter()
        .map(|message| format!("{message:?}"))
        .collect();
    let heard = sounds_heard(bot, from, anchor)
        .into_iter()
        .map(|(_, sound)| sound)
        .collect();
    let state = bot.block_at(anchor.0, anchor.1, anchor.2)?.map(describe);
    Ok((said, heard, state))
}

async fn anchor_in_the_nether(address: &str) -> Result<AnchorInTheNether> {
    let mut roof = onto_the_roof(address, "Settler").await?;
    let bot = &mut roof.bot;
    bot.record_heard();
    let (x, y, z) = roof.stand;
    let floor = (x + 3, y - 1, z);
    let charges = charge_anchor(bot, floor).await?;
    let anchor = (floor.0, floor.1 + 1, floor.2);
    let first_use = use_empty_handed(bot, anchor, true).await?;
    let second_use = use_empty_handed(bot, anchor, false).await?;
    Ok(AnchorInTheNether {
        charges,
        first_use,
        second_use,
    })
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_respawn_anchor_charges_and_sets_the_spawn_in_the_nether_as_on_the_official_server() {
    let (theirs, ours) = on_both(anchor_in_the_nether).await;
    let (theirs, ours) = both(theirs, ours);
    let mut differences = Differences::default();
    compare_charges(&mut differences, &theirs.charges, &ours.charges);
    differences.differ(
        "the first use's messages",
        theirs.first_use.0 == ours.first_use.0,
    );
    differences.differ(
        "the first use's sounds",
        theirs.first_use.1 == ours.first_use.1,
    );
    differences.differ(
        "the anchor after the first use",
        theirs.first_use.2 == ours.first_use.2,
    );
    differences.differ(
        "the second use's messages",
        theirs.second_use.0 == ours.second_use.0,
    );
    differences.differ(
        "the second use's sounds",
        theirs.second_use.1 == ours.second_use.1,
    );
    differences.differ(
        "the anchor after the second use",
        theirs.second_use.2 == ours.second_use.2,
    );
    differences.differ(
        "the spawn was set with a message",
        !theirs.first_use.0.is_empty(),
    );
    differences.assert_none();
}

// ---------------------------------------------------------------------------------
// 4. A bed used in the Nether.
// ---------------------------------------------------------------------------------

#[derive(Debug)]
struct BedInTheNether {
    /// The bed's two halves as placed, foot first.
    placed: Vec<Option<String>>,
    blast: Blast,
}

async fn bed_in_the_nether(address: &str) -> Result<BedInTheNether> {
    let mut roof = onto_the_roof(address, "Sleeper").await?;
    let bot = &mut roof.bot;
    bot.record_heard();
    let (x, y, z) = roof.stand;
    // Facing east, so that the bed's head is east of its foot.
    turn(bot, (-90.0, 45.0)).await?;
    let foot = (x + 2, y, z);
    let head = (x + 3, y, z);
    place(
        bot,
        &mut [],
        items::RED_BED,
        blocks::RED_BED,
        (foot.0, foot.1 - 1, foot.2),
        face::TOP,
        foot,
    )
    .await?;
    wait(bot, &mut [], PATIENCE, "the bed's head", |bot| {
        is(
            bot.block_at(head.0, head.1, head.2).ok().flatten(),
            blocks::RED_BED,
        )
    })
    .await?;
    let placed = vec![
        bot.block_at(foot.0, foot.1, foot.2)?.map(describe),
        bot.block_at(head.0, head.1, head.2)?.map(describe),
    ];
    let blast = blow_up(bot, foot, &[foot, head]).await?;
    Ok(BedInTheNether { placed, blast })
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_bed_used_in_the_nether_explodes_with_fire_as_on_the_official_server() {
    let (theirs, ours) = on_both(bed_in_the_nether).await;
    let (theirs, ours) = both(theirs, ours);
    let mut differences = Differences::default();
    differences.differ("the bed placed", theirs.placed == ours.placed);
    compare_blasts(&mut differences, "the bed", &theirs.blast, &ours.blast);
    differences.differ(
        "the bed's explosion left fire",
        theirs.blast.left.contains("minecraft:fire"),
    );
    differences.assert_none();
}

// ---------------------------------------------------------------------------------
// 5. Water poured in the Nether.
// ---------------------------------------------------------------------------------

#[derive(Debug)]
struct WaterInTheNether {
    /// The sounds the one who poured heard, relative to where the water would go.
    pourer_heard: Vec<HeardSound>,
    /// The sounds another player three blocks away heard.
    listener_heard: Vec<HeardSound>,
    /// The blocks either was sent from the pouring on, by name.
    changed: BTreeSet<String>,
    /// The particles each was shown from the pouring on, with their place relative to
    /// the water's block: the evaporation's smoke.
    smoke: (Vec<LevelParticles>, Vec<LevelParticles>),
    /// What each shows where the water would go.
    shown: (Option<String>, Option<String>),
}

async fn water_in_the_nether(address: &str) -> Result<WaterInTheNether> {
    let roof = onto_the_roof(address, "Pourer").await?;
    let (mut pourer, frame, stand) = (roof.bot, roof.frame, roof.stand);
    let mut listener = {
        let others = std::slice::from_mut(&mut pourer);
        let (mut listener, _, _) = to_the_nether(address, "Listener", Some(frame), others).await?;
        dimensions::climb_to_the_roof(&mut listener, others).await?;
        drain(others).await?;
        listener
            .fly_to(
                (
                    f64::from(stand.0) + 0.5,
                    f64::from(stand.1),
                    f64::from(stand.2) + 3.5,
                ),
                (0.0, 0.0),
                1.0,
            )
            .await?;
        let put = teleports(&listener);
        round_trip(&mut listener, others, put).await?;
        listener
    };
    pourer.record_heard();
    listener.record_heard();
    let floor = (stand.0 + 3, ROOF, stand.2);
    let water = (floor.0, floor.1 + 1, floor.2);
    let (pourer_from, listener_from) = (mark(&pourer), mark(&listener));
    let particles_from = (pourer.particles.len(), listener.particles.len());
    pour(
        &mut pourer,
        std::slice::from_mut(&mut listener),
        items::WATER_BUCKET,
        floor,
    )
    .await?;
    let heard = |bot: &Bot| {
        sounds_heard(bot, listener_from, water)
            .iter()
            .any(|(_, sound)| sound.sound == sounds::BLOCK_FIRE_EXTINGUISH)
    };
    // A sound that never comes is what the comparison reports.
    let _ = wait(
        &mut listener,
        std::slice::from_mut(&mut pourer),
        Duration::from_secs(10),
        "the water's hiss",
        heard,
    )
    .await;
    let put = teleports(&pourer);
    round_trip(&mut pourer, std::slice::from_mut(&mut listener), put).await?;
    let put = teleports(&listener);
    round_trip(&mut listener, std::slice::from_mut(&mut pourer), put).await?;
    let sounds_of = |bot: &Bot, from: usize| -> Vec<HeardSound> {
        sounds_heard(bot, from, water)
            .into_iter()
            .map(|(_, sound)| sound)
            .collect()
    };
    // Only what changed at the water's place and around it: each bot sees the land of
    // its Nether below the roof, where lava flows of its own accord.
    let near = |(x, y, z): (i32, i32, i32)| {
        (x - water.0)
            .abs()
            .max((y - water.1).abs())
            .max((z - water.2).abs())
            <= 1
    };
    let changed = dimensions::changes(&pourer, pourer_from)
        .into_iter()
        .chain(dimensions::changes(&listener, listener_from))
        .filter(|(_, position, _)| near(*position))
        .map(|(_, _, state)| block_name(state))
        .collect();
    let shown_by = |bot: &Bot| {
        bot.block_at(water.0, water.1, water.2)
            .ok()
            .flatten()
            .map(describe)
    };
    let shown_particles = |bot: &Bot, from: usize| -> Vec<LevelParticles> {
        bot.particles[from.min(bot.particles.len())..]
            .iter()
            .map(|particles| LevelParticles {
                position: [
                    particles.position[0] - f64::from(water.0),
                    particles.position[1] - f64::from(water.1),
                    particles.position[2] - f64::from(water.2),
                ],
                ..*particles
            })
            .collect()
    };
    Ok(WaterInTheNether {
        pourer_heard: sounds_of(&pourer, pourer_from),
        listener_heard: sounds_of(&listener, listener_from),
        smoke: (
            shown_particles(&pourer, particles_from.0),
            shown_particles(&listener, particles_from.1),
        ),
        changed,
        shown: (shown_by(&pourer), shown_by(&listener)),
    })
}

/// The hiss without its pitch, which the game draws (`2.6 + (a − b) × 0.8`, from 1.8
/// to 3.4); and whether the pitch is in that range.
fn hisses(heard: &[HeardSound]) -> Vec<(HeardSound, bool)> {
    heard
        .iter()
        .filter(|sound| sound.sound == sounds::BLOCK_FIRE_EXTINGUISH)
        .map(|sound| {
            let in_range = (1.8..=3.4).contains(&sound.pitch);
            (
                HeardSound {
                    pitch: 0.0,
                    ..sound.clone()
                },
                in_range,
            )
        })
        .collect()
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn water_poured_in_the_nether_hisses_for_others_and_leaves_none_as_on_the_official_server() {
    let (theirs, ours) = on_both(water_in_the_nether).await;
    let (theirs, ours) = both(theirs, ours);
    let mut differences = Differences::default();
    differences.differ(
        "the hiss the listener heard",
        hisses(&theirs.listener_heard) == hisses(&ours.listener_heard),
    );
    differences.differ(
        "the hiss the pourer heard",
        hisses(&theirs.pourer_heard) == hisses(&ours.pourer_heard),
    );
    differences.differ("the blocks changed", theirs.changed == ours.changed);
    differences.differ("the blocks shown", theirs.shown == ours.shown);
    differences.differ("the smoke", theirs.smoke == ours.smoke);
    differences.differ(
        "the listener heard a hiss of blocks",
        hisses(&theirs.listener_heard)
            .iter()
            .any(|(sound, in_range)| *in_range && sound.source == SOUND_SOURCE_BLOCKS),
    );
    differences.assert_none();
}
