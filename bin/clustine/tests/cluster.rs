//! The services as processes of their own on this machine: a coordinator, a world
//! store, two workers and an edge, which together are one server.

mod common;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use clustine_botswarm::{Bot, Crossing, cross};
use clustine_data::blocks;
use clustine_protocol::packets::play::face;
use tokio::process::{Child, Command};

use common::{VIEW_DISTANCE, free_address, view_area};

const PATIENCE: Duration = Duration::from_secs(30);

const AIR: Option<i32> = Some(blocks::AIR.0 as i32);
const STONE: Option<i32> = Some(blocks::STONE.0 as i32);

/// The chunk x coordinate at which the world is divided: block x = 48.
const BOUNDARY: &str = "3";

/// The processes of a cluster and where they listen.
struct Cluster {
    world: PathBuf,
    /// Where each process writes its log.
    logs: PathBuf,
    coordinator: (String, Option<Child>),
    store: (String, Option<Child>),
    workers: [(String, Option<Child>); 2],
    edge: (String, Option<Child>),
}

impl Cluster {
    /// Picks addresses for a cluster whose world and logs are kept in `directory`.
    async fn new(directory: &Path) -> Self {
        let logs = directory.join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        Self {
            world: directory.join("world"),
            logs,
            coordinator: (free_address().await, None),
            store: (free_address().await, None),
            workers: [(free_address().await, None), (free_address().await, None)],
            edge: (free_address().await, None),
        }
    }

    /// Starts a process of the server binary with the given arguments. What it logs is
    /// appended to a file named after it.
    fn spawn(&self, name: &str, arguments: &[&str]) -> Child {
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.logs.join(name))
            .unwrap();
        Command::new(env!("CARGO_BIN_EXE_clustine"))
            .args(arguments)
            .env("NO_COLOR", "1")
            .stdout(Stdio::null())
            .stderr(log)
            .kill_on_drop(true)
            .spawn()
            .unwrap()
    }

    /// Starts every process, the ones that depend on others first, so that each has to
    /// wait for what it needs. Returns once players can join.
    async fn start(&mut self) {
        let edge = self.spawn(
            "edge",
            &[
                "edge",
                "--coordinator",
                &self.coordinator.0,
                "--bind",
                &self.edge.0,
                "--view-distance",
                &VIEW_DISTANCE.to_string(),
            ],
        );
        self.edge.1 = Some(edge);
        for number in 0..2 {
            let name = format!("worker-{number}");
            let worker = self.spawn(
                &name,
                &[
                    "worker",
                    "--coordinator",
                    &self.coordinator.0,
                    "--store",
                    &self.store.0,
                    "--listen",
                    &self.workers[number].0,
                    "--name",
                    &name,
                ],
            );
            self.workers[number].1 = Some(worker);
        }
        let store = self.spawn(
            "worldstore",
            &[
                "worldstore",
                "--listen",
                &self.store.0,
                "--world",
                self.world.to_str().unwrap(),
            ],
        );
        self.store.1 = Some(store);
        let coordinator = self.spawn(
            "coordinator",
            &[
                "coordinator",
                "--listen",
                &self.coordinator.0,
                "--boundaries",
                BOUNDARY,
                // The shortest there is. It is also how long a new coordinator waits
                // before it gives regions away.
                "--lease-seconds",
                "3",
            ],
        );
        self.coordinator.1 = Some(coordinator);

        for _ in 0..600 {
            if clustine_botswarm::ping(&self.edge.0).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the cluster did not come up:\n{}", self.all_logs());
    }

    fn processes(&mut self) -> Vec<(&'static str, &mut Option<Child>)> {
        let [first, second] = &mut self.workers;
        vec![
            ("edge", &mut self.edge.1),
            ("worker-0", &mut first.1),
            ("worker-1", &mut second.1),
            ("worldstore", &mut self.store.1),
            ("coordinator", &mut self.coordinator.1),
        ]
    }

    /// Kills every process without warning.
    async fn kill(&mut self) {
        for (_, process) in self.processes() {
            if let Some(mut process) = process.take() {
                process.kill().await.unwrap();
            }
        }
    }

    /// Asks every process to stop, the way Kubernetes does, in the order given by
    /// `processes`, and checks that each ends without an error.
    async fn terminate(&mut self) {
        let logs = self.logs.clone();
        for (name, process) in self.processes() {
            let Some(mut process) = process.take() else {
                continue;
            };
            let pid = process.id().unwrap().to_string();
            let sent = Command::new("kill").args(["-TERM", &pid]).status().await;
            assert!(sent.unwrap().success());
            let status = tokio::time::timeout(PATIENCE, process.wait())
                .await
                .unwrap_or_else(|_| panic!("{name} did not stop"))
                .unwrap();
            assert!(
                status.success(),
                "{name} ended with {status}:\n{}",
                std::fs::read_to_string(logs.join(name)).unwrap_or_default()
            );
        }
    }

    fn log(&self, name: &str) -> String {
        std::fs::read_to_string(self.logs.join(name)).unwrap_or_default()
    }

    fn all_logs(&self) -> String {
        ["coordinator", "worldstore", "worker-0", "worker-1", "edge"]
            .map(|name| format!("--- {name} ---\n{}", self.log(name)))
            .join("\n")
    }
}

/// Joins and waits for the chunks around the spawn point, which reach across the
/// boundary.
async fn join(address: &str, name: &str) -> Bot {
    let mut bot = Bot::join(address, name).await.unwrap();
    let count = view_area((0, 0), VIEW_DISTANCE).len();
    bot.wait_for_chunks(count, PATIENCE).await.unwrap();
    bot
}

/// Breaks a block west of the boundary and places one east of it, each from its own
/// side, and waits until the server has confirmed both.
async fn build_on_both_sides(bot: &mut Bot) {
    bot.walk_to(45.5, 0.5, 0.5).await.unwrap();
    bot.dig(46, -61, 1).await.unwrap();
    bot.walk_to(49.5, 0.5, 0.5).await.unwrap();
    let last = bot.use_item_on(50, -61, 1, face::TOP).await.unwrap();
    bot.wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= last)
        .await
        .unwrap();
    assert_built_on_both_sides(bot);
}

fn assert_built_on_both_sides(bot: &Bot) {
    assert_eq!(bot.block_at(46, -61, 1).unwrap(), AIR);
    assert_eq!(bot.block_at(50, -60, 1).unwrap(), STONE);
}

/// Bots walk back and forth between two worker processes and build on both sides. What
/// they built outlasts every process being killed, and each process stops cleanly when
/// asked to.
#[tokio::test(flavor = "multi_thread")]
async fn a_cluster_of_processes_is_one_server() {
    let directory = tempfile::tempdir().unwrap();
    let mut cluster = Cluster::new(directory.path()).await;
    cluster.start().await;
    let address = cluster.edge.0.clone();

    let crossing = Crossing {
        walkers: 3,
        rounds: 2,
        west: 20.5,
        // As far east as the watcher at the spawn point sees with the tests' view distance.
        east: 70.5,
        ..Crossing::default()
    };
    let report = cross(&address, &crossing)
        .await
        .unwrap_or_else(|error| panic!("{error:#}\n{}", cluster.all_logs()));
    assert_eq!(report.crossings, 12);
    assert_eq!(report.blocks_built, 12);
    // Both workers took part: each let every walker go twice and took them in twice.
    for worker in ["worker-0", "worker-1"] {
        let log = cluster.log(worker);
        for message in [
            "player arrived from another region",
            "player departed to another region",
        ] {
            let count = log.matches(message).count();
            assert!(
                count >= 6,
                "{worker} logged `{message}` {count} times:\n{log}"
            );
        }
    }

    // What both regions were told is on disk before players are, whatever becomes of
    // the processes.
    let mut builder = join(&address, "Builder").await;
    build_on_both_sides(&mut builder).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    cluster.kill().await;
    drop(builder);

    cluster.start().await;
    let mut visitor = join(&address, "Visitor").await;
    assert_built_on_both_sides(&visitor);

    // Asked to stop, the workers hand what has changed to the world store, which then
    // has nothing left in its logs.
    visitor.dig(2, -61, 1).await.unwrap();
    visitor.walk_to(49.5, 0.5, 0.5).await.unwrap();
    let last = visitor.dig(50, -60, 1).await.unwrap();
    visitor
        .wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= last)
        .await
        .unwrap();
    drop(visitor);
    cluster.terminate().await;
    for region in 0..2 {
        let log = cluster.world.join(format!("logs/{region}.wal"));
        assert_eq!(
            std::fs::metadata(&log).unwrap().len(),
            0,
            "{}",
            log.display()
        );
    }

    // And the next cluster on that world finds it as it was left.
    cluster.start().await;
    let visitor = join(&address, "Visitor").await;
    assert_eq!(visitor.block_at(2, -61, 1).unwrap(), AIR);
    assert_eq!(visitor.block_at(46, -61, 1).unwrap(), AIR);
    assert_eq!(visitor.block_at(50, -60, 1).unwrap(), AIR);
    drop(visitor);
    cluster.terminate().await;
}
