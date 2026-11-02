//! Comparisons with the official server of the ender dragon's first moves (phase P,
//! `docs/groundwork/p-plan.md`, sections 19.2 (E9 and the fight), 19.3 and 19.5 items
//! 7, 10 and 14; `p-brief.md` 6.1, 6.3 and 6.4): what a player who comes into the End
//! and keeps still is shown of the dragon, of the crystals on the pillars, and of the
//! fight's bar, over the first half minute of the dragon's circling.
//!
//! These tests need Java, the server jar downloaded by `cargo datagen`, and your
//! agreement to the Minecraft EULA, so they are ignored by default. Run them with:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true tools/oracles.sh dragon_oracle
//! ```
//!
//! **The world** is `end_oracle.rs`'s, for its reasons: the flat world Clustine serves,
//! with `level-seed=0` on the official server, so that the End, its island and its ten
//! pillars with their crystals are the same on both; and a bot gets into the End as a
//! player does, through a ring of frames it fills with eyes (`common/dimensions.rs`,
//! `to_the_end`).
//!
//! **Where the bot keeps still.** Not on the platform at (100, 49, 0): the dragon
//! circles the island's middle 60 blocks out (`EnderDragon.findClosestNode`, the outer
//! ring) and the pillars stand 42 out, so from the platform half the circle and most
//! of the pillars are beyond what a client is shown. The bot flies from the platform
//! over the island to above its middle, at y 140 (`fly_over_the_island`), and hovers
//! there, still, for the rest of the scene. Being creative it is no target of the
//! dragon's (`DragonHoldingPatternPhase.findNewTarget` looks for a player by
//! `TargetingConditions.forCombat()`, which leaves creative players out; 19.5 item 14),
//! nor knocked back by its wings (`EnderDragon.knockBack` leaves creative players out).
//!
//! **The view distance** is 8 on both, not the tests' 3: an entity is shown within
//! `min(its tracking range, view distance × 16)` blocks across, and only in a chunk the
//! player is sent (`ChunkMap$TrackedEntity.updatePlayer`). The dragon's tracking range
//! is 10 chunks (`EntityTypes.ENDER_DRAGON`, `clientTrackingRange(10)`), so with 8 the
//! bot above the middle sees it anywhere within 128 blocks, past every place its
//! circling can take it (`FURTHEST`); with 3 it would see it only within 48.
//!
//! What the dragon's path draws at random (its yaw at its making, the heights of its
//! waypoints, which way round it goes, when it turns to land) is not compared exactly;
//! what the game's rules bound is, and each bound says the rule it comes from.

mod common;
#[path = "common/dimensions.rs"]
#[allow(dead_code)]
mod dimensions;

use std::collections::BTreeMap;
use std::f64::consts::PI;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use clustine::Config;
use clustine_botswarm::Bot;
use clustine_coordinator::Policy;
use clustine_data::{GAME_VERSION, entity_types};
use clustine_protocol::packet_ids::play::clientbound;
use clustine_protocol::packets::play::{BossOperation, ClientboundPlay, EntityDataValue, VecDelta};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::time::{Instant, timeout};

use dimensions::{PATIENCE, fly_over_the_island, to_the_end, wait};

// ---------------------------------------------------------------------------------
// The official server on the flat world of seed 0, as `end_oracle.rs` starts it, with
// the view distance of these scenes.
// ---------------------------------------------------------------------------------

/// The view distance of both servers in these scenes (the module's comment says why).
const VIEW: i32 = 8;

/// How long the official server may take to start or to answer a command.
const START_TIMEOUT: Duration = Duration::from_secs(180);

/// The official server in creative on Clustine's flat world, of seed 0; `{port}` and
/// `{view}` are replaced.
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
        // A directory of its own, so that this target and `end_oracle` may run at once.
        let directory = target.join("oracle-dragon").join(GAME_VERSION);
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
            .replace("{view}", &VIEW.to_string());
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
        // What Clustine's tests set by themselves (`common::config`): no mob comes of
        // itself, and none stands where a frame goes.
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

/// Clustine as the tests start it, with the view distance of these scenes; regions
/// follow their players by its distances where the tests' world has no pins.
fn clustine_config() -> Config {
    let config = common::config();
    Config {
        view_distance: VIEW,
        follow: config
            .follow
            .as_ref()
            .map(|_| Policy::for_view_distance(VIEW as u32)),
        ..config
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
    let (server, address) = common::start_with(clustine_config()).await;
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

    /// Notes `what` where `problem` says something is wrong, with what it says.
    fn wrong(&mut self, what: &str, problem: Option<String>) {
        if let Some(problem) = problem {
            self.0.push(format!("{what}: {problem}"));
        }
    }

    fn assert_none(self) {
        assert!(self.0.is_empty(), "differ: {:#?}", self.0);
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

/// A packet's kind: its name in the game's list where the codec does not read it, and
/// the codec's name for it where it does (as `end_oracle.rs` names them).
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

/// The entity a packet is about, for the packets that are about one entity.
fn entity_of(packet: &ClientboundPlay) -> Option<i32> {
    Some(match packet {
        ClientboundPlay::SpawnEntity(p) => p.entity_id,
        ClientboundPlay::SyncEntityPosition(p) => p.entity_id,
        ClientboundPlay::SetHeadRotation(p) => p.entity_id,
        ClientboundPlay::SetEntityData(p) => p.entity_id,
        ClientboundPlay::SetEquipment(p) => p.entity_id,
        ClientboundPlay::Animate(p) => p.entity_id,
        ClientboundPlay::MoveEntityPos(p) => p.entity_id,
        ClientboundPlay::MoveEntityPosRot(p) => p.entity_id,
        ClientboundPlay::MoveEntityRot(p) => p.entity_id,
        ClientboundPlay::SetEntityMotion(p) => p.entity_id,
        ClientboundPlay::EntityEvent(p) => p.entity_id,
        ClientboundPlay::SoundEntity(p) => p.entity_id,
        ClientboundPlay::DamageEvent(p) => p.entity_id,
        ClientboundPlay::UpdateAttributes(p) => p.entity_id,
        _ => return None,
    })
}

/// Whether a packet moves an entity: a relative move, a turn or an absolute place
/// (`ServerEntity.createMovePacket`).
fn is_move(packet: &ClientboundPlay) -> bool {
    matches!(
        packet,
        ClientboundPlay::MoveEntityPos(_)
            | ClientboundPlay::MoveEntityPosRot(_)
            | ClientboundPlay::MoveEntityRot(_)
            | ClientboundPlay::SyncEntityPosition(_)
    )
}

/// Where a relative move takes an entity from `from`, in 4096ths of a block
/// (`VecDeltaCodec`); the steps of a stepped move add up.
fn moved_by(from: (f64, f64, f64), delta: &VecDelta) -> (f64, f64, f64) {
    let total = match delta {
        VecDelta::Linear(delta) => delta.map(i64::from),
        VecDelta::Stepped(steps) => steps.iter().fold([0; 3], |sum, (_, step)| {
            [
                sum[0] + i64::from(step[0]),
                sum[1] + i64::from(step[1]),
                sum[2] + i64::from(step[2]),
            ]
        }),
    };
    (
        from.0 + total[0] as f64 / 4096.0,
        from.1 + total[1] as f64 / 4096.0,
        from.2 + total[2] as f64 / 4096.0,
    )
}

// ---------------------------------------------------------------------------------
// The game's facts the bounds come from.
// ---------------------------------------------------------------------------------

/// The index of the dragon's phase in its data (`EnderDragon.DATA_PHASE`): after the
/// eight of `Entity`, the seven of `LivingEntity` and the one of `Mob`, each class's
/// `defineId` in order.
const PHASE_INDEX: u8 = 16;

/// The phases by their ids (`EnderDragonPhase`, 0 to 10 in order).
const HOLDING_PATTERN: i32 = 0;
const STRAFE_PLAYER: i32 = 1;
const CHARGING_PLAYER: i32 = 8;

/// The index of an end crystal's beam target in its data (`EndCrystal.DATA_BEAM_TARGET`,
/// the first after `Entity`'s eight).
const BEAM_TARGET_INDEX: u8 = 8;

/// How many parts a dragon has, whose ids are the ones after its own: the server makes
/// them in its constructor, right after the dragon (each `new EnderDragonPart` takes
/// the next of `Entity.ENTITY_COUNTER`), and the client numbers them so
/// (`EnderDragon.recreateFromPacket`: part i is the packet's id + i + 1).
const PARTS: i32 = 8;

/// The ticks a scene watches the dragon circle: 30 seconds. Long enough for it to fly
/// from its making at (0, 128, 0) out to the outer ring of its nodes, 60 blocks away,
/// at up to about 1.1 blocks a tick (`TERMINAL_SPEED`), and on round a part of it, from
/// node to node: about 200 of its moves, one every three ticks, which makes the rate's
/// bound tight. Short enough to stay well inside the first minute of its life, after
/// which `ServerEntity` sends a whole position at its 401st update
/// (`createMovePacket`: `teleportDelay > 400`, one update in three ticks): what the
/// window sees is the steady flight's kinds.
const WINDOW: i64 = 600;

/// How long a scene waits for its window to pass, whatever the servers' pace.
const WINDOW_PATIENCE: Duration = Duration::from_secs(180);

/// How often a moving entity's move is sent: on every update tick, every third tick
/// for a type with the default `updateInterval(3)` (`EntityType$Builder`; the dragon's
/// builder in `EntityTypes` sets none), the gate of `ServerEntity.sendChanges`
/// (`UpdateInterval.periodic(3)`). A flying dragon moves by more than the codec's
/// least step (`7.62939453125E-6` squared) in every three ticks, so each update tick
/// sends one.
const UPDATE_INTERVAL: f64 = 3.0;

/// The fastest a dragon in its holding pattern flies, in blocks a tick. Its push is at
/// most 0.06 a tick (`moveRelative(0.06 * (f19 * f20 + 1 - f20))`, both factors at
/// most 1), and its horizontal speed is kept by at most 0.95 a tick (`0.8 + 0.15 *
/// (dot + 1) / 2`), so it tends to 0.06 × 0.95 / 0.05 = 1.14; the vertical push is
/// a hundredth of the fly speed, 0.6, kept by 0.91.
const TERMINAL_SPEED: f64 = 1.2;

/// The height band of a dragon in its holding pattern. Its waypoints are its nodes'
/// heights plus up to 20 (`DragonHoldingPatternPhase.navigateToNextPathNode`:
/// `y + nextFloat() * 20`), and a node is at `max(73, the column's
/// MOTION_BLOCKING_NO_LEAVES height + 5)`, 15 for the middle ring
/// (`EnderDragon.findClosestNode`). The lowest waypoint is so at 73; the highest stands
/// over a middle node on a pillar (the node at (40, 0) is within pillar 0's column),
/// whose top is at most 76 + 27 = 103 (`EndSpikeFeature`: heights `76 + 3v`), so 103 +
/// 15 + 20 = 138; the dragon is made at 128. It climbs and dives slowly, its vertical
/// push a hundredth of its fly speed: a few blocks either way leave room for its swing
/// past a waypoint.
const LOWEST: f64 = 73.0 - 5.0;
const HIGHEST: f64 = 138.0 + 5.0;

/// The farthest a dragon in its holding pattern goes from the island's axis. Its nodes
/// are 60, 40 and 20 blocks out (`findClosestNode`: `floor(60 cos …)`, so within
/// 60.7), it takes a node as reached within 10 blocks (`DragonHoldingPatternPhase`:
/// distance squared below 100), and it turns at most about 3.8 degrees a tick at its
/// fastest (`getTurnSpeed`: `0.7 / f / f` with `f` its speed plus one, the turn kept by
/// 0.8 and taken a tenth a tick, of at most 50 degrees), a circle of about 17 blocks'
/// radius at 1.14 a tick: past a node it swings out by at most that circle's width.
const FURTHEST: f64 = 61.0 + 10.0 + 2.0 * 17.0;

/// Where pillar `i`'s centre is (`EndSpikeFeature$SpikeCacheLoader`:
/// `floor(42 cos(2(−π + π/10 · i)))`, and the sine for z).
fn spike(i: u32) -> (i32, i32) {
    let angle = 2.0 * (-PI + 0.3141592653589793 * f64::from(i));
    (
        (42.0 * angle.cos()).floor() as i32,
        (42.0 * angle.sin()).floor() as i32,
    )
}

/// The heights of the crystals: one on each pillar, at the pillar's height plus one
/// (`EndSpikeFeature.placeSpike`: `(cx + 0.5, h + 1, cz + 0.5)`), the heights `76 + 3v`
/// for the values 0 to 9 shuffled among the pillars.
fn crystal_heights() -> Vec<i32> {
    (0..10).map(|v| 76 + 3 * v + 1).collect()
}

// ---------------------------------------------------------------------------------
// The scene.
// ---------------------------------------------------------------------------------

/// What the bot was shown of the dragon, the crystals and the bar.
#[derive(Debug)]
struct FirstMoves {
    /// The kinds of the packets about the dragon from its spawn up to its first move.
    spawned_with: Vec<String>,
    /// Its data at its spawn, each field's index and value.
    spawn_data: Option<Vec<String>>,
    /// Its attributes at its spawn.
    spawn_attributes: Option<String>,
    /// An entity the bot was shown under one of the parts' ids, or the bot's own id
    /// among them.
    in_the_parts_ids: Option<String>,
    /// The window's length in game ticks, from the clock's packets.
    ticks: i64,
    /// How many packets of each kind about the dragon came in the window.
    kinds: BTreeMap<String, usize>,
    /// The phases the dragon's data told, in order, each run of one as one, from its
    /// spawn to the window's end.
    phases: Vec<i32>,
    /// Where each move in the window put the dragon while it held its pattern.
    holding: Vec<(f64, f64, f64)>,
    /// The longest step from one move to the next while it held its pattern.
    longest_step: f64,
    /// Whether the dragon was taken from the bot's sight in the window.
    left_sight: bool,
    /// The crystals the bot was shown, by place, with the data each was told.
    crystals: BTreeMap<(i32, i32, i32), Vec<String>>,
    /// Whether every crystal stood in its block's middle.
    crystals_centred: bool,
    /// The beam targets the crystals were told, by crystal.
    beam_targets: Vec<String>,
    /// How near the dragon came to a crystal in the window.
    nearest_to_a_crystal: Option<f64>,
    /// The bar's operations, from the arrival to the window's end.
    bar: Vec<String>,
}

/// The dragon the bot is shown now, if any.
fn dragon_shown(bot: &Bot) -> bool {
    bot.entities
        .values()
        .any(|entity| entity.kind == entity_types::ENDER_DRAGON)
}

/// The game time of the first clock's packet at or after `from` in `packets`, with its
/// index.
fn clock_after(packets: &[(Instant, ClientboundPlay)], from: usize) -> Option<(usize, i64)> {
    packets
        .iter()
        .enumerate()
        .skip(from)
        .find_map(|(at, (_, packet))| match packet {
            ClientboundPlay::SetTime(time) => Some((at, time.game_time)),
            _ => None,
        })
}

/// The window, as indices into `packets`: from the first clock's packet at or after
/// `from` to the first whose game time is [`WINDOW`] later, with the ticks between.
fn window(packets: &[(Instant, ClientboundPlay)], from: usize) -> Option<(usize, usize, i64)> {
    let (start, began) = clock_after(packets, from)?;
    packets
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(at, (_, packet))| match packet {
            ClientboundPlay::SetTime(time) if time.game_time >= began + WINDOW => {
                Some((start, at, time.game_time - began))
            }
            _ => None,
        })
}

fn bar_operation(operation: &BossOperation) -> String {
    match operation {
        BossOperation::Add {
            name,
            progress,
            colour,
            overlay,
            properties,
        } => format!(
            "add {name:?}, progress {progress}, colour {colour}, overlay {overlay}, flags \
             {properties}"
        ),
        BossOperation::Remove => "remove".to_owned(),
        BossOperation::UpdateProgress(progress) => format!("progress {progress}"),
        BossOperation::UpdateName(name) => format!("name {name:?}"),
        BossOperation::UpdateStyle { colour, overlay } => {
            format!("style: colour {colour}, overlay {overlay}")
        }
        BossOperation::UpdateProperties(flags) => format!("flags {flags}"),
    }
}

fn values(values: &[(u8, EntityDataValue)]) -> Vec<String> {
    values
        .iter()
        .map(|(index, value)| format!("{index}: {value:?}"))
        .collect()
}

async fn first_moves(address: &str) -> Result<FirstMoves> {
    let (mut bot, travel) = to_the_end(address, "Watcher", &mut []).await?;
    let flight = fly_over_the_island(&mut bot, &mut [], (0.5, 0.5)).await;
    reported(&bot, flight)?;
    // From here the bot keeps still above the middle.
    let shown = wait(&mut bot, &mut [], PATIENCE, "the dragon", dragon_shown).await;
    reported(&bot, shown)?;
    let ready = bot.packets.as_ref().map_or(0, Vec::len);
    let passed = wait(
        &mut bot,
        &mut [],
        WINDOW_PATIENCE,
        "the window of the dragon's first moves",
        |bot| {
            bot.packets
                .as_deref()
                .is_some_and(|packets| window(packets, ready).is_some())
        },
    )
    .await;
    reported(&bot, passed)?;
    let after = bot.packets.take().unwrap_or_default();
    let (start, end, ticks) = window(&after, ready).context("the window passed just now")?;
    // Every packet in order, the travel's first; the window's ends shifted to match.
    let shift = travel.packets.len();
    let packets: Vec<&ClientboundPlay> = travel
        .packets
        .iter()
        .chain(after.iter())
        .map(|(_, packet)| packet)
        .collect();
    let (start, end) = (start + shift, end + shift);
    let packets = &packets[..end];

    let spawn = packets
        .iter()
        .position(|packet| {
            matches!(packet, ClientboundPlay::SpawnEntity(spawn)
                if spawn.kind == entity_types::ENDER_DRAGON)
        })
        .context("no dragon among the packets")?;
    let ClientboundPlay::SpawnEntity(spawned) = packets[spawn] else {
        unreachable!("found as a spawn");
    };
    let dragon = spawned.entity_id;

    // Its spawn: what `ServerEntity.sendPairingData` sends about it, which ends at its
    // first update. The update is told by its packets, a move or a motion (the pairing
    // sends no motion); which of the two the first update brings depends on how the
    // dragon's first tick falls against its tracker's, and is not compared.
    let mut spawned_with = Vec::new();
    let mut spawn_data = None;
    let mut spawn_attributes = None;
    for packet in &packets[spawn..] {
        if entity_of(packet) != Some(dragon) {
            continue;
        }
        if is_move(packet) || matches!(packet, ClientboundPlay::SetEntityMotion(_)) {
            break;
        }
        spawned_with.push(kind(packet));
        match packet {
            ClientboundPlay::SetEntityData(data) if spawn_data.is_none() => {
                spawn_data = Some(values(&data.values));
            }
            ClientboundPlay::UpdateAttributes(attributes) if spawn_attributes.is_none() => {
                spawn_attributes = Some(format!("{:?}", attributes.attributes));
            }
            _ => {}
        }
    }

    // The parts' ids: no other entity may have them.
    let parts = dragon + 1..=dragon + PARTS;
    let mut in_the_parts_ids = parts
        .contains(&bot.info.login.entity_id)
        .then(|| format!("the bot itself, {}", bot.info.login.entity_id));
    for packet in packets.iter() {
        if let ClientboundPlay::SpawnEntity(other) = packet
            && parts.contains(&other.entity_id)
        {
            in_the_parts_ids.get_or_insert_with(|| {
                format!(
                    "entity {} of kind {} beside the dragon {dragon}",
                    other.entity_id, other.kind
                )
            });
        }
    }

    // The dragon's flight, followed from its spawn; judged in the window.
    let mut at = (spawned.x, spawned.y, spawned.z);
    let mut phase = None;
    let mut phases = Vec::new();
    let mut kinds = BTreeMap::new();
    let mut holding = Vec::new();
    let mut longest_step = 0.0_f64;
    let mut held_at: Option<(f64, f64, f64)> = None;
    let mut left_sight = false;
    let mut flight = Vec::new();
    for (index, packet) in packets.iter().enumerate().skip(spawn) {
        let in_window = index >= start;
        if let ClientboundPlay::RemoveEntities(removed) = packet
            && removed.entity_ids.contains(&dragon)
            && in_window
        {
            left_sight = true;
        }
        if entity_of(packet) != Some(dragon) {
            continue;
        }
        match packet {
            ClientboundPlay::SpawnEntity(again) => at = (again.x, again.y, again.z),
            ClientboundPlay::SyncEntityPosition(sync) => {
                if let Some(end) = sync.path.end() {
                    at = end;
                }
            }
            ClientboundPlay::MoveEntityPos(step) => at = moved_by(at, &step.delta),
            ClientboundPlay::MoveEntityPosRot(step) => at = moved_by(at, &step.delta),
            ClientboundPlay::SetEntityData(data) => {
                for (index, value) in &data.values {
                    if *index == PHASE_INDEX
                        && let EntityDataValue::Int(told) = value
                    {
                        phase = Some(*told);
                        if phases.last() != Some(told) {
                            phases.push(*told);
                        }
                    }
                }
            }
            _ => {}
        }
        if in_window {
            *kinds.entry(kind(packet)).or_insert(0) += 1;
            if is_move(packet) {
                flight.push(at);
                if phase == Some(HOLDING_PATTERN) {
                    if let Some((x, y, z)) = held_at {
                        let step =
                            ((at.0 - x).powi(2) + (at.1 - y).powi(2) + (at.2 - z).powi(2)).sqrt();
                        longest_step = longest_step.max(step);
                    }
                    held_at = Some(at);
                    holding.push(at);
                } else {
                    held_at = None;
                }
            }
        }
    }

    // The crystals and what they were told.
    let mut crystal_ids = BTreeMap::new();
    let mut crystals = BTreeMap::new();
    let mut crystals_centred = true;
    let mut beam_targets = Vec::new();
    for packet in packets.iter() {
        match packet {
            ClientboundPlay::SpawnEntity(crystal) if crystal.kind == entity_types::END_CRYSTAL => {
                let place = (
                    crystal.x.floor() as i32,
                    crystal.y.floor() as i32,
                    crystal.z.floor() as i32,
                );
                crystals_centred &= crystal.x - crystal.x.floor() == 0.5
                    && crystal.y == crystal.y.floor()
                    && crystal.z - crystal.z.floor() == 0.5;
                crystal_ids.insert(crystal.entity_id, place);
                crystals.entry(place).or_insert_with(Vec::new);
            }
            ClientboundPlay::SetEntityData(data) => {
                if let Some(place) = crystal_ids.get(&data.entity_id) {
                    let told = crystals.entry(*place).or_insert_with(Vec::new);
                    told.extend(values(&data.values));
                    for (index, value) in &data.values {
                        if *index == BEAM_TARGET_INDEX {
                            beam_targets.push(format!("{place:?}: {value:?}"));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let nearest_to_a_crystal = flight
        .iter()
        .flat_map(|(x, y, z)| {
            crystals.keys().map(move |(cx, cy, cz)| {
                let (dx, dy, dz) = (
                    x - (f64::from(*cx) + 0.5),
                    y - f64::from(*cy),
                    z - (f64::from(*cz) + 0.5),
                );
                (dx * dx + dy * dy + dz * dz).sqrt()
            })
        })
        .reduce(f64::min);

    let bar = packets
        .iter()
        .filter_map(|packet| match packet {
            ClientboundPlay::BossEvent(event) => Some(bar_operation(&event.operation)),
            _ => None,
        })
        .collect();

    Ok(FirstMoves {
        spawned_with,
        spawn_data,
        spawn_attributes,
        in_the_parts_ids,
        ticks,
        kinds,
        phases,
        holding,
        longest_step,
        left_sight,
        crystals,
        crystals_centred,
        beam_targets,
        nearest_to_a_crystal,
        bar,
    })
}

// ---------------------------------------------------------------------------------
// The judgements, each by the game's rule, of one server's run.
// ---------------------------------------------------------------------------------

/// The moves of the window against one every [`UPDATE_INTERVAL`] ticks, with a tenth
/// and two either way for the window's ends and for an update that a server's lag
/// put on the other side of a clock's packet.
fn rate_problem(moves: &FirstMoves) -> Option<String> {
    let sent = [
        "MoveEntityPos",
        "MoveEntityPosRot",
        "MoveEntityRot",
        "SyncEntityPosition",
    ]
    .iter()
    .map(|kind| moves.kinds.get(*kind).copied().unwrap_or(0))
    .sum::<usize>() as f64;
    let expected = moves.ticks as f64 / UPDATE_INTERVAL;
    let slack = expected / 10.0 + 2.0;
    ((sent - expected).abs() > slack).then(|| {
        format!(
            "{sent} moves in {} ticks, where one every {UPDATE_INTERVAL} ticks is {expected:.0}",
            moves.ticks
        )
    })
}

/// The kinds of packets about the dragon in the window that the game's
/// `ServerEntity.sendChanges` does not send a dragon flying its holding pattern: it
/// sends moves, its motion with them (the type tracks deltas, `trackDeltas` true by
/// default), its head's turn, and its data where it changed (its phase); a whole
/// position at most once in 400 updates (`teleportDelay > 400`), or where its being on
/// the ground changes, which a dragon without physics never is.
fn kinds_problem(moves: &FirstMoves) -> Option<String> {
    const SENT: &[&str] = &[
        "MoveEntityPos",
        "MoveEntityPosRot",
        "MoveEntityRot",
        "SetEntityMotion",
        "SetHeadRotation",
        "SetEntityData",
        "SyncEntityPosition",
    ];
    let others: Vec<&String> = moves
        .kinds
        .keys()
        .filter(|kind| !SENT.contains(&kind.as_str()))
        .collect();
    let syncs = moves.kinds.get("SyncEntityPosition").copied().unwrap_or(0);
    if !others.is_empty() {
        return Some(format!("kinds no flying dragon is sent with: {others:?}"));
    }
    (syncs > 1).then(|| format!("{syncs} whole positions, where at most one comes in 400 updates"))
}

/// The dragon's motion is sent with its moves, at most once an update and, since its
/// velocity changes every tick of its flight, in most of them.
fn motion_problem(moves: &FirstMoves) -> Option<String> {
    let count = |kind: &str| moves.kinds.get(kind).copied().unwrap_or(0) as f64;
    let sent = count("MoveEntityPos") + count("MoveEntityPosRot") + count("MoveEntityRot");
    let motion = count("SetEntityMotion");
    (motion > sent + 2.0 || motion < sent / 2.0)
        .then(|| format!("{motion} motions with {sent} moves"))
}

/// The first place in the window, while the dragon held its pattern, that is out of the
/// height band or too far from the island's axis; or a step from one move to the next
/// longer than an update's three ticks at its fastest, and a little for the codec's
/// rounding.
fn flight_problem(moves: &FirstMoves) -> Option<String> {
    if moves.holding.is_empty() {
        return Some("no move while the dragon held its pattern".to_owned());
    }
    let longest = TERMINAL_SPEED * UPDATE_INTERVAL + 0.01;
    if moves.longest_step > longest {
        return Some(format!(
            "a step of {:.2} blocks from one move to the next, where three ticks at its \
             fastest are {longest:.2}",
            moves.longest_step
        ));
    }
    moves.holding.iter().find_map(|&(x, y, z)| {
        let out = x.hypot(z);
        (!(LOWEST..=HIGHEST).contains(&y) || out > FURTHEST).then(|| {
            format!(
                "at ({x:.1}, {y:.1}, {z:.1}), {out:.1} out, where the band is {LOWEST} to \
                 {HIGHEST} and the farthest {FURTHEST}"
            )
        })
    })
}

/// The phases: the dragon is made holding its pattern (`EnderDragonFight.createNewDragon`
/// sets `HOLDING_PATTERN`), and with only a creative player about it neither strafes
/// nor charges (both choose by `forCombat`).
fn phases_problem(moves: &FirstMoves) -> Option<String> {
    if moves.phases.first() != Some(&HOLDING_PATTERN) {
        return Some(format!(
            "the first phase told is {:?}, not the holding pattern",
            moves.phases.first()
        ));
    }
    moves
        .phases
        .iter()
        .any(|phase| [STRAFE_PLAYER, CHARGING_PLAYER].contains(phase))
        .then(|| format!("a phase that targets a player: {:?}", moves.phases))
}

/// The crystals against `EndSpikeFeature`: ten, one over each pillar's centre, in the
/// middle of its block, at the heights `76 + 3v + 1`, each once.
fn crystals_problem(moves: &FirstMoves) -> Option<String> {
    let places: Vec<(i32, i32, i32)> = moves.crystals.keys().copied().collect();
    let mut columns: Vec<(i32, i32)> = places.iter().map(|(x, _, z)| (*x, *z)).collect();
    columns.sort_unstable();
    let mut spikes: Vec<(i32, i32)> = (0..10).map(spike).collect();
    spikes.sort_unstable();
    let mut heights: Vec<i32> = places.iter().map(|(_, y, _)| *y).collect();
    heights.sort_unstable();
    if columns != spikes {
        return Some(format!(
            "crystals over {columns:?}, where the pillars' centres are {spikes:?}"
        ));
    }
    if heights != crystal_heights() {
        return Some(format!(
            "crystals at the heights {heights:?}, where they are {:?}",
            crystal_heights()
        ));
    }
    (!moves.crystals_centred).then(|| "a crystal off its block's middle".to_owned())
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn the_dragon_s_spawn_and_first_moves_are_sent_as_on_the_official_server() {
    let (theirs, ours) = on_both(first_moves).await;
    let (theirs, ours) = both(theirs, ours);
    let mut differences = Differences::default();

    // Its spawn: the same packets and the same data (the game's: its health of 200,
    // `setHealth(getMaxHealth())` in the constructor, and its phase 0, set by
    // `createNewDragon` over the defined default `HOVERING`, 10; only values unlike
    // their defaults are sent, `getNonDefaultValues`).
    differences.differ(
        "the kinds of the packets of the dragon's spawn",
        theirs.spawned_with == ours.spawned_with,
    );
    differences.differ(
        "the dragon's data at its spawn",
        theirs.spawn_data == ours.spawn_data,
    );
    differences.differ(
        "the dragon's attributes at its spawn",
        theirs.spawn_attributes == ours.spawn_attributes,
    );
    differences.wrong(
        "the official server shows an entity under a part's id (the scene is wrong)",
        theirs.in_the_parts_ids.clone(),
    );
    differences.wrong(
        "Clustine shows an entity under one of the ids of the dragon's eight parts",
        ours.in_the_parts_ids.clone(),
    );

    // Its first moves.
    for (server, moves) in [("the official server", &theirs), ("Clustine", &ours)] {
        let scene = if server == "Clustine" {
            ""
        } else {
            " (the scene's bound is wrong)"
        };
        differences.wrong(
            &format!("{server}'s dragon's rate of moves{scene}"),
            rate_problem(moves),
        );
        differences.wrong(
            &format!("{server}'s dragon's kinds of packets{scene}"),
            kinds_problem(moves),
        );
        differences.wrong(
            &format!("{server}'s dragon's motion{scene}"),
            motion_problem(moves),
        );
        differences.wrong(
            &format!("{server}'s dragon's holding pattern{scene}"),
            flight_problem(moves),
        );
        differences.wrong(
            &format!("{server}'s dragon's phases{scene}"),
            phases_problem(moves),
        );
        differences.wrong(
            &format!("{server}'s crystals{scene}"),
            crystals_problem(moves),
        );
        differences.wrong(
            &format!("{server}'s dragon left the bot's sight above the middle{scene}"),
            moves.left_sight.then(|| "removed in the window".to_owned()),
        );
    }

    // The crystals: the same on both, a world of one seed; and their beams, which the
    // game never sets on the pillars' crystals outside a respawn (`EndSpikeFeature`
    // sets the feature's beam target, empty in world generation, which is the data's
    // default and so not sent; only `DragonRespawnStage` beams them). The beam a player
    // sees between the dragon and its nearest crystal is the client's own drawing from
    // its copy of the dragon (`EnderDragon.checkCrystals` runs on both sides).
    differences.differ(
        "the crystals' places and data",
        theirs.crystals == ours.crystals,
    );
    differences.differ(
        "the crystals' beam targets",
        theirs.beam_targets == ours.beam_targets,
    );
    println!(
        "the dragon came within {:?} of a crystal on the official server and {:?} on \
         Clustine",
        theirs.nearest_to_a_crystal, ours.nearest_to_a_crystal
    );

    // The bar: added once with the dragon's full health, and not changed while nobody
    // hurts it (`EnderDragonFight.updateDragon` sets the progress each tick, which
    // `ServerBossEvent.setProgress` sends only when it changes; `updatePlayers` adds and
    // removes no one who stays within 192 of (0, 128, 0)).
    differences.differ("the bar's operations", theirs.bar == ours.bar);
    differences.wrong(
        "Clustine's bar",
        (ours.bar.len() != 1 || !ours.bar[0].contains("progress 1,"))
            .then(|| format!("{:?}, where one add at progress 1 is all", ours.bar)),
    );
    differences.assert_none();
}
