//! The services as processes of their own on this machine, for the tests that start,
//! stop and kill them: a coordinator, a world store, workers and an edge.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::{Child, Command};

use super::{VIEW_DISTANCE, free_address};

const PATIENCE: Duration = Duration::from_secs(30);

/// The name of the worker numbered `number`.
pub fn worker_name(number: usize) -> String {
    format!("worker-{number}")
}

/// The processes of a cluster and where they listen.
pub struct Cluster {
    pub world: PathBuf,
    /// Where each process writes its log, in a file named after it.
    pub logs: PathBuf,
    pub coordinator: (String, Option<Child>),
    pub store: (String, Option<Child>),
    /// The workers; the one numbered `n` is called `worker-n`.
    pub workers: Vec<(String, Option<Child>)>,
    pub edge: (String, Option<Child>),
    /// The chunk x coordinates at which the coordinator divides the world, separated
    /// by commas.
    boundaries: String,
    /// What every worker is started with besides what it needs to find the others.
    pub worker_arguments: Vec<String>,
    /// The largest view distance the edge grants, in chunks.
    pub view_distance: i32,
    /// The coordinator's lease in seconds, or `None` for the one it has when it is not
    /// told any.
    pub lease_seconds: Option<u64>,
}

impl Cluster {
    /// Picks addresses for a cluster of `workers` workers whose world, divided at
    /// `boundaries`, and logs are kept in `directory`.
    pub async fn new(directory: &Path, workers: usize, boundaries: &str) -> Self {
        let logs = directory.join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let mut addresses = Vec::new();
        for _ in 0..workers {
            addresses.push((free_address().await, None));
        }
        Self {
            world: directory.join("world"),
            logs,
            coordinator: (free_address().await, None),
            store: (free_address().await, None),
            workers: addresses,
            edge: (free_address().await, None),
            boundaries: boundaries.to_owned(),
            worker_arguments: Vec::new(),
            view_distance: VIEW_DISTANCE,
            // The shortest there is. It is also how long a new coordinator waits before
            // it gives regions away.
            lease_seconds: Some(3),
        }
    }

    /// Starts a process of the server binary with the given arguments. What it logs is
    /// appended to a file named after it.
    pub fn spawn(&self, name: &str, arguments: &[&str]) -> Child {
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
    pub async fn start(&mut self) {
        let edge = self.spawn(
            "edge",
            &[
                "edge",
                "--coordinator",
                &self.coordinator.0,
                "--bind",
                &self.edge.0,
                "--view-distance",
                &self.view_distance.to_string(),
            ],
        );
        self.edge.1 = Some(edge);
        for number in 0..self.workers.len() {
            self.start_worker(number);
        }
        self.start_store();
        self.start_coordinator();

        for _ in 0..600 {
            if clustine_botswarm::ping(&self.edge.0).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the cluster did not come up:\n{}", self.all_logs());
    }

    /// Starts the coordinator.
    pub fn start_coordinator(&mut self) {
        let lease = self.lease_seconds.map(|seconds| seconds.to_string());
        let mut arguments = vec![
            "coordinator",
            "--listen",
            &self.coordinator.0,
            "--boundaries",
            &self.boundaries,
        ];
        if let Some(lease) = &lease {
            arguments.extend(["--lease-seconds", lease]);
        }
        let coordinator = self.spawn("coordinator", &arguments);
        self.coordinator.1 = Some(coordinator);
    }

    /// Starts the worker numbered `number`.
    pub fn start_worker(&mut self, number: usize) {
        let name = worker_name(number);
        let mut arguments = vec![
            "worker",
            "--coordinator",
            &self.coordinator.0,
            "--store",
            &self.store.0,
            "--listen",
            &self.workers[number].0,
            "--name",
            &name,
        ];
        arguments.extend(self.worker_arguments.iter().map(String::as_str));
        let worker = self.spawn(&name, &arguments);
        self.workers[number].1 = Some(worker);
    }

    /// Starts the world store on the cluster's world.
    pub fn start_store(&mut self) {
        let store = self.spawn(
            "worldstore",
            &[
                "worldstore",
                "--listen",
                &self.store.0,
                "--world",
                self.world.to_str().unwrap(),
                // As the coordinator divides the world, or the store would refuse the
                // workers' hellos.
                "--boundaries",
                &self.boundaries,
            ],
        );
        self.store.1 = Some(store);
    }

    /// The command that asks the coordinator to move `region`, to the worker called `to`
    /// or to any: `clustine move`, as whoever operates the cluster runs it. What it
    /// prints is for the caller to read.
    pub fn move_command(&self, region: usize, to: Option<&str>) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_clustine"));
        command
            .args(["move", "--coordinator", &self.coordinator.0, "--region"])
            .arg(region.to_string())
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .kill_on_drop(true);
        if let Some(to) = to {
            command.args(["--to", to]);
        }
        command
    }

    /// Waits until the process `name` has logged `message` at least `times` times.
    pub async fn wait_for_log(&self, name: &str, message: &str, times: usize) {
        for _ in 0..600 {
            if self.log(name).matches(message).count() >= times {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!(
            "{name} did not log `{message}` {times} times:\n{}",
            self.all_logs()
        );
    }

    /// Every process by name, in the order in which they are asked to stop. The
    /// coordinator goes before the workers: a worker that is told to stop while there
    /// is a coordinator waits up to 20 seconds for somebody to hand its region to, and
    /// here nobody comes.
    pub fn processes(&mut self) -> Vec<(String, &mut Option<Child>)> {
        let mut processes = vec![
            ("edge".to_owned(), &mut self.edge.1),
            ("coordinator".to_owned(), &mut self.coordinator.1),
        ];
        for (number, worker) in self.workers.iter_mut().enumerate() {
            processes.push((worker_name(number), &mut worker.1));
        }
        processes.push(("worldstore".to_owned(), &mut self.store.1));
        processes
    }

    /// Kills every process without warning.
    pub async fn kill(&mut self) {
        for (_, process) in self.processes() {
            if let Some(mut process) = process.take() {
                process.kill().await.unwrap();
            }
        }
    }

    /// Asks every process to stop, the way Kubernetes does, in the order given by
    /// `processes`, and checks that each ends without an error.
    pub async fn terminate(&mut self) {
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
                std::fs::read_to_string(logs.join(&name)).unwrap_or_default()
            );
        }
    }

    /// What the process called `name` has logged since the cluster was made.
    pub fn log(&self, name: &str) -> String {
        std::fs::read_to_string(self.logs.join(name)).unwrap_or_default()
    }

    pub fn all_logs(&self) -> String {
        self.names()
            .iter()
            .map(|name| format!("--- {name} ---\n{}", self.log(name)))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The names of the processes, which are also the names of their logs.
    pub fn names(&self) -> Vec<String> {
        let mut names = vec!["coordinator".to_owned(), "worldstore".to_owned()];
        names.extend((0..self.workers.len()).map(worker_name));
        names.push("edge".to_owned());
        names
    }
}

/// A test's leave to run its cluster; the others that wait get theirs when it is
/// dropped.
#[allow(dead_code)] // Held, never looked at.
pub struct Turn(tokio::sync::OwnedSemaphorePermit);

/// How many test clusters run at the same time, and what hands out the turns.
fn turns() -> &'static (std::sync::Arc<tokio::sync::Semaphore>, u32) {
    static TURNS: std::sync::OnceLock<(std::sync::Arc<tokio::sync::Semaphore>, u32)> =
        std::sync::OnceLock::new();
    TURNS.get_or_init(|| {
        // A cluster is half a dozen processes that mostly wait, for leases above all,
        // so a test takes minutes and a fraction of a processor. One for every two
        // processors leaves room for the moments in which a cluster does work: with
        // more, clusters starve each other until leases run out by themselves, which
        // says nothing about the server. `CLUSTINE_TEST_CLUSTERS` sets another number.
        let processors = std::thread::available_parallelism().map_or(2, |count| count.get());
        let asked = std::env::var("CLUSTINE_TEST_CLUSTERS").ok();
        let count = asked
            .and_then(|count| count.parse().ok())
            .unwrap_or(processors / 2)
            .clamp(1, 16) as u32;
        (
            std::sync::Arc::new(tokio::sync::Semaphore::new(count as usize)),
            count,
        )
    })
}

/// Waits until this test may run its cluster beside those that run already.
#[allow(dead_code)] // Not every test binary uses it.
pub async fn turn() -> Turn {
    let (turns, _) = turns();
    Turn(turns.clone().acquire_owned().await.expect("never closed"))
}

/// Waits until this test may run its cluster with no other beside it, for a test that
/// measures how long something takes. Those that asked before it finish first, and
/// those that ask after it wait for it.
#[allow(dead_code)] // Not every test binary uses it.
pub async fn turn_alone() -> Turn {
    let (turns, count) = turns();
    Turn(
        turns
            .clone()
            .acquire_many_owned(*count)
            .await
            .expect("never closed"),
    )
}
