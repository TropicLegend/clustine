//! The services as processes of their own on this machine, for the tests that start,
//! stop and kill them: a coordinator, a world store, workers and an edge.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use clustine_rpc::RegionList;
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

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
    /// The chunk x coordinates at which the world store pins regions side by side,
    /// separated by commas; or nothing, for a world that begins as one home region.
    pins: String,
    /// What the coordinator is started with besides where it listens, where the store
    /// is and its lease: how it reshapes, for one. A cluster with pins tells its
    /// coordinator to reshape by hand unless this says how it reshapes.
    pub coordinator_arguments: Vec<String>,
    /// What every worker is started with besides what it needs to find the others.
    pub worker_arguments: Vec<String>,
    /// The largest view distance the edge grants, in chunks.
    pub view_distance: i32,
    /// The coordinator's lease in seconds, or `None` for the one it has when it is not
    /// told any.
    pub lease_seconds: Option<u64>,
    /// How long each worker's log was when the worker was last started again or last
    /// lost the world store: what it says after that is about its present life.
    pub since: BTreeMap<usize, usize>,
    /// How long the coordinator's log was when the coordinator was last started again.
    pub coordinator_since: usize,
}

impl Cluster {
    /// Picks addresses for a cluster of `workers` workers whose world and logs are
    /// kept in `directory`. With `pins`, chunk x coordinates separated by commas, the
    /// world is regions pinned side by side there, numbered from west to east, which
    /// stay as they are unless somebody asks or the test says how its coordinator
    /// reshapes. Without, the store and the coordinator are told nothing of it, and
    /// do what they do then.
    pub async fn new(directory: &Path, workers: usize, pins: &str) -> Self {
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
            pins: pins.to_owned(),
            coordinator_arguments: Vec::new(),
            worker_arguments: Vec::new(),
            view_distance: VIEW_DISTANCE,
            // The shortest there is. It is also how long a new coordinator waits before
            // it gives regions away.
            lease_seconds: Some(3),
            since: (0..workers).map(|worker| (worker, 0)).collect(),
            coordinator_since: 0,
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

    /// Starts the coordinator. It is told nothing of how the world is divided: it
    /// knows no region until it has read the store's list.
    pub fn start_coordinator(&mut self) {
        let lease = self.lease_seconds.map(|seconds| seconds.to_string());
        let mut arguments = vec![
            "coordinator",
            "--listen",
            &self.coordinator.0,
            // Whose list of regions tells it which regions there are.
            "--store",
            &self.store.0,
        ];
        if let Some(lease) = &lease {
            arguments.extend(["--lease-seconds", lease]);
        }
        // Pinned regions are there for a boundary at a known place and for regions
        // with known numbers, so they stay unless the test has its coordinator decide.
        let told =
            |argument: &String| argument == "--reshape" || argument.starts_with("--reshape=");
        if !self.pins.is_empty() && !self.coordinator_arguments.iter().any(told) {
            arguments.extend(["--reshape", "by-hand"]);
        }
        arguments.extend(self.coordinator_arguments.iter().map(String::as_str));
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
        // With the sign for equality, so that a coordinate west of the origin is not
        // taken for another option.
        let pins = format!("--pin={}", self.pins);
        let mut arguments = vec![
            "worldstore",
            "--listen",
            &self.store.0,
            "--world",
            self.world.to_str().unwrap(),
        ];
        // An empty list is no list of coordinates: the store is told nothing then.
        if !self.pins.is_empty() {
            arguments.push(&pins);
        }
        let store = self.spawn("worldstore", &arguments);
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

    /// The command that asks the coordinator to have the region `survivor` absorb the
    /// region `absorbed`: `clustine merge`, as whoever operates the cluster runs it.
    pub fn merge_command(&self, survivor: u32, absorbed: u32) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_clustine"));
        command
            .args(["merge", "--coordinator", &self.coordinator.0, "--survivor"])
            .arg(survivor.to_string())
            .arg("--absorbed")
            .arg(absorbed.to_string())
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .kill_on_drop(true);
        command
    }

    /// The command that asks the coordinator to split the players standing in `chunks`
    /// off `region`: `clustine split`, as whoever operates the cluster runs it.
    pub fn split_command(&self, region: u32, chunks: &[(i32, i32)]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_clustine"));
        command
            .args(["split", "--coordinator", &self.coordinator.0, "--region"])
            .arg(region.to_string())
            .arg("--chunks")
            .args(chunks.iter().map(|(x, z)| format!("{x},{z}")))
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .kill_on_drop(true);
        command
    }

    /// The regions of the world as the world store has them, which is what decides
    /// which regions there are; or why the store does not say.
    pub async fn regions(&self) -> Result<RegionList, String> {
        let store = self.store.0.clone();
        let read = tokio::task::spawn_blocking(move || clustine_worldstore::regions(&store));
        let read = read.await.expect("reading the list does not panic");
        read.map_err(|error| error.to_string())
    }

    /// The last routing table the coordinator has logged since its log was `since`
    /// bytes long, which is where the log of its present life begins.
    pub fn table(&self, since: usize) -> Option<Table> {
        let log = self.log("coordinator");
        let log = log.get(since..).unwrap_or_default();
        let mut lines = log.lines().rev();
        let line = lines.find(|line| line.contains("the routing table changed"))?;
        Some(Table(line.to_owned()))
    }

    /// The last routing table the coordinator logged in its present life.
    pub fn present_table(&self) -> Option<Table> {
        self.table(self.coordinator_since)
    }

    /// The owner of every region that has one, as the coordinator last logged its
    /// routing table: the address of the worker and the epoch.
    pub fn routes(&self) -> BTreeMap<u32, (String, u64)> {
        self.present_table()
            .map(|table| table.routes())
            .unwrap_or_default()
    }

    /// The number of the worker that listens on `address`.
    pub fn worker_at(&self, address: &str) -> Option<usize> {
        self.workers.iter().position(|worker| worker.0 == address)
    }

    /// The worker that runs `region` according to the routing table.
    pub fn owner(&self, region: u32) -> Option<usize> {
        let (address, _) = self.routes().remove(&region)?;
        self.worker_at(&address)
    }

    /// The regions each worker runs according to the routing table.
    pub fn loads(&self) -> Vec<Vec<u32>> {
        let mut loads = vec![Vec::new(); self.workers.len()];
        for (region, (address, _)) in self.routes() {
            if let Some(worker) = self.worker_at(&address) {
                loads[worker].push(region);
            }
        }
        loads
    }

    /// What a worker has logged in its present life.
    pub fn log_since(&self, worker: usize) -> String {
        let log = self.log(&worker_name(worker));
        log.get(self.since[&worker]..)
            .unwrap_or_default()
            .to_owned()
    }

    /// How many times a worker has logged `words`, in all its lives.
    pub fn said(&self, worker: usize, words: &str) -> usize {
        self.log(&worker_name(worker)).matches(words).count()
    }

    /// Whether `region` has an owner that is alive, has said in its present life that
    /// it runs the region with the epoch the routing table names, and is the one the
    /// edge last linked to for that region.
    pub fn runs(&self, region: u32) -> bool {
        let Some((address, epoch)) = self.routes().remove(&region) else {
            return false;
        };
        let Some(worker) = self.worker_at(&address) else {
            return false;
        };
        let running = format!("running a region region={region} epoch={epoch} ");
        let linked = format!("linked to a region region={region} epoch=");
        let edge = self.log("edge");
        let last_link = edge.lines().rev().find(|line| line.contains(&linked));
        self.workers[worker].1.is_some()
            && self.log_since(worker).contains(&running)
            && last_link.is_some_and(|line| line.contains(&format!("{linked}{epoch} ")))
    }

    /// Whether the loads of the workers that are there differ by one at most: then
    /// the coordinator has evened out what it would, and begins no release of its own.
    pub fn even(&self) -> bool {
        let loads = self.loads();
        let alive = (0..loads.len()).filter(|worker| self.workers[*worker].1.is_some());
        let counts: Vec<usize> = alive.map(|worker| loads[worker].len()).collect();
        let ends = counts.iter().max().zip(counts.iter().min());
        ends.is_some_and(|(most, fewest)| most - fewest <= 1)
    }

    /// Waits until every worker of a cluster that has just been started has registered,
    /// for `patience` at most and looking every `look`, and returns the name of a
    /// process that ended instead because its address was in use, if there is one.
    pub async fn taken_address(&mut self, patience: Duration, look: Duration) -> Option<String> {
        let waiting = Instant::now();
        loop {
            let mut ended = None;
            for (name, process) in self.processes() {
                let gone = process.as_mut().and_then(|child| child.try_wait().unwrap());
                if gone.is_some() {
                    ended = Some(name);
                }
            }
            if let Some(name) = ended {
                let in_use = self.log(&name).contains("Address already in use");
                // Anything else that ends a process is for the test to find.
                return in_use.then_some(name);
            }
            let registered = (0..self.workers.len()).all(|worker| {
                let log = self.log(&worker_name(worker));
                log.contains("waiting to be given a region") || log.contains("given a region")
            });
            if registered || waiting.elapsed() > patience {
                return None;
            }
            tokio::time::sleep(look).await;
        }
    }

    /// Kills a worker without warning.
    pub async fn kill_worker(&mut self, worker: usize) {
        let name = worker_name(worker);
        let mut process = self.workers[worker]
            .1
            .take()
            .unwrap_or_else(|| panic!("{name} is not running"));
        process.kill().await.unwrap();
    }

    /// Starts a worker that is not running. What it says from here on is about its
    /// present life.
    pub fn start_worker_again(&mut self, worker: usize) {
        self.since
            .insert(worker, self.log(&worker_name(worker)).len());
        self.start_worker(worker);
    }

    /// Kills the world store without warning and starts it again. From here on what
    /// the workers say about running a region has to be said anew.
    pub async fn kill_and_start_the_store(&mut self) {
        let mut store = self.store.1.take().expect("the store is running");
        store.kill().await.unwrap();
        for worker in 0..self.workers.len() {
            let length = self.log(&worker_name(worker)).len();
            self.since.insert(worker, length);
        }
        self.start_store();
    }

    /// Kills the coordinator without warning and starts another, which knows nothing
    /// of the one before.
    pub async fn kill_and_start_the_coordinator(&mut self) {
        let coordinator = self.coordinator.1.take();
        let mut coordinator = coordinator.expect("the coordinator is running");
        coordinator.kill().await.unwrap();
        self.coordinator_since = self.log("coordinator").len();
        self.start_coordinator();
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

/// A routing table as the coordinator logged it: the line it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table(pub String);

impl Table {
    /// What the table says of each region, in the words of the log: "region 1 at
    /// 127.0.0.1:4000 with epoch 7" or "region 1 without an owner".
    fn entries(&self) -> impl Iterator<Item = Vec<&str>> {
        let regions = self.0.split("regions=").nth(1).unwrap_or_default();
        regions
            .split(", ")
            .map(|entry| entry.split_whitespace().collect())
    }

    /// The owner of every region that has one: the address of the worker and the
    /// epoch.
    pub fn routes(&self) -> BTreeMap<u32, (String, u64)> {
        let routed = self.entries().filter_map(|words| match words.as_slice() {
            ["region", region, "at", address, "with", "epoch", epoch, ..] => Some((
                region.parse().ok()?,
                ((*address).to_owned(), epoch.parse().ok()?),
            )),
            _ => None,
        });
        routed.collect()
    }

    /// The regions the coordinator knows, with an owner or without, in ascending
    /// order.
    pub fn known(&self) -> Vec<u32> {
        let known = self.entries().filter_map(|words| match words.as_slice() {
            ["region", region, ..] => region.parse().ok(),
            _ => None,
        });
        known.collect()
    }
}

/// What a command that asks the coordinator for something came to: `clustine move`,
/// `clustine merge` or `clustine split`.
#[derive(Debug, Clone)]
pub struct Asked {
    /// What was asked, for a message.
    pub what: String,
    /// The status the command ended with, if it ended by itself.
    pub code: Option<i32>,
    /// What it printed, and what it complained of.
    pub said: String,
    pub complained: String,
    /// How long the command ran.
    pub took: Duration,
}

impl Asked {
    /// Whether the command ended without an error and printed `words`.
    pub fn says(&self, words: &str) -> bool {
        self.code == Some(0) && self.said.contains(words)
    }

    /// Whether the command ended with an error, printed nothing, and gave a reason
    /// with `words` in it.
    pub fn was_told_no_because(&self, words: &str) -> bool {
        self.code.is_some_and(|code| code != 0)
            && self.said.trim().is_empty()
            && self.complained.contains(words)
    }

    /// How long the command says it took, from asking to the coordinator's answer.
    pub fn own_time(&self) -> Option<Duration> {
        let (before, _) = self.said.split_once(" ms after asking")?;
        let millis = before.split_whitespace().next_back()?.parse().ok()?;
        Some(Duration::from_millis(millis))
    }

    /// In a line, for the list of what was done.
    pub fn outcome(&self) -> String {
        let said = self.said.trim().replace('\n', "; ");
        let complained = self.complained.trim().replace('\n', "; ");
        format!(
            "exit status {:?} after {:.3} s; it printed \"{said}\" and complained \"{complained}\"",
            self.code,
            self.took.as_secs_f64()
        )
    }
}

/// Runs `command`, which asks the coordinator for `what`, without waiting for what
/// comes of it.
pub fn ask(what: String, mut command: Command) -> JoinHandle<Asked> {
    tokio::spawn(async move {
        let asking = Instant::now();
        let output = command.output().await.expect("the server binary runs");
        Asked {
            what,
            code: output.status.code(),
            said: String::from_utf8_lossy(&output.stdout).into_owned(),
            complained: String::from_utf8_lossy(&output.stderr).into_owned(),
            took: asking.elapsed(),
        }
    })
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
