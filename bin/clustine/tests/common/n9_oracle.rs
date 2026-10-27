//! The official server with the classic flat world of a given seed and its own natural
//! spawning, for the comparison of spawning (`docs/groundwork/n9-plan.md`, section 11,
//! comparison 11, and decision 3 of section 18: both servers get the same seed).
//!
//! The harness's `clustine_botswarm::Oracle` serves the flat world without a seed, and
//! turns off spawning by a command before a bot joins; a seeded world it serves is the
//! game's overworld. Neither is what this comparison needs, and the harness is not this
//! part's to change, so this starts the jar itself the same way, in a directory of its
//! own, with no command run at all. It belongs in the harness as an `OracleWorld` of a
//! flat world with a seed and spawning on; until then it lives here.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use clustine_data::GAME_VERSION;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::timeout;

const START_TIMEOUT: Duration = Duration::from_secs(180);

/// A running official server; killed when dropped.
pub struct FlatSeeded {
    address: String,
    _server: Child,
}

impl FlatSeeded {
    /// Starts the jar `cargo datagen` keeps on a fresh classic flat world of `seed`,
    /// granting `view_distance` and simulating as far, with every game rule at its
    /// default, and waits until it accepts players.
    pub async fn start(seed: i64, view_distance: u32) -> Result<FlatSeeded> {
        let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
        let jar = target
            .join("datagen")
            .join(GAME_VERSION)
            .join("server.jar")
            .canonicalize()
            .context("the server jar is missing; run `cargo datagen` to download it")?;
        let directory = target.join("oracle-n9").join(GAME_VERSION);
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
        let world = directory.join("world");
        if world.exists() {
            std::fs::remove_dir_all(&world)?;
        }
        let port = {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            listener.local_addr()?.port()
        };
        // The classic flat world: the game's default layers and plains, which is what
        // `level-type=flat` with no settings gives, and Clustine's flat world is.
        let properties = format!(
            "online-mode=false\nwhite-list=false\nserver-ip=127.0.0.1\nserver-port={port}\n\
             gamemode=creative\nspawn-protection=0\nenforce-secure-profile=false\nmotd=oracle\n\
             level-type=minecraft\\:flat\nlevel-seed={seed}\ngenerate-structures=false\n\
             view-distance={view_distance}\nsimulation-distance={view_distance}\n"
        );
        std::fs::write(directory.join("server.properties"), properties)?;
        let mut server = Command::new("java")
            .args(["-Xmx2G", "-jar"])
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
        // Read on, so that the server never blocks on a full pipe.
        tokio::spawn(async move { while let Ok(Some(_)) = output.next_line().await {} });
        Ok(FlatSeeded {
            address: format!("127.0.0.1:{port}"),
            _server: server,
        })
    }

    pub fn address(&self) -> &str {
        &self.address
    }
}
