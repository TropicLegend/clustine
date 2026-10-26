//! The official server, run with a world of its own for the comparison of light after
//! changes: started with the jar `cargo datagen` keeps, given commands on its console,
//! saved and stopped.
//!
//! `clustine_botswarm::Oracle` does the same for the comparisons with bots, but it is
//! asynchronous and serves a flat world four blocks deep; this crate's tests have no
//! runtime and want a ground deep enough to dig caves in, and nothing else of it. The
//! world is kept in a directory of its own (`target/oracle-light/`), so that an oracle of
//! another test process never shares it.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Duration;

use clustine_data::GAME_VERSION;

/// The environment variable by which the person running the test states that they agree
/// to the Minecraft EULA, as for every other comparison.
const EULA_VARIABLE: &str = "CLUSTINE_ACCEPT_MINECRAFT_EULA";

/// The most the server may take to start or to answer a command. A bound, not a wait:
/// every wait below ends on what the server says.
const ANSWER_BOUND: Duration = Duration::from_secs(180);

/// A creative server in offline mode with a flat world of bedrock, 59 stone, 3 dirt and
/// grass, whose top is at y -1: deep enough for a cave under the ground, and every layer
/// of the ground a uniform section or a part of one. No structures, no features.
///
/// No player ever joins, and the game stops ticking a server that has been empty for a
/// minute unless told not to (`pause-when-empty-seconds`); the comparison counts ticks,
/// so it is told. Nor is a long tick taken for a hung server (`max-tick-time`): a full
/// check may run beside it.
const PROPERTIES: &str = r#"online-mode=false
pause-when-empty-seconds=-1
max-tick-time=-1
white-list=false
server-ip=127.0.0.1
server-port={port}
gamemode=creative
spawn-protection=0
enforce-secure-profile=false
motd=light
level-type=minecraft\:flat
generator-settings={"layers"\:[{"block"\:"minecraft\:bedrock","height"\:1},{"block"\:"minecraft\:stone","height"\:59},{"block"\:"minecraft\:dirt","height"\:3},{"block"\:"minecraft\:grass_block","height"\:1}],"biome"\:"minecraft\:plains"}
generate-structures=false
"#;

/// A running official server. It is killed when this value is dropped, unless it was
/// stopped first.
pub struct Server {
    process: Child,
    console: ChildStdin,
    lines: Receiver<String>,
    world: PathBuf,
}

impl Server {
    /// Starts the official server with a fresh world and waits until it says it is done
    /// starting. Panics, saying why, where the jar is missing or nobody agreed to the
    /// EULA.
    pub fn start() -> Self {
        let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
        let jar = target
            .join("datagen")
            .join(GAME_VERSION)
            .join("server.jar")
            .canonicalize()
            .expect("the server jar is missing; run `cargo datagen` to download it");
        let directory = target.join("oracle-light").join(GAME_VERSION);
        std::fs::create_dir_all(&directory).expect("the server's directory is made");
        agree_or_check_eula(&directory);
        // A fresh world keeps runs independent of each other.
        let world = directory.join("world");
        if world.exists() {
            std::fs::remove_dir_all(&world).expect("the old world is removed");
        }
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .expect("a free port")
            .port();
        std::fs::write(
            directory.join("server.properties"),
            PROPERTIES.replace("{port}", &port.to_string()),
        )
        .expect("the server's properties are written");

        let mut process = Command::new("java")
            .args(["-Xmx2G", "-jar"])
            .arg(&jar)
            .arg("nogui")
            .current_dir(&directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("starting `java`; is a Java runtime installed?");
        let console = process.stdin.take().expect("the server's console");
        let output = process.stdout.take().expect("the server's output");
        // Read on a thread of its own, so that the server never blocks on a full pipe.
        let (said, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else { break };
                if said.send(line).is_err() {
                    break;
                }
            }
        });
        let server = Self {
            process,
            console,
            lines,
            world,
        };
        let mut log = Vec::new();
        loop {
            match server.lines.recv_timeout(ANSWER_BOUND) {
                Ok(line) if line.contains("Done (") => break,
                Ok(line) => log.push(line),
                Err(RecvTimeoutError::Timeout) => {
                    panic!("the official server did not start:\n{}", log.join("\n"))
                }
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("the official server exited:\n{}", log.join("\n"))
                }
            }
        }
        server
    }

    /// The directory of the server's world.
    pub fn world(&self) -> &Path {
        &self.world
    }

    /// Runs `command` on the console and returns the line of the log that answers it:
    /// one that holds one of `succeeded`. A line that holds one of `failed`, or the mark
    /// of a command the server could not read, fails the test with the server's words.
    pub fn command(&mut self, command: &str, succeeded: &[&str], failed: &[&str]) -> String {
        // What the server said before the command answers nothing of it.
        while self.lines.try_recv().is_ok() {}
        writeln!(self.console, "{command}").expect("the server's console takes a command");
        self.console
            .flush()
            .expect("the server's console is flushed");
        loop {
            let line = match self.lines.recv_timeout(ANSWER_BOUND) {
                Ok(line) => line,
                Err(error) => panic!("the official server did not answer `{command}`: {error}"),
            };
            assert!(
                !line.contains("<--[HERE]"),
                "the official server could not read `{command}`: {line}"
            );
            assert!(
                !failed.iter().any(|answer| line.contains(answer)),
                "the official server refused `{command}`: {line}"
            );
            if succeeded.iter().any(|answer| line.contains(answer)) {
                return line;
            }
        }
    }

    /// The game's time in ticks, as the server counts it.
    pub fn game_time(&mut self) -> u64 {
        let line = self.command("time query gametime", &["The game time is"], &[]);
        let after = line
            .split("The game time is ")
            .nth(1)
            .expect("the answer holds the time");
        after
            .split(' ')
            .next()
            .and_then(|number| number.parse().ok())
            .unwrap_or_else(|| panic!("no game time in `{line}`"))
    }

    /// Returns once the game has run at least `ticks` ticks from now: read from the game,
    /// so that a slow server is waited for as long as it is slow. The reads are spaced so
    /// as not to flood the console; the bound only ends a server that stopped ticking.
    pub fn let_ticks_pass(&mut self, ticks: u64) {
        let until = self.game_time() + ticks;
        for _ in 0..12_000 {
            if self.game_time() >= until {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("the official server did not run {ticks} ticks");
    }

    /// Saves every chunk and waits until the files are written.
    pub fn save(&mut self) {
        self.command(
            "save-all flush",
            &["Saved the game"],
            &["Unable to save the game"],
        );
    }

    /// Stops the server, which saves the world, and waits until its process has ended.
    pub fn stop(mut self) {
        writeln!(self.console, "stop").expect("the server's console takes a command");
        self.console
            .flush()
            .expect("the server's console is flushed");
        // The process ends when it is done saving; its output ends with it.
        for _ in 0..7_200 {
            if let Some(status) = self.process.try_wait().expect("the server's state") {
                assert!(
                    status.success(),
                    "the official server stopped with {status}"
                );
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("the official server did not stop");
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // A test that failed leaves no server running.
        if matches!(self.process.try_wait(), Ok(None)) {
            let _ = self.process.kill();
            let _ = self.process.wait();
        }
    }
}

/// Makes sure `eula.txt` records agreement, writing it if the person running the test
/// has stated theirs.
fn agree_or_check_eula(directory: &Path) {
    let eula = directory.join("eula.txt");
    if std::env::var(EULA_VARIABLE).is_ok_and(|value| value == "true") {
        std::fs::write(&eula, "eula=true\n").expect("eula.txt is written");
        return;
    }
    let agreed = std::fs::read_to_string(&eula)
        .is_ok_and(|text| text.lines().any(|line| line.trim() == "eula=true"));
    assert!(
        agreed,
        "the official server needs you to agree to the Minecraft EULA \
         (https://aka.ms/MinecraftEULA); if you do, set {EULA_VARIABLE}=true"
    );
}
