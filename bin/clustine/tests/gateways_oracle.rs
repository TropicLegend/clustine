//! Comparisons with the official server of the End's gateways (phase P,
//! `docs/groundwork/p-plan.md`, sections 19.5 item 9, 19.6 and 19.8; `p-brief.md` 2.6):
//! what a player is sent from the moment they go into a gateway whose exit is unknown
//! until they are placed where it sends them, the placing's flags and place, the return
//! gateway's cage at the exit, and the island's blocks about the exit.
//!
//! These tests need Java, the server jar downloaded by `cargo datagen`, and your
//! agreement to the Minecraft EULA, so they are ignored by default. Run them with:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true tools/oracles.sh gateways_oracle
//! ```
//!
//! **The world.** The exit is found in the End's outer islands, 1024 blocks out, so the
//! End must be the same on both: as in `end_oracle.rs`, the official server runs the
//! flat world Clustine serves (the same layers) with `level-seed=0`, and Clustine's flat
//! world has the End of seed 0, as the game's flat world has the End of its seed. The
//! End's terrain, its islands and the search over them are then the same on both, and
//! so is an island a search would make (its random is of its place, not of the world).
//!
//! **The gateway.** A gateway comes only of the dragon's death. On the official server
//! the console places the one the dragon's death places first, at (96, 75, 0)
//! (`place feature minecraft:end_gateway_delayed`: the cage, the gateway of age 0 with
//! no exit), once the bot is in the End. Clustine has no commands: the gateway is
//! written into its stored world, with the same blocks, between a stop and a start
//! (`dimensions::a_world_with_a_gateway`), and the bot comes into the End by the portal
//! a bot opened before. On both the bot then flies over the gateway, breaks the cage's
//! bedrock over it as a creative player can, and comes down into it
//! (`dimensions::down_into_the_gateway`).
//!
//! The console sets nothing else but what Clustine's tests set by themselves: no mob
//! spawns, and those the flat world was made with are removed.

mod common;
#[path = "common/dimensions.rs"]
#[allow(dead_code)]
mod dimensions;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use clustine_botswarm::Bot;
use clustine_data::{BlockState, GAME_VERSION, blocks};
use clustine_protocol::packet_ids::play::clientbound;
use clustine_protocol::packets::play::ClientboundPlay;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::time::timeout;

use dimensions::{
    GATEWAY, a_world_with_a_gateway, arrive, block_name, fly_over_the_island, gateway_shown,
    through_the_gateway, to_the_end,
};

// ---------------------------------------------------------------------------------
// The official server on the flat world of seed 0, as in `end_oracle.rs`.
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
        // A directory of its own, so that `end_oracle` may run beside it.
        let directory = target.join("oracle-gateways").join(GAME_VERSION);
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

/// Runs `scene` with a bot that stands on the End's platform beside the gateway at
/// [`GATEWAY`], against the official server and then against Clustine, and gives what
/// each did, the official server's first.
async fn on_both<T>(scene: impl AsyncFn(&mut Bot) -> Result<T>) -> (Result<T>, Result<T>) {
    let exclusive = EXCLUSIVE.lock().await;
    let theirs = async {
        let mut oracle = FlatOfSeedZero::start().await?;
        let (mut bot, _) = to_the_end(&oracle.address, "Traveller", &mut []).await?;
        let (x, y, z) = GATEWAY;
        let placed = oracle
            .command(
                &format!(
                    "execute in minecraft:the_end run place feature \
                     minecraft:end_gateway_delayed {x} {y} {z}"
                ),
                &["Placed", "Failed"],
            )
            .await?;
        ensure!(
            !placed.contains("Failed"),
            "the official server did not place the gateway: {placed}"
        );
        gateway_shown(&mut bot, &mut []).await?;
        let result = scene(&mut bot).await;
        drop(bot);
        drop(oracle);
        result
    }
    .await;
    drop(exclusive);
    let world = a_world_with_a_gateway().await;
    let ours = async {
        let mut bot = arrive(&world, "Traveller", &mut []).await?;
        scene(&mut bot).await
    }
    .await;
    world.server.stop().await;
    (theirs, ours)
}

/// What differs between the two servers' runs: what, and each server's value.
#[derive(Default)]
struct Differences(Vec<String>);

impl Differences {
    fn differ<T: std::fmt::Debug + PartialEq>(&mut self, what: &str, theirs: &T, ours: &T) {
        if theirs != ours {
            self.0.push(format!(
                "{what}:\n  official: {theirs:?}\n  Clustine: {ours:?}"
            ));
        }
    }

    fn assert_none(self) {
        assert!(self.0.is_empty(), "differ:\n{}", self.0.join("\n"));
    }
}

fn both<T: std::fmt::Debug>(theirs: Result<T>, ours: Result<T>) -> (T, T) {
    let theirs = theirs.unwrap_or_else(|error| panic!("on the official server: {error:#}"));
    let ours = ours.unwrap_or_else(|error| panic!("on Clustine: {error:#}"));
    println!("official: {theirs:#?}\nClustine: {ours:#?}");
    (theirs, ours)
}

// ---------------------------------------------------------------------------------
// What the packets are, as `end_oracle.rs` names them.
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

/// The kinds that come of the world going on and being shown rather than of the
/// gateway: keep-alives, the clock, the dragon's bar, chunks and entities.
const NOT_OF_THE_GATEWAY: &[&str] = &[
    "ClientboundKeepAlive",
    "SetTime",
    "BossEvent",
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

/// The look the bot keeps while it goes through.
const LOOK: (f32, f32) = (37.0, 12.0);

// ---------------------------------------------------------------------------------
// The blocks a bot is shown.
// ---------------------------------------------------------------------------------

/// The name of each block that is not air in the box from `low` to `high`, as the bot
/// is shown it, each chunk read once; fails where the bot does not hold a chunk of it.
fn shown_blocks(
    bot: &Bot,
    low: (i32, i32, i32),
    high: (i32, i32, i32),
) -> Result<BTreeMap<(i32, i32, i32), String>> {
    let mut blocks = BTreeMap::new();
    let min_y = bot.info.dimension_type()?.min_y;
    for chunk_x in low.0 >> 4..=high.0 >> 4 {
        for chunk_z in low.2 >> 4..=high.2 >> 4 {
            let sections = bot
                .sections((chunk_x, chunk_z))?
                .with_context(|| format!("the bot does not hold chunk ({chunk_x}, {chunk_z})"))?;
            let changed: BTreeMap<_, _> =
                bot.changed_blocks((chunk_x, chunk_z)).into_iter().collect();
            for x in (chunk_x * 16).max(low.0)..=(chunk_x * 16 + 15).min(high.0) {
                for z in (chunk_z * 16).max(low.2)..=(chunk_z * 16 + 15).min(high.2) {
                    for y in low.1..=high.1 {
                        let above = y - min_y;
                        let Some(section) = usize::try_from(above / 16)
                            .ok()
                            .filter(|_| above >= 0)
                            .and_then(|index| sections.get(index))
                        else {
                            continue;
                        };
                        let state = changed.get(&(x, y, z)).copied().unwrap_or_else(|| {
                            let index = (above as usize % 16) << 8
                                | (z as usize & 15) << 4
                                | (x as usize & 15);
                            section.blocks.get(index)
                        });
                        let air = u16::try_from(state).is_ok_and(|id| BlockState(id).is_air());
                        if !air {
                            blocks.insert((x, y, z), block_name(state));
                        }
                    }
                }
            }
        }
    }
    Ok(blocks)
}

// ---------------------------------------------------------------------------------
// Through the gateway.
// ---------------------------------------------------------------------------------

/// What a bot that went into the gateway was sent and shown.
#[derive(Debug)]
struct ThroughTheGateway {
    /// The kinds of the packets from the step into the gateway to the placing, the
    /// placing's included, without those of the world going on.
    kinds: Vec<String>,
    /// The block events of the gateway at [`GATEWAY`] among them: action and parameter.
    beam: Vec<(u8, u8)>,
    /// The placing's relative flags, velocity and turn.
    placing: Option<(i32, [f64; 3], (f32, f32))>,
    /// Where it was placed.
    placed_at: Option<(f64, f64, f64)>,
    /// The bot's look after the placing.
    look: (f32, f32),
    /// The gateways the bot is shown about where it came out: the return gateway.
    exits: Vec<(i32, i32, i32)>,
    /// The cage about the first of them, by place relative to it.
    cage: BTreeMap<(i32, i32, i32), String>,
    /// The blocks that are not air within 12 across of where the bot came out, from 40
    /// below it to 30 above, by their places, but for the return gateway's cage: the
    /// land the search read, and the island where it made one. By their places, so
    /// that two exits apart still compare the land the same.
    island: BTreeMap<(i32, i32, i32), String>,
}

async fn through_the_first_gateway(bot: &mut Bot) -> Result<ThroughTheGateway> {
    let (gx, _, gz) = GATEWAY;
    fly_over_the_island(bot, &mut [], (f64::from(gx) + 0.5, f64::from(gz) + 0.5)).await?;
    let cruise = bot.location.1;
    let through = through_the_gateway(bot, &mut [], GATEWAY, cruise, LOOK)
        .await
        .with_context(|| bot.report())?;
    let end = through
        .packets
        .iter()
        .position(|(_, packet)| matches!(packet, ClientboundPlay::SynchronizePlayerPosition(_)))
        .map_or(through.packets.len(), |at| at + 1);
    let gateway_block = blocks::END_GATEWAY
        .block_id()
        .and_then(|id| i32::try_from(id).ok());
    let mut seen = ThroughTheGateway {
        kinds: Vec::new(),
        beam: Vec::new(),
        placing: None,
        placed_at: None,
        look: bot.look,
        exits: Vec::new(),
        cage: BTreeMap::new(),
        island: BTreeMap::new(),
    };
    for (_, packet) in &through.packets[..end] {
        let kind = kind(packet);
        // The bot's own breaking of the bedrock over the gateway may be answered after
        // its step in, on the official server; it is the scene's, not the gateway's.
        let (gx, gy, gz) = GATEWAY;
        let over = |(x, y, z): (i32, i32, i32)| (x, z) == (gx, gz) && (y == gy + 1 || y == gy + 2);
        if matches!(packet, ClientboundPlay::BlockUpdate(update)
            if over((update.position.x, update.position.y, update.position.z)))
        {
            continue;
        }
        // The advancement a gateway gives ("Remote Getaway") and its message in the
        // chat: advancements are not built in Clustine (`docs/roadmap.md`, named there).
        if matches!(
            kind.as_str(),
            "SystemChat" | "minecraft:update_advancements"
        ) {
            continue;
        }
        if let ClientboundPlay::BlockUpdate(update) = packet {
            // Which block changed says what sent it.
            let at = &update.position;
            seen.kinds.push(format!(
                "BlockUpdate ({}, {}, {}) {}",
                at.x,
                at.y,
                at.z,
                block_name(update.state)
            ));
        } else if !NOT_OF_THE_GATEWAY.contains(&kind.as_str()) {
            seen.kinds.push(kind);
        }
        match packet {
            ClientboundPlay::BlockEvent(event)
                if (event.position.x, event.position.y, event.position.z) == GATEWAY
                    && Some(event.block) == gateway_block =>
            {
                seen.beam.push((event.action, event.param));
            }
            ClientboundPlay::SynchronizePlayerPosition(placing) if seen.placing.is_none() => {
                seen.placing = Some((
                    placing.relative_flags,
                    [placing.velocity_x, placing.velocity_y, placing.velocity_z],
                    (placing.yaw, placing.pitch),
                ));
                seen.placed_at = Some((placing.x, placing.y, placing.z));
            }
            _ => {}
        }
    }
    // The return gateway: ten above the tallest block about where the exit was found,
    // and the bot on the tallest within 5 of two above it.
    let (x, y, z) = dimensions::feet(bot);
    let about = shown_blocks(bot, (x - 6, y, z - 6), (x + 6, (y + 24).min(255), z + 6))?;
    seen.exits = about
        .iter()
        .filter(|(_, name)| name.as_str() == "minecraft:end_gateway")
        .map(|(at, _)| *at)
        .collect();
    if let Some(&(ex, ey, ez)) = seen.exits.first() {
        let cage = shown_blocks(bot, (ex - 1, ey - 2, ez - 1), (ex + 1, ey + 2, ez + 1))?;
        seen.cage = cage
            .into_iter()
            .map(|((x, y, z), name)| ((x - ex, y - ey, z - ez), name))
            .collect();
    }
    let cage = ["minecraft:end_gateway", "minecraft:bedrock"];
    seen.island = shown_blocks(
        bot,
        (x - 12, (y - 40).max(0), z - 12),
        (x + 12, (y + 30).min(255), z + 12),
    )?
    .into_iter()
    .filter(|(_, name)| !cage.contains(&name.as_str()))
    .collect();
    Ok(seen)
}

/// The places where two maps of blocks differ, with each side's block, at most `most`.
fn block_differences(
    theirs: &BTreeMap<(i32, i32, i32), String>,
    ours: &BTreeMap<(i32, i32, i32), String>,
    most: usize,
) -> Vec<String> {
    let mut places: Vec<_> = theirs.keys().chain(ours.keys()).copied().collect();
    places.sort_unstable();
    places.dedup();
    let air = "air".to_owned();
    places
        .into_iter()
        .filter(|place| theirs.get(place) != ours.get(place))
        .take(most)
        .map(|place| {
            format!(
                "{place:?}: official {}, Clustine {}",
                theirs.get(&place).unwrap_or(&air),
                ours.get(&place).unwrap_or(&air)
            )
        })
        .collect()
}

/// 19.8 and `p-brief.md` 2.6: from the step into a gateway whose exit is unknown to the
/// placing, the packets in their order (the beam's block event, no `Respawn`, the
/// teleport), the beam's action and parameter, the placing's flags, velocity and turn
/// (`DELTA ∪ ROTATION`, nought) and its place, and the look the bot has after it.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn going_into_a_gateway_is_sent_as_on_the_official_server() {
    let (theirs, ours) = on_both(through_the_first_gateway).await;
    let (theirs, ours) = both(theirs, ours);
    let mut differences = Differences::default();
    differences.differ("the kinds and their order", &theirs.kinds, &ours.kinds);
    differences.differ("the beam's block events", &theirs.beam, &ours.beam);
    differences.differ(
        "the placing's flags, velocity and turn",
        &theirs.placing,
        &ours.placing,
    );
    differences.differ("the placing's place", &theirs.placed_at, &ours.placed_at);
    differences.differ("the look after the placing", &theirs.look, &ours.look);
    differences.assert_none();
}

/// 19.8: the exit the search finds, where the return gateway stands
/// (`knownExit(from, false)`), its cage, and the blocks about it: the island the
/// search found, or made where it found none to stand on.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_gateway_s_exit_and_its_island_are_as_on_the_official_server() {
    let (theirs, ours) = on_both(through_the_first_gateway).await;
    let (theirs, ours) = both(theirs, ours);
    let mut differences = Differences::default();
    differences.differ(
        "the return gateways about where the bot came out",
        &theirs.exits,
        &ours.exits,
    );
    differences.differ("the return gateway's cage", &theirs.cage, &ours.cage);
    let island = block_differences(&theirs.island, &ours.island, 20);
    differences.differ(
        "the blocks about the exit (the first 20 places that differ)",
        &Vec::<String>::new(),
        &island,
    );
    differences.assert_none();
}
