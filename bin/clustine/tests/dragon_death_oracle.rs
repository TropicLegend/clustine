//! Comparisons with the official server of the ender dragon's death and of the credits
//! after it (phase P, `docs/groundwork/p-plan.md`, sections 19.2 (E9 and the fight),
//! 19.5 items 7, 8 and 11, and 19.7): a bot that plays kills the dragon on both servers,
//! and what a player beside it is shown of its death, what the death leaves on the
//! island, and what the exit portal does to a player who steps in, are compared.
//!
//! These tests need Java, the server jar downloaded by `cargo datagen`, and your
//! agreement to the Minecraft EULA, so they are ignored by default. Run them with:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true tools/oracles.sh dragon_death_oracle
//! ```
//!
//! Killing the dragon takes minutes on each server, so the scene is played once on
//! each, and the three comparisons read that one run ([`runs`]). The same scene on
//! Clustine alone, judged by the game's rules instead of against the official server,
//! is the ignored test
//! `the_dragon_killed_by_playing_dies_leaves_its_podium_and_sends_the_credits_by_the_rules`:
//!
//! ```text
//! cargo test -p clustine --test dragon_death_oracle -- --ignored by_the_rules
//! ```
//!
//! **The world** is `end_oracle.rs`'s and `dragon_oracle.rs`'s, for their reasons: the
//! flat world Clustine serves, with `level-seed=0` on the official server, so that the
//! End, its island, its pillars and the order of its gateways are the same on both; a
//! bot gets into the End through a ring of frames it fills with eyes
//! (`common/dimensions.rs`, `to_the_end`); the view distance is 8, so that the dragon
//! and the crystals are shown anywhere over the island.
//!
//! **The kill, by playing.** A creative player is no target of the dragon's (every
//! phase that looks for a player does so by `TargetingConditions.forCombat()`, which
//! leaves creative players out; 19.5 item 14), so it never strafes, charges or breathes
//! at the bot, and the bot can stand where it likes. The scene:
//!
//! 1. The bot breaks the ten end crystals on the pillars with a netherite sword, from
//!    four blocks above each (a creative player's reach to an entity is 8 from the
//!    eyes: `entity_interaction_range` 3, the creative 2 and the handlers' 3). An end
//!    crystal hurt by anything but an explosion or the dragon explodes and is removed
//!    (`EndCrystal.hurtServer`); a creative player that flies is neither hurt nor
//!    pushed by it (`ServerExplosion.hurtEntities`). Without crystals the dragon is not
//!    healed (`EnderDragon.checkCrystals`, a point every ten ticks from the nearest
//!    crystal within 32), and it lands far more often: at the end of each path of its
//!    holding pattern it turns to land with a chance of one in `crystals + 3`
//!    (`DragonHoldingPatternPhase.findNewTarget`), one in three with none, where a path
//!    is one step between nodes of its outer ring, a few seconds.
//! 2. It waits above the podium. When the dragon sits (`SITTING_SCANNING`, which with
//!    no target it keeps for 100 ticks before it takes off:
//!    `DragonSittingScanningPhase.doServerTick`), the bot moves above its head and hits
//!    the head (the part right after the dragon's id, `EnderDragon.subEntities`), which
//!    takes the whole damage where any other part takes a quarter and one
//!    (`EnderDragon.hurt(level, part, source, damage)`). A netherite sword's hit at full
//!    strength is 8 (`Player.attack`: the attack damage of 1 and the sword's 7). The bot
//!    hits once at each of the clock's packets, which come every 20 ticks: always at full
//!    strength (the sword's 12.5 ticks, `getAttackStrengthScale`) and past the hurt's
//!    cooldown (10 ticks of its 20, `LivingEntity.hurtServer`). It hits only within 80
//!    ticks of the clock's packet before the sitting was told, so that every hit lands
//!    on a sitting dragon, and at most five times a perch: 40, below the quarter of its
//!    health (50) that would send it up at once (`sittingDamageReceived`).
//! 3. The killing hit thus falls on a sitting dragon, which dies where it sits: its
//!    `handleKillingBlow` turns only a flying dragon to `DYING`, so this one goes straight
//!    into `tickDeath`, 200 ticks of rising a tenth of a block each, with its experience
//!    from the 150th tick and its removal at the 200th.
//!
//! Every wait is bounded: a perch within [`PERCH_PATIENCE`], the whole fight within
//! [`FIGHT_PATIENCE`]. The scene prints how long the kill took on each server.
//!
//! What the dragon's path and its experience draw at random is not compared exactly;
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
use clustine_botswarm::{Bot, Connection};
use clustine_coordinator::Policy;
use clustine_data::{GAME_VERSION, blocks, entity_types, items};
use clustine_protocol::packet_ids::play::clientbound;
use clustine_protocol::packets::play::{
    BossOperation, ClientCommand, ClientboundPlay, EntityDataValue, Respawn,
    SynchronizePlayerPosition, client_command, game_event,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::OnceCell;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::time::{Instant, timeout};

use dimensions::{
    END_CRUISE, OVERWORLD, PATIENCE, block_name, column_is_clear, column_report,
    fly_over_the_island, gateway_cage, is, is_air, take, ticks, to_the_end, wait,
};

// ---------------------------------------------------------------------------------
// The official server on the flat world of seed 0, as `dragon_oracle.rs` starts it.
// ---------------------------------------------------------------------------------

/// The view distance of both servers in these scenes: 8, as `dragon_oracle.rs` has it,
/// so that the dragon (tracked within 10 chunks) and the crystals (16) are shown
/// wherever they are over the island, and the gateways 96 blocks out are among the
/// chunks a bot above the middle holds.
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
        // A directory of its own, so that this target and the other End comparisons may
        // run at once.
        let directory = target.join("oracle-dragon-death").join(GAME_VERSION);
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
        // A fresh world keeps runs independent of each other: a dragon killed in an
        // earlier run would leave no egg and award 500 rather than 12000.
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
        // itself, and none stands where a frame goes. Nothing else is set by the
        // console: the dragon, its crystals and its death are the game's own.
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

/// What one server did in the scene, or what went wrong, as text so that the three
/// comparisons can share it.
type Run = std::result::Result<Whole, String>;

/// The one run of the scene on each server, the official server's first, which the
/// three comparisons share: the kill takes minutes on each.
static RUNS: OnceCell<(Run, Run)> = OnceCell::const_new();

async fn runs() -> &'static (Run, Run) {
    RUNS.get_or_init(|| async {
        let theirs = match FlatOfSeedZero::start().await {
            Ok(oracle) => {
                let run = scene(&oracle.address).await;
                drop(oracle);
                run.map_err(|error| format!("{error:#}"))
            }
            Err(error) => Err(format!("the official server did not start: {error:#}")),
        };
        let ours = on_clustine().await;
        (theirs, ours)
    })
    .await
}

/// The scene on a fresh Clustine of the flat world.
async fn on_clustine() -> Run {
    let (server, address) = common::start_with(clustine_config()).await;
    let run = scene(&address).await;
    server.stop().await;
    run.map_err(|error| format!("{error:#}"))
}

/// Collects what differs between the two servers' runs, or what breaks a rule, by name.
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

/// Both runs, each failure naming its server.
fn both(runs: &'static (Run, Run)) -> (&'static Whole, &'static Whole) {
    let theirs = runs
        .0
        .as_ref()
        .unwrap_or_else(|error| panic!("on the official server: {error}"));
    let ours = runs
        .1
        .as_ref()
        .unwrap_or_else(|error| panic!("on Clustine: {error}"));
    (theirs, ours)
}

/// The servers by name, for the judgements made of each, with what a broken rule on it
/// means: on the official server, that the scene's bound is wrong.
fn servers<'a>(theirs: &'a Whole, ours: &'a Whole) -> [(&'static str, &'a Whole); 2] {
    [
        ("the official server (the scene's rule is wrong)", theirs),
        ("Clustine", ours),
    ]
}

/// Runs a step of a scene, and on failure says so with what the bot was sent.
fn reported<T>(bot: &Bot, result: Result<T>) -> Result<T> {
    result.with_context(|| bot.report())
}

/// The packets the bot has kept.
fn packets(bot: &Bot) -> &[(Instant, ClientboundPlay)] {
    bot.packets.as_deref().unwrap_or(&[])
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

/// Whether a packet moves or turns an entity, or tells its motion
/// (`ServerEntity.sendChanges`), which comes of where it is rather than of its death.
fn is_motion(packet: &ClientboundPlay) -> bool {
    matches!(
        packet,
        ClientboundPlay::MoveEntityPos(_)
            | ClientboundPlay::MoveEntityPosRot(_)
            | ClientboundPlay::MoveEntityRot(_)
            | ClientboundPlay::SyncEntityPosition(_)
            | ClientboundPlay::SetEntityMotion(_)
            | ClientboundPlay::SetHeadRotation(_)
    )
}

/// The runs of one kind as one, in order.
fn deduplicated(kinds: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut kinds: Vec<String> = kinds.into_iter().collect();
    kinds.dedup();
    kinds
}

// ---------------------------------------------------------------------------------
// The game's facts.
// ---------------------------------------------------------------------------------

/// The index of a living entity's health in its data (`LivingEntity.DATA_HEALTH_ID`:
/// after the eight of `Entity`, its flags at 8 and its health at 9).
const HEALTH_INDEX: u8 = 9;

/// The index of the dragon's phase in its data (`EnderDragon.DATA_PHASE`, as
/// `dragon_oracle.rs` says).
const PHASE_INDEX: u8 = 16;

/// The sitting phases (`EnderDragonPhase`: 5 `SITTING_FLAMING`, 6 `SITTING_SCANNING`, 7
/// `SITTING_ATTACKING`; `isSitting` of each).
const SITTING: [i32; 3] = [5, 6, 7];

/// The index of an experience orb's worth in its data (`ExperienceOrb.DATA_VALUE`, the
/// first after `Entity`'s eight).
const ORB_VALUE_INDEX: u8 = 8;

/// The dragon's head among its parts: the first, whose id is the dragon's own plus one
/// (`EnderDragon.recreateFromPacket`, as `dragon_oracle.rs` says).
const HEAD: i32 = 1;

/// How far the head stands out from the dragon's middle along its yaw, and below it
/// while it sits (`EnderDragon.aiStep`, `tickPart(head, sin * 6.5, getHeadYOffset() + …,
/// -cos * 6.5)`, the offset −1 while sitting).
const HEAD_OUT: f64 = 6.5;

/// The entity event of a living entity's death (`LivingEntity.die`:
/// `broadcastEntityEvent(this, 3)`).
const DEATH_EVENT: i8 = 3;

/// The global level event of the dragon's death (`EnderDragon.tickDeath` at
/// `dragonDeathTime` 1: `globalLevelEvent(1028, blockPosition(), 0)`).
const DRAGON_DIED: i32 = 1028;

/// The experience of a first kill (`tickDeath`: 12000 where the fight has never killed
/// a dragon, else 500), awarded as `floor(12000 × 0.08)` at each fifth tick past the
/// 150th (155 to 200, ten times) and `floor(12000 × 0.2)` at the 200th.
const FIRST_KILL: i32 = 12_000;

/// The ticks of `tickDeath`, from the killing blow to the removal (`dragonDeathTime`
/// counted to 200), and how far the dragon rises in each (`move(SELF, (0, 0.1, 0))`;
/// the dragon has no physics, so nothing stops it).
const DYING_TICKS: i64 = 200;
const RISE_PER_TICK: f64 = 0.1;

/// How many orbs the official server may merge into others before they are shown
/// (`ExperienceOrb.tryMergeToExisting`: an orb of the same worth within half a block,
/// whose id is the draw of one in 40). An award of 960 makes two pairs of the same
/// worth at one place: a few merges at the most in eleven awards.
const MERGES_AT_MOST: usize = 8;

/// `ExperienceOrb.getExperienceValue`: the largest of the orb sizes that fits.
fn experience_value(amount: i32) -> i32 {
    [2477, 1237, 617, 307, 149, 73, 37, 17, 7, 3]
        .into_iter()
        .find(|&size| amount >= size)
        .unwrap_or(1)
}

/// The orbs one award of `amount` makes (`ExperienceOrb.award`: the largest size that
/// fits, again and again, until nothing is left).
fn orbs_of(mut amount: i32) -> Vec<i32> {
    let mut orbs = Vec::new();
    while amount > 0 {
        let value = experience_value(amount);
        amount -= value;
        orbs.push(value);
    }
    orbs
}

/// Every orb a first kill makes, before any merge: ten awards of 960 and one of 2400.
fn first_kill_orbs() -> Vec<i32> {
    let mut orbs: Vec<i32> = (0..10)
        .flat_map(|_| orbs_of(FIRST_KILL * 8 / 100))
        .chain(orbs_of(FIRST_KILL * 2 / 10))
        .collect();
    orbs.sort_unstable();
    orbs
}

/// Java's `Random`, which the game's `SingleThreadedRandomSource` is: the gateways'
/// order is shuffled with it.
struct JavaRandom(i64);

impl JavaRandom {
    const MULTIPLIER: i64 = 0x5_DEEC_E66D;
    const MASK: i64 = (1 << 48) - 1;

    fn new(seed: i64) -> JavaRandom {
        JavaRandom((seed ^ Self::MULTIPLIER) & Self::MASK)
    }

    fn next(&mut self, bits: u32) -> i32 {
        self.0 = self.0.wrapping_mul(Self::MULTIPLIER).wrapping_add(0xB) & Self::MASK;
        (self.0 >> (48 - bits)) as i32
    }

    /// `BitRandomSource.nextInt(bound)`.
    fn next_int(&mut self, bound: i32) -> i32 {
        if bound & -bound == bound {
            return ((i64::from(bound) * i64::from(self.next(31))) >> 31) as i32;
        }
        loop {
            let bits = self.next(31);
            let value = bits % bound;
            if bits.wrapping_sub(value).wrapping_add(bound - 1) >= 0 {
                return value;
            }
        }
    }
}

/// Where the gateway of index `index` goes (`EnderDragonFight.spawnNewGateway`:
/// `floor(96 cos(2(−π + 0.15707963267948966 i)))`, the sine for z, at y 75).
fn gateway_place(index: i32) -> (i32, i32, i32) {
    let angle = 2.0 * (-PI + 0.157_079_632_679_489_66 * f64::from(index));
    (
        (96.0 * angle.cos()).floor() as i32,
        75,
        (96.0 * angle.sin()).floor() as i32,
    )
}

/// Where the first dragon's death puts its gateway in a world of `seed`: the fight
/// shuffles the indices 0 to 19 with the world's seed (`EnderDragonFight.init`:
/// `Util.shuffle(gateways, RandomSource.createThreadLocalInstance(seed))`, Fisher and
/// Yates from the end) and takes the last (`spawnNewGateway`: `remove(size − 1)`).
fn first_gateway(seed: i64) -> (i32, i32, i32) {
    let mut order: Vec<i32> = (0..20).collect();
    let mut random = JavaRandom::new(seed);
    for size in (2..=order.len()).rev() {
        let bound = i32::try_from(size).expect("twenty");
        let other = usize::try_from(random.next_int(bound)).expect("a draw below its bound");
        order.swap(size - 1, other);
    }
    gateway_place(*order.last().expect("twenty gateways"))
}

/// The active exit portal as `EndPodiumFeature.place` and `setDragonKilled` leave it, at
/// `(dx, dy, dz)` from the podium's origin, where the feature writes a block: within 3.5
/// of the origin (the squared distance between the blocks' corners below 12.25), end
/// stone round bedrock below the origin's layer, bedrock round the portal on it, air
/// above; the pillar of bedrock four high from the origin, a wall torch on each of its
/// sides two up, and the egg on top of it (`MOTION_BLOCKING` at the podium's column).
fn podium_block(dx: i32, dy: i32, dz: i32) -> Option<&'static str> {
    if (dx, dz) == (0, 0) && (0..4).contains(&dy) {
        return Some("minecraft:bedrock");
    }
    if (dx, dy, dz) == (0, 4, 0) {
        return Some("minecraft:dragon_egg");
    }
    if dy == 2 && dx.abs() + dz.abs() == 1 {
        return Some("minecraft:wall_torch");
    }
    let distance = dx * dx + dy * dy + dz * dz;
    if f64::from(distance) >= 3.5 * 3.5 {
        return None;
    }
    let inner = f64::from(distance) < 2.5 * 2.5;
    Some(match dy {
        ..=-1 if inner => "minecraft:bedrock",
        ..=-1 => "minecraft:end_stone",
        0 if inner => "minecraft:end_portal",
        0 => "minecraft:bedrock",
        _ => "minecraft:air",
    })
}

// ---------------------------------------------------------------------------------
// What the bot is told of the dragon, read as it comes.
// ---------------------------------------------------------------------------------

/// The dragon as the bot's packets tell it, read from where the fight began.
#[derive(Debug)]
struct Watch {
    dragon: i32,
    /// How far into the bot's packets it has read.
    cursor: usize,
    position: (f64, f64, f64),
    phase: Option<i32>,
    /// The game time of the clock's last packet read.
    clock: Option<i64>,
    /// How many times the dragon has sat down, and the clock's time read before the
    /// last sitting down was told, while it sits.
    sittings: u32,
    sat: Option<(u32, Option<i64>)>,
    /// The first sign of its death: its health told as nothing, its death's event, or
    /// its death's level event; with its phase and place then.
    dead_at: Option<usize>,
    phase_at_death: Option<i32>,
    position_at_death: Option<(f64, f64, f64)>,
    /// Its removal, with where it was last shown.
    removed_at: Option<usize>,
    last_position: Option<(f64, f64, f64)>,
}

impl Watch {
    fn new(bot: &Bot, dragon: i32) -> Result<Watch> {
        let seen = bot
            .entities
            .get(&dragon)
            .context("the dragon is not shown")?;
        Ok(Watch {
            dragon,
            cursor: packets(bot).len(),
            position: seen.position,
            phase: None,
            clock: bot.times.last().map(|time| time.game_time),
            sittings: 0,
            sat: None,
            dead_at: None,
            phase_at_death: None,
            position_at_death: None,
            removed_at: None,
            last_position: None,
        })
    }

    fn died(&mut self, at: usize) {
        if self.dead_at.is_none() {
            self.dead_at = Some(at);
            self.phase_at_death = self.phase;
            self.position_at_death = Some(self.position);
        }
    }

    /// Reads what the bot has been sent since it last read.
    fn read(&mut self, bot: &Bot) {
        let packets = packets(bot);
        for (at, (_, packet)) in packets.iter().enumerate().skip(self.cursor) {
            match packet {
                ClientboundPlay::SetTime(time) => self.clock = Some(time.game_time),
                ClientboundPlay::SpawnEntity(spawn) if spawn.entity_id == self.dragon => {
                    self.position = (spawn.x, spawn.y, spawn.z);
                }
                ClientboundPlay::SyncEntityPosition(sync) if sync.entity_id == self.dragon => {
                    if let Some(end) = sync.path.end() {
                        self.position = end;
                    }
                }
                ClientboundPlay::MoveEntityPos(step) if step.entity_id == self.dragon => {
                    self.position = moved_by(self.position, &step.delta);
                }
                ClientboundPlay::MoveEntityPosRot(step) if step.entity_id == self.dragon => {
                    self.position = moved_by(self.position, &step.delta);
                }
                ClientboundPlay::SetEntityData(data) if data.entity_id == self.dragon => {
                    for (index, value) in &data.values {
                        match (*index, value) {
                            (PHASE_INDEX, EntityDataValue::Int(phase)) => {
                                let sits = SITTING.contains(phase);
                                let sat = self.phase.is_some_and(|was| SITTING.contains(&was));
                                if sits && !sat {
                                    self.sittings += 1;
                                    self.sat = Some((self.sittings, self.clock));
                                } else if !sits {
                                    self.sat = None;
                                }
                                self.phase = Some(*phase);
                            }
                            (HEALTH_INDEX, EntityDataValue::Float(health)) if *health <= 0.0 => {
                                self.died(at);
                            }
                            _ => {}
                        }
                    }
                }
                ClientboundPlay::EntityEvent(event)
                    if event.entity_id == self.dragon && event.event == DEATH_EVENT =>
                {
                    self.died(at);
                }
                ClientboundPlay::LevelEvent(event) if event.event == DRAGON_DIED => self.died(at),
                ClientboundPlay::RemoveEntities(removed)
                    if removed.entity_ids.contains(&self.dragon) && self.removed_at.is_none() =>
                {
                    self.died(at);
                    self.removed_at = Some(at);
                    self.last_position = Some(self.position);
                }
                _ => {}
            }
        }
        self.cursor = packets.len();
    }

    fn is_dead(&self) -> bool {
        self.dead_at.is_some()
    }
}

/// Where a relative move takes an entity from `from`, in 4096ths of a block
/// (`VecDeltaCodec`); the steps of a stepped move add up.
fn moved_by(
    from: (f64, f64, f64),
    delta: &clustine_protocol::packets::play::VecDelta,
) -> (f64, f64, f64) {
    use clustine_protocol::packets::play::VecDelta;
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
// The scene.
// ---------------------------------------------------------------------------------

/// How long the bot waits for the dragon to sit on the podium, and for it to rise
/// again. With no crystal left it turns to land with a chance of one in three at the end
/// of each path of a few seconds; five minutes is far past any run of bad draws.
const PERCH_PATIENCE: Duration = Duration::from_secs(300);

/// How long the whole fight may take, from the first wait for a perch to the death.
const FIGHT_PATIENCE: Duration = Duration::from_secs(1200);

/// How long the dying may take to be seen through: 200 ticks are ten seconds.
const DYING_PATIENCE: Duration = Duration::from_secs(120);

/// How many ticks after the clock's packet read before the dragon sat down the bot
/// still hits it: the sitting lasts 100 ticks from a moment no earlier, and the hit
/// sent at the clock's packet of 80 lands within a tick or two.
const LAST_HIT: i64 = 80;

/// How fast the bot flies across the island, in blocks a tick (the official server
/// lets a player move up to 10 a tick, `ServerGamePacketListenerImpl.handleMovePlayer`).
const CRUISE_SPEED: f64 = 2.0;

/// Everything the scene saw on one server.
#[derive(Debug)]
struct Whole {
    kill: KillTook,
    death: Death,
    leavings: Leavings,
    credits: Credits,
}

/// How the kill went, and how long it took: printed, not judged.
#[derive(Debug)]
#[allow(dead_code)] // Read in what the tests print.
struct KillTook {
    /// From joining until the dragon was dead.
    whole: Duration,
    /// Breaking the ten crystals.
    crystals: Duration,
    /// From the first wait for a perch to the death.
    fight: Duration,
    perches: u32,
    hits: u32,
}

/// What a player beside the dragon is shown of its death, from the killing hit to its
/// removal (the orbs to a little after it).
#[derive(Debug)]
struct Death {
    /// Whether the killing hit fell on a sitting dragon, as the scene means it to.
    killed_sitting: bool,
    /// The kinds of the packets about the dragon, but its moves, each run of one as one;
    /// and its removal.
    dragon_kinds: Vec<String>,
    /// The dragon's data told, each value with its index.
    dragon_data: Vec<String>,
    entity_events: Vec<i8>,
    /// Every level event: its number, data and whether it is heard everywhere.
    level_events: Vec<(i32, i32, bool)>,
    /// How far the death's level event was from the dragon's block, on each axis.
    death_event_off: Option<(i32, i32, i32)>,
    /// The bar's operations from the killing hit to a little after the dragon's
    /// removal, those after it marked so.
    bar: Vec<String>,
    /// The worths of the orbs shown, in order of size.
    orbs: Vec<i32>,
    /// The ticks of the dying as the dragon's rise tells them, and as the client's
    /// clock tells them from the death's level event to the removal (printed, not
    /// judged: it is the servers' pace as much as the game's).
    ticks_by_rise: Option<i64>,
    #[allow(dead_code)] // Read in what the tests print.
    ticks_by_clock: Option<i64>,
}

/// What the death leaves.
#[derive(Debug)]
struct Leavings {
    /// The podium's origin's height: its middle layer, where the portal is.
    origin_y: i32,
    /// The blocks within four of the podium's column, from one below the origin's layer
    /// to five above it, by place relative to the origin.
    podium: BTreeMap<(i32, i32, i32), String>,
    /// The places of the twenty where a gateway stands.
    gateways: Vec<(i32, i32, i32)>,
    /// The first of those's cage, each block that is not as `EndGatewayFeature` puts it.
    cage: Vec<String>,
}

/// What a player who steps into the exit portal is sent.
#[derive(Debug)]
struct Credits {
    /// From the step into the portal up to and with the win screen: every kind, and
    /// those that are not of the level being shown ([`NOT_OF_THE_STEP`]), each run of
    /// one as one. The first is printed, not judged: orbs come to the bot and the
    /// things about it move in their own time.
    #[allow(dead_code)] // Read in what the tests print.
    all_to_the_win: Vec<String>,
    to_the_win: Vec<String>,
    /// The win screen's game event and value.
    win: Option<(u8, f32)>,
    /// From the client's command to the placing: the kinds, as `end_oracle.rs` reads a
    /// change of dimension's, the game events, the `Respawn`'s fields.
    respawn_kinds: Vec<String>,
    game_events: Vec<(u8, f32)>,
    respawn: Option<String>,
    respawn_dimension: Option<String>,
    respawn_keep: Option<u8>,
    /// The place relative to the world spawn's block, and the relative flags.
    placed: Option<((f64, f64, f64), i32)>,
    health_after: Option<f32>,
}

/// The dragon the bot is shown, if any.
fn dragon_of(bot: &Bot) -> Option<i32> {
    bot.entities
        .values()
        .find(|entity| entity.kind == entity_types::ENDER_DRAGON)
        .map(|entity| entity.entity_id)
}

/// The podium's origin's height as the bot is shown it: three below the top of the
/// pillar of bedrock at the island's middle, the rim of bedrock round the origin's
/// layer standing three out.
fn podium_origin(bot: &Bot) -> Option<i32> {
    let top = (0..200)
        .rev()
        .find(|y| !is_air(bot.block_at(0, *y, 0).ok().flatten()))?;
    let origin = top - 3;
    let bedrock = |x, y, z| is(bot.block_at(x, y, z).ok().flatten(), blocks::BEDROCK);
    ((origin..=top).all(|y| bedrock(0, y, 0)) && bedrock(3, origin, 0)).then_some(origin)
}

/// Flies the bot up to the cruising height where it is, across to over `to`, and down
/// to it, its column clear.
async fn cruise_to(bot: &mut Bot, to: (f64, f64, f64)) -> Result<()> {
    let cruise = f64::from(END_CRUISE);
    let look = bot.look;
    let (x, _, z) = bot.location;
    bot.fly_to((x, cruise, z), look, CRUISE_SPEED).await?;
    bot.fly_to((to.0, cruise, to.2), look, CRUISE_SPEED).await?;
    let (column_x, column_z) = (to.0.floor() as i32, to.2.floor() as i32);
    let low = to.1.floor() as i32;
    ensure!(
        column_is_clear(bot, column_x, column_z, low, END_CRUISE + 1),
        "the way down to {to:?} is not clear: {}",
        column_report(bot, column_x, column_z, low, END_CRUISE + 1)
    );
    bot.fly_to(to, look, CRUISE_SPEED).await
}

/// Breaks every end crystal the bot is shown, one after another round the island, each
/// with a hit from four blocks above it, and waits for each to go.
async fn break_the_crystals(bot: &mut Bot) -> Result<()> {
    let crystal_count = |bot: &Bot| {
        (bot.entities.values())
            .filter(|entity| entity.kind == entity_types::END_CRYSTAL)
            .count()
    };
    wait(bot, &mut [], PATIENCE, "the ten crystals", |bot| {
        crystal_count(bot) >= 10
    })
    .await?;
    let mut crystals: Vec<(f64, i32, (f64, f64, f64))> = bot
        .entities
        .values()
        .filter(|entity| entity.kind == entity_types::END_CRYSTAL)
        .map(|entity| {
            let (x, _, z) = entity.position;
            (z.atan2(x), entity.entity_id, entity.position)
        })
        .collect();
    ensure!(
        crystals.len() == 10,
        "{} crystals shown, where the ten pillars have one each",
        crystals.len()
    );
    crystals.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (_, crystal, (x, y, z)) in crystals {
        // Four above its feet: above the iron bars of a caged pillar, which reach a
        // block above the crystal's top, and well within reach of it.
        cruise_to(bot, (x, y + 4.0, z)).await?;
        bot.attack(crystal).await?;
        let what = format!("the crystal {crystal} at ({x}, {y}, {z}) broken");
        wait(bot, &mut [], PATIENCE, &what, |bot| {
            !bot.entities.contains_key(&crystal)
        })
        .await?;
    }
    let (x, _, z) = bot.location;
    let look = bot.look;
    bot.fly_to((x, f64::from(END_CRUISE), z), look, CRUISE_SPEED)
        .await
}

/// Where the bot hits the sitting dragon's head from: three above the head's place
/// (or higher, where something stands there), whose eyes are within reach of the
/// head's box even where the place is off by a few blocks.
fn above_the_head(bot: &Bot, watch: &Watch) -> Result<(f64, f64, f64)> {
    let yaw = bot
        .entities
        .get(&watch.dragon)
        .context("the dragon is not shown")?
        .yaw;
    let (x, y, z) = watch.position;
    let yaw = f64::from(yaw).to_radians();
    let (head_x, head_z) = (x + yaw.sin() * HEAD_OUT, z - yaw.cos() * HEAD_OUT);
    let (column_x, column_z) = (head_x.floor() as i32, head_z.floor() as i32);
    for up in 3..=6 {
        let feet = y + f64::from(up);
        let low = feet.floor() as i32;
        if column_is_clear(bot, column_x, column_z, low, low + 2) {
            return Ok((head_x, feet, head_z));
        }
    }
    bail!(
        "no room above the dragon's head: {}",
        column_report(bot, column_x, column_z, y as i32, y as i32 + 9)
    )
}

/// Fights the dragon from above the podium until it is dead (the module's comment says
/// how). Returns the place in the bot's packets of the hit that killed it, with the
/// fight's numbers.
async fn fight(bot: &mut Bot, watch: &mut Watch) -> Result<(usize, u32, u32)> {
    let deadline = Instant::now() + FIGHT_PATIENCE;
    let mut perches = 0;
    let mut hits = 0;
    let mut marks = Vec::new();
    while !watch.is_dead() {
        ensure!(
            Instant::now() < deadline,
            "the dragon is not dead after {FIGHT_PATIENCE:?}: {perches} perches, {hits} hits"
        );
        wait(
            bot,
            &mut [],
            PERCH_PATIENCE,
            "the dragon on the podium",
            |bot| {
                watch.read(bot);
                watch.is_dead() || watch.sat.is_some()
            },
        )
        .await?;
        if watch.is_dead() {
            break;
        }
        let Some((sitting, Some(since))) = watch.sat else {
            // It sat down before the clock was read: not to be hit by this rule.
            wait(bot, &mut [], PERCH_PATIENCE, "the dragon up again", |bot| {
                watch.read(bot);
                watch.is_dead() || watch.sat.is_none()
            })
            .await?;
            continue;
        };
        perches += 1;
        let spot = above_the_head(bot, watch)?;
        let look = bot.look;
        bot.fly_to(spot, look, 1.0).await?;
        loop {
            watch.read(bot);
            let still = watch.sat.is_some_and(|(sat, _)| sat == sitting);
            if watch.is_dead() || !still || watch.clock.unwrap_or(since) > since + LAST_HIT {
                break;
            }
            marks.push(packets(bot).len());
            bot.attack(watch.dragon + HEAD).await?;
            hits += 1;
            let told = bot.times.len();
            wait(bot, &mut [], PATIENCE, "the clock's next packet", |bot| {
                bot.times.len() > told
            })
            .await?;
        }
        wait(bot, &mut [], PERCH_PATIENCE, "the dragon up again", |bot| {
            watch.read(bot);
            watch.is_dead() || !watch.sat.is_some_and(|(sat, _)| sat == sitting)
        })
        .await?;
    }
    let dead_at = watch.dead_at.context("dead")?;
    let mark = marks
        .iter()
        .rev()
        .copied()
        .find(|mark| *mark <= dead_at)
        .context("the dragon died before any hit of the bot's")?;
    Ok((mark, perches, hits))
}

/// Joins, goes into the End, kills the dragon and goes through the exit portal into the
/// credits and back; what it saw.
async fn scene(address: &str) -> Result<Whole> {
    let began = Instant::now();
    let (mut bot, _) = to_the_end(address, "Slayer", &mut []).await?;
    let flight = fly_over_the_island(&mut bot, &mut [], (0.5, 0.5)).await;
    reported(&bot, flight)?;
    let shown = wait(&mut bot, &mut [], PATIENCE, "the dragon", |bot| {
        dragon_of(bot).is_some()
    })
    .await;
    reported(&bot, shown)?;
    let shown = wait(&mut bot, &mut [], PATIENCE, "the podium", |bot| {
        podium_origin(bot).is_some()
    })
    .await;
    reported(&bot, shown)?;
    let origin_y = podium_origin(&bot).context("shown just now")?;
    // Where the dragon lands: on top of the pillar (`MOTION_BLOCKING_NO_LEAVES`).
    let top = origin_y + 4;
    take(&mut bot, items::NETHERITE_SWORD).await?;

    let crystals_began = Instant::now();
    let broken = break_the_crystals(&mut bot).await;
    reported(&bot, broken)?;
    let crystals = crystals_began.elapsed();

    let hover = (0.5, f64::from(top + 3), 0.5);
    let there = cruise_to(&mut bot, hover).await;
    reported(&bot, there)?;
    let dragon = dragon_of(&bot).context("the dragon is not shown above the podium")?;
    let mut watch = Watch::new(&bot, dragon)?;
    let fight_began = Instant::now();
    let fought = fight(&mut bot, &mut watch).await;
    let (mark, perches, hits) = reported(&bot, fought)?;
    let kill = KillTook {
        whole: began.elapsed(),
        crystals,
        fight: fight_began.elapsed(),
        perches,
        hits,
    };
    println!("{address}: {kill:?}");

    // Back over the podium's column, where the orbs the dying dragon drops over it come
    // to the bot, and where the bot will step down into the exit portal.
    let look = bot.look;
    let back = bot.fly_to(hover, look, 1.0).await;
    reported(&bot, back)?;
    let removed = wait(
        &mut bot,
        &mut [],
        DYING_PATIENCE,
        "the dragon's removal",
        |bot| {
            watch.read(bot);
            watch.removed_at.is_some()
        },
    )
    .await;
    reported(&bot, removed)?;
    let (portal, egg) = ((2, origin_y, 0), (0, top, 0));
    let left = wait(
        &mut bot,
        &mut [],
        PATIENCE,
        "the exit portal and the egg",
        |bot| {
            is(
                bot.block_at(portal.0, portal.1, portal.2).ok().flatten(),
                blocks::END_PORTAL,
            ) && is(
                bot.block_at(egg.0, egg.1, egg.2).ok().flatten(),
                blocks::DRAGON_EGG,
            )
        },
    )
    .await;
    reported(&bot, left)?;
    let gateways = |bot: &Bot| -> Vec<(i32, i32, i32)> {
        (0..20)
            .map(gateway_place)
            .filter(|&(x, y, z)| is(bot.block_at(x, y, z).ok().flatten(), blocks::END_GATEWAY))
            .collect()
    };
    let gateway = wait(&mut bot, &mut [], PATIENCE, "the first gateway", |bot| {
        !gateways(bot).is_empty()
    })
    .await;
    reported(&bot, gateway)?;
    // The orbs come with the dragon's last ticks: one more of the clock's packets, and
    // every one has been shown.
    let told = bot.times.len();
    let clock = wait(
        &mut bot,
        &mut [],
        PATIENCE,
        "the clock's next packet",
        |bot| bot.times.len() > told,
    )
    .await;
    reported(&bot, clock)?;

    let death = death_of(packets(&bot), &watch, mark)?;
    let leavings = leavings_of(&bot, origin_y, gateways(&bot));
    let credits = credits(bot, origin_y, top).await?;
    Ok(Whole {
        kill,
        death,
        leavings,
        credits,
    })
}

/// The death as the packets from the killing hit `mark` tell it.
fn death_of(packets: &[(Instant, ClientboundPlay)], watch: &Watch, mark: usize) -> Result<Death> {
    let dragon = watch.dragon;
    let removed = watch.removed_at.context("removed")?;
    let window = &packets[mark..=removed];
    let mut dragon_kinds = Vec::new();
    let mut dragon_data = Vec::new();
    let mut entity_events = Vec::new();
    let mut level_events = Vec::new();
    let mut death_event = None;
    let mut bar = Vec::new();
    for (when, packet) in window {
        let about = entity_of(packet) == Some(dragon);
        if about && !is_motion(packet) {
            dragon_kinds.push(kind(packet));
        }
        match packet {
            ClientboundPlay::RemoveEntities(gone) if gone.entity_ids.contains(&dragon) => {
                dragon_kinds.push(kind(packet));
            }
            ClientboundPlay::SetEntityData(data) if about => {
                dragon_data.extend(
                    data.values
                        .iter()
                        .map(|(index, value)| format!("{index}: {value:?}")),
                );
            }
            ClientboundPlay::EntityEvent(event) if about => entity_events.push(event.event),
            ClientboundPlay::LevelEvent(event) => {
                level_events.push((event.event, event.data, event.global));
                if event.event == DRAGON_DIED && death_event.is_none() {
                    death_event = Some((*when, event.position));
                }
            }
            _ => {}
        }
    }
    // The bar, from the killing hit to now. The game takes it away before the
    // dragon's removal, in the same tick (`setDragonKilled` before `remove`); Clustine
    // takes it away in that tick too, but a worker sends what concerns one player
    // after its tick's events, so after the removal: named, and not compared, as a
    // player sees both go at once.
    for (_, packet) in packets.iter().skip(mark) {
        if let ClientboundPlay::BossEvent(event) = packet {
            bar.push(bar_operation(&event.operation));
        }
    }
    // The orbs, from the killing hit to now.
    let mut orb_ids = BTreeMap::new();
    for (_, packet) in &packets[mark..] {
        match packet {
            ClientboundPlay::SpawnEntity(spawn) if spawn.kind == entity_types::EXPERIENCE_ORB => {
                orb_ids.entry(spawn.entity_id).or_insert(None);
            }
            ClientboundPlay::SetEntityData(data) => {
                if let Some(worth) = orb_ids.get_mut(&data.entity_id) {
                    for (index, value) in &data.values {
                        if let (ORB_VALUE_INDEX, EntityDataValue::Int(value)) = (*index, value) {
                            worth.get_or_insert(*value);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let mut orbs: Vec<i32> = orb_ids.values().map(|worth| worth.unwrap_or(0)).collect();
    orbs.sort_unstable();
    let at_death = watch.position_at_death;
    let death_event_off = death_event.zip(at_death).map(|((_, at), (x, y, z))| {
        (
            at.x - x.floor() as i32,
            at.y - y.floor() as i32,
            at.z - z.floor() as i32,
        )
    });
    let ticks_by_rise = at_death
        .zip(watch.last_position)
        .map(|((_, from, _), (_, to, _))| ((to - from) / RISE_PER_TICK).round() as i64);
    let ticks_by_clock = death_event.map(|(when, _)| ticks(when, packets[removed].0));
    Ok(Death {
        killed_sitting: watch
            .phase_at_death
            .is_some_and(|phase| SITTING.contains(&phase)),
        dragon_kinds: deduplicated(dragon_kinds),
        dragon_data,
        entity_events,
        level_events,
        death_event_off,
        bar,
        orbs,
        ticks_by_rise,
        ticks_by_clock,
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

/// What the death left, as the bot is shown it.
fn leavings_of(bot: &Bot, origin_y: i32, gateways: Vec<(i32, i32, i32)>) -> Leavings {
    let mut podium = BTreeMap::new();
    for dy in -1..=5 {
        for dx in -4..=4 {
            for dz in -4..=4 {
                let state = bot.block_at(dx, origin_y + dy, dz).ok().flatten();
                podium.insert(
                    (dx, dy, dz),
                    state.map_or_else(|| "not held".to_owned(), block_name),
                );
            }
        }
    }
    let mut cage = Vec::new();
    if let Some(&(x, y, z)) = gateways.first() {
        for dx in -1..=1 {
            for dy in -2..=2 {
                for dz in -1..=1 {
                    let state = bot.block_at(x + dx, y + dy, z + dz).ok().flatten();
                    let wanted = gateway_cage(dx, dy, dz);
                    let right = if wanted == blocks::AIR {
                        is_air(state)
                    } else {
                        is(state, wanted)
                    };
                    if !right {
                        cage.push(format!(
                            "({dx}, {dy}, {dz}): {}",
                            state.map_or_else(|| "not held".to_owned(), block_name)
                        ));
                    }
                }
            }
        }
    }
    Leavings {
        origin_y,
        podium,
        gateways,
        cage,
    }
}

/// The kinds that come of showing the level, of the things in it moving, and of the
/// orbs the bot picks up, rather than of the step into the exit portal.
const NOT_OF_THE_STEP: &[&str] = &[
    "ClientboundKeepAlive",
    "SetTime",
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
    "TakeItemEntity",
    "SetExperience",
    "Sound",
    "SoundEntity",
    "LevelParticles",
    "minecraft:bundle_delimiter",
];

/// The kinds that come of showing a level rather than of changing to it
/// (`end_oracle.rs`'s list).
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
/// time (`end_oracle.rs`'s list, named in `dimensions_oracle.rs`).
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

/// A `Respawn`'s fields, the seed's hash among them (both worlds are of seed 0), as
/// `end_oracle.rs` shows them.
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

/// What the client was sent once it asked for its respawn.
#[derive(Default)]
struct AfterTheAsk {
    /// The kinds up to and with the placing.
    kinds: Vec<String>,
    game_events: Vec<(u8, f32)>,
    respawn: Option<Respawn>,
    place: Option<SynchronizePlayerPosition>,
    health: Option<f32>,
}

/// Reads what the server sends `connection` until a `Respawn` has come and a place
/// after it, and then a health, or two of the clock's packets without one (the game
/// tells the health in the first tick of the new player, `ServerPlayer.doTick`).
async fn read_the_respawn(connection: &mut Connection) -> Result<AfterTheAsk> {
    let deadline = Instant::now() + PATIENCE;
    let mut seen = AfterTheAsk::default();
    let mut clocks_since_placed = 0;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let frame = timeout(left, connection.read_frame())
            .await
            .context("the respawn did not come in time")??;
        let packet = ClientboundPlay::decode(&frame)?;
        if seen.place.is_none() {
            seen.kinds.push(kind(&packet));
        }
        match packet {
            ClientboundPlay::Respawn(respawn) => {
                ensure!(seen.respawn.is_none(), "a second Respawn: {respawn:?}");
                seen.respawn = Some(respawn);
            }
            ClientboundPlay::SynchronizePlayerPosition(place) if seen.respawn.is_some() => {
                seen.place.get_or_insert(place);
            }
            ClientboundPlay::SetHealth(health) if seen.respawn.is_some() => {
                seen.health.get_or_insert(health.health);
            }
            ClientboundPlay::GameEvent(event) if seen.place.is_none() => {
                seen.game_events.push((event.event, event.value));
            }
            ClientboundPlay::SetTime(_) if seen.place.is_some() => clocks_since_placed += 1,
            _ => {}
        }
        if seen.place.is_some() && (seen.health.is_some() || clocks_since_placed >= 2) {
            return Ok(seen);
        }
    }
}

/// From over the podium, steps down into the exit portal, waits for the win screen,
/// and asks for the respawn as the client does when the credits end.
async fn credits(mut bot: Bot, origin_y: i32, top: i32) -> Result<Credits> {
    // A block of the portal two east of the pillar: the inside of the rim is the portal
    // within 2.5 of the origin, and from three above the pillar's top down to it there
    // is nothing (the torches stand one out from the pillar).
    let (x, z) = (2, 0);
    let above = f64::from(top + 3);
    ensure!(
        column_is_clear(&bot, x, z, origin_y + 1, top + 4),
        "the way down into the exit portal is not clear: {}",
        column_report(&bot, x, z, origin_y + 1, top + 4)
    );
    let look = bot.look;
    let over = bot.fly_to((2.5, above, 0.5), look, 1.0).await;
    reported(&bot, over)?;
    let down = bot
        .fly_to((2.5, f64::from(origin_y) + 1.05, 0.5), look, 0.5)
        .await;
    reported(&bot, down)?;
    // In with one step, as `to_the_end` drops into the portal it opens.
    let mark = packets(&bot).len();
    bot.stand_at(2.5, f64::from(origin_y), 0.5).await?;
    let won = |bot: &Bot| {
        packets(bot)[mark..].iter().position(|(_, packet)| {
            matches!(packet, ClientboundPlay::GameEvent(event)
                if event.event == game_event::WIN_GAME)
        })
    };
    let shown = wait(&mut bot, &mut [], PATIENCE, "the win screen", |bot| {
        won(bot).is_some()
    })
    .await;
    reported(&bot, shown)?;
    let win_at = mark + won(&bot).context("shown just now")?;
    let kept = packets(&bot);
    // A block's change is named by its place and block, which say what made it.
    let all_to_the_win: Vec<String> = kept[mark..=win_at]
        .iter()
        .map(|(_, packet)| match packet {
            ClientboundPlay::BlockUpdate(update) => format!(
                "BlockUpdate ({}, {}, {}) {}",
                update.position.x,
                update.position.y,
                update.position.z,
                block_name(update.state)
            ),
            _ => kind(packet),
        })
        .collect();
    // The official server tells the bot air at (2, 63, 0), over the portal block it
    // steps into, which the bot already has as air (the way down is checked clear
    // above): a change of nothing, whose cause was not found (2026-11-02). Not judged.
    let to_the_win = deduplicated(
        all_to_the_win
            .iter()
            .filter(|kind| !NOT_OF_THE_STEP.contains(&kind.as_str()))
            .filter(|kind| !(kind.starts_with("BlockUpdate") && kind.ends_with(" minecraft:air")))
            .cloned(),
    );
    let win = match &kept[win_at].1 {
        ClientboundPlay::GameEvent(event) => Some((event.event, event.value)),
        _ => None,
    };
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
    drop(connection);
    let respawn_kinds = seen
        .kinds
        .iter()
        .filter(|kind| !NOT_OF_THE_CHANGE.contains(&kind.as_str()))
        .cloned()
        .collect();
    Ok(Credits {
        all_to_the_win,
        to_the_win,
        win,
        respawn_kinds,
        game_events: seen.game_events,
        respawn: seen
            .respawn
            .as_ref()
            .map(|respawn| respawn_fields(&registries, respawn)),
        respawn_dimension: seen
            .respawn
            .as_ref()
            .map(|respawn| respawn.spawn.dimension_name.clone()),
        respawn_keep: seen.respawn.as_ref().map(|respawn| respawn.data_to_keep),
        placed: seen.place.as_ref().map(|place| {
            (
                (
                    place.x - f64::from(spawn.x),
                    place.y - f64::from(spawn.y),
                    place.z - f64::from(spawn.z),
                ),
                place.relative_flags,
            )
        }),
        health_after: seen.health,
    })
}

// ---------------------------------------------------------------------------------
// The judgements by the game's rules, of one server's run.
// ---------------------------------------------------------------------------------

/// The killing hit fell on a sitting dragon, as the scene means it to: else its death
/// is a flying dragon's (`DYING`, a flight to the podium first) and the bounds below do
/// not hold.
fn kill_problem(death: &Death) -> Option<String> {
    (!death.killed_sitting).then(|| "the killing hit fell on a dragon that was not sitting".into())
}

/// The death's packets about the dragon: its death's event once (`LivingEntity.die`),
/// its health told as nothing, and nothing of the `DYING` phase, which only a flying
/// dragon is turned to (`handleKillingBlow`).
fn dragon_problem(death: &Death) -> Option<String> {
    if death.entity_events != [DEATH_EVENT] {
        return Some(format!(
            "entity events {:?}, where the death's {DEATH_EVENT} once is all",
            death.entity_events
        ));
    }
    let health = format!("{HEALTH_INDEX}: Float(0.0)");
    if !death.dragon_data.contains(&health) {
        return Some(format!(
            "no health of nothing among its data {:?}",
            death.dragon_data
        ));
    }
    let dying = format!("{PHASE_INDEX}: Int(9)");
    death
        .dragon_data
        .contains(&dying)
        .then(|| format!("the DYING phase among its data {:?}", death.dragon_data))
}

/// The global level event of the death: once, heard everywhere, with no data, at the
/// dragon's block (`blockPosition()` at its first tick of dying, a tenth of a block
/// above where it sat, so one block up at the most).
fn global_event_problem(death: &Death) -> Option<String> {
    let deaths: Vec<&(i32, i32, bool)> = death
        .level_events
        .iter()
        .filter(|(event, _, _)| *event == DRAGON_DIED)
        .collect();
    if deaths != [&(DRAGON_DIED, 0, true)] {
        return Some(format!(
            "the death's level events {deaths:?}, where one of data 0 heard everywhere is all"
        ));
    }
    match death.death_event_off {
        Some((x, y, z)) if x.abs() <= 1 && (0..=1).contains(&y) && z.abs() <= 1 => None,
        off => Some(format!(
            "the death's level event {off:?} from the dragon's block when it died"
        )),
    }
}

/// The bar: its progress told as nothing at the killing hit (`updateDragon` from
/// `tickDeath`), and taken away at the end, before the dragon's removal of the same
/// tick (`tickDeath` at 200: `setDragonKilled`, whose `setVisible(false)` sends the
/// removal at once, then `remove(KILLED)`); no other change while the dragon dies.
fn bar_problem(death: &Death) -> Option<String> {
    (death.bar != ["progress 0", "remove"]).then(|| {
        format!(
            "{:?}, where progress 0 and then its removal are all",
            death.bar
        )
    })
}

/// The orbs: each of a worth the first kill's awards make (`first_kill_orbs`), none
/// twice more than they make, and at most [`MERGES_AT_MOST`] merged away before they
/// were shown.
fn orbs_problem(death: &Death) -> Option<String> {
    let mut left = first_kill_orbs();
    for orb in &death.orbs {
        match left.iter().position(|worth| worth == orb) {
            Some(at) => {
                left.remove(at);
            }
            None => {
                return Some(format!(
                    "an orb of {orb} beyond what a first kill makes, among {:?}",
                    death.orbs
                ));
            }
        }
    }
    (left.len() > MERGES_AT_MOST).then(|| {
        format!(
            "{} orbs shown, where a first kill makes {} and few merge before they are shown; \
             missing {left:?}",
            death.orbs.len(),
            first_kill_orbs().len()
        )
    })
}

/// The dying's length by the dragon's rise: 200 ticks of a tenth of a block each, less
/// the last two at the most, which the dragon's moves (one in three ticks,
/// `ServerEntity`) may not show before its removal.
fn dying_problem(death: &Death) -> Option<String> {
    match death.ticks_by_rise {
        Some(ticks) if (DYING_TICKS - 3..=DYING_TICKS).contains(&ticks) => None,
        ticks => Some(format!(
            "the dragon rose {ticks:?} ticks' worth from its death to its removal, where it \
             dies for {DYING_TICKS}"
        )),
    }
}

/// The podium against `podium_block`, each place the feature writes.
fn podium_problem(leavings: &Leavings) -> Option<String> {
    let wrong: Vec<String> = leavings
        .podium
        .iter()
        .filter_map(|(&(dx, dy, dz), shown)| {
            let wanted = podium_block(dx, dy, dz)?;
            (shown != wanted).then(|| format!("({dx}, {dy}, {dz}): {shown}, not {wanted}"))
        })
        .collect();
    (!wrong.is_empty()).then(|| format!("{wrong:?}"))
}

/// One gateway, at seed 0's first place, in its cage.
fn gateway_problem(leavings: &Leavings) -> Option<String> {
    let first = first_gateway(0);
    if leavings.gateways != [first] {
        return Some(format!(
            "gateways at {:?}, where seed 0's first is at {first:?}",
            leavings.gateways
        ));
    }
    (!leavings.cage.is_empty()).then(|| format!("its cage: {:?}", leavings.cage))
}

/// The credits: the win screen with the value 0 (`ServerPlayer.showEndCredits`, 19.5
/// item 11), and no `Respawn` before it.
fn win_problem(credits: &Credits) -> Option<String> {
    if credits.win != Some((game_event::WIN_GAME, 0.0)) {
        return Some(format!("the win screen {:?}", credits.win));
    }
    credits
        .to_the_win
        .iter()
        .any(|kind| kind == "Respawn")
        .then(|| format!("a Respawn before the win screen: {:?}", credits.to_the_win))
}

/// The respawn after the credits: into the overworld, keeping the attribute modifiers
/// alone (`PlayerList.respawn(player, true, …)`: `KEEP_ATTRIBUTE_MODIFIERS`), placed
/// absolutely within the spawn's radius (`respawn_radius`, 10) at `height` above the
/// world spawn's block.
fn respawn_problem(credits: &Credits, height: f64) -> Option<String> {
    if credits.respawn_dimension.as_deref() != Some(OVERWORLD) {
        return Some(format!("the Respawn into {:?}", credits.respawn_dimension));
    }
    if credits.respawn_keep != Some(Respawn::KEEP_ATTRIBUTE_MODIFIERS) {
        return Some(format!("the Respawn keeps {:?}", credits.respawn_keep));
    }
    let on_the_radius = credits.placed.is_some_and(|((x, y, z), flags)| {
        flags == 0 && x.abs() <= 10.5 && z.abs() <= 10.5 && (y - height).abs() < 1e-9
    });
    (!on_the_radius).then(|| format!("placed at {:?} from the world spawn", credits.placed))
}

/// Every rule of the game the scene's run is judged by, on the server `server`.
fn rules(differences: &mut Differences, server: &str, run: &Whole, height: f64) {
    let death = &run.death;
    differences.wrong(&format!("{server}'s kill"), kill_problem(death));
    differences.wrong(
        &format!("{server}'s dragon's packets in its death"),
        dragon_problem(death),
    );
    differences.wrong(
        &format!("{server}'s level event of the death"),
        global_event_problem(death),
    );
    differences.wrong(&format!("{server}'s bar"), bar_problem(death));
    differences.wrong(&format!("{server}'s orbs"), orbs_problem(death));
    differences.wrong(&format!("{server}'s dying"), dying_problem(death));
    differences.wrong(
        &format!("{server}'s exit portal and egg"),
        podium_problem(&run.leavings),
    );
    differences.wrong(
        &format!("{server}'s gateway"),
        gateway_problem(&run.leavings),
    );
    differences.wrong(&format!("{server}'s win screen"), win_problem(&run.credits));
    differences.wrong(
        &format!("{server}'s respawn after the credits"),
        respawn_problem(&run.credits, height),
    );
}

// ---------------------------------------------------------------------------------
// The comparisons.
// ---------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn the_dragon_s_death_is_sent_to_a_player_beside_it_as_on_the_official_server() {
    let (theirs, ours) = both(runs().await);
    println!(
        "official: {:#?}\n{:#?}\nClustine: {:#?}\n{:#?}",
        theirs.kill, theirs.death, ours.kill, ours.death
    );
    let mut differences = Differences::default();
    let (a, b) = (&theirs.death, &ours.death);
    differences.differ(
        "the kinds of the packets about the dragon in its death",
        a.dragon_kinds == b.dragon_kinds,
    );
    // The pose `DYING` (index 6, 7) a dying mob has is not shown by Clustine, a named
    // difference (`clustine-sim`'s `shown.rs`): the death's entity event shows its fall.
    let shown = |data: &[String]| -> Vec<String> {
        data.iter()
            .filter(|value| value.as_str() != "6: Pose(7)")
            .cloned()
            .collect()
    };
    differences.differ(
        "the dragon's data in its death, but its pose",
        shown(&a.dragon_data) == shown(&b.dragon_data),
    );
    differences.differ(
        "the dragon's entity events",
        a.entity_events == b.entity_events,
    );
    differences.differ("the level events", a.level_events == b.level_events);
    differences.differ("the bar's operations", a.bar == b.bar);
    differences.differ(
        "the dying's length by the dragon's rise, within the three ticks its moves may \
         leave unshown",
        a.ticks_by_rise
            .zip(b.ticks_by_rise)
            .is_some_and(|(a, b)| (a - b).abs() <= 3),
    );
    for (server, run) in servers(theirs, ours) {
        let death = &run.death;
        differences.wrong(&format!("{server}'s kill"), kill_problem(death));
        differences.wrong(
            &format!("{server}'s dragon's packets in its death"),
            dragon_problem(death),
        );
        differences.wrong(
            &format!("{server}'s level event of the death"),
            global_event_problem(death),
        );
        differences.wrong(&format!("{server}'s bar"), bar_problem(death));
        differences.wrong(&format!("{server}'s orbs"), orbs_problem(death));
        differences.wrong(&format!("{server}'s dying"), dying_problem(death));
    }
    differences.assert_none();
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn the_exit_portal_the_egg_and_the_first_gateway_are_left_as_on_the_official_server() {
    let (theirs, ours) = both(runs().await);
    println!(
        "official: {:#?}\nClustine: {:#?}",
        theirs.leavings, ours.leavings
    );
    let mut differences = Differences::default();
    let (a, b) = (&theirs.leavings, &ours.leavings);
    differences.differ("the podium's height", a.origin_y == b.origin_y);
    for (place, block) in &a.podium {
        let same = b.podium.get(place) == Some(block);
        if !same {
            println!(
                "at {place:?} from the podium's origin: official {block}, Clustine {:?}",
                b.podium.get(place)
            );
        }
        differences.differ(
            &format!("the block at {place:?} from the podium's origin"),
            same,
        );
    }
    differences.differ("the gateways' places", a.gateways == b.gateways);
    differences.differ("the gateway's cage", a.cage == b.cage);
    for (server, run) in servers(theirs, ours) {
        differences.wrong(
            &format!("{server}'s exit portal and egg"),
            podium_problem(&run.leavings),
        );
        differences.wrong(
            &format!("{server}'s gateway"),
            gateway_problem(&run.leavings),
        );
    }
    differences.assert_none();
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn the_credits_and_the_respawn_after_them_are_sent_as_on_the_official_server() {
    let (theirs, ours) = both(runs().await);
    println!(
        "official: {:#?}\nClustine: {:#?}",
        theirs.credits, ours.credits
    );
    let mut differences = Differences::default();
    let (a, b) = (&theirs.credits, &ours.credits);
    differences.differ(
        "the kinds from the step into the exit portal to the win screen",
        a.to_the_win == b.to_the_win,
    );
    differences.differ("the win screen's game event", a.win == b.win);
    let sent: Vec<String> = a
        .respawn_kinds
        .iter()
        .filter(|kind| !NOT_SENT_BY_CLUSTINE.contains(&kind.as_str()))
        .cloned()
        .collect();
    differences.differ(
        "the kinds from the client's command to the placing",
        sent == b.respawn_kinds,
    );
    differences.differ(
        "the game events of the respawn",
        a.game_events == b.game_events,
    );
    differences.differ("the Respawn's fields", a.respawn == b.respawn);
    differences.differ(
        "the health after the respawn",
        a.health_after == b.health_after,
    );
    // The height is the official server's: on the flat world both stand on the grass.
    let height = a.placed.map_or(f64::NAN, |((_, y, _), _)| y);
    for (server, run) in servers(theirs, ours) {
        differences.wrong(&format!("{server}'s win screen"), win_problem(&run.credits));
        differences.wrong(
            &format!("{server}'s respawn after the credits"),
            respawn_problem(&run.credits, height),
        );
    }
    differences.assert_none();
}

// ---------------------------------------------------------------------------------
// Clustine alone.
// ---------------------------------------------------------------------------------

/// The scene on Clustine alone, judged by the game's rules: for running Clustine's half
/// without Java. Ignored, as it kills the dragon by playing, which takes minutes.
#[tokio::test]
#[ignore = "kills the dragon by playing, which takes minutes; run it by name"]
async fn the_dragon_killed_by_playing_dies_leaves_its_podium_and_sends_the_credits_by_the_rules() {
    let run = on_clustine()
        .await
        .unwrap_or_else(|error| panic!("on Clustine: {error}"));
    println!("Clustine: {run:#?}");
    let mut differences = Differences::default();
    // A respawn at the world spawn stands on its block: the flat world's grass, where
    // the spawn's block is the one a player stands in.
    rules(&mut differences, "Clustine", &run, 0.0);
    differences.assert_none();
}

#[test]
fn a_first_kill_s_awards_make_sixty_six_orbs_worth_twelve_thousand() {
    let orbs = first_kill_orbs();
    assert_eq!(orbs.iter().sum::<i32>(), FIRST_KILL);
    assert_eq!(orbs.len(), 66);
    assert_eq!(orbs_of(960), [617, 307, 17, 17, 1, 1]);
    assert_eq!(orbs_of(2400), [1237, 617, 307, 149, 73, 17]);
}

#[test]
fn java_s_random_draws_as_the_game_s() {
    // `new Random(0).nextInt(…)`'s first draws, as Java gives them.
    let mut random = JavaRandom::new(0);
    assert_eq!(random.next_int(10), 0);
    assert_eq!(random.next_int(10), 8);
    assert_eq!(random.next_int(10), 9);
    // `new Random(0).nextInt()` and `new Random(42).nextInt()`, whole draws of 32 bits.
    assert_eq!(JavaRandom::new(0).next(32), -1_155_484_576);
    assert_eq!(JavaRandom::new(42).next(32), -1_170_105_035);
}

#[test]
fn the_podium_s_rule_counts_the_feature_s_blocks() {
    let count = |dy: i32, name: &str| {
        (-4..=4)
            .flat_map(|dx| (-4..=4).map(move |dz| (dx, dz)))
            .filter(|&(dx, dz)| podium_block(dx, dy, dz) == Some(name))
            .count()
    };
    // As `end_podium.rs`'s own tests count them: 21 of bedrock and 16 of end stone
    // below, a rim of 16 round the portal, and the portal within it but the pillar.
    assert_eq!(count(-1, "minecraft:bedrock"), 21);
    assert_eq!(count(-1, "minecraft:end_stone"), 16);
    assert_eq!(count(0, "minecraft:bedrock"), 16 + 1);
    assert_eq!(count(0, "minecraft:end_portal"), 20);
    assert_eq!(count(2, "minecraft:wall_torch"), 4);
    assert_eq!(count(4, "minecraft:dragon_egg"), 1);
}
