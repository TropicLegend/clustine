//! What light costs the edge: the measurements of step L7 of
//! `docs/adr/0023-light-at-the-edge.md`, section 9, its table "What is measured,
//! where, and what fails", row by row.
//!
//! Each row starts a fresh single process, `clustine --seed`, its world under
//! `TMPDIR`, reshaping by hand so that the world is one home region whatever its
//! players do and no merge or split falls into a figure. The edge logs a line of
//! figures every two seconds while it turns (`services/edge/src/figures.rs`): totals
//! since the process started, and two longest times since the line before. A row
//! takes the first line logged after it began and the first logged after it ended,
//! and their difference is what the row cost; the longest times are the largest of
//! the lines in between.
//!
//! The rows:
//!
//! 1. One join, seven chunks in ten taken as changed.
//! 2. One join into terrain nobody has been in, every chunk taken as changed: the
//!    bot says that it is far away.
//! 3. A hundred in one place while forty chunks change four times a second, each a
//!    block of glowstone placed and broken in a chunk of its own: once within
//!    everyone's view, and once at the rim of the view of those in the middle, since
//!    the edge sends a change's light only to whoever lacks a chunk beside it.
//! 4. A hundred entering at once in one place, seven chunks in ten taken as changed.
//! 5. A hundred entering at once, apart: they are placed on a grid whose views do
//!    not overlap and leave, the server is started again on the same world with
//!    seven chunks in ten taken as changed, and they all enter together; on two
//!    light threads and on the default.
//! 6. A join beside a hundred who have their views.
//!
//! Terrain of a seed is made when it is first needed, on the same machine, and kept by
//! the store from then on. Every time below includes that, and the test says how long
//! making the chunks of one view takes on one thread here.
//!
//! An ordinary run, which is in the full check, has five bots and a view distance of
//! 2, and asserts only what must always hold: nobody is dropped, every bot is sent
//! its whole view, and a join into one place costs no more jobs than the chunks it
//! was sent and [`SLACK`]. It asserts no time: a build with debug assertions checks every entry
//! of the edge at every turn. The measurement is the same test with other numbers
//! and an optimised build, and judges each row by the record's limits as well:
//!
//! ```text
//! CLUSTINE_LIGHTS=100 CLUSTINE_LIGHTS_VIEW=8 \
//!     cargo test --release -p clustine --test lights -- --nocapture
//! ```
//!
//! `CLUSTINE_LIGHTS` is the number of bots of the crowds, `CLUSTINE_LIGHTS_VIEW` the
//! view distance (8 at most: a bot asks for no more), and `CLUSTINE_LIGHTS_SECONDS`
//! how long the forty chunks of row 3 change, 60 in a measurement and 3 otherwise.
//! `CLUSTINE_LIGHTS_ROWS=4,6` runs those rows only.

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clustine_botswarm::{Behaviour, Bot};
use clustine_data::{blocks, items};
use clustine_protocol::packets::play::face;
use clustine_world::{ChunkGenerator, ChunkPos};
use clustine_worldgen::TerrainGenerator;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use common::wandering::{a_repetition, number_from};
use common::{free_address, view_area};

/// The world's seed, the one of the hand runs.
const SEED: i64 = 12345;

/// How often the bots and the test look again at what they wait for.
const LOOK: Duration = Duration::from_millis(50);

/// Five ticks: a chunk whose block changes this often changes four times a second.
const EVERY_CHANGE: Duration = Duration::from_millis(250);

/// The phase every bot waits for to leave.
const END: u32 = u32::MAX;

/// How many jobs a join may cost beyond the chunks it was sent. A chunk is lit once
/// for those who wait for it, and again only when a neighbour comes after its job
/// was dispatched, which in a join comes of generation that is slower than the hold:
/// the edge sends the chunk with the light it could make and lights it anew when
/// the neighbour is there. Where the terrain is generated as the view is asked for,
/// a few of a view's chunks are that late; more than this many is a chunk lit twice
/// with no reason.
const SLACK: u64 = 8;

/// The record's time for a job, in milliseconds (L0), and how much more a join of
/// a view may cost on the threads before it fails (section 9).
const JOB_MILLISECONDS: f64 = 2.2;
const JOB_MARGIN: f64 = 1.5;

/// The record's guess at the longest turn of the fan-out that is not felt.
const LONGEST_TURN_MICROSECONDS: u64 = 5_000;

/// The record's guess at how long a bot entering among a hundred apart may wait for
/// its nine chunks after the last of them came: a second of hold, a second of jobs,
/// and as much again. The edge measures it as the longest time from a chunk's coming
/// to its first sending.
const LONGEST_UNSENT_MILLISECONDS: u64 = 3_000;

/// What a run is asked to do.
#[derive(Clone, Copy)]
struct Asked {
    bots: usize,
    view: i32,
    seconds: u64,
    /// Whether the record's limits are judged: only with the measurement's numbers.
    measuring: bool,
}

impl Asked {
    fn from_the_environment() -> Self {
        let bots = number_from("CLUSTINE_LIGHTS");
        let view = number_from("CLUSTINE_LIGHTS_VIEW");
        let measuring = bots.is_some() || view.is_some();
        let view = view.unwrap_or(2) as i32;
        assert!(
            (2..=8).contains(&view),
            "CLUSTINE_LIGHTS_VIEW is from 2 to 8: a bot asks for 8 at most"
        );
        let default_seconds = if measuring { 60 } else { 3 };
        Self {
            bots: bots.unwrap_or(5) as usize,
            view,
            seconds: number_from("CLUSTINE_LIGHTS_SECONDS").unwrap_or(default_seconds),
            measuring,
        }
    }

    /// How long anything a row waits for may take before the row fails. Only ever
    /// runs out when something hangs: a hundred views made from nothing take
    /// minutes at a view distance of 8.
    fn patience(&self) -> Duration {
        if self.measuring {
            Duration::from_secs(1_800)
        } else {
            Duration::from_secs(300)
        }
    }

    /// The chunks of one view.
    fn view_chunks(&self) -> u64 {
        view_area((0, 0), self.view).len() as u64
    }
}

/// One line of the edge's figures, by name.
#[derive(Debug, Clone, Default)]
struct Figures(Vec<(String, u64)>);

impl Figures {
    /// The figures of a log line, if it is the edge's line of figures.
    fn of_line(line: &str) -> Option<Self> {
        let (_, after) = line.split_once("light figures")?;
        let (_, said) = after.split_once("figures=")?;
        let words: Vec<&str> = said.split_whitespace().collect();
        let pairs = words.chunks_exact(2).map(|pair| {
            let value = pair[1].trim_matches('"').parse().ok()?;
            Some((pair[0].trim_matches('"').to_owned(), value))
        });
        pairs.collect::<Option<Vec<_>>>().map(Self)
    }

    fn get(&self, name: &str) -> u64 {
        let found = self.0.iter().find(|(named, _)| named == name);
        found.map_or(0, |(_, value)| *value)
    }
}

/// What the edge's figures say of a stretch of time: from the first line logged
/// after it began to the first logged after it ended.
struct Stretch {
    before: Figures,
    after: Figures,
    /// The lines after `before` up to and with `after`, for the longest times.
    lines: Vec<Figures>,
}

impl Stretch {
    /// What a total grew by.
    fn grew(&self, name: &str) -> u64 {
        self.after.get(name).saturating_sub(self.before.get(name))
    }

    /// The largest of a figure that is the longest since the line before.
    fn longest(&self, name: &str) -> u64 {
        let all = self.lines.iter().map(|line| line.get(name));
        all.max().unwrap_or(0)
    }

    /// Every figure, for whoever reads the output.
    fn said(&self) -> String {
        let mut said = Vec::new();
        for (name, _) in &self.after.0 {
            let value = if name.starts_with("longest") {
                self.longest(name)
            } else {
                self.grew(name)
            };
            said.push(format!("{name} {value}"));
        }
        said.join(" ")
    }
}

/// The single process as a process of its own, with its log in a file.
struct Server {
    child: tokio::process::Child,
    address: String,
    log: PathBuf,
}

impl Server {
    /// Starts `clustine` with the world in `world` and its log in `log`, chunks of
    /// `as_changed` tenths of the world taken as changed and `threads` light
    /// threads, or the default; waits until it accepts connections.
    async fn start(
        world: &Path,
        log: PathBuf,
        view: i32,
        as_changed: u8,
        threads: Option<usize>,
    ) -> Self {
        let address = free_address().await;
        let written = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .unwrap();
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_clustine"));
        command
            .args(["--bind", &address, "--world"])
            .arg(world)
            .args(["--seed", &SEED.to_string()])
            .args(["--view-distance", &view.to_string()])
            .args(["--reshape", "by-hand", "--max-players", "1000"])
            .env("NO_COLOR", "1")
            .env("CLUSTINE_LIGHT_AS_CHANGED", as_changed.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(written)
            .kill_on_drop(true);
        match threads {
            Some(threads) => command.env("CLUSTINE_LIGHT_THREADS", threads.to_string()),
            None => command.env_remove("CLUSTINE_LIGHT_THREADS"),
        };
        let child = command.spawn().unwrap();
        let server = Self {
            child,
            address,
            log,
        };
        let began = Instant::now();
        while clustine_botswarm::ping(&server.address).await.is_err() {
            assert!(
                began.elapsed() < Duration::from_secs(60),
                "the server did not start:\n{}",
                server.log_text()
            );
            tokio::time::sleep(LOOK).await;
        }
        server
    }

    fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// Every line of figures it has logged, in order.
    fn figures(&self) -> Vec<Figures> {
        self.log_text()
            .lines()
            .filter_map(Figures::of_line)
            .collect()
    }

    /// Waits for the first line of figures logged after now, and returns where it
    /// is among them and what it says.
    async fn next_figures(&self) -> (usize, Figures) {
        let seen = self.figures().len();
        let began = Instant::now();
        loop {
            if let Some(line) = self.figures().get(seen) {
                return (seen, line.clone());
            }
            // The edge turns at least for its patience with regions, so a line
            // comes every two seconds.
            assert!(
                began.elapsed() < Duration::from_secs(60),
                "the edge logged no figures:\n{}",
                self.log_text()
            );
            tokio::time::sleep(LOOK).await;
        }
    }

    /// The stretch from `before` (as [`Server::next_figures`] returned it) to the
    /// first line logged after now.
    async fn stretch_since(&self, before: (usize, Figures)) -> Stretch {
        let (until, after) = self.next_figures().await;
        let lines = self.figures()[before.0 + 1..=until].to_vec();
        Stretch {
            before: before.1,
            after,
            lines,
        }
    }

    /// Asks it to stop as an operator does, and waits until it has.
    async fn stop(mut self) {
        let pid = self.child.id().expect("the process runs").to_string();
        let sent = tokio::process::Command::new("kill")
            .args(["-TERM", &pid])
            .status()
            .await;
        assert!(sent.unwrap().success(), "kill -TERM {pid}");
        let stopped = tokio::time::timeout(Duration::from_secs(120), self.child.wait()).await;
        assert!(
            stopped.is_ok(),
            "the server did not stop:\n{}",
            self.log_text()
        );
    }
}

/// What a bot does, one step after another, after it has entered.
#[derive(Debug, Clone)]
enum Step {
    /// Stays until the test's phase is this one or later.
    Await(u32),
    /// Says that it is at this x and z, at the height it has.
    Jump(f64, f64),
    /// Goes to the middle of this chunk, beside its ground, with glowstone in hand.
    TakeUpGlowstone((i32, i32)),
    /// Places the glowstone and breaks it again, so many changes in all, one every
    /// five ticks, and waits until the last is acknowledged.
    Change(u32),
    /// Waits until the server has kept where it is, and leaves.
    KeepAndLeave,
}

/// What the test knows of a bot, as the bot last noted it.
#[derive(Debug, Clone, Default)]
struct Seen {
    /// Where it entered, once it has.
    entered_at: Option<(f64, f64, f64)>,
    /// The chunk its view is around now, and whether it holds all of that view.
    centre: (i32, i32),
    whole: bool,
    /// When the last of the nine chunks around it came, and the first and the last
    /// of its view, while it holds them.
    nine_at: Option<Instant>,
    first_at: Option<Instant>,
    view_at: Option<Instant>,
    /// The chunks of its view.
    view_chunks: usize,
    /// Light updates for chunks it held.
    light_updates: u32,
    /// The block it changes, and how many changes it has sent.
    ground: Option<(i32, i32, i32)>,
    changes: u32,
    /// How many of its steps it has done, and whether it has left as asked.
    steps_done: usize,
    left: bool,
    /// Why it ended, if it ended otherwise.
    ended: Option<String>,
}

/// A bot that runs on its own task, reading everything the server sends it, and
/// does its steps as the test's phases allow.
struct Driven {
    name: String,
    seen: Arc<Mutex<Seen>>,
    task: JoinHandle<()>,
}

impl Driven {
    fn start(
        address: &str,
        name: String,
        view: i32,
        steps: Vec<Step>,
        gate: &watch::Receiver<u32>,
    ) -> Self {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let task = tokio::spawn({
            let (address, name, seen, gate) =
                (address.to_owned(), name.clone(), seen.clone(), gate.clone());
            async move {
                if let Err(error) = act(&address, &name, view, steps, gate, &seen).await {
                    seen.lock().unwrap().ended = Some(format!("{error:#}"));
                }
            }
        });
        Self { name, seen, task }
    }

    fn seen(&self) -> Seen {
        self.seen.lock().unwrap().clone()
    }
}

/// The chunk of a place.
fn chunk_of(location: (f64, f64, f64)) -> (i32, i32) {
    (
        (location.0.floor() as i32) >> 4,
        (location.2.floor() as i32) >> 4,
    )
}

/// The first and the last time the chunks `wanted` came, if the bot holds them all.
fn came(bot: &Bot, wanted: &[(i32, i32)]) -> Option<(Instant, Instant)> {
    let mut times = wanted
        .iter()
        .map(|chunk| bot.chunks_came.get(chunk).copied());
    let first = times.next()??;
    times.try_fold((first, first), |(least, most), time| {
        let time = time?;
        Some((least.min(time), most.max(time)))
    })
}

/// Notes what the bot holds, for the test to read.
fn note(bot: &Bot, view: i32, seen: &Mutex<Seen>) {
    let centre = chunk_of(bot.location);
    let mut seen = seen.lock().unwrap();
    if seen.centre != centre || seen.view_chunks == 0 {
        seen.centre = centre;
        seen.whole = false;
        seen.nine_at = None;
        seen.first_at = None;
        seen.view_at = None;
    }
    // Once there, the times stay: a chunk sent again later is not what is measured.
    if seen.nine_at.is_none() {
        let nine: Vec<(i32, i32)> = (-1..=1)
            .flat_map(|x| (-1..=1).map(move |z| (centre.0 + x, centre.1 + z)))
            .collect();
        seen.nine_at = came(bot, &nine).map(|(_, last)| last);
    }
    let area = view_area(centre, view);
    seen.view_chunks = area.len();
    if !seen.whole
        && let Some((first, last)) = came(bot, &area)
    {
        seen.whole = true;
        seen.first_at = Some(first);
        seen.view_at = Some(last);
    }
    seen.light_updates = bot.stats.light_updates;
}

/// Reads what the server sends until `until` holds.
async fn pump(
    bot: &mut Bot,
    view: i32,
    seen: &Mutex<Seen>,
    mut until: impl FnMut(&Bot) -> bool,
) -> Result<()> {
    loop {
        note(bot, view, seen);
        if until(bot) {
            return Ok(());
        }
        bot.idle(LOOK).await?;
        if let Some(ending) = &bot.ended {
            bail!("the connection ended: {ending}");
        }
    }
}

/// The highest block that is not air in the column at `x` and `z`, as the bot was
/// told.
fn ground_at(bot: &Bot, x: i32, z: i32) -> Result<i32> {
    let dimension = bot.info.dimension_type()?;
    let air = Some(blocks::AIR.0 as i32);
    for y in (dimension.min_y..dimension.min_y + dimension.height as i32).rev() {
        if bot.block_at(x, y, z)? != air {
            return Ok(y);
        }
    }
    bail!("the column at {x}, {z} is all air")
}

async fn act(
    address: &str,
    name: &str,
    view: i32,
    steps: Vec<Step>,
    gate: watch::Receiver<u32>,
    seen: &Mutex<Seen>,
) -> Result<()> {
    let behaviour = Behaviour {
        view_distance: u8::try_from(view)?,
        ..Behaviour::default()
    };
    let mut bot = Bot::join_with(address, name, behaviour).await?;
    seen.lock().unwrap().entered_at = Some(bot.location);
    for step in steps {
        match step {
            Step::Await(phase) => pump(&mut bot, view, seen, |_| *gate.borrow() >= phase).await?,
            Step::Jump(x, z) => bot.step_to(x, z).await?,
            Step::TakeUpGlowstone(chunk) => {
                pump(&mut bot, view, seen, |bot| bot.chunks.contains_key(&chunk)).await?;
                let (x, z) = (chunk.0 * 16 + 10, chunk.1 * 16 + 8);
                let ground = ground_at(&bot, x, z)?;
                // Two blocks from the glowstone, within reach of it.
                let (feet_x, feet_z) = (f64::from(chunk.0 * 16) + 8.5, f64::from(z) + 0.5);
                bot.stand_at(feet_x, f64::from(ground + 1), feet_z).await?;
                bot.take_from_creative_inventory(1, items::GLOWSTONE)
                    .await?;
                bot.select_slot(1).await?;
                seen.lock().unwrap().ground = Some((x, ground, z));
            }
            Step::Change(count) => {
                let (x, y, z) = seen.lock().unwrap().ground.context("no ground taken up")?;
                let mut last = 0;
                for change in 0..count {
                    last = if change % 2 == 0 {
                        bot.use_item_on(x, y, z, face::TOP).await?
                    } else {
                        bot.dig(x, y + 1, z).await?
                    };
                    seen.lock().unwrap().changes = change + 1;
                    let next = Instant::now() + EVERY_CHANGE;
                    pump(&mut bot, view, seen, |_| Instant::now() >= next).await?;
                }
                pump(&mut bot, view, seen, |bot| {
                    bot.acknowledged_sequence >= last
                })
                .await?;
            }
            Step::KeepAndLeave => {
                bot.stand_until_kept(Duration::from_secs(300)).await?;
                bot.close().await?;
                let mut seen = seen.lock().unwrap();
                seen.steps_done += 1;
                seen.left = true;
                return Ok(());
            }
        }
        seen.lock().unwrap().steps_done += 1;
    }
    pump(&mut bot, view, seen, |_| *gate.borrow() == END).await?;
    bot.close().await?;
    Ok(())
}

/// A row: what it says and what failed in it.
struct Row {
    number: &'static str,
    failures: Vec<String>,
}

impl Row {
    fn new(number: &'static str, what: &str) -> Self {
        println!("L7 row {number}: {what}");
        Self {
            number,
            failures: Vec::new(),
        }
    }

    fn say(&self, what: impl AsRef<str>) {
        println!("L7 row {}: {}", self.number, what.as_ref());
    }

    /// A failure of what must always hold.
    fn fails(&mut self, why: impl Into<String>) {
        let why = why.into();
        self.say(format!("FAILS: {why}"));
        self.failures.push(why);
    }

    /// A limit of the record: judged only in a measurement.
    fn judge(&mut self, asked: &Asked, holds: bool, limit: impl Into<String>) {
        let limit = limit.into();
        if !asked.measuring {
            return;
        }
        if holds {
            self.say(format!("holds: {limit}"));
        } else {
            self.fails(format!("the record's limit: {limit}"));
        }
    }

    fn end(&self) {
        let verdict = if self.failures.is_empty() {
            "passes".to_owned()
        } else {
            format!("fails ({})", self.failures.len())
        };
        self.say(verdict);
    }
}

/// The bots of a row and the gate of its phases.
struct Bots {
    gate: watch::Sender<u32>,
    driven: Vec<Driven>,
    patience: Duration,
}

impl Bots {
    fn new(asked: &Asked) -> Self {
        Self {
            gate: watch::channel(0).0,
            driven: Vec::new(),
            patience: asked.patience(),
        }
    }

    fn start(&mut self, address: &str, name: String, view: i32, steps: Vec<Step>) {
        let gate = self.gate.subscribe();
        let driven = Driven::start(address, name, view, steps, &gate);
        self.driven.push(driven);
    }

    fn phase(&self, phase: u32) {
        self.gate.send_replace(phase);
    }

    /// Waits until `done` holds of every bot of `which`, and fails if a bot ended
    /// without being asked to or patience runs out.
    async fn until(
        &self,
        what: &str,
        which: impl Fn(&Driven) -> bool,
        done: impl Fn(&Seen) -> bool,
    ) -> Result<()> {
        let began = Instant::now();
        loop {
            let mut waiting = 0;
            for driven in self.driven.iter().filter(|driven| which(driven)) {
                let seen = driven.seen();
                if let Some(ended) = &seen.ended {
                    bail!("{} ended while {what}: {ended}", driven.name);
                }
                if !done(&seen) {
                    waiting += 1;
                }
            }
            if waiting == 0 {
                return Ok(());
            }
            if began.elapsed() > self.patience {
                bail!("{waiting} bots were still waiting for {what}");
            }
            tokio::time::sleep(LOOK).await;
        }
    }

    async fn all(&self, what: &str, done: impl Fn(&Seen) -> bool) -> Result<()> {
        self.until(what, |_| true, done).await
    }

    fn seen(&self) -> Vec<Seen> {
        self.driven.iter().map(Driven::seen).collect()
    }

    /// The bots that ended without being asked to.
    fn ended(&self) -> Vec<String> {
        let ended = self.driven.iter().filter_map(|driven| {
            let seen = driven.seen();
            seen.ended.map(|why| format!("{}: {why}", driven.name))
        });
        ended.collect()
    }

    /// Lets every bot leave and waits until their tasks are over.
    async fn finish(self) {
        self.gate.send_replace(END);
        for driven in self.driven {
            let _ = tokio::time::timeout(Duration::from_secs(60), driven.task).await;
        }
    }
}

/// The time from `from` to the latest of `times`, if every one is there.
fn latest_after(from: Instant, times: impl Iterator<Item = Option<Instant>>) -> Option<Duration> {
    let mut latest = from;
    for time in times {
        latest = latest.max(time?);
    }
    Some(latest - from)
}

fn seconds(time: Option<Duration>) -> String {
    time.map_or("-".to_owned(), |time| {
        format!("{:.3} s", time.as_secs_f64())
    })
}

/// A share of one processor: so many milliseconds in a stretch of `wall`.
fn share(milliseconds: u64, wall: Duration) -> String {
    let share = milliseconds as f64 / (wall.as_secs_f64() * 1_000.0).max(1.0);
    format!("{milliseconds} ms, {:.1} % of one processor", share * 100.0)
}

/// What every row checks of the edge and the bots: nobody dropped, nobody ended.
fn nobody_dropped(row: &mut Row, stretch: &Stretch, bots: &Bots) {
    let dropped = stretch.grew("dropped");
    if dropped > 0 {
        row.fails(format!(
            "the edge dropped {dropped} players for a full queue"
        ));
    }
    for ended in bots.ended() {
        row.fails(format!("a bot ended: {ended}"));
    }
}

/// What a join of one view checks: no more jobs than the chunks sent and [`SLACK`].
fn jobs_within_the_chunks_sent(row: &mut Row, stretch: &Stretch) {
    let (jobs, chunks) = (stretch.grew("jobs"), stretch.grew("chunks"));
    if jobs > chunks + SLACK {
        row.fails(format!(
            "{jobs} jobs for {chunks} chunks sent: more than {SLACK} beyond them"
        ));
    }
}

/// How long making the chunks of the view around `centre` takes here on one thread,
/// for whoever reads a time that includes generation.
fn generation_of_a_view(centre: (i32, i32), view: i32) -> String {
    let terrain = TerrainGenerator::new(SEED);
    let area = view_area(centre, view);
    let began = std::time::Instant::now();
    for (x, z) in &area {
        std::hint::black_box(terrain.generate(ChunkPos::new(*x, *z)));
    }
    let took = began.elapsed();
    format!(
        "making the {} chunks of the view around chunk {centre:?} takes {:.0} ms on one thread \
         here ({:.1} ms a chunk) without a server beside it",
        area.len(),
        took.as_secs_f64() * 1_000.0,
        took.as_secs_f64() * 1_000.0 / area.len() as f64
    )
}

/// The chunk players enter in.
fn spawn_chunk() -> (i32, i32) {
    let spawn = TerrainGenerator::new(SEED).spawn_point();
    chunk_of((spawn.x, spawn.y, spawn.z))
}

/// A fresh directory for a row's world and log, under `TMPDIR`.
fn directory() -> tempfile::TempDir {
    tempfile::Builder::new().prefix("lights").tempdir().unwrap()
}

/// Row 1. One join, seven chunks in ten taken as changed.
async fn one_join(asked: &Asked) -> Row {
    let mut row = Row::new("1", "one join, seven chunks in ten changed");
    let directory = directory();
    let world = directory.path().join("world");
    let server = Server::start(&world, directory.path().join("log"), asked.view, 7, None).await;
    let before = server.next_figures().await;
    let mut bots = Bots::new(asked);
    let began = Instant::now();
    bots.start(&server.address, "Lone".to_owned(), asked.view, Vec::new());
    if let Err(error) = bots.all("its view", |seen| seen.whole).await {
        row.fails(format!("{error:#}"));
    }
    let stretch = server.stretch_since(before).await;
    let seen = bots.seen().pop().unwrap_or_default();
    row.say(stretch.said());
    row.say(format!(
        "the nine around the bot in {}, its view of {} in {} (its last chunk); {} light \
         updates came to it",
        seconds(seen.nine_at.map(|at| at - began)),
        seen.view_chunks,
        seconds(seen.view_at.map(|at| at - began)),
        seen.light_updates
    ));
    if stretch.grew("updates") > 0 {
        row.say(format!(
            "light updates were sent: the longest a chunk waited at the edge to be sent was \
             {} ms, against a hold of 1 s; a chunk that came after its neighbours had been sent \
             (generation slower than the hold) is lit again and its neighbours updated",
            stretch.longest("longest_unsent_ms")
        ));
    }
    nobody_dropped(&mut row, &stretch, &bots);
    jobs_within_the_chunks_sent(&mut row, &stretch);
    let (jobs, chunks) = (stretch.grew("jobs"), stretch.grew("chunks"));
    row.judge(
        asked,
        jobs <= chunks,
        format!("{jobs} jobs for {chunks} chunks sent"),
    );
    let updates = stretch.grew("updates");
    row.judge(asked, updates == 0, format!("{updates} light updates sent"));
    let limit = JOB_MARGIN * JOB_MILLISECONDS * asked.view_chunks() as f64;
    let thread = stretch.grew("thread_ms");
    row.judge(
        asked,
        thread as f64 <= limit,
        format!("{thread} ms of thread time, against {limit:.0} ms"),
    );
    bots.finish().await;
    server.stop().await;
    row.end();
    row
}

/// Row 2. One join into terrain nobody has been in, every chunk taken as changed.
async fn a_join_into_new_terrain(asked: &Asked) -> Row {
    let mut row = Row::new("2", "one join where nothing has been generated");
    let directory = directory();
    let world = directory.path().join("world");
    let server = Server::start(&world, directory.path().join("log"), asked.view, 10, None).await;
    let mut bots = Bots::new(asked);
    // A thousand chunks east of where players enter.
    let far = (16_008.5, 8.5);
    bots.start(
        &server.address,
        "Far".to_owned(),
        asked.view,
        vec![Step::Await(1), Step::Jump(far.0, far.1)],
    );
    if let Err(error) = bots
        .all("its view where it entered", |seen| seen.whole)
        .await
    {
        row.fails(format!("{error:#}"));
    }
    let before = server.next_figures().await;
    let began = Instant::now();
    bots.phase(1);
    let there = chunk_of((far.0, 0.0, far.1));
    if let Err(error) = bots
        .all("its view far away", |seen| {
            seen.centre == there && seen.whole
        })
        .await
    {
        row.fails(format!("{error:#}"));
    }
    let stretch = server.stretch_since(before).await;
    let seen = bots.seen().pop().unwrap_or_default();
    row.say(stretch.said());
    let (jobs, updates) = (stretch.grew("jobs"), stretch.grew("updates"));
    let chunks = seen.view_chunks.max(1) as f64;
    let rate = match (seen.first_at, seen.view_at) {
        (Some(first), Some(last)) if last > first => chunks / (last - first).as_secs_f64(),
        _ => f64::INFINITY,
    };
    row.say(format!(
        "{:.2} jobs and {:.2} updates a chunk of the view; the view in {}; its chunks came to \
         the bot at {rate:.1} a second from the first to the last, which is the rate at which \
         they could leave the edge and no faster than they came to it; {} light updates came \
         to the bot; {}",
        jobs as f64 / chunks,
        updates as f64 / chunks,
        seconds(seen.view_at.map(|at| at - began)),
        seen.light_updates,
        generation_of_a_view(there, asked.view)
    ));
    if rate > 20.0 {
        row.say(
            "the chunks came faster than twenty a second: the record's case of slow arrival \
             was not exercised",
        );
    }
    if asked.measuring && (jobs as f64 / chunks > 2.0 || updates as f64 / chunks > 1.0) {
        row.say("above two jobs or one update a chunk: the record asks that the hold be looked at again");
    }
    nobody_dropped(&mut row, &stretch, &bots);
    bots.finish().await;
    server.stop().await;
    row.end();
    row
}

/// The chunks of those who change blocks in row 3, `count` of them in a view around
/// `centre`: at its rim, spread along it, or nearest first, none the centre itself
/// and all within the view of anyone in any of them or in the centre.
fn chunks_that_change(centre: (i32, i32), view: i32, count: usize, rim: bool) -> Vec<(i32, i32)> {
    if rim {
        let area = view_area(centre, view);
        let beside =
            |(x, z): (i32, i32)| (-1..=1).flat_map(move |a| (-1..=1).map(move |b| (x + a, z + b)));
        let mut rim: Vec<(i32, i32)> = area
            .iter()
            .copied()
            .filter(|&chunk| beside(chunk).any(|near| !area.contains(&near)))
            .collect();
        // Around the rim, so that taking every so many spreads them along it.
        let angle = |&(x, z): &(i32, i32)| f64::from(z - centre.1).atan2(f64::from(x - centre.0));
        rim.sort_by(|a, b| angle(a).total_cmp(&angle(b)));
        let count = count.min(rim.len()).max(1);
        return (0..count).map(|n| rim[n * rim.len() / count]).collect();
    }
    let reach = (view / 2).min(3);
    let mut around: Vec<(i32, i32)> = (-reach..=reach)
        .flat_map(|x| (-reach..=reach).map(move |z| (x, z)))
        .filter(|&offset| offset != (0, 0))
        .collect();
    around.sort_by_key(|&(x, z)| (x.abs().max(z.abs()), x, z));
    around.truncate(count);
    let chunks = around
        .into_iter()
        .map(|(x, z)| (centre.0 + x, centre.1 + z));
    chunks.collect()
}

/// Starts `count` bots named `prefix` and a number, all doing `steps`.
fn crowd(bots: &mut Bots, address: &str, prefix: &str, count: usize, view: i32, steps: &[Step]) {
    for number in 0..count {
        bots.start(address, format!("{prefix}{number}"), view, steps.to_vec());
    }
}

/// Row 3. A hundred in one place while forty chunks change four times a second:
/// within everyone's view, and again at the rim of the view of those who stand in
/// the middle. The edge sends a change's light only to those who lack a chunk beside
/// it, since whoever has them all lights it as the client always has; within
/// everyone's view nobody is sent an update, and at the rim most are.
async fn a_hundred_where_forty_chunks_change(asked: &Asked, rim: bool) -> Row {
    let what = if rim {
        "a hundred in one place while forty chunks at the rim of their view change four \
         times a second"
    } else {
        "a hundred in one place while forty chunks within everyone's view change four \
         times a second"
    };
    let mut row = Row::new(if rim { "3 (rim)" } else { "3" }, what);
    let directory = directory();
    let world = directory.path().join("world");
    let server = Server::start(&world, directory.path().join("log"), asked.view, 0, None).await;
    let placers = (asked.bots * 2).div_ceil(5).max(1);
    let chunks = chunks_that_change(spawn_chunk(), asked.view, placers, rim);
    let changes = (asked.seconds * 4) as u32;
    let mut bots = Bots::new(asked);
    for (number, chunk) in chunks.iter().enumerate() {
        let steps = [
            Step::Await(1),
            Step::TakeUpGlowstone(*chunk),
            Step::Await(2),
            Step::Change(changes),
        ];
        bots.start(
            &server.address,
            format!("Changer{number}"),
            asked.view,
            steps.to_vec(),
        );
    }
    let rest = asked.bots.saturating_sub(chunks.len());
    crowd(&mut bots, &server.address, "Stander", rest, asked.view, &[]);
    row.say(format!(
        "{} bots, {} of them changing a block each in a chunk of its own, {changes} changes each",
        chunks.len() + rest,
        chunks.len()
    ));
    let mut failed = bots.all("their views", |seen| seen.whole).await.err();
    if failed.is_none() {
        bots.phase(1);
        let changers = |driven: &Driven| driven.name.starts_with("Changer");
        failed = bots
            .until("glowstone in hand", changers, |seen: &Seen| {
                seen.steps_done >= 2
            })
            .await
            .err();
    }
    if failed.is_none() {
        // The changers' views are around their own chunks now.
        failed = bots.all("their views", |seen| seen.whole).await.err();
    }
    let before = server.next_figures().await;
    let began = Instant::now();
    if failed.is_none() {
        bots.phase(2);
        let changers = |driven: &Driven| driven.name.starts_with("Changer");
        failed = bots
            .until("their changes", changers, |seen: &Seen| {
                seen.steps_done >= 4
            })
            .await
            .err();
    }
    let wall = began.elapsed();
    if let Some(error) = failed {
        row.fails(format!("{error:#}"));
    }
    let stretch = server.stretch_since(before).await;
    row.say(stretch.said());
    let updates_seen: u64 = bots
        .seen()
        .iter()
        .map(|seen| u64::from(seen.light_updates))
        .sum();
    row.say(format!(
        "over {:.1} s of changes: compressing updates {}; {} updates sent, {} bytes of them \
         before compression; the bots counted {updates_seen} updates in all",
        wall.as_secs_f64(),
        share(stretch.grew("compress_updates_ms"), wall),
        stretch.grew("updates"),
        stretch.grew("update_bytes"),
    ));
    nobody_dropped(&mut row, &stretch, &bots);
    let longest = stretch.longest("longest_turn_us");
    row.judge(
        asked,
        longest <= LONGEST_TURN_MICROSECONDS,
        format!("the longest turn {longest} us"),
    );
    bots.finish().await;
    server.stop().await;
    row.end();
    row
}

/// Row 4. A hundred entering at once in one place, seven chunks in ten changed.
async fn a_hundred_entering_in_one_place(asked: &Asked) -> Row {
    let mut row = Row::new("4", "a hundred entering at once in one place");
    let directory = directory();
    let world = directory.path().join("world");
    let server = Server::start(&world, directory.path().join("log"), asked.view, 7, None).await;
    let before = server.next_figures().await;
    let mut bots = Bots::new(asked);
    let began = Instant::now();
    crowd(
        &mut bots,
        &server.address,
        "Crowd",
        asked.bots,
        asked.view,
        &[],
    );
    if let Err(error) = bots.all("their views", |seen| seen.whole).await {
        row.fails(format!("{error:#}"));
    }
    let wall = began.elapsed();
    let stretch = server.stretch_since(before).await;
    let seen = bots.seen();
    row.say(stretch.said());
    row.say(format!(
        "every bot had its nine after {} and its view after {}; compressing chunks {}; \
         {} packets made in {} ms; {}",
        seconds(latest_after(began, seen.iter().map(|seen| seen.nine_at))),
        seconds(latest_after(began, seen.iter().map(|seen| seen.view_at))),
        share(stretch.grew("compress_chunks_ms"), wall),
        stretch.grew("packets_made"),
        stretch.grew("packet_ms"),
        generation_of_a_view(spawn_chunk(), asked.view)
    ));
    nobody_dropped(&mut row, &stretch, &bots);
    let jobs = stretch.grew("jobs");
    let view = asked.view_chunks();
    // Jobs are a chunk's and not a player's: a hundred cost what one does.
    if jobs > view + SLACK {
        row.fails(format!(
            "{jobs} jobs for a view of {view} chunks: more than {SLACK} beyond them"
        ));
    }
    let longest = stretch.longest("longest_turn_us");
    row.judge(
        asked,
        longest <= LONGEST_TURN_MICROSECONDS,
        format!("the longest turn {longest} us"),
    );
    bots.finish().await;
    server.stop().await;
    row.end();
    row
}

/// Row 5. A hundred entering at once, apart, on `threads` light threads and on the
/// default.
async fn a_hundred_entering_apart(asked: &Asked) -> Row {
    let mut row = Row::new("5", "a hundred entering at once, apart");
    let directory = directory();
    let world = directory.path().join("world");
    // Views that do not overlap: the view across and the buffer on both sides.
    let apart = 2 * asked.view + 5;
    let side = (asked.bots as f64).sqrt().ceil() as i32;
    let spawn = spawn_chunk();
    let places: Vec<(f64, f64)> = (0..asked.bots as i32)
        .map(|number| {
            let (x, z) = (number % side - side / 2, number / side - side / 2);
            let chunk = (spawn.0 + x * apart, spawn.1 + z * apart);
            (f64::from(chunk.0 * 16) + 8.5, f64::from(chunk.1 * 16) + 8.5)
        })
        .collect();

    // Placed with the switch off, so that what they left behind is not measured.
    let server = Server::start(&world, directory.path().join("log-0"), asked.view, 0, None).await;
    let mut bots = Bots::new(asked);
    for (number, (x, z)) in places.iter().enumerate() {
        let steps = vec![Step::Jump(*x, *z), Step::KeepAndLeave];
        bots.start(&server.address, format!("Apart{number}"), asked.view, steps);
    }
    let placed = bots.all("their places", |seen| seen.left).await;
    bots.finish().await;
    server.stop().await;
    if let Err(error) = placed {
        row.fails(format!("{error:#}"));
        row.end();
        return row;
    }
    row.say(format!(
        "{} bots on a grid {apart} chunks apart; {}; all of them make {} times as many",
        places.len(),
        generation_of_a_view(spawn, asked.view),
        places.len()
    ));

    for (run, threads) in [Some(2), None].into_iter().enumerate() {
        let log = directory.path().join(format!("log-{}", run + 1));
        let server = Server::start(&world, log, asked.view, 7, threads).await;
        let said = threads.map_or("the default threads".to_owned(), |threads| {
            format!("{threads} threads")
        });
        let before = server.next_figures().await;
        let mut bots = Bots::new(asked);
        let began = Instant::now();
        for number in 0..places.len() {
            bots.start(
                &server.address,
                format!("Apart{number}"),
                asked.view,
                Vec::new(),
            );
        }
        if let Err(error) = bots.all("their views", |seen| seen.whole).await {
            row.fails(format!("on {said}: {error:#}"));
        }
        let stretch = server.stretch_since(before).await;
        let seen = bots.seen();
        for (number, (seen, place)) in seen.iter().zip(&places).enumerate() {
            let entered = seen.entered_at.map(|(x, _, z)| (x, z));
            if entered.is_some_and(|entered| entered != *place) {
                row.fails(format!(
                    "Apart{number} entered at {entered:?} and not where it left, {place:?}"
                ));
            }
        }
        row.say(format!("on {said}: {}", stretch.said()));
        let jobs = stretch.grew("jobs");
        row.say(format!(
            "on {said}: the last view was whole after {}, every bot had its nine after {}; \
             {jobs} jobs, {:.2} a chunk of the views",
            seconds(latest_after(began, seen.iter().map(|seen| seen.view_at))),
            seconds(latest_after(began, seen.iter().map(|seen| seen.nine_at))),
            jobs as f64 / (asked.view_chunks() * places.len() as u64).max(1) as f64
        ));
        nobody_dropped(&mut row, &stretch, &bots);
        // The jobs are written down and not bounded: a hundred views apart are made
        // by generation, one chunk after another and slower than the hold, and a
        // chunk whose neighbour comes after its job is lit again, as the record's
        // line for terrain that is being generated says.
        let unsent = stretch.longest("longest_unsent_ms");
        row.judge(
            asked,
            unsent <= LONGEST_UNSENT_MILLISECONDS,
            format!("on {said}, the longest a chunk waited at the edge to be sent: {unsent} ms"),
        );
        bots.finish().await;
        server.stop().await;
    }
    row.end();
    row
}

/// Row 6. A join beside a hundred who have their views.
async fn a_join_beside_a_hundred(asked: &Asked) -> Row {
    let mut row = Row::new("6", "a join beside a hundred");
    let directory = directory();
    let world = directory.path().join("world");
    let server = Server::start(&world, directory.path().join("log"), asked.view, 0, None).await;
    let mut bots = Bots::new(asked);
    crowd(
        &mut bots,
        &server.address,
        "Crowd",
        asked.bots,
        asked.view,
        &[],
    );
    let mut failed = bots.all("their views", |seen| seen.whole).await.err();
    let before = server.next_figures().await;
    let began = Instant::now();
    if failed.is_none() {
        bots.start(&server.address, "Late".to_owned(), asked.view, Vec::new());
        failed = bots
            .all("the late one's view", |seen| seen.whole)
            .await
            .err();
    }
    if let Some(error) = failed {
        row.fails(format!("{error:#}"));
    }
    let stretch = server.stretch_since(before).await;
    let late = bots.seen().pop().unwrap_or_default();
    row.say(stretch.said());
    row.say(format!(
        "the late one had its view after {}; {} packets made in {} ms",
        seconds(late.view_at.map(|at| at - began)),
        stretch.grew("packets_made"),
        stretch.grew("packet_ms"),
    ));
    nobody_dropped(&mut row, &stretch, &bots);
    jobs_within_the_chunks_sent(&mut row, &stretch);
    let longest = stretch.longest("longest_turn_us");
    row.judge(
        asked,
        longest <= LONGEST_TURN_MICROSECONDS,
        format!("the longest turn {longest} us"),
    );
    bots.finish().await;
    server.stop().await;
    row.end();
    row
}

/// Step L7's six rows, one after another, each on a server of its own. In an
/// ordinary run: nobody is dropped, every bot has its whole view, and a join into
/// one place costs no more jobs than the chunks sent and a few. In a measurement,
/// the record's limits as well.
#[tokio::test(flavor = "multi_thread")]
async fn what_light_costs_the_edge_stays_within_what_the_record_allows() {
    if a_repetition() {
        return;
    }
    let asked = Asked::from_the_environment();
    println!(
        "L7: {} bots, a view distance of {} ({} chunks), {} s of changes, seed {SEED}, {}",
        asked.bots,
        asked.view,
        asked.view_chunks(),
        asked.seconds,
        if asked.measuring {
            "judged by the record's limits"
        } else {
            "an ordinary run"
        }
    );
    println!("L7: {}", generation_of_a_view(spawn_chunk(), asked.view));
    let began = Instant::now();
    // A measurement may be of some rows only: row 5 alone takes ten minutes.
    let only: Option<Vec<u32>> = std::env::var("CLUSTINE_LIGHTS_ROWS").ok().map(|rows| {
        let row = |row: &str| row.trim().parse().expect("CLUSTINE_LIGHTS_ROWS is numbers");
        rows.split(',').map(row).collect()
    });
    let wanted = |row: u32| only.as_ref().is_none_or(|only| only.contains(&row));
    let mut rows = Vec::new();
    if wanted(1) {
        rows.push(one_join(&asked).await);
    }
    if wanted(2) {
        rows.push(a_join_into_new_terrain(&asked).await);
    }
    if wanted(3) {
        rows.push(a_hundred_where_forty_chunks_change(&asked, false).await);
        rows.push(a_hundred_where_forty_chunks_change(&asked, true).await);
    }
    if wanted(4) {
        rows.push(a_hundred_entering_in_one_place(&asked).await);
    }
    if wanted(5) {
        rows.push(a_hundred_entering_apart(&asked).await);
    }
    if wanted(6) {
        rows.push(a_join_beside_a_hundred(&asked).await);
    }
    println!("L7: the rows took {:.0} s", began.elapsed().as_secs_f64());
    let failures: Vec<String> = rows
        .iter()
        .flat_map(|row| {
            let number = row.number;
            row.failures
                .iter()
                .map(move |failure| format!("row {number}: {failure}"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
