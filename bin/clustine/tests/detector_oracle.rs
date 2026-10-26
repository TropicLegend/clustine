//! A comparison with the official server of the daylight detector
//! (`docs/groundwork/n8-plan.md`, sections 6.4 and 9, comparison 2): a detector and an
//! inverted one in the open, and a detector under a roof whose sky light is 11, their
//! power compared exactly at the days of their updates through a day in steps, and day
//! by day where the power steps in the stretches 11,000 to 13,700 (dusk) and 22,330 to
//! 24,000 (dawn). It checks the sky's darkening, the sun's angle and its ease, and the
//! rounding of `round(i · cos f)` on a half.
//!
//! **How each server is brought to a day.** A detector reads the day of the overworld's
//! clock at a tick whose game time is a multiple of 20. The official server is asked with
//! commands: its clock is paused (`time pause`), so the game time runs on while the day
//! stands still, and for each day wanted it is set (`time set`) and sprinted 60 ticks
//! (`tick sprint`), which holds at least two updates at that day; the bot reads the three
//! detectors once it has been sent a time packet 25 ticks after the set, which the
//! server sends after the changes of the update's tick. Clustine has no commands. Its
//! clock is the world's, kept in the world store's table file, which a region takes
//! the later of with its own when it is restored. So each of Clustine's runs stops the
//! server, writes into the table a game time that is a multiple of 20 and a day some
//! ticks before the first day wanted, both later than anything the world has had,
//! starts it again and reads the updates as they come: each update's day is its game
//! time plus the day less the game time that the time packet of the join says, and
//! those come at days of one remainder by 20, the run's. A run is planned for each
//! remainder and stretch of days wanted, and several worlds (copies of one built world)
//! run at the same time. Which update a change of a detector belongs to is read from
//! the order of the packets: the game sends a block entity's change with the next
//! tick's changes, after the time packet of its own tick, so a change is the update of
//! the last time packet before it, unless it came half a second or more after it, when
//! it is the next one's (and the printout says so).
//!
//! **What is compared.** First the official server alone: its power on the grid of the
//! days of remainder 7 by 20 through both stretches, and between two of them where a
//! detector's power changed, the exact day it steps at, by halving (each detector's
//! power is monotonic through a stretch). Then both servers at the days wanted: every
//! thousandth day from 7 and the four updates after it; the grid of each stretch; and
//! the day before and the day of every step the official server takes. A difference is
//! listed with its day, its place in the day, the game time Clustine read it at, and the
//! three powers on each side.
//!
//! **If it fails**, look first at the days listed: a difference at the day of a step and
//! not the day before is a step a day late or early, which is the ease or the rounding
//! (`clustine-sim/src/time.rs`, `region/daylight_detector.rs`); a difference along a
//! whole stretch of the inverted detector is the darkening; one only under the roof is
//! the light at the detector (`region/light.rs`). "Clustine's clock did not take" means
//! the table's clock was not the one the region went on with.
//!
//! It needs Java, the server jar downloaded by `cargo datagen`, and your agreement to
//! the Minecraft EULA, so it is ignored by default. Run it on a quiet machine, alone:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test detector_oracle -- --ignored --nocapture --test-threads 1
//! ```

mod common;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use clustine::Config;
use clustine_botswarm::{Bot, Oracle};
use clustine_data::{BlockState, items};
use clustine_format::TableFile;
use clustine_protocol::packets::play::{ClientboundPlay, face};
use tokio::sync::Mutex;
use tokio::time::Instant;

use common::{config, start_with};

const PATIENCE: Duration = Duration::from_secs(30);

/// The grass of both servers' flat worlds.
const GROUND: i32 = -61;

/// The detector in the open, on the grass.
const OPEN: (i32, i32, i32) = (20, GROUND + 1, -36);
/// The inverted one beside it.
const INVERTED: (i32, i32, i32) = (22, GROUND + 1, -36);
/// The one under the roof: in a cell dug out of the dirt below the grass, at the end of
/// a corridor of three cells to a shaft open to the sky, four steps from where the sky's
/// light is 15, so 11.
const ROOFED: (i32, i32, i32) = (26, GROUND - 1, -36);

/// Where the bot hovers while it reads, above the detectors.
const HOVER: (f64, f64, f64) = (24.5, -56.0, -33.5);

/// The remainder by 20 of the days on the grids: not 0, so that the day and the game
/// time are out of step on both servers.
const PHASE: u64 = 7;

/// The stretches where the power steps, day time by day time.
const DUSK: (u64, u64) = (11_000, 13_700);
const DAWN: (u64, u64) = (22_330, 24_000);

/// The ticks Clustine's clock is set before a run's first update: time for the server to
/// start and the bot to join. A run whose bot came too late is run again with more.
const LEAD: u64 = 160;
const LONG_LEAD: u64 = 600;

/// The worlds Clustine runs at the same time.
const WORLDS: usize = 4;

/// Two days wanted of one remainder are read in one run of Clustine's if they are no
/// further apart than this: 20 seconds of running against a restart.
const MERGE_GAP: u64 = 400;

/// The power of the three detectors at one update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Powers {
    open: u8,
    inverted: u8,
    roofed: u8,
}

impl Powers {
    fn of(self, which: usize) -> u8 {
        [self.open, self.inverted, self.roofed][which]
    }
}

const NAMES: [&str; 3] = ["in the open", "inverted", "under the roof"];

/// The power of the detector state `id`, or why it is not one.
fn power_of(id: Option<i32>) -> Result<u8> {
    let state = id
        .and_then(|id| u16::try_from(id).ok())
        .map(BlockState)
        .context("the bot does not have the block")?;
    let block = state.block().context("no state of 26.3")?;
    ensure!(
        block.name == "minecraft:daylight_detector",
        "{} where a detector should be",
        block.name
    );
    let power = state
        .properties()
        .find(|(name, _)| *name == "power")
        .context("a detector without power")?
        .1;
    Ok(power.parse()?)
}

fn inverted_of(id: Option<i32>) -> bool {
    id.and_then(|id| u16::try_from(id).ok())
        .map(BlockState)
        .is_some_and(|state| state.properties().any(|p| p == ("inverted", "true")))
}

fn read_powers(bot: &Bot) -> Result<Powers> {
    let at = |(x, y, z): (i32, i32, i32)| power_of(bot.block_at(x, y, z).ok().flatten());
    Ok(Powers {
        open: at(OPEN)?,
        inverted: at(INVERTED)?,
        roofed: at(ROOFED)?,
    })
}

/// The id of the overworld's clock in the time packets the bot is sent.
fn overworld_clock(bot: &Bot) -> Result<i32> {
    let clocks = bot.info.registry("minecraft:world_clock")?;
    let index = clocks
        .iter()
        .position(|name| name == "minecraft:overworld")
        .context("no overworld clock")?;
    Ok(i32::try_from(index)?)
}

/// The day of the overworld's clock and the game time of the latest time packet that
/// said it.
fn clock_said(bot: &Bot, overworld: i32) -> Option<(i64, i64)> {
    bot.times.iter().rev().find_map(|time| {
        time.clocks
            .iter()
            .find(|clock| clock.clock == overworld)
            .map(|clock| (time.game_time, clock.total_ticks))
    })
}

fn latest_game_time(bot: &Bot) -> Option<i64> {
    bot.times.last().map(|time| time.game_time)
}

/// Joins `address` as `name`, flies to the hover and waits for the detectors' chunks.
async fn join(address: &str, name: &str) -> Result<Bot> {
    let mut bot = Bot::join(address, name).await?;
    bot.wait_until(PATIENCE, Bot::is_loaded).await?;
    bot.set_flying(true).await?;
    let from = bot.location;
    bot.fly_to((from.0, HOVER.1, from.2), (0.0, 90.0), 1.0)
        .await?;
    bot.fly_to(HOVER, (0.0, 90.0), 1.0).await?;
    bot.wait_until(PATIENCE, |bot| {
        [OPEN, INVERTED, ROOFED]
            .iter()
            .all(|&(x, y, z)| matches!(bot.block_at(x, y, z), Ok(Some(_))))
    })
    .await
    .context("the detectors' chunks")?;
    Ok(bot)
}

/// One click and the wait for its acknowledgement, from just above the block's column,
/// within reach of what lies below the grass.
async fn act(bot: &mut Bot, held: Option<i32>, act: Act) -> Result<()> {
    let (Act::Dig((x, _, z)) | Act::Top((x, _, z))) = act;
    let near = (
        f64::from(x) + 0.5,
        f64::from(GROUND) + 1.7,
        f64::from(z) + 0.5,
    );
    bot.fly_to(near, (0.0, 90.0), 1.0).await?;
    match held {
        Some(item) => {
            bot.take_from_creative_inventory(0, item).await?;
            bot.select_slot(0).await?;
        }
        None => {
            bot.empty_hotbar_slot(8).await?;
            bot.select_slot(8).await?;
        }
    }
    let sequence = match act {
        Act::Dig((x, y, z)) => bot.dig(x, y, z).await?,
        Act::Top((x, y, z)) => bot.use_item_on(x, y, z, face::TOP).await?,
    };
    bot.wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= sequence)
        .await
        .with_context(|| format!("{act:?} was not acknowledged"))
}

#[derive(Debug, Clone, Copy)]
enum Act {
    Dig((i32, i32, i32)),
    /// A click on the top of a block.
    Top((i32, i32, i32)),
}

/// Builds the three detectors, the inverted one clicked once, as a player would.
async fn build(bot: &mut Bot) -> Result<()> {
    let below = |(x, y, z): (i32, i32, i32)| (x, y - 1, z);
    act(bot, Some(items::DAYLIGHT_DETECTOR), Act::Top(below(OPEN))).await?;
    act(
        bot,
        Some(items::DAYLIGHT_DETECTOR),
        Act::Top(below(INVERTED)),
    )
    .await?;
    act(bot, None, Act::Top(INVERTED)).await?;
    let (x, y, z) = ROOFED;
    // The cell, the corridor and the shaft, which goes up through the grass.
    for dx in 0..=4 {
        act(bot, None, Act::Dig((x + dx, y, z))).await?;
    }
    act(bot, None, Act::Dig((x + 4, y + 1, z))).await?;
    act(bot, Some(items::DAYLIGHT_DETECTOR), Act::Top(below(ROOFED))).await?;
    bot.wait_until(PATIENCE, |bot| {
        read_powers(bot).is_ok()
            && inverted_of(
                bot.block_at(INVERTED.0, INVERTED.1, INVERTED.2)
                    .ok()
                    .flatten(),
            )
            && !inverted_of(bot.block_at(OPEN.0, OPEN.1, OPEN.2).ok().flatten())
    })
    .await
    .context("the detectors as built")
}

/// The official server, its clock paused, and a bot reading the detectors.
struct Official {
    oracle: Oracle,
    bot: Bot,
    overworld: i32,
    read: BTreeMap<u64, Powers>,
}

impl Official {
    async fn start() -> Result<Self> {
        let oracle = Oracle::start(false).await?;
        // Rain darkens the sky, which Clustine has none of.
        oracle
            .command("gamerule advance_weather false", &["now set to", "already"])
            .await?;
        oracle
            .command("weather clear", &["Set the weather"])
            .await?;
        let mut bot = join(oracle.address(), "Reader").await?;
        build(&mut bot).await?;
        oracle
            .command("time pause", &["Paused clock", "already paused"])
            .await?;
        let overworld = overworld_clock(&bot)?;
        Ok(Self {
            oracle,
            bot,
            overworld,
            read: BTreeMap::new(),
        })
    }

    /// The three powers at an update at `day`.
    async fn at(&mut self, day: u64) -> Result<Powers> {
        if let Some(read) = self.read.get(&day) {
            return Ok(*read);
        }
        let answer = self
            .oracle
            .command(
                &format!("time set {day}"),
                &["tick(s)", "Set the time", "already set"],
            )
            .await?;
        let overworld = self.overworld;
        let day_said =
            |bot: &Bot| clock_said(bot, overworld).filter(|(_, said)| *said == day as i64);
        if !answer.contains("already") {
            self.bot
                .wait_until(PATIENCE, |bot| day_said(bot).is_some())
                .await
                .with_context(|| format!("the official server's time packet of day {day}"))?;
        }
        let set = day_said(&self.bot)
            .map(|(game_time, _)| game_time)
            .or_else(|| latest_game_time(&self.bot))
            .context("no time packet")?;
        self.oracle
            .command("tick sprint 60", &["Sprint completed"])
            .await?;
        self.bot
            .wait_until(PATIENCE, |bot| {
                latest_game_time(bot).is_some_and(|time| time >= set + 25)
            })
            .await
            .context("the time packet after an update")?;
        let read = read_powers(&self.bot)?;
        self.read.insert(day, read);
        Ok(read)
    }

    /// Every day in `(from, to]` where some detector's power is not what it was the day
    /// before, by halving, given that each is monotonic between the two.
    async fn steps_between(&mut self, from: u64, to: u64) -> Result<Vec<u64>> {
        let mut steps = Vec::new();
        let mut low = from;
        loop {
            let (at_low, at_high) = (self.at(low).await?, self.at(to).await?);
            if at_low == at_high {
                return Ok(steps);
            }
            // The first day after `low` whose powers differ from `low`'s.
            let (mut same, mut differs) = (low, to);
            while differs - same > 1 {
                let middle = same + (differs - same) / 2;
                if self.at(middle).await? == at_low {
                    same = middle;
                } else {
                    differs = middle;
                }
            }
            steps.push(differs);
            low = differs;
        }
    }
}

/// The days on the grid of a stretch: of remainder [`PHASE`] by 20, from the last one
/// before it to the first one after.
fn grid((from, to): (u64, u64)) -> Vec<u64> {
    let first = from - (from + 20 - PHASE) % 20;
    (0..)
        .map(|step| first + 20 * step)
        .take_while(|day| *day < to + 20)
        .collect()
}

/// Every thousandth day from [`PHASE`], with the four updates after each.
fn through_the_day() -> Vec<u64> {
    (0..24)
        .flat_map(|thousand| (0..5).map(move |update| 1_000 * thousand + PHASE + 20 * update))
        .collect()
}

/// One of Clustine's runs: the updates at `first`, `first + 20`, … up to `last`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Run {
    first: u64,
    last: u64,
}

impl Run {
    fn days(self) -> impl Iterator<Item = u64> {
        (self.first..=self.last).step_by(20)
    }

    fn length(self) -> u64 {
        self.last - self.first
    }
}

/// The runs that read every day of `wanted`: by remainder, each run as long as the gaps
/// between its days are no more than [`MERGE_GAP`], the longest first.
fn plan(wanted: &BTreeSet<u64>) -> Vec<Run> {
    let mut runs = Vec::new();
    for remainder in 0..20 {
        let mut days = wanted.iter().copied().filter(|day| day % 20 == remainder);
        let Some(first) = days.next() else {
            continue;
        };
        let mut run = Run { first, last: first };
        for day in days {
            if day - run.last > MERGE_GAP {
                runs.push(run);
                run = Run {
                    first: day,
                    last: day,
                };
            } else {
                run.last = day;
            }
        }
        runs.push(run);
    }
    runs.sort_by_key(|run| std::cmp::Reverse(run.length()));
    runs
}

/// What Clustine read in a run: the powers at each of its days, and what to say of how.
struct Ran {
    read: BTreeMap<u64, (Powers, i64)>,
    notes: Vec<String>,
}

/// A world of Clustine's, kept on disk, whose clock only moves forward.
struct World {
    directory: PathBuf,
    /// The least game time and day the next run may set: above anything the world had.
    next_game_time: u64,
    next_day: u64,
}

impl World {
    fn on_disk(&self) -> Config {
        Config {
            world: Some(self.directory.clone()),
            ..config()
        }
    }

    /// Runs `run` with the clock set `lead` ticks before its first update.
    async fn run(&mut self, run: Run, lead: u64) -> Result<Ran> {
        // A game time that is a multiple of 20 and a day of the run's first update's
        // remainder: the updates, at game times that are multiples of 20, are then at the
        // run's days.
        let game_time = self.next_game_time.next_multiple_of(20);
        let days_before = (self.next_day + lead).div_ceil(24_000) + 1;
        let day = days_before * 24_000 + run.first - lead;
        set_clock(&self.directory, game_time, day)?;
        let first_update = game_time + lead;
        let last_update = first_update + run.length();
        self.next_game_time = last_update + 20 * 1_000;
        self.next_day = day + lead + run.length() + 24_000;

        let (server, address) = start_with(self.on_disk()).await;
        let ran = read_run(&address, run, game_time, day, first_update, last_update).await;
        server.stop().await;
        ran
    }
}

/// Reads the updates `first_update` to `last_update` of a Clustine started at
/// `game_time` and `day`, as a bot is sent them.
async fn read_run(
    address: &str,
    run: Run,
    game_time: u64,
    day: u64,
    first_update: u64,
    last_update: u64,
) -> Result<Ran> {
    let mut bot = join(address, "Reader").await?;
    let overworld = overworld_clock(&bot)?;
    bot.wait_until(PATIENCE, |bot| clock_said(bot, overworld).is_some())
        .await
        .context("the join's clock")?;
    let (joined_time, joined_day) = clock_said(&bot, overworld).context("the join's clock")?;
    let offset = joined_day - joined_time;
    ensure!(
        offset == day as i64 - game_time as i64 && joined_time >= game_time as i64,
        "Clustine's clock did not take: set to game time {game_time} and day {day}, the \
         join said {joined_time} and {joined_day}"
    );
    bot.record_packets();
    let snapshot = read_powers(&bot)?;
    let end = last_update as i64 + 40;
    bot.wait_until(Duration::from_secs(run.length() / 20 + 60), |bot| {
        latest_game_time(bot).is_some_and(|time| time >= end)
    })
    .await
    .context("the run's last update")?;

    let mut notes = Vec::new();
    let mut changes: Vec<(i64, usize, u8)> = Vec::new();
    let mut last_time: Option<(i64, Instant)> = None;
    let mut first_time = None;
    let places = [OPEN, INVERTED, ROOFED];
    for (when, packet) in bot.packets.as_deref().unwrap_or_default() {
        let changed: Vec<((i32, i32, i32), i32)> = match packet {
            ClientboundPlay::SetTime(time) => {
                last_time = Some((time.game_time, *when));
                first_time.get_or_insert(time.game_time);
                continue;
            }
            ClientboundPlay::BlockUpdate(update) => vec![(
                (update.position.x, update.position.y, update.position.z),
                update.state,
            )],
            ClientboundPlay::SectionBlocksUpdate(update) => {
                let (sx, sy, sz) = update.section;
                update
                    .blocks
                    .iter()
                    .map(|&((x, y, z), state)| {
                        (
                            (
                                sx * 16 + i32::from(x),
                                sy * 16 + i32::from(y),
                                sz * 16 + i32::from(z),
                            ),
                            state,
                        )
                    })
                    .collect()
            }
            _ => continue,
        };
        for (at, state) in changed {
            let Some(which) = places.iter().position(|place| *place == at) else {
                continue;
            };
            let Some((time, came)) = last_time else {
                notes.push(format!(
                    "a change of the detector {} before any time packet",
                    NAMES[which]
                ));
                continue;
            };
            let late = when.duration_since(came) >= Duration::from_millis(500);
            let update = if late { time + 20 } else { time };
            if late {
                notes.push(format!(
                    "the detector {}'s change of the update at game time {update} came \
                     {:?} after the time packet of {time}, before its own",
                    NAMES[which],
                    when.duration_since(came)
                ));
            }
            changes.push((update, which, power_of(Some(state))?));
        }
    }
    let first_time = first_time.context("no time packet in the run")?;
    if (first_update as i64) < first_time + 20 {
        bail!(
            "the bot was reading from game time {first_time} on, too late for the update \
             at {first_update}"
        );
    }
    let mut read = BTreeMap::new();
    for (wanted, update) in run.days().zip((first_update..=last_update).step_by(20)) {
        let update = update as i64;
        let mut powers = [snapshot.open, snapshot.inverted, snapshot.roofed];
        for &(at, which, power) in &changes {
            if at <= update {
                powers[which] = power;
            }
        }
        let powers = Powers {
            open: powers[0],
            inverted: powers[1],
            roofed: powers[2],
        };
        ensure!(
            (update + offset) % 24_000 == (wanted % 24_000) as i64,
            "the update at game time {update} is at day {}, not at {wanted}",
            update + offset
        );
        read.insert(wanted, (powers, update));
    }
    Ok(Ran { read, notes })
}

/// Sets the clock of the stopped world in `directory` to `game_time` and `day`.
fn set_clock(directory: &Path, game_time: u64, day: u64) -> Result<()> {
    let path = directory.join("regions").join("table");
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut table = TableFile::decode(&bytes).context("the table file")?;
    ensure!(
        table.clock[0] < game_time && table.clock[1] < day,
        "the world's clock {:?} is already past game time {game_time} and day {day}",
        table.clock
    );
    table.clock = [game_time, day];
    std::fs::write(&path, table.encode())?;
    Ok(())
}

fn copy_directory(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Builds the detectors in a world of Clustine's, then reads every day of `wanted` in
/// runs on [`WORLDS`] copies of it at once.
async fn clustine(wanted: &BTreeSet<u64>) -> Result<(BTreeMap<u64, (Powers, i64)>, Vec<String>)> {
    let directory = tempfile::tempdir()?;
    let built = directory.path().join("built");
    {
        let on_disk = Config {
            world: Some(built.clone()),
            ..config()
        };
        let (server, address) = start_with(on_disk).await;
        let built = async {
            let mut bot = join(&address, "Reader").await?;
            build(&mut bot).await
        }
        .await;
        server.stop().await;
        built.context("building the detectors on Clustine")?;
    }
    let queue = Arc::new(Mutex::new(VecDeque::from(plan(wanted))));
    let mut workers = Vec::new();
    for number in 0..WORLDS {
        let copy = directory.path().join(format!("world{number}"));
        copy_directory(&built, &copy)?;
        let queue = Arc::clone(&queue);
        workers.push(tokio::spawn(async move {
            let mut world = World {
                directory: copy,
                next_game_time: 1_000_000,
                next_day: 1_000_000,
            };
            let mut read = BTreeMap::new();
            let mut notes = Vec::new();
            loop {
                let Some(run) = queue.lock().await.pop_front() else {
                    break;
                };
                let ran = match world.run(run, LEAD).await {
                    Ok(ran) => ran,
                    Err(error) => {
                        notes.push(format!("{run:?} again with a longer lead: {error:#}"));
                        world.run(run, LONG_LEAD).await?
                    }
                };
                read.extend(ran.read);
                notes.extend(ran.notes);
            }
            anyhow::Ok((read, notes))
        }));
    }
    let mut read = BTreeMap::new();
    let mut notes = Vec::new();
    for worker in workers {
        let (more, said) = worker.await??;
        read.extend(more);
        notes.extend(said);
    }
    drop(directory);
    Ok((read, notes))
}

/// Where in the day `day` is, for a difference's line.
fn where_in_the_day(day: u64) -> &'static str {
    let time = day % 24_000;
    if (DUSK.0..=DUSK.1).contains(&time) {
        "at dusk"
    } else if time >= DAWN.0 || time < 133 {
        "at dawn"
    } else if time < 12_000 {
        "by day"
    } else {
        "by night"
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn daylight_detectors_give_the_power_of_the_official_server_through_a_day() {
    let mut official = Official::start().await.unwrap();
    let mut wanted: BTreeSet<u64> = through_the_day().into_iter().collect();
    let mut steps = BTreeSet::new();
    for stretch in [DUSK, DAWN] {
        let days = grid(stretch);
        wanted.extend(days.iter().copied());
        for pair in days.windows(2) {
            let found = official.steps_between(pair[0], pair[1]).await.unwrap();
            steps.extend(found);
        }
    }
    for &step in &steps {
        wanted.extend([step - 1, step]);
    }
    for &day in &wanted {
        official.at(day).await.unwrap();
    }
    let theirs = official.read.clone();
    drop(official);
    println!(
        "the official server steps at {} days of the stretches: {steps:?}",
        steps.len()
    );

    let (ours, notes) = clustine(&wanted).await.unwrap();
    for note in &notes {
        println!("note: {note}");
    }
    let mut differences = Vec::new();
    for &day in &wanted {
        let their = theirs[&day];
        let Some(&(our, game_time)) = ours.get(&day) else {
            differences.push(format!("day {day}: not read on Clustine"));
            continue;
        };
        let step = if steps.contains(&day) {
            " (a step of the official server's)"
        } else if steps.contains(&(day + 1)) {
            " (the day before a step)"
        } else {
            ""
        };
        let line = format!(
            "day {day} {}{step}, game time {game_time} on Clustine: official {their:?}, \
             Clustine {our:?}",
            where_in_the_day(day)
        );
        if their != our {
            let which: Vec<&str> = (0..3)
                .filter(|which| their.of(*which) != our.of(*which))
                .map(|which| NAMES[which])
                .collect();
            differences.push(format!("{line}; differs {which:?}"));
            println!("≠ {line}");
        } else {
            println!("  {line}");
        }
    }
    assert!(
        differences.is_empty(),
        "the detectors' power differs at {} of {} days:\n  {}",
        differences.len(),
        wanted.len(),
        differences.join("\n  ")
    );
}

#[test]
fn the_grids_hold_their_stretches_at_the_phase() {
    for stretch in [DUSK, DAWN] {
        let days = grid(stretch);
        assert!(days[0] <= stretch.0 && days[0] + 20 > stretch.0, "{days:?}");
        let last = *days.last().unwrap();
        assert!(last >= stretch.1 && last < stretch.1 + 20, "{days:?}");
        assert!(days.iter().all(|day| day % 20 == PHASE));
        assert!(days.windows(2).all(|pair| pair[1] - pair[0] == 20));
    }
}

#[test]
fn the_runs_read_every_day_wanted_once_and_at_its_remainder() {
    let mut wanted: BTreeSet<u64> = through_the_day().into_iter().collect();
    wanted.extend(grid(DUSK));
    wanted.extend([11_890, 11_891, 13_001, 22_500, 22_501, 23_999]);
    let runs = plan(&wanted);
    let mut read = BTreeSet::new();
    for run in &runs {
        assert_eq!(run.first % 20, run.last % 20, "{run:?}");
        for day in run.days() {
            read.insert(day);
        }
    }
    assert!(wanted.is_subset(&read), "{:?}", wanted.difference(&read));
    // Read days that were not wanted are only those inside a run's gaps.
    for run in &runs {
        let days: Vec<u64> = run.days().filter(|day| wanted.contains(day)).collect();
        assert_eq!(days.first(), Some(&run.first));
        assert_eq!(days.last(), Some(&run.last));
        assert!(days.windows(2).all(|pair| pair[1] - pair[0] <= MERGE_GAP));
    }
    assert!(
        runs.windows(2)
            .all(|pair| pair[0].length() >= pair[1].length())
    );
}

#[test]
fn the_detectors_keep_clear_of_each_other_and_of_the_spawn() {
    let places = [OPEN, INVERTED, ROOFED];
    for (index, place) in places.iter().enumerate() {
        assert!(
            place.0 > 12 || place.2 < -12,
            "{place:?} is where players join"
        );
        for other in &places[index + 1..] {
            assert_ne!(place, other);
        }
    }
    // The shaft is four steps from the roofed detector and farther from the others.
    let shaft = (ROOFED.0 + 4, ROOFED.2);
    for place in [OPEN, INVERTED] {
        assert!((place.0 - shaft.0).abs() + (place.2 - shaft.1).abs() > 4);
    }
}
