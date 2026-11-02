//! Comparisons with the official server of the End (phase P, `docs/groundwork/p-plan.md`,
//! sections 19.3, 19.5 and 19.7; `p-brief.md` 2.4 and 4.3; `p-respawn-facts.md`): what a
//! player is sent when an End portal they opened takes them onto the platform, the
//! platform itself, the dragon fight's bar and the dragon's first sight, and a death in
//! the void with the respawn after it.
//!
//! These tests need Java, the server jar downloaded by `cargo datagen`, and your
//! agreement to the Minecraft EULA, so they are ignored by default. Run them with:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true tools/oracles.sh end_oracle
//! ```
//!
//! **The world.** A bot gets into the End as a player does on both servers: it places a
//! ring of end portal frames on the ground beside where it joined, fills them with eyes
//! of ender and drops in (`common/dimensions.rs`, `to_the_end`). That wants level ground
//! whatever the place a join lands on, which the official server spreads at random over
//! the spawn's radius: the flat world. And the End must be the same on both. Clustine's
//! flat world has the End of seed 0, as the game's flat world has the End of its seed;
//! the harness's flat world (`OracleWorld::FLAT`) has a seed drawn at random, and its
//! seeded world (`OracleWorld::frozen_seed`) is the game's overworld, hills and water
//! and all, where a ring of frames has nothing level to stand on. So these scenes start
//! the official server themselves, on the flat world Clustine serves (the same layers as
//! the harness's) with `level-seed=0`: the same End, island, pillars and all, on both;
//! and the same seed's hash, which the `Respawn` therefore compares too. The console
//! sets only what Clustine's tests set by themselves: no mob spawns (`spawn_mobs`
//! false), and those the flat world was made with are removed, as the harness does, so
//! that no animal stands where a frame goes. The view distance is Clustine's tests' on
//! both, so that both show the dragon from as far.
//!
//! What depends on the game's draws or the clock is not compared exactly: where a join
//! and a respawn land within the spawn's radius, where the ring is (so the portal's
//! opening is compared relative to it), the dragon's yaw and where it has flown by the
//! time the bot sees it.

mod common;
#[path = "common/dimensions.rs"]
#[allow(dead_code)]
mod dimensions;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use clustine_botswarm::{Bot, Connection};
use clustine_data::{GAME_VERSION, entity_types};
use clustine_protocol::packet_ids::play::clientbound;
use clustine_protocol::packets::play::{
    BossOperation, ClientCommand, ClientboundPlay, Respawn, SetHealth, SynchronizePlayerPosition,
    client_command,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::time::{Instant, timeout};

use dimensions::{
    END_FLOOR, END_PLATFORM, EndTravel, PATIENCE, block_name, fly_over_the_island,
    off_the_platform, ticks, to_the_end, wait,
};

// ---------------------------------------------------------------------------------
// The official server on the flat world of seed 0.
// ---------------------------------------------------------------------------------

/// How long the official server may take to start or to answer a command.
const START_TIMEOUT: Duration = Duration::from_secs(180);

/// The official server in creative on Clustine's flat world, of seed 0, with
/// Clustine's tests' view distance; `{port}` is replaced by a free port.
const PROPERTIES: &str = r#"online-mode=false
white-list=false
server-ip=127.0.0.1
server-port={port}
gamemode=creative
spawn-protection=0
enforce-secure-profile=false
motd=oracle
level-type=minecraft\:flat
generator-settings={"layers"\:[{"block"\:"minecraft\:bedrock","height"\:1},{"block"\:"minecraft\:dirt","height"\:2},{"block"\:"minecraft\:grass_block","height"\:1}],"biome"\:"minecraft\:plains"}
generate-structures=false
level-seed=0
view-distance={view}
"#;

/// The scenes of one process share the server's directory: one at a time.
static EXCLUSIVE: Mutex<()> = Mutex::const_new(());

/// A running official server, killed when dropped.
struct FlatOfSeedZero {
    address: String,
    _server: Child,
    console: ChildStdin,
    lines: UnboundedReceiver<String>,
}

impl FlatOfSeedZero {
    async fn start() -> Result<FlatOfSeedZero> {
        let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
        let jar = target
            .join("datagen")
            .join(GAME_VERSION)
            .join("server.jar")
            .canonicalize()
            .context("the server jar is missing; run `cargo datagen` to download it")?;
        let directory = target.join("oracle-end").join(GAME_VERSION);
        std::fs::create_dir_all(&directory)?;
        let agreed = std::env::var(clustine_botswarm::oracle::EULA_VARIABLE)
            .is_ok_and(|value| value == "true");
        ensure!(
            agreed,
            "the official server needs you to agree to the Minecraft EULA \
             (https://aka.ms/MinecraftEULA); if you do, set {}=true",
            clustine_botswarm::oracle::EULA_VARIABLE
        );
        std::fs::write(directory.join("eula.txt"), "eula=true\n")?;
        // A fresh world keeps runs independent of each other.
        let world = directory.join("world");
        if world.exists() {
            std::fs::remove_dir_all(&world)?;
        }
        let port = {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            listener.local_addr()?.port()
        };
        let properties = PROPERTIES
            .replace("{port}", &port.to_string())
            .replace("{view}", &common::VIEW_DISTANCE.to_string());
        std::fs::write(directory.join("server.properties"), properties)?;
        let mut server = Command::new("java")
            .args(["-Xmx1G", "-jar"])
            .arg(&jar)
            .arg("nogui")
            .current_dir(&directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("starting `java`; is a Java runtime installed?")?;
        let mut output = BufReader::new(server.stdout.take().context("no server output")?).lines();
        let console = server.stdin.take().context("no server console")?;
        let started = async {
            let mut log = Vec::new();
            while let Some(line) = output.next_line().await? {
                if line.contains("Done (") {
                    return Ok(());
                }
                log.push(line);
            }
            bail!("the official server exited:\n{}", log.join("\n"))
        };
        timeout(START_TIMEOUT, started)
            .await
            .context("the official server did not start in time")??;
        // Read on, so that the server never blocks on a full pipe; what it says
        // answers the commands.
        let (said, lines) = unbounded_channel();
        tokio::spawn(async move {
            while let Ok(Some(line)) = output.next_line().await {
                if said.send(line).is_err() {
                    break;
                }
            }
        });
        let mut oracle = FlatOfSeedZero {
            address: format!("127.0.0.1:{port}"),
            _server: server,
            console,
            lines,
        };
        // What Clustine's tests set by themselves (`common::config`).
        oracle
            .command("gamerule spawn_mobs false", &["now set to"])
            .await?;
        oracle
            .command(
                "kill @e[type=!minecraft:player]",
                &["Killed", "No entity was found"],
            )
            .await?;
        Ok(oracle)
    }

    /// Runs `command` on the console and waits for a line that holds one of `answers`.
    async fn command(&mut self, command: &str, answers: &[&str]) -> Result<String> {
        while self.lines.try_recv().is_ok() {}
        self.console
            .write_all(format!("{command}\n").as_bytes())
            .await?;
        self.console.flush().await?;
        let lines = &mut self.lines;
        let answered = async {
            loop {
                let line = lines.recv().await.context("the official server exited")?;
                ensure!(
                    !line.contains("<--[HERE]"),
                    "the official server refused `{command}`: {line}"
                );
                if answers.iter().any(|answer| line.contains(answer)) {
                    return Ok(line);
                }
            }
        };
        timeout(START_TIMEOUT, answered)
            .await
            .with_context(|| format!("the official server did not answer `{command}` in time"))?
    }
}

/// Runs `run` against the official server and then against Clustine, each on a fresh
/// flat world of seed 0, and gives what each did, the official server's first.
async fn on_both<T>(run: impl AsyncFn(&str) -> Result<T>) -> (Result<T>, Result<T>) {
    let exclusive = EXCLUSIVE.lock().await;
    let oracle = FlatOfSeedZero::start()
        .await
        .expect("the official server starts");
    let theirs = run(&oracle.address).await;
    drop(oracle);
    drop(exclusive);
    let (server, address) = common::start().await;
    let ours = run(&address).await;
    server.stop().await;
    (theirs, ours)
}

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

/// Runs a step of a scene, and on failure says so with what the bot was sent.
fn reported<T>(bot: &Bot, result: Result<T>) -> Result<T> {
    result.with_context(|| bot.report())
}

// ---------------------------------------------------------------------------------
// What the packets are, as `dimensions_oracle.rs` names them.
// ---------------------------------------------------------------------------------

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
/// time, named in `dimensions_oracle.rs`: the permission level (`EntityEvent` 24 to 28)
/// and its command tree, a line of the console's chat, the world border, the tick rate,
/// the post effects, and the inventory and held slot, which Clustine sends after the
/// placing.
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

fn registry_name(registries: &[(String, Vec<String>)], registry: &str, id: i32) -> String {
    registries
        .iter()
        .find(|(name, _)| name == registry)
        .and_then(|(_, entries)| entries.get(usize::try_from(id).ok()?))
        .cloned()
        .unwrap_or_else(|| format!("{registry} {id}"))
}

/// A `Respawn`'s fields, the seed's hash among them (both worlds are of seed 0).
fn respawn_fields(registries: &[(String, Vec<String>)], respawn: &Respawn) -> String {
    let spawn = &respawn.spawn;
    format!(
        "type {}, {}, seed's hash {}, mode {} (was {:?}), debug {}, flat {}, death {:?}, \
         cooldown {}, sea {}, keep {}",
        registry_name(registries, "minecraft:dimension_type", spawn.dimension_type),
        spawn.dimension_name,
        spawn.hashed_seed,
        spawn.game_mode,
        spawn.previous_game_mode,
        spawn.is_debug,
        spawn.is_flat,
        spawn.death_location,
        spawn.portal_cooldown,
        spawn.sea_level,
        respawn.data_to_keep,
    )
}

/// The level event of an End portal opened (`EnderEyeItem.useOn`,
/// `ServerLevel.globalLevelEvent`), and of a portal's travel (`PLAY_PORTAL_SOUND`).
const END_PORTAL_OPENED: i32 = 1038;
const PORTAL_TRAVEL: i32 = 1032;

async fn into_the_end(address: &str, name: &str) -> Result<(Bot, EndTravel)> {
    to_the_end(address, name, &mut []).await
}

// ---------------------------------------------------------------------------------
// 1. The change of dimension into the End.
// ---------------------------------------------------------------------------------

/// The packets a client is sent from its change of level (`Respawn`) up to its placing
/// on the platform, with their fields, and the portal's opening and travel sounds.
#[derive(Debug)]
struct Change {
    /// The kinds in order, without those that come of showing the new level.
    kinds: Vec<String>,
    /// How many chunks of the old level the client was told to forget after the
    /// `Respawn`, shown and not judged (the client ignores them).
    forgotten: usize,
    respawn: Option<String>,
    difficulty: Option<String>,
    /// The placing's relative flags and velocity.
    placing: Option<(i32, [f64; 3])>,
    /// The placing's place and turn: the platform's, the same in every world.
    placed_at: Option<[f64; 5]>,
    abilities: Vec<String>,
    /// The default spawn's dimension and turn.
    default_spawn: Vec<(String, f32, f32)>,
    game_events: Vec<(u8, f32)>,
    /// The portal's opening: its place relative to the ring's middle, data, whether it
    /// is heard everywhere; and how many times it was heard.
    opened: Option<String>,
    opened_times: usize,
    /// The travel's sound after the `Respawn` (`p-brief.md` 2.4: "portal sound").
    travel_sound: Option<String>,
}

async fn change_into_the_end(address: &str) -> Result<Change> {
    let (bot, travel) = into_the_end(address, "Traveller").await?;
    let packets = &travel.packets;
    let start = packets
        .iter()
        .position(|(_, packet)| matches!(packet, ClientboundPlay::Respawn(_)))
        .with_context(|| format!("no Respawn\n{}", bot.report()))?;
    let end = packets[start..]
        .iter()
        .position(|(_, packet)| matches!(packet, ClientboundPlay::SynchronizePlayerPosition(_)))
        .map_or(packets.len(), |at| start + at + 1);
    let centre = travel.ring.centre();
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
        opened: None,
        opened_times: 0,
        travel_sound: None,
    };
    for (_, packet) in &packets[..start] {
        if let ClientboundPlay::LevelEvent(event) = packet
            && event.event == END_PORTAL_OPENED
        {
            change.opened_times += 1;
            change.opened.get_or_insert_with(|| {
                format!(
                    "at {:?} from the ring's middle, data {}, global {}",
                    (
                        event.position.x - centre.0,
                        event.position.y - centre.1,
                        event.position.z - centre.2
                    ),
                    event.data,
                    event.global
                )
            });
        }
    }
    for (_, packet) in &packets[start..end] {
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
                change.respawn = Some(respawn_fields(&bot.info.registries, respawn));
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
            _ => {}
        }
    }
    change.travel_sound = packets
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(at, (_, packet))| match packet {
            ClientboundPlay::LevelEvent(event) if event.event == PORTAL_TRAVEL => Some(format!(
                "data {}, global {}, {} the placing",
                event.data,
                event.global,
                if at < end { "before" } else { "after" }
            )),
            _ => None,
        });
    Ok(change)
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_change_of_dimension_into_the_end_is_sent_as_on_the_official_server() {
    let (theirs, ours) = on_both(change_into_the_end).await;
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
    differences.differ(
        "the placing's flags and velocity",
        theirs.placing == ours.placing,
    );
    differences.differ(
        "the placing's place and turn",
        theirs.placed_at == ours.placed_at,
    );
    differences.differ("the abilities", theirs.abilities == ours.abilities);
    differences.differ(
        "the default spawn",
        theirs.default_spawn == ours.default_spawn,
    );
    differences.differ("the game events", theirs.game_events == ours.game_events);
    differences.differ(
        "the portal's opening (level event 1038)",
        theirs.opened == ours.opened && theirs.opened_times == ours.opened_times,
    );
    differences.differ(
        "the travel's sound (level event 1032)",
        theirs.travel_sound == ours.travel_sound,
    );
    println!(
        "chunks forgotten after the Respawn: official {}, Clustine {}",
        theirs.forgotten, ours.forgotten
    );
    differences.assert_none();
}

// ---------------------------------------------------------------------------------
// 2. The platform.
// ---------------------------------------------------------------------------------

/// The platform as the bot is shown it on arrival: each block of the 5 × 5 of its floor
/// and of the three layers above, by place relative to where the bot stands, and where
/// it stands.
#[derive(Debug)]
struct Platform {
    blocks: BTreeMap<(i32, i32, i32), String>,
    stands: (f64, f64, f64),
}

async fn platform(address: &str) -> Result<Platform> {
    let (bot, _) = into_the_end(address, "Lander").await?;
    let (x, y, z) = END_PLATFORM;
    let mut blocks = BTreeMap::new();
    for dy in -1..=2 {
        for dx in -2..=2 {
            for dz in -2..=2 {
                let state = bot.block_at(x + dx, y + dy, z + dz)?;
                blocks.insert(
                    (dx, dy, dz),
                    state.map_or_else(|| "not held".to_owned(), block_name),
                );
            }
        }
    }
    Ok(Platform {
        blocks,
        stands: bot.location,
    })
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn the_end_s_platform_is_made_as_on_the_official_server() {
    let (theirs, ours) = on_both(platform).await;
    let (theirs, ours) = both(theirs, ours);
    let mut differences = Differences::default();
    for (place, block) in &theirs.blocks {
        let same = ours.blocks.get(place) == Some(block);
        if !same {
            println!(
                "at {place:?} from the platform's middle: official {block}, Clustine {:?}",
                ours.blocks.get(place)
            );
        }
        differences.differ(&format!("the block at {place:?}"), same);
    }
    differences.differ("where the bot stands", theirs.stands == ours.stands);
    differences.assert_none();
}

// ---------------------------------------------------------------------------------
// 3. The fight's bar and the dragon's first sight.
// ---------------------------------------------------------------------------------

/// The bar's add and the dragon as the bot first sees it.
#[derive(Debug)]
struct Fight {
    /// The bar's name as the codec shows it, its progress, colour, overlay and flags.
    bar: (String, f32, i32, i32, u8),
    /// The dragon's spawn: its kind's name, place, and the ticks from the bot's
    /// placing on the platform until it was seen.
    dragon_kind: String,
    dragon_at: (f64, f64, f64),
    dragon_after: i64,
}

fn bar_added<'a>(
    packets: impl IntoIterator<Item = &'a (Instant, ClientboundPlay)>,
) -> Option<(String, f32, i32, i32, u8)> {
    packets.into_iter().find_map(|(_, packet)| match packet {
        ClientboundPlay::BossEvent(event) => match &event.operation {
            BossOperation::Add {
                name,
                progress,
                colour,
                overlay,
                properties,
            } => Some((
                format!("{name:?}"),
                *progress,
                *colour,
                *overlay,
                *properties,
            )),
            _ => None,
        },
        _ => None,
    })
}

/// The first dragon the bot is shown, with when.
fn dragon_spawned<'a>(
    packets: impl IntoIterator<Item = &'a (Instant, ClientboundPlay)>,
) -> Option<(Instant, i32, (f64, f64, f64))> {
    packets.into_iter().find_map(|(when, packet)| match packet {
        ClientboundPlay::SpawnEntity(spawn) if spawn.kind == entity_types::ENDER_DRAGON => {
            Some((*when, spawn.kind, (spawn.x, spawn.y, spawn.z)))
        }
        _ => None,
    })
}

async fn fight(address: &str) -> Result<Fight> {
    let (mut bot, travel) = into_the_end(address, "Watcher").await?;
    let placed = travel
        .packets
        .iter()
        .rev()
        .find_map(|(when, packet)| {
            matches!(packet, ClientboundPlay::SynchronizePlayerPosition(_)).then_some(*when)
        })
        .context("no placing")?;
    let seen =
        |bot: &Bot| bar_added(travel.packets.iter().chain(bot.packets.iter().flatten())).is_some();
    let waited = wait(&mut bot, &mut [], PATIENCE, "the dragon's bar", seen).await;
    reported(&bot, waited)?;
    let bar = bar_added(travel.packets.iter().chain(bot.packets.iter().flatten()))
        .context("seen just now")?;
    let flight = fly_over_the_island(&mut bot, &mut [], (24.5, 0.5)).await;
    reported(&bot, flight)?;
    let shown = |bot: &Bot| {
        dragon_spawned(travel.packets.iter().chain(bot.packets.iter().flatten())).is_some()
    };
    let waited = wait(&mut bot, &mut [], PATIENCE, "the dragon", shown).await;
    reported(&bot, waited)?;
    let (when, kind, at) =
        dragon_spawned(travel.packets.iter().chain(bot.packets.iter().flatten()))
            .context("seen just now")?;
    Ok(Fight {
        bar,
        dragon_kind: registry_name(&bot.info.registries, "minecraft:entity_type", kind),
        dragon_at: at,
        dragon_after: ticks(placed, when),
    })
}

/// Whether a dragon made at (0, 128, 0) when the bot came onto the platform, or later,
/// can be at `at` after `after` ticks: the holding pattern's flight is well under a
/// block and a half a tick.
fn within_first_moves(at: (f64, f64, f64), after: i64) -> bool {
    let reach = 1.5 * after.max(0) as f64 + 4.0;
    let (x, y, z) = at;
    (x * x + (y - 128.0).powi(2) + z * z).sqrt() <= reach
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn the_dragon_fight_s_bar_and_the_dragon_are_shown_as_on_the_official_server() {
    let (theirs, ours) = on_both(fight).await;
    let (theirs, ours) = both(theirs, ours);
    let mut differences = Differences::default();
    differences.differ("the bar's name", theirs.bar.0 == ours.bar.0);
    differences.differ("the bar's progress", theirs.bar.1 == ours.bar.1);
    differences.differ("the bar's colour", theirs.bar.2 == ours.bar.2);
    differences.differ("the bar's overlay", theirs.bar.3 == ours.bar.3);
    differences.differ("the bar's flags", theirs.bar.4 == ours.bar.4);
    differences.differ("the dragon's kind", theirs.dragon_kind == ours.dragon_kind);
    differences.differ(
        "the official dragon's first place is beyond its first moves from (0, 128, 0) \
         (the scene's bound is wrong)",
        within_first_moves(theirs.dragon_at, theirs.dragon_after),
    );
    differences.differ(
        "Clustine's dragon's first place is beyond its first moves from (0, 128, 0)",
        within_first_moves(ours.dragon_at, ours.dragon_after),
    );
    differences.assert_none();
}

// ---------------------------------------------------------------------------------
// 4. The void's death and the respawn.
// ---------------------------------------------------------------------------------

/// Below the End's lowest y less the void's 64, where the void hurts.
const IN_THE_VOID: f64 = (END_FLOOR - 80) as f64;

/// Besides those that come of showing a level: the clock, which both send now and
/// then; the bar, which is taken away as the bot falls out of its reach at a time each
/// server checks its own way (reported apart); and the death message in the chat,
/// which Clustine has not (19.5 item 15). And the advancements, which Clustine has not;
/// and the healths, compared apart as the hurts (`hurts`): the official server's player
/// heals between the void's hurts by their full food (`FoodData.tick`, creative too),
/// which Clustine leaves to survival, so it is hurt once more before it dies, and it
/// is told its health of nothing in the tick after its death, where Clustine tells it
/// before (after the death the edge holds what the player is told until the respawn).
const NOT_OF_THE_FALL: &[&str] = &[
    "SetTime",
    "BossEvent",
    "SystemChat",
    "minecraft:update_advancements",
    "SetHealth",
];

/// What the bot was sent from stepping off the platform until its death screen, and
/// after asking for its respawn.
#[derive(Debug)]
struct VoidDeath {
    /// The kinds in order, up to and with the death (`PlayerCombatKill`), each run of
    /// one kind as one.
    kinds: Vec<String>,
    /// What each hurt the player lived through took, in order.
    hurts: Vec<f32>,
    /// Whether the bar was taken away before the death.
    bar_removed: bool,
    message: String,
    /// The `Respawn`'s fields after `PERFORM_RESPAWN`.
    respawn: Option<String>,
    /// The respawn's place relative to the world spawn's block, and its relative
    /// flags.
    placed: Option<((f64, f64, f64), i32)>,
    health_after: Option<f32>,
}

/// What the client was sent once it asked for its respawn: the `Respawn`, and after it
/// the place and the health.
#[derive(Default)]
struct AfterTheAsk {
    respawn: Option<Respawn>,
    place: Option<SynchronizePlayerPosition>,
    health: Option<SetHealth>,
}

/// Reads what the server sends `connection` until a `Respawn` has come and, after it, a
/// place and a health of more than nothing.
async fn read_the_respawn(connection: &mut Connection) -> Result<AfterTheAsk> {
    let deadline = Instant::now() + PATIENCE;
    let mut seen = AfterTheAsk::default();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let frame = timeout(left, connection.read_frame())
            .await
            .context("the respawn did not come in time")??;
        match ClientboundPlay::decode(&frame)? {
            ClientboundPlay::Respawn(respawn) => {
                ensure!(seen.respawn.is_none(), "a second Respawn: {respawn:?}");
                seen.respawn = Some(respawn);
            }
            ClientboundPlay::SynchronizePlayerPosition(place) if seen.respawn.is_some() => {
                seen.place.get_or_insert(place);
            }
            ClientboundPlay::SetHealth(health) if seen.respawn.is_some() && health.health > 0.0 => {
                seen.health.get_or_insert(health);
            }
            _ => {}
        }
        if seen.place.is_some() && seen.health.is_some() {
            return Ok(seen);
        }
    }
}

async fn void_death(address: &str) -> Result<VoidDeath> {
    let (mut bot, _) = into_the_end(address, "Faller").await?;
    // From here what comes is the fall's.
    bot.packets = Some(Vec::new());
    let off = off_the_platform(&mut bot, &mut [], IN_THE_VOID).await;
    reported(&bot, off)?;
    let died = |bot: &Bot| !bot.deaths.is_empty();
    let waited = wait(&mut bot, &mut [], PATIENCE, "the void's death", died).await;
    reported(&bot, waited)?;
    let packets = bot.packets.take().unwrap_or_default();
    let death = packets
        .iter()
        .position(|(_, packet)| matches!(packet, ClientboundPlay::PlayerCombatKill(_)))
        .context("the death is among the packets")?;
    let fall = &packets[..=death];
    let kinds = fall
        .iter()
        .map(|(_, packet)| kind(packet))
        .filter(|kind| {
            !NOT_OF_THE_CHANGE.contains(&kind.as_str()) && !NOT_OF_THE_FALL.contains(&kind.as_str())
        })
        .collect::<Vec<_>>();
    let mut kinds = kinds;
    kinds.dedup();
    let hurts = hurts(fall);
    let bar_removed = fall.iter().any(|(_, packet)| {
        matches!(packet, ClientboundPlay::BossEvent(event) if event.operation == BossOperation::Remove)
    });
    let message = format!("{:?}", bot.deaths[0].message);
    let spawn = bot
        .world_spawn
        .as_ref()
        .map(|spawn| spawn.position)
        .context("the world spawn was told")?;
    let registries = bot.info.registries.clone();
    let mut connection = bot.into_connection();
    connection
        .write(&ClientCommand {
            action: client_command::PERFORM_RESPAWN,
        })
        .await?;
    let seen = read_the_respawn(&mut connection).await?;
    let respawn = seen
        .respawn
        .as_ref()
        .map(|respawn| respawn_fields(&registries, respawn));
    let placed = seen.place.as_ref().map(|place| {
        (
            (
                place.x - f64::from(spawn.x),
                place.y - f64::from(spawn.y),
                place.z - f64::from(spawn.z),
            ),
            place.relative_flags,
        )
    });
    drop(connection);
    Ok(VoidDeath {
        kinds,
        hurts,
        bar_removed,
        message,
        respawn,
        placed,
        health_after: seen.health.map(|health| health.health),
    })
}

/// What each hurt that left the player alive took: the health told after a
/// `DamageEvent` from the health told before it.
fn hurts(fall: &[(Instant, ClientboundPlay)]) -> Vec<f32> {
    let mut health = 20.0;
    let mut hurt = false;
    let mut taken = Vec::new();
    for (_, packet) in fall {
        match packet {
            ClientboundPlay::DamageEvent(_) => hurt = true,
            ClientboundPlay::SetHealth(told) => {
                if hurt && told.health > 0.0 {
                    taken.push(health - told.health);
                }
                hurt = false;
                health = told.health;
            }
            _ => {}
        }
    }
    taken
}

/// Whether a respawn's place, relative to the world spawn's block, is on the spawn's
/// radius (`respawn_radius`, 10 by default, `p-respawn-facts.md` 4) and at its height.
/// The official server draws the place within it at random; Clustine puts it on the
/// spawn's block (named in 19.7, item 7 of the review).
fn on_the_spawn_s_radius(placed: Option<((f64, f64, f64), i32)>, height: f64) -> bool {
    placed.is_some_and(|((x, y, z), flags)| {
        flags == 0 && x.abs() <= 10.5 && z.abs() <= 10.5 && (y - height).abs() < 1e-9
    })
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_death_in_the_end_s_void_and_its_respawn_are_sent_as_on_the_official_server() {
    let (theirs, ours) = on_both(void_death).await;
    let (theirs, ours) = both(theirs, ours);
    let mut differences = Differences::default();
    differences.differ("the kinds and their order", theirs.kinds == ours.kinds);
    let the_last = |hurts: &[f32]| hurts.len().min(4);
    differences.differ(
        "the first four hurts the player lived through",
        theirs.hurts[..the_last(&theirs.hurts)] == ours.hurts[..the_last(&ours.hurts)],
    );
    differences.differ(
        "the bar taken away before the death",
        theirs.bar_removed == ours.bar_removed,
    );
    differences.differ("the death message", theirs.message == ours.message);
    differences.differ("the Respawn's fields", theirs.respawn == ours.respawn);
    // The height is the official server's: on the flat world both stand on the grass.
    let height = theirs.placed.map_or(f64::NAN, |((_, y, _), _)| y);
    differences.differ(
        "the official respawn is off the spawn's radius (the scene is wrong)",
        on_the_spawn_s_radius(theirs.placed, height),
    );
    differences.differ(
        "the respawn's place: off the spawn's radius, or at another height or relative",
        on_the_spawn_s_radius(ours.placed, height),
    );
    differences.differ(
        "the health after the respawn",
        theirs.health_after == ours.health_after,
    );
    differences.assert_none();
}
