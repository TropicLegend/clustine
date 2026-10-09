//! Regions are merged and split when somebody asks the coordinator for it, with
//! `clustine merge` and `clustine split`, on a cluster of real processes: a
//! coordinator, a world store, workers and an edge. These are the scenarios P1 to P4
//! of `docs/adr/0014-merging-and-splitting.md`, section 10: what the processes and
//! the commands do, **without players**. What players notice of a merge or a split,
//! and that nothing of theirs is lost, is for the tests under bots to show.
//!
//! A split does nothing unless a player stands in a chunk it names, so P1 to P4 never
//! see a worker run the part of one. One test beyond them puts a bot into a region
//! for that, and looks at the processes only: at what the command says, at the world
//! store's list, and at who runs the new region, also under a coordinator that knows
//! nothing of the split. It asks nothing of what the bot is shown.
//!
//! The last test has no coordinator's process. It is about what a worker does that is
//! given a region which was absorbed; a coordinator knows no region but those of the
//! world store's list, so none gives such a region out, and the test plays the
//! coordinator to do it.
//!
//! What is true of the world is read where it is decided: in the world store's list
//! of regions. Who runs what is read from the routing table the coordinator logs and
//! from what the workers log, as in the tests of moves.
//!
//! The tests print how long each command took. `CLUSTINE_RESHAPES_KEEP` keeps the
//! processes' logs of a test that passes; those of a test that fails are always
//! kept, and the failure says where.
//!
//! The processes are those of an unoptimised build, as in every test here.

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clustine_botswarm::Bot;
use clustine_region::RegionId;
use clustine_rpc::link::End;
use clustine_rpc::{Assignment, FromCoordinator, RegionList, ToCoordinator, tcp};
use clustine_world::{EntityId, EntityIds, Vec3};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use common::processes::{Asked, Cluster, Turn, ask, turn, worker_name};

/// How long anything may take that is merely waited for. It only ever runs out when
/// something hangs.
const PATIENCE: Duration = Duration::from_secs(60);

/// How often a state that is waited for is looked at.
const LOOK: Duration = Duration::from_millis(10);

/// The coordinator's lease when it is told none, which is how these tests start it.
const LEASE: Duration = Duration::from_secs(5);

/// What a region is called on the command line and in the logs.
type Region = u32;

/// Whether this run of the tests is the one that repeats the end-to-end tests on a
/// world divided into regions, which `CLUSTINE_TEST_PINS` asks for. These tests
/// divide their worlds themselves, so they run once, in the run without it.
fn a_repetition() -> bool {
    std::env::var_os("CLUSTINE_TEST_PINS").is_some()
}

/// A cluster without players, and the means to ask its coordinator for things and to
/// do harm to its workers.
struct Reshapes {
    /// Its leave to run beside the other tests' clusters.
    _turn: Turn,
    /// Where the world and the logs are; taken out when they are to be kept.
    directory: Option<TempDir>,
    cluster: Cluster,
    test: String,
    started: Instant,
    /// What was done to the cluster and what it did about it, in order.
    deeds: Vec<String>,
    /// How long the coordinator's log was when the coordinator was last started.
    coordinator_since: usize,
}

impl Reshapes {
    /// Starts a cluster of `workers` workers whose world is divided at `boundaries`.
    /// Returns once every region runs and the workers' loads differ by one at most,
    /// so that the coordinator moves nothing by itself from here on.
    async fn start(test: &str, boundaries: &str, workers: usize) -> Self {
        let turn = turn().await;
        let directory = tempfile::Builder::new()
            .prefix("clustine-reshapes-")
            .tempdir()
            .unwrap();
        let mut cluster = Cluster::new(directory.path(), workers, boundaries).await;
        // The lease a cluster has unless someone says otherwise.
        cluster.lease_seconds = None;
        cluster.start().await;
        let mut reshapes = Self {
            _turn: turn,
            directory: Some(directory),
            cluster,
            test: test.to_owned(),
            started: Instant::now(),
            deeds: Vec::new(),
            coordinator_since: 0,
        };
        let list = reshapes.settled().await;
        // The edge links to the regions a moment after they run. A test that counts
        // its links, to see whether a merge or a split made it link anew, begins when
        // the first ones are there.
        let regions = Self::living(&list);
        let linked = |reshapes: &Self| regions.iter().all(|region| reshapes.links_to(*region) > 0);
        reshapes
            .until("the edge has linked to every region", linked)
            .await;
        reshapes
    }

    /// Notes something that was done to the cluster, or that it did.
    fn note(&mut self, deed: String) {
        let deed = format!("{:7.3} s: {deed}", self.started.elapsed().as_secs_f64());
        println!("{}: {deed}", self.test);
        self.deeds.push(deed);
    }

    /// Fails the test, saying what led up to it and where the logs are, which are kept.
    fn fail(&mut self, message: &str) -> ! {
        self.note("failed".to_owned());
        let kept = self.keep();
        let mut report = format!("{message}\n\nwhat was done:\n");
        for deed in &self.deeds {
            report.push_str(&format!("  {deed}\n"));
        }
        report.push_str(&format!(
            "the routing table last logged: {:?}\n",
            self.routes()
        ));
        report.push_str("the logs of the processes:\n");
        for name in self.cluster.names() {
            report.push_str(&format!("  {}\n", kept.join("logs").join(name).display()));
        }
        panic!("{report}");
    }

    /// Keeps the directory with the world and the logs beyond the test.
    fn keep(&mut self) -> PathBuf {
        match self.directory.take() {
            Some(directory) => directory.keep(),
            None => self.cluster.logs.parent().unwrap().to_owned(),
        }
    }

    /// Fails if a process has ended that nobody killed.
    fn tend(&mut self) {
        let mut ended = None;
        for (name, process) in self.cluster.processes() {
            let gone = process.as_mut().and_then(|child| child.try_wait().unwrap());
            if let Some(status) = gone {
                ended = Some(format!("{name} ended by itself ({status})"));
            }
        }
        if let Some(ended) = ended {
            self.fail(&ended);
        }
    }

    /// Waits until `state` holds, looking after the processes meanwhile, and returns
    /// how long that took. Fails, naming `what` was waited for, if it takes longer than
    /// anything should.
    async fn until(&mut self, what: &str, mut state: impl FnMut(&Self) -> bool) -> Duration {
        let waiting = Instant::now();
        loop {
            self.tend();
            if state(self) {
                return waiting.elapsed();
            }
            if waiting.elapsed() > PATIENCE {
                self.fail(&format!("waited {PATIENCE:?} in vain until {what}"));
            }
            tokio::time::sleep(LOOK).await;
        }
    }

    /// The regions of the world as the world store has them, which is what decides;
    /// or why the store does not say.
    async fn read_list(&self) -> Result<RegionList, String> {
        self.cluster.regions().await
    }

    /// The regions of the world as the world store has them, from a store that runs.
    async fn list(&mut self) -> RegionList {
        match self.read_list().await {
            Ok(list) => list,
            Err(error) => self.fail(&format!("the world store's list cannot be read: {error}")),
        }
    }

    /// The living regions of `list`, in ascending order.
    fn living(list: &RegionList) -> Vec<Region> {
        list.regions.iter().map(|info| info.region.0).collect()
    }

    /// The last routing table the coordinator logged in its present life, as the line
    /// it is.
    fn table(&self) -> Option<String> {
        let table = self.cluster.table(self.coordinator_since)?;
        Some(table.0)
    }

    /// The owner of every region that has one, as the coordinator last logged its
    /// routing table: the address of the worker and the epoch.
    fn routes(&self) -> BTreeMap<Region, (String, u64)> {
        let table = self.cluster.table(self.coordinator_since);
        table.map(|table| table.routes()).unwrap_or_default()
    }

    /// The regions the coordinator last logged as known to it, with an owner or
    /// without, in ascending order.
    fn known(&self) -> Vec<Region> {
        let table = self.cluster.table(self.coordinator_since);
        table.map(|table| table.known()).unwrap_or_default()
    }

    /// The number of the worker that listens on `address`.
    fn worker_at(&self, address: &str) -> Option<usize> {
        let workers = &self.cluster.workers;
        workers.iter().position(|worker| worker.0 == address)
    }

    /// The worker that runs `region` according to the routing table.
    fn owner(&self, region: Region) -> Option<usize> {
        let (address, _) = self.routes().remove(&region)?;
        self.worker_at(&address)
    }

    /// The epoch `region` is run with according to the routing table.
    fn epoch(&self, region: Region) -> Option<u64> {
        self.routes().remove(&region).map(|(_, epoch)| epoch)
    }

    /// The regions each worker runs according to the routing table.
    fn loads(&self) -> Vec<Vec<Region>> {
        let mut loads = vec![Vec::new(); self.cluster.workers.len()];
        for (region, (address, _)) in self.routes() {
            if let Some(worker) = self.worker_at(&address) {
                loads[worker].push(region);
            }
        }
        loads
    }

    /// Whether `region` has an owner whose process runs and has said that it runs the
    /// region with the epoch the routing table names.
    fn runs(&self, region: Region) -> bool {
        let Some((address, epoch)) = self.routes().remove(&region) else {
            return false;
        };
        let Some(worker) = self.worker_at(&address) else {
            return false;
        };
        let running = format!("running a region region={region} epoch={epoch} ");
        self.cluster.workers[worker].1.is_some()
            && self.cluster.log(&worker_name(worker)).contains(&running)
    }

    /// Whether the coordinator knows exactly the regions `living`, and each of them is
    /// run by someone.
    fn whole(&self, living: &[Region]) -> bool {
        self.known() == living && living.iter().all(|region| self.runs(*region))
    }

    /// Whether the loads of the workers that are there differ by one at most: then
    /// the coordinator has evened out what it would, and begins no release of its own.
    fn even(&self) -> bool {
        let loads = self.loads();
        let alive = (0..loads.len()).filter(|worker| self.cluster.workers[*worker].1.is_some());
        let counts: Vec<usize> = alive.map(|worker| loads[worker].len()).collect();
        let ends = counts.iter().max().zip(counts.iter().min());
        ends.is_some_and(|(most, fewest)| most - fewest <= 1)
    }

    /// Waits until every region the world store's list has is run by someone, the
    /// coordinator knows of no other, and `more` holds as well. Returns the list.
    ///
    /// The list is read anew at every look. What a worker asked of the store before
    /// it was killed, or while nobody could hear what came of it, the store may do a
    /// moment after the test first looked.
    async fn runs_and(&mut self, what: &str, more: impl Fn(&Self) -> bool) -> RegionList {
        let waiting = Instant::now();
        loop {
            self.tend();
            // A store that was just started may not answer yet.
            let list = self.read_list().await;
            if let Ok(list) = &list
                && self.whole(&Self::living(list))
                && more(self)
            {
                return list.clone();
            }
            if waiting.elapsed() > PATIENCE {
                self.fail(&format!(
                    "waited {PATIENCE:?} in vain until {what}; the list is {list:?}"
                ));
            }
            tokio::time::sleep(LOOK).await;
        }
    }

    /// Waits until every region the world store's list has is run by someone, and
    /// nothing else is. Returns the list.
    async fn everything_runs(&mut self) -> RegionList {
        let what = "the coordinator knows the regions of the store's list and each is run";
        self.runs_and(what, |_| true).await
    }

    /// Waits until every region runs and the workers share them evenly, so that the
    /// coordinator moves nothing by itself from here on.
    async fn settled(&mut self) -> RegionList {
        let what = "every region runs and the workers share them evenly";
        let list = self.runs_and(what, Self::even).await;
        self.note(format!(
            "regions {:?} run, shared as {:?}",
            Self::living(&list),
            self.loads()
        ));
        list
    }

    /// Runs `command`, which asks the coordinator for `what`, without waiting for
    /// what comes of it.
    fn asking(&mut self, what: String, command: Command) -> JoinHandle<Asked> {
        self.note(format!("asked for {what}"));
        ask(what, command)
    }

    /// Waits for what a command comes to.
    async fn answer(&mut self, asking: JoinHandle<Asked>) -> Asked {
        self.until("the command has ended", |_| asking.is_finished())
            .await;
        let asked = asking.await.expect("asking does not panic");
        self.note(format!("{}: {}", asked.what, asked.outcome()));
        asked
    }

    /// `clustine merge`, without waiting for what comes of it.
    fn merging(&mut self, survivor: Region, absorbed: Region) -> JoinHandle<Asked> {
        let command = self.cluster.merge_command(survivor, absorbed);
        let what = format!("region {survivor} to absorb region {absorbed}");
        self.asking(what, command)
    }

    /// `clustine merge`, and what comes of it.
    async fn merge(&mut self, survivor: Region, absorbed: Region) -> Asked {
        let asking = self.merging(survivor, absorbed);
        self.answer(asking).await
    }

    /// `clustine split`, and what comes of it.
    async fn split(&mut self, region: Region, chunks: &[(i32, i32)]) -> Asked {
        let command = self.cluster.split_command(region, chunks);
        let what = format!("region {region} to be split at the chunks {chunks:?}");
        let asking = self.asking(what, command);
        self.answer(asking).await
    }

    /// `clustine move` to any worker, without waiting for what comes of it.
    fn moving(&mut self, region: Region) -> JoinHandle<Asked> {
        let command = self.cluster.move_command(region as usize, None);
        self.asking(format!("region {region} to be moved"), command)
    }

    /// `clustine move` to any worker, and what comes of it.
    async fn move_region(&mut self, region: Region) -> Asked {
        let asking = self.moving(region);
        self.answer(asking).await
    }

    /// How many times the coordinator has logged `message`.
    fn coordinator_said(&self, message: &str) -> usize {
        self.cluster.log("coordinator").matches(message).count()
    }

    /// How many times the edge has linked to `region`.
    fn links_to(&self, region: Region) -> usize {
        let linked = format!("linked to a region region={region} epoch=");
        self.cluster.log("edge").matches(&linked).count()
    }

    /// Sends `signal` to a worker.
    async fn signal(&mut self, worker: usize, signal: &str, what: &str) {
        let process = self.cluster.workers[worker].1.as_ref();
        let pid = process
            .and_then(|process| process.id())
            .expect("a running worker");
        let sent = Command::new("kill")
            .args([format!("-{signal}"), pid.to_string()])
            .status()
            .await;
        assert!(sent.unwrap().success());
        self.note(format!("{what} {}", worker_name(worker)));
    }

    /// Kills the coordinator without warning.
    async fn kill_coordinator(&mut self) {
        let coordinator = self.cluster.coordinator.1.take();
        let mut coordinator = coordinator.expect("the coordinator is running");
        coordinator.kill().await.unwrap();
        self.note("killed the coordinator".to_owned());
    }

    /// Starts a coordinator, which knows nothing of the one before.
    fn start_coordinator(&mut self) {
        self.coordinator_since = self.cluster.log("coordinator").len();
        self.cluster.start_coordinator();
        self.note("started the coordinator".to_owned());
    }

    /// Kills the world store without warning.
    async fn kill_store(&mut self) {
        let mut store = self.cluster.store.1.take().expect("the store is running");
        store.kill().await.unwrap();
        self.note("killed the world store".to_owned());
    }

    /// Starts the world store on the world as the one before left it.
    fn start_store(&mut self) {
        self.cluster.start_store();
        self.note("started the world store".to_owned());
    }

    /// The epoch with which `worker` last said it opens `part`, split off `region`.
    fn split_off_with(&self, worker: usize, region: Region, part: Region) -> Option<u64> {
        let log = self.cluster.log(&worker_name(worker));
        let opening = format!("opening the new region region={region} part={part} epoch=");
        let (_, rest) = log.rsplit_once(&opening)?;
        rest.split_whitespace().next()?.parse().ok()
    }

    /// Kills a worker without warning.
    async fn kill_worker(&mut self, worker: usize, why: &str) {
        let name = worker_name(worker);
        let mut process = self.cluster.workers[worker]
            .1
            .take()
            .unwrap_or_else(|| panic!("{name} is not running"));
        process.kill().await.unwrap();
        self.note(format!("killed {name}, {why}"));
    }

    /// Ends the test: stops every process the way Kubernetes does, which fails if one
    /// of them ends with an error, and removes the logs unless they are to be kept.
    async fn finish(mut self) {
        self.tend();
        self.cluster.terminate().await;
        if std::env::var_os("CLUSTINE_RESHAPES_KEEP").is_some() {
            let kept = self.keep();
            println!("{}: the logs are kept in {}", self.test, kept.display());
        }
    }
}

/// P1. `clustine merge` of the two stripes prints the survivor, and the world store's
/// list then has one region, pinned to both areas. `clustine merge` of the home
/// region into the other is refused and changes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn two_stripes_are_merged_when_somebody_asks_and_the_home_region_is_never_absorbed() {
    if a_repetition() {
        return;
    }
    // Region 0 is west of block x = 48 and has the chunk players enter in.
    let mut reshapes = Reshapes::start("merge", "3", 2).await;
    let before = reshapes.list().await;
    let routes = reshapes.routes();
    if (before.home.0, Reshapes::living(&before)) != (0, vec![0, 1]) {
        reshapes.fail(&format!(
            "the world did not begin as two stripes: {before:?}"
        ));
    }

    let refused = reshapes.merge(1, 0).await;
    if !refused.was_told_no_because("the home region, which is never absorbed") {
        let outcome = refused.outcome();
        reshapes.fail(&format!(
            "a merge of the home region into the other was not refused for what it is: {outcome}"
        ));
    }
    let after = reshapes.list().await;
    if after != before || reshapes.routes() != routes {
        reshapes.fail(&format!(
            "a merge that was refused changed something: the list was {before:?} and is {after:?}"
        ));
    }
    for worker in 0..2 {
        if reshapes
            .cluster
            .log(&worker_name(worker))
            .contains("asked to release")
        {
            let name = worker_name(worker);
            reshapes.fail(&format!(
                "{name} was asked to release its region by a refused merge"
            ));
        }
    }

    let links = reshapes.links_to(0);
    let survivor_epoch = reshapes.epoch(0);
    let merged = reshapes.merge(0, 1).await;
    if !merged.says("region 0 has absorbed region 1") {
        let outcome = merged.outcome();
        reshapes.fail(&format!("the merge did not print the survivor: {outcome}"));
    }
    let Some(own_time) = merged.own_time() else {
        reshapes.fail("the merge did not say how long it took");
    };
    reshapes.note(format!(
        "the merge took {} ms by its own account, and the command {} ms",
        own_time.as_millis(),
        merged.took.as_millis()
    ));

    // The list decides, and has it by the time the command has its answer.
    let list = reshapes.list().await;
    let mut both: Vec<_> = before
        .regions
        .iter()
        .flat_map(|info| info.pinned.clone())
        .collect();
    let mut pinned = list
        .regions
        .first()
        .map(|info| info.pinned.clone())
        .unwrap_or_default();
    let by_west_end = |area: &clustine_world::ChunkArea| area.min_x;
    both.sort_by_key(by_west_end);
    pinned.sort_by_key(by_west_end);
    let absorbed: Vec<(Region, Region)> = list
        .absorbed
        .iter()
        .map(|(gone, into)| (gone.0, into.0))
        .collect();
    if Reshapes::living(&list) != [0] || pinned != both || both.len() != 2 || absorbed != [(1, 0)] {
        reshapes.fail(&format!(
            "after the merge the list does not have one region pinned to both areas: {list:?}"
        ));
    }
    if list.next != before.next || list.home != before.home {
        reshapes.fail(&format!(
            "a merge changed the home region or the next id: {list:?}"
        ));
    }

    // The coordinator knows one region, which its worker went on running, and the
    // worker that released the other runs nothing.
    reshapes.everything_runs().await;
    if reshapes.epoch(0) != survivor_epoch {
        reshapes.fail("the survivor changed hands at the merge");
    }
    let Some(table) = reshapes
        .table()
        .filter(|table| table.contains("absorbed=1 "))
    else {
        reshapes.fail("the routing table does not name the region that was absorbed");
    };
    reshapes.note(format!("the routing table after the merge: {table}"));
    // The merge closed the survivor's links, and the edge is let in again at once.
    reshapes
        .until("the edge has linked to the survivor again", |reshapes| {
            reshapes.links_to(0) > links
        })
        .await;

    // What is no more cannot be merged again, in either direction.
    for (survivor, absorbed) in [(0, 1), (1, 0)] {
        let again = reshapes.merge(survivor, absorbed).await;
        if !again.was_told_no_because("the world has no region 1") {
            let outcome = again.outcome();
            reshapes.fail(&format!(
                "a merge with a region that is no more was not refused: {outcome}"
            ));
        }
    }
    reshapes.finish().await;
}

/// P2. `clustine split` with no player anywhere is told that nobody stands there, and
/// nothing changes: not the list, not who runs what, and not the links of the region,
/// which goes on as if nothing had been asked.
#[tokio::test(flavor = "multi_thread")]
async fn a_split_with_no_player_anywhere_is_told_that_nobody_stands_there() {
    if a_repetition() {
        return;
    }
    let mut reshapes = Reshapes::start("split", "3", 2).await;
    let before = reshapes.list().await;
    let routes = reshapes.routes();
    let links = (reshapes.links_to(0), reshapes.links_to(1));

    // Chunks west of the origin among them, of which the command line has to make
    // coordinates and not options.
    let asked_for: [(Region, &[(i32, i32)]); 3] = [
        (0, &[(1, 1), (-2, 5), (-7, -3)]),
        (1, &[(5, 0)]),
        // The chunk players enter in, in which nobody is split off even if they
        // stand there.
        (0, &[(0, 0)]),
    ];
    for (region, chunks) in asked_for {
        let split = reshapes.split(region, chunks).await;
        if !split.was_told_no_because("no player stands in a chunk named that the region holds") {
            let outcome = split.outcome();
            reshapes.fail(&format!(
                "a split with no player anywhere was not told that nobody stands there: {outcome}"
            ));
        }
        reshapes.note(format!(
            "a split that finds nobody took {} ms",
            split.took.as_millis()
        ));
        let after = reshapes.list().await;
        // A region's epoch and bounds are its own business; which regions there are
        // and which id comes next is what a split would change.
        let same = Reshapes::living(&after) == Reshapes::living(&before)
            && (after.next, &after.absorbed) == (before.next, &before.absorbed);
        if !same || reshapes.routes() != routes {
            reshapes.fail(&format!(
                "a split that found nobody changed something: the list was {before:?} and is {after:?}"
            ));
        }
    }

    // What the coordinator refuses before any worker hears of it.
    let unknown = reshapes.split(7, &[(1, 1)]).await;
    if !unknown.was_told_no_because("the world has no region 7") {
        let outcome = unknown.outcome();
        reshapes.fail(&format!(
            "a split of a region that is not there was not refused: {outcome}"
        ));
    }

    // The regions tick on with the links they had: a split that is off closes none.
    reshapes.everything_runs().await;
    if (reshapes.links_to(0), reshapes.links_to(1)) != links {
        reshapes.fail("a split that found nobody made the edge link to a region again");
    }
    // And they can still do what they are asked: the next thing is not turned away
    // as coming in the middle of the last.
    let merged = reshapes.merge(0, 1).await;
    if !merged.says("region 0 has absorbed region 1") {
        let outcome = merged.outcome();
        reshapes.fail(&format!(
            "a merge after splits that found nobody failed: {outcome}"
        ));
    }
    reshapes.finish().await;
}

/// P3. A merge that is asked for during a move is refused, and so is a move during a
/// merge; and `clustine move` of the survivor after a merge works: its next owner
/// restores the merged region.
#[tokio::test(flavor = "multi_thread")]
async fn a_move_and_a_merge_refuse_each_other_and_the_survivor_of_a_merge_is_moved() {
    if a_repetition() {
        return;
    }
    // Three stripes on two workers: region 1, between block x = 0 and x = 64, has
    // the chunk players enter in.
    let mut reshapes = Reshapes::start("move and merge", "0,4", 2).await;
    let before = reshapes.list().await;
    let home = before.home.0;
    if (home, Reshapes::living(&before)) != (1, vec![0, 1, 2]) {
        reshapes.fail(&format!(
            "the world did not begin as three stripes: {before:?}"
        ));
    }

    // A move that stays under way for as long as the test wants: of a region whose
    // owner is stopped and so cannot release it. It is one of the two regions of the
    // worker that has two, so that the move leaves nothing for the coordinator to
    // even out.
    let loads = reshapes.loads();
    let Some(busy) = loads.iter().position(|regions| regions.len() == 2) else {
        reshapes.fail(&format!(
            "no worker runs two of the three regions: {loads:?}"
        ));
    };
    let moved = *loads[busy]
        .iter()
        .find(|region| **region != home)
        .expect("one of two is not home");
    let begun = reshapes.coordinator_said("a move has begun");
    reshapes.signal(busy, "STOP", "stopped").await;
    let moving = reshapes.moving(moved);
    reshapes
        .until("the move has begun", |reshapes| {
            reshapes.coordinator_said("a move has begun") > begun
        })
        .await;
    for (survivor, absorbed) in [(home, moved), (moved, 3 - home - moved)] {
        let merge = reshapes.merge(survivor, absorbed).await;
        if !merge.was_told_no_because(&format!("region {moved} is being released")) {
            let outcome = merge.outcome();
            reshapes.fail(&format!(
                "a merge asked for during a move was not refused for it: {outcome}"
            ));
        }
    }
    reshapes.signal(busy, "CONT", "let go on").await;
    let moved_now = reshapes.answer(moving).await;
    if !moved_now.says("released by its owner") {
        let outcome = moved_now.outcome();
        reshapes.fail(&format!(
            "the move did not go through once its owner went on: {outcome}"
        ));
    }
    reshapes.settled().await;

    // A merge that stays under way likewise: the owner of the region to absorb is
    // stopped and does not release it.
    let absorbed = 3 - home - moved;
    let Some(releasing) = reshapes.owner(absorbed) else {
        reshapes.fail("the region to absorb has no owner");
    };
    let begun = reshapes.coordinator_said("a merge or a split has begun");
    reshapes.signal(releasing, "STOP", "stopped").await;
    let merging = reshapes.merging(home, absorbed);
    reshapes
        .until("the merge has begun", |reshapes| {
            reshapes.coordinator_said("a merge or a split has begun") > begun
        })
        .await;
    for region in [home, absorbed] {
        let refused = reshapes.move_region(region).await;
        let reason = format!("region {region} is part of a merge or a split that is under way");
        if !refused.was_told_no_because(&reason) {
            let outcome = refused.outcome();
            reshapes.fail(&format!(
                "a move asked for during a merge was not refused for it: {outcome}"
            ));
        }
    }
    // Nor is either of them merged with a third meanwhile, or split.
    let third = reshapes.merge(moved, absorbed).await;
    if !third.was_told_no_because("is part of a merge or a split that is under way") {
        let outcome = third.outcome();
        reshapes.fail(&format!(
            "a second merge of a region that is being merged was not refused: {outcome}"
        ));
    }
    reshapes.signal(releasing, "CONT", "let go on").await;
    let merged = reshapes.answer(merging).await;
    if !merged.says(&format!("region {home} has absorbed region {absorbed}")) {
        let outcome = merged.outcome();
        reshapes.fail(&format!(
            "the merge did not go through once the owner went on: {outcome}"
        ));
    }
    reshapes.note(format!(
        "a merge whose region to absorb was held up took {} ms",
        merged.took.as_millis()
    ));
    let list = reshapes.everything_runs().await;
    let pinned = list
        .regions
        .iter()
        .find(|info| info.region.0 == home)
        .map(|info| info.pinned.len());
    if Reshapes::living(&list) != [home.min(moved), home.max(moved)] || pinned != Some(2) {
        reshapes.fail(&format!(
            "after the merge the list is not two regions, one of two areas: {list:?}"
        ));
    }

    // The survivor is moved, right away: nothing is evened out for a lease after a
    // merge, so the move meets no release of the coordinator's own.
    let Some(from) = reshapes.owner(home) else {
        reshapes.fail("the survivor has no owner");
    };
    let epoch = reshapes.epoch(home);
    let moved_on = reshapes.move_region(home).await;
    if !moved_on.says("released by its owner") {
        let outcome = moved_on.outcome();
        reshapes.fail(&format!(
            "the survivor of a merge could not be moved: {outcome}"
        ));
    }
    reshapes
        .until("the survivor runs on another worker", |reshapes| {
            reshapes.runs(home)
                && reshapes.owner(home) != Some(from)
                && reshapes.epoch(home) > epoch
        })
        .await;
    reshapes.settled().await;
    let after = reshapes.list().await;
    let now = after
        .regions
        .iter()
        .find(|info| info.region.0 == home)
        .map(|info| info.pinned.len());
    if Reshapes::living(&after) != Reshapes::living(&list) || now != Some(2) {
        reshapes.fail(&format!(
            "the move of the survivor changed the regions: {after:?}"
        ));
    }
    reshapes.finish().await;
}

/// P4. A worker is killed while it has a region absorb another: here, when it has
/// just been told to. Whether the world store has the merge by then or not, every
/// region its list has is run by someone within two leases.
#[tokio::test(flavor = "multi_thread")]
async fn every_region_is_run_again_within_two_leases_of_a_worker_killed_while_it_absorbs() {
    if a_repetition() {
        return;
    }
    let told = "asked to have the region absorb another";
    a_worker_is_killed_while_it_absorbs("kill", told).await;
}

/// P4 again, with the worker killed a moment later: when its region has handed the
/// merge to the world store, which may well have it then.
#[tokio::test(flavor = "multi_thread")]
async fn every_region_is_run_again_within_two_leases_of_a_worker_killed_as_it_hands_in_a_merge() {
    if a_repetition() {
        return;
    }
    a_worker_is_killed_while_it_absorbs("late kill", "handing the store a merge or a split").await;
}

/// Has the home region of two stripes absorb the other, kills the home region's worker
/// when it has logged `moment`, and fails unless every region the world store's list
/// has is run by someone within two leases of that, with the list as before the
/// merge or as after it.
async fn a_worker_is_killed_while_it_absorbs(test: &str, moment: &str) {
    let mut reshapes = Reshapes::start(test, "3", 2).await;
    let Some(survivor) = reshapes.owner(0) else {
        reshapes.fail("the home region has no owner");
    };
    let name = worker_name(survivor);
    let merging = reshapes.merging(0, 1);
    reshapes
        .until("the survivor's worker is at the merge", |reshapes| {
            reshapes.cluster.log(&name).contains(moment)
        })
        .await;
    let why = format!("which had just logged `{moment}`");
    reshapes.kill_worker(survivor, &why).await;
    let killed = Instant::now();

    // Whoever asked is told what the coordinator finds out, and not left waiting.
    let asked = reshapes.answer(merging).await;
    let list = reshapes.everything_runs().await;
    let took = killed.elapsed();
    let absorbed: Vec<(Region, Region)> = list
        .absorbed
        .iter()
        .map(|(gone, into)| (gone.0, into.0))
        .collect();
    let found = match (Reshapes::living(&list).as_slice(), absorbed.as_slice()) {
        ([0, 1], []) => "the regions as they were before the merge",
        ([0], [(1, 0)]) => "the merge done",
        _ => reshapes.fail(&format!(
            "the list is neither as before the merge nor as after it: {list:?}"
        )),
    };
    reshapes.note(format!(
        "found {found}; every region ran again {:.3} s after the kill; the command was told: {}",
        took.as_secs_f64(),
        asked.outcome()
    ));
    if took > 2 * LEASE {
        reshapes.fail(&format!(
            "every region was run again only {:.3} s after the worker was killed; two leases are {:?}",
            took.as_secs_f64(),
            2 * LEASE
        ));
    }
    // The command agrees with the list where it claims that the merge was done.
    if asked.code == Some(0) && found != "the merge done" {
        reshapes.fail("the command said that the merge was done, and the list has both regions");
    }
    // The worker that is left runs everything.
    let other = 1 - survivor;
    if reshapes.loads()[other] != Reshapes::living(&list) {
        let loads = reshapes.loads();
        reshapes.fail(&format!(
            "the worker that is left does not run every region: {loads:?}"
        ));
    }
    reshapes.finish().await;
}

/// Beyond P1 to P4, with one bot, which stands still: a region with a player in it is
/// split, and the worker that split it runs the new region from then on, with the
/// epoch the coordinator named, also when the coordinator is replaced by one that
/// knows nothing of the split, and when the coordinator is killed while the worker is
/// at a split and never hears what came of it. The new region is absorbed again by
/// the region it was split off, on the same worker.
#[tokio::test(flavor = "multi_thread")]
async fn the_part_of_a_split_is_run_by_the_worker_that_made_it_whatever_the_coordinator_knows() {
    if a_repetition() {
        return;
    }
    let mut reshapes = Reshapes::start("part", "3", 2).await;
    // A player in a chunk of region 0 that is not the one players enter in, two
    // chunks west of it. The bot is kept connected and does nothing more.
    let mut bot = match Bot::join(&reshapes.cluster.edge.0, "Splinter").await {
        Ok(bot) => bot,
        Err(error) => reshapes.fail(&format!("the bot could not join: {error:#}")),
    };
    if let Err(error) = bot.walk_to(-24.5, 0.5, 0.5).await {
        reshapes.fail(&format!("the bot could not walk: {error:#}"));
    }
    let leave = Arc::new(AtomicBool::new(false));
    let standing = tokio::spawn({
        let leave = Arc::clone(&leave);
        async move {
            while !leave.load(Ordering::Relaxed) {
                bot.idle(Duration::from_millis(50)).await?;
            }
            anyhow::Ok(())
        }
    });
    let Some(owner) = reshapes.owner(0) else {
        reshapes.fail("the home region has no owner");
    };

    // The region hears of the bot's last steps a moment after the bot has made them,
    // so a split may find nobody there at first.
    let chunks = [(-2, 0)];
    let waiting = Instant::now();
    let split = loop {
        let split = reshapes.split(0, &chunks).await;
        if !split.was_told_no_because("no player stands in a chunk named") {
            break split;
        }
        if waiting.elapsed() > PATIENCE {
            reshapes.fail("no split ever found the bot in its chunk");
        }
    };
    if !split.says("region 2 has been split off region 0") {
        let outcome = split.outcome();
        reshapes.fail(&format!(
            "the split did not print the new region: {outcome}"
        ));
    }
    reshapes.note(format!(
        "the split took {} ms by its own account, and the command {} ms",
        split.own_time().unwrap_or_default().as_millis(),
        split.took.as_millis()
    ));
    let list = reshapes.everything_runs().await;
    if (Reshapes::living(&list), list.next.0) != (vec![0, 1, 2], 3) {
        reshapes.fail(&format!(
            "after the split the list does not have the new region: {list:?}"
        ));
    }
    // The worker that split the region runs the part, with the epoch it was told, and
    // edges are let in to it under that epoch.
    let with = reshapes.split_off_with(owner, 0, 2);
    if reshapes.owner(2) != Some(owner) || reshapes.epoch(2) != with || with.is_none() {
        let routes = reshapes.routes();
        reshapes.fail(&format!(
            "the new region is not run by the worker that made it, with the epoch {with:?}: {routes:?}"
        ));
    }
    reshapes
        .until("the edge has linked to the new region", |reshapes| {
            reshapes.links_to(2) > 0
        })
        .await;

    // A coordinator that never heard of the split is told of the new region by the
    // worker that runs it, and leaves it there.
    reshapes.kill_coordinator().await;
    reshapes.start_coordinator();
    reshapes.everything_runs().await;
    if (reshapes.owner(2), reshapes.epoch(2)) != (Some(owner), with) {
        let routes = reshapes.routes();
        reshapes.fail(&format!(
            "under a new coordinator the new region is not its worker's as it was: {routes:?}"
        ));
    }

    // The part goes back into the region it came from, which the same worker runs:
    // it releases the one and opens it again to have the other absorb it.
    let merged = reshapes.merge(0, 2).await;
    if !merged.says("region 0 has absorbed region 2") {
        let outcome = merged.outcome();
        reshapes.fail(&format!("the part could not be absorbed again: {outcome}"));
    }
    let list = reshapes.everything_runs().await;
    if (Reshapes::living(&list), list.next.0) != (vec![0, 1], 3) {
        reshapes.fail(&format!(
            "after the merge the list still has the part: {list:?}"
        ));
    }

    // Split again, and the coordinator dies while the worker is at it: nobody hears
    // from the coordinator what came of the split, and the next coordinator hears it
    // from the worker.
    let name = worker_name(owner);
    let asked = "asked to split the region region=0 ";
    let before = reshapes.cluster.log(&name).matches(asked).count();
    let command = reshapes.cluster.split_command(0, &chunks);
    let splitting = reshapes.asking("region 0 to be split again".to_owned(), command);
    reshapes
        .until("the worker has been told to split", |reshapes| {
            reshapes.cluster.log(&name).matches(asked).count() > before
        })
        .await;
    reshapes.kill_coordinator().await;
    let unanswered = reshapes.answer(splitting).await;
    reshapes
        .until("the worker has made the split", |reshapes| {
            reshapes.split_off_with(owner, 0, 3).is_some()
        })
        .await;
    reshapes.start_coordinator();
    let list = reshapes.everything_runs().await;
    let with = reshapes.split_off_with(owner, 0, 3);
    if Reshapes::living(&list) != [0, 1, 3] || with.is_none() {
        let outcome = unanswered.outcome();
        reshapes.fail(&format!(
            "the split that the coordinator did not live to see was not made: {list:?}; the \
             command: {outcome}"
        ));
    }
    if (reshapes.owner(3), reshapes.epoch(3)) != (Some(owner), with) {
        let routes = reshapes.routes();
        reshapes.fail(&format!(
            "the region that was split off while the coordinator died is not its worker's, \
             with the epoch {with:?}: {routes:?}"
        ));
    }
    if reshapes
        .cluster
        .log(&name)
        .contains("the coordinator has taken the region")
    {
        reshapes.fail("a region was taken from the worker that split one off");
    }

    leave.store(true, Ordering::Relaxed);
    match standing.await.expect("the bot does not panic") {
        Ok(()) => reshapes.note("the bot was connected to the end".to_owned()),
        // What a player is shown at a split is for the tests under bots to judge.
        Err(error) => reshapes.note(format!("the bot was disconnected on the way: {error:#}")),
    }
    reshapes.finish().await;
}

/// Beyond P1 to P4: the world store is killed while a worker has a region absorb
/// another, and started again. Whether the store has the merge or not, the workers
/// open their regions again, every region its list has is run, and whoever asked is
/// told something and not left waiting.
#[tokio::test(flavor = "multi_thread")]
async fn every_region_is_run_again_when_the_world_store_is_killed_in_the_middle_of_a_merge() {
    if a_repetition() {
        return;
    }
    let mut reshapes = Reshapes::start("store", "3", 2).await;
    let Some(survivor) = reshapes.owner(0) else {
        reshapes.fail("the home region has no owner");
    };
    let name = worker_name(survivor);
    let merging = reshapes.merging(0, 1);
    reshapes
        .until(
            "the survivor's worker has been told to absorb",
            |reshapes| {
                let log = reshapes.cluster.log(&name);
                log.contains("asked to have the region absorb another")
            },
        )
        .await;
    reshapes.kill_store().await;
    reshapes.start_store();

    let asked = reshapes.answer(merging).await;
    let list = reshapes.everything_runs().await;
    let absorbed: Vec<(Region, Region)> = list
        .absorbed
        .iter()
        .map(|(gone, into)| (gone.0, into.0))
        .collect();
    let found = match (Reshapes::living(&list).as_slice(), absorbed.as_slice()) {
        ([0, 1], []) => "the regions as they were before the merge",
        ([0], [(1, 0)]) => "the merge done",
        _ => reshapes.fail(&format!(
            "the list is neither as before the merge nor as after it: {list:?}"
        )),
    };
    reshapes.note(format!(
        "found {found}; the command was told: {}",
        asked.outcome()
    ));
    if asked.code == Some(0) && found != "the merge done" {
        reshapes.fail("the command said that the merge was done, and the list has both regions");
    }
    // What is left can be merged, or is merged already, and nothing is in the way of
    // asking: no merge is left half done in a worker.
    if found != "the merge done" {
        reshapes.settled().await;
        // The survivor's worker can still be at what the store's death left of the
        // first merge when the routing table has every region running again: it
        // then says that the region is in the middle of something, which is no
        // merge left half done, and it is asked again until it has got over it.
        let asked_again = Instant::now();
        let merged = loop {
            let merged = reshapes.merge(0, 1).await;
            let busy = merged.was_told_no_because("in the middle of a release, a merge or a split");
            if !busy || asked_again.elapsed() > PATIENCE {
                break merged;
            }
        };
        if !merged.says("region 0 has absorbed region 1") {
            let outcome = merged.outcome();
            reshapes.fail(&format!(
                "after the store was back the regions could not be merged: {outcome}"
            ));
        }
        reshapes.everything_runs().await;
    }
    reshapes.finish().await;
}

/// The coordinator's end of a worker's connection, in the test that plays the
/// coordinator.
type Heard = End<FromCoordinator, ToCoordinator>;

/// Where every player enters the world.
const SPAWN: Vec3 = Vec3::new(0.5, -60.0, 0.5);

/// What a coordinator tells a worker that is to run `regions`, each with its epoch.
fn orders(regions: &[(Region, u64)]) -> FromCoordinator {
    let assignments = regions.iter().map(|(region, epoch)| Assignment {
        region: RegionId(*region),
        epoch: *epoch,
        // A worker goes by what the world store says of them.
        entity_ids: EntityIds {
            first: EntityId(0),
            end: EntityId(0),
        },
    });
    FromCoordinator::Assigned {
        spawn: SPAWN,
        assignments: assignments.collect(),
    }
}

/// Fails the test that plays the coordinator, with what the processes have logged.
fn failed(cluster: &Cluster, message: &str) -> ! {
    panic!("{message}\n{}", cluster.all_logs());
}

/// What the worker says next besides that it is there. Fails if it says nothing in
/// time or has gone; `what` says what was waited for.
async fn said(cluster: &Cluster, worker: &mut Heard, what: &str) -> ToCoordinator {
    let hearing = async {
        loop {
            match worker.recv().await {
                Some(ToCoordinator::Heartbeat { .. }) => {}
                word => return word,
            }
        }
    };
    match timeout(PATIENCE, hearing).await {
        Ok(Some(word)) => word,
        Ok(None) => failed(cluster, &format!("the worker has gone before {what}")),
        Err(_) => failed(cluster, &format!("waited {PATIENCE:?} in vain for {what}")),
    }
}

/// Reads what the worker says until it is something else than where its players are,
/// and returns that.
async fn word(cluster: &Cluster, worker: &mut Heard, what: &str) -> ToCoordinator {
    loop {
        match said(cluster, worker, what).await {
            ToCoordinator::Players { .. } => {}
            word => return word,
        }
    }
}

/// Reads what the worker says of where its players are until it names exactly
/// `regions`, each with its epoch and at a tick after `after`, and returns the latest
/// of those ticks. Fails if the worker says anything else meanwhile.
async fn runs(cluster: &Cluster, worker: &mut Heard, regions: &[(Region, u64)], after: u64) -> u64 {
    let what = format!("the worker to run exactly {regions:?}");
    loop {
        match said(cluster, worker, &what).await {
            ToCoordinator::Players { regions: named } => {
                let run: Vec<(Region, u64)> =
                    named.iter().map(|of| (of.region.0, of.epoch)).collect();
                if run == regions && named.iter().all(|of| of.tick > after) {
                    return named.iter().map(|of| of.tick).max().unwrap_or(after);
                }
            }
            other => failed(cluster, &format!("waiting for {what}, heard {other:?}")),
        }
    }
}

/// Beyond P1 to P4: a worker that is given a region which the world store has as
/// absorbed is refused by the store, drops the region and goes on, and says that the
/// region it went into has absorbed it, which is the word that makes a coordinator
/// read the list.
///
/// No coordinator gives such a region out any more: it knows no region but those of
/// the store's list. So the test plays the coordinator, as `reports.rs` does, which
/// is also how it hears every word the worker says. A worker that runs both regions
/// of a world pinned at chunk 3 is told to release region 1 and to have region 0
/// absorb it, and is then given region 1 again with a higher epoch.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_that_is_given_a_region_that_was_absorbed_drops_it_and_says_so() {
    if a_repetition() {
        return;
    }
    let _turn = turn().await;
    let directory = tempfile::Builder::new()
        .prefix("clustine-reshapes-")
        .tempdir()
        .unwrap();
    // The address is one that nothing listened on a moment ago. Another test's
    // process can have taken it since, and then another address is tried.
    let (mut cluster, coordinator) = loop {
        let cluster = Cluster::new(directory.path(), 1, "3").await;
        match TcpListener::bind(&cluster.coordinator.0).await {
            Ok(coordinator) => break (cluster, coordinator),
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {}
            Err(error) => panic!("listening as the coordinator: {error}"),
        }
    };
    cluster.start_store();
    cluster.start_worker(0);

    // The worker registers, holding nothing, and is given both regions.
    let Ok(accepted) = timeout(PATIENCE, coordinator.accept()).await else {
        failed(&cluster, "the worker did not connect to the coordinator");
    };
    let (stream, _) = accepted.expect("accepting a connection");
    let mut worker: Heard = tcp::link(stream, 256);
    match said(&cluster, &mut worker, "the worker to register").await {
        ToCoordinator::RegisterWorker { holding, .. } if holding.is_empty() => {}
        other => failed(
            &cluster,
            &format!("expected a worker to register with nothing, and heard {other:?}"),
        ),
    }
    worker.send(orders(&[(0, 3), (1, 5)])).await.unwrap();
    runs(&cluster, &mut worker, &[(0, 3), (1, 5)], 0).await;

    // The merge, as a coordinator has it made: the one region released, and the
    // other told to absorb it under a new epoch.
    let release = FromCoordinator::Release {
        region: RegionId(1),
        epoch: 5,
    };
    worker.send(release).await.unwrap();
    let released = word(&cluster, &mut worker, "region 1 to be released").await;
    let as_asked = ToCoordinator::Released {
        region: RegionId(1),
        epoch: 5,
    };
    if released != as_asked {
        failed(
            &cluster,
            &format!("the worker was to release region 1, and said {released:?}"),
        );
    }
    worker.send(orders(&[(0, 3)])).await.unwrap();
    let absorb = FromCoordinator::Absorb {
        region: RegionId(0),
        epoch: 3,
        absorbed: RegionId(1),
        as_epoch: 6,
    };
    worker.send(absorb).await.unwrap();
    let absorbed = ToCoordinator::AbsorbEnded {
        region: RegionId(0),
        absorbed: RegionId(1),
        outcome: Ok(()),
    };
    let merged = word(&cluster, &mut worker, "what came of the merge").await;
    if merged != absorbed {
        failed(
            &cluster,
            &format!("region 0 was to absorb region 1, and the worker said {merged:?}"),
        );
    }
    let gone = |list: &RegionList| {
        let absorbed = list.absorbed.iter().map(|(gone, into)| (gone.0, into.0));
        Reshapes::living(list) == [0] && absorbed.collect::<Vec<_>>() == [(1, 0)]
    };
    let list = cluster.regions().await;
    if !list.as_ref().is_ok_and(gone) {
        failed(
            &cluster,
            &format!("after the merge the list does not have region 1 as absorbed: {list:?}"),
        );
    }
    let before = runs(&cluster, &mut worker, &[(0, 3)], 0).await;

    // The region that is no more is given out: with an epoch above every one it was
    // ever opened with, so that the store refuses it for being absorbed and for
    // nothing else.
    let dropping = "the world store has the region as absorbed by another; dropping it";
    if cluster.log(&worker_name(0)).contains(dropping) {
        failed(
            &cluster,
            "the worker dropped a region before it was given one that is no more",
        );
    }
    worker.send(orders(&[(0, 3), (1, 8)])).await.unwrap();
    let dropped = word(&cluster, &mut worker, "the worker's word of the region").await;
    if dropped != absorbed {
        failed(
            &cluster,
            &format!(
                "the worker was to say that region 0 has absorbed the region it was given, \
                 and said {dropped:?}"
            ),
        );
    }
    cluster.wait_for_log(&worker_name(0), dropping, 1).await;

    // It goes on with the region it has, which ticks, and runs nothing else, though
    // its orders still name the other. The list is as it was.
    runs(&cluster, &mut worker, &[(0, 3)], before).await;
    let list = cluster.regions().await;
    if !list.as_ref().is_ok_and(gone) {
        failed(
            &cluster,
            &format!("the list changed when the absorbed region was given out: {list:?}"),
        );
    }
    let ended = cluster.workers[0]
        .1
        .as_mut()
        .and_then(|worker| worker.try_wait().unwrap());
    if let Some(status) = ended {
        failed(&cluster, &format!("the worker ended by itself ({status})"));
    }

    // Without a coordinator a worker that is told to stop does not wait for anybody
    // to take its region.
    drop((worker, coordinator));
    cluster.terminate().await;
}
