//! Runs the official Minecraft server as a reference to compare against.
//!
//! The bots and the Clustine server share one codec, so a mistake in it can be invisible
//! when they only talk to each other. Running the same scenario against the official
//! server shows such mistakes.
//!
//! The server jar is the one `cargo datagen` downloads. Mojang's server only starts once
//! the Minecraft EULA (<https://aka.ms/MinecraftEULA>) has been agreed to, which nobody
//! can do on someone else's behalf: see [`Oracle::start`].

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use clustine_data::GAME_VERSION;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::sync::{Mutex, MutexGuard};
use tokio::time::timeout;

/// Setting this environment variable to `true` states that the person running the
/// command agrees to the Minecraft EULA.
pub const EULA_VARIABLE: &str = "CLUSTINE_ACCEPT_MINECRAFT_EULA";

/// How long the server may take to start. It usually needs a few seconds.
const START_TIMEOUT: Duration = Duration::from_secs(180);

/// A creative server in offline mode, matching what Clustine serves. `{port}` is
/// replaced by a free port, and the world's lines follow ([`FLAT`] or [`SEEDED`]).
const PROPERTIES: &str = r#"online-mode=false
white-list=false
server-ip=127.0.0.1
server-port={port}
gamemode=creative
spawn-protection=0
enforce-secure-profile=false
motd=oracle
"#;

/// The flat world Clustine serves without a seed.
const FLAT: &str = r#"level-type=minecraft\:flat
generator-settings={"layers"\:[{"block"\:"minecraft\:bedrock","height"\:1},{"block"\:"minecraft\:dirt","height"\:2},{"block"\:"minecraft\:grass_block","height"\:1}],"biome"\:"minecraft\:plains"}
generate-structures=false
"#;

/// The overworld of a seed with its structures, as Clustine serves it with `--seed`.
/// `{seed}` is replaced by the seed.
const SEEDED: &str = r#"level-type=minecraft\:normal
level-seed={seed}
generate-structures=true
"#;

/// The world an oracle serves, and how it is kept before a bot joins.
#[derive(Debug, Clone, PartialEq)]
pub struct OracleWorld {
    /// The seed of the game's overworld, with its structures; none for the flat world
    /// of [`Oracle::start`].
    pub seed: Option<i64>,
    /// The view distance the server grants, in chunks; none for the game's default.
    pub view_distance: Option<u32>,
    /// Whether the game's ticks are frozen (`tick freeze`) before a bot joins, so that
    /// nothing generation made moves before a bot is shown it. A frozen server still
    /// makes chunks, sends them and shows their entities (`ServerChunkCache.tick` runs
    /// the tracker whether or not the game runs normally), and a player still moves;
    /// no other entity ticks and nothing spawns.
    pub frozen: bool,
}

impl OracleWorld {
    /// The flat world of [`Oracle::start`], with no mob in it.
    pub const FLAT: Self = Self {
        seed: None,
        view_distance: None,
        frozen: false,
    };

    /// The overworld of `seed`, frozen, with the view distance `view_distance`.
    pub fn frozen_seed(seed: i64, view_distance: u32) -> Self {
        Self {
            seed: Some(seed),
            view_distance: Some(view_distance),
            frozen: true,
        }
    }
}

/// All oracles of a process use the same directory, so only one may run at a time.
static EXCLUSIVE: Mutex<()> = Mutex::const_new(());

/// A running official server. It is killed when this value is dropped.
pub struct Oracle {
    address: String,
    _server: Child,
    /// Kept open so that the server's console does not see its input end; commands
    /// are written to it ([`Oracle::command`]).
    console: Mutex<ChildStdin>,
    /// The lines the server writes, read on so that it never blocks on a full pipe.
    lines: Mutex<UnboundedReceiver<String>>,
    /// Held until the server has been killed; declared last so it is released last.
    _exclusive: MutexGuard<'static, ()>,
}

impl Oracle {
    /// Starts the official server with a fresh world and waits until it accepts players.
    ///
    /// `accept_eula` states that the person running this agrees to the Minecraft EULA;
    /// it is also taken from the environment variable [`EULA_VARIABLE`].
    pub async fn start(accept_eula: bool) -> Result<Self> {
        Self::start_with(accept_eula, &OracleWorld::FLAT).await
    }

    /// Starts the official server with a fresh `wanted` world, as [`Oracle::start`]
    /// does with the flat one.
    pub async fn start_with(accept_eula: bool, wanted: &OracleWorld) -> Result<Self> {
        // Tests run in parallel; a second oracle waits until the first is gone.
        let exclusive = EXCLUSIVE.lock().await;
        let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
        let jar = target
            .join("datagen")
            .join(GAME_VERSION)
            .join("server.jar")
            .canonicalize()
            .context("the server jar is missing; run `cargo datagen` to download it")?;
        let directory = target.join("oracle").join(GAME_VERSION);
        std::fs::create_dir_all(&directory)?;

        accept_or_check_eula(&directory, accept_eula)?;
        // A fresh world keeps runs independent of each other.
        let world = directory.join("world");
        if world.exists() {
            std::fs::remove_dir_all(&world)?;
        }
        let port = free_port().await?;
        let mut properties = PROPERTIES.replace("{port}", &port.to_string());
        match wanted.seed {
            Some(seed) => properties.push_str(&SEEDED.replace("{seed}", &seed.to_string())),
            None => properties.push_str(FLAT),
        }
        if let Some(view) = wanted.view_distance {
            properties.push_str(&format!("view-distance={view}\n"));
        }
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

        // Keep reading so the server never blocks on a full output pipe; what it says
        // answers the commands written to its console.
        let (said, lines) = unbounded_channel();
        tokio::spawn(async move {
            while let Ok(Some(line)) = output.next_line().await {
                if said.send(line).is_err() {
                    break;
                }
            }
        });
        let oracle = Self {
            address: format!("127.0.0.1:{port}"),
            _server: server,
            console: Mutex::new(console),
            lines: Mutex::new(lines),
            _exclusive: exclusive,
        };

        if wanted.seed.is_none() {
            // Clustine had no mobs when this was written, and an animal that wanders
            // onto a spot makes the official server refuse to place a block there:
            // comparisons of placing lost a click or two in every run to that. So no
            // mob spawns, and any that has is removed, before a bot joins. Clustine had
            // no random ticks either, and a sapling the official server grew a stage
            // made a comparison of placing differ once; so none tick.
            oracle
                .command("gamerule spawn_mobs false", &["now set to"])
                .await?;
            oracle
                .command("gamerule random_tick_speed 0", &["now set to"])
                .await?;
            oracle
                .command(
                    "kill @e[type=!minecraft:player]",
                    &["Killed", "No entity was found"],
                )
                .await?;
        }
        // A seeded world keeps `spawn_mobs` on: the game rule also turns off the
        // animals a chunk is made with (`NaturalSpawner.spawnMobsForChunkGeneration`
        // reads it), which are what such a world is compared for. A frozen server
        // spawns nothing else.
        if wanted.frozen {
            oracle
                .command("tick freeze", &["The game is frozen"])
                .await?;
        }
        Ok(oracle)
    }

    /// Runs `command` on the server's console, as an operator would, and waits for the
    /// line of the server's log that holds one of `answers`, which it returns. A
    /// command the server cannot read is an error.
    pub async fn command(&self, command: &str, answers: &[&str]) -> Result<String> {
        let mut lines = self.lines.lock().await;
        // What the server said before the command answers nothing of it.
        while lines.try_recv().is_ok() {}
        {
            let mut console = self.console.lock().await;
            console.write_all(format!("{command}\n").as_bytes()).await?;
            console.flush().await?;
        }
        let answered = async {
            loop {
                let line = lines.recv().await.context("the official server exited")?;
                // The server echoes a command it cannot read with this mark in it.
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

    /// The address bots connect to, as `host:port`.
    pub fn address(&self) -> &str {
        &self.address
    }
}

/// Makes sure `eula.txt` in `directory` records agreement, writing it if the person
/// running this has stated their agreement.
fn accept_or_check_eula(directory: &Path, accept: bool) -> Result<()> {
    let eula: PathBuf = directory.join("eula.txt");
    let accept = accept || std::env::var(EULA_VARIABLE).is_ok_and(|value| value == "true");
    if accept {
        std::fs::write(&eula, "eula=true\n")?;
        return Ok(());
    }
    let agreed = std::fs::read_to_string(&eula)
        .is_ok_and(|text| text.lines().any(|line| line.trim() == "eula=true"));
    ensure!(
        agreed,
        "the official server needs you to agree to the Minecraft EULA \
         (https://aka.ms/MinecraftEULA); if you do, pass --accept-eula or set {EULA_VARIABLE}=true"
    );
    Ok(())
}

/// A port nothing listens on right now.
async fn free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    Ok(listener.local_addr()?.port())
}
