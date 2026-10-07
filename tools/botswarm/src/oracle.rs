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
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpListener;
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, MutexGuard};
use tokio::time::timeout;

/// Setting this environment variable to `true` states that the person running the
/// command agrees to the Minecraft EULA.
pub const EULA_VARIABLE: &str = "CLUSTINE_ACCEPT_MINECRAFT_EULA";

/// How long the server may take to start. It usually needs a few seconds.
const START_TIMEOUT: Duration = Duration::from_secs(180);

/// A flat creative world in offline mode, matching what Clustine serves. `{port}` is
/// replaced by a free port.
const PROPERTIES: &str = r#"online-mode=false
white-list=false
server-ip=127.0.0.1
server-port={port}
gamemode=creative
level-type=minecraft\:flat
generator-settings={"layers"\:[{"block"\:"minecraft\:bedrock","height"\:1},{"block"\:"minecraft\:dirt","height"\:2},{"block"\:"minecraft\:grass_block","height"\:1}],"biome"\:"minecraft\:plains"}
generate-structures=false
spawn-protection=0
enforce-secure-profile=false
motd=oracle
"#;

/// All oracles of a process use the same directory, so only one may run at a time.
static EXCLUSIVE: Mutex<()> = Mutex::const_new(());

/// A running official server. It is killed when this value is dropped.
pub struct Oracle {
    address: String,
    _server: Child,
    /// Held until the server has been killed; declared last so it is released last.
    _exclusive: MutexGuard<'static, ()>,
}

impl Oracle {
    /// Starts the official server with a fresh world and waits until it accepts players.
    ///
    /// `accept_eula` states that the person running this agrees to the Minecraft EULA;
    /// it is also taken from the environment variable [`EULA_VARIABLE`].
    pub async fn start(accept_eula: bool) -> Result<Self> {
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
        std::fs::write(
            directory.join("server.properties"),
            PROPERTIES.replace("{port}", &port.to_string()),
        )?;

        let mut server = Command::new("java")
            .args(["-Xmx1G", "-jar"])
            .arg(&jar)
            .arg("nogui")
            .current_dir(&directory)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("starting `java`; is a Java runtime installed?")?;
        let mut output = BufReader::new(server.stdout.take().context("no server output")?).lines();

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
        // Keep reading so the server never blocks on a full output pipe.
        tokio::spawn(async move { while let Ok(Some(_)) = output.next_line().await {} });

        Ok(Self {
            address: format!("127.0.0.1:{port}"),
            _server: server,
            _exclusive: exclusive,
        })
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
