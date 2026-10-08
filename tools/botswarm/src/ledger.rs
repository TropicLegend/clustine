//! The ledger scenario: players who keep playing, and keep book of what they were told.
//!
//! It is meant for a server whose parts are being killed underneath it. Several bots
//! walk back and forth along lanes of their own, across whatever boundaries between
//! regions lie on the way, and all the while place and break blocks beside their lane,
//! on their side of a boundary and beyond it, switch the slot they hold and take items
//! into their hotbar. They do not wait for one action to be handled before they walk on
//! or do the next, as a player does not.
//!
//! Every action on a block goes into the bot's **ledger** with its sequence number, and
//! once the server has acknowledged it, with what the server then said the block is. The
//! scenario fails unless all of this holds:
//!
//! - nobody is disconnected, and nobody is moved by the server after joining;
//! - every action is acknowledged within the patience, and has the effect it has on a
//!   server that nobody disturbs: a placed block is the block of the item the bot
//!   believes to hold, a broken one is air;
//! - a block a bot was told about stays as it was told, for that bot, for every other
//!   bot that has it in view and for an auditor who joins when everyone is done: the
//!   ledgers' last word about a block is what the world holds;
//! - whenever the server says which slot or which items a bot holds, it is what the bot
//!   believes;
//! - no bot ever sees two entities for one player, and the entity of another bot does
//!   not vanish unless it was far enough away to have walked out of view;
//! - at the end every bot is seen by the others, and by the auditor, where it stands,
//!   as the entity it was when it joined.
//!
//! The plots the bots build on are disjoint, so what a block should be is always one
//! bot's word.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use clustine_data::{ITEMS, blocks, entity_types, items};
use clustine_protocol::item::ItemStack;
use clustine_protocol::packets::play::face;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::Bot;

/// The height of the surface of a flat world, which is the height players stand at and
/// the height the bots build at.
const GROUND: i32 = -60;

/// The length of a client tick.
const TICK: Duration = Duration::from_millis(50);

/// How far apart the lanes are, in blocks. A bot builds on the two rows east-to-west
/// beside its lane, which leaves a row nobody uses between one bot's plot and the next
/// bot's lane.
const LANE_SPACING: i32 = 4;

/// How far along its lane from where it stands a bot builds, in blocks.
const REACH: i32 = 2;

/// Blocks per tick on the way to the lane, which is not what the scenario is about.
const TO_THE_LANE: f64 = 0.8;

/// How many of a bot's actions may wait for the server at once. A player who sees
/// nothing happen stops clicking, too.
const MOST_PENDING: usize = 8;

/// How close to where it is going a bot counts as there, in blocks. A step shorter than
/// this is not taken.
const ARRIVED: f64 = 1e-6;

const AIR: i32 = blocks::AIR.0 as i32;

/// The items the bots take into their hotbars. Each is a block.
const PALETTE: [i32; 7] = [
    items::STONE,
    items::COBBLESTONE,
    items::DIRT,
    items::OAK_PLANKS,
    items::BRICKS,
    items::GLASS,
    items::SANDSTONE,
];

/// What the bots of the ledger scenario do.
#[derive(Debug, Clone)]
pub struct Ledger {
    /// How many players play.
    pub bots: usize,
    /// How many times each of them walks to `east` and back to `west` before it stops,
    /// if that is to end the scenario.
    pub rounds: Option<u32>,
    /// How long the bots play before they stop, if that is to end the scenario. With
    /// neither this nor `rounds` they play until [`Progress::finish`] is called.
    pub duration: Option<Duration>,
    /// The x coordinates the bots walk between.
    pub west: f64,
    pub east: f64,
    /// The x coordinate of the first block east of each boundary between regions that
    /// lies between `west` and `east`. The scenario only uses them to say when a bot
    /// steps across one and which actions reached across one; the bots build wherever
    /// they are, which near a boundary is on both sides of it.
    pub lines: Vec<i32>,
    /// Blocks per tick of the slowest bot; each further one is a little faster.
    pub speed: f64,
    /// What the bots' choices follow from.
    pub seed: u64,
    /// How long a bot waits for the server to acknowledge an action, and to show what it
    /// acknowledged, before the scenario fails. An edge gives up on a player after 20
    /// seconds without their region, so anything above that only ever catches a hang.
    pub patience: Duration,
    /// What the names of the bots begin with.
    pub name_prefix: String,
}

impl Default for Ledger {
    fn default() -> Self {
        Self {
            bots: 4,
            rounds: Some(3),
            duration: None,
            west: 40.5,
            east: 90.5,
            lines: Vec::new(),
            speed: 0.3,
            seed: 1,
            patience: Duration::from_secs(30),
            name_prefix: String::new(),
        }
    }
}

/// What a run of the ledger scenario observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerReport {
    /// How many actions on blocks the bots took, all of which were acknowledged.
    pub actions: u32,
    pub placed: u32,
    pub broken: u32,
    /// How many of the actions were on a block beyond a boundary from where the bot
    /// stood.
    pub across_a_line: u32,
    /// How many times a bot stepped across a boundary.
    pub crossings: u32,
    /// How many times a bot selected another slot or took an item into its hotbar.
    pub hotbar_changes: u32,
    /// How many acknowledgements arrived a client tick or more before the block was
    /// shown as the action left it. A client shows the old block in between.
    pub shown_late: u32,
    /// The longest a bot waited for an acknowledgement, and whose action that was.
    pub longest_wait: Duration,
    pub longest_wait_of: String,
    /// How many blocks the ledgers have a word about, each of which the auditor saw.
    pub blocks_audited: u32,
    /// The ledgers' last word about every block, by x, y and z: what the world holds
    /// when the scenario is over, for whoever wants to look again later, with
    /// [`audit_blocks`].
    pub blocks: BTreeMap<(i32, i32, i32), i32>,
}

impl std::fmt::Display for LedgerReport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} actions acknowledged ({} placed, {} broken, {} across a boundary), \
             {} steps across a boundary, {} changes of what is held, \
             {} shown after their acknowledgement, {} blocks audited; \
             longest wait for an acknowledgement {:.3} s ({})",
            self.actions,
            self.placed,
            self.broken,
            self.across_a_line,
            self.crossings,
            self.hotbar_changes,
            self.shown_late,
            self.blocks_audited,
            self.longest_wait.as_secs_f64(),
            self.longest_wait_of,
        )
    }
}

/// A small generator of numbers that follow from a seed (SplitMix64), so that a run can
/// be repeated.
#[derive(Debug, Clone)]
pub struct Random(u64);

impl Random {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn number(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut mixed = self.0;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        mixed ^ (mixed >> 31)
    }

    /// A number from 0 up to, but not including, `bound`, which must not be 0.
    pub fn below(&mut self, bound: u64) -> u64 {
        self.number() % bound
    }

    /// True once in `times` on average.
    pub fn one_in(&mut self, times: u64) -> bool {
        self.below(times) == 0
    }
}

/// A bot stepping across a boundary, said just before the step is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCrossing {
    /// Which bot, counted from 0.
    pub bot: usize,
    /// The first block east of the boundary.
    pub line: i32,
    pub eastwards: bool,
    /// How many steps across a boundary there have been, this one included.
    pub count: u64,
}

/// One bot as [`Progress`] last heard of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BotProgress {
    /// Whether the bot is on its lane and playing.
    pub playing: bool,
    pub x: f64,
    /// The sequence number of its latest action, and up to which the server has
    /// acknowledged them.
    pub sent: i32,
    pub acknowledged: i32,
}

#[derive(Default)]
struct BotCounters {
    playing: AtomicBool,
    x: AtomicU64,
    sent: AtomicI32,
    acknowledged: AtomicI32,
}

/// Where a running ledger scenario is, for whoever disturbs the server meanwhile, and
/// the means to end it.
pub struct Progress {
    finish: AtomicBool,
    bots: Vec<BotCounters>,
    crossings: watch::Sender<Option<LineCrossing>>,
}

impl Progress {
    /// For a scenario of `bots` bots.
    pub fn new(bots: usize) -> Arc<Self> {
        Arc::new(Self {
            finish: AtomicBool::new(false),
            bots: (0..bots).map(|_| BotCounters::default()).collect(),
            crossings: watch::channel(None).0,
        })
    }

    /// Tells the bots to stop playing where they are; the scenario then draws its
    /// conclusions.
    pub fn finish(&self) {
        self.finish.store(true, Ordering::Relaxed);
    }

    fn finishing(&self) -> bool {
        self.finish.load(Ordering::Relaxed)
    }

    /// Every bot as last heard of.
    pub fn bots(&self) -> Vec<BotProgress> {
        self.bots
            .iter()
            .map(|bot| BotProgress {
                playing: bot.playing.load(Ordering::Relaxed),
                x: f64::from_bits(bot.x.load(Ordering::Relaxed)),
                sent: bot.sent.load(Ordering::Relaxed),
                acknowledged: bot.acknowledged.load(Ordering::Relaxed),
            })
            .collect()
    }

    /// Changes whenever a bot is about to step across a boundary.
    pub fn crossings(&self) -> watch::Receiver<Option<LineCrossing>> {
        self.crossings.subscribe()
    }
}

/// A block of a plot, by x and z; all are at the height of [`GROUND`].
type Cell = (i32, i32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Place,
    Break,
}

/// How far the server has got with an action.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Stage {
    /// Not acknowledged yet.
    Sent,
    /// Acknowledged, but the block is not shown as the action leaves it yet.
    Acknowledged(Instant),
    /// Acknowledged and shown.
    Shown,
}

/// A line of a ledger.
#[derive(Debug, Clone)]
struct Entry {
    sequence: i32,
    kind: Kind,
    cell: Cell,
    /// What the block is once the action has been handled.
    expected: i32,
    /// Where the bot stood.
    from: f64,
    sent: Instant,
    stage: Stage,
    /// How long the acknowledgement took.
    waited: Option<Duration>,
}

impl std::fmt::Display for Entry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self.kind {
            Kind::Place => "placing",
            Kind::Break => "breaking",
        };
        write!(
            formatter,
            "action {} ({kind} at x = {}, z = {} from x = {:.2}, to leave block state {})",
            self.sequence, self.cell.0, self.cell.1, self.from, self.expected
        )
    }
}

/// What a bot hands in when it has stopped playing.
struct Finished {
    bot: usize,
    name: String,
    /// The entity the bot was told it is when it joined.
    entity_id: i32,
    /// The x and z coordinates the bot stands at.
    location: (f64, f64),
    entries: Vec<Entry>,
    /// The last word about every block of the bot's plot.
    words: BTreeMap<Cell, i32>,
    crossings: u32,
    hotbar_changes: u32,
    shown_late: u32,
}

/// What all the bots handed in, for each of them to compare with what it sees.
struct Conclusions {
    /// The last word about every block, and the name of the bot whose word it is.
    words: BTreeMap<Cell, (i32, String)>,
    /// Every bot with where it stands.
    bots: Vec<Standing>,
}

/// Where a bot stands when it has stopped playing, and as which entity.
struct Standing {
    name: String,
    entity_id: i32,
    /// The x and z coordinates.
    location: (f64, f64),
}

impl Standing {
    /// What `bot` sees differently, if anything: the player has to be the entity it
    /// was from the start, in the place where it stands.
    fn fault(&self, bot: &Bot) -> Option<String> {
        let (x, z) = self.location;
        let seen = bot
            .entities
            .get(&self.entity_id)
            .filter(|entity| bot.player_list.get(&entity.uuid) == Some(&self.name))
            .map(|entity| entity.position);
        (seen != Some((x, f64::from(GROUND), z))).then(|| {
            format!(
                "{} stands at x = {x}, z = {z} as entity {} and is seen there as {seen:?}; \
                 by name they are seen as {:?}",
                self.name,
                self.entity_id,
                bot.seen_player(&self.name)
            )
        })
    }

    /// Whether someone in the chunk `from` surely has this bot in view.
    fn surely_in_view(&self, from: (i32, i32), view_distance: i32) -> bool {
        let (x, z) = self.location;
        surely_in_view(from, chunk_of(x, z), view_distance)
    }
}

/// The name of the bot numbered `number`.
fn bot_name(prefix: &str, number: usize) -> String {
    format!("{prefix}Ledger{number}")
}

/// Runs the ledger scenario against the server at `address`. Fails if any of what the
/// module documentation lists does not hold. `progress` has to be made for as many
/// bots as `ledger` has.
pub async fn ledger(
    address: &str,
    ledger: &Ledger,
    progress: &Arc<Progress>,
) -> Result<LedgerReport> {
    ensure!(ledger.bots > 0, "the ledger scenario needs a bot");
    ensure!(
        progress.bots.len() == ledger.bots,
        "the progress is for another number of bots"
    );
    ensure!(
        ledger.west < ledger.east && ledger.speed > 0.0,
        "the bots need somewhere to walk"
    );

    let (finished_in, mut finished_out) = mpsc::channel(ledger.bots);
    let (checked_in, mut checked_out) = mpsc::channel(ledger.bots);
    let (conclude, _) = watch::channel(None::<Arc<Conclusions>>);
    let (leave, _) = watch::channel(false);
    let mut bots = JoinSet::new();
    for number in 0..ledger.bots {
        let name = bot_name(&ledger.name_prefix, number);
        let (address, ledger, progress) = (address.to_owned(), ledger.clone(), progress.clone());
        let finished = finished_in.clone();
        let checked = checked_in.clone();
        let conclusions = conclude.subscribe();
        let leave = leave.subscribe();
        bots.spawn(async move {
            let mut player = Player::join(&address, number, name.clone(), &ledger, progress)
                .await
                .with_context(|| format!("{name} could not join"))?;
            player
                .run(finished, conclusions, checked, leave)
                .await
                .with_context(|| format!("{name} failed"))
        });
    }
    drop((finished_in, checked_in));

    // Until every bot has stopped playing. A bot that ends before it is told to leave
    // has failed, and so has the scenario.
    let mut finished: Vec<Finished> = Vec::new();
    while finished.len() < ledger.bots {
        tokio::select! {
            one = finished_out.recv() => finished.push(one.context("the bots are gone")?),
            Some(ended) = bots.join_next() => {
                ended??;
                bail!("a bot left before it was told to");
            }
        }
    }
    finished.sort_by_key(|bot| bot.bot);

    let mut words = BTreeMap::new();
    for bot in &finished {
        for (cell, state) in &bot.words {
            let earlier = words.insert(*cell, (*state, bot.name.clone()));
            ensure!(earlier.is_none(), "two bots built at {cell:?}");
        }
    }
    let conclusions = Arc::new(Conclusions {
        words,
        bots: finished
            .iter()
            .map(|bot| Standing {
                name: bot.name.clone(),
                entity_id: bot.entity_id,
                location: bot.location,
            })
            .collect(),
    });

    // Every bot compares what it sees with what all of them were told.
    conclude.send_replace(Some(conclusions.clone()));
    let mut checked = 0;
    while checked < ledger.bots {
        tokio::select! {
            one = checked_out.recv() => {
                one.context("the bots are gone")?;
                checked += 1;
            }
            Some(ended) = bots.join_next() => {
                ended??;
                bail!("a bot left before it was told to");
            }
        }
    }

    // And so does someone who has seen nothing of it. The bots stay meanwhile, and
    // have to be heard: one of them failing now is as much a failure as before.
    let auditing = audit(address, ledger, &conclusions);
    tokio::pin!(auditing);
    let blocks_audited = tokio::select! {
        audited = &mut auditing => audited.context("the auditor found fault")?,
        Some(ended) = bots.join_next() => {
            ended??;
            bail!("a bot left before it was told to");
        }
    };

    leave.send_replace(true);
    while let Some(ended) = bots.join_next().await {
        ended??;
    }

    let entries = || finished.iter().flat_map(|bot| &bot.entries);
    let count = |kind: Kind| entries().filter(|entry| entry.kind == kind).count() as u32;
    let (longest_wait, longest_wait_of) = finished
        .iter()
        .flat_map(|bot| {
            bot.entries
                .iter()
                .map(|entry| (entry.waited.unwrap_or_default(), &bot.name, entry))
        })
        .max_by_key(|(waited, _, _)| *waited)
        .map(|(waited, name, entry)| (waited, format!("{name}'s {entry}")))
        .unwrap_or_default();
    Ok(LedgerReport {
        actions: entries().count() as u32,
        placed: count(Kind::Place),
        broken: count(Kind::Break),
        across_a_line: entries()
            .filter(|entry| across(&ledger.lines, entry.from, entry.cell.0))
            .count() as u32,
        crossings: finished.iter().map(|bot| bot.crossings).sum(),
        hotbar_changes: finished.iter().map(|bot| bot.hotbar_changes).sum(),
        shown_late: finished.iter().map(|bot| bot.shown_late).sum(),
        longest_wait,
        longest_wait_of,
        blocks_audited,
        blocks: conclusions
            .words
            .iter()
            .map(|((x, z), (word, _))| ((*x, GROUND, *z), *word))
            .collect(),
    })
}

/// Whether a boundary lies between someone standing at `from` and the block at `x`.
fn across(lines: &[i32], from: f64, x: i32) -> bool {
    lines
        .iter()
        .any(|line| (from < f64::from(*line)) != (x < *line))
}

/// The chunk that has the block column at `x` and `z`.
fn chunk_of(x: f64, z: f64) -> (i32, i32) {
    ((x.floor() as i32) >> 4, (z.floor() as i32) >> 4)
}

/// Whether someone in the chunk `from` with the view distance `view_distance` surely
/// has the chunk `to` in view, wherever in their chunks the two are.
fn surely_in_view(from: (i32, i32), to: (i32, i32), view_distance: i32) -> bool {
    (from.0 - to.0).abs().max((from.1 - to.1).abs()) < view_distance
}

/// One bot of the scenario with what it believes and what it has been told.
struct Player {
    bot: Bot,
    number: usize,
    name: String,
    ledger: Ledger,
    progress: Arc<Progress>,
    random: Random,
    /// The z coordinate of the lane, and of the two rows of the plot beside it.
    lane: f64,
    rows: [i32; 2],
    /// The slot and the items the bot believes to hold, and what the server last said
    /// about each, which it says rarely.
    slot: i16,
    hotbar: [Option<ItemStack>; 9],
    told_slot: i32,
    told_hotbar: [Option<ItemStack>; 9],
    entries: Vec<Entry>,
    /// The entries the server has not both acknowledged and shown yet, as indices.
    open: Vec<usize>,
    /// The last word about each block of the plot: how the bot found it, or what the
    /// server said after the last action on it.
    words: BTreeMap<Cell, i32>,
    crossings: u32,
    hotbar_changes: u32,
    shown_late: u32,
}

impl Player {
    /// Joins and waits to be in the world.
    async fn join(
        address: &str,
        number: usize,
        name: String,
        ledger: &Ledger,
        progress: Arc<Progress>,
    ) -> Result<Self> {
        let mut bot = Bot::join(address, &name).await?;
        bot.wait_for_chunks(1, ledger.patience).await?;
        let lane_block = number as i32 * LANE_SPACING;
        Ok(Self {
            number,
            name,
            ledger: ledger.clone(),
            progress,
            // Every bot chooses differently, and all of them by the seed.
            random: Random::new(
                ledger.seed ^ (number as u64 + 1).wrapping_mul(0xA24B_AED4_963E_E407),
            ),
            lane: f64::from(lane_block) + 0.5,
            rows: [lane_block + 1, lane_block + 2],
            slot: bot.selected_slot as i16,
            hotbar: bot.hotbar,
            told_slot: bot.selected_slot,
            told_hotbar: bot.hotbar,
            entries: Vec::new(),
            open: Vec::new(),
            words: BTreeMap::new(),
            crossings: 0,
            hotbar_changes: 0,
            shown_late: 0,
            bot,
        })
    }

    /// Plays until the scenario is over; see [`ledger`] for the order of things.
    async fn run(
        &mut self,
        finished: mpsc::Sender<Finished>,
        mut conclusions: watch::Receiver<Option<Arc<Conclusions>>>,
        checked: mpsc::Sender<()>,
        mut leave: watch::Receiver<bool>,
    ) -> Result<()> {
        self.play().await?;
        let handed_in = Finished {
            bot: self.number,
            name: self.name.clone(),
            entity_id: self.bot.info.login.entity_id,
            location: (self.bot.location.0, self.bot.location.2),
            entries: self.entries.clone(),
            words: self.words.clone(),
            crossings: self.crossings,
            hotbar_changes: self.hotbar_changes,
            shown_late: self.shown_late,
        };
        // Nobody listens any more if the scenario has failed elsewhere.
        let _ = finished.send(handed_in).await;

        // Stays, and goes on watching, until told to leave. Whoever vanishes after that
        // has left.
        let mut compared = false;
        loop {
            if *leave.borrow_and_update() {
                return Ok(());
            }
            self.look_around()?;
            let concluded = conclusions.borrow_and_update().clone();
            if let Some(concluded) = concluded.filter(|_| !compared) {
                self.compare(&concluded).await?;
                compared = true;
                let _ = checked.send(()).await;
                continue;
            }
            tokio::select! {
                told = leave.changed() => {
                    // Nobody is left to tell: the scenario has failed elsewhere.
                    if told.is_err() {
                        return Ok(());
                    }
                }
                _ = conclusions.changed(), if !compared => {}
                idled = self.bot.idle(TICK) => idled.context("disconnected while waiting")?,
            }
        }
    }

    /// Walks to the lane and back and forth along it, building, until the rounds are
    /// walked, the time is up or the scenario is told to finish. Then waits for the
    /// server to have dealt with everything.
    async fn play(&mut self) -> Result<()> {
        let started = Instant::now();
        // Along the spawn point's x first, where nobody builds, so that no bot walks
        // through another's plot.
        let speed = self.ledger.speed * (1.0 + self.number as f64 * 0.17);
        let x = self.bot.location.0;
        self.bot.walk_to(x, self.lane, TO_THE_LANE).await?;
        self.bot
            .walk_to(self.ledger.west, self.lane, TO_THE_LANE)
            .await?;
        self.look_around()?;
        // How the bot finds its plot is the first word about every block of it that is
        // in view, so that a block nobody touched is noticed if it changes.
        let (west, east) = (
            self.ledger.west.floor() as i32,
            self.ledger.east.floor() as i32,
        );
        for x in west - REACH..=east + REACH {
            for z in self.rows {
                if let Some(found) = self.bot.block_at(x, GROUND, z)? {
                    self.words.insert((x, z), found);
                }
            }
        }
        self.progress.bots[self.number]
            .playing
            .store(true, Ordering::Relaxed);

        let mut round = 0;
        'playing: loop {
            for end in [self.ledger.east, self.ledger.west] {
                // Steps do not add up to the length of the lane exactly; the last one
                // ends on the spot.
                while (self.bot.location.0 - end).abs() > ARRIVED {
                    let over = self.progress.finishing()
                        || self.ledger.rounds.is_some_and(|rounds| round >= rounds)
                        || self
                            .ledger
                            .duration
                            .is_some_and(|duration| started.elapsed() >= duration);
                    if over {
                        break 'playing;
                    }
                    self.act().await?;
                    self.step_towards(end, speed).await?;
                    self.settle()?;
                }
                self.still_as_told()?;
            }
            round += 1;
        }
        self.progress.bots[self.number]
            .playing
            .store(false, Ordering::Relaxed);

        while !self.open.is_empty() {
            self.bot
                .idle(TICK)
                .await
                .context("disconnected while waiting for acknowledgements")?;
            self.settle()?;
        }
        self.still_as_told()?;
        // The chunks follow a player; once they have, the server knows where the bot is.
        let own_chunk = chunk_of(self.bot.location.0, self.bot.location.2);
        self.bot
            .wait_until(self.ledger.patience, |bot| {
                bot.center == Some(own_chunk) && bot.chunks.contains_key(&own_chunk)
            })
            .await
            .context("the chunks around the player did not follow")?;
        self.settle()
    }

    /// One step along the lane towards `end`, which takes a client tick.
    async fn step_towards(&mut self, end: f64, speed: f64) -> Result<()> {
        let from = self.bot.location.0;
        let to = if (end - from).abs() <= speed + ARRIVED {
            end
        } else {
            from + speed.copysign(end - from)
        };
        for line in &self.ledger.lines {
            let line_x = f64::from(*line);
            if (from < line_x) != (to < line_x) {
                self.crossings += 1;
                self.progress.crossings.send_modify(|crossing| {
                    *crossing = Some(LineCrossing {
                        bot: self.number,
                        line: *line,
                        eastwards: to > from,
                        count: (*crossing).map_or(0, |earlier| earlier.count) + 1,
                    });
                });
            }
        }
        self.bot
            .walk_to(to, self.lane, speed)
            .await
            .with_context(|| format!("disconnected while walking at x = {from:.2}"))
    }

    /// Does one of the things a player does, or more often nothing.
    async fn act(&mut self) -> Result<()> {
        match self.random.below(40) {
            0 => {
                let slot = self.random.below(9) as i16;
                let item = PALETTE[self.random.below(PALETTE.len() as u64) as usize];
                self.bot.take_from_creative_inventory(slot, item).await?;
                self.hotbar[slot as usize] = Some(ItemStack { item, count: 1 });
                self.hotbar_changes += 1;
            }
            1 | 2 => {
                self.slot = self.random.below(9) as i16;
                self.bot.select_slot(self.slot).await?;
                self.hotbar_changes += 1;
            }
            3..=10 => self.build().await?,
            _ => {}
        }
        Ok(())
    }

    /// Places a block on a free block of the plot within reach, or breaks the block
    /// that is there, and notes it in the ledger.
    async fn build(&mut self) -> Result<()> {
        if self.open.len() >= MOST_PENDING {
            return Ok(());
        }
        let from = self.bot.location.0;
        let mut x = from.floor() as i32 + self.random.below(2 * REACH as u64 + 1) as i32 - REACH;
        // Next to a boundary, half of what a bot does is beyond it: that is what takes
        // two regions, and there is little room for it.
        let near = self
            .ledger
            .lines
            .iter()
            .find(|line| (from - f64::from(**line)).abs() <= f64::from(REACH) + 0.5);
        if let Some(line) = near
            && self.random.one_in(2)
        {
            let beyond = self.random.below(REACH as u64) as i32;
            x = if from < f64::from(*line) {
                line + beyond
            } else {
                line - 1 - beyond
            };
        }
        let z = self.rows[self.random.below(2) as usize];
        let cell = (x, z);
        // One thing at a time with a block, so that what it should be is never in doubt.
        if self
            .open
            .iter()
            .any(|open| self.entries[*open].cell == cell)
        {
            return Ok(());
        }
        let word = match self.words.get(&cell) {
            Some(word) => *word,
            None => {
                // How the bot finds a block is the first word about it.
                let Some(found) = self.bot.block_at(x, GROUND, z)? else {
                    return Ok(());
                };
                self.words.insert(cell, found);
                found
            }
        };
        let (kind, expected, sequence) = if word == AIR {
            let held = self.hotbar[self.slot as usize]
                .and_then(|stack| ITEMS.get(usize::try_from(stack.item).ok()?)?.block);
            let Some(block) = held else {
                return Ok(());
            };
            let sequence = self.bot.use_item_on(x, GROUND - 1, z, face::TOP).await?;
            (Kind::Place, i32::from(block.0), sequence)
        } else {
            (Kind::Break, AIR, self.bot.dig(x, GROUND, z).await?)
        };
        self.open.push(self.entries.len());
        self.entries.push(Entry {
            sequence,
            kind,
            cell,
            expected,
            from,
            sent: Instant::now(),
            stage: Stage::Sent,
            waited: None,
        });
        Ok(())
    }

    /// Notes what the server has acknowledged and shown since the last look, and fails
    /// if it has taken too long over something or said something that cannot be.
    fn settle(&mut self) -> Result<()> {
        let now = Instant::now();
        let patience = self.ledger.patience;
        let mut still_open = Vec::new();
        for index in std::mem::take(&mut self.open) {
            let entry = &mut self.entries[index];
            let (x, z) = entry.cell;
            if entry.stage == Stage::Sent && self.bot.acknowledged_sequence >= entry.sequence {
                entry.waited = Some(now - entry.sent);
                entry.stage = Stage::Acknowledged(now);
            }
            match entry.stage {
                Stage::Sent => ensure!(
                    now - entry.sent <= patience,
                    "{entry} was not acknowledged within {patience:?}; \
                     the server has acknowledged up to {}",
                    self.bot.acknowledged_sequence
                ),
                Stage::Acknowledged(since) => {
                    let seen = self.bot.block_at(x, GROUND, z)?;
                    if seen == Some(entry.expected) {
                        if since != now {
                            self.shown_late += 1;
                        }
                        entry.stage = Stage::Shown;
                        self.words.insert(entry.cell, entry.expected);
                    } else {
                        ensure!(
                            now - since <= patience,
                            "{entry} was acknowledged after {:?}, but {patience:?} later \
                             the block is shown as {seen:?}: the action was lost",
                            entry.waited.unwrap_or_default()
                        );
                    }
                }
                Stage::Shown => {}
            }
            if entry.stage != Stage::Shown {
                still_open.push(index);
            }
        }
        self.open = still_open;

        let counters = &self.progress.bots[self.number];
        counters
            .x
            .store(self.bot.location.0.to_bits(), Ordering::Relaxed);
        counters.sent.store(
            self.entries.last().map_or(0, |entry| entry.sequence),
            Ordering::Relaxed,
        );
        counters
            .acknowledged
            .store(self.bot.acknowledged_sequence, Ordering::Relaxed);
        self.look_around()
    }

    /// Fails if the server has moved the bot, has said that it holds something else
    /// than it believes, or has taken another bot's entity away from close by.
    fn look_around(&mut self) -> Result<()> {
        // A server that is unsure where a player is puts them somewhere, which a client
        // sees as being moved. That happens once, on joining.
        ensure!(
            self.bot.stats.teleports_confirmed == 1,
            "the server moved the player {} times, last to {:?}",
            self.bot.stats.teleports_confirmed,
            self.bot.location
        );
        if self.bot.selected_slot != self.told_slot {
            self.told_slot = self.bot.selected_slot;
            ensure!(
                self.told_slot == i32::from(self.slot),
                "the server says slot {} is held, the bot selected slot {}",
                self.told_slot,
                self.slot
            );
        }
        if self.bot.hotbar != self.told_hotbar {
            self.told_hotbar = self.bot.hotbar;
            ensure!(
                self.told_hotbar == self.hotbar,
                "the server says the hotbar holds {:?}, the bot put {:?} there",
                self.told_hotbar,
                self.hotbar
            );
        }

        let own_chunk = chunk_of(self.bot.location.0, self.bot.location.2);
        let view_distance = self.bot.info.login.view_distance;
        let others = format!("{}Ledger", self.ledger.name_prefix);
        for gone in std::mem::take(&mut self.bot.vanished) {
            let Some(name) = self.bot.player_list.get(&gone.uuid) else {
                continue;
            };
            // Someone who is not part of this is free to leave.
            if gone.kind != entity_types::PLAYER || !name.starts_with(&others) {
                continue;
            }
            let (x, _, z) = gone.position;
            // A bot can have walked out of view only from the rim of what is in view.
            ensure!(
                !surely_in_view(own_chunk, chunk_of(x, z), view_distance),
                "the entity of {name}, who is still playing, vanished at x = {x:.2}, z = {z:.2} \
                 while {} stood at x = {:.2}, z = {:.2}",
                self.name,
                self.bot.location.0,
                self.bot.location.2
            );
        }
        Ok(())
    }

    /// Fails if a block of the plot that nothing is under way for is shown as anything
    /// but the last word about it.
    fn still_as_told(&self) -> Result<()> {
        let open: BTreeSet<Cell> = self
            .open
            .iter()
            .map(|index| self.entries[*index].cell)
            .collect();
        for (cell, word) in &self.words {
            if open.contains(cell) {
                continue;
            }
            // A chunk that is out of view says nothing.
            if let Some(seen) = self.bot.block_at(cell.0, GROUND, cell.1)? {
                ensure!(
                    seen == *word,
                    "the block at x = {}, z = {} is shown as {seen} after the server said it \
                     is {word}; the ledger about it: {}",
                    cell.0,
                    cell.1,
                    self.history(*cell)
                );
            }
        }
        Ok(())
    }

    /// What the ledger has about a block, for a message.
    fn history(&self, cell: Cell) -> String {
        let lines: Vec<String> = self
            .entries
            .iter()
            .filter(|entry| entry.cell == cell)
            .map(|entry| format!("{entry} {:?}", entry.stage))
            .collect();
        if lines.is_empty() {
            "nothing".to_owned()
        } else {
            lines.join("; ")
        }
    }

    /// Waits until the bot sees every block it has in view as the ledgers' last word
    /// says and every other bot in view where it stands, and fails if that does not
    /// come about.
    async fn compare(&mut self, concluded: &Conclusions) -> Result<()> {
        let own_chunk = chunk_of(self.bot.location.0, self.bot.location.2);
        let view_distance = self.bot.info.login.view_distance;
        let in_view: Vec<&Standing> = concluded
            .bots
            .iter()
            .filter(|other| {
                other.name != self.name && other.surely_in_view(own_chunk, view_distance)
            })
            .collect();
        let faults = |bot: &Bot| -> Vec<String> {
            let mut faults = Vec::new();
            for (cell, (word, whose)) in &concluded.words {
                match bot.block_at(cell.0, GROUND, cell.1) {
                    Ok(None) => {}
                    Ok(Some(seen)) if seen == *word => {}
                    seen => faults.push(format!(
                        "the block at x = {}, z = {} is shown as {seen:?}, {whose}'s ledger says {word}",
                        cell.0, cell.1
                    )),
                }
            }
            faults.extend(in_view.iter().filter_map(|other| other.fault(bot)));
            faults
        };
        let waited = self
            .bot
            .wait_until(self.ledger.patience, |bot| faults(bot).is_empty())
            .await;
        if let Err(error) = waited {
            bail!(
                "{error:#}; what {} sees differs from what everyone was told: {}",
                self.name,
                faults(&self.bot).join("; ")
            );
        }
        Ok(())
    }
}

/// Joins as someone who has seen nothing of what went before, walks along the lanes
/// and fails unless every block is what the ledgers' last word about it says and every
/// bot stands where it says it does. Returns how many blocks it compared.
async fn audit(address: &str, ledger: &Ledger, concluded: &Conclusions) -> Result<u32> {
    let mut auditor = walk_and_compare(address, ledger, &concluded.words).await?;

    // And the bots are where they say they are, as far as they are in view from here.
    let own_chunk = chunk_of(auditor.location.0, auditor.location.2);
    let view_distance = auditor.info.login.view_distance;
    let misplaced = |bot: &Bot| -> Vec<String> {
        concluded
            .bots
            .iter()
            .filter(|other| other.surely_in_view(own_chunk, view_distance))
            .filter_map(|other| other.fault(bot))
            .collect()
    };
    if let Err(error) = auditor
        .wait_until(ledger.patience, |bot| misplaced(bot).is_empty())
        .await
    {
        bail!("{error:#}: {}", misplaced(&auditor).join("; "));
    }
    Ok(concluded.words.len() as u32)
}

/// Joins a server and fails unless every block of `blocks`, by x, y and z, is the block
/// state it is given as: what [`LedgerReport::blocks`] holds, looked at again, for
/// example after everything that served the world has been started anew. `ledger` is
/// what the scenario was run with.
pub async fn audit_blocks(
    address: &str,
    ledger: &Ledger,
    blocks: &BTreeMap<(i32, i32, i32), i32>,
) -> Result<()> {
    ensure!(
        blocks.keys().all(|(_, y, _)| *y == GROUND),
        "the ledgers only have blocks on the ground"
    );
    let words = blocks
        .iter()
        .map(|((x, _, z), word)| ((*x, *z), (*word, "a bot".to_owned())))
        .collect();
    walk_and_compare(address, ledger, &words).await?;
    Ok(())
}

/// Joins as the auditor, walks along the lanes and fails unless every block is what
/// the last word about it says. Returns the auditor, standing in the middle of the
/// lanes.
async fn walk_and_compare(
    address: &str,
    ledger: &Ledger,
    words: &BTreeMap<Cell, (i32, String)>,
) -> Result<Bot> {
    let name = format!("{}Auditor", ledger.name_prefix);
    let mut auditor = Bot::join(address, &name)
        .await
        .context("the auditor could not join")?;
    auditor.wait_for_chunks(1, ledger.patience).await?;

    // Past everything that was built, at a pace that lets the chunks keep up, on the
    // lane the spawn point is on.
    let z = auditor.location.2;
    let mut seen: BTreeMap<Cell, i32> = BTreeMap::new();
    let middle = (ledger.west + ledger.east) / 2.0;
    for stop in [ledger.west, ledger.east, middle] {
        auditor
            .walk_to(stop, z, 0.8)
            .await
            .context("the auditor was disconnected")?;
        let own_chunk = chunk_of(auditor.location.0, auditor.location.2);
        auditor
            .wait_until(ledger.patience, |bot| {
                bot.center == Some(own_chunk) && bot.chunks.contains_key(&own_chunk)
            })
            .await
            .context("the chunks around the auditor did not follow")?;
        // What is in view now and was not before is as the server has it now; what was
        // in view all along would have been updated had it changed, and nobody builds
        // any more.
        for cell in words.keys() {
            if let Some(state) = auditor.block_at(cell.0, GROUND, cell.1)? {
                seen.insert(*cell, state);
            }
        }
    }

    let mut faults = Vec::new();
    for (cell, (word, whose)) in words {
        match seen.get(cell) {
            Some(state) if state == word => {}
            Some(state) => faults.push(format!(
                "the block at x = {}, z = {} is {state}, {whose}'s ledger says {word}",
                cell.0, cell.1
            )),
            None => faults.push(format!(
                "the block at x = {}, z = {} never came into view",
                cell.0, cell.1
            )),
        }
    }
    ensure!(
        faults.is_empty(),
        "{} of {} blocks are not what the ledgers say: {}",
        faults.len(),
        words.len(),
        faults.join("; ")
    );
    Ok(auditor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_numbers() {
        let numbers = |seed| {
            let mut random = Random::new(seed);
            (0..8).map(|_| random.below(1000)).collect::<Vec<_>>()
        };
        assert_eq!(numbers(7), numbers(7));
        assert_ne!(numbers(7), numbers(8));
    }

    #[test]
    fn an_action_is_across_a_line_when_the_line_is_between_the_player_and_the_block() {
        assert!(across(&[48], 47.5, 48));
        assert!(across(&[48], 48.2, 47));
        assert!(!across(&[48], 47.5, 47));
        assert!(!across(&[48], 48.0, 49));
        assert!(!across(&[], 47.5, 48));
    }

    #[test]
    fn only_the_rim_of_what_is_in_view_is_not_surely_in_view() {
        assert!(surely_in_view((2, 0), (4, 0), 3));
        assert!(surely_in_view((2, 0), (0, 2), 3));
        assert!(!surely_in_view((2, 0), (5, 0), 3));
    }
}
